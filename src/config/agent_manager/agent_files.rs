//! Agent identity file operations — list, read, write, delete files in agent identity directories

use std::fs;

use crate::error::{AlephError, Result};

use super::{AgentManager, WorkspaceFile, BOOTSTRAP_FILES};

impl AgentManager {
    /// Validate agent_id: only alphanumeric, underscore, hyphen
    pub(super) fn validate_agent_id(&self, agent_id: &str) -> Result<()> {
        if agent_id.is_empty() {
            return Err(AlephError::invalid_config(
                "agent_id must not be empty".to_string(),
            ));
        }
        if agent_id.len() > super::MAX_ID_LENGTH {
            return Err(AlephError::invalid_config(format!(
                "agent_id '{}' exceeds max length {}",
                agent_id,
                super::MAX_ID_LENGTH
            )));
        }
        if !agent_id
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return Err(AlephError::invalid_config(format!(
                "Invalid agent_id '{}': must be alphanumeric, '_', or '-'",
                agent_id
            )));
        }
        Ok(())
    }

    /// List files in an agent's identity directory
    pub fn list_files(&self, agent_id: &str) -> Result<Vec<WorkspaceFile>> {
        self.validate_agent_id(agent_id)?;
        let agent_dir = self.agents_root.join(agent_id);
        if !agent_dir.exists() {
            return Ok(Vec::new());
        }

        let mut files = Vec::new();
        let entries = fs::read_dir(&agent_dir)
            .map_err(|e| AlephError::IoError(format!("Failed to read agent dir: {}", e)))?;

        for entry in entries {
            let entry = entry
                .map_err(|e| AlephError::IoError(format!("Failed to read dir entry: {}", e)))?;
            let metadata = entry
                .metadata()
                .map_err(|e| AlephError::IoError(format!("Failed to read metadata: {}", e)))?;

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

    /// Read a file from an agent's identity directory
    pub fn read_file(&self, agent_id: &str, filename: &str) -> Result<String> {
        self.validate_agent_id(agent_id)?;
        self.validate_filename(filename)?;
        let path = self.agents_root.join(agent_id).join(filename);
        fs::read_to_string(&path).map_err(|e| {
            AlephError::IoError(format!("Failed to read file '{}': {}", path.display(), e))
        })
    }

    /// Write a file to an agent's identity directory
    pub fn write_file(&self, agent_id: &str, filename: &str, content: &str) -> Result<()> {
        self.validate_agent_id(agent_id)?;
        self.validate_filename(filename)?;
        let agent_dir = self.agents_root.join(agent_id);
        fs::create_dir_all(&agent_dir)
            .map_err(|e| AlephError::IoError(format!("Failed to create agent dir: {}", e)))?;
        let path = agent_dir.join(filename);
        fs::write(&path, content).map_err(|e| {
            AlephError::IoError(format!("Failed to write file '{}': {}", path.display(), e))
        })
    }

    /// Delete a file from an agent's identity directory
    pub fn delete_file(&self, agent_id: &str, filename: &str) -> Result<()> {
        self.validate_agent_id(agent_id)?;
        self.validate_filename(filename)?;
        let path = self.agents_root.join(agent_id).join(filename);
        if path.exists() {
            fs::remove_file(&path).map_err(|e| {
                AlephError::IoError(format!("Failed to delete file '{}': {}", path.display(), e))
            })?;
        }
        Ok(())
    }
}
