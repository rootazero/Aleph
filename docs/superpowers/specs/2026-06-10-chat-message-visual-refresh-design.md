# Chat 消息流视觉刷新（玻璃语言对齐）— 设计

日期：2026-06-10
范围：`interfaces/webchat/`（Panel 前端）——`styles/tailwind.css` 为主 + `src/components/markdown.rs` / `src/components/tool_card.rs` / `src/views/chat/{messages,reasoning}.rs` 定点 class 替换。后端 `src/` 零改动。

## 背景

前两轮玻璃优化（2026-06-09 `ba646da5c`、2026-06-10 `921b6daa1`）把 chrome（侧栏、modal、popover、menu）全部迁移到玻璃材质，建立了 `--glass-*` token 体系、`.glass` 组件类和 `prefers-reduced-transparency` 回退。但**对话消息区从未动过**：

- 消息气泡还是平面纯色卡片（用户 `bg-primary` 实心蓝、助手 `bg-surface-raised`），与玻璃化 chrome 形成视觉断档
- `.markdown-body` 排版（tailwind.css:624-679）是早期风格：标题全 700 粗细无层次、表格每格 1px 实线网格、引用块灰线+斜体（中文斜体效果差）
- 代码块样式硬拼在 `markdown.rs` 的内联 Tailwind 类里（`render_markdown`:56 与 `render_streaming`:179 重复两份），纯色 `surface-sunken` 底+实线边框
- 工具卡片（`tool_card.rs`）、思考面板（`reasoning.rs`）、步骤条（StepStrip）、空状态（ChatHero）同为旧风格

Markdown 解析栈本身不弱（pulldown-cmark + syntect 双主题语法高亮 + 流式轻量渲染器），**问题纯在视觉层**——本轮不动解析与渲染逻辑。

## 已确认的视觉决策（用户经浏览器 mockup 选定）

| 决策 | 选定方案 |
|------|----------|
| 范围 | 全消息流统一改造（气泡+markdown+代码块+工具卡+思考面板+步骤条+空状态+分隔线+错误） |
| 助手消息形态 | **玻璃气泡卡片**（保留气泡结构，升级玻璃材质；否决文档式平铺与缘线折中） |
| 代码块 | **精修标题栏**（保留语言+Copy 标题栏结构，换玻璃材质；否决 macOS 红绿灯与极简浮动标签） |
| 用户气泡 | **accent 着色玻璃**（否决保持纯色 primary） |

Mockup 留档：`.superpowers/brainstorm/84404-1781081478/content/`（已 gitignore，仅本地参考）。

## 核心技术决策：仿玻璃，零新增 backdrop-filter

消息列表可达数百条气泡，每条挂 `backdrop-filter` 是真实合成器开销；且气泡在滚动流内、背后是静态面板背景，模糊无可感知收益——纯付成本。因此：

- **气泡与内嵌表面一律"仿玻璃"**：半透明 oklch 底 + 160° 渐变高光叠加 + 1px 半透明边框（顶边更亮）+ 柔影。视觉与 mockup 一致（mockup 即此手法）。
- 真 `backdrop-filter` 继续只属于瞬时悬浮表面（modal/menu/popover），延续前两轮的性能红线。
- 不加 `will-change`。

## A. 气泡材质（新组件类）

`tailwind.css` `@layer components` 新增，`messages.rs` 做 class 替换（`bg-surface-raised` → `msg-glass`；`bg-primary text-white` → `msg-glass-user`）：

**`.msg-glass`（助手气泡）**
- 底：dark `oklch(0.24 0.02 310 / 0.75)`；light `oklch(1 0 0 / 0.65)`
- 光泽：`linear-gradient(160deg, oklch(1 0 0 / 0.05), transparent 42%)` 叠加（复用 `.glass` 同款手法，light 模式高光减淡）
- 边框：1px 半透明，dark 顶边 `oklch(1 0 0 / 0.22)` 提亮；light 改深色 hairline（白底上白高光不可见）
- 影：dark `0 4px 16px oklch(0 0 0 / 0.25)`；light 减淡

**`.msg-glass-user`（用户气泡，accent 着色玻璃）**
- 全部颜色从 `--color-primary` 经 `color-mix(in oklch, …)` 派生：半透明同色底（~65% 不透明度）、同色系亮边、同色柔影
- 四个 accent 色板（ocean/forest/sunset/rose）切换自动跟随，零每色板特例
- 文字近白，维持 WCAG AA 对比

**三主题适配**
- `html.glass`：镜像现有 `.glass` 强化规则（顶边更亮）
- `prefers-reduced-transparency`：`msg-glass` / `msg-glass-user` / `glass-inset` 全部回退不透明实底——追加进现有回退 media query 块

## B. Markdown 排版刷新（`.markdown-body`，纯 CSS）

渲染器输出的 HTML 结构不变：

| 元素 | 现状 | 新规格 |
|------|------|--------|
| 标题 | h1-h3 全 700 粗、阶梯扁 | 字号阶梯拉开（h1 1.25rem / h2 1.0625 / h3 0.9375），h1/h2 底部渐隐 hairline |
| 链接 | primary 色 + 实线下划线 + hover 整体 opacity | accent 色，`text-decoration-color` 低透明度，hover 变实 |
| 引用块 | 2px 灰线 + 斜体 | 3px 圆头 accent 缘线 + 同色极淡背景洗，**去斜体**（中文渲染差） |
| 表格 | 每格 1px 实线网格 | 外层圆角 hairline 容器、表头半透明填充+底部分隔、行间仅水平 hairline、无竖线；保留 `display:block` 横向滚动 |
| 行内代码 | `surface-sunken` 实底 | 半透明底 + 1px hairline 边框 chip |
| hr | 实线 | 两端渐隐渐变 hairline |
| 图片 | 圆角 | 圆角 + hairline 边框 + 柔影 |
| 任务列表 | `accent-color` 已有 | 不动 |

## C. 代码块（精修标题栏）

DOM 结构保持 `wrapper > header > pre`，但**内联 Tailwind 类收敛为语义类**：`render_markdown` 与 `render_streaming` 的重复类字符串（两份）迁到 `.code-block-wrapper` 子选择器统一供样式。Rust 字符串大幅简化，未来改样式零碰 Rust。

- **标题栏**：`linear-gradient(180deg, oklch(1 0 0 / 0.05), transparent)` 高光 + 半透明底（dark `oklch(0.19 0.02 310 / 0.9)`）+ 底部 hairline；左语言标签、右 Copy 按钮
- **`pre`**：深色半透明底（dark `oklch(0.15 0.02 310 / 0.85)`），无独立边框
- **wrapper**：统一 1px hairline + 亮顶边 + 10px 圆角 + `overflow:hidden`
- **Copy 按钮**：hover 显隐与 onclick 复制逻辑（含 "Copied!" 反馈）原样保留，样式 chip 化
- **syntect 不动**：双主题选择（`base16-ocean.dark` / `InspiredGitHub`）与背景剥离逻辑原样；背景由 CSS 供给
- streaming 版（无 Copy 按钮）同款材质，完成切换无视觉跳变
- light 模式：标题栏/pre 换浅色半透明系，配 InspiredGitHub 前景

## D. 嵌入表面（`.glass-inset` 共享类）

新增 **`.glass-inset`**：比气泡更轻的内嵌玻璃（半透明底 + hairline 边框 + 微亮顶边、无影），建立材质层级 `面板 Atmosphere → 气泡 msg-glass → 内嵌 glass-inset`：

| 表面 | 文件 | 改动 |
|------|------|------|
| 工具卡片容器 | `components/tool_card.rs` | 换 `.glass-inset`；状态色（运行/成功/失败）与折叠逻辑不动；展开区（diff/输出）背景与代码块 `pre` 同款 |
| 思考面板 | `views/chat/reasoning.rs` | 折叠态保持轻量文本行（脉冲点不动）；展开全文容器换 `.glass-inset` |
| 步骤条 StepStrip | `views/chat/messages.rs:614-665` | hairline 语言轻量化 |
| 空状态 ChatHero | `views/chat/messages.rs:18-86` | 启动器建议卡片换玻璃 chip |
| 日期分隔线 | `views/chat/messages.rs:281-290` | 两端渐隐 hairline + 中央半透明胶囊 |
| 错误消息 | `messages.rs` error_view | danger 着色玻璃（同用户气泡手法，从 danger token `color-mix` 派生） |

## E. 动效（克制，零新增 keyframe）

- 入场 `aleph-msg-in`（0.34s）与"流式中不重放"逻辑不动
- 流式光标：方块微调为圆角细条 + accent 渐变（仍 `animate-pulse`）
- `reading-dots`、悬停操作栏/Copy 按钮 opacity 过渡均不动
- `prefers-reduced-motion` 无新增回退点（无新动画）

## F. 回退、性能与验证

**回退矩阵**

| 条件 | 行为 |
|------|------|
| `prefers-reduced-transparency` | 全部新类回退不透明实底（挂现有 media query） |
| light 模式 | 专属 token：白玻璃底 + 深色 hairline |
| `html.glass` | 镜像 `.glass` 强化（顶边更亮） |
| accent 切换 | `color-mix` 派生全自动跟随 |

**性能**：零新增 `backdrop-filter`；渐变+半透明合成成本可忽略；WASM 体积不受影响（CSS 为主 + class 字符串替换）。

**验证**
1. `cargo check` webchat crate + `cargo clippy` + `just wasm` 通过
2. `markdown.rs` 补最小单元测试（现零测试）：断言 `render_markdown` / `render_streaming` 输出含新语义类、语言标签 HTML 转义不回归
3. 视觉验收：按 CLAUDE.md 刷新链（`just wasm` → 重编 binary → 替换触发 supervisor relaunch），chrome-devtools 在 **dark / light / glass 三主题**截图一条含全部 markdown 元素（标题/表格/代码块/引用/任务列表/链接/行内代码）+ 工具调用 + 思考过程的长消息逐项核对
4. 开发在 **git worktree 隔离**进行（用户要求）；遵守项目雷区：worktree 内只合并不删除，清理用新会话

**刻意排除（勿"顺手修"）**
- 不动 pulldown-cmark / syntect 解析与高亮逻辑
- 不动消息列表吸底滚动、流式切换渲染器等行为逻辑
- 不动 chrome 已玻璃化的表面（modal/menu/sidebar）
- 不为气泡加 backdrop-filter（性能红线）
- 不拆分 `messages.rs`（678 行未达拆分阈值）
