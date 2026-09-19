//! A raw TLS 1.3 client, written for this suite.
//!
//! Nothing here hides a byte. [`buf`] is the TLS byte grammar (`uint8`, `uint16`, `uint24`
//! and the length-prefixed vectors of RFC 8446 §3), [`crypto`] is the key schedule with its
//! labels spelled out, [`record`] is the record layer, [`msg`] builds and parses handshake
//! messages, [`conn`] is one connection and [`client`] drives a whole handshake. Every step
//! leaves a [`conn::Trace`] entry behind, which is what lets a failing test print the
//! message that went wrong, its hex, the transcript hash that was in force and the
//! key-schedule label being derived at that moment.
//!
//! A full client stack (`rustls`) would be shorter and would tell a learner nothing.

pub mod annotate;
pub mod buf;
pub mod client;
pub mod conn;
pub mod crypto;
pub mod msg;
pub mod record;
pub mod sig;

use std::fmt;

// ---------------------------------------------------------------------------------------
// Record and handshake type codes (RFC 8446 §5.1, §4)
// ---------------------------------------------------------------------------------------

/// `ContentType` of a TLS record (RFC 8446 §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    /// 20 — the compatibility `ChangeCipherSpec` record.
    ChangeCipherSpec,
    /// 21 — an alert.
    Alert,
    /// 22 — a handshake message fragment.
    Handshake,
    /// 23 — application data (and every encrypted record).
    ApplicationData,
    /// Anything else, kept as the raw byte so a test can say what arrived.
    Other(u8),
}

impl ContentType {
    /// The byte this type is written as.
    pub fn as_u8(self) -> u8 {
        match self {
            ContentType::ChangeCipherSpec => 20,
            ContentType::Alert => 21,
            ContentType::Handshake => 22,
            ContentType::ApplicationData => 23,
            ContentType::Other(b) => b,
        }
    }

    /// Read a type byte.
    pub fn from_u8(b: u8) -> ContentType {
        match b {
            20 => ContentType::ChangeCipherSpec,
            21 => ContentType::Alert,
            22 => ContentType::Handshake,
            23 => ContentType::ApplicationData,
            other => ContentType::Other(other),
        }
    }

    /// The name RFC 8446 gives this type.
    pub fn name(self) -> String {
        match self {
            ContentType::ChangeCipherSpec => "change_cipher_spec(20)".into(),
            ContentType::Alert => "alert(21)".into(),
            ContentType::Handshake => "handshake(22)".into(),
            ContentType::ApplicationData => "application_data(23)".into(),
            ContentType::Other(b) => format!("unknown({b})"),
        }
    }
}

/// `HandshakeType` (RFC 8446 §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandshakeType(pub u8);

impl HandshakeType {
    /// `client_hello(1)`
    pub const CLIENT_HELLO: HandshakeType = HandshakeType(1);
    /// `server_hello(2)`
    pub const SERVER_HELLO: HandshakeType = HandshakeType(2);
    /// `new_session_ticket(4)`
    pub const NEW_SESSION_TICKET: HandshakeType = HandshakeType(4);
    /// `end_of_early_data(5)`
    pub const END_OF_EARLY_DATA: HandshakeType = HandshakeType(5);
    /// `encrypted_extensions(8)`
    pub const ENCRYPTED_EXTENSIONS: HandshakeType = HandshakeType(8);
    /// `certificate(11)`
    pub const CERTIFICATE: HandshakeType = HandshakeType(11);
    /// `certificate_request(13)`
    pub const CERTIFICATE_REQUEST: HandshakeType = HandshakeType(13);
    /// `certificate_verify(15)`
    pub const CERTIFICATE_VERIFY: HandshakeType = HandshakeType(15);
    /// `finished(20)`
    pub const FINISHED: HandshakeType = HandshakeType(20);
    /// `key_update(24)`
    pub const KEY_UPDATE: HandshakeType = HandshakeType(24);
    /// `message_hash(254)`, the synthetic message of a HelloRetryRequest transcript.
    pub const MESSAGE_HASH: HandshakeType = HandshakeType(254);
    /// `hello_request(0)` from TLS 1.2, which a 1.3 server must never send.
    pub const HELLO_REQUEST: HandshakeType = HandshakeType(0);

    /// The name RFC 8446 gives this type.
    pub fn name(self) -> String {
        let n = match self.0 {
            0 => "hello_request_RESERVED",
            1 => "client_hello",
            2 => "server_hello",
            4 => "new_session_ticket",
            5 => "end_of_early_data",
            8 => "encrypted_extensions",
            11 => "certificate",
            13 => "certificate_request",
            15 => "certificate_verify",
            20 => "finished",
            24 => "key_update",
            254 => "message_hash",
            _ => return format!("unknown({})", self.0),
        };
        format!("{n}({})", self.0)
    }
}

/// `AlertLevel` (RFC 8446 §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertLevel {
    /// 1 — in TLS 1.3 only `close_notify` and `user_canceled` are warnings.
    Warning,
    /// 2 — every other alert.
    Fatal,
    /// Anything else.
    Other(u8),
}

impl AlertLevel {
    /// Read a level byte.
    pub fn from_u8(b: u8) -> AlertLevel {
        match b {
            1 => AlertLevel::Warning,
            2 => AlertLevel::Fatal,
            other => AlertLevel::Other(other),
        }
    }

    /// The byte this level is written as.
    pub fn as_u8(self) -> u8 {
        match self {
            AlertLevel::Warning => 1,
            AlertLevel::Fatal => 2,
            AlertLevel::Other(b) => b,
        }
    }

    /// The name RFC 8446 gives this level.
    pub fn name(self) -> String {
        match self {
            AlertLevel::Warning => "warning(1)".into(),
            AlertLevel::Fatal => "fatal(2)".into(),
            AlertLevel::Other(b) => format!("unknown({b})"),
        }
    }
}

/// An `AlertDescription` (RFC 8446 §6), kept as its byte so unknown codes survive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlertDescription(pub u8);

impl AlertDescription {
    /// `close_notify(0)`
    pub const CLOSE_NOTIFY: AlertDescription = AlertDescription(0);
    /// `unexpected_message(10)`
    pub const UNEXPECTED_MESSAGE: AlertDescription = AlertDescription(10);
    /// `bad_record_mac(20)`
    pub const BAD_RECORD_MAC: AlertDescription = AlertDescription(20);
    /// `record_overflow(22)`
    pub const RECORD_OVERFLOW: AlertDescription = AlertDescription(22);
    /// `handshake_failure(40)`
    pub const HANDSHAKE_FAILURE: AlertDescription = AlertDescription(40);
    /// `bad_certificate(42)`
    pub const BAD_CERTIFICATE: AlertDescription = AlertDescription(42);
    /// `illegal_parameter(47)`
    pub const ILLEGAL_PARAMETER: AlertDescription = AlertDescription(47);
    /// `decode_error(50)`
    pub const DECODE_ERROR: AlertDescription = AlertDescription(50);
    /// `decrypt_error(51)`
    pub const DECRYPT_ERROR: AlertDescription = AlertDescription(51);
    /// `protocol_version(70)`
    pub const PROTOCOL_VERSION: AlertDescription = AlertDescription(70);
    /// `insufficient_security(71)`
    pub const INSUFFICIENT_SECURITY: AlertDescription = AlertDescription(71);
    /// `internal_error(80)`
    pub const INTERNAL_ERROR: AlertDescription = AlertDescription(80);
    /// `inappropriate_fallback(86)`
    pub const INAPPROPRIATE_FALLBACK: AlertDescription = AlertDescription(86);
    /// `user_canceled(90)`
    pub const USER_CANCELED: AlertDescription = AlertDescription(90);
    /// `missing_extension(109)`
    pub const MISSING_EXTENSION: AlertDescription = AlertDescription(109);
    /// `unsupported_extension(110)`
    pub const UNSUPPORTED_EXTENSION: AlertDescription = AlertDescription(110);
    /// `unrecognized_name(112)`
    pub const UNRECOGNIZED_NAME: AlertDescription = AlertDescription(112);
    /// `no_renegotiation(100)`, a TLS 1.2 alert a server may still answer with.
    pub const NO_RENEGOTIATION: AlertDescription = AlertDescription(100);
    /// `certificate_required(116)`
    pub const CERTIFICATE_REQUIRED: AlertDescription = AlertDescription(116);
    /// The peer's certificate is not one the receiver can build a trusted chain for.
    pub const UNKNOWN_CA: AlertDescription = AlertDescription(48);
    /// The certificate was rejected for a reason with no more specific alert.
    pub const CERTIFICATE_UNKNOWN: AlertDescription = AlertDescription(46);
    /// `no_application_protocol(120)`
    pub const NO_APPLICATION_PROTOCOL: AlertDescription = AlertDescription(120);

    /// The name RFC 8446 gives this description.
    pub fn name(self) -> String {
        let n = match self.0 {
            0 => "close_notify",
            10 => "unexpected_message",
            20 => "bad_record_mac",
            21 => "decryption_failed_RESERVED",
            22 => "record_overflow",
            40 => "handshake_failure",
            42 => "bad_certificate",
            43 => "unsupported_certificate",
            44 => "certificate_revoked",
            45 => "certificate_expired",
            46 => "certificate_unknown",
            47 => "illegal_parameter",
            48 => "unknown_ca",
            49 => "access_denied",
            50 => "decode_error",
            51 => "decrypt_error",
            70 => "protocol_version",
            71 => "insufficient_security",
            80 => "internal_error",
            86 => "inappropriate_fallback",
            90 => "user_canceled",
            100 => "no_renegotiation_RESERVED",
            109 => "missing_extension",
            110 => "unsupported_extension",
            112 => "unrecognized_name",
            113 => "bad_certificate_status_response",
            115 => "unknown_psk_identity",
            116 => "certificate_required",
            120 => "no_application_protocol",
            _ => return format!("unknown({})", self.0),
        };
        format!("{n}({})", self.0)
    }
}

impl fmt::Display for AlertDescription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

// ---------------------------------------------------------------------------------------
// Negotiation code points
// ---------------------------------------------------------------------------------------

/// `TLS_AES_128_GCM_SHA256`
pub const TLS_AES_128_GCM_SHA256: u16 = 0x1301;
/// `TLS_AES_256_GCM_SHA384`
pub const TLS_AES_256_GCM_SHA384: u16 = 0x1302;
/// `TLS_CHACHA20_POLY1305_SHA256`
pub const TLS_CHACHA20_POLY1305_SHA256: u16 = 0x1303;

/// The three mandatory-to-name TLS 1.3 cipher suites, in the order a client usually offers.
pub const ALL_SUITES: [u16; 3] = [
    TLS_AES_256_GCM_SHA384,
    TLS_CHACHA20_POLY1305_SHA256,
    TLS_AES_128_GCM_SHA256,
];

/// The name of a cipher suite code point.
pub fn suite_name(suite: u16) -> String {
    match suite {
        TLS_AES_128_GCM_SHA256 => "TLS_AES_128_GCM_SHA256(0x1301)".into(),
        TLS_AES_256_GCM_SHA384 => "TLS_AES_256_GCM_SHA384(0x1302)".into(),
        TLS_CHACHA20_POLY1305_SHA256 => "TLS_CHACHA20_POLY1305_SHA256(0x1303)".into(),
        0x1304 => "TLS_AES_128_CCM_SHA256(0x1304)".into(),
        0x1305 => "TLS_AES_128_CCM_8_SHA256(0x1305)".into(),
        other => format!("unknown(0x{other:04x})"),
    }
}

/// `NamedGroup` code points this client can do the maths for.
pub const GROUP_X25519: u16 = 0x001d;
/// `secp256r1`, the other group this client implements.
pub const GROUP_SECP256R1: u16 = 0x0017;
/// `secp384r1`, offered but never key-shared: used to make the server ask for a retry.
pub const GROUP_SECP384R1: u16 = 0x0018;
/// `ffdhe2048`, named only to prove unknown groups are skipped.
pub const GROUP_FFDHE2048: u16 = 0x0100;

/// The name of a named-group code point.
pub fn group_name(group: u16) -> String {
    match group {
        GROUP_SECP256R1 => "secp256r1(0x0017)".into(),
        GROUP_SECP384R1 => "secp384r1(0x0018)".into(),
        0x0019 => "secp521r1(0x0019)".into(),
        GROUP_X25519 => "x25519(0x001d)".into(),
        0x001e => "x448(0x001e)".into(),
        GROUP_FFDHE2048 => "ffdhe2048(0x0100)".into(),
        0x11ec => "X25519MLKEM768(0x11ec)".into(),
        other if is_grease(other) => format!("GREASE(0x{other:04x})"),
        other => format!("unknown(0x{other:04x})"),
    }
}

/// `rsa_pkcs1_sha256`
pub const SIG_RSA_PKCS1_SHA256: u16 = 0x0401;
/// `ecdsa_secp256r1_sha256`
pub const SIG_ECDSA_SECP256R1_SHA256: u16 = 0x0403;
/// `rsa_pss_rsae_sha256`
pub const SIG_RSA_PSS_RSAE_SHA256: u16 = 0x0804;
/// `rsa_pss_rsae_sha384`
pub const SIG_RSA_PSS_RSAE_SHA384: u16 = 0x0805;
/// `rsa_pss_rsae_sha512`
pub const SIG_RSA_PSS_RSAE_SHA512: u16 = 0x0806;
/// `ed25519`
pub const SIG_ED25519: u16 = 0x0807;

/// The name of a signature-scheme code point.
pub fn sig_name(scheme: u16) -> String {
    match scheme {
        0x0201 => "rsa_pkcs1_sha1(0x0201)".into(),
        SIG_RSA_PKCS1_SHA256 => "rsa_pkcs1_sha256(0x0401)".into(),
        0x0501 => "rsa_pkcs1_sha384(0x0501)".into(),
        0x0601 => "rsa_pkcs1_sha512(0x0601)".into(),
        SIG_ECDSA_SECP256R1_SHA256 => "ecdsa_secp256r1_sha256(0x0403)".into(),
        0x0503 => "ecdsa_secp384r1_sha384(0x0503)".into(),
        0x0603 => "ecdsa_secp521r1_sha512(0x0603)".into(),
        SIG_RSA_PSS_RSAE_SHA256 => "rsa_pss_rsae_sha256(0x0804)".into(),
        SIG_RSA_PSS_RSAE_SHA384 => "rsa_pss_rsae_sha384(0x0805)".into(),
        SIG_RSA_PSS_RSAE_SHA512 => "rsa_pss_rsae_sha512(0x0806)".into(),
        SIG_ED25519 => "ed25519(0x0807)".into(),
        0x0809 => "rsa_pss_pss_sha256(0x0809)".into(),
        other if is_grease(other) => format!("GREASE(0x{other:04x})"),
        other => format!("unknown(0x{other:04x})"),
    }
}

/// Extension type code points used by this suite (RFC 8446 §4.2).
pub const EXT_SERVER_NAME: u16 = 0;
/// `max_fragment_length(1)`
pub const EXT_MAX_FRAGMENT_LENGTH: u16 = 1;
/// `supported_groups(10)`
pub const EXT_SUPPORTED_GROUPS: u16 = 10;
/// `signature_algorithms(13)`
pub const EXT_SIGNATURE_ALGORITHMS: u16 = 13;
/// `application_layer_protocol_negotiation(16)`
pub const EXT_ALPN: u16 = 16;
/// `pre_shared_key(41)`
pub const EXT_PRE_SHARED_KEY: u16 = 41;
/// `early_data(42)`
pub const EXT_EARLY_DATA: u16 = 42;
/// `supported_versions(43)`
pub const EXT_SUPPORTED_VERSIONS: u16 = 43;
/// `cookie(44)`
pub const EXT_COOKIE: u16 = 44;
/// `psk_key_exchange_modes(45)`
pub const EXT_PSK_KEY_EXCHANGE_MODES: u16 = 45;
/// `key_share(51)`
pub const EXT_KEY_SHARE: u16 = 51;

/// The name of an extension code point.
pub fn ext_name(ext: u16) -> String {
    let n = match ext {
        0 => "server_name",
        1 => "max_fragment_length",
        5 => "status_request",
        10 => "supported_groups",
        13 => "signature_algorithms",
        14 => "use_srtp",
        15 => "heartbeat",
        16 => "application_layer_protocol_negotiation",
        18 => "signed_certificate_timestamp",
        19 => "client_certificate_type",
        20 => "server_certificate_type",
        21 => "padding",
        41 => "pre_shared_key",
        42 => "early_data",
        43 => "supported_versions",
        44 => "cookie",
        45 => "psk_key_exchange_modes",
        47 => "certificate_authorities",
        48 => "oid_filters",
        49 => "post_handshake_auth",
        50 => "signature_algorithms_cert",
        51 => "key_share",
        other if is_grease(other) => return format!("GREASE(0x{other:04x})"),
        other => return format!("unknown({other})"),
    };
    format!("{n}({ext})")
}

/// True for the sixteen GREASE code points of RFC 8701 (`0x?a?a` with both nibbles equal).
pub fn is_grease(value: u16) -> bool {
    value & 0x0f0f == 0x0a0a && (value >> 12) == ((value >> 4) & 0x0f)
}

/// The sixteen GREASE values, in order.
pub fn grease_values() -> Vec<u16> {
    (0..16u16).map(|i| (i << 12) | (i << 4) | 0x0a0a).collect()
}

/// `legacy_version` as it appears in a ClientHello and a ServerHello: always 0x0303.
pub const LEGACY_VERSION_TLS12: u16 = 0x0303;
/// The version a first-flight record may carry instead (RFC 8446 §5.1).
pub const LEGACY_VERSION_TLS10: u16 = 0x0301;
/// The real version, which lives in `supported_versions`.
pub const TLS13_VERSION: u16 = 0x0304;
/// TLS 1.2, for the "a 1.2-only client is refused" stage.
pub const TLS12_VERSION: u16 = 0x0303;

/// The largest `TLSPlaintext.length` RFC 8446 §5.1 allows: 2^14.
pub const MAX_PLAINTEXT: usize = 16_384;
/// The largest `TLSCiphertext.length`: 2^14 + 256.
pub const MAX_CIPHERTEXT: usize = 16_384 + 256;

/// The last eight bytes of `ServerHello.random` when a TLS 1.3 server negotiates TLS 1.2,
/// and the same for TLS 1.1 and below (RFC 8446 §4.1.3).
pub const DOWNGRADE_SENTINEL_TLS12: [u8; 8] = [0x44, 0x4F, 0x57, 0x4E, 0x47, 0x52, 0x44, 0x01];
/// The TLS 1.1-and-below downgrade sentinel.
pub const DOWNGRADE_SENTINEL_TLS11: [u8; 8] = [0x44, 0x4F, 0x57, 0x4E, 0x47, 0x52, 0x44, 0x00];
/// `ServerHello.random` of a HelloRetryRequest: SHA-256 of "HelloRetryRequest" (RFC 8446 §4.1.3).
pub const HELLO_RETRY_REQUEST_RANDOM: [u8; 32] = [
    0xCF, 0x21, 0xAD, 0x74, 0xE5, 0x9A, 0x61, 0x11, 0xBE, 0x1D, 0x8C, 0x02, 0x1E, 0x65, 0xB8, 0x91,
    0xC2, 0xA2, 0x11, 0x16, 0x7A, 0xBB, 0x8C, 0x5E, 0x07, 0x9E, 0x09, 0xE2, 0xC8, 0xA8, 0x33, 0x9C,
];

/// The context string a server signs under in CertificateVerify (RFC 8446 §4.4.3).
pub const SERVER_CV_CONTEXT: &str = "TLS 1.3, server CertificateVerify";
/// The context string a client signs under.
pub const CLIENT_CV_CONTEXT: &str = "TLS 1.3, client CertificateVerify";

// ---------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------

/// Everything that can go wrong while talking TLS to a server.
#[derive(Debug)]
pub enum TlsError {
    /// The peer closed the TCP connection before sending enough bytes.
    Closed,
    /// Nothing arrived in time; the string says what was being waited for.
    Timeout(String),
    /// A socket error.
    Io(std::io::Error),
    /// The record layer could not frame the bytes.
    Record(String),
    /// A handshake message did not parse; the string names the field path.
    Decode(String),
    /// The maths failed: a bad AEAD tag, a signature that does not verify, a bad Finished.
    Crypto(String),
    /// The server sent a well-formed alert.
    Alert(AlertLevel, AlertDescription),
    /// The server did something RFC 8446 forbids.
    Protocol(String),
}

impl fmt::Display for TlsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TlsError::Closed => write!(f, "the server closed the connection"),
            TlsError::Timeout(what) => write!(f, "timed out waiting for {what}"),
            TlsError::Io(e) => write!(f, "socket error: {e}"),
            TlsError::Record(m) => write!(f, "bad record: {m}"),
            TlsError::Decode(m) => write!(f, "cannot decode: {m}"),
            TlsError::Crypto(m) => write!(f, "crypto check failed: {m}"),
            TlsError::Alert(level, desc) => {
                write!(f, "the server sent alert {} {}", level.name(), desc.name())
            }
            TlsError::Protocol(m) => write!(f, "protocol violation: {m}"),
        }
    }
}

impl std::error::Error for TlsError {}

impl From<std::io::Error> for TlsError {
    fn from(e: std::io::Error) -> Self {
        TlsError::Io(e)
    }
}

/// Result of a TLS operation.
pub type TlsResult<T> = Result<T, TlsError>;

/// Lower-case hex with no separators.
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Undo [`hex`].
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// The first `n` bytes as hex, with an ellipsis when there are more.
pub fn hex_prefix(bytes: &[u8], n: usize) -> String {
    if bytes.len() <= n {
        hex(bytes)
    } else {
        format!("{}… ({} bytes)", hex(&bytes[..n]), bytes.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grease_values_are_recognised_and_nothing_else_is() {
        for v in grease_values() {
            assert!(is_grease(v), "0x{v:04x} must be GREASE");
        }
        assert_eq!(grease_values().len(), 16);
        assert_eq!(grease_values()[0], 0x0a0a);
        assert_eq!(grease_values()[15], 0xfafa);
        for v in [
            TLS_AES_128_GCM_SHA256,
            GROUP_X25519,
            GROUP_SECP256R1,
            EXT_KEY_SHARE,
            0x0a0b,
            0x1a0a,
        ] {
            assert!(!is_grease(v), "0x{v:04x} must not be GREASE");
        }
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(hex(&[0, 0x12, 0xff]), "0012ff");
        assert_eq!(unhex("0012ff"), Some(vec![0, 0x12, 0xff]));
        assert_eq!(unhex("00 12 ff"), Some(vec![0, 0x12, 0xff]));
        assert_eq!(unhex("abc"), None);
        assert_eq!(unhex("zz"), None);
    }

    #[test]
    fn code_points_have_names() {
        assert_eq!(suite_name(0x1301), "TLS_AES_128_GCM_SHA256(0x1301)");
        assert_eq!(group_name(0x001d), "x25519(0x001d)");
        assert_eq!(sig_name(0x0804), "rsa_pss_rsae_sha256(0x0804)");
        assert_eq!(ext_name(51), "key_share(51)");
        assert_eq!(
            AlertDescription::BAD_RECORD_MAC.name(),
            "bad_record_mac(20)"
        );
        assert_eq!(HandshakeType::FINISHED.name(), "finished(20)");
        assert_eq!(ContentType::from_u8(23).name(), "application_data(23)");
    }

    #[test]
    fn the_hello_retry_request_random_is_sha256_of_its_name() {
        use sha2::{Digest, Sha256};
        let want = Sha256::digest(b"HelloRetryRequest");
        assert_eq!(&HELLO_RETRY_REQUEST_RANDOM[..], &want[..]);
    }
}
