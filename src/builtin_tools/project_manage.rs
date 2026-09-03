//! `project_manage` — the conversational face of project rooms (R8).
//!
//! Everything the Panel's room UI can do to a room's identity and roster, the
//! model can do by being asked in words. The RPC face is not the authority
//! here and this is not a second implementation of it: both go through
//! [`crate::projects::authz`] for who-may-do-what and through
//! [`crate::projects::events::publish_changed`] for announcing it, so a room
//! renamed by the model and a room renamed by a click are the same event to
//! every client.
//!
//! ## Why the actor is read per call, not per construction
//!
//! `visibility::ambient_actor()` is read inside `call`, on every invocation.
//! Resolving it once at construction would weld one identity into a tool that
//! outlives the turn that built it — and a wrong-but-valid identity is the
//! failure mode with no symptom: every check passes, against the wrong person.
//!
//! ## `bind_workspace` is here now, and what had to become true first
//!
//! Binding a room to a directory is the fifth writer of `workspace_path`, and
//! since P2 that column is the default cwd of every member's run — so a writer
//! is a permission grant, not a preference. This verb was kept off the tool
//! face until 2026-08-29 for a reason that was true when written and is not
//! any more: the only in-run answer to "may this actor name a server
//! directory" was `caller_may_choose_directory()`, which reads the ambient
//! `CALLER_ROLE` task-local — and that task-local is dead past the
//! `tokio::spawn` every run crosses. It therefore did not gate a tool call,
//! it mis-answered one, in whichever direction its `None` arm happened to
//! point.
//!
//! What this face uses instead is not a second predicate. It is the SAME
//! question asked of the object that is alive inside a run:
//! [`TurnContext::caller_is_operator`](crate::tools::turn_context::TurnContext::caller_is_operator)
//! — the exact predicate `src/tools/scoped/dispatch.rs`'s `check_operator_gate`
//! reads to decide whether a config-tier tool may proceed. The gateway builds
//! a `TurnContext` per turn with the connection's role stamped into it, so
//! `"guest"` (every chat-tier channel) fails, `"operator"` passes, and an
//! absent role means "no role was recorded" — an internal/cron run — rather
//! than "the task-local died". That distinction is the whole reason the
//! ambient form could not be reused here, and it is why this one can be.
//!
//! Two things this deliberately does NOT do:
//!
//! - It does not put `project_manage` in
//!   [`crate::gateway::method_authz`]'s `OPERATOR_TOOLS`. That gate is
//!   per-TOOL, and `list` / `get` / `create` / `member_list` are actions plain
//!   members are meant to use; promoting the whole tool would refuse eight
//!   verbs to close one. The gate wanted here is per-ACTION.
//! - It does not gate UNBINDING. Passing no path is a de-escalation, and the
//!   RPC face has always left it reachable for exactly the situation that
//!   needs it most — a room stuck on a folder that has gone missing, which
//!   `build_run_request` refuses to run in. Gating the way out of a broken
//!   state is how a gate starts pushing people toward the wider setting.
//!
//! ## Still deliberately NOT here: `bind_channel`
//!
//! `projects.channel.bind` points a room at a channel group conversation —
//! its exposure runs OUTWARD, into an audience the roster does not control,
//! which is a different question from naming a folder on this machine. It
//! stays on the Panel/RPC/CLI faces (spec §7).
//!
//! **`DESCRIPTION` names those same three faces, in those same words**, and
//! `the_model_facing_copy_names_the_same_faces` pins the spelling. Until
//! 2026-08-30 it named one of the three ("on the Panel"): under-inclusive
//! rather than false, which is why nothing red — and it is the copy the model
//! obeys, so it is also the sentence a user asking "then where?" gets relayed.
//! The CLI face shipped in this same round, which is what made one-of-three
//! wrong; a fourth face has to update all three carriers, and the guard is
//! what makes that a red rather than a memory.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{AlephError, Result};
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::events::ChangeKind;
use crate::gateway::security::SecurityStore;
use crate::projects::{authz, Project, ProjectStore};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// What to do. One verb per action, no overloading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProjectAction {
    /// Rooms this caller is on.
    List,
    /// One room, with its roster.
    Get,
    /// Create a room owned by this caller.
    Create,
    /// Rename a room (owner or org admin).
    Rename,
    /// Archive a room (owner or org admin). Members and memory are kept.
    Archive,
    /// Add a user to the roster (owner or org admin).
    MemberAdd,
    /// Remove a user from the roster (owner or org admin).
    MemberRemove,
    /// The roster of one room.
    MemberList,
    /// Point the room at a folder, or (with no `path`) release it.
    ///
    /// Owner-or-admin like the other mutations, and additionally
    /// operator-tier when a path is named — see the module doc.
    BindWorkspace,
}

/// Arguments for `project_manage`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ProjectManageArgs {
    /// Which operation to perform.
    pub action: ProjectAction,
    /// Room id. Required by every action except `list` and `create`.
    #[serde(default)]
    pub project_id: Option<String>,
    /// New room name. Required by `create` and `rename`.
    #[serde(default)]
    pub name: Option<String>,
    /// Target user id. Required by `member_add` / `member_remove`.
    #[serde(default)]
    pub user_id: Option<String>,
    /// Absolute folder for `bind_workspace`. Omitted (or empty) RELEASES the
    /// room's folder rather than being a missing-argument error — that is the
    /// same shape `projects.bind_workspace` has on the wire, and the
    /// difference is load-bearing: releasing is the repair for a room pointed
    /// at a folder that no longer exists, so it must not need the same
    /// authority naming one does.
    #[serde(default)]
    pub path: Option<String>,
}

/// One room as the model sees it.
///
/// Shares [`crate::gateway::handlers::projects::ProjectView`]'s projection so
/// the fields the model reads are the fields a client reads. A separate shape
/// here would be a second answer to "what is a project row".
pub type ProjectRow = crate::gateway::handlers::projects::ProjectView;

/// Output of `project_manage`.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectManageOutput {
    /// Echo of the action, so a transcript reads without the request.
    pub action: ProjectAction,
    /// Populated by `get` / `create` / `rename`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectRow>,
    /// Populated by `list`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<ProjectRow>,
    /// Populated by `member_list` / `member_add` / `member_remove`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub member_ids: Vec<String>,
    /// One line for the model to relay.
    pub message: String,
}

/// Conversational management of project rooms.
#[derive(Clone)]
pub struct ProjectManageTool {
    store: Arc<ProjectStore>,
    /// Resolves org-admin standing. Absent before the security store is
    /// injected, in which case admin escalation simply does not apply — the
    /// owner path still works, and the failure direction is refusal.
    users: Option<Arc<SecurityStore>>,
    /// Announces mutations to connected clients. Absent in a headless or
    /// partially-booted process; a change still lands, it just does not
    /// live-refresh anyone.
    events: Option<Arc<GatewayEventBus>>,
}

impl ProjectManageTool {
    #[must_use]
    pub const fn new(
        store: Arc<ProjectStore>,
        users: Option<Arc<SecurityStore>>,
        events: Option<Arc<GatewayEventBus>>,
    ) -> Self {
        Self {
            store,
            users,
            events,
        }
    }
}

impl ProjectManageTool {
    /// Who is asking, this call.
    fn actor() -> Option<String> {
        crate::gateway::visibility::ambient_actor()
    }

    /// Resolve a room the actor may address, or the one refusal shape.
    ///
    /// "Not on the roster" and "does not exist" are one message on purpose:
    /// a distinct refusal would tell a stranger the room is real.
    fn room(&self, id: &str, actor: Option<&str>) -> Result<Project> {
        authz::project_for(&self.store, id, actor)
            .ok_or_else(|| AlephError::tool(format!("project not found: {id}")))
    }

    /// Refuse unless the actor may reconfigure `project`.
    fn require_owner(&self, project: &Project, actor: Option<&str>) -> Result<()> {
        let is_admin = match (actor, self.users.as_ref()) {
            (Some(caller), Some(users)) => matches!(
                users.get_user(caller),
                Ok(Some(u))
                    if u.role == crate::gateway::security::store::UserRole::Admin
                        && u.status == crate::gateway::security::store::UserStatus::Active
            ),
            _ => false,
        };
        if authz::is_owner(project, actor, is_admin) {
            return Ok(());
        }
        Err(AlephError::tool(
            "not the project owner: only the room's owner or an org admin may do that".to_string(),
        ))
    }

    /// Refuse unless `target` may be dropped from `project`'s roster.
    ///
    /// The decision core lives in [`authz::may_remove_member`] — this is a
    /// thin wrapper so the tool face asks the same question the RPC face
    /// does (`handlers/projects.rs::handle_member_remove`), not a second
    /// spelling of it.
    fn require_removable(project: &Project, target: &str) -> Result<()> {
        if authz::may_remove_member(project, target) {
            return Ok(());
        }
        Err(AlephError::tool(authz::OWNER_REMOVAL_REFUSAL.to_string()))
    }

    /// Refuse a roster mutation naming somebody who is not an active
    /// principal.
    ///
    /// Skipped entirely when no security store was injected — `self.users ==
    /// None` reads as unrestricted here, matching [`Self::require_owner`]'s
    /// admin escalation and every other predicate on this face that depends
    /// on the store. The decision core, when the store is present, is
    /// [`authz::is_active_principal`] — the same one
    /// `handlers/projects.rs::require_known_user` asks.
    fn require_known_user(&self, user_id: &str) -> Result<()> {
        match self.users.as_deref() {
            Some(users) if !authz::is_active_principal(users, user_id) => {
                Err(AlephError::tool(authz::unknown_user_refusal(user_id)))
            }
            _ => Ok(()),
        }
    }

    /// Announce a mutation on the same channel the RPC face uses.
    fn announce(&self, project_id: &str, change: ChangeKind, affected_user: Option<&str>) {
        if let Some(bus) = self.events.as_ref() {
            crate::projects::events::publish_changed(bus, project_id, change, affected_user);
        }
    }

    fn render(&self, project: Project) -> Result<ProjectRow> {
        let members = self
            .store
            .members(&project.id)
            .map_err(|e| AlephError::tool(format!("failed to read roster: {e}")))?;
        Ok(crate::gateway::handlers::projects::render_project(
            project, members,
        ))
    }

    /// Refuse unless the running turn is config-tier.
    ///
    /// The in-run equivalent of the RPC face's `require_directory_choice` →
    /// `caller_may_choose_directory()`. It is not a second derivation of that
    /// question: it reads
    /// [`TurnContext::caller_is_operator`](crate::tools::turn_context::TurnContext::caller_is_operator),
    /// the very method `src/tools/scoped/dispatch.rs`'s `check_operator_gate`
    /// calls when it decides whether a config-tier tool may run at all.
    ///
    /// `is_loopback` has no in-run meaning and so is not consulted: a run is
    /// not a connection, and the gateway has already resolved the originating
    /// connection's authority into `TurnContext::caller_role` by the time any
    /// tool executes. `None` there means "no role was recorded" — a cron or
    /// in-process run — which the config-tier predicate has always admitted;
    /// it is NOT the dead-task-local `None` that made
    /// `caller_may_choose_directory()` unusable on this face.
    fn require_operator_tier() -> Result<()> {
        let operator = crate::tools::turn_context::current_turn_context()
            .is_none_or(|t| t.caller_is_operator());
        if operator {
            return Ok(());
        }
        Err(AlephError::tool(
            "binding a project workspace requires an operator-tier session: that folder \
             becomes every member's working directory. Releasing it (omit `path`) does not.",
        ))
    }

    fn need<'a>(value: Option<&'a String>, field: &str, action: ProjectAction) -> Result<&'a str> {
        value
            .map(String::as_str)
            .ok_or_else(|| AlephError::tool(format!("`{field}` is required for action={action:?}")))
    }
}

#[async_trait]
impl AlephTool for ProjectManageTool {
    const NAME: &'static str = "project_manage";
    const DESCRIPTION: &'static str =
        "Manage project rooms — shared workspaces with their own roster, memory and group chat. \
         Actions: list (rooms you are on), get, create, rename, archive, member_add, \
         member_remove, member_list, bind_workspace. Rename/archive/member changes need you to \
         be the room's owner or an org admin. A room you are not on reads as not found, so an \
         id that comes back missing may simply not be yours. bind_workspace with a path points \
         the room at a folder, which becomes every member's working directory, so it also needs \
         an operator-tier session; with no path it RELEASES the folder and needs only \
         ownership, which is the repair when a room's folder has gone missing. Binding a room \
         to a chat channel is a separate, operator-only action on the Panel/RPC/CLI faces, \
         not here.";

    type Args = ProjectManageArgs;
    type Output = ProjectManageOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let actor = Self::actor();
        let actor = actor.as_deref();
        let action = args.action;

        match action {
            ProjectAction::List => {
                let all = self
                    .store
                    .list()
                    .map_err(|e| AlephError::tool(format!("failed to list projects: {e}")))?;
                let mut rows = Vec::new();
                for p in all {
                    if crate::gateway::visibility::project_visible_to(&p.id, actor) {
                        rows.push(self.render(p)?);
                    }
                }
                let message = format!("{} room(s)", rows.len());
                Ok(ProjectManageOutput {
                    action,
                    project: None,
                    projects: rows,
                    member_ids: Vec::new(),
                    message,
                })
            }
            ProjectAction::Get => {
                let id = Self::need(args.project_id.as_ref(), "project_id", action)?;
                let project = self.room(id, actor)?;
                let name = project.name.clone();
                Ok(ProjectManageOutput {
                    action,
                    project: Some(self.render(project)?),
                    projects: Vec::new(),
                    member_ids: Vec::new(),
                    message: format!("room '{name}'"),
                })
            }
            ProjectAction::Create => {
                let name = Self::need(args.name.as_ref(), "name", action)?;
                let project = self
                    .store
                    .create(name, actor, None)
                    .map_err(|e| AlephError::tool(format!("failed to create project: {e}")))?;
                let id = project.id.clone();
                self.announce(&id, ChangeKind::Created, None);
                Ok(ProjectManageOutput {
                    action,
                    project: Some(self.render(project)?),
                    projects: Vec::new(),
                    member_ids: Vec::new(),
                    message: format!("created room '{name}' ({id})"),
                })
            }
            ProjectAction::Rename => {
                let id = Self::need(args.project_id.as_ref(), "project_id", action)?;
                let name = Self::need(args.name.as_ref(), "name", action)?;
                let project = self.room(id, actor)?;
                self.require_owner(&project, actor)?;
                let renamed = self
                    .store
                    .rename(id, name)
                    .map_err(|e| AlephError::tool(format!("failed to rename project: {e}")))?;
                self.announce(id, ChangeKind::Updated, None);
                Ok(ProjectManageOutput {
                    action,
                    project: Some(self.render(renamed)?),
                    projects: Vec::new(),
                    member_ids: Vec::new(),
                    message: format!("renamed to '{name}'"),
                })
            }
            ProjectAction::Archive => {
                let id = Self::need(args.project_id.as_ref(), "project_id", action)?;
                let project = self.room(id, actor)?;
                self.require_owner(&project, actor)?;
                self.store
                    .archive(id)
                    .map_err(|e| AlephError::tool(format!("failed to archive project: {e}")))?;
                self.announce(id, ChangeKind::Updated, None);
                Ok(ProjectManageOutput {
                    action,
                    project: None,
                    projects: Vec::new(),
                    member_ids: Vec::new(),
                    message: format!("archived '{}'; members and memory are kept", project.name),
                })
            }
            ProjectAction::MemberAdd => {
                let id = Self::need(args.project_id.as_ref(), "project_id", action)?;
                let user = Self::need(args.user_id.as_ref(), "user_id", action)?;
                let project = self.room(id, actor)?;
                self.require_owner(&project, actor)?;
                self.require_known_user(user)?;
                self.store
                    .add_member(id, user)
                    .map_err(|e| AlephError::tool(format!("failed to add member: {e}")))?;
                // Authority-change audit (T04): the tool face wrote NONE of
                // these before this round (`grep -c audit
                // src/builtin_tools/project_manage.rs` == 0), while the RPC
                // twin (`handlers/projects.rs`) always has. Unconditional —
                // see `ProjectStore::add_member`'s doc for why a re-grant is
                // worth a row even though a non-revocation is not. The actor
                // is `visibility::ambient_actor()`, not `CALLER_USER`: this
                // runs inside a spawned run where the latter is dead (see
                // `agent_manage/update.rs:237`'s precedent). The
                // `project_manage.` prefix is what makes this row
                // distinguishable from the RPC face's `projects.member.add`.
                if let Some(log) = crate::security::audit::global() {
                    log.log(crate::security::audit::AuditEntry::authority_change(
                        crate::gateway::visibility::ambient_actor(),
                        format!("project_manage.member_add: {user} → {id}"),
                    ));
                }
                // `affected_user` is set ONLY on removal (it is what lets a
                // just-removed member still receive the frame telling their
                // client to drop the room); an addition is visible to the
                // roster it just joined.
                self.announce(id, ChangeKind::Updated, None);
                let members = self
                    .store
                    .members(id)
                    .map_err(|e| AlephError::tool(format!("failed to read roster: {e}")))?;
                Ok(ProjectManageOutput {
                    action,
                    project: None,
                    projects: Vec::new(),
                    member_ids: members,
                    message: format!("added {user} to '{}'", project.name),
                })
            }
            ProjectAction::MemberRemove => {
                let id = Self::need(args.project_id.as_ref(), "project_id", action)?;
                let user = Self::need(args.user_id.as_ref(), "user_id", action)?;
                let project = self.room(id, actor)?;
                self.require_owner(&project, actor)?;
                Self::require_removable(&project, user)?;
                let changed = self
                    .store
                    .remove_member(id, user)
                    .map_err(|e| AlephError::tool(format!("failed to remove member: {e}")))?;
                // Authority-change audit (T04), gated on `changed` the same
                // way the RPC face is: naming somebody who was never seated
                // is not a revocation. See `MemberAdd` above for why the
                // actor is `ambient_actor()` and the `project_manage.`
                // prefix distinguishes this from the RPC face's row.
                if changed {
                    if let Some(log) = crate::security::audit::global() {
                        log.log(crate::security::audit::AuditEntry::authority_change(
                            crate::gateway::visibility::ambient_actor(),
                            format!("project_manage.member_remove: {user} ← {id}"),
                        ));
                    }
                }
                // Named here, and only here: by the time this frame is
                // published the roster no longer admits them, so without
                // `affected_user` the one person who most needs to learn they
                // were removed is the one the visibility rule now refuses.
                // Guarded on `changed`: a bystander who was never on the
                // roster must not be told they were dropped from it.
                self.announce(id, ChangeKind::Updated, changed.then_some(user));
                let members = self
                    .store
                    .members(id)
                    .map_err(|e| AlephError::tool(format!("failed to read roster: {e}")))?;
                let message = if changed {
                    format!("removed {user} from '{}'", project.name)
                } else {
                    format!("{user} was not a member of '{}'", project.name)
                };
                Ok(ProjectManageOutput {
                    action,
                    project: None,
                    projects: Vec::new(),
                    member_ids: members,
                    message,
                })
            }
            ProjectAction::MemberList => {
                let id = Self::need(args.project_id.as_ref(), "project_id", action)?;
                let project = self.room(id, actor)?;
                let members = self
                    .store
                    .members(id)
                    .map_err(|e| AlephError::tool(format!("failed to read roster: {e}")))?;
                let message = format!("{} member(s) in '{}'", members.len(), project.name);
                Ok(ProjectManageOutput {
                    action,
                    project: None,
                    projects: Vec::new(),
                    member_ids: members,
                    message,
                })
            }
            ProjectAction::BindWorkspace => {
                let id = Self::need(args.project_id.as_ref(), "project_id", action)?;
                let project = self.room(id, actor)?;
                self.require_owner(&project, actor)?;

                // Empty and absent both mean "release it", exactly as
                // `handle_bind_workspace` reads them. A trimmed-empty string
                // is the shape a model produces when it means "clear this",
                // and letting it through as a path would hand the store a
                // relative "" to canonicalise.
                let path = args
                    .path
                    .as_deref()
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(std::path::PathBuf::from);

                // Only when a path is actually NAMED — see the module doc for
                // why releasing must stay reachable from a broken state.
                if path.is_some() {
                    Self::require_operator_tier()?;
                }

                let bound = self
                    .store
                    .bind_workspace(id, path.as_deref())
                    .map_err(|e| AlephError::tool(format!("failed to bind workspace: {e}")))?;
                self.announce(id, ChangeKind::Updated, None);
                let message = bound.workspace_path.as_ref().map_or_else(
                    || format!("released '{}' from its folder", bound.name),
                    |p| format!("'{}' now works in {}", bound.name, p.display()),
                );
                Ok(ProjectManageOutput {
                    action,
                    project: Some(self.render(bound)?),
                    projects: Vec::new(),
                    member_ids: Vec::new(),
                    message,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::caller_identity::CALLER_USER;
    use crate::projects::roster::TEST_GUARD;
    use rusqlite::Connection;
    use std::sync::MutexGuard;

    fn fixture() -> (
        ProjectManageTool,
        Project,
        Arc<ProjectStore>,
        MutexGuard<'static, ()>,
    ) {
        let guard = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let store = Arc::new(ProjectStore::new(Connection::open_in_memory().unwrap()));
        store.create_schema().unwrap();
        let users = Arc::new(SecurityStore::in_memory().unwrap());
        for (id, role) in [
            ("u-alice", crate::gateway::security::store::UserRole::Member),
            ("u-bob", crate::gateway::security::store::UserRole::Member),
            ("u-carol", crate::gateway::security::store::UserRole::Admin),
        ] {
            users.create_user(id, id, role).unwrap();
        }
        let project = store.create("shared room", Some("u-alice"), None).unwrap();
        store.add_member(&project.id, "u-bob").unwrap();
        let tool = ProjectManageTool::new(Arc::clone(&store), Some(users), None);
        (tool, project, store, guard)
    }

    async fn as_user<T>(user: &str, fut: impl std::future::Future<Output = T>) -> T {
        CALLER_USER.scope(Some(user.to_string()), fut).await
    }

    #[tokio::test]
    async fn list_shows_only_the_rooms_the_actor_is_on() {
        let (tool, project, store, _g) = fixture();
        let other = store.create("someone else", Some("u-carol"), None).unwrap();

        let out = as_user("u-bob", async {
            tool.call(ProjectManageArgs {
                action: ProjectAction::List,
                project_id: None,
                name: None,
                user_id: None,
                path: None,
            })
            .await
        })
        .await
        .expect("list succeeds");
        let ids: Vec<&str> = out.projects.iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&project.id.as_str()), "bob is on this one");
        assert!(!ids.contains(&other.id.as_str()), "and not on that one");
    }

    /// The tool face must refuse a non-owner for the same reason the RPC face
    /// does — and refuse a stranger by making the room look absent, so the
    /// refusal is not an existence oracle.
    #[tokio::test]
    async fn a_member_cannot_rename_and_a_stranger_cannot_tell_the_room_exists() {
        let (tool, project, _store, _g) = fixture();

        let member = as_user("u-bob", async {
            tool.call(ProjectManageArgs {
                action: ProjectAction::Rename,
                project_id: Some(project.id.clone()),
                name: Some("bob's room".into()),
                user_id: None,
                path: None,
            })
            .await
        })
        .await;
        let msg = member
            .expect_err("a plain member may not rename")
            .to_string();
        assert!(msg.contains("not the project owner"), "got: {msg}");

        let stranger = as_user("u-mallory", async {
            tool.call(ProjectManageArgs {
                action: ProjectAction::Rename,
                project_id: Some(project.id.clone()),
                name: Some("mine now".into()),
                user_id: None,
                path: None,
            })
            .await
        })
        .await;
        let absent = as_user("u-mallory", async {
            tool.call(ProjectManageArgs {
                action: ProjectAction::Rename,
                project_id: Some("p-nope".into()),
                name: Some("mine now".into()),
                user_id: None,
                path: None,
            })
            .await
        })
        .await;
        assert_eq!(
            stranger.expect_err("refused").to_string(),
            absent
                .expect_err("absent")
                .to_string()
                .replace("p-nope", &project.id),
            "a room you are not on must read exactly like one that does not exist"
        );
    }

    #[tokio::test]
    async fn the_owner_and_an_org_admin_may_both_change_the_roster() {
        let (tool, project, store, _g) = fixture();

        as_user("u-alice", async {
            tool.call(ProjectManageArgs {
                action: ProjectAction::MemberAdd,
                project_id: Some(project.id.clone()),
                name: None,
                user_id: Some("u-carol".into()),
                path: None,
            })
            .await
        })
        .await
        .expect("the owner may add");
        assert!(store
            .members(&project.id)
            .unwrap()
            .contains(&"u-carol".to_string()));

        as_user("u-carol", async {
            tool.call(ProjectManageArgs {
                action: ProjectAction::MemberRemove,
                project_id: Some(project.id.clone()),
                name: None,
                user_id: Some("u-bob".into()),
                path: None,
            })
            .await
        })
        .await
        .expect("an org admin may remove");
        assert!(!store
            .members(&project.id)
            .unwrap()
            .contains(&"u-bob".to_string()));
    }

    /// `project_manage.rs:4-9` claims both faces go through `projects::authz`
    /// for who-may-do-what. This is the tool-face half of the owner-removal
    /// rule the RPC face already had (`handlers/projects.rs`'s
    /// `the_owner_cannot_be_removed_from_their_own_roster`): neither the
    /// owner themself nor an org admin may drop the owner off the roster,
    /// because the roster IS the visibility predicate and doing so would
    /// leave the room addressable by nobody.
    #[tokio::test]
    async fn the_tool_face_also_refuses_to_remove_the_owner() {
        let (tool, project, store, _g) = fixture();
        // carol is an org admin but, like the RPC face
        // (`the_owner_and_an_org_admin_may_both_add_a_member`), admin is not
        // a bypass of the roster visibility gate: she must be seated before
        // `room()` will resolve the project for her at all.
        store.add_member(&project.id, "u-carol").unwrap();
        let before = store.members(&project.id).unwrap();

        for actor in ["u-alice", "u-carol"] {
            let denied = as_user(actor, async {
                tool.call(ProjectManageArgs {
                    action: ProjectAction::MemberRemove,
                    project_id: Some(project.id.clone()),
                    name: None,
                    user_id: Some("u-alice".into()),
                    path: None,
                })
                .await
            })
            .await;
            let msg = denied
                .expect_err(&format!("{actor} may not remove the owner"))
                .to_string();
            assert!(
                msg.contains(authz::OWNER_REMOVAL_REFUSAL),
                "tool-face refusal must be the shared authz text, got: {msg}"
            );
        }
        assert_eq!(
            store.members(&project.id).unwrap(),
            before,
            "the roster must be unchanged by either refused attempt"
        );
    }

    /// T04: the tool face was auditing NOTHING (`grep -c audit
    /// src/builtin_tools/project_manage.rs` = 0). A successful `member_add`
    /// and `member_remove` must each write exactly one AuthorityChange row,
    /// and its detail must be distinguishable from the RPC face's string for
    /// the same verb (`handlers/projects.rs`'s `projects.member.add` /
    /// `projects.member.remove`) — otherwise `aleph audit` cannot tell which
    /// face a grant or revocation came through.
    #[tokio::test]
    async fn member_add_and_remove_are_each_audited_once_and_distinguishably() {
        let _serial = crate::security::audit::AUDIT_TEST_LOCK.lock().unwrap();
        let (log, mut rx) = crate::security::audit::SecurityAuditLog::new(16);
        crate::security::audit::replace_global_for_test(&log);

        let (tool, project, _store, _g) = fixture();

        as_user("u-alice", async {
            tool.call(ProjectManageArgs {
                action: ProjectAction::MemberAdd,
                project_id: Some(project.id.clone()),
                name: None,
                user_id: Some("u-carol".into()),
                path: None,
            })
            .await
        })
        .await
        .expect("the owner may add");

        as_user("u-alice", async {
            tool.call(ProjectManageArgs {
                action: ProjectAction::MemberRemove,
                project_id: Some(project.id.clone()),
                name: None,
                user_id: Some("u-carol".into()),
                path: None,
            })
            .await
        })
        .await
        .expect("the owner may remove");

        // The audit log is process-global and `cargo test` runs threads in
        // parallel, so other tests exercising this same tool concurrently
        // can write into the channel this test just installed. Filter to
        // this test's own project id rather than assuming the channel holds
        // only what this test produced.
        let mut details = Vec::new();
        while let Ok(entry) = rx.try_recv() {
            assert_eq!(
                entry.event_type,
                crate::security::audit::AuditEventType::AuthorityChange
            );
            if entry.detail.contains(&project.id) {
                details.push(entry.detail);
            }
        }
        assert_eq!(
            details.len(),
            2,
            "exactly one AuthorityChange row per verb, got: {details:?}"
        );
        assert!(
            details.iter().all(|d| !d.starts_with("projects.member.")),
            "the tool face's rows must read differently from the RPC face's \
             `projects.member.*` strings, got: {details:?}"
        );
    }

    /// The other half of the same claim: `handlers/projects.rs`'s
    /// `require_known_user` gates `member_add` on the security store before
    /// the tool face did. A nonexistent id and a deactivated one must both
    /// be refused, and neither may leave a `project_members` row behind.
    #[tokio::test]
    async fn the_tool_face_also_refuses_an_unknown_or_deactivated_member() {
        let (tool, project, store, _g) = fixture();
        tool.users
            .as_ref()
            .expect("fixture injects a security store")
            .create_user("u-dana", "u-dana", crate::gateway::security::store::UserRole::Member)
            .unwrap();
        tool.users
            .as_ref()
            .unwrap()
            .update_user(
                "u-dana",
                None,
                None,
                Some(crate::gateway::security::store::UserStatus::Deactivated),
            )
            .unwrap();
        let before = store.members(&project.id).unwrap();

        for candidate in ["u-nobody", "u-dana"] {
            let denied = as_user("u-alice", async {
                tool.call(ProjectManageArgs {
                    action: ProjectAction::MemberAdd,
                    project_id: Some(project.id.clone()),
                    name: None,
                    user_id: Some(candidate.into()),
                    path: None,
                })
                .await
            })
            .await;
            let msg = denied.expect_err(candidate).to_string();
            assert!(
                msg.contains(&authz::unknown_user_refusal(candidate)),
                "got: {msg}"
            );
        }
        assert_eq!(
            store.members(&project.id).unwrap(),
            before,
            "no project_members row for either candidate"
        );
    }

    /// When no security store was injected, the known-user check does not
    /// apply — matching every other predicate on this face (see
    /// `ProjectManageTool::users`'s doc). The owner-removal rule needs no
    /// store at all, so it still refuses.
    #[tokio::test]
    async fn without_a_security_store_the_known_user_check_is_unrestricted() {
        let _guard = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let store = Arc::new(ProjectStore::new(Connection::open_in_memory().unwrap()));
        store.create_schema().unwrap();
        let project = store.create("solo room", Some("u-alice"), None).unwrap();
        let tool = ProjectManageTool::new(Arc::clone(&store), None, None);

        let added = as_user("u-alice", async {
            tool.call(ProjectManageArgs {
                action: ProjectAction::MemberAdd,
                project_id: Some(project.id.clone()),
                name: None,
                user_id: Some("u-anyone".into()),
                path: None,
            })
            .await
        })
        .await;
        assert!(
            added.is_ok(),
            "no security store injected ⇒ the known-user gate does not apply: {added:?}"
        );
    }

    /// The half of the 2026-08-25 ruling that did NOT change: pointing a room
    /// at a channel conversation is not an action here. Asserted on the SCHEMA
    /// rather than in prose, so a future action added without re-opening that
    /// ruling fails here rather than shipping.
    ///
    /// `bind_workspace` used to be asserted absent by this same test. It is
    /// now asserted PRESENT below — the ruling was overturned on 2026-08-29
    /// once an in-run operator predicate existed (see the module doc), and the
    /// two halves were never the same question: naming a folder on this
    /// machine is bounded by the machine, while binding a chat conversation
    /// exposes the room outward to an audience the roster does not control.
    #[test]
    fn binding_a_chat_conversation_is_still_not_an_action() {
        let schema = serde_json::to_string(&schemars::schema_for!(ProjectAction))
            .expect("action schema serializes");
        assert!(
            !schema.contains("bind_channel") && !schema.contains("channel"),
            "binding a room to a channel conversation exposes it outward, past the roster; \
             it stays on the Panel/RPC/CLI faces (spec §7): {schema}"
        );
        for expected in [
            "list",
            "get",
            "create",
            "rename",
            "archive",
            "member_add",
            "member_remove",
            "member_list",
            "bind_workspace",
        ] {
            assert!(schema.contains(expected), "missing action {expected}");
        }
    }

    /// The three carriers of "where binding lives" must agree, and the one the
    /// model obeys is the one with no compiler behind it.
    ///
    /// The module doc and the assertion message above both say
    /// "Panel/RPC/CLI faces"; `DESCRIPTION` said "on the Panel" for the whole
    /// round in which the CLI face shipped. Nothing could have caught that:
    /// a description is a `&str`, a doc comment is a comment, and neither
    /// reads the other. This is the cheapest thing that reds when they drift.
    #[test]
    fn the_model_facing_copy_names_the_same_faces() {
        let desc = <ProjectManageTool as crate::tools::AlephTool>::DESCRIPTION;
        assert!(
            desc.contains("Panel/RPC/CLI faces"),
            "the model-facing copy must name the same faces as the module doc \
             and the schema guard, in the same words, so one grep finds all \
             three: {desc}"
        );
    }

    /// A turn carrying `role`, as the gateway stamps one per turn.
    fn turn_as(role: Option<&str>) -> crate::tools::turn_context::TurnContext {
        crate::tools::turn_context::TurnContext {
            session_key: crate::routing::session_key::SessionKey::main("main"),
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: role.map(str::to_string),
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
        }
    }

    /// Run `fut` as `user`, inside a turn whose connection had `role`.
    ///
    /// Both scopes, because the two answer different questions and this verb
    /// asks both: `CALLER_USER` decides WHICH ROOMS this actor may address
    /// (ownership), `TURN_CONTEXT` decides whether this connection may name a
    /// server-side folder at all (tier). A test that scoped only one would
    /// pass for the wrong reason.
    async fn as_user_at_tier<T>(
        user: &str,
        role: Option<&str>,
        fut: impl std::future::Future<Output = T>,
    ) -> T {
        crate::tools::turn_context::TURN_CONTEXT
            .scope(
                turn_as(role),
                CALLER_USER.scope(Some(user.to_string()), fut),
            )
            .await
    }

    fn bind_args(project_id: &str, path: Option<&str>) -> ProjectManageArgs {
        ProjectManageArgs {
            action: ProjectAction::BindWorkspace,
            project_id: Some(project_id.to_string()),
            name: None,
            user_id: None,
            path: path.map(str::to_string),
        }
    }

    /// The gate the tool face applies must be the SAME predicate the dispatch
    /// chokepoint applies, not a lookalike written next to it.
    ///
    /// Pinned against `turn_context::role_is_operator` — the function
    /// `check_operator_gate` reaches through `TurnContext::caller_is_operator`
    /// — rather than against a literal list of role strings. A list would say
    /// "guest is refused today"; this says "this face refuses exactly whom the
    /// config-tier gate refuses", which is the property that has to survive a
    /// new role being introduced somewhere else entirely.
    #[tokio::test]
    async fn the_tier_gate_admits_exactly_who_the_config_tier_gate_admits() {
        for role in [None, Some("operator"), Some("guest"), Some("member")] {
            let admitted = crate::tools::turn_context::TURN_CONTEXT
                .scope(turn_as(role), async {
                    ProjectManageTool::require_operator_tier().is_ok()
                })
                .await;
            assert_eq!(
                admitted,
                crate::tools::turn_context::role_is_operator(role),
                "role {role:?}: this face and the config-tier gate must agree"
            );
        }
    }

    /// A chat-tier run binding a workspace would point every member of the
    /// room at an arbitrary folder with no human in the loop.
    ///
    /// The second assertion is the one that matters: a refusal that still
    /// wrote the row would be a gate that reports "no" and means "yes".
    #[tokio::test]
    async fn a_chat_tier_run_may_not_bind_a_workspace() {
        let (tool, project, store, _g) = fixture();
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().to_string_lossy().to_string();

        let err = as_user_at_tier("u-alice", Some("guest"), async {
            tool.call(bind_args(&project.id, Some(&path))).await
        })
        .await
        .expect_err("a chat-tier run must be refused");
        assert!(
            err.to_string().contains("operator-tier"),
            "the refusal must name what is missing, not just decline: {err}"
        );
        assert_eq!(
            store.get(&project.id).unwrap().unwrap().workspace_path,
            None,
            "the refusal must not have written the folder anyway"
        );
    }

    /// The gate is ADDITIONAL to ownership, not a replacement for it.
    ///
    /// Without this, an operator-tier connection could redirect the working
    /// directory of a room belonging to somebody else — the tier answers "may
    /// this connection name paths", never "is this room yours".
    #[tokio::test]
    async fn operator_tier_does_not_substitute_for_owning_the_room() {
        let (tool, project, store, _g) = fixture();
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().to_string_lossy().to_string();

        let err = as_user_at_tier("u-bob", Some("operator"), async {
            tool.call(bind_args(&project.id, Some(&path))).await
        })
        .await
        .expect_err("a plain member may not rebind the room");
        assert!(
            err.to_string().contains("not the project owner"),
            "got: {err}"
        );
        assert_eq!(
            store.get(&project.id).unwrap().unwrap().workspace_path,
            None
        );
    }

    /// The owner at operator tier binds, and the row actually moves.
    #[tokio::test]
    async fn the_owner_at_operator_tier_binds_the_folder() {
        let (tool, project, store, _g) = fixture();
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().to_string_lossy().to_string();

        let out = as_user_at_tier("u-alice", Some("operator"), async {
            tool.call(bind_args(&project.id, Some(&path))).await
        })
        .await
        .expect("the owner at operator tier may bind");
        assert!(
            out.project
                .expect("a bound room comes back")
                .workspace_path
                .is_some(),
            "the receipt must show the folder it says it bound"
        );
        assert!(store
            .get(&project.id)
            .unwrap()
            .unwrap()
            .workspace_path
            .is_some());
    }

    /// Releasing is a de-escalation and is NOT tier-gated — which is load
    /// bearing rather than lenient: the state that most needs releasing is a
    /// room pointed at a folder that has gone missing, and `build_run_request`
    /// refuses to run there. A tier gate on the way out would leave a
    /// chat-tier owner with a room they cannot use and cannot repair, whose
    /// only escape is asking for a wider connection.
    ///
    /// Both spellings are asserted, because a model that means "clear this"
    /// produces either, and only one of them is an obvious no-path call.
    #[tokio::test]
    async fn releasing_a_folder_is_not_tier_gated() {
        let (tool, project, store, _g) = fixture();
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().to_string_lossy().to_string();

        for release_with in [None, Some("   ")] {
            as_user_at_tier("u-alice", Some("operator"), async {
                tool.call(bind_args(&project.id, Some(&path))).await
            })
            .await
            .expect("bound first");

            as_user_at_tier("u-alice", Some("guest"), async {
                tool.call(bind_args(&project.id, release_with)).await
            })
            .await
            .unwrap_or_else(|e| panic!("releasing with {release_with:?} must not be gated: {e}"));

            assert_eq!(
                store.get(&project.id).unwrap().unwrap().workspace_path,
                None,
                "releasing with {release_with:?} must actually clear the folder"
            );
        }
    }

    /// A missing required field is a caller error with a name in it, not a
    /// panic and not a silent no-op on some other room.
    #[tokio::test]
    async fn a_missing_required_field_names_itself() {
        let (tool, _project, _store, _g) = fixture();
        let err = as_user("u-alice", async {
            tool.call(ProjectManageArgs {
                action: ProjectAction::Get,
                project_id: None,
                name: None,
                user_id: None,
                path: None,
            })
            .await
        })
        .await
        .expect_err("get needs a project_id");
        assert!(err.to_string().contains("project_id"), "got: {err}");
    }

    /// An unrestricted caller (cron, A2A, in-process) is admitted, the same
    /// first arm every predicate in this codebase opens with — and creating a
    /// room without an actor leaves it unowned rather than guessing an owner.
    #[tokio::test]
    async fn an_unrestricted_caller_may_create_and_read_back() {
        let (tool, _project, _store, _g) = fixture();
        let out = tool
            .call(ProjectManageArgs {
                action: ProjectAction::Create,
                project_id: None,
                name: Some("headless room".into()),
                user_id: None,
                path: None,
            })
            .await
            .expect("create succeeds");
        let row = out.project.expect("a created room comes back");
        assert_eq!(row.name, "headless room");
        assert_eq!(row.owner_user_id, None);
    }
}
