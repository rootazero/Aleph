//! Workspace CRUD, channel active agent, and maintenance operations

use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};
use std::collections::HashMap;
use tracing::{debug, info};

use super::{CacheState, Workspace, WorkspaceError, WorkspaceManager};

impl WorkspaceManager {
    // =========================================================================
    // Workspace CRUD
    // =========================================================================

    /// Create a new workspace
    pub async fn create(
        &self,
        id: &str,
        profile: &str,
        description: Option<&str>,
    ) -> Result<Workspace, WorkspaceError> {
        // Validate profile exists
        if self.get_profile(profile).is_none() {
            return Err(WorkspaceError::ProfileNotFound(profile.to_string()));
        }

        let now = Utc::now();
        let workspace = Workspace {
            id: id.to_string(),
            profile: profile.to_string(),
            created_at: now,
            last_active_at: now,
            cache_state: CacheState::None,
            env_vars: HashMap::new(),
            description: description.map(String::from),
            name: id.to_string(),
            icon: None,
            is_archived: false,
            decay_rate: None,
            permanent_fact_types: Vec::new(),
            default_model: None,
            system_prompt_override: None,
            allowed_tools: Vec::new(),
        };

        let conn = self.conn.lock().map_err(|e| {
            WorkspaceError::Database(format!("Lock error: {}", e))
        })?;

        conn.execute(
            "INSERT INTO workspaces (id, profile, created_at, last_active_at, description, name)
             VALUES (?, ?, ?, ?, ?, ?)",
            params![
                &workspace.id,
                &workspace.profile,
                now.timestamp(),
                now.timestamp(),
                &workspace.description,
                &workspace.name,
            ],
        )
        .map_err(|e| {
            if e.to_string().contains("UNIQUE constraint") {
                WorkspaceError::AlreadyExists(id.to_string())
            } else {
                WorkspaceError::Database(format!("Insert failed: {}", e))
            }
        })?;

        info!("Created workspace '{}' with profile '{}'", id, profile);

        Ok(workspace)
    }

    /// Get a workspace by ID
    pub async fn get(&self, id: &str) -> Result<Option<Workspace>, WorkspaceError> {
        let conn = self.conn.lock().map_err(|e| {
            WorkspaceError::Database(format!("Lock error: {}", e))
        })?;

        let result = conn.query_row(
            "SELECT id, profile, created_at, last_active_at, cache_state, env_vars, description,
                    name, icon, is_archived, decay_rate, permanent_fact_types,
                    default_model, system_prompt_override, allowed_tools
             FROM workspaces WHERE id = ? AND archived = 0",
            params![id],
            |row| {
                Self::row_to_workspace(row)
            },
        );

        match result {
            Ok(ws) => Ok(Some(ws)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(WorkspaceError::Database(e.to_string())),
        }
    }

    /// List all workspaces
    pub async fn list(&self, include_archived: bool) -> Result<Vec<Workspace>, WorkspaceError> {
        let conn = self.conn.lock().map_err(|e| {
            WorkspaceError::Database(format!("Lock error: {}", e))
        })?;

        let query = if include_archived {
            "SELECT id, profile, created_at, last_active_at, cache_state, env_vars, description,
                    name, icon, is_archived, decay_rate, permanent_fact_types,
                    default_model, system_prompt_override, allowed_tools
             FROM workspaces ORDER BY last_active_at DESC"
        } else {
            "SELECT id, profile, created_at, last_active_at, cache_state, env_vars, description,
                    name, icon, is_archived, decay_rate, permanent_fact_types,
                    default_model, system_prompt_override, allowed_tools
             FROM workspaces WHERE archived = 0 ORDER BY last_active_at DESC"
        };

        let mut stmt = conn.prepare(query)
            .map_err(|e| WorkspaceError::Database(e.to_string()))?;

        let workspaces = stmt
            .query_map([], Self::row_to_workspace)
            .map_err(|e| WorkspaceError::Database(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(workspaces)
    }

    /// Update workspace metadata (name, description, icon)
    ///
    /// Only non-None fields are applied. Uses COALESCE to preserve existing
    /// values for fields not provided. Returns the updated workspace,
    /// or None if the workspace was not found.
    pub async fn update(
        &self,
        id: &str,
        name: Option<&str>,
        description: Option<&str>,
        icon: Option<&str>,
    ) -> Result<Option<Workspace>, WorkspaceError> {
        if id == "global" {
            return Err(WorkspaceError::CannotModifyGlobal);
        }

        // Scope the MutexGuard so it is dropped before any .await
        let affected = {
            let conn = self.conn.lock().map_err(|e| {
                WorkspaceError::Database(format!("Lock error: {}", e))
            })?;

            let now = Utc::now().timestamp();
            let name_owned = name.map(|s| s.to_string());
            let desc_owned = description.map(|s| s.to_string());
            let icon_owned = icon.map(|s| s.to_string());

            conn.execute(
                "UPDATE workspaces SET
                    name = COALESCE(?1, name),
                    description = COALESCE(?2, description),
                    icon = COALESCE(?3, icon),
                    last_active_at = ?4
                 WHERE id = ?5",
                params![name_owned, desc_owned, icon_owned, now, id],
            )
            .map_err(|e| WorkspaceError::Database(format!("Update failed: {}", e)))?
        };

        if affected == 0 {
            return Ok(None);
        }

        debug!("Updated workspace '{}' metadata", id);
        self.get(id).await
    }

    /// Update workspace's last active timestamp
    pub async fn touch(&self, id: &str) -> Result<(), WorkspaceError> {
        let conn = self.conn.lock().map_err(|e| {
            WorkspaceError::Database(format!("Lock error: {}", e))
        })?;

        conn.execute(
            "UPDATE workspaces SET last_active_at = ? WHERE id = ?",
            params![Utc::now().timestamp(), id],
        )
        .map_err(|e| WorkspaceError::Database(e.to_string()))?;

        Ok(())
    }

    /// Update workspace's cache state
    pub async fn update_cache_state(
        &self,
        id: &str,
        cache_state: &CacheState,
    ) -> Result<(), WorkspaceError> {
        let conn = self.conn.lock().map_err(|e| {
            WorkspaceError::Database(format!("Lock error: {}", e))
        })?;

        let cache_json = serde_json::to_string(cache_state)
            .map_err(|e| WorkspaceError::Database(format!("Serialize error: {}", e)))?;

        conn.execute(
            "UPDATE workspaces SET cache_state = ?, last_active_at = ? WHERE id = ?",
            params![cache_json, Utc::now().timestamp(), id],
        )
        .map_err(|e| WorkspaceError::Database(e.to_string()))?;

        debug!("Updated cache state for workspace '{}'", id);

        Ok(())
    }

    /// Archive a workspace (soft delete)
    pub async fn archive(&self, id: &str) -> Result<bool, WorkspaceError> {
        if id == "global" {
            return Err(WorkspaceError::CannotModifyGlobal);
        }

        let conn = self.conn.lock().map_err(|e| {
            WorkspaceError::Database(format!("Lock error: {}", e))
        })?;

        let affected = conn
            .execute("UPDATE workspaces SET archived = 1 WHERE id = ?", params![id])
            .map_err(|e| WorkspaceError::Database(e.to_string()))?;

        if affected > 0 {
            info!("Archived workspace '{}'", id);
        }

        Ok(affected > 0)
    }

    /// Delete a workspace permanently
    pub async fn delete(&self, id: &str) -> Result<bool, WorkspaceError> {
        if id == "global" {
            return Err(WorkspaceError::CannotModifyGlobal);
        }

        let conn = self.conn.lock().map_err(|e| {
            WorkspaceError::Database(format!("Lock error: {}", e))
        })?;

        // Remove any channel_active_agent references pointing to this workspace (agent_id = workspace_id in 1:1 model)
        conn.execute(
            "DELETE FROM channel_active_agent WHERE agent_id = ?",
            params![id],
        )
        .ok();

        let affected = conn
            .execute("DELETE FROM workspaces WHERE id = ?", params![id])
            .map_err(|e| WorkspaceError::Database(e.to_string()))?;

        if affected > 0 {
            info!("Deleted workspace '{}'", id);
        }

        Ok(affected > 0)
    }

    // =========================================================================
    // Channel Active Agent
    // =========================================================================

    /// Set the active agent for a channel+peer combination.
    ///
    /// Enforces 1:1 constraint: an agent may only be bound to one channel at a time.
    /// Returns `AgentAlreadyBound` if the agent is already bound to a different channel.
    pub fn set_active_agent(&self, channel: &str, peer_id: &str, agent_id: &str) -> Result<(), WorkspaceError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = Utc::now().timestamp();

        // 1:1 constraint: check if agent is already bound to another channel
        let existing: Option<String> = conn.prepare(
            "SELECT channel FROM channel_active_agent WHERE agent_id = ?1 AND channel != ?2"
        ).map_err(|e| WorkspaceError::Database(e.to_string()))?
        .query_row(params![agent_id, channel], |row| row.get(0))
        .optional()
        .map_err(|e| WorkspaceError::Database(e.to_string()))?;

        if let Some(occupied_channel) = existing {
            return Err(WorkspaceError::AgentAlreadyBound {
                agent_id: agent_id.to_string(),
                channel: occupied_channel,
            });
        }

        // Clear all existing bindings for this channel (any peer_id) before inserting
        conn.execute(
            "DELETE FROM channel_active_agent WHERE channel = ?1",
            params![channel],
        ).map_err(|e| WorkspaceError::Database(e.to_string()))?;

        conn.execute(
            "INSERT INTO channel_active_agent (channel, peer_id, agent_id, updated_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![channel, peer_id, agent_id, now],
        ).map_err(|e| WorkspaceError::Database(e.to_string()))?;
        Ok(())
    }

    /// Clear the active agent override for a channel+peer combination.
    ///
    /// This restores default routing (config bindings / default_agent) for
    /// the given channel+peer. Called when user switches back to "main".
    pub fn clear_active_agent(&self, channel: &str, _peer_id: &str) -> Result<(), WorkspaceError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        // Clear all bindings for this channel (any peer_id) to ensure clean unbind
        conn.execute(
            "DELETE FROM channel_active_agent WHERE channel = ?1",
            params![channel],
        ).map_err(|e| WorkspaceError::Database(e.to_string()))?;
        Ok(())
    }

    /// Get the active agent for a channel+peer combination
    pub fn get_active_agent(&self, channel: &str, peer_id: &str) -> Result<Option<String>, WorkspaceError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT agent_id FROM channel_active_agent WHERE channel = ?1 AND peer_id = ?2"
        ).map_err(|e| WorkspaceError::Database(e.to_string()))?;
        let result = stmt.query_row(params![channel, peer_id], |row| row.get::<_, String>(0));
        match result {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(WorkspaceError::Database(e.to_string())),
        }
    }

    /// Reverse lookup: which channel is this agent bound to?
    pub fn get_channel_for_agent(&self, agent_id: &str) -> Result<Option<String>, WorkspaceError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT channel FROM channel_active_agent WHERE agent_id = ?1 LIMIT 1"
        ).map_err(|e| WorkspaceError::Database(e.to_string()))?;
        let result = stmt.query_row(params![agent_id], |row| row.get::<_, String>(0));
        match result {
            Ok(ch) => Ok(Some(ch)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(WorkspaceError::Database(e.to_string())),
        }
    }

    /// Get all agent→channel bindings (for Panel agents.bindings RPC).
    pub fn get_all_agent_bindings(&self) -> Result<std::collections::HashMap<String, String>, WorkspaceError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT DISTINCT agent_id, channel FROM channel_active_agent"
        ).map_err(|e| WorkspaceError::Database(e.to_string()))?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }).map_err(|e| WorkspaceError::Database(e.to_string()))?;
        let mut map = std::collections::HashMap::new();
        for row in rows {
            let (agent_id, channel) = row.map_err(|e| WorkspaceError::Database(e.to_string()))?;
            map.insert(agent_id, channel);
        }
        Ok(map)
    }

    // =========================================================================
    // Maintenance
    // =========================================================================

    /// Archive inactive workspaces
    pub async fn archive_inactive(&self) -> Result<usize, WorkspaceError> {
        if self.config.archive_after_days == 0 {
            return Ok(0);
        }

        let threshold = Utc::now().timestamp()
            - (self.config.archive_after_days as i64 * 24 * 60 * 60);

        let conn = self.conn.lock().map_err(|e| {
            WorkspaceError::Database(format!("Lock error: {}", e))
        })?;

        let affected = conn
            .execute(
                "UPDATE workspaces SET archived = 1
                 WHERE last_active_at < ? AND id != 'global' AND archived = 0",
                params![threshold],
            )
            .map_err(|e| WorkspaceError::Database(e.to_string()))?;

        if affected > 0 {
            info!("Archived {} inactive workspaces", affected);
        }

        Ok(affected)
    }

    // =========================================================================
    // Internal Helpers
    // =========================================================================

    /// Parse a workspace row from SQLite
    fn row_to_workspace(row: &rusqlite::Row) -> rusqlite::Result<Workspace> {
        let cache_state_json: Option<String> = row.get(4)?;
        let env_vars_json: Option<String> = row.get(5)?;
        let permanent_fact_types_json: Option<String> = row.get(11)?;
        let allowed_tools_json: Option<String> = row.get(14)?;
        let ws_id: String = row.get(0)?;
        let name: String = row.get::<_, Option<String>>(7)?
            .unwrap_or_else(|| ws_id.clone());

        Ok(Workspace {
            id: ws_id,
            profile: row.get(1)?,
            created_at: DateTime::from_timestamp(row.get::<_, i64>(2)?, 0)
                .unwrap_or_else(Utc::now),
            last_active_at: DateTime::from_timestamp(row.get::<_, i64>(3)?, 0)
                .unwrap_or_else(Utc::now),
            cache_state: cache_state_json
                .and_then(|j| serde_json::from_str(&j).ok())
                .unwrap_or_default(),
            env_vars: env_vars_json
                .and_then(|j| serde_json::from_str(&j).ok())
                .unwrap_or_default(),
            description: row.get(6)?,
            name,
            icon: row.get(8)?,
            is_archived: row.get::<_, i32>(9).unwrap_or(0) != 0,
            decay_rate: row.get(10)?,
            permanent_fact_types: permanent_fact_types_json
                .and_then(|j| serde_json::from_str(&j).ok())
                .unwrap_or_default(),
            default_model: row.get(12)?,
            system_prompt_override: row.get(13)?,
            allowed_tools: allowed_tools_json
                .and_then(|j| serde_json::from_str(&j).ok())
                .unwrap_or_default(),
        })
    }
}
