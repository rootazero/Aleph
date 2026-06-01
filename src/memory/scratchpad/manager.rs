// core/src/memory/scratchpad/manager.rs

//! Scratchpad Manager
//!
//! Manages the lifecycle of agent scratchpad files stored under
//! `~/.aleph/workspaces/<agent_id>/` — see
//! [`default_workspace_root`] for the resolution rule. The first arg to
//! [`Self::new`] is the on-disk subdirectory name (historically called
//! `project_id`); in the unified agent model this is the agent id.
//!
//! Per-run project overrides (the desktop App's "进入项目工作" flow,
//! see [`crate::projects`]) do NOT relocate the scratchpad — runtime
//! working memory stays bound to the agent, so a single agent's
//! scratchpad survives a user toggling between project folders.
//!
//! [`default_workspace_root`]: crate::config::agent_resolver::default_workspace_root

use crate::error::AlephError;
use std::path::PathBuf;
use tokio::fs;

use super::template::{generate_scratchpad, DEFAULT_TEMPLATE};

/// Configuration for scratchpad behavior
#[derive(Debug, Clone)]
pub struct ScratchpadConfig {
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
            filename: "scratchpad.md".to_string(),
            history_filename: "session_history.log".to_string(),
            backup_on_write: true,
        }
    }
}

/// A single plan item parsed from the scratchpad's `## Plan` section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanItem {
    /// Item text (the part after the `- [ ]` / `- [x]` marker).
    pub text: String,
    /// `true` when the checkbox is `[x]`.
    pub done: bool,
}

/// Structural snapshot of a scratchpad's objective + plan, used by the
/// goal-loop hook. Carries no judgment — just the parsed checkbox state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScratchpadSnapshot {
    /// The objective text, or `None` when unset / still `[No active task]`.
    pub objective: Option<String>,
    /// Plan items in document order (excludes the `- [ ] ...` placeholder).
    pub items: Vec<PlanItem>,
}

impl ScratchpadSnapshot {
    /// Plan items whose checkbox is still `[ ]`.
    pub fn incomplete(&self) -> Vec<&PlanItem> {
        self.items.iter().filter(|i| !i.done).collect()
    }

    /// `true` when an objective is set AND at least one real plan item is
    /// still unchecked. This is the structural condition the goal-loop
    /// hook fires on — not a semantic completion judgment.
    pub fn has_pending_work(&self) -> bool {
        self.objective.is_some() && self.items.iter().any(|i| !i.done)
    }
}

/// Manages agent scratchpad files under `~/.aleph/workspaces/<agent_id>/`.
pub struct ScratchpadManager {
    /// Base directory for this project's scratchpad files
    project_dir: PathBuf,
    session_id: String,
    config: ScratchpadConfig,
}

impl ScratchpadManager {
    /// Create a new ScratchpadManager for an agent workspace
    ///
    /// Files are stored under `~/.aleph/workspaces/<agent_id>/`.
    /// Falls back to a temp-style path for testing.
    pub fn new(project_id: &str, session_id: &str) -> Self {
        let project_dir = crate::config::agent_resolver::default_workspace_root().join(project_id);

        Self {
            project_dir,
            session_id: session_id.to_string(),
            config: ScratchpadConfig::default(),
        }
    }

    /// Create with an explicit base directory (for testing)
    pub fn with_dir(project_dir: PathBuf, session_id: &str) -> Self {
        Self {
            project_dir,
            session_id: session_id.to_string(),
            config: ScratchpadConfig::default(),
        }
    }

    /// Create with custom configuration
    pub fn with_config(project_dir: PathBuf, session_id: &str, config: ScratchpadConfig) -> Self {
        Self {
            project_dir,
            session_id: session_id.to_string(),
            config,
        }
    }

    /// Get the project directory path
    pub fn project_dir(&self) -> &PathBuf {
        &self.project_dir
    }

    /// Get the scratchpad file path
    pub fn scratchpad_path(&self) -> PathBuf {
        self.project_dir.join(&self.config.filename)
    }

    /// Get the history log path
    pub fn history_path(&self) -> PathBuf {
        self.project_dir.join(&self.config.history_filename)
    }

    /// Ensure the project directory exists
    pub async fn ensure_dir(&self) -> Result<(), AlephError> {
        fs::create_dir_all(&self.project_dir)
            .await
            .map_err(|e| AlephError::other(format!("Failed to create project dir: {}", e)))
    }

    /// Check if scratchpad file exists
    pub fn exists(&self) -> bool {
        self.scratchpad_path().exists()
    }

    /// Check if scratchpad has meaningful content (not just template)
    pub async fn has_content(&self) -> Result<bool, AlephError> {
        if !self.exists() {
            return Ok(false);
        }

        let content = self.read().await?;

        // Check if it's more than just the default template
        let has_objective = !content.contains("[No active task]");
        let has_plan_items = content.contains("- [x]")
            || (content.contains("- [ ]") && !content.contains("- [ ] ..."));
        let has_working_state = {
            const HEADER: &str = "## Working State";
            if let Some(pos) = content.find(HEADER) {
                let after = &content[pos..];
                if let Some(next_section) = after[HEADER.len()..].find("##") {
                    let working_content = &after[HEADER.len()..HEADER.len() + next_section];
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

    /// Parse the objective + plan checkboxes into a [`ScratchpadSnapshot`].
    ///
    /// Pure structural read — uses the same markers as [`Self::has_content`]
    /// (`## Objective` / `## Plan` sections, `- [ ]` / `- [x]` checkboxes,
    /// skipping the `- [ ] ...` placeholder). Returns an empty snapshot when
    /// no scratchpad file exists.
    pub async fn snapshot(&self) -> Result<ScratchpadSnapshot, AlephError> {
        if !self.exists() {
            return Ok(ScratchpadSnapshot::default());
        }
        Ok(parse_snapshot(&self.read().await?))
    }

    /// Read scratchpad content
    pub async fn read(&self) -> Result<String, AlephError> {
        fs::read_to_string(self.scratchpad_path())
            .await
            .map_err(|e| AlephError::other(format!("Failed to read scratchpad: {}", e)))
    }

    /// Write content to scratchpad (creates backup if configured)
    pub async fn write(&self, content: &str) -> Result<(), AlephError> {
        self.ensure_dir().await?;

        // Backup existing file if configured
        if self.config.backup_on_write && self.exists() {
            let backup_path = self.scratchpad_path().with_extension("md.bak");
            if let Ok(existing) = fs::read_to_string(self.scratchpad_path()).await {
                let _ = fs::write(&backup_path, existing).await;
            }
        }

        fs::write(self.scratchpad_path(), content)
            .await
            .map_err(|e| AlephError::other(format!("Failed to write scratchpad: {}", e)))
    }

    /// Initialize scratchpad with default template
    pub async fn initialize(&self, objective: Option<&str>) -> Result<(), AlephError> {
        let content = generate_scratchpad(objective, &self.session_id);
        self.write(&content).await
    }

    /// Append a note to the Notes section
    pub async fn append_note(&self, note: &str) -> Result<(), AlephError> {
        let mut content = if self.exists() {
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
    pub async fn set_objective(&self, objective: &str) -> Result<(), AlephError> {
        let mut content = if self.exists() {
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
    pub async fn set_plan(&self, items: &[&str]) -> Result<(), AlephError> {
        let mut content = if self.exists() {
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
    pub async fn complete_item(&self, item_index: usize) -> Result<(), AlephError> {
        let mut content = self.read().await?;

        // Find and replace the nth "- [ ]" with "- [x]"
        let mut new_content = String::new();
        let mut last_end = 0;

        for (count, (start, _)) in content.match_indices("- [ ]").enumerate() {
            if count == item_index {
                new_content.push_str(&content[last_end..start]);
                new_content.push_str("- [x]");
                last_end = start + 5;
                break;
            }
        }

        if last_end > 0 {
            new_content.push_str(&content[last_end..]);
            content = new_content;
        }

        content = self.update_timestamp(content);
        self.write(&content).await
    }

    /// Clear scratchpad (reset to empty template)
    pub async fn clear(&self) -> Result<(), AlephError> {
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

/// Return the trimmed text of the markdown section between `header` and the
/// next `## ` heading (or end of document).
fn extract_section<'a>(content: &'a str, header: &str) -> Option<&'a str> {
    let start = content.find(header)? + header.len();
    let rest = &content[start..];
    let end = rest.find("\n## ").unwrap_or(rest.len());
    Some(rest[..end].trim())
}

/// Parse objective + plan checkboxes out of raw scratchpad markdown.
///
/// Free function (no I/O) so it is trivially unit-testable. Mirrors the
/// marker conventions of [`ScratchpadManager::has_content`].
pub(crate) fn parse_snapshot(content: &str) -> ScratchpadSnapshot {
    let objective = extract_section(content, "## Objective")
        .map(str::trim)
        .filter(|o| !o.is_empty() && *o != "[No active task]")
        .map(str::to_string);

    let items = extract_section(content, "## Plan")
        .map(|plan| {
            plan.lines()
                .filter_map(|line| {
                    let line = line.trim();
                    if let Some(text) = line.strip_prefix("- [ ] ") {
                        Some((text.trim(), false))
                    } else if let Some(text) = line.strip_prefix("- [x] ") {
                        Some((text.trim(), true))
                    } else {
                        None
                    }
                })
                // Drop the default `- [ ] ...` placeholder.
                .filter(|(text, done)| !(!*done && *text == "..."))
                .map(|(text, done)| PlanItem {
                    text: text.to_string(),
                    done,
                })
                .collect::<Vec<PlanItem>>()
        })
        .unwrap_or_default();

    ScratchpadSnapshot { objective, items }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parse_snapshot_empty_template_has_no_pending_work() {
        let snap = parse_snapshot(DEFAULT_TEMPLATE);
        assert_eq!(snap.objective, None);
        assert!(snap.items.is_empty(), "placeholder must be skipped");
        assert!(!snap.has_pending_work());
    }

    #[test]
    fn parse_snapshot_objective_plus_mixed_checkboxes() {
        let md = "# Current Task\n\n## Objective\nShip auth\n\n## Plan\n- [x] Design API\n- [ ] Implement\n- [ ] Test\n\n## Working State\n\n## Notes\n";
        let snap = parse_snapshot(md);
        assert_eq!(snap.objective.as_deref(), Some("Ship auth"));
        assert_eq!(snap.items.len(), 3);
        let pending = snap.incomplete();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].text, "Implement");
        assert!(snap.has_pending_work());
    }

    #[test]
    fn parse_snapshot_all_done_has_no_pending_work() {
        let md = "## Objective\nDone goal\n\n## Plan\n- [x] A\n- [x] B\n\n## Working State\n";
        let snap = parse_snapshot(md);
        assert_eq!(snap.objective.as_deref(), Some("Done goal"));
        assert!(!snap.has_pending_work(), "all boxes checked → no pending");
    }

    #[test]
    fn parse_snapshot_plan_without_objective_does_not_fire() {
        // Items present but objective never set → hook stays dormant.
        let md = "## Objective\n[No active task]\n\n## Plan\n- [ ] orphan step\n\n## Working State\n";
        let snap = parse_snapshot(md);
        assert_eq!(snap.objective, None);
        assert!(!snap.has_pending_work());
    }

    #[tokio::test]
    async fn test_manager_creates_directory() {
        let temp = tempdir().unwrap();
        let project_dir = temp.path().join("test-project");

        let manager = ScratchpadManager::with_dir(project_dir.clone(), "test-session");
        manager.ensure_dir().await.unwrap();

        assert!(manager.project_dir().exists());
    }

    #[tokio::test]
    async fn test_initialize_creates_file() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess-123");

        manager.initialize(Some("Test objective")).await.unwrap();

        assert!(manager.exists());
        let content = manager.read().await.unwrap();
        assert!(content.contains("Test objective"));
        assert!(content.contains("sess-123"));
    }

    #[tokio::test]
    async fn test_has_content_empty() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess");

        manager.initialize(None).await.unwrap();

        assert!(!manager.has_content().await.unwrap());
    }

    #[tokio::test]
    async fn test_has_content_with_objective() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess");

        manager.initialize(Some("Build feature X")).await.unwrap();

        assert!(manager.has_content().await.unwrap());
    }

    #[tokio::test]
    async fn test_append_note() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess");

        manager.initialize(None).await.unwrap();
        manager.append_note("This is a test note").await.unwrap();

        let content = manager.read().await.unwrap();
        assert!(content.contains("This is a test note"));
    }

    #[tokio::test]
    async fn test_set_plan() {
        let temp = tempdir().unwrap();
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess");

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
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess");

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
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess");

        manager.write("First version").await.unwrap();
        manager.write("Second version").await.unwrap();

        let backup_path = manager.scratchpad_path().with_extension("md.bak");
        assert!(backup_path.exists());

        let backup = tokio::fs::read_to_string(&backup_path).await.unwrap();
        assert_eq!(backup, "First version");
    }
}
