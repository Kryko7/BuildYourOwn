//! A deliberately wrong TLS server, used to show what a red `tlstest` run looks like.
//!
//! It does the two things every TLS server must do — accept connections, and say nothing
//! until the client has spoken — and then gets the very first field of its answer wrong:
//! the record it writes carries `legacy_record_version` 0x0301, which is the client-only
//! exception of RFC 8446 §5.1, and the ServerHello inside it claims `legacy_version`
//! 0x0304 and echoes the wrong session id.
//!
//! So stage 01 is green and stage 02 is red, with hex dumps and marked bytes:
//!
//! ```text
//! cargo build --release --example broken_server
//! ./target/release/tlstest --server target/release/examples/broken_server --until 5
//! ```
//!
//! It is deliberately *not* a starting point for the real thing: it never derives a key and
//! never sends a second message. Everything past the ServerHello is your job.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(port) = flag(&args, "-accept").and_then(|p| p.parse::<u16>().ok()) else {
        eprintln!(
            "usage: broken_server -accept <port> -cert <cert.pem> -key <key.pem> -rev \
             [-naccept <n>]"
        );
        std::process::exit(2);
    };
    // The certificate is read only to prove the harness handed one over; this server never
    // gets far enough to send it.
    if let Some(cert) = flag(&args, "-cert") {
        if std::fs::metadata(&cert).is_err() {
            eprintln!("broken_server: cannot read the certificate at {cert}");
            std::process::exit(2);
        }
    }
    let naccept = flag(&args, "-naccept").and_then(|n| n.parse::<u32>().ok());

    let listener = match TcpListener::bind(("0.0.0.0", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("broken_server: cannot bind port {port}: {e}");
            std::process::exit(2);
        }
    };
    println!("broken_server: listening on 0.0.0.0:{port} (and answering nonsense)");
    let mut served = 0u32;
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                serve(s);
                served += 1;
                if let Some(limit) = naccept {
                    if served >= limit {
                        return;
                    }
                }
            }
            Err(e) => eprintln!("broken_server: accept failed: {e}"),
        }
    }
}

/// The value that follows `name` on the command line.
fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// Read whatever the client says, then answer with a wrong ServerHello.
fn serve(mut stream: TcpStream) {
    let mut header = [0u8; 5];
    if stream.read_exact(&mut header).is_err() {
        return;
    }
    let length = u16::from_be_bytes([header[3], header[4]]) as usize;
    if length > 1 << 14 {
        // Even this server refuses an oversized record, so stage 03 has something to pass.
        let _ = stream.write_all(&[0x15, 0x03, 0x03, 0x00, 0x02, 0x02, 0x16]);
        return;
    }
    let mut body = vec![0u8; length];
    if stream.read_exact(&mut body).is_err() {
        return;
    }
    if header[0] != 22 || body.first() != Some(&1) {
        // Not a ClientHello: close, the way a real server would.
        return;
    }
    let _ = stream.write_all(&server_hello_record());
    let _ = stream.flush();
}

/// A ServerHello record with three deliberate mistakes in it.
fn server_hello_record() -> Vec<u8> {
    let mut body = Vec::new();
    // Mistake 1: legacy_version must be 0x0303, whatever version is negotiated.
    body.extend_from_slice(&0x0304u16.to_be_bytes());
    // A fixed random, so two runs look the same. A real server must not do this either.
    body.extend_from_slice(&[0x5b; 32]);
    // Mistake 2: legacy_session_id_echo must be the client's, byte for byte. This is not.
    body.push(4);
    body.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
    body.extend_from_slice(&0x1301u16.to_be_bytes());
    body.push(0);
    // No extensions at all: no supported_versions, no key_share.
    body.extend_from_slice(&0u16.to_be_bytes());

    let mut message = vec![2u8];
    message.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]);
    message.extend_from_slice(&body);

    let mut record = vec![22u8];
    // Mistake 3: 0x0301 is the client's first-flight exception, never a server's.
    record.extend_from_slice(&0x0301u16.to_be_bytes());
    record.extend_from_slice(&(message.len() as u16).to_be_bytes());
    record.extend_from_slice(&message);
    record
}
