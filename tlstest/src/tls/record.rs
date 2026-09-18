//! The record layer: `TLSPlaintext`, `TLSCiphertext` and `TLSInnerPlaintext` (RFC 8446 §5).
//!
//! A record on the wire is always five header bytes and then a fragment:
//!
//! ```text
//! struct {
//!     ContentType type;                     // 22 handshake, 23 application_data, 21 alert, 20 CCS
//!     ProtocolVersion legacy_record_version; // always 0x0303 after the first flight
//!     uint16 length;                        // <= 2^14 plaintext, <= 2^14 + 256 ciphertext
//!     opaque fragment[length];
//! } TLSPlaintext;
//! ```
//!
//! Once keys are in force the real content type moves *inside* the encrypted fragment and
//! the outer one is always `application_data(23)` — that is [`InnerPlaintext`], and it is
//! also where the optional zero padding lives. The five header bytes are the AEAD's
//! additional data, which is why a record cannot be moved, truncated or retyped.

use super::crypto::TrafficKeys;
use super::{ContentType, TlsError, TlsResult, MAX_CIPHERTEXT, MAX_PLAINTEXT};

/// One record as it appeared on the wire.
#[derive(Debug, Clone)]
pub struct Record {
    /// The outer `ContentType` byte.
    pub content_type: ContentType,
    /// The outer `legacy_record_version`.
    pub legacy_version: u16,
    /// The fragment, still encrypted when the record was protected.
    pub fragment: Vec<u8>,
    /// The whole record, header included, exactly as it arrived.
    pub raw: Vec<u8>,
}

impl Record {
    /// The five header bytes, which are also the AEAD's additional data.
    pub fn header(&self) -> [u8; 5] {
        let len = self.fragment.len() as u16;
        [
            self.content_type.as_u8(),
            (self.legacy_version >> 8) as u8,
            self.legacy_version as u8,
            (len >> 8) as u8,
            len as u8,
        ]
    }

    /// Build the bytes of a record with these exact header fields, checking nothing.
    ///
    /// Deliberate nonsense is the point of several stages, so this never validates the
    /// length or the version — [`RecordLayer`] does that on the way in.
    pub fn build(content_type: u8, legacy_version: u16, fragment: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(5 + fragment.len());
        out.push(content_type);
        out.extend_from_slice(&legacy_version.to_be_bytes());
        out.extend_from_slice(&(fragment.len() as u16).to_be_bytes());
        out.extend_from_slice(fragment);
        out
    }

    /// Build a record whose `length` field deliberately disagrees with the fragment.
    pub fn build_with_length(
        content_type: u8,
        legacy_version: u16,
        claimed_length: u16,
        fragment: &[u8],
    ) -> Vec<u8> {
        let mut out = Vec::with_capacity(5 + fragment.len());
        out.push(content_type);
        out.extend_from_slice(&legacy_version.to_be_bytes());
        out.extend_from_slice(&claimed_length.to_be_bytes());
        out.extend_from_slice(fragment);
        out
    }

    /// Parse one record from the front of `bytes`, returning it and how many bytes it used.
    pub fn parse(bytes: &[u8]) -> TlsResult<(Record, usize)> {
        if bytes.len() < 5 {
            return Err(TlsError::Record(format!(
                "a record header is five bytes; only {} arrived",
                bytes.len()
            )));
        }
        let length = u16::from_be_bytes([bytes[3], bytes[4]]) as usize;
        if bytes.len() < 5 + length {
            return Err(TlsError::Record(format!(
                "the record header claims {length} fragment bytes, {} arrived",
                bytes.len() - 5
            )));
        }
        let record = Record {
            content_type: ContentType::from_u8(bytes[0]),
            legacy_version: u16::from_be_bytes([bytes[1], bytes[2]]),
            fragment: bytes[5..5 + length].to_vec(),
            raw: bytes[..5 + length].to_vec(),
        };
        Ok((record, 5 + length))
    }

    /// The check RFC 8446 §5.1 puts on an incoming record's length.
    pub fn check_length(&self, encrypted: bool) -> TlsResult<()> {
        let limit = if encrypted {
            MAX_CIPHERTEXT
        } else {
            MAX_PLAINTEXT
        };
        if self.fragment.len() > limit {
            return Err(TlsError::Record(format!(
                "record.length is {}, more than the {limit} byte limit of RFC 8446 section 5.1",
                self.fragment.len()
            )));
        }
        Ok(())
    }
}

/// A decrypted record: `TLSInnerPlaintext` split back into content, type and padding.
///
/// ```text
/// struct {
///     opaque content[TLSPlaintext.length];
///     ContentType type;
///     uint8 zeros[length_of_padding];
/// } TLSInnerPlaintext;
/// ```
#[derive(Debug, Clone)]
pub struct InnerPlaintext {
    /// The real content type, the last non-zero byte of the decrypted fragment.
    pub content_type: ContentType,
    /// The content itself.
    pub content: Vec<u8>,
    /// How many zero bytes of padding followed the type byte.
    pub padding: usize,
}

impl InnerPlaintext {
    /// Build the inner plaintext for `content` of type `content_type`, with `padding` zeros.
    pub fn build(content_type: ContentType, content: &[u8], padding: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(content.len() + 1 + padding);
        out.extend_from_slice(content);
        out.push(content_type.as_u8());
        out.resize(out.len() + padding, 0);
        out
    }

    /// Split a decrypted fragment back into content, type and padding.
    ///
    /// An all-zero fragment has no type byte at all, which RFC 8446 §5.4 says is an
    /// `unexpected_message` alert rather than a record of type zero.
    pub fn parse(decrypted: &[u8]) -> TlsResult<InnerPlaintext> {
        let Some(end) = decrypted.iter().rposition(|b| *b != 0) else {
            return Err(TlsError::Record(
                "the decrypted record is all zeros: TLSInnerPlaintext has no content type byte \
                 (RFC 8446 section 5.4)"
                    .into(),
            ));
        };
        Ok(InnerPlaintext {
            content_type: ContentType::from_u8(decrypted[end]),
            content: decrypted[..end].to_vec(),
            padding: decrypted.len() - end - 1,
        })
    }
}

/// The keys, IVs and sequence numbers in force in each direction.
///
/// A sequence number counts records since the keys it belongs to came into force, and
/// **resets to zero every time the keys change** — that is the rule §5.3 states in one
/// sentence and that costs a day to find when it is missed.
#[derive(Debug, Default)]
pub struct RecordLayer {
    /// Keys the client writes with, once the handshake has produced them.
    pub write: Option<TrafficKeys>,
    /// Records written under the current write keys.
    pub write_seq: u64,
    /// Keys the client reads with.
    pub read: Option<TrafficKeys>,
    /// Records read under the current read keys.
    pub read_seq: u64,
    /// What the client puts in `legacy_record_version` on the way out.
    pub write_version: u16,
}

impl RecordLayer {
    /// A record layer with no keys: everything is `TLSPlaintext`.
    pub fn new() -> RecordLayer {
        RecordLayer {
            write_version: super::LEGACY_VERSION_TLS12,
            ..Default::default()
        }
    }

    /// Install new write keys and reset the write sequence number to zero.
    pub fn set_write(&mut self, keys: TrafficKeys) {
        self.write = Some(keys);
        self.write_seq = 0;
    }

    /// Install new read keys and reset the read sequence number to zero.
    pub fn set_read(&mut self, keys: TrafficKeys) {
        self.read = Some(keys);
        self.read_seq = 0;
    }

    /// True once records the client writes are protected.
    pub fn writing_encrypted(&self) -> bool {
        self.write.is_some()
    }

    /// True once records the client reads are protected.
    pub fn reading_encrypted(&self) -> bool {
        self.read.is_some()
    }

    /// Wrap `content` in a record, encrypting it when write keys are in force.
    ///
    /// The plaintext path writes the content type in the header; the encrypted path writes
    /// `application_data(23)` outside and puts the real type inside the ciphertext.
    pub fn seal(
        &mut self,
        content_type: ContentType,
        content: &[u8],
        padding: usize,
    ) -> TlsResult<Vec<u8>> {
        let Some(keys) = &self.write else {
            return Ok(Record::build(
                content_type.as_u8(),
                self.write_version,
                content,
            ));
        };
        let inner = InnerPlaintext::build(content_type, content, padding);
        let length = inner.len() + keys.aead.tag_len();
        if length > MAX_CIPHERTEXT {
            return Err(TlsError::Record(format!(
                "a protected record would be {length} bytes, more than the {MAX_CIPHERTEXT} limit"
            )));
        }
        let aad = [
            ContentType::ApplicationData.as_u8(),
            (self.write_version >> 8) as u8,
            self.write_version as u8,
            (length >> 8) as u8,
            length as u8,
        ];
        let nonce = keys.nonce(self.write_seq);
        let sealed = keys.aead.seal(&keys.key, &nonce, &aad, &inner)?;
        self.write_seq += 1;
        let mut out = Vec::with_capacity(5 + sealed.len());
        out.extend_from_slice(&aad);
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    /// Decrypt a protected record, advancing the read sequence number.
    pub fn open(&mut self, record: &Record) -> TlsResult<InnerPlaintext> {
        let Some(keys) = &self.read else {
            return Err(TlsError::Record(
                "cannot open a record: no read keys are in force yet".into(),
            ));
        };
        let nonce = keys.nonce(self.read_seq);
        let aad = record.header();
        let plain = keys
            .aead
            .open(&keys.key, &nonce, &aad, &record.fragment)
            .map_err(|e| match e {
                TlsError::Crypto(m) => TlsError::Crypto(format!(
                    "{m} — read sequence number {}, nonce {}",
                    self.read_seq,
                    super::hex(&nonce)
                )),
                other => other,
            })?;
        self.read_seq += 1;
        InnerPlaintext::parse(&plain)
    }

    /// The nonce the next written record will use, for a report line.
    pub fn next_write_nonce(&self) -> Option<Vec<u8>> {
        self.write.as_ref().map(|k| k.nonce(self.write_seq))
    }

    /// The nonce the next read record will use.
    pub fn next_read_nonce(&self) -> Option<Vec<u8>> {
        self.read.as_ref().map(|k| k.nonce(self.read_seq))
    }
}

/// Split `payload` into fragments no larger than `max`, the way a client that has to honour
/// the record size limit does.
pub fn fragment(payload: &[u8], max: usize) -> Vec<&[u8]> {
    if payload.is_empty() {
        return vec![&payload[..0]];
    }
    payload.chunks(max.max(1)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tls::crypto::{AeadAlg, HashAlg, Suite};

    fn keys() -> TrafficKeys {
        let suite = Suite {
            code: crate::tls::TLS_AES_128_GCM_SHA256,
            hash: HashAlg::Sha256,
            aead: AeadAlg::Aes128Gcm,
        };
        TrafficKeys::derive(suite, &[0x42; 32]).expect("derive traffic keys")
    }

    #[test]
    fn a_plaintext_record_round_trips() {
        let bytes = Record::build(22, 0x0303, b"hello");
        assert_eq!(bytes, vec![22, 3, 3, 0, 5, b'h', b'e', b'l', b'l', b'o']);
        let (r, used) = Record::parse(&bytes).expect("parse");
        assert_eq!(used, 10);
        assert_eq!(r.content_type, ContentType::Handshake);
        assert_eq!(r.legacy_version, 0x0303);
        assert_eq!(r.fragment, b"hello");
        assert_eq!(r.header(), [22, 3, 3, 0, 5]);
    }

    #[test]
    fn a_short_record_is_a_record_error_not_a_panic() {
        assert!(Record::parse(&[22, 3, 3]).is_err());
        assert!(Record::parse(&[22, 3, 3, 0, 9, 1, 2]).is_err());
    }

    #[test]
    fn oversized_records_are_rejected_at_the_documented_limits() {
        let plain = Record {
            content_type: ContentType::Handshake,
            legacy_version: 0x0303,
            fragment: vec![0; MAX_PLAINTEXT + 1],
            raw: Vec::new(),
        };
        assert!(plain.check_length(false).is_err());
        assert!(plain.check_length(true).is_ok(), "2^14+1 is a legal ciphertext");
        let big = Record {
            fragment: vec![0; MAX_CIPHERTEXT + 1],
            ..plain
        };
        assert!(big.check_length(true).is_err());
    }

    #[test]
    fn inner_plaintext_hides_the_type_behind_the_padding() {
        let inner = InnerPlaintext::build(ContentType::Handshake, b"abc", 4);
        assert_eq!(inner, vec![b'a', b'b', b'c', 22, 0, 0, 0, 0]);
        let parsed = InnerPlaintext::parse(&inner).expect("parse");
        assert_eq!(parsed.content, b"abc");
        assert_eq!(parsed.content_type, ContentType::Handshake);
        assert_eq!(parsed.padding, 4);
    }

    #[test]
    fn an_all_zero_inner_plaintext_is_an_error() {
        assert!(InnerPlaintext::parse(&[0, 0, 0]).is_err());
        assert!(InnerPlaintext::parse(&[]).is_err());
    }

    #[test]
    fn sealing_then_opening_gives_the_content_back() {
        let mut out = RecordLayer::new();
        let mut back = RecordLayer::new();
        out.set_write(keys());
        back.set_read(keys());
        for i in 0..4u8 {
            let content = vec![i; 10];
            let bytes = out
                .seal(ContentType::Handshake, &content, usize::from(i))
                .expect("seal");
            assert_eq!(bytes[0], 23, "the outer type is always application_data");
            let (record, used) = Record::parse(&bytes).expect("parse");
            assert_eq!(used, bytes.len());
            let inner = back.open(&record).expect("open");
            assert_eq!(inner.content, content);
            assert_eq!(inner.content_type, ContentType::Handshake);
            assert_eq!(inner.padding, usize::from(i));
        }
        assert_eq!(out.write_seq, 4);
        assert_eq!(back.read_seq, 4);
    }

    #[test]
    fn a_record_opened_out_of_order_fails_because_the_nonce_moved() {
        let mut out = RecordLayer::new();
        let mut back = RecordLayer::new();
        out.set_write(keys());
        back.set_read(keys());
        let first = out.seal(ContentType::ApplicationData, b"one", 0).expect("seal");
        let second = out.seal(ContentType::ApplicationData, b"two", 0).expect("seal");
        let (r2, _) = Record::parse(&second).expect("parse");
        assert!(
            back.open(&r2).is_err(),
            "record 1 must not open with the sequence-0 nonce"
        );
        let mut fresh = RecordLayer::new();
        fresh.set_read(keys());
        let (r1, _) = Record::parse(&first).expect("parse");
        assert!(fresh.open(&r1).is_ok());
    }

    #[test]
    fn installing_keys_resets_the_sequence_number() {
        let mut layer = RecordLayer::new();
        layer.set_write(keys());
        layer.seal(ContentType::Handshake, b"x", 0).expect("seal");
        assert_eq!(layer.write_seq, 1);
        layer.set_write(keys());
        assert_eq!(layer.write_seq, 0, "new keys mean a new sequence number");
    }

    #[test]
    fn fragmenting_never_loses_a_byte() {
        let payload: Vec<u8> = (0..100).collect();
        let parts = fragment(&payload, 30);
        assert_eq!(parts.len(), 4);
        let joined: Vec<u8> = parts.concat();
        assert_eq!(joined, payload);
        assert_eq!(fragment(&[], 10).len(), 1, "an empty record is still a record");
    }
}
