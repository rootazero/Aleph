# Canvas Knowledge Graph Visualization — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an interactive knowledge graph visualization panel to Aleph's web UI, with agent-isolated graphs, force-directed layout, and wiki/facts detail panel.

**Architecture:** Server exposes 4 new JSON-RPC methods (`graph.query`, `graph.neighbors`, `graph.node_detail`, `graph.search`) that query the existing GraphStore/MemoryStore with agent filtering. Frontend implements a Canvas 2D rendering engine in Leptos/WASM with force-directed layout, zoom/pan, and a right-side detail panel. Canvas is a new top-level panel tab.

**Tech Stack:** Rust, Leptos 0.8 (CSR), web_sys Canvas 2D API, wasm-bindgen, serde_json, existing SQLite GraphStore/MemoryStore.

**Spec:** `docs/superpowers/specs/2026-04-11-canvas-knowledge-graph-design.md`

---

## File Map

### Server-side (new files)

| File | Responsibility |
|------|---------------|
| `src/gateway/handlers/graph.rs` | JSON-RPC handlers for graph.query, graph.neighbors, graph.node_detail, graph.search |
| `src/gateway/handlers/graph_types.rs` | Request/response serde types for graph API |

### Server-side (modified files)

| File | Change |
|------|--------|
| `src/gateway/handlers/mod.rs` | Register graph.* methods in HandlerRegistry |

### Frontend — Canvas Engine (new files in `interfaces/webchat/src/canvas_engine/`)

| File | Responsibility |
|------|---------------|
| `mod.rs` | Public API re-exports |
| `types.rs` | CanvasNode, CanvasEdge, ViewMode, ViewState, Color |
| `adapter.rs` | Server response → CanvasNode/CanvasEdge conversion |
| `viewport.rs` | Zoom, pan, world↔screen coordinate transform, hit testing |
| `layout.rs` | Force-directed layout with Barnes-Hut approximation |
| `renderer.rs` | Canvas 2D draw calls (nodes, edges, labels, tooltip, selection) |
| `interaction.rs` | Mouse/wheel/touch event dispatch → state mutations |

### Frontend — Canvas View (new files in `interfaces/webchat/src/views/canvas/`)

| File | Responsibility |
|------|---------------|
| `mod.rs` | CanvasView top-level component, wires engine + panels |
| `toolbar.rs` | Agent label, search box, mode toggle, filter popover |
| `graph_canvas.rs` | `<canvas>` element, render loop, event binding |
| `detail_panel.rs` | Right-side wiki markdown + facts list panel |
| `breadcrumb.rs` | Global → Node → Node breadcrumb navigation |

### Frontend — API + Integration (new/modified files)

| File | Change |
|------|--------|
| `interfaces/webchat/src/api/graph.rs` | New: GraphApi struct with RPC call methods |
| `interfaces/webchat/src/api/mod.rs` | Modified: add `pub mod graph;` |
| `interfaces/webchat/src/components/bottom_bar.rs` | Modified: add PanelMode::Canvas variant + tab |
| `interfaces/webchat/src/app.rs` | Modified: add Canvas panel routing in MainContent |
| `interfaces/webchat/src/views/mod.rs` | Modified: add `pub mod canvas;` |
| `interfaces/webchat/Cargo.toml` | Modified: add web-sys canvas features |

---

## Task 1: Server-Side Graph API Types

**Files:**
- Create: `src/gateway/handlers/graph_types.rs`

- [ ] **Step 1: Create graph_types.rs with request/response structs**

```rust
// src/gateway/handlers/graph_types.rs
use serde::{Deserialize, Serialize};

// === graph.query ===

#[derive(Debug, Deserialize)]
pub struct GraphQueryParams {
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub kind_filter: Vec<String>,
}

fn default_limit() -> usize {
    100
}

#[derive(Debug, Serialize)]
pub struct GraphQueryResponse {
    pub nodes: Vec<GraphNodeDto>,
    pub edges: Vec<GraphEdgeDto>,
}

#[derive(Debug, Serialize)]
pub struct GraphNodeDto {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub aliases: Vec<String>,
    pub decay_score: f32,
    pub edge_count: usize,
    pub has_wiki: bool,
}

#[derive(Debug, Serialize)]
pub struct GraphEdgeDto {
    pub id: String,
    pub from_id: String,
    pub to_id: String,
    pub relation: String,
    pub weight: f32,
    pub confidence: f32,
}

// === graph.neighbors ===

#[derive(Debug, Deserialize)]
pub struct GraphNeighborsParams {
    pub node_id: String,
    #[serde(default = "default_depth")]
    pub depth: u8,
    #[serde(default = "default_neighbor_limit")]
    pub limit: usize,
}

fn default_depth() -> u8 {
    2
}

fn default_neighbor_limit() -> usize {
    50
}

// Response reuses GraphQueryResponse

// === graph.node_detail ===

#[derive(Debug, Deserialize)]
pub struct GraphNodeDetailParams {
    pub node_id: String,
}

#[derive(Debug, Serialize)]
pub struct GraphNodeDetailResponse {
    pub node: GraphNodeDto,
    pub wiki: Option<WikiDto>,
    pub facts: Vec<FactDto>,
}

#[derive(Debug, Serialize)]
pub struct WikiDto {
    pub id: String,
    pub content: String,
    pub fact_source: String,
    pub updated_at: i64,
}

#[derive(Debug, Serialize)]
pub struct FactDto {
    pub id: String,
    pub content: String,
    pub confidence: f32,
    pub fact_type: String,
}

// === graph.search ===

#[derive(Debug, Deserialize)]
pub struct GraphSearchParams {
    pub query: String,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
}

fn default_search_limit() -> usize {
    10
}

#[derive(Debug, Serialize)]
pub struct GraphSearchResponse {
    pub results: Vec<GraphSearchResult>,
}

#[derive(Debug, Serialize)]
pub struct GraphSearchResult {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub match_field: String,
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p alephcore`
Expected: PASS (types module not yet wired, but syntax should be valid)

- [ ] **Step 3: Commit**

```bash
git add src/gateway/handlers/graph_types.rs
git commit -m "gateway: add graph API request/response types"
```

---

## Task 2: Server-Side graph.query Handler

**Files:**
- Create: `src/gateway/handlers/graph.rs`
- Modify: `src/gateway/handlers/mod.rs`

- [ ] **Step 1: Create graph.rs with handle_query function**

The handler needs access to the MemoryBackend (which wraps GraphStore + MemoryStore). Follow the pattern from `memory.rs` handlers:

```rust
// src/gateway/handlers/graph.rs
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::memory::store::MemoryBackend;
use serde_json::json;

use super::graph_types::*;

pub async fn handle_query(
    request: JsonRpcRequest,
    db: MemoryBackend,
    agent_id: String,
) -> JsonRpcResponse {
    let params: GraphQueryParams = match request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
    {
        Some(p) => p,
        None => GraphQueryParams {
            limit: 100,
            kind_filter: vec![],
        },
    };

    let workspace = &agent_id;

    // Fetch all nodes for this agent, sorted by decay_score descending
    let all_nodes = match db.graph_store().get_all_nodes(workspace).await {
        Ok(nodes) => nodes,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to fetch graph nodes: {}", e),
            );
        }
    };

    // Filter by kind if specified
    let mut filtered: Vec<_> = if params.kind_filter.is_empty() {
        all_nodes
    } else {
        all_nodes
            .into_iter()
            .filter(|n| params.kind_filter.contains(&n.kind))
            .collect()
    };

    // Sort by weight (decay_score) descending, take top-K
    filtered.sort_by(|a, b| b.decay_score.partial_cmp(&a.decay_score).unwrap_or(std::cmp::Ordering::Equal));
    filtered.truncate(params.limit);

    let node_ids: Vec<&str> = filtered.iter().map(|n| n.id.as_str()).collect();

    // Fetch edges between the selected nodes
    let mut edges = Vec::new();
    for node in &filtered {
        if let Ok(node_edges) = db.graph_store().get_edges_for_node(&node.id, workspace).await {
            for edge in node_edges {
                // Only include edges where both endpoints are in our node set
                if node_ids.contains(&edge.from_id.as_str())
                    && node_ids.contains(&edge.to_id.as_str())
                {
                    edges.push(edge);
                }
            }
        }
    }

    // Deduplicate edges by id
    edges.sort_by(|a, b| a.id.cmp(&b.id));
    edges.dedup_by(|a, b| a.id == b.id);

    // Count edges per node and check wiki existence
    let mut edge_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for edge in &edges {
        *edge_counts.entry(edge.from_id.clone()).or_default() += 1;
        *edge_counts.entry(edge.to_id.clone()).or_default() += 1;
    }

    let mut node_dtos = Vec::with_capacity(filtered.len());
    for node in &filtered {
        let facts = db
            .graph_store()
            .get_facts_for_node(&node.id, workspace)
            .await
            .unwrap_or_default();

        let has_wiki = {
            let mut found = false;
            for (fact_id, _weight) in &facts {
                if let Ok(Some(fact)) = db.memory_store().get_fact(fact_id).await {
                    if fact.fact_type.as_str() == "wiki" {
                        found = true;
                        break;
                    }
                }
            }
            found
        };

        node_dtos.push(GraphNodeDto {
            id: node.id.clone(),
            name: node.name.clone(),
            kind: node.kind.clone(),
            aliases: node.aliases.clone(),
            decay_score: node.decay_score,
            edge_count: *edge_counts.get(&node.id).unwrap_or(&0),
            has_wiki,
        });
    }

    let edge_dtos: Vec<GraphEdgeDto> = edges
        .into_iter()
        .map(|e| GraphEdgeDto {
            id: e.id,
            from_id: e.from_id,
            to_id: e.to_id,
            relation: e.relation,
            weight: e.weight,
            confidence: e.confidence,
        })
        .collect();

    let response = GraphQueryResponse {
        nodes: node_dtos,
        edges: edge_dtos,
    };

    match serde_json::to_value(&response) {
        Ok(value) => JsonRpcResponse::success(request.id, value),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize response: {}", e),
        ),
    }
}
```

- [ ] **Step 2: Add graph module to mod.rs**

In `src/gateway/handlers/mod.rs`, add:
```rust
pub mod graph;
pub mod graph_types;
```

And register placeholder in `HandlerRegistry::new()`:
```rust
registry.register("graph.query", |req| async move {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        "graph.query requires MemoryBackend - wire at startup".to_string(),
    )
});
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

Note: The actual wiring of `handle_query` with the MemoryBackend happens at Gateway startup. This follows the same pattern as existing handlers (placeholder first, wire later). The exact wiring location depends on how the Gateway injects shared state — check `src/gateway/mod.rs` or `src/bin/aleph-server/commands/start/mod.rs` for the wiring point.

- [ ] **Step 4: Commit**

```bash
git add src/gateway/handlers/graph.rs src/gateway/handlers/graph_types.rs src/gateway/handlers/mod.rs
git commit -m "gateway: add graph.query handler for Top-K node retrieval"
```

---

## Task 3: Server-Side graph.neighbors Handler

**Files:**
- Modify: `src/gateway/handlers/graph.rs`

- [ ] **Step 1: Add handle_neighbors function to graph.rs**

```rust
pub async fn handle_neighbors(
    request: JsonRpcRequest,
    db: MemoryBackend,
    agent_id: String,
) -> JsonRpcResponse {
    let params: GraphNeighborsParams = match request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
    {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing required param: node_id".to_string(),
            );
        }
    };

    let workspace = &agent_id;

    // BFS to collect N-hop neighborhood
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut frontier = vec![params.node_id.clone()];
    visited.insert(params.node_id.clone());

    for _hop in 0..params.depth {
        let mut next_frontier = Vec::new();
        for node_id in &frontier {
            if let Ok(node_edges) = db.graph_store().get_edges_for_node(node_id, workspace).await {
                for edge in node_edges {
                    let neighbor = if edge.from_id == *node_id {
                        &edge.to_id
                    } else {
                        &edge.from_id
                    };
                    if visited.insert(neighbor.clone()) {
                        next_frontier.push(neighbor.clone());
                    }
                }
            }
        }
        frontier = next_frontier;
        if visited.len() >= params.limit {
            break;
        }
    }

    // Truncate to limit
    let node_ids: Vec<String> = visited.into_iter().take(params.limit).collect();

    // Fetch node details
    let mut nodes = Vec::new();
    for nid in &node_ids {
        if let Ok(Some(node)) = db.graph_store().get_node(nid, workspace).await {
            nodes.push(node);
        }
    }

    // Fetch edges between collected nodes
    let node_id_set: std::collections::HashSet<&str> =
        node_ids.iter().map(|s| s.as_str()).collect();
    let mut edges = Vec::new();
    for node in &nodes {
        if let Ok(node_edges) = db.graph_store().get_edges_for_node(&node.id, workspace).await {
            for edge in node_edges {
                if node_id_set.contains(edge.from_id.as_str())
                    && node_id_set.contains(edge.to_id.as_str())
                {
                    edges.push(edge);
                }
            }
        }
    }
    edges.sort_by(|a, b| a.id.cmp(&b.id));
    edges.dedup_by(|a, b| a.id == b.id);

    // Build DTOs (same as handle_query)
    let mut edge_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for edge in &edges {
        *edge_counts.entry(edge.from_id.clone()).or_default() += 1;
        *edge_counts.entry(edge.to_id.clone()).or_default() += 1;
    }

    let mut node_dtos = Vec::with_capacity(nodes.len());
    for node in &nodes {
        let facts = db
            .graph_store()
            .get_facts_for_node(&node.id, workspace)
            .await
            .unwrap_or_default();
        let has_wiki = {
            let mut found = false;
            for (fact_id, _) in &facts {
                if let Ok(Some(fact)) = db.memory_store().get_fact(fact_id).await {
                    if fact.fact_type.as_str() == "wiki" {
                        found = true;
                        break;
                    }
                }
            }
            found
        };
        node_dtos.push(GraphNodeDto {
            id: node.id.clone(),
            name: node.name.clone(),
            kind: node.kind.clone(),
            aliases: node.aliases.clone(),
            decay_score: node.decay_score,
            edge_count: *edge_counts.get(&node.id).unwrap_or(&0),
            has_wiki,
        });
    }

    let edge_dtos: Vec<GraphEdgeDto> = edges
        .into_iter()
        .map(|e| GraphEdgeDto {
            id: e.id,
            from_id: e.from_id,
            to_id: e.to_id,
            relation: e.relation,
            weight: e.weight,
            confidence: e.confidence,
        })
        .collect();

    let response = GraphQueryResponse {
        nodes: node_dtos,
        edges: edge_dtos,
    };

    match serde_json::to_value(&response) {
        Ok(value) => JsonRpcResponse::success(request.id, value),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize: {}", e),
        ),
    }
}
```

- [ ] **Step 2: Register placeholder in mod.rs**

```rust
registry.register("graph.neighbors", |req| async move {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        "graph.neighbors requires MemoryBackend - wire at startup".to_string(),
    )
});
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/gateway/handlers/graph.rs src/gateway/handlers/mod.rs
git commit -m "gateway: add graph.neighbors handler for N-hop neighborhood"
```

---

## Task 4: Server-Side graph.node_detail Handler

**Files:**
- Modify: `src/gateway/handlers/graph.rs`

- [ ] **Step 1: Add handle_node_detail function**

```rust
pub async fn handle_node_detail(
    request: JsonRpcRequest,
    db: MemoryBackend,
    agent_id: String,
) -> JsonRpcResponse {
    let params: GraphNodeDetailParams = match request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
    {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing required param: node_id".to_string(),
            );
        }
    };

    let workspace = &agent_id;

    // Fetch node
    let node = match db.graph_store().get_node(&params.node_id, workspace).await {
        Ok(Some(n)) => n,
        Ok(None) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Node not found: {}", params.node_id),
            );
        }
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to fetch node: {}", e),
            );
        }
    };

    // Fetch linked facts
    let fact_links = db
        .graph_store()
        .get_facts_for_node(&params.node_id, workspace)
        .await
        .unwrap_or_default();

    let mut wiki_dto: Option<WikiDto> = None;
    let mut fact_dtos: Vec<FactDto> = Vec::new();

    for (fact_id, _weight) in &fact_links {
        if let Ok(Some(fact)) = db.memory_store().get_fact(fact_id).await {
            if !fact.is_valid {
                continue;
            }
            if fact.fact_type.as_str() == "wiki" && wiki_dto.is_none() {
                wiki_dto = Some(WikiDto {
                    id: fact.id.clone(),
                    content: fact.content.clone(),
                    fact_source: fact.fact_source.as_str().to_string(),
                    updated_at: fact.updated_at,
                });
            } else {
                fact_dtos.push(FactDto {
                    id: fact.id.clone(),
                    content: fact.content.clone(),
                    confidence: fact.confidence,
                    fact_type: fact.fact_type.as_str().to_string(),
                });
            }
        }
    }

    // Sort facts by confidence descending
    fact_dtos.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));

    // Count edges for the node DTO
    let edge_count = db
        .graph_store()
        .get_edges_for_node(&node.id, workspace)
        .await
        .map(|e| e.len())
        .unwrap_or(0);

    let node_dto = GraphNodeDto {
        id: node.id,
        name: node.name,
        kind: node.kind,
        aliases: node.aliases,
        decay_score: node.decay_score,
        edge_count,
        has_wiki: wiki_dto.is_some(),
    };

    let response = GraphNodeDetailResponse {
        node: node_dto,
        wiki: wiki_dto,
        facts: fact_dtos,
    };

    match serde_json::to_value(&response) {
        Ok(value) => JsonRpcResponse::success(request.id, value),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize: {}", e),
        ),
    }
}
```

- [ ] **Step 2: Register placeholder in mod.rs**

```rust
registry.register("graph.node_detail", |req| async move {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        "graph.node_detail requires MemoryBackend - wire at startup".to_string(),
    )
});
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/gateway/handlers/graph.rs src/gateway/handlers/mod.rs
git commit -m "gateway: add graph.node_detail handler for wiki + facts"
```

---

## Task 5: Server-Side graph.search Handler

**Files:**
- Modify: `src/gateway/handlers/graph.rs`

- [ ] **Step 1: Add handle_search function**

```rust
pub async fn handle_search(
    request: JsonRpcRequest,
    db: MemoryBackend,
    agent_id: String,
) -> JsonRpcResponse {
    let params: GraphSearchParams = match request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
    {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing required param: query".to_string(),
            );
        }
    };

    let workspace = &agent_id;

    // Use existing resolve_entity for fuzzy matching
    let resolved = db
        .graph_store()
        .resolve_entity(&params.query, None, workspace)
        .await
        .unwrap_or_default();

    let results: Vec<GraphSearchResult> = resolved
        .into_iter()
        .take(params.limit)
        .map(|r| GraphSearchResult {
            id: r.node_id,
            name: r.name,
            kind: r.kind,
            match_field: if r.matched_alias.is_some() {
                "alias".to_string()
            } else {
                "name".to_string()
            },
        })
        .collect();

    let response = GraphSearchResponse { results };

    match serde_json::to_value(&response) {
        Ok(value) => JsonRpcResponse::success(request.id, value),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize: {}", e),
        ),
    }
}
```

- [ ] **Step 2: Register placeholder in mod.rs**

```rust
registry.register("graph.search", |req| async move {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        "graph.search requires MemoryBackend - wire at startup".to_string(),
    )
});
```

- [ ] **Step 3: Wire all graph handlers at Gateway startup**

Find the wiring point in the gateway startup code (where other handlers like identity, memory are wired with shared state). Add wiring for all 4 graph handlers following the same pattern. The exact location depends on the codebase — look for where `MemoryBackend` is passed to handlers.

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/handlers/graph.rs src/gateway/handlers/mod.rs
git commit -m "gateway: add graph.search handler and wire all graph handlers"
```

---

## Task 6: Frontend — Add web-sys Canvas Features

**Files:**
- Modify: `interfaces/webchat/Cargo.toml`

- [ ] **Step 1: Add canvas-related web-sys features**

In `interfaces/webchat/Cargo.toml`, extend the `web-sys` features list:

```toml
[dependencies.web-sys]
version = "0.3"
features = [
    # ... existing features ...
    # Canvas features:
    "HtmlCanvasElement",
    "CanvasRenderingContext2d",
    "TextMetrics",
    "DomRect",
    "MouseEvent",
    "WheelEvent",
    "TouchEvent",
    "TouchList",
    "Touch",
]
```

- [ ] **Step 2: Verify WASM compilation**

Run: `cd interfaces/webchat && cargo check --target wasm32-unknown-unknown`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/Cargo.toml
git commit -m "webchat: add web-sys canvas features for graph visualization"
```

---

## Task 7: Frontend — Canvas Engine Types

**Files:**
- Create: `interfaces/webchat/src/canvas_engine/mod.rs`
- Create: `interfaces/webchat/src/canvas_engine/types.rs`

- [ ] **Step 1: Create types.rs with core data structures**

```rust
// interfaces/webchat/src/canvas_engine/types.rs
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn zero() -> Self {
        Self { x: 0.0, y: 0.0 }
    }

    pub fn length(&self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    pub fn normalized(&self) -> Self {
        let len = self.length();
        if len < 1e-10 {
            Self::zero()
        } else {
            Self {
                x: self.x / len,
                y: self.y / len,
            }
        }
    }

    pub fn distance_to(&self, other: &Vec2) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        (dx * dx + dy * dy).sqrt()
    }
}

impl std::ops::Add for Vec2 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self { x: self.x + rhs.x, y: self.y + rhs.y }
    }
}

impl std::ops::Sub for Vec2 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self { x: self.x - rhs.x, y: self.y - rhs.y }
    }
}

impl std::ops::Mul<f64> for Vec2 {
    type Output = Self;
    fn mul(self, rhs: f64) -> Self {
        Self { x: self.x * rhs, y: self.y * rhs }
    }
}

impl std::ops::AddAssign for Vec2 {
    fn add_assign(&mut self, rhs: Self) {
        self.x += rhs.x;
        self.y += rhs.y;
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub fn to_css(&self) -> String {
        format!("rgb({},{},{})", self.r, self.g, self.b)
    }

    pub fn to_css_alpha(&self, alpha: f64) -> String {
        format!("rgba({},{},{},{})", self.r, self.g, self.b, alpha)
    }
}

// Kind → color mapping
pub fn kind_color(kind: &str) -> Color {
    match kind {
        "person" => Color::new(37, 99, 235),    // #2563eb blue
        "concept" => Color::new(124, 58, 237),   // #7c3aed purple
        "project" => Color::new(5, 150, 105),    // #059669 green
        "tool" => Color::new(217, 119, 6),       // #d97706 amber
        "skill" => Color::new(220, 38, 38),      // #dc2626 red
        "event" => Color::new(8, 145, 178),      // #0891b2 cyan
        _ => Color::new(107, 114, 128),           // #6b7280 gray
    }
}

// Kind → icon mapping
pub fn kind_icon(kind: &str) -> &'static str {
    match kind {
        "person" => "\u{1F464}",   // 👤
        "concept" => "\u{1F4A1}",  // 💡
        "project" => "\u{1F4C1}",  // 📁
        "tool" => "\u{1F527}",     // 🔧
        "skill" => "\u{1F3AF}",    // 🎯
        "event" => "\u{1F4C5}",    // 📅
        _ => "\u{2753}",           // ❓
    }
}

#[derive(Debug, Clone)]
pub struct CanvasNode {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub aliases: Vec<String>,
    pub icon: &'static str,
    pub color: Color,
    pub radius: f64,
    pub has_wiki: bool,
    pub position: Vec2,
    pub velocity: Vec2,
    pub pinned: bool,
}

#[derive(Debug, Clone)]
pub struct CanvasEdge {
    pub from_idx: usize,
    pub to_idx: usize,
    pub relation: String,
    pub is_wikilink: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ViewMode {
    Global { top_k: usize },
    Local { center_node_id: String, depth: u8 },
}

#[derive(Debug, Clone)]
pub struct BreadcrumbEntry {
    pub node_id: String,
    pub node_name: String,
}

#[derive(Debug, Clone)]
pub struct ViewState {
    pub mode: ViewMode,
    pub selected_node: Option<String>,
    pub hovered_node: Option<String>,
    pub breadcrumb: Vec<BreadcrumbEntry>,
    pub kind_filter: HashSet<String>,
}

impl ViewState {
    pub fn new() -> Self {
        Self {
            mode: ViewMode::Global { top_k: 100 },
            selected_node: None,
            hovered_node: None,
            breadcrumb: vec![],
            kind_filter: HashSet::new(),
        }
    }
}
```

- [ ] **Step 2: Create mod.rs**

```rust
// interfaces/webchat/src/canvas_engine/mod.rs
pub mod types;
pub mod adapter;
pub mod viewport;
pub mod layout;
pub mod renderer;
pub mod interaction;
```

- [ ] **Step 3: Create stub files for other modules**

Create empty stub files so the module compiles:
- `adapter.rs`: `// GraphNode/Edge → CanvasNode/CanvasEdge conversion`
- `viewport.rs`: `// Zoom/pan transform, hit testing`
- `layout.rs`: `// Force-directed layout`
- `renderer.rs`: `// Canvas 2D draw calls`
- `interaction.rs`: `// Mouse/touch event handling`

- [ ] **Step 4: Wire canvas_engine in lib.rs**

Add `pub mod canvas_engine;` to `interfaces/webchat/src/lib.rs`.

- [ ] **Step 5: Verify compilation**

Run: `cd interfaces/webchat && cargo check --target wasm32-unknown-unknown`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/
git commit -m "webchat: add canvas engine types (CanvasNode, CanvasEdge, ViewState)"
```

---

## Task 8: Frontend — Canvas Engine Adapter

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/adapter.rs`

- [ ] **Step 1: Implement server response → CanvasNode/Edge conversion**

```rust
// interfaces/webchat/src/canvas_engine/adapter.rs
use super::types::*;
use serde::Deserialize;

// Mirror of server-side GraphNodeDto / GraphEdgeDto for deserialization
#[derive(Debug, Clone, Deserialize)]
pub struct GraphNodeDto {
    pub id: String,
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub decay_score: f32,
    pub edge_count: usize,
    pub has_wiki: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GraphEdgeDto {
    pub id: String,
    pub from_id: String,
    pub to_id: String,
    pub relation: String,
    pub weight: f32,
    pub confidence: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GraphQueryResponse {
    pub nodes: Vec<GraphNodeDto>,
    pub edges: Vec<GraphEdgeDto>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WikiDto {
    pub id: String,
    pub content: String,
    pub fact_source: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FactDto {
    pub id: String,
    pub content: String,
    pub confidence: f32,
    pub fact_type: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NodeDetailResponse {
    pub node: GraphNodeDto,
    pub wiki: Option<WikiDto>,
    pub facts: Vec<FactDto>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchResultDto {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub match_field: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GraphSearchResponse {
    pub results: Vec<SearchResultDto>,
}

/// Convert server response to canvas-ready data
pub fn adapt_graph_response(response: &GraphQueryResponse) -> (Vec<CanvasNode>, Vec<CanvasEdge>) {
    let nodes: Vec<CanvasNode> = response
        .nodes
        .iter()
        .enumerate()
        .map(|(i, dto)| {
            // Compute radius from weight: min 10, max 30, scaled by decay_score * edge_count
            let weight = dto.decay_score as f64 * (dto.edge_count as f64 + 1.0).ln();
            let radius = 10.0 + (weight * 4.0).min(20.0);

            // Scatter initial positions in a circle to help layout converge faster
            let angle = (i as f64 / response.nodes.len() as f64) * std::f64::consts::TAU;
            let spread = 200.0;

            CanvasNode {
                id: dto.id.clone(),
                name: dto.name.clone(),
                kind: dto.kind.clone(),
                aliases: dto.aliases.clone(),
                icon: kind_icon(&dto.kind),
                color: kind_color(&dto.kind),
                radius,
                has_wiki: dto.has_wiki,
                position: Vec2::new(angle.cos() * spread, angle.sin() * spread),
                velocity: Vec2::zero(),
                pinned: false,
            }
        })
        .collect();

    // Build id → index map for edge resolution
    let id_to_idx: std::collections::HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();

    let edges: Vec<CanvasEdge> = response
        .edges
        .iter()
        .filter_map(|dto| {
            let from_idx = id_to_idx.get(dto.from_id.as_str()).copied()?;
            let to_idx = id_to_idx.get(dto.to_id.as_str()).copied()?;
            Some(CanvasEdge {
                from_idx,
                to_idx,
                relation: dto.relation.clone(),
                is_wikilink: dto.relation == "references",
            })
        })
        .collect();

    (nodes, edges)
}
```

- [ ] **Step 2: Verify compilation**

Run: `cd interfaces/webchat && cargo check --target wasm32-unknown-unknown`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/adapter.rs
git commit -m "webchat: add graph response adapter for canvas engine"
```

---

## Task 9: Frontend — Viewport (Zoom/Pan/Hit Testing)

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/viewport.rs`

- [ ] **Step 1: Implement Viewport struct**

```rust
// interfaces/webchat/src/canvas_engine/viewport.rs
use super::types::{CanvasNode, Vec2};

#[derive(Debug, Clone)]
pub struct Viewport {
    pub offset: Vec2,   // Translation in screen pixels
    pub scale: f64,     // Zoom level (1.0 = 100%)
    pub width: f64,     // Canvas width in pixels
    pub height: f64,    // Canvas height in pixels
}

impl Viewport {
    pub fn new(width: f64, height: f64) -> Self {
        Self {
            offset: Vec2::new(width / 2.0, height / 2.0),
            scale: 1.0,
            width,
            height,
        }
    }

    /// Convert world coordinates to screen coordinates
    pub fn world_to_screen(&self, world: Vec2) -> Vec2 {
        Vec2 {
            x: world.x * self.scale + self.offset.x,
            y: world.y * self.scale + self.offset.y,
        }
    }

    /// Convert screen coordinates to world coordinates
    pub fn screen_to_world(&self, screen: Vec2) -> Vec2 {
        Vec2 {
            x: (screen.x - self.offset.x) / self.scale,
            y: (screen.y - self.offset.y) / self.scale,
        }
    }

    /// Zoom centered on a screen point
    pub fn zoom_at(&mut self, screen_point: Vec2, delta: f64) {
        let min_scale = 0.1;
        let max_scale = 5.0;

        let old_scale = self.scale;
        self.scale = (self.scale * (1.0 + delta)).clamp(min_scale, max_scale);

        // Adjust offset to keep the point under cursor stationary
        let ratio = self.scale / old_scale;
        self.offset.x = screen_point.x - (screen_point.x - self.offset.x) * ratio;
        self.offset.y = screen_point.y - (screen_point.y - self.offset.y) * ratio;
    }

    /// Pan by a screen-space delta
    pub fn pan(&mut self, dx: f64, dy: f64) {
        self.offset.x += dx;
        self.offset.y += dy;
    }

    /// Center the viewport on a world-space point
    pub fn center_on(&mut self, world_point: Vec2) {
        self.offset.x = self.width / 2.0 - world_point.x * self.scale;
        self.offset.y = self.height / 2.0 - world_point.y * self.scale;
    }

    /// Hit test: find the node under a screen point
    pub fn hit_test(&self, screen_point: Vec2, nodes: &[CanvasNode]) -> Option<usize> {
        let world = self.screen_to_world(screen_point);

        // Search in reverse order (last drawn = on top)
        nodes
            .iter()
            .enumerate()
            .rev()
            .find(|(_, node)| {
                let dist = world.distance_to(&node.position);
                dist <= node.radius
            })
            .map(|(idx, _)| idx)
    }

    /// Check if a world-space point is visible on screen (with margin)
    pub fn is_visible(&self, world_point: Vec2, margin: f64) -> bool {
        let screen = self.world_to_screen(world_point);
        screen.x >= -margin
            && screen.x <= self.width + margin
            && screen.y >= -margin
            && screen.y <= self.height + margin
    }
}
```

- [ ] **Step 2: Verify compilation**

Run: `cd interfaces/webchat && cargo check --target wasm32-unknown-unknown`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/viewport.rs
git commit -m "webchat: add viewport with zoom/pan/hit-testing"
```

---

## Task 10: Frontend — Force-Directed Layout

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/layout.rs`

- [ ] **Step 1: Implement ForceLayout with Barnes-Hut approximation**

```rust
// interfaces/webchat/src/canvas_engine/layout.rs
use super::types::{CanvasEdge, CanvasNode, Vec2};

pub struct LayoutConfig {
    pub repulsion_strength: f64,
    pub attraction_strength: f64,
    pub damping: f64,
    pub center_gravity: f64,
    pub max_velocity: f64,
    pub convergence_threshold: f64,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            repulsion_strength: 800.0,
            attraction_strength: 0.015,
            damping: 0.85,
            center_gravity: 0.02,
            max_velocity: 40.0,
            convergence_threshold: 0.5,
        }
    }
}

pub struct ForceLayout {
    pub config: LayoutConfig,
    pub is_settled: bool,
}

impl ForceLayout {
    pub fn new() -> Self {
        Self {
            config: LayoutConfig::default(),
            is_settled: false,
        }
    }

    /// Run one iteration of the force simulation
    /// Returns total kinetic energy (for convergence detection)
    pub fn tick(&mut self, nodes: &mut [CanvasNode], edges: &[CanvasEdge]) -> f64 {
        let n = nodes.len();
        if n == 0 {
            self.is_settled = true;
            return 0.0;
        }

        // Accumulate forces per node
        let mut forces = vec![Vec2::zero(); n];

        // 1. Repulsion: all pairs (O(n^2) for simplicity; upgrade to Barnes-Hut if >500 nodes)
        for i in 0..n {
            for j in (i + 1)..n {
                let delta = nodes[i].position - nodes[j].position;
                let dist = delta.length().max(1.0);
                let force_magnitude = self.config.repulsion_strength / (dist * dist);
                let force = delta.normalized() * force_magnitude;
                forces[i] += force;
                forces[j] = forces[j] - force;
            }
        }

        // 2. Attraction: connected pairs (Hooke's law)
        for edge in edges {
            if edge.from_idx >= n || edge.to_idx >= n {
                continue;
            }
            let delta = nodes[edge.to_idx].position - nodes[edge.from_idx].position;
            let dist = delta.length().max(1.0);
            let force = delta.normalized() * (dist * self.config.attraction_strength);
            forces[edge.from_idx] += force;
            forces[edge.to_idx] = forces[edge.to_idx] - force;
        }

        // 3. Center gravity
        for i in 0..n {
            let to_center = Vec2::zero() - nodes[i].position;
            forces[i] += to_center * self.config.center_gravity;
        }

        // 4. Apply forces → velocity → position
        let mut total_energy = 0.0;
        for i in 0..n {
            if nodes[i].pinned {
                nodes[i].velocity = Vec2::zero();
                continue;
            }

            nodes[i].velocity = (nodes[i].velocity + forces[i]) * self.config.damping;

            // Clamp velocity
            let speed = nodes[i].velocity.length();
            if speed > self.config.max_velocity {
                nodes[i].velocity = nodes[i].velocity.normalized() * self.config.max_velocity;
            }

            nodes[i].position += nodes[i].velocity;
            total_energy += speed * speed;
        }

        self.is_settled = total_energy < self.config.convergence_threshold;
        total_energy
    }

    /// Reset settlement state (call when data changes)
    pub fn wake(&mut self) {
        self.is_settled = false;
    }
}
```

- [ ] **Step 2: Verify compilation**

Run: `cd interfaces/webchat && cargo check --target wasm32-unknown-unknown`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/layout.rs
git commit -m "webchat: add force-directed graph layout engine"
```

---

## Task 11: Frontend — Canvas 2D Renderer

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/renderer.rs`

- [ ] **Step 1: Implement renderer**

```rust
// interfaces/webchat/src/canvas_engine/renderer.rs
use super::types::*;
use super::viewport::Viewport;
use web_sys::CanvasRenderingContext2d;

pub struct Renderer;

impl Renderer {
    /// Draw the entire graph frame
    pub fn draw(
        ctx: &CanvasRenderingContext2d,
        viewport: &Viewport,
        nodes: &[CanvasNode],
        edges: &[CanvasEdge],
        selected: Option<&str>,
        hovered: Option<&str>,
        kind_filter: &std::collections::HashSet<String>,
    ) {
        // Clear
        ctx.clear_rect(0.0, 0.0, viewport.width, viewport.height);

        // Background
        ctx.set_fill_style_str("#0a0a0f");
        ctx.fill_rect(0.0, 0.0, viewport.width, viewport.height);

        // Save context and apply viewport transform
        ctx.save();
        ctx.translate(viewport.offset.x, viewport.offset.y).ok();
        ctx.scale(viewport.scale, viewport.scale).ok();

        // Draw edges first (below nodes)
        Self::draw_edges(ctx, viewport, nodes, edges, selected, kind_filter);

        // Draw nodes
        Self::draw_nodes(ctx, viewport, nodes, selected, hovered, kind_filter);

        ctx.restore();
    }

    fn draw_edges(
        ctx: &CanvasRenderingContext2d,
        viewport: &Viewport,
        nodes: &[CanvasNode],
        edges: &[CanvasEdge],
        selected: Option<&str>,
        kind_filter: &std::collections::HashSet<String>,
    ) {
        for edge in edges {
            let from = &nodes[edge.from_idx];
            let to = &nodes[edge.to_idx];

            // Skip if either endpoint is filtered out
            if !kind_filter.is_empty()
                && (!kind_filter.contains(&from.kind) || !kind_filter.contains(&to.kind))
            {
                continue;
            }

            let is_highlighted = selected
                .map(|s| s == from.id || s == to.id)
                .unwrap_or(false);

            if is_highlighted {
                ctx.set_stroke_style_str("rgba(167,139,250,0.8)");
                ctx.set_line_width(2.0 / viewport.scale);
            } else {
                ctx.set_stroke_style_str("rgba(51,51,51,0.6)");
                ctx.set_line_width(1.0 / viewport.scale);
            }

            if edge.is_wikilink {
                ctx.set_line_dash(&js_sys::Array::of2(
                    &(4.0 / viewport.scale).into(),
                    &(4.0 / viewport.scale).into(),
                ))
                .ok();
            }

            ctx.begin_path();
            ctx.move_to(from.position.x, from.position.y);
            ctx.line_to(to.position.x, to.position.y);
            ctx.stroke();

            if edge.is_wikilink {
                ctx.set_line_dash(&js_sys::Array::new()).ok();
            }
        }
    }

    fn draw_nodes(
        ctx: &CanvasRenderingContext2d,
        viewport: &Viewport,
        nodes: &[CanvasNode],
        selected: Option<&str>,
        hovered: Option<&str>,
        kind_filter: &std::collections::HashSet<String>,
    ) {
        for node in nodes {
            // Skip filtered nodes
            if !kind_filter.is_empty() && !kind_filter.contains(&node.kind) {
                continue;
            }

            // Skip off-screen nodes
            if !viewport.is_visible(node.position, node.radius * viewport.scale * 2.0) {
                continue;
            }

            let is_selected = selected.map(|s| s == node.id).unwrap_or(false);
            let is_hovered = hovered.map(|s| s == node.id).unwrap_or(false);

            // Selection ring
            if is_selected {
                ctx.set_stroke_style_str("rgba(167,139,250,0.8)");
                ctx.set_line_width(3.0 / viewport.scale);
                ctx.begin_path();
                ctx.arc(
                    node.position.x,
                    node.position.y,
                    node.radius + 4.0 / viewport.scale,
                    0.0,
                    std::f64::consts::TAU,
                )
                .ok();
                ctx.stroke();
            }

            // Node circle
            let fill_color = if is_hovered {
                node.color.to_css_alpha(0.9)
            } else {
                node.color.to_css_alpha(0.75)
            };
            ctx.set_fill_style_str(&fill_color);
            ctx.begin_path();
            ctx.arc(
                node.position.x,
                node.position.y,
                node.radius,
                0.0,
                std::f64::consts::TAU,
            )
            .ok();
            ctx.fill();

            // Icon (only when large enough on screen)
            let screen_radius = node.radius * viewport.scale;
            if screen_radius >= 15.0 {
                let font_size = (node.radius * 0.8).max(8.0);
                ctx.set_font(&format!("{}px sans-serif", font_size));
                ctx.set_text_align("center");
                ctx.set_text_baseline("middle");
                ctx.fill_text(node.icon, node.position.x, node.position.y)
                    .ok();
            }

            // Wiki badge
            if node.has_wiki && screen_radius >= 12.0 {
                let badge_r = 5.0 / viewport.scale;
                let bx = node.position.x + node.radius * 0.7;
                let by = node.position.y + node.radius * 0.7;
                ctx.set_fill_style_str("#1e3a5f");
                ctx.begin_path();
                ctx.arc(bx, by, badge_r, 0.0, std::f64::consts::TAU).ok();
                ctx.fill();
                ctx.set_font(&format!("{}px sans-serif", 6.0 / viewport.scale));
                ctx.set_fill_style_str("#93c5fd");
                ctx.fill_text("\u{1F4D6}", bx, by).ok();
            }

            // Label (only when zoomed in enough)
            if screen_radius >= 10.0 {
                let font_size = (10.0 / viewport.scale).max(6.0).min(14.0);
                ctx.set_font(&format!("{}px sans-serif", font_size));
                ctx.set_fill_style_str("rgba(170,170,170,0.9)");
                ctx.set_text_align("center");
                ctx.set_text_baseline("top");
                ctx.fill_text(
                    &node.name,
                    node.position.x,
                    node.position.y + node.radius + 4.0 / viewport.scale,
                )
                .ok();
            }
        }
    }

    /// Draw a tooltip near the cursor (in screen space, call after ctx.restore())
    pub fn draw_tooltip(
        ctx: &CanvasRenderingContext2d,
        screen_pos: Vec2,
        node: &CanvasNode,
    ) {
        let lines = vec![
            format!("{} ({})", node.name, node.kind),
            format!("decay: {:.2}", 0.0_f32), // placeholder, actual value from server
            format!("aliases: {}", if node.aliases.is_empty() { "none".to_string() } else { node.aliases.join(", ") }),
        ];

        let padding = 8.0;
        let line_height = 16.0;
        let width = 220.0;
        let height = padding * 2.0 + line_height * lines.len() as f64;
        let x = screen_pos.x + 12.0;
        let y = screen_pos.y + 12.0;

        // Background
        ctx.set_fill_style_str("rgba(22,22,31,0.95)");
        ctx.begin_path();
        ctx.round_rect_with_f64(x, y, width, height, 6.0).ok();
        ctx.fill();

        // Border
        ctx.set_stroke_style_str("rgba(42,42,58,1)");
        ctx.set_line_width(1.0);
        ctx.stroke();

        // Text
        ctx.set_fill_style_str("#ccc");
        ctx.set_font("12px sans-serif");
        ctx.set_text_align("left");
        ctx.set_text_baseline("top");
        for (i, line) in lines.iter().enumerate() {
            ctx.fill_text(line, x + padding, y + padding + i as f64 * line_height)
                .ok();
        }
    }
}
```

- [ ] **Step 2: Verify compilation**

Run: `cd interfaces/webchat && cargo check --target wasm32-unknown-unknown`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/renderer.rs
git commit -m "webchat: add Canvas 2D renderer for graph nodes and edges"
```

---

## Task 12: Frontend — Interaction Handler

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/interaction.rs`

- [ ] **Step 1: Implement interaction event types and dispatcher**

```rust
// interfaces/webchat/src/canvas_engine/interaction.rs
use super::types::Vec2;

/// Events emitted by the interaction layer to the view
#[derive(Debug, Clone)]
pub enum CanvasEvent {
    SelectNode(String),
    DeselectNode,
    EnterLocalView(String),
    HoverNode(Option<String>),
    DragStart { node_idx: usize },
    DragMove { world_pos: Vec2 },
    DragEnd,
}

/// Tracks mouse/touch state for gesture recognition
pub struct InteractionState {
    pub is_panning: bool,
    pub is_dragging_node: bool,
    pub dragged_node_idx: Option<usize>,
    pub last_mouse_screen: Vec2,
    pub mouse_down_screen: Vec2,
    pub mouse_down_time: f64,
    pub last_click_time: f64,
}

impl InteractionState {
    pub fn new() -> Self {
        Self {
            is_panning: false,
            is_dragging_node: false,
            dragged_node_idx: None,
            last_mouse_screen: Vec2::zero(),
            mouse_down_screen: Vec2::zero(),
            mouse_down_time: 0.0,
            last_click_time: 0.0,
        }
    }

    /// Determine if a mouse-up counts as a click (vs drag)
    pub fn is_click(&self, up_pos: Vec2) -> bool {
        let dist = up_pos.distance_to(&self.mouse_down_screen);
        dist < 5.0
    }

    /// Determine if a click is a double-click (within 300ms)
    pub fn is_double_click(&self, now: f64) -> bool {
        now - self.last_click_time < 300.0
    }
}
```

- [ ] **Step 2: Verify compilation**

Run: `cd interfaces/webchat && cargo check --target wasm32-unknown-unknown`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/interaction.rs
git commit -m "webchat: add canvas interaction state and event types"
```

---

## Task 13: Frontend — Graph API Client

**Files:**
- Create: `interfaces/webchat/src/api/graph.rs`
- Modify: `interfaces/webchat/src/api/mod.rs`

- [ ] **Step 1: Create GraphApi struct**

```rust
// interfaces/webchat/src/api/graph.rs
use crate::canvas_engine::adapter::*;
use crate::context::DashboardState;
use serde_json::json;

pub struct GraphApi;

impl GraphApi {
    pub async fn query(
        state: &DashboardState,
        limit: usize,
        kind_filter: Vec<String>,
    ) -> Result<GraphQueryResponse, String> {
        let params = json!({
            "limit": limit,
            "kind_filter": kind_filter,
        });
        let result = state.rpc_call("graph.query", params).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse graph.query: {}", e))
    }

    pub async fn neighbors(
        state: &DashboardState,
        node_id: &str,
        depth: u8,
        limit: usize,
    ) -> Result<GraphQueryResponse, String> {
        let params = json!({
            "node_id": node_id,
            "depth": depth,
            "limit": limit,
        });
        let result = state.rpc_call("graph.neighbors", params).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse graph.neighbors: {}", e))
    }

    pub async fn node_detail(
        state: &DashboardState,
        node_id: &str,
    ) -> Result<NodeDetailResponse, String> {
        let params = json!({ "node_id": node_id });
        let result = state.rpc_call("graph.node_detail", params).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse graph.node_detail: {}", e))
    }

    pub async fn search(
        state: &DashboardState,
        query: &str,
        limit: usize,
    ) -> Result<GraphSearchResponse, String> {
        let params = json!({ "query": query, "limit": limit });
        let result = state.rpc_call("graph.search", params).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse graph.search: {}", e))
    }
}
```

- [ ] **Step 2: Add module to api/mod.rs**

Add `pub mod graph;` to `interfaces/webchat/src/api/mod.rs`.

- [ ] **Step 3: Verify compilation**

Run: `cd interfaces/webchat && cargo check --target wasm32-unknown-unknown`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/api/graph.rs interfaces/webchat/src/api/mod.rs
git commit -m "webchat: add GraphApi client for graph JSON-RPC methods"
```

---

## Task 14: Frontend — PanelMode + Routing Integration

**Files:**
- Modify: `interfaces/webchat/src/components/bottom_bar.rs`
- Modify: `interfaces/webchat/src/app.rs`
- Modify: `interfaces/webchat/src/views/mod.rs`
- Create: `interfaces/webchat/src/views/canvas/mod.rs` (placeholder)

- [ ] **Step 1: Add PanelMode::Canvas variant**

In `interfaces/webchat/src/components/bottom_bar.rs`:

Add `Canvas` variant to `PanelMode` enum:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelMode {
    Chat,
    Dashboard,
    Canvas,    // NEW
    Agents,
    Settings,
}
```

Update `from_path`:
```rust
pub fn from_path(path: &str) -> Self {
    if path.starts_with("/agents") {
        Self::Agents
    } else if path.starts_with("/dashboard") {
        Self::Dashboard
    } else if path.starts_with("/canvas") {
        Self::Canvas
    } else if path.starts_with("/settings") {
        Self::Settings
    } else {
        Self::Chat
    }
}
```

Add Canvas tab in the bottom bar view, between Dashboard and Agents. Use an appropriate SVG icon (graph/nodes icon) or text label "Canvas".

- [ ] **Step 2: Create placeholder Canvas view**

```rust
// interfaces/webchat/src/views/canvas/mod.rs
use leptos::prelude::*;

#[component]
pub fn CanvasView() -> impl IntoView {
    view! {
        <div class="flex-1 flex items-center justify-center text-gray-500">
            <p>"Canvas — Knowledge Graph (loading...)"</p>
        </div>
    }
}
```

Add `pub mod canvas;` to `interfaces/webchat/src/views/mod.rs`.

- [ ] **Step 3: Add Canvas panel routing in app.rs MainContent**

Follow the existing pattern: add a `<div>` wrapper with `style:display` toggle for `PanelMode::Canvas`, and render `CanvasView` inside it.

- [ ] **Step 4: Build WASM and verify in browser**

Run: `cd interfaces/webchat && trunk build` (or the project's WASM build command)
Expected: New Canvas tab appears in bottom bar, clicking it shows the placeholder text.

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/components/bottom_bar.rs interfaces/webchat/src/app.rs interfaces/webchat/src/views/canvas/ interfaces/webchat/src/views/mod.rs
git commit -m "webchat: add Canvas as top-level panel with placeholder view"
```

---

## Task 15: Frontend — Canvas View Components (Toolbar + Detail Panel + Breadcrumb)

**Files:**
- Create: `interfaces/webchat/src/views/canvas/toolbar.rs`
- Create: `interfaces/webchat/src/views/canvas/detail_panel.rs`
- Create: `interfaces/webchat/src/views/canvas/breadcrumb.rs`

- [ ] **Step 1: Create toolbar.rs**

The toolbar contains: agent label, search input, Global/Local toggle, filter button.

```rust
// interfaces/webchat/src/views/canvas/toolbar.rs
use leptos::prelude::*;

#[component]
pub fn CanvasToolbar(
    agent_name: Signal<String>,
    search_query: RwSignal<String>,
    is_local_mode: Signal<bool>,
    #[prop(into)] on_toggle_mode: Callback<()>,
    #[prop(into)] on_search: Callback<String>,
) -> impl IntoView {
    let search_input = RwSignal::new(String::new());

    let handle_search_submit = move |_| {
        let q = search_input.get();
        if !q.is_empty() {
            on_search.call(q);
        }
    };

    view! {
        <div class="flex items-center gap-3 px-4 py-2 bg-[#16161f] border-b border-[#2a2a3a] text-sm">
            // Agent label
            <span class="px-3 py-1 rounded-md bg-[#1e1e2e] text-[#a0a0b0] cursor-pointer hover:text-white"
                  title="Switch agent in Agents panel">
                {move || format!("\u{1F916} {} \u{2197}", agent_name.get())}
            </span>

            // Search box
            <input
                type="text"
                class="flex-1 px-3 py-1.5 rounded-md bg-[#1e1e2e] border border-[#333] text-[#ccc] placeholder-[#666] text-sm outline-none focus:border-[#a78bfa]"
                placeholder="\u{1F50D} Search nodes..."
                prop:value=search_input
                on:input=move |ev| search_input.set(event_target_value(&ev))
                on:keydown=move |ev| {
                    if ev.key() == "Enter" {
                        handle_search_submit(());
                    }
                }
            />

            // Global/Local toggle
            <div class="flex rounded-md overflow-hidden border border-[#333]">
                <button
                    class=move || if !is_local_mode.get() { "px-3 py-1 bg-[#3730a3] text-[#e0e7ff]" } else { "px-3 py-1 bg-[#1e1e2e] text-[#a0a0b0] hover:text-white" }
                    on:click=move |_| { if is_local_mode.get() { on_toggle_mode.call(()); } }
                >
                    "\u{1F310} Global"
                </button>
                <button
                    class=move || if is_local_mode.get() { "px-3 py-1 bg-[#3730a3] text-[#e0e7ff]" } else { "px-3 py-1 bg-[#1e1e2e] text-[#a0a0b0] hover:text-white" }
                    on:click=move |_| { if !is_local_mode.get() { on_toggle_mode.call(()); } }
                >
                    "\u{1F4CD} Local"
                </button>
            </div>
        </div>
    }
}
```

- [ ] **Step 2: Create detail_panel.rs**

The detail panel shows wiki markdown + facts for the selected node.

```rust
// interfaces/webchat/src/views/canvas/detail_panel.rs
use crate::canvas_engine::adapter::{FactDto, NodeDetailResponse, WikiDto};
use crate::canvas_engine::types::*;
use leptos::prelude::*;

#[component]
pub fn DetailPanel(
    detail: Signal<Option<NodeDetailResponse>>,
    #[prop(into)] on_wikilink_click: Callback<String>,
) -> impl IntoView {
    view! {
        <div class="w-80 bg-[#13131c] border-l border-[#2a2a3a] overflow-y-auto p-5 text-sm">
            {move || match detail.get() {
                None => view! {
                    <p class="text-[#666] text-center mt-8">"Click a node to see details"</p>
                }.into_any(),
                Some(d) => {
                    let node = d.node;
                    let wiki = d.wiki;
                    let facts = d.facts;
                    view! {
                        <div>
                            // Entity header
                            <div class="flex items-center gap-3 mb-4">
                                <div class="w-9 h-9 rounded-full flex items-center justify-center text-lg"
                                     style=move || format!("background:{}", kind_color(&node.kind).to_css())>
                                    {kind_icon(&node.kind)}
                                </div>
                                <div>
                                    <div class="text-lg font-semibold text-white">{&node.name}</div>
                                    <div class="text-xs text-[#888]">
                                        {format!("{} \u{00B7} decay: {:.2} \u{00B7} {} edges",
                                            node.kind, node.decay_score, node.edge_count)}
                                    </div>
                                </div>
                            </div>

                            // Wiki section
                            <div class="text-xs font-semibold text-[#888] uppercase tracking-wider mt-4 mb-2">
                                "\u{1F4D6} Wiki"
                            </div>
                            {match wiki {
                                Some(w) => {
                                    // Render markdown content using pulldown-cmark
                                    let html = render_markdown(&w.content);
                                    view! {
                                        <div class="bg-[#1a1a26] rounded-lg p-3.5 text-sm leading-relaxed"
                                             inner_html=html />
                                    }.into_any()
                                }
                                None => view! {
                                    <p class="text-[#555] italic">"No wiki page compiled yet"</p>
                                }.into_any(),
                            }}

                            // Facts section
                            <div class="text-xs font-semibold text-[#888] uppercase tracking-wider mt-4 mb-2">
                                {format!("\u{1F4CB} Related Facts ({})", facts.len())}
                            </div>
                            <div class="space-y-1.5">
                                {facts.into_iter().map(|f| {
                                    view! {
                                        <div class="px-3 py-2 bg-[#1a1a26] rounded-md text-xs text-[#aaa] border-l-2 border-[#3730a3]">
                                            {&f.content}
                                            <span class="ml-2 text-[#555]">{format!("({:.2})", f.confidence)}</span>
                                        </div>
                                    }
                                }).collect_view()}
                            </div>
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

/// Convert markdown to HTML using pulldown-cmark (already in deps)
fn render_markdown(md: &str) -> String {
    use pulldown_cmark::{Parser, Options, html};
    let parser = Parser::new_ext(md, Options::all());
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);
    html_output
}
```

- [ ] **Step 3: Create breadcrumb.rs**

```rust
// interfaces/webchat/src/views/canvas/breadcrumb.rs
use crate::canvas_engine::types::BreadcrumbEntry;
use leptos::prelude::*;

#[component]
pub fn Breadcrumb(
    entries: Signal<Vec<BreadcrumbEntry>>,
    #[prop(into)] on_navigate: Callback<Option<String>>,
) -> impl IntoView {
    view! {
        <div class="flex items-center gap-1 px-4 py-1.5 bg-[#111118] border-b border-[#2a2a3a] text-xs text-[#888]">
            <button
                class="hover:text-white cursor-pointer"
                on:click=move |_| on_navigate.call(None)
            >
                "\u{1F310} Global"
            </button>
            {move || entries.get().into_iter().map(|entry| {
                let id = entry.node_id.clone();
                view! {
                    <span class="text-[#444]">" \u{203A} "</span>
                    <button
                        class="hover:text-white cursor-pointer"
                        on:click=move |_| on_navigate.call(Some(id.clone()))
                    >
                        {entry.node_name}
                    </button>
                }
            }).collect_view()}
        </div>
    }
}
```

- [ ] **Step 4: Update views/canvas/mod.rs to export sub-components**

```rust
pub mod toolbar;
pub mod detail_panel;
pub mod breadcrumb;
pub mod graph_canvas;

use leptos::prelude::*;
// Full CanvasView implementation will be in Task 17
```

- [ ] **Step 5: Verify compilation**

Run: `cd interfaces/webchat && cargo check --target wasm32-unknown-unknown`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/views/canvas/
git commit -m "webchat: add Canvas toolbar, detail panel, and breadcrumb components"
```

---

## Task 16: Frontend — Graph Canvas Component (Render Loop + Events)

**Files:**
- Create: `interfaces/webchat/src/views/canvas/graph_canvas.rs`

- [ ] **Step 1: Create the `<canvas>` Leptos component with render loop**

This is the core component that owns the `<canvas>` element, runs the render loop via `requestAnimationFrame`, and dispatches mouse events to the interaction handler.

```rust
// interfaces/webchat/src/views/canvas/graph_canvas.rs
use crate::canvas_engine::interaction::*;
use crate::canvas_engine::layout::ForceLayout;
use crate::canvas_engine::renderer::Renderer;
use crate::canvas_engine::types::*;
use crate::canvas_engine::viewport::Viewport;
use leptos::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use web_sys::HtmlCanvasElement;

/// Shared mutable state for the render loop (not reactive — direct mutation for 60fps)
pub struct GraphState {
    pub nodes: Vec<CanvasNode>,
    pub edges: Vec<CanvasEdge>,
    pub viewport: Viewport,
    pub layout: ForceLayout,
    pub interaction: InteractionState,
    pub selected_node_id: Option<String>,
    pub hovered_node_id: Option<String>,
    pub kind_filter: std::collections::HashSet<String>,
}

#[component]
pub fn GraphCanvas(
    graph_state: Rc<RefCell<GraphState>>,
    #[prop(into)] on_select_node: Callback<Option<String>>,
    #[prop(into)] on_enter_local: Callback<String>,
) -> impl IntoView {
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();

    // Start render loop after mount
    Effect::new(move || {
        if let Some(canvas_el) = canvas_ref.get() {
            let canvas: HtmlCanvasElement = canvas_el.into();
            let ctx = canvas
                .get_context("2d")
                .unwrap()
                .unwrap()
                .dyn_into::<web_sys::CanvasRenderingContext2d>()
                .unwrap();

            // Set canvas size to match parent
            let parent = canvas.parent_element().unwrap();
            let w = parent.client_width() as f64;
            let h = parent.client_height() as f64;
            canvas.set_width(w as u32);
            canvas.set_height(h as u32);

            {
                let mut state = graph_state.borrow_mut();
                state.viewport = Viewport::new(w, h);
            }

            // requestAnimationFrame render loop
            let state_clone = graph_state.clone();
            let render_loop: Rc<RefCell<Option<Closure<dyn FnMut()>>>> =
                Rc::new(RefCell::new(None));
            let render_loop_clone = render_loop.clone();

            *render_loop.borrow_mut() = Some(Closure::new(move || {
                let mut state = state_clone.borrow_mut();

                // Tick layout if not settled
                if !state.layout.is_settled {
                    state.layout.tick(&mut state.nodes, &state.edges);
                }

                // Render
                Renderer::draw(
                    &ctx,
                    &state.viewport,
                    &state.nodes,
                    &state.edges,
                    state.selected_node_id.as_deref(),
                    state.hovered_node_id.as_deref(),
                    &state.kind_filter,
                );

                // Schedule next frame
                web_sys::window()
                    .unwrap()
                    .request_animation_frame(
                        render_loop_clone
                            .borrow()
                            .as_ref()
                            .unwrap()
                            .as_ref()
                            .unchecked_ref(),
                    )
                    .unwrap();
            }));

            // Kick off first frame
            web_sys::window()
                .unwrap()
                .request_animation_frame(
                    render_loop
                        .borrow()
                        .as_ref()
                        .unwrap()
                        .as_ref()
                        .unchecked_ref(),
                )
                .unwrap();
        }
    });

    // Mouse event handlers
    let state_for_down = graph_state.clone();
    let state_for_move = graph_state.clone();
    let state_for_up = graph_state.clone();
    let state_for_wheel = graph_state.clone();

    let on_mouse_down = move |ev: web_sys::MouseEvent| {
        let mut state = state_for_down.borrow_mut();
        let screen = Vec2::new(ev.offset_x() as f64, ev.offset_y() as f64);
        state.interaction.mouse_down_screen = screen;
        state.interaction.last_mouse_screen = screen;
        state.interaction.mouse_down_time = js_sys::Date::now();

        if let Some(idx) = state.viewport.hit_test(screen, &state.nodes) {
            state.interaction.is_dragging_node = true;
            state.interaction.dragged_node_idx = Some(idx);
            state.nodes[idx].pinned = true;
        } else {
            state.interaction.is_panning = true;
        }
    };

    let on_mouse_move = move |ev: web_sys::MouseEvent| {
        let mut state = state_for_move.borrow_mut();
        let screen = Vec2::new(ev.offset_x() as f64, ev.offset_y() as f64);
        let delta_screen = screen - state.interaction.last_mouse_screen;
        state.interaction.last_mouse_screen = screen;

        if state.interaction.is_panning {
            state.viewport.pan(delta_screen.x, delta_screen.y);
        } else if state.interaction.is_dragging_node {
            if let Some(idx) = state.interaction.dragged_node_idx {
                let world = state.viewport.screen_to_world(screen);
                state.nodes[idx].position = world;
                state.layout.wake();
            }
        } else {
            // Hover detection
            let hit = state.viewport.hit_test(screen, &state.nodes);
            state.hovered_node_id = hit.map(|idx| state.nodes[idx].id.clone());
        }
    };

    let on_select_clone = on_select_node.clone();
    let on_enter_clone = on_enter_local.clone();
    let on_mouse_up = move |ev: web_sys::MouseEvent| {
        let mut state = state_for_up.borrow_mut();
        let screen = Vec2::new(ev.offset_x() as f64, ev.offset_y() as f64);
        let now = js_sys::Date::now();

        // Release pinned node
        if let Some(idx) = state.interaction.dragged_node_idx {
            state.nodes[idx].pinned = false;
        }

        if state.interaction.is_click(screen) {
            if let Some(idx) = state.viewport.hit_test(screen, &state.nodes) {
                let node_id = state.nodes[idx].id.clone();
                if state.interaction.is_double_click(now) {
                    // Double-click → enter local view
                    on_enter_clone.call(node_id);
                } else {
                    // Single click → select
                    state.selected_node_id = Some(node_id.clone());
                    on_select_clone.call(Some(node_id));
                }
            } else {
                // Click on empty space → deselect
                state.selected_node_id = None;
                on_select_clone.call(None);
            }
            state.interaction.last_click_time = now;
        }

        state.interaction.is_panning = false;
        state.interaction.is_dragging_node = false;
        state.interaction.dragged_node_idx = None;
    };

    let on_wheel = move |ev: web_sys::WheelEvent| {
        ev.prevent_default();
        let mut state = state_for_wheel.borrow_mut();
        let screen = Vec2::new(ev.offset_x() as f64, ev.offset_y() as f64);
        let delta = -ev.delta_y() * 0.001;
        state.viewport.zoom_at(screen, delta);
    };

    view! {
        <div class="flex-1 relative overflow-hidden bg-[#0a0a0f]">
            <canvas
                node_ref=canvas_ref
                class="w-full h-full"
                on:mousedown=on_mouse_down
                on:mousemove=on_mouse_move
                on:mouseup=on_mouse_up
                on:wheel=on_wheel
            />
        </div>
    }
}
```

- [ ] **Step 2: Verify compilation**

Run: `cd interfaces/webchat && cargo check --target wasm32-unknown-unknown`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/views/canvas/graph_canvas.rs
git commit -m "webchat: add GraphCanvas component with render loop and mouse events"
```

---

## Task 17: Frontend — Wire CanvasView (Top-Level Composition)

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/mod.rs`

- [ ] **Step 1: Implement full CanvasView that wires everything together**

```rust
// interfaces/webchat/src/views/canvas/mod.rs
pub mod breadcrumb;
pub mod detail_panel;
pub mod graph_canvas;
pub mod toolbar;

use crate::api::graph::GraphApi;
use crate::canvas_engine::adapter::*;
use crate::canvas_engine::layout::ForceLayout;
use crate::canvas_engine::types::*;
use crate::canvas_engine::viewport::Viewport;
use crate::context::DashboardState;
use graph_canvas::GraphState;
use leptos::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

#[component]
pub fn CanvasView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // Reactive signals for UI state
    let view_mode = RwSignal::new(ViewMode::Global { top_k: 100 });
    let selected_node_id = RwSignal::new(None::<String>);
    let node_detail = RwSignal::new(None::<NodeDetailResponse>);
    let breadcrumb = RwSignal::new(Vec::<BreadcrumbEntry>::new());
    let search_query = RwSignal::new(String::new());
    let agent_name = Signal::derive(move || "default".to_string()); // TODO: wire to global agent context
    let is_local_mode = Signal::derive(move || matches!(view_mode.get(), ViewMode::Local { .. }));

    // Non-reactive graph state for the render loop (60fps direct mutation)
    let graph_state = Rc::new(RefCell::new(GraphState {
        nodes: vec![],
        edges: vec![],
        viewport: Viewport::new(800.0, 600.0),
        layout: ForceLayout::new(),
        interaction: crate::canvas_engine::interaction::InteractionState::new(),
        selected_node_id: None,
        hovered_node_id: None,
        kind_filter: std::collections::HashSet::new(),
    }));

    // Load graph data when connected or view mode changes
    let graph_state_for_load = graph_state.clone();
    Effect::new(move || {
        if state.is_connected.get() {
            let mode = view_mode.get();
            let gs = graph_state_for_load.clone();
            let state = state;
            leptos::task::spawn_local(async move {
                let response = match mode {
                    ViewMode::Global { top_k } => {
                        GraphApi::query(&state, top_k, vec![]).await
                    }
                    ViewMode::Local {
                        ref center_node_id,
                        depth,
                    } => {
                        GraphApi::neighbors(&state, center_node_id, depth, 50).await
                    }
                };

                if let Ok(resp) = response {
                    let (nodes, edges) = adapt_graph_response(&resp);
                    let mut g = gs.borrow_mut();
                    g.nodes = nodes;
                    g.edges = edges;
                    g.layout.wake();
                }
            });
        }
    });

    // Fetch node detail when selected
    Effect::new(move || {
        if let Some(node_id) = selected_node_id.get() {
            if state.is_connected.get() {
                let state = state;
                leptos::task::spawn_local(async move {
                    if let Ok(detail) = GraphApi::node_detail(&state, &node_id).await {
                        node_detail.set(Some(detail));
                    }
                });
            }
        } else {
            node_detail.set(None);
        }
    });

    // Callbacks
    let on_select_node = Callback::new(move |id: Option<String>| {
        selected_node_id.set(id);
    });

    let on_enter_local = Callback::new(move |node_id: String| {
        // Push breadcrumb and switch to local mode
        let gs = graph_state.clone();
        let name = {
            let g = gs.borrow();
            g.nodes
                .iter()
                .find(|n| n.id == node_id)
                .map(|n| n.name.clone())
                .unwrap_or_default()
        };
        breadcrumb.update(|b| {
            b.push(BreadcrumbEntry {
                node_id: node_id.clone(),
                node_name: name,
            });
        });
        view_mode.set(ViewMode::Local {
            center_node_id: node_id,
            depth: 2,
        });
    });

    let on_breadcrumb_navigate = Callback::new(move |target: Option<String>| {
        match target {
            None => {
                // Back to global
                breadcrumb.set(vec![]);
                view_mode.set(ViewMode::Global { top_k: 100 });
            }
            Some(node_id) => {
                // Truncate breadcrumb to this entry
                breadcrumb.update(|b| {
                    if let Some(pos) = b.iter().position(|e| e.node_id == node_id) {
                        b.truncate(pos + 1);
                    }
                });
                view_mode.set(ViewMode::Local {
                    center_node_id: node_id,
                    depth: 2,
                });
            }
        }
    });

    let on_toggle_mode = Callback::new(move |_: ()| {
        let current = view_mode.get();
        match current {
            ViewMode::Global { .. } => {
                // Can't switch to local without a target — do nothing
            }
            ViewMode::Local { .. } => {
                breadcrumb.set(vec![]);
                view_mode.set(ViewMode::Global { top_k: 100 });
            }
        }
    });

    let on_search = Callback::new(move |query: String| {
        let state = state;
        leptos::task::spawn_local(async move {
            if let Ok(results) = GraphApi::search(&state, &query, 10).await {
                if let Some(first) = results.results.first() {
                    selected_node_id.set(Some(first.id.clone()));
                    // TODO: center viewport on the node
                }
            }
        });
    });

    let on_wikilink_click = Callback::new(move |_slug: String| {
        // TODO: resolve slug to node_id and navigate
    });

    let graph_state_for_canvas = graph_state.clone();

    view! {
        <div class="flex flex-col h-full">
            <toolbar::CanvasToolbar
                agent_name=agent_name
                search_query=search_query
                is_local_mode=is_local_mode
                on_toggle_mode=on_toggle_mode
                on_search=on_search
            />

            {move || if is_local_mode.get() {
                view! {
                    <breadcrumb::Breadcrumb
                        entries=Signal::derive(move || breadcrumb.get())
                        on_navigate=on_breadcrumb_navigate
                    />
                }.into_any()
            } else {
                view! { <div /> }.into_any()
            }}

            <div class="flex flex-1 overflow-hidden">
                <graph_canvas::GraphCanvas
                    graph_state=graph_state_for_canvas
                    on_select_node=on_select_node
                    on_enter_local=on_enter_local
                />

                {move || if selected_node_id.get().is_some() {
                    view! {
                        <detail_panel::DetailPanel
                            detail=Signal::derive(move || node_detail.get())
                            on_wikilink_click=on_wikilink_click
                        />
                    }.into_any()
                } else {
                    view! { <div /> }.into_any()
                }}
            </div>
        </div>
    }
}
```

- [ ] **Step 2: Build WASM and test in browser**

Run the full WASM build. Open the browser, navigate to Canvas tab:
- Verify toolbar renders with agent label, search box, mode toggle
- Verify canvas area renders (may show empty graph if no data)
- If server is running with graph data, verify nodes appear and layout animates

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/views/canvas/mod.rs
git commit -m "webchat: wire CanvasView with toolbar, detail panel, breadcrumb, and graph canvas"
```

---

## Task 18: Server-Side Handler Wiring

**Files:**
- Modify: Gateway startup code (exact file depends on wiring pattern — check `src/gateway/mod.rs` or `src/bin/aleph-server/commands/start/mod.rs`)

- [ ] **Step 1: Wire graph handlers with MemoryBackend**

Find where other handlers (like `memory.search`, `identity.get`) are wired with shared state. Follow the same pattern to replace the placeholder registrations with actual handler calls:

```rust
// Pseudocode — adapt to actual wiring mechanism
let db = memory_backend.clone();
let agent_resolver = /* however agent_id is resolved from session */;

registry.wire("graph.query", move |req| {
    let db = db.clone();
    let agent_id = resolve_agent_from_request(&req);
    async move { graph::handle_query(req, db, agent_id).await }
});
// ... same for graph.neighbors, graph.node_detail, graph.search
```

The exact wiring depends on how the gateway passes shared state to handlers. The agent executing this task should read the existing wiring code for `memory.search` or `identity.get` and replicate the pattern.

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 3: Test end-to-end**

1. Start aleph-server: `cargo run --bin aleph-server -- start`
2. Open browser at Canvas tab
3. Verify graph loads (if the agent has graph data)
4. Click a node → verify detail panel shows
5. Double-click → verify local view loads

- [ ] **Step 4: Commit**

```bash
git add src/gateway/
git commit -m "gateway: wire graph.query/neighbors/node_detail/search handlers"
```

---

## Summary

| Task | Component | Files |
|------|-----------|-------|
| 1 | Server graph API types | `graph_types.rs` |
| 2 | `graph.query` handler | `graph.rs`, `mod.rs` |
| 3 | `graph.neighbors` handler | `graph.rs` |
| 4 | `graph.node_detail` handler | `graph.rs` |
| 5 | `graph.search` handler | `graph.rs`, `mod.rs` |
| 6 | web-sys canvas features | `Cargo.toml` |
| 7 | Canvas engine types | `canvas_engine/types.rs`, `mod.rs` |
| 8 | Adapter (server→canvas) | `canvas_engine/adapter.rs` |
| 9 | Viewport (zoom/pan) | `canvas_engine/viewport.rs` |
| 10 | Force-directed layout | `canvas_engine/layout.rs` |
| 11 | Canvas 2D renderer | `canvas_engine/renderer.rs` |
| 12 | Interaction handler | `canvas_engine/interaction.rs` |
| 13 | Graph API client | `api/graph.rs` |
| 14 | PanelMode + routing | `bottom_bar.rs`, `app.rs` |
| 15 | Toolbar + detail + breadcrumb | `views/canvas/*.rs` |
| 16 | GraphCanvas component | `views/canvas/graph_canvas.rs` |
| 17 | CanvasView composition | `views/canvas/mod.rs` |
| 18 | Server handler wiring | Gateway startup code |
