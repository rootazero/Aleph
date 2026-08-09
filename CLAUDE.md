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

- **恒真的谓词等于没判，且它撒的谎只有对面看得见** —— 「有没有 handler」是结构性恒真，「这件事真做得到吗」才是谓词
- **零消费者的通道优先 CUT，不 CONNECT** —— R10；接一条死抽象比删它贵
- **守卫要断言「效果到达了」，不是「调用发生了」** —— 问「把这一步的返回值扔掉，测试还绿吗？」绿 ⇒ 你守的是产地不是连线 → §3.5
- **列举法只覆盖立法当天的世界** —— 白名单式判据（"这算不算合成消息"/"保真字段集"/"受支持的维度"）必然漂移；改问**这段字是谁写的**、**不在我这张表上的那部分呢**。⚠️ **当"默认＝全都要"时，重放一份清单不是恢复而是收窄**：判据要倒过来问「我这次重放，把哪些原本会到达的东西挡在外面了」（Panel 重连只重放 3 个字面量 topic，把"无 filter ⇒ 收全部"的新连接压成只收那三类，`stream.*` 自此静默死亡而连接灯是绿的）→ §6.1
- **两条投影同时喂同一个 append-only 状态，就会翻倍** —— 一份事实在线上有两种投影（权威流 vs 有意有损的镜像）时，消费者往往被写成"每条投影都是唯一来源"。加一个消费分支前先问**这条事实还会不会从另一条路再来一次**，以及**我这个容器是覆盖式还是追加式**（覆盖幂等、追加翻倍）。**测试不会告诉你**：单投影测试各自全绿，而两条投影从不在同一个测试里交错——那正是生产中唯一存在的调用序 → §5.13
- **一句关于"什么被闸住"的话，往往有三份拷贝，其中一份是发给模型的** —— 代码里的地板、doc comment、以及每回合进 prompt 的那句描述。改地板时三份一起改；**发给模型的那份说了假话最贵**（`Full` 档告诉模型"nothing pauses for confirmation"，而工具自声明的确认门根本不看档位）→ §5.12
- **同一事实的两份表述，只改一份就是静默说谎** —— 代码 vs 工具 `DESCRIPTION`、代码 vs 文档数字、代码 vs 注释；**注释正是说谎的那一方**
- **真源必须在被依赖的一侧** —— 依赖方向 `linux → shared` ⇒ 真源在 `shared/`；反着放就是把同一个问题回答 N 次
- **契约的两半住在两个 crate 里时，"有测试"这件事本身会骗人** —— `aleph workspace create|archive` 自写下之日起每次调用都 `INVALID_PARAMS`（CLI 发 `{"name"}`，handler 要 `id`），而 CLI 那侧的测试断言的是 `json!({"name":…})["name"] == "test-ws"`：**一个只读自己刚写下的字面量的断言，测的是 serde_json 不是你的代码**，它永远绿。判据：跨 crate 的 wire 契约要么**共用一个类型**（重命名 ⇒ 编译错；单一源 `aleph_protocol::workspace`，`aleph-cli` 按设计不许依赖 `alephcore`），要么在**依赖两边的那一侧**留一条真正对账的测试（`workspace.rs::every_column_the_cli_renders_is_present_in_the_list_response`，用改 wire key 的变异证过 RED）。**同族：展示列也要对账**——那张表读的 `status`/`created` 服务端从来没发过（真名 `is_archived`/`created_at`），于是每行印一列破折号，看起来只是"还没有值"
- **fail-soft 的跳过不是「不存在」的证据** —— `Ok(None)` 给读者防卡死用，拿它当 DELETE / 放行判据就是不可逆损坏；**按状态做的闸，`Err` 必须是拒绝不能是放行**
- **「被拒」不许读作「没有」** —— 上一条的显示面镜像（同族还有 §8 的「未知不许读作健康」）。一个 `Err` 被折成值（`Some(false)` / 空列表 / 空字符串）之后，UI 就在替服务器**发明一个它从未说过的答案**，而最贵的那种是**自信的假话**：引导清单把 admin 拒绝读成"没配置",于是对着一个配好的 provider 喊 `PENDING Configure a chat provider` 并邀请点进用不了的页。判据一句话：**只有 `Ok` 有资格断言被读的那个东西**；`Err` 的每一种（拒绝 / 断线 / 解析失败）都只能说"我不知道"。单一源 `interfaces/webchat/src/components/admin_refusal.rs`（识别咽喉，非权限判定——Panel 刻意不持有客户端角色谓词，见 `context.rs` 的 `role` 字段注释）
- **一个动词有 N 个面时，"谁能看"要在每个面用同一个推导** —— 别在 RPC 面写 `visible_owner_filter()`、在事件面写角色臂：**operator 的 `CALLER_USER` 是 `OWNER_USER_ID` 而不是 `None`**，所以 owner-keyed 谓词对 operator 也会生效。两面分歧的症状比"看不见"更怪——事件面放行、列表面过滤，而 Panel 每收一帧就按列表面重建 ⇒ **卡片到达后当场消失**。单一源 `caller_identity::caller_is_member`（＝ admin 闸自己的谓词）↔ `event_scope::is_superuser_scope`
- **隔离环境的 QA 结构上只测得到「新建的对象」** —— 干净 HOME 里没有存量，于是**迁移前写下的行**（缺列、缺戳、旧单位）整类测不到，而真实部署里那才是多数。补法是把已有行改成迁移前的形态再开机，**不是**让 fixture 造一个"看起来像旧的"状态 → §5.22
- **拒绝形状做得越好，时序 bug 越像安全行为** —— no-oracle 要求"拒绝"与"不存在"逐字节相同，代价是**异步写盘的行**在写盘前被读到时，给出的也是同一个 not-found。看到它先问「这一步是不是还没落盘」，再问「是不是没权限」 → §5.22
- **一个身份/谓词有两半，branch 一半等于没 branch** —— 一个动词的两张脸（工具 vs RPC）必须共用判据**也共用推导**（如 `workflow_step_review` ↔ `teams.workflow.approve_step` 共用 `verdict_admissible`）
- **先数这个能力有几张脸，再决定谓词放哪** —— 「两张脸」只是最常见的数目，不是上限。2026-08-08 一轮里同一个问法抓到四种不同的漏面：**一条连接有两个方向**（登录墙只挂在请求臂，事件臂四项判据无一是身份 ⇒ 未认证 socket 收得到 operator 的 shell）· **一个谓词有两种取 actor 的方式**（`CALLER_USER` 在 spawn 出的 run 里是死的 ⇒ 工具面照文档接现成谓词会拿到静默恒真）· **一个前缀下装着两类帧**（按 topic 前缀键控的表只能一次答完两者）· **一个能力有服务端和客户端两半**。→ §5.22 round 2
- **没有客户端的能力不算已交付，服务端那半再完整也不算** —— `users.create`/`users.update` 完整实现、注册两遍、admin-gated、pin 齐备、接了吊销管线，**全仓零调用者**：三期多用户机件因此在出厂形态下整体不可达。提交一个 RPC 家族前先问**谁调它**，答案必须指得出一个 shipped surface（同族：§5.21「一个"展示用"字段在提交前必须能指出渲染它的那一行代码」）
- **把人挡在门外不等于约束了他，可能只是把他推过了门** —— 审批面对 member 两面全关 ⇒ 他的 run 死在 120s 超时，而文档记录的解法是把档位拉到 `full`：最不安全的档位成了唯一能用的档位。**一个把用户往宽设置上推的权限系统已经反转了自己的目的。** 加闸时问「被闸住的人接下来会干什么」，不只问「这道闸拦住了什么」→ §5.22 round 2 ③
- **一个 fail-closed 的答案被当成值消费，就会反转成许可** —— 装饰器对外人返回的 `Ok(None)` 被 `.ok().flatten()` 折进 `leader: Option<_>`，再读成「没有 leader ⇒ 谁都能审」。闸要跑在折叠**之前**：`Ok(None)`（拒绝）一旦和「这东西本来就没有」合流，两者永远分不开。凡 `.ok().flatten()` / `unwrap_or_default()` 落在**装饰器**返回值上都要问：这个默认值和那个拒绝长得一样吗 → §5.22 round 2 ⑤
- **第二个构造点默认继承不到第一个的任何档位** —— 加档位时 grep 它的 `::new(` 有几个调用点；**反向也要判断**，不是每个 builder 都该补齐，理由写进构造函数 doc → §2.19
- **「先认领、后执行」的调度，界限要在执行时刻成立** —— 只在入队处判等于没判（`loop timeout_minutes` 曾在上限之后多跑 119 分钟）。**推论（适用于任何长跑单元）**：凡「先认领、后执行」的调度，界限要在**执行时刻**成立；只在入队处判等于没判。**2026-08-05 已在 goal 的 wait-barrier 上复发一次**——同一个形状，第二个子系统：那里绕过 claim 的 boot rearm 路径连一道界都没有，所以「谁绕过了认领，谁就得自己带界」是这条纪律的第二半。→ §4.1/§4.2
- **「先记录意图、再做不可逆动作」：只记录"做完了"的机件，分不出"没做"和"做了但没记上"** —— 跨越不可逆边界**之前**盖持久戳，新进程拿到状态那一刻按"结果未知"退休 → §5.6
- **一次性的章不能在动作确认之前花掉** —— 要么事后盖，要么必须可归还
- **一次性的动作，哪个面执行了哪个面就是唯一机会** —— 工具面触发、RPC 面不触发，事后补不回来
- **同一件事有两个 id，就等于没有寻址路径** —— 进程内句柄与持久记录键各自现造 UUID、互不指认时，那份数据**在库里但够不到**，症状是"查无此物"而非报错（`subagent` 的 `request_id` vs `SubagentSpawned.child_id`）。判据：**这个 id 在写它的那条记录上出现过吗**？位置/时间关联不是替代品——同一 turn 的并发兄弟共享 `turn_id`，按顺序对齐必然串台 → §4.11
- **纯内存的注册表在进程消失后不是"空了"，是"撒谎了"** —— 它对已完成的工作回答"从来没有过这个东西"，而调用方的合理反应是**重做**。能力对标时逐行问**这张表在进程消失后还成立吗**：只比内存内的生命周期管理（容量／LRU／TTL）会让整张表漏掉这一维（§4.11 round-10 漏了一整轮）→ §4.11 §4.13
- **崩溃边界上的"未知"不能写成"失败"** —— 派发前已落盘的意图 + 没有应答 ⇒ 副作用**可能已经落地**；说"它失败了"等于请求重复执行不可逆操作。陈述认知状态，重做与否归模型 → §4.13
- **修好一处，会让它下游从没跑过的路径第一次真正跑起来** —— 那不是回归，是**新暴露面**，值得在同一轮重审
- **跨会话只能"降活跃度"** —— quiet（stop/pause/clear）可跨，arm（start/resume/加配额）必须在该单元自己的会话跑 → §4.1/§4.2
- **窗口以天计的检测器必须持久化** —— 把窗口长度 × tick 周期，跟进程实际寿命比一比；量级接近就必须落在进程之外 → §2.8
- **同构分区有 N 个，维护默认只覆盖第一个** —— 写路径经合成 id 分区（`{base}__proj-*`/`__u-*`/`__p-*`）之后，每一条"遍历全部"的维护/对账/枚举路径都要重问「它遍历的是 default 那一个，还是全部」。已在**两个**子系统上各犯一次：做梦历史（§2.8）与笔记索引开机对账（§2.5）。**枚举必须有单一源**（`project_scope::list_note_corpora`；此前同一问题有三份互不一致的答案），且**脚手架（写）不得跟着对账（修）一起 fan-out** → §2.5 §2.8
- **DEFER 若建立在「这条路走不到」上，就欠一次真实负载实测** —— 猜出来的边界只在边缘塌陷，而边缘正是真机所在 → §3.14
- **能被精确回答的数字别用常量猜** —— 先问仓库里有没有人已经知道它；换算单位有没有单一源 → §4.9
- **把一个字段升格成运行时能力之前，先数它有几个写入者** —— 休眠的展示字段（`workspace_path` 曾只是 picker 里的一行字）一旦接上运行时权力（成为 run 的 cwd），它的**每一个**写入者都追溯成了权限授予点；写入者与读取者必须同批过同一道闸，否则「两步都合法、合起来等价」（先注册目录、再进房间聊天）就是绕闸路径 → §5.22

- **在构造期解析的身份，是一个没有生产者也照样"成功"的身份** —— `unwrap_or_else(|| "main")` 之类的兜底把一个**字面量焊进每一个消费者**并持续整个进程寿命，而**错误的身份是完全合法的身份** ⇒ 零报错、零测试红（`researcher` 领导的团队拒绝自己的 leader 并接受 `main`；审批全记在 `main` 名下）。判据两句：① 这个字段**有没有生产者**（grep 赋值点，不是 grep 类型）；② 施动者该由**这次调用**决定还是由**进程启动**决定——是前者就从 `TURN_CONTEXT` 每次取，构造参数只配当 fallback（单一源 `builtin_tools::acting_agent`）→ §4.13
- **进程内存不是状态：凡"重启后这个 id 还查得到吗"答不上来的表，都欠一个 sidecar** —— 而且**只记录"做完了"的机件分不出"从没跑过"和"跑了但写丢了"**，所以开机对账要写**终态墓碑**而非删行（否则 not-found 同时意味着"你打错了"和"它随上个进程死了"，还顺手扔掉死前的产出）。**推论**：产物一旦跨进程落盘，脱敏就不能再按 run 的 attendedness 决定——读它的是**后来那个进程**，而它可能把内容扇出到聊天通道 → §4.13 §5.1

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

- **能力接上了 ≠ 模型会用它** —— 加/删任何 capability 的同一笔改动里必须 grep 工具 `DESCRIPTION`（prompt 在劝模型别用，比缺失更难发现）→ §5.17
- **⚠️ 分类器已经存在，只是没人问它**：`block_goal_on_failure` 曾把**任何**失败都判成 goal 的终态（Blocked + 删焊入计划 + 推「已中止」），而 `ExecutionError::receipt_kind()` **早就是三个用户面共用的单一源**，其 doc 自陈：此层出现限流/网络签名意味着整条 provider 链都试过、失败**确属瞬时**。同一仓里 `llm_retry::extract_retry_after_str` 能给退避时长、wait-barrier 能 park 自唤醒——**三块零件齐备，谁都没连**。判据一句话：写下一个 `if error { 终态 }` 之前先问**「这个错误已经被谁分过类了」**；答案通常是"有，而且比你打算写的那版更准"。同族是 §4.9「能被精确回答的数字，别用常量猜」——那条讲魔数，这条讲**分支**，两者都是「已知事实就在同一个 crate 里，零调用」。
- **目录条目写字面量会整体遮蔽工具常量** —— `BUILTIN_TOOL_DEFINITIONS` 的 `description:` 必须指向常量；守卫 `definitions.rs::tests::no_catalog_entry_inlines_its_description` 是**源码级**的（运行时分不出"来自常量"和"恰好字节相同"）。现在要问的是**这些字节值不值**：`catalog_description_bytes_ratchet` 实测 81,274 B，**每个请求都付**（`truncate_tool_descriptions` 默认 `false`，没有任何一档配置让它免费）→ §5.17
- **deny 检查有方向** —— `path_is_denied` 只向下问「我在不在保护区里」；还要 `contains_denied_descendant` 向上问「我下面有没有保护区」，且必须共用同一份展开+归一化。会**遍历**的动词（copy/move/delete/organize）顶层闸永远不够 → §3.4
- **取消不是判决** —— 墙钟超时 / 传输抖动 / 审批过期 / 用户取消，四者都不是"关于这次调用的失败"；归因在派发咽喉 `scoped/dispatch.rs`，用**成功态**表达"被打断" → §3.3
- **会 park 的工具必须听取消令牌** —— 这个 await 的最长睡眠时间就是取消的最坏延迟；超过一两秒必须进 `tokio::select!` → §4.11
- **进程全局的表，新增**枚举**入口时要问「调用者凭什么看见这一行」** —— 模型能拿到的 id 只有两个来源：自己 spawn 的返回值，和枚举面；`list` 是目录不是内容 → §4.11
- **入参规范化要落在参数的解析处**（一个 `resolve_*` 边界），不是某个 handler —— 否则同一个模型对同一个分类得到互相矛盾的答案
- **执行清单单一形状是 `shared/protocol/src/plan.rs::PlanSnapshot`** —— 分解 100% 归 LLM（R7），**不要新建 `todo` 工具**（Panel 按字面工具名 `"scratchpad"` 取数）→ §3.13
- **执行档位唯一强制点是 `src/tools/scoped/`** —— 任何新的能执行工具的 surface（新 RPC / 快路径 / 后台产地）不经过它就自带旁路 → [SECURITY.md](docs/reference/SECURITY.md) §5.12
- **参数级审批闸只在能举卡的 surface 上成立**，且**举给人的那张卡必须包含闸所依据的那个字段**（按字典序渲染 + 200 字符截断会把被闸字段挤掉）；「操作者显式点名了这个工具」在代码里必须是**精确匹配**，不能用会匹配 glob 的查找 → §4.12
- **闸的范围必须覆盖「能把这个闸拿掉的那个动词」** —— 还要问「有没有两步都合法、合起来等价的路径」 → §4.12
- **命令硬底线扫的是 `normalize.rs` 的规范化副本**，不是你写的那行字（两份视图 / `-enc` 载荷已解码回注且关不掉 / 规则间隙用 `seg!()` 不是 `[^\n]*`）→ [SANDBOX.md](docs/reference/SANDBOX.md) §3.8

### 4. 网关 · 通道 · 投递（`src/gateway/`）

- **「至多一次」只覆盖了「传输层报了错」那一半——进程消失是第三种结局** → §5.6
- **「按表断言的安全性」在多了第二个生产者之后就作废** —— 安全位必须随记录走（`DeadLetterReason::replay_safe`）→ §5.6
- **队头退避必须带上队尾**（只在一个 batch 内做队头阻塞，跨 tick 就漏，保序**永久性**坏掉且零报错）→ §5.6
- **上限要量对量纲** —— 数行不数字节的 CWE-400 防御，对能装内联媒体的表等于没设 → §5.6
- **「保序」只在慢路径上实现等于没有** —— 让快路径先把慢路径排空（`flush_conversation` 跑在 `send_attempt` 之前）→ §5.6
- **机会性探测不能花别人的预算** —— 瞬时失败原样放回，**但歧义终态照旧结算** → §5.6
- **一条 durable 记录不能比它引用的东西活得久** —— 按引用持久化时字节上限量的是引用而非被引用物；托管在唯一准入咽喉取得，优先把字节收进记录自身 → §5.6
- **「状态」回答不了「该不该自愈」，缺的那一半是意图** —— 先问「我凭什么认为它现在应该是好的」，答案不在被检测方的状态里（真源是 `ChannelRegistry::DesiredChannelState`）；**别在退出路径上无条件覆写 status** → [GATEWAY.md](docs/reference/GATEWAY.md)
- **加了通道 adapter ≠ 用户能配** —— 必须手工进 `gateway/interfaces/plugin.rs` 的工厂表（`register_plain_channel!`）→ [GATEWAY.md](docs/reference/GATEWAY.md)
- **`ChannelCapabilities` 的每个位都是承诺** —— 声明了就必须覆写对应的 `Channel` 方法（默认体一律 `Err` 并指名道姓）；频道寻址是**两步**（先 `channel_directory` 换 id）→ §5.18
- **车道是候车室，不是运行登记簿** —— 取槽成功时必须 `busy_queue::mark_admitted`，否则 `Steer`/`Interrupt` **静默退化成 `Queue`**（三件事一起修或一起坏）→ §4.8
- **「提前返回的快路径」会静默吞掉请求上除它自己之外的一切** —— 判据不是「这条路径会不会崩」而是「它跳过了哪些本该发生的解析」；新增 per-request 指令字段必须在 `steering.rs::carries_more_than_text` 登记。**一个方向被想到、反方向没有**是这类缺陷最常见的形状 → §4.8
- **`devices` 是 panel 与 cluster 共用的一张表，`device_id` 两边都是对端自报的** —— 任何「按 id 认领一行」的新路径必须先拒掉另一半命名空间（判据单一源 `PANEL_DEVICE_TYPE`）→ [SECURITY.md#auth-ux](docs/reference/SECURITY.md#auth-ux) · `src/gateway/CLAUDE.md`
- **团队群聊投影前必须先按当前 `chat.team_id` 作用域** —— 订阅是 `team.*` 通配，否则后台团队的气泡挤进任意会话 → §4.5
- **新增 `read_*` 一类只读 RPC 记得进 `gateway/lane.rs::override_for`** —— 后缀启发式不认它就落 Mutate 车道被幂等键守卫拒掉 → §6.8
- **「谁拥有」回答不了「谁能看」** —— 共享房间的 `owner_user_id` 记的是**创建者**，只裁 owner-only 动词；可见性判据是名册（`projects::roster::is_member`，经 `visibility::project_visible` / `session_visible_to` 到达）。凡给一张表加了共享语义，`owner` 列的含义要重新问一遍——拿它答 can-see 就是 P2 修掉的那类 bug 复发 → §5.22
- **每个可见性谓词都欠一个显式 actor 孪生，因为工具面取不到 task-local** —— `visible_owner_filter()` 读 `CALLER_USER`，而它在 spawn 出的 run 里恒 `None`，**每一次工具调用都在里面**：照文档接现成谓词的工具作者拿到的是**静默恒真**。孪生是 `session_visible_to` / `partition_visible_to` / `project_visible_to`，工具一律传 `scope::ambient_owner()`。新工具碰 per-user/per-room 数据时先问「这个谓词的 actor 从哪来」→ §5.22 round 2 ⑤
- **谓词改了、下推的过滤器没改，症状是「能进去但列表里没有」** —— 寻址面的 `visibility::session_visible`（task-local）与列表面的 `visibility::session_visible_to`（显式 actor，两个 backend 的 `SessionFilter::owner_visible_to` 都经它）是同一个判据的两张脸，必须同批改；grep 内存谓词找齐所有下推点，只改一半的那半是静默的。⚠️ **`..Default::default()` 会把这个字段留成 `None` = 全体 owner**：`session_list` 工具就是这么漏的，而 RPC 孪生一直设着它。⚠️ **刻意没有 `SessionFilter::visible_scope_ids`**：房间可见性不下推成 SQL 列表，而是由两个 backend 共用的内存谓词裁决——想加 SQL 下推请先读 `session_visible_to` 的 doc → §5.22
- **投影必须由真源在自己的写锁里发布** —— `projects::roster` 是 `project_members` 的进程内投影，发布点在 store 的写锁**内**（变更 + 快照 + 发布同一个 `with_conn` 闭包，`republish_roster_locked`）。「写完再发」的两次取锁不是原子的：并发写会按提交的**反序**发布，输的那次把已删成员复活到下一次名册写入为止，而 `is_member` 是房间授权的**全部**判据 ⇒ fail-open。第二个写入者就是第二个真源，CLI 要改名册必须走 IPC 而不是直开数据库 → §5.22
- **多设备共享的事实不能住在 `localStorage`** —— 判据是**这个值对第二台设备还成立吗**；「房间用哪个会话」曾按 `project_id` 存在每个浏览器里，于是第二个成员进房什么也没找到、开了自己的会话，两人共享记忆分区与工作目录却**永不同框**（任何界面、任何刷新都看不见对方）。真源是 `projects.current_session_key` + `projects.room_session` 的原子认领 → §5.22

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
- **markdown 是真源这句话，得有人在运行时执行它** —— 每个 agent 目录都被写入 `.obsidian/` vault 配置（＝明说"去 Obsidian 里编辑"），所以外部编辑/删除必须有监视器回流（`notes/watcher.rs`）；**监视根先 `canonicalize`**，通知回报的是规范路径（macOS `/var`→`/private/var`），拿未解析的根 `strip_prefix` 会把整个 vault 判成"不是笔记"——在跑、什么都不做、零症状。**动作由文件系统当前状态决定，不由事件类型**；非 `NotFound` 的 stat 错误必须跳过（另一条分支是删除）→ §2.5
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

- **「按会话的状态」住进单例组件 ＝ 切页签就串味** —— 判据：**这份状态在用户切到另一个对话后还成立吗**？不成立就进 `SessionSnapshot`，边沿按 `ConvId` 记（单一源 `shared_ui_logic::state::composer_queue::was_busy_across_switch`）→ §4.7
- **但「进了 `SessionSnapshot`」只是必要条件** —— `activate`（快照往返）**紧接着一行** `clear_session()`，给快照加字段时必须走一遍「恢复之后还会跑什么」；**「A 之后紧跟 B」时算数的是 B**，两段代码各自都对，只有真机看得出来 → §4.7
- **别再造"写一个信号、指望别处排空"的预填通道** —— 多平台下必然漏一个消费者且**零报错**；草稿只有一个家 `ChatState.draft`，唯一入口 `seed_draft`（合并不覆盖，这个 composer 没有 undo）→ §4.8
- **交付物 ≠ 聊天记录** —— `artifact_publish` 的成品 vs `session.export_html` 的 transcript；什么算成品 **100% 归模型判断（R7）**；导出文档**零 `<script>` 是硬约束**（`src/export/page.rs`）→ §6.8
- **右栏默认是收起的**（`LayoutMode::ChatOnly`）—— 长在面板里的提示在那个状态下等于不存在；徽标必须数**面板真正装的东西**；「一行能点开什么」的谓词 offer 侧与 serve 侧必须读同一份（`PreviewTarget::for_item` ↔ `is_previewable_text`）→ §6.8
- **Panel UI 编译期嵌入二进制**（`rust_embed`）—— 改完看不到效果 = 漏了重编 server → [DESKTOP_SHELL.md](docs/reference/DESKTOP_SHELL.md)

### 8. 配置 · 诊断 · 自管理 · Hook

- **一句关于运行时的承诺，必须由**每一条**到达它的路径执行** —— 先问**这句话是谁执行的**，再问**是不是每条路径都会执行它**（第二问才是这类缺陷的家）；执行点收进唯一写咽喉（`config/live_apply.rs::apply_live_sections`，执行的就是声明用的那张表 `reload_impact.rs::LIVE_SECTIONS`）；**声明要能被降级**——恒真的声明等于没声明 → §5.8
- **传感器不许创造它测量的东西** —— 诊断 / 审计 / 只读 RPC 一律不能用会建目录的路径 helper（`get_config_dir()` 是纯查找，`get_data_dir()` 不是）→ §5.9
- **「未知」不许读作「健康」** —— 没有死线的检查会把沉默伪装成健康（`doctor` 跑在 agent 回合里 ⇒ 挂住整个回合，唯一症状是沉默）；超时折叠成**指名道姓的 Warning** → §5.9
- **脱敏这类"每条输出都必须过"的闸要下沉到咽喉** —— 那是唯一能替"作者根本没想过凭据"的检查兜住的位置 → §5.9。**先数这个东西有几条腿**：unattended 脱敏曾只包 `TraceSink`，而 run 的输出还从 `EventEmitter` 那条腿出去并被 `OriginFanoutEmitter` 明文投进 Telegram——同一段 final text 在同一个 run 里一边打码一边明文 → §5.1
- **hook 注册了 ≠ hook 会触发** —— 三个各自独立的静默死因（matcher 挂在无 `tool_name` 的事件 / interceptor 挂在只派发 observer 的缝 / consent 仍 pending）；**唯一诊断入口是 `hooks_manage(action='list', only_unreachable=true)`**（运行时视图，不是 `~/.aleph/hooks.json` 那个文件视图）；consent 绑的是**脚本内容**不是命令字符串；**任何工具都不能批准 hook** → §5.10

### 9. 外部集成（MCP · Hub · Provider 路由 · 媒体）

- **MCP 有两个纪元，且纪元是 server 的属性不是请求的属性** —— `connection.rs::probe_era` 探一次闩进 `OnceLock`；判据只有一条（错误码落在 `-32020..=-32099` ⇒ modern）；HTTP 上**必须**把带 JSON-RPC error body 的 4xx 当协议应答返回 → §5.20
- **三个咽喉别绕开** —— 请求只能由 `connection.rs::request()` 造；`Mcp-Method`/`Mcp-Name` 由 `http.rs` 从正要发出的 body 现推；服务端发起的 sampling/elicitation/roots 全走 MRTR。**`resultType` 缺省必须读作 `complete`** → §5.20
- **声明能力＝承诺** —— `can_sample` 的谓词必须是 `handler.has_callback()`（宿主实现 `mcp/sampling_bridge.rs::serve_sampling`，**必须懒解析**，回调要在**任何 transport 启动之前**装上）→ §5.20
- **订阅事件流来建状态的机件，必须在订阅之后对账一次** —— 问「我订阅之前发生的事，谁告诉我？」；boot 恰恰把一切放在订阅之前（曾让**每一台**配好的 MCP server 的工具在每次启动后都进不了注册表，而 `mcp.list` 报 healthy）。顺序必须是先 `subscribe()` 再对账。**纯通知型订阅者不适用** → §5.20
- **Hub 只消费不策展** —— 目录槽是 **replace 语义**，可疑 artifact 会静默覆盖 last-good ⇒ 校验必须在**任何条目进缓存之前**；**给用户看的数字必须有校验者**（`sha256`/`git_ref` 曾展示却从不校验）；**`installed` 与 `update_available` 是两个不同的真源**且生产者必须落在消费者那条路上 → §5.21
- **一个"展示用"字段在提交前必须能指出渲染它的那一行代码** —— 指不出就是 CUT，不是"以后再接" → §5.21
- **加 hub 工具要动五处登记**（`hub/mod.rs` + `definitions.rs` + `groups.rs` + constructor 的**构造段和 schema 段**两处 + dispatch）—— 漏 schema 段＝注册了但模型看不见，漏 dispatch＝看得见但调不到 → §5.21
- **「动态路由」是三件事，且第三件不在 `src/routing/`** —— 工具选择（prompt，禁止意图分类）/ 消息→agent（`src/routing/`）/ 请求→provider（`src/providers/route_policy.rs`）。业界那些"路由大脑"整类违 R7，**不移植** → §3.6
- **判断"这是主槽吗"只认 `SlotKind`，绝不能拿 `tier == Unknown` 当代理**（两个方向都错过）；**装饰器少一层委托，整条链的能力就没了**（门是 `AiProvider::supports_streaming()`）；**「这一轮用哪个模型」只有一个决定点** `runner_impl.rs::effective_model_directive`，**别让新来源止步于 UI** → §3.6
- **状态码判断一律走 `llm_retry::has_status_code`** —— `contains("401")` 会命中 `40123` 这种 token 计数 → §3.6
- **同一个能力有两套栈时，先问「工具接的是哪一套」** —— 单一源 `src/media/resolve.rs::transcription_service` 被 `agent_init` 与 registry 构造器共用；**编进散文里的参数等于没传**（`language` 是原生参数）→ §7.6
- **语音模式经进程级注册表摆渡，写入点有两个都要写**（channel 侧 `inbound_router/executor.rs` + Panel 侧 `handlers/agent.rs`）—— 漏一个，那个 surface 的语音回合永远拿不到口语风格层 → §2.4

### 10. 构建与验证

- **`cargo check` 不编译 `#[cfg(test)]`** —— 删 `pub fn` / 字段的同一笔里必须跑 `cargo test --no-run`；只跑 `cargo check` 等于没验证 → [CODE_ORGANIZATION.md](docs/reference/CODE_ORGANIZATION.md)
- **最小可信验证集是五条命令，不是一条**：
  ```
  cargo test -p alephcore --lib --no-run
  cargo test -p alephcore --features test-helpers --test '*' --no-run   # --all-targets 只展开 target 不展开 feature
  cargo check -p aleph-panel                                            # -p alephcore 永远看不见这个 crate
  cargo check -p aleph-desktop-{macos,windows,linux}                    # 跨平台改动要 check 那个目标的限肢 crate
  cargo clippy --all-targets                                            # examples 只有它才暴露
  ```
- **`interfaces/webchat/` 有任何改动（哪怕不是你改的）就跑一次 `cargo check -p aleph-panel`** —— 这个 crate 的**语义合并冲突是常态形状**：一侧的类型 + 另一侧的调用点，git 不报冲突、两边单独看都完整。合并实现过同一功能的分支前先 grep 功能名；修完**先看警告再看错误**（`unused variable` 说明那半边根本没有调用者，正解是 CUT）
- **`cargo check -p aleph-desktop-shell` 前需先 `just _stage-shell-placeholders`**（tauri-build 要求 externalBin 占位文件存在）
- **`MessageRecord.timestamp` 单位有歧义**（SQLite 写秒 / file backend 写毫秒）—— 一律走 `MessageRecord::instant()` / `rfc3339()`（`src/gateway/session_store/types.rs`，1e11 分界），裸格式化就是这个 bug 的下一次复发。**源头未改是有意的**——该值同时是 `get_history_before` 的分页游标，改单位要连全部存量会话一起迁移

---

## 📍 子系统路由 (Read Before Editing)

| 你要动的目录 | 先读 |
|---|---|
| `src/harness/` | [HARNESS_PHILOSOPHY.md](docs/reference/HARNESS_PHILOSOPHY.md) · `src/harness/CLAUDE.md` · FEATURE_LOCATOR §3.1 |
| `src/thinker/` `src/context/` | 判据清单 §1 · FEATURE_LOCATOR §2.3 §2.18 §2.19 |
| `src/tool_output/` | 判据清单 §2 · FEATURE_LOCATOR §2.7 §3.14 |
| `src/tools/` `src/builtin_tools/` | [TOOL_SYSTEM.md](docs/reference/TOOL_SYSTEM.md) · [SECURITY.md](docs/reference/SECURITY.md) · §3.2–§3.14 |
| `src/gateway/` | [GATEWAY.md](docs/reference/GATEWAY.md) · `src/gateway/CLAUDE.md` · §4.8 §5.6 §5.18 |
| `src/memory/` `src/note/` | [MEMORY_SYSTEM.md](docs/reference/MEMORY_SYSTEM.md) + memory/ 三分册 · §2.5 §2.9 §2.16 |
| `src/providers/` | [MODEL_CATALOG.md](docs/reference/MODEL_CATALOG.md) · §3.6 §4.9 |
| `src/mcp/` | §5.20（dual-era 协议） |
| `src/hub/` | [ALEPH_HUB.md](docs/reference/ALEPH_HUB.md) · §5.21 |
| `src/loop_graph/` `src/workflow/` | [GRAPH_LAYER.md](docs/reference/GRAPH_LAYER.md) · §4.12 |
| `src/identity/` | [AGENT_IDENTITY.md](docs/reference/AGENT_IDENTITY.md) · §5.17 |
| `src/config/` `src/diagnostics/` | §5.8 §5.9 §5.10 |
| `desktop/` | [WINDOWS_RUNTIME.md](docs/reference/WINDOWS_RUNTIME.md) · [LINUX_DESKTOP.md](docs/reference/LINUX_DESKTOP.md) · [DESKTOP_BRIDGE.md](docs/reference/DESKTOP_BRIDGE.md) · §7.1–§7.4 |
| `interfaces/webchat/` | [DESKTOP_SHELL.md](docs/reference/DESKTOP_SHELL.md) · §4.7 §6.8 |
| `src/agents/` `src/teams/` | [MULTI_AGENT_SYSTEM.md](docs/reference/MULTI_AGENT_SYSTEM.md) · §4.4 §4.5 §4.13 |
| `src/tasks/cron/` `src/tasks/heartbeat/` | §4.13（写面对账守卫 · 共用告警判据 · 停摆 job）· `src/tasks/shared/alert.rs` |
| `src/sandbox/` | [SANDBOX.md](docs/reference/SANDBOX.md) · §3.8 |

> **对照表已做完，别重做**：openclaw（gateway / cluster / hub / model catalog）· codex（权限模型 / Multi-agent V2）· hermes · pi · LangGraph · RouteLLM/LiteLLM/Bifrost · DeepSeek-Reasonix · FluidVoice/WhisperLive · SkillOpt · buzz。逐项结论与"刻意不做清单"都在对应 reference 文档里。

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

### 三根会话旋钮 (Session Knobs)

三者**正交**，都由 Panel composer pill 或对话式工具切换：

| 旋钮 | 值 | 管什么 | 单一源 |
|---|---|---|---|
| **执行档位 Exec Tier** | `Ask` / `Auto`(默认) / `Full` | 工具执行**审批**。读工具**声明的元数据**（幂等/destructive），不认名字；未知工具在 `Ask` 档 fail-closed | `src/tools/scoped/`（唯一强制点）→ [SECURITY.md](docs/reference/SECURITY.md) |
| **会话模式 Session Mode** | `chat` / `work`(默认) / `code` | 工具**呈现面**静态分区（R10 渐进披露例外）。不授予不拒绝任何权限 | `src/config/types/policies/session_mode.rs` → [MODE_SYSTEM.md](docs/reference/MODE_SYSTEM.md) |
| **繁忙输入 Busy Input** | `Steer`(默认) / `Interrupt` / `Queue` | 会话已有 run 在跑时新消息怎么办 | `src/gateway/busy_queue/` → FEATURE_LOCATOR §4.8 |

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

**总入口**: [FEATURE_LOCATOR.md](docs/reference/FEATURE_LOCATOR.md) —— 按 §编号组织的全项目现状库，判据清单里每个 `→ §x.y` 都指向它。

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
