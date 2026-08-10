//! `workspace_manage`: the conversational face of `workspace.*`.
//!
//! R8 ("everything is a tool"): workspace records existed only as RPCs, reached
//! by the Panel's `/settings/workspaces` page and `aleph workspace …`. "Rename
//! the crypto workspace" or "did I archive that one?" were among the few things
//! an operator could not do by talking.
//!
//! Redline: pure I/O translation (R4), no reasoning (R7). Every verdict —
//! the partition gate, the split between "no such row" and "archived,
//! read-only", whether a create collision has a way back — is
//! [`crate::gateway::agent_env::ops`]'s, the SAME module the RPC handlers call.
//! What is here is argument shape and rendering.
//!
//! # Gating: operator-only, and it is the same principal set the RPC face keeps
//!
//! `workspace.` is admin-gated on the wire
//! ([`crate::gateway::method_admin`]'s `ADMIN_PREFIXES`, since 2026-08-08 — a
//! member had been able to rename and archive an operator's workspaces). This
//! tool is in [`crate::gateway::method_authz`]'s `OPERATOR_TOOLS`, which the
//! dispatch chokepoint (`ScopedToolService`) enforces against
//! `TurnContext::caller_is_operator`. The two gates are different mechanisms,
//! but `"member"` and `"guest"` both fail the operator predicate
//! (`turn_context::role_is_operator`, pinned by `member_role_is_not_operator`),
//! so the tool face is no wider than the wire face.
//!
//! # Why there is no confirmation gate on `archive`
//!
//! `archive` is a soft delete and, since 2026-08-09, reversible: `unarchive`
//! restores the row, the id stays taken in the meantime, and the memory and
//! notes under that id are never touched. It is therefore not in the
//! irreversible tail `CONFIRMATION_REQUIRED_TOOLS` exists for (`vault_store`,
//! `agent_delete`, `team_disband`). Making the `Auto` tier stop for
//! `action = "archive"` specifically would also mean adding a SECOND
//! argument-level rule to the tier system, whose only one
//! (`DESTRUCTIVE_FILE_OPS`) documents itself as the only one; that needs a
//! harder reason than this verb has.
//!
//! Nor is this tool read-only (`READ_ONLY_TOOLS`): it multiplexes reads and
//! writes behind one name, exactly like `file_ops`, so under the `Ask` tier
//! even `list` raises a card. Declaring the whole tool idempotent to spare that
//! would tell the tier a write is a read.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{AlephError, Result};
use crate::gateway::agent_env::ops::{self, WorkspaceOpError};
use crate::gateway::agent_env::AgentEnvStore;
use crate::tools::AlephTool;

/// How this face spells the way back from an archived id collision.
///
/// The tool cannot reach `workspace.unarchive`; telling the model to call it
/// would be advice it cannot act on. Only the archived-collision arm of
/// [`WorkspaceOpError::text`] reads this — a plain collision is byte-identical
/// on both faces by construction, which is what keeps it from being an
/// existence oracle.
const RESTORE_VERB: &str = "action=\"unarchive\"";

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct WorkspaceManageArgs {
    /// One of: "list", "get", "create", "update", "archive", "unarchive".
    pub action: String,
    /// Workspace id — required by every action except "list". URL-safe slug
    /// (`"crypto"`); it is also the agent id this environment belongs to.
    #[serde(default)]
    pub id: Option<String>,
    /// "create": the display name (defaults to the id when omitted).
    /// "update": the new display name; omit to leave it alone.
    #[serde(default)]
    pub name: Option<String>,
    /// Description. Omit on "update" to leave it alone.
    #[serde(default)]
    pub description: Option<String>,
    /// Emoji or icon identifier. Omit on "update" to leave it alone.
    #[serde(default)]
    pub icon: Option<String>,
    /// "list" only: also return archived workspaces (hidden by default).
    #[serde(default)]
    pub include_archived: bool,
}

#[derive(Clone)]
pub struct WorkspaceManageTool {
    /// The SAME `Arc` the gateway handed the RPC handlers, injected from
    /// `BuiltinToolConfig::workspace_manager`.
    ///
    /// Not a store this tool opens for itself, and the reason is not tidiness:
    /// `AgentEnvStore` publishes `WorkspaceChanged` from inside its own
    /// mutating verbs, and only the instance built at startup has the event bus
    /// attached (`AgentEnvStore::with_event_bus`). A hand-rolled
    /// `with_defaults()` would work perfectly and silently stop every open
    /// Panel from refreshing.
    store: Arc<AgentEnvStore>,
}

impl WorkspaceManageTool {
    #[must_use]
    pub const fn new(store: Arc<AgentEnvStore>) -> Self {
        Self { store }
    }

    /// The id an addressed action needs, or a refusal naming the action that
    /// wanted it.
    fn addressed(action: &str, args: &WorkspaceManageArgs) -> Result<String> {
        match args.id.as_deref().map(str::trim) {
            Some(id) if !id.is_empty() => Ok(id.to_string()),
            _ => Err(AlephError::tool(format!(
                "{action}: `id` is required (the workspace id, as shown by action=\"list\")"
            ))),
        }
    }
}

fn refuse(e: &WorkspaceOpError) -> AlephError {
    AlephError::tool(e.text(RESTORE_VERB))
}

#[async_trait]
impl AlephTool for WorkspaceManageTool {
    const NAME: &'static str = "workspace_manage";
    /// Deliberately does NOT restate what the argument schema already carries:
    /// that `update` is a patch, that `include_archived` widens `list`, what
    /// the fields mean. Those ship with the schema, to a caller that by
    /// definition has it. What is here is the part no field doc can hold — what
    /// a workspace record *is*, what it is not, and what `archive` leaves
    /// behind — because that is what decides whether to reach for this tool at
    /// all, and it is what the model would otherwise state wrongly to the user.
    const DESCRIPTION: &'static str = r#"Workspace records — list | get | create | update | archive | unarchive; all take "id" except list.

A workspace id IS an agent id (1:1) and names that agent's memory and notes partition. The record carries display metadata only: it does not set model, tools or system prompt — those live in the agent's profile, which this tool cannot change. create makes the record, not an agent (`agent_create` does that).

archive is a soft delete: the record leaves the default list and goes read-only, but its id stays taken and its memory and notes stay on disk. unarchive puts it back."#;

    type Args = WorkspaceManageArgs;
    type Output = Value;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let store = self.store.as_ref();
        match args.action.trim() {
            "list" => {
                let rows = ops::list(store, args.include_archived)
                    .await
                    .map_err(|e| refuse(&e))?;
                Ok(json!({
                    "action": "list",
                    "count": rows.len(),
                    "include_archived": args.include_archived,
                    "workspaces": rows,
                }))
            }
            "get" => {
                let id = Self::addressed("get", &args)?;
                let ws = ops::get(store, &id).await.map_err(|e| refuse(&e))?;
                Ok(json!({ "action": "get", "workspace": ws }))
            }
            "create" => {
                let id = Self::addressed("create", &args)?;
                // The store defaults an empty name to the id; sending the id
                // rather than "" keeps the two sides from disagreeing about
                // what the default is (see `WorkspaceCreateParams::name`).
                let name = args.name.clone().unwrap_or_else(|| id.clone());
                let ws = ops::create(
                    store,
                    aleph_protocol::workspace::WorkspaceCreateParams {
                        id,
                        name,
                        description: args.description.clone(),
                        icon: args.icon.clone(),
                    },
                )
                .await
                .map_err(|e| refuse(&e))?;
                Ok(json!({ "action": "create", "workspace": ws }))
            }
            "update" => {
                let id = Self::addressed("update", &args)?;
                let ws = ops::update(
                    store,
                    aleph_protocol::workspace::WorkspaceUpdateParams {
                        id,
                        name: args.name.clone(),
                        description: args.description.clone(),
                        icon: args.icon.clone(),
                    },
                )
                .await
                .map_err(|e| refuse(&e))?;
                Ok(json!({ "action": "update", "workspace": ws }))
            }
            "archive" => {
                let id = Self::addressed("archive", &args)?;
                ops::archive(store, &id).await.map_err(|e| refuse(&e))?;
                Ok(json!({
                    "action": "archive",
                    "id": id,
                    // Says what actually happened, so the model does not have
                    // to infer "soft" from the absence of a deletion.
                    "archived": true,
                    "reversible_with": "action=\"unarchive\"",
                }))
            }
            "unarchive" => {
                let id = Self::addressed("unarchive", &args)?;
                let ws = ops::unarchive(store, &id).await.map_err(|e| refuse(&e))?;
                Ok(json!({ "action": "unarchive", "workspace": ws }))
            }
            other => Err(AlephError::tool(format!(
                "action must be one of list, get, create, update, archive, unarchive — got '{other}'"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::agent_env::{AgentEnvStore, AgentEnvStoreConfig};
    use tempfile::TempDir;

    fn tool() -> (WorkspaceManageTool, TempDir) {
        let dir = TempDir::new().expect("tempdir");
        let store = AgentEnvStore::new(AgentEnvStoreConfig {
            db_path: dir.path().join("agent_envs.db"),
            ..Default::default()
        })
        .expect("agent env store");
        store.load_profiles(std::collections::HashMap::new());
        (WorkspaceManageTool::new(Arc::new(store)), dir)
    }

    async fn call(t: &WorkspaceManageTool, v: Value) -> Result<Value> {
        t.call(serde_json::from_value(v).expect("args")).await
    }

    /// The ids a `list` result names.
    ///
    /// Membership, not a count: `AgentEnvStore::init_schema` seeds a `global`
    /// row into every store, so an absolute count asserts on a fixture detail
    /// and goes red the day a second seed appears.
    fn ids(listed: &Value) -> Vec<String> {
        listed["workspaces"]
            .as_array()
            .expect("workspaces array")
            .iter()
            .map(|w| w["id"].as_str().expect("id").to_string())
            .collect()
    }

    /// The round trip an operator actually asks for, end to end: make one, see
    /// it, rename it, put it away, notice it is gone from the list, bring it
    /// back. Written as one test because the interesting assertions are about
    /// what the PREVIOUS verb left behind.
    #[tokio::test]
    async fn the_six_verbs_compose_into_one_workspace_lifecycle() {
        let (t, _dir) = tool();

        let created = call(&t, json!({"action":"create","id":"crypto","name":"Crypto"}))
            .await
            .expect("create");
        assert_eq!(created["workspace"]["id"], "crypto");
        assert_eq!(created["workspace"]["name"], "Crypto");
        assert_eq!(created["workspace"]["is_archived"], false);

        let renamed = call(
            &t,
            json!({"action":"update","id":"crypto","name":"Crypto Research"}),
        )
        .await
        .expect("update");
        assert_eq!(renamed["workspace"]["name"], "Crypto Research");

        // A patch leaves untouched fields alone — the whole point of the
        // Option-per-field shape.
        let icon_only = call(&t, json!({"action":"update","id":"crypto","icon":"🪙"}))
            .await
            .expect("icon patch");
        assert_eq!(icon_only["workspace"]["name"], "Crypto Research");
        assert_eq!(icon_only["workspace"]["icon"], "🪙");

        let archived = call(&t, json!({"action":"archive","id":"crypto"}))
            .await
            .expect("archive");
        assert_eq!(archived["archived"], true);

        let listed = call(&t, json!({"action":"list"})).await.expect("list");
        assert!(
            !ids(&listed).contains(&"crypto".to_string()),
            "archived rows are hidden by default: {listed}"
        );

        let listed_all = call(&t, json!({"action":"list","include_archived":true}))
            .await
            .expect("list all");
        assert!(ids(&listed_all).contains(&"crypto".to_string()));
        assert_eq!(
            listed_all["count"].as_u64().expect("count"),
            listed_all["workspaces"].as_array().expect("rows").len() as u64,
            "`count` must describe the rows actually sent, not a second tally"
        );

        // Readable while archived — `get` reads through, which is what makes
        // the "readable, not writable" rule observable rather than a claim.
        let read = call(&t, json!({"action":"get","id":"crypto"}))
            .await
            .expect("get archived");
        assert_eq!(read["workspace"]["is_archived"], true);

        let restored = call(&t, json!({"action":"unarchive","id":"crypto"}))
            .await
            .expect("unarchive");
        assert_eq!(restored["workspace"]["is_archived"], false);
        assert_eq!(
            restored["workspace"]["name"], "Crypto Research",
            "archiving must not have discarded the metadata"
        );
    }

    /// The tool face must reach the SAME verdicts as the wire face, and say so
    /// in words the model can act on. An archived row refuses a write with
    /// "archived", not "not found" — the caller can read it in the next breath,
    /// so a not-found would be false to their face.
    #[tokio::test]
    async fn a_write_to_an_archived_workspace_says_archived_not_missing() {
        let (t, _dir) = tool();
        call(&t, json!({"action":"create","id":"crypto","name":"Crypto"}))
            .await
            .expect("create");
        call(&t, json!({"action":"archive","id":"crypto"}))
            .await
            .expect("archive");

        let refused = call(&t, json!({"action":"update","id":"crypto","name":"nope"}))
            .await
            .expect_err("archived rows are read-only");
        let text = refused.to_string();
        assert!(text.contains("archived"), "{text}");
        assert!(!text.contains("not found"), "{text}");

        let missing = call(&t, json!({"action":"update","id":"ghost","name":"nope"}))
            .await
            .expect_err("no such row");
        assert!(missing.to_string().contains("not found"));
    }

    /// A collision with an archived holder must name the way back **in this
    /// surface's spelling**. Naming `workspace.unarchive` here would be advice
    /// the model cannot act on — it has no RPC face — and the operator would be
    /// told to pick a different id instead of restoring the row they meant.
    #[tokio::test]
    async fn an_id_held_by_an_archived_workspace_points_at_this_tool_s_own_verb() {
        let (t, _dir) = tool();
        call(&t, json!({"action":"create","id":"crypto","name":"Crypto"}))
            .await
            .expect("create");
        call(&t, json!({"action":"archive","id":"crypto"}))
            .await
            .expect("archive");

        let collision = call(&t, json!({"action":"create","id":"crypto","name":"again"}))
            .await
            .expect_err("id is taken");
        let text = collision.to_string();
        assert!(text.contains(RESTORE_VERB), "{text}");
        assert!(
            !text.contains("workspace.unarchive"),
            "the tool face must not be sent to an RPC method it cannot call: {text}"
        );
    }

    /// An addressed action with no id is a caller mistake, and the refusal has
    /// to name which action wanted it — the model sees one tool, not six, so
    /// "`id` is required" alone does not say which call to fix.
    ///
    /// The list is the full addressed set on purpose: a seventh verb added
    /// without an [`WorkspaceManageTool::addressed`] call would reach `ops`
    /// with an empty id, and `ops` would answer "not found" — a caller mistake
    /// dressed as a fact about the store.
    #[tokio::test]
    async fn every_addressed_action_refuses_a_missing_id_by_name() {
        let (t, _dir) = tool();
        for action in ["get", "create", "update", "archive", "unarchive"] {
            let err = match call(&t, json!({ "action": action })).await {
                Ok(v) => panic!("{action} must require an id, returned {v}"),
                Err(e) => e.to_string(),
            };
            assert!(
                err.contains(&format!("{action}: `id` is required")),
                "the refusal must name the action that wanted the id: {err}"
            );
        }
        // Blank and whitespace-only are the same mistake as absent.
        for id in ["", "   "] {
            let err = match call(&t, json!({ "action": "get", "id": id })).await {
                Ok(v) => panic!("a blank id must be refused, returned {v}"),
                Err(e) => e.to_string(),
            };
            assert!(err.contains("`id` is required"), "{err}");
        }
    }

    /// A store-level refusal has to arrive as words, not as a swallowed
    /// failure. `global` is the seeded row every store carries and the store
    /// refuses to archive it; the model needs to be able to tell the user why
    /// rather than retry.
    #[tokio::test]
    async fn a_store_refusal_reaches_the_model_in_words() {
        let (t, _dir) = tool();
        let err = call(&t, json!({"action":"archive","id":"global"}))
            .await
            .expect_err("the global env cannot be archived")
            .to_string();
        assert!(err.contains("Failed to archive workspace"), "{err}");
        assert!(err.contains("Cannot modify global agent"), "{err}");
    }

    #[tokio::test]
    async fn an_unknown_action_lists_the_ones_that_exist() {
        let (t, _dir) = tool();
        let err = call(&t, json!({"action":"delete","id":"crypto"}))
            .await
            .expect_err("no such action")
            .to_string();
        assert!(err.contains("archive"), "{err}");
        assert!(err.contains("got 'delete'"), "{err}");
    }
}
