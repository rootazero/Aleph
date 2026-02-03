// core/src/memory/scratchpad/manager.rs

//! Scratchpad Manager
//!
//! Manages the lifecycle of project-local scratchpad files.

use crate::error::AetherError;
use std::path::PathBuf;
use tokio::fs;

use super::template::{generate_scratchpad, DEFAULT_TEMPLATE};

/// Configuration for scratchpad behavior
#[derive(Debug, Clone)]
pub struct ScratchpadConfig {
    /// Directory name within project root (default: ".aether")
    pub dir_name: String,
    /// Scratchpad filename (default: "scratchpad.md")
    pub filename: String,
    /// History log filename (default: "session_history.log")
    pub history_filename: String,
    /// Create backup before overwrite
    pub backup_on_write: bool,
}

impl Default for ScratchpadConfig {
    fn default() -> Self {
        Self {
            dir_name: ".aether".to_string(),
            filename: "scratchpad.md".to_string(),
            history_filename: "session_history.log".to_string(),
            backup_on_write: true,
        }
    }
}

/// Manages project-local scratchpad files
pub struct ScratchpadManager {
    project_root: PathBuf,
    session_id: String,
    config: ScratchpadConfig,
}

impl ScratchpadManager {
    /// Create a new ScratchpadManager for a project
    pub fn new(project_root: PathBuf, session_id: &str) -> Self {
        Self {
            project_root,
            session_id: session_id.to_string(),
            config: ScratchpadConfig::default(),
        }
    }

    /// Create with custom configuration
    pub fn with_config(project_root: PathBuf, session_id: &str, config: ScratchpadConfig) -> Self {
        Self {
            project_root,
            session_id: session_id.to_string(),
            config,
        }
    }

    /// Get the .aether directory path
    pub fn aether_dir(&self) -> PathBuf {
        self.project_root.join(&self.config.dir_name)
    }

    /// Get the scratchpad file path
    pub fn scratchpad_path(&self) -> PathBuf {
        self.aether_dir().join(&self.config.filename)
    }

    /// Get the history log path
    pub fn history_path(&self) -> PathBuf {
        self.aether_dir().join(&self.config.history_filename)
    }

    /// Ensure the .aether directory exists
    pub async fn ensure_dir(&self) -> Result<(), AetherError> {
        fs::create_dir_all(self.aether_dir())
            .await
            .map_err(|e| AetherError::other(format!("Failed to create .aether dir: {}", e)))
    }

    /// Check if scratchpad file exists
    pub async fn exists(&self) -> bool {
        self.scratchpad_path().exists()
    }

    /// Check if scratchpad has meaningful content (not just template)
    pub async fn has_content(&self) -> Result<bool, AetherError> {
        if !self.exists().await {
            return Ok(false);
        }

        let content = self.read().await?;

        // Check if it's more than just the default template
        let has_objective = !content.contains("[No active task]");
        let has_plan_items = content.contains("- [x]")
            || (content.contains("- [ ]") && !content.contains("- [ ] ..."));
        let has_working_state = {
            if let Some(pos) = content.find("## Working State") {
                let after = &content[pos..];
                if let Some(next_section) = after[16..].find("##") {
                    let working_content = &after[16..16 + next_section];
                    !working_content.trim().is_empty()
                } else {
                    false
                }
            } else {
                false
            }
        };

        Ok(has_objective || has_plan_items || has_working_state)
    }

    /// Read scratchpad content
    pub async fn read(&self) -> Result<String, AetherError> {
        fs::read_to_string(self.scratchpad_path())
            .await
            .map_err(|e| AetherError::other(format!("Failed to read scratchpad: {}", e)))
    }

    /// Write content to scratchpad (creates backup if configured)
    pub async fn write(&self, content: &str) -> Result<(), AetherError> {
        self.ensure_dir().await?;

        // Backup existing file if configured
        if self.config.backup_on_write && self.exists().await {
            let backup_path = self.scratchpad_path().with_extension("md.bak");
            if let Ok(existing) = fs::read_to_string(self.scratchpad_path()).await {
                let _ = fs::write(&backup_path, existing).await;
            }
        }

        fs::write(self.scratchpad_path(), content)
            .await
            .map_err(|e| AetherError::other(format!("Failed to write scratchpad: {}", e)))
    }

    /// Initialize scratchpad with default template
    pub async fn initialize(&self, objective: Option<&str>) -> Result<(), AetherError> {
        let content = generate_scratchpad(objective, &self.session_id);
        self.write(&content).await
    }

    /// Append a note to the Notes section
    pub async fn append_note(&self, note: &str) -> Result<(), AetherError> {
        let mut content = if self.exists().await {
            self.read().await?
        } else {
            generate_scratchpad(None, &self.session_id)
        };

        // Find Notes section and append
        if let Some(notes_pos) = content.find("## Notes") {
            let insert_pos = notes_pos + "## Notes".len();
            let timestamp = chrono::Utc::now().format("%H:%M");
            let note_line = format!("\n- [{}] {}", timestamp, note);
            content.insert_str(insert_pos, &note_line);
        }

        // Update timestamp
        content = self.update_timestamp(content);

        self.write(&content).await
    }

    /// Update the objective
    pub async fn set_objective(&self, objective: &str) -> Result<(), AetherError> {
        let mut content = if self.exists().await {
            self.read().await?
        } else {
            generate_scratchpad(Some(objective), &self.session_id)
        };

        // Replace objective
        if let Some(obj_pos) = content.find("## Objective") {
            if let Some(plan_pos) = content.find("## Plan") {
                let before = &content[..obj_pos + "## Objective".len()];
                let after = &content[plan_pos..];
                content = format!("{}\n{}\n\n{}", before, objective, after);
            }
        }

        content = self.update_timestamp(content);
        self.write(&content).await
    }

    /// Update plan items
    pub async fn set_plan(&self, items: &[&str]) -> Result<(), AetherError> {
        let mut content = if self.exists().await {
            self.read().await?
        } else {
            generate_scratchpad(None, &self.session_id)
        };

        // Build plan section
        let plan_content: String = items
            .iter()
            .map(|item| format!("- [ ] {}", item))
            .collect::<Vec<_>>()
            .join("\n");

        // Replace plan section
        if let Some(plan_pos) = content.find("## Plan") {
            if let Some(working_pos) = content.find("## Working State") {
                let before = &content[..plan_pos + "## Plan".len()];
                let after = &content[working_pos..];
                content = format!("{}\n{}\n\n{}", before, plan_content, after);
            }
        }

        content = self.update_timestamp(content);
        self.write(&content).await
    }

    /// Mark a plan item as complete
    pub async fn complete_item(&self, item_index: usize) -> Result<(), AetherError> {
        let mut content = self.read().await?;

        // Find and replace the nth "- [ ]" with "- [x]"
        let mut count = 0;
        let mut new_content = String::new();
        let mut last_end = 0;

        for (start, _) in content.match_indices("- [ ]") {
            if count == item_index {
                new_content.push_str(&content[last_end..start]);
                new_content.push_str("- [x]");
                last_end = start + 5;
                break;
            }
            count += 1;
        }

        if last_end > 0 {
            new_content.push_str(&content[last_end..]);
            content = new_content;
        }

        content = self.update_timestamp(content);
        self.write(&content).await
    }

    /// Clear scratchpad (reset to empty template)
    pub async fn clear(&self) -> Result<(), AetherError> {
        self.write(DEFAULT_TEMPLATE).await
    }

    /// Update the "Last updated" timestamp
    fn update_timestamp(&self, mut content: String) -> String {
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");

        if let Some(pos) = content.find("_Last updated:") {
            if let Some(end) = content[pos..].find("_\n") {
                let before = &content[..pos];
                let after = &content[pos + end + 2..];
                content = format!("{}_Last updated: {}_\n{}", before, now, after);
            }
        }

        content
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_manager_new_creates_directory() {
        let temp = tempdir().unwrap();
        let project_root = temp.path().to_path_buf();

        let manager = ScratchpadManager::new(project_root.clone(), "test-session");
        manager.ensure_dir().await.unwrap();

        assert!(manager.aether_dir().exists());
    }

    #[tokio::test]
    async fn test_initialize_creates_file() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::new(temp.path().to_path_buf(), "sess-123");

        manager.initialize(Some("Test objective")).await.unwrap();

        assert!(manager.exists().await);
        let content = manager.read().await.unwrap();
        assert!(content.contains("Test objective"));
        assert!(content.contains("sess-123"));
    }

    #[tokio::test]
    async fn test_has_content_empty() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::new(temp.path().to_path_buf(), "sess");

        manager.initialize(None).await.unwrap();

        assert!(!manager.has_content().await.unwrap());
    }

    #[tokio::test]
    async fn test_has_content_with_objective() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::new(temp.path().to_path_buf(), "sess");

        manager.initialize(Some("Build feature X")).await.unwrap();

        assert!(manager.has_content().await.unwrap());
    }

    #[tokio::test]
    async fn test_append_note() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::new(temp.path().to_path_buf(), "sess");

        manager.initialize(None).await.unwrap();
        manager.append_note("This is a test note").await.unwrap();

        let content = manager.read().await.unwrap();
        assert!(content.contains("This is a test note"));
    }

    #[tokio::test]
    async fn test_set_plan() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::new(temp.path().to_path_buf(), "sess");

        manager.initialize(None).await.unwrap();
        manager
            .set_plan(&["Step 1", "Step 2", "Step 3"])
            .await
            .unwrap();

        let content = manager.read().await.unwrap();
        assert!(content.contains("- [ ] Step 1"));
        assert!(content.contains("- [ ] Step 2"));
        assert!(content.contains("- [ ] Step 3"));
    }

    #[tokio::test]
    async fn test_complete_item() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::new(temp.path().to_path_buf(), "sess");

        manager.initialize(None).await.unwrap();
        manager.set_plan(&["Step 1", "Step 2"]).await.unwrap();
        manager.complete_item(0).await.unwrap();

        let content = manager.read().await.unwrap();
        assert!(content.contains("- [x] Step 1"));
        assert!(content.contains("- [ ] Step 2"));
    }

    #[tokio::test]
    async fn test_backup_on_write() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::new(temp.path().to_path_buf(), "sess");

        manager.write("First version").await.unwrap();
        manager.write("Second version").await.unwrap();

        let backup_path = manager.scratchpad_path().with_extension("md.bak");
        assert!(backup_path.exists());

        let backup = tokio::fs::read_to_string(&backup_path).await.unwrap();
        assert_eq!(backup, "First version");
    }
}
