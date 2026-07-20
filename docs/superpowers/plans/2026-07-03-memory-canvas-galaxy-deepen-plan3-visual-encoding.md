# 记忆 Canvas 星系 — Plan 3: 视觉编码 · 社区聚类 + 新鲜度 (WS-3 上半)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development / executing-plans。Steps use `- [ ]`。

**Goal:** 让星系**按社区在空间成簇**（社区质心引力），并让**越新的节点越亮**（新鲜度调制节点亮度）——两者纯 CPU 侧、零 GLSL、可单测，消费 Plan 2 供出的 `community_id` / `updated_at`。

**Architecture:** 全部在 `views/canvas/`：`build_galaxy`（mod.rs）把 DTO 的 `community_id`/`updated_at` 灌进 `GalaxyNode`，新鲜度按亮度缩放 `color`（复用既有 color attr + hdr_boost，无新 GLSL attr location，无 blank-canvas 风险）；`ForceLayout`（gl/layout3d.rs）新增每社区质心引力项。边按 kind 着色因需贯穿 edge/dedup/LOD/upload 对齐，拆到 **Plan 4**。

**Tech Stack:** Rust · gl/layout3d.rs 纯逻辑 native 单测（`cargo test -p aleph-panel --lib`）。

## Global Constraints

- **零 GLSL / 零 attr-location 改动**（避开 [[project-memory-canvas-galaxy-polish]] 的 `layout(location=N)` ↔ `setup_instanced` 对齐铁律 / blank-canvas 坑）。
- **退化安全**：无 `community_id`（冷缓存）→ 质心引力项为 0（等同现状）；无 `updated_at` → 亮度 = 1.0（不变）。
- **视觉 QA 延后**：编译 + 单测在此验证；观感（成簇强度、亮度对比）需用户 `just wasm` 重建 dist 后浏览器看（stale-embed 坑）。
- **编译门**：`cargo test -p aleph-panel --lib`（一次，含新单测 + 既有回归）。**极度节制 cargo**：Plan 3 全部改动一次收尾。
- **提交**：`canvas: <desc>`，不加归属。

---

## Task 1: `GalaxyNode` 携带 community + 新鲜度亮度（build_galaxy）

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/canvas/gl/mod.rs`（`GalaxyNode` 加 `community` 字段 + 测试 helper）
- Modify: `interfaces/webchat/src/platform/wide/views/canvas/mod.rs`（`build_galaxy` + `recency_scale` 纯函数 + 测试）

**Interfaces:**
- Produces: `GalaxyNode.community: Option<u32>`；`fn recency_scale(updated_at: Option<i64>, oldest: i64, newest: i64) -> f32`（None→1.0；newest→1.0；oldest→`RECENCY_FLOOR`）。`build_galaxy` 用它把 `color` 按新鲜度缩放，并把 `community_id` 灌进节点。

- [ ] **Step 1: `GalaxyNode` 加 `community` 字段**

`gl/mod.rs` 的 `GalaxyNode` 改为：

```rust
#[derive(Debug, Clone)]
pub struct GalaxyNode {
    pub id: String,
    pub name: String,
    pub category: String,
    pub link_count: u32,
    pub pos: Vec3,
    /// Base RGB in [0,1] (category color, pre-HDR-boost, recency-scaled).
    pub color: [f32; 3],
    /// Louvain community id (`None` on a cold graph cache). Drives spatial
    /// clustering (community centroid gravity in `ForceLayout`).
    pub community: Option<u32>,
}
```

同文件 `#[cfg(test)] mod highlight_tests` 的 `fn node(...)` 补 `community: None,`（否则测试构造不全编不过）：

```rust
        GalaxyNode {
            id: id.into(),
            name: id.into(),
            category: "x".into(),
            link_count: 0,
            pos: Vec3::zero(),
            color: [1.0, 1.0, 1.0],
            community: None,
        }
```

- [ ] **Step 2: 写 `recency_scale` 失败测试**

`views/canvas/mod.rs` 的 `#[cfg(test)] mod tests` 里加：

```rust
    #[test]
    fn recency_scale_maps_newest_to_full_oldest_to_floor() {
        // None → full brightness.
        assert_eq!(recency_scale(None, 100, 200), 1.0);
        // Equal bounds → full (avoid div-by-zero).
        assert_eq!(recency_scale(Some(150), 200, 200), 1.0);
        // Newest → 1.0, oldest → floor.
        assert!((recency_scale(Some(200), 100, 200) - 1.0).abs() < 1e-6);
        assert!((recency_scale(Some(100), 100, 200) - RECENCY_FLOOR).abs() < 1e-6);
        // Midpoint sits between floor and 1.0.
        let mid = recency_scale(Some(150), 100, 200);
        assert!(mid > RECENCY_FLOOR && mid < 1.0);
        // Out-of-range clamps.
        assert_eq!(recency_scale(Some(50), 100, 200), RECENCY_FLOOR);
    }
```

- [ ] **Step 3: 运行确认失败**

Run: `cargo test -p aleph-panel --lib recency_scale`
Expected: FAIL — `cannot find function recency_scale` / `RECENCY_FLOOR`。

- [ ] **Step 4: 实现 `recency_scale` + 常量**

`views/canvas/mod.rs`（`build_galaxy` 上方）加：

```rust
/// Minimum brightness scale for the oldest note — keeps stale nodes visible
/// while newer ones glow brighter.
const RECENCY_FLOOR: f32 = 0.55;

/// Map a note's `updated_at` to a brightness scale in `[RECENCY_FLOOR, 1.0]`
/// across the graph's [oldest, newest] window. `None` (or a degenerate window)
/// → 1.0 (full brightness, no penalty).
fn recency_scale(updated_at: Option<i64>, oldest: i64, newest: i64) -> f32 {
    let Some(t) = updated_at else {
        return 1.0;
    };
    if newest <= oldest {
        return 1.0;
    }
    let f = ((t - oldest) as f32 / (newest - oldest) as f32).clamp(0.0, 1.0);
    RECENCY_FLOOR + (1.0 - RECENCY_FLOOR) * f
}
```

- [ ] **Step 5: `build_galaxy` 灌 community + 缩放 color**

在 `build_galaxy`（`mod.rs`）里，`let nodes: Vec<GalaxyNode> = ...` 之前算新鲜度窗口：

```rust
    // Recency window across the returned nodes (for brightness scaling).
    let (oldest, newest) = resp
        .nodes
        .iter()
        .filter_map(|n| n.updated_at)
        .fold((i64::MAX, i64::MIN), |(lo, hi), t| (lo.min(t), hi.max(t)));
```

把 `.map(|(n, pos)| GalaxyNode { ... })` 改为：

```rust
        .map(|(n, pos)| {
            let scale = recency_scale(n.updated_at, oldest, newest);
            let base = category_rgb(&n.category);
            GalaxyNode {
                id: n.id.clone(),
                name: n.name.clone(),
                category: n.category.clone(),
                link_count: n.link_count as u32,
                pos,
                color: [base[0] * scale, base[1] * scale, base[2] * scale],
                community: n.community_id,
            }
        })
```

- [ ] **Step 6: 运行测试转绿**

Run: `cargo test -p aleph-panel --lib recency_scale`
Expected: PASS。

---

## Task 2: 社区质心引力（ForceLayout）

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/canvas/gl/layout3d.rs`（`ForceLayout` 存 communities + `step` 加质心引力 + 测试）
- Modify: `interfaces/webchat/src/platform/wide/views/canvas/mod.rs`（`build_galaxy` 传 communities 给 `ForceLayout::new`）
- Modify: `interfaces/webchat/src/platform/wide/views/canvas/gl/scene.rs`（`set_graph` 从 `data.nodes` 取 communities 传 `ForceLayout::new`）

**Interfaces:**
- Consumes: `GalaxyNode.community`（Task 1）。
- Produces: `ForceLayout::new(node_count, edges, communities: &[Option<u32>])`（签名加第三参）；`step` 对有 community 的节点施加朝本社区质心的弱拉力（常量 `COMMUNITY_PULL`）。

- [ ] **Step 1: 写失败测试（同社区更近）**

`gl/layout3d.rs` 测试模块加：

```rust
    #[test]
    fn same_community_settles_closer_than_cross_community() {
        // 4 nodes, NO edges. Communities: {0,1}=A, {2,3}=B. Centroid gravity
        // should pull same-community nodes together despite pure repulsion.
        let ids: Vec<String> = (0..4).map(|i| format!("n{i}")).collect();
        let comms = vec![Some(0u32), Some(0), Some(1), Some(1)];
        let mut l = ForceLayout::new(4, &[], &comms);
        let mut pos = l.seed(&ids);
        for _ in 0..400 {
            l.step(&mut pos);
        }
        let same = pos[0].sub(&pos[1]).length();
        let cross = pos[0].sub(&pos[2]).length();
        assert!(same < cross, "same-community {same} should be < cross {cross}");
    }
```

同时更新既有测试对 `ForceLayout::new` 的调用（它们现在传 2 参）：`ForceLayout::new(10, &line_graph(10))` → `ForceLayout::new(10, &line_graph(10), &vec![None; 10])`（`seed_is_deterministic`/`energy_decreases_over_steps`/`converges_within_budget`/`connected_nodes_closer_than_unconnected` 四处，各按其 n 传 `&vec![None; n]`）。

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p aleph-panel --lib layout3d`
Expected: FAIL（`ForceLayout::new` 参数数不符）。

- [ ] **Step 3: `ForceLayout` 存 communities + 常量**

`gl/layout3d.rs` 顶部加常量（挨着既有常量）：

```rust
const COMMUNITY_PULL: f32 = 0.015; // gravity toward own community centroid
```
`use` 区加：
```rust
use std::collections::HashMap;
```
`struct ForceLayout` 加字段：
```rust
    communities: Vec<Option<u32>>,
```
`new` 改签名 + 初始化：
```rust
    pub fn new(node_count: usize, edges: &[(u32, u32)], communities: &[Option<u32>]) -> ForceLayout {
        ForceLayout {
            n: node_count,
            edges: edges.to_vec(),
            vel: vec![Vec3::zero(); node_count],
            last_max_disp: f32::INFINITY,
            communities: communities.to_vec(),
        }
    }
```

- [ ] **Step 4: `step` 加社区质心引力**

在 `step` 的「Centering + integrate」段之前（即 Springs 之后、`let mut max_disp` 之前）插入：

```rust
        // Community centroid gravity: pull each node toward its community's
        // centroid so Louvain communities settle as spatial clusters. No-op
        // when no node carries a community (cold cache).
        if self.communities.iter().any(Option::is_some) {
            let mut sums: HashMap<u32, (Vec3, usize)> = HashMap::new();
            for (i, c) in self.communities.iter().enumerate() {
                if let Some(cid) = c {
                    let e = sums.entry(*cid).or_insert((Vec3::zero(), 0));
                    e.0 = e.0.add(&pos[i]);
                    e.1 += 1;
                }
            }
            for (i, c) in self.communities.iter().enumerate() {
                if let Some(cid) = c {
                    let (sum, count) = sums[cid];
                    let centroid = sum.scale(1.0 / count as f32);
                    let to_centroid = centroid.sub(&pos[i]);
                    force[i] = force[i].add(&to_centroid.scale(COMMUNITY_PULL));
                }
            }
        }
```

> `self.communities` 长度可能与 `self.n` 不符（防御）：循环用 `self.communities.iter().enumerate()` 且 `i < pos.len()` 隐含成立（communities 由 n 个节点构造）；若担心，`step` 首行可 `let comm = &self.communities;` 并在索引前 `if i >= self.n { continue; }`——此处 communities 恒为 n 长，省略。

- [ ] **Step 5: `build_galaxy` 传 communities**

`mod.rs` 的 `build_galaxy`：把
```rust
    let layout = ForceLayout::new(ids.len(), &edges);
```
改为（node 尚未建，用 DTO 的 community_id 顺序，与 ids 对齐）：
```rust
    let communities: Vec<Option<u32>> = resp.nodes.iter().map(|n| n.community_id).collect();
    let layout = ForceLayout::new(ids.len(), &edges, &communities);
```

- [ ] **Step 6: `scene.rs::set_graph` 传 communities**

`gl/scene.rs` 的 `set_graph`：把
```rust
        let layout = ForceLayout::new(n, &data.edges);
```
改为：
```rust
        let communities: Vec<Option<u32>> = data.nodes.iter().map(|node| node.community).collect();
        let layout = ForceLayout::new(n, &data.edges, &communities);
```

- [ ] **Step 7: 编译门 + 全部单测（一次收尾 Plan 3）**

Run: `cargo test -p aleph-panel --lib`
Expected: 全绿——新增 `recency_scale_*` / `same_community_settles_closer_*` + 既有 layout3d/highlight/canvas 回归全 PASS，零 dead_code 警告。

- [ ] **Step 8: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git add interfaces/webchat/src/platform/wide/views/canvas/
git commit -m "canvas: cluster galaxy by community centroid gravity + brighten by note recency"
```

---

## 完成标准（Plan 3）

- `cargo test -p aleph-panel --lib` 绿（新增 2 组单测：recency 亮度映射、同社区更近）。
- 冷缓存（无 community）行为与现状一致（质心项 no-op、亮度全 1.0）。
- 观感（成簇/亮度）待用户 `just wasm` 浏览器 QA。
- 边按 kind 着色 → Plan 4；pan/性能/intent 抽取 → Plan 5。
