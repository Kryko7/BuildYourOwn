//! Stage 08 — Pipelined requests (many frames in one write). **[ext]**

use crate::assert::{Check, Failure};
use crate::examples::{ExampleSpec, Wire};
use crate::kafka_test;
use crate::proto::Conn;
use crate::stages::{
    api_versions_body_v4, api_versions_request, correlation_id_of, raw_flexible_frame, Stage, Test,
    API_VERSIONS_KEY, API_VERSIONS_V4,
};

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 8,
        slug: "pipelined_requests",
        name: "Pipelined requests",
        ext: true,
        hints: &[
            "A client may write several frames before reading any response",
            "Buffer incoming bytes: one read() can hold two requests, or half of one",
            "Answer in the order the requests arrived — Kafka guarantees per-connection order",
            "Do not start reading a new frame until message_size bytes of the current one are in",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("ten pipelined requests come back in order", ten_in_order).ext(),
            Test::new(
                "two frames in a single write are both answered",
                two_in_one_write,
            )
            .ext(),
            Test::new("a frame split across two writes is answered", split_frame).ext(),
            Test::new("a byte-at-a-time request is answered", byte_at_a_time).ext(),
            Test::new("fifty pipelined requests keep their order", fifty_in_order).ext(),
        ],
    }
}

fn frame(id: i32) -> Vec<u8> {
    let payload = raw_flexible_frame(
        API_VERSIONS_KEY,
        API_VERSIONS_V4,
        id,
        Some("kafkatest"),
        &[],
        &api_versions_body_v4(),
    );
    let mut out = (payload.len() as i32).to_be_bytes().to_vec();
    out.extend_from_slice(&payload);
    out
}

async fn expect_ids(conn: &mut Conn, ids: &[i32], what: &str) -> Result<(), Failure> {
    let mut seen = Vec::new();
    for i in 0..ids.len() {
        let resp = conn.read_frame().await.map_err(|e| {
            Failure::proto(e, Some(conn)).note(format!("{what}: response {}", i + 1))
        })?;
        seen.push(correlation_id_of(&resp, conn)?);
    }
    let mut c = Check::new(what.to_string(), conn);
    c.eq("responses.correlation_ids", ids.to_vec(), seen);
    c.finish()
}

kafka_test!(ten_in_order, |ctx| {
    let mut conn = ctx.connect().await?;
    let ids: Vec<i32> = (0..10).map(|i| 11_000 + i).collect();
    let mut buf = Vec::new();
    for id in &ids {
        buf.extend_from_slice(&frame(*id));
    }
    conn.send_bytes(&buf)
        .await
        .map_err(|e| Failure::proto(e, Some(&conn)))?;
    expect_ids(&mut conn, &ids, "ten frames written at once").await
});

kafka_test!(two_in_one_write, |ctx| {
    let mut conn = ctx.connect().await?;
    let mut buf = frame(12_001);
    buf.extend_from_slice(&frame(12_002));
    conn.send_bytes(&buf)
        .await
        .map_err(|e| Failure::proto(e, Some(&conn)))?;
    expect_ids(&mut conn, &[12_001, 12_002], "two frames in one write").await
});

kafka_test!(split_frame, |ctx| {
    let mut conn = ctx.connect().await?;
    let f = frame(13_001);
    let (head, tail) = f.split_at(6);
    conn.send_bytes(head)
        .await
        .map_err(|e| Failure::proto(e, Some(&conn)))?;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    conn.send_bytes(tail)
        .await
        .map_err(|e| Failure::proto(e, Some(&conn)))?;
    expect_ids(&mut conn, &[13_001], "a frame split across two writes").await
});

kafka_test!(byte_at_a_time, |ctx| {
    let mut conn = ctx.connect().await?;
    let f = frame(14_001);
    for b in &f {
        conn.send_bytes(std::slice::from_ref(b))
            .await
            .map_err(|e| Failure::proto(e, Some(&conn)))?;
    }
    expect_ids(&mut conn, &[14_001], "a request sent one byte at a time").await
});

kafka_test!(fifty_in_order, |ctx| {
    let mut conn = ctx.connect().await?;
    let ids: Vec<i32> = (0..50).map(|i| 15_000 + i).collect();
    let mut buf = Vec::new();
    for id in &ids {
        buf.extend_from_slice(&frame(*id));
    }
    conn.send_bytes(&buf)
        .await
        .map_err(|e| Failure::proto(e, Some(&conn)))?;
    expect_ids(&mut conn, &ids, "fifty pipelined frames").await
});

/// Worked examples: several frames written before any answer is read.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![ExampleSpec::wire("Three frames in one write()", |env| {
        Ok(Wire::Frames(vec![
            env.payload(API_VERSIONS_V4, 101, &api_versions_request())?,
            env.payload(API_VERSIONS_V4, 102, &api_versions_request())?,
            env.payload(API_VERSIONS_V4, 103, &api_versions_request())?,
        ]))
    })
    .request(
        "Three ApiVersions v4 frames, correlation ids 101, 102 and 103, written to the \
             socket in a single write and without reading anything in between",
    )
    .response(
        "Three response frames in exactly that order — 101, 102, 103 — each a complete, \
             size-prefixed ApiVersions v4 response",
    )
    .note(
        "One read() can deliver three requests, or half of one. Drive the connection off a \
             buffer: while the buffer holds 4 + size bytes, take a frame out of it and answer. \
             Responses must keep request order; Kafka guarantees it per connection.",
    )]
}
