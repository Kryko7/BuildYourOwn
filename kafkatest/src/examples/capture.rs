//! `--capture-examples`: send every stage's example requests to a real broker and record
//! what came back.
//!
//! The request bytes are built here with the same codec the tests use, so an example can
//! never drift; the response bytes are whatever Apache Kafka answered. Both are annotated
//! field by field, and a capture in which a walk did not consume a whole frame is reported
//! as a suite bug rather than quietly committed.

use super::annotate;
use super::{CapturedFile, Example, ExampleEnv, ExampleSpec, Expect};
use crate::catalog::now_iso8601;
use crate::config::BrokerDef;
use crate::fixtures::FixtureHandle;
use crate::proto::Conn;
use crate::runner::{RunOptions, Runner};
use crate::stages::Stage;
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

/// A capture never runs faster than this, because of the long-poll examples.
const MIN_TIMEOUT_MS: u64 = 20_000;
/// How long an `acks=0` example waits to prove nothing comes back.
const SILENCE_MS: u64 = 700;

/// Boot the broker, walk every stage's examples, and write `examples/captured.json`.
pub async fn run(def: BrokerDef, opts: RunOptions, stages: &[Stage], out: &Path) -> Result<()> {
    let timeout = Duration::from_millis(opts.timeout_ms.max(MIN_TIMEOUT_MS));
    let mut runner = Runner::new(def.clone(), opts)?;
    runner
        .boot()
        .await
        .map_err(|f| anyhow::anyhow!("cannot start '{}': {}", def.name, f.messages.join("; ")))?;

    if let Some(addr) = runner.addr() {
        warm_up(addr, timeout).await;
    }

    // The committed capture, so an example whose bytes differ only in what the broker
    // minted this boot keeps its committed form and the diff shows real changes only.
    let previous = CapturedFile::load_or_empty(out);
    let mut file = CapturedFile {
        generated_at: now_iso8601(),
        broker: def.name.clone(),
        broker_version: def.version.clone(),
        stages: BTreeMap::new(),
    };
    let mut problems: Vec<String> = Vec::new();
    let mut total = 0usize;
    let mut unchanged = 0usize;

    for stage in stages {
        let specs = (stage.examples)();
        let mut captured = Vec::new();
        for (index, spec) in specs.iter().enumerate() {
            let ex = one(&mut runner, stage, index, spec, timeout, &mut problems).await?;
            captured.push(ex);
            total += 1;
        }
        let kept = previous.keep_unchanged(stage.number, &mut captured);
        unchanged += kept;
        println!(
            "  stage {:02} {:<48} {} example{}{}",
            stage.number,
            stage.name,
            specs.len(),
            if specs.len() == 1 { "" } else { "s" },
            if kept == specs.len() {
                ", unchanged".to_string()
            } else if kept > 0 {
                format!(", {kept} unchanged")
            } else {
                String::new()
            }
        );
        file.stages.insert(stage.number.to_string(), captured);
    }
    runner.end_run();

    if let Some(dir) = out.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).ok();
        }
    }
    std::fs::write(out, file.to_json()?)
        .with_context(|| format!("cannot write {}", out.display()))?;
    println!(
        "\n{total} examples captured from '{}' over {} stages, written to {} ({unchanged} \
         unchanged but for broker-minted values, kept as committed)",
        def.name,
        stages.len(),
        out.display()
    );
    if !problems.is_empty() {
        for p in &problems {
            eprintln!("  ! {p}");
        }
        bail!(
            "{} example(s) could not be annotated field by field; the schemas in \
             src/examples/schema_*.rs and the bytes disagree",
            problems.len()
        );
    }
    Ok(())
}

/// Nudge the two pieces of state a freshly formatted Kafka creates lazily, so the examples
/// show the steady state a learner will see rather than a cold-boot retriable error.
///
/// `FindCoordinator` for a group creates `__consumer_offsets` the first time it is asked,
/// and answers 15 (COORDINATOR_NOT_AVAILABLE) until it exists; `InitProducerId` answers 14
/// (COORDINATOR_LOAD_IN_PROGRESS) until the controller has handed the broker its first block
/// of producer ids. Real clients retry both, and so does this: neither is what stages 36 and
/// 39 are about. Failures here are ignored — the capture then simply records what the broker
/// said.
async fn warm_up(addr: std::net::SocketAddr, timeout: Duration) {
    use kafka_protocol::messages::{FindCoordinatorRequest, InitProducerIdRequest, ProducerId};
    use kafka_protocol::protocol::StrBytes;

    let mut find = FindCoordinatorRequest::default();
    find.key_type = 0;
    find.coordinator_keys = vec![StrBytes::from_static_str(super::EXAMPLE_GROUP)];
    let mut init = InitProducerIdRequest::default();
    init.transactional_id = None;
    init.transaction_timeout_ms = -1;
    init.producer_id = ProducerId(-1);
    init.producer_epoch = -1;

    let mut coordinator = false;
    let mut producer_id = false;
    for _ in 0..60 {
        if coordinator && producer_id {
            return;
        }
        let Ok(mut conn) = Conn::connect(addr, timeout).await else {
            tokio::time::sleep(Duration::from_millis(250)).await;
            continue;
        };
        if !coordinator {
            if let Ok(resp) = conn.request(4, &find).await {
                coordinator = resp.coordinators.first().map(|c| c.error_code) == Some(0);
            }
        }
        if !producer_id {
            if let Ok(resp) = conn.request(4, &init).await {
                producer_id = resp.error_code == 0;
            }
        }
        if !(coordinator && producer_id) {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

/// Mark the request fields whose value the broker minted — the topic ids the fixtures got
/// — so the site can say "this one is different on your broker" about exactly those.
fn mark_broker_minted(fields: &mut [super::FieldAnn], env: &ExampleEnv) {
    let ids: Vec<&str> = env.topics.values().map(|t| t.id.as_str()).collect();
    for f in fields {
        f.varies = ids.iter().any(|id| f.value.contains(id));
    }
}

async fn one(
    runner: &mut Runner,
    stage: &Stage,
    index: usize,
    spec: &ExampleSpec,
    timeout: Duration,
    problems: &mut Vec<String>,
) -> Result<Example> {
    let where_ = format!("stage {:02} example '{}'", stage.number, spec.title);
    let fixtures = (spec.fixtures)();
    let handle = if fixtures.is_empty() {
        FixtureHandle::default()
    } else {
        let salt = format!("ex{:02}{}", stage.number, index + 1);
        runner
            .materialize(&fixtures, &salt)
            .await
            .map_err(|f| anyhow::anyhow!("{where_}: {}", f.messages.join("; ")))?
    };
    let env = ExampleEnv::from_fixtures(&handle);

    let mut example = Example {
        title: spec.title.to_string(),
        kind: spec.expect.as_str().to_string(),
        request: spec.request.to_string(),
        request_hex: String::new(),
        response: spec.response.to_string(),
        response_hex: String::new(),
        note: spec.note.map(str::to_string),
        request_fields: Vec::new(),
        response_fields: Vec::new(),
        env: if handle.topics.is_empty() {
            None
        } else {
            Some(env.clone())
        },
        minted_response: Vec::new(),
    };
    let Some(build) = spec.build else {
        return Ok(example);
    };

    let wire = build(&env).map_err(|e| anyhow::anyhow!("{where_}: {e}"))?;
    let bytes = wire.bytes();
    let mut apis = annotate::request_apis(&bytes);
    if let Some(v) = spec.response_version {
        for a in &mut apis {
            a.1 = v;
        }
    }
    let annotate::Annotated {
        fields: request_fields,
        clean,
        ..
    } = annotate::annotate_request(&bytes);
    // A `Wire::Raw` example is deliberately malformed (stages 9 and 44): the annotation
    // is best-effort there, and stopping early is the point rather than a bug.
    let deliberate = matches!(wire, super::Wire::Raw(_));
    if !clean && !deliberate {
        problems.push(format!(
            "{where_}: the request walk did not consume the whole frame"
        ));
    }
    example.request_hex = super::to_hex(&bytes);
    example.request_fields = request_fields;
    mark_broker_minted(&mut example.request_fields, &env);

    let addr = runner
        .addr()
        .ok_or_else(|| anyhow::anyhow!("{where_}: no broker is running"))?;
    let mut conn = Conn::connect(addr, timeout)
        .await
        .map_err(|e| anyhow::anyhow!("{where_}: cannot connect: {e}"))?;
    conn.send_bytes(&bytes)
        .await
        .map_err(|e| anyhow::anyhow!("{where_}: cannot send: {e}"))?;

    let response = match spec.expect {
        Expect::Nothing => Vec::new(),
        Expect::Response => {
            let mut out = Vec::new();
            for _ in 0..apis.len().max(1) {
                let payload = conn
                    .read_frame()
                    .await
                    .map_err(|e| anyhow::anyhow!("{where_}: no response: {e}"))?;
                out.extend_from_slice(&(payload.len() as i32).to_be_bytes());
                out.extend_from_slice(&payload);
            }
            out
        }
        Expect::Silence => {
            match conn.read_silence(Duration::from_millis(SILENCE_MS)).await {
                Ok(seen) if seen.is_empty() => {}
                Ok(seen) => problems.push(format!(
                    "{where_}: expected silence, but {} bytes came back",
                    seen.len()
                )),
                Err(e) => problems.push(format!("{where_}: expected silence, got {e}")),
            }
            Vec::new()
        }
        Expect::Closed => {
            match conn.expect_closed(timeout).await {
                Ok(seen) if seen.is_empty() => {}
                Ok(seen) => {
                    // Some brokers answer *and* close; keep whatever they said.
                    return Ok(finish(example, seen, &apis, problems, &where_));
                }
                Err(e) => problems.push(format!(
                    "{where_}: expected the broker to close the connection, got {e}"
                )),
            }
            Vec::new()
        }
    };
    Ok(finish(example, response, &apis, problems, &where_))
}

fn finish(
    mut example: Example,
    response: Vec<u8>,
    apis: &[(i16, i16)],
    problems: &mut Vec<String>,
    where_: &str,
) -> Example {
    if response.is_empty() {
        return example;
    }
    let annotate::Annotated {
        fields,
        clean,
        minted,
    } = annotate::annotate_response(&response, apis);
    if !clean {
        problems.push(format!(
            "{where_}: the response walk did not consume the whole frame"
        ));
    }
    example.response_hex = super::to_hex(&response);
    example.response_fields = fields;
    example.minted_response = minted;
    example
}
