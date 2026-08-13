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

use rusqlite::{Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::gateway::security::store::OWNER_USER_ID;
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
            add_current_session_key_column(conn)
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
    pub fn project_for_session_key(
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
}
