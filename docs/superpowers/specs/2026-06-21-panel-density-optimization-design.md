# Panel 信息密度优化 — 设计文档

- **日期**: 2026-06-21
- **范围**: `interfaces/webchat`（Leptos/WASM Panel）
- **目标**: 在不改变"优雅气质"的前提下，把当前设计调得更紧凑——缩小字号、压低边框/控件高度、减弱阴影、收紧元素间距，从而单位面积显示更多文字内容。重灾区是聊天侧栏的菜单 chrome 与对话气泡的阴影/间距。
- **力度**: 适中 ~12–15%（"克制"档）。

---

## 1. 背景与诊断

Panel 是一套成熟、令牌驱动的 OKLCH "Quiet Luxury" 设计系统。密度"虚胖"集中在两层：

**A. 全局令牌层** (`interfaces/webchat/styles/tailwind.css`)
- 正文 `0.875rem`(14px)，宽屏(≥1600px) 升 `0.9375rem`(15px)，`line-height: 1.55`。
- 阴影令牌 `--shadow-md/lg/xl` 三层叠加；气泡专用 `--msg-glass-shadow: 0 4px 16px var(--mat-shadow)`。
- **关键杠杆**: Tailwind v4 的 `--spacing` 基准令牌**当前未被覆盖**（=默认 0.25rem）。覆盖它一个值，全panel 所有 `p-*/py-*/gap-*/space-y-*/w-*/h-*` 数值类按比例收紧。
- 圆角、字号**已各有**一个运行时旋钮（`--control-ui-radius-scale` / `--control-ui-text-scale`），由外观设置页驱动。本设计新增第三个同构旋钮管"间距"。

**B. 组件内联类层**（约 35 个 `.rs` 文件硬编码 Tailwind 类）
- 聊天侧栏 `src/components/chat_sidebar.rs`: 顶部 `p-3 space-y-2` + **三个全宽带边框按钮纵向堆叠**（群聊 👥 / 项目管理占位 📁 / Aleph Hub 🧩，各 `py-1.5`）+ 分隔线 + agent 选择器 + 搜索框（`py-2`）。正文（会话列表）出现前就吃掉大量高度。会话行 `px-3 py-2.5`。
- 聊天气泡 `src/views/chat/messages.rs`: 列表 `space-y-3`（行距 12px），气泡 `rounded-2xl px-4 py-3` + `0 4px 16px` 环境阴影。

---

## 2. 红线与非目标 (Non-Goals)

- **不碰玻璃材质系统**: `--mat-*` 材质原语、`.glass`/`.msg-glass` 的镜面高光(sheen)、颗粒(grain)、frosted blur、OKLCH 调色板**保持不变**。密度优化只动留白 / 字号 / 阴影强度。
- **不破坏镜像不变量**: `.dark` 与 `@media (prefers-color-scheme: dark)` 的 verbatim 镜像由 `src/appearance.rs::mirror_blocks_are_verbatim_copies` 测试守护。本设计所有令牌改动落在 `@theme` / `:root` 单点定义区（`--spacing`、`body`、`--shadow-*`、`--msg-glass-shadow`），**不触碰任何 `.dark` 镜像块**。
- **不逐文件翻修所有 tab**: 绝大多数 tab 的间距由全局 `--spacing` 闸自动受益；只在点名重灾区做定点手术。
- **不引入新依赖**、不改后端 RPC、不改 R1–R10 任何架构边界。旋钮是纯前端 localStorage + CSS 变量，与现有三轴完全同构。

---

## 3. 架构：三正交轴

| 轴 | CSS 变量 | 管什么 | 状态 |
|---|---|---|---|
| 字号 | `--control-ui-text-scale` | 字体（经 root font-size，所有 rem 重算） | 现有 |
| 圆角 | `--control-ui-radius-scale` | `--radius-*` 圆角 | 现有 |
| **紧凑度** | **`--control-ui-density`** | **`--spacing` 间距基准** | **新增** |

三轴互不耦合。心智模型、enum 形状、读写重放路径与现有完全一致。

---

## 4. 详细改动

### 块 1 — 全局令牌闸 (`styles/tailwind.css`)

单点定义区，**不触镜像块**。

1. **新增 `--spacing` 覆盖**（接上紧凑度旋钮 + 烤入 12% 紧凑基线）。放入 `@theme` 块：
   ```css
   /* Density knob drives this single Tailwind v4 base unit; default
      0.22rem ≈ 0.25rem × 0.88 → ~12% tighter baseline, knob scales around it. */
   --spacing: calc(0.22rem * var(--control-ui-density, 1));
   ```
   `--control-ui-density` 默认值（旋钮未设/清键）→ fallback `1` → 0.22rem 紧凑基线。

2. **正文字号 + 行距**（`body` 规则）:
   - `font-size: 0.875rem` → `0.8125rem`（14px → 13px）
   - `line-height: 1.55` → `1.5`
   - 宽屏分支 `@media (min-width:1600px) body { font-size: 0.9375rem }` → `0.875rem`（15px → 14px）
   - `letter-spacing` 保持 `-0.005em` 不变。

3. **气泡阴影减弱**（`--msg-glass-shadow`，单点）:
   - `0 4px 16px var(--mat-shadow)` → `0 2px 10px var(--mat-shadow)`

4. **通用阴影令牌减弱 ~20–30%**（`@theme` 单点，玻璃高光不受影响）:
   - `--shadow-md`: 最外层 `0 8px 20px /0.11` → `0 8px 18px /0.09`
   - `--shadow-lg`: 最外层 `0 18px 40px /0.16` → `0 16px 34px /0.12`
   - `--shadow-xl`: 最外层 `0 32px 68px /0.22` → `0 24px 52px /0.16`（弹层/卡片/picker 阴影整体变轻）
   - `--shadow-sm/xs` 已很轻，保持不变。

> 影响面说明：`--spacing` 同时驱动部分定宽/定高控件（如 `w-9 h-9` 图标按钮 → ~32px）。适中档下仍是可点尺寸；个别被全局闸压得过小的控件在块 3 局部补回。`max-w-3xl` 等 container 命名尺寸**不**依赖 `--spacing`，消息列宽度不变。

### 块 2 — "紧凑度"旋钮 (`appearance.rs` + `settings/appearance.rs` + i18n)

**`src/appearance.rs`** — 新增 `Density` enum，完全仿 `FontScale` / `Roundness`：

```rust
const KEY_DENSITY: &str = "aleph-density";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    Compact,  // 默认（清键）— 新紧凑基线
    Cozy,     // 适中 — 回到当前原始留白
    Spacious, // 宽松
}
```

| 档位 | `label()` | `css_value()`(=`--control-ui-density`) | `--spacing` 实际 | `storage_value()` |
|---|---|---|---|---|
| Compact（默认） | "紧凑" | `"1"` | ~0.22rem | `None`（清键） |
| Cozy | "适中" | `"1.13"` | ~0.25rem（原始） | `Some("1.13")` |
| Spacious | "宽松" | `"1.25"` | ~0.275rem | `Some("1.25")` |

配套：
- `Density::ALL`、`from_storage`（`Some("1.13")=>Cozy`、`Some("1.25")=>Spacious`、`_=>Compact`）
- `read_density()` / `apply_density(d)`（`set_property("--control-ui-density", d.css_value())` + `persist`）
- `init_appearance()` 末尾追加 `if density != Density::Compact { apply_density(density) }`
- 单测 `density_round_trips_via_css_value`（host 可跑，仿现有 `font_scale_round_trips_via_css_value`）：清键默认 round-trip + `ALL` 往返。

**`src/views/settings/appearance.rs`** — 在"圆角"分段控件下方加一行同款"紧凑度"分段控件（复用现有 segmented control 渲染逻辑，绑 `read_density`/`apply_density`）。

**`locales/`** — 加 `紧凑度 / Density` 标题与三档 label 的中英文 key（仿现有外观轴的 i18n 条目）。

### 块 3 — 定点手术（点名重灾区）

**`src/components/chat_sidebar.rs`** — 顶部"高级功能区"从三个全宽带边框按钮纵向堆叠 → **一行三个紧凑图标按钮**：
- 群聊 👥（active）、项目管理 📁（disabled + "coming soon"，保留禁用语义但收成图标 + tooltip）、Aleph Hub 🧩（active，导航 `/extensions`）。
- 实现：外层 `flex items-center gap-1.5`，每个按钮 `flex-1` 或定宽方形 + `title` tooltip 保留可达性；保留各自原有 `on:click`/`disabled`/导航逻辑不动。
- 顶部容器 `p-3` → `p-2`；"高级↔普通聊天"分隔线减淡（`border-border/50` → `border-border/40` 或移除多余 `space-y`）。
- 预计为会话列表回收 ≈2 行高度。

**`src/views/chat/messages.rs`** — 消息列表 `space-y-3` → `space-y-2`；气泡 `px-4 py-3` 经全局 `--spacing` 闸自动收紧（无需逐处改），气泡阴影由块 1 的 `--msg-glass-shadow` 减弱。仅在全局闸效果不足处补 1–2 行局部调整。

**其他 tab 轻扫** — settings / memory / agents 等由全局 `--spacing` 闸自动受益。**仅**在个别 chrome 仍明显过重处补少量局部手术，不逐文件翻修（克制）。

---

## 5. 验证

1. **Host 单测**: `Density` round-trip 测试随 `cargo test -p aleph-panel --lib`（或现有 appearance 测试所在 crate）走，**不需 WASM**。
2. **编译 + 产物刷新**: 一次 `just wasm` 重建 `dist/`（被 git 跟踪），确认 WASM 编译通过 + `mirror_blocks_are_verbatim_copies` 等既有测试仍绿。
3. **目视回归（可选）**: 服务端重编 + 截图重部署按用户指令再做；默认只交代码 + 刷新 `dist/`。
4. **手动 checklist**: 切换紧凑度三档 → 间距随档变化、清键回紧凑；切暗色/各强调色/各材质 → 阴影与间距正常；侧栏图标行三按钮 tooltip/禁用/导航行为不变。

---

## 6. 改动面清单

| 文件 | 改动 |
|---|---|
| `interfaces/webchat/styles/tailwind.css` | `--spacing`、`body` 字号/行距、宽屏分支、`--msg-glass-shadow`、`--shadow-md/lg/xl` |
| `interfaces/webchat/src/appearance.rs` | `Density` enum + `KEY_DENSITY` + read/apply/init + 单测 |
| `interfaces/webchat/src/views/settings/appearance.rs` | "紧凑度"分段控件一行 |
| `interfaces/webchat/locales/*` | 紧凑度标题 + 三档 label（中英） |
| `interfaces/webchat/src/components/chat_sidebar.rs` | 高级功能区三按钮 → 一行图标按钮 + 顶部 padding/分隔线收紧 |
| `interfaces/webchat/src/views/chat/messages.rs` | `space-y-3`→`space-y-2`（+ 必要的局部微调） |
| `interfaces/webchat/dist/*` | `just wasm` 重建产物 |

约 6 源文件 + dist 重建。外科、可回退、零新依赖。

---

## 7. 风险与缓解

| 风险 | 缓解 |
|---|---|
| 全局 `--spacing` 把某些定宽/定高控件压得过小 | 适中档仅 12%；个别控件块 3 局部补回；用户可旋钮调"适中/宽松"即时回退（无需重编） |
| 13px 正文偏小 | 与字号轴正交，用户可用现有"字号"旋钮升档；本身也是最易回退的单点改动 |
| 误触 `.dark` 镜像不变量测试 | 所有令牌改动严格落在单点定义区，不碰镜像块；`just wasm` 前跑既有测试确认 |
| 图标行降低"项目管理 coming soon"可发现性 | 保留 disabled 态 + tooltip；语义不丢，仅收紧视觉占位 |

---

## 8. 不做 (Out of Scope)

- 不重做整套配色/材质/动效。
- 不引入响应式断点级别的密度自适应（旋钮足够）。
- 不逐一翻修每个 tab 的内联间距（依赖全局闸 + 点名手术）。
- 不改任何后端 / RPC / 桌面壳。
