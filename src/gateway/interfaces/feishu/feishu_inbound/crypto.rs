//! Feishu webhook payload cryptography.
//!
//! When an Encrypt Key is configured, Feishu/Lark AES-encrypts event-subscription
//! callbacks and signs them with `sha256(timestamp + nonce + encrypt_key + body)`.
//! These helpers decrypt the payload and verify the signature so webhook mode can
//! actually read events. Previously the body was parsed verbatim and never
//! decrypted, so encrypted callbacks (the default once an Encrypt Key is set —
//! which the config layer *requires* for webhook mode) were silently dropped.

use aes_cbc::Aes256;
use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

type Aes256CbcDec = cbc::Decryptor<Aes256>;

/// Decrypt a Feishu AES-256-CBC encrypted event payload.
///
/// `encrypt_key` is the app's Encrypt Key; `encrypt_b64` is the base64 value of the
/// `encrypt` field. The decryption key is `sha256(encrypt_key)`; the IV is the first
/// 16 bytes of the decoded blob; the remainder is PKCS#7-padded ciphertext.
pub fn decrypt_event(encrypt_key: &str, encrypt_b64: &str) -> Result<String, String> {
    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD
        .decode(encrypt_b64.trim())
        .map_err(|e| format!("base64 decode failed: {e}"))?;
    if data.len() <= 16 {
        return Err("ciphertext too short".to_string());
    }
    let key = Sha256::digest(encrypt_key.as_bytes());
    let (iv, ciphertext) = data.split_at(16);
    let decryptor = Aes256CbcDec::new_from_slices(key.as_slice(), iv)
        .map_err(|e| format!("cipher init failed: {e}"))?;
    let mut buf = ciphertext.to_vec();
    let plaintext = decryptor
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|e| format!("decrypt failed: {e}"))?;
    String::from_utf8(plaintext.to_vec()).map_err(|e| format!("utf8 decode failed: {e}"))
}

/// Verify a Feishu webhook signature.
///
/// Feishu sends `hex(sha256(timestamp + nonce + encrypt_key + raw_body))` in the
/// `X-Lark-Signature` header. Comparison is constant-time.
#[must_use]
pub fn verify_signature(
    encrypt_key: &str,
    timestamp: &str,
    nonce: &str,
    raw_body: &str,
    signature: &str,
) -> bool {
    let mut hasher = Sha256::new();
    hasher.update(timestamp.as_bytes());
    hasher.update(nonce.as_bytes());
    hasher.update(encrypt_key.as_bytes());
    hasher.update(raw_body.as_bytes());
    let computed = hex::encode(hasher.finalize());
    computed.as_bytes().ct_eq(signature.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::{decrypt_event, verify_signature};
    use aes_cbc::Aes256;
    use cbc::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
    use sha2::{Digest, Sha256};

    type Aes256CbcEnc = cbc::Encryptor<Aes256>;

    /// Mirror Feishu's encryption so the decrypt path can be round-tripped.
    fn encrypt_event(encrypt_key: &str, plaintext: &str, iv: &[u8; 16]) -> String {
        use base64::Engine;
        let key = Sha256::digest(encrypt_key.as_bytes());
        let encryptor = Aes256CbcEnc::new_from_slices(key.as_slice(), iv).unwrap();
        let ct = encryptor.encrypt_padded_vec_mut::<Pkcs7>(plaintext.as_bytes());
        let mut blob = iv.to_vec();
        blob.extend_from_slice(&ct);
        base64::engine::general_purpose::STANDARD.encode(blob)
    }

    #[test]
    fn decrypt_round_trip() {
        let key = "test-encrypt-key";
        let plaintext = r#"{"challenge":"abc","type":"url_verification"}"#;
        let iv = [7u8; 16];
        let blob = encrypt_event(key, plaintext, &iv);
        let decrypted = decrypt_event(key, &blob).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn decrypt_rejects_garbage() {
        assert!(decrypt_event("k", "not base64!!!").is_err());
        assert!(decrypt_event("k", "QUJD").is_err()); // decodes to 3 bytes -> too short
    }

    #[test]
    fn signature_matches_reference() {
        let key = "enc";
        let ts = "1700000000";
        let nonce = "n123";
        let body = r#"{"a":1}"#;
        let mut h = Sha256::new();
        h.update(ts.as_bytes());
        h.update(nonce.as_bytes());
        h.update(key.as_bytes());
        h.update(body.as_bytes());
        let expected = hex::encode(h.finalize());
        assert!(verify_signature(key, ts, nonce, body, &expected));
        assert!(!verify_signature(key, ts, nonce, body, "deadbeef"));
    }
}
