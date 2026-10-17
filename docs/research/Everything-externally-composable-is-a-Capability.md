# Everything externally composable is a Capability

> **Aleph Zahir 总纲 · v3（2026-10-02 修订）**
> v2 原文备份：`Everything-externally-composable-is-a-Capability.v2.bak.md`
>
> **Zahir** —— Aleph 的 harness 名，与 dsh 的 **Cordis**、Pi 的 **Durable** 同级。
> 一句话：**注册一次，处处投影；所有权可撤销，效果可追溯。**

---

## 〇、先读这一节：怎么在重构会话里用这份文档

**这份文档的性质**：它是**总纲**，不是 spec，也不是实现计划。它规定方向、边界和判据；具体的类型、文件、迁移步骤由新一轮的 brainstorming → spec → plan 产出。

**开工前置条件（按顺序）**

1. `plugin-scope-round`（插件作用域 + CC 兼容 + MCP 面）**已合并进 main**。那一轮在插件维度做了"效果作用域 + 可撤销注册"，正是本总纲要推广的第一块。
2. **第一步是清点，不是写 spec**（见 §十四）。清点结果决定这是一次"整理"还是"真正升一级"，也决定范围。v3 中出现的 Aleph 路径与行数来自 2026-10-02 的一次**抽样探索**，不是 §十四 要求的正式清点（没有逐项附谓词与 commit）。
3. 标注 `[待核实]` 的断言，写进 spec 之前先对照源码核实。v3 中关于 **Pi Durable** 的断言已对照 `/Volumes/TBU4/Github/pi/packages/durable`（`README.md`、`docs/spec.md`、`src/harness/tool.ts`）与官方博文核实；关于 **dsh / Pi coding-agent** 的断言仍为 `[待核实]`。
4. CLAUDE.md 写着"对照表已做完：pi · deepseek-harness"。本轮**修订**那两份对照，并补一份 Pi Durable 对照，不重做。

**版本变更**

| 版本 | 类别 | 改动 |
|---|---|---|
| v2 | 坚持 | ① 能力语义与插件装配分离 ② 所有权 + 可撤销副作用推广到插件之外 ③ 协议只是能力的投影——**真正的收益是让一类缺陷从结构上写不出来** |
| v2 | 推回 | ① 不是白纸：收敛现有注册表 ② 依赖图不在会话中途增删工具 ③ Scope 只建有消费者的级别 ④ 分类区分"种类"与"关系" ⑤ Resolver 不按消息内容挑能力 ⑥ 优先级按现状重排 |
| **v3** | 命名 | harness 正式命名 **Zahir**（§一）；文中 "Capability Kernel" 统一改为 Zahir |
| **v3** | 新增 | **从 Pi Durable 学什么**（§4.2）：持久性是能力描述的一部分——effect sandwich、replay 声明、所有权树、exactly-once 提交、durable memo |
| **v3** | 修订 | §六 已有形态表填入抽样路径；§七 Service 判为 relation、descriptor 增加持久性字段；§八 区分**可见性 Scope** 与**生命周期 Scope**；§九 补"可撤销"之外的"可恢复"；§十一 四件难事附 Pi Durable 参考答案 |
| **v3** | 推回 Pi Durable | 不照搬"一条原子提交线 + 文档存储"（§4.3） |

---

## 一、Aleph 与 Zahir 的定位

### 1.1 命名：Zahir

> 博尔赫斯《扎伊尔》（*El Zahir*）：一枚硬币，见过它的人再也无法忘记它，它在意识里反复浮现，直到成为唯一能看见的东西。

隐喻两层：

1. **单一真源**：一个能力只注册一次，工具面 / RPC / MCP / CLI / 事件 / Panel 都是它的投影——你绕不开它，也注册不出第二份（§12.1）。
2. **不会遗忘**：进程死了，已承诺的意图、所有权、未完成的工作仍然被记住；重启后从它们继续（§4.2、§9.4）。

与同类命名的关系：

| 项目 | harness 名 | 名字强调什么 |
|---|---|---|
| DeepSeek Harness（dsh） | **Cordis**（拉丁语 "心的"） | 一切皆插件的组合核心 |
| Pi | **Durable** | 持久这一**属性** |
| Aleph | **Zahir** | 单一真源 + 不遗忘这一**结构** |

同出博尔赫斯：Aleph 是"包含一切点的点"，Zahir 是"让人只能看见它的那个物"——一个是整个系统，一个是系统里不可绕过的那一层。

**用法约定**

- 文档与对外：`Aleph Zahir`，简称 Zahir。
- 代码：**不为改名而改名**。若 spec 决定新建模块 / crate，命名用 `zahir`；收敛进已有模块时沿用原名（R3，判据：衡量的是删了多少，不是建了多少）。
- CLI：子命令以 §十六 清点后的 `al` 命令树为准，不预先占用 `al zahir`。

### 1.2 Aleph 是什么，Zahir 是什么

**Aleph 不是白纸，而是一个完整的 agent runtime。** 抽样探索看到的现有实现：

- Agent loop：`src/harness/`（约 19.6k 行含测试；`AgentHarness::run()` 在 `src/harness/agent.rs`）
- SQLite 会话事件日志：`src/session/store.rs`（3153 行），由 `src/session/actor.rs` 在启动时回放
- 崩溃恢复：`src/gateway/resume_coordinator.rs`（3746 行，boot-scan）+ `src/session/boundary_repair.rs` + `src/session/reduction.rs`
- 可撤销注册：`src/extension/effects/`（`EffectScope` + `Disposer`）
- 多套持久化任务：Cron（`src/tasks/cron/store.rs`）、`agent_tasks` 表（`src/resilience/database/state_database/schema.rs`）、子 agent 恢复标记
- 多种界面：Desktop / CLI / TUI / webchat

**Zahir 是 Aleph 的 harness 层**——夹在 Agent loop 与执行基底之间，负责能力的**注册、所有权、撤销、恢复与投影**：

> **Zahir = Registry · Scope · Effects · Recovery · Projection**

它不是一个与 Aleph 并列的新 runtime，也不进 `src/harness/`（R10）。它是把现有若干注册表、清理路径、恢复启发式**收敛**成一套统一规则后的名字。

```
                    ┌─────────────────────────────┐
                    │  Aleph Agent Runtime         │
                    │  Agent Loop（src/harness/）   │
                    └──────────────┬──────────────┘
                                   │ resolve / invoke / observe
                    ┌──────────────▼──────────────┐
                    │            Zahir             │
                    │ Registry · Scope · Effects   │
                    │ Recovery · Projection        │
                    └──────────────┬──────────────┘
          ┌───────────┬────────────┼────────────┬───────────┐
          ↓           ↓            ↓            ↓           ↓
        Tool      Resource    EventSource     Task        Agent
          │           │            │            │           │
   Browser/Shell  Files/DB/   hooks/streams  Cron/jobs  Sub-agent/
   Code/GUI       Memory                     background  ACP peer
```

### 1.3 对外关系：互操作，不是寄生

dsh、Pi、Claude Code、Codex 可以经 MCP 调用 Aleph 的工具、经 ACP 调用 Aleph 的 Agent；Aleph 也经 MCP / ACP client 调用它们。这些都是 Zahir 能力的**投影**或 `implements` 关系：

```
Aleph ≠ dsh plugin   ·  Aleph ≠ Pi extension
Aleph ≠ CLI tool     ·  Aleph ≠ MCP server
```

---

## 二、从 dsh / Cordis 学什么：所有权、依赖与副作用生命周期的制度化

以下关于 dsh 的描述来自 v1，**`[待核实]`**：

- dsh 自称 "no privileged core"：model adapter、tool registry、session、agent、agent-loop 都以 plugin / service 身份参与组合，经 services、typed events、reversible effects 组成运行时。
- `ctx.tools` / `ctx.llm` / `ctx.agents` 是 Service，插件经 `inject` 声明依赖；依赖未就绪插件不运行，必需服务消失时依赖它的插件自动卸载，服务恢复后可重载。
- `ctx.on()` / registry 注册 / `ctx.plugin()` 挂子插件等动作绑定到所属 fiber 的生命周期，unload 时自动撤销；timer / socket / watcher 可经 `ctx.effect()` 显式绑定 disposer。

dsh 真正先进的地方不是"插件多"，而是**把能力的所有权、依赖和副作用生命周期制度化了**：

```
Capability → Owner / Scope → Effects → Mount → Active → Unmount → 全部撤销
```

而不是：

```
register() register() register() ... 最后祈祷没人忘记 cleanup()
```

**为什么这对 Aleph 是真需求而不是品味**：Aleph 反复付账的几类缺陷，根子都是"谁拥有、谁清理"没有制度化——测试临时文件泄漏、进程级静态缓存被污染（`static GLOBS` 一类）、注册了却没有派发（判据 §7）、报成功却什么都没发生（判据 §11）。

**已有的第一块**：`plugin-scope-round` 在插件维度落地了效果作用域 + 可撤销注册。`src/extension/effects/mod.rs` 的模块注释写明了吸收边界：*Absorbed from Cordis `fiber.ts:418-561` … as an ownership rule only: no DI container, no Proxy context, no cascade restart*。**这三条"不引"的裁决在 v3 继续有效**；本轮是把所有权规则推广到会话、子 agent、MCP 连接、浏览器会话等维度。

---

## 三、不照搬 dsh：Plugin 是装配机制，Capability 才是语义

```
dsh:                              Aleph Zahir:
Plugin                            Capability（kind，§七）
 ├── provides Service              ├── Tool
 ├── registers Tool                ├── Resource
 ├── listens Event                 ├── EventSource
 └── owns Effects                  ├── Task
                                   └── Agent
                                  + relation：implements / requires / owned-by
```

**Plugin 是部署/装配机制；Capability 才是能力语义。** 这一点 Aleph 已经在隐式地做：浏览器双引擎背后是一套接口、provider 背后是 trait、沙箱背后是平台实现。Zahir 给它一个正式名字和一套统一的所有权规则。

```
Browser  ← implements ← Obscura / Chromium（均经 crates/aleph-cdp）
Shell    ← implements ← Local / Sandbox / SSH / Container
LLM      ← implements ← OpenAI / Anthropic / DeepSeek / Local
Memory   ← implements ← Working / Long-term / Vector / Graph
```

Agent 看到的是统一的 Capability，不知道也不应该知道底下是哪个实现。

---

## 四、从 Pi 学什么

### 4.1 Pi coding-agent：极小核心 + 极强扩展面（`[待核实]`）

- Pi 定位 minimal terminal coding harness，把 workflow 功能放在 extensions / skills / prompt templates / packages。
- 默认工具极少（read / write / edit / bash），但有正式 extension API：注册 Tool / Command / Shortcut / Provider，监听并修改 tool call / tool result / agent 生命周期 / prompt，支持热重载。
- 有 package 系统（npm / Git / 本地路径），支持 RPC、JSON event stream、SDK embedding。MCP 不在核心。

"Bash-only"不是准确描述。Bash 的意义是 **Default primitive + Universal escape hatch**。Aleph 吸收这个思想，但不复制 Pi 的 coding-agent 定位。

### 4.2 Pi Durable：持久性是能力描述的一部分（已核实）

Pi Durable（`@earendil-works/pi-durable`，`packages/durable/src` 约 17.7k 行）是 Pi 1.0 同期发布的实验包。**它不是单纯的存储层**：Harness 自带 generation / tool / compaction 三种内置任务，agent loop 就在里面；Pi coding-agent 下的 `src/experimental/durable/` 只是一个约 7 个文件的示例应用。官方定义：

> A harness is storage plus the machinery needed to run one or more conversations … Everything the harness runs, from calling the model to executing a tool, is a task.

规范（`docs/spec.md`）的核心规则：*A Session atomically commits immutable entries, full task records, and Chord-tracked documents. Only committed state is observable.*

对 Zahir 有用的不是它的存储形态，而是下面七条机制。每条都标出 Aleph 现状（来自抽样探索）：

| # | Pi Durable 机制 | 要点 | Aleph 现状 | Zahir 取舍 |
|---|---|---|---|---|
| D1 | **Effect sandwich**（spec §5.2） | `commit intent → perform effect → commit outcome`；重开时停在 intent 阶段 = "效果可能已发生"，由 phase handler 重试 / 轮询外部句柄 / 记为中断 | 有同形的零件：`ResumeAttempted` 意图戳（`src/session/events.rs`）、`RunStarted`/`RunFinished` 标记、判据 §15 的意图戳规则 | **吸收为规则**：凡跨不可逆边界的 effect 都走三明治（§9.4） |
| D2 | **Tool replay 声明** | `defineTool({ replay: "safe" })`，默认 `unsafe`；恢复时**存储的 intent 与当前定义都说 safe** 才重跑，否则回报 "interrupted and may have partially run"（`src/harness/tool.ts:93-110`） | 无声明。`boundary_repair.rs` 用 `DanglingProvenance::{ThisRestart, EarlierRun}` 与 denied / parked 状态**推断**措辞，最坏回报 "OUTCOME UNKNOWN" | **吸收**：replay 策略进 descriptor，由 recovery 读取（§7.3）。这是"把启发式变成声明"的直接实例 |
| D3 | **所有权树 + 前台/后台** | 任务与会话同一棵所有权树；abort **自底向上**，每个任务先清理自己的效果；**任务只在其拥有的工作完成后才算完成**。前台任务属于当前工作（Esc 会中止）；后台任务属于会话但不属于当前工作 | 取消经 `CancellationToken::child_token()` 派生（`src/agents/subagent_tool/spawn.rs`）、团队树级 poison token（`src/teams/broadcast/`）。**未见"父等子完成"与自底向上清理** | **吸收**为 Scope 树语义（§八、§9.4） |
| D4 | **exactly-once 提交** | `requestId` 使 submission 恰好一次，崩溃后重试拿回原 submission | `src/gateway/idempotency.rs` 是 RPC 级、**进程内** `DashMap` + TTL——重启即丢，不跨崩溃 | **吸收**：提交幂等键须落盘（§十一 第 5 条） |
| D5 | **Durable memo** | 任务内 first-writer-wins 小值；hook 的决策（如审批结果）存 memo，重启后不再重问 | `[待核实]`：审批结果是否跨重启保留未查 | **吸收**为 hook / 审批的持久化约定（§十一 第 6 条） |
| D6 | **存名字，不存代码** | 会话只存 extension / tool **名字**；registry 可在运行中替换，已在跑的调用用旧代码跑完，下一次用新代码；重启后取新进程装的实现 | 插件热重载存在（`src/gateway/hot_reload.rs`），未核对"运行中调用用旧代码跑完"的语义 | **吸收**为 Registry 规则（§10.3） |
| D7 | **工具 / 提示变化入转录** | system prompt 与工具集的变化记录在 transcript 的**变化发生处**，fork / 重启看到模型当时所见；支持的模型只发 diff 保 prompt cache | v2 规则：会话中途不增删工具 | **保留 v2 规则**；D7 作为将来放宽时的唯一合法路径记录在案（§10.4） |

另有两条值得引用的"非目标"（spec §13）：Pi Durable **不做**"进程内强制终止不合作的扩展代码"，也**不做**自动 checkpoint 启发式。前者与 Rust 没有 async Drop 是同一类现实（§9.2）：所有权树只能**请求**清理，不能**强制**清理。

### 4.3 不照搬 Pi Durable

| Pi Durable 的选择 | 为什么 Aleph 不照搬 |
|---|---|
| **一条原子提交线**：entries / tasks / documents 同一事务 | Aleph 的会话日志、任务表、记忆索引分属不同存储，各有事务边界与消费者。合并它们是一次存储层重写，不是 harness 收敛，超出本总纲范围。Zahir 要的是**每个边界上的生命周期规则一致**，不是一个 DB。`[spec 须论证]`：若某条跨存储的不变量确实需要原子性，单独提出 |
| **Documents（Chord 类型化 JSON + fork 策略）** | Aleph 没有对应消费者；按 YAGNI 不引。若将来出现"应用状态必须与转录一致"的真实需求，D1 的意图戳已能覆盖大部分 |
| **TypeScript / 单线程事件循环** | Rust/tokio 下 dispose、取消、Drop 都更难（§9.2） |
| **无内置 subagent，靠工具几行代码组装** | Aleph 已有子 agent 体系与恢复标记；不倒退，只把它们挂进所有权树 |

---

## 五、dsh / Pi / Pi Durable / Aleph Zahir 对照

| 维度 | dsh（Cordis） | Pi coding-agent | Pi Durable | Aleph Zahir 怎么吸收 |
|---|---|---|---|---|
| 核心 | Plugin / Service Graph | Minimal Agent Core | Harness = storage + task machinery | **收敛现有注册表**（§六） |
| 依赖 | 强依赖声明 | 较弱 | 无 DI；models / env 在 `Harness.open` 注入 | 依赖声明 + 会话边界生效（§十） |
| 生命周期 | 强、可逆 | 有 lifecycle | 所有权树，自底向上 abort | 推广 `EffectScope` + 所有权树（§八、§九） |
| 持久性 | `[待核实]` | 进程死了人来续 | **一切步骤皆 checkpoint 任务** | **effect sandwich + replay 声明**，不合并存储（§4.2、§4.3） |
| Tool | 一等公民 | 一等公民 | `defineTool` + `replay` | descriptor 带 replay / owner（§7.3） |
| Scope | Context / Agent | session / package / project | conversation / task 所有权 | 可见性与生命周期分开（§八） |
| 热重载 | 深度融入 | extension reload | 同名替换，存名不存码 | 吸收 D6（§10.3） |
| CLI / RPC / SDK | 有 | 很强 | SDK + 多客户端 attach | 扩展已有 `al` 与 JSON-RPC Gateway |
| 极简性 | 较低 | 很高 | 高（agent 可读完全部源码） | 核心保持小（R3 / R10） |

最好的 Zahir 不是三者折中，而是：**Cordis 的所有权纪律 + Pi 的极小核心 + Pi Durable 的持久语义 + Aleph 自己的单一真源投影**。

---

## 六、Zahir：收敛，不是新建

### 6.1 结构

```
┌────────────────────────────────────────────────────┐
│                  Aleph Agent Layer                 │
│   Agent Loop（src/harness/，R10 锁定）/ Sub-Agent   │
└─────────────────────────┬──────────────────────────┘
                          │  resolve / invoke / observe
┌─────────────────────────┴──────────────────────────┐
│                       Zahir                        │
│  Registry · Scope · Effects · Recovery · Policy    │
│  Schema · Event · Cancellation · Projection        │
└──────────────┬──────────────┬──────────────┬───────┘
             Tool         Resource       EventSource
             Task          Agent
```

Agent Loop 不直接拥有能力，只能：`resolve(capability) → invoke(capability) → observe(events)`。这与 R10「薄 Harness，笨循环」同向，并且**不需要改动 `src/harness/`**——Zahir 落在工具呈现层与运行时层。

### 6.2 ⚠️ Aleph 不是白纸：已有形态（抽样，待 §十四 正式清点）

| Zahir 组件 | Aleph 已有形态（路径已确认存在） | 收敛方向 |
|---|---|---|
| Registry | `ToolCatalog`（`src/tool_metadata/registry/mod.rs`）、RPC `HandlerRegistry`（`src/gateway/handlers/mod.rs`）、插件注册（`src/extension/`）、skill 注册、MCP manager（`src/mcp/manager/`）、ACP harness manager（`src/acp/manager/`）、provider 预设表 | 按 kind 收敛；每合并一个删一个 |
| Scope / Effects | `EffectScope` + `Disposer`（`src/extension/effects/`），逆序 dispose；覆盖 `registry_row` / `wasm_module` / `mcp_server` / `service` / `memory_extension` / `slash_command` 六类 | 推广到 Session / Run / Task；补浏览器会话、子进程、临时目录 |
| 可见性 | `ScopeKey::{Global, Project(PathBuf)}`（`src/extension/visibility.rs`） | 保持；与生命周期 Scope 分开（§八） |
| Recovery | `resume_coordinator.rs`、`boundary_repair.rs`、`reduction.rs`（`RunDisposition`、`DanglingProvenance`） | 读 descriptor 的 replay 声明，替代措辞推断（D2） |
| Cancellation | `CancellationToken` 派生（`src/agents/subagent_tool/spawn.rs`）、团队 poison token（`src/teams/broadcast/`） | 挂到 Scope 树上，补"父等子完成"（D3） |
| Policy / Permission | 审批门链（`src/tools/scoped/gate_chain.rs` 等）、`[sandbox.command_policy]`、工具权限三层、principal / spend ledger | 保持；投影时 principal 来源见 §十一 |
| Idempotency | `src/gateway/idempotency.rs`（进程内） | 补落盘的提交幂等键（D4） |
| Schema | schemars | 保持 |
| Event Bus | `src/gateway/events/`、插件 hooks | 保持，事件面从 descriptor 投影 |

**本轮真正的工作不是"设计一个内核"，而是"把现有的 N 个注册表、M 条清理路径、K 处恢复推断收敛成一套规则"。**

最大的风险是：Zahir 与旧注册表**并存**。那正是判据 §1「同一事实的两份表述」。硬规则：

- **每引入一个 Zahir 组件，同一轮里必须删掉它取代的旧结构**；做不到删除就不引入。
- **衡量成功的指标是删掉了几个旧东西，不是新建了几个模块。**

### 6.3 ⚠️ Resolver 必须守住 R7

只允许两种解析方式：

1. **静态**：按配置 / 作用域 / 权限做的、不看消息内容的分区（与 `src/tools/scoped/` 的静态 `retain` 同性质）；
2. **模型发起**：如 `tool_search`——加载决策 100% 由模型做出。

**禁止**：Resolver 按用户消息内容挑选或过滤能力（R7 意图路由、R10 第二个"不"）。

---

## 七、Capability 的分类与描述

### 7.1 种类（kind）与关系（relation）

```
Capability kind（对外呈现，面向消费者）
├── Tool         模型可以主动调用
├── Resource     Agent 可以读取 / 订阅
├── EventSource  持续产生事件
├── Task         异步 / 后台工作（可 checkpoint）
└── Agent        可被调用的智能体（子 agent、ACP peer）

Capability relation（内部结构，面向运行时）
├── implements   Obscura implements Browser
│                Claude Code（经 ACP harness）implements Agent
│                外部 MCP server implements 一组 Tool / Resource
├── requires     BrowserTools requires Browser
└── owned-by     BrowserSession owned-by Session scope
```

### 7.2 Service 是 relation 的一端，不是 kind（v2 遗留问题的裁决）

v2 把 Service 的去留交给 spec。v3 给出裁决和判定问句：

> **判定问句**：模型或运行时之外的系统，是否需要**在运行时按名字**发现、调用、订阅它？
> 是 ⇒ Capability kind；否 ⇒ 实现细节，最多作为 `implements` / `requires` 关系的一端出现。

- `Browser` 是 Service：模型从不直接调用 "Browser"，它调用 `navigate` / `click`（Tool），读 `page_state`（Resource）。Browser 只出现在 `BrowserTools requires Browser`、`Obscura implements Browser` 里。
- 旁证：Pi Durable 的 registry 里也没有 Service——models 与 execution env 在 `Harness.open` 时作为依赖注入，不进扩展注册表。

⚠️ 分类法写进类型系统前，仍须用 §十四 清点出的真实实例逐一试分类；分不进去的实例就是分类法的缺陷。

### 7.3 Descriptor：一份描述里包含持久语义（v3 新增）

v2 的 descriptor 只描述"是什么、怎么调"。v3 要求它也描述**崩溃后怎么办**——否则 recovery 只能继续靠推断（D2）。示意（以 spec 为准）：

```rust
pub struct CapabilityDescriptor {
    pub name: CapabilityName,      // 持久层只存名字（D6）
    pub kind: CapabilityKind,
    pub schema: SchemaRef,
    pub replay: ReplayPolicy,      // 仅 Tool / Task 有意义
    pub lifetime: LifetimeHint,    // 默认挂在哪一级生命周期 Scope
}

pub enum ReplayPolicy {
    /// 只读或天然幂等：中断后可重跑。
    Safe,
    /// 默认。中断后不重跑，向模型回报"被中断，可能已部分执行"。
    Unsafe,
    /// 带外部幂等键（如支付 key = task id）：重跑由键保证只生效一次。
    Keyed,
}
```

- **默认 `Unsafe`**，与 Pi Durable 一致：漏标的工具得到的是"保守地不重跑"，而不是"悄悄重跑一次部署"。
- 恢复判定取**意图戳里记下的策略**与**当前 descriptor 的策略**两者都允许才重跑（同 `tool.ts` 的双重检查），防止热重载后策略变松导致误重跑。
- `Keyed` 是否需要单列，由 spec 依据清点出的真实工具决定；若没有消费者，删掉它。

**浏览器示例**：

```
Browser
├── Tool         navigate / click / type / screenshot     replay: navigate=Safe? click=Unsafe
├── Resource     page / DOM / accessibility_tree / page_state
├── EventSource  page_loaded / network_request / navigation_changed / user_interaction
└── relations
    ├── implemented by  Obscura / Chromium（均经 crates/aleph-cdp）
    └── owned-by        BrowserSession
```

"上帝之手"、双向 IPC、鼠标事件拦截、网页空间状态，都只是 Browser 的不同事件与操作面，不是塞进 Agent Loop 的特殊逻辑。

---

## 八、Scope：可见性与生命周期是两件事

### 8.1 ⚠️ v3 纠正：不要把 `ScopeKey` 当生命周期 Scope

`ScopeKey::{Global, Project}` 回答的是"**谁看得见**"（`visible_to(key, ctx)`）；`EffectScope` 回答的是"**谁拥有、谁清理**"。两者正交：一个 Project 可见的插件，其效果仍归插件的 `EffectScope`。把两者合进同一个层级枚举，会让"可见性变化"被误读成"需要 dispose"。

| 维度 | 问题 | 现有实现 | 级别 |
|---|---|---|---|
| 可见性 Scope | 谁看得见这个能力 | `ScopeKey` | Global / Project |
| 生命周期 Scope | 谁拥有、何时撤销、崩溃后如何重建 | `EffectScope`（插件级） | 见 8.2 |

### 8.2 生命周期 Scope：只建有消费者的级别

| 级别 | 抽样看到的消费者 | 持久性 | 重启时 |
|---|---|---|---|
| **Runtime（含插件）** | 插件 `EffectScope`、全局 provider | 不持久 | 从配置重新装配 |
| **Session** | 会话事件日志、会话级 MCP 连接、子 agent 生命周期 | **持久**（`session/store.rs`） | 回放日志重建 |
| **Run** | `AgentHarness::run()` 的取消令牌、run 预算、`RunStarted`/`RunFinished` | 标记持久，句柄不持久 | `resume_coordinator` 依标记续跑 |
| **Task** | Cron job、`agent_tasks` 表、子 agent `SubagentSpawned`/`SubagentReturned` | **持久**（多套表） | 各自恢复路径，待统一 |

- **Turn Scope 不建**：抽样未发现消费者；会碰 R9 / R10 并打破 prompt cache。
- 准确名单仍以 §十四 清点为准；找不到消费者的级别不建，其余按三次法则。

### 8.3 Scope 树的规则（吸收 D3）

1. Scope 是**树**：子 Scope dispose 先于父 Scope（自底向上）。
2. **父 Scope 只在其拥有的工作全部终结后才算终结**——今天的 `child_token()` 只做到"父取消则子取消"，没做到"父等子"。
3. **前台 / 后台**：前台工作属于当前 Run，用户中止 Run 会波及它；后台工作挂在 Session 下，Run 中止不波及，须显式中止它或其 Session。
4. 每个 Scope 必须能回答"它属于哪个 principal"（§十一）。

---

## 九、Reversible Effects 与 Durable Effects

任何 Capability 注册行为都必须有 owner：

```
mount Tool        → owner = Session（工具集在会话边界确定，§十）
subscribe Event   → owner = Session
open Browser      → owner = BrowserSession
spawn Process     → owner = Task
connect MCP       → owner = Session

Owner.dispose() → unsubscribe / unregister / terminate / close / flush / release
```

### 9.1 ⚠️ 可撤销的边界

**能撤销的是"注册"，不是"世界上发生过的事"。**

| 可撤销（Zahir 负责） | 不可撤销（Zahir 只负责停止与记录） |
|---|---|
| 工具 / 事件订阅 / 路由的注册 | 已写出的文件、已发出的网络请求 |
| MCP 连接、浏览器会话、子进程句柄 | 已派生进程对外部世界造成的效果 |
| 临时目录（若由 Scope 创建并拥有） | 已发送的消息、已扣的花费 |

对不可撤销的那一列，dispose 只能做到"停止继续产生效果"。

### 9.2 ⚠️ Rust 的现实：没有 async Drop

Cordis 依托 JS 单线程事件循环；Pi Durable 也明确不做"强制终止不合作的扩展代码"。在 Rust / tokio 里以下更难，spec 必须逐条回答：

1. **没有 async Drop**：Scope 被 drop 而没有显式 `dispose().await` 时怎么办？（同步兜底？泄漏告警？禁止隐式 drop？）
2. **dispose 失败**：`Err` 时父 Scope 继续撤销兄弟效果还是停止？失败的效果处于什么状态？（`Err` 不得读作"已撤销"——判据 §8。现有 `DisposeReport` / `DisposeOutcome` 是起点。）
3. **撤销顺序**：同一 Scope 内逆注册序（`EffectScope` 已如此），跨 Scope 自底向上。
4. **与取消的交互**：正在执行的 invoke 所在 Scope 被 dispose 时，invoke 如何被取消、结果归谁。

### 9.3 类型草图（仅示意，以 spec 为准）

```rust
pub struct Scope {
    pub id: ScopeId,
    pub parent: Option<ScopeId>,
    pub principal: PrincipalId,   // §十一
    pub foreground: bool,         // §8.3 规则 3
    pub effects: EffectSet,
}
```

```
Session.dispose() → Run.dispose() → Task.dispose()
                  → Browser.dispose() → MCP.dispose() → Tool unregister()
```

### 9.4 Durable Effects：从"可撤销"到"可恢复"（v3 新增，吸收 D1 / D2）

可撤销回答"正常结束时怎么清理"；可恢复回答"**进程半路死了**，重启后怎么办"。Zahir 对每一个跨不可逆边界的 effect 施加**效果三明治**：

```
① 落盘意图（含 descriptor 的 replay 策略、参数、所属 Scope）
② 执行外部效果
③ 落盘结果 / 下一阶段
```

重启时停在 ① 与 ③ 之间 = **效果可能已发生**。处理只有三种，按 descriptor 选择，不由 recovery 猜：

| replay | 重启后 |
|---|---|
| `Safe`（且意图戳也是 Safe） | 重跑 |
| `Keyed` | 带同一幂等键重跑 |
| `Unsafe`（默认） | 不重跑；向模型回报"被中断，可能已部分执行"，由模型决定（A2：让模型看见并自愈；R10 第 5 不） |

这把 `boundary_repair.rs` 的措辞推断变成声明驱动：**措辞保留**（provenance、denied、parked 仍有价值），**是否重跑**改由 descriptor 决定。判据 §15（只记录"做完了"的机件分不出"没做"和"做了没记上"）在此成为结构，而不是靠每个工具作者记住。

---

## 十、Dependency Graph：声明依赖，但不在会话中途增删工具

```
BrowserTool    requires: browser.page, browser.input, permission
CodeExecution  requires: filesystem, subprocess, sandbox
WebResearch    requires: browser, network, memory
```

### 10.1 ⚠️ 推回 v1 的"自动停用 / 自动重启用"

1. **R10 与 prompt cache**：会话中途工具集变化会打破 prompt cache，也会让模型困惑。
2. **判据 §8**："依赖消失"只能说"我不知道"，不能读作"这个工具不存在"。
3. **Rust 实现成本**：见 §9.2。

### 10.2 修订后的语义

- **工具集在会话边界（或显式 reload）时确定**，会话中途不增删。
- 依赖不可用时，工具**仍在列表中**，调用返回**结构化的"暂不可用"错误**（原因、是否可重试），让模型看见并自愈。
- 依赖图用于：启动 / reload 的装配顺序、Scope dispose 的撤销顺序、`doctor` 诊断。

### 10.3 热替换：存名字，不存代码（吸收 D6）

- 持久层（会话日志、任务表、意图戳）只引用能力的**名字**，从不引用实现。
- 同名重新安装 = 原子替换；**已在执行的调用用开始时的实现跑完**，下一次调用用新实现。
- 重启后按名字解析到新进程装配的实现；名字解析不到时，按 §10.2 返回结构化"暂不可用"，而不是让恢复崩溃。

### 10.4 若将来要放宽"会话中途不增删工具"

唯一合法路径是 D7：**变化本身作为条目写入转录、位于变化发生处**，使 fork / 重启 / 回放看到模型当时所见；且只在支持增量工具变更的 provider 上发 diff。不满足这两条的放宽一律不做。本轮不做。

---

## 十一、spec 必须回答的难事（v3：附 Pi Durable 参考答案）

| # | 难事 | 参考答案（非结论） |
|---|---|---|
| 1 | **按作用域的权限**：每个 Scope 属于哪个 principal？子 Scope 能否拿到更大权限？投影到 MCP / CLI / Panel 时调用方 principal 从哪来？ | Pi Durable 无多 principal，不可参考。Aleph 自己回答；默认"子不大于父" |
| 2 | **重启后 Scope 怎么重建**：哪些持久、哪些随进程死亡？"半撤销"的 Scope 是什么状态？ | §8.2 持久性列 + §9.4 效果三明治：持久的是**意图与名字**，不是句柄；半撤销 = 意图戳在、结果戳不在，按 replay 处理 |
| 3 | **schema 跨 crate、跨 wire 的版本契约**：descriptor 投影到 MCP / RPC / CLI 时谁是真源？旧客户端读到什么？ | Pi Durable 给任务 / 文档定义带 `version` 字段与迁移（spec §3.6）。Zahir 的 descriptor 至少需要同样的版本号 |
| 4 | **可撤销的边界** | §9.1 |
| 5 | **提交幂等跨崩溃**（v3 新增）：客户端在崩溃后重试同一条消息，会不会跑两次？ | 参考 D4：幂等键随 submission 落盘，重试返回原 submission。Aleph 今天的 `idempotency.rs` 在进程内，需决定键落在 Gateway 还是会话日志 |
| 6 | **hook / 审批决策跨崩溃**（v3 新增）：重启后审批会不会重问、或者被重复执行？ | 参考 D5：决策作为 first-writer-wins memo 与所属任务一起落盘 |

---

## 十二、协议是投影：一份描述，多个出口

```
                          Zahir
                           │
              ┌────────────┼────────────┐
              ↓            ↓            ↓
             MCP          CLI          RPC
              │            │            │
        ┌─────┴────┐       │       external apps
        ↓          ↓       ↓
       dsh        Pi     Unix
```

Aleph 内部：`CapabilityDescriptor / CapabilityHandler / CapabilityScope / CapabilityEffect`。
MCP 只是：`Capability → MCP Tool / Resource / Prompt representation`。新协议 = 新增一个 adapter，不动 Core。

### 12.1 这才是"升一级"的真正来源

判据 §9「一个动词有几张脸，判据就要在每张脸上用同一个推导」是 Aleph **最常复发**的一类缺陷。如果**每个能力只有一份 descriptor，每张脸都从它投影出来**：

- §1「同一事实的两份表述」——在能力元数据上写不出来了；
- §7「两端完整而中间没线」——注册即派发；
- §9「一个动词几张脸」——每张脸同一个推导；
- §11「报成功的 no-op」——修在 invoke 路径上，一次覆盖全部能力；
- **§15「分不出没做和做了没记上」**（v3 新增）——replay 策略在 descriptor 里，恢复从它推导。

**让判据清单里的几条从"要靠人记住"变成"写不出来"——这才是提升一个层级。**

### 12.2 红线对照

| 红线 | 本总纲的约束 |
|---|---|
| **R3** 核心轻量化 | Zahir 由现有结构收敛而成，不引入重三方库；不引 Chord / 文档存储（§4.3） |
| **R4** Interface 层无业务逻辑 | CLI / Panel / 宿主适配器只做投影与 I/O |
| **R7** LLM 主权 | Resolver 不按消息内容挑能力（§6.3）；中断与依赖不可用都交给模型处理（§9.4、§10.2） |
| **R8** 工具即一切 | Capability 的管理操作本身也是能力 |
| **R9** 智慧在 Prompt 中 | 不做 Turn 级 prompt 贡献（§8.2） |
| **R10** 薄 Harness | Zahir 不进 `src/harness/`；会话中途不增删工具（§十） |

### 12.3 ⚠️ ACP 与 MCP 都是双向的：别和现有 ACP 做重复

**现有 ACP（2026-09-28 grep 核实，未通读 `src/acp/manager/`）**

- `src/acp/` 的方向是 **Aleph 作为 ACP client**：驱动外部 CLI agent（Claude Code / Codex / Gemini 的 `--acp`），支持 Tool mode 与 Agent mode。
- `src/acp/incoming.rs` 是这条连接的**反方向半边**：外部 agent 回头请求 `fs/read_text_file` / `fs/write_text_file` / `session/request_permission` / `terminal/*`，文件操作限定在会话工作目录内，权限请求接入 Aleph 审批门。
- 消费方：团队调度（`src/teams/dispatcher/acp_bridge.rs`）、Panel 的 ACP harness 管理页、会话持久化（`src/acp/manager/persistence.rs`）。
- **Aleph 作为 ACP agent（被别人调用）的那一侧目前不存在**：接收 `session/new` / `session/prompt` 的只有测试用的 `src/acp/mock_server.rs`。

| 协议 | 方向 | 在 Zahir 里的位置 | 现状 |
|---|---|---|---|
| ACP | Aleph → 外部 agent（client） | Layer 1 / relation：外部 agent `implements` Agent | 已有：`src/acp/` |
| ACP | 宿主 → Aleph（agent 侧） | Layer 4：把 Aleph 的 Agent 投影为 ACP agent | 缺 |
| MCP | Aleph → 外部 MCP server（client） | Layer 1 / relation：外部 server `implements` 一组 Tool / Resource | 已有：`src/mcp/manager/` |
| MCP | 宿主 → Aleph（server 侧） | Layer 4：把 Aleph 的能力投影为 MCP Tool / Resource | 已有：`/mcp` 面 |

**三条硬规则**

1. **每个协议的报文类型只有一份。** ACP agent 侧必须复用 `src/acp/protocol.rs`；MCP 同理。位置不合适就**移动**，不复制（判据 §1 / §10，同 `crates/aleph-cdp` 禁令）。
2. **client 侧也要纳入 Zahir。** ACP harness manager 与 MCP manager 都是 §6.2 要收敛的注册表。
3. **同一套审批语义，两个方向一个推导。** 外部 agent → `request_permission` → `incoming.rs` → 审批门；将来 Aleph 作为 ACP agent 向宿主**发出** `request_permission`。"什么需要审批、结果怎么解读"共用一处推导。

**外部能力的 replay**（v3 新增）：经 MCP / ACP client 接入的外部工具，descriptor 的 replay **一律默认 `Unsafe`**，除非协议元数据明确声明只读（如 MCP tool annotations 的 read-only 提示，`[待核实]` 具体字段）。不得因为"是外部的"就假定可重跑。

**样板候选**：现有 ACP harness 同时挂在 Tool mode、Agent mode、团队调度、Panel 四个面上——正是"一个能力、多个出口"的真实实例（以 §十四 为准）。

---

## 十三、成功判据与失败形态

**成功判据**

1. 删掉的旧结构数量 ≥ 新引入的 Zahir 组件数量；
2. 至少一个动词的全部面（工具 / RPC / 客户端 / 事件 / MCP）由同一份 descriptor 投影，且有守卫在任一面漂移时变红；
3. 至少一类泄漏（测试临时文件 / 进程级缓存 / MCP 连接 / 浏览器会话）由 Scope dispose 结构性消除，且有变异测试证明守卫会红；
4. **至少一个工具的崩溃恢复由 descriptor 的 replay 声明驱动**（v3 新增），且有"杀进程于 ① ③ 之间"的测试覆盖 Safe 与 Unsafe 两支；
5. `src/harness/` 零改动。

**失败形态（出现即停下复盘）**

- Zahir 与旧注册表并存超过一个任务（判据 §1）；
- 为"未来可能的需求"建了没有消费者的 Scope 级别、能力种类或 replay 变体（YAGNI）；
- 任何按消息内容过滤能力的代码（R7）；
- 会话中途工具集发生变化（且不满足 §10.4）；
- `dispose()` 的 `Err` 被当作"已撤销"消费（判据 §8）；
- 出现第二套 ACP 或 MCP 报文类型实现（§12.3 规则 1）；
- ACP / MCP client 侧注册表在本轮结束时仍游离于 Zahir 之外（§12.3 规则 2）；
- **recovery 里出现新的"按工具名特判是否重跑"**（v3 新增）——重跑与否只能来自 descriptor；
- **为持久性引入第二套意图 / 结果记录**（v3 新增），而不是复用会话日志与现有意图戳。

---

## 十四、第一步：清点（在写 spec 之前）

派一个子代理，在合并后的 main 上产出以下清点表（每个数字带它的谓词与 commit——判据 §18）：

1. **注册表清点**：每一个"按名字存放可调用 / 可订阅东西"的结构——位置、键类型、写者、读者、是否有撤销路径。
2. **多面动词清点**：每个动词暴露在哪几张脸上（工具 / RPC / 客户端 / 事件 / MCP / CLI），各面推导是否同一处。
3. **生命周期清点**：每一个需要清理的资源——谁创建、谁清理、清理路径是否存在、是否经 `EffectScope`。
4. **Scope 消费者清点**：Runtime / Session / Run / Task / Turn 各级有没有真实消费者；**可见性与生命周期分开列**。
5. **双向协议面现状**：出口侧（`/mcp`、`al`、JSON-RPC）、入口侧（ACP harness、MCP client）、报文类型真源、审批推导链。
6. **持久性清点**（v3 新增）：
   - 每个工具若在执行中被杀，今天重启后会发生什么（重跑 / 回报中断 / 措辞推断 / 丢失）；按"只读 / 幂等 / 外部有副作用"给出拟定 replay；
   - 现有意图戳与运行标记的全集（`ResumeAttempted`、`RunStarted`/`RunFinished`、`SubagentSpawned`/`SubagentReturned` 等）及其写读位置；
   - 提交幂等：哪些入口有幂等键，键是否跨重启；
   - 审批 / hook 决策是否跨重启保留；
   - 取消传播：哪些地方父取消会等子完成，哪些不等。

**读法**：注册表、多面动词、恢复推断点如果只有少数几个，本轮是一次**整理**；如果有十几个，才是真正的**升一级**。范围按清点结果定。

---

## 十五、对外：三层兼容（dsh / Pi 同构）

| 层 | dsh | Pi | 性质 |
|---|---|---|---|
| 1 · 标准协议 | Aleph MCP Server → dsh MCP client → `ctx.tools` | Pi MCP extension → Aleph MCP Server | 零专用代码即可工作 |
| 2 · 原生适配器 | `dsh-plugin-aleph`（session / lifecycle / permissions / events / UI） | `@aleph/pi-extension`；Pi Durable 侧为一个 `defineExtension`，可把 descriptor 的 replay 直接映射到 `defineTool({ replay })` | 深度集成 |
| 3 · Agent 级 | ACP：dsh 调用 Aleph Agent | Pi → `al` CLI（NDJSON / JSON） | Agent 互操作 / 通用兜底 |

```
宿主 → Aleph Tool        = MCP
宿主 → Aleph Agent       = ACP / Agent protocol
Aleph ↔ 宿主 internals   = Native adapter
```

⚠️ 第 3 层的 ACP 指 **Aleph 作为 ACP agent 被调用**——这一侧今天不存在（§12.3）。

**CLI 是 Universal Fallback，不是 Primary Integration**：`bash al ...` 会丢失 structured schema、tool metadata、capability discovery、permission semantics、typed result、streaming、cancellation、lifecycle，以及 replay 语义。

---

## 十六、CLI：扩展已有的 `al`，不新建

**Aleph 已有 `al` CLI。** 在它上面扩展：

```
al capabilities     列出（按 scope / principal 过滤；显示 kind 与 replay）
al exec <capability>
al sessions / al agents
al mcp / al rpc
--json / --jsonl / --stream / --session / --agent
```

具体子命令以清点出的现有 `al` 命令树为准；CLI 只做投影与 I/O（R4）。

---

## 十七、五层架构

```
Layer 5 — Host Integration      dsh Plugin · Pi Extension · CLI · Desktop UI · IDE
Layer 4 — Protocol（出口）       MCP server · ACP agent · JSON-RPC / NDJSON · HTTP / WebSocket
Layer 3 — Zahir                 Registry · Scope · Effects · Recovery · Events · Policy · Schema · Projection
Layer 2 — Agent Runtime         Agent Loop(harness) · Model Router · Context · Memory · Sub-agent · Scheduler
Layer 1 — Execution Substrate   Browser · FS · Shell · Process · Network · GUI · LLM · Storage · Sandbox
                                · ACP client（外部 agent）· MCP client（外部 server）   ← 入口侧，见 §12.3
```

同一个协议可以同时出现在 Layer 4（出口）和 Layer 1（入口）；两处共用同一份报文类型。

⚠️ **层与依赖方向要一致**：图中 Zahir 在 Agent Runtime 之上，而 §6.1 又说 Agent Loop 经 Zahir 调用能力。spec 必须画清**编译期依赖方向**（谁 `use` 谁），并与 CLAUDE.md P1 的单向依赖 `Interface → Core → Domain` 对齐。

编译期约束：Aleph Core **不依赖** dsh、Pi、Pi Durable 或任何具体 MCP 实现；它们都在 adapter 层。

---

## 十八、原则的最终表述

> **Everything externally composable is a Capability.**
> 一切需要被 Agent、Runtime 或其他系统**动态发现、调用、替换、订阅或管理**的能力，都是 Capability。

> **Every Capability says how it survives.**（v3 新增）
> 每个能力的描述里都写明：它归谁所有、怎么撤销、进程死后能不能重来。

`AgentLoop implementation` 不是 Capability；Browser、Shell、Memory、Tool、Task、Sub-agent、MCP connection 可以是。LLM provider、Browser 引擎这类只被 `implements` / `requires` 引用的东西，按 §7.2 判定问句处理。

**判定问句**：*模型或运行时之外的系统，是否需要在运行时按名字发现、调用、替换、订阅或管理它？* 否 ⇒ 实现细节，不进 Capability kind。

---

## 十九、优先级（按 Aleph 现状重排）

| 优先级 | 事项 | 说明 |
|---|---|---|
| **P0-a** | §十四 清点（含第 6 项持久性） | 决定范围；不做它，后面都是猜 |
| **P0-b** | Scope / Effect 推广 | 从 `EffectScope` 泛化到 Session / Run / Task；补"父等子"；同轮删掉被取代的清理逻辑 |
| **P0-c** | 单一 descriptor + 多面投影 | 先选一个多面动词做样板（候选：ACP harness 的四个面） |
| **P0-d** | Registry 收敛 | 按清点结果逐个合并，每合并一个删一个 |
| **P1** | **replay 声明驱动恢复** | descriptor 加 `ReplayPolicy`；`boundary_repair` / `resume_coordinator` 读它决定是否重跑；同轮删掉被取代的推断分支 |
| P1 | 提交幂等跨崩溃 | 决定幂等键落点（Gateway 落盘 or 会话日志），替换进程内 `DashMap` 的跨崩溃职责 |
| P1 | MCP 面 / `al` CLI 改为从 descriptor 投影 | 两者已存在，改为投影而非新建 |
| P1 | Native dsh Adapter · Native Pi / Pi Durable Extension | 并列方向 |
| P2 | 审批 / hook 决策 durable memo | 视 §十四 第 6 项结果，若今天已跨重启则删除此项 |
| P2 | ACP agent 侧 / Agent 级互操作 | 复用 `src/acp/protocol.rs`，审批语义与 `incoming.rs` 共用推导 |
| P3 | 更多 Host Adapters | Claude Code / Codex / IDE / Desktop / Web |

---

## 二十、总纲图

```
                         ┌──────────────────────┐
                         │        HOSTS         │
                         │ dsh │ Pi │ Codex     │
                         │ Claude │ IDE │ CLI   │
                         └──────────┬───────────┘
                                    │
                         Host / Protocol Adapters
                  （每个出口都从同一份 descriptor 投影）
                                    │
                    ┌───────────────▼───────────────┐
                    │          ALEPH ZAHIR           │
                    │  （由现有注册表收敛而成）       │
                    │ Registry · Scope · Effects     │
                    │ Recovery（replay 声明驱动）     │
                    │ Schema · Policy · Projection   │
                    └───────────────┬───────────────┘
                                    │
                    ┌───────────────▼───────────────┐
                    │         AGENT RUNTIME          │
                    │ Agent Loop（R10 锁定）          │
                    │ Session log · Context · Memory │
                    │ Sub-Agent · Task Scheduler     │
                    └───────────────┬───────────────┘
             ┌──────────────────────┼──────────────────────┐
             ▼                      ▼                      ▼
          Browser               Execution              Cognition
     Obscura / Chromium      Shell / FS / Process     LLM / Memory
     (经 aleph-cdp)           Sandbox / Network        Retrieval
```

```
dsh 不需要成为 Aleph · Pi 不需要成为 Aleph · Aleph 也不需要成为 dsh 或 Pi
```

---

## 最后：压缩成五句话

1. **Zahir 是 Aleph 的 harness**：注册一次，处处投影；所有权可撤销，效果可追溯。它由现有注册表收敛而成，不是新建内核。
2. **从 Cordis 学所有权纪律**（Scope + Reversible Effects + 依赖声明），Aleph 已在插件维度做了第一块；不引 DI 容器、Proxy context、级联重启。
3. **从 Pi 学极小核心，从 Pi Durable 学持久语义**：效果三明治、replay 声明、所有权树、跨崩溃幂等——但不照搬它的单一提交线与文档存储。
4. **dsh、Pi 是互操作对象，不是 Foundation**；Aleph 的 Foundation 是 Zahir 的单一 descriptor。
5. **Zahir 的价值在于让几类反复付账的缺陷从结构上写不出来**（两份表述、几张脸漂移、没做与做了没记上）；衡量它的是删掉了多少旧东西，不是新建了多少新东西。
