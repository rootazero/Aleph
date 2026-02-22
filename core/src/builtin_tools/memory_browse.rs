//! Memory browse tool for hierarchical VFS navigation
//!
//! Provides ls/read/glob operations on the aleph:// VFS.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::error::ToolError;
use crate::error::Result;
use crate::memory::VectorDatabase;
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
    database: Arc<VectorDatabase>,
}

impl MemoryBrowseTool {
    pub fn new(database: Arc<VectorDatabase>) -> Self {
        Self { database }
    }

    async fn call_impl(
        &self,
        args: MemoryBrowseArgs,
    ) -> std::result::Result<MemoryBrowseOutput, ToolError> {
        use super::{notify_tool_result, notify_tool_start};

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
            BrowseAction::Ls => self.handle_ls(&args.path).await?,
            BrowseAction::Read => self.handle_read(&args.path).await?,
            BrowseAction::Glob => {
                let pattern = args.pattern.as_deref().unwrap_or("*");
                self.handle_glob(&args.path, pattern).await?
            }
        };

        notify_tool_result("memory_browse", &output.summary, true);
        Ok(output)
    }

    async fn handle_ls(&self, path: &str) -> std::result::Result<MemoryBrowseOutput, ToolError> {
        let children = self.database.list_path_children(path).await
            .map_err(|e| ToolError::Execution(format!("Failed to list path: {}", e)))?;

        let entries: Vec<BrowseLsEntry> = children.into_iter().map(|c| BrowseLsEntry {
            name: c.name,
            path: c.full_path,
            is_directory: c.is_directory,
            fact_count: c.fact_count,
            l1_available: c.has_l1,
            abstract_line: c.abstract_line,
        }).collect();

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

    async fn handle_read(&self, path: &str) -> std::result::Result<MemoryBrowseOutput, ToolError> {
        // First try L1 overview
        if let Ok(Some(l1)) = self.database.get_l1_overview(path).await {
            return Ok(MemoryBrowseOutput {
                action: "read".to_string(),
                path: path.to_string(),
                entries: None,
                content: Some(l1.content.clone()),
                metadata: Some(ReadMetadata {
                    fact_type: l1.fact_type.to_string(),
                    fact_source: l1.fact_source.to_string(),
                    created_at: l1.created_at,
                    updated_at: l1.updated_at,
                    confidence: l1.confidence,
                }),
                matches: None,
                summary: format!("{} - L1 Overview", path),
            });
        }

        // Otherwise return all facts at this path
        let facts = self.database.get_facts_by_path_prefix(path).await
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

    async fn handle_glob(&self, path: &str, pattern: &str) -> std::result::Result<MemoryBrowseOutput, ToolError> {
        let all_facts = self.database.get_facts_by_path_prefix(path).await
            .map_err(|e| ToolError::Execution(format!("Failed to glob: {}", e)))?;

        let matches: Vec<GlobMatch> = all_facts.into_iter()
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
}

impl Clone for MemoryBrowseTool {
    fn clone(&self) -> Self {
        Self {
            database: self.database.clone(),
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
    use super::*;

    #[test]
    fn test_browse_args_serialization() {
        let args = MemoryBrowseArgs {
            action: BrowseAction::Ls,
            path: "aleph://user/".to_string(),
            pattern: None,
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
}
