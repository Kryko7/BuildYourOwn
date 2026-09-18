//! Handshake messages: building a ClientHello field by field, and parsing everything the
//! server sends back (RFC 8446 §4).
//!
//! The ClientHello builder exposes every field a stage might want to get deliberately
//! wrong — `legacy_version`, `legacy_session_id`, the suite list, which groups get a real
//! key share, duplicated extensions, lying lengths — because most of section B is about
//! what a server does with an odd but legal hello, and what it does with an illegal one.

use super::buf::{Reader, Writer};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use super::{
    ext_name, group_name, AlertDescription, AlertLevel, HandshakeType, TlsError, TlsResult,
    EXT_ALPN, EXT_COOKIE, EXT_EARLY_DATA, EXT_KEY_SHARE, EXT_PRE_SHARED_KEY,
    EXT_PSK_KEY_EXCHANGE_MODES, EXT_SERVER_NAME, EXT_SIGNATURE_ALGORITHMS, EXT_SUPPORTED_GROUPS,
    EXT_SUPPORTED_VERSIONS, GROUP_SECP256R1, GROUP_X25519, LEGACY_VERSION_TLS12, TLS13_VERSION,
};

// ---------------------------------------------------------------------------------------
// Extensions
// ---------------------------------------------------------------------------------------

/// One extension: a code point and its `extension_data`, kept as raw bytes so anything can
/// be put in one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extension {
    /// The `extension_type` code point.
    pub ext_type: u16,
    /// The `extension_data`, without its own two-byte length.
    pub data: Vec<u8>,
}

impl Extension {
    /// An extension from a code point and its data.
    pub fn new(ext_type: u16, data: Vec<u8>) -> Extension {
        Extension { ext_type, data }
    }

    /// Its name, for a report line.
    pub fn name(&self) -> String {
        ext_name(self.ext_type)
    }

    /// Write `extension_type` + `extension_data`.
    pub fn encode(&self, w: &mut Writer) {
        w.u16(self.ext_type).vec16(&self.data);
    }
}

/// Encode an extension block: `Extension extensions<n..2^16-1>`.
pub fn encode_extensions(extensions: &[Extension]) -> Vec<u8> {
    let mut w = Writer::new();
    w.nest16(|inner| {
        for e in extensions {
            e.encode(inner);
        }
    });
    w.finish()
}

/// Parse an extension block that has already had its outer two-byte length removed.
pub fn parse_extensions(bytes: &[u8], path: &str) -> TlsResult<Vec<Extension>> {
    let mut r = Reader::new(bytes, path);
    let mut out = Vec::new();
    while !r.done() {
        let ext_type = r.u16(&format!("extensions[{}].extension_type", out.len()))?;
        let data = r.vec16(&format!("extensions[{}].extension_data", out.len()))?;
        out.push(Extension::new(ext_type, data.to_vec()));
    }
    Ok(out)
}

/// The first extension of this type, if there is one.
pub fn find_extension(extensions: &[Extension], ext_type: u16) -> Option<&Extension> {
    extensions.iter().find(|e| e.ext_type == ext_type)
}

/// `server_name` as a client sends it: one `host_name(0)` entry.
pub fn server_name_extension(host: &str) -> Extension {
    let mut w = Writer::new();
    w.nest16(|list| {
        list.u8(0).vec16(host.as_bytes());
    });
    Extension::new(EXT_SERVER_NAME, w.finish())
}

/// `application_layer_protocol_negotiation`: a list of protocol names.
pub fn alpn_extension(protocols: &[String]) -> Extension {
    let mut w = Writer::new();
    w.nest16(|list| {
        for p in protocols {
            list.vec8(p.as_bytes());
        }
    });
    Extension::new(EXT_ALPN, w.finish())
}

/// `supported_versions` as a *client* sends it: a one-byte-counted list.
pub fn supported_versions_extension(versions: &[u16]) -> Extension {
    let mut w = Writer::new();
    w.nest8(|list| {
        for v in versions {
            list.u16(*v);
        }
    });
    Extension::new(EXT_SUPPORTED_VERSIONS, w.finish())
}

/// `supported_groups`: the groups the client will do the maths for, share or no share.
pub fn supported_groups_extension(groups: &[u16]) -> Extension {
    let mut w = Writer::new();
    w.nest16(|list| {
        for g in groups {
            list.u16(*g);
        }
    });
    Extension::new(EXT_SUPPORTED_GROUPS, w.finish())
}

/// `signature_algorithms`: the schemes the client will accept in CertificateVerify.
pub fn signature_algorithms_extension(schemes: &[u16]) -> Extension {
    let mut w = Writer::new();
    w.nest16(|list| {
        for s in schemes {
            list.u16(*s);
        }
    });
    Extension::new(EXT_SIGNATURE_ALGORITHMS, w.finish())
}

/// `psk_key_exchange_modes`: 0 = psk_ke, 1 = psk_dhe_ke.
pub fn psk_key_exchange_modes_extension(modes: &[u8]) -> Extension {
    let mut w = Writer::new();
    w.nest8(|list| {
        for m in modes {
            list.u8(*m);
        }
    });
    Extension::new(EXT_PSK_KEY_EXCHANGE_MODES, w.finish())
}

/// `key_share` as a client sends it: `KeyShareEntry client_shares<0..2^16-1>`.
pub fn client_key_share_extension(shares: &[(u16, Vec<u8>)]) -> Extension {
    let mut w = Writer::new();
    w.nest16(|list| {
        for (group, key_exchange) in shares {
            list.u16(*group).vec16(key_exchange);
        }
    });
    Extension::new(EXT_KEY_SHARE, w.finish())
}

/// `cookie`, echoed back after a HelloRetryRequest asked for one.
pub fn cookie_extension(cookie: &[u8]) -> Extension {
    let mut w = Writer::new();
    w.vec16(cookie);
    Extension::new(EXT_COOKIE, w.finish())
}

/// `early_data` in a ClientHello: empty.
pub fn early_data_extension() -> Extension {
    Extension::new(EXT_EARLY_DATA, Vec::new())
}

// ---------------------------------------------------------------------------------------
// Key exchange
// ---------------------------------------------------------------------------------------

/// A client key share: the private half, the bytes that went on the wire, and the ability
/// to finish the (EC)DHE once the server's share comes back.
pub enum KeyExchange {
    /// X25519 (RFC 7748): 32 bytes each way, and the shared secret is the raw u-coordinate.
    X25519 {
        /// The client's private scalar.
        secret: Box<x25519_dalek::StaticSecret>,
        /// `key_exchange` as sent.
        public: Vec<u8>,
    },
    /// secp256r1: an uncompressed point, 65 bytes, and the shared secret is the x-coordinate.
    P256 {
        /// The client's private scalar.
        secret: Box<p256::SecretKey>,
        /// `key_exchange` as sent, `04 || X || Y`.
        public: Vec<u8>,
    },
}

impl std::fmt::Debug for KeyExchange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "KeyExchange({})", group_name(self.group()))
    }
}

impl KeyExchange {
    /// Build a key share for `group` from 32 bytes of (seeded) entropy.
    ///
    /// The bytes come from the run's RNG so a `--seed` reproduces the same ClientHello. A
    /// test client may do that; a real one must not.
    pub fn from_entropy(group: u16, entropy: &[u8; 32]) -> TlsResult<KeyExchange> {
        match group {
            GROUP_X25519 => {
                let secret = x25519_dalek::StaticSecret::from(*entropy);
                let public = x25519_dalek::PublicKey::from(&secret).as_bytes().to_vec();
                Ok(KeyExchange::X25519 {
                    secret: Box::new(secret),
                    public,
                })
            }
            GROUP_SECP256R1 => {
                // A scalar has to be in [1, n); derive again if the raw bytes are not.
                let mut bytes = *entropy;
                for round in 0u8..8 {
                    match p256::SecretKey::from_slice(&bytes) {
                        Ok(secret) => {
                            let public = secret
                                .public_key()
                                .to_encoded_point(false)
                                .as_bytes()
                                .to_vec();
                            return Ok(KeyExchange::P256 {
                                secret: Box::new(secret),
                                public,
                            });
                        }
                        Err(_) => {
                            bytes = super::crypto::HashAlg::Sha256
                                .digest(&[entropy.as_slice(), &[round]].concat())
                                .try_into()
                                .map_err(|_| {
                                    TlsError::Crypto("SHA-256 did not produce 32 bytes".into())
                                })?;
                        }
                    }
                }
                Err(TlsError::Crypto(
                    "could not derive a secp256r1 scalar from the seed".into(),
                ))
            }
            other => Err(TlsError::Crypto(format!(
                "this client cannot do the maths for {}",
                group_name(other)
            ))),
        }
    }

    /// The group this share belongs to.
    pub fn group(&self) -> u16 {
        match self {
            KeyExchange::X25519 { .. } => GROUP_X25519,
            KeyExchange::P256 { .. } => GROUP_SECP256R1,
        }
    }

    /// The `key_exchange` bytes that went into the ClientHello.
    pub fn public(&self) -> &[u8] {
        match self {
            KeyExchange::X25519 { public, .. } | KeyExchange::P256 { public, .. } => public,
        }
    }

    /// Finish the (EC)DHE with the server's `key_exchange`, yielding the shared secret that
    /// is extracted into the Handshake Secret.
    pub fn complete(&self, peer: &[u8]) -> TlsResult<Vec<u8>> {
        match self {
            KeyExchange::X25519 { secret, .. } => {
                let raw: [u8; 32] = peer.try_into().map_err(|_| {
                    TlsError::Crypto(format!(
                        "server_hello.key_share.key_exchange for x25519 must be 32 bytes, got {}",
                        peer.len()
                    ))
                })?;
                let shared = secret.diffie_hellman(&x25519_dalek::PublicKey::from(raw));
                if !shared.was_contributory() {
                    return Err(TlsError::Crypto(
                        "the x25519 shared secret is all zeros: the server sent a small-order \
                         point (RFC 8446 section 7.4.2 says abort with illegal_parameter)"
                            .into(),
                    ));
                }
                Ok(shared.as_bytes().to_vec())
            }
            KeyExchange::P256 { secret, .. } => {
                let point = p256::EncodedPoint::from_bytes(peer).map_err(|e| {
                    TlsError::Crypto(format!(
                        "server_hello.key_share.key_exchange is not a secp256r1 point: {e}"
                    ))
                })?;
                let public = p256::PublicKey::from_sec1_bytes(point.as_bytes()).map_err(|e| {
                    TlsError::Crypto(format!("server_hello.key_share.key_exchange: {e}"))
                })?;
                let shared = p256::ecdh::diffie_hellman(secret.to_nonzero_scalar(), public.as_affine());
                Ok(shared.raw_secret_bytes().to_vec())
            }
        }
    }

    /// How many bytes a `key_exchange` for this group is.
    pub fn share_len(group: u16) -> Option<usize> {
        match group {
            GROUP_X25519 => Some(32),
            GROUP_SECP256R1 => Some(65),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------------------
// ClientHello
// ---------------------------------------------------------------------------------------

/// Everything a ClientHello carries, with nothing enforced.
#[derive(Debug, Clone)]
pub struct ClientHello {
    /// `legacy_version`, which RFC 8446 pins at 0x0303 whatever version is really wanted.
    pub legacy_version: u16,
    /// `random`, 32 bytes.
    pub random: [u8; 32],
    /// `legacy_session_id`, which the server must echo byte for byte.
    pub legacy_session_id: Vec<u8>,
    /// `cipher_suites`, most preferred first.
    pub cipher_suites: Vec<u16>,
    /// `legacy_compression_methods`, which must be exactly `[0]`.
    pub legacy_compression_methods: Vec<u8>,
    /// The extensions, in the order they go on the wire.
    pub extensions: Vec<Extension>,
}

impl ClientHello {
    /// Encode the message body (without the four-byte handshake header).
    pub fn encode_body(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u16(self.legacy_version)
            .raw(&self.random)
            .vec8(&self.legacy_session_id)
            .nest16(|list| {
                for s in &self.cipher_suites {
                    list.u16(*s);
                }
            })
            .vec8(&self.legacy_compression_methods)
            .raw(&encode_extensions(&self.extensions));
        w.finish()
    }

    /// Encode the whole handshake message: `client_hello(1)` + a three-byte length + body.
    pub fn encode(&self) -> Vec<u8> {
        encode_handshake(HandshakeType::CLIENT_HELLO, &self.encode_body())
    }

    /// Parse a ClientHello body, which the annotator and the resumption stage both need.
    pub fn parse_body(body: &[u8]) -> TlsResult<ClientHello> {
        let mut r = Reader::new(body, "client_hello");
        let legacy_version = r.u16("legacy_version")?;
        let random: [u8; 32] = r
            .take(32, "random")?
            .try_into()
            .map_err(|_| TlsError::Decode("client_hello.random must be 32 bytes".into()))?;
        let legacy_session_id = r.vec8("legacy_session_id")?.to_vec();
        let suites = r.vec16("cipher_suites")?;
        let mut cipher_suites = Vec::new();
        let mut sr = Reader::new(suites, "client_hello.cipher_suites");
        while !sr.done() {
            cipher_suites.push(sr.u16("entry")?);
        }
        let legacy_compression_methods = r.vec8("legacy_compression_methods")?.to_vec();
        let ext_bytes = r.vec16("extensions")?;
        let extensions = parse_extensions(ext_bytes, "client_hello")?;
        r.expect_done("client_hello")?;
        Ok(ClientHello {
            legacy_version,
            random,
            legacy_session_id,
            cipher_suites,
            legacy_compression_methods,
            extensions,
        })
    }
}

// ---------------------------------------------------------------------------------------
// The handshake framing that sits inside records
// ---------------------------------------------------------------------------------------

/// One handshake message: `msg_type(1) length(3) body[length]`.
#[derive(Debug, Clone)]
pub struct HandshakeMessage {
    /// The `msg_type` byte.
    pub msg_type: HandshakeType,
    /// The body, without the four-byte header.
    pub body: Vec<u8>,
    /// The whole message including its header — this, and only this, goes into the transcript.
    pub raw: Vec<u8>,
}

impl HandshakeMessage {
    /// Parse one message from the front of `bytes`, returning it and how many bytes it used.
    pub fn parse(bytes: &[u8]) -> TlsResult<(HandshakeMessage, usize)> {
        if bytes.len() < 4 {
            return Err(TlsError::Decode(format!(
                "a handshake header is four bytes; only {} arrived",
                bytes.len()
            )));
        }
        let length = u32::from_be_bytes([0, bytes[1], bytes[2], bytes[3]]) as usize;
        if bytes.len() < 4 + length {
            return Err(TlsError::Decode(format!(
                "handshake message {} claims a {length}-byte body, {} arrived",
                HandshakeType(bytes[0]).name(),
                bytes.len() - 4
            )));
        }
        Ok((
            HandshakeMessage {
                msg_type: HandshakeType(bytes[0]),
                body: bytes[4..4 + length].to_vec(),
                raw: bytes[..4 + length].to_vec(),
            },
            4 + length,
        ))
    }

    /// The name of this message type.
    pub fn name(&self) -> String {
        self.msg_type.name()
    }
}

/// Wrap a body in a handshake header.
pub fn encode_handshake(msg_type: HandshakeType, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + body.len());
    out.push(msg_type.0);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]);
    out.extend_from_slice(body);
    out
}

// ---------------------------------------------------------------------------------------
// Parsed server messages
// ---------------------------------------------------------------------------------------

/// A parsed ServerHello (or HelloRetryRequest, which is a ServerHello with a fixed random).
#[derive(Debug, Clone)]
pub struct ServerHello {
    /// `legacy_version`, which must be 0x0303.
    pub legacy_version: u16,
    /// `random`; equal to [`super::HELLO_RETRY_REQUEST_RANDOM`] for a retry request.
    pub random: [u8; 32],
    /// `legacy_session_id_echo`, which must equal what the client sent.
    pub legacy_session_id_echo: Vec<u8>,
    /// The chosen `cipher_suite`.
    pub cipher_suite: u16,
    /// `legacy_compression_method`, which must be 0.
    pub legacy_compression_method: u8,
    /// The extensions.
    pub extensions: Vec<Extension>,
}

impl ServerHello {
    /// Parse a ServerHello body.
    pub fn parse(body: &[u8]) -> TlsResult<ServerHello> {
        let mut r = Reader::new(body, "server_hello");
        let legacy_version = r.u16("legacy_version")?;
        let random: [u8; 32] = r
            .take(32, "random")?
            .try_into()
            .map_err(|_| TlsError::Decode("server_hello.random must be 32 bytes".into()))?;
        let legacy_session_id_echo = r.vec8("legacy_session_id_echo")?.to_vec();
        let cipher_suite = r.u16("cipher_suite")?;
        let legacy_compression_method = r.u8("legacy_compression_method")?;
        let ext_bytes = r.vec16("extensions")?;
        let extensions = parse_extensions(ext_bytes, "server_hello")?;
        r.expect_done("server_hello")?;
        Ok(ServerHello {
            legacy_version,
            random,
            legacy_session_id_echo,
            cipher_suite,
            legacy_compression_method,
            extensions,
        })
    }

    /// True when this ServerHello is really a HelloRetryRequest.
    pub fn is_hello_retry_request(&self) -> bool {
        self.random == super::HELLO_RETRY_REQUEST_RANDOM
    }

    /// The version in the server's `supported_versions` extension, if it sent one.
    pub fn selected_version(&self) -> Option<u16> {
        let e = find_extension(&self.extensions, EXT_SUPPORTED_VERSIONS)?;
        (e.data.len() == 2).then(|| u16::from_be_bytes([e.data[0], e.data[1]]))
    }

    /// The server's `key_share`: `(group, key_exchange)`.
    pub fn key_share(&self) -> TlsResult<Option<(u16, Vec<u8>)>> {
        let Some(e) = find_extension(&self.extensions, EXT_KEY_SHARE) else {
            return Ok(None);
        };
        let mut r = Reader::new(&e.data, "server_hello.key_share");
        let group = r.u16("group")?;
        let key_exchange = r.vec16("key_exchange")?.to_vec();
        r.expect_done("key_share")?;
        Ok(Some((group, key_exchange)))
    }

    /// A HelloRetryRequest's `key_share`, which carries only the group it wants.
    pub fn retry_group(&self) -> TlsResult<Option<u16>> {
        let Some(e) = find_extension(&self.extensions, EXT_KEY_SHARE) else {
            return Ok(None);
        };
        let mut r = Reader::new(&e.data, "hello_retry_request.key_share");
        let group = r.u16("selected_group")?;
        r.expect_done("key_share")?;
        Ok(Some(group))
    }

    /// The `cookie` a HelloRetryRequest asked the client to echo.
    pub fn cookie(&self) -> TlsResult<Option<Vec<u8>>> {
        let Some(e) = find_extension(&self.extensions, EXT_COOKIE) else {
            return Ok(None);
        };
        let mut r = Reader::new(&e.data, "hello_retry_request.cookie");
        let cookie = r.vec16("cookie")?.to_vec();
        r.expect_done("cookie")?;
        Ok(Some(cookie))
    }

    /// The PSK identity index the server chose, when it accepted one.
    pub fn selected_psk(&self) -> TlsResult<Option<u16>> {
        let Some(e) = find_extension(&self.extensions, EXT_PRE_SHARED_KEY) else {
            return Ok(None);
        };
        let mut r = Reader::new(&e.data, "server_hello.pre_shared_key");
        let index = r.u16("selected_identity")?;
        r.expect_done("pre_shared_key")?;
        Ok(Some(index))
    }
}

/// A parsed Certificate message (RFC 8446 §4.4.2).
#[derive(Debug, Clone)]
pub struct CertificateMsg {
    /// `certificate_request_context`, empty in a server's Certificate.
    pub request_context: Vec<u8>,
    /// `(cert_data, extensions)` per entry, leaf first.
    pub entries: Vec<(Vec<u8>, Vec<Extension>)>,
}

impl CertificateMsg {
    /// Parse a Certificate body.
    pub fn parse(body: &[u8]) -> TlsResult<CertificateMsg> {
        let mut r = Reader::new(body, "certificate");
        let request_context = r.vec8("certificate_request_context")?.to_vec();
        let list = r.vec24("certificate_list")?;
        r.expect_done("certificate")?;
        let mut lr = Reader::new(list, "certificate.certificate_list");
        let mut entries = Vec::new();
        while !lr.done() {
            let i = entries.len();
            let cert_data = lr.vec24(&format!("[{i}].cert_data"))?.to_vec();
            let ext_bytes = lr.vec16(&format!("[{i}].extensions"))?;
            let extensions =
                parse_extensions(ext_bytes, &format!("certificate.certificate_list[{i}]"))?;
            entries.push((cert_data, extensions));
        }
        Ok(CertificateMsg {
            request_context,
            entries,
        })
    }

    /// The end-entity certificate, which RFC 8446 §4.4.2 puts first.
    pub fn leaf(&self) -> TlsResult<&[u8]> {
        self.entries
            .first()
            .map(|(d, _)| d.as_slice())
            .ok_or_else(|| {
                TlsError::Protocol(
                    "certificate.certificate_list is empty: a server must send its end-entity \
                     certificate first"
                        .into(),
                )
            })
    }
}

/// A parsed CertificateVerify (RFC 8446 §4.4.3).
#[derive(Debug, Clone)]
pub struct CertificateVerifyMsg {
    /// The `SignatureScheme` the server signed with.
    pub algorithm: u16,
    /// The signature itself.
    pub signature: Vec<u8>,
}

impl CertificateVerifyMsg {
    /// Parse a CertificateVerify body.
    pub fn parse(body: &[u8]) -> TlsResult<CertificateVerifyMsg> {
        let mut r = Reader::new(body, "certificate_verify");
        let algorithm = r.u16("algorithm")?;
        let signature = r.vec16("signature")?.to_vec();
        r.expect_done("certificate_verify")?;
        Ok(CertificateVerifyMsg {
            algorithm,
            signature,
        })
    }
}

/// A parsed NewSessionTicket (RFC 8446 §4.6.1).
#[derive(Debug, Clone)]
pub struct NewSessionTicket {
    /// How long the ticket is good for, in seconds.
    pub ticket_lifetime: u32,
    /// The obfuscation offset added to the client's `obfuscated_ticket_age`.
    pub ticket_age_add: u32,
    /// The nonce the resumption PSK is derived with.
    pub ticket_nonce: Vec<u8>,
    /// The opaque ticket, which becomes the PSK identity.
    pub ticket: Vec<u8>,
    /// The ticket's extensions (`early_data` lives here).
    pub extensions: Vec<Extension>,
}

impl NewSessionTicket {
    /// Parse a NewSessionTicket body.
    pub fn parse(body: &[u8]) -> TlsResult<NewSessionTicket> {
        let mut r = Reader::new(body, "new_session_ticket");
        let ticket_lifetime = r.u32("ticket_lifetime")?;
        let ticket_age_add = r.u32("ticket_age_add")?;
        let ticket_nonce = r.vec8("ticket_nonce")?.to_vec();
        let ticket = r.vec16("ticket")?.to_vec();
        let ext_bytes = r.vec16("extensions")?;
        let extensions = parse_extensions(ext_bytes, "new_session_ticket")?;
        r.expect_done("new_session_ticket")?;
        Ok(NewSessionTicket {
            ticket_lifetime,
            ticket_age_add,
            ticket_nonce,
            ticket,
            extensions,
        })
    }

    /// The `max_early_data_size` this ticket advertises, when it allows early data at all.
    pub fn max_early_data(&self) -> Option<u32> {
        let e = find_extension(&self.extensions, EXT_EARLY_DATA)?;
        (e.data.len() == 4)
            .then(|| u32::from_be_bytes([e.data[0], e.data[1], e.data[2], e.data[3]]))
    }
}

/// A KeyUpdate message: `update_not_requested(0)` or `update_requested(1)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyUpdate {
    /// The `request_update` byte.
    pub request_update: u8,
}

impl KeyUpdate {
    /// `update_not_requested(0)`
    pub const NOT_REQUESTED: KeyUpdate = KeyUpdate { request_update: 0 };
    /// `update_requested(1)`
    pub const REQUESTED: KeyUpdate = KeyUpdate { request_update: 1 };

    /// Parse a KeyUpdate body.
    pub fn parse(body: &[u8]) -> TlsResult<KeyUpdate> {
        let mut r = Reader::new(body, "key_update");
        let request_update = r.u8("request_update")?;
        r.expect_done("key_update")?;
        Ok(KeyUpdate { request_update })
    }

    /// Encode a whole KeyUpdate handshake message.
    pub fn encode(self) -> Vec<u8> {
        encode_handshake(HandshakeType::KEY_UPDATE, &[self.request_update])
    }

    /// The name of the `request_update` value.
    pub fn name(self) -> String {
        match self.request_update {
            0 => "update_not_requested(0)".into(),
            1 => "update_requested(1)".into(),
            other => format!("unknown({other})"),
        }
    }
}

/// A parsed alert record body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Alert {
    /// `warning(1)` or `fatal(2)`.
    pub level: AlertLevel,
    /// The description.
    pub description: AlertDescription,
}

impl Alert {
    /// Parse the two bytes of an alert.
    pub fn parse(body: &[u8]) -> TlsResult<Alert> {
        if body.len() != 2 {
            return Err(TlsError::Decode(format!(
                "an alert is exactly two bytes (level, description); {} arrived",
                body.len()
            )));
        }
        Ok(Alert {
            level: AlertLevel::from_u8(body[0]),
            description: AlertDescription(body[1]),
        })
    }

    /// The two bytes of an alert.
    pub fn encode(self) -> Vec<u8> {
        vec![self.level.as_u8(), self.description.0]
    }

    /// A fatal alert.
    pub fn fatal(description: AlertDescription) -> Alert {
        Alert {
            level: AlertLevel::Fatal,
            description,
        }
    }

    /// `warning close_notify`, the polite way to end a connection.
    pub fn close_notify() -> Alert {
        Alert {
            level: AlertLevel::Warning,
            description: AlertDescription::CLOSE_NOTIFY,
        }
    }

    /// How this alert reads in a report.
    pub fn describe(self) -> String {
        format!("{} {}", self.level.name(), self.description.name())
    }
}

/// The default `signature_algorithms` a client offers: everything this client can verify.
pub fn default_signature_algorithms() -> Vec<u16> {
    vec![
        super::SIG_ECDSA_SECP256R1_SHA256,
        super::SIG_RSA_PSS_RSAE_SHA256,
        super::SIG_RSA_PSS_RSAE_SHA384,
        super::SIG_RSA_PSS_RSAE_SHA512,
        super::SIG_ED25519,
        super::SIG_RSA_PKCS1_SHA256,
    ]
}

/// A ClientHello with every field at the value a well-behaved TLS 1.3 client uses.
pub fn default_client_hello(
    random: [u8; 32],
    session_id: Vec<u8>,
    shares: &[(u16, Vec<u8>)],
) -> ClientHello {
    let groups: Vec<u16> = vec![GROUP_X25519, GROUP_SECP256R1];
    ClientHello {
        legacy_version: LEGACY_VERSION_TLS12,
        random,
        legacy_session_id: session_id,
        cipher_suites: super::ALL_SUITES.to_vec(),
        legacy_compression_methods: vec![0],
        extensions: vec![
            supported_versions_extension(&[TLS13_VERSION]),
            supported_groups_extension(&groups),
            signature_algorithms_extension(&default_signature_algorithms()),
            client_key_share_extension(shares),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_hello_round_trips_through_its_own_parser() {
        let kx = KeyExchange::from_entropy(GROUP_X25519, &[7u8; 32]).expect("x25519");
        let hello = default_client_hello(
            [9u8; 32],
            vec![1, 2, 3, 4],
            &[(kx.group(), kx.public().to_vec())],
        );
        let body = hello.encode_body();
        let back = ClientHello::parse_body(&body).expect("parse");
        assert_eq!(back.legacy_version, 0x0303);
        assert_eq!(back.random, [9u8; 32]);
        assert_eq!(back.legacy_session_id, vec![1, 2, 3, 4]);
        assert_eq!(back.cipher_suites, super::super::ALL_SUITES.to_vec());
        assert_eq!(back.legacy_compression_methods, vec![0]);
        assert_eq!(back.extensions.len(), 4);
        assert!(find_extension(&back.extensions, EXT_KEY_SHARE).is_some());
    }

    #[test]
    fn the_handshake_header_is_a_type_byte_and_a_uint24() {
        let msg = encode_handshake(HandshakeType::CLIENT_HELLO, &[0xaa; 300]);
        assert_eq!(&msg[..4], &[1, 0, 1, 44]);
        let (parsed, used) = HandshakeMessage::parse(&msg).expect("parse");
        assert_eq!(used, 304);
        assert_eq!(parsed.msg_type, HandshakeType::CLIENT_HELLO);
        assert_eq!(parsed.body.len(), 300);
        assert_eq!(parsed.raw, msg);
    }

    #[test]
    fn two_messages_in_one_buffer_are_read_in_order() {
        let mut buf = encode_handshake(HandshakeType::ENCRYPTED_EXTENSIONS, &[0, 0]);
        buf.extend_from_slice(&encode_handshake(HandshakeType::FINISHED, &[1; 32]));
        let (first, used) = HandshakeMessage::parse(&buf).expect("first");
        assert_eq!(first.msg_type, HandshakeType::ENCRYPTED_EXTENSIONS);
        let (second, _) = HandshakeMessage::parse(&buf[used..]).expect("second");
        assert_eq!(second.msg_type, HandshakeType::FINISHED);
    }

    #[test]
    fn x25519_agrees_with_itself() {
        let a = KeyExchange::from_entropy(GROUP_X25519, &[1u8; 32]).expect("a");
        let b = KeyExchange::from_entropy(GROUP_X25519, &[2u8; 32]).expect("b");
        assert_eq!(a.public().len(), 32);
        let ab = a.complete(b.public()).expect("ab");
        let ba = b.complete(a.public()).expect("ba");
        assert_eq!(ab, ba);
        assert_eq!(ab.len(), 32);
    }

    #[test]
    fn secp256r1_agrees_with_itself_and_sends_an_uncompressed_point() {
        let a = KeyExchange::from_entropy(GROUP_SECP256R1, &[3u8; 32]).expect("a");
        let b = KeyExchange::from_entropy(GROUP_SECP256R1, &[4u8; 32]).expect("b");
        assert_eq!(a.public().len(), 65);
        assert_eq!(a.public()[0], 0x04, "TLS sends uncompressed points");
        assert_eq!(a.complete(b.public()).expect("ab"), b.complete(a.public()).expect("ba"));
    }

    #[test]
    fn a_small_order_x25519_point_is_refused() {
        let a = KeyExchange::from_entropy(GROUP_X25519, &[5u8; 32]).expect("a");
        assert!(a.complete(&[0u8; 32]).is_err(), "all-zero u must be rejected");
        assert!(a.complete(&[0u8; 31]).is_err(), "a short share must be rejected");
    }

    #[test]
    fn extensions_encode_with_two_length_prefixes() {
        let e = Extension::new(43, vec![2, 3, 4]);
        let bytes = encode_extensions(std::slice::from_ref(&e));
        assert_eq!(bytes, vec![0, 7, 0, 43, 0, 3, 2, 3, 4]);
        let back = parse_extensions(&bytes[2..], "t").expect("parse");
        assert_eq!(back, vec![e]);
    }

    #[test]
    fn the_server_name_extension_has_the_shape_rfc6066_gives_it() {
        let e = server_name_extension("example.test");
        // list length, name_type(0), host_name length, the name.
        assert_eq!(&e.data[..3], &[0, 15, 0]);
        assert_eq!(&e.data[5..], b"example.test");
    }

    #[test]
    fn alerts_parse_and_name_themselves() {
        let a = Alert::parse(&[2, 40]).expect("parse");
        assert_eq!(a.level, AlertLevel::Fatal);
        assert_eq!(a.description, AlertDescription::HANDSHAKE_FAILURE);
        assert_eq!(a.describe(), "fatal(2) handshake_failure(40)");
        assert_eq!(Alert::close_notify().encode(), vec![1, 0]);
        assert!(Alert::parse(&[2]).is_err());
    }
}
