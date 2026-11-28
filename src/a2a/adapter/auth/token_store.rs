use std::collections::HashMap;

use subtle::ConditionallySelectable;
use uuid::Uuid;

use crate::security::secret_equal::secret_equal_bytes;

/// Stores verified API tokens and their process-local A2A identities.
pub struct TokenStore {
    tokens: HashMap<String, Uuid>,
}

impl TokenStore {
    #[must_use]
    pub fn new(tokens: Vec<String>) -> Self {
        Self {
            tokens: tokens
                .into_iter()
                .map(|token| (token, Uuid::new_v4()))
                .collect(),
        }
    }

    #[must_use]
    pub(crate) fn credential_id(&self, token: &str) -> Option<Uuid> {
        if token.is_empty() {
            return None;
        }
        // Compare every token and select its identity without a match-dependent branch.
        let mut identity = 0u128;
        for (stored, id) in &self.tokens {
            let matches = secret_equal_bytes(token.as_bytes(), stored.as_bytes());
            identity = u128::conditional_select(&identity, &id.as_u128(), (matches as u8).into());
        }
        (identity != 0).then(|| Uuid::from_u128(identity))
    }

    #[cfg(test)]
    fn is_valid(&self, token: &str) -> bool {
        self.credential_id(token).is_some()
    }

    pub fn add(&mut self, token: String) {
        self.tokens.entry(token).or_insert_with(Uuid::new_v4);
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

    #[test]
    fn empty_presented_token_never_valid() {
        // Even if a blank token was misconfigured into the store, an empty
        // bearer token must not authenticate (no auth bypass).
        let store = TokenStore::new(vec![String::new()]);
        assert!(!store.is_valid(""));
        // A real token still works.
        let store = TokenStore::new(vec!["real".to_string()]);
        assert!(!store.is_valid(""));
        assert!(store.is_valid("real"));
    }
}
