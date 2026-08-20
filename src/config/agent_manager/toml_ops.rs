//! TOML document operations — load, save, find, and build agent tables

use std::fs;

use serde::Deserialize;
use toml_edit::{Array, DocumentMut, Item, Table};
use tracing::debug;

use crate::config::types::agents_def::{AgentDefinition, AgentModelRef, AgentsConfig};
use crate::error::{AlephError, Result};

use super::AgentManager;

impl AgentManager {
    /// Load config and parse the [agents] section
    pub(super) fn load_config(&self) -> Result<AgentsConfig> {
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

        let wrapper: Wrapper = toml::from_str(&content)
            .map_err(|e| AlephError::invalid_config(format!("Failed to parse config: {e}")))?;

        Ok(wrapper.agents)
    }

    /// Load config file as a `toml_edit` Document for format-preserving edits
    pub(super) fn load_document(&self) -> Result<DocumentMut> {
        let content = fs::read_to_string(&self.config_path).map_err(|e| {
            AlephError::IoError(format!(
                "Failed to read config file '{}': {}",
                self.config_path.display(),
                e
            ))
        })?;

        content
            .parse::<DocumentMut>()
            .map_err(|e| AlephError::invalid_config(format!("Failed to parse config as TOML: {e}")))
    }

    /// Atomic write: write to .tmp, fsync, then rename
    pub(super) fn save_document(&self, doc: &DocumentMut) -> Result<()> {
        let content = doc.to_string();
        let tmp_path = self.config_path.with_extension("toml.tmp");

        fs::write(&tmp_path, &content)
            .map_err(|e| AlephError::IoError(format!("Failed to write tmp config: {e}")))?;

        // fsync on unix
        #[cfg(unix)]
        {
            use std::fs::OpenOptions;
            use std::io::Write;
            let mut f = OpenOptions::new()
                .write(true)
                .open(&tmp_path)
                .map_err(|e| {
                    AlephError::IoError(format!("Failed to open temp file for fsync: {e}"))
                })?;
            f.flush()
                .map_err(|e| AlephError::IoError(format!("Failed to flush temp file: {e}")))?;
            f.sync_all()
                .map_err(|e| AlephError::IoError(format!("Failed to fsync temp file: {e}")))?;
        }

        fs::rename(&tmp_path, &self.config_path)
            .map_err(|e| AlephError::IoError(format!("Failed to rename tmp config: {e}")))?;

        // Restore 600 permissions (owner read/write only): the rename replaces
        // the config file with the freshly-created temp file (default umask
        // perms), which would otherwise widen the 600 set by save_to_file.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = fs::metadata(&self.config_path) {
                let mut perms = metadata.permissions();
                perms.set_mode(0o600);
                if let Err(e) = fs::set_permissions(&self.config_path, perms) {
                    tracing::warn!("Failed to set config file permissions: {}", e);
                }
            }
        }

        debug!("Saved config to {}", self.config_path.display());
        Ok(())
    }

    /// Find the index of an agent in the [[agents.list]] array by ID
    pub(super) fn find_agent_index(&self, doc: &DocumentMut, id: &str) -> Result<usize> {
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
            "Agent '{id}' not found in config"
        )))
    }

    /// Append an `AgentDefinition` to the [[agents.list]] array in the document
    pub(super) fn append_agent_to_document(
        &self,
        doc: &mut DocumentMut,
        def: &AgentDefinition,
    ) -> Result<()> {
        // Ensure [agents] table exists
        if doc.get("agents").is_none() {
            doc["agents"] = Item::Table(Table::new());
        }

        // Build the agent table
        let agent = agent_definition_to_table(def);

        // Append to [[agents.list]]
        let agents = doc["agents"]
            .as_table_mut()
            .ok_or_else(|| AlephError::invalid_config("[agents] is not a table"))?;

        if agents.get("list").is_none() {
            // Create the array of tables
            agents.insert("list", Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));
        }

        let list = agents
            .get_mut("list")
            .and_then(|v| v.as_array_of_tables_mut())
            .ok_or_else(|| {
                AlephError::invalid_config("[[agents.list]] is not an array of tables")
            })?;

        list.push(agent);
        Ok(())
    }
}

/// Build a `toml_edit::Table` from an `AgentDefinition`, one key per set field.
fn agent_definition_to_table(def: &AgentDefinition) -> Table {
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
        agent["model"] = model_ref_to_item(model);
    }

    if let Some(ref skills) = def.skills {
        let mut arr = Array::new();
        for s in skills {
            arr.push(s.as_str());
        }
        agent["skills"] = toml_edit::value(arr);
    }

    if let Some(archetype) = def.archetype {
        agent["archetype"] = toml_edit::value(archetype.as_str());
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
        agent["identity"] = Item::Table(t);
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

    agent
}

/// Write `AgentModelRef` as a `toml_edit` Item:
/// Legacy → bare string; Qualified → inline table `{ provider, model }`.
pub(super) fn model_ref_to_item(m: &AgentModelRef) -> toml_edit::Item {
    match m {
        AgentModelRef::Legacy(s) => toml_edit::value(s.as_str()),
        AgentModelRef::Qualified { provider, model } => {
            let mut t = toml_edit::InlineTable::new();
            t.insert("provider", provider.as_str().into());
            t.insert("model", model.as_str().into());
            toml_edit::value(t)
        }
    }
}
