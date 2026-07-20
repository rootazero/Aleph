# Panel 玻璃材质语言 — 设计方案

> 日期: 2026-06-09 · 范围: Panel (Leptos/WASM) · 状态: 设计已确认，待写实现计划

## 1. 背景与问题

当前 Panel 有一个可切换的 **Vibrant（`translucent`）主题**，三要素（半透明背景 / backdrop-blur / 边框高光）已具备，并接好了 macOS 原生 vibrancy（`NSVisualEffectMaterial::Sidebar`）+ 浏览器 aurora fallback。但实际效果差，根因经截图确认：

1. **质感差（平）**：玻璃面只是「平涂淡色 + 弱模糊」，缺少边缘高光、景深、去塑料感处理 → 像「半透明灰盒子」而非材质。
2. **作为主题存在，而非材质语言**：玻璃是一个要手动切进去的开关档，不是 UI 与生俱来的质感。
3. **文字不清晰 + 背景刺眼**：`html.translucent` 把画布设成**全透明**，于是 macOS 让**真实桌面壁纸**直接透进来。Vibrant 的表面 token 是为「深色 aurora 背景」设计的暗色（如 `--color-surface-raised: oklch(0.22 0.018 310 / 0.55)`），叠在一张明亮壁纸上 → 浑浊灰盒子、刺眼壁纸底、灰字低对比。质感与可读性完全被用户壁纸劫持。

**核心诊断**：不是「模糊不够强」，而是玻璃**没有自己受控的背景**——在 macOS 上它继承任意壁纸，而 token 又假设相反（深色）背景。

## 2. 目标

把玻璃从「可选主题」升级为 Panel 的**默认材质语言**，且：

- 不管用户桌面壁纸是什么，玻璃都**稳定、有质感、文字清晰**。
- 浅色 / 暗色两个模式**都**带玻璃质感。
- 保留通透感（壁纸极淡可感），但由我们自己的受控背景层主导。
- 性能克制（呼应 backdrop-filter 的 GPU 重绘成本），系统级可达性降级。

### 红线定位（关键）

本方案是**纯 Panel 层改造，零原生代码改动**：

- macOS 窗口**本来就**永远 `transparent(true)`（`desktop/shell/src/main.rs:208`）；vibrancy 材质**本来就**在启动时无条件挂上（`main.rs:149`）。
- 当前只有 `html.translucent` 把 webview 背景设透明让 vibrancy 透出；Light/Dark 档用不透明 `bg-surface` 把它盖住。
- 因此「玻璃默认化 + 受控背景」全部靠 **CSS / design token / 退掉 Vibrant 档** 实现，原生 Bridge 一行不动。

→ 符合 **R1**（大脑-四肢分离，不在 src/native 写业务）、**R2**（UI 唯一源在 Leptos）。

## 3. 决策记录（已与用户逐项确认）

| 维度 | 决定 |
|---|---|
| 玻璃后面是什么 | **自带受控背景层**：壁纸与内容之间永远铺一层我们自己的半透明色调 + 柔和光晕 |
| 架构形态 | **成为默认材质语言**：退掉单独的 Vibrant 档，Light/Dark 都默认带玻璃 |
| 暗色背景氛围 | **冷静**：两抹柔和光晕（mauve + 蓝），有呼吸但不抢戏、不刺眼 |
| 浅色玻璃 | **雅灰玻璃**：略冷灰 + 更明确边界 + 深色 AA 文字（亮底上不"消失"） |
| 玻璃质感档位 | **C′ 克制液态玻璃**：质感要素全保留，响度收住（见 §5） |
| 应用范围 | **真玻璃只给固定 chrome**：侧栏/顶栏/弹层/模态。内容区卡片用「半透明 raised 面」坐在受控 aurora 上（**不加 backdrop-filter**），主内容滚动区同理 |

### 关于 C′ 的 GPU 权衡（决策依据）

逐帧 GPU 成本**只有** `backdrop-filter: blur()`，B/C/C′ 同款。C′ 相对 B 多出的描边、噪点、景深、内高光、泛光都是**一次性静态绘制**（合成层缓存，≈0/帧），悬停 transform 仅 hover 时且 GPU 合成廉价。真正会爆 GPU 的「backdrop-filter 叠高频变动内容」已被「仅 chrome + 静态 aurora」从架构上规避。结论：**C′ 运行时 ≈ B，但质感显著更好**，故选 C′ 而非退回 B。

## 4. 背景层模型

堆叠（后 → 前）：

```
桌面壁纸  →  macOS vibrancy 模糊层  →  【我们的半透明受控 aurora 罩层（主导）】  →  玻璃 chrome  →  内容
```

- `--aleph-canvas-base` 从 `transparent` 改为**半透明受控底色**：
  - **暗色**：冷静深空底 + 两抹柔和光晕（mauve/蓝），不透明度足以把壁纸压成极淡律动。
  - **浅色**：近白柔光底 + 极淡 mauve/蓝/绿光晕（绝不刺眼）。
- macOS：always-on 的 vibrancy 在我们半透明罩层后faintly透出 = 保留通透感且稳定。
- 浏览器 / 非 macOS：无壁纸，同一套 CSS 落在不透明底上，观感一致。
- 主内容滚动区直接坐在该 aurora 画布上，**不加 backdrop-filter**（性能关键）。

## 5. 玻璃材质 token（锁定 C′ 参数）

新增一组 `--glass-*` design token + 一个 `.glass-panel` 工具类，封装 C′ 配方。**暗色**基线参数：

- 填充：`linear-gradient(180deg, rgba(255,255,255,0.14), rgba(255,255,255,0.045))`（上亮下暗）
- 模糊：`backdrop-filter: blur(23px) saturate(1.6)`（克制，非 28/1.9）
- 渐变高光描边：`::before` + mask 技法，`linear-gradient(180deg, rgba(255,255,255,0.4), rgba(255,255,255,0.04) 42%, transparent)`
- 细噪点：`::after` SVG `feTurbulence`，`opacity ≈ 0.4`（去塑料感）
- 景深阴影：`inset 0 1px 0 rgba(255,255,255,0.26)`（内顶高光）+ `0 2px 5px / 0 12px 32px rgba(0,0,0,…)`（分层）
- accent 泛光：**仅选中/聚焦态**，`0 0 8px color-mix(…, --color-primary 14%)`（极淡）
- 悬停浮起：`translateY(-1px)`（微）

**浅色（雅灰玻璃）**参数（同结构，换值）：

- 填充：略冷灰高白调 `linear-gradient(180deg, rgba(252,251,255,0.78), rgba(244,242,250,0.6))`
- 描边高光偏白 + 底边极淡墨边 `rgba(60,50,80,0.06)`（更明确边界，亮底不消失）
- 文字用深色 token（主 `oklch(0.25 …)` / 次 `oklch(0.42 …)`，均达 AA）

要素与 accent 调色板联动：玻璃泛光 / 选中态都从 `--color-primary` 取色，自动跟随现有 5 套 accent（mauve/ocean/forest/sunset/rose）。

文字对比：在玻璃语境下重校 `--color-text-*` 至 AA 以上（修「文字不清晰」）。

## 6. 范围：真玻璃只给固定 chrome

**真玻璃面（有限个数、位置固定、后面是静态 aurora → 模糊结果可缓存，零滚动成本）**：
侧栏 `.aleph-sidebar`、顶栏、弹层菜单 / popover（现有 `.glass` 类）、模态对话框。
实现：升级现有 `.glass` 工具类承载 C′ 配方 + 给 `.aleph-sidebar` 选择器加 backdrop-filter。**无需改组件 markup**（这些表面已有 class 钩子）。

**非真玻璃（不加 `backdrop-filter`）**：
- **内容区卡片**（`bg-surface-raised` 盒子）：用半透明 raised 面坐在受控 aurora 上 → 视觉与玻璃 chrome 协调，但滚动时**不逐帧重算模糊**（规避 GPU 陷阱）。仅靠 `--color-surface-raised` token 调整实现，**零 markup 改动**（且 `bg-surface-raised` 与输入框共用，不能上 blur）。
- 主内容滚动区、输入框 / 按钮 / 消息气泡 / 小控件：保持实色。

> **为何卡片不上真玻璃**：卡片在滚动内容区里，backdrop-filter 会每滚动帧重算模糊 = 用户明确担心的 GPU 陷阱；且 `bg-surface-raised` 全站统一（连输入框共用），逐站点加玻璃 class 改动大且违 R3。

## 7. 主题选择器变更

- `ThemeMode`：`System / Light / Dark / Vibrant` → **`System / Light / Dark`**，删除 `Vibrant`。玻璃在三档里都是默认材质。
- **向后兼容**：`ThemeMode::from_storage(Some("translucent"))` → 迁移读成 `Dark`（保留分支，老用户偏好平滑迁移，不报错、无感）。
- `theme_toggle.rs`：选择器去掉 Vibrant 项。
- `command_palette.rs`：删除（或改为兼容别名）切换 "玻璃/vibrant" 的命令；关键词 `["theme","vibrant","translucent","glass","玻璃"]` 相应处理。
- `html.translucent` 专属 CSS 块：删除/合并（其受控 token 思路上移为默认）。

## 8. 性能与可达性

- **`prefers-reduced-transparency`**（macOS「降低透明度」辅助功能触发）→ 自动降级：`.glass-panel` 变不透明实色、移除 `backdrop-filter`、aurora 退为纯色底。系统级尊重，零用户配置。
- 静态渐变（项目此前已移除 60Hz drift 动画，继续保持，不引入新的逐帧动画）。
- chrome-only 限定模糊面数量。
- `prefers-reduced-motion` 已全局尊重，沿用（悬停浮起等在该模式下被抑制）。

## 9. 改动文件清单（全部 Panel 层）

| 文件 | 改动 |
|---|---|
| `interfaces/webchat/styles/tailwind.css` | 主战场：受控 aurora（`--aleph-canvas-base` 半透明化）、升级 `.glass` 工具类为 C′ 配方（暗/浅双套，含 `::before` 描边 + `::after` 噪点）、`.aleph-sidebar` 加 backdrop-filter、`--color-surface-raised` 半透明化（卡片）、reduced-transparency 降级块、删 `html.translucent` 专属块、玻璃语境文字对比重校 |
| `interfaces/webchat/src/appearance.rs` | `ThemeMode` 退 `Vibrant`（enum + `ALL` + `label` + `storage_value` + `apply_mode` arm）；`from_storage` 加 `"translucent" → Dark` 迁移；更新/新增单测 |
| `interfaces/webchat/src/components/theme_toggle.rs` | 仅改顶部 doc 注释（选择器由 `ThemeMode::ALL` 泛型渲染，退 enum 后自动少一项，无逻辑改动） |
| `interfaces/webchat/src/components/command_palette.rs` | 删 `theme.vibrant` action + `apply_theme` 的 `"translucent"` 分支（`remove_3` 保留以清理遗留 class） |

> **无组件 markup 改动**：玻璃 chrome 全靠 CSS（`.glass` 工具类已在所有 popover 上 + `.aleph-sidebar` 选择器）。卡片靠 `--color-surface-raised` token，零站点编辑。

**构建刷新链**（CLAUDE.md 强制，否则看不到效果）：
`just wasm` → `cargo build --release -p alephcore --bin aleph-server`（rust_embed 烧 dist）→ 替换运行中 binary 让 supervisor relaunch。

## 10. 验收标准

- [ ] 任意桌面壁纸（含明亮/高饱和）下，玻璃面稳定、不浑浊、不刺眼。
- [ ] 浅色 / 暗色双模式正文与次要文字对比 ≥ WCAG AA。
- [ ] 主内容滚动区无 `backdrop-filter`（DevTools/审查确认）。
- [ ] 开启系统「降低透明度」后，玻璃自动降级为可用实色。
- [ ] 老用户 `aleph-theme=translucent` 偏好平滑迁移为 Dark，无报错。
- [ ] 5 套 accent 调色板下玻璃泛光/选中态正确跟随 `--color-primary`。
- [ ] `appearance.rs` 单测全绿；`cargo check -p alephcore` 与 panel WASM 构建通过。
- [ ] 原生 Bridge（`desktop/shell`、`desktop/macos`）零改动（R1/R2 验证）。

## 11. 非目标 / YAGNI

- 不引入玻璃强度 / 通透度的用户级滑杆开关（架构选定「默认材质，无开关」）。
- 不做壁纸亮度采样自适应（复杂、脆弱，违 R10 薄 harness 倾向）。
- 不给输入框/按钮/气泡等小控件加玻璃（性能 + 美学克制）。
- 不改原生窗口 / vibrancy 材质类型。
