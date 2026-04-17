use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::RwLock;

const SLACK_API: &str = "https://slack.com/api";

struct CacheEntry {
    name: String,
    expires_at: tokio::time::Instant,
}

pub struct UserDirectory {
    client: reqwest::Client,
    bot_token: String,
    cache: RwLock<HashMap<String, CacheEntry>>,
    ttl: Duration,
}

impl UserDirectory {
    pub fn new(bot_token: String, ttl_secs: u64) -> Self {
        Self {
            client: reqwest::Client::new(),
            bot_token,
            cache: RwLock::new(HashMap::new()),
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    pub async fn resolve(&self, user_id: &str) -> Option<String> {
        {
            let cache = self.cache.read().await;
            if let Some(entry) = cache.get(user_id) {
                if entry.expires_at > tokio::time::Instant::now() {
                    return Some(entry.name.clone());
                }
            }
        }

        let name = self.fetch_user_name(user_id).await?;

        {
            let mut cache = self.cache.write().await;
            cache.insert(
                user_id.to_string(),
                CacheEntry {
                    name: name.clone(),
                    expires_at: tokio::time::Instant::now() + self.ttl,
                },
            );
        }

        Some(name)
    }

    async fn fetch_user_name(&self, user_id: &str) -> Option<String> {
        let resp: serde_json::Value = self
            .client
            .get(format!("{SLACK_API}/users.info"))
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .query(&[("user", user_id)])
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()?;

        if resp["ok"].as_bool() != Some(true) {
            tracing::debug!(
                "Slack users.info failed: {}",
                resp["error"].as_str().unwrap_or("unknown")
            );
            return None;
        }

        resp["user"]["profile"]["display_name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(String::from)
            .or_else(|| {
                resp["user"]["profile"]["real_name"]
                    .as_str()
                    .map(String::from)
            })
    }

    pub async fn clear_cache(&self) {
        self.cache.write().await.clear();
    }

    pub async fn cache_size(&self) -> usize {
        self.cache.read().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cache_defaults_empty() {
        let dir = UserDirectory::new("xoxb-test".to_string(), 3600);
        assert_eq!(dir.cache_size().await, 0);
    }

    #[tokio::test]
    async fn test_clear_cache() {
        let dir = UserDirectory::new("xoxb-test".to_string(), 3600);
        dir.clear_cache().await;
        assert_eq!(dir.cache_size().await, 0);
    }
}
