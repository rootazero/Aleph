# Tools Visibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `tools.catalog` and `tools.effective` RPC handlers that return tools grouped by source, with agent-level allow/deny filtering for effective tools.

**Architecture:** Single new handler file (`tools_visibility.rs`) with shared grouping logic. Uses existing `ToolRegistry.list_all()` for tool enumeration and `AgentDef.is_tool_allowed()` for filtering. Wired at startup via closure capture (same pattern as `commands.list`).

**Tech Stack:** Rust, JSON-RPC handlers, `ToolRegistry`, `AgentDef`, `ToolSource` enum

**Spec:** `docs/superpowers/specs/2026-03-27-tools-visibility-design.md`

---

### Task 1: Create tools_visibility.rs with grouping logic + catalog handler

**Files:**
- Create: `src/gateway/handlers/tools_visibility.rs`
- Modify: `src/gateway/handlers/mod.rs` — add `pub mod tools_visibility;`

- [ ] **Step 1: Create tools_visibility.rs with types and grouping logic**

```rust
//! Tools Visibility RPC Handlers
//!
//! `tools.catalog` — all registered tools grouped by source (no filtering)
//! `tools.effective` — filtered by agent's allow/deny lists

use std::collections::BTreeMap;
use serde::Serialize;
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::agents::AgentDef;
use crate::dispatcher::types::conflict::ToolSource;
use crate::dispatcher::{ToolRegistry, UnifiedTool};
use crate::sync_primitives::Arc;

// === Response types ===

#[derive(Debug, Serialize)]
pub struct ToolsListResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub total: usize,
    pub groups: Vec<ToolGroup>,
}

#[derive(Debug, Serialize)]
pub struct ToolGroup {
    pub id: String,
    pub label: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    pub tools: Vec<ToolEntry>,
}

#[derive(Debug, Serialize)]
pub struct ToolEntry {
    pub name: String,
    pub description: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
}

// === Grouping logic ===

/// Extract source type string and source identifier from ToolSource enum.
fn extract_source(source: &ToolSource) -> (&'static str, Option<String>) {
    match source {
        ToolSource::Native => ("native", None),
        ToolSource::Builtin => ("builtin", None),
        ToolSource::Mcp { server } => ("mcp", Some(server.clone())),
        ToolSource::Skill { id } => ("skill", Some(id.clone())),
        ToolSource::Plugin { plugin_id } => ("plugin", Some(plugin_id.clone())),
        ToolSource::Custom { .. } => ("custom", None),
    }
}

/// Build a group ID from source type and optional source_id.
fn group_id(source_str: &str, source_id: &Option<String>) -> String {
    match source_id {
        Some(id) => format!("{source_str}:{id}"),
        None => source_str.to_string(),
    }
}

/// Build a group label from source type and optional source_id.
fn group_label(source_str: &str, source_id: &Option<String>) -> String {
    match (source_str, source_id) {
        ("native", _) => "Native".to_string(),
        ("builtin", _) => "Built-in".to_string(),
        ("custom", _) => "Custom".to_string(),
        (_, Some(id)) => id.clone(),
        (s, None) => s.to_string(),
    }
}

/// Group a list of tools by their source.
fn group_tools(tools: Vec<UnifiedTool>) -> Vec<ToolGroup> {
    let mut groups: BTreeMap<String, ToolGroup> = BTreeMap::new();

    for tool in tools {
        let (source_str, source_id) = extract_source(&tool.source);
        let gid = group_id(source_str, &source_id);

        let group = groups.entry(gid.clone()).or_insert_with(|| ToolGroup {
            id: gid,
            label: group_label(source_str, &source_id),
            source: source_str.to_string(),
            source_id: source_id.clone(),
            tools: Vec::new(),
        });

        group.tools.push(ToolEntry {
            name: tool.name,
            description: tool.description,
            source: source_str.to_string(),
            source_id,
        });
    }

    groups.into_values().collect()
}

// === Handlers ===

/// `tools.catalog` — list all registered tools grouped by source.
///
/// Called from startup wiring with captured ToolRegistry.
pub async fn handle_catalog(
    request: JsonRpcRequest,
    tool_registry: &ToolRegistry,
) -> JsonRpcResponse {
    let tools = tool_registry.list_all().await;
    let total = tools.len();
    let groups = group_tools(tools);

    let result = ToolsListResult {
        agent_id: None,
        total,
        groups,
    };

    JsonRpcResponse::success(request.id, serde_json::to_value(result).unwrap_or(json!({})))
}

/// `tools.effective` — list tools available to a specific agent.
///
/// Called from startup wiring with captured ToolRegistry + agent lookup closure.
pub async fn handle_effective(
    request: JsonRpcRequest,
    tool_registry: &ToolRegistry,
    agent: Option<&AgentDef>,
) -> JsonRpcResponse {
    let tools = tool_registry.list_all().await;

    // Filter by agent's allow/deny lists
    let (filtered, agent_id) = match agent {
        Some(agent_def) => {
            let filtered: Vec<UnifiedTool> = tools
                .into_iter()
                .filter(|t| agent_def.is_tool_allowed(&t.name))
                .collect();
            (filtered, Some(agent_def.id.clone()))
        }
        None => (tools, None),
    };

    let total = filtered.len();
    let groups = group_tools(filtered);

    let result = ToolsListResult {
        agent_id,
        total,
        groups,
    };

    JsonRpcResponse::success(request.id, serde_json::to_value(result).unwrap_or(json!({})))
}

// === Stub handlers for initial registration ===

/// Stub for `tools.catalog` before wiring
pub async fn handle_catalog_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        request.id,
        -32603,
        "tools.catalog not yet wired — tool registry unavailable",
    )
}

/// Stub for `tools.effective` before wiring
pub async fn handle_effective_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        request.id,
        -32603,
        "tools.effective not yet wired — tool registry unavailable",
    )
}
```

IMPORTANT: Verify exact import paths:
- `crate::agents::AgentDef` — may be `crate::agents::types::AgentDef`, check `src/agents/mod.rs` for re-exports
- `crate::dispatcher::types::conflict::ToolSource` — confirmed at `src/dispatcher/types/conflict.rs:98`
- `crate::dispatcher::{ToolRegistry, UnifiedTool}` — confirmed re-exports from `src/dispatcher/mod.rs`
- `super::super::protocol::{JsonRpcRequest, JsonRpcResponse}` — check actual path from handlers directory

- [ ] **Step 2: Add module declaration**

In `src/gateway/handlers/mod.rs`, add:
```rust
pub mod tools_visibility;
```

And register stubs in `HandlerRegistry::new()`:
```rust
registry.register("tools.catalog", tools_visibility::handle_catalog_stub);
registry.register("tools.effective", tools_visibility::handle_effective_stub);
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`

- [ ] **Step 4: Add tests for grouping logic**

Add `#[cfg(test)] mod tests` in `tools_visibility.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::types::conflict::ToolSource;

    fn make_tool(name: &str, source: ToolSource) -> UnifiedTool {
        UnifiedTool::new(
            format!("{}:{}", source.label().to_lowercase(), name),
            name.to_string(),
            format!("{} description", name),
            source,
        )
    }

    #[test]
    fn test_group_tools_by_source() {
        let tools = vec![
            make_tool("search", ToolSource::Native),
            make_tool("bash", ToolSource::Builtin),
            make_tool("git_status", ToolSource::Mcp { server: "github".into() }),
            make_tool("git_diff", ToolSource::Mcp { server: "github".into() }),
            make_tool("browse", ToolSource::Mcp { server: "browser".into() }),
            make_tool("refine", ToolSource::Skill { id: "refine-text".into() }),
            make_tool("diag", ToolSource::Plugin { plugin_id: "diagnostics".into() }),
        ];

        let groups = group_tools(tools);

        // Should have 5 groups: native, builtin, mcp:github, mcp:browser, skill:refine-text, plugin:diagnostics
        assert_eq!(groups.len(), 6);

        // Check MCP github group has 2 tools
        let github = groups.iter().find(|g| g.id == "mcp:github").unwrap();
        assert_eq!(github.tools.len(), 2);
        assert_eq!(github.source, "mcp");
        assert_eq!(github.source_id, Some("github".into()));
    }

    #[test]
    fn test_group_tools_empty() {
        let groups = group_tools(vec![]);
        assert!(groups.is_empty());
    }

    #[test]
    fn test_extract_source_variants() {
        assert_eq!(extract_source(&ToolSource::Native), ("native", None));
        assert_eq!(extract_source(&ToolSource::Builtin), ("builtin", None));
        assert_eq!(
            extract_source(&ToolSource::Mcp { server: "gh".into() }),
            ("mcp", Some("gh".into()))
        );
        assert_eq!(
            extract_source(&ToolSource::Plugin { plugin_id: "p1".into() }),
            ("plugin", Some("p1".into()))
        );
    }
}
```

Check `UnifiedTool::new()` constructor signature — it may need different args. Grep for `pub fn new` in unified/mod.rs and adapt.

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib tools_visibility`

- [ ] **Step 6: Commit**

```
gateway: add tools.catalog and tools.effective handlers with grouping
```

---

### Task 2: Wire handlers at startup

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init.rs`

- [ ] **Step 1: Wire tools.catalog and tools.effective**

Find the block where `commands.list` is wired (around line 1027-1032). Add similar wiring for the two new handlers immediately after:

```rust
// Wire tools.catalog
{
    let reg = dispatch_registry.clone();
    server.handlers_mut().register("tools.catalog", move |req| {
        let registry = reg.clone();
        async move {
            alephcore::gateway::handlers::tools_visibility::handle_catalog(req, &registry).await
        }
    });
}

// Wire tools.effective
{
    let reg = dispatch_registry.clone();
    let agent_reg = agent_registry.clone();  // agents::registry::AgentRegistry
    server.handlers_mut().register("tools.effective", move |req| {
        let registry = reg.clone();
        let agents = agent_reg.clone();
        async move {
            // Parse agent_id from params
            let agent_id = req.params.as_ref()
                .and_then(|p| p.get("agent_id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            // Look up agent def
            let agent_def = match &agent_id {
                Some(id) => agents.get(id),
                None => agents.get("main"),  // default agent
            };

            alephcore::gateway::handlers::tools_visibility::handle_effective(
                req,
                &registry,
                agent_def.as_ref(),
            ).await
        }
    });
}
```

IMPORTANT: Check which `agent_registry` variable is in scope at this point. The wiring block may use `agent_registry` (the `agents::registry::AgentRegistry` with `AgentDef`), not the gateway's `AgentRegistry` (with `AgentInstance`). Look at the surrounding code to determine the correct variable.

- [ ] **Step 2: Verify compilation**

Run: `cargo check` (full workspace — this modifies the binary crate)

- [ ] **Step 3: Commit**

```
gateway: wire tools.catalog and tools.effective at startup
```

---

### Task 3: Final verification

**Files:** None (verification only)

- [ ] **Step 1: Run all openai_api tests**

Run: `cargo test -p alephcore --lib openai_api`
Expected: All existing tests pass (Phase 1 + Phase 2A).

- [ ] **Step 2: Run tools_visibility tests**

Run: `cargo test -p alephcore --lib tools_visibility`
Expected: All new tests pass.

- [ ] **Step 3: Full workspace check**

Run: `cargo check`
Expected: Clean compilation.

- [ ] **Step 4: Commit (if any fixes needed)**

```
gateway: fix tools visibility integration issues
```

---

## Dependency Graph

```
Task 1 (handler + grouping + tests)
  ↓
Task 2 (startup wiring)
  ↓
Task 3 (verification)
```

All tasks are sequential — each depends on the previous.
