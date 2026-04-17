//! Streaming Handler
//!
//! Handles Discord presence updates for streaming status preview.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingPreview {
    pub user_id: u64,
    pub username: String,
    pub stream_url: String,
    pub title: String,
    pub viewer_count: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct PresenceUpdate {
    pub user_id: u64,
    pub username: String,
    pub activities: Vec<Activity>,
}

#[derive(Debug, Clone)]
pub struct Activity {
    pub kind: ActivityType,
    pub name: String,
    pub url: Option<String>,
    pub details: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone)]
pub enum ActivityType {
    Playing,
    Streaming,
    Listening,
    Watching,
    Custom,
    Competing,
}

#[derive(Default)]
pub struct StreamingCache {
    entries: HashMap<u64, StreamingPreview>,
}

impl StreamingCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, user_id: u64, preview: StreamingPreview) {
        self.entries.insert(user_id, preview);
    }

    pub fn get(&self, user_id: u64) -> Option<&StreamingPreview> {
        self.entries.get(&user_id)
    }

    pub fn remove(&mut self, user_id: u64) {
        self.entries.remove(&user_id);
    }
}

#[derive(Clone)]
pub struct StreamingHandler {
    cache: Arc<RwLock<StreamingCache>>,
}

impl StreamingHandler {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(RwLock::new(StreamingCache::new())),
        }
    }

    pub async fn handle_presence_update(
        &self,
        update: PresenceUpdate,
    ) -> Result<(), StreamingError> {
        for activity in &update.activities {
            if matches!(activity.kind, ActivityType::Streaming) {
                let preview = StreamingPreview {
                    user_id: update.user_id,
                    username: update.username.clone(),
                    stream_url: activity.url.clone().unwrap_or_default(),
                    title: activity.name.clone(),
                    viewer_count: activity
                        .details
                        .as_ref()
                        .and_then(|d| d.get("viewer_count"))
                        .and_then(|v| v.as_i64()),
                };

                let mut cache = self.cache.write().await;
                cache.set(update.user_id, preview);
            }
        }
        Ok(())
    }

    pub async fn get_preview(&self, user_id: u64) -> Option<StreamingPreview> {
        let cache = self.cache.read().await;
        cache.get(user_id).cloned()
    }

    pub async fn remove_preview(&self, user_id: u64) {
        let mut cache = self.cache.write().await;
        cache.remove(user_id);
    }
}

impl Default for StreamingHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StreamingError {
    #[error("streaming error: {0}")]
    Error(String),
}
