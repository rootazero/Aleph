//! Typed errors for the agent-management tools.
//!
//! Each tool in `agent_manage` reports failures through [`AgentManageError`]
//! so callers (and tests) get stable, machine-checkable reasons instead of
//! pre-formatted strings. The conversion to [`crate::error::AlephError`] is
//! the only place that constructs the user-visible message, so two surfaces
//! reporting the same reason stay byte-identical.

use thiserror::Error;

/// Why an agent-management operation refused.
///
/// Distinct variants keep `agent_create` / `agent_delete` / `agent_switch`
/// failure modes machine-checkable: the LLM (and tests) can match on the
/// variant, and the `Display` impl produces a single sentence per variant
/// so the wire output stays consistent across tools.
#[derive(Debug, Error)]
pub enum AgentManageError {
    /// The supplied agent ID is empty or fails the `[a-z0-9][a-z0-9_-]*`
    /// (≤64 chars) grammar.
    #[error("Invalid agent ID: {0}")]
    InvalidId(String),

    /// The agent name and id were both empty (slash-command fast path).
    #[error("Agent name or id is required. Usage: /agent_create <name>")]
    MissingNameOrId,

    /// The target agent does not exist in the runtime registry.
    #[error("Agent '{agent_id}' not found. Available agents: {}", available.join(", "))]
    AgentNotFound {
        agent_id: String,
        /// Sorted list of registered agent IDs (and plugin sub-agents, when
        /// the catalog surface provides them), for actionable error messages.
        available: Vec<String>,
    },

    /// The agent exists but cannot be deleted (built-in or only agent).
    #[error("Cannot delete {0}")]
    ProtectedAgent(String),

    /// The agent ID is already in use — `agent_create` only.
    #[error("Agent '{0}' already exists")]
    AlreadyExists(String),

    /// The home directory cannot be determined (`$HOME` unset, etc.).
    #[error("Cannot determine home directory")]
    NoHomeDir,

    /// An `agent_create` ID was auto-generated from a non-ASCII name and
    /// landed outside the accepted grammar (defensive — should be unreachable
    /// given `generate_agent_id_from_name`).
    #[error("Failed to generate a valid agent ID from name '{0}'")]
    InvalidGeneratedId(String),

    /// The caller invoked an agent-management tool without a `__channel`
    /// context. Mirrors `BindError::EmptyChannel` semantics for the
    /// per-channel tools.
    #[error("No active channel context for this conversation.")]
    NoActiveChannel,

    /// `agent_create` was called with neither `id` nor `name` (slash-command
    /// fast path also turned up empty).
    #[error("Agent name or id is required. Usage: /agent_create <name>")]
    NameAndIdRequired,

    /// I/O failure creating or removing agent files.
    #[error("I/O error: {0}")]
    Io(String),

    /// Underlying persistence store (TOML `AgentManager` or SQLite
    /// `AgentEnvStore`) refused the operation.
    #[error("Storage error: {0}")]
    Store(String),

    /// Required dependency (e.g. `AgentManager` for `agents.list`) was not
    /// wired into the tool at construction.
    #[error("Missing dependency: {0}")]
    MissingDependency(&'static str),
}

impl AgentManageError {
    /// Short, machine-stable reason code (snake_case, wire-stable).
    ///
    /// Surface-agnostic: the Panel RPC layer, the LLM tool output, and the
    /// `aleph-server` CLI all read this through [`crate::error::AlephError`]
    /// adapters so a Panel badge and a model retry decision can speak the
    /// same vocabulary.
    #[must_use]
    pub const fn reason_code(&self) -> &'static str {
        match self {
            Self::InvalidId(_) => "invalid_id",
            Self::MissingNameOrId => "missing_name_or_id",
            Self::AgentNotFound { .. } => "agent_not_found",
            Self::ProtectedAgent(_) => "protected_agent",
            Self::AlreadyExists(_) => "already_exists",
            Self::NoHomeDir => "no_home_dir",
            Self::InvalidGeneratedId(_) => "invalid_generated_id",
            Self::NoActiveChannel => "no_active_channel",
            Self::NameAndIdRequired => "name_and_id_required",
            Self::Io(_) => "io_error",
            Self::Store(_) => "store_error",
            Self::MissingDependency(_) => "missing_dependency",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_codes_are_stable_strings() {
        let cases: Vec<(AgentManageError, &str)> = vec![
            (
                AgentManageError::InvalidId("a".into()),
                "invalid_id",
            ),
            (
                AgentManageError::AgentNotFound {
                    agent_id: "x".into(),
                    available: vec!["y".into()],
                },
                "agent_not_found",
            ),
            (
                AgentManageError::ProtectedAgent("main".into()),
                "protected_agent",
            ),
            (
                AgentManageError::AlreadyExists("foo".into()),
                "already_exists",
            ),
            (AgentManageError::NoHomeDir, "no_home_dir"),
            (AgentManageError::NoActiveChannel, "no_active_channel"),
            (
                AgentManageError::MissingDependency("test"),
                "missing_dependency",
            ),
        ];
        for (err, expected) in cases {
            assert_eq!(err.reason_code(), expected, "variant: {err:?}");
        }
    }

    #[test]
    fn display_includes_available_agents_for_not_found() {
        let err = AgentManageError::AgentNotFound {
            agent_id: "ghost".into(),
            available: vec!["main".into(), "trader".into()],
        };
        let s = err.to_string();
        assert!(s.contains("ghost"));
        assert!(s.contains("main"));
        assert!(s.contains("trader"));
    }
}