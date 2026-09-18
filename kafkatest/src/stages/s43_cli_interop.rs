//! Stage 43 — Interop with Kafka's own command line tools. **[ext]**
//!
//! Every test here drives a real JVM client: the scripts under `bin/` of the reference
//! tarball, pointed at the broker under test. They are the only tests in the suite that do
//! not speak the protocol themselves, which is exactly what makes them valuable — a broker
//! can satisfy a hand-written assertion and still trip over what `kafka-console-consumer.sh`
//! actually does.
//!
//! The tools are slow (a JVM start each), so every test raises its own timeout floor and the
//! whole stage carries the `slow` tag: `--tag slow` selects it, `--only` narrows it further.
//! When no distribution is available (`ctx.dist` is `None`) the test records why it did
//! nothing and passes, rather than failing for a reason that is not the broker's fault.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::fixtures::{rec, FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::proto::records::RecordBatch;
use crate::stages::{
    api_versions, await_high_watermark, expect_still_serving, fetch_request, proto_fail, Ctx,
    Stage, Test, FETCH_V16,
};
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

/// Hard limit on one tool invocation. A JVM client that has not finished by then is killed
/// and the test fails with whatever it had printed.
const TOOL_LIMIT: Duration = Duration::from_secs(30);

/// The three lines `kafka-console-producer.sh` is fed.
const PRODUCED_LINES: [&str; 3] = ["cli-one", "cli-two", "cli-three"];

/// The records the fixture topic starts with.
const FIXTURE_VALUES: [&str; 3] = ["fix-a", "fix-b", "fix-c"];

fn one_topic() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 2).with_batch(
        0,
        FIXTURE_VALUES.iter().map(|v| rec(*v)).collect::<Vec<_>>(),
    ))
}

fn empty_topic() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 1))
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 43,
        slug: "cli_interop",
        name: "Interop with Kafka's own command line tools",
        ext: true,
        hints: &[
            "kafka-topics.sh --describe exercises Metadata(3) and DescribeTopicPartitions(75) \
             together; the tool needs every partition to name a live leader",
            "kafka-console-producer.sh opens with ApiVersions and Metadata, then Produce; keep \
             acks=1 and enable.idempotence=false working before you add InitProducerId(22)",
            "kafka-console-consumer.sh needs FindCoordinator(10), the group protocol, \
             ListOffsets(2) and Fetch(1) — the whole consumer path in one command",
            "kafka-broker-api-versions.sh prints exactly what your ApiVersions response says, \
             so a key advertised there and unimplemented here is a lie a real client believes",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new(
                "kafka-topics.sh --describe reports the fixture topic and its partitions",
                topics_describe,
            )
            .with_fixtures(one_topic)
            .ext()
            .tag("slow")
            .min_timeout_ms(45_000),
            Test::new(
                "kafka-get-offsets.sh reports the end offset of the fixture partition",
                get_offsets,
            )
            .with_fixtures(one_topic)
            .ext()
            .tag("slow")
            .min_timeout_ms(45_000),
            Test::new(
                "kafka-console-producer.sh writes three records our Fetch can read",
                console_producer,
            )
            .with_fixtures(empty_topic)
            .ext()
            .tag("slow")
            .min_timeout_ms(45_000),
            Test::new(
                "kafka-console-consumer.sh prints the fixture records from the beginning",
                console_consumer,
            )
            .with_fixtures(one_topic)
            .ext()
            .tag("slow")
            .min_timeout_ms(45_000),
            Test::new(
                "kafka-broker-api-versions.sh agrees with our own ApiVersions response",
                broker_api_versions,
            )
            .ext()
            .tag("slow")
            .min_timeout_ms(45_000),
            Test::new(
                "the broker still serves after the command line tools have talked to it",
                still_serving,
            )
            .with_fixtures(one_topic)
            .ext()
            .tag("slow")
            .min_timeout_ms(45_000),
        ],
    }
}

// ---------------------------------------------------------------------------------------
// Running a tool
// ---------------------------------------------------------------------------------------

/// What one tool invocation produced.
struct ToolRun {
    tool: String,
    args: Vec<String>,
    stdout: String,
    stderr: String,
    ok: bool,
    took: Duration,
}

impl ToolRun {
    /// stdout and stderr, trimmed to something a terminal block can hold.
    fn output_block(&self) -> String {
        let cut = |label: &str, text: &str| {
            let text = text.trim_end();
            if text.is_empty() {
                return String::new();
            }
            let shown: String = text.chars().take(2000).collect();
            format!("{label}:\n{shown}\n")
        };
        format!(
            "{} {}\n{}{}",
            self.tool,
            self.args.join(" "),
            cut("stdout", &self.stdout),
            cut("stderr", &self.stderr)
        )
    }

    /// A check block that already carries the command and its output as context.
    fn check(&self, what: &str) -> Check {
        let mut c = Check::detached(what);
        c.note(self.output_block());
        c
    }

    /// Fail unless the tool exited 0.
    fn require_success(&self) -> Result<(), Failure> {
        if self.ok {
            return Ok(());
        }
        let mut c = self.check(&format!("that {} succeeds", self.tool));
        c.that(
            &format!("{}.exit_status", self.tool),
            "a zero exit status",
            false,
            "a non-zero exit status",
        );
        c.finish()
    }
}

/// The distribution to run tools from, or `None` with the reason recorded on the context.
fn dist_or_skip(ctx: &mut Ctx) -> Option<std::path::PathBuf> {
    match ctx.dist.clone() {
        Some(d) => Some(d),
        None => {
            ctx.note(
                "skipped: no Apache Kafka distribution is available (ctx.dist is None), so the \
                 command line tools cannot be run — install one by testing --broker \
                 apache_kafka once, or set KAFKATEST_CACHE",
            );
            None
        }
    }
}

/// Run one script from `bin/`, feeding it `stdin`, with a hard timeout.
///
/// `broker::reference::run_tool` is the synchronous version; a JVM client can block for
/// minutes, so this one goes through `tokio::process` (real timeout, `kill_on_drop` so no
/// JVM survives the test) and can write to the tool's stdin.
async fn tool(
    dist: &Path,
    name: &str,
    args: &[String],
    stdin: Option<&str>,
    limit: Duration,
) -> Result<ToolRun, Failure> {
    let path = dist.join("bin").join(name);
    let mut cmd = tokio::process::Command::new(&path);
    cmd.args(args)
        .env("KAFKA_HEAP_OPTS", "-Xmx320M -Xms64M")
        .env("KAFKA_JVM_PERFORMANCE_OPTS", "-XX:TieredStopAtLevel=1")
        .env("LOG_DIR", std::env::temp_dir())
        .stdin(if stdin.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let started = Instant::now();
    let mut child = cmd.spawn().map_err(|e| {
        Failure::harness(format!("cannot run {}: {e}", path.display())).note(
            "stage 43 needs the scripts from the unpacked Kafka tarball; see \
             broker::reference::ensure_installed",
        )
    })?;
    if let Some(data) = stdin {
        if let Some(mut pipe) = child.stdin.take() {
            let _ = pipe.write_all(data.as_bytes()).await;
            let _ = pipe.shutdown().await;
        }
    }
    let describe = format!("{name} {}", args.join(" "));
    // `kill_on_drop` turns the timeout into a kill: the future owns the child, and dropping
    // it on the timeout path sends SIGKILL, so no JVM outlives the test.
    let out = match tokio::time::timeout(limit, child.wait_with_output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return Err(Failure::harness(format!("{describe} failed: {e}"))),
        Err(_) => {
            return Err(Failure::harness(format!(
                "{describe} did not finish within {} s and was killed",
                limit.as_secs()
            ))
            .note("a real client hanging on the broker is a broker bug, not a slow JVM"))
        }
    };
    Ok(ToolRun {
        tool: name.to_string(),
        args: args.to_vec(),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        ok: out.status.success(),
        took: started.elapsed(),
    })
}

/// `--bootstrap-server <host:port>`, the first two arguments of every tool here.
fn bootstrap(ctx: &Ctx) -> Vec<String> {
    vec!["--bootstrap-server".to_string(), ctx.addr.to_string()]
}

fn args(base: Vec<String>, rest: &[&str]) -> Vec<String> {
    let mut v = base;
    v.extend(rest.iter().map(|s| s.to_string()));
    v
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

kafka_test!(topics_describe, |ctx| {
    let Some(dist) = dist_or_skip(ctx) else {
        return Ok(());
    };
    let t = ctx.topic("t1")?.clone();
    let run = tool(
        &dist,
        "kafka-topics.sh",
        &args(bootstrap(ctx), &["--describe", "--topic", &t.name]),
        None,
        TOOL_LIMIT,
    )
    .await?;
    ctx.note(format!(
        "kafka-topics.sh --describe took {} ms",
        run.took.as_millis()
    ));
    run.require_success()?;

    let mut c = run.check("what kafka-topics.sh --describe printed");
    c.that(
        "kafka-topics.sh stdout",
        &format!("a line naming the topic '{}'", t.name),
        run.stdout.contains(&t.name),
        run.stdout.lines().next().unwrap_or("(no output)"),
    );
    c.that(
        "kafka-topics.sh stdout 'PartitionCount'",
        &format!("PartitionCount: {}", t.partitions),
        run.stdout
            .contains(&format!("PartitionCount: {}", t.partitions)),
        run.stdout
            .lines()
            .find(|l| l.contains("PartitionCount"))
            .unwrap_or("(no PartitionCount line)"),
    );
    // One indented "Partition: n" line per partition; the tool prints them from Metadata.
    let per_partition = run
        .stdout
        .lines()
        .filter(|l| l.contains("Partition:"))
        .count();
    c.eq(
        "kafka-topics.sh per-partition lines",
        t.partitions as usize,
        per_partition,
    );
    c.finish()
});

kafka_test!(get_offsets, |ctx| {
    let Some(dist) = dist_or_skip(ctx) else {
        return Ok(());
    };
    let t = ctx.topic("t1")?.clone();
    let run = tool(
        &dist,
        "kafka-get-offsets.sh",
        &args(bootstrap(ctx), &["--topic", &t.name]),
        None,
        TOOL_LIMIT,
    )
    .await?;
    ctx.note(format!(
        "kafka-get-offsets.sh took {} ms",
        run.took.as_millis()
    ));
    run.require_success()?;

    // The tool prints `<topic>:<partition>:<offset>`, one line per partition.
    let mut seen: Vec<(i32, i64)> = Vec::new();
    for line in run.stdout.lines() {
        let parts: Vec<&str> = line.trim().rsplitn(3, ':').collect();
        if parts.len() == 3 && parts[2] == t.name {
            if let (Ok(p), Ok(o)) = (parts[1].parse::<i32>(), parts[0].parse::<i64>()) {
                seen.push((p, o));
            }
        }
    }
    seen.sort_unstable();
    let mut c = run.check("the end offsets kafka-get-offsets.sh reports");
    c.that(
        "kafka-get-offsets.sh stdout",
        "one <topic>:<partition>:<offset> line per partition",
        seen.len() == t.partitions as usize,
        run.stdout.trim().to_string(),
    );
    if let Some((_, offset)) = seen.iter().find(|(p, _)| *p == 0) {
        c.eq(
            &format!("kafka-get-offsets.sh {}:0 end offset", t.name),
            FIXTURE_VALUES.len() as i64,
            *offset,
        );
    }
    if let Some((_, offset)) = seen.iter().find(|(p, _)| *p == 1) {
        c.eq(
            &format!("kafka-get-offsets.sh {}:1 end offset", t.name),
            0i64,
            *offset,
        );
    }
    c.finish()
});

kafka_test!(console_producer, |ctx| {
    let Some(dist) = dist_or_skip(ctx) else {
        return Ok(());
    };
    let t = ctx.topic("t1")?.clone();
    // acks=1 and no idempotence keep the producer off InitProducerId(22), so this test only
    // needs ApiVersions + Metadata + Produce — the path a young broker has.
    let run = tool(
        &dist,
        "kafka-console-producer.sh",
        &args(
            bootstrap(ctx),
            &[
                "--topic",
                &t.name,
                "--producer-property",
                "acks=1",
                "--producer-property",
                "enable.idempotence=false",
                "--producer-property",
                "max.block.ms=20000",
            ],
        ),
        Some(&format!("{}\n", PRODUCED_LINES.join("\n"))),
        TOOL_LIMIT,
    )
    .await?;
    ctx.note(format!(
        "kafka-console-producer.sh took {} ms for {} records",
        run.took.as_millis(),
        PRODUCED_LINES.len()
    ));
    run.require_success()?;

    await_high_watermark(ctx, &t, 0, PRODUCED_LINES.len() as i64)
        .await
        .map_err(|f| f.note(run.output_block()))?;

    let mut conn = ctx.connect().await?;
    let req = fetch_request(&[(t.id, 0, 0)], 1_000);
    let resp = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let bytes = resp
        .responses
        .first()
        .and_then(|r| r.partitions.first())
        .and_then(|p| p.records.clone())
        .map(|b| b.to_vec())
        .unwrap_or_default();
    let batches = RecordBatch::decode_all(&bytes).map_err(|e| {
        Failure::harness(format!(
            "the records kafka-console-producer.sh wrote do not decode: {e:#}"
        ))
    })?;
    let values: Vec<String> = batches
        .iter()
        .flat_map(|b| b.records.iter())
        .map(|r| String::from_utf8_lossy(r.value.as_deref().unwrap_or_default()).to_string())
        .collect();
    let mut c = Check::new(
        "the records a real producer wrote, read back with Fetch",
        &conn,
    );
    c.note(run.output_block());
    c.eq(
        "records[*].value",
        PRODUCED_LINES.iter().map(|s| s.to_string()).collect(),
        values,
    );
    c.finish()
});

kafka_test!(console_consumer, |ctx| {
    let Some(dist) = dist_or_skip(ctx) else {
        return Ok(());
    };
    let t = ctx.topic("t1")?.clone();
    let group = format!("kafkatest-{}", ctx.unique("g"));
    let run = tool(
        &dist,
        "kafka-console-consumer.sh",
        &args(
            bootstrap(ctx),
            &[
                "--topic",
                &t.name,
                "--from-beginning",
                "--max-messages",
                "3",
                "--timeout-ms",
                "15000",
                "--group",
                &group,
            ],
        ),
        None,
        TOOL_LIMIT,
    )
    .await?;
    ctx.note(format!(
        "kafka-console-consumer.sh took {} ms to read {} records",
        run.took.as_millis(),
        FIXTURE_VALUES.len()
    ));
    run.require_success()?;

    let printed: Vec<String> = run
        .stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    let mut c = run.check("what kafka-console-consumer.sh printed");
    c.eq(
        "kafka-console-consumer.sh stdout lines",
        FIXTURE_VALUES.iter().map(|s| s.to_string()).collect(),
        printed,
    );
    c.finish()
});

kafka_test!(broker_api_versions, |ctx| {
    let Some(dist) = dist_or_skip(ctx) else {
        return Ok(());
    };
    let run = tool(
        &dist,
        "kafka-broker-api-versions.sh",
        &args(bootstrap(ctx), &[]),
        None,
        TOOL_LIMIT,
    )
    .await?;
    ctx.note(format!(
        "kafka-broker-api-versions.sh took {} ms",
        run.took.as_millis()
    ));
    run.require_success()?;

    // Every supported entry is printed as `Name(key): lo to hi [usable: n]`. The tool also
    // lists every api key *it* knows and the broker did not send, as `Name(key): UNSUPPORTED`
    // — those say nothing about the broker, so they are filtered out here.
    let mut printed: Vec<i16> = Vec::new();
    let mut unsupported = 0usize;
    for line in run.stdout.lines() {
        if line.contains("UNSUPPORTED") || line.contains("UNASSIGNED") {
            unsupported += 1;
            continue;
        }
        let Some(open) = line.find('(') else { continue };
        let Some(close) = line[open..].find("):") else {
            continue;
        };
        if let Ok(key) = line[open + 1..open + close].trim().parse::<i16>() {
            printed.push(key);
        }
    }
    printed.sort_unstable();
    printed.dedup();

    let mut conn = ctx.connect().await?;
    let ours = api_versions(&mut conn).await?;
    let mut advertised: Vec<i16> = ours.api_keys.iter().map(|k| k.api_key).collect();
    advertised.sort_unstable();

    let missing: Vec<i16> = printed
        .iter()
        .copied()
        .filter(|k| !advertised.contains(k))
        .collect();
    let mut c = Check::new(
        "that kafka-broker-api-versions.sh and our ApiVersions list the same api keys",
        &conn,
    );
    c.note(run.output_block());
    c.that(
        "kafka-broker-api-versions.sh stdout",
        "at least ApiVersions(18) in the printed list",
        printed.contains(&18),
        printed.clone(),
    );
    c.that(
        "api keys printed by the tool but absent from response.api_keys",
        "no such key: the tool prints what the broker itself sent",
        missing.is_empty(),
        missing,
    );
    c.observe("response.api_keys.len", advertised.len());
    c.observe("kafka-broker-api-versions.sh key count", printed.len());
    c.finish()?;
    ctx.note(format!(
        "broker advertises {} api keys; the tool printed {} of them as supported and {} as \
         UNSUPPORTED (api keys the tool knows and the broker does not offer)",
        advertised.len(),
        printed.len(),
        unsupported
    ));
    Ok(())
});

kafka_test!(still_serving, |ctx| {
    let Some(dist) = dist_or_skip(ctx) else {
        return Ok(());
    };
    let t = ctx.topic("t1")?.clone();
    // One more tool round trip, then the protocol path again: a broker that mishandles a
    // real client's connection teardown often stops accepting afterwards.
    let run = tool(
        &dist,
        "kafka-topics.sh",
        &args(bootstrap(ctx), &["--list"]),
        None,
        TOOL_LIMIT,
    )
    .await?;
    ctx.note(format!(
        "kafka-topics.sh --list took {} ms",
        run.took.as_millis()
    ));
    run.require_success()?;
    let mut c = run.check("that kafka-topics.sh --list names the fixture topic");
    c.that(
        "kafka-topics.sh --list stdout",
        &format!("a line equal to '{}'", t.name),
        run.stdout.lines().any(|l| l.trim() == t.name),
        run.stdout.trim().to_string(),
    );
    c.finish()?;
    expect_still_serving(ctx, "Kafka's own command line tools used the broker").await
});

/// Worked examples: the commands this stage runs, and what a correct broker makes them print.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::text("kafka-topics.sh --describe")
            .request(
                "bin/kafka-topics.sh --bootstrap-server <host>:<port> --describe --topic \
                 t1-<run id>, against a fixture topic with 2 partitions and replication \
                 factor 1 — the JVM tool sends ApiVersions, then Metadata(3) and \
                 DescribeTopicPartitions(75)",
            )
            .response(
                "Exit status 0 and a header line 'Topic: t1-<run id>  TopicId: <the real \
                 uuid>  PartitionCount: 2  ReplicationFactor: 1  Configs: ...', then one \
                 indented line per partition: 'Topic: t1-<run id>  Partition: 0  Leader: \
                 <node id>  Replicas: <node id>  Isr: <node id>' (Kafka 4.x adds empty Elr \
                 and LastKnownElr columns). The stage asserts the topic name, the \
                 'PartitionCount: 2' line, and exactly one 'Partition:' line per partition",
            )
            .note(
                "The tool prints what it parsed, so every field here is a claim your broker \
                 made. A partition whose leader is -1, or a replicas/isr list that does not \
                 name a live broker, makes the tool print a partition it will then refuse to \
                 produce to — the most common way a hand-written Metadata response passes a \
                 unit test and fails a real client.",
            ),
        ExampleSpec::text("kafka-console-producer.sh and kafka-console-consumer.sh")
            .request(
                "bin/kafka-console-producer.sh --bootstrap-server <host>:<port> --topic \
                 t1-<run id> --producer-property acks=1 --producer-property \
                 enable.idempotence=false --producer-property max.block.ms=20000, fed \
                 'cli-one', 'cli-two', 'cli-three' on stdin; then bin/kafka-console-consumer.sh \
                 --bootstrap-server <host>:<port> --topic t1-<run id> --from-beginning \
                 --max-messages 3 --timeout-ms 15000 --group kafkatest-<run id> on a fixture \
                 topic holding 'fix-a', 'fix-b', 'fix-c'",
            )
            .response(
                "Both exit 0. The producer prints nothing; the suite then reads partition 0 \
                 with its own Fetch and finds exactly cli-one, cli-two, cli-three, in order. \
                 The consumer prints fix-a, fix-b and fix-c, one per line, and stops at \
                 --max-messages 3",
            )
            .note(
                "acks=1 with enable.idempotence=false keeps the producer off \
                 InitProducerId(22), so this exercises ApiVersions, Metadata and Produce and \
                 nothing else. The consumer is the opposite: FindCoordinator(10), the whole \
                 JoinGroup/SyncGroup/Heartbeat dance, ListOffsets(2), Fetch(1) and \
                 OffsetCommit(8) in one command — it fails if any one of stages 38 to 42 is \
                 wrong.",
            ),
        ExampleSpec::text("kafka-broker-api-versions.sh")
            .request("bin/kafka-broker-api-versions.sh --bootstrap-server <host>:<port>")
            .response(
                "Exit status 0 and one block per broker, '<host>:<port> (id: <node id> rack: \
                 null) -> (', then a line per api key the broker advertised — 'Produce(0): 0 \
                 to 11 [usable: 11]', 'ApiVersions(18): 0 to 4 [usable: 4]', and so on. Keys \
                 the tool knows and the broker did not send are printed as UNSUPPORTED. The \
                 stage asserts that ApiVersions(18) is among the supported lines and that \
                 every key printed there is also in our own ApiVersions response",
            )
            .note(
                "This tool is a mirror: it prints your ApiVersions response and nothing else. \
                 An api key advertised here and not implemented is a lie a real client \
                 believes — it will pick the highest version you claim and hang or crash on \
                 the answer. Advertise only what you serve.",
            ),
    ]
}
