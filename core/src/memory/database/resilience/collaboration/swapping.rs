//! Agent Swapping
//!
//! Implements the Agent Swapping mechanism for memory optimization.
//! Idle agents can be serialized to disk and restored via Shadow Replay.

use crate::error::AlephError;
use crate::memory::database::resilience::recovery::ShadowReplayEngine;
use crate::memory::database::resilience::{SessionStatus, SubagentSession};
use crate::memory::database::VectorDatabase;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Swapping configuration
#[derive(Debug, Clone)]
pub struct SwapConfig {
    /// Directory for swap files
    pub swap_dir: PathBuf,

    /// Maximum idle sessions in memory before swapping
    pub max_idle_in_memory: usize,

    /// Minimum idle time before eligible for swapping (seconds)
    pub min_idle_time_secs: i64,
}

impl Default for SwapConfig {
    fn default() -> Self {
        Self {
            swap_dir: PathBuf::from("/tmp/aleph/swaps"),
            max_idle_in_memory: 10,
            min_idle_time_secs: 300, // 5 minutes
        }
    }
}

/// Serialized agent context for swap file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwappedContext {
    /// Session ID
    pub session_id: String,

    /// Agent type
    pub agent_type: String,

    /// Parent session ID
    pub parent_session_id: String,

    /// Timestamp when swapped
    pub swapped_at: i64,

    /// Total tokens used before swap
    pub total_tokens_used: u64,

    /// Total tool calls before swap
    pub total_tool_calls: u64,
}

/// Swap operation result
#[derive(Debug, Clone)]
pub struct SwapResult {
    /// Sessions swapped out
    pub swapped_out: usize,

    /// Sessions restored
    pub restored: usize,

    /// Total swap file size (bytes)
    pub swap_size_bytes: u64,
}

/// Agent Swap Manager
///
/// Manages the swapping of idle agents to disk to optimize memory usage.
pub struct SwapManager {
    db: Arc<VectorDatabase>,
    replay_engine: Arc<ShadowReplayEngine>,
    config: SwapConfig,

    /// Active swap file paths
    swap_paths: RwLock<std::collections::HashMap<String, PathBuf>>,
}

impl SwapManager {
    /// Create a new Swap Manager
    pub fn new(db: Arc<VectorDatabase>) -> Self {
        let replay_engine = Arc::new(ShadowReplayEngine::new(db.clone()));
        Self::with_config(db, replay_engine, SwapConfig::default())
    }

    /// Create a Swap Manager with custom config
    pub fn with_config(
        db: Arc<VectorDatabase>,
        replay_engine: Arc<ShadowReplayEngine>,
        config: SwapConfig,
    ) -> Self {
        Self {
            db,
            replay_engine,
            config,
            swap_paths: RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// Swap out an idle session to disk
    pub async fn swap_out(&self, session_id: &str) -> Result<PathBuf, AlephError> {
        // Get session from database
        let session = self
            .db
            .get_session(session_id)
            .await?
            .ok_or_else(|| AlephError::config(format!("Session not found: {}", session_id)))?;

        // Verify session is idle
        if session.status != SessionStatus::Idle {
            return Err(AlephError::config(format!(
                "Can only swap idle sessions, current status: {:?}",
                session.status
            )));
        }

        // Create swap context
        let context = SwappedContext {
            session_id: session.id.clone(),
            agent_type: session.agent_type.clone(),
            parent_session_id: session.parent_session_id.clone(),
            swapped_at: chrono::Utc::now().timestamp(),
            total_tokens_used: session.total_tokens_used,
            total_tool_calls: session.total_tool_calls,
        };

        // Ensure swap directory exists
        tokio::fs::create_dir_all(&self.config.swap_dir)
            .await
            .map_err(|e| AlephError::config(format!("Failed to create swap directory: {}", e)))?;

        // Write swap file
        let swap_path = self.config.swap_dir.join(format!("{}.swap", session_id));
        let json = serde_json::to_string_pretty(&context)
            .map_err(|e| AlephError::config(format!("Failed to serialize context: {}", e)))?;

        tokio::fs::write(&swap_path, &json)
            .await
            .map_err(|e| AlephError::config(format!("Failed to write swap file: {}", e)))?;

        // Update session status
        self.db
            .update_session_status(session_id, SessionStatus::Swapped, Some(swap_path.to_str().unwrap_or("")))
            .await?;

        // Store context path in database
        // (context_path field in subagent_sessions table)

        // Track swap path
        let mut paths = self.swap_paths.write().await;
        paths.insert(session_id.to_string(), swap_path.clone());

        info!(
            session_id = %session_id,
            swap_path = %swap_path.display(),
            "Session swapped to disk"
        );

        Ok(swap_path)
    }

    /// Swap in (restore) a session from disk
    pub async fn swap_in(&self, session_id: &str) -> Result<SubagentSession, AlephError> {
        // Get swap file path
        let swap_path = {
            let paths = self.swap_paths.read().await;
            paths.get(session_id).cloned()
        };

        let swap_path = swap_path.ok_or_else(|| {
            AlephError::config(format!("No swap file found for session: {}", session_id))
        })?;

        // Read and parse swap file
        let json = tokio::fs::read_to_string(&swap_path)
            .await
            .map_err(|e| AlephError::config(format!("Failed to read swap file: {}", e)))?;

        let _context: SwappedContext = serde_json::from_str(&json)
            .map_err(|e| AlephError::config(format!("Failed to parse swap file: {}", e)))?;

        // Update session status
        self.db
            .update_session_status(session_id, SessionStatus::Idle, None)
            .await?;

        // Get updated session
        let session = self
            .db
            .get_session(session_id)
            .await?
            .ok_or_else(|| AlephError::config(format!("Session not found: {}", session_id)))?;

        // Clean up swap file
        if let Err(e) = tokio::fs::remove_file(&swap_path).await {
            warn!(
                session_id = %session_id,
                error = %e,
                "Failed to remove swap file"
            );
        }

        // Remove from tracking
        let mut paths = self.swap_paths.write().await;
        paths.remove(session_id);

        info!(
            session_id = %session_id,
            "Session restored from swap"
        );

        Ok(session)
    }

    /// Check for sessions that should be swapped out
    pub async fn check_swap_pressure(&self) -> Result<Vec<String>, AlephError> {
        // Get idle sessions count
        let idle_count = self.db.count_sessions_by_status(SessionStatus::Idle).await?;

        if idle_count <= self.config.max_idle_in_memory as u64 {
            return Ok(Vec::new());
        }

        // Find oldest idle sessions that exceed the limit
        let sessions_to_swap = self.db.get_idle_sessions(100).await?;

        let now = chrono::Utc::now().timestamp();
        let candidates: Vec<String> = sessions_to_swap
            .into_iter()
            .filter(|s| (now - s.last_active_at) >= self.config.min_idle_time_secs)
            .take((idle_count as usize) - self.config.max_idle_in_memory)
            .map(|s| s.id)
            .collect();

        debug!(
            idle_count = idle_count,
            candidates = candidates.len(),
            "Checked swap pressure"
        );

        Ok(candidates)
    }

    /// Perform automatic swapping based on pressure
    pub async fn auto_swap(&self) -> Result<SwapResult, AlephError> {
        let candidates = self.check_swap_pressure().await?;

        let mut swapped_out = 0;
        let mut swap_size_bytes = 0u64;

        for session_id in candidates {
            match self.swap_out(&session_id).await {
                Ok(path) => {
                    swapped_out += 1;
                    if let Ok(metadata) = tokio::fs::metadata(&path).await {
                        swap_size_bytes += metadata.len();
                    }
                }
                Err(e) => {
                    warn!(
                        session_id = %session_id,
                        error = %e,
                        "Failed to swap out session"
                    );
                }
            }
        }

        Ok(SwapResult {
            swapped_out,
            restored: 0,
            swap_size_bytes,
        })
    }

    /// Get the replay engine for session restoration
    pub fn replay_engine(&self) -> &Arc<ShadowReplayEngine> {
        &self.replay_engine
    }

    /// Get current swap statistics
    pub async fn get_stats(&self) -> SwapStats {
        let paths = self.swap_paths.read().await;
        SwapStats {
            swapped_count: paths.len(),
            swap_dir: self.config.swap_dir.clone(),
        }
    }
}

/// Swap statistics
#[derive(Debug, Clone)]
pub struct SwapStats {
    /// Number of currently swapped sessions
    pub swapped_count: usize,

    /// Swap directory path
    pub swap_dir: PathBuf,
}

impl std::fmt::Debug for SwapManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SwapManager")
            .field("config", &self.config)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_swap_config_default() {
        let config = SwapConfig::default();
        assert_eq!(config.max_idle_in_memory, 10);
        assert_eq!(config.min_idle_time_secs, 300);
    }

    #[test]
    fn test_swapped_context_serialization() {
        let context = SwappedContext {
            session_id: "sess_123".to_string(),
            agent_type: "explorer".to_string(),
            parent_session_id: "parent_456".to_string(),
            swapped_at: 1234567890,
            total_tokens_used: 1000,
            total_tool_calls: 50,
        };

        let json = serde_json::to_string(&context).unwrap();
        let parsed: SwappedContext = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.session_id, context.session_id);
        assert_eq!(parsed.agent_type, context.agent_type);
    }
}
