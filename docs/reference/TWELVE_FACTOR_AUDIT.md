# TWELVE_FACTOR_AUDIT.md — 12-Factor Agents 合规审计

> **用途**：Aleph 对 [12-Factor Agents](https://github.com/humanlayer/12-factor-agents) 的逐 factor 合规审计 + 模块改造建议清单。是宪法 `CLAUDE.md` §《🧭 12-Factor 对照与采纳》的**带证据母本**（宪法放蒸馏版，本文放全量）。
> **方法**：逐 factor 对照"已对应 R/P + 代码锚点"，给 ✅（已内化/超越）/ ⚠️（实现存在但宪法缺口或有张力）。锚点随重构漂移，发现不符就地更新。
> **生成**：2026-06-27，依据 `docs/reference/FEATURE_LOCATOR.md` + 源码复核。

## A. 逐 factor 审计

### F1 · Natural Language → Tool Calls — ✅
- 原则：自然语言转结构化工具调用。
- 现状：LLM 发原生 tool_call → harness 分批并/序执行 → 回 ToolResult/ToolError。
- 锚点：`src/harness/agent/act.rs`、`src/tools/scoped/`。已对应 R7/R8/P8。

### F2 · Own Your Prompts — ✅
- 原则：自有提示词，不外包给框架。
- 现状：40+ layer 优先级流水线，分 Basic/Soul/Context/Cached 路径组装。
- 锚点：`src/thinker/prompt_pipeline.rs` + `layers/`。已对应 R9（智慧在 prompt）。

### F3 · Own Your Context Window — ⚠️（→ 采纳 A1）
- 原则：显式拥有 context window 的取舍，不被默认消息格式绑架。
- 现状：Context 层极强——三策略历史压缩、按模型窗口的压缩时机、内容类型路由缩减、FTS5 检索回注、记忆三支柱。**但宪法层无任何命名条目**（R9 仅间接相关）。
- 锚点：`src/context/`（compact/budget/cheap_passes）、`src/tool_output/structured/`（类型路由缩减器，2026-07-30 迁入）、`src/thinker/`。FEATURE_LOCATOR §2.1–2.8、§3.14。
- 缺口：实现 ✅ / 宪法 ✗ → A1 给它正式命名。

### F4 · Tools Are Structured Outputs — ✅
- 原则：工具只是结构化输出，输出与执行解耦。
- 现状：`NativeToolCall` 结构化输出 ↔ `LoopTool` 执行解耦，三层管道（act 并行 / scoped 拦截 / result store 溢出）。
- 锚点：`src/providers/adapter.rs::NativeToolCall`、`src/tools/runtime.rs::LoopTool`、`src/harness/agent/act.rs`。已对应 R8。

### F5 · Unify Execution & Business State — ⚠️（→ 采纳 A3，与 F12 合并）
- 原则：执行状态与业务状态统一、可从单一源（理想为 context window）推断。
- 现状：harness `TurnState` 为逐轮执行态；业务态分散——`looping/` 纯内存（重启即丢）、`goal/` 持久、`tasks/store/` 持久。**无单一 event-log 状态源**。
- 锚点：`src/harness/trait_def.rs::TurnState`、`src/looping/`、`src/goal/store.rs`、`src/agents/swarm/tasks/store/`。
- 缺口：架构性，最深。落地需评估（B §P2-1），预判很可能 YAGNI。

### F6 · Launch / Pause / Resume — ⚠️（→ 采纳 A4）
- 原则：用简单 API 启动/暂停/恢复 agent。
- 现状：机件齐全但分散——取消、续跑、改需求三态打断/注入、workflow resume 各自独立，**未作为一组契约命名**。
- 锚点：`src/gateway/cancellation.rs`、`src/gateway/resume_coordinator.rs`、`src/gateway/execution_engine/steering.rs`、`src/workflow/`。已对应 R5/R6。
- **进展（2026-07-25）**：两条**自治续跑链**已补齐 pause/resume 并跨会话可控——loop 新增 `LoopStatus::Paused` + 唯一原子迁移原语 `LoopRegistry::transition`（goal 的 `GoalStatus::Paused` 早有，本轮补可逆孪生 `GoalStore::pause_if_active`）；`loop(stop|pause, session=…)` / `goal(clear|update status=paused, session=…)` 与 `stop_all` / `pause_all` 杀手闸闭合了"`list` 跨会话可见、只能在本会话停"的 R6/R8 断线。新增可复用边界规则：**跨会话只能 quiet，不能 arm**（下一步只由该单元自己会话的完成钩子认领），跨会话操作带工具内 operator 闸。锚点：`src/looping/mod.rs`、`src/goal/store.rs`、`src/builtin_tools/{loop_manage,goal}.rs`；详见 FEATURE_LOCATOR §4.1/§4.2。
- 剩余缺口：workflow / team task 两类长跑单元尚未纳入同一组命名；统一 API 面（薄 facade）仍待评估（B §P1-1）。

### F7 · Contact Humans With Tool Calls — ✅
- 原则：人在环 = 结构化工具调用，而非特例分支。
- 现状：`ask_user` 工具 + clarification 闸 + 三级 approval。
- 锚点：`src/clarification/`、`src/builtin_tools/ask_user.rs`、`src/approval/`。已对应 R5。

### F8 · Own Your Control Flow — ✅ 典范
- 原则：自有控制流，自定义循环/中断/续跑。
- 现状：薄 harness 笨循环（Think→Act），R10 哲学高度自觉；多维度已**超越** hermes/openclaw/pi（资源域分群并行、保序、失败去重、有界恢复）。
- 锚点：`src/harness/agent/{think,act}.rs`。已对应 R10。

### F9 · Compact Errors Into Context — ⚠️（→ 采纳 A2）
- 原则：把错误压缩进 context，让模型自愈。
- 现状：**采纳侧**——错误经 §2.7 内容路由缩减、ToolError 事件、`think.rs` 有界 provider-failure drain 进 context。**禁止侧**——R10 第 5 不（不做确定性错误恢复策略选择 / hermes 式重试矩阵，有意不移植）。**这两件事的边界没有任何地方写清**，R10 易被误读成"别把错误给模型看"。
- 锚点：`src/tool_output/structured/log.rs`、`src/tool_output/hygiene.rs`（ingress 清洗，2026-07-30）、`src/harness/agent/think.rs`。
- 缺口：纯澄清，零代码（C §读法说明）。

### F10 · Small, Focused Agents — ✅
- 原则：小而专的 agent，而非巨型全能体。
- 现状：teams 多代理 + subagent spawner，3 道防风暴闸。
- 锚点：`src/teams/`、`src/agents/`。已对应 R3。

### F11 · Trigger From Anywhere — ✅ 核心招牌
- 原则：从任何渠道触发，在用户所在处相遇。
- 现状：多端通道（Telegram/Slack/Email/桌面/Panel）+ cron + daemon 事件。
- 锚点：`src/gateway/`、`src/tasks/cron/`。已对应 R5/R6。

### F12 · Stateless Reducer — ⚠️（→ 采纳 A3，与 F5 合并）
- 原则：agent = 对(状态, 事件)的纯 reduce。
- 现状：`prompt.rs` 逐轮重建裸消息 + 增量摘要继承 = reducer-like，但轮内 `TurnState`/有界恢复带状态、loop 纯内存，**非纯 reducer**。
- 锚点：`src/harness/agent/prompt.rs`、`src/context/compact`。
- 缺口：与 F5 同源，合并入 A3。

### F13 · (附录) Pre-fetch Context — ✅（仅备注，不立条款）
- 原则：预取可能需要的 context。
- 现状：assembler 主动召回记忆/上下文，已具备。
- 锚点：`src/memory/assembler/`、`src/context/retrieval/`、memory 召回链。

## B. 模块改造建议清单

> 分级：**P0** 文档级零代码风险；**P1** 轻代码独立不碰 harness；**P2** 架构性需评估。每项注明锚点、性质、为何不在本轮做。代码项各自成后续独立 spec/plan，**不在本轮**。

### P0（文档级）
- **P0-1（A1/F3）**：本文 §A-F3 已写清 Context 层叙事与锚点（引 FEATURE_LOCATOR §2.x）。性质：纯描述，本轮完成。
- **P0-2（A2/F9）**：本文 §C 写"错误压缩 vs 恢复"读法说明。性质：纯描述，本轮完成。（可选：FEATURE_LOCATOR §3.1 R10 注解旁加一行链到 A2——范围外顺带优化，不强制。）

### P1（轻代码，后续 plan）
- **P1-1（A4/F6）**：盘点 `cancellation`/`resume_coordinator`/`steering`/`workflow` resume 现有入口，评估是否值一层**薄 facade/trait** 统一命名"生命周期契约"。约束：facade 落 gateway/loop 层，**绝不进 `src/harness/`**（R10）。先产出"现状映射 + 是否需 facade"判断再决定实现。
- **P1-2（F13 pre-fetch，可选）**：F13 pre-fetch 在 §A 标"已满足"即可，无需代码。

### P2（架构性，仅 backlog，本轮不动）
- **P2-1（A3/F5+F12）**：评估"执行状态单一可重建源"。现状 loop 纯内存（重启即丢，设计意图）、goal/task 各自持久、harness 逐轮重建。**必须先过"加代码前必答 3 问"**（脚手架 vs 认知 / 模型升级后是否还需 / 真实消费者几个）。**预判**：很可能 YAGNI——现状"逐轮重建裸消息 + 持久 session + 增量摘要继承"已覆盖多数 reducer 收益；除非出现真实跨重启执行恢复需求，否则不实现。

## C. 与红线的关系

- 采纳条款 A1–A4 是**叠加在 R/P 之上的工程承诺**，**不是新红线**——它们交叉引用既有 R/P，不新增"违反不得合入"效力。
- **F9 / R10 读法说明（关键，消除误读）**：R10 第 5 不「不做错误恢复策略选择」约束的是**确定性 harness 替模型挑恢复策略 / 多级重试矩阵**；它**不**禁止"把错误压缩并呈递给模型让模型自己决定下一步"。换言之——**让模型看见并自愈错误 = 要（A2 采纳）；让 harness 替模型挑恢复策略 = 不要（R10 仍禁）**。`think.rs` 的 empty/ctx-overflow/max_tokens 有界 drain 是 provider-failure 的幂等恢复，不是策略选择，故合规。
- A3 的硬约束：任何后续实现**不得**让 `src/harness/` 越过 R10 的 12 文件 / `src/harness/tests/budget.rs` 行数棘轮（当前 5043；旧 ~4900 系测量事故残值，已退休）。
