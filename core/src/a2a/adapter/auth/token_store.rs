use std::collections::HashSet;

/// In-memory store for API tokens.
///
/// Provides simple token validation for the tiered authentication system.
/// Tokens are stored as plain strings in a HashSet for O(1) lookup.
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
        self.tokens.contains(token)
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
