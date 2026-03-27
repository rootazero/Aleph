use std::collections::HashMap;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};
use std::sync::Arc;

use super::api::FeishuApi;

const CACHE_CAPACITY: usize = 500;
const CACHE_TTL: Duration = Duration::from_secs(3600); // 1 hour

enum CachedEntry {
    Found { name: String, fetched_at: Instant },
    NotAvailable { fetched_at: Instant },
}

impl CachedEntry {
    fn is_expired(&self) -> bool {
        let fetched_at = match self {
            CachedEntry::Found { fetched_at, .. } => fetched_at,
            CachedEntry::NotAvailable { fetched_at } => fetched_at,
        };
        fetched_at.elapsed() > CACHE_TTL
    }
}

pub struct UserProfileCache {
    api: Arc<FeishuApi>,
    cache: StdMutex<HashMap<String, CachedEntry>>,
}

impl UserProfileCache {
    pub fn new(api: Arc<FeishuApi>) -> Self {
        Self {
            api,
            cache: StdMutex::new(HashMap::with_capacity(CACHE_CAPACITY)),
        }
    }

    pub async fn resolve_name(&self, open_id: &str) -> Option<String> {
        // Check cache
        {
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = cache.get(open_id) {
                if !entry.is_expired() {
                    return match entry {
                        CachedEntry::Found { name, .. } => Some(name.clone()),
                        CachedEntry::NotAvailable { .. } => None,
                    };
                }
            }
        }

        // Fetch from API
        let result: Result<Option<String>, String> = self.api.get_user_info(open_id).await;

        let entry = match result {
            Ok(Some(name)) => CachedEntry::Found { name: name.clone(), fetched_at: Instant::now() },
            _ => CachedEntry::NotAvailable { fetched_at: Instant::now() },
        };

        let name = match &entry {
            CachedEntry::Found { name, .. } => Some(name.clone()),
            CachedEntry::NotAvailable { .. } => None,
        };

        // Store in cache
        {
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());

            // Evict oldest if at capacity
            if cache.len() >= CACHE_CAPACITY && !cache.contains_key(open_id) {
                if let Some(oldest_key) = cache.iter()
                    .min_by_key(|(_, entry)| match entry {
                        CachedEntry::Found { fetched_at, .. } => *fetched_at,
                        CachedEntry::NotAvailable { fetched_at } => *fetched_at,
                    })
                    .map(|(k, _)| k.clone())
                {
                    cache.remove(&oldest_key);
                }
            }

            cache.insert(open_id.to_string(), entry);
        }

        name
    }
}
