# 记忆 Hub 控件归位 + Fold 滑块修复 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把记忆 Hub 的 agent 选择器/图谱列表切换/搜索从顶部工具栏迁入左侧栏、删除左侧栏重复的节点详情面板、把右侧整块归还给 Canvas，并修复失效的 Fold 滑块。

**Architecture:** 纯 Aleph Panel（Leptos/WASM, crate `aleph-panel`）UI 重组，零 Core/IPC 改动（R4 纯 I/O）。新建 `views/memory_hub/sidebar.rs` 承载竖排控件，删除 `views/memory_hub/toolbar.rs`，`mode_sidebar.rs` 的 Memory 分支委托新组件；Fold 滑块经一个可单测的纯函数 `fold_to_lod` 重映射，并把默认/重置值统一到 `DEFAULT_FOLD` 常量。

**Tech Stack:** Rust + Leptos 0.8（`view!` 宏 / `RwSignal` / `Memo` / `#[component]`）、Tailwind（`nav-tile` / `nav-tile-active` 既有类）、`web_sys`（host 编译 stub，使纯逻辑可 `cargo test`）、`just wasm`（wasm-bindgen + wasm-opt + dist 重建）。

## Global Constraints

- 回复中文、代码注释英文（用户约定）。
- 极度节制 cargo 调用：每个代码任务用 `cargo check -p aleph-panel`（host，快）做编译门；`just wasm` 与 `cargo test -p aleph-panel --lib` 在最终验证任务集中跑一次。
- 不新增 i18n 键：复用 `memory.hub_view_graph` / `memory.hub_view_table` / `memory.search_placeholder`（en.json:310/311/271、zh.json 对应行均已存在）。
- 不碰 Core / IPC / WebGL 渲染 / `node_detail_panel.rs` 内部 / 列表表格内容（非目标）。
- 不为 "Fold" 标签引入 i18n（沿用字面量）。
- 部署不在范围：rust_embed 在 `aleph-server` 编译期嵌入 dist，部署是用户单独拍板的一步。
- 提交：默认不 push；本计划只到本地 commit。

---

## File Structure（决策锁定）

| 文件 | 责任 | 本计划动作 |
|------|------|-----------|
| `interfaces/webchat/src/views/memory_hub/sidebar.rs` | Memory 模式左侧栏竖排控件（agent / 图谱·列表 tile / 搜索 / Fold） | **新建**（Task 2） |
| `interfaces/webchat/src/views/memory_hub/mod.rs` | Hub 宿主：视图切换 + 内容区 | 去工具栏、声明/导出 sidebar、Canvas 满幅（Task 3） |
| `interfaces/webchat/src/views/memory_hub/toolbar.rs` | （旧）顶部工具栏 | **删除**（Task 3） |
| `interfaces/webchat/src/components/mode_sidebar.rs` | App 外壳左列 + 各模式子菜单 | Memory 分支委托新组件、删旧 `fn MemorySidebar`（Task 3） |
| `interfaces/webchat/src/views/canvas/mod.rs` | Canvas 宿主 + Fold→LOD 映射 + agent 重置 | 加 `fold_to_lod` + 改 LOD Effect + 重置用 `DEFAULT_FOLD`（Task 1） |
| `interfaces/webchat/src/state/memory.rs` | 共享 Memory 状态 | 加 `DEFAULT_FOLD` 常量 + 默认值用它（Task 1） |

---

## Task 1: 修复 Fold→LOD 映射 + 统一 DEFAULT_FOLD 常量

**Files:**
- Modify: `interfaces/webchat/src/state/memory.rs:78`（默认值）+ 新增常量
- Modify: `interfaces/webchat/src/views/canvas/mod.rs:147`（agent 重置）、`:302-316`（LOD Effect）+ 新增纯函数与单测
- Test: `interfaces/webchat/src/views/canvas/mod.rs` 内的 `#[cfg(test)] mod tests`

**Interfaces:**
- Produces: `pub const DEFAULT_FOLD: usize`（in `state::memory`）；`fn fold_to_lod(fold: usize) -> f32`（private in `canvas::mod`）。
- Consumes: 既有 `mem.fold_threshold: RwSignal<usize>`、`lod_request: RwSignal<f32>`。

- [ ] **Step 1: 写失败测试**（加到 `views/canvas/mod.rs` 的 `#[cfg(test)] mod tests` 末尾，紧跟现有 `dedup_drops_self_loops` 之后）

```rust
    #[test]
    fn fold_to_lod_spans_full_visible_range() {
        // Full slider travel (0..=10) must cover the full LOD range so the
        // control is visibly effective (the old 0..10→[0.991,1.0] map did not).
        assert_eq!(fold_to_lod(0), 1.0); // sparsest: backbone only
        assert_eq!(fold_to_lod(10), 0.0); // densest: all edges
        assert_eq!(fold_to_lod(5), 0.5); // midpoint
        // Monotonic decreasing: higher slider = denser graph (lower lod).
        assert!(fold_to_lod(2) > fold_to_lod(8));
        // Out-of-range slider values clamp instead of overflowing the LOD range.
        assert_eq!(fold_to_lod(99), 0.0);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p aleph-panel --lib fold_to_lod_spans_full_visible_range`
Expected: 编译失败 —— `cannot find function fold_to_lod in this scope`。

- [ ] **Step 3: 实现纯函数**（加到 `views/canvas/mod.rs` 的私有 helpers 区，紧跟 `dedup_undirected_edges` 函数之后，即第 409 行 `}` 之后）

```rust
/// Map the Fold slider value (UI range 0..=10) to an edge-density LOD in [0,1]
/// for the galaxy renderer. Higher slider = denser graph: `fold=0` → lod 1.0
/// (only the ~90th-percentile backbone survives `Scene::recompute_filtered_edges`),
/// `fold=10` → lod 0.0 (all edges). The full slider travel spans the full LOD
/// range, replacing the old `1.0 - (ft-1)/999` map whose 0..10 input only
/// produced lod∈[0.991,1.0] (visibly no change).
fn fold_to_lod(fold: usize) -> f32 {
    let ft = fold.min(10) as f32;
    (1.0 - ft / 10.0).clamp(0.0, 1.0)
}
```

- [ ] **Step 4: 把 LOD Effect 改用纯函数**

把 `views/canvas/mod.rs:312-316` 现有 Effect：

```rust
    Effect::new(move || {
        let ft = fold_threshold.get().clamp(1, 1000) as f32;
        let lod = 1.0 - (ft - 1.0) / 999.0;
        lod_request.set(lod);
    });
```

替换为：

```rust
    Effect::new(move || {
        lod_request.set(fold_to_lod(fold_threshold.get()));
    });
```

同时把该 Effect 上方的注释块（`mod.rs:301-311`，描述旧 1..1000 映射的那段）替换为：

```rust
    // -----------------------------------------------------------------------
    // Fold slider → LOD mapping Effect: fold_threshold (0..=10) → lod (0..1)
    // via `fold_to_lod`. Higher slider = denser graph. The retired cluster-fold
    // semantics are reused purely as an edge-density knob; the slider's full
    // travel now spans the full LOD range (see `fold_to_lod`).
    // -----------------------------------------------------------------------
```

- [ ] **Step 5: 统一 agent 切换重置值**

把 `views/canvas/mod.rs:148` 的 `set_fold_threshold.set(12);` 改为：

```rust
                set_fold_threshold.set(DEFAULT_FOLD);
```

并把文件顶部第 17 行的 import：

```rust
use crate::state::memory::{MemoryState, MemoryView};
```

改为：

```rust
use crate::state::memory::{MemoryState, MemoryView, DEFAULT_FOLD};
```

- [ ] **Step 6: 加 DEFAULT_FOLD 常量 + 统一默认值**

在 `state/memory.rs` 的 `RECENT_VISITED_CAPACITY` 常量（第 10 行）之后新增：

```rust
/// Default Fold-slider value (UI range 0..=10), shared by `MemoryState::new`
/// and the canvas agent-switch reset so the stored value never lands outside
/// the slider range. Maps to a balanced mid-density view (lod 0.5).
pub const DEFAULT_FOLD: usize = 5;
```

把 `state/memory.rs:78` 的 `fold_threshold: RwSignal::new(3),` 改为：

```rust
            fold_threshold: RwSignal::new(DEFAULT_FOLD),
```

- [ ] **Step 7: 跑测试确认通过**

Run: `cargo test -p aleph-panel --lib fold_to_lod_spans_full_visible_range`
Expected: PASS。
（顺带跑既有 canvas 测试不回归：`cargo test -p aleph-panel --lib canvas` 应全绿。）

- [ ] **Step 8: 提交**

```bash
git add interfaces/webchat/src/state/memory.rs interfaces/webchat/src/views/canvas/mod.rs
git commit -m "panel: fix Fold slider LOD mapping + unify DEFAULT_FOLD"
```

---

## Task 2: 新建 Memory 左侧栏控件组件 `views/memory_hub/sidebar.rs`

**Files:**
- Create: `interfaces/webchat/src/views/memory_hub/sidebar.rs`
- Modify: `interfaces/webchat/src/views/memory_hub/mod.rs:12-13`（模块声明/导出）

**Interfaces:**
- Produces: `pub fn MemorySidebar() -> impl IntoView`（in `views::memory_hub`），无参，读写 `MemoryState`：`memory_view` / `agent_id` / `agents` / `search_query` / `search_nonce` / `fold_threshold`。
- Consumes: 既有 `MemoryState` context、i18n 键 `memory.hub_view_graph/hub_view_table/search_placeholder`、Tailwind 类 `nav-tile`/`nav-tile-active`。

> 说明：本任务仅“创建并导出”新组件，尚不接线（Task 3 才替换）。`pub use` 使其为可达 pub 项，不触发 dead_code 警告。此时它与旧 `toolbar.rs` 暂时并存，均可编译。

- [ ] **Step 1: 创建 `views/memory_hub/sidebar.rs`（完整内容）**

```rust
//! Memory-mode left sidebar controls — agent selector, graph/list view toggle,
//! search box, and the Fold (edge-density) slider, stacked top-to-bottom. Pure
//! I/O: reads/writes `MemoryState` only (R4). Replaces both the former
//! `MemoryToolbar` (which sat atop the canvas) and the old `NodeDetailPanel`
//! sidebar instance, leaving the canvas overlay as the single node-detail surface.

use leptos::prelude::*;

use crate::i18n::{t_string, use_i18n};
use crate::state::memory::{MemoryState, MemoryView};

#[component]
#[must_use]
pub fn MemorySidebar() -> impl IntoView {
    let mem = expect_context::<MemoryState>();
    let i18n = use_i18n();
    let is_graph = Memo::new(move |_| mem.memory_view.get() == MemoryView::Graph);
    // Agent picker popover visibility — closes on mouse-leave, mirroring the
    // chat sidebar / former toolbar picker.
    let agent_open = RwSignal::new(false);

    view! {
        <div class="flex flex-col h-full">
            // ── Agent selector (popover drops downward; selector sits at top) ──
            <div class="px-3 pt-3 pb-2">
                <div class="relative">
                    <button
                        type="button"
                        class="w-full flex items-center gap-2 px-3 py-1.5 rounded-lg bg-surface-raised \
                               border border-border text-sm text-text-primary hover:border-primary/60 \
                               focus:outline-none focus:ring-2 focus:ring-primary/30 transition-colors"
                        on:click=move |_| agent_open.update(|v| *v = !*v)
                    >
                        <span class="flex-1 min-w-0 truncate text-left">
                            {move || {
                                let id = mem.agent_id.get();
                                mem.agents.get().iter().find(|a| a.id == id)
                                    .map(|a| a.name.as_deref()
                                        .map(|n| if let Some(e) = a.emoji.as_deref() { format!("{e} {n}") } else { n.to_string() })
                                        .unwrap_or_else(|| a.id.clone()))
                                    .unwrap_or(id)
                            }}
                        </span>
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                             stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"
                             class=move || if agent_open.get() {
                                 "flex-shrink-0 text-text-tertiary rotate-180 transition-transform"
                             } else {
                                 "flex-shrink-0 text-text-tertiary transition-transform"
                             }
                        >
                            <polyline points="6 9 12 15 18 9" />
                        </svg>
                    </button>

                    <Show when=move || agent_open.get()>
                        <div class="glass animate-pop-in absolute top-full left-0 right-0 mt-2 z-50 \
                                    max-h-[60vh] overflow-y-auto rounded-xl border border-border \
                                    bg-surface-overlay/85 shadow-xl p-1.5 space-y-0.5"
                            on:mouseleave=move |_| agent_open.set(false)>
                            {move || {
                                let cur = mem.agent_id.get();
                                let agents = mem.agents.get();
                                if agents.is_empty() {
                                    return view! {
                                        <div class="px-3 py-2 text-sm text-text-tertiary truncate">{cur.clone()}</div>
                                    }.into_any();
                                }
                                agents.into_iter().map(|a| {
                                    let id = a.id.clone();
                                    let id_for_click = id.clone();
                                    let label = a.name.as_deref()
                                        .map(|n| if let Some(e) = a.emoji.as_deref() { format!("{e} {n}") } else { n.to_string() })
                                        .unwrap_or_else(|| a.id.clone());
                                    let is_selected = id == cur;
                                    view! {
                                        <button
                                            type="button"
                                            class=move || {
                                                let base = "w-full flex items-center gap-2 px-3 py-2 \
                                                            rounded-lg text-sm text-left";
                                                if is_selected { format!("{base} nav-tile-active") } else { format!("{base} nav-tile") }
                                            }
                                            on:click=move |_| { agent_open.set(false); mem.agent_id.set(id_for_click.clone()); }
                                        >
                                            <span class="flex-1 min-w-0 truncate">{label}</span>
                                            {is_selected.then(|| view! {
                                                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                                                     stroke-width="3" stroke-linecap="round" stroke-linejoin="round"
                                                     class="flex-shrink-0 text-primary">
                                                    <polyline points="20 6 9 17 4 12" />
                                                </svg>
                                            })}
                                        </button>
                                    }
                                }).collect_view().into_any()
                            }}
                        </div>
                    </Show>
                </div>
            </div>

            // ── Graph / List view toggle — two vertical nav tiles ──
            <div class="px-3 py-1 space-y-0.5">
                <button
                    class=move || if is_graph.get() {
                        "nav-tile-active w-full flex items-center gap-3 px-3 py-2 rounded-lg text-sm"
                    } else {
                        "nav-tile w-full flex items-center gap-3 px-3 py-2 rounded-lg text-sm"
                    }
                    on:click=move |_| mem.memory_view.set(MemoryView::Graph)
                >
                    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
                         class=move || if is_graph.get() { "text-sidebar-accent flex-shrink-0" } else { "text-text-tertiary flex-shrink-0" }
                    >
                        <circle cx="5" cy="6" r="2" /><circle cx="19" cy="6" r="2" /><circle cx="12" cy="18" r="2" />
                        <line x1="6.6" y1="7.4" x2="10.6" y2="16.4" /><line x1="17.4" y1="7.4" x2="13.4" y2="16.4" /><line x1="7" y1="6" x2="17" y2="6" />
                    </svg>
                    <span>{move || t_string!(i18n, memory.hub_view_graph).to_string()}</span>
                </button>
                <button
                    class=move || if is_graph.get() {
                        "nav-tile w-full flex items-center gap-3 px-3 py-2 rounded-lg text-sm"
                    } else {
                        "nav-tile-active w-full flex items-center gap-3 px-3 py-2 rounded-lg text-sm"
                    }
                    on:click=move |_| mem.memory_view.set(MemoryView::Table)
                >
                    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
                         class=move || if is_graph.get() { "text-text-tertiary flex-shrink-0" } else { "text-sidebar-accent flex-shrink-0" }
                    >
                        <line x1="8" y1="6" x2="21" y2="6" /><line x1="8" y1="12" x2="21" y2="12" /><line x1="8" y1="18" x2="21" y2="18" />
                        <line x1="3" y1="6" x2="3.01" y2="6" /><line x1="3" y1="12" x2="3.01" y2="12" /><line x1="3" y1="18" x2="3.01" y2="18" />
                    </svg>
                    <span>{move || t_string!(i18n, memory.hub_view_table).to_string()}</span>
                </button>
            </div>

            // ── Search — live writes search_query; Enter bumps search_nonce ──
            <div class="px-3 py-2">
                <div class="relative">
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
                         class="absolute left-2.5 top-1/2 -translate-y-1/2 text-text-tertiary"
                    >
                        <circle cx="11" cy="11" r="8" />
                        <path d="m21 21-4.35-4.35" />
                    </svg>
                    <input
                        type="search"
                        placeholder=t_string!(i18n, memory.search_placeholder)
                        class="w-full pl-8 pr-2 py-1.5 rounded-md bg-surface-sunken border border-border text-sm text-text-primary placeholder:text-text-tertiary focus:outline-none focus:border-primary/60 focus:ring-1 focus:ring-primary/30"
                        prop:value=move || mem.search_query.get()
                        on:input=move |ev| mem.search_query.set(event_target_value(&ev))
                        on:keydown=move |ev| { if ev.key() == "Enter" { mem.search_nonce.update(|n| *n += 1); } }
                    />
                </div>
            </div>

            // ── Fold slider (edge-density knob; see canvas `fold_to_lod`) ──
            <div class="px-3 pt-2 pb-3">
                <label style="font-size:9.5px;color:var(--color-text-secondary);text-transform:uppercase;letter-spacing:0.05em">
                    "Fold"
                </label>
                <input
                    type="range" min="0" max="10" step="1"
                    class="w-full mt-1 accent-[#a78bfa]"
                    prop:value=move || mem.fold_threshold.get() as i32
                    on:input=move |ev| {
                        if let Ok(v) = event_target_value(&ev).parse::<usize>() {
                            mem.fold_threshold.set(v);
                        }
                    }
                />
            </div>
        </div>
    }
}
```

- [ ] **Step 2: 在 `memory_hub/mod.rs` 声明并导出新模块**

把 `views/memory_hub/mod.rs:12-13`：

```rust
mod toolbar;
use toolbar::MemoryToolbar;
```

改为（本任务先“增”不“删”，与旧 toolbar 暂时并存）：

```rust
mod sidebar;
mod toolbar;
pub use sidebar::MemorySidebar;
use toolbar::MemoryToolbar;
```

- [ ] **Step 3: 编译门**

Run: `cargo check -p aleph-panel`
Expected: 编译通过；无 `MemorySidebar` 相关 unused 警告（`pub use` 使其为 pub 可达项）。旧 `MemoryToolbar` 仍被 mod.rs 使用，无警告。

- [ ] **Step 4: 提交**

```bash
git add interfaces/webchat/src/views/memory_hub/sidebar.rs interfaces/webchat/src/views/memory_hub/mod.rs
git commit -m "panel: add Memory-mode sidebar controls component"
```

---

## Task 3: 接线新组件 + 删旧工具栏/旧侧栏 + Canvas 满幅

**Files:**
- Modify: `interfaces/webchat/src/components/mode_sidebar.rs`（Memory 分支 + 删 `fn MemorySidebar`）
- Modify: `interfaces/webchat/src/views/memory_hub/mod.rs`（去 toolbar、容器简化）
- Delete: `interfaces/webchat/src/views/memory_hub/toolbar.rs`

**Interfaces:**
- Consumes: Task 2 的 `views::memory_hub::MemorySidebar`。
- Produces: 最终布局——左侧栏 = 新控件；右侧 = Canvas/列表满幅，无顶部工具栏。

- [ ] **Step 1: `mode_sidebar.rs` Memory 分支委托新组件**

在 `mode_sidebar.rs` 顶部 import 区（第 9-19 行附近，与其它 `use super::`/`use crate::` 并列）新增：

```rust
use crate::views::memory_hub::MemorySidebar;
```

`mode_sidebar.rs:72` 的 Memory 分支保持不变（仍为 `PanelMode::Memory => view! { <MemorySidebar /> }.into_any(),`）——它现在解析到新导入的外部组件。

- [ ] **Step 2: 删除 `mode_sidebar.rs` 内旧 `fn MemorySidebar`**

删除 `mode_sidebar.rs:150-181` 整段（含 doc 注释 `/// Memory mode sidebar — fold threshold slider and node detail panel.` 到该 `fn MemorySidebar` 的闭合 `}`）：

```rust
/// Memory mode sidebar — fold threshold slider and node detail panel.
/// Agent selector and search now live in the hub toolbar.
#[component]
fn MemorySidebar() -> impl IntoView {
    use crate::state::memory::MemoryState;
    use crate::views::canvas::{NodeDetailPanel, NodeExcerpt};
    use std::collections::HashMap;

    let mem = expect_context::<MemoryState>();
    let excerpts: RwSignal<HashMap<String, NodeExcerpt>> = RwSignal::new(Default::default());

    view! {
        <div class="flex flex-col h-full">
            <div class="px-3 pb-2">
                <label style="font-size:9.5px;color:var(--color-text-secondary);text-transform:uppercase;letter-spacing:0.05em">
                    "Fold"
                </label>
                <input
                    type="range" min="0" max="10" step="1"
                    class="w-full mt-1 accent-[#a78bfa]"
                    prop:value=move || mem.fold_threshold.get() as i32
                    on:input=move |ev| {
                        if let Ok(v) = event_target_value(&ev).parse::<usize>() {
                            mem.fold_threshold.set(v);
                        }
                    }
                />
            </div>
            <NodeDetailPanel excerpts=excerpts />
        </div>
    }
}
```

> 注：该 fn 用到的 `NodeDetailPanel`/`NodeExcerpt`/`HashMap` import 全是**函数体内局部** `use`（行 154-156），随 fn 删除一并消失；顶层第 16 行的 `use crate::state::memory::MemoryState;` 仍被 `SidebarBrand`（行 96）使用，**保留**。`canvas/mod.rs:6` 的 `pub use ... NodeDetailPanel, NodeExcerpt` 仍被 Canvas overlay 使用，**保留**。

- [ ] **Step 3: `memory_hub/mod.rs` 去工具栏 + 容器简化 + Canvas 满幅**

把 `views/memory_hub/mod.rs:12-15`：

```rust
mod sidebar;
mod toolbar;
pub use sidebar::MemorySidebar;
use toolbar::MemoryToolbar;
```

改为：

```rust
mod sidebar;
pub use sidebar::MemorySidebar;
```

并删除现在未使用的 import `use crate::views::memory::Memory;`？—— 不删，`Memory` 仍在内容区使用。检查 `CanvasView` import 亦保留。

把 `views/memory_hub/mod.rs:34-52` 的 view 块：

```rust
    view! {
        <div class="flex flex-col h-full min-h-0">
            <MemoryToolbar />
            <div class="flex-1 min-h-0 relative">
                <div
                    class="absolute inset-0"
                    style:display=move || if is_graph.get() { "block" } else { "none" }
                >
                    <CanvasView />
                </div>
                <div
                    class="absolute inset-0 overflow-y-auto"
                    style:display=move || if is_graph.get() { "none" } else { "block" }
                >
                    <Memory />
                </div>
            </div>
        </div>
    }
```

替换为（删 `<MemoryToolbar/>`，外层退化为单一相对容器，Canvas 满幅）：

```rust
    view! {
        <div class="h-full min-h-0 relative">
            <div
                class="absolute inset-0"
                style:display=move || if is_graph.get() { "block" } else { "none" }
            >
                <CanvasView />
            </div>
            <div
                class="absolute inset-0 overflow-y-auto"
                style:display=move || if is_graph.get() { "none" } else { "block" }
            >
                <Memory />
            </div>
        </div>
    }
```

- [ ] **Step 4: 删除 `toolbar.rs`**

```bash
git rm interfaces/webchat/src/views/memory_hub/toolbar.rs
```

- [ ] **Step 5: 编译门**

Run: `cargo check -p aleph-panel`
Expected: 编译通过，零 unused-import / dead_code 警告（旧 `MemorySidebar`/`MemoryToolbar` 已彻底移除，新组件经 mode_sidebar 接线被消费）。

- [ ] **Step 6: 提交**

```bash
git add interfaces/webchat/src/components/mode_sidebar.rs interfaces/webchat/src/views/memory_hub/mod.rs
git commit -m "panel: relocate Memory controls to sidebar, drop hub toolbar, full-bleed canvas"
```

---

## Task 4: 全量构建 + 重建 dist + 目视验证

**Files:** 无代码改动——构建与验证门。

**Interfaces:** Consumes: Task 1-3 全部产物。

> rust_embed 在 `aleph-server` 编译期把 `interfaces/webchat/dist/*` 静态嵌入；`just wasm` 重建 dist 是“改完 panel 能看到效果”的必经步骤。本任务只重建 dist 供 `just dev` 目视，不重编/不部署 server。

- [ ] **Step 1: 全量单测**

Run: `cargo test -p aleph-panel --lib`
Expected: 全绿，含 Task 1 新增 `fold_to_lod_spans_full_visible_range` 与既有 canvas/state 测试。

- [ ] **Step 2: 重建 WASM/dist**

Run: `just wasm`
Expected: 干净编译；末尾出现 dist 配对守卫通过（`✓ panel dist OK`）。

- [ ] **Step 3: 起 dev 目视（人工核对清单）**

Run: `just dev`，浏览到 `/memory`，逐项确认：
1. 左侧栏从上到下 = agent 选择器 → 图谱 → 列表 → 搜索 → Fold；**无**「在列表中查看 / 编辑 / 最近访问」重复块。
2. 「图谱」「列表」两个竖排 tile 点击切换右侧视图，激活态 `nav-tile-active` 高亮正确随视图变化。
3. agent 选择器 popover 向下展开、选择切换驱动 Canvas 与列表重载。
4. 搜索：图谱模式 Enter → 相机飞向命中节点 + 高亮；列表模式 Enter → 提交服务端搜索并切到 Raw facet。
5. 选中节点时 Canvas **右下角**仍浮现节点详情方框（含「在列表中查看 / 编辑」），左侧栏不再有第二份。
6. **Fold 滑块拖动可见连线疏密变化**：左端（0）仅主干、右端（10）全部连线；中段平滑过渡。
7. 右侧 Canvas 满幅；切到「列表」时表格仍有顶部留白（`aleph-content-top`）。
8. （若在 macOS `.app`）顶部无窗口 chrome 冲突；顶部窄条作为窗口拖拽带可接受。

- [ ] **Step 4:（无新代码）确认工作树干净**

Run: `git status`
Expected: 仅 `interfaces/webchat/dist/*` 因 `just wasm` 重建而变化（按既有约定，dist 是否提交随仓库现状处理；本计划默认随后一并 `git add` dist 与最终状态由用户决定是否纳入发版）。

---

## Self-Review

**1. Spec coverage（逐条对照 spec §2 目标）：**
- G1 删左侧 NodeDetailPanel → Task 3 Step 2。✅
- G2 控件迁入左侧栏、顺序 agent→图谱→列表→搜索→Fold → Task 2（组件）+ Task 3（接线）。✅
- G3 删 MemoryToolbar、Canvas 满幅 → Task 3 Step 3-4。✅
- G4 Fold 修复 + 统一常量 → Task 1。✅
- spec §4.4 代码组织（新建 sidebar.rs、删 toolbar.rs、mode_sidebar 委托）→ Task 2/3。✅
- spec §6 macOS 满幅说明 → Task 4 Step 3.8 目视。✅
- spec 非目标（不碰 Core/渲染/列表内容/cluster-folding）→ 计划未触及。✅

**2. Placeholder scan：** 无 TBD/TODO；所有代码步骤给出完整代码块；`DEFAULT_FOLD=5` 为确定值（非占位）。✅

**3. Type consistency：**
- `DEFAULT_FOLD: usize` —— state/memory.rs 定义、canvas/mod.rs 经 `use ...{..., DEFAULT_FOLD}` 消费、`fold_threshold: RwSignal<usize>` 类型匹配。✅
- `fold_to_lod(fold: usize) -> f32` —— 定义于 canvas/mod.rs，被同文件 LOD Effect 与同文件 tests 调用，签名一致。✅
- `MemorySidebar`（`views::memory_hub`，无参 `#[component]`）—— mode_sidebar.rs 经 `use crate::views::memory_hub::MemorySidebar;` 消费，`<MemorySidebar />` 调用一致；旧同名私有 fn 已在 Task 3 删除，无并存歧义。✅
- i18n 键 `memory.hub_view_graph/hub_view_table/search_placeholder` —— 经 §Global Constraints 核实存在。✅

无遗留问题。
