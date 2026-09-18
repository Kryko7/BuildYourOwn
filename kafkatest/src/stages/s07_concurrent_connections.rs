//! Stage 07 — Concurrent connections.

use crate::assert::Check;
use crate::assert::Failure;
use crate::examples::ExampleSpec;
use crate::kafka_test;
use crate::proto::Conn;
use crate::stages::{
    api_versions_body_v4, correlation_id_of, raw_flexible_frame, Stage, Test, API_VERSIONS_KEY,
    API_VERSIONS_V4,
};
use rand::seq::SliceRandom;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 7,
        slug: "concurrent_connections",
        name: "Concurrent connections",
        ext: false,
        hints: &[
            "Handle each accepted socket in its own thread, task or poll loop",
            "Never let one slow client block the accept loop",
            "Connection state (buffers, correlation ids) must be per connection, not global",
            "Clean up when a client goes away so file descriptors are not leaked",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("eight clients are served at once", eight_clients),
            Test::new(
                "interleaved requests keep their own correlation ids",
                interleaved,
            ),
            Test::new(
                "a seeded random interleaving is answered correctly",
                random_interleaving,
            ),
            Test::new(
                "a client that hangs up does not disturb the others",
                one_hangs_up,
            ),
            Test::new("twenty connections can be open simultaneously", twenty_open),
        ],
    }
}

fn frame(id: i32) -> Vec<u8> {
    raw_flexible_frame(
        API_VERSIONS_KEY,
        API_VERSIONS_V4,
        id,
        Some("kafkatest"),
        &[],
        &api_versions_body_v4(),
    )
}

async fn send_on(conn: &mut Conn, id: i32) -> Result<(), Failure> {
    conn.send_frame(&frame(id))
        .await
        .map_err(|e| Failure::proto(e, Some(conn)))
}

async fn read_id(conn: &mut Conn) -> Result<i32, Failure> {
    let resp = conn
        .read_frame()
        .await
        .map_err(|e| Failure::proto(e, Some(conn)))?;
    correlation_id_of(&resp, conn)
}

kafka_test!(eight_clients, |ctx| {
    let mut conns = Vec::new();
    for i in 0..8i32 {
        let mut c = ctx.connect().await?;
        send_on(&mut c, 5000 + i).await?;
        conns.push(c);
    }
    for (i, conn) in conns.iter_mut().enumerate() {
        let got = read_id(conn).await?;
        let mut c = Check::new(format!("client {} of 8", i + 1), conn);
        c.mark(0..4);
        c.eq("response.correlation_id", 5000 + i as i32, got);
        c.finish()?;
    }
    Ok(())
});

kafka_test!(interleaved, |ctx| {
    let mut a = ctx.connect().await?;
    let mut b = ctx.connect().await?;
    let mut c = ctx.connect().await?;
    send_on(&mut a, 601).await?;
    send_on(&mut b, 602).await?;
    send_on(&mut c, 603).await?;
    // Read in a different order than the sends.
    let got_c = read_id(&mut c).await?;
    let got_a = read_id(&mut a).await?;
    let got_b = read_id(&mut b).await?;
    let mut chk = Check::new("three interleaved clients", &a);
    chk.eq("connection_a.response.correlation_id", 601i32, got_a);
    chk.eq("connection_b.response.correlation_id", 602i32, got_b);
    chk.eq("connection_c.response.correlation_id", 603i32, got_c);
    chk.finish()
});

kafka_test!(random_interleaving, |ctx| {
    let n = 6usize;
    let mut conns = Vec::new();
    for i in 0..n {
        conns.push((7000 + i as i32, ctx.connect().await?));
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.shuffle(&mut ctx.rng);
    for &i in &order {
        let (id, conn) = &mut conns[i];
        let id = *id;
        send_on(conn, id).await?;
    }
    order.shuffle(&mut ctx.rng);
    for &i in &order {
        let (id, conn) = &mut conns[i];
        let want = *id;
        let got = read_id(conn).await?;
        let mut c = Check::new(format!("client {i} in a seeded random interleaving"), conn);
        c.note(format!("seed {:#x}", ctx.seed));
        c.mark(0..4);
        c.eq("response.correlation_id", want, got);
        c.finish()?;
    }
    Ok(())
});

kafka_test!(one_hangs_up, |ctx| {
    let mut keep = ctx.connect().await?;
    {
        let mut doomed = ctx.connect().await?;
        send_on(&mut doomed, 801).await?;
        // Drop without reading the response.
    }
    send_on(&mut keep, 802).await?;
    let got = read_id(&mut keep).await?;
    let mut c = Check::new("the surviving connection after a peer vanished", &keep);
    c.mark(0..4);
    c.eq("response.correlation_id", 802i32, got);
    c.finish()
});

kafka_test!(twenty_open, |ctx| {
    let mut conns = Vec::new();
    for i in 0..20i32 {
        let mut c = ctx
            .connect()
            .await
            .map_err(|f| f.note(format!("connection {} of 20", i + 1)))?;
        send_on(&mut c, 9000 + i).await?;
        conns.push(c);
    }
    for (i, conn) in conns.iter_mut().enumerate() {
        let got = read_id(conn).await?;
        if got != 9000 + i as i32 {
            let mut c = Check::new(format!("connection {} of 20", i + 1), conn);
            c.mark(0..4);
            c.eq("response.correlation_id", 9000 + i as i32, got);
            return c.finish();
        }
    }
    Ok(())
});

/// Worked examples: several clients at the same time.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::text("Four clients, interleaved")
            .request(
                "Four TCP connections; each writes one ApiVersions v4 frame with its own \
                 correlation id (7001, 7002, 7003, 7004) before any of them reads a byte back",
            )
            .response(
                "Four responses, one per socket, each echoing that socket's correlation id. \
                 Connection 7003 may be answered before 7001 — what must never happen is \
                 7001's answer arriving on 7003's socket.",
            )
            .note(
                "Correlation ids are per connection, not global. Keep the read buffer and the \
                 next expected size per socket: a single shared buffer is the classic bug this \
                 stage catches.",
            ),
        ExampleSpec::text("One slow client must not stop the others")
            .request("Ten connections are opened and left idle; an eleventh sends ApiVersions v4")
            .response("The eleventh is answered immediately, while the ten idle sockets stay open")
            .note(
                "Handle each connection concurrently (a task per socket, or a poll loop). A \
                 broker that reads one connection to end-of-stream before accepting the next \
                 one deadlocks on the first idle client.",
            ),
    ]
}
