# Aleph Panel Phase 2 · iPhone 移动端三修复 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复 390px iPhone QA 暴露的三个移动端缺口（settings 双栏溢出 / 顶栏内容遮挡 / chat 顶栏去重），全部 `max-sm` 门控，iPad/桌面不变。

**Architecture:** 纯 Leptos 0.7/WASM + Tailwind CSS 改动，crate `aleph-panel`（`interfaces/webchat/`）。#2 = 1 行 CSS 变量；#3 = chat 顶栏空 title；#1 = 7 个 master-detail 页套统一「钻入折叠」配方（`max-sm:` 可见性切换 + 图标返回按钮 + `!is_mobile` 列表优先）。

**Tech Stack:** Leptos 0.7（`view!` 宏、`class=("name", closure)` 反应式类、`RwSignal`/`signal`）、Tailwind v4（`max-sm:` = <640px）、`ViewportState.is_mobile: RwSignal<bool>`。

## Global Constraints

> 每个 task 的需求都隐含包含本节。

- **断点**：所有改动只在 `max-sm:`（<640px，iPhone 竖屏）生效；**iPad/桌面 ≥640px 字节级不变**（红线）。`max-sm:hidden` 在 ≥640 不生效，故桌面双栏不受反应式隐藏影响。
- **零 core `src/` 改动**；仅改 `interfaces/webchat`。**零新依赖**。**零新 i18n key**（返回按钮用图标 + 硬编码 `aria-label`，沿用 `chat/view.rs:252` 的 `aria-label="Switch agent"` 先例）。
- **ViewportState**：`use crate::state::viewport::ViewportState;`，组件顶部 `let is_mobile = expect_context::<ViewportState>().is_mobile;`（`RwSignal<bool>`，Copy），异步内用 `is_mobile.get_untracked()`。
- **Leptos 反应式类语法**：保留原静态 `class="…"` 不动（仅向其追加 `max-sm:` 类），用**额外的** `class=("max-sm:hidden", move || <bool>)` 元组做条件隐藏。两个 `class` 属性可共存于同一元素。
- **cargo 节制（用户硬约束）**：**不在每个 task 跑 cargo/wasm 构建**。编译与视觉验证**批到最后 Task 10 一次**完成（`cargo check -p aleph-panel --lib` + `just wasm` + 重编 server + 390px 复看）。每个 task 只做编辑 + commit；reviewer 审 diff（类串/闭包/信号名正确性），不构建。
- **提交**：每 task 一次 source-only commit，scope `panel:`，无 attribution。

## 钻入折叠配方（Drill-in Recipe — 所有 #1 task 套用）

给定一个「左列表 + 右详情」双栏视图，记 `DETAIL_ACTIVE` 为「详情/新增表单已激活」的 bool 表达式（每页用其既有信号，见各 task）：

1. **左列表 wrapper**：向其静态 class 追加 `max-sm:w-full max-sm:min-w-0`；再加一个 `class=("max-sm:hidden", move || DETAIL_ACTIVE)`（详情激活时移动端隐藏列表）。
2. **右详情 wrapper**：向其静态 class 追加 `max-sm:w-full max-sm:min-w-0`；再加 `class=("max-sm:hidden", move || !(DETAIL_ACTIVE))`（无详情时移动端隐藏右栏 → 只剩全宽列表；桌面仍显示 EmptyState）。
3. **移动返回按钮**：作为右详情 wrapper 的**第一个子元素**插入（在既有 `{move || …}` 详情内容之前）：

```rust
<button
    type="button"
    class="hidden max-sm:flex items-center gap-1 px-4 py-3 text-sm text-text-tertiary active:bg-surface-sunken"
    aria-label="Back to list"
    on:click=move |_| { /* 清空选择 + 关 add-form，见各 task */ }
>
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor"
         stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <polyline points="15 18 9 12 15 6" />
    </svg>
</button>
```

4. **列表优先**：把该页 mount 时的 auto-select-active 块用 `!is_mobile.get_untracked()` 门控，移动端落在列表（不直接钻进详情）。组件顶部加 `let is_mobile = expect_context::<ViewportState>().is_mobile;`（若该页尚未引入则同时加 `use crate::state::viewport::ViewportState;`）。

> **为何用 `class=(...)` 元组而非 `class:max-sm:hidden=`**：Tailwind 变体名含冒号，`class:` 指令解析会歧义；元组字符串形式接受任意类名。

---

### Task 1: Fix #2 — 顶栏内容偏移（根因，先做）

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css:1956-1965`

**Interfaces:** 无（纯 CSS 变量）。

- [ ] **Step 1: 改移动档 `--aleph-content-top` 值 + 注释**

`tailwind.css` 当前 L1960-1965：

```css
    /* Section pages (`.aleph-content-top`) clear the status-bar safe area on
       phones so a page title never hides under the iOS notch. env() resolves
       to 0 in the browser, so the web preview is unchanged. */
    :root {
        --aleph-content-top: calc(var(--safe-area-top) + 0.85rem);
    }
```

改为：

```css
    /* Section pages (`.aleph-content-top`) must clear BOTH the status-bar safe
       area AND the 43px `MobileTopBar` overlay (`absolute top-0 z-20`, app.rs)
       that sits below the notch — otherwise the first content row hides under
       the bar. 3rem(48px) clears the measured 43px bar + ~5px gap. env()
       resolves to 0 in the browser, so the web preview tracks the device. */
    :root {
        --aleph-content-top: calc(var(--safe-area-top) + 3rem);
    }
```

只改这一处；**不碰** `:root` 桌面档 `0.85rem`（L1932）与 `html[data-platform="macos"]` `2.45rem`（L1945）。

- [ ] **Step 2: Commit**

```bash
git add interfaces/webchat/styles/tailwind.css
git commit -m "panel: clear mobile MobileTopBar in --aleph-content-top (iPhone)"
```

> 验证（批到 Task 10）：<640px 下 `getComputedStyle(document.documentElement).getPropertyValue('--aleph-content-top')` 解析为 48px；settings landing「基础」、agents H1 不再被 43px 顶栏遮挡。

---

### Task 2: Fix #3 — chat 顶栏去重

**Files:**
- Modify: `interfaces/webchat/src/views/chat/view.rs:242-243`

**Interfaces:**
- Consumes: `MobileTopBar(title: Signal<String>, …)`（§11 钉死，**不改组件**）。

- [ ] **Step 1: chat 顶栏中间 title 传空串**

`chat/view.rs:243` 当前：

```rust
                            title=Signal::derive(move || mobile_agent.get().1)
```

改为（左 pill 已显示 avatar+agent名+chevron 作切换器，中标题去重）：

```rust
                            title=Signal::derive(|| String::new())
```

左 pill（L244-269）与 §11 接口不动。空串 → 中间槽不渲染文字。

- [ ] **Step 2: Commit**

```bash
git add interfaces/webchat/src/views/chat/view.rs
git commit -m "panel: drop duplicate agent-name title from chat MobileTopBar"
```

---

### Task 3: Fix #1 — embedding_providers 钻入折叠（参考实现）

**Files:**
- Modify: `interfaces/webchat/src/views/settings/embedding_providers/mod.rs`

**Interfaces:**
- 信号：`selected_provider_id`/`set_selected_provider_id`（`signal`, L37）、`show_add_form`/`set_show_add_form`（L38）。
- `DETAIL_ACTIVE` = `selected_provider_id.get().is_some() || show_add_form.get()`。

- [ ] **Step 1: 加 ViewportState 导入 + 顶部捕获 is_mobile**

文件顶部 `use` 区（L13-21 附近）加：

```rust
use crate::state::viewport::ViewportState;
```

组件体内信号声明附近（L38 之后）加：

```rust
    let is_mobile = expect_context::<ViewportState>().is_mobile;
```

- [ ] **Step 2: 门控 auto-select（列表优先）**

L51-55 当前：

```rust
                        if selected_provider_id.get_untracked().is_none() {
                            if let Some(active) = list.iter().find(|p| p.is_active) {
                                set_selected_provider_id.set(Some(active.id.clone()));
                            }
                        }
```

改为：

```rust
                        if !is_mobile.get_untracked()
                            && selected_provider_id.get_untracked().is_none()
                        {
                            if let Some(active) = list.iter().find(|p| p.is_active) {
                                set_selected_provider_id.set(Some(active.id.clone()));
                            }
                        }
```

- [ ] **Step 3: 左列表 wrapper（L83）套配方步骤 1**

```rust
            <div class="flex flex-col w-5/12 min-w-[400px] border-r border-border">
```

→

```rust
            <div class="flex flex-col w-5/12 min-w-[400px] max-sm:w-full max-sm:min-w-0 border-r border-border"
                 class=("max-sm:hidden", move || selected_provider_id.get().is_some() || show_add_form.get())>
```

- [ ] **Step 4: 右详情 wrapper（L277）套配方步骤 2+3**

```rust
            <div class="w-7/12 min-w-[320px] bg-surface">
                {move || {
```

→

```rust
            <div class="w-7/12 min-w-[320px] max-sm:w-full max-sm:min-w-0 bg-surface"
                 class=("max-sm:hidden", move || !(selected_provider_id.get().is_some() || show_add_form.get()))>
                <button
                    type="button"
                    class="hidden max-sm:flex items-center gap-1 px-4 py-3 text-sm text-text-tertiary active:bg-surface-sunken"
                    aria-label="Back to list"
                    on:click=move |_| { set_selected_provider_id.set(None); set_show_add_form.set(false); }
                >
                    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <polyline points="15 18 9 12 15 6" />
                    </svg>
                </button>
                {move || {
```

（闭合的 `</div>` 不变。）

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/settings/embedding_providers/mod.rs
git commit -m "panel: drill-in collapse embedding providers on iPhone"
```

---

### Task 4: Fix #1 — generation_providers 钻入折叠

**Files:**
- Modify: `interfaces/webchat/src/views/settings/generation_providers/mod.rs`

**Interfaces:**
- 信号：`selected_provider_id`/`set_selected_provider_id`（L70）、`show_add_form`/`set_show_add_form`（L71）。
- `DETAIL_ACTIVE` = `selected_provider_id.get().is_some() || show_add_form.get()`。

- [ ] **Step 1: ViewportState 导入 + 顶部捕获**

顶部 `use` 区加 `use crate::state::viewport::ViewportState;`；信号声明附近（L71 后）加 `let is_mobile = expect_context::<ViewportState>().is_mobile;`。

- [ ] **Step 2: 门控 auto-select**

L104 当前 `if selected_provider_id.get_untracked().is_none() {`（其块内 L131 `set_selected_provider_id.set(Some(sel));`）改为：

```rust
            if !is_mobile.get_untracked() && selected_provider_id.get_untracked().is_none() {
```

- [ ] **Step 3: 左 wrapper（L161）**

```rust
            <div class="flex flex-col w-5/12 min-w-[400px] border-r border-border">
```

→

```rust
            <div class="flex flex-col w-5/12 min-w-[400px] max-sm:w-full max-sm:min-w-0 border-r border-border"
                 class=("max-sm:hidden", move || selected_provider_id.get().is_some() || show_add_form.get())>
```

- [ ] **Step 4: 右 wrapper（L339）+ 返回按钮**

```rust
            <div class="w-7/12 min-w-[320px] bg-surface">
                {move || {
```

→

```rust
            <div class="w-7/12 min-w-[320px] max-sm:w-full max-sm:min-w-0 bg-surface"
                 class=("max-sm:hidden", move || !(selected_provider_id.get().is_some() || show_add_form.get()))>
                <button
                    type="button"
                    class="hidden max-sm:flex items-center gap-1 px-4 py-3 text-sm text-text-tertiary active:bg-surface-sunken"
                    aria-label="Back to list"
                    on:click=move |_| { set_selected_provider_id.set(None); set_show_add_form.set(false); }
                >
                    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <polyline points="15 18 9 12 15 6" />
                    </svg>
                </button>
                {move || {
```

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/settings/generation_providers/mod.rs
git commit -m "panel: drill-in collapse generation providers on iPhone"
```

---

### Task 5: Fix #1 — reranking_providers 钻入折叠

**Files:**
- Modify: `interfaces/webchat/src/views/settings/reranking_providers/mod.rs`

**Interfaces:**
- 信号：`selected_provider`（`RwSignal`, L85）、`show_add_form`/`set_show_add_form`（L86）。
- `DETAIL_ACTIVE` = `selected_provider.get().is_some() || show_add_form.get()`。
- 右栏 wrapper 是 `flex-1`（非 `w-7/12`）。

- [ ] **Step 1: ViewportState 导入 + 顶部捕获**

顶部加 `use crate::state::viewport::ViewportState;`；L86 后加 `let is_mobile = expect_context::<ViewportState>().is_mobile;`。

- [ ] **Step 2: 门控 auto-select（L97-99）**

```rust
                        if selected_provider.get_untracked().is_none() {
                            selected_provider.set(Some(current));
                        }
```

→

```rust
                        if !is_mobile.get_untracked() && selected_provider.get_untracked().is_none() {
                            selected_provider.set(Some(current));
                        }
```

- [ ] **Step 3: 左 wrapper（L114）**

```rust
            <div class="flex flex-col w-5/12 min-w-[400px] border-r border-border">
```

→

```rust
            <div class="flex flex-col w-5/12 min-w-[400px] max-sm:w-full max-sm:min-w-0 border-r border-border"
                 class=("max-sm:hidden", move || selected_provider.get().is_some() || show_add_form.get())>
```

- [ ] **Step 4: 右 wrapper（L205）+ 返回按钮**

```rust
            <div class="flex-1 flex flex-col overflow-hidden">
                {move || {
```

→

```rust
            <div class="flex-1 flex flex-col overflow-hidden max-sm:w-full max-sm:min-w-0"
                 class=("max-sm:hidden", move || !(selected_provider.get().is_some() || show_add_form.get()))>
                <button
                    type="button"
                    class="hidden max-sm:flex items-center gap-1 px-4 py-3 text-sm text-text-tertiary active:bg-surface-sunken"
                    aria-label="Back to list"
                    on:click=move |_| { selected_provider.set(None); set_show_add_form.set(false); }
                >
                    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <polyline points="15 18 9 12 15 6" />
                    </svg>
                </button>
                {move || {
```

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/settings/reranking_providers/mod.rs
git commit -m "panel: drill-in collapse reranking providers on iPhone"
```

---

### Task 6: Fix #1 — acp_harnesses 钻入折叠

**Files:**
- Modify: `interfaces/webchat/src/views/settings/acp_harnesses/mod.rs`

**Interfaces:**
- 信号：`selected_id`（`RwSignal`, L43）、`show_add_form`（`RwSignal`, L44）。
- `DETAIL_ACTIVE` = `selected_id.get().is_some() || show_add_form.get()`。
- 右栏 wrapper 是 `flex-1`（L338）。

- [ ] **Step 1: ViewportState 导入 + 顶部捕获**

顶部加 `use crate::state::viewport::ViewportState;`；L46 后加 `let is_mobile = expect_context::<ViewportState>().is_mobile;`。

- [ ] **Step 2: 门控 auto-select（L64-68）**

```rust
                        if selected_id.get_untracked().is_none() {
                            if let Some(first) = list.first() {
                                selected_id.set(Some(first.id.clone()));
                            }
                        }
```

→

```rust
                        if !is_mobile.get_untracked() && selected_id.get_untracked().is_none() {
                            if let Some(first) = list.first() {
                                selected_id.set(Some(first.id.clone()));
                            }
                        }
```

- [ ] **Step 3: 左 wrapper（L102）**

```rust
            <div class="flex flex-col w-5/12 min-w-[400px] border-r border-border">
```

→

```rust
            <div class="flex flex-col w-5/12 min-w-[400px] max-sm:w-full max-sm:min-w-0 border-r border-border"
                 class=("max-sm:hidden", move || selected_id.get().is_some() || show_add_form.get())>
```

- [ ] **Step 4: 右 wrapper（L338）+ 返回按钮**

```rust
            <div class="flex-1 flex flex-col overflow-hidden">
                {move || {
```

→

```rust
            <div class="flex-1 flex flex-col overflow-hidden max-sm:w-full max-sm:min-w-0"
                 class=("max-sm:hidden", move || !(selected_id.get().is_some() || show_add_form.get()))>
                <button
                    type="button"
                    class="hidden max-sm:flex items-center gap-1 px-4 py-3 text-sm text-text-tertiary active:bg-surface-sunken"
                    aria-label="Back to list"
                    on:click=move |_| { selected_id.set(None); show_add_form.set(false); }
                >
                    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <polyline points="15 18 9 12 15 6" />
                    </svg>
                </button>
                {move || {
```

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/settings/acp_harnesses/mod.rs
git commit -m "panel: drill-in collapse ACP harnesses on iPhone"
```

---

### Task 7: Fix #1 — providers（LLM）钻入折叠

**Files:**
- Modify: `interfaces/webchat/src/views/settings/providers/mod.rs`

**Interfaces:**
- 信号：`selected`（`RwSignal<Option<String>>`）。**无 `show_add_form`**——「添加自定义」用 `selected.set(Some("__new__"))`（L116）作哨兵，故 `DETAIL_ACTIVE` = `selected.get().is_some()`。
- 右栏始终渲染 `ProviderDetailPanel`（内部处理空选）。

- [ ] **Step 1: ViewportState 导入 + 顶部捕获**

顶部加 `use crate::state::viewport::ViewportState;`；`selected` 声明附近加 `let is_mobile = expect_context::<ViewportState>().is_mobile;`。

- [ ] **Step 2: 门控 auto-select（L58）**

```rust
                if selected.get_untracked().is_none() {
```

→

```rust
                if !is_mobile.get_untracked() && selected.get_untracked().is_none() {
```

- [ ] **Step 3: 左 wrapper（L82）**

```rust
            <div class="flex flex-col w-5/12 min-w-0 border-r border-border">
```

→

```rust
            <div class="flex flex-col w-5/12 min-w-0 max-sm:w-full border-r border-border"
                 class=("max-sm:hidden", move || selected.get().is_some())>
```

- [ ] **Step 4: 右 wrapper（L126）+ 返回按钮**

```rust
            <div class="w-7/12 min-w-0 overflow-y-auto">
                <ProviderDetailPanel
```

→

```rust
            <div class="w-7/12 min-w-0 max-sm:w-full overflow-y-auto"
                 class=("max-sm:hidden", move || selected.get().is_none())>
                <button
                    type="button"
                    class="hidden max-sm:flex items-center gap-1 px-4 py-3 text-sm text-text-tertiary active:bg-surface-sunken"
                    aria-label="Back to list"
                    on:click=move |_| selected.set(None)
                >
                    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <polyline points="15 18 9 12 15 6" />
                    </svg>
                </button>
                <ProviderDetailPanel
```

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/settings/providers/mod.rs
git commit -m "panel: drill-in collapse LLM providers on iPhone"
```

---

### Task 8: Fix #1 — search 钻入折叠

**Files:**
- Modify: `interfaces/webchat/src/views/settings/search.rs`

**Interfaces:**
- 信号：`selected`（`RwSignal`, L159）、`show_add_form`（`RwSignal`, L160）。
- `DETAIL_ACTIVE` = `selected.get().is_some() || show_add_form.get()`。
- auto-select 在裸 `spawn_local`（L163-178，非 Effect），mount 跑一次。

- [ ] **Step 1: ViewportState 导入 + 顶部捕获**

顶部加 `use crate::state::viewport::ViewportState;`；L160 后加 `let is_mobile = expect_context::<ViewportState>().is_mobile;`。

- [ ] **Step 2: 门控 auto-select（L167-169）**

```rust
                if !cfg.default_provider.is_empty() {
                    selected.set(Some(cfg.default_provider.clone()));
                }
```

→

```rust
                if !is_mobile.get_untracked() && !cfg.default_provider.is_empty() {
                    selected.set(Some(cfg.default_provider.clone()));
                }
```

- [ ] **Step 3: 左 wrapper（L183）**

```rust
            <div class="flex flex-col w-5/12 min-w-0 border-r border-border">
```

→

```rust
            <div class="flex flex-col w-5/12 min-w-0 max-sm:w-full border-r border-border"
                 class=("max-sm:hidden", move || selected.get().is_some() || show_add_form.get())>
```

- [ ] **Step 4: 右 wrapper（L230）+ 返回按钮**

```rust
            <div class="w-7/12 min-w-0 overflow-y-auto">
                {move || {
```

→

```rust
            <div class="w-7/12 min-w-0 max-sm:w-full overflow-y-auto"
                 class=("max-sm:hidden", move || !(selected.get().is_some() || show_add_form.get()))>
                <button
                    type="button"
                    class="hidden max-sm:flex items-center gap-1 px-4 py-3 text-sm text-text-tertiary active:bg-surface-sunken"
                    aria-label="Back to list"
                    on:click=move |_| { selected.set(None); show_add_form.set(false); }
                >
                    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <polyline points="15 18 9 12 15 6" />
                    </svg>
                </button>
                {move || {
```

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/settings/search.rs
git commit -m "panel: drill-in collapse search providers on iPhone"
```

---

### Task 9: Fix #1 — channels/platform_page 钻入折叠（w-56 变体）

**Files:**
- Modify: `interfaces/webchat/src/views/settings/channels/platform_page.rs`

**Interfaces:**
- 信号：`selected_id`（`RwSignal`, L46）。`show_new_dialog`（L50）是左侧栏内联「新建实例」表单，**不计入** `DETAIL_ACTIVE`。
- `DETAIL_ACTIVE` = `selected_id.get().is_some()`。
- 结构：页头（L176，back-to-channels + 平台身份，**保留不动**）+ body flex（L211）含左 `w-56` 侧栏（L213）+ 右 `flex-1` 详情（L303）。

- [ ] **Step 1: ViewportState 导入 + 顶部捕获**

顶部 `use` 区加 `use crate::state::viewport::ViewportState;`；L53 附近（信号声明后）加 `let is_mobile = expect_context::<ViewportState>().is_mobile;`。

- [ ] **Step 2: 门控 auto-select（L116-118）**

```rust
                    if should_reselect {
                        selected_id.set(list.first().map(|i| i.channel_id.clone()));
                    }
```

→

```rust
                    if should_reselect && !is_mobile.get_untracked() {
                        selected_id.set(list.first().map(|i| i.channel_id.clone()));
                    }
```

- [ ] **Step 3: 左侧栏 wrapper（L213）**

```rust
                <div class="w-56 border-r border-border overflow-y-auto p-3 space-y-1 flex-shrink-0">
```

→

```rust
                <div class="w-56 max-sm:w-full max-sm:min-w-0 border-r border-border overflow-y-auto p-3 space-y-1 flex-shrink-0"
                     class=("max-sm:hidden", move || selected_id.get().is_some())>
```

- [ ] **Step 4: 右详情 wrapper（L303）+ 返回按钮**

```rust
                <div class="flex-1 overflow-y-auto p-6 max-sm:px-4">
                    <div class="max-w-3xl max-sm:max-w-none">
```

→

```rust
                <div class="flex-1 overflow-y-auto p-6 max-sm:px-4 max-sm:w-full max-sm:min-w-0"
                     class=("max-sm:hidden", move || selected_id.get().is_none())>
                    <button
                        type="button"
                        class="hidden max-sm:flex items-center gap-1 -mt-2 mb-1 -ml-1 text-sm text-text-tertiary active:bg-surface-sunken rounded"
                        aria-label="Back to list"
                        on:click=move |_| selected_id.set(None)
                    >
                        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                             stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <polyline points="15 18 9 12 15 6" />
                        </svg>
                    </button>
                    <div class="max-w-3xl max-sm:max-w-none">
```

（注意右栏多一层 `<div class="max-w-3xl …">` 内层，返回按钮放在其**外、wrapper 内**；闭合 `</div>` 数量不变。）

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/settings/channels/platform_page.rs
git commit -m "panel: drill-in collapse channel platform page on iPhone"
```

---

### Task 10: 批量编译 + 构建 + 390px 视觉验证 + 桌面回归

**Files:** 无（验证 + 重建产物）。

- [ ] **Step 1: 一次性 native 编译门（catch 全部 .rs 改动的类型/宏错误）**

```bash
cargo check -p aleph-panel --lib
```

Expected: 编译通过（0 error）。若某页报错，定位该页修正后重跑。

- [ ] **Step 2: 重建 WASM dist**

```bash
just wasm
```

Expected: `npm run build:css` + wasm-release 编译 + wasm-bindgen + wasm-opt 全绿。

- [ ] **Step 3: 提交 dist 产物**

```bash
git add interfaces/webchat/dist
git commit -m "panel: rebuild dist (iPhone Phase 2 mobile fixes)"
```

- [ ] **Step 4: 重编 server 并热换运行中 daemon（rust_embed 编译期嵌入）**

```bash
cargo build -p alephcore --bin aleph-server
```

然后停运行中 daemon（`./target/debug/aleph-server stop` 或对 `pgrep -x aleph-server` 的 PID `kill`）→ 重启 → `curl -s http://127.0.0.1:18790/aleph_panel_bg.wasm | wc -c` 应等于 `ls -l interfaces/webchat/dist/aleph_panel_bg.wasm` 字节数（确认嵌入最新）。

- [ ] **Step 5: 390px 设备仿真逐页复看**

chrome-devtools `emulate viewport=390x844x3,mobile,touch` → reload，逐项确认：
- settings landing「基础」、agents H1、memory「记忆库」标题**不再被顶栏遮挡**（#2）
- chat 顶栏中间无重复 agent 名（#3）
- 嵌入/生成/重排序/ACP/LLM提供商/搜索/频道平台页：无选中=全宽列表；点选=全宽详情 + 左上 `‹` 返回；返回回到列表（#1）

- [ ] **Step 6: 桌面 ≥640px 回归**

`emulate viewport=1280x900x2` → reload，确认上述 7 页仍为左右双栏、顶栏偏移、landing 等 ≥640 表现与改前一致（`max-sm:*` 不生效）。

---

## Self-Review

**Spec coverage:**
- #2 → Task 1 ✅（含 agents「过挤」随之消除，Task 10 Step 5 验证）。
- #3 → Task 2 ✅。
- #1 七页 → Task 3-9（embedding/generation/reranking/acp/providers/search/platform_page）✅。
- 列表优先（`!is_mobile` auto-select 门控）→ 每个 #1 task Step 2 ✅。
- iPad/桌面不变 → `max-sm:` 门控 + Task 10 Step 6 回归 ✅。
- 零新 i18n key → 返回按钮图标 + `aria-label` ✅。

**Placeholder scan:** 无 TBD/TODO；每个改动步骤给出确切 before→after 代码与行号；返回按钮闭包体逐页指明清空的信号。

**Type/signal consistency:**
- `DETAIL_ACTIVE` 各页用其真实信号：embedding/generation `selected_provider_id`+`show_add_form`；reranking `selected_provider`+`show_add_form`；acp `selected_id`+`show_add_form`；providers `selected`（无 add-form）；search `selected`+`show_add_form`；platform_page `selected_id`。
- `signal()` 对（`set_*`）用 `set_x.set(None)`；`RwSignal` 用 `x.set(None)`——返回按钮 on:click 已按各页信号种类区分（embedding/generation 用 `set_*`；reranking/acp/providers/search/platform 用 RwSignal 直接 `.set`）。
- `is_mobile`（`RwSignal<bool>`）`.get_untracked()` → bool，全页一致。

## Non-Goals（YAGNI）

- iPad 专属布局/触控优化、Android 专项、堆叠式折叠、抽共享 master-detail 组件——均不在本计划。
