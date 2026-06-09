# Panel 玻璃效果细节打磨与性能优化 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 Panel 的玻璃材质统一为单一来源、按"持久/瞬时"分级模糊以压低常驻 GPU 占用，并消除假玻璃/零散模糊的不一致。

**Architecture:** 纯 CSS + Leptos class 字符串改动，零原生 Bridge 改动（红线 R2）。新增 4 个分级模糊 token（瞬时弹出层 / 常驻侧栏 / 内容内小模糊 / 全屏遮罩），把 6 个弹出层统一到真 `.glass`、5 处遮罩收口到 `.aleph-scrim`、2 处小模糊收口到 `.aleph-blur-subtle`，并扩展 reduced-transparency 安全阀。

**Tech Stack:** Tailwind CSS (v4 风格 `styles/tailwind.css`) + Leptos (Rust cdylib `aleph_panel`)。

**测试约定（重要）:** 本计划是纯 CSS/标记改动，无业务逻辑，因此**不写单元测试**——这符合本仓 panel CSS 改动的既有惯例。每个任务的"验证"由三类构成：(a) `cargo check -p aleph-panel` 编译通过；(b) `grep` 断言（确认死类已删除/迁移已完成/无残留）；(c) 最终任务的 `just wasm` 构建 + 人工核验（部署因 rust_embed 烧 dist 进 binary 而 DEFERRED）。**panel 必须用 `-p aleph-panel`，不是 `-p alephcore`**（见 spec / 历史教训）。

**关键文件:**
- `interfaces/webchat/styles/tailwind.css` — 所有 CSS（token、`.glass` 材质、`.aleph-scrim`、`.aleph-blur-subtle`、侧栏、reduced-transparency）
- `interfaces/webchat/src/components/{command_palette,directory_browser,notification_center,model_picker,nav_menu,theme_toggle}.rs` — 弹出层 + 部分遮罩
- `interfaces/webchat/src/views/teams/overview.rs`、`src/components/{boot_check_gate,service_blocking_gate}.rs` — 遮罩
- `interfaces/webchat/src/components/mode_sidebar.rs`、`src/views/canvas/minimap_view.rs` — 内容内小模糊

所有相对路径以 `interfaces/webchat/` 为根。

---

### Task 1: 分级模糊 Token 架构

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css:160-162`（:root token 默认值）
- Modify: `interfaces/webchat/styles/tailwind.css:737-738`（`html.glass` 玻璃档覆盖）

- [ ] **Step 1: 在 :root 扩展分级 token**

把当前的：
```css
  /* Glass material strength — overridden by html.glass to intensify. */
  --glass-blur: 23px;
  --glass-saturate: 1.6;
```
替换为：
```css
  /* Tiered blur strengths. Transient popovers (`--glass-blur`) may be lush;
     the always-on sidebar (`--glass-blur-chrome`) stays disciplined; small
     in-content frosts and the full-viewport scrim are deliberately light.
     `html.glass` intensifies the transient + chrome tiers only. */
  --glass-blur: 20px;
  --glass-saturate: 1.6;
  --glass-blur-chrome: 16px;
  --glass-blur-subtle: 8px;
  --scrim-blur: 2px;
```

- [ ] **Step 2: 在 `html.glass` 提升瞬时层并设置常驻层**

把当前的（约 L737-738，`html.glass {` 块内）：
```css
  /* Intensified glass material — pushed past the restrained default. */
  --glass-blur: 30px;
  --glass-saturate: 1.9;
```
替换为：
```css
  /* Intensified glass material — pushed past the restrained default.
     Transient popovers go lush (34px); the always-on sidebar tier
     (--glass-blur-chrome) stays at 24px to cap idle GPU cost. */
  --glass-blur: 34px;
  --glass-saturate: 2.0;
  --glass-blur-chrome: 24px;
```

- [ ] **Step 3: 验证 token 已就位**

Run: `cd interfaces/webchat && grep -n -- "--glass-blur-chrome\|--glass-blur-subtle\|--scrim-blur" styles/tailwind.css`
Expected: 至少 4 行命中（:root 各 1 行 + `html.glass` 的 chrome 1 行）。

- [ ] **Step 4: 编译检查**

Run: `cd interfaces/webchat && cargo check -p aleph-panel`
Expected: 编译通过（CSS 改动不影响 Rust 编译，此步确认未误改 Rust）。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/styles/tailwind.css
git commit -m "panel(glass): introduce tiered blur tokens (transient/chrome/subtle/scrim)"
```

---

### Task 2: 统一 `.glass` 材质打磨

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css:438-442`（`.glass` 基础规则）
- Modify: `interfaces/webchat/styles/tailwind.css:473-476`（`html.glass .glass::before` 亮边）

材质定稿（可视化已确认）：B 的模糊已在 Task 1 提为 34/2.0，本任务补 C 的亮边 + 斜向高光。高光折进 `background-image`（不占用伪元素），亮边走 `::before`、颗粒走 `::after`（已是 0.5，不改）。景深沿用各弹出层现有的 `shadow-xl`/`shadow-2xl` Tailwind 工具类，**不在 `.glass` 里再加 box-shadow**（避免与工具类双重阴影冲突）。

- [ ] **Step 1: `.glass` 基础规则加斜向高光**

把当前的（@layer components 内）：
```css
  .glass {
    position: relative;
    backdrop-filter: blur(var(--glass-blur)) saturate(var(--glass-saturate));
    -webkit-backdrop-filter: blur(var(--glass-blur)) saturate(var(--glass-saturate));
  }
```
替换为：
```css
  .glass {
    position: relative;
    backdrop-filter: blur(var(--glass-blur)) saturate(var(--glass-saturate));
    -webkit-backdrop-filter: blur(var(--glass-blur)) saturate(var(--glass-saturate));
    /* Diagonal sheen folded into the surface itself — leaves ::before for the
       bezel and ::after for grain. Composes with the element's bg-color
       utility (different property), so no conflict with bg-surface-overlay. */
    background-image: linear-gradient(160deg, oklch(1 0 0 / 0.06), transparent 42%);
  }
```

- [ ] **Step 2: 提亮 Glass 档亮边描边**

把当前的：
```css
html.glass .glass::before {
  background: linear-gradient(180deg,
    oklch(1 0 0 / 0.55), oklch(1 0 0 / 0.08) 50%, oklch(1 0 0 / 0));
}
```
替换为：
```css
html.glass .glass::before {
  background: linear-gradient(180deg,
    oklch(1 0 0 / 0.62), oklch(1 0 0 / 0.10) 50%, oklch(1 0 0 / 0.02));
}
```

- [ ] **Step 3: 验证**

Run: `cd interfaces/webchat && grep -n "linear-gradient(160deg" styles/tailwind.css && grep -n "oklch(1 0 0 / 0.62)" styles/tailwind.css`
Expected: 两条 grep 各至少 1 行命中。

- [ ] **Step 4: 编译检查**

Run: `cd interfaces/webchat && cargo check -p aleph-panel`
Expected: 通过。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/styles/tailwind.css
git commit -m "panel(glass): unify .glass material — diagonal sheen + brighter bezel"
```

---

### Task 3: `.aleph-scrim` + `.aleph-blur-subtle` 辅助类 与降级

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css`（在 `.glass` 相关规则之后、`@media (prefers-reduced-transparency)` 之前插入新类；约 L484 处）
- Modify: `interfaces/webchat/styles/tailwind.css:491-507`（reduced-transparency 媒体查询）

`.aleph-scrim` 只设 `backdrop-filter`，不设 `background-color`——各遮罩点保留自己的变暗底色（`bg-black/40` 或 `bg-surface/95|85`），统一的只是模糊 token。

- [ ] **Step 1: 新增两个辅助类**

在 `:root:not(.dark):not(.glass) .glass::after { opacity: 0.25; }`（约 L483）之后插入：
```css

/* ============================================================
   Shared blur helpers — drive the full-viewport modal scrim and the
   light in-content frosts off the tiered tokens, so they follow the
   theme and the reduced-transparency safety valve. The scrim sets only
   backdrop-filter; each call site keeps its own dim background colour.
   ============================================================ */
.aleph-scrim {
  backdrop-filter: blur(var(--scrim-blur));
  -webkit-backdrop-filter: blur(var(--scrim-blur));
}
.aleph-blur-subtle {
  backdrop-filter: blur(var(--glass-blur-subtle));
  -webkit-backdrop-filter: blur(var(--glass-blur-subtle));
}
```

- [ ] **Step 2: 降级——把新 token 在 reduced-transparency 下归零**

把当前 reduced-transparency 块内的 `:root { ... }`（约 L498-501）：
```css
  :root {
    --aleph-canvas-base: var(--aleph-solid-ground) !important;
    --color-surface-raised: oklch(1.00 0 0) !important;
  }
```
替换为：
```css
  :root {
    --aleph-canvas-base: var(--aleph-solid-ground) !important;
    --color-surface-raised: oklch(1.00 0 0) !important;
    --scrim-blur: 0px !important;
    --glass-blur-subtle: 0px !important;
    --glass-blur-chrome: 0px !important;
  }
```
并在同一媒体查询里，把 `.aleph-scrim` / `.aleph-blur-subtle` 显式中和，紧跟现有 `.glass::before, .glass::after, .aleph-sidebar::before { display: none !important; }`（约 L496）之后插入：
```css
  .aleph-scrim, .aleph-blur-subtle {
    backdrop-filter: none !important;
    -webkit-backdrop-filter: none !important;
  }
```

- [ ] **Step 3: 验证**

Run: `cd interfaces/webchat && grep -n "\.aleph-scrim\|\.aleph-blur-subtle" styles/tailwind.css`
Expected: 至少 4 行命中（2 个类定义 + reduced-transparency 中和 1 行含两者 + 可能换行）。

- [ ] **Step 4: 编译检查**

Run: `cd interfaces/webchat && cargo check -p aleph-panel`
Expected: 通过。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/styles/tailwind.css
git commit -m "panel(glass): add .aleph-scrim/.aleph-blur-subtle helpers + reduced-transparency"
```

---

### Task 4: 侧栏常驻模糊解耦到 `--glass-blur-chrome`

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css:814-815`（`.aleph-sidebar::before`）

侧栏是唯一全程挂载的 backdrop-filter，是常驻 GPU 税的来源。把它的模糊从瞬时 token 改到常驻 token，使 Glass 档闲时停在 24px 而非 34px。

- [ ] **Step 1: 改 backdrop-filter 的 blur 来源**

把当前的：
```css
  backdrop-filter: blur(var(--glass-blur)) saturate(var(--glass-saturate));
  -webkit-backdrop-filter: blur(var(--glass-blur)) saturate(var(--glass-saturate));
```
（注意：此处是 `.aleph-sidebar::before` 块内，约 L814-815，**不是** `.glass` 块）替换为：
```css
  backdrop-filter: blur(var(--glass-blur-chrome)) saturate(var(--glass-saturate));
  -webkit-backdrop-filter: blur(var(--glass-blur-chrome)) saturate(var(--glass-saturate));
```

- [ ] **Step 2: 验证只改了侧栏那处**

Run: `cd interfaces/webchat && grep -n "glass-blur-chrome) saturate" styles/tailwind.css`
Expected: 恰好 2 行（backdrop-filter + -webkit-backdrop-filter）。
Run: `cd interfaces/webchat && grep -c "blur(var(--glass-blur)) saturate" styles/tailwind.css`
Expected: `2`（仅剩 `.glass` 基础规则那一处的两行；侧栏已不再命中）。

- [ ] **Step 3: 编译检查**

Run: `cd interfaces/webchat && cargo check -p aleph-panel`
Expected: 通过。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/styles/tailwind.css
git commit -m "panel(glass): decouple always-on sidebar blur to --glass-blur-chrome"
```

---

### Task 5: 弹出层卡片全部统一为真 `.glass`

**Files:**
- Modify: `interfaces/webchat/src/components/command_palette.rs:344-345`
- Modify: `interfaces/webchat/src/components/directory_browser.rs:344`
- Modify: `interfaces/webchat/src/components/notification_center.rs:107`
- Modify: `interfaces/webchat/src/components/model_picker.rs:127-128`
- Modify: `interfaces/webchat/src/components/nav_menu.rs:126-127`
- Modify: `interfaces/webchat/src/components/theme_toggle.rs:127-128`

死类 `glass-surface` 全部删除；假玻璃加 `glass` 类并把底色 alpha 降到 `/85` 让模糊透出；真玻璃统一到 `/85`。

- [ ] **Step 1: command_palette — 加 glass、删死类、/95→/85**

把：
```
                       glass-surface bg-surface-overlay/95 border border-border \
```
替换为：
```
                       glass bg-surface-overlay/85 border border-border \
```

- [ ] **Step 2: directory_browser — 同上**

把：
```
                       glass-surface bg-surface-overlay/95 border border-border \
```
替换为：
```
                       glass bg-surface-overlay/85 border border-border \
```

- [ ] **Step 3: notification_center — 同上**

把：
```
                       glass-surface bg-surface-overlay/95 border border-border \
```
替换为：
```
                       glass bg-surface-overlay/85 border border-border \
```

- [ ] **Step 4: model_picker — 加 glass、/95→/85、删裸 backdrop-blur-md**

把：
```
                            rounded-xl border border-border bg-surface-overlay/95 shadow-xl
                            backdrop-blur-md p-2 space-y-1">
```
替换为：
```
                            glass rounded-xl border border-border bg-surface-overlay/85 shadow-xl
                            p-2 space-y-1">
```

- [ ] **Step 5: nav_menu — 删死类 glass-surface、/90→/85**

把（L126，注意保留 `glass`）：
```
                <div class="glass-surface glass animate-pop-in absolute bottom-full left-2 right-2 mb-2 z-50
                            rounded-xl border border-border bg-surface-overlay/90 shadow-xl p-1.5 space-y-0.5">
```
替换为：
```
                <div class="glass animate-pop-in absolute bottom-full left-2 right-2 mb-2 z-50
                            rounded-xl border border-border bg-surface-overlay/85 shadow-xl p-1.5 space-y-0.5">
```

- [ ] **Step 6: theme_toggle — 删死类 glass-surface、/90→/85**

把（L127）：
```
                <div class="theme-picker-popover glass-surface glass animate-pop-in absolute top-full right-0 mt-2 z-50 w-56
                            rounded-xl border border-border bg-surface-overlay/90 shadow-xl p-3 space-y-3"
```
替换为：
```
                <div class="theme-picker-popover glass animate-pop-in absolute top-full right-0 mt-2 z-50 w-56
                            rounded-xl border border-border bg-surface-overlay/85 shadow-xl p-3 space-y-3"
```

- [ ] **Step 7: 验证死类已绝迹、迁移完成**

Run: `cd interfaces/webchat && grep -rn "glass-surface" src/`
Expected: 无输出（死类已全部删除）。
Run: `cd interfaces/webchat && grep -rn "backdrop-blur-md" src/`
Expected: 无输出（model_picker 的裸工具类已删）。

- [ ] **Step 8: 编译检查**

Run: `cd interfaces/webchat && cargo check -p aleph-panel`
Expected: 通过。

- [ ] **Step 9: Commit**

```bash
git add interfaces/webchat/src/components/command_palette.rs \
        interfaces/webchat/src/components/directory_browser.rs \
        interfaces/webchat/src/components/notification_center.rs \
        interfaces/webchat/src/components/model_picker.rs \
        interfaces/webchat/src/components/nav_menu.rs \
        interfaces/webchat/src/components/theme_toggle.rs
git commit -m "panel(glass): unify all popover cards to the real .glass material"
```

---

### Task 6: 全屏遮罩收口到 `.aleph-scrim`

**Files:**
- Modify: `interfaces/webchat/src/components/command_palette.rs:339`
- Modify: `interfaces/webchat/src/components/directory_browser.rs:337`
- Modify: `interfaces/webchat/src/views/teams/overview.rs:417`
- Modify: `interfaces/webchat/src/components/boot_check_gate.rs:71`
- Modify: `interfaces/webchat/src/components/service_blocking_gate.rs:64`

把各处零散的 `backdrop-blur-sm` 换成 `aleph-scrim`（保留各自变暗底色）。

- [ ] **Step 1: command_palette 遮罩**

把：
```
                class="fixed inset-0 z-[60] bg-black/40 backdrop-blur-sm"
```
替换为：
```
                class="fixed inset-0 z-[60] bg-black/40 aleph-scrim"
```

- [ ] **Step 2: directory_browser 遮罩**

把：
```
                class="fixed inset-0 z-[60] bg-black/40 backdrop-blur-sm"
```
替换为：
```
                class="fixed inset-0 z-[60] bg-black/40 aleph-scrim"
```

- [ ] **Step 3: teams/overview 遮罩**

把：
```
                        class="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm p-4"
```
替换为：
```
                        class="fixed inset-0 z-50 flex items-center justify-center bg-black/40 aleph-scrim p-4"
```

- [ ] **Step 4: boot_check_gate 遮罩**

把：
```
                class="fixed inset-0 z-[9000] flex items-center justify-center bg-surface/95 backdrop-blur-sm p-4"
```
替换为：
```
                class="fixed inset-0 z-[9000] flex items-center justify-center bg-surface/95 aleph-scrim p-4"
```

- [ ] **Step 5: service_blocking_gate 遮罩**

把：
```
                class="fixed inset-0 z-[9500] flex items-center justify-center bg-surface/85 backdrop-blur-sm p-4"
```
替换为：
```
                class="fixed inset-0 z-[9500] flex items-center justify-center bg-surface/85 aleph-scrim p-4"
```

- [ ] **Step 6: 验证无残留 backdrop-blur-sm**

Run: `cd interfaces/webchat && grep -rn "backdrop-blur-sm" src/`
Expected: 无输出。
Run: `cd interfaces/webchat && grep -rc "aleph-scrim" src/ | grep -v ':0' | wc -l`
Expected: `5`（5 个文件各含 aleph-scrim）。

- [ ] **Step 7: 编译检查**

Run: `cd interfaces/webchat && cargo check -p aleph-panel`
Expected: 通过。

- [ ] **Step 8: Commit**

```bash
git add interfaces/webchat/src/components/command_palette.rs \
        interfaces/webchat/src/components/directory_browser.rs \
        interfaces/webchat/src/views/teams/overview.rs \
        interfaces/webchat/src/components/boot_check_gate.rs \
        interfaces/webchat/src/components/service_blocking_gate.rs
git commit -m "panel(glass): consolidate modal scrims onto .aleph-scrim token"
```

---

### Task 7: 内容内小模糊收口到 `.aleph-blur-subtle`

**Files:**
- Modify: `interfaces/webchat/src/components/mode_sidebar.rs:243`
- Modify: `interfaces/webchat/src/views/canvas/minimap_view.rs:62`

**故意不动** `src/views/chat/view.rs:156` 的 `backdrop-blur-[1px]`——它是拖拽文件时才出现的瞬时 drop overlay，1px 是有意的极淡蒙版，归并到 8px 会过度磨砂；保留原样（瞬时 + 开销可忽略）。

- [ ] **Step 1: mode_sidebar 吸顶头**

把：
```
            <div class="sticky top-0 z-10 bg-surface-base/95 backdrop-blur px-3 pt-3 pb-2 border-b border-border/50">
```
替换为：
```
            <div class="sticky top-0 z-10 bg-surface-base/95 aleph-blur-subtle px-3 pt-3 pb-2 border-b border-border/50">
```

- [ ] **Step 2: minimap**

把：
```
                   border border-border/50 bg-surface-raised/80 backdrop-blur"
```
替换为：
```
                   border border-border/50 bg-surface-raised/80 aleph-blur-subtle"
```

- [ ] **Step 3: 验证迁移完成、drop overlay 仍保留**

Run: `cd interfaces/webchat && grep -rn "aleph-blur-subtle" src/`
Expected: 2 行命中（mode_sidebar + minimap）。
Run: `cd interfaces/webchat && grep -rn 'backdrop-blur"' src/ | grep -v 'backdrop-blur-\['`
Expected: 无输出（默认强度的裸 `backdrop-blur` 工具类已绝迹；`backdrop-blur-[1px]` 的 drop overlay 不在此列）。
Run: `cd interfaces/webchat && grep -rn "backdrop-blur-\[1px\]" src/views/chat/view.rs`
Expected: 1 行（drop overlay 仍在，故意保留）。

- [ ] **Step 4: 编译检查**

Run: `cd interfaces/webchat && cargo check -p aleph-panel`
Expected: 通过。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/components/mode_sidebar.rs \
        interfaces/webchat/src/views/canvas/minimap_view.rs
git commit -m "panel(glass): consolidate in-content frosts onto .aleph-blur-subtle"
```

---

### Task 8: 全局审计 + WASM 构建 + 部署说明

**Files:** 无改动（验证任务）。

- [ ] **Step 1: 死类 / 散乱工具类总审计**

Run: `cd interfaces/webchat && grep -rn "glass-surface" src/ styles/; grep -rn "backdrop-blur-sm\|backdrop-blur-md" src/; grep -rn 'backdrop-blur"' src/ | grep -v 'backdrop-blur-\['`
Expected: 三条 grep 全部无输出（除 chat 的 `backdrop-blur-[1px]` 外，所有零散模糊已收口；死类已绝迹）。

- [ ] **Step 2: token 完整性审计**

Run: `cd interfaces/webchat && grep -n -- "--glass-blur:\|--glass-blur-chrome\|--glass-blur-subtle\|--scrim-blur\|--glass-saturate" styles/tailwind.css`
Expected: :root 5 个 token 各 1 行 + `html.glass` 的 glass-blur/saturate/chrome 各 1 行 + reduced-transparency 的 3 个 `!important` 归零行。

- [ ] **Step 3: panel 单测（确保未破坏编译）**

Run: `cd interfaces/webchat && cargo test -p aleph-panel --lib`
Expected: 编译通过、现有测试不回归（注意：历史上 `connection_status::loopback_detection` 可能有 1 个**预存**失败，与本改动无关，不修；其余应绿）。

- [ ] **Step 4: WASM 构建**

Run: `just wasm`
Expected: 重建 `interfaces/webchat/dist/{aleph_panel.js, aleph_panel_bg.wasm, tailwind.css, index.html}` 成功，无报错。

- [ ] **Step 5: 提交 dist 重建产物**

```bash
git add interfaces/webchat/dist/
git commit -m "panel(glass): rebuild WASM dist for glass refinement"
```

- [ ] **Step 6: 人工核验清单（部署后，DEFERRED）**

部署链：`just wasm` → `cargo build --release -p alephcore --bin aleph-server`（rust_embed 烧 dist）→ 替换运行中 binary（dev: `./target/release/aleph-server stop` 后重启；.app: 替换 `Aleph.app/Contents/MacOS/aleph-server` 后 kill pid 让 supervisor relaunch）。核验项：
  - 四档主题（System/Light/Dark/Glass）切换正常，玻璃档亮边 + 磨砂正确。
  - 6 个弹出层（命令面板 / 通知 / 目录浏览 / 模型选择 / nav 菜单 / 主题选择）观感**一致**，都透出磨砂。
  - 5 处模态遮罩变暗 + 轻模糊一致。
  - 侧栏闲时模糊在 Glass 档明显比弹出层弱（24 vs 34）。
  - 系统开启「降低透明度」后，所有模糊消失、退化为纯色。
  - 可选：chrome-devtools-mcp performance trace 对比侧栏常驻模糊 30→24px 的闲时合成开销。

---

## 隔离与合并

按用户要求，**在 git worktree 中执行**（`superpowers:using-git-worktrees`）。完成后遵循本仓既有流程合并 main：合并前 `git log <base>..main` 核验并发推进零重叠，`--no-ff` 合并，worktree + 分支用新会话清理（见 CLAUDE.md「Git Worktree 注意事项」——同会话内勿 `git worktree remove`）。是否 push 由用户决定。

## 自检结果（plan 对 spec）

- spec ① token 架构 → Task 1（+ Task 4 侧栏接线、Task 3 降级归零）。✓
- spec ② 统一 `.glass` 材质 → Task 2（+ 删死类在 Task 5）。✓
- spec ③ 表面迁移：弹出层 → Task 5；遮罩 → Task 6；内容内小模糊 → Task 7（chat drop overlay 故意排除，已在 Task 7 注明，属深思熟虑的范围收窄）。✓
- spec ④ 降级安全阀 → Task 3 Step 2。✓
- spec ⑤ 性能护栏：禁 will-change（计划全程未引入 will-change）、侧栏锁 24（Task 1+4）、遮罩 2px（Task 1+3+6）、静态光晕/颗粒不碰（无任务触及）。✓
- 类型/命名一致性：`.aleph-scrim` / `.aleph-blur-subtle` / `--glass-blur-chrome` / `--glass-blur-subtle` / `--scrim-blur` 在定义（Task 1/3）与消费（Task 4/6/7）处拼写一致。✓
- 与 spec 的有意偏差：spec ② 提到 `box-shadow: 0 20px 50px`，plan 改为沿用各弹出层现有的 `shadow-xl/2xl` 工具类以避免双重阴影冲突——记录在 Task 2 说明中。✓
