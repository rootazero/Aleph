> **历史状态 / Historical status — pre-removal exploration/spec**：本文件记录的是移除前的探索与方案。文中描述的 `runtime.agents.*` 与 `agent_panel` surfaces 属于历史内容，不是当前产品契约；当前终端边界请以 [`TERMINAL_RUNTIME.md`](../reference/TERMINAL_RUNTIME.md) 为准。
>
> **Historical status — pre-removal exploration/spec**: This document records a pre-removal exploration and proposal. The `runtime.agents.*` and `agent_panel` surfaces described here are historical and are not current product contracts; use [`TERMINAL_RUNTIME.md`](../reference/TERMINAL_RUNTIME.md) for the current terminal boundary.

# 阶段 3 Spec — 子代理右栏 Slot

> **任务边界**:本文件是**探索 + 方案**,**不**写实际重构代码。落地属于阶段 1+ 实施,待用户独立批准。
>
> **系列位置**:[阶段 0 总览(2026-10-15-rightside-optimization-plan.md)](2026-10-15-rightside-optimization-plan.md) · [阶段 1 Slot 接口(2026-10-15-rightpane-stage1-slot-spec.md)](2026-10-15-rightpane-stage1-slot-spec.md) · [阶段 2 宿主切换(2026-10-15-rightpane-stage2-host-switching-spec.md)](2026-10-15-rightpane-stage2-host-switching-spec.md) · **阶段 3 本文件**

---

## §0 TL;DR

- **Aleph 已经有 1 套** 子代理共享协议(`aleph_protocol::subagent_tree`,5 处 UI 复用、单一真源),但**没有任何一处**把"子代理"放进**右栏** — 最完整的一处挂在独立路由 `/dashboard/subagents`,不在聊天上下文里。
- **TUI 已经做出正确的 2-tier 模型**:`agents_panel` 上 dock 5 行 + "+N more",`agents_overlay` 全屏列表 + 详情。Panel 没有这套。
- **关键澄清**:"**Live runtime agents**"(后台 pty 上正在跑的程序,`runtime.agents.list` 数据源)和 "**Background sub-agents**"(一次性研究任务的子树,`subagent.tree` 数据源)是**两个不同事实** — 当前 5 个 UI 表面里 3 个把它们视觉混淆。右栏必须分开。
- **本阶段核心动作**:把 subagent_tree 收进右栏 slot(默认 Tab,与方案 3.1.3 一致),`/dashboard/subagents` 保留为全屏 deep-link;live agents 单独再开一个 slot,glyph / 标签 / 数据源都要分开。
- **3 个待拍板决策** S3-A / S3-B / S3-C,见 §3.1。

---

## §1 Aleph 现状审计(已读 7 个文件 + 1 共享协议)

### §1.1 共享协议真源 — `aleph_protocol::subagent_tree`

唯一真源,5 个 UI 表面都从这里取数据。文件级:

- `aleph_protocol::subagent_tree` 公开:`SubagentNode`、`TreeNode`、`NodeLifecycle`、`SubagentTreeEvent`、`build_tree()`、`apply_event()`、`summarize()`、`TOPIC` 常量。
- **无 Leptos / 无 ratatui**:纯数据,host-testable,可在 `alephcore` 端到端测试。
- **关键设计**:森林 = 扁平节点列表 + `build_tree` 重建,**所有 UI 复用同一份 forest**,没有"前端各自重建一次"的双重负担(同判据 §10)。

### §1.2 五个 UI 表面 + 一个常被误认的邻居

| # | 路径 | 终端 | 数据源 | 视图 | 备注 |
|---|---|---|---|---|---|
| 1 | `interfaces/webchat/src/platform/wide/views/subagent_tree/mod.rs` | Panel | `subagent.tree` RPC + `run.subagent_tree` 主题 | 树 + 详情抽屉 + 子会话 transcript | 挂在 `/dashboard/subagents`,**独立路由**,非聊天上下文 |
| 2 | `interfaces/webchat/src/components/sidebar/agent_panel.rs` | Panel | `runtime.agents.list` RPC + `runtime.agents.changed` 主题 | 边栏列表(4 状态 + quiet age) | **runtime agents**,**不是** sub-agents |
| 3 | `interfaces/tui/src/tui/widgets/agent_panel.rs` | TUI 左 28 列 | 同 #2 | 4 状态列表(`Loading/Ready/Refused/Unavailable`) | 同 #2 平行面,TUI 渲染版 |
| 4 | `interfaces/tui/src/tui/widgets/agents_panel.rs` | TUI 输入框上方 dock | 同 #1 | `MAX_AGENT_ROWS=5` + `+N more` | **真正的 sub-agents 紧凑体** |
| 5 | `interfaces/tui/src/tui/widgets/agents_overlay.rs` | TUI `/agents` 覆盖层 | 同 #1 | 列表模式 + 详情模式(居中 modal) | 全屏逃生口 |
| (邻居) | `interfaces/webchat/src/components/agents_sidebar.rs` | Panel | Aleph 自身 agent 身份 | 创建 / 选 / 设默认 | **第三个不同事实** — 既不是 runtime 也不是 sub-agent |

> **命名陷阱**(同判据 §17):`agent_panel.rs` / `agents_panel.rs` / `agents_overlay.rs` / `agents_sidebar.rs` 名字差一词,源扫描测试守护不能误伤。

### §1.3 五个表面的关键 UX 差异(决定右栏 slot 该继承哪一套)

| 维度 | 表面 #1(Panel tree) | 表面 #4(TUI dock) | 表面 #5(TUI overlay) |
|---|---|---|---|
| 排序 | Status/Tools/Duration/Depth(`subagent_tree/state.rs:8-13`) | 由 `agent_display_order()`(app/mod.rs) | 同 #4 |
| 过滤 | All/Running/Failed/Leaves(`state.rs:17-22`) | 无 | 无 |
| 节点行 | glyph(●✓✗⌛⊘)+ heat bar + 任务 + 工具数 + token + elapsed + preview + 子孙计数 | spinner + 任务 + 工具数 + token + elapsed + 最后工具/thinking | 同 #4,但居中 |
| 详情抽屉 | 选中节点 → 全 metadata + result preview + child transcript(走 `chat.history` RPC) | 无 | 居中 modal:title + 行体 + 滚动 + hint |
| 状态持久 | `subscribe_events` + `on_cleanup`(canvas idiom) | `AppState.agents` 信号 | 覆盖层 state 在 `state.agents_overlay` |

### §1.4 痛点(为什么要进右栏)

1. **#1 是最完整的视图,但埋得最深**:用户必须离开聊天上下文去 `/dashboard/subagents` 路由,看不到主对话的增量。
2. **Panel 没有 #4/#5 的 2-tier 模式**:Panel 想看子代理,要么去独立路由,要么只看 sidebar 的 runtime agents(根本不是同一回事)。
3. **5 表面里 3 个混淆"runtime agents"与"sub-agents"**:`agents_panel.rs`(TUI)与 `agent_panel.rs`(TUI + Panel)名差一词,数据源也差 — 用户无法第一眼分辨"这是后台 pty 上的 Claude 进程"还是"这是 Claude 跑了 3 个子研究"。
4. **右栏目前最缺的就是它**:WorkspacePanel 5 体(Artifacts/Deliverables/Tasks/Canvas/Browser)**没有**任何一个能容纳 sub-agent 树。Artifiacts 装静态产物,Canvas 装绘图,都不对。

---

## §2 ChatGPT/Codex 公开 UX 观察(摘要 + 引用,不复述原文)

> 调研路径:A — 公开产品/工程文章(已读 5 篇),B — 用户跑 ChatGPT app 截图,C — 网页版对照。后两者待用户触发。
>
> 全部观察以"功能行为"层描述;引用在每条观察末尾,链接见 §2.6。

### §2.1 思考链右栏(Chain-of-Thought sidebar)

**功能行为**:
- ChatGPT 在启用"思考/推理"模型时,把 CoT 步骤**默认按段折叠**在右侧栏,顶部显示**已展开 / 总步骤**的计数器。
- 点击任一段落可展开完整 CoT;计数器帮助回溯与对比。
- 当前为分批灰度推送,**官方未公开计数器设计细节**;不同账号与平台看到的不一致。

**对 Aleph 右栏的启发**:
- 已有"thinking"事件流(`subagent_tree::last_activity = "llm_thinking"`,见 `subagent_tree/mod.rs:84` 的 `describe_activity`),TUI `agents_panel` 也已把 `thinking…` 嵌进 running 行(见 `agents_panel.rs:91-97`)。右栏 slot 可把"thinking"以**可展开的折叠行**形式呈现,顶部加 running / total 计数 — 与 ChatGPT 的折叠 + 计数器范式一致,但实现路径走 Aleph 自己的事件流,不抄样式。
- **不**复制 ChatGPT 的具体交互(侧滑、图标、动画),只学"可展开 + 计数"这个**结构信号**。

**引用**:[Toolnavs: ChatGPT 思考链右栏布局](https://toolnavs.com/article/484-chatgpt-launches-a-new-quotthinking-outputquot-interface-the-sidebar-displays-th) · [Gadgets360: Deep Research 升级](https://www.gadgets360.com/ai/news/openai-chatgpt-deep-research-upgrade-controls-full-screen-viewer-new-features-released-10992577)

### §2.2 Deep Research 5 阶段(澄清 → 计划 → 迭代搜索 → 多格式读取 → 引用综合)

**功能行为**(来自 [PromptLayer 拆解](https://blog.promptlayer.com/how-deep-research-works/)):
1. **澄清阶段**:模型先反问 1-2 轮,把"宽泛请求"收窄到"可执行的研究计划"。
2. **拆解计划**:把任务切成 N 个子主题,优先广域再钻细节。
3. **迭代搜索**:不是一次 Google,而是"搜 → 读 → 改下一搜"循环,会自反"这条路径死了,换条路"。
4. **多格式读取**:同一 agent 可读 HTML / PDF / 图片,数据任务走 code interpreter。
5. **引用综合**:每个论断配 clickable 内联引用,final 报告带"为什么搜这条"的回顾。

**对 Aleph 的启发**(注意 — Aleph 的 sub-agent 是**已经发生的事实**的回放/管理,不是 Deep Research 那种"实时驱动研究"的引擎。能学的是"状态机形状",不能学"驱动模型"):
- **状态机可借鉴**:Aleph 已有 `NodeLifecycle = Running / Completed / Failed / TimedOut / Cancelled`(见 `subagent_tree/mod.rs` 共享协议)。可在右栏加一个**行内进度条**显示"当前阶段 = 拆解 / 搜索 / 读 / 综合" — 但**先**评估 Aleph 子代理是否真有"阶段"这一维数据;如果没有,不要伪造。
- **可借鉴的反模式**:ChatGPT 阶段 1 频繁追问,被部分用户嫌"啰嗦";Aleph 若做追问应该默认关、可一键开。

### §2.3 计划优先闸(Plan-first gating)

**功能行为**(来自 [ByteByteGo 多 agent 对比](https://blog.bytebytego.com/p/how-openai-gemini-and-claude-use)):
- **OpenAI 风格**:interactive clarification — 在跑之前用追问把范围收窄。
- **Gemini 风格**:autonomous plan proposal — 跑之前自动出计划,**展示给用户审**,可增删子任务。
- **共同点**:跑长任务之前,给用户一次"我现在要这么干,你打断吗"的机会。

**对 Aleph 的启发**:
- Aleph 的 sub-agent 已经**事后存在**(`subagent.tree` 是快照),不是"计划闸"的天然场景。**但**:`describe_activity` 已经在解释 running 节点"现在在干什么"(thinking / tool_called / tool_returned)— 已经是事后归因,不是事前闸。
- 阶段 3 不引入 plan-first;若以后要做(对接到"未来要派发"的口子),可以放阶段 5+。

### §2.4 全屏 Report viewer(双栏:左 TOC + 右 sources)

**功能行为**(来自 [The Verge 报道](https://www.theverge.com/ai-artificial-intelligence/876775/openai-deep-research-chatgpt-full-screen-report-viewer)):
- Deep Research 完成后,用户可在**新窗口**打开报告:**左侧 TOC** 可点击跳段,**右侧 sources** 列引用源。
- 支持 Markdown / Word / PDF 下载。
- Plus / Pro 当天推送,Go / Free 之后。

**对 Aleph 右栏的启发**:
- 这是"sub-agent 跑完之后"的事后视图。**Aleph 已经有等价物**:`subagent_tree/mod.rs:204-235` 的 `render_detail` 抽屉,显示 metadata + result preview + child transcript(走 `chat.history` RPC)。
- 阶段 3 的核心动作是把这个**详情抽屉**从 `/dashboard/subagents` 路由搬进**右栏 slot**,行为不变;保留全屏路由作 deep-link。

### §2.5 多 agent 协同(orchestrator + 并行 sub-agents + synthesizer)

**功能行为**(同 ByteByteGo 拆解,Anthropic / OpenAI / Gemini 三家):
- **共同形状**:1 个 lead agent 把任务拆给 N 个并行 sub-agents(常见 5-20 个);sub-agents 各自带 tool set(web search / browse / code);最后由 1 个 synthesizer + 1 个 citation agent 整合。
- **关键差异**:Anthropic 公开过高层架构图;OpenAI 强调 RL 训练;Gemini 强调多模态整合。
- **Aleph 现状对照**:Aleph 的 `subagent_tree` 已经支持**任意深度**的父子关系 + 子孙计数(`subagent_tree/state.rs` 共享 `summarize` 提供 max_depth + total_tools + total_duration_ms)。**形状**已经对得上;**驱动**层面(谁决定派几个 sub-agent)的差异不在 UI 范畴内,本阶段不涉及。

**对 Aleph 的启发**:
- **子代理列表的"max_depth"信息一定要在右栏显示** — Aleph 已经在 `summarize` 算好(`state.rs:104-105` 的 `s.max_depth`),`subagent_tree/mod.rs:268-271` 的 rollup line `d{} · {} agents · ...` 用了,右栏 slot 必须保留这一行。
- 不复制 Anthropic 的高亮配色 — Aleph 已有自己的 `lifecycle_color`(`visuals.rs:11-18`)+ `heat_color`(`visuals.rs:23-29`),用现成的。

### §2.6 引用一览(全部为公开来源,本 spec 未复述原文)

1. [PromptLayer — How Deep Research Works](https://blog.promptlayer.com/how-deep-research-works/) — 5 阶段拆解
2. [Toolnavs — ChatGPT 思考链右栏布局](https://toolnavs.com/article/484-chatgpt-launches-a-new-quotthinking-outputquot-interface-the-sidebar-displays-th) — CoT sidebar + 计数器
3. [ByteByteGo — 多 agent 协同对比](https://blog.bytebytego.com/p/how-openai-gemini-and-claude-use) — 7 家厂商架构对照
4. [The Verge — Deep Research 全屏 viewer](https://www.theverge.com/ai-artificial-intelligence/876775/openai-deep-research-chatgpt-full-screen-report-viewer) — TOC + sources 双栏
5. [Gadgets360 — Deep Research 升级](https://www.gadgets360.com/ai/news/openai-chatgpt-deep-research-upgrade-controls-full-screen-viewer-new-features-released-10992577) — 范围限制 + 计划可编辑

---

## §3 优化建议

### §3.1 战略决策(已批,见决策日志 §8)

#### **S3-A**:右栏子代理 slot 是默认 Tab 还是按需显示?

| 选项 | 描述 | 收益 | 代价 |
|---|---|---|---|
| **A1: 默认 Tab**(推荐) | 与主方案决策 3.1.3 一致(用户已批),用户进入聊天即可看到"这个 session 有 N 个子代理在跑 / 跑完了" | 零点击即"上下文里始终在",对齐 TUI `agents_panel` 的"在 dock 上始终在" | 抢占了一个 Tab 位;对没有子代理的 session 是空状态 |
| A2: 按需显示 | 用户主动点 Tab 条才出现,默认收折 | 不抢 Tab 位 | 失去"始终在"的价值;与方案 3.1.3 矛盾 |

**推荐 S3-A1**。理由:子代理状态就是"这次聊天在做什么"的副产物,不该被埋;空态(没有子代理时)显示一行引导文字。

#### **S3-B**:live runtime agents 与 background sub-agents 是合并 1 slot 还是分 2 slot?

| 选项 | 描述 | 收益 | 代价 |
|---|---|---|---|
| **B1: 分 2 slot**(推荐) | `subagent_slot`(背景子代理,数据源 `subagent.tree`)+ `runtime_slot`(后台 pty 进程,数据源 `runtime.agents.list`) | 视觉上分清"两个不同事实"(判据 §17 — 错标签比缺标签代价更大) | 多一个 Tab 位 |
| B2: 合并 1 slot | 两个数据源按 lifecycle 混合排,glyph 区分 | 节省 Tab 位 | 视觉混淆;无法表达"两个不同含义" |
| B3: 合并 1 slot + 二级 tab | 一个 slot 内再分 sub-tab | 节省一个外层 Tab | 二级 tab 在窄右栏里挤 |

**推荐 S3-B1**。理由:两个数据源的根因、行为、退出条件都不同 — `runtime` 跟着用户开关 pty 走,`subagent` 跟着一句话任务走。混在一起必然后期要做更多的视觉区分,等于"先合并再分离"。

#### **S3-C**:详情抽屉的展开行为(沿用 `chat.history` 拉 child transcript,还是跳全屏 viewer?)

| 选项 | 描述 | 收益 | 代价 |
|---|---|---|---|
| **C1: 抽屉内展开**(推荐) | 与现状一致(子代理 detail drawer 内联 child transcript,`subagent_tree/mod.rs:204-235`) | 改动小,不破坏现状 | 抽屉高度大,右栏会变高 |
| C2: 跳全屏 viewer | 借鉴 ChatGPT 报告 viewer 模式,`/subagents/{id}` 路由全屏 | 抽屉轻,信息密度高 | 多一条路由,深链管理 |

**推荐 S3-C1**。理由:右栏 slot 的设计本意是"上下文里不离开",跳全屏就违背了 slot 价值。后期若用户跑长任务,再加 C2 作为右栏的"open in full view"按钮即可。

### §3.2 战术改动(7 处)

> 全部属于"待用户批准后的阶段 1+ 实施",本 spec 不写实际代码。

- **T3-1** — 在 `RightPaneRegistry` 注册 `subagent_slot`,**默认 Tab**(S3-A1 决定);紧接方案 3.1.3 的"subagent 默认 Tab"。
- **T3-2** — 把 `subagent_tree/mod.rs` 当前的 view 抽成**portable**:去掉 `max-w-4xl` 容器(`subagent_tree/mod.rs:241` 的 `class="px-6 pb-6 aleph-content-top max-w-4xl mx-auto space-y-4"`),接受父级宽度;右栏 slot 会注入自己的 `clamp 宽度`。
- **T3-3** — `subagent_tree/state.rs` 的 sort/filter + `visuals.rs` 的 glyph/heat/sparkline/format 全部**直接复用**(`pub use` + `pub fn`),无修改;右栏 slot 调用方与全屏路由调用方走同一份代码。
- **T3-4** — `render_detail` 抽屉(子代理 metadata + result preview + child transcript via `chat.history` RPC)行为**完全不变**(`subagent_tree/mod.rs:204-235`),只是宿主从 `/dashboard/subagents` 换成右栏 slot。
- **T3-5** — 在右栏 `subagent_slot` 顶部加一行小字**逃生口**:`"Open full view" → /dashboard/subagents` — 保留全屏 deep-link,符合 S3-C1。
- **T3-6** — 若 S3-B1 接受,在 `RightPaneRegistry` **同时**注册 `runtime_slot`,数据源走 `runtime.agents.list` + `runtime.agents.changed` 主题;glyph 集**故意不复用** subagent 的 `lifecycle_glyph` — `runtime` 走 `state_glyph(Working/Idle/Blocked/Unknown)`,`subagent` 走 `lifecycle_glyph(Running/Completed/Failed/TimedOut/Cancelled)`,**两组 5 vs 4** 形状不同,一眼能分。
- **T3-7** — TUI 端右列(阶段 7)与本阶段对齐:TUI 当前已经有 `agents_panel`(dock) + `agents_overlay`(/agents)的 2-tier,**已经做对**,阶段 7 只需要把它收到 TUI 右列(若 TUI 加右列)或保留现状;**不**做 TUI 端的额外改动。runtime agents 在 TUI 走 `agent_panel`(左 28 列),不动。

### §3.3 注意事项(实现时必须满足)

1. **保持共享协议不变** — `aleph_protocol::subagent_tree` 的所有公共 API 一字不动。
2. **保持 host-testable 性质** — `state.rs` / `visuals.rs` 不能因为右栏集成而 import leptos/ratatui(同模块 doc 注释: "No Leptos; host-testable")。
3. **保持订阅纪律** — 右栏 slot 内的 `subscribe_events` 必须 `on_cleanup` 注销(同 `subagent_tree/mod.rs:206-211` 的 canvas idiom);右栏 host 切换(阶段 2 决策 S2-A 已锁)时,子代理 slot 的订阅**随 slot 一起销毁**,不能跨 slot 泄漏。
4. **保持可降级** — 当 subagent tree 没有数据(空 session),右栏 slot 显示**引导态**:`"No sub-agents in this session yet"`,与 TUI overlay 行为一致(`agents_overlay.rs:79-84` 已有 `"(no sub-agents in this session yet)"` 字符串)。
5. **保持 deep-link** — `/dashboard/subagents` 路由**不删**,作为全屏视图与外部跳转目标。

---

## §4 验收矩阵

| 类别 | 测试 | 预期 |
|---|---|---|
| 单元 | `subagent_tree/state.rs` 既有 2 个测试不破 | 绿 |
| 单元 | `subagent_tree/visuals.rs` 既有 4 个测试不破 | 绿 |
| 单元 | `shared_ui_logic::state::agent_panel` 既有 `state_glyph` distinct 测试不破(运行时端 vs 子代理端必须保持两个 glyph 集各自 distinct) | 绿 |
| 集成 | 打开聊天 → 右栏出现 `subagent_slot` 默认 Tab,无子代理时显示引导 | 行为符合 T3-1 + 注意事项 #4 |
| 集成 | 在有 sub-agent 的 session → 右栏 slot 显示 tree,sort/filter 按钮工作,点击节点展开 detail drawer | 行为符合 T3-2 + T3-3 + T3-4 |
| 集成 | detail drawer 显示 child transcript(走 `chat.history` RPC,与现状一致) | 行为符合 T3-4 |
| 集成 | 右栏 slot 顶部 "Open full view" → 跳到 `/dashboard/subagents`,全屏视图渲染与右栏 slot 状态一致(选中的节点不丢) | 行为符合 T3-5 |
| 集成 | 同时有 runtime agent + sub-agent → 右栏两个 slot 并存,glyph / 标签一眼可分 | 行为符合 T3-6(S3-B1 决定) |
| 集成 | 关闭右栏 → 子代理 slot 的 `subscribe_events` 注销,无泄漏 | 符合注意事项 #3 |
| 回归 | 阶段 2 决策的右栏 host 切换 + ChatOnly 模式 → 切换时 slot 列表保持稳定,subagent slot 始终可访问 | 与 S2-A/B/C 决策不冲突 |
| 视觉 | 右栏 subagent slot 在 280-560px 宽度区间内正常渲染,max_depth 行 + sparkline 可见 | 符合 T3-2 portable 化 |
| 视觉 | 窄列(< 360px)时,detail drawer 内的 child transcript 不挤压工具条;长行不破 | clamp 行为对齐 TUI `clamp_chars` 策略 |

---

## §5 风险

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| **R-S3-1**: 5 表面里 3 个混淆 runtime vs sub-agent,右栏分 2 slot(S3-B1)后,用户可能仍下意识混淆 | 中 | 中 | glyph 集不同(5 vs 4)+ slot 标题文字不同 + 共享子标题明"background" / "live" |
| **R-S3-2**: `subagent_tree/mod.rs` 当前假设 `max-w-4xl` 容器,portable 化(T3-2)可能引入新视觉 bug | 中 | 低 | 视觉测试覆盖 280/400/560 三档宽度 |
| **R-S3-3**: heat bar 颜色阈值(0-6 tools/s,见 `visuals.rs:23-29`)在窄列上可能刺眼 | 低 | 低 | 阶段 2 决策的 clamp 范围内测试,过刺眼时改用 lower-saturation 配色 |
| **R-S3-4**: `/dashboard/subagents` 路由与右栏 slot 共享 `SubagentNode` 数据,但 selected 节点状态分离 — 全屏跳右栏或反向跳时丢失选中 | 中 | 中 | 在 `RightPaneSlot::on_full_view_open(&self, node_id: &str)` 钩子(若需要)传 selected 状态;否则明确"跨视图不保持选中" |
| **R-S3-5**: 思考链右栏(§2.1)的"可展开 + 计数器"模式在 Aleph 子代理上需要新事件类型支持,可能拖出协议改动 | 低 | 高 | 阶段 3 **不**做"thinking 行可展开",只做现有节点的树 + 详情抽屉;thinking 展开留阶段 5+ |
| **R-S3-6**: 5 表面里 Panel #1 与 TUI #4/#5 共享 `summarize`(`subagent_tree/state.rs:104-105`)算法,若右栏 slot 用一份新渲染而不复用,会重蹈"两份 twin"的覆辙(判据 §1) | 低 | 中 | T3-3 明确"无修改直接复用" |
| **R-S3-7**: 阶段 1 trait spec 与阶段 2 宿主切换 spec 已经定型,本阶段不触发红线 R1-R10 任何一条 | — | — | 见 §5 检查表 |

**R10 红线检查表**:
- R1 大脑与四肢分离 — 改的是 UI slot 注册,Core 不动,✓
- R2 UI 逻辑唯一源 — 右栏 slot 是 UI,Leptos 渲染,✓
- R3 核心轻量化 — 不引入新依赖,✓
- R4 Interface 层禁止业务逻辑 — slot 注册在 `interfaces/webchat`,✓
- R5 AI 主动到达 — 本阶段不涉及 AI 主动推送,✓
- R6 一核多端 — 共享协议不变,Panel + TUI 都复用,✓
- R7 LLM 主权 — 不替代 LLM 决策,✓
- R8 工具即一切 — slot 注册走 trait + registry,符合"工具即一切",✓
- R9 智慧在 Prompt 中 — 不涉及中间件智慧迁移,✓
- R10 薄 Harness — 本 spec 不动 `src/harness/`,✓

---

## §6 阶段路线(本 spec 范围)

| 阶段 | 名称 | 周期 | 入口 | 出口 |
|---|---|---|---|---|
| S3-0 | spec 落地(本文件) | 1 天 | 用户批 S3-A/B/C | 决策点锁 |
| S3-1 | `subagent_slot` 接入 trait + 注册默认 Tab | 1-2 天 | 阶段 1 spec 的 `RightPaneRegistry` 已合并 | 右栏出现空态引导 |
| S3-2 | 复用 `subagent_tree` 三件套 + portable 化容器 | 2-3 天 | T3-2 / T3-3 / T3-4 | 完整树视图在右栏 |
| S3-3 | 详情抽屉 + child transcript 内联 | 1 天 | 阶段 1 集成测试绿 | detail drawer 行为与全屏一致 |
| S3-4 | "Open full view" 逃生口 + 视觉打磨 | 0.5-1 天 | T3-5 + 验收矩阵全绿 | 全部 7 个战术改动 ship |

**本阶段不发**:
- 思考链行内可展开(§2.1 启发)— R-S3-5 风险高,留阶段 5+
- Plan-first 闸(§2.3 启发)— Aleph 当前没有"未来要派发"的口子,留阶段 5+
- 全屏 viewer 子路由(§2.4 启发)— R-S3-4 风险中,且 C1 已能满足大部分需求
- TUI 端右列(阶段 7)— T3-7 明确不动 TUI

**后续阶段联动**:
- 阶段 4(capabilities)— `capabilities_slot` 与本阶段共享 `RightPaneRegistry` 与 glyph 资源管理
- 阶段 5(capability 事件)— 思考链展开可在此阶段作为新事件类型
- 阶段 7(TUI 右列)— 借鉴本阶段的 portable 容器思路
- 阶段 6(browser)— browser 体在 WorkspacePanel 已有,本阶段不重复

---

## §7 附录 A — file:line 速查表

| 主题 | 路径 | 行 |
|---|---|---|
| 共享协议真源 | `aleph_protocol::subagent_tree` | 全文 |
| Panel subagent tree 视图 | `interfaces/webchat/src/platform/wide/views/subagent_tree/mod.rs` | 全文(360 行) |
| Panel subagent state(sort/filter) | `interfaces/webchat/src/platform/wide/views/subagent_tree/state.rs` | 全文(150 行) |
| Panel subagent visuals(glyph/heat/sparkline/format) | `interfaces/webchat/src/platform/wide/views/subagent_tree/visuals.rs` | 全文(115 行) |
| Panel runtime agents(非本阶段) | `interfaces/webchat/src/components/sidebar/agent_panel.rs` | 全文(430 行) |
| TUI runtime agents(非本阶段) | `interfaces/tui/src/tui/widgets/agent_panel.rs` | 全文(447 行) |
| TUI subagent dock | `interfaces/tui/src/tui/widgets/agents_panel.rs` | 全文(259 行) |
| TUI subagent overlay | `interfaces/tui/src/tui/widgets/agents_overlay.rs` | 全文(186 行) |
| 阶段 0 总览 | `docs/research/2026-10-15-rightside-optimization-plan.md` | 全文 |
| 阶段 1 slot trait spec | `docs/research/2026-10-15-rightpane-stage1-slot-spec.md` | 全文 |
| 阶段 2 宿主切换 spec | `docs/research/2026-10-15-rightpane-stage2-host-switching-spec.md` | 全文 |
| 阶段 3 本文件 | `docs/research/2026-10-15-rightpane-stage3-subagent-slot-spec.md` | 全文 |

## §8 决策日志

| ID | 决策 | 状态 | 触发 |
|---|---|---|---|
| 3.1.1 | 右栏容器 = A(flex 第三列) | ✅ 已批 | m00161 |
| 3.1.2 | slot 抽象 = trait(可注册) | ✅ 已批 | m00161 |
| 3.1.3 | subagent = 默认 Tab(看阶段 2 宽度 clamp 实测) | ✅ 已批 | m00161 |
| S2-A | 阶段 2 容器过渡 = `<RightPane />` 转发 `<WorkspacePanel />` | ✅ 已批 | m00332 |
| S2-B | 阶段 2 缺右栏时 = ChatOnly 显式分支 | ✅ 已批 | m00332 |
| S2-C | 阶段 2 迁移 = 工作区 tool/缩略图 API 仍由 `WorkspacePanel` 直接读 | ✅ 已批 | m00332 |
| **S3-A** | subagent slot 默认 Tab (A1) | ✅ **已批 A1** | m00482 |
| **S3-B** | runtime + subagent 分 2 slot (B1) | ✅ **已批 B1** | m00482 |
| **S3-C** | 详情抽屉内联展开 (C1) | ✅ **已批 C1** | m00482 |

---

*Last updated: 2026-10-15*
