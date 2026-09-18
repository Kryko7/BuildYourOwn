//! `--capture-examples`: replay every stage's examples against a real server and record
//! what came back.
//!
//! A capture that cannot annotate a byte string down to its last byte is reported as a
//! suite bug rather than quietly committed — an annotation that has drifted is worse than
//! none, and it is the check that keeps `src/tls/annotate.rs` honest as stages are added.

use super::{CapturedFile, Example, ExampleEnv, ExampleSpec, Expect, Part, Scenario};
use crate::catalog::now_iso8601;
use crate::config::ServerDef;
use crate::runner::{RunOptions, Runner};
use crate::server::reference;
use crate::stages::Stage;
use crate::tls::client::Client;
use crate::tls::conn::TlsConn;
use crate::tls::{hex, TlsError};
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

/// How long an example waits for the server's answer.
const READ_TIMEOUT: Duration = Duration::from_millis(3_000);
/// How long a `Silence` example waits to prove nothing comes back.
const SILENCE_MS: u64 = 500;

/// Replay every stage's examples and write `examples/captured.json`.
pub async fn run(def: ServerDef, opts: RunOptions, stages: &[Stage], out: &Path) -> Result<()> {
    let mut runner = Runner::new(def.clone(), opts)?;
    let mut file = CapturedFile {
        generated_at: now_iso8601(),
        server: def.name.clone(),
        server_version: reference::version().ok(),
        stages: BTreeMap::new(),
    };
    let mut problems: Vec<String> = Vec::new();
    let mut total = 0usize;

    for stage in stages {
        let specs = (stage.examples)();
        let mut captured = Vec::new();
        for (index, spec) in specs.iter().enumerate() {
            let options = (spec.server)();
            let addr = runner.boot(&options).map_err(|f| {
                anyhow::anyhow!(
                    "stage {:02} example '{}': cannot start the server: {}",
                    stage.number,
                    spec.title,
                    f.messages.join("; ")
                )
            })?;
            let env = ExampleEnv {
                server_name: "localhost".to_string(),
                seed: u64::from(stage.number) * 100 + index as u64 + 1,
            };
            let example = one(addr, stage, spec, &env, &mut problems).await;
            captured.push(example);
            total += 1;
        }
        println!(
            "  stage {:02} {:<50} {} example{}",
            stage.number,
            stage.name,
            specs.len(),
            if specs.len() == 1 { "" } else { "s" }
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
        "\n{total} examples captured from '{}' over {} stages, written to {}",
        def.name,
        stages.len(),
        out.display()
    );
    if !problems.is_empty() {
        for p in &problems {
            eprintln!("  ! {p}");
        }
        bail!(
            "{} example(s) could not be captured or annotated cleanly",
            problems.len()
        );
    }
    Ok(())
}

async fn one(
    addr: SocketAddr,
    stage: &Stage,
    spec: &ExampleSpec,
    env: &ExampleEnv,
    problems: &mut Vec<String>,
) -> Example {
    let where_ = format!("stage {:02} example '{}'", stage.number, spec.title);
    let mut example = super::merge(spec, None);
    let (request, response) = match &spec.scenario {
        Scenario::Text => (Vec::new(), Vec::new()),
        Scenario::Raw { build, expect } => match raw(addr, build, *expect, env).await {
            Ok(pair) => pair,
            Err(e) => {
                problems.push(format!("{where_}: {e}"));
                return example;
            }
        },
        Scenario::Handshake {
            config,
            request,
            response,
        } => match handshake(addr, config(env), *request, *response).await {
            Ok(pair) => pair,
            Err(e) => {
                problems.push(format!("{where_}: {e}"));
                return example;
            }
        },
        Scenario::Echo { line } => match echo(addr, env, line).await {
            Ok(pair) => pair,
            Err(e) => {
                problems.push(format!("{where_}: {e}"));
                return example;
            }
        },
    };
    let (request_part, response_part) = parts(&spec.scenario);
    example.request_hex = hex(&request);
    example.response_hex = hex(&response);
    if !request.is_empty() {
        let (fields, clean) = request_part.annotate(&request);
        if !clean && !spec.malformed {
            problems.push(format!(
                "{where_}: the walk over the request bytes did not end on the last byte                  (add .malformed() if that is the point of the example)"
            ));
        }
        example.request_fields = fields;
    }
    if !response.is_empty() {
        let (fields, clean) = response_part.annotate(&response);
        if !clean {
            problems.push(format!(
                "{where_}: the walk over the response bytes did not end on the last byte"
            ));
        }
        example.response_fields = fields;
    }
    example
}

fn parts(scenario: &Scenario) -> (Part, Part) {
    match scenario {
        Scenario::Raw { .. } | Scenario::Echo { .. } => {
            (Part::ClientHelloRecord, Part::ClientHelloRecord)
        }
        Scenario::Handshake {
            request, response, ..
        } => (*request, *response),
        Scenario::Text => (Part::None, Part::None),
    }
}

async fn raw(
    addr: SocketAddr,
    build: &fn(&ExampleEnv) -> Result<Vec<u8>, String>,
    expect: Expect,
    env: &ExampleEnv,
) -> Result<(Vec<u8>, Vec<u8>)> {
    let bytes = build(env).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut conn = TlsConn::connect(addr, READ_TIMEOUT)
        .await
        .map_err(|e| anyhow::anyhow!("cannot connect: {e}"))?;
    conn.write_raw(&bytes)
        .await
        .map_err(|e| anyhow::anyhow!("cannot send: {e}"))?;
    let mut response = Vec::new();
    match expect {
        Expect::Records(n) => {
            for i in 0..n {
                match conn.read_record_within(READ_TIMEOUT).await {
                    Ok(r) => response.extend_from_slice(&r.raw),
                    Err(e) => bail!("only {i} of {n} records arrived: {e}"),
                }
            }
        }
        Expect::UntilAlert => loop {
            match conn.read_record_within(READ_TIMEOUT).await {
                Ok(r) => {
                    let done = r.content_type == crate::tls::ContentType::Alert;
                    response.extend_from_slice(&r.raw);
                    if done {
                        break;
                    }
                }
                Err(TlsError::Closed) => break,
                Err(e) => bail!("waiting for an alert: {e}"),
            }
        },
        Expect::UntilClose => loop {
            match conn.read_record_within(READ_TIMEOUT).await {
                Ok(r) => response.extend_from_slice(&r.raw),
                Err(TlsError::Closed) | Err(TlsError::Timeout(_)) => break,
                Err(e) => bail!("waiting for a close: {e}"),
            }
        },
        Expect::Silence => match conn.read_silence(Duration::from_millis(SILENCE_MS)).await {
            Ok(seen) if seen.is_empty() => {}
            Ok(seen) => bail!("expected silence, {} bytes came back", seen.len()),
            Err(TlsError::Closed) => {}
            Err(e) => bail!("expected silence, got {e}"),
        },
    }
    Ok((bytes, response))
}

async fn handshake(
    addr: SocketAddr,
    config: crate::tls::client::ClientConfig,
    request: Part,
    response: Part,
) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut client = Client::connect(addr, READ_TIMEOUT, config)
        .await
        .map_err(|e| anyhow::anyhow!("cannot connect: {e}"))?;
    client
        .handshake()
        .await
        .map_err(|e| anyhow::anyhow!("the handshake did not complete: {e}"))?;
    if request == Part::NewSessionTicket || response == Part::NewSessionTicket {
        client
            .collect_tickets(Duration::from_millis(800))
            .await
            .ok();
    }
    let take = |part: Part| -> Result<Vec<u8>> {
        Ok(match part {
            Part::None => Vec::new(),
            Part::ClientHelloRecord => {
                crate::tls::record::Record::build(22, 0x0303, &client.client_hello_bytes)
            }
            Part::ClientHello => client.client_hello_bytes.clone(),
            Part::ServerHello => client.server_hello_bytes.clone(),
            Part::HelloRetryRequest => client
                .hello_retry_request
                .as_ref()
                .map(|_| client.server_hello_bytes.clone())
                .unwrap_or_default(),
            Part::EncryptedExtensions => client.encrypted_extensions_bytes.clone(),
            Part::Certificate => client.certificate_bytes.clone(),
            Part::CertificateVerify => client.certificate_verify_bytes.clone(),
            Part::ServerFinished => client.server_finished_bytes.clone(),
            Part::ClientFinished => client.client_finished_bytes.clone(),
            Part::NewSessionTicket => {
                let ticket = client
                    .tickets
                    .first()
                    .context("the server sent no NewSessionTicket")?;
                crate::tls::msg::encode_handshake(
                    crate::tls::HandshakeType::NEW_SESSION_TICKET,
                    &encode_ticket(ticket),
                )
            }
            Part::ServerFlight => {
                let mut out = client.encrypted_extensions_bytes.clone();
                out.extend_from_slice(&client.certificate_bytes);
                out.extend_from_slice(&client.certificate_verify_bytes);
                out.extend_from_slice(&client.server_finished_bytes);
                out
            }
        })
    };
    let a = take(request)?;
    let b = take(response)?;
    client.close().await.ok();
    if a.is_empty() && request != Part::None {
        bail!("the {request:?} message never arrived");
    }
    if b.is_empty() && response != Part::None {
        bail!("the {response:?} message never arrived");
    }
    Ok((a, b))
}

/// Re-encode a parsed ticket, so the example shows the message as it was on the wire.
fn encode_ticket(ticket: &crate::tls::msg::NewSessionTicket) -> Vec<u8> {
    let mut w = crate::tls::buf::Writer::new();
    w.u32(ticket.ticket_lifetime)
        .u32(ticket.ticket_age_add)
        .vec8(&ticket.ticket_nonce)
        .vec16(&ticket.ticket)
        .raw(&crate::tls::msg::encode_extensions(&ticket.extensions));
    w.finish()
}

async fn echo(addr: SocketAddr, env: &ExampleEnv, line: &str) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut client = Client::connect(addr, READ_TIMEOUT, env.config())
        .await
        .map_err(|e| anyhow::anyhow!("cannot connect: {e}"))?;
    client
        .handshake()
        .await
        .map_err(|e| anyhow::anyhow!("the handshake did not complete: {e}"))?;
    let before = client.conn.trace.len();
    let answer = client
        .echo_line(line)
        .await
        .map_err(|e| anyhow::anyhow!("the echo did not come back: {e}"))?;
    if answer != line.chars().rev().collect::<String>() {
        bail!("the server answered {answer:?} to {line:?}");
    }
    // The application-data records are the ones written and read after the handshake.
    let sent = client
        .conn
        .trace
        .iter()
        .skip(before)
        .find(|t| {
            t.direction == crate::tls::conn::Direction::Sent && t.what.contains("application_data")
        })
        .map(|t| t.bytes.clone())
        .context("no application_data record was written")?;
    let received = client
        .conn
        .records_in
        .iter()
        .rev()
        .find(|r| r.content_type == crate::tls::ContentType::ApplicationData)
        .map(|r| r.raw.clone())
        .context("no application_data record came back")?;
    client.close().await.ok();
    Ok((sent, received))
}
