//! Stage 17 — The ServerHello, field by field.

use crate::assert::Check;
use crate::examples::{ExampleEnv, ExampleSpec, Part};
use crate::stages::{Stage, Test};
use crate::tls::client::ClientConfig;
use crate::tls::{
    DOWNGRADE_SENTINEL_TLS11, DOWNGRADE_SENTINEL_TLS12, HELLO_RETRY_REQUEST_RANDOM,
    LEGACY_VERSION_TLS12, TLS13_VERSION,
};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 17,
        slug: "server_hello",
        name: "The ServerHello, field by field",
        ext: false,
        hints: &[
            "The body is legacy_version(2), random(32), legacy_session_id_echo(vec8), \
             cipher_suite(2), legacy_compression_method(1), extensions(vec16) — in that order",
            "random is 32 fresh random bytes; it is not a timestamp and not derived from \
             anything the client sent",
            "A TLS 1.3 server must not put the downgrade sentinels of RFC 8446 section 4.1.3 \
             in the last eight bytes of random — those mean 'I negotiated 1.2 on purpose'",
            "key_share and supported_versions are the two extensions that make this a TLS 1.3 \
             ServerHello at all; everything else belongs in EncryptedExtensions",
        ],
        examples: examples,
        tests: vec![
            Test::new("legacy_version is 0x0303", legacy_version),
            Test::new("legacy_compression_method is 0", compression),
            Test::new("random is 32 bytes and is not all zeros", random_bytes),
            Test::new("random differs between two connections", random_varies),
            Test::new(
                "random does not carry a TLS 1.2 downgrade sentinel",
                no_downgrade_sentinel,
            ),
            Test::new(
                "random is not the HelloRetryRequest value on a normal hello",
                not_a_retry,
            ),
            Test::new("key_share names a group and carries a share", key_share),
            Test::new(
                "the body ends exactly where its length says",
                no_trailing_bytes,
            ),
        ],
    }
}

tls_test!(legacy_version, |ctx| {
    let client = ctx.handshake().await?;
    let hello = client
        .server_hello
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no ServerHello"))?;
    let mut c = Check::new("ServerHello.legacy_version");
    c.block("server_hello", &client.server_hello_bytes);
    c.mark(4..6);
    c.eq(
        "server_hello.legacy_version",
        LEGACY_VERSION_TLS12,
        hello.legacy_version,
    );
    c.finish()
});

tls_test!(compression, |ctx| {
    let client = ctx.handshake().await?;
    let hello = client
        .server_hello
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no ServerHello"))?;
    let mut c = Check::new("ServerHello.legacy_compression_method");
    c.block("server_hello", &client.server_hello_bytes);
    c.eq(
        "server_hello.legacy_compression_method",
        0u8,
        hello.legacy_compression_method,
    );
    c.finish()
});

tls_test!(random_bytes, |ctx| {
    let client = ctx.handshake().await?;
    let hello = client
        .server_hello
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no ServerHello"))?;
    let mut c = Check::new("ServerHello.random");
    c.block("server_hello", &client.server_hello_bytes);
    c.mark(6..38);
    c.eq("server_hello.random.len()", 32usize, hello.random.len());
    c.ne("server_hello.random", [0u8; 32].to_vec(), hello.random.to_vec());
    c.that(
        "server_hello.random",
        "not a copy of the client's random",
        client
            .client_hello
            .as_ref()
            .map(|h| h.random != hello.random)
            .unwrap_or(true),
        crate::tls::hex(&hello.random),
    );
    c.finish()
});

tls_test!(random_varies, |ctx| {
    let first = {
        let client = ctx.handshake_with(ctx.config_n(1)).await?;
        client
            .server_hello
            .as_ref()
            .map(|h| h.random)
            .ok_or_else(|| crate::stages::harness("no ServerHello"))?
    };
    let second = {
        let client = ctx.handshake_with(ctx.config_n(2)).await?;
        client
            .server_hello
            .as_ref()
            .map(|h| h.random)
            .ok_or_else(|| crate::stages::harness("no ServerHello"))?
    };
    let mut c = Check::new("ServerHello.random on two connections");
    c.note(
        "The server's random is the only entropy it contributes to the transcript. A counter, \
         a timestamp or a fixed value would make two handshakes correlatable.",
    );
    c.ne(
        "server_hello.random (second connection)",
        crate::tls::hex(&first),
        crate::tls::hex(&second),
    );
    c.finish()
});

tls_test!(no_downgrade_sentinel, |ctx| {
    let client = ctx.handshake().await?;
    let hello = client
        .server_hello
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no ServerHello"))?;
    let tail: [u8; 8] = hello.random[24..32]
        .try_into()
        .map_err(|_| crate::stages::harness("random is not 32 bytes"))?;
    let mut c = Check::new("the last eight bytes of ServerHello.random");
    c.block("server_hello", &client.server_hello_bytes);
    c.mark(30..38);
    c.note(
        "RFC 8446 section 4.1.3: a server that negotiates TLS 1.2 while able to do 1.3 sets \
         these eight bytes to 44 4F 57 4E 47 52 44 01, and a 1.3 client treats that as an \
         attack. A server that negotiated 1.3 must never send it.",
    );
    c.ne(
        "server_hello.random[24..32]",
        crate::tls::hex(&DOWNGRADE_SENTINEL_TLS12),
        crate::tls::hex(&tail),
    );
    c.ne(
        "server_hello.random[24..32] (the 1.1-and-below sentinel)",
        crate::tls::hex(&DOWNGRADE_SENTINEL_TLS11),
        crate::tls::hex(&tail),
    );
    c.finish()
});

tls_test!(not_a_retry, |ctx| {
    let client = ctx.handshake().await?;
    let hello = client
        .server_hello
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no ServerHello"))?;
    let mut c = Check::new("that a normal ServerHello is not a HelloRetryRequest");
    c.block("server_hello", &client.server_hello_bytes);
    c.note(
        "A HelloRetryRequest *is* a ServerHello: it is told apart only by its random being \
         SHA-256(\"HelloRetryRequest\").",
    );
    c.ne(
        "server_hello.random",
        crate::tls::hex(&HELLO_RETRY_REQUEST_RANDOM),
        crate::tls::hex(&hello.random),
    );
    c.eq("hello_retry_request", false, hello.is_hello_retry_request());
    c.finish()
});

tls_test!(key_share, |ctx| {
    let client = ctx.handshake().await?;
    let hello = client
        .server_hello
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no ServerHello"))?;
    let share = hello
        .key_share()
        .map_err(crate::assert::Failure::tls)?
        .ok_or_else(|| crate::stages::harness("no key_share extension"))?;
    let mut c = Check::new("the ServerHello's key_share and supported_versions");
    c.block("server_hello", &client.server_hello_bytes);
    c.eq(
        "server_hello.supported_versions.selected_version",
        TLS13_VERSION,
        hello.selected_version().unwrap_or(0),
    );
    c.that(
        "server_hello.key_share.key_exchange",
        "a non-empty share",
        !share.1.is_empty(),
        share.1.len(),
    );
    c.eq(
        "server_hello.key_share.group",
        client.selected_group.unwrap_or(0),
        share.0,
    );
    c.finish()
});

tls_test!(no_trailing_bytes, |ctx| {
    let client = ctx.handshake().await?;
    let bytes = &client.server_hello_bytes;
    let mut c = Check::new("that the ServerHello's uint24 length is exact");
    c.block("server_hello", bytes);
    c.mark(1..4);
    if bytes.len() >= 4 {
        let claimed = u32::from_be_bytes([0, bytes[1], bytes[2], bytes[3]]) as usize;
        c.eq("server_hello.length", bytes.len() - 4, claimed);
        // The parser already refused trailing bytes; this states it in the report.
        c.note(
            "The suite's own parser rejects a ServerHello with bytes left over after the \
             extensions block, so reaching this check means there were none.",
        );
    } else {
        c.that("server_hello", "at least four bytes", false, bytes.len());
    }
    c.finish()
});

fn config(env: &ExampleEnv) -> ClientConfig {
    env.config()
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::handshake(
            "A ServerHello, byte by byte",
            config,
            Part::ClientHello,
            Part::ServerHello,
        )
        .request("An ordinary TLS 1.3 ClientHello")
        .response(
            "02, a uint24 length, 03 03, 32 random bytes, the echoed session id, the chosen \
             suite, 00, and an extensions block holding supported_versions and key_share.",
        )
        .note(
            "Compare the session id in the two messages: same length byte, same bytes. That is \
             the only field of a ServerHello that is copied rather than chosen.",
        ),
        ExampleSpec::text("The two downgrade sentinels")
            .request("A TLS 1.3-capable server that ends up negotiating TLS 1.2 or lower")
            .response(
                "The last eight bytes of ServerHello.random are 44 4F 57 4E 47 52 44 01 for \
                 TLS 1.2, and 44 4F 57 4E 47 52 44 00 for 1.1 and below — 'DOWNGRD' and a \
                 version byte.",
            )
            .note(
                "It is a signature the client checks: an attacker who strips supported_versions \
                 cannot forge it, because it is inside the server's signed transcript. A 1.3 \
                 handshake must never carry it.",
            ),
    ]
}
