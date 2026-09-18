//! Verifying the server's CertificateVerify (RFC 8446 §4.4.3) with the key it presented.
//!
//! The content that gets signed is deliberately not just the transcript hash:
//!
//! ```text
//! 64 bytes of 0x20            // so a TLS 1.2 signature can never be replayed here
//! "TLS 1.3, server CertificateVerify"
//! 0x00                        // a separator byte
//! Transcript-Hash(ClientHello .. Certificate)
//! ```
//!
//! A server that signs the transcript hash alone, or that uses the *client* context string,
//! produces a signature that verifies against nothing — which is exactly what stage 25
//! checks, byte for byte, before stage 26 does it again for each key type.

use super::crypto::HashAlg;
use super::{
    sig_name, TlsError, TlsResult, SERVER_CV_CONTEXT, SIG_ECDSA_SECP256R1_SHA256, SIG_ED25519,
    SIG_RSA_PKCS1_SHA256, SIG_RSA_PSS_RSAE_SHA256, SIG_RSA_PSS_RSAE_SHA384,
    SIG_RSA_PSS_RSAE_SHA512,
};
use rsa::pkcs1::DecodeRsaPublicKey;
use rsa::pkcs8::DecodePublicKey;
use rsa::signature::Verifier as _;

/// Build the bytes a server signs in CertificateVerify.
pub fn signed_content(context: &str, transcript_hash: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 + context.len() + 1 + transcript_hash.len());
    out.extend_from_slice(&[0x20u8; 64]);
    out.extend_from_slice(context.as_bytes());
    out.push(0);
    out.extend_from_slice(transcript_hash);
    out
}

/// The content a *server* signs: [`signed_content`] with the server context string.
pub fn server_signed_content(transcript_hash: &[u8]) -> Vec<u8> {
    signed_content(SERVER_CV_CONTEXT, transcript_hash)
}

/// The public key of an end-entity certificate, in the shape its algorithm needs.
#[derive(Debug, Clone)]
pub enum PublicKey {
    /// An RSA key, for both the PSS and the PKCS#1 v1.5 schemes.
    Rsa(Box<rsa::RsaPublicKey>),
    /// A secp256r1 key.
    EcdsaP256(Box<p256::ecdsa::VerifyingKey>),
    /// An Ed25519 key.
    Ed25519(Box<ed25519_dalek::VerifyingKey>),
}

impl PublicKey {
    /// The name of the key's algorithm, for a report line.
    pub fn algorithm(&self) -> &'static str {
        match self {
            PublicKey::Rsa(_) => "RSA",
            PublicKey::EcdsaP256(_) => "ECDSA P-256",
            PublicKey::Ed25519(_) => "Ed25519",
        }
    }

    /// Pull the subject public key out of a DER certificate.
    pub fn from_certificate(der: &[u8]) -> TlsResult<PublicKey> {
        use x509_parser::prelude::FromDer;
        let (_, cert) = x509_parser::certificate::X509Certificate::from_der(der)
            .map_err(|e| TlsError::Decode(format!("certificate[0].cert_data is not X.509: {e}")))?;
        let spki = cert.public_key();
        let oid = spki.algorithm.algorithm.to_id_string();
        let key_bytes = spki.subject_public_key.data.as_ref();
        match oid.as_str() {
            // rsaEncryption
            "1.2.840.113549.1.1.1" => {
                let key = rsa::RsaPublicKey::from_pkcs1_der(key_bytes)
                    .or_else(|_| rsa::RsaPublicKey::from_public_key_der(spki.raw))
                    .map_err(|e| {
                        TlsError::Decode(format!("the RSA public key does not parse: {e}"))
                    })?;
                Ok(PublicKey::Rsa(Box::new(key)))
            }
            // id-ecPublicKey — the curve is in the parameters; this client only does P-256.
            "1.2.840.10045.2.1" => {
                let key = p256::ecdsa::VerifyingKey::from_sec1_bytes(key_bytes).map_err(|e| {
                    TlsError::Decode(format!(
                        "the EC public key is not a secp256r1 point (this client only \
                         implements P-256): {e}"
                    ))
                })?;
                Ok(PublicKey::EcdsaP256(Box::new(key)))
            }
            // id-Ed25519
            "1.3.101.112" => {
                let raw: [u8; 32] = key_bytes.try_into().map_err(|_| {
                    TlsError::Decode(format!(
                        "an Ed25519 public key is 32 bytes, this one is {}",
                        key_bytes.len()
                    ))
                })?;
                let key = ed25519_dalek::VerifyingKey::from_bytes(&raw)
                    .map_err(|e| TlsError::Decode(format!("the Ed25519 public key: {e}")))?;
                Ok(PublicKey::Ed25519(Box::new(key)))
            }
            other => Err(TlsError::Decode(format!(
                "the certificate's public key algorithm {other} is not one this client verifies"
            ))),
        }
    }

    /// True when `scheme` can be used with this key at all.
    pub fn accepts(&self, scheme: u16) -> bool {
        matches!(
            (self, scheme),
            (
                PublicKey::Rsa(_),
                SIG_RSA_PSS_RSAE_SHA256
                    | SIG_RSA_PSS_RSAE_SHA384
                    | SIG_RSA_PSS_RSAE_SHA512
                    | SIG_RSA_PKCS1_SHA256
            ) | (PublicKey::EcdsaP256(_), SIG_ECDSA_SECP256R1_SHA256)
                | (PublicKey::Ed25519(_), SIG_ED25519)
        )
    }
}

macro_rules! verify_pss {
    ($digest:ty, $key:expr, $signature:expr, $content:expr) => {{
        let vk = rsa::pss::VerifyingKey::<$digest>::new((**$key).clone());
        let sig = rsa::pss::Signature::try_from($signature).map_err(|e| {
            TlsError::Crypto(format!(
                "certificate_verify.signature is not an RSA-PSS signature: {e}"
            ))
        })?;
        vk.verify($content, &sig).map_err(|e| {
            TlsError::Crypto(format!(
                "the RSA-PSS signature does not verify against the presented key: {e}"
            ))
        })
    }};
}

/// Verify a CertificateVerify signature over `transcript_hash`.
pub fn verify(
    key: &PublicKey,
    scheme: u16,
    signature: &[u8],
    transcript_hash: &[u8],
) -> TlsResult<()> {
    let content = server_signed_content(transcript_hash);
    verify_content(key, scheme, signature, &content)
}

/// Verify a signature over content the caller built, so a test can prove that the *wrong*
/// content does not verify.
pub fn verify_content(
    key: &PublicKey,
    scheme: u16,
    signature: &[u8],
    content: &[u8],
) -> TlsResult<()> {
    if !key.accepts(scheme) {
        return Err(TlsError::Crypto(format!(
            "certificate_verify.algorithm is {} but the certificate carries a {} key",
            sig_name(scheme),
            key.algorithm()
        )));
    }
    match (key, scheme) {
        (PublicKey::Rsa(k), SIG_RSA_PSS_RSAE_SHA256) => {
            verify_pss!(sha2::Sha256, k, signature, content)
        }
        (PublicKey::Rsa(k), SIG_RSA_PSS_RSAE_SHA384) => {
            verify_pss!(sha2::Sha384, k, signature, content)
        }
        (PublicKey::Rsa(k), SIG_RSA_PSS_RSAE_SHA512) => {
            verify_pss!(sha2::Sha512, k, signature, content)
        }
        (PublicKey::Rsa(k), SIG_RSA_PKCS1_SHA256) => {
            let vk = rsa::pkcs1v15::VerifyingKey::<sha2::Sha256>::new((**k).clone());
            let sig = rsa::pkcs1v15::Signature::try_from(signature).map_err(|e| {
                TlsError::Crypto(format!(
                    "certificate_verify.signature is not an RSA signature: {e}"
                ))
            })?;
            vk.verify(content, &sig).map_err(|e| {
                TlsError::Crypto(format!(
                    "the RSA PKCS#1 v1.5 signature does not verify against the presented key: {e}"
                ))
            })
        }
        (PublicKey::EcdsaP256(k), SIG_ECDSA_SECP256R1_SHA256) => {
            let sig = p256::ecdsa::DerSignature::try_from(signature).map_err(|e| {
                TlsError::Crypto(format!(
                    "certificate_verify.signature is not a DER-encoded ECDSA signature: {e}"
                ))
            })?;
            k.verify(content, &sig).map_err(|e| {
                TlsError::Crypto(format!(
                    "the ECDSA P-256 signature does not verify against the presented key: {e}"
                ))
            })
        }
        (PublicKey::Ed25519(k), SIG_ED25519) => {
            let raw: [u8; 64] = signature.try_into().map_err(|_| {
                TlsError::Crypto(format!(
                    "an Ed25519 signature is 64 bytes, this one is {}",
                    signature.len()
                ))
            })?;
            // Ed25519 is PureEdDSA: it signs the content itself, never a pre-hash of it.
            k.verify(content, &ed25519_dalek::Signature::from_bytes(&raw))
                .map_err(|e| {
                    TlsError::Crypto(format!(
                        "the Ed25519 signature does not verify against the presented key: {e}"
                    ))
                })
        }
        (_, other) => Err(TlsError::Crypto(format!(
            "this client cannot verify {}",
            sig_name(other)
        ))),
    }
}

/// The hash a signature scheme is built on, for a report line.
pub fn scheme_hash(scheme: u16) -> Option<HashAlg> {
    match scheme {
        SIG_RSA_PSS_RSAE_SHA256 | SIG_ECDSA_SECP256R1_SHA256 | SIG_RSA_PKCS1_SHA256 => {
            Some(HashAlg::Sha256)
        }
        SIG_RSA_PSS_RSAE_SHA384 => Some(HashAlg::Sha384),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_signed_content_starts_with_sixty_four_spaces_and_a_separator() {
        let c = server_signed_content(&[0xab; 32]);
        assert_eq!(&c[..64], &[0x20u8; 64]);
        assert_eq!(&c[64..64 + 33], SERVER_CV_CONTEXT.as_bytes());
        assert_eq!(
            c[64 + 33],
            0,
            "a zero byte separates the context from the hash"
        );
        assert_eq!(&c[64 + 34..], &[0xab; 32]);
        assert_eq!(c.len(), 64 + 33 + 1 + 32);
    }

    #[test]
    fn the_client_context_produces_different_bytes() {
        let server = signed_content(SERVER_CV_CONTEXT, &[1; 32]);
        let client = signed_content(super::super::CLIENT_CV_CONTEXT, &[1; 32]);
        assert_ne!(server, client);
    }

    #[test]
    fn scheme_hashes_are_named() {
        assert_eq!(scheme_hash(SIG_RSA_PSS_RSAE_SHA256), Some(HashAlg::Sha256));
        assert_eq!(scheme_hash(SIG_RSA_PSS_RSAE_SHA384), Some(HashAlg::Sha384));
        assert_eq!(
            scheme_hash(SIG_ED25519),
            None,
            "Ed25519 signs the content itself"
        );
    }
}
