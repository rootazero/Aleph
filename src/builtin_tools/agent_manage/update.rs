//! `AgentUpdateTool` — patch an existing agent's editable fields.
//!
//! Two surfaces own an agent's configuration today: the runtime registry
//! (the live `AgentInstance` + its `AgentInstanceConfig`) and the TOML
//! `[[agents.list]]` definition (the persisted row that survives restarts).
//! Both must move together — a runtime-only change is lost on the next
//! `agent_delete` + reload, a TOML-only change is invisible until the next
//! boot. This tool wires both, in the same order, with the same field set.
//!
//! Scope: model / name / description / allowed_users. Skills, allowed_links
//! and tool_permissions are still config-file-only.
//!
//! `allowed_users` joined the list in the §5.17 round-5 identity work, and it
//! is why this tool is in `method_authz::OPERATOR_TOOLS`: it writes the very
//! list the run-start gate reads, so an ungated version would let the people
//! that gate refuses add themselves to it. The rule that puts it here rather
//! than behind a dedicated `agent_grant` is R8 — a security-relevant setting
//! reachable only by hand-editing TOML is worse than one an operator can
//! change by asking.
//!
//! # `allowed_users` is the one field with a real runtime half
//!
//! Round 5 shipped the honest version of a no-op: the tool reported *when* the
//! change would bite because it could not make it bite now. Round 6 makes it
//! bite. [`AgentRegistry::set_allowed_users`] installs the new admission list
//! on the live registry entry — the same object
//! [`build_run_request`](crate::gateway::handlers::agent::build_run_request)
//! reads the gate off — so a REVOCATION is in force on the caller's next turn.
//! The RPC face (`agents.update`, what the Panel calls) goes through the same
//! registry method; two faces of one verb cannot disagree about whether a
//! revocation happened.
//!
//! `name` / `description` / `model` deliberately keep the deferred behaviour.
//! `model` would have to be re-resolved through `agent_resolver`'s
//! drop-detection to be installed live, and `display_name` is cosmetic; neither
//! is worth a second path that can disagree with a reload. So `takes_effect`
//! now reports *per patch* what landed and what did not, instead of one
//! sentence for both.
//!
//! # Two arguments this tool used to accept and never wrote anywhere
//!
//! `system_prompt` and `archetype` were reported in `fields_changed` and
//! persisted nowhere: `AgentPatch` has neither field, `AgentInstanceConfig`'s
//! `system_prompt` is composed from the on-disk `SOUL.md` / `AGENTS.md` rather
//! than from TOML, and `AgentDefinition::archetype` is by its own doc consumed
//! once — `agent_resolver::initialize_agent_identity` writes `SOUL.md` with
//! `write_if_missing`, so an existing agent's archetype has already been spent.
//! Both were cut rather than wired: a config key that accepts a write and
//! changes nothing is the same defect one layer down. Editing an existing
//! agent's persona means editing `SOUL.md` (`agents.files.set` / `agent_files`);
//! changing its archetype means creating an agent with the one you want.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::config::agent_manager::{AgentManager, AgentPatch};
use crate::config::types::agents_def::{AgentIdentity, AgentModelRef};
use crate::error::Result;
use crate::gateway::agent_instance::AgentRegistry;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

use super::error::AgentManageError;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for updating an agent. All fields are optional; absent keys
/// leave the corresponding attribute unchanged. Use explicit `null` to
/// clear a tri-state field (`model` only — see `AgentPatch` for the wire
/// form).
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct AgentUpdateArgs {
    /// ID of the agent to update.
    pub agent_id: String,
    /// New display name (None = leave unchanged).
    #[serde(default)]
    pub name: Option<String>,
    /// New description (None = leave unchanged; empty string explicitly
    /// clears the description).
    #[serde(default)]
    pub description: Option<Option<String>>,
    /// New model reference. Tri-state via the wire form:
    /// - absent → unchanged
    /// - `"model": null` → clear (inherit system default)
    /// - `"model": "claude-x"` → set
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub model: Option<Option<AgentModelRef>>,
    /// Which users (`users.user_id`) may start a run **as** this agent.
    /// Absent = unchanged. An empty list clears the restriction, i.e. makes
    /// the agent reachable by everyone again — the same "empty means
    /// unrestricted" convention `allowed_links` uses.
    #[serde(default)]
    pub allowed_users: Option<Vec<String>>,
}

/// Output from agent update.
#[derive(Debug, Clone, Serialize)]
pub struct AgentUpdateOutput {
    /// The agent ID that was updated.
    pub agent_id: String,
    /// Field-level summary so the model can verify the patch landed.
    /// `"name"` → new value, `"model"` → `"cleared"`/`"set"`, etc.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fields_changed: Vec<String>,
    /// Human-readable status message.
    pub message: String,
    /// When the patch actually starts governing behaviour.
    ///
    /// A patch can now straddle both answers — `allowed_users` is installed on
    /// the live registry, everything else is durable-but-deferred — so this
    /// sentence is composed from *what actually landed on this call*, not from
    /// the section's static classification. The two hint texts still come from
    /// [`ReloadImpact`] rather than literals here, so a reword moves this
    /// surface with the config tools instead of stranding it.
    ///
    /// The downgrade direction is the point, and it mirrors
    /// [`config::live_apply::classify_verified`](crate::config::live_apply::classify_verified):
    /// a `Live` claim is only made when the registry write returned `true`. A
    /// REVOCATION that reports success and does not take effect is the one
    /// failure this whole field exists to prevent, and "the runtime was there
    /// to receive it" is not something a static table can know.
    pub takes_effect: String,
}

/// Serde helper: tri-state `Option<Option<T>>` — absent → `None`, explicit
/// `null` → `Some(None)`, value → `Some(Some(value))`. Mirrors the one in
/// `config::agent_manager` so the LLM-facing wire form matches the
/// persistence seam byte-for-byte.
fn deserialize_double_option<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<Option<AgentModelRef>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<AgentModelRef>::deserialize(deserializer)?))
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that patches an existing agent's editable metadata.
///
/// Requires both the runtime registry (for in-memory `AgentInstanceConfig`
/// updates) **and** the TOML `AgentManager` (for persistence across restarts).
/// Either being absent turns the tool into a single-surface write — the
/// non-fatal path is wired the same way `agent_create` does it.
#[derive(Clone)]
pub struct AgentUpdateTool {
    registry: Arc<AgentRegistry>,
    agent_manager: Option<Arc<AgentManager>>,
}

impl AgentUpdateTool {
    #[must_use]
    pub const fn new(
        registry: Arc<AgentRegistry>,
        agent_manager: Option<Arc<AgentManager>>,
    ) -> Self {
        Self {
            registry,
            agent_manager,
        }
    }
}

#[async_trait]
impl AlephTool for AgentUpdateTool {
    const NAME: &'static str = "agent_update";
    // Held under the previous revision's byte count on purpose: this round
    // ADDS a sentence (the live half) and REMOVES two arguments, and
    // `CATALOG_DESCRIPTION_CEILING_BYTES` is a one-way ratchet, so the new
    // sentence is paid for by dropping prose the parameter schema already
    // ships (the `agent_id` / `null` spelling-out, the "say that" coaching).
    const DESCRIPTION: &'static str =
        "Patch an existing agent's name, description, model or allowed_users. Only supplied \
         keys change; explicit `null` clears one. IDs are the identity key — to rename, \
         delete and re-create; to edit persona text, write SOUL.md via the agent-files \
         tools. `allowed_users` lists who may run AS this agent and delegate to it; empty \
         means everyone, and it binds on the next turn. The rest lands in config.toml and \
         bites after restart — relay `takes_effect` verbatim.";

    type Args = AgentUpdateArgs;
    type Output = AgentUpdateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(agent_id = %args.agent_id, "Agent update requested");

        // 1. Existence check via `contains` — never instantiate a lazy entry
        //    just to read its current config (and never fail an update because
        //    a lazy entry chose not to inflate).
        if !self.registry.contains(&args.agent_id).await {
            let mut available = self.registry.list().await;
            available.sort();
            return Err(AgentManageError::AgentNotFound {
                agent_id: args.agent_id.clone(),
                available,
            }
            .into());
        }

        // 2. Record what the caller asked to change, then apply the half that
        //    can be applied live. `collect_changed_fields` is pure — it answers
        //    "what did this call touch", which is not the same question as
        //    "what is in force now", and conflating the two is how the previous
        //    version reported a revocation it had not performed.
        let mut fields_changed = Vec::new();
        collect_changed_fields(&args, &mut fields_changed);

        //    The runtime half. Only `allowed_users` has one: it is installed on
        //    the registry entry the run-start gate reads, so a revocation binds
        //    on the refused caller's next turn. Deliberately NOT via
        //    `registry.get` — that instantiates a lazy entry, and building a
        //    whole `AgentInstance` just to edit one list is work an admission
        //    change should not do (nor should it decide, as a side effect,
        //    which agents are hot).
        let mut applied_live: Vec<&'static str> = Vec::new();
        if let Some(users) = args.allowed_users.as_ref() {
            let normalized = crate::config::types::normalized_allowed_users(users);
            if self
                .registry
                .set_allowed_users(&args.agent_id, normalized)
                .await
            {
                applied_live.push("allowed_users");
            }
            // Authority-change audit (round-5 ⑦): same verb as the RPC face
            // (`agents.update`), reached through the tool. The actor resolver
            // is `ambient_actor()` — this runs inside a spawned run where
            // `CALLER_USER` is dead.
            if let Some(log) = crate::security::audit::global() {
                log.log(crate::security::audit::AuditEntry::authority_change(
                    crate::gateway::visibility::ambient_actor(),
                    format!(
                        "agent_update: allowed_users {} → [{}]",
                        args.agent_id,
                        users.join(", ")
                    ),
                )).await;
            }
        }

        // 3. Persist the patch to TOML when the manager is wired. We always
        //    build an `AgentPatch` and only call `update` when at least one
        //    field was requested, so a model that calls `agent_update({})`
        //    for "I think you should know there's nothing to update" still
        //    gets a clean no-op rather than a TOML roundtrip.
        if self.agent_manager.is_some() {
            let patch = build_toml_patch(&args);
            if patch_has_changes(&patch) {
                if let Some(ref mgr) = self.agent_manager {
                    mgr.update(&args.agent_id, patch).map_err(|e| {
                        AgentManageError::Store(format!(
                            "Failed to persist update for '{}': {}",
                            args.agent_id, e
                        ))
                    })?;
                }
            }
        }

        let message = if fields_changed.is_empty() {
            format!(
                "Agent '{}' unchanged — no editable fields were supplied.",
                args.agent_id
            )
        } else {
            format!(
                "Agent '{}' updated ({}).",
                args.agent_id,
                fields_changed.join(", ")
            )
        };

        info!(
            agent_id = %args.agent_id,
            changed = ?fields_changed,
            "Agent update complete"
        );

        let takes_effect = takes_effect_sentence(&applied_live, &fields_changed);

        Ok(AgentUpdateOutput {
            agent_id: args.agent_id,
            fields_changed,
            message,
            takes_effect,
        })
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Name every field this call touched, in the vocabulary the model reads back.
///
/// Pure, and deliberately separate from the apply: it answers "what did this
/// call ask for", which [`takes_effect_sentence`] then splits against "what is
/// in force now". Every name produced here must be recognised by
/// [`is_live_field`], or a field would silently be described as deferred.
fn collect_changed_fields(args: &AgentUpdateArgs, fields_changed: &mut Vec<String>) {
    if args.name.is_some() {
        fields_changed.push("name".to_string());
    }
    if args.description.is_some() {
        fields_changed.push("description".to_string());
    }
    if let Some(Some(_)) = args.model.as_ref() {
        fields_changed.push("model".to_string());
    } else if matches!(args.model, Some(None)) {
        fields_changed.push("model:cleared".to_string());
    }
    if let Some(users) = args.allowed_users.as_ref() {
        fields_changed.push(if users.is_empty() {
            "allowed_users:cleared".to_string()
        } else {
            "allowed_users".to_string()
        });
    }
}

/// Whether a `fields_changed` entry names the field `live` reports as applied.
///
/// The `:cleared` suffix is a report decoration, not a different field —
/// matching on the base name is what keeps a cleared `allowed_users` (the
/// revocation-adjacent case: "everyone may use this agent again") from being
/// described as deferred while the registry has already been rewritten.
fn is_live_field(entry: &str, live: &[&'static str]) -> bool {
    let base = entry.split_once(':').map_or(entry, |(base, _)| base);
    live.contains(&base)
}

/// Compose the honest "when does this bite" sentence for one patch.
///
/// Three shapes, because a patch can now straddle both answers:
/// everything landed live, nothing did, or some did. The wording of each half
/// is [`ReloadImpact`]'s so this cannot drift from the config tools; only the
/// *split* is decided here, and it is decided from what the registry write
/// actually returned rather than from the section table — a section is the kind
/// of thing that applies live, an individual call is where you learn whether
/// the runtime was there to receive it.
fn takes_effect_sentence(live: &[&'static str], fields_changed: &[String]) -> String {
    let deferred_hint = crate::config::ReloadImpact::classify("agents").agent_hint();
    if live.is_empty() {
        return deferred_hint.to_string();
    }
    let (applied, deferred): (Vec<&str>, Vec<&str>) = fields_changed
        .iter()
        .map(String::as_str)
        .partition(|f| is_live_field(f, live));
    if deferred.is_empty() {
        return crate::config::ReloadImpact::Live.agent_hint().to_string();
    }
    format!(
        "{} — {} The rest ({}) — {}",
        applied.join(", "),
        crate::config::ReloadImpact::Live.agent_hint(),
        deferred.join(", "),
        deferred_hint
    )
}

/// Translate `AgentUpdateArgs` into the TOML `AgentPatch` used by
/// `AgentManager::update`. The description-to-identity mapping is the same
/// one `agent_create` uses, so a model that updates the description then
/// re-reads it via `agent_info` sees the same value the Panel editor
/// displays.
fn build_toml_patch(args: &AgentUpdateArgs) -> AgentPatch {
    let identity = args.description.as_ref().map(|maybe_desc| AgentIdentity {
        description: maybe_desc.clone(),
        ..Default::default()
    });

    AgentPatch {
        name: args.name.clone(),
        identity,
        skills: None,
        skills_blacklist: None,
        subagents: None,
        allowed_links: None,
        allowed_users: args.allowed_users.clone(),
        // The wire form is `Option<Option<AgentModelRef>>`: absent = no
        // change, Some(None) = clear, Some(Some(_)) = set. `AgentPatch`
        // already uses the same tri-state, so a straight move is correct.
        model: args.model.clone(),
    }
}

/// `AgentPatch` doesn't expose "any field set?" — every field is
/// `Option<...>` and `Default::default()` is indistinguishable from
/// "user supplied no patch at all". Walk the wire fields directly so a
/// `{}-shaped` call returns `false` here.
fn patch_has_changes(patch: &AgentPatch) -> bool {
    // Every field this tool can actually set must be named here. The list was
    // safe to keep short while `skills` / `skills_blacklist` / `subagents` /
    // `allowed_links` were hard-coded `None` above; `allowed_users` is the
    // first one that is not, and omitting it would skip the TOML write
    // silently while `fields_changed` still reported the change.
    patch.name.is_some()
        || patch.identity.is_some()
        || patch.model.is_some()
        || patch.allowed_users.is_some()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin_tools::agent_manage::test_utils;
    use crate::tools::AlephTool;

    #[test]
    fn test_update_tool_definition() {
        let registry = Arc::new(AgentRegistry::new());
        let tool = AgentUpdateTool::new(registry, None);
        let def = AlephTool::definition(&tool);
        assert_eq!(def.name, "agent_update");
        assert!(!def.requires_confirmation);
    }

    #[tokio::test]
    async fn unknown_agent_errors_with_available_list() {
        let registry = Arc::new(AgentRegistry::new());
        let (instance, _sm, _t) = test_utils::instance("trader");
        registry.register(instance).await;
        let tool = AgentUpdateTool::new(registry, None);

        let err = tool
            .call(AgentUpdateArgs {
                agent_id: "ghost".into(),
                ..Default::default()
            })
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not found"));
        assert!(msg.contains("trader"), "available list missing: {msg}");
    }

    #[tokio::test]
    async fn empty_patch_reports_unchanged() {
        let registry = Arc::new(AgentRegistry::new());
        let (instance, _sm, _t) = test_utils::instance("trader");
        registry.register(instance).await;
        let tool = AgentUpdateTool::new(registry, None);

        let out = tool
            .call(AgentUpdateArgs {
                agent_id: "trader".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(out.fields_changed.is_empty());
        assert!(out.message.contains("unchanged"));
    }

    #[tokio::test]
    async fn name_change_records_field() {
        let registry = Arc::new(AgentRegistry::new());
        let (instance, _sm, _t) = test_utils::instance("trader");
        registry.register(instance).await;
        let tool = AgentUpdateTool::new(registry, None);

        let out = tool
            .call(AgentUpdateArgs {
                agent_id: "trader".into(),
                name: Some("Quant Bot".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(out.fields_changed, vec!["name".to_string()]);
    }

    #[tokio::test]
    async fn description_clear_records_field() {
        // `description = Some(None)` should be reported as a clear
        // (so the LLM sees that an explicit clear happened).
        let registry = Arc::new(AgentRegistry::new());
        let (instance, _sm, _t) = test_utils::instance("trader");
        registry.register(instance).await;
        let tool = AgentUpdateTool::new(registry, None);

        let out = tool
            .call(AgentUpdateArgs {
                agent_id: "trader".into(),
                description: Some(None),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(out.fields_changed.contains(&"description".to_string()));
    }

    /// The whole point of the runtime half: after this call the registry — the
    /// object `build_run_request` reads the gate off — carries the new list,
    /// with no restart and without instantiating the agent.
    ///
    /// Asserted through `get_allowed_users`, which reads the same field from
    /// whichever entry variant is present. Dropping the `set_allowed_users`
    /// call leaves every `fields_changed` / TOML assertion in this file green
    /// while a revocation stops binding, which is exactly how the previous
    /// version shipped.
    #[tokio::test]
    async fn a_revocation_binds_on_the_live_registry_without_a_restart() {
        let registry = Arc::new(AgentRegistry::new());
        let (instance, _sm, _t) = test_utils::instance("ops");
        registry.register(instance).await;
        let tool = AgentUpdateTool::new(Arc::clone(&registry), None);

        tool.call(AgentUpdateArgs {
            agent_id: "ops".into(),
            allowed_users: Some(vec!["u-alice".into(), "u-bob".into()]),
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!(
            registry.get_allowed_users("ops").await,
            Some(Some(vec!["u-alice".to_string(), "u-bob".to_string()])),
            "the grant must be readable off the registry immediately"
        );

        // The revocation — the direction that must never be reported without
        // being performed.
        tool.call(AgentUpdateArgs {
            agent_id: "ops".into(),
            allowed_users: Some(vec!["u-alice".into()]),
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!(
            registry.get_allowed_users("ops").await,
            Some(Some(vec!["u-alice".to_string()])),
            "u-bob must be gone from the live list, not merely from config.toml"
        );
        assert!(
            !crate::config::types::agent_admits_user(
                registry.get_allowed_users("ops").await.flatten().as_deref(),
                Some("u-bob")
            ),
            "the admission rule must now refuse the revoked user"
        );
    }

    /// Clearing collapses to the storage form (`None`), so a live agent and the
    /// `config.toml` it would be reloaded from cannot disagree about which of
    /// `None` / `Some([])` is on disk.
    #[tokio::test]
    async fn clearing_installs_the_storage_form_on_the_registry() {
        let registry = Arc::new(AgentRegistry::new());
        let (instance, _sm, _t) = test_utils::instance("ops");
        registry.register(instance).await;
        let tool = AgentUpdateTool::new(Arc::clone(&registry), None);

        tool.call(AgentUpdateArgs {
            agent_id: "ops".into(),
            allowed_users: Some(vec!["u-alice".into()]),
            ..Default::default()
        })
        .await
        .unwrap();
        tool.call(AgentUpdateArgs {
            agent_id: "ops".into(),
            allowed_users: Some(vec![]),
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!(
            registry.get_allowed_users("ops").await,
            Some(None),
            "an empty patch list must reach the registry as None, the same \
             shape a reload from TOML produces"
        );
    }

    /// `allowed_users` is the list the run-start gate reads, so the two ways
    /// this tool could silently lose it are both pinned here: a `fields_changed`
    /// entry the model can verify, and — the one that actually loses data —
    /// `patch_has_changes`, which decides whether the TOML write happens at
    /// all. Every other field this tool sets was already named there; this was
    /// the first that was not, and a miss would have reported success while
    /// writing nothing.
    #[tokio::test]
    async fn allowed_users_reaches_both_the_report_and_the_toml_patch() {
        let registry = Arc::new(AgentRegistry::new());
        let (instance, _sm, _t) = test_utils::instance("ops");
        registry.register(instance).await;
        let tool = AgentUpdateTool::new(registry, None);

        let args = AgentUpdateArgs {
            agent_id: "ops".into(),
            allowed_users: Some(vec!["u-alice".into()]),
            ..Default::default()
        };
        let out = tool.call(args.clone()).await.unwrap();
        assert!(out.fields_changed.contains(&"allowed_users".to_string()));

        let patch = build_toml_patch(&args);
        assert_eq!(
            patch.allowed_users.as_deref(),
            Some(&["u-alice".to_string()][..])
        );
        assert!(
            patch_has_changes(&patch),
            "an allowed_users-only patch must still trigger the TOML write"
        );
    }

    /// Clearing is a distinct report from setting: "everyone may use this
    /// agent again" is the answer an operator most needs to see spelled out,
    /// and the TOML side removes the key rather than writing `[]`.
    #[tokio::test]
    async fn clearing_allowed_users_is_reported_as_cleared() {
        let registry = Arc::new(AgentRegistry::new());
        let (instance, _sm, _t) = test_utils::instance("ops");
        registry.register(instance).await;
        let tool = AgentUpdateTool::new(registry, None);

        let out = tool
            .call(AgentUpdateArgs {
                agent_id: "ops".into(),
                allowed_users: Some(vec![]),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(out
            .fields_changed
            .contains(&"allowed_users:cleared".to_string()));
    }

    /// An `allowed_users`-only patch is wholly live, and says so in
    /// `ReloadImpact`'s own words rather than a literal here.
    #[tokio::test]
    async fn an_admission_only_patch_reports_itself_live() {
        let registry = Arc::new(AgentRegistry::new());
        let (instance, _sm, _t) = test_utils::instance("ops");
        registry.register(instance).await;
        let tool = AgentUpdateTool::new(registry, None);

        let out = tool
            .call(AgentUpdateArgs {
                agent_id: "ops".into(),
                allowed_users: Some(vec!["u-alice".into()]),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            out.takes_effect,
            crate::config::ReloadImpact::Live.agent_hint(),
            "the notice must come from the one source, not a literal"
        );
    }

    /// A patch with no live half keeps the deferred sentence verbatim — the
    /// `model`/`name`/`description` behaviour is unchanged, and claiming
    /// otherwise because a sibling field applied is the mirror of the bug the
    /// live half was added to fix.
    #[tokio::test]
    async fn a_patch_with_no_live_half_still_says_restart() {
        let registry = Arc::new(AgentRegistry::new());
        let (instance, _sm, _t) = test_utils::instance("ops");
        registry.register(instance).await;
        let tool = AgentUpdateTool::new(registry, None);

        let out = tool
            .call(AgentUpdateArgs {
                agent_id: "ops".into(),
                name: Some("Ops Bot".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            out.takes_effect,
            crate::config::ReloadImpact::classify("agents").agent_hint()
        );
    }

    /// A straddling patch must name both halves. One sentence for both is what
    /// the previous version had, and it was wrong in whichever direction the
    /// operator happened to care about.
    #[tokio::test]
    async fn a_mixed_patch_names_which_half_is_in_force() {
        let registry = Arc::new(AgentRegistry::new());
        let (instance, _sm, _t) = test_utils::instance("ops");
        registry.register(instance).await;
        let tool = AgentUpdateTool::new(registry, None);

        let out = tool
            .call(AgentUpdateArgs {
                agent_id: "ops".into(),
                name: Some("Ops Bot".into()),
                allowed_users: Some(vec!["u-alice".into()]),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(
            out.takes_effect.contains("allowed_users") && out.takes_effect.contains("name"),
            "both halves must be named: {}",
            out.takes_effect
        );
        assert!(out
            .takes_effect
            .contains(crate::config::ReloadImpact::Live.agent_hint()));
        assert!(out
            .takes_effect
            .contains(crate::config::ReloadImpact::classify("agents").agent_hint()));
    }

    /// The `:cleared` decoration must not read as a different field. Without
    /// [`is_live_field`]'s base-name match, clearing the list would be
    /// described as needing a restart while the registry had already been
    /// rewritten — a lie in the safe-sounding direction, which is how it would
    /// survive review.
    #[tokio::test]
    async fn a_cleared_admission_list_is_still_reported_live() {
        let registry = Arc::new(AgentRegistry::new());
        let (instance, _sm, _t) = test_utils::instance("ops");
        registry.register(instance).await;
        let tool = AgentUpdateTool::new(registry, None);

        let out = tool
            .call(AgentUpdateArgs {
                agent_id: "ops".into(),
                allowed_users: Some(vec![]),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            out.takes_effect,
            crate::config::ReloadImpact::Live.agent_hint(),
            "clearing is an admission change like any other: {}",
            out.takes_effect
        );
    }

    /// An unregistered agent gets no `Live` claim. Unreachable through `call`
    /// (the existence check refuses first), which is exactly why the honesty
    /// is pinned at the composer instead of relying on that check staying put.
    #[test]
    fn nothing_applied_means_nothing_is_claimed_live() {
        assert_eq!(
            takes_effect_sentence(&[], &["allowed_users".to_string()]),
            crate::config::ReloadImpact::classify("agents").agent_hint()
        );
    }
}
