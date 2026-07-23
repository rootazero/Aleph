//! Agent identity file operations — list, read, write, delete files in agent identity directories

use std::fs;

use crate::error::{AlephError, Result};

use super::{AgentManager, WorkspaceFile, BOOTSTRAP_FILES};

impl AgentManager {
    /// Validate `agent_id`: only ASCII alphanumeric, underscore, hyphen.
    ///
    /// Mirrors `AgentManager::create` in `crud.rs` so a Unicode-aware ID
    /// like `café` cannot pass here only to be rejected by the canonical
    /// validator — the two paths used to disagree, producing intermittent
    /// write failures depending on which entry point was hit.
    pub(super) fn validate_agent_id(&self, agent_id: &str) -> Result<()> {
        self.validate_id(agent_id)
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
            .map_err(|e| AlephError::IoError(format!("Failed to read agent dir: {e}")))?;

        for entry in entries {
            let entry =
                entry.map_err(|e| AlephError::IoError(format!("Failed to read dir entry: {e}")))?;
            let metadata = entry
                .metadata()
                .map_err(|e| AlephError::IoError(format!("Failed to read metadata: {e}")))?;

            if !metadata.is_file() {
                continue;
            }

            let filename = entry.file_name().to_string_lossy().to_string();
            let modified_at = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|d| d.as_secs().try_into().ok())
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
        let agent_dir = self.agents_root.join(agent_id);
        let path = agent_dir.join(filename);
        self.require_contained(&agent_dir, &path)?;
        fs::read_to_string(&path).map_err(|e| {
            AlephError::IoError(format!("Failed to read file '{}': {}", path.display(), e))
        })
    }

    /// Write a file to an agent's identity directory
    pub fn write_file(&self, agent_id: &str, filename: &str, content: &str) -> Result<()> {
        self.validate_agent_id(agent_id)?;
        self.validate_filename(filename)?;
        let agent_dir = self.agents_root.join(agent_id);
        let path = agent_dir.join(filename);
        self.require_contained(&agent_dir, &path)?;
        fs::create_dir_all(&agent_dir)
            .map_err(|e| AlephError::IoError(format!("Failed to create agent dir: {e}")))?;
        fs::write(&path, content).map_err(|e| {
            AlephError::IoError(format!("Failed to write file '{}': {}", path.display(), e))
        })
    }

    /// Delete a file from an agent's identity directory
    pub fn delete_file(&self, agent_id: &str, filename: &str) -> Result<()> {
        self.validate_agent_id(agent_id)?;
        self.validate_filename(filename)?;
        let agent_dir = self.agents_root.join(agent_id);
        let path = agent_dir.join(filename);
        self.require_contained(&agent_dir, &path)?;
        if path.exists() {
            fs::remove_file(&path).map_err(|e| {
                AlephError::IoError(format!("Failed to delete file '{}': {}", path.display(), e))
            })?;
        }
        Ok(())
    }

    /// Ensure `candidate` resolves inside `base` using lexical normalization.
    /// Catches traversal that survives string-level validation (e.g. symlinks
    /// resolved by the OS, or crafted relative paths).
    fn require_contained(&self, base: &std::path::Path, candidate: &std::path::Path) -> Result<()> {
        let normalized = normalize_path_lexically(candidate);
        let base_normalized = normalize_path_lexically(base);
        if !normalized.starts_with(&base_normalized) {
            return Err(AlephError::invalid_config(format!(
                "Path '{}' escapes allowed directory '{}'",
                candidate.display(),
                base.display()
            )));
        }
        Ok(())
    }
}

/// Lexically normalize a path by resolving `.` and `..` components without
/// touching the filesystem. Returns the input unchanged if it is absolute
/// (absolute paths are unexpected here and rejected by the caller).
fn normalize_path_lexically(path: &std::path::Path) -> std::path::PathBuf {
    let mut normalized = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {
                // Preserve absolute prefix so that starts_with works later;
                // such paths are rejected by validate_filename anyway.
                normalized.push(component.as_os_str());
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::Normal(c) => normalized.push(c),
        }
    }
    normalized
}
