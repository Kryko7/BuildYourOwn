//! Stage 11 — Nothing in common: `handshake_failure`.

use crate::assert::{Check, Failure};
use crate::examples::{ExampleEnv, ExampleSpec, Expect};
use crate::stages::{check_refused_with, provoke, Stage, Test};
use crate::tls::client::{build_hello, ClientConfig};
use crate::tls::conn::{Incoming, Reaction};
use crate::tls::msg::ServerHello;
use crate::tls::record::Record;
use crate::tls::{group_name, AlertDescription, HandshakeType, TlsError, LEGACY_VERSION_TLS12};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 11,
        slug: "handshake_failure",
        name: "No acceptable parameters: handshake_failure",
        ext: false,
        hints: &[
            "When no offered cipher suite is acceptable, the answer is a fatal \
             handshake_failure(40) — not a ServerHello with a suite the client never offered",
            "The same alert covers a client with no acceptable group and no acceptable \
             signature algorithm",
            "Send the alert in the clear: there are no handshake keys on a connection that \
             never got past the hello",
            "Close the connection after a fatal alert; there is no state left worth keeping",
        ],
        examples,
        tests: vec![
            Test::new(
                "a hello offering only TLS 1.2 suite numbers is refused",
                only_tls12_suites,
            ),
            Test::new(
                "a hello offering an empty cipher suite list is refused",
                empty_suites,
            ),
            Test::new(
                "a hello offering only unknown suite numbers is refused",
                unknown_suites,
            ),
            Test::new(
                "the refusal is a fatal alert, never a warning",
                fatal_not_warning,
            ),
            Test::new(
                "a hello offering only ffdhe2048 is refused or retried, never answered",
                no_common_group,
            ),
            Test::new(
                "the server still serves an acceptable client afterwards",
                still_serving,
            ),
        ],
    }
}

async fn refuse_hello(
    ctx: &mut crate::stages::Ctx,
    hello: Vec<u8>,
    what: &str,
) -> Result<crate::tls::conn::Reaction, crate::assert::Failure> {
    let mut conn = ctx.connect().await?;
    let reaction = provoke(&mut conn, &Record::build(22, LEGACY_VERSION_TLS12, &hello)).await;
    let mut c = Check::new(format!("what the server does with {what}"));
    c.block("client_hello", &hello);
    check_refused_with(
        &mut c,
        "the server's reaction",
        &reaction,
        &[
            AlertDescription::HANDSHAKE_FAILURE,
            AlertDescription::ILLEGAL_PARAMETER,
            AlertDescription::INSUFFICIENT_SECURITY,
            AlertDescription::DECODE_ERROR,
            AlertDescription::PROTOCOL_VERSION,
        ],
    );
    c.finish()?;
    Ok(reaction)
}

/// The ClientHello a configuration describes, with the suite list replaced.
fn hello_with_suites(config: &ClientConfig, suites: &[u16]) -> Result<Vec<u8>, String> {
    let (mut hello, _) = build_hello(config).map_err(|e| e.to_string())?;
    hello.cipher_suites = suites.to_vec();
    Ok(hello.encode())
}

tls_test!(only_tls12_suites, |ctx| {
    let hello = hello_with_suites(&ctx.config(), &[0xc02f, 0xc030, 0x009c])
        .map_err(crate::stages::harness)?;
    refuse_hello(ctx, hello, "a hello offering only TLS 1.2 suites").await?;
    Ok(())
});

tls_test!(empty_suites, |ctx| {
    let hello = hello_with_suites(&ctx.config(), &[]).map_err(crate::stages::harness)?;
    refuse_hello(ctx, hello, "a hello with an empty cipher_suites list").await?;
    Ok(())
});

tls_test!(unknown_suites, |ctx| {
    let hello = hello_with_suites(&ctx.config(), &[0x9a9a, 0xdead, 0xbeef])
        .map_err(crate::stages::harness)?;
    refuse_hello(ctx, hello, "a hello offering three invented suite numbers").await?;
    Ok(())
});

tls_test!(fatal_not_warning, |ctx| {
    let hello = hello_with_suites(&ctx.config(), &[0xc02f]).map_err(crate::stages::harness)?;
    let reaction = refuse_hello(ctx, hello, "a hello with no common suite").await?;
    let mut c = Check::new("the level of the alert that refuses a hello");
    match reaction.alert() {
        Some(alert) => {
            c.note(
                "TLS 1.3 has only two warning-level alerts, close_notify and user_canceled; \
                 everything else is fatal.",
            );
            c.eq("alert.level", 2u8, alert.level.as_u8());
            // RFC 8446 section 4.1.1: "either a handshake_failure or an
            // insufficient_security fatal alert".
            c.that(
                "alert.description",
                "handshake_failure(40) or insufficient_security(71)",
                alert.description == AlertDescription::HANDSHAKE_FAILURE
                    || alert.description == AlertDescription::INSUFFICIENT_SECURITY,
                alert.description.name(),
            );
        }
        None => {
            c.note("the server closed without an alert, which RFC 8446 allows");
        }
    }
    c.finish()
});

tls_test!(no_common_group, |ctx| {
    // Offer only ffdhe2048 and send no key share at all. RFC 8446 leaves the server two
    // correct answers here: refuse with handshake_failure, or ask for a share with a
    // HelloRetryRequest if it does support the group. The reference supports ffdhe2048 and
    // takes the second road, so the check allows both and records which one happened.
    let mut config = ctx.config();
    config.groups = vec![crate::tls::GROUP_FFDHE2048];
    config.share_groups = vec![];
    let (hello, _) = build_hello(&config).map_err(|e| crate::stages::harness(e.to_string()))?;
    let bytes = hello.encode();
    let mut conn = ctx.connect().await?;
    conn.write_raw(&Record::build(22, LEGACY_VERSION_TLS12, &bytes))
        .await
        .map_err(Failure::tls)?;
    // A bare "some message arrived" would also pass the ServerHello this test exists to
    // catch, so the answer is read as a message and only a HelloRetryRequest for a group
    // the client named counts as asking it to try again.
    let reaction = match conn
        .next_message_within(std::time::Duration::from_millis(crate::stages::REACTION_MS))
        .await
    {
        Ok(Incoming::Handshake(m)) if m.msg_type == HandshakeType::SERVER_HELLO => {
            match ServerHello::parse(&m.body) {
                Ok(h) if h.is_hello_retry_request() => match h.retry_group() {
                    Ok(Some(g)) if config.groups.contains(&g) => {
                        Reaction::Message(format!("hello_retry_request for {}", group_name(g)))
                    }
                    other => Reaction::Error(format!(
                        "a hello_retry_request whose key_share is {other:?}, not a group the \
                         client offered"
                    )),
                },
                Ok(_) => Reaction::Error(
                    "a full server_hello, though the client sent no key share it could use".into(),
                ),
                Err(e) => Reaction::Error(format!("an unparsable server_hello: {e}")),
            }
        }
        Ok(Incoming::Alert(a)) => Reaction::Alert(a),
        Ok(other) => Reaction::Error(format!("unexpected {}", other.describe())),
        Err(TlsError::Closed) => Reaction::Closed,
        Err(TlsError::Timeout(_)) => Reaction::Silence,
        Err(e) => Reaction::Error(e.to_string()),
    };
    let mut c = Check::new("a hello whose only group is ffdhe2048, with no key share");
    c.block("client_hello", &bytes);
    c.note(
        "Either answer is conformant: a fatal handshake_failure(40), or a HelloRetryRequest \
         naming a group the server can do. What is not conformant is a ServerHello with a \
         key_share for a group the client never offered.",
    );
    c.that(
        "the server's reaction",
        "a refusal, or a handshake message asking the client to try again",
        reaction.is_refusal() || matches!(reaction, Reaction::Message(_)),
        reaction.describe(),
    );
    c.finish()?;
    ctx.note(format!("the server answered: {}", reaction.describe()));
    Ok(())
});

tls_test!(still_serving, |ctx| {
    let hello = hello_with_suites(&ctx.config(), &[0xc02f]).map_err(crate::stages::harness)?;
    let mut conn = ctx.connect().await?;
    let _ = provoke(&mut conn, &Record::build(22, LEGACY_VERSION_TLS12, &hello)).await;
    drop(conn);
    ctx.expect_still_answering("a hello with no common cipher suite")
        .await
});

fn no_common_suite(env: &ExampleEnv) -> Result<Vec<u8>, String> {
    let hello = hello_with_suites(&env.config(), &[0xc02f, 0xc030])?;
    Ok(Record::build(22, LEGACY_VERSION_TLS12, &hello))
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::raw(
            "A hello with no cipher suite the server can use",
            no_common_suite,
            Expect::UntilAlert,
        )
        .request(
            "A TLS 1.3 ClientHello — right version, right extensions — whose cipher_suites \
             holds only the TLS 1.2 numbers 0xc02f and 0xc030",
        )
        .response(
            "A fatal alert: `02 28`, level fatal(2), description handshake_failure(40), in a \
             plaintext alert record. Then the connection closes.",
        )
        .note(
            "The temptation is to answer with a suite the server likes anyway. A client \
             checks that the returned suite was in its own list, so that handshake dies two \
             messages later with a much more confusing error.",
        ),
        ExampleSpec::text("Which alert for which disagreement")
            .request("No common cipher suite, no common group, no usable signature algorithm")
            .response(
                "handshake_failure(40) for all three. illegal_parameter(47) is for a hello that \
                 is malformed rather than merely incompatible.",
            )
            .note(
                "Do not invent a more specific alert. The distinction a client can act on is \
                 'we have nothing in common' versus 'your message is broken'.",
            ),
    ]
}
