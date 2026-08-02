//! Agent identity file operations — list, read, write, delete files in agent identity directories

use std::fs;

use crate::error::{AlephError, Result};

use super::{curated_owned_reason, is_curated_owned, AgentManager, WorkspaceFile, BOOTSTRAP_FILES};

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
    ///
    /// Curated-memory-owned files are refused: `agents.files.list` enumerates
    /// `MEMORY.md`, so the Panel file editor could otherwise overwrite the
    /// `§`-delimited body with free-form markdown and lock the curated store
    /// into legacy mode.
    pub fn write_file(&self, agent_id: &str, filename: &str, content: &str) -> Result<()> {
        self.validate_agent_id(agent_id)?;
        self.validate_filename(filename)?;
        if is_curated_owned(filename) {
            return Err(AlephError::invalid_config(curated_owned_reason(filename)));
        }
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
    ///
    /// Curated-memory-owned files are refused for the same reason as
    /// [`Self::write_file`] — deleting `MEMORY.md` out from under a loaded
    /// `CuratedMemoryStore` discards every entry with no `remember` receipt.
    pub fn delete_file(&self, agent_id: &str, filename: &str) -> Result<()> {
        self.validate_agent_id(agent_id)?;
        self.validate_filename(filename)?;
        if is_curated_owned(filename) {
            return Err(AlephError::invalid_config(curated_owned_reason(filename)));
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn manager(dir: &TempDir) -> AgentManager {
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            "[agents]\n\n[[agents.list]]\nid = \"main\"\ndefault = true\n",
        )
        .unwrap();
        let workspace_root = dir.path().join("workspaces");
        let agents_root = dir.path().join("agents");
        let trash_root = dir.path().join("trash");
        fs::create_dir_all(&workspace_root).unwrap();
        fs::create_dir_all(&agents_root).unwrap();
        fs::create_dir_all(&trash_root).unwrap();
        AgentManager::new(config_path, workspace_root, agents_root, trash_root)
    }

    #[test]
    fn is_curated_owned_is_case_insensitive_and_narrow() {
        assert!(is_curated_owned("MEMORY.md"));
        assert!(is_curated_owned("memory.md"));
        assert!(!is_curated_owned("SOUL.md"));
        assert!(!is_curated_owned("MEMORY.md.bak"));
    }

    /// `agents.files.set` / `.delete` hand the filename straight to these two
    /// methods with no guard of their own, and `agents.files.list` enumerates
    /// `MEMORY.md` — so without this the Panel file editor could overwrite the
    /// `§`-delimited curated body with free-form markdown (which then reads
    /// back as one legacy entry and blocks every subsequent `remember(add)`).
    #[test]
    fn write_and_delete_refuse_curated_owned_files() {
        let dir = TempDir::new().unwrap();
        let mgr = manager(&dir);
        let agent_dir = mgr.agents_root.join("main");
        fs::create_dir_all(&agent_dir).unwrap();
        let memory = agent_dir.join("MEMORY.md");
        let curated_body = "fact one\n\u{a7}\n";
        fs::write(&memory, curated_body).unwrap();

        let reason = curated_owned_reason("MEMORY.md");
        let err = mgr
            .write_file("main", "MEMORY.md", "# free-form notes")
            .unwrap_err();
        assert!(err.to_string().contains(reason.as_str()), "{err}");
        assert!(reason.contains("remember"), "must name the escape hatch");
        assert_eq!(fs::read_to_string(&memory).unwrap(), curated_body);

        let err = mgr.delete_file("main", "MEMORY.md").unwrap_err();
        assert!(err.to_string().contains(reason.as_str()), "{err}");
        assert!(memory.exists(), "delete must not have run");

        // Case variant resolves to the same file on case-insensitive
        // filesystems, so it must be refused too.
        assert!(mgr.write_file("main", "memory.md", "x").is_err());

        // The guard is narrow: ordinary identity files still round-trip.
        mgr.write_file("main", "SOUL.md", "# Soul").unwrap();
        assert_eq!(mgr.read_file("main", "SOUL.md").unwrap(), "# Soul");
        mgr.delete_file("main", "SOUL.md").unwrap();
    }

    /// The other two write surfaces must reject with the *same* sentence — if
    /// any of them ever re-hand-rolls its own literal, these stop matching.
    /// (The third, `self_config`, is covered by its own in-file tests, which
    /// assert the shared "Use the `remember` tool" wording.)
    #[test]
    fn identity_file_write_refuses_with_the_same_single_sourced_reason() {
        let dir = TempDir::new().unwrap();
        let err = crate::thinker::identity_files::write_identity_file(
            dir.path(),
            "MEMORY.md",
            "# free-form notes",
        )
        .unwrap_err();
        assert_eq!(err, curated_owned_reason("MEMORY.md"));
        assert!(!dir.path().join("MEMORY.md").exists());
    }
}
