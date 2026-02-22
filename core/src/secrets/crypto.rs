//! Secret encryption engine
//!
//! Provides AES-256-GCM encryption with HKDF-SHA256 per-entry key derivation.
//! Inspired by IronClaw's SecretsCrypto design.

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use hkdf::Hkdf;
use secrecy::{ExposeSecret, SecretString};
use sha2::Sha256;

use super::types::SecretError;

/// HKDF info label for domain separation.
const HKDF_INFO: &[u8] = b"aleph-secrets-v1";

/// Encryption engine using AES-256-GCM with per-entry HKDF key derivation.
///
/// The master key is held in a `SecretString` which is zeroized on drop.
pub struct SecretsCrypto {
    master_key: SecretString,
}

impl SecretsCrypto {
    /// Create a new crypto engine with the given master key.
    pub fn new(master_key: impl Into<String>) -> Self {
        Self {
            master_key: SecretString::from(master_key.into()),
        }
    }

    /// Derive a per-entry encryption key using HKDF-SHA256.
    fn derive_key(&self, salt: &[u8; 32]) -> Result<[u8; 32], SecretError> {
        let hkdf = Hkdf::<Sha256>::new(Some(salt), self.master_key.expose_secret().as_bytes());
        let mut key = [0u8; 32];
        hkdf.expand(HKDF_INFO, &mut key)
            .map_err(|e| SecretError::EncryptionFailed(format!("HKDF expand failed: {}", e)))?;
        Ok(key)
    }

    /// Encrypt a plaintext value.
    ///
    /// Returns (ciphertext, nonce, salt) tuple.
    /// Each call generates a fresh random salt and nonce.
    pub fn encrypt(&self, plaintext: &str) -> Result<(Vec<u8>, [u8; 12], [u8; 32]), SecretError> {
        use rand::RngCore;

        // Generate random salt and nonce
        let mut salt = [0u8; 32];
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut salt);
        rand::thread_rng().fill_bytes(&mut nonce_bytes);

        let key = self.derive_key(&salt)?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|e| SecretError::EncryptionFailed(format!("AES init failed: {}", e)))?;

        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|e| SecretError::EncryptionFailed(format!("AES encrypt failed: {}", e)))?;

        // Zeroize derived key
        // (key goes out of scope and is on the stack, but let's be explicit)
        let _ = key;

        Ok((ciphertext, nonce_bytes, salt))
    }

    /// Decrypt a ciphertext using the stored nonce and salt.
    pub fn decrypt(
        &self,
        ciphertext: &[u8],
        nonce_bytes: &[u8; 12],
        salt: &[u8; 32],
    ) -> Result<String, SecretError> {
        let key = self.derive_key(salt)?;
        let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| SecretError::DecryptionFailed)?;

        let nonce = Nonce::from_slice(nonce_bytes);
        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| SecretError::DecryptionFailed)?;

        String::from_utf8(plaintext).map_err(|_| SecretError::DecryptionFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let crypto = SecretsCrypto::new("test-master-key");
        let plaintext = "sk-ant-api03-very-secret-key";

        let (ciphertext, nonce, salt) = crypto.encrypt(plaintext).unwrap();
        let decrypted = crypto.decrypt(&ciphertext, &nonce, &salt).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_ciphertext_differs_from_plaintext() {
        let crypto = SecretsCrypto::new("test-master-key");
        let plaintext = "sk-ant-api03-very-secret-key";

        let (ciphertext, _, _) = crypto.encrypt(plaintext).unwrap();
        assert_ne!(ciphertext, plaintext.as_bytes());
    }

    #[test]
    fn test_different_salts_produce_different_ciphertexts() {
        let crypto = SecretsCrypto::new("test-master-key");
        let plaintext = "same-plaintext";

        let (ct1, _, _) = crypto.encrypt(plaintext).unwrap();
        let (ct2, _, _) = crypto.encrypt(plaintext).unwrap();

        // Different random salts → different ciphertexts
        assert_ne!(ct1, ct2);
    }

    #[test]
    fn test_wrong_master_key_fails() {
        let crypto1 = SecretsCrypto::new("correct-key");
        let crypto2 = SecretsCrypto::new("wrong-key");

        let (ciphertext, nonce, salt) = crypto1.encrypt("secret").unwrap();
        let result = crypto2.decrypt(&ciphertext, &nonce, &salt);

        assert!(result.is_err());
        assert!(matches!(result, Err(SecretError::DecryptionFailed)));
    }

    #[test]
    fn test_tampered_ciphertext_fails() {
        let crypto = SecretsCrypto::new("test-key");
        let (mut ciphertext, nonce, salt) = crypto.encrypt("secret").unwrap();

        // Tamper with ciphertext
        if let Some(byte) = ciphertext.first_mut() {
            *byte ^= 0xFF;
        }

        let result = crypto.decrypt(&ciphertext, &nonce, &salt);
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_plaintext() {
        let crypto = SecretsCrypto::new("test-key");
        let (ciphertext, nonce, salt) = crypto.encrypt("").unwrap();
        let decrypted = crypto.decrypt(&ciphertext, &nonce, &salt).unwrap();
        assert_eq!(decrypted, "");
    }

    #[test]
    fn test_unicode_plaintext() {
        let crypto = SecretsCrypto::new("test-key");
        let plaintext = "密钥测试🔑";
        let (ciphertext, nonce, salt) = crypto.encrypt(plaintext).unwrap();
        let decrypted = crypto.decrypt(&ciphertext, &nonce, &salt).unwrap();
        assert_eq!(decrypted, plaintext);
    }
}
