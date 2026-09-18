//! Stage 27 — The server's Finished.

use crate::assert::Check;
use crate::examples::{ExampleEnv, ExampleSpec, Part};
use crate::stages::{Stage, Test};
use crate::tls::client::ClientConfig;
use crate::tls::crypto::hkdf_expand_label;
use crate::tls::{hex, ALL_SUITES};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 27,
        slug: "server_finished",
        name: "The server's Finished",
        ext: false,
        hints: &[
            "finished_key = HKDF-Expand-Label(server_handshake_traffic_secret, \"finished\", \
             \"\", Hash.length) — the context is empty",
            "verify_data = HMAC(finished_key, Transcript-Hash(everything before this message))",
            "The whole body is the verify_data: no length prefix, no algorithm field, just \
             Hash.length bytes",
            "Finished is the last message of the server's flight, and the transcript it \
             covers is the one right after CertificateVerify",
        ],
        examples: examples,
        tests: vec![
            Test::new("verify_data is Hash.length bytes", length),
            Test::new(
                "verify_data is the HMAC the RFC describes",
                verify_data,
            ),
            Test::new(
                "the finished key comes from the handshake traffic secret",
                finished_key,
            ),
            Test::new(
                "a Finished computed with the client's key does not match",
                wrong_direction,
            ),
            Test::new(
                "a Finished over the wrong transcript does not match",
                wrong_transcript,
            ),
            Test::new(
                "the body is the verify_data and nothing else",
                body_is_bare,
            ),
            Test::new("it works under every cipher suite", every_suite),
        ],
    }
}

tls_test!(length, |ctx| {
    let client = ctx.handshake().await?;
    let suite = client
        .suite
        .ok_or_else(|| crate::stages::harness("no suite"))?;
    let mut c = Check::new("the length of the server's verify_data");
    c.block("server finished", &client.server_finished_bytes);
    c.note(format!(
        "{} means {}, so verify_data is {} bytes",
        suite.name(),
        suite.hash.name(),
        suite.hash.len()
    ));
    c.eq(
        "finished.verify_data.len()",
        suite.hash.len(),
        client.server_verify_data.len(),
    );
    c.eq(
        "finished.length",
        suite.hash.len(),
        client.server_finished_bytes.len().saturating_sub(4),
    );
    c.finish()
});

tls_test!(verify_data, |ctx| {
    let client = ctx.handshake().await?;
    let schedule = client
        .schedule
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no key schedule"))?;
    let keys = schedule
        .server_handshake
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no server handshake keys"))?;
    let want = schedule
        .verify_data(keys, &client.hash_before_server_finished)
        .map_err(crate::assert::Failure::tls)?;
    let mut c = Check::new("the server's verify_data");
    c.block("server finished", &client.server_finished_bytes);
    c.keying(
        &client.hash_before_server_finished,
        "s hs traffic → finished_key → HMAC",
    );
    c.note(
        "This is what proves the server holds the handshake keys — it is a MAC, not a \
         signature, and it covers everything both sides have said.",
    );
    c.bytes_eq("finished.verify_data", &want, &client.server_verify_data);
    c.finish()
});

tls_test!(finished_key, |ctx| {
    let client = ctx.handshake().await?;
    let schedule = client
        .schedule
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no key schedule"))?;
    let keys = schedule
        .server_handshake
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no server handshake keys"))?;
    let hash = schedule.suite.hash;
    let want = hkdf_expand_label(hash, &keys.secret, "finished", &[], hash.len())
        .map_err(crate::assert::Failure::tls)?;
    let have = keys.finished_key().map_err(crate::assert::Failure::tls)?;
    let mut c = Check::new("the finished key");
    c.note(
        "It is expanded from the traffic secret, not from the write key: a separate label so \
         that the MAC key and the record key are independent.",
    );
    c.bytes_eq("finished_key", &want, &have);
    c.eq("finished_key.len()", hash.len(), have.len());
    c.ne("finished_key vs server_write_key", hex(&keys.key), hex(&have));
    c.finish()
});

tls_test!(wrong_direction, |ctx| {
    let client = ctx.handshake().await?;
    let schedule = client
        .schedule
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no key schedule"))?;
    let client_keys = schedule
        .client_handshake
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no client handshake keys"))?;
    let with_client_key = schedule
        .verify_data(client_keys, &client.hash_before_server_finished)
        .map_err(crate::assert::Failure::tls)?;
    let mut c = Check::new("a verify_data computed with the client's finished key");
    c.block("server finished", &client.server_finished_bytes);
    c.note(
        "The two directions have different traffic secrets, so they have different finished \
         keys. Using the wrong one is a silent failure that only shows up here.",
    );
    c.ne(
        "HMAC with the client's finished key",
        hex(&client.server_verify_data),
        hex(&with_client_key),
    );
    c.finish()
});

tls_test!(wrong_transcript, |ctx| {
    let client = ctx.handshake().await?;
    let schedule = client
        .schedule
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no key schedule"))?;
    let keys = schedule
        .server_handshake
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no server handshake keys"))?;
    let mut c = Check::new("verify_data over neighbouring transcript boundaries");
    c.block("server finished", &client.server_finished_bytes);
    c.note_all(client.transcript_lines());
    for (name, hash) in [
        ("CH..Certificate", &client.hash_after_certificate),
        (
            "CH..EncryptedExtensions",
            &client.hash_after_encrypted_extensions,
        ),
        (
            "CH..server Finished (including the message itself)",
            &client.hash_after_server_finished,
        ),
    ] {
        if hash.is_empty() {
            continue;
        }
        let other = schedule
            .verify_data(keys, hash)
            .map_err(crate::assert::Failure::tls)?;
        c.ne(
            &format!("verify_data over Transcript-Hash({name})"),
            hex(&client.server_verify_data),
            hex(&other),
        );
    }
    c.finish()
});

tls_test!(body_is_bare, |ctx| {
    let client = ctx.handshake().await?;
    let bytes = &client.server_finished_bytes;
    let mut c = Check::new("the shape of a Finished message");
    c.block("server finished", bytes);
    c.note(
        "`14`, a uint24 length, and then Hash.length bytes. There is no inner structure at \
         all — the message is the MAC.",
    );
    c.eq(
        "finished.msg_type",
        crate::tls::HandshakeType::FINISHED.0,
        bytes.first().copied().unwrap_or(0),
    );
    if bytes.len() >= 4 {
        let body = u32::from_be_bytes([0, bytes[1], bytes[2], bytes[3]]) as usize;
        c.eq("finished.length", bytes.len() - 4, body);
        c.bytes_eq("finished body", &client.server_verify_data, &bytes[4..]);
    }
    c.finish()
});

tls_test!(every_suite, |ctx| {
    for suite in ALL_SUITES {
        let client = ctx.handshake_with(ctx.config().with_suites(&[suite])).await?;
        let schedule = client
            .schedule
            .as_ref()
            .ok_or_else(|| crate::stages::harness("no key schedule"))?;
        let keys = schedule
            .server_handshake
            .as_ref()
            .ok_or_else(|| crate::stages::harness("no server handshake keys"))?;
        let want = schedule
            .verify_data(keys, &client.hash_before_server_finished)
            .map_err(crate::assert::Failure::tls)?;
        let mut c = Check::new(format!(
            "the server's Finished under {}",
            crate::tls::suite_name(suite)
        ));
        c.block("server finished", &client.server_finished_bytes);
        c.eq(
            "finished.verify_data.len()",
            schedule.suite.hash.len(),
            client.server_verify_data.len(),
        );
        c.bytes_eq("finished.verify_data", &want, &client.server_verify_data);
        c.finish()?;
    }
    Ok(())
});

fn config(env: &ExampleEnv) -> ClientConfig {
    env.config()
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::handshake(
            "The message that ends the server's flight",
            config,
            Part::CertificateVerify,
            Part::ServerFinished,
        )
        .request("The CertificateVerify that comes immediately before it")
        .response(
            "`14`, `00 00 20` (or `00 00 30` under SHA-384), and the verify_data — an HMAC and \
             nothing else.",
        )
        .note(
            "Once this verifies, the client knows the server holds the handshake keys and that \
             neither side's view of the transcript has been tampered with.",
        ),
        ExampleSpec::text("Signature and MAC, and why both")
            .request(
                "CertificateVerify proves possession of the certificate's private key over the \
                 transcript up to Certificate.",
            )
            .response(
                "Finished proves possession of the handshake traffic secret — that is, of the \
                 (EC)DHE shared secret — over the transcript up to CertificateVerify.",
            )
            .note(
                "The signature authenticates the identity; the MAC binds that identity to \
                 *this* key exchange. Without the Finished, a signature captured from another \
                 connection could be replayed.",
            ),
    ]
}
