# Phone Memory 下钻重设计 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把手机 Memory tab 从 Vault-only 单列改成"菜单着陆页 → 下钻 Graph/List → note 详情"的多层级全屏结构,手机宽度零左右分屏。

**Architecture:** 复用现有数据层与组件(R4)。新增两个 phone 叶子屏(`menu.rs` 菜单、`graph.rs` 全屏 Canvas),给 `list.rs`/`detail.rs` 加返回,最后把 `PhoneMemory` 路由器的 pathname 分发从两路扩成四路。路由无需注册——`PanelMode::from_path` 对任何 `starts_with("/memory")` 判为 Memory 模式,`PhoneMemory` 内部自行分发。

**Tech Stack:** Rust + Leptos 0.8 (crate `aleph-panel`, WASM)。复用 `CanvasView`(WebGL2 星系图)、`PhoneShell`、`MemoryState`、phone `.cell` 列表样式。

## Global Constraints

- **R4(纯 I/O)**:不新增任何 core/IPC/数据持久化。所有屏只读写已有 `MemoryState` / `PhoneMemoryState` / 现有 API。
- **零新依赖**:不引入任何 crate。
- **不动 `app.rs` 路由与 `shell.rs` TabBar**:Memory tab 已指 `/memory`;`/memory/graph`、`/memory/list` 经 `starts_with("/memory")` 自动归 Memory 模式。
- **PhoneShell footgun**:绝不给 `PhoneShell` 直接传裸 `{move||…}` dynamic block 紧挨 static 兄弟;PhoneShell 的直接子节点必须都是元素(`<div>`)。混合 static+dynamic 兄弟可放在普通 `<div>` 内部。
- **构建门 = `just wasm`**:WASM 编译是唯一权威编译信号(项目 cargo 节制纪律——默认不跑 `cargo check`/全量测试)。每个 task 末尾 `just wasm` 必须绿。单元测试代码须写(编译期校验 + 文档化),`cargo test` 运行为可选。
- **运行时门 = iOS-sim QA**:按 `feedback-ios-panel-test-via-full-macos-app` 流程,在全部 task 完成后做一次(见末尾验收清单)。
- **桌面字节不变**:本计划只动 `platform/phone/memory/*`,不碰 wide 视图。

---

## File Structure

| 文件 | 动作 | 职责 |
|------|------|------|
| `interfaces/webchat/src/platform/phone/memory/menu.rs` | **新建** | `PhoneMemoryMenu`:菜单着陆页(Agent 内联选择器 + 星系图/列表两个下钻行) |
| `interfaces/webchat/src/platform/phone/memory/graph.rs` | **新建** | `PhoneMemoryGraph`:全屏 `CanvasView` + 返回 + Fold 浮层滑杆 |
| `interfaces/webchat/src/platform/phone/memory/list.rs` | 改 | `PhoneShell` 加 `back="/memory"`(其余不动) |
| `interfaces/webchat/src/platform/phone/memory/detail.rs` | 改 | 返回/重定向目标 `/memory` → `/memory/list`,`back_label` → `"List"` |
| `interfaces/webchat/src/platform/phone/memory/mod.rs` | 改 | `pub mod menu; pub mod graph;` + `MemoryScreen` 枚举 + `screen_for_path` 纯函数 + 单测 + 四路分发 |

---

## Task 1: 新建 Graph 全屏屏 (`graph.rs`)

复用桌面 `CanvasView`(无 props,从 context 读 `DashboardState`/`MemoryState`,内部自管 agent 拉取、星系构建、Fold→LOD、节点详情 overlay)。本屏只负责把它装进一个有确定高度的全屏容器,加返回与 Fold 浮层。

**Files:**
- Create: `interfaces/webchat/src/platform/phone/memory/graph.rs`
- Modify: `interfaces/webchat/src/platform/phone/memory/mod.rs:7-9`(模块声明区,加 `pub mod graph;`)

**Interfaces:**
- Consumes: `CanvasView`(`crate::views::canvas::CanvasView`,无 props);`PhoneShell { title, back?, back_label?, children }`;`MemoryState.fold_threshold: RwSignal<usize>`(0..=10)。
- Produces: `pub fn PhoneMemoryGraph() -> impl IntoView`(Task 4 的分发引用)。

- [ ] **Step 1: 写 `graph.rs` 全文**

```rust
//! Phone Graph screen (`/memory/graph`): full-screen WebGL galaxy — reuses the
//! desktop `CanvasView` — with a back button to the Memory menu and a floating
//! Fold (edge-density) slider. Pure presentation; `CanvasView` reads/writes
//! `MemoryState` (R4). The Fold slider writes `mem.fold_threshold`; CanvasView's
//! Fold→LOD Effect reacts (no extra wiring here).

use leptos::prelude::*;

use crate::platform::phone::shell::PhoneShell;
use crate::state::memory::MemoryState;
use crate::views::canvas::CanvasView;

#[component]
#[must_use]
pub fn PhoneMemoryGraph() -> impl IntoView {
    let mem = expect_context::<MemoryState>();
    view! {
        <PhoneShell title="Memory" back="/memory" back_label="Memory">
        // Single element child for PhoneShell (footgun: no bare dynamic block
        // as a direct PhoneShell child). A definite-height flex child so the
        // WebGL canvas (`w-full h-full`) resolves its height.
        <div style="position:relative; flex:1; min-height:0; border-radius:12px; overflow:hidden;">
            <CanvasView/>
            // Floating Fold (edge-density) control — mirrors the desktop sidebar
            // slider (sidebar.rs). Writes mem.fold_threshold; CanvasView reacts.
            <div
                class="glass"
                style="position:absolute; left:10px; right:10px; bottom:10px; display:flex; align-items:center; gap:10px; padding:8px 12px; border-radius:12px;"
            >
                <span style="font-size:9.5px; text-transform:uppercase; letter-spacing:0.05em; color:var(--color-text-secondary);">"Fold"</span>
                <input
                    type="range" min="0" max="10" step="1" style="flex:1;" class="accent-[#a78bfa]"
                    prop:value=move || mem.fold_threshold.get() as i32
                    on:input=move |ev| {
                        if let Ok(v) = event_target_value(&ev).parse::<usize>() {
                            mem.fold_threshold.set(v);
                        }
                    }
                />
            </div>
        </div>
        </PhoneShell>
    }
}
```

- [ ] **Step 2: 在 `mod.rs` 声明模块**

`mod.rs` 顶部模块声明区当前是:

```rust
pub mod cell;
pub mod detail;
pub mod list;
```

改为(字母序插入 `graph`):

```rust
pub mod cell;
pub mod detail;
pub mod graph;
pub mod list;
```

- [ ] **Step 3: 构建门**

Run: `just wasm`
Expected: 编译绿(`PhoneMemoryGraph` 为 `pub`,虽暂未被分发引用也无 dead-code 警告)。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/platform/phone/memory/graph.rs interfaces/webchat/src/platform/phone/memory/mod.rs
git commit -m "panel: add phone Memory Graph screen (full-screen Canvas + Fold)"
```

---

## Task 2: 新建菜单着陆页 (`menu.rs`)

镜像桌面 `MemorySidebar`:顶部 Agent 选择器(内联可展开,移植 sidebar 的 popover 逻辑为内联列表)+ 两个下钻行(星系图 → `/memory/graph`,列表 → `/memory/list`)。搜索/Fold 不放菜单(归各自的内容屏)。Agent 列表由 `PhoneMemory`(`mod.rs`)既有的 agent-bootstrap Effect 填充 `mem.agents`。

**Files:**
- Create: `interfaces/webchat/src/platform/phone/memory/menu.rs`
- Modify: `interfaces/webchat/src/platform/phone/memory/mod.rs`(模块声明区,加 `pub mod menu;`)

**Interfaces:**
- Consumes: `PhoneShell`;`MemoryState.agent_id: RwSignal<String>`、`MemoryState.agents: RwSignal<Vec<AgentSummary>>`;`AgentSummary { id: String, name: Option<String>, emoji: Option<String>, .. }`;`use_navigate`。
- Produces: `pub fn PhoneMemoryMenu() -> impl IntoView`(Task 4 的分发引用)。

- [ ] **Step 1: 写 `menu.rs` 全文**

```rust
//! Phone Memory menu landing (`/memory`): mirrors the desktop `MemorySidebar`
//! as a full-screen list — an inline-expandable agent selector plus two drill
//! rows (Graph → /memory/graph, List → /memory/list). Search lives in the List
//! screen, Fold in the Graph screen; the menu stays a clean hub. Pure I/O (R4):
//! reads/writes `MemoryState` only; `mem.agents` is populated by the PhoneMemory
//! router's agent-bootstrap Effect.

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::platform::phone::shell::PhoneShell;
use crate::state::memory::MemoryState;

#[component]
#[must_use]
pub fn PhoneMemoryMenu() -> impl IntoView {
    let mem = expect_context::<MemoryState>();
    let navigate = use_navigate();
    let go = move |path: &'static str| {
        let navigate = navigate.clone();
        move |_| navigate(path, NavigateOptions::default())
    };
    // Inline agent-picker expansion (replaces the desktop popover).
    let agent_open = RwSignal::new(false);

    // Current agent label: "emoji name" | "name" | id.
    let current_label = move || {
        let id = mem.agent_id.get();
        mem.agents
            .get()
            .iter()
            .find(|a| a.id == id)
            .map(|a| {
                a.name
                    .as_deref()
                    .map(|n| match a.emoji.as_deref() {
                        Some(e) => format!("{e} {n}"),
                        None => n.to_string(),
                    })
                    .unwrap_or_else(|| a.id.clone())
            })
            .unwrap_or(id)
    };

    view! {
        <PhoneShell title="Memory">
        // ── Agent group ──
        <div>
            <div class="list-header">"Agent"</div>
            <div class="list">
                <div class="cell" on:click=move |_| agent_open.update(|v| *v = !*v)>
                    <span class="cell-leading">
                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="8" r="4"></circle><path d="M5 21a7 7 0 0 1 14 0"></path></svg>
                    </span>
                    <div class="cell-body"><div class="cell-title">{current_label}</div></div>
                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"></polyline></svg>
                </div>
                // Dynamic agent list lives INSIDE the .list div (a plain DOM
                // element), so mixing it with the static cell above is safe.
                {move || agent_open.get().then(|| {
                    let cur = mem.agent_id.get();
                    mem.agents.get().into_iter().map(|a| {
                        let id_for_click = a.id.clone();
                        let is_selected = a.id == cur;
                        let label = a.name.as_deref()
                            .map(|n| match a.emoji.as_deref() {
                                Some(e) => format!("{e} {n}"),
                                None => n.to_string(),
                            })
                            .unwrap_or_else(|| a.id.clone());
                        view! {
                            <div class="cell" on:click=move |_| { agent_open.set(false); mem.agent_id.set(id_for_click.clone()); }>
                                <div class="cell-body">
                                    <div class="cell-title" style=if is_selected { "color:var(--color-primary);" } else { "" }>{label}</div>
                                </div>
                                {is_selected.then(|| view! {
                                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" style="color:var(--color-primary);"><polyline points="20 6 9 17 4 12"></polyline></svg>
                                })}
                            </div>
                        }
                    }).collect_view()
                })}
            </div>
        </div>

        // ── Views group ──
        <div style="margin-top:20px;">
            <div class="list-header">"视图"</div>
            <div class="list">
                <div class="cell" on:click=go("/memory/graph")>
                    <span class="cell-leading">
                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="5" cy="6" r="2"></circle><circle cx="19" cy="6" r="2"></circle><circle cx="12" cy="18" r="2"></circle><line x1="6.6" y1="7.4" x2="10.6" y2="16.4"></line><line x1="17.4" y1="7.4" x2="13.4" y2="16.4"></line><line x1="7" y1="6" x2="17" y2="6"></line></svg>
                    </span>
                    <div class="cell-body"><div class="cell-title">"星系图"</div><div class="cell-sub">"关系网络可视化"</div></div>
                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                </div>
                <div class="cell" on:click=go("/memory/list")>
                    <span class="cell-leading">
                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><line x1="8" y1="6" x2="21" y2="6"></line><line x1="8" y1="12" x2="21" y2="12"></line><line x1="8" y1="18" x2="21" y2="18"></line><line x1="3" y1="6" x2="3.01" y2="6"></line><line x1="3" y1="12" x2="3.01" y2="12"></line><line x1="3" y1="18" x2="3.01" y2="18"></line></svg>
                    </span>
                    <div class="cell-body"><div class="cell-title">"列表"</div><div class="cell-sub">"Vault 笔记列表"</div></div>
                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                </div>
            </div>
        </div>
        </PhoneShell>
    }
}
```

> 说明:PhoneShell 的两个直接子节点都是 `<div>` 元素(Agent group / Views group),无裸 dynamic block。动态 agent 列表在 `.list` 这个普通 `<div>` 内部,符合 footgun 修复模式。

- [ ] **Step 2: 在 `mod.rs` 声明模块**

把模块声明区(Task 1 后为 `cell / detail / graph / list`)改为:

```rust
pub mod cell;
pub mod detail;
pub mod graph;
pub mod list;
pub mod menu;
```

- [ ] **Step 3: 构建门**

Run: `just wasm`
Expected: 编译绿(`PhoneMemoryMenu` 为 `pub`,暂未被引用无警告)。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/platform/phone/memory/menu.rs interfaces/webchat/src/platform/phone/memory/mod.rs
git commit -m "panel: add phone Memory menu landing (agent picker + Graph/List rows)"
```

---

## Task 3: 给 List 与 Detail 加返回

List 现在是 `/memory/list` 的内容屏,需返回菜单;Detail 从 List 进入,返回应回 List。

**Files:**
- Modify: `interfaces/webchat/src/platform/phone/memory/list.rs:53`
- Modify: `interfaces/webchat/src/platform/phone/memory/detail.rs:36,64,71`

**Interfaces:**
- Consumes: `PhoneShell { back?, back_label? }`(已有)。
- Produces: 无新接口(纯属性调整)。

- [ ] **Step 1: List 加返回**

`list.rs` 第 53 行当前:

```rust
        <PhoneShell title="Memory">
```

改为:

```rust
        <PhoneShell title="Memory" back="/memory" back_label="Memory">
```

(同文件第 136 行 `</PhoneShell>` 闭合不变。)

- [ ] **Step 2: Detail 返回/重定向改指 List**

`detail.rs` 共三处把 `/memory` / `"Memory"` 改为 `/memory/list` / `"List"`:

第 36 行(无选中笔记时的重定向),当前:

```rust
                navigate("/memory", NavigateOptions::default());
```

改为:

```rust
                navigate("/memory/list", NavigateOptions::default());
```

第 64 行(重定向过渡的空壳),当前:

```rust
            return view! { <PhoneShell title="Note" back="/memory" back_label="Memory"><div></div></PhoneShell> }
```

改为:

```rust
            return view! { <PhoneShell title="Note" back="/memory/list" back_label="List"><div></div></PhoneShell> }
```

第 71 行(正常详情壳),当前:

```rust
            <PhoneShell title="Note" back="/memory" back_label="Memory">
```

改为:

```rust
            <PhoneShell title="Note" back="/memory/list" back_label="List">
```

- [ ] **Step 3: 构建门**

Run: `just wasm`
Expected: 编译绿。

> 注:此刻 List 的返回指向 `/memory`,而 `/memory` 在 Task 4 之前仍渲染 List(分发未改),所以返回暂表现为重载 List——这是无害的中间态,Task 4 把 `/memory` 切成菜单后即正确。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/platform/phone/memory/list.rs interfaces/webchat/src/platform/phone/memory/detail.rs
git commit -m "panel: add back nav to phone Memory list/detail (list→menu, detail→list)"
```

---

## Task 4: 四路分发(激活整套结构)

提取纯函数 `screen_for_path`(mirror `parse_view_param` 的可测模式),单测它,再把 `PhoneMemory` 的 pathname 分发从两路重写成四路。这一步把 `/memory` 从 List 切成菜单,整套生效。

**Files:**
- Modify: `interfaces/webchat/src/platform/phone/memory/mod.rs`(加枚举 + 纯函数 + `#[cfg(test)]` + import + 重写分发闭包)

**Interfaces:**
- Consumes: `screen_for_path`、`MemoryScreen`(本 task 定义);`PhoneMemoryMenu`(Task 2)、`PhoneMemoryGraph`(Task 1)、`PhoneMemoryList`、`PhoneMemoryDetail`(既有)。
- Produces: `pub(crate) fn screen_for_path(path: &str) -> MemoryScreen` + `pub enum MemoryScreen`。

- [ ] **Step 1: 写失败测试 + 纯函数 + 枚举**

在 `mod.rs` 末尾(`PhoneMemory` 组件之后)追加:

```rust
// ---------------------------------------------------------------------------
// Pure path → screen mapping. Extracted so the routing table is unit-tested
// without the Leptos runtime (mirrors `state::memory::parse_view_param`).
// ---------------------------------------------------------------------------

/// Which phone Memory screen a URL path maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryScreen {
    Menu,
    List,
    Graph,
    Note,
}

#[must_use]
pub(crate) fn screen_for_path(path: &str) -> MemoryScreen {
    match path {
        "/memory/note" => MemoryScreen::Note,
        "/memory/graph" => MemoryScreen::Graph,
        "/memory/list" => MemoryScreen::List,
        _ => MemoryScreen::Menu,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_for_path_maps_each_route() {
        assert_eq!(screen_for_path("/memory"), MemoryScreen::Menu);
        assert_eq!(screen_for_path("/memory/list"), MemoryScreen::List);
        assert_eq!(screen_for_path("/memory/graph"), MemoryScreen::Graph);
        assert_eq!(screen_for_path("/memory/note"), MemoryScreen::Note);
    }

    #[test]
    fn screen_for_path_unknown_falls_back_to_menu() {
        assert_eq!(screen_for_path("/memory/bogus"), MemoryScreen::Menu);
        assert_eq!(screen_for_path("/"), MemoryScreen::Menu);
    }
}
```

- [ ] **Step 2: 验证测试存在并可编译(失败态)**

此刻 `screen_for_path` 已写但分发还没用它,且 `PhoneMemoryMenu`/`PhoneMemoryGraph` 还没 import。按项目 cargo 节制纪律,权威门是 `just wasm`(编译期会校验测试模块)。如需主机跑测试(可选):

Run(可选): `cargo test -p aleph-panel --lib platform::phone::memory::tests`
Expected: 两个测试 PASS(纯 `&str` 匹配,不依赖运行时)。
> 若该 crate 主机不可测(WASM-only),跳过本步——`just wasm` 已编译校验测试体。

- [ ] **Step 3: 重写分发闭包 + import**

`mod.rs` 的 `use self::...` 区当前:

```rust
use self::detail::PhoneMemoryDetail;
use self::list::PhoneMemoryList;
```

改为(加 graph/menu,字母序):

```rust
use self::detail::PhoneMemoryDetail;
use self::graph::PhoneMemoryGraph;
use self::list::PhoneMemoryList;
use self::menu::PhoneMemoryMenu;
```

`PhoneMemory` 组件结尾的分发闭包当前:

```rust
    let location = use_location();
    move || {
        if location.pathname.get() == "/memory/note" {
            view! { <PhoneMemoryDetail/> }.into_any()
        } else {
            view! { <PhoneMemoryList/> }.into_any()
        }
    }
```

改为:

```rust
    let location = use_location();
    move || match screen_for_path(&location.pathname.get()) {
        MemoryScreen::Note => view! { <PhoneMemoryDetail/> }.into_any(),
        MemoryScreen::Graph => view! { <PhoneMemoryGraph/> }.into_any(),
        MemoryScreen::List => view! { <PhoneMemoryList/> }.into_any(),
        MemoryScreen::Menu => view! { <PhoneMemoryMenu/> }.into_any(),
    }
```

- [ ] **Step 4: 构建门**

Run: `just wasm`
Expected: 编译绿。现在 `/memory`=菜单、`/memory/graph`=星系图、`/memory/list`=Vault、`/memory/note`=详情。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/platform/phone/memory/mod.rs
git commit -m "panel: wire phone Memory four-screen dispatch (menu/graph/list/note)"
```

---

## 验收清单(全部 task 后,iOS-sim QA — 运行时权威门)

按 `feedback-ios-panel-test-via-full-macos-app`:重编完整版 macOS app(`just shell-dev` 或 `just shell-build`,其内置 core 在 :18790 重嵌当前 dist)→ iOS sim 连同一本地 core。逐项确认:

- [ ] 底部 Memory tab → 落在**菜单着陆页**(Agent + 星系图/列表两行),**非分屏、非直接列表**。
- [ ] 菜单点 Agent 行 → 内联展开 agent 列表,选一个 → 收起且当前名更新。
- [ ] 菜单点"列表" → 全屏 Vault(搜索/facet/计数/Load more 正常),`‹ Memory` 返回回菜单。
- [ ] Vault 点一条笔记 → 详情(全文 + backlinks),`‹ List` 返回回列表。
- [ ] 菜单点"星系图" → 全屏星系图渲染,Fold 滑杆可调(边密度变化),`‹ Memory` 返回回菜单。
- [ ] 全程**任何屏都无左右分屏**。
- [ ] (已知 follow-up,不阻塞)Canvas 触屏 pan/zoom/tap-select 若不响应 → 记为后续触屏手势适配。

---

## Self-Review(已对 spec 核对)

- **Spec §3 四屏结构** → Task 1(graph)/Task 2(menu)/Task 3(list+detail back)/Task 4(分发)全覆盖。
- **Spec §4 路由机制(无需注册)** → Task 4 仅改 `PhoneMemory` 内部分发,未碰 `app.rs`/router,符合。
- **Spec §5 数据层复用(R4)** → 无新建 core/IPC;menu/graph 只读写 `MemoryState`;list/detail/`PhoneMemoryState` 不变。
- **Spec §6 改动清单** → 与本计划 File Structure 逐项对应(menu/graph 新建、list/detail back、mod.rs 分发、不动 app.rs/shell.rs)。
- **Spec §7 Canvas 范围/风险** → Task 1 全屏挂载 + Fold;触屏手势列为验收清单的 follow-up,不阻塞。
- **类型一致性** → `screen_for_path`/`MemoryScreen` 在 Task 4 定义并即用;`PhoneMemoryMenu`/`PhoneMemoryGraph` 名称在 Task 1/2 定义、Task 4 引用,一致;`mem.fold_threshold`(usize)、`mem.agents`(Vec<AgentSummary>)、`mem.agent_id`(String)与 `state/memory.rs` 一致。
- **占位符扫描** → 无 TBD/TODO;每个改动步骤含完整代码。
