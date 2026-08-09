//! `AgentEnv` CRUD, channel active agent, and maintenance operations

use chrono::{DateTime, Utc};
use rusqlite::params;
use tracing::{debug, info};

use super::{AgentEnv, AgentEnvError, AgentEnvStore, CacheState};

/// The `agent_envs` columns [`AgentEnvStore::row_to_agent_env`] reads, in the
/// order it reads them.
///
/// A constant rather than the literal repeated at each query site: the mapper
/// reads by **index**, so a column added to one SELECT and not another does not
/// fail — it shifts every later field of that query onto the wrong value, which
/// deserializes as plausible garbage rather than as an error. There were three
/// hand-copied copies of this list before `get_including_archived` would have
/// made a fourth.
const ENV_COLUMNS: &str = "id, profile, created_at, last_active_at, cache_state, \
                           description, name, icon, archived, decay_rate, permanent_fact_types";

impl AgentEnvStore {
    // =========================================================================
    // AgentEnv CRUD
    // =========================================================================

    /// Create a new agent environment
    pub async fn create(
        &self,
        id: &str,
        profile: &str,
        description: Option<&str>,
    ) -> Result<AgentEnv, AgentEnvError> {
        // Validate profile exists
        if self.get_profile(profile).is_none() {
            return Err(AgentEnvError::ProfileNotFound(profile.to_string()));
        }

        let now = Utc::now();
        let env = AgentEnv {
            id: id.to_string(),
            profile: profile.to_string(),
            created_at: now,
            last_active_at: now,
            cache_state: CacheState::None,
            description: description.map(String::from),
            name: id.to_string(),
            icon: None,
            is_archived: false,
            decay_rate: None,
            permanent_fact_types: Vec::new(),
        };

        let conn = self
            .conn
            .lock()
            .map_err(|e| AgentEnvError::Database(format!("Lock error: {e}")))?;

        conn.execute(
            "INSERT INTO agent_envs (id, profile, created_at, last_active_at, description, name)
             VALUES (?, ?, ?, ?, ?, ?)",
            params![
                &env.id,
                &env.profile,
                now.timestamp(),
                now.timestamp(),
                &env.description,
                &env.name,
            ],
        )
        .map_err(|e| {
            if e.to_string().contains("UNIQUE constraint") {
                AgentEnvError::AlreadyExists(id.to_string())
            } else {
                AgentEnvError::Database(format!("Insert failed: {e}"))
            }
        })?;

        info!("Created agent env '{}' with profile '{}'", id, profile);

        Ok(env)
    }

    /// Get an agent environment by ID.
    ///
    /// Active rows only. Nearly every caller is a **runtime** lookup — the env
    /// a run executes under, the profile a channel binds to — and resolving one
    /// of those to a soft-deleted row would quietly resurrect it. Ask
    /// [`Self::get_including_archived`] when the archive itself is the
    /// question.
    pub async fn get(&self, id: &str) -> Result<Option<AgentEnv>, AgentEnvError> {
        self.get_where(id, "AND archived = 0")
    }

    /// Get an agent environment by ID, archived or not.
    ///
    /// The one caller is `workspace.get`, and it is not a runtime lookup: the
    /// caller already holds the exact id, and `is_archived` comes back in the
    /// answer, so nothing is resurrected by showing it. Without this,
    /// `workspace list --include-archived` prints a row that `workspace get`
    /// then reports does not exist — the display-side lie this codebase keeps
    /// paying for, one level down from the one that flag was added to fix.
    ///
    /// Read-only on purpose: archived workspaces are **readable, not
    /// writable**. [`Self::update`] refuses them, and the way back is
    /// [`Self::unarchive`] — a separate verb, so restoring a row is something
    /// someone asked for rather than a side effect of editing one.
    pub async fn get_including_archived(
        &self,
        id: &str,
    ) -> Result<Option<AgentEnv>, AgentEnvError> {
        self.get_where(id, "")
    }

    /// Shared body of the two by-id reads. `extra_predicate` is appended to the
    /// `WHERE` clause and is a compile-time literal at both call sites — it
    /// never carries caller input.
    fn get_where(
        &self,
        id: &str,
        extra_predicate: &str,
    ) -> Result<Option<AgentEnv>, AgentEnvError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AgentEnvError::Database(format!("Lock error: {e}")))?;

        let result = conn.query_row(
            &format!("SELECT {ENV_COLUMNS} FROM agent_envs WHERE id = ? {extra_predicate}"),
            params![id],
            Self::row_to_agent_env,
        );

        match result {
            Ok(ws) => Ok(Some(ws)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AgentEnvError::Database(e.to_string())),
        }
    }

    /// List all agent environments
    pub async fn list(&self, include_archived: bool) -> Result<Vec<AgentEnv>, AgentEnvError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AgentEnvError::Database(format!("Lock error: {e}")))?;

        let filter = if include_archived {
            ""
        } else {
            "WHERE archived = 0"
        };
        let query =
            format!("SELECT {ENV_COLUMNS} FROM agent_envs {filter} ORDER BY last_active_at DESC");

        let mut stmt = conn
            .prepare(&query)
            .map_err(|e| AgentEnvError::Database(e.to_string()))?;

        let envs = stmt
            .query_map([], Self::row_to_agent_env)
            .map_err(|e| AgentEnvError::Database(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(envs)
    }

    /// Update agent environment metadata (name, description, icon)
    ///
    /// Only non-None fields are applied. Uses COALESCE to preserve existing
    /// values for fields not provided. Returns the updated agent environment,
    /// or None if the agent environment was not found **or is archived**.
    ///
    /// # Why the write filters `archived = 0`
    ///
    /// It did not until 2026-08-08, and the two halves of this function then
    /// disagreed about what an archived row is: the UPDATE matched it (no
    /// `archived` clause) while the read-back through [`Self::get`] did not.
    /// So an archived row was **really rewritten** and the caller was then told
    /// `Ok(None)` — which `workspace.update` then rendered as "not found". A write
    /// that lands while the response denies it ever happened is the worst shape
    /// available, and it was unreachable only because that RPC had no client;
    /// wiring `aleph workspace update` up is what would have made it real.
    ///
    /// Filtering the write (rather than widening the read-back) is what makes
    /// the existing `Ok(None)` answer TRUE, and it is the semantics the rest of
    /// the family already has: archived workspaces are readable
    /// ([`Self::get_including_archived`]) and not writable.
    ///
    /// This paragraph used to end "there is no unarchive verb, so this is
    /// terminal by construction, not by omission". [`Self::unarchive`] landed
    /// on 2026-08-09 and archiving is reversible now — but the refusal below is
    /// unchanged, deliberately: the way back is **one explicit verb**, not a
    /// rename that silently resurrects its target. Edit an archived row and it
    /// still fails; unarchive it first.
    ///
    /// This `None` is therefore ambiguous by design — "no such row" and "that
    /// row is archived" arrive as the same value. `handle_update` disambiguates
    /// it with a follow-up read, because the two deserve different refusals;
    /// pushing that into the return type would make every existing caller
    /// handle a distinction only one of them has a use for.
    pub async fn update(
        &self,
        id: &str,
        name: Option<&str>,
        description: Option<&str>,
        icon: Option<&str>,
    ) -> Result<Option<AgentEnv>, AgentEnvError> {
        if id == "global" {
            return Err(AgentEnvError::CannotModifyGlobal);
        }

        // Scope the MutexGuard so it is dropped before any .await
        let affected = {
            let conn = self
                .conn
                .lock()
                .map_err(|e| AgentEnvError::Database(format!("Lock error: {e}")))?;

            let now = Utc::now().timestamp();
            let name_owned = name.map(|s| s.to_string());
            let desc_owned = description.map(|s| s.to_string());
            let icon_owned = icon.map(|s| s.to_string());

            conn.execute(
                "UPDATE agent_envs SET
                    name = COALESCE(?1, name),
                    description = COALESCE(?2, description),
                    icon = COALESCE(?3, icon),
                    last_active_at = ?4
                 WHERE id = ?5 AND archived = 0",
                params![name_owned, desc_owned, icon_owned, now, id],
            )
            .map_err(|e| AgentEnvError::Database(format!("Update failed: {e}")))?
        };

        if affected == 0 {
            return Ok(None);
        }

        debug!("Updated agent env '{}' metadata", id);
        self.get(id).await
    }

    /// Update last active timestamp
    pub async fn touch(&self, id: &str) -> Result<(), AgentEnvError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AgentEnvError::Database(format!("Lock error: {e}")))?;

        conn.execute(
            "UPDATE agent_envs SET last_active_at = ? WHERE id = ?",
            params![Utc::now().timestamp(), id],
        )
        .map_err(|e| AgentEnvError::Database(e.to_string()))?;

        Ok(())
    }

    /// Update cache state
    pub async fn update_cache_state(
        &self,
        id: &str,
        cache_state: &CacheState,
    ) -> Result<(), AgentEnvError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AgentEnvError::Database(format!("Lock error: {e}")))?;

        let cache_json = serde_json::to_string(cache_state)
            .map_err(|e| AgentEnvError::Database(format!("Serialize error: {e}")))?;

        conn.execute(
            "UPDATE agent_envs SET cache_state = ?, last_active_at = ? WHERE id = ?",
            params![cache_json, Utc::now().timestamp(), id],
        )
        .map_err(|e| AgentEnvError::Database(e.to_string()))?;

        debug!("Updated cache state for agent '{}'", id);

        Ok(())
    }

    /// Archive an agent environment (soft delete)
    pub async fn archive(&self, id: &str) -> Result<bool, AgentEnvError> {
        if id == "global" {
            return Err(AgentEnvError::CannotModifyGlobal);
        }

        let conn = self
            .conn
            .lock()
            .map_err(|e| AgentEnvError::Database(format!("Lock error: {e}")))?;

        let affected = conn
            .execute(
                "UPDATE agent_envs SET archived = 1 WHERE id = ?",
                params![id],
            )
            .map_err(|e| AgentEnvError::Database(e.to_string()))?;

        if affected > 0 {
            info!("Archived agent env '{}'", id);
        }

        Ok(affected > 0)
    }

    /// Restore an archived agent environment — the inverse of [`Self::archive`].
    ///
    /// Until 2026-08-09 a mistaken archive was permanent. The row stayed
    /// readable ([`Self::get_including_archived`]) and unwritable
    /// ([`Self::update`] filters `archived = 0`), and the id stayed taken,
    /// because [`Self::create`] is a plain INSERT against a primary key. That
    /// last part is unchanged and is the reason this verb exists rather than a
    /// reclaiming `create`: a workspace id is also the directory name under
    /// `~/.aleph/agents/<id>/`, which holds the notes vault, the memory
    /// partition and the skills. Letting `create` take an archived id would
    /// hand a brand-new workspace the previous one's memory, silently. Archive
    /// does not touch any of that, so restoring the row is all there is to do.
    ///
    /// # Why `Option<AgentEnv>` and not `bool`
    ///
    /// [`Self::archive`] answers `bool` because its caller has nothing left to
    /// show — the row has just left the default view. This one's caller does:
    /// `workspace.unarchive` returns the restored workspace, so a client
    /// re-renders from the response instead of racing a follow-up `get`.
    ///
    /// # This `None` is NOT [`Self::update`]'s `None`
    ///
    /// That one is ambiguous by design: "no such row" and "that row is
    /// archived" arrive as the same value, because its UPDATE filters
    /// `archived = 0`. The statement below carries no `archived` predicate, so
    /// `Ok(None)` means exactly one thing — there is no row with this id. Do
    /// not copy `handle_update`'s follow-up probe here; there is nothing left
    /// to disambiguate, and a probe that cannot change the answer reads like
    /// one that can.
    ///
    /// # Idempotent
    ///
    /// Unarchiving a live row succeeds. The postcondition promised — this
    /// workspace is active — holds either way, and the symmetry is with
    /// [`Self::archive`], which likewise reports success for a row that was
    /// already archived (SQLite counts the rows a statement matched, not the
    /// rows whose values it changed).
    ///
    /// `last_active_at` is bumped for the same reason [`Self::update`] bumps
    /// it: both are metadata writes, the field already means "last touched"
    /// rather than "last conversed" (`WorkspaceDetail::last_active_at` says so
    /// on the wire), and `list` orders by it — a workspace someone just went
    /// looking for should not come back buried.
    pub async fn unarchive(&self, id: &str) -> Result<Option<AgentEnv>, AgentEnvError> {
        if id == "global" {
            return Err(AgentEnvError::CannotModifyGlobal);
        }

        // Scope the MutexGuard so it is dropped before the .await below.
        let affected = {
            let conn = self
                .conn
                .lock()
                .map_err(|e| AgentEnvError::Database(format!("Lock error: {e}")))?;

            conn.execute(
                "UPDATE agent_envs SET archived = 0, last_active_at = ?1 WHERE id = ?2",
                params![Utc::now().timestamp(), id],
            )
            .map_err(|e| AgentEnvError::Database(format!("Unarchive failed: {e}")))?
        };

        if affected == 0 {
            return Ok(None);
        }

        info!("Unarchived agent env '{}'", id);
        self.get(id).await
    }

    // =========================================================================
    // Channel Active Agent
    // =========================================================================

    /// Set the active agent for a channel+peer combination.
    ///
    /// Bind an agent to a channel (many-to-one).
    ///
    /// Multiple channels can be bound to the same agent, but each channel
    /// can only be bound to one agent. Re-binding a channel replaces the
    /// previous binding.
    pub fn set_active_agent(&self, channel: &str, agent_id: &str) -> Result<(), AgentEnvError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = Utc::now().timestamp();

        // Single atomic upsert: `channel` is the PRIMARY KEY, so OR REPLACE
        // deletes any conflicting row and inserts the new one as one statement.
        // The old DELETE-then-INSERT pair left the channel UNBOUND (silent
        // downgrade to the default agent) if the INSERT failed after the DELETE
        // committed; the upsert removes that failure window and a round trip.
        conn.execute(
            "INSERT OR REPLACE INTO channel_active_agent (channel, agent_id, updated_at)
             VALUES (?1, ?2, ?3)",
            params![channel, agent_id, now],
        )
        .map_err(|e| AgentEnvError::Database(e.to_string()))?;
        Ok(())
    }

    /// Clear the agent binding for a channel.
    ///
    /// This restores the channel to an unbound state. Called when unbinding
    /// an agent or when deleting an agent that is currently bound.
    pub fn clear_active_agent(&self, channel: &str) -> Result<(), AgentEnvError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "DELETE FROM channel_active_agent WHERE channel = ?1",
            params![channel],
        )
        .map_err(|e| AgentEnvError::Database(e.to_string()))?;
        Ok(())
    }

    /// Clear ALL channel bindings pointing at this agent.
    ///
    /// The binding model is many-to-one (N channels → 1 agent), so deleting an
    /// agent must drop every channel that pointed at it — otherwise the orphaned
    /// channels keep a stale `agent_id` and the inbound router resolves them to a
    /// ghost agent until it falls back. One DELETE-by-`agent_id` rather than a
    /// loop, so a partial cleanup cannot leave some channels cleared and others
    /// pointing at the ghost. (This used to say it mirrored `Self::delete`;
    /// there is no such method on this store and never has been.)
    pub fn clear_bindings_for_agent(&self, agent_id: &str) -> Result<(), AgentEnvError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "DELETE FROM channel_active_agent WHERE agent_id = ?1",
            params![agent_id],
        )
        .map_err(|e| AgentEnvError::Database(e.to_string()))?;
        Ok(())
    }

    /// Get the active agent for a channel.
    ///
    /// Returns the `agent_id` bound to this channel, or None if unbound.
    pub fn get_active_agent(&self, channel: &str) -> Result<Option<String>, AgentEnvError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare("SELECT agent_id FROM channel_active_agent WHERE channel = ?1")
            .map_err(|e| AgentEnvError::Database(e.to_string()))?;
        let result = stmt.query_row(params![channel], |row| row.get::<_, String>(0));
        match result {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AgentEnvError::Database(e.to_string())),
        }
    }

    /// Get every channel bound to each agent (many-to-one aware).
    ///
    /// The binding model is N channels → 1 agent; this returns the full
    /// `agent_id → [channel, …]` grouping for honest listing (the old lossy
    /// one-channel-per-agent map is gone — both the `agent_list` tool and the
    /// `agents.bindings` RPC read this). Channels are sorted for deterministic
    /// output.
    pub fn bindings_by_agent(
        &self,
    ) -> Result<std::collections::HashMap<String, Vec<String>>, AgentEnvError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                "SELECT agent_id, channel FROM channel_active_agent ORDER BY agent_id, channel",
            )
            .map_err(|e| AgentEnvError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| AgentEnvError::Database(e.to_string()))?;
        let mut map: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for row in rows {
            let (agent_id, channel) = row.map_err(|e| AgentEnvError::Database(e.to_string()))?;
            map.entry(agent_id).or_default().push(channel);
        }
        Ok(map)
    }

    // =========================================================================
    // Internal Helpers
    // =========================================================================

    /// Parse an agent environment row from `SQLite`
    fn row_to_agent_env(row: &rusqlite::Row) -> rusqlite::Result<AgentEnv> {
        let cache_state_json: Option<String> = row.get(4)?;
        let permanent_fact_types_json: Option<String> = row.get(10)?;
        let ws_id: String = row.get(0)?;
        let name: String = row
            .get::<_, Option<String>>(6)?
            .unwrap_or_else(|| ws_id.clone());

        Ok(AgentEnv {
            id: ws_id,
            profile: row.get(1)?,
            created_at: DateTime::from_timestamp(row.get::<_, i64>(2)?, 0).unwrap_or_else(Utc::now),
            last_active_at: DateTime::from_timestamp(row.get::<_, i64>(3)?, 0)
                .unwrap_or_else(Utc::now),
            cache_state: cache_state_json
                .and_then(|j| serde_json::from_str(&j).ok())
                .unwrap_or_default(),
            description: row.get(5)?,
            name,
            icon: row.get(7)?,
            is_archived: row.get::<_, i32>(8).unwrap_or(0) != 0,
            decay_rate: row.get(9)?,
            permanent_fact_types: permanent_fact_types_json
                .and_then(|j| serde_json::from_str(&j).ok())
                .unwrap_or_default(),
        })
    }
}
