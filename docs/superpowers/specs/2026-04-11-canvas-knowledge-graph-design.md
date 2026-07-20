# Canvas Knowledge Graph Visualization - Design Spec

**Date:** 2026-04-11
**Status:** Approved
**Scope:** Phase 1 — Graph browsing with search, dual-mode, filtering, wiki integration

---

## 1. Overview

Add a Canvas panel to Aleph's web UI (Leptos/WASM) for visualizing the knowledge graph as an interactive node-link diagram. Each Agent's graph is isolated; switching agents switches the entire graph view. Nodes display associated Wiki pages and Facts in a right-side detail panel.

Canvas is positioned as a **top-level Panel** (5th tab alongside Chat, Dashboard, Agents, Settings), giving it full-screen space for spatial interaction.

### Goals

- Visualize GraphNode/GraphEdge relationships per agent
- Browse and explore the knowledge graph with zoom, pan, search
- View Wiki markdown and related Facts for any entity
- Support Global (Top-K overview) and Local (neighborhood) view modes

### Non-Goals (Phase 1)

- Manual node creation/editing on canvas
- Layout persistence to SQLite
- Canvas as LLM Tool (R9 integration)
- WebGL rendering for 10k+ node graphs

---

## 2. Architecture

### 2.1 Layer Diagram

```
┌─────────────────────────────────────────────────────┐
│  Frontend (Leptos/WASM) — interfaces/webchat/       │
│  Canvas 2D Renderer · Force-Directed Layout          │
│  Zoom/Pan Engine · Node Interaction · Detail Panel   │
├─────────────────────────────────────────────────────┤
│            ↕ WebSocket JSON-RPC 2.0                  │
├─────────────────────────────────────────────────────┤
│  Server API (aleph-server)                           │
│  graph.query · graph.neighbors · graph.search        │
├─────────────────────────────────────────────────────┤
│  Data Layer (existing, no schema changes)            │
│  graph_nodes · graph_edges · memory_entities · facts │
│  Agent column isolation                              │
└─────────────────────────────────────────────────────┘
```

### 2.2 Principles

- **R1 compliant**: Frontend uses only web_sys Canvas 2D API, no platform-specific code
- **R2 compliant**: All UI in Leptos/WASM, no business logic in native shell
- **R3 compliant**: Core provides data via JSON-RPC; layout computation is frontend-only
- **R4 compliant**: Canvas Panel is pure I/O — queries server, renders response

### 2.3 Data Flow

1. User opens Canvas tab → frontend sends `graph.query` via WebSocket
2. Server queries `GraphStore` with `AgentEnvFilter::Single(current_agent_id)`
3. Server returns `{ nodes: GraphNode[], edges: GraphEdge[], wiki_facts: MemoryFact[] }`
4. Frontend runs force-directed layout algorithm on node/edge data
5. Canvas 2D renders nodes, edges, labels at computed positions
6. User clicks node → frontend sends `graph.node_detail` for detail panel; double-click sends `graph.neighbors` for Local View

---

## 3. Navigation & Positioning

### 3.1 Panel Position

Canvas is a **top-level panel** in the BottomBar tab system:

```
[Chat] [Dashboard] [Canvas] [Agents] [Settings]
```

New `PanelMode::Canvas` variant added to the existing enum.

### 3.2 Agent Context

Canvas follows the **global agent context** (same as other panels). No agent selector within Canvas.

Toolbar displays a read-only label: `🤖 researcher ↗` — clicking it navigates to Agents panel for switching.

---

## 4. Canvas Toolbar

```
[🤖 agent-label ↗] [🔍 Search nodes...] [🌐 Global | 📍 Local] [⚙ Filter]
```

| Element | Behavior |
|---------|----------|
| Agent label | Read-only, shows current agent. Click navigates to Agents panel |
| Search box | Fuzzy match on node `name` and `aliases`. Results in dropdown, selecting centers + highlights node |
| Global/Local toggle | Switches between Top-K global view and neighborhood local view |
| Filter button | Popover with checkboxes per `kind` (person, concept, project, tool, etc.) to toggle visibility |

---

## 5. View Modes

### 5.1 Global View (default)

- Loads Top-K nodes ranked by `decay_score × edge_count` (default K=100, configurable)
- All edges between loaded nodes are displayed
- Force-directed layout positions nodes automatically
- Entry point when opening Canvas or switching agents

### 5.2 Local View

- Triggered by double-clicking a node in Global View, or selecting from search
- Centers on the target node, loads 1-2 hop neighbors
- Breadcrumb navigation: `Global → Rust → Ownership`
- Breadcrumb items are clickable to return to previous scope
- Back button returns to Global View

### 5.3 Transition

- Global → Local: double-click node or search-select
- Local → Local: double-click another node in the neighborhood (breadcrumb appends)
- Local → Global: click "Global" in breadcrumb or toggle button

---

## 6. Node Visual Design

### 6.1 Visual Encoding

| Channel | Maps To |
|---------|---------|
| **Size** | Weight: `decay_score × edge_count`, range 20px-60px diameter |
| **Color** | `kind` classification (see palette below) |
| **Icon** | `kind` semantic icon, shown when node diameter ≥ 30px |
| **Label** | `name`, shown below node when zoom level is sufficient |
| **Wiki badge** | 📖 small indicator at bottom-right when node has associated Wiki fact |

### 6.2 Color Palette

| Kind | Color | Icon |
|------|-------|------|
| person | `#2563eb` (blue) | 👤 |
| concept | `#7c3aed` (purple) | 💡 |
| project | `#059669` (green) | 📁 |
| tool | `#d97706` (amber) | 🔧 |
| skill | `#dc2626` (red) | 🎯 |
| event | `#0891b2` (cyan) | 📅 |
| unknown/other | `#6b7280` (gray) | ❓ |

Additional kinds use a deterministic hash-to-color mapping from a predefined palette.

### 6.3 Edge Rendering

- Default: thin gray line (`#333`, 1px, 60% opacity)
- Highlighted (connected to selected node): purple (`#a78bfa`, 2px, 80% opacity)
- Edge labels (relation name) shown only at high zoom levels
- Wikilink edges (`relation="references"`) rendered as dashed lines

---

## 7. Interaction Model

| Action | Behavior |
|--------|----------|
| Single-click node | Select node, open right detail panel with wiki + facts |
| Double-click node | Switch to Local View centered on this node |
| Hover node | Tooltip: name, kind, aliases, decay_score, edge count |
| Scroll wheel | Zoom in/out with cursor as anchor point |
| Click + drag background | Pan the canvas viewport |
| Click + drag node | Temporarily pin node position; release to unpin |
| Click empty area | Deselect node, close detail panel |
| Search box input | Fuzzy filter nodes, dropdown with top 10 matches |
| Select search result | Center viewport on node, select it, open detail panel |
| Filter toggle | Show/hide nodes by kind, edges auto-hide when both endpoints hidden |

---

## 8. Right Detail Panel

Slides out from the right edge when a node is selected. Width: ~320px (fixed). Canvas area shrinks accordingly.

### 8.1 Panel Layout

```
┌─────────────────────────┐
│ [Icon] Entity Name       │
│ kind · decay: 0.92 · 8e │
├─────────────────────────┤
│ 📖 Wiki                  │
│ ┌─────────────────────┐ │
│ │ ## Summary           │ │
│ │ Rendered markdown... │ │
│ │ ## Key Facts         │ │
│ │ - bullet points...   │ │
│ │ ## Related           │ │
│ │ [[wikilinks]]        │ │
│ └─────────────────────┘ │
├─────────────────────────┤
│ 📋 Related Facts (N)     │
│ ┌─────────────────────┐ │
│ │ Fact content (0.95)  │ │
│ │ Fact content (0.82)  │ │
│ │ ...scrollable...     │ │
│ └─────────────────────┘ │
└─────────────────────────┘
```

### 8.2 Content Sources

- **Wiki section**: Query `MemoryStore` for `MemoryFact` where `fact_type=Wiki` linked via `memory_entities` to the selected `GraphNode`
- **Facts section**: Query `GraphStore.get_facts_for_node()` then batch-fetch `MemoryFact` records, sorted by confidence descending
- **Wikilinks**: Rendered as clickable links that navigate to the target node on canvas (center + select)

### 8.3 No Wiki Fallback

If the selected node has no associated Wiki page:
- Wiki section shows "No wiki page compiled yet" with muted text
- Facts section still displays all linked facts
- A subtle indicator replaces the wiki badge on the node

---

## 9. Server API (New JSON-RPC Methods)

### 9.1 `graph.query`

Returns Top-K nodes with their edges for Global View.

**Params:**
```json
{
  "limit": 100,
  "sort_by": "weight",
  "kind_filter": ["person", "concept"]
}
```

**Response:**
```json
{
  "nodes": [
    {
      "id": "gn_xxx", "name": "Rust", "kind": "concept",
      "aliases": ["rust-lang"], "decay_score": 0.92,
      "edge_count": 8, "has_wiki": true
    }
  ],
  "edges": [
    {
      "id": "ge_xxx", "from_id": "gn_a", "to_id": "gn_b",
      "relation": "uses", "weight": 0.8, "confidence": 0.9
    }
  ]
}
```

Note: `edge_count` and `has_wiki` are computed server-side to avoid extra round trips. Agent filter is implicit from the global agent context attached to the WebSocket session.

### 9.2 `graph.neighbors`

Returns 1-2 hop neighborhood for Local View.

**Params:**
```json
{
  "node_id": "gn_xxx",
  "depth": 2,
  "limit": 50
}
```

**Response:** Same shape as `graph.query`.

### 9.3 `graph.node_detail`

Returns wiki + facts for the detail panel.

**Params:**
```json
{
  "node_id": "gn_xxx"
}
```

**Response:**
```json
{
  "node": { "id": "gn_xxx", "name": "Rust", "kind": "concept", ... },
  "wiki": {
    "id": "fact_xxx", "content": "# Rust\n\n## Summary\n...",
    "fact_source": "synthesis", "updated_at": 1712345678
  },
  "facts": [
    { "id": "fact_yyy", "content": "User prefers Rust...", "confidence": 0.95, "fact_type": "preference" }
  ]
}
```

### 9.4 `graph.search`

Fuzzy search across node names and aliases.

**Params:**
```json
{
  "query": "rust",
  "limit": 10
}
```

**Response:**
```json
{
  "results": [
    { "id": "gn_xxx", "name": "Rust", "kind": "concept", "match_field": "name" },
    { "id": "gn_yyy", "name": "Rust Ownership", "kind": "concept", "match_field": "name" }
  ]
}
```

---

## 10. Rendering Engine

### 10.1 Technology

- **Canvas 2D API** via `web_sys::CanvasRenderingContext2d`
- `<canvas>` element managed by a Leptos component with `NodeRef`
- `requestAnimationFrame` loop via `web_sys::window().request_animation_frame()`

### 10.2 Coordinate System

- **World coordinates**: nodes positioned by layout algorithm in unbounded 2D space
- **Screen coordinates**: viewport transform (translate + scale) maps world → screen
- Transform matrix: `[scale, 0, 0, scale, translateX, translateY]`
- Mouse events convert screen → world coords for hit testing

### 10.3 Render Pipeline (per frame)

1. Clear canvas
2. Apply viewport transform
3. Draw edges (lines between node centers)
4. Draw nodes (filled circles with icon/color)
5. Draw labels (if zoom level sufficient)
6. Draw selection highlight (if node selected)
7. Draw hover tooltip (if applicable)

### 10.4 Performance Budget

- Target: 60fps for ≤200 nodes, 30fps for ≤1000 nodes
- Optimization: skip rendering off-screen nodes/edges
- Optimization: reduce label rendering at low zoom levels
- Optimization: throttle layout iterations when graph is settled (low energy)

---

## 11. Force-Directed Layout

### 11.1 Algorithm

Barnes-Hut approximation of the standard force-directed model:

- **Repulsion**: All nodes repel each other (Coulomb's law, quadtree-accelerated)
- **Attraction**: Connected nodes attract (Hooke's law, proportional to edge weight)
- **Centering**: Gentle force pulling all nodes toward canvas center
- **Damping**: Velocity damping to converge to equilibrium

### 11.2 Implementation

Pure Rust, compiled to WASM. Runs in the browser's main thread with requestAnimationFrame.

```rust
struct ForceLayout {
    positions: Vec<Vec2>,   // Node positions (world coords)
    velocities: Vec<Vec2>,  // Node velocities
    edges: Vec<(usize, usize, f32)>,  // (from, to, weight)
    config: LayoutConfig,
}

struct LayoutConfig {
    repulsion_strength: f32,    // default: 1000.0
    attraction_strength: f32,   // default: 0.01
    damping: f32,               // default: 0.9
    center_gravity: f32,        // default: 0.02
    max_velocity: f32,          // default: 50.0
    theta: f32,                 // Barnes-Hut threshold: 0.8
}
```

### 11.3 Convergence

- Layout runs continuously until total kinetic energy drops below threshold
- After convergence, layout pauses (saves CPU)
- Resumes on: new data loaded, node dragged, view mode changed

---

## 12. Frontend Component Structure

```
interfaces/webchat/src/
├── views/
│   └── canvas/
│       ├── mod.rs              // CanvasView top-level component
│       ├── toolbar.rs          // Toolbar (agent label, search, mode toggle, filter)
│       ├── graph_canvas.rs     // <canvas> element + render loop
│       ├── detail_panel.rs     // Right-side wiki + facts panel
│       ├── breadcrumb.rs       // Breadcrumb navigation for Local View
│       └── types.rs            // CanvasNode, CanvasEdge, ViewState types
├── canvas_engine/
│   ├── mod.rs                  // Public API
│   ├── renderer.rs             // Canvas 2D draw calls
│   ├── layout.rs               // Force-directed layout (Barnes-Hut)
│   ├── viewport.rs             // Zoom/pan transform, hit testing
│   ├── interaction.rs          // Mouse/touch event handling
│   └── adapter.rs              // GraphNode/Edge → CanvasNode/CanvasEdge conversion
```

### 12.1 Data Types

```rust
struct CanvasNode {
    id: String,
    name: String,
    kind: String,
    icon: char,
    color: Color,
    radius: f32,         // Computed from weight
    has_wiki: bool,
    position: Vec2,      // Set by layout engine
    pinned: bool,        // True while user is dragging
}

struct CanvasEdge {
    from_idx: usize,     // Index into nodes array
    to_idx: usize,
    relation: String,
    is_wikilink: bool,   // Dashed rendering
}

enum ViewMode {
    Global { top_k: usize },
    Local { center_node_id: String, depth: u8 },
}

struct ViewState {
    mode: ViewMode,
    selected_node: Option<String>,
    breadcrumb: Vec<(String, String)>,  // (node_id, node_name)
    kind_filter: HashSet<String>,       // Empty = show all
}
```

---

## 13. Web-sys Features Required

Add to `interfaces/webchat/Cargo.toml` under `[dependencies.web-sys]` features:

```toml
features = [
    # Existing features...
    # New for Canvas:
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

---

## 14. Testing Strategy

### 14.1 Unit Tests

- Force-directed layout: verify convergence, node separation, edge attraction
- Viewport transform: world ↔ screen coordinate conversion
- Hit testing: node detection at various zoom levels
- Adapter: GraphNode → CanvasNode mapping correctness

### 14.2 Integration Tests

- `graph.query` returns correct nodes/edges filtered by agent
- `graph.neighbors` returns correct N-hop neighborhood
- `graph.node_detail` returns wiki + facts for a node
- Agent isolation: switching agent returns different graph data

### 14.3 Manual Testing

- Visual inspection of node layout, colors, icons, labels
- Interaction testing: zoom, pan, click, double-click, drag
- Detail panel: wiki markdown rendering, facts list
- Mode switching: Global ↔ Local transitions
- Performance: smooth animation with 100+ nodes
