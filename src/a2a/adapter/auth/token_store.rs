use std::collections::HashSet;

use crate::security::secret_equal::secret_equal_bytes;

/// In-memory store for API tokens.
///
/// Provides token validation for the tiered authentication system. Tokens are
/// deduplicated in a `HashSet`, but validation deliberately avoids the
/// hash-based `contains` fast path: presented tokens are attacker-controlled
/// secrets, so each is compared in constant time to avoid leaking a matching
/// prefix via timing.
pub struct TokenStore {
    tokens: HashSet<String>,
}

impl TokenStore {
    pub fn new(tokens: Vec<String>) -> Self {
        Self {
            tokens: tokens.into_iter().collect(),
        }
    }

    pub fn is_valid(&self, token: &str) -> bool {
        // Compare against every stored token without short-circuiting: a plain
        // `HashSet::contains` (or an early `return true`) would make match
        // timing observable. `secret_equal_bytes` is itself length-leak-safe.
        let mut matched = false;
        for stored in &self.tokens {
            matched |= secret_equal_bytes(token.as_bytes(), stored.as_bytes());
        }
        matched
    }

    pub fn add(&mut self, token: String) {
        self.tokens.insert(token);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_store_rejects_all() {
        let store = TokenStore::new(vec![]);
        assert!(!store.is_valid("anything"));
    }

    #[test]
    fn valid_token_accepted() {
        let store = TokenStore::new(vec!["secret-123".to_string()]);
        assert!(store.is_valid("secret-123"));
    }

    #[test]
    fn invalid_token_rejected() {
        let store = TokenStore::new(vec!["secret-123".to_string()]);
        assert!(!store.is_valid("wrong-token"));
    }

    #[test]
    fn add_token_dynamically() {
        let mut store = TokenStore::new(vec![]);
        assert!(!store.is_valid("new-token"));
        store.add("new-token".to_string());
        assert!(store.is_valid("new-token"));
    }

    #[test]
    fn deduplicates_tokens() {
        let store = TokenStore::new(vec!["abc".to_string(), "abc".to_string()]);
        assert!(store.is_valid("abc"));
    }
}
