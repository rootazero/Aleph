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
}

#[async_trait]
impl SandboxBeforeHook for SecurityKernelHook {
    async fn before(&self, ctx: SandboxHookContext<'_>) -> SandboxHookResult {
        // The same text, folded the same way, as the command-policy hook one
        // slot along in the chain. This used to be a second, private
        // reconstruction that (a) stopped at the args, so a script piped in as
        // stdin (`bash -s`) was invisible to every operator pattern, and
        // (b) skipped normalisation entirely, so `r\m` walked past a pattern
        // written as `rm`. Two functions answering "what command is about to
        // run" is one too many: an operator who writes an explicit
        // `custom_blocked` rule is entitled to the same de-obfuscation the
        // built-in rules get, and the only way to keep that true is to read the
        // same string.
        let text = crate::sandbox::command_policy::command_text(ctx.command);
        let command = crate::sandbox::command_policy::normalize::normalize_for_matching(&text);
        match self.kernel.assess_custom(&command) {
            Some(RiskLevel::Blocked) => {
                // Same durable trail as the built-in ruleset next to it: a
                // refusal the operator configured themselves is exactly as
                // worth reconstructing after an incident, and a second audit
                // producer would only be a second thing to keep in step.
                crate::sandbox::command_policy::record_policy_decision(
                    true,
                    ctx.command,
                    &["custom_blocked".to_string()],
                ).await;
                SandboxHookResult::Deny {
                    reason: format!(
                        "command blocked by custom shell-security pattern ([security].custom_blocked): {}",
                        ctx.command.program
                    ),
                }
            }
            Some(RiskLevel::Danger) => {
                tracing::warn!(
                    target: "shell_security",
                    tool = %ctx.command.tool_name,
                    program = %ctx.command.program,
                    "command matched a custom danger pattern (advisory; allowed)"
                );
                crate::sandbox::command_policy::record_policy_decision(
                    false,
                    ctx.command,
                    &["custom_danger".to_string()],
                ).await;
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
            tool_name: "bash".into(),
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
            mask_patterns: Vec::new(),
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
        let ctx = SandboxHookContext::new(&command);
        assert!(matches!(
            hook.before(ctx).await,
            SandboxHookResult::Deny { .. }
        ));
    }

    #[tokio::test]
    async fn allows_unmatched_command() {
        let hook = hook_with(&[r"^secret_tool\b"], &[]);
        let command = cmd("ls", &["-la"]);
        let ctx = SandboxHookContext::new(&command);
        assert!(matches!(hook.before(ctx).await, SandboxHookResult::Allow));
    }

    #[tokio::test]
    async fn danger_pattern_is_advisory_only() {
        let hook = hook_with(&[], &[r"^deploy\b"]);
        let command = cmd("deploy", &["prod"]);
        let ctx = SandboxHookContext::new(&command);
        assert!(matches!(hook.before(ctx).await, SandboxHookResult::Allow));
    }

    /// An operator who writes an explicit `custom_blocked` rule gets the same
    /// de-obfuscation the built-in rules get. Before this hook read the shared
    /// normalised text, a pattern written as `secret_tool` was defeated by
    /// `secret_to\ol` — the shell runs the same program either way.
    #[tokio::test]
    async fn custom_patterns_see_the_de_obfuscated_command() {
        let hook = hook_with(&[r"secret_tool\b"], &[]);
        for args in [
            vec!["-c", r"secret_to\ol --leak"],
            vec!["-c", "secret_to''ol --leak"],
            vec!["-c", "secret_to'o'l --leak"],
            vec!["-c", "secret_tool${IFS}--leak"],
        ] {
            let command = cmd("bash", &args);
            let ctx = SandboxHookContext::new(&command);
            assert!(
                matches!(hook.before(ctx).await, SandboxHookResult::Deny { .. }),
                "must deny: {args:?}"
            );
        }
    }

    /// The reconstruction must include the stdin payload, which is where the
    /// `bash -s` large-script path puts the whole program. A hook that stops at
    /// the args cannot see any of it.
    #[tokio::test]
    async fn custom_patterns_see_a_script_arriving_on_stdin() {
        let hook = hook_with(&[r"secret_tool\b"], &[]);
        let mut command = cmd("bash", &["-s"]);
        command.stdin = Some(b"set -e\nsecret_tool --leak\n".to_vec());
        let ctx = SandboxHookContext::new(&command);
        assert!(matches!(
            hook.before(ctx).await,
            SandboxHookResult::Deny { .. }
        ));
    }

    #[tokio::test]
    async fn no_custom_patterns_is_inert() {
        let hook =
            SecurityKernelHook::new(SecurityKernel::from_config(&ShellSecurityConfig::default()));
        // Even a built-in "blocked"-looking command is allowed: this hook only
        // consults custom patterns, so default configs see no behavior change.
        let command = cmd("rm", &["-rf", "/"]);
        let ctx = SandboxHookContext::new(&command);
        assert!(matches!(hook.before(ctx).await, SandboxHookResult::Allow));
    }
}
