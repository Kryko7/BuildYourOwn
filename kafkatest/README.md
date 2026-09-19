# kafkatest

A self-contained, stage-by-stage conformance tester for Kafka brokers, written in Rust.
It drives **any** broker binary as a black box over TCP — real frames, real record batches,
real on-disk fixtures — and compares what comes back with a suite written in Rust: 47 stages,
315 tests, all validated against Apache Kafka 4.1.2 running in KRaft mode. Every stage also
carries worked examples — 91 of them — showing the exact bytes a broker receives and the exact
bytes Apache Kafka answered, annotated field by field (see [Examples](#examples)).

```
cargo build --release
./target/release/kafkatest --broker apache_kafka --validate --all   # suite self-check, all green
./target/release/kafkatest --broker ./your_program.sh --stage 1     # your first red/green
./target/release/kafkatest --broker my_broker --until 12
./target/release/kafkatest --list                                   # stages, counts, PLAN.md tickboxes
./target/release/kafkatest --list --json catalog.json               # stage catalog for the site
```

The reference broker is downloaded once (~130 MB) into `~/.cache/kafkatest` and run on random
free ports. No Docker, no accounts. Linux and macOS, Java 17+ for the reference broker only.

## What a run looks like

```
Stage 02 Respond with the correlation id
  ✘ the correlation id comes back unchanged FAIL (72 ms)
      response.correlation_id: expected 1867125491, got 1867125492
    ┌ context
    │ while checking that correlation id 1867125491 is echoed
    ┌ expected
    │ response.correlation_id: 1867125491
    ┌ actual
    │ response.correlation_id: 1867125492
    ┌ request bytes (37 bytes, length prefix omitted)
    │ 0000  00 12 00 04 6f 4a 12 f3  00 09 6b 61 66 6b 61 74  |....oJ....kafkat|
    │ 0010  65 73 74 00 0a 6b 61 66  6b 61 74 65 73 74 06 30  |est..kafkatest.0|
    │ 0020  2e 31 2e 30 00                                    |.1.0.           |
    ┌ response bytes (6 bytes, ^^ marks the bytes holding the actual value)
    │ 0000  6f 4a 12 f4 00 07                                 |oJ....          |
    │       ^^ ^^ ^^ ^^
    ┌ broker output
    │ broker stdout (last 1 lines):
    │   broken_broker: listening on 0.0.0.0:41167 (and answering nonsense)
  ✔ the response is at least a 4-byte header (72 ms)
Stage 02  Respond with the correlation id              1/6 passed
```

Every failure names a **decoded field path** (`response.topics[0].error_code`), hex-dumps the
request and the response with the interesting bytes marked, and shows the tail of the broker's
own stdout/stderr. A broker that dies mid-test is its own failure kind:

```
  ✘ the record can be fetched back FAIL [broker crash] (91 ms)
      broker 'my_broker' exited with signal 11 during the test
```

To see the red path without writing a broker:

```
cargo build --release --example broken_broker
./target/release/kafkatest --broker target/release/examples/broken_broker --until 5
```

## CLI

```
kafkatest --broker <name|path> [--stage N] [--until N] [--from N] [--all]
          [--only "substring"] [--tag ext] [--skip-ext]
          [--verbose] [--keep-tmp] [--timeout-ms N] [--seed N]
          [--validate] [--list] [--json report.json] [--no-color]
          [--brokers-file path] [--port N] [--log-dir path]
          [--capture-examples examples/captured.json]
```

| Flag | Meaning |
|---|---|
| `--broker` | A name from `brokers.yaml` (`apache_kafka`, `my_broker`) **or a path**. A path is run as `<path> <server.properties>` with `files` fixtures and a fresh process per test. |
| `--stage N` / `--until N` / `--from N` / `--all` | Stage selection. Gaps are fine: `--until 20` runs the implemented stages up to 20. |
| `--only SUBSTR` | Only tests whose name contains the substring. |
| `--tag ext` | Only tests carrying a tag; `--skip-ext` hides `ext` tests (everything beyond the core track). |
| `--validate` | Run against a *registered* reference broker and word failures as suite bugs. `kafkatest --broker apache_kafka --validate --all` must be all green. |
| `--verbose` | Print the fixture topics and ids the harness created before each test. |
| `--keep-tmp` | Keep every broker instance's temp dir (log dir, `server.properties`, captured output) and print the paths. |
| `--timeout-ms N` | Per-test timeout, default 10 000. Broker boot is not counted. |
| `--seed N` | Seeds every random choice: correlation ids, interleavings, fuzz payloads, derived topic ids. The same seed reproduces the same run. |
| `--port N` | Force the broker's listener port. Refused for the reference broker, which always takes a random one. |
| `--log-dir DIR` | Force the log directory fixtures are written into. |
| `--json FILE` | Machine-readable report (schema below). With `--list`, writes the stage catalog instead; `--list --json` with no value prints the catalog on stdout. |
| `--list` | Stages, source files, test counts and the `PLAN.md` tickbox state. |
| `--no-color` | No ANSI. `NO_COLOR` in the environment does the same. |
| `--brokers-file` | Path to `brokers.yaml`; found next to the cwd or the binary by default. |
| `--capture-examples FILE` | Send every stage's worked examples to the broker and write the request/response bytes and their field annotations to `FILE` (see [Examples](#examples)). Runs no tests. |

Exit code is **0** only when every selected test passed, **1** when something failed, **2** on a
usage or harness error (unknown broker, no stage selected, broker will not start).

## Conventions your broker must follow

The suite encodes the "build your own Kafka" conventions. Where real Kafka and a
hand-written broker legitimately differ, the expectation is loosened so both pass, and the
looser check says why in its `context` block.

| Thing | Convention |
|---|---|
| Startup | `./your_program.sh /path/to/server.properties` — argv[1] is a properties file the harness writes. |
| Listener | `0.0.0.0:9092` by default; the harness reads the port from `listeners=PLAINTEXT://0.0.0.0:<port>` in the properties file, and passes it as `{PORT}` if your `brokers.yaml` entry asks for it. **Never hardcode 9092 in the tester**: other agents run brokers on the same machine, so the reference broker always takes a random free port. |
| Log directory | `log.dirs=<dir>` in the properties file (`/tmp/kraft-combined-logs` by default). The harness writes the fixtures there *before* your process starts, so read them at startup. |
| On-disk format | `__cluster_metadata-0/00000000000000000000.log` (FeatureLevelRecord, TopicRecord, PartitionRecord), `<topic>-<partition>/00000000000000000000.log` (RecordBatch v2 with CRC-32C), `partition.metadata`, `meta.properties`. See "Fixtures" below. |
| Framing | 4-byte big-endian size, then that many bytes. Responses the same way. Never assume one `read()` is one request. |
| Request header | v2 (flexible) for every API this suite uses except where noted: `api_key(int16) api_version(int16) correlation_id(int32) client_id(nullable string) tagged_fields`. `client_id` stays a *plain* string even in flexible versions. Unknown tagged fields must be skipped, not rejected. |
| Response header | v1 (`correlation_id` + tagged fields) for flexible APIs — **except ApiVersions, whose response header is always v0** (no tagged fields), because a client has to parse it before it knows what the broker supports. |
| Error codes | `UNSUPPORTED_VERSION` 35, `UNKNOWN_TOPIC_OR_PARTITION` 3, `UNKNOWN_TOPIC_ID` 100, `TOPIC_ALREADY_EXISTS` 36, `OFFSET_OUT_OF_RANGE` 1, `CORRUPT_MESSAGE` 2, `MESSAGE_TOO_LARGE` 10, `INVALID_REQUIRED_ACKS` 21, `OUT_OF_ORDER_SEQUENCE_NUMBER` 45, `DUPLICATE_SEQUENCE_NUMBER` 46, `INVALID_PRODUCER_EPOCH` 47, `INVALID_RECORD` 87. |
| Message size | Both generated properties files carry `message.max.bytes=65536`; a batch bigger than that is `MESSAGE_TOO_LARGE` (10), per partition (stage 37). |
| API versions | ApiVersions **v4**, DescribeTopicPartitions(75) **v0**, Fetch(1) **v16**, Produce(0) **v11**, Metadata(3) **v12**, CreateTopics(19) **v7**. |
| Robustness | A malformed frame closes *that* connection at worst. The process must survive, the accept loop must keep running, and other connections must keep working. |

## The reference broker

`--broker apache_kafka` is Apache Kafka itself, and it is what `--validate` proves the suite
against. The harness:

1. Downloads `kafka_2.13-4.1.2.tgz` from `archive.apache.org` into `~/.cache/kafkatest`
   (override with `KAFKATEST_CACHE`), behind an `flock`ed lock file so parallel runs share one
   copy, and checks it against the published `.sha512` before unpacking.
2. Asks `kafka-storage.sh random-uuid` for a cluster id, once per process.
3. Writes a KRaft combined-mode `server.properties` (broker + controller in one process,
   single replica, `auto.create.topics.enable=false`) with **two random free ports**, one for
   the listener and one for the controller quorum.
4. Runs `kafka-storage.sh format` over it, then `kafka-server-start.sh`, with a small heap and
   `-XX:TieredStopAtLevel=1` so boot is ~4 s.
5. Waits for the port *and* for `Kafka Server started` in the broker's stdout, because Kafka
   accepts TCP connections well before it can serve them.

The process is started in its own process group and killed as a group (`SIGTERM`, then
`SIGKILL`) when the handle drops, so a wrapper script never leaves a JVM behind. After a run,
`pgrep -f kafka_2.13` returns nothing.

The reference broker restarts **per stage** (boot is the slow part; tests inside a stage use
unique topic names). A broker with `files` fixtures restarts **per test**, because those
fixtures are read at startup.

## Fixtures

A test declares the topics and records it needs; the harness materializes them and hands back a
`FixtureHandle` with the real names, ids and offsets. **Tests never hardcode a topic id.**

```rust
fn one_record() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 1).with_batch(0, vec![rec("hello kafkatest")]))
        .and(TopicSpec::new("t2", 2))
}
```

Two strategies produce the same state:

- **`files`** (your broker) writes Kafka's on-disk format into the log directory before the
  broker starts:

  ```
  <log.dirs>/
    meta.properties
    __cluster_metadata-0/00000000000000000000.log   FeatureLevelRecord(metadata.version=27),
                                                    then TopicRecord + PartitionRecord per topic
    __cluster_metadata-0/partition.metadata
    <topic>-<partition>/00000000000000000000.log    one RecordBatch v2 per declared batch
    <topic>-<partition>/partition.metadata
    <topic>-<partition>/leader-epoch-checkpoint
  ```

  The topic ids are derived from `--seed` plus the topic name, so a run is reproducible.
  `cargo test` proves the bytes are real by running `kafka-dump-log.sh --deep-iteration` and
  `--cluster-metadata-decoder` from the tarball over them (skipped if the tarball is absent).

- **`api`** (the reference broker) creates the same state with `CreateTopics` v7 + `Produce`
  v11 and discovers the ids with `Metadata` v12. It then waits until every partition answers a
  `Fetch` without `NOT_LEADER_OR_FOLLOWER`, because Kafka's controller commits the partition
  record before the broker has created the local log.

A batch can be declared **compressed**, and both strategies honour it — `files` writes a
compressed batch into the segment, `api` produces one, which a broker running with the default
`compression.type=producer` stores byte for byte (stage 27):

```rust
TopicSpec::new("t1", 1)
    .with_compressed_batch(0, records::GZIP, vec![rec("a"), rec("b")])
    .with_compressed_batch(0, records::ZSTD, vec![rec("c")])
```

`RecordBatch::encode`/`decode` do the compression themselves, in the framing Kafka expects
(xerial-framed snappy, the LZ4 *frame* format, plain gzip and zstd streams);
`tests/compressed_segments.rs` proves it with `kafka-dump-log.sh --deep-iteration`.

Topic names are unique per test (`t1-s12x37`), so a per-stage broker can host many tests.

## JSON report

`--json report.json` writes the same schema `shelltest` writes, with `target` where `shelltest`
has `shell`, so one parser reads both tracks:

```jsonc
{
  "target": "apache_kafka",
  "validate": true,
  "passed": 296, "failed": 0, "skipped": 0, "elapsed_ms": 82134,
  "stages": [
    {
      "stage": 2, "name": "Respond with the correlation id",
      "file": "src/stages/s02_correlation_id.rs",
      "passed": 6, "failed": 0, "skipped": 0,
      "tests": [
        {
          "name": "the correlation id comes back unchanged",
          "status": "pass",              // "pass" | "fail" | "skip"
          "ext": false,
          "duration_ms": 4,
          "failures": [],                // one line per failed check
          "skip_reason": null,
          "actual": [],                  // [field path, value] pairs that were seen
          "notes": [],                   // kafkatest only: ctx.note(..) lines, pass or fail
          "failure_kind": null           // kafkatest only: assertion | timeout | connection |
                                         // protocol | broker_crash | harness
        }
      ]
    }
  ]
}
```

`catalog.json` (`--list --json`) is the file the site consumes:

```jsonc
{
  "track": "kafka",
  "generatedAt": "2026-09-12T20:13:31Z",
  "sections": [{ "id": "a", "title": "Bootstrap & framing", "stages": [1,2,3,4,5,6,7,8,9] }],
  "stages": [{
    "number": 1, "slug": "bind", "name": "Bind to the broker port",
    "ext": false, "file": "src/stages/s01_bind.rs",
    "hints": ["Create a TCP listener on 0.0.0.0:9092 and accept connections in a loop", "..."],
    "tests": [{ "name": "the broker accepts a TCP connection" }],  // ext/tags/skipOn when set
    "examples": [{ "title": "...", "kind": "wire", "request": "...", "request_hex": "...",
                   "response": "...", "response_hex": "...", "note": "...",
                   "request_fields": [{ "offset": 0, "length": 4, "field": "size",
                                        "value": "37 (bytes that follow)" }],
                   "response_fields": [ /* ... */ ] }]   // see "Examples" below
  }]
}
```

`sections` lists **all 45 planned stages** whether or not they are implemented, so the site can
draw the whole journey; `stages` holds only the implemented ones. `catalog.json` is committed
and `cargo test` fails if it is stale.

## Examples

Every stage carries one to three **worked examples**: the exact bytes a broker receives and
the exact bytes Apache Kafka 4.1.2 answered, with every field annotated, so a learner can see
what the stage is about before writing a line of code. They live next to the stage's hints in
`catalog.json` and are what the site renders in its wire inspector.

```
./target/release/kafkatest --capture-examples examples/captured.json --broker apache_kafka
./target/release/kafkatest --list --json catalog.json     # merges them into the catalog
```

An example is declared in the stage's own file, and its request is **built with the codec the
tests use** — never typed as hex — so it cannot drift away from the suite:

```rust
fn wire_examples() -> Vec<ExampleSpec> {
    vec![ExampleSpec::wire("ApiVersions v4, the whole exchange", |env| {
        env.request(API_VERSIONS_V4, 7, &api_versions_request())
    })
    .request("ApiVersions v4, correlation id 7, client id 'kafkatest'")
    .response("error_code 0 (NONE), then a compact array of api_keys, then throttle_time_ms 0")
    .note("The response header of ApiVersions is v0 at every version, because a client must \
           parse this answer before it knows what the broker supports.")]
}
```

| | |
|---|---|
| `ExampleSpec::wire(title, build)` | `build` is a non-capturing `fn(&ExampleEnv) -> Result<Wire, String>`. `Wire::Frames(vec![payload, ..])` gets a 4-byte size prefix per frame; `Wire::Raw(bytes)` goes out exactly as given (stages 9 and 44). |
| `ExampleSpec::text(title)` | No wire exchange at all — stage 1 (connect and say nothing), 7 (concurrency), 43 (a `kafka-topics.sh` command line and its stdout), 45 (the soak). `request`/`response` carry the whole example and both `*_hex` are empty. |
| `.with_fixtures(f)` | The same `FixtureSpec` the tests use. The capture materializes it on the reference broker first, so `env.name("t1")` and `env.id("t1")` are a real topic name and a real topic id. |
| `.expect_silence()` / `.expect_closed()` | A correct broker answers nothing (`acks=0`), or closes the connection (a negative size prefix). |
| `.response_version(v)` | Decode the answer as another version — an `UNSUPPORTED_VERSION` reply to ApiVersions is written in the v0 shape. |
| `env.request(version, correlation_id, &req)` | One frame, header and body, encoded by `kafka-protocol`. `env.payload(..)` returns the payload alone, for multi-frame examples. |

Correlation ids are fixed per example (`stage number × 10 + index`), the client id is always
`kafkatest`, and fixture topic names are derived from the stage (`t1-ex231`), so a recapture
changes as little as possible. Topic ids, cluster ids, member ids, producer ids, timestamps
and CRCs *do* change on every boot: those annotations carry `"varies": true` so the site can
say so instead of pretending the value is fixed.

### The JSON the site reads

`catalog.json` gains an `examples` array per stage (snake_case inside the example objects,
camelCase around them, as before):

```jsonc
"examples": [{
  "title": "ApiVersions v4, the whole exchange",
  "kind": "wire",                      // wire | silence | closed | text
  "request": "ApiVersions v4, correlation id 7, client id 'kafkatest'",
  "request_hex": "00000025001200040000000700096b61666b6174657374...",
  "response": "error_code 0 (NONE), then a compact array of api_keys, ...",
  "response_hex": "000003120000000700004a00000000000d0000010004...",
  "note": "The response header of ApiVersions is v0 at every version, ...",
  "request_fields": [                  // annotations for request_hex
    { "offset": 0,  "length": 4,  "field": "size",                 "value": "37 (bytes that follow)" },
    { "offset": 4,  "length": 2,  "field": "header.api_key",       "value": "18 (ApiVersions)" },
    { "offset": 6,  "length": 2,  "field": "header.api_version",   "value": "4" },
    { "offset": 8,  "length": 4,  "field": "header.correlation_id","value": "7" },
    { "offset": 12, "length": 11, "field": "header.client_id",     "value": "'kafkatest'" },
    { "offset": 23, "length": 1,  "field": "header.tagged_fields", "value": "0 tagged fields" },
    { "offset": 24, "length": 10, "field": "body.client_software_name", "value": "'kafkatest'" }
  ],
  "response_fields": [                 // annotations for response_hex, same shape
    { "offset": 8,  "length": 2, "field": "body.error_code",         "value": "0 (NONE)" },
    { "offset": 10, "length": 1, "field": "body.api_keys.length",    "value": "73 entries" },
    { "offset": 11, "length": 2, "field": "body.api_keys[0].api_key","value": "0 (Produce)" }
  ],
  "env": {                             // the fixtures the bytes refer to, when there are any
    "topics": { "t1": { "name": "t1-ex231", "id": "0f3d…", "partitions": 1 } },
    "group": "kafkatest-group"
  }
}]
```

- Offsets are byte offsets **into the hex string's bytes, size prefix included**: offset 0 is
  the first byte of the 4-byte length prefix, so the request header starts at offset 4.
- `field` is a dotted path: `size`, `header.*`, `body.*`, with array indices
  (`body.topics[0].partitions[0].error_code`) and record batches
  (`body.responses[0].partitions[0].records.batch[0].base_offset`). A multi-frame example
  prefixes every path with `frame0.` / `frame1.`.
- Arrays annotate their length and then **only the first element**; the rest are walked so the
  offsets stay right, but the site does not need seventy copies of the same schema.
- `"varies": true` marks a value that is different on every boot; it is omitted when false.
- `kind` says whether there are response bytes at all: `silence` and `closed` examples have an
  empty `response_hex` on purpose, and `text` examples have no bytes at all.

### Recapturing

The words (`title`, `request`, `response`, `note`) come from the stage file at `--list` time,
so editing a summary needs no broker. The bytes and the annotations come from
`examples/captured.json`, so **anything that moves a byte needs a recapture**:

```
cargo build --release
./target/release/kafkatest --capture-examples examples/captured.json --broker apache_kafka
./target/release/kafkatest --list --json catalog.json
cargo test                                   # tests/examples_are_current.rs proves both files
```

The capture boots the reference broker once, warms up the two pieces of state a freshly
formatted Kafka creates lazily (the group coordinator's `__consumer_offsets`, and the first
block of producer ids), materializes each example's fixtures through the `api` strategy, sends
the bytes and reads the answer. Without that warm-up stages 36 and 39 would record
`COORDINATOR_LOAD_IN_PROGRESS` (14) and `COORDINATOR_NOT_AVAILABLE` (15) — what a client sees
in the first second of a broker's life and retries through, not what those stages are about. It also walks both byte strings against
the schemas in `src/examples/schema_*.rs`, and **fails if a walk does not land exactly on the
end of a frame** — that is the check that keeps the annotations honest, and it is why adding
an example for an API the annotator does not know yet means teaching it that API first.

## Registering a broker

`brokers.yaml`:

```yaml
brokers:
  apache_kafka:            # the reference, and the --validate target
    kind: reference
    version: "4.1.2"
    fixtures: api
    restart: per_stage
    boot_timeout_ms: 60000
  my_broker:
    command: ["./your_program.sh", "{PROPS}"]
    port: 9092             # omit (or 0) to let the harness pick a free port
    log_dir: /tmp/kraft-combined-logs
    fixtures: files        # files | api
    restart: per_test      # per_test | per_stage (files always implies per_test)
    cwd: "."
    env: { RUST_LOG: "debug" }
```

Placeholders substituted in `command`, `cwd`, `log_dir` and `env`: `{PROPS}` (the properties
file), `{LOGDIR}`, `{PORT}`, `{TMP}`.

## Adding a stage

One file per stage, `src/stages/sNN_slug.rs`, so several people can work at once without
touching the same file. The whole API is four things: `Stage`, `Test`, `Ctx` and `Check`.

```rust
//! Stage 17 — CreateTopics (19) v7.

use crate::assert::Check;
use crate::fixtures::{FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::stages::{proto_fail, Stage, Test, NONE};

fn one_topic() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 1))
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 17,
        slug: "create_topics",          // the file must be src/stages/s17_create_topics.rs
        name: "CreateTopics (19) v7",
        ext: true,                      // beyond the core track
        hints: &[                       // 2-4 lines; they go into PLAN.md and catalog.json
            "A duplicate name is error 36, not a failure of the whole request",
            "Write a TopicRecord and one PartitionRecord per partition",
        ],
        examples: wire_examples,        // 1-3 worked examples; see "Examples" below
        tests: vec![
            Test::new("a new topic is created", creates),
            Test::new("a duplicate name is error 36", duplicate)
                .with_fixtures(one_topic)   // topics that must exist first
                .ext()                      // --skip-ext hides it
                .restart()                  // force a fresh broker process for this test
                .min_timeout_ms(60_000)     // a floor under --timeout-ms, for slow tests
                .timeout_ms(120_000)        // ... or an outright override, for a restart test
                .tag("slow")                // --tag slow selects it
                .skip_on("apache_kafka", "Kafka allows this and a hand-written broker does not"),
        ],
    }
}

kafka_test!(creates, |ctx| {
    let mut conn = ctx.connect().await?;                 // a fresh TCP connection
    let resp = conn.request(7, &req).await.map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new("CreateTopics for a fresh name", &conn);
    c.eq("response.topics[0].error_code", NONE, resp.topics[0].error_code);
    c.finish()                                            // Result<(), Failure>
});
```

Then add two lines to `src/stages/mod.rs` (`mod s17_create_topics;` and
`s17_create_topics::stage(),`), refresh the docs, and prove it against real Kafka:

```
cargo test                                                   # registry invariants + catalog
./target/release/kafkatest --broker apache_kafka --validate --stage 17
./target/release/kafkatest --capture-examples examples/captured.json --broker apache_kafka
./target/release/kafkatest --list --json catalog.json        # catalog.json must be regenerated
```

Update `PLAN.md`: every stage needs a tickbox line naming its source file and test count
(`- [ ] **Stage NN** — Title (`src/stages/sNN_slug.rs`, N tests)`) followed by its hints. A
`cargo test` checks that every implemented stage has no `(planned)` marker and every stage that
is still only planned keeps one.

### What `Ctx` gives a test

| | |
|---|---|
| `ctx.connect().await?` | A new `Conn` to the broker, with the run's timeout. |
| `conn.request(version, &req).await` | Encode with `kafka-protocol`, send, read, decode, and check the correlation id came back. |
| `conn.send_frame` / `conn.send_bytes` / `conn.read_frame` / `conn.expect_closed` / `conn.read_silence` / `conn.shutdown_write` | The raw-bytes path, for framing and malformed-input tests. |
| `ctx.topic("t1")?` | The `TopicInfo` (real `name`, `id`, `partitions`, `batches`) for a declared fixture. |
| `ctx.fixtures.segment_bytes(&name, 0)?` | The partition's log segment as the broker has it on disk. |
| `ctx.rng`, `ctx.seed` | The seeded RNG. Every random choice must come from it, or `--seed` stops meaning anything. |
| `ctx.unique("prefix")` | A name that is unique to the run but stable for a seed. |
| `ctx.dist` | The unpacked reference distribution, when one is available — use `broker::reference::run_tool(&dist, "kafka-topics.sh", &args)` for interop tests. |
| `ctx.note("p99 0.22 ms")` | An informational line shown under the test **whatever the outcome** (and carried in the JSON report's `notes`). `Check::note` only ever surfaces under a failure; this is for timings, throughput, and "nothing ran, here is why". |
| `ctx.restart_broker().await?` | Stop the broker and start it again from the same spec: same port, same properties file, same log directory, **no reformat**. Only what is on disk survives. Pair it with `Test::timeout_ms(..)`, since the reference broker needs a couple of seconds to come back. |
| `ctx.timeout`, `ctx.log_dir`, `ctx.addr`, `ctx.broker_name` | The rest of the environment. |

### What `Check` gives a test

`Check::new(what, &conn)` captures the connection's last request/response bytes so the failure
can hex-dump them; `Check::detached(what)` is for assertions about files. Then:

`eq` · `ne` · `at_least` · `at_most` · `that(path, expected, ok, actual)` ·
`bytes_eq(path, expected, actual)` · `note(line)` · `observe(path, value)` · `mark(range)` ·
`finish() -> Result<(), Failure>`.

Every failed `eq` on a numeric field automatically marks the bytes in the response that hold
the actual value, so the hex dump points at the mistake. Use `mark(0..4)` when the test already
knows the offset (the correlation id, say).

Stage tests must never `unwrap()` on a network or decode path — `proto_fail(e, &conn)` turns a
`ProtoError` into a `Failure` with the right kind (`timeout`, `connection`, `protocol`) and the
bytes attached.

### History, and what a new stage can lean on

All 47 stages are implemented; `PLAN.md` carries no `(planned)` entry any more. Stages 1-14,
19-24 and 29-32 came first, then 15-18, 25-28, 33-37, 38-42 and 43-45 were written in parallel
against this same harness — a stage is a new `src/stages/sNN_*.rs` plus two lines in
`src/stages/mod.rs`, and nothing else has to change. What a new stage can already lean on:

- `proto::records::RecordBatch` can write a batch with any base offset, producer id, sequence,
  attributes (so: compression bits, the control flag) and even a deliberately wrong CRC or
  record count — see `crc_override` and `record_count_override` for stage 37.
- `fixtures::files` writes the metadata log; `proto::meta` encodes `TopicRecord`,
  `PartitionRecord` and `FeatureLevelRecord` and can decode them back (stages 17, 18).
- `Test::restart()` forces a fresh, *empty* broker process before a test; `Ctx::restart_broker()`
  restarts the running one in place, keeping its log directory — that is how stage 35 proves
  "offsets survive a restart". A test that does the latter declares `Test::timeout_ms(120_000)`.
- `src/stages/group_protocol.rs` holds the classic consumer-group protocol stages 39-42 share:
  Join/Sync/Heartbeat/Leave/OffsetCommit/OffsetFetch builders and a hand-written
  `ConsumerProtocolSubscription`/`Assignment` codec.
- `TopicSpec::with_compressed_batch(partition, codec, records)` puts a gzip/snappy/lz4/zstd batch
  into a fixture, under either strategy (stage 27).
- `broker::reference::run_tool` runs any script from the tarball (`kafka-console-producer.sh`,
  `kafka-topics.sh`, ...) for stage 43.
- `ctx.rng` is seeded per test from `--seed`, which is what makes stage 44's fuzz reproducible.
- `helpers::expect_still_serving(ctx, after)` is the "did the broker survive?" check stages 44
  and 45 need.
- `Test::min_timeout_ms(n)` raises the floor under the per-test deadline (`--timeout-ms` still
  wins when it is larger). Stages 43–45 need it: a JVM command-line tool, 500 fuzz probes or a
  10 000-record soak cannot finish inside the 10 s default. Those stages also carry the `slow`
  tag, so `--tag slow` runs only them and `--only` narrows further.
- `Test::timeout_ms(n)` is the other half of that pair: an *override*, not a floor, for a test
  whose duration has nothing to do with what the operator asked for — stage 35 restarts a JVM
  mid-body. Precedence is `timeout_ms` > `max(--timeout-ms, min_timeout_ms)`.
- `--validate` only swaps the broker and the wording of a failure; it never widens the
  selection, so `--validate --stage 5` runs stage 5 and `--validate` alone still means `--all`.
- `src/examples` is the worked-example machinery: a stage declares `ExampleSpec`s, the request
  is encoded by the same codec the tests use, and `--capture-examples` records what Apache
  Kafka answered plus a field-by-field annotation of both byte strings. Adding an example for
  an API no other stage uses means adding that API's field order to `src/examples/schema_*.rs`
  first — the capture refuses an example whose walk does not end exactly on the frame
  boundary, which is what keeps the annotations honest.

## Deviations from the plan

- **The record codec is hand-written** (`src/proto/records.rs`) rather than reusing
  `kafka_protocol::records`. That crate's encoder derives the batch header from the records, so
  fixtures could not set a base offset, a control flag or a wrong CRC. `kafka-protocol` is
  still used for every request/response body, and a unit test round-trips our batches through
  its decoder.
- **Cluster-metadata records are hand-written** (`src/proto/meta.rs`). `kafka-protocol` 0.18
  generates the client APIs but not `TopicRecord`/`PartitionRecord`/`FeatureLevelRecord`. The
  schemas were read out of `kafka-metadata-4.1.2.jar` and the encoding is checked byte for byte
  against a segment a real broker wrote.
- **The tarball is fetched with `curl`/`wget`**, not an HTTP crate: it happens once, and it
  keeps `reqwest` and a TLS stack out of the dependency tree.
- **`{TMP}` is per broker instance, not per test.** With `restart: per_test` (your broker) that
  is the same thing. With the reference broker's `per_stage` it cannot be, so tests there get
  isolation from unique topic names instead.
- **`--json` doubles as the catalog writer** when combined with `--list`, so that both
  `kafkatest --list --json > catalog.json` and `--list --json catalog.json` work.
- **Stage numbering has gaps.** The registry, `--list` and `catalog.json` all tolerate missing
  numbers so phase-2 stages can be added independently.

## Development

```
cargo build --release
cargo test                                  # 91 unit + 21 integration tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
./target/release/kafkatest --broker apache_kafka --validate --all
pgrep -f kafka_2.13                         # must print nothing afterwards
```

Layout:

```
src/main.rs              CLI (clap), stage selection, --list
src/lib.rs               everything else, so tests/ can use it
src/config.rs            brokers.yaml, placeholder substitution
src/broker/mod.rs        spawn, wait-for-port, process-group kill, crash detection
src/broker/reference.rs  Apache Kafka download, sha512, KRaft format, boot
src/proto/mod.rs         framed client, raw-bytes mode, hex dumps
src/proto/records.rs     RecordBatch v2 writer/reader (CRC-32C, varints, headers)
src/proto/meta.rs        __cluster_metadata record codecs
src/fixtures/mod.rs      FixtureSpec / FixtureHandle
src/fixtures/files.rs    on-disk strategy
src/fixtures/api.rs      CreateTopics + Produce strategy
src/stages/mod.rs        the registry, Stage/Test/Ctx, the kafka_test! macro
src/stages/helpers.rs    api keys, error codes, request builders, raw frames
src/stages/group_protocol.rs  classic consumer-group protocol shared by stages 39-42:
                         Join/Sync/Heartbeat/Leave/OffsetCommit/OffsetFetch builders and
                         a hand-written ConsumerProtocolSubscription/Assignment codec
src/stages/sNN_*.rs      one file per stage
src/assert.rs            Check, Failure, FailureKind
src/report.rs            terminal output + JSON
src/catalog.rs           --list and catalog.json
src/examples/mod.rs      ExampleSpec: the worked examples a stage declares
src/examples/annotate.rs the byte walk that produces the field annotations
src/examples/schema_*.rs per-API field schemas the walk follows
src/examples/capture.rs  --capture-examples: send them and record the answers
tests/                   fixture bytes vs kafka-dump-log.sh, catalog freshness, CLI smoke
examples/broken_broker.rs
PLAN.md                  tickboxes and hints for all 45 stages
catalog.json             committed; a test fails if it is stale
examples/captured.json   committed; the example bytes the reference broker answered
```
