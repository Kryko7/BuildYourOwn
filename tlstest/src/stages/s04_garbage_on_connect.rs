//! Stage 04 — Garbage on connect: an alert or a clean close, never a hang or a crash.

use crate::assert::Check;
use crate::examples::{ExampleEnv, ExampleSpec, Expect};
use crate::stages::{check_refused, hello_message, provoke, Stage, Test};
use crate::tls::conn::Reaction;
use crate::tls::record::Record;
use crate::tls::{LEGACY_VERSION_TLS12};
use crate::tls_test;
use rand::Rng;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 4,
        slug: "garbage_on_connect",
        name: "Garbage on connect",
        ext: false,
        hints: &[
            "Validate the record's content type before anything else: a TLS 1.3 stream only \
             ever starts with handshake(22)",
            "A client that sends HTTP, SSH or noise gets an alert or a close — never a stack \
             trace, never a hang, and never a dead accept loop",
            "One bad connection must not take the others with it: handle each socket's errors \
             where they happen",
            "Whatever you decide, decide it quickly; a stranger must not be able to hold a \
             connection open for ever by sending four bytes",
        ],
        examples: examples,
        tests: vec![
            Test::new("an HTTP request on the TLS port is refused", http_request),
            Test::new("an SSH banner is refused", ssh_banner),
            Test::new("random bytes are refused", random_bytes),
            Test::new(
                "a record with an undefined content type is refused",
                bad_content_type,
            ),
            Test::new(
                "a handshake record holding an unknown message type is refused",
                unknown_handshake_type,
            ),
            Test::new("a single byte followed by silence is survivable", one_byte),
            Test::new(
                "the server still serves a clean handshake afterwards",
                still_serving,
            ),
        ],
    }
}

async fn refuse(ctx: &mut crate::stages::Ctx, bytes: &[u8], what: &str) -> Result<(), crate::assert::Failure> {
    let mut conn = ctx.connect().await?;
    let reaction = provoke(&mut conn, bytes).await;
    let mut c = Check::new(format!("what the server does with {what}"));
    c.block("what the client sent", bytes);
    check_refused(&mut c, "the server's reaction", what, &reaction);
    c.finish()?;
    drop(conn);
    ctx.expect_still_serving(what).await
}

tls_test!(http_request, |ctx| {
    refuse(
        ctx,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nUser-Agent: not-a-tls-client\r\n\r\n",
        "an HTTP/1.1 request",
    )
    .await
});

tls_test!(ssh_banner, |ctx| {
    refuse(ctx, b"SSH-2.0-OpenSSH_9.9\r\n", "an SSH banner").await
});

tls_test!(random_bytes, |ctx| {
    let bytes: Vec<u8> = (0..64).map(|_| ctx.rng.random::<u8>()).collect();
    refuse(ctx, &bytes, "64 random bytes").await
});

tls_test!(bad_content_type, |ctx| {
    let bytes = Record::build(99, LEGACY_VERSION_TLS12, b"not a record type TLS defines");
    refuse(ctx, &bytes, "a record of content type 99").await
});

tls_test!(unknown_handshake_type, |ctx| {
    // A well-formed record carrying handshake type 200, which no version of TLS defines.
    let mut message = vec![200u8, 0, 0, 4];
    message.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
    let bytes = Record::build(22, LEGACY_VERSION_TLS12, &message);
    refuse(ctx, &bytes, "handshake message type 200").await
});

tls_test!(one_byte, |ctx| {
    let mut conn = ctx.connect().await?;
    let reaction = provoke(&mut conn, &[0x16]).await;
    let mut c = Check::new("a single 0x16 byte and then nothing");
    c.note(
        "A server may wait for the rest of the header, or give up. Both are fine; what is not \
         fine is dying, or never coming back to the accept loop.",
    );
    c.that(
        "the server's reaction",
        "silence, an alert or a close",
        !matches!(reaction, Reaction::Error(_)),
        reaction.describe(),
    );
    c.finish()?;
    drop(conn);
    ctx.expect_still_serving("one byte of a record header").await
});

tls_test!(still_serving, |ctx| {
    let garbage: Vec<Vec<u8>> = vec![
        b"GET / HTTP/1.0\r\n\r\n".to_vec(),
        b"\x00\x00\x00\x00".to_vec(),
        Record::build(20, LEGACY_VERSION_TLS12, &[1]),
        Record::build(23, LEGACY_VERSION_TLS12, b"application data before a handshake"),
    ];
    for bytes in &garbage {
        let mut conn = ctx.connect().await?;
        let _ = provoke(&mut conn, bytes).await;
        drop(conn);
    }
    ctx.note(format!("{} kinds of garbage sent", garbage.len()));
    ctx.expect_still_serving("four kinds of garbage").await
});

fn http_bytes(_env: &ExampleEnv) -> Result<Vec<u8>, String> {
    Ok(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n".to_vec())
}

fn good_hello(env: &ExampleEnv) -> Result<Vec<u8>, String> {
    Ok(Record::build(
        22,
        LEGACY_VERSION_TLS12,
        &hello_message(&env.config())?,
    ))
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::raw("An HTTP request on the TLS port", http_bytes, Expect::UntilClose)
            .request("`GET / HTTP/1.1` — the most common thing a TLS port is sent by mistake")
            .response(
                "The connection is closed. A fatal alert first is nicer but optional: the peer \
                 is not speaking TLS and will not understand it either way.",
            )
            .note(
                "`G` is 0x47, which is not a content type TLS defines. That is enough to decide, \
                 on the very first byte, that this is not a TLS stream.",
            ),
        ExampleSpec::raw(
            "And the same server, a moment later, with a real ClientHello",
            good_hello,
            Expect::Records(1),
        )
        .request("A well-formed ClientHello on a fresh connection")
        .response("A ServerHello. The garbage changed nothing.")
        .note(
            "This is the half of the stage that is easy to miss: surviving the bad connection \
             is only half of it, the accept loop has to still be running afterwards.",
        ),
    ]
}
