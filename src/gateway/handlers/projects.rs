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
//! has a `workspace_path`.
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
use crate::gateway::security::store::{SecurityStore, UserRole, UserStatus};
use crate::gateway::visibility;
use crate::projects::{Project, ProjectError, ProjectStatus, ProjectStore};
use crate::sync_primitives::Arc;

/// Serializable view of a project room.
///
/// `workspace_path` is `null` for a room that is not bound to a folder — the
/// ordinary shape for a room created through `projects.create`, and the reason
/// this is an `Option` rather than the pre-P2 `path: String` that spelled
/// "unbound" as `""`.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectView {
    pub id: String,
    pub name: String,
    pub owner_user_id: Option<String>,
    pub workspace_path: Option<String>,
    pub status: String,
    pub member_ids: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_used_at: i64,
}

impl ProjectView {
    fn render(p: Project, member_ids: Vec<String>) -> Self {
        Self {
            id: p.id,
            name: p.name,
            owner_user_id: p.owner_user_id,
            workspace_path: p.workspace_path.map(|w| w.to_string_lossy().to_string()),
            status: p.status.as_str().to_string(),
            member_ids,
            created_at: p.created_at,
            updated_at: p.updated_at,
            last_used_at: p.last_used_at,
        }
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
fn project_not_found(id: Option<Value>, project_id: &str) -> JsonRpcResponse {
    JsonRpcResponse::error(
        id,
        RESOURCE_NOT_FOUND,
        format!("project not found: {project_id}"),
    )
}

/// The single admission point for every addressed `projects.*` handler.
///
/// Fails closed on a store error: a caller must never be admitted because the
/// check itself could not complete.
fn gate_project(
    store: &ProjectStore,
    id: Option<Value>,
    project_id: &str,
) -> Result<Project, JsonRpcResponse> {
    match store.get(project_id) {
        Ok(Some(p)) if visibility::project_visible(&p.id) => Ok(p),
        Ok(_) => Err(project_not_found(id, project_id)),
        Err(e) => {
            tracing::warn!(project_id = %project_id, error = %e, "projects: gate failed closed");
            Err(project_not_found(id, project_id))
        }
    }
}

/// Separate a room's owner from its ordinary members. Assumes
/// [`gate_project`] already ran — this is NOT a visibility check, and calling
/// it alone would answer "are you the owner" about a project the caller
/// cannot see.
///
/// Org admins pass for any room (spec §6.3: owner changes are an admin
/// operation). An unrestricted caller — cron, A2A, an in-process test — passes
/// unconditionally, the same first arm every P1 predicate opens with.
fn require_owner(
    users: &SecurityStore,
    id: Option<Value>,
    project: &Project,
) -> Result<(), JsonRpcResponse> {
    let Some(caller) = visibility::visible_owner_filter() else {
        return Ok(());
    };
    if caller == visibility::owner_or_legacy(project.owner_user_id.as_deref()) {
        return Ok(());
    }
    let is_admin = matches!(
        users.get_user(&caller),
        Ok(Some(u)) if u.role == UserRole::Admin && u.status == UserStatus::Active
    );
    if is_admin {
        return Ok(());
    }
    Err(JsonRpcResponse::error(
        id,
        PERMISSION_DENIED,
        "not the project owner",
    ))
}

/// Reject a roster mutation naming somebody who is not an active principal.
///
/// A deactivated user reads as unknown here on purpose: adding one grants
/// access that only materialises if they are ever reactivated, which is a
/// decision `users.update` owns, not this RPC. Fails closed on a store error —
/// an unverifiable id is not a verified one.
fn require_known_user(
    users: &SecurityStore,
    id: Option<Value>,
    user_id: &str,
) -> Result<(), JsonRpcResponse> {
    let known = match users.get_user(user_id) {
        Ok(u) => u,
        Err(e) => {
            tracing::warn!(user_id = %user_id, error = %e, "projects: member check failed closed");
            None
        }
    };
    match known {
        Some(u) if u.status == UserStatus::Active => Ok(()),
        _ => Err(JsonRpcResponse::error(
            id,
            INVALID_PARAMS,
            format!("unknown user: {user_id}"),
        )),
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
            ProjectView::render(p, members)
        })
        .collect();
    JsonRpcResponse::success(request.id, json!({ "projects": view }))
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
pub async fn handle_create(request: JsonRpcRequest, store: Arc<ProjectStore>) -> JsonRpcResponse {
    let params: CreateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let owner = crate::scope::ambient_owner();
    match store.create(&params.name, owner.as_deref(), None) {
        Ok(project) => {
            let members = store.members(&project.id).unwrap_or_default();
            JsonRpcResponse::success(
                request.id,
                json!({ "project": ProjectView::render(project, members) }),
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
/// never onto somebody else's.
pub async fn handle_add(request: JsonRpcRequest, store: Arc<ProjectStore>) -> JsonRpcResponse {
    let params: AddParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let path = PathBuf::from(&params.path);
    match store.add(&path, params.name) {
        Ok(project) => {
            let members = store.members(&project.id).unwrap_or_default();
            JsonRpcResponse::success(
                request.id,
                json!({ "project": ProjectView::render(project, members) }),
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

pub async fn handle_create_blank(
    request: JsonRpcRequest,
    store: Arc<ProjectStore>,
) -> JsonRpcResponse {
    let params: CreateBlankParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let parent = PathBuf::from(&params.parent);
    match store.create_blank(&parent, &params.name) {
        Ok(project) => {
            let members = store.members(&project.id).unwrap_or_default();
            JsonRpcResponse::success(
                request.id,
                json!({ "project": ProjectView::render(project, members) }),
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
        json!({ "project": ProjectView::render(project, members) }),
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
            JsonRpcResponse::success(
                request.id,
                json!({ "project": ProjectView::render(renamed, members) }),
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
        Ok(()) => JsonRpcResponse::success(
            request.id,
            json!({ "id": params.id, "status": ProjectStatus::Archived.as_str() }),
        ),
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
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "id": params.id, "removed": true })),
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
        Ok(()) => member_list_response(request.id, &store, &params.id),
        Err(e) => project_error_response(request.id, e),
    }
}

pub async fn handle_member_remove(
    request: JsonRpcRequest,
    store: Arc<ProjectStore>,
    users: Arc<SecurityStore>,
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
    // The owner is the one member who cannot be removed: the roster IS the
    // visibility predicate, so dropping them would make the room invisible to
    // the only person who can archive or delete it. Hand the room over first
    // (an admin operation), then remove.
    if params.user_id == visibility::owner_or_legacy(project.owner_user_id.as_deref()) {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "cannot remove the project owner from its own roster",
        );
    }
    match store.remove_member(&params.id, &params.user_id) {
        Ok(()) => member_list_response(request.id, &store, &params.id),
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

fn project_error_response(id: Option<serde_json::Value>, err: ProjectError) -> JsonRpcResponse {
    let (code, msg) = match &err {
        ProjectError::NotFound(_) => (RESOURCE_NOT_FOUND, err.to_string()),
        ProjectError::NotAbsolute(_)
        | ProjectError::NotDirectory(_)
        | ProjectError::InvalidName(_) => (INVALID_PARAMS, err.to_string()),
        ProjectError::AlreadyExists(_) => (INVALID_PARAMS, err.to_string()),
        _ => (INTERNAL_ERROR, err.to_string()),
    };
    JsonRpcResponse::error(id, code, msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::caller_identity::CALLER_USER;
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
}
