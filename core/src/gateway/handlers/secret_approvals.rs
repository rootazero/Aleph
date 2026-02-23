//! Secret usage approval RPC handlers.
//!
//! - secret.approval.request  — Agent requests permission to use a secret
//! - secret.approval.resolve  — Client approves or denies the request
//! - secret.approvals.pending — List pending approval requests

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{Mutex, Notify};
use tracing::info;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};

/// Approval decision
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalDecision {
    Approved,
    Denied,
}

/// A pending secret approval request
#[derive(Debug, Clone, Serialize)]
pub struct SecretApprovalRequest {
    pub id: String,
    pub secret_name: String,
    pub usage: String,
    pub agent_id: Option<String>,
    pub session_key: Option<String>,
    pub created_at: u64,
    pub timeout_ms: u64,
}

/// Internal record with notification channel
struct ApprovalRecord {
    request: SecretApprovalRequest,
    decision: Option<ApprovalDecision>,
    notify: Arc<Notify>,
}

/// Manager for secret approval requests
pub struct SecretApprovalManager {
    pending: Mutex<HashMap<String, ApprovalRecord>>,
}

impl SecretApprovalManager {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub async fn request_approval(
        &self,
        secret_name: &str,
        usage: &str,
        agent_id: Option<&str>,
        session_key: Option<&str>,
        timeout_ms: u64,
    ) -> Result<ApprovalDecision, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let notify = Arc::new(Notify::new());

        let record = ApprovalRecord {
            request: SecretApprovalRequest {
                id: id.clone(),
                secret_name: secret_name.to_string(),
                usage: usage.to_string(),
                agent_id: agent_id.map(|s| s.to_string()),
                session_key: session_key.map(|s| s.to_string()),
                created_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                timeout_ms,
            },
            decision: None,
            notify: notify.clone(),
        };

        {
            let mut pending = self.pending.lock().await;
            pending.insert(id.clone(), record);
        }

        info!(id = %id, secret_name = %secret_name, usage = %usage, "Secret approval requested");

        let timeout = Duration::from_millis(timeout_ms);
        match tokio::time::timeout(timeout, notify.notified()).await {
            Ok(_) => {
                let mut pending = self.pending.lock().await;
                if let Some(record) = pending.remove(&id) {
                    record.decision.ok_or_else(|| "Decision not set".to_string())
                } else {
                    Err("Approval record not found".to_string())
                }
            }
            Err(_) => {
                let mut pending = self.pending.lock().await;
                pending.remove(&id);
                Err(format!("Approval timed out after {}ms", timeout_ms))
            }
        }
    }

    pub async fn resolve_approval(
        &self,
        id: &str,
        decision: ApprovalDecision,
    ) -> Result<(), String> {
        let mut pending = self.pending.lock().await;
        if let Some(record) = pending.get_mut(id) {
            record.decision = Some(decision.clone());
            record.notify.notify_one();
            info!(id = %id, decision = ?decision, "Secret approval resolved");
            Ok(())
        } else {
            Err(format!("Approval request '{}' not found", id))
        }
    }

    pub async fn list_pending(&self) -> Vec<SecretApprovalRequest> {
        let pending = self.pending.lock().await;
        pending.values().map(|r| r.request.clone()).collect()
    }
}

impl Default for SecretApprovalManager {
    fn default() -> Self {
        Self::new()
    }
}

// --- RPC Handler Functions ---

pub async fn handle_request(
    request: JsonRpcRequest,
    manager: Arc<SecretApprovalManager>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        secret_name: String,
        usage: String,
        agent_id: Option<String>,
        session_key: Option<String>,
        #[serde(default = "default_timeout")]
        timeout_ms: u64,
    }
    fn default_timeout() -> u64 {
        30000
    }

    let params: Params = match &request.params {
        Some(p) => match serde_json::from_value(p.clone()) {
            Ok(parsed) => parsed,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id.clone(),
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                )
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id.clone(),
                INVALID_PARAMS,
                "Missing params",
            )
        }
    };

    match manager
        .request_approval(
            &params.secret_name,
            &params.usage,
            params.agent_id.as_deref(),
            params.session_key.as_deref(),
            params.timeout_ms,
        )
        .await
    {
        Ok(decision) => JsonRpcResponse::success(
            request.id.clone(),
            json!({
                "approved": decision == ApprovalDecision::Approved,
                "decision": decision,
            }),
        ),
        Err(e) => JsonRpcResponse::error(request.id.clone(), INTERNAL_ERROR, e),
    }
}

pub async fn handle_resolve(
    request: JsonRpcRequest,
    manager: Arc<SecretApprovalManager>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        id: String,
        decision: ApprovalDecision,
    }

    let params: Params = match &request.params {
        Some(p) => match serde_json::from_value(p.clone()) {
            Ok(parsed) => parsed,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id.clone(),
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                )
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id.clone(),
                INVALID_PARAMS,
                "Missing params",
            )
        }
    };

    match manager.resolve_approval(&params.id, params.decision).await {
        Ok(()) => JsonRpcResponse::success(request.id.clone(), json!({"ok": true})),
        Err(e) => JsonRpcResponse::error(request.id.clone(), INTERNAL_ERROR, e),
    }
}

pub async fn handle_pending(
    request: JsonRpcRequest,
    manager: Arc<SecretApprovalManager>,
) -> JsonRpcResponse {
    let pending = manager.list_pending().await;
    JsonRpcResponse::success(
        request.id.clone(),
        json!({ "approvals": pending }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_approval_resolve_approved() {
        let manager = Arc::new(SecretApprovalManager::new());
        let mgr = manager.clone();

        let handle = tokio::spawn(async move {
            mgr.request_approval("wallet_key", "sign_tx", None, None, 5000)
                .await
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let pending = manager.list_pending().await;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].secret_name, "wallet_key");

        manager
            .resolve_approval(&pending[0].id, ApprovalDecision::Approved)
            .await
            .unwrap();
        let decision = handle.await.unwrap().unwrap();
        assert_eq!(decision, ApprovalDecision::Approved);
    }

    #[tokio::test]
    async fn test_approval_resolve_denied() {
        let manager = Arc::new(SecretApprovalManager::new());
        let mgr = manager.clone();

        let handle = tokio::spawn(async move {
            mgr.request_approval("wallet_key", "sign_tx", None, None, 5000)
                .await
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        let pending = manager.list_pending().await;
        manager
            .resolve_approval(&pending[0].id, ApprovalDecision::Denied)
            .await
            .unwrap();
        let decision = handle.await.unwrap().unwrap();
        assert_eq!(decision, ApprovalDecision::Denied);
    }

    #[tokio::test]
    async fn test_approval_timeout() {
        let manager = Arc::new(SecretApprovalManager::new());
        let result = manager
            .request_approval("key", "use", None, None, 100)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("timed out"));
    }

    #[tokio::test]
    async fn test_resolve_nonexistent() {
        let manager = SecretApprovalManager::new();
        let result = manager
            .resolve_approval("nonexistent-id", ApprovalDecision::Approved)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_list_pending_empty() {
        let manager = SecretApprovalManager::new();
        let pending = manager.list_pending().await;
        assert!(pending.is_empty());
    }
}
