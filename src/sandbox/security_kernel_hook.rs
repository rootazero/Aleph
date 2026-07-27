//! Advisory shell-security hook backed by the `SecurityKernel`.
//!
//! Bridges the Panel-managed `[security]` (`ShellSecurityConfig`) custom
//! patterns into the live sandbox execution path. It is a **purely additive**
//! layer on top of the command-policy hard filter:
//!
//! - a `custom_blocked` match vetoes execution (the user explicitly blocked it);
//! - a `custom_danger` match logs an advisory warning but allows execution
//!   (real danger gating stays with the command-policy hook / approval flow);
//! - built-in kernel patterns are intentionally NOT consulted here, so this
//!   hook never changes behavior for users who configure no custom patterns.

use async_trait::async_trait;

use crate::exec::{RiskLevel, SecurityKernel};
use crate::sandbox::hooks::{SandboxBeforeHook, SandboxHookContext, SandboxHookResult};

/// `SandboxBeforeHook` that consults a `SecurityKernel`'s custom patterns.
pub struct SecurityKernelHook {
    kernel: SecurityKernel,
}

impl SecurityKernelHook {
    #[must_use]
    pub const fn new(kernel: SecurityKernel) -> Self {
        Self { kernel }
    }

    /// Reconstruct the inspected command line from the sandbox command.
    fn command_line(ctx: &SandboxHookContext<'_>) -> String {
        let mut line = ctx.command.program.clone();
        for arg in &ctx.command.args {
            line.push(' ');
            line.push_str(arg);
        }
        line
    }
}

#[async_trait]
impl SandboxBeforeHook for SecurityKernelHook {
    fn name(&self) -> &'static str {
        "security_kernel"
    }

    async fn before(&self, ctx: SandboxHookContext<'_>) -> SandboxHookResult {
        let command = Self::command_line(&ctx);
        match self.kernel.assess_custom(&command) {
            Some(RiskLevel::Blocked) => SandboxHookResult::Deny {
                reason: format!(
                    "command blocked by custom shell-security pattern ([security].custom_blocked): {}",
                    ctx.command.program
                ),
            },
            Some(RiskLevel::Danger) => {
                tracing::warn!(
                    target: "shell_security",
                    tool = ctx.tool_name,
                    program = %ctx.command.program,
                    "command matched a custom danger pattern (advisory; allowed)"
                );
                SandboxHookResult::Allow
            }
            _ => SandboxHookResult::Allow,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{CustomRiskPattern, ShellSecurityConfig};
    use crate::routing::session_key::SessionKey;
    use crate::sandbox::capabilities::SandboxCapabilities;
    use crate::sandbox::command::SandboxCommand;
    use std::collections::HashMap;

    fn cmd(program: &str, args: &[&str]) -> SandboxCommand {
        SandboxCommand {
            session_id: SessionKey::ephemeral("test"),
            program: program.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            env: HashMap::new(),
            stdin: None,
            cwd: None,
            capabilities: SandboxCapabilities::default(),
            timeout: None,
        }
    }

    fn hook_with(blocked: &[&str], danger: &[&str]) -> SecurityKernelHook {
        let config = ShellSecurityConfig {
            enable_custom_patterns: true,
            custom_blocked: blocked
                .iter()
                .map(|p| CustomRiskPattern {
                    pattern: p.to_string(),
                    reason: None,
                })
                .collect(),
            custom_danger: danger
                .iter()
                .map(|p| CustomRiskPattern {
                    pattern: p.to_string(),
                    reason: None,
                })
                .collect(),
        };
        SecurityKernelHook::new(SecurityKernel::from_config(&config))
    }

    #[tokio::test]
    async fn denies_custom_blocked_pattern() {
        let hook = hook_with(&[r"^secret_tool\b"], &[]);
        let command = cmd("secret_tool", &["--leak"]);
        let ctx = SandboxHookContext::new("bash", &command);
        assert!(matches!(
            hook.before(ctx).await,
            SandboxHookResult::Deny { .. }
        ));
    }

    #[tokio::test]
    async fn allows_unmatched_command() {
        let hook = hook_with(&[r"^secret_tool\b"], &[]);
        let command = cmd("ls", &["-la"]);
        let ctx = SandboxHookContext::new("bash", &command);
        assert!(matches!(hook.before(ctx).await, SandboxHookResult::Allow));
    }

    #[tokio::test]
    async fn danger_pattern_is_advisory_only() {
        let hook = hook_with(&[], &[r"^deploy\b"]);
        let command = cmd("deploy", &["prod"]);
        let ctx = SandboxHookContext::new("bash", &command);
        assert!(matches!(hook.before(ctx).await, SandboxHookResult::Allow));
    }

    #[tokio::test]
    async fn no_custom_patterns_is_inert() {
        let hook =
            SecurityKernelHook::new(SecurityKernel::from_config(&ShellSecurityConfig::default()));
        // Even a built-in "blocked"-looking command is allowed: this hook only
        // consults custom patterns, so default configs see no behavior change.
        let command = cmd("rm", &["-rf", "/"]);
        let ctx = SandboxHookContext::new("bash", &command);
        assert!(matches!(hook.before(ctx).await, SandboxHookResult::Allow));
    }
}
