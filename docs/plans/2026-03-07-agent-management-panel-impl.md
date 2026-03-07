# Agent Management Panel Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add full Agent CRUD management as a top-level Panel mode with 6-tab detail view, backed by an AgentManager service and Gateway RPC layer.

**Architecture:** AgentManager (toml_edit CRUD + workspace + trash) → agents.* RPC handlers → Panel Leptos/WASM components. Agent becomes the 4th PanelMode alongside Chat/Dashboard/Settings.

**Tech Stack:** Rust, toml_edit, Leptos/WASM, JSON-RPC 2.0, tokio

**Design doc:** `docs/plans/2026-03-07-agent-management-panel-design.md`

---

## Task 1: Add `toml_edit` dependency

**Files:**
- Modify: `core/Cargo.toml`

**Step 1: Add toml_edit to dependencies**

In `core/Cargo.toml`, add after the `toml = "0.8"` line (around line 49):

```toml
toml_edit = "0.22"
```

**Step 2: Verify it compiles**

Run: `cd core && cargo check -p alephcore 2>&1 | tail -5`
Expected: compilation success (or only pre-existing warnings)

**Step 3: Commit**

```bash
git add core/Cargo.toml core/Cargo.lock
git commit -m "deps: add toml_edit for agent config CRUD"
```

---

## Task 2: Extend AgentDefinition with new value objects

**Files:**
- Modify: `core/src/config/types/agents_def.rs`

**Step 1: Write tests for new types**

Add these tests to the existing `#[cfg(test)] mod tests` block at the bottom of `agents_def.rs`:

```rust
#[test]
fn test_agent_identity_deserialize() {
    let toml_str = r#"
        emoji = "🧑‍💻"
        description = "Full-stack coding specialist"
        avatar = "https://example.com/avatar.png"
        theme = "Write clean code"
    "#;
    let identity: AgentIdentity = toml::from_str(toml_str).unwrap();
    assert_eq!(identity.emoji, Some("🧑‍💻".to_string()));
    assert_eq!(identity.description, Some("Full-stack coding specialist".to_string()));
    assert_eq!(identity.avatar, Some("https://example.com/avatar.png".to_string()));
    assert_eq!(identity.theme, Some("Write clean code".to_string()));
}

#[test]
fn test_agent_model_config_deserialize() {
    let toml_str = r#"
        primary = "claude-opus-4"
        fallbacks = ["claude-sonnet-4", "gpt-4o"]
    "#;
    let mc: AgentModelConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(mc.primary, "claude-opus-4");
    assert_eq!(mc.fallbacks, vec!["claude-sonnet-4", "gpt-4o"]);
}

#[test]
fn test_agent_params_deserialize() {
    let toml_str = r#"
        temperature = 0.3
        max_tokens = 8192
    "#;
    let params: AgentParams = toml::from_str(toml_str).unwrap();
    assert_eq!(params.temperature, Some(0.3));
    assert_eq!(params.max_tokens, Some(8192));
    assert!(params.top_p.is_none());
    assert!(params.top_k.is_none());
}

#[test]
fn test_agent_definition_with_new_fields() {
    let toml_str = r#"
        [[list]]
        id = "coder"
        name = "Code Master"
        default = true

        [list.identity]
        emoji = "🧑‍💻"
        description = "Full-stack coding specialist"

        [list.model_config]
        primary = "claude-opus-4"
        fallbacks = ["claude-sonnet-4"]

        [list.params]
        temperature = 0.3
        max_tokens = 8192
    "#;
    let config: AgentsConfig = toml::from_str(toml_str).unwrap();
    let agent = &config.list[0];
    assert_eq!(agent.id, "coder");
    assert!(agent.identity.is_some());
    assert_eq!(agent.identity.as_ref().unwrap().emoji, Some("🧑‍💻".to_string()));
    assert!(agent.model_config.is_some());
    assert_eq!(agent.model_config.as_ref().unwrap().primary, "claude-opus-4");
    assert!(agent.params.is_some());
    assert_eq!(agent.params.as_ref().unwrap().temperature, Some(0.3));
}

#[test]
fn test_backward_compat_model_field() {
    let toml_str = r#"
        [[list]]
        id = "legacy"
        model = "claude-sonnet-4"
    "#;
    let config: AgentsConfig = toml::from_str(toml_str).unwrap();
    let agent = &config.list[0];
    assert_eq!(agent.model, Some("claude-sonnet-4".to_string()));
    assert!(agent.model_config.is_none());
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib config::types::agents_def -- --nocapture 2>&1 | tail -20`
Expected: FAIL — `AgentIdentity`, `AgentModelConfig`, `AgentParams` not defined yet

**Step 3: Add the new types and fields**

Add these structs BEFORE `AgentDefinition` (after `AgentDefaults`):

```rust
// =============================================================================
// AgentIdentity
// =============================================================================

/// Agent identity for display purposes
///
/// Controls how the agent appears in the UI — emoji, description, avatar, theme.
///
/// # Example TOML
/// ```toml
/// [agents.list.identity]
/// emoji = "🧑‍💻"
/// description = "Full-stack coding specialist"
/// theme = "Write clean, efficient code"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AgentIdentity {
    /// Emoji displayed as agent avatar
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,

    /// One-line description of agent's role
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Avatar image URL or base64 data
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,

    /// Tagline or theme color
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
}

// =============================================================================
// AgentModelConfig
// =============================================================================

/// Model configuration with fallback chain
///
/// # Example TOML
/// ```toml
/// [agents.list.model_config]
/// primary = "claude-opus-4"
/// fallbacks = ["claude-sonnet-4", "gpt-4o"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentModelConfig {
    /// Primary model ID
    pub primary: String,

    /// Fallback model chain (tried in order if primary fails)
    #[serde(default)]
    pub fallbacks: Vec<String>,
}

// =============================================================================
// AgentParams
// =============================================================================

/// Per-agent inference parameters
///
/// # Example TOML
/// ```toml
/// [agents.list.params]
/// temperature = 0.3
/// max_tokens = 8192
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AgentParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
}
```

Then add new fields to `AgentDefinition` (after `skills` field, before `subagents`):

```rust
    /// Agent identity (emoji, description, avatar, theme)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<AgentIdentity>,

    /// Model configuration with fallback chain (overrides `model` field)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_config: Option<AgentModelConfig>,

    /// Per-agent inference parameters
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<AgentParams>,
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib config::types::agents_def -- --nocapture 2>&1 | tail -20`
Expected: ALL PASS (both new and existing tests)

**Step 5: Run full core check**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: compilation success

**Step 6: Commit**

```bash
git add core/src/config/types/agents_def.rs
git commit -m "config: extend AgentDefinition with identity, model_config, params"
```

---

## Task 3: Implement AgentManager — TOML CRUD

**Files:**
- Create: `core/src/config/agent_manager.rs`
- Modify: `core/src/config/mod.rs` (add `pub mod agent_manager;`)

**Step 1: Write tests first**

Create `core/src/config/agent_manager.rs` with the test module at the bottom. Tests use `tempfile` for isolated config files.

```rust
//! Agent Manager — TOML-based CRUD for agent definitions
//!
//! Single entry point for creating, reading, updating, and deleting agents.
//! Uses `toml_edit` for precision edits that preserve comments and formatting.
//! Manages agent workspaces and trash directories.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, value, Array, Item, Table, InlineTable};
use tracing::{debug, info, warn};

use crate::config::types::agents_def::{
    AgentDefinition, AgentIdentity, AgentModelConfig, AgentParams, AgentsConfig, SubagentPolicy,
};
use crate::error::{AlephError, Result};

/// Patch struct for partial agent updates
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<AgentIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_config: Option<AgentModelConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<AgentParams>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagents: Option<SubagentPolicy>,
}

/// Workspace file metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceFile {
    pub filename: String,
    pub size_bytes: u64,
    pub modified_at: i64,
    pub is_bootstrap: bool,
}

/// Bootstrap file names recognized by Aleph
const BOOTSTRAP_FILES: &[&str] = &["SOUL.md", "AGENTS.md", "MEMORY.md", "TOOLS.md", "IDENTITY.md"];

/// Agent Manager for TOML-based CRUD operations
pub struct AgentManager {
    config_path: PathBuf,
    workspace_root: PathBuf,
    trash_root: PathBuf,
}

impl AgentManager {
    /// Create a new AgentManager
    pub fn new(config_path: PathBuf, workspace_root: PathBuf, trash_root: PathBuf) -> Self {
        Self { config_path, workspace_root, trash_root }
    }

    // ── CRUD ────────────────────────────────────────────────────────────

    /// List all agent definitions
    pub fn list(&self) -> Result<Vec<AgentDefinition>> {
        let config = self.load_config()?;
        Ok(config.list)
    }

    /// Get a single agent definition by ID
    pub fn get(&self, id: &str) -> Result<AgentDefinition> {
        let config = self.load_config()?;
        config.list.into_iter()
            .find(|a| a.id == id)
            .ok_or_else(|| AlephError::invalid_config(format!("Agent '{}' not found", id)))
    }

    /// Create a new agent
    pub fn create(&self, def: AgentDefinition) -> Result<()> {
        // Validate ID format
        Self::validate_id(&def.id)?;

        // Check uniqueness
        let existing = self.load_config()?;
        if existing.list.iter().any(|a| a.id == def.id) {
            return Err(AlephError::invalid_config(format!(
                "Agent '{}' already exists", def.id
            )));
        }

        // Append to TOML via toml_edit
        let mut doc = self.load_document()?;
        self.append_agent_to_document(&mut doc, &def)?;
        self.save_document(&doc)?;

        // Create workspace directory
        let workspace = self.workspace_root.join(&def.id);
        fs::create_dir_all(&workspace).map_err(|e| {
            AlephError::invalid_config(format!("Failed to create workspace: {}", e))
        })?;

        // Initialize default SOUL.md
        let soul_path = workspace.join("SOUL.md");
        if !soul_path.exists() {
            let content = format!("# {}\n\nDescribe this agent's personality and behavior.\n",
                def.name.as_deref().unwrap_or(&def.id));
            fs::write(&soul_path, content).map_err(|e| {
                AlephError::invalid_config(format!("Failed to write SOUL.md: {}", e))
            })?;
        }

        info!(agent_id = %def.id, "Agent created");
        Ok(())
    }

    /// Update an existing agent with a patch
    pub fn update(&self, id: &str, patch: AgentPatch) -> Result<()> {
        // Verify agent exists
        let _ = self.get(id)?;

        // Load and modify the TOML document
        let mut doc = self.load_document()?;
        let idx = self.find_agent_index(&doc, id)
            .ok_or_else(|| AlephError::invalid_config(format!("Agent '{}' not found in TOML", id)))?;

        let agents_array = doc["agents"]["list"].as_array_of_tables_mut()
            .ok_or_else(|| AlephError::invalid_config("agents.list is not an array of tables"))?;

        let table = &mut agents_array[idx];

        if let Some(name) = &patch.name {
            table["name"] = value(name.as_str());
        }
        if let Some(identity) = &patch.identity {
            let mut id_table = Table::new();
            if let Some(emoji) = &identity.emoji {
                id_table["emoji"] = value(emoji.as_str());
            }
            if let Some(desc) = &identity.description {
                id_table["description"] = value(desc.as_str());
            }
            if let Some(avatar) = &identity.avatar {
                id_table["avatar"] = value(avatar.as_str());
            }
            if let Some(theme) = &identity.theme {
                id_table["theme"] = value(theme.as_str());
            }
            table["identity"] = Item::Table(id_table);
        }
        if let Some(mc) = &patch.model_config {
            let mut mc_table = Table::new();
            mc_table["primary"] = value(mc.primary.as_str());
            let mut fallbacks = Array::new();
            for f in &mc.fallbacks {
                fallbacks.push(f.as_str());
            }
            mc_table["fallbacks"] = value(fallbacks);
            table["model_config"] = Item::Table(mc_table);
        }
        if let Some(params) = &patch.params {
            let mut p_table = Table::new();
            if let Some(t) = params.temperature {
                p_table["temperature"] = value(t as f64);
            }
            if let Some(m) = params.max_tokens {
                p_table["max_tokens"] = value(m as i64);
            }
            if let Some(p) = params.top_p {
                p_table["top_p"] = value(p as f64);
            }
            if let Some(k) = params.top_k {
                p_table["top_k"] = value(k as i64);
            }
            table["params"] = Item::Table(p_table);
        }
        if let Some(skills) = &patch.skills {
            let mut arr = Array::new();
            for s in skills {
                arr.push(s.as_str());
            }
            table["skills"] = value(arr);
        }

        self.save_document(&doc)?;
        info!(agent_id = %id, "Agent updated");
        Ok(())
    }

    /// Delete an agent (move workspace to trash)
    pub fn delete(&self, id: &str) -> Result<()> {
        let config = self.load_config()?;

        // Must have at least 2 agents
        if config.list.len() <= 1 {
            return Err(AlephError::invalid_config("Cannot delete the only agent"));
        }

        // Cannot delete default agent
        let agent = config.list.iter().find(|a| a.id == id)
            .ok_or_else(|| AlephError::invalid_config(format!("Agent '{}' not found", id)))?;
        if agent.default {
            return Err(AlephError::invalid_config(
                "Cannot delete the default agent. Switch default first."
            ));
        }

        // Remove from TOML
        let mut doc = self.load_document()?;
        let idx = self.find_agent_index(&doc, id)
            .ok_or_else(|| AlephError::invalid_config(format!("Agent '{}' not found in TOML", id)))?;

        let agents_array = doc["agents"]["list"].as_array_of_tables_mut()
            .ok_or_else(|| AlephError::invalid_config("agents.list is not an array of tables"))?;
        agents_array.remove(idx);

        self.save_document(&doc)?;

        // Move workspace to trash
        let workspace = self.workspace_root.join(id);
        if workspace.exists() {
            let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
            let trash_dest = self.trash_root.join(format!("{}_{}", id, timestamp));
            fs::create_dir_all(&self.trash_root).map_err(|e| {
                AlephError::invalid_config(format!("Failed to create trash dir: {}", e))
            })?;
            fs::rename(&workspace, &trash_dest).map_err(|e| {
                warn!(error = %e, "Failed to move workspace to trash, attempting copy");
                AlephError::invalid_config(format!("Failed to trash workspace: {}", e))
            })?;
            info!(agent_id = %id, trash = %trash_dest.display(), "Workspace moved to trash");
        }

        info!(agent_id = %id, "Agent deleted");
        Ok(())
    }

    /// Set default agent
    pub fn set_default(&self, id: &str) -> Result<()> {
        // Verify target exists
        let _ = self.get(id)?;

        let mut doc = self.load_document()?;
        let agents_array = doc["agents"]["list"].as_array_of_tables_mut()
            .ok_or_else(|| AlephError::invalid_config("agents.list is not an array of tables"))?;

        for i in 0..agents_array.len() {
            let table = &mut agents_array[i];
            let is_target = table.get("id")
                .and_then(|v| v.as_str())
                .map(|s| s == id)
                .unwrap_or(false);
            table["default"] = value(is_target);
        }

        self.save_document(&doc)?;
        info!(agent_id = %id, "Default agent changed");
        Ok(())
    }

    // ── Workspace Files ─────────────────────────────────────────────────

    /// List files in an agent's workspace
    pub fn list_files(&self, agent_id: &str) -> Result<Vec<WorkspaceFile>> {
        let workspace = self.workspace_root.join(agent_id);
        if !workspace.exists() {
            return Ok(vec![]);
        }

        let mut files = Vec::new();
        let entries = fs::read_dir(&workspace).map_err(|e| {
            AlephError::invalid_config(format!("Failed to read workspace: {}", e))
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| {
                AlephError::invalid_config(format!("Failed to read entry: {}", e))
            })?;
            let metadata = entry.metadata().map_err(|e| {
                AlephError::invalid_config(format!("Failed to read metadata: {}", e))
            })?;

            if metadata.is_file() {
                let filename = entry.file_name().to_string_lossy().to_string();
                let modified_at = metadata.modified()
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
        }

        files.sort_by(|a, b| a.filename.cmp(&b.filename));
        Ok(files)
    }

    /// Read a workspace file
    pub fn read_file(&self, agent_id: &str, filename: &str) -> Result<String> {
        Self::validate_filename(filename)?;
        let path = self.workspace_root.join(agent_id).join(filename);
        fs::read_to_string(&path).map_err(|e| {
            AlephError::invalid_config(format!("Failed to read '{}': {}", filename, e))
        })
    }

    /// Write a workspace file
    pub fn write_file(&self, agent_id: &str, filename: &str, content: &str) -> Result<()> {
        Self::validate_filename(filename)?;
        let workspace = self.workspace_root.join(agent_id);
        fs::create_dir_all(&workspace).map_err(|e| {
            AlephError::invalid_config(format!("Failed to create workspace: {}", e))
        })?;
        let path = workspace.join(filename);
        fs::write(&path, content).map_err(|e| {
            AlephError::invalid_config(format!("Failed to write '{}': {}", filename, e))
        })?;
        Ok(())
    }

    /// Delete a workspace file
    pub fn delete_file(&self, agent_id: &str, filename: &str) -> Result<()> {
        Self::validate_filename(filename)?;
        let path = self.workspace_root.join(agent_id).join(filename);
        if path.exists() {
            fs::remove_file(&path).map_err(|e| {
                AlephError::invalid_config(format!("Failed to delete '{}': {}", filename, e))
            })?;
        }
        Ok(())
    }

    // ── Internal ────────────────────────────────────────────────────────

    fn load_config(&self) -> Result<AgentsConfig> {
        let contents = fs::read_to_string(&self.config_path).map_err(|e| {
            AlephError::invalid_config(format!("Failed to read config: {}", e))
        })?;
        let full: toml::Value = toml::from_str(&contents).map_err(|e| {
            AlephError::invalid_config(format!("Failed to parse config: {}", e))
        })?;
        let agents_value = full.get("agents").cloned().unwrap_or(toml::Value::Table(Default::default()));
        let config: AgentsConfig = agents_value.try_into().map_err(|e| {
            AlephError::invalid_config(format!("Failed to parse agents config: {}", e))
        })?;
        Ok(config)
    }

    fn load_document(&self) -> Result<DocumentMut> {
        let contents = fs::read_to_string(&self.config_path).map_err(|e| {
            AlephError::invalid_config(format!("Failed to read config: {}", e))
        })?;
        contents.parse::<DocumentMut>().map_err(|e| {
            AlephError::invalid_config(format!("Failed to parse TOML document: {}", e))
        })
    }

    fn save_document(&self, doc: &DocumentMut) -> Result<()> {
        let contents = doc.to_string();
        let temp_path = self.config_path.with_extension("tmp");

        fs::write(&temp_path, &contents).map_err(|e| {
            AlephError::invalid_config(format!("Failed to write temp file: {}", e))
        })?;

        #[cfg(unix)]
        {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(&temp_path)
                .map_err(|e| AlephError::invalid_config(format!("fsync open failed: {}", e)))?;
            file.sync_all()
                .map_err(|e| AlephError::invalid_config(format!("fsync failed: {}", e)))?;
        }

        fs::rename(&temp_path, &self.config_path).map_err(|e| {
            let _ = fs::remove_file(&temp_path);
            AlephError::invalid_config(format!("Atomic rename failed: {}", e))
        })?;

        debug!(path = %self.config_path.display(), "Config saved via toml_edit");
        Ok(())
    }

    fn find_agent_index(&self, doc: &DocumentMut, id: &str) -> Option<usize> {
        let agents_array = doc.get("agents")?.get("list")?.as_array_of_tables()?;
        agents_array.iter().position(|table| {
            table.get("id").and_then(|v| v.as_str()) == Some(id)
        })
    }

    fn append_agent_to_document(&self, doc: &mut DocumentMut, def: &AgentDefinition) -> Result<()> {
        // Ensure [agents] table exists
        if !doc.contains_key("agents") {
            doc["agents"] = toml_edit::Item::Table(Table::new());
        }

        // Ensure [[agents.list]] array exists
        let agents = doc["agents"].as_table_mut()
            .ok_or_else(|| AlephError::invalid_config("[agents] is not a table"))?;

        if !agents.contains_key("list") {
            agents.insert("list", toml_edit::Item::ArrayOfTables(Default::default()));
        }

        let list = agents["list"].as_array_of_tables_mut()
            .ok_or_else(|| AlephError::invalid_config("agents.list is not an array of tables"))?;

        let mut table = Table::new();
        table["id"] = value(&def.id);
        if def.default {
            table["default"] = value(true);
        }
        if let Some(name) = &def.name {
            table["name"] = value(name.as_str());
        }
        if let Some(identity) = &def.identity {
            let mut id_table = Table::new();
            if let Some(emoji) = &identity.emoji {
                id_table["emoji"] = value(emoji.as_str());
            }
            if let Some(desc) = &identity.description {
                id_table["description"] = value(desc.as_str());
            }
            if let Some(avatar) = &identity.avatar {
                id_table["avatar"] = value(avatar.as_str());
            }
            if let Some(theme) = &identity.theme {
                id_table["theme"] = value(theme.as_str());
            }
            table["identity"] = Item::Table(id_table);
        }
        if let Some(mc) = &def.model_config {
            let mut mc_table = Table::new();
            mc_table["primary"] = value(mc.primary.as_str());
            let mut fallbacks = Array::new();
            for f in &mc.fallbacks {
                fallbacks.push(f.as_str());
            }
            mc_table["fallbacks"] = value(fallbacks);
            table["model_config"] = Item::Table(mc_table);
        }
        if let Some(params) = &def.params {
            let mut p_table = Table::new();
            if let Some(t) = params.temperature {
                p_table["temperature"] = value(t as f64);
            }
            if let Some(m) = params.max_tokens {
                p_table["max_tokens"] = value(m as i64);
            }
            if let Some(p) = params.top_p {
                p_table["top_p"] = value(p as f64);
            }
            if let Some(k) = params.top_k {
                p_table["top_k"] = value(k as i64);
            }
            table["params"] = Item::Table(p_table);
        }
        if let Some(skills) = &def.skills {
            let mut arr = Array::new();
            for s in skills {
                arr.push(s.as_str());
            }
            table["skills"] = value(arr);
        }

        list.push(table);
        Ok(())
    }

    fn validate_id(id: &str) -> Result<()> {
        if id.is_empty() || id.len() > 32 {
            return Err(AlephError::invalid_config(
                "Agent ID must be 1-32 characters"
            ));
        }
        if !id.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
            return Err(AlephError::invalid_config(
                "Agent ID must be alphanumeric, hyphens, or underscores"
            ));
        }
        Ok(())
    }

    fn validate_filename(filename: &str) -> Result<()> {
        if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
            return Err(AlephError::invalid_config(
                "Filename must not contain path separators or '..'"
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> (TempDir, AgentManager) {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        let workspace_root = dir.path().join("agents");
        let trash_root = dir.path().join("trash");
        fs::create_dir_all(&workspace_root).unwrap();

        // Write initial config with one agent
        fs::write(&config_path, r#"
[agents.defaults]
model = "claude-sonnet-4"

[[agents.list]]
id = "main"
default = true
name = "Main Agent"
"#).unwrap();

        let manager = AgentManager::new(config_path, workspace_root, trash_root);
        (dir, manager)
    }

    #[test]
    fn test_list_agents() {
        let (_dir, manager) = setup();
        let agents = manager.list().unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].id, "main");
    }

    #[test]
    fn test_get_agent() {
        let (_dir, manager) = setup();
        let agent = manager.get("main").unwrap();
        assert_eq!(agent.id, "main");
        assert!(agent.default);
    }

    #[test]
    fn test_get_agent_not_found() {
        let (_dir, manager) = setup();
        assert!(manager.get("nonexistent").is_err());
    }

    #[test]
    fn test_create_agent() {
        let (_dir, manager) = setup();
        let def = AgentDefinition {
            id: "coder".to_string(),
            name: Some("Coder".to_string()),
            identity: Some(AgentIdentity {
                emoji: Some("🧑‍💻".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        manager.create(def).unwrap();

        let agents = manager.list().unwrap();
        assert_eq!(agents.len(), 2);

        let coder = manager.get("coder").unwrap();
        assert_eq!(coder.name, Some("Coder".to_string()));
        assert_eq!(coder.identity.as_ref().unwrap().emoji, Some("🧑‍💻".to_string()));

        // Workspace created
        assert!(manager.workspace_root.join("coder").exists());
        // SOUL.md initialized
        assert!(manager.workspace_root.join("coder").join("SOUL.md").exists());
    }

    #[test]
    fn test_create_duplicate_fails() {
        let (_dir, manager) = setup();
        let def = AgentDefinition {
            id: "main".to_string(),
            ..Default::default()
        };
        assert!(manager.create(def).is_err());
    }

    #[test]
    fn test_create_invalid_id() {
        let (_dir, manager) = setup();
        let def = AgentDefinition {
            id: "bad id!".to_string(),
            ..Default::default()
        };
        assert!(manager.create(def).is_err());
    }

    #[test]
    fn test_update_agent() {
        let (_dir, manager) = setup();
        let patch = AgentPatch {
            name: Some("Updated Main".to_string()),
            identity: Some(AgentIdentity {
                emoji: Some("🤖".to_string()),
                description: Some("Updated description".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        manager.update("main", patch).unwrap();

        let agent = manager.get("main").unwrap();
        assert_eq!(agent.name, Some("Updated Main".to_string()));
        assert_eq!(agent.identity.as_ref().unwrap().emoji, Some("🤖".to_string()));
    }

    #[test]
    fn test_delete_agent() {
        let (_dir, manager) = setup();
        // Create a second agent first
        manager.create(AgentDefinition {
            id: "temp".to_string(),
            ..Default::default()
        }).unwrap();

        manager.delete("temp").unwrap();
        assert_eq!(manager.list().unwrap().len(), 1);

        // Workspace moved to trash
        assert!(!manager.workspace_root.join("temp").exists());
    }

    #[test]
    fn test_delete_only_agent_fails() {
        let (_dir, manager) = setup();
        assert!(manager.delete("main").is_err());
    }

    #[test]
    fn test_delete_default_agent_fails() {
        let (_dir, manager) = setup();
        manager.create(AgentDefinition {
            id: "other".to_string(),
            ..Default::default()
        }).unwrap();
        assert!(manager.delete("main").is_err());
    }

    #[test]
    fn test_set_default() {
        let (_dir, manager) = setup();
        manager.create(AgentDefinition {
            id: "coder".to_string(),
            ..Default::default()
        }).unwrap();

        manager.set_default("coder").unwrap();

        let agents = manager.list().unwrap();
        let main = agents.iter().find(|a| a.id == "main").unwrap();
        let coder = agents.iter().find(|a| a.id == "coder").unwrap();
        assert!(!main.default);
        assert!(coder.default);
    }

    #[test]
    fn test_workspace_files() {
        let (_dir, manager) = setup();
        // Create workspace with a file
        let ws = manager.workspace_root.join("main");
        fs::create_dir_all(&ws).unwrap();
        fs::write(ws.join("SOUL.md"), "# Main Agent").unwrap();
        fs::write(ws.join("notes.txt"), "some notes").unwrap();

        let files = manager.list_files("main").unwrap();
        assert_eq!(files.len(), 2);

        let soul = files.iter().find(|f| f.filename == "SOUL.md").unwrap();
        assert!(soul.is_bootstrap);

        let notes = files.iter().find(|f| f.filename == "notes.txt").unwrap();
        assert!(!notes.is_bootstrap);
    }

    #[test]
    fn test_read_write_delete_file() {
        let (_dir, manager) = setup();
        let ws = manager.workspace_root.join("main");
        fs::create_dir_all(&ws).unwrap();

        manager.write_file("main", "test.md", "hello").unwrap();
        let content = manager.read_file("main", "test.md").unwrap();
        assert_eq!(content, "hello");

        manager.delete_file("main", "test.md").unwrap();
        assert!(manager.read_file("main", "test.md").is_err());
    }

    #[test]
    fn test_filename_path_traversal_blocked() {
        let (_dir, manager) = setup();
        assert!(manager.read_file("main", "../etc/passwd").is_err());
        assert!(manager.write_file("main", "../../bad", "x").is_err());
    }
}
```

**Step 2: Add the module declaration**

In `core/src/config/mod.rs`, add:
```rust
pub mod agent_manager;
```

**Step 3: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib config::agent_manager -- --nocapture 2>&1 | tail -30`
Expected: ALL PASS

**Step 4: Run full core check**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: compilation success

**Step 5: Commit**

```bash
git add core/src/config/agent_manager.rs core/src/config/mod.rs
git commit -m "config: implement AgentManager with TOML CRUD and workspace management"
```

---

## Task 4: Add `agents.*` RPC handlers

**Files:**
- Create: `core/src/gateway/handlers/agents.rs` (new — note plural, distinct from existing `agent.rs`)
- Modify: `core/src/gateway/handlers/mod.rs` (add `pub mod agents;` and placeholder registrations)

**Step 1: Create the handler file**

Create `core/src/gateway/handlers/agents.rs`:

```rust
//! Agent Management Handlers
//!
//! RPC handlers for agent CRUD and workspace file operations:
//! - agents.list / agents.get / agents.create / agents.update / agents.delete
//! - agents.set_default
//! - agents.files.list / agents.files.get / agents.files.set / agents.files.delete

use serde::{Deserialize, Serialize};
use serde_json::json;
use crate::sync_primitives::Arc;
use tracing::{debug, info};

use crate::config::agent_manager::{AgentManager, AgentPatch, WorkspaceFile};
use crate::config::types::agents_def::{
    AgentDefinition, AgentIdentity, AgentModelConfig, AgentParams, SubagentPolicy,
};

use super::super::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::parse_params;

// ── Response Types ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct AgentSummary {
    pub id: String,
    pub name: Option<String>,
    pub emoji: Option<String>,
    pub description: Option<String>,
    pub model: Option<String>,
    pub is_default: bool,
}

impl From<&AgentDefinition> for AgentSummary {
    fn from(def: &AgentDefinition) -> Self {
        Self {
            id: def.id.clone(),
            name: def.name.clone(),
            emoji: def.identity.as_ref().and_then(|i| i.emoji.clone()),
            description: def.identity.as_ref().and_then(|i| i.description.clone()),
            model: def.model_config.as_ref().map(|mc| mc.primary.clone()).or_else(|| def.model.clone()),
            is_default: def.default,
        }
    }
}

// ── Request Types ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct AgentIdParams {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateAgentParams {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub identity: Option<AgentIdentity>,
    #[serde(default)]
    pub model_config: Option<AgentModelConfig>,
    #[serde(default)]
    pub params: Option<AgentParams>,
    #[serde(default)]
    pub skills: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAgentParams {
    pub id: String,
    pub patch: AgentPatch,
}

#[derive(Debug, Deserialize)]
pub struct FileParams {
    pub agent_id: String,
    pub filename: String,
}

#[derive(Debug, Deserialize)]
pub struct FileSetParams {
    pub agent_id: String,
    pub filename: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct FileListParams {
    pub agent_id: String,
}

// ── Handlers ────────────────────────────────────────────────────────────

pub async fn handle_list(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
) -> JsonRpcResponse {
    debug!("Handling agents.list");
    match manager.list() {
        Ok(agents) => {
            let default_id = agents.iter()
                .find(|a| a.default)
                .map(|a| a.id.clone())
                .unwrap_or_default();
            let summaries: Vec<AgentSummary> = agents.iter().map(AgentSummary::from).collect();
            JsonRpcResponse::success(request.id, json!({
                "agents": summaries,
                "default_id": default_id,
            }))
        }
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

pub async fn handle_get(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
) -> JsonRpcResponse {
    debug!("Handling agents.get");
    let params: AgentIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    match manager.get(&params.id) {
        Ok(def) => {
            let files = manager.list_files(&params.id).unwrap_or_default();
            JsonRpcResponse::success(request.id, json!({
                "definition": def,
                "file_count": files.len(),
            }))
        }
        Err(e) => JsonRpcResponse::error(request.id, INVALID_PARAMS, e.to_string()),
    }
}

pub async fn handle_create(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    debug!("Handling agents.create");
    let params: CreateAgentParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let def = AgentDefinition {
        id: params.id.clone(),
        name: params.name,
        identity: params.identity,
        model_config: params.model_config,
        params: params.params,
        skills: params.skills,
        ..Default::default()
    };

    match manager.create(def) {
        Ok(()) => {
            let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
                section: Some("agents".to_string()),
                value: json!({"action": "created", "agent_id": params.id}),
                timestamp: chrono::Utc::now().timestamp_millis(),
            });
            let _ = event_bus.publish_json(&event);
            JsonRpcResponse::success(request.id, json!({"success": true, "id": params.id}))
        }
        Err(e) => JsonRpcResponse::error(request.id, INVALID_PARAMS, e.to_string()),
    }
}

pub async fn handle_update(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    debug!("Handling agents.update");
    let params: UpdateAgentParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match manager.update(&params.id, params.patch) {
        Ok(()) => {
            let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
                section: Some("agents".to_string()),
                value: json!({"action": "updated", "agent_id": params.id}),
                timestamp: chrono::Utc::now().timestamp_millis(),
            });
            let _ = event_bus.publish_json(&event);
            JsonRpcResponse::success(request.id, json!({"success": true}))
        }
        Err(e) => JsonRpcResponse::error(request.id, INVALID_PARAMS, e.to_string()),
    }
}

pub async fn handle_delete(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    debug!("Handling agents.delete");
    let params: AgentIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match manager.delete(&params.id) {
        Ok(()) => {
            let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
                section: Some("agents".to_string()),
                value: json!({"action": "deleted", "agent_id": params.id}),
                timestamp: chrono::Utc::now().timestamp_millis(),
            });
            let _ = event_bus.publish_json(&event);
            JsonRpcResponse::success(request.id, json!({"success": true}))
        }
        Err(e) => JsonRpcResponse::error(request.id, INVALID_PARAMS, e.to_string()),
    }
}

pub async fn handle_set_default(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    debug!("Handling agents.set_default");
    let params: AgentIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match manager.set_default(&params.id) {
        Ok(()) => {
            let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
                section: Some("agents".to_string()),
                value: json!({"action": "default_changed", "agent_id": params.id}),
                timestamp: chrono::Utc::now().timestamp_millis(),
            });
            let _ = event_bus.publish_json(&event);
            JsonRpcResponse::success(request.id, json!({"success": true}))
        }
        Err(e) => JsonRpcResponse::error(request.id, INVALID_PARAMS, e.to_string()),
    }
}

// ── File Handlers ───────────────────────────────────────────────────────

pub async fn handle_files_list(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
) -> JsonRpcResponse {
    debug!("Handling agents.files.list");
    let params: FileListParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match manager.list_files(&params.agent_id) {
        Ok(files) => JsonRpcResponse::success(request.id, json!({"files": files})),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

pub async fn handle_files_get(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
) -> JsonRpcResponse {
    debug!("Handling agents.files.get");
    let params: FileParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match manager.read_file(&params.agent_id, &params.filename) {
        Ok(content) => JsonRpcResponse::success(request.id, json!({
            "content": content,
            "filename": params.filename,
        })),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

pub async fn handle_files_set(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
) -> JsonRpcResponse {
    debug!("Handling agents.files.set");
    let params: FileSetParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match manager.write_file(&params.agent_id, &params.filename, &params.content) {
        Ok(()) => JsonRpcResponse::success(request.id, json!({"success": true})),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

pub async fn handle_files_delete(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
) -> JsonRpcResponse {
    debug!("Handling agents.files.delete");
    let params: FileParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match manager.delete_file(&params.agent_id, &params.filename) {
        Ok(()) => JsonRpcResponse::success(request.id, json!({"success": true})),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}
```

**Step 2: Register handlers as placeholders in mod.rs**

In `core/src/gateway/handlers/mod.rs`:
- Add `pub mod agents;` after line 50 (`pub mod agent_config;`)
- Add placeholder registrations in `HandlerRegistry::new()` before the closing `registry` return (around line 513):

```rust
        // Agent management (placeholders — actual handlers wired with AgentManager)
        registry.register("agents.list", |req| async move {
            JsonRpcResponse::error(req.id, INTERNAL_ERROR,
                "agents.list requires AgentManager — wire in Gateway startup".to_string())
        });
        registry.register("agents.get", |req| async move {
            JsonRpcResponse::error(req.id, INTERNAL_ERROR,
                "agents.get requires AgentManager — wire in Gateway startup".to_string())
        });
        registry.register("agents.create", |req| async move {
            JsonRpcResponse::error(req.id, INTERNAL_ERROR,
                "agents.create requires AgentManager — wire in Gateway startup".to_string())
        });
        registry.register("agents.update", |req| async move {
            JsonRpcResponse::error(req.id, INTERNAL_ERROR,
                "agents.update requires AgentManager — wire in Gateway startup".to_string())
        });
        registry.register("agents.delete", |req| async move {
            JsonRpcResponse::error(req.id, INTERNAL_ERROR,
                "agents.delete requires AgentManager — wire in Gateway startup".to_string())
        });
        registry.register("agents.set_default", |req| async move {
            JsonRpcResponse::error(req.id, INTERNAL_ERROR,
                "agents.set_default requires AgentManager — wire in Gateway startup".to_string())
        });
        registry.register("agents.files.list", |req| async move {
            JsonRpcResponse::error(req.id, INTERNAL_ERROR,
                "agents.files.list requires AgentManager — wire in Gateway startup".to_string())
        });
        registry.register("agents.files.get", |req| async move {
            JsonRpcResponse::error(req.id, INTERNAL_ERROR,
                "agents.files.get requires AgentManager — wire in Gateway startup".to_string())
        });
        registry.register("agents.files.set", |req| async move {
            JsonRpcResponse::error(req.id, INTERNAL_ERROR,
                "agents.files.set requires AgentManager — wire in Gateway startup".to_string())
        });
        registry.register("agents.files.delete", |req| async move {
            JsonRpcResponse::error(req.id, INTERNAL_ERROR,
                "agents.files.delete requires AgentManager — wire in Gateway startup".to_string())
        });
```

**Step 3: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: compilation success

**Step 4: Commit**

```bash
git add core/src/gateway/handlers/agents.rs core/src/gateway/handlers/mod.rs
git commit -m "gateway: add agents.* RPC handlers with placeholder registration"
```

---

## Task 5: Wire AgentManager in Gateway startup

**Files:**
- Modify: `core/src/bin/aleph/commands/start/builder/handlers.rs`

**Step 1: Add register_agents_handlers function**

Add at the bottom of `handlers.rs`, following the same pattern as `register_guest_handlers`:

```rust
// ─── register_agents_handlers ───────────────────────────────────────────────

pub(in crate::commands::start) fn register_agents_handlers(
    server: &mut GatewayServer,
    manager: &Arc<alephcore::config::agent_manager::AgentManager>,
    event_bus: &Arc<alephcore::gateway::event_bus::GatewayEventBus>,
) {
    use alephcore::gateway::handlers::agents;

    register_handler!(server, "agents.list", agents::handle_list, manager);
    register_handler!(server, "agents.get", agents::handle_get, manager);
    register_handler!(server, "agents.create", agents::handle_create, manager, event_bus);
    register_handler!(server, "agents.update", agents::handle_update, manager, event_bus);
    register_handler!(server, "agents.delete", agents::handle_delete, manager, event_bus);
    register_handler!(server, "agents.set_default", agents::handle_set_default, manager, event_bus);
    register_handler!(server, "agents.files.list", agents::handle_files_list, manager);
    register_handler!(server, "agents.files.get", agents::handle_files_get, manager);
    register_handler!(server, "agents.files.set", agents::handle_files_set, manager);
    register_handler!(server, "agents.files.delete", agents::handle_files_delete, manager);
}
```

**Step 2: Call it from start/mod.rs**

Find where other `register_*_handlers` are called in `core/src/bin/aleph/commands/start/mod.rs` and add:

```rust
    // Agent management
    let agent_manager = Arc::new(alephcore::config::agent_manager::AgentManager::new(
        alephcore::config::Config::default_path(),
        dirs::home_dir().unwrap_or_default().join(".aleph/agents"),
        dirs::home_dir().unwrap_or_default().join(".aleph/trash"),
    ));
    handlers::register_agents_handlers(&mut server, &agent_manager, &event_bus);
```

**Step 3: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: compilation success

**Step 4: Commit**

```bash
git add core/src/bin/aleph/commands/start/builder/handlers.rs core/src/bin/aleph/commands/start/mod.rs
git commit -m "startup: wire AgentManager into Gateway RPC handlers"
```

---

## Task 6: Panel API layer for agents

**Files:**
- Create: `apps/panel/src/api/agents.rs`
- Modify: `apps/panel/src/api.rs` or `apps/panel/src/api/mod.rs` (add module)

**Step 1: Create the API module**

Create `apps/panel/src/api/agents.rs`. Follow the exact same pattern as `api/agent.rs`:

```rust
//! Panel API for agent management (agents.* RPC calls)

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::context::DashboardState;

// ── Types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSummary {
    pub id: String,
    pub name: Option<String>,
    pub emoji: Option<String>,
    pub description: Option<String>,
    pub model: Option<String>,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentsListResponse {
    pub agents: Vec<AgentSummary>,
    pub default_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentIdentity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentModelConfig {
    pub primary: String,
    #[serde(default)]
    pub fallbacks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDetail {
    pub definition: Value,  // Full AgentDefinition JSON
    pub file_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceFile {
    pub filename: String,
    pub size_bytes: u64,
    pub modified_at: i64,
    pub is_bootstrap: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesListResponse {
    pub files: Vec<WorkspaceFile>,
}

// ── API ─────────────────────────────────────────────────────────────────

pub struct AgentsApi;

impl AgentsApi {
    pub async fn list(state: &DashboardState) -> Result<AgentsListResponse, String> {
        let result = state.rpc_call("agents.list", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn get(state: &DashboardState, id: &str) -> Result<AgentDetail, String> {
        let result = state.rpc_call("agents.get", json!({"id": id})).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn create(
        state: &DashboardState,
        id: &str,
        name: Option<&str>,
        identity: Option<&AgentIdentity>,
    ) -> Result<(), String> {
        let params = json!({
            "id": id,
            "name": name,
            "identity": identity,
        });
        state.rpc_call("agents.create", params).await?;
        Ok(())
    }

    pub async fn update(state: &DashboardState, id: &str, patch: Value) -> Result<(), String> {
        let params = json!({"id": id, "patch": patch});
        state.rpc_call("agents.update", params).await?;
        Ok(())
    }

    pub async fn delete(state: &DashboardState, id: &str) -> Result<(), String> {
        state.rpc_call("agents.delete", json!({"id": id})).await?;
        Ok(())
    }

    pub async fn set_default(state: &DashboardState, id: &str) -> Result<(), String> {
        state.rpc_call("agents.set_default", json!({"id": id})).await?;
        Ok(())
    }

    // Files

    pub async fn files_list(state: &DashboardState, agent_id: &str) -> Result<FilesListResponse, String> {
        let result = state.rpc_call("agents.files.list", json!({"agent_id": agent_id})).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn files_get(state: &DashboardState, agent_id: &str, filename: &str) -> Result<String, String> {
        let result = state.rpc_call("agents.files.get", json!({"agent_id": agent_id, "filename": filename})).await?;
        result.get("content").and_then(|v| v.as_str()).map(|s| s.to_string())
            .ok_or_else(|| "Missing content in response".to_string())
    }

    pub async fn files_set(state: &DashboardState, agent_id: &str, filename: &str, content: &str) -> Result<(), String> {
        state.rpc_call("agents.files.set", json!({
            "agent_id": agent_id,
            "filename": filename,
            "content": content,
        })).await?;
        Ok(())
    }

    pub async fn files_delete(state: &DashboardState, agent_id: &str, filename: &str) -> Result<(), String> {
        state.rpc_call("agents.files.delete", json!({
            "agent_id": agent_id,
            "filename": filename,
        })).await?;
        Ok(())
    }
}
```

**Step 2: Register the module**

In `apps/panel/src/api/mod.rs` (or wherever api modules are declared), add:
```rust
pub mod agents;
pub use agents::AgentsApi;
```

**Step 3: Commit**

```bash
git add apps/panel/src/api/agents.rs apps/panel/src/api/mod.rs
git commit -m "panel: add AgentsApi for agents.* RPC calls"
```

---

## Task 7: Panel navigation — PanelMode::Agents + BottomBar

**Files:**
- Modify: `apps/panel/src/components/bottom_bar.rs`
- Modify: `apps/panel/src/components/mode_sidebar.rs`
- Modify: `apps/panel/src/app.rs`

**Step 1: Add Agents to PanelMode**

In `bottom_bar.rs`, add `Agents` variant to enum and update `from_path`:

```rust
pub enum PanelMode {
    Chat,
    Dashboard,
    Agents,
    Settings,
}

impl PanelMode {
    pub fn from_path(path: &str) -> Self {
        if path.starts_with("/agents") {
            Self::Agents
        } else if path.starts_with("/dashboard") {
            Self::Dashboard
        } else if path.starts_with("/settings") {
            Self::Settings
        } else if path.starts_with("/chat") || path == "/" {
            Self::Chat
        } else {
            Self::Chat
        }
    }
}
```

Add the Agents button in BottomBar view (between Dashboard and Settings):

```rust
            <BottomBarItem
                label="Agents"
                mode=PanelMode::Agents
                active_mode=Signal::derive(active_mode)
                on_click=go("/agents")
            >
                <circle cx="12" cy="8" r="4"/>
                <path d="M6 21v-2a4 4 0 0 1 4-4h4a4 4 0 0 1 4 4v2"/>
                <line x1="12" y1="2" x2="12" y2="4"/>
            </BottomBarItem>
```

**Step 2: Add Agents sidebar branch in mode_sidebar.rs**

Add the import and match arm:

```rust
use super::agents_sidebar::AgentsSidebar;
```

In the match:
```rust
PanelMode::Agents => view! { <AgentsSidebar /> }.into_any(),
```

**Step 3: Add Agents router in app.rs**

In `MainContent`, add the Agents container:

```rust
        <div style:display=move || if mode.get() == PanelMode::Agents { "contents" } else { "none" }>
            <AgentsRouter />
        </div>
```

Add a minimal `AgentsRouter` component (placeholder for now):

```rust
#[component]
fn AgentsRouter() -> impl IntoView {
    let location = use_location();

    move || {
        let path = location.pathname.get();
        if path.starts_with("/agents") {
            view! {
                <div class="p-8">
                    <h1 class="text-2xl font-bold text-text-primary">"Agent Management"</h1>
                    <p class="text-text-secondary mt-2">"Coming soon — select an agent from the sidebar"</p>
                </div>
            }.into_any()
        } else {
            ().into_any()
        }
    }
}
```

**Step 4: Commit**

```bash
git add apps/panel/src/components/bottom_bar.rs apps/panel/src/components/mode_sidebar.rs apps/panel/src/app.rs
git commit -m "panel: add Agents as 4th navigation mode in bottom bar"
```

---

## Task 8: AgentsSidebar component

**Files:**
- Create: `apps/panel/src/components/agents_sidebar.rs`
- Modify: `apps/panel/src/components/mod.rs` (add `pub mod agents_sidebar;`)

**Step 1: Create the sidebar component**

Create `apps/panel/src/components/agents_sidebar.rs`. This component:
- Loads agent list via `AgentsApi::list()`
- Shows "+" create button
- Lists agents with emoji + name + default badge
- Has a default agent selector dropdown at bottom
- Navigates to `/agents/{id}/overview` on click

Follow the same Leptos patterns used in `chat_sidebar.rs` and `dashboard_sidebar.rs`.

The implementation should use `spawn_local` for async RPC calls, `signal` for reactive state, and `use_navigate()` for routing — matching existing codebase patterns.

**Step 2: Register module**

In `apps/panel/src/components/mod.rs`, add:
```rust
pub mod agents_sidebar;
```

**Step 3: Commit**

```bash
git add apps/panel/src/components/agents_sidebar.rs apps/panel/src/components/mod.rs
git commit -m "panel: add AgentsSidebar with list, create, and default selector"
```

---

## Task 9: AgentsView main frame with tab routing

**Files:**
- Create: `apps/panel/src/views/agents/mod.rs`
- Modify: `apps/panel/src/views/mod.rs` (add `pub mod agents;`)
- Modify: `apps/panel/src/app.rs` (wire `AgentsRouter` to sub-views)

**Step 1: Create the agents views module**

Create `apps/panel/src/views/agents/mod.rs` with:
- `AgentsView` component that parses `agent_id` and `tab` from URL
- Header showing emoji + name + delete button
- Tab bar with 6 tabs (Overview / Behavior / Files / Skills / Tools / Channels)
- Tab content area routing to sub-components
- Placeholder sub-components for each tab

**Step 2: Wire into AgentsRouter**

Replace the placeholder `AgentsRouter` in `app.rs` with actual routing:

```rust
#[component]
fn AgentsRouter() -> impl IntoView {
    let location = use_location();

    move || {
        let path = location.pathname.get();
        match path.as_str() {
            "/agents" => view! { <crate::views::agents::AgentsView /> }.into_any(),
            p if p.starts_with("/agents/") => view! { <crate::views::agents::AgentsView /> }.into_any(),
            _ => ().into_any(),
        }
    }
}
```

**Step 3: Commit**

```bash
git add apps/panel/src/views/agents/ apps/panel/src/views/mod.rs apps/panel/src/app.rs
git commit -m "panel: add AgentsView with 6-tab routing framework"
```

---

## Task 10: Remove Agent from Settings

**Files:**
- Modify: `apps/panel/src/components/settings_sidebar.rs` (remove `SettingsTab::Agent` from `SETTINGS_GROUPS`)
- Modify: `apps/panel/src/app.rs` (remove `/settings/agent` route from `SettingsRouter`)
- Delete: `apps/panel/src/views/settings/agent.rs`
- Modify: `apps/panel/src/views/settings/mod.rs` (remove `pub mod agent`, `pub use agent::AgentView`)

**Step 1: Remove from settings sidebar**

In `settings_sidebar.rs`, remove `SettingsTab::Agent` from the "Advanced" group in `SETTINGS_GROUPS`.

**Step 2: Remove route from SettingsRouter**

In `app.rs`, remove:
```rust
"/settings/agent" => view! { <AgentView /> }.into_any(),
```

**Step 3: Remove module**

Delete `apps/panel/src/views/settings/agent.rs`.

In `apps/panel/src/views/settings/mod.rs`, remove:
```rust
pub mod agent;
pub use agent::AgentView;
```

**Step 4: Verify compilation**

Run: `cd apps/panel && cargo check 2>&1 | tail -5`
Expected: compilation success (or use `cargo check -p alephcore` if panel doesn't build standalone)

**Step 5: Commit**

```bash
git add -A apps/panel/src/
git commit -m "panel: migrate Agent from Settings to top-level navigation"
```

---

## Task 11: Overview Tab implementation

**Files:**
- Create: `apps/panel/src/views/agents/overview.rs`
- Modify: `apps/panel/src/views/agents/mod.rs` (replace placeholder)

**Step 1: Implement Overview Tab**

Create `apps/panel/src/views/agents/overview.rs` with sections:
- **Identity Editor**: emoji input, name input, description textarea, avatar URL input, theme input
- **Model Configuration**: primary model dropdown (populated from providers), fallbacks comma-separated input
- **Inference Parameters**: temperature slider (0.0-2.0), max_tokens number input, top_p slider, top_k number input
- **Subagent Policy**: allow list text input

Uses `AgentsApi::get()` to load and `AgentsApi::update()` to save.

**Step 2: Commit**

```bash
git add apps/panel/src/views/agents/overview.rs apps/panel/src/views/agents/mod.rs
git commit -m "panel: implement Overview tab with identity, model, and params editing"
```

---

## Task 12: Behavior Tab (migrate from settings)

**Files:**
- Create: `apps/panel/src/views/agents/behavior.rs`

**Step 1: Migrate existing components**

Copy the `FileOpsSection`, `CodeExecSection`, and `GeneralSettingsSection` components from the deleted `settings/agent.rs` into `agents/behavior.rs`. Wrap them in a `BehaviorTab` component.

These components already work — they use `AgentConfigApi` from `api/agent.rs` which is retained.

**Step 2: Commit**

```bash
git add apps/panel/src/views/agents/behavior.rs
git commit -m "panel: migrate Behavior tab from settings/agent.rs"
```

---

## Task 13: Files Tab implementation

**Files:**
- Create: `apps/panel/src/views/agents/files.rs`

**Step 1: Implement Files Tab**

Create `apps/panel/src/views/agents/files.rs` with:
- Left panel: file list (using `AgentsApi::files_list()`), bootstrap files marked with badge
- Right panel: textarea editor for selected file content
- Create file button (filename input + create)
- Delete file button (with confirmation)
- Save button (calls `AgentsApi::files_set()`)

**Step 2: Commit**

```bash
git add apps/panel/src/views/agents/files.rs
git commit -m "panel: implement Files tab with inline editor and file management"
```

---

## Task 14: Skills Tab implementation

**Files:**
- Create: `apps/panel/src/views/agents/skills.rs`

**Step 1: Implement Skills Tab**

Create `apps/panel/src/views/agents/skills.rs` with:
- Load available skills via existing `skills.list` RPC
- Load agent's current skills from `agents.get`
- Toggle checkboxes per skill
- Search/filter input
- Save calls `AgentsApi::update()` with `patch.skills`

**Step 2: Commit**

```bash
git add apps/panel/src/views/agents/skills.rs
git commit -m "panel: implement Skills tab with per-agent toggles"
```

---

## Task 15: Tools Tab implementation (Phase 1: read-only)

**Files:**
- Create: `apps/panel/src/views/agents/tools.rs`

**Step 1: Implement Tools Tab**

Create `apps/panel/src/views/agents/tools.rs` with:
- Display agent's current skills/tool permissions from `agents.get`
- Read-only display of subagent policy
- Info text: "Full tool management coming in Phase 2"

**Step 2: Commit**

```bash
git add apps/panel/src/views/agents/tools.rs
git commit -m "panel: implement Tools tab (Phase 1 read-only)"
```

---

## Task 16: Channels Tab implementation

**Files:**
- Create: `apps/panel/src/views/agents/channels.rs`

**Step 1: Implement Channels Tab**

Create `apps/panel/src/views/agents/channels.rs` with:
- List routing rules bound to this agent (from existing `routing_rules.*` RPC or inline display)
- Add binding form: channel type dropdown, account/peer ID input
- Remove binding button
- Default agent selector (global — calls `AgentsApi::set_default()`)

**Step 2: Commit**

```bash
git add apps/panel/src/views/agents/channels.rs
git commit -m "panel: implement Channels tab with routing bindings and default selector"
```

---

## Task 17: Final integration test and cleanup

**Step 1: Run all core tests**

Run: `cargo test -p alephcore --lib 2>&1 | tail -20`
Expected: ALL PASS (except pre-existing `markdown_skill::loader` failures)

**Step 2: Run cargo check for whole project**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: success

**Step 3: Final commit with summary**

```bash
git add -A
git commit -m "agents: complete Agent Management Panel — CRUD, 6-tab UI, workspace files"
```
