//! TOML document operations — load, save, find, and build agent tables

use std::fs;

use serde::Deserialize;
use toml_edit::{DocumentMut, Item, Table};
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

    /// Atomic replacement with private, unique staging and error-path cleanup.
    pub(super) fn save_document(&self, doc: &DocumentMut) -> Result<()> {
        crate::utils::atomic_io::write_atomic(&self.config_path, doc.to_string().as_bytes())
            .map_err(|e| AlephError::IoError(format!("Failed to save config: {e}")))?;
        debug!("Saved config to {}", self.config_path.display());
        Ok(())
    }

    /// Serialize the complete read/modify/save transaction across manager instances.
    /// Other config writers must use this same sidecar to participate.
    pub(super) fn with_config_write<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let mut lock_path = self.config_path.as_os_str().to_os_string();
        lock_path.push(".lock");
        crate::utils::atomic_io::with_file_lock(std::path::Path::new(&lock_path), |_| Ok(f()))
            .map_err(|e| AlephError::IoError(format!("Failed to lock config: {e}")))?
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
        let agent = agent_definition_to_table(def)?;

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

/// Use the definition's serde contract so active fields cannot silently disappear.
fn agent_definition_to_table(def: &AgentDefinition) -> Result<Table> {
    let content = toml::to_string(def)
        .map_err(|e| AlephError::invalid_config(format!("Failed to serialize agent: {e}")))?;
    let mut agent = content
        .parse::<DocumentMut>()
        .map_err(|e| AlephError::invalid_config(format!("Failed to build agent table: {e}")))?
        .into_table();
    // Deprecated and ignored by the resolver; do not persist new workspace overrides.
    agent.remove("workspace");
    Ok(agent)
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
