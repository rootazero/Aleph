//! ExecSecurityGate — three-layer defensive gate for shell execution.
//!
//! Intercepts bash/code_exec tool calls in SingleStepExecutor:
//! 1. Risk assessment via SecurityKernel (Blocked/Danger/Caution/Safe)
//! 2. Human approval via ExecApprovalManager for Danger tier
//! 3. Sandbox execution for Safe/Caution (macOS only)
//! 4. SecretMasker applied to all ToolSuccess outputs

use std::sync::Arc;

use serde_json::Value;

use crate::exec::{ExecApprovalManager, SecretMasker, SecurityKernel};
use crate::exec::sandbox::{FallbackPolicy, SandboxManager};

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
}
