# Canvas Agent Selector — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an agent selector dropdown to the memory canvas so users can switch which agent's memory graph is rendered, with full state reset on switch and end-to-end `agent_id` plumbing through the four `graph.*` JSON-RPC handlers.

**Architecture:** Backend adds `agent_id: Option<String>` to four params structs and replaces seven production-code references to `DEFAULT_AGENT_ID` with `params.agent_id.as_deref().unwrap_or(...)`. Frontend adds an `AgentSelectorBar` component above `CanvasToolbar`, fetches the agent list once on mount, and uses a Leptos `Effect` subscribing to `agent_id` to reset all canvas state on switch. The four `GraphApi` methods gain an `agent_id: &str` parameter that all call sites in `RadialCanvasView` thread through.

**Tech Stack:** Rust (alephcore gateway, axum), Leptos 0.8 + WASM (interfaces/webchat), serde for JSON-RPC params, tokio for async tests, real `SqliteMemoryBackend` (temp dir) for handler integration tests.

**Spec:** `docs/superpowers/specs/2026-04-27-canvas-agent-selector-design.md`

---

## File Structure

| File | Role | Change |
|---|---|---|
| `src/gateway/handlers/graph_types.rs` | JSON-RPC param structs | Modify — add `agent_id: Option<String>` to 4 structs |
| `src/gateway/handlers/graph.rs` | Handler impls + tests | Modify — thread `agent_id` through 4 handlers; add 8 tests |
| `interfaces/webchat/src/api/graph.rs` | Frontend RPC client | Modify — add `agent_id: &str` parameter to 4 methods |
| `interfaces/webchat/src/views/canvas/agent_selector.rs` | Selector UI | Create — `AgentSelectorBar` Leptos component + `format_agent_option` pure fn |
| `interfaces/webchat/src/views/canvas/mod.rs` | Canvas root | Modify — register submodule, add agent signals, wire bar, add reset Effect, thread `agent_id` to all `GraphApi` calls |

**Spec corrections applied in this plan:**

1. **Spec §1 says `DEFAULT_AGENT_ID` is referenced at 8 sites in `graph.rs`.** That count includes line 406 inside `#[cfg(test)] mod tests` (a fixture string for an existing test). The test fixture stays untouched. **Production-code sites: 7** (lines 102, 148, 162, 232, 249, 259, 321 in current `graph.rs`).
2. **Spec §5.1 example shows `kind_filter: Vec<String>` in `GraphQueryParams`.** That field does not exist in `graph_types.rs` today — the webchat sends `kind_filter` in JSON but serde silently drops it. This plan does **not** add `kind_filter`. The only field added to each params struct is `agent_id: Option<String>`.
3. **Spec §7 prescribes Leptos component-level tests** (`mount_to_body` + signal injection). The webchat crate has no existing component test harness, and standing one up is out of scope for this feature. This plan replaces those tests with a pure-function unit test on the rendering logic (`format_agent_option`) plus a manual smoke checklist (Task 10). Backend testing is unchanged — full TDD with a real temp `SqliteMemoryBackend`.

---

## Task 1: Add `agent_id` field to four params structs

**Files:**
- Modify: `src/gateway/handlers/graph_types.rs:1-44`

This is a pure type change — no behavior yet. Subsequent tasks rely on the field existing.

- [ ] **Step 1: Add `agent_id: Option<String>` to `GraphQueryParams`**

In `src/gateway/handlers/graph_types.rs`, change:

```rust
#[derive(Debug, Deserialize)]
pub struct GraphQueryParams {
    #[serde(default = "default_limit")]
    pub limit: usize,
}
```

to:

```rust
#[derive(Debug, Deserialize)]
pub struct GraphQueryParams {
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub agent_id: Option<String>,
}
```

- [ ] **Step 2: Add `agent_id: Option<String>` to `GraphNeighborsParams`**

Change:

```rust
#[derive(Debug, Deserialize)]
pub struct GraphNeighborsParams {
    pub node_id: String,
    #[serde(default = "default_depth")]
    pub depth: u8,
    #[serde(default = "default_neighbor_limit")]
    pub limit: usize,
}
```

to:

```rust
#[derive(Debug, Deserialize)]
pub struct GraphNeighborsParams {
    pub node_id: String,
    #[serde(default = "default_depth")]
    pub depth: u8,
    #[serde(default = "default_neighbor_limit")]
    pub limit: usize,
    #[serde(default)]
    pub agent_id: Option<String>,
}
```

- [ ] **Step 3: Add `agent_id: Option<String>` to `GraphNodeDetailParams`**

Change:

```rust
#[derive(Debug, Deserialize)]
pub struct GraphNodeDetailParams {
    pub node_id: String,
}
```

to:

```rust
#[derive(Debug, Deserialize)]
pub struct GraphNodeDetailParams {
    pub node_id: String,
    #[serde(default)]
    pub agent_id: Option<String>,
}
```

- [ ] **Step 4: Add `agent_id: Option<String>` to `GraphSearchParams`**

Change:

```rust
#[derive(Debug, Deserialize)]
pub struct GraphSearchParams {
    pub query: String,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
}
```

to:

```rust
#[derive(Debug, Deserialize)]
pub struct GraphSearchParams {
    pub query: String,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
    #[serde(default)]
    pub agent_id: Option<String>,
}
```

- [ ] **Step 5: Verify the file compiles**

Run: `cargo check -p alephcore`
Expected: clean check (no warnings about unused `agent_id` — fields are public so no `#[allow(dead_code)]` needed).

- [ ] **Step 6: Commit**

```bash
git add src/gateway/handlers/graph_types.rs
git commit -m "graph: add optional agent_id field to 4 params structs"
```

---

## Task 2: TDD — thread `agent_id` through `handle_query_impl`

**Files:**
- Modify: `src/gateway/handlers/graph.rs:101-103` (handler call)
- Modify: `src/gateway/handlers/graph.rs:362-485` (test module — add tests)

- [ ] **Step 1: Add a helper that seeds two agents into the DB**

In `src/gateway/handlers/graph.rs`, inside the existing `#[cfg(test)] mod tests { … }` block (after the existing `make_note` helper around line 388), append:

```rust
    /// Seed `db` with one note per agent. Returns (alpha_path, beta_path).
    async fn seed_two_agents(db: &MemoryBackend) -> (String, String) {
        let alpha_note = make_note("AlphaOnly", "concept", vec![]);
        let beta_note = make_note("BetaOnly", "concept", vec![]);
        db.index_note(&alpha_note, "alpha", "concept").await.unwrap();
        db.index_note(&beta_note, "beta", "concept").await.unwrap();
        ("concept/AlphaOnly".to_string(), "concept/BetaOnly".to_string())
    }

    fn query_request(limit: usize, agent_id: Option<&str>) -> JsonRpcRequest {
        let params = match agent_id {
            Some(id) => serde_json::json!({ "limit": limit, "agent_id": id }),
            None => serde_json::json!({ "limit": limit }),
        };
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "graph.query".to_string(),
            params: Some(params),
            id: Some(serde_json::json!(1)),
        }
    }
```

- [ ] **Step 2: Write the two failing tests for `handle_query_impl`**

Append inside the same test module:

```rust
    #[tokio::test]
    async fn graph_query_uses_explicit_agent_id() {
        let db = make_db();
        let (alpha_path, _beta_path) = seed_two_agents(&db).await;

        let req = query_request(50, Some("alpha"));
        let resp = handle_query_impl(req, db).await;
        assert!(resp.error.is_none(), "expected success: {:?}", resp.error);
        let result: GraphQueryResponse =
            serde_json::from_value(resp.result.expect("result")).expect("deserialize");

        let ids: Vec<&str> = result.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains(&alpha_path.as_str()), "alpha note must appear: {ids:?}");
        assert!(!ids.iter().any(|id| id.contains("BetaOnly")),
            "beta note must NOT appear when querying alpha: {ids:?}");
    }

    #[tokio::test]
    async fn graph_query_falls_back_to_default_agent_when_omitted() {
        let db = make_db();
        // Seed only into the default agent
        let main_note = make_note("MainNote", "concept", vec![]);
        db.index_note(&main_note, crate::routing::DEFAULT_AGENT_ID, "concept")
            .await
            .unwrap();

        let req = query_request(50, None);
        let resp = handle_query_impl(req, db).await;
        assert!(resp.error.is_none(), "expected success: {:?}", resp.error);
        let result: GraphQueryResponse =
            serde_json::from_value(resp.result.expect("result")).expect("deserialize");

        let ids: Vec<&str> = result.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.iter().any(|id| id.contains("MainNote")),
            "default agent's note must appear when agent_id omitted: {ids:?}");
    }
```

- [ ] **Step 3: Run the two new tests to verify they fail**

Run: `cargo test -p alephcore --lib graph_query_uses_explicit_agent_id graph_query_falls_back_to_default_agent_when_omitted -- --nocapture`

Expected: `graph_query_uses_explicit_agent_id` FAILS — `alpha note must appear` (because the handler still calls `get_graph_data(DEFAULT_AGENT_ID, …)`, returning empty data since the default agent has no notes). The fallback test may pass coincidentally.

- [ ] **Step 4: Update `handle_query_impl` to read `agent_id` from params**

In `src/gateway/handlers/graph.rs`, replace lines 101-103:

```rust
    let (entries, links) = match db
        .get_graph_data(crate::routing::DEFAULT_AGENT_ID, params.limit)
        .await
    {
```

with:

```rust
    let agent_id = params
        .agent_id
        .as_deref()
        .unwrap_or(crate::routing::DEFAULT_AGENT_ID);
    let (entries, links) = match db.get_graph_data(agent_id, params.limit).await {
```

- [ ] **Step 5: Run the two tests again to verify they pass**

Run: `cargo test -p alephcore --lib graph_query_uses_explicit_agent_id graph_query_falls_back_to_default_agent_when_omitted -- --nocapture`

Expected: both PASS.

- [ ] **Step 6: Run the full handler test module to confirm no regression**

Run: `cargo test -p alephcore --lib gateway::handlers::graph::tests`
Expected: all tests PASS (the existing `graph_neighbors_*` and `compute_hop_depth_*` tests are unaffected).

- [ ] **Step 7: Commit**

```bash
git add src/gateway/handlers/graph.rs
git commit -m "graph: thread agent_id through handle_query_impl"
```

---

## Task 3: TDD — thread `agent_id` through `handle_neighbors_impl`

**Files:**
- Modify: `src/gateway/handlers/graph.rs:145-176` (handler — two call sites: `get_neighbors` and `get_note_index`)
- Modify: `src/gateway/handlers/graph.rs` test module

- [ ] **Step 1: Update the `neighbors_request` helper to accept an optional `agent_id`**

Replace the existing helper near line 390:

```rust
    fn neighbors_request(node_id: &str, depth: u8, limit: usize) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "graph.neighbors".to_string(),
            params: Some(serde_json::json!({
                "node_id": node_id,
                "depth": depth,
                "limit": limit,
            })),
            id: Some(serde_json::json!(1)),
        }
    }
```

with:

```rust
    fn neighbors_request(node_id: &str, depth: u8, limit: usize) -> JsonRpcRequest {
        neighbors_request_with_agent(node_id, depth, limit, None)
    }

    fn neighbors_request_with_agent(
        node_id: &str,
        depth: u8,
        limit: usize,
        agent_id: Option<&str>,
    ) -> JsonRpcRequest {
        let mut params = serde_json::json!({
            "node_id": node_id,
            "depth": depth,
            "limit": limit,
        });
        if let Some(id) = agent_id {
            params["agent_id"] = serde_json::Value::String(id.to_string());
        }
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "graph.neighbors".to_string(),
            params: Some(params),
            id: Some(serde_json::json!(1)),
        }
    }
```

The original two-arg signature still works — existing tests stay green.

- [ ] **Step 2: Write the two failing tests**

Append inside the test module:

```rust
    #[tokio::test]
    async fn graph_neighbors_uses_explicit_agent_id() {
        let db = make_db();
        // Seed alpha with center→neighbor, beta with same center id but different neighbor
        let alpha_center = make_note("Hub", "concept", vec!["concept/AlphaPeer"]);
        let alpha_peer = make_note("AlphaPeer", "concept", vec![]);
        let beta_center = make_note("Hub", "concept", vec!["concept/BetaPeer"]);
        let beta_peer = make_note("BetaPeer", "concept", vec![]);
        db.index_note(&alpha_center, "alpha", "concept").await.unwrap();
        db.index_note(&alpha_peer, "alpha", "concept").await.unwrap();
        db.index_note(&beta_center, "beta", "concept").await.unwrap();
        db.index_note(&beta_peer, "beta", "concept").await.unwrap();

        let req = neighbors_request_with_agent("concept/Hub", 2, 50, Some("alpha"));
        let resp = handle_neighbors_impl(req, db).await;
        assert!(resp.error.is_none(), "expected success: {:?}", resp.error);
        let result: GraphNeighborsResponse =
            serde_json::from_value(resp.result.expect("result")).expect("deserialize");

        let neighbor_ids: Vec<&str> = result.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(neighbor_ids.iter().any(|id| id.contains("AlphaPeer")),
            "alpha neighbor must appear: {neighbor_ids:?}");
        assert!(!neighbor_ids.iter().any(|id| id.contains("BetaPeer")),
            "beta neighbor must NOT appear: {neighbor_ids:?}");
    }

    #[tokio::test]
    async fn graph_neighbors_falls_back_to_default_agent_when_omitted() {
        let db = make_db();
        let center = make_note("Hub", "concept", vec!["concept/MainPeer"]);
        let peer = make_note("MainPeer", "concept", vec![]);
        let agent = crate::routing::DEFAULT_AGENT_ID;
        db.index_note(&center, agent, "concept").await.unwrap();
        db.index_note(&peer, agent, "concept").await.unwrap();

        let req = neighbors_request("concept/Hub", 2, 50); // no agent_id
        let resp = handle_neighbors_impl(req, db).await;
        assert!(resp.error.is_none(), "expected success: {:?}", resp.error);
    }
```

- [ ] **Step 3: Run the two tests to verify they fail**

Run: `cargo test -p alephcore --lib graph_neighbors_uses_explicit_agent_id -- --nocapture`
Expected: FAIL with "expected success" (the handler can't find `concept/Hub` under `alpha` because it queries `DEFAULT_AGENT_ID`).

- [ ] **Step 4: Update `handle_neighbors_impl` to read `agent_id` from params**

Replace lines 145-176 in `src/gateway/handlers/graph.rs`:

```rust
    let (entries, links) = match db
        .get_neighbors(
            &params.node_id,
            crate::routing::DEFAULT_AGENT_ID,
            params.depth,
            params.limit,
        )
        .await
    {
        Ok(data) => data,
        Err(e) => {
            return JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("NoteStore error: {e}"))
        }
    };

    // Look up the center node entry so the frontend can pin it at world origin.
    let center_entry = match db
        .get_note_index(&params.node_id, crate::routing::DEFAULT_AGENT_ID)
        .await
    {
```

with:

```rust
    let agent_id = params
        .agent_id
        .as_deref()
        .unwrap_or(crate::routing::DEFAULT_AGENT_ID);

    let (entries, links) = match db
        .get_neighbors(&params.node_id, agent_id, params.depth, params.limit)
        .await
    {
        Ok(data) => data,
        Err(e) => {
            return JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("NoteStore error: {e}"))
        }
    };

    // Look up the center node entry so the frontend can pin it at world origin.
    let center_entry = match db.get_note_index(&params.node_id, agent_id).await {
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p alephcore --lib graph_neighbors_ -- --nocapture`
Expected: all four `graph_neighbors_*` tests PASS (including the two pre-existing ones).

- [ ] **Step 6: Commit**

```bash
git add src/gateway/handlers/graph.rs
git commit -m "graph: thread agent_id through handle_neighbors_impl"
```

---

## Task 4: TDD — thread `agent_id` through `handle_node_detail_impl`

**Files:**
- Modify: `src/gateway/handlers/graph.rs:230-260` (handler — three references: `get_note_index`, local `agent_id` var, `get_incoming_links`)
- Modify: `src/gateway/handlers/graph.rs` test module

- [ ] **Step 1: Write the two failing tests**

Append inside the test module:

```rust
    fn node_detail_request(node_id: &str, agent_id: Option<&str>) -> JsonRpcRequest {
        let mut params = serde_json::json!({ "node_id": node_id });
        if let Some(id) = agent_id {
            params["agent_id"] = serde_json::Value::String(id.to_string());
        }
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "graph.node_detail".to_string(),
            params: Some(params),
            id: Some(serde_json::json!(1)),
        }
    }

    #[tokio::test]
    async fn graph_node_detail_uses_explicit_agent_id() {
        let db = make_db();
        // Same id under two agents, different content_hash — proves we got alpha's row.
        let alpha = KnowledgeNote {
            content_hash: "alpha_hash".to_string(),
            ..make_note("Shared", "concept", vec![])
        };
        let beta = KnowledgeNote {
            content_hash: "beta_hash".to_string(),
            ..make_note("Shared", "concept", vec![])
        };
        db.index_note(&alpha, "alpha", "concept").await.unwrap();
        db.index_note(&beta, "beta", "concept").await.unwrap();

        let req = node_detail_request("concept/Shared", Some("alpha"));
        let resp = handle_node_detail_impl(req, db).await;
        // The note exists in alpha — handler must succeed (not return "not found").
        assert!(resp.error.is_none(), "expected success: {:?}", resp.error);
    }

    #[tokio::test]
    async fn graph_node_detail_returns_not_found_for_other_agent() {
        let db = make_db();
        // Note exists in alpha but we ask for beta → must be 'not found'.
        let alpha = make_note("AlphaOnly", "concept", vec![]);
        db.index_note(&alpha, "alpha", "concept").await.unwrap();

        let req = node_detail_request("concept/AlphaOnly", Some("beta"));
        let resp = handle_node_detail_impl(req, db).await;
        assert!(resp.error.is_some(), "expected error for cross-agent lookup");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alephcore --lib graph_node_detail_ -- --nocapture`
Expected: `graph_node_detail_uses_explicit_agent_id` may FAIL with "Note not found" (handler queries default agent); `graph_node_detail_returns_not_found_for_other_agent` may unexpectedly PASS for the wrong reason — this is OK, the first failure proves the bug.

- [ ] **Step 3: Update `handle_node_detail_impl`**

In `src/gateway/handlers/graph.rs`, replace lines 230-260:

```rust
    // Fetch the note index entry.
    let entry = match db
        .get_note_index(&params.node_id, crate::routing::DEFAULT_AGENT_ID)
        .await
    {
        Ok(Some(e)) => e,
        Ok(None) => {
            return JsonRpcResponse::error(
                req.id,
                INVALID_PARAMS,
                format!("Note not found: {}", params.node_id),
            )
        }
        Err(e) => {
            return JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("NoteStore error: {e}"))
        }
    };

    // Read the markdown file from disk using the full path (includes category subdirectory).
    let agent_id = crate::routing::DEFAULT_AGENT_ID; // TODO: derive from request when multi-agent is wired
    let md_path = notes_dir()
        .join(agent_id)
        .join(format!("{}.md", entry.path));
    let content = tokio::fs::read_to_string(&md_path)
        .await
        .unwrap_or_default();

    // Fetch backlinks (incoming links).
    let backlinks = db
        .get_incoming_links(&params.node_id, crate::routing::DEFAULT_AGENT_ID)
        .await
        .unwrap_or_default();
```

with:

```rust
    let agent_id = params
        .agent_id
        .as_deref()
        .unwrap_or(crate::routing::DEFAULT_AGENT_ID);

    // Fetch the note index entry.
    let entry = match db.get_note_index(&params.node_id, agent_id).await {
        Ok(Some(e)) => e,
        Ok(None) => {
            return JsonRpcResponse::error(
                req.id,
                INVALID_PARAMS,
                format!("Note not found: {}", params.node_id),
            )
        }
        Err(e) => {
            return JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("NoteStore error: {e}"))
        }
    };

    // Read the markdown file from disk using the full path (includes category subdirectory).
    let md_path = notes_dir()
        .join(agent_id)
        .join(format!("{}.md", entry.path));
    let content = tokio::fs::read_to_string(&md_path)
        .await
        .unwrap_or_default();

    // Fetch backlinks (incoming links).
    let backlinks = db
        .get_incoming_links(&params.node_id, agent_id)
        .await
        .unwrap_or_default();
```

Note: this removes the `// TODO: derive from request when multi-agent is wired` comment — the TODO is now resolved.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p alephcore --lib graph_node_detail_ -- --nocapture`
Expected: both PASS.

- [ ] **Step 5: Commit**

```bash
git add src/gateway/handlers/graph.rs
git commit -m "graph: thread agent_id through handle_node_detail_impl"
```

---

## Task 5: TDD — thread `agent_id` through `handle_search_impl`

**Files:**
- Modify: `src/gateway/handlers/graph.rs:317-323` (handler call)
- Modify: `src/gateway/handlers/graph.rs` test module

- [ ] **Step 1: Write the two failing tests**

Append inside the test module:

```rust
    fn search_request(query: &str, limit: usize, agent_id: Option<&str>) -> JsonRpcRequest {
        let mut params = serde_json::json!({ "query": query, "limit": limit });
        if let Some(id) = agent_id {
            params["agent_id"] = serde_json::Value::String(id.to_string());
        }
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "graph.search".to_string(),
            params: Some(params),
            id: Some(serde_json::json!(1)),
        }
    }

    #[tokio::test]
    async fn graph_search_uses_explicit_agent_id() {
        let db = make_db();
        let alpha = make_note("AlphaUnique", "concept", vec![]);
        let beta = make_note("BetaUnique", "concept", vec![]);
        db.index_note(&alpha, "alpha", "concept").await.unwrap();
        db.index_note(&beta, "beta", "concept").await.unwrap();

        let req = search_request("Unique", 20, Some("alpha"));
        let resp = handle_search_impl(req, db).await;
        assert!(resp.error.is_none(), "expected success: {:?}", resp.error);
        let result: GraphSearchResponse =
            serde_json::from_value(resp.result.expect("result")).expect("deserialize");

        let names: Vec<&str> = result.results.iter().map(|r| r.name.as_str()).collect();
        assert!(names.iter().any(|n| n.contains("AlphaUnique")),
            "alpha hit must appear: {names:?}");
        assert!(!names.iter().any(|n| n.contains("BetaUnique")),
            "beta hit must NOT appear: {names:?}");
    }

    #[tokio::test]
    async fn graph_search_falls_back_to_default_agent_when_omitted() {
        let db = make_db();
        let main = make_note("MainSearchTarget", "concept", vec![]);
        db.index_note(&main, crate::routing::DEFAULT_AGENT_ID, "concept")
            .await
            .unwrap();

        let req = search_request("MainSearchTarget", 20, None);
        let resp = handle_search_impl(req, db).await;
        assert!(resp.error.is_none(), "expected success: {:?}", resp.error);
        let result: GraphSearchResponse =
            serde_json::from_value(resp.result.expect("result")).expect("deserialize");
        assert!(!result.results.is_empty(),
            "default agent's note must be findable when agent_id omitted");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alephcore --lib graph_search_ -- --nocapture`
Expected: `graph_search_uses_explicit_agent_id` FAILS — `alpha hit must appear` because the handler queries `DEFAULT_AGENT_ID` and finds nothing.

- [ ] **Step 3: Update `handle_search_impl`**

In `src/gateway/handlers/graph.rs`, replace lines 317-323:

```rust
    let entries = match db
        .search_notes_fts(
            &params.query,
            crate::routing::DEFAULT_AGENT_ID,
            params.limit,
        )
        .await
    {
```

with:

```rust
    let agent_id = params
        .agent_id
        .as_deref()
        .unwrap_or(crate::routing::DEFAULT_AGENT_ID);
    let entries = match db
        .search_notes_fts(&params.query, agent_id, params.limit)
        .await
    {
```

- [ ] **Step 4: Run all handler tests to verify everything passes**

Run: `cargo test -p alephcore --lib gateway::handlers::graph::tests`
Expected: ALL tests PASS — 4 pre-existing + 8 new (2 per handler × 4 handlers).

- [ ] **Step 5: Run clippy on the changed file**

Run: `cargo clippy -p alephcore --lib -- -D warnings 2>&1 | grep -E 'graph\.rs|graph_types\.rs' || echo 'no warnings in changed files'`
Expected: `no warnings in changed files`.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/handlers/graph.rs
git commit -m "graph: thread agent_id through handle_search_impl"
```

---

## Task 6: Frontend — add `agent_id` parameter to `GraphApi`

**Files:**
- Modify: `interfaces/webchat/src/api/graph.rs:1-49` (all four methods)

This is plumbing only — call sites are updated in Task 8. After this task the frontend will not compile until Task 8 lands; that's fine because the four files are committed together as a logical unit. Alternatively split: ship Task 6 + Task 8 in one commit.

- [ ] **Step 1: Update `GraphApi::query` signature and JSON params**

Replace `query` (lines 8-16):

```rust
    pub async fn query(
        state: &DashboardState,
        limit: usize,
        kind_filter: Vec<String>,
    ) -> Result<GraphQueryResponse, String> {
        let params = json!({ "limit": limit, "kind_filter": kind_filter });
        let result = state.rpc_call("graph.query", params).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse graph.query: {}", e))
    }
```

with:

```rust
    pub async fn query(
        state: &DashboardState,
        agent_id: &str,
        limit: usize,
        kind_filter: Vec<String>,
    ) -> Result<GraphQueryResponse, String> {
        let params = json!({
            "agent_id": agent_id,
            "limit": limit,
            "kind_filter": kind_filter,
        });
        let result = state.rpc_call("graph.query", params).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse graph.query: {}", e))
    }
```

- [ ] **Step 2: Update `GraphApi::neighbors`**

Replace `neighbors` (lines 18-28):

```rust
    pub async fn neighbors(
        state: &DashboardState,
        node_id: &str,
        depth: u8,
        limit: usize,
    ) -> Result<GraphNeighborsResponse, String> {
        let params = json!({ "node_id": node_id, "depth": depth, "limit": limit });
        let result = state.rpc_call("graph.neighbors", params).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse graph.neighbors: {}", e))
    }
```

with:

```rust
    pub async fn neighbors(
        state: &DashboardState,
        agent_id: &str,
        node_id: &str,
        depth: u8,
        limit: usize,
    ) -> Result<GraphNeighborsResponse, String> {
        let params = json!({
            "agent_id": agent_id,
            "node_id": node_id,
            "depth": depth,
            "limit": limit,
        });
        let result = state.rpc_call("graph.neighbors", params).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse graph.neighbors: {}", e))
    }
```

- [ ] **Step 3: Update `GraphApi::node_detail`**

Replace `node_detail` (lines 30-38):

```rust
    pub async fn node_detail(
        state: &DashboardState,
        node_id: &str,
    ) -> Result<NoteDetailResponse, String> {
        let params = json!({ "node_id": node_id });
        let result = state.rpc_call("graph.node_detail", params).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse graph.node_detail: {}", e))
    }
```

with:

```rust
    pub async fn node_detail(
        state: &DashboardState,
        agent_id: &str,
        node_id: &str,
    ) -> Result<NoteDetailResponse, String> {
        let params = json!({ "agent_id": agent_id, "node_id": node_id });
        let result = state.rpc_call("graph.node_detail", params).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse graph.node_detail: {}", e))
    }
```

- [ ] **Step 4: Update `GraphApi::search`**

Replace `search` (lines 40-48):

```rust
    pub async fn search(
        state: &DashboardState,
        query: &str,
        limit: usize,
    ) -> Result<GraphSearchResponse, String> {
        let params = json!({ "query": query, "limit": limit });
        let result = state.rpc_call("graph.search", params).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse graph.search: {}", e))
    }
```

with:

```rust
    pub async fn search(
        state: &DashboardState,
        agent_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<GraphSearchResponse, String> {
        let params = json!({ "agent_id": agent_id, "query": query, "limit": limit });
        let result = state.rpc_call("graph.search", params).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse graph.search: {}", e))
    }
```

- [ ] **Step 5: Do not commit yet — Task 7 + Task 8 update the call sites and we land them together**

The webchat crate will not compile in isolation after this task. Continue to Task 7 immediately.

---

## Task 7: Frontend — `AgentSelectorBar` component

**Files:**
- Create: `interfaces/webchat/src/views/canvas/agent_selector.rs`
- Modify: `interfaces/webchat/src/views/canvas/mod.rs:1-7` (add `mod agent_selector;`)

- [ ] **Step 1: Create the new file with the pure formatter and a unit test**

Create `interfaces/webchat/src/views/canvas/agent_selector.rs`:

```rust
use crate::api::agents::AgentSummary;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

/// Format one dropdown option label.
///
/// Rules:
/// - emoji + name + " (id)" when both present, e.g. "🤖 Main (main)"
/// - id-only fallback when name is missing, e.g. "worker-3 (worker-3)"
/// - no leading emoji when emoji is missing
/// - default agent gets a trailing " ★"
pub fn format_agent_option(agent: &AgentSummary, is_default: bool) -> String {
    let name = agent.name.as_deref().filter(|s| !s.is_empty()).unwrap_or(&agent.id);
    let star = if is_default { " ★" } else { "" };
    match agent.emoji.as_deref().filter(|s| !s.is_empty()) {
        Some(emoji) => format!("{emoji} {name} ({}){star}", agent.id),
        None => format!("{name} ({}){star}", agent.id),
    }
}

#[component]
pub fn AgentSelectorBar(
    agent_id: RwSignal<String>,
    agents: RwSignal<Vec<AgentSummary>>,
    default_agent_id: RwSignal<String>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    on_refresh: impl Fn() + 'static + Copy,
) -> impl IntoView {
    let on_change = move |ev: web_sys::Event| {
        let target: Option<web_sys::HtmlSelectElement> = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok());
        if let Some(select) = target {
            agent_id.set(select.value());
        }
    };

    let on_refresh_click = move |_| on_refresh();

    view! {
        <div class="flex items-center gap-3 px-4 py-2 bg-surface-raised border-b border-border">
            <div class="text-sm font-medium text-text-secondary">"Agent"</div>

            {move || {
                if loading.get() {
                    view! {
                        <span class="text-sm text-text-tertiary">"Loading agents…"</span>
                    }.into_any()
                } else if let Some(msg) = error.get() {
                    view! {
                        <span class="text-sm text-error">
                            {format!("Failed to load agents: {msg} · ")}
                            <button class="underline" on:click=on_refresh_click>"Retry"</button>
                        </span>
                    }.into_any()
                } else {
                    view! {
                        <select
                            class="px-3 py-1.5 text-sm bg-surface-sunken border border-border rounded-lg
                                   text-text-primary focus:outline-none focus:border-primary/50"
                            on:change=on_change
                            prop:value=move || agent_id.get()
                        >
                            {move || {
                                let current = agent_id.get();
                                let default = default_agent_id.get();
                                agents.get().iter().map(|a| {
                                    let label = format_agent_option(a, a.id == default);
                                    let id = a.id.clone();
                                    let selected = id == current;
                                    view! {
                                        <option value=id.clone() selected=selected>{label}</option>
                                    }
                                }).collect_view()
                            }}
                        </select>
                    }.into_any()
                }
            }}

            <button
                class="px-2 py-1 text-sm text-text-secondary hover:text-text-primary"
                title="Refresh agent list"
                on:click=on_refresh_click
            >
                "↻"
            </button>

            <div class="flex-1" />
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::agents::AgentSummary;

    fn agent(id: &str, name: Option<&str>, emoji: Option<&str>) -> AgentSummary {
        AgentSummary {
            id: id.to_string(),
            name: name.map(String::from),
            emoji: emoji.map(String::from),
            description: None,
            model: None,
            is_default: false,
        }
    }

    #[test]
    fn formats_emoji_name_id_when_all_present() {
        let a = agent("main", Some("Main"), Some("🤖"));
        assert_eq!(format_agent_option(&a, false), "🤖 Main (main)");
    }

    #[test]
    fn falls_back_to_id_when_name_missing() {
        let a = agent("worker-3", None, None);
        assert_eq!(format_agent_option(&a, false), "worker-3 (worker-3)");
    }

    #[test]
    fn omits_emoji_prefix_when_emoji_missing() {
        let a = agent("fox", Some("fox-research"), None);
        assert_eq!(format_agent_option(&a, false), "fox-research (fox)");
    }

    #[test]
    fn appends_star_for_default_agent() {
        let a = agent("main", Some("Main"), Some("🤖"));
        assert_eq!(format_agent_option(&a, true), "🤖 Main (main) ★");
    }

    #[test]
    fn empty_string_emoji_is_treated_as_missing() {
        let a = agent("main", Some("Main"), Some(""));
        assert_eq!(format_agent_option(&a, false), "Main (main)");
    }

    #[test]
    fn empty_string_name_is_treated_as_missing() {
        let a = agent("main", Some(""), None);
        assert_eq!(format_agent_option(&a, false), "main (main)");
    }
}
```

- [ ] **Step 2: Register the submodule in `mod.rs`**

In `interfaces/webchat/src/views/canvas/mod.rs`, replace lines 1-6:

```rust
mod breadcrumb;
mod detail_panel;
mod graph_canvas;
#[cfg(target_arch = "wasm32")]
mod minimap_view;
mod toolbar;
```

with:

```rust
mod agent_selector;
mod breadcrumb;
mod detail_panel;
mod graph_canvas;
#[cfg(target_arch = "wasm32")]
mod minimap_view;
mod toolbar;
```

- [ ] **Step 3: Run the formatter unit tests**

Run: `cargo test -p alephwebchat --lib views::canvas::agent_selector::tests`

(If the webchat crate name is different, find it: `grep '^name' interfaces/webchat/Cargo.toml`. Adjust the `-p` flag accordingly.)

Expected: 6 tests PASS.

- [ ] **Step 4: Do not commit yet — Task 8 wires this into the view tree**

Continue to Task 8.

---

## Task 8: Frontend — wire agent state and selector into `RadialCanvasView`

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/mod.rs` (multiple regions)

This is the biggest task. It (a) introduces the agent signals, (b) fetches agents on mount, (c) inserts `AgentSelectorBar` into the view tree, (d) threads `agent_id` into all six `GraphApi` call sites, and (e) adds the reset Effect.

- [ ] **Step 1: Add the `agent_selector` import next to existing imports**

In `interfaces/webchat/src/views/canvas/mod.rs`, in the import block around lines 32-35, after `use toolbar::CanvasToolbar;` add:

```rust
use crate::api::agents::{AgentSummary, AgentsApi};
use agent_selector::AgentSelectorBar;
```

- [ ] **Step 2: Add the agent signals at the top of `RadialCanvasView`**

In `RadialCanvasView` body, immediately after the `expect_context` line (currently line 51), insert:

```rust
    // -----------------------------------------------------------------------
    // Agent selection signals
    // -----------------------------------------------------------------------
    // Placeholder is the literal "main" — must match server's DEFAULT_AGENT_ID.
    // If they ever diverge, the worst case is one extra graph fetch on mount,
    // because AgentsApi::list().default_id overrides this once it resolves.
    let agent_id = RwSignal::new("main".to_string());
    let agents = RwSignal::new(Vec::<AgentSummary>::new());
    let default_agent_id = RwSignal::new("main".to_string());
    let agents_loading = RwSignal::new(false);
    let agents_error = RwSignal::new(None::<String>);

    // Fetch the agent list once on mount, plus a reusable refresh callback.
    let fetch_agents = {
        let state = state.clone();
        move || {
            let state = state.clone();
            agents_loading.set(true);
            agents_error.set(None);
            spawn_local(async move {
                match AgentsApi::list(&state).await {
                    Ok(resp) => {
                        agents.set(resp.agents);
                        let new_default = resp.default_id;
                        default_agent_id.set(new_default.clone());
                        // Only override agent_id if it would actually change.
                        if agent_id.get_untracked() != new_default {
                            agent_id.set(new_default);
                        }
                        agents_loading.set(false);
                    }
                    Err(e) => {
                        agents_error.set(Some(e));
                        agents_loading.set(false);
                    }
                }
            });
        }
    };

    // Initial fetch
    {
        let fetch = fetch_agents.clone();
        Effect::new(move || {
            fetch();
        });
    }
```

(The two-step pattern — define `fetch_agents` then call it inside an `Effect::new` — is needed so we can also pass `fetch_agents` to the selector bar's refresh button.)

- [ ] **Step 3: Update Effect 1 (initial mount) to subscribe to and use `agent_id`**

In `RadialCanvasView`, find the `Effect::new(move || {` block around line 98 (the one that calls `GraphApi::query` and then `GraphApi::neighbors` for the entry pick). Make two changes inside the closure:

a) After the `if !state.is_connected.get() { return; }` line, add a line that subscribes to `agent_id` so the Effect re-runs when it changes:

```rust
        let agent = agent_id.get();
```

b) Pass `&agent` to the two `GraphApi` calls. Replace:

```rust
            let query_result = GraphApi::query(&state, 500, vec![]).await.ok();
```

with:

```rust
            let query_result = GraphApi::query(&state, &agent, 500, vec![]).await.ok();
```

And replace:

```rust
            match GraphApi::neighbors(&state, &entry_id, 3, 200).await {
```

with:

```rust
            match GraphApi::neighbors(&state, &agent, &entry_id, 3, 200).await {
```

(Both `agent` and `entry_id` are owned `String`s; `&agent` and `&entry_id` are valid `&str`.)

- [ ] **Step 4: Update Effect-fetch (Effect 2) to subscribe to and use `agent_id`**

Find the `Effect::new(move || {` block around line 172 (the one starting with `let Some(id) = active_request.get() else { return };`). Inside, after `let now_ms = now_ms();`, add:

```rust
        let agent = agent_id.get();
```

Then update the `GraphApi::neighbors` call inside the `spawn_local`. Replace:

```rust
            match GraphApi::neighbors(&state, &id, 3, 200).await {
```

with:

```rust
            match GraphApi::neighbors(&state, &agent, &id, 3, 200).await {
```

(Capture `agent` into the spawn_local closure — it's already in scope; the move closure will own it.)

- [ ] **Step 5: Update Effect 3 (node detail) to subscribe to and use `agent_id`**

Find the `Effect::new(move || {` block around line 240 (the one that handles `selected_node.get()`). Inside, replace:

```rust
            Some(id) => {
                spawn_local(async move {
                    match GraphApi::node_detail(&state, &id).await {
```

with:

```rust
            Some(id) => {
                let agent = agent_id.get();
                spawn_local(async move {
                    match GraphApi::node_detail(&state, &agent, &id).await {
```

- [ ] **Step 6: Update Effect 4 (hover prefetch) to subscribe to and use `agent_id`**

Find the `Effect::new(move || {` block around line 272 (the one starting with `let Some(id) = prefetch_request.get() else { return };`). Inside, after `if prefetch_e4.borrow().has(&id, now) { return; }`, add:

```rust
        let agent = agent_id.get();
```

Then update the `GraphApi::neighbors` call inside the `spawn_local`. Replace:

```rust
            match GraphApi::neighbors(&state, &id, 3, 200).await {
```

with:

```rust
            match GraphApi::neighbors(&state, &agent, &id, 3, 200).await {
```

- [ ] **Step 7: Update `on_search` closure to pass `agent_id`**

Find the `let on_search = move |query: String| {` block around line 367. Replace:

```rust
        spawn_local(async move {
            match GraphApi::search(&state, &query, 20).await {
```

with:

```rust
        let agent = agent_id.get();
        spawn_local(async move {
            match GraphApi::search(&state, &agent, &query, 20).await {
```

- [ ] **Step 8: Insert `AgentSelectorBar` above `CanvasToolbar` in the view tree**

Find the `view! { <div class="flex flex-col h-full">` block around line 412. Replace:

```rust
        <div class="flex flex-col h-full">
            <CanvasToolbar
                search_query=search_query
                on_search=on_search
                fold_threshold=fold_threshold
                set_fold_threshold=set_fold_threshold
                visible_counts=visible_counts
            />
```

with:

```rust
        <div class="flex flex-col h-full">
            <AgentSelectorBar
                agent_id=agent_id
                agents=agents
                default_agent_id=default_agent_id
                loading=agents_loading
                error=agents_error
                on_refresh=fetch_agents
            />

            <CanvasToolbar
                search_query=search_query
                on_search=on_search
                fold_threshold=fold_threshold
                set_fold_threshold=set_fold_threshold
                visible_counts=visible_counts
            />
```

- [ ] **Step 9: Verify webchat crate compiles**

Run: `cargo check -p alephwebchat --target wasm32-unknown-unknown 2>&1 | tail -30`

(If the build target isn't wasm32, the canvas module is conditional. Try `just dev` or whatever command rebuilds the WASM in this repo. Check `justfile` first: `cat justfile | head -40`.)

Expected: clean check.

- [ ] **Step 10: Commit Tasks 6–8 together**

```bash
git add interfaces/webchat/src/api/graph.rs \
        interfaces/webchat/src/views/canvas/agent_selector.rs \
        interfaces/webchat/src/views/canvas/mod.rs
git commit -m "canvas: wire agent_id through GraphApi and add AgentSelectorBar"
```

---

## Task 9: Frontend — agent-switch reset Effect

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/mod.rs` (add one Effect)

The user's design requires that switching the agent fully resets the canvas: nodes, breadcrumb, search query, fold threshold, zoom, pan, drag — everything. The four existing graph-fetch Effects already subscribe to `agent_id` (Task 8 steps 3-7), so refetch happens automatically. This Effect is the reset side.

- [ ] **Step 1: Add the reset Effect after the `fetch_agents` initialisation**

In `RadialCanvasView`, after the initial-fetch Effect added in Task 8 step 2 and before `Effect 1`, insert:

```rust
    // -----------------------------------------------------------------------
    // Agent-switch reset Effect.
    // Subscribes to `agent_id`; on a real change (prev != current), wipes all
    // canvas view state so the new agent's graph renders from a clean slate.
    // The four graph-fetch Effects also subscribe to `agent_id` and re-fire
    // automatically — this Effect's only job is the reset.
    //
    // The closure returns the current `agent_id`, so the next invocation sees
    // it as `prev`. On first mount `prev == None` and the reset body is skipped
    // (avoids clearing empty state before Effect 1's initial fetch).
    // -----------------------------------------------------------------------
    let nav_reset = nav.clone();
    let gs_reset = graph_state.clone();
    let prefetch_reset = prefetch.clone();
    Effect::new(move |prev: Option<String>| {
        let current = agent_id.get();
        if let Some(p) = prev.as_ref() {
            if *p != current {
                // Reset reactive signals
                set_selected_node.set(None);
                set_node_detail.set(None);
                set_detail_content.set(DetailContent::Closed);
                set_breadcrumb.set(Vec::new());
                search_query.set(String::new());
                set_fold_threshold.set(12);
                set_focus_id.set(None);
                set_focus_neighbors.set(Vec::new());
                set_visible_counts.set((0, 0));
                last_response.set(None);
                prefetch_request.set(None);
                all_dtos.set(Vec::new());

                // Reset non-reactive state
                *nav_reset.borrow_mut() = NavController::new();
                *prefetch_reset.borrow_mut() = PrefetchCache::new();
                {
                    let mut gs = gs_reset.borrow_mut();
                    gs.nodes.clear();
                    gs.edges.clear();
                    gs.selected_node = None;
                    gs.viewport.offset.x = gs.viewport.width / 2.0;
                    gs.viewport.offset.y = gs.viewport.height / 2.0;
                    gs.viewport.scale = 1.0;
                    gs.drag_offset = (0.0, 0.0);
                }

                // Force the four graph-fetch Effects to re-fire by clearing
                // active_request first, then setting it to None for a clean slate.
                // (Effect 1 will pick a new entry node from the new agent's data.)
                active_request.set(None);
            }
        }
        current
    });
```

Note: `12` matches the existing `fold_threshold` initial value at line 59. If Task 7 in this codebase introduces a `DEFAULT_FOLD_THRESHOLD` constant, use it instead.

- [ ] **Step 2: Verify the webchat compiles**

Run the same check command from Task 8 step 9.

Expected: clean check. If clippy complains about `fold_threshold` being captured both as `set_fold_threshold` (writer) and `fold_threshold` (reader) inside the closure, that's fine — Leptos signals are designed for this.

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/views/canvas/mod.rs
git commit -m "canvas: reset all view state when agent_id changes"
```

---

## Task 10: Manual smoke verification

**Files:** None (manual test)

The spec section 7 prescribes Leptos component-level integration tests. The webchat has no test harness for that, and standing one up is out of scope. The smoke checklist below substitutes.

- [ ] **Step 1: Build a fresh release binary and restart the server**

Per `CLAUDE.md`'s process-management rule:

```bash
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
sleep 2
ps aux | grep "[a]leph-server" | grep -v zsh | grep -v cp | grep -v tail
# (should print nothing)

just build
target/release/aleph-server start
```

- [ ] **Step 2: Open the dashboard canvas in a browser and verify:**

1. The `AgentSelectorBar` appears above the toolbar.
2. The dropdown is initially in `Loading agents…` state, then populates with all agents.
3. The default agent's option ends with ` ★`.
4. Switching to a non-default agent:
   - clears the current graph immediately,
   - re-fetches and re-renders for the new agent,
   - resets the breadcrumb, search box, and fold-threshold slider to defaults.
5. The `↻` button refreshes the agent list without changing the current selection (unless the current agent was deleted).
6. Searching for a known node from the new agent navigates to it correctly (proves search RPC threads `agent_id`).
7. Clicking a node shows its detail panel with the correct backlinks (proves `node_detail` RPC threads `agent_id`).

- [ ] **Step 3: Sanity-check via DevTools network panel**

Open the browser DevTools → Network tab → filter for `graph.`. Confirm the JSON params for `graph.query`, `graph.neighbors`, `graph.node_detail`, and `graph.search` all include an `"agent_id"` field with the currently selected agent.

- [ ] **Step 4: Final commit if any documentation updates emerged**

If the smoke test surfaced an inaccuracy in the spec, update the spec and commit:

```bash
git add docs/superpowers/specs/2026-04-27-canvas-agent-selector-design.md
git commit -m "spec(canvas-agent-selector): correct <whatever>"
```

Otherwise no commit — the feature is complete.

---

## Self-Review Notes (recorded during plan authoring)

**Spec coverage:**
- §5.1 backend params change → Task 1
- §5.1 backend handler threading + 8 unit tests → Tasks 2-5
- §5.2 AgentSelectorBar component + format rules → Task 7
- §5.3 RadialCanvasView signal layout + mount fetch → Task 8 step 2
- §5.3 AgentSelectorBar insertion above CanvasToolbar → Task 8 step 8
- §5.4 reset Effect → Task 9
- §5.5 GraphApi signature change → Task 6 + Task 8 steps 3-7
- §6 error handling → built into AgentSelectorBar's loading/error branches (Task 7) and reset Effect's clean-slate behavior (Task 9)
- §7.1 backend tests → 8 tests across Tasks 2-5
- §7.2 frontend tests — substituted with pure-fn tests on `format_agent_option` (6 tests in Task 7) plus the manual smoke checklist in Task 10. Documented in the "Spec corrections" section above.
- §7.3 integration test — substituted with manual smoke (Task 10 step 2 case 4). Documented above.
- §8 sequencing → Tasks 1-5 form Phase 1 (backend, independently shippable); Tasks 6-9 form Phase 2 (frontend). Task 10 is verification.

**Type/name consistency check:** `agent_id` (snake_case) used everywhere. `AgentSummary`, `AgentsApi::list`, `AgentsListResponse` match the existing types in `interfaces/webchat/src/api/agents.rs`. The reset Effect uses the exact signal names defined in `RadialCanvasView` today (verified by inspection of `mod.rs:51-89`).

**Placeholder scan:** No "TBD"/"TODO"/"implement later" remain in this plan. Every code block is the actual code to write.
