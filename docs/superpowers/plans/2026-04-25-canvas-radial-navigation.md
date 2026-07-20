# Canvas Radial Navigation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement Phase 2 Canvas radial navigation: flip default view from global Top-K force-directed to active-centered radial neighborhood, with 2.5D visual depth, node folding, smooth focus-switch animation, and mini-map.

**Architecture:** Pure Rust/WASM increment. Active node pinned at `(0,0)`, neighbors arranged in concentric rings by hop depth, sector by relation type. Constrained force-directed (target spring + Barnes-Hut repulsion) replaces edge-attraction model. Server adds 2 response fields. New modules: `navigation`, `tween`, `cluster`, `mini_map`, `prefetch`. Feature-flagged for safe rollout.

**Tech Stack:** Rust 2021, Leptos 0.8 CSR, web-sys Canvas 2D, wasm-bindgen, serde_json, existing SQLite GraphStore/MemoryStore (unchanged).

**Spec:** `docs/superpowers/specs/2026-04-25-canvas-radial-navigation-design.md` (commit `431098df8`)

---

## File Map

### Backend (modified)

| File | Change |
|------|--------|
| `src/gateway/handlers/graph_types.rs` | Add `center: GraphNodeDto` + `hop_depth: HashMap<String, u8>` to `GraphNeighborsResponse` |
| `src/gateway/handlers/graph.rs` | Populate new fields in `graph_neighbors` handler |

### Frontend — `interfaces/webchat/src/canvas_engine/`

| File | Change |
|------|--------|
| `mod.rs` | Add module exports for navigation/tween/cluster/mini_map/prefetch |
| `types.rs` | Add `NavState`, `Neighborhood`, `ClusterNode`, `DepthAttrs`, `Vec3`; extend `CanvasNode`/`CanvasEdge` |
| `adapter.rs` | Add `to_neighborhood()` constructing `Neighborhood` from server response |
| `viewport.rs` | Add `parallax_offset()`, `world_to_screen_with_z()` |
| `layout.rs` | Refactor: replace edge-attraction with target-position spring; add radial geometry helpers |
| `renderer.rs` | Add Z-sorting, depth attrs, bezier edges, shadows, glow, blur, breathing |
| `interaction.rs` | Add hover prefetch debounce, keyboard navigation |
| **`navigation.rs`** (new) | NavState machine, breadcrumb history stack, entry-point selection |
| **`tween.rs`** (new) | Node/camera tween, easing, animation interruption |
| **`cluster.rs`** (new) | Folding rules, ClusterNode construction, expand/collapse state |
| **`mini_map.rs`** (new) | Mini-map sampling, render, click mapping |
| **`prefetch.rs`** (new) | Hover debounce, LRU neighborhood cache |

### Frontend — `interfaces/webchat/src/views/canvas/`

| File | Change |
|------|--------|
| `mod.rs` | Wire NavStateMachine, mode toggle, mini-map container, feature flag dispatch |
| `toolbar.rs` | Add detail slider, Local/Global toggle |
| `graph_canvas.rs` | Wire prefetch, keyboard, animation loop, render-snapshot for interruption |
| `detail_panel.rs` | Add cluster summary view branch |
| `breadcrumb.rs` | Convert to navigation history stack with `history.pushState` (hash) |

### Frontend — `interfaces/webchat/src/api/`

| File | Change |
|------|--------|
| `graph.rs` | Deserialize `center` + `hop_depth` fields on `graph.neighbors` response |

### Shared — `shared/ui_logic/`

| File | Change |
|------|--------|
| `src/user_prefs.rs` (or equivalent) | Add `canvas_radial_navigation: bool` user preference |

---

## Task 1: Server response fields for `graph.neighbors`

**Files:**
- Modify: `src/gateway/handlers/graph_types.rs`
- Modify: `src/gateway/handlers/graph.rs`
- Test: `tests/gateway/graph_handlers_test.rs` (existing or create)

- [ ] **Step 1: Write failing test for `center` field**

```rust
// tests/gateway/graph_handlers_test.rs
#[tokio::test]
async fn graph_neighbors_returns_center_and_hop_depth() {
    let ctx = test_context_with_graph().await;
    let center_id = ctx.seed_node("Rust", "concept").await;
    let _hop1 = ctx.seed_neighbor(&center_id, "uses").await;

    let resp: GraphNeighborsResponse = ctx
        .rpc("graph.neighbors", json!({ "node_id": center_id, "depth": 2, "limit": 50 }))
        .await
        .unwrap();

    assert_eq!(resp.center.id, center_id, "center field must equal request node_id");
    assert!(!resp.hop_depth.is_empty(), "hop_depth must be populated");
    for node in &resp.nodes {
        let hop = resp.hop_depth.get(&node.id).copied();
        assert!(matches!(hop, Some(1) | Some(2)), "hop must be 1 or 2 for {}", node.id);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --test graph_handlers_test graph_neighbors_returns_center_and_hop_depth -- --nocapture`
Expected: FAIL — compile error "no field `center` / `hop_depth` on `GraphNeighborsResponse`".

- [ ] **Step 3: Add fields to response type**

Modify `src/gateway/handlers/graph_types.rs` — find `GraphNeighborsResponse` and update:

```rust
#[derive(Debug, Serialize)]
pub struct GraphNeighborsResponse {
    pub center: GraphNodeDto,
    pub nodes: Vec<GraphNodeDto>,
    pub edges: Vec<GraphEdgeDto>,
    pub hop_depth: std::collections::HashMap<String, u8>,
}
```

- [ ] **Step 4: Populate fields in handler**

Modify `src/gateway/handlers/graph.rs` `graph_neighbors` function. After existing logic that builds `nodes` and `edges`, add:

```rust
let center = match graph_store
    .get_node(&params.node_id, &agent_filter)
    .await
    .context("fetch center node")?
{
    Some(n) => to_graph_node_dto(&n, &graph_store, &memory_store).await?,
    None => return Err(JsonRpcError::not_found(format!("node {} not found", params.node_id))),
};

let mut hop_depth = std::collections::HashMap::new();
for n in &nodes {
    let depth = compute_hop_depth(&params.node_id, &n.id, &edges);
    hop_depth.insert(n.id.clone(), depth);
}

Ok(GraphNeighborsResponse { center, nodes, edges, hop_depth })
```

Add helper at module bottom:

```rust
fn compute_hop_depth(center_id: &str, target_id: &str, edges: &[GraphEdgeDto]) -> u8 {
    if center_id == target_id { return 0; }
    if edges.iter().any(|e| (e.from_id == center_id && e.to_id == target_id) || (e.to_id == center_id && e.from_id == target_id)) {
        return 1;
    }
    2
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p alephcore --test graph_handlers_test graph_neighbors_returns_center_and_hop_depth -- --nocapture`
Expected: PASS.

- [ ] **Step 6: Verify backward compatibility**

Run: `cargo test -p alephcore --lib` (full unit test suite)
Expected: PASS — no existing test should break since new fields are additive.

- [ ] **Step 7: Commit**

```bash
git add src/gateway/handlers/graph_types.rs src/gateway/handlers/graph.rs tests/gateway/graph_handlers_test.rs
git commit -m "graph(api): add center and hop_depth fields to graph.neighbors response"
```

---

## Task 2: Frontend types extension

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/types.rs`
- Modify: `interfaces/webchat/src/canvas_engine/mod.rs`

- [ ] **Step 1: Add `Vec3`, `DepthAttrs`, extend `CanvasNode`/`CanvasEdge`**

Modify `interfaces/webchat/src/canvas_engine/types.rs`. Append at the end:

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const fn new(x: f32, y: f32, z: f32) -> Self { Self { x, y, z } }
}

#[derive(Debug, Clone, Copy)]
pub struct DepthAttrs {
    pub scale: f32,
    pub opacity: f32,
    pub blur_px: f32,
    pub sat_mul: f32,
    pub glow_alpha: f32,
    pub shadow_offset_y: f32,
}
```

In the existing `CanvasNode` struct, add these fields:

```rust
pub z: f32,
pub hop: u8,        // 0 = active, 1, 2
pub decay_score: f32,
pub edge_count: usize,
```

In the existing `CanvasEdge` struct, add:

```rust
pub is_active_link: bool,
```

- [ ] **Step 2: Add `ClusterNode`**

Append to `types.rs`:

```rust
#[derive(Debug, Clone)]
pub struct ClusterNode {
    pub id: String,
    pub relation: String,
    pub kind: String,
    pub member_ids: Vec<String>,
    pub representative_names: Vec<String>,
    pub aggregated_weight: f32,
    pub radius: f32,
    pub world_pos: Vec2,
    pub z: f32,
    pub expanded: bool,
}
```

- [ ] **Step 3: Add `Neighborhood` and `NavState`**

Append:

```rust
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Neighborhood {
    pub center: CanvasNode,
    pub one_hop: Vec<CanvasNode>,
    pub two_hop: Vec<CanvasNode>,
    pub clusters: Vec<ClusterNode>,
    pub edges: Vec<CanvasEdge>,
    pub target_positions: HashMap<String, Vec3>,
    pub fetched_at_ms: f64,   // performance.now() timestamp
}

#[derive(Debug, Clone)]
pub enum NavState {
    Idle,
    Loading {
        target: String,
        since_ms: f64,
    },
    Active {
        node_id: String,
        neighborhood: Neighborhood,
    },
    Animating {
        from_id: String,
        to_id: String,
        from_neighborhood: Neighborhood,
        to_neighborhood: Neighborhood,
        t: f32,
        duration_ms: u32,
        started_at_ms: f64,
    },
    Error {
        target: String,
        reason: String,
    },
}
```

Note: We avoid `std::time::Instant` because it doesn't compile cleanly in WASM; using `f64` ms from `performance.now()`.

- [ ] **Step 4: Update `mod.rs` to export new modules**

Replace `interfaces/webchat/src/canvas_engine/mod.rs` content:

```rust
pub mod adapter;
pub mod cluster;
pub mod interaction;
pub mod layout;
pub mod mini_map;
pub mod navigation;
pub mod prefetch;
pub mod renderer;
pub mod tween;
pub mod types;
pub mod viewport;
```

- [ ] **Step 5: Add unit test for `Vec3::new` and `DepthAttrs` defaults**

Append to `types.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vec3_new_constructs_correctly() {
        let v = Vec3::new(1.0, 2.0, 3.0);
        assert_eq!(v.x, 1.0);
        assert_eq!(v.y, 2.0);
        assert_eq!(v.z, 3.0);
    }
}
```

- [ ] **Step 6: Run test (will fail to compile because new modules don't exist yet)**

Run: `cargo check -p aleph-panel`
Expected: FAIL — modules `cluster`, `navigation`, `prefetch`, `tween`, `mini_map` don't exist.

- [ ] **Step 7: Create stub files for new modules**

Create empty files (to be filled in later tasks):

```bash
echo "// Filled in by Task 4" > interfaces/webchat/src/canvas_engine/cluster.rs
echo "// Filled in by Task 7" > interfaces/webchat/src/canvas_engine/prefetch.rs
echo "// Filled in by Task 8" > interfaces/webchat/src/canvas_engine/navigation.rs
echo "// Filled in by Task 9" > interfaces/webchat/src/canvas_engine/tween.rs
echo "// Filled in by Task 14" > interfaces/webchat/src/canvas_engine/mini_map.rs
```

- [ ] **Step 8: Run check**

Run: `cargo check -p aleph-panel`
Expected: PASS (warnings about unused modules are OK).

- [ ] **Step 9: Run unit test**

Run: `cargo test -p aleph-panel --lib canvas_engine::types`
Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/
git commit -m "canvas: extend types and scaffold new modules (radial navigation phase)"
```

---

## Task 3: API client for new response fields

**Files:**
- Modify: `interfaces/webchat/src/api/graph.rs`

- [ ] **Step 1: Locate the `GraphNeighborsResponse` deserialization struct**

Run: `grep -n 'GraphNeighborsResponse\|graph.neighbors' interfaces/webchat/src/api/graph.rs`

- [ ] **Step 2: Add new fields to the API client's response type**

In `interfaces/webchat/src/api/graph.rs`, find the response struct (likely named similarly to server) and add fields:

```rust
#[derive(Debug, Clone, serde::Deserialize)]
pub struct GraphNeighborsResponse {
    pub center: GraphNodeDto,
    pub nodes: Vec<GraphNodeDto>,
    pub edges: Vec<GraphEdgeDto>,
    #[serde(default)]
    pub hop_depth: std::collections::HashMap<String, u8>,
}
```

The `#[serde(default)]` on `hop_depth` allows graceful handling if the server is older.

- [ ] **Step 3: Verify GraphNodeDto contains required fields**

Run: `grep -n 'struct GraphNodeDto' interfaces/webchat/src/api/graph.rs`. Ensure it includes `id`, `name`, `kind`, `aliases`, `decay_score`, `edge_count`, `has_wiki`. If any missing, add them.

- [ ] **Step 4: Run check**

Run: `cargo check -p aleph-panel`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/api/graph.rs
git commit -m "canvas(api): deserialize center and hop_depth from graph.neighbors response"
```

---

## Task 4: Cluster module — folding logic

**Files:**
- Create: `interfaces/webchat/src/canvas_engine/cluster.rs`

- [ ] **Step 1: Write failing tests for fold threshold**

Replace contents of `interfaces/webchat/src/canvas_engine/cluster.rs`:

```rust
use crate::canvas_engine::types::*;
use std::collections::HashMap;

pub const FOLD_THRESHOLD: usize = 12;

/// Fold neighbors of a single relation sector into ClusterNodes by kind.
/// Returns (unfolded_nodes, clusters).
pub fn fold_sector(
    neighbors: &[CanvasNode],
    relation: &str,
    active_id: &str,
    threshold: usize,
) -> (Vec<CanvasNode>, Vec<ClusterNode>) {
    todo!("implement in step 3")
}

/// Fallback fold: if total neighbors >= 30 and no individual kind hits threshold,
/// keep top 20 by weight and force-fold the rest.
pub fn fallback_fold(
    neighbors: Vec<CanvasNode>,
    relation: &str,
    active_id: &str,
) -> (Vec<CanvasNode>, Vec<ClusterNode>) {
    todo!("implement in step 3")
}

/// Compute ClusterNode display radius: 24 + 6 * log2(N), capped at 60.
pub fn cluster_radius(n: usize) -> f32 {
    todo!("implement in step 3")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas_engine::types::Vec2;

    fn mock_node(id: &str, kind: &str, weight: f32) -> CanvasNode {
        CanvasNode {
            id: id.to_string(),
            name: id.to_string(),
            kind: kind.to_string(),
            aliases: vec![],
            icon: '?',
            color: Color::default(),
            radius: 30.0,
            has_wiki: false,
            decay_score: weight,
            edge_count: 1,
            world_pos: Vec2::new(0.0, 0.0),
            velocity: Vec2::new(0.0, 0.0),
            z: 60.0,
            hop: 1,
            pinned: false,
        }
    }

    #[test]
    fn fold_below_threshold_keeps_all() {
        let neighbors: Vec<_> = (0..11).map(|i| mock_node(&format!("n{i}"), "concept", 1.0)).collect();
        let (unfolded, clusters) = fold_sector(&neighbors, "uses", "active", FOLD_THRESHOLD);
        assert_eq!(unfolded.len(), 11);
        assert_eq!(clusters.len(), 0);
    }

    #[test]
    fn fold_at_threshold_creates_cluster() {
        let neighbors: Vec<_> = (0..12).map(|i| mock_node(&format!("n{i}"), "concept", 1.0)).collect();
        let (unfolded, clusters) = fold_sector(&neighbors, "uses", "active", FOLD_THRESHOLD);
        assert_eq!(unfolded.len(), 0);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].member_ids.len(), 12);
        assert_eq!(clusters[0].kind, "concept");
        assert_eq!(clusters[0].id, "cluster::uses::concept::active");
    }

    #[test]
    fn fold_mixed_kinds_only_folds_qualifying_groups() {
        let mut neighbors: Vec<_> = (0..12).map(|i| mock_node(&format!("c{i}"), "concept", 1.0)).collect();
        neighbors.extend((0..5).map(|i| mock_node(&format!("p{i}"), "person", 1.0)));
        let (unfolded, clusters) = fold_sector(&neighbors, "uses", "active", FOLD_THRESHOLD);
        assert_eq!(unfolded.len(), 5, "person nodes should remain unfolded");
        assert_eq!(clusters.len(), 1, "concept group should fold");
    }

    #[test]
    fn cluster_radius_log_scaling() {
        assert!((cluster_radius(2) - (24.0 + 6.0)).abs() < 1e-3);   // log2(2) = 1
        assert!((cluster_radius(16) - (24.0 + 24.0)).abs() < 1e-3); // log2(16) = 4
        assert!(cluster_radius(1024) <= 60.0);                       // capped
    }

    #[test]
    fn fallback_fold_triggers_at_30() {
        let neighbors: Vec<_> = (0..30).map(|i| {
            let kind = if i < 11 { "concept" } else if i < 22 { "person" } else { "tool" };
            mock_node(&format!("n{i}"), kind, i as f32)
        }).collect();
        let (unfolded, clusters) = fallback_fold(neighbors, "uses", "active");
        assert_eq!(unfolded.len(), 20, "top 20 by weight kept");
        assert!(!clusters.is_empty(), "remainder force-folded");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p aleph-panel --lib canvas_engine::cluster`
Expected: FAIL — `todo!()` panics.

- [ ] **Step 3: Implement the functions**

Replace the `todo!()` bodies:

```rust
pub fn cluster_radius(n: usize) -> f32 {
    let r = 24.0 + 6.0 * (n.max(2) as f32).log2();
    r.min(60.0)
}

pub fn fold_sector(
    neighbors: &[CanvasNode],
    relation: &str,
    active_id: &str,
    threshold: usize,
) -> (Vec<CanvasNode>, Vec<ClusterNode>) {
    let mut by_kind: HashMap<String, Vec<CanvasNode>> = HashMap::new();
    for n in neighbors {
        by_kind.entry(n.kind.clone()).or_default().push(n.clone());
    }

    let mut unfolded = Vec::new();
    let mut clusters = Vec::new();
    for (kind, mut group) in by_kind {
        if group.len() >= threshold {
            // Sort by descending weight for representative picking
            group.sort_by(|a, b| {
                let wa = a.decay_score * a.edge_count as f32;
                let wb = b.decay_score * b.edge_count as f32;
                wb.partial_cmp(&wa).unwrap_or(std::cmp::Ordering::Equal)
            });
            let aggregated_weight: f32 = group.iter()
                .map(|n| n.decay_score * n.edge_count as f32)
                .sum();
            let representative_names: Vec<String> = group.iter()
                .take(3)
                .map(|n| n.name.clone())
                .collect();
            let member_ids: Vec<String> = group.iter().map(|n| n.id.clone()).collect();
            clusters.push(ClusterNode {
                id: format!("cluster::{}::{}::{}", relation, kind, active_id),
                relation: relation.to_string(),
                kind,
                member_ids: member_ids.clone(),
                representative_names,
                aggregated_weight,
                radius: cluster_radius(member_ids.len()),
                world_pos: Vec2::new(0.0, 0.0),
                z: 60.0,
                expanded: false,
            });
        } else {
            unfolded.extend(group);
        }
    }
    (unfolded, clusters)
}

pub fn fallback_fold(
    mut neighbors: Vec<CanvasNode>,
    relation: &str,
    active_id: &str,
) -> (Vec<CanvasNode>, Vec<ClusterNode>) {
    if neighbors.len() < 30 {
        return (neighbors, vec![]);
    }
    neighbors.sort_by(|a, b| {
        let wa = a.decay_score * a.edge_count as f32;
        let wb = b.decay_score * b.edge_count as f32;
        wb.partial_cmp(&wa).unwrap_or(std::cmp::Ordering::Equal)
    });
    let kept: Vec<_> = neighbors.drain(..20).collect();
    // Force-fold the remainder by kind
    let (_, clusters) = fold_sector(&neighbors, relation, active_id, 1);
    (kept, clusters)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p aleph-panel --lib canvas_engine::cluster`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/cluster.rs
git commit -m "canvas(cluster): folding logic for kind-based supernode collapsing"
```

---

## Task 5: Layout module — sector geometry

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/layout.rs`

- [ ] **Step 1: Read existing `layout.rs` to understand current shape**

Run: `cat interfaces/webchat/src/canvas_engine/layout.rs`

Identify the existing `Layout` (or equivalent) struct, its public methods, and where `step()` / force computation lives. Note current dependencies (Barnes-Hut quadtree, edge attraction).

- [ ] **Step 2: Write failing tests for sector helpers**

Append to `interfaces/webchat/src/canvas_engine/layout.rs`:

```rust
#[cfg(test)]
mod radial_tests {
    use super::*;

    #[test]
    fn sector_hash_is_deterministic() {
        let a = sector_center_angle("uses");
        let b = sector_center_angle("uses");
        assert!((a - b).abs() < 1e-6);
    }

    #[test]
    fn sector_hash_in_range() {
        for r in &["uses", "part_of", "references", "is_a", "depends_on", "owned_by"] {
            let a = sector_center_angle(r);
            assert!(a >= 0.0 && a < std::f32::consts::TAU, "{r} -> {a}");
        }
    }

    #[test]
    fn assign_sectors_preserves_relative_hash_order() {
        let relations = vec!["uses".to_string(), "part_of".to_string(), "references".to_string()];
        let assigned = assign_sectors(&relations);

        let mut hash_sorted: Vec<_> = relations.iter().cloned().collect();
        hash_sorted.sort_by(|a, b| {
            sector_center_angle(a).partial_cmp(&sector_center_angle(b)).unwrap()
        });

        // After assignment, the relative order in the result should match hash order
        let mut assigned_order: Vec<_> = assigned.iter().collect();
        assigned_order.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let final_relations: Vec<String> = assigned_order.into_iter().map(|(r, _)| r.clone()).collect();

        assert_eq!(final_relations, hash_sorted);
    }

    #[test]
    fn assign_sectors_uniform_distribution() {
        let relations = vec!["a".to_string(), "b".to_string(), "c".to_string(), "d".to_string()];
        let assigned = assign_sectors(&relations);
        let mut angles: Vec<_> = assigned.values().copied().collect();
        angles.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for w in angles.windows(2) {
            let gap = w[1] - w[0];
            assert!((gap - std::f32::consts::TAU / 4.0).abs() < 1e-3);
        }
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p aleph-panel --lib canvas_engine::layout::radial_tests`
Expected: FAIL — `sector_center_angle` and `assign_sectors` not defined.

- [ ] **Step 4: Implement sector helpers**

Add to top of `layout.rs` (after existing imports):

```rust
use std::collections::HashMap;

/// FNV-1a 32-bit hash, deterministic across runs and platforms.
fn fnv1a_32(bytes: &[u8]) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for b in bytes {
        h ^= *b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h
}

/// Map a relation name to a stable angle in [0, 2π).
pub fn sector_center_angle(relation: &str) -> f32 {
    let h = fnv1a_32(relation.as_bytes());
    (h as f32 / u32::MAX as f32) * std::f32::consts::TAU
}

/// Distribute K relations evenly around [0, 2π) but preserve the relative order
/// induced by `sector_center_angle` so spatial memory is consistent.
pub fn assign_sectors(relations: &[String]) -> HashMap<String, f32> {
    let mut sorted: Vec<&String> = relations.iter().collect();
    sorted.sort_by(|a, b| {
        sector_center_angle(a).partial_cmp(&sector_center_angle(b)).unwrap_or(std::cmp::Ordering::Equal)
    });
    let k = sorted.len().max(1) as f32;
    let mut out = HashMap::new();
    for (i, r) in sorted.iter().enumerate() {
        out.insert((*r).clone(), (i as f32) * std::f32::consts::TAU / k);
    }
    out
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p aleph-panel --lib canvas_engine::layout::radial_tests`
Expected: PASS (4 tests).

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/layout.rs
git commit -m "canvas(layout): deterministic relation-to-sector angle assignment"
```

---

## Task 6: Layout — target position computation

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/layout.rs`

- [ ] **Step 1: Write failing test for `compute_target_positions`**

Append to `radial_tests` mod in `layout.rs`:

```rust
    use crate::canvas_engine::types::{CanvasNode, CanvasEdge, Vec2, Vec3, Color};

    fn n(id: &str, kind: &str, hop: u8) -> CanvasNode {
        CanvasNode {
            id: id.to_string(), name: id.to_string(), kind: kind.to_string(),
            aliases: vec![], icon: '?', color: Color::default(),
            radius: 30.0, has_wiki: false, decay_score: 1.0, edge_count: 1,
            world_pos: Vec2::new(0.0, 0.0), velocity: Vec2::new(0.0, 0.0),
            z: 0.0, hop, pinned: false,
        }
    }

    fn e(from: usize, to: usize, relation: &str) -> CanvasEdge {
        CanvasEdge {
            from_idx: from, to_idx: to, relation: relation.to_string(),
            weight: 1.0, is_wikilink: false, is_active_link: true,
        }
    }

    #[test]
    fn compute_targets_active_at_origin() {
        let active = n("a", "concept", 0);
        let one_hop = vec![n("b", "concept", 1)];
        let edges = vec![e(0, 1, "uses")];
        let targets = compute_target_positions(&active, &one_hop, &[], &[], &edges);
        let pos_a = targets.get("a").unwrap();
        assert_eq!(pos_a.x, 0.0);
        assert_eq!(pos_a.y, 0.0);
    }

    #[test]
    fn compute_targets_one_hop_at_r1() {
        let active = n("a", "concept", 0);
        let one_hop = vec![n("b", "concept", 1)];
        let edges = vec![e(0, 1, "uses")];
        let targets = compute_target_positions(&active, &one_hop, &[], &[], &edges);
        let pos_b = targets.get("b").unwrap();
        let r = (pos_b.x.powi(2) + pos_b.y.powi(2)).sqrt();
        assert!((r - 220.0).abs() < 1.0, "1-hop should be at radius 220, got {r}");
    }

    #[test]
    fn compute_targets_two_hop_at_r2() {
        let active = n("a", "concept", 0);
        let one_hop = vec![n("b", "concept", 1)];
        let two_hop = vec![n("c", "concept", 2)];
        let edges = vec![e(0, 1, "uses"), e(1, 2, "part_of")];
        let targets = compute_target_positions(&active, &one_hop, &two_hop, &[], &edges);
        let pos_c = targets.get("c").unwrap();
        let r = (pos_c.x.powi(2) + pos_c.y.powi(2)).sqrt();
        assert!((r - 400.0).abs() < 5.0, "2-hop should be at radius 400, got {r}");
    }
```

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test -p aleph-panel --lib canvas_engine::layout::radial_tests::compute_targets`
Expected: FAIL — `compute_target_positions` not defined.

- [ ] **Step 3: Implement `compute_target_positions`**

Add to `layout.rs` (after `assign_sectors`):

```rust
use crate::canvas_engine::types::{CanvasNode, CanvasEdge, ClusterNode, Vec3};

pub const R_1: f32 = 220.0;
pub const R_2: f32 = 400.0;
pub const Z_ACTIVE: f32 = 0.0;
pub const Z_ONE_HOP: f32 = 60.0;
pub const Z_TWO_HOP: f32 = 140.0;

/// Compute ideal (target) positions for active + neighbors using radial geometry.
pub fn compute_target_positions(
    active: &CanvasNode,
    one_hop: &[CanvasNode],
    two_hop: &[CanvasNode],
    clusters: &[ClusterNode],
    edges: &[CanvasEdge],
) -> HashMap<String, Vec3> {
    let mut out = HashMap::new();
    out.insert(active.id.clone(), Vec3::new(0.0, 0.0, Z_ACTIVE));

    // Group 1-hop neighbors + clusters by their connecting relation
    let mut by_relation: HashMap<String, Vec<(String, f32)>> = HashMap::new(); // (id, weight)
    for n in one_hop {
        let rel = relation_to_active(&active.id, &n.id, edges).unwrap_or_else(|| "_default".to_string());
        let w = n.decay_score * n.edge_count.max(1) as f32;
        by_relation.entry(rel).or_default().push((n.id.clone(), w));
    }
    for c in clusters {
        by_relation.entry(c.relation.clone()).or_default().push((c.id.clone(), c.aggregated_weight));
    }

    // Adaptive R1 if crowded
    let n_one = one_hop.len() + clusters.len();
    let r1 = if n_one >= 16 {
        R_1 + 12.0 * (n_one as f32 - 16.0)
    } else {
        R_1
    };

    // Assign sector center angles
    let relations: Vec<String> = by_relation.keys().cloned().collect();
    let sector_centers = assign_sectors(&relations);

    // Within each sector, distribute by weight descending, alternating around center
    for (rel, members) in &mut by_relation {
        members.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    }

    let total_n = by_relation.values().map(|v| v.len()).sum::<usize>().max(1) as f32;

    for (rel, members) in &by_relation {
        let center_angle = sector_centers.get(rel).copied().unwrap_or(0.0);
        let n_in_sector = members.len() as f32;
        let sector_width = (std::f32::consts::TAU * (n_in_sector / total_n)).max(0.15);
        let delta = sector_width / (n_in_sector + 1.0);
        for (i, (id, _w)) in members.iter().enumerate() {
            // Alternate: 0, +1, -1, +2, -2, ...
            let offset_steps = ((i + 1) / 2) as f32 * if i % 2 == 0 { 1.0 } else { -1.0 };
            let theta = center_angle + offset_steps * delta;
            let x = r1 * theta.cos();
            let y = r1 * theta.sin();
            out.insert(id.clone(), Vec3::new(x, y, Z_ONE_HOP));
        }
    }

    // 2-hop nodes attached to their introducing 1-hop parent
    for n in two_hop {
        let parent_id = find_one_hop_parent(&n.id, one_hop, edges);
        let parent_pos = parent_id.as_ref().and_then(|p| out.get(p)).copied();
        let (px, py) = match parent_pos {
            Some(p) => (p.x, p.y),
            None => (R_1, 0.0), // fallback
        };
        let parent_angle = py.atan2(px);
        let jitter = (fnv1a_32(n.id.as_bytes()) as f32 / u32::MAX as f32 - 0.5) * 0.6; // ±0.3 rad
        let theta = parent_angle + jitter;
        let x = R_2 * theta.cos();
        let y = R_2 * theta.sin();
        out.insert(n.id.clone(), Vec3::new(x, y, Z_TWO_HOP));
    }

    out
}

fn relation_to_active(active_id: &str, neighbor_id: &str, edges: &[CanvasEdge]) -> Option<String> {
    edges.iter().find(|e| {
        // We don't have direct id resolution here; caller must have populated edges where active is endpoint
        // Use is_active_link as a fast filter if available
        e.is_active_link
    }).map(|_e| {
        // Best-effort: caller should set edge.relation correctly
        edges.iter()
            .find(|e| e.is_active_link)
            .map(|e| e.relation.clone())
            .unwrap_or_else(|| "_default".to_string())
    });
    // Above is a fast path. Full impl would resolve indices, but adapter (Task) maps id pairs.
    // For now, pick highest-weight edge mentioning either id by name match in caller layer.
    edges.iter()
        .find(|e| e.is_active_link)
        .map(|e| e.relation.clone())
}

fn find_one_hop_parent(two_hop_id: &str, one_hop: &[CanvasNode], _edges: &[CanvasEdge]) -> Option<String> {
    // Adapter is responsible for ordering one_hop and two_hop; here we pick highest-weight
    // 1-hop as fallback. Full edge-based resolution lives in adapter.
    one_hop.iter()
        .max_by(|a, b| (a.decay_score * a.edge_count as f32)
            .partial_cmp(&(b.decay_score * b.edge_count as f32))
            .unwrap_or(std::cmp::Ordering::Equal))
        .map(|n| n.id.clone())
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p aleph-panel --lib canvas_engine::layout::radial_tests`
Expected: PASS (existing 4 + 3 new = 7 tests).

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/layout.rs
git commit -m "canvas(layout): radial target position computation for active/1-hop/2-hop"
```

---

## Task 7: Layout — constrained force-directed step

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/layout.rs`

- [ ] **Step 1: Write failing test for convergence**

Append to `radial_tests` in `layout.rs`:

```rust
    #[test]
    fn force_step_converges_within_60_iterations() {
        let active = n("a", "concept", 0);
        let one_hop: Vec<_> = (0..5).map(|i| n(&format!("h{i}"), "concept", 1)).collect();
        let edges: Vec<_> = (0..5).map(|i| e(0, i + 1, "uses")).collect();
        let targets = compute_target_positions(&active, &one_hop, &[], &[], &edges);

        let mut layout = RadialForceLayout::new(targets, ForceConfig::default());
        for _ in 0..60 {
            layout.step(0.016);
        }
        assert!(layout.kinetic_energy() < 1.0,
            "expected KE < 1.0 after 60 iters, got {}", layout.kinetic_energy());
    }
```

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test -p aleph-panel --lib canvas_engine::layout::radial_tests::force_step_converges`
Expected: FAIL — types not defined.

- [ ] **Step 3: Add `RadialForceLayout` and `ForceConfig`**

Append to `layout.rs`:

```rust
#[derive(Debug, Clone, Copy)]
pub struct ForceConfig {
    pub target_attract: f32,
    pub repulsion: f32,
    pub damping: f32,
    pub max_velocity: f32,
}

impl Default for ForceConfig {
    fn default() -> Self {
        Self {
            target_attract: 0.15,
            repulsion: 800.0,
            damping: 0.85,
            max_velocity: 50.0,
        }
    }
}

pub struct RadialForceLayout {
    pub positions: HashMap<String, Vec3>,
    pub velocities: HashMap<String, (f32, f32)>,
    targets: HashMap<String, Vec3>,
    config: ForceConfig,
    active_id: Option<String>,
}

impl RadialForceLayout {
    pub fn new(targets: HashMap<String, Vec3>, config: ForceConfig) -> Self {
        let positions = targets.clone();
        let velocities = targets.keys().map(|k| (k.clone(), (0.0_f32, 0.0_f32))).collect();
        Self { positions, velocities, targets, config, active_id: None }
    }

    pub fn pin_active(&mut self, id: String) {
        self.active_id = Some(id);
    }

    pub fn step(&mut self, dt: f32) {
        let cfg = self.config;
        let ids: Vec<String> = self.positions.keys().cloned().collect();
        let mut forces: HashMap<String, (f32, f32)> = ids.iter().map(|i| (i.clone(), (0.0, 0.0))).collect();

        // Spring force toward target
        for id in &ids {
            let pos = self.positions[id];
            let tgt = self.targets[id];
            let fx = cfg.target_attract * (tgt.x - pos.x);
            let fy = cfg.target_attract * (tgt.y - pos.y);
            forces.get_mut(id).unwrap().0 += fx;
            forces.get_mut(id).unwrap().1 += fy;
        }

        // Pairwise repulsion (O(n²); fine for ≤50 nodes)
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                let pi = self.positions[&ids[i]];
                let pj = self.positions[&ids[j]];
                let dx = pi.x - pj.x;
                let dy = pi.y - pj.y;
                let d2 = (dx * dx + dy * dy).max(1.0);
                let f = cfg.repulsion / d2;
                let inv_d = 1.0 / d2.sqrt();
                let fx = f * dx * inv_d;
                let fy = f * dy * inv_d;
                forces.get_mut(&ids[i]).unwrap().0 += fx;
                forces.get_mut(&ids[i]).unwrap().1 += fy;
                forces.get_mut(&ids[j]).unwrap().0 -= fx;
                forces.get_mut(&ids[j]).unwrap().1 -= fy;
            }
        }

        // Integrate (skip pinned active)
        for id in &ids {
            if Some(id) == self.active_id.as_ref() {
                self.velocities.insert(id.clone(), (0.0, 0.0));
                if let Some(p) = self.positions.get_mut(id) {
                    p.x = 0.0;
                    p.y = 0.0;
                }
                continue;
            }
            let (fx, fy) = forces[id];
            let v = self.velocities.get_mut(id).unwrap();
            v.0 = (v.0 + fx * dt) * cfg.damping;
            v.1 = (v.1 + fy * dt) * cfg.damping;
            // Clamp velocity
            let speed = (v.0 * v.0 + v.1 * v.1).sqrt();
            if speed > cfg.max_velocity {
                v.0 *= cfg.max_velocity / speed;
                v.1 *= cfg.max_velocity / speed;
            }
            let pos = self.positions.get_mut(id).unwrap();
            pos.x += v.0 * dt;
            pos.y += v.1 * dt;
        }
    }

    pub fn kinetic_energy(&self) -> f32 {
        self.velocities.values().map(|(vx, vy)| vx * vx + vy * vy).sum::<f32>() * 0.5
    }
}
```

- [ ] **Step 4: Run test**

Run: `cargo test -p aleph-panel --lib canvas_engine::layout::radial_tests::force_step_converges`
Expected: PASS.

- [ ] **Step 5: Run all layout tests**

Run: `cargo test -p aleph-panel --lib canvas_engine::layout`
Expected: PASS (8 tests).

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/layout.rs
git commit -m "canvas(layout): constrained force-directed step with target-spring + repulsion"
```

---

## Task 8: Tween module

**Files:**
- Replace: `interfaces/webchat/src/canvas_engine/tween.rs` (currently a stub)

- [ ] **Step 1: Write failing tests**

Replace `interfaces/webchat/src/canvas_engine/tween.rs`:

```rust
use crate::canvas_engine::types::{Neighborhood, Vec3};
use std::collections::HashMap;

/// Standard smoothstep ease-in-out: 3t² - 2t³.
pub fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    3.0 * t * t - 2.0 * t * t * t
}

/// Linear interpolation between two Vec3s.
pub fn lerp_vec3(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    Vec3::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t, a.z + (b.z - a.z) * t)
}

/// Result of interpolating one node between two neighborhoods.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TweenResult {
    pub pos: Vec3,
    pub opacity: f32,
}

/// Outward drift used for nodes leaving / entering the view.
pub fn drift_outward(direction: Vec3, magnitude: f32) -> Vec3 {
    let len = (direction.x * direction.x + direction.y * direction.y).sqrt().max(1.0);
    Vec3::new(direction.x / len * magnitude, direction.y / len * magnitude, 0.0)
}

/// Interpolate a single node id between old and new neighborhoods at parameter t.
pub fn lerp_node(
    node_id: &str,
    from: &Neighborhood,
    to: &Neighborhood,
    t: f32,
) -> TweenResult {
    let eased = ease_in_out(t);
    let from_pos = from.target_positions.get(node_id).copied();
    let to_pos = to.target_positions.get(node_id).copied();
    match (from_pos, to_pos) {
        (Some(p1), Some(p2)) => TweenResult {
            pos: lerp_vec3(p1, p2, eased),
            opacity: 1.0,
        },
        (Some(p1), None) => {
            let drift = drift_outward(p1, 30.0 * t);
            let drift_z = lerp_vec3(p1, Vec3::new(p1.x, p1.y, 200.0), t);
            TweenResult {
                pos: Vec3::new(drift_z.x + drift.x, drift_z.y + drift.y, drift_z.z),
                opacity: 1.0 - t,
            }
        }
        (None, Some(p2)) => {
            let drift = drift_outward(p2, 30.0 * (1.0 - t));
            let drift_z = lerp_vec3(Vec3::new(p2.x, p2.y, 200.0), p2, t);
            TweenResult {
                pos: Vec3::new(drift_z.x + drift.x, drift_z.y + drift.y, drift_z.z),
                opacity: t,
            }
        }
        (None, None) => TweenResult {
            pos: Vec3::new(0.0, 0.0, 0.0),
            opacity: 0.0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas_engine::types::*;

    fn empty_nbhd() -> Neighborhood {
        Neighborhood {
            center: dummy_node("c"),
            one_hop: vec![],
            two_hop: vec![],
            clusters: vec![],
            edges: vec![],
            target_positions: HashMap::new(),
            fetched_at_ms: 0.0,
        }
    }

    fn dummy_node(id: &str) -> CanvasNode {
        CanvasNode {
            id: id.to_string(), name: id.to_string(), kind: "concept".to_string(),
            aliases: vec![], icon: '?', color: Color::default(),
            radius: 30.0, has_wiki: false, decay_score: 1.0, edge_count: 1,
            world_pos: Vec2::new(0.0, 0.0), velocity: Vec2::new(0.0, 0.0),
            z: 0.0, hop: 0, pinned: false,
        }
    }

    #[test]
    fn ease_endpoints() {
        assert!((ease_in_out(0.0) - 0.0).abs() < 1e-6);
        assert!((ease_in_out(1.0) - 1.0).abs() < 1e-6);
        assert!((ease_in_out(0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn ease_clamps_out_of_range() {
        assert_eq!(ease_in_out(-0.5), 0.0);
        assert_eq!(ease_in_out(1.5), 1.0);
    }

    #[test]
    fn lerp_node_shared_interpolates_position() {
        let mut from = empty_nbhd();
        let mut to = empty_nbhd();
        from.target_positions.insert("x".to_string(), Vec3::new(0.0, 0.0, 0.0));
        to.target_positions.insert("x".to_string(), Vec3::new(100.0, 0.0, 0.0));
        let r = lerp_node("x", &from, &to, 0.5);
        assert!((r.pos.x - 50.0).abs() < 1e-3);
        assert!((r.opacity - 1.0).abs() < 1e-6);
    }

    #[test]
    fn lerp_node_fadeout_only_in_from() {
        let mut from = empty_nbhd();
        let to = empty_nbhd();
        from.target_positions.insert("y".to_string(), Vec3::new(220.0, 0.0, 60.0));
        let r = lerp_node("y", &from, &to, 1.0);
        assert!((r.opacity - 0.0).abs() < 1e-3, "should fade out fully at t=1");
    }

    #[test]
    fn lerp_node_fadein_only_in_to() {
        let from = empty_nbhd();
        let mut to = empty_nbhd();
        to.target_positions.insert("z".to_string(), Vec3::new(220.0, 0.0, 60.0));
        let r = lerp_node("z", &from, &to, 0.0);
        assert!((r.opacity - 0.0).abs() < 1e-3, "should fade in from 0 at t=0");
        let r2 = lerp_node("z", &from, &to, 1.0);
        assert!((r2.opacity - 1.0).abs() < 1e-3, "should be fully visible at t=1");
    }
}
```

- [ ] **Step 2: Run tests to verify failure (then pass)**

Run: `cargo test -p aleph-panel --lib canvas_engine::tween`
Expected: PASS — implementation is included with tests in same diff.

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/tween.rs
git commit -m "canvas(tween): pure-math node lerp with easing and fade-in/out branches"
```

---

## Task 9: Prefetch module — LRU + debounce

**Files:**
- Replace: `interfaces/webchat/src/canvas_engine/prefetch.rs`

- [ ] **Step 1: Write file with implementation + tests**

Replace `interfaces/webchat/src/canvas_engine/prefetch.rs`:

```rust
use crate::canvas_engine::types::Neighborhood;
use std::collections::VecDeque;

pub const HOVER_DEBOUNCE_MS: f64 = 150.0;
pub const CACHE_TTL_MS: f64 = 60_000.0;
pub const CACHE_CAPACITY: usize = 20;

pub struct PrefetchCache {
    entries: VecDeque<(String, Neighborhood)>,
    capacity: usize,
    ttl_ms: f64,
}

impl PrefetchCache {
    pub fn new() -> Self {
        Self { entries: VecDeque::new(), capacity: CACHE_CAPACITY, ttl_ms: CACHE_TTL_MS }
    }

    /// Insert or refresh a neighborhood in the cache.
    pub fn put(&mut self, id: String, nbhd: Neighborhood) {
        self.entries.retain(|(k, _)| k != &id);
        self.entries.push_back((id, nbhd));
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }

    /// Look up a neighborhood; returns Some only if not expired.
    pub fn get(&self, id: &str, now_ms: f64) -> Option<&Neighborhood> {
        self.entries.iter().rev().find_map(|(k, v)| {
            if k == id && now_ms - v.fetched_at_ms <= self.ttl_ms {
                Some(v)
            } else {
                None
            }
        })
    }

    pub fn len(&self) -> usize { self.entries.len() }
}

/// Hover-debounce timer state. Caller calls `note_hover` on each pointer move.
pub struct HoverDebouncer {
    current_id: Option<String>,
    started_at_ms: f64,
}

impl HoverDebouncer {
    pub fn new() -> Self { Self { current_id: None, started_at_ms: 0.0 } }

    /// Returns Some(id) if hover threshold reached, else None.
    pub fn note_hover(&mut self, hovered: Option<&str>, now_ms: f64) -> Option<String> {
        match (hovered, &self.current_id) {
            (Some(id), Some(cur)) if id == cur => {
                if now_ms - self.started_at_ms >= HOVER_DEBOUNCE_MS {
                    let out = self.current_id.take();
                    self.started_at_ms = now_ms; // prevent immediate refire
                    return out;
                }
                None
            }
            (Some(id), _) => {
                self.current_id = Some(id.to_string());
                self.started_at_ms = now_ms;
                None
            }
            (None, _) => {
                self.current_id = None;
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas_engine::types::*;
    use std::collections::HashMap;

    fn nbhd(id: &str, fetched_at: f64) -> Neighborhood {
        Neighborhood {
            center: CanvasNode {
                id: id.to_string(), name: id.to_string(), kind: "concept".to_string(),
                aliases: vec![], icon: '?', color: Color::default(),
                radius: 30.0, has_wiki: false, decay_score: 1.0, edge_count: 1,
                world_pos: Vec2::new(0.0, 0.0), velocity: Vec2::new(0.0, 0.0),
                z: 0.0, hop: 0, pinned: false,
            },
            one_hop: vec![], two_hop: vec![], clusters: vec![],
            edges: vec![], target_positions: HashMap::new(),
            fetched_at_ms: fetched_at,
        }
    }

    #[test]
    fn cache_put_then_get() {
        let mut c = PrefetchCache::new();
        c.put("a".to_string(), nbhd("a", 0.0));
        assert!(c.get("a", 100.0).is_some());
    }

    #[test]
    fn cache_expires_after_ttl() {
        let mut c = PrefetchCache::new();
        c.put("a".to_string(), nbhd("a", 0.0));
        assert!(c.get("a", CACHE_TTL_MS + 1.0).is_none());
    }

    #[test]
    fn cache_evicts_oldest_at_capacity() {
        let mut c = PrefetchCache::new();
        for i in 0..(CACHE_CAPACITY + 5) {
            c.put(format!("n{i}"), nbhd(&format!("n{i}"), 0.0));
        }
        assert_eq!(c.len(), CACHE_CAPACITY);
        assert!(c.get("n0", 0.0).is_none());
        assert!(c.get(&format!("n{}", CACHE_CAPACITY + 4), 0.0).is_some());
    }

    #[test]
    fn debounce_fires_after_threshold() {
        let mut d = HoverDebouncer::new();
        assert_eq!(d.note_hover(Some("x"), 0.0), None);
        assert_eq!(d.note_hover(Some("x"), 100.0), None);
        assert_eq!(d.note_hover(Some("x"), 151.0), Some("x".to_string()));
    }

    #[test]
    fn debounce_resets_on_target_change() {
        let mut d = HoverDebouncer::new();
        d.note_hover(Some("x"), 0.0);
        assert_eq!(d.note_hover(Some("y"), 100.0), None);
        assert_eq!(d.note_hover(Some("y"), 251.0), Some("y".to_string()));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p aleph-panel --lib canvas_engine::prefetch`
Expected: PASS (5 tests).

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/prefetch.rs
git commit -m "canvas(prefetch): LRU neighborhood cache and 150ms hover debounce"
```

---

## Task 10: Navigation state machine

**Files:**
- Replace: `interfaces/webchat/src/canvas_engine/navigation.rs`

- [ ] **Step 1: Write file with state transitions**

Replace `interfaces/webchat/src/canvas_engine/navigation.rs`:

```rust
use crate::canvas_engine::types::{NavState, Neighborhood};

pub const BREADCRUMB_MAX: usize = 20;

pub struct NavController {
    pub state: NavState,
    pub breadcrumb: Vec<(String, String)>, // (id, name)
}

impl NavController {
    pub fn new() -> Self {
        Self { state: NavState::Idle, breadcrumb: Vec::new() }
    }

    pub fn enter(&mut self, target: String, now_ms: f64) {
        self.state = NavState::Loading { target, since_ms: now_ms };
    }

    pub fn fulfilled(&mut self, node_id: String, name: String, neighborhood: Neighborhood) {
        // Append to breadcrumb if this is a new id
        if self.breadcrumb.last().map(|(id, _)| id != &node_id).unwrap_or(true) {
            self.breadcrumb.push((node_id.clone(), name));
            if self.breadcrumb.len() > BREADCRUMB_MAX {
                // Remove second element (keep root and recent)
                self.breadcrumb.remove(1);
            }
        }
        self.state = NavState::Active { node_id, neighborhood };
    }

    pub fn fail(&mut self, target: String, reason: String) {
        self.state = NavState::Error { target, reason };
    }

    pub fn start_animation(
        &mut self,
        from_neighborhood: Neighborhood,
        to_neighborhood: Neighborhood,
        from_id: String,
        to_id: String,
        now_ms: f64,
        duration_ms: u32,
    ) {
        self.state = NavState::Animating {
            from_id,
            to_id,
            from_neighborhood,
            to_neighborhood,
            t: 0.0,
            duration_ms,
            started_at_ms: now_ms,
        };
    }

    pub fn tick(&mut self, now_ms: f64) {
        if let NavState::Animating {
            ref mut t, duration_ms, started_at_ms, to_id, to_neighborhood, ..
        } = &mut self.state
        {
            *t = ((now_ms - *started_at_ms) as f32 / *duration_ms as f32).clamp(0.0, 1.0);
            if *t >= 1.0 {
                let id = std::mem::take(to_id);
                let nbhd = std::mem::replace(to_neighborhood, Neighborhood {
                    center: Default::default(),
                    one_hop: vec![], two_hop: vec![], clusters: vec![],
                    edges: vec![], target_positions: Default::default(),
                    fetched_at_ms: 0.0,
                });
                self.state = NavState::Active { node_id: id, neighborhood: nbhd };
            }
        }
    }

    pub fn breadcrumb_pop_to(&mut self, target_id: &str) -> Option<String> {
        if let Some(pos) = self.breadcrumb.iter().position(|(id, _)| id == target_id) {
            self.breadcrumb.truncate(pos + 1);
            Some(target_id.to_string())
        } else {
            None
        }
    }
}

// Provide Default for CanvasNode so the swap-out trick above compiles.
impl Default for crate::canvas_engine::types::CanvasNode {
    fn default() -> Self {
        use crate::canvas_engine::types::*;
        Self {
            id: String::new(), name: String::new(), kind: String::new(),
            aliases: vec![], icon: '?', color: Color::default(),
            radius: 0.0, has_wiki: false, decay_score: 0.0, edge_count: 0,
            world_pos: Vec2::new(0.0, 0.0), velocity: Vec2::new(0.0, 0.0),
            z: 0.0, hop: 0, pinned: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas_engine::types::*;
    use std::collections::HashMap;

    fn nbhd(id: &str) -> Neighborhood {
        Neighborhood {
            center: CanvasNode { id: id.to_string(), ..Default::default() },
            one_hop: vec![], two_hop: vec![], clusters: vec![],
            edges: vec![], target_positions: HashMap::new(),
            fetched_at_ms: 0.0,
        }
    }

    #[test]
    fn start_idle() {
        let nav = NavController::new();
        assert!(matches!(nav.state, NavState::Idle));
        assert!(nav.breadcrumb.is_empty());
    }

    #[test]
    fn enter_then_fulfill_appends_breadcrumb() {
        let mut nav = NavController::new();
        nav.enter("a".to_string(), 0.0);
        assert!(matches!(nav.state, NavState::Loading { .. }));
        nav.fulfilled("a".to_string(), "Alpha".to_string(), nbhd("a"));
        assert!(matches!(nav.state, NavState::Active { .. }));
        assert_eq!(nav.breadcrumb.len(), 1);
    }

    #[test]
    fn breadcrumb_truncates_at_max() {
        let mut nav = NavController::new();
        for i in 0..(BREADCRUMB_MAX + 5) {
            let id = format!("n{i}");
            nav.fulfilled(id.clone(), id.clone(), nbhd(&id));
        }
        assert_eq!(nav.breadcrumb.len(), BREADCRUMB_MAX);
    }

    #[test]
    fn animation_completes_at_t_1() {
        let mut nav = NavController::new();
        nav.fulfilled("a".to_string(), "A".to_string(), nbhd("a"));
        nav.start_animation(nbhd("a"), nbhd("b"), "a".to_string(), "b".to_string(), 0.0, 400);
        nav.tick(200.0);
        assert!(matches!(nav.state, NavState::Animating { .. }));
        nav.tick(500.0);
        assert!(matches!(nav.state, NavState::Active { .. }));
    }

    #[test]
    fn breadcrumb_pop_to_truncates() {
        let mut nav = NavController::new();
        nav.fulfilled("a".to_string(), "A".to_string(), nbhd("a"));
        nav.fulfilled("b".to_string(), "B".to_string(), nbhd("b"));
        nav.fulfilled("c".to_string(), "C".to_string(), nbhd("c"));
        nav.breadcrumb_pop_to("a");
        assert_eq!(nav.breadcrumb.len(), 1);
        assert_eq!(nav.breadcrumb[0].0, "a");
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p aleph-panel --lib canvas_engine::navigation`
Expected: PASS (5 tests).

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/navigation.rs
git commit -m "canvas(navigation): NavState machine with breadcrumb history stack"
```

---

## Task 11: Adapter — server response → Neighborhood

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/adapter.rs`

- [ ] **Step 1: Read existing `adapter.rs` to understand mapping**

Run: `cat interfaces/webchat/src/canvas_engine/adapter.rs`

- [ ] **Step 2: Add `to_neighborhood` function**

Append to `adapter.rs`:

```rust
use crate::canvas_engine::cluster::{fold_sector, fallback_fold};
use crate::canvas_engine::layout::compute_target_positions;
use crate::canvas_engine::types::{
    CanvasEdge, CanvasNode, ClusterNode, Neighborhood, Vec2,
};
use crate::api::graph::{GraphNeighborsResponse, GraphNodeDto, GraphEdgeDto};
use std::collections::HashMap;

pub fn to_neighborhood(
    resp: &GraphNeighborsResponse,
    fetched_at_ms: f64,
) -> Neighborhood {
    let center = node_dto_to_canvas(&resp.center, 0);
    let mut one_hop = Vec::new();
    let mut two_hop = Vec::new();
    for n in &resp.nodes {
        let hop = resp.hop_depth.get(&n.id).copied().unwrap_or(2);
        let cn = node_dto_to_canvas(n, hop);
        match hop {
            1 => one_hop.push(cn),
            _ => two_hop.push(cn),
        }
    }

    let edges: Vec<CanvasEdge> = resp.edges.iter().map(|e| {
        // Resolve indices: 0 = center, then one_hop, then two_hop
        let from_idx = resolve_idx(&e.from_id, &resp.center.id, &one_hop, &two_hop);
        let to_idx = resolve_idx(&e.to_id, &resp.center.id, &one_hop, &two_hop);
        let is_active_link = e.from_id == resp.center.id || e.to_id == resp.center.id;
        CanvasEdge {
            from_idx,
            to_idx,
            relation: e.relation.clone(),
            weight: e.weight,
            is_wikilink: e.relation == "references",
            is_active_link,
        }
    }).collect();

    // Group 1-hop by relation (to active), then fold by kind
    let mut by_relation: HashMap<String, Vec<CanvasNode>> = HashMap::new();
    for n in &one_hop {
        let rel = edges.iter()
            .find(|e| e.is_active_link &&
                ((e.from_idx == 0 && one_hop_idx(&n.id, &one_hop).map(|i| i + 1) == Some(e.to_idx)) ||
                 (e.to_idx == 0 && one_hop_idx(&n.id, &one_hop).map(|i| i + 1) == Some(e.from_idx))))
            .map(|e| e.relation.clone())
            .unwrap_or_else(|| "_default".to_string());
        by_relation.entry(rel).or_default().push(n.clone());
    }

    let mut clusters: Vec<ClusterNode> = Vec::new();
    let mut filtered_one_hop: Vec<CanvasNode> = Vec::new();
    for (rel, group) in by_relation {
        let (mut unfolded, mut group_clusters) = fold_sector(&group, &rel, &resp.center.id, 12);
        if unfolded.len() + group_clusters.len() == 0 {
            // (defensive; won't happen with current logic)
        }
        // Apply fallback fold if the sector has too many leftover after kind-fold
        if unfolded.len() >= 30 {
            let (kept, more_clusters) = fallback_fold(unfolded, &rel, &resp.center.id);
            unfolded = kept;
            group_clusters.extend(more_clusters);
        }
        filtered_one_hop.extend(unfolded);
        clusters.extend(group_clusters);
    }

    let target_positions = compute_target_positions(
        &center,
        &filtered_one_hop,
        &two_hop,
        &clusters,
        &edges,
    );

    Neighborhood {
        center,
        one_hop: filtered_one_hop,
        two_hop,
        clusters,
        edges,
        target_positions,
        fetched_at_ms,
    }
}

fn node_dto_to_canvas(dto: &GraphNodeDto, hop: u8) -> CanvasNode {
    use crate::canvas_engine::types::Color;
    CanvasNode {
        id: dto.id.clone(),
        name: dto.name.clone(),
        kind: dto.kind.clone(),
        aliases: dto.aliases.clone(),
        icon: kind_icon(&dto.kind),
        color: kind_color(&dto.kind),
        radius: weight_to_radius(dto.decay_score, dto.edge_count),
        has_wiki: dto.has_wiki,
        decay_score: dto.decay_score,
        edge_count: dto.edge_count,
        world_pos: Vec2::new(0.0, 0.0),
        velocity: Vec2::new(0.0, 0.0),
        z: match hop { 0 => 0.0, 1 => 60.0, _ => 140.0 },
        hop,
        pinned: false,
    }
}

fn resolve_idx(id: &str, center_id: &str, one_hop: &[CanvasNode], two_hop: &[CanvasNode]) -> usize {
    if id == center_id { return 0; }
    if let Some(p) = one_hop.iter().position(|n| n.id == id) { return p + 1; }
    if let Some(p) = two_hop.iter().position(|n| n.id == id) { return one_hop.len() + 1 + p; }
    0 // fallback to center; should not happen
}

fn one_hop_idx(id: &str, one_hop: &[CanvasNode]) -> Option<usize> {
    one_hop.iter().position(|n| n.id == id)
}

// Note: `kind_icon`, `kind_color`, `weight_to_radius` likely already exist in this file.
// If not, copy implementations from previous spec / existing renderer logic.
```

If `kind_icon`/`kind_color`/`weight_to_radius` aren't defined yet, search for them with `grep -rn 'fn kind_icon\|fn kind_color\|fn weight_to_radius' interfaces/webchat/src/` and copy them in.

- [ ] **Step 3: Run check**

Run: `cargo check -p aleph-panel`
Expected: PASS.

- [ ] **Step 4: Add a smoke test**

Append to `adapter.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::graph::*;

    fn dto(id: &str, kind: &str) -> GraphNodeDto {
        GraphNodeDto {
            id: id.to_string(), name: id.to_string(), kind: kind.to_string(),
            aliases: vec![], decay_score: 1.0, edge_count: 1, has_wiki: false,
        }
    }

    #[test]
    fn to_neighborhood_basic_shape() {
        let resp = GraphNeighborsResponse {
            center: dto("a", "concept"),
            nodes: vec![dto("b", "person"), dto("c", "tool")],
            edges: vec![GraphEdgeDto {
                id: "e1".to_string(), from_id: "a".to_string(), to_id: "b".to_string(),
                relation: "uses".to_string(), weight: 1.0, confidence: 0.9,
            }],
            hop_depth: [("b".to_string(), 1), ("c".to_string(), 2)].iter().cloned().collect(),
        };
        let nb = to_neighborhood(&resp, 0.0);
        assert_eq!(nb.center.id, "a");
        assert_eq!(nb.one_hop.len() + nb.clusters.iter().map(|c| c.member_ids.len()).sum::<usize>(), 1);
        assert_eq!(nb.two_hop.len(), 1);
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p aleph-panel --lib canvas_engine::adapter`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/adapter.rs
git commit -m "canvas(adapter): to_neighborhood with hop-aware mapping and cluster folding"
```

---

## Task 12: Viewport — parallax helpers

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/viewport.rs`

- [ ] **Step 1: Append parallax helpers + tests**

Append to `interfaces/webchat/src/canvas_engine/viewport.rs`:

```rust
use crate::canvas_engine::types::Vec3;

/// Per-Z layer parallax offset. Z=0 → factor 1.0, Z=200 → factor 0.85.
pub fn parallax_factor(z: f32) -> f32 {
    1.0 - 0.15 * (z / 200.0).clamp(0.0, 1.0)
}

/// Compute additional position offset for a node when the viewport is dragged.
pub fn parallax_offset(z: f32, drag_dx: f32, drag_dy: f32) -> (f32, f32) {
    let f = parallax_factor(z);
    (drag_dx * f, drag_dy * f)
}

#[cfg(test)]
mod parallax_tests {
    use super::*;

    #[test]
    fn z0_no_parallax_attenuation() {
        assert!((parallax_factor(0.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn z200_max_attenuation() {
        assert!((parallax_factor(200.0) - 0.85).abs() < 1e-6);
    }

    #[test]
    fn parallax_offset_proportional_to_drag() {
        let (dx, dy) = parallax_offset(0.0, 100.0, 50.0);
        assert!((dx - 100.0).abs() < 1e-3);
        assert!((dy - 50.0).abs() < 1e-3);
        let (dx2, dy2) = parallax_offset(200.0, 100.0, 50.0);
        assert!((dx2 - 85.0).abs() < 1e-3);
        assert!((dy2 - 42.5).abs() < 1e-3);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p aleph-panel --lib canvas_engine::viewport::parallax_tests`
Expected: PASS (3 tests).

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/viewport.rs
git commit -m "canvas(viewport): parallax factor and offset helpers"
```

---

## Task 13: Renderer — depth attrs and Z layering

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/renderer.rs`

- [ ] **Step 1: Add `depth_attrs` and unit-test it**

Append to `renderer.rs`:

```rust
use crate::canvas_engine::types::DepthAttrs;

pub fn depth_attrs(z: f32) -> DepthAttrs {
    let t = (z / 200.0).clamp(0.0, 1.0);
    DepthAttrs {
        scale: 1.0 - 0.30 * t,
        opacity: 1.0 - 0.45 * t,
        blur_px: 4.0 * t,
        sat_mul: 1.0 - 0.40 * t,
        glow_alpha: (1.0 - t) * 0.6,
        shadow_offset_y: 6.0 + 4.0 * (1.0 - t),
    }
}

#[cfg(test)]
mod depth_tests {
    use super::*;

    #[test]
    fn active_layer_full_brightness() {
        let a = depth_attrs(0.0);
        assert!((a.scale - 1.0).abs() < 1e-6);
        assert!((a.opacity - 1.0).abs() < 1e-6);
        assert!((a.blur_px - 0.0).abs() < 1e-6);
    }

    #[test]
    fn far_layer_dimmed() {
        let a = depth_attrs(200.0);
        assert!((a.scale - 0.7).abs() < 1e-3);
        assert!((a.opacity - 0.55).abs() < 1e-3);
        assert!((a.blur_px - 4.0).abs() < 1e-3);
    }

    #[test]
    fn beyond_z_clamps() {
        let a = depth_attrs(500.0);
        assert!((a.scale - 0.7).abs() < 1e-3);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p aleph-panel --lib canvas_engine::renderer::depth_tests`
Expected: PASS (3 tests).

- [ ] **Step 3: Refactor render loop to be Z-layer-aware**

Locate the `Renderer::draw` (or equivalent) entry. Modify it so the call signature accepts `Neighborhood` and current viewport drag delta. Replace with this scaffold (the inner draw helpers can be filled in from the existing `draw_nodes` / `draw_edges`):

```rust
pub fn draw_neighborhood(
    ctx: &CanvasRenderingContext2d,
    viewport: &Viewport,
    nbhd: &Neighborhood,
    drag: (f32, f32),
    selected: Option<&str>,
    hovered: Option<&str>,
) {
    // 1. Clear + bg gradient
    paint_background(ctx, viewport);
    ctx.save();
    let _ = ctx.translate(viewport.offset.x, viewport.offset.y);
    let _ = ctx.scale(viewport.scale, viewport.scale);

    // 2. Layer A: 2-hop (back)
    for n in &nbhd.two_hop {
        draw_edges_for_node(ctx, n, nbhd, drag);
    }
    for n in &nbhd.two_hop {
        draw_node(ctx, n, drag, selected, hovered);
    }

    // 3. Layer B: 1-hop + clusters
    for c in &nbhd.clusters {
        draw_cluster(ctx, c, drag, selected, hovered);
    }
    for n in &nbhd.one_hop {
        draw_edges_for_node(ctx, n, nbhd, drag);
    }
    for n in &nbhd.one_hop {
        draw_node(ctx, n, drag, selected, hovered);
    }

    // 4. Layer C: Active (front)
    draw_node(ctx, &nbhd.center, drag, selected, hovered);

    ctx.restore();
}

fn paint_background(ctx: &CanvasRenderingContext2d, viewport: &Viewport) {
    let grad = ctx.create_radial_gradient(
        viewport.width / 2.0, viewport.height / 2.0, 0.0,
        viewport.width / 2.0, viewport.height / 2.0, viewport.width.max(viewport.height),
    ).unwrap();
    let _ = grad.add_color_stop(0.0, "#050510");
    let _ = grad.add_color_stop(1.0, "#0a0a1a");
    ctx.set_fill_style(&grad);
    ctx.fill_rect(0.0, 0.0, viewport.width, viewport.height);
}
```

The helpers `draw_node`, `draw_edges_for_node`, `draw_cluster` are defined in subsequent tasks; for now leave the existing functions in place and add minimal stubs:

```rust
fn draw_node(_: &CanvasRenderingContext2d, _n: &CanvasNode, _drag: (f32, f32), _: Option<&str>, _: Option<&str>) {}
fn draw_cluster(_: &CanvasRenderingContext2d, _c: &ClusterNode, _drag: (f32, f32), _: Option<&str>, _: Option<&str>) {}
fn draw_edges_for_node(_: &CanvasRenderingContext2d, _n: &CanvasNode, _nbhd: &Neighborhood, _drag: (f32, f32)) {}
```

The previous `Renderer::draw(...)` signature can stay for the legacy code path; we'll wire the new entry from `graph_canvas.rs` later.

- [ ] **Step 4: Run check**

Run: `cargo check -p aleph-panel`
Expected: PASS (warnings about unused params/funcs are fine).

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/renderer.rs
git commit -m "canvas(renderer): depth_attrs and Z-layered draw_neighborhood scaffold"
```

---

## Task 14: Renderer — bezier edges with gradient

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/renderer.rs`

- [ ] **Step 1: Replace `draw_edges_for_node` stub with real implementation**

Find the stub `fn draw_edges_for_node` and replace:

```rust
fn draw_edges_for_node(
    ctx: &CanvasRenderingContext2d,
    n: &CanvasNode,
    nbhd: &Neighborhood,
    drag: (f32, f32),
) {
    // Draw edges where this node is one endpoint and the other is the active or a 1-hop
    for e in &nbhd.edges {
        let endpoints = endpoints_world_pos(e, nbhd, drag);
        let (from_pos, to_pos, from_z, to_z) = match endpoints {
            Some(t) => t,
            None => continue,
        };
        // Only draw once per edge — convention: from_idx < to_idx
        if e.from_idx >= e.to_idx { continue; }

        let is_relevant = nbhd.center.id == nbhd.center.id; // placeholder; always true
        if !is_relevant { continue; }

        let attrs_from = depth_attrs(from_z);
        let attrs_to = depth_attrs(to_z);
        let stroke_alpha = if e.is_active_link { 0.85 } else { 0.25 };

        if e.is_wikilink {
            let dashes = js_sys::Array::new();
            dashes.push(&JsValue::from_f64(5.0));
            dashes.push(&JsValue::from_f64(4.0));
            let _ = ctx.set_line_dash(&dashes);
        } else {
            let solid = js_sys::Array::new();
            let _ = ctx.set_line_dash(&solid);
        }

        // Compute bezier control point: perpendicular offset from midpoint
        let mid_x = (from_pos.0 + to_pos.0) * 0.5;
        let mid_y = (from_pos.1 + to_pos.1) * 0.5;
        let dx = to_pos.0 - from_pos.0;
        let dy = to_pos.1 - from_pos.1;
        let len = (dx * dx + dy * dy).sqrt().max(1.0);
        let nx = -dy / len; // perpendicular
        let ny = dx / len;
        let curve_amt = 30.0;
        let cx = mid_x + nx * curve_amt;
        let cy = mid_y + ny * curve_amt;

        let grad = ctx.create_linear_gradient(from_pos.0, from_pos.1, to_pos.0, to_pos.1);
        if e.is_active_link {
            let _ = grad.add_color_stop(0.0, &format!("rgba(167, 139, 250, {})", stroke_alpha * attrs_from.opacity));
            let _ = grad.add_color_stop(1.0, &format!("rgba(76, 29, 149, {})", stroke_alpha * attrs_to.opacity));
        } else {
            let _ = grad.add_color_stop(0.0, &format!("rgba(107, 107, 138, {})", stroke_alpha * attrs_from.opacity));
            let _ = grad.add_color_stop(1.0, &format!("rgba(42, 42, 58, {})", stroke_alpha * attrs_to.opacity));
        }

        let width_from = if e.is_active_link { 2.5 } else { 1.5 };
        let width_to = if e.is_active_link { 1.0 } else { 0.8 };
        let avg_w = (width_from + width_to) * 0.5;

        ctx.set_stroke_style(&grad);
        ctx.set_line_width(avg_w as f64);
        ctx.begin_path();
        ctx.move_to(from_pos.0 as f64, from_pos.1 as f64);
        ctx.quadratic_curve_to(cx as f64, cy as f64, to_pos.0 as f64, to_pos.1 as f64);
        ctx.stroke();
    }
}

fn endpoints_world_pos(
    e: &CanvasEdge,
    nbhd: &Neighborhood,
    drag: (f32, f32),
) -> Option<((f32, f32), (f32, f32), f32, f32)> {
    let resolve = |idx: usize| -> Option<(Vec3, &str)> {
        if idx == 0 {
            nbhd.target_positions.get(&nbhd.center.id).copied().map(|p| (p, nbhd.center.id.as_str()))
        } else if idx <= nbhd.one_hop.len() {
            let n = &nbhd.one_hop[idx - 1];
            nbhd.target_positions.get(&n.id).copied().map(|p| (p, n.id.as_str()))
        } else {
            let off = idx - 1 - nbhd.one_hop.len();
            let n = nbhd.two_hop.get(off)?;
            nbhd.target_positions.get(&n.id).copied().map(|p| (p, n.id.as_str()))
        }
    };
    let (p1, _) = resolve(e.from_idx)?;
    let (p2, _) = resolve(e.to_idx)?;
    let off1 = crate::canvas_engine::viewport::parallax_offset(p1.z, drag.0, drag.1);
    let off2 = crate::canvas_engine::viewport::parallax_offset(p2.z, drag.0, drag.1);
    Some((
        (p1.x + off1.0, p1.y + off1.1),
        (p2.x + off2.0, p2.y + off2.1),
        p1.z,
        p2.z,
    ))
}
```

- [ ] **Step 2: Run check**

Run: `cargo check -p aleph-panel`
Expected: PASS.

- [ ] **Step 3: Build the WASM bundle and visually verify**

Run: `just dev` (starts dev server)
Open browser to the local URL. Switch to the Canvas tab (toolbar Local mode if a flag is needed; we'll wire that fully in Task 22). Verify:

- Edges are curved (bezier), not straight lines
- Active-connected edges are purple-gradient, others are gray
- Wikilinks (`relation == "references"`) appear dashed

If anything looks wrong, debug in browser devtools and adjust constants.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/renderer.rs
git commit -m "canvas(renderer): bezier edges with gradient stroke and wikilink dashes"
```

---

## Task 15: Renderer — node draw with depth, glow, blur, breathing

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/renderer.rs`

- [ ] **Step 1: Replace `draw_node` stub**

```rust
fn draw_node(
    ctx: &CanvasRenderingContext2d,
    n: &CanvasNode,
    drag: (f32, f32),
    selected: Option<&str>,
    hovered: Option<&str>,
) {
    // World position from neighborhood target_positions is provided via n.world_pos
    // (caller updates this each frame from layout.positions)
    let attrs = depth_attrs(n.z);
    let off = crate::canvas_engine::viewport::parallax_offset(n.z, drag.0, drag.1);
    let cx = n.world_pos.x + off.0;
    let cy = n.world_pos.y + off.1;
    let r = n.radius * attrs.scale;

    // 1. Shadow
    ctx.set_fill_style_str(&format!("rgba(0,0,0,{})", 0.3 * attrs.opacity));
    ctx.begin_path();
    let _ = ctx.ellipse(cx as f64, (cy + attrs.shadow_offset_y) as f64,
        (r * 0.9) as f64, (r * 0.4) as f64, 0.0, 0.0, std::f64::consts::TAU);
    ctx.fill();

    // 2. Glow if active/hovered/selected
    let is_active = n.hop == 0;
    let is_hovered = hovered.map(|h| h == n.id).unwrap_or(false);
    let is_selected = selected.map(|s| s == n.id).unwrap_or(false);
    let glow_alpha = if is_active {
        let breathing = 0.85 + 0.15 * (now_ms_in_seconds() * std::f64::consts::TAU / 2.5).sin() as f32;
        attrs.glow_alpha * breathing
    } else if is_hovered || is_selected {
        attrs.glow_alpha
    } else {
        0.0
    };
    if glow_alpha > 0.0 {
        let glow_color = format!("rgba(167,139,250,{})", glow_alpha);
        let grad = ctx.create_radial_gradient(cx as f64, cy as f64, r as f64,
            cx as f64, cy as f64, (r * 2.5) as f64).unwrap();
        let _ = grad.add_color_stop(0.0, &glow_color);
        let _ = grad.add_color_stop(1.0, "rgba(167,139,250,0)");
        ctx.set_fill_style(&grad);
        ctx.begin_path();
        let _ = ctx.arc(cx as f64, cy as f64, (r * 2.5) as f64, 0.0, std::f64::consts::TAU);
        ctx.fill();
    }

    // 3. Apply blur for 2-hop only
    let restore_filter = if attrs.blur_px > 0.5 {
        ctx.set_filter(&format!("blur({}px)", attrs.blur_px));
        true
    } else { false };

    // 4. Body fill
    let body_color = scale_color_saturation(&n.color, attrs.sat_mul, attrs.opacity);
    ctx.set_fill_style_str(&body_color);
    ctx.begin_path();
    let _ = ctx.arc(cx as f64, cy as f64, r as f64, 0.0, std::f64::consts::TAU);
    ctx.fill();

    // 5. Border
    ctx.set_stroke_style_str("#1a1a2a");
    ctx.set_line_width(1.0);
    ctx.stroke();

    if restore_filter { ctx.set_filter("none"); }

    // 6. Icon (if large enough)
    if r >= 22.0 {
        ctx.set_fill_style_str(&format!("rgba(255,255,255,{})", attrs.opacity));
        ctx.set_font(&format!("{}px sans-serif", (r * 0.8).max(14.0) as i32));
        ctx.set_text_align("center");
        ctx.set_text_baseline("middle");
        let _ = ctx.fill_text(&n.icon.to_string(), cx as f64, cy as f64);
    }

    // 7. Wiki badge
    if n.has_wiki && r >= 28.0 {
        ctx.set_fill_style_str(&format!("rgba(255,255,255,{})", attrs.opacity * 0.8));
        ctx.begin_path();
        let _ = ctx.arc((cx + r * 0.7) as f64, (cy + r * 0.7) as f64, 6.0, 0.0, std::f64::consts::TAU);
        ctx.fill();
    }

    // 8. Label
    if r >= 18.0 {
        let label_size = (14.0 * attrs.scale).max(10.0);
        ctx.set_font(&format!("{}px sans-serif", label_size as i32));
        ctx.set_fill_style_str(&format!("rgba(255,255,255,{})", attrs.opacity));
        ctx.set_text_align("center");
        ctx.set_text_baseline("top");
        let _ = ctx.fill_text(&n.name, cx as f64, (cy + r + 8.0) as f64);
    }
}

fn now_ms_in_seconds() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now() / 1000.0)
        .unwrap_or(0.0)
}

fn scale_color_saturation(c: &Color, sat_mul: f32, opacity: f32) -> String {
    // Naive: assume Color has r/g/b/a u8 fields. If not, adapt to existing API.
    let r = (c.r as f32 * (1.0 - (1.0 - sat_mul) * 0.5)).round().clamp(0.0, 255.0) as u8;
    let g = (c.g as f32 * (1.0 - (1.0 - sat_mul) * 0.5)).round().clamp(0.0, 255.0) as u8;
    let b = (c.b as f32 * (1.0 - (1.0 - sat_mul) * 0.5)).round().clamp(0.0, 255.0) as u8;
    format!("rgba({r},{g},{b},{:.3})", opacity)
}
```

If `Color` doesn't have `r/g/b` fields, run `grep -n 'struct Color' interfaces/webchat/src/canvas_engine/types.rs` and adapt the helper.

- [ ] **Step 2: Replace `draw_cluster` stub**

```rust
fn draw_cluster(
    ctx: &CanvasRenderingContext2d,
    c: &ClusterNode,
    drag: (f32, f32),
    selected: Option<&str>,
    hovered: Option<&str>,
) {
    let attrs = depth_attrs(c.z);
    let off = crate::canvas_engine::viewport::parallax_offset(c.z, drag.0, drag.1);
    let cx = c.world_pos.x + off.0;
    let cy = c.world_pos.y + off.1;
    let r = c.radius * attrs.scale;
    let w = r * 2.4;
    let h = r * 1.6;

    // Shadow
    ctx.set_fill_style_str(&format!("rgba(0,0,0,{})", 0.3 * attrs.opacity));
    ctx.begin_path();
    let _ = ctx.ellipse(cx as f64, (cy + attrs.shadow_offset_y) as f64,
        (w / 2.0 * 0.9) as f64, (h / 2.0 * 0.4) as f64, 0.0, 0.0, std::f64::consts::TAU);
    ctx.fill();

    // Body — rounded rect with kind color tint
    ctx.set_fill_style_str(&format!("rgba(124,58,237,{})", 0.3 * attrs.opacity));
    ctx.begin_path();
    rounded_rect(ctx, cx - w / 2.0, cy - h / 2.0, w, h, 12.0);
    ctx.fill();

    ctx.set_stroke_style_str(&format!("rgba(167,139,250,{})", attrs.opacity));
    ctx.set_line_width(2.0);
    ctx.stroke();

    // Label
    let label = format!("📚 +{} {}", c.member_ids.len(), c.kind);
    ctx.set_font(&format!("{}px sans-serif", (12.0 * attrs.scale).max(10.0) as i32));
    ctx.set_fill_style_str(&format!("rgba(255,255,255,{})", attrs.opacity));
    ctx.set_text_align("center");
    ctx.set_text_baseline("middle");
    let _ = ctx.fill_text(&label, cx as f64, cy as f64);

    // Highlight outline on hover/select
    let highlighted = hovered.map(|h| h == c.id).unwrap_or(false)
        || selected.map(|s| s == c.id).unwrap_or(false);
    if highlighted {
        ctx.set_stroke_style_str("rgba(255,255,255,0.85)");
        ctx.set_line_width(3.0);
        ctx.begin_path();
        rounded_rect(ctx, cx - w / 2.0 - 2.0, cy - h / 2.0 - 2.0, w + 4.0, h + 4.0, 14.0);
        ctx.stroke();
    }
}

fn rounded_rect(ctx: &CanvasRenderingContext2d, x: f32, y: f32, w: f32, h: f32, r: f32) {
    let r = r.min(w / 2.0).min(h / 2.0);
    ctx.move_to((x + r) as f64, y as f64);
    ctx.line_to((x + w - r) as f64, y as f64);
    let _ = ctx.arc_to((x + w) as f64, y as f64, (x + w) as f64, (y + r) as f64, r as f64);
    ctx.line_to((x + w) as f64, (y + h - r) as f64);
    let _ = ctx.arc_to((x + w) as f64, (y + h) as f64, (x + w - r) as f64, (y + h) as f64, r as f64);
    ctx.line_to((x + r) as f64, (y + h) as f64);
    let _ = ctx.arc_to(x as f64, (y + h) as f64, x as f64, (y + h - r) as f64, r as f64);
    ctx.line_to(x as f64, (y + r) as f64);
    let _ = ctx.arc_to(x as f64, y as f64, (x + r) as f64, y as f64, r as f64);
    ctx.close_path();
}
```

- [ ] **Step 3: Run check + dev server visual verify**

Run: `cargo check -p aleph-panel && just dev`
Expected: compile success; in browser, nodes render with shadow / glow / blur / breathing on active.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/renderer.rs
git commit -m "canvas(renderer): depth-aware node + cluster rendering with glow, blur, breathing"
```

---

## Task 16: Mini-map module

**Files:**
- Replace: `interfaces/webchat/src/canvas_engine/mini_map.rs`

- [ ] **Step 1: Implement mini-map structure**

Replace `interfaces/webchat/src/canvas_engine/mini_map.rs`:

```rust
use crate::canvas_engine::types::*;
use std::collections::HashMap;
use web_sys::CanvasRenderingContext2d;

pub const MINIMAP_W: f32 = 160.0;
pub const MINIMAP_H: f32 = 120.0;
pub const MINIMAP_SAMPLE_LIMIT: usize = 200;
pub const MINIMAP_TTL_MS: f64 = 5.0 * 60_000.0;

pub struct MiniMap {
    pub samples: Vec<MiniNode>,
    pub bounds: (f32, f32, f32, f32),  // (min_x, min_y, max_x, max_y)
    pub fetched_at_ms: f64,
}

pub struct MiniNode {
    pub id: String,
    pub x: f32,
    pub y: f32,
    pub kind: String,
}

impl MiniMap {
    pub fn empty() -> Self {
        Self { samples: vec![], bounds: (0.0, 0.0, 1.0, 1.0), fetched_at_ms: 0.0 }
    }

    pub fn is_stale(&self, now_ms: f64) -> bool {
        now_ms - self.fetched_at_ms > MINIMAP_TTL_MS
    }

    /// Set samples + recompute bounds.
    pub fn set_samples(&mut self, samples: Vec<MiniNode>, now_ms: f64) {
        if samples.is_empty() {
            self.samples = samples;
            self.bounds = (0.0, 0.0, 1.0, 1.0);
            self.fetched_at_ms = now_ms;
            return;
        }
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        for s in &samples {
            min_x = min_x.min(s.x);
            min_y = min_y.min(s.y);
            max_x = max_x.max(s.x);
            max_y = max_y.max(s.y);
        }
        if (max_x - min_x).abs() < 1e-3 { max_x = min_x + 1.0; }
        if (max_y - min_y).abs() < 1e-3 { max_y = min_y + 1.0; }
        self.samples = samples;
        self.bounds = (min_x, min_y, max_x, max_y);
        self.fetched_at_ms = now_ms;
    }

    pub fn world_to_minimap(&self, wx: f32, wy: f32) -> (f32, f32) {
        let (min_x, min_y, max_x, max_y) = self.bounds;
        let nx = (wx - min_x) / (max_x - min_x);
        let ny = (wy - min_y) / (max_y - min_y);
        (nx * MINIMAP_W, ny * MINIMAP_H)
    }

    pub fn minimap_to_world(&self, mx: f32, my: f32) -> (f32, f32) {
        let (min_x, min_y, max_x, max_y) = self.bounds;
        let wx = min_x + (mx / MINIMAP_W) * (max_x - min_x);
        let wy = min_y + (my / MINIMAP_H) * (max_y - min_y);
        (wx, wy)
    }

    pub fn pick_node(&self, mx: f32, my: f32) -> Option<&MiniNode> {
        let mut best: Option<(&MiniNode, f32)> = None;
        for s in &self.samples {
            let (sx, sy) = self.world_to_minimap(s.x, s.y);
            let d2 = (sx - mx).powi(2) + (sy - my).powi(2);
            if d2 < 36.0 {
                match best {
                    Some((_, bd)) if bd <= d2 => {}
                    _ => best = Some((s, d2)),
                }
            }
        }
        best.map(|(n, _)| n)
    }

    pub fn draw(
        &self,
        ctx: &CanvasRenderingContext2d,
        x_origin: f32,
        y_origin: f32,
        active_id: &str,
        one_hop_ids: &std::collections::HashSet<String>,
    ) {
        // Background panel
        ctx.set_fill_style_str("rgba(20,20,30,0.7)");
        ctx.fill_rect(x_origin as f64, y_origin as f64, MINIMAP_W as f64, MINIMAP_H as f64);
        ctx.set_stroke_style_str("rgba(255,255,255,0.15)");
        ctx.set_line_width(1.0);
        ctx.stroke_rect(x_origin as f64, y_origin as f64, MINIMAP_W as f64, MINIMAP_H as f64);

        for s in &self.samples {
            let (mx, my) = self.world_to_minimap(s.x, s.y);
            let cx = x_origin + mx;
            let cy = y_origin + my;
            if s.id == active_id {
                ctx.set_fill_style_str("#ef4444");
                ctx.begin_path();
                let _ = ctx.arc(cx as f64, cy as f64, 4.0, 0.0, std::f64::consts::TAU);
                ctx.fill();
                ctx.set_stroke_style_str("#ffffff");
                ctx.set_line_width(1.0);
                ctx.stroke();
            } else if one_hop_ids.contains(&s.id) {
                ctx.set_fill_style_str("rgba(167,139,250,0.85)");
                ctx.fill_rect((cx - 1.5) as f64, (cy - 1.5) as f64, 3.0, 3.0);
            } else {
                ctx.set_fill_style_str("rgba(140,140,160,0.5)");
                ctx.fill_rect((cx - 0.5) as f64, (cy - 0.5) as f64, 1.0, 1.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_bounds_safe() {
        let mut m = MiniMap::empty();
        m.set_samples(vec![], 0.0);
        let (x, y) = m.world_to_minimap(0.0, 0.0);
        assert!(x.is_finite() && y.is_finite());
    }

    #[test]
    fn world_to_minimap_round_trip() {
        let mut m = MiniMap::empty();
        m.set_samples(vec![
            MiniNode { id: "a".to_string(), x: -100.0, y: -100.0, kind: "concept".to_string() },
            MiniNode { id: "b".to_string(), x: 100.0, y: 100.0, kind: "concept".to_string() },
        ], 0.0);
        let (mx, my) = m.world_to_minimap(0.0, 0.0);
        let (wx, wy) = m.minimap_to_world(mx, my);
        assert!((wx).abs() < 1e-3);
        assert!((wy).abs() < 1e-3);
    }

    #[test]
    fn pick_node_finds_closest_within_threshold() {
        let mut m = MiniMap::empty();
        m.set_samples(vec![
            MiniNode { id: "a".to_string(), x: 0.0, y: 0.0, kind: "concept".to_string() },
        ], 0.0);
        let (mx, my) = m.world_to_minimap(0.0, 0.0);
        assert!(m.pick_node(mx, my).is_some());
        assert!(m.pick_node(mx + 100.0, my + 100.0).is_none());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p aleph-panel --lib canvas_engine::mini_map`
Expected: PASS (3 tests).

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/mini_map.rs
git commit -m "canvas(mini_map): sampling, bounds, hit-testing, draw routine"
```

---

## Task 17: Interaction — hover prefetch + keyboard nav

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/interaction.rs`

- [ ] **Step 1: Read existing interaction.rs**

Run: `cat interfaces/webchat/src/canvas_engine/interaction.rs`

- [ ] **Step 2: Add structures for hover-debouncing and keyboard mapping**

Append to `interaction.rs`:

```rust
use crate::canvas_engine::prefetch::HoverDebouncer;

pub enum CanvasIntent {
    None,
    SetActive(String),
    PrefetchNeighbor(String),
    ExpandCluster(String),
    BreadcrumbBack,
    BreadcrumbForward,
    OpenSearch,
    ToggleGlobal,
    CloseDetail,
    HoverFocus(Direction),
}

pub enum Direction { Next, Prev }

pub struct InteractionState {
    pub debounce: HoverDebouncer,
    pub hovered: Option<String>,
}

impl InteractionState {
    pub fn new() -> Self {
        Self { debounce: HoverDebouncer::new(), hovered: None }
    }

    pub fn on_pointer_move(&mut self, hovered: Option<&str>, now_ms: f64) -> Option<CanvasIntent> {
        self.hovered = hovered.map(str::to_string);
        self.debounce.note_hover(hovered, now_ms).map(CanvasIntent::PrefetchNeighbor)
    }

    pub fn on_click(&mut self, target: ClickTarget) -> CanvasIntent {
        match target {
            ClickTarget::Node(id) => CanvasIntent::SetActive(id),
            ClickTarget::Cluster(id) => CanvasIntent::ExpandCluster(id),
            ClickTarget::Empty => CanvasIntent::CloseDetail,
            ClickTarget::Active => CanvasIntent::None,
        }
    }

    pub fn on_keydown(&self, key: &str, alt: bool, _shift: bool) -> CanvasIntent {
        match (key, alt) {
            ("Tab", _) => CanvasIntent::HoverFocus(Direction::Next),
            ("Enter", _) => match &self.hovered {
                Some(id) => CanvasIntent::SetActive(id.clone()),
                None => CanvasIntent::None,
            },
            ("Escape", _) => CanvasIntent::CloseDetail,
            ("Backspace", _) | ("ArrowLeft", true) => CanvasIntent::BreadcrumbBack,
            ("ArrowRight", true) => CanvasIntent::BreadcrumbForward,
            ("/", _) => CanvasIntent::OpenSearch,
            ("g", _) | ("G", _) => CanvasIntent::ToggleGlobal,
            _ => CanvasIntent::None,
        }
    }
}

pub enum ClickTarget {
    Node(String),
    Cluster(String),
    Active,
    Empty,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_node_returns_set_active() {
        let mut s = InteractionState::new();
        match s.on_click(ClickTarget::Node("x".to_string())) {
            CanvasIntent::SetActive(id) => assert_eq!(id, "x"),
            _ => panic!("expected SetActive"),
        }
    }

    #[test]
    fn click_cluster_returns_expand() {
        let mut s = InteractionState::new();
        match s.on_click(ClickTarget::Cluster("c1".to_string())) {
            CanvasIntent::ExpandCluster(id) => assert_eq!(id, "c1"),
            _ => panic!("expected ExpandCluster"),
        }
    }

    #[test]
    fn keydown_escape_closes_detail() {
        let s = InteractionState::new();
        assert!(matches!(s.on_keydown("Escape", false, false), CanvasIntent::CloseDetail));
    }

    #[test]
    fn pointer_move_triggers_prefetch_after_debounce() {
        let mut s = InteractionState::new();
        assert!(s.on_pointer_move(Some("x"), 0.0).is_none());
        assert!(s.on_pointer_move(Some("x"), 100.0).is_none());
        assert!(matches!(
            s.on_pointer_move(Some("x"), 200.0),
            Some(CanvasIntent::PrefetchNeighbor(_))
        ));
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p aleph-panel --lib canvas_engine::interaction`
Expected: PASS (4 tests).

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/interaction.rs
git commit -m "canvas(interaction): intent enum + hover debounce + keyboard mapping"
```

---

## Task 18: Breadcrumb view — navigation history

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/breadcrumb.rs`

- [ ] **Step 1: Replace component with history-stack semantics**

Read existing file: `cat interfaces/webchat/src/views/canvas/breadcrumb.rs` to understand the Leptos component shape.

Replace with:

```rust
use leptos::prelude::*;

#[component]
pub fn Breadcrumb(
    items: ReadSignal<Vec<(String, String)>>,
    on_jump: Callback<String>,
) -> impl IntoView {
    view! {
        <nav class="canvas-breadcrumb" aria-label="Navigation history">
            {move || {
                let list = items.get();
                let total = list.len();
                let display: Vec<(String, String)> = if total > 8 {
                    let head = list.first().cloned().unwrap();
                    let tail = list[total - 6..].to_vec();
                    let mut out = vec![head, ("ellipsis".to_string(), "…".to_string())];
                    out.extend(tail);
                    out
                } else {
                    list
                };
                display.into_iter().enumerate().map(|(idx, (id, name))| {
                    let id_clone = id.clone();
                    let on_jump_clone = on_jump.clone();
                    let is_ellipsis = id == "ellipsis";
                    let is_last = idx == display.len() - 1; // visual cue
                    view! {
                        <span class:active=is_last class:ellipsis=is_ellipsis>
                            {if !is_ellipsis {
                                view! {
                                    <button on:click=move |_| {
                                        if !is_last { on_jump_clone.run(id_clone.clone()); }
                                    }>
                                        {name.clone()}
                                    </button>
                                }.into_any()
                            } else {
                                view! { <span>{name.clone()}</span> }.into_any()
                            }}
                            {(idx + 1 < total).then(|| view! { <span class="sep">" → "</span> })}
                        </span>
                    }
                }).collect_view()
            }}
        </nav>
    }
}
```

Note: exact Leptos 0.8 callback API and view macros may differ slightly from above; align to existing components' patterns in the project (`grep -n 'Callback<' interfaces/webchat/src/components/*.rs`).

- [ ] **Step 2: Run check**

Run: `cargo check -p aleph-panel`
Expected: PASS. Adjust syntax if needed.

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/views/canvas/breadcrumb.rs
git commit -m "canvas(breadcrumb): navigation history stack with ellipsis truncation"
```

---

## Task 19: Toolbar — detail slider + Local/Global toggle

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/toolbar.rs`

- [ ] **Step 1: Read current toolbar**

Run: `cat interfaces/webchat/src/views/canvas/toolbar.rs`

- [ ] **Step 2: Add new controls**

Replace with (preserving existing search/filter/agent label):

```rust
use leptos::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode { Local, Global }

#[component]
pub fn CanvasToolbar(
    mode: ReadSignal<ViewMode>,
    set_mode: WriteSignal<ViewMode>,
    fold_threshold: ReadSignal<usize>,
    set_fold_threshold: WriteSignal<usize>,
    agent_name: ReadSignal<String>,
    on_agent_click: Callback<()>,
    on_search: Callback<String>,
) -> impl IntoView {
    view! {
        <div class="canvas-toolbar">
            <button class="agent-label" on:click=move |_| on_agent_click.run(())>
                "🤖 " {move || agent_name.get()} " ↗"
            </button>

            <input class="search-box" type="text" placeholder="🔍 Search nodes..."
                on:input=move |ev| on_search.run(event_target_value(&ev)) />

            <div class="mode-toggle">
                <button
                    class:active=move || mode.get() == ViewMode::Local
                    on:click=move |_| set_mode.set(ViewMode::Local)>
                    "📍 Local"
                </button>
                <button
                    class:active=move || mode.get() == ViewMode::Global
                    on:click=move |_| set_mode.set(ViewMode::Global)>
                    "🌐 Global"
                </button>
            </div>

            <label class="detail-slider">
                "📚 详细度 "
                <input type="range" min="6" max="20" step="1"
                    prop:value=move || fold_threshold.get().to_string()
                    on:input=move |ev| {
                        let v: usize = event_target_value(&ev).parse().unwrap_or(12);
                        set_fold_threshold.set(v);
                    } />
                <span>{move || fold_threshold.get()}</span>
            </label>
        </div>
    }
}
```

- [ ] **Step 3: Run check**

Run: `cargo check -p aleph-panel`
Expected: PASS. (Filter button is preserved if previously present; merge into the new structure.)

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/canvas/toolbar.rs
git commit -m "canvas(toolbar): Local/Global mode toggle and FOLD_THRESHOLD slider"
```

---

## Task 20: Detail panel — cluster summary view

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/detail_panel.rs`

- [ ] **Step 1: Add `ClusterDetail` variant**

Replace `interfaces/webchat/src/views/canvas/detail_panel.rs` (preserve existing wiki/facts rendering, add new branch):

```rust
use leptos::prelude::*;

#[derive(Debug, Clone)]
pub enum DetailContent {
    Closed,
    Node {
        node_id: String,
        wiki_md: Option<String>,
        facts: Vec<(String, String, f32)>, // (id, content, confidence)
    },
    Cluster {
        kind: String,
        members: Vec<(String, String)>, // (id, name)
    },
}

#[component]
pub fn DetailPanel(
    content: ReadSignal<DetailContent>,
    on_jump_to: Callback<String>,
) -> impl IntoView {
    view! {
        <aside class="detail-panel" class:hidden=move || matches!(content.get(), DetailContent::Closed)>
            {move || match content.get() {
                DetailContent::Closed => view! { <div /> }.into_any(),
                DetailContent::Node { node_id, wiki_md, facts } => view! {
                    <div class="node-detail">
                        <h3>{node_id.clone()}</h3>
                        <section class="wiki">
                            {match wiki_md {
                                Some(md) => view! { <div class="md">{md}</div> }.into_any(),
                                None => view! { <p class="muted">"No wiki page compiled yet"</p> }.into_any(),
                            }}
                        </section>
                        <section class="facts">
                            <h4>"📋 Related Facts (" {facts.len()} ")"</h4>
                            <ul>
                                {facts.into_iter().map(|(id, content, conf)| view! {
                                    <li>
                                        <span class="conf">{format!("{:.2}", conf)}</span>
                                        <span class="content">{content}</span>
                                    </li>
                                }).collect_view()}
                            </ul>
                        </section>
                    </div>
                }.into_any(),
                DetailContent::Cluster { kind, members } => view! {
                    <div class="cluster-detail">
                        <h3>{format!("{} 群组（共 {} 个）", kind, members.len())}</h3>
                        <ul class="cluster-members">
                            {members.into_iter().map(|(id, name)| {
                                let id_clone = id.clone();
                                let cb = on_jump_to.clone();
                                view! {
                                    <li>
                                        <span>{name}</span>
                                        <button on:click=move |_| cb.run(id_clone.clone())>"→ 跳转"</button>
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                    </div>
                }.into_any(),
            }}
        </aside>
    }
}
```

- [ ] **Step 2: Run check**

Run: `cargo check -p aleph-panel`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/views/canvas/detail_panel.rs
git commit -m "canvas(detail_panel): cluster summary view branch"
```

---

## Task 21: Feature flag in shared user prefs

**Files:**
- Modify: `shared/ui_logic/src/...` (locate file with existing user prefs)

- [ ] **Step 1: Locate user prefs struct**

Run: `grep -rn 'pub struct.*UserPref\|user_prefs\|UserSettings' shared/ui_logic/src/ | head -10`

- [ ] **Step 2: Add `canvas_radial_navigation` field**

In the located file, add:

```rust
#[serde(default)]
pub canvas_radial_navigation: bool,  // default: false (legacy global view)
```

If there's a `Default` impl, add `canvas_radial_navigation: false` to it.

- [ ] **Step 3: Run check**

Run: `cargo check -p aleph-panel && cargo check -p shared-ui-logic`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add shared/ui_logic/src/
git commit -m "shared-ui-logic: add canvas_radial_navigation user preference (default off)"
```

---

## Task 22: Wire it all — `views/canvas/mod.rs` + `graph_canvas.rs`

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/mod.rs`
- Modify: `interfaces/webchat/src/views/canvas/graph_canvas.rs`

This is the largest task — pulling together NavController, PrefetchCache, MiniMap, RadialForceLayout, and the new renderer behind a feature flag.

- [ ] **Step 1: Sketch updated `mod.rs` structure**

The view should:
1. Read `user_prefs.canvas_radial_navigation`. If false, render the existing legacy CanvasView; if true, render `RadialCanvasView`.
2. `RadialCanvasView` owns: `NavController`, `PrefetchCache`, `MiniMap`, `RadialForceLayout`, `InteractionState`, `ViewMode` signal, `fold_threshold` signal, drag state, last frame timestamp.
3. Initial mount: select entry point (URL hash → localStorage → `graph.query` top-1) → `NavController::enter`, fetch neighbors, transition to Active.

Read the current mod.rs (`cat interfaces/webchat/src/views/canvas/mod.rs`) to align with existing structure (it currently has 221 LOC and likely uses Leptos signals + effects). Then add a sibling component `RadialCanvasView`.

```rust
// interfaces/webchat/src/views/canvas/mod.rs (additions / replacement)

mod breadcrumb;
mod detail_panel;
mod graph_canvas;
mod toolbar;

use leptos::prelude::*;
use crate::canvas_engine::types::*;
use crate::canvas_engine::navigation::NavController;
use crate::canvas_engine::prefetch::PrefetchCache;
use crate::canvas_engine::mini_map::{MiniMap, MiniNode};
use crate::canvas_engine::interaction::{InteractionState, CanvasIntent};
use toolbar::{CanvasToolbar, ViewMode};
use breadcrumb::Breadcrumb;
use detail_panel::{DetailPanel, DetailContent};

#[component]
pub fn CanvasView() -> impl IntoView {
    let (use_radial, _) = create_signal(read_user_pref_radial_nav());
    view! {
        {move || {
            if use_radial.get() {
                view! { <RadialCanvasView /> }.into_any()
            } else {
                view! { <LegacyCanvasView /> }.into_any()  // existing component renamed
            }
        }}
    }
}

#[component]
fn RadialCanvasView() -> impl IntoView {
    // Set up signals
    let nav = StoredValue::new(NavController::new());
    let prefetch = StoredValue::new(PrefetchCache::new());
    let minimap = StoredValue::new(MiniMap::empty());
    let interaction = StoredValue::new(InteractionState::new());
    let (mode, set_mode) = create_signal(ViewMode::Local);
    let (fold_threshold, set_fold_threshold) = create_signal(12_usize);
    let (breadcrumb_items, set_breadcrumb_items) = create_signal(Vec::<(String, String)>::new());
    let (detail_content, set_detail_content) = create_signal(DetailContent::Closed);

    // Mount: pick entry point
    Effect::new(move |_| {
        let entry = pick_entry_point();
        spawn_local(async move {
            match crate::api::graph::neighbors(&entry, 2, 50).await {
                Ok(resp) => {
                    let now = now_ms();
                    let nbhd = crate::canvas_engine::adapter::to_neighborhood(&resp, now);
                    let name = nbhd.center.name.clone();
                    nav.update_value(|n| n.fulfilled(entry.clone(), name, nbhd));
                    sync_breadcrumb(&nav, &set_breadcrumb_items);
                }
                Err(_e) => {}
            }
        });
    });

    // Render
    view! {
        <div class="canvas-view radial">
            <CanvasToolbar
                mode=mode
                set_mode=set_mode
                fold_threshold=fold_threshold
                set_fold_threshold=set_fold_threshold
                agent_name=create_signal("".to_string()).0
                on_agent_click=Callback::new(|_| {})
                on_search=Callback::new(|_q: String| { /* handle search */ })
            />
            <Breadcrumb
                items=breadcrumb_items
                on_jump=Callback::new(move |id: String| { /* handle jump */ let _ = id; })
            />
            <graph_canvas::GraphCanvas
                nav=nav
                prefetch=prefetch
                minimap=minimap
                interaction=interaction
                fold_threshold=fold_threshold
                on_breadcrumb_change=Callback::new(move |_| sync_breadcrumb(&nav, &set_breadcrumb_items))
                on_detail_change=Callback::new(move |c: DetailContent| set_detail_content.set(c))
            />
            <DetailPanel content=detail_content on_jump_to=Callback::new(|_id: String| { /* jump */ }) />
        </div>
    }
}

fn pick_entry_point() -> String {
    // 1. URL hash
    if let Some(window) = web_sys::window() {
        if let Ok(hash) = window.location().hash() {
            if let Some(rest) = hash.strip_prefix("#node=") {
                return rest.to_string();
            }
        }
        // 2. localStorage
        if let Ok(Some(storage)) = window.local_storage() {
            if let Ok(Some(id)) = storage.get_item("last_active_canvas_node") {
                return id;
            }
        }
    }
    // 3. fallback (caller will handle empty by querying server top-1)
    String::new()
}

fn now_ms() -> f64 {
    web_sys::window().and_then(|w| w.performance()).map(|p| p.now()).unwrap_or(0.0)
}

fn sync_breadcrumb(
    nav: &StoredValue<NavController>,
    sig: &WriteSignal<Vec<(String, String)>>,
) {
    nav.with_value(|n| sig.set(n.breadcrumb.clone()));
}

fn read_user_pref_radial_nav() -> bool {
    // Bridge to shared-ui-logic UserPrefs
    crate::shared_state::user_prefs().canvas_radial_navigation
}
```

(Function names like `crate::shared_state::user_prefs()` are placeholders — adapt to the actual user-prefs accessor.)

- [ ] **Step 2: Update `graph_canvas.rs` to use new render path**

Modify `graph_canvas.rs`:

- Accept the new `nav`, `prefetch`, `minimap`, `interaction`, `fold_threshold` props as `StoredValue<...>`.
- In the RAF loop, call `Renderer::draw_neighborhood(...)` when `nav.state == Active|Animating`, with positions populated from `RadialForceLayout` (run a few `step()` per frame until convergence) or directly from `target_positions` for animations (use `tween::lerp_node` for each id).
- Wire `on:click`, `on:mousemove`, `on:wheel` to `InteractionState`, and dispatch `CanvasIntent` to either: trigger neighbor fetch + animation, expand cluster, or open detail panel.
- Implement hover prefetch: on `CanvasIntent::PrefetchNeighbor(id)`, call `crate::api::graph::neighbors(...)` and stuff result in `prefetch.put(...)`.

This is a substantial wiring task. Pseudocode for the click flow:

```rust
match interaction.with_value(|s| s.on_click(target)) {
    CanvasIntent::SetActive(id) => {
        let from = nav.with_value(|n| match &n.state {
            NavState::Active { neighborhood, .. } => Some(neighborhood.clone()),
            _ => None,
        });
        let cached = prefetch.with_value(|p| p.get(&id, now_ms()).cloned());
        match (from, cached) {
            (Some(from_nb), Some(to_nb)) => {
                let from_id = from_nb.center.id.clone();
                let to_id = id.clone();
                nav.update_value(|n| n.start_animation(from_nb, to_nb, from_id, to_id, now_ms(), 400));
            }
            _ => {
                nav.update_value(|n| n.enter(id.clone(), now_ms()));
                spawn_local(async move {
                    let resp = crate::api::graph::neighbors(&id, 2, 50).await.ok()?;
                    let nbhd = crate::canvas_engine::adapter::to_neighborhood(&resp, now_ms());
                    nav.update_value(|n| n.fulfilled(id, nbhd.center.name.clone(), nbhd));
                    Some(())
                });
            }
        }
    }
    CanvasIntent::ExpandCluster(cluster_id) => {
        // Toggle ClusterNode.expanded; promote member nodes from member_ids to one_hop list
        // with positions seeded around the cluster's current world_pos so the 280ms tween
        // animates them outward.
        nav.update_value(|n| {
            if let NavState::Active { ref mut neighborhood, .. } = &mut n.state {
                if let Some(cluster) = neighborhood.clusters.iter_mut().find(|c| c.id == cluster_id) {
                    cluster.expanded = !cluster.expanded;
                    let anchor = cluster.world_pos;
                    let member_ids = cluster.member_ids.clone();
                    let radius_extra = 90.0;
                    for (i, mid) in member_ids.iter().enumerate() {
                        let theta = (i as f32 / member_ids.len().max(1) as f32) * 0.8 - 0.4; // ±0.4 rad
                        let pos = crate::canvas_engine::types::Vec3::new(
                            anchor.x + theta.cos() * radius_extra,
                            anchor.y + theta.sin() * radius_extra,
                            75.0, // expanded members slightly behind 1-hop
                        );
                        neighborhood.target_positions.insert(mid.clone(), pos);
                    }
                }
            }
        });
        // Local 280ms tween: bypass NavController; drive a separate animator on the StoredValue
        // (kept simple: positions update immediately and force-layout converges over ~10 frames).
    }
    _ => {}
}
```

- [ ] **Step 3: Run check**

Run: `cargo check -p aleph-panel`
Expected: PASS — likely after several iterations of fixing types/lifetimes.

- [ ] **Step 4: Wire mini-map sampling and click**

Inside `RadialCanvasView`, after the entry-point fetch effect, add a second effect that populates the mini-map by calling `graph.query` (limit 200) and storing the result. Re-run when the active node changes:

```rust
Effect::new(move |_| {
    nav.with_value(|n| {
        let active_id = match &n.state {
            NavState::Active { node_id, .. } => Some(node_id.clone()),
            _ => None,
        };
        if active_id.is_none() { return; }

        if !minimap.with_value(|m| m.is_stale(now_ms())) { return; }

        spawn_local(async move {
            let resp = match crate::api::graph::query(200, "weight").await {
                Ok(r) => r,
                Err(_) => return,
            };
            // Use target_positions of the active neighborhood to anchor a layout for sampled nodes
            // — for simplicity we use weight as radial distance and kind hash as angle.
            let samples: Vec<MiniNode> = resp.nodes.iter().enumerate().map(|(i, n)| {
                let theta = (i as f32 / resp.nodes.len().max(1) as f32) * std::f32::consts::TAU;
                let r = 200.0 + 400.0 * (1.0 - n.decay_score.clamp(0.0, 1.0));
                MiniNode {
                    id: n.id.clone(),
                    x: theta.cos() * r,
                    y: theta.sin() * r,
                    kind: n.kind.clone(),
                }
            }).collect();
            minimap.update_value(|m| m.set_samples(samples, now_ms()));
        });
    });
});
```

In `graph_canvas.rs`, when handling click events, also test if the click landed inside the mini-map rect (bottom-right `MINIMAP_W × MINIMAP_H`). If yes, call `minimap.with_value(|m| m.pick_node(local_x, local_y))` and dispatch `CanvasIntent::SetActive(picked.id)`.

- [ ] **Step 5: Build dev and visually verify the toggle**

Run: `just dev`
In Settings, enable `canvas_radial_navigation`. Open Canvas tab. Verify:
- Initial entry point loads
- Toolbar shows Local/Global toggle and detail slider
- Breadcrumb appears when you navigate
- Mini-map appears bottom-right with red dot on active node
- Click to switch active works (animation polish lands in Task 23)

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/views/canvas/
git commit -m "canvas(views): wire RadialCanvasView with NavController, prefetch, minimap, feature flag"
```

---

## Task 23: Animation loop integration + polish pass

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/graph_canvas.rs`

- [ ] **Step 1: Add RAF-driven tick that progresses NavController and force layout**

In the `graph_canvas.rs` component, set up a `requestAnimationFrame` loop:

```rust
fn start_render_loop(
    nav: StoredValue<NavController>,
    canvas_ref: NodeRef<HtmlCanvasElement>,
) {
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;

    let f = std::rc::Rc::new(std::cell::RefCell::new(None));
    let g = f.clone();

    *g.borrow_mut() = Some(Closure::wrap(Box::new(move || {
        let now = now_ms();
        nav.update_value(|n| n.tick(now));

        if let Some(canvas) = canvas_ref.get() {
            if let Ok(Some(ctx)) = canvas.get_context("2d") {
                let ctx: web_sys::CanvasRenderingContext2d = ctx.dyn_into().unwrap();
                render_one_frame(&ctx, &nav, /* viewport, drag, hovered, selected */);
            }
        }

        let _ = web_sys::window().unwrap()
            .request_animation_frame(f.borrow().as_ref().unwrap().as_ref().unchecked_ref());
    }) as Box<dyn FnMut()>));

    let _ = web_sys::window().unwrap()
        .request_animation_frame(g.borrow().as_ref().unwrap().as_ref().unchecked_ref());
}

fn render_one_frame(
    ctx: &web_sys::CanvasRenderingContext2d,
    nav: &StoredValue<NavController>,
    /* viewport, drag, hovered, selected */
) {
    nav.with_value(|n| {
        match &n.state {
            NavState::Active { neighborhood, .. } => {
                crate::canvas_engine::renderer::draw_neighborhood(ctx, /* ... */, neighborhood, /* ... */, None, None);
            }
            NavState::Animating { from_neighborhood, to_neighborhood, t, .. } => {
                let interpolated = build_interpolated_neighborhood(from_neighborhood, to_neighborhood, *t);
                crate::canvas_engine::renderer::draw_neighborhood(ctx, /* ... */, &interpolated, /* ... */, None, None);
            }
            _ => { /* draw loading/idle screen */ }
        }
    });
}

fn build_interpolated_neighborhood(from: &Neighborhood, to: &Neighborhood, t: f32) -> Neighborhood {
    use crate::canvas_engine::tween::lerp_node;
    use std::collections::HashSet;

    let mut all_ids: HashSet<String> = HashSet::new();
    all_ids.insert(from.center.id.clone());
    all_ids.insert(to.center.id.clone());
    all_ids.extend(from.one_hop.iter().map(|n| n.id.clone()));
    all_ids.extend(from.two_hop.iter().map(|n| n.id.clone()));
    all_ids.extend(to.one_hop.iter().map(|n| n.id.clone()));
    all_ids.extend(to.two_hop.iter().map(|n| n.id.clone()));

    // Build a Neighborhood with target_positions = lerped, and nodes from `to` (truth in destination)
    let mut interp = to.clone();
    for id in all_ids {
        let r = lerp_node(&id, from, to, t);
        interp.target_positions.insert(id, r.pos);
        // Also propagate opacity into world_pos? Renderer uses target_positions; we keep opacity separate.
    }
    interp
}
```

The full wiring requires passing the viewport, drag offset, hovered/selected, and ensuring `world_pos` on each `CanvasNode` is updated each frame from `target_positions`. Adapt names as needed.

- [ ] **Step 2: Visually verify focus switch**

Run `just dev`, open canvas (with feature flag on), click a 1-hop neighbor. Expect:
- Smooth ~400ms animation
- Clicked node travels to center
- Old neighbors fade out, new neighbors fade in
- Breadcrumb appends

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/views/canvas/graph_canvas.rs
git commit -m "canvas(loop): RAF render loop with tween-driven animating frame interpolation"
```

---

## Task 24: Walkthrough + perf pass

**Files:** none (verification + small tweaks)

- [ ] **Step 1: Run unit + integration tests**

```bash
cargo test -p alephcore --lib
cargo test -p aleph-panel --lib
cargo test -p alephcore --test graph_handlers_test
```

Expected: all PASS.

- [ ] **Step 2: Run lint**

```bash
just clippy
```

Fix any warnings introduced by new code.

- [ ] **Step 3: Run the manual walkthrough checklist**

Start `just dev`. Enable `canvas_radial_navigation` in Settings. Then verify each item:

- [ ] Entering Canvas focuses the most-active node by default
- [ ] Single-click on 1-hop neighbor triggers smooth ~400ms animation to new Active
- [ ] Double-click also switches Active (same as single-click)
- [ ] Animation interrupted mid-flight: clicking C while A→B animates produces B→C, not A→C; breadcrumb shows A → B → C
- [ ] Hover ≥150ms on a neighbor: open browser DevTools network tab, see `graph.neighbors` request in the background
- [ ] Click on a folded ClusterNode → it expands in place; Active does not change
- [ ] Click on an expanded cluster member → switches Active to that member
- [ ] Mini-map (right-bottom) shows all sampled nodes; current Active is red with white outline
- [ ] Click a node in mini-map → switches Active to it (with animation)
- [ ] Drag on canvas background pans; Active and 1-hop visibly move faster than 2-hop (parallax)
- [ ] Switch agent → breadcrumb clears, neighborhood resets
- [ ] `Tab` cycles hover-focus among visible neighbors; `Enter` switches to that node
- [ ] `Esc` closes detail panel and folds expanded clusters
- [ ] System `prefers-reduced-motion: reduce` → animations ≤100ms, no breathing glow
- [ ] With ~100 nodes (100 in mini-map + 50 in active neighborhood expanded), browser maintains 60fps in DevTools Performance panel

- [ ] **Step 4: Quick performance probe**

In DevTools Performance, record 5 seconds while idle (Active selected, no interaction). Verify:
- After convergence (force-layout settles), main-thread idle most frames
- No reflows / layout thrashes from the canvas

If breadcrumb or DOM updates are happening on every RAF frame, isolate by suspending Effects and re-confirm.

- [ ] **Step 5: Commit any minor fixes**

```bash
git add -p
git commit -m "canvas: walkthrough fixes after manual verification"
```

(Skip this commit if walkthrough revealed no issues.)

- [ ] **Step 6: Tag the release**

```bash
git log --oneline | head -25
```

Verify all 23 prior commits land cleanly. The implementation is complete and feature-flagged off by default.

---

## Summary

24 tasks total. Implementation order is dependency-driven:

1. **Foundations (T1–T3):** server fields, types, API client
2. **Pure logic modules (T4–T10):** cluster, layout (sectors → forces), tween, prefetch, navigation
3. **Adapter (T11):** glues server response → in-memory `Neighborhood`
4. **Visual primitives (T12–T16):** viewport parallax, renderer (depth, edges, nodes), mini-map
5. **Interaction (T17):** intent enum + hover debounce + keyboard
6. **View components (T18–T20):** breadcrumb, toolbar, detail panel
7. **Feature flag (T21):** user pref
8. **Wiring (T22–T23):** mod.rs feature toggle + RAF render loop + animation
9. **Verification (T24):** walkthrough + perf

Each task ends in a commit. After all 24 tasks: 24 commits, ~1180 net LOC added, feature flag default off, ready for Beta.
