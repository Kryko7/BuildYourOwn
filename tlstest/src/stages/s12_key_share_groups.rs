//! Stage 12 — `key_share`: x25519 and secp256r1.

use crate::assert::Check;
use crate::examples::{ExampleEnv, ExampleSpec, Part};
use crate::stages::{Stage, Test};
use crate::tls::client::ClientConfig;
use crate::tls::msg::KeyExchange;
use crate::tls::{group_name, GROUP_SECP256R1, GROUP_X25519};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 12,
        slug: "key_share_groups",
        name: "key_share: x25519 and secp256r1",
        ext: false,
        hints: &[
            "The ClientHello's key_share carries a list of (group, key_exchange) pairs; the \
             ServerHello's carries exactly one",
            "x25519 key_exchange is 32 raw bytes; secp256r1 is a 65-byte uncompressed point \
             starting with 0x04",
            "The shared secret is the raw X coordinate — 32 bytes for both of these groups — \
             and goes straight into HKDF-Extract as the IKM",
            "Reject an all-zero x25519 result: it means the peer sent a small-order point \
             (RFC 8446 section 7.4.2, RFC 7748 section 6.1)",
        ],
        examples: examples,
        tests: vec![
            Test::new("a hello sharing only x25519 completes", x25519_only),
            Test::new("a hello sharing only secp256r1 completes", p256_only),
            Test::new(
                "a hello sharing both groups gets exactly one back",
                both_groups,
            ),
            Test::new(
                "the server's key_share names a group the client shared",
                share_is_offered,
            ),
            Test::new(
                "an x25519 key_exchange is exactly 32 bytes",
                x25519_length,
            ),
            Test::new(
                "a secp256r1 key_exchange is a 65-byte uncompressed point",
                p256_length,
            ),
            Test::new(
                "the shared secret is 32 bytes for both groups",
                shared_secret_length,
            ),
            Test::new(
                "the data path works under each group",
                echo_under_each_group,
            ),
        ],
    }
}

tls_test!(x25519_only, |ctx| {
    let client = ctx
        .handshake_with(ctx.config().with_groups(&[GROUP_X25519]))
        .await?;
    let mut c = Check::new("a handshake over x25519 alone");
    c.block("server_hello", &client.server_hello_bytes);
    c.eq(
        "server_hello.key_share.group",
        GROUP_X25519,
        client.selected_group.unwrap_or(0),
    );
    c.finish()
});

tls_test!(p256_only, |ctx| {
    let client = ctx
        .handshake_with(ctx.config().with_groups(&[GROUP_SECP256R1]))
        .await?;
    let mut c = Check::new("a handshake over secp256r1 alone");
    c.block("server_hello", &client.server_hello_bytes);
    c.eq(
        "server_hello.key_share.group",
        GROUP_SECP256R1,
        client.selected_group.unwrap_or(0),
    );
    c.finish()
});

tls_test!(both_groups, |ctx| {
    let client = ctx
        .handshake_with(ctx.config().with_groups(&[GROUP_X25519, GROUP_SECP256R1]))
        .await?;
    let hello = client
        .server_hello
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no ServerHello"))?;
    let share = hello
        .key_share()
        .map_err(crate::assert::Failure::tls)?
        .ok_or_else(|| crate::stages::harness("the ServerHello carried no key_share"))?;
    let mut c = Check::new("a hello that shares both groups");
    c.block("server_hello", &client.server_hello_bytes);
    c.note("The ServerHello's key_share is one entry, never a list.");
    c.that(
        "server_hello.key_share.group",
        "x25519 or secp256r1",
        [GROUP_X25519, GROUP_SECP256R1].contains(&share.0),
        group_name(share.0),
    );
    c.finish()
});

tls_test!(share_is_offered, |ctx| {
    // Share only secp256r1 while naming both in supported_groups. A server that picks
    // x25519 would be asking for a share it was never sent.
    let config = ctx
        .config()
        .with_group_shares(&[GROUP_X25519, GROUP_SECP256R1], &[GROUP_SECP256R1]);
    let client = ctx.handshake_with(config).await?;
    let mut c = Check::new("that the server's group was one the client shared");
    c.block("client_hello", &client.client_hello_bytes);
    c.block("server_hello", &client.server_hello_bytes);
    c.note(
        "supported_groups says what the client *can* do; key_share says what it has already \
         computed. A server that wants a group from the first list but not the second has to \
         send a HelloRetryRequest — stage 41.",
    );
    c.eq(
        "server_hello.key_share.group",
        GROUP_SECP256R1,
        client.selected_group.unwrap_or(0),
    );
    c.finish()
});

tls_test!(x25519_length, |ctx| {
    let client = ctx
        .handshake_with(ctx.config().with_groups(&[GROUP_X25519]))
        .await?;
    let hello = client
        .server_hello
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no ServerHello"))?;
    let (group, share) = hello
        .key_share()
        .map_err(crate::assert::Failure::tls)?
        .ok_or_else(|| crate::stages::harness("no key_share"))?;
    let mut c = Check::new("the length of the server's x25519 share");
    c.block("server_hello", &client.server_hello_bytes);
    c.eq("server_hello.key_share.group", GROUP_X25519, group);
    c.eq(
        "server_hello.key_share.key_exchange.len()",
        KeyExchange::share_len(GROUP_X25519).unwrap_or(0),
        share.len(),
    );
    c.finish()
});

tls_test!(p256_length, |ctx| {
    let client = ctx
        .handshake_with(ctx.config().with_groups(&[GROUP_SECP256R1]))
        .await?;
    let hello = client
        .server_hello
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no ServerHello"))?;
    let (group, share) = hello
        .key_share()
        .map_err(crate::assert::Failure::tls)?
        .ok_or_else(|| crate::stages::harness("no key_share"))?;
    let mut c = Check::new("the shape of the server's secp256r1 share");
    c.block("server_hello", &client.server_hello_bytes);
    c.eq("server_hello.key_share.group", GROUP_SECP256R1, group);
    c.eq(
        "server_hello.key_share.key_exchange.len()",
        KeyExchange::share_len(GROUP_SECP256R1).unwrap_or(0),
        share.len(),
    );
    c.eq(
        "server_hello.key_share.key_exchange[0]",
        0x04u8,
        share.first().copied().unwrap_or(0),
    );
    c.note("0x04 is the SEC1 tag for an uncompressed point; TLS 1.3 never uses compressed ones.");
    c.finish()
});

tls_test!(shared_secret_length, |ctx| {
    for group in [GROUP_X25519, GROUP_SECP256R1] {
        let client = ctx
            .handshake_with(ctx.config().with_groups(&[group]))
            .await?;
        let mut c = Check::new(format!("the shared secret over {}", group_name(group)));
        c.note(
            "Both of these groups produce a 32-byte secret: the raw u-coordinate for x25519, \
             the X coordinate for P-256. That is the IKM of HKDF-Extract.",
        );
        c.eq(
            "(EC)DHE shared secret length",
            32usize,
            client.shared_secret.len(),
        );
        c.ne("(EC)DHE shared secret", vec![0u8; 32], client.shared_secret.clone());
        c.finish()?;
    }
    Ok(())
});

tls_test!(echo_under_each_group, |ctx| {
    for (i, group) in [GROUP_X25519, GROUP_SECP256R1].into_iter().enumerate() {
        let config = ctx.config_n(i as u64).with_groups(&[group]);
        let mut client = ctx.handshake_with(config).await?;
        let line = format!("group{i}");
        let answer = client.echo_line(&line).await.map_err(|e| {
            crate::stages::handshake_failure(e, &client)
                .note(format!("echoing over {}", group_name(group)))
        })?;
        let mut c = Check::new(format!("the echo over {}", group_name(group)));
        c.eq("echo", crate::stages::reversed(&line), answer);
        c.finish()?;
    }
    Ok(())
});

fn x25519_config(env: &ExampleEnv) -> ClientConfig {
    env.config().with_groups(&[GROUP_X25519])
}

fn p256_config(env: &ExampleEnv) -> ClientConfig {
    env.config().with_groups(&[GROUP_SECP256R1])
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::handshake(
            "An x25519 key share, and the one that comes back",
            x25519_config,
            Part::ClientHello,
            Part::ServerHello,
        )
        .request(
            "key_share holds one entry: group 0x001d and a 32-byte key_exchange — the client's \
             ephemeral public key",
        )
        .response(
            "key_share holds one entry, not a list: group 0x001d and the server's own 32 \
             bytes.",
        )
        .note(
            "X25519(client_private, server_public) and X25519(server_private, client_public) \
             are the same 32 bytes, and that value is the (EC)DHE input to HKDF-Extract.",
        ),
        ExampleSpec::handshake(
            "The same handshake over secp256r1",
            p256_config,
            Part::ClientHello,
            Part::ServerHello,
        )
        .request("key_share group 0x0017 with a 65-byte key_exchange beginning 04")
        .response("key_share group 0x0017 with the server's 65-byte point.")
        .note(
            "65 bytes is `04 || X || Y`, uncompressed SEC1. The shared secret is the X \
             coordinate only — 32 bytes, not 64.",
        ),
    ]
}
