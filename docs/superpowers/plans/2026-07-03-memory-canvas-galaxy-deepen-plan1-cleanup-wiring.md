# 记忆 Canvas 星系 — Plan 1: 死代码清除 + bug 连线 (WS-1 + WS-4)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 删除记忆 Canvas 子系统 ~2,400 行死代码，并修复三处"功能存在但从未连线"的 bug（Recently-visited / backlinks / breadcrumb），不改渲染行为。

**Architecture:** 纯面板层（crate `aleph-panel`）。先做互相级联的死代码删除（一次编译门），再做三处独立的 UI 连线修复。不触 Core、不触 GLSL、不改任何可见渲染逻辑。对应 spec `docs/superpowers/specs/2026-07-03-memory-canvas-galaxy-deepen-design.md` 的 WS-1 与 WS-4（截断提示除外——它依赖 Plan 2 的 `total` 字段）。

**Tech Stack:** Rust + Leptos 0.8 (WASM)，web-sys，原生 `#[cfg(test)]` 纯逻辑单测。

## Global Constraints

- **crate**: 全部改动在 `interfaces/webchat/src/`（crate `aleph-panel`）。
- **编译门命令**: `cargo check -p aleph-panel`（web-sys 在 native 也编译，无需 wasm target）。单测 `cargo test -p aleph-panel --lib <filter>`。
- **极度节制 cargo**（用户铁律）: 删除类任务（1–3）级联互依，**合并为一次 `cargo check` 收尾**即可；连线类任务（4–6）各自 check。不跑全量测试。
- **不改渲染**: 本 plan 不得改变星系任何可见渲染/交互行为，只删死码 + 接三条断线。
- **immutable / 风格**: 遵循既有文件风格；删除即删除，不留注释墓碑（P6）。
- **提交信息**: English，`<scope>: <description>`，scope 用 `canvas:` 或 `panel:`。
- **红线**: R3/R10/P6（YAGNI 撤回，不留口）；R4（不在面板处理业务逻辑——本 plan 不新增业务逻辑）。

---

## Task 1: 清除死掉的 2D canvas engine（级联删除，一次编译门）

**Files:**
- Delete: `interfaces/webchat/src/canvas_engine/json_canvas/convert.rs`
- Delete: `interfaces/webchat/src/canvas_engine/json_canvas/mod.rs`
- Delete: `interfaces/webchat/src/canvas_engine/prefetch.rs`
- Delete: `interfaces/webchat/src/canvas_engine/scatter.rs`
- Delete: `interfaces/webchat/src/canvas_engine/layout.rs`
- Delete: `interfaces/webchat/src/canvas_engine/cluster.rs`
- Delete: `interfaces/webchat/src/canvas_engine/types.rs`
- Modify: `interfaces/webchat/src/canvas_engine/mod.rs`（移除 6 个 `pub mod` 声明）
- Modify: `interfaces/webchat/src/canvas_engine/adapter.rs`（移除死函数 + 死类型 + neighbor 参数/响应）
- Modify: `interfaces/webchat/src/api/graph.rs`（移除 `GraphApi::neighbors` 方法 + import）

**Interfaces:**
- Consumes: 无（纯删除）。
- Produces: 精简后的 `canvas_engine` 只保留 `adapter`(仅活 DTO) / `category_color` / `fnv1a` / `interaction` / `markdown_excerpt`。`adapter.rs` 仍导出 `NoteNodeDto` / `NoteLinkDto` / `GraphQueryResponse` / `NoteDetailResponse` / `SearchResultDto` / `GraphSearchResponse`。

- [ ] **Step 1: 删除 7 个死文件（含 json_canvas 整目录）**

```bash
cd /Volumes/TBU4/Workspace/Aleph/interfaces/webchat/src/canvas_engine
rm -rf json_canvas
rm -f prefetch.rs scatter.rs layout.rs cluster.rs types.rs
```

- [ ] **Step 2: 从 `canvas_engine/mod.rs` 移除死模块声明**

把 `interfaces/webchat/src/canvas_engine/mod.rs` 全文替换为（只留活模块）：

```rust
pub mod adapter;
pub mod category_color;
pub mod fnv1a;
pub mod interaction;
pub mod markdown_excerpt;
```

- [ ] **Step 3: 清理 `adapter.rs` —— 删死函数、死类型、neighbor 参数/响应**

在 `interfaces/webchat/src/canvas_engine/adapter.rs` 中删除以下项（保留仍被消费的 DTO：`NoteNodeDto` / `NoteLinkDto` / `GraphQueryResponse` / `NoteDetailResponse` / `SearchResultDto` / `GraphSearchResponse`）：
- 函数 `adapt_graph_response`、`populate_orphans`、`to_neighborhood` 及它们的 `#[cfg(test)] mod tests` 中**仅测试这些函数**的用例；
- 结构体 `GraphNeighborsResponse`、`GraphNeighborsParams`（若存在）、以及任何 `use` 到已删 `types::{CanvasNode, CanvasEdge, Neighborhood}` / `super::{layout, scatter, cluster, prefetch}` 的导入行；
- 顶部 `use` 块里指向已删模块的行。

> 执行提示：先 `grep -n "CanvasNode\|CanvasEdge\|Neighborhood\|adapt_graph_response\|populate_orphans\|to_neighborhood\|GraphNeighbors\|use super::\(layout\|scatter\|cluster\|prefetch\)\|use crate::canvas_engine::\(types\|layout\|scatter\|cluster\|prefetch\)" interfaces/webchat/src/canvas_engine/adapter.rs`，逐条删除命中行/块。DTO 的 `#[derive]` 与字段保持不动。

- [ ] **Step 4: 从 `api/graph.rs` 移除死方法 `GraphApi::neighbors`**

在 `interfaces/webchat/src/api/graph.rs`：删除整个 `pub async fn neighbors(...) -> Result<GraphNeighborsResponse, String> { ... }` 方法块，并从顶部 `use crate::canvas_engine::adapter::{...}` 里移除 `GraphNeighborsResponse`（和 `GraphNeighborsParams`，若被 import）。其余方法（`query` / `search` / `node_detail` / `update_note`）保持不动。

- [ ] **Step 5: 编译门（本删除簇一次收尾）**

Run: `cargo check -p aleph-panel`
Expected: 编译通过，**无 `unused` / `dead_code` 警告**（若仍有 unused import/type 报错，回到 Step 3/4 补删命中项）。

- [ ] **Step 6: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git add interfaces/webchat/src/canvas_engine interfaces/webchat/src/api/graph.rs
git commit -m "canvas: purge dead 2D canvas engine (json_canvas/prefetch/scatter/layout/cluster/types + neighbors)"
```

---

## Task 2: 清除死信号与 node_card 组件

**Files:**
- Delete: `interfaces/webchat/src/platform/wide/views/canvas/node_card.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/canvas/mod.rs`（移除 `pub mod node_card;` 与 `all_dtos` 信号）
- Modify: `interfaces/webchat/src/state/memory.rs`（移除 `focus_id`、`breadcrumb_entries` 字段 + 初始化）

**Interfaces:**
- Consumes: 无。
- Produces: `MemoryState` 不再含 `focus_id` / `breadcrumb_entries`；`CanvasView` 不再 clone 全量节点到 `all_dtos`。

- [ ] **Step 1: 删除死组件文件**

```bash
rm -f /Volumes/TBU4/Workspace/Aleph/interfaces/webchat/src/platform/wide/views/canvas/node_card.rs
```

- [ ] **Step 2: `views/canvas/mod.rs` 移除 `node_card` 模块声明**

删除 `interfaces/webchat/src/platform/wide/views/canvas/mod.rs` 第 3 行：

```rust
pub mod node_card;
```

- [ ] **Step 3: `views/canvas/mod.rs` 移除死信号 `all_dtos`**

在同文件删除下列三处（第 106-107 行声明、第 149 行 reset、第 177 行 populate 内的 `all_dtos.set(...)`）：

删声明（约 106-107 行）：
```rust
    // Full-graph node cache — populated once on mount, used to compute the
    // ghost-dot ring of orphans (nodes outside the current connected component).
    let all_dtos: RwSignal<Vec<NoteNodeDto>> = RwSignal::new(Vec::new());
```

删 agent-switch reset 里的这一行（约 149 行）：
```rust
                all_dtos.set(Vec::new());
```

删 galaxy-build Effect 里的这一行（约 177 行），保留其后的 `galaxy_data.set(...)`：
```rust
                all_dtos.set(r.nodes.clone());
```

> 删除后若 `NoteNodeDto` 的 `use` 变为未使用，则一并从 `mod.rs` 顶部 import 移除（`grep -n "NoteNodeDto" mod.rs` 复核；`build_galaxy` 仍用 `GraphQueryResponse`，`NoteNodeDto` 可能变孤立）。

- [ ] **Step 4: `state/memory.rs` 移除死字段**

在 `interfaces/webchat/src/state/memory.rs` 删除结构体字段（第 49-50 行）：
```rust
    pub focus_id: RwSignal<Option<String>>,
    pub breadcrumb_entries: RwSignal<Vec<String>>,
```
以及 `new()` 里的初始化（第 85-86 行）：
```rust
            focus_id: RwSignal::new(None),
            breadcrumb_entries: RwSignal::new(Vec::new()),
```

- [ ] **Step 5: 编译门**

Run: `cargo check -p aleph-panel`
Expected: 通过、无 unused 警告。

- [ ] **Step 6: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git add interfaces/webchat/src/platform/wide/views/canvas/mod.rs \
        interfaces/webchat/src/state/memory.rs
git rm interfaces/webchat/src/platform/wide/views/canvas/node_card.rs
git commit -m "canvas: drop unmounted NodeCard + dead all_dtos/focus_id/breadcrumb_entries signals"
```

---

## Task 3: 重命名 RadialCanvasView → GalaxyCanvasView + 清失效注释

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/canvas/mod.rs`

**Interfaces:**
- Consumes: 无。
- Produces: 内部组件 `GalaxyCanvasView`（`pub fn CanvasView` 对外签名不变，只改其内部包裹组件名）。

- [ ] **Step 1: 重命名内部组件**

在 `views/canvas/mod.rs`：
- 将 `fn RadialCanvasView()` 改名为 `fn GalaxyCanvasView()`（约 35 行）；
- 将 `CanvasView` 里的 `view! { <RadialCanvasView /> }` 改为 `view! { <GalaxyCanvasView /> }`（约 26 行）。

- [ ] **Step 2: 清理失效注释**

在同文件删除/更正指向已退役机制的注释：
- 约 225 行 `active_request` 相关注释段（描述"retired radial-fetch path"的 `active_request` 已不存在）——删掉提及 `active_request` 的那几行注释；
- 约 308-316 行 `fold_to_lod` 文档里 "retired cluster-fold semantics" 表述保持（它解释历史，仍准确），但把开头引用 `RadialCanvasView` 的 doc（若有）改为 `GalaxyCanvasView`。

> 仅改注释与名字，不动任何逻辑。

- [ ] **Step 3: 编译门**

Run: `cargo check -p aleph-panel`
Expected: 通过。

- [ ] **Step 4: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git add interfaces/webchat/src/platform/wide/views/canvas/mod.rs
git commit -m "canvas: rename RadialCanvasView -> GalaxyCanvasView, drop stale radial comments"
```

---

## Task 4: 修复 "Recently visited" —— 选中节点时累积

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/canvas/mod.rs`（`on_event::SelectNode` 分支）

**Interfaces:**
- Consumes: `MemoryState::push_recent(&self, id: String)`（已存在于 `state/memory.rs:98`，纯逻辑已被 `push_recent_into` 的单测覆盖）。
- Produces: 每次在星系点击选中节点，`mem.recent_visited` 前插该 id → `NodeDetailPanel` 空态的 "Recently visited" 列表真实累积。

- [ ] **Step 1: 确认既有纯逻辑测试仍在（不新增冗余测试）**

Run: `cargo test -p aleph-panel --lib push_recent`
Expected: PASS（`push_recent_caps_at_8_and_dedupes` 等 3 个用例通过）。这些已覆盖累积/去重/封顶逻辑；本任务只补"调用点"，无需新单测。

- [ ] **Step 2: 在 SelectNode 处调用 push_recent**

在 `views/canvas/mod.rs` 的 `on_event` 闭包 `CanvasEvent::SelectNode(id)` 分支（约 188 行）里，`set_selected_node.set(Some(id.clone()));` 之后加一行：

```rust
        CanvasEvent::SelectNode(id) => {
            set_selected_node.set(Some(id.clone()));
            mem.push_recent(id.clone());
            // Drive the scene via intent channels:
            // 1. Fly camera to selected node.
            focus_request.set(Some(id.clone()));
```

（`mem` 已在组件作用域内 `expect_context`，`id` 是 `String`，`push_recent` 取所有权故 `clone`。）

- [ ] **Step 3: 编译门**

Run: `cargo check -p aleph-panel`
Expected: 通过。

- [ ] **Step 4: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git add interfaces/webchat/src/platform/wide/views/canvas/mod.rs
git commit -m "canvas: populate Recently-visited on node select (push_recent was never called)"
```

---

## Task 5: 在详情面板显示 backlinks

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/canvas/node_detail_panel.rs`

**Interfaces:**
- Consumes: `GraphApi::node_detail` 返回的 `NoteDetailResponse.backlinks: Vec<String>`（`adapter.rs:42`，当前被丢弃）。
- Produces: `NodeExcerpt` 新增 `backlinks: Vec<String>` 字段；`DetailFor` 渲染可点击的 backlinks 列表，点击 = `mem.selected_node.set(Some(id))`（复用既有选中通路）。

- [ ] **Step 1: `NodeExcerpt` 增 `backlinks` 字段**

在 `node_detail_panel.rs` 的 `pub struct NodeExcerpt`（约 13-21 行）加字段：

```rust
#[derive(Clone)]
pub struct NodeExcerpt {
    pub id: String,
    pub name: String,
    pub category: String,
    pub tags: Vec<String>,
    pub body_markdown: String,
    pub breadcrumb: Vec<String>,
    pub backlinks: Vec<String>,
}
```

- [ ] **Step 2: 在 fetch Effect 里填充 backlinks（不再丢弃）**

在 fetch Effect（约 55-62 行）构造 `NodeExcerpt` 处，把 `detail.backlinks` 接上：

```rust
                    let ex = NodeExcerpt {
                        id: detail.node.id.clone(),
                        name: detail.node.name,
                        category: detail.node.category,
                        tags: detail.node.tags,
                        body_markdown: detail.content,
                        breadcrumb: Vec::new(),
                        backlinks: detail.backlinks,
                    };
```

- [ ] **Step 3: 在 `DetailFor` 渲染 backlinks 列表**

在 `DetailFor` 组件里，从 `excerpt` 取 `backlinks`（在 `let tags = excerpt.tags.clone();` 附近，约 108 行加）：

```rust
    let backlinks = excerpt.backlinks.clone();
```

然后在 `view!` 里，tags 区块之后追加（复用 `mem`，点击设选中节点触发既有 fly-to/highlight/detail 通路）：

```rust
            {(!backlinks.is_empty()).then(|| {
                let bl = backlinks.clone();
                view! {
                    <div style="margin-top:10px">
                        <div style="text-transform:uppercase;font-size:9.5px;color:var(--text-meta);letter-spacing:0.05em;margin-bottom:4px">
                            "Backlinks"
                        </div>
                        <ul style="list-style:none;padding:0;margin:0;display:flex;flex-direction:column;gap:3px">
                            {bl.into_iter().map(|id| {
                                let id_click = id.clone();
                                view! {
                                    <li
                                        style="font-size:11px;color:var(--cat-reference);padding:3px 6px;border-radius:4px;background:rgba(96,165,250,0.08);cursor:pointer"
                                        on:click=move |_| mem.selected_node.set(Some(id_click.clone()))
                                    >
                                        {id}
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                    </div>
                }
            })}
```

> `mem` 已在 `DetailFor` 作用域内（`let mem = expect_context::<MemoryState>();`，约 101 行）。若 `mem` 因借用被 move 进多个闭包报错，`MemoryState` 是 `Copy`（`#[derive(Clone, Copy)]`），直接复制即可，无需 clone。

- [ ] **Step 4: 编译门**

Run: `cargo check -p aleph-panel`
Expected: 通过。

- [ ] **Step 5: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git add interfaces/webchat/src/platform/wide/views/canvas/node_detail_panel.rs
git commit -m "canvas: render node backlinks in detail panel (were fetched but dropped)"
```

---

## Task 6: 用 note path 填充 breadcrumb（纯函数 + TDD）

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/canvas/node_detail_panel.rs`

**Interfaces:**
- Consumes: `NoteDetailResponse.node.path: String`（`NoteNodeDto.path`，`adapter.rs:15`）。
- Produces: 纯函数 `fn breadcrumb_from_path(path: &str) -> Vec<String>`（把 `a/b/c.md` → `["a","b"]`，丢文件名与扩展、丢空段），`NoteExcerpt.breadcrumb` 由它填充；既有 breadcrumb `<div>` 恢复显示。

- [ ] **Step 1: 写失败测试**

在 `node_detail_panel.rs` 末尾新增：

```rust
#[cfg(test)]
mod tests {
    use super::breadcrumb_from_path;

    #[test]
    fn breadcrumb_strips_filename_and_splits_dirs() {
        assert_eq!(breadcrumb_from_path("project/aleph/notes/foo.md"),
                   vec!["project", "aleph", "notes"]);
    }

    #[test]
    fn breadcrumb_empty_for_bare_filename() {
        assert_eq!(breadcrumb_from_path("foo.md"), Vec::<String>::new());
        assert_eq!(breadcrumb_from_path(""), Vec::<String>::new());
    }

    #[test]
    fn breadcrumb_ignores_empty_and_dot_segments() {
        assert_eq!(breadcrumb_from_path("/a//b/c.md"), vec!["a", "b"]);
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p aleph-panel --lib breadcrumb`
Expected: FAIL —— `cannot find function breadcrumb_from_path`。

- [ ] **Step 3: 实现纯函数**

在 `node_detail_panel.rs`（`NodeExcerpt` 定义下方、组件之前）加：

```rust
/// Turn a note path like `a/b/c.md` into breadcrumb dir segments `["a","b"]`.
/// Drops the final filename component, empty segments, and `.` segments.
fn breadcrumb_from_path(path: &str) -> Vec<String> {
    let mut segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty() && *s != ".").collect();
    segs.pop(); // drop the filename (last component)
    segs.into_iter().map(str::to_owned).collect()
}
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p aleph-panel --lib breadcrumb`
Expected: PASS（3 用例）。

- [ ] **Step 5: 接进 fetch Effect**

把 Task 5 Step 2 里的 `breadcrumb: Vec::new(),` 改为：

```rust
                        breadcrumb: breadcrumb_from_path(&detail.node.path),
```

> `detail.node.path` 是 `NoteNodeDto.path`（`String`）。既有 `DetailFor` 已有 breadcrumb `<div>`（约 171-175 行），填充后自动显示。

- [ ] **Step 6: 编译门**

Run: `cargo check -p aleph-panel`
Expected: 通过。

- [ ] **Step 7: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git add interfaces/webchat/src/platform/wide/views/canvas/node_detail_panel.rs
git commit -m "canvas: fill detail-panel breadcrumb from note path (was always empty)"
```

---

## 完成标准（Plan 1）

- `cargo check -p aleph-panel` 干净、零 dead_code/unused 警告。
- `cargo test -p aleph-panel --lib`（canvas + memory 相关 filter）全绿。
- 手动 QA（需 `just wasm` 重建 dist——stale-embed 坑）：点击节点后 "Recently visited" 累积；详情面板显示 backlinks + 目录 breadcrumb。
- 后续 Plan 2（WS-2 Core 供数据 + 截断提示）、Plan 3（WS-3 视觉编码）、Plan 4（WS-5 pan/性能/intent 抽取）另行编写。
