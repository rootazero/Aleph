use std::collections::HashMap;
use std::sync::Mutex as StdMutex;

/// Cached user profile info from Feishu.
#[derive(Debug, Clone)]
pub struct UserProfile {
    pub open_id: String,
    pub name: Option<String>,
}

/// Simple in-memory cache for Feishu user profiles.
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_miss_and_hit() {
        let cache = UserProfileCache::new();
        assert!(cache.get("ou_123").is_none());

        cache.insert(UserProfile { open_id: "ou_123".into(), name: Some("Alice".into()) });
        let p = cache.get("ou_123").unwrap();
        assert_eq!(p.name.as_deref(), Some("Alice"));
    }
}
