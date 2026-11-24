//! Caller-updated retrieval configuration, independent of derived data caches.

use crate::config::types::profile::SmartRecallConfig;
use crate::sync_primitives::Arc;
use std::collections::HashMap;
use tokio::sync::RwLock;

#[derive(Clone)]
pub(crate) struct MemorySearchSettings {
    pub workspace: Arc<RwLock<String>>,
    pub session_key: Arc<RwLock<String>>,
    pub smart_recall: Arc<RwLock<HashMap<String, SmartRecallConfig>>>,
}

impl Default for MemorySearchSettings {
    fn default() -> Self {
        Self {
            workspace: Arc::new(RwLock::new(crate::memory::DEFAULT_AGENT.into())),
            session_key: Arc::new(RwLock::new(String::new())),
            smart_recall: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}
