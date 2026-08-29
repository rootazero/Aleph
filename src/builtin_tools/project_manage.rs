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
//! ## Deliberately NOT here: `bind_workspace`
//!
//! Binding a room to a directory is the fifth writer of `workspace_path`, and
//! since P2 that column is the default cwd of every member's run — so a writer
//! is a permission grant, not a preference. The RPC face gates it with
//! `caller_may_choose_directory()`, which as of 2026-08-28 (`546984c2b`)
//! REFUSES a caller with no connection role (cron, A2A, in-process) unless the
//! connection is loopback — narrowed from the fail-OPEN form this comment used
//! to describe. That narrowing does not make the ambient form safe to reuse on
//! THIS face either: `CALLER_ROLE` is dead past the `tokio::spawn` every run
//! crosses, so a tool call here always sees `None` role and non-loopback, and
//! the gate would always refuse — silently blocking a legitimate room's
//! automation rather than admitting a hostile one. Reaching the right answer
//! needs the run's actually-enforced role, read from `ScopedToolService` and
//! passed to `caller_may_choose_directory_as` explicitly.
//!
//! The alternative — a stricter predicate just for this face — would be a
//! second answer to "may this actor name a server directory", and two answers
//! to one question is the shape this codebase has watched drift more than
//! once. So the verb stays on the Panel/RPC path, where the gate it needs
//! already means what it says. Ruled 2026-08-25.

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
         member_remove, member_list. Rename/archive/member changes need you to be the room's \
         owner or an org admin. A room you are not on reads as not found, so an id that comes \
         back missing may simply not be yours. Binding a room to a folder is NOT here: that \
         changes every member's working directory, so it stays in the Panel's room settings.";

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
                self.store
                    .add_member(id, user)
                    .map_err(|e| AlephError::tool(format!("failed to add member: {e}")))?;
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
                self.store
                    .remove_member(id, user)
                    .map_err(|e| AlephError::tool(format!("failed to remove member: {e}")))?;
                // Named here, and only here: by the time this frame is
                // published the roster no longer admits them, so without
                // `affected_user` the one person who most needs to learn they
                // were removed is the one the visibility rule now refuses.
                self.announce(id, ChangeKind::Updated, Some(user));
                let members = self
                    .store
                    .members(id)
                    .map_err(|e| AlephError::tool(format!("failed to read roster: {e}")))?;
                Ok(ProjectManageOutput {
                    action,
                    project: None,
                    projects: Vec::new(),
                    member_ids: members,
                    message: format!("removed {user} from '{}'", project.name),
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

    /// `bind_workspace` is deliberately absent (see the module doc). Asserted
    /// on the SCHEMA rather than in prose, so a future action added without
    /// re-opening that ruling fails here rather than shipping.
    #[test]
    fn the_action_set_does_not_include_binding_a_directory() {
        let schema = serde_json::to_string(&schemars::schema_for!(ProjectAction))
            .expect("action schema serializes");
        assert!(
            !schema.contains("bind_workspace") && !schema.contains("workspace"),
            "binding a room to a directory changes every member's cwd; it stays on the \
             Panel/RPC path where caller_may_choose_directory means what it says: {schema}"
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
        ] {
            assert!(schema.contains(expected), "missing action {expected}");
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
            })
            .await
            .expect("create succeeds");
        let row = out.project.expect("a created room comes back");
        assert_eq!(row.name, "headless room");
        assert_eq!(row.owner_user_id, None);
    }
}
