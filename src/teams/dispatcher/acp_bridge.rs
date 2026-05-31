//! ACP↔teams bridge.
//!
//! Lets Claude Code / Codex / Gemini CLI / any other ACP harness run as a
//! first-class team member by routing dispatcher `execute_member_task` calls
//! to [`AcpAdapterManager::prompt_named`] when the `agent_id` follows the
//! reserved namespace.
//!
//! ## Naming convention
//!
//! - `acp:<harness>` — harness with the default unnamed session.
//! - `acp:<harness>/<session>` — harness with a named session, so the same
//!   harness can run multiple concurrent personas in one team.
//!
//! Concrete examples:
//! - `acp:claude-code`
//! - `acp:codex/backend`
//! - `acp:gemini/frontend`
//!
//! The harness ID must match one of the entries registered with
//! [`AcpAdapterManager`] (built-in presets cover ~16 popular CLIs; users can
//! register more via `[acp.adapters.<id>]` TOML).
//!
//! ## Why a parallel path (not registry registration)?
//!
//! ACP harnesses are external OS processes with their own session pool, mode
//! switching, and cancel semantics handled by [`AcpAdapterManager`]. Wrapping
//! one as an [`AgentInstance`] would require synthesizing a fake LLM provider,
//! tool registry, and prompt builder — fighting the abstraction. Routing
//! directly to the ACP manager keeps each subsystem speaking its native API.

use crate::acp::manager::AcpAdapterManager;
use crate::sync_primitives::Arc;

use super::runner::{MemberRunOutcome, MemberRunStatus};

/// Reserved prefix that marks a team-member `agent_id` as an ACP harness.
pub const ACP_MEMBER_PREFIX: &str = "acp:";

/// Parsed reference to an ACP harness + optional session name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpMemberRef {
    pub harness_id: String,
    /// `None` = the default unnamed session for this harness.
    pub session_name: Option<String>,
}

impl AcpMemberRef {
    /// Parse `acp:<harness>[/<session>]`. Returns `None` if the input does not
    /// start with [`ACP_MEMBER_PREFIX`] or is otherwise malformed.
    pub fn parse(agent_id: &str) -> Option<Self> {
        let rest = agent_id.strip_prefix(ACP_MEMBER_PREFIX)?;
        if rest.is_empty() {
            return None;
        }
        match rest.split_once('/') {
            None => Some(Self {
                harness_id: rest.to_string(),
                session_name: None,
            }),
            Some((harness, "")) => Some(Self {
                harness_id: harness.to_string(),
                session_name: None,
            }),
            Some((harness, name)) if !harness.is_empty() => Some(Self {
                harness_id: harness.to_string(),
                session_name: Some(name.to_string()),
            }),
            _ => None,
        }
    }

    /// Render back to the canonical `acp:<harness>[/<session>]` form.
    pub fn render(&self) -> String {
        match self.session_name.as_deref() {
            None | Some("") => format!("{ACP_MEMBER_PREFIX}{}", self.harness_id),
            Some(name) => format!("{ACP_MEMBER_PREFIX}{}/{}", self.harness_id, name),
        }
    }
}

/// Execute one task by handing it to the ACP harness identified by
/// `member_ref`. Wraps the manager's result into [`MemberRunOutcome`] so the
/// dispatcher writes uniform state regardless of agent kind.
///
/// `cwd` is the working directory the harness should run in — usually the
/// team's project root or the dispatcher's inherited workspace.
pub async fn execute_acp_member_task(
    acp_manager: &Arc<AcpAdapterManager>,
    member_ref: &AcpMemberRef,
    cwd: &str,
    prompt_text: String,
    timeout_secs: u64,
) -> MemberRunOutcome {
    let prompt_future = acp_manager.prompt_named(
        &member_ref.harness_id,
        &prompt_text,
        cwd,
        member_ref.session_name.as_deref(),
        None, // honour harness default mode
        true, // reuse session — keep context across team turns
        None, // no streaming callback yet
    );

    let timeout = std::time::Duration::from_secs(timeout_secs);
    match tokio::time::timeout(timeout, prompt_future).await {
        Ok(Ok(reply)) => MemberRunOutcome {
            status: MemberRunStatus::Completed,
            reply: Some(reply),
            error: None,
        },
        Ok(Err(e)) => MemberRunOutcome {
            status: MemberRunStatus::Failed,
            reply: None,
            error: Some(format!(
                "ACP harness '{}' failed: {e}",
                member_ref.harness_id
            )),
        },
        Err(_) => MemberRunOutcome {
            status: MemberRunStatus::Timeout,
            reply: None,
            error: Some(format!(
                "ACP harness '{}' timed out after {timeout_secs}s",
                member_ref.harness_id
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bare_harness() {
        let r = AcpMemberRef::parse("acp:claude-code").unwrap();
        assert_eq!(r.harness_id, "claude-code");
        assert_eq!(r.session_name, None);
        assert_eq!(r.render(), "acp:claude-code");
    }

    #[test]
    fn parse_named_session() {
        let r = AcpMemberRef::parse("acp:codex/backend").unwrap();
        assert_eq!(r.harness_id, "codex");
        assert_eq!(r.session_name.as_deref(), Some("backend"));
        assert_eq!(r.render(), "acp:codex/backend");
    }

    #[test]
    fn parse_trailing_slash_drops_session() {
        // `acp:codex/` is treated as `acp:codex` (no session).
        let r = AcpMemberRef::parse("acp:codex/").unwrap();
        assert_eq!(r.harness_id, "codex");
        assert_eq!(r.session_name, None);
    }

    #[test]
    fn parse_rejects_non_acp_prefix() {
        assert_eq!(AcpMemberRef::parse("researcher"), None);
        assert_eq!(AcpMemberRef::parse("native:claude-code"), None);
    }

    #[test]
    fn parse_rejects_empty_harness() {
        assert_eq!(AcpMemberRef::parse("acp:"), None);
        assert_eq!(AcpMemberRef::parse("acp:/named"), None);
    }

    #[test]
    fn render_round_trips() {
        for id in &["acp:claude-code", "acp:codex/backend", "acp:gemini/x"] {
            let parsed = AcpMemberRef::parse(id).unwrap();
            assert_eq!(parsed.render(), *id);
        }
    }
}
