//! Stage 31 — The data path under every cipher suite and group.

use crate::assert::Check;
use crate::examples::{ExampleEnv, ExampleSpec, Part};
use crate::stages::{payload_line, reversed, Stage, Test};
use crate::tls::client::ClientConfig;
use crate::tls::{
    group_name, suite_name, ALL_SUITES, GROUP_SECP256R1, GROUP_X25519, TLS_AES_128_GCM_SHA256,
    TLS_AES_256_GCM_SHA384, TLS_CHACHA20_POLY1305_SHA256,
};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 31,
        slug: "echo_all_suites",
        name: "The data path under every suite",
        ext: false,
        hints: &[
            "The record layer does not care which AEAD it is using: same framing, same inner \
             content type, same nonce rule",
            "What does change is the key length (16 or 32 bytes) and, for the SHA-384 suite, \
             every secret and transcript hash in the schedule",
            "ChaCha20-Poly1305 has no block size, so its ciphertext is exactly the plaintext \
             length plus the 16-byte tag — the same as GCM here",
            "Test the data path under each suite separately: a key-schedule bug that only \
             affects SHA-384 is invisible under the other two",
        ],
        examples,
        tests: vec![
            Test::new("TLS_AES_128_GCM_SHA256 carries data", aes128),
            Test::new("TLS_AES_256_GCM_SHA384 carries data", aes256),
            Test::new("TLS_CHACHA20_POLY1305_SHA256 carries data", chacha),
            Test::new(
                "many lines in a row under every suite",
                many_lines_every_suite,
            ),
            Test::new(
                "the ciphertext is the plaintext plus one type byte and a 16-byte tag",
                ciphertext_overhead,
            ),
            Test::new("every suite works over x25519", suites_over_x25519),
            Test::new("every suite works over secp256r1", suites_over_p256),
        ],
    }
}

/// Echo one line under a given suite, and check the answer.
async fn echo_under(
    ctx: &crate::stages::Ctx,
    suite: u16,
    line: &str,
) -> Result<(), crate::assert::Failure> {
    let mut client = ctx
        .handshake_with(ctx.config().with_suites(&[suite]))
        .await?;
    let answer = client.echo_line(line).await.map_err(|e| {
        crate::stages::handshake_failure(e, &client)
            .note(format!("echoing under {}", suite_name(suite)))
    })?;
    let mut c = Check::new(format!("the data path under {}", suite_name(suite)));
    c.note_all(client.schedule_lines());
    c.eq("echo", reversed(line), answer);
    c.finish()
}

tls_test!(aes128, |ctx| {
    echo_under(ctx, TLS_AES_128_GCM_SHA256, "aes-one-two-eight").await
});

tls_test!(aes256, |ctx| {
    echo_under(ctx, TLS_AES_256_GCM_SHA384, "aes-two-five-six").await
});

tls_test!(chacha, |ctx| {
    echo_under(ctx, TLS_CHACHA20_POLY1305_SHA256, "chacha-poly").await
});

tls_test!(many_lines_every_suite, |ctx| {
    for suite in ALL_SUITES {
        let mut client = ctx
            .handshake_with(ctx.config().with_suites(&[suite]))
            .await?;
        let mut c = Check::new(format!("ten lines under {}", suite_name(suite)));
        for i in 0..10 {
            let line = payload_line(16 + i, &mut ctx.rng);
            let answer = client.echo_line(&line).await.map_err(|e| {
                crate::stages::handshake_failure(e, &client).note(format!(
                    "line {i} under {} — the nonce for it is iv XOR {i}",
                    suite_name(suite)
                ))
            })?;
            c.eq(&format!("echo of line {i}"), reversed(&line), answer);
        }
        c.finish()?;
    }
    Ok(())
});

tls_test!(ciphertext_overhead, |ctx| {
    for suite in ALL_SUITES {
        let mut client = ctx
            .handshake_with(ctx.config().with_suites(&[suite]))
            .await?;
        let line = payload_line(32, &mut ctx.rng);
        let before = client.conn.records_in.len();
        let answer = client
            .echo_line(&line)
            .await
            .map_err(|e| crate::stages::handshake_failure(e, &client))?;
        let record = client
            .conn
            .records_in
            .iter()
            .skip(before)
            .find(|r| r.content_type == crate::tls::ContentType::ApplicationData)
            .ok_or_else(|| crate::stages::harness("no application record came back"))?;
        let mut c = Check::new(format!(
            "the size of an answer record under {}",
            suite_name(suite)
        ));
        c.block("the record the server sent", &record.raw);
        c.note(
            "plaintext + 1 byte of inner content type + 16 bytes of tag, and no padding — \
             which is what a server that sends no padding produces.",
        );
        c.eq("echo", reversed(&line), answer);
        c.at_least(
            "record.length",
            line.len() + 1 + 1 + 16,
            record.fragment.len(),
        );
        c.at_most(
            "record.length",
            line.len() + 1 + 1 + 16 + 256,
            record.fragment.len(),
        );
        c.finish()?;
    }
    Ok(())
});

/// Echo under every suite over one named group.
async fn suites_over(ctx: &crate::stages::Ctx, group: u16) -> Result<(), crate::assert::Failure> {
    for suite in ALL_SUITES {
        let config = ctx.config().with_suites(&[suite]).with_groups(&[group]);
        let mut client = ctx.handshake_with(config).await?;
        let line = format!("{}-{:x}", group_name(group), suite);
        let answer = client.echo_line(&line).await.map_err(|e| {
            crate::stages::handshake_failure(e, &client).note(format!(
                "{} over {}",
                suite_name(suite),
                group_name(group)
            ))
        })?;
        let mut c = Check::new(format!("{} over {}", suite_name(suite), group_name(group)));
        c.eq("echo", reversed(&line), answer);
        c.finish()?;
    }
    Ok(())
}

tls_test!(suites_over_x25519, |ctx| {
    suites_over(ctx, GROUP_X25519).await
});

tls_test!(suites_over_p256, |ctx| {
    suites_over(ctx, GROUP_SECP256R1).await
});

fn chacha_config(env: &ExampleEnv) -> ClientConfig {
    env.config().with_suites(&[TLS_CHACHA20_POLY1305_SHA256])
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::echo("An echo under AES-128-GCM", "suite")
            .request("`suite\\n` as one application_data record under TLS_AES_128_GCM_SHA256")
            .response("`etius\\n`, the same way back.")
            .note(
                "The record layer is suite-agnostic: change the AEAD and the bytes on the wire \
                 keep the same shape. Only the key length and the tag's contents differ.",
            ),
        ExampleSpec::handshake(
            "The ServerHello that picked ChaCha20-Poly1305",
            chacha_config,
            Part::ClientHello,
            Part::ServerHello,
        )
        .request("cipher_suites holds only 13 03")
        .response("server_hello.cipher_suite is 13 03; everything else looks identical.")
        .note(
            "The suite changes the AEAD and, for 0x1302, the hash — and nothing else about the \
             record layer. Same five header bytes, same inner content type, same nonce rule.",
        ),
    ]
}
