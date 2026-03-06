//! Config-driven ExtraFilesHook — reads extra files from `prompt.extra_files` config.

use std::path::PathBuf;

use crate::error::Result;
use crate::thinker::prompt_hooks_v2::{ExtraFile, ExtraFilesContext, ExtraFilesHook};

/// Built-in ExtraFilesHook that reads files specified in `prompt.extra_files` config.
pub struct ConfigExtraFilesHook {
    workspace_dir: PathBuf,
    paths: Vec<String>,
}

impl ConfigExtraFilesHook {
    /// Create from config values.
    pub fn new(workspace_dir: PathBuf, paths: Vec<String>) -> Self {
        Self { workspace_dir, paths }
    }
}

impl ExtraFilesHook for ConfigExtraFilesHook {
    fn name(&self) -> &str {
        "config_extra_files"
    }

    fn extra_files(&self, _ctx: &ExtraFilesContext) -> Result<Vec<ExtraFile>> {
        let mut files = Vec::new();
        for path in &self.paths {
            let full_path = self.workspace_dir.join(path);
            if let Ok(content) = std::fs::read_to_string(&full_path) {
                if !content.trim().is_empty() {
                    files.push(ExtraFile {
                        path: path.clone(),
                        content,
                    });
                }
            }
        }
        Ok(files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::fs;

    #[test]
    fn reads_configured_files() {
        let dir = tempdir().unwrap();
        let docs_dir = dir.path().join("docs");
        fs::create_dir_all(&docs_dir).unwrap();
        fs::write(docs_dir.join("API.md"), "# API Reference").unwrap();
        fs::write(docs_dir.join("ARCH.md"), "# Architecture").unwrap();

        let hook = ConfigExtraFilesHook::new(
            dir.path().to_path_buf(),
            vec!["docs/API.md".into(), "docs/ARCH.md".into()],
        );

        let ctx = ExtraFilesContext {
            workspace_dir: dir.path().to_path_buf(),
            session_key: "test".into(),
            channel: "cli".into(),
        };

        let files = hook.extra_files(&ctx).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "docs/API.md");
        assert!(files[0].content.contains("API Reference"));
        assert_eq!(files[1].path, "docs/ARCH.md");
    }

    #[test]
    fn skips_missing_files() {
        let dir = tempdir().unwrap();
        let hook = ConfigExtraFilesHook::new(
            dir.path().to_path_buf(),
            vec!["nonexistent.md".into()],
        );

        let ctx = ExtraFilesContext {
            workspace_dir: dir.path().to_path_buf(),
            session_key: "test".into(),
            channel: "cli".into(),
        };

        let files = hook.extra_files(&ctx).unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn skips_empty_files() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("empty.md"), "   \n  ").unwrap();

        let hook = ConfigExtraFilesHook::new(
            dir.path().to_path_buf(),
            vec!["empty.md".into()],
        );

        let ctx = ExtraFilesContext {
            workspace_dir: dir.path().to_path_buf(),
            session_key: "test".into(),
            channel: "cli".into(),
        };

        let files = hook.extra_files(&ctx).unwrap();
        assert!(files.is_empty());
    }
}
