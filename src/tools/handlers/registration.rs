//! Registration helpers for MCP tool handlers.
//!
//! Encapsulates the "scan → build handler → register" logic so that the MCP
//! connection lifecycle can call a single entry point when a server connects,
//! and a matching cleanup path when it tears down. Extension/plugin variants
//! were removed 2026-05-20 — see `tools::handlers::mod` for rationale.

use crate::extension::effects::async_disposer;
use crate::sync_primitives::Arc;

use crate::mcp::{McpClient, McpTool};
use crate::tool_metadata::ToolCatalog;
use crate::tools::descriptor::ToolCapabilityDescriptor;
use crate::tools::handlers::mcp::McpHandler;
use crate::tools::handlers::ToolHandler;
use crate::tools::probes::mcp::McpServerProbe;
use crate::tools::registration_scope::ToolRegistrationScope;
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
/// qualified names that were newly registered.
///
/// Every registration this call makes — the registry entry via its
/// generation-guarded [`RegistrationHandle`](crate::tools::registry::RegistrationHandle),
/// and (when a catalog is supplied) the health probe plus the catalog
/// projection — is tracked in `scope`. Tearing the server down is then
/// `scope.dispose()`, which is generation-guarded and therefore cannot remove
/// a replacement registration that took over the same name. The caller owns
/// the scope; this function never disposes it.
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
    scope: &mut ToolRegistrationScope,
) -> Vec<String> {
    let mut registered = Vec::with_capacity(tools.len());
    // Catalog identities and registry generations created by this call. The
    // disposer compares generations before removing each projection.
    let mut catalog_entries: Vec<(String, u64, String)> = Vec::new();
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
        // The descriptor is projected from the handler's own `ToolDefinition`,
        // so the registry's `matches_definition` pairing check passes by
        // construction. Revision 0 is normalized to the registry-assigned
        // value inside `register`.
        let descriptor = ToolCapabilityDescriptor::from_definition(&handler.definition(), 0);
        let catalog_builder = tool_catalog
            .is_some()
            .then(|| descriptor.to_unified_tool(format!("mcp:{server_id}:{qualified}")));
        match registry.register(descriptor, handler) {
            Ok(handle) => {
                // `handle` is the generation-guarded disposer for this entry.
                // Tracking it in the scope makes teardown generation-guarded:
                // once another registration replaces this name the handle is a
                // no-op, so a stale scope can never remove a replacement.
                debug_assert_eq!(handle.name(), qualified.as_str());
                let revision = handle.revision();
                scope.track(handle);
                if let Some(disp) = tool_catalog {
                    disp.register_health_probe(
                        qualified.clone(),
                        Arc::new(McpServerProbe::new(Arc::clone(&client), server_id)),
                    );
                    let catalog_id = disp
                        .register_with_conflict_resolution(
                            catalog_builder.expect("catalog projection was prepared"),
                        )
                        .await;
                    catalog_entries.push((qualified.clone(), revision, catalog_id));
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
    // When a catalog projection was made, own its teardown from the same scope
    // so disposing the scope cleans the catalog and probes too — not just the
    // registry entries. Tracked last, so it runs FIRST on dispose (reverse
    // order), removing the projections before the registry entries they point
    // at. Only added when something was actually registered: an empty call must
    // not `remove_by_mcp_server` another scope's entries.
    if let Some(disp) = tool_catalog {
        if !registered.is_empty() {
            let catalog = Arc::clone(disp);
            let registry = registry.clone();
            let catalog_entries = catalog_entries.clone();
            scope.track_disposer(
                format!("catalog:{server_id}"),
                async_disposer(move || {
                    let catalog = Arc::clone(&catalog);
                    let registry = registry.clone();
                    let catalog_entries = catalog_entries.clone();
                    async move {
                        for (name, revision, catalog_id) in catalog_entries {
                            let still_current = registry
                                .descriptor(&name)
                                .is_some_and(|descriptor| descriptor.revision == revision);
                            if !still_current {
                                continue;
                            }
                            let _ = catalog.health().unregister_probe(&name);
                            let _ = catalog.remove_by_id(&catalog_id).await;
                        }
                        Ok(())
                    }
                }),
            );
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
///
/// **Compatibility / emergency path only.** It removes by an externally
/// derived name set, so it can delete a *replacement* registration that took
/// over the same name. Normal lifecycle teardown is driven by the per-server
/// [`ToolRegistrationScope`] the bridge holds (generation-guarded, replacement
/// safe). This entry survives for callers that must sweep residue no scope
/// owns — e.g. the bridge's one-time startup sweep of entries left behind by a
/// previous incarnation — and for tests.
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
    use crate::tool_metadata::{ToolSource as CatalogToolSource, UnifiedTool};
    use serde_json::json;

    /// A throwaway scope for one call. These tests assert registration
    /// effects, not disposal, so it is dropped without being disposed.
    fn scope() -> ToolRegistrationScope {
        ToolRegistrationScope::new("test")
    }

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
            &mut scope(),
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
        register_mcp_tools(&reg, None, client, "srv", &[ro, boom], None, &mut scope()).await;
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
        register_mcp_tools(
            &reg,
            None,
            client,
            "srv",
            &[tool("slow", "d")],
            Some(600),
            &mut scope(),
        )
        .await;
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
        register_mcp_tools(
            &reg,
            None,
            client,
            "srv",
            &[tool("slow", "d")],
            None,
            &mut scope(),
        )
        .await;
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
        let names = register_mcp_tools(
            &reg,
            None,
            client,
            "srv",
            &[bad, bad2, good],
            None,
            &mut scope(),
        )
        .await;
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
        let names =
            register_mcp_tools(&reg, None, client, "clock", &tools, None, &mut scope()).await;
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
            &mut scope(),
        )
        .await;
        register_mcp_tools(
            &reg,
            None,
            Arc::clone(&client),
            "beta",
            &[tool("y", "d")],
            None,
            &mut scope(),
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
        let first =
            register_mcp_tools(&reg, None, Arc::clone(&client), "s", &t, None, &mut scope()).await;
        let second = register_mcp_tools(&reg, None, client, "s", &t, None, &mut scope()).await;
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
            &mut scope(),
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
        register_mcp_tools(
            &reg,
            Some(&disp),
            client,
            "srv",
            &[tool("a", "d")],
            None,
            &mut scope(),
        )
        .await;
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
        let mut t = tool("do_thing", "does a thing");
        t.requires_confirmation = true;
        let names = register_mcp_tools(
            &reg,
            Some(&catalog),
            client,
            "srv",
            &[t],
            None,
            &mut scope(),
        )
        .await;
        assert_eq!(names.len(), 1);
        let in_catalog = catalog.list_by_mcp_server("srv").await;
        assert_eq!(in_catalog.len(), 1, "MCP tool must appear in ToolCatalog");
        assert_eq!(in_catalog[0].name, names[0]);
        assert!(in_catalog[0].requires_confirmation);
        assert_eq!(
            in_catalog[0].parameters_schema,
            Some(json!({"type": "object"}))
        );
        assert!(matches!(
            in_catalog[0].source,
            CatalogToolSource::Mcp { ref server } if server == "srv"
        ));
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
            &mut scope(),
        )
        .await;
        let removed = unregister_mcp_tools(&reg, Some(&catalog), "srv").await;
        assert_eq!(removed.len(), 1);
        assert!(catalog.list_by_mcp_server("srv").await.is_empty());
    }

    #[tokio::test]
    async fn scope_dispose_removes_registry_entry_and_catalog_projection() {
        use crate::tool_metadata::ToolCatalog;
        let reg = ToolHandlerRegistry::new();
        let catalog = Arc::new(ToolCatalog::new());
        let client = Arc::new(McpClient::new());
        let mut owned = ToolRegistrationScope::new("mcp:srv");
        register_mcp_tools(
            &reg,
            Some(&catalog),
            client,
            "srv",
            &[tool("t", "d")],
            None,
            &mut owned,
        )
        .await;
        assert!(reg.resolve("srv__t").is_some());

        let report = owned.dispose().await;
        assert!(report.all_ok(), "clean disposal: {report:?}");
        assert_eq!(report.owner, "mcp:srv");
        assert!(reg.resolve("srv__t").is_none(), "registry entry removed");
        assert!(
            catalog.list_by_mcp_server("srv").await.is_empty(),
            "catalog projection removed with the scope"
        );
    }

    /// The bridge resyncs a server by disposing the old scope and registering
    /// the current tool list into a fresh one. A tool dropped from the server's
    /// advertised list must disappear, and nothing from the old scope may
    /// linger.
    #[tokio::test]
    async fn resync_disposes_old_scope_and_drops_removed_tool() {
        let reg = ToolHandlerRegistry::new();
        let client = Arc::new(McpClient::new());

        let mut first = ToolRegistrationScope::new("mcp:srv@1");
        register_mcp_tools(
            &reg,
            None,
            Arc::clone(&client),
            "srv",
            &[tool("a", "d"), tool("b", "d")],
            None,
            &mut first,
        )
        .await;
        assert!(reg.resolve("srv__a").is_some());
        assert!(reg.resolve("srv__b").is_some());
        assert!(first.dispose().await.all_ok());

        let mut second = ToolRegistrationScope::new("mcp:srv@2");
        register_mcp_tools(
            &reg,
            None,
            client,
            "srv",
            &[tool("a", "d")],
            None,
            &mut second,
        )
        .await;
        assert!(reg.resolve("srv__a").is_some(), "kept tool stays");
        assert!(
            reg.resolve("srv__b").is_none(),
            "tool removed from the server list must not leak"
        );
    }

    /// A stale scope (one whose server already resynced) must not remove a
    /// replacement that a later registration put under the same name. The
    /// generation-guarded handle makes the stale dispose a no-op.
    #[tokio::test]
    async fn stale_scope_dispose_does_not_remove_replacement() {
        let reg = ToolHandlerRegistry::new();
        let catalog = Arc::new(ToolCatalog::new());
        let client = Arc::new(McpClient::new());
        let mut stale = ToolRegistrationScope::new("mcp:srv@old");
        register_mcp_tools(
            &reg,
            Some(&catalog),
            Arc::clone(&client),
            "srv",
            &[tool("t", "d")],
            None,
            &mut stale,
        )
        .await;

        // A newer registration takes over the same qualified name.
        let replacement = McpHandler::new(
            Arc::clone(&client),
            "srv".to_string(),
            "t".to_string(),
            "replacement".to_string(),
            json!({"type": "object"}),
        );
        let replacement: Arc<dyn ToolHandler> = Arc::new(replacement);
        let descriptor = ToolCapabilityDescriptor::from_definition(&replacement.definition(), 0);
        reg.replace(descriptor, Arc::clone(&replacement))
            .expect("replace succeeds");
        catalog
            .register_with_conflict_resolution(UnifiedTool::new(
                "mcp:srv:replacement".to_string(),
                "replacement".to_string(),
                "replacement projection".to_string(),
                CatalogToolSource::Mcp {
                    server: "srv".to_string(),
                },
            ))
            .await;

        // Disposing the superseded scope must be a no-op for the replacement.
        assert!(stale.dispose().await.all_ok());
        let live = reg.resolve("srv__t");
        assert!(live.is_some(), "replacement must survive a stale dispose");
        assert!(
            catalog
                .list_by_mcp_server("srv")
                .await
                .iter()
                .any(|entry| entry.name == "replacement"),
            "stale scope must not remove the replacement's catalog projection"
        );
        let live = live.expect("replacement remains registered");
        assert_eq!(
            live.definition().name,
            replacement.definition().name,
            "resolve still returns the replacement capability"
        );
    }

    /// A failing disposer must be reported but must not abort the rest of the
    /// teardown. Scope disposal runs in reverse track order, so the failure is
    /// tracked last to run first.
    #[tokio::test]
    async fn failing_disposer_does_not_skip_remaining_cleanup() {
        use crate::extension::effects::sync_disposer;
        use std::sync::atomic::{AtomicBool, Ordering};

        let ran = Arc::new(AtomicBool::new(false));
        let mut owned = ToolRegistrationScope::new("mcp:srv");
        let flag = Arc::clone(&ran);
        owned.track_disposer(
            "ok",
            sync_disposer(move || {
                flag.store(true, Ordering::SeqCst);
                Ok(())
            }),
        );
        owned.track_disposer("boom", sync_disposer(|| Err("kaboom".to_string())));

        let report = owned.dispose().await;
        assert!(!report.all_ok(), "the failure must be surfaced");
        assert_eq!(report.failures().count(), 1);
        assert!(
            ran.load(Ordering::SeqCst),
            "a failing disposer must not skip later cleanup"
        );
    }
}
