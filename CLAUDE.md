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

### R10. 薄 Harness 哲学，笨循环编排核心 (Thin Harness, Dumb Loop)

> *"If you're not the model, you're the harness."* — Vivek Trivedy
> *"Models get stronger → harness gets thinner."* — Anthropic

- **薄 Harness 哲学 (Thin Harness)**: Aleph 采纳 Anthropic 流派，运行时极简、信任模型。Harness 是脚手架不是认知层。**模型越强，Harness 越薄** — 优秀的 Harness 必须通过"面向未来测试 (Future-Proof Test)"：换更强的模型，性能自然提升，无需修改 Harness 代码
- **笨循环 (Dumb Loop)**: `src/harness/` 仅承载 Think→Act 轮次调度，**不参与任何推理**。所有智能决策（意图理解、工具选择、安全评估、完成度判断）由 LLM 一次推理调用自然完成
- **核心边界**: `src/harness/` 锁 **12 文件**；行数由 `src/harness/tests/budget.rs` 的棘轮守（实测非手算、只减不增、增必答下方 3 问），当前 **5008 行**（2026-07-20 −62＝移除 `DiminishingReturnsDetector` 硬停 [R10 5-不 #3：loop 不做完成度判断]，`think.rs` 弃 `after_turn` 消费点、detector/`StopDiminishing`/`TurnMetrics` 删自 `src/context/budget/`，见 budget.rs::CEILING；2026-07-18 −2＝流式旁路修复：`stream_llm_call` 弃 `as_http_provider()` 降级分支、改多态 `execute_streaming_dyn`，副作用下沉 `src/providers/` 装饰器，见 budget.rs::CEILING；Batch 6，2026-07-17：两侧同日从 5035 出发、合并实测 5072——上调 +80＝ambient 审批关联 + 完成序 live 事件；删除 −42＝test-only `run_turn` 簇外迁 `tests/harness_ext.rs` + 恒零 `consecutive_errors` trace 字段删除；3 问作答在 budget.rs）。旧的 ~4900 系一次手算口径事故（生产 `impl` 中间的缩进 `#[cfg(test)]` 截断 `agent.rs`、静默漏计 846 行）的残值，**已退休**——红线是棘轮机制本身，不是那个具体数字：
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
> **硬约束**：**不得**为此让 `src/harness/` 越过 R10 的 12 文件 / `budget.rs` 行数棘轮（当前 5008），或把业务状态搬进笨循环。具体统一方案先走"加代码前必答 3 问"，列为 backlog 评估（见审计文档 B §P2-1）。
> 关联：R10、P4（依赖倒置）。锚点：`src/harness/trait_def.rs::TurnState`、`src/looping/`、`src/goal/`、`src/agents/swarm/tasks/store/`。
> ↳ 采纳·非红线（方向性原则，落地需独立评估）

**A4 · 统一 Launch / Pause / Resume 契约 (Unified Lifecycle Contract, F6)**
> 把已存在的取消（`cancellation.rs`）、续跑（`resume_coordinator.rs`）、改需求打断/注入（`steering.rs` 三态 Steer/Interrupt/Queue）、workflow resume 命名为**一组生命周期契约**：任何长跑单元（goal / loop / workflow / team task）都应可被一致地启动、暂停、恢复、取消。
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
> **信任模型 = 网络边界**: 默认只绑 `127.0.0.1`；`[gateway] host = "0.0.0.0"` 显式开放局域网。方法级门槛是 device tier（远程 Panel 默认 Chat tier，config 类 RPC 须 operator 提权），协议护栏是 WS Origin 校验。详见 [SECURITY.md#auth-ux](docs/reference/SECURITY.md#auth-ux)。

> **执行档位 (Exec Tier)**: 工具执行权限有一根面向用户的旋钮——`Ask` / `Auto`（默认）/ `Full`，Panel composer pill 选（本会话，随第一条消息生效）或 Settings → Policies 设（全局）。规则读工具**声明的元数据**（幂等 / destructive），不认名字；未知工具在 `Ask` 档 fail-closed；`[sandbox.command_policy]` 硬底线任何档位都压不下去。**唯一强制点是 `src/tools/scoped/`——任何新的能执行工具的 surface（新 RPC / 新快路径 / 新后台产地）不经过它就自带旁路**（已堵：斜杠快路径 / `tools.invoke` / 后台续跑）。详见 [SECURITY.md](docs/reference/SECURITY.md) 与 FEATURE_LOCATOR §5.12。

> **⚠️ Panel ↔ Daemon 资源嵌入链**: Panel UI 经 `rust_embed` 在 `aleph-server` **编译时**静态嵌入二进制，运行中的 daemon 不读磁盘 dist/*。改完 panel 看不到效果＝漏了重编 binary。完整刷新链（`just wasm` → 重编 server → 替换运行中 binary，dev / macOS .app / Windows 三种 daemon 替换法）详见 [DESKTOP_SHELL.md](docs/reference/DESKTOP_SHELL.md)。

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
- 极度节制 cargo 调用（系统负担）—— 默认不跑全量测试，高风险合并至多一次 `cargo check --lib`

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
| CLUSTER.md | [docs/reference/CLUSTER.md](docs/reference/CLUSTER.md) — Aleph 集群（单中心非对称节点联邦）：reverse RPC + `node_invoke`/`node_file` + 命令 allowlist + 审批回中心 + 断线 fail-fast |
| TOOL_SYSTEM.md | [docs/reference/TOOL_SYSTEM.md](docs/reference/TOOL_SYSTEM.md) |
| MEMORY_SYSTEM.md | [docs/reference/MEMORY_SYSTEM.md](docs/reference/MEMORY_SYSTEM.md) |
| └─ RAW_MEMORY.md | [docs/reference/memory/RAW_MEMORY.md](docs/reference/memory/RAW_MEMORY.md) |
| └─ NOTES.md | [docs/reference/memory/NOTES.md](docs/reference/memory/NOTES.md) |
| └─ RETRIEVAL.md | [docs/reference/memory/RETRIEVAL.md](docs/reference/memory/RETRIEVAL.md) |
| └─ DREAM_DAEMON.md | [docs/reference/memory/DREAM_DAEMON.md](docs/reference/memory/DREAM_DAEMON.md) |
| EXTENSION_SYSTEM.md | [docs/reference/EXTENSION_SYSTEM.md](docs/reference/EXTENSION_SYSTEM.md) |
| PLUGIN_SYSTEM.md | [docs/reference/PLUGIN_SYSTEM.md](docs/reference/PLUGIN_SYSTEM.md) |
| WORKFLOW_INTEROP.md | [docs/reference/WORKFLOW_INTEROP.md](docs/reference/WORKFLOW_INTEROP.md) |
| SECURITY.md | [docs/reference/SECURITY.md](docs/reference/SECURITY.md) — 信任模型 + 工具权限三层（`tool_permissions` × exec tier × sandbox 硬底线，唯一强制点 `src/tools/scoped/`）+ 动作化审批门 + **codex / hermes / pi 对照表（Gap analysis，改权限模型前先看，别重做对比）** |
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
