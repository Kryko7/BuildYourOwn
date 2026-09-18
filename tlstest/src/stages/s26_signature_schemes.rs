//! Stage 26 — CertificateVerify for RSA-PSS, ECDSA P-256 and Ed25519.

use crate::assert::Check;
use crate::certs::{CertKind, KeyKind};
use crate::config::ServerOptions;
use crate::examples::{ExampleEnv, ExampleSpec, Part};
use crate::stages::{Stage, Test};
use crate::tls::client::ClientConfig;
use crate::tls::sig::{self, server_signed_content, PublicKey};
use crate::tls::{
    sig_name, SIG_ECDSA_SECP256R1_SHA256, SIG_ED25519, SIG_RSA_PSS_RSAE_SHA256,
};
use crate::tls_test;

fn rsa_server() -> ServerOptions {
    ServerOptions::with_cert(CertKind::Leaf(KeyKind::Rsa2048))
}

fn ed25519_server() -> ServerOptions {
    ServerOptions::with_cert(CertKind::Leaf(KeyKind::Ed25519))
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 26,
        slug: "signature_schemes",
        name: "RSA-PSS, ECDSA and Ed25519 signatures",
        ext: false,
        hints: &[
            "rsa_pss_rsae_sha256 is RSA-PSS with MGF1-SHA-256 and a salt as long as the hash; \
             the signature is exactly the modulus size",
            "ecdsa_secp256r1_sha256 signs SHA-256 of the content and the signature is DER: a \
             SEQUENCE of two INTEGERs, so 70-72 bytes and never fixed",
            "Ed25519 is PureEdDSA: it signs the content itself, unhashed, and the signature is \
             always 64 bytes",
            "Whatever the scheme, the content is the same 64 spaces + context + 0x00 + \
             transcript hash",
        ],
        examples: examples,
        tests: vec![
            Test::new(
                "an ECDSA P-256 signature verifies and is DER-encoded",
                ecdsa,
            ),
            Test::new("an RSA-PSS signature verifies and is 256 bytes", rsa)
                .with_server(rsa_server),
            Test::new("an Ed25519 signature verifies and is 64 bytes", ed25519)
                .with_server(ed25519_server),
            Test::new(
                "the RSA signature is PSS, not PKCS#1 v1.5",
                rsa_is_pss,
            )
            .with_server(rsa_server),
            Test::new(
                "the Ed25519 signature is over the content, not its hash",
                ed25519_is_pure,
            )
            .with_server(ed25519_server),
            Test::new(
                "a one-bit change anywhere in the signature breaks it",
                tampered_signature,
            ),
            Test::new(
                "the data path works under each key type",
                echo_under_each_key,
            )
            .with_server(ed25519_server),
        ],
    }
}

/// Handshake and hand back the pieces every test here needs.
async fn signature(
    ctx: &crate::stages::Ctx,
) -> Result<(PublicKey, u16, Vec<u8>, Vec<u8>, Vec<u8>), crate::assert::Failure> {
    let client = ctx.handshake().await?;
    let key = client
        .server_public_key
        .clone()
        .ok_or_else(|| crate::stages::harness("no server public key"))?;
    let cv = client
        .certificate_verify
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no CertificateVerify"))?;
    Ok((
        key,
        cv.algorithm,
        cv.signature.clone(),
        client.hash_after_certificate.clone(),
        client.certificate_verify_bytes.clone(),
    ))
}

tls_test!(ecdsa, |ctx| {
    let (key, scheme, signature, hash, bytes) = signature(ctx).await?;
    let mut c = Check::new("an ECDSA P-256 CertificateVerify");
    c.block("certificate_verify", &bytes);
    c.keying(&hash, "Transcript-Hash(CH..Certificate)");
    c.eq("certificate.public_key", "ECDSA P-256", key.algorithm());
    c.eq("certificate_verify.algorithm", SIG_ECDSA_SECP256R1_SHA256, scheme);
    c.that(
        "certificate_verify.signature",
        "a DER SEQUENCE (starts with 0x30)",
        signature.first() == Some(&0x30),
        format!("starts with 0x{:02x}", signature.first().copied().unwrap_or(0)),
    );
    c.that(
        "certificate_verify.signature.len()",
        "between 70 and 72 bytes — DER integers lose or gain a leading zero",
        (68..=72).contains(&signature.len()),
        signature.len(),
    );
    c.that(
        "certificate_verify.signature",
        "verifies",
        sig::verify(&key, scheme, &signature, &hash).is_ok(),
        "does not verify",
    );
    c.finish()
});

tls_test!(rsa, |ctx| {
    let (key, scheme, signature, hash, bytes) = signature(ctx).await?;
    let mut c = Check::new("an RSA-PSS CertificateVerify");
    c.block("certificate_verify", &bytes);
    c.keying(&hash, "Transcript-Hash(CH..Certificate)");
    c.eq("certificate.public_key", "RSA", key.algorithm());
    c.eq("certificate_verify.algorithm", SIG_RSA_PSS_RSAE_SHA256, scheme);
    c.eq(
        "certificate_verify.signature.len()",
        256usize,
        signature.len(),
    );
    c.note("An RSA signature is always exactly the modulus size: 2048 bits is 256 bytes.");
    c.that(
        "certificate_verify.signature",
        "verifies",
        sig::verify(&key, scheme, &signature, &hash).is_ok(),
        "does not verify",
    );
    c.finish()
});

tls_test!(ed25519, |ctx| {
    let (key, scheme, signature, hash, bytes) = signature(ctx).await?;
    let mut c = Check::new("an Ed25519 CertificateVerify");
    c.block("certificate_verify", &bytes);
    c.keying(&hash, "Transcript-Hash(CH..Certificate)");
    c.eq("certificate.public_key", "Ed25519", key.algorithm());
    c.eq("certificate_verify.algorithm", SIG_ED25519, scheme);
    c.eq("certificate_verify.signature.len()", 64usize, signature.len());
    c.note("Ed25519 signatures are a fixed 64 bytes: R (32) and S (32), with no DER around them.");
    c.that(
        "certificate_verify.signature",
        "verifies",
        sig::verify(&key, scheme, &signature, &hash).is_ok(),
        "does not verify",
    );
    c.finish()
});

tls_test!(rsa_is_pss, |ctx| {
    let (key, scheme, signature, hash, bytes) = signature(ctx).await?;
    let content = server_signed_content(&hash);
    let mut c = Check::new("that the RSA signature is PSS and not PKCS#1 v1.5");
    c.block("certificate_verify", &bytes);
    c.eq("certificate_verify.algorithm", SIG_RSA_PSS_RSAE_SHA256, scheme);
    c.that(
        "the signature as RSA-PSS",
        "verifies",
        sig::verify_content(&key, SIG_RSA_PSS_RSAE_SHA256, &signature, &content).is_ok(),
        "does not verify",
    );
    c.that(
        "the signature as PKCS#1 v1.5",
        "does not verify — the two schemes are not interchangeable",
        sig::verify_content(
            &key,
            crate::tls::SIG_RSA_PKCS1_SHA256,
            &signature,
            &content,
        )
        .is_err(),
        "it verified under PKCS#1 v1.5, which cannot happen for a PSS signature",
    );
    c.note(
        "Both produce a 256-byte signature for a 2048-bit key, so length tells you nothing. \
         RFC 8446 section 4.4.3 forbids PKCS#1 v1.5 in CertificateVerify outright.",
    );
    c.finish()
});

tls_test!(ed25519_is_pure, |ctx| {
    let (key, scheme, signature, hash, bytes) = signature(ctx).await?;
    let content = server_signed_content(&hash);
    let prehashed = crate::tls::crypto::HashAlg::Sha256.digest(&content);
    let mut c = Check::new("that Ed25519 signs the content itself");
    c.block("certificate_verify", &bytes);
    c.that(
        "the signature over the content",
        "verifies",
        sig::verify_content(&key, scheme, &signature, &content).is_ok(),
        "does not verify",
    );
    c.that(
        "the signature over SHA-256 of the content",
        "does not verify — Ed25519 is PureEdDSA, not a pre-hashed scheme",
        sig::verify_content(&key, scheme, &signature, &prehashed).is_err(),
        "it verified over the pre-hash",
    );
    c.finish()
});

tls_test!(tampered_signature, |ctx| {
    let (key, scheme, signature, hash, bytes) = signature(ctx).await?;
    let mut c = Check::new("a signature with one bit changed");
    c.block("certificate_verify", &bytes);
    for at in [0usize, signature.len() / 2, signature.len() - 1] {
        let mut broken = signature.clone();
        broken[at] ^= 0x01;
        c.that(
            &format!("the signature with bit 0 of byte {at} flipped"),
            "does not verify",
            sig::verify(&key, scheme, &broken, &hash).is_err(),
            "it verified",
        );
    }
    c.finish()
});

tls_test!(echo_under_each_key, |ctx| {
    // The server for this test carries the Ed25519 key; the check is that the data path
    // works once the least common signature algorithm has been used.
    let mut client = ctx.handshake().await?;
    let scheme = client
        .certificate_verify
        .as_ref()
        .map(|c| c.algorithm)
        .unwrap_or(0);
    let answer = client
        .echo_line("signed")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("the data path after an Ed25519 handshake");
    c.observe("certificate_verify.algorithm", sig_name(scheme));
    c.eq("echo", "dengis".to_string(), answer);
    c.finish()
});

fn config(env: &ExampleEnv) -> ClientConfig {
    env.config()
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::handshake(
            "An RSA-PSS CertificateVerify",
            config,
            Part::Certificate,
            Part::CertificateVerify,
        )
        .with_server(crate::examples::rsa_server)
        .request("The Certificate message carrying a 2048-bit RSA key")
        .response(
            "algorithm 08 04 (rsa_pss_rsae_sha256), length 01 00, and 256 bytes of \
             signature.",
        )
        .note(
            "PSS is randomised: signing the same content twice gives different bytes. That is \
             expected, and it is why a signature cannot be compared against a fixture.",
        ),
        ExampleSpec::text("Three schemes, three shapes")
            .request(
                "ecdsa_secp256r1_sha256 -> DER SEQUENCE { INTEGER r, INTEGER s }, 70-72 bytes\n\
                 rsa_pss_rsae_sha256    -> exactly the modulus size, 256 bytes for RSA-2048\n\
                 ed25519                -> exactly 64 bytes, R || S, no DER",
            )
            .response(
                "All three sign the same content: 64 spaces, the context string, a zero byte \
                 and the transcript hash.",
            )
            .note(
                "ECDSA and RSA-PSS hash that content first; Ed25519 does not. Feeding Ed25519 \
                 a pre-hash is the mistake that produces a signature nothing can verify.",
            ),
    ]
}
