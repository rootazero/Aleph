//! HMAC Signature Verification
//!
//! Provides HMAC-SHA256 signature verification for webhook payloads.
//! Supports multiple signature formats (GitHub, Stripe, Generic).

use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use tracing::{debug, warn};

use super::config::SignatureFormat;

type HmacSha256 = Hmac<Sha256>;

/// Signature verification result
#[derive(Debug)]
pub enum VerificationResult {
    /// Signature is valid
    Valid,
    /// Signature is invalid
    Invalid,
    /// Signature header is missing
    Missing,
    /// Signature format is malformed
    Malformed(String),
    /// Verification is disabled
    Disabled,
}

impl VerificationResult {
    /// Returns true if the signature is valid or verification is disabled
    pub fn is_ok(&self) -> bool {
        matches!(self, VerificationResult::Valid | VerificationResult::Disabled)
    }

    /// Returns true if the signature verification failed
    pub fn is_err(&self) -> bool {
        !self.is_ok()
    }
}

/// Verify a webhook signature
///
/// # Arguments
/// * `format` - The signature format to use
/// * `secret` - The HMAC secret
/// * `signature_header` - The signature header value (if present)
/// * `payload` - The raw request body
///
/// # Returns
/// The verification result
pub fn verify_signature(
    format: SignatureFormat,
    secret: &str,
    signature_header: Option<&str>,
    payload: &[u8],
) -> VerificationResult {
    match format {
        SignatureFormat::None => VerificationResult::Disabled,
        SignatureFormat::Github => verify_github_signature(secret, signature_header, payload),
        SignatureFormat::Stripe => verify_stripe_signature(secret, signature_header, payload),
        SignatureFormat::Generic => verify_generic_signature(secret, signature_header, payload),
    }
}

/// Verify GitHub-style signature
///
/// Format: `sha256=<hex_signature>`
fn verify_github_signature(
    secret: &str,
    signature_header: Option<&str>,
    payload: &[u8],
) -> VerificationResult {
    let header = match signature_header {
        Some(h) => h,
        None => return VerificationResult::Missing,
    };

    // Parse "sha256=<hex>" format
    let signature_hex = match header.strip_prefix("sha256=") {
        Some(hex) => hex,
        None => {
            return VerificationResult::Malformed(
                "GitHub signature must start with 'sha256='".to_string(),
            )
        }
    };

    // Decode hex signature
    let signature_bytes = match hex::decode(signature_hex) {
        Ok(bytes) => bytes,
        Err(e) => return VerificationResult::Malformed(format!("Invalid hex: {}", e)),
    };

    // Compute expected signature
    let expected = compute_hmac_sha256(secret, payload);

    // Constant-time comparison
    if constant_time_compare(&signature_bytes, &expected) {
        debug!("GitHub signature verified successfully");
        VerificationResult::Valid
    } else {
        warn!("GitHub signature verification failed");
        VerificationResult::Invalid
    }
}

/// Verify Stripe-style signature
///
/// Format: `t=<timestamp>,v1=<signature>[,v0=<old_signature>]`
fn verify_stripe_signature(
    secret: &str,
    signature_header: Option<&str>,
    payload: &[u8],
) -> VerificationResult {
    let header = match signature_header {
        Some(h) => h,
        None => return VerificationResult::Missing,
    };

    // Parse Stripe signature format
    let mut timestamp: Option<&str> = None;
    let mut signatures: Vec<&str> = Vec::new();

    for part in header.split(',') {
        let part = part.trim();
        if let Some(ts) = part.strip_prefix("t=") {
            timestamp = Some(ts);
        } else if let Some(sig) = part.strip_prefix("v1=") {
            signatures.push(sig);
        }
        // Ignore v0 and other versions
    }

    let timestamp = match timestamp {
        Some(ts) => ts,
        None => {
            return VerificationResult::Malformed("Missing timestamp in Stripe signature".to_string())
        }
    };

    if signatures.is_empty() {
        return VerificationResult::Malformed("Missing v1 signature in Stripe header".to_string());
    }

    // Stripe signs: timestamp + "." + payload
    let signed_payload = format!("{}.{}", timestamp, String::from_utf8_lossy(payload));
    let expected = compute_hmac_sha256(secret, signed_payload.as_bytes());
    let expected_hex = hex::encode(&expected);

    // Check if any of the signatures match
    for sig in signatures {
        if constant_time_compare(sig.as_bytes(), expected_hex.as_bytes()) {
            debug!("Stripe signature verified successfully");
            return VerificationResult::Valid;
        }
    }

    warn!("Stripe signature verification failed");
    VerificationResult::Invalid
}

/// Verify generic signature
///
/// Format: plain hex signature
fn verify_generic_signature(
    secret: &str,
    signature_header: Option<&str>,
    payload: &[u8],
) -> VerificationResult {
    let header = match signature_header {
        Some(h) => h,
        None => return VerificationResult::Missing,
    };

    // Decode hex signature
    let signature_bytes = match hex::decode(header.trim()) {
        Ok(bytes) => bytes,
        Err(e) => return VerificationResult::Malformed(format!("Invalid hex: {}", e)),
    };

    // Compute expected signature
    let expected = compute_hmac_sha256(secret, payload);

    // Constant-time comparison
    if constant_time_compare(&signature_bytes, &expected) {
        debug!("Generic signature verified successfully");
        VerificationResult::Valid
    } else {
        warn!("Generic signature verification failed");
        VerificationResult::Invalid
    }
}

/// Compute HMAC-SHA256
fn compute_hmac_sha256(secret: &str, data: &[u8]) -> Vec<u8> {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Constant-time comparison to prevent timing attacks
fn constant_time_compare(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

/// Generate a signature for testing/debugging
pub fn generate_signature(format: SignatureFormat, secret: &str, payload: &[u8]) -> String {
    let hmac = compute_hmac_sha256(secret, payload);
    let hex_sig = hex::encode(&hmac);

    match format {
        SignatureFormat::Github => format!("sha256={}", hex_sig),
        SignatureFormat::Stripe => {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let signed_payload = format!("{}.{}", timestamp, String::from_utf8_lossy(payload));
            let stripe_hmac = compute_hmac_sha256(secret, signed_payload.as_bytes());
            format!("t={},v1={}", timestamp, hex::encode(&stripe_hmac))
        }
        SignatureFormat::Generic | SignatureFormat::None => hex_sig,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SECRET: &str = "test_secret_key";
    const TEST_PAYLOAD: &[u8] = b"{\"event\":\"test\"}";

    #[test]
    fn test_github_signature_valid() {
        let hmac = compute_hmac_sha256(TEST_SECRET, TEST_PAYLOAD);
        let signature = format!("sha256={}", hex::encode(&hmac));

        let result = verify_signature(
            SignatureFormat::Github,
            TEST_SECRET,
            Some(&signature),
            TEST_PAYLOAD,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_github_signature_invalid() {
        let result = verify_signature(
            SignatureFormat::Github,
            TEST_SECRET,
            Some("sha256=0000000000000000000000000000000000000000000000000000000000000000"),
            TEST_PAYLOAD,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_github_signature_missing() {
        let result = verify_signature(SignatureFormat::Github, TEST_SECRET, None, TEST_PAYLOAD);
        assert!(matches!(result, VerificationResult::Missing));
    }

    #[test]
    fn test_github_signature_malformed() {
        let result = verify_signature(
            SignatureFormat::Github,
            TEST_SECRET,
            Some("invalid_format"),
            TEST_PAYLOAD,
        );
        assert!(matches!(result, VerificationResult::Malformed(_)));
    }

    #[test]
    fn test_stripe_signature_valid() {
        let timestamp = 1234567890u64;
        let signed_payload = format!("{}.{}", timestamp, String::from_utf8_lossy(TEST_PAYLOAD));
        let hmac = compute_hmac_sha256(TEST_SECRET, signed_payload.as_bytes());
        let signature = format!("t={},v1={}", timestamp, hex::encode(&hmac));

        let result = verify_signature(
            SignatureFormat::Stripe,
            TEST_SECRET,
            Some(&signature),
            TEST_PAYLOAD,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_stripe_signature_missing_timestamp() {
        let result = verify_signature(
            SignatureFormat::Stripe,
            TEST_SECRET,
            Some("v1=abc123"),
            TEST_PAYLOAD,
        );
        assert!(matches!(result, VerificationResult::Malformed(_)));
    }

    #[test]
    fn test_generic_signature_valid() {
        let hmac = compute_hmac_sha256(TEST_SECRET, TEST_PAYLOAD);
        let signature = hex::encode(&hmac);

        let result = verify_signature(
            SignatureFormat::Generic,
            TEST_SECRET,
            Some(&signature),
            TEST_PAYLOAD,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_signature_none() {
        let result = verify_signature(SignatureFormat::None, TEST_SECRET, None, TEST_PAYLOAD);
        assert!(matches!(result, VerificationResult::Disabled));
        assert!(result.is_ok());
    }

    #[test]
    fn test_generate_github_signature() {
        let sig = generate_signature(SignatureFormat::Github, TEST_SECRET, TEST_PAYLOAD);
        assert!(sig.starts_with("sha256="));

        let result = verify_signature(SignatureFormat::Github, TEST_SECRET, Some(&sig), TEST_PAYLOAD);
        assert!(result.is_ok());
    }

    #[test]
    fn test_generate_generic_signature() {
        let sig = generate_signature(SignatureFormat::Generic, TEST_SECRET, TEST_PAYLOAD);

        let result = verify_signature(SignatureFormat::Generic, TEST_SECRET, Some(&sig), TEST_PAYLOAD);
        assert!(result.is_ok());
    }

    #[test]
    fn test_constant_time_compare() {
        let a = b"hello";
        let b = b"hello";
        let c = b"world";
        let d = b"hell";

        assert!(constant_time_compare(a, b));
        assert!(!constant_time_compare(a, c));
        assert!(!constant_time_compare(a, d)); // Different lengths
    }
}
