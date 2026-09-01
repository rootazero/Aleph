//! On-disk project catalogue. See [`crate::projects`] for the module-level
//! contract.
//!
//! Storage is `~/.aleph/data/projects.db` (SQLite). Two tables:
//!
//! - `projects` — the entity: id, name, owner, bound workspace, status.
//! - `project_members` — the roster. **Membership IS the authorization**
//!   (spec §6.1); there are no per-resource grants in v1.
//!
//! One table, two views (human ruling 2026-08-06): the Panel's
//! "recent working directory" picker is this same table filtered to rows that
//! have a `workspace_path`, ordered by `last_used_at`. That is why [`add`] /
//! [`create_blank`] / [`find_by_path`] survive the promotion unchanged in
//! behaviour — they are the picker's entry points and they land on
//! [`ProjectStore::create`] underneath.
//!
//! [`add`]: ProjectStore::add
//! [`create_blank`]: ProjectStore::create_blank
//! [`find_by_path`]: ProjectStore::find_by_path

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::SystemTime;

use aleph_protocol::projects::BindingPeerKind;
use rusqlite::{Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::gateway::security::store::OWNER_USER_ID;
use crate::projects::binding::{self, ChannelBinding, ClaimSource};
use crate::projects::roster::{self, RosterSnapshot};
use crate::sync_primitives::{Arc, Mutex};

/// Whether a project is live or filed away. Archived projects keep their
/// roster and their memory partition — archiving is not deletion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectStatus {
    Active,
    Archived,
}

impl ProjectStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Archived => "archived",
        }
    }

    /// Parse a stored status. An unrecognised value reads as `Archived` rather
    /// than `Active`: a row we cannot interpret must not silently become a live
    /// room.
    #[must_use]
    pub fn from_stored(s: &str) -> Self {
        if s == "active" {
            Self::Active
        } else {
            Self::Archived
        }
    }
}

/// One project room.
///
/// `owner_user_id` is `None` on rows created before the multi-user arc and on
/// rows created outside any dispatch scope (CLI, internal). Absent reads as
/// [`OWNER_USER_ID`] — adoption by absence, zero backfill. The single
/// derivation of that rule is `gateway::visibility::owner_or_legacy`; never
/// re-spell it here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    /// Stable identifier, `p-<uuid simple>`. The `p-` prefix is load-bearing:
    /// [`crate::memory::project_scope::SCOPED_FAMILIES`] and
    /// `gateway::visibility::partition_visible` both key on it to tell a
    /// project partition from the legacy `proj-` directory family (note
    /// `"proj-…"` does not start with `"p-"`, so the two cannot collide).
    pub id: String,
    /// Display name. Defaults to the folder's basename for path-registered
    /// projects.
    pub name: String,
    pub owner_user_id: Option<String>,
    /// Absolute path this room is bound to, canonicalised at insert time.
    /// `None` for a room with no workspace of its own.
    pub workspace_path: Option<PathBuf>,
    pub status: ProjectStatus,
    /// Unix-seconds creation time.
    pub created_at: i64,
    /// Unix-seconds last mutation time.
    pub updated_at: i64,
    /// Unix-seconds last activation time, bumped by [`ProjectStore::touch`]
    /// so the recent-directory view can sort by recency.
    pub last_used_at: i64,
    /// The room's canonical chat session, or `None` before any member has
    /// opened it.
    ///
    /// Server-side because it is a fact shared BETWEEN devices: a room is one
    /// conversation for every member (spec §6.4), and the per-browser
    /// `localStorage` map this replaces could not express that — each member's
    /// Panel opened its own session and never saw the others' turns.
    /// [`ProjectStore::claim_session_key`] is the only writer.
    pub current_session_key: Option<String>,
}

/// Typed errors surfaced to RPC handlers.
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("db: {0}")]
    Db(String),
    #[error("path not absolute: {0}")]
    NotAbsolute(PathBuf),
    #[error("path is not a directory: {0}")]
    NotDirectory(PathBuf),
    #[error("project not found: {0}")]
    NotFound(String),
    #[error("project already exists: {0}")]
    AlreadyExists(PathBuf),
    #[error("invalid project name: {0}")]
    InvalidName(String),
    /// The verb is well-formed and the project exists, but applying it here
    /// would destroy something the caller cannot see — currently only
    /// [`ProjectStore::remove`] on a room that has a conversation.
    ///
    /// Deliberately NOT `NotFound`: the caller already knows this project
    /// exists (they named it and they are its owner), so there is no existence
    /// to leak, and an honest refusal that names the alternative is what makes
    /// the boundary actionable. Same split as `handlers/projects.rs`'s
    /// `gate_project` (not-found) vs `require_owner` (forbidden).
    #[error("{0}")]
    Invalid(String),
}

fn db_err(e: impl std::fmt::Display) -> ProjectError {
    ProjectError::Db(e.to_string())
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Mint a fresh project id. See [`Project::id`] for why the prefix matters.
fn mint_id() -> String {
    format!("p-{}", uuid::Uuid::new_v4().simple())
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS projects (
    id                  TEXT PRIMARY KEY,
    name                TEXT NOT NULL,
    owner_user_id       TEXT,
    workspace_path      TEXT,
    status              TEXT NOT NULL DEFAULT 'active',
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    last_used_at        INTEGER NOT NULL,
    current_session_key TEXT
);
CREATE TABLE IF NOT EXISTS project_members (
    project_id TEXT NOT NULL,
    user_id    TEXT NOT NULL,
    added_at   INTEGER NOT NULL,
    PRIMARY KEY (project_id, user_id)
);
CREATE INDEX IF NOT EXISTS idx_project_members_user ON project_members(user_id);
";

/// The owner-keyed uniqueness rule for bound workspaces, kept out of [`SCHEMA`]
/// only because it interpolates [`OWNER_USER_ID`].
///
/// Deliberately NOT global: a global unique index on `workspace_path` would
/// tell bob that alice has already bound this folder — a cross-user existence
/// oracle, the exact defect `idx_teams_name_active` shipped with. And
/// deliberately `COALESCE`, not the raw column: SQLite treats NULLs as
/// distinct, so keying on the raw column would silently drop the constraint
/// for every unstamped row — i.e. for every row in a single-user database.
fn workspace_uniqueness_ddl() -> String {
    format!(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_projects_owner_path_active
             ON projects(COALESCE(owner_user_id, '{OWNER_USER_ID}'), workspace_path)
             WHERE workspace_path IS NOT NULL AND status = 'active';"
    )
}

/// The room ⟷ conversation binding table.
///
/// Kept out of [`SCHEMA`] for the same reason `workspace_uniqueness_ddl` is:
/// it is created together with its own index, and both must be applied AFTER
/// the column migrations above. It references no column added by a migration,
/// so it is safe against a pre-rooms catalogue — but the index is written here
/// rather than in `SCHEMA` so that the table and the constraint that gives it
/// its meaning can never be applied apart.
const BINDING_DDL: &str = "
CREATE TABLE IF NOT EXISTS project_channel_bindings (
    project_id TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    peer_kind  TEXT NOT NULL,
    peer_id    TEXT NOT NULL,
    bound_by   TEXT,
    bound_at   INTEGER NOT NULL,
    label      TEXT,
    PRIMARY KEY (channel_id, peer_kind, peer_id)
);
CREATE INDEX IF NOT EXISTS idx_project_channel_bindings_project
    ON project_channel_bindings(project_id);
";

/// SQLite-backed catalogue.
///
/// Cheap to clone (the connection is shared). The API is synchronous — hence
/// [`std::sync::Mutex`] rather than the tokio mutex `src/teams/store.rs` uses:
/// a sync method cannot await an async lock, and every caller here is either a
/// gateway handler doing one indexed lookup or a boot-time migration.
#[derive(Debug, Clone)]
pub struct ProjectStore {
    conn: Arc<Mutex<Connection>>,
}

impl ProjectStore {
    /// Wrap an already-open connection. Used by [`Self::shared`] and by tests.
    #[must_use]
    pub fn new(conn: Connection) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    /// The process-wide handle.
    ///
    /// Every consumer shares one connection. The four historical call sites
    /// that each did an ad-hoc `ProjectStore::new()` would, under SQLite, each
    /// open their own connection and race for the write lock.
    ///
    /// Under `cfg(test)` this is an in-memory database with the schema created
    /// but **no** migration run: `migrate()` ends in `roster::publish`, which
    /// REPLACES the process-global snapshot and would erase a concurrently
    /// running roster test's projection. It also keeps `cargo test` from
    /// materialising a real `~/.aleph/data/projects.db` on the developer's
    /// machine.
    #[must_use]
    pub fn shared() -> Arc<Self> {
        static SHARED: OnceLock<Arc<ProjectStore>> = OnceLock::new();
        Arc::clone(SHARED.get_or_init(|| {
            #[cfg(test)]
            {
                let conn = Connection::open_in_memory().expect("in-memory sqlite");
                let store = ProjectStore::new(conn);
                let _ = store.create_schema();
                Arc::new(store)
            }
            #[cfg(not(test))]
            {
                let conn = crate::utils::paths::get_data_dir()
                    .map_err(|e| e.to_string())
                    .and_then(|dir| {
                        crate::utils::sqlite_open::open_sqlite_safe(&dir.join("projects.db"))
                            .map_err(|e| e.to_string())
                    })
                    .unwrap_or_else(|e| {
                        tracing::warn!(
                            error = %e,
                            "projects: falling back to an in-memory catalogue; project rooms will \
                             not persist across restarts"
                        );
                        Connection::open_in_memory().expect("in-memory sqlite")
                    });
                Arc::new(ProjectStore::new(conn))
            }
        }))
    }

    fn with_conn<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, ProjectError>,
    ) -> Result<T, ProjectError> {
        let guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        f(&guard)
    }

    /// Create tables and indexes. Idempotent.
    pub fn create_schema(&self) -> Result<(), ProjectError> {
        self.with_conn(|conn| {
            conn.execute_batch(SCHEMA).map_err(db_err)?;
            conn.execute_batch(&workspace_uniqueness_ddl())
                .map_err(db_err)?;
            conn.execute_batch(BINDING_DDL).map_err(db_err)?;
            add_current_session_key_column(conn)?;
            // AFTER the column migration, never inside `SCHEMA`. On a database
            // created before rooms existed, `SCHEMA`'s `CREATE TABLE IF NOT
            // EXISTS` is a no-op and the column is added by the line above — an
            // index over it placed in `SCHEMA` would run first, fail with "no
            // such column", and take the whole catalogue down at boot on
            // exactly the deployments that predate rooms. An isolated test HOME
            // only ever has the new shape, so nothing here would have caught it.
            //
            // Partial (`WHERE ... IS NOT NULL`) because the overwhelming
            // majority of rows have no claim, and `project_for_session_key`
            // only ever probes non-NULL keys.
            conn.execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_projects_session_key
                     ON projects(current_session_key)
                     WHERE current_session_key IS NOT NULL;",
            )
            .map_err(db_err)
        })
    }

    /// Full boot-time migration: schema, one-time `projects.json` adoption,
    /// then publish the roster projection.
    pub fn migrate(&self) -> Result<(), ProjectError> {
        self.create_schema()?;
        if let Ok(dir) = crate::utils::paths::get_config_dir() {
            self.migrate_from_json(&dir.join("projects.json"))?;
        }
        self.republish_roster()
    }

    /// Adopt a pre-P2 `~/.aleph/projects.json` catalogue.
    ///
    /// Idempotent by the owner-keyed unique index plus `INSERT OR IGNORE`, NOT
    /// by the rename below — a crash between the insert and the rename is a
    /// real state, and re-running must not duplicate. A failed rename is
    /// therefore not a failed migration.
    ///
    /// Writes `project_members`, so it ends its closure with
    /// [`Self::republish_roster_locked`] like every other roster mutation.
    /// This is `pub`, and until 2026-08-07 it was the one roster-mutating
    /// method whose projection depended on what the caller did NEXT — it
    /// happened to be correct only because [`Self::migrate`] calls
    /// `republish_roster` on the line after. A `pub` mutator whose correctness
    /// lives at its call site is one new caller away from a roster that
    /// silently reports "not a member" for every adopted row, and `is_member`
    /// is the whole room authorization predicate. `migrate`'s own trailing
    /// republish stays: this one is skipped entirely when there is no
    /// `projects.json` to adopt, which is the normal case.
    pub fn migrate_from_json(&self, json_path: &Path) -> Result<(), ProjectError> {
        let bytes = match std::fs::read(json_path) {
            Ok(b) if !b.is_empty() => b,
            Ok(_) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(ProjectError::Io(e)),
        };
        let legacy: LegacyStoreFile = serde_json::from_slice(&bytes)?;

        self.with_conn(|conn| {
            for entry in &legacy.projects {
                let id = mint_id();
                let changed = conn
                    .execute(
                        "INSERT OR IGNORE INTO projects
                            (id, name, owner_user_id, workspace_path, status,
                             created_at, updated_at, last_used_at)
                         VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?5, ?6)",
                        rusqlite::params![
                            id,
                            entry.name,
                            OWNER_USER_ID,
                            entry.path.to_string_lossy(),
                            entry.created_at,
                            entry.last_used_at,
                        ],
                    )
                    .map_err(db_err)?;
                if changed == 1 {
                    conn.execute(
                        "INSERT OR IGNORE INTO project_members (project_id, user_id, added_at)
                         VALUES (?1, ?2, ?3)",
                        rusqlite::params![id, OWNER_USER_ID, entry.created_at],
                    )
                    .map_err(db_err)?;
                }
            }
            Self::republish_roster_locked(conn)
        })?;

        // Best-effort marker so a healthy install stops re-reading the file.
        let _ = std::fs::rename(json_path, json_path.with_extension("json.migrated"));
        Ok(())
    }

    /// Re-publish the whole roster projection from `project_members`, from
    /// INSIDE the caller's connection lock.
    ///
    /// Every roster mutation calls this in the same [`Self::with_conn`] closure
    /// that performed the write, so the snapshot read and the publish cannot be
    /// interleaved by a second mutation. Taking the lock twice — write, release,
    /// re-take, read, publish — let two concurrent writers publish in the
    /// opposite order to the one they committed in, and the loser's stale
    /// snapshot resurrected a just-removed member until the next roster write.
    /// `is_member` is the whole room authorization predicate, so that direction
    /// of failure is fail-open.
    fn republish_roster_locked(conn: &Connection) -> Result<(), ProjectError> {
        let mut stmt = conn
            .prepare("SELECT project_id, user_id FROM project_members")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(db_err)?;
        let mut pairs = Vec::new();
        for row in rows {
            pairs.push(row.map_err(db_err)?);
        }
        roster::publish(RosterSnapshot::from_pairs(pairs));
        Ok(())
    }

    /// [`Self::republish_roster_locked`] for callers that hold no lock yet —
    /// boot migration only. A mutation must never use this: see that function's
    /// doc for the interleaving it exists to prevent.
    fn republish_roster(&self) -> Result<(), ProjectError> {
        self.with_conn(Self::republish_roster_locked)
    }

    // -- entity ------------------------------------------------------------

    /// Create a room. The owner is added to the roster in the same call —
    /// a project whose owner is not a member would be invisible to its own
    /// creator.
    pub fn create(
        &self,
        name: &str,
        owner: Option<&str>,
        workspace: Option<&Path>,
    ) -> Result<Project, ProjectError> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(ProjectError::InvalidName(name.to_string()));
        }
        let now = now_secs();
        let project = Project {
            id: mint_id(),
            name: trimmed.to_string(),
            owner_user_id: owner.map(str::to_string),
            workspace_path: workspace.map(Path::to_path_buf),
            status: ProjectStatus::Active,
            created_at: now,
            updated_at: now,
            last_used_at: now,
            current_session_key: None,
        };

        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO projects
                    (id, name, owner_user_id, workspace_path, status,
                     created_at, updated_at, last_used_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?6)",
                rusqlite::params![
                    project.id,
                    project.name,
                    project.owner_user_id,
                    project
                        .workspace_path
                        .as_ref()
                        .map(|p| p.to_string_lossy().to_string()),
                    project.status.as_str(),
                    now,
                ],
            )
            .map_err(db_err)?;
            conn.execute(
                "INSERT OR IGNORE INTO project_members (project_id, user_id, added_at)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    project.id,
                    project.owner_user_id.as_deref().unwrap_or(OWNER_USER_ID),
                    now
                ],
            )
            .map_err(db_err)?;
            Self::republish_roster_locked(conn)
        })?;
        Ok(project)
    }

    pub fn get(&self, id: &str) -> Result<Option<Project>, ProjectError> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT id, name, owner_user_id, workspace_path, status,
                        created_at, updated_at, last_used_at, current_session_key
                 FROM projects WHERE id = ?1",
                [id],
                row_to_project,
            )
            .optional()
            .map_err(db_err)
        })
    }

    /// Every project, newest activity first. **Unfiltered** — visibility
    /// filtering belongs to the gateway handler, which is the layer that knows
    /// who is asking.
    pub fn list(&self) -> Result<Vec<Project>, ProjectError> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, name, owner_user_id, workspace_path, status,
                            created_at, updated_at, last_used_at, current_session_key
                     FROM projects ORDER BY last_used_at DESC",
                )
                .map_err(db_err)?;
            let rows = stmt.query_map([], row_to_project).map_err(db_err)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(db_err)?);
            }
            Ok(out)
        })
    }

    pub fn rename(&self, id: &str, name: &str) -> Result<Project, ProjectError> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(ProjectError::InvalidName(name.to_string()));
        }
        self.update_one(
            "UPDATE projects SET name = ?2, updated_at = ?3 WHERE id = ?1",
            rusqlite::params![id, trimmed, now_secs()],
            id,
        )?;
        self.require(id)
    }

    pub fn archive(&self, id: &str) -> Result<(), ProjectError> {
        self.update_one(
            "UPDATE projects SET status = 'archived', updated_at = ?2 WHERE id = ?1",
            rusqlite::params![id, now_secs()],
            id,
        )
    }

    /// Bump `last_used_at`. Unlike the pre-P2 store this is NOT a silent no-op
    /// for an unknown id — a caller touching a project that does not exist is
    /// a bug worth surfacing, and the handler above maps it to `not found`.
    pub fn touch(&self, id: &str) -> Result<(), ProjectError> {
        self.update_one(
            "UPDATE projects SET last_used_at = ?2 WHERE id = ?1",
            rusqlite::params![id, now_secs()],
            id,
        )
    }

    /// Forget a project. The on-disk folder is left untouched — removal means
    /// "forget about this project", never "delete files".
    ///
    /// # Refuses to forget a ROOM that has a conversation
    ///
    /// This verb predates P2 and its semantics are the directory catalogue's:
    /// drop a folder from the recents list. A project ROOM is a different
    /// object wearing the same row — it has a roster, a `p-*` memory
    /// partition, artifacts, and a claimed session — and for a room the roster
    /// is not merely metadata, it is the ENTIRE visibility predicate
    /// (`visibility::owner_and_scope_visible_to` asks
    /// `roster::is_member(project, actor)` and nothing else).
    ///
    /// So deleting the row and its `project_members` in one write does not
    /// "forget" the room: it makes the room's transcript unreachable by every
    /// principal alive, including the operator and the person who created it.
    /// Not deleted — permanently invisible, with the rows still on disk and no
    /// predicate that can ever return true for them again. The `main__p-<id>`
    /// partition keeps being enumerated and maintained nightly by
    /// `list_scoped_agent_ids`, which reads the filesystem and has no idea the
    /// room is gone.
    ///
    /// [`Self::archive`] is the verb that means "forget" for a room: it flips
    /// `status` and keeps the roster, so members stop seeing it in the picker
    /// and the conversation stays reachable. A room is therefore refused here
    /// and pointed at it.
    ///
    /// A catalogue entry — no claimed session — removes exactly as it always
    /// did, which is every pre-P2 caller.
    pub fn remove(&self, id: &str) -> Result<(), ProjectError> {
        self.with_conn(|conn| {
            let claimed: Option<Option<String>> = conn
                .query_row(
                    "SELECT current_session_key FROM projects WHERE id = ?1",
                    [id],
                    |r| r.get::<_, Option<String>>(0),
                )
                .optional()
                .map_err(db_err)?;
            match claimed {
                None => return Err(ProjectError::NotFound(id.to_string())),
                Some(Some(_)) => {
                    return Err(ProjectError::Invalid(format!(
                        "project '{id}' is a room with a conversation: removing it would leave \
                         that conversation unreachable to everyone, including you, because a \
                         room's visibility IS its roster. Archive it instead."
                    )));
                }
                Some(None) => {}
            }

            let changed = conn
                .execute("DELETE FROM projects WHERE id = ?1", [id])
                .map_err(db_err)?;
            if changed == 0 {
                return Err(ProjectError::NotFound(id.to_string()));
            }
            conn.execute("DELETE FROM project_members WHERE project_id = ?1", [id])
                .map_err(db_err)?;
            Self::republish_roster_locked(conn)
        })
    }

    pub fn bind_workspace(&self, id: &str, path: Option<&Path>) -> Result<Project, ProjectError> {
        let canonical = match path {
            Some(p) => Some(canonical_dir(p)?),
            None => None,
        };
        self.update_one(
            "UPDATE projects SET workspace_path = ?2, updated_at = ?3 WHERE id = ?1",
            rusqlite::params![
                id,
                canonical.as_ref().map(|p| p.to_string_lossy().to_string()),
                now_secs()
            ],
            id,
        )?;
        self.require(id)
    }

    // -- roster ------------------------------------------------------------

    pub fn add_member(&self, id: &str, user_id: &str) -> Result<(), ProjectError> {
        self.with_conn(|conn| {
            let exists: bool = conn
                .prepare("SELECT 1 FROM projects WHERE id = ?1")
                .and_then(|mut stmt| stmt.exists([id]))
                .map_err(db_err)?;
            if !exists {
                return Err(ProjectError::NotFound(id.to_string()));
            }
            conn.execute(
                "INSERT OR IGNORE INTO project_members (project_id, user_id, added_at)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![id, user_id, now_secs()],
            )
            .map_err(db_err)?;
            Self::republish_roster_locked(conn)
        })
    }

    pub fn remove_member(&self, id: &str, user_id: &str) -> Result<(), ProjectError> {
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM project_members WHERE project_id = ?1 AND user_id = ?2",
                rusqlite::params![id, user_id],
            )
            .map_err(db_err)?;
            Self::republish_roster_locked(conn)
        })
    }

    // -- canonical room session --------------------------------------------

    /// Claim `candidate` as this room's canonical chat session, or return the
    /// key a member already claimed.
    ///
    /// Get-or-create, atomic: the conditional `UPDATE … WHERE
    /// current_session_key IS NULL` and the read-back run in one
    /// [`Self::with_conn`] closure, so two members opening the room at the same
    /// moment converge — the loser adopts the winner's key instead of forking a
    /// second conversation. Returns the effective key, which is `candidate`
    /// only when this call is the one that claimed it.
    pub fn claim_session_key(&self, id: &str, candidate: &str) -> Result<String, ProjectError> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE projects SET current_session_key = ?2
                 WHERE id = ?1 AND current_session_key IS NULL",
                rusqlite::params![id, candidate],
            )
            .map_err(db_err)?;
            let claimed: Option<Option<String>> = conn
                .query_row(
                    "SELECT current_session_key FROM projects WHERE id = ?1",
                    [id],
                    |r| r.get::<_, Option<String>>(0),
                )
                .optional()
                .map_err(db_err)?;
            match claimed {
                // The row exists and the UPDATE above cannot have left it NULL,
                // so an inner `None` means a concurrent `remove` won the race —
                // report it as the missing project it now is.
                Some(Some(key)) => Ok(key),
                _ => Err(ProjectError::NotFound(id.to_string())),
            }
        })
    }

    /// The project that claimed `session_key` as its room conversation, if any.
    ///
    /// The reverse of [`Self::claim_session_key`], and the reason it can exist
    /// at all: that method is the SOLE writer of `current_session_key`, so this
    /// column is a declaration by the room rather than an inference about it.
    ///
    /// It answers a question no other source can. The room's session ROW does
    /// not yet exist when the room is opened — `projects.room_session` claims
    /// the key, and whoever speaks first creates the row — so between those two
    /// writes there is nothing that says the key belongs to a room. A bare
    /// `chat.send` in that window (no `project_id`; the Panel always sends one,
    /// a plain RPC client need not) stamps the row `personal:<first speaker>`,
    /// permanently: `stamp_attribution` is create-only by design, and
    /// `attribution_backfill` cannot heal it because its predicate is
    /// `owner_user_id IS NULL AND scope_id IS NULL` and the row is stamped, not
    /// blank. The room then vanishes for every other member INCLUDING its owner
    /// while `projects.list` keeps listing it.
    ///
    /// `Ok(None)` for "no room claims this key" — the overwhelmingly common
    /// case, and the one that must stay a cheap indexed lookup rather than a
    /// scan.
    ///
    /// `pub(in crate::projects)` because this is **arm 1's raw ingredient**,
    /// and the composition it feeds ([`Self::room_claiming`]) must keep exactly
    /// one author. Anything outside `projects` that could reach both this and
    /// [`Self::project_for_bound_session`] could re-derive that composition,
    /// and this repo has paid for that shape before — the two readers this task
    /// converged were each re-deriving it. The visibility is what makes a third
    /// derivation a **compile error** rather than a convention the next author
    /// has to have read. Contrast [`Self::project_for_conversation`], which
    /// stays `pub`: it is an independent query, not an ingredient of the
    /// precedence rule, so narrowing it would block legitimate cross-module
    /// callers without preventing any re-derivation.
    pub(in crate::projects) fn project_for_session_key(
        &self,
        session_key: &str,
    ) -> Result<Option<String>, ProjectError> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT id FROM projects WHERE current_session_key = ?1",
                [session_key],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(db_err)
        })
    }

    /// Bind a conversation to a room.
    ///
    /// The conversation side is the primary key, so a conversation another room
    /// already holds is refused with [`ProjectError::Invalid`] rather than
    /// silently taken over — an overwrite would move a live room's traffic
    /// somewhere its members cannot see.
    ///
    /// Re-binding a conversation to the SAME room is a no-op that succeeds and
    /// refreshes the label: an operator repeating a bind must not be told they
    /// broke something.
    ///
    /// `channel_id` and `peer_id` are normalized via
    /// [`binding::normalize_component`] before storage (Ruling AD) — the same
    /// normalization a live `SessionKey` applies — so the operator's spelling
    /// need not match a channel adapter's exactly. Pass the original spelling
    /// as `label` if it is worth preserving for display; it is not otherwise
    /// recoverable from the stored row. The already-bound conflict error below
    /// names the operator's original input, not the normalized key, so it
    /// reads back the words the operator actually typed.
    pub fn bind_conversation(
        &self,
        project_id: &str,
        channel_id: &str,
        peer_kind: BindingPeerKind,
        peer_id: &str,
        bound_by: Option<&str>,
        label: Option<&str>,
    ) -> Result<ChannelBinding, ProjectError> {
        let now = now_secs();
        let peer_kind_col = peer_kind.as_str();
        // Normalized forms, bound to their own names rather than shadowing
        // `channel_id`/`peer_id`: the conflict error below must be able to
        // quote the operator's own spelling back at them, not the key we
        // actually store.
        let channel_key = binding::normalize_component(channel_id);
        let peer_key = binding::normalize_component(peer_id);
        self.with_conn(|conn| {
            let existing: Option<String> = conn
                .query_row(
                    "SELECT project_id FROM project_channel_bindings
                     WHERE channel_id = ?1 AND peer_kind = ?2 AND peer_id = ?3",
                    rusqlite::params![channel_key, peer_kind_col, peer_key],
                    |r| r.get::<_, String>(0),
                )
                .optional()
                .map_err(db_err)?;
            if let Some(owner) = existing.as_deref() {
                if owner != project_id {
                    return Err(ProjectError::Invalid(format!(
                        "{channel_id}:{peer_id} is already bound to project {owner}; \
                         unbind it there first"
                    )));
                }
            }
            let exists: Option<String> = conn
                .query_row(
                    "SELECT id FROM projects WHERE id = ?1 AND status = 'active'",
                    [project_id],
                    |r| r.get::<_, String>(0),
                )
                .optional()
                .map_err(db_err)?;
            if exists.is_none() {
                return Err(ProjectError::NotFound(project_id.to_string()));
            }
            conn.execute(
                "INSERT INTO project_channel_bindings
                     (project_id, channel_id, peer_kind, peer_id, bound_by, bound_at, label)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(channel_id, peer_kind, peer_id) DO UPDATE SET
                     label = excluded.label,
                     bound_by = excluded.bound_by,
                     bound_at = excluded.bound_at",
                rusqlite::params![
                    project_id,
                    channel_key,
                    peer_kind_col,
                    peer_key,
                    bound_by,
                    now,
                    label
                ],
            )
            .map_err(db_err)?;
            Ok(ChannelBinding {
                project_id: project_id.to_string(),
                channel_id: channel_key.clone(),
                peer_kind,
                peer_id: peer_key.clone(),
                bound_by: bound_by.map(str::to_string),
                bound_at: now,
                label: label.map(str::to_string),
            })
        })
    }

    /// Release a conversation. `Ok(false)` means nothing was bound — a distinct
    /// answer from `Ok(true)`, because a receipt that says "unbound" about a
    /// conversation that never was is a client asserting a result it did not
    /// observe.
    ///
    /// `channel_id` and `peer_id` are normalized the same way
    /// [`Self::bind_conversation`] normalizes them before storing, so an
    /// operator's original spelling still resolves to the stored row.
    pub fn unbind_conversation(
        &self,
        channel_id: &str,
        peer_kind: BindingPeerKind,
        peer_id: &str,
    ) -> Result<bool, ProjectError> {
        let peer_kind_col = peer_kind.as_str();
        let channel_id = binding::normalize_component(channel_id);
        let peer_id = binding::normalize_component(peer_id);
        self.with_conn(|conn| {
            let n = conn
                .execute(
                    "DELETE FROM project_channel_bindings
                     WHERE channel_id = ?1 AND peer_kind = ?2 AND peer_id = ?3",
                    rusqlite::params![channel_id, peer_kind_col, peer_id],
                )
                .map_err(db_err)?;
            Ok(n > 0)
        })
    }

    /// The room a conversation belongs to, if any.
    ///
    /// Sibling of [`Self::project_for_session_key`]: both answer "which room
    /// owns this turn" and both must stay a cheap indexed lookup, because
    /// [`Self::room_claiming`] calls them on every run.
    ///
    /// `channel_id` and `peer_id` are normalized the same way
    /// [`Self::bind_conversation`] normalizes them before storing. A caller
    /// passing the components straight out of [`binding::conversation_of`]
    /// (already normalized, since they came from a live `SessionKey`) pays
    /// nothing extra — normalization is idempotent.
    pub fn project_for_conversation(
        &self,
        channel_id: &str,
        peer_kind: BindingPeerKind,
        peer_id: &str,
    ) -> Result<Option<String>, ProjectError> {
        let peer_kind_col = peer_kind.as_str();
        let channel_id = binding::normalize_component(channel_id);
        let peer_id = binding::normalize_component(peer_id);
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT project_id FROM project_channel_bindings
                 WHERE channel_id = ?1 AND peer_kind = ?2 AND peer_id = ?3",
                rusqlite::params![channel_id, peer_kind_col, peer_id],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(db_err)
        })
    }

    /// The room a session key's conversation is bound to, if any.
    ///
    /// Composes [`binding::conversation_of`] (decomposing the key into the
    /// `(channel, peer_kind, peer_id)` triple a binding is keyed on) with
    /// [`Self::project_for_conversation`] (the stored lookup). `Ok(None)` for
    /// a key that is not a conversation at all (a DM, a task, a main session)
    /// as well as for a conversation nothing is bound to — both mean the same
    /// thing to a caller asking "does a room claim this key".
    ///
    /// This is arm 2 of [`Self::room_claiming`], which is now its only caller
    /// — it re-composes neither this nor `conversation_of` +
    /// `project_for_conversation`, the same rule arm 1 already follows by
    /// calling [`Self::project_for_session_key`] directly instead of
    /// duplicating its query. It stays a method of its own rather than being
    /// inlined there because the decomposition it performs is a property of
    /// the *key*, not of the claim precedence above it.
    ///
    /// `pub(in crate::projects)` for the same reason as
    /// [`Self::project_for_session_key`] — see that method's doc. These two are
    /// arm 1's and arm 2's ingredients; holding both is what would let someone
    /// rebuild [`Self::room_claiming`]'s precedence somewhere else.
    pub(in crate::projects) fn project_for_bound_session(
        &self,
        session_key: &crate::routing::session_key::SessionKey,
    ) -> Result<Option<String>, ProjectError> {
        let Some((channel_id, peer_kind, peer_id)) = binding::conversation_of(session_key) else {
            return Ok(None);
        };
        self.project_for_conversation(&channel_id, peer_kind, &peer_id)
    }

    /// The project that claims `session_key` as its room conversation, by
    /// either of the two ways a room can claim one — and **which** of the two
    /// answered.
    ///
    /// The single composition of [`Self::project_for_session_key`] (arm 1,
    /// [`ClaimSource::ExplicitClaim`]) and [`Self::project_for_bound_session`]
    /// (arm 2, [`ClaimSource::BoundConversation`]). It lives on `ProjectStore`
    /// for the same reason arm 2's own composition does — see that method's
    /// doc: a composition of catalogue lookups belongs on the catalogue, not
    /// re-assembled at each caller. `projects::binding` is the other plausible
    /// home and is deliberately not used: that module is store-free by
    /// construction (`store.rs` depends on it, never the reverse), and putting
    /// this there would invert that dependency to buy nothing.
    ///
    /// **Precedence:** an explicit claim outranks a binding. The two
    /// disagreeing means an operator bound a conversation some room had
    /// already claimed by key; the claim is the declaration, so it wins — and
    /// the mismatch is said out loud rather than silently resolved.
    ///
    /// **Returns `Option`, not `Result`** — the only lookup on this type that
    /// swallows its own error, and the reason it can is that both callers had
    /// already ruled the same way on a degraded catalogue: a SQLite hiccup
    /// must turn into neither a mis-scoped turn nor a refused one. The cost is
    /// bounded (the row is then stamped the way it was stamped before rooms
    /// existed); the alternative makes a transient database fault look like a
    /// permissions failure. A ruling both callers share is exactly the thing
    /// that must not be written twice.
    ///
    /// What the callers do **not** share is what a `None` *means* to them —
    /// "leave the producer's stamp alone" after admission, "fall through to
    /// the personal arm" on it — nor what an invisible project means, which
    /// differs per arm on one side and not the other. Those are policy and
    /// stay at the two call sites; only the lookup is here.
    pub(crate) fn room_claiming(
        &self,
        session_key: &crate::routing::session_key::SessionKey,
    ) -> Option<(String, ClaimSource)> {
        // (1) The Panel-minted room conversation, claimed by `projects.room_session`.
        let claimed = match self.project_for_session_key(&session_key.to_key_string()) {
            Ok(pid) => pid,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "projects: room claim lookup failed; treating the key as not-a-room"
                );
                None
            }
        };
        // (2) A channel conversation bound to a room. Keyed on the
        // conversation, so an `agent_switch` (which changes the session key's
        // agent component) does not un-bind it.
        let bound = match self.project_for_bound_session(session_key) {
            Ok(pid) => pid,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "projects: conversation binding lookup failed; treating the key as not-a-room"
                );
                None
            }
        };
        match (claimed, bound) {
            (Some(a), Some(b)) if a != b => {
                tracing::warn!(
                    claimed = %a,
                    bound = %b,
                    "projects: a session key is claimed by one room and its conversation is bound to another; \
                     taking the explicit claim"
                );
                Some((a, ClaimSource::ExplicitClaim))
            }
            (Some(a), _) => Some((a, ClaimSource::ExplicitClaim)),
            (None, Some(b)) => Some((b, ClaimSource::BoundConversation)),
            (None, None) => None,
        }
    }

    /// Every conversation a room is bound to, oldest first.
    ///
    /// A row whose stored `peer_kind` does not parse back as a
    /// [`BindingPeerKind`] is skipped with a `warn!` naming the row's primary
    /// key, rather than silently dropped: every writer of that column goes
    /// through [`BindingPeerKind::as_str`], so a value its `FromStr` cannot
    /// take means the row came from elsewhere (or was corrupted), and a
    /// bindings table that silently omits a row is indistinguishable from an
    /// empty one on the `list` surface.
    ///
    /// `channel_id`/`peer_id` on the returned rows are read back exactly as
    /// [`Self::bind_conversation`] stored them — already normalized (Ruling
    /// AD). A caller that wants the operator's original spelling reads
    /// [`ChannelBinding::label`], not these two fields.
    pub fn bindings_for(&self, project_id: &str) -> Result<Vec<ChannelBinding>, ProjectError> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT channel_id, peer_kind, peer_id, bound_by, bound_at, label
                     FROM project_channel_bindings WHERE project_id = ?1
                     ORDER BY bound_at, channel_id, peer_id",
                )
                .map_err(db_err)?;
            let rows = stmt
                .query_map([project_id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, Option<String>>(5)?,
                    ))
                })
                .map_err(db_err)?;
            let mut out = Vec::new();
            for row in rows {
                let (channel_id, peer_kind_raw, peer_id, bound_by, bound_at, label) =
                    row.map_err(db_err)?;
                let Ok(peer_kind) = peer_kind_raw.parse::<BindingPeerKind>() else {
                    tracing::warn!(
                        project_id = %project_id,
                        channel_id = %channel_id,
                        peer_id = %peer_id,
                        raw = %peer_kind_raw,
                        "projects: unrecognised peer_kind in project_channel_bindings row; skipping"
                    );
                    continue;
                };
                out.push(ChannelBinding {
                    project_id: project_id.to_string(),
                    channel_id,
                    peer_kind,
                    peer_id,
                    bound_by,
                    bound_at,
                    label,
                });
            }
            Ok(out)
        })
    }

    pub fn members(&self, id: &str) -> Result<Vec<String>, ProjectError> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT user_id FROM project_members WHERE project_id = ?1 ORDER BY added_at",
                )
                .map_err(db_err)?;
            let rows = stmt
                .query_map([id], |r| r.get::<_, String>(0))
                .map_err(db_err)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(db_err)?);
            }
            Ok(out)
        })
    }

    /// Every project's roster in ONE query, keyed by project id.
    ///
    /// Exists so a list endpoint that renders rosters does not run
    /// [`Self::members`] once per row. Projects with an empty roster are
    /// absent from the map rather than present-and-empty — a caller rendering
    /// a list should treat "missing" as "no members", which is what the table
    /// actually says.
    pub fn rosters(&self) -> Result<HashMap<String, Vec<String>>, ProjectError> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT project_id, user_id FROM project_members ORDER BY added_at")
                .map_err(db_err)?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map_err(db_err)?;
            let mut out: HashMap<String, Vec<String>> = HashMap::new();
            for row in rows {
                let (project_id, user_id) = row.map_err(db_err)?;
                out.entry(project_id).or_default().push(user_id);
            }
            Ok(out)
        })
    }

    // -- recent-directory view (the picker's entry points) ------------------

    /// Register (or refresh) the project bound to `path` for the current
    /// caller. This is the recent-directory picker's write path: a second
    /// registration of the same folder by the same owner collapses onto the
    /// existing row rather than creating a duplicate.
    pub fn add(&self, path: &Path, name: Option<String>) -> Result<Project, ProjectError> {
        self.add_for(path, name, crate::scope::ambient_owner().as_deref())
    }

    /// [`Self::add`] with the owner passed explicitly.
    ///
    /// Required by any caller that has crossed a `spawn`/`spawn_blocking`
    /// boundary: both ambient attribution mechanisms are task-locals and are
    /// DEAD on the far side, so the implicit lookup would silently resolve to
    /// the legacy owner and file a member's folder into the owner's recents.
    /// Capture the owner before the boundary and hand it in here.
    pub fn add_for(
        &self,
        path: &Path,
        name: Option<String>,
        owner: Option<&str>,
    ) -> Result<Project, ProjectError> {
        let absolute = canonical_dir(path)?;
        if let Some(existing) = self.find_by_path_for(&absolute, owner)? {
            let renamed = name
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(str::to_string);
            self.with_conn(|conn| {
                match &renamed {
                    Some(n) => conn.execute(
                        "UPDATE projects SET last_used_at = ?2, updated_at = ?2, name = ?3
                         WHERE id = ?1",
                        rusqlite::params![existing.id, now_secs(), n],
                    ),
                    None => conn.execute(
                        "UPDATE projects SET last_used_at = ?2 WHERE id = ?1",
                        rusqlite::params![existing.id, now_secs()],
                    ),
                }
                .map_err(db_err)?;
                Ok(())
            })?;
            return self.require(&existing.id);
        }
        let display = resolve_display_name(&absolute, name)?;
        self.create(&display, owner, Some(&absolute))
    }

    /// Materialise a fresh empty directory at `<parent>/<name>` then register
    /// it. Fails if the directory already exists — callers must route the user
    /// through [`Self::add`] for existing folders.
    pub fn create_blank(&self, parent: &Path, name: &str) -> Result<Project, ProjectError> {
        let trimmed = name.trim();
        // Reject both separators on every platform: a name is a single path
        // component, and `MAIN_SEPARATOR` alone would miss '/' on Windows
        // (where it is '\\'), letting "nested/bad" through as a traversal risk.
        if trimmed.is_empty() || trimmed.contains('/') || trimmed.contains('\\') {
            return Err(ProjectError::InvalidName(name.to_string()));
        }
        let parent_abs = canonical_dir(parent)?;
        let target = parent_abs.join(trimmed);
        if target.exists() {
            return Err(ProjectError::AlreadyExists(target));
        }
        std::fs::create_dir_all(&target)?;
        self.add(&target, Some(trimmed.to_string()))
    }

    /// Look up the CURRENT CALLER's project bound to `path`.
    ///
    /// Scoped to the caller on purpose: path binding is unique per owner, not
    /// globally (see [`workspace_uniqueness_ddl`]), so a global lookup here
    /// would hand one user another user's room.
    pub fn find_by_path(&self, path: &Path) -> Result<Option<Project>, ProjectError> {
        self.find_by_path_for(path, crate::scope::ambient_owner().as_deref())
    }

    /// [`Self::find_by_path`] with the owner passed explicitly — see
    /// [`Self::add_for`] for why a spawned caller must use this.
    pub fn find_by_path_for(
        &self,
        path: &Path,
        owner: Option<&str>,
    ) -> Result<Option<Project>, ProjectError> {
        let canonical = canonical_dir(path)?;
        let owner_key = owner.unwrap_or(OWNER_USER_ID);
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT id, name, owner_user_id, workspace_path, status,
                        created_at, updated_at, last_used_at, current_session_key
                 FROM projects
                 WHERE workspace_path = ?1
                   AND COALESCE(owner_user_id, ?2) = ?2
                   AND status = 'active'",
                rusqlite::params![canonical.to_string_lossy(), owner_key],
                row_to_project,
            )
            .optional()
            .map_err(db_err)
        })
    }

    // -- helpers -----------------------------------------------------------

    fn require(&self, id: &str) -> Result<Project, ProjectError> {
        self.get(id)?
            .ok_or_else(|| ProjectError::NotFound(id.to_string()))
    }

    fn update_one(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
        id: &str,
    ) -> Result<(), ProjectError> {
        let changed = self.with_conn(|conn| conn.execute(sql, params).map_err(db_err))?;
        if changed == 0 {
            return Err(ProjectError::NotFound(id.to_string()));
        }
        Ok(())
    }
}

fn row_to_project(row: &Row<'_>) -> rusqlite::Result<Project> {
    let status: String = row.get(4)?;
    let workspace: Option<String> = row.get(3)?;
    Ok(Project {
        id: row.get(0)?,
        name: row.get(1)?,
        owner_user_id: row.get(2)?,
        workspace_path: workspace.map(PathBuf::from),
        status: ProjectStatus::from_stored(&status),
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        last_used_at: row.get(7)?,
        current_session_key: row.get(8)?,
    })
}

/// Add `projects.current_session_key` to a database created before rooms had a
/// canonical session.
///
/// Idempotent — called on every open. SQLite has no `ALTER TABLE … ADD COLUMN
/// IF NOT EXISTS`, so the `pragma_table_info` probe guards it by hand, the same
/// way `gateway::pairing_store` adds `approved_senders.user_id`. No backfill:
/// `NULL` is the correct value for every existing room (nobody has opened its
/// chat yet), and it is exactly what [`ProjectStore::claim_session_key`] treats
/// as unclaimed.
fn add_current_session_key_column(conn: &Connection) -> Result<(), ProjectError> {
    let present: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('projects') WHERE name = 'current_session_key'")
        .and_then(|mut stmt| stmt.exists([]))
        .map_err(db_err)?;
    if !present {
        conn.execute(
            "ALTER TABLE projects ADD COLUMN current_session_key TEXT",
            [],
        )
        .map_err(db_err)?;
    }
    Ok(())
}

/// The pre-P2 `projects.json` shape, kept only so [`ProjectStore::migrate_from_json`]
/// can read it once.
#[derive(Debug, Deserialize)]
struct LegacyStoreFile {
    #[serde(default)]
    projects: Vec<LegacyProject>,
}

#[derive(Debug, Deserialize)]
struct LegacyProject {
    name: String,
    path: PathBuf,
    #[serde(default)]
    created_at: i64,
    #[serde(default)]
    last_used_at: i64,
}

fn canonical_dir(path: &Path) -> Result<PathBuf, ProjectError> {
    if !path.is_absolute() {
        return Err(ProjectError::NotAbsolute(path.to_path_buf()));
    }
    if !path.is_dir() {
        return Err(ProjectError::NotDirectory(path.to_path_buf()));
    }
    let canonical = std::fs::canonicalize(path)?;
    Ok(canonical)
}

fn resolve_display_name(
    path: &Path,
    override_name: Option<String>,
) -> Result<String, ProjectError> {
    if let Some(name) = override_name {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    let basename = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project");
    Ok(basename.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Serialising guard for every test that publishes a roster — see
    /// [`roster::TEST_GUARD`] for why it exists and why it lives there rather
    /// than here.
    use crate::projects::roster::TEST_GUARD as ROSTER_TEST_GUARD;

    fn fresh_store() -> ProjectStore {
        let store = ProjectStore::new(Connection::open_in_memory().unwrap());
        store.create_schema().unwrap();
        store
    }

    /// A catalogue created before rooms existed must still open.
    ///
    /// `SCHEMA`'s `CREATE TABLE IF NOT EXISTS projects` is a no-op against such
    /// a database — the table is there, the column is not — so
    /// `current_session_key` arrives only via
    /// [`add_current_session_key_column`]. Anything that *reads* the column
    /// (an index, a trigger, a view) therefore has to be ordered after it, and
    /// putting it in `SCHEMA` instead fails with "no such column" and takes the
    /// whole catalogue down at boot.
    ///
    /// This is the shape an isolated test HOME is structurally blind to: every
    /// fixture in this module builds a *fresh* database, which always has the
    /// new column. The pre-migration row has to be constructed on purpose.
    #[test]
    fn a_pre_rooms_catalogue_still_opens_and_indexes() {
        let conn = Connection::open_in_memory().unwrap();
        // The projects table exactly as it was before rooms: no
        // `current_session_key`.
        conn.execute_batch(
            "CREATE TABLE projects (
                 id                  TEXT PRIMARY KEY,
                 name                TEXT NOT NULL,
                 owner_user_id       TEXT,
                 workspace_path      TEXT,
                 status              TEXT NOT NULL DEFAULT 'active',
                 created_at          INTEGER NOT NULL,
                 updated_at          INTEGER NOT NULL,
                 last_used_at        INTEGER NOT NULL
             );
             INSERT INTO projects (id, name, created_at, updated_at, last_used_at)
             VALUES ('p-old', 'before rooms', 1, 1, 1);",
        )
        .unwrap();

        let store = ProjectStore::new(conn);
        store
            .create_schema()
            .expect("a pre-rooms catalogue must migrate, not fail to open");

        // The column exists and the row survived...
        assert_eq!(
            store.get("p-old").unwrap().map(|p| p.current_session_key),
            Some(None),
            "the pre-existing row must survive the migration unclaimed"
        );
        // ...and the index over it was created, not skipped.
        let indexed: i64 = store
            .with_conn(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'index' AND name = 'idx_projects_session_key'",
                    [],
                    |r| r.get(0),
                )
                .map_err(db_err)
            })
            .unwrap();
        assert_eq!(
            indexed, 1,
            "the claim lookup must be indexed, as its doc says"
        );

        // Idempotent: opening the same database again is not an error.
        store.create_schema().expect("re-open must be a no-op");
    }

    /// The same migration-order criterion, asserted about the table this
    /// round added rather than about the one round 8 added.
    ///
    /// The design spec names this test; it did not exist until 2026-08-30,
    /// and the criterion it guards was satisfied by construction the whole
    /// time — `project_channel_bindings` and its index are both in `SCHEMA`,
    /// and the index reads only that table's own `project_id`. That is
    /// exactly the state a pinning test is for: **the property held, and
    /// nothing would have said so if it stopped holding.** Put that index on
    /// a column some later migration adds and `create_schema` fails with
    /// "no such column" against every database that predates it — which is
    /// every deployed one, and none of the fixtures in this module.
    ///
    /// Distinct from `a_pre_rooms_catalogue_still_opens_and_indexes` two
    /// tests up in what it *names*: that one would red on a mis-ordered
    /// index too, because `create_schema` runs as a whole — but it names
    /// only the `projects` column and that column's index, so a reader
    /// auditing the new table finds no assertion mentioning it. This one
    /// binds a row that predates the table and reads the binding back.
    #[test]
    fn a_pre_rooms_catalogue_still_opens_and_binds() {
        let conn = Connection::open_in_memory().unwrap();
        // Pre-rooms: no `current_session_key`, and no bindings table at all.
        conn.execute_batch(
            "CREATE TABLE projects (
                 id                  TEXT PRIMARY KEY,
                 name                TEXT NOT NULL,
                 owner_user_id       TEXT,
                 workspace_path      TEXT,
                 status              TEXT NOT NULL DEFAULT 'active',
                 created_at          INTEGER NOT NULL,
                 updated_at          INTEGER NOT NULL,
                 last_used_at        INTEGER NOT NULL
             );
             INSERT INTO projects (id, name, created_at, updated_at, last_used_at)
             VALUES ('p-old', 'before rooms', 1, 1, 1);",
        )
        .unwrap();

        let store = ProjectStore::new(conn);
        store
            .create_schema()
            .expect("a pre-rooms catalogue must migrate, not fail to open");

        // The table and its index arrived with the schema, not with a later
        // migration that a pre-rooms database would never have run.
        let objects: i64 = store
            .with_conn(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE name IN ('project_channel_bindings',
                                    'idx_project_channel_bindings_project')",
                    [],
                    |r| r.get(0),
                )
                .map_err(db_err)
            })
            .unwrap();
        assert_eq!(
            objects, 2,
            "the bindings table and its index must both exist after migrating a \
             pre-rooms catalogue"
        );

        // And a row that predates the table can be bound and read back.
        store
            .bind_conversation("p-old", "tg", BindingPeerKind::Group, "C-42", None, None)
            .expect("a pre-rooms room must be bindable");
        assert_eq!(
            store
                .project_for_conversation("tg", BindingPeerKind::Group, "C-42")
                .unwrap()
                .as_deref(),
            Some("p-old"),
            "the binding must be readable through the lookup the router uses"
        );
    }

    #[test]
    fn legacy_json_catalogue_migrates_into_the_table_once() {
        let _g = ROSTER_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempdir().unwrap();
        let json = dir.path().join("projects.json");
        let alpha = dir.path().join("alpha");
        std::fs::create_dir_all(&alpha).unwrap();
        std::fs::write(
            &json,
            format!(
                r#"{{"version":1,"projects":[{{"id":"deadbeefdeadbeef","name":"alpha",
                    "path":{},"created_at":100,"last_used_at":200}}]}}"#,
                serde_json::to_string(&alpha).unwrap()
            ),
        )
        .unwrap();

        let store = fresh_store();
        store.migrate_from_json(&json).unwrap();

        let all = store.list().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "alpha");
        assert_eq!(all[0].created_at, 100, "timestamps are preserved verbatim");
        assert!(
            all[0].id.starts_with("p-"),
            "ids are re-minted into the p- family, got {}",
            all[0].id
        );
        assert_eq!(all[0].owner_user_id.as_deref(), Some(OWNER_USER_ID));
        assert_eq!(
            store.members(&all[0].id).unwrap(),
            vec![OWNER_USER_ID.to_string()]
        );

        // Idempotent: a crash between the insert and the rename marker is a
        // real state, so re-running must not duplicate.
        let json2 = dir.path().join("projects.json");
        std::fs::write(
            &json2,
            format!(
                r#"{{"version":1,"projects":[{{"id":"deadbeefdeadbeef","name":"alpha",
                    "path":{},"created_at":100,"last_used_at":200}}]}}"#,
                serde_json::to_string(&alpha).unwrap()
            ),
        )
        .unwrap();
        store.migrate_from_json(&json2).unwrap();
        assert_eq!(store.list().unwrap().len(), 1);
    }

    /// `migrate_from_json` writes `project_members`, so it must publish the
    /// projection itself — not rely on `migrate()` republishing on the next
    /// line.
    ///
    /// Called STANDALONE here on purpose: that is the shape a second `pub`
    /// caller would have, and it is the shape under which the adopted rows
    /// used to land in SQLite while `roster::is_member` — the whole room
    /// authorization predicate — kept answering `false` for every one of them.
    #[test]
    fn migrate_from_json_publishes_the_roster_it_adopted() {
        let _g = ROSTER_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempdir().unwrap();
        let json = dir.path().join("projects.json");
        let alpha = dir.path().join("alpha");
        std::fs::create_dir_all(&alpha).unwrap();
        std::fs::write(
            &json,
            format!(
                r#"{{"version":1,"projects":[{{"id":"deadbeefdeadbeef","name":"alpha",
                    "path":{},"created_at":100,"last_used_at":200}}]}}"#,
                serde_json::to_string(&alpha).unwrap()
            ),
        )
        .unwrap();

        let store = fresh_store();
        // No `migrate()` — nothing republishes after this call.
        store.migrate_from_json(&json).unwrap();

        let adopted = &store.list().unwrap()[0];
        assert!(
            roster::is_member(&adopted.id, OWNER_USER_ID),
            "the adopted membership reached the projection, not just the table"
        );
        assert!(
            !roster::is_member(&adopted.id, "u-stranger"),
            "publishing the projection must not widen it"
        );
    }

    #[test]
    fn two_users_may_bind_the_same_folder() {
        let _g = ROSTER_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let store = fresh_store();

        let a = store.create("repo", Some("u-alice"), Some(&repo)).unwrap();
        let b = store.create("repo", Some("u-bob"), Some(&repo));
        assert!(
            b.is_ok(),
            "path uniqueness is per-owner, never global (no existence oracle)"
        );
        assert_ne!(a.id, b.unwrap().id);
    }

    #[test]
    fn the_same_owner_rebinding_a_folder_collapses_onto_one_row() {
        let _g = ROSTER_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempdir().unwrap();
        let x = dir.path().join("x");
        std::fs::create_dir_all(&x).unwrap();
        let store = fresh_store();

        let first = store.add(&x, None).unwrap();
        let again = store.add(&x, None).unwrap();
        assert_eq!(
            first.id, again.id,
            "the recent-directory picker must not duplicate"
        );
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[test]
    fn creating_a_project_publishes_its_roster() {
        let _g = ROSTER_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let store = fresh_store();
        let p = store.create("room", Some("u-alice"), None).unwrap();
        assert!(roster::is_member(&p.id, "u-alice"));
        assert!(!roster::is_member(&p.id, "u-bob"));

        store.add_member(&p.id, "u-bob").unwrap();
        assert!(
            roster::is_member(&p.id, "u-bob"),
            "the projection follows the write"
        );

        store.remove_member(&p.id, "u-bob").unwrap();
        assert!(
            !roster::is_member(&p.id, "u-bob"),
            "spec §10: removal revokes visibility immediately"
        );
    }

    #[test]
    fn removing_a_project_takes_its_roster_with_it() {
        let _g = ROSTER_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let store = fresh_store();
        let p = store.create("room", Some("u-alice"), None).unwrap();
        store.remove(&p.id).unwrap();
        assert!(!roster::is_member(&p.id, "u-alice"));
        assert!(store.get(&p.id).unwrap().is_none());
        assert!(matches!(
            store.remove(&p.id).unwrap_err(),
            ProjectError::NotFound(_)
        ));
    }

    /// A room whose conversation exists cannot be "forgotten", because for a
    /// room the roster is the whole visibility predicate: dropping it makes the
    /// transcript unreachable to every principal alive rather than deleting it.
    #[test]
    fn a_room_with_a_conversation_is_refused_and_pointed_at_archive() {
        let _g = ROSTER_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let store = fresh_store();
        let p = store.create("room", Some("u-alice"), None).unwrap();
        store.add_member(&p.id, "u-bob").unwrap();
        store.claim_session_key(&p.id, "agent:main:room:1").unwrap();

        let err = store.remove(&p.id).unwrap_err();
        assert!(
            matches!(err, ProjectError::Invalid(ref m) if m.contains("Archive it instead")),
            "the refusal must name the verb that does work: {err}"
        );

        // Nothing half-applied: the room, its roster and its session pointer
        // are all still there.
        assert!(store.get(&p.id).unwrap().is_some());
        assert!(roster::is_member(&p.id, "u-bob"));
        assert_eq!(
            store
                .claim_session_key(&p.id, "agent:main:room:2")
                .unwrap()
                .as_str(),
            "agent:main:room:1"
        );

        // Archiving is the verb that works, and it keeps the roster — which is
        // what keeps the conversation reachable.
        store.archive(&p.id).unwrap();
        assert!(roster::is_member(&p.id, "u-bob"));
    }

    /// The pre-P2 catalogue entry — a folder in the recents list, no
    /// conversation — is unaffected, which is every caller this verb had
    /// before rooms existed.
    #[test]
    fn a_catalogue_entry_without_a_conversation_still_removes() {
        let _g = ROSTER_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let store = fresh_store();
        let p = store
            .create("just-a-folder", Some("u-alice"), None)
            .unwrap();
        store.remove(&p.id).unwrap();
        assert!(store.get(&p.id).unwrap().is_none());
    }

    #[test]
    fn archiving_keeps_the_row_and_frees_the_path_binding() {
        let _g = ROSTER_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempdir().unwrap();
        let w = dir.path().join("w");
        std::fs::create_dir_all(&w).unwrap();
        let store = fresh_store();

        let p = store.create("room", Some("u-alice"), Some(&w)).unwrap();
        store.archive(&p.id).unwrap();
        assert_eq!(
            store.get(&p.id).unwrap().unwrap().status,
            ProjectStatus::Archived,
            "archiving is not deletion"
        );
        // The partial unique index only covers active rows, so the same owner
        // may bind the folder again in a fresh room.
        assert!(store.create("room2", Some("u-alice"), Some(&w)).is_ok());
    }

    #[test]
    fn list_is_ordered_by_recency_and_never_evicts() {
        let _g = ROSTER_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let store = fresh_store();
        // The pre-P2 store capped the catalogue at 64 and evicted the oldest.
        // Under project rooms that would silently delete a room someone else
        // is working in, so the cap is gone.
        for i in 0..70 {
            store
                .create(&format!("p{i}"), Some("u-alice"), None)
                .unwrap();
        }
        assert_eq!(store.list().unwrap().len(), 70);
    }

    #[test]
    fn add_rejects_relative_and_missing() {
        let _g = ROSTER_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempdir().unwrap();
        let store = fresh_store();
        let relative = PathBuf::from("./not-absolute");
        assert!(matches!(
            store.add(&relative, None).unwrap_err(),
            ProjectError::NotAbsolute(_)
        ));
        let missing = dir.path().join("ghost");
        assert!(matches!(
            store.add(&missing, None).unwrap_err(),
            ProjectError::NotDirectory(_)
        ));
    }

    #[test]
    fn create_blank_makes_dir_and_registers() {
        let _g = ROSTER_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempdir().unwrap();
        let store = fresh_store();
        let project = store.create_blank(dir.path(), "new-app").unwrap();
        assert!(project.workspace_path.as_ref().unwrap().is_dir());
        assert_eq!(project.name, "new-app");
    }

    #[test]
    fn create_blank_refuses_existing_dir_and_separators() {
        let _g = ROSTER_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempdir().unwrap();
        let store = fresh_store();
        let existing = dir.path().join("preexisting");
        std::fs::create_dir_all(&existing).unwrap();
        assert!(matches!(
            store.create_blank(dir.path(), "preexisting").unwrap_err(),
            ProjectError::AlreadyExists(_)
        ));
        assert!(matches!(
            store.create_blank(dir.path(), "nested/bad").unwrap_err(),
            ProjectError::InvalidName(_)
        ));
    }

    #[test]
    fn touch_surfaces_an_unknown_id_instead_of_silently_succeeding() {
        let _g = ROSTER_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let store = fresh_store();
        assert!(matches!(
            store.touch("p-nope").unwrap_err(),
            ProjectError::NotFound(_)
        ));
    }

    /// An unreadable status must not read as a live room.
    #[test]
    fn an_unparseable_status_reads_as_archived() {
        assert_eq!(ProjectStatus::from_stored("active"), ProjectStatus::Active);
        assert_eq!(
            ProjectStatus::from_stored("who-knows"),
            ProjectStatus::Archived
        );
    }

    /// A room's canonical session is claimed once and never re-minted: the
    /// second member to open the room adopts the first member's key, which is
    /// what makes a room ONE conversation rather than one per device.
    #[test]
    fn the_first_claim_wins_and_later_callers_adopt_it() {
        let _g = ROSTER_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let store = fresh_store();
        let p = store.create("room", Some("u-alice"), None).unwrap();
        assert_eq!(
            store.get(&p.id).unwrap().unwrap().current_session_key,
            None,
            "a fresh room has no session until someone opens its chat"
        );

        let alice = store
            .claim_session_key(&p.id, "agent:main:room-alice")
            .unwrap();
        assert_eq!(alice, "agent:main:room-alice");

        // Bob's Panel proposes its own candidate (his default agent may differ)
        // and must still land on Alice's conversation.
        let bob = store
            .claim_session_key(&p.id, "agent:coder:room-bob")
            .unwrap();
        assert_eq!(bob, "agent:main:room-alice");
        assert_eq!(
            store
                .get(&p.id)
                .unwrap()
                .unwrap()
                .current_session_key
                .as_deref(),
            Some("agent:main:room-alice"),
            "the claim is durable, not per-call"
        );
    }

    #[test]
    fn claiming_a_session_for_an_unknown_room_is_not_found() {
        let _g = ROSTER_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let store = fresh_store();
        assert!(matches!(
            store
                .claim_session_key("p-nope", "agent:main:x")
                .unwrap_err(),
            ProjectError::NotFound(_)
        ));
    }

    /// Two members opening the same room at the same instant must converge on
    /// one key. The claim is a conditional UPDATE plus a read-back inside a
    /// single connection lock, so the loser reads the winner's value rather
    /// than overwriting it.
    #[test]
    fn concurrent_claims_all_return_the_same_key() {
        let _g = ROSTER_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let store = fresh_store();
        let p = store.create("room", Some("u-alice"), None).unwrap();

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let store = store.clone();
                let id = p.id.clone();
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    store
                        .claim_session_key(&id, &format!("agent:main:room-{i}"))
                        .unwrap()
                })
            })
            .collect();

        let keys: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let persisted = store.get(&p.id).unwrap().unwrap().current_session_key;
        assert_eq!(persisted.as_deref(), Some(keys[0].as_str()));
        assert!(
            keys.iter().all(|k| *k == keys[0]),
            "every caller must see one room session, got {keys:?}"
        );
    }

    /// The projection must never survive its own table: after concurrent
    /// roster churn the published snapshot has to agree with
    /// `project_members`. With the publish outside the mutation's lock, a
    /// preempted writer could publish a snapshot read BEFORE another writer's
    /// delete and resurrect a removed member — fail-open, since `is_member` is
    /// the whole room authorization predicate.
    #[test]
    fn concurrent_roster_writes_never_leave_a_stale_projection() {
        let _g = ROSTER_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let store = fresh_store();
        let p = store.create("room", Some("u-alice"), None).unwrap();

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = ["u-bob", "u-carol"]
            .iter()
            .map(|user| {
                let store = store.clone();
                let id = p.id.clone();
                let user = (*user).to_string();
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    for _ in 0..200 {
                        store.add_member(&id, &user).unwrap();
                        store.remove_member(&id, &user).unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(
            store.members(&p.id).unwrap(),
            vec!["u-alice".to_string()],
            "the table itself must end with only the owner"
        );
        for gone in ["u-bob", "u-carol"] {
            assert!(
                !roster::is_member(&p.id, gone),
                "{gone} was removed but the projection still admits them"
            );
        }
        assert!(roster::is_member(&p.id, "u-alice"));
    }

    /// The `p-` family must stay distinguishable from the legacy `proj-`
    /// directory family that `partition_visible` rules org-tier.
    #[test]
    fn the_project_id_family_cannot_collide_with_the_legacy_directory_family() {
        assert!(mint_id().starts_with("p-"));
        assert!(
            !"proj-deadbeef".starts_with("p-"),
            "if this ever becomes true, partition_visible's arm order is wrong"
        );
    }

    /// A binding is keyed on the conversation, so a second room cannot claim a
    /// conversation the first one already holds. The refusal must be loud: a
    /// silent overwrite would move an existing room's traffic somewhere else.
    ///
    /// The `project_for_conversation` assertion below also exercises
    /// `project_for_conversation` normalizing a RAW (un-normalized) input:
    /// it passes `"C0A1"` against a row stored under `"c0a1"`. See
    /// `a_binding_written_with_an_operator_spelling_is_found_by_the_session_key_lookup`
    /// for the fuller property -- that this normalization also agrees with a
    /// live `SessionKey`'s, checked against that independent oracle rather
    /// than a literal repeated on both sides the way this test does it.
    #[test]
    fn a_conversation_belongs_to_at_most_one_room() {
        let _guard = crate::projects::roster::TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let store = ProjectStore::new(Connection::open_in_memory().unwrap());
        store.create_schema().unwrap();
        let a = store.create("room a", Some("u-alice"), None).unwrap();
        let b = store.create("room b", Some("u-alice"), None).unwrap();

        store
            .bind_conversation(
                &a.id,
                "telegram",
                BindingPeerKind::Group,
                "C0A1",
                Some("u-alice"),
                None,
            )
            .expect("the first bind succeeds");
        let second = store.bind_conversation(
            &b.id,
            "telegram",
            BindingPeerKind::Group,
            "C0A1",
            Some("u-alice"),
            None,
        );
        assert!(
            matches!(second, Err(ProjectError::Invalid(_))),
            "the second room must be refused, not silently take the conversation over"
        );
        assert_eq!(
            store
                .project_for_conversation("telegram", BindingPeerKind::Group, "C0A1")
                .unwrap(),
            Some(a.id.clone()),
            "the original binding must survive the refused attempt"
        );
    }

    /// The conflict error must quote the operator's own spelling back at
    /// them, not the normalized key `project_channel_bindings` actually
    /// stores under: an operator who typed `Slack`/`#Eng` and is told
    /// `slack:-eng is already bound` may not recognise it as their own input.
    #[test]
    fn the_conflict_error_names_the_operators_original_spelling() {
        let _guard = crate::projects::roster::TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let store = ProjectStore::new(Connection::open_in_memory().unwrap());
        store.create_schema().unwrap();
        let a = store.create("room a", Some("u-alice"), None).unwrap();
        let b = store.create("room b", Some("u-alice"), None).unwrap();
        store
            .bind_conversation(
                &a.id,
                "Slack",
                BindingPeerKind::Group,
                "#Eng",
                Some("u-alice"),
                None,
            )
            .expect("the first bind succeeds");

        let err = store
            .bind_conversation(
                &b.id,
                "Slack",
                BindingPeerKind::Group,
                "#Eng",
                Some("u-alice"),
                None,
            )
            .expect_err("the second room must be refused");
        let ProjectError::Invalid(message) = err else {
            panic!("expected ProjectError::Invalid, got {err:?}");
        };
        assert!(
            message.starts_with("Slack:#Eng"),
            "the error must echo the operator's own spelling (\"Slack:#Eng\"), \
             not the normalized key (\"slack:-eng\"); got: {message}"
        );
    }

    /// Unbinding is idempotent and reports whether anything actually changed —
    /// "nothing was bound" and "I unbound it" are different answers, and a
    /// caller that renders a receipt needs to tell them apart.
    #[test]
    fn unbinding_reports_whether_it_changed_anything() {
        let _guard = crate::projects::roster::TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let store = ProjectStore::new(Connection::open_in_memory().unwrap());
        store.create_schema().unwrap();
        let a = store.create("room a", Some("u-alice"), None).unwrap();
        store
            .bind_conversation(
                &a.id,
                "slack",
                BindingPeerKind::Group,
                "C9",
                Some("u-alice"),
                None,
            )
            .unwrap();

        assert!(store
            .unbind_conversation("slack", BindingPeerKind::Group, "C9")
            .unwrap());
        assert!(!store
            .unbind_conversation("slack", BindingPeerKind::Group, "C9")
            .unwrap());
        assert_eq!(
            store
                .project_for_conversation("slack", BindingPeerKind::Group, "C9")
                .unwrap(),
            None
        );
    }

    /// One room may live in several conversations (Telegram + Slack). The
    /// uniqueness constraint is on the conversation side only — that is what
    /// "one core, many channels" costs here.
    #[test]
    fn a_room_may_be_bound_to_several_conversations() {
        let _guard = crate::projects::roster::TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let store = ProjectStore::new(Connection::open_in_memory().unwrap());
        store.create_schema().unwrap();
        let a = store.create("room a", Some("u-alice"), None).unwrap();
        store
            .bind_conversation(
                &a.id,
                "telegram",
                BindingPeerKind::Group,
                "C1",
                Some("u-alice"),
                None,
            )
            .unwrap();
        store
            .bind_conversation(
                &a.id,
                "slack",
                BindingPeerKind::Group,
                "C2",
                Some("u-alice"),
                Some("#eng"),
            )
            .unwrap();
        let bound = store.bindings_for(&a.id).unwrap();
        assert_eq!(bound.len(), 2);
        // Not bound[1]: both binds land in the same `bound_at` second, so the
        // `ORDER BY bound_at, channel_id, peer_id` tiebreak (correct, and not
        // to be changed for this test) sorts by channel_id -- "slack" before
        // "telegram" -- not by insertion order. Find the row instead of
        // indexing it so this test does not depend on that tiebreak.
        let slack = bound
            .iter()
            .find(|b| b.channel_id == "slack")
            .expect("the slack binding is present");
        assert_eq!(slack.label.as_deref(), Some("#eng"));
    }

    /// A catalogue created before this table existed must still open. The
    /// isolated test HOME only ever builds the newest shape, so the old one has
    /// to be constructed on purpose — the same reason
    /// `a_pre_rooms_catalogue_still_opens_and_indexes` exists two tables up.
    #[test]
    fn a_pre_binding_catalogue_still_opens_and_binds() {
        let _guard = crate::projects::roster::TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE projects (
                 id TEXT PRIMARY KEY,
                 name TEXT NOT NULL,
                 owner_user_id TEXT,
                 workspace_path TEXT,
                 status TEXT NOT NULL DEFAULT 'active',
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL,
                 last_used_at INTEGER NOT NULL
             );",
        )
        .unwrap();
        let store = ProjectStore::new(conn);
        store
            .create_schema()
            .expect("a catalogue predating the binding table must migrate, not fail to open");
        let a = store.create("room a", Some("u-alice"), None).unwrap();
        store
            .bind_conversation(
                &a.id,
                "telegram",
                BindingPeerKind::Group,
                "C0",
                Some("u-alice"),
                None,
            )
            .expect("binding must work on a migrated catalogue");
    }

    /// `bind_conversation` normalizes its own inputs via
    /// [`binding::normalize_component`]. What is worth pinning is not that
    /// fact alone -- it is that the normalization agrees with a live
    /// `SessionKey`'s, checked against an INDEPENDENT oracle rather than a
    /// second call to the same function: this binds through
    /// `bind_conversation` with the operator's raw spelling, then derives the
    /// expected key by constructing a real `SessionKey::group` and reading
    /// its own sanitized fields via `conversation_of` -- never calling
    /// `normalize_component` directly. That is the WRITE side.
    ///
    /// `SessionKey::group` normalizes at construction, so the value
    /// `conversation_of` hands back is already normalized: querying
    /// `project_for_conversation` with it cannot tell whether
    /// `project_for_conversation` normalizes its OWN inputs, only whether
    /// `bind_conversation` did. So this test also queries directly with the
    /// operator's raw, un-normalized spelling -- the READ side, and the only
    /// thing that distinguishes this test from
    /// `a_conversation_belongs_to_at_most_one_room` covering the same ground.
    /// Both assertions must stay: deleting either one un-pins the half it
    /// alone proves, silently.
    #[test]
    fn a_binding_written_with_an_operator_spelling_is_found_by_the_session_key_lookup() {
        let _guard = crate::projects::roster::TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let store = ProjectStore::new(Connection::open_in_memory().unwrap());
        store.create_schema().unwrap();
        let room = store.create("room a", Some("u-alice"), None).unwrap();
        store
            .bind_conversation(
                &room.id,
                "Slack",
                BindingPeerKind::Group,
                "#Eng",
                Some("u-alice"),
                Some("#Eng"),
            )
            .expect("bind with the operator's own spelling, capitals and all");

        // WRITE side: a live inbound message builds its SessionKey from the
        // channel adapter's raw ids the same way every real conversation
        // does -- NOT pre-normalized by the caller. If bind_conversation's
        // normalization disagreed with SessionKey::group's, this lookup
        // (keyed on values conversation_of derives independently, never
        // calling normalize_component) would miss.
        let key = crate::routing::session_key::SessionKey::group(
            "main",
            "Slack",
            crate::routing::session_key::PeerKind::Group,
            "#Eng",
        );
        let (channel, kind, peer) =
            binding::conversation_of(&key).expect("a group key is a conversation");
        assert_eq!(
            store
                .project_for_conversation(&channel, kind, &peer)
                .unwrap(),
            Some(room.id.clone()),
            "a binding written with the operator's original spelling must be \
             found by a lookup keyed on what a live SessionKey independently \
             derives -- bind_conversation's normalization must agree with \
             SessionKey::group's, not merely with itself"
        );

        // READ side: query with the operator's raw spelling directly, not
        // via conversation_of. This is what pins project_for_conversation
        // normalizing its own inputs -- the assertion above is structurally
        // blind to that half, since conversation_of already normalized what
        // it fed it.
        assert_eq!(
            store
                .project_for_conversation("Slack", BindingPeerKind::Group, "#Eng")
                .unwrap(),
            Some(room.id),
            "project_for_conversation must normalize a raw query the same way \
             bind_conversation normalized what it stored, or a lookup using \
             the operator's own spelling would never find a binding that \
             lists as bound"
        );
    }
}
