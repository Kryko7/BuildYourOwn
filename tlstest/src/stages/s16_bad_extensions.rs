//! Stage 16 — `legacy_session_id`, duplicated extensions and lying lengths.

use crate::assert::Check;
use crate::examples::{ExampleEnv, ExampleSpec, Expect};
use crate::stages::{check_refused_with, provoke, provoke_edited_hello, Stage, Test};
use crate::tls::client::{build_hello, ClientConfig};
use crate::tls::msg::{find_extension, Extension};
use crate::tls::record::Record;
use crate::tls::{AlertDescription, EXT_SUPPORTED_VERSIONS, LEGACY_VERSION_TLS12};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 16,
        slug: "bad_extensions",
        name: "Session id echo, duplicate extensions, lying lengths",
        ext: false,
        hints: &[
            "legacy_session_id is echoed into the ServerHello byte for byte, whatever the \
             client put there — 32 bytes, zero bytes, or anything between",
            "A ClientHello with the same extension type twice is illegal_parameter(47); \
             checking for it means keeping a set of the types you have seen",
            "Every length must be checked against the bytes you actually have before you \
             index; an extension claiming 400 bytes inside a 40-byte block is a decode_error",
            "legacy_compression_methods must be exactly `01 00`; anything else is \
             illegal_parameter",
        ],
        examples,
        tests: vec![
            Test::new(
                "a 32-byte legacy_session_id is echoed exactly",
                session_id_echo,
            ),
            Test::new(
                "an empty legacy_session_id is echoed as empty",
                empty_session_id,
            ),
            Test::new(
                "an odd-length legacy_session_id is echoed exactly",
                odd_session_id,
            ),
            Test::new("a duplicated extension is refused", duplicate_extension),
            Test::new(
                "an extension whose length runs past the block is refused",
                lying_extension_length,
            ),
            Test::new(
                "an extensions block whose length runs past the message is refused",
                lying_block_length,
            ),
            Test::new(
                "a compression method other than null is refused",
                bad_compression,
            ),
            Test::new(
                "the server still serves a clean hello afterwards",
                still_serving,
            ),
        ],
    }
}

/// Handshake with this session id and check the echo.
async fn echo_session_id(
    ctx: &crate::stages::Ctx,
    id: Vec<u8>,
) -> Result<(), crate::assert::Failure> {
    let config = ctx.config().with_session_id(id.clone());
    let client = ctx.handshake_with(config).await?;
    let hello = client
        .server_hello
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no ServerHello"))?;
    let mut c = Check::new(format!("the echo of a {}-byte legacy_session_id", id.len()));
    c.block("client_hello", &client.client_hello_bytes);
    c.block("server_hello", &client.server_hello_bytes);
    c.note(
        "The field exists only so a TLS 1.3 handshake looks like a resumed TLS 1.2 one to a \
         middlebox. The server copies it and never interprets it.",
    );
    c.bytes_eq(
        "server_hello.legacy_session_id_echo",
        &id,
        &hello.legacy_session_id_echo,
    );
    c.finish()
}

tls_test!(session_id_echo, |ctx| {
    echo_session_id(ctx, (0u8..32).collect()).await
});

tls_test!(empty_session_id, |ctx| {
    echo_session_id(ctx, Vec::new()).await
});

tls_test!(odd_session_id, |ctx| {
    echo_session_id(ctx, vec![0xab; 7]).await
});

tls_test!(duplicate_extension, |ctx| {
    let (reaction, bytes) = provoke_edited_hello(ctx, ctx.config(), |hello| {
        if let Some(e) = find_extension(&hello.extensions, EXT_SUPPORTED_VERSIONS).cloned() {
            hello.extensions.push(e);
        }
    })
    .await?;
    let mut c = Check::new("a hello carrying supported_versions twice");
    c.block("client_hello", &bytes);
    c.note(
        "RFC 8446 section 4.2: 'There MUST NOT be more than one extension of the same type in \
         a given extension block'. The named alert is illegal_parameter(47).",
    );
    check_refused_with(
        &mut c,
        "the server's reaction",
        &reaction,
        &[
            AlertDescription::ILLEGAL_PARAMETER,
            AlertDescription::DECODE_ERROR,
            AlertDescription::UNEXPECTED_MESSAGE,
            AlertDescription::HANDSHAKE_FAILURE,
        ],
    );
    c.finish()
});

tls_test!(lying_extension_length, |ctx| {
    // Build a clean hello, then overwrite one extension's length with something far too big.
    let (hello, _) =
        build_hello(&ctx.config()).map_err(|e| crate::stages::harness(e.to_string()))?;
    let mut bytes = hello.encode();
    // Find the extensions block: 4 header + 2 version + 32 random + session id + suites +
    // compression, then a two-byte extensions length.
    let session_len = bytes[4 + 2 + 32] as usize;
    let mut at = 4 + 2 + 32 + 1 + session_len;
    let suites_len = u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize;
    at += 2 + suites_len;
    let compression_len = bytes[at] as usize;
    at += 1 + compression_len;
    let ext_block = at + 2;
    // The first extension's length is at ext_block + 2.
    let target = ext_block + 2;
    bytes[target] = 0x01;
    bytes[target + 1] = 0x90; // 400 bytes, which the block does not contain
    let mut conn = ctx.connect().await?;
    let reaction = provoke(&mut conn, &Record::build(22, LEGACY_VERSION_TLS12, &bytes)).await;
    let mut c = Check::new("a hello whose first extension claims 400 bytes it does not have");
    c.block("client_hello", &bytes);
    c.mark(target..target + 2);
    c.note(
        "Check the length against the bytes you have before you index. This is the shape of \
         every buffer-overread CVE the TLS ecosystem has ever had.",
    );
    check_refused_with(
        &mut c,
        "the server's reaction",
        &reaction,
        &[
            AlertDescription::DECODE_ERROR,
            AlertDescription::ILLEGAL_PARAMETER,
            AlertDescription::UNEXPECTED_MESSAGE,
            AlertDescription::HANDSHAKE_FAILURE,
        ],
    );
    c.finish()?;
    drop(conn);
    ctx.expect_still_answering("an extension with a lying length")
        .await
});

tls_test!(lying_block_length, |ctx| {
    let (hello, _) =
        build_hello(&ctx.config()).map_err(|e| crate::stages::harness(e.to_string()))?;
    let mut bytes = hello.encode();
    let session_len = bytes[4 + 2 + 32] as usize;
    let mut at = 4 + 2 + 32 + 1 + session_len;
    let suites_len = u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize;
    at += 2 + suites_len;
    let compression_len = bytes[at] as usize;
    at += 1 + compression_len;
    // `at` is the two-byte extensions-block length; make it far larger than the message.
    bytes[at] = 0x0f;
    bytes[at + 1] = 0xff;
    let mut conn = ctx.connect().await?;
    let reaction = provoke(&mut conn, &Record::build(22, LEGACY_VERSION_TLS12, &bytes)).await;
    let mut c = Check::new("a hello whose extensions block claims 4095 bytes");
    c.block("client_hello", &bytes);
    c.mark(at..at + 2);
    check_refused_with(
        &mut c,
        "the server's reaction",
        &reaction,
        &[
            AlertDescription::DECODE_ERROR,
            AlertDescription::ILLEGAL_PARAMETER,
            AlertDescription::UNEXPECTED_MESSAGE,
            AlertDescription::HANDSHAKE_FAILURE,
        ],
    );
    c.finish()?;
    drop(conn);
    ctx.expect_still_answering("an extensions block with a lying length")
        .await
});

tls_test!(bad_compression, |ctx| {
    let (mut hello, _) =
        build_hello(&ctx.config()).map_err(|e| crate::stages::harness(e.to_string()))?;
    hello.legacy_compression_methods = vec![0, 1];
    let bytes = hello.encode();
    let mut conn = ctx.connect().await?;
    let reaction = provoke(&mut conn, &Record::build(22, LEGACY_VERSION_TLS12, &bytes)).await;
    let mut c = Check::new("a hello offering a compression method other than null");
    c.block("client_hello", &bytes);
    c.note(
        "RFC 8446 section 4.1.2: legacy_compression_methods must be a single zero byte, and \
         'if a TLS 1.3 ClientHello is received with any other value ... the server MUST abort \
         with an illegal_parameter alert'.",
    );
    check_refused_with(
        &mut c,
        "the server's reaction",
        &reaction,
        &[
            AlertDescription::ILLEGAL_PARAMETER,
            AlertDescription::DECODE_ERROR,
            AlertDescription::HANDSHAKE_FAILURE,
        ],
    );
    c.finish()?;
    drop(conn);
    ctx.expect_still_answering("a hello offering compression")
        .await
});

tls_test!(still_serving, |ctx| {
    let (mut hello, _) =
        build_hello(&ctx.config()).map_err(|e| crate::stages::harness(e.to_string()))?;
    hello
        .extensions
        .push(Extension::new(EXT_SUPPORTED_VERSIONS, vec![2, 3, 4]));
    let mut conn = ctx.connect().await?;
    let _ = provoke(
        &mut conn,
        &Record::build(22, LEGACY_VERSION_TLS12, &hello.encode()),
    )
    .await;
    drop(conn);
    ctx.expect_still_answering("a malformed hello").await
});

fn duplicate_hello(env: &ExampleEnv) -> Result<Vec<u8>, String> {
    let (mut hello, _) = build_hello(&env.config()).map_err(|e| e.to_string())?;
    if let Some(e) = find_extension(&hello.extensions, EXT_SUPPORTED_VERSIONS).cloned() {
        hello.extensions.push(e);
    }
    Ok(Record::build(22, LEGACY_VERSION_TLS12, &hello.encode()))
}

fn config(env: &ExampleEnv) -> ClientConfig {
    env.config()
}

fn examples() -> Vec<ExampleSpec> {
    let _ = config;
    vec![
        ExampleSpec::raw(
            "A hello carrying supported_versions twice",
            duplicate_hello,
            Expect::UntilAlert,
        )
        .request(
            "An ordinary ClientHello with the supported_versions extension appended a second \
             time at the end of the block",
        )
        .response(
            "A fatal alert. illegal_parameter(47) is the one RFC 8446 section 4.2 names; \
             decode_error(50) also appears in the wild.",
        )
        .note(
            "Finding this needs a set of the extension types already seen. A parser written as \
             'loop, match, assign' will silently take the last one and never notice.",
        ),
        ExampleSpec::text("legacy_session_id is copied, never interpreted")
            .request(
                "The client puts 32 random bytes in legacy_session_id (what every browser \
                 does), or zero bytes, or seven.",
            )
            .response(
                "server_hello.legacy_session_id_echo holds exactly those bytes, with exactly \
                 that length.",
            )
            .note(
                "The field means nothing in TLS 1.3; it exists so the handshake looks like a \
                 resumed TLS 1.2 one to a middlebox. Copy it verbatim, and note that a \
                 non-empty one is also the signal to send the compatibility ChangeCipherSpec.",
            ),
    ]
}
