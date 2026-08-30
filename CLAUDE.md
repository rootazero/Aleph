# CLAUDE.md

> **Context Tiers**
> **Tier 1（本文件，每次加载）** = 红线 R1–R10 · 原则 P1–P8 · 12-Factor 采纳条款 · 开发指南 · **工程判据清单**（触发器）· 子系统路由表。
> **Tier 2（按需加载）** = `docs/reference/*`，判据清单里每条 `→ §x.y` 都指向那里的全文（通常是本文件曾经那份摘要的 2–4 倍深度）。
> **Tier 3（默认忽略）** = `docs/archive/`、历史规格，除非明确要求不碰。
>
> 本文件只承载**没有任何单份 reference 文档能说出口的东西**：跨子系统的约束、红线、以及"你不知道自己需要查它"的判据。**详情一律不写在这里**——写进来就是给每次会话付一遍钱。

---

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
- **精简原则 (prune-the-prompt)**: 智慧迁进 prompt ≠ 把 prompt 写厚。模型越强越需要**更少方向/约束/示例**——few-shot 会变笼子，低密度冗余稀释注意力；新模型发布后第一件事是**修剪上下文**，把验证/自我纠错建进**架构（运行时信号）**而非 prompt。加 prompt 字节前过**两把尺**：
  1. 这是模型**做不到的运行时事实**，还是我在**教强模型怎么思考**？后者别进 prompt。
  2. **这句话有没有一个工具拥有它**？有 → 写进那个工具的 `DESCRIPTION`（随 schema 发，且只发给真能调它的请求）。system prompt 只承载**没有任何单个工具能说出口的东西**（跨工具取舍 / 运行时事实 / 安全边界）。
- **两把尺都已建进架构**：`src/thinker/prompt_contract.rs` 的 `reachable_layers` / `scaffold_bytes_ratchet` / `no_sentence_is_stated_twice`；量一下用 `aleph-server prompt-size`。
- **⚠️ 第二把尺有前置条件**：见判据清单「目录条目写字面量会整体遮蔽工具常量」——尺子量不到的地方，搬过去等于删掉。
- 详见 [HARNESS_PHILOSOPHY.md §8](docs/reference/HARNESS_PHILOSOPHY.md) 与 FEATURE_LOCATOR §1.1

### R10. 薄 Harness 哲学，笨循环编排核心 (Thin Harness, Dumb Loop)

> *"If you're not the model, you're the harness."* — Vivek Trivedy
> *"Models get stronger → harness gets thinner."* — Anthropic

- **薄 Harness 哲学**: Aleph 采纳 Anthropic 流派，运行时极简、信任模型。Harness 是脚手架不是认知层。**模型越强，Harness 越薄** — 优秀的 Harness 必须通过"面向未来测试 (Future-Proof Test)"：换更强的模型，性能自然提升，无需修改 Harness 代码
- **笨循环**: `src/harness/` 仅承载 Think→Act 轮次调度，**不参与任何推理**。所有智能决策由 LLM 一次推理调用自然完成
- **核心边界**: `src/harness/` 锁 **12 文件**：
  - 顶层 (8)：`mod.rs` / `agent.rs` / `deps.rs` / `trait_def.rs` / `callback.rs` / `chain_context.rs` / `trace.rs` / `trace_sink.rs`
  - `agent/` (4)：`think.rs` / `act.rs` / `guardrails.rs` / `prompt.rs`
- **行数红线＝棘轮机制本身，不是某个具体数字**：由 `src/harness/tests/budget.rs::CEILING` 守（实测非手算、只减不增、增必答下方 3 问）。**代码是权威——本文件刻意不复制那个数字**，因为文档抄一份就漂移过一次。逐轮增减账与 3 问作答在 `budget.rs::CEILING` 与 `src/harness/CLAUDE.md`
- **行数增长红线**：任何新增 LOC 必须先答 3 问。新增文件需在 PR 描述里说明为何无法装进现有 12 个文件之一
- **循环里的 5 个"不"**:
  1. ❌ 不判断意图分类
  2. ❌ 不按**消息意图**做工具过滤 / 相关性评分（渐进式工具披露例外，见下）
  3. ❌ 不做完成度判断（除模型显式 stop）
  4. ❌ 不做内容审查 / 安全打分
  5. ❌ 不做错误恢复策略选择
  - ⓘ **渐进式工具披露例外（Aleph 采纳）**: "core 工具静态常驻 + 全量工具目录 + `tool_search` 元工具按需加载 schema" 是**不看消息内容的静态分区**、加载决策 100% 由模型发起，与 `src/tools/scoped/` 已有的三道静态 `retain` 同层同性质，**不属**第 2 不所指的"按意图过滤"。分区落在工具呈现层，**不进 `src/harness/`**
- **12 模块各归其所**: 行业共识的 12 大 Harness 模块每一个都有独立物理位置，**不在 `src/harness/` 内堆积**
- **YAGNI 撤回模式**: 任何"零现有消费者"的抽象立即删除/撤回，绝不"为未来留口"
- **加代码前必答 3 问**:
  1. 这是脚手架还是认知？认知必须搬到 prompt
  2. 模型升级一档还需要它吗？不需要就删
  3. 现在有几个真实消费者？零个就撤回
- **关联**: R3 + R7 + R9 在 Agent Harness 工程上的落地。详见 [HARNESS_PHILOSOPHY.md](docs/reference/HARNESS_PHILOSOPHY.md)

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
- **相关逻辑物理聚合** — 紧密相关的类型、函数、trait 放在同一模块目录下
- **命名即文档** — 模块名、函数名、类型名应准确反映其唯一职责
- **大文件及时拆分** — 单文件超过 500 行应考虑按职责拆分 (参见 [CODE_ORGANIZATION.md](docs/reference/CODE_ORGANIZATION.md))

### P3. 可扩展性 (Extensibility)

- **开放-封闭原则 (OCP)** — 新增功能通过实现 trait / 注册插件完成，不修改已有核心逻辑
- **策略模式优于条件分支** — 用 trait object / enum dispatch 替代 `if-else` 链或 `match` 的无限膨胀
- **插件化优先** — 非核心功能优先实现为 Skill / MCP Server / WASM 插件
- **Schema 驱动** — 接口使用 JSON Schema (schemars) 自描述，新增字段不破坏旧客户端

### P4. 依赖倒置 (Dependency Inversion)

- **高层模块不依赖低层模块，两者都依赖抽象** — Core 定义 trait，具体实现在 crate 边界之外
- **实践**: `DesktopCapability` trait 在 core 定义、实现在 `desktop/shared/`；`MemoryStore` trait 在 core 定义、SQLite+sqlite-vec 实现在 `src/memory/` 但可替换
- **构造时注入** — 通过 `AppContext` / Builder 在启动时组装依赖，运行时不 `new` 具体类型

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

- **语义理解交给 LLM** — 自然语言 → 结构化意图的任务，优先 LLM 语义理解，而非正则或关键词匹配
- **禁止脆弱的模式匹配** — 不要用 regex 解析用户自然语言。正则只适用于格式固定的机器生成文本
- **LLM 可做则 LLM 做** — 分类、提取、推理、生成交给 LLM，而非硬编码规则
- **结构化输出** — LLM 返回 JSON，代码层只负责解析和执行，不做语义判断

---

## 🧭 12-Factor 对照与采纳 (12-Factor Conformance & Adoption)

> 本节是叠加在 R1–R10 / P1–P8 之上的**映射层**——**不改任何红线/原则，不设新红线**。逐 factor 证据、锚点与模块改造清单见 [TWELVE_FACTOR_AUDIT.md](docs/reference/TWELVE_FACTOR_AUDIT.md)。

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
> F7 的"人在环"有两条腿：R5 的非阻塞多端推送 + `ask_user` / `ClarificationManager` 的**阻塞式澄清**（锚点 `src/builtin_tools/ask_user.rs` + `src/clarification/`，默认开启）。

### 采纳条款 (Adoption Clauses) — 工程承诺级，**非红线**

**A1 · 自有 Context Window (F3)**
> agent 按 **Prompt → Context → Harness → Loop** 四层构建；**Context 层**（历史压缩 / 压缩时机 / 内容类型路由缩减 / FTS5 检索回注 / 记忆三支柱）是一等工程关切，**context 的取舍由我们显式拥有**，不外包给框架默认消息格式。
> 关联：R9 / P6。锚点：`src/context/`、`src/thinker/`。

**A2 · 错误压缩 ≠ 错误恢复 (F9)**
> **采纳**：把工具/Provider 错误**压缩并呈递给模型**，让模型下一轮自愈。
> **仍禁**（= R10 第 5 不）：在 harness 里做**确定性的"错误恢复策略选择 / 多级重试矩阵"**。
> 边界一句话：**让模型看见并自愈错误 = 要；让 harness 替模型挑恢复策略 = 不要**。
> 锚点：`src/tool_output/structured/`、`src/tool_output/hygiene.rs`、`src/harness/agent/think.rs`。

**A3 · 状态可重建，趋向纯 Reducer (F5+F12)**
> **方向性承诺**：执行状态应尽量可从**单一持久源**重建；每轮 Think 趋向"对持久 context 的纯 reduce"。新增有状态机件时，优先让其状态可观测、可重建。
> **硬约束**：**不得**为此让 `src/harness/` 越过 R10 的 12 文件 / `budget.rs::CEILING` 棘轮，或把业务状态搬进笨循环。
> 锚点：`src/harness/trait_def.rs::TurnState`、`src/looping/`、`src/goal/`、`src/agents/swarm/tasks/store/`。

**A4 · 统一 Launch / Pause / Resume 契约 (F6)**
> 取消（`cancellation.rs`）、续跑（`resume_coordinator.rs`）、打断/注入（`steering.rs` 三态 Steer/Interrupt/Queue）、workflow resume 合称**一组生命周期契约**：任何长跑单元（goal / loop / workflow / team task）都应可被一致地启动、暂停、恢复、取消。
> 由此推出的两条**跨单元规则**见判据清单 §0。逐轮进展见 FEATURE_LOCATOR §4.1 / §4.2。
> 关联：R5 / R6。统一 API 面（薄 facade，**不进 harness**）列为 backlog。

---

## 🛠 技术栈与禁用清单 (Tech Stack & Do NOT introduce)

**核心栈**: Rust Core (tokio + serde) · 记忆层 SQLite + sqlite-vec · 接口 JSON Schema (schemars) · Panel Leptos/WASM · 桌面壳 Tauri。

**Do NOT introduce unless explicitly requested**（基于 R1/R3/R7 推导，违者不得合入）:

- **为 Aleph 自身代码引入第二个 async runtime**（async-std / smol）—— 一方代码全栈锁定 tokio（Cargo.lock 中的 async-std 是三方传递依赖，不影响此禁令）
- **独立向量数据库 client 进 core**（qdrant / lancedb / milvus 等）—— 记忆层已锁 sqlite + sqlite-vec
- **`src` 中直接依赖平台 API crate**（windows-rs / core-graphics / cocoa / objc / winapi）—— 违 R1，必须走原生 Bridge IPC
- **正则 / 规则引擎做意图识别或路由** —— 违 R7/P8，语义判断交 LLM
- **非 serde 的序列化栈** —— 全栈 serde

---

## ⚠️ 工程判据清单 (Hard-Won Criteria)

> 每一条都是一次**静默失效**的代价——没崩溃、没报错、测试全绿，只是"这个功能好像从来没生效过"。
> 这里只放**触发器**（判据 + 关键锚点），全文在 `→` 指向的 §（深度通常是这里的 2–4 倍）。
> **改动某子系统前先扫一遍对应分组**；「跨子系统通用形状」每次改动都适用。

### 0. 跨子系统通用形状（每次改动都适用）

- **一个被授权的值，如果经模型可写的字段送达，判它的那道闸就分不出它和模型编的那个** —— 而闸是照着「模型编的」写的，所以它拒的正是那个授权值。`run_loop` 把 run 的 effective workspace 注入进每一次省略 `working_dir` 的 `bash`/`code_exec` 调用（`registry_adapter::execute`），`WorkspaceSandbox` 却把 cwd 钉在 `workspaces/<sha256(session)[..16]>`：32 位十六进制目录名永远不等于 agent id、也不等于项目路径 ⇒ **出厂装机上每一次不带 `working_dir` 的 shell 调用都是 `Capability denied: cwd outside workspace root`**（`[sandbox] enabled` 默认 true，关掉换来的是拒绝一切的 `NoopSandbox`）。而这句话在仓里有**四份表述**，三份说「有效工作区」（`RuntimeContext.working_dir` doc / 每轮发给模型的 `cwd=` / 注入本身），说了算的是最少数的那一份。判据三句：① **一个网关拥有的值要走网关拥有的通道**（现为 `sandbox::context::EXEC_WORKSPACE`，`run_agent_loop` 发布、模型侧写不到），别塞进工具参数再指望下游认得出来；② 这类缺陷**四轮单测看不见**是结构性的——沙箱测试手搓 `SandboxCommand`（没有工具，没人注入）、工具测试跑假沙箱（有注入，没有 containment），**只有把两半放进同一个进程才看得见**（`tests/exec_workspace_jail.rs`）；③ 注入式默认值顺带吃掉了模型的相对路径（`working_dir:"src"` 被**替换成**根而不是解析到根之下），所以删掉注入同时修好了两件事 → §3.15
- **恒真的谓词等于没判，且它撒的谎只有对面看得见** —— 「有没有 handler」是结构性恒真，「这件事真做得到吗」才是谓词。⚠️ 而「不是闸」有**四张脸，四张都长得像在工作**，只有一个问题分得开：**在什么情况下这东西会变红？** 答不出一个具体情形，它就不是闸。**恒红**（一条检查被注册在它的前提**永远不可能成立**的地方——只在已 boot 进程里才判得动的那条挂进了离线注册表 ⇒ 每次调用、每台机器、永远开火，那个"CI 可以据以门禁"的退出码于是成了常量）· **恒绿**（一次扫描的语料里**不含**它要找的那种东西 ⇒ 欠一个否定用例）· **不可失败**（吞退出码 / 短路 / 断言一个结构性恒真的谓词，即本条）· **没装上**（闸本身从未被安装，而它"未安装"的回落值落在某个合法配置的取值范围内）→ 附录 C.4
- **零消费者的通道优先 CUT，不 CONNECT** —— R10；接一条死抽象比删它贵。⚠️ **但「优先」不是「一律」，而分辨它们的问句不是"这东西有用吗"（什么都有用），是「这个问题在仓里已经有几个答案」**：同一天在 `providers.*` 上抓到的两个零客户端 RPC 要了**相反**的处置——`needsSetup` 是"agent 能不能作答"的**第三个**答案（Panel 清单一个、它一个、真相一个）⇒ CUT；`healthcheck` 答的那一问**没有别的答案**（`providers test` 只覆盖一个，`aleph doctor` 是整个引擎 + 散文 finding，而无头部署没有 Panel）⇒ CONNECT，代价是一条 CLI 命令。判据的第二半是**这个能力的第 N 个答案本身就是缺陷**：三个答案里有两个是错的，且方向相反（"有没有行" vs "点过 Test 没有"），因为没有一个是在问真正的那件事
- **修掉你读到的那几个实例，不等于修掉那一类——而这两者在测试套件里长得一模一样** —— 「收敛写者时要数一遍写者」的第二形态，这次连着犯了两轮：`TempDir` 的 guard 绑在**一个会返回的帧**里，树在调用者用它之前就被删掉，而 SQLite 继续用它早就打开的 fd 写、被测代码自己 `create_dir_all` 回来 ⇒ **零报错、零红测**，唯一症状是磁盘上多一棵没人认领的树（本机累计 7 623 条 / 4.0 GB）。上一轮我在 `dreaming` 里修了两处、把判据写进了记忆，**同一个子系统里还剩四个**，因为我修的是「我读到的那几行」。判据三句：① 写下"这一类修完了"之前，**写一条会按名字红的守卫**（`utils::scratch::no_helper_drops_the_scratch_guard_before_returning`，源码级——运行时分不出"guard 提前 drop 了"和"guard 干完了活"），然后**手动破坏它一次**看它红、且点得出文件行号；② **`static` / `OnceLock` / `LazyLock` 永远不 drop**，所以挂在它们上的 `impl Drop` 从来不跑——`mem::forget` / `TempDir::keep()` 的理由永远写着「在测试二进制里可以接受」，那句话**每次运行都成立、第四千次不成立**，且它漏掉的不止目录（两个 e2e 探针把 `aleph-server` 停在 `static OnceCell` 里，每跑一次留一个**还在监听端口的服务进程**）。正确的钩子是 `atexit`（正常返回与 `process::exit` 都跑，后者正是 libtest 结束失败运行的方式），单一源 `utils::scratch::{keep_until_exit, reap_on_exit}`；③ ⚠️ **量具会骗人：`ls -1` 不列点开头的条目**，而 `tempfile` 的目录就叫 `.tmpXXXXXX` ——第一次"测完"我报的是 2 条/次，真数是 60 条/次。**用一个量具下结论之前，先确认它看得见你要数的那一类东西**（末态：全量 lib+集成一轮留下 3 个空目录 / 0 字节 / 0 个孤儿进程）
- **一个传感器没有生产者，不是「功能休眠」，是一个谓词在静默地回答相反的问题** —— 而它诞生的方式是**一次正确的 CUT 撞上一次正确的 restore**。`record_activity()` 被 2026-08-08 断线审计以「零调用点」为由删掉（判断没错），**同日**另一支 autonomy 合并因为要用它而恢复了**全部消费者侧**（`activity_checker` / `Interrupted` / `Cancelled` / `is_vacuous_interruption` / 子循环接线）——恢复的是它自己够得到的那一半，生产者那一半**没有编译器替你找**。留下的东西比缺哪一半都糟：`idle_seconds()` 从此量的是**进程运行时长**，于是「每阶段让位给用户」在开机 15 分钟后恒假（纸面设计），15 分钟内恒真且**方向相反**——进窗口就开一个在第一个 stage 中止的 cycle，记成 `success`，把每天仅一次的做梦预算花在一个什么都没做的夜晚上，同时往 churn 窗灌一条空事件（正好在语料被动得最多时解除检测器武装）。判据三句：① **一次 CUT 与一次 restore 撞在一起时，去数被 CUT 的东西有几个面**——消费者侧编译器会替你叫，生产者侧不会；② 传感器类的东西欠一条**源码级 census 点名它的生产者**（**先剥注释行**，否则一句提到函数名的 doc 就能满足它；**且这条 census 要挂在那个已经知道「谁会起 run」的表上而不是自己列名字**——见「一条守卫如果按名字列举它要检查的成员」的第二形态）；③ 生产者是**白名单不是共享路径**——机器流量（cron / heartbeat / A2A / 模型驱动的委派）若也盖戳，夜间自动化会把这套机制永久饿死，而它churn 的语料恰恰最需要维护 → memory/DREAM_DAEMON.md §2.1
- **一个机制的存在理由如果写在另一个文件里，删它的人不会读到那里——而删除本身没有测试** —— 「同一事实的两份表述」最贵的形态：不是两份描述漂了，是**其中一份被当成残留清掉了**。`interfaces/webchat/dist/.gitkeep` 唯一的用途说明在 `.gitignore` 的注释里（就在保留它的那条规则上面两行），哨兵被读成 vestigial 删掉 ⇒ `#[derive(RustEmbed)]` 的 `folder` 不存在是**硬编译错误**，`alephcore` 在任何全新 clone 上四条错误起步，而删它的那笔改动在作者机器上有 dist/ 所以全绿。判据两句：① **删一个"看起来没用"的文件/常量/条目之前，grep 它的名字**——它的理由通常写在一个不会被一起打开的文件里（`.gitignore` / build 脚本 / 另一个 crate 的 doc）；② 修法**不是把哨兵加回来**，而是让那条要求**自满足**（`build.rs` 在自己 crate 编译前 `create_dir_all`），因为一条靠"有人知道"活着的约定会被删第二次
- **守卫要断言「效果到达了」，不是「调用发生了」** —— 问「把这一步的返回值扔掉，测试还绿吗？」绿 ⇒ 你守的是产地不是连线 → §3.5
- **一张列举法名单的上游，常常是一个装了两种东西的字段** —— 白名单存在，往往不是因为作者偷懒，而是因为**形状答不了那一问**，消费者只能靠字段之外的信息重建区别。`ScratchpadOutput.content` 同时装原始 markdown 与进度回显 ⇒ 频道推送只能靠 `PROGRESS_ACTIONS` 四个动作名判断"这算不算进度"，而名单只描述立法当天的动作。删名单之前先问**这两种东西凭什么共用一个字段**；拆成两个字段之后判据回到形状上，名单整体消失（不是缩短）→ §3.13
- **一个「报成功的 no-op」，修在执行者身上比修在每个已知实例上便宜——而它第一次说出口时报出来的数目，才是那一类的真实大小** —— 上一轮按字段修了三个 `skip_serializing_if` 的 config section（一个 section 空了就不被序列化 ⇒ `merge_sections` 找不到 ⇒ warn+skip ⇒ **删最后一个元素静默 no-op 却报成功**）。那是列举法：只覆盖立法当天的三个字段，第四个加进来时不会有人知道。这一轮把判据搬到执行者自己身上——**命名了却不在序列化里的 section ⇒ `Err`，不再 warn+skip**——第一次跑就抓到**两个没人预测到的**：`providers` 是**刻意**不可清空的（`guard_incremental_providers` 会响亮报错，属误报，判据因此欠第三条出路，而那条出路要从 guard 的**函数名**派生不要列举）· 而 `config.patch` 对一个 `Config` 上根本不存在的 section（那条测试里逐字写着 `"ui"`）一直答 `success: true` **并发 `config.changed` 事件**，测试断言的正是这个谎。⚠️ **边界要写窄**：「缺席 ⇒ 删掉磁盘上那一节」是错的修法——那个 fail-soft 是承重的（`save.rs` 两个 guard 的存在就是因为不完整的 `Config` 曾抹掉过 embedding providers），把它写进 merge 等于把整类潜伏 bug 从**静默跳过**升级成**静默擦除**。要的是「让 no-op 说出口」，不是「让 no-op 变成删除」。
- **收敛写者时要数一遍写者，不然最后那个是靠没人扫过活下来的** —— 「所有写 X 的路径都必须走单一源」这类收敛**分三轮**才做完过：`upsert_section` 收了 `set_objective`/`set_plan`（§3.13 ⑤），一轮后发现 `set_item_status` 没加入（round-2 ②），再一轮后发现 `append_note` **也**没加入（2026-08-10 ②）——每一轮都以为自己扫完了，每一轮的漏网者都在报 `success: true` 地做 no-op。判据：写下"现在所有写者都过单一源了"之前，**grep 出这个 section / 这张表 / 这个键的全部写入点并逐个点名**，别按记忆列举 → §3.13。⚠️ **第四轮（2026-08-20）是我在同一次改动里对自己犯的**：marketplace 的 `add` 有**四**个面（Panel / `plugin_manage` / `aleph-server` 子命令 / `aleph` CLI），我把前三个 compose 成 add+update、在文档里写下「三个面因此都在」，而第四个就在我刚编辑过的文件里。**「我数出来是三个」本身就该触发一次 grep**——这一族里数错的方向永远是少数一个。
- **终局记账要挂在「那个值」上，不是挂在「产出它的那个分支」上——而调用方的注释会替这条断线作证** —— 「收敛写者时要数一遍写者」的镜像：那条讲一个动作有 N 个写者，这条讲一个**返回值**有 N 个生产者，而收尾只写在其中一个的臂里。`apply_tool_call_guardrail` 的 `GuardrailDecision::Block` 臂做全套收尾（`ToolError` 事件 / `on_tool_call_done` / 时间线 / trace），而**第二个产出同一个 `ToolCallGuardOutcome::Block` 的地方**（`Sanitize` 结果 reparse 失败的 fail-closed 臂）只喊一声 `on_safety_block` 就裸返回；两个调用点都按「guardrail 已经发过 ToolError + trace」处理并 `continue`，其中一个**逐字这么写在注释里**。于是那次调用被请求、然后在任何地方都没有解决：孤儿 `tool_use`（下一次 `build_prompt` 把整个 assistant 轮丢掉）· 永久转圈的工具卡 · 权威 `tool_summaries` 少一条。判据两句：① **grep 这个枚举值有几处 `return`/构造点**，别 grep 产生它的那个分支；② 修法是让那个值只有**一条**出生路径（`guardrails.rs::settle_blocked_call`），不是在第二处再抄一遍收尾 → §3.1 Round 9 ②
- **一个把 N 个分类扇入 1 个值的 `match`，等于没分类——而 `#[allow(clippy::match_same_arms)]` 就是那个 tell** —— 「恒真的谓词等于没判」的**分类**形态：谓词算出来了，然后被扔掉。`run()` 的退出臂算 `HarnessError::class()` 再把四个 `ErrorClass` 全映射成 `Cancelled`，于是一次 provider 鉴权失败在 Panel/TUI 的 trace 里渲染成中性的"已取消"（`Info` 状态），比命中上限还轻，且与用户按停逐字节同形——**而紧挨着的会话日志写的是 `Errored`，一份事实的两条投影互相矛盾，说谎的是有渲染器的那一条**。⚠️ 同族第二半更贵：**一句无条件的"统一归口"赋值会抹掉别处刚设好的真实原因**——`ReactiveCompactExhausted` 的唯一生产者设好它就返回那个 `Err`，正落进这条臂被覆写，于是那个变体（doc 自陈存在理由是"阻止该路径匿名泄漏"）从未活着到达任何读者。判据：**看到一个 `_ =>` 或多臂同值的归口赋值，去问"它盖掉的那个值是谁写的、什么时候写的"** → §3.1 Round 9 ③
- **一个「只减不增」的闸，设在实测值之上的那段差额就是它已经发出去的额度——而发放时的措辞永远是"给下一个 PR 留点余量"** —— 棘轮 / 配额 / 上限这一族的通用形态，在 R10 行数红线上犯过四次，前三次是攒出来的、第四次是**批准**的：一笔改动把 `CEILING` 设成 `实测 + 1024`，注释同时写着 "headroom … so the next adjacent PR doesn't trip the ratchet" 和 "still a hard cap, not a soft floor"——**这两句不可能同时为真**，而结果是那条红线允许 20% 的无声增长。判据两句：① **下调不需要答三问，把闸设在实测值之上需要**——那不是记账，是发放；② 这类闸的健康度不是"它有没有被触发过"，是**"它离实测值有多远"**，所以每次下调都要把闸拉回贴着实测值 → `src/harness/CLAUDE.md` 第四种漂移 · §3.1 Round 9 ①
- **列举法只覆盖立法当天的世界** —— 白名单式判据（"这算不算合成消息"/"保真字段集"/"受支持的维度"）必然漂移；改问**这段字是谁写的**、**不在我这张表上的那部分呢**。⚠️ **当"默认＝全都要"时，重放一份清单不是恢复而是收窄**：判据要倒过来问「我这次重放，把哪些原本会到达的东西挡在外面了」（Panel 重连只重放 3 个字面量 topic，把"无 filter ⇒ 收全部"的新连接压成只收那三类，`stream.*` 自此静默死亡而连接灯是绿的）→ §6.1
- **扇出的每个成员各自去读同一个"来源"，就有 N 份来源** —— 读取发生在扇出**之后**时，"同一个来源"只是措辞：detached 的成员读到的"现在"是**它开始跑的那个现在**，不是委派的那个现在（后台子代理会 fork 到委派之后的未来），而 K 个成员各读各的就得到 K 份不同前缀 ⇒ **K 次全价 cache write 而不是 1 写 K−1 读**。判据：**这份来源在我读它的时候还会变吗**；会变就在扇出前快照一次并把快照传下去（`fork::ForkSource = Arc<Vec<…>>`）。⚠️ 这一类的**症状只出现在账单上**——每个成员都返回了正确结果，测试全绿 → §4.11 round-12
- **一个角色的 prompt 承诺，要和它的默认输入并排读一遍** —— 两句话分别看都对，合起来才是 bug：`verify` 的 prompt 逐字写着「try to break the implementation, don't confirm it」「report what you actually observed, **not what you expected**」，而它的默认 `context_mode` 是 `Summary` ＝ 先读**被对抗方对自己刚做的工作的自述**、再去看现实。判据：凡角色声明了立场（对抗 / 独立 / 第二意见），去看它**默认**吃进什么；这类默认值是**正确性**属性，不是便利性，选错的方向必须是"要出声地要求"而不是"默认就给" → §4.11 round-12
- **两条投影同时喂同一个 append-only 状态，就会翻倍** —— 一份事实在线上有两种投影（权威流 vs 有意有损的镜像）时，消费者往往被写成"每条投影都是唯一来源"。加一个消费分支前先问**这条事实还会不会从另一条路再来一次**，以及**我这个容器是覆盖式还是追加式**（覆盖幂等、追加翻倍）。**测试不会告诉你**：单投影测试各自全绿，而两条投影从不在同一个测试里交错——那正是生产中唯一存在的调用序 → §5.13
- **一句关于"什么被闸住"的话，往往有三份拷贝，其中一份是发给模型的** —— 代码里的地板、doc comment、以及每回合进 prompt 的那句描述。改地板时三份一起改；**发给模型的那份说了假话最贵**（`Full` 档告诉模型"nothing pauses for confirmation"，而工具自声明的确认门根本不看档位）→ §5.12
- **同一事实的两份表述，只改一份就是静默说谎** —— 代码 vs 工具 `DESCRIPTION`、代码 vs 文档数字、代码 vs 注释；**注释正是说谎的那一方**。⚠️ **一条"已知缺口"记录也是一份表述，而且它会被后来的修复悄悄作废**：gap 0（档位目录 member 不可读）在被裁决时**早已在 wire 上关掉了**——carve-out 与收窄响应都在，只有三处文档还在描述旧世界。开工修一条记录在案的 gap，第一步是**去代码里确认它还成立**，第二步才是修；确认成立不了时欠的是一条守卫（把"现在是对的"钉住），不是一次改动。⚠️ **反向同样成立，而且更难看见：一个缺陷会被别处引用成安全论证，修掉它就悄悄抽走了那条论证的一条腿**。`agent.resume` 豁免 agent 准入闸的两条理由之一，逐字是「撤销本来就要重启才生效」——那句话把 `agent_update` 运行时半边的 no-op 当成了自己的地基；把 no-op 修成真的之后，那条窄缝从「重启之后才可达」变成「紧跟撤销就可达」，而 resume 那侧**一个字符都没动、一条测试都不会红**。判据：**修一个缺陷之前，grep 谁把这个缺陷写进了自己的理由**（找「因为它本来就……所以这里不用管」这种句式），不只 grep 谁调用了它——调用点编译器帮你找，论证只有你自己找得到。⚠️ **第二半（2026-08-10 补闸时才看清）：剩下的那条腿往往也不是它自称的那么宽**。resume 的 ② 是「续跑不可操纵」，字面为真——但 `allowed_users` 守的是 `tool_permissions` 的 agent 轴，而续跑**重新进 harness、继续按那个 agent 的权限调工具**。「那份工作已经过闸」只对**已经做完的**那部分成立，对下一轮模型自己想出来的动作一个字都没说。判据：**一条腿倒下之后，别默认另一条撑得住——去问它覆盖的是不是同一件事**（现闸在 `resume_named_session`，排在 `session_visible` 之后）。⚠️ **第三半的方向是反的，而且它没有触发时刻**：前两半都发生在「修一个缺陷之前」——一条既存缺陷在替别人的论证撑腰；反过来是**写下一条新发现之后**，去 grep 有没有哪条既存记录的**理由**建立在你刚刚收窄掉的那件事上。调用点编译器替你找，论证只有你自己找得到，而这一次连症状都没有：**结论还对、理由已经死了**，所以从结论看它永远是绿的——一条观测记录排除某个假设的理由是「这条测试持有那个 guard」，而同一批提交里、四十行以上刚刚写下：持有那个 guard 是**写者**的义务，那句话清不掉一个**读者侧**的比较。⚠️ 而**更窄的理由不是吹毛求疵**：两种措辞**准入的测试集不同**——那条测试确实安全，但真的原因是那把锁**连续持有**（guard 作为字段、`let _g = …` 绑到函数结尾），不是「持有过」；一条在两次读之间**取过又放掉**它的测试满足旧措辞、且仍然暴露 → 附录 C.5
- **一个「只朝一个方向拨」的覆写，缺的不是一行 `else`——先问那个被覆写的值本身可不可信** —— 上一条的第三半，也是这一族里最贵的一次自我纠正：`stream_protocol == EditBased ⇒ 打开流式` 没有关闭臂，读起来就是「漏了 `else`」，而**那正是错的修法**。① 补 `else` **不够**：`line`/`wechat` 声明 `EditBased` 而它们的 `edit` 无条件 `UnsupportedFeature`，是被 `if` **主动推进**坏路径的——写在 `else` 里的地板永远跑不到最坏的那两个。② 补 `else` **过宽**：`slack`/`mattermost`/`msteams` 声明 `None` 却真能编辑、今天流式正常，`else { false }` 把静默截断修成**可见的功能倒退**。真正的谓词既不是被覆写的值也不是覆写者，而是**这件事物理上做不做得到**（`caps.editing`）；放宽臂与收窄臂分别回答**偏好**与**可能性**，先放宽后收窄，于是对今天工作正常的通道逐字节 no-op。⚠️ 配套一条独立的：**「覆写存在」不是能力证据，要读它返回什么**——`line`/`wechat`/`signal` 的 `edit` 覆写只是把默认 `Err` 换了句措辞，`feishu` 的还转发一层给同样无条件 `Err` 的 `MessageOps::edit`；按「有没有 `async fn edit`」列免疫名单，四个通道全判反。这一条**运行时查不了**（覆写着只为返回 `Err` 与根本没覆写完全同形），所以守卫只能是源码级的 → §5.7
- **一句写在注释里的"这做不到"，比一句写错的事实贵——它不只描述错了代码，它把修复也一起否掉了** —— 「同一事实的两份表述」那条的极端形式。`scratchpad(request_approval)` 的 doc 逐字写着「advisory by construction… **a gate that enforced itself would need a plan-state machine inside the loop — that is cognition, and the loop does not hold cognition**」，于是「批准之后什么都不变」被读成一条 R10 不变量，而不是一根没接的线：**零件全在**（`ExecTier` 是档位、`scratchpad` 是计划文件、`clarification::ask` 是人类闸），缺的只有中间那一步。那句话真的只有一半对——**决定**是认知（归人类），把解析器**已经算好**的档位翻一下是记账。判据两句：① 读到「这需要 X，而 X 违反红线」时，先问**它真的需要 X 吗**，把「谁做判断」和「谁记结果」分开数；② 反过来写这类句子时，把否定的**范围**写窄（「决定不能进循环」而不是「这件事不能做」）——宽一格的措辞会被后来者当成结论引用，而它没有测试守着
- **一个每轮只解析一次的值，要能中途改变，就得有个句柄——重新解析是第二个真源** —— 上一条的实施面。档位在 `resolve_turn_permissions` 里每轮解一次并交给这一轮的 `ScopedToolService`；让批准立刻生效的诱惑是「在闸点重新解析一次」，那要读一次 session store（热路径）**并且**制造两个「这一轮是什么档」的答案，并发写入时必然分歧。正解是解析处铸一个句柄（`PlanGate` = `AtomicBool` + restore 档）挂到**两个 service 都已经在传的那个上下文**（`TurnContext`）上，闸点读它、工具翻它。⚠️ 配套：**这个句柄的 restore 值要用推导不要用记忆**——存一个 `*_pre_plan` 键就等于要求**每一个**写原值的路径都记得同时写它（这里是两个：pill 的 `sessions.patch` 与请求携带的 stamp），正是「收敛写者时要数一遍写者」收过三轮账的形状
- **能不能按归属裁决，取决于生产者有没有把归属留下来** —— 一个帧被判成 `Global` + 角色闸，常常不是因为它真是舰队级的，而是因为**派生它的那个函数把 `session_key` 丢了**（`r5_router::approval_for` 从 `ApprovalRequested` 只抄了 `approval_id`）。症状读起来像"这类东西本来就没有归属"，而代价是**归属最明确的那个人收不到**。判据：写下"这个帧没有可用来 scope 的字段"之前，先 grep 它是从谁派生出来的，看那一步丢了什么 → §5.22
- **两级缩减串联时，第二级看到的「尾巴」是第一级的尾巴，不是数据的尾巴** —— 而第二级的 doc 通常正是拿「保住结尾那个真正的错误」来论证自己存在的。判据：**这个缓冲是谁给我的，它完整吗**？把 `(kept, dropped)` 交给缩减器，它就分不出「连续」和「中间有洞」，于是补的 marker 报出一个不完整的数——传 `{head, tail, total}` 让这件事在类型上说得出口，计数按 total 而不是按幸存部分。⚠️ 同一句承诺照例有三份拷贝，**最贵的那份在工具 `DESCRIPTION` 里**（这里逐字写着 "we keep both the head and the tail"，而 >8 MiB 的 build 从来没拿到过尾巴）→ §3.15⑪A
- **一个字段没有渲染者，未必是没人接线——先问它挂在哪一行上，以及那一行的工作是什么** —— §9 那条「指不出渲染它的那一行代码就是 CUT」讲的是**处置**，这条讲**诊断**，而两者的答案可能相反。`CatalogEntry.capabilities` / `.cost` 定义清晰、服务端填得好好的、客户端 DTO 也声明了，三轮下来仍然零渲染：因为它们挂在 **provider 行**上、描述的是 `default_model`，而那一行的全部工作是让你挑**另一个**模型 —— 唯一诚实的标签是「你没有在选的那个模型的窗口」，所以每个渲染器作者都单独地、正确地决定了不画它。**判据：一个人人都不用的字段，先看它的主语和那张表的主语是不是同一个**；不是就下沉到正确的行（这里是 `RosterModel`，逐 id 经同一个 join 点解析），而不是 CUT 掉一份真正有用的数据、也不是硬接一个会说谎的标签。**症状特征是「三个面都没接」而不是「有一个面漏了」**——一个面漏接是疏忽，全都没接是那份数据回答错了问题。
- **投影成 `Vec<String>` 的那一刻，它旁边那个可选字段整类消失了——而丢弃发生在渲染之外，所以每个渲染器看起来都对** —— `ClarificationOption` 有 `description`，频道渲染 `label — description`，而帧与 RPC 都把它投影成标签数组 ⇒ Panel 与 TUI 各自渲染一个裸 label，三张脸没有一张有 bug、字段就是到不了。判据：**把一个结构体压成标量数组之前，数一遍它有几个字段、以及少掉的那些谁还需要**；答案通常是「渲染器需要，而它没有别的来源」。同族是 §5 的「『保真』只在你列举过的字段上成立」，只是这次列举法藏在类型里而不是白名单里
- **为兼容而留的旧表述是投影不是副本：它必须和新表述在同一个函数里派生，而且它描述的是游标处那一个** —— 上一条的第二半。一条 wire 长出更丰富的表述时，旧的扁平字段要留给老客户端，于是同一事实有了两份——**两份表述只要有两个作者就会漂**（§0 已有那条讲的是「只改一份」，这条讲的是「一开始就别给它第二个作者」）。⚠️ 还有一个更细的错法：投影**指向第 0 个**看起来天经地义，但顺序作答的客户端渲染的是它**即将回答**的那一问，指向第 0 个会让第一次之后的每次回复都答在屏幕上的错问题上——而两边都返回了「成功」。单一源 `clarification::session::ask_user_frame`，守卫 `the_frames_legacy_projection_is_the_question_at_the_cursor`
- **「同一事实的两份表述」的第三形态：不是两份漂了，也不是一份被删了，而是其中一份从出生起就是另一份的削弱版——削掉的恰好是它独有输入才需要的那部分** —— 前两种形态都靠 grep 名字找得到，这一种找不到：**两份代码都在、都被调用、都有测试**。成因永远是同一句话——"这里再跑一遍完整管线太重了，这一步只需要 X"。`normalize_for_matching` 对外层命令做「剥不可见符 → 规范化 Windows 路径前缀 → 出多份视图」，而解码出来的 `-EncodedCommand` 载荷走的是手写的够用版（只折转义），于是载荷**恰好**保留了够用版不知道的那两件事：`\\?\C:\` 与零宽符——`powershell -enc <b64>` 因此把灾难铁底降成一句 warn，而两条路径各自的测试全绿。判据两句：① **一份数据被"再喂进"同一层时，它必须走那一层的入口函数（递归回真源），不许走一段够用的复制**；② 反过来，看到一个"简化版处理"时，问的不是"它够不够用"，而是**"它少掉的那几步，正好是谁的输入需要的"**——答案通常是"只有走这条路的输入需要"，那正是它被简化掉的原因
- **「同一事实的两份表述」的第四形态：其中一份是**对另一份的引用**，而引用一旦写下，被引用的那一方就从「可以改」变成了「不许改」——却没有人通知它** —— 前三种形态的两份表述各自描述同一个事实，改错了至少 grep 得到；这一种的第二份**不描述事实，它描述的是另一个子系统的行为**，于是改动被引用方是一次完全正常、完全正确的改动，而它悄悄抽掉了引用方的地基。`mobile/ios` 的 `PairingTarget` 逐字写着「Parsing mirrors the desktop lite shell's `ConnectionTarget::parse`, **and that is a promise, not a remark**」，按名字引用 `connection::default_port_for` / `gateway_probe::{target_origin, unreachable_message}` 三个符号，并**各配了一条测试钉住**——而 `desktop/shell/` 那一侧**零处提到 iOS**。判据三句：① 写下「这里与 X 保持一致」时，**去 X 那边留一条反向指针**（一句 doc 就够——它是唯一会被改 X 的人读到的东西）；② 引用方那条测试**钉的是被引用方的行为**，所以它红的时候点名的是错的文件，而它绿只证明引用方没变；③ ⚠️ **最贵的是两半由不同工具链验证时**——这里 `cargo` 与 `xcodebuild` 没有任何一条命令同时看得见，所以「改掉 Rust 侧的默认端口规则」在 CI 上是全绿的，而它已经让手机端的地址栏解析和桌面端分家了。
- **凡「动词 + 参数列表」的规则，先问它锚在第几个参数上** —— 一条读起来完整的规则可以只覆盖参数表的**第一位**，而多参数是那个动词的正常用法而非混淆手法：`rm -rf ./build /` 删掉整盘，两条递归删除规则却都把危险目标锚在第一个操作数上，读起来干干净净。跨越中间参数时**跨越的边界要按「词」定而不是按字符**——token 首字符不许是注释符、token 内不许含语句分隔符，否则 `rm -rf ./out && ls /` 会被缝成一条罪状（那是 `seg!()` 存在的理由，而 `seg!()` 在这里用不了，因为它按字符跨） `canonicalize` 在 Windows 返回 `\\?\C:\…`，对 API 层是对的、对人是错的。修法是出线转换（`utils::paths::display_string`），**不是**把存储也换掉：那个转换**是部分的**（超过 `MAX_PATH` 与 UNC 保留前缀），所以两边各转一次会让 `starts_with` 从**放行翻成拒绝**——`allowed_roots` 作用域检查正是这个形状 → §5.22
- **同一段配置被解析两次时，第二个解析点未必拿到和第一个相同的输入——而少掉的那个字段如果是 required，它就整段失败** —— 密钥迁移（`CHANNEL_SECRET_FIELDS`）把 `bot_token` / `app_secret` 搬进 vault 并**从 `config.toml` 里摘掉**，构造路径因此先 `inject_channel_secrets` 再解析；而只读策略字段的 gating 路径照着原始 block 解析，于是在**任何存过一次通道的部署上**必然 `Err`，打一行 warn，然后把 router 静默退回 `ChannelConfig::default()` —— **正是那条桥接存在的理由**。telegram 的桥自密钥迁移落地那天起就是死的，零报错、零红测（fixture 都手写完整 config，从不经过迁移）。**同一天在同一份配置上抓到三个解析点，三个失效方式各不相同**：gating arm 打 warn 然后退回默认（telegram + feishu），而 `inbound_router::executor::try_create_feishu_emitter` 的 `.ok()?` **一个字都不说**——它直接返回 `None`，于是整个 `FeishuEventEmitter`（流式卡片 + typing indicator）在每一个存过通道的部署上不可达，而回复照常从普通路径发出去，**症状是零**。判据三句：① **grep 出这份数据有几个解析点，再问每个点拿到的是不是同一个 `Value`**；② 共用一个取值函数而不是在每个点抄一遍 hydration（现 `subsystems.rs::gating_config`）；③ **消费者根本不该去重建一份所有者已经握着的东西**——executor 没有 vault 句柄，也不该为了回答 channel 早就回答过的问题去要一个；正解是让所有者把它**发布**出来（`feishu::api_handle` 现在同时发 `Arc<FeishuApi>` 与那份已水化的 `FeishuConfig`），顺带更正确：emitter 该按**正在跑的**那份配置行事，不是按此后被人编辑过的文件。⚠️ **发现方式值得记**：我为「共享 client」写的断言，变异之后**没有变红**——而正确反应不是「守卫瞎了」，是**先怀疑自己的判断**：那条路径在更早一步就 `None` 了，所以变异根本到不了我以为它在的地方。发现路径：`qa/channels/run.sh` 报 `Permission denied: Mention required in group`，而配置里写着 `require_mention = false`
- **一个字段被表单收集、被前置过滤器尊重、被真正裁决的那一层忽略——三件事都"有代码"** —— 所以读任何一层都看不出问题。Feishu 卡片提供 `dm_allowed` / `groups_allowed` / `group_policy` / `group_allowlist` / `require_mention` 五个字段，channel 自己的 `InboundPolicy` 认三个，而 `inbound_router::check_permission` 一个都收不到（只有 imessage / telegram 有 `From<&*Config> for ChannelConfig` 桥）。判据：**指出这个字段的裁决者是谁，再问它读得到吗**；答案是"读不到"时，桥接的每条臂都要问**这条臂是放宽还是收窄**——`dm_allowed` 是个布尔，说不出"谁"，所以 `true` 只能保持今天的 `Pairing`，读成 `Open` 就是把每一个存量部署静默打开。⚠️ **这一条是被本轮自己造出来的**：feishu 在 2026-08-18 之前配不出来，所以那个休眠的缺口不承重；**把一个东西变成可配置的，会让它周围每一个休眠的缺口当场承重**。⚠️ **而这条判据的判定方法是问「路是谁铺的」，不是问「代码多老」**——两者会给出相反的答案：`BusyInputMode::for_shared_room` 那段确实早于本轮（所以一次复审据此判它 out of scope），而 `git log main -S"fn bind_conversation"` 是**空的**——开那扇门的是本轮。**代码是既存的，承重是这一轮加上去的**：「这个缺陷是既存的」不等于「这个暴露是既存的」，而只有后者决定它归不归你修
- **serde 不逐字段降级：一个字段的类型太窄，整份文档就解析不出来** —— 而症状是「这个东西整个不见了」，不是「少了一个字段」。同一天在两处：CC manifest 的五个组件字段是 `Option<String>`，而 Claude Code 允许 path | 数组 | 内联对象（Anthropic 自己有两个 manifest 用内联 `mcpServers`）⇒ 插件拿到 `PluginStatus::Error` + **零** capability；marketplace 的 `source` 是裸 `String` 而上游是六元联合 ⇒ **一条 `{source:"github"}` 让整个市场的所有插件不可见**。判据三句：① 对一个**别人定义**的格式，问的不是「这个字段是什么类型」而是**「上游允许几种形状」**；② **只放宽类型会更糟**——那是把响亮的拒绝换成静默的零能力加载，内联分支必须同批拿到真消费者，没有消费者的那条臂要**点名 warn 后跳过**而不是猜；③ 这类守卫要**自带证伪**：把同一份 fixture 反序列化进它取代的那个形状并断言报错。一次性变异证明不了三个月后「用 String 也行吧」那次简化，而那正是下一个读者会做的事
- **一个字段对某个成员是哨兵而不是数据时，每一处解释这个字段的代码都要认得那个哨兵——而它们通常不止一处，只有一处会被记得** —— 内置 marketplace 的 `source` 是字面量 `"bundled"`（内容由 `bundled::extractor` 从二进制解到 `<cache>/aleph-official`），`update()` 认得它、`resolve_cache_dir()` 不认，于是**每一次查找**把这个哨兵交给本地路径解析器、按进程 cwd 解、解不到、跳过 ⇒ **出厂就有的那个 marketplace 对所有查找不可读**，随包插件的整条按名安装路径因此从未成功过（`plugin.install{source:"<官方名>"}` → `search_plugin` → 0 结果 → "not found, try marketplace update first"，而 update 写的正是查找从不读的那个目录；hub 的 `"local"` 源形式同乘这条路）。判据三句：① 哨兵值要有**一个所有者**，别让 N 个解释者各自记得；② 收敛是让第二处**调用**第一处（现 `update` 调 `resolve_cache_dir`），不是在第二处再抄一遍分支；③ ⚠️ **这个缺陷的隐身衣是「跳过读不出的 cache」这件对的事**——同一个 `continue` 对**查找**正确（一个坏 marketplace 不该让整次查找失败），对**目录**是谎（空列表同时意味着「没匹配」「没同步」「不是 marketplace 仓」「manifest 坏了」）。所以那趟遍历要把 problems **交出来**让每个调用者自己裁决，而不是在遍历里替两类调用者一起裁决
- **一条守卫如果自己列举它要比对的"事实"，它对没被列举的那一族结构性失明——而那一族里往往正好有一个在重复** —— 「列举法只覆盖立法当天的世界」落在**守卫的输入**上，而不是它的规则上。`no_environment_fact_is_stated_twice` 手写了 6 个 `RuntimeContext` 事实、**零个 sandbox 事实**，于是 `Network:` 被 `SecurityLayer`（Stable）和 `OperatingEnvelopeLayer`（Dynamic）**同回合、同对象、同字段**渲染两遍，守卫连着四轮报绿。判据两句：① 事实清单要**从拥有事实的那个类型派生**，且用**穷尽解构**（`let Self { a, b, c } = self;`）实现——加字段时是**编译错误**，逼字段的所有者当场回答「这是不是模型可见的」；② 派生出来的清单要有**自保断言**（"这一轮到底比对了几条"），否则清单缩水与守卫失效在报告里长得一模一样。⚠️ 配套：这类守卫按"值"匹配，所以**过短的值必须跳过**，而跳过的判据要是**每次现算的字符串性质**（长度阈值），不是一张会腐烂成许可证的事实白名单
- **数一个通道有几个生产者时，问的不是"谁调它"，是"能调它的那个类型有几个构造点、那些构造点碰得到这件事吗"** —— 一条通道可以两端完整、两端有测试、而中间那根线**结构上不可能存在**：`TurnEnvelope.{parent, run_id}` 是"子代理专用"字段，而 `TurnEnvelope` 全仓只有一个生产构造点（网关回合），子代理**根本不走那条路**（它直接驱动 `AgentHarness::run`，不建 `FlowRequest`）。dead-code 分析看不见这一类——生产者不算死（它有个写者字段），消费者不算死（它有个渲染器）。而**文档会替它作证**：上一轮记录逐字写着「子代理派发器在新建 FlowRequest 时填两者」，那句话描述的是一个不存在的调用点。判据：读到"由 X 派发器填充"时，去 grep **X 建不建那个结构体**
- **一处 post-pipeline 手焊，是一条断线留下的疤** —— 凡看到「因为这条路径没有 X 可读，所以我在这里手动补上」这种注释，别把它读成风格选择，去问**为什么这条路径没有 X**。`build_system_prompt` 为 `<strategy>` 与会话模式各留了一处手焊，两处注释都写着「Basic 路径不穿 `ResolvedContext`」——而真正的后果不是这两个事实要手写，是**其余十几个读 `ResolvedContext` 的层在子代理提示词里全体沉默**（cwd / 模型 / 分支 / 可写根 / 网络 / sandbox 姿态一个都没有）。手焊只补了作者当时正需要的那两个，缺席的那一大片没有人会去数
- **「我手上没有类型化句柄，所以只能沉默」——先数一遍选项有几个：重算一遍是第二个推导，问那个执法者不是** —— 子代理提示词四轮不说审批档位，理由写得很扎实：档位对孩子**是被强制的**（它跑在父的 `ScopedToolService` 上，连同同一个 `PlanGate` `Arc`），而「这一轮是什么档」的**第二个推导**可能与真正执法的那道闸分歧，所以沉默更安全。两句都对，只是选项被数成了两个：**能回答这一问的对象本来就在调用者手里**（`SpawnerBase.parent_tools` 就是那道闸），缺的只是一个问它的方法。判据两句：① 读到「这条路径没有 X 的句柄」时，先去看**执行 X 的那个对象在不在作用域里**——在的话，让它交出**它自己裁决时调的那一个方法**（`ScopedToolService::enforced_exec_tier` ≡ `effective_exec_tier`），那是同一个推导的第二个读者，不是第二个推导；② 别改成把值当参数穿下来——**快照会过期**，而这一族事实（档位 / 权限 / 预算）恰恰可以被人中途翻掉（人批准计划 ⇒ 闸当场翻档，而孩子的提示词还在描述翻档前的世界）。⚠️ 沉默的代价不是均匀的：`Plan` 是唯一**拒绝**而非询问的档，`subagent` 又被刻意放行（`PLAN_REACHABLE_TOOLS`），所以一个不被告知的孩子会把整个迭代预算花在用一次次被拒来发现一句话就能说清的事
- **一个「必须跨 spawn 携带什么」的单一源，成员资格的判据是失效方向，不是这个类型的名字** —— `CarriedAttribution` 叫 attribution，而 `EXEC_WORKSPACE` 是**执行授权**，于是它在门口被挡了一整轮；但那个类型的第一句 doc 写的是「the task-locals a spawned unit of work must carry」——**契约比名字宽**。丢掉它的失效方向是前五个载荷都没有的第四种：不是静默、不是敞开、不是自信地错，是**死**——`jail_root_for` 回落到 `workspaces/<hash(全新 nonce)>`，一个首次使用才创建的**空目录**，于是孩子跑的每条命令都在一棵什么都没有的树里，它发往真实项目的每个绝对路径被 `cwd outside workspace root` 拒掉。**前台子代理一直是对的**（它不跨 spawn），所以同一次委派工作不工作取决于调用方传没传 `background`——这类缺陷最难被报上来的形状。判据：往这种集合加成员时问**丢了会怎样**，别问**它算不算这个名字**；而这件事之所以便宜，是因为守卫早就从「点名三个组合子」改成了「必须走 `.reestablish(`」——**规则而非名单**，四个 spawn 站点一个都不用改
- **真源必须在被依赖的一侧** —— 依赖方向 `linux → shared` ⇒ 真源在 `shared/`；反着放就是把同一个问题回答 N 次
- **契约的两半住在两个 crate 里时，"有测试"这件事本身会骗人** —— `aleph workspace create|archive` 自写下之日起每次调用都 `INVALID_PARAMS`（CLI 发 `{"name"}`，handler 要 `id`），而 CLI 那侧的测试断言的是 `json!({"name":…})["name"] == "test-ws"`：**一个只读自己刚写下的字面量的断言，测的是 serde_json 不是你的代码**，它永远绿。判据：跨 crate 的 wire 契约要么**共用一个类型**（重命名 ⇒ 编译错；单一源 `aleph_protocol::workspace`，`aleph-cli` 按设计不许依赖 `alephcore`），要么在**依赖两边的那一侧**留一条真正对账的测试（`workspace.rs::every_column_the_cli_renders_is_present_in_the_list_response`，用改 wire key 的变异证过 RED）。**同族：展示列也要对账**——那张表读的 `status`/`created` 服务端从来没发过（真名 `is_archived`/`created_at`），于是每行印一列破折号，看起来只是"还没有值"
- **⚠️ 第二次复发（2026-08-11，`aleph-tui`），而且这次它躲在一句自称保证的话后面** —— `send_to_agent` 是 TUI 唯一的 `agent.run` 发送点，doc 逐字写着 "the request shape can never drift across the four paths"，而它发的是 `"message"`（`chat.send` 的键），`agent.run` 要 `input`。**一个"唯一发送点"保证的是四条路径彼此一致，不是它们对**——形状在那一个点上错，四条就一起错，而收敛本身让人以为这一族已经被看过了。同一轮的第二个：TUI 自造的会话键 `chat-<uuid8>` 过不了 `SessionKey::parse`，而 `AgentRouter::route` 对解析失败**不报错、另起 epoch**，于是客户端握着一个服务端从没听说过的键去调六个 RPC。判据补两句：① **"我们已经把它收敛到一个点了"不是正确性证据**，去问那一个点的形状是从哪来的——手写字面量就是没有对手；② **一个"宽容"的解析入口（fall-through 而非报错）会把客户端的错误变成服务端的静默分叉**，凡按 id/键寻址的入口，解析失败该是拒绝还是另起一个新对象，必须是被写下来的决定。单一源 `aleph_protocol::session_thread`，两个方向各一条对账（请求侧 deserialize、响应侧**键集相等且期望从契约类型派生**）→ FEATURE_LOCATOR §5.23
- **⚠️ 第三次复发（2026-08-13，`aleph-cli`），这次两个方向同时错，而且没有一条测试能看见** —— `aleph providers list` 渲染 `type`/`default` 两列，服务端只发过 `provider_type`/`is_default` ⇒ 两列自写下之日起每行都是破折号；`providers get` 更彻底，它读顶层而不是 `provider` 信封 ⇒ **每一行**都是破折号；`providers add`/`test` 发扁平 `{name,type,api_key,base_url}` 而 handler 要 `{name, config:{…}}` ⇒ **每一次调用都是 `INVALID_PARAMS`**，从来没成功过。三个都在同一个文件里，都不是"最近改坏的"。判据补两句：① **一个「展示用」的列和一个请求体是同一类东西**——前者错了看起来像"这个字段还没有值"，后者错了看起来像"这个命令还没做完"，两种都不像 bug，所以都不会被报上来；② **信封也是 wire key，而且它通常是最后一个没被类型化的部分**：行做成契约类型之后，包着它的那个 `{"items": …}` 还留在三个客户端各写一遍——收敛成 `CatalogResult` / `ProviderListResult` / `ProviderGetResult`，成本一行，收益是把"最后一处手抄"这个位置整个消灭掉。单一源 `aleph_protocol::providers`。
- **形状太简单的东西会被复制而不是共用，而没人写的那份拷贝是看不见的** —— `exec_tier` / `session_mode` / `think_level` 是同一套三孪生：前两个各自被加进 `sessions.list` 的解码、`sessions.patch` 的校验、Panel 的 pill，第三个**一处都没有**——它自写下之日起就被 `turn_thinking` 持久化并每轮强制，只是没有任何客户端面读得到，所以junk 值存得进去、每轮被 warn 掉、而"一个没人读的 knob"和"一个没人设的 knob"长得一模一样。成因不是偷懒：`im.custom.get(k)?.as_str()` 太短了，短到共用它显得小题大做，于是它被逐 knob 抄写，而第三次抄写没有发生。判据：**一族同构的解码/校验/渲染，超过两个就收敛成一张表 + 一条从源码派生名单的 census**（`session_snapshot.rs` ↔ `modify.rs::every_session_knob_is_validated_on_patch`），别数"我加了几个"——数"这一族总共有几个" → §5.23
- **"服务端已经持久化了"回答不了"客户端拿得到吗"，而 attach 面是最容易漏的那一个** —— 一个对话的模式 / 档位 / 推理档 / 累计 token 全都有耐久的家，且每轮都在被强制；缺的是**没有任何响应把它们交给按键重新连上来的客户端**。于是重开的终端在一个服务端仍按自己的值治理的对话上，画出了**装机默认值**。判据一句话：**这个事实有持久行吗？有的话，客户端 attach 时读的是哪一个响应？** 答不上就是它还没有读者。修法是挂到客户端 attach 时本就要发的那个响应上（`chat.history` 的 `active_run` / `plan` 是同一个论证的先例——它们是**一个**快照，分两次调用就开出一个"拿着 transcript 却拿着另一份设置"的窗口），不是新造一个 `sessions.get`（第二个存在性 oracle）→ §5.23
- **解析只能证明"超集"，永远证不出"相等"——而超发就住在这个缝里** —— 上一条的第二半，同一个 wire 上第二次犯。那条对账测试（`every_column_the_cli_renders_is_present_in_the_..._response`）把真实响应 parse 成契约类型再断言字段都在，**方向只有一个**：serde 默认忽略未知键，所以它对"响应里还有什么"结构性失明。`workspace.get` 因此把整个 `AgentEnv` 发上线，多出 `env_vars`/`allowed_tools`/`system_prompt_override`/`default_model` 四个字段——**全仓既无写入者也无读取者**（`ActiveAgentEnv` 自称流经整条执行流水线，却在解析边界把它们全丢了），即一个"看起来可设置、设了永远没反应"的配置面。判据两句：① **契约类型要用来"构造"响应，不只是用来"解析"响应**——从契约类型 build，超发就成了编译期不可能，而不是一条要记得写的断言；② 断言要写成**键集相等**且**期望值从契约类型自身派生**（序列化一份取 keys），写字面量清单就是同一个列举法错误挪高一层。反向的"未知字段容忍"是另一条独立性质，归协议 crate，别塞进对账夹具里——夹具一旦混进服务端已经不发的键，它就从"真实响应"变成了最后一处谎言的藏身地
- **一个子系统的负半边有出口，正半边没有，这个不对称本身就是缺陷** —— 拒绝侧建了熔断器 / 冷却 / 半开探测（因为"永久拒绝"显然坏），而**授权侧的"永久允许"连列表都没有**：用户点完「本会话允许」既看不到也收不回。判据一句话：**这个决定有没有反悔的路**？两个方向要分别问一遍——通常只有让人不舒服的那个方向被想到了。⚠️ 配套：可枚举性是可撤销性的前提，而**列表里只有指纹就等于没有列表**（人分不出哪条是要删的那条），所以授权记录必须存下**当时给人看的那句话** → §5.12。⚠️ **第二例（2026-08-23，重连修复）方向一样、成因不同：负半边写完之后，正半边根本没有人想起来它存在**。重连时「结算服务端不确认的 route」有出口（composer 卡在 Stop 是显然坏的），而「服务端确认、本端却没有 route」——core 重启后 `resume_coordinator` 用**新 run id** 重新触发的那一轮——**一个字都没有**：红点亮着、服务端在流、transcript 一动不动直到那轮结束，且 `run.session_updated` 的重新水化对**正在跑**的会话是刻意抑制的，救不了。判据补一句：**把一个判断写成谓词之后，把它取反念一遍**（「服务端不认的 route」↔「本端不认的 run」），再问那一半有没有代码

- **fail-soft 的跳过不是「不存在」的证据** —— `Ok(None)` 给读者防卡死用，拿它当 DELETE / 放行判据就是不可逆损坏；**按状态做的闸，`Err` 必须是拒绝不能是放行**
- **一次 `write` 不是一次事件，所以两个写者能拼出一份谁也没写过的文件** —— `fs::write` ＝ `create+truncate+write_all` 三步，**truncate 与 write 之间可以插进另一个写者**：B 先 truncate、A 落笔 567 字节、B 再写 509 字节 ⇒ 文件是 B 的完整文档 + A 的尾巴。判据不是「这里会不会并发」（`session_store` 的 `metadata.json` 有 **16 个**无锁读-改-写调用点，答案永远是会），而是**这份文件被撕开之后，读它的人会得到什么**。这里的答案是最坏的一种：`list_sessions` 对解析失败静默 `continue`、`read_metadata` 报错 ⇒ **同一个会话在每个面上同时"不存在"**（列表里没有 / `chat.history` 答 not-found / patch 拒绝），而转录完好地躺在旁边，且**因为是磁盘损坏所以重启不治**——用户唯一会试的那一招正好无效。修法是既有单一源 `utils::atomic_write::atomic_write_file`（同目录 temp + fsync + rename），它保证的是**幸存者是一份完整文档**，**不是**「没人丢更新」——后者要跨读-改-写持锁，是另一件事，别把两者说成一件。**两件都做完了，做法值得抄**：锁那半不是「记得先 lock」的纪律，而是**模块边界**——写函数私有在 `file_backend/meta.rs` 里，父模块够不到，唯一的写入口 `MetaGuard` 只能由「先取锁、再读文档」的 `MetaLocks::lock` 产出，于是「读-改-写是一个临界区」按构造成立（源码级守卫只认得它被教过的那几种形状，这里不需要教）。⚠️ **创建路径同属读-改-写**（「不存在 ⇒ 创建」），别把它漏在锁外 → §5.23b
- **「本地观测不到」通常是某一条协议的性质，不是那个事实的性质** —— 同一个 knob 常有第二个载体，而那个载体的门是另一把。推理档在 OpenAI 协议上由 `EndpointClass` 决定去留，而 `Local`（127.0.0.1）与 `Custom`（任何自建 host）**两个都**是 `supports_reasoning_effort: false` ⇒ 没有任何本地 host 能观测它，于是它被记成「rig 结构性观测不到」。Anthropic 协议不看 endpoint class，按**模型名**决定 thinking 块 ⇒ 一个本地 Anthropic mock + 一个 pre-4.6 的 Claude 模型 id，整段就在 wire 上（`budget_tokens` 随档位 1024/4096/10000/20000/50000）。判据：写下「这个环境测不了」之前，先问**换一条协议/载体还测不测得了** → §5.23b round-3
- **一个命令族如果只有一个入口，那个入口丢掉参数就等于整族只能被无参调用——而无参行为往往长得像功能** —— TUI 的 `/` 在空 composer 上打开命令面板而不是键入斜杠，所以面板是唯一入口；面板确认执行的是条目的 `full_command`（裸 `/think`），输入里跟在后面的字一个不带。于是四个会话 knob + `/tools` + `/compress [instructions]` 从来没有人设成功过，而它们的无参回应是「打印当前值 + 用法」——**读起来像功能不像 bug**，所以这一族被标成「已交付」。判据：**说一个命令面已交付之前，问它带参数那条路谁跑过**。⚠️ 配套：修好之后第一次跑起来的路径要重审——过滤器同时匹配名字**和描述**、确认执行 `selected=0`，「mode」因此选中描述里含 "mode" 的 `/tools`；无参时只是烦人，带参时会把参数交给另一个命令（现 `command_tree::filter_rank`：精确名 > 前缀 > 仅描述命中）
- **测试开关不能是环境变量** —— `std::env` 是进程全局的，libtest 并行跑：三条测试从来没设过那个豁免（恒红），而第四条为了验证生产 accept-set 会把它**删掉**——即便补上，两者也会随机互相打架。用 thread-local + RAII guard（`hub::install::AllowLocalGitUrl`），兄弟测试够不到，`--test-threads=1` 下也会在 drop 时复位。⚠️ **推论（2026-08-21）：当那个环境变量已经存在且没法 thread-local 化时（`ALEPH_HOME` 是产品面的旋钮，不是测试开关），「一部分测试隔离」比「全都不隔离」更糟，不是更好。** 隔离的那个把 `ALEPH_HOME` 改成一棵**它自己的 guard 会删掉的**临时树，于是没隔离的兄弟不是写进真实 home，而是写进一个它不知道存在、随时会被 drop 掉的目录——单跑全绿、全量偶红。四条 pdf 测试正是这样：它们的输出路径被 `resolve_output_path` 重写到工作区目录，共用**一个固定路径**、各 23 MB、并行跑，而只有一条隔离。判据两句：① 看到「某几条测试加了隔离」时，去数**同一进程里还有几条读同一个变量**——答案不是零就得一起加；② 症状是**时间戳**：一次全量跑之后真实 home 里有的文件是今天、有的是两天前，那就是竞态本身在磁盘上留的记录。⚠️ **而这条判据一直没被应用，是因为守卫自己的义务句不问读者**——一条按「写者」措辞的契约，对**跨两次读做比较的读者**结构性失明：那个读者要的是值在**整个比较跨度**上稳定，不只是在某次写入期间稳定。`ALEPH_HOME_TEST_GUARD` 逐字如此（"Any test that **sets/removes** `ALEPH_HOME` MUST hold this guard"），而**它的理由那段是认读者的**（"so they don't **observe** each other's temporary directories"）——义务那句不是，于是一个只读的测试从不被告知自己在范围内。守卫本身正确、被所有写者遵守、**对自己一半的危险结构性失明**，这就是那个不对称：**被隔离的那一半是写者**。它是**读**出来的性质，不取决于任何测试红过 → 附录 C.5
- **JSON 表达不了 NaN/Infinity，所以用 `json!` 搭的 fixture 永远到不了非有限值的守卫** —— `serde_json::json!` 把它们变成 `null`，而 `null` 反序列化不进 `f64` ⇒ 那条测试在**自己的输入**上失败，从未到达它要检查的那道闸，且从写下之日起每次都红。非有限值要在解析**之后**装进去
- **「没有东西可解析」和「解析失败」是两个答案** —— 把后者写成前者会撞上前者的断言：工具审批卡没有命令行，生产路径给的是 `ok: true, segments: []`，而三处 fixture 各自写了 `CommandAnalysis::error(..)`，十一条测试因此从未通过。这类「空 ≠ 错」的空值要有名字（`CommandAnalysis::not_a_command()`），生产与 fixture 共用它
- **凡「跳过一条坏记录」的循环，都要问「跳过之后这个对象还有别的面能看见它吗」** —— 上一条的显示面。跳过本身通常是对的（一条坏记录不该让整张列表失败），**沉默才是贵的那一半**：一个 `Err(_) => continue` 让三个 surface 一致地报告"没有这个东西"，而没有任何一行日志说出真正发生的事。要么出声（点名文件/主键的 `warn!`），要么让它成为一个可枚举的错误项 → §5.23b
- **「被拒」不许读作「没有」** —— 上一条的显示面镜像（同族还有 §8 的「未知不许读作健康」）。一个 `Err` 被折成值（`Some(false)` / 空列表 / 空字符串）之后，UI 就在替服务器**发明一个它从未说过的答案**，而最贵的那种是**自信的假话**：引导清单把 admin 拒绝读成"没配置",于是对着一个配好的 provider 喊 `PENDING Configure a chat provider` 并邀请点进用不了的页。判据一句话：**只有 `Ok` 有资格断言被读的那个东西**；`Err` 的每一种（拒绝 / 断线 / 解析失败）都只能说"我不知道"。单一源 `interfaces/webchat/src/components/admin_refusal.rs`（识别咽喉，非权限判定——Panel 刻意不持有客户端角色谓词，见 `context.rs` 的 `role` 字段注释）
- **修好读半边，会让写半边更难被发现——因为那一页从此「看起来已经处理过了」** —— 上一条的第二半，同一个模块上第二次犯。读路径逐页接完 `settings_load_error` 之后，写路径**一处没接**：加载失败被礼貌解释、Save 却回一句裸协议串，**同一个判决对用户讲了两个故事**，而后者更贵（他以为自己操作失败了，不知道自己没权限）。⚠️ **别用「member 进不去那一页」自我安慰**：`settings::StepStatus::Restricted` 只给 Quick Setup 清单上色，设置页照常渲染、按钮是活的。判据：给一个错误分类器接线时先**数这个 surface 有几个方向会产生这个错误**（读/写至少两个），别只接你正看着的那个。
- **修「习惯写法」不等于修「那一类」——而中间那一步最像已经修完了** —— 上一条的同一轮里、我自己犯的第二个错，比原 bug 更值得记。报告上来是 `routing_rules.rs` **1 处**；按「同族」展开抓到 **48 处**，全是 `format!("Failed to …")`；真正的类是 **154 处**——因为 `set(Some(e))`（连框都不加，直接把协议串给用户）占了 45%，`format!("Delete failed: {e}")` 只是换了个动词。**48 那一版最危险**：它有守卫、有测试、全绿，而守卫钉住的是一种**词法**，于是给还坏着的一半发了合格证。判据三句：① 写守卫前先**分别数每种写法各有多少**（这里是 91 : 76），任何一种超过零就不能当作规则；② 判据要落在**这个值是不是服务器错误**，不是落在它被格式化成了什么样子；③ **纯文本判据碰得到真边界**——`ChatSendError` / `MicError` 这类**类型化**错误信号和 `Option<String>` 的那行逐字节一样，此时**例外必须是每次从源码重新推导的**（`receiver_holds_a_string` 查同文件声明），不能是一张页面 allowlist——前者不会腐烂成对某个 surface 的许可，后者会。单一源与 RED 证明在 `admin_refusal.rs::no_error_signal_is_fed_an_unclassified_error`（crate 级、零页面 allowlist；找不到声明一律**报**不一律**放**）。配套：包装在「永远不会被拒」的 surface 上是**逐字节 no-op**（非 refusal 原样透传），所以统一规则的代价是零，而 allowlist 的代价是第二个真源——同 `disposed_reads.rs` 拒绝例外的那条论证
- **词法守卫不会红着失败，它只是看不见——所以「它有 RED 测试」证明的是它认得的那几种形状，不是那一类** —— 上一条的第二轮，同一个模块、同一个错误**升了一层**：替代品同时用了**三个代理**（接收者名字含 `err` / payload 是 `Some(e)` / `format!` 里有 `{e}` 字面量），2026-08-10 发现**每个代理各自漏了一批**——`Some((false, e))` 元组（5 个 provider 测试面 + 绑定选择器，名字里没有 `err`）· `format!("{}{}", label, e)` 位置参数（`config_template.rs` 一家 6 处，全在 admin-gated 的 `channel.`/`config.` 上）· 连**类型例外**都是代理（`Option<String>` 精确匹配把 `Option<(bool, String)>` 判成"装不下 String"，于是那道本为 `MicError` 设的闸豁免了五个页面）。共 14 处，而守卫**为它认得的每一种形状都写了 RED 测试**、全绿。判据两句：① 别问「这个写法对不对」，问**这个值是从哪来的**（现规则锚 `Err(<ident>)` 绑定，`Err(_)` 因**没有值可分类**而在规则之外——那是另一类缺陷，另有 census 钉住不许增长）；② **代理会成群出现**（⚠️ **同族第三形态：不是没看见，是看见了别人的。** 「所有调 `check_browser_approval` 的工具都必须带着 policy 被构造」这条 census 第一版用**固定 400 字符窗口**从 `X::new(` 往后找 `.with_approval_policy`——而这些工具在构造器里**相邻声明**，于是删掉 `browser_emulate` 那一行之后窗口读进了 `browser_cookies` 的那一行，**照样绿**。判据：窗口式扫描要问**它的边界是谁定的**；应当止于被扫描单元自己的语法终点（这里是那条 `let … ;` 的分号），不是一个字符数。更普遍的一句：**一条没被证伪过的守卫不算守卫**——写完就手动破坏它一次，看它是不是红、以及红的时候有没有点名。**⚠️ 而破坏之后要数红了几条**：2026-08-14 我按「服务端两条发现路径都不读 `discoverable`、六个预设每次 sweep 都被拨号」这个判断改了代码、写了三条测试、并把它写进三处文档；`if false` 一破坏**只红了一条**，回头读最深那层才发现 `discovery::refresh_models` 从写下之日起就在拒，一次 HTTP 都没发生过。**红的条数比预期少时，先怀疑的是自己的判断而不是守卫**——上游少一个 `if` 与下游有一个 `if` 在症状上完全不同，而只有后者是可观测的；写下「X 从来没有被检查过」之前，去读那条路径的最深一层。）——发现一个词法代理时，把同一个判据里**每一个**"看起来像"的子句都数一遍，它们通常各漏一批。许可（licence）同理要从源码现推：本地包装器（`cluster.rs::fleet_error_label`）算已分类，而"任意本地函数调用"就是一张谁都能铸的许可证
- **一条词法守卫的语料是被预处理过的，而预处理可能正好删掉了你要找的那个东西——它还删得对** —— 上一条问「它认得几种拼法」，这条问**它读的是哪份文本**。`utils::source_scan::code_text` 按设计删除字符串字面量的**载荷**（否则守卫会命中自己的消息串），于是**用键的「值」拼写**的同一次裸读（`request.metadata.get("scope_id")`，而不是 `SCOPE_META_KEY`）对 `flow_scope_census` 的三道检查**全部隐形**——一次复审就靠它在「`cargo test run_loop::` 全绿」的构建上把真机夹具打回了和修复前逐字节同一个签名。**把值加进 `FORBIDDEN` 修不了**：扫描器在搜索之前就把它们删了。修法是**两个视图**——标识符走 `code_text`，值走 `code_keeping_literals`（留载荷、由词法器去掉**全部**注释，含跟在代码后面那种）且**带引号精确匹配**：不带引号的 `scope_id` 正是被保护那一行上的字段名，会在修好的代码上开火。判据：**写一条词法守卫时，先问它的语料是谁预处理的、预处理掉了什么。**⚠️ **配套一条 Rust 事实，因为「这条计数绕不绕得过」有三个不同的答案**：`as` 别名**挡不住裸的关联函数针**——Rust 没有 `use Type::assoc_fn`，`use X as Y` 只改类型名，固有关联函数自己的标识符原样活下来（实测：别名化的 `from_persisted` 仍命中 = 1）；挡得住**带类型名**那种针（`FlowScope::resolved(`）的才是别名（= 0）；挡得住**裸关联函数名**的是**结构体字面量**（= 0——`pub` 结构体 + `pub` 字段，一个构造子都不用点名）。三者逐条实测钉在 `flow_scope_census::tests::the_declined_from_persisted_count_would_have_caught_the_duplicate`，别把它们当成一件事答。
- **`Result<_, String>` 会替每一个消费面提前作出「服务器坏了」这个判决** —— 上两条的**服务端**镜像：那两条讲的是分类器接漏了一个面，这条讲的是**根本没有可接的分类**。错误类型装不下"是谁的错"，RPC 面就只剩一个选择——把调用方错误一起折成 `INTERNAL_ERROR`（＝去重试、去看服务器日志），而真相是**改一下请求就行**；工具面照旧把真话给模型，于是**同一动词的两张脸对同一次拒绝讲两个故事**。判据不是"错误信息够不够详细"（cron 的信息一直是对的，错的是 code），而是**这个类型表达得了分类吗**。修法：三分（not-found / 调用方可改 / 我方失败）+ 唯一映射咽喉（`gateway/handlers/task_error.rs::respond`，与 `projects::project_error_response` 同一份三分法）+ 源码级守卫禁止各面自写 `INTERNAL_ERROR`。⚠️ **别加 `From<String>`**：`?` 能自动转换的那一刻，下一个新增的调用方错误就默认变回 internal——原 bug 换成语言特性再来一次 → §4.13c 附录 A 第 18 条
- **「还没准备好」被答成「失败了」——这一族里唯一由我方自己伪造 `Err` 的那个** —— 前三条是把服务器的 `Err` 读成值；这条反过来：**传输层自己造了一个 `Err`，冒充服务器对这次调用的判决**。`rpc_call` 在 socket 未授权时不发帧、直接 `Err("Not connected")`，于是**每一处 mount 期加载都继承了这份混淆**：冷加载（直接 URL / 刷新——SPA 内跳转不算）必然输掉与 handshake 的竞速，页面渲染自己的初始值、打出「网关不可用」、**永不重试**。它 100% 复现，所以读起来像"这页没做完"而不像 race。判据三句：① **区分「我没问到」和「它答了不」，责任在造 `Err` 的那一层**，不在三十个调用点；② 先数**这个问题在仓里已经有几个答案**——本轮数到**五种**（跟踪 `is_connected` 的 `Effect` / 裸 `spawn_local` / 3×500 ms 重试 / 50×100 ms 轮询 / `should_fetch` 纯函数），一半是错的，**而它们各自都有测试**；③ 地板的旁路只能是**另一个函数**（`send_rpc`，给 handshake 用——它才是让 socket 授权的那次调用），写成 `method == "connect"` 的字符串比较是一次改名就会静默扩大的例外。⚠️ **闸不能建在「通道存在」上**：`connect()` 在 handshake **之前**就装好了 `rpc_tx`，按它放行等于把请求写进未授权窗口。单一源 `context.rs::gateway_readiness` / `await_gateway_ready`
- **一个源码级守卫的「绿」，只覆盖它的块识别器认得的那种块** —— `disposed_reads` 的规则是「`.await` 之后不许裸 `get_untracked()`」，它的 doc 明写**不容例外**（连 root-owned 信号都不放），但扫描器只走 `spawn_local(` 开的块。本轮新写的 `async fn await_gateway_ready` 正好落进盲区：**守卫全绿，规则被违反**——绿的是"我扫不到你"，不是"你合规"。判据：加守卫或往守卫下面加代码时，问的不是"这条规则对不对"，而是**它认得几种块**。此处扩到 `async fn` / `async move {` / `async {` 的**存量违规是 0**，所以又一次印证了 §0 那条「统一规则的代价是零，而 allowlist 的代价是第二个真源」——免费的时候就别留盲区
- **一个动词有 N 个面时，"谁能看"要在每个面用同一个推导** —— 别在 RPC 面写 `visible_owner_filter()`、在事件面写角色臂：**operator 的 `CALLER_USER` 是 `OWNER_USER_ID` 而不是 `None`**，所以 owner-keyed 谓词对 operator 也会生效。两面分歧的症状比"看不见"更怪——事件面放行、列表面过滤，而 Panel 每收一帧就按列表面重建 ⇒ **卡片到达后当场消失**。单一源 `caller_identity::caller_is_member`（＝ admin 闸自己的谓词）↔ `event_scope::is_superuser_scope`
- **隔离环境的 QA 结构上只测得到「新建的对象」** —— 干净 HOME 里没有存量，于是**迁移前写下的行**（缺列、缺戳、旧单位）整类测不到，而真实部署里那才是多数。补法是把已有行改成迁移前的形态再开机，**不是**让 fixture 造一个"看起来像旧的"状态 → §5.22
- **拒绝形状做得越好，时序 bug 越像安全行为** —— no-oracle 要求"拒绝"与"不存在"逐字节相同，代价是**异步写盘的行**在写盘前被读到时，给出的也是同一个 not-found。看到它先问「这一步是不是还没落盘」，再问「是不是没权限」 → §5.22
- **一个身份/谓词有两半，branch 一半等于没 branch** —— 一个动词的两张脸（工具 vs RPC）必须共用判据**也共用推导**（如 `workflow_step_review` ↔ `teams.workflow.approve_step` 共用 `verdict_admissible`）
- **先数这个能力有几张脸，再决定谓词放哪** —— 「两张脸」只是最常见的数目，不是上限。2026-08-08 一轮里同一个问法抓到四种不同的漏面：**一条连接有两个方向**（登录墙只挂在请求臂，事件臂四项判据无一是身份 ⇒ 未认证 socket 收得到 operator 的 shell）· **一个谓词有两种取 actor 的方式**（`CALLER_USER` 在 spawn 出的 run 里是死的 ⇒ 工具面照文档接现成谓词会拿到静默恒真）· **一个前缀下装着两类帧**（按 topic 前缀键控的表只能一次答完两者）· **一个能力有服务端和客户端两半**。→ §5.22 round 2
- **没有客户端的能力不算已交付，服务端那半再完整也不算** —— `users.create`/`users.update` 完整实现、注册两遍、admin-gated、pin 齐备、接了吊销管线，**全仓零调用者**：三期多用户机件因此在出厂形态下整体不可达。提交一个 RPC 家族前先问**谁调它**，答案必须指得出一个 shipped surface（同族：§5.21「一个"展示用"字段在提交前必须能指出渲染它的那一行代码」）
- **把人挡在门外不等于约束了他，可能只是把他推过了门** —— 审批面对 member 两面全关 ⇒ 他的 run 死在 120s 超时，而文档记录的解法是把档位拉到 `full`：最不安全的档位成了唯一能用的档位。**一个把用户往宽设置上推的权限系统已经反转了自己的目的。** 加闸时问「被闸住的人接下来会干什么」，不只问「这道闸拦住了什么」→ §5.22 round 2 ③。⚠️ **第二半：一道没有门把手的门不是闸，是墙——而它长得跟「安全默认」一模一样。** `ConfigApprovalPolicy::load()` 在策略文件**缺失**时也回落 all-Ask，`check_browser_approval` 把 `Ask` 变成一句拒绝**字符串**，而全仓没有任何代码写那个文件、这一层也没有卡可点 ⇒ 出厂装机上 15 个 browser 工具（含 `browser_open`/`browser_navigate` 两个入口）外加 desktop/pim/media/hooks **永久拒绝**，同一文件里那张文档化的 curated 默认表**零生产消费者**。判据两句：① 写下一个 fail-closed 默认值时，**说出谁能把它打开、从哪个界面打开**——答不上就不是 fail-closed，是 fail-dead；② 区分**缺失**与**损坏**：文件不存在是「用户还没表态」（该走产品默认），文件读不出/解不开才是「配置坏了」（才该收紧）。把两者写成同一个回落，就等于让未配置的装机继承为坏配置准备的姿态 → §3.12 第三轮 ①
- **一个 fail-closed 的答案被当成值消费，就会反转成许可** —— 装饰器对外人返回的 `Ok(None)` 被 `.ok().flatten()` 折进 `leader: Option<_>`，再读成「没有 leader ⇒ 谁都能审」。闸要跑在折叠**之前**：`Ok(None)`（拒绝）一旦和「这东西本来就没有」合流，两者永远分不开。凡 `.ok().flatten()` / `unwrap_or_default()` 落在**装饰器**返回值上都要问：这个默认值和那个拒绝长得一样吗 → §5.22 round 2 ⑤。⚠️ **第二例的反转方向更贵，而且它伪装成"更小的修法"**：让 `PairingStore::sender_user` 对停用用户返回 `None` 读起来是三行的收窄，实际上那个 `None` 不是"拒绝"而是**未绑定**，唯一的消费者把未绑定读作 legacy owner 语义（不盖戳 ⇒ run 被机主收养）——于是被闸住的成员**升格成机主**，拿到 operator 的 scope、记忆与会话。判据：**往一个已有含义的返回值里塞第二种含义之前，先去读消费者拿它当什么**；答案是"当默认值"时，收窄必须发生在别处（这里是**撤销那条凭据**，与吊销设备对称） → §5.22 round-4 ①
- **一个过滤器里的无条件豁免，会让任何想经它收窄的新规则对被豁免的那个成员静默失效——而它对别的成员照常生效，所以随手挑一个别的成员写的测试是绿的** —— 上一条讲「拒绝被折成值」，这条讲「规则根本没跑到」。`ScopedToolService::is_allowed` 对**挂载式** `SubagentTool` 无条件 `return true`（它不在 builtin registry 里，非空 allow-set 会把它整个藏起来，所以这条豁免本身是对的），于是「把 `subagent` 从 `allowed` 里摘掉」这种读起来最自然的收窄写法，对 `subagent` 恰恰是 **no-op**——而 no-op 与生效在测试里同形。`/btw` 侧问的只读天花板因此把两条 `PLAN_REACHABLE_TOOLS` 豁免撤在**真正裁决的那一层**（`permission_for` rung −1，读的还是同一个常量而不是第二张名单），不撤在 allow-set 上。判据两句：① **写一条收窄规则之前，先读它要经过的那道闸的每一条早返回**，问「我要收窄的那个东西，会不会在到达我的规则之前就被放行了」；② 这类豁免通常是为**可见性**（列不列给模型看）设的，而你的规则问的是**可执行性**——两个问题共用一道闸时，答错的永远是后来加的那个 → §4.14
- **一个人有几条凭据，撤销就得覆盖几条——而文档常常只记得第一条** —— principal 经**两条独立凭据**绑到系统（设备票 / channel sender 批准），停用只撤了设备那条，于是离职的成员照旧从 Telegram 说话、照旧读写自己的分区、并且**能调 `goal(action='update')` / `loop(action='resume')` 把同一个 handler 刚打上的冻结解开**。判据：写"停用/吊销"这个动词时，**grep 出所有能把流量认成这个人的表**（这里是 `devices.user_id` 与 `approved_senders.user_id`），别只数你正看着的那条。同族反面：绑定**生产者**也得数——三个（`pair --user` / `channel.pairing.approve` / `gateway.ticket.create`）里只有一个在问「这个 id 还活着吗」，另外两个把"停用"和"不存在"折成同一个 `(None, "guest")` 却只校验存在 → §5.22 round-4 ①②④
- **收敛到「单一源」不够——要收敛到**对面那一半实际在用的那个源**，否则你只是把 N 个错答案换成了一个** —— 上一条的第二天就复发，而且是我自己造的：把 `team::create` 的根从手搓 `dirs::home_dir()` 指向 `agent_resolver::default_agents_root()`，写者/读者分裂看起来修好了、测试全绿。但 `default_*_root()` 答的是**「没有配置时**agents 住哪」，而重启后真正重建这个 agent 的是 `resolve_agent_dir(agent, defaults)`——它读 `[agents.defaults] agents_root / workspace_root`。于是配了这两个键的装机上，provision 仍然写在 resolver 不看的地方，症状和修之前**一模一样**（成员重启后没有 SOUL.md、工作区是空的、两边都不报错）。判据两句：① 收敛前先问**「读这份数据的那一半，具体调的是哪个函数」**，把根收敛到**那个**函数上，不是收敛到它旁边那个名字很像的默认值；② 分辨这两者的问法是**「这个函数吃不吃配置」**——`default_*_root()` 无参，`resolve_agent_dir(agent, defaults)` 有参，**签名本身就在说它们答的不是同一问**。顺带：这类「已解析的值」通常**已经被作用域里某个对象拿着**了（这里是 `AgentManager.{agents_root, workspace_root}`，四个 provisioning 面全都握着它），所以正解是**读回来**而不是再算一遍——而喂错它的地方只有一个（boot），修一处即可 → §5.8
- **一张为了让守卫落地而设的豁免清单，是一张有期限的许可证——而它到期的方式是没人再去看它** —— 守卫报绿只证明「每个犯规都**登记过**」，不证明「没有犯规」。`no_hand_rolled_aleph_home_outside_the_allowlist` 从行级收紧到文件级时附了一张 16 项 `HOME_JOIN_PENDING_FIX`，doc 逐字写着「each really does hand-roll a home-rooted `.aleph` path」「**This list may only shrink**」——没有任何力让它缩，于是一条没少地出厂：三条是**活的写者/读者分裂**（`hooks.add` 往真实 home 写，而 `load_user_hooks` 读 `ALEPH_HOME`，**且后者的注释逐字在描述这个失效**；`skill_manage` 创建报成功却永不被热加载；`team::create` provision 的根不是重启后 resolver 去找的那个），两条是**热路径**（沙箱牢笼的锚、`AgentEnvStore::with_defaults` 开机即用）。判据两句：① 写下一条豁免清单的同一笔里，写下**会让它缩的那个力**——只减不增的断言 + 一条「豁免项若不再犯规就报错」的守卫 + 一个真正排空它的轮次；答不上「谁会让它变短」，登记就是归档。② **源码级守卫只证明拼写变了**；同一件事的行为要另一条运行时围栏（这里是隔离 `ALEPH_HOME`、断言九个状态根全部 `starts_with(get_config_dir())`），两条都要按「没被证伪过的守卫不算守卫」破坏一次 → §5.8
- **一条守卫如果按名字列举它要检查的成员，成员集合增长时它不会知道** —— 「列举法只覆盖立法当天的世界」在**守卫自己**身上的形态，而这一次它连着漏了两轮：`every_spawn_reestablishes_task_locals` 按字面量要求三个组合子名，`caller_role` 加入那组 task-local 时它没听说，房间作者加入时它也没听说，而 batch 腿**一路全绿**地把两个都丢了——因为它认得的那三个名字确实都在。判据：守卫要求的是**那个知道集合是什么的类型**（`CarriedAttribution::reestablish`），不是集合的成员；扩充集合于是成为对一个类型的改动，每个调用点白继承，而不是每条守卫每个站点都要被单独告知一遍 → §5.22 round-4 ⑨。⚠️ **第二形态（2026-08-21）：名单守得住「这几个还在」，守不住「多了一个」，也守不住「反方向的那个开始做了」——而后者往往才是把功能关掉的那个方向。** 做梦活动传感器的第一版 census 点名两个人类咽喉，于是第三个人类入口出现时它不会知道，cron 开始盖戳时它更不会知道（那会让 `idle_seconds()` 常年贴零、做梦此后再不运行，零报错）。修法不是补名字，是**把这一问挂到那个已经知道集合是什么的 census 上**——`RUN_REQUEST_PRODUCERS` 早就穷举了 `src/` 里每个起 run 的站点，加一列 `Ingress::Human { stamp_in } / Machine` 就让新成员白继承这一问，并让「除声明处外无人盖戳」这个**否定闭包**第一次表达得出来。两条推论：① `stamp_in` 要允许指向**别的文件**（判据发生在闸上、构造发生在下游是常态）；② 同一张表上不同的列可以有**不同的对称性**——`stamps` 只正向断言（获得归属是改进），ingress 两向都断言（机器盖戳是缺陷），把这个不对称写进 doc，否则下一个人会「顺手补齐」
- **第二个构造点默认继承不到第一个的任何档位** —— 加档位时 grep 它的 `::new(` 有几个调用点；**反向也要判断**，不是每个 builder 都该补齐，理由写进构造函数 doc → §2.19。⚠️ **数出来是七个就别再想构造点了**：`SecretMasker::new()` 有七个生产调用点，把配置线接到其中一个等于"一条腿打码、六条腿明文"，而且每条腿单测都绿。配置该接在**类型**上（进程级 `install_operator_patterns`），让不知道它存在的调用点也继承
- **上限 / 信号量 / 注册表的生命周期必须不短于它约束的那个东西** —— 判据一句话：**这个约束会不会比被它约束的对象先死**？`SubagentTool` 是 per-request 构造，而后台子代理是刻意活过 run 的 detached task ⇒ 第 N 轮的孩子握着 S_N 的 permit，第 N+1 轮拿到全新的 S_{N+1} **又是满额**，operator 配 4 而实际几十个同时打 provider。同一个错配在 `BackgroundAgentTracker` 上以**相反症状**发生过（per-request tracker "silently dropped every result once the spawning run returned"）——那次丢的是结果，这次丢的是约束。修法是把它键到真正的约束单元（会话）上并用 `Weak` 持有：permit 自带 `Arc`，所以"还在飞的孩子"天然把条目撑住，空闲即可回收 → §4.13c
- **一条只写在散文里的裁定，防不住下一个真诚的修复者** —— 「刻意不做」记录的用途是拦住**重提**，它拦不住**顺手修好**：本轮我读文档之前就已经把 §5.3 那条 2026-08-07 用户裁定（会话授权不进 unattended 续跑）当成 bug 修了，理由自洽、测试全绿、方向正好相反。入口不是"要不要重提这个议题"，而是**用户报的那个症状**（「我的会话授权怎么不生效了」）把人直接领到那两个代码块前面。判据两句：① **一条裁定如果只有散文，它欠一条会红的测试**——测试要**先证明该机制在正常情形下确实生效**（授权在 attended 会话里确实抑制重问），再断言被裁定的那一侧，否则它可能因为别的原因绿；② 反过来，**动一个"看起来是 bug"的顺序/默认值之前，先 grep 它在 `docs/reference/` 里有没有名字**——这类裁定按定义长得像 bug，那正是它需要被写下来的原因
- **一个布尔够挡住调用，不够解释调用——而下游有三个消费者都在等那句解释** —— `a || b || c` 形状的闸，人看到的卡、模型收到的拒绝、审计读到的 detail 三处拿到的是同一句对**所有臂**都成立、对**哪条都不可行动**的套话。把"为什么"做成一个**有名字的有序枚举**（一个 `id()` 给机器、一个 `reason()` 给人），排序判据只有一条：**这句 reason 不许误导读者「改什么能改变结果」**——所以**不可移除的地板排第一**（对同时命中显式 `ask` 条目的 `vault_store` 报"策略说 ask"，等于把运维指向一个改了也没用的设置）。配一条「链分类的集合 == 闸住的集合」的守卫：有闸必有因、有因必有闸
- **`HashMap` 上的"最严格者胜出"，动作是确定的，胜出的那条 key 不是** —— `restrictive_min` 可交换 ⇒ **值**不受迭代序影响，于是没人多想；但一旦要把**是哪条规则**说给人听（卡片引用 override key），同等严格度下引哪条就成了随机的，同一张卡两次渲染可能指向不同条目。判据：**把一个聚合结果的"来源"暴露出去之前，先问这个来源在无序容器上唯一吗**；不唯一就补一条确定性破平（字典序最小 key），并用「重建 N 次 map 重采样迭代序」的测试钉住——不是断言一次
- **两个子系统是孪生时，一边修好的判据要主动搬过去，而不是等它在另一边被重新发现** —— cron 与 heartbeat 在**三个**共同问题上曾有分歧（慢任务阻不阻塞循环 / 投递失败算不算失败 / 告警投递失败要不要寄存），每一次都是 heartbeat 对、cron 错，而 cron 那侧的错法各不相同、各自静默。改动其中一个时问：**这个判据的孪生子系统怎么回答同一个问题**？答案不同就有一个是 bug。收敛时真源落在被依赖的一侧（`tasks/shared/`），不要复制 → §4.13c。**2026-08-11 第三次复发，这次孪生的是两个断路器**：`GuardianBreaker`（provider 健康）一直是 `Closed/Open/HalfOpen` + 300s 冷却 + `record_success` 复位，而 `DenialLedger` 的会话暂停**只增不减、任何批准都不复位、永不重开**——两者对「熔断了怎么恢复」给出了相反的答案，且后者的常量 doc 与模块 doc 都自称「**连续** 3 次」。症状最贵的那一面是**它打到的是最认真的用户**：对三件**不同**的事说「不」＝该会话此后每一道确认门（含 chat-tier 设备唯一的授权途径）无卡直接拒，出口只剩把档位拉宽——**一个把用户推向最不安全设置的闸已经反转了自己的目的**。判据补一句：**"熔断/暂停/降级"这类状态，写下它的同一笔里就要回答"什么让它恢复"**；答不上就不是断路器，是保险丝 → SECURITY.md《The denial breaker recovers》
- **「先认领、后执行」的调度，界限要在执行时刻成立** —— 只在入队处判等于没判（`loop timeout_minutes` 曾在上限之后多跑 119 分钟）。**推论（适用于任何长跑单元）**：凡「先认领、后执行」的调度，界限要在**执行时刻**成立；只在入队处判等于没判。**2026-08-05 已在 goal 的 wait-barrier 上复发一次**——同一个形状，第二个子系统：那里绕过 claim 的 boot rearm 路径连一道界都没有，所以「谁绕过了认领，谁就得自己带界」是这条纪律的第二半。→ §4.1/§4.2
- **「先记录意图、再做不可逆动作」：只记录"做完了"的机件，分不出"没做"和"做了但没记上"** —— 跨越不可逆边界**之前**盖持久戳，新进程拿到状态那一刻按"结果未知"退休 → §5.6
- **一次性的章不能在动作确认之前花掉** —— 要么事后盖，要么必须可归还
- **一次性的动作，哪个面执行了哪个面就是唯一机会** —— 工具面触发、RPC 面不触发，事后补不回来
- **一个动作有两个终端臂时，"公告"这类副作用默认只写在成功那一臂** —— `execute()` 的 `publish_session_updated` 住在 `Ok` 里，而**失败的一轮同样移动了 transcript**（harness 在派发**之前**就 append 了用户消息，错误回执也落了盘）⇒ 每一个靠这一帧重新水化的面停在失败前的状态。判据：写完终局副作用后**数一遍这个函数有几条终端路径**；两条以上就抽成一个共用函数（参数只推导一次），并留一条**点名失败路径**的守卫——只数调用次数会被"在成功臂里复制一遍"骗过 → §6.9 ③
- **一个吞掉关闭键的模态，必须能产出服务端接受的每一种答案** —— TUI 的 `ask_user` overlay 只渲染菜单，而 `Esc` 对它是**刻意吞掉**的（run park 在 oneshot 上，关掉就是孤儿）。于是一个**没有选项**的问题（自由文本对每个问题都合法，工具描述正逐字要求模型别加 "other" 选项）成了谁都答不了也关不掉的模态，能离开它的只有 Ctrl+C（取消这一轮 run）和 Ctrl+C×2 / Ctrl+D（退出 TUI）——**没有一个键能回答它**，用户只能放弃这次工作。而它只是"少一个输入框"，读起来完全不像 P1。判据两句：① 数一遍**服务端接受几种答案**，客户端能产出的必须是同一个集合；② 拒绝关闭的 UI 欠一条**终局帧**，"再答一次"不算出路——那个问题可能已经不在了（`stream.clarification_ended` 有第三张脸从没接过）→ §5.3 第七轮
- **同一个选择，展示形态与线上形态分家之后，按展示形态回话会静默降级** —— 「存储 vs 展示」那条的**客户端孪生**。Panel 点选项发 1-based 索引，TUI 发标签字面量；标签长出 `— description` 装饰的那一刻，TUI 的回复不再等于任何选项，`interpret_reply` 当自由文本收下 ⇒ **对人完全正确，对模型是"用户没选任何一项"**，零报错。判据：**回程走的是哪一种形态**——渲染尽管加装饰，回话必须发那个不带装饰的键 → §5.3 第七轮
- **一条按元素成立的规则，用在整次调用上就会连累它没话说的那些元素** —— 四问里一问要凭据，`secret` 于是否掉整次 `ask_user`：另外三问人没看见，模型拿到一个错误而不是三个答案。判据：**这条规则的主语是"这一个元素"还是"这一次调用"**；是前者就在注册/投递前分区，并把被扣下的那些**点名报回**（形状答"有没有没问到的"，别让消费者去维护一张情形清单）。全部被扣下才整体失败——park 在一个没有问题的请求上是 600 s 停摆 → §5.3 第七轮
- **同一件事有两个 id，就等于没有寻址路径** —— 进程内句柄与持久记录键各自现造 UUID、互不指认时，那份数据**在库里但够不到**，症状是"查无此物"而非报错（`subagent` 的 `request_id` vs `SubagentSpawned.child_id`）。判据：**这个 id 在写它的那条记录上出现过吗**？位置/时间关联不是替代品——同一 turn 的并发兄弟共享 `turn_id`，按顺序对齐必然串台 → §4.11
- **纯内存的注册表在进程消失后不是"空了"，是"撒谎了"** —— 它对已完成的工作回答"从来没有过这个东西"，而调用方的合理反应是**重做**。能力对标时逐行问**这张表在进程消失后还成立吗**：只比内存内的生命周期管理（容量／LRU／TTL）会让整张表漏掉这一维（§4.11 round-10 漏了一整轮）→ §4.11 §4.13b
- **崩溃边界上的"未知"不能写成"失败"** —— 派发前已落盘的意图 + 没有应答 ⇒ 副作用**可能已经落地**；说"它失败了"等于请求重复执行不可逆操作。陈述认知状态，重做与否归模型 → §4.13a
- **一个为了绕开缺陷而加的步骤，会顺手解释掉它自己制造的症状——而那句解释读起来像一条产品结论** —— 「同一事实的两份表述」在**夹具**上的形态，也是这一族里最容易自我说服的一种。真机夹具为了让分页可测，把语料改键到读者看的分区（`relocate_notes.py`）；它只改 `notes_index`/`notes_links`，FTS 与向量行留在原处，于是每一个检索面都**诚实地**报 0→0。那个 0→0 被记进三处文档，措辞是「本夹具结构上答不了检索面，是夹具的代价不是产品结论」——每个字都对，而真相是**检索透视和被绕开的那个缺陷是同一个缺陷**，绕开步骤只是把它从「读不到笔记」搬成了「检索不到笔记」。判据两句：① 写下「这是夹具的局限」之前先问**这个局限是不是我刚刚亲手造的**——凡绕开步骤，都要逐个数它动了哪些表、没动哪些表，以及**没动的那些是谁的输入**；② 绕开步骤的正确归宿是**随修复一起删除**，删掉之后夹具就从「绕开缺陷」变成「缺陷的回归测试」，而这两者在 PASS 数上长得一模一样（本轮 9/9 → 修好后 seed 直接断言 `listFacts(base).total == NOTE_COUNT`）。
- **一个「没找到」的读数，可能是量具在用另一种形态搜索——而它和「这东西真的不在」在报告里逐字相同** —— 本轮两次，两次都差点被我报成产品缺陷。① **`textContent` 是原始文本，`innerText` 是用户看到的文本**：分区徽标由 CSS 大写，屏幕上是 `MAIN__U-OWNER`、DOM 里是 `main__u-owner` ⇒ 拿屏幕上那个去匹配 `textContent` 恒不命中，而「不命中」读起来正是「这个徽标没渲染」。② **一次 before/after 比较跨过了一次被拒的保存**：第二次读数时那张卡还停在编辑态，抽取到的于是是另一行 ⇒ 对一个**没变**的列表报「变了」，采信就是一条 P1（「超预算被拒却改了列表」）。判据两句：**比较只在两次读数的取景框相同时成立**（先离开瞬时状态再测，别在瞬时状态里测），以及**写下「X 不存在」之前先问我搜的形态是不是它存储的形态**。同族是既有的「量具会骗人：`ls -1` 不列点开头的条目」，只是那次漏计、这两次是**反向**——量具报出了一个并不存在的缺陷。
- **「这是本机」不能采信被测方的自述，而照妖镜的通用形式不是来源 IP，是「一个只有本机才答得出的值」** —— [[feedback-local-build-local-test]] 记的照妖镜是服务端日志里的来源 IP，本轮**用不了**：那台 `aleph-server` 根本不记逐请求访问日志。真正的证据是页面上那句 `显示 500 / 1040 条笔记`——1040 是几分钟前种进本机 scratch 库的数，别的机器答不出它。**所以夹具要故意种一个别处不可能有的值，它同时是断言和身份证。**配套：一个报 `isLocal: true` 却对 loopback `ERR_CONNECTION_REFUSED` 的扩展实例，**该换的是仪器不是结论**——同一台机器上 `chrome-devtools-mcp` 自己拉起 Chrome、不走扩展通道，一次跑通 12/12。**「我只有一个仪器」时，「测不了」和「它坏了」也长得一模一样。**
- **一条会误报的守卫比一条不报的守卫贵，因为它会被当成证据引用** —— 「窗口式扫描要问它的边界是谁定的」那条判据的**假阳性**方向。`every_memory_dispatch_arm_composes_the_partition` 用固定 20 行窗口，而一次无关的注释改动（给那条臂加了段锁竞争说明）把解析推到了第 21 行 ⇒ 守卫从此点名一条**四轮来一直正确**的 `recall_context`。代价不是那条红本身，是**它被写进文档当成 D2 的现场证据**，于是一条真缺陷的记录里挂着一条与它无关的红。判据：① 窗口止于**被扫描单元自己的语法终点**（这里是下一条同缩进的 match arm），不是行数；② 加一条「这一轮到底扫了几条」的自保断言——扫不到和扫过了在报告里长得一样；③ **一条长期红着的守卫要定期回去问它现在还在说真话吗**，尤其是被别处引用为论据的那些。
- **关于「这几道闸到底覆盖什么」的散文没有测试，所以它每被触碰一次就漂一次——而每一版都比上一版读起来更像强论证** —— 上一条讲一条**会误报的守卫**被当成证据引用，这条讲被引用的东西根本不是守卫，是一段**话**。同一句覆盖声称连续**三代**写错，每一代都出自一个称职的人、都在纠正上一代：①「layer 1 无论怎么拼都拒绝」——它拒的是**形状**不是**来源**；②「layer 4 对第二次解析一律变红」——过宽；③「反对『第二个答案』的是 layer 3 的计数」——计数反对的是第二个**出现**（第二个调用点 / 第二次铸造 / 一条 import），不是第二个**答案**，一个算对了的 fork 一次都不加。**修法不是写第四稿，是让每条覆盖声称成为一条会红的测试**——**包括断言「某个洞是开着的」那几条**（`flow_scope_census::tests` 里逐层各一个用例），命名成「谁将来堵上它，谁就被迫更新那段散文」。配套三条：① **每条 bound 要标明是谁在撑它**——一条具名测试 / 一次记录在案的实测 / 别的模块自己的守卫；而**写不成用例的必须在原地承认它不是用例**（编译失败断言、「那一组测试里有几条属于第 N 层」这种计数、以及模块之外的读者，都不是），否则「每一句都有用例」本身就是第四代；② **「加机件去让一句写宽了的话变成真的」是把同一个错误再犯一遍**——正确的动作是把话写窄，不是把闸加宽；③ **一条守卫的语料如果由守卫自己的作者提供，它就不是独立的地面真值**——本轮刻意让活普查与「声称测试」**共用**谓词函数（不然「这一层反对什么」就有了第二个答案，正是这个模块存在的理由），代价是**一个错的谓词同时错在两个地方**，所以语料常量必须是独立的地面真值，**且这件事要用变异证明**：破坏一个谓词，看声称测试红不红。
- **一个裁定的代理不是那个裁定，而代理失效的方向恰好读起来像违规** —— 我拿「`run_loop/mod.rs` 零删除」当某条裁定的代理用了 **44** 个 commit；第 45 个是 `7 1`，删的却是**另一个函数** doc 里的一行。判据两句：① 代理只在**它和被代理的性质分不开**的那些情形里成立，**写下它的同一刻就要说出哪一类情形会把两者分开**——说不出就不是代理，是巧合；② ⚠️ **而让我读错的是量具：`git` 的 hunk 头上那个函数名不是被改的那个函数**，它是 git 从 hunk 起点**往上**找到的最近的前一个 item（本仓合成验证：删掉 `fn beta` doc 里的一行，hunk 头打的是 `fn alpha()`）。所以拿 hunk 头判「哪个函数被动了」会**稳定地**答错一格，而错的那一格总是指向上一个函数——它不像噪声，它像证据。
- **修好一处，会让它下游从没跑过的路径第一次真正跑起来** —— 那不是回归，是**新暴露面**，值得在同一轮重审
- **跨会话只能"降活跃度"** —— quiet（stop/pause/clear）可跨，arm（start/resume/加配额）必须在该单元自己的会话跑 → §4.1/§4.2
- **窗口以天计的检测器必须持久化** —— 把窗口长度 × tick 周期，跟进程实际寿命比一比；量级接近就必须落在进程之外 → §2.8
- **同构分区有 N 个，维护默认只覆盖第一个** —— 写路径经合成 id 分区（`{base}__proj-*`/`__u-*`/`__p-*`）之后，每一条"遍历全部"的维护/对账/枚举路径都要重问「它遍历的是 default 那一个，还是全部」。已在**两个**子系统上各犯一次：做梦历史（§2.8）与笔记索引开机对账（§2.5）。**枚举必须有单一源**（`project_scope::list_note_corpora`；此前同一问题有三份互不一致的答案），且**脚手架（写）不得跟着对账（修）一起 fan-out** → §2.5 §2.8
- **DEFER 若建立在「这条路走不到」上，就欠一次真实负载实测** —— 猜出来的边界只在边缘塌陷，而边缘正是真机所在 → §3.14
- **一条注释可以在写下时完全正确，后来变成谎言而读起来仍然自洽——因为它引用的是一个会变的外部事实** —— 「同一事实的两份表述」那一族里最难看见的一种：没有第二份表述在打架，只有一份，而且它自证充分。`claude-sonnet-5` 的价格行逐字写着「durable rate $3/$15（$2/$10 是跑到 2026-08-31 的 launch promo，记 durable 免得促销结束后低报）」——推理无懈可击，只是厂商后来**取消了那次涨价**并把促销价定为标准价，于是这行注释每读一次都在把一个 50% 的高估论证得更牢。判据：**改一个数字之前先读它的注释；注释里的理由如果指向一个外部事实（厂商公告 / 上游目录 / 某个"还没公布"），那要核的是那个事实，不是这个数字**。⚠️ 反向也成立且更贵：**外部目录也不自动正确**——同一次比对里 models.dev 抄的正是那个 promo 价，照单全收就是把注释里已经写明的取舍推翻掉。→ MODEL_CATALOG §7「表里的数字过期了怎么办」
- **能被精确回答的数字别用常量猜** —— 先问仓库里有没有人已经知道它；换算单位有没有单一源 → §4.9
- **把一个字段升格成运行时能力之前，先数它有几个写入者** —— 休眠的展示字段（`workspace_path` 曾只是 picker 里的一行字）一旦接上运行时权力（成为 run 的 cwd），它的**每一个**写入者都追溯成了权限授予点；写入者与读取者必须同批过同一道闸，否则「两步都合法、合起来等价」（先注册目录、再进房间聊天）就是绕闸路径 → §5.22

- **在构造期解析的身份，是一个没有生产者也照样"成功"的身份** —— `unwrap_or_else(|| "main")` 之类的兜底把一个**字面量焊进每一个消费者**并持续整个进程寿命，而**错误的身份是完全合法的身份** ⇒ 零报错、零测试红（`researcher` 领导的团队拒绝自己的 leader 并接受 `main`；审批全记在 `main` 名下）。判据两句：① 这个字段**有没有生产者**（grep 赋值点，不是 grep 类型）；② 施动者该由**这次调用**决定还是由**进程启动**决定——是前者就从 `TURN_CONTEXT` 每次取，构造参数只配当 fallback（单一源 `builtin_tools::acting_agent`）→ §4.13b
- **一个进程全局的表被第二个实例写，症状是「我的行不见了」而不是「多了几行」** —— 上一条的实施陷阱。给内存表加 sidecar 时，写盘那半必须绑在**实例**上而不是全局读一个开关：`ProcessRegistry` 每个实例的 `next_id` 都从 1 开始，台账按 id 键控 ⇒ 第二个写者不是追加，是**用另一个 owner 覆盖行 #1**，真 owner 的 lookup 从此答"没有这个东西"（即这条 sidecar 本来要修的那个谎，换了个来源）。生产只有一个实例所以这问题不存在，**测试并行造几十个**，于是它以**单跑绿、全量红**浮出来——孤立跑的那些测试才是在说安慰话的那一方。修法是把非 journaling 构造函数标 `#[cfg(test)]`，让"别造第二个写者"从约定变成编译错误 → §3.15③
- **用名字 grep 找断线，会漏掉那种「唯一的外部引用就是撒谎的注释」的断线** —— `kill_all_running_background` 全仓两处命中：定义，和 `process_registry.rs` 上那句声称它已接线的 doc。**把 bug 藏起来的注释正是它唯一的搜索命中**，所以扫断线前先剥掉注释行。同族：`#[derive(Serialize)]` 的 struct 字段没有 Rust 消费者是正常的——它们真正的消费者是读 JSON 的模型 → §3.15①。⚠️ **反方向同样成立，而且它会让一条新守卫在写下当天就说谎**：一条「这个名字必须已经不在了」的源码守卫，会被**解释它为什么该消失的那段注释**满足（本轮 `memory_search` 的 `!code.contains("retrieve_all_agents")` 正是如此）。两个方向一句话：**扫描器判的是代码，注释是文档；`.filter(|l| !l.trim_start().starts_with("//"))` 是两边共同的前置**
- **一个「所有 X 都必须过闸」的守卫写完之后，问的不是规则对不对，而是它认得几种注册形状** —— 描述字节棘轮与 `no_sentence_is_stated_twice` **同时**漏掉十个工具 13,389 B，因为两者都只读 `BUILTIN_TOOL_DEFINITIONS`，而 `agent_init` 还会从 registry map 把工具表补全。两把尺、同一个盲区、同一处成因。收敛时**两者读同一张表**（`REGISTRY_ONLY_DESCRIPTIONS`）——各持一份清单就是这张表要防的错误挪高一层 → §3.15⑦
- **把哨兵发出去再从对方的回复里找它，只有在对方不回显你的请求时才成立——而回显是很常见的** —— `wait_probe` 的 `ALEPH_WAIT_FOUND` 是它构造的**每一个探针源码里的字面量**，而 `playwright-cli eval` 的回执带一段 `### Ran Playwright code` **把跑过的脚本原样贴回来**：`out.contains(SENTINEL)` 于是在第一次轮询就为真，**默认 driver 上每一次 `browser_wait_for` 都谎报 found**，模型对着还没渲染的页面继续动作，零报错零红测。判据不是「这个哨兵够不够独特」（它很独特），而是**我搜的这个通道里有没有我自己的问题**。修法**不是**在探针里耍花招把哨兵拆开（下一个"简化"它的人就会把 bug 装回来，而且模型等待的文本恰好是哨兵时照样中招），而是**让那一层交出值而不是通话记录**（`parse_result_value` 抽 `### Result` 段），并把这件事写成 trait 方法的契约。⚠️ 这类 bug 对 fake backend **结构性不可见**：假后端返回的正是代码所期望的那一种东西 → §3.12
- **一个值被检查过和被使用之间如果有 `await`，那次检查的有效期就是那个 await 的长度** —— 而**审批 await 的长度是人的反应时间**。判据不是"这里有没有 TOCTOU"（到处都有），而是**这个窗口里有没有别人写得进来**：沙箱进程能往自己的 session workspace 里写，所以兄弟命令换掉 cwd 的一个组件就够了。闸要架在「**这次调用是否真的 await 了人**」上，不是「网关配没配」——没配的网关立刻应答、缓存的授权根本不问，两者都不开窗口 → §3.15④
- **一个权限层按某个轴分级，那个轴就不能由调用方自己挑** —— `tool_permissions` 三层合并的中间那层是**按 agent** 的，而 `agent_id` 是 `chat.send` 上调用方填的字符串、四轮无人校验：被某个 agent 的 `deny` 挡住，换个名字就换一套权限。**会话轴的闸答不了这一问，而且按设计答不了**——换 agent 产生的是一条**全新会话键**，`existing_session_is_visible` 必须放行它（新对话的第一句）。判据两句：① **指出这一层的分级键，再问「这个键是谁写的」**，答案是「请求里带来的」就等于没分级；② 闸放进**所有入口共用的那个构造器**并让它成为**必填参数**——编译错误强于注册表 pin，所以那种情况**别再加**登记条目或源码 pin（第二个更弱的真源）。顺带永远追问「有没有两步都合法、合起来等价的路」（这里是委派面）→ §5.17
- **一条问责记录只记「哪个身份」时，对「哪个人」是沉默的——而那个人通常早就在上游算好了** —— 签名链记了四轮 `agent_id`，多用户装机上分不出 operator 和 member 的动作：非否认对 agent 成立、对人不成立。而 `build_run_request` 早已把 `AUTHOR_USER_KEY` 盖进 run metadata、`ambient_actor()` 早已能在工具面读到它——**两端都在，中间那根线没接**。判据：给任何「谁做的」字段命名前先问**它答的是哪一问**（哪个身份 / 哪个人），再 grep 另一问的答案是不是已经躺在 metadata 里。⚠️ **往 preimage 加可选字段：排在最后 + 空值一个字节都不发**，才不作废既存链（`opt_lp` 的 0x00 标记会）；兼容测试**手工重算旧布局**，别钉 hex 字面量——没人分得清真的 golden 和被刷新过的 golden → §5.17
- **进程内存不是状态：凡"重启后这个 id 还查得到吗"答不上来的表，都欠一个 sidecar** —— 而且**只记录"做完了"的机件分不出"从没跑过"和"跑了但写丢了"**，所以开机对账要写**终态墓碑**而非删行（否则 not-found 同时意味着"你打错了"和"它随上个进程死了"，还顺手扔掉死前的产出）。**推论**：产物一旦跨进程落盘，脱敏就不能再按 run 的 attendedness 决定——读它的是**后来那个进程**，而它可能把内容扇出到聊天通道 → §4.13b §4.13c §5.1
- **一份实施计划的冲突扫描如果只两两比较任务，就看不见一个"没有任何任务认领"的生产者** —— 两两核对回答的是「这两个任务的产出和消费对得上吗」，回答不了「这份计划里读的每一个进程级句柄/注册表，有没有一个任务真的在生产路径里装它」。per-principal 花费上限那一轮里，`spend::install_policy`/`install_ledger` 被 9 个任务共同依赖读取，而没有一个任务的验收标准写"调用它"——两两扫描因此全程干净：句柄的实现者（Task 4）与读者（Task 5/6/7/8）之间从未出现分歧，分歧根本不在任何一对里。生产上 `current_policy()` 永远读到 `SpendPolicy::default()`（`enabled()` 恒假），`spend::check` 从不拒绝任何调用；`global_ledger()` 永远懒回退到 `InMemorySpendLedger`，重启清零；`spend.query` 对一台配了 `per_user_usd` 的机器如实报告 `configured: false`——**这句话是真的，问题恰恰在这里**。找它的问法是**一次独立的扫描，不是冲突表里的一行**："这份计划读的每一个句柄/注册表，哪个任务在生产路径里真的装它"，跟"这两个任务是否一致"是两个不同的问题 → §5.22 round-7
- **一个 fail-closed 的默认值和一个从未安装的句柄，从外面看是同一个读数** —— `spend.query` 报 `configured: false`，对"这台机器没配置 ceiling"是真话，对"这台机器配了 ceiling 但 boot 从没调用过 `install_policy`"也是**同一句**真话；两种截然不同的世界只有读源码才能分开，运行时没有任何信号能替读者分辨。凡一个 `OnceLock`/`ArcSwap` 式进程级句柄，其"未安装"回退值恰好落在某个合法配置的取值范围内时（这里是"无限额"），就不要指望默认值自己会说话——闸要么在源码层面把"未安装"变成不可能（本例是 boot 无条件调用 `install_policy`/`install_ledger`，配上按名字报错的源码级 census 钉住这两个调用确实存在），要么在诊断面上把"我没被装"和"我被装成了这个值"分开报告 → §5.22 round-7。⚠️ **这两半现在都有单一源，别再手搓**（2026-08-26，同形句柄 46 个）：`CapabilitySlot::install(v)` 把**写值与盖戳做成同一个动作**（"记得也顺手 `mark()` 一下"那种纪律，失效起来是一句**自信的假话**，比今天的沉默更贵），条件安装的 `else` 臂调 `decline("缺了什么")` 而不是静默跳过；诊断面 `core/capability-wiring` 是三态——**未 boot**（这个进程答不了，去问 daemon）· **booted 且完整** · **booted 有洞**（逐 slot 报，严重级由 `MissingSemantics` **派生不手填**）。⚠️ 两条边界：① **`MissingSemantics` 要从消费者那边推，别抄配置默认值**——`reads_as` 写的是那个回落分支**真正交给读者**的值（`INSTALLED_LOCALE` 是 `t_ui` 的 `En`，不是 config 默认的 `Zh`），而**按句柄的名字读会读反**（两个 approval requester 听起来像"闸不闸了"，实际每条 `None` 臂都硬拒 ⇒ 永久自动拒绝）；② **懒缓存不是能力句柄**——只从自己够得到的数据 `get_or_init` 的那种，"还没建"在那里是正确答案 → §5.25
- **一次扫描只能为「它枚举过的那些形状」背书，而结论通常是按目录写的** —— 本轮用尽了文件系统的每一种拼法（`.exists()` / `read_dir` / `metadata` / 每个 `fs::`）去扫「把『我看不了』答成『那里没有东西』」，然后写下「这个目录里没有第九个」。**真的有第九个，而它一次文件系统调用都没有**：`Err(_) => Missing` 加上 `spawn_blocking` JoinError 的 `.unwrap_or(Missing)`，每个 `Missing` 都渲染成 `[ok]` ⇒ 一个 panic 掉的探测任务报出来的是「没装浏览器」。判据三句：① 结论的作用域要写成**方法能看见的那一类**（"没有别的 fs 形状的实例"），不是**目录**（"这个目录里没有别的实例"）——后者是前者悄悄放大一档；② tell 是**同一份报告披露了目录外的实例、却漏掉了目录内的**，那说明搜索是按拼法组织的而不是按性质；③ 「列举法只覆盖立法当天的世界」在这里升了一层——漏的不是一张**名字**的清单，是一张**形状**的清单。⚠️ 同一条对**否定断言**成立：正面发现可以来自阅读，而「其余 N 个没有这个问题」必须有一次搜索背书，且**那次搜索的作用域就是结论的作用域**——本轮有人靠阅读找到一句陈旧注释（正确），紧接着断言另外九个文件没有这句话而背后什么都没有（假的），**恰恰是正面那一半为真，才让人不去查否定的那一半**。⚠️ 第三个轴是**平台限定**：一份带平台限定的缺陷报告，说的是它**在哪里被找过**，不是它在哪里存在（一条记成「Windows-only 红」的间歇失败，三天后在 macOS 上复现——要么当初那个限定太窄，要么那是同名的第二个机制，两种都得写下来）→ 附录 C.3
- **一次「严格更强」的替换，会顺手丢掉旧形式顺带抓住的那个性质** —— 而这笔交易通常是隐形的，因为新结构在你正要修的那件事上确实严格更好，没有人会去找旧的那个**顺带**抓住过什么。`assert_eq!(known.len(), LIST.len())`（总数对总数）换成 `BTreeMap<file, usize>`（每个登记文件必须精确匹配一次）：强化是承重的、并被一个在旧形式下会互相抵消的变异证明过，但标量比的是两个**总数**，所以一条**重复的登记行**（1 条匹配、6 行登记）会响亮失败，而 map 按路径做键、重复行塌成一个键就过了（一行补回 `assert_eq!(per_file.len(), LIST.len())`）。判据：**换掉一个聚合量之前，问旧的那个能看见什么而新的看不见。** 同族是既有的「解析只能证明超集，永远证不出相等」 → 附录 C.3

### 1. Prompt · 前缀缓存 · 上下文（`src/thinker/` `src/context/`）

> 本仓**唯一一类症状只出现在账单上**的失效：缓存命中按 ~10% 计、缓存写入按 **1.25×** 计，两者都"正常返回"。

- **`stability()` 说字节变不变，`priority()` 说阅读顺序** —— 用其一推另一（两个方向都是）就是层跑错区的成因 → §2.18
- **"跳过注入"永远不等于"没有 marker"** —— 否定分支必须显式 `strip_cache_control`，否则 `cache_retention=off` 反而更贵 → §2.18
- **dynamic 系统尾不是免费区** —— 未打标的 dynamic 块排在**每一个**消息级断点之前 → §2.18
- **断点只押在下一轮仍在那个下标的字节上** —— 合成 `<system-reminder>` 尾与 MoA guidance 尾都不持久化；跳过它们不消耗预算，**往前放永远安全** → §2.18 §4.9
- **表上每个 Dynamic 层都欠一个自己的界** —— 层自造文本 assert 在层里，层只透传 assert 在**生产者**里；`CONDITIONALLY_SILENT` 上的层棘轮读数恒 0 B（位置对 ≠ 大小没问题）→ §2.18
- **守卫在场 ≠ 判据完整** —— 凡因"每次生产请求都设置它"而进 `resolve()` 的字段，必须同批进 `stable_prefix_ignores_per_run_facts` 的 shift（`SandboxSummary::isolated_worktree` 曾漏）→ §2.18
- **一层只有一个 `stability()`；两半分歧时答案是拆层，不是取更差的评级** → §2.18
- **任何按 HashMap 迭代序进 prompt 的集合都是缓存炸弹**；**任何"重建整个配置结构体"的 RPC 都会静默吃掉 DTO 表达不了的字段** → §2.18
- **`CacheMonitor` 的判据必须是读主导（`reads >= writes`）** —— "任何非零 read 就清零 streak"在实际布局下永远攒不到告警 → §2.18
- **提示词散文里点名的工具，是那个工具名的第二份拷贝，而且是模型真正照做的那一份** —— 它没有编译器也没有调用点，所以工具改名 / 从来就没有过这个名字，两种都不会红：`agent_catalog` 的引导句让**每一个** Full 模式提示词去调 `delegate`（那是 `groups.rs` 的一个工具**分类 id**，真名 `subagent`），住在 Stable 块里，代价是模型每次照做都换来一次 tool-not-found。守卫要**解析句中每一个反引号名字逐个对真工具表求解**，不是断言"句子里含 subagent"——后者是列举法，加第二个工具引用当天就失明 → §4.11 round-12
- **一个"最后 N 条"的保护尾，只有在它数的东西和被保护的东西是同一类时才成立** —— `fresh_tail` 数的是**持久化的回合**，而 compactor 与地板作用的那个向量尾部还挂着最多 5 条**从不落盘**的合成消息（≤4 条 `<system-reminder>` nudge + recall 串）。共用一个预算 ⇒ 6 条的配置在提示全点火时只护住 **1 条**真实消息，而提示恰恰只在长的、失败密集的 run 里点火——也就是压缩真正跑起来的那种 run。**三件事同时坏，方向各不相同**：模型丢掉上一轮刚读过的原文；`latest_user_task` 扫到的尾几乎全是脚手架 ⇒ `<conversation_focus>` **整个消失**（§2.19 修的是「锚到了脚手架」，这是同一个洞的另一半）；`cut_end` 随本轮点火几条提示而抖动 ⇒ 指纹缓存的 `c.end <= cut_end` 在那些轮失配、条目被清、重付一次摘要并用新措辞把 provider 前缀重键——**正是这个缓存存在的理由**。判据三句：① 数之前先问**这个计数里混进了别的类吗**（两类就该是两个数，且是 `+` 不是 `max`）；② 同一课 §2.18 已经在 `PreflightPipeline` 上过一遍，**第二层没被通知**是这一族的常态形状——修一处时 grep 还有谁在用同一个数；③ 传下去的方式要是**必填参数**而不是默认值，编译错误强于登记表 → §2.20 ①
- **手算"我插入了几条"，会在插入条数变成条件性的那天错，而错法是静默丢一条消息** —— `splice_preserved` 插的是 `[用户原话…, 摘要, 载体…]`，载体（执行清单 / 文件台账）**只在窗口里有东西时才出现**；调用方自己写的 `preserved.len() + 1` 因此少 1。下游用它算 gap 坐标 ⇒ 合并窗口少收**最新那条** gap 消息，而 `store_cache` 记的覆盖**包含**它 ⇒ 下一轮缓存命中、整段被摘要替换，**那条消息从此不在上下文里、也从未进过任何摘要**。判据：**只有做插入的那个函数知道它插了几条，就让它返回**——调用方侧的算术是同一个问题的第二个答案，而第二个答案迟早是错的 → §2.20 ②
- **`User` 角色不等于"用户说的"** —— 摘要、执行清单载体、以及 `orphan_tool_result_note` 把孤儿 tool_result 降级成的纯文本（正文就是一整条工具结果）全都骑在这个角色上。逐字保真只跳过摘要 ⇒ 后两者被当成用户原话整条回贴，最贵的一条能吃掉 20k 用户预算，吃的正是摘要此刻在替换的那段。判据：**"这段字是谁写的"要有单一源**（`nudges::is_synthetic_reminder`），且**刻意不能按 fence 一刀切**——`user_interjection_note` 用同一道 fence 包真实用户 steering → §2.20 ③
- **"每个 drain site 都会做 X"这句话，要数一遍 drain site 有几个** —— `splice_preserved` 覆盖四个，SessionMemoryReuse 是**第五个**（它自己 drain、自己 insert），于是**唯一不花钱的那条压缩路径，正是模型丢掉自己执行清单的那条**。同族于「收敛写者时要数一遍写者」，只是这次漏的那个是因为它**没有走那个共用函数**——grep 共用函数的调用点找不到它 → §2.20 ④
- **不写进摘要正文的东西，才在摘要器失败时还在** —— 确定性事实（用户原话 / 执行清单 / 文件读写台账）做成**载体消息**而不是拼进摘要字符串：pi 把 `<read-files>`/`<modified-files>` 附在摘要文本里，一次摘要器失败就一起丢。配套两条：**失败的调用不是事实**（写失败没改文件、读失败没拿到字节，谎报会让模型据以行动），**载体必须有界**（它在预算算完之后才被插进去，即窗口已经超预算的那一刻）→ §2.20 ⑤
- **一个"缺省值"如果回答的是另一个问题，它就不是缺省值，是谎话——而降级路径是它最常藏身的地方** —— `resolve_working_dir` 在 caller 给的路径不是目录时降级到**守护进程 cwd**，读起来是 P7 优雅降级，实际是把 §2.3 旗舰修复消灭的那句谎话按另一个触发条件装了回去：caller 传路径是在断言「这是这一 run 的授权根」，而进程 cwd 回答的是「守护进程从哪启动」。模型把 `<cwd>` 当权威、往里发绝对路径、被 jail 逐一拒绝。判据两句：① 写降级时说出**兜底值回答的是哪一问**，和被兜底的值是不是同一问；不是就该降级到**沉默**，这要求类型上"不知道"说得出口（`Option`）；② 同一个函数里 `None` 参数与"给了但不可用"是**两问两答**——`prompt-size` 传 `None` 问的确实是「这台机器在哪」，进程 cwd 对它是正解，把两者写成同一条回落就是让其中一问永远拿到另一问的答案
- **环境信封只有一个事实源 `RuntimeContext`** —— prompt layer 不许自己读 `std::env`；per-run 字节进可缓存前缀 = 整段会话缓存作废 → §2.3
- **往 `<tag>` 里插用户/模型正文前先 `xml_util::escape_xml`**；**外层转义 ≠ 内层格式安全**（行式块里 `\n` 原样穿过，能伪造权威行）→ §2.3 §4.12
- **「这条尾部消息是不是脚手架」在仓里被三层各自回答过**（provider 缓存断点 / harness 条数 / context 摘要锚）—— 单一源必须落在**产地**：`thinker/nudges.rs::is_synthetic_reminder` / `providers/moa/prompts.rs::carries_advisory_guidance`；防漂移守卫必须断言在**源码**上。⚠️ 一刀切按 fence 是错的——`user_interjection_note` 用同一道 fence 包**真实**用户 steering 消息 → §2.19
- **`messages` 表是投影，不是真源** —— 改模型看到的动 `session_events`（`SessionService::emit_event` / `retire_from` / `retire_through`），改用户看到的才动 `messages`（`MessageProjector`）；`retire_from` 要连 FTS 一起删，`retire_through` 必须保留 → §2.1
- **「这个 run 的 provider」≠「这一轮的 provider」** —— 侧信道（压缩/摘要）会整份继承 MoA 装饰器链，走 `MoaProvider::acting_chain()`，且替换要在 `MeteringProvider` 包裹**之前** → §4.9

### 2. 工具输出 · 结果处理（`src/tool_output/`）

- **内容感知清洗必须跑在扁平化之前** —— `Value::to_string()` 把结果压成一行，四个行制缩减器对 builtin/MCP **全瞎**；清洗点是字段级 `hygiene::clean_result_value(&mut Value)` → §3.14
- **同一个病有三个宿主（builtin / MCP / 每工具压缩器）** —— 「接通了谁来跑」≠「接通了跑在什么上」 → §3.14
- **持久化的永远是未经清洗的原文**（`reduced_from`），否则缩减不可逆、`ctx_search`/`read_file` 挖不回 → §3.14
- **只有类型路由成功时才把信号内联**；opaque 输出保持"仅 marker"——分不清信号与噪声时 head/tail 只是猜 → §3.14
- **缩减器只判定"什么是信号"，不判定"是否更小"** —— 量纲由中央 `Reduction::is_meaningful_shrink` 按**字节**管（数行会把 200 KB 的 minified 单行判成"94% 缩减"）→ §3.14
- **围栏是结构不是内容** —— `<<<EXTERNAL_UNTRUSTED_CONTENT>` 首尾两行是唯一的不可信边界，**半个围栏比没有围栏更糟**；单一源 `content_sanitizer::split_external_fence` + `tool_output/fence.rs::rewrite_interior` → §3.14
- **「最小尺寸闸」的量纲必须和被闸内容的形状一致** —— `MIN_LINES=8` 拦死了唯一一种没有换行的内容类型（JSON 的线上形态就是一行），`tool_output::structured::reduce` 现按 kind 走 `ContentKind::min_lines` → §2.7
- **带 cap 参数的渲染函数要问：有没有调用者传过小于「全部」的值？** —— 没有，那个参数就是断线 → §2.7
- **知道哪些行重要的组件，必须也是决定能放几行的组件** —— 调用点手里的预算不能丢弃（`Profile::for_token_budget`）→ §2.7
- **闸要下沉到它约束的那个东西里** —— 行式摘要器不能摘要一行，这是 `distill_output` 自己的前置条件，不是各调用方各打一次补丁 → §3.14
- **`agent_trace` 是有意有损的镜像**（bounded `mpsc(256)` + `try_send`，满即丢）—— 权威终态在 `run_complete` 的 `summary.tool_summaries[]` / `summary.errors[]` / `summary.plan`；新写消费者必须在流末对账 → §6.1

### 3. 工具层 · 权限 · 路径（`src/tools/` `src/builtin_tools/`）

- **注册不是派发：让模型「看见」一个工具的每一条路，都不会让一次调用「到达」它** —— 下一条讲描述里点名的东西模型调不调得到，这条讲那个工具自己。`plugin_manage` 同时在 `BUILTIN_TOOL_DEFINITIONS`、`create_tool_boxed` 和 `core_tools::reg` 里——三个注册面，每一个都足以让它出现在工具表上、让它 959 B 的描述在每个请求上计费，而 `ToolRegistry::execute_tool` 是一张**手写 match**，没有它的臂 ⇒ 每次调用落 `_ =>` 答 `Unknown tool`。判据两句：① **能被广告的形状有几种，守卫就要认得几种**（这里是目录 `name:` 与 `reg(tools, "…")`，从源码派生而非列举，单一源 `builtin_registry/dispatchable.rs`）；② ⚠️ **散文守不住一条线**——`select_model`/`doctor`/`config_audit` 三个**同一形状**的修复注释，就写在这个缺口上方二十行、逐字描述了它。**为什么 16k 个进程内测试看不见它**：每一个都在问某个注册面「这个工具在吗」，而每一个都正确地答了「在」
- **工具 DESCRIPTION 里点名的每一个东西，模型都得能调——RPC 方法名不能** —— §1 那条「提示词散文里点名的工具是那个工具名的第二份拷贝」的第二形态，而这一半更容易漏：`tool_usage` 逐字告诉模型「removal goes through self_config (MCP), **plugins.uninstall**, or skill_manage」——三条路里中间那条是 JSON-RPC 方法。为它写的守卫**从 `method_census.rs` 读方法名**而不是复述一份（列举法照例只覆盖立法当天），且按**精确的已注册方法名**匹配而不是「看起来带点」的启发式——后者会把 `.mcp.json` / `plugin.toml` / `hooks.json` 一起判成违规。写完立刻又抓到三个没人读过的（`canvas.apply` / `teams.list_templates` / `acp.create`）：**一个守卫第一次跑出来的命中数，才是那一类的真实大小**
- **能力接上了 ≠ 模型会用它** —— 加/删任何 capability 的同一笔改动里必须 grep 工具 `DESCRIPTION`（prompt 在劝模型别用，比缺失更难发现）→ §5.17
- **⚠️ 分类器已经存在，只是没人问它**：`block_goal_on_failure` 曾把**任何**失败都判成 goal 的终态（Blocked + 删焊入计划 + 推「已中止」），而 `ExecutionError::receipt_kind()` **早就是三个用户面共用的单一源**，其 doc 自陈：此层出现限流/网络签名意味着整条 provider 链都试过、失败**确属瞬时**。同一仓里 `llm_retry::extract_retry_after_str` 能给退避时长、wait-barrier 能 park 自唤醒——**三块零件齐备，谁都没连**。判据一句话：写下一个 `if error { 终态 }` 之前先问**「这个错误已经被谁分过类了」**；答案通常是"有，而且比你打算写的那版更准"。同族是 §4.9「能被精确回答的数字，别用常量猜」——那条讲魔数，这条讲**分支**，两者都是「已知事实就在同一个 crate 里，零调用」。
- **一个守卫验了「这个名字指向真东西」，不等于验了「调用它的那个 payload 能到」** —— `/model <id>` 在**每一个**斜杠面上（TUI 键入 / TUI 命令面板 / Panel composer / 全部 channel）从写下之日起就是坏的：`slash_command.rs::build_tool_arguments` 的 15 个 arm 里没有 `select_model`，于是落进通用臂产出 `{input,query,args,input_text}`，而 `SelectModelArgs.model` 是必填 `String` ⇒ 每次 `Validation` 错。而守卫 `aliases.rs::every_shorthand_target_is_executable` **恒绿**，因为它问的是"这个别名的目标在不在工具表里"——目标一直都在。判据：**一张「名字 → 处理器」的表，除了名字解析得出来，还欠一条「参数装得进去」**。修法要**派生不要列举**：required 字段集从目标**自己的 schema** 读（`create_tool_boxed(t)?.definition().parameters` 里的 `required`），工具长出新必填字段时自动红；构造不出来的目标每次重新推导 `is_none()`，不读一张会腐烂成许可证的白名单。⚠️ **别用 `#[serde(default)]` 修**——那是把一个响亮的校验错换成一次静默 no-op，比原 bug 贵。单一源与 RED 证明在 `slash_command.rs::every_shorthand_payload_satisfies_its_targets_required_fields`。
- **目录条目写字面量会整体遮蔽工具常量** —— `BUILTIN_TOOL_DEFINITIONS` 的 `description:` 必须指向常量；守卫 `definitions.rs::tests::no_catalog_entry_inlines_its_description` 是**源码级**的（运行时分不出"来自常量"和"恰好字节相同"）。现在要问的是**这些字节值不值**：`catalog_description_bytes_ratchet` 实测 **93,938 B**（目录 80,549 + registry-only 13,389，其中 workspace_manage 目录条目 +519 B 与同日 workflow 描述调整合并而来），**每个请求都付**（`truncate_tool_descriptions` 默认 `false`，没有任何一档配置让它免费）→ §5.17
- **⚠️ 那个数字 2026-08-10 从 82,462 跳到 93,358，不是花掉了 11 KB，是量具第一次看见全部** —— 上一条的孪生，也是「守卫的绿只覆盖它的块识别器认得的那种块」在**同一条纪律自己的量具**上复发：十个工具（pim/system/automation/permission/media/scratchpad/goal/loop/loop_graph/strategy）由 `builder/core_tools.rs` 的 `reg(` 注册但**不在** `BUILTIN_TOOL_DEFINITIONS` 里，而 `agent_init` 会从 registry map 把工具表补全 ⇒ 它们的描述一直在每个请求上发，只是**从未被任何一把尺量到**。`no_sentence_is_stated_twice` 曾有**同一个**盲区，现读同一张表 `REGISTRY_ONLY_DESCRIPTIONS`（**不是各持一份清单**——那正是这张表要防的错误挪高一层）。判据：给一个「所有 X 都必须过闸」的守卫写完之后，问的不是规则对不对，而是**它认得几种注册形状**；`every_registered_core_tool_is_accounted` 现在按名字红。修法**刻意不是**把十个补进目录——`BUILTIN_TOOL_DEFINITIONS` 同时驱动 ToolCatalog 建行 / fallback 校验 / 渐进披露 / `dangerous_tools` 校验，registry-only 是有记录的既定形态
- **一条 deny 策略有几个执行层，就要问它绑住了几个** —— `deny_read_globs` 的每个生产消费者都是 OS 驱动（seatbelt/AppContainer），而 `file_ops` 另建了一份不读它的凭据清单 ⇒ operator 写 `**/.env` 之后 `bash` 被内核挡住、`file_read` 明文可读，**而他没有任何办法知道**。同族于「一个动词有 N 个面时，谁能看要在每个面用同一个推导」，只是这次落在安全谓词上。翻译要**复用** `deny_globs::glob_to_anchored_regex` 而不是近似它 → §3.15⑤
- **deny 检查有方向** —— `path_is_denied` 只向下问「我在不在保护区里」；还要 `contains_denied_descendant` 向上问「我下面有没有保护区」，且必须共用同一份展开+归一化。会**遍历**的动词（copy/move/delete/organize）顶层闸永远不够 → §3.4
- **取消不是判决** —— 墙钟超时 / 传输抖动 / 审批过期 / 用户取消，四者都不是"关于这次调用的失败"；归因在派发咽喉 `scoped/dispatch.rs`，用**成功态**表达"被打断" → §3.3
- **⚠️ 同一句话的审批版，代价大一档：一个 fail-closed 的答案，如果和"人说了不"共用一个词，账本就会替一个没发生过的决定记账** —— 上一条讲工具派发的归因，这条讲**审批结果**。`ApprovalOutcome::Denied` 曾同时表示「人拒绝了」和**四种「根本没人被问到」**（没接 requester / 解不出路由 / **Telegram 投递失败** / 频道无审批能力），于是一次网络抖动就 ⓐ 让该指纹整个会话粘滞、ⓑ 推进暴力破解熔断器（三次即暂停全部确认门）、ⓒ **告诉模型"用户已经拒绝过这个动作"——一句它会转述给用户的假话**。判据：**先数这个 fail-closed 出口有几种成因，再问"其中几种是人做的决定"**；答案不是"全部"时，那个枚举就少一个变体（`Unavailable` / `DenialReason::Unreachable`，与既有的「timeout 不是决定」同一条规则）。⚠️ 配套：**别在两个门各写一遍 `Timeout => Timeout, _ => UserRejected`**——那个 `_` 就是漏斗（单一源 `DenialReason::for_refusal`）→ §5.12 round-3
- **当系统能为一个意图举 N 张卡时，"连续 N 次"这个阈值量的就不再是用户的行为** —— 并发同款调用各自挂一张卡，用户逐张点"不"＝ N 次 `record_denial(同一指纹)`，一次三路扇出就打满三次阈值。阈值要数**意图**不数**卡片**；同时问它的孪生问题：**这个"是"/"不"送达了并排等着的同款卡吗**（批准级联而拒绝不级联时，覆盖范围由竞速决定——晚到的被账本接住，已经在等的接不住）→ §5.12 round-3
- **一根每请求的旋钮，买不到一次永久的改动** —— 判据一句话：**这个闸站下之后发生的事，寿命比让它站下的那个理由长吗**？`exec_tier` 是 `chat.send{exec_tier}` 的每消息旋钮，而 `self_config` 写 `policies.tool_permissions` 是装机级且永久的 ⇒ `full` 档下一次不举卡的写入，摘掉的是此后**每个会话、每个档位**下的闸。凡"拆闸的那个动词"的闸（§4.12 那条），都必须是**不看档位的地板**而不是某一档的规则 → §5.12 round-3
- **会 park 的工具必须听取消令牌** —— 这个 await 的最长睡眠时间就是取消的最坏延迟；超过一两秒必须进 `tokio::select!` → §4.11
- **同一句话对「转向」也成立，而且这一半更容易只写一半：一个 park 欠两条臂** —— 上一条讲停止，这条讲改主意。mid-loop steer 把用户的话**耐久写进 session log** 并回客户端一个成功，运行中的 loop 在**下一个轮次边界**读它——而这一轮的 Act 正睡在 `subagent{action:"wait"}`（600 s）/ `bash{process_action:"wait"}`（170 s）里，那个边界就是 park 的尽头：消息已落盘、发送方已被告知成功、十分钟内什么都不会发生，零报错零红测试。判据：**新增任何会睡超过一两秒的分支时，`select!` 要同时挂 `cancel` 与 `session::steer_signal::watch_current_turn()`**；只挂一条，另一条静默失效。⚠️ 这一条的**发现方式**值得记住：`wait_cancelled` 的 doc 逐字引用 codex 的 `WaitOutcome::Steered`（「新输入是一等唤醒理由」）来**论证**「必须听 cancel 令牌」，然后只实现了 cancel 那一半——**一段散文引用了一个本仓并不具备的行为，来给自己做论证**，是「同一事实的两份表述」的新变种，grep 调用点找不到它。⚠️ 信号刻意是**纯边沿**（`watch` 通道，无 pending 标志）：level 标志需要一条「已消费」边，而唯一可观测的那条（下一条 `AssistantMessage`）是近似的，永不清 ⇒ 下一个 run 的第一次 park 立刻返回并**永远如此**（每圈烧一轮的早返回死循环），清太早 ⇒ 原 bug 原样保留。**inert 臂必须是 `pending()`**——写成「没东西可听就当作已 steer」会让每个无头 run（cron/内部）的 wait 瞬间返回并汇报一个不存在的用户。它与 `AgentHarness::has_unanswered_user_message`（level·权威·只有 harness 调得动）是**互补**不是重复，分工写在 `src/session/steer_signal.rs` 模块 doc 的对照表里。⚠️ **缝有两种，选哪种不是口味**：**拥有循环的挂进循环**（不为脱身丢弃在途工作，最坏延迟一次迭代）；**wait 是一次不透明调用的**用具名组合子 `SteerWatch::race`（race 后 drop），而走这条路的调用点**欠一条「为什么在这里 drop 已经是安全的」的论证**——`browser_wait_for` 的论证是「派发咽喉本来就在停止时 drop 这同一个 future」，即新增触发器而非新增 drop 站点。⚠️ **`ask_user` 与审批门刻意不接**（一个为提问而存在的 park 不该被「用户在别处作答」终结；fail-closed 的审批只能被决定解掉）。四个已接站点 + 两个拒绝在 §4.8 Round-10 ⑨ 逐个点名 → §4.8 Round-10
- **进程全局的表，新增**枚举**入口时要问「调用者凭什么看见这一行」** —— 模型能拿到的 id 只有两个来源：自己 spawn 的返回值，和枚举面；`list` 是目录不是内容 → §4.11
- **入参规范化要落在参数的解析处**（一个 `resolve_*` 边界），不是某个 handler —— 否则同一个模型对同一个分类得到互相矛盾的答案
- **执行清单单一形状是 `shared/protocol/src/plan.rs::PlanSnapshot`** —— 分解 100% 归 LLM（R7），**不要新建 `todo` 工具**（Panel 按字面工具名 `"scratchpad"` 取数）→ §3.13
- **执行档位唯一强制点是 `src/tools/scoped/`** —— 任何新的能执行工具的 surface（新 RPC / 快路径 / 后台产地）不经过它就自带旁路 → [SECURITY.md](docs/reference/SECURITY.md) §5.12
- **一个「地板」如果排在 explicit 条目之下，它就不是地板，是默认值** —— 上一条讲「哪些闸必须不看档位」，这条讲**一条档位规则自己什么时候是地板**。`effective_permission` 原本让 explicit `[policies.tool_permissions]` 条目赢过档位，这对 `Ask`/`Auto`/`Full` **完全正确**——那三档只会**问**，operator 点了名就是他把那一问答了；对 `Plan` 恰好**完全错误**：那一档什么都不问，它每轮发给模型的承诺是「什么都不会跑」，而一条几个月前为别的原因写下的 `"bash" = "allow"` 就能把它掏空，且**只在配置过工具权限的装机上**掏空——即最有东西可丢的那一批。判据两句：① 写下一条新规则时先问**它是「更具体的东西可以覆盖」还是「更具体的东西也不许突破」**，后者要排在 explicit **之前**（现为 `effective_permission` 的 rung 0）；② 判据落在**裁决值**上而非档位名字上——`rule_for` 的契约是「至多 `Ask`」，所以 `Some(Deny)` **按构造**就等于「一个拒绝而非询问的档位」，将来新增的拒绝档自动继承地板；写成 `tier == Some(ExecTier::Plan)` 就只描述了立法当天的世界。⚠️ 这一条**推翻的是该档位出厂时一条有名字、有理由、有测试的断言**（`an_explicit_entry_still_outranks_the_plan_tier`），**不是修一个疏漏**——所以它欠三条「范围可证」的守卫，且三条都已在场：`Ask`/`Auto`/`Full` 逐字节不变、`default = "deny"` 仍是基线不是地板、地板只加拒绝从不发放（幂等工具上 operator 的 `deny` 照旧生效）。少了它们，「地板生效了」和「地板吃掉了整个权限模型」在测试套件里长得一模一样
- **「没有面画那个按钮」不是闸，它只是四个渲染器碰巧一致** —— 判据：**这个值在 wire 上收得下吗**？`allow-always` 从第一天起就是 `exec.approval.resolve` 接受的合法值，而"没有 surface 提供它"被当成了控制（`clamped()` 无条件降级是第三处答案，Panel 硬编码三按钮是第四处）。一个 RPC 客户端就绕过全部渲染器。修法是把「这张卡可以给哪几档」在**闸上算一次**（`exec::allowed_decisions::for_confirm_gate`，输入只有「哪条规则闸住的」和「发起者是不是 operator 档」）、随记录到达每个面、**回来时由 resolver 按同一份列表强制**；渲染器画得更少是安全方向，画得更多不再可能。⚠️ 配套一条：让「不提供某档」成为**编译期必须说出口的事**（`to_outcome_within(allowed)` 没有免参数版本），否则下一个 decision→outcome 调用点默认就是最宽的那档 → §5.12
- **一句"任何配置都关不掉它"的卡片，不能同时提供一个能关掉它的按钮** —— 上一条的第二半，也是 §0「一句关于什么被闸住的话有三份拷贝」在**同一张卡**上的退化形式：`tool_declared` 那条规则的 `reason()` 逐字告诉读者"它在每个档位下都问，`allow` 也关不掉"，而持久授权按钮就是关掉它的那个动作。判据不是"这个按钮危险吗"，是**这张卡自己刚说过什么**。同族：装机级的授权（持久 allowlist ＝ `[policies.tool_permissions]` `allow` 的每调用版）不能由 member 建 —— 他建的那条会静默授权**其他人**的同款调用，而这一点在他的卡片上一个字都没写 → §5.12
- **参数级审批闸只在能举卡的 surface 上成立**，且**举给人的那张卡必须包含闸所依据的那个字段**（按字典序渲染 + 200 字符截断会把被闸字段挤掉）；「操作者显式点名了这个工具」在代码里必须是**精确匹配**，不能用会匹配 glob 的查找 → §4.12
- **闸的范围必须覆盖「能把这个闸拿掉的那个动词」** —— 还要问「有没有两步都合法、合起来等价的路径」。**「另一个工具改配置」就是这条路**：参数级卡的正当性建立在「这条 override 是**人**写的」，而写它的那个工具（`self_config`）此前一张卡都不举 ⇒ 一次不举卡的 `policies.tool_permissions.overrides = {"loop_graph":"allow"}` 就永久摘掉 root/frozen 面前唯一的人闸。判据：**这道闸依赖的那个配置，谁能写它，那个写入面举卡吗** → §4.12 round-12 ②
- **一道「升级给更高权限裁决」的闸，只有在发起者答不了它的时候才是闸** —— 这类闸通常**不拒绝**（`check_operator_gate` 对非 operator 举卡而非报错），于是"谁收到这张卡"才是真正的判据。卡按**发起者自己的 session_key** 登记，而列表/解决两个 RPC 对 member 是开的、可见性只问「这个 session 是不是你的」⇒ **member 自批**，整个 `OPERATOR_TOOLS` 家族一并失守。判据：**写下「转交给更高权限」的分支时，去看那张卡最后落在谁手里**。⚠️ 修法有陷阱：**帧**可以发空 key（复用既有 `OperatorOnly` 编码），**记录不能**——`cascade_session_grant` 与会话级清理都按 `record.session_key` 匹配，两边都置空会让不同用户的卡互相 cascade。安全位要**随记录走**（`ExecApprovalRecord::operator_only`），别去改那个记录**另有用途**的键 → §4.12 round-12 ①
- **判据钉在一个「每轮都会改」的常量上，等于钉在空气上** —— 且它的失效方向是**反的**：`enable_audit` 拿 `job.prompt == AUDIT_TEMPLATE` 认领孤儿，而 job 的 prompt 是安装当天写死的、模板此后改了五次 ⇒ 对**存在最久**的那个环恒假，而它正是这段代码要认领的对象；新装的反而匹配，所以 fixture 与 CI 全绿、只有生产是坏的。判据：**比对用的这两个值，是同一时刻写下的吗**？不是就改用一个不会漂的身份（名字/id），把模板留作次要匹配 → §4.12 round-12 ⑥
- **`Some("")` 不是「没有」，是一条通过每一次 `is_some()` 的假路由** —— 把 `String` 字段无条件包成 `Some(..)` 的注入器，在**恰恰没有那个东西**的回合上（无 channel 的 cron / heartbeat / webhook / `tools.invoke`）产出空串路由：下游 `approval_is_routable` 变真 ⇒ 值守判定反转、`UNATTENDED_KEY` 永不设置，而 prompt 还会告诉模型「运行时会替你投递，不要调消息工具」——**连它自己的兜底也一并拿走**。判据：`Option<String>` 的生产者要问**空串该不该塌缩成 `None`** → §4.12 round-12 ⑤
- **命令硬底线扫的是 `normalize.rs` 的规范化副本**，不是你写的那行字（**多份视图**：POSIX 折叠 / 路径保留 / 各自的 shell-word 版〔全引号删除 + `$IFS` 展开〕；`-enc` 载荷已解码回注、**递归走同一条管线**且关不掉；规则间隙用 `seg!()` 不是 `[^\n]*`）→ [SANDBOX.md](docs/reference/SANDBOX.md) §3.8
- **一张「危险清单」和一条「日常用法必须仍然放行」的守卫是同一笔改动的两半，而且只有第二半会告诉你清单长过头了** —— 设备类加到能拦住 `dd of=/dev/rdisk0` 是对的，加到能拦住 `> /dev/null` 就不再是地板而是停机，**两者之间没有任何报错**。同族的方法论一句：**这类清单的覆盖率要实测不要推演**——推演出来的绕过约一半是假的（本仓实测：`sudo`/`env`/`timeout`/`xargs`/`bash -c` 五种"需要 AST 才能递归"的包装其实早就命中），而候选清单里**必须同时放良性样本**，否则你会用一次静默的假阳性换掉一条绕过，且只有前者用户会踩到

- **一个谓词只有一个消费者时，它的两半会被迫共用一句话——而那句话通常对其中一半是假的** —— `UsageEntry::is_idle` ＝ `never_used ‖ idle_days≥N`，于是唯一的消费者只能写一个标题，它选的 `"N extension(s) idle for 30+ days"` 对 never-used 行**没有可引用的时长**（`idle_days` 正是 `None`）⇒ 装机十分钟的机器把整套 bundled skill 报成月度休眠。判据：**看一个 `‖` 谓词的下游能不能分别描述两条臂**；不能就拆成互斥的两个，别让措辞去掩盖类型上的合并 → §5.24 ①
- **一张清单如果附带一个动作邀请，它列的每一行都必须真的能被那个动作作用** —— 上一条的第二半，且更贵：清理报告列出 bundled skill，而 `remove_skill` 对它们返回 `PermissionDenied` ⇒ 报告邀请了一个必然失败的动作，且在全新装机上那批就是清单的绝大多数（第一印象＝53 件删不掉的东西）。判据：**这一行，被我建议的那个动词作用会成功吗**；答不了就说明缺一个"可作用性"的位（`UsageEntry::removable`），而它必须从**真正拒绝的那段代码**推导，不是从 id 或路径猜 → §5.24 ①

### 4. 网关 · 通道 · 投递（`src/gateway/`）

- **「至多一次」只覆盖了「传输层报了错」那一半——进程消失是第三种结局** → §5.6
- **「按表断言的安全性」在多了第二个生产者之后就作废** —— 安全位必须随记录走（`DeadLetterReason::replay_safe`）→ §5.6
- **队头退避必须带上队尾**（只在一个 batch 内做队头阻塞，跨 tick 就漏，保序**永久性**坏掉且零报错）→ §5.6
- **上限要量对量纲** —— 数行不数字节的 CWE-400 防御，对能装内联媒体的表等于没设 → §5.6
- **「保序」只在慢路径上实现等于没有** —— 让快路径先把慢路径排空（`flush_conversation` 跑在 `send_attempt` 之前）→ §5.6
- **机会性探测不能花别人的预算** —— 瞬时失败原样放回，**但歧义终态照旧结算** → §5.6
- **一条 durable 记录不能比它引用的东西活得久** —— 按引用持久化时字节上限量的是引用而非被引用物；托管在唯一准入咽喉取得，优先把字节收进记录自身 → §5.6
- **「状态」回答不了「该不该自愈」，缺的那一半是意图** —— 先问「我凭什么认为它现在应该是好的」，答案不在被检测方的状态里（真源是 `ChannelRegistry::DesiredChannelState`）；**别在退出路径上无条件覆写 status** → [GATEWAY.md](docs/reference/GATEWAY.md)
- **加了通道 adapter ≠ 用户能配；Panel 有张卡片 ≠ 那个通道存在** —— 「哪些通道配得出来」曾是两份互不对账的表述（服务端工厂表 / Panel `ALL_CHANNELS`），feishu 与 msteams 在缝里各躺四个月：有完整 adapter、有设置表单、**没有 factory** ⇒ 用户填完存下只换来一行 `Failed to create channel`。⚠️ 两层原因都值得记：① 2026-07-26 那次回填**按 `impl ChannelFactory` 枚举**，所以「连 factory 都没有」的这两个对它**结构性不可见**；② 它留下的绊线是**单向包含式**断言，而**两边同时缺失的东西在包含式断言里读起来就是通过**——这类对账的断言必须是**集合相等**。现单一源 `aleph_protocol::channels::CONFIGURABLE_CHANNEL_TYPES`，服务端断集合相等、Panel 断每张卡都在表内 → [GATEWAY.md](docs/reference/GATEWAY.md)
- **一个上游用几条通道表达同一个失败，分类器就得读几条——只读传输层那条，最贵的一类会被判成永久失败** —— §9 那条讲外部 CLI 的「退出码只是其中一种通道」，这是它在 HTTP 上的形态，而这一侧有**下游后果**：`ChannelError::RateLimited` 是 `channel_registry::send` 唯一会重试、`delivery_queue::should_enqueue` 唯一会重新入队的变体，其余一律终态。Lark 把限频**同时**写在两处——现代网关 `HTTP 429`、一批文档化的旧版 OpenAPI `HTTP 400`，而**两者的 body 都带 `code: 99991400`**；feishu 只读状态码 ⇒ 旧版那条塌成 `SendFailed` ⇒ **回复被静默丢弃，且上层不会被告知**。判据三句：① **谓词要是「两条通道任一说了算」**（`429 ‖ code`），不是二选一；② **别把 `400` 单独读作限频**——那是通用 bad-request，会把畸形调用重试到预算耗尽；③ **等待时长是服务器说了算的数字**：Lark 发 `x-ogw-ratelimit-reset`（秒），而代码读的是它从不发送的 `retry-after`，于是每一次被承认的限频都睡那个硬编码兜底值（这条是「能被精确回答的数字别用常量猜」）。⚠️ 配套：为了拿到 body 里的 `code`，**解析必须发生在判决之前**——先判后解析正是旧版看不见 400 那一半的原因；而只解析不留状态码，403 就只剩一句 `error decoding response body`（同一句话对过期密钥、代理 502、真畸形载荷全都成立）。单一源 `feishu/envelope.rs`，真机 fixture `qa/channels/run.sh errors`
- **`ChannelCapabilities` 的每个位都是承诺** —— 声明了就必须覆写对应的 `Channel` 方法（默认体一律 `Err` 并指名道姓）；⚠️ **反向不成立**：覆写存在 ≠ 能力存在——`line`/`wechat`/`signal` 的 `edit` 覆写只是把默认 `Err` 换了句措辞，`feishu` 的还转发一层给同样无条件 `Err` 的 `MessageOps::edit`。判据永远是**那个位**，不是有没有那个 `fn`；而这个位现在是流式的地板（`apply_channel_capabilities`），读错它就是静默截断 → §5.7。频道寻址是**两步**（先 `channel_directory` 换 id）→ §5.18
- **一条唤醒边只回答它自己那个问题，而等待者可能在等别的事** —— 车道原有的两条边（`notify_slot_free` / `mark_admitted`）都在答「run 槽空了吗」，而被 `max_pending_steering` 推回的 steer **恰恰不等槽**（它要 steer 的 sibling 必须继续跑），它等的是 sibling **答话** ⇒ 睡满 `wake_fallback_secs`（30 s）兜底 tick，而三处文档都写着「burst 一排空就重投」。判据：**先说出这个等待者在等的那件事，再看现有的边有没有一条在描述它**；一条都没有时，兜底 tick 就成了你的机制（而它按定义只是安全网）。新边的产地要选**那件事变成事实的唯一接缝**（assistant 轮次有三个产地 ⇒ 只能挂 `MessageProjector::on_appended`，且排在会丢帧的 `try_send` **之前**），并且**只唤醒真正在等它的票**（高频事件一律唤醒＝把别人的「等槽」换成零收益的每轮重试）→ §4.8 Round-9
- **一个在闸之后才算出来的事实，答不了闸的问题——而两者都"有代码"，所以看起来是接好的** —— `carries_more_than_text` 靠 `SLASH_COMMAND_MODE_KEY` 判断"这条消息不能折进正在跑的兄弟"，而 `agent.run`（TUI）唯一的解析发生在 `execute()` 里、即闸**之后** ⇒ 有 run 在飞时，TUI 发的**每一条**斜杠命令都被折成 steering 文本、永不执行，零报错零红测。判据：**这个字段是谁读的，读它的那一步排在算它的那一步前面还是后面**；顺带数一遍**有几个面在算它**（当时有两份推导，弱的那份只产得出 `direct_tool`，于是 skill/mcp/自定义命令在那个面上整类静默失效）。单一源 `ExecutionEngine::stamp_slash_mode`，两条守卫按"凡 spawn 的函数"表述而非按 handler 名字列举 → §5.24 ②
- **车道是候车室，不是运行登记簿** —— 取槽成功时必须 `busy_queue::mark_admitted`，否则 `Steer`/`Interrupt` **静默退化成 `Queue`**（三件事一起修或一起坏）→ §4.8
- **修好一条堵塞之后，被它挡住的每一个动词都第一次真正开火——包括那些「开火」意味着破坏的** —— 上一条的第二半，同一个车道。`mark_admitted` 让后来者能在前驱运行期间够到引擎，这正是 `Steer` 需要的；而 `Interrupt` 拿到同一条通路的意思是**每条排队消息都会在毫秒内杀掉前驱刚变成的那个 run**，一个 burst 里 N 条只剩最后一条活着、N-1 轮工作被销毁，而没有任何人按过停止。判据是**时间性的**：只能取消一个「在本消息**开始等待之前**就已被接纳」的 run（车道半边 `busy_queue::waiting_since`、引擎半边 `ActiveRun.admitted_at`，两边都必须是**单调钟**——墙钟跳变会静默反转判决，而 `started_at` 是给人看的那一份）。⚠️ **两条更便宜的判据都是错的**：「目标是不是本车道最近接纳的那个」会连同「sibling 健康时新到达的真 Interrupt」一起压掉（那正是这个模式存在的理由）；而忘了给「没有 ticket 的生产者」留 `None ⇒ 不设限` 那条臂，修的就不是 burst 而是把 Interrupt 整个关掉 → §4.8 Round-8 ①
- **一个 id 在客户端手里的时刻，和它在投递过滤器眼里可解析的时刻，是两回事** —— `chat.send` 一返回，客户端就握着 `run_id`；而 `EventVisibilityIndex` 的 run→session 种子只来自 `stream.run_accepted`，那是**准入之后**才发的。于是每一个「这个 run 从没进过引擎」的终局帧（车道满 / 等待超时 / 被停止清掉）都分类 `ByRunId`、解析落空、**fail-closed 拒给每一条连接，operator 也不例外**——三条写在文档里的用户回执因此全是静默丢弃，其中一条正是专门写来关掉「停不掉的 pending 气泡」的那条。修法是**让帧自报归属**（`RunError.session_key`，只有 `spawn_queued_run` 设它），并把播种判据从「topic 是不是那一个」改成「**这一帧有没有同时说出 run_id 和 session_key**」——`note_frame` 跑在过滤器之前，所以帧给自己播种。判据一句话：**加一个只在准入前发得出来的帧之前，先问它凭什么被解析**（同族＝ `src/gateway/CLAUDE.md` 地雷 H：解析句柄的安装条件不得比帧的生产条件更窄）→ §4.8 Round-8 ②
- **「停止」这个动词有两种寻址方式，只有一种接了完整的那条线** —— 按 session key 停（channel `/stop`）走 `cancel_session`，它会 walk 委派子运行；按 run id 停（Panel `chat.abort` / TUI `/stop` / `aleph chat abort`）落在 `AgentRunManager::cancel_run`，而它调的是 `execution_adapter.cancel(run_id)` —— **只点一个令牌**。leader 停了，它 `task_manage`/delegate/后台子代理派出去的成员运行继续跑、继续烧 token，且此后没有任何 surface 够得到它们——正是那趟 walk 被写出来堵的 detach 泄漏。**而 `cancel_session` 自己的 doc、FEATURE_LOCATOR §4.8、CLAUDE.md 打磨话术三处都写着 `chat.abort` 已经走它**（「同一事实的两份表述」再来一次，这回是三份，代码那份才是真的）。修法是加一个**查询**而不是第三个动词（`ExecutionAdapter::session_of_run`，默认 `None`），让 run-id 那张脸解析出会话后走**同一个** `cancel_session`；引擎不握着这个 run（已结束 / 仍在车道里）时回退原语，那条路同时是 `cancel_queued_run` 需要的 → §4.8 Round-8 ③
- **「提前返回的快路径」会静默吞掉请求上除它自己之外的一切** —— 判据不是「这条路径会不会崩」而是「它跳过了哪些本该发生的解析」；新增 per-request 指令字段必须在 `steering.rs::carries_more_than_text` 登记。**一个方向被想到、反方向没有**是这类缺陷最常见的形状 → §4.8
- **`devices` 是 panel 与 cluster 共用的一张表，`device_id` 两边都是对端自报的** —— 任何「按 id 认领一行」的新路径必须先拒掉另一半命名空间（判据单一源 `PANEL_DEVICE_TYPE`）→ [SECURITY.md#auth-ux](docs/reference/SECURITY.md#auth-ux) · `src/gateway/CLAUDE.md`
- **团队群聊投影前必须先按当前 `chat.team_id` 作用域** —— 订阅是 `team.*` 通配，否则后台团队的气泡挤进任意会话 → §4.5
- **新增 `read_*` 一类只读 RPC 记得进 `gateway/lane.rs::override_for`** —— 后缀启发式不认它就落 Mutate 车道被幂等键守卫拒掉 → §6.8
- **「谁拥有」回答不了「谁能看」** —— 共享房间的 `owner_user_id` 记的是**创建者**，只裁 owner-only 动词；可见性判据是名册（`projects::roster::is_member`，经 `visibility::project_visible` / `session_visible_to` 到达）。凡给一张表加了共享语义，`owner` 列的含义要重新问一遍——拿它答 can-see 就是 P2 修掉的那类 bug 复发 → §5.22
- **每个可见性谓词都欠一个显式 actor 孪生，因为工具面取不到 task-local** —— `visible_owner_filter()` 读 `CALLER_USER`，而它在 spawn 出的 run 里恒 `None`，**每一次工具调用都在里面**：照文档接现成谓词的工具作者拿到的是**静默恒真**。孪生是 `session_visible_to` / `partition_visible_to` / `project_visible_to`。新工具碰 per-user/per-room 数据时先问「这个谓词的 actor 从哪来」→ §5.22 round 2 ⑤
- **⚠️ 但那个 actor 不是 `ambient_owner()` —— 房间里它是房主，不是说话人** —— `scope::ambient_owner()` 答的是「**这一行属于谁**」（房间会话行的 owner 列记创建者，对每个成员是同一个人，这正是共享分区的机制）；答「**谁在问**」必须用 `visibility::ambient_actor()`（房间取说话人，非房间与前者逐字节相同）。**四个谓词各自独立踩了它**（`memory_search` 分区闸 / `session_list` / `session_send` / `ScopedTeamStore`），共同成因是仓里三处文档把这句话**写反了**——判据：写 `ambient_owner()` 之前先说出你答的是哪一问，源码级 census `visibility::AMBIENT_OWNER_CENSUS` 会逼你说 → §5.22 round 3 ①
- **写路径合成了分区，读路径必须用同一个函数合成** —— 每个记忆/笔记写者都过 `project_scope::session_write_id`，九个读者读裸 persona `main` ⇒「列出我的笔记」对刚写下的笔记答「没有」。**这不是多用户专属**：loopback Panel 会话就是 `Personal(u-owner)`，出厂分区本来就是 `main__u-owner`。工具面唯一答案 `caller_memory_partition`（房间里必须拒绝的孪生是 `caller_profile_partition`）。⚠️ **一次扫荡覆盖的是「面」不是「族」**：那轮扫的是工具面 dispatch 臂，**网关 RPC 面一个都没扫到**，于是 2026-08-21 真机一开 Panel 就撞见两个——`memory.trace` 与 `memory.listFacts`/`stats`/图谱。判据：读到「这一族修过了」，先问**修的是哪一面**，再去数这个动词还有几张脸。⚠️ **而「数」不能照抄上一轮数出来的那个数**：网关面被记成三个 handler，真数是**八个**——漏掉的四个里有 raw 那一整柱（`memory.search`）和**检索透视自己**（`memory.retrieve_with_trace`），所以上一轮记下的「透视结构上答不了」并不是夹具的局限，是同一个缺陷的另一张脸。现单一源 `gateway::handlers::memory_scope::{read_partitions, primary_read_partition}` + 源码级 census。三条配套判据：① **组合排在可见性闸之后**（闸判的是调用方真发过来的那个字符串）；② **已组合的 id 必须原样通过**——重新组合产出 `main__u-owner__u-owner` 这个幽灵分区，而改写成调用者自己的分区会把一次**拒绝**降级成一次**静默替换**；③ **表达不了并集的那个面要单独承认**（`NoteNodeDto` 无 `agent_id` ⇒ 星图走单分区，而统计卡的图计数必须跟着它走，否则卡片描述的是一张没人画的图）→ §5.22 round 3 ② · §6.7
- **归属跨 `tokio::spawn` 要用 `scope::CarriedAttribution`，而且只重建 task-local 不够** —— `run_loop` 只从 `request.metadata` 重建 run 的 scope、`ensure_session_under_request_scope` 也只读它，所以**两半都要写**（task-local + `stamp_metadata`）。`caller_role` 同在这个载体里，因为它在同一处以**相反方向**失效：`role_is_operator(None)` 按设计为真 ⇒ 丢了角色是 fail-**OPEN**（跳过 exec-tier 天花板与 operator-tool 闸）。census `scope_stamping_producers_are_all_accounted_for` → §5.22 round 3 ③④
- **写在 doc comment 里的 census，会在它点名的那个东西上失效** —— `teams/scoped.rs` 点名 `task_list` 欠 retain，`task_list` 一整轮没有；`run_loop` 的 producer 名单把 teams dispatcher 列为「归属就在 metadata 里」而它一个键都没写。**census 一律写成会按名字红掉的测试**，不写成散文 → §5.22 round 3 ⑤
- **跨 crate 的 wire 契约，两边各持一份形状就会互相抵消** —— 服务端按**移除**窄化 + 客户端 DTO 缺一个 `#[serde(default)]` = **整份**响应解不出来，而 serde 的报错不是拒绝形状，连「被拒了」都说不出口，于是同一轮的两半互相抵消、两边测试全绿。key set 放进两边都依赖的那个 crate（`aleph_protocol::tool_permissions`），两边**各写一条对账** → §5.22 round 3 ⑥
- **「谁能看」是名册，所以删掉名册不是删除，是对所有人隐藏** —— 房间可见性谓词**就是** `roster::is_member`，`projects.remove` 一次写删掉行与名册 ⇒ transcript 对活着的每个 principal（含 operator 与创建者）不可达，而分区照旧被夜间维护。凡「删掉这一行」的动词都要问**谁还指得到它引用的东西**；答不上就让这个动词对那类对象不可用（比造一条级联便宜，也不制造第二个真源）→ §5.22 round 3 ⑧
- **谓词改了、下推的过滤器没改，症状是「能进去但列表里没有」** —— 寻址面的 `visibility::session_visible`（task-local）与列表面的 `visibility::session_visible_to`（显式 actor，两个 backend 的 `SessionFilter::owner_visible_to` 都经它）是同一个判据的两张脸，必须同批改；grep 内存谓词找齐所有下推点，只改一半的那半是静默的。⚠️ **`..Default::default()` 会把这个字段留成 `None` = 全体 owner**：`session_list` 工具就是这么漏的，而 RPC 孪生一直设着它。⚠️ **刻意没有 `SessionFilter::visible_scope_ids`**：房间可见性不下推成 SQL 列表，而是由两个 backend 共用的内存谓词裁决——想加 SQL 下推请先读 `session_visible_to` 的 doc → §5.22
- **投影必须由真源在自己的写锁里发布** —— `projects::roster` 是 `project_members` 的进程内投影，发布点在 store 的写锁**内**（变更 + 快照 + 发布同一个 `with_conn` 闭包，`republish_roster_locked`）。「写完再发」的两次取锁不是原子的：并发写会按提交的**反序**发布，输的那次把已删成员复活到下一次名册写入为止，而 `is_member` 是房间授权的**全部**判据 ⇒ fail-open。第二个写入者就是第二个真源，CLI 要改名册必须走 IPC 而不是直开数据库 → §5.22
- **多设备共享的事实不能住在 `localStorage`** —— 判据是**这个值对第二台设备还成立吗**；「房间用哪个会话」曾按 `project_id` 存在每个浏览器里，于是第二个成员进房什么也没找到、开了自己的会话，两人共享记忆分区与工作目录却**永不同框**（任何界面、任何刷新都看不见对方）。真源是 `projects.current_session_key` + `projects.room_session` 的原子认领 → §5.22
- **上一条的孪生：一份持久状态，如果展示面只由事件流喂养，那它看到的是近似值** —— 前者问「这一帧从哪产」，这条问「**新连上来的那个客户端从哪读**」。执行清单是磁盘上的 markdown（模型 / `<execution_plan>` / 停机守卫共读的真源），而 Panel 的 todo 条只有 live 帧、`RunSummary` 与自己内存里的快照三个生产者 ⇒ 刷新 / 第二个标签页 / 第二台设备 / 房间队友只能靠**重放有损的 `agent_trace` 镜像**重建，且重放路径连 live 路径那条 `settle_plan` 对账都没有；正在跑的那一轮更是连 trace 都还没落盘。判据：**这个东西有持久行吗？有的话，attach 时读的是那一行还是一串帧？** 修法是挂到客户端 attach 时本就要发的那个响应上（`chat.history` 的 `active_run` 是同一个论证的先例，连授权都是免费的），并让真源**最后说话**（排在重放之后）；`None` 三义（旧 core / 没有 / 解析失败）一律读作「我没有答案」而非「没有」→ §3.13
- **一个"实时显示"的帧，产地必须是那个事实变成持久行的地方** —— 挂在更早的生命周期节点上（因为那里刚好有个现成的热帧）就会渲染出**重载后消失**的东西。`RunAccepted` 早于 harness seed 用户消息：既覆盖不到「发了 `RunAccepted` 却从不落用户行」的快路径斜杠命令，携带的 `request.input` 也不等于最终落库的文本（`/moa X` 先被改写、多模态另走一路）。单一源是 `session_projector::project_event` 的追加点——四个产地（主路径 `session_seed` / 快路径 / simple 引擎 / steering）全经过它，且只有那里"要宣布的字节"**按构造**等于 `chat.history` 之后会回放的字节。⚠️ **配套两条，各自独立**：① **去重自己的回声不能建立在一个还在飞的响应上**——`start_run` 先 `tokio::spawn` 再返回 run_id，帧能跑赢那个响应，所以 `is_own_run`（在回合**末**成立）在回合**首**是竞态；按身份判（author ≠ me）没有这个窗口，代价是同一个人的第二个标签页不受益。② **帧必然晚于 `RunAccepted` 到达**，所以要**插到那个空占位气泡之前**，否则提问排在答案下面；判据是「这个占位还没产出任何东西吗」——有输出之后再到的就该追加，那正是 steering，位置恰好相反 → §6.1

### 5. 记忆 · 笔记（`src/memory/` `src/note/`）

> 这一层坏掉时不崩溃、不报错、测试全绿，唯一表现是**模型"想不起来"**。

- **`~/.aleph` 下的任何路径，写它的和读它的必须是同一个函数** —— 不是"看起来一样的两段代码"；`dirs::home_dir()` 出现在 `src/` 里基本都是这个 bug 的候选（`ALEPH_HOME` 未设时逐字节相同，本机永远测不出来）。守卫 `utils::paths::tests::no_hand_rolled_aleph_home_outside_the_allowlist` 是**源码级**的 → §2.16 §5.8
- **`MEMORY.md` 是一种格式不是一篇文档**（`\n§\n` 分隔）—— 当散文写会让 curated store 进 **legacy 静默模式**（`remember(add)` 恒 `LegacyBlocked`，占位散文被当记忆事实注入 prompt）；新 agent 种子必须是**空字符串** → §2.16
- **内容进 `<CuratedMemory>` 前必须 `escape_xml`**（热区是 Stable 层，一条 `</CuratedMemory>` 在此后每次请求里闭合信封）；**凡以 chars 命名的阈值就得数 chars**——预算如此，`min_user_chars` 这类**门**同样如此（数 bytes ⇒ CJK 拿 3 倍额度，恰恰是门要挡的那种会话花掉一次 LLM 调用）→ §2.16
- **只在「跑到底」时才重写的文件，没有任何过期机制** —— 每一条早返回都让上一份原样留着，而它可能坐在常开区。**让它说出自己的捕获日期只是让模型知道它多老，不阻止它到达**；缺的那道闸就是**已经被解析出来的那个日期**。且「读不出日期」必须判**过期**——把「判断不了」读成「新鲜」正好让无界那一档从为它设的闸里走过去 → §2.16
- **一条只覆盖一条轨的台账，等于把它承诺的那个问题只在那条轨上回答** —— `memory_trace(kind:"write_decision")` 对用户承诺「为什么没记住」，而生产者一度只有 `remember`：热区为真、往下一级静默为空。**写入工具的规矩写在 prompt 里而不写在代码里，同理**（rung 2 的「同一条修正别登记两次」曾只是一句话，rung 1 从第一天就结构性拒绝逐字重复）→ §2.16
- **写进 frontmatter 的模型输出必须过 `yaml_scalar`** —— 一条解析失败让整篇笔记从语料里**永久消失**（三处都只 `debug!`）→ §2.5
- **召回信号按笔记的真实归属命名空间记账** —— 别用 `agent_ids.first()` 当代表标签（`NoteDecay.access_weight` 读的是 scoped id）；**对 prompt 没有贡献的命中不该赚到信号** → §2.5
- **排序即预算优先级** —— `hydrate` 严格按序扣槽位，"常开地板"必须排在前面才钉得住；注释写 "can never be dropped" 不构成保证 → §2.5
- **「保真」只在你列举过的字段上成立** —— frontmatter 白名单外的 key（`cssclass`/`publish`/自定义）被第一次写路径永久销毁；直通用 `extra_frontmatter`（`BTreeMap` ⇒ 发射序确定）。**刻意不用 `#[serde(flatten)]`**：它把结构体路由进 serde 的 buffered 表示，会改变自定义 `deserialize_with` 看到原生 YAML 值时的行为——在一条已有回归测试钉住的解析路径上做静默行为变更，比原 bug 更贵 → §2.9
- **「每次调用都追加一行」在每夜调一次的调用方那里就是缓慢的正文腐蚀** —— `KnowledgeNote::add_links` 每次新起一行 `Related:`，而夜间织入每夜调它一次。问「调用方节奏 × 副作用 = 三十天后什么样」；footer 建模成**位置**不是**事件**；**无需改动时不要重写**（churn `content_hash` → 白花一次重嵌）→ §2.9
- **降级的理由如果不取决于是哪一半坏了，只守一半就是没守** —— 把理由写成一句话，逐个失败模式问「这句话对它成立吗」 → [RETRIEVAL.md](docs/reference/memory/RETRIEVAL.md)
- **「不支持」一头静默一头致命，两个症状合起来会读成第三件事** —— 「受支持的 X 集合」出现第二份拷贝就该收敛成单一源（`EMBEDDING_DIM_TABLES`）→ [RETRIEVAL.md](docs/reference/memory/RETRIEVAL.md)
- **一个吞掉失败的操作，其「后面会有人补」的理由必须能指出那个补救者** —— 何时跑 / 怎么知道补哪些 / 跑一次多贵；三问答不上一个，这个吞就是丢（修法：按内容哈希记账 `embedded_hash`，`""` 读作陈旧）→ [RETRIEVAL.md](docs/reference/memory/RETRIEVAL.md)
- **复用现成读取器前先问它凭什么知道该去哪读** —— 重复的是「怎么拼路径」该收敛，「根在哪」属于各自的所有者，收敛过去就是制造断线
- **markdown 是真源这句话，得有人在运行时执行它** —— 每个 agent 目录都被写入 `.obsidian/` vault 配置（＝明说"去 Obsidian 里编辑"），所以外部编辑/删除必须有监视器回流（`notes/watcher.rs`）；**监视根先 `canonicalize`**，通知回报的是规范路径（macOS `/var`→`/private/var`），拿未解析的根 `strip_prefix` 会把整个 vault 判成"不是笔记"——在跑、什么都不做、零症状。⚠️ **同一条规则对比较的另一边同样成立，而那一边的失效方向是反的**：`extension/watcher.rs` 的 `runtime_data_dirs` **排除**表拼法未解析 ⇒ `starts_with` 一条都不匹配、整张表是死的，症状不是"什么都不做"而是"什么都放行"（画布每次拖拽都触发全量 `extension.reloaded` + `tools.changed`）。判据两句：① **归一化要落在同一个函数上、两边都过一遍**（这里是既有的 `canonicalize_best_effort`）；② **只归一化一边只修好一个平台**——FSEvents 替你解析，inotify/Windows 按所给的根拼，所以监视根也要归一。命中条件是"home 路径上有软链"，默认装机不中、**每一次 QA 运行都中**（`$TMPDIR` 在 `/var` 下），这就是它四轮没被发现的原因。**动作由文件系统当前状态决定，不由事件类型**；非 `NotFound` 的 stat 错误必须跳过（另一条分支是删除）→ §2.5
- **重索引 ≠ 重嵌** —— 不经 `write_note*` 直接改写笔记字节的写者（`note_lint`/`note_drift`/面板编辑/watcher）必须走会跑 `finalize_side_effects` 的那条路，否则向量永远描述改写前的正文，且只在下次开机以一个 `stale_vectors` 计数露头 → §2.5
- **「模式」标签要描述发生过的事，不是配置** —— 报告"我用了哪条路径"的字段必须由逐腿候选数派生（`"hybrid"` 曾包括向量腿颗粒无收那种）
- **做梦层：`record_X()` 之后立刻 `advance()` 的两段式协议里，任何读"当前缓冲"的谓词在生产中恒空** —— 单测用生产从不使用的调用序，全绿 → §2.8

### 6. 桌面四肢（`desktop/`）

- **Windows 桌面坐标 = 进程属性，不是 API 属性** —— DPI-unaware 进程读到的 `GetWindowRect`/`GetCursorPos`/`SendInput`/UIA 矩形全被虚拟化，而截图不被；latch 有**两个调用点**（`WindowsPlatform::new()` 与 `NativeScreen::new()`）。`DisplayInfo.scale_factor` 的语义是「本进程几何空间里 1 单位 = 几个物理像素」，**不是显示器 DPI 比** → §7.1
- **DPI 只统一了单位，还有两处「读写不同源」** —— 指针走 `desktop/shared/src/win_input.rs`（不是 `enigo` 的 Abs，它按主屏归一化且不发 VIRTUALDESK ⇒ 副屏每次点击都落主屏）；窗口 bounds 是 DWM 扩展帧而 `SetWindowPos` 吃原始矩形（`win_window::FramePadding` 补偿）→ §7.1
- **「什么算一个窗口」只在 `desktop/shared/src/win_window.rs` 回答一次**；桌面层 shell-out 走 `script_exec::hidden_command`；**UIA 客户端工作全进程串行**（`ax.rs::uia_gate`，否则裸 `E_FAIL` 看起来像"偶发抽风"）→ §7.1
- **macOS 上 `aleph-server` 不是一个 app** —— 凡要「主 run loop」或「前台身份」的 AppKit 能力全不成立**且都报成功**（`NSEvent` 全局监视器永不触发 ⇒ 全局按键只能靠 `escape_listener.rs` 的 listen-only `CGEventTap`；`activate`/`AXFrontmost` 拿不到前台）。加这类能力前先假设它不成立、再实测 → §7.2
- **macOS「一个 app 名字是什么意思」只有一个答案**（`desktop/shared/src/macos/app.rs`）—— `terminate()` 回 `true` 只表示 Apple Event 送到了，**必须轮询 `isTerminated` 验证** → §7.2
- **Linux 桌面实现不在 `desktop/linux/`** —— 窗口/剪贴板/应用/输入/截图/OCR 全在 `desktop/shared/`（`action/window_linux/`、`action/wayland_input.rs`、`linux/{session,clipboard,app,proc}.rs`）；会话类型只有一个答案 `shared/src/linux/session.rs` → §7.1-§7.4
- **凡等桌面服务的 shell-out 都要带死线** —— 判据是**这条命令在等别的进程还是在算**（`xclip -o` 等持有选区的应用、`notify-send` 等 D-Bus、`pactl` 等声音服务器）；**自己写轮询也不行**（只 `try_wait` 不排空管道 ⇒ 64 KiB 后双向死锁）→ §7.1-§7.4
- **AT-SPI：连接共享（424 ms/次）、句柄不共享，但共享连接必须先探活再交出去**（换 runtime 复用不报错、直接永远等）；**`CacheItem` 的字段名与它装的东西不符**（`short_name` 才是 name），照"看起来对"的字段读会让标题命中率从 47/70 掉到 3/70 而**单测全绿**（只有 `tests/atspi_live.rs` 抓得到）→ §7.1-§7.4
- **AX 三件事都必须可见** —— 节点预算是**协议的**（`ax::DEFAULT_MAX_NODES`，结果带 `node_count`/`truncated`/`tree_truncated`）；`query_focused` 的 `pid` 是问题本身不是过滤器；`AXUIElementSetMessagingTimeout` 每个进入遍历的句柄都要设（不被子元素继承）→ §7.1
- **密码框判据是两条腿**（原生 role + 共享词表 `desktop/shared/src/ax_secure.rs`），且**必须在读值之前**判 → §7.1-§7.4
- **「这个动词没有定向对应物」不是让它绕过输入闸的理由** —— 任何合成输入的新动词必须同时：进 `native.rs::is_input_action`、按 rail 分发、结果里报 `delivery`；跨调用留住物理资源的动词必须**在同一条轨上释放** → §7.3
- **「声明了死线」≠「花了死线」** —— 单一源 `methods::suggested_timeout_ms` → `bridge::client::rpc_timeout_for`；**任何"推荐值"常量加进协议时，同一笔改动里必须有消费者** → [DESKTOP_BRIDGE.md](docs/reference/DESKTOP_BRIDGE.md)

### 7. Panel · 前端（`interfaces/webchat/`）

- **一个机制住在一个有挂载条件的组件里，等于对那个条件之外的每一个形态关掉了它——而"关掉"和"这个形态用不着它"在代码里逐字相同** —— 上一条讲状态放错容器，这条讲**机制**放错容器。多端共享一条线程的**全部**客户端机件（对话登记 / 帧路由 / 重连基线复位 / 结算 / 重接）都写在 `ChatSidebar` 里，而它挂在 `not_phone` 门后 ⇒ phone 的 `SessionMap` 一个对话都没有、`resolve_target` 三步全落空、**每一帧在碰 `ChatState` 之前就被 return**：没有助手气泡、没有工具行、没有最终答案、**没有任何日志**，而 `FormFactor::Phone` 是视口带宽、iOS Panel 壳恒在带内 ⇒ 那条产品线从来没有流式过。判据三句：① 写一个**跨形态**的机制时先问**它挂在谁的生命周期上**，答案是某个组件就去数那个组件有几个挂载条件（这里的正解是 app root，两个形态白继承一份）；② **这类缺陷唯一的搜索命中往往是一句说反了的注释**——`resolve_target` 第 3 步逐字写着"这让任何不注册对话的 surface 照常工作"，而第 3 步自己也要 `active_conv()`，那句话从写下之日起就是假的；③ 修法是**让缺席的那一方补上登记**，不是给解析器加一个兜底——凭空造出目标的兜底正是"前台劫持"（外来 run 的整轮渲染进用户正在读的对话，下一条消息发去别人的会话）本身 → §6.9 round-2
- **把一个 fail-open 的路由兜底收窄成 fail-closed 之前，先数一遍是谁在教它** —— 收窄的第一个受害者通常是**教学帧自己**，因为它走同一条闸。TUI 的 `frame_belongs_here` 对未学过的 run id 一律保留，而唯一教它 `run_id → session_key` 的就是那个 run 的 `RunAccepted`；一旦这道闸能回答"不是我的"，教学帧当场被丢，此后这一屏**再也学不到任何非自己发起的 run**，连后来切到那个 run 真正的会话都认不出来——而症状是"终于不串台了"，看起来像修好了。判据两句：① 收窄前列出**这个判断的知识是从哪几帧来的**，那几帧要显式豁免（它们通常自带归属，本来就是自证的）；② 顺带检查**有界容器的淘汰**——「未知＝保留」的年代淘汰无害，「未知＝丢弃」的年代它会把用户正在看的那一轮静默踢出自己的 transcript，所以活跃那一个要排在查表**之前**回答 → §6.9 round-2 ④
- **「按会话的状态」住进单例组件 ＝ 切页签就串味** —— 判据：**这份状态在用户切到另一个对话后还成立吗**？不成立就进 `SessionSnapshot`，边沿按 `ConvId` 记（单一源 `shared_ui_logic::state::composer_queue::was_busy_across_switch`）→ §4.7
- **但「进了 `SessionSnapshot`」只是必要条件** —— `activate`（快照往返）**紧接着一行** `clear_session()`，给快照加字段时必须走一遍「恢复之后还会跑什么」；**「A 之后紧跟 B」时算数的是 B**，两段代码各自都对，只有真机看得出来 → §4.7
- **页面的首次加载不必自己等 socket，但「重连后刷新」必须自己写** —— 两个不同的问题，别用一个答案糊过去。「别问太早」由 `DashboardState::rpc_call` 有界等待兜底（见 §0），所以组件体里的裸 `spawn_local` 不再是冷加载 bug；「再问一次」没有通用 helper 也**刻意不做**一个——每个需要重跑的页面都还要跟踪**别的**信号（`include_archived` / `ws_id` / "agent 列表还空着"），只认 `is_connected` 的 helper 比它替换掉的四行 `Effect` 能力更弱、会零消费者（R10）。范本：`WorkspacesView`（跟 `include_archived`）与 `canvas::CanvasView`（跟"列表还空着"，因为重跑会丢掉用户的选择）
- **别再造"写一个信号、指望别处排空"的预填通道** —— 多平台下必然漏一个消费者且**零报错**；草稿只有一个家 `ChatState.draft`，唯一入口 `seed_draft`（合并不覆盖，这个 composer 没有 undo）→ §4.8
- **交付物 ≠ 聊天记录** —— `artifact_publish` 的成品 vs `session.export_html` 的 transcript；什么算成品 **100% 归模型判断（R7）**；导出文档**零 `<script>` 是硬约束**（`src/export/page.rs`）→ §6.8
- **右栏默认是收起的**（`LayoutMode::ChatOnly`）—— 长在面板里的提示在那个状态下等于不存在；徽标必须数**面板真正装的东西**；「一行能点开什么」的谓词 offer 侧与 serve 侧必须读同一份（`PreviewTarget::for_item` ↔ `is_previewable_text`）→ §6.8
- **一帧带着自己的归属到达，"我认不出它"就必须是丢弃，不能是"给我正在看的那个"** —— `resolve_target` 对本客户端没有 route 的 `run_accepted` 回退 `active_conv()`，于是**第二个标签页 / 房间队友 / CLI / 任意 channel / 每一次 cron tick** 的整段回合渲染进用户当前的对话，还把那个 tab 的 `session_key` 覆写掉（下一条消息发去别人的会话）。帧一直带着 `session_key`、`conv_for_session_key` 一直存在——**两者从未被连起来**。判据两句：① 回退到"前台/默认/当前"之前先问**这个东西自报了归属吗**；② 收窄时只拒**能被证明属于别处的**（两侧都已知且不同），别拒"我算不出来"——后者会连带杀掉新会话第一回合和老 core → §6.9 ①
- **反向索引的写者只有一个时，"另一条路开的对象"整类不可寻址** —— `meta.session_key` 的唯一写者是 `bind_run`（发送路径），所以**只读打开**的对话没有身份，**三个**读这张表的判断同时哑掉（外来 run 路由 / 红点归属 / 重选会话复用 tab——A→B→A 开三个标签页）。判据：给一张反向索引加读者前先 grep 它的**写者有几个、覆盖哪几条创建路径** → §6.9 ①
- **服务端序号是"一条连接内"的事实，把它当客户端寿命内的基线就会永久静默丢帧** —— `set_server_running` 丢 `seq <= server_seq`，而重启后的 core 从 0 重新编号 ⇒ 新进程的**每一帧**都被丢弃，红点冻结在旧进程死亡那一刻、无任何报错，冷启动种子也救不了（它只在 `server_seq == 0` 生效且只跑一次）。判据：任何按服务端 seq/revision 单调丢弃的客户端状态，都欠一条**每次握手成功就重置基线**的线；重置时**别顺手清空被守护的那份状态**（清空读起来是"全都结束了"，而那在长跑 run 跨过掉线时正好是反的）→ §6.9 ④
- **只由终局帧驱动的客户端结算，跨不过连接中断** —— `settle_run` 只由 `run_complete`/`run_error` 驱动 ⇒ core 重启前那一轮永不结算，composer 卡在 Stop、红点常亮到刷新页面为止。修法是重连后拿服务端的权威集对账；**结算不等于判决**（可能已完成 / 已被 resume 成新 run / 已死），用 `settle_abandoned_run` 这种"停止认为它在飞"的形状，不用 `complete_run`/`fail_run`；**算不出归属的一律跳过而不是结算** → §6.9 ④。⚠️ **「拿权威集对账」这半句是那次的修法，不是这一类的修法**：把它当成结论搬到第二个面上，会撞见两种各自足以让它整个不成立的情况，而 `/btw` overlay 一次撞见两种——那个集合**是另一个面独有的**（`stream.running_set_changed` 在 `PANEL_ONLY_STREAM_METHODS` 里），且它**按会话键控**而侧问跑在一个客户端按设计算不出名字的派生会话上，所以就算拿到了也答不了它。判据两句：① 搬一个修法之前先问**那个权威集答的是哪一问、用什么键**，再问**我这个面看不看得见它**；② 答案是"看不见/答不了"时，去数**这个客户端手里还剩哪个 id，以及有没有一个读接受那个 id**（这里是 run id ↔ `agent.status`，一次往返，且只在真有东西在飞时发）。⚠️ 配套：那次对账只需要"在不在集合里"两态，逐 id 问则是**四态**，第三第四态最容易被折掉——**「服务端答了『没这条记录』」与「我根本没问到」是两件事**（前者 `CliError::Rpc`，后者传输层：只有前者能结算），而**一个这个客户端不认识的状态词不许并进"还在跑"**（转圈永不停不可恢复，提前收摊只是少看一段还在跑的输出）→ §4.14 ⑨b
- **守卫证明过的前提，在 body 里不是证明** —— `<Show when=…>` 与它的 body 是**两个反应式作用域**：信号被清空时 body 可以在守卫重算并卸载它**之前**先跑一次新值，于是一句 `expect("visible implies Some")` 把一次再普通不过的调度顺序变成**整页崩溃**（`todo_panel.rs` 每当计划跑到 100%：`settle_plan` 先 Show、**紧接着一条语句** archive 做 `set(None)`）。修法不是把 `expect` 换成默认值，而是**取消守卫/body 的分裂**——单次读 + `Option` 视图（`None` 天然什么都不渲染），body 自己判定自己的可见性。⚠️ 这是竞态：**同一条路径在另一次观察里不崩**，所以"修完跑一次没崩"证明力有限，保证来自崩溃点不再存在 → §3.13 ⑦
- **两个客户端拿到同一份数据 ≠ 落在同一个状态** —— 上一条的孪生，也是「把持久事实喂给冷客户端」这类连线的通用收尾判据。把 `plan` 送到冷加载路径之后，落地那一行手搓了 `apply_plan_update(projection(..))`，而"结算"是**两步**（对账 + **把已完成的沉进 transcript**）：少了第二步，冷客户端顶着一条 100% 的清单，而 live 客户端那边它早已沉成一枚归档胶囊——**同一个对话的两个客户端看到不同的东西，正是这条连线本来要消灭的缺陷**，且下一回合会把它再沉一次（重复胶囊）。判据两句：① 接好数据后再问**"live 客户端此刻停在哪个状态"**，冷路径要落在那个状态上；② **一个两步动作，别把第一步单独 `pub` 出去**——投影暴露在外就是在邀请下一个人只做一半（`plan_settlement` 现 `pub(super)`，唯一入口 `ChatState::settle_plan`）→ §3.13 ⑧
- **一个「无界计数器 + 每个读者各夹一次」的高亮，不会错触发——所以没有任何测试会红，症状出现在反方向的那个键上** —— 命令面板的 ↓ 撞 `saturating_add(1)`、渲染与 Enter 各自 `min(len-1)`，两者**始终一致**（都夹到最后一行），于是「亮的不是触发的」这个常见判据在这里恒为假；真正的缺陷是 ↓ 按过头之后 **↑ 要按同样多次才动一格**，读起来像键坏了而不像 bug。⚠️ 这条同时是**判据本身会被抄错**的例子：`preset_picker` 的模块 doc 拿「两处 clamp 互相矛盾」论证自己为什么共享一个夹取函数，而那句话对被它引用的那个文件从来不成立——**引用另一个模块的缺陷来论证自己的设计时，去那个模块确认它现在（和当初）是不是那样坏的**。修法是让高亮**只在写侧**经 `picker_nav::step_highlight` 移动（四个 surface 共用），读侧的夹取保留但已是同一个函数
- **一个「下面还有」的渐隐必须是条件性的，常驻的那种在列表已经到底时仍在压暗最后一行** —— 判据不是「加不加渐隐」而是**它说的话什么时候是假的**：读者抓到它撒谎一次就此不再读它。故 `.aleph-scroll-more` 由 `picker_nav::publish_more_below` 逐帧测量后挂载/摘除，且 `has_more_below` **模块私有**——够到它就意味着你自己读了几何量，而在该测量的那个 deferred 回调里读几何量正是下一条要防的 panic。⚠️ 配套：`prefers-reduced-transparency` 下**不能照抄 `.chat-scroll-fade` 的 null 掉**——那个是装饰（内容溶进 chrome 带），这个**携带事实**，要换渲染（不透明底边）而不是删信息
- **`request_animation_frame` 回调里的 `get_untracked()` 与 `spawn_local` 里 `.await` 之后的那个是同一类 panic，而 `disposed_reads` 的块识别器看不见它** —— rAF 回调晚一帧执行，这一帧足够组件卸载（换路由/关抽屉），`NodeRef::get_untracked` 会 unwrap ⇒ 整页掉进恢复覆盖层。**且这里没有文本规则可写**：回调常是别处定义的具名闭包，扫描器跟不过去。结构性修法是**让这件事只有一种拼写**（测量收进 `publish_more_below`，谓词私有），不是加一条只认得内联 `move ||` 形式的半盲守卫——那种守卫会给你正在用的那个形状发合格证
- **一条按目录划定范围的守卫，回答的是没人问过的问题——用户看到的是一块屏幕，不是一条路径** —— `no_file_under_platform_phone_hardcodes_chinese_copy` 在 2026-08-18 是绿的，而手机上的 `/settings/appearance` 正渲染着八个中文词：它们来自 `crate::appearance` 的 `ThemeMode::label()` 一族，一个 `use` 之外、一层目录之上的共享模块。判据换成**可达性**（从 `platform/phone/` 的 `use crate::…` 走一跳、模块粒度，现 `no_module_a_phone_screen_reaches_hardcodes_chinese_copy`），代价是三个文件；走两跳就覆盖大半个 crate，那就退化成一条带着永不缩短的豁免清单的全 crate 规则。⚠️ 两个方向的近似都要在 doc 里写明：**两跳之外仍然看不见**（已声明的边界），而模块粒度会**过度包含**（`platform/phone/chat/history.rs` 只 import 了 `components/chat_sidebar.rs` 的一个函数，整个模块就算可达）——后者是安全方向，代价是多翻译一句，而按 item 粒度精确解析的失败模式是**静默漏掉**
- **一个"速率"设定读起来像承诺，但它约束的是导数不是结果——而用户等的是结果** —— 而且这一族的**修法本身有个陷阱**：把界加在导数上仍然不收敛。打字机按 `dt * cps` 平铺，滞后量是 `backlog / cps`，`backlog` 由模型产出多少决定 ⇒ 默认 200 cps 下六秒产出的 2 500 字要爬 12.5 秒（`run_complete` 早已解锁 composer、spinner 早已消失），10 KB 代码块要爬 50 秒。我的第一版修法是速率地板 `max(cps, backlog / MAX_LAG)`——读起来完全正确，**实测 10 k 字仍要 8.4 秒**：速率随它要排空的积压一起缩小，逼近是指数的、永不到达。界必须落在**被约束的量**上而不是它的导数上（`revealed >= total - cps * MAX_LAG`：一个滑动窗口，超窗的欠账作废不排队），于是两条性质同时成立且都能测——产出期间恒定落后 N 秒，最后一块到达后至多 N 秒收尾。判据三句：① 写下一个速率/配额/限流常量时，说出**它给的是什么保证**，答不上「多久之内一定结束」就是没有保证；② 补界时先问**这个界是加在量上还是加在速度上**，后者在量会变的情形下不收敛；③ **参照实现的复杂度可能是它的单位造成的，不是问题本身要的**——codex 同一问题用了两档齿轮 + 4 个阈值 + 2 个 hold 计时器 + 迟滞，因为它的排队单位是"渲染行"、代价不可计价，只能拿队深/队龄两个代理去逼近一个算不出来的滞后；单位换成字符之后滞后可直接算，整套策略塌成一个 `max`，且单调连续 ⇒ 迟滞不再需要。照抄参照的结构会把它的**单位缺陷**一起抄进来
- **一条在参照实现里"恰好等价"的判据，移植过来就是错的——而它错在你有而对方没有的那个产品面上** —— hermes 的吸底重挂判据是 `last.role === "user"`，在单用户桌面应用里等价于"我发了消息"；Aleph 移植时泛化成"`role == "user"` 的行数变多了"，而 Aleph 有房间：队友发言推的正是 `role: "user"` 的行 ⇒ **别人打字把我的视口拽到底**。同一个代理还有第二种误读——切会话整份替换消息向量，行数两个方向都会变，切到消息更多的会话读作"发送"、更少的读作"我在往回读时来了新内容"（刚打开的会话上弹假的「↓ 新消息」并停在上一份 transcript 的滚动位置）。判据两句：① 移植一条判据时，先问**它在对方那里为什么成立**，那个理由通常是对方**没有**的某个东西（这里是"只有一个人能追加用户行"）；② 别把谓词代理成"从数据里能观察到的某个量变了"，**直接给它一个只有真正的施动者能写的信号**（现 `ChatState::sends`，唯一写者 `push_user_message`；判定纯函数 `shared_ui_logic::state::chat_scroll::scroll_action`，用旧代理做变异证过 RED）。⚠️ 顺带：`next_msg_id` 那种 id 分配器**不是**这个信号——它和 `archive_active_plan` 共享，计划胶囊退场会把它顶上去
- **一个聚合状态如果只由"还有没有在跑"派生，它就会替失败和未知一起发合格证** —— 探索块的头部 ✓ 由 `completed`（＝没有 running 了）驱动，`ExploreEntry` 根本不带状态 ⇒ 四个读里两个失败、一个 settle 成 `unknown` 的块照样报「✓ Explored 4 items」。**`unknown` 尤其贵**：那是 Panel 自己给「结果帧丢了而 run 结束了」起的名字，洗成一个对勾等于把它存在的唯一理由扔掉。判据三句：① "停了"和"成了"是两个谓词，聚合要用后者；② 折叠顺序按**最告警者胜出**且把 `Ok` 排最后（只有全员都说成功才到得了），`Failed` 要压过 `Unknown`（确定的失败比缺帧更可行动）；③ **不认识的状态词一律读作"我无法担保"不读作成功**——那是唯一一个会撒谎的方向，而新 core 加一个状态词是必然会发生的事
- **分组是减法：用在一项上时它只减不增** —— 连读折叠是为了不让十二次读铺满 transcript，套在单次读上，头部写着 `Explored 1 items`（连英文复数都是错的），把这一行唯一的信息（哪个文件？什么查询？）藏进折叠三角，而正上方同一档的非读工具是把参数摊开的。判据：**给一个"聚合/折叠/摘要"写下阈值时，把 n=1 代进去读一遍它的输出**；比不折叠更差就该降级回原形。⚠️ 附带收益值得记：降级同时**缓解了分类器的窄**——`is_explore_tool` 只认得 `file_read` 与 `*search`，Aleph 的只读族还有十几个会打断连读，降级后碎片渲染成信息完整的工具行而不是两个空头部。而那个分类器**刻意不扩**：只读性的真源在 core（`READ_ONLY_TOOLS` + `file_ops`/`inbox_read` 的逐参数 claim），Panel 拿不到也不该为一个装饰性分组去要（新 RPC + 缓存 + 订阅，违 R3/R10），抄一份名单过去就是「列举法只覆盖立法当天的世界」再来一次——且这次连"能不能按名字回答"都不成立
- **一个只由属性表达状态的控件，属性缺席和状态为 false 在 DOM 上逐字节相同** —— 所以「没接上」看起来就是「关着」，而不是坏掉。`.ios-switch` 的开态**全部**来自 `styles/ios.css` 的 `[aria-pressed="true"]`（轨道颜色 + 旋钮 `translateX`），而三个手机端调用点写的是 `attr:aria-pressed=…`：那个前缀是给**组件**转发属性用的，落在原生 `<button>` 上宏收下、什么都不发 ⇒ 真机 `getAttribute('aria-pressed')` 是 `null`，于是每个 provider / embedding 条目 / 模型路由升级开关**无论真实状态一律显示为关**。代价不止观感：点击处理器翻的是 `!enabled`，所以在一个看起来关着的开关上点第一下，**是把一个启用着的 provider 关掉**。判据两句：① **写一个承载状态的属性时，去真机读一次 `getAttribute`**——同一个 crate 里另有三处 `aria-pressed=`（无前缀）一直是对的，差别只有那五个字符；② 守卫只能是**源码级**的（运行时分不出"没设"和"设成了 false"），且要把**样式表那一半**一起钉住——选择器一旦被改成 class，规则就不再描述任何东西却会永远绿（现 `platform/phone/mod.rs::switch_state`，两条：`the_stylesheet_still_keys_the_on_state_off_aria_pressed` + `every_ios_switch_really_sets_aria_pressed`）
- **Panel UI 编译期嵌入二进制**（`rust_embed`）—— 改完看不到效果 = 漏了重编 server；⚠️ **debug 构建例外**：`rust_embed` 在 debug 下从磁盘读，所以只改 Panel 时重跑 wasm+bindgen 即可、无需重编 server（release 才是真嵌入）→ [DESKTOP_SHELL.md](docs/reference/DESKTOP_SHELL.md)

### 8. 配置 · 诊断 · 自管理 · Hook

- **一句关于运行时的承诺，必须由**每一条**到达它的路径执行** —— 先问**这句话是谁执行的**，再问**是不是每条路径都会执行它**（第二问才是这类缺陷的家）；执行点收进唯一写咽喉（`config/live_apply.rs::apply_live_sections`，执行的就是声明用的那张表 `reload_impact.rs::LIVE_SECTIONS`）；**声明要能被降级**——恒真的声明等于没声明 → §5.8
- **一份"该清理什么"的报告，它的错误方向是不对称的：漏报只是少省了点空间，误报是在教用户删掉在用的东西** —— 所以它欠三个位而不是一个：这行**测得了吗**（`NotMeasurable`）、**动得了吗**（`removable`）、**它安静了多久是被测出来的还是根本没有过**（never-used vs idle 必须互斥）。任何一个位缺席，措辞都会替它编一个答案 → §5.24 ①
- **传感器不许创造它测量的东西** —— 诊断 / 审计 / 只读 RPC 一律不能用会建目录的路径 helper（`get_config_dir()` 是纯查找，`get_data_dir()` 不是）→ §5.9
- **「未知」不许读作「健康」** —— 没有死线的检查会把沉默伪装成健康（`doctor` 跑在 agent 回合里 ⇒ 挂住整个回合，唯一症状是沉默）；超时折叠成**指名道姓的 Warning** → §5.9。⚠️ **超时只是其中一种成因**：同一个模块另有九处把「我看不了」答成「那里没有东西」（`read_dir` 失败折成空目录 · `Path::exists()` 对**任何**错误返回 `false` · 一条样本量地板），统统落在 `Finding::ok` 上配一句关于**没人看过的那个世界**的肯定陈述——最强的一句是数据目录读不出来时断言 **"no agent has run"**。修法是**咽喉加一条源码级守卫，不是逐点改**：`diagnostics::check::{Presence, DirListing}` 把第三个答案做成 `Err(Finding)`，而 **`Unknown` 刻意不是 `Presence` 的一个变体**——那样 `matches!(p, Absent)` 会静默错，要把它当缺席花掉必须显式写出来。⚠️ **而过度修复不是安全方向**：把一个**确定**的答案改成 `unknown`，会把一条本来能用的检查降级成一条耸肩的检查，且比原缺陷更难发现，**因为它看起来就是那个修复**。判据是**「这段代码知不知道」**，不是「这条臂里有没有 `Err` 或 `None`」——`Option` 的 `None`、`ErrorKind::NotFound` 是知道；`JoinError`、`PermissionDenied`、以及任何没被读过含义的 `Err(_)` 是不知道，所以每一条 `Err(_) =>` 都欠一次「这个 `Err` 到底可能是什么」的阅读，答案是「只可能是 not-found」时就留着并把理由写进注释 → §5.9
- **一条判决取决于「我在不在那个已经 boot 的进程里」的检查，不属于那个唯一消费者是离线命令的注册表** —— `core/capability-wiring` 在 `default_registry()` 里待过几个 commit，而那是离线 `aleph-server doctor` 建的注册表、那个命令**按定义**是冷进程 ⇒ 冷分支**每次调用、每台机器、永远**开火，`report.ok()` 恒假，那个自陈"给 CI 门禁用"的退出码永远不可能是 0。⚠️ 当时接受它的论证更值得记：「这个退出码本来就不可靠，`core/duplicate-instance` 与 `core/config-parse` 也会翻它」——那句话把**有条件触发**（运维能清掉）和**无条件触发**（谁都清不掉）说成了同一类，**别拿前者给后者背书**。修法**不是**把 severity 降回 `Info`（今天的渲染器下 `Info` 就是"看不见"：`render_human` 从不打 tag、把 `Info` 映射成字面量 `"ok"`、只在 `is_problem()` 时才打 detail，于是一条检查**必须花一个 `Warning` 才能被看见**），是挂专用 builder（`with_capability_wiring_check`，与 `with_runtime_checks` / `with_extension_usage_check` 并列——共同点是**离线入口结构上给不了它们要的东西**）→ §5.9 · 附录 C.4
- **脱敏这类"每条输出都必须过"的闸要下沉到咽喉** —— 那是唯一能替"作者根本没想过凭据"的检查兜住的位置 → §5.9。**先数这个东西有几条腿**：unattended 脱敏曾只包 `TraceSink`，而 run 的输出还从 `EventEmitter` 那条腿出去并被 `OriginFanoutEmitter` 明文投进 Telegram——同一段 final text 在同一个 run 里一边打码一边明文 → §5.1
- **hook 注册了 ≠ hook 会触发** —— 三个各自独立的静默死因（matcher 挂在无 `tool_name` 的事件 / interceptor 挂在只派发 observer 的缝 / consent 仍 pending）；**唯一诊断入口是 `hooks_manage(action='list', only_unreachable=true)`**（运行时视图，不是 `~/.aleph/hooks.json` 那个文件视图）；consent 绑的是**脚本内容**不是命令字符串；**任何工具都不能批准 hook** → §5.10

### 9. 外部集成（MCP · Hub · Provider 路由 · 媒体）

- **MCP 有两个纪元，且纪元是 server 的属性不是请求的属性** —— `connection.rs::probe_era` 探一次闩进 `OnceLock`；判据只有一条（错误码落在 `-32020..=-32099` ⇒ modern）；HTTP 上**必须**把带 JSON-RPC error body 的 4xx 当协议应答返回 → §5.20
- **三个咽喉别绕开** —— 请求只能由 `connection.rs::request()` 造；`Mcp-Method`/`Mcp-Name` 由 `http.rs` 从正要发出的 body 现推；服务端发起的 sampling/elicitation/roots 全走 MRTR。**`resultType` 缺省必须读作 `complete`** → §5.20
- **声明能力＝承诺** —— `can_sample` 的谓词必须是 `handler.has_callback()`（宿主实现 `mcp/sampling_bridge.rs::serve_sampling`，**必须懒解析**，回调要在**任何 transport 启动之前**装上）→ §5.20
- **订阅事件流来建状态的机件，必须在订阅之后对账一次** —— 问「我订阅之前发生的事，谁告诉我？」；boot 恰恰把一切放在订阅之前（曾让**每一台**配好的 MCP server 的工具在每次启动后都进不了注册表，而 `mcp.list` 报 healthy）。顺序必须是先 `subscribe()` 再对账。**纯通知型订阅者不适用** → §5.20
- **"卸载后残留会被清扫兜住"只对改了名字的东西成立** —— 同 id 重装的那一行**从来不是 orphan**，所以孤儿清扫永远看不到它，新装静默继承旧计数与旧 idle 年龄。判据：**这个兜底的触发条件，覆盖得了"换个同名的新东西"吗**。修法接在**咽喉**而不是调用点（MCP 的六个 `remove_server` 调用点共用一个 actor 方法；插件没有咽喉、有三个各自删目录的写者，所以那侧欠一条数站点的 census）。⚠️ **安装路径刻意不清**——原地升级是同一个插件，抹掉历史会把长期服役的报成全新 → §5.24 ③
- **Hub 只消费不策展** —— 目录槽是 **replace 语义**，可疑 artifact 会静默覆盖 last-good ⇒ 校验必须在**任何条目进缓存之前**；**给用户看的数字必须有校验者**（`sha256`/`git_ref` 曾展示却从不校验）；**`installed` 与 `update_available` 是两个不同的真源**且生产者必须落在消费者那条路上 → §5.21
- **一个"展示用"字段在提交前必须能指出渲染它的那一行代码** —— 指不出就是 CUT，不是"以后再接" → §5.21
- **一个「兼容某某格式」的宿主，它的脚手架就是那份格式的可执行文档——而脚手架和解析器是两个作者** —— `aleph plugin init --type nodejs` 写 `kind = "nodejs"`，`PluginKind` 是 `{wasm,mcp,static}` ⇒ `unknown variant` ⇒ 装不上；`aleph plugin validate` 说没问题、`aleph plugin pack` 照样打包，因为 CLI 自带一套更弱的 schema。而 `--type nodejs` 正是开发指南的**第一个例子**，能用的那个默认值反而没写进文档——**被记录的快乐路径就是坏掉的那条**。三句判据：① 词汇要有**所有者**（这里是 `aleph_protocol::plugins::PLUGIN_RUNTIMES`，两侧各一条 census），三个持有者零个所有者必然分歧；② **一条只断言「脚手架写下的那个字面量还在」的测试，测的是 `format!` 不是你的代码**（原测试 `manifest.contains(r#"kind = "nodejs""#)` 因此全程绿）——断言要从共享词汇**派生**；③ crate 边界挡住直接复用时（`interfaces/cli` 不许依赖 `alephcore`），两侧各写一半：CLI 验字符串在不在词汇里，服务端验**脚手架写出的形状**解析得出来
- **加 hub 工具要动五处登记**（`hub/mod.rs` + `definitions.rs` + `groups.rs` + constructor 的**构造段和 schema 段**两处 + dispatch）—— 漏 schema 段＝注册了但模型看不见，漏 dispatch＝看得见但调不到 → §5.21
- **一个「驱动外部 CLI」的适配器，先数它发过那个 CLI 要求的**生命周期**动词没有** —— browser 的 managed backend 发了 28 个子命令、独独没有 `open`，而 `playwright-cli` 的每一个动词都要求先 `open`：**默认 driver 从来没启动过浏览器**，四轮无人发现。症状在类型上是可见的——`BrowserError::NoSession` 被构造、**全仓零消费者**——但只有真机会告诉你为什么。配套两条：① **错误分类器只读 stderr，就分不出把诊断写到 stdout 的 CLI**（那句 "not open" 在 stdout、stderr 空、退出 1 ⇒ `NoSession` 永远分类不出来，惰性修复也就永远触发不了）；② **重复执行「开始」动词可能是破坏性的**（实测第二次 `open` 换新 pid 并丢掉全部 tab），所以自动补发只能挂在**对方自己的拒绝**上，不能挂在自己的猜测上 → §3.12
- **一个外部进程的成功判据，要问它自己怎么表达失败——退出码只是其中一种通道** —— `playwright-cli` 只对**参数**错误退 1；`eval` 抛异常、元素不匹配、未处理的 modal state、以及每一条 `File access denied`，全部是 stdout 里一段 `### Error` + **exit 0**。只看 `status.success()` ⇒ `browser_pdf` 对一个它被拒绝写入的文件回答「Saved PDF to <path>」，`browser_upload` 回答「Uploaded 1 file(s)」而一个字节都没传。判据两句：① **让两条通道走同一个分类器**（否则 "not open" 从 stdout 说出来能触发惰性启动、从 stderr 说出来就不能）；② 在**不可信内容**上做的段头匹配要锚在**第一个**段头——`snapshot`/`console` 会把页面文本回显进 `### Result`，放宽成「任意一行等于 `### Error`」就是让页面自己决定读它的调用失不失败 → §3.12
- **给外部工具加一条 containment 配置，可能顺带收窄它别的能力——加完要把它「还会写文件」的每个动词跑一遍** —— 为阻止 CLI 往 server 的 cwd 里写页面快照而设的 `outputDir`，同时把 playwright-core 的 `checkFile` 允许根收成 `outputDir ∪ cwd`：screenshot / pdf / state-save / upload **四个动词一起坏**，且因为上一条全部报成功。判据：一个配置项的**文档**说它做什么，不等于它**只**做那件事。⚠️ 解法是关掉那道**更弱的第二个答案**（`allowUnrestrictedFileAccess`），不是绕开它——真正的闸是本仓自己的 protected-location denylist（更知情、有测试），而 `outputDir ∪ cwd` 会放行 server 启动目录（实践中是一个 git 检出）却拒绝 `/tmp`，那不是任何人选过的边界 → §3.12
- **同一个意思的拒绝，措辞和通道都可能不止一份** —— CLI 对「浏览器没开」有**两句**：未知会话走 stdout（`… is not open, please run open first`），**有记录但已关闭**的会话（＝任何配了 `user_data_dir` 的持久 profile）走 stderr 抛 node 异常（`Error: Browser 'x' is not open. Run …`）。分类器只认第一句 ⇒ 被 idle reaper 关掉的持久 profile 再也拿不到 `NoSession`、惰性启动永不触发、**收割器把它回收的东西弄成了砖**。判据：写下一条锚点前问**这个拒绝有几种发生方式**，然后取它们的公共子串，别删掉旧锚点（第三种措辞更可能像其中一条，而不是像你预测的那条）→ §3.12

- **「这个 CLI 没有对应 flag」不等于「没有对应面」** —— 上一条的孪生，且它推翻的是一条**有理有据的旧记录**：上一轮读了 `--help` 的 flag 列表、没找到 proxy/user_data_dir/extra_args，就诚实记下「无对应面，猜 flag 名会产出静默忽略」。找错了面——`open --config <file>` 有一份文档化的 JSON schema（`browser.launchOptions` 就是 playwright `LaunchOptions`，含 `proxy` 与 `args`），`close` 也一直存在。判据：写下「这个外部工具做不到 X」之前，**把 flag 列表、配置文件 schema、子命令表分别看一遍**；三者是三个面，`--help` 只答得了第一个 → §3.12
- **一个「读外部进程输出」的 parser，格式必须从**真实输出**抄，不能从记忆里描述** —— `parse_tab_line` 的 doc 写着「Playwright CLI 格式 `Tab N: URL`」，那个格式**没有任何 driver 发过**（真格式 `- 1: (current) [Title](url)`）⇒ 每一行解析成 `None`，而后果不是"少了点信息"：`active_tab_id` 恒 `None` ⇒ tab id 回退哨兵、`post_nav` 的落点 SSRF 复检**对空列表审计通过**。判据：parser 的每种支持格式旁边要能指出它是从哪次真实输出抄来的 → §3.12
- **一个 verb 发给外部 server 的参数名，只有那台 server 的 schema 能裁决——fake backend 结构上看不见它** —— 因为假后端返回的正是代码所期望的东西。chrome-devtools-mcp 的 `fill_form` 要 `elements`，Aleph 发的是 `fields`，而顶层 `additionalProperties: true` 会**收下**多余的键、只拒缺失的必填键 ⇒ 每次调用 `-32602`，`browser_fill_form` 在该 driver 上从未填过一次表。与 round-2 的 `wait_for` 收 `string[]` 而我们发裸字符串是同一个形状、同一个 driver。判据：接一个外部 MCP/CLI 动词时，**去问它 `tools/list`（或 `--help`/config schema）要真的 schema**，然后把参数构造抽成一个有名字的函数、用测试把键钉住——「这个键读起来很合理」是这类缺陷的全部成因 → §3.12
- **⚠️ 而你读的那份源码，必须是正在跑的那个版本** —— 上一条的元判据，本轮亲自踩：npx 缓存里躺着 8 个版本的 chrome-devtools-mcp，我用 `ls -d … | tail -1` 打开了 **1.3.0**（那里路径闸对未协商 roots 的客户端确实是 no-op），而 run.sh 用 `sort -V | tail -1` 跑的是 **1.7.0**（v1.6.0 起路径闸**默认生效**，`roots()` 恒含 tmpdir，只有 `--allow-unrestricted-paths` 能关）。源码说「没有限制」，真机说「Access denied」。判据一句话：**写下「这个外部工具的行为是 X」之前，先确认你读的那份拷贝就是被测的那份**；版本选择用 `sort -V`，别用目录序
- **两个 backend 实现同一个 trait 时，一边修好的判据要主动搬过去**（§0 孪生条的 backend 形态）—— 管理型 driver 在 round-2 被修成「交出值而不是通话记录」（`parse_result_value`），MCP driver 没有：`browser_evaluate` 一直回「Script ran on page and returned: + 一段 json 代码块」这样一段散文。**症状不是报错，是同一个工具在两个 driver 上返回两种形状** ⇒ 任何比较值（而非子串搜索）的调用方在一个 driver 上对、在另一个上错。单一源 `chrome_mcp_backend::parse_evaluate_value`，契约与 `playwright_cli::parse_result_value` 逐条对齐（无 fence ⇒ `None` ⇒ 原样透传，因为抛异常的脚本回的是裸 `Error:`）
- **一个 trait 默认方法说「不支持」时，正面覆盖测不到它——负面那半要单独断言** —— MCP driver 的 `pdf`/`save_state`/`cookies` 是一侧能力，`Err(unsupported_in_existing_session)`；一个返回 `success: true` 却什么都不做的桩，正面用例一条都抓不到。QA 因此对这三个动词断言**拒绝**且**拒绝里点名补救**（"use a managed profile"）
- **凡「无 X 降级」的测试，先问它的绿是代码属性还是机器属性** —— 那批断言在浏览器不可达时全绿，`open` 一修好，其中一个当场把真 Chrome 开到公网（而它的慢路径还会**联网装运行时**）。密封点要落在**唯一那个伸手到进程外的边界**（这里是 `resolve_binary` 的 `cfg(test)` 早返回），不是逐个改测试——后者会漏，且下一个新测试默认不密封。⚠️ 两条配套：密封**本身要有断言**（否则它只是没被证伪过），且要说清**它覆盖哪几种块**（`cfg(test)` 只覆盖 `--lib`，`tests/` 集成测试不在其内）→ §3.12
- **「动态路由」是三件事，且第三件不在 `src/routing/`** —— 工具选择（prompt，禁止意图分类）/ 消息→agent（`src/routing/`）/ 请求→provider（`src/providers/route_policy.rs`）。业界那些"路由大脑"整类违 R7，**不移植** → §3.6
- **判断"这是主槽吗"只认 `SlotKind`，绝不能拿 `tier == Unknown` 当代理**（两个方向都错过）；**装饰器少一层委托，整条链的能力就没了**（门是 `AiProvider::supports_streaming()`）；**「这一轮用哪个模型」只有一个决定点** `runner_impl.rs::effective_model_directive`，**别让新来源止步于 UI** → §3.6
- **状态码判断一律走 `llm_retry::has_status_code`** —— `contains("401")` 会命中 `40123` 这种 token 计数 → §3.6
- **同一个能力有两套栈时，先问「工具接的是哪一套」** —— 单一源 `src/media/resolve.rs::transcription_service` 被 `agent_init` 与 registry 构造器共用；**编进散文里的参数等于没传**（`language` 是原生参数）→ §7.6
- **语音模式经进程级注册表摆渡，写入点有两个都要写**（channel 侧 `inbound_router/executor.rs` + Panel 侧 `handlers/agent.rs`）—— 漏一个，那个 surface 的语音回合永远拿不到口语风格层 → §2.4

### 10. 构建与验证

- **`cargo check` 不编译 `#[cfg(test)]`** —— 删 `pub fn` / 字段的同一笔里必须跑 `cargo test --no-run`；只跑 `cargo check` 等于没验证 → [CODE_ORGANIZATION.md](docs/reference/CODE_ORGANIZATION.md)
- **⚠️ 这条对每个 crate 都成立，而最小验证集只对 `alephcore` 用了 `--no-run`** —— `aleph-panel` 的文档化检查就是 `cargo check -p aleph-panel`，于是它的测试二进制在一次形状搬迁（`PresetProviderDto` → `aleph_protocol::providers::GenerationPresetRow`）之后**整程编译不过、805 条测试一条没跑**，包括同一段改动里新写的那些。同族更外一层：`cargo check` / `--lib` 也**不编译 `tests/`**，所以「这三个 re-export 零调用者」这类论断可以在两条命令都同意的情况下是假的（`tests/security_integration.rs` 一直在 import 它们，`--all-targets` 因此整程没构建过）。判据：**说"零调用者"之前，先问哪几条命令编译得到那些调用者**
- **把一个函数改成 `async`，它的调用点会变成一个未 await 的 future——Rust 报的是 WARNING 不是 error** —— 于是那一步在**一切照常编译**的情况下静默停止执行。`freeze_owned_background_work` 加第三条腿时正是这个形状：整个停用扫描（吊销设备 / 撤销频道凭据 / 冻结 goal·loop·cron）会一起不再运行，而 `cargo check`、`cargo test --no-run`、CI 全绿。判据两句：① **给一个已有调用点的函数加 `async` 时，去读那些调用点**，别信构建；② grep 编译输出时别只筛 `^error`——这一类只出现在 `^warning` 里（`unused_must_use` / `unused` 家族）
- **一个 `from_row` 配 N 个 `SELECT`，位置解码就是一份没有编译器背书的契约** —— 给其中一个投影加第 K 列会让另外那些的 `row.get(K)` 变成**运行时**错误（这里差点炸的是 `get_device_by_fingerprint`——每次远程 connect——与 `get_device`——节点准入）。判据：列清单收敛成**一个常量**由所有 `SELECT` 插值；「三处手抄的列名恰好一致」不是不变量，是巧合 → §5.22 round-4 ⑤
- **CLI 参数定义的冲突是 debug 断言，不是编译错误——所以「编译 + 单测全绿」与「这个二进制根本起不来」可以同时为真** —— `aleph-tui` 的 `-c` 被 `--config` 和新加的 `--continue` 同时占用，clap 的 `debug_asserts` 在 `Args::parse()` 里 panic，**早于 main 的任何一行**：debug 构建 100% 启动即死、任何参数都一样；release 不 assert，改为静默把字母判给其中一个。三样东西同时看不见它：不是编译错误、`cargo check` 不编译测试、**而下面那五条最小验证集根本不到 `aleph-tui` / `aleph-cli` 这两个 crate**。判据两句：① 每个 clap 二进制都欠一条 `Args::command().debug_assert()` 测试（clap 自己的校验器，当测试跑）；② **给一个已发布的二进制加短参之前，先数这个字母已经有谁在用**——冲突时字母归**已发布**的那一个，新参数走长名（`--session` 在同一个 struct 里早有先例：`-s` 被 `--server` 占了就用 `-k`） → §5.23b
- **`cargo test -p aleph-panel --lib` 编译的不是它出厂的那份产物，所以一条错位的 `#[cfg(test)]` 能全绿地过去** —— 上一条讲最小验证集漏了哪些 crate，这条讲**同一个 crate 的两个目标**。panel 的 shipped artifact 是 `wasm32-unknown-unknown` 上的**非 test** cdylib，而 `--lib` 测试构建里 `cfg(test)` 为真：把一个新 `mod` 插进 `#[cfg(test)]` 与它所门控的那个 `mod` **之间**，属性就改门了新模块，旧模块变成无门——测试构建两个都编译（全绿），`just wasm` 上旧模块的 `use crate::disposed_reads::…` 直接 E0432，因为那个模块本身是 `#[cfg(test)]` 的。判据两句：① **往一个 `#[cfg(...)]` 上方插东西之前，先看那个属性门的是谁**（插入点在属性和 item 之间是最容易犯的一种）；② 改 `interfaces/webchat/` 之后跑一次 `cargo build -p aleph-panel --lib --target wasm32-unknown-unknown --profile wasm-release`（或直接 `just wasm`）——它是唯一编译出厂形态的命令，顺带把 `--lib` 测试构建看不见的 `unused_imports` 也报出来
- **⚠️ 最小验证集不**跑**客户端 crate 的测试** —— 下面五条里**跑测试**的那几条没有 `-p aleph-tui` / `-p aleph-cli`（那条 `--workspace` clippy 会 lint 到它们，而 **lint 不是测试**：它编译得到那些 `#[cfg(test)]`，但一条断言都不跑），而这两个 crate 恰恰是「wire 契约两半住在两个 crate」缺陷的常客（`aleph workspace create` 与 TUI 的 `agent.run` 都在这里翻过车）。改动它们的同一笔里跑 `cargo test -p aleph-tui -p aleph-cli`
- **嵌进字符串里的那门语言，编译器一个字都不看** —— `execute_batch(r#"…"#)` 里写 Rust 风格 `//` 注释，rustc 全绿、clippy 全绿、`cargo check` 全绿，而 SQLite 在第一个 `/` 上语法错误 ⇒ **整份迁移中止**，那批表在运行时压根不存在（`coord_task*` 一条没建，teams / workflow / swarm 任务全死，`--lib` **133 条红**，2026-08-11 修）。**注释语法写错不是排版问题，是一次停机**。判据：往 SQL / 正则 / shell / JSON-Schema 这类**嵌入式字符串**里加解释性文字时，注释符要按**那门语言**写（SQL 是 `--`），并且**改完必须跑一次真的执行它的测试**——本仓这段的守卫恰好存在（任何建表的单测都会红），只是提交那笔改动时没跑到
- **源码级守卫里用 `\n` 锚定的分隔符，在 CRLF 检出上匹配不到任何东西** —— 本仓 Windows 检出是 CRLF（git 自动转换），所以 `src.split("\n#[cfg(test)]\n")` 的实际字节是 `\r\n#[cfg(test)]\r\n`，**永不匹配** ⇒ "生产前缀"变成整份文件，守卫开始扫自己的测试模块，把**断言字符串里的字面量**当成命中。症状是 Windows 红、CI（LF）绿，看起来像"平台差异"。**更贵的是安静的那一半**：`checked > 0` 这类"我确实扫到了东西"的自保断言，会被测试模块里的字面量满足 ⇒ 真的生产站点被删掉时它照样绿。写源码级守卫时分隔符**不要锚行首行尾**（`split("#[cfg(test)]")` 即可），且自保断言要能区分"扫到的是生产代码"。⚠️ **这条规则写下来之后，同一次会话新增的第二条守卫仍然带着它出厂**（`run_loop/tests.rs::scope_stamping_producers_are_all_accounted_for`，2026-08-09 修）——记下一条判据不等于扫过它的其余实例，**同批新增的守卫要一起扫**。⚠️ **判据比"含 `\n` 就有病"更锐利**：坏的是 **`\n` 前面还有字符**（`]\n` 在 CRLF 下是 `]\r\n`，不匹配）；**`\n` 开头的分隔符是安全的**（`"\n}"` 在 `\r\n}` 里照样命中，`cron/real.rs` 因此无恙）。最稳的形状是先 `.replace('\r', "")` 再 split——`server_init.rs` 就是范本，它连非空自保断言一起写了。⚠️ **同一个 CRLF 在 shell 那一侧的形态：三个工具读入时把 `\r` 剥掉，两个不剥**——`sed` / `grep` / `awk` 剥，`head` / `tail` / `cat` / `od` 不剥（本树实测），于是**同一个文件、五个工具、两种内容**，而没有任何一个会出声。脚本化编辑因此要**保字节**（latin1 读写，或直接用 node），改完按 **`CR 数 == LF 数`** 与**零个 U+FFFD** 复核。⚠️ **而这条对 `node -e` 也成立，本会话就栽了一次**：经这层 shell 传进去的 `\r` 被吃掉一个反斜杠，在 JS 字符串里变成**一个真的回车**，把 `/\r\n/g` 写成了一个语法错误的正则——**要写含反斜杠的脚本就走带引号的 heredoc**（实测：单反斜杠原样活下来，双反斜杠塌成一个），别走 `-e`
- **一条会卡死的测试比一条会红的测试贵得多，因为整套件从此没有信号——而它通常卡在自己的「成功」分支上** —— `cached_repo_root_releases_lock_before_filesystem_io` 把 `try_lock()` 的**守卫**绑进一个变量，然后 `join()` 工作线程：`try_lock` 成功正是被断言的好结果，而它意味着主线程**还握着锁**，工作线程收尾那次 `cache.lock()` 永远等不到 ⇒ **通过路径就是死锁路径**。单跑时那 20 ms 常常够工作线程整个跑完（锁空闲、断言空转通过），15k 测试并行时不够 ⇒ `--lib` 停在 15888/15911，**零 CPU、零输出**，读起来像"这几条很慢"而不像挂了（2026-08-12 修）。判据两句：① **断言取的是值还是守卫**——`let ok = m.try_lock().is_ok();` 与 `let g = m.try_lock();` 在类型上只差一个 `.is_ok()`，在运行时差一个死锁，凡 `try_lock` / `try_recv` / `try_borrow` 的结果要跨越一次 `join` / `await` / 阻塞调用，先问**这中间有没有人要这把锁**；② **一次 `--lib` 没有跑到 `test result:` 行就不算跑过**——尾巴上的 "has been running for over 60 seconds" 是唯一的症状，用 `Get-Process | TotalProcessorTime` 采两次差值区分"慢"和"死"（0 就是死）
- **一条负向断言，先证明它找的那个字符串**有可能**出现在这份 payload 里** —— 「恒真的谓词等于没判」在真机夹具上的形态，而这里它伪装得更好：`commands.list` 返回的是名字/描述树、**从不携带正文**，所以「没有未展开的 `${CLAUDE_PLUGIN_ROOT}` 幸存」在那里恒绿——对一个展开完全坏掉的插件也恒绿。判据是**先锚定后否定**：先断言那段正文确实在这份 payload 里（用一个展开前后都不变的子串），否定断言才有意义。⚠️ 配套：**锚点要选在截断活得下来的那一半**——`hooks_manage` 把动作标签截到 80 字符，尾巴上的 marker 因此被切掉，而头部（`command: sh ` 之后紧跟的那个路径前缀）恰好是未展开变量会出现的位置
- **一个 QA 断言报警时，先证明那个回归是真的，再动手修它——否则你加的东西可能是个 no-op** —— 「红的条数比预期少时先怀疑自己的判断」的反向孪生，代价更贵：这次是**红得比预期多**。trust 场景首跑读到「91 个插件 0 个 loaded」，看起来像新策略把整个随包技能库带走了，于是我加了一个 `trust_gated` 旗标去豁免它们。**把闸对每一个条目强制打开之后，被闸住的集合一个都没变**——技能目录没有插件 manifest，`parse_dir` 先拒了它，`error` 行在闸之前就记下了；那些行在强制之前也不是 `loaded`。所以那是一条**选错的断言**，不是它看起来的那个回归，而我为它写的旗标是零消费者抽象（R10 撤回）。判据两句：① **改代码之前，用变异证明那条路径真的会走到你以为的地方**（这里只要把谓词写死成最坏值，看断言变不变）；② 撤回时**把「为什么这个并集是安全的」留在原地**，否则下一个读者会重新推出同一个错误警报
- **变异 harness 的四分判据，顺序和分类一样重要——因为 cargo 对「测试失败」也打 `^error:`，对 RED 也打 `0 passed`** —— 上一轮记下「必须四分 RED / GREEN / BUILD-ERROR / VACUOUS」之后，这一轮写第一版就连错两次：先把 `0 passed` 当 VACUOUS（而 `test result: FAILED. 0 passed; 1 failed` 正是真红），再把 `^error:` 当 BUILD-ERROR（而 cargo 在测试失败时也打 `error: test failed, to rerun pass …`）。**两次都把「守卫正常工作」读成「量具坏了」，方向恰好相反于上一轮那次。** 正确顺序是：`running 0 tests` ⇒ VACUOUS → `test result: FAILED` ⇒ RED → `test result: ok` ⇒ GREEN → **剩下的（连 `test result:` 行都没有）才是 BUILD-ERROR**。判据一句话：**分类器要按「这句话只可能由哪一种结局打印」排序，别按它出现得早不早排序**。⚠️ 两条后来才付的账：① **「没有 `test result:` 行」装着两种东西**——构建失败**和跑挂了**，落进 BUILD-ERROR 桶时先看有没有编译错误行，没有就是挂了；② **分类器本身是一件仪器，在它没跑过的 crate 上有一段未测量的量程**——`aleph-panel --lib` 里一次失败可以经 **wasm-bindgen 的 panic hook** 中止（panic-in-panic，SIGABRT）并且**一行 `test result:` 都不打**，于是真 RED 被读成 BUILD-ERROR；而一次带过滤器的运行打出 `1 filtered out`，单独看就是**穿着 GREEN 外衣的 VACUOUS**（把 GREEN 当成覆盖之前先看过滤计数）→ 附录 C.5
- **一个没有 `mod` 声明的测试文件，和一个全绿的测试文件在任何报告里长得一模一样** —— `src/gateway/handlers/plugins/handlers/tests.rs` 有 325 行插件 RPC 参数测试、零 `mod` 语句，自写下之日起一次都没编译过；`cargo check --all-targets` 也看不见它——**一个没被声明的文件根本不是这个 crate 的一部分**，没有任何 lint 会提它。接上之后立刻现出它累积的腐烂（`INTERNAL_ERROR` 三处断言从未 import、一个从未用过的 import），而这正是「从来没构建过」的指纹。判据：`ls` 一遍测试目录，对每个 `.rs` 问**谁 `mod` 它**；顺带记一次复发——`cargo test -p alephcore --lib --no-run` 在 HEAD 上**又**是红的（9 个错，`explain_fact` 加了参数 / `ChannelOutboxArgs` 加了字段 / `chrome_launch_args` 加了参数，三处生产签名改动只跑了 `cargo check`），2026-08-17 那次记的是同一个 crate 的同一件事。**这一族的力在 CI 上**：`.github/workflows/aleph-core-ci.yml` 跑的是 `cargo test -p alephcore --lib`（会构建），所以红的是"没人在合并前跑过 CI"而不是"没有闸"
- **一个跑不起来的套件不会红，它会安静——而它安静期间被抬过的每一个棘轮数字都是手算的** —— `cargo test -p alephcore --lib` 自 `5433648fc` 起编译不过（`Some(tokio::spawn(async move {` 用 `});` 收尾，少一个 `)`），**16,450 条 lib 测试一条没跑**，`aleph-server` 二进制也一直构建不出来。期间有人抬了描述字节棘轮：`92_798 + 1_297 = 94_095` ——**正好是算术**，因为闸跑不了的时候作者只能算。真实测量是 `94_605`。判据三句：① 抬棘轮的同一笔里必须有**那条测试真的跑过一次**的证据，纯算术的新数字要在账本里写明它是算的；② 修好构建之后**第一次跑出来的失败数，才是那一族的真实大小**——本轮是 4 条，全部先于该分支存在；③ 那 4 条里有 2 条**钉住的是已被有意撤销的规则**（`voice_mode_set` 的 `"default"` 回落、`registry_adapter` 的 dm/group 塌缩），处置是改测试并同步**那条撤销没改到的 doc**（`claim_session_key` 还在拿塌缩给自己做论证），**不是**让代码回去迁就测试
- **少一个 `)` 不是排版问题，是一次作用域重排——它会让一段读起来正确的代码"正确"在一个不存在的世界里** —— `Some(tokio::spawn(async move { … });` 把函数余下部分整个吞进闭包，于是 BIN-R4-13 那句「把 engine 声明在 cron 块**外面**好让 heartbeat 复用同一个 `Arc`」在**块内**声明也读得通、也过 review。判据：一个"我把它提到了外层作用域"的注释，去看它声明的那一行**缩进在谁里面**；编译器本该替你查，而当它被一个更早的定界符错误挡住时，它一个字都不会说
- **一个数字必须带着它测的那个谓词、以及它测于哪个 commit，否则它会在每一次复核时移动——而每个复核者都会断定上一个人错了** —— 这不是马虎，恰恰相反：下一个人善意地量了一个**略微不同的问题**，得到一个不同的数，**两个数都对**（同一句话背后，**111** 是"裸子串搜 `#[cfg(test)]`"、**120** 是"`cfg_test_portion()` 什么都没返回而该文件确实含测试"——后者才是那句话真正断言的性质）。跨度同理：`utils/source_scan.rs` 写着「实测 1734 个文件带该标记」，而**写下它的那一轮自己把它挪到了 1739**。⚠️ **两次测量彼此吻合不等于互相印证——如果它们是因为同一个原因才吻合的**：问「这两个测量会不会因为同一个理由一起错」；而两个**各自都精确求和**的分区未必在切同一个对象，可比的只有它们各自的总数。⚠️ **一次落回预期值的测量才是最该重跑的那个**：本轮每个被独立重测的数字都变大，唯一那个落回去的（46 → 47 → 46）是两个方向相反的错误恰好抵消——数字对了，花名册差三个人。**分解才是那个 tell，总数不是。**⚠️ 而来源也要说出口：一次更正只证明**旧值错了**，不证明新值对；**提交信息是关于一次 diff 的散文，不是对树的测量**——它读起来更权威恰恰因为它挂在一个 diff 上，本轮把一个中途 commit subject 里的"八"抄进了文档（真值 14），抄进的正是论证这条判据的那一段。⚠️ 而它不只发生在跨复核者之间——**一句 `grep -c` 就够**：想数「那条测试红过几次」，数到的是「有几次跑里出现过一个失败」（红的是另一条）；换个字面量去数，又撞上**干净的跑里也含**的那个子串（`FAILED=[]`）。**没人能从读那句 grep 预测到它**，救下它的不是方法，是手里**恰好有第二个来源**——所以一个计数的谓词就是那句 grep 自己写下的那个，写下结论前去读它数的是不是你要的那件事 → 附录 C.1
- **一个裸计数在被写下之前，先说出它数的是哪一类、用什么量具数的** —— 上一条讲一个数字**在复核之间**移动，这条讲它在**一次**测量里就已经是错的。本轮控制器侧四次，四种成因各不相同，而**四次都产出一个看起来完全合理的数字**：

  | 量具 | 报出 | 真值 | 成因 |
  |---|---|---|---|
  | `grep -c '::'` 数基线失败名单 | 26 | **21** | 文件**头部注释**在解释两处删除时引用了全限定测试名——**文件自己的说明回答了我的搜索** |
  | `grep -c "alice_session("` | 12 | **11** | 把**定义行**算进了调用点 |
  | 函数体字节数（三方各测一次） | 685/246 · 685/235 · 604/135 | 同一性质 | 三种抽取边界约定，而**没有一方说出自己用的是哪一种** |
  | `grep -o '[①②③④⑤]'` | 545 | **27** | 这个 shell 的方括号是**字节集**：`①–⑤` 的 UTF-8 字节落在 `e2 91 a0..a4`，而**每一个相邻的 CJK 字符都在往这个集合里投字节** ⇒ 读数跟着整行的**字节量**走，跟被数的那个字符几乎无关 |

  第四行值得单独看一眼：它不是「按字节切开所以偏大三倍」，是**根本没在数那个字符**——同一条坏量具在早一个 commit 上报的是 **2814**，**连它的错都不可复现**，因为它量的是语料的字节量。既有判据（「用一个量具下结论之前，先确认它看得见你要数的那一类」）因此补两句：**也要确认它只看得见那一类**，以及**它认不认得你这类字符**——CJK 与圈号一律用 node 数，别用 `grep -o` / `grep -c`。⚠️ **而一次 before/after 的差额还欠第三个数**：我判定「31 → 27 对不上」、把它写成「结论还对、理由已经死了」交出去，而复审逐个分类重数后对账**精确成立**——`31 −6（外层标记改成配套 N）−2（①②→其一/其二）+4（新写的说明自己带进来的两个「①–⑤」区间）= 27`。**我数了改动前，也数了改动后，唯独没数这次修复自己往被计数的那一类里添了什么**——而它添的恰好就是那一类。判据：**差额对不上时，先问「这次操作自己往被计数的那一类里添了几个」**；缺的那一项永远在修复自己身上（与「红的条数比预期少时先怀疑自己的判断」同族，但那条讲**观测少于预期**，这条讲**两次观测之间的差额**）。
- **你跑的那棵树、你调的那个探针，都不是你以为的那个——所以相信一个颜色之前，先说出你预期的那个具体后果，然后专门去查那个后果** —— 不是 diff，不是颜色，也不是一个你自己写的探针。三种成因**都产出干净的绿外加一份看起来正确的 diff**，四分类器对三者全失明（输出是诚实的，不诚实的是输入）：① **`CARGO_MANIFEST_DIR` 是编译期烤进二进制的**，所以在 A 树里构建的测试二进制无论你在哪棵树里运行它，读的永远是 A 树的源码——源码级守卫全中，本轮因此在**全轮最严格的那次复核里**产出过一个假 GREEN，两次独立的后续运行在同一处得到 RED；② **那次编辑根本没落地**——补丁脚本在中途 assert 失败、在它**唯一那次写入之前**退出（所以补丁脚本必须**在写入任何一处之前**校验它的每一个锚点：半写比一处没写更糟，树会停在一个没人设计过的状态里）；③ **探针重新实现了它要测的那段逻辑**，于是它测的是自己的重实现——与实现分歧时**先怀疑探针**（它是没有测试的那一个），更好的做法是**让探针去调实现自己的 helper**。⚠️ 配套：**一次运行分不出「这个说法错了」和「这个测量有噪声」**，而用哪种防御取决于那件仪器是不是你自己的——**别人的仪器你重复测量，自己的仪器你先怀疑它** → 附录 C.2
- **重构一个有几十个消费者的共享工具时，「行为没变」要测不要读** —— `utils::source_scan` 这一轮长出 `code_keeping_literals` 并与 `code_text` 共用一次词法走，而**读代码读不出**旧函数是否逐字节没动；它的消费者是几十个源码级守卫，每一个都会在它变了的时候**安静地**换答案。方法值得抄：把**两个 sha 上的模块各抽成一个独立程序**，在真语料上逐函数比对输出（`src/ tests/ interfaces/ shared/ desktop/`：**3 297** 个 `.rs`、**16 485** 次比较、**0 处不同**），**并且先把量具弄坏一次**（强制 `keep = true` ⇒ 12 290 次里 **2 300** 处不同，带首次分歧偏移）——**一条没被证伪过的守卫不算守卫，这句话对量具自己也成立**。⚠️ 顺带一条记账纪律：这三个数里只有**文件数**在本轮复测时纹丝不动（3 297），原记录里那个「50.6 MB」复测是 48.3 MB——**体积随树漂，别把它和结论一起引用**。
- **`cargo fmt -p <crate> -- <file>` 不会只格式化那个文件** —— `cargo fmt --help` 第一行逐字写着它格式化「当前 crate 的**全部** bin 与 lib 文件」，而 `--` 之后的东西是**传给 rustfmt 的选项**，不是文件过滤器。要格式化单个文件只有一条路：直接 **`rustfmt <file>`**。共享 worktree 里这条尤其贵——它会把树里既有的漂移一并卷进你的改动，而别人看到的是"有第三方在实时编辑这棵树"。**本仓已经栽过两次**（一次 66 个、一次 5 个无关文件），而第一次记录下来的是**症状**（"别跑 `cargo fmt -p alephcore`"）：**记录症状阻止不了复发**——第二次那个人相信自己已经限定了范围，并且手里有一句看起来能证明这一点的咒语 → 附录 C.6
- **一条被挪到后台的验证命令，它的「completed (exit code 0)」不是结果——那是管道尾巴上 `tail` 的退出码** —— 而会被挪到后台的恰恰是这个仓最重的那条闸：`cargo test -p alephcore --features test-helpers --test '*'` 要 `-j 1`（默认并行会以一个**假的** `can't find crate` E0463 失败——那是资源限制不是代码），而 `-j 1` 是一小时量级，超过本 harness 的 600 s 前台上限。**一次实测 17 分钟链完 29 / 137 个集成测试二进制——所以 29 只是它停在哪儿，不是结论。**判据：**这条命令的结论要么真的跑完，要么标成 UNCLAIMED**；第三种「跑了一半然后按比例外推」是这一族最容易被接受的假话，因为它读起来像一次测量。同族一句：后台命令要按**它自己写下的那行 `test result:`** 判，不按 harness 报给你的那个退出码。
- **最小可信验证集是六条命令，不是一条**：
  ```
  cargo test -p alephcore --lib --no-run
  cargo test -p alephcore --bins                                        # 唯一一条真跑而非 --no-run 的：
      # --lib 与 --bins 是两个 target，前者一条都带不到 src/bin/ 下的 94 条（含钉住 boot 无条件
      # install_policy/install_ledger 的那条 census）；clippy 编译它们但不跑断言，所以只有这条会红
  cargo test -p alephcore --features test-helpers --test '*' --no-run -j 1
      # --all-targets 只展开 target 不展开 feature；`-j 1` 不是保守是必须——默认并行会以一个假的
      # `can't find crate` E0463 失败（资源限制，不是你的代码），而这条要一小时量级，见上一条
  cargo test -p aleph-panel --lib --no-run                               # check 看不见它的 #[cfg(test)]；曾整程编译不过
  cargo check -p aleph-desktop-{macos,windows,linux}                    # 跨平台改动要 check 那个目标的限肢 crate
  cargo clippy --workspace --all-targets                                # 先 just _stage-shell-placeholders
      # --all-targets 展开的是 target（examples 只有它才暴露），--workspace 展开的才是 package：
      # 根 Cargo.toml 无 default-members ⇒ 默认只 lint 根 crate 一个（13 个成员），panel/tui/cli 全在外面
  ```
- **`interfaces/webchat/` 有任何改动（哪怕不是你改的）就跑一次 `cargo test -p aleph-panel --lib`**（不是 `cargo check`——它看不见这个 crate 的测试模块，那正是 805 条测试整程没跑的原因） —— 这个 crate 的**语义合并冲突是常态形状**：一侧的类型 + 另一侧的调用点，git 不报冲突、两边单独看都完整。合并实现过同一功能的分支前先 grep 功能名；修完**先看警告再看错误**（`unused variable` 说明那半边根本没有调用者，正解是 CUT）
- **`cargo check -p aleph-desktop-shell` 前需先 `just _stage-shell-placeholders`**（tauri-build 要求 externalBin 占位文件存在）—— **上面那条 `--workspace` clippy 同样要**，它会把 shell crate 一起编。占位路径**别在别处抄一份**：那条 recipe 自己推 triple、Windows 补 `.exe`、`AlephBridge-` 只在 macOS 上建，手抄的模板会漏掉后两件
- **`MessageRecord.timestamp` 单位有歧义**（SQLite 写秒 / file backend 写毫秒）—— 一律走 `MessageRecord::instant()` / `rfc3339()`（`src/gateway/session_store/types.rs`，1e11 分界），裸格式化就是这个 bug 的下一次复发。**源头未改是有意的**——该值同时是 `get_history_before` 的分页游标，改单位要连全部存量会话一起迁移
- **一个子代理报 idle 而无产出，和它**断线**结束，是两件事——后者是唯一一种可能把树留在半应用状态的结局** —— idle 那种的规矩是先去读它的产物和日志再判断它的能力（**一个 idle 的 agent 不是关于那个 agent 的证据**）；`idleReason: "failed"`（连接中断）那种要走**现场勘查**，四步：① **先勘查再决定，而且四项互相独立**——树干净吗（有没有半应用的探针）· 有没有 cargo 残留 · commit 落地了几个 · 报告写了没有；② **不要由控制器代写它的报告**——它手里有你没有的上下文，**而一份由你写的报告不是独立证据**；让它续跑（`SendMessage` 到那个 agent，名字仍可寻址）；③ **明确要求它交代「断线那一刻有没有留下半途而废的东西」**——它主动说出的缺口是便宜的，你后面自己发现的不是（本次答案是干净的：断在写报告那条 bash 的**解析期**，heredoc 引号不配对，一个字节都没写出去）；④ **但那是唯一不可能知道自己错了的那一方给出的声称**，所以复审要分一点预算给**完整性**而不只是正确性——本次照做，并因此找到一处「报出的计数只在证伪它的那次运行里打印」的散文缺陷。

---

## 📍 子系统路由 (Read Before Editing)

| 你要动的目录 | 先读 |
|---|---|
| `src/harness/` | [HARNESS_PHILOSOPHY.md](docs/reference/HARNESS_PHILOSOPHY.md) · `src/harness/CLAUDE.md` · FEATURE_LOCATOR §3.1 |
| `src/thinker/` `src/context/` | 判据清单 §1 · FEATURE_LOCATOR §2.1 §2.3 §2.18 §2.19 §2.20 |
| `src/tool_output/` | 判据清单 §2 · FEATURE_LOCATOR §2.7 §3.14 |
| `src/tools/` `src/builtin_tools/` | [TOOL_SYSTEM.md](docs/reference/TOOL_SYSTEM.md) · [SECURITY.md](docs/reference/SECURITY.md) · §3.2–§3.14 |
| `src/gateway/btw/` | §4.14（`/btw` 侧问）· [SECURITY.md](docs/reference/SECURITY.md) 的只读地板一节 · **改到达顺序或退休面前先读 §4.14 的机制图**，真机 `qa/btw_tui/run.sh {frames,promote}` |
| `src/gateway/` | [GATEWAY.md](docs/reference/GATEWAY.md) · `src/gateway/CLAUDE.md` · §4.8 §4.14 §5.6 §5.18 §6.9 · **改 `interfaces/<channel>/` 或通道接线前跑 `qa/channels/run.sh`**（33 条断言，三阶段：`reach` — 三个通道真被构造 · msteams 对照组 · qq 扁平拼法过了配置解析 · feishu `start()` 对 mock Lark 真拨号 · webhook 事件→agent 回合→回复打回 `im/v1/messages`；`errors` — 旧版 400+99991400 限频被重试且退避读 `x-ogw-ratelimit-reset` · 403 报状态码且不重试 · 无限频码的 400 是终态；`approval` — 通道腿的「通知+永久等待」：卡带无过期哨兵**且该键真在 wire 上**（否则断言恒真）· 真发到 Feishu · **过了旧的 120s 死线仍在 pending** · `/approve` 文本回复仍能结掉一张已经超过旧死线的卡。**自带一次 reboot**：`policies` 是 `ReloadImpact::Restart`，运行时 patch 会被保存并忽略，而那会让这一阶段对着一道从未武装的闸作断言） |
| `src/memory/` `src/note/` | [MEMORY_SYSTEM.md](docs/reference/MEMORY_SYSTEM.md) + memory/ 三分册 · §2.5 §2.9 §2.16 |
| `src/providers/` | [MODEL_CATALOG.md](docs/reference/MODEL_CATALOG.md) · §3.6 §4.9 |
| `src/browser/` `src/builtin_tools/browser_tools/` | §3.12 · 判据清单 §9（外部 CLI/MCP 适配器）· **真机 QA `qa/browser_managed/run.sh {open,ambient,headed,tools,frames,reap,pdf,existing,exec-offload}`——两个 driver 的每个动词都有效果断言，改这两个目录前跑一遍** |
| `src/mcp/` | §5.20（dual-era 协议）· §5.24（卸载要丢 usage 行）|
| `src/hub/` | [ALEPH_HUB.md](docs/reference/ALEPH_HUB.md) · §5.21 |
| `src/loop_graph/` `src/workflow/` | [GRAPH_LAYER.md](docs/reference/GRAPH_LAYER.md) · §4.12 |
| `src/identity/` | [AGENT_IDENTITY.md](docs/reference/AGENT_IDENTITY.md) · §5.17 |
| `src/config/` `src/diagnostics/` | §5.8 §5.9 §5.10 · §5.24（扩展调用记录 → `ext/idle-extensions`）|
| `desktop/` | [WINDOWS_RUNTIME.md](docs/reference/WINDOWS_RUNTIME.md) · [LINUX_DESKTOP.md](docs/reference/LINUX_DESKTOP.md) · [DESKTOP_BRIDGE.md](docs/reference/DESKTOP_BRIDGE.md) · §7.1–§7.4 |
| `interfaces/webchat/` | [DESKTOP_SHELL.md](docs/reference/DESKTOP_SHELL.md) · §4.7 §6.8 §6.9 · 真机 QA `qa/picker_nav/run.sh`（键盘 walk / 条件渐隐 / 手机端加 provider，三档宽度各带效果断言）|
| `src/canvas/` `interfaces/webchat/src/platform/wide/views/canvas/` | [CANVAS.md](docs/reference/CANVAS.md) · §6.10 · 真机 QA `qa/canvas/run.sh`（九项清单每条带效果断言） |
| `interfaces/tui/` `interfaces/cli/` `shared/protocol/` | 判据清单 §0（跨 crate wire 契约）· FEATURE_LOCATOR §5.4（`providers.*` 契约 + 搜索匹配器）· §5.11 §5.13 §5.23 |
| `src/agents/` `src/teams/` | [MULTI_AGENT_SYSTEM.md](docs/reference/MULTI_AGENT_SYSTEM.md) · §4.4 §4.5 §4.13a–c |
| `src/tasks/cron/` `src/tasks/heartbeat/` | §4.13b（写面对账守卫 · 共用告警判据 · 停摆 job）· §4.13c（**不阻塞 tick · 投递失败即失败 · 孪生子系统对账**）· `src/tasks/shared/{alert,delivery}.rs` |
| `src/sandbox/` | [SANDBOX.md](docs/reference/SANDBOX.md) · §3.8 · §3.15（后台执行生命周期 · 实时尾巴 · 两阶段 cwd 闸） |

> **对照表已做完，别重做**：openclaw（gateway / cluster / hub / model catalog）· codex（权限模型 / Multi-agent V2）· hermes · pi · LangGraph · RouteLLM/LiteLLM/Bifrost · DeepSeek-Reasonix · FluidVoice/WhisperLive · SkillOpt · buzz · **deepseek-harness (dsh, 2026-08-15)**（Cordis 插件架构本身不移植；10 维扫描 + 13 项对抗验证的逐项结论与 DEFER 见 FEATURE_LOCATOR §3.1 dsh 轮）。逐项结论与"刻意不做清单"都在对应 reference 文档里。

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
| `just verify-build` | CI 验证三产物三平台能否正常构建（build-only，不打 tag、不发布） |
| `just release YY.M.D` | **发版**: 更新 VERSION + 提交推送 + 触发 GitHub workflow（需先写 changelog） |

> ⚠️ 验证充分性见判据清单 §10——`cargo check -p alephcore` 的绿只验证了仓库的一小半。

### Rust 工具链

- **MSRV = 1.95**（由 `sysinfo 0.39` 决定），在 `Cargo.toml` 的 `[workspace.package]` 与 `[package]` 两处 `rust-version` 声明。
- 仓库根的 `rust-toolchain.toml` 钉住具体 stable（当前 `1.96.0`），本地与 CI 自动使用同一工具链——无需 `rustup default` 或 `cargo +<ver>`。抬高 MSRV 时同步更新这两处。

### 会话旋钮 (Session Knobs)

**别在这里维护一个数目**——上一版标题写着「三根」而表里已经不止三行。全部**正交**，且**不是每一根都有 pill**：见「谁在拨」列。

前五根共用一套机制：值住在 `SessionMetadata.identity_meta.custom[<key>]`，precedence 一律 **请求 > 会话 > 全局**，请求携带的值会被**盖回会话**（所以选择活过它所在的那一轮），解析各在 `src/gateway/execution_engine/turn_*.rs` 的孪生模块里。加第六根之前先读那五个文件里任意一个——它们逐行同形是有意的。

| 旋钮 | 值 | 管什么 | 谁在拨 | 单一源 |
|---|---|---|---|---|
| **执行档位 Exec Tier** | `Plan` / `Ask` / `Auto`(默认) / `Full` | 工具执行**审批**。读工具**声明的元数据**（幂等/destructive），不认名字；未知工具在 `Ask` 档 fail-closed。**`Plan` 是只读规划档，仅会话可选**（`builtin_tiers()` 装机三档 vs `session_tiers()` 四档）：mutating 一律拒绝，人类在 `scratchpad(action='request_approval')` 批准后**同一轮当场**交回 restore 档。⚠️ `Plan` 的拒绝是**地板**（`effective_permission` rung 0），排在 explicit `[policies.tool_permissions]` 条目**之上**——一条 `"bash"="allow"` 掀不翻它；其余三档的 explicit 优先逐字节不变 | Panel pill + `chat.send{exec_tier}` + TUI `/tier` | `src/tools/scoped/`（唯一强制点）+ `src/tools/plan_gate.rs` → [SECURITY.md](docs/reference/SECURITY.md) |
| **会话模式 Session Mode** | `chat` / `work`(默认) / `code` | 工具**呈现面**静态分区（R10 渐进披露例外）。不授予不拒绝任何权限 | Panel pill + `chat.send{mode}` + `session_set_mode` 工具 + TUI `/mode` | `src/config/types/policies/session_mode.rs` → [MODE_SYSTEM.md](docs/reference/MODE_SYSTEM.md) |
| **推理档 Think Level** | `off`…`xhigh`，**未设=不发指令** | 模型被要求想多深（reasoning token 按 output 计费） | Panel pill + `chat.send{thinking}` + `self_config` + TUI `/think` | `src/agents/thinking.rs` + `execution_engine/turn_thinking.rs` |
| **记忆模式 Memory Mode** | `on` / `off`（默认跟 `[memory] enabled`） | 这一轮 prompt **注入**不注入 curated memory / 笔记索引 / 召回。**不闸工具、不闸写** | Panel pill + `chat.send{memory}` + TUI `/memory-mode` | `src/memory/session_memory_mode.rs` + `harness_bridge/prompt_build.rs`（唯一闸点）→ FEATURE_LOCATOR §5.23 |
| **模型 pin Model Pin** | 任意 model id（+可选 provider） | 这个对话此后用哪个模型（下一 run 起生效） | **只有 `select_model` 工具**（R8 对话式）——`sessions.patch` 明确拒绝该键。TUI 的 `/providers` 选择器**不是第二个写者**：它确认时发 `/model <id>` 这条网关命令，仍然落到同一个工具。Panel 的 `ModelPicker` **只显示不设置**（无 per-turn override 时 pill 印的就是 pin，否则它会报出用户刚换掉的那个模型） | `src/providers/session_model_handle.rs` + `gateway/session_model_pin.rs` + `execution_engine/turn_model.rs` |
| **繁忙输入 Busy Input** | `Steer`(默认) / `Interrupt` / `Queue` | 会话已有 run 在跑时新消息怎么办 | **per-channel 配置**（channel 实例配置块里的扁平键 `busy_input_mode`，经 `ChannelPolicyConfig` 解析）+ 三个写死的生产者（team run / OpenAI 兼容面 / 续跑，全钉 `queue`）。**Panel 靠手势而非旋钮**：`＋`/Enter = 客户端幽灵队列（≈Queue，且可 ↑ 撤回）· 轮边界自动 flush = Steer（服务端默认档）· `⚡`/Esc = abort + 重排（≈Interrupt） | `src/gateway/busy_queue/` → FEATURE_LOCATOR §4.8 |

> **别急着给 Busy Input 加参数**：三种处置在 Panel 上都已可达且各自正确，加一条 `busy_input` wire 参数会得到零消费者的通道（R10）。要改的是**手势与模式的对应关系**，不是新增旋钮面。

> **加第六根旋钮的清单**（前五根里有一根每一步都漏过，代价见 §5.23）：① `turn_*.rs` 孪生解析器；② `sessions.patch` 的 `knob_validators()`（census 会红）；③ `session_snapshot.rs` 的解码（census 会红）；④ 至少一个客户端面读得到它——**没有读者的 knob 和没人设的 knob 长得一模一样**；⑤ 如果它闸住了什么，那句话要同时出现在代码、doc 和**发给模型的 prompt** 里。

`[sandbox.command_policy]` 的硬底线**任何档位都压不下去**。

### 分发形态与信任模型

- **三产物**（同一 tag）: 完整桌面 App（内置 `aleph-server`，单机零配置）/ Aleph Panel 纯壳 App（连局域网 server）/ 独立 `aleph-server` 二进制。详见 [PRODUCT_TOPOLOGY.md](docs/reference/PRODUCT_TOPOLOGY.md)
- **信任模型 = 网络边界 + 登录墙**: 默认只绑 `127.0.0.1`；`[gateway] host = "0.0.0.0"` 显式开放局域网。loopback 免凭据恒 operator；远程须在 `connect` 出示 device token / 一次性配对票 / 共享 token 之一，**过了就是 operator，与本地完全一致——单层，没有 Chat/Config 子层**。协议护栏是 WS Origin 校验。详见 [SECURITY.md#auth-ux](docs/reference/SECURITY.md#auth-ux)

### Windows 构建

`just shell-build` / `just shell-dev` 在 Windows 同样适用（justfile 已守卫 macOS 专属步骤、自动追加 `.exe`），产物为 NSIS `.exe` + `.msi`。一次性前置依赖与全量构建步骤详见 [WINDOWS_RUNTIME.md](docs/reference/WINDOWS_RUNTIME.md)。

### 版本管理

- **CalVer** — 格式 `YY.M.D`（两位年，月/日不补零，如 `26.5.21`），每天最多发布一个版本。该格式同时是合法 semver 并满足 Windows MSI 版本约束
- **VERSION 文件是唯一版本源** — `build.rs` 读取 → 注入 `ALEPH_VERSION` → 代码用 `env!("ALEPH_VERSION")`
- **禁止**在代码中硬编码版本号，禁止用 `env!("CARGO_PKG_VERSION")`
- Panel System Info、Gateway 版本、MCP/ACP 协议版本、CLI `--version`、GitHub workflow release tag 全部读 VERSION 文件

### 发版流程

`just release YY.M.D` 触发三产物×三平台构建发布（发版前先写 CHANGELOG.md）。完整两步流程、`just verify-build` 预检、CI fail-fast 轮询详见 [RELEASE.md](docs/reference/RELEASE.md)。

### Feature Flags

所有生产功能始终编译，无需 feature flags。仅保留测试用 features：`loom`（并发测试）、`test-helpers`（集成测试工具）。

### 提交规范 / 分支策略

- English commit messages. Format: `<scope>: <description>` — 例：`gateway: add WebSocket server foundation`
- **单分支开发模式**：所有开发工作直接在 main 分支进行
- `EnterWorktree` 会话内只合并不删除（同会话 `git worktree remove` 会损坏 Shell）。详见 [CODE_ORGANIZATION.md](docs/reference/CODE_ORGANIZATION.md)

### 进程管理

Singleton 由 OS 级 `flock`（`~/.aleph/data/aleph.lock`）强制；CLI 写子命令经 `with_policy` 走 IPC 或本地拿锁，不与服务竞争。`kill -9` 后可立即重启。doctor 的 `core/duplicate-instance` 是运行时哨兵——**多进程竞争同一 vault → HMAC 失败 → vault 数据丢失**。详见 [PROCESS_MANAGEMENT.md](docs/reference/PROCESS_MANAGEMENT.md)。

### 内置文件与 Shell 工具

- **`file_edit`** 支持单 op（`old_string`+`new_string`）与多原子 op（`edits: [...]`）——多 op 全部匹配**原始文件**，重叠/非唯一立即拒
- **`file_write` 是 no-op 时跳过 atomic rename、保留 mtime**（`FileWriteOutput.unchanged`），增量 build 不被误触发
- **长任务（>3 min build/install）必须 `background: true`** —— `WAIT_MAX_TIMEOUT_SECS=170` 是 180s tool budget 的硬约束，**不要**尝试扩展（违反 R10）
- 全部现状与打磨记录见 [FEATURE_LOCATOR §3.4](docs/reference/FEATURE_LOCATOR.md#34-内置文件工具-builtin-file-tools)，改这几处前先看一遍

### My Working Style

- 先给方案再写代码；不确定时列出选项，不猜测（呼应 P1 与全局 CLAUDE.md）
- 重大变更前先问，小优化可直接执行
- 回复用中文，代码注释用英文，文档中英双语
- 按需正常使用 cargo（`check` / `test` / `clippy`）—— 编译与测试验证优先，不再强制节制调用次数

---

## 📚 文档索引 (Tier 2)

**总入口**: [FEATURE_LOCATOR.md](docs/reference/FEATURE_LOCATOR.md) —— 按 §编号组织的全项目现状库，判据清单里每个 `→ §x.y` 与 `→ 附录 C.x` 都指向它（**附录 C = 验证纪律全文**：这个绿是怎么骗你的——数字 / 仪器 / 扫描边界 / 闸 / 跑测试 / 命令陷阱）。

| 文档 | 说明 |
|------|------|
| [ARCHITECTURE.md](docs/reference/ARCHITECTURE.md) | 总体架构 |
| [PRODUCT_TOPOLOGY.md](docs/reference/PRODUCT_TOPOLOGY.md) | 一套源码 → 三产物排列组合 + 参考部署拓扑 |
| [HARNESS_PHILOSOPHY.md](docs/reference/HARNESS_PHILOSOPHY.md) | 薄 Harness + 笨循环（R10 详解、棘轮流水账） |
| [TWELVE_FACTOR_AUDIT.md](docs/reference/TWELVE_FACTOR_AUDIT.md) | 12-Factor 逐 factor 审计 + A1–A4 母本 + backlog |
| [AGENT_SYSTEM.md](docs/reference/AGENT_SYSTEM.md) | Agent 系统 |
| [AGENT_LOOP_CONTEXT_BUDGET.md](docs/reference/AGENT_LOOP_CONTEXT_BUDGET.md) · [AGENT_LOOP_TOOL_EXECUTION.md](docs/reference/AGENT_LOOP_TOOL_EXECUTION.md) · [AGENT_LOOP_RECOVERY.md](docs/reference/AGENT_LOOP_RECOVERY.md) | 循环三分册 |
| [GRAPH_LAYER.md](docs/reference/GRAPH_LAYER.md) | 循环治理图：六词闭集治理边 + 锚点/冻结/根参照 + 审计环；四种单循环失败的拓扑解法 |
| [MULTI_AGENT_SYSTEM.md](docs/reference/MULTI_AGENT_SYSTEM.md) | 多 agent / 团队 / 群聊直播面 |
| [GATEWAY.md](docs/reference/GATEWAY.md) | 网关、通道、投递队列 |
| [CLUSTER.md](docs/reference/CLUSTER.md) | 集群（单中心非对称节点联邦）+ `node_manage` |
| [TOOL_SYSTEM.md](docs/reference/TOOL_SYSTEM.md) | 工具系统 |
| [MODEL_CATALOG.md](docs/reference/MODEL_CATALOG.md) | 预设 provider/模型四表 + 单一 join 点 + 漂移守卫契约 |
| [MODE_SYSTEM.md](docs/reference/MODE_SYSTEM.md) | 会话模式 chat/work/code |
| [CANVAS.md](docs/reference/CANVAS.md) | 白板画布：四层架构 + 乐观锁并发协议 + 能力 URL 素材面 + iframe 沙箱边界 |
| [MEMORY_SYSTEM.md](docs/reference/MEMORY_SYSTEM.md) | 记忆总览 |
| └ [RAW_MEMORY.md](docs/reference/memory/RAW_MEMORY.md) · [NOTES.md](docs/reference/memory/NOTES.md) · [RETRIEVAL.md](docs/reference/memory/RETRIEVAL.md) | 三支柱分册 |
| └ [DREAM_DAEMON.md](docs/reference/memory/DREAM_DAEMON.md) | 离线做梦 + 自进化纪律（**`DreamGate` 已删，勿复活**） |
| [EXTENSION_SYSTEM.md](docs/reference/EXTENSION_SYSTEM.md) · [PLUGIN_SYSTEM.md](docs/reference/PLUGIN_SYSTEM.md) | 插件**运行时** |
| [ALEPH_HUB.md](docs/reference/ALEPH_HUB.md) | 扩展**分发**：目录契约 + 三道 ingest 闸 + 出处账本 |
| [WORKFLOW_INTEROP.md](docs/reference/WORKFLOW_INTEROP.md) | 工作流互操作 |
| [SECURITY.md](docs/reference/SECURITY.md) | 信任模型 + 工具权限三层 + 动作化审批门 |
| [AGENT_IDENTITY.md](docs/reference/AGENT_IDENTITY.md) | 每 agent Ed25519 密钥 + 签名哈希链账本 + 威胁模型（买到什么/买不到什么） |
| [DESIGN_PATTERNS.md](docs/reference/DESIGN_PATTERNS.md) · [CODE_ORGANIZATION.md](docs/reference/CODE_ORGANIZATION.md) · [DOMAIN_MODELING.md](docs/reference/DOMAIN_MODELING.md) · [AGENT_DESIGN_PHILOSOPHY.md](docs/reference/AGENT_DESIGN_PHILOSOPHY.md) | 工程规范 |
| [SERVER_DEVELOPMENT.md](docs/reference/SERVER_DEVELOPMENT.md) · [SESSION_SERVICE.md](docs/reference/SESSION_SERVICE.md) · [SANDBOX.md](docs/reference/SANDBOX.md) | 服务端 / 会话 / 沙箱 |
| [DESKTOP_BRIDGE.md](docs/reference/DESKTOP_BRIDGE.md) · [DESKTOP_SHELL.md](docs/reference/DESKTOP_SHELL.md) | 桌面 Bridge / 壳 |
| [WINDOWS_RUNTIME.md](docs/reference/WINDOWS_RUNTIME.md) | Windows 部署运维 + DPI + 刷新二进制链 |
| [LINUX_DESKTOP.md](docs/reference/LINUX_DESKTOP.md) | Linux 能力矩阵（X11 vs Wayland）+ 诊断顺序 + 验证状态诚实标注 |
| [MODEL_PERCEIVABLE_ECOSYSTEM.md](docs/reference/MODEL_PERCEIVABLE_ECOSYSTEM.md) · [SKILL_TRIGGER_ENHANCEMENT.md](docs/reference/SKILL_TRIGGER_ENHANCEMENT.md) | 生态可感知 / Skill 触发 |
| [GOOGLE_MEET_BRIDGE.md](docs/reference/GOOGLE_MEET_BRIDGE.md) · [WHATSAPP_ARCHITECTURE_DESIGN.md](docs/reference/WHATSAPP_ARCHITECTURE_DESIGN.md) | 单点集成设计 |
| [RELEASE.md](docs/reference/RELEASE.md) · [PROCESS_MANAGEMENT.md](docs/reference/PROCESS_MANAGEMENT.md) | 发版 / 进程管理 |

> **官方 skills/plugins 离线兜底**: 根目录 `skills/` 与 `plugins/` 是两个 git submodule（upstream = 兄弟仓 Aleph-skills / Aleph-plugins），经 `include_dir!` 在 `aleph-server` **编译期嵌入二进制**（`src/bundled/mod.rs`）。首次安装优先 git clone 上游；**网络故障时回退到这份嵌入快照**。**勿删这两个目录**——`include_dir!` 是编译期宏，目录缺失直接编译失败，并连带破坏 `build.rs` rerun / CI `submodules: recursive` / `justfile` 发版重嵌链。

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

> **生态统一管理约定**: 以上 7 仓为同级兄弟目录，远端均在 `github.com/rootazero/`。**始终从主项目 `Aleph/` 启动会话**，周边仓作为兄弟目录就地操作——这样跨会话长期记忆统一沉淀到主项目的全局 memory 库（按工作目录路径编码），spec/plan 统一落在主项目的 `docs/superpowers/{specs,plans}`（整个 `docs/` 树已纳入 git 版本管理，新建 docs 默认被跟踪，不再需要 `git add -f`）。周边仓的 spec 以子项目名作文件名前缀（如 `2026-06-23-aleph-mcp-xxx.md`）。避免直接进周边仓启动会话导致记忆库分裂。

---

## 🧠 长期记忆与质量门 (Memory & Hooks)

- **长期记忆**: 走全局 `~/.claude/projects/.../memory/`（跨会话、Git 不追踪）。**不在项目内另造 MEMORY.md**——避免与全局记忆双源冲突。
- **质量门 (Hooks)**: 当前**未挂** `.claude/hooks/`。本文件里的规则目前靠模型遵守；未来如需"强制执行层"（如 PostToolUse → `cargo fmt`），在 `.claude/hooks/` 配置即可。
