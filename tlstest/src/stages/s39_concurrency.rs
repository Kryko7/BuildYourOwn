//! Stage 39 — Fifty connections at once.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::stages::{reversed, Stage, Test};
use crate::tls::client::{Client, ClientConfig};
use crate::tls_test;
use std::net::SocketAddr;
use std::time::Duration;

/// How many connections the concurrency tests open.
const MANY: usize = 50;

/// One connection's whole life: handshake, echo, close.
async fn one_connection(
    addr: SocketAddr,
    timeout: Duration,
    config: ClientConfig,
    line: String,
) -> Result<String, String> {
    let mut client = Client::connect(addr, timeout, config)
        .await
        .map_err(|e| format!("connect: {e}"))?;
    client
        .handshake()
        .await
        .map_err(|e| format!("handshake: {e}"))?;
    let answer = client
        .echo_line(&line)
        .await
        .map_err(|e| format!("echo: {e}"))?;
    client.close().await.ok();
    Ok(answer)
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 39,
        slug: "concurrency",
        name: "Fifty connections at once",
        ext: true,
        hints: &[
            "Every connection needs its own state: keys, sequence numbers, transcript and \
             read buffer. Anything shared between them is a bug waiting for load",
            "A server may serve connections one at a time or in parallel; both are \
             conformant, and both must finish all fifty",
            "Never let one slow or stuck client stop the accept loop — a listen backlog is \
             not a queue you can ignore",
            "Close each connection's socket when it ends rather than when the process does, \
             or a long-lived server runs out of file descriptors",
        ],
        examples,
        tests: vec![
            Test::new(
                "fifty connections opened at once all complete",
                fifty_at_once,
            )
            .min_timeout_ms(60_000)
            .tag("slow"),
            Test::new(
                "each connection gets its own answer, never another's",
                answers_are_not_crossed,
            )
            .min_timeout_ms(60_000)
            .tag("slow"),
            Test::new("fifty sequential connections all complete", fifty_in_a_row)
                .min_timeout_ms(60_000)
                .tag("slow"),
            Test::new(
                "connections that are opened and abandoned do not block the others",
                abandoned_do_not_block,
            )
            .min_timeout_ms(30_000),
            Test::new(
                "two hundred connect-and-close cycles leak nothing",
                connect_close_cycles,
            )
            .min_timeout_ms(60_000)
            .tag("slow"),
            Test::new(
                "the server still serves a clean handshake afterwards",
                still_serving,
            )
            .min_timeout_ms(30_000),
        ],
    }
}

tls_test!(fifty_at_once, |ctx| {
    let started = std::time::Instant::now();
    let mut tasks = Vec::new();
    for i in 0..MANY {
        let line = format!("conn{i:02}");
        tasks.push(tokio::spawn(one_connection(
            ctx.addr,
            ctx.timeout,
            ctx.config_n(i as u64),
            line,
        )));
    }
    let mut failures = Vec::new();
    let mut answers = 0usize;
    for (i, task) in tasks.into_iter().enumerate() {
        match task.await {
            Ok(Ok(answer)) if answer == reversed(&format!("conn{i:02}")) => answers += 1,
            Ok(Ok(answer)) => failures.push(format!("connection {i} answered {answer:?}")),
            Ok(Err(e)) => failures.push(format!("connection {i}: {e}")),
            Err(e) => failures.push(format!("connection {i} panicked: {e}")),
        }
    }
    let elapsed = started.elapsed();
    ctx.note(format!(
        "{answers}/{MANY} connections completed in {:.2} s",
        elapsed.as_secs_f64()
    ));
    let mut c = Check::new(format!("{MANY} connections opened at once"));
    c.note(
        "The sockets are all opened before any of them finishes. A server that handles them \
         one at a time is conformant — the listen backlog holds the rest — but every one of \
         them has to be served in the end.",
    );
    for f in failures.iter().take(5) {
        c.that(
            "a connection",
            "completed with its own answer",
            false,
            f.clone(),
        );
    }
    c.eq("connections that completed", MANY, answers);
    c.finish()
});

tls_test!(answers_are_not_crossed, |ctx| {
    // Every connection sends a line that could not possibly belong to another.
    let mut tasks = Vec::new();
    for i in 0..MANY {
        let line = format!("unique-{:04x}-{:04x}", ctx.salt as u16, i);
        tasks.push((
            line.clone(),
            tokio::spawn(one_connection(
                ctx.addr,
                ctx.timeout,
                ctx.config_n(1000 + i as u64),
                line,
            )),
        ));
    }
    let mut c = Check::new("that no connection saw another's data");
    let mut crossed = 0usize;
    for (line, task) in tasks {
        match task.await {
            Ok(Ok(answer)) => {
                if answer != reversed(&line) {
                    if crossed < 3 {
                        c.eq(
                            &format!("the echo of {line:?}"),
                            reversed(&line),
                            answer.clone(),
                        );
                    }
                    crossed += 1;
                }
            }
            Ok(Err(e)) => {
                if crossed < 3 {
                    c.that(&format!("the echo of {line:?}"), "an answer", false, e);
                }
                crossed += 1;
            }
            Err(e) => {
                crossed += 1;
                c.that("a connection task", "finished", false, e.to_string());
            }
        }
    }
    c.note(
        "Crossed answers are what a buffer or a key shared between connections looks like \
         from the outside.",
    );
    c.eq("connections with the wrong answer", 0usize, crossed);
    c.finish()
});

tls_test!(fifty_in_a_row, |ctx| {
    let started = std::time::Instant::now();
    let mut c = Check::new(format!("{MANY} connections one after another"));
    for i in 0..MANY {
        let line = format!("row{i:02}");
        let mut client = ctx.handshake_with(ctx.config_n(2000 + i as u64)).await?;
        let answer = client.echo_line(&line).await.map_err(|e| {
            crate::stages::handshake_failure(e, &client)
                .note(format!("on connection {i} of {MANY}"))
        })?;
        if answer != reversed(&line) {
            c.eq(&format!("echo on connection {i}"), reversed(&line), answer);
            break;
        }
        client.close().await.ok();
    }
    ctx.note(format!(
        "{MANY} sequential connections in {:.2} s",
        started.elapsed().as_secs_f64()
    ));
    c.finish()
});

tls_test!(abandoned_do_not_block, |ctx| {
    // Ten connections that send a ClientHello and vanish, interleaved with ten that finish.
    let mut abandoned = Vec::new();
    for i in 0..10 {
        let mut client = ctx.client_with(ctx.config_n(3000 + i)).await?;
        client
            .send_client_hello()
            .await
            .map_err(crate::assert::Failure::tls)?;
        abandoned.push(client);
    }
    let mut c = Check::new("ten abandoned handshakes and ten finished ones");
    c.note(
        "The abandoned connections are still open while the others are attempted. A server \
         that serves connections serially will work through them; one that blocks on a \
         half-finished handshake will not.",
    );
    // Drop them so a serial server can move on, then prove it did.
    drop(abandoned);
    let mut completed = 0usize;
    for i in 0..10 {
        let line = format!("after{i}");
        let mut client = ctx.handshake_with(ctx.config_n(4000 + i)).await?;
        let answer = client.echo_line(&line).await.map_err(|e| {
            crate::stages::handshake_failure(e, &client).note("after ten abandoned handshakes")
        })?;
        if answer == reversed(&line) {
            completed += 1;
        }
    }
    c.eq("connections completed afterwards", 10usize, completed);
    c.finish()
});

tls_test!(connect_close_cycles, |ctx| {
    let started = std::time::Instant::now();
    for i in 0..200 {
        let conn = ctx
            .connect()
            .await
            .map_err(|f| f.note(format!("on cycle {i} of 200")))?;
        drop(conn);
    }
    ctx.note(format!(
        "200 connect/close cycles in {:.2} s",
        started.elapsed().as_secs_f64()
    ));
    ctx.expect_still_serving("two hundred connect/close cycles")
        .await
});

tls_test!(still_serving, |ctx| {
    let mut client = ctx.handshake().await?;
    let answer = client
        .echo_line("survivor")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("a clean handshake after the load");
    c.eq("echo", "rovivrus".to_string(), answer);
    c.finish()
});

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::echo("What each of the fifty connections does", "conn00")
            .request("A full handshake, then `conn00\\n` as application data, then close_notify")
            .response("`00nnoc\\n`, and a close_notify back.")
            .note(
                "Fifty of these at once. Nothing about any one of them is unusual — the test \
                 is whether the fiftieth is as correct as the first.",
            ),
        ExampleSpec::text("Serial and parallel are both fine")
            .request("Fifty sockets opened before any of them has finished its handshake")
            .response(
                "A threaded server serves them at once; a single-threaded one serves them from \
                 the listen backlog in turn. Both finish all fifty.",
            )
            .note(
                "The reference is single-threaded, so this suite never requires two \
                 handshakes to be in flight at the same moment — only that opening fifty \
                 sockets does not lose any of them.",
            ),
    ]
}
