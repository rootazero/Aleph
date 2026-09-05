# Run Reduction — 崩溃边界与进展证据的单一推导

- **日期**：2026-08-31
- **分支**：`run-reduction`（worktree `../Aleph-run-reduction`）
- **状态**：设计已批准，待实施
- **切片**：这是「持久化事实 → 派生运行状态 → 派生模型上下文 → 派生 UI → 崩溃后重新归约」这条链的 **A 片：归约内核**。B 片（投影不再有损）与 C 片（跨 store 词表统一）见 §8。

---

## 1. 问题

Aleph 的这条链已经有两环是健康的：

| 环节 | 现状 | 判定 |
|---|---|---|
| 持久化事实 | `session_events`（`SessionEventStore`，file jsonl + sqlite 双 backend） | ✅ SSOT 成立 |
| 派生模型上下文 | `harness/agent/prompt.rs::build_prompt(events, tail_start)`，**纯函数**，孤儿 `tool_use` 剥离 / steering 重排 / 四条 transient nudge 都在此一处派生 | ✅ 已是纯 reducer |
| **派生运行状态** | **没有单一派生** | ⚠️ 本轮目标 |
| 派生 UI | `MessageProjector` 异步 drain → `messages`，满队列 `try_send` 丢弃 | ⚠️ B 片 |
| 崩溃后重新归约 | 五条互不相干的启动通道 | ⚠️ 本轮收敛其中三条 |

### 1.1 「运行状态」今天是平行内存态，不是派生态

五条归约通道，各自一套崩溃边界推导：

1. `gateway::projection_reconciler::ProjectionReconciler::reconcile_interrupted` — events→messages 回填，boot only
2. `gateway::resume_coordinator::ResumeCoordinator::resume_from_markers` — 重跑 harness run，boot + 按需
3. `agents::background_persistence::init_and_reconcile` — 后台子 agent 孤儿墓碑，boot only
4. `agents::swarm::tasks::store::runs::abandon_orphaned_runs` + `teams::dispatcher::schedule::reclaim` — swarm 任务，有活体扫描
5. `tasks::cron::service::catchup::run_startup_catchup` — cron 补跑

「interrupted」这个动词因此有**三张脸、三份推导**（判据 #9）：

| store | 词表 | 谁裁决 |
|---|---|---|
| `session_events` | `RunOutcome::{Completed,Cancelled,Errored,Abandoned}` + `ScanVerdict::{Clean,Interrupted}` | `classify_markers`：数尾部 `RunStarted` |
| `background_persistence` sidecar | `RunPhase::{Running,Settled,Abandoned}` → `status_label()` = `"interrupted_by_restart"` | `init_and_reconcile`：boot 时 `Running` 即孤儿 |
| coord task store（SQLite） | `TaskRunStatus::{Running,Completed,Failed,Timeout,Abandoned}` + 哨兵 `RUN_ABANDONED_BY_JANITOR_ERROR` | `abandon_orphaned_runs`：live id 名单外即孤儿 |

### 1.2 两处推导住在同一个文件里却互不知情

`resume_coordinator.rs` 自己就有两份：

- `classify_markers(&[SessionEventRecord]) -> ScanVerdict`（L169）——只看 run marker，数尾部 `RunStarted`
- `compute_boundary_repairs(&[SessionEventRecord]) -> Vec<SessionEvent>`（L296）——扫**整份日志**，给每个无回执的 `ToolCallRequested` 合成 `ToolError`

`ProjectionReconciler` 复用了第一份，`repair_boundary` 用第二份。没有任何一处东西说得出「这次 run 处在什么状态」。

### 1.3 一条靠默契成立的不变量

`compute_boundary_repairs` 扫全日志却是正确的，理由是**所有非崩溃的终止路径都会自己关闭未执行的调用**：

- `harness::agent::think::close_unexecuted_tool_uses` — `/stop` 之后为剩余 `tool_use` 合成 `ToolError`
- `harness::agent::act::emit_deferred_tool_results` — steer 检查点为跳过的调用合成 `ToolResult`
- `tools::scoped::dispatch` — 审批拒绝写 `ToolCallDenied`，act.rs 随后仍写 `ToolError`

于是「悬空 ⟺ 本次崩溃」今天成立。但这条依赖的理由住在**另外两个文件**里，`resume_coordinator.rs` 这一侧没有任何断言钉住它。这正是判据 #1 的形状：一份表述描述的不是事实，而是另一个子系统的行为。

### 1.4 由此产生的一个错误归因（本轮修复）

每个悬空调用今天都收到同一句：

> `OUTCOME UNKNOWN — the server restarted after this call was dispatched but before its result was recorded. …`

对**上一次**崩溃留下、当时没被修复过的悬空调用，这句话在时间上是假的。可达路径有二：

1. 崩溃时 `[resume] enabled = false`，之后才开启自动扫描
2. 会话超出 `resume` 的 recency 过滤，后来由 `agent.resume` / `aleph-server resume` 手动点名恢复

两种情况下，`repair_boundary` 会把一次**更早**的悬空说成「本次重启」。判据 #17：错的标签比缺的贵。

### 1.5 「做了一半」读不出来

`agents::subagent_tool::recovery::Recovered` 是跨两个持久源做 union 的正确原型（模块 doc 明写「两个持久源覆盖的集合不同」），但 `Recovered::Interrupted { child_session, flow }` 是**二值判决**——它说不出前任做到了哪一步。

这正是三次中断恢复转录里人工做的那件事：读磁盘上的半成品，分成「已做对 / 做了一半 / 没开始」。`Interrupted` 今天只能说「它没做完，你自己去读子会话」。

---

## 2. 参考项目对照

| 维度 | codex (`rollout-trace`) | pi (`harness/reducer.ts`) | deepseek-harness (`session-controller`) | 本轮取用 |
|---|---|---|---|---|
| 归约产物 | `replay_bundle() -> RolloutTrace`，带 `REDUCED_TRACE_SCHEMA_VERSION` + 落盘缓存 | `LaneState`（`openOperation` / `toolBatch` / `pendingSteer`） | host 唯一计算点，`key → {value, seq}` | **取「具名归约产物」**，不取落盘缓存（§8.6） |
| 未完成操作追踪 | reducer 内 `ToolCallStarted` 待决集，replay 结束 `resolve_pending_spawn_edge_fallbacks()` | `toolBatch.calls[].started / resultExists` | — | **取**：`DanglingCall` + `run_anchor` 归属 |
| 日志矛盾 | — | `RecordLogCorruption` 12 种闭集理由，恢复时**拒绝**而非修复 | `invariant.ts` | **不取**（§8.2） |
| 配置从日志派生 | `model_context.rs` | `EffectiveLaneConfiguration` | `model-selection-projection.ts` | **不取**（§8.3） |
| UI 重连 | `thread_history/{realtime,segment_paging}` | — | `asOfSeq` 一致性切 + push frame，higher seq wins；`mergeOrderedBaseline` | **不取**（B 片） |
| 性能设施 | `reverse_jsonl_scanner` / `seekable_reader` / `session_index` | — | — | **不取**（YAGNI，§8.6） |

**架构映射原则**：不移植 codex 的 bundle 文件格式，也不移植 pi 的 lane 概念。Aleph 已有的 `SessionEvent` 闭集枚举 + `SessionEventRecord{seq}` 就是它们的 raw event log；缺的只是那个**具名的、纯的归约函数**。

---

## 3. 范围

### 3.1 归约要回答的两个问题（其余不答）

1. **崩溃边界**：哪些 tool call 越过了派发线而没有回执，且分别属于哪一次 run
2. **进展证据**：这次 run 停下前做成了什么

### 3.2 输送面

- 模型面（`repair_text` 进 prompt；`subagent` 工具结果多出事实句）
- 三个服务端消费者：`ResumeCoordinator` / `ProjectionReconciler` / `subagent_tool::recovery`
- **不动** CLI / TUI / Panel / `shared/protocol` wire 契约

### 3.3 一条明确的自我约束（R7）

归约算出的进展**不得**用来替模型做重跑决策。「进展过半就只补未完成的部分」这类规则引擎是判据里「越俎代庖」的形状。归约只陈述事实；要不要重做是模型的推理。这与转录里人工采用的策略一致——给的是「先 git diff 读前任做了什么」，不是「系统已判定第 3 条做了一半」。

---

## 4. 数据形状

新文件 `src/session/reduction.rs`（约 200 行 + 测试）。

放在 `src/session/` 而非 `src/harness/`：归约是持久事实的**读面**，不是 Think→Act 轮次调度。R10 的 12 文件锁与 `budget.rs::CEILING` 棘轮不受影响。

```rust
/// 一次 run 的崩溃处置。刻意只有两个变体。
pub enum RunDisposition {
    Clean,
    Interrupted { trailing_starts: usize },
}

pub struct DanglingCall {
    pub call_id: String,
    pub tool_name: String,
    pub turn_id: TurnId,
    /// 这个调用属于哪一次 run。
    /// `Some(seq)` = 最后一次 `RunStarted` 的 seq（本次崩溃）；
    /// `None`      = 更早的某次 run 留下、从未被修复过的悬空。
    pub run_anchor: Option<EventSeq>,
}

pub struct RunProgress {
    pub tool_calls_dispatched: usize,
    pub tool_calls_answered: usize,
    pub assistant_messages: usize,
    pub last_activity_at: Option<Timestamp>,
}
```

**`RunProgress` 的作用域 = `run_anchor` 之后**（含锚点事件本身之后的所有事件），不是全日志。理由与 §4.2 同源：进展是「**这次** run 做成了什么」，一个跨越多次 run 累加的计数命名的是另一个集合。

- `run_anchor: Some(seq)` → 统计 `events` 中 `seq > anchor` 的部分
- `run_anchor: None`（日志里没有任何 `RunStarted`——legacy 会话，或子会话死在 run marker 落盘之前）→ 统计**全部** events，并且这是唯一的全日志口径。这不是回退到「更宽松」，而是「这份日志只有一次 run 的量」这个事实的直接表达。

`last_activity_at` 取作用域内**最后一个携带 `at` 的事件**的时间戳，任何类型皆可（`AssistantMessage` / `ToolResult` / `ToolError` / `UserMessage` …）。取「最后一个事件」而非「最后一个工具事件」，因为问题是「它什么时候还活着」，不是「它最后一次用工具是什么时候」。

```rust

pub struct RunReduction {
    pub disposition: RunDisposition,
    /// 最后一个 `RunStarted` 的 seq（不是下标）。
    pub run_anchor: Option<EventSeq>,
    pub run_id: Option<String>,
    pub dangling: Vec<DanglingCall>,
    pub progress: RunProgress,
}
```

### 4.1 为什么 `RunDisposition` 只有两个变体

考虑过第三个 `NeverStarted`（日志里根本没有 run marker 的 legacy 会话），**决定不加**：今天没有任何消费者会把它与 `Clean` 区别对待。一个没有读者的变体正是这个仓库自己删过两轮的东西——`ApprovalSource::Autoconfirm`、`ErrorKind` 的六个变体，`src/session/events.rs` 的 doc 里写着为什么。判据 #2：一个不会改变任何人行为的谓词等于没判。等出现第一个真读者时，它和它的读者同一笔进来。

### 4.2 为什么锚点是 seq 不是下标

`reduce_run` 会被喂两种输入：`load_all_events()`（全日志）和 `get_events(id, offset, limit)`（分页切片）。下标在两种输入下含义不同，seq 在两种输入下是同一个值。判据 #12：一个在 A 输入里选出、在 B 输入里应用的下标，命名的是另一个集合。

### 4.3 单一推导的机制

```rust
/// 处置的唯一推导。输入是 run marker 序列。
pub fn reduce_disposition(markers: &[SessionEventRecord]) -> RunDisposition;

/// 全量归约。内部把 events 过滤成 marker 子序列后**调用上面那个函数**，
/// 不重新数一遍。
pub fn reduce_run(events: &[SessionEventRecord]) -> RunReduction;
```

于是「什么算中断」在整个仓库里只有一处字面表达。配一条 proptest 作为反漂移装置（G1，§6）。

两个函数都是**纯的**：零 I/O、零 async、零全局状态。这是它们能被严肃变异证伪的前提，也是拒绝方案 3（下沉进 store trait，两个 backend 各一份实现）的理由——判据 #10：两边各持一份形状就会互相抵消。

---

## 5. 三个消费者的接线

### 5.1 `ResumeCoordinator` — 两处推导合并，边界按归属分流

| 今天 | 之后 |
|---|---|
| `classify_markers(&markers)` | `reduce_disposition(&markers)` |
| `compute_boundary_repairs(&events)` 扫整份日志 | `reduce_run(&events).dangling`，按 `run_anchor` 分流 |

`repair_text(tool, provenance)` 出两句，共用一个构造器：

- `Provenance::ThisRestart` → 现有措辞不变
- `Provenance::EarlierRun` → 「这个会话更早的一次 run 结束时没有记下这个调用的结果」

两句都必须含五个语义要点：`OUTCOME UNKNOWN` / 显式否定「失败」/ 点名工具 / 副作用可能已落地 / 核实现状后再决定是否重做。由**同一组断言**在两条臂上各查一遍（判据 #14：闸的两个方向都要问）。

**`repairs_for` 对两种归属都出修复事件**，只是措辞不同——不是「只修本次的、旧的留着不管」。理由：旧悬空一旦不修，`build_prompt` 会把它的 `tool_use` 块当孤儿丢掉，模型从此看不见那次调用发生过；而它的副作用可能仍然在磁盘上。缺的读起来像「还没有值」，那正是判据 #17 要防的。

保留不动：
- `load_run_markers()` — 廉价索引预过滤（`WHERE event_type IN ('run_started','run_finished')`），两阶段的第一阶段。删了 boot 扫描会退化成全库读。
- `latest_project_root` — 独立职责（恢复时的工作目录），不属于崩溃边界。

### 5.2 `ProjectionReconciler` — 只换推导，不换触发条件

`classify_markers` → `reduce_disposition`。触发条件仍是「run 被中断」。

**本节收益有限，明写在此**：它只是让 `ProjectionReconciler` 不再持有第二个 `classify_markers` 调用点。背压丢弃导致 `Clean` 会话永久丢行（§8.1 的 D1）**本轮不修**——那个修法需要一个投影水位的持久化处，属于 B 片。不在这里夹带半成品水位。

同一轮修掉两处说谎注释（判据 #1，最贵的那份表述在注释里）：

- `src/gateway/session_projector.rs:20` — 说「P1 has no events→messages reconciliation pass；a boot-time reconciler is a P2 follow-up」，而 P2 早已落地
- `src/gateway/projection_reconciler.rs:12` — 说「file backend only」，而 sqlite backend 经 `source_seq` 列（`session_manager/ops/crud.rs:501`）重建 `row_id`，实际覆盖

### 5.3 `subagent_tool::recovery` — 「做了一半」第一次可读

`Recovered::Interrupted { child_session, flow }` 增加 `progress: Option<RunProgress>`。

预算纪律跟着该模块自己已写下的规矩走：

| 入口 | 语义 | 加载子会话日志？ |
|---|---|---|
| `resolve_forgotten(ids)` → `to_json` | 「告诉我这一个」，全文本，已经贵 | **是**，按 id 精确加载，`reduce_run` 出 progress |
| `list_from_log(known)` → `to_list_row` | 目录，几十行，已按 `LIST_RESULT_PREVIEW_CHARS` 截断 | **否**，`progress: None`，行里仍给 `child_session` |

目录行与详情行的差别是**详略**，不是**事实冲突**——与该模块已有的 `result_preview` / `result_chars` 是同一先例。`Option<RunProgress>` 的 `None` 只说「这一面没查」，不说「没有进展」（判据 #8：fail-closed 的答案只有资格说「我不知道」）。

`to_json` 的 `Interrupted` 臂 note 增加一句**只陈述、不判决**的事实：

> Before it stopped it had dispatched N tool calls, M of which recorded a result, and produced K assistant messages; last activity at T. Read the child transcript to judge what is done.

---

## 6. 验证纪律

每条守卫先回答判据 #2 的问句：**在什么情况下这东西会变红？** 答不出具体情形的不写。

| # | 守卫 | 守什么 | 证伪方式（提交前实跑，看红的是不是这一条） |
|---|---|---|---|
| G1 | proptest `∀log. reduce_run(log).disposition == reduce_disposition(markers_of(log))` | 「什么算中断」只有一处字面表达 | 给 `reduce_run` 加捷径（「有 dangling 就算 Interrupted」）→ 必须红 |
| G2 | 归属分流，夹具用**可达**形状 `[Started(a), Req(c1), Started(b), Req(c2)]`（run a 崩溃 → 当时 `[resume] enabled = false` 故无修复 → 用户发新消息起 run b → b 也崩）→ `c1.run_anchor == None` ∧ `c2.run_anchor == Some(seq_b)`，两句 repair 文本不同 | 旧悬空不再被说成「本次重启」 | 把 `run_anchor` 恒设成 `Some(anchor)`（即今天的行为）→ 必须红。**这条同时是「今天那个缺陷真的存在过」的证据** |
| G2b | 不变量违反形状 `[Started(a), Req(c1), Finished(a), Started(b)]`——一次**干净结束**的 run 却留下悬空调用，即 §1.3 那条默契被打破 | 该形状被归约如实报出（`c1.run_anchor == None`），不被静默吞掉也不被误报成本次重启 | 把「Finished 之前的悬空」过滤掉 → 必须红。⚠️ 这条**不**做 fail-closed 拒绝（§8.2 已裁），只保证事实不丢 |
| G3 | 两句 repair 文本**各自**含五个语义要点 | 闸的两个方向都问 | 任一臂删掉任一要点 → 必须红。断言语义不断言字节：`!contains("failed")` 会被文本自己的否定句命中，§4.13a 记着第一版就是这么错红的 |
| G4 | `RunProgress` 四字段各一条 | 进展是真数出来的 | 把 `tool_calls_answered` 的源换成 `dispatched` → 必须红**且只红这一条**（判据 #18） |
| G5 | 计数 mock：`list_from_log` 的 `get_events` 调用次数 == 1 | 目录面没有多出 N 次子会话读 | 在 `to_list_row` 里加一次子会话加载 → 必须红。刻意断言「调用没有发生」，判据 #4 的反向用法 |

**不加的守卫**：删掉 `classify_markers` / `ScanVerdict` / `compute_boundary_repairs` 后不写「全 crate 零引用」census——编译器已经是那条守卫（`pub(crate)` 符号删掉后还有引用就编译不过）。给同一个问题造第二个真源。

### 6.1 真机 QA 装置 `qa/resume_boundary/`

单测断言的是 `repair_text` 的字节，证明不了那句话真的**到达了 prompt**（判据 #4：守卫要断言效果到达了，不是调用发生了）。本轮修的恰恰是「模型读到一句假话」，所以必须有真机面。

沿用现有约定：`qa/lib/scratch_home.sh` + `qa/lib/build.sh` + `qa/busy_input/` 的确定性 mock provider；独立 scratch `ALEPH_HOME`；`KEEP=1` 保留现场。

```
./qa/resume_boundary/run.sh crash      # 起 server → 驱动一个长工具调用 → kill -9 →
                                       # 重启 → 断言 repair 事件落盘，且 mock provider
                                       # 收到的下一轮请求体里含 OUTCOME UNKNOWN
./qa/resume_boundary/run.sh attribute  # 制造「上一次崩溃留下、本次才恢复」的悬空
                                       # （崩溃时 [resume] enabled = false，重启后开启）
                                       # → 断言它拿到的是第二句措辞，而不是「本次重启」
```

`crash` 阶段的断言必须落在 **mock provider 收到的请求体**上，不是落在服务端日志上——日志能证明 repair 被合成，证明不了它进了 prompt。

`attribute` 阶段是本轮唯一能真机证伪 §1.4 那个错误归因的地方：跑在修复前必须 FAIL（拿到第一句），修复后 PASS。**提交前两次都跑，把两次输出记进本文件的实施记录**。

### 6.2 验证集（CLAUDE.md 六条中与本轮相关的四条）

```bash
cargo test -p alephcore --lib                 # 真跑，不是 --no-run：reduction 是本轮主体
cargo test -p alephcore --bins                # start/mod.rs 的 boot census 会被 reconciler 构造变化碰到
cargo test -p alephcore --features test-helpers --test '*' --no-run
cargo clippy --workspace --all-targets        # 先 just _stage-shell-placeholders
```

不改 `interfaces/webchat/` `interfaces/tui/` `interfaces/cli/` `desktop/`，那几条不跑。

---

## 7. 熵减清单

| 位置 | 处置 |
|---|---|
| `gateway/resume_coordinator.rs::classify_markers` | **删**，并入 `session::reduction::reduce_disposition` |
| `gateway/resume_coordinator.rs::ScanVerdict` | **删**，被 `RunDisposition` 取代。⚠️ `memory/content_scanner.rs` 有个同名不同物的 `ScanVerdict`，不碰 |
| `gateway/resume_coordinator.rs::compute_boundary_repairs` | **删**扫描逻辑，保留薄构造器 `repairs_for(&RunReduction) -> Vec<SessionEvent>` |
| `gateway/session_projector.rs:20` 注释 | **改写**（P2 回填器已存在） |
| `gateway/projection_reconciler.rs:12` 注释 | **改写**（sqlite backend 也覆盖） |

明确保留、并说明为什么不是死代码：`load_run_markers()`（廉价预过滤）、`latest_project_root`（独立职责）、`Recovered` 三变体（union 原型，只加字段）。

---

## 8. 刻意不做（附复现步骤，留给下一轮）

### 8.1 D1 — 背压丢弃的行永远回不来（B 片）

`session_projector.rs:355` 满队列 `try_send` 丢行 → 该 run 后来正常 `RunFinished` → 归约判 `Clean` → `ProjectionReconciler` 计入 `skipped_clean` 跳过 → Panel 永久少一行，SSOT 完好。

注释自己写着「may lag until a P2 reconciler catches it up」，而 P2 回填器的触发条件（run 被中断）**不覆盖这个形状**（判据 #3：守卫的绿只覆盖它认得的那种形状；判据 #7：两端完整而中间没线）。

复现：压满 `QUEUE_CAP = 4096`（单轮内高频工具调用），确认 `projector queue full; dropping` 出现，等 run 正常结束，重启，观察 Panel 该会话缺行且 `reconcile` 报 `skipped_clean += 1`。

修法：把回填器的触发条件从「run 被中断」改成「投影水位有缺口」——需要一个水位的持久化处。

**→ 2026-09-02 已做**（`2026-09-02-crash-recovery-r2-design.md` B 片 / plan T5）：走的**不是**水位而是**seq 集合求差**（`present` 谓词直接问转录已有哪些 `source_seq`），所以持久化水位那一步整个不需要了——`sessions.projected_through` 反过来成了下一轮的「刻意不做」第 1 条。丢行改成记 `missed`（载荷本来就在 SSOT），`Clean` 闸与 `skipped_clean` 已删，boot 候选 = 活动窗口 ∪ Interrupted，另有 `core/projection-holes` doctor 检查做无界扫描。见 FEATURE_LOCATOR §4.13a ⑯ 与 SESSION_SERVICE.md。

### 8.2 日志矛盾 fail-closed（用户已裁）

pi 的 `RecordLogCorruption`（12 种闭集理由，恢复时拒绝而非修复）。Aleph 今天遇到矛盾日志（两个未闭合 run、`call_id` 重复、`ToolResult` 无对应 `Requested`）会静默按最宽松路径走。

**→ 2026-09-02 已做**（plan T1/T2）：`LogContradiction` 九变体闭集，两条 REJECT + 七条 REPORT，`reduce_run` / `reduce_disposition` 返回 `Result`；会话进 `ResumeReport.refused`，收据说 `log_inconsistent`，doctor `core/session-log` 每种矛盾一条 finding。**没有照抄 pi 的 12 条**——闭集是从本仓日志里数出来的，而 REJECT 只留给「切片不可归约」那两种：把 `FinishWithoutStart` 判成 REJECT 会拒掉本仓自己的 `abandoned-*` / `delegated-*` closer（附录 D.0.164）。见 §4.13a ⑩⑪⑫。

### 8.3 恢复时重放当时的模型与档位（用户已裁）

pi 的 `EffectiveLaneConfiguration` / codex 的 `model_context.rs`。被恢复的 run 今天用的是**现在**的会话旋钮（model pin / 推理档 / 执行档），不是崩溃时那一份。会触及 `SESSION_KNOBS` 的 precedence（请求 > 会话 > 全局）。

**→ 2026-09-02 已做**（plan T4）：`RunStarted.envelope`（`RunEnvelopeSnapshot` 六字段）+ `plan_resume`。四根走 request rung（快照 > 会话 > 全局），**`exec_tier` 例外**：走天花板键、只收紧不放宽（附录 D.4.39）。model 先 `validate_snapshot_model` 再回放，降级要说给模型听并计 `degraded`；恢复不 stamp 会话行；无快照的 marker 计 `unsnapshotted`。见 SESSION_KNOBS.md「崩溃恢复」段与 §4.13a ⑬。

### 8.4 CLI / TUI / Panel 面（用户已裁）

`aleph-server resume` / `agent.resume` 的返回体不携带进展摘要。要动 `shared/protocol` 就是跨 crate wire 契约（判据 #10：键集要放进两边都依赖的那个 crate 并用它构造响应）。

**→ 2026-09-02 已做**（plan T3/T7）：`shared/protocol` 新增 `resume.rs` / `sessions.rs` / `metrics.rs`，`session_thread.rs` 增 `LastRunState`；服务端**用这些类型构造**响应，四个手写镜像已删。渲染点四个（Panel `chat_sidebar::{last_run_notice, run_badge}`、TUI `commands::{last_run_notice, last_run_mark}`）。见 §4.13a ⑮。

### 8.5 跨 store 词表统一（C 片）

sidecar 的 `RunPhase` 与 coord 的 `TaskRunStatus` 仍各说各话。第 4、5 条归约通道（swarm / cron）本轮不碰。

**→ 2026-09-02 部分做**（plan T6）：sidecar 不再整条替换日志（`Recovered::Sidecar` 合并两源），`settled_label` 读 outcome，`process_journal` 的 `JobPhase` 孪生同批改。**词表合并本身仍未做**——「三套子 agent 词汇合并为一份日志派生态」是 r2 spec §7 第 4 条的「刻意不做」（裁定 A7）。见 §4.13a ⑭。

### 8.6 归约结果落盘缓存（YAGNI）

codex 的 `REDUCED_STATE_FILE_NAME` + `REDUCED_TRACE_SCHEMA_VERSION`。marker 预过滤已经很便宜，全量归约只跑在候选上；缓存本身就是「同一事实的第二份表述」的教科书形状，要配 schema 版本棘轮才安全。等真机量到再说。

**→ 2026-09-02 仍不做，但真机数字到了**：`qa/resume_boundary/run.sh claims` 把 A10 推迟的两处无上限读当**数字**打印出来（`chat.history` 每次 attach 的 `load_all_events`、`sessions.list` 每次的 `load_run_markers()` 全表）——「记录在案的成本」只有带着数字才算记录在案。上限本身是 r2 spec §7 第 6 条的刻意不做。

---

## 9. 环境假设（明确记录）

1. **`/Volumes/TBU` 当前未挂载**（`ls /Volumes/` 只有 `TBU4`）。CLAUDE.md 记着会话检出应在 `/Volumes/TBU/Workspace/Aleph`，该路径不存在，故本轮在 `/Volumes/TBU4/Workspace/Aleph` 就地开 worktree。
2. **worktree 必须自带 `CARGO_TARGET_DIR`**。`.cargo/config.toml` 钉了一个共享绝对 target dir（`qa/session_order/run.sh` 就是因此才用 `cargo metadata` 问真实路径）。不隔离的话「我测过了」测的是另一棵 worktree 的字节。
3. 全程不碰 `main`。

---

## 10. 实施顺序

1. `src/session/reduction.rs` + G1/G2/G3/G4 单测（TDD：先红）
2. `ResumeCoordinator` 接线 + 删除 `classify_markers` / `ScanVerdict` / `compute_boundary_repairs`
3. `ProjectionReconciler` 换推导 + 两处注释改写
4. `subagent_tool::recovery` 加 `progress` + G5
5. `qa/resume_boundary/run.sh` 两阶段；`attribute` 在修复前跑一次记录 FAIL
6. 全量验证集 + 六条守卫逐条变异证伪，记录红的名单
7. 更新 `docs/reference/FEATURE_LOCATOR.md`（§4.13a 增补、附录 E.0 触发器）

---

## 11. 全量验证与守卫证伪记录（Task 7）

### 11.1 最小可信验证集

| 命令 | 结果 |
|---|---|
| `cargo test -p alephcore --lib` | 首次跑 3 红；`git submodule update --init --recursive` 后重跑仅 1 红，且该红与本轮改动无关（原因见下）——17854 passed, 1 failed, 17 ignored |
| `cargo test -p alephcore --bins` | 94 passed, 0 failed（含 `src/bin/aleph-server/commands/start/mod.rs` 的 boot census；`ProjectionReconciler` 构造签名本轮未变，未受影响）|
| `cargo test -p alephcore --features test-helpers --test '*' --no-run` | 编译通过，0 error，0 warning |
| `just _stage-shell-placeholders && cargo clippy --workspace --all-targets` | 0 warning, 0 error（约 6 分钟，`dev` profile 全 workspace）|

`--lib` 首跑 3 红的成因逐条核实：

1. `extension::validation::tests::every_bundled_plugin_passes_the_installers_own_validation`、`gateway::execution_engine::btw_wire_tests::no_shipped_command_word_resolves_as_a_side_question` — 本 worktree 的 `skills/`、`plugins/` 两个 git submodule 未初始化（`git submodule status` 两行都带 `-` 前缀）。两条守卫自己的 panic 文本已经指名根因并给出修法（"this checkout has not initialised them...run `git submodule update --init --recursive`"）。执行后两条转绿，未改任何代码，属本机检出状态问题，非本轮引入。
2. `harness::tests::budget::the_harness_line_budget_does_not_grow` — `src/harness/` 现测得 5246 budgeted lines，超冻结上限（`CEILING`）5233。`git diff $(git merge-base run-reduction main) HEAD -- src/harness/` 为空——本轮六个任务对 `src/harness/` 零改动，`CEILING` 常量与被测行数在本轮开始前即与 `main`（`8bed67331`）完全一致。这条红是从 `main` 继承的既有状态，不是本轮引入；Task 7 不持有修复产品代码的授权（且 R10 禁止改 `src/harness/`），如实上报，不处理。

### 11.2 QA 装置独立复跑

两阶段均由本任务（与撰写夹具的 task6-qa 不同的 agent）独立复跑，均 PASS：

- `crash`：rc=0，`PASS (1 repair text chunk(s) reached the model)`；先行的 `assert-dangling` 自检报 `ok: 1 dangling call(s): ['toolu_2']`。
- `attribute`：rc=0，`PASS (2 repair text chunk(s) reached the model)`（约 4 分钟）；两次 `assert-dangling` 自检分别报 `ok: 1 dangling call(s)`、`ok: 2 dangling call(s): ['toolu_2', 'toolu_3']`。

### 11.3 删除符号消费者计数（修正后的排除表）

Task 7 brief 原排除表有误——把 `ScanVerdict` 当成全仓库唯一的同名类型处理。实际存在两个同名不同物：`src/memory/content_scanner.rs::ScanVerdict`（内存内容扫描，`enum { Clean, Rejected }`）与 `src/skill/guard.rs::ScanVerdict`（技能目录扫描，`struct { level, findings }`，消费者在 `src/skill/mod.rs`、`src/builtin_tools/remember.rs`）。用修正后的排除表复查：

```
grep -rn "classify_markers\|compute_boundary_repairs" src/ tests/ interfaces/ shared/ --include="*.rs"
→ 仅 src/session/reduction.rs 模块文档两处历史提及（有意保留，说明「取代了什么」）

grep -rn "ScanVerdict" src/ tests/ --include="*.rs" \
  | grep -v "src/memory/content_scanner.rs" | grep -v "src/skill/" | grep -v "src/builtin_tools/remember.rs"
→ 零命中
```

无第四个消费者。另用未过滤的 `grep -rln "ScanVerdict" src/ tests/` 核对：命中文件恰好是 `content_scanner.rs`、`skill/mod.rs`、`skill/guard.rs`、`remember.rs` 四个，与排除表逐一对应，排除表完整、没有漏项。

### 11.4 六条守卫的证伪结果汇总

来源：Task 1 Step 6（`task-1-report.md`）、Task 2 Step 5（`task-2-report.md`）、Task 3 Step 6（`task-3-report.md`）、Task 5 Step 6（`task-5-report.md`）、Task 6 Step 6（`task-6-report.md`）。

| 守卫 | 变异 | 预期红 | 实测红 | 相符？ |
|---|---|---|---|---|
| **G1** | `reduce_disposition(&markers)` 换成「有 dangling 就算 `Interrupted{1}`」的捷径（pass 2 提到前面） | `reduce_run_asks_reduce_disposition`（仅此） | 同上 **+** `a_log_with_no_run_marker_attributes_to_earlier_not_this_restart`；proptest 缩到最小反例 `tags=[2]`（`left: Interrupted{1}, right: Clean`） | **否——更强**（1→2）|
| **G2** | provenance 匹配被恒定替换为 `DanglingProvenance::ThisRestart` | 3 条命名测试（`dangling_calls_are_attributed_to_their_own_run`、`a_dangling_call_under_a_finished_run_is_reported_as_earlier`、`a_log_with_no_run_marker_attributes_to_earlier_not_this_restart`） | 同 3 条，无多无少 | 相符 |
| **G2b** | pass 2 只收 `record.seq > anchor` 的悬空，静默丢弃 `EarlierRun` 情形 | `a_dangling_call_under_a_finished_run_is_reported_as_earlier`（仅此） | 同上 **+** `dangling_calls_are_attributed_to_their_own_run`（长度断言 `left:1,right:2`）**+** `a_log_with_no_run_marker_attributes_to_earlier_not_this_restart`（越界 panic，`r.dangling[0]` 在空 vec 上取值）| **否——更强**（1→3）|
| **G3-A** | `EarlierRun` 的引导句被折叠成与 `ThisRestart` 相同的文本 | `repairs_speak_a_different_sentence_per_provenance` 在 "an earlier run in this session" 断言处红 | 同上，panic 位置与断言文本精确匹配 | 相符 |
| **G3-B** | 共享结尾删掉 "side effects" 三词 | 同一测试，`assert_five_points`（当时名为 `assert_four_points`，查四点）红，两臂（`EarlierRun`/`ThisRestart`）均受影响 | 同一测试在第一臂（`EarlierRun`，`c1`）panic；第二臂结构上保证同样失败（两臂共用同一段 `boundary_repair_text` 格式化代码，断言 panic-on-first-failure 未继续跑到第二臂）| 相符（前臂实测 · 后臂结构推断）|
| **G4** | `progress.tool_calls_answered` 的来源换成 `progress.tool_calls_dispatched` | `progress_counts_only_the_current_run`、`answered_never_exceeds_dispatched` | 同 2 条，无多无少 | 相符 |
| **G5-A** | `list_from_log` 在 sidecar 循环前多插一次 `get_events(child_session, ...)`（结果丢弃）| `the_directory_face_reads_only_the_parent_log` | 同上，计数断言 `left:2, right:1` | 相符 |
| **G5-B** | `resolve_forgotten` 末尾的 `progress` 补全循环整段删除 | `the_detail_face_loads_the_childs_progress` | 同上，`progress: None` 未被填充 | 相符 |
| **QA 装置**（`attribute`，跑在 `merge-base(run-reduction, main)=8bed67331` 即修复前的树）| 修复前的 `boundary_repair_text(tool: &str)` 不带 provenance 参数，两次悬空统一说 "the server restarted" | FAIL，"the dangle left by the EARLIER run was blamed on this restart"（§1.4 所述缺陷）| FAIL，文本逐字匹配；`KEEP=1` 复核确认两次悬空读到的都是同一句 "OUTCOME UNKNOWN — the server restarted..." | 相符 |

九行里两行"不符"——**G1** 与 **G2b**——方向完全一致：守卫比预测更强（分别 1→2、1→3），从未出现更弱。这不是缺陷，但也不是"预测对了"：判据 #18 要求任何预期/实测不符都先怀疑守卫而非变异，这里的正确读法是**预测过窄**——当初只想到了一条会被打中的测试，实际还有别的测试同样依赖同一条不变量（G1 的第二条测试覆盖"无 `RunStarted` 时的悬空"边界；G2b 的两条追加测试分别覆盖长度不变量和越界这两种"静默丢弃"的具体表现形式）。九行里没有一行"预期红、实测未红"——判据 #18 最担心的那种伪装成绿的守卫，这一轮没有出现。六条守卫（G1、G2、G2b、G3、G4、G5）连同 QA 装置对 G2/§1.4 的真机复现，均经过了真实的、非恒真的证伪。
