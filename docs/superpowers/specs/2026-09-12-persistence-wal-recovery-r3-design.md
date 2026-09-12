# 持久化与崩溃归约 Round 3 — 原子批 · 意图戳 · 停车事实 · 日志自描述 · 恢复形状 · 运行时墓碑 (Persistence & Crash-Reduction Round 3)

> **日期** 2026-09-12 · **分支** `worktree-persistence-r3`（base `5e85060b8` = main = origin/main）· **worktree** `D:\Workspace\Aleph\.claude\worktrees\persistence-r3`
> **对标** pi (`T:\Github\pi` @ `71dca871b`) · codex (`T:\Github\codex`) · deepseek-harness (`T:\Github\deepseek-harness`) · Claude Code 真实崩溃恢复会话（`C:\Users\zou\Desktop\失败恢复.txt`）
> **扫描产物**（全文，四份）：[`2026-09-12-persistence-r3-scans/`](2026-09-12-persistence-r3-scans/) — `scan-aleph.md`（Aleph 普查：34 行台账 + 15 缺陷 + 断线 + 可复用清单）· `scan-pi.md` · `scan-codex.md` · `scan-dsh.md`
> **前序** [2026-09-02 crash-recovery-r2](2026-09-02-crash-recovery-r2-design.md) · [2026-07-04 session-lifecycle-contract](2026-07-04-session-lifecycle-contract-design.md) · [2026-07-04 session-ssot-foundation-p1](2026-07-04-session-ssot-foundation-p1-design.md)

管线：**持久化事实 → 派生运行状态 → 派生模型上下文 → 派生 UI → 崩溃后重新归约**。本轮的立场只有一句：**任何跨越副作用的事实先落盘再动手，任何多步的持久操作要么全在要么全不在，任何"我不知道"都不许被读成"没有"。**

---

## 0. 本轮范围与阶段

| 阶段 | 组 | 内容 | 状态 |
|---|---|---|---|
| **Phase 1（本轮实施）** | A | 原子多步 + 意图戳（缺陷 #2 #3 #4 #6 #10 #11） | 本 spec §4–§5 |
| | B | 停车 / hook 拒绝 / 作用域成为事实（#7 #8 #9） | §5.4 §6 |
| | C | 日志自描述 + 恢复执行形状（#1 #5 #14） | §7 §8 |
| | D | 运行时资源墓碑（#12 #13） | §9 |
| **Phase 2（本 spec 定稿，A–D 落地后另开 plan）** | E | 子 agent 运行标记统一 | §10.1 |
| | F | 斜杠命令 / 旋钮 / hook 成为事件（含 #15，推翻 r2 A3） | §10.2 |
| | G | 自动压缩持久化 | §10.3 |

---

## 1. 问题：崩溃后静默错的 15 处（Aleph 普查 B 表）

普查方法与 34 行「持久事实台账」（哪里存、副作用前还是后、boot 由谁归约、`kill -9` 丢什么）见 `scan-aleph.md` §A。下面只留形状与锚点，全部在 main `5e85060b8` 逐行读过。

| # | 缺陷 | 一句话场景 | 锚点 | 归组 |
|---|---|---|---|---|
| 1 | 两条 boot 臂两个判决 | `orphan_notice` 直写 `messages`「请重发」，随后 `ResumeCoordinator` 又把同一 run 续上；那行绕过 SSOT，模型看不见 | `src/gateway/orphan_notice.rs:61` · `agent_init/mod.rs:1347` | C |
| 2 | crash-loop 棘轮在动作的错误一侧 | 计数 = 被重触发 run 自己的 `RunStarted`；在 admit / hook / seed / 记忆召回阶段崩 ⇒ 计数不动 ⇒ 无界重触发 | `resume_coordinator.rs:989,1199` · `runner_impl.rs:978` | A |
| 3 | 手动 `/compact` 撕裂 | 3 写：`SystemMessage` → `CompactionPerformed` → `retire_through`；中途崩 ⇒ 摘要 + 全量历史永久同在 prompt，无人补完 | `src/context/compact/manual.rs:455-497` | A |
| 4 | session-split 撕裂 | 7 写，epoch **先**登记；父 `RunFinished` 未落 ⇒ 父被重触发且尾巴复制，子无 open run | `session_split.rs:50-135` | A |
| 5 | 单行解码失败全局爆炸 | 一条读不出的事件行 ⇒ `load_run_markers` 整体失败 ⇒ **所有**会话跳过 resume，只有一行 warn；无事件 schema 版本 | `src/session/store.rs:396,569` · `resume_coordinator.rs:686-692` | C |
| 6 | seed → `RunStarted` 窗口 | `UserMessage` 落了、`RunStarted` 没落 ⇒ reducer 读 `Clean`，永不恢复；只有 resilience `tasks` 行知道 | `runner_impl.rs:342 → 978` | A |
| 7 | 停在门前 = 「可能已执行」 | 审批 / `ask_user` 停车的调用与在飞调用在日志里同形；修复文本断言副作用可能已落地；`ExecApprovalManager` / `ClarificationManager` 纯内存 | `boundary_repair.rs:97-121` · `exec/manager.rs:300` · `clarification/session.rs:256` | B |
| 8 | 恢复放宽作用域 | `/<skill>` 的 `allowed_tools` 与 `/btw` 只读戳只在请求 metadata，不在 `RunEnvelopeSnapshot` ⇒ 恢复时全工具面、真实档位 | `resume_coordinator.rs:501-537` · `slash_skill_scope.rs` · `btw/mod.rs` | B |
| 9 | hook 拒绝抹掉整个 turn | `BeforeAgentStart` 拒绝在 seed 之前返回 ⇒ 用户消息与拒绝文本都不在日志，重载像什么都没发生 | `run_loop/mod.rs:484-520` | B |
| 10 | 快路径先效果后落盘 | L0 fast path 先执行工具再写 `UserMessage`+`AssistantMessage`，无 `ToolCallRequested`；`select_model` 已 pin、transcript 无痕 | `execution_engine/fast_path.rs:43-80` | A |
| 11 | 会话花费计数搭 post-run 事件 | `AssistantRunMeta` 前崩 ⇒ 会话总额永久少计；ledger 与会话行分叉 | `execute.rs:966-999` | A |
| 12 | PTY 无墓碑 | `PtyManager` 是 `LazyLock` 内存表；重启后 `pty.*` 答「不存在」，OS 子进程孤儿 | `src/gateway/pty/manager.rs:1-12,218` | D |
| 13 | bash 后台无 pid | journal 有但不记 pid ⇒ 只能说「liveness unknown」 | `process_journal.rs:34-48,415` | D |
| 14 | 串行 resume | boot 逐个 `await` 整个 run；一个长 run 挡住其余会话恢复与 busy-queue 重注入 | `start/mod.rs:3008-3090` · `execute.rs:885` | C |
| 15 | `AgentEnd` hook 改写 ≠ 日志 | `updated_output` 在 `AssistantMessage` 已持久之后改写交付文本 | `run_loop/mod.rs:649-657` | **F（Phase 2）** |

**断线 / 死码 / 撒谎注释**（普查 C 表）：`AgentInstance::{add_message*, reset_session}` 零生产调用者 · `CompactionPerformed` 一个生产者零恢复消费者 · `start/mod.rs:391`「split degrades to FinalReply」（实为 compact-to-fit）· `helpers.rs:327-334`「legacy `messages` remains authoritative」（它是投影）· `projection_reconciler.rs:29-31`「cron/heartbeat 不发 run marker」（经 bridge 无条件发）· `service.rs:67-80` 硬编码「nine sites」· 三套互不指认的 run id（A5，记录不改）。

---

## 2. 参考项目对照（摘要；全文见四份扫描产物）

| 维度 | codex | pi | deepseek-harness | Aleph 现状 → 本轮 |
|---|---|---|---|---|
| 日志载体 / 耐久 | JSONL，`flush()` 只到页缓存无 fsync；SQLite 严格滞后投影 | v3 出货无 fsync；v4 registers+txn 仍无 fsync | JSONL + zstd 帧校验，逐批 fsync，失败 `ftruncate` 回滚 | SQLite WAL + `synchronous=NORMAL`，`emit_event` await INSERT ✅ → **屏障式 FULL**（四种事件） |
| 多步持久操作 | `Compacted` 一次 append + 锁 | v4 一行 = 一事务；操作可恢复 | `start→end` 括号 + 孤儿阻断 | 3 写 / 7 写无原子 ❌ → **SQLite 单事务 `append_batch`**（三家结构上做不到） |
| 悬空调用了结 | 读侧合成 `aborted`（UUIDv5） | provider 适配层补 | `TOOL_NOT_STARTED` / `TOOL_OUTCOME_UNKNOWN` 落盘 | 三臂 + 归属 ✅ → **第四臂「停在门前，从未执行」** |
| 日志矛盾 / 损坏 | 无 | v4 拒开 | `Corruption ≠ Unsupported` | 9 变体闭集 ✅；单行失败全局 ❌ → **逐行隔离 + 第三 REJECT** |
| schema 版本 | `serde(default)` | header `v:4` | `SESSION_FORMAT_VERSION` + `ignorable` | 无 ❌ → **信封 `v` + `ignorable`，只加不改** |
| 旋钮回放 | `TurnContext` | `lane.config` | config-as-events | `RunStarted.envelope` ✅ → **+ `allowed_tools` / `btw`**；旋钮成事件留 F |
| 审批 / 澄清停车 | 不持久 | 不持久 | `approval/asked→decided` 落盘但 reducer 不看 | 内存 ❌ → **`ToolCallParked` + reducer 消费**（超越三家） |
| 斜杠命令 | 只记效果 | 不记 | `command/run→done` | fast-path 先效果 ❌ → **与普通 run 同形**；命令事件留 F |
| hook | 不持久 | 只在消费事务 | `hook/invoked→result` | 拒绝抹 turn ❌ → **`Error{HookStop}` 回执**；hook 事件留 F |
| 子 agent | SQLite `thread_spawn_edges` Open/Closed | 无 | 子 = 独立 Session + descriptor | sidecar + 合并 ✅；子无 run marker → **留 E** |
| 压缩 → 上下文 | `replacement_history` 物化 | `retainedTail` | surface `replace` | 手动可持久、自动只在内存 → **手动进事务；自动留 G** |
| UI 投影 | warn-and-continue | 快照 + fold + `rebase` | log leads, cache follows | 单写者 + reconciler ✅；`orphan_notice` 第二写者 ❌ → **CUT** |
| 后台进程 / PTY | 不持久 | 不持久 | 不持久 | bash journal 无 pid、PTY 无墓碑 ❌ → **同一 journal，两句墓碑** |
| 恢复触发 | daemon recovery file | `open[]` 交 app | resume 先 `open(write)` 排他 | 三面 + 互斥 + 上限 ✅；串行 + 棘轮错位 ❌ → **`JoinSet` 有界并行 + `ResumeAttempted`** |
| 崩溃注入测试 | 无 | 无 | 真 SIGKILL e2e | 5 Node 阶段 ✅ → **+6 阶段，失效点用 hook / 审批门撑开** |

**移植的是形状不是代码**：dsh 的「意图先于决定」与 `ignorable`；pi v4 的 reserved-id 思路只取其精神（seq 由 actor 串行分配，批内 seq 连续即是 reserved）；codex 的「一个策略函数决定什么是事实」（→ `durability_of`）。Aleph 的 Rust/SQLite 优势：事务与 `JoinSet`。

---

## 3. 裁定（本轮由用户在线做出）

| # | 裁定 | 出处 |
|---|---|---|
| U1 | 本轮实施 A/B/C/D；E/F/G 写进本 spec 作 Phase 2，A–D 做完再开 plan | 范围问答 |
| U2 | 方案 1「SQLite 单事务批追加 + 意图戳」；否决方案 2（dsh 式括号 + boot redo）与方案 3（pi v4 操作寄存器，违 R10） | 方案问答 |
| U3 | 耐久等级 = **屏障式 FULL**：只有 `ToolCallRequested` / `RunStarted` / `ResumeAttempted` / `UserMessage` 四种事件在 commit 时 fsync WAL，其余 `NORMAL`。`ToolCallParked` 是 Normal（丢了退回「结果未知」是保守方向） | 耐久问答 + §3 修正 |
| U4 | 停车的调用恢复时**只告诉模型**（第四臂文本），不重投递审批 / 澄清；重投递是 Phase 2 候选 | 停车问答 |
| U5 | 孤儿进程**记 pid + 墓碑 + 报存活，不杀** | 孤儿问答 |
| U6 | #15 从 A 组移到 F：诚实修法是 hook 结果成为事件，Phase 1 的任何补丁都是第二份表述 | §2 展示时确认 |
| U7 | 不合并 main：worktree 分支上提交并报告 | 用户协议 |

**沿用的 r2 裁定**（未被本轮推翻）：A1（2 REJECT + 7 REPORT，本轮 +1 REJECT）· A2（marker 位置配对）· A3（快照 > 会话 > 全局；**F 将推翻**）· A5（不 join run id）· A6（不落盘投影水位）· A7（子 agent 只报事实不重派）· A8（不做 Resume 按钮）· A11（QA 夹具用 Node）。

---

## 4. §1 · 原子批追加 + 屏障式耐久

### 4.1 一个写路径

- `SessionEventStore::append` 收成 **`append_batch(session_id, Vec<SessionEvent>, retire: Option<Retire>) -> Result<Vec<EventRecord>>`**；单事件 = 长度 1 的批。`Retire::{Through(seq), From(seq)}` 的 UPDATE 与 N 条 INSERT 同在一个 `BEGIN IMMEDIATE`（`rusqlite::Transaction`——没有 `commit` 就回滚，编译期钉住）。
- seq 仍由 `SessionActor` 串行分配：新增 `EmitBatch` 臂与 `EmitEvent` 同一队列；批内 seq 连续（`head+1..=head+N`）。观察者（projector）与 broadcaster 在 **commit 之后**按序逐条触发；`replay()` 仍不触发——P1 的单写者 / 重放幂等两条不变量原样保留。
- `SessionService::emit_batch` 是唯一公开面，`emit_event` 变成它的单元素包装。FTS 镜像仍 best-effort，在事务外。
- 现有 `(session_id, seq)` UNIQUE 冲突自愈重试一次（`actor.rs:166-180`）对整批生效。

### 4.2 耐久策略住在店里

- `durability_of(&SessionEvent) -> Durability::{Normal, Barrier}` **一个函数**（codex「一个策略函数决定什么是事实」）。Barrier 集合 = `ToolCallRequested` / `RunStarted` / `ResumeAttempted` / `UserMessage`（U3）。批里任一事件是 Barrier ⇒ 整批 Barrier。
- Barrier 的实现：**同一连接上事务前后切 `PRAGMA synchronous=FULL` / `NORMAL`**（per-connection 设置；WAL 模式下 FULL = commit 时 fsync WAL）。实现 agent 先核实事件店连接是否被 `Mutex` 独占——是则切换安全；否则 Barrier 事务要在持锁期间完成。
- harness **零改动**——它调的还是 `emit_event`。
- 同一张策略表的第二列是 `ignorable`（§7.3）：本轮没有任何事件声明它，但列在。

### 4.3 收成一笔的调用点

| 操作 | 现在 | 之后 |
|---|---|---|
| 手动 `/compact`（`manual.rs:455-497`） | 3 写 | 1 批 `[SystemMessage(summary), CompactionPerformed] + Retire::Through(to_seq)`；watchdog 通告在 commit 后 |
| `/undo`（`handlers/chat.rs:815-831` + `marker_balance.rs:47`） | `retire_from` + 追加 `RunFinished{Abandoned}` | 1 批 `[RunFinished{Abandoned}] + Retire::From(seq)`；`balance_run_markers_after_retire` 变成这一批的**构造函数**，不再是第二步 |
| session-split（`session_split.rs:50-135`） | 7 写，epoch 先登记 | 6 条事件 1 批；**epoch 登记移到批之后**（4.4） |
| fast-path 后半（`fast_path.rs`） | 效果后 2 行 | 效果后 1 批 `[ToolResult, AssistantMessage, RunFinished]`（前半见 5.3） |
| hook 拒绝（5.4） | 无 | 1 批 5 条 |

### 4.4 split 的 epoch：唯一留下的两步

`register_epoch` 写在 SessionStore 的 `sessions` 表（`sqlite_backend/mod.rs:665`），与事件日志同一 `sessions.db` 但**不同连接**——跨连接进不了同一事务。顺序改为「先批、后 epoch」，撕裂形状从「路由指向子、父无 RunFinished」变成「日志说已分裂、路由还指向父」；boot 加一条**幂等 heal**：`ProjectionReconciler` 活动窗口候选里，最新 `SessionForked` 的子 id ≠ epoch 表 ⇒ 补登记（log-leads-cache-follows，不是 redo）。**实现 agent 第一件事核实两张表是否真的不同连接——若同一连接则直接进事务，heal 臂不建。** 文件后端 `register_epoch` 返回 `None`，heal 臂对它 no-op 并计数。

### 4.5 这个设计让什么变难

批的内容必须先在内存里凑齐——压缩摘要的 LLM 调用在事务**外**，摘要落库前不算数（本来就该如此）。大批（split 尾巴复制）让 `EmitBatch` 持有 actor 队列更久——split 是每小时个位数的操作。Barrier 每轮多 3–5 次 fsync（各 ~1–10 ms）。

---

## 5. §2 · 意图戳与窗口归约

### 5.1 `ResumeAttempted`（#2）

- 新 marker 事件 `ResumeAttempted { target: RunRef, attempt: u32 }`（`RunRef` = 目标 `RunStarted` 的 seq，或 `Unanswered` 时那条 `UserMessage` 的 seq），由 `ResumeCoordinator` 在重触发**之前**写（Barrier）。
- `reduce_run` 的尝试次数 = 上一个 `RunFinished` 之后的 `ResumeAttempted` 条数；`trailing_starts` **删除**。`max_attempts=3` 第一次真的成立。
- 不加 wire 字段：`abandoned` 收据已是它的出口。`load_run_markers` 把它当 marker 返回。委托臂（cron / heartbeat / team，只关 marker 不重触发）不写它。
- `LogContradiction`：`ResumeAttempted` 出现在无 open run 且无 `Unanswered` 之处 ⇒ `ResumeWithoutTarget`（REPORT，读法：忽略）。

### 5.2 `Unanswered`（#6）

- **不挪 `RunStarted` 到 seed 前**——那会让首条 `UserMessage` 落在 run 内、被 `count_pending_steering` 读成一次 steer（r2 已把边界定在 `max(last AssistantMessage, last RunStarted)`）。
- `reduce_disposition` 加第四种判决 **`Unanswered { user_seq }`**：上一个 `RunFinished`（或日志起点）之后有 `UserMessage`、其后既无 `RunStarted` 也无 `AssistantMessage`。
- `resume_from_markers` 对它与 `Interrupted` 同路：无悬空调用故不修边界，直接 `ResumeAttempted` + 重触发（`FlowInput::Resume` 本就不重 seed）。recency 取那条 `UserMessage` 的时刻。
- **前提由 census 钉成等式**：`UserMessage` 在 `[RunStarted, RunFinished]` 之外的生产者只有 `seed_session` 一处（steer 只注入运行中会话；lane-deferred 消息不在日志）。
- 四张脸：`LastRunDisposition` 加 `Unanswered` 变体（`shared/protocol`），Panel / TUI 的 `last_run_notice` 文案「上一条消息没有得到回答」；`RunBadge::label` 共用判据函数。

### 5.3 fast-path 与普通 run 同形（#10）

先批 `[TurnStarted, UserMessage, RunStarted{envelope}, ToolCallRequested]`（Barrier；与 `seed_session` 同形）→ 执行工具 → 批 `[ToolResult, AssistantMessage, RunFinished]`。中途崩 ⇒ `Interrupted` + 悬空 `select_model` ⇒ 既有三臂修复对模型说「结果未知」⇒ 恢复的 run 走正常 harness，模型看到 `/model x` 与修复文本自己决定（R8：它有 `select_model` 工具）。不为 fast-path 造第二种 run 形状。census 断言「每条 L0 路径先写 `ToolCallRequested` 再执行」（从派发点派生，不列举）。

### 5.4 hook 拒绝写回执（#9）

`BeforeAgentStart` 拒绝时一批 `[TurnStarted, UserMessage, RunStarted, Error{kind: HookStop, text}, RunFinished{Cancelled}]`——`ErrorKind` 从只有 `Guardrail` 加 `HookStop`，复用 `SessionEvent::Error` 的回执路径与投影臂。日志说「run 发生了、hook 停了它」，reducer 读 `Clean`，不会被 5.2 误判成 `Unanswered` 然后重触发三次。hook **允许**时流程不变；hook **执行中**崩溃仍丢用户消息——由 §8.2(b) 的 `tasks` 行臂报「消息在落盘前丢失」，不在这里挪 seed。

### 5.5 花费从事实折叠（#11）

`AssistantMessage.usage` 已逐 Think 落盘；会话花费总额改为对它的折叠 **`session_usage_totals(log)`** 一处派生。`AssistantRunMeta` 只保留折叠给不出的东西（run_id join、context window、occupancy），**花费字段删除**，读者逐个改指折叠（先数读者——判据 #6）。`ProjectionReconciler` 对缺 meta 的 run 用同一折叠合成 gauge 戳。`SpendLedger` 不动（它是逐调用美元账，另一个事实）。

### 5.6 守卫

`Unanswered` 进合法轨迹前缀测试（每个前缀都 `Ok`）；proptest `attempts(log) == count(ResumeAttempted since last RunFinished)`；fast-path census 等式；变异：`ResumeAttempted` 写点挪到重触发之后 ⇒ `ratchet` 阶段（§11）变红。

---

## 6. §3 · 停车成为事实 + 恢复不放宽作用域

### 6.1 `ToolCallParked`（#7）

- 新事件 **`ToolCallParked { call_id, reason: Approval | Clarification | PreHook }`**，在**停车之前**写（Normal 耐久，U3），写点 = 今天写 `ToolCallApproved/Denied` 的那道闸（`scoped/dispatch.rs:1160-1200`）与 `ask_user` 的入队点。
- `reduce_run` 单次升序扫描里 `Parked` 挂到最近前驱的同 `call_id` `Requested`；`DanglingCall` 增 `parked: Option<ParkReason>`。
- `boundary_repair` 加**第四臂**（与三臂共享同一条尾巴）：「这次调用**没有执行**：重启时它在等 {操作员审批 | 你上一个问题的回答 | pre-tool hook}；仍需要就重新发起，它会再过一次门。」守卫断语义：含否定句「没有执行」+ 含工具名 + 含原因词。
- `LogContradiction` 加 `ParkedWithoutRequest`（REPORT，读法：忽略）。
- `ExecApprovalManager` / `ClarificationManager` 仍是内存（U4）。

### 6.2 闸上的静默跳过

无 ambient `CallIdentity` 时 `Approved/Denied` 静默不写（`dispatch.rs:1173-1180`）是「报成功的 no-op」。改成：审批门内**必须**有身份——census 从派发点派生「每条经过审批门的路径都携带 `CallIdentity`」（等式）；确有合法的无身份路径（内部 cron 工具调用之类），它不得经过审批门。

### 6.3 envelope 扩两键（#8）

`RunEnvelopeSnapshot` 加 `allowed_tools: Option<Vec<String>>`（`slash_skill_scope`）与 `btw: Option<BtwStamp>`；两者 `#[serde(default)]`；`RUN_ENVELOPE_KNOB_KEYS` census 同步；`plan_resume` 经既有 `resume_metadata` 回放。它们是**逐 run 事实**不是会话旋钮：只有快照一根 rung，无会话 / 全局回退；`exec_tier` 只收紧的规则不变。一个 `/btw` 的中断 run 恢复后仍是只读。

### 6.4 面（判据 #17）

`shared/protocol::DanglingCallView` 加 `parked: Option<String>`；Panel `chat_sidebar.rs::last_run_notice` 与 TUI `commands.rs::last_run_notice` 从「N 次结果未知」变成「N 次结果未知、M 次从未执行（等审批 / 等回答）」——Panel 走 `locales/{en,zh}.json`，TUI 在函数里。`ResumeReceipt` **不加词**。

---

## 7. §4 · 日志自描述 + 单行失败隔离（#5）

### 7.1 隔离在行

行解码从「整批 `Vec<SessionEvent>` 要么全成要么全败」改成逐行 `Result<SessionEvent, UndecodableRecord { seq, kind_tag, error }>`。`load_run_markers` 按会话分组后，只有**含坏行的那个会话**进 `LogContradiction::UndecodableRecord{seq}`——第三个 **REJECT**，`tag()` = `session-log-undecodable-record`，收据仍是既有 `log_inconsistent`（不加词）；其余会话照常。`ProjectionReconciler` 候选集与 `core/session-log` 用同一解码器。

### 7.2 模型上下文侧 fail-closed 不静默跳

`get_events` 遇坏行对该会话返回 `Err`，run 以一句明确拒绝结束（「seq N 有无法读取的记录，运行 `aleph-server doctor`」）——跳过一条事实等于对模型撒谎。**谁能打开这个闸、从哪里**（判据 #14）：`/undo` 到 seq 之前（`retire_from` 不需要解码），或 doctor `--fix` 对**单条**记录 `retire`——与 r2「矛盾 report-only 不可修」不冲突：矛盾是「信哪条」的人类判断，坏行是机械事实，且 retire 是软的。

### 7.3 信封两个键，变体不动

行 JSON 顶层加 `v: u16`（写它的 schema 号，只作诊断措辞）与 `ignorable: bool`（默认省略）。已知变体上 serde 忽略未知键；未知变体解码失败时解码器回头看原始 JSON 的 `ignorable`：真 ⇒ `Skipped{kind_tag}`（reducer / prompt / projector 一律跳过，doctor 计数），假或缺 ⇒ `UndecodableRecord`。**缺键只许读作 false**。谁声明 `ignorable: true`：`durability_of` 旁的同一张策略表——本轮为空。

### 7.4 表布局只加不改

`migrate_add_session_events` 保持「缺列就加」，census 钉「事件表迁移只有 ADD COLUMN」；**不引入 `PRAGMA user_version` 门**——「日志比二进制新就拒开」是 fail-dead，逐行隔离已把降级压成「几个会话被拒、其余照跑」。`RunEnvelopeSnapshot` 每个字段都是 `Option` + `default`（census）。

---

## 8. §5 · 恢复的执行形状

### 8.1 有界并行（#14）

boot 的 detached 任务保持「reconcile 先于 resume」（旧行回填在新行追加之前——真依赖）；`resume_interrupted_runs` 的重触发改 **`JoinSet` + `Semaphore([resume] max_concurrent，默认 2)`** 扇出，每个重触发仍先拿 `ResumeSlot`、内部仍受 Execute 车道的 `RunConcurrency` 许可（前者「谁在恢复」，后者「谁在跑」）。`ResumeReport` 从 `JoinSet` 收拢求和（按候选顺序）。**busy-queue `reinject_survivors` 在扇出启动之后立刻跑**——幸存消息重进逐会话准入（会话正被恢复 ⇒ lane-deferred），实现 agent 核实 `reinject` 确实过准入。`[resume] max_concurrent` 经 schemars 自动进 `config_*` 工具面（R8），不进 SESSION_KNOBS。

### 8.2 两条 boot 臂并成一条（#1）

`orphan_notice.rs` **CUT**。`reconcile_orphaned_tasks` 继续对账 `tasks` 表，「通知」半边搬进 `ResumeCoordinator`，排在 marker 扫描**之后**，于是能分开：
- **(a)** 日志有 open run / `Unanswered` ⇒ 恢复臂处理，不发「请重发」。
- **(b)** `tasks` 行在、日志里**没有**该 task 的任何事件（admit / `BeforeAgentStart` hook 期间崩）⇒ 唯一「请重发」为真的情形。经 `emit_event` 写 `SystemMessage`：「你于 T 发出的一条消息在记录前丢失{：«前 80 字»}，请重发」——`tasks` 行是否带输入由实现 agent 核实。写完把该 `tasks` 行标 `notified`，幂等。
- **(c)** `abandoned` ⇒ 既有 `RunFinished{Abandoned}` closer 之后追加一条 `SystemMessage` 说明原因；run 已关，天然只写一次。`refused` **不写通知**（每次 boot 都会再 refuse），出口是 doctor finding。

---

## 9. §6 · 运行时资源墓碑（#12 #13）

### 9.1 同一张表

PTY 会话与 bash 后台作业都是「本进程 spawn 的 OS 子进程」——进**同一个** `process_journal`（加 `kind: Bash | Pty`），不造第五份 sidecar（现有四份：subagent / bash / busy-queue / scratchpad，判据 #16）。`PtyManager` 内存表照旧管**活的**会话，journal 是它的持久影子：spawn **前**写 `{id, kind, pid: None, cmd, cwd, session_key, started_at}`（意图），spawn 后补 `pid` + **进程创建时刻**（`sysinfo`，防 pid 复用），退出时写终态。

### 9.2 boot 探活，两句墓碑

`init_and_reconcile` 对每条无终态记录探活（pid 存在 ∧ 创建时刻一致）：
- `exited_during_restart`：「作业/终端 X 由上一个服务进程启动，那个进程在 T 被终止；X 的进程已退出（退出码未知），最后输出如下：…」
- `still_running_unattached { pid }`：「…X 的进程**仍在运行**（pid N，启动于 T2），本服务无法重新附接；要停止它，运行 `taskkill /PID N` / `kill N`」。
每臂说出模型下一步能做什么；**不杀**（U5）。探活抽成 `probe_liveness(pid, created_at) -> Liveness` 纯边界，单测注入。R1：pid 探活是 spawn 它的父进程就地做的事，属 `session_lock.rs` 同类合法开口。

### 9.3 消费者是工具面

`bash{process_action: wait|status|kill}` 与 `terminal` / `pty.*` 对「不在活表、journal 有墓碑」的 id 回墓碑文本，不是 `not found`。`kill` 对 `still_running_unattached` 给出命令而不假装执行。返回体带 `lost_with_restart: true`（`skip_serializing_if`，现有路径 byte-identical）。墓碑保留期沿用 journal 既有清理规则（没有就加「N 天或 M 条」并 census）。Panel `pty.list` **本轮不列墓碑**。

---

## 10. Phase 2（本 spec 定稿，A–D 后另开 plan）

### 10.1 E · 子 agent 运行标记统一

子会话由 spawner 写 `RunStarted{envelope}` / `RunFinished`（`subagent_spawner/mod.rs:675,744,1009`），`reduce_run` 父子一视同仁，r2 ⑭ 的合并读法收缩。`SubagentSpawned` 从「拿到并发许可之后」挪到**之前**并带 `queued: true`，子的 `RunStarted` 即「真的开始」——排队中的子从 sidecar 独有变成日志事实（codex `thread_spawn_edges` 的等价物：`Spawned` 开、`Returned` 关）。子 envelope 记 `delegation_depth`。父的悬空 `subagent` 调用得到逐子事实；排队未开始的子给 `ToolCallParked` 第四种 `reason: Concurrency`。**A7 维持**。

### 10.2 F · 命令 / 旋钮 / hook 成为事件（含 #15）

三对：`CommandInvoked{name,args,source} → CommandSettled{kind,text?,source_seq?}`（永不重放）；`KnobChanged{key,value,source: User|Tool|Resume|Config}` 在 `identity_meta.custom[key]` 的唯一写点写（先数写者）；`HookInvoked{point,handler_id} → HookResult{decision,updated_output?,duration_ms}`。**F 推翻 A3**：「快照 > 会话」改成「快照时刻与其后最新 `KnobChanged` 之间谁更晚谁赢」；`exec_tier` 只收紧不变。**#15 在这里关**：`effective_assistant_text(log)` 一处派生，projector 与 prompt builder 都读它。

### 10.3 G · 自动压缩持久化

`CompactAndContinue` / `CompactToFit`（`directive.rs:95-228`）改走与手动相同的一批 `[SystemMessage(summary), CompactionPerformed] + Retire::Through`——重启后不重算摘要。`CompactionPerformed` 得到渲染器：Panel / TUI 压缩分割线（否则按判据 #17 CUT）。还债：`session_projector::event_retired` 分不清「擦除」与「停止重放」——retire 加 `reason: Compacted | Rewound`，projector 对 `Compacted` 保留行。R10：全在 `src/context/compact/`。

**依赖**：E 依赖 5.1 与 6.1；F 依赖 4.1；G 依赖 4.1 的 `Retire::Through`。三者互不依赖，可并行三个 plan。

---

## 11. 验证纪律

- **六条最小可信集**（CLAUDE.md）；Panel 因 5.2 / 6.4 文案改动 ⇒ `cargo test -p aleph-panel --lib` + `just wasm`；TUI ⇒ `cargo test -p aleph-tui`。`--lib` 基线先在 `5e85060b8` **重测**，之后比名字不比条数。
- **真机夹具（Node，`qa/resume_boundary/run.sh`）**：既有 5 阶段重跑 + 新增 6 阶段，每阶段带**断言地板**：
  - `parked`：审批门停车时 SIGKILL ⇒ 第四臂文本；
  - `unanswered`：sleep 10s 的 `BeforeAgentStart` hook 撑开 seed→RunStarted 窗口后 SIGKILL ⇒ `Unanswered` 被恢复；kill 在 hook 里 ⇒ 8.2(b) 恰一条通知、二次 boot 仍恰一条；
  - `ratchet`：永远 sleep 的 hook，三次 boot ⇒ `abandoned`，第四次不触发；
  - `parallel`：三会话 open，一个慢 ⇒ 另两个先 `resumed`；
  - `undecodable`：`node:sqlite` 手写一条未来行 ⇒ 该会话 refused、其余 resumed；加 `ignorable` ⇒ 全 resumed、doctor 计 skipped 1；
  - `tombstone`：`sleep 300` 后台作业跨两次 boot ⇒ 先「仍在运行 pid N」后「已退出」。
  **失效点全部用可配置的 hook / 审批门撑开，生产代码不加任何 failpoint。** 旧 Python 阶段：`crash` 被 `parked`+`unanswered` 覆盖删除；`attribute` 移植成 Node。
- **变异账本**（plan 尾部记录**观测到的**红）：4.1 retire 出事务 ⇒ 手动 compact 的事务性测试红；5.1 `ResumeAttempted` 挪后 ⇒ `ratchet` 红；6.1 `Parked` 写点挪后 ⇒ `parked` 红；7.1 逐行 `Result` 改回整批 `?` ⇒ `undecodable` 红；8.2 `orphan_notice` 复活 ⇒ census 等式红；9.1 意图挪到 spawn 后 ⇒ `tombstone` 的「意图在、pid 缺」断言红。
- **agent 纪律**（r2 plan §0.1–0.3 原样）：首条命令 `git status --porcelain`；「Verified:」行必须能在 transcript 里找到真实 cargo 调用；编排者不 `git add` agent 正在改的文件；heavy build 用 detached pwsh + 前台轮询，`CARGO_TARGET_DIR=D:/Workspace/Aleph/target` 共享、`CARGO_PROFILE_TEST_DEBUG=line-tables-only`、`--test '*'` 加 `-j 1`。

---

## 12. 熵减（本轮删除）

`orphan_notice.rs` 整文件 · `trailing_starts` 计数 · `AgentInstance::{add_message*, reset_session}` 及其测试（先核零生产调用者） · `SessionEventStore::append` 单条路径（并入 `append_batch`） · `AssistantRunMeta` 花费字段与其读者 · `dispatch.rs:1173-1180` 静默跳过臂 · `balance_run_markers_after_retire` 作为独立第二步 · 四条撒谎注释（`start/mod.rs:391` · `helpers.rs:327-334` · `projection_reconciler.rs:29-31` · `service.rs:67-80`「nine sites」→ census） · `qa/resume_boundary` 两个 Python 阶段。

---

## 13. 刻意不做（附理由）

| 项 | 理由 |
|---|---|
| 审批 / 澄清重投递 | U4；Phase 2 候选 |
| 杀孤儿进程；Job Object / `PDEATHSIG` 子进程寿命绑定 | U5「不杀」 |
| MCP stdio 旧进程孤儿 | 与 §9 同形不同表；本轮只核一行 `kill_on_drop` |
| 部分流式输出持久化 | 三家也丢；pi v4 frames 列表要每秒 N 次写，独立一轮 |
| 落盘投影水位 | A6 仍成立 |
| 两套 run id join | A5 |
| Panel/TUI Resume 按钮；`SessionInfo.state` 清扫 / CUT | A8；零读者但删 wire 字段是协议改动 |
| `pty.list` 列墓碑 | 四张脸各要一个渲染器；模型是本轮消费者 |
| `PRAGMA user_version` 门 | fail-dead |
| `RunStarted` 挪到 seed 前 | 破坏 steer 边界 |
| `last_seen_seq` 增量回放 | r2 #7 |
| codex 式 `replacement_history` 物化 | retire+summary 已是有界 prompt，物化是第二份表述 |
| 跨进程 writer lock | 单例 `flock` 已覆盖 |
| E / F / G 实施 | Phase 2 |
| 合并 main | U7 |

---

## 14. 环境假设

1. Windows 主机；worktree `D:\Workspace\Aleph\.claude\worktrees\persistence-r3`，共享 `CARGO_TARGET_DIR=D:/Workspace/Aleph/target`；`--test '*'` 需 `-j 1`。
2. alephcore lib-test 一次完整编译 ~16 min，超过 Bash 10 min 上限——detached pwsh + 前台轮询。
3. `python3` / `python` 是 WindowsApps 占位（退出 49）——夹具全部 Node；`node:sqlite` 需 Node ≥ 22.5，实现 agent 先核版本。
4. 基线 `--lib` 红名单在分支基点重测后存 scratchpad，与记忆 `windows-alephcore-lib-baseline-2026-09-02` 比名字。
