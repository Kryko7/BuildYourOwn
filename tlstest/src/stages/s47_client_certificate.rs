//! Stage 47 — The client's Certificate and CertificateVerify.
//!
//! Client authentication is the handshake's mirror image. The client sends the same two
//! messages the server sent, under its own handshake keys, in the gap between the server's
//! Finished and its own — and proves it holds the key by signing the transcript so far.
//!
//! Two details carry most of the weight. The signature covers a **different context string**
//! from the server's ("TLS 1.3, client CertificateVerify"), which is what stops a signature
//! made in one role being replayed in the other. And the client's Certificate is itself in
//! the transcript, so the Finished that follows is over a longer history than an
//! unauthenticated handshake's — authenticate, and both sides' verify_data change.

use crate::assert::Check;
use crate::config::ServerOptions;
use crate::examples::ExampleSpec;
use crate::stages::{check_refused_with, reaction_past_tickets, Stage, Test};
use crate::tls::msg::{encode_handshake, CertificateMsg, CertificateVerifyMsg};
use crate::tls::{sig, AlertDescription, HandshakeType};
use crate::tls_test;

fn requiring() -> ServerOptions {
    ServerOptions::default().requiring_client_cert()
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 47,
        slug: "client_certificate",
        name: "The client's Certificate and CertificateVerify",
        ext: true,
        hints: &[
            "Both messages go out under the *client* handshake traffic keys, after the server's \
             Finished and before the client's own",
            "The client's Certificate echoes the certificate_request_context byte for byte — \
             empty during a handshake, but echoing is the rule",
            "CertificateVerify signs 64 spaces, then \"TLS 1.3, client CertificateVerify\", then \
             a zero byte, then the transcript hash up to and including the client's Certificate \
             — a different context string from the server's, on purpose",
            "The client's Certificate and CertificateVerify are in the transcript, so the \
             Finished that follows covers them and its verify_data differs from an \
             unauthenticated handshake's",
        ],
        examples,
        tests: vec![
            Test::new("a client certificate completes the handshake", completes)
                .ext()
                .with_server(requiring),
            Test::new("and the data path still works afterwards", data_after_auth)
                .ext()
                .with_server(requiring),
            Test::new(
                "the client's Certificate echoes the request context",
                echoes_context,
            )
            .ext()
            .with_server(requiring),
            Test::new(
                "the CertificateVerify uses a scheme the server offered",
                scheme_was_offered,
            )
            .ext()
            .with_server(requiring),
            Test::new(
                "the client's messages are in the transcript before its Finished",
                in_the_transcript,
            )
            .ext()
            .with_server(requiring),
            Test::new(
                "a signature over the server's context string is refused",
                wrong_context_refused,
            )
            .ext()
            .with_server(requiring),
            Test::new(
                "a signature over the wrong transcript is refused",
                wrong_transcript_refused,
            )
            .ext()
            .with_server(requiring),
            Test::new(
                "a certificate the server's CA never signed is refused",
                untrusted_certificate_refused,
            )
            .ext()
            .with_server(requiring),
        ],
    }
}

tls_test!(completes, |ctx| {
    let mut client = ctx.handshake_authenticated().await?;
    // Sending the flight proves nothing on its own — the server's objection, if it has one,
    // arrives afterwards. Reading an echo back is what shows the certificate was accepted.
    let echoed = client
        .echo_line("hello")
        .await
        .map_err(crate::stages::Failure::tls)?;
    let mut c = Check::new("an authenticated handshake");
    c.block("client certificate", &client.client_certificate_bytes);
    c.note(
        "The server required a certificate, the client sent one and proved it holds the key, \
         and the handshake finished. Everything else in this stage takes that apart.",
    );
    c.that(
        "the client sent a Certificate",
        "a non-empty message",
        !client.client_certificate_bytes.is_empty(),
        client.client_certificate_bytes.len(),
    );
    c.that(
        "the client sent a CertificateVerify",
        "a non-empty message",
        !client.client_certificate_verify_bytes.is_empty(),
        client.client_certificate_verify_bytes.len(),
    );
    c.eq(
        "the server answered rather than objecting",
        "olleh",
        echoed.as_str(),
    );
    c.finish()
});

tls_test!(data_after_auth, |ctx| {
    let mut client = ctx.handshake_authenticated().await?;
    let echoed = client
        .echo_line("authenticated")
        .await
        .map_err(crate::stages::Failure::tls)?;
    let expected: String = "authenticated".chars().rev().collect();
    let mut c = Check::new("application data after client authentication");
    c.note(
        "Client authentication changes the handshake, not the record layer: once both \
         Finished messages are in, the application keys are derived the same way and the \
         -rev echo works exactly as it does without a client certificate.",
    );
    c.eq("the echo", expected.as_str(), echoed.as_str());
    c.finish()
});

tls_test!(echoes_context, |ctx| {
    let client = ctx.handshake_authenticated().await?;
    let request = client
        .certificate_request_msg
        .clone()
        .ok_or_else(|| crate::stages::harness("no CertificateRequest"))?;
    let body = &client.client_certificate_bytes[4..];
    let sent = CertificateMsg::parse(body).map_err(crate::stages::Failure::tls)?;
    let mut c = Check::new("Certificate.certificate_request_context");
    c.block("client certificate", &client.client_certificate_bytes);
    c.mark(4..5);
    c.note(
        "The client copies the context out of the request rather than sending an empty one. \
         During a handshake the two are the same thing, because the request's context is \
         empty — but post-handshake authentication sends a non-empty context, and a client \
         that hard-codes zero cannot answer it.",
    );
    c.bytes_eq(
        "certificate.certificate_request_context",
        &request.context,
        &sent.request_context,
    );
    c.finish()
});

tls_test!(scheme_was_offered, |ctx| {
    let client = ctx.handshake_authenticated().await?;
    let request = client
        .certificate_request_msg
        .clone()
        .ok_or_else(|| crate::stages::harness("no CertificateRequest"))?;
    let offered = request
        .signature_algorithms()
        .map_err(crate::stages::Failure::tls)?;
    let body = &client.client_certificate_verify_bytes[4..];
    let cv = CertificateVerifyMsg::parse(body).map_err(crate::stages::Failure::tls)?;
    let mut c = Check::new("CertificateVerify.algorithm");
    c.block(
        "client certificate_verify",
        &client.client_certificate_verify_bytes,
    );
    c.mark(4..6);
    c.note(
        "A client may only sign with a scheme the server listed in its CertificateRequest. \
         Choosing one outside the list is not a weaker handshake, it is an illegal one.",
    );
    c.that(
        "the scheme the client signed with was offered",
        "a scheme from the request's signature_algorithms",
        offered.contains(&cv.algorithm),
        format!("0x{:04x}", cv.algorithm),
    );
    c.finish()
});

tls_test!(in_the_transcript, |ctx| {
    let client = ctx.handshake_authenticated().await?;
    let order: Vec<String> = client
        .transcript
        .as_ref()
        .map(|t| t.checkpoints.iter().map(|(n, _)| n.clone()).collect())
        .unwrap_or_default();
    let pos = |name: &str| order.iter().position(|m| m == name);
    let mut c = Check::new("the client's flight in the transcript");
    c.observe("the transcript, in order", order.join(", "));
    c.note(
        "This is why authenticating changes both sides' Finished: the client's Certificate \
         and CertificateVerify are hashed into the transcript before verify_data is computed. \
         A server that verifies the client's signature but forgets to hash these two messages \
         will compute a Finished that does not match.",
    );
    c.that(
        "the client's Certificate is in the transcript",
        "present",
        pos("client certificate").is_some(),
        pos("client certificate"),
    );
    c.that(
        "its CertificateVerify follows it",
        "a higher index",
        matches!(
            (pos("client certificate"), pos("client certificate_verify")),
            (Some(a), Some(b)) if b == a + 1
        ),
        (pos("client certificate"), pos("client certificate_verify")),
    );
    c.that(
        "and the client's Finished comes after both",
        "the last of the three",
        matches!(
            (pos("client certificate_verify"), pos("client finished")),
            (Some(a), Some(b)) if b > a
        ),
        (pos("client certificate_verify"), pos("client finished")),
    );
    c.finish()
});

/// Send a client Certificate, then a CertificateVerify the caller built, then Finished,
/// and report what the server did about it.
async fn bad_certificate_verify(
    ctx: &mut crate::stages::Ctx,
    what: &str,
    sign_over: fn(&crate::tls::client::Client) -> Vec<u8>,
) -> Result<(), crate::stages::Failure> {
    let auth = ctx.certs.client_auth().map_err(|e| {
        crate::stages::harness(format!("cannot mint the client certificate: {e:#}"))
    })?;
    let mut client = ctx.client().await?;
    if let Err(e) = client.handshake_through_server_flight().await {
        return Err(crate::stages::handshake_failure(e, &client));
    }
    let request = client
        .certificate_request_msg
        .clone()
        .ok_or_else(|| crate::stages::harness("no CertificateRequest"))?;
    let cert = CertificateMsg::one(&request.context, &auth.client_der);
    if let Err(e) = client
        .send_client_certificate_bytes(&encode_handshake(
            HandshakeType::CERTIFICATE,
            &cert.encode_body(),
        ))
        .await
    {
        return Err(crate::stages::handshake_failure(e, &client));
    }
    let content = sign_over(&client);
    let signature = sig::sign_pem(&auth.key_pkcs8_pem, auth.scheme, &content)
        .map_err(crate::stages::Failure::tls)?;
    let cv = CertificateVerifyMsg {
        algorithm: auth.scheme,
        signature,
    };
    let message = encode_handshake(HandshakeType::CERTIFICATE_VERIFY, &cv.encode_body());
    if let Err(e) = client.send_client_certificate_verify_bytes(&message).await {
        return Err(crate::stages::handshake_failure(e, &client));
    }
    let _ = client.send_client_finished().await;
    let reaction = reaction_past_tickets(&mut client.conn).await;
    let mut c = Check::new(format!("what the server does with {what}"));
    c.block("the CertificateVerify that was sent", &message);
    c.note(
        "A client that cannot prove it holds the key in its certificate has not authenticated, \
         and a server that accepts it has authenticated nobody at all — anyone could replay a \
         certificate they read off the wire.",
    );
    check_refused_with(
        &mut c,
        "the server's reaction",
        &reaction,
        &[
            AlertDescription::DECRYPT_ERROR,
            AlertDescription::BAD_CERTIFICATE,
            AlertDescription::HANDSHAKE_FAILURE,
            AlertDescription::ILLEGAL_PARAMETER,
            AlertDescription::DECODE_ERROR,
        ],
    );
    c.finish()
}

tls_test!(wrong_context_refused, |ctx| {
    bad_certificate_verify(
        ctx,
        "a signature made with the server's context string",
        |client| {
            let hash = client
                .transcript
                .as_ref()
                .map(|t| t.current())
                .unwrap_or_default();
            sig::server_signed_content(&hash)
        },
    )
    .await
});

tls_test!(wrong_transcript_refused, |ctx| {
    bad_certificate_verify(
        ctx,
        "a signature over the transcript as it stood before the client's Certificate",
        |client| sig::client_signed_content(&client.hash_after_server_finished),
    )
    .await
});

tls_test!(untrusted_certificate_refused, |ctx| {
    // A certificate the server has never heard of, with a CertificateVerify that is
    // genuinely correct for it: the proof of possession is sound, the identity is not.
    // Separating the two is the point — a server that only checks the signature has
    // authenticated whoever turns up.
    let other = ctx
        .certs
        .get(crate::certs::CertKind::Leaf(
            crate::certs::KeyKind::EcdsaP256,
        ))
        .map_err(|e| crate::stages::harness(format!("cannot mint a certificate: {e:#}")))?;
    let key_pem = std::fs::read_to_string(&other.key_pem)
        .map_err(|e| crate::stages::harness(format!("cannot read the stray key: {e}")))?;
    let mut client = ctx.client().await?;
    if let Err(e) = client.handshake_through_server_flight().await {
        return Err(crate::stages::handshake_failure(e, &client));
    }
    let request = client
        .certificate_request_msg
        .clone()
        .ok_or_else(|| crate::stages::harness("no CertificateRequest"))?;
    let cert = CertificateMsg::one(&request.context, other.leaf_der());
    if let Err(e) = client
        .send_client_certificate_bytes(&encode_handshake(
            HandshakeType::CERTIFICATE,
            &cert.encode_body(),
        ))
        .await
    {
        return Err(crate::stages::handshake_failure(e, &client));
    }
    let hash = client
        .transcript
        .as_ref()
        .map(|t| t.current())
        .unwrap_or_default();
    let scheme = crate::certs::KeyKind::EcdsaP256.signature_scheme();
    let signature = sig::sign_pem(&key_pem, scheme, &sig::client_signed_content(&hash))
        .map_err(crate::stages::Failure::tls)?;
    let cv = CertificateVerifyMsg {
        algorithm: scheme,
        signature,
    };
    if let Err(e) = client
        .send_client_certificate_verify_bytes(&encode_handshake(
            HandshakeType::CERTIFICATE_VERIFY,
            &cv.encode_body(),
        ))
        .await
    {
        return Err(crate::stages::handshake_failure(e, &client));
    }
    let _ = client.send_client_finished().await;
    let reaction = reaction_past_tickets(&mut client.conn).await;
    let mut c = Check::new("what the server does with a certificate from an unknown CA");
    c.note(
        "The signature over this certificate is correct, so a server that stops at \"does \
         the CertificateVerify check out\" lets it through. Requiring a client certificate \
         means requiring one that chains to a CA the server trusts, and the alert says which \
         part failed: unknown_ca, not decrypt_error.",
    );
    check_refused_with(
        &mut c,
        "the server's reaction",
        &reaction,
        &[
            AlertDescription::UNKNOWN_CA,
            AlertDescription::BAD_CERTIFICATE,
            AlertDescription::CERTIFICATE_UNKNOWN,
            AlertDescription::HANDSHAKE_FAILURE,
            AlertDescription::CERTIFICATE_REQUIRED,
        ],
    );
    c.finish()
});

fn config(env: &crate::examples::ExampleEnv) -> crate::tls::client::ClientConfig {
    env.config()
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::handshake(
            "The client's own Certificate",
            config,
            crate::examples::Part::CertificateRequest,
            crate::examples::Part::ClientCertificate,
        )
        .with_server(requiring)
        .request("The request the client is answering")
        .response(
            "`0b`, a uint24 length, the echoed context (`00`), a uint24 list length, then one \
             entry: the DER and its own `00 00` extensions block.",
        )
        .note(
            "The same message shape the server sent, in the other direction. The context byte \
             is copied from the request rather than assumed empty.",
        ),
        ExampleSpec::handshake(
            "The client's CertificateVerify",
            config,
            crate::examples::Part::ClientCertificate,
            crate::examples::Part::ClientCertificateVerify,
        )
        .with_server(requiring)
        .request("The certificate whose key is about to be proven")
        .response("`0f`, a uint24 length, the SignatureScheme, and a uint16-length signature.")
        .note(
            "The signature is over 64 spaces, then \"TLS 1.3, client CertificateVerify\", then \
             a zero byte, then the transcript hash taken *after* the Certificate above. \
             Getting the context string or the hash boundary wrong produces a signature that \
             is perfectly valid over the wrong thing, which is why the server's rejection is \
             decrypt_error rather than a decode error.",
        ),
    ]
}
