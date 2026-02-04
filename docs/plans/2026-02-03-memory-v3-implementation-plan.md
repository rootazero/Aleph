# Memory System v3 "Glass Box" Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement the Memory v3 "Glass Box" architecture, enabling observable, intervenable, and explainable memory management for CLI scenarios.

**Architecture:** Introduce a three-plane architecture (Data/Control/Management) with Session Scratchpad for working memory, Lazy Decay for read-time strength calculation, Hybrid Trigger for compression safety, and CLI tools for direct user intervention.

**Tech Stack:** Rust, SQLite, tokio, fs2 (file locking), serde, chrono

---

## Milestone 1: Scratchpad Foundation

### Task 1.1: Create Scratchpad Module Structure

**Files:**
- Create: `core/src/memory/scratchpad/mod.rs`
- Create: `core/src/memory/scratchpad/template.rs`
- Modify: `core/src/memory/mod.rs:36` (add module export)

**Step 1: Create scratchpad directory**

```bash
mkdir -p core/src/memory/scratchpad
```

**Step 2: Write template.rs with Scratchpad Markdown template**

```rust
// core/src/memory/scratchpad/template.rs

//! Scratchpad Markdown templates

/// Default scratchpad template for new sessions
pub const DEFAULT_TEMPLATE: &str = r#"# Current Task

## Objective
[No active task]

## Plan
- [ ] ...

## Working State


## Notes


---
_Last updated: _
_Session: _
"#;

/// Generate a scratchpad with populated metadata
pub fn generate_scratchpad(objective: Option<&str>, session_id: &str) -> String {
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    let obj = objective.unwrap_or("[No active task]");

    format!(
        r#"# Current Task

## Objective
{}

## Plan
- [ ] ...

## Working State


## Notes


---
_Last updated: {}_
_Session: {}_
"#,
        obj, now, session_id
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_template_has_sections() {
        assert!(DEFAULT_TEMPLATE.contains("## Objective"));
        assert!(DEFAULT_TEMPLATE.contains("## Plan"));
        assert!(DEFAULT_TEMPLATE.contains("## Working State"));
        assert!(DEFAULT_TEMPLATE.contains("## Notes"));
    }

    #[test]
    fn test_generate_scratchpad_with_objective() {
        let result = generate_scratchpad(Some("Build auth module"), "sess-123");
        assert!(result.contains("Build auth module"));
        assert!(result.contains("sess-123"));
    }
}
```

**Step 3: Write mod.rs for scratchpad module**

```rust
// core/src/memory/scratchpad/mod.rs

//! Session Scratchpad Module
//!
//! Provides working memory for active tasks, stored as project-local
//! Markdown files that are immune to compression.

pub mod template;
mod manager;
mod history;

pub use manager::ScratchpadManager;
pub use history::SessionHistory;
pub use template::{DEFAULT_TEMPLATE, generate_scratchpad};
```

**Step 4: Update memory/mod.rs to export scratchpad**

Add after line 35 in `core/src/memory/mod.rs`:
```rust
pub mod scratchpad;
```

And add to re-exports section:
```rust
pub use scratchpad::{ScratchpadManager, SessionHistory};
```

**Step 5: Run cargo check**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo check
```
Expected: Compilation errors about missing manager.rs and history.rs (we'll create them next)

**Step 6: Commit**

```bash
git add core/src/memory/scratchpad/ core/src/memory/mod.rs
git commit -m "feat(memory): add scratchpad module structure and template"
```

---

### Task 1.2: Implement ScratchpadManager

**Files:**
- Create: `core/src/memory/scratchpad/manager.rs`
- Test: `core/src/memory/scratchpad/manager.rs` (inline tests)

**Step 1: Write the test for ScratchpadManager::new**

```rust
// core/src/memory/scratchpad/manager.rs

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_manager_new_creates_directory() {
        let temp = tempdir().unwrap();
        let project_root = temp.path().to_path_buf();

        let manager = ScratchpadManager::new(project_root.clone(), "test-session");

        // .aether directory should exist after initialization
        assert!(manager.aether_dir().exists() || manager.ensure_dir().await.is_ok());
    }
}
```

**Step 2: Write ScratchpadManager struct and new()**

```rust
// core/src/memory/scratchpad/manager.rs

//! Scratchpad Manager
//!
//! Manages the lifecycle of project-local scratchpad files.

use crate::error::AlephError;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;

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
    pub async fn ensure_dir(&self) -> Result<(), AlephError> {
        fs::create_dir_all(self.aether_dir())
            .await
            .map_err(|e| AlephError::other(format!("Failed to create .aether dir: {}", e)))
    }

    /// Check if scratchpad file exists
    pub async fn exists(&self) -> bool {
        self.scratchpad_path().exists()
    }

    /// Check if scratchpad has meaningful content (not just template)
    pub async fn has_content(&self) -> Result<bool, AlephError> {
        if !self.exists().await {
            return Ok(false);
        }

        let content = self.read().await?;

        // Check if it's more than just the default template
        let has_objective = !content.contains("[No active task]");
        let has_plan_items = content.contains("- [x]") ||
                           (content.contains("- [ ]") && !content.contains("- [ ] ..."));
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
    pub async fn read(&self) -> Result<String, AlephError> {
        fs::read_to_string(self.scratchpad_path())
            .await
            .map_err(|e| AlephError::other(format!("Failed to read scratchpad: {}", e)))
    }

    /// Write content to scratchpad (creates backup if configured)
    pub async fn write(&self, content: &str) -> Result<(), AlephError> {
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
            .map_err(|e| AlephError::other(format!("Failed to write scratchpad: {}", e)))
    }

    /// Initialize scratchpad with default template
    pub async fn initialize(&self, objective: Option<&str>) -> Result<(), AlephError> {
        let content = generate_scratchpad(objective, &self.session_id);
        self.write(&content).await
    }

    /// Append a note to the Notes section
    pub async fn append_note(&self, note: &str) -> Result<(), AlephError> {
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
    pub async fn set_objective(&self, objective: &str) -> Result<(), AlephError> {
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
    pub async fn set_plan(&self, items: &[&str]) -> Result<(), AlephError> {
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
    pub async fn complete_item(&self, item_index: usize) -> Result<(), AlephError> {
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
        manager.set_plan(&["Step 1", "Step 2", "Step 3"]).await.unwrap();

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
```

**Step 3: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo test scratchpad --lib
```
Expected: All tests pass

**Step 4: Commit**

```bash
git add core/src/memory/scratchpad/manager.rs
git commit -m "feat(memory): implement ScratchpadManager with CRUD operations"
```

---

### Task 1.3: Implement SessionHistory

**Files:**
- Create: `core/src/memory/scratchpad/history.rs`

**Step 1: Write the test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_append_and_read() {
        let temp = tempdir().unwrap();
        let history = SessionHistory::new(temp.path().join("history.log"));

        history.append("First entry", "sess-1").await.unwrap();
        history.append("Second entry", "sess-2").await.unwrap();

        let entries = history.read_recent(10).await.unwrap();
        assert_eq!(entries.len(), 2);
    }
}
```

**Step 2: Implement SessionHistory**

```rust
// core/src/memory/scratchpad/history.rs

//! Session History Log
//!
//! Archives completed scratchpad content for traceability.

use crate::error::AlephError;
use std::path::PathBuf;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;

/// Manages the session history log file
pub struct SessionHistory {
    path: PathBuf,
}

/// A parsed history entry
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub timestamp: String,
    pub session_id: String,
    pub content: String,
}

impl SessionHistory {
    /// Create a new SessionHistory manager
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Append content to the history log
    pub async fn append(&self, content: &str, session_id: &str) -> Result<(), AlephError> {
        // Ensure parent directory exists
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| AlephError::other(format!("Failed to create history dir: {}", e)))?;
        }

        let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S");
        let entry = format!(
            "\n--- Archived: {} (Session: {}) ---\n{}\n",
            timestamp, session_id, content
        );

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await
            .map_err(|e| AlephError::other(format!("Failed to open history file: {}", e)))?;

        file.write_all(entry.as_bytes())
            .await
            .map_err(|e| AlephError::other(format!("Failed to write history: {}", e)))?;

        Ok(())
    }

    /// Read recent history entries
    pub async fn read_recent(&self, max_entries: usize) -> Result<Vec<HistoryEntry>, AlephError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(&self.path)
            .await
            .map_err(|e| AlephError::other(format!("Failed to read history: {}", e)))?;

        let entries: Vec<HistoryEntry> = content
            .split("\n--- Archived:")
            .skip(1) // Skip empty first split
            .filter_map(|entry| self.parse_entry(entry))
            .collect();

        // Return most recent entries
        let start = entries.len().saturating_sub(max_entries);
        Ok(entries[start..].to_vec())
    }

    /// Parse a single history entry
    fn parse_entry(&self, raw: &str) -> Option<HistoryEntry> {
        let lines: Vec<&str> = raw.lines().collect();
        if lines.is_empty() {
            return None;
        }

        // First line format: " TIMESTAMP (Session: ID) ---"
        let header = lines[0];
        let timestamp_end = header.find(" (Session:")?;
        let timestamp = header[1..timestamp_end].trim().to_string();

        let session_start = header.find("Session:")? + 8;
        let session_end = header.find(") ---")?;
        let session_id = header[session_start..session_end].trim().to_string();

        let content = lines[1..].join("\n").trim().to_string();

        Some(HistoryEntry {
            timestamp,
            session_id,
            content,
        })
    }

    /// Get total size of history file
    pub async fn size_bytes(&self) -> Result<u64, AlephError> {
        if !self.path.exists() {
            return Ok(0);
        }

        let metadata = fs::metadata(&self.path)
            .await
            .map_err(|e| AlephError::other(format!("Failed to get history metadata: {}", e)))?;

        Ok(metadata.len())
    }

    /// Rotate history if it exceeds max size
    pub async fn rotate_if_needed(&self, max_size_bytes: u64) -> Result<bool, AlephError> {
        let current_size = self.size_bytes().await?;

        if current_size <= max_size_bytes {
            return Ok(false);
        }

        // Rename current file to .old
        let old_path = self.path.with_extension("log.old");
        if old_path.exists() {
            fs::remove_file(&old_path)
                .await
                .map_err(|e| AlephError::other(format!("Failed to remove old history: {}", e)))?;
        }

        fs::rename(&self.path, &old_path)
            .await
            .map_err(|e| AlephError::other(format!("Failed to rotate history: {}", e)))?;

        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_append_and_read() {
        let temp = tempdir().unwrap();
        let history = SessionHistory::new(temp.path().join("history.log"));

        history.append("First entry", "sess-1").await.unwrap();
        history.append("Second entry", "sess-2").await.unwrap();

        let entries = history.read_recent(10).await.unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].session_id, "sess-1");
        assert_eq!(entries[1].session_id, "sess-2");
    }

    #[tokio::test]
    async fn test_read_empty() {
        let temp = tempdir().unwrap();
        let history = SessionHistory::new(temp.path().join("nonexistent.log"));

        let entries = history.read_recent(10).await.unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn test_size_bytes() {
        let temp = tempdir().unwrap();
        let history = SessionHistory::new(temp.path().join("history.log"));

        assert_eq!(history.size_bytes().await.unwrap(), 0);

        history.append("Some content here", "sess").await.unwrap();

        assert!(history.size_bytes().await.unwrap() > 0);
    }

    #[tokio::test]
    async fn test_rotate() {
        let temp = tempdir().unwrap();
        let history = SessionHistory::new(temp.path().join("history.log"));

        // Write enough to exceed 100 bytes
        history.append("A".repeat(100).as_str(), "sess").await.unwrap();

        let rotated = history.rotate_if_needed(50).await.unwrap();
        assert!(rotated);

        // Old file should exist
        assert!(temp.path().join("history.log.old").exists());
    }
}
```

**Step 3: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo test history --lib
```
Expected: All tests pass

**Step 4: Commit**

```bash
git add core/src/memory/scratchpad/history.rs
git commit -m "feat(memory): implement SessionHistory for scratchpad archival"
```

---

### Task 1.4: Final Integration and Module Cleanup

**Files:**
- Modify: `core/src/memory/scratchpad/mod.rs`
- Modify: `core/src/memory/mod.rs`

**Step 1: Update scratchpad/mod.rs with proper exports**

```rust
// core/src/memory/scratchpad/mod.rs

//! Session Scratchpad Module
//!
//! Provides working memory for active tasks, stored as project-local
//! Markdown files that are immune to compression.
//!
//! ## Architecture
//!
//! - **scratchpad.md**: Current active task state
//! - **session_history.log**: Archive of completed tasks
//!
//! ## Usage
//!
//! ```rust,ignore
//! let manager = ScratchpadManager::new(project_root, "session-id");
//! manager.initialize(Some("Build auth module")).await?;
//! manager.set_plan(&["Design API", "Implement", "Test"]).await?;
//! manager.complete_item(0).await?;
//! ```

pub mod template;
mod manager;
mod history;

pub use manager::{ScratchpadManager, ScratchpadConfig};
pub use history::{SessionHistory, HistoryEntry};
pub use template::{DEFAULT_TEMPLATE, generate_scratchpad};
```

**Step 2: Update memory/mod.rs exports**

Ensure the exports are correct:
```rust
// Add to re-exports section in core/src/memory/mod.rs
pub use scratchpad::{ScratchpadManager, ScratchpadConfig, SessionHistory};
```

**Step 3: Run full test suite**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo test memory::scratchpad
```
Expected: All tests pass

**Step 4: Run cargo clippy**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo clippy --lib -- -D warnings
```
Expected: No warnings

**Step 5: Commit Milestone 1**

```bash
git add -A
git commit -m "feat(memory): complete Milestone 1 - Scratchpad Foundation

- ScratchpadManager for project-local .aether/scratchpad.md
- SessionHistory for archiving completed tasks
- Markdown templates with sections: Objective, Plan, Working State, Notes
- Backup on write for crash recovery
- Full test coverage"
```

---

## Milestone 2: Lazy Decay Engine

### Task 2.1: Add decay_invalidated_at Field to Database

**Files:**
- Modify: `core/src/memory/database/core.rs:94-107`
- Modify: `core/src/memory/database/facts/crud.rs`
- Modify: `core/src/memory/context.rs:253-281`

**Step 1: Write test for new field**

```rust
// Add to core/src/memory/database/facts/tests.rs or inline

#[tokio::test]
async fn test_decay_invalidated_at_field() {
    let db = create_test_db().await;

    let mut fact = MemoryFact::new(
        "Test fact".to_string(),
        FactType::Other,
        vec![],
    );

    db.insert_fact(fact.clone()).await.unwrap();

    // Soft delete with decay
    let now = chrono::Utc::now().timestamp();
    db.soft_delete_fact(&fact.id, "decay", Some(now)).await.unwrap();

    // Verify field was set
    let retrieved = db.get_fact_including_invalid(&fact.id).await.unwrap();
    assert_eq!(retrieved.decay_invalidated_at, Some(now));
}
```

**Step 2: Update MemoryFact struct in context.rs**

Add after line 273 in `core/src/memory/context.rs`:
```rust
    /// Timestamp when fact was invalidated due to decay (Unix seconds)
    /// Used for recycle bin retention period
    pub decay_invalidated_at: Option<i64>,
```

Update the `new()` and `with_id()` methods to initialize this field to `None`.

**Step 3: Update database schema in core.rs**

Add to the `memory_facts` table schema after line 106:
```sql
                decay_invalidated_at INTEGER,
```

Add index after line 116:
```sql
            -- Index for decay invalidation queries (recycle bin)
            CREATE INDEX IF NOT EXISTS idx_facts_decay_invalidated
                ON memory_facts(decay_invalidated_at)
                WHERE decay_invalidated_at IS NOT NULL;
```

**Step 4: Update insert_fact in crud.rs**

Update the INSERT statement to include the new field:
```rust
            r#"
            INSERT INTO memory_facts (
                id, content, fact_type, embedding, source_memory_ids,
                created_at, updated_at, confidence, is_valid, invalidation_reason,
                specificity, temporal_scope, decay_invalidated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            "#,
            params![
                // ... existing params ...
                fact.decay_invalidated_at,
            ],
```

**Step 5: Add soft_delete_fact method**

```rust
impl VectorDatabase {
    /// Soft delete a fact with optional decay timestamp
    pub async fn soft_delete_fact(
        &self,
        fact_id: &str,
        reason: &str,
        decay_timestamp: Option<i64>,
    ) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        conn.execute(
            r#"
            UPDATE memory_facts
            SET is_valid = 0,
                invalidation_reason = ?2,
                updated_at = ?3,
                decay_invalidated_at = ?4
            WHERE id = ?1
            "#,
            params![fact_id, reason, now, decay_timestamp],
        )
        .map_err(|e| AlephError::config(format!("Failed to soft delete fact: {}", e)))?;

        Ok(())
    }
}
```

**Step 6: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo test facts --lib
```

**Step 7: Commit**

```bash
git add core/src/memory/database/ core/src/memory/context.rs
git commit -m "feat(memory): add decay_invalidated_at field for recycle bin"
```

---

### Task 2.2: Implement Lazy Decay Calculator

**Files:**
- Modify: `core/src/memory/decay.rs`

**Step 1: Write test for type-aware decay**

```rust
#[test]
fn test_ephemeral_decays_faster() {
    let config = DecayConfig::default();
    let now = 1000000;
    let fifteen_days_ago = now - (15 * 86400);

    let strength = MemoryStrength {
        access_count: 0,
        last_accessed: fifteen_days_ago,
        creation_time: fifteen_days_ago,
    };

    // Normal type: ~0.71 after 15 days (half of half-life)
    let normal_score = strength.calculate_strength_for_type(&config, now, &FactType::Other);

    // Ephemeral: ~0.5 after 15 days (full half-life with 0.5x multiplier)
    let ephemeral_score = strength.calculate_strength_for_type(&config, now, &FactType::Ephemeral);

    assert!(ephemeral_score < normal_score);
}
```

**Step 2: Add calculate_strength_for_type method**

```rust
impl MemoryStrength {
    /// Calculate strength with type-specific half-life
    pub fn calculate_strength_for_type(
        &self,
        config: &DecayConfig,
        now: i64,
        fact_type: &FactType,
    ) -> f32 {
        // Protected types never decay
        if config.is_protected(fact_type) {
            return 1.0;
        }

        let effective_half_life = config.effective_half_life(fact_type);
        let days_since_access = (now - self.last_accessed) as f32 / 86400.0;

        // Handle infinite half-life
        if effective_half_life.is_infinite() {
            return 1.0;
        }

        let base_decay = 0.5_f32.powf(days_since_access / effective_half_life);
        let access_boost = (self.access_count as f32 * config.access_boost).min(2.0);

        (base_decay * (1.0 + access_boost)).min(1.0)
    }
}
```

**Step 3: Add Ephemeral handling to DecayConfig**

Update `effective_half_life` method:
```rust
    pub fn effective_half_life(&self, fact_type: &FactType) -> f32 {
        match fact_type {
            FactType::Preference => self.half_life_days * 2.0,
            FactType::Personal => f32::INFINITY,
            // Note: We check TemporalScope::Ephemeral separately, not FactType
            _ => self.half_life_days,
        }
    }

    /// Get effective half-life considering temporal scope
    pub fn effective_half_life_with_scope(
        &self,
        fact_type: &FactType,
        temporal_scope: &TemporalScope,
    ) -> f32 {
        let base = self.effective_half_life(fact_type);

        if base.is_infinite() {
            return base;
        }

        match temporal_scope {
            TemporalScope::Ephemeral => base * 0.5,
            TemporalScope::Permanent => base * 3.0,
            TemporalScope::Contextual => base,
        }
    }
```

**Step 4: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo test decay --lib
```

**Step 5: Commit**

```bash
git add core/src/memory/decay.rs
git commit -m "feat(memory): add type-aware decay calculation with temporal scope"
```

---

### Task 2.3: Implement Lazy Decay in Retrieval

**Files:**
- Create: `core/src/memory/lazy_decay.rs`
- Modify: `core/src/memory/mod.rs`
- Modify: `core/src/memory/fact_retrieval.rs`

**Step 1: Create lazy_decay.rs module**

```rust
// core/src/memory/lazy_decay.rs

//! Lazy Decay Engine
//!
//! Calculates memory strength at read-time and asynchronously
//! invalidates decayed facts without blocking retrieval.

use crate::error::AlephError;
use crate::memory::context::{FactType, MemoryFact, TemporalScope};
use crate::memory::database::VectorDatabase;
use crate::memory::decay::{DecayConfig, MemoryStrength};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Result of lazy decay evaluation
#[derive(Debug)]
pub struct DecayEvaluation {
    /// Facts that passed decay check (still valid)
    pub valid_facts: Vec<MemoryFact>,
    /// Fact IDs that should be invalidated
    pub pending_invalidations: Vec<String>,
    /// Access updates to apply
    pub pending_access_updates: Vec<(String, i64)>, // (fact_id, timestamp)
}

/// Lazy decay processor
pub struct LazyDecayEngine {
    config: DecayConfig,
    db: Arc<VectorDatabase>,
    /// Channel for async invalidation tasks
    invalidation_tx: mpsc::Sender<InvalidationTask>,
}

struct InvalidationTask {
    fact_id: String,
    timestamp: i64,
}

impl LazyDecayEngine {
    /// Create a new lazy decay engine
    pub fn new(config: DecayConfig, db: Arc<VectorDatabase>) -> Self {
        let (tx, mut rx) = mpsc::channel::<InvalidationTask>(100);

        // Spawn background task for async invalidations
        let db_clone = db.clone();
        tokio::spawn(async move {
            while let Some(task) = rx.recv().await {
                if let Err(e) = db_clone
                    .soft_delete_fact(&task.fact_id, "decay", Some(task.timestamp))
                    .await
                {
                    tracing::warn!(
                        fact_id = %task.fact_id,
                        error = %e,
                        "Failed to invalidate decayed fact"
                    );
                }
            }
        });

        Self {
            config,
            db,
            invalidation_tx: tx,
        }
    }

    /// Evaluate decay for a batch of facts
    ///
    /// Returns valid facts and queues invalidations asynchronously.
    pub async fn evaluate(&self, facts: Vec<MemoryFact>) -> DecayEvaluation {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let mut valid_facts = Vec::new();
        let mut pending_invalidations = Vec::new();
        let mut pending_access_updates = Vec::new();

        for fact in facts {
            // Skip already invalid facts
            if !fact.is_valid {
                continue;
            }

            // Calculate current strength
            let strength = MemoryStrength {
                access_count: 0, // TODO: track in fact
                last_accessed: fact.updated_at,
                creation_time: fact.created_at,
            };

            let current_strength = strength.calculate_strength_for_type(
                &self.config,
                now,
                &fact.fact_type,
            );

            if current_strength < self.config.min_strength {
                // Queue for async invalidation
                pending_invalidations.push(fact.id.clone());

                let _ = self.invalidation_tx.send(InvalidationTask {
                    fact_id: fact.id.clone(),
                    timestamp: now,
                }).await;
            } else {
                // Valid fact - queue access update
                pending_access_updates.push((fact.id.clone(), now));
                valid_facts.push(fact);
            }
        }

        DecayEvaluation {
            valid_facts,
            pending_invalidations,
            pending_access_updates,
        }
    }

    /// Batch update access timestamps (call after retrieval completes)
    pub async fn apply_access_updates(&self, updates: Vec<(String, i64)>) -> Result<(), AlephError> {
        for (fact_id, timestamp) in updates {
            self.db.update_fact_access(&fact_id, timestamp).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests require database setup - see integration_tests.rs
}
```

**Step 2: Add update_fact_access to VectorDatabase**

```rust
impl VectorDatabase {
    /// Update fact access timestamp
    pub async fn update_fact_access(&self, fact_id: &str, timestamp: i64) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        conn.execute(
            "UPDATE memory_facts SET updated_at = ?2 WHERE id = ?1",
            params![fact_id, timestamp],
        )
        .map_err(|e| AlephError::config(format!("Failed to update fact access: {}", e)))?;

        Ok(())
    }
}
```

**Step 3: Update mod.rs**

Add to `core/src/memory/mod.rs`:
```rust
pub mod lazy_decay;
pub use lazy_decay::{LazyDecayEngine, DecayEvaluation};
```

**Step 4: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo test lazy_decay --lib
```

**Step 5: Commit**

```bash
git add core/src/memory/lazy_decay.rs core/src/memory/mod.rs core/src/memory/database/
git commit -m "feat(memory): implement LazyDecayEngine for read-time decay evaluation"
```

---

## Milestone 3: Hybrid Trigger

### Task 3.1: Implement CompressionTrigger

**Files:**
- Create: `core/src/memory/compression/trigger.rs`
- Modify: `core/src/memory/compression/mod.rs`

**Step 1: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_threshold_trigger() {
        let config = TriggerConfig::default();
        let trigger = CompressionTrigger::new(config.clone());

        // Below threshold - no trigger
        let result = trigger.check_tokens(100_000, 128_000);
        assert!(result.is_none());

        // Above threshold - triggers
        let result = trigger.check_tokens(120_000, 128_000);
        assert!(matches!(result, Some(TriggerReason::TokenThreshold { .. })));
    }

    #[test]
    fn test_signal_trigger() {
        let config = TriggerConfig::default();
        let trigger = CompressionTrigger::new(config);

        let signal = CompressionSignal::Milestone {
            task_description: "Build auth".to_string(),
            completion_indicator: "done".to_string(),
        };

        let result = trigger.check_signal(Some(signal.clone()), 50_000, 128_000);
        assert!(matches!(result, Some(TriggerReason::Signal(_))));
    }

    #[test]
    fn test_both_trigger() {
        let config = TriggerConfig::default();
        let trigger = CompressionTrigger::new(config);

        let signal = CompressionSignal::Milestone {
            task_description: "Build auth".to_string(),
            completion_indicator: "done".to_string(),
        };

        let result = trigger.check_signal(Some(signal), 120_000, 128_000);
        assert!(matches!(result, Some(TriggerReason::Both { .. })));
    }
}
```

**Step 2: Implement trigger.rs**

```rust
// core/src/memory/compression/trigger.rs

//! Hybrid Compression Trigger
//!
//! Combines signal-based smart triggering with token threshold safety net.

use super::signal_detector::CompressionSignal;
use serde::{Deserialize, Serialize};

/// Configuration for compression triggering
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerConfig {
    /// Maximum token window size
    pub max_token_window: usize,
    /// Trigger threshold as fraction (0.9 = 90%)
    pub trigger_threshold: f32,
    /// Target after compression as fraction (0.5 = 50%)
    pub target_after_compression: f32,
    /// Whether signal detection is enabled
    pub signal_detection_enabled: bool,
    /// Recent turns to keep during aggressive compression
    pub keep_recent_turns: usize,
}

impl Default for TriggerConfig {
    fn default() -> Self {
        Self {
            max_token_window: 128_000,
            trigger_threshold: 0.9,
            target_after_compression: 0.5,
            signal_detection_enabled: true,
            keep_recent_turns: 5,
        }
    }
}

/// Reason for triggering compression
#[derive(Debug, Clone)]
pub enum TriggerReason {
    /// Triggered by signal detection
    Signal(CompressionSignal),
    /// Triggered by token threshold
    TokenThreshold {
        current: usize,
        max: usize,
    },
    /// Both signal and threshold
    Both {
        signal: CompressionSignal,
        tokens: usize,
    },
}

impl TriggerReason {
    /// Check if this was a safety net trigger (not signal-driven)
    pub fn is_safety_net(&self) -> bool {
        matches!(self, TriggerReason::TokenThreshold { .. })
    }

    /// Get compression aggressiveness
    pub fn aggressiveness(&self) -> CompressionAggressiveness {
        match self {
            TriggerReason::Signal(CompressionSignal::Milestone { .. }) => {
                CompressionAggressiveness::Full
            }
            TriggerReason::Signal(CompressionSignal::ContextSwitch { .. }) => {
                CompressionAggressiveness::TopicOnly
            }
            TriggerReason::TokenThreshold { .. } => {
                CompressionAggressiveness::Aggressive
            }
            TriggerReason::Both { .. } => {
                CompressionAggressiveness::Full
            }
            _ => CompressionAggressiveness::Normal,
        }
    }
}

/// How aggressively to compress
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionAggressiveness {
    /// Normal compression with semantic boundaries
    Normal,
    /// Full compression + archive scratchpad
    Full,
    /// Only compress old topic
    TopicOnly,
    /// Emergency: keep only recent N turns
    Aggressive,
}

/// Hybrid compression trigger
pub struct CompressionTrigger {
    config: TriggerConfig,
}

impl CompressionTrigger {
    /// Create a new trigger with configuration
    pub fn new(config: TriggerConfig) -> Self {
        Self { config }
    }

    /// Check if compression should be triggered
    pub fn check(
        &self,
        signal: Option<CompressionSignal>,
        current_tokens: usize,
    ) -> Option<TriggerReason> {
        self.check_signal(signal, current_tokens, self.config.max_token_window)
    }

    /// Check with explicit max tokens
    pub fn check_signal(
        &self,
        signal: Option<CompressionSignal>,
        current_tokens: usize,
        max_tokens: usize,
    ) -> Option<TriggerReason> {
        let threshold = (max_tokens as f32 * self.config.trigger_threshold) as usize;
        let over_threshold = current_tokens > threshold;

        match (signal, over_threshold) {
            (Some(s), true) => Some(TriggerReason::Both {
                signal: s,
                tokens: current_tokens,
            }),
            (Some(s), false) if self.config.signal_detection_enabled => {
                Some(TriggerReason::Signal(s))
            }
            (None, true) => Some(TriggerReason::TokenThreshold {
                current: current_tokens,
                max: threshold,
            }),
            _ => None,
        }
    }

    /// Check tokens only (bypass signal detection)
    pub fn check_tokens(&self, current_tokens: usize, max_tokens: usize) -> Option<TriggerReason> {
        let threshold = (max_tokens as f32 * self.config.trigger_threshold) as usize;

        if current_tokens > threshold {
            Some(TriggerReason::TokenThreshold {
                current: current_tokens,
                max: threshold,
            })
        } else {
            None
        }
    }

    /// Get configuration
    pub fn config(&self) -> &TriggerConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_threshold_trigger() {
        let config = TriggerConfig::default();
        let trigger = CompressionTrigger::new(config.clone());

        let result = trigger.check_tokens(100_000, 128_000);
        assert!(result.is_none());

        let result = trigger.check_tokens(120_000, 128_000);
        assert!(matches!(result, Some(TriggerReason::TokenThreshold { .. })));
    }

    #[test]
    fn test_signal_trigger() {
        let config = TriggerConfig::default();
        let trigger = CompressionTrigger::new(config);

        let signal = CompressionSignal::Milestone {
            task_description: "Build auth".to_string(),
            completion_indicator: "done".to_string(),
        };

        let result = trigger.check_signal(Some(signal.clone()), 50_000, 128_000);
        assert!(matches!(result, Some(TriggerReason::Signal(_))));
    }

    #[test]
    fn test_both_trigger() {
        let config = TriggerConfig::default();
        let trigger = CompressionTrigger::new(config);

        let signal = CompressionSignal::Milestone {
            task_description: "Build auth".to_string(),
            completion_indicator: "done".to_string(),
        };

        let result = trigger.check_signal(Some(signal), 120_000, 128_000);
        assert!(matches!(result, Some(TriggerReason::Both { .. })));
    }

    #[test]
    fn test_aggressiveness() {
        let milestone = TriggerReason::Signal(CompressionSignal::Milestone {
            task_description: "test".to_string(),
            completion_indicator: "done".to_string(),
        });
        assert_eq!(milestone.aggressiveness(), CompressionAggressiveness::Full);

        let threshold = TriggerReason::TokenThreshold { current: 100, max: 90 };
        assert_eq!(threshold.aggressiveness(), CompressionAggressiveness::Aggressive);
    }

    #[test]
    fn test_is_safety_net() {
        let threshold = TriggerReason::TokenThreshold { current: 100, max: 90 };
        assert!(threshold.is_safety_net());

        let signal = TriggerReason::Signal(CompressionSignal::Milestone {
            task_description: "test".to_string(),
            completion_indicator: "done".to_string(),
        });
        assert!(!signal.is_safety_net());
    }
}
```

**Step 3: Update compression/mod.rs**

Add to exports:
```rust
mod trigger;
pub use trigger::{CompressionTrigger, TriggerConfig, TriggerReason, CompressionAggressiveness};
```

**Step 4: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo test trigger --lib
```

**Step 5: Commit**

```bash
git add core/src/memory/compression/trigger.rs core/src/memory/compression/mod.rs
git commit -m "feat(memory): implement Hybrid Trigger with token threshold safety net"
```

---

## Remaining Milestones (Summary)

Due to document length, the remaining milestones are summarized. Each follows the same TDD pattern.

### Milestone 4: Archival Pipeline
- Task 4.1: Create `archival.rs` (refactor from `service.rs`)
- Task 4.2: Implement `archive_scratchpad()` method
- Task 4.3: Integrate with SignalDetector

### Milestone 5: CLI Tools
- Task 5.1: Create `cli/` module structure
- Task 5.2: Implement file locking (`lock.rs`)
- Task 5.3: Implement `list` command
- Task 5.4: Implement `add`, `edit`, `forget`, `restore` commands
- Task 5.5: Implement `gc` command
- Task 5.6: Implement `dump`/`import` commands

### Milestone 6: Explainability
- Task 6.1: Create audit log table schema
- Task 6.2: Implement `AuditLogger`
- Task 6.3: Implement `explain` command
- Task 6.4: Integrate audit logging throughout system

---

## Testing Commands

```bash
# Run all memory tests
cd /Volumes/TBU4/Workspace/Aether/core && cargo test memory --lib

# Run with output
cargo test memory --lib -- --nocapture

# Run specific module
cargo test memory::scratchpad --lib
cargo test memory::lazy_decay --lib
cargo test memory::compression::trigger --lib

# Clippy check
cargo clippy --lib -- -D warnings

# Build check
cargo build --lib
```

---

## Verification Checklist

After completing all milestones:

- [ ] All tests pass: `cargo test memory --lib`
- [ ] No clippy warnings: `cargo clippy --lib -- -D warnings`
- [ ] Scratchpad creates `.aether/scratchpad.md` in project directory
- [ ] Lazy decay filters facts at read time
- [ ] Hybrid trigger fires at 90% token threshold
- [ ] CLI commands work with file locking
- [ ] Audit log records all memory operations

---

*Plan created: 2026-02-03*
*Design reference: docs/plans/2026-02-03-memory-v3-glass-box-design.md*
