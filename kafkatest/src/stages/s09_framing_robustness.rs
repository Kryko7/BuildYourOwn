//! Stage 09 — Framing robustness: the broker must survive rubbish. **[ext]**

use crate::assert::Failure;
use crate::examples::{ExampleSpec, Wire};
use crate::kafka_test;
use crate::proto::ProtoError;
use crate::stages::{
    api_versions_body_v4, expect_still_serving, raw_flexible_frame, Stage, Test, API_VERSIONS_KEY,
    API_VERSIONS_V4,
};
use rand::Rng;
use std::time::Duration;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 9,
        slug: "framing_robustness",
        name: "Framing robustness",
        ext: true,
        hints: &[
            "Validate message_size before allocating: reject anything absurd instead of \
             reserving gigabytes",
            "A truncated frame is a closed connection, never a crashed process",
            "One bad connection must not take down the accept loop or the other clients",
            "A half-closed socket (client shut down its write side) is a normal end of stream",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new(
                "a size prefix larger than the bytes sent does not crash the broker",
                oversized,
            )
            .ext(),
            Test::new("a zero-length frame does not crash the broker", zero_length).ext(),
            Test::new(
                "a negative size prefix does not crash the broker",
                negative_size,
            )
            .ext(),
            Test::new(
                "an enormous size prefix does not exhaust memory",
                enormous_size,
            )
            .ext(),
            Test::new("garbage bytes do not crash the broker", garbage).ext(),
            Test::new(
                "a truncated header does not crash the broker",
                truncated_header,
            )
            .ext(),
            Test::new("a half-closed connection is handled cleanly", half_close).ext(),
            Test::new(
                "other connections keep working while one is abused",
                others_unaffected,
            )
            .ext(),
        ],
    }
}

/// Send bytes, tolerate any reaction (response, close, reset), then prove the broker lives.
async fn abuse(ctx: &crate::stages::Ctx, bytes: &[u8], what: &str) -> Result<(), Failure> {
    let mut conn = ctx.connect().await?;
    match conn.send_bytes(bytes).await {
        Ok(()) => {}
        Err(ProtoError::Closed) | Err(ProtoError::Io(_)) => {}
        Err(e) => return Err(Failure::proto(e, Some(&conn)).note(format!("while sending {what}"))),
    }
    // Whatever the broker does here is fine: answer, close, or reset. It must not die.
    let _ = conn.read_silence(Duration::from_millis(300)).await;
    drop(conn);
    expect_still_serving(ctx, what).await
}

kafka_test!(oversized, |ctx| {
    let mut bytes = 1000i32.to_be_bytes().to_vec();
    bytes.extend_from_slice(&[0u8; 10]);
    abuse(ctx, &bytes, "a frame claiming 1000 bytes but carrying 10").await
});

kafka_test!(zero_length, |ctx| {
    abuse(ctx, &0i32.to_be_bytes(), "a zero-length frame").await
});

kafka_test!(negative_size, |ctx| {
    abuse(ctx, &(-1i32).to_be_bytes(), "a frame with size -1").await
});

kafka_test!(enormous_size, |ctx| {
    let mut bytes = i32::MAX.to_be_bytes().to_vec();
    bytes.extend_from_slice(b"not two gigabytes");
    abuse(ctx, &bytes, "a frame claiming 2 GiB").await
});

kafka_test!(garbage, |ctx| {
    let len: usize = 64;
    let mut body: Vec<u8> = (0..len).map(|_| ctx.rng.random::<u8>()).collect();
    let mut bytes = (len as i32).to_be_bytes().to_vec();
    bytes.append(&mut body);
    abuse(ctx, &bytes, "64 random bytes in a well-formed frame").await
});

kafka_test!(truncated_header, |ctx| {
    let payload = raw_flexible_frame(
        API_VERSIONS_KEY,
        API_VERSIONS_V4,
        1,
        Some("kafkatest"),
        &[],
        &api_versions_body_v4(),
    );
    let cut = payload.len() / 2;
    let mut bytes = (payload.len() as i32).to_be_bytes().to_vec();
    bytes.extend_from_slice(&payload[..cut]);
    abuse(ctx, &bytes, "half of a valid request").await
});

kafka_test!(half_close, |ctx| {
    let mut conn = ctx.connect().await?;
    let payload = raw_flexible_frame(
        API_VERSIONS_KEY,
        API_VERSIONS_V4,
        4242,
        Some("kafkatest"),
        &[],
        &api_versions_body_v4(),
    );
    conn.send_frame(&payload)
        .await
        .map_err(|e| Failure::proto(e, Some(&conn)))?;
    let _ = conn.shutdown_write().await;
    // A response or a clean close are both correct; a hang or a crash are not.
    let _ = conn.expect_closed(Duration::from_millis(1000)).await;
    drop(conn);
    expect_still_serving(ctx, "a half-closed connection").await
});

kafka_test!(others_unaffected, |ctx| {
    let mut good = ctx.connect().await?;
    let payload = raw_flexible_frame(
        API_VERSIONS_KEY,
        API_VERSIONS_V4,
        555,
        Some("kafkatest"),
        &[],
        &api_versions_body_v4(),
    );
    good.send_frame(&payload)
        .await
        .map_err(|e| Failure::proto(e, Some(&good)))?;

    {
        let mut bad = ctx.connect().await?;
        let _ = bad.send_bytes(&[0xff, 0xff, 0xff, 0xff, 1, 2, 3]).await;
    }

    let resp = good
        .read_frame()
        .await
        .map_err(|e| Failure::proto(e, Some(&good)).note("the well-behaved connection broke"))?;
    let mut c = crate::assert::Check::new("the well-behaved connection", &good);
    c.mark(0..4);
    c.eq(
        "response.correlation_id",
        555i32,
        crate::stages::correlation_id_of(&resp, &good)?,
    );
    c.finish()?;
    expect_still_serving(ctx, "a peer sent a negative size prefix").await
});

/// Worked examples: rubbish on the wire, and what surviving it looks like.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("A negative size prefix", |_env| {
            Ok(Wire::Raw(vec![0xff, 0xff, 0xff, 0xff]))
        })
        .expect_closed()
        .request("Four bytes: ff ff ff ff — a size prefix of -1")
        .response(
            "No response at all. Apache Kafka closes this connection and goes on serving \
             every other one.",
        )
        .note(
            "message_size is a *signed* int32. Validate it before you allocate: reject \
             anything negative or absurd, close that one connection, and keep the accept loop \
             alive. Reserving a buffer of the claimed size is how a broker dies here.",
        ),
        ExampleSpec::wire("A size prefix that promises more than it sends", |_env| {
            let mut bytes = 1000i32.to_be_bytes().to_vec();
            bytes.extend_from_slice(&[0u8; 10]);
            Ok(Wire::Raw(bytes))
        })
        .expect_silence()
        .request("A size prefix of 1000 followed by only 10 bytes, and then nothing")
        .response(
            "Nothing, and no disconnect either: the broker is still waiting for the other 990 \
             bytes of a frame it has been promised.",
        )
        .note(
            "A partial frame is normal — TCP splits wherever it likes. Buffer what arrived and \
             wait for the rest; do not answer, and do not treat the short read as an error. \
             The connection ends when the client gives up.",
        ),
    ]
}
