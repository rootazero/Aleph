//! Approval decision logic for command execution.

use super::allowlist::match_allowlist;
use super::analysis::{CommandAnalysis, CommandSegment};
use super::config::{ExecAsk, ExecSecurity, ResolvedExecConfig};

/// Default safe binaries (read-only operations)
pub const DEFAULT_SAFE_BINS: &[&str] = &[
    "jq", "grep", "cut", "sort", "uniq", "head", "tail", "tr", "wc", "cat", "echo", "pwd", "ls",
    "which", "env", "date", "true", "false", "test", "basename", "dirname", "realpath", "stat",
    "file", "diff", "comm", "tee", "xargs", "seq", "printf",
];

/// Decision result for command execution
#[derive(Debug, Clone)]
pub enum ApprovalDecision {
    /// Allow execution
    Allow,
    /// Deny execution with reason
    Deny { reason: String },
    /// Need user approval
    NeedApproval { request: ApprovalRequest },
}

/// Request for user approval
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    /// Unique request ID
    pub id: String,
    /// Full command string
    pub command: String,
    /// Working directory
    pub cwd: Option<String>,
    /// Command analysis result
    pub analysis: CommandAnalysis,
    /// Agent ID
    pub agent_id: String,
    /// Session key
    pub session_key: String,
}

/// Context for execution decision
#[derive(Debug, Clone)]
pub struct ExecContext {
    pub agent_id: String,
    pub session_key: String,
    pub cwd: Option<String>,
    pub command: String,
    /// Whether this command is from a skill
    pub from_skill: bool,
}

/// Decide whether to allow command execution
pub fn decide_exec_approval(
    config: &ResolvedExecConfig,
    analysis: &CommandAnalysis,
    context: &ExecContext,
) -> ApprovalDecision {
    // 1. Analysis must be OK
    if !analysis.ok {
        return ApprovalDecision::Deny {
            reason: analysis
                .reason
                .clone()
                .unwrap_or_else(|| "command parse error".into()),
        };
    }

    // 2. Check security level
    match config.security {
        ExecSecurity::Deny => {
            return ApprovalDecision::Deny {
                reason: "command execution denied by security policy".into(),
            };
        }
        ExecSecurity::Full => {
            return ApprovalDecision::Allow;
        }
        ExecSecurity::Allowlist => { /* continue checking */ }
    }

    // 3. Auto-allow skills if configured
    if config.auto_allow_skills && context.from_skill {
        return ApprovalDecision::Allow;
    }

    // 4. Check all segments
    for segment in &analysis.segments {
        match check_segment(config, segment) {
            SegmentDecision::Allow => continue,
            SegmentDecision::NeedApproval => {
                // Check ask policy
                if config.ask == ExecAsk::Off {
                    return apply_fallback(config.ask_fallback);
                }
                return ApprovalDecision::NeedApproval {
                    request: build_approval_request(analysis, context),
                };
            }
            SegmentDecision::Deny(reason) => {
                return ApprovalDecision::Deny { reason };
            }
        }
    }

    // 5. Check if ask=always
    if config.ask == ExecAsk::Always {
        return ApprovalDecision::NeedApproval {
            request: build_approval_request(analysis, context),
        };
    }

    ApprovalDecision::Allow
}

/// Decision for a single segment
enum SegmentDecision {
    Allow,
    NeedApproval,
    Deny(String),
}

/// Check a single command segment
fn check_segment(config: &ResolvedExecConfig, segment: &CommandSegment) -> SegmentDecision {
    let Some(resolution) = &segment.resolution else {
        return SegmentDecision::NeedApproval;
    };

    // Check safe bins (with argument restrictions)
    if is_safe_bin_usage(&resolution.executable_name, &segment.argv) {
        return SegmentDecision::Allow;
    }

    // Check allowlist
    if match_allowlist(&config.allowlist, resolution).is_some() {
        return SegmentDecision::Allow;
    }

    SegmentDecision::NeedApproval
}

/// Check if command uses a safe binary without dangerous arguments
fn is_safe_bin_usage(executable: &str, argv: &[String]) -> bool {
    if !DEFAULT_SAFE_BINS
        .iter()
        .any(|b| b.eq_ignore_ascii_case(executable))
    {
        return false;
    }

    // Arguments must not contain file paths or redirections
    for arg in argv.iter().skip(1) {
        // Skip flags
        if arg.starts_with('-') {
            continue;
        }
        // Disallow paths
        if arg.contains('/') || arg.contains('\\') {
            return false;
        }
    }

    true
}

/// Apply fallback security level
fn apply_fallback(fallback: ExecSecurity) -> ApprovalDecision {
    match fallback {
        ExecSecurity::Full => ApprovalDecision::Allow,
        _ => ApprovalDecision::Deny {
            reason: "approval required but ask is disabled".into(),
        },
    }
}

/// Build an approval request
fn build_approval_request(analysis: &CommandAnalysis, context: &ExecContext) -> ApprovalRequest {
    ApprovalRequest {
        id: uuid::Uuid::new_v4().to_string(),
        command: context.command.clone(),
        cwd: context.cwd.clone(),
        analysis: analysis.clone(),
        agent_id: context.agent_id.clone(),
        session_key: context.session_key.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::parser::analyze_shell_command;

    fn default_config() -> ResolvedExecConfig {
        ResolvedExecConfig {
            security: ExecSecurity::Allowlist,
            ask: ExecAsk::OnMiss,
            ask_fallback: ExecSecurity::Deny,
            auto_allow_skills: false,
            allowlist: vec![],
        }
    }

    fn context(command: &str) -> ExecContext {
        ExecContext {
            agent_id: "main".into(),
            session_key: "agent:main:main".into(),
            cwd: None,
            command: command.into(),
            from_skill: false,
        }
    }

    #[test]
    fn test_deny_policy() {
        let config = ResolvedExecConfig {
            security: ExecSecurity::Deny,
            ..default_config()
        };
        let analysis = analyze_shell_command("ls", None, None);
        let decision = decide_exec_approval(&config, &analysis, &context("ls"));

        assert!(matches!(decision, ApprovalDecision::Deny { .. }));
    }

    #[test]
    fn test_full_policy() {
        let config = ResolvedExecConfig {
            security: ExecSecurity::Full,
            ..default_config()
        };
        let analysis = analyze_shell_command("rm -rf /", None, None);
        let decision = decide_exec_approval(&config, &analysis, &context("rm -rf /"));

        assert!(matches!(decision, ApprovalDecision::Allow));
    }

    #[test]
    fn test_safe_bin_allowed() {
        let config = default_config();
        let analysis = analyze_shell_command("echo hello", None, None);
        let decision = decide_exec_approval(&config, &analysis, &context("echo hello"));

        assert!(matches!(decision, ApprovalDecision::Allow));
    }

    #[test]
    fn test_safe_bin_with_path_needs_approval() {
        let config = default_config();
        let analysis = analyze_shell_command("cat /etc/passwd", None, None);
        let decision = decide_exec_approval(&config, &analysis, &context("cat /etc/passwd"));

        assert!(matches!(decision, ApprovalDecision::NeedApproval { .. }));
    }

    #[test]
    fn test_unknown_command_needs_approval() {
        let config = default_config();
        let analysis = analyze_shell_command("npm install", None, None);
        let decision = decide_exec_approval(&config, &analysis, &context("npm install"));

        assert!(matches!(decision, ApprovalDecision::NeedApproval { .. }));
    }

    #[test]
    fn test_ask_off_uses_fallback() {
        let config = ResolvedExecConfig {
            ask: ExecAsk::Off,
            ask_fallback: ExecSecurity::Deny,
            ..default_config()
        };
        let analysis = analyze_shell_command("npm install", None, None);
        let decision = decide_exec_approval(&config, &analysis, &context("npm install"));

        assert!(matches!(decision, ApprovalDecision::Deny { .. }));
    }

    #[test]
    fn test_auto_allow_skills() {
        let config = ResolvedExecConfig {
            auto_allow_skills: true,
            ..default_config()
        };
        let analysis = analyze_shell_command("npm install", None, None);
        let mut ctx = context("npm install");
        ctx.from_skill = true;
        let decision = decide_exec_approval(&config, &analysis, &ctx);

        assert!(matches!(decision, ApprovalDecision::Allow));
    }

    #[test]
    fn test_is_safe_bin_usage() {
        assert!(is_safe_bin_usage("echo", &["echo".into(), "hello".into()]));
        assert!(is_safe_bin_usage("ls", &["ls".into(), "-la".into()]));
        assert!(!is_safe_bin_usage("cat", &["cat".into(), "/etc/passwd".into()]));
        assert!(!is_safe_bin_usage("npm", &["npm".into(), "install".into()]));
    }
}
