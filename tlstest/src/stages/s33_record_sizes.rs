//! Stage 33 — Record sizes: the limit, a megabyte, and a great many small ones.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::stages::{payload_line, reversed, Stage, Test};
use crate::tls::client::Client;
use crate::tls::{ContentType, MAX_CIPHERTEXT, MAX_PLAINTEXT};
use crate::tls_test;
use std::time::Duration;

/// How long a line this suite uses. The reference reads into a 16 KiB buffer and answers a
/// longer line in chunks, so the suite stays well below that and says so.
const LINE: usize = 4_000;

/// Read application data until `n` bytes have arrived.
async fn read_exactly(client: &mut Client, n: usize) -> Result<Vec<u8>, Failure> {
    let mut out = client.pending_app_data().to_vec();
    let deadline = tokio::time::Instant::now() + client.conn.timeout;
    while out.len() < n {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return Err(crate::stages::harness(format!(
                "wanted {n} bytes back, {} arrived",
                out.len()
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
        number: 33,
        slug: "record_sizes",
        name: "Record sizes and volume",
        ext: false,
        hints: &[
            "A sender may split application data into records however it likes; a receiver \
             must reassemble the stream and never assume one record is one message",
            "Keep each protected record's length within 2^14 + 256; when the data is bigger, \
             write several records",
            "A megabyte of traffic is a few dozen records, not one: the size limit is on the \
             record, not on the connection",
            "Many tiny records are legal and expensive — 17 bytes of overhead each — but they \
             must still all arrive, in order",
        ],
        examples,
        tests: vec![
            Test::new(
                "a record at the plaintext size limit is carried",
                at_the_limit,
            )
            .min_timeout_ms(20_000),
            Test::new("a four-kilobyte line round-trips", long_line),
            Test::new("a megabyte of lines round-trips", a_megabyte)
                .min_timeout_ms(60_000)
                .tag("slow"),
            Test::new("five hundred one-byte lines round-trip", many_small)
                .min_timeout_ms(30_000)
                .tag("slow"),
            Test::new(
                "a line split over many tiny records is answered once",
                tiny_records,
            ),
            Test::new(
                "every record the server sends is within the ciphertext limit",
                server_respects_the_limit,
            )
            .min_timeout_ms(20_000),
            Test::new(
                "the connection still works after all of that",
                still_working,
            )
            .min_timeout_ms(20_000),
        ],
    }
}

tls_test!(at_the_limit, |ctx| {
    // One record whose plaintext is as close to 2^14 as the framing allows: as many
    // newline-terminated lines as fit.
    let line = payload_line(LINE, &mut ctx.rng);
    let per = line.len() + 1;
    let count = (MAX_PLAINTEXT - 64) / per;
    let mut payload = String::new();
    for _ in 0..count {
        payload.push_str(&line);
        payload.push('\n');
    }
    let mut client = ctx.handshake().await?;
    client
        .write_app_data(payload.as_bytes())
        .await
        .map_err(Failure::tls)?;
    let seen = read_exactly(&mut client, payload.len()).await?;
    let mut c = Check::new("one record at the plaintext size limit");
    c.note(format!(
        "{count} lines of {LINE} bytes in a single record: {} bytes of plaintext, under the \
         {MAX_PLAINTEXT}-byte limit",
        payload.len()
    ));
    let expected: String = (0..count)
        .map(|_| format!("{}\n", reversed(&line)))
        .collect();
    c.eq("the answer", expected.len(), seen.len());
    c.bytes_eq("the answer bytes", expected.as_bytes(), &seen);
    c.finish()
});

tls_test!(long_line, |ctx| {
    let line = payload_line(LINE, &mut ctx.rng);
    let mut client = ctx.handshake().await?;
    let answer = client
        .echo_line(&line)
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new(format!("a {LINE}-byte line"));
    c.eq("echo.len()", line.len(), answer.len());
    c.eq("echo", reversed(&line), answer);
    c.finish()
});

tls_test!(a_megabyte, |ctx| {
    let line = payload_line(1_000, &mut ctx.rng);
    let count = 1_000usize;
    let mut client = ctx.handshake().await?;
    let started = std::time::Instant::now();
    // Write in batches so several lines share a record, the way a real client would.
    let batch = 16usize;
    let mut expected = 0usize;
    for chunk in 0..count.div_ceil(batch) {
        let n = batch.min(count - chunk * batch);
        let mut payload = String::with_capacity(n * (line.len() + 1));
        for _ in 0..n {
            payload.push_str(&line);
            payload.push('\n');
        }
        expected += payload.len();
        client
            .write_app_data(payload.as_bytes())
            .await
            .map_err(Failure::tls)?;
    }
    let seen = read_exactly(&mut client, expected).await?;
    let elapsed = started.elapsed();
    ctx.note(format!(
        "{} KiB each way in {:.2} s over {} records written",
        expected / 1024,
        elapsed.as_secs_f64(),
        client.conn.layer.write_seq
    ));
    let mut c = Check::new("a megabyte of lines");
    c.eq("bytes returned", expected, seen.len());
    let want = reversed(&line);
    let mut wrong = 0usize;
    for (i, answer) in seen.split(|b| *b == b'\n').take(count).enumerate() {
        if answer != want.as_bytes() {
            if wrong == 0 {
                c.that(
                    &format!("line {i} of the answer"),
                    "the reversed line",
                    false,
                    String::from_utf8_lossy(&answer[..answer.len().min(32)]).to_string(),
                );
            }
            wrong += 1;
        }
    }
    c.eq("lines that came back wrong", 0usize, wrong);
    c.finish()
});

tls_test!(many_small, |ctx| {
    let mut client = ctx.handshake().await?;
    let started = std::time::Instant::now();
    let mut c = Check::new("five hundred one-byte lines");
    for i in 0..500 {
        let line = char::from(b'a' + (i % 26) as u8).to_string();
        let answer = client.echo_line(&line).await.map_err(|e| {
            crate::stages::handshake_failure(e, &client).note(format!("on line {i} of 500"))
        })?;
        if answer != line {
            c.eq(&format!("echo of line {i}"), line, answer);
            break;
        }
    }
    ctx.note(format!(
        "500 round trips in {:.2} s; the write sequence number reached {}",
        started.elapsed().as_secs_f64(),
        client.conn.layer.write_seq
    ));
    c.at_least("records written", 500u64, client.conn.layer.write_seq);
    c.finish()
});

tls_test!(tiny_records, |ctx| {
    let mut client = ctx.handshake().await?;
    let line = "onebyteatatime";
    for byte in line.bytes() {
        client.write_app_data(&[byte]).await.map_err(Failure::tls)?;
    }
    // Nothing should have come back yet: no newline has arrived.
    let early = client
        .read_app_data(Duration::from_millis(200))
        .await
        .unwrap_or_default();
    client.write_app_data(b"\n").await.map_err(Failure::tls)?;
    let seen = read_exactly(&mut client, line.len() + 1).await?;
    let mut c = Check::new("one line written one byte per record");
    c.note(format!(
        "{} records for {} bytes of content — 17 bytes of overhead each",
        line.len() + 1,
        line.len() + 1
    ));
    c.that(
        "application data before the newline",
        "nothing",
        early.is_empty(),
        String::from_utf8_lossy(&early).to_string(),
    );
    c.bytes_eq(
        "the answer bytes",
        format!("{}\n", reversed(line)).as_bytes(),
        &seen,
    );
    c.finish()
});

tls_test!(server_respects_the_limit, |ctx| {
    let line = payload_line(LINE, &mut ctx.rng);
    let mut client = ctx.handshake().await?;
    let mut payload = String::new();
    for _ in 0..3 {
        payload.push_str(&line);
        payload.push('\n');
    }
    client
        .write_app_data(payload.as_bytes())
        .await
        .map_err(Failure::tls)?;
    let _ = read_exactly(&mut client, payload.len()).await?;
    let mut c = Check::new("the size of every record the server sent");
    let mut biggest = 0usize;
    for (i, record) in client.conn.records_in.iter().enumerate() {
        biggest = biggest.max(record.fragment.len());
        let limit = if record.content_type == ContentType::ApplicationData {
            MAX_CIPHERTEXT
        } else {
            MAX_PLAINTEXT
        };
        c.at_most(
            &format!("records[{i}].length"),
            limit,
            record.fragment.len(),
        );
    }
    c.note(format!(
        "{} records, the largest {biggest} bytes (the ciphertext limit is {MAX_CIPHERTEXT})",
        client.conn.records_in.len()
    ));
    c.finish()
});

tls_test!(still_working, |ctx| {
    let line = payload_line(LINE, &mut ctx.rng);
    let mut client = ctx.handshake().await?;
    let _ = client
        .echo_line(&line)
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let answer = client
        .echo_line("after")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("a short line after a long one");
    c.eq("echo", "retfa".to_string(), answer);
    c.finish()
});

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::echo("A short line, for scale", "size")
            .request("`size\\n` — five bytes of content in a 22-byte ciphertext")
            .response("`ezis\\n`, the same shape back.")
            .note(
                "Every protected record costs five header bytes, one inner type byte and a \
                 16-byte tag. At one byte per record that is 22 bytes on the wire for one — \
                 legal, and a good reason to batch.",
            ),
        ExampleSpec::text("The two ceilings")
            .request(
                "TLSPlaintext.length <= 2^14        (16384)\n\
                 TLSCiphertext.length <= 2^14 + 256 (16640)",
            )
            .response(
                "A megabyte of application data is therefore at least 64 records. Splitting is \
                 the sender's job and reassembling is the receiver's.",
            )
            .note(
                "The limits are per record and say nothing about the connection. A receiver \
                 that allocates one buffer per connection and grows it for ever has a \
                 different bug, and an attacker will find it.",
            ),
    ]
}
