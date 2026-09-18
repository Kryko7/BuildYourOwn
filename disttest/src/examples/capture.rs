//! `--capture-examples`: run every stage's examples against the reference its ladder names
//! and write down what came back.
//!
//! One reference node, one three-member reference cluster and one primitives process per
//! topic serve the whole capture, so the whole file is produced in a few seconds.

use super::{
    annotate_json, to_hex, CapturedFile, ClusterStep, Example, ExampleBody, ExampleSpec,
};
use crate::cluster::workload::{FaultSchedule, WorkloadSpec};
use crate::cluster::Cluster;
use crate::config::{TargetDef, TargetKind};
use crate::etcd::Client;
use crate::lin;
use crate::node::{self, reference, NodeHandle};
use crate::prim::PrimProc;
use crate::runner::RunOptions;
use crate::stages::{Ladder, Stage};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Capture every example and write the file.
pub async fn run(
    targets: &BTreeMap<String, TargetDef>,
    stages: &[Stage],
    out: &Path,
    opts: &RunOptions,
) -> Result<()> {
    let etcd = targets
        .get("etcd")
        .context("targets.yaml has no 'etcd' reference")?;
    let prim_target = targets
        .get("reference_primitives")
        .context("targets.yaml has no 'reference_primitives' reference")?;
    if etcd.kind != TargetKind::Reference {
        bail!("the 'etcd' target must be `kind: reference` for a capture");
    }
    let version = etcd
        .version
        .clone()
        .unwrap_or_else(|| reference::DEFAULT_VERSION.to_string());
    let dist = reference::ensure_installed(&version)?;
    let tmp = std::env::temp_dir().join(format!("disttest-capture-{}", std::process::id()));
    std::fs::create_dir_all(&tmp)?;
    let timeout = Duration::from_millis(opts.timeout_ms.max(5_000));

    let mut needs_node = false;
    let mut needs_cluster = false;
    let mut topics: Vec<&'static str> = Vec::new();
    for s in stages {
        for e in (s.examples)() {
            match &e.body {
                ExampleBody::Node { .. } => needs_node = true,
                ExampleBody::Cluster { .. } | ExampleBody::Workload { .. } => needs_cluster = true,
                ExampleBody::Primitives { topic, .. } => {
                    if !topics.contains(topic) {
                        topics.push(topic);
                    }
                }
            }
        }
    }

    let mut single: Option<(NodeHandle, Client)> = None;
    if needs_node {
        single = Some(start_single(etcd, &dist, &tmp.join("node"), timeout)?);
    }
    let mut cluster: Option<Cluster> = None;
    if needs_cluster {
        let mut c = Cluster::start(
            etcd,
            Some(&dist),
            &tmp.join("cluster"),
            3,
            0,
            timeout,
            opts.seed,
        )
        .await
        .map_err(|f| anyhow::anyhow!("cannot start the reference cluster: {}", f.messages.join("; ")))?;
        c.wait_for_leader(Duration::from_millis(10_000))
            .await
            .map_err(|f| anyhow::anyhow!("the reference cluster has no leader: {}", f.messages.join("; ")))?;
        cluster = Some(c);
    }

    let prim_argv = vec![reference::example_binary(&prim_target.name)?
        .to_string_lossy()
        .to_string()];
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let mut file = CapturedFile {
        generated_at: crate::catalog::now_iso8601(),
        target: "etcd".to_string(),
        target_version: reference::dist_version(&dist),
        stages: BTreeMap::new(),
    };

    for stage in stages {
        let specs = (stage.examples)();
        if specs.is_empty() {
            continue;
        }
        let mut captured = Vec::new();
        for spec in &specs {
            let example = match &spec.body {
                ExampleBody::Node { setup, path, body } => {
                    let (_, client) = single
                        .as_ref()
                        .context("a node example needs the reference node")?;
                    capture_node(spec, client, setup, path, body).await?
                }
                ExampleBody::Cluster { steps } => {
                    let c = cluster
                        .as_mut()
                        .context("a cluster example needs the reference cluster")?;
                    capture_cluster(spec, c, steps()).await?
                }
                ExampleBody::Workload {
                    duration_ms,
                    clients,
                    keys,
                } => {
                    let c = cluster
                        .as_mut()
                        .context("a workload example needs the reference cluster")?;
                    capture_workload(spec, c, *duration_ms, *clients, *keys, opts.seed, stage.number)
                        .await?
                }
                ExampleBody::Primitives { topic, commands } => {
                    capture_prim(spec, &prim_argv, &cwd, topic, commands(), &tmp, timeout).await?
                }
            };
            println!(
                "  stage {:02} {:<11} {}",
                stage.number,
                stage.ladder.as_str(),
                example.title
            );
            captured.push(example);
        }
        file.stages.insert(stage.number.to_string(), captured);
    }

    if let Some((mut n, _)) = single {
        n.stop();
    }
    if let Some(mut c) = cluster {
        c.shutdown();
    }
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::write(out, file.to_json()?)
        .with_context(|| format!("cannot write {}", out.display()))?;
    println!("captured {} stages into {}", file.stages.len(), out.display());
    let _ = Ladder::ALL;
    Ok(())
}

fn start_single(
    def: &TargetDef,
    dist: &Path,
    tmp: &Path,
    timeout: Duration,
) -> Result<(NodeHandle, Client)> {
    let ports = node::free_ports(2)?;
    let peer_url = format!("http://127.0.0.1:{}", ports[1]);
    let spec = node::spec_for(
        def,
        Some(dist),
        "m1",
        tmp,
        &tmp.join("data"),
        ports[0],
        ports[1],
        &format!("m1={peer_url}"),
        &peer_url,
    )?;
    let handle = NodeHandle::start(spec)?;
    let client = Client::new(&handle.spec.client_url(), "m1", timeout)
        .map_err(|e| anyhow::anyhow!("cannot build a client: {e}"))?;
    Ok((handle, client))
}

async fn capture_node(
    spec: &ExampleSpec,
    client: &Client,
    setup: &fn() -> Vec<(String, Value)>,
    path: &str,
    body: &fn() -> Value,
) -> Result<Example> {
    for (p, b) in setup() {
        client
            .post_raw(&p, &b)
            .await
            .map_err(|e| anyhow::anyhow!("setup {p} failed: {e}"))?;
    }
    let request_body = body();
    let request_text = serde_json::to_string(&request_body)?;
    let resp = client
        .post_raw(path, &request_body)
        .await
        .map_err(|e| anyhow::anyhow!("{path} failed: {e}"))?;
    let response_text = resp.text();
    let pretty_response = serde_json::from_str::<Value>(&response_text)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| response_text.clone());
    Ok(Example {
        title: spec.title.to_string(),
        kind: spec.body.kind().as_str().to_string(),
        request: format!(
            "{}\n\nPOST {path}\n{}",
            spec.request,
            serde_json::to_string_pretty(&request_body)?
        ),
        request_hex: to_hex(request_text.as_bytes()),
        response: format!(
            "{}\n\nHTTP {} {}\n{pretty_response}",
            spec.response, resp.status, resp.reason
        ),
        response_hex: to_hex(response_text.as_bytes()),
        note: spec.note.map(str::to_string),
        request_fields: annotate_json(&request_text, "request."),
        response_fields: annotate_json(&response_text, "response."),
    })
}

async fn capture_cluster(
    spec: &ExampleSpec,
    cluster: &mut Cluster,
    steps: Vec<ClusterStep>,
) -> Result<Example> {
    let mut asked = String::new();
    let mut answered = String::new();
    for step in &steps {
        let client = cluster.client(step.member);
        let resp = client
            .post_raw(&step.path, &step.body)
            .await
            .map_err(|e| anyhow::anyhow!("{} on m{} failed: {e}", step.path, step.member + 1))?;
        asked.push_str(&format!(
            "m{} POST {}  {}\n    {}\n",
            step.member + 1,
            step.path,
            step.note,
            serde_json::to_string(&step.body)?
        ));
        answered.push_str(&format!(
            "m{} {} {}\n    {}\n",
            step.member + 1,
            resp.status,
            resp.reason,
            resp.text()
        ));
    }
    Ok(Example {
        title: spec.title.to_string(),
        kind: spec.body.kind().as_str().to_string(),
        request: format!("{}\n\n{}", spec.request, asked.trim_end()),
        request_hex: to_hex(asked.trim_end().as_bytes()),
        response: format!("{}\n\n{}", spec.response, answered.trim_end()),
        response_hex: to_hex(answered.trim_end().as_bytes()),
        note: spec.note.map(str::to_string),
        request_fields: Vec::new(),
        response_fields: Vec::new(),
    })
}

async fn capture_workload(
    spec: &ExampleSpec,
    cluster: &mut Cluster,
    duration_ms: u64,
    clients: usize,
    keys: usize,
    seed: u64,
    stage: u32,
) -> Result<Example> {
    let mut wspec = WorkloadSpec::small(seed, &format!("example{stage}"));
    wspec.duration = Duration::from_millis(duration_ms);
    wspec.clients = clients;
    wspec.keys = keys;
    let schedule = FaultSchedule::none();
    let result = crate::cluster::workload::run(cluster, &wspec, &schedule)
        .await
        .map_err(|f| anyhow::anyhow!("the example workload failed: {}", f.messages.join("; ")))?;
    let verdict = lin::check(&result.history);
    let key = result.history.keys().first().cloned().unwrap_or_default();
    let entries = result.history.for_key(&key);
    let shown: Vec<lin::Entry> = entries.into_iter().take(12).collect();
    let asked = format!(
        "{} clients, {} keys, {} ms, no faults\n{}",
        clients,
        keys,
        duration_ms,
        schedule.render()
    );
    let answered = format!(
        "{}\n\nthe first {} operations recorded on {key}:\n{}\n\nverdict: {}",
        result.summary(),
        shown.len(),
        lin::render_entries(&shown),
        match &verdict {
            lin::Verdict::Linearizable => "linearizable".to_string(),
            lin::Verdict::Inconclusive { key, states } =>
                format!("inconclusive on {key} after {states} states"),
            lin::Verdict::NotLinearizable(v) => v.render(),
        }
    );
    Ok(Example {
        title: spec.title.to_string(),
        kind: spec.body.kind().as_str().to_string(),
        request: format!("{}\n\n{asked}", spec.request),
        request_hex: to_hex(asked.as_bytes()),
        response: format!("{}\n\n{answered}", spec.response),
        response_hex: to_hex(answered.as_bytes()),
        note: spec.note.map(str::to_string),
        request_fields: Vec::new(),
        response_fields: Vec::new(),
    })
}

async fn capture_prim(
    spec: &ExampleSpec,
    argv: &[String],
    cwd: &Path,
    topic: &str,
    commands: Vec<String>,
    tmp: &Path,
    timeout: Duration,
) -> Result<Example> {
    let mut proc = PrimProc::start(argv, cwd, &[], topic, &tmp.join("prim"), timeout)
        .await
        .map_err(|f| anyhow::anyhow!("cannot start the primitives reference: {}", f.messages.join("; ")))?;
    let mut asked = format!("./your_program.sh {topic}\n");
    let mut answered = String::new();
    for c in &commands {
        let v = proc
            .send(c)
            .await
            .map_err(|f| anyhow::anyhow!("{c}: {}", f.messages.join("; ")))?;
        asked.push_str(&format!("{c}\n"));
        answered.push_str(&format!("{}\n", serde_json::to_string(&v)?));
    }
    proc.close().await;
    Ok(Example {
        title: spec.title.to_string(),
        kind: spec.body.kind().as_str().to_string(),
        request: format!("{}\n\n{}", spec.request, asked.trim_end()),
        request_hex: to_hex(asked.trim_end().as_bytes()),
        response: format!("{}\n\n{}", spec.response, answered.trim_end()),
        response_hex: to_hex(answered.trim_end().as_bytes()),
        note: spec.note.map(str::to_string),
        request_fields: Vec::new(),
        response_fields: Vec::new(),
    })
}
