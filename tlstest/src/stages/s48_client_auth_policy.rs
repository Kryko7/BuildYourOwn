//! Stage 48 — Declining, and the difference between requesting and requiring.
//!
//! A CertificateRequest is a question, and "no" is a legal answer: the client sends a
//! Certificate whose list is **empty** rather than staying silent. What happens next is the
//! server's policy, and it is the only place the two modes differ — on the wire, a server
//! that merely asks and a server that insists send exactly the same CertificateRequest.
//!
//! The alert for insisting is a specific one. `certificate_required(116)` exists so that a
//! client can tell "you must authenticate" apart from "your certificate was bad", and a
//! server that answers with `handshake_failure` has told the client to give up when it
//! should have told it to try again with a certificate.

use crate::assert::Check;
use crate::config::ServerOptions;
use crate::examples::ExampleSpec;
use crate::stages::{check_refused_with, reaction_past_tickets, Stage, Test};
use crate::tls::conn::Reaction;
use crate::tls::msg::CertificateMsg;
use crate::tls::AlertDescription;
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
        number: 48,
        slug: "client_auth_policy",
        name: "Declining, and requesting against requiring",
        ext: true,
        hints: &[
            "Declining is an empty Certificate, not silence: the client still sends the message, \
             with a zero-length certificate_list, and no CertificateVerify after it",
            "A server that only *requests* must carry on with an unauthenticated connection — \
             the handshake completes and application data flows",
            "A server that *requires* must fail the handshake, and certificate_required(116) is \
             the alert that says why; handshake_failure loses that information",
            "An empty Certificate is followed by Finished and nothing else — a CertificateVerify \
             with no certificate to verify is an illegal message",
        ],
        examples,
        tests: vec![
            Test::new(
                "a client that declines sends an empty Certificate, not silence",
                declines_with_empty_list,
            )
            .ext()
            .with_server(asking),
            Test::new(
                "a server that only requests carries on without one",
                request_carries_on,
            )
            .ext()
            .with_server(asking),
            Test::new("and its data path works unauthenticated", request_data_path)
                .ext()
                .with_server(asking),
            Test::new(
                "a server that requires one refuses the empty list",
                require_refuses_empty,
            )
            .ext()
            .with_server(requiring),
            Test::new(
                "and says certificate_required rather than something vaguer",
                require_says_why,
            )
            .ext()
            .with_server(requiring),
            Test::new(
                "authenticating against the same server succeeds",
                requiring_accepts_a_certificate,
            )
            .ext()
            .with_server(requiring),
        ],
    }
}

tls_test!(declines_with_empty_list, |ctx| {
    let client = ctx.handshake_declining().await?;
    let body = &client.client_certificate_bytes[4..];
    let sent = CertificateMsg::parse(body).map_err(crate::stages::Failure::tls)?;
    let mut c = Check::new("the client's empty Certificate");
    c.block("client certificate", &client.client_certificate_bytes);
    c.note(
        "Saying nothing would leave the server waiting for a message that never comes. The \
         answer to \"show me a certificate\" is a Certificate message with an empty list, and \
         the handshake carries on from there.",
    );
    c.eq(
        "certificate.certificate_list.len()",
        0usize,
        sent.entries.len(),
    );
    c.that(
        "no CertificateVerify followed it",
        "nothing to verify, so nothing sent",
        client.client_certificate_verify_bytes.is_empty(),
        client.client_certificate_verify_bytes.len(),
    );
    c.finish()
});

tls_test!(request_carries_on, |ctx| {
    let mut client = ctx.handshake_declining().await?;
    let echoed = client
        .echo_line("anonymous")
        .await
        .map_err(crate::stages::Failure::tls)?;
    let mut c = Check::new("a requested certificate that was not supplied");
    c.note(
        "This is the common case on the public web the other way round — the server asks in \
         case the client has one, and gets on with it when it does not. Refusing here would \
         make the request compulsory, which is the other mode.",
    );
    c.eq("the echo", "suomynona", echoed.as_str());
    c.finish()
});

tls_test!(request_data_path, |ctx| {
    let mut client = ctx.handshake_declining().await?;
    let mut seen = Vec::new();
    for line in ["one", "two", "three"] {
        seen.push(
            client
                .echo_line(line)
                .await
                .map_err(crate::stages::Failure::tls)?,
        );
    }
    let mut c = Check::new("the unauthenticated connection keeps working");
    c.note(
        "Nothing about the connection is second-class: the same keys, the same record layer, \
         the same everything. The server simply knows less about who is at the other end.",
    );
    c.eq(
        "three echoes",
        vec!["eno".to_string(), "owt".to_string(), "eerht".to_string()],
        seen,
    );
    c.finish()
});

/// Decline, then report what the server did about it.
async fn decline_and_watch(
    ctx: &mut crate::stages::Ctx,
) -> Result<Reaction, crate::stages::Failure> {
    let mut client = ctx.client().await?;
    if let Err(e) = client.handshake_through_server_flight().await {
        return Err(crate::stages::handshake_failure(e, &client));
    }
    if let Err(e) = client.send_empty_client_certificate().await {
        return Err(crate::stages::handshake_failure(e, &client));
    }
    let _ = client.send_client_finished().await;
    Ok(reaction_past_tickets(&mut client.conn).await)
}

tls_test!(require_refuses_empty, |ctx| {
    let reaction = decline_and_watch(ctx).await?;
    let mut c = Check::new("what a requiring server does with an empty Certificate");
    c.note(
        "Requiring a certificate has to mean refusing the connection without one. A server \
         that prints a complaint and carries on has required nothing — which is exactly what \
         `openssl s_server -Verify` does until it is also given -verify_return_error.",
    );
    check_refused_with(
        &mut c,
        "the server's reaction",
        &reaction,
        &[
            AlertDescription::CERTIFICATE_REQUIRED,
            AlertDescription::HANDSHAKE_FAILURE,
            AlertDescription::BAD_CERTIFICATE,
            AlertDescription::UNEXPECTED_MESSAGE,
        ],
    );
    c.finish()
});

tls_test!(require_says_why, |ctx| {
    let reaction = decline_and_watch(ctx).await?;
    let mut c = Check::new("the alert a requiring server sends");
    c.note(
        "certificate_required(116) was added in TLS 1.3 for exactly this moment. It tells the \
         client the connection failed because it did not authenticate, which is something it \
         can act on — unlike handshake_failure, which could mean anything.",
    );
    c.that(
        "the alert names the reason",
        "certificate_required(116)",
        matches!(&reaction, Reaction::Alert(a)
            if a.description == AlertDescription::CERTIFICATE_REQUIRED),
        reaction.describe(),
    );
    c.finish()
});

tls_test!(requiring_accepts_a_certificate, |ctx| {
    let mut client = ctx.handshake_authenticated().await?;
    let echoed = client
        .echo_line("identified")
        .await
        .map_err(crate::stages::Failure::tls)?;
    let mut c = Check::new("the same server, given what it asked for");
    c.note(
        "The pair of tests is the point: the same server, the same request, and the outcome \
         decided entirely by whether the client answered it.",
    );
    c.eq("the echo", "deifitnedi", echoed.as_str());
    c.finish()
});

fn config(env: &crate::examples::ExampleEnv) -> crate::tls::client::ClientConfig {
    env.config()
}

fn examples() -> Vec<ExampleSpec> {
    vec![ExampleSpec::handshake(
        "What a server asks for when it will insist",
        config,
        crate::examples::Part::ClientHello,
        crate::examples::Part::CertificateRequest,
    )
    .with_server(requiring)
    .request("A hello against a server that requires a client certificate")
    .response("The same CertificateRequest a merely-asking server sends.")
    .note(
        "Worth comparing with stage 46's example byte for byte: they are identical. Nothing \
         in the request says whether the server will insist, so a client cannot tell in \
         advance whether declining will cost it the connection.",
    )]
}
