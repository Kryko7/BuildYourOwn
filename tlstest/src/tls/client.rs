//! Driving a whole TLS 1.3 handshake, one flight at a time.
//!
//! [`Client`] keeps every intermediate value a test might want to assert on — the
//! ClientHello it built, the ServerHello it parsed, the transcript hash at each message
//! boundary, the key schedule with its labels, the certificate chain, the signature and
//! both Finished messages — and it keeps them *after a failure too*, which is why the
//! driver is a struct with steps rather than one function returning a result.
//!
//! A stage that only cares about the ServerHello calls [`Client::send_client_hello`] and
//! [`Client::read_server_hello`]; a stage that wants an echo calls [`Client::handshake`]
//! and then [`Client::echo_line`].

use super::buf::Writer;
use crate::certs::ClientAuth;

use super::conn::{Incoming, TlsConn};
use super::crypto::{KeySchedule, Suite, TrafficKeys, Transcript};
use super::msg::{
    alpn_extension, client_key_share_extension, cookie_extension, default_signature_algorithms,
    encode_handshake, find_extension, parse_extensions, psk_key_exchange_modes_extension,
    server_name_extension, signature_algorithms_extension, supported_groups_extension,
    supported_versions_extension, Alert, CertificateMsg, CertificateRequestMsg,
    CertificateVerifyMsg, ClientHello, Extension, KeyExchange, KeyUpdate, NewSessionTicket,
    ServerHello,
};
use super::sig::{self, PublicKey};
use super::{
    group_name, hex, suite_name, AlertDescription, ContentType, HandshakeType, TlsError, TlsResult,
    ALL_SUITES, EXT_ALPN, EXT_KEY_SHARE, EXT_PRE_SHARED_KEY, EXT_SUPPORTED_VERSIONS,
    GROUP_SECP256R1, GROUP_X25519, LEGACY_VERSION_TLS12, TLS13_VERSION,
};
use std::net::SocketAddr;
use std::time::Duration;

/// A resumption PSK the client is offering.
#[derive(Debug, Clone)]
pub struct PskOffer {
    /// The ticket, which becomes the PSK identity.
    pub identity: Vec<u8>,
    /// `obfuscated_ticket_age` as it goes on the wire.
    pub obfuscated_ticket_age: u32,
    /// The PSK itself, derived from the previous connection's resumption master secret.
    pub psk: Vec<u8>,
    /// The suite the PSK was established under; its hash is the binder's hash.
    pub suite: Suite,
}

/// Everything about the ClientHello this client will send.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// `legacy_version`; RFC 8446 pins it at 0x0303.
    pub legacy_version: u16,
    /// `cipher_suites`, most preferred first.
    pub cipher_suites: Vec<u16>,
    /// `supported_groups`.
    pub groups: Vec<u16>,
    /// The groups that actually get a `KeyShareEntry`; a subset of `groups`.
    pub share_groups: Vec<u16>,
    /// `signature_algorithms`.
    pub signature_algorithms: Vec<u16>,
    /// The contents of `supported_versions`.
    pub versions: Vec<u16>,
    /// `legacy_session_id`; 32 random bytes is what a compatibility-mode client sends.
    pub session_id: Vec<u8>,
    /// `server_name`, when one should be sent.
    pub server_name: Option<String>,
    /// ALPN protocols, when the extension should be sent at all.
    pub alpn: Vec<String>,
    /// Extensions appended after the standard ones, verbatim.
    pub extra_extensions: Vec<Extension>,
    /// Send the `ChangeCipherSpec` compatibility record after the ClientHello.
    pub send_ccs: bool,
    /// Follow a HelloRetryRequest with a second ClientHello.
    pub follow_hello_retry: bool,
    /// The 32 bytes the `random` and the key shares are derived from.
    pub entropy: [u8; 32],
    /// A resumption PSK to offer.
    pub psk: Option<PskOffer>,
    /// Offer `early_data` alongside the PSK.
    pub offer_early_data: bool,
    /// Flip a bit of the PSK binder before sending it, so a test can prove the server
    /// really checks it.
    pub corrupt_psk_binder: bool,
}

impl Default for ClientConfig {
    fn default() -> ClientConfig {
        ClientConfig {
            legacy_version: LEGACY_VERSION_TLS12,
            cipher_suites: ALL_SUITES.to_vec(),
            groups: vec![GROUP_X25519, GROUP_SECP256R1],
            share_groups: vec![GROUP_X25519],
            signature_algorithms: default_signature_algorithms(),
            versions: vec![TLS13_VERSION],
            session_id: vec![0x5a; 32],
            server_name: Some("localhost".to_string()),
            alpn: Vec::new(),
            extra_extensions: Vec::new(),
            send_ccs: true,
            follow_hello_retry: true,
            entropy: [0x2c; 32],
            psk: None,
            offer_early_data: false,
            corrupt_psk_binder: false,
        }
    }
}

impl ClientConfig {
    /// A configuration whose randomness comes from `seed`, so a run is reproducible.
    pub fn seeded(seed: u64) -> ClientConfig {
        let material = super::crypto::HashAlg::Sha256.digest(&seed.to_be_bytes());
        let mut entropy = [0u8; 32];
        entropy.copy_from_slice(&material);
        let mut session_id = super::crypto::HashAlg::Sha256
            .digest(b"session-id")
            .to_vec();
        for (i, b) in session_id.iter_mut().enumerate() {
            *b ^= entropy[i % 32];
        }
        ClientConfig {
            entropy,
            session_id,
            ..ClientConfig::default()
        }
    }

    /// Offer exactly these cipher suites.
    pub fn with_suites(mut self, suites: &[u16]) -> ClientConfig {
        self.cipher_suites = suites.to_vec();
        self
    }

    /// Offer these groups and send a key share for each of them.
    pub fn with_groups(mut self, groups: &[u16]) -> ClientConfig {
        self.groups = groups.to_vec();
        self.share_groups = groups.to_vec();
        self
    }

    /// Offer these groups but send key shares only for `shares`.
    pub fn with_group_shares(mut self, groups: &[u16], shares: &[u16]) -> ClientConfig {
        self.groups = groups.to_vec();
        self.share_groups = shares.to_vec();
        self
    }

    /// Offer exactly these signature schemes.
    pub fn with_signature_algorithms(mut self, schemes: &[u16]) -> ClientConfig {
        self.signature_algorithms = schemes.to_vec();
        self
    }

    /// Offer these ALPN protocols.
    pub fn with_alpn(mut self, protocols: &[&str]) -> ClientConfig {
        self.alpn = protocols.iter().map(|p| (*p).to_string()).collect();
        self
    }

    /// Send this `server_name`, or none at all.
    pub fn with_server_name(mut self, name: Option<&str>) -> ClientConfig {
        self.server_name = name.map(str::to_string);
        self
    }

    /// Send this `legacy_session_id`.
    pub fn with_session_id(mut self, id: Vec<u8>) -> ClientConfig {
        self.session_id = id;
        self
    }

    /// Append an extension after the standard ones.
    pub fn with_extension(mut self, extension: Extension) -> ClientConfig {
        self.extra_extensions.push(extension);
        self
    }

    /// Send, or do not send, the `ChangeCipherSpec` compatibility record.
    pub fn with_ccs(mut self, send: bool) -> ClientConfig {
        self.send_ccs = send;
        self
    }

    /// Offer this resumption PSK.
    pub fn with_psk(mut self, psk: PskOffer) -> ClientConfig {
        self.psk = Some(psk);
        self
    }

    /// Offer this resumption PSK with a deliberately wrong binder.
    pub fn with_broken_psk(mut self, psk: PskOffer) -> ClientConfig {
        self.psk = Some(psk);
        self.corrupt_psk_binder = true;
        self
    }

    /// 32 bytes of entropy for the `n`th key share of this hello.
    pub fn share_entropy(&self, n: usize) -> [u8; 32] {
        let mut input = self.entropy.to_vec();
        input.push(n as u8);
        input.extend_from_slice(b"key share");
        let digest = super::crypto::HashAlg::Sha256.digest(&input);
        let mut out = [0u8; 32];
        out.copy_from_slice(&digest);
        out
    }

    /// The 32-byte `random` this configuration sends.
    pub fn random(&self) -> [u8; 32] {
        let digest =
            super::crypto::HashAlg::Sha256.digest(&[self.entropy.as_slice(), b"random"].concat());
        let mut out = [0u8; 32];
        out.copy_from_slice(&digest);
        out
    }
}

/// A TLS 1.3 client driving one connection.
pub struct Client {
    /// The connection, with its record layer and its trace.
    pub conn: TlsConn,
    /// What the ClientHello was built from.
    pub config: ClientConfig,
    /// The key shares the client generated, in the order they were offered.
    pub key_shares: Vec<KeyExchange>,
    /// The ClientHello, parsed back from the bytes that were sent.
    pub client_hello: Option<ClientHello>,
    /// The ClientHello handshake message, header included.
    pub client_hello_bytes: Vec<u8>,
    /// A HelloRetryRequest, when one arrived.
    pub hello_retry_request: Option<ServerHello>,
    /// The ServerHello.
    pub server_hello: Option<ServerHello>,
    /// The ServerHello handshake message, header included.
    pub server_hello_bytes: Vec<u8>,
    /// The negotiated suite.
    pub suite: Option<Suite>,
    /// The group the (EC)DHE was done over.
    pub selected_group: Option<u16>,
    /// The (EC)DHE shared secret.
    pub shared_secret: Vec<u8>,
    /// The key schedule, with every derivation recorded.
    pub schedule: Option<KeySchedule>,
    /// The transcript.
    pub transcript: Option<Transcript>,
    /// EncryptedExtensions, as a list.
    pub encrypted_extensions: Vec<Extension>,
    /// The EncryptedExtensions message bytes.
    pub encrypted_extensions_bytes: Vec<u8>,
    /// The Certificate message.
    pub certificate: Option<CertificateMsg>,
    /// The Certificate message bytes.
    pub certificate_bytes: Vec<u8>,
    /// The CertificateVerify message.
    pub certificate_verify: Option<CertificateVerifyMsg>,
    /// The CertificateVerify message bytes.
    pub certificate_verify_bytes: Vec<u8>,
    /// A CertificateRequest, if the server asked for a client certificate.
    pub certificate_request: Option<Vec<u8>>,
    /// The same message, parsed.
    pub certificate_request_msg: Option<CertificateRequestMsg>,
    /// Whether the client handshake keys are already installed (installing resets the
    /// record sequence number, so it must happen exactly once).
    client_handshake_keys_installed: bool,
    /// The client's own Certificate message bytes, when it authenticated.
    pub client_certificate_bytes: Vec<u8>,
    /// The client's own CertificateVerify message bytes, when it authenticated.
    pub client_certificate_verify_bytes: Vec<u8>,
    /// The server's Finished message bytes.
    pub server_finished_bytes: Vec<u8>,
    /// The `verify_data` the server sent.
    pub server_verify_data: Vec<u8>,
    /// The client's Finished message bytes.
    pub client_finished_bytes: Vec<u8>,
    /// The server's public key, read out of the leaf certificate.
    pub server_public_key: Option<PublicKey>,
    /// Transcript hash after the ClientHello.
    pub hash_after_client_hello: Vec<u8>,
    /// Transcript hash after the ServerHello — what the handshake traffic secrets use.
    pub hash_after_server_hello: Vec<u8>,
    /// Transcript hash after EncryptedExtensions.
    pub hash_after_encrypted_extensions: Vec<u8>,
    /// Transcript hash after Certificate — what CertificateVerify signs.
    pub hash_after_certificate: Vec<u8>,
    /// Transcript hash after CertificateVerify.
    pub hash_after_certificate_verify: Vec<u8>,
    /// Transcript hash the server's Finished covers: everything before it. In a full
    /// handshake that is the hash after CertificateVerify; in a resumed one, after
    /// EncryptedExtensions.
    pub hash_before_server_finished: Vec<u8>,
    /// Transcript hash after the server's Finished — what the application secrets use.
    pub hash_after_server_finished: Vec<u8>,
    /// Transcript hash after the client's Finished — what `res master` uses.
    pub hash_after_client_finished: Vec<u8>,
    /// How many `ChangeCipherSpec` records the server sent.
    pub server_ccs_records: usize,
    /// Whether a server `ChangeCipherSpec` arrived before the first encrypted message.
    pub server_ccs_before_encrypted: bool,
    /// The ALPN protocol the server chose, when it chose one.
    pub alpn_selected: Option<String>,
    /// Tickets the server has sent so far.
    pub tickets: Vec<NewSessionTicket>,
    /// Application data read while looking for something else.
    pending_app: Vec<u8>,
    /// True once the client's Finished has been sent.
    pub finished: bool,
}

impl Client {
    /// Connect and prepare a client, without sending anything.
    pub async fn connect(
        addr: SocketAddr,
        timeout: Duration,
        config: ClientConfig,
    ) -> TlsResult<Client> {
        let conn = TlsConn::connect(addr, timeout).await?;
        Ok(Client::over(conn, config))
    }

    /// Prepare a client over a connection that is already open.
    pub fn over(conn: TlsConn, config: ClientConfig) -> Client {
        Client {
            conn,
            config,
            key_shares: Vec::new(),
            client_hello: None,
            client_hello_bytes: Vec::new(),
            hello_retry_request: None,
            server_hello: None,
            server_hello_bytes: Vec::new(),
            suite: None,
            selected_group: None,
            shared_secret: Vec::new(),
            schedule: None,
            transcript: None,
            encrypted_extensions: Vec::new(),
            encrypted_extensions_bytes: Vec::new(),
            certificate: None,
            certificate_bytes: Vec::new(),
            certificate_verify: None,
            certificate_verify_bytes: Vec::new(),
            certificate_request: None,
            server_finished_bytes: Vec::new(),
            server_verify_data: Vec::new(),
            certificate_request_msg: None,
            client_handshake_keys_installed: false,
            client_certificate_bytes: Vec::new(),
            client_certificate_verify_bytes: Vec::new(),
            client_finished_bytes: Vec::new(),
            server_public_key: None,
            hash_after_client_hello: Vec::new(),
            hash_after_server_hello: Vec::new(),
            hash_after_encrypted_extensions: Vec::new(),
            hash_after_certificate: Vec::new(),
            hash_after_certificate_verify: Vec::new(),
            hash_before_server_finished: Vec::new(),
            hash_after_server_finished: Vec::new(),
            hash_after_client_finished: Vec::new(),
            server_ccs_records: 0,
            server_ccs_before_encrypted: false,
            alpn_selected: None,
            tickets: Vec::new(),
            pending_app: Vec::new(),
            finished: false,
        }
    }

    // -----------------------------------------------------------------------------------
    // Flight 1: the ClientHello
    // -----------------------------------------------------------------------------------

    /// Build the ClientHello this configuration describes, without sending it.
    pub fn build_client_hello(&mut self) -> TlsResult<ClientHello> {
        let (hello, shares) = build_hello(&self.config)?;
        self.key_shares = shares;
        Ok(hello)
    }

    /// Build the ClientHello and remember the key shares behind it, without sending it.
    ///
    /// A test that wants to put the hello on the wire itself — one record at a time, or
    /// with a byte changed — calls this, writes the bytes, and then hands them back with
    /// [`Client::note_client_hello`] so the transcript and the key shares line up.
    pub fn prepare_client_hello(&mut self) -> TlsResult<(ClientHello, Vec<u8>)> {
        let hello = self.build_client_hello()?;
        let bytes = hello.encode();
        Ok((hello, bytes))
    }

    /// Build and send the ClientHello, starting the transcript.
    pub async fn send_client_hello(&mut self) -> TlsResult<()> {
        let hello = self.build_client_hello()?;
        self.send_built_client_hello(hello).await
    }

    /// Send a ClientHello the caller built (or mangled) itself.
    pub async fn send_built_client_hello(&mut self, hello: ClientHello) -> TlsResult<()> {
        let mut bytes = hello.encode();
        if self.config.psk.is_some() {
            bytes = self.attach_psk_binder(&hello)?;
        }
        self.conn.write_handshake(&bytes).await?;
        self.note_client_hello(hello, bytes);
        Ok(())
    }

    /// Record a ClientHello in the transcript without sending it again — for tests that
    /// wrote the bytes themselves, record by record.
    pub fn note_client_hello(&mut self, hello: ClientHello, bytes: Vec<u8>) {
        // The transcript starts under SHA-256 and is re-hashed if the server picks a
        // SHA-384 suite; the raw bytes never change, only the digest taken over them.
        let hash = self
            .config
            .psk
            .as_ref()
            .map(|p| p.suite.hash)
            .unwrap_or(super::crypto::HashAlg::Sha256);
        let mut transcript = Transcript::new(hash);
        transcript.push(&bytes, "client_hello");
        self.hash_after_client_hello = transcript.current();
        self.transcript = Some(transcript);
        self.client_hello_bytes = bytes;
        self.client_hello = Some(hello);
    }

    /// Build the ClientHello with a `pre_shared_key` extension whose binder is computed
    /// over the truncated hello, as RFC 8446 §4.2.11.2 requires.
    fn attach_psk_binder(&mut self, hello: &ClientHello) -> TlsResult<Vec<u8>> {
        let Some(offer) = self.config.psk.clone() else {
            return Ok(hello.encode());
        };
        let hash = offer.suite.hash;
        let binder_len = hash.len();
        // The extension with a zero-filled binder of the right length, so the lengths are
        // final before anything is hashed.
        let mut identities = Writer::new();
        identities.nest16(|list| {
            list.vec16(&offer.identity).u32(offer.obfuscated_ticket_age);
        });
        let mut binders = Writer::new();
        binders.nest16(|list| {
            list.vec8(&vec![0u8; binder_len]);
        });
        let mut data = identities.finish();
        let binders_bytes = binders.finish();
        data.extend_from_slice(&binders_bytes);

        let mut with_psk = hello.clone();
        with_psk
            .extensions
            .push(Extension::new(EXT_PRE_SHARED_KEY, data));
        let full = with_psk.encode();
        // "Truncated ClientHello": everything up to but not including the binder list.
        let truncated_len = full.len() - binders_bytes.len();
        let truncated = &full[..truncated_len];

        let mut schedule = KeySchedule::new(offer.suite, Some(&offer.psk));
        let binder_key = schedule.derive_binder_key()?;
        let binder_keys = TrafficKeys::derive(offer.suite, &binder_key)?;
        let finished_key = binder_keys.finished_key()?;
        let binder = hash.hmac(&finished_key, &hash.digest(truncated));

        let mut out = full;
        let start = truncated_len + 3; // the binder list length (2) and the binder length (1)
        out[start..start + binder_len].copy_from_slice(&binder);
        if self.config.corrupt_psk_binder {
            out[start] ^= 0x01;
        }
        self.schedule = Some(schedule);
        Ok(out)
    }

    /// Send the `ChangeCipherSpec` compatibility record, if the configuration asks for it.
    pub async fn maybe_send_ccs(&mut self) -> TlsResult<()> {
        if self.config.send_ccs {
            self.conn.write_change_cipher_spec().await?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------------------
    // Flight 2: the ServerHello
    // -----------------------------------------------------------------------------------

    /// Read the ServerHello, following a HelloRetryRequest when one arrives and the
    /// configuration allows it, and install the handshake traffic keys.
    pub async fn read_server_hello(&mut self) -> TlsResult<()> {
        let message = self.next_handshake("server_hello").await?;
        if message.msg_type != HandshakeType::SERVER_HELLO {
            return Err(TlsError::Protocol(format!(
                "the server's first handshake message must be server_hello(2); it sent {}",
                message.name()
            )));
        }
        let hello = ServerHello::parse(&message.body)?;
        if hello.is_hello_retry_request() {
            self.hello_retry_request = Some(hello.clone());
            self.server_hello_bytes = message.raw.clone();
            if !self.config.follow_hello_retry {
                return Err(TlsError::Protocol(
                    "the server asked for a HelloRetryRequest and this client was told not to \
                     follow one"
                        .into(),
                ));
            }
            self.handle_hello_retry(&hello, &message.raw).await?;
            return Box::pin(self.read_server_hello()).await;
        }
        self.server_hello_bytes = message.raw.clone();
        let suite = Suite::from_code(hello.cipher_suite).ok_or_else(|| {
            TlsError::Protocol(format!(
                "server_hello.cipher_suite is {}, which is not one of the three TLS 1.3 suites \
                 this client offered",
                suite_name(hello.cipher_suite)
            ))
        })?;
        if !self.config.cipher_suites.contains(&hello.cipher_suite) {
            return Err(TlsError::Protocol(format!(
                "server_hello.cipher_suite is {}, which the ClientHello never offered",
                suite_name(hello.cipher_suite)
            )));
        }
        self.suite = Some(suite);

        let transcript = self
            .transcript
            .as_mut()
            .ok_or_else(|| TlsError::Protocol("no ClientHello has been sent".into()))?;
        if transcript.hash_alg() != suite.hash {
            // The transcript starts under SHA-256 because that is what two of the three
            // suites use; a server that picks the SHA-384 one moves every hash taken so far.
            transcript.rehash(suite.hash);
            transcript.mark("client_hello");
        }
        self.hash_after_client_hello = transcript.current();
        transcript.push(&message.raw, "server_hello");
        self.hash_after_server_hello = transcript.current();

        let (group, peer_share) = hello.key_share()?.ok_or_else(|| {
            TlsError::Protocol(
                "server_hello carries no key_share extension: a TLS 1.3 server that is not \
                 resuming with psk_ke must send one (RFC 8446 section 4.2.8)"
                    .into(),
            )
        })?;
        let kx = self
            .key_shares
            .iter()
            .find(|k| k.group() == group)
            .ok_or_else(|| {
                TlsError::Protocol(format!(
                    "server_hello.key_share names {}, for which the ClientHello sent no share",
                    group_name(group)
                ))
            })?;
        self.selected_group = Some(group);
        self.shared_secret = kx.complete(&peer_share)?;
        self.server_hello = Some(hello.clone());

        let psk = self.config.psk.as_ref().map(|p| p.psk.clone());
        let accepted_psk = hello.selected_psk()?.is_some();
        let mut schedule = KeySchedule::new(suite, accepted_psk.then_some(()).and(psk.as_deref()));
        let shared = self.shared_secret.clone();
        let hash = self.hash_after_server_hello.clone();
        schedule.enter_handshake(&shared, &hash)?;
        let server_keys = schedule
            .server_handshake
            .clone()
            .ok_or_else(|| TlsError::Crypto("no server handshake traffic keys".into()))?;
        self.conn.layer.set_read(server_keys);
        self.schedule = Some(schedule);
        Ok(())
    }

    /// Answer a HelloRetryRequest with a second ClientHello (RFC 8446 §4.1.4).
    async fn handle_hello_retry(
        &mut self,
        retry: &ServerHello,
        retry_bytes: &[u8],
    ) -> TlsResult<()> {
        let suite = Suite::from_code(retry.cipher_suite).ok_or_else(|| {
            TlsError::Protocol(format!(
                "hello_retry_request.cipher_suite is {}",
                suite_name(retry.cipher_suite)
            ))
        })?;
        let group = retry.retry_group()?.ok_or_else(|| {
            TlsError::Protocol(
                "hello_retry_request carries no key_share telling the client which group to use"
                    .into(),
            )
        })?;
        // The first ClientHello is replaced by the synthetic message_hash before the retry
        // request is appended — the one rule of §4.4.1 that cannot be guessed.
        let transcript = self
            .transcript
            .as_mut()
            .ok_or_else(|| TlsError::Protocol("no ClientHello has been sent".into()))?;
        if transcript.hash_alg() != suite.hash {
            transcript.rehash(suite.hash);
            transcript.mark("client_hello");
        }
        self.hash_after_client_hello = transcript.current();
        transcript.replace_with_message_hash();
        transcript.push(retry_bytes, "hello_retry_request");

        self.config.share_groups = vec![group];
        if !self.config.groups.contains(&group) {
            self.config.groups.push(group);
        }
        let mut hello = self.build_client_hello()?;
        if let Some(cookie) = retry.cookie()? {
            hello.extensions.push(cookie_extension(&cookie));
        }
        let bytes = hello.encode();
        self.conn.write_handshake(&bytes).await?;
        let transcript = self
            .transcript
            .as_mut()
            .ok_or_else(|| TlsError::Protocol("no transcript".into()))?;
        transcript.push(&bytes, "client_hello (second)");
        self.hash_after_client_hello = transcript.current();
        self.client_hello_bytes = bytes;
        self.client_hello = Some(hello);
        self.maybe_send_ccs().await?;
        Ok(())
    }

    // -----------------------------------------------------------------------------------
    // Flight 2 continued: the server's encrypted flight
    // -----------------------------------------------------------------------------------

    /// Read EncryptedExtensions, Certificate, CertificateVerify and Finished, verifying the
    /// signature and the server's `verify_data` as they arrive.
    pub async fn read_server_flight(&mut self) -> TlsResult<()> {
        let mut saw_encrypted_extensions = false;
        loop {
            let message = self.next_handshake("the server's encrypted flight").await?;
            let transcript = self
                .transcript
                .as_mut()
                .ok_or_else(|| TlsError::Protocol("no transcript".into()))?;
            match message.msg_type {
                HandshakeType::ENCRYPTED_EXTENSIONS => {
                    if saw_encrypted_extensions {
                        return Err(TlsError::Protocol(
                            "the server sent encrypted_extensions twice".into(),
                        ));
                    }
                    saw_encrypted_extensions = true;
                    let mut r = super::buf::Reader::new(&message.body, "encrypted_extensions");
                    let ext_bytes = r.vec16("extensions")?;
                    r.expect_done("encrypted_extensions")?;
                    self.encrypted_extensions =
                        parse_extensions(ext_bytes, "encrypted_extensions")?;
                    self.encrypted_extensions_bytes = message.raw.clone();
                    self.alpn_selected = alpn_choice(&self.encrypted_extensions);
                    transcript.push(&message.raw, "encrypted_extensions");
                    self.hash_after_encrypted_extensions = transcript.current();
                }
                HandshakeType::CERTIFICATE_REQUEST => {
                    self.certificate_request_msg =
                        Some(CertificateRequestMsg::parse(&message.body)?);
                    self.certificate_request = Some(message.raw.clone());
                    transcript.push(&message.raw, "certificate_request");
                }
                HandshakeType::CERTIFICATE => {
                    if !saw_encrypted_extensions {
                        return Err(TlsError::Protocol(
                            "certificate arrived before encrypted_extensions: RFC 8446 section 4.3.1 \
                             makes EncryptedExtensions the first message of the encrypted flight"
                                .into(),
                        ));
                    }
                    let cert = CertificateMsg::parse(&message.body)?;
                    self.server_public_key = Some(PublicKey::from_certificate(cert.leaf()?)?);
                    self.certificate = Some(cert);
                    self.certificate_bytes = message.raw.clone();
                    transcript.push(&message.raw, "certificate");
                    self.hash_after_certificate = transcript.current();
                }
                HandshakeType::CERTIFICATE_VERIFY => {
                    let cv = CertificateVerifyMsg::parse(&message.body)?;
                    let key = self.server_public_key.as_ref().ok_or_else(|| {
                        TlsError::Protocol("certificate_verify arrived before certificate".into())
                    })?;
                    sig::verify(
                        key,
                        cv.algorithm,
                        &cv.signature,
                        &self.hash_after_certificate,
                    )?;
                    self.certificate_verify = Some(cv);
                    self.certificate_verify_bytes = message.raw.clone();
                    transcript.push(&message.raw, "certificate_verify");
                    self.hash_after_certificate_verify = transcript.current();
                }
                HandshakeType::FINISHED => {
                    // verify_data covers everything up to but not including the Finished
                    // itself. In a full handshake that is the hash after CertificateVerify;
                    // in a resumed one there is no Certificate at all and it is the hash
                    // after EncryptedExtensions. Taking the transcript as it stands right
                    // now is both, and is what RFC 8446 section 4.4.4 actually says.
                    let expected_over = transcript.current();
                    self.hash_before_server_finished = expected_over.clone();
                    let schedule = self
                        .schedule
                        .as_ref()
                        .ok_or_else(|| TlsError::Crypto("no key schedule".into()))?;
                    let keys = schedule
                        .server_handshake
                        .as_ref()
                        .ok_or_else(|| TlsError::Crypto("no server handshake keys".into()))?;
                    let want = schedule.verify_data(keys, &expected_over)?;
                    if want != message.body {
                        return Err(TlsError::Crypto(format!(
                            "the server's Finished does not match: expected verify_data {}, got {} \
                             (HMAC of the finished key over the transcript hash {})",
                            hex(&want),
                            hex(&message.body),
                            hex(&expected_over)
                        )));
                    }
                    self.server_verify_data = message.body.clone();
                    self.server_finished_bytes = message.raw.clone();
                    let transcript = self
                        .transcript
                        .as_mut()
                        .ok_or_else(|| TlsError::Protocol("no transcript".into()))?;
                    transcript.push(&message.raw, "server finished");
                    self.hash_after_server_finished = transcript.current();
                    let hash = self.hash_after_server_finished.clone();
                    if let Some(schedule) = self.schedule.as_mut() {
                        schedule.enter_application(&hash)?;
                    }
                    return Ok(());
                }
                other => {
                    return Err(TlsError::Protocol(format!(
                        "{} has no place in a server's first flight",
                        other.name()
                    )))
                }
            }
        }
    }

    // -----------------------------------------------------------------------------------
    // Flight 3: the client's Finished
    // -----------------------------------------------------------------------------------

    /// Answer a CertificateRequest with a Certificate and a CertificateVerify.
    ///
    /// Both go out under the client's *handshake* keys and both go into the transcript
    /// before Finished is computed — which is the point of the exercise: the server's
    /// verify_data covers the client's certificate, so authenticating changes the Finished
    /// on both sides. Call this after [`read_server_flight`] and before
    /// [`send_client_finished`].
    pub async fn send_client_auth(&mut self, auth: &ClientAuth) -> TlsResult<()> {
        let request = self.certificate_request_msg.clone().ok_or_else(|| {
            TlsError::Protocol(
                "the server never sent a certificate_request, so there is nothing to answer".into(),
            )
        })?;
        let accepted = request.signature_algorithms()?;
        if !accepted.contains(&auth.scheme) {
            return Err(TlsError::Protocol(format!(
                "the server's certificate_request does not accept 0x{:04x}, the scheme the \
                 suite's client certificate uses",
                auth.scheme
            )));
        }
        let cert = CertificateMsg::one(&request.context, &auth.client_der);
        self.send_client_certificate_bytes(&encode_handshake(
            HandshakeType::CERTIFICATE,
            &cert.encode_body(),
        ))
        .await?;

        let hash = self
            .transcript
            .as_ref()
            .ok_or_else(|| TlsError::Protocol("no transcript".into()))?
            .current();
        let signature = sig::sign_pem(
            &auth.key_pkcs8_pem,
            auth.scheme,
            &sig::client_signed_content(&hash),
        )?;
        let cv = CertificateVerifyMsg {
            algorithm: auth.scheme,
            signature,
        };
        self.send_client_certificate_verify_bytes(&encode_handshake(
            HandshakeType::CERTIFICATE_VERIFY,
            &cv.encode_body(),
        ))
        .await
    }

    /// Send an empty Certificate: "I have nothing you would accept".
    ///
    /// Legal, and the whole difference between a server that *requests* a certificate and
    /// one that *requires* it — the first carries on, the second sends an alert.
    pub async fn send_empty_client_certificate(&mut self) -> TlsResult<()> {
        let context = self
            .certificate_request_msg
            .as_ref()
            .map(|r| r.context.clone())
            .unwrap_or_default();
        let cert = CertificateMsg::none(&context);
        self.send_client_certificate_bytes(&encode_handshake(
            HandshakeType::CERTIFICATE,
            &cert.encode_body(),
        ))
        .await
    }

    /// Send a Certificate the caller built, for the tests that build a wrong one.
    pub async fn send_client_certificate_bytes(&mut self, message: &[u8]) -> TlsResult<()> {
        self.write_client_handshake(message).await?;
        self.client_certificate_bytes = message.to_vec();
        self.transcript
            .as_mut()
            .ok_or_else(|| TlsError::Protocol("no transcript".into()))?
            .push(message, "client certificate");
        Ok(())
    }

    /// Send a CertificateVerify the caller built, for the tests that build a wrong one.
    pub async fn send_client_certificate_verify_bytes(&mut self, message: &[u8]) -> TlsResult<()> {
        self.write_client_handshake(message).await?;
        self.client_certificate_verify_bytes = message.to_vec();
        self.transcript
            .as_mut()
            .ok_or_else(|| TlsError::Protocol("no transcript".into()))?
            .push(message, "client certificate_verify");
        Ok(())
    }

    /// Install the client's handshake keys, once.
    ///
    /// Installing them resets the record sequence number to zero, which is right the first
    /// time and wrong every time after: a client that authenticates writes three protected
    /// handshake records (Certificate, CertificateVerify, Finished) and the nonce for each
    /// is built from a sequence number that must keep counting. Re-installing per message
    /// encrypts all three under nonce zero, and the server's second record fails its MAC.
    fn ensure_client_handshake_keys(&mut self) -> TlsResult<()> {
        if self.client_handshake_keys_installed {
            return Ok(());
        }
        let keys = self
            .schedule
            .as_ref()
            .ok_or_else(|| TlsError::Crypto("no key schedule".into()))?
            .client_handshake
            .clone()
            .ok_or_else(|| TlsError::Crypto("no client handshake keys".into()))?;
        self.conn.layer.set_write(keys);
        self.client_handshake_keys_installed = true;
        Ok(())
    }

    /// Write one handshake message under the client's handshake keys.
    async fn write_client_handshake(&mut self, message: &[u8]) -> TlsResult<()> {
        self.ensure_client_handshake_keys()?;
        self.conn.write_handshake(message).await
    }

    /// Send the client's Finished and switch both directions to application keys.
    pub async fn send_client_finished(&mut self) -> TlsResult<()> {
        let verify_data = self.client_verify_data()?;
        let message = encode_handshake(HandshakeType::FINISHED, &verify_data);
        self.send_client_finished_bytes(&message).await
    }

    /// Send a Finished message the caller built, which is how the "a wrong Finished is
    /// `decrypt_error`" test sends a deliberately bad one.
    pub async fn send_client_finished_bytes(&mut self, message: &[u8]) -> TlsResult<()> {
        self.ensure_client_handshake_keys()?;
        self.conn.write_handshake(message).await?;
        self.client_finished_bytes = message.to_vec();
        let transcript = self
            .transcript
            .as_mut()
            .ok_or_else(|| TlsError::Protocol("no transcript".into()))?;
        transcript.push(message, "client finished");
        self.hash_after_client_finished = transcript.current();
        let hash = self.hash_after_client_finished.clone();
        let (client_app, server_app) = {
            let schedule = self
                .schedule
                .as_mut()
                .ok_or_else(|| TlsError::Crypto("no key schedule".into()))?;
            schedule.derive_resumption_master(&hash)?;
            (
                schedule.client_application.clone(),
                schedule.server_application.clone(),
            )
        };
        if let Some(keys) = client_app {
            self.conn.layer.set_write(keys);
        }
        if let Some(keys) = server_app {
            self.conn.layer.set_read(keys);
        }
        self.finished = true;
        Ok(())
    }

    /// The `verify_data` the client's Finished should carry.
    pub fn client_verify_data(&self) -> TlsResult<Vec<u8>> {
        let schedule = self
            .schedule
            .as_ref()
            .ok_or_else(|| TlsError::Crypto("no key schedule".into()))?;
        let keys = schedule
            .client_handshake
            .as_ref()
            .ok_or_else(|| TlsError::Crypto("no client handshake keys".into()))?;
        // RFC 8446 §4.4.4: the client's verify_data is over the transcript up to and
        // including its own CertificateVerify, when it sent one. Without client
        // authentication nothing stands between the server's Finished and the client's, so
        // the cached hash is that same value — but once the client authenticates, the two
        // differ by two messages and using the cached one produces a Finished the server
        // rejects with decrypt_error.
        let hash = if self.client_certificate_bytes.is_empty() {
            self.hash_after_server_finished.clone()
        } else {
            self.transcript
                .as_ref()
                .ok_or_else(|| TlsError::Protocol("no transcript".into()))?
                .current()
        };
        schedule.verify_data(keys, &hash)
    }

    /// The whole handshake: hello, retry if asked, the server's flight, and Finished.
    pub async fn handshake(&mut self) -> TlsResult<()> {
        self.send_client_hello().await?;
        self.maybe_send_ccs().await?;
        self.read_server_hello().await?;
        self.read_server_flight().await?;
        self.send_client_finished().await?;
        Ok(())
    }

    /// Everything up to and including the server's Finished, stopping before the client's
    /// own flight — where a test that builds its own Certificate or CertificateVerify picks
    /// up.
    pub async fn handshake_through_server_flight(&mut self) -> TlsResult<()> {
        self.send_client_hello().await?;
        self.maybe_send_ccs().await?;
        self.read_server_hello().await?;
        self.read_server_flight().await
    }

    /// A full handshake that answers the server's CertificateRequest along the way.
    ///
    /// The only difference from [`handshake`](Self::handshake) is the two messages sent
    /// between the server's flight and the client's Finished — which is exactly what client
    /// authentication is.
    pub async fn handshake_with_client_auth(&mut self, auth: &ClientAuth) -> TlsResult<()> {
        self.send_client_hello().await?;
        self.maybe_send_ccs().await?;
        self.read_server_hello().await?;
        self.read_server_flight().await?;
        self.send_client_auth(auth).await?;
        self.send_client_finished().await?;
        Ok(())
    }

    /// A full handshake that answers the CertificateRequest with an empty Certificate.
    pub async fn handshake_declining_client_auth(&mut self) -> TlsResult<()> {
        self.send_client_hello().await?;
        self.maybe_send_ccs().await?;
        self.read_server_hello().await?;
        self.read_server_flight().await?;
        self.send_empty_client_certificate().await?;
        self.send_client_finished().await?;
        Ok(())
    }

    // -----------------------------------------------------------------------------------
    // After the handshake
    // -----------------------------------------------------------------------------------

    /// Read the next handshake message, skipping `ChangeCipherSpec` records and turning an
    /// alert into an error that names it.
    pub async fn next_handshake(&mut self, what: &str) -> TlsResult<super::msg::HandshakeMessage> {
        loop {
            match self.conn.next_message().await? {
                Incoming::Handshake(m) => {
                    if m.msg_type == HandshakeType::NEW_SESSION_TICKET {
                        self.tickets.push(NewSessionTicket::parse(&m.body)?);
                        continue;
                    }
                    return Ok(m);
                }
                Incoming::ChangeCipherSpec(_) => {
                    self.server_ccs_records += 1;
                    if self.server_hello.is_none() || self.encrypted_extensions.is_empty() {
                        self.server_ccs_before_encrypted = true;
                    }
                }
                Incoming::Alert(a) => return Err(TlsError::Alert(a.level, a.description)),
                Incoming::AppData(d) => self.pending_app.extend_from_slice(&d),
                Incoming::Unexpected(r) => {
                    return Err(TlsError::Protocol(format!(
                        "a {} record arrived while waiting for {what}",
                        r.content_type.name()
                    )))
                }
            }
        }
    }

    /// Write application data.
    pub async fn write_app_data(&mut self, data: &[u8]) -> TlsResult<()> {
        self.conn.write_app_data(data).await
    }

    /// Read some application data, collecting any tickets or KeyUpdates that arrive first.
    pub async fn read_app_data(&mut self, timeout: Duration) -> TlsResult<Vec<u8>> {
        if !self.pending_app.is_empty() {
            return Ok(std::mem::take(&mut self.pending_app));
        }
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                return Err(TlsError::Timeout("application data from the server".into()));
            }
            match self.conn.next_message_within(left).await? {
                Incoming::AppData(d) if !d.is_empty() => return Ok(d),
                Incoming::AppData(_) => {}
                Incoming::Handshake(m) => self.post_handshake(&m).await?,
                Incoming::ChangeCipherSpec(_) => self.server_ccs_records += 1,
                Incoming::Alert(a) => return Err(TlsError::Alert(a.level, a.description)),
                Incoming::Unexpected(r) => {
                    return Err(TlsError::Protocol(format!(
                        "a {} record arrived where application data was expected",
                        r.content_type.name()
                    )))
                }
            }
        }
    }

    /// Handle a post-handshake message: a ticket, or a KeyUpdate that has to be answered.
    async fn post_handshake(&mut self, m: &super::msg::HandshakeMessage) -> TlsResult<()> {
        match m.msg_type {
            HandshakeType::NEW_SESSION_TICKET => {
                self.tickets.push(NewSessionTicket::parse(&m.body)?);
                Ok(())
            }
            HandshakeType::KEY_UPDATE => {
                let update = KeyUpdate::parse(&m.body)?;
                self.rekey_read()?;
                if update.request_update == 1 {
                    self.send_key_update(KeyUpdate::NOT_REQUESTED).await?;
                }
                Ok(())
            }
            other => Err(TlsError::Protocol(format!(
                "{} is not a post-handshake message a TLS 1.3 server may send",
                other.name()
            ))),
        }
    }

    /// Advance the server's application traffic secret one generation.
    pub fn rekey_read(&mut self) -> TlsResult<()> {
        let schedule = self
            .schedule
            .as_mut()
            .ok_or_else(|| TlsError::Crypto("no key schedule".into()))?;
        let keys = schedule
            .server_application
            .as_ref()
            .ok_or_else(|| TlsError::Crypto("no server application keys".into()))?
            .updated()?;
        schedule.server_application = Some(keys.clone());
        self.conn.layer.set_read(keys);
        Ok(())
    }

    /// Advance the client's application traffic secret one generation.
    pub fn rekey_write(&mut self) -> TlsResult<()> {
        let schedule = self
            .schedule
            .as_mut()
            .ok_or_else(|| TlsError::Crypto("no key schedule".into()))?;
        let keys = schedule
            .client_application
            .as_ref()
            .ok_or_else(|| TlsError::Crypto("no client application keys".into()))?
            .updated()?;
        schedule.client_application = Some(keys.clone());
        self.conn.layer.set_write(keys);
        Ok(())
    }

    /// Send a KeyUpdate and move the write keys on, as RFC 8446 §4.6.3 requires.
    pub async fn send_key_update(&mut self, update: KeyUpdate) -> TlsResult<()> {
        self.conn.write_handshake(&update.encode()).await?;
        self.rekey_write()
    }

    /// Collect tickets the server sends after the handshake, for up to `timeout`.
    pub async fn collect_tickets(&mut self, timeout: Duration) -> TlsResult<usize> {
        let before = self.tickets.len();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                return Ok(self.tickets.len() - before);
            }
            match self.conn.next_message_within(left).await {
                Ok(Incoming::Handshake(m)) => self.post_handshake(&m).await?,
                Ok(Incoming::ChangeCipherSpec(_)) => self.server_ccs_records += 1,
                Ok(Incoming::AppData(d)) => self.pending_app.extend_from_slice(&d),
                Ok(Incoming::Alert(a)) => return Err(TlsError::Alert(a.level, a.description)),
                Ok(Incoming::Unexpected(_)) => {}
                Err(TlsError::Timeout(_)) => return Ok(self.tickets.len() - before),
                Err(e) => return Err(e),
            }
        }
    }

    /// Send one line and read the reversed line back, which is what `-rev` does.
    ///
    /// The reference strips the trailing `\r`/`\n`, reverses what is left and writes it
    /// back with exactly one `\n`, so this returns the answer without that newline.
    pub async fn echo_line(&mut self, line: &str) -> TlsResult<String> {
        self.write_app_data(format!("{line}\n").as_bytes()).await?;
        self.read_line().await
    }

    /// Read application data until a newline arrives, and return the line without it.
    pub async fn read_line(&mut self) -> TlsResult<String> {
        let timeout = self.conn.timeout;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(at) = self.pending_app.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.pending_app.drain(..=at).collect();
                return Ok(String::from_utf8_lossy(&line[..at]).to_string());
            }
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                return Err(TlsError::Timeout(format!(
                    "a newline-terminated answer (got {} byte(s) so far: {:?})",
                    self.pending_app.len(),
                    String::from_utf8_lossy(&self.pending_app)
                )));
            }
            let more = self.read_app_data(left).await?;
            self.pending_app.extend_from_slice(&more);
        }
    }

    /// Application data read but not yet consumed by [`Client::read_line`].
    pub fn pending_app_data(&self) -> &[u8] {
        &self.pending_app
    }

    /// Send `close_notify`.
    pub async fn close(&mut self) -> TlsResult<()> {
        self.conn.write_alert(Alert::close_notify()).await
    }

    /// Send `close_notify` and wait for the server's.
    pub async fn close_both_ways(&mut self, timeout: Duration) -> TlsResult<Alert> {
        self.close().await?;
        loop {
            match self.conn.next_message_within(timeout).await? {
                Incoming::Alert(a) => return Ok(a),
                Incoming::Handshake(m) => self.post_handshake(&m).await?,
                Incoming::AppData(d) => self.pending_app.extend_from_slice(&d),
                Incoming::ChangeCipherSpec(_) => self.server_ccs_records += 1,
                Incoming::Unexpected(_) => {}
            }
        }
    }

    /// A fatal alert, sent under whatever protection is in force.
    pub async fn send_fatal(&mut self, description: AlertDescription) -> TlsResult<()> {
        self.conn
            .write_record(ContentType::Alert, &Alert::fatal(description).encode())
            .await
    }

    /// A PSK offer built from this connection's resumption master secret and a ticket.
    pub fn psk_from_ticket(&self, ticket: &NewSessionTicket) -> TlsResult<PskOffer> {
        let schedule = self
            .schedule
            .as_ref()
            .ok_or_else(|| TlsError::Crypto("no key schedule".into()))?;
        let suite = self
            .suite
            .ok_or_else(|| TlsError::Crypto("no cipher suite was negotiated".into()))?;
        Ok(PskOffer {
            identity: ticket.ticket.clone(),
            // The age is obfuscated by adding the server's `ticket_age_add`; a resumption
            // milliseconds after the ticket arrived is age ~0.
            obfuscated_ticket_age: ticket.ticket_age_add.wrapping_add(0),
            psk: schedule.resumption_psk(&ticket.ticket_nonce)?,
            suite,
        })
    }

    /// The key schedule's report lines, for a failure block.
    pub fn schedule_lines(&self) -> Vec<String> {
        self.schedule
            .as_ref()
            .map(KeySchedule::report_lines)
            .unwrap_or_default()
    }

    /// The transcript-hash checkpoints, for a failure block.
    pub fn transcript_lines(&self) -> Vec<String> {
        self.transcript
            .as_ref()
            .map(|t| {
                t.checkpoints
                    .iter()
                    .map(|(label, h)| format!("after {label:<28} {}", hex(h)))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Build the ClientHello a configuration describes, and the key shares behind it.
///
/// A free function because several stages want the bytes without a connection: the fuzz
/// stage mutates them, the malformed-extension stage rewrites a length inside them, and the
/// worked examples encode them offline.
pub fn build_hello(config: &ClientConfig) -> TlsResult<(ClientHello, Vec<KeyExchange>)> {
    let mut key_shares = Vec::new();
    let mut shares = Vec::new();
    for (i, group) in config.share_groups.iter().enumerate() {
        let kx = KeyExchange::from_entropy(*group, &config.share_entropy(i))?;
        shares.push((*group, kx.public().to_vec()));
        key_shares.push(kx);
    }
    let mut extensions = Vec::new();
    if let Some(name) = &config.server_name {
        extensions.push(server_name_extension(name));
    }
    extensions.push(supported_versions_extension(&config.versions));
    extensions.push(supported_groups_extension(&config.groups));
    extensions.push(signature_algorithms_extension(&config.signature_algorithms));
    extensions.push(client_key_share_extension(&shares));
    if !config.alpn.is_empty() {
        extensions.push(alpn_extension(&config.alpn));
    }
    if config.psk.is_some() {
        extensions.push(psk_key_exchange_modes_extension(&[1]));
    }
    extensions.extend(config.extra_extensions.clone());
    if config.offer_early_data {
        extensions.push(super::msg::early_data_extension());
    }
    Ok((
        ClientHello {
            legacy_version: config.legacy_version,
            random: config.random(),
            legacy_session_id: config.session_id.clone(),
            cipher_suites: config.cipher_suites.clone(),
            legacy_compression_methods: vec![0],
            extensions,
        },
        key_shares,
    ))
}

/// The ALPN protocol an EncryptedExtensions block selected, if it selected one.
pub fn alpn_choice(extensions: &[Extension]) -> Option<String> {
    let e = find_extension(extensions, EXT_ALPN)?;
    let mut r = super::buf::Reader::new(&e.data, "encrypted_extensions.alpn");
    let mut list = r.sub16("protocol_name_list").ok()?;
    let name = list.vec8("protocol_name").ok()?;
    Some(String::from_utf8_lossy(name).to_string())
}

/// The version a ServerHello selected, defaulting to its `legacy_version` when it sent no
/// `supported_versions` extension at all (which is what a TLS 1.2 server does).
pub fn negotiated_version(hello: &ServerHello) -> u16 {
    find_extension(&hello.extensions, EXT_SUPPORTED_VERSIONS)
        .and_then(|e| (e.data.len() == 2).then(|| u16::from_be_bytes([e.data[0], e.data[1]])))
        .unwrap_or(hello.legacy_version)
}

/// True when the ServerHello carries a `key_share` extension.
pub fn has_key_share(hello: &ServerHello) -> bool {
    find_extension(&hello.extensions, EXT_KEY_SHARE).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seeded_config_is_reproducible_and_seed_dependent() {
        let a = ClientConfig::seeded(7);
        let b = ClientConfig::seeded(7);
        let c = ClientConfig::seeded(8);
        assert_eq!(a.random(), b.random());
        assert_eq!(a.session_id, b.session_id);
        assert_ne!(a.random(), c.random());
        assert_eq!(a.session_id.len(), 32);
    }

    #[test]
    fn a_built_client_hello_has_the_mandatory_extensions() {
        // Building a hello needs no socket, so this exercises the builder directly.
        let config = ClientConfig::seeded(3);
        let mut shares = Vec::new();
        let mut key_shares = Vec::new();
        for (i, group) in config.share_groups.iter().enumerate() {
            let kx = KeyExchange::from_entropy(*group, &config.share_entropy(i)).expect("kx");
            shares.push((*group, kx.public().to_vec()));
            key_shares.push(kx);
        }
        let hello = ClientHello {
            legacy_version: config.legacy_version,
            random: config.random(),
            legacy_session_id: config.session_id.clone(),
            cipher_suites: config.cipher_suites.clone(),
            legacy_compression_methods: vec![0],
            extensions: vec![
                server_name_extension("localhost"),
                supported_versions_extension(&config.versions),
                supported_groups_extension(&config.groups),
                signature_algorithms_extension(&config.signature_algorithms),
                client_key_share_extension(&shares),
            ],
        };
        let bytes = hello.encode();
        assert_eq!(bytes[0], 1, "client_hello(1)");
        let back = ClientHello::parse_body(&bytes[4..]).expect("parse");
        assert_eq!(back.legacy_version, 0x0303);
        for ext in [
            super::super::EXT_SUPPORTED_VERSIONS,
            super::super::EXT_SUPPORTED_GROUPS,
            super::super::EXT_SIGNATURE_ALGORITHMS,
            super::super::EXT_KEY_SHARE,
        ] {
            assert!(
                find_extension(&back.extensions, ext).is_some(),
                "missing {}",
                super::super::ext_name(ext)
            );
        }
    }
}
