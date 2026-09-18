//! Stage 30 — The `-rev` echo: the application layer this track uses.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::stages::{payload_line, reversed, Stage, Test};
use crate::tls::client::Client;
use crate::tls_test;
use std::time::Duration;

/// Read exactly `n` bytes of application data, or fail saying how many arrived.
async fn read_exactly(client: &mut Client, n: usize) -> Result<Vec<u8>, Failure> {
    let mut out = client.pending_app_data().to_vec();
    let deadline = tokio::time::Instant::now() + client.conn.timeout;
    while out.len() < n {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return Err(crate::stages::harness(format!(
                "wanted {n} bytes of application data, {} arrived: {:?}",
                out.len(),
                String::from_utf8_lossy(&out)
            )));
        }
        match client.read_app_data(left).await {
            Ok(more) => out.extend_from_slice(&more),
            Err(e) => return Err(crate::stages::handshake_failure(e, client)),
        }
    }
    Ok(out)
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 30,
        slug: "echo",
        name: "The -rev echo",
        ext: false,
        hints: &[
            "`-rev` is line-based: buffer bytes until a newline arrives, then answer that one \
             line and keep the rest",
            "Strip the trailing CR and LF, reverse the bytes that are left, and write them \
             back followed by exactly one LF",
            "An empty line — a lone newline — answers with a lone newline; there is nothing \
             to reverse",
            "Only complete lines are answered: a write with no newline in it gets nothing \
             back until one arrives",
        ],
        examples,
        tests: vec![
            Test::new("one line comes back reversed", one_line),
            Test::new(
                "the answer ends with exactly one newline",
                exactly_one_newline,
            ),
            Test::new("a CRLF line is answered with a bare LF", crlf),
            Test::new("an empty line answers with an empty line", empty_line),
            Test::new(
                "two lines in one write get two answers, in order",
                two_lines,
            ),
            Test::new(
                "a line spread over two records is answered once",
                line_across_records,
            ),
            Test::new(
                "a write with no newline gets no answer until one arrives",
                no_newline_no_answer,
            ),
            Test::new(
                "a line of punctuation and digits round-trips byte for byte",
                mixed_bytes,
            ),
        ],
    }
}

tls_test!(one_line, |ctx| {
    let mut client = ctx.handshake().await?;
    let answer = client
        .echo_line("hello")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("the answer to one line");
    c.note("the client wrote `hello\\n` as one application_data record");
    c.eq("echo", "olleh".to_string(), answer);
    c.finish()
});

tls_test!(exactly_one_newline, |ctx| {
    let mut client = ctx.handshake().await?;
    client
        .write_app_data(b"abcdef\n")
        .await
        .map_err(Failure::tls)?;
    let seen = read_exactly(&mut client, 7).await?;
    // Give a server that wanted to send more a moment to do so.
    let extra = client
        .read_app_data(Duration::from_millis(200))
        .await
        .unwrap_or_default();
    let mut c = Check::new("the exact bytes of the answer");
    c.note(
        "The reference strips the trailing newline, reverses what is left and writes back one \
         LF. Not CRLF, not two, not none.",
    );
    c.bytes_eq("the answer bytes", b"fedcba\n", &seen);
    c.eq("bytes after the answer", 0usize, extra.len());
    c.finish()
});

tls_test!(crlf, |ctx| {
    let mut client = ctx.handshake().await?;
    client
        .write_app_data(b"abc\r\n")
        .await
        .map_err(Failure::tls)?;
    let seen = read_exactly(&mut client, 4).await?;
    let mut c = Check::new("a line terminated with CRLF");
    c.note(
        "Both the CR and the LF are stripped before the reversal, and exactly one LF is put \
         back — a reversed line never starts with a stray CR.",
    );
    c.bytes_eq("the answer bytes", b"cba\n", &seen);
    c.finish()
});

tls_test!(empty_line, |ctx| {
    let mut client = ctx.handshake().await?;
    client.write_app_data(b"\n").await.map_err(Failure::tls)?;
    let seen = read_exactly(&mut client, 1).await?;
    let mut c = Check::new("a lone newline");
    c.bytes_eq("the answer bytes", b"\n", &seen);
    c.finish()
});

tls_test!(two_lines, |ctx| {
    let mut client = ctx.handshake().await?;
    client
        .write_app_data(b"first\nsecond\n")
        .await
        .map_err(Failure::tls)?;
    let seen = read_exactly(&mut client, 13).await?;
    let mut c = Check::new("two lines written as one record");
    c.note(
        "One record, two lines, two answers — and in the order they were sent. The record \
         boundary means nothing to the application layer.",
    );
    c.bytes_eq("the answer bytes", b"tsrif\ndnoces\n", &seen);
    c.finish()
});

tls_test!(line_across_records, |ctx| {
    let mut client = ctx.handshake().await?;
    client.write_app_data(b"abc").await.map_err(Failure::tls)?;
    tokio::time::sleep(Duration::from_millis(30)).await;
    client
        .write_app_data(b"def\n")
        .await
        .map_err(Failure::tls)?;
    let seen = read_exactly(&mut client, 7).await?;
    let mut c = Check::new("one line split across two records");
    c.note(
        "The application layer sees a byte stream. Two records, one line, one answer — and \
         the answer reverses the whole line, not each record.",
    );
    c.bytes_eq("the answer bytes", b"fedcba\n", &seen);
    c.finish()
});

tls_test!(no_newline_no_answer, |ctx| {
    let mut client = ctx.handshake().await?;
    client
        .write_app_data(b"unterminated")
        .await
        .map_err(Failure::tls)?;
    let quiet = client
        .read_app_data(Duration::from_millis(400))
        .await
        .unwrap_or_default();
    let mut c = Check::new("a write with no newline in it");
    c.that(
        "application data before the newline arrives",
        "nothing at all",
        quiet.is_empty(),
        String::from_utf8_lossy(&quiet).to_string(),
    );
    c.finish()?;
    // And the moment the newline shows up, the whole line is answered.
    client.write_app_data(b"\n").await.map_err(Failure::tls)?;
    let seen = read_exactly(&mut client, 13).await?;
    let mut c = Check::new("the answer once the newline arrives");
    c.bytes_eq("the answer bytes", b"detanimretnu\n", &seen);
    c.finish()
});

tls_test!(mixed_bytes, |ctx| {
    let line = payload_line(48, &mut ctx.rng);
    let mut client = ctx.handshake().await?;
    let punctuated = format!("{line}!?,.:;-_0123456789");
    let answer = client.echo_line(&punctuated).await.map_err(|e| {
        crate::stages::handshake_failure(e, &client).note(format!("the line was {punctuated:?}"))
    })?;
    let mut c = Check::new("a line of letters, digits and punctuation");
    c.note("The reversal is over bytes, so every printable ASCII character behaves the same.");
    c.eq("echo", reversed(&punctuated), answer);
    c.finish()
});

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::echo("One line in, one line out", "hello")
            .request(
                "An application_data record whose plaintext is `hello\\n` — six bytes inside \
                 the AEAD, plus the inner content type and the tag",
            )
            .response(
                "An application_data record whose plaintext is `olleh\\n`. The record on the \
                 wire is opaque; only the length gives its size away.",
            )
            .note(
                "Compare the two record lengths in the hex: both are 6 + 1 + 16 = 23 bytes of \
                 ciphertext. The extra byte is the inner content_type and the 16 are the tag.",
            ),
        ExampleSpec::text("Exactly what `-rev` does")
            .request(
                "Read until a newline. Strip trailing CR and LF. Reverse the remaining bytes. \
                 Write them, then one LF.",
            )
            .response(
                "`hello\\n`   -> `olleh\\n`\n\
                 `abc\\r\\n`   -> `cba\\n`\n\
                 `\\n`        -> `\\n`\n\
                 `abc` (no newline) -> nothing, until a newline arrives",
            )
            .note(
                "The reference reads into a fixed buffer, so a line longer than that buffer is \
                 answered in chunks, each reversed on its own. This suite keeps its lines well \
                 under that so the contract stays simple.",
            ),
    ]
}
