//! Stage 25 — CertificateVerify: the context string and the transcript.

use crate::assert::Check;
use crate::examples::{ExampleEnv, ExampleSpec, Part};
use crate::stages::{Stage, Test};
use crate::tls::client::ClientConfig;
use crate::tls::sig::{self, server_signed_content, signed_content};
use crate::tls::{hex, CLIENT_CV_CONTEXT, SERVER_CV_CONTEXT};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 25,
        slug: "certificate_verify",
        name: "CertificateVerify: context and transcript",
        ext: false,
        hints: &[
            "The signed content is 64 bytes of 0x20, then \"TLS 1.3, server \
             CertificateVerify\", then a single 0x00, then Transcript-Hash(CH..Certificate)",
            "The 64 spaces and the context string exist so a signature made here can never be \
             replayed as a TLS 1.2 one, or as a client's",
            "The body is algorithm (a uint16 SignatureScheme) and signature (an \
             opaque<0..2^16-1>) — nothing else",
            "Sign the transcript hash as it stood *before* this message; the message cannot \
             cover itself",
        ],
        examples,
        tests: vec![
            Test::new(
                "the signature verifies over the documented content",
                verifies,
            ),
            Test::new(
                "it does not verify over the transcript hash alone",
                not_bare_hash,
            ),
            Test::new(
                "it does not verify under the client context string",
                not_client_context,
            ),
            Test::new(
                "it does not verify over the wrong transcript boundary",
                not_wrong_boundary,
            ),
            Test::new(
                "the 64 leading spaces are part of the signed content",
                spaces_matter,
            ),
            Test::new("the body is exactly algorithm and signature", body_shape),
            Test::new(
                "the signature is fresh on every connection",
                fresh_each_time,
            ),
        ],
    }
}

tls_test!(verifies, |ctx| {
    let client = ctx.handshake().await?;
    let key = client
        .server_public_key
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no server public key"))?;
    let cv = client
        .certificate_verify
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no CertificateVerify"))?;
    let content = server_signed_content(&client.hash_after_certificate);
    let mut c = Check::new("the CertificateVerify signature");
    c.block("certificate_verify", &client.certificate_verify_bytes);
    c.block("the bytes that were signed", &content);
    c.keying(
        &client.hash_after_certificate,
        "Transcript-Hash(CH..Certificate)",
    );
    c.note(format!(
        "content = 64 × 0x20 || {SERVER_CV_CONTEXT:?} || 0x00 || the transcript hash \
         ({} bytes in total)",
        content.len()
    ));
    c.that(
        "certificate_verify.signature",
        "verifies against the leaf certificate's public key",
        sig::verify_content(key, cv.algorithm, &cv.signature, &content).is_ok(),
        sig::verify_content(key, cv.algorithm, &cv.signature, &content)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default(),
    );
    c.finish()
});

/// Prove that `content` is *not* what was signed.
fn must_not_verify(c: &mut Check, path: &str, client: &crate::tls::client::Client, content: &[u8]) {
    let (Some(key), Some(cv)) = (
        client.server_public_key.as_ref(),
        client.certificate_verify.as_ref(),
    ) else {
        return;
    };
    c.that(
        path,
        "does not verify — this is not what gets signed",
        sig::verify_content(key, cv.algorithm, &cv.signature, content).is_err(),
        "it verified, which would mean the context string does nothing",
    );
}

tls_test!(not_bare_hash, |ctx| {
    let client = ctx.handshake().await?;
    let mut c = Check::new("the signature against the bare transcript hash");
    c.block("certificate_verify", &client.certificate_verify_bytes);
    c.note(
        "Signing the transcript hash directly is the most natural mistake, and it produces a \
         handshake that only ever works against another implementation with the same bug.",
    );
    must_not_verify(
        &mut c,
        "the signature over Transcript-Hash(CH..Certificate) alone",
        &client,
        &client.hash_after_certificate,
    );
    c.finish()
});

tls_test!(not_client_context, |ctx| {
    let client = ctx.handshake().await?;
    let content = signed_content(CLIENT_CV_CONTEXT, &client.hash_after_certificate);
    let mut c = Check::new("the signature under the client's context string");
    c.block("certificate_verify", &client.certificate_verify_bytes);
    c.note(format!(
        "the client's string is {CLIENT_CV_CONTEXT:?} — one word different, and that is the \
         point: a server's signature must not be usable as a client's"
    ));
    must_not_verify(
        &mut c,
        "the signature over the client context",
        &client,
        &content,
    );
    c.finish()
});

tls_test!(not_wrong_boundary, |ctx| {
    let client = ctx.handshake().await?;
    let mut c = Check::new("the signature over neighbouring transcript boundaries");
    c.block("certificate_verify", &client.certificate_verify_bytes);
    c.note_all(client.transcript_lines());
    must_not_verify(
        &mut c,
        "the signature over Transcript-Hash(CH..EncryptedExtensions)",
        &client,
        &server_signed_content(&client.hash_after_encrypted_extensions),
    );
    must_not_verify(
        &mut c,
        "the signature over Transcript-Hash(CH..CertificateVerify)",
        &client,
        &server_signed_content(&client.hash_after_certificate_verify),
    );
    must_not_verify(
        &mut c,
        "the signature over Transcript-Hash(CH..ServerHello)",
        &client,
        &server_signed_content(&client.hash_after_server_hello),
    );
    c.finish()
});

tls_test!(spaces_matter, |ctx| {
    let client = ctx.handshake().await?;
    // The same content with the 64 spaces removed.
    let mut without = Vec::new();
    without.extend_from_slice(SERVER_CV_CONTEXT.as_bytes());
    without.push(0);
    without.extend_from_slice(&client.hash_after_certificate);
    let mut c = Check::new("the 64 leading spaces");
    c.block("certificate_verify", &client.certificate_verify_bytes);
    c.note(
        "RFC 8446 section 4.4.3: the padding is there so that a TLS 1.2 signature — which \
         starts with the client and server randoms — can never be a valid 1.3 one and vice \
         versa.",
    );
    must_not_verify(
        &mut c,
        "the signature over the content without its 64 spaces",
        &client,
        &without,
    );
    // And a separator other than zero.
    let mut wrong_separator = vec![0x20u8; 64];
    wrong_separator.extend_from_slice(SERVER_CV_CONTEXT.as_bytes());
    wrong_separator.push(0x20);
    wrong_separator.extend_from_slice(&client.hash_after_certificate);
    must_not_verify(
        &mut c,
        "the signature over the content with a space instead of the 0x00 separator",
        &client,
        &wrong_separator,
    );
    c.finish()
});

tls_test!(body_shape, |ctx| {
    let client = ctx.handshake().await?;
    let bytes = &client.certificate_verify_bytes;
    let cv = client
        .certificate_verify
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no CertificateVerify"))?;
    let mut c = Check::new("the shape of a CertificateVerify body");
    c.block("certificate_verify", bytes);
    if bytes.len() >= 8 {
        c.mark(4..6);
        let body = u32::from_be_bytes([0, bytes[1], bytes[2], bytes[3]]) as usize;
        let sig_len = u16::from_be_bytes([bytes[6], bytes[7]]) as usize;
        c.eq("certificate_verify.length", bytes.len() - 4, body);
        c.eq(
            "certificate_verify.signature.length",
            cv.signature.len(),
            sig_len,
        );
        c.eq(
            "certificate_verify.length accounted for",
            2 + 2 + sig_len,
            body,
        );
        c.observe(
            "certificate_verify.algorithm",
            crate::tls::sig_name(cv.algorithm),
        );
    } else {
        c.that(
            "certificate_verify",
            "at least eight bytes",
            false,
            bytes.len(),
        );
    }
    c.finish()
});

tls_test!(fresh_each_time, |ctx| {
    let first = {
        let client = ctx.handshake_with(ctx.config_n(1)).await?;
        client
            .certificate_verify
            .as_ref()
            .map(|c| c.signature.clone())
            .ok_or_else(|| crate::stages::harness("no CertificateVerify"))?
    };
    let second = {
        let client = ctx.handshake_with(ctx.config_n(2)).await?;
        client
            .certificate_verify
            .as_ref()
            .map(|c| c.signature.clone())
            .ok_or_else(|| crate::stages::harness("no CertificateVerify"))?
    };
    let mut c = Check::new("the signature on two different connections");
    c.note(
        "The transcript contains both randoms and both key shares, so the signed content is \
         different every time even for the same client. A repeated signature would mean the \
         transcript was not really being signed.",
    );
    c.ne(
        "certificate_verify.signature (second connection)",
        hex(&first),
        hex(&second),
    );
    c.finish()
});

fn config(env: &ExampleEnv) -> ClientConfig {
    env.config()
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::handshake(
            "A CertificateVerify message",
            config,
            Part::Certificate,
            Part::CertificateVerify,
        )
        .request("The Certificate message whose hash this signature covers")
        .response(
            "`0f`, a uint24 length, a uint16 SignatureScheme, a uint16 signature length, and \
             the signature.",
        )
        .note(
            "The signature is over content the message does not contain: 64 spaces, a context \
             string, a zero byte and the transcript hash up to and including Certificate.",
        ),
        ExampleSpec::text("The bytes that are signed")
            .request(
                "20 20 20 ... 20                                   (64 bytes)\n\
                 \"TLS 1.3, server CertificateVerify\"               (33 bytes)\n\
                 00                                                (1 byte)\n\
                 Transcript-Hash(ClientHello .. Certificate)        (32 or 48 bytes)",
            )
            .response(
                "130 bytes under SHA-256, or 146 under SHA-384. That string is hashed by the \
                 signature algorithm — except for Ed25519, which signs it directly.",
            )
            .note(
                "Every element earns its place: the spaces defeat cross-protocol replay, the \
                 string separates client from server, the zero byte stops the string running \
                 into the hash.",
            ),
    ]
}
