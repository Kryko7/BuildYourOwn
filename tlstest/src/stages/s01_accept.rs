//! Stage 01 — Accept a connection and let the client speak first.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::stages::{Stage, Test};
use crate::tls_test;
use std::time::Duration;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 1,
        slug: "accept",
        name: "Accept a connection and let the client speak first",
        ext: false,
        hints: &[
            "Bind a TCP listener on the port `-accept` names and accept in a loop; the port \
             is chosen by the harness, never hardcoded",
            "Say nothing on accept: TLS is client-speaks-first, and the ClientHello is the \
             first byte either side sends",
            "Set SO_REUSEADDR so a restart does not hit 'address already in use'",
            "A client that connects and vanishes without a ClientHello is normal; close that \
             socket and go back to accepting",
        ],
        examples,
        tests: vec![
            Test::new("the server accepts a TCP connection", accepts),
            Test::new(
                "nothing is sent before the client speaks",
                silent_until_asked,
            ),
            Test::new(
                "a connection is accepted after an earlier one closed",
                reconnect,
            ),
            Test::new("ten connections in a row are all accepted", ten_connections),
            Test::new(
                "a client that disconnects without sending anything is survivable",
                silent_disconnect,
            ),
            Test::new(
                "two sockets can be open at once, and both get served",
                two_sockets,
            ),
            Test::new(
                "the server is still serving after all of that",
                still_serving,
            ),
        ],
    }
}

tls_test!(accepts, |ctx| {
    let conn = ctx.connect().await?;
    let mut c = Check::new("that the server accepted the connection");
    c.that(
        "connection.peer",
        &format!("a socket connected to {}", ctx.addr),
        conn.addr == ctx.addr,
        conn.addr,
    );
    c.finish()
});

tls_test!(silent_until_asked, |ctx| {
    let mut conn = ctx.connect().await?;
    let quiet = Duration::from_millis(400);
    let seen = conn.read_silence(quiet).await.map_err(|e| {
        Failure::tls(e).note(
            "the server wrote or closed before the client had sent a ClientHello; TLS is \
             strictly client-speaks-first",
        )
    })?;
    let mut c = Check::new("that the server waits for the ClientHello");
    c.that(
        "connection.bytes_received_before_the_client_hello",
        "no bytes at all",
        seen.is_empty(),
        format!(
            "{} bytes: {}",
            seen.len(),
            crate::tls::hex_prefix(&seen, 16)
        ),
    );
    c.finish()
});

tls_test!(reconnect, |ctx| {
    {
        let _first = ctx.connect().await?;
    }
    let _second = ctx
        .connect()
        .await
        .map_err(|f| f.note("the server stopped accepting after a client disconnected"))?;
    Ok(())
});

tls_test!(ten_connections, |ctx| {
    for i in 0..10 {
        let conn = ctx
            .connect()
            .await
            .map_err(|f| f.note(format!("connection number {} of 10", i + 1)))?;
        drop(conn);
    }
    Ok(())
});

tls_test!(silent_disconnect, |ctx| {
    for _ in 0..5 {
        let conn = ctx.connect().await?;
        drop(conn);
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    ctx.expect_accepting("five silent connects and disconnects")
        .await
});

// Two sockets open at the same time. A server that handles connections serially (the
// reference does) is perfectly conformant, so this only asks that the second connect is
// accepted by the kernel and that neither socket sees any unsolicited bytes.
tls_test!(two_sockets, |ctx| {
    let mut first = ctx.connect().await?;
    let mut second = ctx
        .connect()
        .await
        .map_err(|f| f.note("the second socket was opened while the first was still open"))?;
    let quiet = Duration::from_millis(200);
    let a = first.read_silence(quiet).await.unwrap_or_default();
    let b = second.read_silence(quiet).await.unwrap_or_default();
    let mut c = Check::new("two sockets open at once");
    c.that(
        "the first socket",
        "no unsolicited bytes",
        a.is_empty(),
        format!("{} bytes", a.len()),
    );
    c.that(
        "the second socket",
        "no unsolicited bytes",
        b.is_empty(),
        format!("{} bytes", b.len()),
    );
    c.finish()?;
    drop(first);
    drop(second);
    ctx.expect_accepting("two sockets held open at once").await
});

tls_test!(still_serving, |ctx| {
    for _ in 0..3 {
        let conn = ctx.connect().await?;
        drop(conn);
    }
    ctx.expect_accepting("a handful of bare TCP connections")
        .await
});

/// Worked examples: what the server sees, and what a correct server does about it.
fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::text("A client connects and says nothing")
            .request("TCP connect to the port named by `-accept`; the client sends no bytes")
            .response(
                "Nothing on the wire. The socket stays open and the server waits for a \
                 ClientHello.",
            )
            .note(
                "TLS is strictly client-speaks-first: the ClientHello is the first byte either \
                 side sends. A server that writes a greeting on accept, or that closes an idle \
                 socket immediately, fails this stage.",
            ),
        ExampleSpec::text("A client connects and disconnects without a ClientHello")
            .request("TCP connect, then an immediate FIN")
            .response(
                "The server closes that socket and goes back to accepting. Nothing is logged \
                 as fatal and the process keeps running.",
            )
            .note(
                "Port scanners and health checks do this all day. `read()` returning 0 before \
                 a ClientHello is the end of a connection, not an error to abort on.",
            ),
    ]
}
