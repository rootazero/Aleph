//! Atomic Operations Tool
//!
//! Provides high-level atomic operations powered by the Atomic Engine:
//! - Search: Semantic search with regex/fuzzy/AST support
//! - Replace: Batch replacement across files with preview
//! - Move: File/directory movement with import path updates

use crate::builtin_tools::error::ToolError;
use crate::engine::{
    AtomicAction, AtomicExecutor, FileFilter, SearchPattern, SearchScope,
};
use crate::error::Result;
use crate::tools::AlephTool;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::info;

/// Atomic operation type
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AtomicOperation {
    /// Search files with pattern matching
    Search,
    /// Replace text across files
    Replace,
    /// Move file or directory with import updates
    Move,
}

/// Arguments for atomic operations tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AtomicOpsArgs {
    /// The operation to perform
    pub operation: AtomicOperation,

    /// Search pattern (for search/replace operations)
    #[serde(default)]
    pub pattern: Option<String>,

    /// Pattern type: "regex", "fuzzy", or "ast"
    #[serde(default = "default_pattern_type")]
    pub pattern_type: String,

    /// Replacement text (for replace operation)
    #[serde(default)]
    pub replacement: Option<String>,

    /// Search scope: "workspace", "directory", or file path
    #[serde(default = "default_scope")]
    pub scope: String,

    /// File extension filter (e.g., "rs", "toml")
    #[serde(default)]
    pub extension: Option<String>,

    /// Source path (for move operation)
    #[serde(default)]
    pub source: Option<String>,

    /// Destination path (for move operation)
    #[serde(default)]
    pub destination: Option<String>,

    /// Update import paths after move (default: true)
    #[serde(default = "default_true")]
    pub update_imports: bool,

    /// Dry run mode (preview changes without applying)
    #[serde(default)]
    pub dry_run: bool,
}

fn default_pattern_type() -> String {
    "regex".to_string()
}

fn default_scope() -> String {
    "workspace".to_string()
}

fn default_true() -> bool {
    true
}

/// Output from atomic operations tool
#[derive(Debug, Clone, Serialize)]
pub struct AtomicOpsOutput {
    pub success: bool,
    pub operation: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matches_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replacements_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub moved_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub moved_to: Option<String>,
}

/// Atomic operations tool
#[derive(Clone)]
pub struct AtomicOpsTool {
    workspace_root: PathBuf,
}

impl AtomicOpsTool {
    /// Create a new atomic operations tool
    pub fn new(workspace_root: PathBuf) -> Self {
        Self { workspace_root }
    }

    /// Build search pattern from args
    fn build_pattern(&self, args: &AtomicOpsArgs) -> std::result::Result<SearchPattern, ToolError> {
        let pattern_str = args.pattern.as_ref()
            .ok_or_else(|| ToolError::InvalidArgs("pattern is required".to_string()))?;

        match args.pattern_type.as_str() {
            "regex" => Ok(SearchPattern::Regex {
                pattern: pattern_str.clone(),
            }),
            "fuzzy" => Ok(SearchPattern::Fuzzy {
                text: pattern_str.clone(),
                threshold: 0.8,
            }),
            "ast" => Ok(SearchPattern::Ast {
                query: pattern_str.clone(),
                language: "rust".to_string(),
            }),
            _ => Err(ToolError::InvalidArgs(format!(
                "Invalid pattern type: {}. Must be 'regex', 'fuzzy', or 'ast'",
                args.pattern_type
            ))),
        }
    }

    /// Build search scope from args
    fn build_scope(&self, args: &AtomicOpsArgs) -> SearchScope {
        match args.scope.as_str() {
            "workspace" => SearchScope::Workspace,
            "directory" => SearchScope::Directory {
                path: self.workspace_root.clone(),
                recursive: true,
            },
            path => SearchScope::File {
                path: PathBuf::from(path),
            },
        }
    }

    /// Build file filters from args
    fn build_filters(&self, args: &AtomicOpsArgs) -> Vec<FileFilter> {
        let mut filters = Vec::new();

        if let Some(ext) = &args.extension {
            filters.push(FileFilter::Extension(ext.clone()));
        }

        filters
    }

    /// Internal implementation
    async fn call_impl(&self, args: AtomicOpsArgs) -> std::result::Result<AtomicOpsOutput, ToolError> {
        use crate::builtin_tools::{notify_tool_result, notify_tool_start};

        let op_name = match &args.operation {
            AtomicOperation::Search => "搜索",
            AtomicOperation::Replace => "替换",
            AtomicOperation::Move => "移动",
        };

        let args_summary = format!("{}: {:?}", op_name, args.pattern.as_ref().or(args.source.as_ref()));
        notify_tool_start("atomic_ops", &args_summary);

        info!(
            operation = ?args.operation,
            pattern = ?args.pattern,
            scope = %args.scope,
            "AtomicOpsTool::call invoked"
        );

        let executor = AtomicExecutor::new(self.workspace_root.clone());

        let result: std::result::Result<AtomicOpsOutput, ToolError> = match args.operation {
            AtomicOperation::Search => {
                let pattern = self.build_pattern(&args)?;
                let scope = self.build_scope(&args);
                let filters = self.build_filters(&args);

                let action = AtomicAction::Search {
                    pattern,
                    scope,
                    filters,
                };

                executor.execute(&action).await
                    .map_err(|e| ToolError::Execution(format!("Search failed: {}", e)))?;

                // TODO: Parse search results from executor output
                let matches_count = 0;

                Ok(AtomicOpsOutput {
                    success: true,
                    operation: "search".to_string(),
                    message: format!("Found {} matches", matches_count),
                    matches_count: Some(matches_count),
                    replacements_count: None,
                    moved_from: None,
                    moved_to: None,
                })
            }

            AtomicOperation::Replace => {
                let pattern = self.build_pattern(&args)?;
                let replacement = args.replacement.as_ref()
                    .ok_or_else(|| ToolError::InvalidArgs("replacement is required".to_string()))?;
                let scope = self.build_scope(&args);

                let action = AtomicAction::Replace {
                    search: Box::new(pattern),
                    replacement: replacement.clone(),
                    scope,
                    preview: args.dry_run,
                    dry_run: args.dry_run,
                };

                executor.execute(&action).await
                    .map_err(|e| ToolError::Execution(format!("Replace failed: {}", e)))?;

                // TODO: Parse replacement results from executor output
                let replacements_count = 0;

                Ok(AtomicOpsOutput {
                    success: true,
                    operation: "replace".to_string(),
                    message: if args.dry_run {
                        format!("Preview: {} replacements would be made", replacements_count)
                    } else {
                        format!("Made {} replacements", replacements_count)
                    },
                    matches_count: None,
                    replacements_count: Some(replacements_count),
                    moved_from: None,
                    moved_to: None,
                })
            }

            AtomicOperation::Move => {
                let source = args.source.as_ref()
                    .ok_or_else(|| ToolError::InvalidArgs("source is required".to_string()))?;
                let destination = args.destination.as_ref()
                    .ok_or_else(|| ToolError::InvalidArgs("destination is required".to_string()))?;

                let action = AtomicAction::Move {
                    source: PathBuf::from(source),
                    destination: PathBuf::from(destination),
                    update_imports: args.update_imports,
                    create_parent: true,
                };

                executor.execute(&action).await
                    .map_err(|e| ToolError::Execution(format!("Move failed: {}", e)))?;

                Ok(AtomicOpsOutput {
                    success: true,
                    operation: "move".to_string(),
                    message: format!("Moved {} to {}", source, destination),
                    matches_count: None,
                    replacements_count: None,
                    moved_from: Some(source.clone()),
                    moved_to: Some(destination.clone()),
                })
            }
        };

        // Notify result
        match &result {
            Ok(output) => {
                notify_tool_result("atomic_ops", &output.message, output.success);
            }
            Err(e) => {
                notify_tool_result("atomic_ops", &e.to_string(), false);
            }
        }

        result
    }
}

impl Default for AtomicOpsTool {
    fn default() -> Self {
        Self::new(PathBuf::from("."))
    }
}

#[async_trait]
impl AlephTool for AtomicOpsTool {
    const NAME: &'static str = "atomic_ops";
    const DESCRIPTION: &'static str = r#"Perform atomic operations powered by the Atomic Engine:
- search: Search files with regex/fuzzy/AST pattern matching
- replace: Batch replace text across files with preview/dry-run support
- move: Move files/directories with automatic import path updates (Rust)

PATTERN TYPES:
- regex: Regular expression pattern (default)
- fuzzy: Fuzzy text matching with threshold
- ast: AST-level code search (language-aware)

SCOPE:
- workspace: Search entire workspace (default)
- directory: Search in workspace root directory
- <path>: Search in specific file

EXAMPLES:
- Search: {"operation": "search", "pattern": "TODO", "scope": "workspace"}
- Replace: {"operation": "replace", "pattern": "old", "replacement": "new", "dry_run": true}
- Move: {"operation": "move", "source": "old.rs", "destination": "new.rs", "update_imports": true}"#;

    type Args = AtomicOpsArgs;
    type Output = AtomicOpsOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use tokio::fs;

    #[tokio::test]
    async fn test_search_operation() {
        let dir = tempdir().unwrap();
        let tool = AtomicOpsTool::new(dir.path().to_path_buf());

        // Create test file
        let test_file = dir.path().join("test.rs");
        fs::write(&test_file, "fn main() {\n    println!(\"TODO: implement\");\n}").await.unwrap();

        let args = AtomicOpsArgs {
            operation: AtomicOperation::Search,
            pattern: Some("TODO".to_string()),
            pattern_type: "regex".to_string(),
            replacement: None,
            scope: "workspace".to_string(),
            extension: Some("rs".to_string()),
            source: None,
            destination: None,
            update_imports: true,
            dry_run: false,
        };

        let result = tool.call_impl(args).await.unwrap();
        assert!(result.success);
        assert_eq!(result.operation, "search");
    }

    #[tokio::test]
    async fn test_move_operation() {
        let dir = tempdir().unwrap();
        let tool = AtomicOpsTool::new(dir.path().to_path_buf());

        // Create test file
        let source = dir.path().join("old.rs");
        fs::write(&source, "fn test() {}").await.unwrap();

        let destination = dir.path().join("new.rs");

        let args = AtomicOpsArgs {
            operation: AtomicOperation::Move,
            pattern: None,
            pattern_type: "regex".to_string(),
            replacement: None,
            scope: "workspace".to_string(),
            extension: None,
            source: Some(source.to_string_lossy().to_string()),
            destination: Some(destination.to_string_lossy().to_string()),
            update_imports: true,
            dry_run: false,
        };

        let result = tool.call_impl(args).await.unwrap();
        assert!(result.success);
        assert_eq!(result.operation, "move");
        assert!(destination.exists());
        assert!(!source.exists());
    }
}
