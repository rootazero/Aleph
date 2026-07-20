# Aleph Hub 左栏导航化 + 隐藏 Hub 内切换器 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 Aleph Hub 的「精选 + 分类」从右侧横向 chips 迁移成左栏垂直导航（新增「全部」全局入口），并在 Hub 模式下隐藏左栏底部的 section 切换器。

**Architecture:** 纯前端（Leptos/WASM）布局调整。把 `StoreState` 的 `provide_context` 上提到两列共同父级 `AppContent`（仿 `ChatState`），使左栏 `ExtensionsSidebar` 与主区 `BrowsePane` 共享同一 `category` 信号；左栏渲染新 `CategoryNav` 组件，主区移除 `CategoryChips`；`ModeSidebar` 在 Extensions 模式下不渲染 `NavMenu`。

**Tech Stack:** Rust + Leptos 0.7 (`leptos::prelude`), Tailwind 设计 token（`nav-tile` / `nav-tile-active` / `border-border` 等），i18n via `leptos_i18n`（`t_string!` / `use_i18n`）。

## Global Constraints

- 改动范围严格限定 `interfaces/webchat/src/`，不动 core / `model.rs` / i18n 文件（`"all"` 过滤逻辑与 `extensions.cat.all` 标签已存在）。
- **极度节制 cargo**：不做 per-task 编译；所有编译验证集中到最后 Task 5 的**一次** `cargo check -p aleph-panel --lib --target wasm32-unknown-unknown`。Task 1–4 的 gate 是代码正确性自查。
- 单分支开发：直接在 `main` 工作（项目 CLAUDE.md「所有开发工作直接在 main 分支进行」）。
- 提交策略：仅在用户明确要求时提交（用户 git 习惯）；本计划把提交集中在 Task 5 末尾，由用户决定是否执行。
- 代码注释用英文；保持与既有组件相同的 Leptos 写法与 Tailwind token。
- 不重置点击分类时的 kind/trust/query 筛选（与现有 chip 行为字节一致）。

---

### Task 1: 上提 `StoreState` 到 `AppContent`，`ExtensionsView` 改为消费

把 `StoreState` 的创建与 provide 从 `ExtensionsView`（主区内）上移到 `AppContent`（两列共同父级），让左栏也能 `expect_context` 到同一份 store。

**Files:**
- Modify: `interfaces/webchat/src/views/extensions/mod.rs:44`（`fn new` 可见性）
- Modify: `interfaces/webchat/src/views/extensions/mod.rs:86-87`（`ExtensionsView` 改 `expect_context`）
- Modify: `interfaces/webchat/src/app.rs:12`（import 增加 `StoreState`）
- Modify: `interfaces/webchat/src/app.rs:97` 之后（`AppContent` 内 `provide_context`）

**Interfaces:**
- Produces: `StoreState` 通过 `AppContent` 的 `provide_context` 暴露给两列；后续 Task 2 的 `CategoryNav` 与既有 `BrowsePane` 都靠 `expect_context::<StoreState>()` 拿到它。`StoreState::new()` 变为 `pub(crate)`。

- [ ] **Step 1: 放开 `StoreState::new` 可见性**

`interfaces/webchat/src/views/extensions/mod.rs:44`，把：

```rust
impl StoreState {
    fn new() -> Self {
```

改为：

```rust
impl StoreState {
    pub(crate) fn new() -> Self {
```

- [ ] **Step 2: `ExtensionsView` 改为消费上提的 store**

`interfaces/webchat/src/views/extensions/mod.rs`，把（约 86-87 行）：

```rust
    let i18n = use_i18n();
    let store = StoreState::new();
    provide_context(store);
    let navigate = use_navigate();
```

改为：

```rust
    let i18n = use_i18n();
    // Store is now provided by AppContent (parent of both columns) so the
    // left-column CategoryNav and this main-area view share one selection.
    let store = expect_context::<StoreState>();
    let navigate = use_navigate();
```

- [ ] **Step 3: `app.rs` import 增加 `StoreState`**

`interfaces/webchat/src/app.rs:12`，把：

```rust
use crate::views::extensions::ExtensionsView;
```

改为：

```rust
use crate::views::extensions::{ExtensionsView, StoreState};
```

- [ ] **Step 4: 在 `AppContent` 内 provide `StoreState`**

`interfaces/webchat/src/app.rs`，在 `provide_context(NotificationsState::new());`（约 97 行）之后新增：

```rust
    // Extensions (Aleph Hub) store — lifted above both columns so the
    // left-column category nav (ExtensionsSidebar) and the main-area grid
    // (BrowsePane) share one `category` selection. Mirrors ChatState's
    // split-column sharing (see above).
    provide_context(StoreState::new());
```

- [ ] **Step 5: 正确性自查（不编译）**

确认：①`ExtensionsView` 不再 `StoreState::new()` / `provide_context`，避免重复 provide；②`app.rs` 仅新增一处 provide；③`expect_context::<StoreState>()` 的祖先（`AppContent`）已 provide。无需 cargo。

---

### Task 2: 新建 `CategoryNav` 并在 `ExtensionsSidebar` 渲染

左栏空 `<div>` 填充为垂直分类导航：精选 / 全部 两个全局入口置顶，分割线后接 13 个 `CATEGORIES` 分类。

**Files:**
- Create: `interfaces/webchat/src/components/extensions/category_nav.rs`
- Modify: `interfaces/webchat/src/components/extensions/mod.rs`（注册 `pub mod category_nav;`）
- Modify: `interfaces/webchat/src/views/extensions/mod.rs:142-148`（`ExtensionsSidebar` 渲染 `CategoryNav`）

**Interfaces:**
- Consumes: Task 1 的 `expect_context::<StoreState>()`（读写 `store.category`）；既有 `crate::views::extensions::model::CATEGORIES` 与 `crate::components::extensions::labels::category_label`。
- Produces: `crate::components::extensions::category_nav::CategoryNav` 组件（无参 `#[component]`）。

- [ ] **Step 1: 创建 `category_nav.rs`**

新建 `interfaces/webchat/src/components/extensions/category_nav.rs`，内容：

```rust
use leptos::prelude::*;

use crate::components::extensions::labels::category_label;
use crate::i18n::use_i18n;
use crate::views::extensions::model::CATEGORIES;
use crate::views::extensions::StoreState;

/// Vertical category navigation for the Aleph Hub left column.
///
/// Two "global" entries (Featured / All) are pinned on top, then the 13
/// functional-category facets below a divider. Each entry drives
/// `store.category` — identical behavior to the old horizontal CategoryChips,
/// just relocated to the left column to declutter the main area.
#[component]
#[must_use]
pub fn CategoryNav() -> impl IntoView {
    let store = expect_context::<StoreState>();
    let i18n = use_i18n();

    let item = move |value: &'static str, label: String, emoji: &'static str| {
        let active = move || store.category.get() == value;
        view! {
            <button
                class=move || {
                    let base = "w-full flex items-center gap-2.5 px-3 py-2 rounded-lg text-sm";
                    if active() {
                        format!("{base} nav-tile-active")
                    } else {
                        format!("{base} nav-tile")
                    }
                }
                on:click=move |_| store.category.set(value.to_string())
            >
                <span class="flex-shrink-0 w-5 text-center">{emoji}</span>
                <span class="flex-1 text-left truncate">{label}</span>
            </button>
        }
    };

    view! {
        <nav class="flex flex-col h-full overflow-y-auto px-2 py-3 gap-0.5">
            {item("featured", category_label(i18n, "featured"), "★")}
            {item("all", category_label(i18n, "all"), "🗂")}
            <div class="my-2 border-t border-border"></div>
            {CATEGORIES
                .iter()
                .map(|c| item(c.value, category_label(i18n, c.value), c.emoji))
                .collect_view()}
        </nav>
    }
}
```

- [ ] **Step 2: 注册模块**

`interfaces/webchat/src/components/extensions/mod.rs`，在 `pub mod card;` 后按字母序新增一行：

```rust
pub mod card;
pub mod category_nav;
pub mod chips;
```

- [ ] **Step 3: `ExtensionsSidebar` 渲染 `CategoryNav`**

`interfaces/webchat/src/views/extensions/mod.rs`，把：

```rust
#[component]
#[must_use]
pub fn ExtensionsSidebar() -> impl IntoView {
    // Minimal secondary column; the store's own topbar (chips/search/installed) lives in the
    // main area per the mockup. Category quick-nav is added with browse in Task 5.
    view! { <div class="flex flex-col h-full"></div> }
}
```

改为：

```rust
#[component]
#[must_use]
pub fn ExtensionsSidebar() -> impl IntoView {
    // Left-column category navigation (Featured / All / 13 facets); shares the
    // app-level StoreState so selections drive the main-area BrowsePane grid.
    view! { <crate::components::extensions::category_nav::CategoryNav /> }
}
```

- [ ] **Step 4: 正确性自查（不编译）**

确认：①`category_nav.rs` 的四个 import 路径均存在（`labels` 为 `pub mod`、`StoreState` / `CATEGORIES` 已 `pub`）；②`item` 闭包返回类型在三处调用（featured/all + map）一致，与既有 `CategoryChips::chip` 写法同构；③`mod.rs` 模块名 `category_nav` 与文件名一致。

---

### Task 3: 主区移除 `CategoryChips`，删除孤儿组件

`BrowsePane` 不再渲染横向分类 chips（已迁到左栏）；`CategoryChips` 失去唯一消费者，从 `chips.rs` 删除。

**Files:**
- Modify: `interfaces/webchat/src/views/extensions/browse.rs:7-9`（import）、`browse.rs:71-77`（chrome 块）
- Modify: `interfaces/webchat/src/components/extensions/chips.rs:1-36`（删除 `CategoryChips` 及孤儿 import）
- Modify: `interfaces/webchat/src/components/extensions/labels.rs:28`（注释把 `CategoryChips` 更新为 `CategoryNav`）

**Interfaces:**
- Consumes: 无新增。
- Produces: `chips.rs` 仅余 `FilterSegs` / `StoreSearch` / `pub use category_label`；`browse.rs` 主区 chrome 变为「搜索框 + 筛选段」。

- [ ] **Step 1: `browse.rs` 移除 `CategoryChips` import**

`interfaces/webchat/src/views/extensions/browse.rs`，把（7-9 行）：

```rust
use crate::components::extensions::chips::{
    category_label, CategoryChips, FilterSegs, StoreSearch,
};
```

改为：

```rust
use crate::components::extensions::chips::{category_label, FilterSegs, StoreSearch};
```

- [ ] **Step 2: `browse.rs` 移除 `<CategoryChips />` 渲染**

`interfaces/webchat/src/views/extensions/browse.rs`，把（约 72-77 行）：

```rust
        // Chrome: search + chips + filter segments
        <div class="flex flex-col gap-3 mb-4">
            <StoreSearch />
            <CategoryChips />
            <FilterSegs />
        </div>
```

改为：

```rust
        // Chrome: search + filter segments (category nav lives in the left column now)
        <div class="flex flex-col gap-3 mb-4">
            <StoreSearch />
            <FilterSegs />
        </div>
```

- [ ] **Step 3: `chips.rs` 删除 `CategoryChips` 组件与孤儿 import**

`interfaces/webchat/src/components/extensions/chips.rs`，删除文件顶部的 `CATEGORIES` import 与整个 `CategoryChips` 组件（原 1-36 行），使文件开头变为：

```rust
use leptos::prelude::*;

use crate::i18n::{t_string, use_i18n};
use crate::views::extensions::StoreState;

pub use crate::components::extensions::labels::category_label;

#[component]
#[must_use]
pub fn FilterSegs() -> impl IntoView {
```

（即：移除 `use crate::views::extensions::model::CATEGORIES;`，移除 `pub fn CategoryChips() -> impl IntoView { ... }` 整段；`FilterSegs` 及其后内容保持不变。）

- [ ] **Step 4: `labels.rs` 更新陈旧注释**

`interfaces/webchat/src/components/extensions/labels.rs:28`，把：

```rust
/// Used for BOTH chip labels (CategoryChips) and shelf titles (browse.rs).
```

改为：

```rust
/// Used for BOTH the left-column CategoryNav labels and shelf titles (browse.rs).
```

- [ ] **Step 5: 正确性自查（不编译）**

确认：①`CategoryChips` 全仓再无引用（Task 2 的 `CategoryNav` 是独立组件，不引用 `CategoryChips`）；②`chips.rs` 删除 `CATEGORIES` import 后，`FilterSegs` / `StoreSearch` 不依赖它（它们只用 `store` / `t_string!` / `use_i18n`）；③`category_label` 再导出保留（`browse.rs` 货架标题仍用）。

---

### Task 4: Extensions 模式下隐藏底部 `NavMenu`

Hub 是全屏专注态，靠主区头部「Back to Chat」退出；左栏底部的 section 切换器在 Extensions 模式下不渲染，回到 chat 后重现。

**Files:**
- Modify: `interfaces/webchat/src/components/mode_sidebar.rs:79-80`

**Interfaces:**
- Consumes: 既有 `mode` Memo（`mode_sidebar.rs:59`）与 `PanelMode`（同文件已 import）。
- Produces: 无新接口。

- [ ] **Step 1: 条件渲染 `NavMenu`**

`interfaces/webchat/src/components/mode_sidebar.rs`，把（79-80 行）：

```rust
            // Persistent bottom-left section switcher
            <NavMenu />
```

改为：

```rust
            // Bottom-left section switcher — hidden inside the Aleph Hub
            // (Extensions) full-screen takeover, which exits via its own
            // header "Back to Chat"; the switcher reappears in every other mode.
            {move || (mode.get() != PanelMode::Extensions).then(|| view! { <NavMenu /> })}
```

- [ ] **Step 2: 正确性自查（不编译）**

确认：①`mode` Memo 在该 `view!` 作用域内可用；②`.then(|| view!{...})` 返回 `Option<_>`，Leptos `IntoView` 接受；③其它模式（Chat/Dashboard/…）仍渲染 `NavMenu`。

---

### Task 5: 一次性编译验证 +（用户确认后）提交

集中编译验证，遵守 cargo 节制。

**Files:** 无（仅验证 / 提交）

- [ ] **Step 1: 单次 WASM 编译检查**

Run:
```bash
cargo check -p aleph-panel --lib --target wasm32-unknown-unknown
```
Expected: exit 0，无 error（允许既有无关 warning）。若报错，按错误定位回对应 Task 修复后再次 check（仍尽量合并为一次）。

- [ ] **Step 2: 提交（仅在用户要求时执行）**

```bash
git add interfaces/webchat/src/app.rs \
        interfaces/webchat/src/components/extensions/category_nav.rs \
        interfaces/webchat/src/components/extensions/mod.rs \
        interfaces/webchat/src/components/extensions/chips.rs \
        interfaces/webchat/src/components/extensions/labels.rs \
        interfaces/webchat/src/components/mode_sidebar.rs \
        interfaces/webchat/src/views/extensions/mod.rs \
        interfaces/webchat/src/views/extensions/browse.rs
git commit -m "panel: move Aleph Hub featured/categories into left sidebar nav"
```

- [ ] **Step 3: 部署提示（可选，眼见为实时）**

按 CLAUDE.md「Panel ↔ Daemon 资源嵌入链」：`just wasm` → 重编 `aleph-server` binary → 替换运行中 binary。本计划不自动执行部署。

---

## Self-Review

**1. Spec coverage（逐条对照 spec）:**
- spec §3 状态共享（方案 A 上提 StoreState）→ Task 1 ✓
- spec §4.1 左栏垂直导航 + 全部入口 → Task 2 ✓
- spec §4.2 主区去拥挤 + 删孤儿 CategoryChips → Task 3 ✓
- spec §4.3 隐藏 Hub 内 NavMenu → Task 4 ✓
- spec §5 改动清单 8 文件 → Task 1–4 全覆盖（app.rs / mod.rs ×2 / category_nav.rs / browse.rs / chips.rs / mode_sidebar.rs；额外含 labels.rs 注释订正）✓
- spec §7 验证（单次 wasm check）→ Task 5 ✓

**2. Placeholder scan:** 无 TBD/TODO/"类似上文"；每个 code step 给出完整代码与精确路径。✓

**3. Type consistency:** `CategoryNav`（组件名）、`StoreState::new()`（`pub(crate)`）、`store.category`（`RwSignal<String>`）、`category_label(i18n, value)` 在各 Task 间一致；`item` 闭包签名 `(value: &'static str, label: String, emoji: &'static str)` 与 `CATEGORIES`/`category_label` 返回类型匹配。✓
