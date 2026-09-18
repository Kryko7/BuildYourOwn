//! Stage 07 — Half-close, abandoned handshakes and staying alive.

use crate::assert::{Check, Failure};
use crate::examples::{ExampleEnv, ExampleSpec, Expect};
use crate::stages::{hello_message, Stage, Test};
use crate::tls::record::Record;
use crate::tls::LEGACY_VERSION_TLS12;
use crate::tls_test;
use std::time::Duration;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 7,
        slug: "half_close",
        name: "Half-close and abandoned handshakes",
        ext: false,
        hints: &[
            "read() returning 0 is the peer's FIN: a normal end of stream, not an error and \
             not a reason to abort the process",
            "A client that shuts down its write half mid-handshake will never send more; drop \
             that connection's state and carry on",
            "Never let one abandoned handshake hold a resource for ever — the accept loop has \
             to come back to accepting",
            "A write to a socket the peer has closed is EPIPE or ECONNRESET; handle it where \
             it happens rather than letting it end the process",
        ],
        examples: examples,
        tests: vec![
            Test::new(
                "a client that half-closes before the ClientHello is survivable",
                half_close_before_hello,
            ),
            Test::new(
                "a client that half-closes right after the ClientHello is survivable",
                half_close_after_hello,
            ),
            Test::new(
                "a client that abandons the handshake mid-flight is survivable",
                abandon_mid_handshake,
            ),
            Test::new(
                "a client that resets the connection after the ServerHello is survivable",
                reset_after_server_hello,
            ),
            Test::new(
                "ten abandoned handshakes in a row do not wedge the server",
                ten_abandoned,
            ),
            Test::new(
                "a clean handshake still works after every one of those",
                still_serving,
            ),
        ],
    }
}

tls_test!(half_close_before_hello, |ctx| {
    let mut conn = ctx.connect().await?;
    conn.shutdown_write().await.map_err(Failure::tls)?;
    // Whatever the server says, it must not die.
    let _ = conn.read_silence(Duration::from_millis(300)).await;
    drop(conn);
    ctx.expect_still_serving("a half-close before any TLS bytes")
        .await
});

tls_test!(half_close_after_hello, |ctx| {
    let message = hello_message(&ctx.config()).map_err(crate::stages::harness)?;
    let mut conn = ctx.connect().await?;
    conn.write_raw(&Record::build(22, LEGACY_VERSION_TLS12, &message))
        .await
        .map_err(Failure::tls)?;
    conn.shutdown_write().await.map_err(Failure::tls)?;
    // The server may still send its whole flight into a half-closed socket; reading it is
    // optional, surviving it is not.
    let _ = conn.read_silence(Duration::from_millis(400)).await;
    drop(conn);
    ctx.expect_still_serving("a half-close straight after the ClientHello")
        .await
});

tls_test!(abandon_mid_handshake, |ctx| {
    let mut client = ctx.client().await?;
    client.send_client_hello().await.map_err(Failure::tls)?;
    client.maybe_send_ccs().await.map_err(Failure::tls)?;
    if let Err(e) = client.read_server_hello().await {
        return Err(crate::stages::handshake_failure(e, &client));
    }
    // Walk away without ever sending Finished.
    drop(client);
    tokio::time::sleep(Duration::from_millis(50)).await;
    ctx.expect_still_serving("a handshake abandoned after the ServerHello")
        .await
});

tls_test!(reset_after_server_hello, |ctx| {
    let mut client = ctx.client().await?;
    client.send_client_hello().await.map_err(Failure::tls)?;
    if let Err(e) = client.read_server_hello().await {
        return Err(crate::stages::handshake_failure(e, &client));
    }
    // Send half a record and vanish: the server is now waiting for bytes that never come.
    client
        .conn
        .write_raw(&[23, 0x03, 0x03, 0x01, 0x00, 0xaa, 0xbb])
        .await
        .map_err(Failure::tls)?;
    drop(client);
    ctx.expect_still_serving("a truncated record and a reset")
        .await
});

tls_test!(ten_abandoned, |ctx| {
    let message = hello_message(&ctx.config()).map_err(crate::stages::harness)?;
    for i in 0..10 {
        let mut conn = ctx
            .connect()
            .await
            .map_err(|f| f.note(format!("abandoned handshake number {} of 10", i + 1)))?;
        conn.write_raw(&Record::build(22, LEGACY_VERSION_TLS12, &message))
            .await
            .map_err(Failure::tls)?;
        drop(conn);
    }
    ctx.note("ten handshakes started and abandoned");
    ctx.expect_still_serving("ten abandoned handshakes").await
});

tls_test!(still_serving, |ctx| {
    let mut client = ctx.client_with(ctx.config_n(3)).await?;
    client.send_client_hello().await.map_err(Failure::tls)?;
    drop(client);
    let mut conn = ctx.connect().await?;
    conn.shutdown_write().await.map_err(Failure::tls)?;
    drop(conn);
    let mut client = ctx
        .handshake_with(ctx.config_n(4))
        .await
        .map_err(|f| f.note("after an abandoned handshake and a half-close"))?;
    let answer = client
        .echo_line("alive")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("the echo on a connection after two abandoned ones");
    c.eq("echo", "evila".to_string(), answer);
    c.finish()
});

fn hello_then_fin(env: &ExampleEnv) -> Result<Vec<u8>, String> {
    Ok(Record::build(
        22,
        LEGACY_VERSION_TLS12,
        &hello_message(&env.config())?,
    ))
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::raw(
            "A ClientHello, then the client shuts down its write half",
            hello_then_fin,
            Expect::UntilClose,
        )
        .request(
            "A complete ClientHello, immediately followed by a TCP FIN on the client's write \
             half. No Finished will ever arrive.",
        )
        .response(
            "The server may send its whole flight anyway, and then closes. Either way the \
             process survives and the next connection is accepted.",
        )
        .note(
            "read() == 0 means the peer will send nothing more. Free that connection's state \
             and return to accept(); it is not an error condition.",
        ),
        ExampleSpec::text("An abandoned handshake")
            .request(
                "The client sends a ClientHello, reads the ServerHello, and then simply stops — \
                 no Finished, no close_notify, the socket left open.",
            )
            .response(
                "Nothing, until the server's own timeout. What matters is that the server is \
                 still accepting new connections while it waits.",
            )
            .note(
                "This is what a port scanner and a flaky mobile client both look like. A server \
                 that blocks its accept loop on one half-finished handshake is trivially \
                 denied service by a single connection.",
            ),
    ]
}
