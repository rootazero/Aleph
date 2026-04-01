//! TOML document operations — load, save, find, and build agent tables

use std::fs;

use serde::Deserialize;
use toml_edit::{Array, DocumentMut, Item, Table};
use tracing::debug;

use crate::config::types::agents_def::{AgentDefinition, AgentsConfig};
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
            .map_err(|e| AlephError::invalid_config(format!("Failed to parse config: {}", e)))?;

        Ok(wrapper.agents)
    }

    /// Load config file as a toml_edit Document for format-preserving edits
    pub(super) fn load_document(&self) -> Result<DocumentMut> {
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
    pub(super) fn save_document(&self, doc: &DocumentMut) -> Result<()> {
        let content = doc.to_string();
        let tmp_path = self.config_path.with_extension("toml.tmp");

        fs::write(&tmp_path, &content)
            .map_err(|e| AlephError::IoError(format!("Failed to write tmp config: {}", e)))?;

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

        fs::rename(&tmp_path, &self.config_path)
            .map_err(|e| AlephError::IoError(format!("Failed to rename tmp config: {}", e)))?;

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
            "Agent '{}' not found in config",
            id
        )))
    }

    /// Append an AgentDefinition to the [[agents.list]] array in the document
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
