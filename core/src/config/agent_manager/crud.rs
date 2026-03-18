//! CRUD operations for agent definitions

use std::fs;

use toml_edit::Array;
use tracing::{info, warn};

use crate::config::agent_resolver::{initialize_agent_dir, initialize_workspace};
use crate::config::types::agents_def::AgentDefinition;
use crate::error::{AlephError, Result};

use super::{AgentManager, AgentPatch};

impl AgentManager {
    /// Create a new AgentManager
    ///
    /// On construction, ensures at least one default agent exists in the config
    /// file. If `[[agents.list]]` is empty or missing, writes a default "main"
    /// agent to the TOML config and creates its workspace directory.
    ///
    /// Also reconciles orphan workspaces: if a workspace directory exists under
    /// `workspace_root` but has no corresponding entry in config, a minimal
    /// definition is appended so it becomes visible in both the Panel UI and
    /// the `agent_list` tool.
    pub fn new(
        config_path: std::path::PathBuf,
        workspace_root: std::path::PathBuf,
        agents_root: std::path::PathBuf,
        trash_root: std::path::PathBuf,
    ) -> Self {
        let mgr = Self {
            config_path,
            workspace_root,
            agents_root,
            trash_root,
        };

        // Ensure at least one agent exists in config file
        if let Ok(config) = mgr.load_config() {
            if config.list.is_empty() {
                // Remove `list = []` (plain array) if present — it conflicts
                // with the `[[agents.list]]` (array of tables) format that
                // append_agent_to_document expects.
                if let Ok(mut doc) = mgr.load_document() {
                    if let Some(agents) = doc.get_mut("agents").and_then(|v| v.as_table_mut()) {
                        if agents.get("list").and_then(|v| v.as_array()).is_some() {
                            agents.remove("list");
                            let _ = mgr.save_document(&doc);
                        }
                    }
                }

                let def = AgentDefinition {
                    id: "main".to_string(),
                    default: true,
                    name: Some("Main Agent".to_string()),
                    ..Default::default()
                };
                if let Err(e) = mgr.create(def) {
                    warn!("Failed to create default agent in config: {}", e);
                } else {
                    info!("Created default 'main' agent in config");
                }
            }
        }

        // Reconcile orphan workspaces with config
        mgr.reconcile_orphan_workspaces();

        mgr
    }

    /// Scan workspace_root for directories that have no matching config entry
    /// and register them as minimal agent definitions.
    fn reconcile_orphan_workspaces(&self) {
        let entries = match fs::read_dir(&self.workspace_root) {
            Ok(entries) => entries,
            Err(_) => return,
        };

        let config = match self.load_config() {
            Ok(c) => c,
            Err(_) => return,
        };

        let known_ids: std::collections::HashSet<&str> =
            config.list.iter().map(|a| a.id.as_str()).collect();

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(name) => name.to_string(),
                None => continue,
            };

            // Skip hidden directories (e.g. .DS_Store folder)
            if dir_name.starts_with('.') {
                continue;
            }

            // Skip if already in config
            if known_ids.contains(dir_name.as_str()) {
                continue;
            }

            // Validate that this looks like a valid agent ID
            if self.validate_id(&dir_name).is_err() {
                warn!(
                    "Skipping orphan workspace '{}': invalid agent ID format",
                    dir_name
                );
                continue;
            }

            // Try to extract agent name from SOUL.md first line (# Agent Name)
            let agent_name = self.read_agent_name_from_workspace(&path)
                .unwrap_or_else(|| dir_name.clone());

            let def = AgentDefinition {
                id: dir_name.clone(),
                name: Some(agent_name),
                ..Default::default()
            };

            match self.append_agent_to_config(&def) {
                Ok(()) => {
                    info!(
                        "Reconciled orphan workspace '{}' into config",
                        dir_name
                    );
                }
                Err(e) => {
                    warn!(
                        "Failed to reconcile orphan workspace '{}': {}",
                        dir_name, e
                    );
                }
            }
        }
    }

    /// Try to read the agent's display name from workspace files.
    ///
    /// Checks IDENTITY.md for `**Name:** value` first, then falls back to
    /// the directory name.
    fn read_agent_name_from_workspace(&self, ws_path: &std::path::Path) -> Option<String> {
        // Try IDENTITY.md: look for "**Name:** <value>" pattern
        let identity_path = ws_path.join("IDENTITY.md");
        if let Ok(content) = fs::read_to_string(identity_path) {
            for line in content.lines() {
                let trimmed = line.trim().trim_start_matches("- ");
                if let Some(rest) = trimmed.strip_prefix("**Name:**") {
                    let name = rest.trim();
                    if !name.is_empty() {
                        return Some(name.to_string());
                    }
                }
            }
        }
        None
    }

    /// Append an agent definition to the config file without creating workspace
    /// directories (they already exist for orphan workspaces).
    fn append_agent_to_config(&self, def: &AgentDefinition) -> Result<()> {
        let mut doc = self.load_document()?;
        self.append_agent_to_document(&mut doc, def)?;
        self.save_document(&doc)?;
        Ok(())
    }

    // =========================================================================
    // Public CRUD API
    // =========================================================================

    /// List all agent definitions from config
    pub fn list(&self) -> Result<Vec<AgentDefinition>> {
        let config = self.load_config()?;
        Ok(config.list)
    }

    /// Get a single agent definition by ID
    pub fn get(&self, id: &str) -> Result<AgentDefinition> {
        let config = self.load_config()?;
        config
            .list
            .into_iter()
            .find(|a| a.id == id)
            .ok_or_else(|| AlephError::invalid_config(format!("Agent '{}' not found", id)))
    }

    /// Create a new agent definition
    ///
    /// Validates the ID, checks uniqueness, appends to TOML,
    /// creates workspace directory, and initializes SOUL.md.
    pub fn create(&self, def: AgentDefinition) -> Result<()> {
        self.validate_id(&def.id)?;

        // Check uniqueness
        let existing = self.load_config()?;
        if existing.list.iter().any(|a| a.id == def.id) {
            return Err(AlephError::invalid_config(format!(
                "Agent '{}' already exists",
                def.id
            )));
        }

        // Append to TOML document
        let mut doc = self.load_document()?;
        self.append_agent_to_document(&mut doc, &def)?;
        self.save_document(&doc)?;

        // Initialize agent state directory + identity files (SOUL.md, etc.)
        let agent_state_dir = self.agents_root.join(&def.id);
        initialize_agent_dir(&agent_state_dir).map_err(|e| {
            AlephError::IoError(format!(
                "Failed to initialize agent dir for '{}': {}",
                def.id, e
            ))
        })?;
        let agent_name = def.name.as_deref().unwrap_or(&def.id);

        // Workspace directory for project files (SOUL.md, AGENTS.md, etc.)
        let ws_dir = self.workspace_root.join(&def.id);
        std::fs::create_dir_all(&ws_dir).map_err(|e| {
            AlephError::IoError(format!(
                "Failed to create workspace for '{}': {}",
                def.id, e
            ))
        })?;
        initialize_workspace(&ws_dir, agent_name).map_err(|e| {
            AlephError::IoError(format!(
                "Failed to initialize workspace files for '{}': {}",
                def.id, e
            ))
        })?;

        info!(
            "Created agent '{}' with workspace at {}",
            def.id,
            ws_dir.display()
        );
        Ok(())
    }

    /// Update an existing agent's fields via patch
    pub fn update(&self, id: &str, patch: AgentPatch) -> Result<()> {
        let mut doc = self.load_document()?;
        let idx = self.find_agent_index(&doc, id)?;

        let agents_table = doc
            .get_mut("agents")
            .and_then(|v| v.as_table_like_mut())
            .ok_or_else(|| AlephError::invalid_config("[agents] section not found"))?;

        let list = agents_table
            .get_mut("list")
            .and_then(|v| v.as_array_of_tables_mut())
            .ok_or_else(|| AlephError::invalid_config("[[agents.list]] not found"))?;

        let agent_table = list.get_mut(idx).ok_or_else(|| {
            AlephError::invalid_config(format!("Agent at index {} not found", idx))
        })?;

        // Apply patch fields
        if let Some(name) = &patch.name {
            agent_table["name"] = toml_edit::value(name.as_str());
        }

        if let Some(identity) = &patch.identity {
            let mut t = toml_edit::Table::new();
            if let Some(ref emoji) = identity.emoji {
                t["emoji"] = toml_edit::value(emoji.as_str());
            }
            if let Some(ref desc) = identity.description {
                t["description"] = toml_edit::value(desc.as_str());
            }
            if let Some(ref avatar) = identity.avatar {
                t["avatar"] = toml_edit::value(avatar.as_str());
            }
            if let Some(ref theme) = identity.theme {
                t["theme"] = toml_edit::value(theme.as_str());
            }
            agent_table["identity"] = toml_edit::Item::Table(t);
        }

        if let Some(mc) = &patch.model_config {
            let mut t = toml_edit::Table::new();
            t["primary"] = toml_edit::value(mc.primary.as_str());
            if !mc.fallbacks.is_empty() {
                let mut arr = Array::new();
                for f in &mc.fallbacks {
                    arr.push(f.as_str());
                }
                t["fallbacks"] = toml_edit::value(arr);
            }
            agent_table["model_config"] = toml_edit::Item::Table(t);
        }

        if let Some(params) = &patch.params {
            let mut t = toml_edit::Table::new();
            if let Some(temp) = params.temperature {
                t["temperature"] = toml_edit::value(temp as f64);
            }
            if let Some(max_tok) = params.max_tokens {
                t["max_tokens"] = toml_edit::value(max_tok as i64);
            }
            if let Some(top_p) = params.top_p {
                t["top_p"] = toml_edit::value(top_p as f64);
            }
            if let Some(top_k) = params.top_k {
                t["top_k"] = toml_edit::value(top_k as i64);
            }
            agent_table["params"] = toml_edit::Item::Table(t);
        }

        if let Some(skills) = &patch.skills {
            let mut arr = Array::new();
            for s in skills {
                arr.push(s.as_str());
            }
            agent_table["skills"] = toml_edit::value(arr);
        }

        if let Some(skills_blacklist) = &patch.skills_blacklist {
            let mut arr = Array::new();
            for s in skills_blacklist {
                arr.push(s.as_str());
            }
            agent_table["skills_blacklist"] = toml_edit::value(arr);
        }

        if let Some(subagents) = &patch.subagents {
            let mut t = toml_edit::Table::new();
            let mut arr = Array::new();
            for a in &subagents.allow {
                arr.push(a.as_str());
            }
            t["allow"] = toml_edit::value(arr);
            agent_table["subagents"] = toml_edit::Item::Table(t);
        }

        if let Some(allowed_links) = &patch.allowed_links {
            if allowed_links.is_empty() {
                // Empty list = all allowed, remove the key
                agent_table.remove("allowed_links");
            } else {
                let mut arr = Array::new();
                for l in allowed_links {
                    arr.push(l.as_str());
                }
                agent_table["allowed_links"] = toml_edit::value(arr);
            }
        }

        self.save_document(&doc)?;
        info!("Updated agent '{}'", id);
        Ok(())
    }

    /// Delete an agent definition
    ///
    /// Rejects deletion of the only agent or the default agent.
    /// Moves the workspace directory to trash.
    pub fn delete(&self, id: &str) -> Result<()> {
        let config = self.load_config()?;

        // Reject if only agent
        if config.list.len() <= 1 {
            return Err(AlephError::invalid_config(
                "Cannot delete the only agent",
            ));
        }

        // Reject if default agent
        if let Some(agent) = config.list.iter().find(|a| a.id == id) {
            if agent.default {
                return Err(AlephError::invalid_config(
                    "Cannot delete the default agent. Set another agent as default first.",
                ));
            }
        } else {
            return Err(AlephError::invalid_config(format!(
                "Agent '{}' not found",
                id
            )));
        }

        // Remove from TOML document
        let mut doc = self.load_document()?;
        let idx = self.find_agent_index(&doc, id)?;

        let agents_table = doc
            .get_mut("agents")
            .and_then(|v| v.as_table_like_mut())
            .ok_or_else(|| AlephError::invalid_config("[agents] section not found"))?;

        let list = agents_table
            .get_mut("list")
            .and_then(|v| v.as_array_of_tables_mut())
            .ok_or_else(|| AlephError::invalid_config("[[agents.list]] not found"))?;

        list.remove(idx);
        self.save_document(&doc)?;

        // Move workspace to trash
        let ws_dir = self.workspace_root.join(id);
        if ws_dir.exists() {
            let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
            let trash_name = format!("{}_{}", id, timestamp);
            let trash_dir = self.trash_root.join(trash_name);
            fs::create_dir_all(&self.trash_root).map_err(|e| {
                AlephError::IoError(format!("Failed to create trash dir: {}", e))
            })?;
            fs::rename(&ws_dir, &trash_dir).map_err(|e| {
                AlephError::IoError(format!(
                    "Failed to move workspace to trash: {}",
                    e
                ))
            })?;
            info!("Moved workspace to trash: {}", trash_dir.display());
        }

        // Move agent state directory to trash
        let agent_state_dir = self.agents_root.join(id);
        if agent_state_dir.exists() {
            let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
            let trash_name = format!("{}_agent_{}", id, timestamp);
            let trash_dir = self.trash_root.join(trash_name);
            fs::create_dir_all(&self.trash_root).map_err(|e| {
                AlephError::IoError(format!("Failed to create trash dir: {}", e))
            })?;
            fs::rename(&agent_state_dir, &trash_dir).map_err(|e| {
                AlephError::IoError(format!(
                    "Failed to move agent state dir to trash: {}",
                    e
                ))
            })?;
            info!("Moved agent state to trash: {}", trash_dir.display());
        }

        info!("Deleted agent '{}'", id);
        Ok(())
    }

    /// Set an agent as the default, unsetting all others
    pub fn set_default(&self, id: &str) -> Result<()> {
        let mut doc = self.load_document()?;

        // Verify the target agent exists
        let _target_idx = self.find_agent_index(&doc, id)?;

        let agents_table = doc
            .get_mut("agents")
            .and_then(|v| v.as_table_like_mut())
            .ok_or_else(|| AlephError::invalid_config("[agents] section not found"))?;

        let list = agents_table
            .get_mut("list")
            .and_then(|v| v.as_array_of_tables_mut())
            .ok_or_else(|| AlephError::invalid_config("[[agents.list]] not found"))?;

        for i in 0..list.len() {
            if let Some(table) = list.get_mut(i) {
                let agent_id = table
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                table["default"] = toml_edit::value(agent_id == id);
            }
        }

        self.save_document(&doc)?;
        info!("Set default agent to '{}'", id);
        Ok(())
    }

    // =========================================================================
    // Validation helpers
    // =========================================================================

    /// Validate agent ID: 1-32 chars, alphanumeric, hyphens, underscores
    pub(super) fn validate_id(&self, id: &str) -> Result<()> {
        if id.is_empty() || id.len() > super::MAX_ID_LENGTH {
            return Err(AlephError::invalid_config(format!(
                "Agent ID must be 1-{} characters, got {}",
                super::MAX_ID_LENGTH,
                id.len()
            )));
        }

        if !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(AlephError::invalid_config(format!(
                "Agent ID '{}' contains invalid characters. Only alphanumeric, hyphens, and underscores are allowed.",
                id
            )));
        }

        Ok(())
    }

    /// Validate filename: no path separators or traversal
    pub(super) fn validate_filename(&self, filename: &str) -> Result<()> {
        if filename.is_empty()
            || filename.contains('/')
            || filename.contains('\\')
            || filename.contains("..")
        {
            return Err(AlephError::invalid_config(format!(
                "Invalid filename '{}': must not contain '/', '\\', or '..'",
                filename
            )));
        }
        Ok(())
    }
}
