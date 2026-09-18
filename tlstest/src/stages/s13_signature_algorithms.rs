//! Stage 13 — `signature_algorithms` against the key the server was given.

use crate::assert::Check;
use crate::certs::{CertKind, KeyKind};
use crate::config::ServerOptions;
use crate::examples::{ExampleEnv, ExampleSpec, Part};
use crate::stages::{check_refused_with, provoke_edited_hello, Stage, Test};
use crate::tls::client::ClientConfig;
use crate::tls::msg::signature_algorithms_extension;
use crate::tls::{
    sig_name, AlertDescription, EXT_SIGNATURE_ALGORITHMS, SIG_ECDSA_SECP256R1_SHA256, SIG_ED25519,
    SIG_RSA_PKCS1_SHA256, SIG_RSA_PSS_RSAE_SHA256,
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
        number: 13,
        slug: "signature_algorithms",
        name: "signature_algorithms and the server's key",
        ext: false,
        hints: &[
            "signature_algorithms(13) is mandatory in a TLS 1.3 ClientHello; a hello without \
             it is missing_extension(109)",
            "Pick a scheme the client offered *and* the server's own key can produce: an \
             ECDSA P-256 key cannot sign rsa_pss_rsae_sha256",
            "An RSA key in TLS 1.3 signs with RSA-PSS (0x0804/5/6), never PKCS#1 v1.5 — those \
             code points exist only for certificate signatures",
            "When nothing in the list fits the key, the answer is handshake_failure(40), not \
             a signature the client cannot check",
        ],
        examples,
        tests: vec![
            Test::new("an ECDSA key signs with ecdsa_secp256r1_sha256", ecdsa_key),
            Test::new("an RSA key signs with RSA-PSS", rsa_key).with_server(rsa_server),
            Test::new("an Ed25519 key signs with ed25519", ed25519_key).with_server(ed25519_server),
            Test::new(
                "the chosen scheme is always one the client offered",
                scheme_is_offered,
            ),
            Test::new(
                "an ECDSA server refuses a client that only offers RSA-PSS",
                no_common_scheme,
            ),
            Test::new(
                "an RSA server refuses a client that only offers ECDSA",
                rsa_no_common_scheme,
            )
            .with_server(rsa_server),
            Test::new(
                "a hello with no signature_algorithms extension is refused",
                missing_extension,
            ),
            Test::new(
                "rsa_pkcs1_sha256 alone is not enough for an RSA key",
                pkcs1_is_not_enough,
            )
            .with_server(rsa_server),
        ],
    }
}

/// Handshake offering exactly these schemes, and report the one the server signed with.
async fn signed_with(
    ctx: &crate::stages::Ctx,
    schemes: &[u16],
) -> Result<(u16, Vec<u8>), crate::assert::Failure> {
    let config = ctx.config().with_signature_algorithms(schemes);
    let client = ctx.handshake_with(config).await?;
    let cv = client
        .certificate_verify
        .as_ref()
        .ok_or_else(|| crate::stages::harness("the server sent no CertificateVerify"))?;
    Ok((cv.algorithm, client.certificate_verify_bytes.clone()))
}

tls_test!(ecdsa_key, |ctx| {
    let (scheme, bytes) = signed_with(ctx, &[SIG_ECDSA_SECP256R1_SHA256]).await?;
    let mut c = Check::new("the scheme an ECDSA P-256 key signs with");
    c.block("certificate_verify", &bytes);
    c.mark(4..6);
    c.eq(
        "certificate_verify.algorithm",
        SIG_ECDSA_SECP256R1_SHA256,
        scheme,
    );
    c.finish()
});

tls_test!(rsa_key, |ctx| {
    let (scheme, bytes) = signed_with(ctx, &[SIG_RSA_PSS_RSAE_SHA256]).await?;
    let mut c = Check::new("the scheme an RSA-2048 key signs with");
    c.block("certificate_verify", &bytes);
    c.mark(4..6);
    c.note(
        "'rsae' means an rsaEncryption key used with PSS — the common case. \
         rsa_pss_pss_* is for a certificate whose key is marked RSASSA-PSS.",
    );
    c.eq(
        "certificate_verify.algorithm",
        SIG_RSA_PSS_RSAE_SHA256,
        scheme,
    );
    c.finish()
});

tls_test!(ed25519_key, |ctx| {
    let (scheme, bytes) = signed_with(ctx, &[SIG_ED25519]).await?;
    let mut c = Check::new("the scheme an Ed25519 key signs with");
    c.block("certificate_verify", &bytes);
    c.mark(4..6);
    c.eq("certificate_verify.algorithm", SIG_ED25519, scheme);
    c.finish()
});

tls_test!(scheme_is_offered, |ctx| {
    // Offer everything; whatever comes back must be in the list.
    let offered = crate::tls::msg::default_signature_algorithms();
    let (scheme, bytes) = signed_with(ctx, &offered).await?;
    let mut c = Check::new("that the scheme came from the client's list");
    c.block("certificate_verify", &bytes);
    c.that(
        "certificate_verify.algorithm",
        "one of the schemes the ClientHello offered",
        offered.contains(&scheme),
        sig_name(scheme),
    );
    c.finish()
});

/// Ask for a scheme the server's key cannot produce, and expect a refusal.
async fn expect_no_common_scheme(
    ctx: &mut crate::stages::Ctx,
    schemes: &[u16],
    what: &str,
) -> Result<(), crate::assert::Failure> {
    let config = ctx.config().with_signature_algorithms(schemes);
    let (reaction, bytes) = provoke_edited_hello(ctx, config, |_| {}).await?;
    let mut c = Check::new(format!("what the server does with {what}"));
    c.block("client_hello", &bytes);
    check_refused_with(
        &mut c,
        "the server's reaction",
        &reaction,
        &[
            AlertDescription::HANDSHAKE_FAILURE,
            AlertDescription::ILLEGAL_PARAMETER,
            AlertDescription::MISSING_EXTENSION,
            AlertDescription::INSUFFICIENT_SECURITY,
        ],
    );
    c.finish()
}

tls_test!(no_common_scheme, |ctx| {
    expect_no_common_scheme(
        ctx,
        &[SIG_RSA_PSS_RSAE_SHA256],
        "an ECDSA server and a client that offers only RSA-PSS",
    )
    .await
});

tls_test!(rsa_no_common_scheme, |ctx| {
    expect_no_common_scheme(
        ctx,
        &[SIG_ECDSA_SECP256R1_SHA256, SIG_ED25519],
        "an RSA server and a client that offers only ECDSA and Ed25519",
    )
    .await
});

tls_test!(missing_extension, |ctx| {
    let (reaction, bytes) = provoke_edited_hello(ctx, ctx.config(), |hello| {
        hello
            .extensions
            .retain(|e| e.ext_type != EXT_SIGNATURE_ALGORITHMS);
    })
    .await?;
    let mut c = Check::new("a hello with no signature_algorithms extension");
    c.block("client_hello", &bytes);
    c.note(
        "RFC 8446 section 9.2 makes signature_algorithms mandatory for a client that does \
         certificate-based authentication, and section 4.2 names missing_extension(109).",
    );
    check_refused_with(
        &mut c,
        "the server's reaction",
        &reaction,
        &[
            AlertDescription::MISSING_EXTENSION,
            AlertDescription::HANDSHAKE_FAILURE,
            AlertDescription::ILLEGAL_PARAMETER,
            AlertDescription::DECODE_ERROR,
        ],
    );
    c.finish()
});

tls_test!(pkcs1_is_not_enough, |ctx| {
    expect_no_common_scheme(
        ctx,
        &[SIG_RSA_PKCS1_SHA256],
        "an RSA server and a client that offers only rsa_pkcs1_sha256",
    )
    .await?;
    let mut c = Check::new("why PKCS#1 v1.5 is not enough");
    c.note(
        "RFC 8446 section 4.2.3: the rsa_pkcs1_* code points may appear in \
         signature_algorithms for *certificate* signatures, but must not be used in \
         CertificateVerify. A TLS 1.3 handshake signs with PSS.",
    );
    c.finish()
});

fn ecdsa_config(env: &ExampleEnv) -> ClientConfig {
    env.config()
        .with_signature_algorithms(&[SIG_ECDSA_SECP256R1_SHA256])
}

fn rsa_config(env: &ExampleEnv) -> ClientConfig {
    env.config()
        .with_signature_algorithms(&[SIG_RSA_PSS_RSAE_SHA256])
}

fn examples() -> Vec<ExampleSpec> {
    let _ = signature_algorithms_extension;
    vec![
        ExampleSpec::handshake(
            "A client that offers only ECDSA, and an ECDSA server",
            ecdsa_config,
            Part::ClientHello,
            Part::CertificateVerify,
        )
        .request("signature_algorithms holds one entry: 04 03, ecdsa_secp256r1_sha256")
        .response(
            "CertificateVerify.algorithm is 04 03, followed by a DER-encoded ECDSA signature — \
             a SEQUENCE of two INTEGERs, so its length varies between 70 and 72 bytes.",
        )
        .note(
            "The server picks a scheme that is in the client's list *and* that its own key can \
             produce. Those are two separate conditions and both have to hold.",
        ),
        ExampleSpec::handshake(
            "The same client against an RSA-2048 server",
            rsa_config,
            Part::ClientHello,
            Part::CertificateVerify,
        )
        .with_server(crate::examples::rsa_server)
        .request("signature_algorithms holds 08 04, rsa_pss_rsae_sha256")
        .response(
            "CertificateVerify.algorithm is 08 04 and the signature is exactly 256 bytes — the \
             modulus size of a 2048-bit key.",
        )
        .note(
            "PSS, not PKCS#1 v1.5. The two produce signatures of the same length, so a server \
             that uses the wrong one looks right until a client actually verifies it.",
        ),
    ]
}
