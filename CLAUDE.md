# CLAUDE.md

> **Context Tiers**
> **Tier 1（本文件，每次会话都付一遍钱）**＝红线 R1–R10 · 原则 P1–P8 · 判据**形状名索引** · 子系统路由 · 开发指南。
> **Tier 2（按需加载）**＝ `docs/reference/*`：本文每个 `→` 指针都指向那里的全文（深度通常是这里的 5–20 倍）。
> **Tier 3（默认忽略）**＝ `docs/archive/`、历史规格，除非明确要求。
>
> 本文件只承载**没有任何单份 reference 文档能说出口的东西**：跨子系统的约束、红线、以及"你不知道自己需要查它"的入口。**详情一律不写在这里**——写进来就是给每次会话付一遍钱。
>
> 🚦 **写入纪律（2026-08-28 用户裁定 · 2026-08-30 收紧）**：凡**代码话术**——案例叙述、某次静默失效的来龙去脉、逐轮改造记录、被推翻的假设——一律进 [FEATURE_LOCATOR.md](docs/reference/FEATURE_LOCATOR.md)：**触发器**进**附录 E**，**全文**进**附录 D**（验证纪律进**附录 C**）。
> 本文件这一侧只在**出现一个新形状**时增一行；出现新实例时**不增**。两处都写全文＝「同一事实的两份表述」，正是判据自己反复付账的那一类。
> 也**别新建平行的 reference 文件**去装判据——那是给同一个问题造第二个真源。

---
## 🛑 架构红线 (Architectural Redlines)

> 最高优先级约束，违反的代码不得合入。这里只留**禁令**，例外与推导见指针。

| # | 红线 | 禁令 / 原则 |
|---|------|------------|
| **R1** | 大脑与四肢绝对分离 | 严禁在 `src` 中直接调用平台系统 API（AppKit / Vision / CoreGraphics / windows-rs）；核心只定义能力契约 (Trait)，物理实现由原生 Bridge (Swift / 其他) 经 IPC 提供。**例外·进程隔离内核**：restricted-token / job-object / AppContainer / 完整性级别 / SID·ACL 与本地 PID 探测**必须由 spawn 子进程的父进程就地发起**，无法经 IPC 桥委托 ⇒ `src/sandbox/*` 与 `builtin_tools/desktop/session_lock.rs` 的平台 FFI（`cfg(windows)` 门控）是**立意之外的合法开口**，非违规——R1 针对的是桌面 UI / 屏幕 / Vision **四肢** → [SANDBOX.md](docs/reference/SANDBOX.md) |
| **R2** | UI 逻辑唯一源 | 严禁在原生 Bridge 中实现有业务逻辑的设置页 / 表单 / 列表；复杂业务 UI 一律在 Leptos (WASM) Panel，Bridge 只做系统 API 调用与桥接 |
| **R3** | 核心轻量化 | 严禁为单一非核心功能往 core 引入沉重三方库；优先实现为 Skill (Python/Bash) 或 MCP Server。**内核只调度，不搬砖** |
| **R4** | Interface 层禁止业务逻辑 | Channel / Bot / CLI / Panel 不做数据持久化、记忆检索或任务规划——纯 I/O：输入转 JSON-RPC 发给 Server，响应渲染给用户 |
| **R5** | AI 主动到达 | 通过用户**已有的**工作通道主动送达（多端推送 / 内联建议 / 订阅式 Daemon 触发）；不抢焦点、不弹模态，但不因此砍掉必要的交互入口 |
| **R6** | 一核多端 | Aleph 是常驻后台服务，UI 不是必需品；Rust Core 是唯一大脑，多端通道只负责 I/O 与渲染，不参与业务推理 |
| **R7** | LLM 主权 | 严禁用确定性代码替代 LLM 擅长的推理判断（意图识别 / 任务评估 / 路由决策 / 内容分类）。对每个模块问：**这是在赋能 LLM，还是越俎代庖？** 赋能层（Gateway/Memory/Daemon/Soul/Provider/Tool/MCP/压缩/安全硬过滤）保留；意图规则引擎、POE 验证管线、多层 Tool Filter、Context 多层合并、Dispatcher 意图分析一律禁止 |
| **R8** | 工具即一切 | Aleph 自身**所有可配置操作**都暴露为工具，让 LLM 用自然语言完成配置（agent / provider / channel / skill / MCP / daemon 规则）。核心循环：`用户自然语言 → LLM 理解意图 → LLM 选择工具 → 工具执行 → 结果返回 LLM → LLM 回复用户`。**对话即管理面板** |
| **R9** | 智慧在 Prompt 中 | 被移除的中间件的智慧**迁移**到 system prompt，不是丢弃。**但 prune-the-prompt**：模型越强越需要更少方向 / 约束 / 示例，新模型发布后第一件事是**修剪**上下文。加字节前过两把尺——① 这是模型**做不到的运行时事实**，还是我在教强模型怎么思考？② **有没有一个工具拥有这句话**？有 → 写进那个工具的 `DESCRIPTION`。两把尺已建进 `src/thinker/prompt_contract.rs`，量一下用 `aleph-server prompt-size`。⚠️ 第二把尺**有前置条件**（目录条目写字面量会整体遮蔽工具常量——附录 E.3），尺子量不到的地方搬过去等于删掉 → [HARNESS_PHILOSOPHY.md §8](docs/reference/HARNESS_PHILOSOPHY.md) |
| **R10** | 薄 Harness，笨循环 | `src/harness/` 锁 **12 文件**、只承载 Think→Act 轮次调度、循环里有 **5 个"不"**、加代码前必答 **3 问**、任何"零消费者"的抽象立即 CUT（YAGNI 撤回）。**行数红线＝棘轮机制本身**（`src/harness/tests/budget.rs::CEILING`，实测非手算、只减不增、增必答 3 问）——**代码是权威，本文件刻意不复制那个数字**，因为文档抄一份就漂移过一次 → [HARNESS_PHILOSOPHY.md](docs/reference/HARNESS_PHILOSOPHY.md) · [`src/harness/CLAUDE.md`](src/harness/CLAUDE.md) |

> **R10 的渐进式工具披露例外**：「core 工具静态常驻 + 全量目录 + `tool_search` 按需加载 schema」是**不看消息内容的静态分区**、加载决策 100% 由模型发起，与 `src/tools/scoped/` 已有的三道静态 `retain` 同层同性质，**不属**第 2 不所指的"按意图过滤"；它落在工具呈现层，**不进 `src/harness/`**。

---
## 🧬 设计原则 (Design Principles)

> 红线之下的工程纪律。全文见 [DESIGN_PATTERNS.md](docs/reference/DESIGN_PATTERNS.md) · [CODE_ORGANIZATION.md](docs/reference/CODE_ORGANIZATION.md) · [DOMAIN_MODELING.md](docs/reference/DOMAIN_MODELING.md)。

| # | 原则 | 一句话 |
|---|------|--------|
| **P1** | 低耦合 | 模块间通过 Trait 通信；禁止跨层直接调用；依赖单向 `Interface → Core → Domain`；状态变化优先走事件 |
| **P2** | 高内聚 | 单一职责；相关逻辑物理聚合；命名即文档；单文件超 500 行考虑按职责拆分 |
| **P3** | 可扩展性 | OCP——新功能靠实现 trait / 注册插件，不改核心；策略模式优于条件分支；插件化优先；接口用 JSON Schema (schemars) 自描述 |
| **P4** | 依赖倒置 | 高层与低层都依赖抽象；core 定义 trait、实现在 crate 边界外；构造时经 `AppContext`/Builder 注入，运行时不 `new` 具体类型 |
| **P5** | 最小知识 | 只与直接协作者通信（`a.b().c().d()` 是设计缺陷信号）；不暴露内部字段引用链；`pub(crate)` 优于 `pub` |
| **P6** | 简洁性 | 奥卡姆剃刀 + YAGNI；三次法则（第三次重复才抽象）；废弃代码**删除**不注释；扁平优于嵌套（early return / `?`） |
| **P7** | 防御性设计 | 系统边界严格校验、内部信任已校验数据；外部依赖失败优雅降级不 panic；锁 `unwrap_or_else(\|e\| e.into_inner())`；字符串切片用 `char_indices()` / `.get(..n)`，不用 `&s[..n]` |
| **P8** | LLM 优先 | 自然语言 → 结构化意图交给 LLM；**禁止用 regex 解析用户自然语言**（正则只配格式固定的机器文本）；LLM 返回 JSON，代码只解析和执行 |

---
## 🧭 12-Factor 采纳

12 条 factor 的逐条对照、四条**采纳条款 A1–A4**（自有 Context Window · 错误压缩 ≠ 错误恢复 · 状态可重建趋向纯 Reducer · 统一 Launch/Pause/Resume 契约）的正文与证据，全文在 [TWELVE_FACTOR_AUDIT.md §D](docs/reference/TWELVE_FACTOR_AUDIT.md)。

它们是**叠加在 R1–R10 / P1–P8 之上的映射层**——工程承诺级，**不改任何红线、不设新红线**。唯一需要记在 Tier 1 的边界：**让模型看见并自愈错误 = 要（A2）；让 harness 替模型挑恢复策略 = 不要（R10 第 5 不）**；且 A3 不得让 `src/harness/` 越过 12 文件 / `budget.rs::CEILING` 棘轮。

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
## ⚠️ 工程判据 — 形状名索引 (Hard-Won Criteria)

> 每一条判据都是一次**静默失效**的代价：没崩溃、没报错、测试全绿，只是"这个功能好像从来没生效过"。
> **这里只留形状名**——用来认出「我现在踩的是哪一类」。触发器（判据句 + 锚点 + 一句机制 + 指针）在
> [FEATURE_LOCATOR 附录 E](docs/reference/FEATURE_LOCATOR.md)（分组 §0–§10 与下方路由表一一对应），全文在**附录 D**、验证纪律在**附录 C**。
> **改动某子系统前先扫一遍 附录 E 的对应分组**；附录 E.0「跨子系统通用形状」每次改动都适用。
> ⚠️ **转发**：代码注释与 `docs/` 里写的「CLAUDE.md §N」/「判据清单 §N」指的就是**附录 E.N**（分组号未变，只是全文搬了家）。
> **这张表只在出现一个新形状时才增加一行**——新实例进附录 E，不进这里。

1. **同一事实的两份表述** — 只改一份就是静默说谎。四种形态：两份漂了 / 其中一份被当残留清掉了 / 一份从出生起就是另一份的**削弱版** / 一份描述的不是事实而是**另一个子系统的行为**（引用一旦写下，被引用方就从"可以改"变成"不许改"却没人通知它）。**最贵的那份在注释、在文档数字、在发给模型的 `DESCRIPTION` 里**——注释正是说谎的那一方。
2. **恒真的谓词等于没判** — 它有**四张脸**（恒红 / 恒绿 / 不可失败 / 没装上），四张都长得像在工作。唯一分得开的问句：**在什么情况下这东西会变红？** 答不出一个具体情形，它就不是闸。同族：把 N 个分类扇入 1 个值的 `match`（`#[allow(clippy::match_same_arms)]` 就是那个 tell）。
3. **守卫的绿只覆盖它认得的那种形状** — 块识别器 / 注册形状 / 它自己列举的"事实"清单 / 名单式成员 / 窗口边界 / `Option` 的隐式 `default`。写完先问的不是「规则对不对」，是**它认得几种形状**；清单要**从拥有事实的那个类型派生**并配自保断言。**一条没被证伪过的守卫不算守卫**；⚠️ 一条会误报的守卫比一条不报的更贵——它会被当成证据引用。
4. **守卫要断言「效果到达了」，不是「调用发生了」** — 问：把这一步的返回值扔掉，测试还绿吗？绿 ⇒ 你守的是产地不是连线。
5. **列举法只覆盖立法当天的世界** — 白名单 / 哨兵值 / 保真字段集 / 受支持维度。改问「**这段字是谁写的**」「**不在我这张表上的那部分呢**」。⚠️ 当默认是「全都要」时，**重放一份清单不是恢复而是收窄**。
6. **先数一遍** — 面 / 写者 / 读者 / 生产者 / 构造点 / 凭据 / 解析点 / 终端臂 / 推导者。**数错的方向永远是少一个**——「我数出来是三个」本身就该触发一次 grep，且 grep 前先剥注释行。数的是「这个事实有几个推导者」，不是「这个变量有几个读者」。
7. **两端完整而中间没线** — 传感器没有生产者 / post-pipeline 手焊 / 同一件事两个 id 互不指认 / 载荷齐全而信封坏掉 / 注册了但派发表没有那条臂。**dead-code 分析对这一类结构性失明**（两端都有测试），而**唯一的搜索命中常常是一句撒谎的注释**。
8. **fail-closed 的答案被当成值消费，就反转成许可** — `Err` / `Ok(None)` / 空列表 / 空字符串只有资格说「**我不知道**」。「被拒」不许读作「没有」，「还没准备好」不许答成「失败了」，「未知」不许读作「健康」，`Err` 不许当放行判据。
9. **一个动词有几张脸，判据就要在每张脸上用同一个推导** — 工具面 / RPC 面 / 客户端面 / 事件面 / 一条连接的两个方向。共用判据**也要共用推导**。**没有客户端的能力不算已交付**，服务端那半再完整也不算。
10. **跨 crate 的 wire 契约，两边各持一份形状就会互相抵消** — 一个只读自己刚写下的字面量的断言测的是 serde 不是你的代码，**永远绿**；键集要放进两边都依赖的那个 crate 并**用它构造响应**（解析只能证明超集，永远证不出相等）。**信封也是 wire key**；**展示列错的比缺的贵**——缺的读起来像"还没有值"，错的读起来像事实。
11. **一个「报成功的 no-op」** — 修在**执行者**身上比修在每个已知实例上便宜；而它**第一次说出口时报出来的数目，才是那一类的真实大小**。
12. **顺序 / 单位 / 边界必须在同一处派生** — 在 A 序里选出、在 B 序里应用 ⇒ 它命名的是另一个集合；游标必须和它比较的那一列共用同一份解析；把「有顺序」改造成「**只有一次调用**」比加强守卫便宜。
13. **一个上限 / 棘轮 / 信号量，位置与寿命决定它约束什么** — 设在**实测值之上**的那段差额就是它已经发出去的额度；在**物化点之后**才检查的预算约束的是输出不是内存；**约束的寿命不得短于被它约束的东西**；「先认领后执行」的界限要在**执行时刻**成立。
14. **闸的两个方向都要问** — 负半边有出口而正半边没有，这个不对称本身就是缺陷；**被闸住的人接下来会干什么**；**谁能把它打开、从哪个界面**——答不上就不是 fail-closed 是 fail-dead。闸的范围必须盖住「**能把这个闸拿掉的那个动词**」，并追问有没有「两步都合法、合起来等价」的路。
15. **不可逆边界与一次性的动作** — 跨越之前先盖**意图**戳（只记录"做完了"的机件分不出"没做"和"做了但没记上"）；一次性的闩漏一次就是**永远**；崩溃边界上的"未知"不能写成"失败"；进程内存不是状态。
16. **孪生子系统 / 第 N 次复发** — 一边修好的判据要**主动搬过去**，别等它在另一边被重新发现（改动一个时问：它的孪生怎么回答同一问题）。⚠️ 一条注释里写着数目（"两个孪生"/"三处"）就是一张会腐烂的名单——把那一问挂到**已经知道集合是什么的那张表**上。
17. **一份"展示用"的东西，提交前必须能指出渲染它的那一行代码** — 指不出就是 CUT，不是"以后再接"。**错的标签比缺的贵**；不认识的状态词一律读作「我无法担保」；`X::default()` 当占位会替每个字段说出一个具体的谎。
18. **量具会骗人** — 数字要带着它测的**谓词**和它测于哪个 **commit**；一次扫描只为**它枚举过的那些形状**背书（结论的作用域是**方法**，不是目录）；变异之后**先看红的名单是不是预期的那一份**，再看条数；**别人的仪器你重复测量，自己的仪器你先怀疑它** → [附录 C](docs/reference/FEATURE_LOCATOR.md)

---
## 📍 子系统路由 (Read Before Editing)

> **FL** = [FEATURE_LOCATOR.md](docs/reference/FEATURE_LOCATOR.md)（`FL §x.y` 指它的正文章节）；**E.N** 指它的**附录 E** 判据分组。真机装置的**每个阶段在证明什么**见 [`qa/README.md`](qa/README.md)。

| 你要动的目录 | 先读 | 判据 | 真机 QA |
|---|---|---|---|
| `src/harness/` | [HARNESS_PHILOSOPHY.md](docs/reference/HARNESS_PHILOSOPHY.md) · [`src/harness/CLAUDE.md`](src/harness/CLAUDE.md) · FL §3.1 | E.0 | — |
| `src/thinker/` `src/context/` | FL §2.1 §2.3 §2.18 §2.19 §2.20 | E.1 | — |
| `src/tool_output/` | FL §2.7 §3.14 | E.2 | — |
| `src/tools/` `src/builtin_tools/` | [TOOL_SYSTEM.md](docs/reference/TOOL_SYSTEM.md) · [SECURITY.md](docs/reference/SECURITY.md) · FL §3.2–§3.14 | E.3 | — |
| `src/builtin_tools/file_search/` | FL §3.4 | E.0 E.3 | `qa/file_search/run.sh {floor,page,reach,steer}` · `cargo bench --bench file_search_scan` |
| `src/gateway/` | [GATEWAY.md](docs/reference/GATEWAY.md) · [`src/gateway/CLAUDE.md`](src/gateway/CLAUDE.md) · FL §4.8 §5.6 §5.18 §5.26 §6.9 | E.4 | `qa/channels/run.sh {reach,errors,approval}` |
| `src/gateway/btw/` | FL §4.14 的机制图 · [SECURITY.md](docs/reference/SECURITY.md) 只读地板 | E.4 | `qa/btw_tui/run.sh {frames,promote}` |
| `src/gateway/session_store/` `session_manager/` | FL §6.9 | E.0 | `qa/session_order/run.sh` |
| `src/memory/` `src/note/` | [MEMORY_SYSTEM.md](docs/reference/MEMORY_SYSTEM.md) + memory/ 三分册 · FL §2.5 §2.9 §2.16 | E.5 | `qa/memory_curated/run.sh` |
| `src/providers/` | [MODEL_CATALOG.md](docs/reference/MODEL_CATALOG.md) · FL §3.6 §4.9 | E.9 | — |
| `src/browser/` `src/builtin_tools/browser_tools/` | FL §3.12 | E.9 | `qa/browser_managed/run.sh`（九个阶段，两个 driver） |
| `src/mcp/` · `src/hub/` | FL §5.20 §5.24 · [ALEPH_HUB.md](docs/reference/ALEPH_HUB.md) FL §5.21 | E.9 | `qa/plugins/run.sh` |
| `src/loop_graph/` `src/workflow/` · `src/identity/` | [GRAPH_LAYER.md](docs/reference/GRAPH_LAYER.md) FL §4.12 · [AGENT_IDENTITY.md](docs/reference/AGENT_IDENTITY.md) FL §5.17 | E.3 E.0 | — |
| `src/config/` `src/diagnostics/` · `src/sandbox/` | FL §5.8 §5.9 §5.10 §5.24 · [SANDBOX.md](docs/reference/SANDBOX.md) FL §3.8 §3.15 | E.8 E.3 | — |
| `src/agents/` `src/teams/` · `src/tasks/cron/` `src/tasks/heartbeat/` | [MULTI_AGENT_SYSTEM.md](docs/reference/MULTI_AGENT_SYSTEM.md) FL §4.4 §4.5 §4.13a–c（cron/heartbeat 是孪生，共用 `src/tasks/shared/{alert,delivery}.rs`） | E.0 | `qa/teamchat_rooms/run.sh` |
| `desktop/` | [WINDOWS_RUNTIME.md](docs/reference/WINDOWS_RUNTIME.md) · [LINUX_DESKTOP.md](docs/reference/LINUX_DESKTOP.md) · [DESKTOP_BRIDGE.md](docs/reference/DESKTOP_BRIDGE.md) · FL §7.1–§7.4 | E.6 | — |
| `interfaces/webchat/` | [DESKTOP_SHELL.md](docs/reference/DESKTOP_SHELL.md) · FL §4.7 §6.8 §6.9 | E.7 | `qa/picker_nav/run.sh` |
| `src/canvas/` + Panel canvas 视图 | [CANVAS.md](docs/reference/CANVAS.md) · FL §6.10 | E.7 | `qa/canvas/run.sh` |
| `interfaces/tui/` `interfaces/cli/` `shared/protocol/` | FL §5.4 §5.11 §5.13 §5.23 | E.0（跨 crate wire 契约） | — |

> **对照表已做完，别重做**：openclaw · codex · hermes · pi · LangGraph · RouteLLM/LiteLLM/Bifrost · DeepSeek-Reasonix · FluidVoice/WhisperLive · SkillOpt · buzz · deepseek-harness。逐项结论与"刻意不做清单"都在对应 reference 文档里。

---
## 🔧 开发指南

### 构建命令

| Command | Description |
|---------|-------------|
| `cargo run --bin aleph-server` · `cargo check -p alephcore` | Start server (debug) · quick compile check（**只验证了仓库的一小半**，见下方验证集） |
| `just dev` · `just build` | Dev server（先重建 WASM）· Release build (WASM + server) |
| `just shell-dev` · `just shell-build` · `just shell-build-lite` | 桌面 App dev · 完整 installers（.dmg/.msi/.deb，内置 server）· Panel 纯壳（无 server，连局域网） |
| `just test-all` · `just clippy` · `just wasm` | 全量测试（core + desktop + proptest）· Lint · 唯一编译 Panel **出厂形态**的命令 |
| `just verify-build` · `just release YY.M.D` | CI 验证三产物三平台能否构建（不打 tag）· **发版**（需先写 changelog）→ [RELEASE.md](docs/reference/RELEASE.md) |

### 最小可信验证集 — 六条命令，不是一条

```
cargo test -p alephcore --lib --no-run
cargo test -p alephcore --bins            # 唯一真跑而非 --no-run 的一条：--lib 带不到 src/bin/ 下的 94 条
                                          # （含钉住 boot 无条件 install_policy/install_ledger 的 census）
cargo test -p alephcore --features test-helpers --test '*' --no-run   # --all-targets 只展开 target 不展开 feature
cargo test -p aleph-panel --lib --no-run  # check 看不见它的 #[cfg(test)]；曾整程编译不过
cargo check -p aleph-desktop-{macos,windows,linux}   # 跨平台改动要 check 那个目标的限肢 crate
cargo clippy --workspace --all-targets    # 先 just _stage-shell-placeholders；--all-targets 展开 target、
                                          # --workspace 展开 package（无 default-members ⇒ 默认只 lint 根 crate）
```

- **`cargo check` 不编译 `#[cfg(test)]`**——删 `pub fn` / 字段的同一笔里必须跑 `cargo test --no-run`；改动 `interfaces/tui/` `interfaces/cli/` 的同一笔里跑 `cargo test -p aleph-tui -p aleph-cli`（**上面那条 `--workspace` clippy 会 lint 它们，但 lint 不是测试**）。
- **`interfaces/webchat/` 有任何改动（哪怕不是你改的）就跑一次 `cargo test -p aleph-panel --lib`**——这个 crate 的**语义合并冲突是常态形状**（一侧的类型 + 另一侧的调用点，git 不报冲突、两边单独看都完整）。修完**先看警告再看错误**：`unused variable` 说明那半边根本没有调用者，正解是 CUT。只改 Panel 时用 `just wasm`——它是唯一编译**出厂形态**的命令。
- **`cargo check -p aleph-desktop-shell` 前需先 `just _stage-shell-placeholders`**（tauri-build 要求 externalBin 占位文件存在）；**`--workspace` clippy 同样要**。占位路径**别在别处抄一份**——那条 recipe 自己推 triple、Windows 补 `.exe`、`AlephBridge-` 只在 macOS 上建。
- 验证是怎么骗你的（数字 / 仪器 / 扫描边界 / 闸 / 命令陷阱）→ [FEATURE_LOCATOR 附录 C](docs/reference/FEATURE_LOCATOR.md) · 触发器 → 附录 E.10。

### 工具链与版本

- **MSRV = 1.95**（由 `sysinfo 0.39` 决定），在 `Cargo.toml` 的 `[workspace.package]` 与 `[package]` 两处 `rust-version` 声明；根 `rust-toolchain.toml` 钉住具体 stable（当前 `1.96.0`），本地与 CI 自动使用同一工具链——无需 `rustup default` 或 `cargo +<ver>`。抬高 MSRV 时同步更新这两处。
- **CalVer `YY.M.D`**（两位年、月/日不补零，如 `26.5.21`；同时是合法 semver 并满足 Windows MSI 约束），每天最多一个版本。**VERSION 文件是唯一版本源**——`build.rs` 读取 → 注入 `ALEPH_VERSION` → 代码用 `env!("ALEPH_VERSION")`；**禁止**硬编码版本号，**禁止** `env!("CARGO_PKG_VERSION")`。Panel System Info、Gateway 版本、MCP/ACP 协议版本、CLI `--version`、release tag 全读它。发版走 `just release YY.M.D` → [RELEASE.md](docs/reference/RELEASE.md)；Windows 构建前置依赖 → [WINDOWS_RUNTIME.md](docs/reference/WINDOWS_RUNTIME.md)。

### 会话旋钮 (Session Knobs)

**正交**的会话旋钮：执行档位 / 会话模式 / 推理档 / 记忆模式 / 模型 pin / 繁忙输入。**别在这里维护一个数目**（上一版标题写着「三根」而表里早就不止三行）。除繁忙输入外共用一套机制（值住在 `SessionMetadata.identity_meta.custom[<key>]`，precedence **请求 > 会话 > 全局**，解析在 `src/gateway/execution_engine/turn_*.rs` 的孪生模块里）。表、每根的"谁在拨"、以及**加一根新旋钮要动的每一处**全在 [SESSION_KNOBS.md](docs/reference/SESSION_KNOBS.md)。

`[sandbox.command_policy]` 的硬底线**任何档位都压不下去**。

### 分发形态与信任模型

- **三产物**（同一 tag）：完整桌面 App（内置 `aleph-server`，单机零配置）/ Aleph Panel 纯壳 App（连局域网 server）/ 独立 `aleph-server` 二进制 → [PRODUCT_TOPOLOGY.md](docs/reference/PRODUCT_TOPOLOGY.md)
- **信任模型 = 网络边界 + 登录墙**：默认只绑 `127.0.0.1`；`[gateway] host = "0.0.0.0"` 显式开放局域网。loopback 免凭据恒 operator；远程须在 `connect` 出示 device token / 一次性配对票 / 共享 token 之一，**过了就是 operator，与本地完全一致——单层，没有 Chat/Config 子层**。协议护栏是 WS Origin 校验 → [SECURITY.md#auth-ux](docs/reference/SECURITY.md#auth-ux)

### Feature Flags / 提交规范 / 进程管理

- 所有生产功能始终编译，无需 feature flags。仅保留测试用：`loom`（并发）、`test-helpers`（集成测试工具）。
- English commit messages，格式 `<scope>: <description>`（例：`gateway: add WebSocket server foundation`）。**单分支开发**：所有工作直接在 main。`EnterWorktree` 会话内只合并不删除（同会话 `git worktree remove` 会损坏 Shell）→ [CODE_ORGANIZATION.md](docs/reference/CODE_ORGANIZATION.md)
- Singleton 由 OS 级 `flock`（`~/.aleph/data/aleph.lock`）强制；CLI 写子命令经 `with_policy` 走 IPC 或本地拿锁。`kill -9` 后可立即重启。doctor 的 `core/duplicate-instance` 是运行时哨兵——**多进程竞争同一 vault → HMAC 失败 → vault 数据丢失** → [PROCESS_MANAGEMENT.md](docs/reference/PROCESS_MANAGEMENT.md)

### 内置文件与 Shell 工具

**搜索走 `grep` / `find`，不走 bash**——`grep` 是内容搜索、`find` 是文件名发现，共用 `src/builtin_tools/file_search/walk.rs` 的 `.gitignore`-aware 走树 ＋ deny 闸。多个词写成**一次** alternation（`grep{pattern:"a|b|c"}`）；先 `files_only: true` 拿路径，再 `file_read{offset,limit}` 只读命中附近。非走 shell 不可时用 `rg` 而不是 `grep`。`file_ops(search)` 是**另一张脸**（文件管理：size/type/extension）。**长任务（>3 min build/install）必须 `background: true`**——`WAIT_MAX_TIMEOUT_SECS=170` 是 180s tool budget 的硬约束，**不要**尝试扩展（违 R10）。全部现状 → [FEATURE_LOCATOR §3.4](docs/reference/FEATURE_LOCATOR.md)

### My Working Style

- 先给方案再写代码；不确定时列出选项，不猜测（呼应 P1 与全局 CLAUDE.md）
- 重大变更前先问，小优化可直接执行
- 回复用中文，代码注释用英文，文档中英双语
- 按需正常使用 cargo（`check` / `test` / `clippy`）——编译与测试验证优先，不再强制节制调用次数

---
## 📚 文档索引 (Tier 2)

**总入口**：[FEATURE_LOCATOR.md](docs/reference/FEATURE_LOCATOR.md) —— 按 §编号组织的全项目现状库。
**附录 C** = 验证纪律全文（这个绿是怎么骗你的）· **附录 D** = 工程判据全文· **附录 E** = 判据触发器清单（§0–§10，本文形状索引的下一层）。**代码话术一律写进那里。**

| 文档 | 说明 |
|------|------|
| [ARCHITECTURE.md](docs/reference/ARCHITECTURE.md) · [PRODUCT_TOPOLOGY.md](docs/reference/PRODUCT_TOPOLOGY.md) | 总体架构 / 一套源码 → 三产物 + 参考部署拓扑 |
| [HARNESS_PHILOSOPHY.md](docs/reference/HARNESS_PHILOSOPHY.md) · [`src/harness/CLAUDE.md`](src/harness/CLAUDE.md) | 薄 Harness + 笨循环（R10 详解、12 文件、5 不、3 问、棘轮流水账） |
| [TWELVE_FACTOR_AUDIT.md](docs/reference/TWELVE_FACTOR_AUDIT.md) | 12-Factor 逐 factor 审计 + **A1–A4 采纳条款母本** + backlog |
| [AGENT_SYSTEM.md](docs/reference/AGENT_SYSTEM.md) · [AGENT_DESIGN_PHILOSOPHY.md](docs/reference/AGENT_DESIGN_PHILOSOPHY.md) · [AGENT_LOOP_CONTEXT_BUDGET.md](docs/reference/AGENT_LOOP_CONTEXT_BUDGET.md) · [AGENT_LOOP_TOOL_EXECUTION.md](docs/reference/AGENT_LOOP_TOOL_EXECUTION.md) · [AGENT_LOOP_RECOVERY.md](docs/reference/AGENT_LOOP_RECOVERY.md) | Agent 系统 / 设计哲学 / 循环三分册 |
| [GRAPH_LAYER.md](docs/reference/GRAPH_LAYER.md) · [MULTI_AGENT_SYSTEM.md](docs/reference/MULTI_AGENT_SYSTEM.md) · [CLUSTER.md](docs/reference/CLUSTER.md) | 循环治理图（六词闭集治理边 + 锚点/冻结 + 审计环）· 多 agent / 团队 / 群聊直播面 · 集群联邦 |
| [GATEWAY.md](docs/reference/GATEWAY.md) · [`src/gateway/CLAUDE.md`](src/gateway/CLAUDE.md) · [WORKFLOW_INTEROP.md](docs/reference/WORKFLOW_INTEROP.md) | 网关、通道、投递队列 · 工作流互操作 |
| [TOOL_SYSTEM.md](docs/reference/TOOL_SYSTEM.md) · [SESSION_KNOBS.md](docs/reference/SESSION_KNOBS.md) · [MODE_SYSTEM.md](docs/reference/MODE_SYSTEM.md) | 工具系统 / 六根会话旋钮 / 会话模式 chat·work·code |
| [MODEL_CATALOG.md](docs/reference/MODEL_CATALOG.md) | 预设 provider/模型四表 + 单一 join 点 + 漂移守卫契约 |
| [CANVAS.md](docs/reference/CANVAS.md) | 白板画布：四层架构 + 乐观锁并发 + 能力 URL 素材面 + iframe 沙箱边界 |
| [MEMORY_SYSTEM.md](docs/reference/MEMORY_SYSTEM.md) → [RAW_MEMORY.md](docs/reference/memory/RAW_MEMORY.md) · [NOTES.md](docs/reference/memory/NOTES.md) · [RETRIEVAL.md](docs/reference/memory/RETRIEVAL.md) · [DREAM_DAEMON.md](docs/reference/memory/DREAM_DAEMON.md) | 记忆总览 + 三支柱分册 + 离线做梦（**`DreamGate` 已删，勿复活**） |
| [EXTENSION_SYSTEM.md](docs/reference/EXTENSION_SYSTEM.md) · [PLUGIN_SYSTEM.md](docs/reference/PLUGIN_SYSTEM.md) · [ALEPH_HUB.md](docs/reference/ALEPH_HUB.md) | 插件**运行时** / 扩展**分发**（目录契约 + 三道 ingest 闸 + 出处账本） |
| [SECURITY.md](docs/reference/SECURITY.md) · [AGENT_IDENTITY.md](docs/reference/AGENT_IDENTITY.md) | 信任模型 + 工具权限三层 + 动作化审批门 / 每 agent Ed25519 + 签名哈希链账本 |
| [DESIGN_PATTERNS.md](docs/reference/DESIGN_PATTERNS.md) · [CODE_ORGANIZATION.md](docs/reference/CODE_ORGANIZATION.md) · [DOMAIN_MODELING.md](docs/reference/DOMAIN_MODELING.md) | 工程规范 |
| [SERVER_DEVELOPMENT.md](docs/reference/SERVER_DEVELOPMENT.md) · [SESSION_SERVICE.md](docs/reference/SESSION_SERVICE.md) · [SANDBOX.md](docs/reference/SANDBOX.md) | 服务端 / 会话 / 沙箱 |
| [DESKTOP_BRIDGE.md](docs/reference/DESKTOP_BRIDGE.md) · [DESKTOP_SHELL.md](docs/reference/DESKTOP_SHELL.md) · [WINDOWS_RUNTIME.md](docs/reference/WINDOWS_RUNTIME.md) · [LINUX_DESKTOP.md](docs/reference/LINUX_DESKTOP.md) | 桌面 Bridge / 壳 / Windows 运维 + DPI / Linux 能力矩阵 |
| [MODEL_PERCEIVABLE_ECOSYSTEM.md](docs/reference/MODEL_PERCEIVABLE_ECOSYSTEM.md) · [SKILL_TRIGGER_ENHANCEMENT.md](docs/reference/SKILL_TRIGGER_ENHANCEMENT.md) · [GOOGLE_MEET_BRIDGE.md](docs/reference/GOOGLE_MEET_BRIDGE.md) · [WHATSAPP_ARCHITECTURE_DESIGN.md](docs/reference/WHATSAPP_ARCHITECTURE_DESIGN.md) | 生态可感知 / Skill 触发 / 单点集成设计 |
| [RELEASE.md](docs/reference/RELEASE.md) · [PROCESS_MANAGEMENT.md](docs/reference/PROCESS_MANAGEMENT.md) · [`qa/README.md`](qa/README.md) | 发版 / 进程管理 / 真机 QA 装置（每个阶段在证明什么） |

> **官方 skills/plugins 离线兜底**：根目录 `skills/` 与 `plugins/` 是两个 git submodule（upstream = 兄弟仓 Aleph-skills / Aleph-plugins），经 `include_dir!` 在 `aleph-server` **编译期嵌入二进制**（`src/bundled/mod.rs`）。首次安装优先 git clone 上游，**网络故障时回退到这份嵌入快照**。**勿删这两个目录**——`include_dir!` 是编译期宏，目录缺失直接编译失败，并连带破坏 `build.rs` rerun / CI `submodules: recursive` / `justfile` 发版重嵌链。

---
## 🏢 官方仓库 (Official Repositories)

| 仓库 | 路径 |
|------|------|
| **Aleph（主项目）** — Rust Core + 多端架构 | `/Volumes/TBU/Workspace/Aleph` |
| Aleph-Hub（扩展目录中心）· Aleph-homepage（Next.js 首页）· Aleph-docs · Aleph-mcp · Aleph-plugins · Aleph-skills | `/Volumes/TBU4/Workspace/`（Hub 与 homepage 在 TBU 上另有检出） |

> ⚠️ **挂载点**：`/Volumes/TBU4` **经常未挂载**。**会话工作检出、git root、编辑落点一律是 `/Volumes/TBU/Workspace/Aleph`**——`TBU4/Workspace/Aleph` 是第二份检出，别跨盘编辑。动周边仓前先 `ls /Volumes/`，别从一次 TBU4 miss 得出"参考项目不可用"。
> 7 仓为同级兄弟目录，远端均在 `github.com/rootazero/`。**始终从主项目 `Aleph/` 启动会话**，周边仓作为兄弟目录就地操作——这样跨会话记忆统一沉淀到主项目的全局 memory 库，spec/plan 统一落在 `docs/superpowers/{specs,plans}`（`docs/` 树已纳入 git）。周边仓的 spec 以子项目名作文件名前缀。

---
## 🧠 长期记忆与质量门 (Memory & Hooks)

- **长期记忆**：走全局 `~/.claude/projects/.../memory/`（跨会话、Git 不追踪）。**不在项目内另造 MEMORY.md**——避免与全局记忆双源冲突。
- **质量门 (Hooks)**：当前**未挂** `.claude/hooks/`。本文件的规则目前靠模型遵守；未来如需强制执行层（如 PostToolUse → `cargo fmt`），在 `.claude/hooks/` 配置即可。
