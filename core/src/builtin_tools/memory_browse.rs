//! Memory browse tool for hierarchical VFS navigation
//!
//! Provides ls/read/glob operations on the aleph:// VFS.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

use super::error::ToolError;
use crate::error::Result;
use crate::memory::namespace::NamespaceScope;
use crate::memory::store::{MemoryBackend, MemoryStore, PathEntry as StorePathEntry};
use crate::memory::workspace::WorkspaceFilter;
use crate::memory::{FactSource, MemoryFact, MemoryLayer, SearchFilter, DEFAULT_WORKSPACE};
use crate::tools::AlephTool;

/// Browse action type
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum BrowseAction {
    /// List direct children of a path
    Ls,
    /// Read full content of a specific fact
    Read,
    /// Pattern-match search under a path
    Glob,
}

/// Arguments for memory_browse tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MemoryBrowseArgs {
    /// The action to perform (ls, read, glob)
    pub action: BrowseAction,
    /// The aleph:// path to operate on
    pub path: String,
    /// Glob pattern (only used with glob action)
    #[serde(default)]
    pub pattern: Option<String>,
    /// Workspace to browse in. If omitted, uses the active workspace from execution context.
    #[serde(default)]
    pub workspace: Option<String>,
}

/// A single directory entry
#[derive(Debug, Clone, Serialize)]
pub struct BrowseLsEntry {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub fact_count: usize,
    pub l1_available: bool,
    pub abstract_line: String,
}

/// Output from memory_browse tool
#[derive(Debug, Clone, Serialize)]
pub struct MemoryBrowseOutput {
    /// The action that was performed
    pub action: String,
    /// The path that was browsed
    pub path: String,
    /// Directory listing (for ls action)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entries: Option<Vec<BrowseLsEntry>>,
    /// Content (for read action)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Metadata (for read action)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ReadMetadata>,
    /// Glob matches (for glob action)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matches: Option<Vec<GlobMatch>>,
    /// Human-readable summary
    pub summary: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReadMetadata {
    pub fact_type: String,
    pub fact_source: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct GlobMatch {
    pub path: String,
    pub abstract_line: String,
}

/// Memory browse tool
pub struct MemoryBrowseTool {
    database: MemoryBackend,
    /// Shared default workspace ID, set by the execution engine based on active workspace.
    /// Falls back to DEFAULT_WORKSPACE ("default") when not set.
    default_workspace: Arc<RwLock<String>>,
}

impl MemoryBrowseTool {
    pub fn new(database: MemoryBackend) -> Self {
        Self {
            database,
            default_workspace: Arc::new(RwLock::new(DEFAULT_WORKSPACE.to_string())),
        }
    }

    /// Get a shared handle to the default workspace setting.
    ///
    /// The execution engine can update this value when the active workspace changes,
    /// so that tool calls without an explicit `workspace` arg use the correct workspace.
    pub fn default_workspace_handle(&self) -> Arc<RwLock<String>> {
        Arc::clone(&self.default_workspace)
    }

    /// Replace the workspace handle with an externally-shared one.
    ///
    /// Used by BuiltinToolRegistry to share a single workspace handle across
    /// both memory_search and memory_browse tools.
    pub fn set_workspace_handle(&mut self, handle: Arc<RwLock<String>>) {
        self.default_workspace = handle;
    }

    async fn call_impl(
        &self,
        args: MemoryBrowseArgs,
    ) -> std::result::Result<MemoryBrowseOutput, ToolError> {
        use super::{notify_tool_result, notify_tool_start};

        // Resolve workspace: explicit arg > default_workspace (set by execution engine) > DEFAULT_WORKSPACE
        let default_ws = self.default_workspace.read().await;
        let workspace = args.workspace.as_deref().unwrap_or(&default_ws);

        // Validate path
        if !args.path.starts_with("aleph://") {
            return Err(ToolError::Execution(
                format!("Invalid path: must start with aleph://, got: {}", args.path)
            ));
        }

        let action_name = match args.action {
            BrowseAction::Ls => "ls",
            BrowseAction::Read => "read",
            BrowseAction::Glob => "glob",
        };

        notify_tool_start("memory_browse", &format!("{} {}", action_name, args.path));

        let output = match args.action {
            BrowseAction::Ls => self.handle_ls(&args.path, workspace).await?,
            BrowseAction::Read => self.handle_read(&args.path, workspace).await?,
            BrowseAction::Glob => {
                let pattern = args.pattern.as_deref().unwrap_or("*");
                self.handle_glob(&args.path, pattern, workspace).await?
            }
        };

        notify_tool_result("memory_browse", &output.summary, true);
        Ok(output)
    }

    async fn handle_ls(&self, path: &str, workspace: &str) -> std::result::Result<MemoryBrowseOutput, ToolError> {
        let children: Vec<StorePathEntry> = self.database
            .list_by_path(path, &NamespaceScope::Owner, workspace)
            .await
            .map_err(|e| ToolError::Execution(format!("Failed to list path: {}", e)))?;

        let mut entries = Vec::with_capacity(children.len());
        for child in children {
            let summary = self.summary_fact_for_path(&child.path, workspace).await?;
            let l1_available = summary
                .as_ref()
                .is_some_and(|f| f.layer == MemoryLayer::L1Overview);
            let abstract_line = summary
                .as_ref()
                .map(|f| Self::extract_abstract_line(&f.content))
                .unwrap_or_default();

            entries.push(BrowseLsEntry {
                name: child.path.strip_prefix(path).unwrap_or(&child.path).to_string(),
                path: child.path.clone(),
                is_directory: !child.is_leaf,
                fact_count: child.child_count,
                l1_available,
                abstract_line,
            });
        }

        let summary = format!("{} - {} entries", path, entries.len());

        Ok(MemoryBrowseOutput {
            action: "ls".to_string(),
            path: path.to_string(),
            entries: Some(entries),
            content: None,
            metadata: None,
            matches: None,
            summary,
        })
    }

    async fn handle_read(&self, path: &str, workspace: &str) -> std::result::Result<MemoryBrowseOutput, ToolError> {
        if let Some(summary_fact) = self.summary_fact_for_path(path, workspace).await? {
            return Ok(MemoryBrowseOutput {
                action: "read".to_string(),
                path: path.to_string(),
                entries: None,
                content: Some(summary_fact.content.clone()),
                metadata: Some(ReadMetadata {
                    fact_type: summary_fact.fact_type.to_string(),
                    fact_source: summary_fact.fact_source.to_string(),
                    created_at: summary_fact.created_at,
                    updated_at: summary_fact.updated_at,
                    confidence: summary_fact.confidence,
                }),
                matches: None,
                summary: format!("{} - {} summary", path, summary_fact.layer),
            });
        }

        // Otherwise return all L2 detail facts under this prefix.
        let facts = self.database
            .get_facts_by_path_prefix(path, &self.detail_filter(workspace), 500)
            .await
            .map_err(|e| ToolError::Execution(format!("Failed to read path: {}", e)))?;

        if facts.is_empty() {
            return Ok(MemoryBrowseOutput {
                action: "read".to_string(),
                path: path.to_string(),
                entries: None,
                content: Some("No content at this path.".to_string()),
                metadata: None,
                matches: None,
                summary: format!("{} - empty", path),
            });
        }

        let combined_content = facts.iter()
            .map(|f| format!("- [{}] {}", f.fact_type, f.content))
            .collect::<Vec<_>>()
            .join("\n");

        // When multiple facts, aggregate metadata
        let metadata = if facts.len() == 1 {
            let f = &facts[0];
            ReadMetadata {
                fact_type: f.fact_type.to_string(),
                fact_source: f.fact_source.to_string(),
                created_at: f.created_at,
                updated_at: f.updated_at,
                confidence: f.confidence,
            }
        } else {
            ReadMetadata {
                fact_type: "Mixed".to_string(),
                fact_source: "Multiple".to_string(),
                created_at: facts.iter().map(|f| f.created_at).min().unwrap_or(0),
                updated_at: facts.iter().map(|f| f.updated_at).max().unwrap_or(0),
                confidence: facts.iter().map(|f| f.confidence).sum::<f32>() / facts.len() as f32,
            }
        };

        Ok(MemoryBrowseOutput {
            action: "read".to_string(),
            path: path.to_string(),
            entries: None,
            content: Some(combined_content),
            metadata: Some(metadata),
            matches: None,
            summary: format!("{} - {} facts", path, facts.len()),
        })
    }

    async fn handle_glob(&self, path: &str, pattern: &str, workspace: &str) -> std::result::Result<MemoryBrowseOutput, ToolError> {
        let path_facts = self.database
            .get_facts_by_path_prefix(path, &self.detail_filter(workspace), 1000)
            .await
            .map_err(|e| ToolError::Execution(format!("Failed to glob: {}", e)))?;

        let matches: Vec<GlobMatch> = path_facts.into_iter()
            .filter(|f| {
                let relative = f.path.strip_prefix(path).unwrap_or(&f.path);
                if pattern == "*" {
                    true
                } else {
                    relative.contains(pattern.trim_matches('*'))
                }
            })
            .map(|f| GlobMatch {
                path: f.path.clone(),
                abstract_line: f.content.chars().take(100).collect(),
            })
            .collect();

        let summary = format!("{} - {} matches for '{}'", path, matches.len(), pattern);

        Ok(MemoryBrowseOutput {
            action: "glob".to_string(),
            path: path.to_string(),
            entries: None,
            content: None,
            metadata: None,
            matches: Some(matches),
            summary,
        })
    }

    fn detail_filter(&self, workspace: &str) -> SearchFilter {
        SearchFilter::new()
            .with_valid_only()
            .with_workspace(WorkspaceFilter::Single(workspace.to_string()))
            .with_layer(MemoryLayer::L2Detail)
    }

    async fn summary_fact_for_path(
        &self,
        path: &str,
        workspace: &str,
    ) -> std::result::Result<Option<MemoryFact>, ToolError> {
        let filter = SearchFilter::new()
            .with_valid_only()
            .with_workspace(WorkspaceFilter::Single(workspace.to_string()));

        let mut summaries: Vec<MemoryFact> = self
            .database
            .get_facts_by_path_prefix(path, &filter, 128)
            .await
            .map_err(|e| ToolError::Execution(format!("Failed to read summaries: {}", e)))?
            .into_iter()
            .filter(|f| f.path == path && f.fact_source == FactSource::Summary)
            .collect();

        summaries.sort_by(|a, b| {
            Self::summary_layer_rank(a.layer)
                .cmp(&Self::summary_layer_rank(b.layer))
                .then_with(|| b.updated_at.cmp(&a.updated_at))
        });

        Ok(summaries.into_iter().next())
    }

    fn summary_layer_rank(layer: MemoryLayer) -> u8 {
        match layer {
            MemoryLayer::L1Overview => 0,
            MemoryLayer::L0Abstract => 1,
            MemoryLayer::L2Detail => 2,
        }
    }

    fn extract_abstract_line(content: &str) -> String {
        content
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or_default()
            .chars()
            .take(120)
            .collect()
    }
}

impl Clone for MemoryBrowseTool {
    fn clone(&self) -> Self {
        Self {
            database: self.database.clone(),
            default_workspace: self.default_workspace.clone(),
        }
    }
}

#[async_trait]
impl AlephTool for MemoryBrowseTool {
    const NAME: &'static str = "memory_browse";
    const DESCRIPTION: &'static str = "Browse hierarchical memory using aleph:// paths. \
        Supports ls (list directory), read (get content), and glob (pattern search). \
        Use after memory_search discovers relevant paths.";

    type Args = MemoryBrowseArgs;
    type Output = MemoryBrowseOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "memory_browse(action='ls', path='aleph://user/preferences/')".to_string(),
            "memory_browse(action='read', path='aleph://user/preferences/coding/')".to_string(),
            "memory_browse(action='glob', path='aleph://knowledge/', pattern='*rust*')".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use crate::sync_primitives::Arc;

    use crate::memory::store::lance::LanceMemoryBackend;
    use crate::memory::store::MemoryBackend;
    use crate::memory::{FactType, MemoryFact};

    use super::*;

    async fn create_test_db() -> (MemoryBackend, tempfile::TempDir) {
        let temp_dir = tempfile::tempdir().unwrap();
        let backend = LanceMemoryBackend::open_or_create(temp_dir.path()).await.unwrap();
        (Arc::new(backend), temp_dir)
    }

    #[test]
    fn test_browse_args_serialization() {
        let args = MemoryBrowseArgs {
            action: BrowseAction::Ls,
            path: "aleph://user/".to_string(),
            pattern: None,
            workspace: None,
        };
        let json = serde_json::to_string(&args).unwrap();
        assert!(json.contains("aleph://user/"));
        assert!(json.contains("ls"));
    }

    #[test]
    fn test_invalid_path_detection() {
        assert!(!"/invalid/path".starts_with("aleph://"));
        assert!("aleph://user/".starts_with("aleph://"));
    }

    #[test]
    fn test_browse_output_serialization() {
        let output = MemoryBrowseOutput {
            action: "ls".to_string(),
            path: "aleph://user/".to_string(),
            entries: Some(vec![BrowseLsEntry {
                name: "preferences/".to_string(),
                path: "aleph://user/preferences/".to_string(),
                is_directory: true,
                fact_count: 5,
                l1_available: true,
                abstract_line: "User coding preferences".to_string(),
            }]),
            content: None,
            metadata: None,
            matches: None,
            summary: "aleph://user/ - 1 entries".to_string(),
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("preferences/"));
        // content should be absent (skip_serializing_if = None)
        assert!(!json.contains("\"content\""));
    }

    #[tokio::test]
    async fn test_memory_browse_ls_marks_l1_available() {
        let (db, _temp_dir) = create_test_db().await;
        let tool = MemoryBrowseTool::new(db.clone());

        let detail_fact = MemoryFact::new("User likes Rust".into(), FactType::Preference, vec![])
            .with_path("aleph://user/preferences/coding/".to_string())
            .with_layer(MemoryLayer::L2Detail);

        let summary_fact = MemoryFact::new("Coding overview\n- Rust".into(), FactType::Other, vec![])
            .with_path("aleph://user/preferences/coding/".to_string())
            .with_fact_source(FactSource::Summary)
            .with_layer(MemoryLayer::L1Overview);

        db.insert_fact(&detail_fact).await.unwrap();
        db.insert_fact(&summary_fact).await.unwrap();

        let output = tool
            .handle_ls("aleph://user/preferences/", "default")
            .await
            .unwrap();

        let entries = output.entries.unwrap();
        let coding = entries
            .iter()
            .find(|entry| entry.path == "aleph://user/preferences/coding/")
            .unwrap();

        assert!(coding.l1_available);
        assert!(!coding.abstract_line.is_empty());
    }
}
