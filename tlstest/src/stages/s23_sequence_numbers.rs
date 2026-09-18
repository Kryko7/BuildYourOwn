//! Stage 23 — Record sequence numbers, and what happens when the keys change.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::stages::{Stage, Test};
use crate::tls::record::{Record, RecordLayer};
use crate::tls::{hex, ContentType};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 23,
        slug: "sequence_numbers",
        name: "Record sequence numbers and key changes",
        ext: false,
        hints: &[
            "Each direction keeps a 64-bit counter of records written under the current keys; \
             it is never sent, both sides just count",
            "The counter resets to zero every time the keys change — handshake keys to \
             application keys, and again after every KeyUpdate",
            "ChangeCipherSpec records are not encrypted and do not advance either counter",
            "A record that arrives out of order simply will not authenticate: the nonce is \
             wrong, so the tag is wrong, and that is bad_record_mac",
        ],
        examples: examples,
        tests: vec![
            Test::new(
                "the client's write counter is where it should be after the handshake",
                counter_after_handshake,
            ),
            Test::new(
                "the counter resets when the handshake keys become application keys",
                reset_on_key_change,
            ),
            Test::new(
                "many small records in a row all authenticate",
                many_records,
            ),
            Test::new(
                "the counter advances by one per record, not per byte",
                one_per_record,
            ),
            Test::new(
                "a ChangeCipherSpec record does not advance the counter",
                ccs_does_not_count,
            ),
            Test::new(
                "a record replayed at the wrong sequence number does not authenticate",
                replay_fails,
            ),
        ],
    }
}

tls_test!(counter_after_handshake, |ctx| {
    let client = ctx.handshake().await?;
    let mut c = Check::new("the sequence numbers right after the handshake");
    c.note(
        "The client wrote exactly one record under the handshake keys — its Finished — and \
         then installed the application keys, which set the counter back to zero.",
    );
    c.eq("client write sequence number", 0u64, client.conn.layer.write_seq);
    c.eq("server read sequence number", 0u64, client.conn.layer.read_seq);
    c.finish()
});

tls_test!(reset_on_key_change, |ctx| {
    let mut client = ctx.handshake().await?;
    let before = client.conn.layer.write_seq;
    client
        .write_app_data(b"one\n")
        .await
        .map_err(Failure::tls)?;
    let after_one = client.conn.layer.write_seq;
    let _ = client
        .read_line()
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    // A KeyUpdate installs new keys, which must reset the counter.
    client
        .send_key_update(crate::tls::msg::KeyUpdate::NOT_REQUESTED)
        .await
        .map_err(Failure::tls)?;
    let after_update = client.conn.layer.write_seq;
    let mut c = Check::new("the write counter across a key change");
    c.eq("sequence number after the handshake", 0u64, before);
    c.eq("sequence number after one record", 1u64, after_one);
    c.note(
        "The KeyUpdate itself goes out under the *old* keys at sequence 2, and the new keys \
         start again at zero.",
    );
    c.eq("sequence number after the KeyUpdate", 0u64, after_update);
    c.finish()
});

tls_test!(many_records, |ctx| {
    let mut client = ctx.handshake().await?;
    let mut lines = Vec::new();
    for i in 0..20 {
        let line = format!("seq{i:02}");
        let answer = client.echo_line(&line).await.map_err(|e| {
            crate::stages::handshake_failure(e, &client)
                .note(format!("on record number {i}; the nonce for it is iv XOR {i}"))
        })?;
        lines.push((line, answer));
    }
    let mut c = Check::new("twenty records in a row");
    c.note(format!(
        "write sequence number reached {}",
        client.conn.layer.write_seq
    ));
    for (line, answer) in &lines {
        c.eq(
            &format!("echo of {line:?}"),
            crate::stages::reversed(line),
            answer.clone(),
        );
    }
    c.at_least("client write sequence number", 20u64, client.conn.layer.write_seq);
    c.finish()
});

tls_test!(one_per_record, |ctx| {
    let mut client = ctx.handshake().await?;
    let before = client.conn.layer.write_seq;
    // One record holding 400 bytes, not 400 records.
    let payload = crate::stages::payload_line(400, &mut ctx.rng);
    client
        .write_app_data(format!("{payload}\n").as_bytes())
        .await
        .map_err(Failure::tls)?;
    let after = client.conn.layer.write_seq;
    let answer = client
        .read_line()
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("that the counter counts records, not bytes");
    c.eq("records written", 1u64, after - before);
    c.eq("echo", crate::stages::reversed(&payload), answer);
    c.finish()
});

tls_test!(ccs_does_not_count, |ctx| {
    let mut client = ctx.handshake_with(ctx.config().with_ccs(false)).await?;
    let before_read = client.conn.layer.read_seq;
    let before_write = client.conn.layer.write_seq;
    let ccs_in = client
        .conn
        .records_in
        .iter()
        .filter(|r| r.content_type == ContentType::ChangeCipherSpec)
        .count();
    let answer = client
        .echo_line("ccs")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("that ChangeCipherSpec records are not counted");
    c.note(format!(
        "the server sent {ccs_in} plaintext CCS record(s) during the handshake"
    ));
    c.note(
        "If they had advanced the read counter, the first application record would have \
         decrypted with the wrong nonce and failed with bad_record_mac.",
    );
    c.eq("read sequence number after the handshake", 0u64, before_read);
    c.eq("write sequence number after the handshake", 0u64, before_write);
    c.eq("echo", "scc".to_string(), answer);
    c.finish()
});

tls_test!(replay_fails, |ctx| {
    // Send one record, keep its bytes, and prove the suite's own record layer will not open
    // it a second time — which is the same reason a server must not either.
    let mut client = ctx.handshake().await?;
    let answer = client
        .echo_line("replay")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let record = client
        .conn
        .records_in
        .iter()
        .rev()
        .find(|r| r.content_type == ContentType::ApplicationData)
        .cloned()
        .ok_or_else(|| crate::stages::harness("no application record came back"))?;
    // The echo is not necessarily the server's first protected record: two
    // NewSessionTickets usually go out under the application keys first. Its sequence
    // number is whatever the read counter has reached minus one.
    let seq = client.conn.layer.read_seq.saturating_sub(1);
    let keys = client
        .schedule
        .as_ref()
        .and_then(|s| s.server_application.clone())
        .ok_or_else(|| crate::stages::harness("no server application keys"))?;
    let mut fresh = RecordLayer::new();
    fresh.set_read(keys.clone());
    fresh.read_seq = seq;
    let at_seq = fresh.open(&record);
    let mut wrong = RecordLayer::new();
    wrong.set_read(keys.clone());
    wrong.read_seq = seq + 1;
    let at_next = wrong.open(&record);
    let mut c = Check::new("a record opened at the wrong sequence number");
    c.block("the server's application record", &record.raw);
    c.eq("echo", "yalper".to_string(), answer);
    c.note(format!(
        "the record was sealed at sequence {seq}; nonce {} — at sequence {} it would be {}",
        hex(&keys.nonce(seq)),
        seq + 1,
        hex(&keys.nonce(seq + 1))
    ));
    c.that(
        &format!("opening it at sequence {seq}"),
        "succeeds",
        at_seq.is_ok(),
        at_seq.err().map(|e| e.to_string()).unwrap_or_default(),
    );
    c.that(
        &format!("opening it at sequence {}", seq + 1),
        "fails, because the nonce is different",
        at_next.is_err(),
        "the tag verified, which cannot happen if the nonce moved",
    );
    c.finish()
});

fn examples() -> Vec<ExampleSpec> {
    let _ = Record::build(0, 0, &[]);
    vec![
        ExampleSpec::text("The counter's whole life")
            .request(
                "handshake keys installed -> 0\n\
                 the client's Finished     -> 1\n\
                 application keys installed -> 0\n\
                 first application record   -> 1\n\
                 KeyUpdate sent (under the old keys) -> 3, then new keys -> 0",
            )
            .response(
                "The server counts its own writes the same way, independently. Nothing about \
                 either counter is ever transmitted.",
            )
            .note(
                "The reset is the part that bites. A counter that keeps running across the \
                 key change gives every record after the handshake the wrong nonce, and the \
                 failure looks exactly like a wrong key.",
            ),
        ExampleSpec::text("Why out-of-order is not a special case")
            .request("A record arrives twice, or two records arrive swapped")
            .response(
                "The second one is opened with the next nonce, which is not the nonce it was \
                 sealed with, so the tag does not verify: bad_record_mac(20), fatal.",
            )
            .note(
                "TLS 1.3 does not need a replay window because the record layer is strictly \
                 ordered and the transport is TCP. There is nothing extra to implement.",
            ),
    ]
}
