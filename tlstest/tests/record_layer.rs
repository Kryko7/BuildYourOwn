//! Record-layer round trips, against the rules of RFC 8446 §5 rather than a fixture.
//!
//! The unit tests inside `src/tls/record.rs` cover the happy path; these are the ones worth
//! running against the whole crate: a record built by the writer, framed by the parser,
//! opened by the reader, and every boundary condition the suite relies on.

use tlstest::tls::crypto::{Suite, TrafficKeys};
use tlstest::tls::msg::{encode_handshake, HandshakeMessage};
use tlstest::tls::record::{fragment, InnerPlaintext, Record, RecordLayer};
use tlstest::tls::{hex, ContentType, HandshakeType, ALL_SUITES, MAX_CIPHERTEXT, MAX_PLAINTEXT};

fn keys(code: u16) -> TrafficKeys {
    let suite = Suite::from_code(code).expect("a suite this client implements");
    TrafficKeys::derive(suite, &vec![0x2bu8; suite.hash.len()]).expect("derive traffic keys")
}

#[test]
fn a_record_survives_the_writer_and_the_parser_under_every_suite() {
    for code in ALL_SUITES {
        let mut out = RecordLayer::new();
        let mut back = RecordLayer::new();
        out.set_write(keys(code));
        back.set_read(keys(code));
        for (i, content) in [b"".as_slice(), b"x", &[7u8; 1000]].iter().enumerate() {
            let bytes = out
                .seal(ContentType::ApplicationData, content, i * 17)
                .expect("seal");
            let (record, used) = Record::parse(&bytes).expect("parse");
            assert_eq!(
                used,
                bytes.len(),
                "the parser must consume exactly one record"
            );
            assert_eq!(record.content_type, ContentType::ApplicationData);
            assert_eq!(record.legacy_version, 0x0303);
            record
                .check_length(true)
                .expect("within the ciphertext limit");
            let inner = back.open(&record).expect("open");
            assert_eq!(inner.content, *content);
            assert_eq!(inner.content_type, ContentType::ApplicationData);
            assert_eq!(inner.padding, i * 17);
        }
    }
}

#[test]
fn the_additional_data_is_the_header_with_the_ciphertext_length() {
    let mut out = RecordLayer::new();
    out.set_write(keys(tlstest::tls::TLS_AES_128_GCM_SHA256));
    let bytes = out.seal(ContentType::Handshake, b"hello", 0).expect("seal");
    let (record, _) = Record::parse(&bytes).expect("parse");
    assert_eq!(
        record.header(),
        [
            23,
            3,
            3,
            (record.fragment.len() >> 8) as u8,
            record.fragment.len() as u8
        ]
    );
    // 5 content + 1 inner type + 16 tag.
    assert_eq!(record.fragment.len(), 5 + 1 + 16);
    assert_eq!(&bytes[..5], &record.header());
}

#[test]
fn changing_any_header_byte_breaks_the_tag() {
    for at in 0..5usize {
        let mut out = RecordLayer::new();
        let mut back = RecordLayer::new();
        out.set_write(keys(tlstest::tls::TLS_AES_256_GCM_SHA384));
        back.set_read(keys(tlstest::tls::TLS_AES_256_GCM_SHA384));
        let mut bytes = out
            .seal(ContentType::ApplicationData, b"authenticated", 0)
            .expect("seal");
        bytes[at] ^= 0x01;
        let (record, _) = Record::parse(&bytes).unwrap_or_else(|_| {
            // A mangled length byte may stop it being a parsable record at all, which is
            // the same outcome from the peer's point of view.
            (
                Record {
                    content_type: ContentType::ApplicationData,
                    legacy_version: 0x0303,
                    fragment: vec![0; 4],
                    raw: Vec::new(),
                },
                0,
            )
        });
        assert!(
            back.open(&record).is_err(),
            "flipping header byte {at} must break the AEAD's additional data"
        );
    }
}

#[test]
fn the_sequence_number_drives_the_nonce_and_resets_with_the_keys() {
    let mut layer = RecordLayer::new();
    layer.set_write(keys(tlstest::tls::TLS_CHACHA20_POLY1305_SHA256));
    let first = layer.next_write_nonce().expect("a nonce");
    layer
        .seal(ContentType::ApplicationData, b"one", 0)
        .expect("seal");
    let second = layer.next_write_nonce().expect("a nonce");
    assert_ne!(hex(&first), hex(&second));
    assert_eq!(layer.write_seq, 1);
    layer.set_write(keys(tlstest::tls::TLS_CHACHA20_POLY1305_SHA256));
    assert_eq!(layer.write_seq, 0);
    assert_eq!(
        hex(&layer.next_write_nonce().expect("a nonce")),
        hex(&first)
    );
}

#[test]
fn the_plaintext_and_ciphertext_limits_are_applied_to_the_right_records() {
    let plain = Record {
        content_type: ContentType::Handshake,
        legacy_version: 0x0303,
        fragment: vec![0; MAX_PLAINTEXT],
        raw: Vec::new(),
    };
    plain
        .check_length(false)
        .expect("2^14 is the plaintext limit");
    let over = Record {
        fragment: vec![0; MAX_PLAINTEXT + 1],
        ..plain.clone()
    };
    assert!(over.check_length(false).is_err());
    assert!(
        over.check_length(true).is_ok(),
        "2^14+1 is a legal ciphertext"
    );
    let way_over = Record {
        fragment: vec![0; MAX_CIPHERTEXT + 1],
        ..plain
    };
    assert!(way_over.check_length(true).is_err());
}

#[test]
fn sealing_something_too_large_is_refused_rather_than_truncated() {
    let mut layer = RecordLayer::new();
    layer.set_write(keys(tlstest::tls::TLS_AES_128_GCM_SHA256));
    let err = layer
        .seal(ContentType::ApplicationData, &vec![0u8; MAX_CIPHERTEXT], 0)
        .expect_err("a record over the limit must be refused");
    assert!(err.to_string().contains("more than"), "{err}");
}

#[test]
fn handshake_messages_survive_any_fragmentation() {
    let message = encode_handshake(
        HandshakeType::CLIENT_HELLO,
        &(0..=255u8).collect::<Vec<u8>>(),
    );
    for size in [1usize, 7, 64, 255, 1000] {
        let mut joined = Vec::new();
        for part in fragment(&message, size) {
            let record = Record::build(22, 0x0303, part);
            let (parsed, _) = Record::parse(&record).expect("parse");
            joined.extend_from_slice(&parsed.fragment);
        }
        assert_eq!(joined, message, "fragmenting at {size} lost bytes");
        let (back, used) = HandshakeMessage::parse(&joined).expect("reassemble");
        assert_eq!(used, message.len());
        assert_eq!(back.msg_type, HandshakeType::CLIENT_HELLO);
        assert_eq!(back.raw, message);
    }
}

#[test]
fn inner_plaintext_finds_the_type_past_any_amount_of_padding() {
    for padding in [0usize, 1, 17, 1000] {
        let inner = InnerPlaintext::build(ContentType::Alert, b"\x01\x00", padding);
        assert_eq!(inner.len(), 2 + 1 + padding);
        let parsed = InnerPlaintext::parse(&inner).expect("parse");
        assert_eq!(parsed.content, b"\x01\x00");
        assert_eq!(parsed.content_type, ContentType::Alert);
        assert_eq!(parsed.padding, padding);
    }
    // An all-zero fragment has no content type at all.
    assert!(InnerPlaintext::parse(&[0, 0, 0, 0]).is_err());
    assert!(InnerPlaintext::parse(&[]).is_err());
}

#[test]
fn a_record_that_claims_more_than_it_carries_does_not_parse() {
    assert!(Record::parse(&[]).is_err());
    assert!(Record::parse(&[22, 3, 3, 0]).is_err());
    assert!(Record::parse(&[22, 3, 3, 0, 4, 1, 2, 3]).is_err());
    // And one that carries more than it claims parses exactly one record.
    let (record, used) = Record::parse(&[22, 3, 3, 0, 2, 1, 2, 9, 9, 9]).expect("parse");
    assert_eq!(used, 7);
    assert_eq!(record.fragment, vec![1, 2]);
}

#[test]
fn a_deliberately_wrong_length_field_can_still_be_built() {
    // The malformed-record stages need this: the writer must not "helpfully" fix it.
    let bytes = Record::build_with_length(22, 0x0303, 0xffff, b"short");
    assert_eq!(&bytes[..5], &[22, 3, 3, 0xff, 0xff]);
    assert_eq!(bytes.len(), 10);
    assert!(Record::parse(&bytes).is_err(), "and it must not parse");
}
