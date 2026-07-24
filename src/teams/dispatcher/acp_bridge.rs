//! ACP↔teams naming bridge.
//!
//! Lets Claude Code / Codex / Gemini CLI / any other ACP harness run as a
//! first-class team member. This module owns only the `agent_id` **naming
//! convention** that marks a member as an ACP harness; the actual task
//! execution lives in [`super::runner::execute_member_task`], which routes the
//! structured `MemberDispatchTarget::AcpSession` variant through the gateway's
//! `AcpAdapterManager`.
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
//! The harness ID must match one of the entries registered with the
//! `AcpAdapterManager` (built-in presets cover ~16 popular CLIs; users can
//! register more via `[acp.adapters.<id>]` TOML).
//!
//! ## Why a parallel path (not registry registration)?
//!
//! ACP harnesses are external OS processes with their own session pool, mode
//! switching, and cancel semantics handled by the `AcpAdapterManager`. Wrapping
//! one as an `AgentInstance` would require synthesizing a fake LLM provider,
//! tool registry, and prompt builder — fighting the abstraction. Routing
//! directly to the ACP manager keeps each subsystem speaking its native API.

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
    ///
    /// Long-form roster ids (`acp:<harness>:<cwd>[:<name>]`, minted by
    /// [`crate::teams::types::acp_member_id`] and shown in rosters/status)
    /// are deliberately rejected: harness ids are `AcpAdapterManager` registry
    /// keys and never contain `':'`, so a colon in the harness segment means
    /// the caller pasted a displayed long-form id. Accepting it would persist
    /// a garbage `harness_id` (e.g. `claude_code:`) that only fails later at
    /// dispatch — fail fast at add time instead (use `team_acp_member` for
    /// members that need an explicit cwd/session).
    #[must_use]
    pub fn parse(agent_id: &str) -> Option<Self> {
        let rest = agent_id.strip_prefix(ACP_MEMBER_PREFIX)?;
        if rest.is_empty() {
            return None;
        }
        // Reject ':' anywhere in the harness segment (everything before the
        // first '/', or the whole rest when there is no session part).
        let harness_end = rest.find('/').unwrap_or(rest.len());
        if rest[..harness_end].contains(':') {
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
    #[must_use]
    pub fn render(&self) -> String {
        match self.session_name.as_deref() {
            None | Some("") => format!("{ACP_MEMBER_PREFIX}{}", self.harness_id),
            Some(name) => format!("{ACP_MEMBER_PREFIX}{}/{}", self.harness_id, name),
        }
    }
}

// Note: task execution for ACP members lives in
// [`super::runner::execute_member_task`] (which routes the structured
// `MemberDispatchTarget::AcpSession` variant through the gateway's
// `AcpAdapterManager`). This module owns only the `agent_id` naming
// convention (`AcpMemberRef`) so team-creation tools can recognise and
// validate ACP members before the dispatcher ever sees the task.

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
    fn parse_rejects_long_form_roster_ids() {
        // `acp_member_id` mints the long form `acp:<harness>:<cwd>[:<name>]`
        // (displayed in rosters/status). Accepting it here used to decompose
        // `acp:claude_code:/work/proj` into harness_id "claude_code:" (trailing
        // colon) — persisted as routing truth and only failing later at
        // dispatch. Harness ids are registry keys and never contain ':', so
        // any ':' in the harness segment must fail-fast at parse time.
        assert_eq!(AcpMemberRef::parse("acp:claude_code:/work/proj"), None);
        assert_eq!(
            AcpMemberRef::parse("acp:claude_code:/work/proj:backend"),
            None
        );
        // Colon before the first '/' is still a harness-segment colon.
        assert_eq!(AcpMemberRef::parse("acp:codex:extra/backend"), None);
        // A colon in the SESSION name is not the harness segment — unchanged.
        let r = AcpMemberRef::parse("acp:codex/name:v2").unwrap();
        assert_eq!(r.harness_id, "codex");
        assert_eq!(r.session_name.as_deref(), Some("name:v2"));
    }

    #[test]
    fn render_round_trips() {
        for id in &["acp:claude-code", "acp:codex/backend", "acp:gemini/x"] {
            let parsed = AcpMemberRef::parse(id).unwrap();
            assert_eq!(parsed.render(), *id);
        }
    }
}
