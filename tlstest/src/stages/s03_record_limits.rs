//! Stage 03 — Record size limits (RFC 8446 §5.1, §5.2).

use crate::assert::Check;
use crate::examples::{ExampleEnv, ExampleSpec, Expect};
use crate::stages::{check_refused_with, hello_message, provoke, Stage, Test};
use crate::tls::record::Record;
use crate::tls::{AlertDescription, LEGACY_VERSION_TLS12, MAX_PLAINTEXT};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 3,
        slug: "record_limits",
        name: "Record size limits",
        ext: false,
        hints: &[
            "TLSPlaintext.length may not exceed 2^14 (16384); a longer one is a \
             record_overflow alert, not a bigger buffer",
            "Check the length field before allocating: a two-byte length can ask for 65535 \
             bytes that will never arrive",
            "TLSCiphertext may be 2^14 + 256, which leaves room for the content type, the \
             padding and the AEAD tag",
            "A record that promises more bytes than the client ever sends must time out or \
             close, never spin or wedge the accept loop",
        ],
        examples,
        tests: vec![
            Test::new(
                "a record claiming more than 2^14 fragment bytes is refused",
                oversized_record,
            ),
            Test::new(
                "a record exactly at the 2^14 limit is not refused for being too long",
                at_the_limit,
            ),
            Test::new(
                "a length prefix that promises bytes the client never sends does not wedge \
                 the server",
                short_of_promise,
            ),
            Test::new("a zero-length handshake record is survivable", zero_length),
            Test::new(
                "a record with length 0xffff is refused without allocating it",
                enormous_length,
            ),
            Test::new(
                "the server still serves a clean handshake after all of that",
                still_serving,
            ),
        ],
    }
}

/// A handshake record whose fragment is `len` bytes of padding-ish filler.
fn big_record(len: usize) -> Vec<u8> {
    let mut fragment = vec![0u8; len];
    // Make it look like a handshake message so the length is the only thing wrong.
    fragment[0] = 1;
    let body = len - 4;
    fragment[1] = (body >> 16) as u8;
    fragment[2] = (body >> 8) as u8;
    fragment[3] = body as u8;
    Record::build(22, LEGACY_VERSION_TLS12, &fragment)
}

tls_test!(oversized_record, |ctx| {
    let mut conn = ctx.connect().await?;
    let reaction = provoke(&mut conn, &big_record(MAX_PLAINTEXT + 1)).await;
    let mut c = Check::new("a TLSPlaintext record one byte over the 2^14 limit");
    c.note(format!(
        "sent a handshake record with length {} (the limit is {MAX_PLAINTEXT})",
        MAX_PLAINTEXT + 1
    ));
    check_refused_with(
        &mut c,
        "the server's reaction",
        &reaction,
        &[
            AlertDescription::RECORD_OVERFLOW,
            AlertDescription::DECODE_ERROR,
            AlertDescription::UNEXPECTED_MESSAGE,
            AlertDescription::ILLEGAL_PARAMETER,
            AlertDescription::HANDSHAKE_FAILURE,
            AlertDescription::INTERNAL_ERROR,
        ],
    );
    c.finish()?;
    drop(conn);
    ctx.expect_still_answering("an oversized record").await
});

tls_test!(at_the_limit, |ctx| {
    let mut conn = ctx.connect().await?;
    let reaction = provoke(&mut conn, &big_record(MAX_PLAINTEXT)).await;
    let mut c = Check::new("a TLSPlaintext record exactly at the 2^14 limit");
    c.note(
        "The fragment is 16384 bytes of nonsense, so the server is free to reject it — but \
         never with record_overflow, because the length itself is legal.",
    );
    c.that(
        "the server's reaction",
        "anything but a record_overflow alert",
        reaction.alert().map(|a| a.description) != Some(AlertDescription::RECORD_OVERFLOW),
        reaction.describe(),
    );
    c.finish()?;
    drop(conn);
    ctx.expect_still_answering("a record at the size limit")
        .await
});

tls_test!(short_of_promise, |ctx| {
    let mut conn = ctx.connect().await?;
    // A header promising 1000 bytes, followed by ten.
    let mut bytes = vec![22, 0x03, 0x03, 0x03, 0xe8];
    bytes.extend_from_slice(&[0u8; 10]);
    let reaction = provoke(&mut conn, &bytes).await;
    let mut c = Check::new("a record header that promises 1000 bytes and delivers ten");
    c.note(
        "Whatever the server does here is fine — wait, close, or alert — as long as it does \
         not hang for ever and the process survives.",
    );
    c.that(
        "the server's reaction",
        "anything except a hang that outlasts the test",
        true,
        reaction.describe(),
    );
    c.finish()?;
    drop(conn);
    ctx.expect_still_answering("a record that promised more than it delivered")
        .await
});

tls_test!(zero_length, |ctx| {
    let mut conn = ctx.connect().await?;
    let reaction = provoke(&mut conn, &Record::build(22, LEGACY_VERSION_TLS12, &[])).await;
    let mut c = Check::new("a zero-length handshake record");
    c.note(
        "RFC 8446 section 5.1 forbids an empty handshake fragment, so a refusal is right; a \
         server that ignores it and waits for more is also alive, which is what matters here.",
    );
    c.that(
        "the server's reaction",
        "an alert, a close, or patience — but not a crash",
        !matches!(reaction, crate::tls::conn::Reaction::Error(_)),
        reaction.describe(),
    );
    c.finish()?;
    drop(conn);
    ctx.expect_still_answering("a zero-length record").await
});

tls_test!(enormous_length, |ctx| {
    let mut conn = ctx.connect().await?;
    let mut bytes = vec![22, 0x03, 0x03, 0xff, 0xff];
    bytes.extend_from_slice(&hello_message(&ctx.config()).map_err(crate::stages::harness)?);
    let reaction = provoke(&mut conn, &bytes).await;
    let mut c = Check::new("a record header claiming 65535 fragment bytes");
    c.note(
        "65535 is over the 2^14 limit, so this is a record_overflow — and a server must have \
         decided that from the two length bytes, before reserving anything.",
    );
    check_refused_with(
        &mut c,
        "the server's reaction",
        &reaction,
        &[
            AlertDescription::RECORD_OVERFLOW,
            AlertDescription::DECODE_ERROR,
            AlertDescription::UNEXPECTED_MESSAGE,
            AlertDescription::ILLEGAL_PARAMETER,
            AlertDescription::HANDSHAKE_FAILURE,
            AlertDescription::INTERNAL_ERROR,
        ],
    );
    c.finish()?;
    drop(conn);
    ctx.expect_still_answering("a 64 KiB length prefix").await
});

tls_test!(still_serving, |ctx| {
    for bytes in [
        big_record(MAX_PLAINTEXT + 1),
        Record::build(22, LEGACY_VERSION_TLS12, &[]),
        vec![22, 3, 3, 0xff, 0xff, 1, 2, 3],
    ] {
        let mut conn = ctx.connect().await?;
        let _ = provoke(&mut conn, &bytes).await;
        drop(conn);
    }
    ctx.expect_still_answering("three malformed records in a row")
        .await
});

fn oversized_bytes(_env: &ExampleEnv) -> Result<Vec<u8>, String> {
    Ok(big_record(MAX_PLAINTEXT + 1))
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::raw(
            "A record one byte over the limit",
            oversized_bytes,
            Expect::UntilClose,
        )
        .request(
            "A handshake record whose length field says 16385 — one more than the 2^14 that \
             RFC 8446 section 5.1 allows",
        )
        .response(
            "A fatal alert (record_overflow(22) is the one the RFC names) and the connection \
             closed. Some servers close without an alert, which is also allowed.",
        )
        .note(
            "The decision is made from the two length bytes alone. A server that allocates \
             `length` bytes first and checks afterwards has just let a stranger reserve 64 KiB \
             per connection.",
        ),
        ExampleSpec::text("Where the two limits come from")
            .request("TLSPlaintext.length <= 2^14 and TLSCiphertext.length <= 2^14 + 256")
            .response(
                "The extra 256 bytes on the ciphertext cover the inner content-type byte, any \
                 zero padding and the 16-byte AEAD tag.",
            )
            .note(
                "Apply the right limit to the right record: an encrypted record of 16400 bytes \
                 is legal, a plaintext one of the same size is not.",
            ),
    ]
}
