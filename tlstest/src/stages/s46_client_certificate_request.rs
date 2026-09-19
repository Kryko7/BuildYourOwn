//! Stage 46 — CertificateRequest: the server asks the client to prove who it is.

use crate::assert::Check;
use crate::config::ServerOptions;
use crate::examples::ExampleSpec;
use crate::stages::{Stage, Test};
use crate::tls::msg::CertificateRequestMsg;
use crate::tls::EXT_SIGNATURE_ALGORITHMS;
use crate::tls_test;

fn asking() -> ServerOptions {
    ServerOptions::default().requesting_client_cert()
}

fn requiring() -> ServerOptions {
    ServerOptions::default().requiring_client_cert()
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 46,
        slug: "client_certificate_request",
        name: "CertificateRequest",
        ext: true,
        hints: &[
            "CertificateRequest is handshake type 13 and belongs in the encrypted flight, after \
             EncryptedExtensions and before the server's own Certificate",
            "Its body is certificate_request_context (opaque<0..255>, empty during a handshake) \
             followed by an extensions block — there is no certificate_types or \
             supported_signature_algorithms list any more, those were TLS 1.2",
            "signature_algorithms is mandatory in it: without it the client has no way to know \
             what it may sign with, so an empty extensions block is an illegal message",
            "Sending it at all is a choice; a server that never wants client certificates simply \
             omits the message and nothing else about the handshake changes",
        ],
        examples,
        tests: vec![
            Test::new("the server sends one when it is asking", sends_one)
                .ext()
                .with_server(asking),
            Test::new("it arrives inside the encrypted flight", is_encrypted)
                .ext()
                .with_server(asking),
            Test::new("it comes before the server's own Certificate", ordering)
                .ext()
                .with_server(asking),
            Test::new("the context is empty during a handshake", empty_context)
                .ext()
                .with_server(asking),
            Test::new("it carries signature_algorithms", carries_sig_algs)
                .ext()
                .with_server(asking),
            Test::new(
                "the schemes it offers are ones a TLS 1.3 client can use",
                schemes_are_usable,
            )
            .ext()
            .with_server(asking),
            Test::new(
                "requiring a certificate looks the same on the wire as requesting one",
                require_looks_the_same,
            )
            .ext()
            .with_server(requiring),
            Test::new(
                "a server that was not asked for client auth sends none",
                none_when_not_asked,
            )
            .ext(),
        ],
    }
}

fn request_of(
    client: &crate::tls::client::Client,
) -> Result<CertificateRequestMsg, crate::stages::Failure> {
    client
        .certificate_request_msg
        .clone()
        .ok_or_else(|| crate::stages::harness("the server sent no CertificateRequest"))
}

tls_test!(sends_one, |ctx| {
    let client = ctx.handshake_authenticated().await?;
    let mut c = Check::new("CertificateRequest is sent");
    c.block(
        "certificate_request",
        &client.certificate_request.clone().unwrap_or_default(),
    );
    c.note(
        "A server configured to want client certificates announces it with this message. \
         Everything else in this stage is about its shape.",
    );
    c.that(
        "the server sent a certificate_request",
        "a certificate_request message",
        client.certificate_request.is_some(),
        client.certificate_request.as_ref().map(|r| r.len()),
    );
    c.finish()
});

tls_test!(is_encrypted, |ctx| {
    let client = ctx.handshake_authenticated().await?;
    let raw = client.certificate_request.clone().unwrap_or_default();
    let mut c = Check::new("CertificateRequest is encrypted");
    c.block("certificate_request", &raw);
    c.note(
        "It is part of the server's encrypted flight, so a passive observer cannot even see \
         that client authentication is being asked for. The suite only ever sees it because \
         it holds the handshake keys.",
    );
    c.eq(
        "certificate_request.msg_type",
        13u8,
        raw.first().copied().unwrap_or(0),
    );
    c.that(
        "it was read under the server handshake keys",
        "a non-empty message body",
        !raw.is_empty(),
        raw.len(),
    );
    c.finish()
});

tls_test!(ordering, |ctx| {
    let client = ctx.handshake_authenticated().await?;
    let order: Vec<String> = client
        .transcript
        .as_ref()
        .map(|t| t.checkpoints.iter().map(|(n, _)| n.clone()).collect())
        .unwrap_or_default();
    let mut c = Check::new("CertificateRequest comes before Certificate");
    c.note(
        "RFC 8446 section 4.3.2 fixes the order: EncryptedExtensions, then CertificateRequest \
         if there is one, then the server's Certificate and CertificateVerify.",
    );
    let pos = |name: &str| order.iter().position(|m| m == name);
    c.observe("the encrypted flight, in order", order.join(", "));
    c.that(
        "encrypted_extensions opens the encrypted flight",
        "the message right after server_hello",
        pos("encrypted_extensions") == pos("server_hello").map(|i| i + 1),
        order
            .get(pos("server_hello").map(|i| i + 1).unwrap_or(0))
            .cloned()
            .unwrap_or_default(),
    );
    c.that(
        "certificate_request comes before certificate",
        "a lower index than certificate",
        matches!(
            (pos("certificate_request"), pos("certificate")),
            (Some(a), Some(b)) if a < b
        ),
        (pos("certificate_request"), pos("certificate")),
    );
    c.finish()
});

tls_test!(empty_context, |ctx| {
    let client = ctx.handshake_authenticated().await?;
    let req = request_of(&client)?;
    let mut c = Check::new("certificate_request_context");
    c.block(
        "certificate_request",
        &client.certificate_request.clone().unwrap_or_default(),
    );
    c.mark(4..5);
    c.note(
        "The context exists so that post-handshake authentication can tell one request from \
         another. During the handshake there is only ever one, so it is empty — and the \
         client echoes whatever is here in its own Certificate.",
    );
    c.eq(
        "certificate_request.context.len()",
        0usize,
        req.context.len(),
    );
    c.finish()
});

tls_test!(carries_sig_algs, |ctx| {
    let client = ctx.handshake_authenticated().await?;
    let req = request_of(&client)?;
    let mut c = Check::new("signature_algorithms is present");
    c.note(
        "This is the only extension RFC 8446 makes mandatory in a CertificateRequest. Without \
         it the client cannot know which schemes its CertificateVerify may use, so a request \
         with an empty extensions block is not a lenient request — it is a broken one.",
    );
    c.that(
        "the extensions block carries signature_algorithms (13)",
        "extension 13 present",
        req.extensions
            .iter()
            .any(|e| e.ext_type == EXT_SIGNATURE_ALGORITHMS),
        req.extensions
            .iter()
            .map(|e| e.ext_type)
            .collect::<Vec<_>>(),
    );
    c.finish()
});

tls_test!(schemes_are_usable, |ctx| {
    let client = ctx.handshake_authenticated().await?;
    let req = request_of(&client)?;
    let schemes = req
        .signature_algorithms()
        .map_err(|e| crate::stages::harness(format!("{e}")))?;
    let mut c = Check::new("the offered schemes");
    c.note(
        "A TLS 1.3 client may only sign with the schemes listed here, and the list must hold \
         at least one that TLS 1.3 still allows — the PKCS#1 v1.5 schemes were removed for \
         CertificateVerify, so a list of nothing but those would leave the client unable to \
         answer.",
    );
    c.that(
        "at least one scheme is offered",
        "a non-empty list",
        !schemes.is_empty(),
        schemes.len(),
    );
    let modern = schemes.iter().any(|s| {
        matches!(
            *s,
            0x0403 | 0x0503 | 0x0603 | 0x0804 | 0x0805 | 0x0806 | 0x0807
        )
    });
    c.that(
        "at least one is an ECDSA, RSA-PSS or EdDSA scheme TLS 1.3 permits",
        "one of 0x0403/0x0503/0x0603/0x0804..0x0807",
        modern,
        schemes
            .iter()
            .map(|s| format!("0x{s:04x}"))
            .collect::<Vec<_>>(),
    );
    c.finish()
});

tls_test!(require_looks_the_same, |ctx| {
    let client = ctx.handshake_authenticated().await?;
    let req = request_of(&client)?;
    let mut c = Check::new("requesting against requiring");
    c.note(
        "Whether a server merely asks for a certificate or refuses to continue without one is \
         a local policy decision. Nothing in the CertificateRequest says which it is: the \
         difference shows up only in what the server does when the client declines.",
    );
    c.eq(
        "certificate_request.context.len()",
        0usize,
        req.context.len(),
    );
    c.that(
        "it still carries signature_algorithms",
        "extension 13 present",
        req.extensions
            .iter()
            .any(|e| e.ext_type == EXT_SIGNATURE_ALGORITHMS),
        req.extensions
            .iter()
            .map(|e| e.ext_type)
            .collect::<Vec<_>>(),
    );
    c.finish()
});

tls_test!(none_when_not_asked, |ctx| {
    let client = ctx.handshake().await?;
    let mut c = Check::new("no CertificateRequest by default");
    c.note(
        "Most TLS servers never want a client certificate, and for them the message simply \
         does not exist. A server that sends one unasked would force every client to have a \
         certificate or to decline explicitly.",
    );
    c.that(
        "the handshake carried no certificate_request",
        "no such message",
        client.certificate_request.is_none(),
        client.certificate_request.as_ref().map(|r| r.len()),
    );
    c.finish()
});

fn config(env: &crate::examples::ExampleEnv) -> crate::tls::client::ClientConfig {
    env.config()
}

fn examples() -> Vec<ExampleSpec> {
    vec![ExampleSpec::handshake(
        "A CertificateRequest, field by field",
        config,
        crate::examples::Part::ClientHello,
        crate::examples::Part::CertificateRequest,
    )
    .with_server(asking)
    .request("An ordinary ClientHello, against a server that wants a client certificate")
    .response(
        "`0d`, a uint24 length, `00` for the empty certificate_request_context, then a \
         uint16-counted extensions block holding signature_algorithms(13).",
    )
    .note(
        "Two bytes of context and a list of schemes is the whole message: TLS 1.2's \
         certificate_types and its own copy of the CA list are gone. Note where it sits — \
         after EncryptedExtensions, before the server's Certificate, and encrypted, so \
         nobody watching the connection can tell that client authentication is in play.",
    )]
}
