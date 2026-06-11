# 侧栏菜单项玻璃化视觉刷新 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 Panel 左栏六处菜单项的选中/悬停态统一为同一套"磨砂内嵌瓷砖"材质（`.nav-tile` / `.nav-tile-active`），消除两套不一致的选中词汇，并把选中态显著性上调到"一眼可辨"。

**Architecture:** 新增两个 `@layer components` 组件类承载材质（颜色/着色/描边/高光），所有数值基于自适应 token 做 `color-mix`，单一定义自动跟随 light/dark/glass 三主题 + 四 accent 色板。六处调用点把原本写死的选中/悬停 utility 串替换为组件类，布局类（flex/gap/padding/rounded）保留在调用点。纯视觉，无逻辑改动。

**Tech Stack:** Leptos (WASM) + Tailwind v4 CSS（`interfaces/webchat/styles/tailwind.css`，`@layer components`）。

---

## 关键设计决策（实现者必读）

1. **`.nav-tile` 与 `.nav-tile-active` 互斥使用，不叠加。** 每个调用点的 `if is_active { ... } else { ... }` 结构里：active 分支只用 `nav-tile-active`，inactive 分支只用 `nav-tile`。

   **为什么不叠加**：若 active 行同时带 `.nav-tile`，则 `.nav-tile:hover`（特异度 0,2,0）会盖过 `.nav-tile-active`（0,1,0），导致鼠标悬停在选中行上时 accent 着色被冲成中性灰。互斥使用彻底规避此陷阱——选中行没有 `:hover` 规则，悬停保持 accent。

2. **两个类都预置 `border: 1px solid transparent`**，保证 active↔inactive 切换时盒模型不变、行间零位移。`.nav-tile-active` 只改 `border-color`，不改 border 宽度。

3. **显著性数值**（本轮精修目标，对应 spec 的"效果不明显"修正）：着色 22%、描边 38%、inset 高光 0.10。

4. **删除 utility 串里这些属性**（已被组件类接管，保留会冲突或冗余）：`bg-sidebar-active` / `bg-sidebar-active/50` 等、`bg-primary/10` / `bg-primary/12`、`text-sidebar-accent` / `text-primary` / `text-text-secondary`、`hover:*`、`font-medium`、`transition-all duration-200` / `transition-colors`。**保留**布局类：`flex items-center gap-* px-* py-* rounded-lg text-sm mx-2 w-full text-left relative group justify-between` 等。

## 测试说明（重要——非常规 TDD）

本计划是纯 CSS/markup 视觉改动，**没有有意义的失败-先行单元测试**：断言 class 字符串字面量只会重复我们刚写的内容、脆弱且零价值（违反"修实现不修测试"）。本项目既有惯例（见前几轮玻璃刷新）即用 **wasm 构建 + clippy + 既有测试回归 + chrome-devtools 三主题截图** 验收。本计划遵循该惯例：每个编辑任务以 **grep 验证旧词汇已消除** 作为即时检查，最终任务做完整构建 + 视觉核验。

## File Structure

| 文件 | 责任 | 改动 |
|------|------|------|
| `interfaces/webchat/styles/tailwind.css` | 全局样式单一来源 | 新增 `.nav-tile`/`.nav-tile-active` 组件类 + reduced-transparency 回退 |
| `interfaces/webchat/src/components/sidebar/sidebar_item.rs` | Dashboard 导航行（共享 `SidebarItem` 组件） | 归一 + 删左光条 |
| `interfaces/webchat/src/components/mode_sidebar.rs` | Settings tabs 导航行 | 归一 |
| `interfaces/webchat/src/components/agents_sidebar.rs` | 智能体列表行 | 归一 |
| `interfaces/webchat/src/components/chat_sidebar.rs` | 对话历史会话行（旧 `bg-primary/10` 离群） | 归一 |
| `interfaces/webchat/src/components/nav_menu.rs` | 底部 section 切换器：触发器 + 弹窗项（弹窗项旧 `bg-primary/12` 离群） | 归一 |
| `interfaces/webchat/src/views/teams/mod.rs` | TeamsSidebar 导航行 | 归一 |

---

## Task 1: 新增 `.nav-tile` / `.nav-tile-active` 组件类 + 无障碍回退

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css`（在 `@layer components` 块内 `.glass-inset { ... }` 之后、该块闭合 `}` 之前插入；回退插入 `prefers-reduced-transparency` media 块内 `.glass-inset` 回退之后）

- [ ] **Step 1: 在 `@layer components` 块内追加两个组件类**

定位 `.glass-inset { ... }` 规则（约 608–612 行）紧随其后的内容，在该 `@layer components` 块的闭合 `}`（约 613 行）**之前**插入：

```css
  /* Sidebar nav items — faux frosted inset tile. Used MUTUALLY EXCLUSIVELY:
     inactive rows get `.nav-tile`, the selected row gets `.nav-tile-active`
     (never both — see note below). All colours derive from adaptive tokens
     via color-mix, so light/dark/glass + the four accent palettes follow
     automatically. Zero backdrop-filter (nav items are many + route-switch
     frequently; the tile sits on .aleph-sidebar's real frosted layer). */
  .nav-tile,
  .nav-tile-active {
    border: 1px solid transparent;        /* placeholder: equal box model both states → zero layout shift */
    transition: background .15s, color .15s, box-shadow .15s, border-color .15s;
  }
  .nav-tile { color: var(--color-text-secondary); }
  .nav-tile:hover {
    background-color: color-mix(in oklch, var(--color-text-primary) 8%, transparent);
    color: var(--color-text-primary);
  }
  /* NOTE: active rows intentionally carry ONLY .nav-tile-active (not .nav-tile),
     so no .nav-tile:hover rule washes out the accent when a selected row is
     hovered. */
  .nav-tile-active {
    color: var(--color-sidebar-accent);
    background-color: color-mix(in oklch, var(--color-sidebar-accent) 22%, transparent);
    border-color: color-mix(in oklch, var(--color-sidebar-accent) 38%, transparent);
    box-shadow: inset 0 1px 0 oklch(1 0 0 / 0.10);   /* top highlight: source of glassiness */
    font-weight: 600;
  }
```

- [ ] **Step 2: 在 reduced-transparency media 块内追加不透明回退**

定位 `@media (prefers-reduced-transparency: reduce) { ... }` 块内的 `.glass-inset { background-color: var(--color-surface-sunken) !important; }`（约 660 行），在其**之后、该 media 块闭合 `}`（约 661 行）之前**插入：

```css
  /* Sidebar nav tile → opaque solid; drop the faux-glass highlight. */
  .nav-tile-active {
    background-color: var(--color-sidebar-active) !important;
    box-shadow: none !important;
  }
```

- [ ] **Step 3: 验证 CSS 语法 + 类已写入**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
grep -n "nav-tile" interfaces/webchat/styles/tailwind.css
```
Expected: 至少 6 处命中（`.nav-tile,` / `.nav-tile-active` 分组、`.nav-tile {` color、`.nav-tile:hover`、`.nav-tile-active {` 主定义、回退块里的 `.nav-tile-active`）。确认主定义在 `@layer components` 内、回退在 `prefers-reduced-transparency` media 内。

- [ ] **Step 4: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git add interfaces/webchat/styles/tailwind.css
git commit -m "panel: add .nav-tile/.nav-tile-active frosted sidebar tile classes"
```

---

## Task 2: sidebar_item.rs（Dashboard 导航行）归一 + 删左光条

**Files:**
- Modify: `interfaces/webchat/src/components/sidebar/sidebar_item.rs:33-43`

- [ ] **Step 1: 替换 active/inactive class 串**

找到（约 33–39 行）：

```rust
        <A href=href attr:class=move || {
            if is_active() {
                "relative group flex items-center gap-3 px-3 py-2 rounded-lg text-sidebar-accent bg-sidebar-active transition-all duration-200 font-medium"
            } else {
                "relative group flex items-center gap-3 px-3 py-2 rounded-lg text-text-secondary hover:text-text-primary hover:bg-sidebar-active/50 transition-all duration-200"
            }
        }>
```

替换为：

```rust
        <A href=href attr:class=move || {
            if is_active() {
                "nav-tile-active relative group flex items-center gap-3 px-3 py-2 rounded-lg"
            } else {
                "nav-tile relative group flex items-center gap-3 px-3 py-2 rounded-lg"
            }
        }>
```

- [ ] **Step 2: 删除绝对定位左光条**

找到并**删除**（约 40–43 行）：

```rust
            // Active indicator bar
            {move || is_active().then(|| view! {
                <div class="absolute left-0 top-1/2 -translate-y-1/2 w-0.5 h-5 bg-sidebar-accent rounded-full"></div>
            })}
```

（瓷砖已标识选中，左光条是双重指示。删除后 `relative group` 仍被 icon 的 badge 定位与 `group-hover` 使用，保留。）

- [ ] **Step 3: 验证旧词汇已消除**

Run:
```bash
grep -nE "bg-sidebar-active|text-sidebar-accent|indicator bar|w-0.5 h-5" interfaces/webchat/src/components/sidebar/sidebar_item.rs
```
Expected: 无输出（空）。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/components/sidebar/sidebar_item.rs
git commit -m "panel: unify dashboard sidebar item to nav-tile, drop left bar"
```

---

## Task 3: mode_sidebar.rs（Settings tabs）归一

**Files:**
- Modify: `interfaces/webchat/src/components/mode_sidebar.rs:311-315`

- [ ] **Step 1: 替换 active/inactive class 串**

找到（约 310–316 行）：

```rust
                                            attr:class=move || {
                                                if is_active() {
                                                    "flex items-center gap-3 px-3 py-2 rounded-lg text-sm transition-all duration-200 bg-sidebar-active text-sidebar-accent font-medium"
                                                } else {
                                                    "flex items-center gap-3 px-3 py-2 rounded-lg text-sm transition-all duration-200 hover:bg-sidebar-active/50 text-text-secondary hover:text-text-primary"
                                                }
                                            }
```

替换为：

```rust
                                            attr:class=move || {
                                                if is_active() {
                                                    "nav-tile-active flex items-center gap-3 px-3 py-2 rounded-lg text-sm"
                                                } else {
                                                    "nav-tile flex items-center gap-3 px-3 py-2 rounded-lg text-sm"
                                                }
                                            }
```

（注意：约 321–324 行 icon `<svg>` 自带 `text-sidebar-accent`/`text-text-tertiary` 切换，是独立的图标着色，**不在本任务范围，保持不动**。）

- [ ] **Step 2: 验证旧词汇已消除（仅 nav 行，icon 的 text-sidebar-accent 仍在属预期）**

Run:
```bash
grep -nE "bg-sidebar-active" interfaces/webchat/src/components/mode_sidebar.rs
```
Expected: 无输出（空）。`text-sidebar-accent` 仍会在 icon svg 行命中——属预期，不处理。

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/components/mode_sidebar.rs
git commit -m "panel: unify settings sidebar tabs to nav-tile"
```

---

## Task 4: agents_sidebar.rs（智能体列表行）归一

**Files:**
- Modify: `interfaces/webchat/src/components/agents_sidebar.rs:185-189`

- [ ] **Step 1: 替换 active/inactive class 串**

找到（约 184–190 行）：

```rust
                                            class=move || {
                                                if is_active {
                                                    "flex items-center gap-2 px-4 py-2 mx-2 rounded-lg text-sm bg-sidebar-active text-sidebar-accent font-medium"
                                                } else {
                                                    "flex items-center gap-2 px-4 py-2 mx-2 rounded-lg text-sm hover:bg-sidebar-active/50 text-text-secondary hover:text-text-primary"
                                                }
                                            }
```

替换为：

```rust
                                            class=move || {
                                                if is_active {
                                                    "nav-tile-active flex items-center gap-2 px-4 py-2 mx-2 rounded-lg text-sm"
                                                } else {
                                                    "nav-tile flex items-center gap-2 px-4 py-2 mx-2 rounded-lg text-sm"
                                                }
                                            }
```

（注意：约 198 行 channel_badge 的 `bg-primary/10 text-primary` 是**徽章**不是 nav 行，**不在范围，保持不动**。）

- [ ] **Step 2: 验证旧词汇已消除**

Run:
```bash
grep -nE "bg-sidebar-active|text-sidebar-accent" interfaces/webchat/src/components/agents_sidebar.rs
```
Expected: 无输出（空）。

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/components/agents_sidebar.rs
git commit -m "panel: unify agents sidebar list to nav-tile"
```

---

## Task 5: chat_sidebar.rs（对话历史会话行）归一 — 消除 accent-purple 离群

**Files:**
- Modify: `interfaces/webchat/src/components/chat_sidebar.rs:781-788`

- [ ] **Step 1: 替换 format! 的 base + 两分支**

找到（约 781–788 行）：

```rust
                                                    class=move || format!(
                                                        "w-full text-left px-3 py-2.5 rounded-lg text-sm transition-colors flex items-center justify-between {}",
                                                        if is_active() {
                                                            "bg-primary/10 text-primary font-medium"
                                                        } else {
                                                            "text-text-secondary hover:bg-surface-sunken hover:text-text-primary"
                                                        }
                                                    )
```

替换为：

```rust
                                                    class=move || format!(
                                                        "w-full text-left px-3 py-2.5 rounded-lg text-sm flex items-center justify-between {}",
                                                        if is_active() {
                                                            "nav-tile-active"
                                                        } else {
                                                            "nav-tile"
                                                        }
                                                    )
```

（从 base 串移除了 `transition-colors`——`.nav-tile`/`.nav-tile-active` 自带 transition；保留 `transition-colors` 这个 utility 会因 Tailwind v4 utilities 层优先级高于 components 层而盖掉组件类的 box-shadow 过渡。）

- [ ] **Step 2: 验证旧词汇已消除**

Run:
```bash
grep -nE "bg-primary/10|text-primary font-medium" interfaces/webchat/src/components/chat_sidebar.rs
```
Expected: 无输出（空）。

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/components/chat_sidebar.rs
git commit -m "panel: unify chat history rows to nav-tile (drop accent-purple outlier)"
```

---

## Task 6: nav_menu.rs（底部 section 切换器：触发器 + 弹窗项）归一

**Files:**
- Modify: `interfaces/webchat/src/components/nav_menu.rs:88-95`（触发器）与 `:138-145`（弹窗项）

- [ ] **Step 1: 替换触发器（trigger）class**

找到（约 88–95 行）：

```rust
                class=move || {
                    let base = "w-full flex items-center gap-2.5 px-3 py-2 rounded-lg text-sm transition-colors";
                    if open.get() {
                        format!("{base} bg-sidebar-active text-text-primary")
                    } else {
                        format!("{base} text-text-secondary hover:text-text-primary hover:bg-sidebar-active/60")
                    }
                }
```

替换为：

```rust
                class=move || {
                    let base = "w-full flex items-center gap-2.5 px-3 py-2 rounded-lg text-sm";
                    if open.get() {
                        format!("{base} nav-tile-active")
                    } else {
                        format!("{base} nav-tile")
                    }
                }
```

- [ ] **Step 2: 替换弹窗项（popup item）class**

找到（约 138–145 行）：

```rust
                                class=move || {
                                    let base = "w-full flex items-center gap-2.5 px-3 py-2 rounded-lg text-sm transition-colors";
                                    if is_active() {
                                        format!("{base} bg-primary/12 text-primary font-medium")
                                    } else {
                                        format!("{base} text-text-secondary hover:bg-sidebar-active/70 hover:text-text-primary")
                                    }
                                }
```

替换为：

```rust
                                class=move || {
                                    let base = "w-full flex items-center gap-2.5 px-3 py-2 rounded-lg text-sm";
                                    if is_active() {
                                        format!("{base} nav-tile-active")
                                    } else {
                                        format!("{base} nav-tile")
                                    }
                                }
```

（弹窗本身的 `.glass` 真磨砂层保持不动；瓷砖坐其上。）

- [ ] **Step 3: 验证旧词汇已消除**

Run:
```bash
grep -nE "bg-sidebar-active|bg-primary/12|text-primary font-medium" interfaces/webchat/src/components/nav_menu.rs
```
Expected: 无输出（空）。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/components/nav_menu.rs
git commit -m "panel: unify nav menu trigger + popup items to nav-tile"
```

---

## Task 7: views/teams/mod.rs（TeamsSidebar）归一

**Files:**
- Modify: `interfaces/webchat/src/views/teams/mod.rs:153-159`

- [ ] **Step 1: 替换 active/inactive class 串**

找到（约 153–159 行）：

```rust
            class=move || {
                if is_active() {
                    "w-full flex items-center px-3 py-2 rounded-lg text-sm bg-sidebar-active text-sidebar-accent font-medium"
                } else {
                    "w-full flex items-center px-3 py-2 rounded-lg text-sm hover:bg-sidebar-active/50 text-text-secondary hover:text-text-primary"
                }
            }
```

替换为：

```rust
            class=move || {
                if is_active() {
                    "nav-tile-active w-full flex items-center px-3 py-2 rounded-lg text-sm"
                } else {
                    "nav-tile w-full flex items-center px-3 py-2 rounded-lg text-sm"
                }
            }
```

- [ ] **Step 2: 验证旧词汇已消除**

Run:
```bash
grep -nE "bg-sidebar-active|text-sidebar-accent" interfaces/webchat/src/views/teams/mod.rs
```
Expected: 无输出（空）。

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/views/teams/mod.rs
git commit -m "panel: unify teams sidebar to nav-tile"
```

---

## Task 8: 全仓回归 — 构建、lint、测试、重建 dist、视觉核验

**Files:**
- Modify (生成物): `interfaces/webchat/dist/tailwind.css`、`interfaces/webchat/dist/aleph_panel*.{js,wasm}`、`interfaces/webchat/dist/index.html`

- [ ] **Step 1: 全仓确认旧词汇在六处 nav 行已清零**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
grep -rnE "bg-sidebar-active|bg-primary/1[02]" \
  interfaces/webchat/src/components/sidebar/sidebar_item.rs \
  interfaces/webchat/src/components/mode_sidebar.rs \
  interfaces/webchat/src/components/agents_sidebar.rs \
  interfaces/webchat/src/components/chat_sidebar.rs \
  interfaces/webchat/src/components/nav_menu.rs \
  interfaces/webchat/src/views/teams/mod.rs
```
Expected: 无输出（空）。`text-sidebar-accent` 仅应在 mode_sidebar.rs 的 **icon svg** 行残留（属预期）。

- [ ] **Step 2: WASM target 编译检查（强制——native check 漏 cfg(wasm32) 门控代码）**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
cargo build -p aleph-panel --lib --target wasm32-unknown-unknown
```
Expected: 编译通过，无 error。（`--lib` 必需——`just wasm` 同款；否则 cdylib/残留 bin 会触发 bitcode 加载错。若 worktree 共享 target-dir 报路径异常，按 CLAUDE.md "Panel↔Daemon 资源嵌入链" 用绝对 target 路径。）

- [ ] **Step 3: clippy 干净**

Run:
```bash
cargo clippy -p aleph-panel --lib --target wasm32-unknown-unknown 2>&1 | tail -20
```
Expected: 无新增 warning（treat warnings as errors 口径）。

- [ ] **Step 4: panel 既有测试回归**

Run:
```bash
cargo test -p aleph-panel 2>&1 | tail -20
```
Expected: 全部通过（349+ 测试，0 失败）。纯视觉改动不应影响任何测试。

- [ ] **Step 5: 重建 dist 包（让 rust_embed 烧入新样式）**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
just wasm
```
Expected: `interfaces/webchat/dist/{tailwind.css, aleph_panel.js, aleph_panel_bg.wasm, index.html}` 更新。`git status` 应显示 dist 下文件变更。
（若 worktree 致 `just wasm` 路径错配，按 CLAUDE.md 用绝对 target 路径手动跑 wasm-bindgen + wasm-opt + tailwind 生成。）

- [ ] **Step 6: 视觉核验（不污染对话历史）**

按 spec「验证」段：重编 `aleph-server` binary 嵌入新 dist → 替换运行中 .app binary、supervisor 重拉 → `aleph-server bootstrap-url` 认证 → chrome-devtools 注入 `position:fixed` 覆盖层，用真实 `.aleph-sidebar` + 真实 token 渲染六处侧栏选中/悬停态 → 切 **dark / glass / light 三主题**（+ 抽查 ocean/forest/sunset/rose accent 色板）截图。

**重点核对**：
- 六处选中行材质一致，Chat / NavMenu 的 accent-purple 离群消失；
- 选中态显著强于改前中性灰块，**light 主题下 22% 着色 + 38% 描边一眼可辨**但不刺眼；
- active↔inactive 切换行间零位移、严格对齐；
- 系统开启 "Reduce transparency" 时选中行为不透明实底、无高光。

- [ ] **Step 7: Commit 生成物**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git add interfaces/webchat/dist
git commit -m "panel: rebuild dist with unified nav-tile sidebar material"
```

---

## Self-Review（计划作者已核对）

- **Spec coverage**：spec 落地清单 6 调用点 → Task 2–7 一一对应；CSS 材质规格 + 无障碍回退 → Task 1；显著性数值（22/38/0.10）→ Task 1 Step 1/2；验证段（wasm 构建 + 三主题截图 + reduced-transparency）→ Task 8。spec 明确排除项（command_palette / 内容区树 / MemorySidebar）未建任务，符合预期。
- **Placeholder scan**：无 TBD/TODO；每个编辑步骤给出完整前后字符串与精确文件行。
- **Type/命名一致性**：组件类名 `.nav-tile` / `.nav-tile-active` 在 Task 1 定义、Task 2–7 全程一致引用；互斥使用规则在六处统一落实。
