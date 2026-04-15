//! Thread Binding Handler
//!
//! Manages thread bindings with sub-agent support.

use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Thread info entry
#[derive(Debug, Clone)]
pub struct ThreadInfo {
    /// Original message ID that started the thread
    pub parent_message_id: u64,
    /// Discord thread ID
    pub thread_id: u64,
    /// Guild ID
    pub guild_id: u64,
    /// Channel ID
    pub channel_id: u64,
    /// Agent IDs participating in this thread
    pub participants: Vec<AgentId>,
    /// When the binding was created
    pub created_at: DateTime<Utc>,
}

/// Agent identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentId(pub String);

impl AgentId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Thread binding errors
#[derive(Debug, thiserror::Error)]
pub enum ThreadBindingError {
    #[error("thread binding not found: {0}")]
    NotFound(u64),

    #[error("already bound: {0}")]
    AlreadyBound(u64),

    #[error("sub-agent not allowed in thread: {0}")]
    SubAgentNotAllowed(u64),
}

/// Handler for thread bindings
#[derive(Clone)]
pub struct ThreadBindingHandler {
    /// Thread ID -> ThreadInfo
    bindings: Arc<RwLock<HashMap<u64, ThreadInfo>>>,
    /// Parent message ID -> Thread ID
    message_to_thread: Arc<RwLock<HashMap<u64, u64>>>,
    /// Allow sub-agents by default
    allow_sub_agents: bool,
}

impl ThreadBindingHandler {
    /// Create a new ThreadBindingHandler
    pub fn new() -> Self {
        Self {
            bindings: Arc::new(RwLock::new(HashMap::new())),
            message_to_thread: Arc::new(RwLock::new(HashMap::new())),
            allow_sub_agents: false,
        }
    }

    /// Enable or disable sub-agent participation
    #[must_use]
    pub fn with_sub_agents(mut self, allow: bool) -> Self {
        self.allow_sub_agents = allow;
        self
    }

    /// Create a new thread binding
    pub async fn create_binding(
        &self,
        parent_message_id: u64,
        thread_id: u64,
        guild_id: u64,
        channel_id: u64,
        agent_id: AgentId,
    ) -> Result<ThreadInfo, ThreadBindingError> {
        {
            let bindings = self.bindings.read().await;
            if bindings.contains_key(&thread_id) {
                return Err(ThreadBindingError::AlreadyBound(thread_id));
            }
        }

        let binding = ThreadInfo {
            parent_message_id,
            thread_id,
            guild_id,
            channel_id,
            participants: vec![agent_id],
            created_at: Utc::now(),
        };

        {
            let mut bindings = self.bindings.write().await;
            bindings.insert(thread_id, binding.clone());
        }
        {
            let mut message_to_thread = self.message_to_thread.write().await;
            message_to_thread.insert(parent_message_id, thread_id);
        }

        Ok(binding)
    }

    /// Add a sub-agent participant to a thread
    pub async fn add_participant(
        &self,
        thread_id: u64,
        agent_id: AgentId,
    ) -> Result<(), ThreadBindingError> {
        if !self.allow_sub_agents {
            return Err(ThreadBindingError::SubAgentNotAllowed(thread_id));
        }

        let mut bindings = self.bindings.write().await;
        let binding = bindings
            .get_mut(&thread_id)
            .ok_or_else(|| ThreadBindingError::NotFound(thread_id))?;

        if !binding.participants.contains(&agent_id) {
            binding.participants.push(agent_id);
        }

        Ok(())
    }

    /// Remove a participant from a thread
    pub async fn remove_participant(
        &self,
        thread_id: u64,
        agent_id: &AgentId,
    ) -> Result<(), ThreadBindingError> {
        let mut bindings = self.bindings.write().await;
        let binding = bindings
            .get_mut(&thread_id)
            .ok_or_else(|| ThreadBindingError::NotFound(thread_id))?;

        binding.participants.retain(|a| a != agent_id);
        Ok(())
    }

    /// Get a thread binding by thread ID
    pub async fn get_binding(&self, thread_id: u64) -> Option<ThreadInfo> {
        let bindings = self.bindings.read().await;
        bindings.get(&thread_id).cloned()
    }

    /// Get thread ID for a parent message
    pub async fn get_thread_for_message(&self, message_id: u64) -> Option<u64> {
        let message_to_thread = self.message_to_thread.read().await;
        message_to_thread.get(&message_id).copied()
    }

    /// Delete a thread binding
    pub async fn delete_binding(&self, thread_id: u64) -> Result<(), ThreadBindingError> {
        let binding = {
            let mut bindings = self.bindings.write().await;
            bindings
                .remove(&thread_id)
                .ok_or_else(|| ThreadBindingError::NotFound(thread_id))?
        };

        {
            let mut message_to_thread = self.message_to_thread.write().await;
            message_to_thread.remove(&binding.parent_message_id);
        }

        Ok(())
    }
}

impl Default for ThreadBindingHandler {
    fn default() -> Self {
        Self::new()
    }
}
