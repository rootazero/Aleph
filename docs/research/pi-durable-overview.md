---
title: Pi Durable — Agent Harness 技术档案
source: B 站视频字幕（小木头，"Pi Durable 是什么，有什么用？Pi 1.0 同日发布的 Agent Harness 框架"）
source_srt: "Pi Durable 是什么，有什么用？Pi 1.0 同日发布的 Agent Harness 框架.srt"
release_context: Pi 1.0 发布同日（10 月 1 日）
audience: Agent framework architecture reference
topic: pi-durable
tags: [pi, agent-harness, durable, framework, architecture, reference]
---

# Pi Durable：Agent Harness 架构参考

> **本文用途**：Pi Durable 的架构与技术档案——忠实于源码与官方字幕，作为下游 agent 做架构分析时的输入资料。安装与上手细节已剔除。
>
> **核心结论（剧透）**：Pi Durable 全部能力建立在**一条原子提交贯穿全局**这一前提之上——
>
> > **所有变化先通过同一条原子提交写进存储；提交完成之前不展示任何东西。**
>
> 一旦接受这个前提，crash recovery、fork、inbox/steer、多人协作、subagent、运行中替换代码都"自动"成立，无需各自单独设计。

## 阅读地图

| 想知道的事 | 跳到 |
|----------|------|
| "一条原子提交"在源码层是怎么落实的 | [§8 核心机制](#8-核心机制) |
| crash / 重启 / 中断时系统在做什么 | [§9 崩溃恢复](#9-崩溃恢复) |
| fork / inbox / 多端 / subagent / 运行时换代码如何派生自同一条原则 | [§10 派生功能](#10-派生功能) |
| 存储与执行环境怎么解耦（Tx / Storage / Tracker 三层） | [§12 存储与执行环境](#12-存储与执行环境) |
| Chord 复制状态的数据结构 / watch 协议 / 选型依据 | [§12.2 Chord：复制状态的物理原语](#122-chord复制状态的物理原语) |
| 规范层的 8 条 invariants（不可绕过的工程契约） | [§8.13 规范层的 8 条 invariants](#813-规范层的-8-条-invariants) |
| Tool / Generation 重放协议（tool 崩溃 / 流中断恢复） | [§9.7 ToolTask 两阶段重放协议](#97-tooltask-两阶段重放协议)、[§9.8 GenerationTask 的 partial reconciliation](#98-generationtask-的-partial-reconciliation) |
| Pi Durable 在 Pi 体系里的位置与代码量基线 | [§6 与 Pi 的关系](#6-与-pi-的关系)、[§7 代码体量](#7-代码体量) |
| 与其他持久化运行时（Cloudflare DO 等）对比 | [§13 与 Cloudflare Durable Objects](#13-与-cloudflare-durable-objects) |
| 想避免最常见的工程陷阱（10 条最设计相关 footguns） | [§14.6 踩坑清单](#146-踩坑清单spec-12-提炼) |

---

## 1. 引子

- **Pi 1.0 发布**：10 月 1 日，Code Mode、MCP 延迟加载工具等成为正式功能。
- **同日发布**：实验性框架 **Pi Durable**。
- **作者背书**：作者通读了 README、设计文档、示例，并拉到本地跑了两个实验（包括用 DeepSeek 驱动官方度假规划 Agent），穿插在后面的分享中。

## 2. Pi Durable 是什么

- 与 Pi 1.0 同一天发布的**实验性 npm 包**。
- 官方定位：**durable agent harness**——一个用来构建各种 Agent 应用的框架（coding agent 是其中一种）。
- 对外暴露的是一套**接口（API）**，让开发者自由拼装不同形态的 Agent 产品。

## 3. Harness 的定义

官方把 harness 定义为四件事：

1. **一份存储**：会话状态、任务进度都落盘。
2. **运行机制**：运行一段或多段大模型会话的整套调度。
3. **工具**：模型可调用的能力。
4. **执行环境**：工具实际跑在哪里。

> 一句话：Harness = 存储 + 调度 + 工具 + 执行环境。

## 4. 10 组接口（设计面）

> 这是 Pi Durable 对外的**设计面**——10 组接口即"harness 这个词在它身上具体指什么"。

| # | 接口组 | 关键能力 | 对设计意味着 |
|---|--------|----------|--------------|
| 1 | **打开和恢复** | 指定存储（如 SQLite）；会话记录与任务进度写入；重启用同存储继续 | 存储是可注入的，进程是无状态的（重启即重建） |
| 2 | **管理会话** | 创建会话；从任意条目**分叉**出新会话 | 会话是显式一等公民，不是模型上下文的副产品 |
| 3 | **提交和等待** | 提交输入、等回答、中止；忙时新消息可**排队 / 插话转向 / 拒绝** | 把"人类输入"建模成与"模型输出"同构的事件流 |
| 4 | **会话配置** | 每段会话可单独配：模型、思考强度、工具、指令、工作目录；改动作为一次原子 commit 写入 `pi.agent` rewindable 文档，可回卷到任意 checkpoint | 配置是会话级而非进程级，与 transcript 同原子，便于 fork 各自隔离 |
| 5 | **扩展 Agent 能力** | 自定义工具、system prompt 段落、钩子（工具/模型请求前后插入逻辑）；打包成**扩展（extension）**装进**注册表（registry）** | 扩展名（而非代码）存于会话，代码可运行时替换 |
| 6 | **应用自有状态** | 带类型的应用状态（文档 / document），与会话一起持久化；可定义带 checkpoint 的自定义长任务 | 把"应用状态"与会话合并到同一原子单元，fork 时一并分叉 |
| 7 | **订阅会话状态** | 订阅会话实时变更，推给终端、网页、客户端 | UI 端只是"已提交状态"的渲染器，不存任何私有状态 |
| 8 | **存储与执行环境** | 存储可选 SQLite / JSONL / 内存；自带**读/写/编辑文件、执行命令**四个工具 | 存储后端是 pluggable；执行环境是一组可替换的小接口 |
| 9 | **任务图订阅** | `Harness.taskGraph()` / `watchTaskGraph()` 返回所有任务的父子关系、依赖、状态 | subagent 关系是显式数据，可观测、可中断、可图形化 |
| 10 | **多端视图同步** | `Conversation.viewState()` 返回 `AttachedReplicatedState<ConversationView>`；每个客户端订阅一次，拿到当前视图 + 增量 | "白嫖" Chord 的复制状态——多端同步无需自定义协议 |

## 5. 典型场景（看设计要应对的边界）

1. **长时间运行**：几十分钟到几小时；进程崩溃 / 机器重启 / 容器重部署都能从断点继续。
2. **聊天平台 Agent**：Slack 主对话 + Thread 分叉，Thread 与主对话可同时运行。
3. **多人共用协作**：多界面同时连同一会话，后加入者直接看当前状态，工作中可插话。
4. **带 subagent**：主 Agent 把调研 / 审阅交给 subagent，subagent 可用更便宜的模型 + 只读工具在后台运行。
5. **带业务状态**：待办、计划、工单与会话一起保存、一起 fork，避免状态与对话对不上。
6. **后台自动化**：会话空闲时后台任务继续。
7. **官方 demo**：终端 coding agent + 度假规划 Agent，二者均使用上述接口。

## 6. 与 Pi 的关系

- **Pi 的定位**：单人在终端用的 coding agent；进程挂了人看一眼就能让它继续。**这是 Pi 1.0 专注的事，原则不变。**
- **Pi Durable 的定位**：把同样技术以各种形式带给更多人。需要的新特性：
  - 能在**任何地方**运行
  - 能从**不同界面**接入
  - 会话可以**无限长**
  - 能应对**内部和外部的严重故障**
  - 允许**多人同时操控**同一个 Agent
- **关系**：Pi Durable 不替代 Pi，而是构建各种 Agent 应用的框架。
- **共享**：代码（模型接入库 `pi-ai` 等）、原则（**极简**、**可定制**）。
- **单独成包的目的**：在不打扰 Pi 主线的前提下试验新设计，验证有价值的经验后整合回 Pi。

> **设计含义**：Pi Durable 是 Pi 的"实验田"。这种主从关系是开源项目做大之后常见的解法：核心产品稳定运行，实验在独立包演进，最后回流。

## 7. 代码体量

- Pi Durable 自身：**约 15000 行**（不算测试）。
- 用 GPT 分词：**约 15 万 token**。
- 用 Claude 分词：**约 25 万 token**。
- 设计目标：与 Pi 其他部分一样，被设计成让 **Agent 能很轻易读懂它**。
- 体量定位：在现代化模型的上下文窗口中可以"比较轻松地支持"。

> **设计含义**：~15000 行 + 让 Agent 能读懂 = 一个可由 LLM 直接消化并复制/改造的完整参考实现。这是"小代码量 + 大能力"基线。

## 8. 核心机制

> §8.1–§8.5 给出表观规则；§8.6–§8.13 把规则落到源码层——任务状态机、所有权树、Session 主线、memos、sleep 分段、retry/deferred 持久化、规范 invariant。

### 8.1 核心对象：Harness

定义：一份存储 + 在这份存储上运行 Agent 的那套机制。

### 8.2 全局规则

> **所有变化都通过同一条原子提交写进存储；提交完成之前不展示任何东西。**

是后续所有能力（崩溃恢复、分叉、多人协作、subagent、运行中替换代码……）的共同前提。

### 8.3 存储里的两类东西

1. **会话记录条目（transcript）**
   - 用户消息、模型回答、工具执行结果都属于这一类。
   - 一旦写入就**不可再改**。
2. **任务**
   - `pi.generation`：负责请求一次模型。
   - `pi.tool`：负责执行一次工具调用。
   - 任务是**持久化的状态机**，每往前走一步都先把进度存为一个 **checkpoint**。

### 8.4 一次带两个工具调用的问答流程

```
1. 用户提交一句话
2. 写入一条用户消息（transcript）
3. 启动 generation 任务，请求模型
4. 模型决定调用两个工具 → 该回答写入一条模型消息（transcript）
5. 两个工具调用各是一个 tool 任务，归属于刚才那个 generation
6. 工具执行完，结果分别写成两条 tool 结果（transcript）
7. generation 等两个工具都结束，把这一轮交给下一个 generation
8. 下一个 generation 拿到最终回答（模型消息）
```

### 8.5 界面渲染规则

- 界面上看到的一切（流式输出、工具日志、排队消息）**都是从已经提交的状态里读出来的**。
- **流式内容最多每 100 毫秒提交一次**。
- 进程突然崩溃时，流式输出最多丢失最后那 100 毫秒。

> **设计含义**：界面 = 已提交状态的纯渲染器。这是让多人协作、fork、live reload 全部"白嫖"的原子。Pi Durable 把这条推到极致——UI 里**没有任何私有状态**。

### 8.6 Task 5-state machine

源码里每个任务是持久化状态机，5 个状态：

| 状态 | 含义 |
|------|------|
| `pending` | 已创建未运行 |
| `running` | 调度器正在执行 callback |
| `waiting` | 阻塞于依赖（join、sleep、子任务、模型响应） |
| `completing` | callback 已返回，正在写最终 commit |
| `terminal` | 终态（completed / aborted / failed） |

前 4 个是"活的"——调度器只持有 `LIVE_STATUSES = ["pending","running","waiting","completing"]` 的内存索引（`#live: Map<TaskId, AnyTaskRecord>`）。Terminal 任务落出 `#live`，但仍能从存储里读回。

终端转移（spec §5.3）是 4 步原子操作：① 写终态记录；② append 结果 entries；③ retire 所有 task-scope 文档；④ resolve 该任务 settle 的 submissions。`completing` 状态的存在正是因为这 4 步可能被 owned work 拖成两段 commit——先把 owned work 等掉（hold，spec §5.5 rule 4），再走完整 4 步。

> **设计含义**：把"活"与"死"显式分开有双重收益：调度器内存预算可预测（取决于活态数量而非历史任务数）；Harness 可以安全 join / waitForTask 终态任务而不担心内存泄漏。

### 8.7 所有权树与级联中止

任务之间形成一棵树：

- 每个任务有 `owner`：要么是另一个 task（嵌套），要么是 conversation（顶层）
- 顶层任务的 owner 是 conversation；subagent 创建的子任务 owner 是父 task
- **中止语义**：中止父 → 子代自动 cascade 中止

`abort(taskId)` 的四种归宿：

| 归宿 | 触发条件 |
|------|---------|
| no-op | 已 terminal |
| 等自然结束 | owned live + 已有 invocation（设 abortRequested mark，等当前 run callback 自然结束） |
| 仅设 mark | owned live + 有 run invocation（不立即停，等 callback 走完） |
| orphaned | 既不在 `#live` 也没有任何定义能接 |

**新 owned work 必须挂在 live owner 上**（spec §5.5 规则 1）：`createTask` / `createConversation` 时若 owner 在 commit 的 final candidate 里是 `completing` / `terminal` / abort-marked，会被拒。这条把"结算那一刻禁止创建 owned work"再往前推一档——连"结算前一刻创建一个快死的 owner"也封掉。

**持有态（`completing`）存在的因果**（spec §5.5 规则 2-5）：task 在终态 commit 时若还有 live owned work，结局被压成 `{ status: "completing", outcome }`——task 自杀 ≠ 工作完成，它的 subagent / 子任务可能还在跑。Scheduler 在 later commit 写最终 `terminal`（每次 commit 后评估，含 hold 后新创的工作；open() 也重评每个 completing）。Held outcome 是终态：phase / runtime commit / abort handler 都不再跑；非 completed = abort 意图（下层 cascade + drain），completed = 等下层正常结束；waiters / inspection 看 completing 仍是 live 直到 final commit。

**中止结算从下到上**（spec §5.5）：abort handler 启动前必须等所有 owned 普通工作 drain 完（这样 handler 看到的是已确定的 outcome）。Handler 不能返回 `waiting`、不能创建 owned child——只能 inline 补偿，或创建 background conversation-owned task（cascade 触不到，靠 `runtime.waitForTask()` 持住 invocation）。后台任务默认是 boundary，除非直接 abort 或被 `Conversation.abort(context, { background: true })` 主动 abort。

**结算 commit 不能创建 owned work**（`#validateOwners`）：

一个 task 不能在自己的结算 commit 里再开 owned work（owned conversation 或 owned task）。在 `settleSuccess` 之前的 `#validateOwners` 会校验：task 引用必须存在、状态不能是 `terminal` / `completing`、不能已经被 abort 标记。这条规则避免"task 即将死时还在招兵买马"的孤儿局面——一个 task 自杀的同时不允许再分派出新工作，否则新工作没人接管。

> **设计含义**：ownership tree 把"subagent / fork / inbox 取消"统一成同一机制——避免"取消 X 还要手动取消 X 的 X"这种容易漏的代码路径。Spec §5.5 在这条基础上又加了三道关：(1) 结算那一刻禁止"边死边开新工作"（`#validateOwners`）；(2) 终结被 owned work 拖住时强制走 `completing` 态而非直接 terminal——task 自杀不等于工作完成；(3) abort handler 不能创建 owned child，只能 inline 补偿或 background——防止 cascade 错乱。三条规则一起，ownership 是真正闭环的：tree 不漏、中止有序、终结不撒谎。
>
> 注：owner 死了，conversation 还能用——可以发起 new run 去审讯已完成 subagent，那是 ordinary work，不属于任何 finish（spec §5.5）。这条容易被忽略：拥有关系只决定 abort 链，不决定 conversation 可用性。

### 8.8 Session 主线（session line）

源码 `scheduler.ts` 把不变量写成：

> Every task transition is decided and written by one callback serialized on the Session line.

所有任务状态变迁——reservation、marks、runtime commits、finalization、phase 前 precedence 检查——都在 Session 主线串行执行。Handlers、joins 在主线之外异步触发，但它们读到的状态都是已 commit 不可变快照。

每次 commit 都必经 `#gated(invocation, change, context)` 三重门：

1. 任务还 running
2. invocation 没 ended（避免 callback 跑完后再 commit）
3. 没被 abortRequested

任何一项不满足 → commit 被拒，runtime 写 commit 不可能成功。

`Invocation` 是这条主线的承载对象：`{mode: "run"|"abort", controller: AbortController, context, watches, ended, done}`。`controller.signal` 是 runtime.signal 的来源；`done` promise 是回调结束的 wait point。

> **设计含义**：把"回调执行"和"状态写入"绑在同一条主线，让调度器无需加锁即可保证 consistency。#gated 是这条主线的检查点——任何 commit 必须先经过它，否则 invariant 2（"文档更新仅在存储 commit 成功后发布"）就可能被绕过。

### 8.9 Memos：活态附属

`runtime.memo(name)` / `runtime.memo(name, value)`——读或原子写一条命名备忘：

- 仅在任务**活态**期间有效
- 一旦进入 `completing` 或 `terminal`，**所有 memos 被剥掉**（`withState` 的 strip 步骤）

> 这意味着 memos **不跨重启持久化**——重启后从头建一个新 generation 时，memo 表是空的。

**与 `LiveDoc.generation.retry` 那种"持久化到 LiveDoc 的字段"不同**——后者是 LiveDoc 的一部分，跨重启保留。memos 是纯运行时附属。

> **设计含义**：memos vs 持久化字段是两条不同的设计路径——**短命状态用 memos**（不需要 commit / 不进 transcript / 跨重启可重算），**跨重启状态进 LiveDoc**。混淆这两条路径是常见的设计错误：把每次重试的退避塞进 memos 会让重启后丢失退避，把用户上下文塞进 LiveDoc 会污染 conversation-owned document 的 fork 语义。

### 8.10 长睡眠的分段实现

源码 `MAX_TIMER_DELAY = 2_147_483_647`——约 24.8 天。超过这个时间的 sleep 由调度器自动分段。

含义：
- `runtime.sleep(until)` 即使设的是 30 天也不会被 setTimeout 上限卡住
- 各 sleep 段可被 abort 唤醒，cancel cross-restart
- 实际业务里 sleep 多用于 deferred model response polling（`pollAt = now + handle.pollAfterMs ?? 5000`，一般不超过分钟级），长 sleep 多用于"几小时后提醒"类应用层场景

> **设计含义**：调度器把 sleep 当成持久化的时间事件——`#live` 里 `waiting: sleep` 的 task 自动有 `until` 字段，重启后从存储恢复，到期由 next session 接管。这让"24.8 天上限"成为非工程性约束——调度器能 sleep 多久取决于业务想等多久。

### 8.11 durable retry backoff

模型调用失败时的退避也持久化在 LiveDoc 里：`generation.retry: { at, error }`。重启之后调度器读这个字段就知道"上次是 n 秒前 retry 失败、退避还没结束"——**退避状态跨进程重启不丢失**，不必靠内存计时器。

### 8.12 deferred polling

部分 provider 的"延迟响应"（如异步 batch）通过 `generation.deferred: { pollAt }` 持久化下次轮询时间。重启之后调度器照常继续轮询，不会丢响应。

### 8.13 规范层的 8 条 invariants

源码 `docs/spec.md` 把"一条原子提交"翻译成 8 条可验证的规则：

1. 一个 Session commit 在所有记录和文档写上**原子**
2. 文档更新仅在**存储 commit 成功**后发布
3. 所有可见进度都是持久的；**没有易变发布路径**
4. 外部 effects **不在 Session mutation 事务中**执行
5. 条目和 ID 不可变，committed 后永不重用
6. 文档 draft 在事务回调结束时**完全撤销**；值必须 strict JSON
7. mutation line **在 settlement 期间保持**；observer 仅同步捕获不可变状态
8. 不确定的存储失败 = 致命 — 必须**重开 Session**

> **设计含义**：8 条 invariant 是 §8.2 那条全局规则的"可证伪版"。把原则翻译成可被 CI / 测试套件验证的具体规则，比"我们的设计原则是 X"更难糊弄。Invariant 8 还附带修复动作（"重开 Session"），让规则不仅可证伪、可执行。

## 9. 崩溃恢复

### 9.1 effect sandwich（三层夹心）

每个任务都按同一个顺序执行：

1. **写**：把打算做什么写进存储。
2. **做**：真正去做。
3. **写**：把结果写进存储，推进到下一个 checkpoint。

新进程打开同一份存储，会找到所有还没完成或没结束的任务，从各自最后一个 checkpoint 继续。

### 9.2 难点：进程死在中间层

如果进程恰好死在"已经做了，但结果还没写"的位置，重启以后没人知道这件事到底做完没有。

### 9.3 解决：把判断权交给写工具的人

- **`replay` 属性**：定义工具时声明。
  - `replay: "safe"`：工具可安全地重新执行（读文件、搜索、查询等只读操作适合此声明）。
  - 未声明：崩溃后**不会重新执行**，模型会收到一条"工具被中断、可能已执行一部分"的错误信息，连同崩溃前已提交的输出一起，由模型决定下一步。
- **`memo` 机制**：自定义任务可用此机制记录"这件事已经做过"，重启后先查 memo，避免同一操作执行两遍（例如同一封邮件不会发两次）。

> **设计含义**：durability 不是框架替你做完的事，而是**框架 + 工具作者共同定义的事**。框架提供 sandwich 与 replay 协议，幂等性 / 重做安全性的语义由工具作者声明。这种"声明 + 重放"模型比"框架自动重做"更诚实——因为现实中大多数工具既不是纯只读也不是纯幂等，没有 ground truth 的话框架不该替你猜。

### 9.4 实验 1：模拟模型下的崩溃恢复

> Pi 自带**模拟模型**按脚本返回固定工具调用，不需要 API key，结果可复现。

**工具定义**：
- `fetch_report`：模拟读取报告，每 0.4 秒一行进度，12 步；声明 `replay: "safe"`。
- `send_email`：发邮件；**未声明 replay**。

**流程**：
1. 模型在同一轮**并行**调用这两个工具。
2. 跑到第 7 步时对进程 `kill -9`。
3. 用同一份 SQLite 文件 + 同一 `requestId` 重新打开 Harness。

**三点观察**：
1. **不重复提交**：相同 `requestId` 让 Harness 返回原编号为 7 的提交，不让 Agent 把同一件事做两遍。
2. **`fetch_report` 重放**：声明了 replay，从第 1 步重跑到第 12 步。
3. **`send_email` 不重放**：返回中断错误 + 崩溃前已提交的 7 行输出。模型固定输出："报告已拿到，邮件被中断，没有重发。"

**结论**：
- **能从断点继续 ≠ 每个操作只执行一次**。
- Pi Durable 保证**每一步都有记录**、崩溃后**都能从记录继续**。
- **一个操作能否重做，由写工具的人自行判断**。

### 9.5 调度器的 reconciliation

Harness 重新打开（`Harness.open()` 或 scheduler `open()`）时会做一次 reconciliation：

- 找出所有还停留在 `running` 状态的任务（说明上一进程死在执行中）。
- 把它改回 `pending` 状态，等待下次调度。
- 同一时间 LiveDoc 里的 generation / tool slot 也被同步收敛——已完成的 slot 收尾，partial 流被终结。

这一步是关键：**调度器不假设任务还在跑，只看持久化的状态机当前在哪个 phase**。

### 9.6 abort 后的 assistant entry

如果 generation 中途 crash，已经流式提交的部分（`LiveDoc.generation.message`）会怎么处理？

- `LiveDoc.generation.message` 是**已经 commit 进存储**的 throttled partial，不是缓存。
- crash 时调度器把这条 partial **结算为一条 `aborted` 的 assistant entry** 入 transcript。
- token 仍然计入 `pi.usage`（模型真的跑过这部分）。
- 模型下次被调度时，看到的就是"上次中断了"的明确信号，而非"从某处丢失了 100ms"。

> **设计含义**：reconciliation + aborted entry 把"crash 是常见情况"刻进调度器代码——不是靠 try/catch 兜底，而是把"上一进程死在某个 phase"作为**正常输入**来对待。

### 9.7 ToolTask 两阶段重放协议

源码 `src/harness/tool.ts` 把每个工具调用拆成两个 phase：

**call phase**（首次执行走这里）：
1. 解析 call、查 registry、跑 `prepareArguments`
2. 跑 `beforeTool` hooks
3. **commit checkpoint**：`{phase: "execute", arguments: final, replay: tool.replay ?? "unsafe"}` 到 `pi.live.tools` slot
4. 真正执行 `tool.execute(args, api, context)`

**execute phase**（**仅由恢复路径进入**——首次执行永远不直接进这里）：
1. 读 storage 里 stored args + stored replay policy
2. **当且仅当 stored 和 current 都声明 `safe` 才重放**——清空 progress，从头跑
3. 否则 → settle as `failed`，message = "Tool ... was interrupted and may have partially run"
4. **`failed` 触发 cascade abort**——该 tool call own 的 conversations 都自动中止

**三种归宿**：

| 情况 | 结局 | 效果 |
|------|------|------|
| 正常返回 | `completed` | 完成 |
| tool 抛错（非 abort） | `failed` | cascade abort owned conversations |
| tool 抛错 + `runtime.signal.aborted` | re-throw | abort handler 接走 |
| `result.isError=true` | `completed` | 完成但带 error |

> **设计含义**：ToolTask 的两 phase 把"幂等性"显式化成 stored/current 双方同意的契约——比"框架默认重做"或"框架默认不重做"更诚实。Unsafe 工具的 partial output 一定**已经 commit 进存储**（progress 是 throttled commit，不是缓存），所以模型总能从已提交的输出做决策，不需要假设"工具可能做过什么"。

### 9.8 GenerationTask 的 partial reconciliation

generation 在 streaming 时按 100ms 间隔把 partial 写进 `LiveDoc.generation.message`。如果进程死在 streaming 中：

- 重启后 scheduler 收尾 LiveDoc.generation 里的 partial
- `convertPartial(tx, live, conversationId)` 把这条 partial **结算为一条 `aborted` assistant entry** 入 transcript
- token 仍计入 `pi.usage`（模型真的跑过这部分）
- 下次 generation 被调度时看到的是"上次中断了"的明确信号，而非"从某处丢失了 100ms"

**commit 串行化保证**：
- `streamResponse` 维持 `inFlight: Promise<void> | undefined`——一次只有一个 in-flight commit
- `finally { stopped = true; clearTimeout(timer); await inFlight; }`——terminal commit 之前所有 partial 都已 flush

> **设计含义**：partial reconciliation 把"streaming crash"作为正常输入处理——不会出现"上次跑到 70% 但 transcript 找不到那条 entry"的对账问题。token 计入 usage 还有个微妙好处：成本归因不会因为 crash 而失真。

## 10. 派生功能

> "一切都先写进存储"这个前提一旦成立，原本需要单独设计的功能就变得顺理成章。

### 10.1 分叉（fork）

- **场景**：同一段对话要往两个方向继续。
- **行为**：会话可从任意条目分出新会话；新会话看到分叉点前历史，之后各自继续。
- **条目**：**不复制**。子会话的条目按 `parent.at` 链回父历史读取（`SCAN_PAGE_SIZE = 256`），相当于父条目 + 自己的新条目。
- **文档**：**必须复制**。每个 conversation scope 文档在 fork 时生成新 DocumentId，**值按文档定义里的 fork 策略决定**：
  - `initial`（LiveDoc、InboxDoc 选这个）— 分叉后从初始值开始；好处：fork 时无 in-flight 任务，新分支从干净起点起步
  - `asOf` — 取分叉那一刻的快照值
  - `current` — 沿用父会话当前的值
- **spec §3.7 硬约束**：
  - **fork 必须指向一个具体可见 entry `E`**——不是任意时间点；同一 commit 内不同文档状态需分别开 commit 才能分别 fork。
  - **Task 文档 / tasks 永不复制**——subagent 的 owned work 不随父会话分叉走（防止 orphan）；**Session 文档保持共享且不可 rewind**——`pi.live` 这类全局 LiveDoc 不参与 fork。
  - **fork commit 不能同时写父文档的 `fork: "current"`**——复制读的是 committed pre-batch 状态，源 doc 必须先 settle，否则 transaction 被拒。
- **作者实验**：官方分叉示例——在分支里改一份笔记，父会话里的那一份不受影响。

> **设计含义**：fork 的实现成本分两块被消解——**条目复用 + 文档复制**。条目复用不需要回放历史；文档复制给应用状态一个"分叉即隔离"的默认语义。

### 10.2 收件箱（inbox）

> **数据模型（spec §6）**：inbox 状态是 conversation-scope 内置 LiveDoc（kind `pi.inbox`，`fork: "initial"`——fork 起点为空），run 状态在另一个内置 LiveDoc `pi.live` 里（`run: { taskId, inputSubmissionIds }`，`run !== undefined` 即 busy）。两者职责分离：inbox 只管排队，run 只管执行归属。当轮 tool task 单独列在 `pi.live.tools`，**tool task 不持有 run**——run 始终是 generation task 的。

Agent 正在干活时收到新消息，提交策略有四种：

| 策略 | 行为 | 典型用途 |
|------|------|----------|
| **追加任务** | 排到当前轮回答之后，等这轮结束再开始 | 默认行为 |
| **插话转向（steer）** | 放在当前工具调用之后，立即加入进行中的工作（Codex 的 steer） | "先看 staging 的日志"调整方向 |
| **拒绝** | 直接报错 | Agent 忙且不适合被打断 |
| **纯写入** | 只往 transcript 写一条条目，不触发模型 | 记录"用户打开了某个文件"这类事件 |

**两条队列设置**（在 InboxDoc 元数据里）：
- `steeringMode`: `"all"` / `"one-at-a-time"`（默认后者）—— 一次转向多少条输入
- `followUpMode`: `"all"` / `"one-at-a-time"`（默认后者）—— 一次追加多少条输入

**边界触发**：boundary 在 commit 边界（`postTools` 或 `final`）按 spec §6 选择表处理提交：

| boundary | write | steer | follow-up |
|----------|-------|-------|-----------|
| `postTools` | 全部写入 | 按 mode 取第一个 / 全部 | 不选 |
| `final` | 全部写入 | 按 mode 取第一个 / 全部 | 按 mode 取第一个 / 全部 |

边界总是**先写后读**：writes 先按 ID 顺序放置，再处理 user items（同 ID 顺序）。两个特例：
- `head: "self"` 的 write（重置）把 `postTools` 升为 `final`——重置切掉了当前 run 的上下文，run 以 `unanswered: reset` 收尾。
- `final` 边界未选中任何 user item 且无 reset 时，conversation 由 `onYield` continuation 维持活跃（不进入 idle）。

**stale write 检测**：write 的 head 指向 active range 之前的 entry → settle 为 `unanswered: stale`，避免"针对已废弃上下文写入"的歧义。两个例外：
- 用户输入永远不 stale——排队的用户输入按 ID 顺序在新 run 里被选中（write 是唯一会被 stale 的 submission 类型）。
- `head: "self"` 的 write（重置）永远不 stale。

这条规则意外地为**后台 compaction 排序**提供了天然机制：compaction B 选在 tail 100 / cut 在 70 时，若 blocking compaction A 先 cut 在 150 完成，B 的 summary 自动 settle 为 stale（70 < 150）；A 切在 60 时 B 反而 placed（覆盖范围 A ⊆ B）。compaction 之间的"谁更早"无需显式协调——cut 位置就是天然的 happens-before 排序键。

> **设计含义**：四种提交模式 + 两条队列设置 + 边界触发 + stale 检测，把"用户输入"建模成了与"模型输出"同构的事件。这种粒度让交互层的设计变得有形状：默认 vs steer vs 拒绝是用户可显式选择的，而非框架替你拍板。Inbox 因此不仅是"队列"，而是 commit 边界处的**类型化输入协议**——schema、stale 判定、并发控制都要落地。

### 10.3 多人协作

- **场景**：几个人 / 几个界面同时看着同一个 Agent。
- **原理**：界面只渲染已提交状态，因此：
  - 多个客户端可连接同一会话。
  - 后加入的客户端先拿到当前完整视图，再接收后续变化，无需重放历史。
- **物理原语**：`Conversation.viewState()` 返回 `AttachedReplicatedState<ConversationView>`——Chord 风格的"复制状态对象"，订阅一次拿到当前视图 + 后续增量。
- **传输**：推给远程客户端的是每次提交对应的一小组操作，数据量小，适合 WebSocket；也可走兼容 Pi 原格式的事件流，代价是传输量更大。

> **设计含义**：多人协作的代价被压到"传输已提交事件的增量"——本质上是一个 changelog 推送，而不是 replay 或状态同步。`AttachedReplicatedState<ConversationView>` 是这套机制的物理原语：**每个客户端订阅一次，拿到当前快照 + 增量**，协议层只剩一个。

### 10.4 Subagent

- **核心里没有 subagent 概念**。
- subagent = **由工具创建并归该工具调用所有的会话**。
- **创建 API**：`ToolExecutionApi.createTask(task, input, options, ctx)`——runtime 写 commit 时新建子任务：
  - `options.ownership`：`{kind: "task", taskId: parentTaskId}`（嵌套）或 `{kind: "conversation"}`（顶层）
  - `options.background: true`——任务**不参与 cascade abort**，前台 Esc 不会中止后台任务
- **流程**：工具创建子会话 → 给它换模型 / 换工具 / 加指令 → 提交任务 → 等回答
- **子会话也是普通会话**，天然具备：崩溃恢复、费用统计、能在界面里单独查看
- **中止顺序**：归属关系决定。中止父级的工具调用，系统从下往上中止它拥有的所有工作，每一层先撤销自己已做过的操作
- **后台任务**：通过 `background: true` 显式声明；前台按 Esc 不会中止后台任务（abort cascade 不波及）

**`createCompaction` 是这套机制的样板**：
- conversation-owned compaction：`{kind: "conversation"}` → 走 Inbox admission 协议（`placeSummary` 通过 `admitSubmission({type: "write", requestId: "compaction:taskId", entry})`），非阻塞
- task-owned (blocking) compaction：`{kind: "task", taskId: owner}` → 直接 `appendEntry`，阻塞当前 generation 直到 compaction 完成

> **设计含义**：subagent 不是新概念，而是"由某个工具调用创建、归属于该调用的会话"。这种"subagent = 普通会话 + 归属指针"的设计避免了一个常见的过度抽象陷阱：subagent 拥有完整的 harness 全部能力（持久化、fork、inbox、订阅），却不需要任何额外的代码路径。归属指针还顺带解决了中止语义（自下而上、撤销优先）。`background: true` 把"前台不波及后台"的常见需求显式化——避免每个工具自己写 try/catch 吞 Esc。代价是：subagent 与父会话**不共享上下文**，所有信息都靠显式传参。

### 10.5 运行中替换代码

- **场景**：Agent 还在跑，你想改它的工具。
- **原理**：
  - 对话记录里只保存**扩展的名字**，不保存代码。
  - 代码里的注册表可在 Agent 运行时直接替换。
  - 已经开始的调用用旧代码跑；下一次调用用新代码。
- **系统提示词**：每次请求模型前，根据当前启用的扩展重新拼装；变化作为一条条目写进 transcript，**只把变化的部分发给模型**，尽量保住提示词缓存。
- **作者实验**：官方示例——替换发生时正在执行的那次调用输出 `v1`，下一次调用输出 `v2`。

> **设计含义**：把"配置 / 代码"从"会话状态"里拆出去——会话记录只记**名字**，注册表是运行时的内存对象。这让代码 / 配置变成可热替换的资源，且不破坏 in-flight 的调用。系统提示词用"增量条目"承载扩展变化，保住 prompt cache 的命中率——这是一个常见的工程取舍，值得照抄。

### 11. 实验 2：度假规划 Agent（真实模型）

> Pi 仓库里的度假规划 Agent demo，约 1300 行 TypeScript（大部分是终端界面）。

**关键设计**：
- `research` 工具在后台启动一个 subagent。
- subagent 并行做三项搜索：天气、博物馆、火车（**6 秒 / 10 秒 / 30 秒**）。
- 搜索工具声明了可重放（replay = safe）。
- 搜索结果写死，方便演示。

**主 Agent 的并发行为**：
> subagent 在后台调研时，**主 Agent 并没有停下来等**。
>
> 主 Agent 把调研交出去以后，立刻回头问用户（哪个周末出发？想要什么节奏？酒店预算是多少？）。

**崩溃与恢复**：
1. 进程被直接杀掉（这一时刻天气 + 博物馆已搜完，火车还在搜索）。
2. 用同一存储重新打开 Harness（同一会话 ID）。
3. **任务面板只剩火车搜索**：天气和博物馆不重执行，火车从最后一个 checkpoint 继续。
4. 火车搜索完成后，subagent 写报告送回主会话。
5. 主 Agent 整理成完整行程单（周六晴 24 度户外 / 周日下午阵雨博物馆 / 离开坐火车去萨尔茨堡每 30 分钟一班——最后这条正是重启后才搜完的）。

**数据核对**：
- 直接打开 SQLite 文件：子会话里**共三条搜索结果，每条只有一次**。
  - 天气和博物馆：崩溃前写入。
  - 火车：重启后写入。
- 最终行程同时用上了三项结果。

**费用与界面**：
- 这次完整运行费用约 **0.002 美元**。
- demo 的**界面代码里没有任何处理崩溃的逻辑**，只负责渲染会话当前状态。
- **恢复完全由 Harness 自己完成**。

> **设计含义**：这个 demo 本身是 §8.5 的活注脚——"UI 是已提交状态的纯渲染器"在 1300 行 demo 里实现了"零崩溃处理"：1300 行界面代码里没有任何崩溃处理逻辑，全部由 Harness 在数据层消化。

## 12. 存储与执行环境

**存储后端**：
- 三种自带存储：**内存 / SQLite / JSONL**。
- SQLite 和 JSONL 的核心实现**不依赖 Node API**，可稍作适配跑在 Bun 或 Cloudflare Durable Objects 等环境。
- 官方提供**一致性测试**和**基准测试**，方便自实现基于键值存储或 PostgreSQL 的后端。

### 12.1 三层抽象：Tx / Storage / Tracker

存储后端的"可插拔"不是一蹴而就的，它依赖三层抽象的清晰分离：

| 层 | 角色 | 关键不变量 |
|----|------|-----------|
| **Tx** | callback 里的事务对象 | 收集读写到 staging；ReadAfterWrite 保护 |
| **Storage** | 物理存储后端 | `storage.commit()` 原子化整批 |
| **Tracker** | Chord 的 in-memory observer | `adopt(seq)` swap 才是"可见" |

**Tx 层**——每次 Session commit 回调拿一个 `Transaction implements Tx`。内部 `#writes: StorageWrite[]` 收集读写；`settleSuccess()` → `assemble()` 出一个物理批次。**ReadAfterWrite**：callback 内写过之后不能再读——避免拿陈旧值参与计算（写后读只会拿到 Tx 自己 staging 的旧值，不会拿到本次 commit 的新值）。

**Storage 层**——Tx 把 writes assemble 成一个原子批次交给 storage；`storage.commit()` 成功才走 `adopt(seq)`，失败走 `discard()`。SQLite / JSONL / DO / 内存这四种后端只需要实现 storage 这一层的写入语义（commit 原子、seq 分配、adopt 回调），Tx 与 Tracker 层都不需要改。

**Tracker 层**——Chord tracker 是 in-memory 状态追踪，文档的每一个 address 都连着一个 `CommittedStateSource`。只有 `adopt(seq)` 成功才会触发 `pointer swap`，observer 才看到新值。这就是"提交完成之前不展示任何东西"的物理基础——storage 里的 commit 还没成功，UI 连陈旧帧都看不到。

### 12.2 Chord：复制状态的物理原语

Chord 是 Pi Durable 自带的"复制状态"层，位于 Tx 与 Tracker 之间。每个文档持久化为 `base + ops + checkpoint`：

- **base**——完整 JSON 快照，每次 checkpoint 重写
- **ops**——`[["set", path, value], ["delete", path]]` delta 数组
- **checkpointWhen 谓词**——决定何时把 ops 折叠进 base（如 `info.deltasSinceBase >= 99`）

#### 文档定义字段

| 字段 | 含义 |
|------|------|
| `kind` | 文档类型（**外部协议的一部分**，必须稳定） |
| `version` | schema 版本（schema 变更需迁移，**不是改名**） |
| `scope` | `session` / `conversation` / `task`（决定寿命 + fork 时是否复制） |
| `initial(seed)` | 创建时的初始值 |
| `family: true` | 同 kind 多实例，逻辑键 `(kind, conversationId, key)` |
| `history` | `"latest"` / `"rewindable"`——是否支持 `asOf(at)` 读历史 |
| `fork` | `"initial"` / `"current"` / `"asOf"`——fork 时如何复制 |

#### watch 协议

- 一次 Session commit = 一个 watch 帧
- listener 签名 `(value, context, delivery) => void`
- `delivery.kind`：`hydrate` / `update`
- `delivery.sequence`：stream 序列号（**不是 Session commit 序号**）
- **缓冲上限 100 帧**；超出后用 `[["r", newestValue]]` 替换 pending 后缀（避免内存失控）
- retired 源 publish `[["r", null]]`，订阅者要么撤销、要么 terminal 在 null

#### 文档生命周期

- 同一逻辑键可多次创建 → 每次分配新 numeric incarnation id（**永不重用**）
- 第一次 `tx.doc()` 创建实例，后续 seed 被忽略
- task-scope 文档：task 进入 `terminal` → **自动 retire**（见 §8.7）

#### 选型理由（出自 chord-delta-findings.md）

候选方案：normalized graph storage（每个对象/数组 → canonical node + numeric ID，wire 用 `["@", id]` 引用）。**放弃**：

| 指标 | Tree delta | Graph |
|------|------------|-------|
| Ready producer heap | 139.5 MiB | 430.4 MiB |
| Snapshot apply | 27 μs | 1,417 ms |
| Pipeline 最大 RSS | 852 MiB | 4,978 MiB |

Graph 在引用重赋值上快（不必遍历子节点），但 hydration / 复制 / 全量读都慢到无法接受。

最终方案：**tree-delta + weak-reference cache + spread-first clone**
- weak-cache patch：永久保留从 3,526 MiB → 203 MiB
- spread-first clone：V8 对象布局从 56 字节/point → 48 字节/point
- 残留 ~1.2 KB/容器 bookkeeping overhead（可接受）

> **设计含义**：Chord 把"复制状态"压到三个简单原语——**base / delta ops / checkpoint 谓词**。watch 帧式流推，缓冲 100 帧后用全量替换兜底，订阅者写代码只关心"value 变了"而不需要懂 delta 协议。选 tree-delta 不是基于"理论优雅"，是基于**20K strokes / 2M points 实测工作量下的内存与延迟**——这才是工程取舍的可验证依据。

**执行环境**：
- 需要读写文件、执行命令的工具通过一个**很小的执行环境接口**工作。
- Harness 可以跑在一台服务器上，工具实际在另一台机器 / 容器 / 沙箱里执行。
- 每段会话可有自己的**工作目录**。
- **构建时机**：env 是 `HarnessOptions.env(target, ctx) => ExecutionEnv | undefined` 的回调函数，**每次会话创建 / fork 时调用一次**（不在 Session 线上，可异步）。env 拿到会话的目标环境信息（如 `cwd`）并返回该会话的环境对象。

> **设计含义**：存储与执行环境都被收敛到**小接口**。这种收敛带来的好处是：Harness 本体不变，存储换 SQLite / JSONL / 内存 / 未来 PG 不影响调度代码；执行环境从本机切到容器 / 沙箱 / 远端不影响任务逻辑。env 是 per-conversation 而非全局（每段会话可独立切到不同沙箱），且异步可阻塞——这把"环境注入"和"会话生命周期"解耦得很彻底。**接口边界 = 演化边界**。

## 13. 与 Cloudflare Durable Objects

- **名字相似，但不是一回事**：
  - Pi Durable = 一个 npm 包。
  - Cloudflare Durable Objects = Cloudflare 的运行时。
- **可以组合使用**：Cloudflare Agents SDK 在 10 月 2 日发布了 **PiHarness 测试版**，把 Pi Durable 跑进 Durable Objects。
- **工作方式**：对象被回收后，由定时唤醒机制重新拉起，从 checkpoint 继续。

> **设计含义**：durability 的"持续性"在 Pi Durable 里由显式存储（SQLite / JSONL）实现，在 Durable Objects 里由平台隐式实现（定时唤醒 + 对象持久）。两种实现殊途同归——任何"状态先于执行"的系统都会自然得到 §10 的全部能力。**前提始终是同一条**。

## 14. 总结

### 14.1 一句话核心

> **Agent 状态全部放在存储里，进程只负责从存储里读出进度、接着往下执行。**
>
> 进程没了，换一个进程打开同一份存储就能继续跑。

### 14.2 共性前提

崩溃恢复、分叉、多人协作、subagent、运行中替换代码……看起来是不同功能，其实都来自同一个前提：

> **每一步先写进存储，再执行，再展示。**

### 14.3 适合参考的场景

- 需要**长时间运行**的 Agent。
- 需要**多人一起操控**的 Agent。
- 需要在 runtime 调整工具 / 配置 / prompt 而不打断 in-flight 任务的场景。
- 想验证"小代码量 + 大能力"基线是否成立。

### 14.4 不适合参考之处

- 如果项目只需要短会话、单用户、单进程的简单 agent，Pi Durable 的持久化层是**过度设计**——需要存储后端、需要一致性测试 replay 协议，带来的复杂度远大于收益。
- `replay` 协议要求每个工具作者做声明，工程门槛明显高于"调一下 API 就完事"的轻量级 agent 框架。

### 14.5 Pi Durable 的设计取舍一览

Pi Durable 的全部设计取舍可以分三层——正面机制、边界工程、代价局限。前面章节已经讲过每条的机制细节，本节把它们聚成一览。

**正面：一条原则派生的全部能力**：

1. **"先写存储、再执行、再展示"作为顶层原则**——fork、多端、live reload、subagent 都从这条派生。
2. **UI 是已提交状态的纯渲染器**——多端同步最便宜的实现路径，无需任何多端协调代码。
3. **subagent = 普通会话 + 归属指针**——避免专门造一套 subagent 类型系统，所有 harness 能力自动具备。
4. **document 与 transcript 在同一次原子提交里修改**——消除 fork 时的状态合并复杂度。
5. **扩展名（而非代码）存于会话**——注册表是运行时的内存对象，获得代码热替换能力。
6. **replay 协议 + memo 由工具作者声明**——诚实地把幂等性语义交给工具作者，不替他们猜。
7. **增量条目承载 system prompt 扩展变化**——保住 prompt cache 命中率。
8. **durable retry backoff + deferred polling 持久化进 LiveDoc**——跨重启的计时状态**也是状态**。
9. **8 条 invariants 把原则翻译成可验证规则**——比"我们信仰 X"更难糊弄，且 invariant 8 自带修复动作（"重开 Session"），让规则不仅可证伪、可执行。
10. **`#validateOwners` 把 orphan 路径堵死**——一个 task 不能在自己的结算 commit 里再开 owned work（owned conversation 或 owned task）。这条规则把 ownership 树从"概念"升级为"结算时也闭环"——避免 task 边死边开新工作的孤儿局面；否则上层 abort cascade 设计得再好，被开出来的 owned work 也会变成无人接管的孤儿。

**边界：工程化取舍**：

- **fork 时文档必须复制，但策略由文档作者声明**（`initial` / `asOf` / `current` 三选一）。LiveDoc/InboxDoc 选 `initial` 是有意为之——fork 时无 in-flight。fork 策略不应写死在 fork 操作里，应作为文档元数据让应用层自治。
- **流式 100ms 提交节流**——工程经验值，影响 partial 持久化粒度与重放代价。
- **存储后端小接口 + 一致性测试套件**——自实现 PG / KV 等后端的工程化路径，官方提供 reference 实现（SQLite / JSONL / 内存）。
- **env 是 per-conversation 异步 builder**（`HarnessOptions.env(target, ctx)`）——每段会话可独立切沙箱，且环境注入与 Session 主线解耦。

**代价：什么场景不适合 Pi Durable**：

- **默认沙盒执行 = 本机 shell**。自带的"读/写/编辑文件 + 执行命令"工具面向本机执行；需要更严格沙盒（容器 / WASM / eBPF）时应当重做工具层。
- **replay 协议要求工具作者声明**——工程门槛高于"调一下 API 就完事"的轻量级 agent 框架。
- **持久化层对短会话 / 单用户 / 单进程是过度设计**——需要存储后端、需要一致性测试，复杂度远大于收益。

### 14.6 踩坑清单（spec §12 提炼）

spec §12 列了 36 条 footgun——**不是善意提醒，是工程契约**。以下 10 条是最设计相关的，移植 Pi Durable 时要逐条核对：

- **Tx 内写过不能再读 table**——`#read` 在 `#hasTableWrite=true` 后抛 ReadAfterWrite。文档 draft 不受此限。
- **async commit 持有 Session 主线**——回调里不能 await 模型、工具、进程、网络、人类、嵌套 commit、session waiter。要等就用 Tx 方法或回调外。
- **abort handler 不能建 owned work**——abort handler 跑在被 abort-marked 的 task 里，`#validateOwners` 拒绝。补偿要在执行 effect 的那一层做：abort handler、内联、或 background conversation-owned 任务。
- **owned work 持有 owner**——task 有 owned live ordinary work → 保持 completing，要等那些 work 结束才进 terminal。前台 Esc 终止不了 background 之外的 owned work。
- **subagent 用量不要重复计**——owned 子会话的 pi.usage 已被 harness 计费；tool result 里**不要**再 report，否则 subtree sum 双倍。
- **compaction 期间的写会丢**——摘要反映 compaction 选 cut 的时刻。summarization 期间写的 edit 会丢；head 也可能被后续更窄的 cut 抹掉。必须在 compaction 前放置这种 write。
- **每次 compaction 都花钱**——manual / background compaction 都付一次 summarization request。哪怕结果 stale（被后续更窄的 cut 抢走）也照付。
- **schema 改不了名**——`kind` 和可见 mount path 是公开协议的一部分。schema 升级必须迁移，**不是改名**。同 kind 重复定义是 caller misuse；Session 不维护 registry 去查。
- **storage 不确定失败 → Session 毒化**——storage commit 后 ack 前的失败 = 不可恢复。**不要 catch 继续用**，必须 close + reopen。
- **buffered watch 上限 100 帧**——慢订阅者保留最多 100 个 exact frame，超出后未投递后缀被一个全量替换 `[["r", newestValue]]` 替换。需要审计每次 transition 的，把每条事实落到不可变 entry 里单独查询。

> **设计含义**：spec §12 不是"善意提醒"列表——是 Pi Durable 写代码时**强约束**的工程契约。每条都对应一个不可绕过的不变量，违反立即报错或数据腐败。借鉴 Pi Durable 时把这十条当作**移植清单**：哪些契约可以弱化（比如 short-lived 项目可以放开 ReadAfterWrite），哪些必须保留（比如 Session line 不能阻塞 IO、storage 不确定失败 → 毒化）。