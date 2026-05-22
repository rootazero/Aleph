//! Integration coverage for `tools.catalog` / `tools.effective` RPC handlers.
//!
//! Asserts (Spec 1):
//! - `handle_catalog` respects the `source` filter param (P2).
//! - `handle_effective` respects the live agent registry's allowlist (D1).
//! - `handle_effective` returns `agent_id` field for the resolved agent.
//!
//! Uses a real `tool_metadata::ToolCatalog` seeded by hand so we don't depend
//! on the builtin-tools wiring (which has its own integration tests).
//!
//! `tools.invoke` is covered exhaustively by the unit tests at
//! `src/gateway/handlers/tools_invoke.rs#tests` (4 gate tests + 5 legacy);
//! a duplicate integration here would only re-test the same code path.

use alephcore::agents::{AgentDef, AgentMode, AgentRegistry};
use alephcore::tool_metadata::{ToolCatalog, ToolSource, UnifiedTool};
use alephcore::gateway::handlers::tools_visibility::{handle_catalog, handle_effective};
use alephcore::gateway::protocol::JsonRpcRequest;
use serde_json::json;

/// Build a registry seeded with one tool per source family used by the
/// `extract_source` mapping. Mirrors the unit-test fixture but goes through
/// the real `ToolCatalog` so we exercise the public surface.
async fn registry_with_mixed_sources() -> ToolCatalog {
    let reg = ToolCatalog::new();
    let tools = vec![
        UnifiedTool::new("native:search", "search", "Search", ToolSource::Native),
        UnifiedTool::new("builtin:help", "help", "Help", ToolSource::Builtin),
        UnifiedTool::new(
            "mcp:github:status",
            "github_status",
            "GH status",
            ToolSource::Mcp {
                server: "github".into(),
            },
        ),
        UnifiedTool::new(
            "mcp:filesystem:ls",
            "fs_ls",
            "List files",
            ToolSource::Mcp {
                server: "filesystem".into(),
            },
        ),
        UnifiedTool::new(
            "skill:refine-text:refine",
            "refine",
            "Refine text",
            ToolSource::Skill {
                id: "refine-text".into(),
            },
        ),
    ];
    for t in tools {
        reg.register_with_conflict_resolution(t).await;
    }
    reg
}

#[tokio::test]
async fn catalog_returns_all_when_no_filter() {
    let reg = registry_with_mixed_sources().await;
    let req = JsonRpcRequest::with_id("tools.catalog", None, json!(1));
    let resp = handle_catalog(req, &reg).await;
    assert!(resp.is_success(), "expected success: {:?}", resp.error);
    let result = resp.result.unwrap();
    assert_eq!(result["total"], 5);
    // Five distinct group ids: native, builtin, mcp:github, mcp:filesystem, skill:refine-text
    assert_eq!(result["groups"].as_array().unwrap().len(), 5);
}

#[tokio::test]
async fn catalog_source_filter_exact_match() {
    let reg = registry_with_mixed_sources().await;
    let req = JsonRpcRequest::with_id("tools.catalog", Some(json!({"source": "native"})), json!(1));
    let resp = handle_catalog(req, &reg).await;
    let result = resp.result.unwrap();
    assert_eq!(result["total"], 1);
    assert_eq!(result["groups"][0]["tools"][0]["name"], "search");
}

#[tokio::test]
async fn catalog_source_filter_prefix_wildcard() {
    let reg = registry_with_mixed_sources().await;
    let req = JsonRpcRequest::with_id("tools.catalog", Some(json!({"source": "mcp:*"})), json!(1));
    let resp = handle_catalog(req, &reg).await;
    let result = resp.result.unwrap();
    assert_eq!(result["total"], 2);
}

#[tokio::test]
async fn catalog_source_filter_no_results() {
    let reg = registry_with_mixed_sources().await;
    let req = JsonRpcRequest::with_id(
        "tools.catalog",
        Some(json!({"source": "mcp:does-not-exist"})),
        json!(1),
    );
    let resp = handle_catalog(req, &reg).await;
    let result = resp.result.unwrap();
    assert_eq!(result["total"], 0);
}

#[tokio::test]
async fn effective_respects_user_added_agent_allowlist() {
    let reg = registry_with_mixed_sources().await;
    let agents = AgentRegistry::new();
    agents.register(
        AgentDef::new("scoped", AgentMode::SubAgent).with_allowed_tools(vec!["search".into()]),
    );

    let agent_def = agents.get("scoped");
    let req = JsonRpcRequest::with_id(
        "tools.effective",
        Some(json!({"agent_id": "scoped"})),
        json!(1),
    );
    let resp = handle_effective(req, &reg, agent_def.as_ref()).await;
    let result = resp.result.unwrap();
    assert_eq!(result["total"], 1);
    assert_eq!(result["agent_id"], "scoped");
    assert_eq!(result["groups"][0]["tools"][0]["name"], "search");
}

#[tokio::test]
async fn effective_source_filter_applies_after_allowlist() {
    let reg = registry_with_mixed_sources().await;
    let agents = AgentRegistry::new();
    // Allow two tools across two source families
    agents.register(
        AgentDef::new("mixed", AgentMode::SubAgent)
            .with_allowed_tools(vec!["search".into(), "github_status".into()]),
    );

    let agent_def = agents.get("mixed");
    // Filter to mcp:* → only github_status remains
    let req = JsonRpcRequest::with_id(
        "tools.effective",
        Some(json!({"agent_id": "mixed", "source": "mcp:*"})),
        json!(1),
    );
    let resp = handle_effective(req, &reg, agent_def.as_ref()).await;
    let result = resp.result.unwrap();
    assert_eq!(result["total"], 1);
    assert_eq!(result["groups"][0]["tools"][0]["name"], "github_status");
}

#[tokio::test]
async fn effective_with_no_agent_returns_all_tools() {
    let reg = registry_with_mixed_sources().await;
    let req = JsonRpcRequest::with_id("tools.effective", None, json!(1));
    let resp = handle_effective(req, &reg, None).await;
    let result = resp.result.unwrap();
    assert_eq!(result["total"], 5);
    assert!(result["agent_id"].is_null());
}
