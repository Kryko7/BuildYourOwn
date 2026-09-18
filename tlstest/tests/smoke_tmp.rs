use std::time::Duration;
use tlstest::certs::{CertKind, CertStore, KeyKind};
use tlstest::config::{RestartPolicy, ServerDef, ServerKind, ServerOptions};
use tlstest::server::{build_spec, free_port, ServerHandle};
use tlstest::tls::client::{Client, ClientConfig};
use tlstest::tls::*;

fn def() -> ServerDef {
    ServerDef { name: "openssl".into(), kind: ServerKind::Reference, command: vec![], cwd: None,
        env: Default::default(), restart: RestartPolicy::PerTest, boot_timeout_ms: 10000, extra_args: vec![] }
}

async fn boot(dir: &std::path::Path, store: &CertStore, kind: CertKind, opts: ServerOptions) -> ServerHandle {
    let m = store.get(kind).expect("cert");
    let port = free_port().expect("port");
    let spec = build_spec(&def(), dir, port, &m, &opts, Duration::from_secs(10)).expect("spec");
    ServerHandle::start(spec).expect("start")
}

#[tokio::test]
async fn advanced_paths() {
    let dir = tempfile::tempdir().expect("tmp");
    let store = CertStore::new(&dir.path().join("certs")).expect("store");

    // 1. secp256r1 + chacha + alpn
    let h = boot(dir.path(), &store, CertKind::Leaf(KeyKind::EcdsaP256), ServerOptions::default()).await;
    let cfg = ClientConfig::seeded(1).with_groups(&[GROUP_SECP256R1])
        .with_suites(&[TLS_CHACHA20_POLY1305_SHA256]).with_alpn(&["http/1.1"]);
    let mut c = Client::connect(h.addr, Duration::from_secs(5), cfg).await.expect("c");
    c.handshake().await.expect("handshake p256/chacha");
    eprintln!("p256+chacha+alpn: suite={:?} group={:?} alpn={:?}", c.suite.map(|s| s.name()),
        c.selected_group.map(group_name), c.alpn_selected);
    assert_eq!(c.echo_line("abc").await.expect("echo"), "cba");
    drop(h);

    // 2. HelloRetryRequest: offer groups but no key share at all
    let h = boot(dir.path(), &store, CertKind::Leaf(KeyKind::EcdsaP256), ServerOptions::default()).await;
    let cfg = ClientConfig::seeded(2).with_group_shares(&[GROUP_X25519, GROUP_SECP256R1], &[]);
    let mut c = Client::connect(h.addr, Duration::from_secs(5), cfg).await.expect("c");
    match c.handshake().await {
        Ok(()) => eprintln!("HRR: retry={} group={:?} cookie={:?}", c.hello_retry_request.is_some(),
            c.selected_group.map(group_name),
            c.hello_retry_request.as_ref().and_then(|h| h.cookie().ok()).flatten().map(|v| v.len())),
        Err(e) => { eprintln!("TRACE:\n{}", c.conn.trace_lines().join("\n")); panic!("HRR failed: {e}") }
    }
    assert_eq!(c.echo_line("xyz").await.expect("echo"), "zyx");
    drop(h);

    // 3. tickets + resumption
    let h = boot(dir.path(), &store, CertKind::Leaf(KeyKind::EcdsaP256), ServerOptions::default()).await;
    let mut c = Client::connect(h.addr, Duration::from_secs(5), ClientConfig::seeded(3)).await.expect("c");
    c.handshake().await.expect("handshake");
    assert_eq!(c.echo_line("one").await.expect("echo"), "eno");
    let n = c.collect_tickets(Duration::from_millis(800)).await.unwrap_or(0);
    eprintln!("tickets: new={n} total={} lifetime={:?} early={:?}", c.tickets.len(),
        c.tickets.first().map(|t| t.ticket_lifetime), c.tickets.first().and_then(|t| t.max_early_data()));
    if let Some(t) = c.tickets.first() {
        let psk = c.psk_from_ticket(t).expect("psk");
        c.close().await.ok();
        let cfg = ClientConfig::seeded(4).with_psk(psk);
        let mut c2 = Client::connect(h.addr, Duration::from_secs(5), cfg).await.expect("c2");
        match c2.handshake().await {
            Ok(()) => eprintln!("resumed: psk accepted = {:?}",
                c2.server_hello.as_ref().and_then(|s| s.selected_psk().ok()).flatten()),
            Err(e) => { eprintln!("TRACE:\n{}", c2.conn.trace_lines().join("\n")); eprintln!("resumption failed: {e}") }
        }
        eprintln!("resumed echo: {:?}", c2.echo_line("rr").await);
    }
    drop(h);

    // 4. key update
    let h = boot(dir.path(), &store, CertKind::Leaf(KeyKind::EcdsaP256), ServerOptions::default()).await;
    let mut c = Client::connect(h.addr, Duration::from_secs(5), ClientConfig::seeded(5)).await.expect("c");
    c.handshake().await.expect("handshake");
    assert_eq!(c.echo_line("pre").await.expect("echo"), "erp");
    c.send_key_update(tlstest::tls::msg::KeyUpdate::REQUESTED).await.expect("key update");
    eprintln!("after key update: {:?}", c.echo_line("post").await);
    drop(h);

    // 5. naccept
    let h = boot(dir.path(), &store, CertKind::Leaf(KeyKind::EcdsaP256), ServerOptions::default().with_naccept(2)).await;
    eprintln!("naccept argv: {}", h.spec.command_line());
    let mut h = h;
    for i in 0..2 {
        let mut c = Client::connect(h.addr, Duration::from_secs(5), ClientConfig::seeded(6 + i)).await.expect("c");
        match c.handshake().await { Ok(()) => eprintln!("  naccept conn {i} ok"), Err(e) => eprintln!("  naccept conn {i}: {e}") }
        c.close().await.ok();
    }
    eprintln!("exited after 2: {:?}", h.wait_for_exit(Duration::from_secs(2)).is_some());
}
