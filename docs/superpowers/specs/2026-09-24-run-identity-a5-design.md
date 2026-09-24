# Run 身份单一来源 — 设计规格 (Run Identity, A5 — Design Spec)

**Date:** 2026-09-24
**Branch:** main（单分支开发；实施时每期一个 worktree、各自合并）
**Scope:** `src/orchestrator/{dispatch.rs,harness_bridge/runner_impl.rs}` · `src/gateway/execution_engine/{execute.rs,helpers.rs,gate.rs,session_run_registry.rs,run_loop/}` · `src/context/compact/{session_split.rs,directive.rs,event_snap.rs}` · `src/session/epoch_registrar.rs` · `src/gateway/resume_coordinator.rs` · `src/gateway/session_projector.rs` + `projector_sub/run_span.rs` · `src/gateway/session_store/{mod.rs,sqlite_backend,file_backend}`
**Status:** 已审阅（2026-09-24）。**P1 + P3 实施**（计划 [`2026-09-24-run-identity-a5-p1-p3.md`](../plans/2026-09-24-run-identity-a5-p1-p3.md)）；**P2 延后**（U4）。写计划时核实的结果已回写各节，标 〔amended 2026-09-24 (plan)〕
**Source:** persistence-r3 计划的 FOLLOW-UP F1 · F10 · F14 · F26 · F29 · F30（[`2026-09-12-persistence-wal-recovery-r3.md`](../plans/2026-09-12-persistence-wal-recovery-r3.md) 「FOLLOW-UP 清单」）
**Criteria:** CLAUDE.md 判据 #1 #4 #7 #8 #13 #14 #15 #16 #19

> English summary: A run writes three kinds of records into the session event log — the
> `RunStarted`/`RunFinished` markers, and the `AssistantRunMeta` that bills it — and today they
> carry two different ids that nothing joins: the harness bridge mints its own marker id, the
> engine stamps the meta with `RunRequest.run_id`. Every consumer that joins by id therefore
> misses (the billing idempotence guard re-bills; `snap_out_of_open_run` never pairs across a
> split), and session split makes it worse by minting a third id and moving a live run into a
> child session the engine never claims. This design makes the engine's `run_id` the only id a
> run's markers carry (the bridge receives it; split reuses it), makes the run's claim follow it
> into an adopted child before the child becomes routable, writes the parent span's meta at split
> time and the final meta on the session the run ended on, and turns "stamp, then bill" into one
> store operation so a stamped-but-unbilled row can no longer be produced. Forward-only: logs
> already on disk are read as they are.

---

## 0. 用户裁定 (Rulings, 2026-09-24)

| # | 问题 | 裁定 |
|---|---|---|
| **U1** | 历史日志（id 永不相等的旧 span、F1 在 `027eeb7d9..26c08d6ef` 间翻倍过的 dev/QA home） | **只管往后**。读侧永久容忍两种形状，历史双计不修，写进 RELEASE NOTE |
| **U2** | 两个 id 由谁统一 | **引擎 id 下行**：bridge 与 split 都沿用 `RunRequest.run_id` |
| **U3** | file 后端（出厂默认）两次落盘之间的崩溃窗 | **接受，先戳后计**：只在进程崩溃恰落两写之间时少计一次，方向是少计不重计；写进已知限制与 RELEASE NOTE |
| **U4** | 出厂默认（file 后端）没有 epoch registrar，session split **从不发生**；P2 只服务 `session_store_backend = "sqlite"` | **P1 + P3 现在做，P2 延后**。§3 原样保留为延后设计；触发条件：SQLite 成为默认，或 file 后端接上 registrar。F26 / F29 / F30 随 P2 一起延后 |
| **U5** | 〔amended 2026-09-24 (plan)〕P1 之后合成戳与迟到的 meta 同 id：doctor 修复先合成（只计 token），meta 随后到达读作 `AlreadyStamped` ⇒ 该 run 的 cost / model / 仪表**丢失** | **接受并写明**。token 恰好计一次（F1 关闭的就是重计）；丢的是美元与仪表，只发生在「`RunFinished` 与 meta 之间的一次 append 宽窗口里恰好有人跑 `request_repair`」。进已知限制与 RELEASE NOTE |

---

## 1. 已核实的事实 (Verified facts, measured at `d830bcdb1`)

「核实」列：**读** = 本 spec 作者读过该行；**普查** = 只读代理的普查报告给出、未逐行复读（实施前的 V 项会覆盖其中影响设计的几条）。

| 事实 | 锚点 | 核实 | 对设计的意义 |
|---|---|---|---|
| bridge 自铸 marker id，注释明说「never joined, in either direction」（裁定 A5） | `runner_impl.rs:962-968` | 读 | 这是要删的那一处 |
| `FlowRequest` 没有 run id 字段；生产构造点**只有一处**，测试 10 处 | `orchestrator/dispatch.rs:436-584` · `run_loop/inner.rs:1382` · `orchestrator/tests/dispatch.rs` | 读 | 加字段的成本已知 |
| 引擎 run id 在 `run_loop` 手里（`inner.rs:43` 参数、`:678` 进 `TurnContext`） | `run_loop/inner.rs` | 读 | 下行不需要新的来源 |
| hookstop 自铸 `hookstop-<uuid>`，但手里有 `&RunRequest`；这条路返回 `Ok`，`execute()` 会给它戳一枚**引擎 id** 的 meta | `run_loop/mod.rs:472-478` | 读 / 普查 | 正是会错配的那一种 |
| fast path 自铸 `slash-<uuid>`，不经 `execute()`、不产 meta | `fast_path.rs:34-42` | 读 / 普查 | 唯一保留的自铸（§2） |
| split 铸 `split_run_id` 写成**父的闭**与**子的开**；父的开是 marker id ⇒ 父日志开闭不等 | `session_split.rs:171-202` | 读 | split 两侧按 id 都配不上 |
| split 把父自己的 `RunStarted` 复制进子尾巴（F14），`session_split.rs:1104` 的测试钉住它 | `session_split.rs:1104` | 普查 | 要改写的钉子 |
| `abandon` 写 `abandoned-<uuid>` 的闭，永不配任何开 | `resume_coordinator.rs:1900-1905` | 读 | §2 |
| `already_stamped_by` **按 id** 比：空行或「另一个 run 的 id」答 `false` ⇒ 覆写并再计一次 | `sqlite_backend/mod.rs:648-662`（file 后端 `:1533` 调它） | 读 | F1 双计的机制 |
| `snap_out_of_open_run` **按 id** 配闭与开 | `context/compact/event_snap.rs:52-72` | 读 | F1 条目没点名的第二个按 id 消费者 |
| `collect_run_spans` / `reduce_run` / `run_usage_totals` / `load_retired_run_anchors` 纯按位置 | `run_span.rs:119-180` · `session/reduction.rs:514-581` · `session/usage_fold.rs:61` · `session/store.rs:193` | 普查 | 旧日志继续可读（U1）的依据 |
| `stamp_run_meta` 唯一调用点在 `execute.rs:989`，写 `request.session_key`（父） | `helpers.rs:98-133` · `execute.rs:989` | 读 / 普查 | F26 |
| `SessionRunRegistry::try_claim` 唯一生产调用者是 `gate.rs:107`，按父键；子从不被认领 | `session_run_registry.rs:108` · `gate.rs:107` | 普查 | F30 |
| `SessionEpochRegistrar` 是进程级、boot 注入一次；split 在 `register_epoch(child)` 那一刻让子可路由 | `session/epoch_registrar.rs` · `session_split.rs:231` · `start/mod.rs:394` | 读 | 收养的注入点（§3） |
| projector meta 臂与孪生 `synthesize_missing_stamps` 都是**先戳、后计**，戳是计的幂等闸；`billed` 只活在内存里 | `session_projector.rs:799-850` · `run_span.rs:220-250` | 读 | F10 的机制（§4） |
| `bill_run_from_fold` 的 `cost_usd: None` 走 `unwrap_or(0.0)`：只计 token，不标「未定价」 | `session_projector.rs:960-969` | 读 | 父段 partial meta 可以不带 cost（§3） |
| **出厂默认 session store 是 `file`**；它的戳（transcript JSONL 整文件重写）与计（`metadata.json`）在**同一把** `lock_metadata` 下，但是两个文件 | `start/helpers.rs:218-235` · `file_backend/mod.rs:1398-1424,1488-1545` | 读 | §4 的 file 臂与 U3 |
| 〔amended 2026-09-24 (plan)〕file 分支返回 `(store, None)`：没有 `SessionManager` ⇒ 没有 epoch registrar ⇒ `directive.rs` 的 `(compactor, registrar)` 匹配落到 compact-to-fit——**默认配置下 split 从不发生** | `start/helpers.rs:300` · `start/mod.rs:386-400` · `context/compact/directive.rs` `SplitSession` 臂 | 读 | U4 |
| 〔amended〕引擎 run 内的重试循环每次重建 `FlowRequest` 并重新 dispatch（`MAX_FALLBACK_ATTEMPTS`）⇒ P1 之后**同一会话日志可有多组 `R` 开闭**（判据 #19） | `run_loop/inner.rs:680,1413,1534-1581` | 读 | 按 id 的消费者须取**最近**的一个（§2） |
| 〔amended〕subagent spawner 直接 `AgentHarness::run`，不经 `HarnessRunner`、**不写 run marker**（`recovery.rs` 里的 `RunStarted` 全在 `:894` 起的测试模块） | `agents/subagent_spawner/mod.rs:1002` · `agents/subagent_tool/recovery.rs:894` | 读 | V3 结论：P1 不覆盖、也无须覆盖 |
| 〔amended〕`HarnessRunner` 有 8 个实现者：生产 1（`runner_impl.rs:26`）、测试 7 | `handlers/chat.rs:1991` · `handlers/flow_admin.rs:140` · `orchestrator/tests/dispatch.rs:22,354,479,687` · `tests/gateway_chat_common/mod.rs:127` | 读 | P1 加参数的触及面 |
| 〔amended〕生产里的 `SessionStore` 实现者只有两个后端，没有转发装饰器 | `file_backend/mod.rs:483` · `sqlite_backend/mod.rs:51` | 读 | §4 的方法改名不会被一层默认实现吞掉（判据 #7） |

---

## 2. 设计一：run id 只有一个来源（F1 · F14）

**规则：一个引擎 run 写进日志的每一枚 marker，id 都是 `RunRequest.run_id`。自铸只剩「不是引擎 run」的那一种——没有 meta，就没有东西要 join。**

| 写者 | 现在 | 改成 |
|---|---|---|
| `runner_impl.rs:968` 开 / `:1040` 闭 | 自铸 `run_marker_id` | `FlowRequest.run_id`（新字段，必填 `String`，不给 `Default`——判据 #17 的 `X::default()` 谎言）；删 A5 那段注释 |
| `run_loop/mod.rs:478` hookstop | `hookstop-<uuid>` | `request.run_id` |
| `session_split.rs:171` | 新铸 `split_run_id` | 删；用父 open run 的 id（`reduce_run` 已在 `:118` 算出）——父闭、子开、子闭都是 R |
| split 复制尾巴 | 连父的 `RunStarted` 一起复制 | 滤掉父自己的 opener ⇒ 子里只有 split opener 一个（F14） |
| `resume_coordinator.rs:1902` `abandon` | `abandoned-<uuid>` | 有 open run ⇒ 用它的 id（与 `marker_balance.rs:108` / `boundary_repair.rs:289` 同一推导）；Unanswered 臂没有 open run ⇒ 保留合成 id |
| `fast_path.rs:39` `slash-*` | 自铸 | **不动**；在铸造点写一句为什么它是唯一例外 |

**变真的：** 合成戳与 meta 同 id ⇒ `already_stamped_by` 认得 ⇒ F1 的双计（含 doctor 竞争窗 ①）消失；`snap_out_of_open_run` 在 split 两侧都配得上；`runner_impl.rs:1106` 按 id 找自己的 opener 时不再先撞上复制进来的那个。

**判据 #19——这一笔把什么变成了复数：** 同一个 run id 从此出现在**两个会话**里。已知的按 id 消费者都是会话作用域（`already_stamped_by` 按行、`snap` 与 `runner_impl:1106` 扫单个会话的日志）。**日志之外**以 `run_id` 为键的东西（tracker、`agent.run_*` 事件、Panel、`task_traces`）今天已用引擎 id，不变——但由 V4 普查逐个核对，不靠推断。

**`snap_out_of_open_run` 的一处加固：** 引擎的重试循环（§1）——以及延后的 §3 F29 重开——会让同一日志出现 `R 开 → R 闭 → R 开`；它按 `position` 取**第一个**匹配的 opener，会往回切过头。〔amended 2026-09-24 (plan)〕原稿写「取 cut 之前最近的那个匹配」——对**多个 id** 是错的：cut 之后若闭了 `a` 和 `b` 两个 run，「最近的任一匹配」是 `b` 的 opener，会把 `a` 的 opener 留在 cut 之前被退休。正确的推导：**对 cut 之后闭合的每一个 id，取 cut 之前该 id 的最后一个 opener（它才是那个闭所配的开）；返回这些位置里的最小值**，没有则 `cut`。两个测试：同 id 重用（旧代码回退到 0）、两个 id（钉住朴素 `rposition`）。

---

## 3. 设计二：认领跟着 run 走进被收养的子会话（F30 · F26 · F29）——**延后（U4）**

> 〔amended 2026-09-24 (plan)〕本节不在 P1 + P3 计划内。它依赖的 V1 V2 V5 V8 随之延后。重启时先重读 §1 的 U4 行：触发条件成立之前，split 在默认配置上不可达。

### 3.1 收养（F30）

- 收养在 registry 里是**两步**：
  - `adopt(parent, child)`：父被认领 T 持有 ⇒ 把 child 挂到同一个 T 下（原子）。从这一刻起别人认领不到 child。
  - `confirm(child)`：子的 seed batch **落地之后**才调，T 的「最终会话」指针此时才移到 child。seed 失败 ⇒ `release_adoption(child)` 把它从 T 摘下。
  - 为什么分两步：只有一步的话，要么收养在 seed 之后（收养失败时子里已有一个开着的 R，F29 又在父上重开 R，boot heal 补注册 epoch 后两个会话各有一个开着的 R ⇒ resume 双执行），要么指针在 seed 之前就移动（seed 失败后收尾 meta 写进一个没落地的子）。
- `SessionEpochRegistrar` 加对应的三个方法，默认实现 no-op（`adopt_run` 返回 `NotClaimed`），测试替身不用改。生产实现是 boot 时组合出的包装（registrar + registry），**不按 run 注入、不碰 `FlowRequest`**。
- split 的顺序（契约）：**父 closer（+ 父段 meta，§3.3）→ `adopt_run` → 子 seed batch → `confirm` → `register_epoch`**。子可路由的那一刻（`register_epoch`）必须早已被认领；`register_epoch` 失败仍照今天只记日志、由 boot heal 补注册——子已被认领、run 已在子上，不算 split 失败。
- `adopt_run` 的结果：
  - `Adopted` ⇒ 继续；
  - `NotClaimed`（父本来就没被引擎认领，例如一条不经 `gate.rs` 的路）⇒ 照今天的样子继续，`debug!` 一行——子与父同等待遇，不是退步（判据 #14：问过这个方向）；
  - `Err` ⇒ 不写子 seed，按 split 失败走 §3.4（此时还没有任何子日志）。
- 收益：`is_running` 的三个读者**不改一行就变真**——准入闸、resume 协调器的 `defer_if_running`（F28 的 check-then-act 窗在这里关上）、`chat.clear` 的 `retire_from_and_close_run`（`handlers/mod.rs:181-184`）。
- 再 split（child1 → child2）：child1 已挂在 T 下 ⇒ `adopt(child1, child2)` 照常成立。

### 3.2 释放盖住每一把被收养的键

`RunSlot` 释放 T 时释放挂在 T 下的**每一把**键，并**唤醒每把键的 busy lane**。只释放不唤醒 = 子上排队的消息永远停着（判据 #7 × #13）。测试断言的是「子上排队的那条消息**跑起来了**」，不是「release 被调用了」（判据 #4）。

### 3.3 meta 落到哪里（F26）

- **父段：** split 写父 closer 的**同一批**里追加 `AssistantRunMeta { run_id: R, cost/model/gauge: None }` ⇒ 父的 split 前 token 在写入时计费；同一批 ⇒ 位置上不可能被别的 run 插进来锚错。
- **收尾：** `execute.rs:989` 不再写 `request.session_key`，改问 registry「T 最后收养的键」⇒ 写到 run 结束时所在的会话，带整个 run 的 cost / model。
- 这个事实**只有一个来源**（registry）。不加 `FlowOutcome.final_session`——那会和 `harness.final_session_id()` 成为两份表述（判据 #1）。
- 结果：两段都在写入时计费，token 各按自己的 fold 计一次，cost 只计在最终会话上一次；heal 从此只为真正的崩溃合成。

### 3.4 子 seed 失败（F29）

- 父 closer 仍然**先**写（`session_split.rs` 现有注释给出了理由：反过来，崩在两写之间会留下两个开着的 R，resume 会双执行）。
- 触发条件：`adopt_run` 返回 `Err`，或子 seed batch 失败（后者先 `release_adoption(child)`，所以「最终会话」指针从没移动过）。
- 回退 compact-to-fit **之前**，在父上重写 `RunStarted { run_id: R, envelope, project_root }`（取自 `reduce_run` 已算出的 open run）⇒ 余下部分重新有 marker、可 resume。
- 重开这一写也失败 ⇒ 退回今天的安全方向（`UnmarkedActivity`，不双执行），`warn!`。
- 没有收养发生 ⇒ 收尾 meta 自然落在父、锚到重开的 opener，与 §3.3 一致。

### 3.5 这一节让什么更难

registry 从「一键一认领」变成「一个认领多把键」——又一次加宽（判据 #19）。所有把「键 ↔ 认领」当一一对应的地方由 V5 普查：`running_sessions()`、释放、`try_claim` 的冲突判断、诊断输出。

---

## 4. 设计三：「已戳未计」不再能产生（F10）

**原则：不加修复路径，让这个状态无法被产生。** 已经卡在这个状态的历史行按 U1 不修。

1. **先 fold，后动手。** `bill_run_from_fold` 的 fold 读取挪到戳**之前**；读失败 ⇒ `Retry`，什么都还没写。事件只追加，`(run_start, meta_seq]` 在读时是固定的，先读是安全的。
2. **`SessionStore::stamp_and_bill_in_range`** 取代两个孪生调用点（projector meta 臂、`synthesize_missing_stamps`）里的两步写（判据 #16：一个操作，两处共用）：
   - **SQLite：** 一个事务；任何一步失败整体回滚 ⇒ `Retry`。严格的恰好一次。
   - **file（出厂默认）：** 在它已持有的那一把 `MetaGuard` 下先重写 transcript（戳）、再 commit `metadata.json`（计）；计费出错 ⇒ 不写 transcript ⇒ `Retry`。
   - 〔amended 2026-09-24 (plan)〕V6 结论：`stamp_assistant_metadata_in_range` 的生产调用者**恰好**是这两个孪生点 ⇒ 不新增方法，而是**把它改名为 `stamp_and_bill_in_range` 并加一个 `bill: Option<&RunBill>` 参数**（一个操作一个名字，不留旧名做死孪生）。`update_session_usage` 在本计划后没有生产调用者（`handlers/projects_channel.rs:1102` 是测试替身），但它有 6 个实现者；它的 SQL 抽成两处共用的 helper，trait 方法本身的去留**不在本计划内**（P6 另议）。
   - 〔amended〕file 臂：计费（`metadata.json`）写失败时，**仍在同一把锁下**把行的旧 metadata 写回 transcript，再返回 `Err` ⇒ `Retry`。为此 `MetaGuard` 加一个不消耗 guard 的 `write_back(&mut self)`（`commit(self)` 会随返回放锁）。回滚本身也失败 ⇒ `warn!`，这一行是「已戳未计」——与 U3 的崩溃窗同一方向。
3. **拆掉两义的 bool**：`Projected::Stamped { bill: BillOutcome }`，`BillOutcome::{Billed, NothingToBill, Unfoldable}`。〔amended〕`Unfoldable` 取代原稿的 `Unanchored`，覆盖两种「无从 fold」：meta 前面没有 `RunStarted`（例如 opener 已被 `/compact` 退休），或没有装 event log（`ProjectionCtx.events == None`）。照旧只戳不计，但**报出来**、`warn!`，不再默默算作「没计」（判据 #8）。fold 的**读失败**不属于它——那是 `Retry`。`RepairReport.usage_rebilled` 只数 `Billed`。

**已知限制（U3）：** file 后端的两个文件各自原子写，彼此之间不原子。先戳后计 ⇒ 进程崩溃恰落两写之间时该 run 少计一次，永不重计。瞬时错误那一类已被第 2 条关掉。

---

## 5. 非目标 (Out of scope)

- 历史日志的迁移、重写或双计撤销（U1）。
- 把 session 用量改成从戳求和的派生值（U3 否决的第三个选项；如要做，单独一轮）。
- fast path 进入引擎 run 身份。
- 非 SQLite 的 `SessionEventStore` 实现者须覆写 `load_retired_run_anchors`——trait doc 已写明，生产里只有 SQLite 一个实现者；本轮不动。
- F31（terminal/pty 谓词不对称）、F27（晚趟重注入等最慢 run）、Phase T 承接的两个产品缺陷。
- `src/harness/` 不在范围内：`final_session_id()` 不改，`budget.rs::CEILING` 不应变化（V7 实测确认）。

---

## 6. 实施前必须先核实的 (V items — 每条带结论写回本节)

| # | 问题 | 为什么影响设计 |
|---|---|---|
| **V1** | split 之后，发给**父键**的 steer / busy 消息落进哪份日志（Panel 这类显式键的调用方会继续寻址父） | §3.2 的唤醒对象、以及父上是否会出现第二个 run |
| **V2** | §3.3 父段 partial meta 的 `turn_id` 取哪一个；meta 臂与 fold 是否读它 | partial meta 的形状 |
| **V3** | subagent 的 run 走不走 `runner_impl`；走的话它们拿到的是哪个 `RunRequest.run_id` | §2 的覆盖面 — 〔amended〕**已结**：不走，也不写 marker（§1） |
| **V4** | 日志之外以 `run_id` 为键的全部位置（tracker、`agent.run_*`、`task_traces`、Panel、busy queue durable） | §2 的判据 #19 普查 — 〔amended〕计划 Task 1 执行，结论写回这里。**已结**（Task 1，1d678e010）：生产写者 6、按 id 读者 2、日志外无 marker id 键；表见计划 Task 1 |
| **V5** | registry 里「键 ↔ 认领一一对应」的全部假设 | §3.5 的判据 #19 普查 |
| **V6** | `update_session_usage` / `stamp_assistant_metadata_in_range` 的全部调用者（剥掉测试模块） | §4 第 2 条的删改范围 — 〔amended〕**已结**（§4 第 2 条） |
| **V7** | 改动后 `harness/tests/budget.rs` 实测值 | §5 的「不动 harness」 — 〔amended〕计划 Task 1 量基线、Task 8 / Task 12 复量。**已结**（Task 1，1d678e010）：基线实测 5266 / `CEILING` 5266（`the_harness_line_budget_does_not_grow` PASS，零余量；测试通过时不打印总数，按 `budgeted_lines` 口径逐文件手算得出，见计划 Task 1） |
| **V8** | registry 用 `SessionKey`、split 用 `SessionId`——`adopt_run` 的生产实现在哪一层做转换 | §3.1 的接口形状 |

---

## 7. 测试 (Test plan)

**新钉子（全部断言效果到达，判据 #4）：**
- 一个 run：`RunRequest.run_id` = 日志里的 `RunStarted.run_id` = `RunFinished.run_id` = `AssistantRunMeta.run_id`——从**日志读回**断言，不断言调用参数。
- 合成戳先落、meta 后到 ⇒ session 用量只增一次（F1 回归）。
- split：父闭、子开、子闭三者同 id；子里只有一个 opener（F14）；`snap_out_of_open_run` 在两侧都配对。
- split 后 run 仍在子上：发往子的新消息**排队**而不是开第二个 run（F30），run 结束后**那条消息跑起来了**（§3.2）；resume 协调器对子答 busy。
- split 后：父段与子段在 run 结束**之前**就各自计了 token，cost 只在子上计一次（F26）。
- 子 seed 失败：父日志 `R 开 → R 闭 → R 开`，崩溃重放读作可 resume（F29）；`snap` 取最近的 opener；收尾 meta 落在**父**上（指针没被移动过）；子键已从 T 摘下。
- `adopt_run` 失败：不存在任何子日志，父上同样重开 R。
- `stamp_and_bill_in_range`：两个后端上，计费一步注入失败 ⇒ 行**未**被戳、下次重放计费成功（F10）；`NothingToBill` / `Unfoldable` 各有一例。

**要改写（不是删除）的旧钉子：** `session_split.rs:933-946`（`split_run_id`）、`:1104`（复制的 opener）；`run_loop/mod.rs:948`（`hookstop-` 前缀）；断言 `Projected::Stamped { billed }` 的**四**处（`session_projector.rs:1663` `:1841` `:1911` `:1964`——〔amended〕原稿数成三处，漏了 `an_unanchored_meta_stamps_but_does_not_bill`，它变成 `Unfoldable`）。

〔amended 2026-09-24 (plan)〕原稿把 `session_projector.rs:2132` `:2152` `:2179` `:2276` 列为「要改写」——**错了**。它们钉的是 marker ≠ 引擎 id 的日志仍被正确读取，而 U1 让旧日志原样可读，所以它们**必须保持绿**；只改它们的注释（「生产的 id 形状」→「P1 之前写下的日志形状」），并在旁边加一个同 id 的新形状孪生。`projection_reconciler.rs:1374` `:1436` 同理保持绿。

**变异检查：** 每期完成后，把该期的关键一笔撤掉（如：bridge 恢复自铸、`adopt` 改成 no-op、`stamp_and_bill` 拆回两步），确认红的**恰好**是预期那几条（判据 #18）。

**验证集：** 最小可信验证集七条；`qa/resume_boundary/run.sh` 的相关阶段；`just test-shared` 不在范围（不改 `shared/` `interfaces/`，V4 若发现 Panel 以 run_id 为键且受影响则加回）。

---

## 8. 分期 (Phases)

| 期 | 内容 | 依赖 |
|---|---|---|
| **P1** | §2：`FlowRequest.run_id`、bridge / hookstop / split / abandon 统一、F14 过滤、`snap` 最近匹配 —— **已落地 `ef4902932`** | V3 V4 |
| **P2** | §3：`adopt`、多键释放与唤醒、父段 partial meta、收尾 meta 落最终会话、F29 重开 —— **延后（U4）** | P1（重开沿用 R）· V1 V2 V5 V8 |
| **P3** | §4：`stamp_and_bill_in_range` 两后端、`BillOutcome` —— **已落地 `7244c8b2f`** | 与 P1/P2 无代码依赖，可并行或先做 · V6 |

每期一个 worktree、各自合并；每期结束跑一次变异检查。

---

## 9. RELEASE NOTE（草稿）

- 一个 run 的开始 / 结束标记与计费记录从此使用同一个 run id；旧会话日志照原样读取，不迁移。
- 跨越升级的那一个 run（旧标记、新计费记录）可能仍被多计一次；此前在 `027eeb7d9..26c08d6ef` 之间被翻倍计费的 dev / QA 数据不会被修正。
- 上下文分裂（session split）之后，新会话在该 run 结束前不再接受并发的第二个 run；两段对话的用量在 run 进行中即入账。
- 默认的 file 会话存储：进程若恰好在「标记」与「入账」两次写盘之间崩溃，该 run 的用量会少计一次（不会重复计）。
- 〔amended 2026-09-24 (plan)〕若 `doctor` 的修复恰好在一个 run 结束与它的计费记录落盘之间运行，该 run 的 token 只计一次，但它的费用与模型名不会入账（U5）。
- 〔amended〕上下文分裂只在 `session_store_backend = "sqlite"` 下发生；上面第三条（分裂后的排队与入账时点）随 P2 延后，本次发布**不含**。
