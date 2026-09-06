//! Project room handlers — the `projects.*` RPC surface.
//!
//! Backed by [`crate::projects::ProjectStore`]. These handlers are pure I/O
//! (R4): they validate params, gate, delegate to the store, and shape JSON.
//!
//! ## One table, two views
//!
//! The Panel's "recent working directory" picker and the project-room list are
//! the same rows (human ruling 2026-08-06). `add` / `create_blank` / `touch`
//! are the picker's entry points; `create` / `rename` / `archive` /
//! `member.*` are the room's. Nothing distinguishes them but whether the row
//! has a `workspace_path` — and `bind_workspace` is the verb that moves a row
//! between the two views in either direction.
//!
//! ## The five writers of `workspace_path` — four gated, one exempt
//!
//! `add`, `create_blank` and `bind_workspace` are three of the four that
//! CHOOSE a directory. Since P2 (Task 7) that column is the default working
//! directory of every run in the room, so all three go through
//! [`require_directory_choice`] — the same config-tier predicate the per-turn
//! `project_root` override uses. Adding a sixth writer without a gate reopens
//! "register a folder, then chat in it", a two-step path to an arbitrary
//! server directory with both steps legal.
//!
//! The fourth chooser is `builtin_tools::project_manage`'s `bind_workspace`
//! action, which calls `ProjectStore::bind_workspace` DIRECTLY rather than
//! routing through this module. It is gated, but **by a different predicate,
//! and that is deliberate**: `require_operator_tier()` reads
//! `TurnContext::caller_is_operator()`, because the ambient
//! `caller_may_choose_directory()` is dead inside a run — the task-local is
//! not re-established across the harness spawn, which is exactly what made
//! that predicate constant-true on this face until 2026-08-30. `TurnContext`
//! is per TURN rather than per connection and the gateway has already
//! resolved the connection's authority into it before any tool executes.
//! It fires only when a path is NAMED, so RELEASING a binding stays reachable
//! from a session a bad binding broke.
//!
//! The fifth is `execution_engine::run_loop::inner`, which auto-registers a
//! run's `workspace_override` into the catalogue (via `ProjectStore::add_for`)
//! so a CLI/programmatic cwd shows up in the desktop picker next time. It is
//! exempt, and the exemption is an invariant rather than an oversight: **it
//! never introduces a directory, it only records one the run is already
//! executing in.** It grants no reach — it makes an already-held cwd visible
//! in a list.
//!
//! Which means the authority question is settled UPSTREAM, at whichever
//! producer decided the run's cwd, and there are several: `handlers::agent`
//! (gated — a caller-named `project_root` needs
//! `caller_may_choose_directory()`; a room's bound path was gated when bound),
//! a channel's configured `default_workspace` (operator-written config, and
//! room runs take the by-id branch before this one anyway), a resumed run's
//! persisted path, a workspace inherited by a subagent or team member (team
//! WORKTREE runs are excluded here outright, by the `team_worktree_path`
//! metadata key). Do not read that list as exhaustive — read the rule:
//!
//! - a new writer of `workspace_path` that CHOOSES a directory owes
//!   [`require_directory_choice`];
//! - a new SOURCE of `workspace_override` owes a gate at its own choice point,
//!   because this line will faithfully catalogue whatever it produces.
//!
//! The count is load-bearing: leaving it at three means whoever adds a
//! genuinely new writer believes they are the fourth when they are the fifth,
//! and looks for three precedents when there are four.
//!
//! **And this paragraph went stale exactly the way it warns about.** It said
//! "four writers — three gated" from P2 until 2026-08-30, when
//! `project_manage(bind_workspace)` landed as the fourth chooser and nothing
//! here changed; SECURITY.md's copy of the count went stale in the same
//! silence. Nothing catches this — a count of members is prose, and prose has
//! no compiler. Two consequences worth carrying: the RULE below is the part
//! that survives, so read it rather than the number; and when you add the
//! sixth, grep `workspace_path` and `bind_workspace` for every writer rather
//! than trusting this list, then fix BOTH this doc and SECURITY.md's
//! «The workspace binding is a privilege» bullet, which names this module as
//! its authority.
//!
//! ## The four verdicts (spec §6.3)
//!
//! | situation | response |
//! |---|---|
//! | project does not exist | `RESOURCE_NOT_FOUND` "project not found: {id}" |
//! | project exists, caller is not on its roster | **the same, byte for byte** |
//! | caller is a member but not owner/admin, doing an owner-level operation | `PERMISSION_DENIED` "not the project owner" |
//! | the member being added is not an active `users` row | `INVALID_PARAMS` "unknown user: {id}" |
//!
//! The first two collapsing into one response is deliberate: distinguishing
//! them would turn `projects.get` into a cross-user existence oracle. The
//! third one does leak "this project exists" — but the caller is already on
//! its roster, so they knew. That is not an oracle.
//!
//! ## The single admission point
//!
//! Every handler that takes a caller-supplied project id goes through
//! [`gate_project`] BEFORE touching anything else, and owner-level operations
//! additionally through [`require_owner`]. Do not write
//! `project.owner_user_id == caller` at a call site — that is the second
//! derivation `gateway::visibility` exists to prevent, and it would miss both
//! the fail-closed and the no-oracle halves.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::super::protocol::{
    JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, PERMISSION_DENIED,
    RESOURCE_NOT_FOUND,
};
use super::parse_params;
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::events::ChangeKind;
use crate::gateway::security::store::{SecurityStore, UserRole, UserStatus};
use crate::gateway::visibility;
use crate::projects::{self, Project, ProjectError, ProjectStatus, ProjectStore};
use crate::sync_primitives::Arc;

/// Serializable view of a project room.
///
/// The shape itself lives in `aleph_protocol` (as
/// [`aleph_protocol::projects::ProjectRow`]) rather than here, for the reason
/// `projects.channel.*` already put its shapes there: `aleph-cli` must not
/// depend on `alephcore`, so `aleph projects list` had no way to name this row
/// and would have hand-written a copy of it. The Panel's
/// `api::projects::ProjectInfo` was such a copy until it became a `pub use` of
/// this same row, and a hand-copied client row is how `aleph providers list`
/// came to render two columns (`type`, `default`) the server had never sent.
///
/// The alias is deliberately kept: this name is what the seven construction
/// sites below and `builtin_tools::project_manage` already read, and the
/// direction that matters is that the response is **built from** the contract
/// type. A test that only parses a response proves the client's fields are a
/// subset of what was sent, never that the two are the same set.
///
/// `workspace_path` is `null` for a room that is not bound to a folder — the
/// ordinary shape for a room created through `projects.create`, and the reason
/// it is an `Option` rather than the pre-P2 `path: String` that spelled
/// "unbound" as `""`.
///
/// When it is set, [`render_project`] runs it through
/// [`crate::utils::paths::display_string`]. The stored value comes out of
/// `std::fs::canonicalize`, so on Windows it carries the `\\?\`
/// extended-length prefix — which is right for the filesystem layer and wrong
/// in the project chip, the recents list, and every refusal message that
/// echoes a path back. The row keeps the canonical bytes; only this projection
/// is simplified, and the round trip is safe because every path the client
/// sends back is re-canonicalised by `ProjectStore::canonical_dir`.
pub type ProjectView = aleph_protocol::projects::ProjectRow;

/// Project a stored room plus its roster into the wire row.
///
/// A free function rather than `ProjectView::render`, because Rust does not
/// allow an inherent impl on a type from another crate. Same body, same
/// callers.
pub(crate) fn render_project(p: Project, member_ids: Vec<String>) -> ProjectView {
    ProjectView {
        id: p.id,
        name: p.name,
        owner_user_id: p.owner_user_id,
        workspace_path: p
            .workspace_path
            .map(|w| crate::utils::paths::display_string(&w)),
        status: p.status.as_str().to_string(),
        member_ids,
        created_at: p.created_at,
        updated_at: p.updated_at,
        last_used_at: p.last_used_at,
    }
}

// ============================================================================
// Gates
// ============================================================================

/// The response an unreachable project produces — whether it does not exist,
/// belongs to a roster the caller is not on, or could not be resolved at all.
///
/// One constructor so the three causes stay indistinguishable. The wording is
/// the one `projects.get` already returned for a missing id before P2, so a
/// single-user deployment sees byte-identical output to before.
#[must_use]
pub(super) fn project_not_found(id: Option<Value>, project_id: &str) -> JsonRpcResponse {
    JsonRpcResponse::error(
        id,
        RESOURCE_NOT_FOUND,
        format!("project not found: {project_id}"),
    )
}

/// The single admission point for every addressed `projects.*` handler.
///
/// A thin wrapper over [`crate::projects::authz::project_for`], which is the
/// shared derivation: this face supplies the ambient caller, the tool face
/// supplies its own. Fails closed on a store error, and answers a refusal
/// with the same not-found shape absence produces — see that module's doc.
#[allow(clippy::result_large_err)] // house shape for Result<_, JsonRpcResponse> gates
pub(super) fn gate_project(
    store: &ProjectStore,
    id: Option<Value>,
    project_id: &str,
) -> Result<Project, JsonRpcResponse> {
    crate::projects::authz::project_for(
        store,
        project_id,
        visibility::visible_owner_filter().as_deref(),
    )
    .ok_or_else(|| project_not_found(id, project_id))
}

/// Separate a room's owner from its ordinary members. Assumes
/// [`gate_project`] already ran — this is NOT a visibility check, and calling
/// it alone would answer "are you the owner" about a project the caller
/// cannot see.
///
/// Org admins pass for any room (spec §6.3: owner changes are an admin
/// operation). An unrestricted caller — cron, A2A, an in-process test — passes
/// unconditionally, the same first arm every P1 predicate opens with. The
/// admin lookup lives here rather than in `authz` so that module depends on
/// nothing but the project row.
#[allow(clippy::result_large_err)] // house shape for Result<_, JsonRpcResponse> gates
fn require_owner(
    users: &SecurityStore,
    id: Option<Value>,
    project: &Project,
) -> Result<(), JsonRpcResponse> {
    let actor = visibility::visible_owner_filter();
    let is_admin = actor.as_deref().is_some_and(|caller| {
        matches!(
            users.get_user(caller),
            Ok(Some(u)) if u.role == UserRole::Admin && u.status == UserStatus::Active
        )
    });
    if crate::projects::authz::is_owner(project, actor.as_deref(), is_admin) {
        return Ok(());
    }
    Err(JsonRpcResponse::error(
        id,
        PERMISSION_DENIED,
        "not the project owner",
    ))
}

/// Reject a caller who may not point a project at an arbitrary server folder.
///
/// The three verbs that write `workspace_path` — [`handle_add`],
/// [`handle_create_blank`], [`handle_bind_workspace`] — share this with the
/// per-run `project_root` override in
/// [`crate::gateway::handlers::agent::build_run_request`]. Since P2 a room's
/// bound folder IS the default cwd of every member's run, so a writer weaker
/// than the reader would be a two-step path to the same place with both steps
/// legal: register a folder, then chat in it.
///
/// Orthogonal to [`require_owner`], which asks "is this room yours to
/// reconfigure". This asks "may this connection name server-side paths at
/// all" — a room's owner connected from a chat-tier LAN device fails here and
/// passes there.
#[allow(clippy::result_large_err)] // house shape for Result<_, JsonRpcResponse> gates
fn require_directory_choice(id: Option<Value>) -> Result<(), JsonRpcResponse> {
    if crate::gateway::caller_identity::caller_may_choose_directory() {
        return Ok(());
    }
    Err(JsonRpcResponse::error(
        id,
        PERMISSION_DENIED,
        "choosing a working directory requires config-tier authorization or a \
         local (loopback) connection",
    ))
}

/// Reject a roster mutation naming somebody who is not an active principal.
///
/// A thin `JsonRpcResponse` wrapper over
/// [`crate::projects::authz::is_active_principal`] — the decision core (a
/// deactivated user reads as unknown, a store error fails closed) lives
/// there so this face and the tool face
/// (`builtin_tools::project_manage::ProjectManageTool::require_known_user`)
/// ask the same question rather than each keeping their own answer.
#[allow(clippy::result_large_err)] // house shape for Result<_, JsonRpcResponse> gates
fn require_known_user(
    users: &SecurityStore,
    id: Option<Value>,
    user_id: &str,
) -> Result<(), JsonRpcResponse> {
    if crate::projects::authz::is_active_principal(users, user_id) {
        Ok(())
    } else {
        Err(JsonRpcResponse::error(
            id,
            INVALID_PARAMS,
            crate::projects::authz::unknown_user_refusal(user_id),
        ))
    }
}

// ============================================================================
// projects.list
// ============================================================================

#[derive(Debug, Default, Deserialize)]
pub struct ListParams {
    /// Archived rooms are hidden by default — otherwise `projects.archive`
    /// would have no observable effect on the surface that lists them.
    #[serde(default)]
    pub include_archived: bool,
}

pub async fn handle_list(request: JsonRpcRequest, store: Arc<ProjectStore>) -> JsonRpcResponse {
    let params: ListParams = if request.params.is_some() {
        match parse_params(&request) {
            Ok(p) => p,
            Err(e) => return e,
        }
    } else {
        ListParams::default()
    };

    let projects = match store.list() {
        Ok(p) => p,
        Err(e) => return project_error_response(request.id, e),
    };
    let mut rosters = match store.rosters() {
        Ok(r) => r,
        Err(e) => return project_error_response(request.id, e),
    };

    let view: Vec<ProjectView> = projects
        .into_iter()
        .filter(|p| params.include_archived || p.status == ProjectStatus::Active)
        .filter(|p| visibility::project_visible(&p.id))
        .map(|p| {
            let members = rosters.remove(&p.id).unwrap_or_default();
            render_project(p, members)
        })
        .collect();
    // The envelope is a wire key too, and it is usually the last hand-copied
    // part: the rows got a contract type while `{"projects": …}` stayed a
    // literal in the handler and again in every client. Constructing
    // `ProjectListResult` is how `aleph projects list` learns the key rather
    // than guessing it.
    JsonRpcResponse::success(
        request.id,
        json!(aleph_protocol::projects::ProjectListResult { projects: view }),
    )
}

// ============================================================================
// projects.create
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateParams {
    pub name: String,
}

/// Create an unbound room. Creation surfaces have no addressed record to gate
/// on — the new row is stamped with the caller instead, and the store puts the
/// owner on the roster in the same call. Binding a workspace is a separate,
/// owner-level operation.
pub async fn handle_create(
    request: JsonRpcRequest,
    store: Arc<ProjectStore>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: CreateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let owner = crate::scope::ambient_owner();
    match store.create(&params.name, owner.as_deref(), None) {
        Ok(project) => {
            let members = store.members(&project.id).unwrap_or_default();
            projects::events::publish_changed(&event_bus, &project.id, ChangeKind::Created, None);
            JsonRpcResponse::success(
                request.id,
                json!({ "project": render_project(project, members) }),
            )
        }
        Err(e) => project_error_response(request.id, e),
    }
}

// ============================================================================
// projects.add
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct AddParams {
    pub path: String,
    #[serde(default)]
    pub name: Option<String>,
}

/// Register an existing folder — the picker's write path. Creation surface,
/// same ruling as [`handle_create`]: `ProjectStore::add` resolves the caller
/// off the ambient scope and collapses onto THEIR existing row for that path,
/// never onto somebody else's — guaranteed by `find_by_path_for`'s
/// owner-scoped comparison, which resolves an unset `owner_user_id` to the
/// fixed `OWNER_USER_ID` constant rather than to whichever caller happens to
/// be asking (see that method's doc for the query this claim depends on).
///
/// Gated by [`require_directory_choice`]: the row this writes carries a
/// `workspace_path`, which since P2 becomes a run's cwd.
pub async fn handle_add(
    request: JsonRpcRequest,
    store: Arc<ProjectStore>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: AddParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Err(denial) = require_directory_choice(request.id.clone()) {
        return denial;
    }
    let path = PathBuf::from(&params.path);
    match store.add(&path, params.name) {
        Ok(project) => {
            let members = store.members(&project.id).unwrap_or_default();
            // `Updated`, not `Created`: `ProjectStore::add_for` can collapse
            // onto an existing row for the same path (see its own doc) rather
            // than insert a new one, so `Created` would overclaim on the
            // common "re-add a folder already in the picker" path.
            projects::events::publish_changed(&event_bus, &project.id, ChangeKind::Updated, None);
            JsonRpcResponse::success(
                request.id,
                json!({ "project": render_project(project, members) }),
            )
        }
        Err(e) => project_error_response(request.id, e),
    }
}

// ============================================================================
// projects.create_blank
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateBlankParams {
    pub parent: String,
    pub name: String,
}

/// Create a folder and register it. Same gate as [`handle_add`], and a
/// stronger reason for it: this one also *creates* a directory server-side.
pub async fn handle_create_blank(
    request: JsonRpcRequest,
    store: Arc<ProjectStore>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: CreateBlankParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Err(denial) = require_directory_choice(request.id.clone()) {
        return denial;
    }
    let parent = PathBuf::from(&params.parent);
    match store.create_blank(&parent, &params.name) {
        Ok(project) => {
            let members = store.members(&project.id).unwrap_or_default();
            projects::events::publish_changed(&event_bus, &project.id, ChangeKind::Created, None);
            JsonRpcResponse::success(
                request.id,
                json!({ "project": render_project(project, members) }),
            )
        }
        Err(e) => project_error_response(request.id, e),
    }
}

// ============================================================================
// projects.get
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct GetParams {
    pub id: String,
}

pub async fn handle_get(request: JsonRpcRequest, store: Arc<ProjectStore>) -> JsonRpcResponse {
    let params: GetParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let project = match gate_project(&store, request.id.clone(), &params.id) {
        Ok(p) => p,
        Err(denial) => return denial,
    };
    let members = match store.members(&project.id) {
        Ok(m) => m,
        Err(e) => return project_error_response(request.id, e),
    };
    JsonRpcResponse::success(
        request.id,
        json!({ "project": render_project(project, members) }),
    )
}

// ============================================================================
// projects.rename
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct RenameParams {
    pub id: String,
    pub name: String,
}

pub async fn handle_rename(
    request: JsonRpcRequest,
    store: Arc<ProjectStore>,
    users: Arc<SecurityStore>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: RenameParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let project = match gate_project(&store, request.id.clone(), &params.id) {
        Ok(p) => p,
        Err(denial) => return denial,
    };
    if let Err(denial) = require_owner(&users, request.id.clone(), &project) {
        return denial;
    }
    match store.rename(&params.id, &params.name) {
        Ok(renamed) => {
            let members = store.members(&renamed.id).unwrap_or_default();
            projects::events::publish_changed(&event_bus, &renamed.id, ChangeKind::Updated, None);
            JsonRpcResponse::success(
                request.id,
                json!({ "project": render_project(renamed, members) }),
            )
        }
        Err(e) => project_error_response(request.id, e),
    }
}

// ============================================================================
// projects.archive
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct ArchiveParams {
    pub id: String,
}

/// Archive a room. Not deletion — the roster and the memory partition survive,
/// which is why this is reversible in a way `projects.remove` is not.
pub async fn handle_archive(
    request: JsonRpcRequest,
    store: Arc<ProjectStore>,
    users: Arc<SecurityStore>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: ArchiveParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let project = match gate_project(&store, request.id.clone(), &params.id) {
        Ok(p) => p,
        Err(denial) => return denial,
    };
    if let Err(denial) = require_owner(&users, request.id.clone(), &project) {
        return denial;
    }
    match store.archive(&params.id) {
        Ok(()) => {
            // `Updated`, not `Deleted`: archiving is reversible and keeps the
            // roster (see this handler's own doc), matching `WorkspaceChanged`'s
            // documented archive/restore convention — `Deleted` would be a
            // claim a client could act on and be wrong about.
            projects::events::publish_changed(&event_bus, &params.id, ChangeKind::Updated, None);
            JsonRpcResponse::success(
                request.id,
                json!({ "id": params.id, "status": ProjectStatus::Archived.as_str() }),
            )
        }
        Err(e) => project_error_response(request.id, e),
    }
}

// ============================================================================
// projects.bind_workspace
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct BindWorkspaceParams {
    pub id: String,
    /// Absent or JSON `null` unbinds — the room keeps its roster, memory and
    /// history and its runs go back to the agent's default workspace. That is
    /// also the repair for a folder that has gone missing, which
    /// `build_run_request` refuses to run in.
    #[serde(default)]
    pub path: Option<String>,
}

/// Point a room at a folder (spec §8, Task 7). Every member's run in this room
/// then defaults its working directory to it — which is why this needs BOTH
/// gates:
///
/// - [`require_owner`] — redirecting where the whole room works is an owner
///   decision, not a member's. `handle_create`'s doc has said so since Task 3.
/// - [`require_directory_choice`] — naming a server-side path at all is
///   config-tier, exactly as it is for a per-turn `project_root`.
///
/// Absolute-ness, existence and canonicalisation are the store's
/// (`ProjectStore::bind_workspace` → `canonical_dir`), so the path this writes
/// is already resolved — no `..` survives to the run.
pub async fn handle_bind_workspace(
    request: JsonRpcRequest,
    store: Arc<ProjectStore>,
    users: Arc<SecurityStore>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: BindWorkspaceParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let project = match gate_project(&store, request.id.clone(), &params.id) {
        Ok(p) => p,
        Err(denial) => return denial,
    };
    if let Err(denial) = require_owner(&users, request.id.clone(), &project) {
        return denial;
    }
    // Only when actually naming a path: unbinding is a de-escalation and must
    // stay reachable from the surface that got stuck. A chat-tier member who
    // can see the room but not choose folders can still un-stick it.
    let path = params
        .path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(PathBuf::from);
    if path.is_some() {
        if let Err(denial) = require_directory_choice(request.id.clone()) {
            return denial;
        }
    }
    match store.bind_workspace(&params.id, path.as_deref()) {
        Ok(bound) => {
            let members = store.members(&bound.id).unwrap_or_default();
            projects::events::publish_changed(&event_bus, &bound.id, ChangeKind::Updated, None);
            JsonRpcResponse::success(
                request.id,
                json!({ "project": render_project(bound, members) }),
            )
        }
        Err(e) => project_error_response(request.id, e),
    }
}

// ============================================================================
// projects.remove
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct RemoveParams {
    pub id: String,
}

pub async fn handle_remove(
    request: JsonRpcRequest,
    store: Arc<ProjectStore>,
    users: Arc<SecurityStore>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: RemoveParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let project = match gate_project(&store, request.id.clone(), &params.id) {
        Ok(p) => p,
        Err(denial) => return denial,
    };
    if let Err(denial) = require_owner(&users, request.id.clone(), &project) {
        return denial;
    }
    match store.remove(&params.id) {
        Ok(()) => {
            projects::events::publish_changed(&event_bus, &params.id, ChangeKind::Deleted, None);
            JsonRpcResponse::success(request.id, json!({ "id": params.id, "removed": true }))
        }
        Err(e) => project_error_response(request.id, e),
    }
}

// ============================================================================
// projects.touch
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct TouchParams {
    pub id: String,
}

/// Bump `last_used_at`. Member-level on purpose: every member entering the
/// room reorders it in their own picker, and that is not an owner decision.
pub async fn handle_touch(request: JsonRpcRequest, store: Arc<ProjectStore>) -> JsonRpcResponse {
    let params: TouchParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Err(denial) = gate_project(&store, request.id.clone(), &params.id) {
        return denial;
    }
    match store.touch(&params.id) {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "id": params.id })),
        Err(e) => project_error_response(request.id, e),
    }
}

// ============================================================================
// projects.room_session
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct RoomSessionParams {
    pub id: String,
    /// The agent whose session the caller would open. Only consulted when
    /// nobody has claimed the room's session yet; once claimed, the stored key
    /// names its own agent and this is ignored. Absent normalises to the
    /// default agent.
    #[serde(default)]
    pub agent_id: Option<String>,
}

/// Get-or-create the room's canonical chat session key.
///
/// This is what makes a project room ONE conversation. Before it, the
/// `project_id → session_key` map lived in each browser's `localStorage`, so
/// the second member to enter a room found nothing, opened a fresh session and
/// talked to the agent alone — the members shared a memory partition and a
/// workspace but never a transcript.
///
/// The claim is atomic inside [`ProjectStore::claim_session_key`], so two
/// members opening the room simultaneously converge instead of forking. The
/// candidate is only a proposal: a caller whose default agent differs still
/// receives the key that was actually claimed.
pub async fn handle_room_session(
    request: JsonRpcRequest,
    store: Arc<ProjectStore>,
) -> JsonRpcResponse {
    let params: RoomSessionParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let project = match gate_project(&store, request.id.clone(), &params.id) {
        Ok(p) => p,
        Err(denial) => return denial,
    };
    let candidate = crate::gateway::router::SessionKey::project_room(
        params.agent_id.unwrap_or_default(),
        &project.id,
    )
    .to_key_string();
    match store.claim_session_key(&project.id, &candidate) {
        Ok(session_key) => JsonRpcResponse::success(
            request.id,
            json!({ "id": project.id, "session_key": session_key }),
        ),
        // A room that vanished between the gate and the claim is unreachable
        // for the same reason a room the caller cannot see is — same response.
        Err(ProjectError::NotFound(_)) => project_not_found(request.id, &params.id),
        Err(e) => project_error_response(request.id, e),
    }
}

// ============================================================================
// projects.member.add / projects.member.remove / projects.member.list
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct MemberParams {
    pub id: String,
    pub user_id: String,
}

pub async fn handle_member_add(
    request: JsonRpcRequest,
    store: Arc<ProjectStore>,
    users: Arc<SecurityStore>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: MemberParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let project = match gate_project(&store, request.id.clone(), &params.id) {
        Ok(p) => p,
        Err(denial) => return denial,
    };
    if let Err(denial) = require_owner(&users, request.id.clone(), &project) {
        return denial;
    }
    if let Err(denial) = require_known_user(&users, request.id.clone(), &params.user_id) {
        return denial;
    }
    match store.add_member(&params.id, &params.user_id) {
        Ok(()) => {
            // Authority-change audit (round-5 ⑦): the roster IS the
            // visibility predicate, so adding a member is a grant.
            if let Some(log) = crate::security::audit::global() {
                log.log(crate::security::audit::AuditEntry::authority_change(
                    crate::gateway::caller_identity::current_caller_user(),
                    format!("projects.member.add: {} → {}", params.user_id, params.id),
                ))
                .await;
            }
            // No `affected_user`: the newly-added member is already on the
            // roster by the time this publishes (`add_member` republishes
            // inside its own write lock), so the ordinary roster-membership
            // arm already admits them — unlike the removal below, there is
            // no gap for the carve-out to close.
            projects::events::publish_changed(&event_bus, &params.id, ChangeKind::Updated, None);
            member_list_response(request.id, &store, &params.id)
        }
        Err(e) => project_error_response(request.id, e),
    }
}

pub async fn handle_member_remove(
    request: JsonRpcRequest,
    store: Arc<ProjectStore>,
    users: Arc<SecurityStore>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: MemberParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let project = match gate_project(&store, request.id.clone(), &params.id) {
        Ok(p) => p,
        Err(denial) => return denial,
    };
    if let Err(denial) = require_owner(&users, request.id.clone(), &project) {
        return denial;
    }
    // The decision core is `authz::may_remove_member` (the owner is the one
    // member who cannot be dropped — the roster IS the visibility predicate,
    // so removing them would make the room invisible to the only person who
    // can archive or delete it); this face and the tool face
    // (`ProjectManageTool::require_removable`) both ask it rather than each
    // re-typing `== project.owner_user_id`.
    if !crate::projects::authz::may_remove_member(&project, &params.user_id) {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            crate::projects::authz::OWNER_REMOVAL_REFUSAL,
        );
    }
    match store.remove_member(&params.id, &params.user_id) {
        Ok(changed) => {
            // Authority-change audit (round-5 ⑦, gated by `changed` in T04):
            // removing a member revokes their view of the room — but only
            // when a row was actually deleted. `params.user_id` naming
            // somebody who was never on the roster is not a revocation and
            // must not be recorded as one.
            if changed {
                if let Some(log) = crate::security::audit::global() {
                    log.log(crate::security::audit::AuditEntry::authority_change(
                        crate::gateway::caller_identity::current_caller_user(),
                        format!("projects.member.remove: {} ← {}", params.user_id, params.id),
                    ))
                    .await;
                }
            }
            // `affected_user`: the roster projection no longer admits
            // `params.user_id` by the time this publishes
            // (`remove_member` republishes inside its own write lock), so
            // without naming them here they would never learn they were
            // dropped — see `ProjectsChanged::affected_user`'s doc. Named
            // ONLY when `changed`: a bystander who was never seated must
            // not be told they were dropped.
            projects::events::publish_changed(
                &event_bus,
                &params.id,
                ChangeKind::Updated,
                changed.then_some(params.user_id.as_str()),
            );
            member_list_response(request.id, &store, &params.id)
        }
        Err(e) => project_error_response(request.id, e),
    }
}

pub async fn handle_member_list(
    request: JsonRpcRequest,
    store: Arc<ProjectStore>,
) -> JsonRpcResponse {
    let params: GetParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Err(denial) = gate_project(&store, request.id.clone(), &params.id) {
        return denial;
    }
    member_list_response(request.id, &store, &params.id)
}

// ============================================================================
// Helpers
// ============================================================================

/// The shared success shape of all three roster verbs — a mutation answers
/// with the roster it produced, so a caller never has to follow up with a
/// second round trip to learn what it now is.
fn member_list_response(
    id: Option<Value>,
    store: &ProjectStore,
    project_id: &str,
) -> JsonRpcResponse {
    match store.members(project_id) {
        Ok(members) => {
            JsonRpcResponse::success(id, json!({ "id": project_id, "member_ids": members }))
        }
        Err(e) => project_error_response(id, e),
    }
}

pub(super) fn project_error_response(
    id: Option<serde_json::Value>,
    err: ProjectError,
) -> JsonRpcResponse {
    let (code, msg) = match &err {
        ProjectError::NotFound(_) => (RESOURCE_NOT_FOUND, err.to_string()),
        ProjectError::NotAbsolute(_)
        | ProjectError::NotDirectory(_)
        | ProjectError::InvalidName(_) => (INVALID_PARAMS, err.to_string()),
        ProjectError::AlreadyExists(_) => (INVALID_PARAMS, err.to_string()),
        // Not `RESOURCE_NOT_FOUND`: the caller named a project they own and
        // can see, so there is no existence to leak — and a refusal that names
        // the alternative (`archive`) is the only thing that makes the
        // boundary actionable.
        ProjectError::Invalid(_) => (INVALID_PARAMS, err.to_string()),
        _ => (INTERNAL_ERROR, err.to_string()),
    };
    JsonRpcResponse::error(id, code, msg)
}

// ============================================================================
// projects.workspace.list / projects.workspace.read
// ============================================================================
//
// A read-only browse of the directory a room is bound to. Two RPCs, one gate
// order, and four separate reasons a path can be refused — kept apart on
// purpose, because folding them together is how a browse surface turns into
// an existence oracle for the filesystem.
//
// The order is: room visibility (`gate_project`) -> is the room bound at all
// -> does the requested path resolve INSIDE the bound root -> is it denied by
// the credential / `deny_read_globs` floor. Each answers a different question,
// and only the first two are safe to describe to the caller.

/// Largest slice of a file `projects.workspace.read` will return.
const WORKSPACE_READ_MAX_BYTES: usize = 64 * 1024;

/// How much of a file is sniffed for NUL before calling it binary.
const WORKSPACE_BINARY_SNIFF_BYTES: usize = 8 * 1024;

#[derive(Debug, Deserialize)]
pub struct WorkspaceListParams {
    pub project_id: String,
    /// Path relative to the bound root. Absent, empty, or `"."` means the
    /// root itself.
    #[serde(default)]
    pub rel_path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WorkspaceReadParams {
    pub project_id: String,
    pub rel_path: String,
}

/// One directory entry as the Panel renders it.
#[derive(Debug, Serialize)]
struct WorkspaceEntry {
    name: String,
    is_dir: bool,
    /// Byte size for a file; `0` for a directory (not the directory's
    /// recursive weight — this surface never walks).
    size: u64,
}

/// Why a workspace path could not be served.
///
/// Separate from the JSON-RPC response so the two callers map it once, and so
/// the mapping is reviewable in one place: `Outside` is the caller's mistake
/// and says so, while `Denied` and `Missing` deliberately collapse into the
/// SAME not-found shape. A denied file must not be distinguishable from an
/// absent one, or the read-denial floor becomes a way to enumerate which
/// secrets exist.
enum PathRefusal {
    /// Resolved outside the bound root: `..`, an absolute path, or a symlink
    /// pointing out of the tree.
    Outside,
    /// Absent, unreadable, or matched by the read-denial floor.
    Missing,
}

/// Resolve `rel` under `root`, refusing anything that escapes or is denied.
///
/// Both sides are canonicalized by the same call before comparison, and the
/// comparison runs on the canonical (verbatim, `\?\`-prefixed on Windows)
/// forms — never on a display-converted one. `utils::paths::display_string`
/// is a PARTIAL conversion, so converting each side once and then comparing
/// flips a legitimate path from admitted to refused.
///
/// `canonicalize` is a pure lookup: it never creates the directory it is
/// asked about, which is what makes it safe on a diagnostic surface.
fn resolve_in_workspace(root: &std::path::Path, rel: &str) -> Result<PathBuf, PathRefusal> {
    let canonical_root = root.canonicalize().map_err(|_| PathRefusal::Missing)?;

    let trimmed = rel.trim();
    let candidate = if trimmed.is_empty() || trimmed == "." {
        canonical_root.clone()
    } else {
        // `Path::join` with an absolute operand REPLACES the base rather than
        // appending to it, so an absolute `rel_path` would silently escape
        // here. The containment test below catches it either way; rejecting
        // it outright keeps the stated reason honest.
        let joined = std::path::Path::new(trimmed);
        if joined.is_absolute() {
            return Err(PathRefusal::Outside);
        }
        canonical_root.join(joined)
    };

    // Resolves symlinks, so a link inside the root pointing outside it fails
    // containment rather than passing on its spelling.
    let canonical = candidate.canonicalize().map_err(|_| PathRefusal::Missing)?;
    if !canonical.starts_with(&canonical_root) {
        return Err(PathRefusal::Outside);
    }

    // The same floor `file_read` and the OS sandbox drivers enforce: Aleph
    // own credential surface plus the operator `[sandbox] deny_read_globs`.
    // Reusing that reader is the point — a second denial list here would be a
    // second answer to "may this be read", and the two would drift.
    let denied = crate::builtin_tools::file_ops::get_denied_paths();
    if crate::builtin_tools::file_ops::path_is_denied(&canonical, &denied) {
        return Err(PathRefusal::Missing);
    }
    Ok(canonical)
}

/// Map a [`PathRefusal`] onto the wire.
fn workspace_refusal(id: Option<Value>, refusal: &PathRefusal, rel_path: &str) -> JsonRpcResponse {
    match refusal {
        PathRefusal::Outside => JsonRpcResponse::error(
            id,
            PERMISSION_DENIED,
            "Path resolves outside the project workspace".to_string(),
        ),
        PathRefusal::Missing => JsonRpcResponse::error(
            id,
            RESOURCE_NOT_FOUND,
            format!("No such path in this workspace: {rel_path}"),
        ),
    }
}

/// Handle `projects.workspace.list`.
///
/// An unbound room answers `{"root_bound": false, "entries": []}` rather than
/// an error: not having chosen a folder is a state, not a failure, and the
/// Panel renders a bind prompt for it. A room the caller cannot see is
/// refused by `gate_project` with the same not-found shape a nonexistent room
/// produces.
pub async fn handle_workspace_list(
    request: JsonRpcRequest,
    store: Arc<ProjectStore>,
) -> JsonRpcResponse {
    let params: WorkspaceListParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let project = match gate_project(&store, request.id.clone(), &params.project_id) {
        Ok(p) => p,
        Err(denial) => return denial,
    };
    let Some(root) = project.workspace_path.as_ref() else {
        return JsonRpcResponse::success(
            request.id,
            json!({ "root_bound": false, "entries": Vec::<WorkspaceEntry>::new() }),
        );
    };

    let rel = params.rel_path.unwrap_or_default();
    let dir = match resolve_in_workspace(root, &rel) {
        Ok(p) => p,
        Err(refusal) => return workspace_refusal(request.id, &refusal, &rel),
    };

    let Ok(reader) = std::fs::read_dir(&dir) else {
        return workspace_refusal(request.id, &PathRefusal::Missing, &rel);
    };

    let denied = crate::builtin_tools::file_ops::get_denied_paths();
    let mut entries: Vec<WorkspaceEntry> = Vec::new();
    for item in reader.flatten() {
        // Per-entry, and on the canonical form: a denied file must not appear
        // in a listing it would be refused from reading. An entry that cannot
        // be canonicalized (a broken symlink, a race with a delete) is
        // skipped rather than reported — this surface describes what is
        // readable, and a name it cannot resolve is not.
        let Ok(canonical) = item.path().canonicalize() else {
            continue;
        };
        if crate::builtin_tools::file_ops::path_is_denied(&canonical, &denied) {
            continue;
        }
        let Ok(meta) = item.metadata() else {
            continue;
        };
        entries.push(WorkspaceEntry {
            name: item.file_name().to_string_lossy().into_owned(),
            is_dir: meta.is_dir(),
            size: if meta.is_dir() { 0 } else { meta.len() },
        });
    }
    // Directories first, then by name — a stable order, so a listing does not
    // reshuffle between two reads of an unchanged directory.
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));

    JsonRpcResponse::success(
        request.id,
        json!({ "root_bound": true, "entries": entries }),
    )
}

/// Handle `projects.workspace.read`.
///
/// Text only, capped at [`WORKSPACE_READ_MAX_BYTES`]. A binary file is
/// refused rather than lossily decoded: this feeds a text preview, and
/// replacement characters would misrepresent the file contents as damaged.
pub async fn handle_workspace_read(
    request: JsonRpcRequest,
    store: Arc<ProjectStore>,
) -> JsonRpcResponse {
    let params: WorkspaceReadParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let project = match gate_project(&store, request.id.clone(), &params.project_id) {
        Ok(p) => p,
        Err(denial) => return denial,
    };
    // An unbound room has no root under which anything could resolve, so
    // every rel_path is genuinely absent. Same shape as a denied one.
    let Some(root) = project.workspace_path.as_ref() else {
        return workspace_refusal(request.id, &PathRefusal::Missing, &params.rel_path);
    };

    let file = match resolve_in_workspace(root, &params.rel_path) {
        Ok(p) => p,
        Err(refusal) => return workspace_refusal(request.id, &refusal, &params.rel_path),
    };
    if file.is_dir() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "That path is a directory; use projects.workspace.list".to_string(),
        );
    }

    let Ok(bytes) = std::fs::read(&file) else {
        return workspace_refusal(request.id, &PathRefusal::Missing, &params.rel_path);
    };
    let sniff = bytes.len().min(WORKSPACE_BINARY_SNIFF_BYTES);
    if bytes[..sniff].contains(&0) {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "Binary file; this surface previews text only".to_string(),
        );
    }

    let truncated = bytes.len() > WORKSPACE_READ_MAX_BYTES;
    // Truncate on a char boundary. Cutting mid-sequence would emit a
    // replacement character that reads as corruption IN the file rather than
    // as a truncation OF it — the caller cannot tell those apart.
    let content = if truncated {
        let mut cut = WORKSPACE_READ_MAX_BYTES;
        // A continuation byte (0b10xxxxxx) means the cut landed inside a
        // multi-byte sequence; walk back to the start of it.
        while cut > 0 && (bytes[cut] & 0xC0) == 0x80 {
            cut -= 1;
        }
        String::from_utf8_lossy(&bytes[..cut]).into_owned()
    } else {
        String::from_utf8_lossy(&bytes).into_owned()
    };

    JsonRpcResponse::success(
        request.id,
        json!({ "content": content, "truncated": truncated }),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::caller_identity::CALLER_USER;
    use crate::gateway::events::GatewayEventFrame;
    use crate::projects::roster::TEST_GUARD as ROSTER_TEST_GUARD;
    use rusqlite::Connection;
    use std::sync::MutexGuard;

    /// `alice` owns the room, `bob` is a plain member, `carol` is an org
    /// admin, `mallory` is a stranger. Returned guard serialises the roster
    /// projection — see [`crate::projects::roster::TEST_GUARD`].
    fn room() -> (
        Arc<ProjectStore>,
        Arc<SecurityStore>,
        Project,
        MutexGuard<'static, ()>,
    ) {
        let guard = ROSTER_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let store = Arc::new(ProjectStore::new(Connection::open_in_memory().unwrap()));
        store.create_schema().unwrap();

        let users = Arc::new(SecurityStore::in_memory().unwrap());
        for (id, role) in [
            ("u-alice", UserRole::Member),
            ("u-bob", UserRole::Member),
            ("u-carol", UserRole::Admin),
            ("u-mallory", UserRole::Member),
        ] {
            users.create_user(id, id, role).unwrap();
        }

        let project = store.create("shared room", Some("u-alice"), None).unwrap();
        store.add_member(&project.id, "u-bob").unwrap();
        (store, users, project, guard)
    }

    fn rpc(method: &str, params: Value) -> JsonRpcRequest {
        JsonRpcRequest::with_id(method, Some(params), json!(1))
    }

    fn err_of(resp: &JsonRpcResponse) -> (i32, String) {
        let e = resp.error.as_ref().expect("expected an error response");
        (e.code, e.message.clone())
    }

    fn test_event_bus() -> Arc<GatewayEventBus> {
        Arc::new(GatewayEventBus::new())
    }

    /// The no-oracle contract: a room that belongs to somebody else's roster
    /// must be indistinguishable from an id that was never minted.
    #[tokio::test]
    async fn a_foreign_project_reads_exactly_like_a_missing_one() {
        let (store, _users, project, _guard) = room();

        let foreign = CALLER_USER
            .scope(
                Some("u-mallory".to_string()),
                handle_get(
                    rpc("projects.get", json!({ "id": project.id })),
                    store.clone(),
                ),
            )
            .await;
        let missing = CALLER_USER
            .scope(
                Some("u-mallory".to_string()),
                handle_get(
                    rpc("projects.get", json!({ "id": "p-does-not-exist" })),
                    store.clone(),
                ),
            )
            .await;

        // Byte-identical modulo the id each one echoes, which the caller
        // supplied and already knows.
        let (fc, fm) = err_of(&foreign);
        let (mc, mm) = err_of(&missing);
        assert_eq!(fc, mc, "same code");
        assert_eq!(
            fm.replace(&project.id, "X"),
            mm.replace("p-does-not-exist", "X")
        );
        assert!(foreign.result.is_none() && missing.result.is_none());
    }

    #[tokio::test]
    async fn list_shows_only_the_projects_i_am_on() {
        let (store, _users, project, _guard) = room();

        let ids = |resp: JsonRpcResponse| -> Vec<String> {
            resp.result.expect("list never errors on visibility")["projects"]
                .as_array()
                .expect("projects array")
                .iter()
                .map(|p| p["id"].as_str().unwrap_or_default().to_string())
                .collect()
        };

        for member in ["u-alice", "u-bob"] {
            let seen = ids(CALLER_USER
                .scope(
                    Some(member.to_string()),
                    handle_list(rpc("projects.list", json!({})), store.clone()),
                )
                .await);
            assert_eq!(seen, vec![project.id.clone()], "{member} is on the roster");
        }

        let stranger = ids(CALLER_USER
            .scope(
                Some("u-mallory".to_string()),
                handle_list(rpc("projects.list", json!({})), store.clone()),
            )
            .await);
        assert!(
            stranger.is_empty(),
            "a stranger sees no rooms: {stranger:?}"
        );

        // An unrestricted (internal) caller keeps the pre-P2 whole view.
        let all = ids(handle_list(rpc("projects.list", json!({})), store).await);
        assert_eq!(all, vec![project.id]);
    }

    #[tokio::test]
    async fn a_member_cannot_add_another_member() {
        let (store, users, project, _guard) = room();

        let denied = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_member_add(
                    rpc(
                        "projects.member.add",
                        json!({ "id": project.id, "user_id": "u-mallory" }),
                    ),
                    store.clone(),
                    users,
                    test_event_bus(),
                ),
            )
            .await;

        let (code, msg) = err_of(&denied);
        assert_eq!(code, PERMISSION_DENIED);
        assert_eq!(msg, "not the project owner");
        assert_eq!(
            store.members(&project.id).unwrap(),
            vec!["u-alice".to_string(), "u-bob".to_string()],
            "the roster must be untouched"
        );
    }

    #[tokio::test]
    async fn the_owner_and_an_org_admin_may_both_add_a_member() {
        let (store, users, project, _guard) = room();

        let ok = CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_member_add(
                    rpc(
                        "projects.member.add",
                        json!({ "id": project.id, "user_id": "u-mallory" }),
                    ),
                    store.clone(),
                    users.clone(),
                    test_event_bus(),
                ),
            )
            .await;
        assert!(ok.error.is_none(), "the owner may add: {:?}", ok.error);

        // carol is an org admin who is NOT on the roster — she still cannot
        // SEE the room, so the visibility gate refuses her before the owner
        // gate is ever consulted. Admin is not a bypass for the roster.
        let admin_outside = CALLER_USER
            .scope(
                Some("u-carol".to_string()),
                handle_member_add(
                    rpc(
                        "projects.member.add",
                        json!({ "id": project.id, "user_id": "u-bob" }),
                    ),
                    store.clone(),
                    users.clone(),
                    test_event_bus(),
                ),
            )
            .await;
        assert_eq!(err_of(&admin_outside).0, RESOURCE_NOT_FOUND);

        // Once on the roster, the same admin passes the owner gate.
        store.add_member(&project.id, "u-carol").unwrap();
        let admin_inside = CALLER_USER
            .scope(
                Some("u-carol".to_string()),
                handle_rename(
                    rpc(
                        "projects.rename",
                        json!({ "id": project.id, "name": "renamed by admin" }),
                    ),
                    store.clone(),
                    users,
                    test_event_bus(),
                ),
            )
            .await;
        assert!(
            admin_inside.error.is_none(),
            "an org admin on the roster may rename: {:?}",
            admin_inside.error
        );
    }

    #[tokio::test]
    async fn adding_an_unknown_user_is_rejected_and_changes_nothing() {
        let (store, users, project, _guard) = room();
        let before = store.members(&project.id).unwrap();

        for candidate in ["u-nobody", ""] {
            let denied = CALLER_USER
                .scope(
                    Some("u-alice".to_string()),
                    handle_member_add(
                        rpc(
                            "projects.member.add",
                            json!({ "id": project.id, "user_id": candidate }),
                        ),
                        store.clone(),
                        users.clone(),
                        test_event_bus(),
                    ),
                )
                .await;
            let (code, msg) = err_of(&denied);
            assert_eq!(code, INVALID_PARAMS, "{candidate}");
            assert_eq!(msg, format!("unknown user: {candidate}"));
        }
        assert_eq!(store.members(&project.id).unwrap(), before);
    }

    /// Removing a member revokes their sight of the room in the same call —
    /// the roster projection IS the predicate, so there is no second store to
    /// fall out of sync with.
    #[tokio::test]
    async fn removing_a_member_takes_the_room_out_of_their_list() {
        let (store, users, project, _guard) = room();

        let removed = CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_member_remove(
                    rpc(
                        "projects.member.remove",
                        json!({ "id": project.id, "user_id": "u-bob" }),
                    ),
                    store.clone(),
                    users,
                    test_event_bus(),
                ),
            )
            .await;
        assert!(removed.error.is_none(), "{:?}", removed.error);

        let bob_sees = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_get(
                    rpc("projects.get", json!({ "id": project.id })),
                    store.clone(),
                ),
            )
            .await;
        assert_eq!(err_of(&bob_sees).0, RESOURCE_NOT_FOUND);
    }

    /// Task 6 (`projects.changed` push topic): removing a member must publish
    /// a frame naming THAT user as `affected_user` — the roster projection no
    /// longer admits them by the time this fires, so without the carve-out
    /// their own client would never learn to drop the room from its list.
    #[tokio::test]
    async fn handle_member_remove_emits_projects_changed_naming_the_removed_user() {
        let (store, users, project, _guard) = room();
        let bus = test_event_bus();
        let mut rx = bus.subscribe_typed();

        let removed = CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_member_remove(
                    rpc(
                        "projects.member.remove",
                        json!({ "id": project.id, "user_id": "u-bob" }),
                    ),
                    store,
                    users,
                    Arc::clone(&bus),
                ),
            )
            .await;
        assert!(removed.error.is_none(), "{:?}", removed.error);

        match rx.try_recv() {
            Ok(GatewayEventFrame::ProjectsChanged {
                project_id,
                change,
                affected_user,
            }) => {
                assert_eq!(project_id, project.id);
                assert_eq!(change, ChangeKind::Updated);
                assert_eq!(affected_user.as_deref(), Some("u-bob"));
            }
            other => panic!("expected a ProjectsChanged frame, got {other:?}"),
        }
    }

    /// T04: removing somebody who was never on the roster must not report a
    /// revocation that never happened — no AuthorityChange row (queried via
    /// the audit log, not inferred from "the call returned Ok"), and the
    /// push frame names nobody, because nobody was actually dropped.
    #[tokio::test]
    async fn removing_a_non_member_writes_no_audit_row_and_names_nobody() {
        let _serial = crate::security::audit::AUDIT_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (log, mut rx_audit) =
            crate::security::audit::SecurityAuditLog::new(crate::security::audit::TEST_LOG_CAPACITY);
        crate::security::audit::replace_global_for_test(&log);

        let (store, users, project, _guard) = room();
        let bus = test_event_bus();
        let mut rx = bus.subscribe_typed();

        let removed = CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_member_remove(
                    rpc(
                        "projects.member.remove",
                        json!({ "id": project.id, "user_id": "u-mallory" }),
                    ),
                    store,
                    users,
                    Arc::clone(&bus),
                ),
            )
            .await;
        assert!(
            removed.error.is_none(),
            "removing a non-member is a no-op, not an error: {:?}",
            removed.error
        );

        // The audit log is process-global and `cargo test` runs threads in
        // parallel, so a concurrently-running test can also write into the
        // channel this test just installed — filter to this test's own
        // project id rather than assuming the channel is empty outright.
        let mut leaked = Vec::new();
        while let Ok(entry) = rx_audit.try_recv() {
            if entry.detail.contains(&project.id) {
                leaked.push(entry.detail);
            }
        }
        // `replace_global_for_test`'s contract: clear before releasing the
        // lock, so a later non-audit test never writes into a handle whose
        // receiver is gone. Before the assertion, so a failing one still
        // leaves the process clean.
        crate::security::audit::clear_global_for_test();
        assert!(
            leaked.is_empty(),
            "no member was actually dropped — this must not write an AuthorityChange row, got: {leaked:?}"
        );

        match rx.try_recv() {
            Ok(GatewayEventFrame::ProjectsChanged { affected_user, .. }) => {
                assert!(
                    affected_user.is_none(),
                    "nobody was actually removed — the push must not name a bystander"
                );
            }
            other => panic!("expected a ProjectsChanged frame, got {other:?}"),
        }
    }

    /// Every OTHER mutation leaves `affected_user` `None` — pinned on
    /// `rename` as the representative case. `member_remove` above is the one
    /// exception.
    #[tokio::test]
    async fn handle_rename_emits_projects_changed_with_no_affected_user() {
        let (store, users, project, _guard) = room();
        let bus = test_event_bus();
        let mut rx = bus.subscribe_typed();

        let renamed = CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_rename(
                    rpc(
                        "projects.rename",
                        json!({ "id": project.id, "name": "renamed" }),
                    ),
                    store,
                    users,
                    Arc::clone(&bus),
                ),
            )
            .await;
        assert!(renamed.error.is_none(), "{:?}", renamed.error);

        match rx.try_recv() {
            Ok(GatewayEventFrame::ProjectsChanged {
                project_id,
                change,
                affected_user,
            }) => {
                assert_eq!(project_id, project.id);
                assert_eq!(change, ChangeKind::Updated);
                assert_eq!(affected_user, None);
            }
            other => panic!("expected a ProjectsChanged frame, got {other:?}"),
        }
    }

    /// The roster is the visibility predicate, so an owner removed from it
    /// would lose the room they alone can archive or delete.
    #[tokio::test]
    async fn the_owner_cannot_be_removed_from_their_own_roster() {
        let (store, users, project, _guard) = room();

        let denied = CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_member_remove(
                    rpc(
                        "projects.member.remove",
                        json!({ "id": project.id, "user_id": "u-alice" }),
                    ),
                    store.clone(),
                    users,
                    test_event_bus(),
                ),
            )
            .await;
        assert_eq!(err_of(&denied).0, INVALID_PARAMS);
        assert!(store
            .members(&project.id)
            .unwrap()
            .contains(&"u-alice".to_string()));
    }

    /// `touch` is member-level; `rename`/`archive`/`remove` are not. Pin the
    /// asymmetry so a future refactor cannot quietly level them.
    #[tokio::test]
    async fn a_member_may_touch_but_not_archive() {
        let (store, users, project, _guard) = room();

        let touched = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_touch(
                    rpc("projects.touch", json!({ "id": project.id })),
                    store.clone(),
                ),
            )
            .await;
        assert!(touched.error.is_none(), "{:?}", touched.error);

        let archived = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_archive(
                    rpc("projects.archive", json!({ "id": project.id })),
                    store.clone(),
                    users,
                    test_event_bus(),
                ),
            )
            .await;
        assert_eq!(err_of(&archived).0, PERMISSION_DENIED);
    }

    /// Archiving hides the room from the default list without dropping the
    /// roster — otherwise `archive` and `remove` would be the same verb.
    #[tokio::test]
    async fn an_archived_room_leaves_the_default_list_but_keeps_its_roster() {
        let (store, users, project, _guard) = room();

        let archived = CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_archive(
                    rpc("projects.archive", json!({ "id": project.id })),
                    store.clone(),
                    users,
                    test_event_bus(),
                ),
            )
            .await;
        assert!(archived.error.is_none(), "{:?}", archived.error);

        let listed = |params: Value, store: Arc<ProjectStore>| async move {
            CALLER_USER
                .scope(
                    Some("u-alice".to_string()),
                    handle_list(rpc("projects.list", params), store),
                )
                .await
                .result
                .expect("list succeeds")["projects"]
                .as_array()
                .expect("array")
                .len()
        };
        assert_eq!(listed(json!({}), store.clone()).await, 0);
        assert_eq!(
            listed(json!({ "include_archived": true }), store.clone()).await,
            1
        );
        assert_eq!(store.members(&project.id).unwrap().len(), 2);
    }

    /// Every gate opens with the unrestricted-caller arm, which is the
    /// single-user zero-change guarantee: no `CALLER_USER` scope at all must
    /// behave exactly like pre-P2.
    #[tokio::test]
    async fn an_unscoped_caller_is_unrestricted() {
        let (store, users, project, _guard) = room();

        let got = handle_get(
            rpc("projects.get", json!({ "id": project.id })),
            store.clone(),
        )
        .await;
        assert!(got.error.is_none(), "{:?}", got.error);

        let renamed = handle_rename(
            rpc(
                "projects.rename",
                json!({ "id": project.id, "name": "cron renamed me" }),
            ),
            store.clone(),
            users,
            test_event_bus(),
        )
        .await;
        assert!(renamed.error.is_none(), "{:?}", renamed.error);
    }

    /// An unbound room renders `workspace_path: null`, not `""` — the pre-P2
    /// view spelled "no folder" as an empty string, which a Panel cannot
    /// distinguish from a folder whose path failed to render.
    #[tokio::test]
    async fn an_unbound_room_renders_a_null_workspace_path() {
        let (store, _users, project, _guard) = room();
        let got = handle_get(rpc("projects.get", json!({ "id": project.id })), store).await;
        let view = &got.result.expect("success")["project"];
        assert!(view["workspace_path"].is_null());
        assert_eq!(view["status"], "active");
        assert_eq!(view["owner_user_id"], "u-alice");
    }

    // ------------------------------------------------------------------
    // projects.bind_workspace (Task 7)
    // ------------------------------------------------------------------

    /// Drive a handler with a fully-populated connection identity. `role:
    /// None` is the unrestricted internal caller every gate opens with.
    async fn as_caller<F, T>(user: &str, role: Option<&str>, loopback: bool, fut: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        use crate::gateway::caller_identity::{CALLER_IS_LOOPBACK, CALLER_ROLE};
        CALLER_USER
            .scope(
                Some(user.to_string()),
                CALLER_ROLE.scope(
                    role.map(str::to_string),
                    CALLER_IS_LOOPBACK.scope(loopback, fut),
                ),
            )
            .await
    }

    /// An existing directory, canonicalised the same way the store will
    /// canonicalise it, so the round-trip comparison is exact on Windows too
    /// (where `canonicalize` adds the `\\?\` verbatim prefix).
    fn a_real_dir() -> PathBuf {
        std::fs::canonicalize(std::env::temp_dir()).expect("temp dir is canonicalisable")
    }

    #[tokio::test]
    async fn the_owner_binds_a_folder_and_the_room_reports_it() {
        let (store, users, project, _guard) = room();
        let dir = a_real_dir();

        let bound = as_caller(
            "u-alice",
            Some("operator"),
            false,
            handle_bind_workspace(
                rpc(
                    "projects.bind_workspace",
                    json!({ "id": project.id, "path": dir.display().to_string() }),
                ),
                store.clone(),
                users,
                test_event_bus(),
            ),
        )
        .await;

        let view = &bound.result.expect("bind succeeds")["project"];
        // The response is the DISPLAY form (no `\\?\` on Windows); the row keeps
        // the canonical bytes. Asserting both in one test is the point — they
        // are allowed to differ, and only here is that difference legible.
        assert_eq!(
            view["workspace_path"],
            crate::utils::paths::display_string(&dir)
        );
        assert_eq!(
            store.get(&project.id).unwrap().unwrap().workspace_path,
            Some(dir),
            "the row, not just the response"
        );
    }

    /// Rebinding redirects where the WHOLE room works — an owner decision.
    #[tokio::test]
    async fn a_plain_member_cannot_rebind_the_room() {
        let (store, users, project, _guard) = room();

        let denied = as_caller(
            "u-bob",
            Some("operator"),
            true,
            handle_bind_workspace(
                rpc(
                    "projects.bind_workspace",
                    json!({ "id": project.id, "path": a_real_dir().display().to_string() }),
                ),
                store.clone(),
                users,
                test_event_bus(),
            ),
        )
        .await;

        assert_eq!(err_of(&denied).0, PERMISSION_DENIED);
        assert!(store
            .get(&project.id)
            .unwrap()
            .unwrap()
            .workspace_path
            .is_none());
    }

    /// The gate this whole task turns on: without it, "register a folder,
    /// then chat in it" is a two-step route to an arbitrary server directory
    /// for a caller who may not name one directly.
    #[tokio::test]
    async fn a_remote_chat_tier_owner_cannot_name_a_folder() {
        let (store, users, project, _guard) = room();

        let denied = as_caller(
            "u-alice",
            Some("member"),
            false,
            handle_bind_workspace(
                rpc(
                    "projects.bind_workspace",
                    json!({ "id": project.id, "path": a_real_dir().display().to_string() }),
                ),
                store.clone(),
                users,
                test_event_bus(),
            ),
        )
        .await;

        assert_eq!(err_of(&denied).0, PERMISSION_DENIED);
        assert!(store
            .get(&project.id)
            .unwrap()
            .unwrap()
            .workspace_path
            .is_none());
    }

    /// Unbinding is a de-escalation and stays reachable from the surface that
    /// got stuck — otherwise a room whose folder vanished would be
    /// unrepairable from the only connection its owner has.
    #[tokio::test]
    async fn the_same_owner_may_still_unbind_from_a_chat_tier_connection() {
        let (store, users, project, _guard) = room();
        store
            .bind_workspace(&project.id, Some(&a_real_dir()))
            .unwrap();

        let unbound = as_caller(
            "u-alice",
            Some("member"),
            false,
            handle_bind_workspace(
                rpc(
                    "projects.bind_workspace",
                    json!({ "id": project.id, "path": Value::Null }),
                ),
                store.clone(),
                users,
                test_event_bus(),
            ),
        )
        .await;

        assert!(unbound.error.is_none(), "{:?}", unbound.error);
        assert!(store
            .get(&project.id)
            .unwrap()
            .unwrap()
            .workspace_path
            .is_none());
    }

    /// The no-oracle contract holds for the new verb too: a stranger learns
    /// nothing about whether the room exists.
    #[tokio::test]
    async fn a_stranger_binding_gets_not_found_not_permission_denied() {
        let (store, users, project, _guard) = room();

        let denied = as_caller(
            "u-mallory",
            Some("operator"),
            true,
            handle_bind_workspace(
                rpc(
                    "projects.bind_workspace",
                    json!({ "id": project.id, "path": a_real_dir().display().to_string() }),
                ),
                store,
                users,
                test_event_bus(),
            ),
        )
        .await;

        let (code, msg) = err_of(&denied);
        assert_eq!(code, RESOURCE_NOT_FOUND);
        assert!(msg.starts_with("project not found:"), "{msg}");
    }

    /// The other two writers of `workspace_path`. Registering a folder was
    /// open to any member before P2 because the column was inert; Task 7 made
    /// it a working directory.
    #[tokio::test]
    async fn the_other_two_workspace_writers_carry_the_same_gate() {
        let (store, _users, _project, _guard) = room();
        let dir = a_real_dir();

        let added = as_caller(
            "u-bob",
            Some("member"),
            false,
            handle_add(
                rpc("projects.add", json!({ "path": dir.display().to_string() })),
                store.clone(),
                test_event_bus(),
            ),
        )
        .await;
        assert_eq!(err_of(&added).0, PERMISSION_DENIED, "projects.add");

        // A parent nobody else has touched, so "the folder is absent" can only
        // mean the gate ran before the `mkdir`.
        let parent = tempfile::tempdir().expect("tempdir");
        let blanked = as_caller(
            "u-bob",
            Some("member"),
            false,
            handle_create_blank(
                rpc(
                    "projects.create_blank",
                    json!({ "parent": parent.path().display().to_string(), "name": "escalation" }),
                ),
                store.clone(),
                test_event_bus(),
            ),
        )
        .await;
        assert_eq!(
            err_of(&blanked).0,
            PERMISSION_DENIED,
            "projects.create_blank"
        );
        assert!(
            !parent.path().join("escalation").exists(),
            "the gate must run BEFORE mkdir, not after"
        );
    }

    /// The desktop App's own Panel is chat-tier by pairing and local by
    /// address. Zero-config single-machine use must not have regressed.
    #[tokio::test]
    async fn the_local_panel_still_registers_folders() {
        let (store, _users, _project, _guard) = room();

        let added = as_caller(
            "u-alice",
            Some("member"),
            true,
            handle_add(
                rpc(
                    "projects.add",
                    json!({ "path": a_real_dir().display().to_string() }),
                ),
                store,
                test_event_bus(),
            ),
        )
        .await;
        assert!(added.error.is_none(), "{:?}", added.error);
    }

    // ── projects.room_session (C1) ──────────────────────────────────────

    fn session_key_of(resp: &JsonRpcResponse) -> String {
        resp.result
            .as_ref()
            .and_then(|r| r.get("session_key"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("expected a session_key, got {:?}", resp.error))
            .to_string()
    }

    /// The whole point of C1: two members of one room resolve to ONE session,
    /// even though each Panel proposes its own default agent. Before this,
    /// each browser minted its own key from `localStorage` and the members
    /// never saw each other's turns.
    #[tokio::test]
    async fn two_members_of_a_room_resolve_to_the_same_session() {
        let (store, _users, project, _guard) = room();

        let alice = as_caller(
            "u-alice",
            Some("member"),
            true,
            handle_room_session(
                rpc(
                    "projects.room_session",
                    json!({ "id": project.id, "agent_id": "main" }),
                ),
                store.clone(),
            ),
        )
        .await;
        let bob = as_caller(
            "u-bob",
            Some("member"),
            false,
            handle_room_session(
                rpc(
                    "projects.room_session",
                    json!({ "id": project.id, "agent_id": "coder" }),
                ),
                store.clone(),
            ),
        )
        .await;

        assert_eq!(session_key_of(&alice), session_key_of(&bob));
        assert_eq!(
            session_key_of(&alice),
            crate::gateway::router::SessionKey::project_room("main", &project.id).to_key_string(),
            "the first caller's candidate is the one that sticks"
        );
        // Re-entering the room reuses it rather than forking a second one.
        let again = as_caller(
            "u-alice",
            Some("member"),
            true,
            handle_room_session(
                rpc("projects.room_session", json!({ "id": project.id })),
                store,
            ),
        )
        .await;
        assert_eq!(session_key_of(&again), session_key_of(&alice));
    }

    /// A stranger must not learn that the room exists, and must not be able to
    /// mint its session either. Same refusal shape as every other addressed
    /// `projects.*` verb, byte for byte.
    #[tokio::test]
    async fn a_stranger_asking_for_a_rooms_session_gets_not_found() {
        let (store, _users, project, _guard) = room();

        let denied = as_caller(
            "u-mallory",
            Some("member"),
            false,
            handle_room_session(
                rpc("projects.room_session", json!({ "id": project.id })),
                store.clone(),
            ),
        )
        .await;
        let missing = as_caller(
            "u-mallory",
            Some("member"),
            false,
            handle_room_session(
                rpc("projects.room_session", json!({ "id": "p-never-minted" })),
                store.clone(),
            ),
        )
        .await;
        assert_eq!(err_of(&denied).0, RESOURCE_NOT_FOUND);
        assert_eq!(
            err_of(&denied).1.replace(&project.id, "p-never-minted"),
            err_of(&missing).1,
            "a refused room and a missing room must be indistinguishable"
        );
        assert!(
            store
                .get(&project.id)
                .unwrap()
                .unwrap()
                .current_session_key
                .is_none(),
            "a refused caller must not have claimed the room's session"
        );
    }

    // ========================================================================
    // projects.workspace.list / .read
    // ========================================================================

    /// Bind `project` to a fresh temp tree containing a couple of files and a
    /// subdirectory. Returns the guard — dropping it deletes the tree, so it
    /// must outlive every assertion (see the scratch-guard criterion: a guard
    /// bound to a returning frame deletes the tree before the caller uses it).
    fn bound_room(store: &ProjectStore, project: &Project) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "hello room").unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src").join("main.rs"), "fn main() {}").unwrap();
        store
            .bind_workspace(&project.id, Some(dir.path()))
            .expect("bind");
        dir
    }

    fn ws_entries(resp: &JsonRpcResponse) -> Vec<String> {
        let body = resp.result.as_ref().expect("success");
        body["entries"]
            .as_array()
            .expect("entries array")
            .iter()
            .map(|e| e["name"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    #[tokio::test]
    async fn a_member_lists_the_bound_root_and_a_stranger_gets_the_not_found_shape() {
        let (store, _users, project, _g) = room();
        let _tree = bound_room(&store, &project);

        let member = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_workspace_list(
                    rpc(
                        "projects.workspace.list",
                        json!({ "project_id": project.id }),
                    ),
                    store.clone(),
                )
                .await
            })
            .await;
        let names = ws_entries(&member);
        assert!(names.contains(&"README.md".to_string()), "got {names:?}");
        assert!(names.contains(&"src".to_string()), "got {names:?}");
        assert_eq!(
            member.result.as_ref().unwrap()["root_bound"],
            json!(true),
            "a bound room reports itself bound"
        );

        // A stranger must not be able to tell a room they cannot see from one
        // that does not exist.
        let stranger = CALLER_USER
            .scope(Some("u-mallory".to_string()), async {
                handle_workspace_list(
                    rpc(
                        "projects.workspace.list",
                        json!({ "project_id": project.id }),
                    ),
                    store.clone(),
                )
                .await
            })
            .await;
        let absent = CALLER_USER
            .scope(Some("u-mallory".to_string()), async {
                handle_workspace_list(
                    rpc("projects.workspace.list", json!({ "project_id": "p-nope" })),
                    store.clone(),
                )
                .await
            })
            .await;
        assert_eq!(err_of(&stranger).0, err_of(&absent).0);
        assert_eq!(
            err_of(&stranger).1,
            err_of(&absent).1.replace("p-nope", &project.id),
            "refusal must not be distinguishable from absence"
        );
    }

    #[tokio::test]
    async fn an_unbound_room_reports_unbound_rather_than_failing() {
        let (store, _users, project, _g) = room();
        let resp = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_workspace_list(
                    rpc(
                        "projects.workspace.list",
                        json!({ "project_id": project.id }),
                    ),
                    store.clone(),
                )
                .await
            })
            .await;
        let body = resp.result.as_ref().expect("not an error");
        assert_eq!(body["root_bound"], json!(false));
        assert_eq!(body["entries"], json!([]));
    }

    #[tokio::test]
    async fn a_relative_escape_and_an_absolute_path_are_both_refused_as_outside() {
        let (store, _users, project, _g) = room();
        let _tree = bound_room(&store, &project);

        for probe in ["../outside", "src/../../outside"] {
            let resp = CALLER_USER
                .scope(Some("u-bob".to_string()), async {
                    handle_workspace_list(
                        rpc(
                            "projects.workspace.list",
                            json!({ "project_id": project.id, "rel_path": probe }),
                        ),
                        store.clone(),
                    )
                    .await
                })
                .await;
            assert!(resp.result.is_none(), "{probe} must not succeed");
        }

        // An absolute path would REPLACE the base in `Path::join`, so this is
        // the case a naive join gets wrong.
        // Built from MAIN_SEPARATOR rather than written with a literal
        // backslash: the point is "an absolute path for this platform", and
        // spelling it twice invites one of the two to rot.
        let abs = if cfg!(windows) {
            format!("C:{}Windows", std::path::MAIN_SEPARATOR)
        } else {
            "/etc".to_string()
        };
        let resp = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_workspace_list(
                    rpc(
                        "projects.workspace.list",
                        json!({ "project_id": project.id, "rel_path": abs }),
                    ),
                    store.clone(),
                )
                .await
            })
            .await;
        assert_eq!(err_of(&resp).0, PERMISSION_DENIED);
    }

    #[tokio::test]
    async fn read_returns_the_file_and_a_directory_is_refused_as_a_param_error() {
        let (store, _users, project, _g) = room();
        let _tree = bound_room(&store, &project);

        let ok = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_workspace_read(
                    rpc(
                        "projects.workspace.read",
                        json!({ "project_id": project.id, "rel_path": "README.md" }),
                    ),
                    store.clone(),
                )
                .await
            })
            .await;
        let body = ok.result.as_ref().expect("success");
        assert_eq!(body["content"], json!("hello room"));
        assert_eq!(body["truncated"], json!(false));

        let dir = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_workspace_read(
                    rpc(
                        "projects.workspace.read",
                        json!({ "project_id": project.id, "rel_path": "src" }),
                    ),
                    store.clone(),
                )
                .await
            })
            .await;
        assert_eq!(err_of(&dir).0, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn an_oversized_file_truncates_on_a_char_boundary_and_says_so() {
        let (store, _users, project, _g) = room();
        let tree = bound_room(&store, &project);
        // Multi-byte characters, sized so the 64 KiB cut lands INSIDE one.
        // A byte-wise cut would emit a replacement char, which the caller
        // cannot tell from corruption in the file itself.
        let big = "四".repeat(WORKSPACE_READ_MAX_BYTES);
        std::fs::write(tree.path().join("big.txt"), &big).unwrap();

        let resp = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_workspace_read(
                    rpc(
                        "projects.workspace.read",
                        json!({ "project_id": project.id, "rel_path": "big.txt" }),
                    ),
                    store.clone(),
                )
                .await
            })
            .await;
        let body = resp.result.as_ref().expect("success");
        assert_eq!(body["truncated"], json!(true));
        let content = body["content"].as_str().unwrap();
        assert!(content.len() <= WORKSPACE_READ_MAX_BYTES);
        assert!(
            !content.contains('\u{FFFD}'),
            "truncation must not manufacture a replacement character"
        );
        assert!(content.starts_with('四'));
    }

    #[tokio::test]
    async fn a_binary_file_is_refused_rather_than_lossily_decoded() {
        let (store, _users, project, _g) = room();
        let tree = bound_room(&store, &project);
        std::fs::write(tree.path().join("blob.bin"), [0x89, 0x50, 0x00, 0x01]).unwrap();

        let resp = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_workspace_read(
                    rpc(
                        "projects.workspace.read",
                        json!({ "project_id": project.id, "rel_path": "blob.bin" }),
                    ),
                    store.clone(),
                )
                .await
            })
            .await;
        assert_eq!(err_of(&resp).0, INVALID_PARAMS);
    }

    /// The read-denial floor has to bind this surface too, or a room bound to
    /// a tree containing Aleph's own credential store would browse it in
    /// plain text — the exact half-applied-floor asymmetry `path_utils`
    /// exists to close.
    ///
    /// Points `ALEPH_HOME` inside the bound tree so `get_denied_paths`'s
    /// config-dir entries resolve to real files under it.
    ///
    /// Lock order is roster (taken by `room()`) THEN `ALEPH_HOME`, matching
    /// every other site in the tree — two orders would be an ABBA deadlock,
    /// the failure mode `HomeEnvGuards`'s doc records hanging a whole `--lib`
    /// run. The roster lock is not optional here even though this test is
    /// about paths: `ProjectStore::create` republishes the PROCESS-GLOBAL
    /// roster projection, so creating a room outside the guard silently
    /// revokes every other in-flight test's membership. An earlier draft of
    /// this test did exactly that, and the symptom landed on a sibling —
    /// an unrelated read failing `gate_project` roughly one run in seven.
    #[tokio::test]
    async fn a_denied_path_is_absent_from_the_listing_and_reads_as_not_found() {
        let (store, _users, project, _roster) = room();
        let tree = tempfile::tempdir().unwrap();
        let state = tree.path().join(".alephstate");
        std::fs::create_dir(&state).unwrap();
        std::fs::write(state.join("secrets.vault"), "ENCRYPTED").unwrap();
        std::fs::write(state.join("notes.txt"), "not a secret").unwrap();
        let _home = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(&state);
        store
            .bind_workspace(&project.id, Some(tree.path()))
            .expect("bind");

        let listed = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_workspace_list(
                    rpc(
                        "projects.workspace.list",
                        json!({ "project_id": project.id, "rel_path": ".alephstate" }),
                    ),
                    store.clone(),
                )
                .await
            })
            .await;
        let names = ws_entries(&listed);
        assert!(
            !names.contains(&"secrets.vault".to_string()),
            "the vault must not appear in a listing it cannot be read from: {names:?}"
        );
        assert!(
            names.contains(&"notes.txt".to_string()),
            "only the denied entry is hidden, not the whole directory: {names:?}"
        );

        let read = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_workspace_read(
                    rpc(
                        "projects.workspace.read",
                        json!({ "project_id": project.id, "rel_path": ".alephstate/secrets.vault" }),
                    ),
                    store.clone(),
                )
                .await
            })
            .await;
        assert_eq!(
            err_of(&read).0,
            RESOURCE_NOT_FOUND,
            "a denied file must be indistinguishable from an absent one"
        );
    }
}
