//! Turning example bytes back into a list of annotated fields.
//!
//! The site highlights a request or a response byte by byte, so every example carries a
//! `{offset, length, field, value}` entry per field. They are produced by walking the bytes
//! with the schema of the API in question ([`super::schema_core`], [`super::schema_data`],
//! [`super::schema_groups`]) rather than by decoding into structs, because only a walk
//! knows *where* each field was.
//!
//! The walk is its own check: [`Walk::consumed_everything`] is false whenever the schema
//! and the bytes disagree, and `--capture-examples` refuses to write a file with such an
//! example in it.

use super::FieldAnn;
use crate::proto::records;
use crate::stages::error_name;
use kafka_protocol::messages::ApiKey;
use uuid::Uuid;

/// A cursor over one frame that records what it reads.
pub struct Walk<'a> {
    buf: &'a [u8],
    pos: usize,
    end: usize,
    path: Vec<String>,
    emit: bool,
    out: Vec<FieldAnn>,
    truncated: bool,
    /// True while walking a request, where the harness chose every byte.
    request_side: bool,
}

impl<'a> Walk<'a> {
    /// Walk `buf[start..end]`.
    pub fn new(buf: &'a [u8], start: usize, end: usize) -> Walk<'a> {
        Walk {
            buf,
            pos: start,
            end: end.min(buf.len()),
            path: Vec::new(),
            emit: true,
            out: Vec::new(),
            truncated: false,
            request_side: false,
        }
    }

    /// Mark this walk as walking a request rather than a response.
    pub fn for_request(mut self) -> Walk<'a> {
        self.request_side = true;
        self
    }

    /// Where the cursor is.
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// How many bytes are left in this frame.
    pub fn remaining(&self) -> usize {
        self.end.saturating_sub(self.pos)
    }

    /// False once the bytes ran out under the schema.
    pub fn ok(&self) -> bool {
        !self.truncated
    }

    /// True when the walk read exactly the frame, which is what proves the schema right.
    pub fn consumed_everything(&self) -> bool {
        !self.truncated && self.pos == self.end
    }

    /// The annotations gathered so far.
    pub fn into_anns(self) -> Vec<FieldAnn> {
        self.out
    }

    /// Push a path segment (an array element, a nested struct).
    pub fn enter(&mut self, segment: impl Into<String>) {
        self.path.push(segment.into());
    }

    /// Pop a path segment.
    pub fn leave(&mut self) {
        self.path.pop();
    }

    /// Full dotted path of a field name at the current depth.
    pub fn path_of(&self, name: &str) -> String {
        let mut parts: Vec<&str> = self.path.iter().map(String::as_str).collect();
        if !name.is_empty() {
            parts.push(name);
        }
        parts.join(".")
    }

    /// Record one annotation, unless emitting is suppressed (array elements past the first).
    pub fn ann(&mut self, offset: usize, length: usize, name: &str, value: impl Into<String>) {
        if !self.emit {
            return;
        }
        let field = self.path_of(name);
        let value = value.into();
        let varies = self.resolve_varies(&field, varies(&field, &value));
        self.out.push(FieldAnn {
            offset,
            length,
            field,
            value,
            varies,
        });
    }

    /// Record an annotation whose "varies between boots" answer the caller knows.
    pub fn ann_varies(
        &mut self,
        offset: usize,
        length: usize,
        name: &str,
        value: impl Into<String>,
        varies: bool,
    ) {
        if !self.emit {
            return;
        }
        let field = self.path_of(name);
        let varies = self.resolve_varies(&field, varies);
        self.out.push(FieldAnn {
            offset,
            length,
            field,
            value: value.into(),
            varies,
        });
    }

    /// A request is built by the harness, so nothing in it changes between captures on its
    /// own. The capture then marks the few request fields that really do come from the
    /// broker — a topic id it minted — by comparing them with the fixtures it created.
    fn resolve_varies(&self, _field: &str, proposed: bool) -> bool {
        !self.request_side && proposed
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.truncated || self.pos + n > self.end {
            self.truncated = true;
            return None;
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Some(s)
    }

    /// A signed 8-bit field.
    pub fn i8(&mut self, name: &str) -> i8 {
        let at = self.pos;
        let Some(s) = self.take(1) else { return 0 };
        let v = s[0] as i8;
        self.ann(at, 1, name, v.to_string());
        v
    }

    /// A boolean (one byte).
    pub fn boolean(&mut self, name: &str) -> bool {
        let at = self.pos;
        let Some(s) = self.take(1) else { return false };
        let v = s[0] != 0;
        self.ann(at, 1, name, v.to_string());
        v
    }

    /// A signed 16-bit field.
    pub fn i16(&mut self, name: &str) -> i16 {
        let at = self.pos;
        let Some(s) = self.take(2) else { return 0 };
        let v = i16::from_be_bytes([s[0], s[1]]);
        self.ann(at, 2, name, v.to_string());
        v
    }

    /// A signed 16-bit error code, annotated the way the report writes error codes.
    pub fn error_code(&mut self, name: &str) -> i16 {
        let at = self.pos;
        let Some(s) = self.take(2) else { return 0 };
        let v = i16::from_be_bytes([s[0], s[1]]);
        self.ann(at, 2, name, format!("{v} ({})", error_name(v)));
        v
    }

    /// The `api_key` of a request header, annotated with the API's name.
    pub fn api_key(&mut self, name: &str) -> i16 {
        let at = self.pos;
        let Some(s) = self.take(2) else { return 0 };
        let v = i16::from_be_bytes([s[0], s[1]]);
        self.ann(at, 2, name, api_label(v));
        v
    }

    /// A signed 32-bit field.
    pub fn i32(&mut self, name: &str) -> i32 {
        let at = self.pos;
        let Some(s) = self.take(4) else { return 0 };
        let v = i32::from_be_bytes([s[0], s[1], s[2], s[3]]);
        self.ann(at, 4, name, v.to_string());
        v
    }

    /// A signed 64-bit field.
    pub fn i64(&mut self, name: &str) -> i64 {
        let at = self.pos;
        let Some(s) = self.take(8) else { return 0 };
        let v = i64::from_be_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]);
        self.ann(at, 8, name, v.to_string());
        v
    }

    /// A 16-byte uuid; the nil uuid is the one value that never varies.
    pub fn uuid(&mut self, name: &str) -> Uuid {
        let at = self.pos;
        let Some(s) = self.take(16) else {
            return Uuid::nil();
        };
        let mut b = [0u8; 16];
        b.copy_from_slice(s);
        let id = Uuid::from_bytes(b);
        let label = if id.is_nil() {
            format!("{id} (the all-zero id)")
        } else {
            id.to_string()
        };
        self.ann_varies(at, 16, name, label, !id.is_nil());
        id
    }

    /// A non-compact nullable string (`int16` length), as in a request header v1/v2.
    pub fn legacy_nullable_string(&mut self, name: &str) -> Option<String> {
        let at = self.pos;
        let s = self.take(2)?;
        let len = i16::from_be_bytes([s[0], s[1]]);
        if len < 0 {
            self.ann(at, 2, name, "null");
            return None;
        }
        let body = self.take(len as usize)?;
        let text = String::from_utf8_lossy(body).to_string();
        self.ann(at, 2 + len as usize, name, format!("'{text}'"));
        Some(text)
    }

    /// An unsigned varint; the raw building block of every compact type.
    fn uvarint_raw(&mut self) -> Option<(u64, usize, usize)> {
        let at = self.pos;
        let mut value: u64 = 0;
        let mut shift = 0;
        loop {
            let b = self.take(1)?;
            value |= ((b[0] & 0x7f) as u64) << shift;
            if b[0] & 0x80 == 0 {
                break;
            }
            shift += 7;
            if shift > 63 {
                self.truncated = true;
                return None;
            }
        }
        Some((value, at, self.pos - at))
    }

    /// An unsigned varint field in its own right (a length, a count).
    pub fn uvarint(&mut self, name: &str) -> u64 {
        let Some((v, at, len)) = self.uvarint_raw() else {
            return 0;
        };
        self.ann(at, len, name, v.to_string());
        v
    }

    /// A compact string (`uvarint(len + 1)` then the bytes).
    pub fn compact_string(&mut self, name: &str) -> Option<String> {
        let (n, at, vlen) = self.uvarint_raw()?;
        if n == 0 {
            self.ann(at, vlen, name, "null");
            return None;
        }
        let len = (n - 1) as usize;
        let body = self.take(len)?;
        let text = String::from_utf8_lossy(body).to_string();
        self.ann(at, vlen + len, name, format!("'{text}'"));
        Some(text)
    }

    /// Compact bytes (`uvarint(len + 1)` then the bytes), annotated as a blob.
    pub fn compact_bytes(&mut self, name: &str) -> Option<(usize, usize)> {
        let (n, at, vlen) = self.uvarint_raw()?;
        if n == 0 {
            self.ann(at, vlen, name, "null");
            return None;
        }
        let len = (n - 1) as usize;
        let start = self.pos;
        self.take(len)?;
        self.ann(at, vlen + len, name, format!("{len} bytes"));
        Some((start, len))
    }

    /// A compact array: the length varint, then `f` per element (annotating the first).
    pub fn compact_array<F>(&mut self, name: &str, mut f: F) -> usize
    where
        F: FnMut(&mut Walk<'a>, usize),
    {
        let Some((n, at, vlen)) = self.uvarint_raw() else {
            return 0;
        };
        if n == 0 {
            self.ann(at, vlen, &format!("{name}.length"), "null");
            return 0;
        }
        let count = (n - 1) as usize;
        let label = if count == 1 {
            "1 entry".to_string()
        } else {
            format!("{count} entries")
        };
        self.ann(at, vlen, &format!("{name}.length"), label);
        self.each(name, count, &mut f);
        count
    }

    /// A non-compact array (`int32` count), for the pre-flexible versions.
    pub fn array<F>(&mut self, name: &str, mut f: F) -> usize
    where
        F: FnMut(&mut Walk<'a>, usize),
    {
        let at = self.pos;
        let Some(s) = self.take(4) else { return 0 };
        let n = i32::from_be_bytes([s[0], s[1], s[2], s[3]]);
        if n < 0 {
            self.ann(at, 4, &format!("{name}.length"), "null");
            return 0;
        }
        let label = if n == 1 {
            "1 entry".to_string()
        } else {
            format!("{n} entries")
        };
        self.ann(at, 4, &format!("{name}.length"), label);
        self.each(name, n as usize, &mut f);
        n as usize
    }

    fn each<F>(&mut self, name: &str, count: usize, f: &mut F)
    where
        F: FnMut(&mut Walk<'a>, usize),
    {
        for i in 0..count {
            if self.truncated {
                return;
            }
            let was = self.emit;
            // Only the first element is annotated; the rest are walked so the cursor stays
            // in step, but the site does not need a hundred copies of the same schema.
            if i > 0 {
                self.emit = false;
            }
            self.enter(format!("{name}[{i}]"));
            f(self, i);
            self.leave();
            self.emit = was;
        }
    }

    /// A nullable struct, as the flexible protocol writes one: a one-byte presence marker
    /// (`0xff` for null, `0x01` for "a struct follows") and then the struct's own fields.
    pub fn nullable_struct<F>(&mut self, name: &str, mut f: F)
    where
        F: FnMut(&mut Walk<'a>),
    {
        if self.truncated || self.pos >= self.end {
            self.truncated = true;
            return;
        }
        let at = self.pos;
        let marker = self.buf[self.pos] as i8;
        self.pos += 1;
        if marker != 1 {
            self.ann(at, 1, name, format!("null ({:#04x})", marker as u8));
            return;
        }
        self.ann(at, 1, &format!("{name}.present"), "1 (a struct follows)");
        self.enter(name.to_string());
        f(self);
        self.leave();
    }

    /// A nested struct, so its fields get a `name.` prefix.
    pub fn nested<F>(&mut self, name: &str, mut f: F)
    where
        F: FnMut(&mut Walk<'a>),
    {
        self.enter(name.to_string());
        f(self);
        self.leave();
    }

    /// A tagged-field section: the count and then every tag, as one annotation.
    pub fn tags(&mut self, name: &str) {
        let at = self.pos;
        let Some((n, _, _)) = self.uvarint_raw() else {
            return;
        };
        let mut described: Vec<String> = Vec::new();
        for _ in 0..n {
            let Some((tag, _, _)) = self.uvarint_raw() else {
                return;
            };
            let Some((size, _, _)) = self.uvarint_raw() else {
                return;
            };
            if self.take(size as usize).is_none() {
                return;
            }
            described.push(format!("tag {tag} ({size} bytes)"));
        }
        let value = if n == 0 {
            "0 tagged fields".to_string()
        } else {
            format!("{n} tagged fields: {}", described.join(", "))
        };
        let len = self.pos - at;
        self.ann(at, len, name, value);
    }

    /// Whatever is left of the frame, as one blob (only used when a walk gives up).
    pub fn rest(&mut self, name: &str, why: &str) {
        let at = self.pos;
        let len = self.remaining();
        if len == 0 {
            return;
        }
        self.pos = self.end;
        self.ann(at, len, name, format!("{len} bytes — {why}"));
    }

    /// Annotate a record-batch blob at `[start, start + len)`: batch headers and the first
    /// record of the first batch.
    pub fn record_batches(&mut self, name: &str, start: usize, len: usize) {
        let end = (start + len).min(self.buf.len());
        let mut at = start;
        let mut index = 0usize;
        while at + 61 <= end {
            let bytes = &self.buf[at..end];
            let batch_len = i32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
            let total = 12 + batch_len.max(0) as usize;
            let was = self.emit;
            if index > 0 {
                self.emit = false;
            }
            self.enter(format!("{name}[{index}]"));
            self.batch_header(at);
            self.leave();
            self.emit = was;
            if batch_len <= 0 || at + total > end {
                break;
            }
            at += total;
            index += 1;
        }
    }

    /// The 61-byte v2 batch header at `at`, plus a peek at the first record.
    fn batch_header(&mut self, at: usize) {
        let b = self.buf;
        let g8 = |o: usize| -> i64 {
            let mut v = [0u8; 8];
            v.copy_from_slice(&b[o..o + 8]);
            i64::from_be_bytes(v)
        };
        let g4 = |o: usize| -> i32 { i32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) };
        let g2 = |o: usize| -> i16 { i16::from_be_bytes([b[o], b[o + 1]]) };
        if at + 61 > b.len() {
            return;
        }
        self.ann(at, 8, "base_offset", g8(at).to_string());
        self.ann(
            at + 8,
            4,
            "batch_length",
            format!("{} (bytes after this field)", g4(at + 8)),
        );
        self.ann(
            at + 12,
            4,
            "partition_leader_epoch",
            g4(at + 12).to_string(),
        );
        self.ann(at + 16, 1, "magic", format!("{} (v2)", b[at + 16] as i8));
        let crc = u32::from_be_bytes([b[at + 17], b[at + 18], b[at + 19], b[at + 20]]);
        self.ann_varies(
            at + 17,
            4,
            "crc",
            format!("{crc:#010x} (CRC-32C of everything after this field)"),
            true,
        );
        let attributes = g2(at + 21);
        let codec = records::codec_name(attributes & records::COMPRESSION_MASK);
        let control = if attributes & (1 << 5) != 0 {
            ", control"
        } else {
            ""
        };
        self.ann(
            at + 21,
            2,
            "attributes",
            format!("{attributes} (compression {codec}{control})"),
        );
        self.ann(at + 23, 4, "last_offset_delta", g4(at + 23).to_string());
        self.ann_varies(
            at + 27,
            8,
            "base_timestamp",
            g8(at + 27).to_string(),
            g8(at + 27) > 0,
        );
        self.ann_varies(
            at + 35,
            8,
            "max_timestamp",
            g8(at + 35).to_string(),
            g8(at + 35) > 0,
        );
        self.ann_varies(
            at + 43,
            8,
            "producer_id",
            g8(at + 43).to_string(),
            g8(at + 43) >= 0,
        );
        self.ann(at + 51, 2, "producer_epoch", g2(at + 51).to_string());
        self.ann(at + 53, 4, "base_sequence", g4(at + 53).to_string());
        let count = g4(at + 57);
        self.ann(at + 57, 4, "record_count", count.to_string());
        if count > 0 && attributes & records::COMPRESSION_MASK == 0 {
            self.enter("records[0]".to_string());
            self.first_record(at + 61);
            self.leave();
        }
    }

    /// The first record of an uncompressed batch, field by field.
    fn first_record(&mut self, at: usize) {
        let mut c = records::Cursor::new(&self.buf[at.min(self.buf.len())..]);
        let mut here = at;
        let before = c.remaining();
        let Ok(length) = c.varint() else { return };
        let used = before - c.remaining();
        self.ann(here, used, "length", format!("{length} (varint)"));
        here += used;

        let before = c.remaining();
        let Ok(attrs) = c.i8() else { return };
        let used = before - c.remaining();
        self.ann(here, used, "attributes", attrs.to_string());
        here += used;

        let before = c.remaining();
        let Ok(ts) = c.varlong() else { return };
        let used = before - c.remaining();
        self.ann(here, used, "timestamp_delta", format!("{ts} (varlong)"));
        here += used;

        let before = c.remaining();
        let Ok(od) = c.varint() else { return };
        let used = before - c.remaining();
        self.ann(here, used, "offset_delta", format!("{od} (varint)"));
        here += used;

        let before = c.remaining();
        let Ok(key) = c.nullable() else { return };
        let used = before - c.remaining();
        self.ann(
            here,
            used,
            "key",
            match &key {
                None => "null".to_string(),
                Some(k) => format!("{} bytes {}", k.len(), preview(k)),
            },
        );
        here += used;

        let before = c.remaining();
        let Ok(value) = c.nullable() else { return };
        let used = before - c.remaining();
        self.ann(
            here,
            used,
            "value",
            match &value {
                None => "null".to_string(),
                Some(v) => format!("{} bytes {}", v.len(), preview(v)),
            },
        );
        here += used;

        let before = c.remaining();
        let Ok(headers) = c.varint() else { return };
        let used = before - c.remaining();
        self.ann(here, used, "headers.length", format!("{headers} (varint)"));
    }
}

/// `"hello"` for printable payloads, hex otherwise.
fn preview(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) if s.chars().all(|c| !c.is_control()) => format!("'{s}'"),
        _ => format!("0x{}", super::to_hex(&b[..b.len().min(8)])),
    }
}

/// The last segment of a field path, without its array index.
fn leaf_of(field: &str) -> &str {
    let leaf = field.rsplit('.').next().unwrap_or(field);
    leaf.split('[').next().unwrap_or(leaf)
}

/// Fields whose value is legitimately different on every boot.
fn varies(field: &str, value: &str) -> bool {
    let leaf = leaf_of(field);
    match leaf {
        "cluster_id" | "member_id" | "host" | "port" | "session_id" | "topic_id" => true,
        "producer_id" => value != "-1",
        "log_append_time_ms" | "timestamp" | "base_timestamp" | "max_timestamp" => value != "-1",
        "crc" => true,
        _ => false,
    }
}

/// The api key and version of every frame in a request byte string.
pub fn request_apis(bytes: &[u8]) -> Vec<(i16, i16)> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 4 <= bytes.len() {
        let size = i32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
        if size < 4 || at + 4 + size as usize > bytes.len() {
            break;
        }
        let body = &bytes[at + 4..at + 4 + size as usize];
        if body.len() < 4 {
            break;
        }
        out.push((
            i16::from_be_bytes([body[0], body[1]]),
            i16::from_be_bytes([body[2], body[3]]),
        ));
        at += 4 + size as usize;
    }
    out
}

/// Annotate a request byte string (size prefixes included).
///
/// Returns the annotations and whether every frame was walked to its last byte; a `false`
/// there means the schema in this module and the bytes disagree, which is a suite bug.
pub fn annotate_request(bytes: &[u8]) -> (Vec<FieldAnn>, bool) {
    let mut out = Vec::new();
    let mut clean = true;
    let mut at = 0usize;
    let frames = count_frames(bytes);
    let mut index = 0usize;
    while at < bytes.len() {
        let prefix = frame_prefix(frames, index);
        let Some((body_start, body_end)) = frame_bounds(bytes, at, &prefix, &mut out) else {
            clean = false;
            break;
        };
        let mut w = Walk::new(bytes, body_start, body_end).for_request();
        if !prefix.is_empty() {
            w.enter(prefix.trim_end_matches('.').to_string());
        }
        let (key, version) = request_header(&mut w);
        match ApiKey::try_from(key) {
            Ok(api) => {
                w.nested("body", |w| request_body(w, api, version));
            }
            Err(_) => w.rest("body", "an api key this harness does not know"),
        }
        clean &= w.consumed_everything();
        out.extend(w.into_anns());
        at = body_end;
        index += 1;
    }
    (out, clean)
}

/// Annotate a response byte string, given the `(api key, version)` of each request frame.
pub fn annotate_response(bytes: &[u8], apis: &[(i16, i16)]) -> (Vec<FieldAnn>, bool) {
    let mut out = Vec::new();
    let mut clean = true;
    let mut at = 0usize;
    let frames = count_frames(bytes);
    let mut index = 0usize;
    while at < bytes.len() {
        let prefix = frame_prefix(frames, index);
        let Some((body_start, body_end)) = frame_bounds(bytes, at, &prefix, &mut out) else {
            clean = false;
            break;
        };
        let (key, version) = apis
            .get(index)
            .or_else(|| apis.last())
            .copied()
            .unwrap_or((-1, 0));
        let mut w = Walk::new(bytes, body_start, body_end);
        if !prefix.is_empty() {
            w.enter(prefix.trim_end_matches('.').to_string());
        }
        let header_version = ApiKey::try_from(key)
            .map(|a| a.response_header_version(version))
            .unwrap_or(0);
        w.nested("header", |w| {
            w.i32("correlation_id");
            if header_version >= 1 {
                w.tags("tagged_fields");
            }
        });
        match ApiKey::try_from(key) {
            Ok(api) => w.nested("body", |w| response_body(w, api, version)),
            Err(_) => w.rest("body", "an api key this harness does not know"),
        }
        clean &= w.consumed_everything();
        out.extend(w.into_anns());
        at = body_end;
        index += 1;
    }
    (out, clean)
}

fn count_frames(bytes: &[u8]) -> usize {
    let mut at = 0usize;
    let mut n = 0usize;
    while at + 4 <= bytes.len() {
        let size = i32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
        if size < 0 || at + 4 + size as usize > bytes.len() {
            return n + 1;
        }
        at += 4 + size as usize;
        n += 1;
    }
    if at < bytes.len() {
        n += 1;
    }
    n
}

fn frame_prefix(frames: usize, index: usize) -> String {
    if frames > 1 {
        format!("frame{index}.")
    } else {
        String::new()
    }
}

/// Annotate the size prefix and return the body's `[start, end)`, or `None` when the
/// prefix does not describe the bytes that follow (a deliberately broken frame).
fn frame_bounds(
    bytes: &[u8],
    at: usize,
    prefix: &str,
    out: &mut Vec<FieldAnn>,
) -> Option<(usize, usize)> {
    let field = |name: &str| format!("{prefix}{name}");
    if at + 4 > bytes.len() {
        out.push(FieldAnn {
            offset: at,
            length: bytes.len() - at,
            field: field("size"),
            value: format!(
                "{} bytes — too short for a 4-byte size prefix",
                bytes.len() - at
            ),
            varies: false,
        });
        return None;
    }
    let size = i32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    let available = bytes.len() - at - 4;
    if size < 0 || size as usize > available {
        out.push(FieldAnn {
            offset: at,
            length: 4,
            field: field("size"),
            value: format!("{size} — but only {available} bytes follow"),
            varies: false,
        });
        if available > 0 {
            out.push(FieldAnn {
                offset: at + 4,
                length: available,
                field: field("payload"),
                value: format!("{available} bytes, fewer than the size prefix promises"),
                varies: false,
            });
        }
        return None;
    }
    out.push(FieldAnn {
        offset: at,
        length: 4,
        field: field("size"),
        value: format!("{size} (bytes that follow)"),
        varies: false,
    });
    Some((at + 4, at + 4 + size as usize))
}

/// The request header, returning `(api key, api version)`.
fn request_header(w: &mut Walk<'_>) -> (i16, i16) {
    let mut key = 0i16;
    let mut version = 0i16;
    w.nested("header", |w| {
        key = w.api_key("api_key");
        version = w.i16("api_version");
        w.i32("correlation_id");
        let header_version = ApiKey::try_from(key)
            .map(|a| a.request_header_version(version))
            .unwrap_or(2);
        if header_version >= 1 {
            w.legacy_nullable_string("client_id");
        }
        if header_version >= 2 {
            w.tags("tagged_fields");
        }
    });
    (key, version)
}

/// `"18 (ApiVersions)"`.
pub fn api_label(key: i16) -> String {
    match ApiKey::try_from(key) {
        Ok(api) => format!("{key} ({api:?})"),
        Err(_) => format!("{key} (unknown api key)"),
    }
}

fn request_body(w: &mut Walk<'_>, api: ApiKey, version: i16) {
    use ApiKey::*;
    match api {
        ApiVersions => super::schema_core::api_versions_request(w, version),
        DescribeTopicPartitions => super::schema_core::describe_topic_partitions_request(w),
        Metadata => super::schema_core::metadata_request(w, version),
        CreateTopics => super::schema_core::create_topics_request(w, version),
        DeleteTopics => super::schema_core::delete_topics_request(w, version),
        Fetch => super::schema_data::fetch_request(w, version),
        Produce => super::schema_data::produce_request(w, version),
        ListOffsets => super::schema_data::list_offsets_request(w, version),
        InitProducerId => super::schema_data::init_producer_id_request(w, version),
        FindCoordinator => super::schema_groups::find_coordinator_request(w, version),
        JoinGroup => super::schema_groups::join_group_request(w, version),
        SyncGroup => super::schema_groups::sync_group_request(w, version),
        Heartbeat => super::schema_groups::heartbeat_request(w, version),
        LeaveGroup => super::schema_groups::leave_group_request(w, version),
        OffsetCommit => super::schema_groups::offset_commit_request(w, version),
        OffsetFetch => super::schema_groups::offset_fetch_request(w, version),
        _ => w.rest("", "a request body this harness does not annotate"),
    }
}

fn response_body(w: &mut Walk<'_>, api: ApiKey, version: i16) {
    use ApiKey::*;
    match api {
        ApiVersions => super::schema_core::api_versions_response(w, version),
        DescribeTopicPartitions => super::schema_core::describe_topic_partitions_response(w),
        Metadata => super::schema_core::metadata_response(w, version),
        CreateTopics => super::schema_core::create_topics_response(w, version),
        DeleteTopics => super::schema_core::delete_topics_response(w, version),
        Fetch => super::schema_data::fetch_response(w, version),
        Produce => super::schema_data::produce_response(w, version),
        ListOffsets => super::schema_data::list_offsets_response(w, version),
        InitProducerId => super::schema_data::init_producer_id_response(w, version),
        FindCoordinator => super::schema_groups::find_coordinator_response(w, version),
        JoinGroup => super::schema_groups::join_group_response(w, version),
        SyncGroup => super::schema_groups::sync_group_response(w, version),
        Heartbeat => super::schema_groups::heartbeat_response(w, version),
        LeaveGroup => super::schema_groups::leave_group_response(w, version),
        OffsetCommit => super::schema_groups::offset_commit_response(w, version),
        OffsetFetch => super::schema_groups::offset_fetch_response(w, version),
        _ => w.rest("", "a response body this harness does not annotate"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::examples::ExampleEnv;
    use crate::stages::{api_versions_request, API_VERSIONS_V4};

    fn env() -> ExampleEnv {
        ExampleEnv {
            topics: Default::default(),
            group: crate::examples::EXAMPLE_GROUP.to_string(),
        }
    }

    #[test]
    fn a_request_walk_names_the_header_fields() {
        let wire = env()
            .request(API_VERSIONS_V4, 7, &api_versions_request())
            .expect("encode");
        let bytes = wire.bytes();
        let (anns, clean) = annotate_request(&bytes);
        assert!(
            clean,
            "the ApiVersions v4 request schema must consume it all"
        );
        let by_field = |f: &str| {
            anns.iter()
                .find(|a| a.field == f)
                .map(|a| a.value.clone())
                .unwrap_or_default()
        };
        assert_eq!(by_field("size"), "37 (bytes that follow)");
        assert_eq!(by_field("header.api_key"), "18 (ApiVersions)");
        assert_eq!(by_field("header.api_version"), "4");
        assert_eq!(by_field("header.correlation_id"), "7");
        assert_eq!(by_field("header.client_id"), "'kafkatest'");
        assert_eq!(by_field("header.tagged_fields"), "0 tagged fields");
        assert_eq!(by_field("body.client_software_name"), "'kafkatest'");
        assert_eq!(anns[0].offset, 0);
        assert_eq!(anns[1].offset, 4, "the header starts after the size prefix");
    }

    #[test]
    fn a_short_frame_is_annotated_rather_than_guessed() {
        let bytes = vec![0, 0, 0, 100, 1, 2, 3];
        let (anns, clean) = annotate_request(&bytes);
        assert!(!clean);
        assert!(anns[0].value.contains("only 3 bytes follow"), "{anns:?}");
    }

    #[test]
    fn request_apis_reads_every_frame() {
        let e = env();
        let a = e
            .payload(API_VERSIONS_V4, 1, &api_versions_request())
            .expect("encode");
        let wire = crate::examples::Wire::Frames(vec![a.clone(), a]);
        assert_eq!(request_apis(&wire.bytes()), vec![(18, 4), (18, 4)]);
    }

    #[test]
    fn varying_fields_are_marked() {
        assert!(varies("body.topics[0].topic_id", "3f2d..."));
        assert!(varies("body.producer_id", "1000"));
        assert!(!varies("body.producer_id", "-1"));
        assert!(!varies("body.error_code", "0 (NONE)"));
    }
}
