//! Web fetch tool

use serde::{Deserialize, Serialize};
use super::error::ToolError;

/// Arguments for web fetch tool
#[derive(Debug, Deserialize, Serialize)]
pub struct WebFetchArgs {
    /// URL to fetch
    pub url: String,
}

/// Web fetch result
#[derive(Debug, Serialize)]
pub struct WebFetchResult {
    pub url: String,
    pub title: Option<String>,
    pub content: String,
}

/// Web fetch tool (skeleton)
#[derive(Default)]
pub struct WebFetchTool;

impl WebFetchTool {
    pub fn new() -> Self {
        Self
    }

    /// Execute fetch (placeholder)
    pub async fn execute(&self, args: WebFetchArgs) -> Result<WebFetchResult, ToolError> {
        // Will be implemented in Phase 3
        Ok(WebFetchResult {
            url: args.url,
            title: None,
            content: String::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_web_fetch_args() {
        let args: WebFetchArgs = serde_json::from_str(r#"{"url": "https://example.com"}"#).unwrap();
        assert_eq!(args.url, "https://example.com");
    }
}
