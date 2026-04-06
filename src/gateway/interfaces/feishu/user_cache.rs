use std::collections::HashMap;
use std::sync::Mutex as StdMutex;

use crate::gateway::interfaces::feishu::api::FeishuApi;

/// Cached user profile info from Feishu.
#[derive(Debug, Clone)]
pub struct UserProfile {
    pub open_id: String,
    pub name: Option<String>,
}

/// Simple in-memory cache for Feishu user profiles with async API lookup.
pub struct UserProfileCache {
    cache: StdMutex<HashMap<String, UserProfile>>,
}

impl UserProfileCache {
    pub fn new() -> Self {
        Self {
            cache: StdMutex::new(HashMap::new()),
        }
    }

    /// Get a cached profile by open_id.
    pub fn get(&self, open_id: &str) -> Option<UserProfile> {
        let guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        guard.get(open_id).cloned()
    }

    /// Insert or update a profile.
    pub fn insert(&self, profile: UserProfile) {
        let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        guard.insert(profile.open_id.clone(), profile);
    }

    /// Get user name: cache first, then API if not cached.
    /// Updates cache with fetched info.
    pub async fn get_name(&self, open_id: &str, api: &FeishuApi) -> Option<String> {
        // Try cache first
        if let Some(profile) = self.get(open_id) {
            return profile.name.clone();
        }

        // Cache miss - fetch from API
        if let Ok(Some(name)) = api.get_user_info(open_id).await {
            let profile = UserProfile {
                open_id: open_id.to_string(),
                name: Some(name.clone()),
            };
            self.insert(profile);
            return Some(name);
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_miss_and_hit() {
        let cache = UserProfileCache::new();
        assert!(cache.get("ou_123").is_none());

        cache.insert(UserProfile {
            open_id: "ou_123".into(),
            name: Some("Alice".into()),
        });
        let p = cache.get("ou_123").unwrap();
        assert_eq!(p.name.as_deref(), Some("Alice"));
    }
}
