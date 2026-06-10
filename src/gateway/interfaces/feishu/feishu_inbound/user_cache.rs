use crate::sync_primitives::Mutex as StdMutex;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::gateway::interfaces::feishu::api::FeishuApi;

const MAX_CAPACITY: usize = 500;
const DEFAULT_TTL: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone)]
pub struct UserProfile {
    pub open_id: String,
    pub name: Option<String>,
}

struct CacheEntry {
    profile: UserProfile,
    inserted_at: Instant,
    last_accessed_at: Instant,
}

pub struct UserProfileCache {
    cache: StdMutex<HashMap<String, CacheEntry>>,
}

impl Default for UserProfileCache {
    fn default() -> Self {
        Self::new()
    }
}

impl UserProfileCache {
    #[must_use]
    pub fn new() -> Self {
        Self {
            cache: StdMutex::new(HashMap::with_capacity(MAX_CAPACITY)),
        }
    }

    fn evict_if_needed(
        guard: &mut crate::sync_primitives::MutexGuard<'_, HashMap<String, CacheEntry>>,
    ) {
        if guard.len() < MAX_CAPACITY {
            return;
        }
        let now = Instant::now();
        guard.retain(|_, e| now.duration_since(e.inserted_at) < DEFAULT_TTL);
        if guard.len() >= MAX_CAPACITY {
            let oldest = guard
                .iter()
                .min_by_key(|(_, e)| e.last_accessed_at)
                .map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                guard.remove(&k);
            }
        }
    }

    pub fn get(&self, open_id: &str) -> Option<UserProfile> {
        let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if let Some(entry) = guard.get_mut(open_id) {
            if now.duration_since(entry.inserted_at) >= DEFAULT_TTL {
                guard.remove(open_id);
                return None;
            }
            entry.last_accessed_at = now;
            return Some(entry.profile.clone());
        }
        None
    }

    pub fn insert(&self, profile: UserProfile) {
        let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        Self::evict_if_needed(&mut guard);
        let now = Instant::now();
        guard.insert(
            profile.open_id.clone(),
            CacheEntry {
                profile,
                inserted_at: now,
                last_accessed_at: now,
            },
        );
    }

    pub async fn get_name(&self, open_id: &str, api: &FeishuApi) -> Option<String> {
        if let Some(profile) = self.get(open_id) {
            return profile.name.clone();
        }
        if let Ok(Some(name)) = api.get_user_info(open_id).await {
            self.insert(UserProfile {
                open_id: open_id.to_string(),
                name: Some(name.clone()),
            });
            return Some(name);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_hit_and_miss() {
        let cache = UserProfileCache::new();
        assert!(cache.get("ou_123").is_none());
        cache.insert(UserProfile {
            open_id: "ou_123".into(),
            name: Some("Alice".into()),
        });
        let p = cache.get("ou_123").unwrap();
        assert_eq!(p.name.as_deref(), Some("Alice"));
    }

    #[test]
    fn test_ttl_eviction() {
        let cache = UserProfileCache::new();
        cache.insert(UserProfile {
            open_id: "ou_1".into(),
            name: Some("Bob".into()),
        });
        assert!(cache.get("ou_1").is_some());
    }
}
