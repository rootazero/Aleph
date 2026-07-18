//! `memory_browse` — browse the knowledge notes filesystem.
//!
//! Replaces the VFS-based browse model. Since notes are stored as real
//! markdown files at `~/.aleph/memory/note/{agent}/{category}/{filename}.md`,
//! browsing is done via filesystem operations directly.

use std::path::PathBuf;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{AlephError, Result};
use crate::tools::AlephTool;

/// Reject a `category`/`filename` segment that could escape the agent's
/// memory directory. Browsing must stay inside `memory_dir/{agent_id}/`;
/// `Path::join` with `..` traverses upward and with an absolute path replaces
/// the base, so the raw LLM-supplied segments must be validated first (R5
/// namespace isolation). Mirrors the guard in `scratchpad.rs`.
fn validate_path_component(name: &str) -> Result<()> {
    if name.is_empty()
        || name.contains("..")
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
        || name.starts_with('.')
    {
        return Err(AlephError::tool(format!(
            "Invalid path segment '{name}': must not be empty, contain path separators, \
             '..', null bytes, or start with '.'"
        )));
    }
    Ok(())
}

/// Browse action type
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MemoryBrowseAction {
    /// List categories (no path) or notes in a category (path is category name)
    List,
    /// Read a specific note's markdown content (path is "category/filename")
    Read,
}

/// Arguments for `memory_browse` tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryBrowseArgs {
    /// Action to perform: list (browse categories/files) or read (get note content).
    pub action: MemoryBrowseAction,
    /// For list: optional category name ("reference", "preference", etc.) to list files within.
    /// For read: full note path like "wiki/rust-ownership".
    #[serde(default)]
    pub path: Option<String>,
}

/// Output from `memory_browse` tool
#[derive(Debug, Clone, Serialize)]
pub struct MemoryBrowseOutput {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entries: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// Memory browse tool — filesystem-based note browser.
#[derive(Clone)]
pub struct MemoryBrowseTool {
    memory_dir: PathBuf,
    agent_id: String,
}

impl MemoryBrowseTool {
    #[must_use]
    pub const fn new(memory_dir: PathBuf, agent_id: String) -> Self {
        Self {
            memory_dir,
            agent_id,
        }
    }

    /// Per-run agent id: prefer the turn's task-local (set by the dispatch
    /// chokepoint), fall back to the construction-time field on non-scoped
    /// paths (direct calls, tests). Mirrors `MemorySearchTool::call_impl` — a
    /// process-wide single tool instance is shared across agents, so the
    /// construction-time `agent_id` is only a fallback, not the truth per run.
    fn active_agent_id(&self) -> String {
        crate::tools::turn_context::current_agent_id().unwrap_or_else(|| self.agent_id.clone())
    }

    pub(crate) async fn handle_list(&self, category: Option<&str>) -> Result<MemoryBrowseOutput> {
        let base = self.memory_dir.join(self.active_agent_id());
        let target = match category {
            None => base,
            Some(cat) => {
                validate_path_component(cat)?;
                base.join(cat)
            }
        };

        if !target.exists() {
            return Ok(MemoryBrowseOutput {
                success: true,
                message: format!("No entries at {}", category.unwrap_or("/")),
                entries: Some(Vec::new()),
                content: None,
            });
        }

        let mut entries = Vec::new();
        let mut dir = tokio::fs::read_dir(&target)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to read dir: {e}")))?;

        while let Some(entry) = dir
            .next_entry()
            .await
            .map_err(|e| AlephError::tool(format!("Read dir entry: {e}")))?
        {
            let name = entry.file_name().to_string_lossy().to_string();
            // Skip hidden files and the archive directory
            if name.starts_with('.') || name == "archive" {
                continue;
            }

            let file_type = entry
                .file_type()
                .await
                .map_err(|e| AlephError::tool(format!("Read file type: {e}")))?;

            if category.is_none() {
                // Top-level: only list directories (categories)
                if file_type.is_dir() {
                    entries.push(name);
                }
            } else {
                // Within a category: only list .md files, strip extension
                if file_type.is_file() && name.ends_with(".md") {
                    entries.push(name.trim_end_matches(".md").to_string());
                }
            }
        }
        entries.sort();

        Ok(MemoryBrowseOutput {
            success: true,
            message: format!(
                "Listed {} entries under {}",
                entries.len(),
                category.unwrap_or("/")
            ),
            entries: Some(entries),
            content: None,
        })
    }

    pub(crate) async fn handle_read(&self, path: &str) -> Result<MemoryBrowseOutput> {
        let (category, filename) = path.split_once('/').ok_or_else(|| {
            AlephError::tool("path must be 'category/filename' (e.g. 'reference/rust-ownership')")
        })?;
        validate_path_component(category)?;
        validate_path_component(filename)?;

        let file = self
            .memory_dir
            .join(self.active_agent_id())
            .join(category)
            .join(format!("{filename}.md"));

        let content = tokio::fs::read_to_string(&file)
            .await
            .map_err(|e| AlephError::tool(format!("Read note '{path}': {e}")))?;

        Ok(MemoryBrowseOutput {
            success: true,
            message: format!("Read note {path}"),
            entries: None,
            content: Some(content),
        })
    }
}

#[async_trait]
impl AlephTool for MemoryBrowseTool {
    const NAME: &'static str = "memory_browse";
    const DESCRIPTION: &'static str = "Browse the knowledge notes filesystem. \
         Action 'list' with no path shows categories; with a category name shows files in it. \
         Action 'read' with 'category/filename' returns the markdown content of that note.";

    type Args = MemoryBrowseArgs;
    type Output = MemoryBrowseOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "memory_browse(action='list')  // list all categories".to_string(),
            "memory_browse(action='list', path='reference')  // list notes in reference category"
                .to_string(),
            "memory_browse(action='read', path='wiki/rust-ownership')  // read one note"
                .to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match args.action {
            MemoryBrowseAction::List => self.handle_list(args.path.as_deref()).await,
            MemoryBrowseAction::Read => {
                let path = args
                    .path
                    .ok_or_else(|| AlephError::tool("'path' required for 'read' action"))?;
                self.handle_read(&path).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::DEFAULT_AGENT_ID;
    use tempfile::tempdir;

    async fn setup() -> (MemoryBrowseTool, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let agent = dir.path().join(DEFAULT_AGENT_ID);
        tokio::fs::create_dir_all(&agent).await.unwrap();
        let tool = MemoryBrowseTool::new(dir.path().to_path_buf(), DEFAULT_AGENT_ID.into());
        (tool, dir)
    }

    #[tokio::test]
    async fn list_empty_agent_returns_empty_entries() {
        let (tool, _dir) = setup().await;
        let result = tool.handle_list(None).await.unwrap();
        assert_eq!(result.entries.unwrap(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn list_top_level_returns_categories_only() {
        let (tool, dir) = setup().await;
        let agent_dir = dir.path().join(DEFAULT_AGENT_ID);
        tokio::fs::create_dir_all(agent_dir.join("reference"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(agent_dir.join("preference"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(agent_dir.join("archive"))
            .await
            .unwrap(); // should be skipped
        tokio::fs::write(agent_dir.join("stray.txt"), "x")
            .await
            .unwrap(); // file at top level, skipped

        let result = tool.handle_list(None).await.unwrap();
        let entries = result.entries.unwrap();
        assert_eq!(
            entries,
            vec!["preference".to_string(), "reference".to_string()]
        );
    }

    #[tokio::test]
    async fn list_category_returns_markdown_filenames() {
        let (tool, dir) = setup().await;
        let wiki = dir.path().join(format!("{DEFAULT_AGENT_ID}/wiki"));
        tokio::fs::create_dir_all(&wiki).await.unwrap();
        tokio::fs::write(wiki.join("rust.md"), "content")
            .await
            .unwrap();
        tokio::fs::write(wiki.join("go.md"), "content")
            .await
            .unwrap();
        tokio::fs::write(wiki.join("not-markdown.txt"), "x")
            .await
            .unwrap();

        let result = tool.handle_list(Some("wiki")).await.unwrap();
        let entries = result.entries.unwrap();
        assert_eq!(entries, vec!["go".to_string(), "rust".to_string()]);
    }

    #[tokio::test]
    async fn read_returns_markdown_content() {
        let (tool, dir) = setup().await;
        let wiki = dir.path().join(format!("{DEFAULT_AGENT_ID}/wiki"));
        tokio::fs::create_dir_all(&wiki).await.unwrap();
        tokio::fs::write(wiki.join("rust.md"), "# Rust\n\nSystems language")
            .await
            .unwrap();

        let result = tool.handle_read("wiki/rust").await.unwrap();
        assert!(result.content.unwrap().contains("Systems language"));
    }

    #[tokio::test]
    async fn read_invalid_path_format_errors() {
        let (tool, _dir) = setup().await;
        let result = tool.handle_read("no-slash").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn read_missing_file_errors() {
        let (tool, _dir) = setup().await;
        let result = tool.handle_read("wiki/missing").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn read_rejects_path_traversal() {
        let (tool, _dir) = setup().await;
        // `..` in either segment must be rejected before touching the filesystem.
        assert!(tool.handle_read("../../../etc/passwd").await.is_err());
        assert!(tool.handle_read("wiki/../../secret").await.is_err());
        assert!(tool.handle_read("..%2f/x").await.is_err());
    }

    #[tokio::test]
    async fn list_rejects_path_traversal() {
        let (tool, _dir) = setup().await;
        assert!(tool.handle_list(Some("../..")).await.is_err());
        assert!(tool.handle_list(Some("..")).await.is_err());
    }

    #[test]
    fn validate_component_accepts_normal_names() {
        assert!(validate_path_component("reference").is_ok());
        assert!(validate_path_component("rust-ownership").is_ok());
        assert!(validate_path_component("..").is_err());
        assert!(validate_path_component("a/b").is_err());
        assert!(validate_path_component(".hidden").is_err());
        assert!(validate_path_component("").is_err());
    }
}
