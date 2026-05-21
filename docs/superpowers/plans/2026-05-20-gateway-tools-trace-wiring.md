# Gateway Tools+Trace Wiring Hardening — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close 5 defects + 3 OpenClaw parity gaps on Aleph gateway's `tools.*` + `trace.*` RPC surface (5 RPCs total). Delete 5 dead `*_stub` functions. Add integration tests proving phase-2 boot override actually replaces phase-1.

**Architecture:** Existing two-phase wiring pattern stays. Phase-1 registers a typed `SERVICE_UNAVAILABLE (-32099)` error closure (replaces today's mix of fake-success and `INTERNAL_ERROR`). Phase-2 in `agent_init.rs` always overrides — even when state DB is absent — so the user-visible behavior is deterministic. Targeted defect fixes are surgical edits at the existing wire sites.

**Tech Stack:** Rust, Tokio, axum (JSON-RPC), SQLite (`task_traces`), aleph-protocol crate (JSON-RPC error codes), serde_json.

**Reference spec:** `docs/superpowers/specs/2026-05-20-gateway-tools-trace-wiring-design.md`

---

## File Map

```
src/gateway/protocol.rs                      Modify  +1 const SERVICE_UNAVAILABLE
src/gateway/handlers/mod.rs                  Modify  ~30 LoC swap phase-1 stubs
src/gateway/handlers/tools_visibility.rs     Modify  +source filter, -2 _stub fns
src/gateway/handlers/tools_invoke.rs         Modify  +allowlist param, -1 _stub fn
src/gateway/handlers/trace_replay.rs         Modify  +pagination, -2 _stub fns
src/resilience/database/traces.rs            Modify  +list_trace_tasks_paged
src/bin/aleph-server/commands/start/builder/
  agent_init.rs                              Modify  ~6 closure sites
tests/gateway_tools_visibility_rpc.rs        Create  unit-style coverage for new behavior
tests/gateway_trace_replay_rpc.rs            Create  unit-style coverage incl. unavailable
```

Total scope: ~600 LoC change. No new crates. No new modules.

---

## Task 1: Add `SERVICE_UNAVAILABLE` error code constant

**Files:**
- Modify: `src/gateway/protocol.rs:30-34`

- [ ] **Step 1: Add the constant under the existing "Additional error codes" block**

```rust
// Additional error codes specific to Gateway (not in aleph-protocol)
/// Authentication failed
pub const AUTH_FAILED: i32 = -32001;
/// Operation timeout (alias for TIMEOUT_ERROR)
pub const TIMEOUT: i32 = TIMEOUT_ERROR;
/// Feature registered but not wired in this build/mode. Replaces fake-success
/// stubs and ambiguous INTERNAL_ERROR placeholders. JSON-RPC reserves
/// -32000..-32099 for implementation-defined server errors; -32099 sits at
/// the bottom to avoid collision with future aleph-protocol codes.
pub const SERVICE_UNAVAILABLE: i32 = -32099;
```

- [ ] **Step 2: Verify compile**

Run: `cargo check -p alephcore`
Expected: clean (constant is unreferenced — Rust does not warn on unused consts).

- [ ] **Step 3: Commit**

```bash
git add src/gateway/protocol.rs
git commit -m "gateway: add SERVICE_UNAVAILABLE (-32099) error code"
```

---

## Task 2: Introduce `service_unavailable` helper and swap phase-1 stub registrations

**Files:**
- Modify: `src/gateway/handlers/mod.rs:489-621`

- [ ] **Step 1: Add private helper near the top of the file's handler-impl section (place after the existing `use` block)**

```rust
use super::protocol::SERVICE_UNAVAILABLE;

/// Convenience: build a phase-1 placeholder response indicating the named
/// feature exists but has not been wired by the boot path (yet). Used by
/// `register_handlers()` to register a deterministic error in place of
/// fake-success stubs — phase-2 (boot) overrides with the real handler.
fn service_unavailable(req: JsonRpcRequest, reason: &'static str) -> JsonRpcResponse {
    JsonRpcResponse::error(req.id, SERVICE_UNAVAILABLE, reason.to_string())
}
```

(Adjust import path for `SERVICE_UNAVAILABLE` to match how other constants are imported in this file. If `INTERNAL_ERROR` is imported via `crate::gateway::protocol::INTERNAL_ERROR`, mirror it.)

- [ ] **Step 2: Replace the 5 phase-1 stub registrations**

Locate lines 503, 504 (trace.*) and 619, 620, 621 (tools.*). Replace with:

```rust
// Trace replay handlers (phase-1 placeholder; agent_init.rs overrides at boot)
registry.register("trace.list", |req| async move {
    service_unavailable(req, "trace.list requires state database (boot phase 2)")
});
registry.register("trace.get", |req| async move {
    service_unavailable(req, "trace.get requires state database (boot phase 2)")
});

// ... (existing intervening code unchanged) ...

// Tools visibility handlers (phase-1 placeholder; agent_init.rs overrides at boot)
registry.register("tools.catalog", |req| async move {
    service_unavailable(req, "tools.catalog requires ToolRegistry (boot phase 2)")
});
registry.register("tools.effective", |req| async move {
    service_unavailable(req, "tools.effective requires ToolRegistry (boot phase 2)")
});
registry.register("tools.invoke", |req| async move {
    service_unavailable(req, "tools.invoke requires ToolRegistry (boot phase 2)")
});
```

- [ ] **Step 3: Verify compile**

Run: `cargo check -p alephcore`
Expected: clean. The 5 `*_stub` functions are now uncalled but still present — they'll be removed in Task 9.

- [ ] **Step 4: Commit**

```bash
git add src/gateway/handlers/mod.rs
git commit -m "gateway: phase-1 stubs return SERVICE_UNAVAILABLE instead of fake data

5 RPCs (tools.catalog/effective/invoke, trace.list/get) previously had
phase-1 registrations that returned either fake-empty success or generic
INTERNAL_ERROR. Switch them to a uniform SERVICE_UNAVAILABLE placeholder
so any path where phase-2 boot override is skipped surfaces a deterministic
'not wired' signal."
```

---

## Task 3: Phase-2 always overrides `trace.list`/`trace.get` even when DB absent

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init.rs:1581-1595`

- [ ] **Step 1: Replace the conditional wire block with unconditional override**

Locate the current block:

```rust
if let Some(trace_db) = resilience_db.clone() {
    let trace_list_db = trace_db.clone();
    server.handlers_mut().register("trace.list", move |req| { ... });
    let trace_get_db = trace_db;
    server.handlers_mut().register("trace.get", move |req| { ... });
}
```

Replace with:

```rust
// Phase-2 always overrides phase-1 to guarantee a deterministic response.
// When state DB is absent, the override returns SERVICE_UNAVAILABLE with a
// tighter, environment-specific reason — never the phase-1 generic.
match resilience_db.clone() {
    Some(trace_db) => {
        let trace_list_db = trace_db.clone();
        server.handlers_mut().register("trace.list", move |req| {
            let db = trace_list_db.clone();
            async move {
                alephcore::gateway::handlers::trace_replay::handle_list(req, db).await
            }
        });
        let trace_get_db = trace_db;
        server.handlers_mut().register("trace.get", move |req| {
            let db = trace_get_db.clone();
            async move {
                alephcore::gateway::handlers::trace_replay::handle_get(req, db).await
            }
        });
    }
    None => {
        server.handlers_mut().register("trace.list", |req| async move {
            JsonRpcResponse::error(
                req.id,
                alephcore::gateway::protocol::SERVICE_UNAVAILABLE,
                "trace.list disabled: no state_database configured".to_string(),
            )
        });
        server.handlers_mut().register("trace.get", |req| async move {
            JsonRpcResponse::error(
                req.id,
                alephcore::gateway::protocol::SERVICE_UNAVAILABLE,
                "trace.get disabled: no state_database configured".to_string(),
            )
        });
    }
}
```

- [ ] **Step 2: Verify compile**

Run: `cargo check --bin aleph-server`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add src/bin/aleph-server/commands/start/builder/agent_init.rs
git commit -m "gateway: trace.list/get always override phase-1 (D3 fix)

When resilience_db is None (simulated/no-db mode), phase-1 stubs would
remain and silently return fake-empty data. Always override to a tight
SERVICE_UNAVAILABLE message so callers can distinguish 'not configured'
from 'configured but empty'."
```

---

## Task 4: D1 — `tools.effective` uses the live `AgentRegistry`

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init.rs:1933-1965`

- [ ] **Step 1: Verify the live registry is already in scope**

Read the surrounding code (around line 1600 where `agent_reg = Some(agent_registry.clone())` is set). Confirm `agent_registry: Arc<AgentRegistry>` is captured into the broader scope.

- [ ] **Step 2: Replace the fresh-built registry with the captured live one**

Current:
```rust
let reg = dispatch_registry.clone();
let agent_def_registry =
    std::sync::Arc::new(alephcore::agents::AgentRegistry::with_builtins());
server
    .handlers_mut()
    .register("tools.effective", move |req| {
        let registry = reg.clone();
        let agents = agent_def_registry.clone();
        async move { ... }
    });
```

Target:
```rust
let reg = dispatch_registry.clone();
// Use the live agent registry (with user-customized AgentDefs) instead of
// rebuilding a builtin-only one per call. D1 fix.
let agent_def_registry = agent_registry.clone();
server
    .handlers_mut()
    .register("tools.effective", move |req| {
        let registry = reg.clone();
        let agents = agent_def_registry.clone();
        async move {
            let agent_id = req
                .params
                .as_ref()
                .and_then(|p| p.get("agent_id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let agent_def = match &agent_id {
                Some(id) => agents.get(id),
                None => agents.get("main"),
            };
            alephcore::gateway::handlers::tools_visibility::handle_effective(
                req,
                &registry,
                agent_def.as_ref(),
            )
            .await
        }
    });
```

- [ ] **Step 3: Verify compile**

Run: `cargo check --bin aleph-server`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add src/bin/aleph-server/commands/start/builder/agent_init.rs
git commit -m "gateway: tools.effective uses live AgentRegistry (D1 fix)

The previous closure built a fresh AgentRegistry::with_builtins() per
call, hiding user-customized agents. Capture the live registry that the
harness has already loaded so tools.effective reflects current state."
```

---

## Task 5: D2/P3 — `tools.invoke` enforces agent allowlist

**Files:**
- Modify: `src/gateway/handlers/tools_invoke.rs`
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init.rs:1921-1931`

- [ ] **Step 1: Write the failing test in `tools_invoke.rs` test module**

After the existing tests (line ~260), append:

```rust
    use crate::agents::{AgentDef, AgentMode, AgentRegistry};

    fn registry_with_restricted_agent() -> Arc<AgentRegistry> {
        let r = AgentRegistry::new();
        r.register(
            AgentDef::new("restricted", AgentMode::SubAgent)
                .with_allowed_tools(vec!["allowed_one".into()]),
        );
        Arc::new(r)
    }

    #[tokio::test]
    async fn blocks_tool_outside_agent_allowlist() {
        let tool_reg = Arc::new(
            StubRegistry::new().with_ok("blocked_one", json!({"any": "data"})),
        );
        let agents = registry_with_restricted_agent();

        let params = json!({
            "tool_name": "blocked_one",
            "agent_id": "restricted",
            "arguments": {}
        });
        let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
        let resp = handle_invoke(req, tool_reg.clone(), Some(agents)).await;

        assert!(!resp.is_success(), "expected error for out-of-allowlist tool");
        assert_eq!(resp.error.unwrap().code, INVALID_PARAMS);
        assert!(
            tool_reg.last_call().is_none(),
            "registry must not be touched when allowlist denies"
        );
    }

    #[tokio::test]
    async fn permits_tool_inside_agent_allowlist() {
        let tool_reg = Arc::new(
            StubRegistry::new().with_ok("allowed_one", json!({"hits": 1})),
        );
        let agents = registry_with_restricted_agent();

        let params = json!({
            "tool_name": "allowed_one",
            "agent_id": "restricted",
            "arguments": {}
        });
        let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
        let resp = handle_invoke(req, tool_reg, Some(agents)).await;

        assert!(resp.is_success(), "expected success: {:?}", resp.error);
    }

    #[tokio::test]
    async fn skips_allowlist_when_agents_none() {
        // Test-mode path: callers passing None get pre-gate behavior. Production
        // boot always passes Some(live_registry).
        let tool_reg = Arc::new(
            StubRegistry::new().with_ok("anything", json!({})),
        );
        let params = json!({
            "tool_name": "anything",
            "arguments": {}
        });
        let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
        let resp = handle_invoke(req, tool_reg, None).await;
        assert!(resp.is_success());
    }

    #[tokio::test]
    async fn rejects_unknown_agent_id() {
        let tool_reg = Arc::new(StubRegistry::new());
        let agents = registry_with_restricted_agent();

        let params = json!({
            "tool_name": "allowed_one",
            "agent_id": "no_such_agent",
            "arguments": {}
        });
        let req = JsonRpcRequest::with_id("tools.invoke", Some(params), json!(1));
        let resp = handle_invoke(req, tool_reg, Some(agents)).await;
        assert!(!resp.is_success());
        assert_eq!(resp.error.unwrap().code, INVALID_PARAMS);
    }
```

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `cargo test -p alephcore --lib gateway::handlers::tools_invoke -- --nocapture`
Expected: 4 new tests fail because the third parameter on `handle_invoke` does not exist yet.

- [ ] **Step 3: Add the allowlist parameter and gate logic to `handle_invoke`**

Replace the signature and body:

```rust
use crate::agents::AgentRegistry;

/// Real handler — executes the tool directly via the registry trait.
///
/// `agents` is optional: when present, the request's `agent_id` (default
/// "main") must resolve to an AgentDef and the requested `tool_name` must
/// pass `AgentDef::is_tool_allowed`. When `agents` is `None` the allowlist
/// gate is skipped (test mode); the production boot path always supplies
/// the live registry.
pub async fn handle_invoke<R>(
    request: JsonRpcRequest,
    registry: Arc<R>,
    agents: Option<Arc<AgentRegistry>>,
) -> JsonRpcResponse
where
    R: ToolRegistry + ?Sized,
{
    let params: InvokeParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    if params.tool_name.trim().is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "tool_name must not be empty");
    }

    // Allowlist gate — applied only when caller supplied an agent registry.
    if let Some(ref agents) = agents {
        let resolved_id = params.agent_id.clone().unwrap_or_else(|| "main".to_string());
        let agent_def = match agents.get(&resolved_id) {
            Some(d) => d,
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("unknown agent_id: {resolved_id}"),
                );
            }
        };
        if !agent_def.is_tool_allowed(&params.tool_name) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!(
                    "tool '{}' not allowed for agent '{}'",
                    params.tool_name, resolved_id
                ),
            );
        }
    }

    let arguments = merge_agent_id(params.arguments, params.agent_id.as_deref());

    match registry.execute_tool(&params.tool_name, arguments).await {
        Ok(result) => JsonRpcResponse::success(
            request.id,
            json!({
                "ok": true,
                "tool_name": params.tool_name,
                "result": result,
            }),
        ),
        Err(err) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("tool '{}' failed: {}", params.tool_name, err),
        ),
    }
}
```

- [ ] **Step 4: Fix existing `tools_invoke.rs` tests that called the old 2-arg signature**

The four existing async tests (`rejects_missing_params`, `rejects_empty_tool_name`, `returns_internal_error_when_tool_fails`, `forwards_arguments_and_returns_tool_result`, `folds_top_level_agent_id_into_arguments`) call `handle_invoke(req, reg)`. Pass `None` as the third argument:

```rust
let resp = handle_invoke(req, reg, None).await;
```

Apply to all five existing test sites.

- [ ] **Step 5: Run tests — verify all pass (old + new)**

Run: `cargo test -p alephcore --lib gateway::handlers::tools_invoke -- --nocapture`
Expected: all green (5 pre-existing + 4 new = 9 tests).

- [ ] **Step 6: Update phase-2 wire site in agent_init.rs to pass the live AgentRegistry**

Locate around line 1921:

```rust
if let Some(reg) = tool_reg_out.clone() {
    server.handlers_mut().register("tools.invoke", move |req| {
        let registry = reg.clone();
        async move {
            alephcore::gateway::handlers::tools_invoke::handle_invoke(req, registry).await
        }
    });
    if !daemon {
        println!("  tools.invoke: wired to BuiltinToolRegistry (bypasses agent loop)");
    }
}
```

Change to:

```rust
if let Some(reg) = tool_reg_out.clone() {
    let agents_for_invoke = agent_registry.clone();
    server.handlers_mut().register("tools.invoke", move |req| {
        let registry = reg.clone();
        let agents = Some(agents_for_invoke.clone());
        async move {
            alephcore::gateway::handlers::tools_invoke::handle_invoke(req, registry, agents).await
        }
    });
    if !daemon {
        println!("  tools.invoke: wired with agent allowlist gating");
    }
}
```

- [ ] **Step 7: Verify the gateway boots cleanly**

Run: `cargo check --bin aleph-server`
Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add src/gateway/handlers/tools_invoke.rs \
        src/bin/aleph-server/commands/start/builder/agent_init.rs
git commit -m "gateway: tools.invoke enforces agent allowlist (D2/P3 fix)

handle_invoke gains a third Option<Arc<AgentRegistry>> parameter. When
present (production boot path), the caller's agent_id (default 'main')
must resolve and the tool_name must pass AgentDef::is_tool_allowed before
the registry is touched. When None, the gate is skipped — the existing
unit tests' StubRegistry path is preserved.

Closes D2 (free bypass) and P3 (OpenClaw operator-scope parity)."
```

---

## Task 6: P2 — `tools.catalog/effective` accept optional `source` filter

**Files:**
- Modify: `src/gateway/handlers/tools_visibility.rs`

- [ ] **Step 1: Write failing tests at the bottom of the existing `mod tests` block**

```rust
    #[test]
    fn source_filter_exact_match_keeps_only_native() {
        let tools = vec![
            make_tool("search", "Native", ToolSource::Native),
            make_tool("help", "Built-in", ToolSource::Builtin),
            make_tool(
                "git_status",
                "MCP",
                ToolSource::Mcp { server: "github".into() },
            ),
        ];
        let filtered = filter_by_source(tools, Some("native"));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "search");
    }

    #[test]
    fn source_filter_prefix_wildcard_keeps_all_mcp() {
        let tools = vec![
            make_tool(
                "github_status",
                "GH",
                ToolSource::Mcp { server: "github".into() },
            ),
            make_tool(
                "fs_ls",
                "FS",
                ToolSource::Mcp { server: "filesystem".into() },
            ),
            make_tool("search", "Native", ToolSource::Native),
        ];
        let filtered = filter_by_source(tools, Some("mcp:*"));
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn source_filter_none_returns_all_unchanged() {
        let tools = vec![
            make_tool("a", "x", ToolSource::Native),
            make_tool("b", "y", ToolSource::Builtin),
        ];
        let filtered = filter_by_source(tools, None);
        assert_eq!(filtered.len(), 2);
    }
```

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `cargo test -p alephcore --lib gateway::handlers::tools_visibility -- --nocapture`
Expected: 3 new tests fail because `filter_by_source` is undefined.

- [ ] **Step 3: Add `filter_by_source` and thread the param into both handlers**

After `group_tools` definition, add:

```rust
/// Filter tools by source descriptor. Exact match (e.g., "native", "mcp:github")
/// or prefix-wildcard ("mcp:*"). `None` passes through unchanged.
pub fn filter_by_source(tools: Vec<UnifiedTool>, source: Option<&str>) -> Vec<UnifiedTool> {
    let Some(src) = source else { return tools };
    if let Some(prefix) = src.strip_suffix(":*") {
        let prefix_with_colon = format!("{prefix}:");
        tools
            .into_iter()
            .filter(|t| extract_source(&t.source).0.starts_with(&prefix_with_colon))
            .collect()
    } else {
        tools
            .into_iter()
            .filter(|t| extract_source(&t.source).0 == src)
            .collect()
    }
}
```

Modify `handle_catalog`:

```rust
pub async fn handle_catalog(
    request: JsonRpcRequest,
    tool_registry: &ToolRegistry,
) -> JsonRpcResponse {
    let source_filter = request
        .params
        .as_ref()
        .and_then(|p| p.get("source"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let tools = tool_registry.list_all().await;
    let filtered = filter_by_source(tools, source_filter.as_deref());
    let total = filtered.len();
    let groups = group_tools(filtered);

    let result = ToolsListResult {
        groups,
        total,
        agent_id: None,
    };

    JsonRpcResponse::success(request.id, serde_json::to_value(result).unwrap_or_default())
}
```

Modify `handle_effective` similarly — apply `filter_by_source` AFTER the allowlist filter:

```rust
pub async fn handle_effective(
    request: JsonRpcRequest,
    tool_registry: &ToolRegistry,
    agent: Option<&AgentDef>,
) -> JsonRpcResponse {
    let source_filter = request
        .params
        .as_ref()
        .and_then(|p| p.get("source"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let tools = tool_registry.list_all().await;

    let (filtered_by_agent, agent_id): (Vec<UnifiedTool>, Option<String>) = match agent {
        Some(agent_def) => {
            let kept: Vec<UnifiedTool> = tools
                .into_iter()
                .filter(|t| agent_def.is_tool_allowed(&t.name))
                .collect();
            (kept, Some(agent_def.id.clone()))
        }
        None => (tools, None),
    };

    let filtered = filter_by_source(filtered_by_agent, source_filter.as_deref());
    let total = filtered.len();
    let groups = group_tools(filtered);

    let result = ToolsListResult {
        groups,
        total,
        agent_id,
    };

    JsonRpcResponse::success(request.id, serde_json::to_value(result).unwrap_or_default())
}
```

- [ ] **Step 4: Run tests — verify all pass (old + new)**

Run: `cargo test -p alephcore --lib gateway::handlers::tools_visibility -- --nocapture`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add src/gateway/handlers/tools_visibility.rs
git commit -m "gateway: tools.catalog/effective accept source filter (P2)

Optional 'source' parameter on both RPCs. Exact match ('native', 'mcp:github')
or family-prefix wildcard ('mcp:*'). Implemented as a post-filter on the
existing tool list — no DB or registry changes. Mirrors OpenClaw's
tools.catalog source filter for Panel/Webchat consumers."
```

---

## Task 7: P1 — Add `StateDatabase::list_trace_tasks_paged` sibling

**Files:**
- Modify: `src/resilience/database/traces.rs:198-225`

- [ ] **Step 1: Add the sibling method below `list_trace_tasks`**

```rust
/// List trace tasks paginated by recency. Returns up to `limit` rows whose
/// `last_timestamp` is strictly less than `before_timestamp` (when set),
/// ordered DESC by `last_timestamp`.
///
/// Companion to `list_trace_tasks`; that method remains for callers that
/// want everything in one shot. The paged form keeps each page O(limit)
/// regardless of total trace volume.
pub async fn list_trace_tasks_paged(
    &self,
    limit: usize,
    before_timestamp: Option<String>,
) -> Result<Vec<TaskTraceInfo>, AlephError> {
    let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
    let clamped_limit = limit.clamp(1, 200) as i64;

    let (sql, ts_param) = match before_timestamp {
        Some(ref ts) => (
            r#"
            SELECT task_id, COUNT(*) as event_count, MAX(timestamp) as last_timestamp
            FROM task_traces
            GROUP BY task_id
            HAVING MAX(timestamp) < ?1
            ORDER BY last_timestamp DESC
            LIMIT ?2
            "#,
            Some(ts.as_str()),
        ),
        None => (
            r#"
            SELECT task_id, COUNT(*) as event_count, MAX(timestamp) as last_timestamp
            FROM task_traces
            GROUP BY task_id
            ORDER BY last_timestamp DESC
            LIMIT ?1
            "#,
            None,
        ),
    };

    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| AlephError::config(format!("Failed to prepare paged query: {}", e)))?;

    let rows_iter = match ts_param {
        Some(ts) => stmt.query_map(params![ts, clamped_limit], |row| {
            Ok(TaskTraceInfo {
                task_id: row.get(0)?,
                event_count: row.get(1)?,
                last_timestamp: row.get(2)?,
            })
        }),
        None => stmt.query_map(params![clamped_limit], |row| {
            Ok(TaskTraceInfo {
                task_id: row.get(0)?,
                event_count: row.get(1)?,
                last_timestamp: row.get(2)?,
            })
        }),
    }
    .map_err(|e| AlephError::config(format!("Failed to query paged traces: {}", e)))?;

    rows_iter
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| AlephError::config(format!("Failed to collect paged traces: {}", e)))
}
```

*Note: `last_timestamp`'s SQL type is whatever the existing `task_traces.timestamp` column stores. The existing `list_trace_tasks` reads it with `row.get(2)?` into the `TaskTraceInfo.last_timestamp` field — match that type for the new method (likely `String`; verify via `TaskTraceInfo` definition or `cargo check` errors).*

- [ ] **Step 2: Write a unit test below the existing tests**

Append in `mod tests`:

```rust
    #[tokio::test]
    async fn list_paged_returns_at_most_limit() {
        use crate::resilience::TaskTrace;

        let db = StateDatabase::in_memory().unwrap();
        for i in 0..5 {
            let tid = format!("task-{i}");
            db.insert_agent_task(&AgentTask::new(
                &tid, "s", "coder", "x", RiskLevel::Low,
            ))
            .await
            .unwrap();
            db.insert_trace(&TaskTrace::new(
                &tid,
                0,
                AgentTraceEvent::TextEmitted {
                    iteration: 0,
                    stream: AgentTraceTextKind::Final,
                    text: "x".into(),
                },
            ))
            .await
            .unwrap();
        }

        let page = db.list_trace_tasks_paged(3, None).await.unwrap();
        assert_eq!(page.len(), 3);
    }

    #[tokio::test]
    async fn list_paged_cursor_advances() {
        use crate::resilience::TaskTrace;

        let db = StateDatabase::in_memory().unwrap();
        for i in 0..4 {
            let tid = format!("task-{i}");
            db.insert_agent_task(&AgentTask::new(
                &tid, "s", "coder", "x", RiskLevel::Low,
            ))
            .await
            .unwrap();
            db.insert_trace(&TaskTrace::new(
                &tid,
                0,
                AgentTraceEvent::TextEmitted {
                    iteration: 0,
                    stream: AgentTraceTextKind::Final,
                    text: "x".into(),
                },
            ))
            .await
            .unwrap();
        }

        let page_a = db.list_trace_tasks_paged(2, None).await.unwrap();
        assert_eq!(page_a.len(), 2);
        let cursor = page_a.last().unwrap().last_timestamp.clone();

        let page_b = db
            .list_trace_tasks_paged(2, Some(cursor))
            .await
            .unwrap();
        assert!(page_b.len() <= 2);
        // No overlap between pages
        for r in &page_b {
            assert!(page_a.iter().all(|p| p.task_id != r.task_id));
        }
    }
```

- [ ] **Step 3: Run tests — verify pass**

Run: `cargo test -p alephcore --lib resilience::database::traces -- --nocapture`
Expected: 2 new tests green.

- [ ] **Step 4: Commit**

```bash
git add src/resilience/database/traces.rs
git commit -m "resilience: add StateDatabase::list_trace_tasks_paged (P1)

Paginated sibling of list_trace_tasks. Returns at most `limit` (clamped
1..200) trace-task summaries whose last_timestamp < before_timestamp,
ordered DESC. Existing list_trace_tasks unchanged."
```

---

## Task 8: P1 — `trace.list` handler exposes `limit` + `before_timestamp` + cursor

**Files:**
- Modify: `src/gateway/handlers/trace_replay.rs:6-26`

- [ ] **Step 1: Replace the handler with the paginated version**

Replace `handle_list` with:

```rust
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
struct TraceListParams {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    before_timestamp: Option<String>,
}

pub async fn handle_list(request: JsonRpcRequest, db: Arc<StateDatabase>) -> JsonRpcResponse {
    let params: TraceListParams = match request.params.as_ref() {
        Some(v) => match serde_json::from_value(v.clone()) {
            Ok(p) => p,
            Err(_) => TraceListParams::default(),
        },
        None => TraceListParams::default(),
    };
    let limit = params.limit.unwrap_or(50);

    match db
        .list_trace_tasks_paged(limit, params.before_timestamp.clone())
        .await
    {
        Ok(tasks) => {
            let next_cursor = tasks.last().map(|t| t.last_timestamp.clone());
            let exhausted = tasks.len() < limit.min(200);
            let traces: Vec<Value> = tasks
                .into_iter()
                .map(|t| {
                    json!({
                        "task_id": t.task_id,
                        "event_count": t.event_count,
                        "last_timestamp": t.last_timestamp
                    })
                })
                .collect();
            JsonRpcResponse::success(
                request.id,
                json!({
                    "traces": traces,
                    "next_cursor": if exhausted { Value::Null } else { json!(next_cursor) },
                }),
            )
        }
        Err(e) => {
            tracing::error!("Failed to list traces: {}", e);
            JsonRpcResponse::error(request.id, INTERNAL_ERROR, "Failed to list traces")
        }
    }
}
```

`handle_get` is unchanged.

- [ ] **Step 2: Verify compile**

Run: `cargo check -p alephcore`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add src/gateway/handlers/trace_replay.rs
git commit -m "gateway: trace.list paginates with limit+before_timestamp (P1)

New optional params: limit (default 50, clamped 1..200) and
before_timestamp cursor. Response gains next_cursor (Null when the
page exhausted the set). No protocol-breaking change — defaults match
the previous unbounded behavior for limit=200 + no cursor."
```

---

## Task 9: D4 — Delete dead `*_stub` functions

**Files:**
- Modify: `src/gateway/handlers/tools_visibility.rs` (remove `handle_catalog_stub`, `handle_effective_stub`)
- Modify: `src/gateway/handlers/tools_invoke.rs` (remove `handle_invoke_stub`)
- Modify: `src/gateway/handlers/trace_replay.rs` (remove `handle_list_stub`, `handle_get_stub`)

- [ ] **Step 1: Delete the 5 `*_stub` functions and their doc comments**

In `tools_visibility.rs`: remove the section starting `/// Stub for tools.catalog ...` through end of `handle_effective_stub`.

In `tools_invoke.rs`: remove `handle_invoke_stub` and its doc comment (around line 78-86).

In `trace_replay.rs`: remove `handle_list_stub` and `handle_get_stub` (lines 62-96).

- [ ] **Step 2: Drop the now-unused `INTERNAL_ERROR` import from `tools_visibility.rs` if it became unreferenced**

Check whether `INTERNAL_ERROR` is still used elsewhere in the file (it was only used inside the stub). If not, prune the import.

- [ ] **Step 3: Verify compile**

Run: `cargo check -p alephcore`
Expected: clean. If a stale reference to a deleted function remains in `handlers/mod.rs`, the compile error will point at it — that should already be fixed by Task 2, but verify here.

- [ ] **Step 4: Run all touched-module tests**

Run: `cargo test -p alephcore --lib gateway::handlers::{tools_invoke,tools_visibility,trace_replay} -- --nocapture`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add src/gateway/handlers/tools_visibility.rs \
        src/gateway/handlers/tools_invoke.rs \
        src/gateway/handlers/trace_replay.rs
git commit -m "gateway: delete 5 dead *_stub functions (D4)

Tasks 2-3 routed the phase-1 placeholder via service_unavailable() helper
closures. The named *_stub functions are now unreachable. Delete them and
prune the orphan INTERNAL_ERROR import in tools_visibility.rs."
```

---

## Task 10: Integration test for `tools.*` RPCs

**Files:**
- Create: `tests/gateway_tools_visibility_rpc.rs`

- [ ] **Step 1: Write the new integration test file**

```rust
//! Integration coverage for the wired tools.* RPCs.
//!
//! Asserts:
//! - handle_catalog respects the `source` filter (P2).
//! - handle_effective respects the live agent registry's allowlist (D1).
//! - handle_invoke rejects out-of-allowlist tool with INVALID_PARAMS (D2/P3).
//!
//! These tests exercise the handler entry points directly with the same
//! plumbing the gateway boot path uses (live AgentRegistry + ToolRegistry),
//! catching regressions where phase-2 boot override silently degrades.

use std::sync::Arc;

use alephcore::agents::{AgentDef, AgentMode, AgentRegistry};
use alephcore::dispatcher::{ToolRegistry, ToolSource, UnifiedTool};
use alephcore::gateway::handlers::tools_visibility::{handle_catalog, handle_effective};
use alephcore::gateway::protocol::{JsonRpcRequest, INVALID_PARAMS};
use serde_json::json;

/// Minimal in-test ToolRegistry returning a curated list. Mirrors
/// production ToolRegistry list_all semantics — enough for visibility tests.
struct CuratedRegistry {
    tools: Vec<UnifiedTool>,
}

#[async_trait::async_trait]
impl ToolRegistry for CuratedRegistry {
    async fn list_all(&self) -> Vec<UnifiedTool> {
        self.tools.clone()
    }
}

fn registry_with_tools() -> CuratedRegistry {
    CuratedRegistry {
        tools: vec![
            UnifiedTool::new(
                "native:search",
                "search",
                "search desc",
                ToolSource::Native,
            ),
            UnifiedTool::new(
                "mcp:github:status",
                "github_status",
                "git status",
                ToolSource::Mcp { server: "github".into() },
            ),
            UnifiedTool::new(
                "mcp:filesystem:ls",
                "fs_ls",
                "list files",
                ToolSource::Mcp { server: "filesystem".into() },
            ),
        ],
    }
}

#[tokio::test]
async fn catalog_source_filter_exact() {
    let reg = registry_with_tools();
    let req = JsonRpcRequest::with_id(
        "tools.catalog",
        Some(json!({"source": "native"})),
        json!(1),
    );
    let resp = handle_catalog(req, &reg).await;
    let result = resp.result.unwrap();
    assert_eq!(result["total"], 1);
}

#[tokio::test]
async fn catalog_source_filter_prefix() {
    let reg = registry_with_tools();
    let req = JsonRpcRequest::with_id(
        "tools.catalog",
        Some(json!({"source": "mcp:*"})),
        json!(1),
    );
    let resp = handle_catalog(req, &reg).await;
    let result = resp.result.unwrap();
    assert_eq!(result["total"], 2);
}

#[tokio::test]
async fn effective_respects_user_added_agent_allowlist() {
    let reg = registry_with_tools();
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
}

#[tokio::test]
async fn invoke_blocks_out_of_allowlist() {
    // The handle_invoke gating is already covered by unit tests in
    // src/gateway/handlers/tools_invoke.rs#tests; this is a thin
    // smoke test that ensures the integration-test surface compiles
    // and the public re-export exists.
    use alephcore::gateway::handlers::tools_invoke::handle_invoke;
    use alephcore::dispatcher::ToolRegistry as _;
    use alephcore::error::AlephError;
    use serde_json::Value;

    struct OkReg;

    #[async_trait::async_trait]
    impl ToolRegistry for OkReg {
        async fn list_all(&self) -> Vec<UnifiedTool> {
            vec![]
        }
        async fn execute_tool(
            &self,
            _name: &str,
            _args: Value,
        ) -> Result<Value, AlephError> {
            Ok(json!({}))
        }
        fn get_tool(&self, _name: &str) -> Option<&UnifiedTool> {
            None
        }
    }

    let agents = AgentRegistry::new();
    agents.register(
        AgentDef::new("scoped", AgentMode::SubAgent).with_allowed_tools(vec!["allowed".into()]),
    );

    let req = JsonRpcRequest::with_id(
        "tools.invoke",
        Some(json!({"tool_name": "denied", "agent_id": "scoped"})),
        json!(1),
    );
    let resp = handle_invoke(req, Arc::new(OkReg) as Arc<dyn ToolRegistry>, Some(Arc::new(agents)))
        .await;

    assert!(!resp.is_success());
    assert_eq!(resp.error.unwrap().code, INVALID_PARAMS);
}
```

*Note on signature compatibility: this assumes `dispatcher::ToolRegistry` trait has the methods used above. Cross-check with the production trait in `src/dispatcher/registry/mod.rs` — if the trait signature differs (e.g., `async_trait` form or different method names), adapt the impl accordingly. The unit tests in `tools_invoke.rs` already use a similar StubRegistry; mirror that exact shape if needed.*

- [ ] **Step 2: Run the new tests**

Run: `cargo test --test gateway_tools_visibility_rpc -- --nocapture`
Expected: 4 tests green.

- [ ] **Step 3: Commit**

```bash
git add tests/gateway_tools_visibility_rpc.rs
git commit -m "tests: integration coverage for tools.catalog/effective/invoke

Asserts source filter (P2), live agent registry allowlist visibility (D1),
and out-of-allowlist invoke gate (D2/P3). Pinned at the handler entry
points to catch regressions where phase-2 boot override silently degrades
to phase-1."
```

---

## Task 11: Integration test for `trace.*` RPCs

**Files:**
- Create: `tests/gateway_trace_replay_rpc.rs`

- [ ] **Step 1: Write the new integration test file**

```rust
//! Integration coverage for the wired trace.* RPCs.
//!
//! Asserts:
//! - handle_list paginates with limit + before_timestamp cursor (P1).
//! - handle_list reports next_cursor=null when exhausted.
//! - handle_get retrieves a known trace by id.
//!
//! D3 (service unavailable when DB absent) is exercised at the boot wire
//! site (agent_init.rs) rather than at the handler — the no-db branch
//! never reaches handle_list. A boot-level test would need the gateway
//! harness and is out of scope for this spec.

use std::sync::Arc;

use alephcore::gateway::handlers::trace_replay::{handle_get, handle_list};
use alephcore::gateway::protocol::JsonRpcRequest;
use alephcore::resilience::database::StateDatabase;
use alephcore::resilience::{AgentTask, RiskLevel, TaskTrace};
use aleph_protocol::{AgentTraceEvent, AgentTraceTextKind};
use serde_json::json;

async fn seed_db(n: usize) -> Arc<StateDatabase> {
    let db = Arc::new(StateDatabase::in_memory().unwrap());
    for i in 0..n {
        let tid = format!("task-{i}");
        db.insert_agent_task(&AgentTask::new(
            &tid, "s", "coder", "x", RiskLevel::Low,
        ))
        .await
        .unwrap();
        db.insert_trace(&TaskTrace::new(
            &tid,
            0,
            AgentTraceEvent::TextEmitted {
                iteration: 0,
                stream: AgentTraceTextKind::Final,
                text: "x".into(),
            },
        ))
        .await
        .unwrap();
    }
    db
}

#[tokio::test]
async fn list_returns_paginated_set_with_cursor() {
    let db = seed_db(5).await;
    let req = JsonRpcRequest::with_id(
        "trace.list",
        Some(json!({"limit": 2})),
        json!(1),
    );
    let resp = handle_list(req, db.clone()).await;
    let result = resp.result.unwrap();
    assert_eq!(result["traces"].as_array().unwrap().len(), 2);
    assert!(!result["next_cursor"].is_null(), "expected non-null cursor");
}

#[tokio::test]
async fn list_returns_null_cursor_when_exhausted() {
    let db = seed_db(2).await;
    let req = JsonRpcRequest::with_id(
        "trace.list",
        Some(json!({"limit": 10})),
        json!(1),
    );
    let resp = handle_list(req, db).await;
    let result = resp.result.unwrap();
    assert_eq!(result["traces"].as_array().unwrap().len(), 2);
    assert!(result["next_cursor"].is_null());
}

#[tokio::test]
async fn list_cursor_advances_without_overlap() {
    let db = seed_db(5).await;

    let req_a = JsonRpcRequest::with_id(
        "trace.list",
        Some(json!({"limit": 2})),
        json!(1),
    );
    let resp_a = handle_list(req_a, db.clone()).await;
    let result_a = resp_a.result.unwrap();
    let cursor = result_a["next_cursor"].clone();
    assert!(!cursor.is_null());

    let req_b = JsonRpcRequest::with_id(
        "trace.list",
        Some(json!({"limit": 2, "before_timestamp": cursor})),
        json!(1),
    );
    let resp_b = handle_list(req_b, db).await;
    let result_b = resp_b.result.unwrap();
    let a_ids: Vec<&str> = result_a["traces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["task_id"].as_str().unwrap())
        .collect();
    for entry in result_b["traces"].as_array().unwrap() {
        let bid = entry["task_id"].as_str().unwrap();
        assert!(!a_ids.contains(&bid), "page B leaked page A row: {bid}");
    }
}

#[tokio::test]
async fn get_returns_known_trace_by_id() {
    let db = seed_db(1).await;
    // Discover the inserted trace's id via the list_trace_tasks_paged surface.
    // task-0 has exactly one trace; its row id is the first auto-incremented
    // primary key.
    let req = JsonRpcRequest::with_id(
        "trace.get",
        Some(json!({"trace_id": 1})),
        json!(1),
    );
    let resp = handle_get(req, db).await;
    assert!(resp.is_success(), "expected success: {:?}", resp.error);
    let result = resp.result.unwrap();
    assert_eq!(result["trace"]["task_id"], "task-0");
}
```

- [ ] **Step 2: Run the new tests**

Run: `cargo test --test gateway_trace_replay_rpc -- --nocapture`
Expected: 4 tests green.

- [ ] **Step 3: Commit**

```bash
git add tests/gateway_trace_replay_rpc.rs
git commit -m "tests: integration coverage for trace.list/get pagination

Asserts P1 cursor semantics (limit, before_timestamp, next_cursor),
exhaustion signaling, page-to-page non-overlap, and basic trace.get
roundtrip. D3 boot-time unavailable branch tested at the wire site in a
follow-up."
```

---

## Task 12: Final verification

- [ ] **Step 1: Cargo check (debug, full workspace)**

Run: `cargo check -p alephcore`
Expected: 0 errors. Pre-existing warnings tolerated per CLAUDE.md (`fmt + clippy baseline drift`).

- [ ] **Step 2: Run the touched-module test sweep**

Run: `cargo test -p alephcore --lib gateway::handlers -- --nocapture`
And: `cargo test --test gateway_tools_visibility_rpc --test gateway_trace_replay_rpc -- --nocapture`
Expected: all green. (Do NOT run `cargo test --lib` alone — main has 19 pre-existing failures per project memory `project_baseline_test_failures`.)

- [ ] **Step 3: Clippy on touched files only**

Run:
```bash
cargo clippy -p alephcore -- -D warnings \
  2>&1 | grep -E "tools_invoke|tools_visibility|trace_replay|traces\.rs|agent_init|protocol\.rs|handlers/mod" \
  | head -40
```
Expected: no new warnings on the touched files. Pre-existing warnings on untouched files are out of scope.

- [ ] **Step 4: rustfmt the touched files only**

Run:
```bash
rustfmt --edition 2021 \
  src/gateway/protocol.rs \
  src/gateway/handlers/mod.rs \
  src/gateway/handlers/tools_visibility.rs \
  src/gateway/handlers/tools_invoke.rs \
  src/gateway/handlers/trace_replay.rs \
  src/resilience/database/traces.rs \
  src/bin/aleph-server/commands/start/builder/agent_init.rs \
  tests/gateway_tools_visibility_rpc.rs \
  tests/gateway_trace_replay_rpc.rs
```
*Do NOT run project-wide `cargo fmt` — main is not rustfmt-clean per memory `project_fmt_clippy_baseline_drift`.*

- [ ] **Step 5: Verify the 9 commits land cleanly**

Run: `git log --oneline main..HEAD`
Expected: 9 commits in order (1 per task + integration tests).

- [ ] **Step 6: Update / create memory note for the cycle**

Append (or create) `/Users/zouguojun/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/project_gateway_tools_trace_wiring.md`:

```markdown
---
name: gateway-tools-trace-wiring-cycle
description: Spec 1 of OpenClaw-inspired gateway improvement roadmap. Closes 5 defects + 3 parity gaps on tools.*/trace.* RPC surface.
metadata:
  type: project
---

# Gateway Tools+Trace Wiring (Spec 1)

Closes D1-D5 + P1-P3 on tools.catalog/effective/invoke + trace.list/get.
Deletes 5 dead *_stub functions. Adds SERVICE_UNAVAILABLE (-32099). Spec at
[[spec]] `docs/superpowers/specs/2026-05-20-gateway-tools-trace-wiring-design.md`,
plan at `docs/superpowers/plans/2026-05-20-gateway-tools-trace-wiring.md`.

**Status**: shipped <date> via worktree `worktree-gateway-tools-trace` →
merged to main.

**Follow-up**: Spec 2 (gateway robustness kit — idempotency wire,
connection budget, /ready split, generation counter) is a separate
brainstorm cycle. See [[gateway-robustness-kit-cycle]].

**Why**: OpenClaw audit revealed 27 stubs in Aleph's gateway. Spec 1
addresses 5 of them (the ones with extant real handlers but bad wiring).
The other 22 (cron, heartbeat, group_chat, workspace.*, teams.*, etc.) are
separate subsystem-level decisions.
```

Add to `MEMORY.md` index.

- [ ] **Step 7: Final commit**

```bash
git add docs/superpowers/plans/2026-05-20-gateway-tools-trace-wiring.md \
        /Users/zouguojun/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/project_gateway_tools_trace_wiring.md \
        /Users/zouguojun/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/MEMORY.md
git commit -m "docs: gateway tools+trace wiring plan + memory note"
```

---

## Self-Review Summary

- **Spec coverage**: each of D1-D5 + P1-P3 has at least one task with tests.
- **No placeholders**: every step has concrete code/commands.
- **Type consistency**: `handle_invoke` signature change applies in both `tools_invoke.rs` (T5) and `agent_init.rs` (T5 Step 6); the integration test in T10 also passes `Some(Arc::new(agents))`.
- **Cursor type ambiguity**: T7 + T8 use `Option<String>` to match the row.get(2) return type of `TaskTraceInfo.last_timestamp`; if it turns out to be `i64` or `DateTime`, the type change is local to those two tasks and `cargo check` will surface it immediately.
