//! A deliberately broken broker, used to show what a red `kafkatest` run looks like.
//!
//! It does the one thing every Kafka broker must do — accept TCP connections and answer
//! framed requests — and then gets the very first field wrong: the correlation id comes
//! back incremented by one. Stage 01 passes, stages 02-05 fail with hex dumps.
//!
//! ```text
//! cargo build --release --example broken_broker
//! ./target/release/kafkatest --broker target/release/examples/broken_broker --until 5
//! ```
//!
//! Like a real broker it takes a properties file as argv[1] and listens on the port named
//! by `listeners=PLAINTEXT://host:port`, so the harness can give it a free port.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

fn main() {
    let props = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: broken_broker <server.properties>");
            std::process::exit(2);
        }
    };
    let port = match port_from_properties(&props) {
        Some(p) => p,
        None => {
            eprintln!("broken_broker: no PLAINTEXT listener port in {props}");
            std::process::exit(2);
        }
    };
    let listener = match TcpListener::bind(("0.0.0.0", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("broken_broker: cannot bind port {port}: {e}");
            std::process::exit(2);
        }
    };
    println!("broken_broker: listening on 0.0.0.0:{port} (and answering nonsense)");
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                std::thread::spawn(move || serve(s));
            }
            Err(e) => eprintln!("broken_broker: accept failed: {e}"),
        }
    }
}

/// Read `listeners=PLAINTEXT://0.0.0.0:9092,...` and return the plaintext port.
fn port_from_properties(path: &str) -> Option<u16> {
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("listeners=") else {
            continue;
        };
        for entry in rest.split(',') {
            if let Some(hostport) = entry.trim().strip_prefix("PLAINTEXT://") {
                if let Some((_, port)) = hostport.rsplit_once(':') {
                    return port.parse().ok();
                }
            }
        }
    }
    None
}

fn serve(mut stream: TcpStream) {
    loop {
        let mut size = [0u8; 4];
        if stream.read_exact(&mut size).is_err() {
            return;
        }
        let len = i32::from_be_bytes(size);
        if !(4..=10_000_000).contains(&len) {
            return;
        }
        let mut payload = vec![0u8; len as usize];
        if stream.read_exact(&mut payload).is_err() {
            return;
        }
        // The request header is api_key(2) api_version(2) correlation_id(4).
        let correlation = i32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);

        // Bug on purpose: the correlation id is echoed off by one, and the body is a bare
        // error code that no api version actually defines.
        let mut body = correlation.wrapping_add(1).to_be_bytes().to_vec();
        body.extend_from_slice(&7i16.to_be_bytes());
        let mut frame = (body.len() as i32).to_be_bytes().to_vec();
        frame.extend_from_slice(&body);
        if stream.write_all(&frame).is_err() || stream.flush().is_err() {
            return;
        }
    }
}
