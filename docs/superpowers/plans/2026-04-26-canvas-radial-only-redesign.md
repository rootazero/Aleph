# Canvas Radial-Only Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete the legacy canvas view path, rewrite `fold_threshold` semantics so the slider has direct visible effect, add a clickable `GlobalMiniMap` overlay, and simplify the toolbar to a single coherent paradigm.

**Architecture:** One canvas view (Radial) with a deterministic 200×200 minimap overlay. `fold_threshold` becomes "max 1-hop nodes shown; the rest fold by category". `PrefetchCache` is keyed by `(id, threshold)` so threshold changes naturally invalidate stale entries. `LegacyCanvasView`, `Effect 5`, and `Global/Local`/`Radial/Legacy` toggles are removed.

**Tech Stack:** Rust + Leptos 0.8 (CSR/WASM), `web-sys` Canvas 2D API, `cargo test` for unit tests, `just build` / `just dev` for the WASM bundle, `target/release/aleph-server start` for the backend.

**Spec:** [`docs/superpowers/specs/2026-04-26-canvas-radial-only-redesign.md`](../specs/2026-04-26-canvas-radial-only-redesign.md)

**Reference module name:** Cargo package is `aleph-panel` (not `webchat`). Use `cargo test -p aleph-panel --lib ...` everywhere.

---

## File Structure

| File | Responsibility | Change type |
|---|---|---|
| `interfaces/webchat/src/canvas_engine/prefetch.rs` | Hover-prefetch cache with TTL/LRU | Modify: `(id, threshold)` key, remove `clear()` |
| `interfaces/webchat/src/canvas_engine/adapter.rs` | DTO → Neighborhood conversion + folding | Modify: rewrite folding to top-K |
| `interfaces/webchat/src/canvas_engine/cluster.rs` | Cluster fold helpers + radius calc | Modify: delete `fold_sector`/`fallback_fold`, add `group_by_category_into_clusters`, keep `cluster_radius` |
| `interfaces/webchat/src/canvas_engine/mini_map.rs` | Minimap data + render + hit-test | Rewrite: replace `MiniMap` with `GlobalMiniMap` |
| `interfaces/webchat/src/views/canvas/toolbar.rs` | Top toolbar (search, slider) | Modify: remove toggles, add `(K of N)` counter |
| `interfaces/webchat/src/views/canvas/mod.rs` | Canvas root + `RadialCanvasView` + Effects | Modify: delete `LegacyCanvasView`, `Effect 5`, `view_mode` signal, `radial_signal` switch; wire minimap |
| `interfaces/webchat/src/views/canvas/minimap_view.rs` | Leptos component for minimap overlay | Create |
| `interfaces/webchat/src/canvas_engine/types.rs` | Shared types | No change (`ViewMode` enum stays; unused for one release) |
| `interfaces/webchat/src/context.rs` | Dashboard state | No change (`canvas_radial_navigation` stays as no-op for compat) |

---

## Task 1: PrefetchCache keyed by (id, threshold)

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/prefetch.rs`

- [ ] **Step 1.1: Add failing test for new cache-key signature**

Append to `interfaces/webchat/src/canvas_engine/prefetch.rs::tests`:

```rust
#[test]
fn cache_miss_when_threshold_differs() {
    let mut c = PrefetchCache::new();
    c.put("a".to_string(), 12, nbhd("a", 0.0));
    assert!(c.get("a", 12, 100.0).is_some(), "same threshold hits");
    assert!(c.get("a", 6, 100.0).is_none(), "different threshold misses");
}
```

- [ ] **Step 1.2: Run the new test and existing tests; verify the new one fails to compile**

```bash
cargo test -p aleph-panel --lib canvas_engine::prefetch
```
Expected: compile error on `c.put(... 12 ...)` and `c.get(... 12 ...)` because the current signatures take only `(String, Neighborhood)` and `(&str, f64)`.

- [ ] **Step 1.3: Update `PrefetchCache` to use `(String, usize)` as the entry key**

Replace the body of `interfaces/webchat/src/canvas_engine/prefetch.rs` (top, struct + impl) with:

```rust
use crate::canvas_engine::types::Neighborhood;
use std::collections::VecDeque;

pub const HOVER_DEBOUNCE_MS: f64 = 150.0;
pub const CACHE_TTL_MS: f64 = 60_000.0;
pub const CACHE_CAPACITY: usize = 20;

pub struct PrefetchCache {
    entries: VecDeque<((String, usize), Neighborhood)>,
    capacity: usize,
    ttl_ms: f64,
}

impl PrefetchCache {
    pub fn new() -> Self {
        Self { entries: VecDeque::new(), capacity: CACHE_CAPACITY, ttl_ms: CACHE_TTL_MS }
    }

    pub fn put(&mut self, id: String, threshold: usize, nbhd: Neighborhood) {
        let key = (id, threshold);
        self.entries.retain(|(k, _)| k != &key);
        self.entries.push_back((key, nbhd));
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }

    pub fn get(&self, id: &str, threshold: usize, now_ms: f64) -> Option<&Neighborhood> {
        self.entries.iter().rev().find_map(|((k_id, k_thresh), v)| {
            if k_id == id
                && *k_thresh == threshold
                && now_ms - v.fetched_at_ms <= self.ttl_ms
            {
                Some(v)
            } else {
                None
            }
        })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}
```

(Note: `clear()` is intentionally deleted — its only caller is `Effect 5`, which is also being removed in Task 8.)

- [ ] **Step 1.4: Update existing tests to pass `threshold`**

Replace these three existing tests in the same file:

```rust
#[test]
fn cache_put_then_get() {
    let mut c = PrefetchCache::new();
    c.put("a".to_string(), 12, nbhd("a", 0.0));
    assert!(c.get("a", 12, 100.0).is_some());
}

#[test]
fn cache_expires_after_ttl() {
    let mut c = PrefetchCache::new();
    c.put("a".to_string(), 12, nbhd("a", 0.0));
    assert!(c.get("a", 12, CACHE_TTL_MS + 1.0).is_none());
}

#[test]
fn cache_evicts_oldest_at_capacity() {
    let mut c = PrefetchCache::new();
    for i in 0..(CACHE_CAPACITY + 5) {
        c.put(format!("n{i}"), 12, nbhd(&format!("n{i}"), 0.0));
    }
    assert_eq!(c.len(), CACHE_CAPACITY);
    assert!(c.get("n0", 12, 0.0).is_none());
    assert!(c.get(&format!("n{}", CACHE_CAPACITY + 4), 12, 0.0).is_some());
}
```

- [ ] **Step 1.5: Update callers in `views/canvas/mod.rs`**

In `RadialCanvasView` Effect 2 (around line 159), change:
```rust
let cached = prefetch_req.borrow().get(&id, now_ms).cloned();
```
to:
```rust
let cached = prefetch_req.borrow().get(&id, threshold, now_ms).cloned();
```

In Effect 4 (around line 232), change:
```rust
if prefetch_e4.borrow().get(&id, now).is_some() { return; }
```
to:
```rust
let threshold = fold_threshold.get_untracked();
if prefetch_e4.borrow().get(&id, threshold, now).is_some() { return; }
```

In Effect 4 spawned-async block (around line 244), change:
```rust
prefetch_inner.borrow_mut().put(id, nbhd);
```
to:
```rust
prefetch_inner.borrow_mut().put(id, threshold, nbhd);
```

(`threshold` is captured by the spawn closure since `fold_threshold.get_untracked()` was already read above.)

- [ ] **Step 1.6: Run all canvas_engine tests; expect green**

```bash
cargo test -p aleph-panel --lib canvas_engine
```
Expected: all tests pass.

- [ ] **Step 1.7: WASM check**

```bash
cargo check -p aleph-panel --target wasm32-unknown-unknown
```
Expected: exit 0, no errors.

- [ ] **Step 1.8: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/prefetch.rs interfaces/webchat/src/views/canvas/mod.rs
git commit -m "$(cat <<'EOF'
canvas(prefetch): key cache by (id, threshold)

Threshold changes now naturally invalidate stale cache entries instead
of relying on an explicit clear(). Removes the need for Effect 5's
forced-refetch round-trip in the next task.
EOF
)"
```

---

## Task 2: Top-K folding in `to_neighborhood`

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/cluster.rs`
- Modify: `interfaces/webchat/src/canvas_engine/adapter.rs`

- [ ] **Step 2.1: Add failing tests in `cluster.rs::tests` for `group_by_category_into_clusters`**

Append to the bottom of `interfaces/webchat/src/canvas_engine/cluster.rs::tests`:

```rust
#[test]
fn group_by_category_creates_one_cluster_per_category() {
    let nodes = vec![
        node("a", "concept"),
        node("b", "concept"),
        node("c", "reference"),
    ];
    let clusters = group_by_category_into_clusters(nodes, "center");
    assert_eq!(clusters.len(), 2, "concept + reference => 2 clusters");
    let kinds: Vec<&str> = clusters.iter().map(|c| c.kind.as_str()).collect();
    assert!(kinds.contains(&"concept"));
    assert!(kinds.contains(&"reference"));
}

#[test]
fn group_by_category_uses_top3_names_as_representatives() {
    let nodes = (0..5)
        .map(|i| {
            let mut n = node(&format!("n{i}"), "concept");
            n.name = format!("name-{i}");
            n.decay_score = (5 - i) as f32; // n0 highest, n4 lowest
            n.edge_count = 1;
            n
        })
        .collect();
    let clusters = group_by_category_into_clusters(nodes, "center");
    assert_eq!(clusters.len(), 1);
    assert_eq!(clusters[0].representative_names, vec!["name-0", "name-1", "name-2"]);
}

// Helper used by the new tests
fn node(id: &str, category: &str) -> CanvasNode {
    CanvasNode {
        id: id.to_string(),
        name: id.to_string(),
        category: category.to_string(),
        color: Color::new(0, 0, 0),
        radius: 24.0,
        position: Vec2::new(0.0, 0.0),
        velocity: Vec2::new(0.0, 0.0),
        z: 0.0,
        hop: 1,
        pinned: false,
        decay_score: 1.0,
        edge_count: 1,
    }
}
```

- [ ] **Step 2.2: Run tests; expect compile error on `group_by_category_into_clusters`**

```bash
cargo test -p aleph-panel --lib canvas_engine::cluster
```
Expected: compile error: cannot find function `group_by_category_into_clusters`.

- [ ] **Step 2.3: Implement `group_by_category_into_clusters` in `cluster.rs`**

Add at the bottom of `interfaces/webchat/src/canvas_engine/cluster.rs` (before `#[cfg(test)]`):

```rust
/// Fold a slice of nodes into one ClusterNode per distinct `CanvasNode::category`.
/// Each cluster's `representative_names` is the top 3 by descending weight.
///
/// `relation` is set to "_default" since the underlying graph edges no longer
/// carry a relation field.
pub fn group_by_category_into_clusters(
    nodes: Vec<CanvasNode>,
    active_id: &str,
) -> Vec<ClusterNode> {
    let mut by_category: HashMap<String, Vec<CanvasNode>> = HashMap::new();
    for n in nodes {
        by_category.entry(n.category.clone()).or_default().push(n);
    }

    let mut clusters: Vec<ClusterNode> = Vec::with_capacity(by_category.len());
    for (category, mut group) in by_category {
        group.sort_by(|a, b| {
            let wa = a.decay_score * a.edge_count as f32;
            let wb = b.decay_score * b.edge_count as f32;
            wb.partial_cmp(&wa).unwrap_or(std::cmp::Ordering::Equal)
        });
        let aggregated_weight: f32 =
            group.iter().map(|n| n.decay_score * n.edge_count as f32).sum();
        let representative_names: Vec<String> =
            group.iter().take(3).map(|n| n.name.clone()).collect();
        let member_ids: Vec<String> = group.iter().map(|n| n.id.clone()).collect();
        let radius = cluster_radius(member_ids.len());
        clusters.push(ClusterNode {
            id: format!("cluster::_default::{}::{}", category, active_id),
            relation: "_default".to_string(),
            kind: category,
            member_ids,
            representative_names,
            aggregated_weight,
            radius,
            world_pos: Vec2::new(0.0, 0.0),
            z: 60.0,
            expanded: false,
        });
    }
    clusters.sort_by(|a, b| a.kind.cmp(&b.kind));
    clusters
}
```

- [ ] **Step 2.4: Run cluster.rs tests; expect new tests to pass, old tests still passing for now**

```bash
cargo test -p aleph-panel --lib canvas_engine::cluster
```
Expected: 2 new tests pass, old `fold_sector`/`fallback_fold` tests still pass.

- [ ] **Step 2.5: Add failing tests for top-K folding in `adapter.rs::tests`**

Append to `interfaces/webchat/src/canvas_engine/adapter.rs::tests`:

```rust
#[test]
fn top_k_fold_keeps_all_when_under_threshold() {
    let resp = make_resp_with_n_one_hop(8, "concept");
    let nb = to_neighborhood(&resp, 0.0, 12);
    assert_eq!(nb.one_hop.len(), 8);
    assert_eq!(nb.clusters.len(), 0);
}

#[test]
fn top_k_fold_keeps_top_k_by_weight() {
    let mut resp = make_resp_with_n_one_hop(20, "concept");
    // Set weights: node "n0" highest, "n19" lowest
    for (i, n) in resp.nodes.iter_mut().enumerate() {
        n.edge_count = (20 - i as i32).max(1) as u32;
        n.decay_score = 1.0;
    }
    let nb = to_neighborhood(&resp, 0.0, 5);
    assert_eq!(nb.one_hop.len(), 5);
    let kept_ids: Vec<&str> = nb.one_hop.iter().map(|n| n.id.as_str()).collect();
    for i in 0..5 {
        assert!(kept_ids.contains(&format!("n{i}").as_str()),
                "n{i} should be kept (top weight)");
    }
}

#[test]
fn top_k_fold_remainder_splits_by_category() {
    // 3 categories of 8 each = 24 total; threshold=10 ⇒ 10 unfolded, 14 in clusters across categories
    let mut resp = make_resp_with_n_one_hop(0, "concept");
    let mut id_counter = 0;
    for cat in &["concept", "reference", "topic"] {
        for _ in 0..8 {
            resp.nodes.push(NoteNodeDto {
                id: format!("n{id_counter}"),
                name: format!("name{id_counter}"),
                category: (*cat).to_string(),
                ..NoteNodeDto::default()
            });
            resp.hop_depth.insert(format!("n{id_counter}"), 1);
            id_counter += 1;
        }
    }
    let nb = to_neighborhood(&resp, 0.0, 10);
    assert_eq!(nb.one_hop.len(), 10);
    assert_eq!(nb.clusters.len(), 3, "one cluster per category");
    let total_in_clusters: usize = nb.clusters.iter().map(|c| c.member_ids.len()).sum();
    assert_eq!(total_in_clusters, 14);
}
```

If `make_resp_with_n_one_hop` doesn't exist in the test module, add this helper:

```rust
fn make_resp_with_n_one_hop(n: usize, category: &str) -> GraphNeighborsResponse {
    let mut resp = GraphNeighborsResponse {
        center: NoteNodeDto {
            id: "center".to_string(),
            name: "center".to_string(),
            category: "concept".to_string(),
            ..NoteNodeDto::default()
        },
        nodes: Vec::new(),
        edges: Vec::new(),
        hop_depth: HashMap::new(),
    };
    for i in 0..n {
        let id = format!("n{i}");
        resp.nodes.push(NoteNodeDto {
            id: id.clone(),
            name: id.clone(),
            category: category.to_string(),
            ..NoteNodeDto::default()
        });
        resp.hop_depth.insert(id, 1);
    }
    resp
}
```

(`NoteNodeDto::default()` may not exist — if the test fails to compile, replace with a literal struct that fills every field. The test helper exists to keep tests legible; adapt the field list to whatever `NoteNodeDto` declares.)

- [ ] **Step 2.6: Run adapter tests; expect compile/assertion failures**

```bash
cargo test -p aleph-panel --lib canvas_engine::adapter
```
Expected: tests fail because `to_neighborhood` still uses the old `fold_sector` path.

- [ ] **Step 2.7: Rewrite the folding section in `adapter.rs::to_neighborhood`**

Find the block (around line 107-137) starting `// Group 1-hop nodes by the relation label …` and ending after the `for (rel, group) in by_relation` loop.

Replace that entire block with:

```rust
    // Top-K folding: show at most `fold_threshold` 1-hop nodes (the highest-weight
    // ones), and fold the remainder into one ClusterNode per category. The previous
    // by-relation grouping was a no-op since NoteLinkDto carries no relation field.
    let (filtered_one_hop, clusters): (Vec<CanvasNode>, Vec<ClusterNode>) =
        if one_hop.len() <= fold_threshold {
            (one_hop, Vec::new())
        } else {
            let mut sorted = one_hop;
            sorted.sort_by(|a, b| {
                let wa = a.decay_score * a.edge_count as f32;
                let wb = b.decay_score * b.edge_count as f32;
                wb.partial_cmp(&wa).unwrap_or(std::cmp::Ordering::Equal)
            });
            let kept: Vec<CanvasNode> = sorted.drain(..fold_threshold).collect();
            let folded = group_by_category_into_clusters(sorted, &resp.center.id);
            (kept, folded)
        };
```

Imports: ensure the file has `use crate::canvas_engine::cluster::group_by_category_into_clusters;` (replace any line that imports `fold_sector`/`fallback_fold`).

- [ ] **Step 2.8: Run adapter tests; expect green**

```bash
cargo test -p aleph-panel --lib canvas_engine::adapter
```
Expected: all 3 new tests pass plus existing `to_neighborhood_basic_shape` (may need its `12` argument verified).

- [ ] **Step 2.9: Run full canvas_engine test suite; expect green**

```bash
cargo test -p aleph-panel --lib canvas_engine
```
Expected: all tests pass.

- [ ] **Step 2.10: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/cluster.rs interfaces/webchat/src/canvas_engine/adapter.rs
git commit -m "$(cat <<'EOF'
canvas(adapter): rewrite folding to top-K semantics

The slider now directly controls "max 1-hop nodes shown"; the remainder
folds into one ClusterNode per category. Previously fold_sector compared
threshold against per-category subgroup size, which never triggered in
real-world data spread across many categories — the slider had no
visible effect.
EOF
)"
```

---

## Task 3: Delete `fold_sector` and `fallback_fold`

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/cluster.rs`

- [ ] **Step 3.1: Confirm no remaining callers**

```bash
grep -rn "fold_sector\|fallback_fold\|FOLD_THRESHOLD" interfaces/webchat/src
```
Expected: only matches inside `cluster.rs` itself (definitions + tests). If anything outside `cluster.rs` matches, fix it before deleting.

- [ ] **Step 3.2: Delete the old functions and their tests**

In `interfaces/webchat/src/canvas_engine/cluster.rs`:
- Delete `pub const FOLD_THRESHOLD: usize = 12;`
- Delete `pub fn fold_sector(...)` (the entire function body)
- Delete `pub fn fallback_fold(...)` (the entire function body)
- Delete the test functions: `fold_below_threshold_keeps_all`, `fold_at_threshold_creates_cluster`, anything else that calls `fold_sector`/`fallback_fold`

Keep:
- `pub fn cluster_radius(...)`
- `pub fn group_by_category_into_clusters(...)` (added in Task 2)
- `cluster_radius_log_scaling` test
- The `node(...)` test helper added in Task 2
- `group_by_category_*` tests added in Task 2

- [ ] **Step 3.3: Build and test**

```bash
cargo test -p aleph-panel --lib canvas_engine
```
Expected: all surviving tests pass; no compile errors.

- [ ] **Step 3.4: WASM check**

```bash
cargo check -p aleph-panel --target wasm32-unknown-unknown
```
Expected: exit 0.

- [ ] **Step 3.5: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/cluster.rs
git commit -m "$(cat <<'EOF'
canvas(cluster): drop unused fold_sector and fallback_fold

Top-K folding in adapter.rs::to_neighborhood replaces both. The
FOLD_THRESHOLD constant is also gone — the default lives at the
slider signal definition (12 in views/canvas/mod.rs).
EOF
)"
```

---

## Task 4: GlobalMiniMap data layer

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/mini_map.rs`

- [ ] **Step 4.1: Replace mini_map.rs with new struct + tests**

Replace the entire contents of `interfaces/webchat/src/canvas_engine/mini_map.rs` with:

```rust
//! Global minimap: deterministic 2D projection of the full graph.
//!
//! Nodes are placed by hashing their id (angle + radius). Connected components
//! are computed via union-find on the edges and used to color-group nodes.
//! Click hit-testing is exposed via `pick_at`.

use crate::api::memory::{NoteLinkDto, NoteNodeDto};
use crate::canvas_engine::types::{Color, Vec2};
use std::collections::HashMap;
use std::f64::consts::TAU;

#[derive(Clone, Debug)]
pub struct MiniPoint {
    pub id: String,
    pub pos: Vec2,
    pub component: u32,
}

pub struct GlobalMiniMap {
    pub size_px: f32,
    pub points: Vec<MiniPoint>,
    pub component_colors: HashMap<u32, Color>,
}

impl GlobalMiniMap {
    pub fn empty(size_px: f32) -> Self {
        Self { size_px, points: Vec::new(), component_colors: HashMap::new() }
    }

    /// Build a deterministic minimap from full-graph DTOs and edges.
    pub fn build(dtos: &[NoteNodeDto], edges: &[NoteLinkDto], size_px: f32) -> Self {
        let component_of = compute_components(dtos, edges);
        let center = (size_px / 2.0) as f64;
        let max_r = (size_px / 2.0 - 6.0).max(1.0) as f64;

        let mut points = Vec::with_capacity(dtos.len());
        for dto in dtos {
            let h1 = hash_to_unit(&dto.id, 0xA5A5_A5A5);
            let h2 = hash_to_unit(&dto.id, 0x5A5A_5A5A);
            let angle = h1 * TAU;
            let radius = h2.sqrt() * max_r;
            let x = center + radius * angle.cos();
            let y = center + radius * angle.sin();
            let component = component_of.get(&dto.id).copied().unwrap_or(0);
            points.push(MiniPoint {
                id: dto.id.clone(),
                pos: Vec2::new(x, y),
                component,
            });
        }

        let component_colors = assign_component_colors(&points);
        Self { size_px, points, component_colors }
    }

    /// Return the id of the closest node within `hit_radius` of `(mx, my)`,
    /// or `None` if no node is close enough.
    pub fn pick_at(&self, mx: f32, my: f32, hit_radius: f32) -> Option<&str> {
        let mx = mx as f64;
        let my = my as f64;
        let r2 = (hit_radius * hit_radius) as f64;
        let mut best: Option<(&str, f64)> = None;
        for p in &self.points {
            let dx = p.pos.x - mx;
            let dy = p.pos.y - my;
            let d2 = dx * dx + dy * dy;
            if d2 <= r2 && best.map(|(_, b)| d2 < b).unwrap_or(true) {
                best = Some((p.id.as_str(), d2));
            }
        }
        best.map(|(id, _)| id)
    }
}

fn hash_to_unit(s: &str, salt: u64) -> f64 {
    // FNV-1a 64-bit, then map to [0, 1).
    let mut h: u64 = 0xcbf29ce484222325 ^ salt;
    for byte in s.as_bytes() {
        h ^= *byte as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    (h as f64) / (u64::MAX as f64)
}

fn compute_components(dtos: &[NoteNodeDto], edges: &[NoteLinkDto]) -> HashMap<String, u32> {
    let mut idx: HashMap<&str, usize> = HashMap::new();
    for (i, n) in dtos.iter().enumerate() {
        idx.insert(n.id.as_str(), i);
    }
    let mut parent: Vec<usize> = (0..dtos.len()).collect();

    fn find(parent: &mut [usize], x: usize) -> usize {
        let mut root = x;
        while parent[root] != root {
            root = parent[root];
        }
        let mut cur = x;
        while parent[cur] != root {
            let next = parent[cur];
            parent[cur] = root;
            cur = next;
        }
        root
    }

    for e in edges {
        let (Some(&a), Some(&b)) = (idx.get(e.from.as_str()), idx.get(e.to.as_str())) else {
            continue;
        };
        let ra = find(&mut parent, a);
        let rb = find(&mut parent, b);
        if ra != rb {
            parent[ra] = rb;
        }
    }

    let mut out = HashMap::with_capacity(dtos.len());
    let mut roots: HashMap<usize, u32> = HashMap::new();
    let mut next_id: u32 = 0;
    for (i, n) in dtos.iter().enumerate() {
        let root = find(&mut parent, i);
        let cid = *roots.entry(root).or_insert_with(|| {
            let id = next_id;
            next_id += 1;
            id
        });
        out.insert(n.id.clone(), cid);
    }
    out
}

fn assign_component_colors(points: &[MiniPoint]) -> HashMap<u32, Color> {
    let mut comps: Vec<u32> = points.iter().map(|p| p.component).collect();
    comps.sort_unstable();
    comps.dedup();
    let n = comps.len().max(1);
    let mut out = HashMap::with_capacity(n);
    for (i, c) in comps.iter().enumerate() {
        // HSL → RGB, hue spaced evenly, fixed S/L for visual cohesion.
        let hue = (i as f32) * 360.0 / (n as f32);
        let (r, g, b) = hsl_to_rgb(hue, 0.55, 0.55);
        out.insert(*c, Color::new(r, g, b));
    }
    out
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h_prime = h / 60.0;
    let x = c * (1.0 - (h_prime % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match h_prime as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    (
        ((r1 + m) * 255.0).clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).clamp(0.0, 255.0) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::memory::{NoteLinkDto, NoteNodeDto};

    fn dto(id: &str) -> NoteNodeDto {
        NoteNodeDto {
            id: id.to_string(),
            name: id.to_string(),
            category: "concept".to_string(),
            ..NoteNodeDto::default()
        }
    }

    fn link(from: &str, to: &str) -> NoteLinkDto {
        NoteLinkDto { from: from.to_string(), to: to.to_string() }
    }

    #[test]
    fn deterministic_layout() {
        let dtos = vec![dto("a"), dto("b"), dto("c")];
        let edges = vec![link("a", "b")];
        let m1 = GlobalMiniMap::build(&dtos, &edges, 200.0);
        let m2 = GlobalMiniMap::build(&dtos, &edges, 200.0);
        for (p1, p2) in m1.points.iter().zip(m2.points.iter()) {
            assert!((p1.pos.x - p2.pos.x).abs() < 1e-9);
            assert!((p1.pos.y - p2.pos.y).abs() < 1e-9);
        }
    }

    #[test]
    fn pick_at_finds_node() {
        let dtos = vec![dto("a"), dto("b")];
        let m = GlobalMiniMap::build(&dtos, &[], 200.0);
        let target = &m.points[0];
        let hit = m.pick_at(target.pos.x as f32, target.pos.y as f32, 5.0);
        assert_eq!(hit, Some(target.id.as_str()));
    }

    #[test]
    fn pick_at_misses_outside_radius() {
        let dtos = vec![dto("a")];
        let m = GlobalMiniMap::build(&dtos, &[], 200.0);
        let hit = m.pick_at(-100.0, -100.0, 3.0);
        assert!(hit.is_none());
    }

    #[test]
    fn connected_components_share_color() {
        // a-b connected, c isolated.
        let dtos = vec![dto("a"), dto("b"), dto("c")];
        let edges = vec![link("a", "b")];
        let m = GlobalMiniMap::build(&dtos, &edges, 200.0);
        let comp_a = m.points.iter().find(|p| p.id == "a").unwrap().component;
        let comp_b = m.points.iter().find(|p| p.id == "b").unwrap().component;
        let comp_c = m.points.iter().find(|p| p.id == "c").unwrap().component;
        assert_eq!(comp_a, comp_b);
        assert_ne!(comp_a, comp_c);
    }

    #[test]
    fn empty_minimap_has_no_points() {
        let m = GlobalMiniMap::empty(200.0);
        assert_eq!(m.points.len(), 0);
        assert!(m.pick_at(100.0, 100.0, 5.0).is_none());
    }
}
```

(Note: the `..NoteNodeDto::default()` and `NoteLinkDto { ... }` literals must match the actual struct definitions in `api/memory.rs`. If `Default` isn't derived on `NoteNodeDto`, replace with explicit construction.)

- [ ] **Step 4.2: Run mini_map tests**

```bash
cargo test -p aleph-panel --lib canvas_engine::mini_map
```
Expected: all 5 tests pass.

- [ ] **Step 4.3: Confirm no other modules reference the old `MiniMap`**

```bash
grep -rn "MiniMap\|mini_map" interfaces/webchat/src
```
Look for references like `MiniMap::empty()` or `MiniMap::new(...)` outside the new file. The previous code at `views/canvas/mod.rs:84` had `let _minimap = Rc::new(RefCell::new(MiniMap::empty()));`. This is dead code (`_minimap` prefix) and must be removed.

In `views/canvas/mod.rs`, delete the line:
```rust
let _minimap = Rc::new(RefCell::new(MiniMap::empty()));
```
and the corresponding `use crate::canvas_engine::mini_map::MiniMap;` import (around line 18).

- [ ] **Step 4.4: WASM check**

```bash
cargo check -p aleph-panel --target wasm32-unknown-unknown
```
Expected: exit 0.

- [ ] **Step 4.5: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/mini_map.rs interfaces/webchat/src/views/canvas/mod.rs
git commit -m "$(cat <<'EOF'
canvas(minimap): GlobalMiniMap deterministic data layer

Replaces the unused MiniMap struct with GlobalMiniMap: hash-based
deterministic projection, union-find connected components, evenly
spaced HSL hues, and pick_at hit-testing. Pure data; rendering
follows in the next task.
EOF
)"
```

---

## Task 5: GlobalMiniMap rendering helper

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/mini_map.rs`

- [ ] **Step 5.1: Add render method on `GlobalMiniMap`**

Append a new `impl GlobalMiniMap` block to `interfaces/webchat/src/canvas_engine/mini_map.rs` (above the `#[cfg(test)]` module, gated on the `wasm32` target since it touches `web_sys`):

```rust
#[cfg(target_arch = "wasm32")]
mod render {
    use super::*;
    use wasm_bindgen::JsValue;
    use web_sys::CanvasRenderingContext2d;

    impl GlobalMiniMap {
        /// Repaint the minimap into `ctx`. The caller is responsible for
        /// clearing the canvas first if needed.
        ///
        /// `focus_id` is the currently centered Radial node; it gets a thicker
        /// outlined dot.
        /// `focus_neighbor_ids` is the 1-hop set; those points are painted
        /// slightly larger so the user can see the local neighborhood.
        pub fn render(
            &self,
            ctx: &CanvasRenderingContext2d,
            focus_id: Option<&str>,
            focus_neighbor_ids: &[String],
        ) {
            let size = self.size_px as f64;
            let half = size / 2.0;

            // Background circle outline
            ctx.set_stroke_style(&JsValue::from_str("rgba(255,255,255,0.08)"));
            ctx.set_line_width(1.0);
            ctx.begin_path();
            let _ = ctx.arc(half, half, half - 2.0, 0.0, std::f64::consts::TAU);
            ctx.stroke();

            // Node points
            for p in &self.points {
                let is_focus = focus_id.map_or(false, |f| f == p.id);
                let is_neighbor = focus_neighbor_ids.iter().any(|n| n == &p.id);
                let radius = if is_focus { 4.0 } else if is_neighbor { 3.0 } else { 1.6 };

                let color = self
                    .component_colors
                    .get(&p.component)
                    .copied()
                    .unwrap_or(Color::new(180, 180, 180));
                let css = format!("rgb({},{},{})", color.r, color.g, color.b);
                ctx.set_fill_style(&JsValue::from_str(&css));
                ctx.begin_path();
                let _ = ctx.arc(p.pos.x, p.pos.y, radius, 0.0, std::f64::consts::TAU);
                ctx.fill();

                if is_focus {
                    ctx.set_stroke_style(&JsValue::from_str("rgba(255,255,255,0.9)"));
                    ctx.set_line_width(1.5);
                    ctx.begin_path();
                    let _ = ctx.arc(p.pos.x, p.pos.y, radius + 1.5, 0.0, std::f64::consts::TAU);
                    ctx.stroke();
                }
            }
        }
    }
}
```

(`Color::new(r, g, b)` and `Color { r, g, b }` field access must match `types.rs`. If `Color` has private fields, add `pub fn rgb(&self) -> (u8, u8, u8)` accessor in `types.rs`.)

- [ ] **Step 5.2: WASM check (rendering only compiles for wasm32)**

```bash
cargo check -p aleph-panel --target wasm32-unknown-unknown
```
Expected: exit 0.

- [ ] **Step 5.3: Native check (render module excluded)**

```bash
cargo check -p aleph-panel --lib
```
Expected: exit 0; the `render` submodule is gated by `#[cfg(target_arch = "wasm32")]` so it doesn't build on native.

- [ ] **Step 5.4: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/mini_map.rs
git commit -m "$(cat <<'EOF'
canvas(minimap): GlobalMiniMap.render — points + focus highlight

Wasm-only render path: background circle, colored point per node, the
current Radial center gets a thicker outlined dot, 1-hop neighbors are
slightly larger. No edges drawn.
EOF
)"
```

---

## Task 6: MiniMap Leptos component + wire into RadialCanvasView

**Files:**
- Create: `interfaces/webchat/src/views/canvas/minimap_view.rs`
- Modify: `interfaces/webchat/src/views/canvas/mod.rs`
- Modify: `interfaces/webchat/src/views/canvas.rs` (or wherever the `mod toolbar;` declaration lives) — add `mod minimap_view;`

- [ ] **Step 6.1: Create the minimap component file**

Create `interfaces/webchat/src/views/canvas/minimap_view.rs` with:

```rust
use crate::canvas_engine::mini_map::GlobalMiniMap;
use leptos::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, MouseEvent};

const MINIMAP_PX: f32 = 200.0;
const HIT_RADIUS_PX: f32 = 6.0;

#[component]
pub fn MiniMapOverlay(
    minimap: Rc<RefCell<GlobalMiniMap>>,
    focus_id: ReadSignal<Option<String>>,
    focus_neighbor_ids: ReadSignal<Vec<String>>,
    on_pick: impl Fn(String) + 'static + Copy,
) -> impl IntoView {
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();

    // Repaint whenever data or focus changes
    let mm_render = minimap.clone();
    Effect::new(move |_| {
        let _ = focus_id.get();
        let _ = focus_neighbor_ids.get();
        let Some(canvas) = canvas_ref.get() else { return };
        let canvas: HtmlCanvasElement = canvas.unchecked_into();
        let Some(ctx) = canvas
            .get_context("2d")
            .ok()
            .flatten()
            .and_then(|o| o.dyn_into::<CanvasRenderingContext2d>().ok())
        else {
            return;
        };
        ctx.clear_rect(0.0, 0.0, MINIMAP_PX as f64, MINIMAP_PX as f64);
        let neighbors = focus_neighbor_ids.get();
        let focus = focus_id.get();
        mm_render
            .borrow()
            .render(&ctx, focus.as_deref(), &neighbors);
    });

    let mm_click = minimap.clone();
    let on_click = move |ev: MouseEvent| {
        let Some(canvas) = canvas_ref.get() else { return };
        let canvas: HtmlCanvasElement = canvas.unchecked_into();
        let rect = canvas.get_bounding_client_rect();
        let mx = ev.client_x() as f32 - rect.left() as f32;
        let my = ev.client_y() as f32 - rect.top() as f32;
        if let Some(id) = mm_click.borrow().pick_at(mx, my, HIT_RADIUS_PX) {
            on_pick(id.to_string());
        }
    };

    view! {
        <div
            class="absolute bottom-4 right-4 rounded-lg overflow-hidden \
                   border border-border/50 bg-surface-raised/80 backdrop-blur"
            style="width: 200px; height: 200px;"
        >
            <canvas
                node_ref=canvas_ref
                width="200"
                height="200"
                on:click=on_click
                class="cursor-pointer block"
            />
        </div>
    }
}
```

- [ ] **Step 6.2: Register the new module**

In `interfaces/webchat/src/views/canvas/mod.rs`, near the existing `mod toolbar;` (line ~4), add:
```rust
mod minimap_view;
use minimap_view::MiniMapOverlay;
```

Also add the import for the data type:
```rust
use crate::canvas_engine::mini_map::GlobalMiniMap;
```

- [ ] **Step 6.3: Build the minimap inside `RadialCanvasView` Effect 1**

In `RadialCanvasView` (around line 75 where signals are declared), add:
```rust
let minimap: Rc<RefCell<GlobalMiniMap>> = Rc::new(RefCell::new(GlobalMiniMap::empty(200.0)));
let (focus_id, set_focus_id) = signal(None::<String>);
let (focus_neighbors, set_focus_neighbors) = signal(Vec::<String>::new());
```

Inside Effect 1's spawn_local block, after `all_dtos.set(r.nodes.clone());` (around line 108), add:
```rust
            let mm = GlobalMiniMap::build(&r.nodes, &r.edges, 200.0);
            *minimap_init.borrow_mut() = mm;
```

Above the `Effect::new(move || {` for Effect 1, capture `minimap`:
```rust
let minimap_init = minimap.clone();
```

- [ ] **Step 6.4: Update focus state when neighborhoods load**

Inside Effect 2's spawn_local `Ok(resp) =>` branch (around line 174), after `nav_fetch.borrow_mut().fulfilled(...)`, add:
```rust
                    set_focus_id.set(Some(id.clone()));
                    let neighbor_ids: Vec<String> = nbhd.one_hop.iter().map(|n| n.id.clone()).collect();
                    set_focus_neighbors.set(neighbor_ids);
```

Also inside the `if let Some(nbhd) = cached` cache-hit branch (around line 160-164), do the same updates after `nav_req.borrow_mut().fulfilled(...)`.

In Effect 1 too, after `nav_inner.borrow_mut().fulfilled(entry_id, name, nbhd);` (around line 134), add:
```rust
                    set_focus_id.set(Some(entry_id.clone()));
                    let neighbor_ids: Vec<String> = nbhd.one_hop.iter().map(|n| n.id.clone()).collect();
                    set_focus_neighbors.set(neighbor_ids);
```

(`nbhd` is borrowed after `fulfilled` consumes it; restructure to clone the ids before the move into `fulfilled` — i.e. compute `neighbor_ids` and `entry_id_clone` before `nav_inner.borrow_mut().fulfilled(...)`.)

- [ ] **Step 6.5: Add `<MiniMapOverlay/>` to the view**

In `RadialCanvasView::view!` (around line 388-426), inside the `<div class="flex-1 relative bg-[#0a0a0f]">` that wraps `<GraphCanvas>`, add **after** `<GraphCanvas>`:

```rust
                    <MiniMapOverlay
                        minimap=minimap.clone()
                        focus_id=focus_id
                        focus_neighbor_ids=focus_neighbors
                        on_pick=move |id: String| {
                            set_selected_node.set(Some(id.clone()));
                            active_request.set(Some(id));
                        }
                    />
```

- [ ] **Step 6.6: Build WASM and run dev server**

```bash
just build
```

In another shell, restart the server (kill old processes first):
```bash
pkill -f "target/release/aleph-server" 2>/dev/null
sleep 2
target/release/aleph-server start
```
Expected: server logs show port 18790 listening.

- [ ] **Step 6.7: Manual visual check**

Open `http://127.0.0.1:18790/memory` in browser. Expected:
- Bottom-right shows a 200×200 minimap with colored point cloud
- One point has a white outline (current Radial center)
- A few points around it are slightly larger (1-hop neighbors)
- Clicking a different point on the minimap re-centers the Radial view

If not behaving, check browser DevTools console for runtime errors.

- [ ] **Step 6.8: Commit**

```bash
git add interfaces/webchat/src/views/canvas/minimap_view.rs interfaces/webchat/src/views/canvas/mod.rs interfaces/webchat/dist/aleph_panel.js interfaces/webchat/dist/aleph_panel_bg.wasm
git commit -m "$(cat <<'EOF'
canvas(minimap): wire MiniMapOverlay into RadialCanvasView

200x200 bottom-right overlay; clicks dispatch active_request to recenter
the Radial view. focus_id and focus_neighbor_ids signals drive repaint
on every neighborhood change.
EOF
)"
```

---

## Task 7: Toolbar simplification + (K of N) counter

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/toolbar.rs`
- Modify: `interfaces/webchat/src/views/canvas/mod.rs`

- [ ] **Step 7.1: Replace toolbar.rs with simplified version**

Replace the entire contents of `interfaces/webchat/src/views/canvas/toolbar.rs` with:

```rust
use leptos::prelude::*;
use wasm_bindgen::JsCast;

#[component]
pub fn CanvasToolbar(
    search_query: RwSignal<String>,
    on_search: impl Fn(String) + 'static + Copy,
    fold_threshold: ReadSignal<usize>,
    set_fold_threshold: WriteSignal<usize>,
    /// (visible 1-hop count, total 1-hop count) for the "(K of N)" hint
    visible_counts: ReadSignal<(usize, usize)>,
) -> impl IntoView {
    let input_value = RwSignal::new(String::new());

    let on_input = move |ev: web_sys::Event| {
        let target: Option<web_sys::HtmlInputElement> = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
        if let Some(input) = target {
            input_value.set(input.value());
        }
    };

    let on_keydown = move |ev: web_sys::KeyboardEvent| {
        if ev.key() == "Enter" {
            let val = input_value.get();
            search_query.set(val.clone());
            on_search(val);
        }
    };

    let on_slider_input = move |ev: web_sys::Event| {
        let target: Option<web_sys::HtmlInputElement> = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
        if let Some(input) = target {
            let v: usize = input.value().parse().unwrap_or(12);
            set_fold_threshold.set(v);
        }
    };

    view! {
        <div class="flex items-center gap-3 px-4 py-2 bg-surface-raised border-b border-border">
            <div class="flex items-center gap-2 text-sm font-medium text-text-secondary">
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                    stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <circle cx="12" cy="12" r="10"/>
                    <line x1="12" y1="8" x2="12" y2="16"/>
                    <line x1="8" y1="12" x2="16" y2="12"/>
                </svg>
                "Knowledge Graph"
            </div>

            <div class="flex-1" />

            <div class="relative">
                <input
                    type="text"
                    placeholder="Search entities..."
                    class="w-48 px-3 py-1.5 text-sm bg-surface-sunken border border-border rounded-lg
                           text-text-primary placeholder-text-tertiary focus:outline-none focus:border-primary/50"
                    on:input=on_input
                    on:keydown=on_keydown
                />
            </div>

            <div class="flex items-center gap-1.5 text-xs text-text-secondary">
                <span>"Detail"</span>
                <input
                    type="range"
                    min="4"
                    max="30"
                    step="1"
                    class="w-24 accent-primary"
                    prop:value=move || fold_threshold.get().to_string()
                    on:input=on_slider_input
                />
                <span class="w-6 text-center">{move || fold_threshold.get()}</span>
                <span class="text-text-tertiary">
                    {move || {
                        let (k, n) = visible_counts.get();
                        format!("({k} of {n})")
                    }}
                </span>
            </div>
        </div>
    }
}
```

- [ ] **Step 7.2: Add `visible_counts` signal in `RadialCanvasView`**

In `interfaces/webchat/src/views/canvas/mod.rs`, near the other signal declarations in `RadialCanvasView` (around line 75), add:
```rust
let (visible_counts, set_visible_counts) = signal((0usize, 0usize));
```

- [ ] **Step 7.3: Update visible_counts whenever a neighborhood loads**

Inside Effect 1, Effect 2 (cache hit), and Effect 2 (fresh fetch), after the focus_id / focus_neighbors updates added in Task 6, add:
```rust
                    set_visible_counts.set((nbhd.one_hop.len(), nbhd.one_hop.len() + nbhd.clusters.iter().map(|c| c.member_ids.len()).sum::<usize>()));
```

- [ ] **Step 7.4: Update `<CanvasToolbar/>` invocation in `RadialCanvasView` view**

Replace the existing `<CanvasToolbar … />` block (around line 390-398) with:
```rust
            <CanvasToolbar
                search_query=search_query
                on_search=on_search
                fold_threshold=fold_threshold
                set_fold_threshold=set_fold_threshold
                visible_counts=visible_counts
            />
```

- [ ] **Step 7.5: Build and visually check**

```bash
just build
pkill -f "target/release/aleph-server" 2>/dev/null
sleep 2
target/release/aleph-server start
```

Open browser, drag the Detail slider:
- Slider position from 4 to 30 should change visible main-canvas node count
- "(K of N)" updates immediately
- N stays roughly constant for the same center; K monotonically tracks slider

- [ ] **Step 7.6: Commit**

```bash
git add interfaces/webchat/src/views/canvas/toolbar.rs interfaces/webchat/src/views/canvas/mod.rs interfaces/webchat/dist/aleph_panel.js interfaces/webchat/dist/aleph_panel_bg.wasm
git commit -m "$(cat <<'EOF'
canvas(toolbar): drop view toggles, add (K of N) counter

Toolbar is now title + search + Detail slider. The slider range is
4..=30 (default 12) and reads "(K of N)" so users can see exactly
how the threshold maps to visible nodes.
EOF
)"
```

---

## Task 8: Delete LegacyCanvasView, Effect 5, and view-switching root

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/mod.rs`

- [ ] **Step 8.1: Locate and delete Effect 5**

In `interfaces/webchat/src/views/canvas/mod.rs`, find the block beginning with the comment `// Effect 5: fold_threshold change → bust cache, re-issue active_request` (around line 253) and ending at `active_request.set(Some(id));` (around line 281). Delete the entire Effect including its surrounding `let nav_thresh = nav.clone();` and `let prefetch_thresh = prefetch.clone();` setup lines.

- [ ] **Step 8.2: Replace `CanvasView` root with direct radial render**

Find the existing `CanvasView` root component (around line 35-50) and replace its entire body with:
```rust
#[component]
pub fn CanvasView() -> impl IntoView {
    view! { <RadialCanvasView /> }
}
```

Delete:
- The `radial_signal`/`canvas_radial_navigation` reads
- The `match` or `if` selecting between `RadialCanvasView` and `LegacyCanvasView`
- Any `use crate::api::settings::save_canvas_radial_navigation;` if no longer referenced (check with `grep`)

- [ ] **Step 8.3: Delete `LegacyCanvasView`**

Find `fn LegacyCanvasView()` (around line 491) and delete from `#[component]` to the end of its function body (around line 720). Also delete any helper functions only used by it (e.g., `adapt_graph_response` if exclusively legacy — verify with grep first).

- [ ] **Step 8.4: Delete `view_mode`/`set_view_mode` signal and the `on_toggle_mode` closure in RadialCanvasView**

In `RadialCanvasView`:
- Delete the line `let (view_mode, set_view_mode) = signal(ViewMode::Global { top_k: 100 });`
- Delete the `let on_toggle_mode = move || …` closure (around line 325-332)
- Delete any `use crate::canvas_engine::types::ViewMode;` if only this file used it

For places that wrote to `set_view_mode` (e.g., `EnterLocalView` in `on_event`, `on_search`, `on_breadcrumb_navigate`), delete only the `set_view_mode.set(...)` lines but keep the `set_breadcrumb` and `active_request.set(Some(id))` lines — those still drive the radial flow.

- [ ] **Step 8.5: Sweep for stale imports / dead code**

```bash
cargo check -p aleph-panel --target wasm32-unknown-unknown 2>&1 | grep -E "warning|error"
```
Expected: no errors. Address warnings about unused imports by deleting them. Pre-existing unrelated warnings can remain.

- [ ] **Step 8.6: Run all canvas tests**

```bash
cargo test -p aleph-panel --lib canvas_engine
cargo test -p aleph-panel --lib views::canvas
```
Expected: all tests pass.

- [ ] **Step 8.7: Commit**

```bash
git add interfaces/webchat/src/views/canvas/mod.rs
git commit -m "$(cat <<'EOF'
canvas: delete LegacyCanvasView, Effect 5, and view switching

Single radial paradigm. ViewMode enum stays in types.rs unused for
one release to avoid downstream churn; canvas_radial_navigation
remains in DashboardState as a no-op for the same reason.
EOF
)"
```

---

## Task 9: Final verification + clean build

**Files:** none modified

- [ ] **Step 9.1: Clean release build**

```bash
just build
```
Expected: WASM bundle rebuilt; no errors.

- [ ] **Step 9.2: Restart server**

```bash
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
sleep 2
ps aux | grep "[a]leph-server" | grep -v cp | grep -v tail
# Expected: no processes listed
target/release/aleph-server start &
```
Expected: server listens on 18790, exactly one process.

- [ ] **Step 9.3: Manual integration tests in browser**

Open `http://127.0.0.1:18790/memory`. Run through this checklist:

1. **Initial load**: minimap visible bottom-right; ~500 colored points; one outlined dot for Radial center.
2. **Detail slider 4 → 30**: main canvas node count changes monotonically; "(K of N)" updates in real time.
3. **MiniMap click**: clicking a far point recenters the Radial view; minimap focus marker follows.
4. **Search**: typing in search → Enter → result re-centers Radial + breadcrumb updates + minimap focus follows.
5. **Hover prefetch**: hover a 1-hop node 200ms then click → no perceptible fetch latency (cache hit).
6. **Breadcrumb navigation**: click an earlier breadcrumb entry → Radial returns to that center.
7. **No console errors**: open DevTools Console; reload; verify no runtime errors.

- [ ] **Step 9.4: Clean exit and report**

If all manual tests pass, confirm:
- All 9 tasks committed
- `cargo test -p aleph-panel --lib` exits 0
- `cargo check -p aleph-panel --target wasm32-unknown-unknown` exits 0
- Server runs without panics

If any manual test fails, return to the relevant task and re-iterate. Do NOT mark this plan complete until all 7 manual tests pass.

- [ ] **Step 9.5: Commit final cleanup if any incidental fixes needed**

If incidental fixes were made during manual testing (e.g., a CSS class typo), commit them under their own message:
```bash
git add <files>
git commit -m "canvas: fix <specific issue from manual testing>"
```

---

## Self-Review

**Spec coverage check:**

| Spec section | Implementing task |
|---|---|
| Architecture: Delete legacy | Task 8 |
| GlobalMiniMap data | Task 4 |
| GlobalMiniMap render | Task 5 |
| GlobalMiniMap wire-in | Task 6 |
| Top-K folding | Task 2 |
| `group_by_category_into_clusters` | Task 2 |
| Slider range 4..=30 + (K of N) | Task 7 |
| PrefetchCache (id, threshold) key | Task 1 |
| Delete `clear()` | Task 1 |
| Delete `Effect 5` | Task 8 |
| Delete `fold_sector` / `fallback_fold` | Task 3 |
| Tests for top-K | Task 2 |
| Tests for minimap | Task 4 |
| Tests for cache key | Task 1 |
| Manual integration verification | Task 9 |

No gaps detected.

**Type consistency check:**
- `GlobalMiniMap::build(&[NoteNodeDto], &[NoteLinkDto], f32)` used in Task 4 (definition), Task 6 (call). ✓
- `GlobalMiniMap::pick_at(f32, f32, f32) -> Option<&str>` used in Task 4 (definition), Task 6 (call). ✓
- `PrefetchCache::put(String, usize, Neighborhood)` used in Task 1 (definition), Task 1 caller updates, Effect 4 in mod.rs. ✓
- `group_by_category_into_clusters(Vec<CanvasNode>, &str) -> Vec<ClusterNode>` used in Task 2 (definition), adapter.rs call. ✓
- `visible_counts: ReadSignal<(usize, usize)>` used in Task 7 (toolbar prop), set in Task 7 (RadialCanvasView writes). ✓
- `focus_id: ReadSignal<Option<String>>` and `focus_neighbor_ids: ReadSignal<Vec<String>>` used in Task 6 (component prop), set in Task 6 (RadialCanvasView writes). ✓

**Placeholder scan:**
No "TBD" / "fill in" / "similar to Task N" / unimplemented references. Each step provides actual code or actual commands.

---

## Open Items the Engineer Should Watch For

1. **`Color` field access**: if `Color::r/g/b` are not `pub`, add accessors in `types.rs`. If `Color::new(r, g, b)` does not exist, use the actual constructor.
2. **`NoteNodeDto::default()`**: if the struct does not derive `Default`, the test helpers must build it explicitly. Use the actual fields from `api/memory.rs`.
3. **`NoteLinkDto` field names**: confirm `from`/`to` match (they did in the latest read). Adjust if renamed.
4. **`GraphNeighborsResponse::hop_depth`**: confirm field type is `HashMap<String, u8>` (it was in `adapter.rs`); test helpers must match.
5. **`RadialCanvasView` line numbers** in this plan are approximate (based on a snapshot of `mod.rs`). Use the surrounding comments and structure as anchors.

