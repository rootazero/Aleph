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

    /// A digest this repository did not compute.
    ///
    /// The test this replaced built `expected` by hashing
    /// `ts | nonce | key | body` with the same `Sha256` calls, in the same
    /// order, as [`verify_signature`] itself — so it agreed with the
    /// implementation by construction. Swap two of those four fields in
    /// production and the test swaps them too: it stays green while every
    /// callback Lark sends is rejected, and there are no credentials in CI to
    /// notice.
    ///
    /// The literal below is `sha256("1700000000" + "n123" + "enc" + "{\"a\":1}")`
    /// as computed by `shasum -a 256`, outside this crate. The *order* it
    /// encodes is Lark's documented one (Feishu Open Platform, "Step 3: Receive
    /// events" — timestamp, nonce, encrypt_key, body, then SHA-256), which is
    /// the half of this that no local fixture can establish: a round-trip only
    /// ever proves we agree with ourselves.
    ///
    /// What this does not cover is the signature *order* under a real
    /// credential — but the throttle and refusal paths, which this note used to
    /// list here as equally unreachable, turned out not to need one: they need
    /// a far end you control, not a real one. `qa/channels/run.sh errors`
    /// drives them through `mock_lark.py`'s `/__inject` queue, and the first
    /// thing it found was that Lark's legacy throttle shape (HTTP 400 carrying
    /// `code: 99991400`) never became `FeishuSendError::RateLimited` at all.
    const LARK_REFERENCE_DIGEST: &str =
        "7f4b9dcba215e2a6da178216cc00fed0b591f17058045440240767ba35736c6b";

    #[test]
    fn signature_matches_an_independently_computed_digest() {
        assert!(verify_signature(
            "enc",
            "1700000000",
            "n123",
            r#"{"a":1}"#,
            LARK_REFERENCE_DIGEST,
        ));
    }

    /// Reordering the concatenation must not still verify.
    ///
    /// The digest of `ts | key | nonce | body` — the single most likely typo,
    /// and the one the self-computed test could not see.
    #[test]
    fn a_reordered_concatenation_does_not_verify() {
        const SWAPPED: &str = "ce501b409d730aa46dcb304c772481b2c2b9776792e5cf9b5e4527c53b973545";
        assert_ne!(SWAPPED, LARK_REFERENCE_DIGEST);
        assert!(!verify_signature(
            "enc",
            "1700000000",
            "n123",
            r#"{"a":1}"#,
            SWAPPED,
        ));
        assert!(!verify_signature("enc", "1700000000", "n123", r#"{"a":1}"#, "deadbeef"));
    }
}
