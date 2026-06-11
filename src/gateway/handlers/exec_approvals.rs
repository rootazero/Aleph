//! Exec approval RPC handlers.
//!
//! Handlers for exec approval operations:
//! - exec.approval.request - Request approval for a command
//! - exec.approval.resolve - Resolve an approval with a decision
//! - exec.approvals.get - Get approval config with hash
//! - exec.approvals.set - Set approval config (with optimistic lock)
//! - exec.approvals.node.get - Get node approval config
//! - exec.approvals.node.set - Set node approval config

use crate::sync_primitives::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::HandlerRegistry;
use crate::exec::{
    ApprovalBridge, ApprovalDecisionType, ConfigWithHash, ExecApprovalManager, ExecApprovalsFile,
    PendingApproval, StorageError,
};

/// Parameters for exec.approval.request
#[derive(Debug, Deserialize)]
pub struct ApprovalRequestParams {
    /// Command to approve
    pub command: String,
    /// Working directory
    pub cwd: Option<String>,
    /// Agent ID
    pub agent_id: String,
    /// Session key
    pub session_key: String,
    /// Timeout in milliseconds (default: 120000)
    pub timeout_ms: Option<u64>,
}

/// Parameters for exec.approval.resolve
#[derive(Debug, Deserialize)]
pub struct ApprovalResolveParams {
    /// Approval request ID
    pub id: String,
    /// Decision
    pub decision: ApprovalDecisionType,
    /// Display name of resolver
    pub resolved_by: Option<String>,
}

/// Parameters for exec.approvals.set
#[derive(Debug, Deserialize)]
pub struct ApprovalsSetParams {
    /// New config
    pub config: ExecApprovalsFile,
    /// Base hash for optimistic lock
    pub base_hash: String,
}

/// Response for exec.approvals.get
#[derive(Debug, Serialize)]
pub struct ApprovalsGetResponse {
    pub config: ExecApprovalsFile,
    pub hash: String,
}

/// Response for exec.approval.request
#[derive(Debug, Serialize)]
pub struct ApprovalRequestResponse {
    /// Request ID
    pub id: String,
    /// Whether approved
    pub approved: bool,
    /// Decision (if resolved)
    pub decision: Option<ApprovalDecisionType>,
    /// Timeout occurred
    pub timeout: bool,
}

/// Response for list pending
#[derive(Debug, Serialize)]
pub struct PendingListResponse {
    pub pending: Vec<PendingApproval>,
}

/// Parameters for exec.callback.handle
#[derive(Debug, Deserialize)]
pub struct CallbackHandleParams {
    /// Callback data from inline keyboard button
    pub callback_data: String,
    /// User who clicked the button
    pub user_id: String,
}

/// Response for exec.callback.handle
#[derive(Debug, Serialize)]
pub struct CallbackHandleResponse {
    /// Whether the callback was handled
    pub handled: bool,
    /// Response text to show user
    pub response_text: Option<String>,
    /// Approval ID if relevant
    pub approval_id: Option<String>,
    /// Decision made
    pub decision: Option<ApprovalDecisionType>,
}

/// 把 exec-approval 全部方法注册进 JSON-RPC 处理器注册表。
/// 所有方法共享同一个 `Arc<ExecApprovalManager>`。
pub fn register_handlers(registry: &mut HandlerRegistry, manager: Arc<ExecApprovalManager>) {
    {
        let m = manager.clone();
        registry.register("exec.approval.request", move |req| {
            let m = m.clone();
            async move { handle_approval_request(req, m).await }
        });
    }
    {
        let m = manager.clone();
        registry.register("exec.approval.resolve", move |req| {
            let m = m.clone();
            async move { handle_approval_resolve(req, m).await }
        });
    }
    {
        let m = manager.clone();
        registry.register("exec.approvals.get", move |req| {
            let m = m.clone();
            async move { handle_approvals_get(req, m).await }
        });
    }
    {
        let m = manager.clone();
        registry.register("exec.approvals.set", move |req| {
            let m = m.clone();
            async move { handle_approvals_set(req, m).await }
        });
    }
    {
        let m = manager.clone();
        registry.register("exec.approvals.pending", move |req| {
            let m = m.clone();
            async move { handle_approvals_pending(req, m).await }
        });
    }
    {
        let m = manager.clone();
        registry.register("exec.callback.handle", move |req| {
            let m = m.clone();
            async move { handle_callback(req, m).await }
        });
    }
}

/// Handle exec.approval.request
///
/// Creates an approval request and waits for decision or timeout.
async fn handle_approval_request(
    request: JsonRpcRequest,
    manager: Arc<ExecApprovalManager>,
) -> JsonRpcResponse {
    let params: ApprovalRequestParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Run the real command analyzer so the approval record and UI reflect the
    // command's actual risk instead of a hardcoded "ok" verdict.
    let analysis = crate::exec::analyze_shell_command(
        &params.command,
        params.cwd.as_deref().map(std::path::Path::new),
        None,
    );

    // Allowlist pre-gate: a command whose every segment is either a safe
    // read-only bin or covered by the agent's persisted allowlist (written by
    // the allow-always branch below) is auto-approved without prompting —
    // this is what makes the "Allow Always" button actually mean something.
    // The security/ask ladder is deliberately pinned to Allowlist + OnMiss:
    // this RPC's contract has always been "prompt unless trusted", and
    // honoring a configured `security = deny/full` here would silently flip
    // its behavior for existing callers. Config load failure degrades to the
    // legacy prompt flow, never to an auto-approve.
    let auto_allowed = match manager.get_config() {
        Ok(cfg) => {
            let resolved = cfg.config.resolve_for_agent(&params.agent_id);
            let policy = crate::exec::ResolvedExecConfig {
                security: crate::exec::ExecSecurity::Allowlist,
                ask: crate::exec::ExecAsk::OnMiss,
                ask_fallback: crate::exec::ExecSecurity::Deny,
                auto_allow_skills: resolved.auto_allow_skills,
                allowlist: resolved.allowlist,
                skill_allowlist: resolved.skill_allowlist,
            };
            let context = crate::exec::ExecContext {
                agent_id: params.agent_id.clone(),
                session_key: params.session_key.clone(),
                cwd: params.cwd.clone(),
                command: params.command.clone(),
                from_skill: false,
                skill_id: None,
                skill_name: None,
            };
            matches!(
                crate::exec::decide_exec_approval(&policy, &analysis, &context),
                crate::exec::ApprovalDecision::Allow
            )
        }
        Err(e) => {
            tracing::warn!(error = %e, "exec approvals config unavailable — falling back to prompt");
            false
        }
    };

    if auto_allowed {
        return JsonRpcResponse::success(
            request.id,
            json!(ApprovalRequestResponse {
                id: uuid::Uuid::new_v4().to_string(),
                approved: true,
                // No explicit user decision: trust came from the allowlist /
                // safe-bin policy, so `decision` stays empty.
                decision: None,
                timeout: false,
            }),
        );
    }

    // Create approval request
    let approval_request = crate::exec::ApprovalRequest {
        id: uuid::Uuid::new_v4().to_string(),
        command: params.command,
        cwd: params.cwd,
        analysis,
        agent_id: params.agent_id,
        session_key: params.session_key,
        reason: None,
    };

    let timeout_ms = params.timeout_ms.unwrap_or(120_000);
    let record = manager.create(&approval_request, timeout_ms);
    let id = record.id.clone();

    // Wait for decision
    let decision = manager.wait_for_decision(record).await;

    let (approved, timeout) = match decision {
        Some(ApprovalDecisionType::AllowOnce)
        | Some(ApprovalDecisionType::AllowSession)
        | Some(ApprovalDecisionType::AllowAlways) => (true, false),
        Some(ApprovalDecisionType::Deny) => (false, false),
        None => (false, true),
    };

    // Allow-always: persist one allowlist pattern per unique segment
    // executable, so the pre-gate above skips the prompt next time. The
    // manager already clamps allow-always to a session grant for
    // Danger-classified commands, so reaching this branch implies the risk
    // level permits permanent allowlisting.
    if let Some(ApprovalDecisionType::AllowAlways) = decision {
        let mut seen = std::collections::BTreeSet::new();
        for segment in &approval_request.analysis.segments {
            let Some(resolution) = &segment.resolution else {
                continue;
            };
            let pattern = resolution.executable_name.clone();
            if pattern.is_empty() || !seen.insert(pattern.clone()) {
                continue;
            }
            let resolved_path = resolution
                .resolved_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string());
            if let Err(e) = manager.add_to_allowlist(
                &approval_request.agent_id,
                &pattern,
                Some(&approval_request.command),
                resolved_path.as_deref(),
            ) {
                tracing::warn!(
                    error = %e,
                    pattern = %pattern,
                    "failed to persist allow-always grant to allowlist"
                );
            }
        }
    }

    JsonRpcResponse::success(
        request.id,
        json!(ApprovalRequestResponse {
            id,
            approved,
            decision,
            timeout,
        }),
    )
}

/// Handle exec.approval.resolve
///
/// Resolves a pending approval with a decision.
async fn handle_approval_resolve(
    request: JsonRpcRequest,
    manager: Arc<ExecApprovalManager>,
) -> JsonRpcResponse {
    let params: ApprovalResolveParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let resolved = manager.resolve(&params.id, params.decision, params.resolved_by);

    if resolved {
        JsonRpcResponse::success(request.id, json!({ "ok": true }))
    } else {
        JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Approval not found or already resolved: {}", params.id),
        )
    }
}

/// Handle exec.approvals.get
///
/// Returns the current approval config with hash.
async fn handle_approvals_get(
    request: JsonRpcRequest,
    manager: Arc<ExecApprovalManager>,
) -> JsonRpcResponse {
    match manager.get_config() {
        Ok(ConfigWithHash { config, hash }) => {
            JsonRpcResponse::success(request.id, json!(ApprovalsGetResponse { config, hash }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to load config: {e}"),
        ),
    }
}

/// Handle exec.approvals.set
///
/// Updates the approval config with optimistic locking.
async fn handle_approvals_set(
    request: JsonRpcRequest,
    manager: Arc<ExecApprovalManager>,
) -> JsonRpcResponse {
    let params: ApprovalsSetParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match manager.set_config(params.config, &params.base_hash) {
        Ok(new_hash) => JsonRpcResponse::success(request.id, json!({ "hash": new_hash })),
        Err(StorageError::OptimisticLockFailed { base, current }) => JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!(
                "Config changed since last load. Expected hash: {base}, current: {current}. Please reload and retry."
            ),
        ),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Failed to save config: {e}")),
    }
}

/// Handle exec.approvals.pending
///
/// Returns list of pending approvals.
async fn handle_approvals_pending(
    request: JsonRpcRequest,
    manager: Arc<ExecApprovalManager>,
) -> JsonRpcResponse {
    let pending = manager.list_pending();
    JsonRpcResponse::success(request.id, json!(PendingListResponse { pending }))
}

/// Handle exec.callback.handle
///
/// Handles a callback from inline keyboard button click.
/// Parses the callback data and resolves the approval.
async fn handle_callback(
    request: JsonRpcRequest,
    manager: Arc<ExecApprovalManager>,
) -> JsonRpcResponse {
    let params: CallbackHandleParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Parse callback data using ApprovalBridge
    let (approval_id, decision) = match ApprovalBridge::parse_callback(&params.callback_data) {
        Some(parsed) => parsed,
        None => {
            return JsonRpcResponse::success(
                request.id,
                json!(CallbackHandleResponse {
                    handled: false,
                    response_text: Some("Invalid callback data".into()),
                    approval_id: None,
                    decision: None,
                }),
            );
        }
    };

    // Resolve the approval
    let resolved = manager.resolve(&approval_id, decision, Some(params.user_id.clone()));

    if resolved {
        let response_text = ApprovalBridge::decision_response_text(&decision).to_string();

        JsonRpcResponse::success(
            request.id,
            json!(CallbackHandleResponse {
                handled: true,
                response_text: Some(response_text),
                approval_id: Some(approval_id),
                decision: Some(decision),
            }),
        )
    } else {
        JsonRpcResponse::success(
            request.id,
            json!(CallbackHandleResponse {
                handled: false,
                response_text: Some("Approval not found or already resolved".into()),
                approval_id: Some(approval_id),
                decision: None,
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::ExecApprovalsStorage;
    use tempfile::TempDir;

    fn temp_manager() -> (TempDir, Arc<ExecApprovalManager>) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("exec-approvals.json");
        let storage = Arc::new(ExecApprovalsStorage::with_path(path));
        let manager = Arc::new(ExecApprovalManager::with_storage(storage));
        (dir, manager)
    }

    #[tokio::test]
    async fn test_handle_approvals_get() {
        let (_dir, manager) = temp_manager();

        let request = JsonRpcRequest::with_id("exec.approvals.get", None, json!(1));
        let response = handle_approvals_get(request, manager).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert!(result.get("config").is_some());
        assert!(result.get("hash").is_some());
    }

    #[tokio::test]
    async fn test_handle_approvals_set() {
        let (_dir, manager) = temp_manager();

        // First get the current hash
        let get_request = JsonRpcRequest::with_id("exec.approvals.get", None, json!(1));
        let get_response = handle_approvals_get(get_request, manager.clone()).await;
        let hash = get_response.result.unwrap()["hash"]
            .as_str()
            .unwrap()
            .to_string();

        // Now set with the correct base hash
        let set_request = JsonRpcRequest::new(
            "exec.approvals.set",
            Some(json!({
                "config": { "version": 1 },
                "base_hash": hash
            })),
            Some(json!(1)),
        );
        let set_response = handle_approvals_set(set_request, manager).await;

        assert!(set_response.is_success());
    }

    #[tokio::test]
    async fn test_handle_approvals_set_optimistic_lock_failure() {
        let (_dir, manager) = temp_manager();

        // Try to set with wrong hash
        let set_request = JsonRpcRequest::new(
            "exec.approvals.set",
            Some(json!({
                "config": { "version": 1 },
                "base_hash": "wrong-hash"
            })),
            Some(json!(1)),
        );
        let set_response = handle_approvals_set(set_request, manager).await;

        assert!(set_response.is_error());
    }

    #[tokio::test]
    async fn test_handle_approvals_pending() {
        let (_dir, manager) = temp_manager();

        let request = JsonRpcRequest::with_id("exec.approvals.pending", None, json!(1));
        let response = handle_approvals_pending(request, manager).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert!(result.get("pending").is_some());
    }

    #[tokio::test]
    async fn test_handle_approval_resolve_not_found() {
        let (_dir, manager) = temp_manager();

        let request = JsonRpcRequest::new(
            "exec.approval.resolve",
            Some(json!({
                "id": "non-existent-id",
                "decision": "allow-once"
            })),
            Some(json!(1)),
        );
        let response = handle_approval_resolve(request, manager).await;

        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_callback_invalid_data() {
        let (_dir, manager) = temp_manager();

        let request = JsonRpcRequest::new(
            "exec.callback.handle",
            Some(json!({
                "callback_data": "invalid-data",
                "user_id": "user123"
            })),
            Some(json!(1)),
        );
        let response = handle_callback(request, manager).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert_eq!(result["handled"], false);
    }

    #[tokio::test]
    async fn test_handle_callback_approval_not_found() {
        let (_dir, manager) = temp_manager();

        let request = JsonRpcRequest::new(
            "exec.callback.handle",
            Some(json!({
                "callback_data": "approve:non-existent:once",
                "user_id": "user123"
            })),
            Some(json!(1)),
        );
        let response = handle_callback(request, manager).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert_eq!(result["handled"], false);
        assert_eq!(result["approval_id"], "non-existent");
    }

    #[tokio::test]
    async fn safe_bin_command_is_auto_approved_without_prompt() {
        let (_dir, manager) = temp_manager();

        let request = JsonRpcRequest::new(
            "exec.approval.request",
            Some(json!({
                "command": "echo hello",
                "agent_id": "main",
                "session_key": "agent:main:main",
                "timeout_ms": 50
            })),
            Some(json!(1)),
        );
        // No resolver running: the pre-gate must answer before any prompt, so
        // a short timeout never fires.
        let response = handle_approval_request(request, manager.clone()).await;
        assert!(response.is_success());
        let result = response.result.unwrap();
        assert_eq!(result["approved"], true);
        assert_eq!(result["timeout"], false);
        assert!(result["decision"].is_null());
        assert!(
            manager.list_pending().is_empty(),
            "no prompt was registered"
        );
    }

    // `cat` resolves via PATH and `/etc/hosts` is a valid path only on Unix;
    // on Windows the segment resolution fails and the persistence loop has
    // nothing to write, so the scenario is Unix-scoped.
    #[cfg(unix)]
    #[tokio::test]
    async fn allow_always_persists_to_allowlist_and_pre_gates_next_request() {
        let (_dir, manager) = temp_manager();

        let req_json = json!({
            // `cat` with a path argument misses the safe-bin gate, so the
            // first request must prompt; `cat` resolves via PATH on every
            // supported platform.
            "command": "cat /etc/hosts",
            "agent_id": "main",
            "session_key": "agent:main:main",
            "timeout_ms": 10_000
        });

        // Resolve the pending approval with allow-always as soon as it shows.
        let resolver_mgr = manager.clone();
        let resolver = tokio::spawn(async move {
            for _ in 0..200 {
                if let Some(p) = resolver_mgr.list_pending().first() {
                    resolver_mgr.resolve(
                        &p.record.id,
                        ApprovalDecisionType::AllowAlways,
                        Some("tester".to_string()),
                    );
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            panic!("approval prompt never appeared");
        });

        let request = JsonRpcRequest::new(
            "exec.approval.request",
            Some(req_json.clone()),
            Some(json!(1)),
        );
        let response = handle_approval_request(request, manager.clone()).await;
        resolver.await.unwrap();

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert_eq!(result["approved"], true);
        assert_eq!(result["decision"], "allow-always");

        // The grant persisted to the agent's allowlist…
        let cfg = manager.get_config().unwrap();
        let allowlist = cfg
            .config
            .agents
            .get("main")
            .and_then(|a| a.allowlist.clone())
            .unwrap_or_default();
        assert!(
            allowlist.iter().any(|e| e.pattern == "cat"),
            "allow-always must persist the executable pattern: {allowlist:?}"
        );

        // …so the identical command now auto-approves with no prompt.
        let request2 = JsonRpcRequest::new("exec.approval.request", Some(req_json), Some(json!(2)));
        let response2 = handle_approval_request(request2, manager.clone()).await;
        assert!(response2.is_success());
        let result2 = response2.result.unwrap();
        assert_eq!(result2["approved"], true);
        assert!(
            result2["decision"].is_null(),
            "pre-gate, not a user decision"
        );
        assert!(manager.list_pending().is_empty());
    }

    #[tokio::test]
    async fn register_handlers_registers_all_methods() {
        let (_dir, manager) = temp_manager();
        let mut registry = HandlerRegistry::empty();
        register_handlers(&mut registry, manager);
        for m in [
            "exec.approval.request",
            "exec.approval.resolve",
            "exec.approvals.get",
            "exec.approvals.set",
            "exec.approvals.pending",
            "exec.callback.handle",
        ] {
            assert!(registry.has_method(m), "method {m} not registered");
        }
    }
}
