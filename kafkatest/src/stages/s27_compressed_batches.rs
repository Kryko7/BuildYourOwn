//! Stage 27 — Compressed record batches: gzip, snappy, lz4, zstd.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::fixtures::{rec, FixtureSpec, TopicSpec};
use crate::kafka_test;
use crate::proto::records::{codec_name, RecordBatch, GZIP, LZ4, SNAPPY, ZSTD};
use crate::stages::{fetch_request, proto_fail, Ctx, Stage, Test, FETCH_V16, NONE};

/// The four codecs, in the order the fixture writes them.
const CODECS: &[i16] = &[GZIP, SNAPPY, LZ4, ZSTD];

/// One two-record batch per codec: offsets 0-1 gzip, 2-3 snappy, 4-5 lz4, 6-7 zstd.
fn one_batch_per_codec() -> FixtureSpec {
    FixtureSpec::with(
        TopicSpec::new("t1", 1)
            .with_compressed_batch(0, GZIP, vec![rec("gzip-0"), rec("gzip-1")])
            .with_compressed_batch(0, SNAPPY, vec![rec("snappy-0"), rec("snappy-1")])
            .with_compressed_batch(0, LZ4, vec![rec("lz4-0"), rec("lz4-1")])
            .with_compressed_batch(0, ZSTD, vec![rec("zstd-0"), rec("zstd-1")]),
    )
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 27,
        slug: "compressed_batches",
        name: "Compressed batches: gzip, snappy, lz4, zstd",
        ext: true,
        hints: &[
            "The codec lives in attribute bits 0-2 of the batch header: 1 gzip, 2 snappy, \
             3 lz4, 4 zstd — everything from recordCount onwards is the compressed blob",
            "Never recompress or re-encode on the fetch path: copy the stored bytes out, \
             attribute bits and all, and the CRC still verifies",
            "The record count in the header counts the uncompressed records, so you do not \
             have to decompress anything to serve a fetch",
            "Kafka's framings are specific: snappy is xerial-framed, lz4 is the LZ4 frame \
             format, gzip and zstd are plain streams",
        ],
        examples: wire_examples,
        tests: vec![
            Test::new("a gzip batch keeps its codec bits", gzip_bits)
                .with_fixtures(one_batch_per_codec)
                .ext(),
            Test::new("a snappy batch keeps its codec bits", snappy_bits)
                .with_fixtures(one_batch_per_codec)
                .ext(),
            Test::new("an lz4 batch keeps its codec bits", lz4_bits)
                .with_fixtures(one_batch_per_codec)
                .ext(),
            Test::new("a zstd batch keeps its codec bits", zstd_bits)
                .with_fixtures(one_batch_per_codec)
                .ext(),
            Test::new(
                "every compressed payload decompresses to what was written",
                values,
            )
            .with_fixtures(one_batch_per_codec)
            .ext(),
            Test::new(
                "offsets and the watermark span the compressed batches",
                offsets,
            )
            .with_fixtures(one_batch_per_codec)
            .ext(),
            Test::new(
                "the bytes are the stored ones, not re-encoded",
                matches_segment,
            )
            .with_fixtures(one_batch_per_codec)
            .ext(),
        ],
    }
}

/// Fetch the whole partition and decode (and decompress) every batch.
async fn fetch_all(ctx: &Ctx) -> Result<(Vec<RecordBatch>, Vec<u8>, i64), Failure> {
    let t = ctx.topic("t1")?.clone();
    let mut conn = ctx.connect().await?;
    let req = fetch_request(&[(t.id, 0, 0)], 500);
    let resp = conn
        .request(FETCH_V16, &req)
        .await
        .map_err(|e| proto_fail(e, &conn))?;
    let mut c = Check::new(format!("fetching the compressed topic '{}'", t.name), &conn);
    c.eq("response.error_code", NONE, resp.error_code);
    let Some(p) = resp.responses.first().and_then(|r| r.partitions.first()) else {
        c.that(
            "response.responses[0].partitions[0]",
            "one partition entry",
            false,
            "none",
        );
        return Err(c
            .finish()
            .err()
            .unwrap_or_else(|| Failure::harness("no partition entry")));
    };
    c.eq(
        "response.responses[0].partitions[0].error_code",
        NONE,
        p.error_code,
    );
    c.finish()?;
    let bytes = p.records.as_ref().map(|r| r.to_vec()).unwrap_or_default();
    let high_watermark = p.high_watermark;
    // decode_all verifies the CRC-32C and decompresses each payload; a broker that
    // re-encoded the batch fails here rather than in a value comparison.
    let batches = RecordBatch::decode_all(&bytes).map_err(|e| {
        Failure::harness(format!(
            "the returned compressed batches do not decode: {e:#}"
        ))
    })?;
    Ok((batches, bytes, high_watermark))
}

/// Assert that the `index`-th returned batch carries `codec` and its two records.
async fn expect_codec(ctx: &Ctx, index: usize, codec: i16) -> Result<(), Failure> {
    let (batches, _, _) = fetch_all(ctx).await?;
    let name = codec_name(codec);
    let mut c = Check::detached(format!("the {name} batch of the partition"));
    c.note(format!(
        "the fixture holds one batch per codec, in the order {:?}",
        CODECS.iter().map(|c| codec_name(*c)).collect::<Vec<&str>>()
    ));
    c.eq("records.batches.len", CODECS.len(), batches.len());
    let Some(b) = batches.get(index) else {
        return c.finish();
    };
    c.eq(
        &format!("records.batches[{index}].attributes & 0x07"),
        codec,
        b.compression(),
    );
    c.eq(
        &format!("records.batches[{index}].records.len"),
        2usize,
        b.records.len(),
    );
    c.eq(
        &format!("records.batches[{index}].last_offset_delta"),
        1i32,
        b.last_offset_delta,
    );
    c.finish()
}

kafka_test!(gzip_bits, |ctx| { expect_codec(ctx, 0, GZIP).await });
kafka_test!(snappy_bits, |ctx| { expect_codec(ctx, 1, SNAPPY).await });
kafka_test!(lz4_bits, |ctx| { expect_codec(ctx, 2, LZ4).await });
kafka_test!(zstd_bits, |ctx| { expect_codec(ctx, 3, ZSTD).await });

kafka_test!(values, |ctx| {
    let (batches, _, _) = fetch_all(ctx).await?;
    let got: Vec<String> = batches
        .iter()
        .flat_map(|b| b.records.iter())
        .map(|r| String::from_utf8_lossy(r.value.as_deref().unwrap_or_default()).to_string())
        .collect();
    let want: Vec<String> = CODECS
        .iter()
        .flat_map(|c| {
            let n = codec_name(*c);
            vec![format!("{n}-0"), format!("{n}-1")]
        })
        .collect();
    let mut c = Check::detached("the record values behind the four compressed payloads");
    c.eq("records[*].value", want, got);
    c.finish()
});

kafka_test!(offsets, |ctx| {
    let (batches, _, high_watermark) = fetch_all(ctx).await?;
    let mut c = Check::detached("the offsets of four two-record compressed batches");
    c.note("a compressed batch still advertises its offsets in the plain header");
    c.eq(
        "records.batches[*].base_offset",
        vec![0i64, 2, 4, 6],
        batches.iter().map(|b| b.base_offset).collect::<Vec<i64>>(),
    );
    c.eq(
        "response.responses[0].partitions[0].high_watermark",
        8i64,
        high_watermark,
    );
    c.finish()
});

kafka_test!(matches_segment, |ctx| {
    let t = ctx.topic("t1")?.clone();
    let (_, bytes, _) = fetch_all(ctx).await?;
    let segment = ctx.fixtures.segment_bytes(&t.name, 0)?;
    let mut c = Check::detached("the fetched compressed bytes against the log segment");
    c.note(format!(
        "segment: {}",
        ctx.fixtures.segment_path(&t.name, 0).display()
    ));
    c.note("decompressing and recompressing changes the payload and breaks the CRC");
    let prefix = segment.get(..bytes.len()).unwrap_or(&segment);
    c.bytes_eq("records", prefix, &bytes);
    c.finish()
});

/// One gzip batch on its own, so the attributes field is easy to find in the hex.
fn one_gzip_batch() -> FixtureSpec {
    FixtureSpec::with(TopicSpec::new("t1", 1).with_compressed_batch(
        0,
        GZIP,
        vec![rec("gzip-0"), rec("gzip-1")],
    ))
}

/// Worked examples: a gzip batch served untouched, and four codecs in one partition.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("A gzip batch comes back still compressed", |env| {
            env.request(
                FETCH_V16,
                271,
                &fetch_request(&[(env.id("t1")?, 0, 0)], 500),
            )
        })
        .with_fixtures(one_gzip_batch)
        .request(
            "Fetch v16, correlation id 271: partition 0 from offset 0 of a topic holding one \
             gzip-compressed batch of two records, 'gzip-0' and 'gzip-1'",
        )
        .response(
            "error_code 0, high_watermark 2, and the stored batch byte for byte: attributes \
             0x0001 — bits 0-2 carry the codec, 1 is gzip — last_offset_delta 1, a record count \
             of 2, and a gzip stream where the plain records would otherwise be",
        )
        .note(
            "Serving this fetch needs no decompression at all: the batch header is plain, and \
             the record count already says how many records are inside the blob. Decompressing \
             and re-encoding would change the bytes and invalidate the CRC.",
        ),
        ExampleSpec::wire("Four codecs in one partition", |env| {
            env.request(
                FETCH_V16,
                272,
                &fetch_request(&[(env.id("t1")?, 0, 0)], 500),
            )
        })
        .with_fixtures(one_batch_per_codec)
        .request(
            "The same request against a partition holding four two-record batches: gzip at \
             offsets 0-1, snappy at 2-3, lz4 at 4-5, zstd at 6-7",
        )
        .response(
            "error_code 0, high_watermark 8, and the four batches concatenated in offset order, \
             each keeping its own attributes — 0x0001, 0x0002, 0x0003, 0x0004",
        )
        .note(
            "Compression is a property of a batch, not of a topic or a partition, so any \
             mixture is legal. Kafka's framings are specific — snappy is xerial-framed, lz4 is \
             the LZ4 frame format — but the fetch path never has to know which is which.",
        ),
    ]
}
