//! Stage 05 — A ClientHello split across TCP segments and across records.

use crate::assert::{Check, Failure};
use crate::examples::{ExampleEnv, ExampleSpec, Expect};
use crate::stages::{hello_message, Stage, Test};
use crate::tls::client::{Client, ClientConfig};
use crate::tls::record::Record;
use crate::tls::{ContentType, LEGACY_VERSION_TLS12};
use crate::tls_test;
use std::time::Duration;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 5,
        slug: "split_client_hello",
        name: "A ClientHello split across segments and records",
        ext: false,
        hints: &[
            "TCP has no message boundaries: keep a buffer per connection and only act when a \
             whole record is in it",
            "A handshake message may span several records, so keep a second buffer for \
             handshake bytes and only parse when msg_type's uint24 length is satisfied",
            "Reassemble across records before parsing, never the other way round: the record \
             boundaries are not part of the message",
            "Only the handshake bytes go into the transcript — never the five-byte record \
             headers, however the message was fragmented",
        ],
        examples: examples,
        tests: vec![
            Test::new(
                "a ClientHello written one byte at a time is answered",
                byte_at_a_time,
            ),
            Test::new(
                "a ClientHello split into two TCP writes is answered",
                two_writes,
            ),
            Test::new(
                "a ClientHello split across three records is answered",
                three_records,
            ),
            Test::new(
                "a ClientHello whose first record holds only the header bytes is answered",
                header_only_first,
            ),
            Test::new(
                "the handshake completes normally after a fragmented hello",
                completes,
            ),
            Test::new(
                "the transcript is over the message, not the records it arrived in",
                transcript_ignores_records,
            ),
        ],
    }
}

/// Write a ClientHello as `chunks` records and read the first record that comes back.
async fn fragmented_hello(
    ctx: &crate::stages::Ctx,
    records: usize,
) -> Result<Record, Failure> {
    let message = hello_message(&ctx.config()).map_err(crate::stages::harness)?;
    let mut conn = ctx.connect().await?;
    let per = message.len().div_ceil(records.max(1));
    for chunk in message.chunks(per) {
        conn.write_raw(&Record::build(22, LEGACY_VERSION_TLS12, chunk))
            .await
            .map_err(Failure::tls)?;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    conn.read_record().await.map_err(|e| {
        Failure::tls(e).note(format!(
            "after a ClientHello split across {records} handshake records"
        ))
    })
}

tls_test!(byte_at_a_time, |ctx| {
    let message = hello_message(&ctx.config()).map_err(crate::stages::harness)?;
    let record = Record::build(22, LEGACY_VERSION_TLS12, &message);
    let mut conn = ctx.connect().await?;
    for byte in &record {
        conn.write_raw(std::slice::from_ref(byte))
            .await
            .map_err(Failure::tls)?;
    }
    let answer = conn.read_record().await.map_err(|e| {
        Failure::tls(e).note("after a ClientHello written one byte per TCP write")
    })?;
    let mut c = Check::new("the answer to a hello written one byte at a time");
    c.block("the server's first record", &answer.raw);
    c.note(format!("{} separate write() calls", record.len()));
    c.eq(
        "record.type",
        ContentType::Handshake.as_u8(),
        answer.content_type.as_u8(),
    );
    c.finish()
});

tls_test!(two_writes, |ctx| {
    let message = hello_message(&ctx.config()).map_err(crate::stages::harness)?;
    let record = Record::build(22, LEGACY_VERSION_TLS12, &message);
    let split = record.len() / 2;
    let mut conn = ctx.connect().await?;
    conn.write_raw(&record[..split]).await.map_err(Failure::tls)?;
    tokio::time::sleep(Duration::from_millis(20)).await;
    conn.write_raw(&record[split..]).await.map_err(Failure::tls)?;
    let answer = conn
        .read_record()
        .await
        .map_err(|e| Failure::tls(e).note("after a hello split across two TCP writes"))?;
    let mut c = Check::new("the answer to a hello split across two TCP writes");
    c.block("the server's first record", &answer.raw);
    c.note(format!(
        "the first write carried {split} bytes, the second {}",
        record.len() - split
    ));
    c.eq(
        "record.type",
        ContentType::Handshake.as_u8(),
        answer.content_type.as_u8(),
    );
    c.finish()
});

tls_test!(three_records, |ctx| {
    let answer = fragmented_hello(ctx, 3).await?;
    let mut c = Check::new("the answer to a hello spread over three handshake records");
    c.block("the server's first record", &answer.raw);
    c.eq(
        "record.type",
        ContentType::Handshake.as_u8(),
        answer.content_type.as_u8(),
    );
    c.finish()
});

tls_test!(header_only_first, |ctx| {
    let message = hello_message(&ctx.config()).map_err(crate::stages::harness)?;
    let mut conn = ctx.connect().await?;
    // The first record carries the four handshake header bytes and nothing else: the
    // server now knows a 500-byte message is coming and has none of it.
    conn.write_raw(&Record::build(22, LEGACY_VERSION_TLS12, &message[..4]))
        .await
        .map_err(Failure::tls)?;
    tokio::time::sleep(Duration::from_millis(20)).await;
    conn.write_raw(&Record::build(22, LEGACY_VERSION_TLS12, &message[4..]))
        .await
        .map_err(Failure::tls)?;
    let answer = conn.read_record().await.map_err(|e| {
        Failure::tls(e).note("after a hello whose first record held only the four header bytes")
    })?;
    let mut c = Check::new("the answer to a header-only first record");
    c.block("the server's first record", &answer.raw);
    c.eq(
        "record.type",
        ContentType::Handshake.as_u8(),
        answer.content_type.as_u8(),
    );
    c.finish()
});

tls_test!(completes, |ctx| {
    let conn = ctx.connect().await?;
    let mut client = Client::over(conn, ctx.config());
    let (hello, message) = client.prepare_client_hello().map_err(Failure::tls)?;
    // Write the hello in four records by hand, then let the driver take over.
    let per = message.len().div_ceil(4);
    for chunk in message.chunks(per) {
        client
            .conn
            .write_raw(&Record::build(22, LEGACY_VERSION_TLS12, chunk))
            .await
            .map_err(Failure::tls)?;
    }
    client.note_client_hello(hello, message);
    client.maybe_send_ccs().await.map_err(Failure::tls)?;
    match async {
        client.read_server_hello().await?;
        client.read_server_flight().await?;
        client.send_client_finished().await
    }
    .await
    {
        Ok(()) => {}
        Err(e) => {
            return Err(crate::stages::handshake_failure(e, &client)
                .note("the hello was written as four records, which must change nothing"))
        }
    }
    let answer = client
        .echo_line("fragmented")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("the echo after a fragmented ClientHello");
    c.eq("echo", "detnemgarf".to_string(), answer);
    c.finish()
});

tls_test!(transcript_ignores_records, |ctx| {
    // Two handshakes with the same ClientHello bytes, one in a single record and one in
    // five. If the record headers had leaked into the transcript, the second one could not
    // have verified the server's Finished — which the driver checks for us.
    //
    // The first connection is finished with and dropped before the second is opened: a
    // server that handles connections one at a time (the reference does) would otherwise
    // never reach the second one.
    let config = ctx.config();
    let (whole_hello_bytes, whole_hash) = {
        let whole = ctx.handshake_with(config.clone()).await?;
        (
            whole.client_hello_bytes.clone(),
            whole.hash_after_client_hello.clone(),
        )
    };
    let conn = ctx.connect().await?;
    let mut client = Client::over(conn, config);
    let (hello, message) = client.prepare_client_hello().map_err(Failure::tls)?;
    for chunk in message.chunks(message.len().div_ceil(5)) {
        client
            .conn
            .write_raw(&Record::build(22, LEGACY_VERSION_TLS12, chunk))
            .await
            .map_err(Failure::tls)?;
    }
    client.note_client_hello(hello, message.clone());
    client.maybe_send_ccs().await.map_err(Failure::tls)?;
    if let Err(e) = async {
        client.read_server_hello().await?;
        client.read_server_flight().await
    }
    .await
    {
        return Err(crate::stages::handshake_failure(e, &client).note(
            "the server's Finished did not verify, which is what happens when record headers \
             end up in the transcript",
        ));
    }
    let mut c = Check::new("that record boundaries stay out of the transcript");
    c.bytes_eq(
        "client_hello bytes hashed into the transcript",
        &whole_hello_bytes,
        &client.client_hello_bytes,
    );
    c.eq(
        "transcript hash after client_hello",
        crate::tls::hex(&whole_hash),
        crate::tls::hex(&client.hash_after_client_hello),
    );
    c.finish()
});

fn split_hello(env: &ExampleEnv) -> Result<Vec<u8>, String> {
    let message = hello_message(&env.config())?;
    let mut out = Vec::new();
    for chunk in message.chunks(message.len().div_ceil(3)) {
        out.extend_from_slice(&Record::build(22, LEGACY_VERSION_TLS12, chunk));
    }
    Ok(out)
}

fn config(env: &ExampleEnv) -> ClientConfig {
    env.config()
}

fn examples() -> Vec<ExampleSpec> {
    let _ = config;
    vec![
        ExampleSpec::raw(
            "One ClientHello, three handshake records",
            split_hello,
            Expect::Records(1),
        )
        .request(
            "The same ClientHello message as always, cut into three pieces and sent as three \
             handshake records back to back",
        )
        .response(
            "One ServerHello, exactly as if the hello had arrived in a single record. Nothing \
             about the answer changes.",
        )
        .note(
            "Look at the three record headers in the request: each has its own length, and the \
             handshake header's uint24 length spans all three. Reassemble, then parse.",
        ),
        ExampleSpec::text("What goes into the transcript")
            .request("A ClientHello that arrived in three records")
            .response(
                "Transcript-Hash(ClientHello) is over the handshake message — msg_type, the \
                 uint24 length and the body — and over nothing else.",
            )
            .note(
                "The five-byte record headers never enter the transcript. A server that hashes \
                 what it read off the socket will pass every test until the first fragmented \
                 hello, and then fail the Finished check with no obvious reason.",
            ),
    ]
}
