# Panel 玻璃档回归 + 明暗一致化 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 Panel 加回第 4 个主题档「玻璃」（深色受控强玻璃），把当前暗色重调为「亮色压暗」，明亮档不动。

**Architecture:** 纯 Panel 层（Leptos/WASM + CSS），原生 Bridge 零改动。玻璃强度靠新增 `--glass-blur`/`--glass-saturate` CSS 变量 + `html.glass` 作用域覆盖实现；主题选择由 `ThemeMode` 枚举驱动（`ThemeMode::ALL` 泛型渲染选择器，加一个变体即多一档）。

**Tech Stack:** Rust + `wasm-bindgen`/`web_sys`（DOM/localStorage），Leptos 组件，Tailwind/原生 CSS（design tokens）。

**Spec:** `docs/superpowers/specs/2026-06-09-panel-glass-theme-restore-design.md`

**关键约束（实现期务必遵守）:**
- 内容卡片 / 滚动区**不得**加 `backdrop-filter`（性能红线）。
- 玻璃档**不暴露原始壁纸**：`--aleph-canvas-base` 始终是受控半透明色，不得设为 `transparent`。
- 原生 `desktop/shell`、`desktop/macos` 零改动。
- 明亮档视觉与改动前逐像素一致——只新增 `--glass-*` 变量默认值（=当前硬编码值），不改明亮档任何既有 token。

**测试说明:** `appearance.rs` 的枚举逻辑是纯 host 单测（`web_sys`-free 的转换函数），用 `cargo test -p alephcore` 跑。CSS 无单测，靠 §最后的人工验收 + 构建通过。每个任务结束 commit；CSS 任务因无自动化测试，以「`cargo check` + 人工 DevTools 审查」替代红绿循环。

---

## File Structure

| 文件 | 职责 | 本计划改动 |
|---|---|---|
| `interfaces/webchat/src/appearance.rs` | 外观偏好单一真源（枚举 + 读/写/应用） | 加 `ThemeMode::Glass` + 迁移改向 + apply_mode + 单测 |
| `interfaces/webchat/styles/tailwind.css` | design tokens + 玻璃材质 + aurora | 变量化玻璃、`html.glass` token 组 + 强化、暗色 glow 重调、`:not(.glass)` 守卫、降级分支 |
| `interfaces/webchat/src/components/theme_toggle.rs` | 顶栏主题选择器 popover | 仅 doc 注释（选择器自动多一项） |
| `interfaces/webchat/src/components/command_palette.rs` | 命令面板快切 | 加回 `theme.glass` action + apply_theme glass 分支 |

任务顺序：先 Rust 枚举（Task 1–2，有单测兜底），再 CSS（Task 3–7，无单测，渐进式可视验证），最后命令面板（Task 8）。

---

## Task 1: `ThemeMode` 加 `Glass` 变体 + 迁移改向

**Files:**
- Modify: `interfaces/webchat/src/appearance.rs:31-69`（enum + `ALL` + `label` + `storage_value` + `from_storage`）
- Test: 同文件 `#[cfg(test)] mod tests`（约 L396-457）

- [ ] **Step 1: 改单测以表达新行为（先红）**

把 `appearance.rs` 末尾的 `legacy_translucent_migrates_to_dark` 测试整体替换为下面这个（改名 + 改断言为 Glass）：

```rust
    #[test]
    fn legacy_translucent_migrates_to_glass() {
        // The retired Vibrant mode persisted as "translucent". Glass is the
        // re-introduced strong-glass successor to Vibrant, so a stored
        // "translucent" must load as Glass (faithful restore of original intent).
        assert_eq!(
            ThemeMode::from_storage(Some("translucent")),
            ThemeMode::Glass
        );
    }

    #[test]
    fn glass_storage_round_trips() {
        assert_eq!(ThemeMode::Glass.storage_value(), Some("glass"));
        assert_eq!(ThemeMode::from_storage(Some("glass")), ThemeMode::Glass);
    }
```

> `mode_storage_round_trips` 与 `non_default_values_persist_a_key` 无需手改 —— 前者遍历 `ThemeMode::ALL`，加变体后自动覆盖 Glass。

- [ ] **Step 2: 跑测试确认失败（红）**

Run: `cargo test -p alephcore appearance:: 2>&1 | tail -20`
Expected: 编译失败 —— `no variant or associated item named Glass found for enum ThemeMode`。

- [ ] **Step 3: 加 `Glass` 变体到 enum + `ALL`**

`appearance.rs:31-39`，把 enum 与 `ALL` 改为：

```rust
/// Light/dark surface family. Drives the `<html>` class list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    System,
    Light,
    Dark,
    /// Dark-based, intensified-glass showcase theme (refined ex-Vibrant).
    Glass,
}

impl ThemeMode {
    pub const ALL: [ThemeMode; 4] = [Self::System, Self::Light, Self::Dark, Self::Glass];
```

- [ ] **Step 4: 补 `label` / `storage_value` / `from_storage` 三个 match 臂**

`appearance.rs:41-68`，分别补 Glass 臂、改迁移目标：

```rust
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "跟随系统",
            Self::Light => "明亮",
            Self::Dark => "暗黑",
            Self::Glass => "玻璃",
        }
    }

    /// `localStorage` value, or `None` for `System` (which clears the key).
    pub fn storage_value(self) -> Option<&'static str> {
        match self {
            Self::System => None,
            Self::Light => Some("light"),
            Self::Dark => Some("dark"),
            Self::Glass => Some("glass"),
        }
    }

    fn from_storage(raw: Option<&str>) -> Self {
        match raw {
            Some("light") => Self::Light,
            Some("dark") => Self::Dark,
            Some("glass") => Self::Glass,
            // Legacy migration: the retired Vibrant mode persisted as
            // "translucent". Glass is the re-introduced strong-glass successor,
            // so map it to Glass (faithful restore), not Dark.
            Some("translucent") => Self::Glass,
            _ => Self::System,
        }
    }
```

- [ ] **Step 5: 跑测试确认通过（绿）**

Run: `cargo test -p alephcore appearance:: 2>&1 | tail -20`
Expected: 仍会编译失败 —— `apply_mode` 的 match 未覆盖 `Glass`（`non-exhaustive patterns`）。这是预期的；Task 2 修。若想本任务独立绿，可先在 `apply_mode` 临时把 Glass 并入 `System` 臂，但更干净的做法是直接进 Task 2（同文件、同次编译）。本步只需确认**枚举三函数的单测逻辑正确**，编译报错只剩 `apply_mode`。

- [ ] **Step 6: Commit（与 Task 2 合并提交亦可）**

若 Task 2 紧接执行，跳过本步，二者一起提交。否则：
```bash
git add interfaces/webchat/src/appearance.rs
git commit -m "panel: add ThemeMode::Glass variant + retarget translucent migration"
```

---

## Task 2: `apply_mode` 支持 Glass class + 模块 doc 注释

**Files:**
- Modify: `interfaces/webchat/src/appearance.rs:325-340`（`apply_mode`）
- Modify: `interfaces/webchat/src/appearance.rs:1-14`（模块 doc 注释）

- [ ] **Step 1: 改 `apply_mode` —— 移除列表加 glass，新增 Glass 臂**

`appearance.rs:325-340`，整段替换为：

```rust
pub fn apply_mode(mode: ThemeMode) {
    if let Some(html) = root() {
        let classes = html.class_list();
        // Includes legacy "translucent" so switching away from a migrated
        // ex-Vibrant preference clears the stale class.
        let _ = classes.remove_4("dark", "light", "glass", "translucent");
        match mode {
            ThemeMode::Light => {
                let _ = classes.add_1("light");
            }
            ThemeMode::Dark => {
                let _ = classes.add_1("dark");
            }
            ThemeMode::Glass => {
                let _ = classes.add_1("glass");
            }
            ThemeMode::System => {}
        }
    }
    persist(KEY_MODE, mode.storage_value());
}
```

- [ ] **Step 2: 更新模块 doc 注释里的档位列举**

`appearance.rs` 第 6 行，把 `mode      — System / Light / Dark → ...` 改为：

```rust
//!   • mode      — System / Light / Dark / Glass    → `<html>` class list
```

- [ ] **Step 3: 跑全部 appearance 单测确认通过（绿）**

Run: `cargo test -p alephcore appearance:: 2>&1 | tail -20`
Expected: PASS（含 `mode_storage_round_trips`、`glass_storage_round_trips`、`legacy_translucent_migrates_to_glass`）。

- [ ] **Step 4: 整 crate check（确保无其它 match 因 enum 扩张而 non-exhaustive）**

Run: `cargo check -p alephcore 2>&1 | tail -25`
Expected: 通过。若报某处对 `ThemeMode` 的 match 缺 `Glass` 臂，记录文件:行，在该 match 补 `ThemeMode::Glass` 臂（行为参照 `Dark`），再重跑。

> 已知消费点：`theme_toggle.rs` 与 Appearance 设置页通过 `ThemeMode::ALL` + `label()` 泛型渲染，**不**对变体做穷举 match，故不受影响。`apply_mode`（本任务）是唯一穷举点。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/appearance.rs
git commit -m "panel: wire ThemeMode::Glass through apply_mode + module doc"
```

---

## Task 3: 玻璃模糊变量化（`--glass-blur` / `--glass-saturate`）

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css:146`（`:root` 加默认变量）
- Modify: `interfaces/webchat/styles/tailwind.css:434-438`（`.glass`）
- Modify: `interfaces/webchat/styles/tailwind.css:779-780`（`.aleph-sidebar::before`）

> 本任务是**纯重构**：把硬编码 `blur(23px) saturate(1.6)` 抽成变量，默认值 = 原值。视觉必须零变化。

- [ ] **Step 1: 在 `:root` 声明默认玻璃变量**

`tailwind.css` 的 `:root {`（约 L146）块内，任意 design-token 处加两行（建议紧邻其它 `--aleph-*` 或在块尾）：

```css
  /* Glass material strength — overridden by html.glass to intensify. */
  --glass-blur: 23px;
  --glass-saturate: 1.6;
```

- [ ] **Step 2: `.glass` 改用变量**

`tailwind.css:434-438`，把 `.glass` 规则体替换为：

```css
  .glass {
    position: relative;
    backdrop-filter: blur(var(--glass-blur)) saturate(var(--glass-saturate));
    -webkit-backdrop-filter: blur(var(--glass-blur)) saturate(var(--glass-saturate));
  }
```

- [ ] **Step 3: `.aleph-sidebar::before` 改用变量**

`tailwind.css:779-780`，把这两行：

```css
  backdrop-filter: blur(23px) saturate(1.6);
  -webkit-backdrop-filter: blur(23px) saturate(1.6);
```

替换为：

```css
  backdrop-filter: blur(var(--glass-blur)) saturate(var(--glass-saturate));
  -webkit-backdrop-filter: blur(var(--glass-blur)) saturate(var(--glass-saturate));
```

- [ ] **Step 4: 构建 WASM + CSS，确认无破坏**

Run: `just wasm 2>&1 | tail -15`
Expected: 构建成功，产出 `interfaces/webchat/dist/{aleph_panel.js, aleph_panel_bg.wasm, tailwind.css, index.html}`。

- [ ] **Step 5: 人工确认明亮/暗黑视觉零变化**

在浏览器/已重编 binary 中开 Panel，确认侧栏与弹层玻璃与改动前一致（因变量默认值 = 原硬编码值）。DevTools 检查 `.aleph-sidebar::before` 的 `backdrop-filter` 计算值仍为 `blur(23px) saturate(1.6)`。

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/styles/tailwind.css
git commit -m "panel: tokenize glass blur/saturate behind CSS variables"
```

---

## Task 4: `:not(.glass)` 守卫所有自动暗色规则

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css` 中**所有** `:root:not(.light)` 选择器（不止 @media token 块，还含一条 `:root:not(.light) .aleph-shell::after` 胶片颗粒后代选择器，约 L1397）。以 `grep -n ':root:not(\.light)'` 的输出为权威清单，逐条加 `:not(.glass)`（实测 8 条）。

> 目的：深色 OS 下 `html.glass` 会同时命中 `:root:not(.light)`（特异性 0,2,0 > `html.glass` 0,1,1），导致自动暗色 token 串味。给每个这类选择器追加 `:not(.glass)`，让玻璃档完全自治。**本任务在 Task 5/6 引入 `html.glass` token 前先行**，避免引入后被串味掩盖。

- [ ] **Step 1: 守卫 surface 基础块**

`tailwind.css:262`，`:root:not(.light) {` → `:root:not(.light):not(.glass) {`

- [ ] **Step 2: 守卫 4 个 accent 暗色映射（system dark）**

`tailwind.css` 约 L386/L395/L404/L413，把这四行的选择器各加 `:not(.glass)`：

```css
  :root:not(.light):not(.glass)[data-accent="ocean"] {
  :root:not(.light):not(.glass)[data-accent="forest"] {
  :root:not(.light):not(.glass)[data-accent="sunset"] {
  :root:not(.light):not(.glass)[data-accent="rose"] {
```

- [ ] **Step 3: 守卫 aurora atmosphere 块**

`tailwind.css:706`，`:root:not(.light) {`（aurora token 那块，紧邻 `--aleph-solid-ground`/`--aleph-canvas-base`）→ `:root:not(.light):not(.glass) {`

- [ ] **Step 4: 守卫 reduced-transparency 暗色 surface**

`tailwind.css:493-494`，把：

```css
@media (prefers-reduced-transparency: reduce) and (prefers-color-scheme: dark) {
  :root:not(.light) {
```

改为：

```css
@media (prefers-reduced-transparency: reduce) and (prefers-color-scheme: dark) {
  :root:not(.light):not(.glass) {
```

- [ ] **Step 5: 全文核对无遗漏**

Run: `grep -n ':root:not(.light)' interfaces/webchat/styles/tailwind.css`
Expected: **每一条**输出都已是 `:root:not(.light):not(.glass)`。若有裸 `:root:not(.light)`（未跟 `:not(.glass)`），补上。

- [ ] **Step 6: 构建确认无语法错误**

Run: `just wasm 2>&1 | tail -8`
Expected: 成功。

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/styles/tailwind.css
git commit -m "panel: guard auto-dark CSS rules with :not(.glass) so glass self-governs"
```

---

## Task 5: 玻璃档受控 aurora + 玻璃材质强化（`html.glass` token 组）

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css` —— 在暗色 aurora `.dark { ... }` 块（约 L694-704）之后新增 `html.glass` 块；在 `.glass::before`（L442-454）相关处之后新增 `html.glass` scoped 强化规则。

> 玻璃档 = 深色受控 aurora（基于旧 Dark 的戏剧感、glow 更强、alpha 略低，但**绝不**透原始壁纸）+ 强化玻璃材质（`--glass-blur`/`saturate` 加大 + 高光描边推亮拉长 + 景深加深）。

- [ ] **Step 0: 守卫 light-side `:root:not(.dark)` 玻璃高光泄漏（特异性前置修复）**

`html.glass` 既不带 `.light` 也不带 `.dark` → 除了 Task 4 守的 dark-side `:not(.light)` 规则，它还会命中 **light-side** `:root:not(.dark)` 规则。其中两条直接给玻璃高光/颗粒上**浅色**值，且特异性 (0,3,1) 高于本任务的 `html.glass .glass::before` (0,2,2)，不守卫则本任务的强化高光**失效**。把这两条加 `:not(.glass)`（约 L472 / L476）：

```css
:root:not(.dark):not(.glass) .glass::before {
:root:not(.dark):not(.glass) .glass::after { opacity: 0.25; }
```

守卫后玻璃档落回 base `.glass::before`/`::after`（(0,1,1)），再被本任务 `html.glass .glass::before/::after` (0,2,2) 干净覆盖。改完核对：`grep -n ':root:not(\.dark)' interfaces/webchat/styles/tailwind.css` 输出每条都带 `:not(.glass)`。

- [ ] **Step 1: 新增 `html.glass` atmosphere token 组**

在 `tailwind.css` 的 `.dark { ... }` aurora 块（约 L704 `}` 之后）插入：

```css
/* --- Atmosphere tokens — Glass (dark-based, intensified, controlled) ---
   Strong-glass showcase theme. Reuses the dramatic dark mood (stronger glow
   than the calmed Dark), with a slightly lower canvas alpha so the controlled
   aurora reads deeper — but NEVER transparent: the macOS vibrancy only faintly
   shows behind this controlled colour, raw wallpaper is never exposed. */
html.glass {
  --aleph-solid-ground: oklch(0.150 0.024 300);
  --aleph-canvas-base:  oklch(0.165 0.024 300 / 0.80);
  --aleph-glow-a:   color-mix(in oklch, var(--color-primary) 34%, transparent);
  --aleph-glow-b:   color-mix(in oklch, var(--color-primary) 22%, transparent);
  --aleph-glow-top: color-mix(in oklch, var(--color-primary) 12%, transparent);
  --aleph-glow-c:   color-mix(in oklch, oklch(0.66 0.14 250) 18%, transparent);
  --aleph-sheen:    oklch(1 0 0 / 0.07);
  /* Cards: a touch more transparent than Dark so they read glassier on aurora. */
  --color-surface-raised: oklch(0.25 0.020 300 / 0.40);
  /* Intensified glass material — pushed past the restrained default. */
  --glass-blur: 30px;
  --glass-saturate: 1.9;
}
```

> 玻璃档需要 `.dark` 那套**离散的暗色 surface/text token**（`--color-surface`、`--color-text-*` 等）才能正常显示，但它不带 `.dark` class、又被 §Task4 的 `:not(.glass)` 挡在自动暗色之外。因此**下一步**显式继承暗色基础 token。

- [ ] **Step 2: 让 `html.glass` 继承暗色基础 surface/text token**

紧接 Step 1 的块之后，加一条把玻璃档并入 `.dark` 显式 token 的选择器组。做法：找到 `.dark {` 主 token 块（**离散色** surface/text/border 那块，非 aurora 块——即 `tailwind.css:223` 起的 `.dark {`），把它的选择器从 `.dark {` 扩成 `.dark, html.glass {`。

具体：`tailwind.css:223`，`.dark {` → `.dark, html.glass {`

> 这样玻璃档拿到与暗色一致的离散 surface/text/border 色，再被 Step 1 的 `html.glass`（更高/同等且靠后）覆盖 aurora 与玻璃强度。同理，若存在 `.dark[data-accent=...]` 的**显式** accent 块（`tailwind.css:347/356/365/374`），玻璃档也应跟随 accent → 把这四个选择器从 `html.dark[data-accent="X"]` 扩成 `html.dark[data-accent="X"], html.glass[data-accent="X"]`。

- [ ] **Step 3: 新增 `html.glass` 玻璃材质强化规则（描边 + 景深）**

在 `.glass::after`（约 L464）之后、`:root:not(.dark) .glass::before`（L468）之前，插入玻璃档专属强化（描边更亮更长、噪点略增）：

```css
/* Glass theme: intensify the specular edge + grain on chrome glass. */
html.glass .glass::before {
  background: linear-gradient(180deg,
    oklch(1 0 0 / 0.55), oklch(1 0 0 / 0.08) 50%, oklch(1 0 0 / 0));
}
html.glass .glass::after { opacity: 0.5; }
```

- [ ] **Step 3b: 玻璃档继承暗色「胶片颗粒」grain（Task 4 审计补充）**

`.aleph-shell::after` 是全屏胶片噪点叠层（base `opacity: 0.025`，暗色提到 `0.05`）。其选择器经 Task 4 已变成 `.dark .aleph-shell::after, :root:not(.light):not(.glass) .aleph-shell::after { opacity: 0.05; }`（约 L1397）。玻璃档不带 `.dark`、又被 `:not(.glass)` 挡在 system-dark 外 → 否则只拿浅色 0.025 颗粒。把 `html.glass` 加进该选择器组（与 Step 2「玻璃继承暗色显式规则」同一原则）：

```css
.dark .aleph-shell::after,
html.glass .aleph-shell::after,
:root:not(.light):not(.glass) .aleph-shell::after { opacity: 0.05; }
```

- [ ] **Step 4: 玻璃档侧栏景深加深（可选但推荐，保持与 chrome 一致）**

在 `.aleph-sidebar::before`（约 L781 `}`）之后插入：

```css
/* Glass theme: deepen the sidebar's frosted layer to match the stronger chrome. */
html.glass .aleph-sidebar {
  box-shadow: inset -1px 0 0 var(--color-border-subtle),
              inset  1px 0 0 var(--aleph-sheen),
              inset  0 1px 0 oklch(1 0 0 / 0.06);
}
```

- [ ] **Step 5: 构建**

Run: `just wasm 2>&1 | tail -8`
Expected: 成功。

- [ ] **Step 6: 人工验收玻璃档**

重编 binary 并替换运行中 binary（见文末刷新链），或浏览器预览。切到玻璃档，确认：
- 深色受控 aurora 比新暗色更有戏剧感、更通透；侧栏/弹层模糊明显更强（30px/1.9）、高光描边更亮。
- **任意桌面壁纸下**不浑浊、不刺眼、不暴露原始桌面；正文 ≥ AA。
- DevTools 检查 `html.glass` 的 `--glass-blur` 计算值为 `30px`，且 `:root:not(.light):not(.glass)` 规则未应用到 `html.glass`。

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/styles/tailwind.css
git commit -m "panel: add glass theme — dark controlled aurora + intensified chrome glass"
```

---

## Task 6: 暗色重调为「亮色压暗」

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css:694-704`（`.dark` aurora 块）
- Modify: `interfaces/webchat/styles/tailwind.css:705-717`（`@media` system-dark aurora 块，与上面保持同值）

> 把暗色 aurora 的 glow drama 从当前 30/19/10%+16% 拉向亮色比例（约 24/15/8%+12%），色相对齐亮色，读作「亮色调暗」。**两处必须同值**（显式 `.dark` 与 system-dark `@media` 镜像），否则显式选暗色与跟随系统暗色观感会分裂。

- [ ] **Step 1: 重调显式 `.dark` aurora glow**

`tailwind.css:694-704` 的 `.dark { ... }` aurora 块，把 glow 四行 + sheen 改为（canvas-base 保持原值或仅微调色相到 300→295 对齐亮色；此处保留 300 不变，只调 glow）：

```css
.dark {
  --aleph-solid-ground: oklch(0.155 0.022 300);
  --aleph-canvas-base:  oklch(0.175 0.022 300 / 0.84);
  --aleph-glow-a:   color-mix(in oklch, var(--color-primary) 24%, transparent);
  --aleph-glow-b:   color-mix(in oklch, var(--color-primary) 15%, transparent);
  --aleph-glow-top: color-mix(in oklch, var(--color-primary)  8%, transparent);
  --aleph-glow-c:   color-mix(in oklch, oklch(0.64 0.10 255) 12%, transparent);
  --aleph-sheen:    oklch(1 0 0 / 0.05);
  /* Cards: semi-transparent so they read as glass-adjacent on the aurora. */
  --color-surface-raised: oklch(0.26 0.018 300 / 0.45);
}
```

> 变更点：glow-a 30→24%、glow-b 19→15%、glow-top 10→8%、glow-c 从 `oklch(0.66 0.14 250) 16%` 调淡为 `oklch(0.64 0.10 255) 12%`（降饱和、微调色相离冷蓝、降强度）。`--color-surface-raised` 不变（暗卡片本就对）。

- [ ] **Step 2: 同步 system-dark `@media` 块（必须与 Step 1 同值）**

`tailwind.css:706-716` 的 `:root:not(.light):not(.glass) {`（Task 4 已加 `:not(.glass)`）块，把同样的 glow 四行 + sheen 改成与 Step 1 完全一致：

```css
    --aleph-solid-ground: oklch(0.155 0.022 300);
    --aleph-canvas-base:  oklch(0.175 0.022 300 / 0.84);
    --aleph-glow-a:   color-mix(in oklch, var(--color-primary) 24%, transparent);
    --aleph-glow-b:   color-mix(in oklch, var(--color-primary) 15%, transparent);
    --aleph-glow-top: color-mix(in oklch, var(--color-primary)  8%, transparent);
    --aleph-glow-c:   color-mix(in oklch, oklch(0.64 0.10 255) 12%, transparent);
    --aleph-sheen:    oklch(1 0 0 / 0.05);
    --color-surface-raised: oklch(0.26 0.018 300 / 0.45);
```

- [ ] **Step 3: 构建**

Run: `just wasm 2>&1 | tail -8`
Expected: 成功。

- [ ] **Step 4: 人工验收暗色**

切暗色档，确认：glow 比改动前更平静、不冷调 drama，读作「亮色调暗」；与玻璃档并排对比，暗色明显更克制、玻璃档明显更戏剧。显式暗色 与 跟随系统暗色 观感一致（两处同值）。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/styles/tailwind.css
git commit -m "panel: retune Dark aurora to a calm darkened-Light (drama moves to Glass)"
```

---

## Task 7: `prefers-reduced-transparency` 玻璃档降级

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css:480-495`（reduced-transparency 降级块）

> 现有降级块把 `.glass`/sidebar 关模糊、canvas 退实底、surface-raised 退不透明。需补 `html.glass` 分支：`--glass-blur` 归零（统一关 chrome 模糊）、canvas/surface 退玻璃档专属实底。

- [ ] **Step 1: 在降级块内追加 `html.glass` 实色覆盖**

`tailwind.css:480-492` 的 `@media (prefers-reduced-transparency: reduce) { ... }` 块内，在 `:root { ... }`（约 L487-490）之后、`.dark { ... }`（L491）之前/之后均可，追加：

```css
  html.glass {
    --glass-blur: 0px;
    --aleph-canvas-base: var(--aleph-solid-ground) !important;
    --color-surface-raised: oklch(0.25 0.020 300) !important;
  }
```

> `--glass-blur: 0px` 让 `.glass`/`.aleph-sidebar::before` 的 `blur(var(--glass-blur))` 自动归零，与该块已有的 `backdrop-filter: none !important` 双保险。`--aleph-solid-ground` 在 `html.glass`（Task 5）已是 `oklch(0.150 0.024 300)` 实色，退给 canvas 即不透明深底。

- [ ] **Step 2: 构建**

Run: `just wasm 2>&1 | tail -8`
Expected: 成功。

- [ ] **Step 3: 人工验收降级**

macOS「系统设置 → 辅助功能 → 显示 → 降低透明度」打开后，切玻璃档，确认：无模糊、不透明深色实底、文字清晰可用。明亮/暗黑档同样降级正常（既有行为不回归）。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/styles/tailwind.css
git commit -m "panel: reduced-transparency fallback for glass theme (opaque dark)"
```

---

## Task 8: 命令面板加回玻璃快切 + theme_toggle doc 注释

**Files:**
- Modify: `interfaces/webchat/src/components/command_palette.rs:58-90`（`apply_theme` 加 glass 分支 + 移除列表）
- Modify: `interfaces/webchat/src/components/command_palette.rs:157-176`（actions 加 `theme.glass`）
- Modify: `interfaces/webchat/src/components/theme_toggle.rs:3`（doc 注释档位列举，纯注释）

- [ ] **Step 1: `apply_theme` 移除列表加 glass + 新增 glass 分支**

`command_palette.rs:69`，`remove_3("dark", "light", "translucent")` → `remove_4("dark", "light", "glass", "translucent")`。

然后在 `match mode { ... }` 的 `"dark" => { ... }` 臂（L78-83）之后、`_ =>`（L84）之前，插入：

```rust
        "glass" => {
            let _ = cls.add_1("glass");
            if let Some(s) = &storage {
                let _ = s.set_item("aleph-theme", "glass");
            }
        }
```

- [ ] **Step 2: 在 actions 列表加 `theme.glass`**

`command_palette.rs`，在 `theme.dark` Action（L164-170）之后、`theme.system`（L171）之前，插入：

```rust
        Action {
            id: "theme.glass",
            label: "Theme: Glass".to_string(),
            keywords: &["theme", "glass", "玻璃", "vibrant", "translucent"],
            group: Group::Theme,
            run: Box::new(|| apply_theme("glass")),
        },
```

- [ ] **Step 3: 更新 `theme_toggle.rs` doc 注释（纯注释）**

`theme_toggle.rs` 第 3 行，把：

```rust
//   • Mode   : System / Light / Dark (glass is the default material in all modes)
```

改为：

```rust
//   • Mode   : System / Light / Dark / Glass (Glass = dark-based intensified glass)
```

- [ ] **Step 4: 构建 + check**

Run: `cargo check -p alephcore 2>&1 | tail -12 && just wasm 2>&1 | tail -8`
Expected: 均成功。

- [ ] **Step 5: 人工验收命令面板**

打开命令面板（⌘K / Ctrl-K），搜 "glass" 或 "玻璃"，执行 → 切到玻璃档；搜 "dark"/"light"/"system" 互切，确认 `glass` class 在互切时被正确清除（不残留）。

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/components/command_palette.rs interfaces/webchat/src/components/theme_toggle.rs
git commit -m "panel: re-add glass quick-switch action + theme_toggle doc"
```

---

## 最终验收（对照 spec §12）

全部任务完成后，逐项核对：

- [ ] 选择器出现 4 档：跟随系统 / 明亮 / 暗黑 / 玻璃（顶栏 popover + Appearance 设置页均自动多一项）。
- [ ] 明亮档与改动前逐像素一致。
- [ ] 暗黑档读作「亮色压暗」，去冷调 drama。
- [ ] 玻璃档：深色受控 aurora + 明显更强模糊/高光/景深；任意壁纸下不暴露桌面、不浑浊、文字 ≥ AA。
- [ ] 深色 OS 下选玻璃档不被自动暗色规则串味（`grep ':root:not(.light)'` 全部带 `:not(.glass)`）。
- [ ] 内容滚动区无 `backdrop-filter`（DevTools 审查；本计划未给任何卡片加，天然满足）。
- [ ] 「降低透明度」下三档均降级为可用实色。
- [ ] 老 `aleph-theme=translucent` 迁移为玻璃档（单测 `legacy_translucent_migrates_to_glass` 已绿）。
- [ ] 5 套 accent 下玻璃泛光/选中态跟随 `--color-primary`（Task 5 Step 2 已让玻璃档继承 accent 块）。
- [ ] `cargo test -p alephcore appearance::` 全绿；`cargo check -p alephcore` + `just wasm` 通过。
- [ ] `desktop/shell`、`desktop/macos` 零改动（`git diff --stat` 确认不含原生路径）。

**部署刷新链**（CLAUDE.md 强制，否则运行中 daemon 看不到效果）：
```
just wasm
cargo build --release -p alephcore --bin aleph-server
# 替换运行中 binary 让 supervisor relaunch：
./target/release/aleph-server stop
cargo run --release -p alephcore --bin aleph-server start
# 或 .app daemon：mv 旧 binary → cp 新 binary → kill <pid> 让 Tauri supervisor relaunch
```
