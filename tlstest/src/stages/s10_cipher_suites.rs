//! Stage 10 — Cipher-suite negotiation across the three TLS 1.3 suites.

use crate::assert::Check;
use crate::examples::{ExampleEnv, ExampleSpec, Part};
use crate::stages::{Stage, Test};
use crate::tls::client::ClientConfig;
use crate::tls::crypto::{HashAlg, Suite};
use crate::tls::{
    suite_name, ALL_SUITES, TLS_AES_128_GCM_SHA256, TLS_AES_256_GCM_SHA384,
    TLS_CHACHA20_POLY1305_SHA256,
};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 10,
        slug: "cipher_suites",
        name: "Cipher-suite negotiation",
        ext: false,
        hints: &[
            "TLS 1.3 has three suites worth implementing: 0x1301 AES-128-GCM-SHA256, 0x1302 \
             AES-256-GCM-SHA384, 0x1303 ChaCha20-Poly1305-SHA256",
            "A suite names a hash *and* an AEAD; the hash drives the whole key schedule and \
             the transcript, so picking 0x1302 means SHA-384 everywhere",
            "The chosen suite must be one the ClientHello offered — a server that picks its \
             own favourite regardless will fail every client's check",
            "TLS 1.2 suite numbers may appear in the list; ignore them rather than choking on \
             them",
        ],
        examples,
        tests: vec![
            Test::new(
                "TLS_AES_128_GCM_SHA256 alone is negotiated and works",
                aes128,
            ),
            Test::new(
                "TLS_AES_256_GCM_SHA384 alone is negotiated and works",
                aes256,
            ),
            Test::new(
                "TLS_CHACHA20_POLY1305_SHA256 alone is negotiated and works",
                chacha,
            ),
            Test::new(
                "the chosen suite is always one the client offered",
                chosen_is_offered,
            ),
            Test::new(
                "a SHA-384 suite drives a SHA-384 key schedule",
                sha384_schedule,
            ),
            Test::new(
                "TLS 1.2 suite numbers in the list are ignored, not fatal",
                tls12_suites_ignored,
            ),
            Test::new(
                "a duplicated suite in the list changes nothing",
                duplicate_suite,
            ),
            Test::new(
                "the suite is echoed in a single uint16, not a list",
                single_uint16,
            ),
        ],
    }
}

/// Handshake with exactly one suite offered, and echo a line under it.
async fn one_suite(
    ctx: &crate::stages::Ctx,
    suite: u16,
    line: &str,
) -> Result<(), crate::assert::Failure> {
    let config = ctx.config().with_suites(&[suite]);
    let mut client = ctx.handshake_with(config).await?;
    let hello = client
        .server_hello
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no ServerHello"))?;
    let chosen = hello.cipher_suite;
    let answer = client
        .echo_line(line)
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new(format!("a handshake offering only {}", suite_name(suite)));
    c.block("server_hello", &client.server_hello_bytes);
    c.eq("server_hello.cipher_suite", suite, chosen);
    c.eq("echo", crate::stages::reversed(line), answer);
    let expected = Suite::from_code(suite)
        .map(|s| s.hash.name())
        .unwrap_or("?");
    c.observe("key schedule hash", expected);
    c.finish()
}

tls_test!(aes128, |ctx| {
    one_suite(ctx, TLS_AES_128_GCM_SHA256, "aes128").await
});

tls_test!(aes256, |ctx| {
    one_suite(ctx, TLS_AES_256_GCM_SHA384, "aes256").await
});

tls_test!(chacha, |ctx| {
    one_suite(ctx, TLS_CHACHA20_POLY1305_SHA256, "chacha").await
});

tls_test!(chosen_is_offered, |ctx| {
    // Offer two of the three, in the least likely order, and see what comes back.
    let offered = [TLS_CHACHA20_POLY1305_SHA256, TLS_AES_128_GCM_SHA256];
    let client = ctx
        .handshake_with(ctx.config().with_suites(&offered))
        .await?;
    let hello = client
        .server_hello
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no ServerHello"))?;
    let mut c = Check::new("that the chosen suite came from the client's list");
    c.block("client_hello", &client.client_hello_bytes);
    c.block("server_hello", &client.server_hello_bytes);
    c.note(
        "Either of the two offered suites is a correct answer; the server's own preference \
         order is its business.",
    );
    c.that(
        "server_hello.cipher_suite",
        "one of the two suites the ClientHello offered",
        offered.contains(&hello.cipher_suite),
        suite_name(hello.cipher_suite),
    );
    c.finish()
});

tls_test!(sha384_schedule, |ctx| {
    let config = ctx.config().with_suites(&[TLS_AES_256_GCM_SHA384]);
    let client = ctx.handshake_with(config).await?;
    let schedule = client
        .schedule
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no key schedule"))?;
    let mut c = Check::new("the key schedule under a SHA-384 suite");
    c.note_all(client.schedule_lines());
    c.eq(
        "suite.hash",
        HashAlg::Sha384.name(),
        schedule.suite.hash.name(),
    );
    c.eq("Early Secret length", 48usize, schedule.early_secret.len());
    c.eq(
        "Handshake Secret length",
        48usize,
        schedule.handshake_secret.len(),
    );
    c.eq(
        "transcript hash length",
        48usize,
        client.hash_after_server_hello.len(),
    );
    c.eq(
        "server write key length",
        32usize,
        schedule
            .server_handshake
            .as_ref()
            .map(|k| k.key.len())
            .unwrap_or(0),
    );
    c.finish()
});

tls_test!(tls12_suites_ignored, |ctx| {
    let mut suites = vec![
        0xc02f, // TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256, a TLS 1.2 suite
        0xc030, // TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384
        0x00ff, // TLS_EMPTY_RENEGOTIATION_INFO_SCSV
    ];
    suites.extend_from_slice(&ALL_SUITES);
    let client = ctx
        .handshake_with(ctx.config().with_suites(&suites))
        .await?;
    let hello = client
        .server_hello
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no ServerHello"))?;
    let mut c = Check::new("a hello mixing TLS 1.2 suite numbers with the 1.3 ones");
    c.block("client_hello", &client.client_hello_bytes);
    c.note(
        "A 1.3 client that also speaks 1.2 sends both, and the SCSV is still out there. \
         Skip what you do not recognise.",
    );
    c.that(
        "server_hello.cipher_suite",
        "one of the three TLS 1.3 suites",
        ALL_SUITES.contains(&hello.cipher_suite),
        suite_name(hello.cipher_suite),
    );
    c.finish()
});

tls_test!(duplicate_suite, |ctx| {
    let suites = [
        TLS_AES_128_GCM_SHA256,
        TLS_AES_128_GCM_SHA256,
        TLS_AES_128_GCM_SHA256,
    ];
    let client = ctx
        .handshake_with(ctx.config().with_suites(&suites))
        .await?;
    let hello = client
        .server_hello
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no ServerHello"))?;
    let mut c = Check::new("a hello that lists the same suite three times");
    c.note("RFC 8446 does not forbid a repeated cipher suite, only a repeated extension.");
    c.eq(
        "server_hello.cipher_suite",
        TLS_AES_128_GCM_SHA256,
        hello.cipher_suite,
    );
    c.finish()
});

tls_test!(single_uint16, |ctx| {
    let client = ctx.handshake().await?;
    // In the ServerHello the suite sits right after the session id echo: two bytes, no
    // length prefix, no list.
    let bytes = &client.server_hello_bytes;
    let session_len = bytes.get(4 + 2 + 32).copied().unwrap_or(0) as usize;
    let at = 4 + 2 + 32 + 1 + session_len;
    let mut c = Check::new("the shape of ServerHello.cipher_suite");
    c.block("server_hello", bytes);
    if at + 2 <= bytes.len() {
        c.mark(at..at + 2);
        let chosen = u16::from_be_bytes([bytes[at], bytes[at + 1]]);
        c.eq(
            "server_hello.cipher_suite (read positionally)",
            client
                .server_hello
                .as_ref()
                .map(|h| h.cipher_suite)
                .unwrap_or(0),
            chosen,
        );
        c.eq(
            "server_hello.legacy_compression_method",
            0u8,
            bytes.get(at + 2).copied().unwrap_or(0xff),
        );
    } else {
        c.that(
            "server_hello",
            "long enough to hold a cipher suite",
            false,
            bytes.len(),
        );
    }
    c.finish()
});

fn aes128_config(env: &ExampleEnv) -> ClientConfig {
    env.config().with_suites(&[TLS_AES_128_GCM_SHA256])
}

fn chacha_config(env: &ExampleEnv) -> ClientConfig {
    env.config().with_suites(&[TLS_CHACHA20_POLY1305_SHA256])
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::handshake(
            "A hello offering only TLS_AES_128_GCM_SHA256",
            aes128_config,
            Part::ClientHello,
            Part::ServerHello,
        )
        .request("cipher_suites holds one entry: 13 01")
        .response("server_hello.cipher_suite echoes 13 01, two bytes and no list around them.")
        .note(
            "0x1301 means SHA-256 and AES-128-GCM: a 32-byte transcript hash, 32-byte secrets, \
             a 16-byte key and a 12-byte IV.",
        ),
        ExampleSpec::handshake(
            "The same hello asking for ChaCha20-Poly1305",
            chacha_config,
            Part::ClientHello,
            Part::ServerHello,
        )
        .request("cipher_suites holds 13 03")
        .response("server_hello.cipher_suite is 13 03.")
        .note(
            "0x1303 is SHA-256 like 0x1301, but a 32-byte AEAD key. Only 0x1302 switches the \
             hash to SHA-384 — and then every secret, every transcript hash and the finished \
             key all become 48 bytes.",
        ),
    ]
}
