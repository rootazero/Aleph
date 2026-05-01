//! Deterministic short hashing for context identifiers sent to LLMs.

use sha2::{Digest, Sha256};

/// Hashes arbitrary context identifiers into short, deterministic tokens.
///
/// Output format: `ctx:` + first 4 bytes of SHA-256 as 8 hex chars.
pub struct ContextIdHasher;

impl ContextIdHasher {
    pub fn hash(input: &str) -> String {
        let digest = Sha256::digest(input.as_bytes());
        let prefix = u32::from_be_bytes(digest[0..4].try_into().unwrap_or([0; 4]));
        format!("ctx:{:08x}", prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_is_deterministic() {
        let h1 = ContextIdHasher::hash("tg:dm:123");
        let h2 = ContextIdHasher::hash("tg:dm:123");
        assert_eq!(h1, h2);
        assert!(h1.starts_with("ctx:"));
        assert_eq!(h1.len(), 12);
    }

    #[test]
    fn test_hash_different_inputs() {
        let h1 = ContextIdHasher::hash("tg:dm:123");
        let h2 = ContextIdHasher::hash("tg:dm:456");
        assert_ne!(h1, h2);
    }
}
