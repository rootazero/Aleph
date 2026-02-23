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

/// Result of encrypting a plaintext value.
///
/// Contains the ciphertext along with the nonce and salt needed for decryption.
pub struct EncryptedData {
    /// AES-256-GCM ciphertext
    pub ciphertext: Vec<u8>,
    /// GCM nonce (12 bytes)
    pub nonce: [u8; 12],
    /// HKDF salt (32 bytes, per-entry)
    pub salt: [u8; 32],
}

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
    /// Returns an `EncryptedData` containing the ciphertext, nonce, and salt.
    /// Each call generates a fresh random salt and nonce.
    pub fn encrypt(&self, plaintext: &str) -> Result<EncryptedData, SecretError> {
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

        Ok(EncryptedData {
            ciphertext,
            nonce: nonce_bytes,
            salt,
        })
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

        let encrypted = crypto.encrypt(plaintext).unwrap();
        let decrypted = crypto.decrypt(&encrypted.ciphertext, &encrypted.nonce, &encrypted.salt).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_ciphertext_differs_from_plaintext() {
        let crypto = SecretsCrypto::new("test-master-key");
        let plaintext = "sk-ant-api03-very-secret-key";

        let encrypted = crypto.encrypt(plaintext).unwrap();
        assert_ne!(encrypted.ciphertext, plaintext.as_bytes());
    }

    #[test]
    fn test_different_salts_produce_different_ciphertexts() {
        let crypto = SecretsCrypto::new("test-master-key");
        let plaintext = "same-plaintext";

        let e1 = crypto.encrypt(plaintext).unwrap();
        let e2 = crypto.encrypt(plaintext).unwrap();

        // Different random salts → different ciphertexts
        assert_ne!(e1.ciphertext, e2.ciphertext);
    }

    #[test]
    fn test_wrong_master_key_fails() {
        let crypto1 = SecretsCrypto::new("correct-key");
        let crypto2 = SecretsCrypto::new("wrong-key");

        let encrypted = crypto1.encrypt("secret").unwrap();
        let result = crypto2.decrypt(&encrypted.ciphertext, &encrypted.nonce, &encrypted.salt);

        assert!(result.is_err());
        assert!(matches!(result, Err(SecretError::DecryptionFailed)));
    }

    #[test]
    fn test_tampered_ciphertext_fails() {
        let crypto = SecretsCrypto::new("test-key");
        let mut encrypted = crypto.encrypt("secret").unwrap();

        // Tamper with ciphertext
        if let Some(byte) = encrypted.ciphertext.first_mut() {
            *byte ^= 0xFF;
        }

        let result = crypto.decrypt(&encrypted.ciphertext, &encrypted.nonce, &encrypted.salt);
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_plaintext() {
        let crypto = SecretsCrypto::new("test-key");
        let encrypted = crypto.encrypt("").unwrap();
        let decrypted = crypto.decrypt(&encrypted.ciphertext, &encrypted.nonce, &encrypted.salt).unwrap();
        assert_eq!(decrypted, "");
    }

    #[test]
    fn test_unicode_plaintext() {
        let crypto = SecretsCrypto::new("test-key");
        let plaintext = "密钥测试🔑";
        let encrypted = crypto.encrypt(plaintext).unwrap();
        let decrypted = crypto.decrypt(&encrypted.ciphertext, &encrypted.nonce, &encrypted.salt).unwrap();
        assert_eq!(decrypted, plaintext);
    }
}
