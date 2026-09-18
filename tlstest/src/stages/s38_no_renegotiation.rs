//! Stage 38 — Renegotiation does not exist in TLS 1.3.

use crate::assert::{Check, Failure};
use crate::examples::{ExampleEnv, ExampleSpec, Expect};
use crate::stages::{check_refused_with, hello_message, provoke, Stage, Test};
use crate::tls::client::build_hello;
use crate::tls::msg::{encode_handshake, Extension};
use crate::tls::record::Record;
use crate::tls::{AlertDescription, ContentType, HandshakeType, LEGACY_VERSION_TLS12};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 38,
        slug: "no_renegotiation",
        name: "Renegotiation is gone",
        ext: false,
        hints: &[
            "TLS 1.3 removed renegotiation: there is no HelloRequest, and a ClientHello after \
             the handshake is unexpected_message(10)",
            "Key rotation is KeyUpdate and nothing else; a peer asking for new keys is not \
             asking to renegotiate anything",
            "The renegotiation_info extension and the SCSV may still arrive from old clients \
             — ignore them, do not act on them",
            "Post-handshake authentication exists, but only when the client offered \
             post_handshake_auth in its hello; without it a CertificateRequest is illegal",
        ],
        examples,
        tests: vec![
            Test::new(
                "a ClientHello after the handshake is refused",
                renegotiation_hello,
            ),
            Test::new(
                "a HelloRequest after the handshake is refused",
                hello_request,
            ),
            Test::new(
                "the server never sends a HelloRequest of its own",
                no_server_hello_request,
            ),
            Test::new(
                "the renegotiation_info extension is ignored, not acted on",
                renegotiation_info_ignored,
            ),
            Test::new(
                "the renegotiation SCSV in the suite list is ignored",
                scsv_ignored,
            ),
            Test::new(
                "a CertificateRequest the client never allowed is refused",
                unsolicited_certificate_request,
            ),
            Test::new(
                "the server still serves a new connection afterwards",
                still_serving,
            ),
        ],
    }
}

/// Send a handshake message after a completed handshake and see what happens.
async fn after_handshake(
    ctx: &mut crate::stages::Ctx,
    message: Vec<u8>,
    what: &str,
) -> Result<(), Failure> {
    let mut client = ctx.handshake().await?;
    client
        .echo_line("before")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    client
        .conn
        .write_record(ContentType::Handshake, &message)
        .await
        .map_err(Failure::tls)?;
    let reaction = crate::stages::reaction_past_tickets(&mut client.conn).await;
    let mut c = Check::new(format!("what the server does with {what}"));
    c.block("the message that was sent", &message);
    check_refused_with(
        &mut c,
        "the server's reaction",
        &reaction,
        &[
            AlertDescription::UNEXPECTED_MESSAGE,
            AlertDescription::DECODE_ERROR,
            AlertDescription::ILLEGAL_PARAMETER,
            AlertDescription::HANDSHAKE_FAILURE,
            AlertDescription::NO_RENEGOTIATION,
        ],
    );
    c.finish()?;
    drop(client);
    ctx.expect_still_serving(what).await
}

tls_test!(renegotiation_hello, |ctx| {
    let hello = hello_message(&ctx.config_n(3)).map_err(crate::stages::harness)?;
    after_handshake(ctx, hello, "a ClientHello sent after the handshake").await
});

tls_test!(hello_request, |ctx| {
    after_handshake(
        ctx,
        encode_handshake(HandshakeType::HELLO_REQUEST, &[]),
        "a TLS 1.2 HelloRequest sent after the handshake",
    )
    .await
});

tls_test!(no_server_hello_request, |ctx| {
    let mut client = ctx.handshake().await?;
    for i in 0..3 {
        let line = format!("quiet{i}");
        client.echo_line(&line).await.map_err(|e| {
            crate::stages::handshake_failure(e, &client).note(format!("on exchange {i}"))
        })?;
    }
    client
        .collect_tickets(std::time::Duration::from_millis(300))
        .await
        .ok();
    let types: Vec<String> = client
        .conn
        .trace
        .iter()
        .filter(|t| t.direction == crate::tls::conn::Direction::Received)
        .map(|t| t.what.clone())
        .collect();
    let mut c = Check::new("everything the server sent after the handshake");
    c.note(
        "A TLS 1.3 server has exactly two post-handshake messages: NewSessionTicket and \
         KeyUpdate. HelloRequest does not exist any more.",
    );
    c.observe("messages received", &types);
    c.that(
        "the server's post-handshake messages",
        "no hello_request among them",
        !types.iter().any(|t| t.contains("hello_request")),
        types.join(", "),
    );
    c.finish()
});

tls_test!(renegotiation_info_ignored, |ctx| {
    // Extension 0xff01 with an empty renegotiated_connection field, as a TLS 1.2 client
    // sends it. A 1.3 server has nothing to do with it.
    let mut config = ctx.config();
    config
        .extra_extensions
        .push(Extension::new(0xff01, vec![0u8]));
    let mut client = ctx.handshake_with(config).await?;
    let answer = client
        .echo_line("reneg")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("a hello carrying renegotiation_info");
    c.block("client_hello", &client.client_hello_bytes);
    c.note(
        "RFC 5746's extension is meaningless in TLS 1.3, and real clients still send it. \
         Skipping it is the same as skipping any other unknown extension.",
    );
    c.eq("echo", "gener".to_string(), answer);
    c.that(
        "encrypted_extensions",
        "no renegotiation_info echoed back",
        !client
            .encrypted_extensions
            .iter()
            .any(|e| e.ext_type == 0xff01),
        "the server echoed renegotiation_info",
    );
    c.finish()
});

tls_test!(scsv_ignored, |ctx| {
    // TLS_EMPTY_RENEGOTIATION_INFO_SCSV is 0x00ff: a cipher suite number that is not a
    // cipher suite.
    let mut suites = vec![0x00ffu16];
    suites.extend_from_slice(&crate::tls::ALL_SUITES);
    let mut client = ctx
        .handshake_with(ctx.config().with_suites(&suites))
        .await?;
    let answer = client
        .echo_line("scsv")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let chosen = client
        .server_hello
        .as_ref()
        .map(|h| h.cipher_suite)
        .unwrap_or(0);
    let mut c = Check::new("a hello whose first cipher suite is the SCSV");
    c.block("client_hello", &client.client_hello_bytes);
    c.ne("server_hello.cipher_suite", 0x00ffu16, chosen);
    c.that(
        "server_hello.cipher_suite",
        "a real TLS 1.3 suite",
        crate::tls::ALL_SUITES.contains(&chosen),
        crate::tls::suite_name(chosen),
    );
    c.eq("echo", "vscs".to_string(), answer);
    c.finish()
});

tls_test!(unsolicited_certificate_request, |ctx| {
    let client = ctx.handshake().await?;
    let mut c = Check::new("whether the server asked for a client certificate");
    c.note(
        "Post-handshake authentication needs the client to have offered \
         post_handshake_auth(49), which this hello did not. A CertificateRequest here would \
         be illegal.",
    );
    c.that(
        "certificate_request",
        "absent",
        client.certificate_request.is_none(),
        "the server sent a CertificateRequest the client never allowed",
    );
    c.that(
        "client_hello.extensions",
        "no post_handshake_auth was offered",
        !client
            .client_hello
            .as_ref()
            .map(|h| h.extensions.iter().any(|e| e.ext_type == 49))
            .unwrap_or(false),
        "the suite's own hello offered post_handshake_auth",
    );
    c.finish()
});

tls_test!(still_serving, |ctx| {
    let hello = hello_message(&ctx.config_n(4)).map_err(crate::stages::harness)?;
    let mut conn = ctx.connect().await?;
    let _ = provoke(&mut conn, &Record::build(22, LEGACY_VERSION_TLS12, &hello)).await;
    drop(conn);
    ctx.expect_still_serving("a renegotiation attempt").await
});

fn scsv_hello(env: &ExampleEnv) -> Result<Vec<u8>, String> {
    let (mut hello, _) = build_hello(&env.config()).map_err(|e| e.to_string())?;
    hello.cipher_suites.insert(0, 0x00ff);
    hello.extensions.push(Extension::new(0xff01, vec![0u8]));
    Ok(Record::build(22, LEGACY_VERSION_TLS12, &hello.encode()))
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::raw(
            "A hello carrying the TLS 1.2 renegotiation baggage",
            scsv_hello,
            Expect::Records(1),
        )
        .request(
            "cipher_suites begins with 00 ff (the renegotiation SCSV) and the extensions end \
             with ff 01 00 01 00 (renegotiation_info, empty)",
        )
        .response("An ordinary ServerHello. Both are skipped like anything else unrecognised.")
        .note(
            "Real clients still send these. A TLS 1.3 server must not try to honour them, and \
             must not refuse a hello for carrying them.",
        ),
        ExampleSpec::text("What replaced renegotiation")
            .request(
                "TLS 1.2 used renegotiation for three things: rekeying, client \
                 authentication after the fact, and changing parameters mid-connection.",
            )
            .response(
                "TLS 1.3 keeps the first as KeyUpdate and the second as post-handshake \
                 authentication, and drops the third. A new ClientHello on a live connection \
                 is simply unexpected_message(10).",
            )
            .note(
                "The removal is deliberate: renegotiation carried a decade of attacks, from \
                 RFC 5746's prefix injection to the triple handshake. There is no compatible \
                 way back and none is wanted.",
            ),
    ]
}
