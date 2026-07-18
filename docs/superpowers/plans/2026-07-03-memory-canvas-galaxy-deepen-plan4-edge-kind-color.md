# 记忆 Canvas 星系 — Plan 4: 边按关系 kind 着色 (WS-3 下半)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development / executing-plans。

**Goal:** 让不同关系类型的边**视觉可区分**：`wikilink`（结构主干）保持端点渐变色；`semantic`/`related`/`co_recalled`/keyword 各上一个独立色调（受 JSON Canvas 6-preset 分类调色板 + code-call-graph-editor「中性默认 + 每 kind 强调」启发）。消费 Plan 2 供出的 edge `kind`。

**Architecture:** **低 churn 平行数组**——广泛消费的 `GraphData.edges: Vec<(u32,u32)>` **不变**（`ForceLayout`/`compute_highlight_edges`/`dedup`/`set_highlight` 全零改动），新增平行 `edge_kinds: Vec<u8>`，只有 `recompute_filtered_edges`（对齐过滤）与 `upload_indexed`（按 kind 定色）消费它。对齐风险收敛在**一个过滤函数**内，加 `debug_assert!` 长度守卫。**零新 GLSL attr location**（复用既有 col_a/col_b）。

**Tech Stack:** Rust · gl 纯逻辑 native 单测。

## Global Constraints

- `edges: Vec<(u32,u32)>` 类型不变；`edge_kinds` 平行、与 edges 同序同长（`debug_assert_eq!(edges.len(), edge_kinds.len())`）。
- 零 GLSL / 零 attr location 改动（复用 col_a/col_b；wikilink 走原端点色，特殊 kind 覆盖两端为 kind 色）。
- 退化：无 kind（旧响应）→ 全 `wikilink` 码 0 → 观感同现状。
- 视觉观感 QA 延后（`just wasm` + 浏览器）。编译门 `cargo test -p aleph-panel --lib` 一次收尾。
- 提交 `canvas: <desc>`，不加归属。

**Kind 码 + 调色板（`edge_kind_code` / `edge_kind_color`）:**
- `0 wikilink` → 端点渐变（`None`，backbone）
- `1 semantic` → cyan `#22d3ee`
- `2 related` → purple `#a78bfa`
- `3 co_recalled` → amber `#fbbf24`
- `4 other/keyword` → green `#34d399`

---

## Task 1: kind 码 + 调色纯函数（gl/edges.rs）

**Files:** Modify `interfaces/webchat/src/platform/wide/views/canvas/gl/edges.rs`

**Interfaces:**
- Produces: `pub fn edge_kind_code(kind: Option<&str>) -> u8`；`pub fn edge_kind_color(code: u8) -> Option<[f32; 3]>`（`None` = 用端点色）。

- [ ] **Step 1: 失败测试**

edges.rs 测试模块加：

```rust
    #[test]
    fn edge_kind_code_maps_known_relations() {
        assert_eq!(edge_kind_code(None), 0);
        assert_eq!(edge_kind_code(Some("wikilink")), 0);
        assert_eq!(edge_kind_code(Some("semantic")), 1);
        assert_eq!(edge_kind_code(Some("related")), 2);
        assert_eq!(edge_kind_code(Some("co_recalled")), 3);
        assert_eq!(edge_kind_code(Some("keyword-verb-whatever")), 4);
    }

    #[test]
    fn edge_kind_color_backbone_is_none_specials_are_some() {
        assert_eq!(edge_kind_color(0), None); // wikilink → endpoint gradient
        assert!(edge_kind_color(1).is_some()); // semantic tinted
        assert!(edge_kind_color(4).is_some());
        // Out-of-range → None (safe default).
        assert_eq!(edge_kind_color(99), None);
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p aleph-panel --lib edge_kind`
Expected: FAIL（函数不存在）。

- [ ] **Step 3: 实现两纯函数（edges.rs 顶部，`EdgeRenderer` 之前）**

```rust
/// Map a `notes_links.relation` kind string to a compact code.
/// `None`/`"wikilink"` = 0 (plain body wikilink, the structural backbone).
pub fn edge_kind_code(kind: Option<&str>) -> u8 {
    match kind {
        None | Some("wikilink") => 0,
        Some("semantic") => 1,
        Some("related") => 2,
        Some("co_recalled") => 3,
        Some(_) => 4, // keyword/entity verbs and any future kind
    }
}

/// Tint color for an edge kind, or `None` to keep the endpoint-color gradient
/// (used for `wikilink`, the backbone). Specials get a distinct hue so they
/// stand out against the wikilink filaments.
pub fn edge_kind_color(code: u8) -> Option<[f32; 3]> {
    match code {
        1 => Some([0.133, 0.827, 0.933]), // semantic  → cyan  #22d3ee
        2 => Some([0.655, 0.545, 0.980]), // related   → purple #a78bfa
        3 => Some([0.984, 0.749, 0.141]), // co_recalled → amber #fbbf24
        4 => Some([0.204, 0.827, 0.600]), // keyword   → green #34d399
        _ => None,                        // 0 wikilink / unknown → endpoint gradient
    }
}
```

- [ ] **Step 4: 转绿**

Run: `cargo test -p aleph-panel --lib edge_kind`
Expected: PASS。

---

## Task 2: `GraphData` 携带 `edge_kinds` + dedup 保 kind（gl/mod.rs + mod.rs）

**Files:**
- Modify `interfaces/webchat/src/platform/wide/views/canvas/gl/mod.rs`（`GraphData` 加字段 + `highlight_tests` fixture）
- Modify `interfaces/webchat/src/platform/wide/views/canvas/mod.rs`（`build_galaxy` + `dedup_undirected_edges` 带 kind + 测试更新）

**Interfaces:**
- Produces: `GraphData.edge_kinds: Vec<u8>`（与 `edges` 同序同长）；`dedup_undirected_edges(impl Iterator<Item=(u32,u32,u8)>) -> (Vec<(u32,u32)>, Vec<u8>)`（首见保留其 kind）。

- [ ] **Step 1: `GraphData` 加 `edge_kinds`**

`gl/mod.rs`：
```rust
#[derive(Debug, Clone, Default)]
pub struct GraphData {
    pub nodes: Vec<GalaxyNode>,
    pub edges: Vec<(u32, u32)>,
    /// Per-edge relation kind code (see `edges::edge_kind_code`), same order &
    /// length as `edges`. Empty when unknown (treated as all-wikilink).
    pub edge_kinds: Vec<u8>,
}
```
`highlight_tests` 里两处 `GraphData { nodes, edges }` 补 `edge_kinds: vec![0; N]`（`highlight_edges_are_neighbor_links_normalized` 的 3 边 → `vec![0;3]`；`unknown_id_yields_empty` 的 0 边 → `vec![]`）。

- [ ] **Step 2: 更新 `dedup_undirected_edges` 测试（带 kind）**

`mod.rs` 测试里 `dedup_collapses_reciprocal_and_duplicate_edges` / `dedup_drops_self_loops` 改为喂 3 元 + 断言返回 `(edges, kinds)`：

```rust
    #[test]
    fn dedup_collapses_reciprocal_and_duplicate_edges() {
        // (0,1) & (1,0) same undirected; (2,3) twice. Kind of first occurrence wins.
        let directed = [(0u32, 1u32, 1u8), (1, 0, 2), (2, 3, 0), (2, 3, 3), (3, 4, 4)];
        let (edges, kinds) = dedup_undirected_edges(directed.into_iter());
        assert_eq!(edges, vec![(0, 1), (2, 3), (3, 4)]);
        assert_eq!(kinds, vec![1, 0, 4]); // first-seen kind per undirected edge
    }

    #[test]
    fn dedup_drops_self_loops() {
        let directed = [(5u32, 5u32, 1u8), (0, 1, 2)];
        let (edges, kinds) = dedup_undirected_edges(directed.into_iter());
        assert_eq!(edges, vec![(0, 1)]);
        assert_eq!(kinds, vec![2]);
    }
```

- [ ] **Step 3: `dedup_undirected_edges` 带 kind**

`mod.rs`：
```rust
/// Collapse directed link rows into unique undirected edges, carrying each
/// edge's relation-kind code. First appearance wins (edge order + its kind).
fn dedup_undirected_edges(
    directed: impl Iterator<Item = (u32, u32, u8)>,
) -> (Vec<(u32, u32)>, Vec<u8>) {
    let mut seen = std::collections::HashSet::new();
    let mut edges = Vec::new();
    let mut kinds = Vec::new();
    for (a, b, kind) in directed {
        if a == b {
            continue; // degenerate self-loop
        }
        let key = (a.min(b), a.max(b));
        if seen.insert(key) {
            edges.push(key);
            kinds.push(kind);
        }
    }
    (edges, kinds)
}
```

- [ ] **Step 4: `build_galaxy` 喂 kind + 存 edge_kinds**

`mod.rs build_galaxy`：`use` 区确保能拿 `edge_kind_code`（`use gl::edges::edge_kind_code;` 或全限定）。把
```rust
    let edges = dedup_undirected_edges(
        resp.edges
            .iter()
            .filter_map(|e| Some((*id_index.get(&e.from)?, *id_index.get(&e.to)?))),
    );
```
改为：
```rust
    let (edges, edge_kinds) = dedup_undirected_edges(resp.edges.iter().filter_map(|e| {
        Some((
            *id_index.get(&e.from)?,
            *id_index.get(&e.to)?,
            gl::edges::edge_kind_code(e.kind.as_deref()),
        ))
    }));
```
并把结尾 `GraphData { nodes, edges }` 改为 `GraphData { nodes, edges, edge_kinds }`。

- [ ] **Step 5: 转绿（dedup 测试）**

Run: `cargo test -p aleph-panel --lib dedup`
Expected: PASS。

---

## Task 3: `Scene` 过滤 kind + `upload_indexed` 按 kind 定色（gl/scene.rs + gl/edges.rs）

**Files:**
- Modify `interfaces/webchat/src/platform/wide/views/canvas/gl/scene.rs`（`filtered_edge_kinds` 字段 + `recompute_filtered_edges` 对齐过滤 + 3 处 `upload_indexed` 调用）
- Modify `interfaces/webchat/src/platform/wide/views/canvas/gl/edges.rs`（`upload_indexed` 加 `edge_kinds` 参 + 按 kind 覆盖端点色）

**Interfaces:**
- Consumes: `GraphData.edge_kinds`、`edge_kind_color`。
- Produces: `EdgeRenderer::upload_indexed(gl, nodes, edges, edge_kinds)`；`Scene.filtered_edge_kinds` 与 `filtered_edges` 对齐。

- [ ] **Step 1: `Scene` 加 `filtered_edge_kinds` 字段 + 初始化**

`scene.rs`：struct 加 `filtered_edge_kinds: Vec<u8>,`（挨着 `filtered_edges`）；`Scene::new` 的返回补 `filtered_edge_kinds: Vec::new(),`。

- [ ] **Step 2: `recompute_filtered_edges` 对齐产出 kinds**

把 `recompute_filtered_edges` 改为同时产出 `filtered_edge_kinds`（三条 return 路径都要对齐）：

```rust
    fn recompute_filtered_edges(&mut self) {
        // Kinds default to all-0 (wikilink) when the parallel vec is absent/short.
        let kind_at = |i: usize| self.data.edge_kinds.get(i).copied().unwrap_or(0);

        if self.lod <= 0.0 || self.data.nodes.is_empty() {
            self.filtered_edges = self.data.edges.clone();
            self.filtered_edge_kinds = (0..self.data.edges.len()).map(kind_at).collect();
            return;
        }

        let mut counts: Vec<u32> = self.data.nodes.iter().map(|n| n.link_count).collect();
        counts.sort_unstable();
        let idx = ((self.lod * 0.9 * (counts.len().saturating_sub(1)) as f32) as usize)
            .min(counts.len().saturating_sub(1));
        let floor = counts[idx];

        if floor == 0 {
            self.filtered_edges = self.data.edges.clone();
            self.filtered_edge_kinds = (0..self.data.edges.len()).map(kind_at).collect();
            return;
        }

        let mut fe = Vec::new();
        let mut fk = Vec::new();
        for (i, &(a, b)) in self.data.edges.iter().enumerate() {
            let lc_a = self.data.nodes.get(a as usize).map_or(0, |n| n.link_count);
            let lc_b = self.data.nodes.get(b as usize).map_or(0, |n| n.link_count);
            if lc_a >= floor || lc_b >= floor {
                fe.push((a, b));
                fk.push(kind_at(i));
            }
        }
        self.filtered_edges = fe;
        self.filtered_edge_kinds = fk;
        debug_assert_eq!(self.filtered_edges.len(), self.filtered_edge_kinds.len());
    }
```

- [ ] **Step 3: 3 处 `upload_indexed` 调用加 `&self.filtered_edge_kinds`**

`scene.rs` 里三处 `self.edges.upload_indexed(&self.ctx.gl, &self.data.nodes, &self.filtered_edges)`（`set_graph` / `set_lod` / `render` 的 settling 段）都改为：
```rust
        self.edges.upload_indexed(
            &self.ctx.gl,
            &self.data.nodes,
            &self.filtered_edges,
            &self.filtered_edge_kinds,
        );
```

- [ ] **Step 4: `edges.rs::upload_indexed` 按 kind 覆盖端点色**

签名加 `edge_kinds: &[u8]`；循环里 kind 有色则两端用 kind 色：
```rust
    pub fn upload_indexed(
        &mut self,
        gl: &Gl,
        nodes: &[super::GalaxyNode],
        edges: &[(u32, u32)],
        edge_kinds: &[u8],
    ) {
        // ... existing Vec::with_capacity setup ...
        for (i, &(a, b)) in edges.iter().enumerate() {
            let (na, nb) = (&nodes[a as usize], &nodes[b as usize]);
            pos_a.extend_from_slice(&[na.pos.x, na.pos.y, na.pos.z]);
            pos_b.extend_from_slice(&[nb.pos.x, nb.pos.y, nb.pos.z]);
            // Kind tint: specials override both endpoints; wikilink keeps gradient.
            match edge_kinds.get(i).copied().and_then(super::edges::edge_kind_color) {
                Some(c) => {
                    col_a.extend_from_slice(&c);
                    col_b.extend_from_slice(&c);
                }
                None => {
                    col_a.extend_from_slice(&na.color);
                    col_b.extend_from_slice(&nb.color);
                }
            }
            phase_a.push(super::nodes::node_phase(&na.id));
            phase_b.push(super::nodes::node_phase(&nb.id));
        }
        // ... existing self.count + bind_upload calls unchanged ...
    }
```
> 注意：把原 `for &(a, b) in edges` 改成 `for (i, &(a, b)) in edges.iter().enumerate()`（拿 index 查 kind）。`super::edges::edge_kind_color` 即同模块 `edge_kind_color`（在 impl 内可直接 `edge_kind_color`，无需 `super::edges::`——用裸名即可）。

- [ ] **Step 5: 编译门 + 全单测（一次收尾 Plan 4）**

Run: `cargo test -p aleph-panel --lib`
Expected: 全绿（新增 edge_kind_* + 改后的 dedup_* + 既有回归），零警告。

- [ ] **Step 6: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git add interfaces/webchat/src/platform/wide/views/canvas/
git commit -m "canvas: tint edges by relation kind (semantic/related/co_recalled/keyword vs wikilink backbone)"
```

---

## 完成标准（Plan 4）

- `cargo test -p aleph-panel --lib` 绿（新增 edge_kind 码/色 + dedup 保 kind 单测）。
- `edges: Vec<(u32,u32)>` 类型未变（ForceLayout/highlight/set_highlight 零改动）。
- 无 kind（旧响应/冷数据）→ 全 wikilink → 观感同现状（退化安全）。
- 边着色观感待 `just wasm` 浏览器 QA。
