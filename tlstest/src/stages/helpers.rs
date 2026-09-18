//! Shared building blocks for stage tests: raw record construction, the "did the server
//! refuse?" check that RFC 8446 makes necessary, and the mutations the fuzz stage uses.

use crate::assert::{Check, Failure};
use crate::tls::client::{build_hello, ClientConfig};
use crate::tls::conn::{Reaction, TlsConn};
use crate::tls::msg::ClientHello;
use crate::tls::record::Record;
use crate::tls::{AlertDescription, TlsError, LEGACY_VERSION_TLS12};
use rand::Rng;
use std::time::Duration;

/// How long a test waits to see whether a server is going to refuse something.
pub const REACTION_MS: u64 = 1_500;

/// Build the ClientHello a configuration describes, as a handshake message.
pub fn hello_message(config: &ClientConfig) -> Result<Vec<u8>, String> {
    build_hello(config)
        .map(|(hello, _)| hello.encode())
        .map_err(|e| e.to_string())
}

/// Build the ClientHello a configuration describes, as one `TLSPlaintext` record.
pub fn hello_record(config: &ClientConfig) -> Result<Vec<u8>, String> {
    Ok(Record::build(
        22,
        LEGACY_VERSION_TLS12,
        &hello_message(config)?,
    ))
}

/// The parsed ClientHello a configuration describes.
pub fn hello_struct(config: &ClientConfig) -> Result<ClientHello, Failure> {
    build_hello(config)
        .map(|(hello, _)| hello)
        .map_err(Failure::tls)
}

/// Send bytes, then find out what the server did about them.
///
/// RFC 8446 lets a server answer a broken flight with an alert *or* simply drop the
/// connection, so every robustness test asks this question rather than demanding one of
/// the two.
pub async fn provoke(conn: &mut TlsConn, bytes: &[u8]) -> Reaction {
    match conn.write_raw(bytes).await {
        Ok(()) => {}
        Err(TlsError::Closed) | Err(TlsError::Io(_)) => return Reaction::Closed,
        Err(e) => return Reaction::Error(e.to_string()),
    }
    conn.reaction(Duration::from_millis(REACTION_MS)).await
}

/// Read until the server does something other than hand over a session ticket.
///
/// A server usually has one or two NewSessionTickets already in flight by the time a test
/// provokes it after the handshake, and those are not an answer to anything.
pub async fn reaction_past_tickets(conn: &mut TlsConn) -> Reaction {
    for _ in 0..8 {
        let reaction = conn.reaction(Duration::from_millis(REACTION_MS)).await;
        match &reaction {
            Reaction::Message(m) if m.contains("new_session_ticket") => continue,
            _ => return reaction,
        }
    }
    Reaction::Silence
}

/// Assert that a server refused: an alert, or a closed connection. Both are legal.
pub fn check_refused(c: &mut Check, path: &str, what: &str, reaction: &Reaction) {
    c.that(
        path,
        &format!("{what} — a fatal alert, or the connection closed"),
        reaction.is_refusal(),
        reaction.describe(),
    );
}

/// Assert that a server refused, and that any alert it sent was one of `allowed`.
///
/// The list exists because RFC 8446 often permits more than one description for the same
/// mistake, and because a server that closes without an alert is still conformant.
pub fn check_refused_with(
    c: &mut Check,
    path: &str,
    reaction: &Reaction,
    allowed: &[AlertDescription],
) {
    let names: Vec<String> = allowed.iter().map(|a| a.name()).collect();
    c.that(
        path,
        &format!(
            "a refusal: one of {} , or the connection closed without an alert",
            names.join(" / ")
        ),
        match reaction {
            Reaction::Alert(a) => allowed.contains(&a.description),
            Reaction::Closed => true,
            _ => false,
        },
        reaction.describe(),
    );
}

/// Assert that the server did *not* refuse: it answered like a normal server.
pub fn check_accepted(c: &mut Check, path: &str, what: &str, reaction: &Reaction) {
    c.that(
        path,
        &format!("{what} — a normal TLS answer"),
        matches!(reaction, Reaction::Message(_)),
        reaction.describe(),
    );
}

/// Connect, build the ClientHello this configuration describes, let `edit` change it, send
/// it and drive the rest of the handshake.
///
/// This is the shape most of section B wants: the interesting part is one field of an
/// otherwise perfectly ordinary hello.
pub async fn handshake_edited(
    ctx: &crate::stages::Ctx,
    config: ClientConfig,
    edit: impl FnOnce(&mut ClientHello),
) -> Result<crate::tls::client::Client, Failure> {
    let mut client = send_edited_hello(ctx, config, edit).await?;
    match async {
        client.read_server_hello().await?;
        client.read_server_flight().await?;
        client.send_client_finished().await
    }
    .await
    {
        Ok(()) => Ok(client),
        Err(e) => Err(crate::stages::handshake_failure(e, &client)),
    }
}

/// Connect, build and edit the ClientHello, send it (and the compatibility CCS), and stop.
pub async fn send_edited_hello(
    ctx: &crate::stages::Ctx,
    config: ClientConfig,
    edit: impl FnOnce(&mut ClientHello),
) -> Result<crate::tls::client::Client, Failure> {
    let conn = ctx.connect().await?;
    let mut client = crate::tls::client::Client::over(conn, config);
    let (mut hello, _) = client.prepare_client_hello().map_err(Failure::tls)?;
    edit(&mut hello);
    let bytes = hello.encode();
    client
        .conn
        .write_record(crate::tls::ContentType::Handshake, &bytes)
        .await
        .map_err(Failure::tls)?;
    client.note_client_hello(hello, bytes);
    client.maybe_send_ccs().await.map_err(Failure::tls)?;
    Ok(client)
}

/// Send an edited ClientHello and report what the server did about it.
pub async fn provoke_edited_hello(
    ctx: &crate::stages::Ctx,
    config: ClientConfig,
    edit: impl FnOnce(&mut ClientHello),
) -> Result<(Reaction, Vec<u8>), Failure> {
    let mut client = send_edited_hello(ctx, config, edit).await?;
    let bytes = client.client_hello_bytes.clone();
    let reaction = client
        .conn
        .reaction(Duration::from_millis(REACTION_MS))
        .await;
    Ok((reaction, bytes))
}

/// The mutations the fuzz stage applies to a byte string, named so a failure can say which
/// one the server fell over on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mutation {
    /// Flip one bit.
    BitFlip,
    /// Cut the bytes short.
    Truncate,
    /// Replace a two-byte length with something enormous.
    HugeLength,
    /// Replace a two-byte length with zero.
    ZeroLength,
    /// Overwrite a run of bytes with random ones.
    Splat,
    /// Duplicate a slice of the bytes, so a length no longer matches.
    Duplicate,
    /// Replace the record's content type with a type TLS does not define.
    WrongContentType,
    /// Replace the handshake message type with one a server never expects here.
    WrongHandshakeType,
}

impl Mutation {
    /// Every mutation, in the order the fuzz stage cycles them.
    pub fn all() -> [Mutation; 8] {
        [
            Mutation::BitFlip,
            Mutation::Truncate,
            Mutation::HugeLength,
            Mutation::ZeroLength,
            Mutation::Splat,
            Mutation::Duplicate,
            Mutation::WrongContentType,
            Mutation::WrongHandshakeType,
        ]
    }

    /// How this mutation reads in a report.
    pub fn name(self) -> &'static str {
        match self {
            Mutation::BitFlip => "a single flipped bit",
            Mutation::Truncate => "the bytes cut short",
            Mutation::HugeLength => "a length field replaced with 0xffff",
            Mutation::ZeroLength => "a length field replaced with zero",
            Mutation::Splat => "a run of bytes replaced with noise",
            Mutation::Duplicate => "a slice duplicated so the lengths no longer add up",
            Mutation::WrongContentType => "an undefined record content type",
            Mutation::WrongHandshakeType => "an undefined handshake message type",
        }
    }
}

/// Apply one mutation to a byte string, using the test's seeded RNG.
pub fn mutate(bytes: &[u8], mutation: Mutation, rng: &mut impl Rng) -> Vec<u8> {
    let mut out = bytes.to_vec();
    if out.is_empty() {
        return out;
    }
    match mutation {
        Mutation::BitFlip => {
            let at = rng.random_range(0..out.len());
            out[at] ^= 1 << rng.random_range(0..8);
        }
        Mutation::Truncate => {
            let keep = rng.random_range(1..=out.len());
            out.truncate(keep);
        }
        Mutation::HugeLength => {
            if out.len() >= 5 {
                out[3] = 0xff;
                out[4] = 0xff;
            }
        }
        Mutation::ZeroLength => {
            if out.len() >= 5 {
                out[3] = 0;
                out[4] = 0;
            }
        }
        Mutation::Splat => {
            let at = rng.random_range(0..out.len());
            let end = (at + rng.random_range(1..=16)).min(out.len());
            for b in &mut out[at..end] {
                *b = rng.random();
            }
        }
        Mutation::Duplicate => {
            let at = rng.random_range(0..out.len());
            let end = (at + rng.random_range(1..=32)).min(out.len());
            let slice = out[at..end].to_vec();
            out.splice(end..end, slice);
        }
        Mutation::WrongContentType => {
            out[0] = rng.random_range(24..=255);
        }
        Mutation::WrongHandshakeType => {
            if out.len() > 5 {
                out[5] = rng.random_range(100..=253);
            }
        }
    }
    out
}

/// The line a `-rev` server should answer with.
///
/// `openssl s_server -rev` strips the trailing `\r` and `\n`, reverses what is left, and
/// writes it back with exactly one `\n`. So the answer to a line is the line reversed, and
/// the reversal is over *bytes*, which is why the suite's payloads stay ASCII.
pub fn reversed(line: &str) -> String {
    line.chars().rev().collect()
}

/// A line of `len` printable ASCII characters, derived from the seed.
pub fn payload_line(len: usize, rng: &mut impl Rng) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    (0..len)
        .map(|_| char::from(ALPHABET[rng.random_range(0..ALPHABET.len())]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    #[test]
    fn a_hello_record_is_a_handshake_record_holding_a_client_hello() {
        let bytes = hello_record(&ClientConfig::seeded(1)).expect("hello");
        assert_eq!(bytes[0], 22);
        assert_eq!(&bytes[1..3], &[0x03, 0x03]);
        let length = u16::from_be_bytes([bytes[3], bytes[4]]) as usize;
        assert_eq!(bytes.len(), 5 + length);
        assert_eq!(bytes[5], 1, "client_hello(1)");
    }

    #[test]
    fn reversed_matches_what_the_reference_does() {
        assert_eq!(reversed("hello"), "olleh");
        assert_eq!(reversed(""), "");
        assert_eq!(reversed("ab"), "ba");
    }

    #[test]
    fn every_mutation_changes_the_bytes_and_none_of_them_panic() {
        let mut rng = StdRng::seed_from_u64(7);
        let original = hello_record(&ClientConfig::seeded(2)).expect("hello");
        for mutation in Mutation::all() {
            let mut changed = false;
            for _ in 0..32 {
                let out = mutate(&original, mutation, &mut rng);
                if out != original {
                    changed = true;
                }
            }
            assert!(changed, "{} never changed anything", mutation.name());
        }
        // And nothing falls over on a degenerate input.
        for mutation in Mutation::all() {
            let _ = mutate(&[], mutation, &mut rng);
            let _ = mutate(&[1], mutation, &mut rng);
        }
    }

    #[test]
    fn mutations_are_reproducible_for_a_seed() {
        let original = hello_record(&ClientConfig::seeded(3)).expect("hello");
        let run = || {
            let mut rng = StdRng::seed_from_u64(99);
            Mutation::all()
                .iter()
                .map(|m| mutate(&original, *m, &mut rng))
                .collect::<Vec<_>>()
        };
        assert_eq!(run(), run(), "--seed must reproduce the same fuzz corpus");
    }

    #[test]
    fn payload_lines_are_printable_and_the_right_length() {
        let mut rng = StdRng::seed_from_u64(5);
        let line = payload_line(64, &mut rng);
        assert_eq!(line.len(), 64);
        assert!(line.chars().all(|c| c.is_ascii_alphanumeric()));
    }
}
