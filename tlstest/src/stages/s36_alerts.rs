//! Stage 36 — The alert record: format, levels and what closes a connection.

use crate::assert::{Check, Failure};
use crate::examples::{ExampleEnv, ExampleSpec, Expect};
use crate::stages::{hello_message, provoke, Stage, Test};
use crate::tls::client::build_hello;
use crate::tls::conn::Reaction;
use crate::tls::msg::Alert;
use crate::tls::record::Record;
use crate::tls::{AlertDescription, AlertLevel, ContentType, LEGACY_VERSION_TLS12};
use crate::tls_test;
use std::time::Duration;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 36,
        slug: "alerts",
        name: "The alert record",
        ext: false,
        hints: &[
            "An alert is a record of type 21 whose fragment is exactly two bytes: level, then \
             description",
            "In TLS 1.3 the level is decoration — every alert except close_notify and \
             user_canceled is fatal whatever the byte says, and a fatal alert always closes \
             the connection",
            "Before keys exist alerts go out in the clear; afterwards they are encrypted like \
             anything else, with an inner content type of 21",
            "An alert record must never be split across records or coalesced with another: \
             two bytes, one record",
        ],
        examples,
        tests: vec![
            Test::new(
                "an alert record is two bytes in a type-21 record",
                alert_shape,
            ),
            Test::new(
                "a pre-keys alert arrives in the clear",
                plaintext_before_keys,
            ),
            Test::new(
                "a post-handshake alert arrives encrypted",
                encrypted_after_keys,
            ),
            Test::new(
                "a fatal alert from the client closes the connection",
                client_fatal_alert,
            ),
            Test::new(
                "a warning alert that is not close_notify does not hang the server",
                stray_warning,
            ),
            Test::new(
                "an alert record of the wrong length is refused",
                bad_alert_length,
            ),
            Test::new(
                "an unknown alert description is survivable",
                unknown_description,
            ),
            Test::new(
                "the server still serves a new connection afterwards",
                still_serving,
            ),
        ],
    }
}

/// Provoke a refusal in the clear and hand back the alert record the server sent.
async fn plaintext_alert(ctx: &crate::stages::Ctx) -> Result<Option<Record>, Failure> {
    // A hello with no TLS 1.3 in supported_versions earns a protocol_version alert with no
    // keys anywhere in sight.
    let (mut hello, _) = build_hello(&ctx.config()).map_err(Failure::tls)?;
    hello
        .extensions
        .retain(|e| e.ext_type != crate::tls::EXT_SUPPORTED_VERSIONS);
    hello
        .extensions
        .insert(1, crate::tls::msg::supported_versions_extension(&[0x0303]));
    let mut conn = ctx.connect().await?;
    let _ = provoke(
        &mut conn,
        &Record::build(22, LEGACY_VERSION_TLS12, &hello.encode()),
    )
    .await;
    Ok(conn
        .records_in
        .iter()
        .find(|r| r.content_type == ContentType::Alert)
        .cloned())
}

tls_test!(alert_shape, |ctx| {
    let record = plaintext_alert(ctx).await?;
    let mut c = Check::new("the shape of an alert record");
    match record {
        Some(record) => {
            c.block("the alert record", &record.raw);
            c.eq(
                "record.type",
                ContentType::Alert.as_u8(),
                record.content_type.as_u8(),
            );
            c.eq("record.length", 2usize, record.fragment.len());
            c.eq(
                "record.legacy_record_version",
                LEGACY_VERSION_TLS12,
                record.legacy_version,
            );
            let alert = Alert::parse(&record.fragment).map_err(Failure::tls)?;
            c.observe("alert", alert.describe());
            c.that(
                "alert.level",
                "warning(1) or fatal(2)",
                matches!(alert.level, AlertLevel::Warning | AlertLevel::Fatal),
                alert.level.name(),
            );
        }
        None => {
            c.note(
                "the server closed without sending an alert, which RFC 8446 allows; there is \
                 nothing to inspect",
            );
        }
    }
    c.finish()
});

tls_test!(plaintext_before_keys, |ctx| {
    let record = plaintext_alert(ctx).await?;
    let mut c = Check::new("whether a pre-keys alert is encrypted");
    match record {
        Some(record) => {
            c.block("the alert record", &record.raw);
            c.note(
                "There are no keys on this connection and there never will be, so the alert \
                 has to be a plaintext type-21 record — the only way the client can read it.",
            );
            c.eq("record.type", 21u8, record.content_type.as_u8());
            c.eq("record.length", 2usize, record.fragment.len());
        }
        None => {
            c.note("the server closed without an alert, which RFC 8446 allows");
        }
    }
    c.finish()
});

tls_test!(encrypted_after_keys, |ctx| {
    let mut client = ctx.handshake().await?;
    client
        .echo_line("alert")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let before = client.conn.records_in.len();
    // A record that will not decrypt: the answer has to be an encrypted alert.
    let mut bytes = client
        .conn
        .layer
        .seal(ContentType::ApplicationData, b"break\n", 0)
        .map_err(Failure::tls)?;
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    client.conn.write_raw(&bytes).await.map_err(Failure::tls)?;
    let reaction = crate::stages::reaction_past_tickets(&mut client.conn).await;
    let plaintext = client
        .conn
        .records_in
        .iter()
        .skip(before)
        .filter(|r| r.content_type == ContentType::Alert)
        .count();
    let mut c = Check::new("whether a post-handshake alert is encrypted");
    c.note(format!("the server answered: {}", reaction.describe()));
    c.note(
        "Once keys are in force the outer content type of every record is \
         application_data(23); the 21 moves inside, where only the peer can see it.",
    );
    c.eq(
        "plaintext alert records after the handshake",
        0usize,
        plaintext,
    );
    c.that(
        "the server's reaction",
        "an encrypted alert, or a closed connection",
        reaction.is_refusal(),
        reaction.describe(),
    );
    c.finish()?;
    drop(client);
    ctx.expect_still_serving("an encrypted alert exchange")
        .await
});

tls_test!(client_fatal_alert, |ctx| {
    let mut client = ctx.handshake().await?;
    client
        .echo_line("fatal")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    client
        .send_fatal(AlertDescription::INTERNAL_ERROR)
        .await
        .map_err(Failure::tls)?;
    let reaction = crate::stages::reaction_past_tickets(&mut client.conn).await;
    let mut c = Check::new("a fatal alert from the client");
    c.note(
        "A fatal alert means the sender has already given up. The receiver closes the \
         connection; answering with another alert is pointless and RFC 8446 does not ask for \
         it.",
    );
    c.that(
        "the server's reaction",
        "a close, or an alert of its own",
        matches!(
            reaction,
            Reaction::Closed | Reaction::Alert(_) | Reaction::Silence
        ),
        reaction.describe(),
    );
    c.finish()?;
    drop(client);
    ctx.expect_still_serving("a fatal alert from the client")
        .await
});

tls_test!(stray_warning, |ctx| {
    let mut client = ctx.handshake().await?;
    let alert = Alert {
        level: AlertLevel::Warning,
        description: AlertDescription::USER_CANCELED,
    };
    client
        .conn
        .write_record(ContentType::Alert, &alert.encode())
        .await
        .map_err(Failure::tls)?;
    // The connection may continue: user_canceled is a warning and says nothing about the
    // record stream. Whatever the server decides, it must decide it quickly.
    let mut c = Check::new("a warning alert that is not close_notify");
    let after = client
        .conn
        .write_record(ContentType::ApplicationData, b"still here\n")
        .await;
    let reaction = crate::stages::reaction_past_tickets(&mut client.conn).await;
    c.note(format!("the server answered: {}", reaction.describe()));
    c.note(
        "RFC 8446 section 6.1 keeps user_canceled as a warning; a server may carry on or may \
         close. What it must not do is stall.",
    );
    c.that(
        "the server's reaction",
        "an answer, an alert or a close — within the timeout",
        after.is_ok() || reaction.is_refusal(),
        reaction.describe(),
    );
    c.that(
        "the server's reaction",
        "not silence",
        !matches!(reaction, Reaction::Silence),
        "nothing at all came back",
    );
    c.finish()?;
    drop(client);
    ctx.expect_still_serving("a stray user_canceled warning")
        .await
});

tls_test!(bad_alert_length, |ctx| {
    // A type-21 record whose fragment is one byte, and another whose fragment is three.
    for fragment in [vec![2u8], vec![2u8, 40, 0]] {
        let mut bytes = Record::build(
            22,
            LEGACY_VERSION_TLS12,
            &hello_message(&ctx.config()).map_err(crate::stages::harness)?,
        );
        bytes.extend_from_slice(&Record::build(21, LEGACY_VERSION_TLS12, &fragment));
        let mut conn = ctx.connect().await?;
        let reaction = provoke(&mut conn, &bytes).await;
        let mut c = Check::new(format!(
            "an alert record whose fragment is {} byte(s)",
            fragment.len()
        ));
        c.note(
            "RFC 8446 section 6: an alert is exactly two bytes. A one-byte record cannot be \
             one, and a three-byte record is two alerts' worth of nonsense.",
        );
        c.that(
            "the server's reaction",
            "an alert, a close, or a ServerHello — but never a crash",
            !matches!(reaction, Reaction::Error(_)),
            reaction.describe(),
        );
        c.finish()?;
        drop(conn);
    }
    ctx.expect_still_serving("alert records of the wrong length")
        .await
});

tls_test!(unknown_description, |ctx| {
    let mut client = ctx.handshake().await?;
    // Description 200 is not assigned; a receiver that matches on known values must not
    // fall over on it.
    let alert = Alert {
        level: AlertLevel::Fatal,
        description: AlertDescription(200),
    };
    client
        .conn
        .write_record(ContentType::Alert, &alert.encode())
        .await
        .map_err(Failure::tls)?;
    let reaction = crate::stages::reaction_past_tickets(&mut client.conn).await;
    let mut c = Check::new("a fatal alert with an unassigned description");
    c.note(format!("the server answered: {}", reaction.describe()));
    c.that(
        "the server's reaction",
        "a close or an alert — the description is unknown, the level is not",
        !matches!(reaction, Reaction::Error(_)),
        reaction.describe(),
    );
    c.finish()?;
    drop(client);
    tokio::time::sleep(Duration::from_millis(50)).await;
    ctx.expect_still_serving("an unknown alert description")
        .await
});

tls_test!(still_serving, |ctx| {
    {
        let mut client = ctx.handshake_with(ctx.config_n(1)).await?;
        client
            .send_fatal(AlertDescription::INTERNAL_ERROR)
            .await
            .map_err(Failure::tls)?;
    }
    let mut client = ctx.handshake_with(ctx.config_n(2)).await?;
    let answer = client
        .echo_line("alive")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("a connection after an alert exchange");
    c.eq("echo", "evila".to_string(), answer);
    c.finish()
});

fn tls12_only_hello(env: &ExampleEnv) -> Result<Vec<u8>, String> {
    let (mut hello, _) = build_hello(&env.config()).map_err(|e| e.to_string())?;
    hello
        .extensions
        .retain(|e| e.ext_type != crate::tls::EXT_SUPPORTED_VERSIONS);
    hello
        .extensions
        .insert(1, crate::tls::msg::supported_versions_extension(&[0x0303]));
    Ok(Record::build(22, LEGACY_VERSION_TLS12, &hello.encode()))
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::raw(
            "An alert record in the clear",
            tls12_only_hello,
            Expect::UntilAlert,
        )
        .request("A ClientHello that offers only TLS 1.2, which a 1.3-only server must refuse")
        .response(
            "`15 03 03 00 02 02 46` — seven bytes: type 21, version 0x0303, length 2, then \
             level fatal(2) and description protocol_version(70).",
        )
        .note(
            "This is the whole alert protocol. Two bytes of payload, and the only ones that \
             ever travel in the clear are the ones sent before the handshake produced keys.",
        ),
        ExampleSpec::text("Levels, and why they barely matter")
            .request("level = warning(1) or fatal(2)")
            .response(
                "In TLS 1.3 only close_notify and user_canceled are warnings. Every other \
                 description is fatal regardless of the byte, and closes the connection.",
            )
            .note(
                "Do not decide what to do from the level byte — decide from the description. A \
                 peer that sends handshake_failure at level 1 is still telling you the \
                 handshake is over.",
            ),
    ]
}
