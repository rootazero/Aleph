//! ClawHub RPC Handlers
//!
//! Handlers for ClawHub skill registry: search, browse, detail, install.

use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use super::parse_params;
use crate::builtin_tools::clawhub::{ClawHubAction, ClawHubArgs, ClawHubTool};
use crate::clawhub::types::SortOrder;
use crate::tools::AlephTool;

// =============================================================================
// Shared singleton
// =============================================================================

/// Shared `ClawHubTool` — also exposes the inner `ClawHubClient` for direct use.
fn get_tool() -> &'static ClawHubTool {
    static TOOL: OnceLock<ClawHubTool> = OnceLock::new();
    TOOL.get_or_init(ClawHubTool::new)
}

// =============================================================================
// Search
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    pub query: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    20
}

/// Search skills by keyword.
pub async fn handle_search(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: SearchParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match get_tool()
        .client()
        .search(&params.query, params.limit)
        .await
    {
        Ok(skills) => JsonRpcResponse::success(request.id, json!({ "skills": skills })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("ClawHub search failed: {}", e),
        ),
    }
}

// =============================================================================
// Browse
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct BrowseParams {
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub cursor: Option<String>,
}

/// Browse skills with sorting and pagination.
pub async fn handle_browse(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: BrowseParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let sort = match params.sort.as_deref() {
        Some("stars") => SortOrder::Stars,
        Some("updated") => SortOrder::Updated,
        Some("trending") => SortOrder::Trending,
        _ => SortOrder::Downloads,
    };

    match get_tool()
        .client()
        .browse(sort, params.limit, params.cursor.as_deref())
        .await
    {
        Ok(resp) => JsonRpcResponse::success(
            request.id,
            json!({
                "skills": resp.skills,
                "cursor": resp.cursor,
                "hasMore": resp.has_more,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("ClawHub browse failed: {}", e),
        ),
    }
}

// =============================================================================
// Detail
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct DetailParams {
    pub slug: String,
}

/// Get detailed info for a single skill.
pub async fn handle_detail(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: DetailParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match get_tool().client().get_skill(&params.slug).await {
        Ok(skill) => JsonRpcResponse::success(request.id, json!({ "skill": skill })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("ClawHub detail failed: {}", e),
        ),
    }
}

// =============================================================================
// Install
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct InstallParams {
    pub slug: String,
    #[serde(default)]
    pub version: Option<String>,
}

/// Install a skill from ClawHub by slug.
pub async fn handle_install(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: InstallParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let args = ClawHubArgs {
        action: ClawHubAction::Install,
        query: None,
        sort: None,
        limit: None,
        cursor: None,
        slug: Some(params.slug),
        version: params.version,
    };

    match get_tool().call(args).await {
        Ok(output) => JsonRpcResponse::success(
            request.id,
            json!({
                "message": output.message,
                "installedSlug": output.installed_slug,
                "installedVersion": output.installed_version,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("ClawHub install failed: {}", e),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_search_params() {
        let json = json!({"query": "web scraping"});
        let params: SearchParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.query, "web scraping");
        assert_eq!(params.limit, 20);
    }

    #[test]
    fn test_search_params_with_limit() {
        let json = json!({"query": "automation", "limit": 5});
        let params: SearchParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.query, "automation");
        assert_eq!(params.limit, 5);
    }

    #[test]
    fn test_browse_params_defaults() {
        let json = json!({});
        let params: BrowseParams = serde_json::from_value(json).unwrap();
        assert!(params.sort.is_none());
        assert_eq!(params.limit, 20);
        assert!(params.cursor.is_none());
    }

    #[test]
    fn test_browse_params_full() {
        let json = json!({"sort": "trending", "limit": 10, "cursor": "abc123"});
        let params: BrowseParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.sort.as_deref(), Some("trending"));
        assert_eq!(params.limit, 10);
        assert_eq!(params.cursor.as_deref(), Some("abc123"));
    }

    #[test]
    fn test_detail_params() {
        let json = json!({"slug": "owner/skill-name"});
        let params: DetailParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.slug, "owner/skill-name");
    }

    #[test]
    fn test_install_params() {
        let json = json!({"slug": "owner/my-skill"});
        let params: InstallParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.slug, "owner/my-skill");
        assert!(params.version.is_none());
    }

    #[test]
    fn test_install_params_with_version() {
        let json = json!({"slug": "owner/my-skill", "version": "1.2.0"});
        let params: InstallParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.slug, "owner/my-skill");
        assert_eq!(params.version.as_deref(), Some("1.2.0"));
    }
}
