//! RecordBatch v2 writer/reader.
//!
//! Kafka's on-disk (and on-the-wire) record format, written by hand so that fixtures can
//! control every header field — base offset, leader epoch, producer id, the control flag —
//! which the encoder in `kafka-protocol` derives instead of exposing. The output is
//! validated two ways: by round-tripping through the reader below and through
//! `kafka-protocol`'s decoder (unit tests), and by `kafka-dump-log.sh --deep-iteration`
//! from the reference tarball (`tests/records_dump_log.rs`).
//!
//! ```text
//! baseOffset           int64
//! batchLength          int32   bytes after this field
//! partitionLeaderEpoch int32
//! magic                int8    always 2
//! crc                  uint32  CRC-32C of everything after this field
//! attributes           int16   bits 0-2 compression, 3 timestampType, 4 transactional,
//!                              5 control, 6 deleteHorizon
//! lastOffsetDelta      int32
//! baseTimestamp        int64
//! maxTimestamp         int64
//! producerId           int64
//! producerEpoch        int16
//! baseSequence         int32
//! recordCount          int32
//! records              [Record]
//! ```

use anyhow::{bail, Context, Result};

/// Magic byte of the only batch format this harness writes.
pub const MAGIC_V2: i8 = 2;
/// Producer id meaning "not idempotent".
pub const NO_PRODUCER_ID: i64 = -1;
/// Producer epoch meaning "not idempotent".
pub const NO_PRODUCER_EPOCH: i16 = -1;
/// Base sequence meaning "not idempotent".
pub const NO_SEQUENCE: i32 = -1;

/// Attribute bits 0-2: the compression codec.
pub const COMPRESSION_MASK: i16 = 0x07;
/// Codec 0: the records are stored as they are.
pub const NO_COMPRESSION: i16 = 0;
/// Codec 1: gzip (a plain gzip stream).
pub const GZIP: i16 = 1;
/// Codec 2: snappy, in the xerial block framing Kafka's `SnappyCompression` writes.
pub const SNAPPY: i16 = 2;
/// Codec 3: lz4, in the LZ4 *frame* format (not a bare block).
pub const LZ4: i16 = 3;
/// Codec 4: zstd (a standard zstd frame).
pub const ZSTD: i16 = 4;

/// The name Kafka's `compression.type` uses for a codec id.
pub fn codec_name(codec: i16) -> &'static str {
    match codec {
        NO_COMPRESSION => "none",
        GZIP => "gzip",
        SNAPPY => "snappy",
        LZ4 => "lz4",
        ZSTD => "zstd",
        _ => "unknown",
    }
}

/// Compress a batch's record payload the way Kafka's `Compression` classes do.
///
/// The framing matters as much as the algorithm: snappy is xerial-framed, lz4 is the LZ4
/// *frame* format, gzip and zstd are plain streams. Anything else and the broker (or the
/// consumer) cannot read the batch back.
pub fn compress_records(codec: i16, raw: &[u8]) -> Result<Vec<u8>> {
    use kafka_protocol::compression::{Compressor, Gzip, Lz4, Snappy, Zstd};
    let mut out: Vec<u8> = Vec::new();
    let fill = |buf: &mut bytes::BytesMut| -> Result<()> {
        buf.extend_from_slice(raw);
        Ok(())
    };
    match codec {
        NO_COMPRESSION => return Ok(raw.to_vec()),
        GZIP => Gzip::compress(&mut out, fill)?,
        SNAPPY => Snappy::compress(&mut out, fill)?,
        LZ4 => Lz4::compress(&mut out, fill)?,
        ZSTD => Zstd::compress(&mut out, fill)?,
        other => bail!("unknown compression codec {other}"),
    }
    Ok(out)
}

/// Undo [`compress_records`].
pub fn decompress_records(codec: i16, payload: &[u8]) -> Result<Vec<u8>> {
    use bytes::Buf;
    use kafka_protocol::compression::{Decompressor, Gzip, Lz4, Snappy, Zstd};
    let mut input = bytes::Bytes::copy_from_slice(payload);
    let drain = |buf: &mut bytes::Bytes| -> Result<Vec<u8>> {
        let n = buf.remaining();
        Ok(buf.copy_to_bytes(n).to_vec())
    };
    match codec {
        NO_COMPRESSION => Ok(payload.to_vec()),
        GZIP => Gzip::decompress(&mut input, drain),
        SNAPPY => Snappy::decompress(&mut input, drain),
        LZ4 => Lz4::decompress(&mut input, drain),
        ZSTD => Zstd::decompress(&mut input, drain),
        other => bail!("unknown compression codec {other}"),
    }
}

/// One record inside a batch. Deltas are relative to the batch's base offset/timestamp.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecordItem {
    /// Per-record attributes; always 0 in v2.
    pub attributes: i8,
    /// Timestamp relative to the batch's `base_timestamp`.
    pub timestamp_delta: i64,
    /// Offset relative to the batch's `base_offset`.
    pub offset_delta: i32,
    /// Record key, `None` for a null key.
    pub key: Option<Vec<u8>>,
    /// Record value, `None` for a null value (a tombstone).
    pub value: Option<Vec<u8>>,
    /// Record headers; a header value may be null.
    pub headers: Vec<(String, Option<Vec<u8>>)>,
}

impl RecordItem {
    /// A record with a value and no key or headers.
    pub fn value(v: impl Into<Vec<u8>>) -> Self {
        RecordItem {
            value: Some(v.into()),
            ..Default::default()
        }
    }

    /// A record with a key and a value.
    pub fn keyed(k: impl Into<Vec<u8>>, v: impl Into<Vec<u8>>) -> Self {
        RecordItem {
            key: Some(k.into()),
            value: Some(v.into()),
            ..Default::default()
        }
    }
}

/// A v2 record batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordBatch {
    /// Offset of the first record in the batch.
    pub base_offset: i64,
    /// Leader epoch the batch was appended in.
    pub partition_leader_epoch: i32,
    /// Batch attributes (compression codec, control flag, ...).
    pub attributes: i16,
    /// Offset of the last record relative to `base_offset`.
    pub last_offset_delta: i32,
    /// Timestamp of the first record.
    pub base_timestamp: i64,
    /// Largest timestamp in the batch.
    pub max_timestamp: i64,
    /// Producer id, `NO_PRODUCER_ID` when not idempotent.
    pub producer_id: i64,
    /// Producer epoch, `NO_PRODUCER_EPOCH` when not idempotent.
    pub producer_epoch: i16,
    /// Base sequence, `NO_SEQUENCE` when not idempotent.
    pub base_sequence: i32,
    /// The records; ignored when `record_count_override` is set.
    pub records: Vec<RecordItem>,
    /// Force a wrong record count into the header (framing/robustness tests only).
    pub record_count_override: Option<i32>,
    /// Force a wrong CRC into the header (corruption tests only).
    pub crc_override: Option<u32>,
}

impl Default for RecordBatch {
    fn default() -> Self {
        RecordBatch {
            base_offset: 0,
            partition_leader_epoch: 0,
            attributes: 0,
            last_offset_delta: 0,
            base_timestamp: 0,
            max_timestamp: 0,
            producer_id: NO_PRODUCER_ID,
            producer_epoch: NO_PRODUCER_EPOCH,
            base_sequence: NO_SEQUENCE,
            records: Vec::new(),
            record_count_override: None,
            crc_override: None,
        }
    }
}

impl RecordBatch {
    /// Build an uncompressed batch whose offsets and timestamps run from the given bases.
    pub fn of(base_offset: i64, base_timestamp: i64, values: Vec<RecordItem>) -> Self {
        let mut b = RecordBatch {
            base_offset,
            base_timestamp,
            max_timestamp: base_timestamp,
            ..Default::default()
        };
        for (i, mut r) in values.into_iter().enumerate() {
            r.offset_delta = i as i32;
            r.timestamp_delta = 0;
            b.records.push(r);
        }
        b.last_offset_delta = b.records.len().saturating_sub(1) as i32;
        b
    }

    /// Offset one past the last record of the batch.
    pub fn next_offset(&self) -> i64 {
        self.base_offset + self.records.len() as i64
    }

    /// True when the control bit is set (the batch holds control records).
    pub fn is_control(&self) -> bool {
        self.attributes & (1 << 5) != 0
    }

    /// Compression codec id from the attribute bits (0 = none).
    pub fn compression(&self) -> i16 {
        self.attributes & COMPRESSION_MASK
    }

    /// The same batch with a compression codec in its attribute bits.
    pub fn with_compression(mut self, codec: i16) -> Self {
        self.attributes = (self.attributes & !COMPRESSION_MASK) | (codec & COMPRESSION_MASK);
        self
    }

    /// Serialize the batch, computing `batchLength` and the CRC-32C.
    ///
    /// Compressing an in-memory buffer cannot fail in practice; if it ever did, the batch
    /// is emitted uncompressed with the codec bits cleared rather than as unreadable bytes.
    pub fn encode(&self) -> Vec<u8> {
        match self.try_encode() {
            Ok(bytes) => bytes,
            Err(_) => {
                let plain = self.clone().with_compression(NO_COMPRESSION);
                plain.try_encode().unwrap_or_default()
            }
        }
    }

    /// Serialize the batch, compressing the record payload when the attributes ask for it.
    pub fn try_encode(&self) -> Result<Vec<u8>> {
        let mut payload = Vec::with_capacity(self.records.len() * 32);
        for r in &self.records {
            encode_record(&mut payload, r);
        }
        let codec = self.compression();
        if codec != NO_COMPRESSION {
            payload = compress_records(codec, &payload).with_context(|| {
                format!("compressing the batch payload with {}", codec_name(codec))
            })?;
        }

        let mut body = Vec::with_capacity(payload.len() + 64);
        // Everything the CRC covers: attributes .. records.
        body.extend_from_slice(&self.attributes.to_be_bytes());
        body.extend_from_slice(&self.last_offset_delta.to_be_bytes());
        body.extend_from_slice(&self.base_timestamp.to_be_bytes());
        body.extend_from_slice(&self.max_timestamp.to_be_bytes());
        body.extend_from_slice(&self.producer_id.to_be_bytes());
        body.extend_from_slice(&self.producer_epoch.to_be_bytes());
        body.extend_from_slice(&self.base_sequence.to_be_bytes());
        let count = self
            .record_count_override
            .unwrap_or(self.records.len() as i32);
        body.extend_from_slice(&count.to_be_bytes());
        body.extend_from_slice(&payload);

        let crc = self.crc_override.unwrap_or_else(|| crc32c::crc32c(&body));

        let mut out = Vec::with_capacity(body.len() + 21);
        out.extend_from_slice(&self.base_offset.to_be_bytes());
        // batchLength counts partitionLeaderEpoch(4) + magic(1) + crc(4) + body.
        let batch_length = (4 + 1 + 4 + body.len()) as i32;
        out.extend_from_slice(&batch_length.to_be_bytes());
        out.extend_from_slice(&self.partition_leader_epoch.to_be_bytes());
        out.push(MAGIC_V2 as u8);
        out.extend_from_slice(&crc.to_be_bytes());
        out.extend_from_slice(&body);
        Ok(out)
    }

    /// Decode one batch, returning it and the number of bytes consumed.
    pub fn decode(buf: &[u8]) -> Result<(RecordBatch, usize)> {
        let mut c = Cursor::new(buf);
        let base_offset = c.i64().context("batch.base_offset")?;
        let batch_length = c.i32().context("batch.batch_length")?;
        if batch_length < 0 {
            bail!("batch.batch_length is negative ({batch_length})");
        }
        let after_len = c.pos;
        let total = after_len + batch_length as usize;
        if total > buf.len() {
            bail!(
                "batch claims {} bytes but only {} are available",
                total,
                buf.len()
            );
        }
        let partition_leader_epoch = c.i32().context("batch.partition_leader_epoch")?;
        let magic = c.i8().context("batch.magic")?;
        if magic != MAGIC_V2 {
            bail!("unsupported record batch magic {magic}, only v2 is supported");
        }
        let crc = c.u32().context("batch.crc")?;
        let crc_start = c.pos;
        let actual_crc = crc32c::crc32c(&buf[crc_start..total]);
        if actual_crc != crc {
            bail!("batch.crc mismatch: header says {crc}, computed {actual_crc}");
        }
        let attributes = c.i16().context("batch.attributes")?;
        let last_offset_delta = c.i32().context("batch.last_offset_delta")?;
        let base_timestamp = c.i64().context("batch.base_timestamp")?;
        let max_timestamp = c.i64().context("batch.max_timestamp")?;
        let producer_id = c.i64().context("batch.producer_id")?;
        let producer_epoch = c.i16().context("batch.producer_epoch")?;
        let base_sequence = c.i32().context("batch.base_sequence")?;
        let count = c.i32().context("batch.record_count")?;
        let codec = attributes & COMPRESSION_MASK;
        let mut records = Vec::new();
        if codec != NO_COMPRESSION {
            // The compressed payload runs from here to the end of the batch; the record
            // count in the header still counts the *uncompressed* records.
            let payload = buf.get(c.pos..total).unwrap_or_default();
            let name = codec_name(codec);
            let raw = decompress_records(codec, payload)
                .with_context(|| format!("decompressing a {name} batch of {count} records"))?;
            let mut rc = Cursor::new(&raw);
            for i in 0..count.max(0) {
                records.push(decode_record(&mut rc).with_context(|| format!("records[{i}]"))?);
            }
        } else if count > 0 {
            for i in 0..count {
                records.push(decode_record(&mut c).with_context(|| format!("records[{i}]"))?);
            }
        }
        Ok((
            RecordBatch {
                base_offset,
                partition_leader_epoch,
                attributes,
                last_offset_delta,
                base_timestamp,
                max_timestamp,
                producer_id,
                producer_epoch,
                base_sequence,
                records,
                record_count_override: None,
                crc_override: None,
            },
            total,
        ))
    }

    /// Decode every batch in a buffer, stopping cleanly at a truncated tail.
    pub fn decode_all(buf: &[u8]) -> Result<Vec<RecordBatch>> {
        let mut out = Vec::new();
        let mut at = 0usize;
        while at + 12 <= buf.len() {
            let (b, used) =
                RecordBatch::decode(&buf[at..]).with_context(|| format!("batch at byte {at}"))?;
            at += used;
            out.push(b);
        }
        Ok(out)
    }
}

/// Concatenate encoded batches, as a log segment or a fetch response body does.
pub fn encode_batches(batches: &[RecordBatch]) -> Vec<u8> {
    let mut out = Vec::new();
    for b in batches {
        out.extend_from_slice(&b.encode());
    }
    out
}

fn encode_record(out: &mut Vec<u8>, r: &RecordItem) {
    let mut body = Vec::with_capacity(16 + r.value.as_ref().map_or(0, |v| v.len()));
    body.push(r.attributes as u8);
    put_varlong(&mut body, r.timestamp_delta);
    put_varint(&mut body, r.offset_delta);
    put_nullable(&mut body, r.key.as_deref());
    put_nullable(&mut body, r.value.as_deref());
    put_varint(&mut body, r.headers.len() as i32);
    for (k, v) in &r.headers {
        put_varint(&mut body, k.len() as i32);
        body.extend_from_slice(k.as_bytes());
        put_nullable(&mut body, v.as_deref());
    }
    put_varint(out, body.len() as i32);
    out.extend_from_slice(&body);
}

fn decode_record(c: &mut Cursor<'_>) -> Result<RecordItem> {
    let len = c.varint().context("record.length")?;
    if len < 0 {
        bail!("record.length is negative ({len})");
    }
    let end = c.pos + len as usize;
    if end > c.buf.len() {
        bail!(
            "record claims {len} bytes, only {} left",
            c.buf.len() - c.pos
        );
    }
    let attributes = c.i8().context("record.attributes")?;
    let timestamp_delta = c.varlong().context("record.timestamp_delta")?;
    let offset_delta = c.varint().context("record.offset_delta")?;
    let key = c.nullable().context("record.key")?;
    let value = c.nullable().context("record.value")?;
    let nheaders = c.varint().context("record.header_count")?;
    if nheaders < 0 {
        bail!("record.header_count is negative ({nheaders})");
    }
    let mut headers = Vec::with_capacity(nheaders.min(64) as usize);
    for i in 0..nheaders {
        let klen = c
            .varint()
            .with_context(|| format!("headers[{i}].key_length"))?;
        if klen < 0 {
            bail!("headers[{i}].key_length is negative");
        }
        let kb = c
            .take(klen as usize)
            .with_context(|| format!("headers[{i}].key"))?;
        let k = String::from_utf8(kb.to_vec())
            .with_context(|| format!("headers[{i}].key is not utf-8"))?;
        let v = c
            .nullable()
            .with_context(|| format!("headers[{i}].value"))?;
        headers.push((k, v));
    }
    if c.pos != end {
        bail!(
            "record length {len} does not match the {} bytes decoded",
            end as i64 - (end - c.pos) as i64
        );
    }
    Ok(RecordItem {
        attributes,
        timestamp_delta,
        offset_delta,
        key,
        value,
        headers,
    })
}

fn put_nullable(out: &mut Vec<u8>, v: Option<&[u8]>) {
    match v {
        None => put_varint(out, -1),
        Some(b) => {
            put_varint(out, b.len() as i32);
            out.extend_from_slice(b);
        }
    }
}

/// Append a zig-zag encoded 32-bit varint.
pub fn put_varint(out: &mut Vec<u8>, v: i32) {
    put_uvarlong(out, ((v << 1) ^ (v >> 31)) as u32 as u64);
}

/// Append a zig-zag encoded 64-bit varint.
pub fn put_varlong(out: &mut Vec<u8>, v: i64) {
    put_uvarlong(out, ((v << 1) ^ (v >> 63)) as u64);
}

fn put_uvarlong(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// A bounds-checked reader over a byte slice; never panics, never unwraps.
pub struct Cursor<'a> {
    /// The buffer being read.
    pub buf: &'a [u8],
    /// Current read position.
    pub pos: usize,
}

impl<'a> Cursor<'a> {
    /// Start reading at the beginning of `buf`.
    pub fn new(buf: &'a [u8]) -> Self {
        Cursor { buf, pos: 0 }
    }

    /// Bytes not yet consumed.
    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    /// Take `n` bytes.
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            bail!("need {n} bytes, {} remain", self.remaining());
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    /// Read a signed byte.
    pub fn i8(&mut self) -> Result<i8> {
        Ok(self.take(1)?[0] as i8)
    }

    /// Read a big-endian i16.
    pub fn i16(&mut self) -> Result<i16> {
        let b = self.take(2)?;
        Ok(i16::from_be_bytes([b[0], b[1]]))
    }

    /// Read a big-endian i32.
    pub fn i32(&mut self) -> Result<i32> {
        let b = self.take(4)?;
        Ok(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Read a big-endian u32.
    pub fn u32(&mut self) -> Result<u32> {
        Ok(self.i32()? as u32)
    }

    /// Read a big-endian i64.
    pub fn i64(&mut self) -> Result<i64> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(i64::from_be_bytes(a))
    }

    /// Read an unsigned LEB128 varint (at most 10 bytes).
    pub fn uvarlong(&mut self) -> Result<u64> {
        let mut out = 0u64;
        let mut shift = 0;
        loop {
            if shift > 63 {
                bail!("varint is longer than 10 bytes");
            }
            let b = self.take(1)?[0];
            out |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(out);
            }
            shift += 7;
        }
    }

    /// Read a zig-zag encoded 32-bit varint.
    pub fn varint(&mut self) -> Result<i32> {
        let v = self.uvarlong()?;
        if v > u32::MAX as u64 {
            bail!("varint does not fit in 32 bits");
        }
        let v = v as u32;
        Ok(((v >> 1) as i32) ^ -((v & 1) as i32))
    }

    /// Read a zig-zag encoded 64-bit varint.
    pub fn varlong(&mut self) -> Result<i64> {
        let v = self.uvarlong()?;
        Ok(((v >> 1) as i64) ^ -((v & 1) as i64))
    }

    /// Read a varint-length-prefixed byte string, `None` when the length is -1.
    pub fn nullable(&mut self) -> Result<Option<Vec<u8>>> {
        let n = self.varint()?;
        if n == -1 {
            return Ok(None);
        }
        if n < 0 {
            bail!("length {n} is negative but not -1");
        }
        Ok(Some(self.take(n as usize)?.to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> RecordBatch {
        let mut b = RecordBatch::of(
            17,
            1_700_000_000_000,
            vec![
                RecordItem::value("hello"),
                RecordItem::keyed("k", "world"),
                RecordItem {
                    value: Some(b"with-headers".to_vec()),
                    headers: vec![
                        ("h1".to_string(), Some(b"v1".to_vec())),
                        ("h2".to_string(), None),
                    ],
                    ..Default::default()
                },
                RecordItem {
                    key: Some(b"tombstone".to_vec()),
                    value: None,
                    ..Default::default()
                },
            ],
        );
        b.partition_leader_epoch = 3;
        b.max_timestamp = 1_700_000_000_000;
        b
    }

    #[test]
    fn round_trips_through_our_reader() {
        let b = sample();
        let bytes = b.encode();
        let (back, used) = RecordBatch::decode(&bytes).expect("decode");
        assert_eq!(used, bytes.len());
        assert_eq!(back, b);
        assert_eq!(back.next_offset(), 21);
    }

    #[test]
    fn round_trips_through_kafka_protocol() {
        use kafka_protocol::records::RecordBatchDecoder;
        let b = sample();
        let mut bytes = bytes::Bytes::from(b.encode());
        let set = RecordBatchDecoder::decode(&mut bytes).expect("kafka-protocol decode");
        assert_eq!(set.records.len(), 4);
        assert_eq!(set.records[0].offset, 17);
        assert_eq!(
            set.records[0].value.as_deref(),
            Some(&b"hello"[..]),
            "first record value"
        );
        assert_eq!(set.records[3].value, None, "tombstone value is null");
        assert_eq!(set.records[2].headers.len(), 2);
    }

    #[test]
    fn multiple_batches_concatenate() {
        let a = RecordBatch::of(0, 1, vec![RecordItem::value("a")]);
        let b = RecordBatch::of(1, 2, vec![RecordItem::value("b"), RecordItem::value("c")]);
        let bytes = encode_batches(&[a.clone(), b.clone()]);
        let all = RecordBatch::decode_all(&bytes).expect("decode_all");
        assert_eq!(all, vec![a, b]);
    }

    #[test]
    fn every_codec_round_trips_through_our_reader() {
        for codec in [GZIP, SNAPPY, LZ4, ZSTD] {
            let b = sample().with_compression(codec);
            let bytes = b.encode();
            let (back, used) = RecordBatch::decode(&bytes)
                .unwrap_or_else(|e| panic!("{} does not decode: {e:#}", codec_name(codec)));
            assert_eq!(used, bytes.len(), "{}", codec_name(codec));
            assert_eq!(back, b, "{}", codec_name(codec));
            assert_eq!(back.compression(), codec, "attribute bits survive");
        }
    }

    #[test]
    fn every_codec_round_trips_through_kafka_protocol() {
        use kafka_protocol::records::RecordBatchDecoder;
        for codec in [GZIP, SNAPPY, LZ4, ZSTD] {
            let b = sample().with_compression(codec);
            let mut bytes = bytes::Bytes::from(b.encode());
            let set = RecordBatchDecoder::decode(&mut bytes)
                .unwrap_or_else(|e| panic!("{} rejected: {e:#}", codec_name(codec)));
            assert_eq!(set.records.len(), 4, "{}", codec_name(codec));
            assert_eq!(set.records[0].value.as_deref(), Some(&b"hello"[..]));
            assert_eq!(set.records[3].value, None, "tombstone survives compression");
        }
    }

    #[test]
    fn compression_actually_shrinks_a_repetitive_payload() {
        let items: Vec<RecordItem> = (0..200)
            .map(|i| RecordItem::value(format!("repeated-value-{}", i % 3)))
            .collect();
        let plain = RecordBatch::of(0, 1, items).encode();
        for codec in [GZIP, SNAPPY, LZ4, ZSTD] {
            let b = RecordBatch::decode(&plain)
                .expect("plain decodes")
                .0
                .with_compression(codec)
                .encode();
            assert!(
                b.len() < plain.len(),
                "{} made the batch bigger ({} vs {})",
                codec_name(codec),
                b.len(),
                plain.len()
            );
        }
    }

    #[test]
    fn snappy_uses_the_xerial_framing_kafka_expects() {
        let b = sample().with_compression(SNAPPY).encode();
        let payload = &b[61..];
        assert_eq!(
            &payload[..16],
            b"\x82SNAPPY\x00\x00\x00\x00\x01\x00\x00\x00\x01",
            "Kafka's snappy stream starts with the xerial magic header"
        );
    }

    #[test]
    fn bad_crc_is_reported() {
        let mut b = sample();
        b.crc_override = Some(0xdead_beef);
        let bytes = b.encode();
        let err = RecordBatch::decode(&bytes).expect_err("must reject");
        assert!(format!("{err}").contains("crc mismatch"), "{err}");
    }

    #[test]
    fn truncation_is_reported_not_panicked() {
        let bytes = sample().encode();
        for cut in [5usize, 13, 30, bytes.len() - 1] {
            assert!(RecordBatch::decode(&bytes[..cut]).is_err(), "cut at {cut}");
        }
    }

    #[test]
    fn varints_round_trip() {
        for v in [0i32, 1, -1, 63, 64, -64, i32::MAX, i32::MIN, 300, -300] {
            let mut out = Vec::new();
            put_varint(&mut out, v);
            let mut c = Cursor::new(&out);
            assert_eq!(c.varint().expect("varint"), v);
            assert_eq!(c.remaining(), 0);
        }
        for v in [0i64, -1, i64::MAX, i64::MIN, 1_700_000_000_000] {
            let mut out = Vec::new();
            put_varlong(&mut out, v);
            let mut c = Cursor::new(&out);
            assert_eq!(c.varlong().expect("varlong"), v);
        }
    }
}
