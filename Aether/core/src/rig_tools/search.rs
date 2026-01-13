//! Web search tool

use serde::{Deserialize, Serialize};
use super::error::ToolError;

/// Arguments for search tool
#[derive(Debug, Deserialize, Serialize)]
pub struct SearchArgs {
    /// Search query
    pub query: String,
    /// Max results (default 5)
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize { 5 }

/// Search result
#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Web search tool (skeleton)
#[derive(Default)]
pub struct SearchTool;

impl SearchTool {
    pub fn new() -> Self {
        Self
    }

    /// Execute search (placeholder)
    pub async fn execute(&self, args: SearchArgs) -> Result<Vec<SearchResult>, ToolError> {
        // Will be implemented in Phase 3
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_args_default_limit() {
        let args: SearchArgs = serde_json::from_str(r#"{"query": "test"}"#).unwrap();
        assert_eq!(args.limit, 5);
    }
}
