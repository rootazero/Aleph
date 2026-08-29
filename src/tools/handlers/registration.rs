//! Registration helpers for MCP tool handlers.
//!
//! Encapsulates the "scan → build handler → register" logic so that the MCP
//! connection lifecycle can call a single entry point when a server connects,
//! and a matching cleanup path when it tears down. Extension/plugin variants
//! were removed 2026-05-20 — see `tools::handlers::mod` for rationale.

use crate::sync_primitives::Arc;

use crate::mcp::{McpClient, McpTool};
use crate::tool_metadata::ToolCatalog;
use crate::tool_metadata::{ToolSource as CatalogToolSource, UnifiedTool};
use crate::tools::handlers::mcp::McpHandler;
use crate::tools::handlers::ToolHandler;
use crate::tools::probes::mcp::McpServerProbe;
use crate::tools::registry::ToolHandlerRegistry;
use crate::tools::service::ToolSource;

use serde_json::Value;

/// Returns `Some(reason)` when a tool's advertised parameters schema is
/// structurally unusable for function-calling and should be quarantined.
///
/// Conservative on purpose — only flags schemas that EVERY provider would
/// reject, so a valid tool is never dropped:
/// - the schema is not a JSON object at all, or
/// - it declares a `type` that is neither `"object"` nor an array containing
///   `"object"`.
///
/// A missing `type` is tolerated (providers default object-shaped tool params),
/// and individual unsupported keywords (`$ref`, `format`, …) are intentionally
/// left untouched — keyword stripping is provider-specific and handled
/// elsewhere (e.g. the Gemini schema cleaner).
fn unusable_tool_schema_reason(schema: &Value) -> Option<&'static str> {
    let Value::Object(map) = schema else {
        return Some("parameters schema is not a JSON object");
    };
    match map.get("type") {
        None => None,
        Some(Value::String(t)) if t == "object" => None,
        Some(Value::String(_)) => Some("parameters schema `type` is not \"object\""),
        Some(Value::Array(types)) => {
            if types.iter().any(|v| v.as_str() == Some("object")) {
                None
            } else {
                Some("parameters schema `type` array does not include \"object\"")
            }
        }
        Some(_) => Some("parameters schema `type` is not a string or array"),
    }
}

/// Register every tool from one MCP server into the shared executor `ToolHandlerRegistry`,
/// and (when a tool catalog is supplied) attach a single
/// [`McpServerProbe`] per qualified name so the `<tool_runtime_state>` block
/// can surface a "server transport down" hint to the LLM.
///
/// Should be invoked *after* a successful `McpClient::start_external_server`
/// or `start_remote_server` for the matching `server_id`. Safe to call
/// repeatedly — collisions log a warning and are skipped. Returns the list of
/// qualified names that were newly registered so the caller can tear them down
/// in a matching `unregister_mcp_tools` on disconnect.
///
/// `timeout_seconds` is the server config's request timeout (`None` = the
/// client's own default). Each handler declares it as its wall-clock budget so
/// the harness cannot preempt a call the MCP client would have returned.
pub async fn register_mcp_tools(
    registry: &ToolHandlerRegistry,
    tool_catalog: Option<&Arc<ToolCatalog>>,
    client: Arc<McpClient>,
    server_id: &str,
    tools: &[McpTool],
    timeout_seconds: Option<u64>,
) -> Vec<String> {
    let mut registered = Vec::with_capacity(tools.len());
    for tool in tools {
        // Quarantine structurally-unusable parameter schemas. A function-call
        // tool's parameters MUST be an object schema; an MCP server that
        // advertises a non-object schema would otherwise make every provider
        // reject the whole LLM request (HTTP 400) for as long as the server is
        // connected, breaking unrelated tool calls. Skip + log instead of
        // poisoning the turn. (openclaw #86689 — quarantine unsupported tool
        // schemas.) We deliberately do NOT strip individual unsupported
        // keywords here; that is provider-specific and regression-prone.
        if let Some(reason) = unusable_tool_schema_reason(&tool.input_schema) {
            tracing::warn!(
                server_id = %server_id,
                tool = %tool.name,
                reason,
                "MCP tool quarantined: unusable parameters schema (skipping registration)"
            );
            continue;
        }
        let mcp_handler = McpHandler::new(
            Arc::clone(&client),
            server_id.to_string(),
            tool.name.clone(),
            tool.description.clone(),
            tool.input_schema.clone(),
        )
        .with_flags(tool.read_only, tool.idempotent, tool.requires_confirmation)
        .with_timeout_seconds(timeout_seconds);
        // Single source of naming truth: the handler computes the provider-
        // safe registry key (strips the manager's `{server}:` namespace
        // prefix, sanitizes to `[A-Za-z0-9_-]{1,64}`). Composing the key
        // here from the raw namespaced name would re-introduce the
        // `server__server:tool` double-prefix the LLM providers reject.
        let qualified = mcp_handler.qualified_name();
        let handler: Arc<dyn ToolHandler> = Arc::new(mcp_handler);
        match registry.register(qualified.clone(), handler) {
            Ok(()) => {
                if let Some(disp) = tool_catalog {
                    disp.register_health_probe(
                        qualified.clone(),
                        Arc::new(McpServerProbe::new(Arc::clone(&client), server_id)),
                    );
                    // R8 same-source catalog registration: makes the MCP tool
                    // visible to commands.list / tools.catalog and resolvable as
                    // a slash command. Uses the CATALOG-side ToolSource::Mcp
                    // { server }, id = mcp:{server}:{qualified}, command name =
                    // the provider-safe qualified name.
                    //
                    // Wire the handler-side `requires_confirmation` flag into the
                    // catalog so `infer_visible_channels` actually produces the
                    // gated visibility it advertises (rather than falling through
                    // to `Vec::new()` for every catalog entry). The handler side
                    // already gates the call; this keeps catalog and handler
                    // aligned so a future list-filter that respects catalog-side
                    // `requires_confirmation` does not disagree with the dispatch
                    // layer.
                    let mut builder = UnifiedTool::new(
                        format!("mcp:{server_id}:{qualified}"),
                        qualified.clone(),
                        tool.description.clone(),
                        CatalogToolSource::Mcp {
                            server: server_id.to_string(),
                        },
                    );
                    if tool.requires_confirmation {
                        builder = builder.with_requires_confirmation(true);
                    }
                    if !tool.read_only && tool.requires_confirmation {
                        // Mutating + confirm-gated tools bump the safety level so
                        // panel/cli-only gating kicks in for destructiveHint=true
                        // servers.
                        builder = builder.with_safety_level(
                            crate::tool_metadata::ToolSafetyLevel::IrreversibleHighRisk,
                        );
                    } else if !tool.read_only {
                        // Mutating without explicit confirm: register at the
                        // lowest mutating level so the catalog reflects the
                        // fact that this tool changes state, but does not
                        // trigger the panel/cli-only gate on its own.
                        builder = builder.with_safety_level(
                            crate::tool_metadata::ToolSafetyLevel::Reversible,
                        );
                    }
                    disp.register_with_conflict_resolution(builder).await;
                }
                registered.push(qualified);
            }
            Err(e) => tracing::warn!(
                error = ?e,
                qualified = %qualified,
                "MCP tool register failed"
            ),
        }
    }
    registered
}

/// Unregister every tool previously registered from the given MCP server.
///
/// Walks the registry snapshot and removes every handler whose
/// `ToolSource` matches `Mcp { server_id }`. When `tool_catalog` is
/// supplied, also tears down the matching health probes. Returns the set of
/// qualified names that were removed.
#[must_use]
pub async fn unregister_mcp_tools(
    registry: &ToolHandlerRegistry,
    tool_catalog: Option<&Arc<ToolCatalog>>,
    server_id: &str,
) -> Vec<String> {
    let snapshot = registry.snapshot();
    let victims: Vec<String> = snapshot
        .iter()
        .filter_map(|(name, handler)| match handler.definition().source {
            ToolSource::Mcp { server_id: ref sid } if sid == server_id => Some(name.clone()),
            _ => None,
        })
        .collect();
    drop(snapshot);
    for name in &victims {
        let _ = registry.unregister(name);
        if let Some(disp) = tool_catalog {
            disp.health().unregister_probe(name);
        }
    }
    if let Some(disp) = tool_catalog {
        let _ = disp.remove_by_mcp_server(server_id).await;
    }
    victims
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str, desc: &str) -> McpTool {
        McpTool {
            name: name.into(),
            description: desc.into(),
            input_schema: json!({"type": "object"}),
            requires_confirmation: false,
            read_only: false,
            idempotent: false,
        }
    }

    #[tokio::test]
    async fn register_mcp_tools_strips_namespaced_prefix_from_registry_key() {
        // The manager hands the bridge namespaced names ("server:tool");
        // the registry key must be the provider-safe `server__tool`, not
        // the colon-bearing double prefix `server__server:tool`.
        let reg = ToolHandlerRegistry::new();
        let client = Arc::new(McpClient::new());
        let names = register_mcp_tools(
            &reg,
            None,
            client,
            "github",
            &[tool("github:create_issue", "d")],
            None,
        )
        .await;
        assert_eq!(names, vec!["github__create_issue"]);
        assert!(reg.snapshot().contains_key("github__create_issue"));
    }

    #[tokio::test]
    async fn register_mcp_tools_carries_annotation_flags_into_definition() {
        let reg = ToolHandlerRegistry::new();
        let client = Arc::new(McpClient::new());
        let mut ro = tool("list_items", "d");
        ro.read_only = true;
        ro.idempotent = true;
        let mut boom = tool("delete_item", "d");
        boom.requires_confirmation = true;
        register_mcp_tools(&reg, None, client, "srv", &[ro, boom], None).await;
        let snap = reg.snapshot();
        let ro_def = snap.get("srv__list_items").unwrap().definition();
        assert!(ro_def.metadata.concurrent_safe);
        assert!(ro_def.metadata.idempotent);
        assert!(!ro_def.metadata.requires_approval);
        let boom_def = snap.get("srv__delete_item").unwrap().definition();
        assert!(boom_def.metadata.requires_approval);
        assert!(!boom_def.metadata.concurrent_safe);
    }

    #[tokio::test]
    async fn register_mcp_tools_declares_a_budget_above_the_server_timeout() {
        // Regression: MCP tools carried NO wall-clock budget, so the harness
        // read them as unbudgeted and aborted the whole run at its own
        // fallback — long before the MCP client's own timeout could return a
        // recoverable error. The declared budget must outlive that timeout.
        let reg = ToolHandlerRegistry::new();
        let client = Arc::new(McpClient::new());
        register_mcp_tools(&reg, None, client, "srv", &[tool("slow", "d")], Some(600)).await;
        let def = reg.snapshot().get("srv__slow").unwrap().definition();
        let budget = def.metadata.max_duration_ms.expect("MCP tool is budgeted");
        assert!(
            budget > 600_000,
            "budget {budget}ms must outlive the server's 600s request timeout"
        );
    }

    #[tokio::test]
    async fn register_mcp_tools_budgets_a_server_with_no_configured_timeout() {
        // No `timeout_seconds` in config → the client's own remote default
        // (300s) applies; the definition must still carry a budget above it.
        let reg = ToolHandlerRegistry::new();
        let client = Arc::new(McpClient::new());
        register_mcp_tools(&reg, None, client, "srv", &[tool("slow", "d")], None).await;
        let def = reg.snapshot().get("srv__slow").unwrap().definition();
        let budget = def.metadata.max_duration_ms.expect("MCP tool is budgeted");
        assert!(budget > 300_000, "budget {budget}ms must clear the default");
    }

    #[test]
    fn unusable_schema_reason_flags_only_clearly_broken() {
        // Tolerated:
        assert!(unusable_tool_schema_reason(&json!({"type": "object"})).is_none());
        assert!(unusable_tool_schema_reason(&json!({})).is_none()); // type omitted
        assert!(unusable_tool_schema_reason(&json!({"type": ["object", "null"]})).is_none());
        // $ref / format are NOT structural — left for provider-specific cleaning:
        assert!(unusable_tool_schema_reason(&json!({"$ref": "#/defs/X"})).is_none());
        // Quarantined:
        assert!(unusable_tool_schema_reason(&json!("not a schema")).is_some());
        assert!(unusable_tool_schema_reason(&json!(42)).is_some());
        assert!(unusable_tool_schema_reason(&json!({"type": "string"})).is_some());
        assert!(unusable_tool_schema_reason(&json!({"type": ["string", "number"]})).is_some());
        assert!(unusable_tool_schema_reason(&json!({"type": 7})).is_some());
    }

    #[tokio::test]
    async fn register_mcp_tools_quarantines_unusable_schema() {
        let reg = ToolHandlerRegistry::new();
        let client = Arc::new(McpClient::new());
        let mut bad = tool("broken", "d");
        bad.input_schema = json!({"type": "string"});
        let mut bad2 = tool("scalar", "d");
        bad2.input_schema = json!("nope");
        let good = tool("ok", "d");
        let names = register_mcp_tools(&reg, None, client, "srv", &[bad, bad2, good], None).await;
        // Only the valid tool is registered; the two broken ones are skipped.
        assert_eq!(names, vec!["srv__ok"]);
        let snap = reg.snapshot();
        assert!(snap.contains_key("srv__ok"));
        assert!(!snap.contains_key("srv__broken"));
        assert!(!snap.contains_key("srv__scalar"));
    }

    #[tokio::test]
    async fn register_mcp_tools_applies_double_underscore_naming() {
        let reg = ToolHandlerRegistry::new();
        let client = Arc::new(McpClient::new());
        let tools = [tool("get_time", "a"), tool("set_tz", "b")];
        let names = register_mcp_tools(&reg, None, client, "clock", &tools, None).await;
        assert_eq!(names, vec!["clock__get_time", "clock__set_tz"]);
        let snap = reg.snapshot();
        assert!(snap.contains_key("clock__get_time"));
        assert!(snap.contains_key("clock__set_tz"));
    }

    #[tokio::test]
    async fn unregister_mcp_tools_removes_only_matching_server() {
        let reg = ToolHandlerRegistry::new();
        let client = Arc::new(McpClient::new());
        register_mcp_tools(
            &reg,
            None,
            Arc::clone(&client),
            "alpha",
            &[tool("x", "d")],
            None,
        )
        .await;
        register_mcp_tools(
            &reg,
            None,
            Arc::clone(&client),
            "beta",
            &[tool("y", "d")],
            None,
        )
        .await;
        assert_eq!(reg.snapshot().len(), 2);
        let removed = unregister_mcp_tools(&reg, None, "alpha").await;
        assert_eq!(removed, vec!["alpha__x"]);
        let remaining: Vec<String> = reg.snapshot().keys().cloned().collect();
        assert_eq!(remaining, vec!["beta__y"]);
    }

    #[tokio::test]
    async fn register_mcp_tools_duplicate_is_skipped_with_warning() {
        let reg = ToolHandlerRegistry::new();
        let client = Arc::new(McpClient::new());
        let t = [tool("dup", "d")];
        let first = register_mcp_tools(&reg, None, Arc::clone(&client), "s", &t, None).await;
        let second = register_mcp_tools(&reg, None, client, "s", &t, None).await;
        assert_eq!(first, vec!["s__dup"]);
        assert!(second.is_empty());
        assert_eq!(reg.snapshot().len(), 1);
    }

    #[tokio::test]
    async fn register_with_tool_catalog_attaches_probe() {
        let reg = ToolHandlerRegistry::new();
        let disp = Arc::new(ToolCatalog::new());
        let client = Arc::new(McpClient::new());
        register_mcp_tools(
            &reg,
            Some(&disp),
            client,
            "srv",
            &[tool("a", "d"), tool("b", "d")],
            None,
        )
        .await;
        // No public introspection of registered probes; instead, force a
        // refresh and observe that an entry materialises (since fresh
        // McpClient reports no live servers, the probe returns Unhealthy).
        let cache = disp.health();
        let _ = cache.refresh("srv__a").await;
        let snap = cache.snapshot();
        // `is_healthy` returns true for empty entries; an entry now exists
        // and reports unhealthy, so the snapshot's `reason` is Some.
        assert!(snap.reason("srv__a").is_some());
    }

    #[tokio::test]
    async fn unregister_with_tool_catalog_drops_probe() {
        let reg = ToolHandlerRegistry::new();
        let disp = Arc::new(ToolCatalog::new());
        let client = Arc::new(McpClient::new());
        register_mcp_tools(&reg, Some(&disp), client, "srv", &[tool("a", "d")], None).await;
        let removed = unregister_mcp_tools(&reg, Some(&disp), "srv").await;
        assert_eq!(removed, vec!["srv__a"]);
        // Re-registering immediately should not collide with a leftover probe
        // (no public assertion on probe count exists, but unregister_probe
        // returns true only when a probe was actually removed; a second
        // unregister returns false).
        assert!(!disp.health().unregister_probe("srv__a"));
    }

    #[tokio::test]
    async fn register_mcp_tools_populates_tool_catalog() {
        use crate::tool_metadata::ToolCatalog;
        let reg = ToolHandlerRegistry::new();
        let catalog = Arc::new(ToolCatalog::new());
        let client = Arc::new(McpClient::new());
        let t = tool("do_thing", "does a thing");
        let names = register_mcp_tools(&reg, Some(&catalog), client, "srv", &[t], None).await;
        assert_eq!(names.len(), 1);
        let in_catalog = catalog.list_by_mcp_server("srv").await;
        assert_eq!(in_catalog.len(), 1, "MCP tool must appear in ToolCatalog");
        assert_eq!(in_catalog[0].name, names[0]);
    }

    #[tokio::test]
    async fn unregister_mcp_tools_clears_tool_catalog() {
        use crate::tool_metadata::ToolCatalog;
        let reg = ToolHandlerRegistry::new();
        let catalog = Arc::new(ToolCatalog::new());
        let client = Arc::new(McpClient::new());
        register_mcp_tools(
            &reg,
            Some(&catalog),
            Arc::clone(&client),
            "srv",
            &[tool("t", "d")],
            None,
        )
        .await;
        let removed = unregister_mcp_tools(&reg, Some(&catalog), "srv").await;
        assert_eq!(removed.len(), 1);
        assert!(catalog.list_by_mcp_server("srv").await.is_empty());
    }
}
