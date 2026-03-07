//! Agent Manager — TOML CRUD for agent definitions
//!
//! Provides create, read, update, delete operations on the `[[agents.list]]`
//! section of the config file, plus workspace file management for each agent.
//!
//! Uses `toml_edit` for format-preserving edits and atomic file saves.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use toml_edit::{Array, DocumentMut, Item, Table};
use tracing::{debug, info, warn};

use crate::config::types::agents_def::{
    AgentDefinition, AgentIdentity, AgentModelConfig, AgentParams, AgentsConfig, SubagentPolicy,
};
use crate::config::agent_resolver::{initialize_workspace, initialize_agent_dir};
use crate::error::{AlephError, Result};

// =============================================================================
// Constants
// =============================================================================

/// Bootstrap files recognized in agent workspaces.
/// Auto-created: SOUL.md, AGENTS.md, MEMORY.md
/// Optional (user-created): IDENTITY.md, TOOLS.md, HEARTBEAT.md, BOOTSTRAP.md
const BOOTSTRAP_FILES: &[&str] = &[
    "SOUL.md",
    "IDENTITY.md",
    "AGENTS.md",
    "TOOLS.md",
    "MEMORY.md",
    "HEARTBEAT.md",
    "BOOTSTRAP.md",
];

/// Maximum length for agent IDs
const MAX_ID_LENGTH: usize = 32;

// =============================================================================
// AgentPatch
// =============================================================================

/// Partial update for an agent definition
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentPatch {
    pub name: Option<String>,
    pub identity: Option<AgentIdentity>,
    pub model_config: Option<AgentModelConfig>,
    pub params: Option<AgentParams>,
    pub skills: Option<Vec<String>>,
    pub subagents: Option<SubagentPolicy>,
}

// =============================================================================
// WorkspaceFile
// =============================================================================

/// Metadata for a file in an agent's workspace directory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceFile {
    pub filename: String,
    pub size_bytes: u64,
    pub modified_at: i64,
    pub is_bootstrap: bool,
}

// =============================================================================
// AgentManager
// =============================================================================

/// Manages agent definitions in the TOML config and their workspace directories
pub struct AgentManager {
    config_path: PathBuf,
    pub workspace_root: PathBuf,
    pub agents_root: PathBuf,
    trash_root: PathBuf,
}

impl AgentManager {
    /// Create a new AgentManager
    ///
    /// On construction, ensures at least one default agent exists in the config
    /// file. If `[[agents.list]]` is empty or missing, writes a default "main"
    /// agent to the TOML config and creates its workspace directory.
    pub fn new(
        config_path: PathBuf,
        workspace_root: PathBuf,
        agents_root: PathBuf,
        trash_root: PathBuf,
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

        mgr
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

        // Initialize workspace directory with standard structure
        let ws_dir = self.workspace_root.join(&def.id);
        let agent_name = def.name.as_deref().unwrap_or(&def.id);
        initialize_workspace(&ws_dir, agent_name).map_err(|e| {
            AlephError::IoError(format!(
                "Failed to initialize workspace for '{}': {}",
                def.id, e
            ))
        })?;

        // Initialize agent state directory
        let agent_state_dir = self.agents_root.join(&def.id);
        initialize_agent_dir(&agent_state_dir).map_err(|e| {
            AlephError::IoError(format!(
                "Failed to initialize agent state dir for '{}': {}",
                def.id, e
            ))
        })?;

        info!("Created agent '{}' with workspace at {}", def.id, ws_dir.display());
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
            let mut t = Table::new();
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
            agent_table["identity"] = Item::Table(t);
        }

        if let Some(mc) = &patch.model_config {
            let mut t = Table::new();
            t["primary"] = toml_edit::value(mc.primary.as_str());
            if !mc.fallbacks.is_empty() {
                let mut arr = Array::new();
                for f in &mc.fallbacks {
                    arr.push(f.as_str());
                }
                t["fallbacks"] = toml_edit::value(arr);
            }
            agent_table["model_config"] = Item::Table(t);
        }

        if let Some(params) = &patch.params {
            let mut t = Table::new();
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
            agent_table["params"] = Item::Table(t);
        }

        if let Some(skills) = &patch.skills {
            let mut arr = Array::new();
            for s in skills {
                arr.push(s.as_str());
            }
            agent_table["skills"] = toml_edit::value(arr);
        }

        if let Some(subagents) = &patch.subagents {
            let mut t = Table::new();
            let mut arr = Array::new();
            for a in &subagents.allow {
                arr.push(a.as_str());
            }
            t["allow"] = toml_edit::value(arr);
            agent_table["subagents"] = Item::Table(t);
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
    // Workspace file operations
    // =========================================================================

    /// List files in an agent's workspace directory
    pub fn list_files(&self, agent_id: &str) -> Result<Vec<WorkspaceFile>> {
        let ws_dir = self.workspace_root.join(agent_id);
        if !ws_dir.exists() {
            return Ok(Vec::new());
        }

        let mut files = Vec::new();
        let entries = fs::read_dir(&ws_dir).map_err(|e| {
            AlephError::IoError(format!("Failed to read workspace dir: {}", e))
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| {
                AlephError::IoError(format!("Failed to read dir entry: {}", e))
            })?;
            let metadata = entry.metadata().map_err(|e| {
                AlephError::IoError(format!("Failed to read metadata: {}", e))
            })?;

            if !metadata.is_file() {
                continue;
            }

            let filename = entry.file_name().to_string_lossy().to_string();
            let modified_at = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);

            files.push(WorkspaceFile {
                is_bootstrap: BOOTSTRAP_FILES.contains(&filename.as_str()),
                filename,
                size_bytes: metadata.len(),
                modified_at,
            });
        }

        files.sort_by(|a, b| a.filename.cmp(&b.filename));
        Ok(files)
    }

    /// Read a file from an agent's workspace
    pub fn read_file(&self, agent_id: &str, filename: &str) -> Result<String> {
        self.validate_filename(filename)?;
        let path = self.workspace_root.join(agent_id).join(filename);
        fs::read_to_string(&path).map_err(|e| {
            AlephError::IoError(format!(
                "Failed to read file '{}': {}",
                path.display(),
                e
            ))
        })
    }

    /// Write a file to an agent's workspace
    pub fn write_file(&self, agent_id: &str, filename: &str, content: &str) -> Result<()> {
        self.validate_filename(filename)?;
        let ws_dir = self.workspace_root.join(agent_id);
        fs::create_dir_all(&ws_dir).map_err(|e| {
            AlephError::IoError(format!("Failed to create workspace dir: {}", e))
        })?;
        let path = ws_dir.join(filename);
        fs::write(&path, content).map_err(|e| {
            AlephError::IoError(format!(
                "Failed to write file '{}': {}",
                path.display(),
                e
            ))
        })
    }

    /// Delete a file from an agent's workspace
    pub fn delete_file(&self, agent_id: &str, filename: &str) -> Result<()> {
        self.validate_filename(filename)?;
        let path = self.workspace_root.join(agent_id).join(filename);
        if path.exists() {
            fs::remove_file(&path).map_err(|e| {
                AlephError::IoError(format!(
                    "Failed to delete file '{}': {}",
                    path.display(),
                    e
                ))
            })?;
        }
        Ok(())
    }

    // =========================================================================
    // Internal helpers
    // =========================================================================

    /// Load config and parse the [agents] section
    fn load_config(&self) -> Result<AgentsConfig> {
        let content = fs::read_to_string(&self.config_path).map_err(|e| {
            AlephError::IoError(format!(
                "Failed to read config file '{}': {}",
                self.config_path.display(),
                e
            ))
        })?;

        #[derive(Deserialize)]
        struct Wrapper {
            #[serde(default)]
            agents: AgentsConfig,
        }

        let wrapper: Wrapper = toml::from_str(&content).map_err(|e| {
            AlephError::invalid_config(format!("Failed to parse config: {}", e))
        })?;

        Ok(wrapper.agents)
    }

    /// Load config file as a toml_edit Document for format-preserving edits
    fn load_document(&self) -> Result<DocumentMut> {
        let content = fs::read_to_string(&self.config_path).map_err(|e| {
            AlephError::IoError(format!(
                "Failed to read config file '{}': {}",
                self.config_path.display(),
                e
            ))
        })?;

        content.parse::<DocumentMut>().map_err(|e| {
            AlephError::invalid_config(format!("Failed to parse config as TOML: {}", e))
        })
    }

    /// Atomic write: write to .tmp, fsync, then rename
    fn save_document(&self, doc: &DocumentMut) -> Result<()> {
        let content = doc.to_string();
        let tmp_path = self.config_path.with_extension("toml.tmp");

        fs::write(&tmp_path, &content).map_err(|e| {
            AlephError::IoError(format!("Failed to write tmp config: {}", e))
        })?;

        // fsync on unix
        #[cfg(unix)]
        {
            use std::fs::OpenOptions;
            use std::io::Write;
            if let Ok(mut f) = OpenOptions::new().write(true).open(&tmp_path) {
                let _ = f.flush();
                let _ = f.sync_all();
            }
        }

        fs::rename(&tmp_path, &self.config_path).map_err(|e| {
            AlephError::IoError(format!("Failed to rename tmp config: {}", e))
        })?;

        debug!("Saved config to {}", self.config_path.display());
        Ok(())
    }

    /// Find the index of an agent in the [[agents.list]] array by ID
    fn find_agent_index(&self, doc: &DocumentMut, id: &str) -> Result<usize> {
        let agents_table = doc
            .get("agents")
            .and_then(|v| v.as_table_like())
            .ok_or_else(|| AlephError::invalid_config("[agents] section not found"))?;

        let list = agents_table
            .get("list")
            .and_then(|v| v.as_array_of_tables())
            .ok_or_else(|| AlephError::invalid_config("[[agents.list]] not found"))?;

        for (i, table) in list.iter().enumerate() {
            if table.get("id").and_then(|v| v.as_str()) == Some(id) {
                return Ok(i);
            }
        }

        Err(AlephError::invalid_config(format!(
            "Agent '{}' not found in config",
            id
        )))
    }

    /// Append an AgentDefinition to the [[agents.list]] array in the document
    fn append_agent_to_document(
        &self,
        doc: &mut DocumentMut,
        def: &AgentDefinition,
    ) -> Result<()> {
        // Ensure [agents] table exists
        if doc.get("agents").is_none() {
            doc["agents"] = Item::Table(Table::new());
        }

        // Build the agent table
        let mut agent = Table::new();
        agent["id"] = toml_edit::value(&def.id);

        if def.default {
            agent["default"] = toml_edit::value(true);
        }

        if let Some(ref name) = def.name {
            agent["name"] = toml_edit::value(name.as_str());
        }

        if let Some(ref profile) = def.profile {
            agent["profile"] = toml_edit::value(profile.as_str());
        }

        if let Some(ref model) = def.model {
            agent["model"] = toml_edit::value(model.as_str());
        }

        if let Some(ref skills) = def.skills {
            let mut arr = Array::new();
            for s in skills {
                arr.push(s.as_str());
            }
            agent["skills"] = toml_edit::value(arr);
        }

        if let Some(ref identity) = def.identity {
            let mut t = Table::new();
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
            agent["identity"] = Item::Table(t);
        }

        if let Some(ref mc) = def.model_config {
            let mut t = Table::new();
            t["primary"] = toml_edit::value(mc.primary.as_str());
            if !mc.fallbacks.is_empty() {
                let mut arr = Array::new();
                for f in &mc.fallbacks {
                    arr.push(f.as_str());
                }
                t["fallbacks"] = toml_edit::value(arr);
            }
            agent["model_config"] = Item::Table(t);
        }

        if let Some(ref params) = def.params {
            let mut t = Table::new();
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
            agent["params"] = Item::Table(t);
        }

        if let Some(ref subagents) = def.subagents {
            let mut t = Table::new();
            let mut arr = Array::new();
            for a in &subagents.allow {
                arr.push(a.as_str());
            }
            t["allow"] = toml_edit::value(arr);
            agent["subagents"] = Item::Table(t);
        }

        // Append to [[agents.list]]
        let agents = doc["agents"]
            .as_table_mut()
            .ok_or_else(|| AlephError::invalid_config("[agents] is not a table"))?;

        if agents.get("list").is_none() {
            // Create the array of tables
            agents.insert(
                "list",
                Item::ArrayOfTables(toml_edit::ArrayOfTables::new()),
            );
        }

        let list = agents
            .get_mut("list")
            .and_then(|v| v.as_array_of_tables_mut())
            .ok_or_else(|| AlephError::invalid_config("[[agents.list]] is not an array of tables"))?;

        list.push(agent);
        Ok(())
    }

    /// Validate agent ID: 1-32 chars, alphanumeric, hyphens, underscores
    fn validate_id(&self, id: &str) -> Result<()> {
        if id.is_empty() || id.len() > MAX_ID_LENGTH {
            return Err(AlephError::invalid_config(format!(
                "Agent ID must be 1-{} characters, got {}",
                MAX_ID_LENGTH,
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
    fn validate_filename(&self, filename: &str) -> Result<()> {
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

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Create a test environment with config file and directories
    fn setup(config_content: &str) -> (TempDir, AgentManager) {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        let workspace_root = dir.path().join("workspaces");
        let agents_root = dir.path().join("agents");
        let trash_root = dir.path().join("trash");

        fs::create_dir_all(&workspace_root).unwrap();
        fs::create_dir_all(&agents_root).unwrap();
        fs::create_dir_all(&trash_root).unwrap();
        fs::write(&config_path, config_content).unwrap();

        let manager = AgentManager::new(config_path, workspace_root, agents_root, trash_root);
        (dir, manager)
    }

    fn base_config() -> &'static str {
        r#"
[agents]

[[agents.list]]
id = "main"
default = true
name = "Main Agent"

[[agents.list]]
id = "coder"
name = "Coder"
"#
    }

    // =========================================================================
    // List / Get
    // =========================================================================

    #[test]
    fn test_list_agents() {
        let (_dir, mgr) = setup(base_config());
        let agents = mgr.list().unwrap();
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].id, "main");
        assert_eq!(agents[1].id, "coder");
    }

    #[test]
    fn test_get_agent() {
        let (_dir, mgr) = setup(base_config());
        let agent = mgr.get("main").unwrap();
        assert_eq!(agent.id, "main");
        assert!(agent.default);
        assert_eq!(agent.name, Some("Main Agent".to_string()));
    }

    #[test]
    fn test_get_agent_not_found() {
        let (_dir, mgr) = setup(base_config());
        let err = mgr.get("nonexistent").unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    // =========================================================================
    // Create
    // =========================================================================

    #[test]
    fn test_create_agent() {
        let (_dir, mgr) = setup(base_config());
        let def = AgentDefinition {
            id: "researcher".to_string(),
            name: Some("Research Agent".to_string()),
            skills: Some(vec!["search".to_string()]),
            ..Default::default()
        };

        mgr.create(def).unwrap();

        // Verify agent was added
        let agents = mgr.list().unwrap();
        assert_eq!(agents.len(), 3);
        let new_agent = agents.iter().find(|a| a.id == "researcher").unwrap();
        assert_eq!(new_agent.name, Some("Research Agent".to_string()));

        // Verify workspace directory was created
        let ws_dir = mgr.workspace_root.join("researcher");
        assert!(ws_dir.exists());

        // Verify SOUL.md was created
        let soul = fs::read_to_string(ws_dir.join("SOUL.md")).unwrap();
        assert!(soul.contains("Research Agent"));

        // Verify TOML is valid by re-parsing
        let content = fs::read_to_string(&mgr.config_path).unwrap();
        let _doc: DocumentMut = content.parse().unwrap();
    }

    #[test]
    fn test_create_duplicate_fails() {
        let (_dir, mgr) = setup(base_config());
        let def = AgentDefinition {
            id: "main".to_string(),
            ..Default::default()
        };

        let err = mgr.create(def).unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn test_create_invalid_id_empty() {
        let (_dir, mgr) = setup(base_config());
        let def = AgentDefinition {
            id: "".to_string(),
            ..Default::default()
        };
        let err = mgr.create(def).unwrap_err();
        assert!(err.to_string().contains("1-32"));
    }

    #[test]
    fn test_create_invalid_id_too_long() {
        let (_dir, mgr) = setup(base_config());
        let def = AgentDefinition {
            id: "a".repeat(33),
            ..Default::default()
        };
        let err = mgr.create(def).unwrap_err();
        assert!(err.to_string().contains("1-32"));
    }

    #[test]
    fn test_create_invalid_id_special_chars() {
        let (_dir, mgr) = setup(base_config());
        let def = AgentDefinition {
            id: "agent/evil".to_string(),
            ..Default::default()
        };
        let err = mgr.create(def).unwrap_err();
        assert!(err.to_string().contains("invalid characters"));
    }

    #[test]
    fn test_create_agent_with_all_fields() {
        let (_dir, mgr) = setup(base_config());
        let def = AgentDefinition {
            id: "full-agent".to_string(),
            name: Some("Full Agent".to_string()),
            default: false,
            identity: Some(AgentIdentity {
                emoji: Some("🤖".to_string()),
                description: Some("A full agent".to_string()),
                avatar: None,
                theme: Some("dark".to_string()),
            }),
            model_config: Some(AgentModelConfig {
                primary: "claude-opus-4".to_string(),
                fallbacks: vec!["gpt-4o".to_string()],
            }),
            params: Some(AgentParams {
                temperature: Some(0.7),
                max_tokens: Some(4096),
                top_p: None,
                top_k: None,
            }),
            skills: Some(vec!["code".to_string(), "search".to_string()]),
            subagents: Some(SubagentPolicy {
                allow: vec!["helper".to_string()],
            }),
            ..Default::default()
        };

        mgr.create(def).unwrap();

        // Re-read and verify
        let agent = mgr.get("full-agent").unwrap();
        assert_eq!(agent.name, Some("Full Agent".to_string()));
        assert!(agent.identity.is_some());
        assert_eq!(
            agent.identity.as_ref().unwrap().emoji,
            Some("🤖".to_string())
        );
        assert!(agent.model_config.is_some());
        assert_eq!(
            agent.model_config.as_ref().unwrap().primary,
            "claude-opus-4"
        );
        assert!(agent.params.is_some());
        // f32 -> f64 conversion may lose precision, check approximately
        let temp = agent.params.as_ref().unwrap().temperature.unwrap();
        assert!((temp - 0.7).abs() < 0.01);
        assert_eq!(agent.params.as_ref().unwrap().max_tokens, Some(4096));
        assert_eq!(
            agent.skills,
            Some(vec!["code".to_string(), "search".to_string()])
        );
        assert!(agent.subagents.is_some());
        assert_eq!(
            agent.subagents.as_ref().unwrap().allow,
            vec!["helper"]
        );
    }

    #[test]
    fn test_create_creates_both_directories() {
        let (_dir, mgr) = setup(base_config());
        let def = AgentDefinition {
            id: "dual".to_string(),
            name: Some("Dual Agent".to_string()),
            ..Default::default()
        };

        mgr.create(def).unwrap();

        // Workspace content dir
        assert!(mgr.workspace_root.join("dual").join("SOUL.md").exists());
        assert!(mgr.workspace_root.join("dual").join("memory").is_dir());

        // Agent state dir
        assert!(mgr.agents_root.join("dual").join("sessions").is_dir());

        // sessions/ should NOT be in workspace
        assert!(!mgr.workspace_root.join("dual").join("sessions").exists());
    }

    #[test]
    fn test_delete_trashes_both_directories() {
        let (_dir, mgr) = setup(base_config());

        // Pre-create both dirs for coder
        fs::create_dir_all(mgr.workspace_root.join("coder")).unwrap();
        fs::write(mgr.workspace_root.join("coder").join("SOUL.md"), "test").unwrap();
        fs::create_dir_all(mgr.agents_root.join("coder").join("sessions")).unwrap();

        mgr.delete("coder").unwrap();

        assert!(!mgr.workspace_root.join("coder").exists());
        assert!(!mgr.agents_root.join("coder").exists());
    }

    // =========================================================================
    // Update
    // =========================================================================

    #[test]
    fn test_update_agent() {
        let (_dir, mgr) = setup(base_config());

        let patch = AgentPatch {
            name: Some("Updated Coder".to_string()),
            params: Some(AgentParams {
                temperature: Some(0.5),
                max_tokens: Some(2048),
                ..Default::default()
            }),
            skills: Some(vec!["git".to_string(), "rust".to_string()]),
            ..Default::default()
        };

        mgr.update("coder", patch).unwrap();

        let agent = mgr.get("coder").unwrap();
        assert_eq!(agent.name, Some("Updated Coder".to_string()));
        assert!(agent.params.is_some());
        let temp = agent.params.as_ref().unwrap().temperature.unwrap();
        assert!((temp - 0.5).abs() < 0.01);
        assert_eq!(agent.params.as_ref().unwrap().max_tokens, Some(2048));
        assert_eq!(
            agent.skills,
            Some(vec!["git".to_string(), "rust".to_string()])
        );
    }

    #[test]
    fn test_update_nonexistent_fails() {
        let (_dir, mgr) = setup(base_config());
        let patch = AgentPatch {
            name: Some("Ghost".to_string()),
            ..Default::default()
        };
        let err = mgr.update("ghost", patch).unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    // =========================================================================
    // Delete
    // =========================================================================

    #[test]
    fn test_delete_agent() {
        let (_dir, mgr) = setup(base_config());

        // Create workspace for coder
        let ws_dir = mgr.workspace_root.join("coder");
        fs::create_dir_all(&ws_dir).unwrap();
        fs::write(ws_dir.join("test.txt"), "hello").unwrap();

        mgr.delete("coder").unwrap();

        // Verify removed from list
        let agents = mgr.list().unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].id, "main");

        // Verify workspace moved to trash
        assert!(!ws_dir.exists());
        let trash_entries: Vec<_> = fs::read_dir(&mgr.trash_root)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(trash_entries.len(), 1);
        let trash_name = trash_entries[0].file_name().to_string_lossy().to_string();
        assert!(trash_name.starts_with("coder_"));
    }

    #[test]
    fn test_delete_only_agent_fails() {
        let config = r#"
[agents]

[[agents.list]]
id = "solo"
default = true
name = "Solo Agent"
"#;
        let (_dir, mgr) = setup(config);
        let err = mgr.delete("solo").unwrap_err();
        assert!(err.to_string().contains("only agent"));
    }

    #[test]
    fn test_delete_default_agent_fails() {
        let (_dir, mgr) = setup(base_config());
        let err = mgr.delete("main").unwrap_err();
        assert!(err.to_string().contains("default agent"));
    }

    #[test]
    fn test_delete_nonexistent_fails() {
        let (_dir, mgr) = setup(base_config());
        let err = mgr.delete("ghost").unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    // =========================================================================
    // Set Default
    // =========================================================================

    #[test]
    fn test_set_default() {
        let (_dir, mgr) = setup(base_config());

        mgr.set_default("coder").unwrap();

        let agents = mgr.list().unwrap();
        let main = agents.iter().find(|a| a.id == "main").unwrap();
        let coder = agents.iter().find(|a| a.id == "coder").unwrap();
        assert!(!main.default);
        assert!(coder.default);
    }

    #[test]
    fn test_set_default_nonexistent_fails() {
        let (_dir, mgr) = setup(base_config());
        let err = mgr.set_default("ghost").unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    // =========================================================================
    // Workspace file operations
    // =========================================================================

    #[test]
    fn test_workspace_file_operations() {
        let (_dir, mgr) = setup(base_config());

        // Write a file
        mgr.write_file("main", "test.md", "# Test\nHello world")
            .unwrap();

        // List files
        let files = mgr.list_files("main").unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "test.md");
        assert!(!files[0].is_bootstrap);
        assert!(files[0].size_bytes > 0);

        // Read file
        let content = mgr.read_file("main", "test.md").unwrap();
        assert_eq!(content, "# Test\nHello world");

        // Delete file
        mgr.delete_file("main", "test.md").unwrap();
        let files = mgr.list_files("main").unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn test_workspace_bootstrap_file_detection() {
        let (_dir, mgr) = setup(base_config());

        mgr.write_file("main", "SOUL.md", "# Soul").unwrap();
        mgr.write_file("main", "custom.md", "# Custom").unwrap();

        let files = mgr.list_files("main").unwrap();
        assert_eq!(files.len(), 2);

        let soul = files.iter().find(|f| f.filename == "SOUL.md").unwrap();
        assert!(soul.is_bootstrap);

        let custom = files.iter().find(|f| f.filename == "custom.md").unwrap();
        assert!(!custom.is_bootstrap);
    }

    #[test]
    fn test_workspace_list_nonexistent_returns_empty() {
        let (_dir, mgr) = setup(base_config());
        let files = mgr.list_files("nonexistent").unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn test_filename_path_traversal_blocked() {
        let (_dir, mgr) = setup(base_config());

        // Various path traversal attempts
        assert!(mgr.read_file("main", "../secret").is_err());
        assert!(mgr.read_file("main", "foo/bar").is_err());
        assert!(mgr.read_file("main", "foo\\bar").is_err());
        assert!(mgr.read_file("main", "..").is_err());

        assert!(mgr.write_file("main", "../evil", "pwned").is_err());
        assert!(mgr.write_file("main", "a/b", "pwned").is_err());

        assert!(mgr.delete_file("main", "../gone").is_err());
    }

    // =========================================================================
    // Validate ID
    // =========================================================================

    #[test]
    fn test_validate_id_valid() {
        let (_dir, mgr) = setup(base_config());
        assert!(mgr.validate_id("my-agent").is_ok());
        assert!(mgr.validate_id("agent_1").is_ok());
        assert!(mgr.validate_id("a").is_ok());
        assert!(mgr.validate_id("Agent-X_99").is_ok());
    }

    #[test]
    fn test_validate_id_invalid() {
        let (_dir, mgr) = setup(base_config());
        assert!(mgr.validate_id("").is_err());
        assert!(mgr.validate_id("has space").is_err());
        assert!(mgr.validate_id("has.dot").is_err());
        assert!(mgr.validate_id("has/slash").is_err());
        assert!(mgr.validate_id(&"x".repeat(33)).is_err());
    }

    // =========================================================================
    // Empty config edge case
    // =========================================================================

    #[test]
    fn test_empty_config_create_first_agent() {
        let (_dir, mgr) = setup("");
        let def = AgentDefinition {
            id: "first".to_string(),
            default: true,
            name: Some("First Agent".to_string()),
            ..Default::default()
        };

        mgr.create(def).unwrap();

        let agents = mgr.list().unwrap();
        assert_eq!(agents.len(), 2); // "main" auto-created + "first"
        assert!(agents.iter().any(|a| a.id == "main"), "auto-created main agent");
        assert!(agents.iter().any(|a| a.id == "first"), "explicitly created first agent");
    }

    // =========================================================================
    // TOML format preservation
    // =========================================================================

    #[test]
    fn test_toml_roundtrip_preserves_other_sections() {
        let config = r#"
[general]
language = "zh"

[agents]

[[agents.list]]
id = "main"
default = true
name = "Main Agent"

[memory]
enabled = true
"#;
        let (_dir, mgr) = setup(config);

        // Create a new agent
        let def = AgentDefinition {
            id: "new-one".to_string(),
            name: Some("New".to_string()),
            ..Default::default()
        };
        mgr.create(def).unwrap();

        // Verify other sections are preserved
        let content = fs::read_to_string(&mgr.config_path).unwrap();
        assert!(content.contains("[general]"));
        assert!(content.contains("language = \"zh\""));
        assert!(content.contains("[memory]"));
        assert!(content.contains("enabled = true"));
    }
}
