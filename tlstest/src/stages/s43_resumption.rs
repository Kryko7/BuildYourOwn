//! Stage 43 — NewSessionTicket and a resumed handshake.

use crate::assert::{Check, Failure};
use crate::examples::{ExampleEnv, ExampleSpec, Part};
use crate::stages::{check_refused_with, reversed, Stage, Test};
use crate::tls::client::{Client, ClientConfig, PskOffer};
use crate::tls::msg::NewSessionTicket;
use crate::tls::{hex, AlertDescription};
use crate::tls_test;
use std::time::Duration;

/// How long a test waits for the tickets a server sends after the handshake.
const TICKET_WAIT: Duration = Duration::from_millis(1_000);

/// Complete a handshake, collect a ticket, and turn it into a PSK offer.
async fn get_ticket(
    ctx: &crate::stages::Ctx,
    n: u64,
) -> Result<(PskOffer, NewSessionTicket, usize), Failure> {
    let mut client = ctx.handshake_with(ctx.config_n(n)).await?;
    client
        .echo_line("first")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    client.collect_tickets(TICKET_WAIT).await.ok();
    let ticket = client
        .tickets
        .first()
        .cloned()
        .ok_or_else(|| crate::stages::harness("the server sent no NewSessionTicket"))?;
    let offer = client.psk_from_ticket(&ticket).map_err(Failure::tls)?;
    let count = client.tickets.len();
    client.close().await.ok();
    Ok((offer, ticket, count))
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 43,
        slug: "resumption",
        name: "NewSessionTicket and resumption",
        ext: true,
        hints: &[
            "A ticket's PSK is HKDF-Expand-Label(resumption_master_secret, \"resumption\", \
             ticket_nonce, Hash.length) — the nonce is what makes two tickets from one \
             connection different",
            "pre_shared_key must be the *last* extension of the ClientHello, because the \
             binder is computed over everything before it",
            "binder = HMAC(finished_key(binder_key), Transcript-Hash(truncated ClientHello)), \
             where the truncation stops just before the binder list's own length",
            "A resumed handshake sends no Certificate and no CertificateVerify: the PSK is \
             the authentication",
        ],
        examples,
        tests: vec![
            Test::new("the server sends a NewSessionTicket", sends_a_ticket).min_timeout_ms(20_000),
            Test::new("the ticket's fields are well-formed", ticket_fields).min_timeout_ms(20_000),
            Test::new(
                "two tickets from one connection have different nonces",
                nonces_differ,
            )
            .min_timeout_ms(20_000),
            Test::new("a ticket resumes the session", resumes).min_timeout_ms(20_000),
            Test::new("a resumed handshake sends no Certificate", no_certificate)
                .min_timeout_ms(20_000),
            Test::new("data flows on a resumed connection", data_flows).min_timeout_ms(20_000),
            Test::new("a wrong binder does not resume", wrong_binder).min_timeout_ms(20_000),
            Test::new(
                "an unknown ticket falls back to a full handshake",
                unknown_ticket,
            )
            .min_timeout_ms(20_000),
        ],
    }
}

tls_test!(sends_a_ticket, |ctx| {
    let (_, ticket, count) = get_ticket(ctx, 1).await?;
    let mut c = Check::new("the tickets a server sends after the handshake");
    c.note(
        "Tickets are optional — a server that sends none simply cannot be resumed with — but \
         they are how every real deployment avoids a second signature.",
    );
    c.at_least("tickets received", 1usize, count);
    c.observe("new_session_ticket.ticket.len()", ticket.ticket.len());
    c.observe("new_session_ticket.ticket_lifetime", ticket.ticket_lifetime);
    c.finish()
});

tls_test!(ticket_fields, |ctx| {
    let (_, ticket, _) = get_ticket(ctx, 2).await?;
    let mut c = Check::new("the fields of a NewSessionTicket");
    c.note(
        "ticket_lifetime is capped at seven days by RFC 8446 section 4.6.1; ticket_age_add is \
         random per ticket so a passive observer cannot correlate resumptions.",
    );
    c.at_least(
        "new_session_ticket.ticket_lifetime",
        1u32,
        ticket.ticket_lifetime,
    );
    c.at_most(
        "new_session_ticket.ticket_lifetime",
        604_800u32,
        ticket.ticket_lifetime,
    );
    c.that(
        "new_session_ticket.ticket",
        "non-empty",
        !ticket.ticket.is_empty(),
        ticket.ticket.len(),
    );
    c.at_most(
        "new_session_ticket.ticket_nonce.len()",
        255usize,
        ticket.ticket_nonce.len(),
    );
    c.observe("new_session_ticket.ticket_nonce", hex(&ticket.ticket_nonce));
    c.observe(
        "new_session_ticket.max_early_data_size",
        ticket.max_early_data(),
    );
    c.finish()
});

tls_test!(nonces_differ, |ctx| {
    let mut client = ctx.handshake().await?;
    client
        .echo_line("tickets")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    client.collect_tickets(TICKET_WAIT).await.ok();
    let mut c = Check::new("the nonces of several tickets from one connection");
    c.note(
        "One resumption_master_secret, several tickets: the ticket_nonce is the only thing \
         that makes their PSKs different.",
    );
    c.observe("tickets received", client.tickets.len());
    if client.tickets.len() >= 2 {
        c.ne(
            "the second ticket's nonce",
            hex(&client.tickets[0].ticket_nonce),
            hex(&client.tickets[1].ticket_nonce),
        );
        let first = client
            .psk_from_ticket(&client.tickets[0])
            .map_err(Failure::tls)?;
        let second = client
            .psk_from_ticket(&client.tickets[1])
            .map_err(Failure::tls)?;
        c.ne("the second ticket's PSK", hex(&first.psk), hex(&second.psk));
    } else {
        c.note(
            "the server sent only one ticket, which is conformant; there is nothing to \
             compare",
        );
        c.at_least("tickets received", 1usize, client.tickets.len());
    }
    c.finish()
});

tls_test!(resumes, |ctx| {
    let (offer, _, _) = get_ticket(ctx, 3).await?;
    let client = ctx.handshake_with(ctx.config_n(4).with_psk(offer)).await?;
    let selected = client
        .server_hello
        .as_ref()
        .and_then(|h| h.selected_psk().ok())
        .flatten();
    let mut c = Check::new("a handshake offering a resumption PSK");
    c.block("client_hello", &client.client_hello_bytes);
    c.block("server_hello", &client.server_hello_bytes);
    c.note(
        "selected_identity is the index into the client's identity list. With one identity \
         offered, accepting it means 0.",
    );
    c.eq(
        "server_hello.pre_shared_key.selected_identity",
        Some(0u16),
        selected,
    );
    c.finish()
});

tls_test!(no_certificate, |ctx| {
    let (offer, _, _) = get_ticket(ctx, 5).await?;
    let client = ctx.handshake_with(ctx.config_n(6).with_psk(offer)).await?;
    let mut c = Check::new("what a resumed handshake leaves out");
    c.note_all(client.transcript_lines());
    c.note(
        "The PSK authenticates the server, so there is nothing to sign and nothing to send: \
         EncryptedExtensions is followed straight by Finished.",
    );
    c.that(
        "certificate",
        "absent on a resumed handshake",
        client.certificate.is_none(),
        "the server sent a Certificate anyway",
    );
    c.that(
        "certificate_verify",
        "absent on a resumed handshake",
        client.certificate_verify.is_none(),
        "the server sent a CertificateVerify anyway",
    );
    c.that(
        "server finished",
        "verified over the transcript after EncryptedExtensions",
        !client.server_verify_data.is_empty(),
        "the handshake did not complete",
    );
    c.finish()
});

tls_test!(data_flows, |ctx| {
    let (offer, _, _) = get_ticket(ctx, 7).await?;
    let mut client = ctx.handshake_with(ctx.config_n(8).with_psk(offer)).await?;
    let answer = client
        .echo_line("resumed")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("the data path on a resumed connection");
    c.eq("echo", reversed("resumed"), answer);
    c.finish()
});

tls_test!(wrong_binder, |ctx| {
    let (offer, _, _) = get_ticket(ctx, 9).await?;
    let conn = ctx.connect().await?;
    let mut client = Client::over(conn, ctx.config_n(10).with_broken_psk(offer));
    client.send_client_hello().await.map_err(Failure::tls)?;
    client.maybe_send_ccs().await.map_err(Failure::tls)?;
    let reaction = crate::stages::reaction_past_tickets(&mut client.conn).await;
    let mut c = Check::new("a resumption offer whose binder is wrong");
    c.block("client_hello", &client.client_hello_bytes);
    c.note(
        "RFC 8446 section 4.2.11.2: 'If this value is not present or does not validate, the \
         server MUST abort the handshake'. Quietly falling back to a full handshake would \
         let an attacker strip the PSK.",
    );
    match &reaction {
        crate::tls::conn::Reaction::Message(m) if m.contains("server_hello") => {
            // Some servers answer the ServerHello first and abort a moment later; look once
            // more before calling it accepted.
            let next = crate::stages::reaction_past_tickets(&mut client.conn).await;
            check_refused_with(
                &mut c,
                "the server's reaction",
                &next,
                &[
                    AlertDescription::DECRYPT_ERROR,
                    AlertDescription::ILLEGAL_PARAMETER,
                    AlertDescription::HANDSHAKE_FAILURE,
                    AlertDescription::DECODE_ERROR,
                    AlertDescription::BAD_RECORD_MAC,
                ],
            );
        }
        other => {
            check_refused_with(
                &mut c,
                "the server's reaction",
                other,
                &[
                    AlertDescription::DECRYPT_ERROR,
                    AlertDescription::ILLEGAL_PARAMETER,
                    AlertDescription::HANDSHAKE_FAILURE,
                    AlertDescription::DECODE_ERROR,
                    AlertDescription::BAD_RECORD_MAC,
                ],
            );
        }
    }
    c.finish()?;
    drop(client);
    ctx.expect_still_serving("a resumption offer with a wrong binder")
        .await
});

tls_test!(unknown_ticket, |ctx| {
    let (mut offer, _, _) = get_ticket(ctx, 11).await?;
    // A ticket the server has never issued: the binder over it is perfectly well formed,
    // but there is nothing to decrypt it with.
    offer.identity = vec![0xab; offer.identity.len().max(32)];
    let client = ctx.handshake_with(ctx.config_n(12).with_psk(offer)).await?;
    let selected = client
        .server_hello
        .as_ref()
        .and_then(|h| h.selected_psk().ok())
        .flatten();
    let mut c = Check::new("an offer carrying a ticket the server never issued");
    c.note(
        "A server that cannot decrypt a ticket must simply not select it and carry on with a \
         full handshake. Aborting would make every expired ticket a connection failure.",
    );
    c.eq(
        "server_hello.pre_shared_key.selected_identity",
        None::<u16>,
        selected,
    );
    c.that(
        "certificate",
        "present, because this became a full handshake",
        client.certificate.is_some(),
        "no Certificate arrived",
    );
    c.finish()
});

fn config(env: &ExampleEnv) -> ClientConfig {
    env.config()
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::handshake(
            "A NewSessionTicket",
            config,
            Part::ClientFinished,
            Part::NewSessionTicket,
        )
        .request("The client's Finished, which is what the resumption secret is taken over")
        .response(
            "`04`, a uint24 length, ticket_lifetime, ticket_age_add, a ticket_nonce, the \
             opaque ticket itself, and an extensions block.",
        )
        .note(
            "The ticket is opaque to the client: it is whatever the server needs to recover \
             the PSK, usually its own state sealed under a key only it holds.",
        ),
        ExampleSpec::text("The binder, step by step")
            .request(
                "binder_key   = Derive-Secret(Early Secret, \"res binder\", \"\")\n\
                 finished_key = HKDF-Expand-Label(binder_key, \"finished\", \"\", Hash.length)\n\
                 binder       = HMAC(finished_key, Transcript-Hash(Truncate(ClientHello)))",
            )
            .response(
                "Truncate() stops just before the binder list's two-byte length — everything \
                 from `01` to the end of the identities is hashed, and nothing after.",
            )
            .note(
                "The Early Secret here is HKDF-Extract(0, PSK), not the zero-PSK one: the \
                 binder proves the client knows the PSK before the server has spent anything \
                 on it.",
            ),
    ]
}
