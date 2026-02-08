//! Maps external identities (Telegram, WhatsApp) to internal user IDs

use dashmap::DashMap;
use serde::{Deserialize, Serialize};

/// Internal user identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UserId {
    Owner,
    Guest(String), // guest_id
}

/// External platform identity
#[derive(Debug, Clone)]
pub struct PlatformIdentity {
    pub platform: String,      // "telegram", "whatsapp"
    pub platform_user_id: String, // "+1234567890", "user123"
}

/// Bidirectional mapping between platform identities and internal user IDs
pub struct IdentityMap {
    /// "platform:user_id" -> UserId
    external_to_internal: DashMap<String, UserId>,
    /// UserId -> Vec<"platform:user_id">
    internal_to_external: DashMap<UserId, Vec<String>>,
}

impl IdentityMap {
    pub fn new() -> Self {
        Self {
            external_to_internal: DashMap::new(),
            internal_to_external: DashMap::new(),
        }
    }

    /// Create identity key from platform and user_id
    fn make_key(platform: &str, platform_user_id: &str) -> String {
        format!("{}:{}", platform, platform_user_id)
    }

    /// Resolve external identity to internal user ID
    pub fn resolve(&self, platform: &str, platform_user_id: &str) -> Option<UserId> {
        let key = Self::make_key(platform, platform_user_id);
        self.external_to_internal.get(&key).map(|v| v.clone())
    }

    /// Add or update mapping
    pub fn add_mapping(&self, platform: &str, platform_user_id: &str, user_id: UserId) {
        let key = Self::make_key(platform, platform_user_id);
        self.external_to_internal.insert(key.clone(), user_id.clone());

        // Update reverse mapping
        self.internal_to_external
            .entry(user_id)
            .or_default()
            .push(key);
    }

    /// Remove mapping
    pub fn remove_mapping(&self, platform: &str, platform_user_id: &str) {
        let key = Self::make_key(platform, platform_user_id);
        if let Some((_, user_id)) = self.external_to_internal.remove(&key) {
            if let Some(mut external_ids) = self.internal_to_external.get_mut(&user_id) {
                external_ids.retain(|k| k != &key);
            }
        }
    }

    /// Get all external identities for a user
    pub fn get_external_identities(&self, user_id: &UserId) -> Vec<String> {
        self.internal_to_external
            .get(user_id)
            .map(|v| v.clone())
            .unwrap_or_default()
    }
}

impl Default for IdentityMap {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_returns_none_for_unknown() {
        let map = IdentityMap::new();
        assert!(map.resolve("telegram", "unknown").is_none());
    }

    #[test]
    fn test_add_and_resolve_mapping() {
        let map = IdentityMap::new();
        map.add_mapping("telegram", "12345", UserId::Owner);

        let result = map.resolve("telegram", "12345");
        assert_eq!(result, Some(UserId::Owner));
    }

    #[test]
    fn test_multiple_external_ids_for_one_user() {
        let map = IdentityMap::new();
        map.add_mapping("telegram", "12345", UserId::Owner);
        map.add_mapping("whatsapp", "+1234567890", UserId::Owner);

        let external_ids = map.get_external_identities(&UserId::Owner);
        assert_eq!(external_ids.len(), 2);
        assert!(external_ids.contains(&"telegram:12345".to_string()));
    }

    #[test]
    fn test_remove_mapping() {
        let map = IdentityMap::new();
        map.add_mapping("telegram", "12345", UserId::Owner);

        map.remove_mapping("telegram", "12345");
        assert!(map.resolve("telegram", "12345").is_none());
    }
}
