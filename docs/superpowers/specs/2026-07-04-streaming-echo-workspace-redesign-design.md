# 流式回显与工作区面板重构 (Streaming Echo & Workspace Panel Redesign)

- **日期**: 2026-07-04
- **状态**: 已确认（brainstorming 四节逐节批准）
- **对应**: FEATURE_LOCATOR §6.1
- **性质**: 深度架构重构 + 功能增强 + 细节打磨 + 错误修复连线

## 1. 问题与诊断

**用户痛点**：流式回显不友好——缺少描述性语言，只见大量工具调用；中间过程的方框（StepStrip + ToolCard 卡片）占用对话窗口空间。

**调查诊断**（2026-07-04，两路并行 Explore 查证）：

- **prompt 层叙述指令早已存在且完备**：`src/thinker/layers/multi_step_conduct.rs` 有专门 "Narrate Your Progress" 段（要求行动前发 8-12 词 preamble）；`src/thinker/layers/guidelines.rs` rule 14 同样要求。非病灶。
- **流式管线健康**：中间轮 assistant 文本逐 token 经 `ResponseChunk` 推送（`src/harness/agent/think.rs` on_delta → `src/gateway/execution_engine/event_drain.rs`），每轮结束再发权威 `text_emitted`（`AgentTrace`）。非病灶。
- **真正病灶在前端呈现层**：`interfaces/webchat/src/platform/wide/views/chat/timeline.rs` 的 `StepStrip` 在 run 完成后把整段过程（**含模型写的叙述**）自动折叠成一行摘要——描述性语言到了前端却被视觉隐藏。展开后每个工具一张带边框/padding/展开体的 ToolCard 卡片，视觉重、占空间。

**参考项目可迁移模式**（codex / hermes-agent）：

| 模式 | 来源 | 采纳决定 |
|------|------|---------|
| preamble 按逻辑分组叙述、trivial 单读免叙述、衔接前文 | codex `prompt_with_apply_patch_instructions.md:31-50` | ✅ 微调进现有 prompt 段 |
| 只读工具（Read/List/Search）实时塌缩成 `Exploring` 聚合块，连续 Read 合并去重 | codex `tui/src/exec_cell/{model,render}.rs` | ✅ 采纳 |
| 状态动词切换 + 彩色圆点 + spinner | codex `exec_cell/render.rs` | ✅ 融入工具行 |
| 长静默兑底（>8s 定时提示，限频限量） | hermes `useLongRunToolCharms.ts` | ✅ 采纳，但用真实信息（工具名+实时耗时），不用随机俏皮话 |
| 客户端合成叙述（禁模型 preamble） | hermes | ❌ 与 R9 相反，不采纳 |

## 2. 已确认的设计决策

1. **叙述来源**：prompt + UI 双管齐下（模型主动叙述 = R9 智慧在 prompt；前端负责不弄丢它）。
2. **目标形态**：叙述为主线，工具降为单行（claude-code 风格 transcript）。
3. **完成后去留**：叙述常驻对话流永不折叠；工具条目收敛（探索块折叠成一行摘要，动作行本身单行常驻）。
4. **工作区新定位**：详情查看器——不再重复时间线叙事；聊天管叙事，工作区管深度，职责互补零重叠。
5. **实施方案**：A · 渲染层重构（数据链路不动，集中重写前端派生与渲染 + prompt 微调）。

## 3. 总体架构

**不动的部分**（数据链路已证实健康）：

- 后端事件管线：`AgentTrace`（`turn_started` / `text_emitted` / `tool_call_*`）+ `ResponseChunk` 逐 token 流式。
- `events.rs` 投影逻辑：`apply_trace_event` / `subscribe_run_events` 不变。
- `ChatState` 消息模型：中间轮仍是 `intermediate-{run}-{n}` 带 `iteration` 的 `ChatMessage`，`tool_calls` 挂在消息上。
- `WorkspaceState.tool_payloads`（args/result 捕获）与 `expanded_events`（展开覆盖集，防 keyed-`<For>` remount 重置）机制保留。

**重写的部分**：

| 文件 | 改动 |
|------|------|
| `views/chat/timeline.rs` | `derive_timeline` 重写：输出新行模型，删除 `StepStrip` 行类型。纯函数，host 可单测 |
| `views/chat/messages.rs` | 删除 `StepStrip` 组件及配套（`latest_step_tool` / `step_narration_head`），新增叙述行/工具行/探索块渲染 |
| `components/tool_card.rs` | ToolCard 瘦身为无边框单行条目（复用现有 `ToolKind` / `tool_headline` / `tool_icon`） |
| `components/workspace_panel.rs` | `ActivityTimeline` + `StepCard` 删除，换 `ToolDetailView` |
| `state/layout.rs` | `WorkspaceState` 增 `selected_tool`，退役 `focused_step` / `current_iteration` 的 step 粒度交叉高亮 |
| `src/thinker/layers/multi_step_conduct.rs` | prompt 段内微调（三条 codex 规则） |

依赖方向不变：`事件 → ChatState/WorkspaceState → 派生（纯函数）→ 渲染`。改动全在 Panel（Leptos/WASM）+ prompt 一处，不碰 `src/harness/`，无 R10 压力。

## 4. 聊天列新形态

派生后的行类型（替换 `TimelineRow::StepStrip`）：

### ① 叙述行（Narration）

中间轮 assistant 文本，无框直排，流式打字机照常（复用 `TypewriterRenderer`），颜色比最终回答略淡（`text-secondary`）。**运行结束后常驻不折叠**。

### ② 工具行（ToolLine）

一个非只读工具一行：

```
▸ ✏️ 编辑 src/config/mod.rs   +12 -3   ✓        （完成，绿✓）
▸ ⚙️ 运行 cargo check          12s ●          （运行中，脉冲点+实时耗时）
▸ ⚙️ 运行 cargo test           ✗ exit 1        （失败，红✗）
```

- 单行 = 折叠箭头 + 图标 + 动词化 headline + diff 统计/耗时 + 状态符。
- 点击展开：内联详情体（沿用 8 行封顶 `MAX_INLINE_LINES` + "+N → 详情栏"溢出行），展开态继续存 `expanded_events`。
- **长静默兑底**：运行超 8s 的工具行显示实时递增耗时（共享 1s ticker，仅 live 行订阅，避免全列表重渲染）。

### ③ 探索聚合块（ExploreGroup）

连续的只读工具（`FileRead` / `Search`，含列目录类）实时塌缩：

```
运行中： ▾ 🔍 探索中… 4 项
            读取 src/config/mod.rs, types.rs
            搜索 "provider_config"
完成后： ▸ ✓ 探索了 4 项（读取×3 · 搜索×1）      ← 自动折叠成一行，点击展开
```

- 聚合边界与 codex 同：遇到非只读工具**或叙述文本**即 flush 开新块。
- 连续 Read 合并去重到一行（文件名 `, ` 连接）。
- 展开态按块 key 存共享状态（顶替现 `strip_open` 的角色），扛 keyed-`<For>` 每 token 重挂载。

### ④ 最终回答

保持现有气泡（`msg-glass`），与过程流形成视觉锚点。用户消息气泡不动。

## 5. 工作区面板改造（详情查看器）

**删除**：`ActivityTimeline` + `StepCard`（与聊天列叙述完全重叠的时间线叙事）。

**新增 `ToolDetailView`**——右栏主体变成当前选中工具的完整详情：

- 头部：图标 + headline + 状态/耗时 + 所属 run/iteration。
- 主体：完整 args（格式化 JSON/命令）、完整 result（**不受 8 行封顶**）、文件类工具渲染 diff/内容预览。复用 `tool_payloads` 现有数据，零新增后端调用。

**选中态**：`WorkspaceState` 新增 `selected_tool: Option<(run_id, tool_id)>`，顶替 `focused_step` 的角色（`focused_step` / `current_iteration` 的 step 粒度交叉高亮随 StepCard 一起退役，简化状态面）。

**联动**：

- 聊天列工具行的 "→ 详情" 溢出行 / 详情按钮 → `reveal_tool` 改写为：设 `selected_tool` + 开 Split。
- **直播跟随**：run 进行中且用户未手动选中时，详情面自动跟随最新开始的工具（工作台感，R5 主动到达）；用户点选任一工具即"钉住"，run 结束解除跟随。

**保留**：`FilesDrawer`（底部文件树抽屉）、团队模式双 tab（交付物/任务）、`unseen_activity` 红点徽章、Split/ChatOnly 切换与浮层动画。

## 6. Prompt 微调

`src/thinker/layers/multi_step_conduct.rs` 现有 "Narrate Your Progress" 段内微调（**不加新 layer**），吸收 codex 三条：

1. 叙述按逻辑分组——一句覆盖一批相关动作，别每个工具一句。
2. trivial 单次读取免叙述——配合前端探索聚合，双端一致降噪。
3. 叙述要衔接前文制造推进感（"Found the config, now wiring the new field…"）。

门控保持不变（`PromptMode::Full` 且非 `Capability::SilentReply`）。

## 7. 错误修复与功能连线清单

本次已知清单（实施中发现的就地补）：

1. **核心修复**：StepStrip 完成即折叠吞掉叙述（本设计根治）。
2. `messages.rs` 的 `latest_step_tool` / `step_narration_head` 及 StepStrip 私有逻辑随组件删除（清理死代码）。
3. `strip_open` 状态改造为探索块展开态，防 remount 重置的既有契约保持（沿用 `ChatState` 存放——`strip_open` 现居于此，改 key 语义为探索块 key 即可；不进卡内本地信号——FEATURE_LOCATOR §6.1 打磨话术的既有约束）。
4. `reveal_tool` / `focused_step` 链路重接到 `selected_tool`（含 `#step-{run}-{it}` 滚动定位效果的清理或重接）。
5. i18n：新增 探索中 / 探索了 / N 项 / 运行中耗时 等 zh + en 词条。

## 8. 测试策略

- **派生纯函数 host 单测**：探索聚合边界（只读连续 / 被写操作打断 / 被叙述打断）、连续 Read 合并去重、完成态折叠、行 key 稳定性。
- **`WorkspaceState` 新状态单测**：`selected_tool`、跟随/钉住语义、`reset` 清理。
- **既有测试迁移**：`timeline.rs` / `workspace_panel.rs` / `layout.rs` 受影响单测同步改写。
- **视觉验证**：Puppeteer headless 走查（全局 memory 有现成 recipe：`reference_panel_testing`）。
- **嵌入链提醒**：Panel 改动需 `just wasm` → 重编 server binary 才可见（rust_embed 编译期嵌入，见 CLAUDE.md）。

## 9. 明确不做（YAGNI）

- 后端新增 narration 事件类型 / 前端全新 transcript entry 状态模型（方案 B，数据链路本就通）。
- hermes 式客户端合成叙述与随机俏皮话 charms（与 R9 相反 / 不专业）。
- codex 新版 `update_plan` 式结构化进度替代散文叙述（Aleph 已有独立 Todo/scratchpad 面板，职责已分离）。
- 工具行虚拟滚动 / 动作行超量二次聚合（单行成本低，先不做）。
