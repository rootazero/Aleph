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
- **精简原则 (prune-the-prompt)**: 智慧迁进 prompt ≠ 把 prompt 写厚。模型越强越需要**更少方向/约束/示例**——few-shot 会变笼子（模型模仿范例而非理解问题空间），低密度冗余稀释注意力；新模型发布后第一件事是**修剪上下文**，把验证/自我纠错建进**架构（运行时信号）**而非 prompt。加 prompt 字节前过**两把尺**：① 这是模型**做不到的运行时事实**，还是我在**教强模型怎么思考**？后者别进 prompt。② **这句话有没有一个工具拥有它**？有 → 写进那个工具的 `DESCRIPTION`（随 schema 发，且只发给真能调它的请求），别写进 system prompt——system prompt 只承载**没有任何单个工具能说出口的东西**（跨工具取舍 / 运行时事实 / 安全边界）。第二把尺是 pi 之镜（2026-07-26）：第一把量不出"完全正确、完全必要、只是放错地方"的重复。**两把尺都已建进架构**：`src/thinker/prompt_contract.rs` 的 `reachable_layers`（层必须能开口，否则带理由进白名单）/ `scaffold_bytes_ratchet`（实测骨架天花板，只减不增）/ `no_sentence_is_stated_twice`；量一下用 `aleph-server prompt-size`。详见 [HARNESS_PHILOSOPHY.md §8](docs/reference/HARNESS_PHILOSOPHY.md) 与 FEATURE_LOCATOR §1.1

### R10. 薄 Harness 哲学，笨循环编排核心 (Thin Harness, Dumb Loop)

> *"If you're not the model, you're the harness."* — Vivek Trivedy
> *"Models get stronger → harness gets thinner."* — Anthropic

- **薄 Harness 哲学 (Thin Harness)**: Aleph 采纳 Anthropic 流派，运行时极简、信任模型。Harness 是脚手架不是认知层。**模型越强，Harness 越薄** — 优秀的 Harness 必须通过"面向未来测试 (Future-Proof Test)"：换更强的模型，性能自然提升，无需修改 Harness 代码
- **笨循环 (Dumb Loop)**: `src/harness/` 仅承载 Think→Act 轮次调度，**不参与任何推理**。所有智能决策（意图理解、工具选择、安全评估、完成度判断）由 LLM 一次推理调用自然完成
- **核心边界**: `src/harness/` 锁 **12 文件**；行数由 `src/harness/tests/budget.rs` 的棘轮守（实测非手算、只减不增、增必答下方 3 问），当前 **5055 行**（**2026-07-26 欠账已清**：`5008 → 5082（+74）` 的 3 问作答补在 `tests/budget.rs::CEILING`；+79 实为前一笔 `c648b5ea4`，前三项全过、第四项"每批 `canonical`/`claims` 穿线"的 `Option::None` 重算臂**零消费者**（唯一传 `None` 的快路径正好撞上 `can_parallel_dispatch` 先行 `return false` 的同一条件），按"零消费者立即撤回"已撤 → **5082 → 5055（−27）**，行为按构造不变。**2026-07-25 文档订正**：本文与 `src/harness/CLAUDE.md` 此前写 5008，而代码里的 `CEILING` 早在 `396c6d200` 就抬到 5082——正是 budget.rs 开头那段"手写状态行会撒谎"要防的事，在文档层复发了一次。**代码是权威**，任何文档数字都只是 `CEILING` 的副本；2026-07-20 −62＝移除 `DiminishingReturnsDetector` 硬停 [R10 5-不 #3：loop 不做完成度判断]，`think.rs` 弃 `after_turn` 消费点、detector/`StopDiminishing`/`TurnMetrics` 删自 `src/context/budget/`，见 budget.rs::CEILING；2026-07-18 −2＝流式旁路修复：`stream_llm_call` 弃 `as_http_provider()` 降级分支、改多态 `execute_streaming_dyn`，副作用下沉 `src/providers/` 装饰器，见 budget.rs::CEILING；Batch 6，2026-07-17：两侧同日从 5035 出发、合并实测 5072——上调 +80＝ambient 审批关联 + 完成序 live 事件；删除 −42＝test-only `run_turn` 簇外迁 `tests/harness_ext.rs` + 恒零 `consecutive_errors` trace 字段删除；3 问作答在 budget.rs）。旧的 ~4900 系一次手算口径事故（生产 `impl` 中间的缩进 `#[cfg(test)]` 截断 `agent.rs`、静默漏计 846 行）的残值，**已退休**——红线是棘轮机制本身，不是那个具体数字：
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
> 关联：R7 / R10。锚点：`src/context/budget/cheap_passes/structured/`、`src/harness/agent/think.rs`。
> ↳ 采纳·非红线（本条是 R10 的读法说明，不改 R10 一字）

**A3 · 状态可重建，趋向纯 Reducer (Reconstructible State, Toward a Pure Reducer, F5+F12)**
> **方向性承诺**：执行状态应尽量可从**单一持久源**重建；每轮 Think 趋向"对持久 context 的纯 reduce"（`prompt.rs` 已逐轮重建裸消息）。新增有状态机件时，优先让其状态可观测、可重建。
> **硬约束**：**不得**为此让 `src/harness/` 越过 R10 的 12 文件 / `budget.rs` 行数棘轮（当前 5055，以 `budget.rs::CEILING` 为准），或把业务状态搬进笨循环。具体统一方案先走"加代码前必答 3 问"，列为 backlog 评估（见审计文档 B §P2-1）。
> 关联：R10、P4（依赖倒置）。锚点：`src/harness/trait_def.rs::TurnState`、`src/looping/`、`src/goal/`、`src/agents/swarm/tasks/store/`。
> ↳ 采纳·非红线（方向性原则，落地需独立评估）

**A4 · 统一 Launch / Pause / Resume 契约 (Unified Lifecycle Contract, F6)**
> 把已存在的取消（`cancellation.rs`）、续跑（`resume_coordinator.rs`）、改需求打断/注入（`steering.rs` 三态 Steer/Interrupt/Queue）、workflow resume 命名为**一组生命周期契约**：任何长跑单元（goal / loop / workflow / team task）都应可被一致地启动、暂停、恢复、取消。
> **进展（2026-07-25）**：goal 与 loop 两条自治续跑链已补齐 pause/resume——loop 新增 `LoopStatus::Paused` 与唯一原子迁移原语 `LoopRegistry::transition`（goal 侧 `GoalStatus::Paused` 早有，本轮补 `GoalStore::pause_if_active`）。同批落地**跨会话生命周期管控**（`loop(action='stop'|'pause', session=…)` / `goal(action='clear'|'update status=paused', session=…)` + `stop_all` / `pause_all` 杀手闸），闭合"`list` 跨会话可见但只能在本会话停"的 R6/R8 断线。**边界规则（新增，适用于任何后续长跑单元）：跨会话只能"降活跃度"**——quiet（stop/pause/clear）可跨，arm（start/resume/加配额）必须在该单元自己的会话跑，因为下一步只由**它自己会话**的完成钩子认领；跨会话操作一律带工具内 operator 闸。详见 FEATURE_LOCATOR §4.1/§4.2。
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

> **会话模式 (Session Mode)**: 与执行档位正交的第三根会话旋钮——`chat` / `work`（默认）/ `code`，Panel composer 模式 pill 选（随第一条消息生效）或 `session_set_mode` 工具对话式切。模式只做**工具呈现面**的静态分区（schema 常驻核 × 整族延迟，`tool_search` 永远可发现+晋升——R10 渐进披露例外的形状），不授予不拒绝任何权限；审批仍归 exec tier。单一源 `src/config/types/policies/session_mode.rs`（族表 `_` 词边界匹配；MCP 限定名 `{server}__{tool}` 整体豁免内建表；子代理继承父分区并获短版 mode line）。详见 [MODE_SYSTEM.md](docs/reference/MODE_SYSTEM.md) 与 FEATURE_LOCATOR §5.16。

> **繁忙输入与消息车道 (Busy Input & Wait Lane)**: 会话已有在跑的 run 时，新消息按通道声明的 `BusyInputMode` 分流——`Steer`（默认，注入 live 日志让循环下一轮接住）/ `Interrupt`（真取消同会话 run 及其委派子运行）/ `Queue`（不打扰，排队）。**投递不了的一律进 `src/gateway/busy_queue/` 的 per-session FIFO 车道**，channel 与 Panel/CLI 三个 surface 共用同一条车道、同一套到达序与溢出策略（ticket **必须在到达路径同步取**，进 spawn 就把到达序换成调度序）。等待端**不轮询**：靠 `SessionRunRegistry::release` 的放槽信号唤醒（codex `InputQueue` 对位），`wake_fallback_secs` 只是漏发兜底。停止有两个粒度——`/stop` 清整条车道，`chat.abort` 按 `run_id` 停单条排队消息（排队中的 run 不在 `active_runs`，引擎的 cancel 够不到它）。旋钮在 `[execution] busy_queue_*` / `max_pending_steering`。详见 FEATURE_LOCATOR §4.8。

> **团队群聊直播面 (Team Group Chat Live Surface)**: 群聊的实时状态全部走 `team.<id>.*` 五类 topic——`message` / `system` / `activity` / `fanout` / `task.<verb>`，**信封只有一种**（唯一发布口 `gateway::event_emitter::team_fanout::publish_team_event`，per-run 的 `TeamFanoutEmitter` 也走它），Panel **只有一个解析点**（`views::chat::team_events::parse_team_topic`，后缀匹配、team id 可含 `.`、`team.changed` 不匹配）。Gateway 订阅是 `team.*` 通配 ⇒ **投影前必须先按当前 `chat.team_id` 作用域**，否则后台团队的气泡会挤进任意会话（含单聊）。`fanout started/settled` 是团队模式下 `active_run_id` 的**唯一写者**——它给群聊撑起 Stop 键（路由到 `teams.chat.cancel`，**不是** `chat.abort`：fan-out 树不在引擎 `active_runs` 里），并顺带接上 composer 已有的队列 auto-drain 忙→闲边沿。历史回放侧 `teams.chat.history` 只回放对话行（`MessageType::Message` 或无 recipients），定向收件箱流量不进群聊，`kind` 由服务端派生。详见 [MULTI_AGENT_SYSTEM.md](docs/reference/MULTI_AGENT_SYSTEM.md) §Group Chat 与 FEATURE_LOCATOR §4.5。

> **⚠️ 加了通道 adapter ≠ 用户能配**: 配置式通道的唯一入口是 `gateway/interfaces/plugin.rs` 那张工厂表——`create_channel_from_config` 查不到 `channel_type` 就 `return None`，`initialize_channels` 只打一行 `Failed to create channel`。表是 2026-04-05 引入的，之后新增的 5 个通道自己注册了，**之前就存在的 10 个（slack / discord / matrix / mattermost / signal / irc / nostr / xmpp / email / webhook）从未回填**，于是它们各自完整的 adapter + config + 测试在生产里整整不可达到 2026-07-26 才补上（`register_channel_plugins` + `register_plain_channel!`）。**新增 adapter 必须手工进这张表**——`every_configurable_channel_type_is_registered` 只钉当前集合防回归，枚举不了 `impl ChannelFactory`，抓不到将来的遗漏。`imessage` / `cli` 刻意不在表内（各有直连路径，注册即死码）。

> **频道寻址 = 两步，通道能力位 = 承诺**: `channel_message` 只吃不透明的 `conversation_id`（`C0A1B2C3` / 数字 chat id / JID），而这类 id **只有入站消息才会产生** —— 所以"把结果发到 #eng-releases"必须先 `channel_directory`（读，`Channel::list_conversations` 的唯一消费者）换到 id 再发。**两者刻意是两个工具**：`ToolFacts::idempotent` 按**工具名**取自 `READ_ONLY_TOOLS`，合并进非幂等的 `channel_message` 会让查询在 `Ask` 档一并被闸（档位只收紧不放宽，没有反向豁免口）。花名册**只读路由元数据**（名字/id/是否成员），**不读消息内容** —— 内容拉取会绕开只作用于**推**来消息的入站访问控制（`inbound_router::check_permission` / dm-group policy / pairing）。另一侧：`ChannelCapabilities` 的每个位都是承诺，**声明了就必须覆写对应的 `Channel` 方法** —— 默认体现在一律 `Err` 并指名道姓报"adapter 声明了却没实现"（此前是 `Ok(())` 静默成功，6 个 adapter 因此谎报，其中 msteams `react` / whatsapp `delete` 会让工具回 `delivered: true` 而对面什么也没收到）。详见 FEATURE_LOCATOR §5.18。

> **⚠️ Panel ↔ Daemon 资源嵌入链**: Panel UI 经 `rust_embed` 在 `aleph-server` **编译时**静态嵌入二进制，运行中的 daemon 不读磁盘 dist/*。改完 panel 看不到效果＝漏了重编 binary。完整刷新链（`just wasm` → 重编 server → 替换运行中 binary，dev / macOS .app / Windows 三种 daemon 替换法）详见 [DESKTOP_SHELL.md](docs/reference/DESKTOP_SHELL.md)。

> **⚠️ hook 注册了 ≠ hook 会触发**: hook 有**三个各自独立的静默死因**，且**没有一个会在触发时报错**——它只是不跑。① `matcher` 挂在**没有 tool_name 的事件**上（matcher 只测 `tool_name`，所以 SessionStart 上的 matcher 永假）；② `"kind":"interceptor"` 挂在**只派发 observer 的缝**上（message / provider / gateway / subagent 全局 fire-and-forget 缝）；③ shell/http 动作的 **consent 还是 pending**（前两个至少有加载期 `warn!`，第三个连 warn 都没有）。**唯一的诊断入口是 `hooks_manage(action='list', only_unreachable=true)`**——它读**运行时**清单（`HookExecutor::inventory()`），一次说全三个死因。**别去翻 `~/.aleph/hooks.json`**：那是四层里的一层，project / project-local / plugin 自带的 hook 在它眼里根本不存在，解析后的 `kind` 也看不到——`hooks.list`＝文件视图（`add`/`remove` 改的就是它），`hooks.registry`＝运行时视图，问"实际注册了什么"只能问后者。可达性谓词**只有一份**（`HookEvent::supports_matcher()` / `supports_interceptor()`，`types/hooks.rs`），加载期 warn 与 `reachable` 读同一个；事件全集同理只有 `HookEvent::ALL`。**两个上限别混**：`executor.rs` 的 64KB 罩**读 stdout/HTTP body**、截断＝硬错误（fail-closed，怕 `deny:` 被吞）；`output_budget.rs` 的 ≈2500 token 罩**进模型上下文**、超限溢写磁盘留路径（fail-soft）。**consent 绑的是脚本内容不是命令字符串**（approve 那一刻的哈希）——脚本改了＝按未批准处理，所以"我明明批过了"的第一问是"脚本动过吗"；而**任何工具都不能批准 hook**（能报告状态、不能授予），否则被注入的模型可以一回合内自写自批。详见 FEATURE_LOCATOR §5.10。

> **执行清单 (Execution List / Todo-Plan)**: 模型把任务分解成一条**带三态的清单**（`- [ ]` / `- [~]` / `- [x]`），落在该 agent 的 scratchpad markdown 里，同一份快照喂五个消费者——run 起点的 `<execution_plan>` prompt 块、模型（工具回显）、用户频道（progress push）、Panel Todo 条、`ScratchpadGoalVerifier` 停机守卫。**线上单一形状是 `shared/protocol/src/plan.rs::PlanSnapshot`**，唯一转换点 `builtin_tools/scratchpad.rs::plan_snapshot_dto`——加字段先动协议，别在 Panel 手抄第三份（曾并存三套 plan-step 词汇，其中两套零 producer，已 CUT）。写语义是**全量替换 + 状态保真**：裸文本项按文案继承旧状态（refinement 幂等），`{text,status}` 原样采信，单-in-progress 由代码强制。**分解本身 100% 归 LLM（R7）**——任何"判断这一步算不算完成"的代码都越线，守卫只能查模型自己的方框。**不要新建 `todo` 工具**：Panel 投影按字面工具名 `"scratchpad"` 取数，第二个工具＝第二个 store + 对现有 UI 完全隐形。详见 FEATURE_LOCATOR §3.13。

> **⚠️ `agent_trace` 流是有意有损的镜像**: `AgentTraceEmitSink` 用 bounded `mpsc(256)` + `try_send` 把 harness trace 镜像到 WS（满即丢，注释明写 best-effort），这是**刻意的**——绝不能让慢消费者背压 agent 循环。推论：**任何消费方都不得把逐事件流当作终态真源**。工具调用的权威终态在 `run_complete` 的 `summary.tool_summaries[]`（core 由 harness `tool_timeline` 构建，`tool_id` 与流事件同源 `call.id`），失败原因在 `summary.errors[]`，**执行清单（todo/plan）的终态在 `summary.plan`**（`aleph_protocol::plan::PlanSnapshot`，core 在自己那条不丢帧的 `event_drain` 里闩存）。新写消费者（Panel / channel / 外部 bridge）必须在流末对账，否则丢一帧就留下永久"进行中"的幽灵状态。Panel 侧参考实现见 FEATURE_LOCATOR §6.1（工具行 `reconcile_tools` + `settle_orphan_tools`；todo 条 `settle_plan`）。

> **交付物 ≠ 聊天记录 (Deliverable ≠ Transcript)**: Aleph 会生成**两种** HTML，混淆它们是这条注记存在的唯一原因。**交付物**＝模型主动调 `artifact_publish` 发布的成品（报告 / 分析 / 方案），落 `ArtifactOrigin::Deliverable`，在右栏置顶并**自动在系统浏览器打开**；**对话记录**＝`session.export_html` 手动导出的整段 transcript（按钮文案「导出对话」），是「给我看当时怎么做的」而不是「把结果给我」。把后者当结果递给用户，等于把答案埋进产生它的过程里。**什么算成品 100% 归模型判断（R7）**——`deliverable` 这个 origin 只可能由那次工具调用产生，任何"扫最终答案 / 看 run 结束了没"的启发式都越线。两者共用 `src/export/page.rs` 的文档外壳、CSP 与字节预算；**导出文档零 `<script>` 是硬约束**（与 Panel 同源，只有零 script 才配得上 `default-src 'none'`，加一个脚本就把这条兜底论证全部作废）。右栏**不再镜像工具调用**（与聊天列折叠内容逐字重复，旧 `components/inspector/` 整套已删）——想给右栏加面之前先问它是不是聊天列已经显示过的东西。**右栏一行能点开什么只有一个谓词**（`components/artifacts/preview.rs::PreviewTarget::for_item`：图片 / 可读文本走面内查看器，PDF·压缩包·已渲染文档保持外链），而"什么算可读文本"落在 `shared/protocol/src/artifact.rs::is_previewable_text` —— **offer 的一侧（Panel）与 serve 的一侧（`artifacts.read_text`）必须读同一份**，散成两份就是"点了永远报错"或"能读却只给下载"，两种都静默。⚠️ **右栏默认是收起的**（`LayoutMode::ChatOnly` + `translateX(100%)` + `pointer-events:none`）：任何长在面板里的提示（被拦横幅）在那个状态下等于不存在，所以"自动打开被拒"必须同时把面板打开；任何"面板里有新东西"的徽标必须数**面板真正装的东西**（`unseen_artifacts` 由产物列表差分驱动 —— 它曾经数的是工具调用，那是检查器时代的残线，为面板没有的东西亮、为面板真有的东西沉默）。另：**新增 `read_*` 一类只读 RPC 记得进 `gateway/lane.rs::override_for`**，后缀启发式不认它就落 Mutate 车道被幂等键守卫拒掉（只在 `require_idempotency_key` 部署上炸）。详见 FEATURE_LOCATOR §6.8。

> **⚠️ `MessageRecord.timestamp` 单位有歧义**: SQLite backend 写秒、file backend 写 `timestamp_millis()`（同一文件里 `created_at`/`last_active_at` 却写 `timestamp()`），trait 文档说秒——**三种说法同时为真**，两种拼写同时在盘上。曾有**五处**读取点各自 `from_timestamp(ts, 0)`，于是导出里出现 58536 年、Panel 侧栏给 7 月的对话标「03-02」。**一律走 `MessageRecord::instant()` / `rfc3339()`**（`src/gateway/session_store/types.rs`，1e11 分界），裸格式化就是这个 bug 的下一次复发。源头未改是**有意的**：该值同时是 `get_history_before` 的分页游标，改单位要连全部存量会话一起迁移。

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

Singleton 由 OS 级 `flock`（`~/.aleph/data/aleph.lock`）强制；CLI 写子命令经 `with_policy` 走 IPC 或本地拿锁，不与服务竞争。`kill -9` 后可立即重启。Spec C 不变量与回归脚本详见 [PROCESS_MANAGEMENT.md](docs/reference/PROCESS_MANAGEMENT.md)。

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
| **GRAPH_LAYER.md** | [docs/reference/GRAPH_LAYER.md](docs/reference/GRAPH_LAYER.md) — 循环治理图（loop-graph governance）：`src/loop_graph/` 六词闭集治理边 + 锚点/冻结/根参照 + 审计环，四种单循环失败（Goodhart/参照盲区/环冲突/测量衰减）的拓扑解法；spec 见 docs/superpowers/specs/2026-07-19 |
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
| EXTENSION_SYSTEM.md | [docs/reference/EXTENSION_SYSTEM.md](docs/reference/EXTENSION_SYSTEM.md) |
| PLUGIN_SYSTEM.md | [docs/reference/PLUGIN_SYSTEM.md](docs/reference/PLUGIN_SYSTEM.md) |
| WORKFLOW_INTEROP.md | [docs/reference/WORKFLOW_INTEROP.md](docs/reference/WORKFLOW_INTEROP.md) |
| SECURITY.md | [docs/reference/SECURITY.md](docs/reference/SECURITY.md) — 信任模型 + 工具权限三层（`tool_permissions` × exec tier × sandbox 硬底线，唯一强制点 `src/tools/scoped/`）+ 动作化审批门 + **codex / hermes / pi 对照表（Gap analysis，改权限模型前先看，别重做对比）** |
| **AGENT_IDENTITY.md** | [docs/reference/AGENT_IDENTITY.md](docs/reference/AGENT_IDENTITY.md) — 每 agent 独立 Ed25519 密钥 + 签名哈希链操作账本（`src/identity/`，生产者＝`tools/scoped/` 唯一咽喉，归属单一源＝`ledger_agent_id()`，**子代理由 `AllowlistToolService` 开 `as_actor` 签自己的活**，密钥生命周期本身进链，读/验＝`agent_identity` 工具 + 离线 `aleph-server identity`）；**威胁模型写明买到什么买不到什么**（不防拥有 `~/.aleph` 的对手、不防进程内冒充、对从未写入的记录无话可说——故 `lost` 计数落库并与 `ok` 并排返回）+ **buzz 逐维度对照表（改这层前先看那张表）** |
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
