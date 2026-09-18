//! Stage 34 — `close_notify` in both directions.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::stages::{Stage, Test};
use crate::tls::conn::Reaction;
use crate::tls::msg::Alert;
use crate::tls::{AlertDescription, AlertLevel, ContentType};
use crate::tls_test;
use std::time::Duration;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 34,
        slug: "close_notify",
        name: "close_notify in both directions",
        ext: false,
        hints: &[
            "close_notify is an alert record — level warning(1), description 0 — sent \
             encrypted under the current application keys",
            "It says 'I will write no more'; the other direction stays open until that side \
             sends its own, which is why it is a warning and not fatal",
            "A server that receives one should stop reading that connection and may answer \
             with its own before closing the socket",
            "A TCP close with no close_notify is a truncation: legal to tolerate, worth \
             telling apart from a clean end of stream",
        ],
        examples,
        tests: vec![
            Test::new("a close_notify from the client is accepted", client_closes),
            Test::new(
                "the server answers with its own close_notify or closes",
                server_answers,
            ),
            Test::new("the alert is level warning(1), not fatal", level_is_warning),
            Test::new(
                "the alert is encrypted, not sent in the clear",
                alert_is_encrypted,
            ),
            Test::new(
                "data written before close_notify still gets answered",
                data_then_close,
            ),
            Test::new("a TCP close with no close_notify is survivable", truncation),
            Test::new(
                "the server still serves a new connection after a clean close",
                still_serving,
            ),
        ],
    }
}

tls_test!(client_closes, |ctx| {
    let mut client = ctx.handshake().await?;
    let answer = client
        .echo_line("bye")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    client.close().await.map_err(Failure::tls)?;
    let mut c = Check::new("a connection ended with close_notify");
    c.eq("echo", "eyb".to_string(), answer);
    c.note("the client wrote an encrypted alert record: level 1, description 0");
    c.finish()?;
    drop(client);
    ctx.expect_still_serving("a clean close_notify").await
});

tls_test!(server_answers, |ctx| {
    let mut client = ctx.handshake().await?;
    client
        .echo_line("closing")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    client.close().await.map_err(Failure::tls)?;
    let reaction = crate::stages::reaction_past_tickets(&mut client.conn).await;
    let mut c = Check::new("what the server does with a close_notify");
    c.note(
        "RFC 8446 section 6.1: each side sends its own close_notify before closing its write \
         side. A server that simply closes the socket is common and tolerated.",
    );
    match &reaction {
        Reaction::Alert(alert) => {
            c.eq(
                "the server's alert.description",
                AlertDescription::CLOSE_NOTIFY.0,
                alert.description.0,
            );
        }
        Reaction::Closed => {
            c.note("the server closed the connection without answering, which is allowed");
        }
        other => {
            c.that(
                "the server's reaction",
                "close_notify, or a closed connection",
                false,
                other.describe(),
            );
        }
    }
    c.finish()
});

tls_test!(level_is_warning, |ctx| {
    let mut client = ctx.handshake().await?;
    client
        .echo_line("level")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    client.close().await.map_err(Failure::tls)?;
    let reaction = crate::stages::reaction_past_tickets(&mut client.conn).await;
    let mut c = Check::new("the level of the server's close_notify");
    match reaction.alert() {
        Some(alert) => {
            c.note(
                "TLS 1.3 has only two warning-level alerts left: close_notify and \
                 user_canceled. Everything else is fatal by definition.",
            );
            c.eq(
                "alert.level",
                AlertLevel::Warning.as_u8(),
                alert.level.as_u8(),
            );
            c.eq(
                "alert.description",
                AlertDescription::CLOSE_NOTIFY.0,
                alert.description.0,
            );
        }
        None => {
            c.note("the server closed without an alert, which RFC 8446 allows");
        }
    }
    c.finish()
});

tls_test!(alert_is_encrypted, |ctx| {
    let mut client = ctx.handshake().await?;
    client
        .echo_line("secret")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let before = client.conn.records_in.len();
    client.close().await.map_err(Failure::tls)?;
    let _ = crate::stages::reaction_past_tickets(&mut client.conn).await;
    let mut c = Check::new("whether the server's alert went out in the clear");
    let plaintext_alerts = client
        .conn
        .records_in
        .iter()
        .skip(before)
        .filter(|r| r.content_type == ContentType::Alert)
        .count();
    c.note(
        "Once application keys are in force every record is application_data(23) on the \
         outside, alerts included — a passive observer cannot even tell a connection ended \
         politely.",
    );
    c.eq(
        "plaintext alert records after the handshake",
        0usize,
        plaintext_alerts,
    );
    c.finish()
});

tls_test!(data_then_close, |ctx| {
    let mut client = ctx.handshake().await?;
    // Write the line and the close_notify back to back, then read the answer.
    client
        .write_app_data(b"lastword\n")
        .await
        .map_err(Failure::tls)?;
    client.close().await.map_err(Failure::tls)?;
    let answer = client.read_line().await.map_err(|e| {
        crate::stages::handshake_failure(e, &client).note(
            "the line and the close_notify were written back to back; the line still has to \
             be answered",
        )
    })?;
    let mut c = Check::new("a line written immediately before close_notify");
    c.eq("echo", "drowtsal".to_string(), answer);
    c.finish()
});

tls_test!(truncation, |ctx| {
    let mut client = ctx.handshake().await?;
    client
        .echo_line("truncate")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    // Close the TCP connection with no close_notify at all.
    client.conn.shutdown_write().await.map_err(Failure::tls)?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(client);
    ctx.expect_still_serving("a TCP close with no close_notify")
        .await
});

tls_test!(still_serving, |ctx| {
    {
        let mut client = ctx.handshake_with(ctx.config_n(1)).await?;
        client
            .echo_line("first")
            .await
            .map_err(|e| crate::stages::handshake_failure(e, &client))?;
        client.close().await.map_err(Failure::tls)?;
    }
    let mut client = ctx.handshake_with(ctx.config_n(2)).await?;
    let answer = client
        .echo_line("second")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("a second connection after a clean close");
    c.eq("echo", "dnoces".to_string(), answer);
    c.finish()
});

fn examples() -> Vec<ExampleSpec> {
    let _ = Alert::close_notify();
    vec![
        ExampleSpec::echo("The last line before a close", "bye")
            .request("`bye\\n`, and then an alert record carrying `01 00` — warning, close_notify")
            .response(
                "`eyb\\n`, and then the server's own close_notify, also encrypted, before it \
                 closes the socket.",
            )
            .note(
                "The alert is two bytes of plaintext inside an application_data record. Its \
                 ciphertext is 2 + 1 + 16 = 19 bytes, which is how you spot one without \
                 keys — and all you can tell is that *something* two bytes long was said.",
            ),
        ExampleSpec::text("Why close_notify is a warning")
            .request("One side has finished writing and says so")
            .response(
                "The other side may keep writing until it sends its own close_notify. Only \
                 then is the connection over.",
            )
            .note(
                "A TCP close without close_notify is a truncation attack in the general case: \
                 the receiver cannot tell a finished stream from a cut one. TLS 1.3 tolerates \
                 it, and an application that cares has to check.",
            ),
    ]
}
