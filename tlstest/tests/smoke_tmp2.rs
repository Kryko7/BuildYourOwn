use std::time::Duration;
use tlstest::certs::{CertKind, CertStore, KeyKind};
use tlstest::config::{RestartPolicy, ServerDef, ServerKind, ServerOptions};
use tlstest::server::{build_spec, free_port, ServerHandle};
use tlstest::tls::client::{Client, ClientConfig};

#[tokio::test]
async fn resumption_debug() {
    let dir = tempfile::tempdir().expect("tmp");
    let store = CertStore::new(&dir.path().join("certs")).expect("store");
    let m = store.get(CertKind::Leaf(KeyKind::EcdsaP256)).expect("cert");
    let def = ServerDef { name: "openssl".into(), kind: ServerKind::Reference, command: vec![], cwd: None,
        env: Default::default(), restart: RestartPolicy::PerTest, boot_timeout_ms: 10000,
        extra_args: vec!["-msg".into()] };
    let port = free_port().expect("port");
    let spec = build_spec(&def, dir.path(), port, &m, &ServerOptions::default(), Duration::from_secs(10)).expect("spec");
    let tmp = spec.tmp.clone();
    let mut h = ServerHandle::start(spec).expect("start");
    let mut c = Client::connect(h.addr, Duration::from_secs(5), ClientConfig::seeded(3)).await.expect("c");
    c.handshake().await.expect("handshake");
    c.echo_line("one").await.expect("echo");
    c.collect_tickets(Duration::from_millis(800)).await.ok();
    let t = c.tickets.first().expect("ticket").clone();
    eprintln!("ticket: lifetime={} age_add={} nonce={} len={}", t.ticket_lifetime, t.ticket_age_add,
        tlstest::tls::hex(&t.ticket_nonce), t.ticket.len());
    let psk = c.psk_from_ticket(&t).expect("psk");
    eprintln!("psk suite={:?} psk={}", psk.suite.name(), tlstest::tls::hex(&psk.psk));
    c.close().await.ok();
    drop(c);
    let cfg = ClientConfig::seeded(4).with_psk(psk);
    let mut c2 = Client::connect(h.addr, Duration::from_secs(5), cfg).await.expect("c2");
    let r = c2.handshake().await;
    eprintln!("second handshake: {r:?}  selected_psk={:?}",
        c2.server_hello.as_ref().and_then(|s| s.selected_psk().ok()).flatten());
    eprintln!("CH hex: {}", tlstest::tls::hex(&c2.client_hello_bytes));
    h.stop();
    eprintln!("--- server output ---\n{}", std::fs::read_to_string(tmp.join("server.stdout")).unwrap_or_default());
}
