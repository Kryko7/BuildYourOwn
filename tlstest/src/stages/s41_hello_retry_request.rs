//! Stage 41 — HelloRetryRequest.

use crate::assert::Check;
use crate::examples::{ExampleEnv, ExampleSpec, Part};
use crate::stages::{reversed, Stage, Test};
use crate::tls::client::ClientConfig;
use crate::tls::crypto::HashAlg;
use crate::tls::msg::find_extension;
use crate::tls::{
    group_name, hex, EXT_COOKIE, EXT_KEY_SHARE, EXT_SUPPORTED_VERSIONS, GROUP_SECP256R1,
    GROUP_X25519, HELLO_RETRY_REQUEST_RANDOM, TLS13_VERSION,
};
use crate::tls_test;

/// A configuration that names two groups but shares neither, which is what makes a server
/// ask for a retry.
fn no_shares(ctx: &crate::stages::Ctx) -> ClientConfig {
    ctx.config()
        .with_group_shares(&[GROUP_X25519, GROUP_SECP256R1], &[])
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 41,
        slug: "hello_retry_request",
        name: "HelloRetryRequest",
        ext: true,
        hints: &[
            "A HelloRetryRequest *is* a ServerHello: same message type, same fields, and a \
             random fixed at SHA-256(\"HelloRetryRequest\")",
            "Its key_share carries only a selected_group and no share; the client answers \
             with a second ClientHello carrying a share for that group",
            "Before the retry goes into the transcript, ClientHello1 is replaced by the \
             synthetic `message_hash` message of RFC 8446 section 4.4.1 — this is the one \
             rule nobody guesses",
            "A cookie, if one is sent, must be echoed back verbatim in the second hello and \
             nowhere else",
        ],
        examples,
        tests: vec![
            Test::new(
                "a hello with no key share earns a HelloRetryRequest",
                earns_a_retry,
            ),
            Test::new(
                "the retry's random is SHA-256 of \"HelloRetryRequest\"",
                retry_random,
            ),
            Test::new("the retry names a group the client offered", retry_group),
            Test::new(
                "the retry carries supported_versions and a key_share of two bytes",
                retry_extensions,
            ),
            Test::new("the handshake completes after the retry", completes),
            Test::new(
                "the transcript uses the synthetic message_hash message",
                message_hash,
            ),
            Test::new(
                "a cookie, if one is sent, is echoed in the second hello",
                cookie_echo,
            ),
            Test::new(
                "data flows on a connection that began with a retry",
                data_flows,
            ),
        ],
    }
}

tls_test!(earns_a_retry, |ctx| {
    let client = ctx.handshake_with(no_shares(ctx)).await?;
    let mut c = Check::new("a hello that offers groups but shares none");
    c.block("client_hello (the second one)", &client.client_hello_bytes);
    c.note(
        "supported_groups says what the client can do; key_share says what it has already \
         computed. With the second list empty the server has no shared secret to work with, \
         so it asks for one.",
    );
    c.that(
        "hello_retry_request",
        "sent",
        client.hello_retry_request.is_some(),
        "the server answered with a normal ServerHello and no key exchange",
    );
    c.finish()
});

tls_test!(retry_random, |ctx| {
    let client = ctx.handshake_with(no_shares(ctx)).await?;
    let retry = client
        .hello_retry_request
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no HelloRetryRequest"))?;
    let mut c = Check::new("the HelloRetryRequest's random");
    c.note(
        "There is no separate message type: a ServerHello with this exact random is a \
         HelloRetryRequest, and one with any other random is not.",
    );
    c.bytes_eq(
        "hello_retry_request.random",
        &HELLO_RETRY_REQUEST_RANDOM,
        &retry.random,
    );
    c.eq(
        "SHA-256(\"HelloRetryRequest\")",
        hex(&HashAlg::Sha256.digest(b"HelloRetryRequest")),
        hex(&retry.random),
    );
    c.eq(
        "is_hello_retry_request",
        true,
        retry.is_hello_retry_request(),
    );
    c.finish()
});

tls_test!(retry_group, |ctx| {
    let client = ctx.handshake_with(no_shares(ctx)).await?;
    let retry = client
        .hello_retry_request
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no HelloRetryRequest"))?;
    let group = retry
        .retry_group()
        .map_err(crate::assert::Failure::tls)?
        .ok_or_else(|| crate::stages::harness("the retry carried no key_share"))?;
    let mut c = Check::new("the group the retry asked for");
    c.note(
        "It has to be a group the client named in supported_groups, and one it did *not* \
         already send a share for — otherwise the retry would be pointless.",
    );
    c.that(
        "hello_retry_request.key_share.selected_group",
        "one of the groups the ClientHello offered",
        [GROUP_X25519, GROUP_SECP256R1].contains(&group),
        group_name(group),
    );
    c.eq(
        "the group the handshake finally used",
        group,
        client.selected_group.unwrap_or(0),
    );
    c.finish()
});

tls_test!(retry_extensions, |ctx| {
    let client = ctx.handshake_with(no_shares(ctx)).await?;
    let retry = client
        .hello_retry_request
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no HelloRetryRequest"))?;
    let key_share = find_extension(&retry.extensions, EXT_KEY_SHARE)
        .ok_or_else(|| crate::stages::harness("the retry carried no key_share"))?;
    let mut c = Check::new("the extensions of a HelloRetryRequest");
    c.note(
        "A retry's key_share is two bytes — a group and nothing else. A ServerHello's is a \
         group plus a share, which is how the two are told apart even before the random is \
         checked.",
    );
    c.eq(
        "hello_retry_request.key_share.extension_data.len()",
        2usize,
        key_share.data.len(),
    );
    c.that(
        "hello_retry_request.supported_versions",
        "present",
        find_extension(&retry.extensions, EXT_SUPPORTED_VERSIONS).is_some(),
        "missing",
    );
    c.eq(
        "hello_retry_request.supported_versions.selected_version",
        TLS13_VERSION,
        retry.selected_version().unwrap_or(0),
    );
    c.eq(
        "hello_retry_request.legacy_session_id_echo",
        hex(&ctx.config().session_id),
        hex(&retry.legacy_session_id_echo),
    );
    c.finish()
});

tls_test!(completes, |ctx| {
    let client = ctx.handshake_with(no_shares(ctx)).await?;
    let mut c = Check::new("a handshake that went through a retry");
    c.note_all(client.transcript_lines());
    c.that(
        "hello_retry_request",
        "sent",
        client.hello_retry_request.is_some(),
        "no retry happened",
    );
    c.that(
        "server finished",
        "verified",
        !client.server_finished_bytes.is_empty(),
        "the handshake did not reach the server's Finished",
    );
    c.that(
        "client finished",
        "sent",
        !client.client_finished_bytes.is_empty(),
        "the client never sent its Finished",
    );
    c.finish()
});

tls_test!(message_hash, |ctx| {
    let client = ctx.handshake_with(no_shares(ctx)).await?;
    let transcript = client
        .transcript
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no transcript"))?;
    let raw = transcript.raw();
    let hash = client
        .suite
        .ok_or_else(|| crate::stages::harness("no suite"))?
        .hash;
    let mut c = Check::new("the synthetic message_hash of RFC 8446 section 4.4.1");
    c.note_all(client.transcript_lines());
    c.note(
        "Transcript-Hash(ClientHello1, HelloRetryRequest, ...) is not the concatenation: \
         ClientHello1 is replaced by `message_hash || 00 00 Hash.length || \
         Hash(ClientHello1)` first.",
    );
    c.eq(
        "the first byte of the transcript",
        254u8,
        raw.first().copied().unwrap_or(0),
    );
    c.eq(
        "the synthetic message's length bytes",
        vec![0u8, 0, hash.len() as u8],
        raw.get(1..4).map(<[u8]>::to_vec).unwrap_or_default(),
    );
    c.note(
        "That the server's Finished verified at all is the real proof: both sides built the \
         same transcript, and a client that skipped the replacement could not have.",
    );
    c.that(
        "server finished",
        "verified against this transcript",
        !client.server_verify_data.is_empty(),
        "the handshake never got there",
    );
    c.finish()
});

tls_test!(cookie_echo, |ctx| {
    let client = ctx.handshake_with(no_shares(ctx)).await?;
    let retry = client
        .hello_retry_request
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no HelloRetryRequest"))?;
    let cookie = retry.cookie().map_err(crate::assert::Failure::tls)?;
    let echoed = client
        .client_hello
        .as_ref()
        .and_then(|h| find_extension(&h.extensions, EXT_COOKIE).cloned());
    let mut c = Check::new("the cookie, if the server sent one");
    c.block("client_hello (the second one)", &client.client_hello_bytes);
    match cookie {
        Some(value) => {
            c.note(
                "A cookie lets a stateless server carry its half of the first exchange in the \
                 client's second hello instead of in memory.",
            );
            c.that(
                "client_hello.cookie",
                "echoed in the second hello",
                echoed.is_some(),
                "the second hello carried no cookie",
            );
            if let Some(e) = echoed {
                c.bytes_eq(
                    "client_hello.cookie",
                    &crate::tls::msg::cookie_extension(&value).data,
                    &e.data,
                );
            }
        }
        None => {
            c.note(
                "the server sent no cookie, which is normal for a server that keeps its own \
                 state across the retry",
            );
            c.that(
                "client_hello.cookie",
                "absent, because none was asked for",
                echoed.is_none(),
                "the client echoed a cookie it was never sent",
            );
        }
    }
    c.finish()
});

tls_test!(data_flows, |ctx| {
    let mut client = ctx.handshake_with(no_shares(ctx)).await?;
    let answer = client
        .echo_line("retried")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("the data path after a retry");
    c.that(
        "hello_retry_request",
        "sent",
        client.hello_retry_request.is_some(),
        "no retry happened",
    );
    c.eq("echo", reversed("retried"), answer);
    c.finish()
});

fn retry_config(env: &ExampleEnv) -> ClientConfig {
    env.config()
        .with_group_shares(&[GROUP_X25519, GROUP_SECP256R1], &[])
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::handshake(
            "The retry request itself",
            retry_config,
            Part::ClientHello,
            Part::HelloRetryRequest,
        )
        .request(
            "A ClientHello naming x25519 and secp256r1 in supported_groups, with an empty \
             key_share list",
        )
        .response(
            "A ServerHello whose random is CF 21 AD 74 … 33 9C and whose key_share holds two \
             bytes: the group it wants.",
        )
        .note(
            "Look at the random in the hex: it is not random at all. Any ServerHello carrying \
             SHA-256(\"HelloRetryRequest\") is a retry, and the client must not treat it as a \
             completed key exchange.",
        ),
        ExampleSpec::text("The transcript after a retry")
            .request(
                "Transcript-Hash(ClientHello1, HelloRetryRequest, ClientHello2, ...) =\n\
                 Hash( message_hash || 00 00 Hash.length || Hash(ClientHello1)\n\
                 \u{20}     || HelloRetryRequest || ClientHello2 || ... )",
            )
            .response(
                "`message_hash` is handshake type 254, and the three length bytes are the hash \
                 length — 00 00 20 for SHA-256, 00 00 30 for SHA-384.",
            )
            .note(
                "The replacement lets a stateless server reconstruct the transcript from the \
                 cookie alone: it never has to remember ClientHello1, only its hash. Skip it \
                 and every retried handshake dies at the server's Finished.",
            ),
    ]
}
