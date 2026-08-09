//! Workspace RPC Handlers
//!
//! Handlers for workspace management: create, list, get, update, archive.
//! Channel agent binding: `channels.set_agent`, agents.bindings.
//! All handlers delegate to `AgentEnvStore` (SQLite-backed).

use aleph_protocol::workspace::{
    WorkspaceCreateParams, WorkspaceDetail, WorkspaceListParams, WorkspaceRef, WorkspaceRow,
    WorkspaceUpdateParams,
};
use serde::Deserialize;
use serde_json::json;

use super::super::protocol::{
    JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, PERMISSION_DENIED,
    RESOURCE_NOT_FOUND,
};
use super::parse_params;
use crate::gateway::agent_env::{AgentEnv, AgentEnvError, AgentEnvStore};
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::event_bus::GatewayEventBus;
use crate::sync_primitives::Arc;

// ============================================================================
// Projection
// ============================================================================

/// Project the stored [`AgentEnv`] onto the detail shape this family promises.
///
/// Every read here used to serialize the whole `AgentEnv`, which put four
/// fields on the wire that Aleph never reads back: `env_vars`,
/// `allowed_tools`, `system_prompt_override` and `default_model`.
/// [`crate::gateway::agent_env::ActiveAgentEnv`] — the struct that actually
/// flows through the execution pipeline — carries `agent_id`, `profile`,
/// `memory_filter` and `agent_env_path`, and drops all four at the resolution
/// boundary. There was no writer for them either — none in any version, not
/// just this one — so what shipped was a permanently-empty configuration
/// surface that looked settable: the failure mode is a caller who sets
/// `default_model` on a workspace and gets silence. Per R10 a channel with zero
/// consumers is CUT, not connected. The four were dropped from `AgentEnv` and
/// from the `agent_envs` table on the same day this projection landed, so today
/// they are not reachable from here even by accident; this function's job is
/// now the general one, of being the single place that decides what a client
/// sees.
///
/// Projecting also makes the contract enforceable in the direction that
/// matters. `aleph-cli` cannot depend on `alephcore`, so the only guard for
/// this wire is the shared type plus assertions on this side; parsing proves
/// the response is a *superset* of the contract, never that it is equal to it.
/// Building the response FROM the contract type closes that gap by
/// construction — a new `AgentEnv` field cannot leak onto the wire by being
/// added, only by being added here on purpose.
fn detail_of(ws: &AgentEnv) -> WorkspaceDetail {
    WorkspaceDetail {
        id: ws.id.clone(),
        name: ws.name.clone(),
        description: ws.description.clone(),
        icon: ws.icon.clone(),
        profile: ws.profile.clone(),
        created_at: ws.created_at,
        last_active_at: ws.last_active_at,
        is_archived: ws.is_archived,
    }
}

/// The list twin of [`detail_of`], onto the narrower row shape.
///
/// Deliberately not `detail_of(..)` minus fields: the two projections answer
/// different questions and [`WorkspaceRow`] documents why it is a second one.
fn row_of(ws: &AgentEnv) -> WorkspaceRow {
    WorkspaceRow {
        id: ws.id.clone(),
        name: ws.name.clone(),
        description: ws.description.clone(),
        created_at: ws.created_at,
        is_archived: ws.is_archived,
    }
}

// ============================================================================
// Create
// ============================================================================

/// The one wording `workspace.create` has for "that id is not available".
///
/// A function rather than the same `format!` at two call sites, because the two
/// sites are required to agree **byte for byte**: [`handle_create`] refuses a
/// partition-invisible id with this shape so the refusal is indistinguishable
/// from a genuine collision, and two literals that merely look alike are one
/// reword away from becoming an existence oracle. Built from the real
/// [`AgentEnvError::AlreadyExists`] value for the same reason — the store's
/// wording is not copied here, it is produced.
fn id_taken(id: &str) -> String {
    format!(
        "Failed to create workspace: {}",
        AgentEnvError::AlreadyExists(id.to_string())
    )
}

/// Create a new workspace
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"workspace.create","params":{"id":"crypto","name":"Crypto Trading"},"id":1}
/// ```
///
/// The param type is [`aleph_protocol::workspace::WorkspaceCreateParams`], the
/// same struct the CLI constructs — see that module for why the shape is not
/// declared here, and for how this method came to reject its only client on
/// every call while every test stayed green.
///
/// P1 partition isolation on the WRITE side, with exactly the coverage
/// boundary [`handle_list`] documents and no more: a workspace id is a
/// user-chosen name that encodes no owner and `agent_envs` has no owner
/// column, so an ordinary id (`"crypto"`) passes for every caller who reaches
/// this handler. What the check does buy is the composed-id half — without it
/// a caller can plant `main__u-alice`, a row that then shows up in ALICE's
/// filtered `workspace.list` under a name and description he chose. Reads were
/// gated first; a write into a partition you cannot read is the strictly worse
/// half.
///
/// An earlier wording here said the planted row carried "attacker-supplied
/// `env_vars` / `system_prompt_override` / `allowed_tools`". That was never
/// true in two separate ways: this method accepts none of those three, and as
/// of 2026-08-09 they are not on the wire at all (see [`detail_of`] for why
/// they have no writer and no reader). The predicate is kept on the honest
/// justification — a row planted in someone else's namespace is bad on its own
/// terms — because a guard resting on a false premise is one refactor away
/// from being deleted as vacuous.
///
/// Since 2026-08-08 the reachable caller set is operator-only — the whole
/// `workspace.` family is admin-gated ([`handle_list`]'s doc has the finding
/// that made that the right fix rather than an owner column). The predicate
/// stays because it is not vacuous for an operator either.
///
/// The denial reuses this method's own "that id is not available" shape
/// ([`id_taken`]), produced from the REAL
/// [`crate::gateway::agent_env::AgentEnvError::AlreadyExists`] value so it stays
/// byte-identical to a genuine collision instead of hard-coding a copy of the
/// store's wording.
///
/// Since 2026-08-09 a genuine collision has a second form: when the id is held
/// by an **archived** workspace the message names [`handle_unarchive`], because
/// "already exists" is true but unactionable and sends the operator off to pick
/// a different id. The partition denial is unaffected — it returns above,
/// without reading the store, so it always carries the plain shape and cannot
/// become an existence oracle. `create_names_unarchive_when_the_id_is_held_by_
/// an_archived_workspace` asserts both halves.
pub async fn handle_create(
    request: JsonRpcRequest,
    workspace_manager: Arc<AgentEnvStore>,
) -> JsonRpcResponse {
    let params: WorkspaceCreateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    if !crate::gateway::visibility::partition_visible(&params.id) {
        return JsonRpcResponse::error(request.id, INTERNAL_ERROR, id_taken(&params.id));
    }

    match workspace_manager
        .create(&params.id, "default", params.description.as_deref())
        .await
    {
        Ok(mut ws) => {
            // `AgentEnvStore::create` takes neither name nor icon, so they are
            // a second write.
            ws.name = params.name;
            ws.icon = params.icon.clone();
            let persisted = workspace_manager
                .update(&params.id, Some(&ws.name), None, params.icon.as_deref())
                .await;

            // Answer with the row that is ON DISK, not the one just asked for.
            //
            // Until 2026-08-09 this returned the locally-mutated `ws`, which
            // made the response a **statement of intent** wearing the shape of
            // an observation: the write above only `warn!`s on failure, so a
            // workspace whose name never persisted was reported back carrying
            // that name, and the caller learned otherwise on their next `get`.
            // That is the same "said ok, wrote nothing" shape `handle_update`
            // and `AgentEnvStore::update` were fixed for a day earlier.
            //
            // It also silently answered a different question about time.
            // `create` builds `created_at` from `Utc::now()` while the store
            // persists `timestamp()` — whole seconds — so the response carried
            // sub-second precision that no later read would ever reproduce.
            //
            // A failed read-back falls back to the constructed value: the row
            // does exist (the INSERT succeeded), and refusing the whole call
            // would be a worse lie than an imprecise success.
            let ws = match (persisted, workspace_manager.get(&params.id).await) {
                (Ok(_), Ok(Some(stored))) => stored,
                (persisted, read_back) => {
                    tracing::warn!(
                        workspace = %params.id,
                        persist_error = ?persisted.err(),
                        read_back = ?read_back.map(|r| r.is_some()),
                        "workspace.create: could not confirm the stored row; \
                         answering with the requested values"
                    );
                    ws
                }
            };

            JsonRpcResponse::success(
                request.id,
                json!({
                    "ok": true,
                    "workspace": detail_of(&ws),
                }),
            )
        }
        // Which collision is it? Both are "that id is not available", but only
        // one of them has a way out, and `unarchive` is not guessable from the
        // words "already exists" — an operator who archived this workspace
        // yesterday would read the bare collision as "someone else has the
        // name" and pick a different id, stranding the row they meant to reuse.
        //
        // The probe only ever upgrades a refusal that is ALREADY TRUE into a
        // more specific one — the same shape [`handle_update`] uses — so a
        // probe that cannot answer falls back to what it was refining rather
        // than inventing something.
        //
        // Not an existence oracle: a partition-invisible id returns from the
        // check above and never reaches the store, so it always gets the plain
        // shape; and for an id the caller CAN see, `workspace.get` reports
        // `is_archived` outright.
        Err(AgentEnvError::AlreadyExists(taken)) => {
            let archived = matches!(
                workspace_manager.get_including_archived(&taken).await,
                Ok(Some(ws)) if ws.is_archived
            );
            let message = if archived {
                format!(
                    "{} — it is archived, not gone. Restore it with \
                     `workspace.unarchive`; an archived workspace keeps its id, \
                     and its memory and notes are still on disk under that id.",
                    id_taken(&taken)
                )
            } else {
                id_taken(&taken)
            };
            JsonRpcResponse::error(request.id, INTERNAL_ERROR, message)
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to create workspace: {e}"),
        ),
    }
}

// ============================================================================
// List
// ============================================================================

/// List all workspaces
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"workspace.list","id":1}
/// ```
///
/// # P1 partition isolation — and what it does NOT cover
///
/// Each row is filtered by `visibility::partition_visible` on its id, the same
/// predicate the `memory.*`/`graph.*` family uses, so an id composed with the
/// partition grammar (`<base>__u-alice`) is invisible to everyone else.
///
/// This used to open by noting that a row "serializes as an [`AgentEnv`], which
/// carries `env_vars`, `system_prompt_override` and `allowed_tools`" — i.e. the
/// leak this filter prevents was about those fields. It no longer serializes as
/// one ([`row_of`]), and those fields turned out to have no writer in the first
/// place. The filter stands on what a row still exposes: its id, name and
/// description.
///
/// That is defense in depth, NOT a closed boundary on its own, and the
/// distinction is recorded here rather than implied by the presence of a
/// check: a workspace id is a user-chosen name (`"project-aleph"`), it
/// encodes no owner, and the `agent_envs` table has no owner column — so an
/// ordinary workspace passes this predicate for every caller who reaches this
/// handler.
///
/// # The residual was write, and it is closed at the admin gate
///
/// The 2026-08-08 real-machine QA exercised the write half: a member renamed
/// and then archived a workspace the operator had just created, both
/// returning `ok`. An earlier wording here named only "one member can read
/// another's `env_vars`", which was wrong twice over: that field has no writer
/// and is no longer sent, and naming a read understated the finding by a whole
/// verb class —
/// `update` and `archive` take the same plain id and clear the same predicate.
/// (That QA also produced a *false* pass on the list side: this handler looked
/// filtered only because the member had archived the row one call earlier and
/// the then-hard-coded `list(false)` skips archived.)
///
/// That wording also said closing it needed an owner column plus a migration,
/// "a schema change and a product decision, not a handler fix". It needed
/// neither. The whole `workspace.` family joined
/// [`crate::gateway::method_admin`]'s `ADMIN_PREFIXES` on 2026-08-08, with no
/// carve-out, because the family has exactly one client and it is already
/// operator (`aleph workspace list|get|create|update|archive`, over loopback);
/// the Panel has none. A member no longer reaches any method in this file.
/// (`get`/`update` had no client at all when that ruling was made and were
/// cited here as such — they gained CLI subcommands on 2026-08-08, inside this
/// same gate, so the ruling stands unchanged.)
///
/// **The predicate below still earns its place, and this file is now the only
/// place that says so.** The family left
/// `method_visibility::SCOPED_METHODS` in the same change — that table's
/// contract is per-user filtering on surfaces a MEMBER reaches, so a claim
/// there would have gone stale — but leaving the table is not the handlers
/// dropping the check. A second `UserRole::Admin` principal connects with
/// `CALLER_USER = Some(their own id)` rather than `OWNER_USER_ID`
/// (`handlers::connect::resolve_connection_identity` returns the linked
/// user's id for any admin that is not the zero-config loopback owner), so
/// `partition_visible` still refuses `main__u-alice` to an operator who is
/// not alice. Two gates, two questions — "may this role call it" and "may
/// this caller address that partition" — and neither implies the other.
///
/// # `include_archived`
///
/// Params are optional as a whole (no params = the active-only view), so this
/// cannot use [`parse_params`], which treats absent params as an error. Params
/// that are *present and unreadable* are still an error: silently defaulting
/// them would answer a narrower question than the caller asked and look like an
/// empty result.
pub async fn handle_list(
    request: JsonRpcRequest,
    workspace_manager: Arc<AgentEnvStore>,
) -> JsonRpcResponse {
    let include_archived = match &request.params {
        None => false,
        Some(params) => match serde_json::from_value::<WorkspaceListParams>(params.clone()) {
            Ok(parsed) => parsed.include_archived,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {e}"),
                );
            }
        },
    };

    match workspace_manager.list(include_archived).await {
        Ok(workspaces) => {
            let visible: Vec<WorkspaceRow> = workspaces
                .iter()
                .filter(|w| crate::gateway::visibility::partition_visible(&w.id))
                .map(row_of)
                .collect();
            JsonRpcResponse::success(request.id, json!({ "workspaces": visible }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to list workspaces: {e}"),
        ),
    }
}

// ============================================================================
// Get
// ============================================================================

/// Get a workspace by ID
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"workspace.get","params":{"id":"crypto"},"id":1}
/// ```
///
/// P1 partition isolation, with the same coverage boundary [`handle_list`]
/// documents: an invisible partition-composed id gets this method's OWN
/// "not found" response, byte-identical to an id that does not exist, and
/// the store is not read for it.
///
/// # Archived workspaces are visible here
///
/// This reads through [`AgentEnvStore::get_including_archived`] rather than
/// `get`. The default read filters `archived = 0` because its callers resolve
/// the env a run executes under; this one is addressed by exact id, is
/// read-only, and reports `is_archived` in the answer.
///
/// Filtering here would reintroduce, one level down, the complaint that
/// `include_archived` was added to `workspace.list` to fix: the list would
/// print a row this method then swears does not exist. "Readable, not
/// writable" is the whole rule — [`handle_update`] refuses the same rows, and
/// [`AgentEnvStore::update`] enforces that below it.
pub async fn handle_get(
    request: JsonRpcRequest,
    workspace_manager: Arc<AgentEnvStore>,
) -> JsonRpcResponse {
    let params: WorkspaceRef = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let not_found = || {
        JsonRpcResponse::error(
            request.id.clone(),
            RESOURCE_NOT_FOUND,
            format!("Workspace '{}' not found", params.id),
        )
    };
    if !crate::gateway::visibility::partition_visible(&params.id) {
        return not_found();
    }

    match workspace_manager.get_including_archived(&params.id).await {
        Ok(Some(ws)) => {
            JsonRpcResponse::success(request.id, json!({ "workspace": detail_of(&ws) }))
        }
        Ok(None) => not_found(),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to get workspace: {e}"),
        ),
    }
}

// ============================================================================
// Update
// ============================================================================

/// Update workspace metadata
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"workspace.update","params":{"id":"crypto","name":"Crypto Research"},"id":1}
/// ```
///
/// P1 partition isolation, same predicate and the SAME coverage boundary
/// [`handle_create`] and [`handle_list`] spell out — defense in depth against
/// partition-composed ids, not a closed boundary for ordinary ones. An
/// invisible id gets this method's OWN "not found" response, byte-identical to
/// an id that does not exist, and the store is never written for it.
///
/// # Archived workspaces are refused, and the refusal says which refusal it is
///
/// [`AgentEnvStore::update`] filters the write to active rows, so an archived
/// id reaches the `Ok(None)` arm below with nothing written — see its doc for
/// why the write was narrowed rather than the read-back widened, and for the
/// shape this replaced (the row was really rewritten and the caller was told it
/// did not exist).
///
/// That arm then asks which `None` it is, because the two have different honest
/// answers and this is the split the rest of the codebase already draws
/// (`src/gateway/CLAUDE.md`, P2 mine E): **invisible → `not_found`**, since
/// existence is itself the secret; **visible but not writable →
/// `PERMISSION_DENIED`**, since the caller can already read the row through
/// [`handle_get`] and a "not found" would simply be false to their face.
///
/// The no-oracle property is untouched: a partition-invisible id returns from
/// the check ABOVE and never reaches the store, so it cannot land on this
/// branch and cannot learn anything from it.
///
/// This distinction only became reachable when `get` started showing archived
/// rows in the same change — before that nothing could contradict the lie, and
/// nothing had a client to see it with either.
pub async fn handle_update(
    request: JsonRpcRequest,
    workspace_manager: Arc<AgentEnvStore>,
) -> JsonRpcResponse {
    let params: WorkspaceUpdateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let not_found = || {
        JsonRpcResponse::error(
            request.id.clone(),
            RESOURCE_NOT_FOUND,
            format!("Workspace '{}' not found", params.id),
        )
    };
    if !crate::gateway::visibility::partition_visible(&params.id) {
        return not_found();
    }

    match workspace_manager
        .update(
            &params.id,
            params.name.as_deref(),
            params.description.as_deref(),
            params.icon.as_deref(),
        )
        .await
    {
        Ok(Some(ws)) => JsonRpcResponse::success(
            request.id.clone(),
            json!({
                "ok": true,
                "workspace": detail_of(&ws),
            }),
        ),
        // Which `None` is it? An archived row is visible to this caller, so
        // saying "not found" would contradict the `workspace.get` they can run
        // in the next breath. Anything else genuinely is not there.
        Ok(None) => match workspace_manager.get_including_archived(&params.id).await {
            // Gated on the OBSERVED flag, not on the row merely existing: the
            // message states a fact about the row, so it has to be one we read
            // rather than one we inferred from which arm we are on.
            Ok(Some(ws)) if ws.is_archived => JsonRpcResponse::error(
                request.id.clone(),
                PERMISSION_DENIED,
                format!(
                    "Workspace '{}' is archived and cannot be modified",
                    params.id
                ),
            ),
            // Everything else — no row, the read failing, or a live row that
            // somehow did not match the write. This probe exists to upgrade a
            // refusal that is already true into a more specific one; a probe
            // that cannot answer falls back to what it was refining rather
            // than inventing something.
            _ => not_found(),
        },
        Err(e) => JsonRpcResponse::error(
            request.id.clone(),
            INTERNAL_ERROR,
            format!("Failed to update workspace: {e}"),
        ),
    }
}

// ============================================================================
// Archive
// ============================================================================

/// Archive (soft-delete) a workspace
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"workspace.archive","params":{"id":"crypto"},"id":1}
/// ```
///
/// P1 partition isolation, same predicate and the SAME coverage boundary
/// [`handle_create`] and [`handle_list`] spell out. The soft-delete is the
/// most destructive verb in this file — an invisible id gets this method's
/// OWN "not found" response, byte-identical to an id that does not exist, and
/// the row is never archived.
///
/// Reversible since 2026-08-09: see [`handle_unarchive`]. What has NOT changed
/// is that the id stays taken — `workspace.create` still refuses it (with a
/// refusal that now names the way back).
pub async fn handle_archive(
    request: JsonRpcRequest,
    workspace_manager: Arc<AgentEnvStore>,
) -> JsonRpcResponse {
    let params: WorkspaceRef = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let not_found = || {
        JsonRpcResponse::error(
            request.id.clone(),
            RESOURCE_NOT_FOUND,
            format!("Workspace '{}' not found", params.id),
        )
    };
    if !crate::gateway::visibility::partition_visible(&params.id) {
        return not_found();
    }

    match workspace_manager.archive(&params.id).await {
        Ok(true) => JsonRpcResponse::success(request.id, json!({ "ok": true })),
        Ok(false) => not_found(),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to archive workspace: {e}"),
        ),
    }
}

// ============================================================================
// Unarchive
// ============================================================================

/// Restore an archived workspace — the inverse of [`handle_archive`].
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"workspace.unarchive","params":{"id":"crypto"},"id":1}
/// ```
///
/// Takes [`WorkspaceRef`], the same param type as `get` and `archive`: it
/// addresses the same thing, and a third struct would be a third place to
/// drift.
///
/// # Why archive stopped being terminal
///
/// It was terminal by omission dressed as design. "Readable, not writable" is a
/// sound rule for an archived row, but with no way back a single mistyped
/// `workspace.archive` was permanent — and it is permanent in the expensive
/// direction, because the id stays taken (`AgentEnvStore::create` is a plain
/// INSERT against a primary key) so the operator cannot even start over under
/// the same name. See [`crate::gateway::agent_env::AgentEnvStore::unarchive`]
/// for why the id staying taken is the right half to keep.
///
/// # This one returns the workspace, unlike `archive`
///
/// Deliberate asymmetry, recorded here so it is not "fixed" into symmetry
/// later. `archive`'s result is that the row left the default view — there is
/// nothing useful left to show. This one's result IS a row, and a caller that
/// has to re-read it with a follow-up `get` is racing anything else holding the
/// store. The envelope is `update`'s, so a client already parsing that shape
/// parses this one.
///
/// P1 partition isolation, same predicate and the same coverage boundary the
/// rest of the family documents: an invisible id gets this method's OWN "not
/// found" response, byte-identical to an id that does not exist, and the flag
/// is never flipped.
pub async fn handle_unarchive(
    request: JsonRpcRequest,
    workspace_manager: Arc<AgentEnvStore>,
) -> JsonRpcResponse {
    let params: WorkspaceRef = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let not_found = || {
        JsonRpcResponse::error(
            request.id.clone(),
            RESOURCE_NOT_FOUND,
            format!("Workspace '{}' not found", params.id),
        )
    };
    if !crate::gateway::visibility::partition_visible(&params.id) {
        return not_found();
    }

    match workspace_manager.unarchive(&params.id).await {
        Ok(Some(ws)) => JsonRpcResponse::success(
            request.id,
            json!({
                "ok": true,
                "workspace": detail_of(&ws),
            }),
        ),
        // Unambiguous, unlike `handle_update`'s `None`: the store's UPDATE
        // carries no `archived` predicate, so this is "no such row" and nothing
        // else. No follow-up probe — there is nothing to disambiguate, and a
        // probe that cannot change the answer reads like one that can.
        Ok(None) => not_found(),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to unarchive workspace: {e}"),
        ),
    }
}

// ============================================================================
// Channel Agent Binding (1:1 model)
// ============================================================================

/// Parameters for `channels.set_agent`
#[derive(Debug, Deserialize)]
pub struct SetAgentParams {
    pub channel_id: String,
    pub agent_id: Option<String>,
}

/// Bind or unbind an agent to/from a channel.
///
/// If `agent_id` is Some, binds the agent (with 1:1 constraint).
/// If `agent_id` is None, unbinds the current agent.
///
/// Delegates to the shared binding seam (`gateway::agent_binding`) — the same
/// implementation behind the `agent_switch` tool — so both surfaces share
/// ghost validation, no-op detection, and `Bound`/`Unbound` lifecycle events
/// (this RPC previously emitted no event at all, leaving other Panels stale).
/// `agent_registry: None` (a minimal server) skips validation, preserving the
/// prior unchecked behavior rather than blocking the bind.
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"channels.set_agent","params":{"channel_id":"rpc","agent_id":"project-x"},"id":1}
/// ```
pub async fn handle_set_agent(
    request: JsonRpcRequest,
    workspace_manager: Arc<AgentEnvStore>,
    agent_registry: Option<Arc<AgentRegistry>>,
    event_bus: Option<Arc<GatewayEventBus>>,
) -> JsonRpcResponse {
    use crate::gateway::agent_binding::{bind_channel_agent, unbind_channel_agent, BindError};

    let params: SetAgentParams =
        match serde_json::from_value(request.params.clone().unwrap_or_default()) {
            Ok(p) => p,
            Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, e.to_string()),
        };

    match params.agent_id {
        Some(agent_id) => {
            match bind_channel_agent(
                agent_registry.as_deref(),
                &workspace_manager,
                event_bus.as_deref(),
                &params.channel_id,
                &agent_id,
            )
            .await
            {
                Ok(outcome) => JsonRpcResponse::success(
                    request.id,
                    json!({
                        "ok": true,
                        "previous_agent": outcome.previous_agent,
                        "no_op": outcome.no_op,
                    }),
                ),
                Err(e @ (BindError::UnknownAgent { .. } | BindError::EmptyChannel)) => {
                    JsonRpcResponse::error(request.id, INVALID_PARAMS, e.to_string())
                }
                Err(e @ BindError::Store(_)) => {
                    JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string())
                }
            }
        }
        None => {
            match unbind_channel_agent(&workspace_manager, event_bus.as_deref(), &params.channel_id)
            {
                Ok(previous) => JsonRpcResponse::success(
                    request.id,
                    json!({"ok": true, "previous_agent": previous}),
                ),
                Err(e @ BindError::EmptyChannel) => {
                    JsonRpcResponse::error(request.id, INVALID_PARAMS, e.to_string())
                }
                Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
            }
        }
    }
}

/// Get every channel bound to each agent for the Panel (many-to-one aware).
///
/// Response shape: `{"bindings": {"<agent_id>": ["<channel>", …]}}` — channels
/// sorted. The previous one-channel-per-agent map was lossy (an agent bound to
/// several channels showed only one); consumers were migrated with the shape.
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"agents.bindings","id":1}
/// ```
pub async fn handle_agent_bindings(
    request: JsonRpcRequest,
    workspace_manager: Arc<AgentEnvStore>,
) -> JsonRpcResponse {
    match workspace_manager.bindings_by_agent() {
        Ok(bindings) => JsonRpcResponse::success(request.id, json!({"bindings": bindings})),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Final-review I6, defense-in-depth half: a partition-composed workspace
    /// id belonging to another user is invisible, and `get` denies with this
    /// method's own not-found rather than a distinct error.
    ///
    /// What this test deliberately does NOT claim: that ordinary workspaces
    /// are isolated. `"crypto"` carries no owner, so it passes the predicate
    /// for everyone who reaches this handler — that half is answered by the
    /// admin gate instead (see [`handle_list`]'s doc).
    ///
    /// The scoped id below is written as a member's for readability, but this
    /// calls the handler directly and so proves nothing about who may dispatch
    /// to it: since 2026-08-08 a member cannot — `workspace.` is admin-gated,
    /// pinned in [`crate::gateway::method_admin`]'s own tests. The predicate
    /// under test binds operators too: a second `UserRole::Admin` principal
    /// carries its OWN `CALLER_USER`, not `OWNER_USER_ID`, so this is the
    /// surviving case rather than a hypothetical one.
    #[tokio::test]
    async fn get_denies_a_foreign_partition_composed_id_as_not_found() {
        use crate::gateway::caller_identity::CALLER_USER;

        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            AgentEnvStore::new(crate::gateway::agent_env::AgentEnvStoreConfig {
                db_path: temp.path().join("agent_envs.db"),
                default_profile: "default".to_string(),
            })
            .expect("agent env store"),
        );
        let req = |id: &str| {
            JsonRpcRequest::with_id("workspace.get", Some(json!({ "id": id })), json!(1))
        };

        let denied = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_get(req("main__u-alice"), store.clone()),
            )
            .await;
        let missing = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_get(req("main__u-alice"), store.clone()),
            )
            .await;
        assert!(denied.result.is_none(), "no AgentEnv may be serialized");
        assert_eq!(
            denied.error.as_ref().map(|e| e.code),
            Some(RESOURCE_NOT_FOUND)
        );
        assert_eq!(
            serde_json::to_string(&denied).unwrap(),
            serde_json::to_string(&missing).unwrap(),
            "a denied id and a nonexistent id must be byte-identical"
        );

        // Not a false positive: bob's own composed id and an ordinary
        // uncomposed id both get past the predicate (and then legitimately
        // 404, since nothing was created).
        for id in ["main__u-bob", "crypto"] {
            let resp = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_get(req(id), store.clone()),
                )
                .await;
            assert_eq!(
                resp.error.as_ref().map(|e| e.code),
                Some(RESOURCE_NOT_FOUND),
                "{id} should reach the store and report a genuine miss"
            );
        }
    }

    /// The write half of the same defense-in-depth check the reads carry.
    /// Reads were gated first, which left the strictly worse half open: a
    /// caller could CREATE `main__u-alice`, and that row then appears only in
    /// ALICE's filtered `workspace.list` under a name and description she never
    /// wrote. (When this was written that caller could be a member; since
    /// 2026-08-08 the family is admin-gated and the surviving case is one
    /// operator addressing another user's partition. The original wording said
    /// the planted row carried `env_vars` / `system_prompt_override` /
    /// `allowed_tools` — see [`handle_create`]'s doc for why that was never the
    /// case.)
    ///
    /// Each assertion is on the STORE after the call, not on the response —
    /// a check that returned the right error while the write still landed
    /// would pass a response-only test.
    ///
    /// Same boundary as [`handle_list`]: this does NOT claim ordinary
    /// workspaces are isolated. The last block proves the opposite on
    /// purpose, so nobody reads this test as more than it is.
    /// One `workspace.create` reaches the wire as TWO frames: `Created`, then
    /// `Updated`.
    ///
    /// [`handle_create`] is two store writes — the INSERT, then the name/icon
    /// write `AgentEnvStore::create` cannot take — and the frames are published
    /// by the store, so the count follows the writes rather than the verb. Real
    /// machine, 2026-08-09: a second Panel answered a single create with two
    /// byte-identical `workspace.list` calls in the same millisecond.
    ///
    /// Pinned rather than fixed. Re-fetching is idempotent, so the cost is one
    /// redundant list call per listener, while coalescing would teach the store
    /// how many writes its callers make — the coupling that emitting from the
    /// store instead of the handlers exists to avoid. What this test protects
    /// is the *documentation*: `ChangeKind::Created` reads like a promise of
    /// one frame, and a consumer that renders `change` (a toast) needs to know
    /// it will be told "created" and then immediately "updated" for a single
    /// user action. If a later change makes create a single write, this goes
    /// red — and the frame's doc has to be corrected in the same commit.
    #[tokio::test]
    async fn create_reaches_the_wire_as_created_then_updated() {
        use crate::gateway::event_bus::GatewayEventBus;
        use crate::gateway::events::{ChangeKind, GatewayEventFrame};

        let temp = tempfile::tempdir().unwrap();
        let bus = Arc::new(GatewayEventBus::new());
        let mut rx = bus.subscribe_typed();
        let store = Arc::new(
            AgentEnvStore::new(crate::gateway::agent_env::AgentEnvStoreConfig {
                db_path: temp.path().join("agent_envs.db"),
                default_profile: "default".to_string(),
            })
            .expect("agent env store")
            .with_event_bus(Arc::clone(&bus)),
        );
        store.load_profiles(std::collections::HashMap::from([(
            "default".to_string(),
            crate::config::ProfileConfig::default(),
        )]));

        let created = handle_create(
            JsonRpcRequest::with_id(
                "workspace.create",
                Some(json!({ "id": "crypto", "name": "Crypto" })),
                json!(1),
            ),
            store.clone(),
        )
        .await;
        assert!(created.error.is_none(), "create must succeed: {created:?}");

        let mut seen = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            if let GatewayEventFrame::WorkspaceChanged {
                workspace_id,
                change,
            } = frame
            {
                seen.push((workspace_id, change));
            }
        }
        assert_eq!(
            seen,
            vec![
                ("crypto".to_string(), ChangeKind::Created),
                ("crypto".to_string(), ChangeKind::Updated),
            ],
            "the second write is the name/icon one; see GatewayEventFrame::WorkspaceChanged"
        );
    }

    #[tokio::test]
    async fn the_workspace_writes_deny_a_foreign_partition_composed_id() {
        use crate::gateway::caller_identity::CALLER_USER;

        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            AgentEnvStore::new(crate::gateway::agent_env::AgentEnvStoreConfig {
                db_path: temp.path().join("agent_envs.db"),
                default_profile: "default".to_string(),
            })
            .expect("agent env store"),
        );
        // `handle_create` hard-codes the `"default"` profile; without it every
        // store write below fails as ProfileNotFound and the denials would
        // look correct for the wrong reason.
        store.load_profiles(std::collections::HashMap::from([(
            "default".to_string(),
            crate::config::ProfileConfig::default(),
        )]));
        let req = |method: &str, params: serde_json::Value| {
            JsonRpcRequest::with_id(method, Some(params), json!(1))
        };
        async fn as_bob<F: std::future::Future<Output = JsonRpcResponse>>(
            fut: F,
        ) -> JsonRpcResponse {
            crate::gateway::caller_identity::CALLER_USER
                .scope(Some("u-bob".to_string()), fut)
                .await
        }

        // --- create: the row must never come into existence -------------
        let created = as_bob(handle_create(
            req(
                "workspace.create",
                json!({ "id": "main__u-alice", "name": "planted" }),
            ),
            store.clone(),
        ))
        .await;
        assert!(created.result.is_none());
        assert!(
            store.get("main__u-alice").await.unwrap().is_none(),
            "a denied create must not insert the row"
        );

        // Byte-identical to a genuine id collision on the SAME id — the only
        // "you cannot have this id" answer this method has ever had. Alice
        // passes the predicate, so hers is the real store error.
        store
            .create("main__u-alice", "default", None)
            .await
            .unwrap();
        let collided = CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_create(
                    req(
                        "workspace.create",
                        json!({ "id": "main__u-alice", "name": "planted" }),
                    ),
                    store.clone(),
                ),
            )
            .await;
        assert_eq!(
            serde_json::to_string(&created).unwrap(),
            serde_json::to_string(&collided).unwrap(),
            "the denial must be the collision shape, not a new one"
        );

        // --- update: alice's real row must keep its name ----------------
        store
            .update("main__u-alice", Some("alice's env"), None, None)
            .await
            .unwrap();
        let updated = as_bob(handle_update(
            req(
                "workspace.update",
                json!({ "id": "main__u-alice", "name": "renamed-by-bob" }),
            ),
            store.clone(),
        ))
        .await;
        assert_eq!(
            updated.error.as_ref().map(|e| e.code),
            Some(RESOURCE_NOT_FOUND)
        );
        assert_eq!(
            store.get("main__u-alice").await.unwrap().unwrap().name,
            "alice's env",
            "a denied update must not rewrite the foreign workspace"
        );

        // --- archive: the row must still be live ------------------------
        let archived = as_bob(handle_archive(
            req("workspace.archive", json!({ "id": "main__u-alice" })),
            store.clone(),
        ))
        .await;
        assert_eq!(
            archived.error.as_ref().map(|e| e.code),
            Some(RESOURCE_NOT_FOUND)
        );
        assert!(
            store.get("main__u-alice").await.unwrap().is_some(),
            "a denied archive must not soft-delete the foreign workspace"
        );
        // …and no existence oracle: the refusal must read the same whether
        // the row is there or not. `main__u-carol` is archived by bob twice —
        // once while it has never been created, once after it has — so what
        // varies between the two responses is EXISTENCE and nothing else.
        //
        // The id is deliberately held fixed rather than comparing two
        // different ids: this method's not-found message echoes the id the
        // caller itself supplied, so two ids would differ on the wire for a
        // reason that leaks nothing, and the comparison would have to be
        // weakened to survive it. Two calls that differ in neither id nor
        // store state would instead compare a call to itself and could not
        // fail at all.
        let never_created = as_bob(handle_archive(
            req("workspace.archive", json!({ "id": "main__u-carol" })),
            store.clone(),
        ))
        .await;
        store
            .create("main__u-carol", "default", None)
            .await
            .unwrap();
        let now_exists = as_bob(handle_archive(
            req("workspace.archive", json!({ "id": "main__u-carol" })),
            store.clone(),
        ))
        .await;
        assert_eq!(
            serde_json::to_string(&never_created).unwrap(),
            serde_json::to_string(&now_exists).unwrap(),
            "the refusal must not tell bob whether main__u-carol exists"
        );
        assert!(
            store.get("main__u-carol").await.unwrap().is_some(),
            "…and neither denied archive may soft-delete the row"
        );

        // --- the boundary, asserted rather than implied ------------------
        // An ordinary id encodes no owner, so bob passes the predicate for
        // it. This check is defense in depth against composed ids ONLY;
        // closing the rest needs an owner column (see `handle_list`).
        store.create("crypto", "default", None).await.unwrap();
        let ordinary = as_bob(handle_archive(
            req("workspace.archive", json!({ "id": "crypto" })),
            store.clone(),
        ))
        .await;
        assert!(
            ordinary.error.is_none(),
            "an ordinary workspace is NOT protected by this check: {:?}",
            ordinary.error
        );
    }

    #[test]
    fn test_create_params_deserialization() {
        let json = serde_json::json!({"id": "crypto", "name": "Crypto Trading"});
        let params: WorkspaceCreateParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.id, "crypto");
        assert_eq!(params.name, "Crypto Trading");
        assert!(params.description.is_none());
    }

    #[test]
    fn test_create_params_with_optional_fields() {
        let json = serde_json::json!({"id": "novel", "name": "Novel", "description": "My novel project", "icon": "\u{1F4D6}"});
        let params: WorkspaceCreateParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.description.as_deref(), Some("My novel project"));
        assert_eq!(params.icon.as_deref(), Some("\u{1F4D6}"));
    }

    #[test]
    fn test_get_params_deserialization() {
        let json = serde_json::json!({"id": "crypto"});
        let params: WorkspaceRef = serde_json::from_value(json).unwrap();
        assert_eq!(params.id, "crypto");
    }

    #[test]
    fn test_update_params_deserialization() {
        let json = serde_json::json!({"id": "crypto", "name": "Crypto Research"});
        let params: WorkspaceUpdateParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.id, "crypto");
        assert_eq!(params.name.as_deref(), Some("Crypto Research"));
        assert!(params.description.is_none());
        assert!(params.icon.is_none());
    }

    #[test]
    fn test_update_params_all_fields() {
        let json = serde_json::json!({
            "id": "crypto",
            "name": "Crypto Research",
            "description": "Updated description",
            "icon": "\u{1F4B0}"
        });
        let params: WorkspaceUpdateParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.id, "crypto");
        assert_eq!(params.name.as_deref(), Some("Crypto Research"));
        assert_eq!(params.description.as_deref(), Some("Updated description"));
        assert_eq!(params.icon.as_deref(), Some("\u{1F4B0}"));
    }

    /// The two commands that had never once worked: `aleph workspace create`
    /// sent `{"name": …}` at a handler that requires `id`, and
    /// `aleph workspace archive` did the same, so both returned
    /// `INVALID_PARAMS` on every invocation for as long as they had existed.
    ///
    /// This drives the handlers with the very types the CLI now constructs
    /// (`aleph_protocol::workspace::*`), so the request half of that gap cannot
    /// reopen without either failing here or failing to compile. The
    /// assertions are on the STORE, not on the response — a handler that
    /// answered `ok` while writing nothing would pass a response-only test.
    ///
    /// The last block re-asserts the historical shape is still rejected. That
    /// is what keeps this test honest: without it, a handler loosened to accept
    /// anything at all would look like a fix.
    #[tokio::test]
    async fn the_cli_create_and_archive_shapes_reach_their_handlers() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            AgentEnvStore::new(crate::gateway::agent_env::AgentEnvStoreConfig {
                db_path: temp.path().join("agent_envs.db"),
                default_profile: "default".to_string(),
            })
            .expect("agent env store"),
        );
        // `load_profiles` seeds "default" itself; this mirrors what
        // `start/mod.rs` does with an empty `[profiles]` section, which is the
        // shipped default and therefore the case that has to work.
        store.load_profiles(std::collections::HashMap::new());

        let created = handle_create(
            JsonRpcRequest::with_id(
                "workspace.create",
                Some(
                    serde_json::to_value(WorkspaceCreateParams {
                        id: "crypto".to_string(),
                        name: "Crypto Trading".to_string(),
                        description: Some("trading notes".to_string()),
                        icon: None,
                    })
                    .unwrap(),
                ),
                json!(1),
            ),
            store.clone(),
        )
        .await;
        assert!(
            created.is_success(),
            "the CLI's create shape must be accepted: {:?}",
            created.error
        );
        let row = store.get("crypto").await.unwrap().expect("row must exist");
        assert_eq!(row.name, "Crypto Trading");
        assert_eq!(row.description.as_deref(), Some("trading notes"));

        let archived = handle_archive(
            JsonRpcRequest::with_id(
                "workspace.archive",
                Some(
                    serde_json::to_value(WorkspaceRef {
                        id: "crypto".to_string(),
                    })
                    .unwrap(),
                ),
                json!(1),
            ),
            store.clone(),
        )
        .await;
        assert!(
            archived.is_success(),
            "the CLI's archive shape must be accepted: {:?}",
            archived.error
        );
        assert!(
            store.get("crypto").await.unwrap().is_none(),
            "archive must actually soft-delete the row"
        );

        // …and the shape that was broken is still a rejection, not a silently
        // accepted alias.
        for (method, params) in [
            ("workspace.create", json!({ "name": "crypto" })),
            ("workspace.archive", json!({ "name": "crypto" })),
        ] {
            let resp = if method == "workspace.create" {
                handle_create(
                    JsonRpcRequest::with_id(method, Some(params), json!(1)),
                    store.clone(),
                )
                .await
            } else {
                handle_archive(
                    JsonRpcRequest::with_id(method, Some(params), json!(1)),
                    store.clone(),
                )
                .await
            };
            assert_eq!(
                resp.error.as_ref().map(|e| e.code),
                Some(INVALID_PARAMS),
                "{method} must still require `id`"
            );
        }
    }

    /// Every column `aleph workspace list` prints has to exist in what this
    /// handler actually emits.
    ///
    /// It did not. The table read `status` and `created`; an `AgentEnv`
    /// serializes `is_archived` and `created_at`. Both columns were rendered
    /// with `.unwrap_or("-")`, so every row printed dashes and read as "this
    /// workspace has no status yet" rather than "this client is asking for
    /// fields that do not exist" — and neither side's tests could see it,
    /// because neither side ever looked at the other.
    ///
    /// The projection is asserted here, on the server, because this is the
    /// side that owns the field names.
    #[tokio::test]
    async fn every_column_the_cli_renders_is_present_in_the_list_response() {
        use aleph_protocol::workspace::WorkspaceList;

        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            AgentEnvStore::new(crate::gateway::agent_env::AgentEnvStoreConfig {
                db_path: temp.path().join("agent_envs.db"),
                default_profile: "default".to_string(),
            })
            .expect("agent env store"),
        );
        store.load_profiles(std::collections::HashMap::new());
        store
            .create("crypto", "default", Some("trading notes"))
            .await
            .unwrap();

        let resp = handle_list(
            JsonRpcRequest::with_id("workspace.list", None, json!(1)),
            store.clone(),
        )
        .await;
        let list: WorkspaceList = serde_json::from_value(resp.result.expect("result"))
            .expect("the CLI's list projection must parse the real response");

        let row = list
            .workspaces
            .iter()
            .find(|w| w.id == "crypto")
            .expect("the created workspace must be listed");
        assert_eq!(row.name, "crypto", "name defaults to the id server-side");
        assert_eq!(row.description.as_deref(), Some("trading notes"));
        assert!(
            row.created_at.timestamp() > 0,
            "created_at must be a real timestamp, not a default"
        );
    }

    /// The `get`/`update` twin of
    /// [`every_column_the_cli_renders_is_present_in_the_list_response`], and it
    /// exists for the same reason: `aleph-cli` cannot depend on `alephcore`, so
    /// the only guard that holds for this contract is a shared type plus an
    /// assertion on THIS side, where the field names are owned.
    ///
    /// It runs against both methods because they return the same envelope from
    /// two different code paths — `get` from a store read, `update` from a
    /// read-back after a write — and a projection that parses one is not
    /// thereby proven against the other.
    #[tokio::test]
    async fn every_field_the_cli_renders_is_present_in_the_get_and_update_responses() {
        use aleph_protocol::workspace::{WorkspaceEnvelope, WorkspaceUpdateParams};

        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            AgentEnvStore::new(crate::gateway::agent_env::AgentEnvStoreConfig {
                db_path: temp.path().join("agent_envs.db"),
                default_profile: "default".to_string(),
            })
            .expect("agent env store"),
        );
        store.load_profiles(std::collections::HashMap::new());
        store
            .create("crypto", "default", Some("trading notes"))
            .await
            .unwrap();

        let got = handle_get(
            JsonRpcRequest::with_id("workspace.get", Some(json!({ "id": "crypto" })), json!(1)),
            store.clone(),
        )
        .await;
        let detail: WorkspaceEnvelope = serde_json::from_value(got.result.expect("result"))
            .expect("the CLI's detail projection must parse the real get response");
        let detail = detail.workspace;
        assert_eq!(detail.id, "crypto");
        assert_eq!(detail.name, "crypto", "name defaults to the id server-side");
        assert_eq!(detail.description.as_deref(), Some("trading notes"));
        assert_eq!(
            detail.profile, "default",
            "Profile is a detail-only line — the list projection has no such field"
        );
        assert!(detail.icon.is_none(), "a fresh workspace has no icon");
        assert!(detail.created_at.timestamp() > 0);
        assert!(detail.last_active_at.timestamp() > 0);
        assert!(!detail.is_archived);

        let updated = handle_update(
            JsonRpcRequest::with_id(
                "workspace.update",
                Some(
                    serde_json::to_value(WorkspaceUpdateParams {
                        id: "crypto".to_string(),
                        name: Some("Crypto Research".to_string()),
                        description: None,
                        icon: Some("\u{1F4B0}".to_string()),
                    })
                    .unwrap(),
                ),
                json!(2),
            ),
            store.clone(),
        )
        .await;
        let patched: WorkspaceEnvelope = serde_json::from_value(updated.result.expect("result"))
            .expect("the same projection must parse the real update response");
        assert_eq!(patched.workspace.name, "Crypto Research");
        assert_eq!(patched.workspace.icon.as_deref(), Some("\u{1F4B0}"));
        assert_eq!(
            patched.workspace.description.as_deref(),
            Some("trading notes"),
            "an omitted field is a patch that leaves the value alone, not a clear"
        );
    }

    /// The converse of the two tests above, and the half that was missing.
    ///
    /// Both of those parse the response into the CLI's projection and check the
    /// fields are there. Parsing proves the response is a **superset** of the
    /// contract — serde ignores unknown keys, which is exactly the property
    /// that let four fields ride the wire unnoticed. `env_vars`,
    /// `allowed_tools`, `system_prompt_override` and `default_model` have no
    /// writer anywhere and no reader in the execution pipeline
    /// ([`crate::gateway::agent_env::ActiveAgentEnv`] drops them), so
    /// `workspace get --json` was publishing a configuration surface that
    /// nothing in Aleph would ever act on.
    ///
    /// So this asserts **equality** of the key sets, in both directions. The
    /// expected set is derived by round-tripping through the contract type
    /// rather than written out as a literal: a literal list is the same
    /// enumeration mistake one level up, green on the day it is written and
    /// silent about every field added after it.
    ///
    /// Failure here means someone went back to serializing the store type
    /// directly. Re-point the handler at `detail_of` / `row_of`; do not widen
    /// this assertion.
    ///
    /// Named `the_read_responses_…` until 2026-08-09, when `unarchive` joined
    /// it: that one is a write whose response carries the same projection, and
    /// a name that excluded it would have argued for leaving it uncovered.
    /// `aleph_protocol::workspace`'s module doc points here by name — if this
    /// is renamed again, that pointer moves in the same edit.
    #[tokio::test]
    async fn the_workspace_responses_carry_the_contract_and_nothing_else() {
        use aleph_protocol::workspace::{WorkspaceEnvelope, WorkspaceList};
        use std::collections::BTreeSet;

        /// Keys a `Serialize` value emits, as a set.
        fn keys_of<T: serde::Serialize>(v: &T) -> BTreeSet<String> {
            serde_json::to_value(v)
                .expect("contract types serialize")
                .as_object()
                .expect("both projections are objects")
                .keys()
                .cloned()
                .collect()
        }

        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            AgentEnvStore::new(crate::gateway::agent_env::AgentEnvStoreConfig {
                db_path: temp.path().join("agent_envs.db"),
                default_profile: "default".to_string(),
            })
            .expect("agent env store"),
        );
        store.load_profiles(std::collections::HashMap::new());
        store
            .create("crypto", "default", Some("trading notes"))
            .await
            .unwrap();

        // --- get: the detail projection, exactly ------------------------
        let got = handle_get(
            JsonRpcRequest::with_id("workspace.get", Some(json!({ "id": "crypto" })), json!(1)),
            store.clone(),
        )
        .await;
        let got = got.result.expect("result");
        let emitted: BTreeSet<String> = got["workspace"]
            .as_object()
            .expect("workspace is an object")
            .keys()
            .cloned()
            .collect();
        let parsed: WorkspaceEnvelope =
            serde_json::from_value(got.clone()).expect("the contract must parse it");
        assert_eq!(
            emitted,
            keys_of(&parsed.workspace),
            "workspace.get must emit the WorkspaceDetail key set and nothing else"
        );

        // --- list: the row projection, exactly --------------------------
        let listed = handle_list(
            JsonRpcRequest::with_id("workspace.list", None, json!(2)),
            store.clone(),
        )
        .await;
        let listed = listed.result.expect("result");
        let emitted: BTreeSet<String> = listed["workspaces"][0]
            .as_object()
            .expect("each row is an object")
            .keys()
            .cloned()
            .collect();
        let parsed: WorkspaceList =
            serde_json::from_value(listed.clone()).expect("the contract must parse it");
        assert_eq!(
            emitted,
            keys_of(&parsed.workspaces[0]),
            "workspace.list rows must emit the WorkspaceRow key set and nothing else"
        );

        // --- unarchive: a write, same projection, same rule ----------------
        // It arrived after the `detail_of` refactor and so has never been able
        // to serialize the store type — which is exactly why it is pinned
        // here rather than trusted: the next projection to be added will also
        // arrive correct, and this is the assertion that keeps it that way.
        assert!(store.archive("crypto").await.unwrap());
        let restored = handle_unarchive(
            JsonRpcRequest::with_id(
                "workspace.unarchive",
                Some(json!({ "id": "crypto" })),
                json!(3),
            ),
            store.clone(),
        )
        .await;
        let restored = restored.result.expect("result");
        let emitted: BTreeSet<String> = restored["workspace"]
            .as_object()
            .expect("workspace is an object")
            .keys()
            .cloned()
            .collect();
        let parsed: WorkspaceEnvelope =
            serde_json::from_value(restored.clone()).expect("the contract must parse it");
        assert_eq!(
            emitted,
            keys_of(&parsed.workspace),
            "workspace.unarchive must emit the WorkspaceDetail key set and nothing else"
        );

        // The four dormant fields, named. The set assertions above already
        // cover them, and since 2026-08-09 they are not even fields of
        // `AgentEnv` any more — this loop is belt-and-braces, kept because
        // `assert_eq!` on a set prints a diff and not a history, and because
        // the names are what a future reader will search for when someone
        // proposes adding one of them back.
        for dead in [
            "env_vars",
            "allowed_tools",
            "system_prompt_override",
            "default_model",
        ] {
            assert!(
                got["workspace"].get(dead).is_none(),
                "`{dead}` has no writer and no reader — it must not be on the wire"
            );
        }
    }

    /// Archived workspaces are **readable, not writable** — the ruling the
    /// whole family now shares, asserted on both halves at once because each
    /// half alone reads as an arbitrary choice.
    ///
    /// The write half is the one that was broken. `AgentEnvStore::update`'s
    /// UPDATE matched archived rows while its read-back (`get`) filtered them,
    /// so an archived workspace was **really rewritten** and the caller was
    /// then told `Ok(None)` — which this handler renders as "not found". It was
    /// unreachable only because `workspace.update` had no client; adding
    /// `aleph workspace update` is what would have made it real.
    ///
    /// So the assertion is on the STORE, not on the response: a handler that
    /// returned exactly this "not found" while the write still landed is
    /// precisely the bug, and it would pass a response-only test.
    #[tokio::test]
    async fn an_archived_workspace_is_readable_but_not_writable() {
        use aleph_protocol::workspace::WorkspaceEnvelope;

        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            AgentEnvStore::new(crate::gateway::agent_env::AgentEnvStoreConfig {
                db_path: temp.path().join("agent_envs.db"),
                default_profile: "default".to_string(),
            })
            .expect("agent env store"),
        );
        store.load_profiles(std::collections::HashMap::new());
        store.create("retired", "default", None).await.unwrap();
        store
            .update("retired", Some("Retired Project"), None, None)
            .await
            .unwrap();
        assert!(store.archive("retired").await.unwrap());

        // --- readable: the row `list --include-archived` prints is reachable
        // by id, and says which state it is in.
        let got = handle_get(
            JsonRpcRequest::with_id("workspace.get", Some(json!({ "id": "retired" })), json!(1)),
            store.clone(),
        )
        .await;
        let envelope: WorkspaceEnvelope = serde_json::from_value(
            got.result
                .expect("an archived workspace must be reachable by id"),
        )
        .expect("projection");
        assert_eq!(envelope.workspace.name, "Retired Project");
        assert!(
            envelope.workspace.is_archived,
            "the Status line has to be able to say `archived`"
        );

        // --- not writable: refused, and the refusal is TRUE.
        let refused = handle_update(
            JsonRpcRequest::with_id(
                "workspace.update",
                Some(json!({ "id": "retired", "name": "renamed-after-archive" })),
                json!(2),
            ),
            store.clone(),
        )
        .await;
        // PERMISSION_DENIED, not RESOURCE_NOT_FOUND: the caller just read this
        // row through `handle_get` above, so "not found" would be false to
        // their face. The invisible case still gets `not_found` and is pinned
        // by `the_workspace_writes_deny_a_foreign_partition_composed_id`, whose
        // id never reaches the store at all.
        assert_eq!(
            refused.error.as_ref().map(|e| e.code),
            Some(PERMISSION_DENIED),
            "an archived row is visible to this caller — refusing it as \
             `not found` contradicts the get they can run next"
        );
        assert!(refused
            .error
            .as_ref()
            .is_some_and(|e| e.message.contains("archived")));
        assert_eq!(
            store
                .get_including_archived("retired")
                .await
                .unwrap()
                .expect("the row is still there")
                .name,
            "Retired Project",
            "the refusal must mean the write did not land — this assertion is \
             the whole test; the response above said `not found` even when it did"
        );

        // Not a false positive: the same patch against a LIVE workspace lands.
        // Without this the test would also pass if `update` had simply been
        // broken for everything.
        store.create("live", "default", None).await.unwrap();
        let applied = handle_update(
            JsonRpcRequest::with_id(
                "workspace.update",
                Some(json!({ "id": "live", "name": "renamed" })),
                json!(3),
            ),
            store.clone(),
        )
        .await;
        assert!(applied.is_success(), "{:?}", applied.error);
        assert_eq!(store.get("live").await.unwrap().unwrap().name, "renamed");
    }

    /// `include_archived` has to reach the store, and a params object this
    /// handler cannot read has to be an error rather than the default view.
    ///
    /// The second half is the one worth a test: `archive` is a soft delete, so
    /// "no archived workspaces" and "I ignored your flag" render identically —
    /// an empty table. A silently-defaulted flag would be indistinguishable
    /// from a correct answer at exactly the moment it mattered.
    #[tokio::test]
    async fn archived_rows_come_back_only_when_the_request_asks_for_them() {
        use aleph_protocol::workspace::WorkspaceList;

        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            AgentEnvStore::new(crate::gateway::agent_env::AgentEnvStoreConfig {
                db_path: temp.path().join("agent_envs.db"),
                default_profile: "default".to_string(),
            })
            .expect("agent env store"),
        );
        store.load_profiles(std::collections::HashMap::new());
        store.create("retired", "default", None).await.unwrap();
        assert!(store.archive("retired").await.unwrap());

        let ids = |resp: JsonRpcResponse| -> Vec<String> {
            serde_json::from_value::<WorkspaceList>(resp.result.expect("result"))
                .expect("the CLI's projection must parse the real response")
                .workspaces
                .into_iter()
                .map(|w| w.id)
                .collect()
        };

        // No params at all is the default view — the shape every caller sent
        // before this parameter existed.
        let default_view = ids(handle_list(
            JsonRpcRequest::with_id("workspace.list", None, json!(1)),
            store.clone(),
        )
        .await);
        assert!(!default_view.contains(&"retired".to_string()));

        let asked = handle_list(
            JsonRpcRequest::with_id(
                "workspace.list",
                Some(json!({ "include_archived": true })),
                json!(2),
            ),
            store.clone(),
        )
        .await;
        let listed: WorkspaceList =
            serde_json::from_value(asked.result.expect("result")).expect("projection");
        let retired = listed
            .workspaces
            .iter()
            .find(|w| w.id == "retired")
            .expect("the archived workspace must be reachable when asked for");
        assert!(
            retired.is_archived,
            "the Status column has to be able to say `archived`"
        );

        // A misspelled flag is refused, not quietly answered with the narrower
        // view (`deny_unknown_fields` on the params type).
        let typo = handle_list(
            JsonRpcRequest::with_id(
                "workspace.list",
                Some(json!({ "include_arcived": true })),
                json!(3),
            ),
            store,
        )
        .await;
        assert_eq!(typo.error.map(|e| e.code), Some(INVALID_PARAMS));
    }

    /// The round trip that makes `archive` reversible.
    ///
    /// [`an_archived_workspace_is_readable_but_not_writable`] pins the terminal
    /// half; this pins the way out. The assertion that matters is the LAST one:
    /// after `unarchive`, an `update` must land **in the store**. A handler that
    /// reported a restored workspace while the row stayed archived would pass
    /// every response-only check here, and the symptom in production would be a
    /// rename that silently does nothing — `AgentEnvStore::update` filters
    /// `archived = 0` — which is the exact shape the 2026-08-08 round was spent
    /// removing.
    #[tokio::test]
    async fn an_archived_workspace_can_be_restored_and_edited_again() {
        use aleph_protocol::workspace::WorkspaceEnvelope;

        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            AgentEnvStore::new(crate::gateway::agent_env::AgentEnvStoreConfig {
                db_path: temp.path().join("agent_envs.db"),
                default_profile: "default".to_string(),
            })
            .expect("agent env store"),
        );
        store.load_profiles(std::collections::HashMap::new());
        store.create("retired", "default", None).await.unwrap();
        store
            .update("retired", Some("Retired Project"), None, None)
            .await
            .unwrap();
        assert!(store.archive("retired").await.unwrap());

        let unarchive = |id: &str, rpc_id: i32| {
            handle_unarchive(
                JsonRpcRequest::with_id(
                    "workspace.unarchive",
                    Some(json!({ "id": id })),
                    json!(rpc_id),
                ),
                store.clone(),
            )
        };

        // --- the refusal that stood before this verb existed ---------------
        let refused = handle_update(
            JsonRpcRequest::with_id(
                "workspace.update",
                Some(json!({ "id": "retired", "name": "renamed-while-archived" })),
                json!(1),
            ),
            store.clone(),
        )
        .await;
        assert_eq!(
            refused.error.as_ref().map(|e| e.code),
            Some(PERMISSION_DENIED),
            "the archived row must still refuse edits — unarchive is the way \
             back, not a looser update"
        );

        // --- restore, and read the restored row out of the response --------
        let restored = unarchive("retired", 2).await;
        let envelope: WorkspaceEnvelope = serde_json::from_value(
            restored
                .result
                .expect("unarchive must return the restored workspace"),
        )
        .expect("the same projection `get`/`update` use must parse it");
        assert!(
            !envelope.workspace.is_archived,
            "the response must show the state it just produced"
        );
        assert_eq!(
            envelope.workspace.name, "Retired Project",
            "unarchive restores the row, it does not reset it"
        );

        // --- and the store agrees ------------------------------------------
        assert!(
            store
                .get("retired")
                .await
                .unwrap()
                .is_some_and(|ws| !ws.is_archived),
            "`get` filters archived rows, so reaching one through it IS the \
             proof the flag came off"
        );

        // --- THE assertion: writable again ---------------------------------
        let applied = handle_update(
            JsonRpcRequest::with_id(
                "workspace.update",
                Some(json!({ "id": "retired", "name": "Back In Service" })),
                json!(3),
            ),
            store.clone(),
        )
        .await;
        assert!(applied.is_success(), "{:?}", applied.error);
        assert_eq!(
            store.get("retired").await.unwrap().unwrap().name,
            "Back In Service",
            "the edit has to reach the row — a response that says `ok` while \
             the write is filtered away is the bug this verb exists to end"
        );

        // --- idempotent, and honest about a missing id ---------------------
        assert!(
            unarchive("retired", 4).await.is_success(),
            "unarchiving a live row promises `this workspace is active`, and \
             that postcondition already holds"
        );
        assert_eq!(
            unarchive("never-existed", 5)
                .await
                .error
                .as_ref()
                .map(|e| e.code),
            Some(RESOURCE_NOT_FOUND),
        );
    }

    /// `workspace.create` answers with the row that is on disk.
    ///
    /// It used to answer with a locally-mutated copy of what the caller asked
    /// for — a statement of intent wearing the shape of an observation. Two
    /// things were wrong with that, and the second is what made it visible:
    ///
    /// 1. `create` cannot set name or icon, so those are a **second** write,
    ///    and that write only `warn!`s on failure. A workspace whose name never
    ///    persisted was reported back carrying that name.
    /// 2. `create` built `created_at` from `Utc::now()` while the store
    ///    persists whole seconds, so the response carried a precision no later
    ///    read could reproduce — the create response and every subsequent `get`
    ///    disagreed about when the workspace was created. That is what a
    ///    2026-08-09 real-machine QA saw on the wire.
    ///
    /// Asserting **full struct equality** against `handle_get` rather than
    /// field-by-field: the property is "these are the same row", and a field
    /// list is the enumeration mistake — it would have missed `created_at`
    /// exactly the way every reader of this code missed it.
    #[tokio::test]
    async fn create_answers_with_the_stored_row_not_the_requested_one() {
        use aleph_protocol::workspace::WorkspaceEnvelope;

        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            AgentEnvStore::new(crate::gateway::agent_env::AgentEnvStoreConfig {
                db_path: temp.path().join("agent_envs.db"),
                default_profile: "default".to_string(),
            })
            .expect("agent env store"),
        );
        store.load_profiles(std::collections::HashMap::new());

        let unwrap = |resp: JsonRpcResponse| -> WorkspaceDetail {
            serde_json::from_value::<WorkspaceEnvelope>(resp.result.expect("result"))
                .expect("projection")
                .workspace
        };

        let created = unwrap(
            handle_create(
                JsonRpcRequest::with_id(
                    "workspace.create",
                    Some(json!({
                        "id": "crypto",
                        "name": "Crypto Trading",
                        "description": "trading notes",
                        "icon": "\u{1F4B0}",
                    })),
                    json!(1),
                ),
                store.clone(),
            )
            .await,
        );
        let fetched = unwrap(
            handle_get(
                JsonRpcRequest::with_id("workspace.get", Some(json!({ "id": "crypto" })), json!(2)),
                store.clone(),
            )
            .await,
        );

        assert_eq!(
            created, fetched,
            "the create response must BE the stored row, not a description of \
             the request that produced it"
        );
        // Not a false positive: the values asked for did reach the store, so
        // this is not two identical wrongs agreeing with each other.
        assert_eq!(created.name, "Crypto Trading");
        assert_eq!(created.icon.as_deref(), Some("\u{1F4B0}"));
        assert_eq!(created.description.as_deref(), Some("trading notes"));
    }

    /// The partition half, and the half that has to be asserted on the STORE:
    /// a refused unarchive must not flip the flag.
    ///
    /// Same shape as the `archive` case in
    /// [`the_workspace_writes_deny_a_foreign_partition_composed_id`] — the id
    /// is held fixed and the STORE STATE is what varies between the two calls,
    /// so what the comparison isolates is existence and nothing else.
    #[tokio::test]
    async fn a_denied_unarchive_neither_restores_the_row_nor_reveals_it() {
        use crate::gateway::caller_identity::CALLER_USER;

        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            AgentEnvStore::new(crate::gateway::agent_env::AgentEnvStoreConfig {
                db_path: temp.path().join("agent_envs.db"),
                default_profile: "default".to_string(),
            })
            .expect("agent env store"),
        );
        store.load_profiles(std::collections::HashMap::new());

        let as_bob = |rpc_id: i32| {
            CALLER_USER.scope(
                Some("u-bob".to_string()),
                handle_unarchive(
                    JsonRpcRequest::with_id(
                        "workspace.unarchive",
                        Some(json!({ "id": "main__u-alice" })),
                        json!(rpc_id),
                    ),
                    store.clone(),
                ),
            )
        };

        let never_created = as_bob(1).await;
        store
            .create("main__u-alice", "default", None)
            .await
            .unwrap();
        assert!(store.archive("main__u-alice").await.unwrap());
        let now_exists = as_bob(1).await;

        assert_eq!(
            serde_json::to_string(&never_created).unwrap(),
            serde_json::to_string(&now_exists).unwrap(),
            "the refusal must not tell bob whether alice's workspace exists"
        );
        assert_eq!(
            now_exists.error.as_ref().map(|e| e.code),
            Some(RESOURCE_NOT_FOUND)
        );
        assert!(
            store
                .get_including_archived("main__u-alice")
                .await
                .unwrap()
                .expect("the row is still there")
                .is_archived,
            "a denied unarchive must not resurrect the foreign workspace — \
             this assertion is the test; the response above is only half of it"
        );
    }

    /// `create` against an archived id names the way back.
    ///
    /// Without this the operator who archived `crypto` yesterday reads
    /// "already exists" as "someone else has that name", picks `crypto-2`, and
    /// strands the row they meant to reuse — along with the notes and memory
    /// still on disk under the old id. The system knows which collision it is;
    /// this makes it say so.
    ///
    /// The other two assertions are the guard rail. A LIVE collision and the
    /// partition-invisible refusal must stay **byte-identical to each other**,
    /// because that identity is what stops `create` from being an existence
    /// oracle. Widening the archived branch to fire on live rows — the obvious
    /// way to get this wrong — breaks the second assertion.
    #[tokio::test]
    async fn create_names_unarchive_when_the_id_is_held_by_an_archived_workspace() {
        use crate::gateway::caller_identity::CALLER_USER;

        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            AgentEnvStore::new(crate::gateway::agent_env::AgentEnvStoreConfig {
                db_path: temp.path().join("agent_envs.db"),
                default_profile: "default".to_string(),
            })
            .expect("agent env store"),
        );
        store.load_profiles(std::collections::HashMap::new());
        let create = |id: &str| {
            handle_create(
                JsonRpcRequest::with_id(
                    "workspace.create",
                    Some(json!({ "id": id, "name": id })),
                    json!(1),
                ),
                store.clone(),
            )
        };
        let message = |resp: &JsonRpcResponse| {
            resp.error
                .as_ref()
                .map(|e| e.message.clone())
                .expect("a collision is an error")
        };

        // --- archived: the refusal has to be actionable --------------------
        store.create("retired", "default", None).await.unwrap();
        assert!(store.archive("retired").await.unwrap());
        let archived_collision = message(&create("retired").await);
        assert!(
            archived_collision.contains("unarchive"),
            "the refusal must name the verb that undoes this: {archived_collision}"
        );
        assert!(
            archived_collision.contains("archived"),
            "…and say why the id is unavailable: {archived_collision}"
        );

        // --- live: unchanged, and the partition refusal still matches it ---
        store.create("live", "default", None).await.unwrap();
        let live_collision = message(&create("live").await);
        assert!(
            !live_collision.contains("unarchive"),
            "a live workspace has no way back to offer: {live_collision}"
        );

        let denied = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_create(
                    JsonRpcRequest::with_id(
                        "workspace.create",
                        Some(json!({ "id": "live", "name": "planted" })),
                        json!(1),
                    ),
                    store.clone(),
                ),
            )
            .await;
        // `live` is not partition-composed, so bob passes the predicate and
        // gets the genuine collision. The invisible case is the one that must
        // match it — same id, so the message is comparable.
        let invisible = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_create(
                    JsonRpcRequest::with_id(
                        "workspace.create",
                        Some(json!({ "id": "main__u-alice", "name": "planted" })),
                        json!(1),
                    ),
                    store.clone(),
                ),
            )
            .await;
        assert_eq!(
            message(&denied),
            live_collision,
            "an ordinary collision must read the same for every caller"
        );
        assert_eq!(
            message(&invisible),
            id_taken("main__u-alice"),
            "the partition refusal is the collision shape, produced not copied"
        );
    }

    #[test]
    fn test_set_agent_params_with_agent() {
        let json = serde_json::json!({"channel_id": "rpc", "agent_id": "project-x"});
        let params: SetAgentParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.channel_id, "rpc");
        assert_eq!(params.agent_id.as_deref(), Some("project-x"));
    }

    #[test]
    fn test_set_agent_params_unbind() {
        let json = serde_json::json!({"channel_id": "rpc"});
        let params: SetAgentParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.channel_id, "rpc");
        assert!(params.agent_id.is_none());
    }

    // ── channels.set_agent ghost-binding validation (AS-1) ────────────────
    // Mirrors the `agent_switch` tool's helpers (builtin_tools/agent_manage/switch.rs).

    use crate::gateway::agent_env::AgentEnvStoreConfig;
    use crate::gateway::agent_instance::{AgentInstance, AgentInstanceConfig};
    use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
    use tempfile::tempdir;

    fn test_workspace_mgr() -> Arc<AgentEnvStore> {
        let temp = tempdir().unwrap();
        let config = AgentEnvStoreConfig {
            db_path: temp.keep().join("test.db"),
            default_profile: "default".to_string(),
        };
        Arc::new(AgentEnvStore::new(config).unwrap())
    }

    fn test_session_store() -> Arc<dyn crate::gateway::session_store::SessionStore> {
        let temp = tempdir().unwrap();
        let cfg = SessionManagerConfig {
            db_path: temp.keep().join("sessions.db"),
            ..Default::default()
        };
        Arc::new(SessionManager::new(cfg).expect("session manager"))
    }

    fn test_instance(agent_id: &str) -> AgentInstance {
        let root = tempdir().unwrap().keep();
        let config = AgentInstanceConfig {
            agent_id: agent_id.to_string(),
            workspace: root.join("workspace"),
            agent_dir: root.join("state"),
            model: "claude-sonnet-4-5".to_string(),
            ..Default::default()
        };
        AgentInstance::new(config, test_session_store()).expect("instance")
    }

    async fn registry_with(agent_id: &str) -> Arc<AgentRegistry> {
        let registry = Arc::new(AgentRegistry::new());
        registry.register(test_instance(agent_id)).await;
        registry
    }

    fn set_agent_req(channel: &str, agent: Option<&str>) -> JsonRpcRequest {
        let params = match agent {
            Some(a) => json!({"channel_id": channel, "agent_id": a}),
            None => json!({"channel_id": channel}),
        };
        JsonRpcRequest::with_id("channels.set_agent", Some(params), json!(1))
    }

    #[tokio::test]
    async fn set_agent_rejects_ghost_when_registry_present() {
        let wm = test_workspace_mgr();
        let registry = registry_with("trader").await;
        let resp = handle_set_agent(
            set_agent_req("telegram", Some("ghost")),
            Arc::clone(&wm),
            Some(registry),
            None,
        )
        .await;
        assert!(resp.is_error());
        let msg = resp.error.unwrap().message;
        assert!(msg.contains("not found"), "unexpected: {msg}");
        assert!(msg.contains("trader"), "should list available: {msg}");
        // The rejected bind persisted nothing.
        assert!(wm.get_active_agent("telegram").unwrap().is_none());
    }

    #[tokio::test]
    async fn set_agent_binds_existing_agent() {
        let wm = test_workspace_mgr();
        let registry = registry_with("trader").await;
        let resp = handle_set_agent(
            set_agent_req("telegram", Some("trader")),
            Arc::clone(&wm),
            Some(registry),
            None,
        )
        .await;
        assert!(resp.is_success(), "expected success: {:?}", resp.error);
        assert_eq!(
            wm.get_active_agent("telegram").unwrap().as_deref(),
            Some("trader")
        );
    }

    #[tokio::test]
    async fn set_agent_skips_validation_without_registry() {
        // A minimal server with no runtime registry must not block binds
        // (graceful fallback — the prior unchecked behavior).
        let wm = test_workspace_mgr();
        let resp = handle_set_agent(
            set_agent_req("telegram", Some("ghost")),
            Arc::clone(&wm),
            None,
            None,
        )
        .await;
        assert!(resp.is_success(), "expected success: {:?}", resp.error);
        assert_eq!(
            wm.get_active_agent("telegram").unwrap().as_deref(),
            Some("ghost")
        );
    }

    #[tokio::test]
    async fn set_agent_unbind_needs_no_registry() {
        let wm = test_workspace_mgr();
        wm.set_active_agent("telegram", "trader").unwrap();
        // Unbind (agent_id: None) never consults the registry.
        let resp =
            handle_set_agent(set_agent_req("telegram", None), Arc::clone(&wm), None, None).await;
        assert!(resp.is_success(), "expected success: {:?}", resp.error);
        assert!(wm.get_active_agent("telegram").unwrap().is_none());
        // The unbind reports which agent was displaced.
        let result = resp.result.unwrap();
        assert_eq!(result["previous_agent"], json!("trader"));
    }

    #[tokio::test]
    async fn set_agent_reports_previous_and_no_op() {
        let wm = test_workspace_mgr();
        let registry = registry_with("trader").await;

        let first = handle_set_agent(
            set_agent_req("telegram", Some("trader")),
            Arc::clone(&wm),
            Some(Arc::clone(&registry)),
            None,
        )
        .await;
        let first = first.result.unwrap();
        assert_eq!(first["previous_agent"], serde_json::Value::Null);
        assert_eq!(first["no_op"], json!(false));

        // Re-binding to the same agent is a reported no-op, not an error.
        let second = handle_set_agent(
            set_agent_req("telegram", Some("trader")),
            Arc::clone(&wm),
            Some(registry),
            None,
        )
        .await;
        let second = second.result.unwrap();
        assert_eq!(second["previous_agent"], json!("trader"));
        assert_eq!(second["no_op"], json!(true));
    }

    // ── agents.bindings many-to-one shape ─────────────────────────────────

    #[tokio::test]
    async fn agent_bindings_reports_all_channels_per_agent() {
        let wm = test_workspace_mgr();
        // Many-to-one: two channels bound to the same agent must BOTH appear
        // (the old one-channel-per-agent map collapsed them to one).
        wm.set_active_agent("telegram", "trader").unwrap();
        wm.set_active_agent("discord", "trader").unwrap();

        let req = JsonRpcRequest::with_id("agents.bindings", None, json!(1));
        let resp = handle_agent_bindings(req, Arc::clone(&wm)).await;
        assert!(resp.is_success(), "expected success: {:?}", resp.error);
        let result = resp.result.unwrap();
        assert_eq!(
            result["bindings"]["trader"],
            json!(["discord", "telegram"]),
            "all bound channels should be listed, sorted"
        );
    }
}
