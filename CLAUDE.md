# CLAUDE.md

## 🛑 架构红线 (Architectural Redlines)

以下为最高优先级约束，所有开发决策必须遵守。违反红线的代码不得合入。

### R1. 大脑与四肢绝对分离 (Brain-Limb Separation)

- **禁令**: 严禁在 `src` 中直接调用特定平台系统 API (AppKit, Vision, CoreGraphics, windows-rs)
- **原则**: 核心层只定义"能力契约 (Trait)"，物理实现由原生 Bridge (Swift / 其他) 通过 IPC 提供
- **例外·进程隔离内核 (Process-Isolation Kernel)**: 沙箱的 restricted-token / job-object / AppContainer / 完整性级别 / SID·ACL 系统调用**必须由 spawn 子进程的父进程在 spawn 时就地发起**——无法经 IPC 桥委托（你没法从一个独立 helper 进程去沙箱化另一个进程；spawn-time 经 IPC 往返施加 restricted token 会削弱安全模型）。本地 PID 存活探测（`OpenProcess`/`GetExitCodeProcess`）同理。故 `src/sandbox/*` 与 `src/builtin_tools/desktop/session_lock.rs` 的平台 FFI（`windows-sys`，Cargo.toml `[target.'cfg(target_os="windows")']` 门控、代码 `#[cfg(windows)]` 门控）是 R1 **立意之外的合法开口**，非违规——R1 立意针对的是桌面 UI / 屏幕 / Vision **四肢**（这些仍强制走原生 Bridge：macOS 侧 `src/` 零 `cocoa`/`objc`/`core-graphics` 直接用法即铁证）。安全内核不是"桥能提供的 capability"。

### R2. UI 逻辑唯一源 (Single Source of UI Truth)

- **禁令**: 严禁在原生 Bridge 中实现具有业务逻辑的设置页面、表单或列表
- **原则**: 所有复杂业务 UI 在 Leptos (WASM) Panel 中实现，原生 Bridge 仅负责系统 API 调用与桥接

### R3. 核心轻量化 (Core Minimalism)

- **禁令**: 严禁为单一非核心功能在 core 中引入沉重的第三方库
- **原则**: 优先实现为 Skill (Python/Bash) 或 MCP Server。内核只调度，不搬砖
- **备注**: 代码层面的奥卡姆剃刀原则和 Rust 大文件拆分规范与此不冲突

### R4. Interface 层禁止业务逻辑 (I/O-Only Interfaces)

- **禁令**: 禁止在 Channel/Bot/CLI/Panel 中处理数据持久化、记忆检索或任务规划逻辑
- **原则**: Interface 层是"纯 I/O"— 输入转为 JSON-RPC 发给 Server，响应渲染给用户

### R5. AI 主动到达 (AI Comes to You)

- **原则**: 减少用户切换上下文的成本，AI 通过用户已有的工作通道主动提供帮助
- **实现**: 多端推送（Telegram / Slack / Email / 桌面通知 / Panel 浮窗等）、内联建议、订阅式 Daemon 触发
- **边界**: 不打扰用户 (不抢焦点、不弹模态对话框)，但不要因此拒绝必要的交互入口

### R6. 一核多端 (One Core, Many Channels)

- **形态**: Aleph 是常驻后台服务（aleph-server），UI 不是必需品；所有终端用户体验都由 Channel/Panel 通过 JSON-RPC 与 Core 对话产生
- **原则**: Rust Core 是唯一大脑，多端通道（CLI / Bot / WebChat Panel / 原生 Bridge）只负责 I/O 与渲染，不参与业务推理
- **备注**: 这已在 R1、R2、R4 中体现，此条作为产品层面的重申

### R7. LLM 主权原则 (LLM Sovereignty)

- **禁令**: 严禁用确定性代码替代 LLM 擅长的推理判断（意图识别、任务评估、路由决策、内容分类等）
- **原则**: 极简系统 + 强大 prompt = 释放 LLM 全部推理能力。把复杂留给模型，把简单留给系统
- **判断标准**: 对每个模块问 — "这是在赋能 LLM（给它做不到的能力），还是在越俎代庖（替它做推理）？"
- **赋能层（保留）**: Gateway 多端触达、Memory 持久记忆、Daemon 事件感知、Soul 人格、Provider 多厂商、Tool 执行能力、MCP 外部服务、Extension 插件生态、上下文压缩、安全硬过滤
- **越俎代庖（禁止）**: Intent Detection 规则引擎、POE 目标验证管线、多层 Tool Filter、Context Aggregation 多层合并、Dispatcher 意图分析

### R8. 工具即一切 (Everything is a Tool)

- **原则**: Aleph 自身的所有可配置操作都应暴露为工具，让 LLM 通过自然语言对话完成配置
- **实现**: Agent 管理（创建/切换/删除）、Provider 配置、Channel 配置、Skill/MCP 安装卸载、Daemon 订阅规则 — 全部是 Tool
- **核心循环**: `用户自然语言 → LLM 理解意图 → LLM 选择工具 → 工具执行 → 结果返回 LLM → LLM 回复用户`
- **效果**: 对话即管理面板。用户无需学习配置文件或 API，自然语言驱动一切

### R9. 智慧在 Prompt 中 (Intelligence Lives in the Prompt)

- **原则**: 被移除的中间件的"智慧"不是丢弃，而是迁移到 system prompt 模板中
- **实现**: 主循环的 LLM 一次调用自然覆盖所有判断（意图理解 + 工具选择 + 安全评估 + 完成度判断）
- **效果**: 零额外 LLM 调用，零中间件税，模型推理能力完整释放
- **精简原则 (prune-the-prompt)**: 智慧迁进 prompt ≠ 把 prompt 写厚。模型越强越需要**更少方向/约束/示例**——few-shot 会变笼子（模型模仿范例而非理解问题空间），低密度冗余稀释注意力；新模型发布后第一件事是**修剪上下文**，把验证/自我纠错建进**架构（运行时信号）**而非 prompt。加 prompt 字节前过**两把尺**：① 这是模型**做不到的运行时事实**，还是我在**教强模型怎么思考**？后者别进 prompt。② **这句话有没有一个工具拥有它**？有 → 写进那个工具的 `DESCRIPTION`（随 schema 发，且只发给真能调它的请求），别写进 system prompt——system prompt 只承载**没有任何单个工具能说出口的东西**（跨工具取舍 / 运行时事实 / 安全边界）。第二把尺是 pi 之镜（2026-07-26）：第一把量不出"完全正确、完全必要、只是放错地方"的重复。**两把尺都已建进架构**：`src/thinker/prompt_contract.rs` 的 `reachable_layers`（层必须能开口，否则带理由进白名单）/ `scaffold_bytes_ratchet`（实测骨架天花板，只减不增）/ `no_sentence_is_stated_twice`；量一下用 `aleph-server prompt-size`。**⚠️ 第二把尺有一个前置条件，2026-08-02 前它不成立**：「写进那个工具的 `DESCRIPTION`」只有在该工具的 `BUILTIN_TOOL_DEFINITIONS` 条目**指向常量**时才真的随 schema 发出；写成手写字面量就把常量整体遮蔽（详见下方「能力接上了 ≠ 模型会用它」的前置条件段）。`no_sentence_is_stated_twice` 此前也只量 layer、看不见工具描述——**尺子量不到的地方，搬过去等于删掉**。现该守卫已同时摄入注册工具的 DESCRIPTION，非空性有断言兜底。详见 [HARNESS_PHILOSOPHY.md §8](docs/reference/HARNESS_PHILOSOPHY.md) 与 FEATURE_LOCATOR §1.1

### R10. 薄 Harness 哲学，笨循环编排核心 (Thin Harness, Dumb Loop)

> *"If you're not the model, you're the harness."* — Vivek Trivedy
> *"Models get stronger → harness gets thinner."* — Anthropic

- **薄 Harness 哲学 (Thin Harness)**: Aleph 采纳 Anthropic 流派，运行时极简、信任模型。Harness 是脚手架不是认知层。**模型越强，Harness 越薄** — 优秀的 Harness 必须通过"面向未来测试 (Future-Proof Test)"：换更强的模型，性能自然提升，无需修改 Harness 代码
- **笨循环 (Dumb Loop)**: `src/harness/` 仅承载 Think→Act 轮次调度，**不参与任何推理**。所有智能决策（意图理解、工具选择、安全评估、完成度判断）由 LLM 一次推理调用自然完成
- **核心边界**: `src/harness/` 锁 **12 文件**；行数由 `src/harness/tests/budget.rs` 的棘轮守（实测非手算、只减不增、增必答下方 3 问），当前 **5146 行**（**2026-08-04 Tool Calling 2.0 +4**：provider 的空/重复 `call_id` 在 assistant event 持久化前 fail-closed，防止 prompt/resume/UI/in-flight 对同一关联键产生互相冲突的终态；三问作答在 `src/harness/CLAUDE.md`。**2026-08-03 Round 10 +33**：保护尾只数持久化消息；**2026-08-03 Round 9 +25**：付清 §2.18 follow-up 账本第 3、7 项——`prompt.rs` 的 orphan 前向扫描收敛到本 turn（后续 turn 复用的 call id 曾能回头改写一条早已缓存的 assistant 消息），`think.rs` 的边界宽限轮保留 tools 数组改用 `ToolChoice::None`（`tools: None` 与 Anthropic 的 tools→system→messages 前缀零共享，它自己注释里那句"变成缓存命中"从来不成立）；⚠️ 账本原写的正解只改 `tool_choice` **不够**，Anthropic adapter 对 `ToolChoice::None` 的实现就是删掉 tools 数组、同一个 wire 形状，已同批修，3 问作答在 `tests/budget.rs::CEILING`。**2026-08-02 Round 8 +22**：组边界查 `run_cancel` 不再让 `/stop` 之后的分组发出幽灵 `ToolError`，并行时钟改在首次 poll 起表而非批次准入，3 问作答在 `tests/budget.rs::CEILING`。**2026-07-30 −4**：Layer-3 turn spill 改用 `result_processing::recovery_footer`，offload 之外补上 index 与 `ctx_search` 提示，两个调用点收进一个闭包顺带付清；同批删掉写单向的 `ToolOutputMetadata.truncated` 写入。下降不需答 3 问。**2026-07-29 Round 7**：`5055 → 5066（+11）`——`guardrails.rs` 的 `Block` 臂补上 `push_tool_invocation`（被拦工具调用此前**缺席权威终态** `tool_timeline`→`RunSummary.tool_summaries`，而 `agent_trace` 是有意有损的镜像，于是它是唯一没有兜底的一类）+13、`trace.rs` 删零生产者的 `LoopTraceTurnOutcome::{HitLimit,Cancelled}` −2，3 问作答在 `tests/budget.rs::CEILING`；**2026-07-26 欠账已清**：`5008 → 5082（+74）` 的 3 问作答补在 `tests/budget.rs::CEILING`；+79 实为前一笔 `c648b5ea4`，前三项全过、第四项"每批 `canonical`/`claims` 穿线"的 `Option::None` 重算臂**零消费者**（唯一传 `None` 的快路径正好撞上 `can_parallel_dispatch` 先行 `return false` 的同一条件），按"零消费者立即撤回"已撤 → **5082 → 5055（−27）**，行为按构造不变。**2026-07-25 文档订正**：本文与 `src/harness/CLAUDE.md` 此前写 5008，而代码里的 `CEILING` 早在 `396c6d200` 就抬到 5082——正是 budget.rs 开头那段"手写状态行会撒谎"要防的事，在文档层复发了一次。**代码是权威**，任何文档数字都只是 `CEILING` 的副本；2026-07-20 −62＝移除 `DiminishingReturnsDetector` 硬停 [R10 5-不 #3：loop 不做完成度判断]，`think.rs` 弃 `after_turn` 消费点、detector/`StopDiminishing`/`TurnMetrics` 删自 `src/context/budget/`，见 budget.rs::CEILING；2026-07-18 −2＝流式旁路修复：`stream_llm_call` 弃 `as_http_provider()` 降级分支、改多态 `execute_streaming_dyn`，副作用下沉 `src/providers/` 装饰器，见 budget.rs::CEILING；Batch 6，2026-07-17：两侧同日从 5035 出发、合并实测 5072——上调 +80＝ambient 审批关联 + 完成序 live 事件；删除 −42＝test-only `run_turn` 簇外迁 `tests/harness_ext.rs` + 恒零 `consecutive_errors` trace 字段删除；3 问作答在 budget.rs）。旧的 ~4900 系一次手算口径事故（生产 `impl` 中间的缩进 `#[cfg(test)]` 截断 `agent.rs`、静默漏计 846 行）的残值，**已退休**——红线是棘轮机制本身，不是那个具体数字：
  - 顶层 (8)：`mod.rs` / `agent.rs` / `deps.rs` / `trait_def.rs` / `callback.rs` / `chain_context.rs` / `trace.rs` / `trace_sink.rs`
  - `agent/` 子目录 (4)：`think.rs` / `act.rs` / `guardrails.rs` / `prompt.rs`（Task 8/9/10 把 `agent.rs` 按 Think/Act/Guardrails/Prompt 拆分为四个子职责）
- **行数增长红线**：任何新增 LOC 必须先回答"加代码前必答 3 问"（脚手架 vs 认知 / 模型升级后是否还需要 / 是否有真实消费者）。新增文件需在 PR 描述里说明为何无法装进现有 12 个文件之一
- **循环里的 5 个"不"**:
  1. ❌ 不判断意图分类
  2. ❌ 不按**消息意图**做工具过滤 / 相关性评分（渐进式工具披露例外，见下）
  3. ❌ 不做完成度判断（除模型显式 stop）
  4. ❌ 不做内容审查 / 安全打分
  5. ❌ 不做错误恢复策略选择
  - ⓘ **渐进式工具披露例外（主流实践，Aleph 采纳）**: "core 工具静态常驻 + 全量工具目录 + `tool_search` 元工具按需加载 schema" 是**不看消息内容的静态分区**、加载决策 100% 由模型发起，与 `src/tools/scoped/` 已有的 allowlist / 权限 Deny / 健康三道静态 `retain` 同层同性质，**不属**第 2 不所指的"按意图过滤"。同款见 Claude Code / Anthropic thin-harness。分区落在工具呈现层，**不进 `src/harness/`**，R10 文件预算零增长。
- **12 模块各归其所**: 行业共识的 12 大 Harness 模块（Tools/Memory/Context/Prompt/State/Error/Guardrails/Verification/Subagents/Init...）每一个都有独立物理位置，**不在 `src/harness/` 内堆积**
- **YAGNI 撤回模式**: 任何"零现有消费者"的抽象立即删除/撤回，绝不"为未来留口"。dissolution 期间累计删除 ~5,200 行死代码
- **加代码前必答 3 问**:
  1. 这是脚手架还是认知？认知必须搬到 prompt
  2. 模型升级一档还需要它吗？不需要就删
  3. 现在有几个真实消费者？零个就撤回
- **关联**: 是 R3 (核心轻量化) + R7 (LLM 主权) + R9 (智慧在 Prompt) 在 Agent Harness 工程上的具体落地。详见 [HARNESS_PHILOSOPHY.md](docs/reference/HARNESS_PHILOSOPHY.md)

---

## 🧬 设计原则 (Design Principles)

以下原则指导 Aleph 每一行代码的编写决策，是架构红线之下的工程纪律。

### P1. 低耦合 (Low Coupling)

- **模块间通过 Trait 通信，不依赖具体实现** — 模块只知道对方的契约，不知道对方的内部结构
- **禁止跨层直接调用** — Core 不直接调用 UI，UI 不直接操作数据库，Interface 不处理业务逻辑
- **依赖方向单向流动** — `Interface → Core → Domain`，禁止反向依赖
- **事件驱动解耦** — 模块间优先通过事件/消息传递状态变化，而非直接方法调用

### P2. 高内聚 (High Cohesion)

- **单一职责** — 每个模块/struct/函数只做一件事，做好一件事
- **相关逻辑物理聚合** — 紧密相关的类型、函数、trait 放在同一模块目录下，不要分散到不同子系统
- **命名即文档** — 模块名、函数名、类型名应准确反映其唯一职责，无需注释解释"它是干什么的"
- **大文件及时拆分** — 单文件超过 500 行应考虑按职责拆分为子模块 (参见 [CODE_ORGANIZATION.md](docs/reference/CODE_ORGANIZATION.md))

### P3. 可扩展性 (Extensibility)

- **开放-封闭原则 (OCP)** — 对扩展开放，对修改封闭。新增功能通过实现 trait / 注册插件完成，不修改已有核心逻辑
- **策略模式优于条件分支** — 用 trait object / enum dispatch 替代 `if-else` 链或 `match` 的无限膨胀
- **插件化优先** — 非核心功能优先实现为 Skill / MCP Server / WASM 插件，而非硬编码进 Core
- **Schema 驱动** — 接口使用 JSON Schema (schemars) 自描述，新增字段不破坏旧客户端

### P4. 依赖倒置 (Dependency Inversion)

- **高层模块不依赖低层模块，两者都依赖抽象** — Core 定义 trait，具体实现在 crate 边界之外
- **实践**: `DesktopCapability` trait 在 core 中定义，native 实现在 `desktop/shared/`；`MemoryStore` trait 在 core 中定义，SQLite+sqlite-vec 实现在 `src/memory/` 但可替换
- **构造时注入** — 通过 `AppContext` / Builder 模式在启动时组装依赖，运行时不 `new` 具体类型

### P5. 最小知识原则 (Least Knowledge / Law of Demeter)

- **只与直接协作者通信** — `a.b().c().d()` 链式调用是设计缺陷的信号
- **封装内部结构** — 不暴露 struct 内部字段的引用链，提供有意义的方法代替
- **接口最小化** — pub API 只暴露调用者真正需要的，`pub(crate)` 优于 `pub`

### P6. 简洁性 (Simplicity — KISS & YAGNI)

- **奥卡姆剃刀** — 如无必要，勿增实体。不为假想的未来需求预留抽象
- **三次法则** — 代码重复不超过两处时不要提前抽象，第三次出现再提取
- **删除优于注释** — 废弃代码直接删除，不要注释掉保留。Git 是时光机
- **扁平优于嵌套** — 优先使用 early return / `?` 操作符，减少缩进层级

### P7. 防御性设计 (Defensive Design)

- **系统边界校验** — 在用户输入、外部 API 响应、IPC 消息的入口处严格校验，内部传递信任已校验的数据
- **优雅降级** — 外部依赖 (LLM/网络/文件系统) 失败时提供 fallback，不 panic
- **锁安全** — `.lock().unwrap_or_else(|e| e.into_inner())`，永远处理 poison
- **UTF-8 安全** — 字符串切片使用 `char_indices()` / `.get(..n)`，不用 `&s[..n]`

### P8. LLM 优先 (LLM-First)

- **语义理解交给 LLM** — 所有需要将自然语言转换为结构化意图的任务（意图识别、参数提取、命令路由），优先使用 LLM 语义理解，而非正则表达式或关键词匹配
- **禁止脆弱的模式匹配** — 不要用 regex 解析用户自然语言输入。正则只适用于格式固定的机器生成文本（如 JSON、URL、日志格式）
- **LLM 可做则 LLM 做** — 如果一项任务 LLM 能完成（分类、提取、推理、生成），就交给 LLM，而非硬编码规则
- **结构化输出** — LLM 返回 JSON 结构化结果，代码层只负责解析和执行，不做语义判断

---

## 🧭 12-Factor 对照与采纳 (12-Factor Conformance & Adoption)

> 本节是叠加在 R1–R10 / P1–P8 之上的**映射层**——**不改任何红线/原则，不设新红线**。它把 [12-Factor Agents](https://github.com/humanlayer/12-factor-agents) 与 Aleph 现状对照，并对真实缺口立"采纳条款"（前缀 A，工程承诺级，**非红线**）。逐 factor 证据、锚点与模块改造建议清单见 [TWELVE_FACTOR_AUDIT.md](docs/reference/TWELVE_FACTOR_AUDIT.md)。

### 对照速览

| # | Factor | 一句话 | 已对应 | 状态 |
|---|--------|--------|--------|------|
| F1 | Natural Language → Tool Calls | 自然语言转结构化工具调用 | R7/R8/P8 | ✅ |
| F2 | Own your prompts | 自有提示词流水线 | R9 | ✅ |
| F3 | Own your context window | 显式拥有 context 取舍 | A1 ↔ R9 | ⚠️→A1 |
| F4 | Tools are structured outputs | 工具=结构化输出，执行解耦 | R8 | ✅ |
| F5 | Unify execution & business state | 执行/业务状态统一可重建 | A3 ↔ R10 | ⚠️→A3 |
| F6 | Launch / Pause / Resume | 统一启动/暂停/恢复契约 | A4 ↔ R5/R6 | ⚠️→A4 |
| F7 | Contact humans with tools | 人在环=工具调用 | R5 | ✅ |
| F8 | Own your control flow | 自有控制流（笨循环） | R10 | ✅ 典范 |
| F9 | Compact errors into context | 压缩错误进 context 自愈 | A2 ↔ R10 | ⚠️→A2 |
| F10 | Small, focused agents | 小而专的 agent | R3 | ✅ |
| F11 | Trigger from anywhere | 触发无处不在 | R5/R6 | ✅ |
| F12 | Stateless reducer | 趋向纯 reducer | A3 ↔ R10 | ⚠️→A3 |

> F13（pre-fetch context）已由 assembler 主动召回满足，仅在审计文档备注，不立条款。

> F7 的"人在环"有两条腿：R5 的非阻塞多端推送（push / 通知）+ `ask_user` / `ClarificationManager` 的**阻塞式澄清**（HITL P4——模型可中途提问并 park 在 oneshot 上等待用户回复，等价于 Claude Code 的 `AskUserQuestion`）。表中只标 R5 易被误读成"只有推送"；阻塞澄清能力一直存在且默认开启，锚点 `src/builtin_tools/ask_user.rs` + `src/clarification/`，详见 [TWELVE_FACTOR_AUDIT.md](docs/reference/TWELVE_FACTOR_AUDIT.md) §F7。

### 采纳条款 (Adoption Clauses)

**A1 · 自有 Context Window (Own Your Context Window, F3)**
> Aleph 的 agent 按 **Prompt → Context → Harness → Loop** 四层构建；其中 **Context 层**（历史压缩 / 按模型窗口的压缩时机 / 内容类型路由缩减 / FTS5 检索回注 / 记忆三支柱）是一等工程关切，**context 的取舍由我们显式拥有**，不外包给框架默认消息格式。
> 关联：R9（智慧在 prompt）、P6（简洁）。锚点：`src/context/`、`src/thinker/`。
> ↳ 采纳·非红线

**A2 · 错误压缩 ≠ 错误恢复 (Compact Errors, Not Recover Strategies, F9)**
> **采纳**：把工具/Provider 错误**压缩并呈递给模型**，让模型在下一轮自愈（内容路由缩减、ToolError 事件、`think.rs` 有界 provider-failure drain）。
> **仍禁**（= R10 第 5 不）：在 harness 里做**确定性的"错误恢复策略选择 / 多级重试矩阵"**（hermes 式 fat-harness 重试有意不移植）。
> 边界一句话：**让模型看见并自愈错误 = 要；让 harness 替模型挑恢复策略 = 不要**。
> 关联：R7 / R10。锚点：`src/tool_output/structured/`（类型路由缩减器）、`src/tool_output/hygiene.rs`（ingress 清洗）、`src/harness/agent/think.rs`。
> ↳ 采纳·非红线（本条是 R10 的读法说明，不改 R10 一字）

**A3 · 状态可重建，趋向纯 Reducer (Reconstructible State, Toward a Pure Reducer, F5+F12)**
> **方向性承诺**：执行状态应尽量可从**单一持久源**重建；每轮 Think 趋向"对持久 context 的纯 reduce"（`prompt.rs` 已逐轮重建裸消息）。新增有状态机件时，优先让其状态可观测、可重建。
> **硬约束**：**不得**为此让 `src/harness/` 越过 R10 的 12 文件 / `budget.rs` 行数棘轮（当前 5146，以 `budget.rs::CEILING` 为准），或把业务状态搬进笨循环。具体统一方案先走"加代码前必答 3 问"，列为 backlog 评估（见审计文档 B §P2-1）。
> 关联：R10、P4（依赖倒置）。锚点：`src/harness/trait_def.rs::TurnState`、`src/looping/`、`src/goal/`、`src/agents/swarm/tasks/store/`。
> ↳ 采纳·非红线（方向性原则，落地需独立评估）

**A4 · 统一 Launch / Pause / Resume 契约 (Unified Lifecycle Contract, F6)**
> 把已存在的取消（`cancellation.rs`）、续跑（`resume_coordinator.rs`）、改需求打断/注入（`steering.rs` 三态 Steer/Interrupt/Queue）、workflow resume 命名为**一组生命周期契约**：任何长跑单元（goal / loop / workflow / team task）都应可被一致地启动、暂停、恢复、取消。
> **进展（2026-07-25）**：goal 与 loop 两条自治续跑链已补齐 pause/resume——loop 新增 `LoopStatus::Paused` 与唯一原子迁移原语 `LoopRegistry::transition`（goal 侧 `GoalStatus::Paused` 早有，本轮补 `GoalStore::pause_if_active`）。同批落地**跨会话生命周期管控**（`loop(action='stop'|'pause', session=…)` / `goal(action='clear'|'update status=paused', session=…)` + `stop_all` / `pause_all` 杀手闸），闭合"`list` 跨会话可见但只能在本会话停"的 R6/R8 断线。**边界规则（新增，适用于任何后续长跑单元）：跨会话只能"降活跃度"**——quiet（stop/pause/clear）可跨，arm（start/resume/加配额）必须在该单元自己的会话跑，因为下一步只由**它自己会话**的完成钩子认领；跨会话操作一律带工具内 operator 闸。详见 FEATURE_LOCATOR §4.1/§4.2。
> **进展（2026-07-30）**：goal 侧并发语义补齐——`commit_field_update` 新增**状态 CAS**（`expected_status` 参 + `FieldUpdate::StatusSuperseded`，并发终态转移不再被工具快照复活，loop parity）；崩溃恢复 `block_if_abandonable` 豁免**全部** wait-barrier（timer-parked goal 由 `GoalWakeService` 复活，误 Block 会永久卡死）；Block/Pause 一律顺手 `without_wait`；`Goal.workspace` 由 claim 流水线记录，三条 hook-less 唤醒路不再丢 project workspace。详见 FEATURE_LOCATOR §4.1 第 9 轮。
> **进展（2026-08-02）**：loop 侧界限语义修正——**「在认领时求值的上限不是上限」**。`timeout_minutes` 此前只在 claim 那一刻判，而 tick 在**一个 cadence 之后**才执行、`confirm_fire` 根本不带时钟，于是 `loop(interval='2h', timeout_minutes=1)` 会在用户所设上限之后 **119 分钟**跑完一整轮并推到原始频道，只有**再下一次** claim 才报停（而 `start` 一旦看到 deadline 就丢掉默认 tick 上限，deadline 常常是唯一的界）。现单一源 `pursuit::fires_out_of_bounds` 把 wake **投影**出来判，且必须在**盖 pending marker 之前**——三个入口全要（`try_claim_tick` / `rearm_after_busy` 的 +30s 重试 / `update(timeout_minutes=…)` 走 `reschedule` 作废在飞 tick）。代价是最多**早**一个 interval 停而非任意晚，这是正确读法：安全上限是上限不是近似。**推论（适用于任何长跑单元）**：凡「先认领、后执行」的调度，界限要在**执行时刻**成立；只在入队处判等于没判。同批：`start` 不再静默摧毁 Paused loop（判据统一为 `status != Stopped`）、`resume` 对已过期 loop 诚实拒绝、claim 计数在同步 supersede 时退款、`list` 与 goal 对称补 operator 闸、`sessions.delete` 接上 `terminate_session_continuations`（此前只有 `/new` 与 `sessions.new` 两个消费者，删掉的会话仍在向不存在的自己投递）。详见 FEATURE_LOCATOR §4.2 第 9 轮。
> 关联：R5（AI 主动到达）、R6（一核多端）。统一 API 面（薄 facade，**不进 harness**）列为 backlog（见审计文档 B §P1-1）。
> ↳ 采纳·非红线

---

## 🛠 技术栈与禁用清单 (Tech Stack & Do NOT introduce)

**核心栈**: Rust Core (tokio + serde) · 记忆层 SQLite + sqlite-vec · 接口 JSON Schema (schemars) · Panel Leptos/WASM · 桌面壳 Tauri。

**Do NOT introduce unless explicitly requested**（基于 R1/R3/R7 推导，违者不得合入）:

- **为 Aleph 自身代码引入第二个 async runtime**（async-std / smol）—— 一方代码全栈锁定 tokio（Cargo.lock 中的 async-std 是三方传递依赖，不影响此禁令）
- **独立向量数据库 client 进 core**（qdrant / lancedb / milvus 等）—— 记忆层已锁 sqlite + sqlite-vec
- **`src` 中直接依赖平台 API crate**（windows-rs / core-graphics / cocoa / objc / winapi）—— 违 R1，必须走原生 Bridge IPC
- **正则 / 规则引擎做意图识别或路由**—— 违 R7/P8，语义判断交 LLM
- **非 serde 的序列化栈**—— 全栈 serde

---

## 🔧 开发指南

### 构建命令

| Command | Description |
|---------|-------------|
| `cargo run --bin aleph-server` | Start server (debug) |
| `cargo check -p alephcore` | Quick compile check |
| `cargo test -p alephcore --lib` | Run core tests |
| `just dev` | Dev server (rebuilds WASM first) |
| `just build` | Release build (WASM + server) |
| `just shell-dev` | Run the desktop App in dev mode |
| `just shell-build` | Build 完整桌面 App installers (.dmg/.msi/.deb，内置 aleph-server) |
| `just shell-build-lite` | Build Aleph Panel 纯壳 App installers (无 server，连局域网内 server) |
| `just test-all` | All tests (core + desktop + proptest) |
| `just clippy` | Lint |
| `just verify-build` | CI 验证三产物（完整 App / Panel 纯壳 App / 独立 server）三平台能否正常构建（build-only，不打 tag、不发布） |
| `just release YY.M.D` | **发版**: 更新 VERSION + 提交推送 + 触发 GitHub workflow（需先写 changelog） |

### Rust 工具链

- **MSRV = 1.95**（由 `sysinfo 0.39` 决定），在 `Cargo.toml` 的 `[workspace.package]` 与 `[package]` 两处 `rust-version` 声明。
- 仓库根的 `rust-toolchain.toml` 钉住具体 stable（当前 `1.96.0`），本地与 CI 自动使用同一工具链——无需 `rustup default` 或 `cargo +<ver>`。抬高 MSRV 时同步更新这两处。

> **分发形态**: Aleph 同一 tag 发三产物——完整桌面 App（内置 `aleph-server`，单机零配置）、Aleph Panel 纯壳 App（连局域网 server）、独立 `aleph-server` 二进制（`install.sh` / `install.ps1`）。详见 [PRODUCT_TOPOLOGY.md](docs/reference/PRODUCT_TOPOLOGY.md)。
>
> **信任模型 = 网络边界 + 登录墙**: 默认只绑 `127.0.0.1`；`[gateway] host = "0.0.0.0"` 显式开放局域网。loopback 免凭据恒 operator；远程须在 `connect` 出示 device token / 一次性配对票 / 共享 token 之一，**过了就是 operator，与本地完全一致——单层，没有 Chat/Config 子层**（`method_authz.rs` 只剩 channel tier 闸，Panel 天然全过）。没过则被登录墙挡在 `connect` 之外。协议护栏是 WS Origin 校验。⚠️ **跨子系统地雷**：`devices` 是 **panel 与 cluster 节点共用的一张表**，两边的 `device_id` 都是**对端自报**的——任何「按 id 认领一行」的新路径都必须先拒掉属于另一半命名空间的行，否则会造出一枚 roster 列不出、`revoke_all_panel_devices` 与 `gateway.token.rotate` 都够不到的 operator 凭据（守卫两侧对称，判据单一源 `PANEL_DEVICE_TYPE`）。详见 [SECURITY.md#auth-ux](docs/reference/SECURITY.md#auth-ux) 与 `src/gateway/CLAUDE.md`。

> **执行档位 (Exec Tier)**: 工具执行权限有一根面向用户的旋钮——`Ask` / `Auto`（默认）/ `Full`，Panel composer pill 选（本会话，随第一条消息生效）或 Settings → Policies 设（全局）。规则读工具**声明的元数据**（幂等 / destructive），不认名字；未知工具在 `Ask` 档 fail-closed；`[sandbox.command_policy]` 硬底线任何档位都压不下去。**唯一强制点是 `src/tools/scoped/`——任何新的能执行工具的 surface（新 RPC / 新快路径 / 新后台产地）不经过它就自带旁路**（已堵：斜杠快路径 / `tools.invoke` / 后台续跑）。详见 [SECURITY.md](docs/reference/SECURITY.md) 与 FEATURE_LOCATOR §5.12。

> **⚠️ 命令硬底线扫的不是你写的那行字 (Command Policy Scans a Normalised Copy)**: `[sandbox.command_policy]` 的规则匹配的是 `normalize.rs` 产出的**匹配副本**，写 Windows 规则前必须知道它和原文差在哪：① **副本是两份**——`\` 既是 POSIX 转义又是 Windows 路径分隔符，所以同时携带 **POSIX 折叠视图**（`d\d`→`dd`）与**路径保留视图**（`C:\Windows` 才存在于这一份），换行分隔、取并集；想按路径写规则就按第二份写，别再依赖"反斜杠已被剥掉"（那个无条件折叠正是 `\\?\C:\`→`\?C:` 绕过 hardline 的成因）。② **`powershell -EncodedCommand` 的载荷已被解码回注**（`-e`/`-ec`/`-enc` 全覆盖，64KiB×8×2 层封顶），规则看得见真脚本——**解码在分层之前，`enforcement="off"` 也关不掉**；别再把 `-enc` 当盲区，也别把 tunable warn `win_encoded_command` 当拦截（它只是纸面痕迹）。③ **规则间隙用 `seg!()`＝`[^\n&|;]*` 不是 `[^\n]*`**——后者会把两条无关语句缝成一条罪状（`del /s build\* & echo C:\` 曾因此被**不可关闭的** hardline 误拦），只有刻意跨管道的下载 cradle 才该用整行间隙。盘根 / 系统路径 / 删除动词 / 递归 flag 都是 `rules.rs` 里的 `macro_rules!` 单一源经 `concat!` 拼接，**别手抄字面量**。详见 [SANDBOX.md](docs/reference/SANDBOX.md) 与 FEATURE_LOCATOR §3.8。

> **会话模式 (Session Mode)**: 与执行档位正交的第三根会话旋钮——`chat` / `work`（默认）/ `code`，Panel composer 模式 pill 选（随第一条消息生效）或 `session_set_mode` 工具对话式切。模式只做**工具呈现面**的静态分区（schema 常驻核 × 整族延迟，`tool_search` 永远可发现+晋升——R10 渐进披露例外的形状），不授予不拒绝任何权限；审批仍归 exec tier。单一源 `src/config/types/policies/session_mode.rs`（族表 `_` 词边界匹配；MCP 限定名 `{server}__{tool}` 整体豁免内建表；子代理继承父分区并获短版 mode line）。详见 [MODE_SYSTEM.md](docs/reference/MODE_SYSTEM.md) 与 FEATURE_LOCATOR §5.16。

> **⚠️ MCP 有两个纪元，且纪元是 server 的属性不是请求的属性**: `2026-07-28` 把 MCP 改成无状态——删掉 `initialize` 握手、协议级 session（`Mcp-Session-Id`）、`ping`、以及**服务端发起的请求**。野外多数 server 仍说旧握手，所以 Aleph 是 dual-era 客户端（**只做客户端**，server 侧在兄弟仓 Aleph-mcp）：`connection.rs::probe_era` 用 `server/discover` 探一次、闩进 `OnceLock`、之后只说那一种。**判据只有一条**——错误码落在 spec 保留段 `-32020..=-32099` ⇒ 对方是 modern（"改正这个请求"），其余一律 legacy 去走 `initialize`；HTTP 上因此**必须**把带 JSON-RPC error body 的 4xx 当协议应答返回，当传输失败吞掉就把唯一的判据吃了。**三个咽喉别绕开**：① 请求只能由 `connection.rs::request()` 造（modern 下每请求都得带 `_meta` 的版本/身份/能力，没有握手能只说一次了，绕过去的新 RPC 只在 modern server 上炸）；② `Mcp-Method`/`Mcp-Name` 由 `http.rs` **从正要发出的 body 现推**（server 校验头与 body 一致，不一致回 `-32020`；同理配置头先插、协议头后插，否则一个配错的头让整台 server 全挂而看不出原因）；③ 服务端发起的 sampling/elicitation/roots 全改走 **MRTR**（server 回 `resultType:"input_required"`，客户端带 `inputResponses` + 逐字节回显的 `requestState` **换新 id** 重试）——顺带修好一个旧缺陷：sampling 此前只在 SSE transport 上可能，现在 HTTP 上也真能用了。**`resultType` 缺省必须读作 `complete`**（规范 MUST，旧版 server 的结果全靠这条不被误判）。**声明能力＝承诺，而承诺的谓词必须是「这件事真做得到吗」**：`can_sample` 曾写作「这条连接有没有 `SamplingHandler`」——那是**结构性恒真**（每条连接都被构造器无条件塞了一个），于是 Aleph 对每台 server 都声明 sampling，而 `McpManagerHandle::set_sampling_callback` **全仓零生产调用方**、回调恒 `None`，server 当真发请求就吃一句 `"No sampling callback registered"`。**恒真的谓词等于没判，且它撒的谎只有对面看得见**。现判据是 `handler.has_callback()`，宿主实现是 `mcp/sampling_bridge.rs::serve_sampling`（进程级 `OnceCell<Arc<dyn AiProvider>>`，`agent_init` 注册；**必须懒解析**——manager 早于 agent 的 provider 存在，提前捕获就赶不上握手声明）。位置有两个都要对：handler 在**握手之前**经构造器传入，**回调在任何 transport 启动之前**装到 client 上（`actor.rs::start_server_internal` 顶部，不是 `start_*_server` 之后），`with_sampling_bridge()` 在 `run()` 之前调。`elicitation`/`roots` 刻意不声明。**legacy SSE 另有两处不合规**（同轮修）：流地址曾硬拼 `{url}/events`（任何 MCP 版本都没规定过这个后缀，合规 server 直接 404），`SseEvent::Endpoint` 只 log 而那正是 server 指定 POST 目标的方式；现配置 URL 就是流地址、`endpoint` 闩进 `post_endpoint` 且**拒绝跨源**（每次 POST 都带 `config.headers`＝server 的认证材料，跟随外来 origin 就是把用户凭据交给他从没配过的主机）。**`ttlMs` 过期 ≠ 内容变了**，所以刷新后要比对内容、只有真变了才发**已有的** `ServerListChanged` 走**已有的**发布路径——直接刷连接缓存就完事是断线（bridge 持有自己那份工具表）。**⚠️ 订阅事件流来建状态的机件，必须在订阅之后对账一次（2026-08-04 真机 QA，HIGH·全量断线）**：`spawn_tool_bridge` 曾**只**消费事件，而 broadcast 接收端只投递**订阅之后**发出的消息。boot 的顺序是「`McpManagerActor::run` 自动启动**每一台**持久化 server（逐台发 `ServerStarted`）→ agent_init（重，数秒）→ 桥订阅」⇒ **普通部署下每一台配好的 MCP server，其工具在每次启动后都进不了注册表**：`mcp.list` 报 healthy、`tool_count=29`，模型一个都拿不到，要等手动 restart / add / 崩溃重连 / server 主动 `tools/list_changed` 才修好（启动**慢**的 server 反而正常——所以它看起来时有时无）。实测真实 chrome-devtools-mcp 连续三次 boot `chromedt=0`，`mcp.restart` 后 `29`。修复是**纯连线**：`resync_all` 早就存在，只是当时只能从 `RecvError::Lagged` 那条臂到达。顺序**必须**是先 `subscribe()` 再对账（反过来开出新窗口），重叠由 `sync_server` 的 unregister-then-register 幂等吸收。**推论适用于任何「订阅事件流来构建状态」的机件**——问一句「我订阅之前发生的事，谁告诉我？」，而 boot 恰恰把一切都放在订阅之前；**纯通知型订阅者不适用**（同进程那个发 `tools.changed` 的 sibling publisher 刻意不改：boot 时没有客户端可通知）。详见 FEATURE_LOCATOR §5.20（含刻意不做清单：`subscriptions/listen` / tasks / MCP Apps / EMA）。

> **⚠️ Aleph Hub 只消费不策展，且「披露里的每个数字都必须有人校验」**: 目录内容（上架 / 策展 / 分类）在兄弟仓 **Aleph-Hub**，发布成 `hub.heyaleph.com/catalog.json`；本仓 `src/hub/` 只**渲染 + 安装**一份已发布目录，安装解析是**纯缓存查表**（`ExtensionEntry.install_spec`），没有 provider registry（2026-06-20 的多源联邦当天即撤回）。四条不能只靠读代码看出来的纪律：① **目录槽是 replace 语义**——一份"解析成功但内容可疑"的 artifact 会静默覆盖 last-good，所以 `entry_count` 不符 / id 重复 / id 占用 `local:` 保留命名空间（那是 `.toggle`/`.uninstall` 的寻址空间）必须在 `HubCatalogArtifact::validate()` 里拒掉，**在任何条目进缓存之前**；`content_hash` 已 CUT（跨语言立不出可复算的规范化契约，留着就是永久断线），`generated_at` 则接进 `SyncReport` 让"0 synced"能和"同步了一份陈旧目录"分开。② **给用户看的数字必须有校验者**——`GitDir.sha256` 曾被披露展示、被 `pin.sha256` 回传却**从不校验**，`git_ref` 更是被 `install_git_skill` 完全忽略（钉了 tag 照装 HEAD）；现在检出走 `clone_or_update_at(url, dir, Option<&str>)`（detached，钉住的不随后续 sync 前移，ref 解析不出来就报错而不是退回 HEAD），摘要走 `directory_digest` 在**第一次写盘之前**比对。③ **`installed` 与 `update_available` 是两个不同的真源，别互相实现**——`installed`/`enabled` 永远由活体后端决定（`collect_installed`），`update_available` 只在**安装出处账本**（`src/hub/origin.rs`，与目录同一个 `hub_catalog.db`）有行时才敢说话；且**生产者必须落在消费者那条路上**：徽标渲染在 `extensions.installed`（已安装面板），只喂 `extensions.catalog` 等于徽标永远不亮。卸载要清账本行，否则"删掉再手工装新版"会继承旧版本号点亮假徽标。④ **远程 MCP 的密钥走 `headers` 不走 env**——`HeaderDecl{secret}` 曾向用户索要、存进 vault，然后 `mcp_config_from_spec` 丢弃、`McpManagerConfig` 连字段都没有、`actor.rs` 恒空头拨号（install 报 `ok:true`，server 401）；值永远是 `{{secret:NAME}}` 引用，解析在拨号那一刻由**与 stdio env 同一个** `resolve_secret_map` 完成，别写第二份。⑤ **镜像故障：数字填了却没有读者**——`McpPreset` 的 `how_to_get_url`（6 条真实控制台 URL）躺在 `catalog.json` 里，而 `official_mcp.rs::map_env` 在**五跳链的第一跳**就丢弃它，于是 Panel 的 Configure 步给用户一个光秃秃的 `AMAP_MAPS_API_KEY` 输入框、不说去哪申请（现已补齐 `EnvDecl`→`SecretDisclosure`→`FieldSpec`，链接文字用 URL 的 host 以免动 locale）。同一个结构体的 `reachability` 相反——**填了数据、`ExtensionEntry` 上根本没有对位字段、Panel 对 `tags` 只搜不渲染**，已整删。**判据**：一个"展示用"字段在提交前必须能指出**渲染它的那一行代码**；指不出就是 CUT 而不是"以后再接"。另：**加 hub 工具要动五处登记**（`builtin_tools/hub/mod.rs` + `definitions.rs` + `groups.rs` + constructor 的**构造段和 schema 段**两处 + dispatch），漏 schema 段＝注册了但模型看不见，漏 dispatch＝看得见但调不到；`verify` 子代理按**精确名**拒绝整个 hub 家族，新工具不进那张表就会被 `*` 放行（`verify_denies_every_hub_tool` 用 `TOOL_CATEGORIES` 单一源钉住）。详见 FEATURE_LOCATOR §5.21（含 openclaw `clawhub` 逐项对照与有意留下的已知限制）。

> **⚠️ 「动态路由」是三件事，且第三件不在 `src/routing/`**: ① **工具选择**＝全量 schema 进 prompt、LLM 自选（`harness/agent/prompt.rs`，禁止任何意图分类，R7）；② **消息→agent/session**＝`src/routing/`（配置 `resolve.rs` + **运行时 overlay `overlay.rs`**）；③ **请求→provider/model**＝`src/providers/route_policy.rs` + `failover/` + `load_stats.rs` + `route_observe.rs`。**RouteLLM / semantic-router / LiteLLM / Bifrost 讲的全是 ③**，而它们的路由**大脑**（对 prompt 打分/分类）整类违 R7，**不移植**——能移植的只有不看 prompt 的三件：冷却状态进候选选择、`Retry-After` 全格式解析、信号缺失 fail-open 到高能力档。**两条地雷**：**(a) 判断"这是主槽吗"只认 `SlotKind`，绝不能拿 `tier == EndpointTier::Unknown` 当代理**——这个代理两个方向都错过：钉住的链主槽带真实 tier ⇒ `select_model(provider=X, model=Y)` 的模型被**丢弃**；live 派生的 fallback 被打成 `Unknown` ⇒ 默认链上每个 fallback 都用**主 provider 的模型 id** 拨号（有模型钉住时整条链必死），同一个占位还让 `always_local` 在默认部署下**等于没设**、让 `cost_aware` 把免费本地端点排最后。**(b) 装饰器少一层委托，整条链的能力就没了**——`FailoverProvider`/`ModelOverrideProvider` 曾都不实现 streaming 缝，而 `may_stream_deltas` 的门当时问的是 `as_http_provider()`，于是**生产环境全程没有真流式**且 `stream_llm_call` 是死代码；现在门是 `AiProvider::supports_streaming()`，`execute_streaming_dyn` 的默认实现**回放**响应到 sink（调了就一定交付，覆写只是批量→实时）。**(c) 「这一轮用哪个模型」只有一个决定点**——`harness_bridge/runner_impl.rs::effective_model_directive`（**per-turn pick ▸ session `select_model` ▸ agent `model_hint` ▸ `BrainRef`**）。Panel composer 的模型 pill 与 `[voice]` 低 TTFT 钉都走 `chat.send.model_override`，而这条链**一度到不了那个决定点**：`FlowRequest` 没有模型字段，于是 pick 只到达附件的 vision 判断和 **`ModelResolved` 横幅——那条告诉用户「已切到 X」的横幅**，这一轮仍由 agent 配置的模型作答。新增模型来源必须进这个函数（现由 `FlowRequest.model_directive` 承运），**别让它止步于 UI**。诊断入口只有一个：`self_config route_status` —— 链成员与 walk 共用 `effective_fallback_names`，`next_order` 是下一次请求的**实际拨号顺序**（`preview_order` 调 walk 自己的 `candidates()`，且**只读**：不消费 round-robin tick、不做 `Open→HalfOpen` 迁移），`config_problems` 列出设了却不生效的 `[route]` 条目。**状态码判断一律走 `llm_retry::has_status_code`**——`contains("401")` 会命中 `40123` 这种 token 计数，把健康 provider 判成 `Permanent` 并锁 10 分钟。详见 FEATURE_LOCATOR §3.6（参考项目逐项对照 + 刻意不做清单已在那里，改这层前先看，不必重做对比）。

> **⚠️ 环境信封 (Environment Envelope) = 一个事实源 + 按易变性分区**: 模型看到的 `cwd` / `os` / `arch` / `shell` / `host` / `repo` / `git` 分支 / 本地时间**全部只出自 `RuntimeContext`**（`src/thinker/runtime_context.rs`）——**prompt layer 不许自己读 `std::env`**。`EnvironmentLayer` 曾自己读 `current_dir()`，于是把 **daemon 进程的目录**当成 agent 的：一次请求里三个互相矛盾的目录（Stable 层 daemon cwd / Dynamic `cwd=` 也是 daemon / 只有 transient tail 是真的），而工具实际跑在 `inner.rs::effective_workspace`。后果不止答错分支——模型照 prompt 发 `bash(working_dir="<daemon 路径>")`（绝对路径不触发 `default_working_dir` 注入）直接吃 `CapabilityDenied "cwd outside workspace root"`，被拒绝进入 prompt 声称它所在的目录。真值经 **`TurnEnvelope`**（`exec_tier` / `session_mode` / `cwd`，`src/thinker/context.rs`，`FlowRequest.envelope` 单字段下传，替掉此前三个同型 `Option` 位置参数）流入 `RuntimeContext::collect_in`，其 `cwd` 与喂给 `default_working_dir` 的是**同一个值**。**分区规则**：进程不变的事实进 Stable（`to_stable_lines` → `EnvironmentLayer` @300），每 run / 每小时变的进 Dynamic（`to_dynamic_line` → `RuntimeContextLayer` @1720；审批档位 / 会话模式 → `OperatingEnvelopeLayer` @1758）——**per-run 字节进可缓存前缀＝整段会话的 provider 前缀缓存作废**（`SecurityLayer` @600 默认 Stable，所以那两根 pill 必须搬出去；`stable_layers_come_before_dynamic` 禁止直接翻 @600 的 stability）。**一个问题只准一个声音**：审批只由被强制的 `ExecTier` 说，未被强制的 `elevated_policy_note` 仅在它缺席时补位。护栏在 `src/thinker/prompt_contract.rs`——**生产恒在的字段必须填进 `production_shaped`（用固定合成值，保持机器无关）**，否则三把尺量的是虚构 scaffold：`runtime_context` 曾被写进 `CONDITIONALLY_SILENT` 当借口，OS/cwd 重复与 500+ B 的档位/模式行因此长期不受棘轮约束。往 `<tag>` 里插用户/模型正文前先过 `xml_util::escape_xml`（六个 §2.3 层曾裸插，`loop(prompt=…)` 可闭合元素在顶层伪造 `Approval mode: full`）。详见 FEATURE_LOCATOR §2.3。

> **语音作为 Context (Voice-as-Context) = 注册表摆渡 + 一词典两消费**: voice 模式由 gateway 入站侧判定，消费却在 harness bridge 的 prompt 组装——两侧只共享 session key，经 `src/gateway/voice/voice_mode.rs` 的进程级注册表摆渡（request metadata 到不了 `build_system_prompt`，旧 `metadata["voice_mode_active"]` 戳因此从无读者、layer 从未在生产触发）。**写入点有两个，都要写**：channel 侧 `inbound_router/executor.rs` 与 Panel 侧 `handlers/agent.rs::build_run_request`——漏一个，那个 surface 的语音回合就永远拿不到口语风格层。条目是 `VoiceTurnState{transcribed, vocabulary}`：`transcribed` 区分「读出来」与「读出来 + 输入是 ASR 转写」（后者追加转写修复规则）；`vocabulary` 把 `[voice] vocabulary` 词表一并给模型修误识别词——喂 ASR 偏置的**同一个** `vocabulary_hint()` 字符串（一词典两消费，FluidVoice 模式），typed 回合不渲染（prompt 字节不变）。注册表带 TTL 懒清扫 + 容量硬顶（镜像 `streaming/relay.rs` 的 StreamRegistry 卫生）；活会话每条入站消息重写条目，清扫永远打不断活回合。详见 FEATURE_LOCATOR §2.4（含 FluidVoice/WhisperLive/WhisperLiveKit 逐项对照与刻意不做清单）。

> **繁忙输入与消息车道 (Busy Input & Wait Lane)**: 会话已有在跑的 run 时，新消息按通道声明的 `BusyInputMode` 分流——`Steer`（默认，注入 live 日志让循环下一轮接住）/ `Interrupt`（真取消同会话 run 及其委派子运行）/ `Queue`（不打扰，排队）。**投递不了的一律进 `src/gateway/busy_queue/` 的 per-session FIFO 车道**，channel 与 Panel/CLI 三个 surface 共用同一条车道、同一套到达序与溢出策略（ticket **必须在到达路径同步取**，进 spawn 就把到达序换成调度序）。等待端**不轮询**：靠 `SessionRunRegistry::release` 的放槽信号唤醒（codex `InputQueue` 对位），`wake_fallback_secs` 只是漏发兜底。⚠️ **车道是候车室，不是运行登记簿**——`deliver_with_ticket` 把 ticket 攥到 `attempt()` 返回，而 `attempt()` **就是整轮 agent run**：所以取槽点 `SessionRunRegistry::try_claim` 成功时必须 `busy_queue::mark_admitted` 把该票撤出车道（与 `release`→`notify_slot_free` 严格镜像）。少了这一步，正在跑的那条消息一直占着队首，后来者永远 `is_front()==false`、永远到不了 `admit_run`——而 `Steer` 与 `Interrupt` **只在同会话 sibling 正在跑时才有意义**，于是两者**静默退化成 `Queue`**（用户的"改需求"不再修正当前 run，而是等它跑完另起一条，×N 轮 LLM 成本；Panel 的批量 flush 注释明写依赖 steer 合流）。同一根因还让 `/stop` 回执把"正在跑的那条"数进"已丢弃的排队消息"、让 `busy_queue.total_waiting` 把已成 run 的消息算作 backlog——**三件事一起修或一起坏，别只看其中一个**。FIFO 只约束**仍在等待**的消息。停止有两个粒度——`/stop` 清整条车道，`chat.abort` 按 `run_id` 停单条排队消息（排队中的 run 不在 `active_runs`，引擎的 cancel 够不到它）。**⚠️ 别把这条车道和 Panel composer 的「撤回」混为一谈**：composer 的 `ChatState.prompt_queue` 在 flush 之前**根本没到过服务端**，所以「向上键召回正在排队的消息」（`↑` 空草稿 / `⌥+↑` / 点幽灵气泡）是**纯客户端**的 `recall_latest_queued` + `seed_draft`，零协议改动；`purge`/`cancel_queued_run` 停的才是已经到了服务端的排队 run。客户端队列两条纪律：**(a) 召回必须非破坏**——`seed_draft` 把召回正文**前插**进现有草稿（`merge_recalled_draft`）、附件并入 `pending_attachments`，绝不覆写，因为这个 composer 没有任何 undo（codex 敢覆写是因为它有 `↑`/`Ctrl+R` 历史兜底，照抄那一半＝永久销毁用户已打的字；连按 ↑ 从队尾往回走，前插正好把队列原序在草稿里重建）；**(b) 草稿只有一个家**＝`ChatState.draft`，wide 与 phone 两个 composer 都是 `let input_text = chat.draft;`，所以读不到草稿的幽灵气泡也不需要中间人——`take_queued_prompt` 之后直接 `seed_draft`。**别再造"写一个信号、指望别处排空"的预填通道**：那种形状在多平台下必然漏一个消费者（`draft_seed` 在 phone 上就没有排空点，一次幽灵气泡点击把整条排队消息扔在地上，零报错）。⚠️ 现役按**下标**寻址（`take_queued_prompt(idx)` / `remove_queued_prompt(idx)`），`QueuedPrompt` **没有 `id` 字段**；安全性依赖 `For` 的复合 key `{idx}:{text}`——队列一变就重新 key、重建行并刷新闭包里的 `idx`；掉任一半都有活的失败（只剩 text ⇒ 过期下标删错行 / 把**已发出**的 prompt 恢复进 composer 让用户再发一遍，只剩 idx ⇒ 行渲染的是已被取走的那条）。该 key 的单一源是 `state.rs::queue_row_key`，`queue_tests` 四条守卫钉住：三条量它的返回值，第四条用 `include_str!` 钉住 `messages.rs` 的 `<For>` **真的在调它**——host 测试看不见 Leptos view，源码断言是唯一抓手，否则前三条会在连线断掉后继续全绿（本仓惯犯形状：守卫断言了调用、没断言效果）。**⚠️ 两条分支各写过一版召回，别被"缺失的方法"骗去补连线（2026-08-03，已结）**：同日两条分支各实现一遍（Round-7 的 merge-进草稿 `seed_draft`，与另一条的 park-草稿-到队尾 `retract_queued_prompt(id)`），合并后**类型来自一侧、调用点来自另一侧**，`aleph-panel` 因此在 main 上编不过。**存活的是 `seed_draft` 这一版**，id 寻址那版已被后续 origin 合并整个退回。所以看到"缺失的方法"时先问**哪一端才是残片**：这两次（`retract_latest_queued`、`draft_seed`+`merge_draft`）**残片都在 composer 一侧**，正解都是 CUT 而非往 `ChatState` 上补——补上就是复活第二条预填路径，而草稿只有一份。origin `f42891763` 已 CUT 完毕，panel 现绿。旋钮在 `[execution] busy_queue_*` / `max_pending_steering`。详见 FEATURE_LOCATOR §4.8。

> **团队群聊直播面 (Team Group Chat Live Surface)**: 群聊的实时状态全部走 `team.<id>.*` 五类 topic——`message` / `system` / `activity` / `fanout` / `task.<verb>`，**信封只有一种**（唯一发布口 `gateway::event_emitter::team_fanout::publish_team_event`，per-run 的 `TeamFanoutEmitter` 也走它），Panel **只有一个解析点**（`views::chat::team_events::parse_team_topic`，后缀匹配、team id 可含 `.`、`team.changed` 不匹配）。Gateway 订阅是 `team.*` 通配 ⇒ **投影前必须先按当前 `chat.team_id` 作用域**，否则后台团队的气泡会挤进任意会话（含单聊）。`fanout started/settled` 是团队模式下 `active_run_id` 的**唯一写者**——它给群聊撑起 Stop 键（路由到 `teams.chat.cancel`，**不是** `chat.abort`：fan-out 树不在引擎 `active_runs` 里），并顺带接上 composer 已有的队列 auto-drain 忙→闲边沿。历史回放侧 `teams.chat.history` 只回放对话行（`MessageType::Message` 或无 recipients），定向收件箱流量不进群聊，`kind` 由服务端派生。详见 [MULTI_AGENT_SYSTEM.md](docs/reference/MULTI_AGENT_SYSTEM.md) §Group Chat 与 FEATURE_LOCATOR §4.5。

> **⚠️ 能力接上了 ≠ 模型会用它（DESCRIPTION 是能力清单的第二份真源）**: 工具的 `DESCRIPTION` 里那句「某平台不可用，回退到 X」**随 schema 发给模型**，它和代码里的 `Option<&dyn Capability>` 是同一个事实的两份表述——只改代码那份，模型仍然按 prompt 行事。Linux 的 AT-SPI 无障碍层落地时，`ax()` 已经返回 `Some`、四个 AX 工具与 `desktop_som` 全部可用，而六处 DESCRIPTION 还在写「unavailable on Linux — fall back to screenshot + gui_locate」：**这是 advertised-but-disabled 的镜像版**——能力真有，prompt 在劝模型别用，比缺失更难发现（工具能跑、测试全绿、模型就是不调）。**加/删任何 capability 的同一笔改动里必须 grep 一遍工具描述**（`src/builtin_tools/**` 的 `DESCRIPTION` / `types.rs` 的字段 doc / registry constructor 的注释），三处都在 prompt 或 schema 上。同族陷阱见下条「加了通道 adapter ≠ 用户能配」与「hook 注册了 ≠ hook 会触发」。**⚠️ 前置条件（2026-08-02 §2.16 round-2 发现）：上面这段的全部前提是「`DESCRIPTION` 随 schema 发给模型」——而对 156 个工具里的 151 个，它根本没发。** `BUILTIN_TOOL_DEFINITIONS`（`src/executor/builtin_registry/definitions.rs`）里每条 `description:` 若写成手写字面量，就会**整体遮蔽**该工具的 `AlephTool::DESCRIPTION`：`agent_init` 先用这张目录建 LLM 工具表，再 `filter(|t| !existing.contains(&t.name))` **只追加目录里没有的名字**，所以构造器注册的富文本永远排在同名目录条目后面、永远进不去。当时只有 5 条目录条目指向自己的常量（五个 file 工具）。后果的严重性在于**它是静默的且测试反向背书**：`memory_protocol.rs::acknowledgment_contract_is_stated_once_per_writing_tool` 断言在**常量**上，于是"D4 确认契约存在"恒绿，而模型一个字都没收到——**测试验证句子存在，从不验证句子发出**。这还让 2026-07-26 的 prune 轮（R9 第二把尺）变成一次静默删除：它把 ~750 token 从 `MemoryProtocolLayer` 搬进工具 `DESCRIPTION` 并宣布"现在只活在工具里各一份"，而那个目的地没通电——**智慧没有迁移，是被删了**。**⚠️ 2026-08-04 全仓扫荡已完成，这条纪律的形态随之改变**：155 条目录条目**全部**指向各自工具的常量，零字面量；无常量的三个工具（`note_orient` / `note_schema` / `user_profile`，此前目录与构造器**各写一份不同的字面量**、构造器那份更丰富）各建了一个常量供两处共用。守卫是 `definitions.rs::tests::no_catalog_entry_inlines_its_description`——**必须是源码级断言**（`include_str!`），因为运行时分不出「来自常量」和「恰好字节相同的字面量」，而失效方式正是它们**停止**相同；该守卫同时钉住"扫到的 `description:` 站点数 == 条目数"，否则换个写法就能让它passing by not looking（首次运行即抓到 round-4 `agent_identity` 那条被 rustfmt 折行的值）。**所以现在要问的不再是"目录条目指向常量了吗"（守卫替你问），而是"这些字节值不值"**：`catalog_description_bytes_ratchet` 实测 **81,274 B**（扫荡前 29,854 B——143 条字面量共 13,508 B 遮蔽着 64,866 B 工具自述），只减不增、增必答那三问。**这笔钱每个请求都付**：`[tools] core` 默认非空⇒渐进披露开着，但非核心工具**只折叠 schema、description 原样全发**，而 `truncate_tool_descriptions` 默认 `false`——**没有任何一档配置让这些字节免费**。同批为此付的账：`no_sentence_is_stated_twice` 一接通就抓出 6 条真重复（AX 平台支持句 ×4、heartbeat 取 ID 提示 ×3、`bash`/`code_exec` 的 `working_dir` 行），全部收敛到一处——**它数的是"发出去几遍"，不是"写了几遍"，所以共享常量被 N 个工具各发一次照样算重复**。 **⚠️ 已复发一次（2026-08-04，§5.17 第四轮）**：`agent_identity` 的目录字面量列了七个 action、**连 `export` 都不提**，于是 2026-08-01 那一轮写进它 `DESCRIPTION` 的整套「导出 + 钉根指纹」说明**一个模型都没收到**——而那一轮的验收测试断言在常量上，全绿。**推论**：这条纪律不只约束「精简 prompt」这一类改动，它约束**任何给工具加能力的改动**——新 action 的说明落在 DESCRIPTION 里，而目录条目决定它发不发得出去；守卫必须**断言在目录那一侧**（读常量的测试会在它正要抓的那个回归里保持全绿，见 `agent_identity::tests::the_catalog_ships_the_tools_own_description`）。

> **⚠️ Linux 桌面实现不在 `desktop/linux/`（找错目录会以为功能不存在）**: `desktop/linux/` 只装 **AT-SPI2 无障碍层**（`ax/`）与 system / permission / media / automation / pim；**窗口管理、剪贴板、应用启动退出、输入、截图、OCR 的 Linux 实现全在 `desktop/shared/`**——`action/window_linux/`（原生 EWMH via `x11rb` + sway/Hyprland IPC）、`action/wayland_input.rs`（ydotool）、`linux/{session,clipboard,app,proc}.rs`。原因是 `action`/`perception` 的 per-OS 臂本来就住在 shared，而**依赖方向是 `linux → shared`，真源必须在被依赖的一侧**（会话类型曾在四处各写一份，剪贴板还靠 spawn 顺序隐式探测，就是把真源放错边的后果）。会话类型/合成器/可用 CLI 工具**只有一个答案**：`desktop/shared/src/linux/session.rs`。⚠️ **凡等桌面服务的 shell-out 都要带死线**——异步侧 `output_capped`、同步侧（`spawn_blocking` 内 / 真同步代码）`output_capped_blocking`；判据是**这条命令在等别的进程还是在算**：`xclip -o` 等**当前持有选区的那个应用**把内容交出来（卡死的应用永远不交，而那正是用户找 agent 的头号理由）、`notify-send` 等通知守护进程的 D-Bus 回复、`swaymsg`/`hyprctl` 等合成器 socket、`pactl` 等声音服务器、`ffmpeg` 等被别人占用的摄像头——这些卡住就把整个 turn 挂到 harness 上限并泄漏子进程，三平台各栽过一次；`uname -r` 这类纯计算不需要（加了是噪音，P6）。**自己写轮询等子进程也不行**：只 `try_wait` 不排空管道，子进程输出一过 64 KiB 管道缓冲就双向死锁，然后给一条工作正常的命令报超时。**AT-SPI 层有三条不能只靠代码看出来的纪律**：① **连接共享、句柄不共享**——`AccessibilityConnection::new()` 实测 **424 ms**（先经会话总线问 a11y 地址，再做第二次握手），而它是**每次 capability 调用**都付；「无状态」这条理由针对的是**元素句柄**（那个确实每次重解析），不是 socket，别再把它套到连接上。**但共享连接必须先探活再交出去**——zbus 连接由**建它的那个 runtime** 驱动，换 runtime 复用（或总线重启后）**不报错、直接永远等**（写这轮代码时真实撞上：单测里每个 `#[tokio::test]` 一个 runtime，第二个用到它的测试静默挂死）；复用路径付一次 250 ms 上限的 `get_id()` 探活，不活就重建。② **`CacheItem` 的字段名与它装的东西不符**——D-Bus 签名是 `… as s u s au`，name 在 `role` 前、description 在 `role` 后，而结构体把前者叫 `short_name`、后者叫 `name`；照「看起来对」的字段读会让标题从 70 个节点的 47 个掉到 3 个，**全部单测照样绿**，只有拿缓存值逐字段对比实时读的 live 探针能抓到（`tests/atspi_live.rs`）。③ **墙钟预算与节点上限都要**——`MAX_NODES` 管读多少、`ax/budget.rs` 管读多久，而每次读都是打进**另一个进程**的 D-Bus 往返，卡死的应用恰恰是用户找 agent 的头号理由。**密码框判据是两条腿**：原生 `Role::PasswordText` + 共享的 `is_password_like` 标签词表（`desktop/shared/src/ax_secure.rs`，Windows 与 Linux 同一份），判定必须**在读值之前**做——被判为密码的字段根本不去取内容；`set_value` 的回读同理只报「对上没对上」，`actual_preview` 对 secure 元素置空。**「前台应用是谁」有两个来源**：窗口层优先，缺席时回落 AT-SPI 的 `State::Active` 顶层 frame——GNOME/KDE Wayland 没有窗口管理 IPC，只靠前者会让整个 AX 层与密码管理器硬阻断在那两个桌面上恒不可用。详见 FEATURE_LOCATOR §7.1-§7.4。

> **⚠️ 「至多一次」的承诺只覆盖了「传输层报了错」那一半 —— 进程消失是第三种结局**: 出站 durable 队列（`src/gateway/delivery_queue.rs`）把错误分成「肯定没送到，可以重试」与「可能已在线上，绝不重试」两类，然后**默认第三类不存在**：daemon 在 `send` 成功之后、`mark_delivered` 之前退出，会留下一条**仍然 pending 且已到期**的行，下次启动原样重投＝重复投递。判据是**这次尝试有没有跨越那个不可逆边界**，而不是它返回了什么——所以正解是在**跨越之前**盖一个持久化的戳（`mark_inflight`），并在**新进程拿到这份状态的那一刻**（`spawn_drain` 赢下 drainer 槽时，`reconcile_inflight`）把幸存的戳按「结果未知」退休而不是重发。**推论适用于任何「先记录意图、再做不可逆动作」的机件**（发消息 / 调外部 API / 付款）：只记录「做完了」的机件，无法把「没做」和「做了但没记上」分开。同批三条同族纪律：**① 「按表断言的安全性」在多了第二个生产者之后就作废** —— redrive 曾靠"只有 `should_enqueue` 允许的错误才进得来死信表"免检重放，终态失败与崩溃对账一落地这条推理就不成立；安全位必须**随记录走**（`DeadLetterReason::replay_safe`），且判据单一源在 Rust 侧、SQL 只做投影。**② 「先认领、后执行」的队列里，队头退避必须带上队尾** —— 只在一个 batch 内做队头阻塞看起来对，跨 tick 就漏（队头退到未来、后继仍然 due，下一 tick 单独把它发出去），保序**永久性**地坏掉且零报错；这与 §4.2 「在认领时求值的上限不是上限」是同一个形状的两面。**③ 上限要量对量纲** —— 数**行**不数**字节**的 CWE-400 防御，对一张行里能装内联媒体（`Attachment.data: Vec<u8>`）的表等于没设。**round-2（2026-08-03）再补三条**：**④ 「保序」只在慢路径上实现，等于没有** —— 队列内部按会话保序做得再对，一条**从没失败过**的消息仍会走快路径当场发出、越过还在服务退避的旧消息（最长 `max_backoff`，且永久性乱序）。而"把快路径也塞进慢路径"通常要改契约（排队的消息没有 `SendResult` 可还）并把乱序换成队头阻塞；正解的形状是**让快路径先把慢路径排空**（`flush_conversation` 在 `send` 里跑在 `send_attempt` 之前，首次失败即停，返回值逐字节不变）。**⑤ 机会性探测不能花别人的预算** —— 顺路搭车的那次尝试若按正式重试结算，十条用户消息就替队头烧完十次 attempts、远早于配置的退避曲线把它死信掉；瞬时失败必须**原样放回**（`AttemptMode::Inline` 只 `clear_inflight`），**但歧义终态照旧结算**（可能已在线上的东西交回去重试就是 at-most-once 之死）。同理**新增第二个claim 者的那一刻**就是无租约 SELECT 失效的那一刻——本仓用进程内 `drain_gate` + `try_lock`（宁可放弃这次冲队，也不把用户可见的发送阻塞在整个 drain tick 之后）。**⑥ 一条 durable 记录不能比它引用的东西活得久** —— 按**引用**持久化（文件路径 / 句柄 / 外部 id）时，字节上限量的是那个引用而不是被引用物，所以"有上限"给不出任何保证：队列里每个 `Attachment.path` 都指向 **OS 临时目录**（`media::cache` 的外泄防护只允许 temp root，TTS 也写那里），两百字节的行稳稳过闸，然后重放出一条媒体已经没了的消息。托管要在**唯一准入咽喉**取得（`take_media_custody` @ `maybe_enqueue`），且**优先把字节收进记录自身**而不是另开 spool 目录——行自包含 ⇒ 淘汰/死信/redrive/上限全部原样工作，没有第二套生命周期要 GC（openclaw 需要 spool 是因为它 queue-first，每条消息都过队列；Aleph 只在失败时入队，别照搬）。详见 FEATURE_LOCATOR §5.6。
>
> **⚠️ 「状态」回答不了「该不该自愈」—— 缺的那一半是意图**: `ChannelStatus::Disconnected` 同时表示**没启动过 / 被 operator 停了 / socket 死了**三件事，所以健康监控无论怎么写谓词都会二选一地错：认它就去重启用户明确停掉的通道，不认它就永远救不了死掉的 socket。旧谓词选了后者（只认 `Error`），而 **discord / irc / xmpp 的连接任务退出时全都写 `Disconnected`**——discord 甚至先赋 `Error` 再**下一行无条件覆盖**——于是这张"僵尸通道安全网"注册了、有重启预算、有测试，**对它最该救的那几个通道从来没触发过**。真源只能是**服务 `start`/`stop` 的那一层**（`ChannelRegistry::DesiredChannelState`），谓词是「该跑 × 挂了 × 陈旧」。**推论**：任何"检测到坏了就自动修"的机件，先问**「我凭什么认为它现在应该是好的」**——这个问题的答案不在被检测方的状态里。另：**别在退出路径上无条件覆写 status**，那一行会把上一行刚记录的失败原因抹掉，对操作面（`channels.list`）和自愈面同时致盲。

> **⚠️ 加了通道 adapter ≠ 用户能配**: 配置式通道的唯一入口是 `gateway/interfaces/plugin.rs` 那张工厂表——`create_channel_from_config` 查不到 `channel_type` 就 `return None`，`initialize_channels` 只打一行 `Failed to create channel`。表是 2026-04-05 引入的，之后新增的 5 个通道自己注册了，**之前就存在的 10 个（slack / discord / matrix / mattermost / signal / irc / nostr / xmpp / email / webhook）从未回填**，于是它们各自完整的 adapter + config + 测试在生产里整整不可达到 2026-07-26 才补上（`register_channel_plugins` + `register_plain_channel!`）。**新增 adapter 必须手工进这张表**——`every_configurable_channel_type_is_registered` 只钉当前集合防回归，枚举不了 `impl ChannelFactory`，抓不到将来的遗漏。`imessage` / `cli` 刻意不在表内（各有直连路径，注册即死码）。

> **频道寻址 = 两步，通道能力位 = 承诺**: `channel_message` 只吃不透明的 `conversation_id`（`C0A1B2C3` / 数字 chat id / JID），而这类 id **只有入站消息才会产生** —— 所以"把结果发到 #eng-releases"必须先 `channel_directory`（读，`Channel::list_conversations` 的唯一消费者）换到 id 再发。**两者刻意是两个工具**：`ToolFacts::idempotent` 按**工具名**取自 `READ_ONLY_TOOLS`，合并进非幂等的 `channel_message` 会让查询在 `Ask` 档一并被闸（档位只收紧不放宽，没有反向豁免口）。花名册**只读路由元数据**（名字/id/是否成员），**不读消息内容** —— 内容拉取会绕开只作用于**推**来消息的入站访问控制（`inbound_router::check_permission` / dm-group policy / pairing）。另一侧：`ChannelCapabilities` 的每个位都是承诺，**声明了就必须覆写对应的 `Channel` 方法** —— 默认体现在一律 `Err` 并指名道姓报"adapter 声明了却没实现"（此前是 `Ok(())` 静默成功，6 个 adapter 因此谎报，其中 msteams `react` / whatsapp `delete` 会让工具回 `delivered: true` 而对面什么也没收到）。详见 FEATURE_LOCATOR §5.18。

> **⚠️ Windows 桌面坐标 = 进程属性，不是 API 属性**: 一个 DPI-unaware 的 Windows 进程读到的 `GetWindowRect` / `GetCursorPos` / `SendInput` 绝对空间 / UIA `CurrentBoundingRectangle` **全部被系统虚拟化**（除以显示器缩放比），而屏幕截图走显示驱动**不被虚拟化** —— 在 Windows 默认 150% 缩放的笔记本屏上，"模型在截图里看到按钮的位置"与"点击落点"差 1.5 倍，且**事后无法补救**（两个数字一样合理）。Rust 二进制不带 manifest ⇒ 默认就是 unaware。opt-in 是**一个 latch 两个调用点**：`WindowsPlatform::new()` **与** `NativeScreen::new()` 都调 `aleph_desktop::win_dpi::ensure_process_dpi_aware()`（`OnceLock`，必须早于任何窗口/DC/UIA 对象），且**读回 OS 实际达成的等级**而非相信返回值 —— 两个点是因为 `src/vision/providers/platform_ocr.rs` 直接构造 `NativeScreen`，只钉桌面工具那条路会让同一块屏在一次运行里报出两个 `scale_factor`（`coordinate_scale` 读的是**实时**等级）。推论：**`DisplayInfo.scale_factor` 的语义是「本进程报出的几何空间里 1 单位 = 几个物理像素」**，不是显示器 DPI 比 —— 有三个消费者按这个语义乘它（`coord_resolve::resolve_viewport` / `set_of_marks` / `coord_resolve::window_frame`），换成 DPI 比就是三处同时错（那正是 Windows 上归一化坐标恒偏 1.5 倍的成因）。
>
> **DPI 只统一了单位，还有两处「读写不同源」把对的数字送到错的地方**（同一形状，各自静默）：① **指针**——绝对定位不走 enigo：它按 `SM_CXSCREEN`（**主屏**）归一化且不发 `MOUSEEVENTF_VIRTUALDESK`（其源码里就是一句 `// TODO`），于是副屏上的点要么超过 65535 被钉在主屏右缘、要么（左/上侧显示器的负全局坐标）被钉在左上角 —— **多显示器上瞄准副屏的每一次点击都落在主屏**，而 `cursor_position`（`GetCursorPos`）读回的是真正的虚拟桌面坐标。真源是 `desktop/shared/src/win_input.rs`（`SendInput` + VIRTUALDESK + `SM_XVIRTUALSCREEN` 归一化，单屏下与 enigo 旧公式**逐字节等价**，有回归测试钉住），入口是 `action::move_pointer` —— 新增鼠标动作走它，别再调 `enigo.move_mouse(.., Abs)`。② **窗口**——`window_list.bounds` 是 DWM **扩展帧**，`SetWindowPos` 吃**原始窗口矩形**，差一圈不可见抓边（本机实测 10–11 px/边）：把读到的 bounds 直接写回去，`move` 每次右下偏一个边框、`resize` 每次视觉缩窄两个边框。补偿在 `win_window::FramePadding`；最大化窗口先 `SetWindowPlacement(SW_SHOWNOACTIVATE)` 退出最大化（**不是** `ShowWindow(SW_RESTORE)` —— 那个抢焦点，违 R5）。
>
> 另三条同族纪律：**「什么算一个窗口」只在 `desktop/shared/src/win_window.rs` 回答一次**（cloaked / tool-window / DWM 可见边框；此前三份私有 `EnumWindows` 各答各的，本章一半 bug 是那个形状）；**桌面层任何 shell-out 走 `script_exec::hidden_command`**，否则无控制台的 daemon 每次调用给用户闪一个黑框；**UIA 客户端工作全进程串行**（`ax.rs::uia_gate`）—— 两个 `spawn_blocking` 线程各自实例化/析构 UI Automation 客户端时，其中一个会拿到裸 `E_FAIL`，看起来像"无障碍层偶发抽风"而不是少了一把锁（两个 live 探针并行跑必现，单跑必过）。详见 [WINDOWS_RUNTIME.md](docs/reference/WINDOWS_RUNTIME.md) 与 FEATURE_LOCATOR §7.1。

> **⚠️ macOS 上 `aleph-server` 不是一个 app —— AppKit 里凡是要「主 run loop」或「前台身份」的东西都不成立，而且都会报成功**: daemon 的主线程被 tokio 占着，从不建 `NSApplication`、从不跑主 `CFRunLoop`。两个实测证伪的假设（macOS 27）：① **`NSEvent.addGlobal/LocalMonitorForEvents` 永不触发** —— handler 由真实 `NSApp` 的主 run loop 派发，补 `NSApplication.shared` + 泵满主 run loop 也不行；而**安装是成功的**（返回非 nil、零 warn），所以没有任何信号说它是死的 —— Esc 紧急停止键因此死了很久，上面每一层都报告它已武装。全局按键只有一条路：**自带线程 + 自己 `CFRunLoop` 上的 listen-only `CGEventTap`**（`desktop/macos/src/escape_listener.rs`；`kCGEventTapOptionListenOnly` 让「吞掉全机器的 Esc」不可表达，而不只是「注意别吞」）。② **`NSRunningApplication.activate` 与 AX `AXFrontmost` 都拿不到前台**，且分别返回 `true` / `kAXErrorSuccess` —— macOS 不把前台交给用户没在驱动的进程。故 `focus_window` 只能轮询 `isActive` 如实报失败，**不要为此把 daemon 改成 bundle app 去抢焦点**（违 R5）；需要够到某个 app 的场景全都有不需要前台的路径（`set_value`/`ax_action`、`screenshot{window_id}`、带 `app`/`pid`/`window_id` 的定向输入）。**推论**：往 macOS 四肢加任何「要成为前台 / 要 AppKit 事件循环」的能力前，先假设它不成立、再实测 —— 这一族失败**全部是静默的**。详见 FEATURE_LOCATOR §7.2。
>
> **⚠️ 跨平台改动要在每个目标上 check 那个目标的限肢 crate**: Windows DPI 轮把 `NativeScreen::new` 改成非 `const`（要落 DPI 感知闩），`MacOSScreen::new` 是 `pub const fn` —— `aleph-desktop-macos` 从此编不过，而 `alephcore` 在 macOS 上依赖它，**整个 macOS 构建坏了一整轮没人发现**，因为日常回路里没有一步单独 build 那个 crate。`cargo check -p aleph-desktop-{macos,windows,linux}` 各跑一次，别只信 `-p alephcore`。桌面壳同理：`cargo check -p aleph-desktop-shell` 前需先 `just _stage-shell-placeholders`（tauri-build 要求 externalBin 占位文件存在），否则 build.rs 直接失败、看起来像代码坏了。
>
> **⚠️ 「声明了死线」≠「花了死线」**: `shared/protocol/.../methods/*.rs` 曾有十个 `SUGGESTED_TIMEOUT_MS*` 常量、文档写着"推荐的客户端超时"、**零消费者**——每个 RPC 都走 60s 兜底。最贵的两个：`ax.query_focused`（`type_text` 焦点闸每批击键前都发，写 3s 实得 60s）、`screen.capture`（**有 xcap 回落**，所以这个数字等于"卡死的 helper 让一次本可瞬间成功的截图多等多久"）。现单一源＝`methods::suggested_timeout_ms`（**按命名空间兜底**，新方法自动继承合理档位而不是掉回一分钟）→ `bridge::client::rpc_timeout_for`；改死线只改协议那一处。同族纪律：**任何"推荐值"常量加进协议时，同一笔改动里必须有消费者**。
>
> **⚠️ AX 的三件事都必须可见**: ① **节点预算是协议的**（`ax::DEFAULT_MAX_NODES`=1500，天花板 10 000）—— 曾经 macOS 10 000 / Windows 4 000 / Linux 1 500 三个私有常量各自**静默**剪树，而模型拿到被剪过的树只会得出"我要找的控件不存在"；结果必须带 `node_count`/`truncated`，`ax_snapshot`·`set_of_marks` 还要把"**列表**被截"(`truncated`)与"**遍历**没走完"(`tree_truncated`)分成两个字段。② **`ax.query_focused` 的 `pid` 是问题本身不是过滤器** —— 定向轨（macOS 默认轨）把击键投进一个**不在前台**的进程，读系统焦点就是在看错窗口，「密码框永远拒绝」因此只在目标恰好是前台时成立；契约是"给了 pid 就只能回那个进程的元素，回不了就 `None`"。③ **`AXUIElementSetMessagingTimeout` 要在每个句柄进入遍历的地方设** —— 该设置 per-element 且**不被 copy 出来的子元素继承**；`AxQuerier` 是 actor，漏一处就让一个卡死的 app 把整条 AX 排在它后面。
>
> **⚠️ macOS「一个 app 名字是什么意思」只有一个答案**: `desktop/shared/src/macos/app.rs`（真源在**被依赖的一侧**，同 `linux/`）。此前 `action::app_launch` 与 `desktop-macos::system::workspace` 各写一份，quit 一个只认 bundle id、一个名与 id 都认 —— 用名字开的 app 用同一个工具关不掉。剪贴板同款双源，且 `action` 那份**丢弃 `setString:forType:` 返回值**把被拒的写报成"已复制"。另：`terminate()` 回 `true` 只表示 Apple Event 送到了，**必须轮询 `isTerminated` 验证**（存盘 sheet 会让 app 原地不动）。

> **⚠️ 「这个动词没有定向对应物」不是让它绕过输入闸的理由**: 桌面输入的 fail-closed 闸（`[desktop] allow_global_pointer`）只作用于 `native.rs::is_input_action` 名单内的动词。`key_button` 曾以「held key 没有定向轨」为由留在名单外 —— 于是默认配置下模型被拒了 `key_combo`，转手用 `key_button {press_action:"click"}` 把**同一记击键**照送进用户的前台窗口，结果里连 `delivery` 都不报。**任何合成输入事件的新动词必须同时做三件事**：进 `is_input_action`、按 `rail` 分发、结果里报 `delivery`；没有定向实现就在 trait 里留 `default → NotImplemented`（`choose_rail` 读 `supports_targeted_input()`，无轨平台自动保持原样），**不要靠把动词移出名单来"保持可用"**。另一半：任何**跨调用留住物理资源**的动词（按下不放）必须把走过的轨记进 `held_inputs` 账本、**在同一条轨上释放** —— 全局释放一次定向按下，等于目标进程的键永远抬不起来、同时朝用户正在打字的窗口甩一记杂散释放。详见 FEATURE_LOCATOR §7.3。

> **⚠️ Panel ↔ Daemon 资源嵌入链**: Panel UI 经 `rust_embed` 在 `aleph-server` **编译时**静态嵌入二进制，运行中的 daemon 不读磁盘 dist/*。改完 panel 看不到效果＝漏了重编 binary。完整刷新链（`just wasm` → 重编 server → 替换运行中 binary，dev / macOS .app / Windows 三种 daemon 替换法）详见 [DESKTOP_SHELL.md](docs/reference/DESKTOP_SHELL.md)。

> **⚠️ hook 注册了 ≠ hook 会触发**: hook 有**三个各自独立的静默死因**，且**没有一个会在触发时报错**——它只是不跑。① `matcher` 挂在**没有 tool_name 的事件**上（matcher 只测 `tool_name`，所以 SessionStart 上的 matcher 永假）；② `"kind":"interceptor"` 挂在**只派发 observer 的缝**上（message / provider / gateway / subagent 全局 fire-and-forget 缝）；③ shell/http 动作的 **consent 还是 pending**（前两个至少有加载期 `warn!`，第三个连 warn 都没有）。**唯一的诊断入口是 `hooks_manage(action='list', only_unreachable=true)`**——它读**运行时**清单（`HookExecutor::inventory()`），一次说全三个死因。**别去翻 `~/.aleph/hooks.json`**：那是四层里的一层，project / project-local / plugin 自带的 hook 在它眼里根本不存在，解析后的 `kind` 也看不到——`hooks.list`＝文件视图（`add`/`remove` 改的就是它），`hooks.registry`＝运行时视图，问"实际注册了什么"只能问后者。可达性谓词**只有一份**（`HookEvent::supports_matcher()` / `supports_interceptor()`，`types/hooks.rs`），加载期 warn 与 `reachable` 读同一个；事件全集同理只有 `HookEvent::ALL`。**两个上限别混**：`executor.rs` 的 64KB 罩**读 stdout/HTTP body**、截断＝硬错误（fail-closed，怕 `deny:` 被吞）；`output_budget.rs` 的 ≈2500 token 罩**进模型上下文**、超限溢写磁盘留路径（fail-soft）。**consent 绑的是脚本内容不是命令字符串**（approve 那一刻的哈希）——脚本改了＝按未批准处理，所以"我明明批过了"的第一问是"脚本动过吗"；而**任何工具都不能批准 hook**（能报告状态、不能授予），否则被注入的模型可以一回合内自写自批。详见 FEATURE_LOCATOR §5.10。

> **执行清单 (Execution List / Todo-Plan)**: 模型把任务分解成一条**带三态的清单**（`- [ ]` / `- [~]` / `- [x]`），落在该 agent 的 scratchpad markdown 里，同一份快照喂五个消费者——run 起点的 `<execution_plan>` prompt 块、模型（工具回显）、用户频道（progress push）、Panel Todo 条、`ScratchpadGoalVerifier` 停机守卫。**线上单一形状是 `shared/protocol/src/plan.rs::PlanSnapshot`**，唯一转换点 `builtin_tools/scratchpad.rs::plan_snapshot_dto`——加字段先动协议，别在 Panel 手抄第三份（曾并存三套 plan-step 词汇，其中两套零 producer，已 CUT）。写语义是**全量替换 + 状态保真**：裸文本项按文案继承旧状态（refinement 幂等），`{text,status}` 原样采信，单-in-progress 由代码强制。**分解本身 100% 归 LLM（R7）**——任何"判断这一步算不算完成"的代码都越线，守卫只能查模型自己的方框。**不要新建 `todo` 工具**：Panel 投影按字面工具名 `"scratchpad"` 取数，第二个工具＝第二个 store + 对现有 UI 完全隐形。详见 FEATURE_LOCATOR §3.13。

> **⚠️ 「提前返回的快路径」会静默吞掉请求上除它自己之外的一切**: `try_inject_steering` 把一条消息注入活跃 run 后返回 `HandledInline`，`execute()` 就此提前返回——而 gate **之后**才解析的东西（斜杠 L0 快路径 / skill·allowed-tools 覆盖 / 逐轮模型指令 / 媒体处理）**一个都没跑**。它此前只挡住了附件，于是被 steer 的 `/moa on` 变成转录里的**字面串**（零事件、零报错、模型把它读成中途插话），中途换模型的 pill 显示 `opus` 而答案来自 `sonnet`。**判据不是「这条路径会不会崩」而是「它跳过了哪些本该发生的解析」**——凡新增 per-request 的指令字段（metadata key / `*_override` / 附件类），同一笔改动里必须问它在**每一条提前返回的路径**上会不会被丢掉，并在唯一的 defer 谓词里登记（`steering.rs::carries_more_than_text`，defer 到车道重投 ≠ drop）。同族：本模块**早就**在反方向剥掉同一份 metadata（`build_steering_rescue_request` 注释写明「rescue 绝不能再进快路径」）——**一个方向被想到、反方向没有**，是这类缺陷最常见的形状。另：这两条只在 2026-07-29 `mark_admitted` 落地后才**可达**（此前运行中那条消息的票占着队首，后续消息根本到不了 `admit_run`）——**修好一处会让它下游从没跑过的路径第一次真正跑起来**，那不是回归，是新暴露面，值得在同一轮重审。详见 FEATURE_LOCATOR §4.8 Round-7。

> **⚠️ 「按会话的状态」住进单例组件＝切页签就串味**: Panel 的 composer 是**一个前台组件**，读的是被投影的单例 `ChatState`；而队列、附件盘、草稿、stop 抑制位**都属于会话**。任何一个留在 composer 局部信号里，都会在切页签时错位——草稿留着而附件盘换掉；在 A 里按停止会花掉欠 B 的那一次 drain 抑制；把「离开繁忙会话、打开空闲会话」读成 busy→idle 从而**当场点燃新会话的队列**；反过来在后台落定的会话**永远拿不到边沿**（两个 drain 触发器都在这一个前台组件里），队列就此搁浅。判据：**这份状态在用户切到另一个对话后还成立吗**？不成立就进 `SessionSnapshot`，边沿判断按 `ConvId` 记（单一源 `shared_ui_logic::state::composer_queue::was_busy_across_switch`）。同族：**预填通道（写一个信号、指望别处排空）在多平台下必然漏一个消费者**——`draft_seed` 在手机端就没有排空点，于是点 ghost 气泡把消息从队列里取出来、扔在地上，而**没有任何报错**；存储没有这种失败模式，所以正解是让状态直接住在 `ChatState` 上、只留一个入口（`seed_draft`，合并不覆盖），而不是「再记得加一个消费者」。**⚠️ 但「进了 `SessionSnapshot`」只是必要条件**（2026-08-03 真机 QA 挖出）：侧栏打开另一个会话走的是 `activate`（快照往返）**紧接着一行** `clear_session()`，而后者清的 21 项里有 15 项正是快照刚恢复的——于是本轮新加进快照的 `draft` 在**最常见的切换路径上从出生就是死的**，草稿与排队消息一次点击就永久消失；`active_run_id` 被清还让 Panel 以为会话已空闲（Stop 键消失、新输入不再排队、下一次 Enter 在**仍在生成**的会话上开出第二个并发 run）。**给快照加字段时必须走一遍「恢复之后还会跑什么」**，真正要手清的只有快照**不**携带的那部分。同源第二例是**顺序**：composer 的 auto-grow Effect 读 `scroll_height`——打字时浏览器**先**改 DOM（读到新值），程序化写信号时框架**在 Effect 之后**才刷 DOM（读到旧值），所以那句注释声称覆盖的六种程序化重写**一种都没生效**（空 composer 保持两行高、多行撤回被裁成一行）。判据合成一句：**「A 之后紧跟 B」时算数的是 B——而两段代码各自都对，读代码看不出来，只有真机看得出来。** 详见 FEATURE_LOCATOR §4.7 Panel 段与 §4.8 Round-7 真机 QA。

> **⚠️ 连线的守卫测试要断言「效果到达了」，不是「调用发生了」**: 2026-08-02 一轮在工具层挖出三条断线，**三条都有绿测试守着**。`ScopedToolService::list()` 里的 `let _ = refresh.fetch_tools();` 让每一个运行时安装的 markdown CLI skill 在整个进程生命周期不可调用，而守它的测试断言的是 `fetch_tools must be called when poll_changes returns true`——在返回值被丢弃的整段时间里一直绿。tool-health 闸的两条腿（`with_health` 零生产调用点 / `ToolHealthCache::refresh` 零生产调用者）同理：probe 注册成功有测试，probe **从没跑过**没人测，而**三处注释宣称这个闸活着**，其中一处点名了一个从未存在的驱动函数 `trigger_health_refresh`。**判据**：写测试时问「如果我把这一步的返回值扔掉，测试还绿吗？」绿 ⇒ 你守的是产地不是连线；要断言的是**消费者那一端拿到了**（工具能按名解析并派发 / 不健康的工具真的从 `list()` 消失）。同族反面教材：`fallback_registry` 让模型 `switch=autocli`（从未注册过的名字，照做必吃 `NotFound`，跨批备忘录再把幽灵拉黑），`gather_budget` 说「你已成功搜了 N 次」而计的是尝试次数（12 次被限流的搜索之后，同一个 prompt 里 `attempt_summary` 说爬阶梯、它说别搜了去交付）。**任何"零消费者"的通道优先 CUT 不 CONNECT**（R10）：`AlephTool::examples()` → `ToolDefinition.llm_context` 85 生产者 0 消费者，整删 90 文件 −976 行；真正承重的内容按 R9 第二把尺搬进那个工具的 `DESCRIPTION`（随 schema 发，且只发给真能调它的请求）并用守卫测试钉住 const 散文与 enum 不漂移。详见 FEATURE_LOCATOR §3.5 与 §3.2。

> **⚠️ 前缀缓存：门只写在注释里就等于没有门；断点押在活不过下一轮的字节上就等于没有断点**: prompt 前缀缓存是本仓**唯一一类症状只出现在账单上**的失效（provider 按 ~10% 计缓存命中、按 **1.25x** 计缓存写入，而两者都"正常返回"），所以它的每一条纪律都必须写在这里而不是等人从日志里发现。七条，逐条都是"看起来在把关，其实没有"：① **预置 marker 绕开所有门**——`split_system_blocks_for_cache` 用 `SystemBlock::cached_text` 无条件给 stable 系统块盖章，而 `cache_retention` 门与 `supports_cache_control` 能力位都在下游、且只管**追加注入**；于是 `off` 照样付 `cache_creation`（**关掉反而更贵**）、Custom 类代理照样被 400。**"跳过注入"永远不等于"没有 marker"，否定分支必须显式 `strip_cache_control`**；别用"把 `cached_text` 改成 `text`"来修——`inject_cache_control_into_system_array` 走 `.rev()` 找**最后一个** text 块，那是 dynamic 尾，断点会落到每轮变化的字节**后面**。② **dynamic 系统尾不是免费区**——Anthropic 按 tools → system → messages 建前缀，所以未打标的 dynamic 块**排在每一个消息级断点之前**；它只是不打断*系统块自己*那个断点。曾有两处注释断言相反（"changing bytes cost only themselves"），三个模块据此被分区进 dynamic 尾。真正的规则一直写在 `harness/deps.rs` 里（那正是 recall 被移到 transient message tail 的理由）。③ **断点只该押在下一轮仍在那个下标的字节上**——`harness/agent/prompt.rs` 在真实历史后追加最多 4 条**从不持久化**的 `<system-reminder>` 提示，而断点预算从尾部往里花：三条提示同时触发就把三个消息级断点全押在结构上不可能复现的位置，且恰在长的、失败密集的循环里触发。跳过它们（**不消耗预算**）即可；**把断点往前放永远安全**，只缩短覆盖跨度不作废它。⚠️ **同一条尾巴还骗过了第三层**（2026-08-04，§2.19 ①）：「这条尾部 user 消息是不是合成脚手架」在仓里有三个独立答案，而 context 层的 `latest_user_task` 只学过 `<memory-context>` / `<live-status>` 两道 fence——**白名单式的判据必然漂移**，压缩摘要因此把 `<conversation_focus>` 锚成 "MAXIMUM ITERATIONS REACHED"，且只在长的、失败密集的 run 上触发。**判据只能是「这段字是谁写的」，不是「它带什么标记」**（单一源落在产地 `thinker/nudges.rs::is_synthetic_reminder`）——而**一刀切按 fence 是错的**：`user_interjection_note` 用同一道 fence 包**真实**用户 steering 消息。④ **`stability()` 曾经默认 `Stable`——2026-08-04 起没有默认体，不表态就编译不过**（移植自 codex `client.rs:307-357` 的穷尽解构惯用法，见 §2.19 ⑥）：此前忘记声明的 layer 靠"省略"骑进可缓存前缀（`ToolRuntimeStateLayer` 就这么让一次 30 秒 TTL 的健康探测作废整段会话，且是**人读代码**发现的——不是任何机制抓到的）；**问题现在在作者唯一知道答案的时刻提出**。⚠️ 但这道门只管"有没有表态"，**管不了"表态得对不对"**——⑤⑥⑦ 三条讲的全是表错的情形，它们照旧要人判断。守卫在 `thinker/prompt_contract.rs`：建两次断字节相等 + per-run 事实（cwd/repo/model/time/**sandbox writable roots**）改变时 stable 前缀必须不变**而 dynamic 尾必须变**（后半句防止第一条空过）。**加 Stable layer 前先跑这两条。** ⑤ **判据是「这些字节会不会变」，不是优先级、不是内容归属、不是"从盘上重读"**（2026-08-03）——第 ② 条的推论此前只被读成"别把易变的放 Stable"，反方向同样要命：dynamic 块**没有自己的 marker**，只被那些排在它之后的消息级断点覆盖，所以停在那里的**不变**字节虽不**造成**未命中，却照样**为**未命中付钱（任一易变邻居一动，整块按 1.25x 重写）。分类实际已退化成"priority ≥ 1700 归 Dynamic"，于是 `agent_catalog`（实测 **1705 B**，全 prompt 最大的一层）、`identity_files`（默认上限 **100 000 字符**）、`extra_files` 三层长期在替邻居交税；`memory_protocol.rs::stability` 还写着 *"byte-identical dynamic bytes never re-key the prefix"* 并**点名 `SessionContextGuideLayer` 为先例**——那是第 ② 条推翻的读法的第四处，且是**分层决策的依据**（前三处只是注释），在扩散。**`stability()` 说内容变不变，`priority()` 说阅读顺序，用其一推另一（两个方向都是）就是层跑错区的成因。** 新守卫 `dynamic_tail_bytes_ratchet`（实测 2054 B → **1017 B**，见 ⑦）把它变成可回归的门；抬它只有一种诚实理由——某层**真的**逐轮变、必须搬出 stable 前缀。⑥ **守卫在场 ≠ 判据完整**——`stable_prefix_ignores_per_run_facts` 从建立起就只 shift `runtime_context`，而 `sandbox_summary` 同样是每轮设置的 per-run 输入，`SandboxSummary::isolated_worktree` 每个隔离 run 一枚新 UUID，`SecurityLayer` @600（当时**未声明 stability ⇒ 默认 Stable**；④ 的默认体已删、它现在显式写 `Stable`——**但那不救这一条：表态成 `Stable` 与忘记表态，字节后果一模一样**）就这么把它渲染进可缓存前缀，团队扇出 N 个子代理各写一次整段前缀。**凡因为"每次生产请求都设置它"而被加进 `resolve()` 的字段，必须同批加进那个 shift**，否则原样重开这个盲区。⑦ **一层只有一个 `stability()`；两半分歧时答案是拆层，不是取更差的那个评级**（2026-08-03 Round 4）——`memory_protocol` 同时装着**常量**目的地阶梯和**逐轮变**的窗口声明，于是整层按易变的那一半判 Dynamic，1037 B 常量长期骑在无 marker 的块上。拆成 `memory_protocol` @1105/Stable + `memory_window` @1745/Dynamic 后棘轮 2054 → **1017 B**，**prompt 正文一字节未变**。两条配套教训：**(a) 归因会反**——立项时以为砍掉的是"常量那三分之二"，实测 production-shaped 输入下窗口声明的两个门**都是 false**，所以那 1037 B **全部**是常量、被它连累的那句贡献 0 B：**`stability()` 问「哪些字节*会*变」，棘轮量「哪些字节*在*渲染」，棘轮只回答后者**。**(b) 层数不变的改动守卫看不见**——走一个来一个，dynamic 层计数断言全程为真；真正钉住的是按名字的 `contains` + 字节棘轮。**另一半是 `CONDITIONALLY_SILENT` 上的 Dynamic 层：那张表只说「没有*固定*输入唤醒它」，不说被唤醒时它有多大，而棘轮量的正是那个固定输入 ⇒ 读数恒 0 B。** `graph_topology` 因此把每个 Root 的 `body` **逐字无上限**插进 1.25x 区（`identity_files` 的形状，连 cap 都没有），而"留在 Dynamic 是对的"这个**位置**判断被当成了"因此没问题"这个**大小**断言。现钳位在 user text 进入渲染的那道缝（handle 80 字符 / root body 600 字符，**截断出声**——静默截断会让模型遵循一条它看不见后半句的根参照），恶意图形 2256 B 实测封顶，普通图形逐字节不变。**纪律：表上每个 Dynamic 层都欠一个自己的界**——层自己造文本就 assert 在层里，层只是透传就 assert 在**生产者**里。 另：**任何按 HashMap 迭代序进 prompt 的集合都是缓存炸弹**（内容逐字节相同、顺序不同 ⇒ provider 读作不同前缀），`aggregate_from_healthy` 是单点修复覆盖 instructions/tools/resources/prompts 四个聚合；**任何"重建整个配置结构体"的 RPC 都会静默吃掉 DTO 表达不了的字段**（`providers.update` 曾如此吞掉 `cache_retention` 等十余个，运维改个颜色就掉回 5 分钟 TTL），正解是**以已存条目为基线只覆写 DTO 能表达的字段**。看门狗侧：`CacheMonitor` 若"任何非零 read 就清零 streak"，在本仓实际出货的 **1 系统块 + 3 消息**布局下**永远攒不到告警**（小系统块几乎恒命中），判据必须是**读主导**（`reads >= writes`）。详见 [FEATURE_LOCATOR §2.18](docs/reference/FEATURE_LOCATOR.md)（含 DeepSeek-Reasonix 逐机制对照表与 10 条已定位未做项，改这层前先看，不必重做对比）。

> **⚠️ 能被精确回答的数字，别用常量猜——先问仓库里有没有人已经知道它**: `src/providers/moa/` 的顾问视图预算是一个 `120_000` 字符的平摊常量，它的注释精确描述了要防的失败（「advisor 用比聚合器小的模型时长跑后每轮全体 400」），而**恰好能回答这个问题的单一源 `model_catalog::resolve_context_window(model)` 就在同一个 crate 里**，从未被调用。这是「能力接上了 ≠ 它在跑」的数字版，且比断线更难发现：**猜出来的常量在大多数情况下是对的**，只在边缘塌陷，测试全绿、日志无声。**判据**：写下一个魔数之前问「这个数是不是某个已知事实的近似？」是 ⇒ 去接那个事实源，把常量降级为天花板/地板。**配套第二问**：换算单位有没有单一源？`chars ↔ tokens` 是 `context::budget::pressure::chars_for_token_budget`（内容感知——同一 token 预算买到的英文字符约是中文的 3.5 倍，平摊比率等于给中文会话超配 3.5 倍，正好在它用户说的语言上触发那个 400）。**同族第三条：占位符不是内容**——失败/超时/熔断跳过留下的 `[failed: …]` 本是给聚合器的信号，却被无差别拼进「Use the advisor responses below as private context」的编号列表里；全灭时那句指令指向一墙不存在的建议，且熔断退休死槽后**每轮原样重印到 run 结束**。判据是**结构化的、在产出那一刻置位的位**（`AdvisorOutcome.advised`），不是事后嗅探 `[failed:` 前缀（措辞一漂移就静默误判）。另：**原始 provider 错误正文入 prompt 必须有预算**，而同一条错误的 trace/面板副本**不该有**——一个给模型读、一个给人排障。详见 FEATURE_LOCATOR §4.9 第八轮。

> **⚠️ deny 检查有方向：只向下问「我在不在保护区里」，没人向上问「我下面有没有保护区」**: `path_is_denied` 是向下的 `starts_with`，而 denylist 里有 `<config_dir>/secrets.vault`、`<config_dir>/data`、`~/.ssh`，**从来没有 `<config_dir>` 或 `~` 本身**。于是**对父目录的操作整类逃逸**：`copy ~/.aleph /tmp/backup` 把 vault 和配对数据库拷到闸外（递归里 deny 复检只在 symlink 分支内，普通文件目录直穿 `fs::copy`）；`delete ~/.aleph` 直达 `remove_dir_all` 抹掉同一批文件，而**直接删那个受保护叶子是正确拒绝的**；`move` 是它的非破坏孪生，一次 `rename` 把整棵树搬出去。修复是 `path_utils::contains_denied_descendant`（向上问），**必须共用向下检查那份展开+归一化**——写第二份就是这个仓库反复被咬的"重复真源"。**推论**：任何按路径授权的检查，加规则前先问「这条规则对**祖先**成立吗」；`copy`/`move`/`delete`/`organize` 这类会**遍历**的动词，顶层闸永远不够，逐项复检是唯一看得见后代的地方。另：`path_is_denied` 每调用一次就重算整张 denylist（每条 entry 一次 `canonicalize()`），而 `search`/`stats` 在 glob 里每命中一个文件调一次——已按 entry 字符串进程级 memo（条目由 home/config 在启动时派生、进程内不变，这是前提；代价是若某条 entry 底下的 fs 形状中途变了，memo 陈旧且陈旧落在**拒绝**一侧）。详见 FEATURE_LOCATOR §3.4。

> **⚠️ 取消不是判决 —— 按一次 `/stop` 曾把被停掉的那次调用拉黑一整个 run**: 取消经 `ToolError::Execution` 返回，而 `is_retryable()==false` 恰是 harness 跨批失败备忘录用来**封禁**一次调用的谓词（`act.rs` 自己的注释：「Only a NON-retryable failure enters the cross-batch memo」）。于是用户按一次停止 ＝ 那个 `(tool, args)` 在整个 run 里被永久拒绝，同时 `render_persistence_hint` 还附上「climb the ladder before fail」的阶梯话术——**关于一次根本没有发生的失败**。Aleph 早就为 `ApprovalExpired` 讲过这个理（"nobody said no — nobody said anything"），取消说得更少却漏了。现有 `ToolError::Cancelled` + `ToolErrorKind::Cancelled`，在唯一知道 token 已取消的派发咽喉 `scoped/dispatch.rs` 归因（**不管工具自己说了什么**——同一瞬间真失败的工具也归给取消，因为封禁一个被用户叫停的调用比放过一次真失败更贵），排除出一次性重试（run 已停，重试只会睡完退避再失败），并抑制阶梯话术。**推论**：任何「把失败记成关于这次调用的判决」的机件，都要先问**这次失败是不是关于这次调用的**——墙钟超时、传输抖动、审批过期、用户取消，四者都不是。详见 FEATURE_LOCATOR §3.3。

> **⚠️ 一个会 park 的工具必须听取消令牌，而「进程全局的表」必须按会话作用域交出去**: 两条互不相干、都在 §4.11 round-8 撞上的通用纪律。**(a) park 就要能被叫停**——`LoopTool::execute` 收到的 per-call `CancellationToken` 在 `subagent wait` 臂**零消费者**，于是一次 `/stop` 落在正 park 的 run 上要再等最长 **600 秒**才生效（`bash` 的 `process_action=wait` 是同族形状，只是窗口小）。判据很简单：**这个 await 的最长睡眠时间，就是取消的最坏延迟**；超过一两秒就必须进 `tokio::select!`。返回值要用**成功态**表达「被打断」而不是 Error——什么都没失败，报成失败等于把判决写进 harness 的失败计数器和跨批备忘录（同上一条）。**(b) 按 id 取 ≠ 按枚举取**——`BackgroundAgentTracker` 是**进程全局**单例，`flat_nodes` 早就带 `root_session` 过滤，而同一张表的 `list_running()` / `all_completed()` 没有：一个会话的模型因此能列出**别的会话**在跑的 request_id，进而 `check_status` 读它的产出、`cancel` 停它的活。**模型能拿到的 id 只有两个来源——自己 spawn 的返回值，和枚举面**；所以作用域只要加在枚举面上就够，不必去改按 id 的语义。推论：任何进程全局的注册表，新增**枚举**入口时都要问「调用者凭什么看见这一行」，`None`（看全部）必须是显式的、留给真的没有归属会话的调用者。同批第三条：**`list` 是目录不是内容**——它曾把 256 条完成项的**完整输出**一次性渲染进结果，全文永远该留在按 id 的那条路上。详见 FEATURE_LOCATOR §4.11 round-8。

> **⚠️ 工具结果的内容感知清洗必须跑在扁平化之前 —— `Value::to_string()` 把整个结果压成一行**: 每个 builtin 工具返回 typed struct ⇒ dispatcher 拿到 `Value::Object` ⇒ `apply_layer_two` 用 `Value::to_string()` 扁平化成**紧凑单行 JSON**（`stdout` 里每个 `\n` 变成两字符转义）。而 Aleph 的两个内容感知清洗器都按行工作：`structured::classify` 要求 ≥8 行、`distill_output` 遍历 `text.lines()`。后果是**双重静默失效**：log/search/diff/json 四个缩减器对**所有 builtin 工具从不触发**（⚠️ 原文这里写「只在 MCP 工具上生效——那边返回裸 `Value::String`，带真换行」，**2026-08-04 实测证伪**：`mcp_adapter` 交给 dispatcher 之前先 `serde_json::to_string`，所以 MCP 拿到的也是一行转义 JSON、同一个病，见下方「同一个病有三个宿主」），而号称"把关键错误内联到 marker 上方"的 `inline_error_digest` 展示的是 **JSON 信封头部**（`{"success":false,"exit_code":101,"stdout":"\n running 2001 tests\ntest…`），编译错误/panic 一个不剩。**故清洗点是 `src/tool_output/hygiene.rs::clean_result_value(&mut Value)`——字段级、在 `out.value` 上跑，绝不要"简化"成对扁平化后的字符串跑。** 三条配套铁律：① **持久化的永远是未经清洗的原文**（`apply_result_budget` 的 `reduced_from` 参数），否则缩减不可逆、`ctx_search`/`read_file` 挖不回被丢的行；② **只有类型路由成功时才把信号内联**，opaque 输出保持"仅 marker"——分不清信号与噪声时 head/tail 切片只是猜；③ **缩减器只判定"什么是信号"，不判定"是否更小"**——量纲由中央 `Reduction::is_meaningful_shrink` 按**字节**管（各缩减器旧的局部守卫全都数**行**，而一条 200 KB 的 minified 命中行是"94% 行缩减 + 1% token 缩减"）。同族三条（都是自审轮实测挖出的）：**(a)** `file_read` 的窗口必须自己守住 token 预算，且**量在下游真正强制的那个字符串上**——扁平化后的结果是**一行、含 `{`**，`looks_like_code` 恒真、恒按 CODE_RATIO 计费，所以按**文件自身**比率定尺会给散文/CSV 超配 1.4 倍，通用 head+tail 截断器照旧从**中间**切掉，而 `message`/`returned_lines`/`truncated` 早已按整窗铸好 ⇒ 模型按 `offset=N` 往后翻会**永远跳过那个洞**（单一源：`pressure::chars_for_result_token_budget` + `text.rs::read_window_tokens()`，后者夹到 `result_processing::read_backstop_tokens`，小窗口模型的 ceiling 能把它压到 2000）。**(b)** 「`[Full output persisted:]` 在 byte 0」不成立——inline 正文在它上方，任何「这条已经有 marker 了别再剪」的判断必须走**逐行**单一源 `result_store::extract_persisted_ref`（`starts_with` 会把缩减正文连同唯一的回溯 handle 一起剪掉）。**(c)** `list`/`search`/`stats` 的集合必须有 cap 且**聚合独立于 cap 累加**（`stats` 曾因 115k token 的 per-file 数组把自己刚算出的四个总数挤出上下文）。
>
> **⚠️ 同一个病有三个宿主，只治了一个（2026-08-04 第二轮）**：上面那条铁律当初只在 builtin 这一个宿主上落地。**① MCP**——`mcp_adapter` 先 `serde_json::to_string(&output.value)` 再围栏，于是四个缩减器在 MCP 路径上同样全瞎，且 `distill_output` 会把那 3 行渲染成一份**假摘要**替换整个结果。**② 每工具压缩器**——`compressor.rs` 的三个策略（`compress_snapshot` 读行、`compress_network_requests` 读 JSON 数组、`compress_screenshot` 读 base64）全都被喂了围栏后的信封：快照被 `cap_line` **静默砍到 500 字符**、网络清单退化成盲目 head 截断、兆字节 base64 原样进预算。**「接通了谁来跑」不等于「接通了跑在什么上」**——`devtools_tool_name` 那次修复只做了前一半，而测试全绿。现两个 stage 同为**字段级**并共用一个走查器（`tool_output/walk.rs`），策略入口收敛到 `tool_output/ingress.rs::clean_for_ingress`。
>
> **⚠️ 围栏是结构不是内容 —— 任何重写文本的 stage 都不许碰它**：`<<<EXTERNAL_UNTRUSTED_CONTENT id=…>` / `<<<END_…>` 两行标记是**唯一**告诉模型「以下不可信、到此为止」的东西，而 ingress 清洗此前**整体替换字段**，于是标记随之消失、或只剩开头那条（`reduce_log` 的 `KEEP_HEAD` 恰好会留下它——**半个围栏比没有围栏更糟**，模型读到一个没有终点的不可信区）。命中面正是最该有围栏的那些：`web_fetch.content`、`browser_*` 抓取正文、MCP 结果，**且只在它们大到触发清洗时才发生**。单一源是 `content_sanitizer::split_external_fence`（严格：首尾行必须是标记、id 必须配对、内部不得再出现标记——否则两段拼接的围栏会被重新缝在错误的边界上）+ `tool_output/fence.rs::rewrite_interior`（清洗与压缩共用）。**推论适用于任何「把一个大围栏拆成若干小围栏」的改动**：拆完必须回答「原来在大围栏里、现在落在小围栏之间的那些字节，还被谁覆盖」——本轮的答案是新的 `sanitize_external_text`（＝`wrap_external_content` 去掉标记那一半，同一份 fold/strip/escape/scrub），所以 MCP 的 `uri`/`name`/`description` 没有掉出覆盖面；`data`/`blob` 刻意不碰（base64 要能解码，且它的字母表表达不出 chat-template 标记）。另：**闸下沉到它约束的那个东西里**——`distill_output` 拒绝单行输入的判据此前是各调用方各打一次补丁（`inline_error_digest` 打了，`reduce_field`/`clean_error_body` 没打）；行式摘要器不能摘要一行，这是它自己的前置条件。详见 [FEATURE_LOCATOR §3.14](docs/reference/FEATURE_LOCATOR.md) 与 §3.4。

> **⚠️ 「按构造不可能发生」的 DEFER，是一条等着被真实负载证伪的推理（2026-08-04 真机 QA，隔离 `ALEPH_HOME` + 真 chrome-devtools-mcp + 记录请求体的 mock LLM）**：上一轮把「压缩后的正文被当 Full output 持久化」判为 DEFER，理由是「压缩产物按构造远低于任何预算，`persist_if_large` 走不到」。真实 `take_snapshot` 压缩后 **13 585 token / 8 000 预算**——当场开火，落盘的是**压缩后的正文**，被丢掉的 443 个节点 `ctx_search`/`read_file` 永远挖不回来，正是 `reduced_from` 存在要防的那件事。根因是那个"构造"只对**有上限的**策略成立（`compress_generic` 封顶 10 KB），而 `compress_snapshot` **保留全部交互节点、没有上限**。**判据：一个 DEFER 如果建立在「这条路走不到」上，它就欠一次真实负载的实测**——猜出来的边界只在边缘塌陷，而边缘正是真机所在（同族＝§4.9「能被精确回答的数字，别用常量猜」的反面：这次是把**没有上限**当成了有上限）。同轮另外两条同样只有真机看得见：**(a)** `compress_snapshot` 的角色匹配写的是 Playwright 的 role-first 文法（`- button "Save"`），而它整张工具名表来自 chrome-devtools-mcp，那台 server 每行都带句柄（`uid=1_4 button "Apply 0"`）⇒ **真实快照一条交互行都匹配不到**，1103 个节点只剩前 20 行、一个控件都没有——**而这条在上一轮把压缩器接通之前不可达**（压缩器当时只见得到序列化信封，任何文法都一样失败）：**修好一处会让它下游从没跑过的路径第一次真正跑起来，那不是回归，是新暴露面，值得在同一轮重审**。**(b)** `ToolResultStore::new` 自己 `dirs::home_dir()` 而不走 `utils::paths` ⇒ 隔离 `ALEPH_HOME` 起的实例把 offload 文件写进**真实的** `~/.aleph`——读写同源所以不报错，但这条路上**唯一比 run 活得更久的产物**掉出了用户选定的 home。**另一条已定位·未做**：`metadata.images` 只有 **Anthropic 协议**消费（`anthropic/proto_impl.rs` 把图片作为同一 user turn 的尾随块发出），`openai_chat` 与 `responses` 的 ToolResult 臂都是 `_ => None` ⇒ 图片静默消失（Chat Completions 的 `role:"tool"` 装不下图片，是 API 约束；补一条尾随 user 消息会给非 vision 模型开出新的 400 面，需配套设计）。诊断判据：工具结果文本里**有** `<N base64 chars returned to the model as a viewable image block>` 而模型看不见图 ⇒ 是协议那一半，不是 hoist。

> **⚠️ 「最小尺寸闸」的量纲必须和被闸内容的形状一致 —— 一个行制闸拦死了唯一一种没有换行的内容类型**: `tool_output::structured::reduce` 用 `MIN_LINES=8` 在**分类之前**拦掉所有候选，而 JSON 的规范线上形态就是**一行**（`Value::to_string()` / `curl` / 每一个 `--format json` / 每个 MCP 文本结果）——于是 `looks_like_json` **从来没被问过**，一份 300 KB 的 API 响应直接掉进 head/tail 字节切片，把 JSON 从中间切成非法语法。**行数是行制缩减器的前提，不是内容的前提**（现按 kind 走 `ContentKind::min_lines`，全局只留一条字节地板）。同一处另有三条同族，每条都不是「少做了一点」而是「看起来在工作」：① **镜像故障——它不是压不动，是压了并且撒谎**：`hygiene` 的 tier-2 缺少 `result_processing::inline_error_digest` 早就写下的那句「无换行的载荷不能被行蒸馏」，同一份单行 JSON（正文里出现 `error` 字样即可）被换成 **400 字符的信封前缀**并扣上 `[Output digest: 1 lines, 1 error]` 的帽子——**一个谓词如果两张脸都必须遵守，它就该长在两张脸共用的那个东西上**（现长在 `distill_output` 自己身上，四个调用点自动一致）。② **调用点手里的预算被丢弃**：`reduce()` 不收预算、reducer 用固定常量，于是声明 6 000 token 的工具拿到「什么都不针对」的 240 行 × 500 字符正文，`apply_result_budget` 再拿**瞎**的 head/tail 收尾——**知道哪些行重要的那个组件，必须也是决定能放几行的那个组件**（现 `Profile::for_token_budget`，换算单一源 `tool_output::scale_to_budget`，锚 `DEFAULT_RESULT_BUDGET_TOKENS` ⇒ 默认预算下逐字节不变、更大的预算不抬上限）。③ **文档化的预算钩子零使用者**：`OutputDigest::render(max_salient)` 的 doc 第一天就写着「调用者可以缩小它来遵守 token 预算」，而**每一个**调用者都传 `salient.len()`，声明 400 token 与声明 8 000 token 的工具拿到逐字节相同的摘要。**判据合成三问**：写「最小尺寸/最小数量」闸时问**这个量纲对每一种被闸内容都成立吗**；写第二张脸时问**兄弟那张脸拒绝了什么**；写一个**带 cap 参数**的渲染函数时问**有没有调用者真的传过小于「全部」的值**——没有，那个参数就是断线。另两条只在本层但同样反直觉：**紧凑进必须紧凑出**（`reduce_json` 恒 `to_string_pretty`，紧凑输入美化后**变大**、被字节守卫正确否掉，于是 reducer 做完全部工作而模型拿回退）；**「低于此就不值得剪」的那个尺寸，正是 stale pass 该剪到的目标**（`min_tokens_to_prune` —— 它竞争的对手是一行占位符，不是原文）。详见 [FEATURE_LOCATOR §2.7](docs/reference/FEATURE_LOCATOR.md)。

> **⚠️ `agent_trace` 流是有意有损的镜像**: `AgentTraceEmitSink` 用 bounded `mpsc(256)` + `try_send` 把 harness trace 镜像到 WS（满即丢，注释明写 best-effort），这是**刻意的**——绝不能让慢消费者背压 agent 循环。推论：**任何消费方都不得把逐事件流当作终态真源**。工具调用的权威终态在 `run_complete` 的 `summary.tool_summaries[]`（core 由 harness `tool_timeline` 构建，`tool_id` 与流事件同源 `call.id`），失败原因在 `summary.errors[]`，**执行清单（todo/plan）的终态在 `summary.plan`**（`aleph_protocol::plan::PlanSnapshot`，core 在自己那条不丢帧的 `event_drain` 里闩存）。新写消费者（Panel / channel / 外部 bridge）必须在流末对账，否则丢一帧就留下永久"进行中"的幽灵状态。Panel 侧参考实现见 FEATURE_LOCATOR §6.1（工具行 `reconcile_tools` + `settle_orphan_tools`；todo 条 `settle_plan`）。

> **交付物 ≠ 聊天记录 (Deliverable ≠ Transcript)**: Aleph 会生成**两种** HTML，混淆它们是这条注记存在的唯一原因。**交付物**＝模型主动调 `artifact_publish` 发布的成品（报告 / 分析 / 方案），落 `ArtifactOrigin::Deliverable`，在右栏置顶并**自动在系统浏览器打开**；**对话记录**＝`session.export_html` 手动导出的整段 transcript（按钮文案「导出对话」），是「给我看当时怎么做的」而不是「把结果给我」。把后者当结果递给用户，等于把答案埋进产生它的过程里。**什么算成品 100% 归模型判断（R7）**——`deliverable` 这个 origin 只可能由那次工具调用产生，任何"扫最终答案 / 看 run 结束了没"的启发式都越线。两者共用 `src/export/page.rs` 的文档外壳、CSP 与字节预算；**导出文档零 `<script>` 是硬约束**（与 Panel 同源，只有零 script 才配得上 `default-src 'none'`，加一个脚本就把这条兜底论证全部作废）。右栏**不再镜像工具调用**（与聊天列折叠内容逐字重复，旧 `components/inspector/` 整套已删）——想给右栏加面之前先问它是不是聊天列已经显示过的东西。**右栏一行能点开什么只有一个谓词**（`components/artifacts/preview.rs::PreviewTarget::for_item`：图片 / 可读文本走面内查看器，PDF·压缩包·已渲染文档保持外链），而"什么算可读文本"落在 `shared/protocol/src/artifact.rs::is_previewable_text` —— **offer 的一侧（Panel）与 serve 的一侧（`artifacts.read_text`）必须读同一份**，散成两份就是"点了永远报错"或"能读却只给下载"，两种都静默。⚠️ **右栏默认是收起的**（`LayoutMode::ChatOnly` + `translateX(100%)` + `pointer-events:none`）：任何长在面板里的提示（被拦横幅）在那个状态下等于不存在，所以"自动打开被拒"必须同时把面板打开；任何"面板里有新东西"的徽标必须数**面板真正装的东西**（`unseen_artifacts` 由产物列表差分驱动 —— 它曾经数的是工具调用，那是检查器时代的残线，为面板没有的东西亮、为面板真有的东西沉默）。另：**新增 `read_*` 一类只读 RPC 记得进 `gateway/lane.rs::override_for`**，后缀启发式不认它就落 Mutate 车道被幂等键守卫拒掉（只在 `require_idempotency_key` 部署上炸）。详见 FEATURE_LOCATOR §6.8。

> **⚠️ 记忆热区是一种格式，不是一篇文档；而它的读写两侧各有一个"看起来通了其实没通"的位置**: `MEMORY.md` 由 `\n§\n` 分隔——**任何把它当散文写的东西都会让 curated store 进 legacy 模式，而 legacy 是静默的**：文件看着正常、写侧测试全绿，唯一症状是 `remember(add)` 永远回 `LegacyBlocked`，同时那段占位散文**被当作一条记忆事实注入系统 prompt**。新 agent 的种子因此必须是**空字符串**——`\n§\n` 也解析成零条目，但只有 `""` 是 `parse∘serialize` 的不动点（sentinel 会在第一次成功写入时被改写）；别往种子里加表头、注释或示例条目，任何非分隔字节都重建这个 bug。存量 agent **不迁移**（`write_if_missing` 从不覆盖），它们靠 `remember(replace)` 自愈。**内容进 `<CuratedMemory>` 前必须 `xml_util::escape_xml`**：热区是 Stable 层，存一条 `</CuratedMemory>` 就在此后**每一次**请求里闭合信封——这是 prompt 注入的持久化版本，而 `content_scanner` 的 8 条规则全是 ASCII 英文、且它本就不是词法问题。**预算数 chars 不数 bytes**（`used_chars` 曾是 `String::len()`，中文热区只拿到 1/3 额度，而错误文案和 prompt 表头都写着 "chars"）。读侧两处：`invalidate_curated` 现在**同时驱逐 `curated_stores`**（否则它的 doc 声称的"拾取盘上改动"是假的，手改 MEMORY.md 要等压缩或重启）；`ALEPH_HOME` 下 resume 快照的**写侧与读侧必须走同一个 `utils::paths` 解析**——`constructor.rs` 曾调 `SnapshotReader::default_path()` 只取其 `Option` 判别、再用 `dirs::home_dir()` 重推路径，那不是"忽略旋钮"而是写这边读那边、整条断开。详见 FEATURE_LOCATOR §2.16 round-2。

> **⚠️ 白名单式的"这算不算合成消息"判据必然漂移，而正确判据是「谁写的」不是「带什么标记」**: 「这条尾部 user 消息是不是脚手架？」在仓里被**三层各自回答过一遍**——provider 层（`anthropic/adapter/cache.rs::is_ephemeral_notice`，跳过缓存断点）、harness（`build_prompt_with_transient_tail` 返回的权威**条数**）、context 层（`compact/summary_utils.rs::latest_user_task`，摘要 focus 锚）。§2.14 教会了第三层跳 `<memory-context>`/`<live-status>` **两道 fence**；§2.18 之后 `build_prompt` 又在真实历史后追加最多 4 条 `<system-reminder>`，**而喂给压缩的正是那个向量**（`think.rs:426`）——于是 `<conversation_focus>` 被锚成「CRITICAL — MAXIMUM ITERATIONS REACHED…」，摘要器据此认定那就是用户的任务，跑题摘要再经指纹缓存**逐轮回放**；且这些提示**只在长的、失败密集的 run 里点火**，正是最亏不起的那种。**⚠️ 而"按 fence 一刀切"是错的**：`nudges::user_interjection_note` 把**真正的用户中途 steering 消息**包在同一道 fence 里，且包的是 `!synthetic` 的持久化 `UserMessage`——一刀切会扔掉用户最近的指令（对 focus 锚是致命的），同时让 provider 层跳过一个**下标完全稳定**的好断点（镜像缺陷，方向相反）。故判据只能问**这段字是谁写的**，单一源必须落在**产地**（`src/thinker/nudges.rs::is_synthetic_reminder`，lead-in 由 `user_interjection_note` 插值同一常量，措辞与判据不可能分家）。**防漂移守卫必须断言在源码上**（`include_str!` 扫本文件每个开 fence 的 `pub const`，未登记即 FAIL）——**运行时分不出「没人登记的常量」和「不存在的常量」，而那正是本 bug 的成因**。推论适用于任何"这条数据是不是我们自己造的"判断：**列举法只覆盖立法当天的世界**。详见 FEATURE_LOCATOR §2.19。
>
> **⚠️ 同一个对象的第二个构造点，默认继承不到第一个的任何档位**: `ContextCompactor` 在 `runner_impl.rs` 拿到 carryover + summary_reuse + **cheap_provider** + monitor_scope，而子代理侧 `subagent_spawner::build_context_triple` **只拿到 monitor_scope**——于是 §3.1 Round 7 好不容易给子代理接上的压缩，**每次都 billing 运维的主推理模型**，而根 agent 早把同样的"读并浓缩"路由到了 flash 兄弟；扇出几个就付几份，纯成本、全程静默。**判据**：给一个可配置对象加档位时，`grep` 一遍它的 `::new(` 有几个调用点——**只喂了其中一个就是给另一个埋了静默降级**。守卫要断言**路由结果**（`summarizer_name()` 读到哪个 provider），不是断言 builder 被调过。**反向同样要判断**：不是每个 builder 都该补齐——`with_cache_carryover` 刻意不给子代理（16 槽 LRU 会被一次性子会话冲垮，反而害了它最该保护的常驻会话），`with_summary_reuse` 同样不给（子会话出生即空，接上去是恒 miss 的查表）。**"补全套装"要逐个问值不值，理由写进构造函数 doc**。详见 FEATURE_LOCATOR §2.19。
>
> **⚠️ `messages` 表是投影，不是真源 —— 改会话历史前先问改的是哪一张**: agent 每轮 prompt 由 **`session_events`** 重建（`think.rs::get_events` → `harness/agent/prompt.rs::build_prompt`）；`messages` 表由 `MessageProjector` 单向投影出来，只服务 Panel 显示、`sessions.history` / 搜索，以及**事件日志为空时**的首轮 legacy seed。**在 `messages` 上做的任何"上下文操作"对模型都是隐形的**——用户手动 `/compact` 就这么静默了很久：它调 `SessionStore::compact(KeepLastN{50})` 删投影表最老的行，报「saved ~N tokens」（N＝`deleted × 50`，编的），而下一轮 prompt 一个 token 都没少，代价是 Panel 滚动区被真删。**判据**：要改模型看到的东西 → 动 `session_events`（`SessionService::emit_event` 追加 / `retire_from` 退休尾部 / `retire_through` 退休头部）；要改用户看到的东西 → 那才是 `messages`。两者语义不同，**不要用一个去实现另一个**。现行手动 `/compact` 在 `src/context/compact/manual.rs`：摘要 + `CompactionPerformed` 检查点 + 前缀软退休，**什么都不删**（行与 FTS 索引都留着，所以 `recall_events` 仍能召回被压缩的细节——这正是"压缩不是净损失"成立的原因）。**另一处同族不对称**：`retire_from`（`chat.clear`/`rewind`）必须连 `session_events_fts` 一起删（否则刚清掉的内容会被 recall 递回模型），`retire_through`（压缩）必须**保留** FTS。详见 FEATURE_LOCATOR §2.1。
>
> **⚠️ `MessageRecord.timestamp` 单位有歧义**: SQLite backend 写秒、file backend 写 `timestamp_millis()`（同一文件里 `created_at`/`last_active_at` 却写 `timestamp()`），trait 文档说秒——**三种说法同时为真**，两种拼写同时在盘上。曾有**五处**读取点各自 `from_timestamp(ts, 0)`，于是导出里出现 58536 年、Panel 侧栏给 7 月的对话标「03-02」。**一律走 `MessageRecord::instant()` / `rfc3339()`**（`src/gateway/session_store/types.rs`，1e11 分界），裸格式化就是这个 bug 的下一次复发。源头未改是**有意的**：该值同时是 `get_history_before` 的分页游标，改单位要连全部存量会话一起迁移。

> **⚠️ 长期记忆的每一次失败都是静默的 (Memory Fails Silently — Three Rules)**: §2.5 记忆三支柱这一层没有崩溃、没有报错、测试全绿，坏掉时的表现只是**模型"想不起来"**——所以它的纪律必须写在这里，而不是等人从日志里发现。① **写进 frontmatter 的模型输出必须过 `yaml_scalar`**（`note/helpers.rs`）。`relations:` 块曾是唯一的例外，于是 `to: [[plan/x]]`（模型在 note API 别处到处见到的形式）被 YAML 解析成嵌套序列，`from_markdown` 在**整个 frontmatter** 上失败 ⇒ 这篇笔记从语料里**永久消失**：`mention_weave` 用 `.ok()?` 丢掉它、`note_decay` `continue` 跳过它、日志最高只有 `debug!`，而 `load_existing_or_default` 还会把它当空笔记递给下一次 ingest 去覆盖。同侧规则：往 `IngestPlan` 加任何**能装路径的字段**都要进 `RefTable::resolve_plan` 的 field policy（`create.relations` 就是漏掉的那个，prompt 却明确允许它写 `[P<n>]` token）。② **召回信号必须按笔记的真实归属命名空间记账**，绝不用 `agent_ids.first()` 当"代表标签"——`read_scope_ids` 返回 `[base, scoped]`，而 `NoteDecay.access_weight` 与进化证据闸读的是 **scoped** id，贴错标签＝项目笔记看起来"从未被召回"被提前归档，同时 base 收获它并不拥有的幻影热度。**对 prompt 没有贡献的命中也不该赚到信号**（FTS 腿返回空正文却照记信号，等于持久化"空笔记很热"）。③ **排序即预算优先级**：`hydrate` 严格按序扣槽位预算并丢弃截断成空的项，所以"常开地板/钉死项"必须**排在前面**才真的钉得住——只在注释里写 "can never be dropped" 不构成保证。三条的共同形状：**能力接上了 ≠ 它在跑**，判据永远是"这条路上有没有一个真实消费者读到了真实的值"。详见 [FEATURE_LOCATOR.md](docs/reference/FEATURE_LOCATOR.md) §2.5 与 [NOTES.md](docs/reference/memory/NOTES.md) / [RETRIEVAL.md](docs/reference/memory/RETRIEVAL.md)。

> **⚠️ `cargo check` 不编译 `#[cfg(test)]` —— 删生产 API 时测试会在盲区里烂掉**: 2026-08-01 发现 `cargo test -p alephcore --lib` 在 main 上**已经编译不过很久了**（47 个错误），根因是几次「删死代码」的提交删掉生产 API 却留下它们的测试；因为日常回路只跑 `cargo check`，而它**不碰 test 代码**，所以整个单元测试套件坏了却没有任何信号。修好之后立刻暴露出 2 个一直在失败、只是从来没机会运行的测试。**推论**：任何删除 `pub fn` / 结构体字段的改动，同一笔里必须跑一次 `cargo test -p alephcore --lib --no-run`；只跑 `cargo check` 等于没验证。测试专用的观察器（`len`/`is_empty`/`get_hint` 这类）正确的归宿是 `#[cfg(test)]` 而不是删除——它们有真实消费者（测试），只是不该进生产表面。**⚠️ 盲区不止 `--lib`（2026-08-03 又撞三次）**：同一形状在**三个**日常回路照不到的 target 上各复发一次——① `cargo test --lib` 本身（`e8b7c3cf9` 收窄 `loop_graph::service::render_session_topology_in` 的可见性，而 `store.rs` 的测试仍在调它，整套 47→1 个错误又编不过）；② **另一个 crate**（`aleph-panel` 在 main 上编不过：`chat/composer/mod.rs` 调两个 `ChatState` 上从未存在的方法，而 `-p alephcore` 永远看不见它）；③ **examples**（`examples/file_logging_demo.rs` 导入已删除的 `alephcore::init_logging`，只有 `--all-targets` 才暴露）；④ **feature 门后的集成测试**（2026-08-04：`tests/cron_probe/delivery_alert.rs` 是同型第四例，藏在 `test-helpers` feature 门后——`cargo check`、`cargo clippy --all-targets`、以及**不带 feature** 的 `cargo test --test '*'` 全都编不到它，因为 `--all-targets` 只展开 target 不展开 feature）。**最小可信验证集是五条命令，不是一条**：`cargo test -p alephcore --lib --no-run` + `cargo test -p alephcore --features test-helpers --test '*' --no-run` + `cargo check -p aleph-panel` + `cargo check -p aleph-desktop-{macos,windows,linux}` + `cargo clippy --all-targets`。**只信 `cargo check -p alephcore` 的绿，等于只验证了仓库的一小半。**

> **⚠️ 同一个能力有两套栈时，先问「工具接的是哪一套」**: "音频转写"在仓里有两条独立的栈——`MediaProcessor`（`src/media/processor.rs`，trait `TranscriptionService`）服务**附件**转写，启动时就按 `[generation] transcription_providers` + vault 解析出真实后端；`MediaPipeline`（trait `MediaProvider`）服务 `media_understand` / `audio_transcribe` / `document_extract` 三个**工具**。两者之间此前**没有桥**：构造器只塞了 image + doc 两个 provider，而 schema 门只看 `media_pipeline.is_some()`（它总是 Some）⇒ **工具对模型可见、用户明明配好了 Whisper、调用永远回 "no media pipeline configured"**。旧注释的理由（注册一个只返 `NoProvider` 的 stub 会让 pipeline 谎称支持音频）**对，但只覆盖了一半**——「配了后端却不注册」是它的镜像故障，而那个才是线上活着的。现单一源 `src/media/resolve.rs::transcription_service` 被 `agent_init` 与 registry 构造器**共用**（此前 agent_init 内联 80 行、构造器什么都不做，两条路可以对"到底配没配转写"给出不同答案），`AudioMediaProvider` **只在真解析出后端时注册**。同批修一处契约违背：工具曾把语言提示编成英文句子 `"Transcribe this audio. Language: zh"` 塞进 `prompt`，而 `TranscriptionService::transcribe` 的 doc 明写 `language` 是**原生参数**（后端当 API 字段发）——**编进散文里的参数等于没传**。详见 FEATURE_LOCATOR §7.6。

> **⚠️ 同一个盲区还有第二层：`-p alephcore` 不编译 Panel，语义合并冲突就住在那里**: 2026-08-03 发现 **main 上的 `aleph-panel` (WASM) 编译不过**——`composer/mod.rs` 调 `chat.retract_latest_queued()` / `chat.add_pending_attachments()`，而 `state.rs` 里这两个方法**不存在**。根因是合并 `c84ce19b6` 把两条**各自独立实现过同一个功能**（↑ 撤回排队消息）的分支合到一起：`state.rs` 取了一侧的 API（Round-7 的 `seed_draft` / `recall_latest_queued`，带测试），`composer/mod.rs` 留下了另一侧的调用点。git 不报冲突（两个文件各自都是干净的 fast-forward），`cargo check -p alephcore` 不碰这个 crate，于是没有任何信号。**推论**：① 改动 `interfaces/webchat/` 的同一笔里必须跑 `cargo check --target wasm32-unknown-unknown`（`just wasm` 亦可）；② **合并两条实现过同一功能的分支时，先 grep 功能名再合**——语义合并冲突的典型形状是「一侧的类型 + 另一侧的调用点」，两边单独看都完整；③ 修好后先看**警告**再看错误：这次修完立刻冒出 `unused variable: retract_latest_queued`，说明那半边根本没有调用者（活的 ↑ 路径一直走 `recall_latest_queued` + `seed_draft`），正解是 CUT 而不是继续修（R10）。**⚠️ 同日复发第二例（同一根因、不同符号）**：`composer/mod.rs` 又读 `chat.draft_seed` / 调自由函数 `merge_draft`，两者在 `ChatState` 上同样**不存在** —— 活的设计是 `ChatState::seed_draft`，它直接写 `chat.draft`（就是 composer 绑成 `input_text` 的那个信号），其 doc 自己写着「there is no prefill channel that could be written with nothing draining it」。同样 CUT。**结论：这个 crate 的合并冲突不是一次性事故，是常态形状** —— `interfaces/webchat/` 有任何改动（哪怕不是你改的）就跑一次 `cargo check -p aleph-panel`；它便宜（十几秒）而 `-p alephcore` 的绿**证明不了这个 crate 能编译**。

> **⚠️ 一个动词的两个面必须共用同一个谓词，且「读不到」不等于「不适用」**: `workflow_step_review`（工具）与 `teams.workflow.{approve,reject}_step`（RPC）是同一个动作的两张脸，而**两张都不校验当前状态**——尽管 RPC 的 doc 注释白纸黑字写着「Refuses to approve tasks that have not yet finished a run」，尽管兄弟工具 `team_task_control` 的**五个臂全都校验**。后果不是脏数据而是一条**全绿的假成功链**：approve 一个 `Pending`（从未跑过）的步骤 → `Completed` 且 `result` 为空 → 下游解阻塞 → 扇入渲染空 → settle 扫描报 `✅ Workflow finished`。现两面共用 `verdict_admissible`。**同一笔里的第二个坑**：工具面的快照读用了 `.ok().flatten()`，于是 `Err`（coord store 是一把 `Mutex<Connection>`，`SQLITE_BUSY` 是日常事件）被折成 `None` → **闸整个跳过**，正好是它的 RPC 孪生显式拒绝的那一路。**判据**：任何按当前状态做的闸，`Err` 必须是拒绝，不能是放行；任何新增的第二张脸，先问「兄弟面校验了什么」。

### Windows 构建

`just shell-build` / `just shell-dev` 在 Windows 同样适用（justfile 已守卫 macOS 专属步骤、自动追加 `.exe`），产物为 NSIS `.exe` + `.msi`。一次性前置依赖（MSVC / WebView2 / protoc / wasm 目标 / `wasm-bindgen-cli` 版本对齐 / `cargo-tauri` / Git for Windows `usr\bin` 入 PATH）与全量构建步骤详见 [WINDOWS_RUNTIME.md](docs/reference/WINDOWS_RUNTIME.md)。

### 版本管理

- **CalVer (日历版本)** — 格式 `YY.M.D`（两位年，月/日不补零，如 `26.5.21`），每天最多发布一个版本。该格式同时是合法 semver 并满足 Windows MSI 版本约束，可直接作为桌面 App 的 bundle 版本
- **VERSION 文件是唯一版本源** — `build.rs` 读取 VERSION → 注入 `ALEPH_VERSION` 环境变量 → 所有代码通过 `env!("ALEPH_VERSION")` 使用
- **禁止** 在代码中硬编码版本号，使用 `env!("ALEPH_VERSION")` 代替 `env!("CARGO_PKG_VERSION")`
- Panel System Info、Gateway 版本、MCP/ACP 协议版本、CLI --version 全部从 VERSION 文件读取
- GitHub workflow 也读取 VERSION 文件作为 release tag

### 发版流程 (Release Process)

`just release YY.M.D` 触发三产物×三平台构建发布（发版前先写 CHANGELOG.md）。完整两步流程、`just verify-build` 预检、CI fail-fast 轮询（`scripts/poll_release_run.py`）详见 [RELEASE.md](docs/reference/RELEASE.md)。

### Feature Flags

所有生产功能始终编译，无需 feature flags。仅保留测试用 features：`loom` (并发测试)、`test-helpers` (集成测试工具)。

### 提交规范

English commit messages. Format: `<scope>: <description>` — Example: `gateway: add WebSocket server foundation`

### 分支策略

**单分支开发模式**：所有开发工作直接在 main 分支进行。

### My Working Style

- 先给方案再写代码；不确定时列出选项，不猜测（呼应 P1 与全局 CLAUDE.md）
- 重大变更前先问，小优化可直接执行
- 回复用中文，代码注释用英文，文档中英双语
- 按需正常使用 cargo（`check` / `test` / `clippy`）—— 编译与测试验证优先，不再强制节制调用次数

### Git Worktree 注意事项

`EnterWorktree` 会话内只合并不删除（同会话 `git worktree remove` 会损坏 Shell）。详见 [CODE_ORGANIZATION.md](docs/reference/CODE_ORGANIZATION.md)。

### 进程管理 (Process Management)

Singleton 由 OS 级 `flock`（`~/.aleph/data/aleph.lock`）强制；CLI 写子命令经 `with_policy` 走 IPC 或本地拿锁，不与服务竞争。`kill -9` 后可立即重启。Spec C 不变量与回归脚本详见 [PROCESS_MANAGEMENT.md](docs/reference/PROCESS_MANAGEMENT.md)。doctor 的 `core/duplicate-instance` 检查（sysinfo 进程表）是本红线的运行时哨兵：检测到其它存活 `aleph-server` 进程即 Warning 并引用本规程——多进程竞争同一 vault → HMAC 失败 → **vault 数据丢失**。

> 信任模型见上文「信任模型 = 网络边界」与 [SECURITY.md#auth-ux](docs/reference/SECURITY.md#auth-ux)。

---

## 📚 文档索引

> **Context Tiers**: Tier 1（每次加载）= 本 CLAUDE.md，项目是什么 + 怎么工作；Tier 2（按需加载）= 下表 `docs/reference/*`，Claude 工作时按主题自取；Tier 3（默认忽略）= `docs/archive/`、历史规格，除非明确要求不碰。

| 文档 | 链接 |
|------|------|
| ARCHITECTURE.md | [docs/reference/ARCHITECTURE.md](docs/reference/ARCHITECTURE.md) |
| **PRODUCT_TOPOLOGY.md** | [docs/reference/PRODUCT_TOPOLOGY.md](docs/reference/PRODUCT_TOPOLOGY.md) — 产品形态：一套源码(panel/core/shell)→三产物(完整App/纯壳Panel/独立core)排列组合 + 参考部署拓扑(家庭服务器+瘦客户端) |
| **HARNESS_PHILOSOPHY.md** | [docs/reference/HARNESS_PHILOSOPHY.md](docs/reference/HARNESS_PHILOSOPHY.md) — 薄 Harness 哲学 + 笨循环编排核心（R10 详解） |
| **TWELVE_FACTOR_AUDIT.md** | [docs/reference/TWELVE_FACTOR_AUDIT.md](docs/reference/TWELVE_FACTOR_AUDIT.md) — 12-Factor Agents 逐 factor 合规审计 + 采纳条款 A1–A4 母本 + 模块改造 backlog（对照宪法《🧭 12-Factor 对照与采纳》节） |
| AGENT_SYSTEM.md | [docs/reference/AGENT_SYSTEM.md](docs/reference/AGENT_SYSTEM.md) |
| AGENT_LOOP_CONTEXT_BUDGET.md | [docs/reference/AGENT_LOOP_CONTEXT_BUDGET.md](docs/reference/AGENT_LOOP_CONTEXT_BUDGET.md) |
| AGENT_LOOP_TOOL_EXECUTION.md | [docs/reference/AGENT_LOOP_TOOL_EXECUTION.md](docs/reference/AGENT_LOOP_TOOL_EXECUTION.md) |
| AGENT_LOOP_RECOVERY.md | [docs/reference/AGENT_LOOP_RECOVERY.md](docs/reference/AGENT_LOOP_RECOVERY.md) |
| **GRAPH_LAYER.md** | [docs/reference/GRAPH_LAYER.md](docs/reference/GRAPH_LAYER.md) — 循环治理图（loop-graph governance）。**⚠️ Round-11（2026-08-03）四条同族纪律，逐条都跨子系统成立**：① **闸的范围必须覆盖「能把这个闸拿掉的那个动词」**——`loop_graph` 的参数级审批卡守的是 `root:`/`frozen:` 写，而 §6.2 objective ACL 靠的是一条 `owns_reference` 边，**它的两个端点都不带受保护前缀**，于是一次 `unlink` 就摘掉整个 ACL——**而拒绝那次写入的错误消息正把这行参数逐字印给模型**。第二扇门更隐蔽：`drop_node` 治理者 + `gc`，**两次调用都不带受保护 id**，合起来到达同一状态。判据：给一个机制加闸时，除了问「谁能改被保护的东西」，还要问**「谁能让这个闸不再适用」**，以及**「有没有两步都合法、合起来等价的路径」**。② **『不可伪造』是关于 kind 的，而边有方向——受保护的 kind 也会出现在来源位置**。round 9/10 两次把覆盖环的**形状**堵死（长度 1、长度 2），词汇却一直开着：`watches`/`audits` 的**来源**可以是任何 kind，而 `anchor:` 只要一次**不举卡**的 `node` 调用 ⇒ 两次免费调用永久静默整层唯一的自证豁免检测器。正解是问**「这个 kind 跑得起来吗」**（Anchor 是测量声明、Frozen 是别处执法的规则，都没有会话没有节奏）。**同一个洞被堵第三次了**，每次都是「上次只修了它的一种表达」。③ **外层格式转义 ≠ 内层格式安全**——`escape_xml` 封住 `<tag>` 那一层，而 `<loop_graph_context>` 内部是**行式格式且行本身携带权威**（`根参照 …（人供给——你可以引用、必须遵循、无权修改）`），`\n` 原样穿过 ⇒ 模型自撰的 node label 能另起一行伪造根参照，逐轮注入且持久。**往任何结构化 prompt 块里插模型可写的值之前，先问这个块的内层格式是什么，两层都要封。**④ **举给人的那张卡必须包含闸所依据的那个字段**——审批卡按 `BTreeMap` 字典序渲染并在 200 字符处截断，于是被闸的 `to_id` 排在**无上限的模型自撰散文之后**被整段截掉；闸正确开火、卡上却没有任何证据说明它闸的是什么，而会话授权随后绑定该调用。同族：`tier_asks_for_arguments` 的「操作者显式点名了这个工具」用了**会匹配 glob** 的查找，一句 `"*" = "allow"` 就关掉 destructive 参数前最后一张卡——**「显式」在代码里必须是精确匹配**。另两条只在本层：**慢环之间的承诺要两个方向都问**（`watcher_is_pokeable` 问「看守叫得醒吗」，`target_has_victory_claim` 问「被看守者会不会宣称胜利」，缺一就在 prompt 里承诺一件不会发生的事）；**一个安装器的幂等要对齐现实而不只对齐自己的账本**（`enable_audit` 的重装建议从不删 cron job，照做就得到两个互相 supersede 裁决的审计环）。**⚠️ Round-10（2026-08-03）另加三条同族纪律**：① **一次性的动作，哪个面执行了哪个面就是唯一机会**——`team_disband` 工具触发胜利宣称而 `teams.disband` RPC（Panel 走这条）不触发，disband 之后团队已是 `disbanded`，工具面事后补不回来。② **豁免必须来自环自己给不了的东西**——`lint_naked_loops` 问「有没有边指向我」，round 9 把长度 1 堵死，长度 2（互看）仍免费；新的 `lint_forged_coverage` 要求向上的 watches/audits 链触到 `Root`（round 11 起只认 `Root`，理由见下）。③ **fail-soft 的丢行和 fail-soft 的 Err 是两扇门**——round 9 关了 ACL 的 `Err` 门，`list_edges` 静默丢弃解析不了的行这扇没关，而 ACL 把「没有这条边」读作放行；存在性问题一律直读解析不会失败的原始列。：`src/loop_graph/` 六词闭集治理边 + 锚点/冻结/根参照 + 审计环，四种单循环失败（Goodhart/参照盲区/环冲突/测量衰减）的拓扑解法；spec 见 docs/superpowers/specs/2026-07-19。**⚠️ 自证豁免必须不可伪造**：`lint_naked_loops` 问的是「有没有 watches/audits 边**指向**我」，所以 `x -[watches]-> x` 曾满足它——一次 `link` 就让被监视者对三个面（lint / doctor / 周审计模板）报出**积极为假**的「拓扑健全」。守卫是双侧的（`upsert_edge` 拒写 + lint 谓词 `from_id != n.id` 兜住存量行）。同理**别**把「有出向 `audits` 边」当豁免：那把豁免从**不可伪造的 kind**（Anchor/Frozen/Root）降级成**优化器自己能授予的边**，是同一个洞的放大版——审计环自报裸奔的正解是人接一条 `root:… -[watches]-> cron:<audit>`，零代码。 **⚠️ 三条同族纪律（2026-08-02 Round-9，逐条都是「看起来在把关，其实没有」）**：① **一次性章不能在动作确认之前花掉**——胜利宣称的 `try_claim_settle_notify` 盖完章就调 poke，而 poke 有三条「没跑成」的出路；Complete goal 的 `completed_at_ms` 此后不再变，所以那次完成**永远**不会再被评审。现 `notify_goal_settled` 返回「有没有真 poke」，false 时 `release_settle_notify` 还回去。推论：凡「盖章 → 做事」的一次性机件，章要么在事后盖，要么必须可归还。② **fail-soft 的跳过不是「不存在」的证据**——`row_to_node` 对未知 enum 文本返回 `Ok(None)` 是给读者防卡死用的，`gc` 拿它当 DELETE 判据，于是一行**存在却解析不了**的节点，它所有的边被不可逆删除（两个 enum 都 `#[non_exhaustive]`，降级运行就是触发条件）。存在性一律直读 `id` 列（`node_ids_present`）。③ **参数级审批闸只在能举卡的 surface 上成立**——`is_denied_on_gateway_surface` 曾只看工具名，于是 `tools.invoke` / heartbeat probe 可以带 `{action:"node", kind:"root", …}` 改写那份逐轮注入每个被治理会话 prompt 的根参照原文。现它**收参数**并直接读 `ExecTier::Auto::asks_for_arguments`——与循环强制的是同一个谓词，未来任何新的参数级规则自动覆盖三处。**另**：codex Multi-agent V2 的逐项对照已做完（GRAPH_LAYER.md §7 末表，结论＝**无一项可移植**，含四条「违 R7/R10 不移植」）；**LangGraph 的逐项对照也已做完**（同节，含 conditional-edge 那条为何是 DECIDE 而非 ENHANCE），改这一层或 `src/workflow/` 前先看那两张表，不必重做对比。|
| MULTI_AGENT_SYSTEM.md | [docs/reference/MULTI_AGENT_SYSTEM.md](docs/reference/MULTI_AGENT_SYSTEM.md) |
| GATEWAY.md | [docs/reference/GATEWAY.md](docs/reference/GATEWAY.md) |
| CLUSTER.md | [docs/reference/CLUSTER.md](docs/reference/CLUSTER.md) — Aleph 集群（单中心非对称节点联邦）：reverse RPC + `node_invoke`/`node_file` + `node_manage`(对话式管舰队, R8) + 命令 allowlist + 审批回中心 + 断线 fail-fast + 版本握手(仅观测)；**内含 openclaw 对照映射表(Gap Analysis)，改集群前先看那张表，不必重做对比** |
| TOOL_SYSTEM.md | [docs/reference/TOOL_SYSTEM.md](docs/reference/TOOL_SYSTEM.md) |
| **MODEL_CATALOG.md** | [docs/reference/MODEL_CATALOG.md](docs/reference/MODEL_CATALOG.md) — 预设 provider / 模型参考数据：四张表(presets·capabilities·pricing·lifecycle) + 单一 join 点 `ModelRecord::resolve` + 按需 `/models` 发现 + **漂移守卫契约**；内含 **opencode / kimi-cli 对照表(Gap Analysis)，改这一层前先看那张表，不必重做对比** |
| **MODE_SYSTEM.md** | [docs/reference/MODE_SYSTEM.md](docs/reference/MODE_SYSTEM.md) — 会话模式 chat/work/code：exec_tier/think_level 的第三孪生，工具呈现面静态分区（R10 渐进披露例外）+ 模式 prompt line + Panel 模式选择器/右栏差异化 |
| MEMORY_SYSTEM.md | [docs/reference/MEMORY_SYSTEM.md](docs/reference/MEMORY_SYSTEM.md) |
| └─ RAW_MEMORY.md | [docs/reference/memory/RAW_MEMORY.md](docs/reference/memory/RAW_MEMORY.md) |
| └─ NOTES.md | [docs/reference/memory/NOTES.md](docs/reference/memory/NOTES.md) |
| └─ RETRIEVAL.md | [docs/reference/memory/RETRIEVAL.md](docs/reference/memory/RETRIEVAL.md) |
| └─ DREAM_DAEMON.md | [docs/reference/memory/DREAM_DAEMON.md](docs/reference/memory/DREAM_DAEMON.md) — 离线做梦维护 + **自进化纪律（SkillOpt 移植）**：strict-`>` 进化门 / EditBudget textual-learning-rate / recall-evidence 门 / rejected-edit buffer 回喂 prompt / best_health 持久化（§3.1；FEATURE_LOCATOR §2.17）。**`DreamGate` 已删（零消费者，勿复活）** |
| EXTENSION_SYSTEM.md | [docs/reference/EXTENSION_SYSTEM.md](docs/reference/EXTENSION_SYSTEM.md) — 插件**运行时**（WASM / Node / SDK / 通道·Provider·HTTP 插件） |
| **ALEPH_HUB.md** | [docs/reference/ALEPH_HUB.md](docs/reference/ALEPH_HUB.md) — 扩展**分发**：目录线上契约 + 三道 ingest 闸 + 安装管线强制点表 + 安装出处账本 + 工具面五处登记；**内含 openclaw `clawhub` 逐项对照表(Gap Analysis)，改这一层前先看那张表，不必重做对比** |
| PLUGIN_SYSTEM.md | [docs/reference/PLUGIN_SYSTEM.md](docs/reference/PLUGIN_SYSTEM.md) |
| WORKFLOW_INTEROP.md | [docs/reference/WORKFLOW_INTEROP.md](docs/reference/WORKFLOW_INTEROP.md) |
| SECURITY.md | [docs/reference/SECURITY.md](docs/reference/SECURITY.md) — 信任模型 + 工具权限三层（`tool_permissions` × exec tier × sandbox 硬底线，唯一强制点 `src/tools/scoped/`）+ 动作化审批门 + **codex / hermes / pi 对照表（Gap analysis，改权限模型前先看，别重做对比）** |
| **AGENT_IDENTITY.md** | [docs/reference/AGENT_IDENTITY.md](docs/reference/AGENT_IDENTITY.md) — 每 agent 独立 Ed25519 密钥 + 签名哈希链操作账本（`src/identity/`，生产者＝`tools/scoped/` 唯一咽喉，归属单一源＝`ledger_agent_id()`，**子代理由 `AllowlistToolService` 开 `as_actor` 签自己的活**，密钥生命周期本身进链且**换钥必须由链自己交代**，读/验＝`agent_identity` 工具 + 离线 `aleph-server identity` + **`export`/`verify --input --pin --expect-head` 交给审计方在没有 DB/vault/daemon 的机器上验**）；**威胁模型写明买到什么买不到什么**（不防拥有 `~/.aleph` 的对手、不防进程内冒充、对从未写入的记录无话可说——故 `lost` 计数落库并与 `ok` 并排返回；**没钉根指纹的导出什么也不证明**；尾部截断**只有钉链头才检得出**——锚随文档走）+ **buzz 逐维度对照表（改这层前先看那张表）**。**⚠️ 第四轮（2026-08-04）三条跨子系统纪律**：① **「先做、再宣告」的顺序决定失败后剩下什么**——轮转曾是「改 keystore → 即发即忘地上链声明 → 回 `ok`」，那条声明一丢，新钥就是**活跃且从未被链声明**的，此后它签的每一行永久 `UndeclaredSigner`，而操作员被告知成功了；正解是把两半收进**同一个单写者**并把顺序排成「铸 → 宣告 → 启用」（任何失败都停在启用之前）。凡「改状态 + 宣告这次改动」的机件，都要问**哪一半先落**，以及**只落一半时活下来的是不是难以抹掉的那一半**。② **「排队未写」是第三种结局**，与 §5.6 投递队列那条同型：什么都没失败，所以失败计数器也不会 +1；有界队列 + 单写者的正解是一个 **FIFO 屏障**（`identity::flush()`），且它必须排在**所有还能产生新记录的子系统停掉之后**，否则它保证的是一个还在增长的队列。③ **一个动词的两张脸要共用判据，也要共用推导**——CLI 的 `export` 曾自己第二次推 root fingerprint 且不自检刚写出的文档，而工具那张脸两件都做；两面现在都走 `verify_export` 并从报告里读值。 |
| DESIGN_PATTERNS.md | [docs/reference/DESIGN_PATTERNS.md](docs/reference/DESIGN_PATTERNS.md) |
| CODE_ORGANIZATION.md | [docs/reference/CODE_ORGANIZATION.md](docs/reference/CODE_ORGANIZATION.md) |
| DOMAIN_MODELING.md | [docs/reference/DOMAIN_MODELING.md](docs/reference/DOMAIN_MODELING.md) |
| AGENT_DESIGN_PHILOSOPHY.md | [docs/reference/AGENT_DESIGN_PHILOSOPHY.md](docs/reference/AGENT_DESIGN_PHILOSOPHY.md) |
| SERVER_DEVELOPMENT.md | [docs/reference/SERVER_DEVELOPMENT.md](docs/reference/SERVER_DEVELOPMENT.md) |
| SANDBOX.md | [docs/reference/SANDBOX.md](docs/reference/SANDBOX.md) |
| SESSION_SERVICE.md | [docs/reference/SESSION_SERVICE.md](docs/reference/SESSION_SERVICE.md) |
| MODEL_PERCEIVABLE_ECOSYSTEM.md | [docs/reference/MODEL_PERCEIVABLE_ECOSYSTEM.md](docs/reference/MODEL_PERCEIVABLE_ECOSYSTEM.md) |
| SKILL_TRIGGER_ENHANCEMENT.md | [docs/reference/SKILL_TRIGGER_ENHANCEMENT.md](docs/reference/SKILL_TRIGGER_ENHANCEMENT.md) |
| WHATSAPP_ARCHITECTURE_DESIGN.md | [docs/reference/WHATSAPP_ARCHITECTURE_DESIGN.md](docs/reference/WHATSAPP_ARCHITECTURE_DESIGN.md) |
| DESKTOP_BRIDGE.md | [docs/reference/DESKTOP_BRIDGE.md](docs/reference/DESKTOP_BRIDGE.md) |
| DESKTOP_SHELL.md | [docs/reference/DESKTOP_SHELL.md](docs/reference/DESKTOP_SHELL.md) |
| WINDOWS_RUNTIME.md | [docs/reference/WINDOWS_RUNTIME.md](docs/reference/WINDOWS_RUNTIME.md) — Windows 运行时部署/运维：install.ps1 安装、前台运行、`--daemon` 限制与替代、单例锁、stop/status 注意事项、LAN 信任、刷新二进制链 |
| **LINUX_DESKTOP.md** | [docs/reference/LINUX_DESKTOP.md](docs/reference/LINUX_DESKTOP.md) — Linux 桌面能力矩阵（X11 vs Wayland 逐能力）+ 装什么（apt 清单 / AT-SPI 开启）+ 诊断顺序（窗口/剪贴板/AX/输入/退出应用）+ 验证状态诚实标注（Wayland 后端仅单测覆盖） |
| GOOGLE_MEET_BRIDGE.md | [docs/reference/GOOGLE_MEET_BRIDGE.md](docs/reference/GOOGLE_MEET_BRIDGE.md) — `google_meet` 薄工具契约 + 外部 transport bridge JSON-RPC 协议 |
| RELEASE.md | [docs/reference/RELEASE.md](docs/reference/RELEASE.md) — 发版两步流程 + `just verify-build` 预检 + CI fail-fast 轮询 |
| PROCESS_MANAGEMENT.md | [docs/reference/PROCESS_MANAGEMENT.md](docs/reference/PROCESS_MANAGEMENT.md) — Singleton flock / Spec C 不变量 / CLI 写策略 |

> **官方 skills/plugins 离线兜底**: 根目录 `skills/` 与 `plugins/` 是两个 git submodule（upstream = 兄弟仓 Aleph-skills / Aleph-plugins），经 `include_dir!` 在 `aleph-server` **编译期嵌入二进制**（`src/bundled/mod.rs`）。首次安装优先从上游兄弟仓 git clone 官方内容（`src/bundled/extractor.rs::extract_bundled_content`）；**网络故障 / clone 失败时回退到这份嵌入快照**，保证完整桌面 App（单机零配置）离线也能装上官方 skills/plugins。**勿删这两个目录**——`include_dir!` 是编译期宏，目录缺失直接编译失败，并连带破坏 `build.rs` rerun / CI `submodules: recursive` / `justfile` 发版重嵌链。（另一层：Hub 浏览目录冷启动 primer 同样投影这份快照，远端 catalog fetch 后整槽覆盖。）

---

## 🏢 官方仓库 (Official Repositories)

| 仓库 | 路径 | 说明 |
|------|------|------|
| Aleph (主项目) | `/Volumes/TBU4/Workspace/Aleph` | Rust Core + 多端架构 |
| Aleph-docs | `/Volumes/TBU4/Workspace/Aleph-docs` | 官方文档 |
| Aleph-homepage | `/Volumes/TBU4/Workspace/Aleph-homepage` | 官方首页 (Next.js) |
| Aleph-Hub | `/Volumes/TBU4/Workspace/Aleph-Hub` | 官方扩展目录中心（策展远程 MCP/Skill/Plugin） |
| Aleph-mcp | `/Volumes/TBU4/Workspace/Aleph-mcp` | 官方 MCP 项目 |
| Aleph-plugins | `/Volumes/TBU4/Workspace/Aleph-plugins` | 官方插件市场 |
| Aleph-skills | `/Volumes/TBU4/Workspace/Aleph-skills` | 官方技能 |

> **生态统一管理约定**: 以上 7 仓为同级兄弟目录（共处 `/Volumes/TBU4/Workspace/`），同属 Aleph 官方生态，远端均在 `github.com/rootazero/`。**始终从主项目 `Aleph/` 启动会话**，周边仓作为兄弟目录就地操作——这样跨会话长期记忆统一沉淀到主项目的全局 memory 库（按工作目录路径编码），spec/plan 统一落在主项目的 `docs/superpowers/{specs,plans}`（superpowers 工作流归档；整个 `docs/` 树现已纳入 git 版本管理——`.gitignore` 的 `/docs/*` 忽略段已移除，新建 docs 默认被跟踪、随常规提交入库，不再需要 `git add -f`），周边仓的 spec 以子项目名作文件名前缀（如 `2026-06-23-aleph-mcp-xxx.md`）。避免直接进周边仓启动会话导致记忆库分裂。

---

## 🧠 长期记忆与质量门 (Memory & Hooks)

- **长期记忆**: 走全局 `~/.claude/projects/.../memory/`（跨会话、Git 不追踪）。**不在项目内另造 MEMORY.md**——避免与全局记忆双源冲突。
- **质量门 (Hooks)**: 当前**未挂** `.claude/hooks/`。CLAUDE.md 里的规则目前靠模型遵守；未来如需"强制执行层"（如 PostToolUse → `cargo fmt`），在 `.claude/hooks/` 配置即可。
