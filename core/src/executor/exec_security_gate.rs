//! ExecSecurityGate — three-layer defensive gate for shell execution.
//!
//! Intercepts bash/code_exec tool calls in SingleStepExecutor:
//! 1. Risk assessment via SecurityKernel (Blocked/Danger/Caution/Safe)
//! 2. Human approval via ExecApprovalManager for Danger tier
//! 3. Sandbox execution for Safe/Caution (macOS only)
//! 4. SecretMasker applied to all ToolSuccess outputs

use std::sync::Arc;

use serde_json::Value;
use tracing::{info, warn};

use crate::exec::{
    analyze_shell_command, ApprovalDecision, ApprovalRequest, ExecApprovalManager,
    ExecContext, RiskLevel, SecretMasker, SecurityKernel, decide_exec_approval,
};
use crate::exec::config::{ExecAsk, ExecSecurity, ResolvedExecConfig};
use crate::exec::manager::DEFAULT_APPROVAL_TIMEOUT_MS;
use crate::exec::sandbox::{FallbackPolicy, SandboxManager};
use crate::exec::socket::ApprovalDecisionType;

/// Decision from pre-execution gate
#[derive(Debug)]
pub enum PreExecDecision {
    /// Proceed with execution
    Allow { use_sandbox: bool },
    /// Block execution with reason
    Block { reason: String },
}

/// Three-layer defensive gate for shell command execution
pub struct ExecSecurityGate {
    security_kernel: SecurityKernel,
    approval_manager: Arc<ExecApprovalManager>,
    sandbox_manager: Option<Arc<SandboxManager>>,
    masker: SecretMasker,
}

impl ExecSecurityGate {
    /// Create a new gate with required approval manager and optional sandbox
    pub fn new(
        approval_manager: Arc<ExecApprovalManager>,
        sandbox_manager: Option<Arc<SandboxManager>>,
    ) -> Self {
        Self {
            security_kernel: SecurityKernel::default(),
            approval_manager,
            sandbox_manager,
            masker: SecretMasker::new(),
        }
    }

    /// Returns true if tool_name is a shell execution tool requiring this gate
    pub fn is_exec_tool(tool_name: &str) -> bool {
        matches!(tool_name, "bash" | "code_exec")
    }

    /// Extract the shell command string from tool arguments.
    ///
    /// Returns None for code_exec tools with non-shell languages
    /// (Python/JavaScript bypass the shell gate).
    pub fn extract_command(tool_name: &str, args: &Value) -> Option<String> {
        match tool_name {
            "bash" => args.get("cmd").and_then(|v| v.as_str()).map(String::from),
            "code_exec" => {
                // Only gate shell language, not Python/JavaScript
                let is_shell = args.get("language")
                    .and_then(|v| v.as_str())
                    .map(|lang| lang == "shell")
                    .unwrap_or(false);
                if is_shell {
                    args.get("code").and_then(|v| v.as_str()).map(String::from)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Pre-execution gate with configurable approval timeout (for testing).
    pub async fn pre_execute_with_timeout(
        &self,
        tool_name: &str,
        args: &Value,
        identity: &aleph_protocol::IdentityContext,
        approval_timeout_ms: u64,
    ) -> PreExecDecision {
        // If we can't extract a command, allow through (e.g., Python/JS)
        let Some(cmd) = Self::extract_command(tool_name, args) else {
            return PreExecDecision::Allow { use_sandbox: false };
        };

        let risk = self.security_kernel.assess(&cmd);

        match risk {
            RiskLevel::Blocked => {
                let assessment = self.security_kernel.assess_detailed(&cmd);
                warn!(
                    cmd = %cmd,
                    "Shell command blocked by SecurityKernel"
                );
                PreExecDecision::Block {
                    reason: format!("Blocked: {} ({})", assessment.reason, cmd),
                }
            }

            RiskLevel::Danger => {
                info!(cmd = %cmd, "Danger-tier command — requesting human approval");
                self.request_approval(&cmd, identity, approval_timeout_ms).await
            }

            RiskLevel::Caution | RiskLevel::Safe => {
                let use_sandbox = self.sandbox_manager.as_ref()
                    .map(|s| s.is_available())
                    .unwrap_or(false);
                info!(cmd = %cmd, risk = ?risk, use_sandbox, "Shell command allowed");
                PreExecDecision::Allow { use_sandbox }
            }
        }
    }

    /// Pre-execute with default 2-minute approval timeout.
    pub async fn pre_execute(
        &self,
        tool_name: &str,
        args: &Value,
        identity: &aleph_protocol::IdentityContext,
    ) -> PreExecDecision {
        self.pre_execute_with_timeout(
            tool_name,
            args,
            identity,
            DEFAULT_APPROVAL_TIMEOUT_MS,
        ).await
    }

    /// Request human approval for a Danger-tier command.
    ///
    /// Builds an ApprovalRequest, registers it with the manager, waits for a decision
    /// (or timeout), and maps the result to PreExecDecision.
    async fn request_approval(
        &self,
        cmd: &str,
        identity: &aleph_protocol::IdentityContext,
        timeout_ms: u64,
    ) -> PreExecDecision {
        // Analyze the command to populate the ApprovalRequest
        let analysis = analyze_shell_command(cmd, None, None);

        // Build a minimal ResolvedExecConfig that triggers NeedApproval for unknown commands
        let config = ResolvedExecConfig {
            security: ExecSecurity::Allowlist,
            ask: ExecAsk::OnMiss,
            ask_fallback: ExecSecurity::Deny,
            auto_allow_skills: false,
            allowlist: vec![],
            skill_allowlist: vec![],
        };

        // Build the exec context from identity fields
        let context = ExecContext {
            // Use identity_id as agent_id (Owner = "owner", Guest = guest UUID)
            agent_id: identity.identity_id.clone(),
            session_key: identity.session_key.clone(),
            cwd: None,
            command: cmd.to_string(),
            from_skill: false,
            skill_id: None,
            skill_name: None,
        };

        // Ask decide_exec_approval; for Danger commands not in allowlist,
        // this returns NeedApproval. Respect Allow/Deny if returned directly.
        let approval_decision = decide_exec_approval(&config, &analysis, &context);

        let request = match approval_decision {
            ApprovalDecision::Allow => {
                info!(cmd = %cmd, "Danger command approved by allowlist policy");
                return PreExecDecision::Allow { use_sandbox: false };
            }
            ApprovalDecision::Deny { reason } => {
                warn!(cmd = %cmd, reason = %reason, "Danger command denied by policy");
                return PreExecDecision::Block { reason };
            }
            ApprovalDecision::NeedApproval { request } => request,
        };

        // Create the approval record (does not yet register for waiting — just builds the record)
        let record = self.approval_manager.create(&request, timeout_ms);
        let record_id = record.id.clone();

        info!(
            cmd = %cmd,
            id = %record_id,
            timeout_ms,
            "Waiting for human approval"
        );

        // Wait for decision or timeout
        let decision = self.approval_manager.wait_for_decision(record).await;

        match decision {
            Some(ApprovalDecisionType::AllowOnce) => {
                info!(cmd = %cmd, id = %record_id, "Danger command approved (once)");
                PreExecDecision::Allow { use_sandbox: false }
            }
            Some(ApprovalDecisionType::AllowAlways) => {
                info!(cmd = %cmd, id = %record_id, "Danger command approved (always)");
                // Add to allowlist for future auto-approval
                let pattern = analysis.segments.first()
                    .and_then(|s| s.resolution.as_ref())
                    .map(|r| {
                        r.resolved_path
                            .as_ref()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_else(|| r.executable_name.clone())
                    });
                if let Some(ref p) = pattern {
                    if let Err(e) = self.approval_manager.add_to_allowlist(&context.agent_id, p) {
                        warn!(cmd = %cmd, pattern = %p, error = %e, "Failed to add to allowlist");
                    } else {
                        info!(cmd = %cmd, pattern = %p, "Added to allowlist");
                    }
                }
                PreExecDecision::Allow { use_sandbox: false }
            }
            Some(ApprovalDecisionType::Deny) => {
                warn!(cmd = %cmd, id = %record_id, "Danger command denied by user");
                PreExecDecision::Block {
                    reason: format!("Denied by user: {}", cmd),
                }
            }
            None => {
                // Timeout — block by default (fail-safe)
                warn!(cmd = %cmd, id = %record_id, "Approval timed out — blocking command");
                PreExecDecision::Block {
                    reason: format!("Approval timed out: {}", cmd),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;
    use crate::exec::ExecApprovalManager;

    fn make_gate() -> ExecSecurityGate {
        let manager = Arc::new(ExecApprovalManager::new());
        ExecSecurityGate::new(manager, None)
    }

    fn make_identity() -> aleph_protocol::IdentityContext {
        aleph_protocol::IdentityContext::owner("session:test".to_string(), "cli".to_string())
    }

    #[test]
    fn test_is_exec_tool() {
        assert!(ExecSecurityGate::is_exec_tool("bash"));
        assert!(ExecSecurityGate::is_exec_tool("code_exec"));
        assert!(!ExecSecurityGate::is_exec_tool("file_ops"));
        assert!(!ExecSecurityGate::is_exec_tool("search"));
        assert!(!ExecSecurityGate::is_exec_tool("translate"));
    }

    #[test]
    fn test_extract_command_bash() {
        let args = json!({"cmd": "ls -la", "timeout": 30});
        let cmd = ExecSecurityGate::extract_command("bash", &args);
        assert_eq!(cmd, Some("ls -la".to_string()));
    }

    #[test]
    fn test_extract_command_code_exec_shell() {
        let args = json!({"language": "shell", "code": "echo hello"});
        let cmd = ExecSecurityGate::extract_command("code_exec", &args);
        assert_eq!(cmd, Some("echo hello".to_string()));
    }

    #[test]
    fn test_extract_command_code_exec_python_bypasses() {
        let args = json!({"language": "python", "code": "print('hello')"});
        let cmd = ExecSecurityGate::extract_command("code_exec", &args);
        assert_eq!(cmd, None);
    }

    #[test]
    fn test_extract_command_missing_field() {
        let args = json!({});
        assert_eq!(ExecSecurityGate::extract_command("bash", &args), None);
    }

    #[tokio::test]
    async fn test_pre_execute_blocked_command() {
        let manager = Arc::new(ExecApprovalManager::new());
        let gate = ExecSecurityGate::new(manager, None);

        let identity = make_identity();
        let args = json!({"cmd": "rm -rf /"});

        let decision = gate.pre_execute("bash", &args, &identity).await;
        assert!(matches!(decision, PreExecDecision::Block { .. }));
        if let PreExecDecision::Block { reason } = decision {
            assert!(reason.contains("Blocked"), "Expected 'Blocked' in reason, got: {}", reason);
        }
    }

    #[tokio::test]
    async fn test_pre_execute_safe_command_allowed() {
        let manager = Arc::new(ExecApprovalManager::new());
        let gate = ExecSecurityGate::new(manager, None);

        let identity = make_identity();
        let args = json!({"cmd": "ls -la"});

        let decision = gate.pre_execute("bash", &args, &identity).await;
        assert!(matches!(decision, PreExecDecision::Allow { .. }));
    }

    #[tokio::test]
    async fn test_pre_execute_caution_command_allowed() {
        let manager = Arc::new(ExecApprovalManager::new());
        let gate = ExecSecurityGate::new(manager, None);

        let identity = make_identity();
        let args = json!({"cmd": "npm install"});

        let decision = gate.pre_execute("bash", &args, &identity).await;
        assert!(matches!(decision, PreExecDecision::Allow { .. }));
    }

    #[tokio::test]
    async fn test_pre_execute_no_command_allows() {
        let manager = Arc::new(ExecApprovalManager::new());
        let gate = ExecSecurityGate::new(manager, None);

        let identity = make_identity();
        let args = json!({"language": "python", "code": "print('hello')"});

        let decision = gate.pre_execute("code_exec", &args, &identity).await;
        assert!(matches!(decision, PreExecDecision::Allow { .. }));
    }

    #[tokio::test]
    async fn test_pre_execute_danger_denied_on_timeout() {
        // With a zero-timeout, no approval is received, so Danger should block
        let manager = Arc::new(ExecApprovalManager::new());
        let gate = ExecSecurityGate::new(manager, None);
        let identity = make_identity();
        // rm -rf ./old_backups is a Danger-tier command (matches ^rm\s+)
        let args = serde_json::json!({"cmd": "rm -rf ./old_backups"});

        // Use 0ms timeout — will immediately timeout → Block
        let decision = gate.pre_execute_with_timeout("bash", &args, &identity, 0).await;
        assert!(
            matches!(decision, PreExecDecision::Block { .. }),
            "Expected Block on timeout, got {:?}", decision
        );
    }

    #[tokio::test]
    async fn test_pre_execute_danger_approved_allow_once() {
        // Simulate a human approving the request
        let manager = Arc::new(ExecApprovalManager::new());
        let gate = ExecSecurityGate::new(manager.clone(), None);
        let identity = make_identity();
        let args = json!({"cmd": "rm -rf ./old_backups"});

        // Spawn a task that approves the first pending request after a short delay
        let manager_clone = manager.clone();
        tokio::spawn(async move {
            // Give the gate time to register the pending request
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            let pending = manager_clone.list_pending();
            if let Some(p) = pending.first() {
                manager_clone.resolve(&p.record.id, ApprovalDecisionType::AllowOnce, None);
            }
        });

        // Use 5s timeout — the spawned task will approve before it expires
        let decision = gate.pre_execute_with_timeout("bash", &args, &identity, 5000).await;
        assert!(
            matches!(decision, PreExecDecision::Allow { .. }),
            "Expected Allow after approval, got {:?}", decision
        );
    }
}
