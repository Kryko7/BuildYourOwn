//! Stage 24 — The Certificate message.

use crate::assert::Check;
use crate::certs::CertKind;
use crate::config::ServerOptions;
use crate::examples::{ExampleEnv, ExampleSpec, Part};
use crate::stages::{Stage, Test};
use crate::tls::client::ClientConfig;
use crate::tls::sig::PublicKey;
use crate::tls_test;
use x509_parser::prelude::FromDer;

fn chain_server() -> ServerOptions {
    ServerOptions::with_cert(CertKind::Chain)
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 24,
        slug: "certificate",
        name: "The Certificate message",
        ext: false,
        hints: &[
            "The body is certificate_request_context (an opaque<0..255>, always empty from a \
             server) and then certificate_list, an opaque<0..2^24-1>",
            "Each entry is cert_data (uint24-length DER) followed by its own extensions block \
             — that per-entry block is new in TLS 1.3 and is usually two zero bytes",
            "The end-entity certificate comes first, and each later entry should certify the \
             one before it; the self-signed root may be left out",
            "The certificate is sent encrypted, under the server handshake traffic keys, so \
             it is not visible to a passive observer",
        ],
        examples,
        tests: vec![
            Test::new(
                "certificate_request_context is empty",
                empty_request_context,
            ),
            Test::new("the list holds at least one certificate", has_a_leaf),
            Test::new(
                "the first entry is a parsable X.509 certificate",
                leaf_parses,
            ),
            Test::new(
                "the leaf's public key is the one that signed CertificateVerify",
                leaf_key_matches,
            ),
            Test::new(
                "every entry carries its own extensions block",
                per_entry_extensions,
            ),
            Test::new(
                "the message's lengths agree at all three levels",
                lengths_agree,
            ),
            Test::new("a three-certificate chain arrives leaf first", chain_order)
                .with_server(chain_server),
            Test::new(
                "the Certificate message arrives encrypted, not in the clear",
                arrives_encrypted,
            ),
        ],
    }
}

tls_test!(empty_request_context, |ctx| {
    let client = ctx.handshake().await?;
    let cert = client
        .certificate
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no Certificate message"))?;
    let mut c = Check::new("Certificate.certificate_request_context");
    c.block("certificate", &client.certificate_bytes);
    c.mark(4..5);
    c.note(
        "The field exists for post-handshake authentication, where a server echoes the \
         context from its CertificateRequest. In the server's own Certificate it is always \
         zero-length.",
    );
    c.eq(
        "certificate.certificate_request_context.len()",
        0usize,
        cert.request_context.len(),
    );
    c.finish()
});

tls_test!(has_a_leaf, |ctx| {
    let client = ctx.handshake().await?;
    let cert = client
        .certificate
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no Certificate message"))?;
    let mut c = Check::new("the certificate list");
    c.block("certificate", &client.certificate_bytes);
    c.at_least(
        "certificate.certificate_list.len()",
        1usize,
        cert.entries.len(),
    );
    c.observe("certificate.certificate_list.len()", cert.entries.len());
    c.finish()
});

tls_test!(leaf_parses, |ctx| {
    let client = ctx.handshake().await?;
    let cert = client
        .certificate
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no Certificate message"))?;
    let leaf = cert.leaf().map_err(crate::assert::Failure::tls)?;
    let mut c = Check::new("the end-entity certificate");
    c.block("certificate", &client.certificate_bytes);
    match x509_parser::certificate::X509Certificate::from_der(leaf) {
        Ok((rest, parsed)) => {
            c.eq(
                "cert_data: bytes left over after the DER",
                0usize,
                rest.len(),
            );
            c.observe("certificate.subject", parsed.subject().to_string());
            c.observe("certificate.issuer", parsed.issuer().to_string());
            c.that(
                "certificate.validity",
                "a certificate that is currently valid",
                parsed.validity().is_valid(),
                format!(
                    "not_before {} not_after {}",
                    parsed.validity().not_before,
                    parsed.validity().not_after
                ),
            );
        }
        Err(e) => {
            c.that(
                "certificate.certificate_list[0].cert_data",
                "a DER-encoded X.509 certificate",
                false,
                e.to_string(),
            );
        }
    }
    c.finish()
});

tls_test!(leaf_key_matches, |ctx| {
    let client = ctx.handshake().await?;
    let cert = client
        .certificate
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no Certificate message"))?;
    let cv = client
        .certificate_verify
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no CertificateVerify"))?;
    let key = PublicKey::from_certificate(cert.leaf().map_err(crate::assert::Failure::tls)?)
        .map_err(crate::assert::Failure::tls)?;
    let mut c = Check::new("the key in the leaf against the signature");
    c.block("certificate", &client.certificate_bytes);
    c.block("certificate_verify", &client.certificate_verify_bytes);
    c.note(
        "This is the whole point of the Certificate message: the signature in \
         CertificateVerify has to check out against the key in entry 0, and no other entry.",
    );
    c.observe(
        "certificate.certificate_list[0].public_key",
        key.algorithm(),
    );
    c.that(
        "certificate_verify.algorithm",
        "usable with the key in the leaf certificate",
        key.accepts(cv.algorithm),
        crate::tls::sig_name(cv.algorithm),
    );
    c.that(
        "certificate_verify.signature",
        "verifies against the leaf's public key",
        crate::tls::sig::verify(
            &key,
            cv.algorithm,
            &cv.signature,
            &client.hash_after_certificate,
        )
        .is_ok(),
        "does not verify",
    );
    c.finish()
});

tls_test!(per_entry_extensions, |ctx| {
    let client = ctx.handshake().await?;
    let cert = client
        .certificate
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no Certificate message"))?;
    let mut c = Check::new("the per-certificate extensions blocks");
    c.block("certificate", &client.certificate_bytes);
    c.note(
        "Every entry is cert_data *and* an extensions block. Usually empty — the two zero \
         bytes after each certificate — but it is a field, not optional padding.",
    );
    for (i, (_, exts)) in cert.entries.iter().enumerate() {
        c.observe(
            &format!("certificate.certificate_list[{i}].extensions.len()"),
            exts.len(),
        );
        for e in exts {
            c.that(
                &format!("certificate.certificate_list[{i}].extensions"),
                "an extension RFC 8446 section 4.4.2 allows here",
                matches!(e.ext_type, 5 | 18),
                crate::tls::ext_name(e.ext_type),
            );
        }
    }
    c.that(
        "certificate.certificate_list",
        "at least one entry, each with an extensions block",
        !cert.entries.is_empty(),
        cert.entries.len(),
    );
    c.finish()
});

tls_test!(lengths_agree, |ctx| {
    let client = ctx.handshake().await?;
    let bytes = &client.certificate_bytes;
    let cert = client
        .certificate
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no Certificate message"))?;
    let mut c = Check::new("the three nested lengths of a Certificate message");
    c.block("certificate", bytes);
    if bytes.len() >= 8 {
        let body = u32::from_be_bytes([0, bytes[1], bytes[2], bytes[3]]) as usize;
        let context = bytes[4] as usize;
        let list = u32::from_be_bytes([
            0,
            bytes[5 + context],
            bytes[6 + context],
            bytes[7 + context],
        ]) as usize;
        c.mark(1..4);
        c.eq("certificate.length", bytes.len() - 4, body);
        c.eq(
            "certificate.certificate_list.length",
            body - 1 - context - 3,
            list,
        );
        let entry_bytes: usize = cert
            .entries
            .iter()
            .map(|(d, _)| 3 + d.len() + 2 + crate::tls::msg::encode_extensions(&[]).len() - 2)
            .sum::<usize>();
        c.observe("certificate.certificate_list entries", cert.entries.len());
        c.at_least("certificate_list.length", entry_bytes, list);
    } else {
        c.that("certificate", "at least eight bytes", false, bytes.len());
    }
    c.finish()
});

tls_test!(chain_order, |ctx| {
    let client = ctx.handshake().await?;
    let cert = client
        .certificate
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no Certificate message"))?;
    let mut c = Check::new("a certificate chain, leaf first");
    c.block("certificate", &client.certificate_bytes);
    c.note(
        "The server was given a PEM holding leaf, intermediate and root. RFC 8446 section \
         4.4.2 makes the first entry the end-entity certificate and each later one the \
         issuer of the previous; the self-signed root may be omitted.",
    );
    let mut subjects = Vec::new();
    for (i, (der, _)) in cert.entries.iter().enumerate() {
        match x509_parser::certificate::X509Certificate::from_der(der) {
            Ok((_, parsed)) => {
                subjects.push((parsed.subject().to_string(), parsed.issuer().to_string()))
            }
            Err(e) => {
                c.that(
                    &format!("certificate.certificate_list[{i}].cert_data"),
                    "a DER certificate",
                    false,
                    e.to_string(),
                );
            }
        }
    }
    c.observe("certificate.certificate_list subjects", &subjects);
    if let Some((subject, _)) = subjects.first() {
        c.that(
            "certificate.certificate_list[0]",
            "the end-entity certificate, not a CA",
            subject.contains("leaf"),
            subject.clone(),
        );
    }
    for i in 1..subjects.len() {
        c.eq(
            &format!("certificate.certificate_list[{i}].subject"),
            subjects[i - 1].1.clone(),
            subjects[i].0.clone(),
        );
    }
    c.finish()
});

tls_test!(arrives_encrypted, |ctx| {
    let client = ctx.handshake().await?;
    let leaf_prefix = client
        .certificate
        .as_ref()
        .and_then(|c| c.leaf().ok().map(|d| d[..d.len().min(24)].to_vec()))
        .ok_or_else(|| crate::stages::harness("no certificate"))?;
    // Search every raw record the server sent for the certificate's first bytes. If the
    // Certificate had gone out in the clear they would be there verbatim.
    let leaked = client
        .conn
        .records_in
        .iter()
        .any(|r| !crate::assert::find(&r.raw, &leaf_prefix).is_empty());
    let mut c = Check::new("that the certificate was not sent in the clear");
    c.note(
        "TLS 1.3 encrypts everything after the ServerHello. A passive observer sees the \
         hellos and then opaque application_data records — the server's identity included.",
    );
    c.that(
        "the certificate's DER in the raw record bytes",
        "not present anywhere",
        !leaked,
        "the first bytes of the certificate appear verbatim in a record the server sent",
    );
    c.finish()
});

fn config(env: &ExampleEnv) -> ClientConfig {
    env.config()
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::handshake(
            "A Certificate message with one entry",
            config,
            Part::ClientHello,
            Part::Certificate,
        )
        .request("An ordinary ClientHello")
        .response(
            "`0b`, a uint24 length, `00` (the empty context), a uint24 list length, then one \
             entry: a uint24 DER length, the certificate, and `00 00` for its extensions.",
        )
        .note(
            "Three uint24 lengths inside one message. The innermost is the DER; the outer two \
             are TLS framing and have to agree with it exactly.",
        ),
        ExampleSpec::handshake(
            "A three-certificate chain",
            config,
            Part::ClientHello,
            Part::Certificate,
        )
        .with_server(crate::examples::chain_server)
        .request("The same hello, against a server holding leaf, intermediate and root")
        .response(
            "certificate_list with three entries, leaf first, each with its own `00 00` \
             extensions block.",
        )
        .note(
            "Order matters: each entry certifies the one before it. Sending the root first — \
             or sorting the file — produces a chain no client can build a path from.",
        ),
    ]
}
