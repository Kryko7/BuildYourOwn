//! Stage 37 — Unexpected, unknown and truncated handshake messages.

use crate::assert::{Check, Failure};
use crate::examples::{ExampleEnv, ExampleSpec, Expect};
use crate::stages::{check_refused_with, hello_message, provoke, Stage, Test};
use crate::tls::msg::encode_handshake;
use crate::tls::record::Record;
use crate::tls::{AlertDescription, HandshakeType, LEGACY_VERSION_TLS12};
use crate::tls_test;
use std::time::Duration;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 37,
        slug: "bad_handshake",
        name: "Unknown and truncated handshake messages",
        ext: false,
        hints: &[
            "Check the uint24 length against the bytes you actually have before you index \
             into the body",
            "A handshake type you do not know is unexpected_message(10) — a server cannot \
             skip it the way it skips an unknown extension, because it has no idea what \
             state it would be in afterwards",
            "A message whose body does not decode is decode_error(50); a message that decodes \
             but is not legal here is illegal_parameter(47)",
            "A truncated message is not an error until the connection ends: more bytes may \
             still be coming, so wait, then fail cleanly",
        ],
        examples,
        tests: vec![
            Test::new(
                "a handshake type TLS does not define is refused",
                unknown_type,
            ),
            Test::new(
                "a message whose uint24 length is absurd is refused",
                absurd_length,
            ),
            Test::new(
                "a ClientHello truncated mid-body does not wedge the server",
                truncated_hello,
            ),
            Test::new(
                "a ClientHello with trailing bytes after the extensions is refused",
                trailing_bytes,
            ),
            Test::new("a zero-length handshake body is refused", zero_length_body),
            Test::new(
                "a handshake message of type 0 (the TLS 1.2 HelloRequest) is refused",
                hello_request,
            ),
            Test::new(
                "the server still serves a clean handshake afterwards",
                still_serving,
            ),
        ],
    }
}

async fn refuse(
    ctx: &mut crate::stages::Ctx,
    bytes: Vec<u8>,
    what: &str,
    allowed: &[AlertDescription],
) -> Result<(), Failure> {
    let mut conn = ctx.connect().await?;
    let reaction = provoke(&mut conn, &bytes).await;
    let mut c = Check::new(format!("what the server does with {what}"));
    c.block("what the client sent", &bytes);
    check_refused_with(&mut c, "the server's reaction", &reaction, allowed);
    c.finish()?;
    drop(conn);
    ctx.expect_still_serving(what).await
}

/// The alerts a server may reasonably answer a malformed first flight with.
const MALFORMED: &[AlertDescription] = &[
    AlertDescription::UNEXPECTED_MESSAGE,
    AlertDescription::DECODE_ERROR,
    AlertDescription::ILLEGAL_PARAMETER,
    AlertDescription::HANDSHAKE_FAILURE,
    AlertDescription::PROTOCOL_VERSION,
    AlertDescription::INTERNAL_ERROR,
];

tls_test!(unknown_type, |ctx| {
    let message = encode_handshake(HandshakeType(77), &[0xaa; 16]);
    refuse(
        ctx,
        Record::build(22, LEGACY_VERSION_TLS12, &message),
        "a handshake message of type 77",
        MALFORMED,
    )
    .await
});

tls_test!(absurd_length, |ctx| {
    // A ClientHello header claiming a 16 MiB body, with four bytes of it.
    let mut message = vec![1u8, 0xff, 0xff, 0xff];
    message.extend_from_slice(&[0x03, 0x03, 0x00, 0x00]);
    refuse(
        ctx,
        Record::build(22, LEGACY_VERSION_TLS12, &message),
        "a ClientHello claiming a 16 MiB body",
        MALFORMED,
    )
    .await
});

tls_test!(truncated_hello, |ctx| {
    let message = hello_message(&ctx.config()).map_err(crate::stages::harness)?;
    let cut = message.len() / 2;
    let mut conn = ctx.connect().await?;
    // The record is honest about its own length; the handshake message inside it is half of
    // one, and the rest will never come.
    let reaction = provoke(
        &mut conn,
        &Record::build(22, LEGACY_VERSION_TLS12, &message[..cut]),
    )
    .await;
    let mut c = Check::new("a ClientHello cut in half");
    c.note(
        "Waiting is the right first answer: the rest of a fragmented message may still be on \
         its way. What matters is that the wait ends and the process survives.",
    );
    c.that(
        "the server's reaction",
        "silence, an alert or a close — but not a crash",
        !matches!(reaction, crate::tls::conn::Reaction::Error(_)),
        reaction.describe(),
    );
    c.finish()?;
    drop(conn);
    tokio::time::sleep(Duration::from_millis(50)).await;
    ctx.expect_still_serving("a truncated ClientHello").await
});

tls_test!(trailing_bytes, |ctx| {
    let message = hello_message(&ctx.config()).map_err(crate::stages::harness)?;
    // Add eight bytes to the body and to the uint24 length: everything parses until the
    // extensions block ends and eight bytes are still left.
    let mut longer = message.clone();
    longer.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef, 0xde, 0xad, 0xbe, 0xef]);
    let body = longer.len() - 4;
    longer[1] = (body >> 16) as u8;
    longer[2] = (body >> 8) as u8;
    longer[3] = body as u8;
    refuse(
        ctx,
        Record::build(22, LEGACY_VERSION_TLS12, &longer),
        "a ClientHello with eight bytes after its extensions block",
        MALFORMED,
    )
    .await
});

tls_test!(zero_length_body, |ctx| {
    refuse(
        ctx,
        Record::build(22, LEGACY_VERSION_TLS12, &[1u8, 0, 0, 0]),
        "a ClientHello with a zero-length body",
        MALFORMED,
    )
    .await
});

tls_test!(hello_request, |ctx| {
    // Type 0 was HelloRequest in TLS 1.2 and is reserved in 1.3.
    refuse(
        ctx,
        Record::build(
            22,
            LEGACY_VERSION_TLS12,
            &encode_handshake(HandshakeType::HELLO_REQUEST, &[]),
        ),
        "a TLS 1.2 HelloRequest",
        MALFORMED,
    )
    .await
});

tls_test!(still_serving, |ctx| {
    for message in [
        encode_handshake(HandshakeType(200), &[0; 4]),
        vec![1u8, 0x00, 0x00, 0x00],
        encode_handshake(HandshakeType::HELLO_REQUEST, &[]),
    ] {
        let mut conn = ctx.connect().await?;
        let _ = provoke(
            &mut conn,
            &Record::build(22, LEGACY_VERSION_TLS12, &message),
        )
        .await;
        drop(conn);
    }
    let mut client = ctx.handshake_with(ctx.config_n(5)).await?;
    let answer = client
        .echo_line("intact")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("a clean handshake after three malformed ones");
    c.eq("echo", "tcatni".to_string(), answer);
    c.finish()
});

fn unknown_message(_env: &ExampleEnv) -> Result<Vec<u8>, String> {
    Ok(Record::build(
        22,
        LEGACY_VERSION_TLS12,
        &encode_handshake(HandshakeType(77), &[0xaa; 16]),
    ))
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::raw(
            "A handshake message of a type that does not exist",
            unknown_message,
            Expect::UntilAlert,
        )
        .request(
            "`4d 00 00 10` and sixteen bytes — a well-framed handshake message whose type, \
             77, TLS has never defined",
        )
        .response("A fatal alert; unexpected_message(10) is the one RFC 8446 names.")
        .note(
            "Unlike an extension, an unknown handshake message cannot be skipped. The \
             framing says how long it is, but not what state the connection would be in \
             afterwards — so there is nothing safe to do but stop.",
        ),
        ExampleSpec::text("Which alert for which kind of broken")
            .request(
                "A type that does not exist         -> unexpected_message(10)\n\
                 A body that will not parse         -> decode_error(50)\n\
                 A body that parses but is illegal  -> illegal_parameter(47)\n\
                 A length longer than the bytes sent -> wait, then decode_error",
            )
            .response(
                "All of them fatal, all of them closing the connection. The distinction is \
                 for the operator reading a log, not for the peer.",
            )
            .note(
                "Check the length before you index. Every one of these is a bounds check that \
                 happens before a parse, not after it.",
            ),
    ]
}
