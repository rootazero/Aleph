> **历史状态说明 / Historical status (2026-10-25)**：本 2026-10-15 方案早于 2026-10-24 runtime-agent/agent-panel 移除；文中关于 sidebar/agent_panel.rs、runtime agents 和 live-agent rows 的引用均为历史规划材料，不构成当前产品契约。当前 terminal 边界见 [`docs/reference/TERMINAL_RUNTIME.md`](../reference/TERMINAL_RUNTIME.md)。
>
> This 2026-10-15 plan predates the 2026-10-24 removal of runtime agents and the agent panel. References to sidebar/agent_panel.rs, runtime agents, and live-agent rows are historical planning material, not current product contracts. The current terminal boundary is [`docs/reference/TERMINAL_RUNTIME.md`](../reference/TERMINAL_RUNTIME.md).
>
# Aleph 右侧栏优化方案 (2026-10-15)

> **任务边界** — 本文是一次**调研 + 方案**交付,**不实施代码重构**。所有 Aleph 现状
> 引用均经过源码核实(只读 Explore agent 输出 + 关键 file:line),ChatGPT 桌面端
> 右侧栏仅在**功能/架构层面**描述(可观察的 UI 行为,未复刻其代码或专有文案)。
>
> **适用对象** — Panel (interfaces/webchat, Leptos/WASM) 与 TUI
> (interfaces/tui, Ratatui);桌面壳 `desktop/shell` 是 Tauri thin host,只承载
> webview,不直接参与右侧栏渲染。
>
> **前置文档** — [ARCHITECTURE.md](../reference/ARCHITECTURE.md) ·
> [DESIGN_PATTERNS.md](../reference/DESIGN_PATTERNS.md) ·
> [FEATURE_LOCATOR.md](../reference/FEATURE_LOCATOR.md) · [HARNESS_PHILOSOPHY.md](../reference/HARNESS_PHILOSOPHY.md)

> **状态校正（2026-10-21）**：这里的“后端已 mount, UI 完全不可见”是 UI 范围结论，不是 runtime mount 缺口。生产启动已经挂载 `ProjectionHost`；`src/capability/provider.rs` 也已提供并接入 Tool registry 的 Provider lifecycle foundation。右栏 Capability body 仍未实现，Provider 的 ExtensionManager 副作用驱动、Bridge sidecar 和动态库卸载仍属于后续工作。

---

## 0. 摘要 (TL;DR)
| 维度 | 当前状态 | 建议主轴 |
|---|---|---|
| 顶层布局 | 14.5rem 左侧栏 + main,无传统"右列" | 把"右列"从 absolute overlay 升级为 flex 列(可选宽度 0/默认/拖拽) |
| 右侧内容 | 硬编码 `WorkspaceBody` 枚举,**7 个 touch point**(`workspace_panel.rs` / `chat/view.rs` / `app.rs` 三处文件收敛) | 抽出 `RightPaneSlot` 注册式接口,实现"添加新 body 只改 1 处";阶段 2 host 切换先把容器从 absolute 改成 flex 兄弟列,见 [`2026-10-15-rightpane-stage2-host-switching-spec.md`](2026-10-15-rightpane-stage2-host-switching-spec.md) |
| 子代理显示 | 3 套独立 UI(sidebar live agents / chat docked / `/dashboard/subagents`) | 折叠为"右列默认 subagent tree + 聊天内 docked 行"两处,路由版降为 deep-link |
| 浏览器 | 只读状态壳 + `_blank` 跳转 | 引入受控 browser view(spec-only,本期定义 wire 协议与 placeholder) |
| 附件 | composer 内仅"文件名 + ✕" | 升级为 type-icon + 缩略图 + 大小 + 全局 attachment tray(右列可钉) |
| 能力/ProjectionHost | 后端已 mount,UI 完全不可见 | 在右列新增 `Capabilities` body(只读、零成本) |
| TUI | 单列垂直 + 28 列左侧 agent panel | 选配:加 `Constraint::Percentage(0..40)` 右列用于 subagent inspector |

**总原则(红线来自 R1–R10):**
- R1 — Core 不渲染 UI,右列只消费事件流
- R2 — 复杂 UI 留在 Leptos/WASM(Rust 端只暴露 `RightPaneSlotDescriptor`)
- R7 — 右列内**不**调用 LLM 决断"哪个 body 高亮";启发式由前端 rule 完成
- R10 — Harness 仍保持 12 文件约束,本次新增不超过 4 个组件文件 + 1 个 slot trait

---

## 1. 现状审计 — Aleph 的"右侧"长什么样

> 完整只读调研见附 A;本节给出结论性地图。

### 1.1 Panel (Leptos/WASM)

- **顶层 shell**( `interfaces/webchat/src/app.rs:419-498` )
  - `<aside class="aleph-sidebar w-[14.5rem]">` —— 固定 14.5rem 左侧
  - `<main class="flex-1 … overflow-y-auto">` —— 占满剩余宽度,**没有"右列"**
- **唯一真"右"UI — Workspace Pane**( `interfaces/webchat/src/components/workspace_panel.rs:184-189` )
  - `position: absolute; right: 0; z-20` —— 浮层,不影响主区 flex
  - 默认隐藏,仅 `LayoutMode::Split` 出现
  - 5 个 body:`Artifacts / Deliverables / Tasks / Canvas / Browser`
  - 宽度 `--aleph-workspace-w` CSS 变量,clamp 防越界,可拖拽
- **另一条右覆盖物 — Team Task Drawer**( `interfaces/webchat/src/components/team_task_strip.rs:107` )
  - `position: fixed; right: 0; z-[81]; w-[320px]` —— 320px 浮层
  - 仅 team 模式出现

### 1.2 TUI (Ratatui)

- **水平切分**( `interfaces/tui/src/tui/render.rs:48-58` )
  - `Layout::horizontal([Length(28), Min(0)])` —— agent panel 在**左** 28 列
  - 隐藏路径:`main_area = full_area`(R8-9 锁)
- **Region 命中系统**( `interfaces/tui/src/tui/regions.rs` )
  - `RegionKind` 只追踪 `BackToBottom` + `ToggleRow`,**右列是主动放弃的方向**

### 1.3 子代理(Subagent)三套 UI

| 位置 | 文件 | 数据源 |
|---|---|---|
| Webchat sidebar live agents | `interfaces/webchat/src/components/sidebar/agent_panel.rs` | `RUNTIME_AGENTS_CHANGED_TOPIC` |
| 聊天内 docked | `interfaces/webchat/src/platform/wide/views/subagent_tree/mod.rs` (挂在路由 `/dashboard/subagents`) | `subagent.tree` RPC + `TREE_TOPIC` |
| TUI 端 | `interfaces/tui/src/tui/widgets/agents_panel.rs` (输入框上方精简) + `agents_overlay.rs` (全屏) | `AppState.agents` |

⚠ **命名陷阱**(已用 R#-N 注释 + 源扫描测试守护):`AgentPanel` / `agents_sidebar` / `agents_panel` / `agents_overlay` 四个名字差一词,见 `agents_sidebar.rs:17-19`。

### 1.4 浏览器(Browser) — 状态壳,而非嵌入

- 主渲染器: `interfaces/webchat/src/platform/wide/views/browser/viewer.rs:781-822`
  - 三态:`Unbound / Bound+Withheld / Bound+Ready`
  - **不**渲染 iframe / webview,只展示元数据 + `LaunchSection.open` → `_blank`
- 独立页 `/viewer/browser`(`viewer/standalone.rs`):通过 WebSocket 拿一帧后 `<img>`,**不持久**任何状态
- 注释明示"the shell still does not control the browser"—— 想做"右栏内嵌浏览器"必须从零写

### 1.5 文件 / 附件

- **草稿**:`interfaces/webchat/src/platform/wide/views/chat/composer/attachments.rs`
  - 145 行,只渲染"文件名 + ✕"两列,**无** type-icon / 缩略图 / 大小
- **落地**:`interfaces/webchat/src/components/artifacts/` —— Workspace Pane 的 body
  - 四类内容:deliverables / inbound / outbound / workspace files
  - 图片有缩略图,文本/二进制仅 `name + size + origin badge`
  - 项目树(files.rs)只做折叠展示,无"双击预览"动作

### 1.6 工具调用 / 审批

- 工具行: `interfaces/webchat/src/components/tool_card.rs`(1438 行)与 TUI `tool_row.rs`
- 审批: `approval_card.rs:60` `ApprovalCard` —— 聊天内 inline + 顶部 banner 共用
- AskUser: `ask_user_card.rs`,启动时 `ClarificationApi::list_pending` 拉一次

### 1.7 能力 / ProjectionHost(Capability Phase 4)

- 后端 mount 位置(已核实):
  - `src/bin/aleph-server/commands/start/mod.rs:70-71` —— `Arc<ProjectionHost>`, `Arc<OwnershipTree>`
  - 行 268-269:`OwnershipTree::new()` + `ProjectionHost::mount_with_diagnostics(...)`
- **UI 侧:无任何展示** —— grep `interfaces/{webchat,tui}` 命中 0 处
- 无 `/settings/capabilities` / `/dashboard/capabilities` 路由
- `ProjectionHost::subscriber_count / last_revision / invalidated?` 全部不可见

### 1.8 当前"右侧栏"痛点(一句)

**没有"右列"抽象**。Workspace Pane 是唯一右侧浮层(absolute overlay),任何新增
right-side 内容都要修改 `WorkspaceBody` 枚举 + `available_with` + `body_label`
+ `body_testid` + `viewer.rs::match` 五处(见 `workspace_panel.rs:42-66, 209-241`)。

---

## 2. ChatGPT / Codex 桌面端右侧栏 — 功能层观察

> **声明**:本节基于 *功能可观察的 UI 行为* 描述,**不复制任何代码、字符串、
> 私有类型或文案**。细节仅用于对齐"用户能感知什么",不作为可移植实现。

### 2.1 形态:多 Tab "Companion" 列

- 主区右侧是**一列**而非浮层,占据屏幕宽度由 `clamp_width` 锁定 **`[280 px, 80% viewport]`** 区间可调(下限定死 280,上限随视口变;`state/layout.rs:132-147`)
- 列顶部是水平 Tab 栏,常见 Tab(从公开 UX 角度):
  1. **Code / Diff** — 当前 turn 的代码产出/修改
  2. **Files** — 会话涉及的文件树 + 元数据
  3. **Browser** — 受控的浏览器视图(computer use 时显示页面截图/可交互)
  4. **Tools / Agents** — 当前 subagent / 工具调用进度
- 列**可隐藏**:右上角一个 chevron 切换;隐藏后主区吃回整个宽度

### 2.2 行为模式(从用户可观察层归纳)

| 行为 | 描述 | 价值 |
|---|---|---|
| **可拖拽宽度** | 列右边缘可拖动,UI 立即重排 | 用户按内容密度自适配 |
| **Tab 持久化** | 选中的 Tab 在 session 间保留 | 不重复劳动 |
| **空状态** | 还没产生内容时显示"等会会有…"的占位 | 降低焦虑 |
| **行内预览** | 在列内直接渲染 HTML/图片/代码,不离开主区 | 减少跳转 |
| **右键 / 浮层操作** | "Open in main area" / "Open in new window" | 多种用法兼容 |
| **键盘可达** | ⌘[` / `]` 在 Tab 间切换 | 键盘党可用 |

### 2.3 与 Aleph 当前形态的对比

| 维度 | ChatGPT/Codex 桌端 | Aleph Webchat (当前) | Aleph TUI (当前) |
|---|---|---|---|
| 是否"列" | 是(flex 布局) | 否(absolute overlay) | 否(单列) |
| 默认 Tab 数 | 3–4 | 5(WorkspaceBody) | 1(agents_panel) |
| 宽度可调 | 拖拽 + 快捷键 | 拖拽(workspace pane) | 无 |
| 浏览器 | 受控嵌入 | 状态壳 + 跳转新窗口 | 无 |
| 文件展示 | 树 + 缩略图 | Artifacts(已落地) + composer chips | 无 |
| Subagent | 集中展示 | 三处分散 | 一处精简 |
| 默认可见 | 用户可关 | Split 模式才出现 | 常驻 28 列 |

---

## 3. 优化建议

### 3.1 战略层(决策性问题)

#### 3.1.1 "右列"作为一等公民 — 替换 absolute overlay

- **方案 A(推荐)** :把 `Workspace Pane` 从 `position:absolute` 升级为 flex 第三列
  - 顶层 `Layout`:`[ModeSidebar | Main | RightPane]` 三个 flex item
  - `RightPane` 默认宽度 0(可配 0/默认/拖拽/折叠)
  - 旧 `Workspace Pane` 的 absolute 行为作"过渡期"保留(`data-legacy="absolute"` attr)
- **方案 B** :保留 absolute,但把"右列"语义抽到 `RightPaneHost` trait
  - 改动小,但治标不治本 — 新增 body 仍要改 5 处
- **取舍**:R1(不调平台 API) + R2(Leptos 承载 UI)都倾向 A;A 让 1.8 的痛点自然消失

#### 3.1.2 Slot 注册式接口(替代硬编码枚举)

- 新增 `RightPaneSlot` trait(位置:`interfaces/webchat/src/components/right_pane/slot.rs`)
  ```text
  pub trait RightPaneSlot: Send + Sync {
      fn id(&self) -> &'static str;          // 稳定 id,供 deep link 用
      fn label(&self) -> &'static str;       // i18n key,不是字符串
      fn visible(&self, ctx: &SlotCtx) -> bool; // 由 ctx 决定
      fn render(&self, ctx: &SlotCtx) -> impl IntoView;
      fn order(&self) -> u16;                // Tab 顺序
      fn telemetry(&self) -> Option<&'static str>; // "subagent" / "browser" / ...
  }
  ```
- 配套 `RightPaneRegistry`: 启动时 `registry.register(SUBAGENT_TREE_SLOT)` 等
- `WorkspaceBody` 枚举 → 改为 `Vec<Arc<dyn RightPaneSlot>>` + `default_open: &[&str]`
- 新增 body 只需:写 1 个 impl + 1 行 `registry.register`,**不**改其他文件
- 风险:动态注册 → 失去静态枚举的可穷尽匹配;对策:用 `id()` 做 match key + 测试锁住可见集合

#### 3.1.3 Subagent tree 提升为右列默认 body

- 现状:三套 UI,`/dashboard/subagents` 是独立路由
- 建议:
  - Webchat 右列默认 body 改为 `SubagentTreeSlot`
  - `/dashboard/subagents` 保留为 deep-link(打开时若右列未开则自动开 + 切到 subagent)
  - Sidebar live agents 行保留(它显示**当前正在跑的 pty agent**,与 tree 不同语义,见 `sidebar/agent_panel.rs:64-78` 数据源)
  - Chat 内 docked `SubagentNode` 折叠为单行(已存在,见 `chat/messages.rs:1837`)
- 收益:`subagent.tree` 事件流 → 一处写入,三处读取的同步问题消失
- 风险:右列宽度被 subagent tree 占满后,`Artifacts` 不再默认可见;对策:`default_open = [subagent, artifacts]`(左右 Tab 切换可设)

#### 3.1.4 浏览器 — 受控视图(spec-only,本期不动手)

- 现状:`viewer.rs` 是状态壳 + `_blank` 跳转(`viewer.rs:36-47 open_in_new_tab`)
- 本期只做:
  1. 把 `BrowserViewer` 升级为 `RightPaneSlot::Browser`
  2. 在 `RightPaneSlot::Browser::render` 内部**继续**走 `LaunchSection.open` → `_blank`(零行为变化)
  3. 在文档/spec 里写清楚"下一期要做 CDP/iframe 嵌入"的 wire 协议
- **不**本期动:webview tag、CDP、screenshot 流(见 [FEATURE_LOCATOR §3.5d](../reference/FEATURE_LOCATOR.md) 已 deferred)

#### 3.1.5 能力/ProjectionHost 健康卡(零成本补强)

- 新增 `RightPaneSlot::Capabilities`
  - 数据源:在 `src/bin/aleph-server/commands/start/mod.rs` 暴露 `/capability/health` JSON
  - 内容:subscriber count / last revision / invalidated? / desync count
  - 视觉:小卡片 + 颜色(green/yellow/red) + 折叠时只显示一个状态徽章
- 收益:后端 Phase 4 状态可观测;不做 P2/P3 也能提前发现失同步
- 风险:无 — 只读 RPC,无副作用

### 3.2 战术层(具体改动点)

#### 3.2.1 草稿附件升级

- 文件:`interfaces/webchat/src/platform/wide/views/chat/composer/attachments.rs:140-244`
- 现状:仅 `name + ✕`(行 156-178)
- 建议:
  - 加 type-icon 24px(MIME 映射,见 `interfaces/webchat/src/components/artifacts/row.rs` 已有)
  - 缩略图:仅对 `image/*` 且 < 256KB 显示
  - 大小:humanize bytes(`42 KB` / `1.2 MB`)
  - 进度:大文件上传时显示进度条(走现有 reader 事件)
- 风险:缩略图生成会引入 canvas/blobby 工具调用;**仅在用户 hover 时生成**,不在列表上批量算

#### 3.2.2 全局 Attachment Tray(右列可钉)

- 概念:用户可以把"我上传的"或"agent 生成的"附件钉到右列,跨 turn 保留
- 数据:`Arc<RwLock<Vec<PinnedAttachment>>>` 在 `WorkspaceState`(`workspace_panel.rs:138-160` 附近)
- 视觉:与 composer chip 共享 `AttachmentChip` 组件(行 156-178 抽出)
- 收益:R5(AI 主动到达) + R8(工具即一切) 的具体落点
- 风险:跨 turn 持有可能撞 memory ceiling;对策:`pinned_max = 50`,LRU

#### 3.2.3 审批/AskUser 现状保留,加 "Pin to right"

- 现状:inline + banner(`approval_card.rs:60` + `chat/view.rs:286-345`)
- 加"钉到右列"按钮 → 把当前 `ApprovalCard` 复制到右列(同 `RightPaneSlot::Approvals`)
- 收益:用户在编辑代码时也能看审批,**不**丢失上下文

#### 3.2.4 命名收敛(低优先级,新代码先做)

- 现有 4 个"agent*" 文件名(见 1.3 ⚠)重命名会动太多 import;短期在新增代码时:
  - `RightPaneSlot` 命名严格区分 `SubagentTreeSlot`(子代理)vs `RuntimeAgentSlot`(当前 pty)
  - 在 `mod.rs` 顶部注释锁定两份语义
  - 加 `#[deny(unreachable_pub)]` 防止下次再创建相似名

### 3.3 TUI 端(可选,本期不强推)

- 当前:`render.rs:48-58` 水平切分只支持"左 28 + 右 main"
- 建议(仅在用户量起来后做):
  - 加 `RightPaneVisibility` 状态: `Off / SubagentInspector / ToolDetail`
  - `Layout::horizontal([Length(28), Min(40), Percentage(0..40)])` 三列
  - `regions.rs` 的 `RegionKind` 加 `RightPane` 一项(单元测试锁住 priority)
- 风险:TUI 屏幕宽度常常 < 120,加第三列会挤;对策:宽度 < 100 时自动 collapse

---

## 4. 实施路线图(分阶段,每阶段可独立 ship)

### 阶段 0 — 规范与 spec(本周)

- [x] 本文档已落地（`docs/research/2026-10-15-rightside-optimization-plan.md`）
- [x] 已在 `docs/reference/FEATURE_LOCATOR.md` 标注右列抽象为“spec 已落，实施未启动”
- [x] 已在根 `AGENTS.md`（Tier 1）索引本文链接
- [x] 决策点：战略层 3.1.1 选 A 还是 B → 已选 A

### 阶段 1 — 抽出 slot trait(1–2 周)

- 新增 `interfaces/webchat/src/components/right_pane/` 子模块,文件:
  - `slot.rs`(`RightPaneSlot` trait)
  - `registry.rs`(`RightPaneRegistry`)
  - `pane.rs`(`RightPane` 容器,负责 width/z-index/拖拽)
  - `slots/subagent_tree.rs`(把现有 `subagent_tree/mod.rs` 改造为 slot impl)
  - `slots/artifacts.rs`(把现有 `artifacts/mod.rs` 改造为 slot impl)
  - `slots/capabilities.rs`(新增,见 3.1.5)
- 测试:`registry` 注册 → render 顺序、可见性、deep-link 行为
- **不**改 `Workspace Pane` 容器;先并跑(legacy absolute + 新 slot 注册结果同 DOM),观察一致

### 阶段 2 — 替换为 flex 列(2–3 周)

- 改 `app.rs:419-498` 顶层 `<main>` 为 `<main> + <RightPane>` 两段
- 把 `workspace_panel.rs:184-189` 的 absolute 移除,改为 flex item
- 旧 absolute 行为通过 `data-legacy="absolute"` + media query 保留 1 个 release
- 验证:Playwright 截图(已有 `playwright.config.ts`)+ 单元测试宽度 clamp

### 阶段 3 — Subagent tree 升级(1 周)

- 把 `subagent_tree/mod.rs:60-152` 的递归渲染抽为 `SubagentTreeSlot::render`
- `/dashboard/subagents` 路由保留 deep-link:右列未开时自动 `RightPane::open_with("subagent")`
- Sidebar live agents 行不动(见 1.3 三处语义差异)

### 阶段 4 — 能力/ProjectionHost 可见(1 周)

- 后端:`src/bin/aleph-server/commands/start/mod.rs` 暴露 `/capability/health`
  - 通过 `cmd.health` RPC 主题,只在 `ProjectionHost` 已有时挂载
- 前端:`slots/capabilities.rs` —— 卡片 + 状态徽章
- 零业务影响,只读

### 阶段 5 — 附件升级 + Tray(2 周)

- 抽出 `AttachmentChip` 共享组件
- composer `attachments.rs:140-244` 升级
- 新增 `pinned_attachments` 状态 + 右列 `AttachmentTray` body
- 注意不破坏 `MAX_ATTACHMENT_SIZE_BYTES` / `MAX_ATTACHMENT_COUNT`(`attachments.rs:35-37, 43`)

### 阶段 6 — 浏览器受控(spec-only,本期仅占位)

- 文档化 wire 协议(deferred)
- `slots/browser.rs` 走旧 `_blank` 行为,加 TODO 注释
- **不**引入 webview tag

### 阶段 7 — TUI 选配(暂定)

- 视用户量再决定;若 TUI 重度用户 ≥ 30%,启 3.3 改造

---

## 5. 风险与权衡

### 5.1 战略级风险

| 风险 | 触发条件 | 缓解 |
|---|---|---|
| 命名爆炸 | 动态 slot 多了,id/label 冲突 | `id` 强制 `&'static str` 且全局唯一;测试断言 |
| 默认 Tab 选择错误 | 默认 subagent tree 不一定对所有用户合适 | `default_open` 走 `WorkspaceState` user override,本地存储 |
| 浏览器控制面 leak | 受控嵌入引入 XSS / CORS | 推迟到阶段 6,本期仅 spec;实施前走 R10 harness 审核 |

### 5.2 战术级风险

| 风险 | 位置 | 缓解 |
|---|---|---|
| 4 个相似文件名误用 | `agent_panel.rs` / `agents_panel.rs` / `agents_sidebar.rs` / `agents_overlay.rs` | 新增 `RightPaneSlot` 命名严格区分 + mod.rs 顶部语义注释 + `deny(unreachable_pub)` |
| SubagentTree 事件流被三处订阅导致回放 | `subagent.tree` RPC | 已有 `DashboardState::subscribe_events` 单例,新代码只通过它订阅 |
| 缩略图批量生成卡 UI | composer 缩略图 | hover 懒生成 + canvas 缓存 |
| Artifacts body 被默认挤掉 | subagent tree 设为默认 | 默认双开(右列宽度足够时);宽度不够时折叠为 Tab 列表 |
| Pin attachment 撞 memory ceiling | 跨 turn 累计 | LRU + `pinned_max = 50` 强约束 |

### 5.3 红线守住

- R1 — Rust Core 不渲染右列 UI;只暴露 `RightPaneSlotDescriptor`(只读 JSON)
- R2 — 复杂业务 UI 留在 Leptos;Rust 端 slot 实现**不**写视觉
- R3 — 不引入重三方库;`use_drag` / `use_resize` 用现有 `web_sys` + `leptos_use`
- R4 — `interfaces/cli` / `interfaces/tui` 不引入右列抽象的**业务**概念;只透传状态
- R5 — Pin/通知路径不绕过现有 `GlobalBus`
- R6 — 移动端 Panel 暂不强制同步;在文档里标注"桌面端优先"
- R7 — 右列**不**调 LLM 决断;Tab 默认值由 rule 决定
- R8 — `Pin` / `Open in new window` 等操作暴露为 `WorkspaceAction` 工具
- R9 — 智能行为移到 prompt:右列"该不该展开"的判断在会话 prompt 里,不写代码
- R10 — 本次新增 ≤ 4 个组件文件 + 1 个 slot trait(见阶段 1 文件清单)

### 5.4 已知不做(明确边界)

- ❌ 不做受控 browser 嵌入(阶段 6 仅 spec)
- ❌ 不做 TUI 右列(阶段 7,需独立审批)
- ❌ 不动 `Workspace Pane` 的 z-index/动画细节(阶段 2 一次到位)
- ❌ 不合并 `ApprovalCard` 与 `AskUserCard`(语义不同,见 1.6)
- ❌ 不做 AI 自动决定"哪个 Tab 该在前面"(R7 反对)
- ❌ 不在右列里直接调 `stream.chat`(R9 反对)

---

## 6. 验证方法

### 6.1 单元测试

- `RightPaneRegistry`:register 顺序、id 唯一性、visible 在不同 ctx 下的结果
- `RightPane` 容器:width clamp、拖拽边界、折叠态切换
- `AttachmentChip`:缩略图生成 lazy / type-icon 映射 / LRU 行为

### 6.2 视觉回归

- 用 `playwright.config.ts` 已有装置,加 `right_pane/` 截图集
- 每个 slot 一张标准状态 + 一张空状态
- 宽度 clamp 边界(200/320/640/1024 px)

### 6.3 端到端 smoke

- 打开 → 关闭右列(快捷键)
- 切换 Tab
- 拖拽到边界(width 0 / width max)
- Pin 一个附件 → 跨 turn 仍在
- 审批卡片 → 钉到右列 → resolve → 卡片消失
- TUI(若启用):subagent inspector 显示 → 退出

### 6.4 性能预算

- 切换 Tab < 50ms(本地状态,无网络)
- 打开右列 < 100ms(纯布局重排)
- 缩略图 hover 生成 < 200ms(单图)
- `subagent.tree` 事件回放 < 300ms(节点数 ≤ 200)

### 6.5 验收标准(分阶段)

- 阶段 1:slot trait 编译通过;`registry` 单元测试 100% 覆盖;legacy `Workspace Pane` 行为不变
- 阶段 2:截图回归通过;旧 absolute 行为可在配置开关下回退
- 阶段 3:`/dashboard/subagents` 路由仍可达;右列未开时 deep-link 自动开;见 [`2026-10-15-rightpane-stage3-subagent-slot-spec.md`](2026-10-15-rightpane-stage3-subagent-slot-spec.md)
- 阶段 4:`/capability/health` 可观测;subscriber count 与后端一致;见 [`2026-10-15-rightpane-stage4-capabilities-slot-spec.md`](2026-10-15-rightpane-stage4-capabilities-slot-spec.md) — S4-A=A2 次级展开 / S4-B=B1 默认折叠 / S4-C=C1 内联 drawer;**只读面板,零写路径**(EffectClaim producer deferred)
- 阶段 5:草稿 chip 信息密度 ≥ 当前 2×;LRU 测试通过;见 [`2026-10-15-rightpane-stage5-attachments-slot-spec.md`](2026-10-15-rightpane-stage5-attachments-slot-spec.md) — S5-A=A2 icon+缩略图 / S5-B=B1 user-pinned / S5-C=C3 双约束 50 条+7 天;**TUI 端仅接口预留,不实施**(阶段 7 独立审批)
- 阶段 6:`/viewer/browser` standalone 组件可挂右栏;`_blank` 兜底保留;见 [`2026-10-15-rightpane-stage6-controlled-browser-slot-spec.md`](2026-10-15-rightpane-stage6-controlled-browser-slot-spec.md) — S6-A=A2 观察+截断 / S6-B=B1 复用 standalone / S6-C=C1 嵌入失败降级 _blank;**B1 路径不引入新 iframe,Tauri webview 与 postMessage 回写留 v2**

---

## 7. 附录 A — 关键 file:line 速查

### Aleph 现状

| 关注点 | 文件 | 行号 |
|---|---|---|
| 顶层 shell | `interfaces/webchat/src/app.rs` | 419-498 |
| 左栏宽度 14.5rem | `interfaces/webchat/src/components/mode_sidebar.rs` | 134 |
| Workspace Pane(真右栏) | `interfaces/webchat/src/components/workspace_panel.rs` | 184-189 |
| Workspace tab 列表 | `interfaces/webchat/src/components/workspace_panel.rs` | 209-225 |
| Team Task Drawer | `interfaces/webchat/src/components/team_task_strip.rs` | 107 |
| TUI 主布局 | `interfaces/tui/src/tui/render.rs` | 48-69 |
| TUI 左 agent panel 28 列 | `interfaces/tui/src/tui/widgets/agent_panel.rs` | 49 |
| TUI regions 命中 | `interfaces/tui/src/tui/regions.rs` | 24-32, 73-116 |
| Sidebar live agents | `interfaces/webchat/src/components/sidebar/agent_panel.rs` | 全文 |
| Subagent 树视图 | `interfaces/webchat/src/platform/wide/views/subagent_tree/mod.rs` | 60-152 |
| TUI docked agents | `interfaces/tui/src/tui/widgets/agents_panel.rs` | 全文 |
| TUI agents overlay | `interfaces/tui/src/tui/widgets/agents_overlay.rs` | 全文 |
| Browser 状态壳 | `interfaces/webchat/src/platform/wide/views/browser/viewer.rs` | 626-822 |
| Browser 独立页 | `interfaces/webchat/src/platform/wide/views/browser/standalone.rs` | 1-100 |
| 草稿附件管线 | `interfaces/webchat/src/platform/wide/views/chat/composer/attachments.rs` | 35-244 |
| Artifacts 面板 | `interfaces/webchat/src/components/artifacts/mod.rs` | 145-330 |
| 工具行渲染 | `interfaces/webchat/src/components/tool_card.rs` | 全文(1438 行) |
| TUI 工具行 | `interfaces/tui/src/tui/widgets/tool_row.rs` | 全文 |
| 审批卡片 | `interfaces/webchat/src/components/approval_card.rs` | 60 起 |
| ProjectionHost mount | `src/bin/aleph-server/commands/start/mod.rs` | 12-13, 70-71, 268-269 |

### 阶段 1 拟新增文件(预案)

```
interfaces/webchat/src/components/right_pane/
├── mod.rs                  # pub use + 启动时 register 调用
├── slot.rs                 # RightPaneSlot trait
├── registry.rs             # RightPaneRegistry
├── pane.rs                 # RightPane 容器(width/拖拽/折叠)
└── slots/
    ├── mod.rs              # 集中 register
    ├── subagent_tree.rs    # 从 subagent_tree/mod.rs 迁入
    ├── artifacts.rs        # 从 artifacts/mod.rs 迁入
    ├── capabilities.rs     # 新增(阶段 4)
    └── browser.rs          # 占位(阶段 6)
```

合计 7 个新文件,**未**突破 R10(原 harness 12 文件,右列抽象与 harness 解耦)。

---

## 8. 附录 B — 决策日志

| 决策 | 选项 | 选定 | 理由 |
|---|---|---|---|
| 3.1.1 容器方案 | A flex / B 保留 absolute | **A** | 用户批 m00161 |
| 3.1.2 slot 抽象 | trait / enum | **trait** | 用户批 m00161 |
| 3.1.3 subagent 默认 Tab | 是 / 否 | **是** | 用户批 m00161;阶段 2 宽度 clamp 实测调整 |
| 3.1.4 浏览器本期 | 受控 / 状态壳 | 状态壳 | R10 + 风险 |
| 3.1.5 能力面板 | 是 / 否 | 是 | 零成本高价值 |
| S2-A 阶段 2 容器过渡 | `<RightPane />` 转发 `<WorkspacePanel />` / 直接重写 | **转发** | 用户批 m00332 |
| S2-B 阶段 2 缺右栏 | ChatOnly 显式分支 / 旧 absolute 行为回退 | **ChatOnly** | 用户批 m00332 |
| S2-C 阶段 2 迁移 | workspace tool/缩略图 API 仍由 `WorkspacePanel` 直接读 / 切到 RightPane 读 | **`WorkspacePanel` 直接读** | 用户批 m00332 |
| **S3-A** | subagent slot = 默认 Tab (A1) / 按需 (A2) | ✅ **A1** | 用户批 m00482 |
| **S3-B** | live runtime + subagent 1 slot 合并 / 2 slot 分离 | ✅ **2 slot 分离** | 用户批 m00482;glyph 集不同(5 形 vs 4 形) |
| **S3-C** | 详情抽屉内联 / 跳全屏 viewer | ✅ **内联** | 用户批 m00482;`/dashboard/subagents` 保留 deep-link |
| **S4-A** | Capabilities slot 位置 = A2 次级展开 | ✅ **A2** | 用户批 m00491;诊断视图非工作主线,默认折叠 |
| **S4-B** | Diagnostic 计数面板 = B1 默认折叠 | ✅ **B1** | 用户批 m00491;普通用户 0 噪声,开发者 1 键展开 |
| **S4-C** | 详情 drawer = C1 内联 | ✅ **C1** | 用户批 m00491;与 S3-C 一致,避免路由分裂 |
| **S5-A** | Chip 升级范围 = A2 icon + 缩略图 | ✅ **A2** | 用户批 m00617;icon 零成本 + 缩略图次低,v2 再加错误/quota |
| **S5-B** | 全局 tray 范围 = B1 仅 user-pinned | ✅ **B1** | 用户批 m00617;ChatGPT 收件箱风格成熟范式 |
| **S5-C** | LRU = C3 双约束 50 条 + 7 天 | ✅ **C3** | 用户批 m00617;count 硬上限 + time 软上限 |
| **S6-A** | 受控范围 = A2 观察 + 截断 | ✅ **A2** | 用户批 m00685;iframe 模式观察天然有,截断 = revoke;回写 = postMessage 留 v2 |
| **S6-B** | 嵌入路径 = B1 复用 standalone 组件 | ✅ **B1** | 用户批 m00685;零新增依赖,代码复用最大;~110 行 |
| **S6-C** | 兜底策略 = C1 嵌入失败降级 _blank | ✅ **C1** | 用户批 m00685;保留 _blank 路径,行为不退化 |
| 3.3 TUI 右列 | 本期 / 暂缓 | 暂缓 | 用户量未到 |

---

*最后更新:2026-10-15 · 与 `refactor/modularization-p0` 分支状态一致*
*作者上下文:2026-10-15 调研任务交付,本文件不涉及代码改动*
