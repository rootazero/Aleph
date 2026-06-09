# Panel 玻璃材质语言 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把玻璃从可切换的 Vibrant 主题升级为 Panel 的默认材质语言——自带受控背景层（壁纸无关、稳定、文字清晰）、C′ 克制液态玻璃只给固定 chrome、卡片用半透明 raised 面、退掉 Vibrant 档。

**Architecture:** 纯 Panel 层（CSS + design token + 退一个 enum 档），原生 Bridge 零改动。macOS 窗口本来就 `transparent(true)` + vibrancy 常驻；改动全在 `interfaces/webchat`。真玻璃（`backdrop-filter`）只给位置固定、后面是静态 aurora 的 chrome（侧栏/顶栏/popover/模态），规避滚动卡片的 GPU 重绘陷阱。

**Tech Stack:** Leptos (Rust/WASM)、Tailwind v4（`@theme` + OKLCH token）、`backdrop-filter`、`prefers-reduced-transparency`。

参考 spec: `docs/superpowers/specs/2026-06-09-panel-glass-material-design.md`

---

## 文件结构

| 文件 | 职责 | 改动类型 |
|---|---|---|
| `interfaces/webchat/src/appearance.rs` | 外观偏好单一真源 | 退 `Vibrant` 档 + `translucent→Dark` 迁移 + 单测 |
| `interfaces/webchat/src/components/command_palette.rs` | 命令面板 action 列表 + 内联 `apply_theme` | 删 vibrant action + translucent 分支 |
| `interfaces/webchat/src/components/theme_toggle.rs` | 主题选择器 popover | 仅改顶部 doc 注释 |
| `interfaces/webchat/styles/tailwind.css` | 全 panel 设计 token + 材质 | 受控 aurora、`.glass` 升级、`.aleph-sidebar` blur、卡片 token、降级、删 translucent 块 |

无组件 markup 改动：popover 已带 `glass` 类、侧栏已带 `.aleph-sidebar` 类、卡片靠 `--color-surface-raised` token。

> **CSS 数值约定**：本计划给出的 alpha / blur / 颜色是**可工作的起始值**；最终观感数值在 Task 6 的 live 预览里微调（玻璃质感本质需肉眼定）。计划锁定的是**结构**（哪些选择器、哪些属性、`::before/::after` 技法、token 改动），不是像素级数值。

---

## Task 1: appearance.rs — 退掉 Vibrant 档 + translucent→Dark 迁移

**Files:**
- Modify: `interfaces/webchat/src/appearance.rs`（enum 31-37、`ALL` 40、`label` 42-49、`storage_value` 52-59、`from_storage` 61-68、`apply_mode` 325-343、tests 399-453）

- [ ] **Step 1: 先写/改测试（RED）**

在 `interfaces/webchat/src/appearance.rs` 的 `#[cfg(test)] mod tests` 内：把 `non_default_values_persist_a_key` 里引用 Vibrant 的那行改掉，并新增迁移测试。

把这一行（约 451 行）：
```rust
        assert!(ThemeMode::Vibrant.storage_value().is_some());
```
改为：
```rust
        assert!(ThemeMode::Dark.storage_value().is_some());
```

在 `mode_system_clears_key` 测试后新增：
```rust
    #[test]
    fn legacy_translucent_migrates_to_dark() {
        // The retired Vibrant mode persisted as "translucent". Glass is now the
        // default material in every mode, so a stored "translucent" must load as Dark.
        assert_eq!(ThemeMode::from_storage(Some("translucent")), ThemeMode::Dark);
    }
```

- [ ] **Step 2: 跑测试确认失败（RED）**

Run: `cd interfaces/webchat && cargo test -p aleph-panel --lib appearance`
Expected: FAIL —— `legacy_translucent_migrates_to_dark` 断言失败（旧代码把 "translucent" 映射为 `Vibrant`，不等于 `Dark`）。

- [ ] **Step 3: 改 enum + 实现（GREEN）**

替换 `ThemeMode` 定义（31-37 行）：
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    System,
    Light,
    Dark,
}
```

替换 `impl ThemeMode` 的 `ALL`/`label`/`storage_value`/`from_storage`（40-68 行）：
```rust
    pub const ALL: [ThemeMode; 3] = [Self::System, Self::Light, Self::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "跟随系统",
            Self::Light => "明亮",
            Self::Dark => "暗黑",
        }
    }

    /// `localStorage` value, or `None` for `System` (which clears the key).
    pub fn storage_value(self) -> Option<&'static str> {
        match self {
            Self::System => None,
            Self::Light => Some("light"),
            Self::Dark => Some("dark"),
        }
    }

    fn from_storage(raw: Option<&str>) -> Self {
        match raw {
            Some("light") => Self::Light,
            Some("dark") => Self::Dark,
            // Legacy migration: the retired Vibrant mode persisted as
            // "translucent". Glass is the default material in every mode now,
            // so map it to Dark instead of silently falling back to System.
            Some("translucent") => Self::Dark,
            _ => Self::System,
        }
    }
```

替换 `apply_mode` 的 `match`（329-340 行），删掉 Vibrant 臂（保留 `remove_3` 以清理遗留 `translucent` class）：
```rust
        let _ = classes.remove_3("dark", "light", "translucent");
        match mode {
            ThemeMode::Light => {
                let _ = classes.add_1("light");
            }
            ThemeMode::Dark => {
                let _ = classes.add_1("dark");
            }
            ThemeMode::System => {}
        }
```

- [ ] **Step 4: 跑测试确认通过（GREEN）**

Run: `cd interfaces/webchat && cargo test -p aleph-panel --lib appearance`
Expected: PASS —— `mode_storage_round_trips` / `mode_system_clears_key` / `legacy_translucent_migrates_to_dark` / `non_default_values_persist_a_key` 全绿。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/appearance.rs
git commit -m "panel: retire Vibrant theme mode, migrate translucent->Dark"
```

---

## Task 2: command_palette.rs — 删 vibrant 命令 + translucent 分支

**Files:**
- Modify: `interfaces/webchat/src/components/command_palette.rs`（`apply_theme` 84-89、`theme.vibrant` action 177-183）
- Modify: `interfaces/webchat/src/components/theme_toggle.rs`（顶部 doc 注释第 3 行）

- [ ] **Step 1: 删 `apply_theme` 的 translucent 分支**

在 `command_palette.rs` 的 `apply_theme`，删除这一段（84-89 行）：
```rust
        "translucent" => {
            let _ = cls.add_2("dark", "translucent");
            if let Some(s) = &storage {
                let _ = s.set_item("aleph-theme", "translucent");
            }
        }
```
保留 68-69 行的 `let _ = cls.remove_3("dark", "light", "translucent");`（继续清理遗留 class）。

- [ ] **Step 2: 删 `theme.vibrant` action**

删除 `build_actions()` 里的整个 vibrant action（177-183 行）：
```rust
        Action {
            id: "theme.vibrant",
            label: "Theme: Vibrant".to_string(),
            keywords: &["theme", "vibrant", "translucent", "glass", "玻璃"],
            group: Group::Theme,
            run: Box::new(|| apply_theme("translucent")),
        },
```

- [ ] **Step 3: 更新 theme_toggle.rs doc 注释**

把 `theme_toggle.rs` 第 3 行：
```rust
//   • Mode   : System / Light / Dark / Vibrant (translucent glass)
```
改为：
```rust
//   • Mode   : System / Light / Dark (glass is the default material in all modes)
```

- [ ] **Step 4: 编译确认（含 clippy）**

Run: `cd interfaces/webchat && cargo clippy -p aleph-panel -- -D warnings 2>&1 | tail -20`
Expected: 编译通过、零新增警告。`theme_toggle` 的选择器由 `ThemeMode::ALL` 泛型渲染，自动只剩 3 项，无需改逻辑。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/components/command_palette.rs interfaces/webchat/src/components/theme_toggle.rs
git commit -m "panel: drop Vibrant command-palette action + picker comment"
```

---

## Task 3: CSS — 受控 aurora 背景层（壁纸无关、保留通透感）

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css`（atmosphere token 652-685、删 685 translucent override）

**目标堆叠**（macOS）：`壁纸 → vibrancy 模糊 → 我们的半透明受控 aurora（主导）→ chrome/内容`。非 macOS 浏览器：半透明 aurora 坐在不透明 ground 上，观感一致。

- [ ] **Step 1: light atmosphere token 增加 solid-ground + 半透明 canvas**

替换 light `:root` 块（653-662 行）里的 canvas-base 行，新增 `--aleph-solid-ground`：
```css
:root {
  /* Opaque ground for non-macOS (browser): the semi-opaque aurora composites
     over this so there's never see-through-to-nothing. */
  --aleph-solid-ground: oklch(0.988 0.004 295);
  /* Semi-opaque controlled aurora base. On macOS the always-on vibrancy faintly
     shows through it (保留通透感); the alpha keeps it wallpaper-independent. */
  --aleph-canvas-base: oklch(0.972 0.005 295 / 0.86);
  --aleph-glow-a:   color-mix(in oklch, var(--color-primary) 20%, transparent);
  --aleph-glow-b:   color-mix(in oklch, var(--color-primary) 13%, transparent);
  --aleph-glow-top: color-mix(in oklch, var(--color-primary)  7%, transparent);
  --aleph-glow-c:   color-mix(in oklch, oklch(0.62 0.13 250) 10%, transparent);
  --aleph-sheen:    oklch(1 0 0 / 0.72);
}
```

- [ ] **Step 2: dark atmosphere token 同样处理**

在 `.dark` 块（665-672）和 `@media (prefers-color-scheme: dark) :root:not(.light)` 块（673-682）里，都把 canvas-base 改半透明并加 ground。两处内容相同，分别替换其中的：
```css
  --aleph-canvas-base: oklch(0.155 0.022 300);
```
为：
```css
  --aleph-solid-ground: oklch(0.155 0.022 300);
  --aleph-canvas-base:  oklch(0.175 0.022 300 / 0.84);
```

- [ ] **Step 3: 删掉 translucent 的 transparent override**

删除 685 行（这是壁纸直透 bug 的根源）：
```css
/* Vibrant glass: drop the opaque base so the OS vibrancy shows through. */
html.translucent { --aleph-canvas-base: transparent; }
```

- [ ] **Step 4: body 背景按平台分流（macOS 透出 vibrancy / 浏览器坐实色）**

在 `.aleph-shell` 块（688 行）之前新增：
```css
/* macOS: let the always-on vibrancy show behind our semi-opaque aurora in every
   mode (faint, controlled 通透感). Non-macOS: paint an opaque ground so the
   semi-opaque aurora has something solid to sit on. */
html[data-platform="macos"],
html[data-platform="macos"] body { background-color: transparent; }
html:not([data-platform="macos"]) body { background-color: var(--aleph-solid-ground); }
```

- [ ] **Step 5: 构建 CSS 确认编译**

Run: `cd interfaces/webchat && npm run build:css 2>&1 | tail -5`
Expected: 无报错产出 `dist/tailwind.css`。
Run: `grep -c "aleph-solid-ground" dist/tailwind.css`
Expected: ≥ 1（token 已进产物）。

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/styles/tailwind.css
git commit -m "panel: controlled semi-opaque aurora backdrop (kill wallpaper bleed)"
```

---

## Task 4: CSS — C′ 玻璃配方（`.glass` 升级 + 侧栏 blur + 卡片 token）

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css`（`.glass` 470-473、`.aleph-sidebar` 712+、surface-raised token）

**说明**：`.glass` 已在所有 popover 上（`glass-surface glass`），fill/border/shadow 由相邻 Tailwind 工具类（`bg-surface-overlay/90 border border-border shadow-xl`）提供。`.glass` 只补工具类给不了的：blur、渐变高光描边（`::before`）、细噪点（`::after`）。避免与工具类的 `box-shadow`/`background`/`border` 打架。

- [ ] **Step 1: 升级 `.glass` 为 C′ 配方（暗色基线）**

替换 `.glass` 块（470-473 行）：
```css
.glass {
  position: relative;
  backdrop-filter: blur(23px) saturate(1.6);
  -webkit-backdrop-filter: blur(23px) saturate(1.6);
}
/* Gradient specular edge — bright at the top, fading down. Painted once,
   composited; ~0 per-frame cost. */
.glass::before {
  content: "";
  position: absolute;
  inset: 0;
  border-radius: inherit;
  padding: 1px;
  background: linear-gradient(180deg,
    oklch(1 0 0 / 0.40), oklch(1 0 0 / 0.04) 42%, oklch(1 0 0 / 0));
  -webkit-mask: linear-gradient(#000 0 0) content-box, linear-gradient(#000 0 0);
  -webkit-mask-composite: xor;
          mask-composite: exclude;
  pointer-events: none;
}
/* Fine grain — kills the "plastic" flatness of a large blur. */
.glass::after {
  content: "";
  position: absolute;
  inset: 0;
  border-radius: inherit;
  pointer-events: none;
  opacity: 0.4;
  background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='120' height='120'%3E%3Cfilter id='n'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.9' numOctaves='2'/%3E%3C/filter%3E%3Crect width='100%25' height='100%25' filter='url(%23n)' opacity='0.035'/%3E%3C/svg%3E");
}
/* The popover's own children must sit above the ::before/::after overlays. */
.glass > * { position: relative; z-index: 1; }
```

- [ ] **Step 2: light 模式下收一点高光（雅灰玻璃口味）**

紧接其后新增（light 模式描边偏白 + 底边极淡墨边，亮底不"消失"）：
```css
:root:not(.dark) .glass::before {
  background: linear-gradient(180deg,
    oklch(1 0 0 / 0.9), oklch(1 0 0 / 0.2) 45%, oklch(0.4 0.02 295 / 0.06));
}
:root:not(.dark) .glass::after { opacity: 0.25; }
```

- [ ] **Step 3: 侧栏成为真玻璃**

在 `.aleph-sidebar` 主块（712 行，`background-color: var(--color-sidebar);` 那个块内）追加 backdrop-filter。在该块的 `box-shadow:` 行之后、闭合 `}` 之前加：
```css
  backdrop-filter: blur(23px) saturate(1.6);
  -webkit-backdrop-filter: blur(23px) saturate(1.6);
```
并确保侧栏底色半透明（blur 才可见）。把该块开头的
```css
  background-color: var(--color-sidebar);
```
改为带 alpha 的受控值：
```css
  background-color: color-mix(in oklch, var(--color-sidebar) 78%, transparent);
```

- [ ] **Step 4: 卡片用半透明 raised 面（不加 blur）**

卡片是全站统一的 `bg-surface-raised`（与输入框共用，故**不能**加 blur）。只把 `--color-surface-raised` 调成半透明，让卡片坐在受控 aurora 上显得通透协调。在 light `:root`（§Task3 改过的块）和 `.dark` / dark `@media` 块里分别新增/调整 surface-raised。

light（`:root` 内，紧跟 ground/canvas 之后加一行；若已有 `--color-surface-raised` 定义于 `@theme`，此处的 `:root` 覆盖之）：
```css
  --color-surface-raised: oklch(1.00 0 0 / 0.55);
```
dark 两块各加：
```css
  --color-surface-raised: oklch(0.26 0.018 300 / 0.45);
```

> 说明：`--color-surface-raised` 原在 `@theme`（约 16 行）定义为不透明。这里在 `:root`/`.dark` 用半透明值覆盖。输入框虽也用 `bg-surface-raised`，半透明对输入框无害（坐在卡片/实色上），只是更轻盈；不引入 blur 所以无 GPU 成本。

- [ ] **Step 5: 构建 + 验证类进产物**

Run: `cd interfaces/webchat && npm run build:css 2>&1 | tail -5`
Expected: 无报错。
Run: `grep -E "\.glass::(before|after)|feTurbulence" dist/tailwind.css | head`
Expected: 命中 `.glass::before` / `.glass::after` / 噪点 SVG。

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/styles/tailwind.css
git commit -m "panel: C-prime glass recipe (.glass + sidebar blur, translucent cards)"
```

---

## Task 5: CSS — reduced-transparency 降级 + 清理遗留 translucent 块

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css`（删 431-464 的 html.translucent 块、删 1337 的 translucent after、新增降级块）

- [ ] **Step 1: 删掉遗留的 `html.translucent` 专属 CSS**

删除 431-464 行整段（`html.translucent[data-platform=macos]` 透明、`html.translucent` surface token 覆盖、browser fallback aurora、`html.translucent aside/.glass-surface` blur）。这些已被 Task 3/4 的默认行为取代。

同时删除 1337 行：
```css
html.translucent .aleph-shell::after { display: none; }
```
（`html.translucent` 选择器已无来源——`apply_mode` 不再加该 class。）

- [ ] **Step 2: 新增系统级「降低透明度」降级块**

在 `.glass` 配方之后（约原 473 行区域）新增。开启系统「降低透明度」时，玻璃退为实色、去 blur、aurora 退纯色：
```css
/* Accessibility: macOS "Reduce transparency" (and the CSS media query it maps
   to) → drop all glass to opaque solids. Zero backdrop-filter, zero see-through. */
@media (prefers-reduced-transparency: reduce) {
  .glass, .aleph-sidebar {
    backdrop-filter: none !important;
    -webkit-backdrop-filter: none !important;
  }
  .glass::before, .glass::after { display: none !important; }
  .aleph-sidebar { background-color: var(--color-sidebar) !important; }
  html[data-platform="macos"],
  html[data-platform="macos"] body { background-color: var(--aleph-solid-ground) !important; }
  :root { --aleph-canvas-base: var(--aleph-solid-ground); }
  :root { --color-surface-raised: var(--color-surface-raised); }
}
```
> 注：最后两行把半透明 token 拉回不透明 ground（canvas）；surface-raised 在该媒体下应使用不透明值——若覆盖未生效，改为显式不透明 oklch（light `oklch(1 0 0)`，dark `oklch(0.26 0.018 300)`）。Task 6 验证。

- [ ] **Step 3: 构建确认无残留 translucent 选择器**

Run: `cd interfaces/webchat && npm run build:css 2>&1 | tail -5 && grep -c "html.translucent\|\.translucent " dist/tailwind.css`
Expected: 构建无报错；grep 计数为 `0`（遗留选择器已清空）。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/styles/tailwind.css
git commit -m "panel: reduced-transparency fallback + remove dead translucent CSS"
```

---

## Task 6: 构建刷新链 + 验收（live 微调）

**Files:** 无（构建 + 人工/DevTools 验收 + 必要的数值微调回到 Task 3/4/5 文件）

- [ ] **Step 1: 全量重建 panel + 烧进 binary**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
just wasm
cargo build --release -p alephcore --bin aleph-server 2>&1 | tail -5
```
Expected: WASM 构建成功、`aleph-server` release 编译成功（`rust_embed` 烧入新 dist）。

- [ ] **Step 2: 替换运行中 binary，让 supervisor relaunch**

dev daemon：
```bash
cd /Volumes/TBU4/Workspace/Aleph
./target/release/aleph-server stop || true
cargo run --release -p alephcore --bin aleph-server start
```
（.app daemon 走 CLAUDE.md 的 mv/cp/kill relaunch 链。）

- [ ] **Step 3: 验收清单（逐项肉眼 + DevTools 核）**

- [ ] 切到一张**明亮/高饱和壁纸**，打开 Panel：chrome 玻璃稳定、不浑浊、不刺眼（壁纸只faintly可感）。
- [ ] Light / Dark 两模式：正文与次要文字对比 ≥ AA（DevTools 取色或对比插件）。
- [ ] 主题选择器只剩 跟随系统 / 明亮 / 暗黑 三项（无「玻璃」）。
- [ ] DevTools 选中一个滚动内容区卡片，确认 `backdrop-filter: none`（卡片无真玻璃）；侧栏/popover 有 `backdrop-filter`。
- [ ] 系统设置开启「显示」→「降低透明度」，重载 Panel：玻璃退为实色、可用、文字清晰。
- [ ] localStorage 手动设 `aleph-theme=translucent` 后重载：表现为 Dark，无报错（迁移生效）。
- [ ] 5 套 accent（魅紫/海蓝/森绿/暖橙/玫瑰）切换：玻璃 aurora/选中泛光跟随 `--color-primary`。
- [ ] 滚动一个长设置页观感流畅（无明显掉帧——卡片无 blur 的收益）。

- [ ] **Step 4: 按需微调数值并提交**

若某项观感不到位（玻璃太浓/太淡、文字对比不足、侧栏底色过透），回到对应 CSS 处微调 alpha/blur，重跑 Step 1-3。满意后：
```bash
git add interfaces/webchat/styles/tailwind.css interfaces/webchat/dist
git commit -m "panel: tune glass material values + rebuild dist"
```
> `dist/` 是 git-tracked（`rust_embed` 源），重建后须一并提交。

---

## Self-Review 结果

- **Spec 覆盖**：受控背景(Task3) / C′ 玻璃(Task4) / 仅 chrome+卡片半透明(Task4) / 退 Vibrant+迁移(Task1-2) / reduced-transparency(Task5) / 双模式文字对比(Task4 + 验收) / accent 跟随(验收) / 零原生改动(架构) / 构建刷新链(Task6) —— 全部有对应任务。
- **占位符**：无 TODO/TBD；每个 code step 给了完整代码。
- **类型一致**：`ThemeMode` 退档后 `ALL`(3)/`label`/`storage_value`/`from_storage`/`apply_mode` 五处同步；测试引用同步（Dark 替 Vibrant + 迁移断言）。`.glass`/`.aleph-sidebar`/`--aleph-canvas-base`/`--aleph-solid-ground`/`--color-surface-raised` 命名跨任务一致。
