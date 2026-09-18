//! Walking a byte string and naming every field in it.
//!
//! This is what turns a worked example from a wall of hex into something a learner can
//! read: each annotation is a byte range, a dotted field path and the decoded value. The
//! walk is deliberately strict — `--capture-examples` refuses an example whose walk does
//! not land exactly on the end of the byte string — because an annotation that has drifted
//! by two bytes is worse than no annotation at all.

use super::buf::Reader;
use super::msg::{Alert, HandshakeMessage};
use super::{
    ext_name, group_name, hex, sig_name, suite_name, AlertDescription, ContentType, HandshakeType,
    TlsResult, EXT_ALPN, EXT_COOKIE, EXT_KEY_SHARE, EXT_PRE_SHARED_KEY, EXT_SERVER_NAME,
    EXT_SIGNATURE_ALGORITHMS, EXT_SUPPORTED_GROUPS, EXT_SUPPORTED_VERSIONS,
};
use serde::{Deserialize, Serialize};

/// One annotated field inside a byte string.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FieldAnn {
    /// Byte offset into the hex string's bytes; offset 0 is the first byte shown.
    pub offset: usize,
    /// Length in bytes.
    pub length: usize,
    /// Field path, e.g. `record.length` or `client_hello.extensions[3].key_share.group`.
    pub field: String,
    /// The decoded value, written the way the report writes it.
    pub value: String,
    /// True when this value legitimately differs from run to run.
    #[serde(default, skip_serializing_if = "is_false")]
    pub varies: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Collects annotations while a walk is in progress.
#[derive(Debug, Default)]
pub struct Walk {
    /// The annotations collected so far.
    pub fields: Vec<FieldAnn>,
    /// Where the walk currently is.
    pub at: usize,
}

impl Walk {
    /// A walk starting at byte `at` of the byte string being annotated.
    pub fn at(at: usize) -> Walk {
        Walk {
            fields: Vec::new(),
            at,
        }
    }

    fn add(&mut self, length: usize, field: impl Into<String>, value: impl Into<String>) {
        self.fields.push(FieldAnn {
            offset: self.at,
            length,
            field: field.into(),
            value: value.into(),
            varies: false,
        });
        self.at += length;
    }

    fn add_varying(&mut self, length: usize, field: impl Into<String>, value: impl Into<String>) {
        self.add(length, field, value);
        if let Some(f) = self.fields.last_mut() {
            f.varies = true;
        }
    }

    fn skip(&mut self, length: usize) {
        self.at += length;
    }
}

/// Annotate a byte string that holds one or more TLS records.
///
/// Returns the annotations and whether the walk consumed the whole string.
pub fn records(bytes: &[u8]) -> (Vec<FieldAnn>, bool) {
    let mut walk = Walk::at(0);
    let mut offset = 0usize;
    let mut index = 0usize;
    while offset + 5 <= bytes.len() {
        let length = u16::from_be_bytes([bytes[offset + 3], bytes[offset + 4]]) as usize;
        if offset + 5 + length > bytes.len() {
            break;
        }
        let prefix = if index == 0 {
            String::new()
        } else {
            format!("record{index}.")
        };
        let content_type = ContentType::from_u8(bytes[offset]);
        walk.at = offset;
        walk.add(1, format!("{prefix}record.type"), content_type.name());
        walk.add(
            2,
            format!("{prefix}record.legacy_record_version"),
            format!(
                "0x{:04x}{}",
                u16::from_be_bytes([bytes[offset + 1], bytes[offset + 2]]),
                if u16::from_be_bytes([bytes[offset + 1], bytes[offset + 2]]) == 0x0301 {
                    " (the first-flight exception of RFC 8446 section 5.1)"
                } else {
                    ""
                }
            ),
        );
        walk.add(
            2,
            format!("{prefix}record.length"),
            format!("{length} (bytes that follow)"),
        );
        let fragment = &bytes[offset + 5..offset + 5 + length];
        match content_type {
            ContentType::Handshake => {
                let mut inner = 0usize;
                let mut n = 0usize;
                while inner < fragment.len() {
                    let Ok((message, used)) = HandshakeMessage::parse(&fragment[inner..]) else {
                        break;
                    };
                    let path = if n == 0 {
                        format!("{prefix}{}", short_name(message.msg_type))
                    } else {
                        format!("{prefix}{}#{n}", short_name(message.msg_type))
                    };
                    let mut sub = Walk::at(offset + 5 + inner);
                    handshake_into(&mut sub, &message, &path);
                    walk.fields.append(&mut sub.fields);
                    inner += used;
                    n += 1;
                }
                walk.at = offset + 5 + length;
            }
            ContentType::Alert => {
                if let Ok(alert) = Alert::parse(fragment) {
                    walk.add(1, format!("{prefix}alert.level"), alert.level.name());
                    walk.add(
                        1,
                        format!("{prefix}alert.description"),
                        alert.description.name(),
                    );
                } else {
                    walk.skip(length);
                }
            }
            ContentType::ChangeCipherSpec => {
                walk.add(
                    length,
                    format!("{prefix}change_cipher_spec"),
                    "0x01, the compatibility byte TLS 1.3 keeps for middleboxes".to_string(),
                );
            }
            ContentType::ApplicationData => {
                walk.add_varying(
                    length,
                    format!("{prefix}record.encrypted_record"),
                    format!(
                        "{length} bytes: the AEAD ciphertext plus its 16-byte tag. The real \
                         content type is inside it, after the content and before the padding.",
                    ),
                );
            }
            ContentType::Other(b) => {
                walk.add(
                    length,
                    format!("{prefix}record.fragment"),
                    format!("{length} bytes of a type {b} record"),
                );
            }
        }
        offset += 5 + length;
        index += 1;
    }
    let clean = offset == bytes.len() && !bytes.is_empty();
    (walk.fields, clean)
}

/// Annotate a byte string that holds one handshake message, header included.
pub fn handshake(bytes: &[u8]) -> (Vec<FieldAnn>, bool) {
    let Ok((message, used)) = HandshakeMessage::parse(bytes) else {
        return (Vec::new(), false);
    };
    let mut walk = Walk::at(0);
    let path = short_name(message.msg_type);
    handshake_into(&mut walk, &message, &path);
    (walk.fields, used == bytes.len())
}

/// Annotate several handshake messages laid end to end, the way a server's flight looks
/// once it has been decrypted.
pub fn handshake_flight(bytes: &[u8]) -> (Vec<FieldAnn>, bool) {
    let mut walk = Walk::at(0);
    let mut offset = 0usize;
    while offset < bytes.len() {
        let Ok((message, used)) = HandshakeMessage::parse(&bytes[offset..]) else {
            break;
        };
        let path = short_name(message.msg_type);
        let mut sub = Walk::at(offset);
        handshake_into(&mut sub, &message, &path);
        walk.fields.append(&mut sub.fields);
        offset += used;
    }
    (walk.fields, offset == bytes.len() && !bytes.is_empty())
}

fn short_name(t: HandshakeType) -> String {
    let full = t.name();
    full.split('(').next().unwrap_or(&full).to_string()
}

fn handshake_into(walk: &mut Walk, message: &HandshakeMessage, path: &str) {
    walk.add(1, format!("{path}.msg_type"), message.msg_type.name());
    walk.add(
        3,
        format!("{path}.length"),
        format!("{} (bytes of body that follow)", message.body.len()),
    );
    let body_at = walk.at;
    let body = &message.body;
    // A body that will not decode simply stops being annotated; the walk still has to land
    // on the right byte for whatever follows, which the line after the match takes care of.
    let _ = match message.msg_type {
        HandshakeType::CLIENT_HELLO => hello_into(walk, body, path, true),
        HandshakeType::SERVER_HELLO => hello_into(walk, body, path, false),
        HandshakeType::ENCRYPTED_EXTENSIONS => encrypted_extensions_into(walk, body, path),
        HandshakeType::CERTIFICATE => certificate_into(walk, body, path),
        HandshakeType::CERTIFICATE_VERIFY => certificate_verify_into(walk, body, path),
        HandshakeType::FINISHED => {
            walk.add_varying(
                body.len(),
                format!("{path}.verify_data"),
                format!(
                    "{} — HMAC(finished_key, Transcript-Hash(everything before this message))",
                    hex(body)
                ),
            );
            Ok(())
        }
        HandshakeType::NEW_SESSION_TICKET => new_session_ticket_into(walk, body, path),
        HandshakeType::KEY_UPDATE => {
            if let Some(b) = body.first() {
                walk.add(
                    1,
                    format!("{path}.request_update"),
                    super::msg::KeyUpdate { request_update: *b }.name(),
                );
            }
            Ok(())
        }
        _ => {
            walk.add(
                body.len(),
                format!("{path}.body"),
                format!("{} bytes", body.len()),
            );
            Ok(())
        }
    };
    // Whatever the per-type walk did — including giving up half way through a body it could
    // not decode — the next message starts after this one's declared length.
    walk.at = body_at + body.len();
}

fn hello_into(walk: &mut Walk, body: &[u8], path: &str, client: bool) -> TlsResult<()> {
    let mut r = Reader::new(body, path);
    let version = r.u16("legacy_version")?;
    walk.add(
        2,
        format!("{path}.legacy_version"),
        format!("0x{version:04x} (TLS 1.3 pins this at 0x0303; the real version is in supported_versions)"),
    );
    let random = r.take(32, "random")?;
    let note = if !client && random == super::HELLO_RETRY_REQUEST_RANDOM {
        " — the fixed SHA-256(\"HelloRetryRequest\") value that makes this a HelloRetryRequest"
    } else {
        ""
    };
    walk.add_varying(
        32,
        format!("{path}.random"),
        format!("{}{note}", hex(random)),
    );
    let session = r.vec8("legacy_session_id")?;
    walk.add(
        1,
        format!("{path}.legacy_session_id.length"),
        format!("{}", session.len()),
    );
    if !session.is_empty() {
        walk.add_varying(
            session.len(),
            format!(
                "{path}.{}",
                if client {
                    "legacy_session_id"
                } else {
                    "legacy_session_id_echo"
                }
            ),
            format!(
                "{}{}",
                hex(session),
                if client {
                    " (the server must echo this back byte for byte)"
                } else {
                    " (must equal the ClientHello's legacy_session_id)"
                }
            ),
        );
    }
    if client {
        let suites = r.vec16("cipher_suites")?;
        walk.add(
            2,
            format!("{path}.cipher_suites.length"),
            format!("{} bytes, {} suites", suites.len(), suites.len() / 2),
        );
        for (i, pair) in suites.chunks(2).enumerate() {
            if pair.len() == 2 {
                walk.add(
                    2,
                    format!("{path}.cipher_suites[{i}]"),
                    suite_name(u16::from_be_bytes([pair[0], pair[1]])),
                );
            }
        }
        let compression = r.vec8("legacy_compression_methods")?;
        walk.add(
            1,
            format!("{path}.legacy_compression_methods.length"),
            format!("{}", compression.len()),
        );
        walk.add(
            compression.len(),
            format!("{path}.legacy_compression_methods"),
            "null(0) — TLS 1.3 allows nothing else".to_string(),
        );
    } else {
        let suite = r.u16("cipher_suite")?;
        walk.add(2, format!("{path}.cipher_suite"), suite_name(suite));
        let compression = r.u8("legacy_compression_method")?;
        walk.add(
            1,
            format!("{path}.legacy_compression_method"),
            format!("{compression} (must be 0)"),
        );
    }
    let extensions = r.vec16("extensions")?;
    walk.add(
        2,
        format!("{path}.extensions.length"),
        format!("{} bytes of extensions", extensions.len()),
    );
    extensions_into(walk, extensions, path, client)?;
    Ok(())
}

fn encrypted_extensions_into(walk: &mut Walk, body: &[u8], path: &str) -> TlsResult<()> {
    let mut r = Reader::new(body, path);
    let extensions = r.vec16("extensions")?;
    walk.add(
        2,
        format!("{path}.extensions.length"),
        format!("{} bytes of extensions", extensions.len()),
    );
    extensions_into(walk, extensions, path, false)
}

fn extensions_into(walk: &mut Walk, bytes: &[u8], path: &str, client: bool) -> TlsResult<()> {
    let mut r = Reader::new(bytes, path);
    let mut index = 0usize;
    while !r.done() {
        let ext_type = r.u16("extension_type")?;
        let data = r.vec16("extension_data")?;
        let base = format!("{path}.extensions[{index}]");
        walk.add(2, format!("{base}.extension_type"), ext_name(ext_type));
        walk.add(2, format!("{base}.length"), format!("{} bytes", data.len()));
        let before = walk.at;
        extension_body_into(walk, ext_type, data, &base, client);
        walk.at = before + data.len();
        index += 1;
    }
    Ok(())
}

fn extension_body_into(walk: &mut Walk, ext_type: u16, data: &[u8], base: &str, client: bool) {
    match ext_type {
        EXT_SUPPORTED_VERSIONS if client => {
            if data.is_empty() {
                return;
            }
            walk.add(1, format!("{base}.versions.length"), format!("{}", data[0]));
            for (i, pair) in data[1..].chunks(2).enumerate() {
                if pair.len() == 2 {
                    walk.add(
                        2,
                        format!("{base}.versions[{i}]"),
                        version_name(u16::from_be_bytes([pair[0], pair[1]])),
                    );
                }
            }
        }
        EXT_SUPPORTED_VERSIONS => {
            if data.len() == 2 {
                walk.add(
                    2,
                    format!("{base}.selected_version"),
                    version_name(u16::from_be_bytes([data[0], data[1]])),
                );
            }
        }
        EXT_SUPPORTED_GROUPS => {
            if data.len() < 2 {
                return;
            }
            walk.add(
                2,
                format!("{base}.named_group_list.length"),
                format!("{} bytes", data.len() - 2),
            );
            for (i, pair) in data[2..].chunks(2).enumerate() {
                if pair.len() == 2 {
                    walk.add(
                        2,
                        format!("{base}.named_group_list[{i}]"),
                        group_name(u16::from_be_bytes([pair[0], pair[1]])),
                    );
                }
            }
        }
        EXT_SIGNATURE_ALGORITHMS => {
            if data.len() < 2 {
                return;
            }
            walk.add(
                2,
                format!("{base}.supported_signature_algorithms.length"),
                format!("{} bytes", data.len() - 2),
            );
            for (i, pair) in data[2..].chunks(2).enumerate() {
                if pair.len() == 2 {
                    walk.add(
                        2,
                        format!("{base}.supported_signature_algorithms[{i}]"),
                        sig_name(u16::from_be_bytes([pair[0], pair[1]])),
                    );
                }
            }
        }
        EXT_KEY_SHARE if client => {
            if data.len() < 2 {
                return;
            }
            walk.add(
                2,
                format!("{base}.client_shares.length"),
                format!("{} bytes", data.len() - 2),
            );
            let mut r = Reader::new(&data[2..], base);
            let mut i = 0;
            while !r.done() {
                let Ok(group) = r.u16("group") else { break };
                let Ok(share) = r.vec16("key_exchange") else {
                    break;
                };
                walk.add(
                    2,
                    format!("{base}.client_shares[{i}].group"),
                    group_name(group),
                );
                walk.add(
                    2,
                    format!("{base}.client_shares[{i}].key_exchange.length"),
                    format!("{}", share.len()),
                );
                walk.add_varying(
                    share.len(),
                    format!("{base}.client_shares[{i}].key_exchange"),
                    format!(
                        "{} — the client's ephemeral public key",
                        super::hex_prefix(share, 8)
                    ),
                );
                i += 1;
            }
        }
        EXT_KEY_SHARE => {
            if data.len() == 2 {
                walk.add(
                    2,
                    format!("{base}.selected_group"),
                    format!(
                        "{} — a HelloRetryRequest names a group and sends no share",
                        group_name(u16::from_be_bytes([data[0], data[1]]))
                    ),
                );
                return;
            }
            if data.len() < 4 {
                return;
            }
            walk.add(
                2,
                format!("{base}.server_share.group"),
                group_name(u16::from_be_bytes([data[0], data[1]])),
            );
            let len = u16::from_be_bytes([data[2], data[3]]) as usize;
            walk.add(
                2,
                format!("{base}.server_share.key_exchange.length"),
                format!("{len}"),
            );
            walk.add_varying(
                len.min(data.len().saturating_sub(4)),
                format!("{base}.server_share.key_exchange"),
                format!(
                    "{} — the server's ephemeral public key; the (EC)DHE shared secret comes from this",
                    super::hex_prefix(&data[4..], 8)
                ),
            );
        }
        EXT_SERVER_NAME if client => {
            if data.len() < 5 {
                return;
            }
            walk.add(
                2,
                format!("{base}.server_name_list.length"),
                format!("{} bytes", data.len() - 2),
            );
            walk.add(
                1,
                format!("{base}.server_name_list[0].name_type"),
                "host_name(0)".to_string(),
            );
            let len = u16::from_be_bytes([data[3], data[4]]) as usize;
            walk.add(
                2,
                format!("{base}.server_name_list[0].host_name.length"),
                format!("{len}"),
            );
            walk.add(
                len.min(data.len().saturating_sub(5)),
                format!("{base}.server_name_list[0].host_name"),
                format!("'{}'", String::from_utf8_lossy(&data[5..])),
            );
        }
        EXT_ALPN => {
            if data.len() < 3 {
                return;
            }
            walk.add(
                2,
                format!("{base}.protocol_name_list.length"),
                format!("{} bytes", data.len() - 2),
            );
            let mut r = Reader::new(&data[2..], base);
            let mut i = 0;
            while !r.done() {
                let Ok(name) = r.vec8("protocol_name") else {
                    break;
                };
                walk.add(
                    1,
                    format!("{base}.protocol_name_list[{i}].length"),
                    format!("{}", name.len()),
                );
                walk.add(
                    name.len(),
                    format!("{base}.protocol_name_list[{i}]"),
                    format!("'{}'", String::from_utf8_lossy(name)),
                );
                i += 1;
            }
        }
        EXT_COOKIE => {
            if data.len() < 2 {
                return;
            }
            walk.add(
                2,
                format!("{base}.cookie.length"),
                format!("{}", data.len() - 2),
            );
            walk.add_varying(
                data.len() - 2,
                format!("{base}.cookie"),
                format!(
                    "{} — echoed back in the second ClientHello",
                    super::hex_prefix(&data[2..], 8)
                ),
            );
        }
        EXT_PRE_SHARED_KEY if !client => {
            if data.len() == 2 {
                walk.add(
                    2,
                    format!("{base}.selected_identity"),
                    format!(
                        "{} — the index of the PSK the server took from the ClientHello's list",
                        u16::from_be_bytes([data[0], data[1]])
                    ),
                );
            }
        }
        _ if data.is_empty() => {
            walk.fields.push(FieldAnn {
                offset: walk.at,
                length: 0,
                field: format!("{base}.extension_data"),
                value: "empty — this extension is a flag".to_string(),
                varies: false,
            });
        }
        _ => {
            walk.add_varying(
                data.len(),
                format!("{base}.extension_data"),
                format!("{} bytes: {}", data.len(), super::hex_prefix(data, 8)),
            );
        }
    }
}

fn certificate_into(walk: &mut Walk, body: &[u8], path: &str) -> TlsResult<()> {
    let mut r = Reader::new(body, path);
    let context = r.vec8("certificate_request_context")?;
    walk.add(
        1,
        format!("{path}.certificate_request_context.length"),
        format!(
            "{} — a server's Certificate always carries an empty context",
            context.len()
        ),
    );
    walk.skip(context.len());
    let list = r.vec24("certificate_list")?;
    walk.add(
        3,
        format!("{path}.certificate_list.length"),
        format!("{} bytes", list.len()),
    );
    let mut lr = Reader::new(list, path);
    let mut i = 0;
    while !lr.done() {
        let Ok(cert) = lr.vec24("cert_data") else {
            break;
        };
        let Ok(exts) = lr.vec16("extensions") else {
            break;
        };
        walk.add(
            3,
            format!("{path}.certificate_list[{i}].cert_data.length"),
            format!("{} bytes", cert.len()),
        );
        walk.add_varying(
            cert.len(),
            format!("{path}.certificate_list[{i}].cert_data"),
            format!(
                "a DER X.509 certificate{}",
                if i == 0 {
                    " — entry 0 is the end-entity certificate, and each later entry certifies the one before it"
                } else {
                    ""
                }
            ),
        );
        walk.add(
            2,
            format!("{path}.certificate_list[{i}].extensions.length"),
            format!(
                "{} — every entry carries its own extensions block, new in TLS 1.3",
                exts.len()
            ),
        );
        walk.skip(exts.len());
        i += 1;
    }
    Ok(())
}

fn certificate_verify_into(walk: &mut Walk, body: &[u8], path: &str) -> TlsResult<()> {
    let mut r = Reader::new(body, path);
    let algorithm = r.u16("algorithm")?;
    walk.add(2, format!("{path}.algorithm"), sig_name(algorithm));
    let signature = r.vec16("signature")?;
    walk.add(
        2,
        format!("{path}.signature.length"),
        format!("{} bytes", signature.len()),
    );
    walk.add_varying(
        signature.len(),
        format!("{path}.signature"),
        format!(
            "over 64 spaces, \"TLS 1.3, server CertificateVerify\", a zero byte and \
             Transcript-Hash(ClientHello..Certificate) — {}",
            super::hex_prefix(signature, 8)
        ),
    );
    Ok(())
}

fn new_session_ticket_into(walk: &mut Walk, body: &[u8], path: &str) -> TlsResult<()> {
    let mut r = Reader::new(body, path);
    let lifetime = r.u32("ticket_lifetime")?;
    walk.add(
        4,
        format!("{path}.ticket_lifetime"),
        format!("{lifetime} seconds"),
    );
    let age_add = r.u32("ticket_age_add")?;
    walk.add_varying(
        4,
        format!("{path}.ticket_age_add"),
        format!("0x{age_add:08x} — added to the client's ticket age so it cannot be correlated"),
    );
    let nonce = r.vec8("ticket_nonce")?;
    walk.add(
        1,
        format!("{path}.ticket_nonce.length"),
        format!("{}", nonce.len()),
    );
    walk.add(
        nonce.len(),
        format!("{path}.ticket_nonce"),
        format!("{} — the PSK is HKDF-Expand-Label(resumption_master_secret, \"resumption\", this, Hash.length)", hex(nonce)),
    );
    let ticket = r.vec16("ticket")?;
    walk.add(
        2,
        format!("{path}.ticket.length"),
        format!("{} bytes", ticket.len()),
    );
    walk.add_varying(
        ticket.len(),
        format!("{path}.ticket"),
        format!(
            "{} — opaque to the client; it becomes the PSK identity",
            super::hex_prefix(ticket, 8)
        ),
    );
    let exts = r.vec16("extensions")?;
    walk.add(
        2,
        format!("{path}.extensions.length"),
        format!(
            "{} — an `early_data` extension here would carry max_early_data_size",
            exts.len()
        ),
    );
    walk.skip(exts.len());
    Ok(())
}

fn version_name(v: u16) -> String {
    match v {
        0x0304 => "TLS 1.3 (0x0304)".into(),
        0x0303 => "TLS 1.2 (0x0303)".into(),
        0x0302 => "TLS 1.1 (0x0302)".into(),
        0x0301 => "TLS 1.0 (0x0301)".into(),
        other if super::is_grease(other) => format!("GREASE(0x{other:04x})"),
        other => format!("unknown(0x{other:04x})"),
    }
}

/// The alert an annotated byte string ends with, if it ends with one.
pub fn trailing_alert(bytes: &[u8]) -> Option<(AlertDescription, usize)> {
    let mut offset = 0usize;
    let mut last = None;
    while offset + 5 <= bytes.len() {
        let length = u16::from_be_bytes([bytes[offset + 3], bytes[offset + 4]]) as usize;
        if offset + 5 + length > bytes.len() {
            break;
        }
        if ContentType::from_u8(bytes[offset]) == ContentType::Alert && length == 2 {
            last = Some((AlertDescription(bytes[offset + 6]), offset));
        }
        offset += 5 + length;
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tls::client::ClientConfig;
    use crate::tls::msg::{default_client_hello, KeyExchange};
    use crate::tls::record::Record;
    use crate::tls::GROUP_X25519;

    fn a_client_hello() -> Vec<u8> {
        let config = ClientConfig::seeded(11);
        let kx = KeyExchange::from_entropy(GROUP_X25519, &[4u8; 32]).expect("kx");
        default_client_hello(
            config.random(),
            config.session_id.clone(),
            &[(kx.group(), kx.public().to_vec())],
        )
        .encode()
    }

    #[test]
    fn a_client_hello_record_walks_to_the_last_byte() {
        let record = Record::build(22, 0x0303, &a_client_hello());
        let (fields, clean) = records(&record);
        assert!(clean, "the walk must consume the whole record");
        assert_eq!(fields[0].field, "record.type");
        assert_eq!(fields[0].value, "handshake(22)");
        assert_eq!(fields[1].field, "record.legacy_record_version");
        assert_eq!(fields[2].field, "record.length");
        assert!(fields.iter().any(|f| f.field == "client_hello.msg_type"));
        assert!(fields
            .iter()
            .any(|f| f.field == "client_hello.random" && f.varies));
        assert!(fields
            .iter()
            .any(|f| f.value.contains("TLS_AES_128_GCM_SHA256")));
        assert!(fields
            .iter()
            .any(|f| f.field.ends_with("client_shares[0].group")));
    }

    #[test]
    fn every_annotation_lies_inside_the_bytes_and_none_overlap_backwards() {
        let record = Record::build(22, 0x0303, &a_client_hello());
        let (fields, _) = records(&record);
        let mut last_end = 0usize;
        for f in &fields {
            assert!(
                f.offset + f.length <= record.len(),
                "{} runs past the end of the bytes",
                f.field
            );
            assert!(
                f.offset >= last_end.saturating_sub(f.length.max(1)),
                "{} goes backwards",
                f.field
            );
            last_end = f.offset + f.length;
        }
    }

    #[test]
    fn an_alert_record_is_named_by_level_and_description() {
        let (fields, clean) = records(&Record::build(21, 0x0303, &[2, 40]));
        assert!(clean);
        assert_eq!(fields[3].field, "alert.level");
        assert_eq!(fields[3].value, "fatal(2)");
        assert_eq!(fields[4].value, "handshake_failure(40)");
        assert_eq!(
            trailing_alert(&Record::build(21, 0x0303, &[2, 40])).map(|(a, _)| a.0),
            Some(40)
        );
    }

    #[test]
    fn the_change_cipher_spec_record_is_explained() {
        let (fields, clean) = records(&Record::build(20, 0x0303, &[1]));
        assert!(clean);
        assert!(fields[3].value.contains("middlebox"), "{:?}", fields[3]);
    }

    #[test]
    fn two_messages_in_one_record_are_both_walked() {
        let mut fragment = crate::tls::msg::encode_handshake(HandshakeType::FINISHED, &[1u8; 32]);
        fragment.extend_from_slice(&crate::tls::msg::encode_handshake(
            HandshakeType::KEY_UPDATE,
            &[1],
        ));
        let (fields, clean) = records(&Record::build(22, 0x0303, &fragment));
        assert!(clean);
        assert!(fields.iter().any(|f| f.field == "finished.msg_type"));
        assert!(fields.iter().any(|f| f.field == "key_update#1.msg_type"));
    }

    #[test]
    fn a_truncated_record_is_reported_as_an_unclean_walk() {
        let (_, clean) = records(&[22, 3, 3, 0, 10, 1, 2, 3]);
        assert!(!clean);
    }
}
