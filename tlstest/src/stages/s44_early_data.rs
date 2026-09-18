//! Stage 44 — Early data offered, and correctly refused.

use crate::assert::{Check, Failure};
use crate::examples::{ExampleEnv, ExampleSpec, Part};
use crate::stages::{reversed, Stage, Test};
use crate::tls::client::{ClientConfig, PskOffer};
use crate::tls::msg::find_extension;
use crate::tls::{ext_name, EXT_EARLY_DATA};
use crate::tls_test;
use std::time::Duration;

/// How long a test waits for the tickets a server sends after the handshake.
const TICKET_WAIT: Duration = Duration::from_millis(1_000);

/// Complete a handshake and turn its first ticket into a PSK offer.
async fn get_ticket(ctx: &crate::stages::Ctx, n: u64) -> Result<(PskOffer, Option<u32>), Failure> {
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
    let max_early = ticket.max_early_data();
    let offer = client.psk_from_ticket(&ticket).map_err(Failure::tls)?;
    client.close().await.ok();
    Ok((offer, max_early))
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 44,
        slug: "early_data",
        name: "Early data, offered and refused",
        ext: true,
        hints: &[
            "Early data is only on offer when the *ticket* said so: a NewSessionTicket with \
             no early_data extension means 0-RTT is not available on it",
            "A client may send early_data in its hello anyway; a server that does not want it \
             simply leaves early_data out of EncryptedExtensions",
            "When early data is refused there is no EndOfEarlyData message — that only exists \
             on a connection where the server accepted it",
            "Refusing is always safe and often right: 0-RTT data is replayable, and RFC 8446 \
             appendix E.5 spends several pages on why",
        ],
        examples,
        tests: vec![
            Test::new(
                "a ticket without early_data does not permit 0-RTT",
                ticket_forbids_early_data,
            )
            .min_timeout_ms(20_000),
            Test::new(
                "offering early_data anyway still completes the handshake",
                offer_is_tolerated,
            )
            .min_timeout_ms(20_000),
            Test::new(
                "the server does not accept early data it never advertised",
                not_accepted,
            )
            .min_timeout_ms(20_000),
            Test::new(
                "EncryptedExtensions carries no early_data extension",
                no_early_data_extension,
            )
            .min_timeout_ms(20_000),
            Test::new(
                "no EndOfEarlyData is expected on a refused connection",
                no_end_of_early_data,
            )
            .min_timeout_ms(20_000),
            Test::new(
                "data flows normally after early data was refused",
                data_flows,
            )
            .min_timeout_ms(20_000),
            Test::new(
                "offering early_data without a PSK is ignored",
                early_data_without_psk,
            ),
        ],
    }
}

tls_test!(ticket_forbids_early_data, |ctx| {
    let (_, max_early) = get_ticket(ctx, 1).await?;
    let mut c = Check::new("what the ticket says about early data");
    c.note(
        "RFC 8446 section 4.6.1: an early_data extension in the ticket carries \
         max_early_data_size and is the *only* thing that makes 0-RTT available on it.",
    );
    c.observe("new_session_ticket.max_early_data_size", max_early);
    c.eq("new_session_ticket.early_data", None::<u32>, max_early);
    c.note(
        "The reference is run without `-early_data`, so it advertises none. A server that \
         does advertise it would return a size here instead.",
    );
    c.finish()
});

tls_test!(offer_is_tolerated, |ctx| {
    let (offer, _) = get_ticket(ctx, 2).await?;
    let mut config = ctx.config_n(3).with_psk(offer);
    config.offer_early_data = true;
    let client = ctx.handshake_with(config).await?;
    let mut c = Check::new("a hello offering early_data the server never advertised");
    c.block("client_hello", &client.client_hello_bytes);
    c.note(
        "An unwanted early_data extension is not an error. The server ignores it, the \
         handshake proceeds, and the client learns it was refused from the absence of the \
         extension in EncryptedExtensions.",
    );
    c.that(
        "client_hello.extensions",
        "carries early_data",
        client
            .client_hello
            .as_ref()
            .map(|h| find_extension(&h.extensions, EXT_EARLY_DATA).is_some())
            .unwrap_or(false),
        "the suite's own hello did not offer early_data",
    );
    c.that(
        "the handshake",
        "completed",
        !client.server_finished_bytes.is_empty(),
        "did not complete",
    );
    c.finish()
});

tls_test!(not_accepted, |ctx| {
    let (offer, _) = get_ticket(ctx, 4).await?;
    let mut config = ctx.config_n(5).with_psk(offer);
    config.offer_early_data = true;
    let client = ctx.handshake_with(config).await?;
    let accepted = find_extension(&client.encrypted_extensions, EXT_EARLY_DATA).is_some();
    let mut c = Check::new("whether the server accepted early data");
    c.block("encrypted_extensions", &client.encrypted_extensions_bytes);
    c.note(
        "Acceptance is signalled by an empty early_data extension in EncryptedExtensions. Its \
         absence is a refusal, and a refusal needs no alert.",
    );
    c.eq("encrypted_extensions.early_data", false, accepted);
    c.finish()
});

tls_test!(no_early_data_extension, |ctx| {
    let (offer, _) = get_ticket(ctx, 6).await?;
    let mut config = ctx.config_n(7).with_psk(offer);
    config.offer_early_data = true;
    let client = ctx.handshake_with(config).await?;
    let names: Vec<String> = client
        .encrypted_extensions
        .iter()
        .map(|e| ext_name(e.ext_type))
        .collect();
    let mut c = Check::new("the extensions of a refused 0-RTT handshake");
    c.block("encrypted_extensions", &client.encrypted_extensions_bytes);
    c.observe("encrypted_extensions", &names);
    for e in &client.encrypted_extensions {
        c.ne(
            &format!("encrypted_extensions[{}]", ext_name(e.ext_type)),
            EXT_EARLY_DATA,
            e.ext_type,
        );
    }
    c.finish()
});

tls_test!(no_end_of_early_data, |ctx| {
    let (offer, _) = get_ticket(ctx, 8).await?;
    let mut config = ctx.config_n(9).with_psk(offer);
    config.offer_early_data = true;
    let client = ctx.handshake_with(config).await?;
    let labels: Vec<String> = client
        .transcript
        .as_ref()
        .map(|t| t.checkpoints.iter().map(|(l, _)| l.clone()).collect())
        .unwrap_or_default();
    let mut c = Check::new("the transcript of a refused 0-RTT handshake");
    c.note_all(client.transcript_lines());
    c.note(
        "EndOfEarlyData is sent only when the server accepted 0-RTT. Sending one anyway \
         would put a message in the transcript the server never expected.",
    );
    c.that(
        "the transcript",
        "no end_of_early_data in it",
        !labels.iter().any(|l| l.contains("end_of_early_data")),
        labels.join(", "),
    );
    c.that(
        "the client's Finished",
        "sent straight after the server's",
        !client.client_finished_bytes.is_empty(),
        "no client Finished",
    );
    c.finish()
});

tls_test!(data_flows, |ctx| {
    let (offer, _) = get_ticket(ctx, 10).await?;
    let mut config = ctx.config_n(11).with_psk(offer);
    config.offer_early_data = true;
    let mut client = ctx.handshake_with(config).await?;
    let answer = client
        .echo_line("onertt")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("the data path after a refused 0-RTT offer");
    c.note(
        "With early data refused the connection is an ordinary 1-RTT one: the client simply \
         sends its data after the handshake instead of during it.",
    );
    c.eq("echo", reversed("onertt"), answer);
    c.finish()
});

tls_test!(early_data_without_psk, |ctx| {
    // early_data with no pre_shared_key at all: there is no PSK to derive an early traffic
    // secret from, so the extension cannot mean anything.
    let mut config = ctx.config();
    config.offer_early_data = true;
    let client = ctx.handshake_with(config).await?;
    let mut c = Check::new("early_data offered with no PSK behind it");
    c.block("client_hello", &client.client_hello_bytes);
    c.note(
        "0-RTT keys come from the PSK's Early Secret. Without a pre_shared_key extension \
         there is nothing to derive them from, so the offer is simply ignored.",
    );
    c.eq(
        "encrypted_extensions.early_data",
        false,
        find_extension(&client.encrypted_extensions, EXT_EARLY_DATA).is_some(),
    );
    c.that(
        "the handshake",
        "completed",
        !client.server_finished_bytes.is_empty(),
        "did not complete",
    );
    c.finish()
});

fn early_data_config(env: &ExampleEnv) -> ClientConfig {
    let mut config = env.config();
    config.offer_early_data = true;
    config
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::handshake(
            "A hello offering early_data, and the refusal",
            early_data_config,
            Part::ClientHello,
            Part::EncryptedExtensions,
        )
        .request(
            "An ordinary ClientHello with an extra extension: type 42, length 0. The \
             extension is a flag and carries no body.",
        )
        .response(
            "EncryptedExtensions with no early_data extension in it. That absence is the \
             whole refusal — there is no alert and nothing to retry.",
        )
        .note(
            "Offering costs the client nothing. A server that does not want 0-RTT says so by \
             saying nothing, and the connection carries on as a normal 1-RTT handshake.",
        ),
        ExampleSpec::text("Why refusing is the sane default")
            .request(
                "0-RTT data is sent before the server has said anything, so it cannot be \
                 bound to a fresh exchange.",
            )
            .response(
                "An attacker who captures it can replay it, and the server cannot tell the \
                 copy from the original without state it may not have.",
            )
            .note(
                "RFC 8446 appendix E.5 is explicit: applications must not use 0-RTT for \
                 anything non-idempotent. Advertising it is opt-in per ticket, and refusing an \
                 offer is always allowed.",
            ),
    ]
}
