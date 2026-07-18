# Panel 玻璃效果细节打磨与性能优化 — 设计

**日期**: 2026-06-09
**范围**: `interfaces/webchat`（aleph-panel crate）的玻璃材质统一、视觉打磨与资源占用优化
**红线**: R2（UI 唯一源在 Panel）。纯 CSS + Leptos class 改动，零原生 Bridge 改动。

## 背景与问题

Panel 当前的玻璃实现已较克制（光晕漂移动画已移除、光晕/颗粒为静态一次性绘制、backdrop-filter 仅用于侧栏 + 2 个弹出层）。但存在三类不一致与可优化点：

1. **`glass-surface` 是未定义的死类**：command_palette、notification_center、directory_browser 的内容卡片用 `glass-surface bg-surface-overlay/95`，既无 CSS 规则、又缺 `glass` 类 → 渲染成 95% 不透明的平板，**没有任何模糊/玻璃感**。而 nav_menu、theme_toggle 用 `glass-surface glass bg-surface-overlay/90` 是真玻璃。同名弹出层观感分裂。
2. **零散的 `backdrop-blur-*` Tailwind 工具类（9 处）**：强度各异（sm/md/[1px]/default），不跟随主题、不响应 `prefers-reduced-transparency`。
3. **常驻模糊与瞬时模糊未分级**：侧栏 `::before` 是唯一全程挂载的 backdrop-filter（持续重绘成本），却和瞬时弹出层共用同一个 `--glass-blur`；Glass 档把它推到 30px，等于常驻 GPU 税被瞬时层的审美需求绑架。

## 核心原则

**瞬时可奢华，常驻要克制。** backdrop-filter 在条件渲染的弹出层/模态上是瞬时的——关闭即 unmount、合成层销毁、GPU 成本完全释放；开着时背景是静态光晕，浏览器缓存模糊结果，成本有界。因此弹出层允许用满模糊。唯一的持久成本是侧栏，必须单独压低。

## 设计

### ① Token 架构（按持久/瞬时分级模糊）

在 `:root` 定义默认值，`.dark` / `html.glass` 按档覆盖：

| Token | 用途 | Light/Dark 默认 | Glass 档 |
|---|---|---|---|
| `--glass-blur` / `--glass-saturate` | **瞬时**弹出层卡片 | 20px / 1.6 | **34px / 2.0** |
| `--glass-blur-chrome` | **常驻**侧栏（唯一全程挂载）| 16px | **24px**（克制，不跟 34）|
| `--glass-blur-subtle` | 内容内小模糊（吸顶头/minimap/聊天薄纱）| 8px | 8px |
| `--scrim-blur` | 全屏遮罩 | 2px | 2px |

- 侧栏 `.aleph-sidebar::before` 的 `backdrop-filter` 从 `var(--glass-blur)` 改为 `var(--glass-blur-chrome)`，使常驻模糊在 Glass 档停在 24px 而非 34px。
- 现有 `--glass-blur: 23px` / `--glass-saturate: 1.6`（:root）与 `html.glass` 的 30px/1.9 调整为上表值（瞬时层提到 34/2.0，:root 默认略降到 20/1.6）。

### ② 统一的 `.glass` 材质（单一来源）

`.glass` 承载完整材质（已在可视化中定稿）：

```
.glass {
  position: relative;
  backdrop-filter: blur(var(--glass-blur)) saturate(var(--glass-saturate));
  /* 斜向高光折进背景，::before/::after 留给亮边与颗粒 */
  background-image: linear-gradient(160deg, oklch(1 0 0 / 0.06), transparent 42%);
  box-shadow: 0 20px 50px oklch(0 0 0 / 0.50), inset 0 1px 0 oklch(1 0 0 / 0.13);
}
.glass::before {  /* 亮边描边 */
  background: linear-gradient(180deg, oklch(1 0 0/0.62), oklch(1 0 0/0.10) 50%, oklch(1 0 0/0.02));
  padding: 1.2px;  /* 现有 mask 结构不变 */
}
.glass::after { opacity: 0.5; /* 细颗粒，现有 SVG noise 不变 */ }
```

- box-shadow / 高光强度按 Light/Dark/Glass 现有的明暗规则微调（Light 档高光更亮、阴影更浅；沿用现有 `:root:not(.dark):not(.glass)` 与 `html.glass` 分支结构）。
- 底色由消费元素的 `bg-surface-overlay/<alpha>` 提供（token 已按主题分档）；材质本身不写死底色。
- **删除死类 `glass-surface`** 的所有用法。

### ③ 表面迁移（消除不一致）

**弹出层卡片** → 统一为真玻璃：
- `command_palette.rs:345`、`notification_center.rs:107`、`directory_browser.rs:344`：`glass-surface bg-surface-overlay/95` → `glass bg-surface-overlay/85`
- `model_picker.rs:128`：裸 `backdrop-blur-md` → `glass`（底色补 `bg-surface-overlay/85`）
- `nav_menu.rs:126`、`theme_toggle.rs:127`：去掉死类 `glass-surface`，`/90` 统一到 `/85`

**全屏遮罩**（5 处）→ 收口到一个辅助类 `.aleph-scrim`：
```
.aleph-scrim {
  background-color: oklch(0 0 0 / 0.40);
  backdrop-filter: blur(var(--scrim-blur));
}
```
替换：`command_palette.rs:339`、`directory_browser.rs:337`、`teams/overview.rs:417`（`bg-black/40 backdrop-blur-sm`），以及 `boot_check_gate.rs:71`、`service_blocking_gate.rs:64`（保留各自的 `bg-surface/95|85` 变暗语义，仅把 `backdrop-blur-sm` 换成 `--scrim-blur` 驱动；这两个 gate 是不透明门，模糊可直接归并）。

**内容内小模糊**（3 处）→ 统一引用 `--glass-blur-subtle`：
- `mode_sidebar.rs:243`（吸顶头）、`minimap_view.rs:62`、`chat/view.rs:156`（薄纱）改为一个共享辅助类或内联 `blur(var(--glass-blur-subtle))`。

### ④ 降级与可达性（安全阀）

扩展现有 `@media (prefers-reduced-transparency: reduce)`：在已有的"`.glass`/侧栏降为纯色"基础上，追加
```
--scrim-blur: 0px;
--glass-blur-subtle: 0px;
```
并确保 `.aleph-scrim` 在该档下退化为纯变暗。`prefers-reduced-motion` 已全局覆盖动画，不改动。

### ⑤ 性能护栏（实现红线）

- **禁止**在任何 backdrop-filter 元素上加 `will-change`（会强制常驻合成层，适得其反）。
- 常驻模糊（侧栏）锁 `--glass-blur-chrome` ≤ 24px；遮罩锁 `--scrim-blur` 2px（全视口但着色器开销极小）。
- 静态光晕 `.aleph-shell::before` / 颗粒 `.aleph-shell::after` 维持现状（一次性绘制、零闲时开销），**不碰**。
- 漂移动画不恢复（此前因 8–13% CPU/GPU 税移除）。

## 不在范围内（YAGNI）

- 不引入 GPU 能力探测 / 运行时质量开关——`prefers-reduced-transparency` 即安全阀。
- 不恢复任何持续动画。
- 不改原生 Bridge、不改 vibrancy 注入逻辑。
- 不新增主题档（仍是 System/Light/Dark/Glass 四档）。

## 验证

- `cargo build --release -p alephcore --bin aleph-server` 后替换运行中 binary（rust_embed 烧 dist）→ 真机肉眼核验四档 + 弹出层一致性。
- 可选：chrome-devtools-mcp 跑 performance trace，对比改动前后闲时（侧栏常驻模糊降 30→24px）与模态开启时的合成开销。
- `prefers-reduced-transparency: reduce` 下确认所有模糊归零、退化纯色。
- panel 单测：`cargo test -p aleph-panel`（纯逻辑无 CSS 断言，主要确保不破坏编译）。

## 隔离

按用户要求，实现在 **git worktree** 中进行，完成后合并 main（遵循本仓单分支模式 + `--no-ff` 合并前核验并发 main 零重叠的既有流程）。

## 部署说明

Panel dist 经 rust_embed 在 `aleph-server` 编译期静态嵌入；改完 CSS/Leptos 必须 `just wasm` + 重编 binary + 替换运行中 binary 才生效。本设计的视觉效果在重编部署前不可见（DEFERRED 部署属正常）。
