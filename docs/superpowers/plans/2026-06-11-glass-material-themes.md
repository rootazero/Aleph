# Glass Material Themes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 Panel 外观扩展为「材质 × 亮暗 × 强调色」三正交旋钮——新增 液态玻璃/极光浓雾 两种材质（奢华磨砂为默认即现观感），重构 CSS token 为原色层+派生层，并让输入框悬浮于消息流之上获得可见的真模糊。

**Architecture:** Rust 侧在 `appearance.rs` 新增 `Material` 枚举（镜像 `Accent`），`ThemeMode::Glass` 退役并迁移到 液态+暗。CSS 侧把现有"每组 token 手写 4 份分支"重组为 9 块 `--mat-*` 原色（3 材质 × 亮/暗 + 3 系统镜像）+ 一份 `color-mix` 派生层。布局侧把 InputArea/SessionTabs 改为悬浮在消息滚动区之上的真模糊面。

**Tech Stack:** Leptos (WASM) + Tailwind v4 + 纯 CSS custom properties；测试为 host-side `cargo test -p aleph-panel --lib`（纯函数，禁 `web_sys` 依赖路径）。

**Spec:** `docs/superpowers/specs/2026-06-11-glass-material-themes-design.md`（含两份情绪板 HTML 于 `docs/superpowers/specs/assets/2026-06-11-glass-material/`，是材质数值的校准锚点）。

---

## 环境与红线（每个 Task 开工前必读）

**工作区**：用手动 worktree，**禁用 EnterWorktree**（本地 main 领先 origin，fresh-base 会丢提交）：

```bash
git -C /Volumes/TBU4/Workspace/Aleph worktree add /Volumes/TBU4/Workspace/aleph-glass-material -b glass-material-themes HEAD
# wasm/CSS 构建需要 node_modules（worktree 不继承）：
ln -s /Volumes/TBU4/Workspace/Aleph/interfaces/webchat/node_modules /Volumes/TBU4/Workspace/aleph-glass-material/interfaces/webchat/node_modules
```

**构建/验证命令**（worktree 内运行；共享 target-dir 有 flock，与并行任务排队属预期，勿设独立 CARGO_TARGET_DIR）：

| 目的 | 命令 |
|------|------|
| host 单测 | `cargo test -p aleph-panel --lib` |
| wasm 编译验证（每个 Rust 改动 task 必跑——native 过不代表 wasm 过） | `cargo build -p aleph-panel --lib --target wasm32-unknown-unknown` |
| CSS 编译 | `cd interfaces/webchat && npm run build:css` |
| 格式化 | `cargo fmt -p aleph-panel` |

**注意**：`just wasm` 在 worktree 内会失败（wasm-bindgen 相对 target 路径错配）——per-task 验证只用上表命令；完整 `just wasm` + 部署在合并回主仓后做（Task 11）。

**CSS 红线**：消息气泡/导航瓦片永不 `backdrop-filter`；画布零动画；常驻模糊面 ≤3（侧栏/输入框/标签条）。`.glass`（类选择器，瞬态弹层用）与 `html.glass`（退役主题选择器）是两回事——删除时只删后者。

---

### Task 0: Worktree 与基线

**Files:** 无代码改动。

- [ ] **Step 1: 建 worktree + node_modules 符号链接**（命令见上）。
- [ ] **Step 2: 基线验证**

```bash
cargo test -p aleph-panel --lib
```
Expected: 全部通过（记录用例数 N，后续任务对照）。

---

### Task 1: `Material` 枚举（appearance.rs）

**Files:**
- Modify: `interfaces/webchat/src/appearance.rs`

- [ ] **Step 1: 写失败测试**（追加到 `appearance.rs` 底部 `mod tests`）

```rust
    #[test]
    fn material_id_round_trips() {
        for m in Material::ALL {
            assert_eq!(Material::from_storage(Some(m.id())), m);
        }
        // Unknown / luxe both fall back to the default material.
        assert_eq!(Material::from_storage(None), Material::Luxe);
        assert_eq!(Material::from_storage(Some("nope")), Material::Luxe);
    }

    #[test]
    fn material_default_clears_key() {
        assert_eq!(Material::Luxe.storage_value(), None);
        assert_eq!(Material::Liquid.storage_value(), Some("liquid"));
        assert_eq!(Material::Aurora.storage_value(), Some("aurora"));
    }
```

- [ ] **Step 2: 跑测试确认失败**

```bash
cargo test -p aleph-panel --lib material
```
Expected: 编译错误 `cannot find type Material`。

- [ ] **Step 3: 实现**——在 `appearance.rs` 的 Accent 段之后（L147 附近）插入新段；KEY 常量区（L21-24）追加一行：

```rust
const KEY_MATERIAL: &str = "aleph-material";
```

```rust
// ---------------------------------------------------------------------------
// Material (glass material family)
// ---------------------------------------------------------------------------

/// Glass material family. `Luxe` is the base look (clears `data-material`);
/// `Liquid` / `Aurora` re-skin every glass surface via the `--mat-*` primitive
/// blocks keyed off `<html data-material="…">`. Orthogonal to mode + accent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Material {
    Luxe,
    Liquid,
    Aurora,
}

impl Material {
    pub const ALL: [Self; 3] = [Self::Luxe, Self::Liquid, Self::Aurora];

    /// Stable id used for both the `data-material` attribute and persistence.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Luxe => "luxe",
            Self::Liquid => "liquid",
            Self::Aurora => "aurora",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Luxe => "奢华磨砂",
            Self::Liquid => "液态玻璃",
            Self::Aurora => "极光浓雾",
        }
    }

    /// Preview swatch background (CSS) for picker chips.
    #[must_use]
    pub const fn preview(self) -> &'static str {
        match self {
            Self::Luxe => "linear-gradient(145deg, oklch(0.95 0.010 300), oklch(0.84 0.030 310))",
            Self::Liquid => {
                "linear-gradient(145deg, oklch(0.82 0.100 310 / 0.9), oklch(0.66 0.130 250 / 0.75))"
            }
            Self::Aurora => {
                "linear-gradient(135deg, oklch(0.75 0.140 350 / 0.85), oklch(0.68 0.120 280 / 0.85), oklch(0.78 0.100 200 / 0.85))"
            }
        }
    }

    /// `localStorage` value, or `None` for `Luxe` (which clears the key).
    #[must_use]
    pub const fn storage_value(self) -> Option<&'static str> {
        match self {
            Self::Luxe => None,
            Self::Liquid => Some("liquid"),
            Self::Aurora => Some("aurora"),
        }
    }

    fn from_storage(raw: Option<&str>) -> Self {
        match raw {
            Some("liquid") => Self::Liquid,
            Some("aurora") => Self::Aurora,
            _ => Self::Luxe,
        }
    }
}
```

Reads 区（`read_roundness` 之后）：

```rust
#[must_use]
pub fn read_material() -> Material {
    Material::from_storage(read_key(KEY_MATERIAL).as_deref())
}
```

Applies 区（`apply_roundness` 之后）：

```rust
pub fn apply_material(material: Material) {
    if let Some(html) = root() {
        if material == Material::Luxe {
            let _ = html.remove_attribute("data-material");
        } else {
            let _ = html.set_attribute("data-material", material.id());
        }
    }
    // Luxe is the base material → clear the key.
    persist(KEY_MATERIAL, material.storage_value());
}
```

`init_appearance()`（L399+）在 roundness 段后追加：

```rust
    let material = read_material();
    if material != Material::Luxe {
        apply_material(material);
    }
```

同时把模块头注释（L3-9）的 "Four orthogonal" 改为 "Five orthogonal" 并补一行 `//!   • material  — glass material family       → data-material attribute`。

- [ ] **Step 4: 跑测试确认通过**

```bash
cargo test -p aleph-panel --lib && cargo build -p aleph-panel --lib --target wasm32-unknown-unknown
```
Expected: 全过（N+2 个用例）。

- [ ] **Step 5: Commit**

```bash
cargo fmt -p aleph-panel && git add -A && git commit -m "panel: add Material appearance axis (luxe/liquid/aurora)"
```

---

### Task 2: `ThemeMode::Glass` 退役 + 旧值迁移

**Files:**
- Modify: `interfaces/webchat/src/appearance.rs`

- [ ] **Step 1: 改写测试**——删除 `mod tests` 中 `glass_storage_round_trips` 与 `legacy_translucent_migrates_to_glass` 两个用例，替换为：

```rust
    #[test]
    fn legacy_glass_values_load_as_dark() {
        // The retired Glass theme (and its Vibrant-era "translucent"
        // predecessor) must keep PARSING — they map to Dark; the material
        // half of the migration is decided by `legacy_glass_migration`.
        assert_eq!(ThemeMode::from_storage(Some("glass")), ThemeMode::Dark);
        assert_eq!(ThemeMode::from_storage(Some("translucent")), ThemeMode::Dark);
    }

    #[test]
    fn legacy_glass_migration_targets_liquid_dark() {
        assert_eq!(
            legacy_glass_migration(Some("glass")),
            Some(("dark", "liquid"))
        );
        assert_eq!(
            legacy_glass_migration(Some("translucent")),
            Some(("dark", "liquid"))
        );
        assert_eq!(legacy_glass_migration(Some("dark")), None);
        assert_eq!(legacy_glass_migration(None), None);
    }
```

- [ ] **Step 2: 跑测试确认失败**

```bash
cargo test -p aleph-panel --lib legacy
```
Expected: `cannot find function legacy_glass_migration`。

- [ ] **Step 3: 实现**

`ThemeMode`（L32-77）整段改为（Glass 变体删除）：

```rust
/// Light/dark surface family. Drives the `<html>` class list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    System,
    Light,
    Dark,
}

impl ThemeMode {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::System => "跟随系统",
            Self::Light => "明亮",
            Self::Dark => "暗黑",
        }
    }

    /// `localStorage` value, or `None` for `System` (which clears the key).
    #[must_use]
    pub const fn storage_value(self) -> Option<&'static str> {
        match self {
            Self::System => None,
            Self::Light => Some("light"),
            Self::Dark => Some("dark"),
        }
    }

    fn from_storage(raw: Option<&str>) -> Self {
        match raw {
            Some("light") => Self::Light,
            // Legacy values: the retired Glass theme ("glass") and its
            // Vibrant-era predecessor ("translucent") were dark-based —
            // both load as Dark. `legacy_glass_migration` (run once on
            // boot) rewrites storage to dark + liquid material.
            Some("dark" | "glass" | "translucent") => Self::Dark,
            _ => Self::System,
        }
    }
}
```

`apply_mode`（L346-364）：match 中删除 `ThemeMode::Glass` 分支；`remove_4("dark", "light", "glass", "translucent")` **保留**（继续清洗旧 DOM 类）。

迁移函数（放在 `init_appearance` 之前）：

```rust
/// Decide the legacy-Glass storage rewrite: a stored "glass"/"translucent"
/// mode becomes dark + liquid material. Pure (host-testable); returns the
/// `(aleph-theme, aleph-material)` values to write, or `None` when no
/// migration applies.
fn legacy_glass_migration(raw_mode: Option<&str>) -> Option<(&'static str, &'static str)> {
    matches!(raw_mode, Some("glass" | "translucent")).then_some(("dark", "liquid"))
}
```

`init_appearance()` 顶部（mode 读取之前）插入：

```rust
    // One-shot legacy migration: Glass-theme users land on dark + liquid.
    if let Some((mode_v, material_v)) = legacy_glass_migration(read_key(KEY_MODE).as_deref()) {
        persist(KEY_MODE, Some(mode_v));
        persist(KEY_MATERIAL, Some(material_v));
    }
```

- [ ] **Step 4: 清扫引用**——`ThemeMode::Glass` 的其他使用点会编译失败；全仓核对：

```bash
grep -rn "ThemeMode::Glass" /Volumes/TBU4/Workspace/aleph-glass-material --include="*.rs"
```
Expected: 零匹配（`theme_toggle.rs` / `settings/appearance.rs` 都只迭代 `ALL`，自动收缩为 3 项）。若有匹配逐一删除该分支。

- [ ] **Step 5: 跑测试 + wasm 编译**

```bash
cargo test -p aleph-panel --lib && cargo build -p aleph-panel --lib --target wasm32-unknown-unknown
```
Expected: 全过。

- [ ] **Step 6: Commit**

```bash
cargo fmt -p aleph-panel && git add -A && git commit -m "panel: retire ThemeMode::Glass, migrate stored glass to dark+liquid"
```

---

### Task 3: CSS 原色层（一）——消息/inset/code token 整合

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css`（L510-639 消息玻璃段）

本任务把 msg-glass 系 token 的 4 份手写分支（`:root` / `.dark` / `html.glass` / 系统暗 media）替换为：奢华磨砂原色块（亮/暗/系统镜像）+ 单份派生块。**视觉零变化**（值逐项对拷）。

- [ ] **Step 1: 替换 L522-573 的四个 token 块**为以下内容（`.msg-glass` 等组件规则 L575-639 暂不动）：

```css
/* ============================================================
   Material primitives — one flat block per material × brightness.
   Every glass surface token below derives from these via color-mix,
   so a new material = one new block, zero new derivations.
   This block: LUXE (default material). Liquid/Aurora live further
   down with the atmosphere primitives.
   ============================================================ */
:root {
  /* surface inks + fill strengths */
  --mat-ink-raise: oklch(1 0 0);
  --mat-ink-sink: oklch(0.45 0.02 295);
  --mat-fill-bubble: 65%;
  --mat-fill-inset: 6%;
  /* edges & light */
  --mat-bubble-sheen: oklch(1 0 0 / 0.35);
  --mat-edge: oklch(0.40 0.02 295 / 0.14);
  --mat-edge-top: oklch(0.40 0.02 295 / 0.20);
  --mat-edge-sunken: oklch(0.40 0.02 295 / 0.12);
  --mat-edge-sunken-top: oklch(0.40 0.02 295 / 0.16);
  /* depth */
  --mat-shadow: oklch(0.35 0.02 295 / 0.10);
  /* leaf colours (no useful derivation — keep exact) */
  --mat-code-header: oklch(0.45 0.02 295 / 0.08);
  --mat-code-pre: oklch(0.45 0.02 295 / 0.05);
  /* user bubble accent recipe */
  --mat-user-fill: 68%;
  --mat-user-glow: 25%;
}
.dark {
  --mat-ink-raise: oklch(0.24 0.02 310);
  --mat-ink-sink: oklch(0.19 0.02 310);
  --mat-fill-bubble: 75%;
  --mat-fill-inset: 70%;
  --mat-bubble-sheen: oklch(1 0 0 / 0.05);
  --mat-edge: oklch(1 0 0 / 0.10);
  --mat-edge-top: oklch(1 0 0 / 0.22);
  --mat-edge-sunken: oklch(1 0 0 / 0.07);
  --mat-edge-sunken-top: oklch(1 0 0 / 0.13);
  --mat-shadow: oklch(0 0 0 / 0.25);
  --mat-code-header: oklch(0.19 0.02 310 / 0.9);
  --mat-code-pre: oklch(0.15 0.02 310 / 0.85);
  --mat-user-fill: 68%;
  --mat-user-glow: 25%;
}
/* System mode (no explicit class) + OS dark — mirror of `.dark`. */
@media (prefers-color-scheme: dark) {
  :root:not(.light) {
    --mat-ink-raise: oklch(0.24 0.02 310);
    --mat-ink-sink: oklch(0.19 0.02 310);
    --mat-fill-bubble: 75%;
    --mat-fill-inset: 70%;
    --mat-bubble-sheen: oklch(1 0 0 / 0.05);
    --mat-edge: oklch(1 0 0 / 0.10);
    --mat-edge-top: oklch(1 0 0 / 0.22);
    --mat-edge-sunken: oklch(1 0 0 / 0.07);
    --mat-edge-sunken-top: oklch(1 0 0 / 0.13);
    --mat-shadow: oklch(0 0 0 / 0.25);
    --mat-code-header: oklch(0.19 0.02 310 / 0.9);
    --mat-code-pre: oklch(0.15 0.02 310 / 0.85);
    --mat-user-fill: 68%;
    --mat-user-glow: 25%;
  }
}

/* ── Derived surface tokens — defined ONCE; all themes/materials flow
      through the primitives above. All theme classes live on <html>, so
      single-element cascade resolution makes the var() substitution pick
      up each material/brightness block automatically. ── */
:root {
  --msg-glass-bg: color-mix(in oklch, var(--mat-ink-raise) var(--mat-fill-bubble), transparent);
  --msg-glass-sheen: linear-gradient(160deg, var(--mat-bubble-sheen), transparent 42%);
  --msg-glass-border: var(--mat-edge);
  --msg-glass-border-top: var(--mat-edge-top);
  --msg-glass-shadow: 0 4px 16px var(--mat-shadow);
  --glass-inset-bg: color-mix(in oklch, var(--mat-ink-sink) var(--mat-fill-inset), transparent);
  --glass-inset-border: var(--mat-edge-sunken);
  --glass-inset-border-top: var(--mat-edge-sunken-top);
  --code-header-bg: var(--mat-code-header);
  --code-pre-bg: var(--mat-code-pre);
}
```

注意：原 L546-558 的 `html.glass` 消息块**直接删除**（Task 2 之后运行时已无人挂 `.glass` 类；其观感由 Task 5 的液态材质接棒）。

- [ ] **Step 2: `.msg-glass-user` 改为参数化派生**——L590-597 中两行替换：

```css
    background-color: color-mix(in oklch, var(--color-primary) var(--mat-user-fill), transparent);
```
（替换原 `background-color: color-mix(in oklch, var(--color-primary) 68%, transparent);`）

```css
    box-shadow: 0 4px 14px color-mix(in oklch, var(--color-primary) var(--mat-user-glow), transparent);
```
（替换原 `box-shadow: 0 4px 14px color-mix(in oklch, var(--color-primary) 25%, transparent);`）

- [ ] **Step 3: 编译 CSS + 肉眼比对**

```bash
cd interfaces/webchat && npm run build:css
```
Expected: 编译通过；`grep -c "msg-glass-bg" styles/tailwind.css` 应为 2（一处派生定义 + 一处 `.msg-glass` 消费）。

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "panel/css: consolidate message-glass tokens into material primitive layer (luxe parity)"
```

---

### Task 4: CSS 原色层（二）——画布/氛围/模糊档 + 粒面层 + reduced-transparency 重写

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css`（L160-168 模糊档、L641-695 reduced-transparency、L908-1050 atmosphere 段）

- [ ] **Step 1: 模糊档迁入原色**——L160-168 的 `:root` 块删去 `--glass-blur/--glass-saturate/--glass-blur-chrome`（保留 `--glass-blur-subtle: 8px; --scrim-blur: 2px;` 两个全局），这三个值改到 Task 3 创建的 luxe `:root` 原色块中追加：

```css
  /* blur tiers (vary by MATERIAL only, not brightness) */
  --glass-blur: 20px;
  --glass-saturate: 1.6;
  --glass-blur-chrome: 16px;
```

- [ ] **Step 2: 氛围 token 原色化**——L915-980 的四个 atmosphere 块（`:root`/`.dark`/`html.glass`/media）整体替换为：往 Task 3 的 luxe 原色三块（`:root`/`.dark`/media 镜像）中**追加**以下原色（亮值/暗值），并新增单份派生块：

`:root` 追加：

```css
  /* canvas field */
  --mat-solid-ground: oklch(0.988 0.004 295);
  --mat-canvas-base: oklch(0.972 0.005 295 / 0.86);
  --mat-glow-a: 20%;
  --mat-glow-b: 13%;
  --mat-glow-top: 7%;
  --mat-glow-counter: color-mix(in oklch, oklch(0.62 0.13 250) 10%, transparent);
  --mat-sheen: oklch(1 0 0 / 0.72);
  --mat-raised: oklch(1.00 0 0 / 0.55);
  --mat-grain: 0.10;
  --mat-fill-chrome: 78%;
  --mat-chrome-topline: oklch(1 0 0 / 0);
  /* popover (.glass) specular edge + grain */
  --mat-pop-spec-hi: oklch(1 0 0 / 0.9);
  --mat-pop-spec-mid: oklch(1 0 0 / 0.2);
  --mat-pop-spec-lo: oklch(0.4 0.02 295 / 0.06);
  --mat-pop-grain: 0.25;
```

`.dark` 追加（media 镜像同值）：

```css
  --mat-solid-ground: oklch(0.155 0.022 300);
  --mat-canvas-base: oklch(0.175 0.022 300 / 0.84);
  --mat-glow-a: 24%;
  --mat-glow-b: 15%;
  --mat-glow-top: 8%;
  --mat-glow-counter: color-mix(in oklch, oklch(0.64 0.10 255) 12%, transparent);
  --mat-sheen: oklch(1 0 0 / 0.05);
  --mat-raised: oklch(0.26 0.018 300 / 0.45);
  --mat-grain: 0.16;
  --mat-fill-chrome: 78%;
  --mat-chrome-topline: oklch(1 0 0 / 0);
  --mat-pop-spec-hi: oklch(1 0 0 / 0.40);
  --mat-pop-spec-mid: oklch(1 0 0 / 0.04);
  --mat-pop-spec-lo: oklch(1 0 0 / 0);
  --mat-pop-grain: 0.4;
```

派生块（放在原 atmosphere 段位置）：

```css
/* --- Atmosphere tokens — derived once from the material primitives --- */
:root {
  --aleph-solid-ground: var(--mat-solid-ground);
  --aleph-canvas-base: var(--mat-canvas-base);
  --aleph-glow-a: color-mix(in oklch, var(--color-primary) var(--mat-glow-a), transparent);
  --aleph-glow-b: color-mix(in oklch, var(--color-primary) var(--mat-glow-b), transparent);
  --aleph-glow-top: color-mix(in oklch, var(--color-primary) var(--mat-glow-top), transparent);
  --aleph-glow-c: var(--mat-glow-counter);
  --aleph-sheen: var(--mat-sheen);
  --color-surface-raised: var(--mat-raised);
}
```

同时删除两处已死的直接赋值（被 atmosphere 覆盖、现在被派生取代）：基础暗色块 L235 的 `--color-surface-raised: oklch(0.20 0.018 310);` 与系统镜像 L274 的同名行。

- [ ] **Step 3: 画布粒面层**——`.aleph-shell::before`（L994-1011）之后新增：

```css
/* Static fine grain over the canvas — painted once, zero per-frame cost.
   Two jobs: (1) gives the chrome backdrop-blur visible structure to frost
   (the grain gets iced flat under sidebar/composer — the frosted-vs-clear
   differential finally shows), (2) dithers gradient banding on the aurora. */
.aleph-shell::after {
  content: "";
  position: fixed;
  inset: 0;
  z-index: -1;
  pointer-events: none;
  opacity: var(--mat-grain);
  background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='120' height='120'%3E%3Cfilter id='n'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.9' numOctaves='2'/%3E%3C/filter%3E%3Crect width='100%25' height='100%25' filter='url(%23n)' opacity='0.035'/%3E%3C/svg%3E");
}
```

- [ ] **Step 4: 侧栏与弹层消费原色**

`.aleph-sidebar`（L1023）背景行替换：

```css
  background-color: color-mix(in oklch, var(--color-sidebar) var(--mat-fill-chrome), transparent);
```

`.aleph-sidebar` 的 box-shadow（L1030-1031）替换为三段（topline 默认透明=零变化）：

```css
  box-shadow: inset -1px 0 0 var(--color-border-subtle),
              inset  1px 0 0 var(--aleph-sheen),
              inset  0 1px 0 var(--mat-chrome-topline);
```

并删除 `html.glass .aleph-sidebar` 块（L1046-1050，topline 原色已接棒）。

`.glass::before`（L456-468）的 background 替换为：

```css
  background: linear-gradient(180deg,
    var(--mat-pop-spec-hi), var(--mat-pop-spec-mid) 45%, var(--mat-pop-spec-lo));
```

`.glass::after`（L470-478）`opacity: 0.4;` 替换为 `opacity: var(--mat-pop-grain);`。
随后**删除** L482-493 的三个主题变体块（`html.glass .glass::before`、`html.glass .glass::after`、`:root:not(.dark):not(.glass)` 两条）——亮/暗差异已由原色承载。

- [ ] **Step 5: reduced-transparency 块重写**——L647-695 整块替换：

```css
@media (prefers-reduced-transparency: reduce) {
  .glass, .aleph-sidebar, .aleph-sidebar::before {
    backdrop-filter: none !important;
    -webkit-backdrop-filter: none !important;
  }
  .glass::before, .glass::after, .aleph-sidebar::before,
  .aleph-shell::after { display: none !important; }
  .aleph-scrim, .aleph-blur-subtle {
    backdrop-filter: none !important;
    -webkit-backdrop-filter: none !important;
  }
  .aleph-sidebar { background-color: var(--color-sidebar) !important; }
  /* Kill the primitives at the source: every material collapses to solid.
     `:root, :root[data-material]` matches the material blocks' specificity;
     this block stays LAST in the theme cascade so it wins. */
  :root, :root[data-material] {
    --aleph-canvas-base: var(--aleph-solid-ground) !important;
    --mat-raised: oklch(1.00 0 0) !important;
    --mat-grain: 0 !important;
    --scrim-blur: 0px !important;
    --glass-blur: 0px !important;
    --glass-blur-subtle: 0px !important;
    --glass-blur-chrome: 0px !important;
  }
  .dark, .dark[data-material] { --mat-raised: oklch(0.20 0.018 310) !important; }
  /* Message-flow faux glass → opaque solids. */
  .msg-glass {
    background-color: var(--color-surface-raised) !important;
    background-image: none !important;
    box-shadow: none !important;
  }
  .msg-glass-user {
    background-color: var(--color-primary) !important;
    background-image: none !important;
    box-shadow: none !important;
  }
  .msg-glass-danger {
    background-color: var(--color-danger-subtle) !important;
    background-image: none !important;
  }
  .glass-inset { background-color: var(--color-surface-sunken) !important; }
  .nav-tile-active {
    background-color: var(--color-sidebar-active) !important;
    box-shadow: none !important;
  }
}
@media (prefers-reduced-transparency: reduce) and (prefers-color-scheme: dark) {
  :root:not(.light), :root:not(.light)[data-material] {
    --mat-raised: oklch(0.20 0.018 310) !important;
  }
}
```

注意：原块中 `html.glass` 三行（L666-670）随重写消失。

- [ ] **Step 6: 编译 + 死引用清点**

```bash
cd interfaces/webchat && npm run build:css && grep -n "html.glass" styles/tailwind.css
```
Expected: 编译通过；`html.glass` 剩余命中仅在 L233 基础色块与 L357-432 accent 块（Task 6 处理）。

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "panel/css: atmosphere/blur/popover primitives + canvas grain + reduced-transparency over primitives"
```

---

### Task 5: 液态玻璃 + 极光浓雾 原色块

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css`（紧跟 luxe 原色块之后插入）

数值来源：`docs/superpowers/specs/assets/2026-06-11-glass-material/direction.html` 六个 skin 的换算锚点；Task 11 截图验收时允许微调（每次微调单独 commit）。

> **粒面值重校（Task 4 评审修正后）**：画布粒面 SVG 已换回全强度 tile（无内嵌 opacity），`--mat-grain` 即有效 alpha。本任务粒面值按修正后乘法链给出：liquid 0.05/0.09、aurora 0.02/0.035（强度排序 liquid > luxe 0.035/0.06 > aurora）。下方代码块中的粒面值已同步更新；不要参照 direction.html 推回旧值。

- [ ] **Step 1: 插入四个材质原色块 + 两个系统镜像**

```css
/* ============================================================
   Material: LIQUID GLASS — translucent lens surfaces, bright
   specular edges, vivid accent-derived light field.
   ============================================================ */
html[data-material="liquid"] {
  --mat-ink-raise: oklch(1 0 0);
  --mat-ink-sink: oklch(0.42 0.04 300);
  --mat-fill-bubble: 46%;
  --mat-fill-inset: 8%;
  --mat-bubble-sheen: oklch(1 0 0 / 0.55);
  --mat-edge: oklch(1 0 0 / 0.55);
  --mat-edge-top: oklch(1 0 0 / 0.92);
  --mat-edge-sunken: oklch(0.40 0.03 300 / 0.14);
  --mat-edge-sunken-top: oklch(1 0 0 / 0.55);
  --mat-shadow: oklch(0.35 0.05 300 / 0.16);
  --mat-code-header: oklch(0.45 0.03 300 / 0.10);
  --mat-code-pre: oklch(0.45 0.03 300 / 0.06);
  --mat-user-fill: 60%;
  --mat-user-glow: 40%;
  --mat-solid-ground: oklch(0.965 0.010 300);
  --mat-canvas-base: oklch(0.955 0.012 300 / 0.82);
  --mat-glow-a: 38%;
  --mat-glow-b: 26%;
  --mat-glow-top: 16%;
  --mat-glow-counter: color-mix(in oklch, oklch(0.70 0.14 250) 26%, transparent);
  --mat-sheen: oklch(1 0 0 / 0.85);
  --mat-raised: oklch(1 0 0 / 0.42);
  --mat-grain: 0.05;
  --mat-fill-chrome: 58%;
  --mat-chrome-topline: oklch(1 0 0 / 0.35);
  --mat-pop-spec-hi: oklch(1 0 0 / 0.95);
  --mat-pop-spec-mid: oklch(1 0 0 / 0.25);
  --mat-pop-spec-lo: oklch(0.4 0.03 300 / 0.06);
  --mat-pop-grain: 0.3;
  --glass-blur: 34px;
  --glass-saturate: 2.0;
  --glass-blur-chrome: 24px;
}
html[data-material="liquid"].dark {
  /* visionOS-style lens: WHITE ink at low fill over a vivid dark field. */
  --mat-ink-raise: oklch(1 0 0);
  --mat-ink-sink: oklch(1 0 0);
  --mat-fill-bubble: 8%;
  --mat-fill-inset: 5%;
  --mat-bubble-sheen: oklch(1 0 0 / 0.10);
  --mat-edge: oklch(1 0 0 / 0.13);
  --mat-edge-top: oklch(1 0 0 / 0.32);
  --mat-edge-sunken: oklch(1 0 0 / 0.08);
  --mat-edge-sunken-top: oklch(1 0 0 / 0.16);
  --mat-shadow: oklch(0 0 0 / 0.35);
  --mat-code-header: oklch(0.16 0.03 305 / 0.85);
  --mat-code-pre: oklch(0.13 0.03 305 / 0.8);
  --mat-user-fill: 48%;
  --mat-user-glow: 45%;
  --mat-solid-ground: oklch(0.140 0.030 300);
  --mat-canvas-base: oklch(0.150 0.030 300 / 0.78);
  --mat-glow-a: 44%;
  --mat-glow-b: 30%;
  --mat-glow-top: 14%;
  --mat-glow-counter: color-mix(in oklch, oklch(0.66 0.14 250) 26%, transparent);
  --mat-sheen: oklch(1 0 0 / 0.08);
  --mat-raised: oklch(0.25 0.020 300 / 0.38);
  --mat-grain: 0.09;
  --mat-fill-chrome: 55%;
  --mat-chrome-topline: oklch(1 0 0 / 0.06);
  --mat-pop-spec-hi: oklch(1 0 0 / 0.62);
  --mat-pop-spec-mid: oklch(1 0 0 / 0.10);
  --mat-pop-spec-lo: oklch(1 0 0 / 0.02);
  --mat-pop-grain: 0.5;
}

/* ============================================================
   Material: AURORA FROST — the canvas IS the show; surfaces are
   thick milky frost the colour bleeds through.
   ============================================================ */
html[data-material="aurora"] {
  --mat-ink-raise: oklch(1 0 0);
  --mat-ink-sink: oklch(0.42 0.03 330);
  --mat-fill-bubble: 58%;
  --mat-fill-inset: 7%;
  --mat-bubble-sheen: oklch(1 0 0 / 0.60);
  --mat-edge: oklch(1 0 0 / 0.70);
  --mat-edge-top: oklch(1 0 0 / 0.95);
  --mat-edge-sunken: oklch(0.40 0.03 330 / 0.13);
  --mat-edge-sunken-top: oklch(1 0 0 / 0.60);
  --mat-shadow: oklch(0.45 0.06 330 / 0.13);
  --mat-code-header: oklch(0.45 0.03 330 / 0.09);
  --mat-code-pre: oklch(0.45 0.03 330 / 0.05);
  --mat-user-fill: 62%;
  --mat-user-glow: 32%;
  --mat-solid-ground: oklch(0.965 0.015 330);
  --mat-canvas-base: oklch(0.960 0.018 330 / 0.80);
  --mat-glow-a: 48%;
  --mat-glow-b: 36%;
  --mat-glow-top: 26%;
  --mat-glow-counter: color-mix(in oklch, oklch(0.72 0.12 200) 34%, transparent);
  --mat-sheen: oklch(1 0 0 / 0.90);
  --mat-raised: oklch(1 0 0 / 0.60);
  --mat-grain: 0.02;
  --mat-fill-chrome: 72%;
  --mat-chrome-topline: oklch(1 0 0 / 0.25);
  --mat-pop-spec-hi: oklch(1 0 0 / 0.95);
  --mat-pop-spec-mid: oklch(1 0 0 / 0.30);
  --mat-pop-spec-lo: oklch(0.4 0.03 330 / 0.05);
  --mat-pop-grain: 0.18;
  --glass-blur: 26px;
  --glass-saturate: 1.5;
  --glass-blur-chrome: 20px;
}
html[data-material="aurora"].dark {
  --mat-ink-raise: oklch(0.22 0.05 315);
  --mat-ink-sink: oklch(0.17 0.04 312);
  --mat-fill-bubble: 50%;
  --mat-fill-inset: 60%;
  --mat-bubble-sheen: oklch(1 0 0 / 0.09);
  --mat-edge: oklch(1 0 0 / 0.11);
  --mat-edge-top: oklch(1 0 0 / 0.27);
  --mat-edge-sunken: oklch(1 0 0 / 0.07);
  --mat-edge-sunken-top: oklch(1 0 0 / 0.14);
  --mat-shadow: oklch(0 0 0 / 0.38);
  --mat-code-header: oklch(0.15 0.04 312 / 0.9);
  --mat-code-pre: oklch(0.12 0.04 312 / 0.85);
  --mat-user-fill: 55%;
  --mat-user-glow: 38%;
  --mat-solid-ground: oklch(0.160 0.040 310);
  --mat-canvas-base: oklch(0.170 0.045 310 / 0.76);
  --mat-glow-a: 40%;
  --mat-glow-b: 32%;
  --mat-glow-top: 18%;
  --mat-glow-counter: color-mix(in oklch, oklch(0.62 0.13 230) 30%, transparent);
  --mat-sheen: oklch(1 0 0 / 0.06);
  --mat-raised: oklch(0.24 0.030 310 / 0.50);
  --mat-grain: 0.035;
  --mat-fill-chrome: 64%;
  --mat-chrome-topline: oklch(1 0 0 / 0.05);
  --mat-pop-spec-hi: oklch(1 0 0 / 0.50);
  --mat-pop-spec-mid: oklch(1 0 0 / 0.08);
  --mat-pop-spec-lo: oklch(1 0 0 / 0.01);
  --mat-pop-grain: 0.35;
}

/* System mode + OS dark — mirrors of the two `.dark` material blocks. */
@media (prefers-color-scheme: dark) {
  :root:not(.light)[data-material="liquid"] {
    /* (verbatim copy of html[data-material="liquid"].dark body) */
  }
  :root:not(.light)[data-material="aurora"] {
    /* (verbatim copy of html[data-material="aurora"].dark body) */
  }
}
```

实施时把两个 media 镜像的注释替换为对应 `.dark` 块的**逐行原文**（与 luxe 镜像同样的纪律：镜像块只许复制，不许独立调值）。

- [ ] **Step 2: 编译 + 选择器自检**

```bash
cd interfaces/webchat && npm run build:css
grep -c "data-material=\"liquid\"" styles/tailwind.css   # expected: 3 (light + dark + media mirror)
grep -c "data-material=\"aurora\"" styles/tailwind.css   # expected: 3
```

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "panel/css: liquid-glass + aurora-frost material primitive blocks"
```

---

### Task 6: `html.glass` 残余清扫

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css`（L233、L357-432）

- [ ] **Step 1: 基础色块**——L233 `.dark, html.glass {` 改为 `.dark {`。

- [ ] **Step 2: accent 暗色块**——L357-392 四处 `html.dark[data-accent="…"], html.glass[data-accent="…"]` 选择器删去 `, html.glass[…]` 半边（保留 `html.dark[…]`）。

- [ ] **Step 3: 全仓清点**

```bash
grep -rn "html\.glass\|\"glass\"\|'glass'" /Volumes/TBU4/Workspace/aleph-glass-material/interfaces/webchat/styles/tailwind.css
grep -rn "html\.glass" /Volumes/TBU4/Workspace/aleph-glass-material/interfaces/webchat/src --include="*.rs"
```
Expected: CSS 零命中 `html.glass`；Rust 侧仅 `appearance.rs` 的迁移字符串（`"glass"`/`"translucent"`，合法保留）与 `apply_mode` 的 DOM 清洗 `remove_4`。`theme-picker-popover` 等处的 `.glass` **类**（瞬态弹层）属正常使用，不动。

- [ ] **Step 4: 编译 + Commit**

```bash
cd interfaces/webchat && npm run build:css
git add -A && git commit -m "panel/css: drop retired html.glass theme selectors"
```

---

### Task 7: 外观设置页「材质」行

**Files:**
- Modify: `interfaces/webchat/src/views/settings/appearance.rs`

- [ ] **Step 1: 接线**——imports 增加 `apply_material, read_material, Material`；组件内（L21 后）增加 `let material = RwSignal::new(read_material());`；`reset` 闭包增加：

```rust
        apply_material(Material::Luxe);
        material.set(Material::Luxe);
```

- [ ] **Step 2: 新增 SettingCard**——插在「主题模式」卡与「强调色」卡之间：

```rust
                // --- Material --------------------------------------------------
                <SettingCard title="材质" desc="界面的玻璃材质风格：奢华磨砂克制内敛，液态玻璃通透鲜活，极光浓雾色彩弥漫。">
                    <div class="flex flex-wrap gap-4">
                        {Material::ALL.into_iter().map(|m| {
                            let active = move || material.get() == m;
                            view! {
                                <button
                                    on:click=move |_| { apply_material(m); material.set(m); }
                                    title=m.label()
                                    class="flex flex-col items-center gap-1.5 group"
                                >
                                    <span
                                        class=move || {
                                            let base = "w-14 h-9 rounded-lg transition-transform group-hover:scale-105";
                                            if active() {
                                                format!("{base} ring-2 ring-offset-2 ring-offset-surface-raised ring-text-primary")
                                            } else {
                                                format!("{base} ring-1 ring-border")
                                            }
                                        }
                                        style=format!("background: {}", m.preview())
                                    />
                                    <span class="text-xs text-text-secondary">{m.label()}</span>
                                </button>
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                </SettingCard>
```

同时把「主题模式」卡的 desc 从 `"界面明暗与玻璃质感。…"` 改为 `"界面明暗。「跟随系统」交由操作系统决定。"`（玻璃语义已移交材质卡）。

- [ ] **Step 3: 验证 + Commit**

```bash
cargo build -p aleph-panel --lib --target wasm32-unknown-unknown && cargo test -p aleph-panel --lib
cargo fmt -p aleph-panel && git add -A && git commit -m "panel: material picker row in Appearance settings"
```

---

### Task 8: ThemeToggle 弹层「材质」行

**Files:**
- Modify: `interfaces/webchat/src/components/theme_toggle.rs`

- [ ] **Step 1: 接线**——import 行（L11）扩为：

```rust
use crate::appearance::{
    apply_accent, apply_material, apply_mode, read_accent, read_material, read_mode, Accent,
    Material, ThemeMode,
};
```

组件内（L91 后）增加 `let material = RwSignal::new(read_material());`。

- [ ] **Step 2: 弹层新增材质行**——插在 Mode div 与 Accent div 之间（复用 `animated_apply` 获得圆形揭幕）：

```rust
                    // Material
                    <div>
                        <p class="px-1 pb-1.5 text-[10px] font-semibold uppercase tracking-wider text-text-tertiary">
                            "材质"
                        </p>
                        <div class="grid grid-cols-3 gap-1">
                            {Material::ALL
                                .into_iter()
                                .map(|m| {
                                    let is_active = move || material.get() == m;
                                    view! {
                                        <button
                                            on:click=move |ev: web_sys::MouseEvent| {
                                                let x = ev.client_x() as f64;
                                                let y = ev.client_y() as f64;
                                                animated_apply(x, y, move || apply_material(m));
                                                material.set(m);
                                            }
                                            class=move || {
                                                let base = "px-1 py-1.5 rounded-lg text-xs font-medium transition-colors";
                                                if is_active() {
                                                    format!("{base} bg-primary text-white")
                                                } else {
                                                    format!("{base} text-text-secondary hover:bg-surface-sunken")
                                                }
                                            }
                                        >
                                            {m.label()}
                                        </button>
                                    }
                                })
                                .collect::<Vec<_>>()}
                        </div>
                    </div>
```

- [ ] **Step 3: 验证 + Commit**

```bash
cargo build -p aleph-panel --lib --target wasm32-unknown-unknown && cargo test -p aleph-panel --lib
cargo fmt -p aleph-panel && git add -A && git commit -m "panel: material row in ThemeToggle popover"
```

---

### Task 9: 悬浮输入框——布局结构与滚动让位

**Files:**
- Modify: `interfaces/webchat/src/views/chat/view.rs:144-148`
- Modify: `interfaces/webchat/src/views/chat/messages.rs:163-168, 224`
- Modify: `interfaces/webchat/src/views/chat/composer/mod.rs:479-481`

- [ ] **Step 1: view.rs 重叠结构**——L144-148 的三段式替换为：

```rust
                // Session tab strip — renders only when ≥2 agents are open.
                <SessionTabs />
                // Overlap container: the scroll area extends to the bottom,
                // the composer FLOATS on top of it (real backdrop blur —
                // messages frost as they flow behind it).
                <div class="relative flex-1 min-h-0">
                    // Message list (scrollable) — or the welcome hero when empty
                    <MessageList />
                    // Input area (floating glass bar pinned over the flow)
                    <InputArea />
                </div>
```

（SessionTabs 的覆盖式处理在 Task 10；本任务先保持其在流内。）

- [ ] **Step 2: messages.rs 适配**——根元素 L163 `class="relative flex-1 min-h-0"` 改为 `class="relative h-full"`（父级不再是 flex 容器）。内容容器 L168 改为：

```rust
                        <div class="max-w-3xl mx-auto px-4 pt-6 pb-[calc(var(--composer-clearance,150px)+1rem)] space-y-3">
```

「新消息」pill L224 的 `bottom-3` 改为 `bottom-[calc(var(--composer-clearance,150px)+0.5rem)]`。

- [ ] **Step 3: composer/mod.rs 悬浮容器 + 高度上报**——view! 根（L480-481）替换为：

```rust
        <div class="absolute inset-x-0 bottom-0 z-10 px-4 pb-4 pt-2 pointer-events-none">
            <div class="max-w-3xl mx-auto pointer-events-auto" node_ref=stack_ref>
```

（两处闭合 div 不变；`pointer-events-none` 让左右空槽的点击穿透到消息，内层恢复 auto。）

组件顶部声明 ref 与上报副作用（与既有 imports 对齐：`wasm_bindgen::closure::Closure`、`wasm_bindgen::JsCast`）：

```rust
    let stack_ref = NodeRef::<leptos::html::Div>::new();
    // Composer height → `--composer-clearance` on <html>, so the scroll
    // content + jump pill always clear the floating bar (queue bar /
    // attachments / multiline growth included). Mirrors the ResizeObserver
    // pattern in views/canvas/graph_canvas.rs; the chat view is kept alive
    // by MainContent, so the leaked closure is one-per-app, not per-visit.
    Effect::new(move |_| {
        let Some(el) = stack_ref.get() else { return };
        let cb: Closure<dyn FnMut(js_sys::Array)> = Closure::new(move |entries: js_sys::Array| {
            if let Ok(entry) = entries.get(0).dyn_into::<web_sys::ResizeObserverEntry>() {
                let h = entry.content_rect().height();
                if let Some(root) = web_sys::window()
                    .and_then(|w| w.document())
                    .and_then(|d| d.document_element())
                    .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
                {
                    let _ = root
                        .style()
                        .set_property("--composer-clearance", &format!("{}px", h + 40.0));
                }
            }
        });
        if let Ok(observer) = web_sys::ResizeObserver::new(cb.as_ref().unchecked_ref()) {
            observer.observe(&el);
        }
        cb.forget();
    });
```

注意：`Effect::new` 读 `stack_ref` 会在 ref 挂载后重跑一次；`cb.forget()` 在首跑（ref 为 None）提前 return 不会执行，观察器只建一次。若编译器提示 `js_sys` 未引入，按文件现状补 `use js_sys;` 或全路径。

- [ ] **Step 4: 验证**

```bash
cargo build -p aleph-panel --lib --target wasm32-unknown-unknown && cargo test -p aleph-panel --lib
```
Expected: 编译通过、用例数不变。

- [ ] **Step 5: Commit**

```bash
cargo fmt -p aleph-panel && git add -A && git commit -m "panel/chat: float the composer over the message flow with clearance tracking"
```

---

### Task 10: 玻璃 chrome 面——输入框真模糊 + 标签条覆盖 + 顶部渐隐

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css`（`.aleph-composer` L1098-1107、reduced-transparency 块）
- Modify: `interfaces/webchat/src/views/chat/view.rs`（SessionTabs 覆盖）
- Modify: `interfaces/webchat/src/views/chat/messages.rs`（顶部渐隐 + 标签让位）
- Modify: `interfaces/webchat/src/components/session_tabs.rs:37-38`

- [ ] **Step 1: 输入框真模糊**——`.aleph-composer`（L1098 起）追加两行（背景/边框/阴影全部不动，半透明 `--mat-raised` 已就位）：

```css
  backdrop-filter: blur(var(--glass-blur-chrome)) saturate(var(--glass-saturate));
  -webkit-backdrop-filter: blur(var(--glass-blur-chrome)) saturate(var(--glass-saturate));
```

- [ ] **Step 2: SessionTabs 覆盖式 + 真模糊**——view.rs 中 Task 9 的结构再调整：`<SessionTabs />` 移入重叠容器顶端：

```rust
                <div class="relative flex-1 min-h-0">
                    <MessageList />
                    <div class="absolute inset-x-0 top-0 z-10"><SessionTabs /></div>
                    <InputArea />
                </div>
```

session_tabs.rs L37-38 的类替换：

```rust
            <div class="aleph-session-tabs flex items-center gap-1 px-2 py-1
                        text-xs overflow-x-auto flex-shrink-0">
```

tailwind.css 新增（`.aleph-composer` 段后）：

```css
/* --- Session tab strip — frosted chrome band over the message flow --- */
.aleph-session-tabs {
  background-color: color-mix(in oklch, var(--color-surface) 55%, transparent);
  backdrop-filter: blur(var(--glass-blur-chrome)) saturate(var(--glass-saturate));
  -webkit-backdrop-filter: blur(var(--glass-blur-chrome)) saturate(var(--glass-saturate));
  border-bottom: 1px solid var(--color-border-subtle);
  box-shadow: inset 0 1px 0 var(--mat-chrome-topline);
}
```

- [ ] **Step 3: 滚动内容标签让位 + 顶部渐隐**——messages.rs：组件顶部取 `let sessions = expect_context::<crate::state::sessions::SessionMap>();`（确认该路径与 session_tabs.rs 的 import 一致），内容容器 class 改为响应式：

```rust
                        <div class=move || format!(
                            "max-w-3xl mx-auto px-4 {} pb-[calc(var(--composer-clearance,150px)+1rem)] space-y-3",
                            if sessions.tab_order.with(|o| o.len() >= 2) { "pt-14" } else { "pt-6" }
                        )>
```

滚动容器 L164 加渐隐类：`class="absolute inset-0 overflow-y-auto chat-scroll-fade"`，CSS 新增：

```css
/* Top fade — content dissolves under the chrome band instead of clipping.
   Static mask on the scroll container; verify scroll perf in WKWebView
   (spec §6.2: if it costs, drop the fade, keep the float). */
.chat-scroll-fade {
  mask-image: linear-gradient(180deg, transparent 0, #000 26px);
  -webkit-mask-image: linear-gradient(180deg, transparent 0, #000 26px);
}
```

- [ ] **Step 4: reduced-transparency 补两面**——Task 4 重写的块内、`.aleph-sidebar` 行后追加：

```css
  .aleph-composer, .aleph-session-tabs {
    backdrop-filter: none !important;
    -webkit-backdrop-filter: none !important;
  }
  .aleph-session-tabs { background-color: var(--color-surface) !important; }
  .chat-scroll-fade { mask-image: none !important; -webkit-mask-image: none !important; }
```

- [ ] **Step 5: 验证 + Commit**

```bash
cd interfaces/webchat && npm run build:css && cd ../..
cargo build -p aleph-panel --lib --target wasm32-unknown-unknown && cargo test -p aleph-panel --lib
cargo fmt -p aleph-panel && git add -A && git commit -m "panel/chat: frosted composer + overlay session tabs + top scroll fade"
```

---

### Task 11: 集成验收（controller 执行，非 subagent）

**Files:** 无新代码（仅允许材质数值微调 commit）。

- [ ] **Step 1: 全量构建**——合并回主仓前在 worktree 跑 `cargo clippy -p aleph-panel --lib --target wasm32-unknown-unknown -- -D warnings`；合并入 main 后在**主仓**跑 `just wasm` + `cargo build --release -p alephcore --bin aleph-server`。
- [ ] **Step 2: 截图矩阵**——standalone HTML 直载 `interfaces/webchat/dist/tailwind.css`（沿用上轮验收法，scratch 文件放 target/ 不入库），chrome-devtools 切换 `<html>` 的 class/attr 组合截图：3 材质 × {亮, 暗} 6 张基准 + 森林×极光 + 日落×极光 + 液态·暗悬浮输入框滚动中态（消息半压输入框）。对照验收点：默认材质与改造前截图无可感知漂移；液态·亮气泡正文对比度 ≥4.5:1（DevTools 拾色器验证，不达标上调 `--mat-fill-bubble` 并单独 commit）。
- [ ] **Step 3: 迁移验证**——部署后浏览器 localStorage 预置 `aleph-theme=glass` → 刷新 → 呈现 液态+暗，storage 变为 `aleph-theme=dark` + `aleph-material=liquid`。
- [ ] **Step 4: 性能基线**——Activity Monitor 对比改造前后空闲 CPU/GPU（参照 bridge 瘦身轮方法）；滚动目测无掉帧；`prefers-reduced-transparency` 模拟下全部坍缩为实色。
- [ ] **Step 5: 部署链**——`mv /Applications/Aleph.app/Contents/MacOS/aleph-server{,.bak}` → `cp target/release/aleph-server /Applications/Aleph.app/Contents/MacOS/` → `kill <pid>`（supervisor 自动 relaunch），`pgrep aleph-server` + `curl -s localhost:18790` 复验。

---

## Self-Review 记录

- **Spec 覆盖**：§3 主题模型→Task 1/2/7/8；§4 token 架构→Task 3/4/5；§5 材质配方+粒面→Task 4/5；§5.3 对比度→Task 11.2；§6 悬浮输入框→Task 9/10；§7 设置 UI→Task 7/8；§8 资源红线→Task 10 限于 2 个新模糊面+静态粒面；§9 回退→Task 4.5/10.4；§10 验收→Task 11；nav-tile 在 §4.2 派生清单中——其既有配方已从 `--color-sidebar-accent` 派生、自动跟随材质，本轮无需改动（刻意 no-op）。
- **占位符**：Task 5 media 镜像块用"逐行复制 `.dark` 块"指令替代重复粘贴——这是明确的机械操作而非 TBD。
- **类型一致性**：`Material`/`apply_material`/`read_material`/`KEY_MATERIAL`/`legacy_glass_migration` 命名在 Task 1/2/7/8 间一致；`--mat-*` 原色家族在 Task 3/4/5/10 间一致（ink-raise/ink-sink/fill-bubble/fill-inset/fill-chrome/bubble-sheen/edge/edge-top/edge-sunken/edge-sunken-top/shadow/code-header/code-pre/user-fill/user-glow/solid-ground/canvas-base/glow-a/b/top/counter/sheen/raised/grain/chrome-topline/pop-spec-hi/mid/lo/pop-grain）。
