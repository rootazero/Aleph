//! Exec approval manager for handling approval requests and decisions.
//!
//! Provides async approval flow with timeout and event broadcasting.

use std::collections::HashMap;
use crate::sync_primitives::{Arc, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tracing::{debug, warn};

use super::config::{AllowlistEntry, ExecApprovalsFile};
use super::decision::ApprovalRequest;
use super::socket::ApprovalDecisionType;
use super::storage::{ConfigWithHash, ExecApprovalsStorage, StorageError};

/// Default timeout for approval requests (2 minutes)
pub const DEFAULT_APPROVAL_TIMEOUT_MS: u64 = 120_000;

/// Record of an approval request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecApprovalRecord {
    /// Unique request ID
    pub id: String,
    /// Full command string
    pub command: String,
    /// Working directory
    pub cwd: Option<String>,
    /// Host identifier
    pub host: Option<String>,
    /// Agent ID
    pub agent_id: String,
    /// Session key
    pub session_key: String,
    /// Primary executable
    pub executable: String,
    /// Resolved executable path
    pub resolved_path: Option<String>,
    /// Creation timestamp (Unix ms)
    pub created_at_ms: u64,
    /// Expiration timestamp (Unix ms)
    pub expires_at_ms: u64,
    /// Resolution timestamp (Unix ms)
    pub resolved_at_ms: Option<u64>,
    /// Decision (if resolved)
    pub decision: Option<ApprovalDecisionType>,
    /// Who resolved (display name)
    pub resolved_by: Option<String>,
}

impl ExecApprovalRecord {
    /// Create from ApprovalRequest
    pub fn from_request(request: &ApprovalRequest, timeout_ms: u64) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let executable = request
            .analysis
            .segments
            .first()
            .and_then(|s| s.resolution.as_ref())
            .map(|r| r.executable_name.clone())
            .unwrap_or_default();

        let resolved_path = request
            .analysis
            .segments
            .first()
            .and_then(|s| s.resolution.as_ref())
            .and_then(|r| r.resolved_path.as_ref())
            .map(|p| p.to_string_lossy().to_string());

        Self {
            id: request.id.clone(),
            command: request.command.clone(),
            cwd: request.cwd.clone(),
            host: None,
            agent_id: request.agent_id.clone(),
            session_key: request.session_key.clone(),
            executable,
            resolved_path,
            created_at_ms: now,
            expires_at_ms: now + timeout_ms,
            resolved_at_ms: None,
            decision: None,
            resolved_by: None,
        }
    }

    /// Check if expired
    pub fn is_expired(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        now > self.expires_at_ms
    }

    /// Check if resolved
    pub fn is_resolved(&self) -> bool {
        self.decision.is_some()
    }
}

/// Internal pending entry with channel
struct PendingEntry {
    record: ExecApprovalRecord,
    sender: Option<oneshot::Sender<Option<ApprovalDecisionType>>>,
    created_at: Instant,
}

/// Pending approval info for external access
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingApproval {
    pub record: ExecApprovalRecord,
    pub remaining_ms: u64,
}

/// Manager for exec approval requests
///
/// Handles the lifecycle of approval requests:
/// 1. Create request and wait for decision
/// 2. Resolve with user decision
/// 3. Update allowlist on allow-always
pub struct ExecApprovalManager {
    pending: Arc<RwLock<HashMap<String, PendingEntry>>>,
    storage: Arc<ExecApprovalsStorage>,
    config_cache: Arc<RwLock<Option<ConfigWithHash>>>,
}

impl ExecApprovalManager {
    /// Create new manager with default storage
    pub fn new() -> Self {
        Self::with_storage(Arc::new(ExecApprovalsStorage::new()))
    }

    /// Create manager with custom storage
    pub fn with_storage(storage: Arc<ExecApprovalsStorage>) -> Self {
        Self {
            pending: Arc::new(RwLock::new(HashMap::new())),
            storage,
            config_cache: Arc::new(RwLock::new(None)),
        }
    }

    /// Create approval request and return record (does not wait)
    ///
    /// # Arguments
    ///
    /// * `request` - The approval request
    /// * `timeout_ms` - Timeout in milliseconds
    ///
    /// # Returns
    ///
    /// The approval record
    pub fn create(&self, request: &ApprovalRequest, timeout_ms: u64) -> ExecApprovalRecord {
        let record = ExecApprovalRecord::from_request(request, timeout_ms);
        debug!(id = %record.id, command = %record.command, "Created approval request");
        record
    }

    /// Wait for decision on an approval request
    ///
    /// This adds the request to pending and waits for resolution or timeout.
    ///
    /// # Arguments
    ///
    /// * `record` - The approval record to wait on
    ///
    /// # Returns
    ///
    /// The decision, or None if timed out
    pub async fn wait_for_decision(
        &self,
        record: ExecApprovalRecord,
    ) -> Option<ApprovalDecisionType> {
        let timeout_ms = record.expires_at_ms.saturating_sub(record.created_at_ms);
        let timeout = Duration::from_millis(timeout_ms);

        let (tx, rx) = oneshot::channel();
        let id = record.id.clone();

        // Add to pending
        {
            let mut pending = self.pending.write().unwrap_or_else(|e| e.into_inner());
            pending.insert(
                id.clone(),
                PendingEntry {
                    record,
                    sender: Some(tx),
                    created_at: Instant::now(),
                },
            );
        }

        // Wait with timeout
        let result = tokio::time::timeout(timeout, rx).await;

        // Remove from pending
        {
            let mut pending = self.pending.write().unwrap_or_else(|e| e.into_inner());
            pending.remove(&id);
        }

        match result {
            Ok(Ok(decision)) => {
                debug!(id = %id, ?decision, "Approval resolved");
                decision
            }
            Ok(Err(_)) => {
                // Channel closed without decision
                debug!(id = %id, "Approval channel closed");
                None
            }
            Err(_) => {
                // Timeout
                debug!(id = %id, "Approval timed out");
                None
            }
        }
    }

    /// Resolve an approval request with a decision
    ///
    /// # Arguments
    ///
    /// * `id` - Request ID
    /// * `decision` - The decision
    /// * `resolved_by` - Display name of resolver (optional)
    ///
    /// # Returns
    ///
    /// `true` if the request was found and resolved
    pub fn resolve(
        &self,
        id: &str,
        decision: ApprovalDecisionType,
        resolved_by: Option<String>,
    ) -> bool {
        let mut pending = self.pending.write().unwrap_or_else(|e| e.into_inner());

        if let Some(entry) = pending.get_mut(id) {
            // Update record
            entry.record.decision = Some(decision);
            entry.record.resolved_by = resolved_by;
            entry.record.resolved_at_ms = Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
            );

            // Send decision to waiter
            if let Some(sender) = entry.sender.take() {
                let _ = sender.send(Some(decision));
            }

            debug!(id = %id, ?decision, "Resolved approval");
            true
        } else {
            warn!(id = %id, "Approval not found or already resolved");
            false
        }
    }

    /// Get snapshot of a pending approval
    pub fn get_pending(&self, id: &str) -> Option<PendingApproval> {
        let pending = self.pending.read().unwrap_or_else(|e| e.into_inner());
        pending.get(id).map(|entry| {
            let now = Instant::now();
            let elapsed = now.duration_since(entry.created_at);
            let timeout_ms = entry
                .record
                .expires_at_ms
                .saturating_sub(entry.record.created_at_ms);
            let remaining = Duration::from_millis(timeout_ms).saturating_sub(elapsed);

            PendingApproval {
                record: entry.record.clone(),
                remaining_ms: remaining.as_millis() as u64,
            }
        })
    }

    /// List all pending approvals
    pub fn list_pending(&self) -> Vec<PendingApproval> {
        let pending = self.pending.read().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();

        pending
            .values()
            .map(|entry| {
                let elapsed = now.duration_since(entry.created_at);
                let timeout_ms = entry
                    .record
                    .expires_at_ms
                    .saturating_sub(entry.record.created_at_ms);
                let remaining = Duration::from_millis(timeout_ms).saturating_sub(elapsed);

                PendingApproval {
                    record: entry.record.clone(),
                    remaining_ms: remaining.as_millis() as u64,
                }
            })
            .collect()
    }

    /// Get current config with hash
    pub fn get_config(&self) -> Result<ConfigWithHash, StorageError> {
        // Try cache first
        {
            let cache = self.config_cache.read().unwrap_or_else(|e| e.into_inner());
            if let Some(ref cached) = *cache {
                return Ok(cached.clone());
            }
        }

        // Load from storage
        let loaded = self.storage.load()?;

        // Update cache
        {
            let mut cache = self.config_cache.write().unwrap_or_else(|e| e.into_inner());
            *cache = Some(loaded.clone());
        }

        Ok(loaded)
    }

    /// Set config with optimistic locking
    ///
    /// # Arguments
    ///
    /// * `config` - New config
    /// * `base_hash` - Hash from when config was loaded
    ///
    /// # Returns
    ///
    /// New hash after save
    pub fn set_config(
        &self,
        config: ExecApprovalsFile,
        base_hash: &str,
    ) -> Result<String, StorageError> {
        let new_hash = self.storage.save(&config, base_hash)?;

        // Update cache
        {
            let mut cache = self.config_cache.write().unwrap_or_else(|e| e.into_inner());
            *cache = Some(ConfigWithHash {
                config,
                hash: new_hash.clone(),
            });
        }

        Ok(new_hash)
    }

    /// Invalidate config cache (force reload on next access)
    pub fn invalidate_cache(&self) {
        let mut cache = self.config_cache.write().unwrap_or_else(|e| e.into_inner());
        *cache = None;
    }

    /// Add entry to allowlist (called on allow-always)
    ///
    /// # Arguments
    ///
    /// * `agent_id` - Agent to add allowlist for
    /// * `pattern` - Pattern to add (usually resolved path)
    pub fn add_to_allowlist(&self, agent_id: &str, pattern: &str) -> Result<(), StorageError> {
        let ConfigWithHash { mut config, hash } = self.get_config()?;

        // Get or create agent config
        let agent_config = config.agents.entry(agent_id.to_string()).or_default();

        // Get or create allowlist
        let allowlist = agent_config.allowlist.get_or_insert_with(Vec::new);

        // Check if already exists
        if allowlist.iter().any(|e| e.pattern == pattern) {
            return Ok(());
        }

        // Add new entry
        allowlist.push(AllowlistEntry {
            id: Some(uuid::Uuid::new_v4().to_string()),
            pattern: pattern.to_string(),
            last_used_at: Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64,
            ),
            last_used_command: None,
            last_resolved_path: None,
        });

        // Save with optimistic lock
        self.set_config(config, &hash)?;

        Ok(())
    }

    /// Clean up expired pending requests
    pub fn cleanup_expired(&self) {
        let mut pending = self.pending.write().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();

        pending.retain(|id, entry| {
            let elapsed = now.duration_since(entry.created_at);
            let timeout_ms = entry
                .record
                .expires_at_ms
                .saturating_sub(entry.record.created_at_ms);

            if elapsed > Duration::from_millis(timeout_ms) {
                // Send None to waiter
                if let Some(sender) = entry.sender.take() {
                    let _ = sender.send(None);
                }
                debug!(id = %id, "Cleaned up expired approval");
                false
            } else {
                true
            }
        });
    }
}

impl Default for ExecApprovalManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::analysis::CommandAnalysis;
    use tempfile::TempDir;

    fn temp_manager() -> (TempDir, ExecApprovalManager) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("exec-approvals.json");
        let storage = Arc::new(ExecApprovalsStorage::with_path(path));
        let manager = ExecApprovalManager::with_storage(storage);
        (dir, manager)
    }

    fn mock_request() -> ApprovalRequest {
        ApprovalRequest {
            id: uuid::Uuid::new_v4().to_string(),
            command: "npm install".to_string(),
            cwd: Some("/project".to_string()),
            analysis: CommandAnalysis {
                ok: true,
                reason: None,
                segments: vec![],
                chains: None,
            },
            agent_id: "main".to_string(),
            session_key: "agent:main:main".to_string(),
        }
    }

    #[test]
    fn test_create_record() {
        let (_dir, manager) = temp_manager();
        let request = mock_request();

        let record = manager.create(&request, 60_000);

        assert_eq!(record.id, request.id);
        assert_eq!(record.command, "npm install");
        assert!(record.expires_at_ms > record.created_at_ms);
    }

    #[tokio::test]
    async fn test_resolve_approval() {
        let (_dir, manager) = temp_manager();
        let request = mock_request();
        let record = manager.create(&request, 60_000);
        let id = record.id.clone();

        // Spawn wait task
        let manager_clone = ExecApprovalManager::with_storage(manager.storage.clone());
        manager_clone.pending.write().unwrap().insert(
            id.clone(),
            PendingEntry {
                record: record.clone(),
                sender: None,
                created_at: Instant::now(),
            },
        );

        // Resolve
        let resolved = manager_clone.resolve(&id, ApprovalDecisionType::AllowOnce, None);
        assert!(resolved);

        // Check pending
        let pending = manager_clone.get_pending(&id);
        assert!(pending.is_some());
        assert_eq!(
            pending.unwrap().record.decision,
            Some(ApprovalDecisionType::AllowOnce)
        );
    }

    #[test]
    fn test_list_pending() {
        let (_dir, manager) = temp_manager();

        // Add some pending
        let request1 = mock_request();
        let record1 = manager.create(&request1, 60_000);
        manager.pending.write().unwrap().insert(
            record1.id.clone(),
            PendingEntry {
                record: record1,
                sender: None,
                created_at: Instant::now(),
            },
        );

        let request2 = mock_request();
        let record2 = manager.create(&request2, 60_000);
        manager.pending.write().unwrap().insert(
            record2.id.clone(),
            PendingEntry {
                record: record2,
                sender: None,
                created_at: Instant::now(),
            },
        );

        let pending = manager.list_pending();
        assert_eq!(pending.len(), 2);
    }

    #[test]
    fn test_config_operations() {
        let (_dir, manager) = temp_manager();

        // Get default config
        let loaded = manager.get_config().unwrap();
        assert_eq!(loaded.config.version, 1);

        // Modify and save
        let mut config = loaded.config.clone();
        config.defaults = Some(super::super::config::ExecDefaults {
            security: Some(super::super::config::ExecSecurity::Allowlist),
            ..Default::default()
        });

        let new_hash = manager.set_config(config, &loaded.hash).unwrap();
        assert_ne!(new_hash, loaded.hash);

        // Verify cache updated
        let reloaded = manager.get_config().unwrap();
        assert!(reloaded.config.defaults.is_some());
    }

    #[test]
    fn test_add_to_allowlist() {
        let (_dir, manager) = temp_manager();

        manager
            .add_to_allowlist("main", "/usr/bin/git")
            .unwrap();

        let config = manager.get_config().unwrap();
        let agent = config.config.agents.get("main").unwrap();
        let allowlist = agent.allowlist.as_ref().unwrap();

        assert_eq!(allowlist.len(), 1);
        assert_eq!(allowlist[0].pattern, "/usr/bin/git");
    }
}
