# Panel 玻璃档回归 + 明暗一致化 — 设计方案

> 日期: 2026-06-09 · 范围: Panel (Leptos/WASM) · 状态: 设计已确认，待写实现计划
> 前序: [2026-06-09-panel-glass-material-design.md](./2026-06-09-panel-glass-material-design.md)

## 1. 背景

前一版 spec（玻璃材质语言）把玻璃从「可选主题」升级为**默认材质**：删掉了单独的 Vibrant（`translucent`）档，让 `System / Light / Dark` 三档都默认带克制玻璃（C′ 配方），并用受控 aurora 解决了壁纸劫持问题。

现状（已落地）：

- `ThemeMode = System / Light / Dark`，Vibrant 已删，`from_storage("translucent") → Dark` 迁移。
- **明亮**：近白受控 aurora `oklch(0.972 0.005 295 / 0.86)` + 半透明白卡片 + 克制玻璃 chrome（`blur(23px) saturate(1.6)`）。
- **暗黑**：冷调深空 aurora `oklch(0.175 0.022 300 / 0.84)`（mauve+蓝 glow，强度 30/19/10%+16%）+ 半透明暗卡片，同一套克制玻璃。
- `.glass` 工具类只给固定 chrome（侧栏/顶栏/弹层/模态）；内容卡片**不上** `backdrop-filter`（滚动逐帧重算 = GPU 陷阱）。

## 2. 需求

加回一个**玻璃档**作为第 4 个并列选项，给用户更多质感选择：

1. **明亮**：保持现状，完全不动。
2. **暗黑**：从「冷调深空」改为「亮色的暗色化」——沿用亮色的色相 / glow 配方 / 玻璃质感，只把表面压暗，读起来是「同一个房间把灯调暗」。
3. **玻璃**（新增第 4 档）：**强化版玻璃**——更强模糊/饱和、更明显的高光描边与内发光、更深景深。

选择器：`System / Light / Dark` → `System / Light / Dark / Glass`。

## 3. 核心叙事：把当前「暗色」拆成两档

当前的暗色同时背了两个身份：既是「暗色模式」，又是「冷调深空 + 强玻璃的戏剧感」。本方案把这两个身份**拆开**：

- **新暗色** = 亮色压暗（平静、与亮色同族、去掉冷调 drama）。
- **新玻璃档** = 接住那份「深色 + 强玻璃戏剧感」，并进一步推满。

→ 「更多选择」是真的多了一种**质感**，而非只多一个开关。玻璃档本质是**精致化的旧 Vibrant**。

## 4. 决策记录（已与用户逐项确认）

| 维度 | 决定 |
|---|---|
| 玻璃档底色基调 | **深色强玻璃**（类旧 Vibrant，精致化） |
| 「强化」方向 | **受控强化**：保留自家受控 aurora，**不放真实壁纸**透入；只把玻璃材质本身推满 |
| 强化覆盖范围 | **仍限 chrome**：侧栏/顶栏/弹层/模态拿强化 backdrop-filter；内容卡片仍不上真模糊（保上一版性能红线） |
| 暗色定位 | **亮色压暗**：同色相/同 glow 配方，调淡 drama；玻璃材质质感与亮色同族 |

### 红线定位

纯 Panel 层改造，**原生 Bridge 零改动**（macOS 窗口本就永远 `transparent(true)`、vibrancy 本就无条件挂上）。玻璃档全靠 CSS / design token / `ThemeMode` 枚举实现 → 符合 **R1**（大脑-四肢分离）、**R2**（UI 唯一源在 Leptos）。

## 5. 三档定位总表

| 档位 | 底色 | 玻璃强度 | 改动 |
|---|---|---|---|
| 明亮 Light | 近白受控 aurora | 克制 | **完全不动** |
| 暗黑 Dark | 亮色压暗（同色相/同 glow 配方，调淡 drama） | 克制 | aurora glow 重调 |
| 玻璃 Glass（新增） | 深色受控 aurora（不放壁纸） | **强化** | 全新 `html.glass` token 组 + 强化规则 |

## 6. 模块 1：主题枚举（`appearance.rs`）

- `ThemeMode`：加 `Glass` 变体。`ALL: [ThemeMode; 4]`，`label() = "玻璃"`，`storage_value() = Some("glass")`。
- `from_storage`：
  - `"glass" → Glass`。
  - **遗留迁移改向**：`"translucent"`（旧 Vibrant 持久值）从「→ Dark」改为 **「→ Glass」**——玻璃档就是精致化的旧 Vibrant，老用户落到玻璃档才是忠实还原。
- `apply_mode`：移除类名列表扩成 `remove_4("dark", "light", "glass", "translucent")`（含遗留 `translucent` 清理）；Glass 档 `add_1("glass")`。
- 模块顶部 doc 注释把 `System / Light / Dark` 更新为含 Glass。
- 单测：
  - `mode_storage_round_trips` 自动覆盖 Glass（经 `ALL`）。
  - `legacy_translucent_migrates_to_dark` → 改名 `legacy_translucent_migrates_to_glass`，断言 `from_storage(Some("translucent")) == Glass`。
  - `non_default_values_persist_a_key` 可补 `Glass.storage_value().is_some()`。

## 7. 模块 2：玻璃材质变量化（`tailwind.css`）

当前 `.glass`（约 L436）与 `.aleph-sidebar::before`（约 L779）把 `blur(23px) saturate(1.6)` 硬编码。改为读 CSS 变量，让玻璃档「调一处、全 chrome 跟随」：

- `:root` 新增 `--glass-blur: 23px` / `--glass-saturate: 1.6`（默认 = 当前克制值）。
- `.glass` 与 `.aleph-sidebar::before` 的 `backdrop-filter` / `-webkit-backdrop-filter` 改用 `blur(var(--glass-blur)) saturate(var(--glass-saturate))`。
- **`html.glass` 作用域**：
  - `--glass-blur: 30px`、`--glass-saturate: 1.9`。
  - scoped 规则把 `.glass::before` 高光描边推亮拉长（顶部更亮、falloff 更长）、内顶高光 inset 与分层景深阴影加深。
- 行为不变保证：`prefers-reduced-transparency` 降级块把 `--glass-blur` 归零即可统一关掉所有 chrome 模糊（见模块 4）。

## 8. 模块 3：玻璃档受控 aurora + 暗色重调

### 玻璃档（`html.glass`）token 组

自带一套**深色受控 aurora**：

- 基于旧 Dark 的戏剧感（glow 比新 Dark 更强），alpha 略低让自家 aurora 更通透——**但绝不暴露原始壁纸**（canvas-base 仍是受控半透明色，macOS vibrancy 只在其后 faint 透出）。
- `--aleph-canvas-base` / `--aleph-glow-*` / `--aleph-sheen` / `--color-surface-raised` 全量给齐（避免靠 @media 串味，见模块 4）。

### 暗色（新 Dark）token 重调

把 `.dark` 与对应 `@media` 块的 aurora 调成「亮色压暗」：

- glow 强度从当前 30/19/10% + 16% 拉向亮色比例（约 24/15/8% + 12%），保留呼吸但去冷调 drama。
- 色相对齐亮色（295–300）。
- surface 维持暗值，整体读作「亮色调暗」。

> 具体数值在实现 + 预览阶段微调；本节锁定**方向与配方一致性**，不锁死像素级 oklch 值。

## 9. 模块 4：特异性与降级（关键正确性）

### `:not(.glass)` 守卫

`html.glass` 在深色 OS 下会同时满足 `@media (prefers-color-scheme: dark)` 里的 `:root:not(.light)` 选择器，而后者特异性（0,2,0）高于 `html.glass`（0,1,1）→ 自动暗色 token 会**串味**覆盖玻璃档。修复：给所有这类规则补 `:not(.glass)`，让玻璃档完全自治。涉及约 6 处：

- aurora atmosphere（约 L705 `:root:not(.light)`）
- 5 套 accent 暗色映射（约 L386/395/404/413）
- surface 基础块（约 L262）
- reduced-transparency 暗色 surface（约 L493/494）

（`.dark` 的**显式** class 规则不影响玻璃档，因为 `html.glass` 不带 `.dark`；只有 @media 自动暗色规则需要守卫。）

### `prefers-reduced-transparency` 降级

现有降级块（约 L480）加 `html.glass` 分支：

- `--glass-blur: 0` → 关掉强化模糊（统一机制）。
- `html.glass` 的 `--aleph-canvas-base → 玻璃实底`、`--color-surface-raised → 不透明深色`。
- 系统级可达性照旧零用户配置尊重。

## 10. 模块 5：命令面板 & 选择器

- `theme_toggle.rs`：选择器由 `ThemeMode::ALL` 泛型渲染 → 自动多出「玻璃」项，**仅改顶部 doc 注释**，无逻辑改动。
- `command_palette.rs`：加回 `theme.glass` 快切 action，关键词 `["theme", "glass", "玻璃", "vibrant", "translucent"]`；apply 时走 `apply_mode(ThemeMode::Glass)`。

## 11. 改动文件清单（全部 Panel 层）

| 文件 | 改动 |
|---|---|
| `interfaces/webchat/src/appearance.rs` | `ThemeMode` 加 `Glass`（enum + `ALL` + `label` + `storage_value` + `apply_mode` arm）；`from_storage` 加 `"glass" → Glass`、迁移改 `"translucent" → Glass`；doc 注释；单测更新 |
| `interfaces/webchat/styles/tailwind.css` | `--glass-blur`/`--glass-saturate` 变量化；`html.glass` token 组 + 强化 `.glass::before`/景深规则；暗色 aurora glow 重调；`:not(.glass)` 守卫 ~6 处；reduced-transparency 加 glass 分支 |
| `interfaces/webchat/src/components/theme_toggle.rs` | 仅 doc 注释 |
| `interfaces/webchat/src/components/command_palette.rs` | 加回 `theme.glass` 快切 action |

> **无组件 markup 改动**：玻璃 chrome 全靠 CSS（`.glass` 工具类 + `.aleph-sidebar::before` 已在位）。

**构建刷新链**（CLAUDE.md 强制，否则看不到效果）：
`just wasm` → `cargo build --release -p alephcore --bin aleph-server`（rust_embed 烧 dist）→ 替换运行中 binary 让 supervisor relaunch。

## 12. 验收标准

- [ ] 选择器出现 4 档：跟随系统 / 明亮 / 暗黑 / 玻璃。
- [ ] 明亮档视觉**与改动前逐像素一致**（未被波及）。
- [ ] 暗黑档读作「亮色压暗」：同色相、glow 平静、去冷调 drama，玻璃材质与亮色同族。
- [ ] 玻璃档：深色受控 aurora + 明显更强的模糊/高光/景深；**任意壁纸下不暴露原始桌面**、不浑浊、文字 ≥ WCAG AA。
- [ ] 深色 OS 下选玻璃档，token **不被自动暗色规则串味**（DevTools 审查 `:not(.glass)` 生效）。
- [ ] 内容滚动区无 `backdrop-filter`（性能红线保持）。
- [ ] 开启系统「降低透明度」后，三档（含玻璃）均降级为可用实色。
- [ ] 老用户 `aleph-theme=translucent` 平滑迁移为**玻璃**档，无报错。
- [ ] 5 套 accent 下玻璃泛光/选中态正确跟随 `--color-primary`。
- [ ] `appearance.rs` 单测全绿；`cargo check -p alephcore` 与 panel WASM 构建通过。
- [ ] 原生 Bridge（`desktop/shell`、`desktop/macos`）零改动（R1/R2 验证）。

## 13. 非目标 / YAGNI

- 不放真实壁纸透入（受控强化）。
- 不扩玻璃到内容卡片 / 滚动区（性能）。
- 不给输入框/按钮/气泡等小控件加玻璃。
- 不加玻璃强度 / 通透度用户级滑杆。
- 不做壁纸亮度采样自适应。
- 不改原生窗口 / vibrancy 材质类型。
