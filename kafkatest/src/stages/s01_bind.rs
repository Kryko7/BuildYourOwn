//! Stage 01 — Bind to the broker port.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::kafka_test;
use crate::stages::{Stage, Test};
use std::time::Duration;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 1,
        slug: "bind",
        name: "Bind to the broker port",
        ext: false,
        hints: &[
            "Create a TCP listener on 0.0.0.0:9092 and accept connections in a loop",
            "Set SO_REUSEADDR so a restart does not hit 'address already in use'",
            "Say nothing until a request arrives: a client speaks first",
            "Accept the next connection even when the previous one is still open",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("the broker accepts a TCP connection", accepts),
            Test::new(
                "nothing is sent before a request arrives",
                silent_until_asked,
            ),
            Test::new(
                "a second connection is accepted while the first is open",
                second_connection,
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
        ],
    }
}

kafka_test!(accepts, |ctx| {
    let conn = ctx.connect().await?;
    let mut c = Check::detached("that the broker accepted the connection");
    c.that(
        "connection.peer",
        &format!("a socket connected to {}", ctx.addr),
        conn.addr == ctx.addr,
        conn.addr,
    );
    c.finish()
});

kafka_test!(silent_until_asked, |ctx| {
    let mut conn = ctx.connect().await?;
    let quiet = Duration::from_millis(300);
    let seen = conn.read_silence(quiet).await.map_err(|e| {
        Failure::proto(e, Some(&conn)).note("the broker closed or wrote unprompted")
    })?;
    let mut c = Check::new("that the broker waits for the client to speak first", &conn);
    c.that(
        "connection.bytes_received_before_any_request",
        "no bytes at all",
        seen.is_empty(),
        format!("{} bytes: {:02x?}", seen.len(), &seen[..seen.len().min(16)]),
    );
    c.finish()
});

kafka_test!(second_connection, |ctx| {
    let first = ctx.connect().await?;
    let second = ctx.connect().await?;
    let mut c = Check::detached("that two connections can be open at once");
    c.that(
        "connections",
        "two independent sockets",
        first.addr == second.addr,
        (first.addr, second.addr),
    );
    c.finish()
});

kafka_test!(reconnect, |ctx| {
    {
        let _first = ctx.connect().await?;
    }
    let _second = ctx
        .connect()
        .await
        .map_err(|f| f.note("the broker stopped accepting after a client disconnected"))?;
    Ok(())
});

kafka_test!(ten_connections, |ctx| {
    let mut conns = Vec::new();
    for i in 0..10 {
        conns.push(
            ctx.connect()
                .await
                .map_err(|f| f.note(format!("connection number {} of 10", i + 1)))?,
        );
    }
    Ok(())
});

kafka_test!(silent_disconnect, |ctx| {
    for _ in 0..5 {
        let conn = ctx.connect().await?;
        drop(conn);
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    let _after = ctx
        .connect()
        .await
        .map_err(|f| f.note("the broker stopped accepting after five silent disconnects"))?;
    Ok(())
});

/// Worked examples: what the broker sees, and what a correct broker does about it.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::text("A client connects and says nothing")
            .request("TCP connect to the broker's port; the client sends no bytes at all")
            .response(
                "Nothing on the wire. The socket stays open and the broker waits for a request.",
            )
            .note(
                "Kafka is strictly request/response: the client always speaks first. A broker \
                 that writes a greeting on accept, or closes an idle connection immediately, \
                 fails this stage.",
            ),
        ExampleSpec::text("A second client connects while the first is still open")
            .request(
                "Two TCP connections to the same port, the second opened before the first closes",
            )
            .response("Both are accepted; nothing is written on either socket.")
            .note(
                "Accept in a loop and hand each connection to its own task or thread. A broker \
                 that only accepts again after the previous client disconnects passes the first \
                 test of this stage and fails the third.",
            ),
    ]
}
