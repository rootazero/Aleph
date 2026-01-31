// core/src/gateway/security/crypto.rs

//! Cryptographic utilities for device authentication.
//!
//! Provides Ed25519 key generation/verification and HMAC-SHA256 token signing.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hmac::{Hmac, Mac};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

/// Errors from cryptographic operations
#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("Invalid public key: {0}")]
    InvalidPublicKey(String),
    #[error("Invalid signature")]
    InvalidSignature,
    #[error("HMAC verification failed")]
    HmacVerificationFailed,
}

/// Device fingerprint - first 16 hex characters of SHA256(public_key)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceFingerprint(pub String);

impl DeviceFingerprint {
    /// Create fingerprint from public key bytes
    pub fn from_public_key(public_key: &[u8]) -> Self {
        use sha2::Digest;
        let hash = Sha256::digest(public_key);
        let hex = hex::encode(hash);
        Self(hex[..16].to_string())
    }
}

impl std::fmt::Display for DeviceFingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Generate a new Ed25519 keypair
///
/// Returns (signing_key_bytes, verifying_key_bytes)
pub fn generate_keypair() -> ([u8; 32], [u8; 32]) {
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();
    (signing_key.to_bytes(), verifying_key.to_bytes())
}

/// Sign a message with Ed25519
pub fn sign_message(signing_key_bytes: &[u8; 32], message: &[u8]) -> [u8; 64] {
    let signing_key = SigningKey::from_bytes(signing_key_bytes);
    let signature = signing_key.sign(message);
    signature.to_bytes()
}

/// Verify an Ed25519 signature
pub fn verify_signature(
    public_key_bytes: &[u8],
    message: &[u8],
    signature_bytes: &[u8],
) -> Result<(), CryptoError> {
    let public_key: [u8; 32] = public_key_bytes
        .try_into()
        .map_err(|_| CryptoError::InvalidPublicKey("Invalid length".into()))?;

    let verifying_key = VerifyingKey::from_bytes(&public_key)
        .map_err(|e| CryptoError::InvalidPublicKey(e.to_string()))?;

    let signature: [u8; 64] = signature_bytes
        .try_into()
        .map_err(|_| CryptoError::InvalidSignature)?;

    let signature = Signature::from_bytes(&signature);

    verifying_key
        .verify(message, &signature)
        .map_err(|_| CryptoError::InvalidSignature)
}

/// Sign a token with HMAC-SHA256
pub fn hmac_sign(secret: &[u8], token: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(token.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Verify an HMAC-SHA256 signature
pub fn hmac_verify(secret: &[u8], token: &str, signature: &str) -> Result<(), CryptoError> {
    let expected = hmac_sign(secret, token);
    // Use constant-time comparison
    if subtle::ConstantTimeEq::ct_eq(expected.as_bytes(), signature.as_bytes()).into() {
        Ok(())
    } else {
        Err(CryptoError::HmacVerificationFailed)
    }
}

/// Generate a random 32-byte secret
pub fn generate_secret() -> [u8; 32] {
    let mut secret = [0u8; 32];
    use rand::RngCore;
    OsRng.fill_bytes(&mut secret);
    secret
}

/// Pairing code constants
pub const PAIRING_CODE_LENGTH: usize = 8;
pub const PAIRING_CODE_CHARSET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// Generate an 8-character Base32 pairing code (excluding confusing chars)
pub fn generate_pairing_code() -> String {
    use rand::Rng;
    let mut rng = OsRng;
    (0..PAIRING_CODE_LENGTH)
        .map(|_| {
            let idx = rng.gen_range(0..PAIRING_CODE_CHARSET.len());
            PAIRING_CODE_CHARSET[idx] as char
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keypair_generation() {
        let (signing, verifying) = generate_keypair();
        assert_eq!(signing.len(), 32);
        assert_eq!(verifying.len(), 32);
    }

    #[test]
    fn test_sign_and_verify() {
        let (signing, verifying) = generate_keypair();
        let message = b"hello world";
        let signature = sign_message(&signing, message);

        assert!(verify_signature(&verifying, message, &signature).is_ok());
    }

    #[test]
    fn test_verify_wrong_message() {
        let (signing, verifying) = generate_keypair();
        let signature = sign_message(&signing, b"hello");

        assert!(verify_signature(&verifying, b"world", &signature).is_err());
    }

    #[test]
    fn test_fingerprint() {
        let (_, verifying) = generate_keypair();
        let fp = DeviceFingerprint::from_public_key(&verifying);
        assert_eq!(fp.0.len(), 16);
        // Should be hex characters
        assert!(fp.0.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_hmac_sign_verify() {
        let secret = generate_secret();
        let token = "test-token-123";
        let signature = hmac_sign(&secret, token);

        assert!(hmac_verify(&secret, token, &signature).is_ok());
        assert!(hmac_verify(&secret, token, "wrong").is_err());
        assert!(hmac_verify(&secret, "wrong-token", &signature).is_err());
    }

    #[test]
    fn test_pairing_code_format() {
        for _ in 0..100 {
            let code = generate_pairing_code();
            assert_eq!(code.len(), 8);
            // Should only contain allowed characters
            assert!(code.chars().all(|c| {
                PAIRING_CODE_CHARSET.contains(&(c as u8))
            }));
            // Should not contain confusing characters
            assert!(!code.contains('0'));
            assert!(!code.contains('1'));
            assert!(!code.contains('I'));
            assert!(!code.contains('O'));
        }
    }
}
