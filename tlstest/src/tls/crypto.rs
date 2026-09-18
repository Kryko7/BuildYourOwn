//! The TLS 1.3 key schedule (RFC 8446 §7.1), written one named label at a time.
//!
//! Everything a learner has to get right is visible here: the `HkdfLabel` structure fed to
//! `HKDF-Expand-Label`, the transcript hash each `Derive-Secret` is taken over, and the
//! `nonce = static_iv XOR sequence_number` rule of §5.3. Every derivation is recorded in
//! [`KeySchedule::steps`] so a failing test can print the exact label that was in force.
//!
//! `tests/rfc8448_vectors.rs` replays the published RFC 8448 trace through this module,
//! which is the strongest evidence the suite's own maths is right before it judges anybody
//! else's.

use super::{TlsError, TlsResult};
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes128Gcm, Aes256Gcm};
use chacha20poly1305::ChaCha20Poly1305;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256, Sha384};

// ---------------------------------------------------------------------------------------
// Hash agility
// ---------------------------------------------------------------------------------------

/// The hash a cipher suite is built on. TLS 1.3 only ever needs these two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashAlg {
    /// SHA-256, `Hash.length` 32.
    Sha256,
    /// SHA-384, `Hash.length` 48.
    Sha384,
}

impl HashAlg {
    /// `Hash.length` in bytes.
    pub fn len(self) -> usize {
        match self {
            HashAlg::Sha256 => 32,
            HashAlg::Sha384 => 48,
        }
    }

    /// Never true; present so `len()` does not trip the lint.
    pub fn is_empty(self) -> bool {
        false
    }

    /// The name used in messages.
    pub fn name(self) -> &'static str {
        match self {
            HashAlg::Sha256 => "SHA-256",
            HashAlg::Sha384 => "SHA-384",
        }
    }

    /// `Hash(data)`
    pub fn digest(self, data: &[u8]) -> Vec<u8> {
        match self {
            HashAlg::Sha256 => Sha256::digest(data).to_vec(),
            HashAlg::Sha384 => Sha384::digest(data).to_vec(),
        }
    }

    /// `HMAC-Hash(key, data)`
    pub fn hmac(self, key: &[u8], data: &[u8]) -> Vec<u8> {
        match self {
            HashAlg::Sha256 => {
                let mut m = <Hmac<Sha256> as Mac>::new_from_slice(key)
                    .unwrap_or_else(|_| <Hmac<Sha256> as Mac>::new_from_slice(&[]).unwrap_or_else(|_| unreachable!("HMAC-SHA256 accepts any key length")));
                m.update(data);
                m.finalize().into_bytes().to_vec()
            }
            HashAlg::Sha384 => {
                let mut m = <Hmac<Sha384> as Mac>::new_from_slice(key)
                    .unwrap_or_else(|_| <Hmac<Sha384> as Mac>::new_from_slice(&[]).unwrap_or_else(|_| unreachable!("HMAC-SHA384 accepts any key length")));
                m.update(data);
                m.finalize().into_bytes().to_vec()
            }
        }
    }

    /// `Hash("")`, the transcript hash of no messages at all.
    pub fn empty_hash(self) -> Vec<u8> {
        self.digest(&[])
    }

    /// A zero-filled secret of `Hash.length` bytes, the IKM of an unauthenticated handshake.
    pub fn zeros(self) -> Vec<u8> {
        vec![0u8; self.len()]
    }
}

// ---------------------------------------------------------------------------------------
// HKDF (RFC 5869) and HKDF-Expand-Label (RFC 8446 §7.1)
// ---------------------------------------------------------------------------------------

/// `HKDF-Extract(salt, IKM)` — one HMAC, keyed with the salt.
pub fn hkdf_extract(alg: HashAlg, salt: &[u8], ikm: &[u8]) -> Vec<u8> {
    alg.hmac(salt, ikm)
}

/// `HKDF-Expand(PRK, info, L)` — RFC 5869 §2.3.
pub fn hkdf_expand(alg: HashAlg, prk: &[u8], info: &[u8], length: usize) -> TlsResult<Vec<u8>> {
    let hash_len = alg.len();
    let n = length.div_ceil(hash_len);
    if n > 255 {
        return Err(TlsError::Crypto(format!(
            "HKDF-Expand asked for {length} bytes, more than 255 * {hash_len}"
        )));
    }
    let mut out = Vec::with_capacity(n * hash_len);
    let mut previous: Vec<u8> = Vec::new();
    for counter in 1..=n {
        let mut input = previous.clone();
        input.extend_from_slice(info);
        input.push(counter as u8);
        previous = alg.hmac(prk, &input);
        out.extend_from_slice(&previous);
    }
    out.truncate(length);
    Ok(out)
}

/// The `HkdfLabel` structure of RFC 8446 §7.1, as it goes on the wire inside `info`:
///
/// ```text
/// struct {
///     uint16 length = Length;
///     opaque label<7..255> = "tls13 " + Label;
///     opaque context<0..255> = Context;
/// } HkdfLabel;
/// ```
///
/// The `"tls13 "` prefix (six bytes, with the space) is the single most common thing to get
/// wrong, which is why it lives in its own function.
pub fn hkdf_label(label: &str, context: &[u8], length: usize) -> Vec<u8> {
    let mut full = Vec::with_capacity(6 + label.len());
    full.extend_from_slice(b"tls13 ");
    full.extend_from_slice(label.as_bytes());
    let mut out = Vec::with_capacity(2 + 1 + full.len() + 1 + context.len());
    out.extend_from_slice(&(length as u16).to_be_bytes());
    out.push(full.len() as u8);
    out.extend_from_slice(&full);
    out.push(context.len() as u8);
    out.extend_from_slice(context);
    out
}

/// `HKDF-Expand-Label(Secret, Label, Context, Length)`.
pub fn hkdf_expand_label(
    alg: HashAlg,
    secret: &[u8],
    label: &str,
    context: &[u8],
    length: usize,
) -> TlsResult<Vec<u8>> {
    hkdf_expand(alg, secret, &hkdf_label(label, context, length), length)
}

/// `Derive-Secret(Secret, Label, Messages)` — `HKDF-Expand-Label` over the transcript hash.
pub fn derive_secret(
    alg: HashAlg,
    secret: &[u8],
    label: &str,
    transcript_hash: &[u8],
) -> TlsResult<Vec<u8>> {
    hkdf_expand_label(alg, secret, label, transcript_hash, alg.len())
}

// ---------------------------------------------------------------------------------------
// AEAD
// ---------------------------------------------------------------------------------------

/// The AEAD a cipher suite protects records with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AeadAlg {
    /// `AEAD_AES_128_GCM`
    Aes128Gcm,
    /// `AEAD_AES_256_GCM`
    Aes256Gcm,
    /// `AEAD_CHACHA20_POLY1305`
    ChaCha20Poly1305,
}

impl AeadAlg {
    /// `key_length` in bytes.
    pub fn key_len(self) -> usize {
        match self {
            AeadAlg::Aes128Gcm => 16,
            AeadAlg::Aes256Gcm | AeadAlg::ChaCha20Poly1305 => 32,
        }
    }

    /// `iv_length` in bytes. Every TLS 1.3 AEAD uses a 12-byte nonce.
    pub fn iv_len(self) -> usize {
        12
    }

    /// The authentication tag length in bytes.
    pub fn tag_len(self) -> usize {
        16
    }

    /// The name used in messages.
    pub fn name(self) -> &'static str {
        match self {
            AeadAlg::Aes128Gcm => "AES-128-GCM",
            AeadAlg::Aes256Gcm => "AES-256-GCM",
            AeadAlg::ChaCha20Poly1305 => "ChaCha20-Poly1305",
        }
    }

    /// Encrypt, returning ciphertext with the tag appended.
    pub fn seal(self, key: &[u8], nonce: &[u8], aad: &[u8], plaintext: &[u8]) -> TlsResult<Vec<u8>> {
        let payload = Payload {
            msg: plaintext,
            aad,
        };
        let out = match self {
            AeadAlg::Aes128Gcm => Aes128Gcm::new_from_slice(key)
                .map_err(|_| TlsError::Crypto("AES-128-GCM needs a 16-byte key".into()))?
                .encrypt(nonce.into(), payload),
            AeadAlg::Aes256Gcm => Aes256Gcm::new_from_slice(key)
                .map_err(|_| TlsError::Crypto("AES-256-GCM needs a 32-byte key".into()))?
                .encrypt(nonce.into(), payload),
            AeadAlg::ChaCha20Poly1305 => ChaCha20Poly1305::new_from_slice(key)
                .map_err(|_| TlsError::Crypto("ChaCha20-Poly1305 needs a 32-byte key".into()))?
                .encrypt(nonce.into(), payload),
        };
        out.map_err(|_| TlsError::Crypto(format!("{} sealing failed", self.name())))
    }

    /// Decrypt, checking the tag.
    pub fn open(
        self,
        key: &[u8],
        nonce: &[u8],
        aad: &[u8],
        ciphertext: &[u8],
    ) -> TlsResult<Vec<u8>> {
        let payload = Payload {
            msg: ciphertext,
            aad,
        };
        let out = match self {
            AeadAlg::Aes128Gcm => Aes128Gcm::new_from_slice(key)
                .map_err(|_| TlsError::Crypto("AES-128-GCM needs a 16-byte key".into()))?
                .decrypt(nonce.into(), payload),
            AeadAlg::Aes256Gcm => Aes256Gcm::new_from_slice(key)
                .map_err(|_| TlsError::Crypto("AES-256-GCM needs a 32-byte key".into()))?
                .decrypt(nonce.into(), payload),
            AeadAlg::ChaCha20Poly1305 => ChaCha20Poly1305::new_from_slice(key)
                .map_err(|_| TlsError::Crypto("ChaCha20-Poly1305 needs a 32-byte key".into()))?
                .decrypt(nonce.into(), payload),
        };
        out.map_err(|_| {
            TlsError::Crypto(format!(
                "{} could not authenticate the record (bad tag)",
                self.name()
            ))
        })
    }
}

// ---------------------------------------------------------------------------------------
// Cipher suites
// ---------------------------------------------------------------------------------------

/// A TLS 1.3 cipher suite: a hash and an AEAD, nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Suite {
    /// The code point, e.g. 0x1301.
    pub code: u16,
    /// The hash the key schedule and the transcript use.
    pub hash: HashAlg,
    /// The AEAD records are protected with.
    pub aead: AeadAlg,
}

impl Suite {
    /// The suite for a code point, or `None` when this client cannot speak it.
    pub fn from_code(code: u16) -> Option<Suite> {
        match code {
            super::TLS_AES_128_GCM_SHA256 => Some(Suite {
                code,
                hash: HashAlg::Sha256,
                aead: AeadAlg::Aes128Gcm,
            }),
            super::TLS_AES_256_GCM_SHA384 => Some(Suite {
                code,
                hash: HashAlg::Sha384,
                aead: AeadAlg::Aes256Gcm,
            }),
            super::TLS_CHACHA20_POLY1305_SHA256 => Some(Suite {
                code,
                hash: HashAlg::Sha256,
                aead: AeadAlg::ChaCha20Poly1305,
            }),
            _ => None,
        }
    }

    /// The suite's name.
    pub fn name(self) -> String {
        super::suite_name(self.code)
    }
}

// ---------------------------------------------------------------------------------------
// Traffic keys
// ---------------------------------------------------------------------------------------

/// A `[sender]_write_key` / `[sender]_write_iv` pair and the secret they came from.
#[derive(Debug, Clone)]
pub struct TrafficKeys {
    /// The traffic secret these were expanded from.
    pub secret: Vec<u8>,
    /// `HKDF-Expand-Label(secret, "key", "", key_length)`
    pub key: Vec<u8>,
    /// `HKDF-Expand-Label(secret, "iv", "", iv_length)`
    pub iv: Vec<u8>,
    /// The AEAD they belong to.
    pub aead: AeadAlg,
    /// The hash of the suite, needed to update the secret.
    pub hash: HashAlg,
}

impl TrafficKeys {
    /// Expand `key` and `iv` from a traffic secret (RFC 8446 §7.3).
    pub fn derive(suite: Suite, secret: &[u8]) -> TlsResult<TrafficKeys> {
        Ok(TrafficKeys {
            key: hkdf_expand_label(suite.hash, secret, "key", &[], suite.aead.key_len())?,
            iv: hkdf_expand_label(suite.hash, secret, "iv", &[], suite.aead.iv_len())?,
            secret: secret.to_vec(),
            aead: suite.aead,
            hash: suite.hash,
        })
    }

    /// The per-record nonce: the static IV with the sequence number XORed into its right
    /// end (RFC 8446 §5.3). The sequence number is never sent; both sides just count.
    pub fn nonce(&self, sequence: u64) -> Vec<u8> {
        let mut nonce = self.iv.clone();
        let seq = sequence.to_be_bytes();
        let start = nonce.len().saturating_sub(8);
        for (i, b) in seq.iter().enumerate() {
            nonce[start + i] ^= b;
        }
        nonce
    }

    /// `application_traffic_secret_N+1 = HKDF-Expand-Label(secret, "traffic upd", "", Hash.length)`
    pub fn updated(&self) -> TlsResult<TrafficKeys> {
        let next = hkdf_expand_label(self.hash, &self.secret, "traffic upd", &[], self.hash.len())?;
        let suite = Suite {
            code: 0,
            hash: self.hash,
            aead: self.aead,
        };
        TrafficKeys::derive(suite, &next)
    }

    /// `finished_key = HKDF-Expand-Label(secret, "finished", "", Hash.length)`
    pub fn finished_key(&self) -> TlsResult<Vec<u8>> {
        hkdf_expand_label(self.hash, &self.secret, "finished", &[], self.hash.len())
    }
}

// ---------------------------------------------------------------------------------------
// The transcript
// ---------------------------------------------------------------------------------------

/// The handshake transcript: every handshake message, in order, exactly as it went on the
/// wire (four-byte header included) and never including record headers.
#[derive(Debug, Clone)]
pub struct Transcript {
    hash: HashAlg,
    bytes: Vec<u8>,
    /// `(after this message type, the hash at that point)`, for the report.
    pub checkpoints: Vec<(String, Vec<u8>)>,
}

impl Transcript {
    /// A fresh, empty transcript.
    pub fn new(hash: HashAlg) -> Transcript {
        Transcript {
            hash,
            bytes: Vec::new(),
            checkpoints: Vec::new(),
        }
    }

    /// Re-hash the accumulated messages under a different hash, which is what happens when
    /// the server's chosen cipher suite turns out to use SHA-384.
    pub fn rehash(&mut self, hash: HashAlg) {
        self.hash = hash;
        self.checkpoints.clear();
    }

    /// The hash this transcript is taken under.
    pub fn hash_alg(&self) -> HashAlg {
        self.hash
    }

    /// Append one complete handshake message (`msg_type` + `length` + body).
    pub fn push(&mut self, message: &[u8], label: impl Into<String>) {
        self.bytes.extend_from_slice(message);
        let h = self.current();
        self.checkpoints.push((label.into(), h));
    }

    /// Replace the transcript with the `message_hash` synthetic message of RFC 8446 §4.4.1,
    /// which is what a HelloRetryRequest does to `ClientHello1`:
    ///
    /// ```text
    /// Transcript-Hash(ClientHello1, HelloRetryRequest, ... Mn) =
    ///     Hash(message_hash || 00 00 Hash.length || Hash(ClientHello1) || HelloRetryRequest || ... || Mn)
    /// ```
    pub fn replace_with_message_hash(&mut self) {
        let digest = self.hash.digest(&self.bytes);
        let mut synthetic = vec![254u8, 0, 0, self.hash.len() as u8];
        synthetic.extend_from_slice(&digest);
        self.bytes = synthetic;
        self.checkpoints
            .push(("message_hash (HelloRetryRequest)".into(), self.current()));
    }

    /// `Transcript-Hash(...)` over everything pushed so far.
    pub fn current(&self) -> Vec<u8> {
        self.hash.digest(&self.bytes)
    }

    /// The raw concatenated messages.
    pub fn raw(&self) -> &[u8] {
        &self.bytes
    }

    /// The hash recorded right after the named message, if it is in the transcript.
    pub fn at(&self, label: &str) -> Option<&[u8]> {
        self.checkpoints
            .iter()
            .find(|(l, _)| l == label)
            .map(|(_, h)| h.as_slice())
    }
}

// ---------------------------------------------------------------------------------------
// The key schedule
// ---------------------------------------------------------------------------------------

/// One derivation, recorded so a failure can name the label that was in force.
#[derive(Debug, Clone)]
pub struct Step {
    /// `HKDF-Extract` or the `Derive-Secret` / `HKDF-Expand-Label` label.
    pub label: String,
    /// What the step produced, named the way RFC 8446 §7.1 names it.
    pub output: String,
    /// The transcript hash the step was taken over, when it took one.
    pub transcript_hash: Option<Vec<u8>>,
    /// The bytes produced.
    pub secret: Vec<u8>,
}

/// The whole key schedule of one connection, step by step.
#[derive(Debug, Clone)]
pub struct KeySchedule {
    /// The suite whose hash and AEAD everything here uses.
    pub suite: Suite,
    /// Every derivation in order.
    pub steps: Vec<Step>,
    /// `Early Secret`
    pub early_secret: Vec<u8>,
    /// `Handshake Secret`
    pub handshake_secret: Vec<u8>,
    /// `Master Secret`
    pub master_secret: Vec<u8>,
    /// `client_handshake_traffic_secret`
    pub client_handshake: Option<TrafficKeys>,
    /// `server_handshake_traffic_secret`
    pub server_handshake: Option<TrafficKeys>,
    /// `client_application_traffic_secret_0`
    pub client_application: Option<TrafficKeys>,
    /// `server_application_traffic_secret_0`
    pub server_application: Option<TrafficKeys>,
    /// `resumption_master_secret`
    pub resumption_master: Option<Vec<u8>>,
    /// `exporter_master_secret`
    pub exporter_master: Option<Vec<u8>>,
    /// `binder_key`, when the handshake offered a PSK.
    pub binder_key: Option<Vec<u8>>,
}

impl KeySchedule {
    /// Start the schedule: `Early Secret = HKDF-Extract(0, PSK or 0)`.
    pub fn new(suite: Suite, psk: Option<&[u8]>) -> KeySchedule {
        let hash = suite.hash;
        let ikm = psk.map(<[u8]>::to_vec).unwrap_or_else(|| hash.zeros());
        let early_secret = hkdf_extract(hash, &hash.zeros(), &ikm);
        let mut ks = KeySchedule {
            suite,
            steps: Vec::new(),
            early_secret: early_secret.clone(),
            handshake_secret: Vec::new(),
            master_secret: Vec::new(),
            client_handshake: None,
            server_handshake: None,
            client_application: None,
            server_application: None,
            resumption_master: None,
            exporter_master: None,
            binder_key: None,
        };
        ks.record("HKDF-Extract(0, PSK)", "Early Secret", None, &early_secret);
        ks
    }

    fn record(&mut self, label: &str, output: &str, transcript: Option<&[u8]>, secret: &[u8]) {
        self.steps.push(Step {
            label: label.to_string(),
            output: output.to_string(),
            transcript_hash: transcript.map(<[u8]>::to_vec),
            secret: secret.to_vec(),
        });
    }

    /// `binder_key = Derive-Secret(Early Secret, "res binder", "")` for a resumption PSK.
    pub fn derive_binder_key(&mut self) -> TlsResult<Vec<u8>> {
        let hash = self.suite.hash;
        let empty = hash.empty_hash();
        let key = derive_secret(hash, &self.early_secret, "res binder", &empty)?;
        self.record("res binder", "binder_key", Some(&empty), &key);
        self.binder_key = Some(key.clone());
        Ok(key)
    }

    /// `client_early_traffic_secret = Derive-Secret(Early Secret, "c e traffic", ClientHello)`.
    pub fn derive_early_traffic(&mut self, transcript_hash: &[u8]) -> TlsResult<TrafficKeys> {
        let hash = self.suite.hash;
        let secret = derive_secret(hash, &self.early_secret, "c e traffic", transcript_hash)?;
        self.record(
            "c e traffic",
            "client_early_traffic_secret",
            Some(transcript_hash),
            &secret,
        );
        TrafficKeys::derive(self.suite, &secret)
    }

    /// `Handshake Secret = HKDF-Extract(Derive-Secret(Early Secret, "derived", ""), ECDHE)`
    /// and the two handshake traffic secrets over `ClientHello..ServerHello`.
    pub fn enter_handshake(&mut self, shared_secret: &[u8], transcript_hash: &[u8]) -> TlsResult<()> {
        let hash = self.suite.hash;
        let empty = hash.empty_hash();
        let derived = derive_secret(hash, &self.early_secret, "derived", &empty)?;
        self.record("derived", "Derived Secret", Some(&empty), &derived);

        let handshake_secret = hkdf_extract(hash, &derived, shared_secret);
        self.record(
            "HKDF-Extract(Derived, (EC)DHE)",
            "Handshake Secret",
            None,
            &handshake_secret,
        );
        self.handshake_secret = handshake_secret.clone();

        let c = derive_secret(hash, &handshake_secret, "c hs traffic", transcript_hash)?;
        self.record(
            "c hs traffic",
            "client_handshake_traffic_secret",
            Some(transcript_hash),
            &c,
        );
        let s = derive_secret(hash, &handshake_secret, "s hs traffic", transcript_hash)?;
        self.record(
            "s hs traffic",
            "server_handshake_traffic_secret",
            Some(transcript_hash),
            &s,
        );
        self.client_handshake = Some(TrafficKeys::derive(self.suite, &c)?);
        self.server_handshake = Some(TrafficKeys::derive(self.suite, &s)?);

        let derived2 = derive_secret(hash, &handshake_secret, "derived", &empty)?;
        self.record("derived", "Derived Secret", Some(&empty), &derived2);
        let master = hkdf_extract(hash, &derived2, &hash.zeros());
        self.record("HKDF-Extract(Derived, 0)", "Master Secret", None, &master);
        self.master_secret = master;
        Ok(())
    }

    /// The two application traffic secrets and the exporter secret, over
    /// `ClientHello..server Finished`.
    pub fn enter_application(&mut self, transcript_hash: &[u8]) -> TlsResult<()> {
        let hash = self.suite.hash;
        let master = self.master_secret.clone();
        let c = derive_secret(hash, &master, "c ap traffic", transcript_hash)?;
        self.record(
            "c ap traffic",
            "client_application_traffic_secret_0",
            Some(transcript_hash),
            &c,
        );
        let s = derive_secret(hash, &master, "s ap traffic", transcript_hash)?;
        self.record(
            "s ap traffic",
            "server_application_traffic_secret_0",
            Some(transcript_hash),
            &s,
        );
        let e = derive_secret(hash, &master, "exp master", transcript_hash)?;
        self.record(
            "exp master",
            "exporter_master_secret",
            Some(transcript_hash),
            &e,
        );
        self.client_application = Some(TrafficKeys::derive(self.suite, &c)?);
        self.server_application = Some(TrafficKeys::derive(self.suite, &s)?);
        self.exporter_master = Some(e);
        Ok(())
    }

    /// `resumption_master_secret = Derive-Secret(Master Secret, "res master",
    /// ClientHello..client Finished)`.
    pub fn derive_resumption_master(&mut self, transcript_hash: &[u8]) -> TlsResult<Vec<u8>> {
        let hash = self.suite.hash;
        let master = self.master_secret.clone();
        let r = derive_secret(hash, &master, "res master", transcript_hash)?;
        self.record(
            "res master",
            "resumption_master_secret",
            Some(transcript_hash),
            &r,
        );
        self.resumption_master = Some(r.clone());
        Ok(r)
    }

    /// The PSK a ticket stands for:
    /// `PSK = HKDF-Expand-Label(resumption_master_secret, "resumption", ticket_nonce, Hash.length)`.
    pub fn resumption_psk(&self, ticket_nonce: &[u8]) -> TlsResult<Vec<u8>> {
        let rms = self
            .resumption_master
            .as_ref()
            .ok_or_else(|| TlsError::Crypto("no resumption_master_secret has been derived".into()))?;
        hkdf_expand_label(
            self.suite.hash,
            rms,
            "resumption",
            ticket_nonce,
            self.suite.hash.len(),
        )
    }

    /// `verify_data = HMAC(finished_key, Transcript-Hash(...))` (RFC 8446 §4.4.4).
    pub fn verify_data(&self, keys: &TrafficKeys, transcript_hash: &[u8]) -> TlsResult<Vec<u8>> {
        let fk = keys.finished_key()?;
        Ok(self.suite.hash.hmac(&fk, transcript_hash))
    }

    /// The label that was in force at the last recorded step, for a failure message.
    pub fn last_label(&self) -> String {
        self.steps
            .last()
            .map(|s| format!("{} → {}", s.label, s.output))
            .unwrap_or_else(|| "nothing derived yet".to_string())
    }

    /// The whole schedule as report lines.
    pub fn report_lines(&self) -> Vec<String> {
        self.steps
            .iter()
            .map(|s| {
                let over = match &s.transcript_hash {
                    Some(h) => format!(" over transcript {}", super::hex_prefix(h, 8)),
                    None => String::new(),
                };
                format!(
                    "{:<32} → {:<38} {}{over}",
                    s.label,
                    s.output,
                    super::hex_prefix(&s.secret, 8)
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hkdf_label_has_the_tls13_prefix_and_two_length_bytes() {
        let info = hkdf_label("key", &[], 16);
        assert_eq!(&info[0..2], &16u16.to_be_bytes(), "the output length first");
        assert_eq!(info[2], 9, "'tls13 key' is nine bytes");
        assert_eq!(&info[3..12], b"tls13 key");
        assert_eq!(info[12], 0, "an empty context is a zero length byte");
        assert_eq!(info.len(), 13);
    }

    #[test]
    fn hkdf_matches_rfc5869_test_case_1() {
        // RFC 5869 Appendix A.1, SHA-256.
        let ikm = vec![0x0bu8; 22];
        let salt: Vec<u8> = (0..13).collect();
        let info: Vec<u8> = (0xf0..0xfa).collect();
        let prk = hkdf_extract(HashAlg::Sha256, &salt, &ikm);
        assert_eq!(
            super::super::hex(&prk),
            "077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5"
        );
        let okm = hkdf_expand(HashAlg::Sha256, &prk, &info, 42).expect("expand");
        assert_eq!(
            super::super::hex(&okm),
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
        );
    }

    #[test]
    fn the_nonce_is_the_iv_xor_the_sequence_number() {
        let keys = TrafficKeys {
            secret: vec![0; 32],
            key: vec![0; 16],
            iv: vec![0xaa; 12],
            aead: AeadAlg::Aes128Gcm,
            hash: HashAlg::Sha256,
        };
        assert_eq!(keys.nonce(0), vec![0xaa; 12], "sequence 0 leaves the IV alone");
        let n1 = keys.nonce(1);
        assert_eq!(&n1[..11], &[0xaa; 11], "only the last eight bytes change");
        assert_eq!(n1[11], 0xab);
        let n = keys.nonce(0x0102_0304_0506_0708);
        assert_eq!(
            n,
            vec![0xaa, 0xaa, 0xaa, 0xaa, 0xab, 0xa8, 0xa9, 0xae, 0xaf, 0xac, 0xad, 0xa2]
        );
    }

    #[test]
    fn every_aead_round_trips_and_rejects_a_tampered_tag() {
        for aead in [
            AeadAlg::Aes128Gcm,
            AeadAlg::Aes256Gcm,
            AeadAlg::ChaCha20Poly1305,
        ] {
            let key = vec![7u8; aead.key_len()];
            let nonce = vec![9u8; aead.iv_len()];
            let aad = b"\x17\x03\x03\x00\x20";
            let sealed = aead.seal(&key, &nonce, aad, b"hello").expect("seal");
            assert_eq!(sealed.len(), 5 + aead.tag_len());
            let opened = aead.open(&key, &nonce, aad, &sealed).expect("open");
            assert_eq!(opened, b"hello");
            let mut broken = sealed.clone();
            let last = broken.len() - 1;
            broken[last] ^= 1;
            assert!(
                aead.open(&key, &nonce, aad, &broken).is_err(),
                "{} accepted a flipped tag bit",
                aead.name()
            );
            assert!(
                aead.open(&key, &nonce, b"different aad", &sealed).is_err(),
                "{} ignored the additional data",
                aead.name()
            );
        }
    }

    #[test]
    fn suites_map_to_the_right_hash_and_aead() {
        let s = Suite::from_code(super::super::TLS_AES_256_GCM_SHA384).expect("suite");
        assert_eq!(s.hash, HashAlg::Sha384);
        assert_eq!(s.aead, AeadAlg::Aes256Gcm);
        assert_eq!(s.hash.len(), 48);
        let s = Suite::from_code(super::super::TLS_CHACHA20_POLY1305_SHA256).expect("suite");
        assert_eq!(s.hash, HashAlg::Sha256);
        assert_eq!(s.aead, AeadAlg::ChaCha20Poly1305);
        assert!(Suite::from_code(0x1304).is_none(), "CCM is not implemented");
    }

    #[test]
    fn the_transcript_hash_of_nothing_is_the_hash_of_the_empty_string() {
        let t = Transcript::new(HashAlg::Sha256);
        assert_eq!(
            super::super::hex(&t.current()),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn the_message_hash_replacement_follows_section_4_4_1() {
        let mut t = Transcript::new(HashAlg::Sha256);
        t.push(&[1, 0, 0, 1, 0xaa], "client_hello");
        let ch_hash = HashAlg::Sha256.digest(&[1, 0, 0, 1, 0xaa]);
        t.replace_with_message_hash();
        let mut want = vec![254u8, 0, 0, 32];
        want.extend_from_slice(&ch_hash);
        assert_eq!(t.raw(), &want[..]);
    }
}
