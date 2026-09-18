//! Stage 45 — Soak: 10 000 records and 1 000 connections. **[ext]**
//!
//! The only hard limit here is that each test finishes inside [`BUDGET`]; the latency and
//! throughput numbers are informational and printed through `ctx.note(..)`, so they show up
//! under a passing test too. A test that runs out of budget fails with the count it actually
//! reached ("3 200 of 10 000 records in 60 s"), which is far more useful than a bare timeout.
//!
//! Everything is measured after a warm-up loop, because the reference broker is a JVM and
//! its first few hundred requests are interpreted, not compiled. Connections are closed
//! explicitly and the file descriptors this process holds are counted before and after, so a
//! leak on *our* side never gets reported as a broker problem.

use crate::assert::{Check, Failure, FailureKind};
use crate::examples::ExampleSpec;
use crate::fixtures::{FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::proto::records::{RecordBatch, RecordItem};
use crate::proto::Conn;
use crate::stages::{
    api_versions_request, await_high_watermark, expect_still_serving, fetch_request,
    produce_request, proto_fail, Ctx, Stage, Test, API_VERSIONS_V4, FETCH_V16, NONE, PRODUCE_V11,
};
use std::time::{Duration, Instant};

/// Every test must finish inside this, or fail with what it achieved.
const BUDGET: Duration = Duration::from_secs(60);

/// Records produced and fetched back by the round-trip test.
const RECORDS: usize = 10_000;
/// Records per Produce request.
const BATCH: usize = 100;
/// Partitions the records are spread over.
const PARTITIONS: i32 = 4;
/// Connect/ApiVersions/close cycles.
const CYCLES: usize = 1_000;
/// Connections used by the concurrency test.
const CONNECTIONS: usize = 50;
/// Requests each of those connections makes.
const PER_CONNECTION: usize = 100;
/// Requests thrown away before any measurement, so JVM warm-up is not in the numbers.
const WARMUP: usize = 200;

fn soak_topic() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", PARTITIONS))
}

fn soak_test(name: &'static str, run: crate::stages::TestFn) -> Test {
    Test::new(name, run)
        .ext()
        .tag("slow")
        .min_timeout_ms(70_000)
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 45,
        slug: "soak",
        name: "Soak: 10 000 records and 1 000 connections",
        ext: true,
        hints: &[
            "Produce and fetch 10 000 records and compare them byte for byte; offsets inside \
             a partition must be contiguous with no gap and no repeat",
            "Open and close 1 000 connections without leaking file descriptors or threads — \
             close the socket when the client goes away, not when the process exits",
            "Fifty concurrent connections must not serialize behind one another, and per \
             connection the responses still come back in request order",
            "p50/p95/p99 latency is reported for information; the only hard limit is that \
             each test finishes within 60 seconds",
        ],
        examples: wire_examples,
        tests: vec![
            soak_test(
                "10 000 records are produced and fetched back byte for byte",
                round_trip,
            )
            .with_fixtures(soak_topic),
            soak_test("offsets are contiguous across every partition", contiguous)
                .with_fixtures(soak_topic),
            soak_test("1 000 connect/ApiVersions/close cycles all succeed", cycles),
            soak_test(
                "50 concurrent connections each complete 100 requests",
                concurrent,
            ),
            soak_test("request latency percentiles are reported", latency),
            soak_test(
                "the broker survives the soak and we leak no file descriptors",
                survives,
            )
            .with_fixtures(soak_topic),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Measurement helpers
// ---------------------------------------------------------------------------------------

/// The `p`-th percentile of a sorted latency sample (`p` in 0.0..=1.0).
fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// `p50 0.21 ms · p95 0.44 ms · p99 1.10 ms · max 3.02 ms` for a latency sample.
fn percentiles(latencies: &mut [Duration]) -> String {
    latencies.sort_unstable();
    let ms = |d: Duration| format!("{:.2} ms", d.as_secs_f64() * 1_000.0);
    format!(
        "p50 {} · p95 {} · p99 {} · max {}",
        ms(percentile(latencies, 0.50)),
        ms(percentile(latencies, 0.95)),
        ms(percentile(latencies, 0.99)),
        ms(percentile(latencies, 1.0))
    )
}

/// Operations per second, for a throughput note.
fn rate(n: usize, elapsed: Duration) -> String {
    let secs = elapsed.as_secs_f64().max(f64::EPSILON);
    format!("{:.0}/s", n as f64 / secs)
}

/// File descriptors this process holds, on platforms that expose them.
fn open_fds() -> Option<usize> {
    std::fs::read_dir("/proc/self/fd").ok().map(Iterator::count)
}

/// Fail with what was achieved, rather than letting the harness report a bare timeout.
fn out_of_budget(what: &str, achieved: usize, target: usize, started: Instant) -> Failure {
    Failure::new(
        FailureKind::Timeout,
        format!(
            "{what}: only {achieved} of {target} finished within {} s (took {:.1} s so far)",
            BUDGET.as_secs(),
            started.elapsed().as_secs_f64()
        ),
    )
    .note("the soak stage's only hard limit is the 60 s budget; the numbers around it are informational")
}

/// The value every record in the soak carries, so a byte-for-byte comparison means something.
fn value_of(n: usize) -> Vec<u8> {
    format!("soak-{n:06}-kafkatest").into_bytes()
}

/// Produce [`RECORDS`] records in batches of [`BATCH`], round-robin over the partitions.
///
/// Returns, per partition, the values written in offset order and the wall-clock time.
async fn produce_all(ctx: &Ctx, topic: &crate::fixtures::TopicInfo) -> Result<Produced, Failure> {
    let started = Instant::now();
    let mut expected: Vec<Vec<Vec<u8>>> = vec![Vec::new(); PARTITIONS as usize];
    let mut conn = ctx.connect().await?;
    let mut latencies = Vec::with_capacity(RECORDS / BATCH);
    let mut written = 0usize;
    for (b, chunk) in (0..RECORDS).collect::<Vec<_>>().chunks(BATCH).enumerate() {
        if started.elapsed() > BUDGET {
            return Err(out_of_budget("producing", written, RECORDS, started));
        }
        let partition = (b as i32) % PARTITIONS;
        let items: Vec<RecordItem> = chunk
            .iter()
            .map(|n| {
                let v = value_of(*n);
                expected[partition as usize].push(v.clone());
                RecordItem::value(v)
            })
            .collect();
        let batch = RecordBatch::of(0, 1_700_000_000_000, items).encode();
        let req = produce_request(&[(topic.name.clone(), partition, batch)], -1);
        let at = Instant::now();
        let resp = conn
            .request(PRODUCE_V11, &req)
            .await
            .map_err(|e| proto_fail(e, &conn).note(format!("on produce batch {b}")))?;
        latencies.push(at.elapsed());
        let code = resp
            .responses
            .first()
            .and_then(|t| t.partition_responses.first())
            .map(|p| p.error_code)
            .unwrap_or(-1);
        if code != NONE {
            let mut c = Check::new(format!("produce batch {b} of the soak"), &conn);
            c.eq(
                &format!("response.responses[0].partition_responses[index={partition}].error_code"),
                NONE,
                code,
            );
            c.finish()?;
        }
        written += chunk.len();
    }
    drop(conn);
    Ok(Produced {
        expected,
        latencies,
        elapsed: started.elapsed(),
    })
}

/// What [`produce_all`] wrote.
struct Produced {
    expected: Vec<Vec<Vec<u8>>>,
    latencies: Vec<Duration>,
    elapsed: Duration,
}

/// Fetch a partition from offset 0 until `want` records have arrived.
///
/// Returns `(absolute offset, value)` pairs in the order the broker sent them.
async fn fetch_all(
    ctx: &Ctx,
    topic: &crate::fixtures::TopicInfo,
    partition: i32,
    want: usize,
    started: Instant,
) -> Result<Vec<(i64, Vec<u8>)>, Failure> {
    let mut got: Vec<(i64, Vec<u8>)> = Vec::with_capacity(want);
    let mut next: i64 = 0;
    while got.len() < want {
        if started.elapsed() > BUDGET {
            return Err(out_of_budget(
                &format!("fetching partition {partition}"),
                got.len(),
                want,
                started,
            ));
        }
        let mut conn = ctx.connect().await?;
        let req = fetch_request(&[(topic.id, partition, next)], 1_000);
        let resp = conn
            .request(FETCH_V16, &req)
            .await
            .map_err(|e| proto_fail(e, &conn).note(format!("fetching from offset {next}")))?;
        let part = resp
            .responses
            .first()
            .and_then(|t| t.partitions.first())
            .ok_or_else(|| {
                Failure::harness(format!(
                    "the fetch of {}-{partition} from offset {next} returned no partition entry",
                    topic.name
                ))
            })?;
        if part.error_code != NONE {
            let mut c = Check::new(format!("fetching {}-{partition}", topic.name), &conn);
            c.eq(
                "response.responses[0].partitions[0].error_code",
                NONE,
                part.error_code,
            );
            c.note(format!("from offset {next}, after {} records", got.len()));
            c.finish()?;
        }
        let bytes = part.records.clone().map(|b| b.to_vec()).unwrap_or_default();
        let batches = RecordBatch::decode_all(&bytes).map_err(|e| {
            Failure::harness(format!(
                "the records of {}-{partition} from offset {next} do not decode: {e:#}",
                topic.name
            ))
        })?;
        let before = got.len();
        for b in &batches {
            for r in &b.records {
                let offset = b.base_offset + i64::from(r.offset_delta);
                if offset >= next {
                    got.push((offset, r.value.clone().unwrap_or_default()));
                }
            }
        }
        drop(conn);
        if got.len() == before {
            return Err(Failure::new(
                FailureKind::Assertion,
                format!(
                    "the fetch of {}-{partition} from offset {next} returned no new records \
                     after {before} of {want}",
                    topic.name
                ),
            )
            .note(
                "a fetch that makes no progress is how a soak finds an off-by-one in the index",
            ));
        }
        next = got.last().map(|(o, _)| o + 1).unwrap_or(next);
    }
    Ok(got)
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

kafka_test!(round_trip, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let started = Instant::now();
    let mut produced = produce_all(ctx, &t).await?;
    ctx.note(format!(
        "produced {RECORDS} records in {} batches of {BATCH} over {PARTITIONS} partitions in \
         {:.2} s ({} records, {} batches); batch latency {}",
        RECORDS / BATCH,
        produced.elapsed.as_secs_f64(),
        rate(RECORDS, produced.elapsed),
        rate(RECORDS / BATCH, produced.elapsed),
        percentiles(&mut produced.latencies)
    ));

    let per_partition = RECORDS / PARTITIONS as usize;
    for p in 0..PARTITIONS {
        await_high_watermark(ctx, &t, p, per_partition as i64).await?;
    }

    let fetch_started = Instant::now();
    let mut fetched = 0usize;
    for p in 0..PARTITIONS {
        let want = produced.expected[p as usize].len();
        let got = fetch_all(ctx, &t, p, want, started).await?;
        fetched += got.len();
        let values: Vec<Vec<u8>> = got.iter().map(|(_, v)| v.clone()).collect();
        let mut c = Check::detached(format!(
            "the {want} records of partition {p}, byte for byte"
        ));
        c.eq(&format!("partition {p} record count"), want, values.len());
        if let Some((i, (want_v, got_v))) = produced.expected[p as usize]
            .iter()
            .zip(values.iter())
            .enumerate()
            .find(|(_, (a, b))| a != b)
        {
            c.bytes_eq(&format!("partition {p} record {i} value"), want_v, got_v);
        }
        c.finish()?;
    }
    ctx.note(format!(
        "fetched {fetched} records back in {:.2} s ({})",
        fetch_started.elapsed().as_secs_f64(),
        rate(fetched, fetch_started.elapsed())
    ));
    let mut c = Check::detached("the whole 10 000 record round trip");
    c.eq("records fetched", RECORDS, fetched);
    c.at_most(
        "elapsed_ms",
        BUDGET.as_millis() as u64,
        started.elapsed().as_millis() as u64,
    );
    c.finish()
});

kafka_test!(contiguous, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let started = Instant::now();
    let produced = produce_all(ctx, &t).await?;
    let per_partition = RECORDS / PARTITIONS as usize;
    for p in 0..PARTITIONS {
        await_high_watermark(ctx, &t, p, per_partition as i64).await?;
    }
    let mut c = Check::detached("the offsets of every partition after 10 000 appends");
    for p in 0..PARTITIONS {
        let want = produced.expected[p as usize].len();
        let got = fetch_all(ctx, &t, p, want, started).await?;
        let offsets: Vec<i64> = got.iter().map(|(o, _)| *o).collect();
        let first_gap = offsets
            .iter()
            .enumerate()
            .find(|(i, o)| **o != *i as i64)
            .map(|(i, o)| format!("record {i} has offset {o}"));
        c.that(
            &format!("partition {p} offsets"),
            &format!("0..{want} with no gap and no repeat"),
            first_gap.is_none(),
            first_gap.unwrap_or_else(|| "contiguous".to_string()),
        );
        c.eq(
            &format!("partition {p} last offset"),
            want as i64 - 1,
            *offsets.last().unwrap_or(&-1),
        );
    }
    c.finish()?;
    ctx.note(format!(
        "{RECORDS} offsets verified contiguous across {PARTITIONS} partitions in {:.2} s",
        started.elapsed().as_secs_f64()
    ));
    Ok(())
});

kafka_test!(cycles, |ctx| {
    // Warm the JVM up first: the first few hundred requests of a cold broker are interpreted.
    for _ in 0..WARMUP {
        let mut conn = ctx.connect().await?;
        conn.request(API_VERSIONS_V4, &api_versions_request())
            .await
            .map_err(|e| proto_fail(e, &conn).note("during warm-up"))?;
        drop(conn);
    }

    let started = Instant::now();
    let mut latencies = Vec::with_capacity(CYCLES);
    for i in 0..CYCLES {
        if started.elapsed() > BUDGET {
            return Err(out_of_budget(
                "connect/ApiVersions/close cycles",
                i,
                CYCLES,
                started,
            ));
        }
        let at = Instant::now();
        let mut conn = ctx.connect().await.map_err(|f| {
            f.note(format!("on cycle {i} of {CYCLES}")).note(
                "a broker that stops accepting part way through is leaking sockets or threads",
            )
        })?;
        let resp = conn
            .request(API_VERSIONS_V4, &api_versions_request())
            .await
            .map_err(|e| proto_fail(e, &conn).note(format!("on cycle {i} of {CYCLES}")))?;
        if resp.error_code != NONE {
            let mut c = Check::new(format!("cycle {i} of {CYCLES}"), &conn);
            c.eq("response.error_code", NONE, resp.error_code);
            c.finish()?;
        }
        // Close explicitly rather than waiting for the end of the test.
        drop(conn);
        latencies.push(at.elapsed());
    }
    let elapsed = started.elapsed();
    ctx.note(format!(
        "{CYCLES} connect/ApiVersions/close cycles in {:.2} s ({}); cycle latency {}",
        elapsed.as_secs_f64(),
        rate(CYCLES, elapsed),
        percentiles(&mut latencies)
    ));
    let mut c = Check::detached("1 000 connect/ApiVersions/close cycles");
    c.at_most(
        "elapsed_ms",
        BUDGET.as_millis() as u64,
        elapsed.as_millis() as u64,
    );
    c.finish()
});

kafka_test!(concurrent, |ctx| {
    let addr = ctx.addr;
    let timeout = ctx.timeout;
    let started = Instant::now();
    let mut handles = Vec::with_capacity(CONNECTIONS);
    for id in 0..CONNECTIONS {
        handles.push(tokio::spawn(async move {
            let mut conn = Conn::connect(addr, timeout)
                .await
                .map_err(|e| format!("connection {id} could not be opened: {e}"))?;
            let mut latencies = Vec::with_capacity(PER_CONNECTION);
            for n in 0..PER_CONNECTION {
                let at = Instant::now();
                let resp = conn
                    .request(API_VERSIONS_V4, &api_versions_request())
                    .await
                    .map_err(|e| format!("connection {id} request {n}: {e}"))?;
                if resp.error_code != NONE {
                    return Err(format!(
                        "connection {id} request {n}: response.error_code was {}, expected 0",
                        resp.error_code
                    ));
                }
                latencies.push(at.elapsed());
            }
            // Explicit close, so the file-descriptor check below means something.
            drop(conn);
            Ok::<Vec<Duration>, String>(latencies)
        }));
    }

    let mut latencies = Vec::with_capacity(CONNECTIONS * PER_CONNECTION);
    let mut done = 0usize;
    let mut problems: Vec<String> = Vec::new();
    for h in handles {
        match h.await {
            Ok(Ok(mut l)) => {
                done += l.len();
                latencies.append(&mut l);
            }
            Ok(Err(why)) => problems.push(why),
            Err(e) => problems.push(format!("a client task panicked: {e}")),
        }
    }
    let elapsed = started.elapsed();
    let total = CONNECTIONS * PER_CONNECTION;
    ctx.note(format!(
        "{CONNECTIONS} concurrent connections × {PER_CONNECTION} requests = {done}/{total} in \
         {:.2} s ({}); request latency {}",
        elapsed.as_secs_f64(),
        rate(done, elapsed),
        percentiles(&mut latencies)
    ));
    let mut c = Check::detached("50 concurrent connections doing 100 requests each");
    c.that(
        "concurrent client errors",
        "no client to fail",
        problems.is_empty(),
        problems.iter().take(5).cloned().collect::<Vec<_>>(),
    );
    c.eq("requests completed", total, done);
    c.at_most(
        "elapsed_ms",
        BUDGET.as_millis() as u64,
        elapsed.as_millis() as u64,
    );
    c.finish()
});

kafka_test!(latency, |ctx| {
    let mut conn = ctx.connect().await?;
    for _ in 0..WARMUP {
        conn.request(API_VERSIONS_V4, &api_versions_request())
            .await
            .map_err(|e| proto_fail(e, &conn).note("during warm-up"))?;
    }

    let sample = 2_000usize;
    let started = Instant::now();
    let mut latencies = Vec::with_capacity(sample);
    for i in 0..sample {
        if started.elapsed() > BUDGET {
            return Err(out_of_budget("latency sampling", i, sample, started));
        }
        let at = Instant::now();
        conn.request(API_VERSIONS_V4, &api_versions_request())
            .await
            .map_err(|e| proto_fail(e, &conn).note(format!("on request {i} of {sample}")))?;
        latencies.push(at.elapsed());
    }
    let elapsed = started.elapsed();
    drop(conn);
    ctx.note(format!(
        "{sample} sequential ApiVersions on one warm connection in {:.2} s ({}); latency {}",
        elapsed.as_secs_f64(),
        rate(sample, elapsed),
        percentiles(&mut latencies)
    ));
    ctx.note(format!(
        "{WARMUP} warm-up requests were discarded before measuring, so the JVM's interpreted \
         start is not in the numbers"
    ));
    let mut c = Check::detached("a 2 000 request latency sample");
    c.eq("requests measured", sample, latencies.len());
    c.at_most(
        "elapsed_ms",
        BUDGET.as_millis() as u64,
        elapsed.as_millis() as u64,
    );
    c.finish()
});

kafka_test!(survives, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let before = open_fds();

    // A short burst of everything the stage does, then the state checks.
    let started = Instant::now();
    for i in 0..300 {
        let mut conn = ctx.connect().await.map_err(|f| {
            f.note(format!(
                "on connection {i} of 300 in the file-descriptor check"
            ))
        })?;
        conn.request(API_VERSIONS_V4, &api_versions_request())
            .await
            .map_err(|e| proto_fail(e, &conn))?;
        drop(conn);
    }
    let after = open_fds();

    let mut c = Check::detached("our own file descriptors after 300 connect/close cycles");
    match (before, after) {
        (Some(b), Some(a)) => {
            // A handful of descriptors may legitimately move (log files, the runtime's own
            // eventfd); anything proportional to the 300 cycles is a leak in the harness.
            c.at_most("open file descriptors in the tester", b + 16, a);
            c.observe("open file descriptors before", b);
            c.observe("open file descriptors after", a);
        }
        _ => {
            ctx.note(
                "file descriptors were not counted: /proc/self/fd is not readable on this \
                 platform, so only the broker-side checks apply",
            );
        }
    }
    c.finish()?;
    if let (Some(b), Some(a)) = (before, after) {
        ctx.note(format!(
            "300 connect/close cycles in {:.2} s; this process held {b} file descriptors \
             before and {a} after",
            started.elapsed().as_secs_f64()
        ));
    }

    // The broker must still do real work, not just answer ApiVersions.
    let mut conn = ctx.connect().await?;
    let batch = RecordBatch::of(
        0,
        1_700_000_000_000,
        vec![RecordItem::value("after-the-soak")],
    )
    .encode();
    let req = produce_request(&[(t.name.clone(), 0, batch)], -1);
    let resp = conn
        .request(PRODUCE_V11, &req)
        .await
        .map_err(|e| proto_fail(e, &conn).note("producing one record after the soak"))?;
    let mut c = Check::new("one produce after the soak", &conn);
    c.eq(
        "response.responses[0].partition_responses[0].error_code",
        NONE,
        resp.responses
            .first()
            .and_then(|t| t.partition_responses.first())
            .map(|p| p.error_code)
            .unwrap_or(-1),
    );
    c.finish()?;
    drop(conn);
    expect_still_serving(ctx, "the soak").await
});

/// Worked examples: the shape of the soak, and what a healthy broker looks like at the end.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::text("10 000 records out and back")
            .request(
                "100 Produce v11 requests of 100 records each, round-robin over the 4 \
                 partitions of the fixture topic, every value the fixed string \
                 soak-<n:06>-kafkatest; then Fetch v16 from offset 0 of each partition, \
                 request after request, until all 10 000 records have come back",
            )
            .response(
                "Every record identical to what was produced, byte for byte and in order, and \
                 the offsets of each partition run 0..2500 with no gap and no repeat. Produce \
                 and fetch throughput and p50/p95/p99 batch latency are reported for \
                 information; the only hard limit is that the test finishes inside 60 s",
            )
            .note(
                "Volume is what finds the bugs a three-record test cannot: an index that is \
                 right until a segment rolls, a batch whose base offset is recomputed on \
                 write, a fetch that returns the same record twice at a batch boundary. A \
                 fetch that comes back with no new records is failed immediately rather than \
                 retried, because that is an off-by-one and not a slow broker.",
            ),
        ExampleSpec::text("1 000 connections, and the state left behind")
            .request(
                "After 200 warm-up requests: 1 000 connect / ApiVersions v4 / close cycles, \
                 each socket closed explicitly; then 50 concurrent connections making 100 \
                 requests each; then 300 more cycles with this process's own file \
                 descriptors counted before and after, and one last Produce",
            )
            .response(
                "Every cycle and all 5 000 concurrent requests succeed inside the 60 s \
                 budget, each connection's responses arrive in the order it asked for them, \
                 the file-descriptor count is unchanged apart from a handful, and the broker \
                 still accepts a connection and stores a record at the end — no leaked \
                 descriptors, no growing memory, no thread left holding a closed socket",
            )
            .note(
                "Close the socket when the client goes away, not when the process exits: a \
                 broker that leaks a descriptor or a thread per connection passes every \
                 functional stage and dies somewhere in the first few hundred cycles here. \
                 The latency percentiles are informational — they are printed under a passing \
                 test so a regression is visible before it becomes a failure.",
            ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_numbers_line_up() {
        assert_eq!(RECORDS % BATCH, 0);
        assert_eq!(RECORDS % PARTITIONS as usize, 0);
        assert_eq!(
            (RECORDS / BATCH) % PARTITIONS as usize,
            0,
            "round robin is even"
        );
    }

    #[test]
    fn percentiles_pick_the_right_sample() {
        let mut v: Vec<Duration> = (1..=100).map(Duration::from_millis).collect();
        assert_eq!(percentile(&v, 0.0), Duration::from_millis(1));
        // index = round((n - 1) * p): 99 * 0.5 rounds up to 50, so the 51st sample.
        assert_eq!(percentile(&v, 0.50), Duration::from_millis(51));
        assert_eq!(percentile(&v, 0.99), Duration::from_millis(99));
        assert_eq!(percentile(&v, 1.0), Duration::from_millis(100));
        assert!(percentiles(&mut v).contains("p99"));
        assert_eq!(percentile(&[], 0.5), Duration::ZERO);
    }

    #[test]
    fn rates_are_per_second() {
        assert_eq!(rate(1_000, Duration::from_secs(2)), "500/s");
    }

    #[test]
    fn record_values_are_distinct_and_stable() {
        assert_eq!(value_of(7), value_of(7));
        assert_ne!(value_of(7), value_of(8));
        assert!(value_of(0).starts_with(b"soak-000000"));
    }
}
