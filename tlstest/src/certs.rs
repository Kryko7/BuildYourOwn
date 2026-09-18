//! Certificates and keys, generated per run with `rcgen` and written as PEM.
//!
//! The suite never ships a certificate: a committed one would expire, and a learner would
//! have no way to see where the bytes came from. Everything here is minted into the run's
//! temporary directory and handed to the server under test as `-cert` and `-key`.
//!
//! Five kinds exist because the handshake changes shape with each of them:
//!
//! | Kind | What it proves |
//! |---|---|
//! | `EcdsaP256` | the default; CertificateVerify is `ecdsa_secp256r1_sha256` |
//! | `Rsa2048` | CertificateVerify is `rsa_pss_rsae_sha256` — PSS, never PKCS#1 v1.5 |
//! | `Ed25519` | CertificateVerify is `ed25519`, which signs the content itself, unhashed |
//! | `Chain` | `certificate_list` has three entries, leaf first, and each carries its own extensions block |
//! | `Expired` | the client's own clock check, not the server's: a TLS server sends an expired certificate quite happily |
//!
//! Key generation is the one thing `--seed` cannot make deterministic: `rcgen`'s backend
//! takes its randomness from the OS, and an RSA key seeded from a test run would be a
//! footgun waiting to be copied. Everything else in the suite is seeded.

use anyhow::{Context, Result};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
    PKCS_ED25519, PKCS_RSA_SHA256,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The key algorithm of a leaf certificate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum KeyKind {
    /// RSA-2048, which a TLS 1.3 server must sign with RSA-PSS.
    Rsa2048,
    /// ECDSA over secp256r1.
    EcdsaP256,
    /// Ed25519.
    Ed25519,
}

impl KeyKind {
    /// The name used in reports and file names.
    pub fn name(self) -> &'static str {
        match self {
            KeyKind::Rsa2048 => "rsa2048",
            KeyKind::EcdsaP256 => "ecdsa_p256",
            KeyKind::Ed25519 => "ed25519",
        }
    }

    /// The `SignatureScheme` a TLS 1.3 server signs CertificateVerify with for this key.
    pub fn signature_scheme(self) -> u16 {
        match self {
            KeyKind::Rsa2048 => crate::tls::SIG_RSA_PSS_RSAE_SHA256,
            KeyKind::EcdsaP256 => crate::tls::SIG_ECDSA_SECP256R1_SHA256,
            KeyKind::Ed25519 => crate::tls::SIG_ED25519,
        }
    }

    /// Every key kind, for the stages that walk them all.
    pub fn all() -> [KeyKind; 3] {
        [KeyKind::EcdsaP256, KeyKind::Rsa2048, KeyKind::Ed25519]
    }
}

/// What a test wants the server to present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CertKind {
    /// A self-signed end-entity certificate with this key.
    Leaf(KeyKind),
    /// leaf → intermediate → root, all ECDSA P-256, sent leaf first.
    Chain,
    /// A self-signed leaf whose `notAfter` is in the past.
    Expired,
}

impl CertKind {
    /// The file-name stem this kind is written under.
    pub fn stem(self) -> String {
        match self {
            CertKind::Leaf(k) => format!("leaf_{}", k.name()),
            CertKind::Chain => "chain".to_string(),
            CertKind::Expired => "expired".to_string(),
        }
    }

    /// The key algorithm behind it.
    pub fn key_kind(self) -> KeyKind {
        match self {
            CertKind::Leaf(k) => k,
            CertKind::Chain | CertKind::Expired => KeyKind::EcdsaP256,
        }
    }

    /// How this kind reads in a report.
    pub fn describe(self) -> String {
        match self {
            CertKind::Leaf(k) => format!("a self-signed {} leaf", k.name()),
            CertKind::Chain => "a three-certificate chain (leaf, intermediate, root)".to_string(),
            CertKind::Expired => "a self-signed leaf that expired in 2021".to_string(),
        }
    }
}

/// One generated certificate and its key, on disk and in memory.
#[derive(Debug, Clone)]
pub struct Material {
    /// Which kind this is.
    pub kind: CertKind,
    /// The PEM file holding the certificate (and the rest of the chain, when there is one).
    pub cert_pem: PathBuf,
    /// The PEM file holding the private key.
    pub key_pem: PathBuf,
    /// A PEM file holding everything *but* the leaf, when there is a chain. `s_server`
    /// reads only the first certificate out of `-cert`, so the reference is handed the
    /// rest through `-cert_chain`; a hand-written server is expected to send whatever its
    /// `-cert` file holds.
    pub issuers_pem: Option<PathBuf>,
    /// Every certificate's DER, leaf first — what the server should send.
    pub chain_der: Vec<Vec<u8>>,
}

impl Material {
    /// The end-entity certificate's DER.
    pub fn leaf_der(&self) -> &[u8] {
        self.chain_der.first().map(Vec::as_slice).unwrap_or(&[])
    }

    /// How many certificates the server should send.
    pub fn chain_len(&self) -> usize {
        self.chain_der.len()
    }
}

/// Generates certificates on demand and keeps them for the rest of the run.
///
/// RSA-2048 generation is the slow one (a fraction of a second in release, a few seconds in
/// a debug build), so nothing is generated until a stage actually asks for it.
pub struct CertStore {
    dir: PathBuf,
    cache: Mutex<BTreeMap<CertKind, Material>>,
}

impl CertStore {
    /// A store that writes into `dir`.
    pub fn new(dir: &Path) -> Result<CertStore> {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("cannot create the certificate directory {}", dir.display()))?;
        Ok(CertStore {
            dir: dir.to_path_buf(),
            cache: Mutex::new(BTreeMap::new()),
        })
    }

    /// Where the PEM files are written.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The material for `kind`, generating it the first time it is asked for.
    pub fn get(&self, kind: CertKind) -> Result<Material> {
        if let Ok(cache) = self.cache.lock() {
            if let Some(m) = cache.get(&kind) {
                return Ok(m.clone());
            }
        }
        let material = self.generate(kind)?;
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(kind, material.clone());
        }
        Ok(material)
    }

    /// The default material every stage uses unless it says otherwise.
    pub fn default_material(&self) -> Result<Material> {
        self.get(CertKind::Leaf(KeyKind::EcdsaP256))
    }

    fn generate(&self, kind: CertKind) -> Result<Material> {
        let stem = kind.stem();
        let cert_pem = self.dir.join(format!("{stem}.cert.pem"));
        let key_pem = self.dir.join(format!("{stem}.key.pem"));
        let issuers_pem = self.dir.join(format!("{stem}.issuers.pem"));
        let mut issuers: Option<String> = None;
        let (pem_text, key_text, chain_der) = match kind {
            CertKind::Leaf(k) => {
                let key = key_pair(k)?;
                let mut params = leaf_params("tlstest leaf")?;
                params.not_after = rcgen::date_time_ymd(2035, 1, 1);
                let cert = params
                    .self_signed(&key)
                    .context("cannot self-sign the leaf certificate")?;
                (cert.pem(), key.serialize_pem(), vec![cert.der().to_vec()])
            }
            CertKind::Expired => {
                let key = key_pair(KeyKind::EcdsaP256)?;
                let mut params = leaf_params("tlstest expired leaf")?;
                params.not_before = rcgen::date_time_ymd(2020, 1, 1);
                params.not_after = rcgen::date_time_ymd(2021, 1, 1);
                let cert = params
                    .self_signed(&key)
                    .context("cannot self-sign the expired certificate")?;
                (cert.pem(), key.serialize_pem(), vec![cert.der().to_vec()])
            }
            CertKind::Chain => {
                let root_key = key_pair(KeyKind::EcdsaP256)?;
                let mut root_params = CertificateParams::new(Vec::<String>::new())
                    .context("root certificate parameters")?;
                root_params
                    .distinguished_name
                    .push(DnType::CommonName, "tlstest root");
                root_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
                root_params.key_usages = vec![
                    KeyUsagePurpose::KeyCertSign,
                    KeyUsagePurpose::CrlSign,
                    KeyUsagePurpose::DigitalSignature,
                ];
                root_params.not_after = rcgen::date_time_ymd(2035, 1, 1);
                let root = root_params
                    .self_signed(&root_key)
                    .context("cannot self-sign the root certificate")?;

                let inter_key = key_pair(KeyKind::EcdsaP256)?;
                let mut inter_params = CertificateParams::new(Vec::<String>::new())
                    .context("intermediate certificate parameters")?;
                inter_params
                    .distinguished_name
                    .push(DnType::CommonName, "tlstest intermediate");
                inter_params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
                inter_params.key_usages = vec![
                    KeyUsagePurpose::KeyCertSign,
                    KeyUsagePurpose::DigitalSignature,
                ];
                inter_params.not_after = rcgen::date_time_ymd(2035, 1, 1);
                let intermediate = inter_params
                    .signed_by(&inter_key, &root, &root_key)
                    .context("cannot sign the intermediate certificate")?;

                let leaf_key = key_pair(KeyKind::EcdsaP256)?;
                let mut leaf_params = leaf_params("tlstest chained leaf")?;
                leaf_params.not_after = rcgen::date_time_ymd(2035, 1, 1);
                let leaf = leaf_params
                    .signed_by(&leaf_key, &intermediate, &inter_key)
                    .context("cannot sign the leaf certificate")?;

                // The file, and the chain, are leaf first — the order RFC 8446 §4.4.2 wants.
                let pem = format!("{}{}{}", leaf.pem(), intermediate.pem(), root.pem());
                issuers = Some(format!("{}{}", intermediate.pem(), root.pem()));
                (
                    pem,
                    leaf_key.serialize_pem(),
                    vec![
                        leaf.der().to_vec(),
                        intermediate.der().to_vec(),
                        root.der().to_vec(),
                    ],
                )
            }
        };
        std::fs::write(&cert_pem, pem_text)
            .with_context(|| format!("cannot write {}", cert_pem.display()))?;
        std::fs::write(&key_pem, key_text)
            .with_context(|| format!("cannot write {}", key_pem.display()))?;
        restrict(&key_pem);
        let issuers_pem = match issuers {
            Some(text) => {
                std::fs::write(&issuers_pem, text)
                    .with_context(|| format!("cannot write {}", issuers_pem.display()))?;
                Some(issuers_pem)
            }
            None => None,
        };
        Ok(Material {
            kind,
            cert_pem,
            key_pem,
            issuers_pem,
            chain_der,
        })
    }
}

fn leaf_params(common_name: &str) -> Result<CertificateParams> {
    let mut params = CertificateParams::new(vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "tlstest.invalid".to_string(),
    ])
    .context("leaf certificate parameters")?;
    params
        .distinguished_name
        .push(DnType::CommonName, common_name);
    params.not_before = rcgen::date_time_ymd(2024, 1, 1);
    params.is_ca = IsCa::ExplicitNoCa;
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
    Ok(params)
}

fn key_pair(kind: KeyKind) -> Result<KeyPair> {
    match kind {
        KeyKind::EcdsaP256 => {
            KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).context("cannot generate a P-256 key")
        }
        KeyKind::Ed25519 => {
            KeyPair::generate_for(&PKCS_ED25519).context("cannot generate an Ed25519 key")
        }
        KeyKind::Rsa2048 => {
            // rcgen's ring backend can sign with an RSA key but cannot make one, so the key
            // comes from the `rsa` crate and goes back in as PKCS#8.
            use rsa::pkcs8::EncodePrivateKey;
            let private = rsa::RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048)
                .context("cannot generate an RSA-2048 key")?;
            let pem = private
                .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
                .context("cannot encode the RSA key as PKCS#8")?;
            KeyPair::from_pkcs8_pem_and_sign_algo(&pem, &PKCS_RSA_SHA256)
                .context("rcgen refused the generated RSA key")
        }
    }
}

fn restrict(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ecdsa_leaf_is_generated_and_parses_as_x509() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = CertStore::new(dir.path()).expect("store");
        let m = store.get(CertKind::Leaf(KeyKind::EcdsaP256)).expect("leaf");
        assert!(m.cert_pem.is_file() && m.key_pem.is_file());
        assert_eq!(m.chain_len(), 1);
        let key = crate::tls::sig::PublicKey::from_certificate(m.leaf_der()).expect("public key");
        assert_eq!(key.algorithm(), "ECDSA P-256");
        let text = std::fs::read_to_string(&m.cert_pem).expect("read");
        assert!(text.starts_with("-----BEGIN CERTIFICATE-----"), "{text}");
    }

    #[test]
    fn an_ed25519_leaf_carries_an_ed25519_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = CertStore::new(dir.path()).expect("store");
        let m = store.get(CertKind::Leaf(KeyKind::Ed25519)).expect("leaf");
        let key = crate::tls::sig::PublicKey::from_certificate(m.leaf_der()).expect("public key");
        assert_eq!(key.algorithm(), "Ed25519");
        assert!(key.accepts(crate::tls::SIG_ED25519));
        assert!(!key.accepts(crate::tls::SIG_RSA_PSS_RSAE_SHA256));
    }

    #[test]
    fn the_chain_is_three_certificates_leaf_first() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = CertStore::new(dir.path()).expect("store");
        let m = store.get(CertKind::Chain).expect("chain");
        assert_eq!(m.chain_len(), 3);
        let text = std::fs::read_to_string(&m.cert_pem).expect("read");
        assert_eq!(text.matches("BEGIN CERTIFICATE").count(), 3);
        use x509_parser::prelude::FromDer;
        let (_, leaf) =
            x509_parser::certificate::X509Certificate::from_der(m.leaf_der()).expect("leaf der");
        assert!(
            leaf.subject().to_string().contains("chained leaf"),
            "the first certificate must be the end-entity one, got {}",
            leaf.subject()
        );
    }

    #[test]
    fn the_expired_certificate_really_is_expired() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = CertStore::new(dir.path()).expect("store");
        let m = store.get(CertKind::Expired).expect("expired");
        use x509_parser::prelude::FromDer;
        let (_, cert) =
            x509_parser::certificate::X509Certificate::from_der(m.leaf_der()).expect("der");
        assert!(!cert.validity().is_valid(), "the certificate must be expired");
    }

    #[test]
    fn material_is_generated_once_and_then_cached() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = CertStore::new(dir.path()).expect("store");
        let a = store.get(CertKind::Leaf(KeyKind::EcdsaP256)).expect("a");
        let b = store.get(CertKind::Leaf(KeyKind::EcdsaP256)).expect("b");
        assert_eq!(a.chain_der, b.chain_der, "the same material must come back");
    }

    #[test]
    fn key_kinds_name_their_signature_scheme() {
        assert_eq!(
            KeyKind::Rsa2048.signature_scheme(),
            crate::tls::SIG_RSA_PSS_RSAE_SHA256
        );
        assert_eq!(
            KeyKind::EcdsaP256.signature_scheme(),
            crate::tls::SIG_ECDSA_SECP256R1_SHA256
        );
        assert_eq!(KeyKind::Ed25519.signature_scheme(), crate::tls::SIG_ED25519);
    }
}
