//! Shared provisioning tail for inline-created team members, plus the
//! declared-toolset contract that governs what they may call.
//!
//! Two entry points create member agents: `team_create` (conversational, R8)
//! and template materialization. Their front halves legitimately differ — role
//! template vs prompt addendum, different error enums, different path helpers —
//! but the tail was duplicated verbatim: build `AgentInstanceConfig`, register
//! the runtime instance, persist the `AgentDefinition`.
//!
//! That duplication is exactly why a member's tool surface has to land here and
//! nowhere else. The surface must be written to **both** ends of the chain:
//!
//! ```text
//! AgentDefinition.skills ──from_resolved──▶ AgentInstanceConfig.tool_whitelist
//!                                                      │
//!                                           AgentInstance::is_tool_allowed
//!                                                      │
//!                          run_loop/inner.rs ── retain ──▶ tools the model sees
//! ```
//!
//! The runtime config governs *this* boot; the persisted definition is what
//! `AgentInstanceConfig::from_resolved` rebuilds from after a restart. Writing
//! only one produces a surface that silently widens (or narrows) on restart —
//! and two copies of the tail is two chances to write only one.

use tracing::{info, warn};

use crate::config::agent_manager::AgentManager;
use crate::config::types::agents_def::{AgentDefinition, AgentModelRef};
use crate::gateway::agent_instance::{
    tool_allowed_by, AgentInstance, AgentInstanceConfig, AgentRegistry,
};
use crate::gateway::session_store::SessionStore;
use crate::sync_primitives::Arc;

/// Tools a **worker** member's launch prompt contracts it to call.
///
/// `broadcast::member_prompt` tells every non-leader member to hand work back
/// with `task_submit`, and the team protocol runs on directed messages. A
/// declared toolset that hides these does not produce a narrower member — it
/// produces a member that is instructed to do something it cannot do, and
/// fails silently at the first hand-off. Hence [`validate_toolset`] rejects it
/// at creation instead of letting it brick at run time (P7 fail-fast).
pub const WORKER_ESSENTIAL_TOOLS: &[&str] = &["task_submit", "message_send"];

/// Tools a **leader** member's launch prompt contracts it to call, on top of
/// [`WORKER_ESSENTIAL_TOOLS`]. Mirrors the four numbered duties in
/// `teams::leader_prompt::build`: decompose (`task_create`), delegate
/// (`team_delegate`), then read and accept/reject the artifact
/// (`task_read_artifact` / `task_review`).
pub const LEADER_ESSENTIAL_TOOLS: &[&str] = &[
    "task_create",
    "task_review",
    "task_read_artifact",
    "team_delegate",
    "team_status",
];

/// A member's declared tool surface. Both lists use the trailing-`*` prefix
/// glob understood by [`tool_allowed_by`]; an empty `allowed` means "every
/// tool", which is the pre-existing behaviour every team got before members
/// could declare a surface at all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemberToolset {
    /// Allowlist. Empty (or containing `"*"`) = all tools.
    pub allowed: Vec<String>,
    /// Denylist, applied first and winning over `allowed`.
    pub denied: Vec<String>,
}

impl MemberToolset {
    /// Build from the optional fields carried by a creation request. `None` on
    /// both sides yields the allow-all default.
    #[must_use]
    pub fn new(allowed: Option<Vec<String>>, denied: Option<Vec<String>>) -> Self {
        Self {
            allowed: allowed.unwrap_or_default(),
            denied: denied.unwrap_or_default(),
        }
    }

    /// True when this toolset imposes no restriction at all — the caller
    /// declared nothing, so the persisted definition should keep its fields
    /// unset rather than storing empty vectors.
    #[must_use]
    pub fn is_unrestricted(&self) -> bool {
        self.allowed.is_empty() && self.denied.is_empty()
    }

    /// Project onto the `AgentDefinition` end of the chain:
    /// `(skills, skills_blacklist)`.
    ///
    /// An unrestricted toolset yields `(None, None)` so a member's TOML entry
    /// stays byte-identical to what the pre-toolset code wrote — the runtime
    /// config's empty vectors and an absent `skills` key mean the same thing
    /// (allow all), and writing empty arrays would churn every existing file.
    ///
    /// The projection is deliberately a named function rather than inline
    /// construction: it is the seam where the two ends of the chain could
    /// disagree, and `restart_round_trip_preserves_the_surface` pins it.
    #[must_use]
    pub fn persisted_fields(&self) -> (Option<Vec<String>>, Option<Vec<String>>) {
        if self.is_unrestricted() {
            return (None, None);
        }
        (
            Some(self.allowed.clone()),
            Some(self.denied.clone()).filter(|d| !d.is_empty()),
        )
    }
}

/// Which contract a member is being provisioned under. Decides which essential
/// tools [`validate_toolset`] insists on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberContract {
    /// Runs under `broadcast::member_prompt`'s obey contract.
    Worker,
    /// Runs under `teams::leader_prompt`'s orchestration contract.
    Leader,
}

impl MemberContract {
    fn essentials(self) -> Vec<&'static str> {
        let mut names = WORKER_ESSENTIAL_TOOLS.to_vec();
        if self == Self::Leader {
            names.extend_from_slice(LEADER_ESSENTIAL_TOOLS);
        }
        names
    }
}

/// Reject a declared toolset that would strand the member.
///
/// Returns the missing tool names (in the order declared by the contract) so
/// the caller can name them in its own error type. An unrestricted toolset
/// always passes — this gate only fires once someone opts into narrowing, so
/// every pre-existing team keeps its current surface.
///
/// Glob-aware via [`tool_allowed_by`], so declaring `["task_*", "team_*",
/// "message_send"]` satisfies the leader contract without listing each verb.
///
/// # Errors
/// Returns `Err` with the missing essential tool names when the declared
/// surface hides any of them.
pub fn validate_toolset(
    toolset: &MemberToolset,
    contract: MemberContract,
) -> Result<(), Vec<&'static str>> {
    if toolset.is_unrestricted() {
        return Ok(());
    }
    let missing: Vec<&'static str> = contract
        .essentials()
        .into_iter()
        .filter(|name| !tool_allowed_by(name, &toolset.allowed, &toolset.denied))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing)
    }
}

/// Everything the shared tail needs to materialize one member agent. The
/// caller has already done the parts that legitimately differ between entry
/// points (id validation, identity files, workspace creation, SOUL.md).
pub struct ProvisionRequest {
    pub agent_id: String,
    /// Display name persisted to the TOML registry; `None` falls back to the id.
    pub display_name: Option<String>,
    pub workspace: std::path::PathBuf,
    pub agent_dir: std::path::PathBuf,
    pub model: String,
    pub system_prompt: Option<String>,
    pub toolset: MemberToolset,
}

/// Dependencies shared by both entry points. Held by reference so neither
/// caller has to clone its whole dependency struct.
pub struct ProvisionDeps<'a> {
    pub registry: &'a Arc<AgentRegistry>,
    pub session_store: &'a Arc<dyn SessionStore>,
    pub agent_manager: Option<&'a Arc<AgentManager>>,
}

/// Register the runtime instance and persist its definition.
///
/// The declared toolset is written to both ends of the chain described in the
/// module docs. TOML persistence stays best-effort (a warn, not an error) —
/// same as the two call sites this replaces: runtime registration already
/// succeeded and the team is usable for this boot.
///
/// # Errors
/// Returns the `AgentInstance::new` failure text; callers wrap it in their own
/// error type.
pub async fn register_member_agent(
    req: ProvisionRequest,
    deps: ProvisionDeps<'_>,
) -> Result<(), String> {
    let config = AgentInstanceConfig {
        agent_id: req.agent_id.clone(),
        workspace: req.workspace,
        model: req.model.clone(),
        system_prompt: req.system_prompt,
        agent_dir: req.agent_dir,
        tool_whitelist: req.toolset.allowed.clone(),
        tool_blacklist: req.toolset.denied.clone(),
        ..Default::default()
    };

    let instance = AgentInstance::new(config, Arc::clone(deps.session_store))
        .map_err(|e| format!("AgentInstance::new for `{}` failed: {e}", req.agent_id))?;
    deps.registry.register(instance).await;

    if let Some(manager) = deps.agent_manager {
        let (skills, skills_blacklist) = req.toolset.persisted_fields();
        let def = AgentDefinition {
            id: req.agent_id.clone(),
            name: req.display_name,
            model: Some(AgentModelRef::Legacy(req.model)),
            skills,
            skills_blacklist,
            ..Default::default()
        };
        if let Err(e) = manager.create(def) {
            warn!(
                agent_id = %req.agent_id,
                error = %e,
                "team member provisioning: failed to persist inline agent to TOML \
                 (runtime registration succeeded)"
            );
        }
    }

    info!(agent_id = %req.agent_id, "team member provisioning: created inline agent");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(allowed: &[&str], denied: &[&str]) -> MemberToolset {
        MemberToolset {
            allowed: allowed.iter().map(|s| (*s).to_string()).collect(),
            denied: denied.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    /// The regression that matters most: every team created before this field
    /// existed declares nothing, and must keep seeing every tool.
    #[test]
    fn undeclared_toolset_is_unrestricted_and_always_valid() {
        let none = MemberToolset::new(None, None);
        assert!(none.is_unrestricted());
        assert!(none.allowed.is_empty(), "empty allowlist = allow-all");
        assert!(validate_toolset(&none, MemberContract::Worker).is_ok());
        assert!(validate_toolset(&none, MemberContract::Leader).is_ok());
    }

    #[test]
    fn narrowed_worker_missing_task_submit_is_rejected_and_names_it() {
        let err = validate_toolset(&ts(&["search", "file_read"], &[]), MemberContract::Worker)
            .expect_err("a worker that cannot submit its task must be rejected");
        assert!(err.contains(&"task_submit"), "missing tool named: {err:?}");
        assert!(err.contains(&"message_send"));
    }

    #[test]
    fn leader_needs_more_than_a_worker() {
        let worker_surface = ts(&["task_submit", "message_send"], &[]);
        assert!(validate_toolset(&worker_surface, MemberContract::Worker).is_ok());
        let err = validate_toolset(&worker_surface, MemberContract::Leader)
            .expect_err("the leader contract needs the orchestration verbs");
        assert!(err.contains(&"task_create"));
        assert!(err.contains(&"task_review"));
        assert!(err.contains(&"team_delegate"));
    }

    /// Globs must satisfy the contract — otherwise every caller is forced to
    /// enumerate verbs that will drift as the team toolset grows.
    #[test]
    fn glob_declaration_satisfies_both_contracts() {
        let globbed = ts(&["task_*", "team_*", "message_send", "search"], &[]);
        assert!(validate_toolset(&globbed, MemberContract::Worker).is_ok());
        assert!(validate_toolset(&globbed, MemberContract::Leader).is_ok());
    }

    /// The denylist wins over the allowlist, so it can strand a member just as
    /// effectively as an omission — the gate has to see through it.
    #[test]
    fn denylist_that_hides_an_essential_is_rejected() {
        let err = validate_toolset(&ts(&["*"], &["task_submit"]), MemberContract::Worker)
            .expect_err("a denied essential must be caught, not just an omitted one");
        assert_eq!(err, vec!["task_submit"]);
    }

    /// `["*"]` with no denials is a restriction in form only; it must behave
    /// exactly like declaring nothing.
    #[test]
    fn explicit_star_allowlist_passes_every_contract() {
        assert!(validate_toolset(&ts(&["*"], &[]), MemberContract::Leader).is_ok());
    }

    /// The invariant this whole module exists for: the surface the model sees
    /// on *this* boot (runtime `tool_whitelist` / `tool_blacklist`) and the one
    /// rebuilt after a restart (`AgentDefinition.skills` → `from_resolved` →
    /// `tool_whitelist`) must admit exactly the same tools. Writing one end and
    /// not the other is the failure mode that made the shared tail worth
    /// extracting, and it is invisible until someone restarts the daemon.
    #[test]
    fn restart_round_trip_preserves_the_surface() {
        let probes = [
            "task_submit",
            "task_create",
            "team_status",
            "message_send",
            "bash",
            "code_exec",
            "file_write",
            "search",
        ];
        for toolset in [
            MemberToolset::new(None, None),
            ts(&["task_*", "team_*", "message_send", "search"], &[]),
            ts(&["*"], &["bash", "code_exec", "file_write"]),
            ts(&["task_submit", "message_send"], &["task_submit"]),
        ] {
            // This boot: the vectors handed to `AgentInstanceConfig`.
            let live = |t: &str| tool_allowed_by(t, &toolset.allowed, &toolset.denied);

            // After a restart: `from_resolved` copies the persisted `skills` /
            // `skills_blacklist` straight into the same two config fields.
            let (skills, blacklist) = toolset.persisted_fields();
            let (skills, blacklist) = (skills.unwrap_or_default(), blacklist.unwrap_or_default());
            let rebuilt = |t: &str| tool_allowed_by(t, &skills, &blacklist);

            for probe in probes {
                assert_eq!(
                    live(probe),
                    rebuilt(probe),
                    "surface drifted across restart for `{probe}` with {toolset:?}"
                );
            }
        }
    }

    /// An undeclared member must persist nothing, so existing TOML entries do
    /// not churn and keep meaning "all tools".
    #[test]
    fn unrestricted_toolset_persists_no_fields() {
        assert_eq!(
            MemberToolset::new(None, None).persisted_fields(),
            (None, None)
        );
    }

    /// A pure denylist still has to persist its allow side, otherwise the
    /// restart path would read `skills = None` and drop the restriction.
    #[test]
    fn declared_toolset_persists_both_sides() {
        let (skills, blacklist) = ts(&["task_*", "search"], &["bash"]).persisted_fields();
        assert_eq!(skills, Some(vec!["task_*".into(), "search".into()]));
        assert_eq!(blacklist, Some(vec!["bash".into()]));

        // An allowlist with no denials leaves the blacklist unset rather than
        // writing an empty array.
        let (skills, blacklist) = ts(&["search"], &[]).persisted_fields();
        assert_eq!(skills, Some(vec!["search".into()]));
        assert_eq!(blacklist, None);
    }

    #[test]
    fn unrestricted_predicate_tracks_both_lists() {
        assert!(MemberToolset::new(None, None).is_unrestricted());
        assert!(!MemberToolset::new(Some(vec!["search".into()]), None).is_unrestricted());
        assert!(
            !MemberToolset::new(None, Some(vec!["bash".into()])).is_unrestricted(),
            "a pure denylist is still a declared restriction"
        );
    }
}
