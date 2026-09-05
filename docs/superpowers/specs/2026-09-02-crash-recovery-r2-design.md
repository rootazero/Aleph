# Crash Recovery Round 2 — 投影不再有损 · 日志矛盾 fail-closed · 崩溃时配置回放 · 子 agent 既证事实 · 三张脸

- **日期**：2026-09-02
- **分支**：`worktree-crash-recovery-r2`（worktree `.claude/worktrees/crash-recovery-r2`，基于 main `d0fc03750`）
- **状态**：设计已定（自主会话；用户不在线，所有裁定见 §3 并逐条标注）
- **承接**：`docs/superpowers/specs/2026-08-31-run-reduction-design.md` 的 §8「刻意不做」全部五项——本轮把 A 片（归约内核）之上的 **B 片（投影）、C 片（子 agent）、③（日志矛盾）、④（当时的模型与档位）、CLI/TUI/Panel 三张脸**一次做完。
- **参考项目**：`T:\Github\codex`（Rust）、`T:\Github\pi`（TS）、`T:\Github\deepseek-harness`（TS；用户写的「deepseek-agent」在本机只有此目录与 `DeepSeek-Reasonix`，后者是推理格式项目，故取前者——**假设 A0**）。

---

## 1. 问题（上一轮留下的五个口）

| # | 口 | 上一轮记录 | 本轮实测（worktree @ d0fc03750，逐行读过） |
|---|---|---|---|
| B | 投影有损 | `try_send` 满队列丢行，`Clean` 会话永不回填（§8.1） | **B-D1a** `session_projector.rs:362-369` 只 warn；`projection_reconciler.rs:94-95` 只对 `Interrupted` 会话回填。**B-D1b** 即便 `Interrupted`，水位 = `max(seq)`（L143），水位以下的洞永不补。**B-D1c** `event_retired` 的 `Err` 被读成 `true`（已退休）→ 永久少一行。**B-D3** `stamp_last_assistant_metadata` 按位置不按 seq，丢一条 `AssistantMessage` 后 RunMeta 盖到上一轮气泡上（错标签比缺标签贵，判据 #17）。**B-D4** RunMeta 丢失 = 永远少计费。**B-D1d** drain 是单个无监督 spawn，无 flush / 无重启。 |
| ③ | 日志矛盾静默宽松 | 两个未闭合 run / call_id 重复 / 无主 ToolResult 都按最宽松走（§8.2） | **③-D1** `reduction.rs:153,200` 用**全日志** `HashSet<call_id>` 配对，弱模型复用 call_id 时第二次派发的悬空被第一次的回执遮蔽。**③-D2** `RunStarted` 追加失败是 warn-and-continue（`runner_impl.rs:867-881`），该 run 崩溃后不可检测且悬空被误判 `ThisRestart`。**③-D3** `chat.rewind` / `truncate` 退休事件不平衡 marker，下一次 boot 会 resume 一个用户已回退的 turn。**③-D4** `ToolCallDenied` 对 reducer 不可见——被拒的调用若 `ToolError` 追加失败，会被修成「结果未知、副作用可能已落地」。**③-D5** 两条前置条件都是 `debug_assert`，release 下读成 `Clean`。**③-D7** `ResumeReport` 没有「拒绝」桶，`not_resumed` 把三种原因扇入一个词。**③-D8** recency 看 `RunStarted` 时刻不看最后活动时刻。 |
| ④ | 恢复用的是**现在**的旋钮 | 被恢复的 run 用现在的 model pin / 推理档 / 执行档（§8.3） | **④-D1** `retrigger`（`resume_coordinator.rs:777-791`）`model_override: None`、metadata 不带任何旋钮键 → 五个 `turn_*.rs` 解析器全部落到**当下**会话行 / 全局配置。**④-D2** `select_model` 的「本轮在当前模型跑完」跨崩溃失效。**④-D8** 若经既有 metadata 键送快照，四个解析器会把崩溃时的值**盖回会话行**（stamp-on-carry），撤销用户崩溃后的改动。**④-D6** 快照里的模型可能已退役——直接送上 wire 是 fail-open。**④-D4** `RunStarted.run_id`（本地 uuid）与 `AssistantRunMeta.run_id`（网关 run id）互不指认。 |
| C | 子 agent「做了一半」读不出、重拉起会重做 | 跨 store 词表各说各话（§8.5） | **C1** `resolve_forgotten` 里 sidecar 命中**替换**而非补充日志裁决——生产环境 sidecar 恒开，`RunProgress` / `child_session` 因此对每个中断的后台子 agent **不可达**（上一轮 ⑨ 的字段在真机上没有读者）。**C2** `RunPhase::Settled` 恒标 `"completed"` + 「do NOT re-run」，failed / timed_out / cancelled 一律被说成完成。**C3** 团队任务会话走 `close_delegated_marker` 只关 marker、不修边界（`resume_coordinator.rs:414-422,483-491`），re-dispatch 时 `build_prompt` 静默丢掉孤儿 tool_use，成员**重发一个可能已执行的调用**。**C4** `context=fork` 子的 progress 把父的事件也算进去。**C5** 中断 note 说「已做的都落地了」，而 reducer 算出的 `dangling` 被丢弃。**C6** 崩溃孤儿的团队 run 行没有 summary，恢复段落说「复用已做的工作」旁边是一片空白。**C7** `announced = true` 在投递**前**盖，投递中崩溃通知永久丢失。**C8** 「读 child_session 的转录」指向的是有损投影。 |
| 面 | 三张脸看不见恢复事实 | `agent.resume` 返回体不带进展（§8.4） | **F1** admin `ResumeResponse` 丢 `busy` / `delegated`。**F2/③-D9** CLI 七个状态词渲染五个。**F3** TUI 会话选择器读服务端**从不发送**的 `name` 键——每一行都退化成裸 key。**F4** `sessions.list.state` 崩溃后陈旧 `running` 且零读者。**F5** `agent.resume` 零客户端调用者。**F6** `sessions.list` 行形状在四个 crate 各手抄一份。**F7** Panel 重连唯一输入 `run_concurrency` 是 `json!` 字面量的手抄镜像，键名一漂就把每个活 run 结算成死的。**F8** TUI 丢 role=tool 行，边界修复文本在 TUI 上不可见（有意，保留）。 |

## 2. 参考项目对照（摘要；全文表见扫描产物）

| 维度 | codex | pi | deepseek-harness | Aleph 现状 → 本轮 |
|---|---|---|---|---|
| 悬空调用了结 | 崩溃不补日志，build-prompt 时合成 "aborted"（`context_manager/normalize.rs`） | reducer 只标 `resultExists:false`；provider 边界瞬态 "No result provided" | `interruptedTurnClosers` 按 durable 回执分两句 UNKNOWN / NOT_STARTED，闭合子**续 seq 落盘一次**（`session/repair.ts`） | 已有 durable `ToolError` + provenance 两句（超越三家）。本轮采 dsh：**最近前驱配对** + **Denied 单独一句**；团队会话也修边界 |
| 日志矛盾闭集 | rollout-trace reducer bail-fast（离线诊断，不在恢复路径） | `RecordLogCorruptionReason` 12 成员闭集，`validateRecordLog` 先于 reduce，throw 不修；21 条合法轨迹每个前缀都绿 | 撕尾可截断 / 已提交损坏 fail-closed；`Unsupported ≠ Corrupt` 两类错误 | 零拒绝臂。本轮采 pi 的闭集 + 先验证后归约 + 合法前缀全绿纪律；收窄为 **2 REJECT + 7 REPORT**（多写者日志里 pi 的「凡协议不能产生即拒绝」会拒掉自家 crash-loop 计数 / split / delegated closer） |
| 有效配置回放 | 每个用户 turn 落 `TurnContextItem`，resume 逆序取最新；live 安全策略胜；模型漂移 `Warning` 不静默换 | `EffectiveLaneConfiguration{model,thinkingLevel}` 按 seq 折叠，「在捕获的配置下重试，哪怕用户昨天换了模型」 | 子 agent launch 描述符在自己后缀里 | **完全缺失**。采 pi 的最小集合作 `RunStarted.envelope`；采 codex 的显式请求 > 持久化、validate-then-degrade、exec_tier 只能收紧 |
| 投影水位 / 重连切面 | 客户端协议无 seq；投影是逐行纯折叠可从任意 ordinal 起 | 无 asOfSeq（全仓 0 命中——用户提到的「asofSeq」在三家都不存在，pi 的等价物是 `revision` 戳快照）；reconnect = 新快照 | write-behind **不丢**：合并窗、失败批次按序保留、显式 `flush()` | drain 是三家中唯一「丢即永失」。采 dsh 的不丢 + flush 屏障，采 codex 的「投影可从任意 seq 补洞」。重连面维持快照替换（Aleph 已等价于 pi） |
| 子运行继承事实 | 子线程 resume 时拒绝在不完整前缀上继续 | 无子 agent | 子 = 一份 durable Session，冷恢复按 session id 打开同一日志，已完成调用按身份继承零重跑；描述符从自己后缀（`seedLength` 之后）折叠 | 四套词汇互相遮蔽。采 dsh 的读侧合并（sidecar 只保留 launch 描述符 + 结算文本 + announce 计数；进度 / 在飞 / child_session 一律从子日志派生，own-suffix 仿 `seedLength`） |
| 错误面 | `EventMsg::Warning` | typed in-band `Result`（`LaneBusy` / `NothingToResume` / …）；malformed cursor 拒绝 | `CorruptionError` vs `FormatUnsupportedError` | 无拒绝桶、同一收据三份手写形状。采 pi 的 typed 拒绝 + doctor 面（Aleph 独有） |
| UI 面 | `ThreadResumeResponse` 携带 settings 快照 + 状态 | `suspended[]{reason, missing}` | 未活子显示 'inactive' 永不 terminal | 没有一张脸能说「上一轮被中断——N 次回执已落盘、M 次结果未知」。采 codex 的「resume/attach 响应携带状态」→ `SessionSnapshot.last_run`；采 pi 的 status 词 + unknown→cannot-vouch 读法（`AgentRunStatusReport` 已用此模式） |

## 3. 裁定与假设（用户不在线，每条都可被推翻）

| # | 裁定 | 理由 |
|---|---|---|
| A0 | 「deepseek-agent」= `T:\Github\deepseek-harness` | 本机只有它与 `DeepSeek-Reasonix`；CLAUDE.md 对照表也列 deepseek-harness |
| A1 | ③ 只留两条 **REJECT**（`OutOfOrderSlice`、`NonMarkerInMarkerSlice`），其余 **REPORT** 并带**纠正读法** | 多写者日志（harness / steering / resume / split / compact / backfill 各自分配 seq）下 pi 式全拒会拒掉自家设计形状；一条不改变任何读法的 REPORT 是「报成功的 no-op」（判据 #11） |
| A2 | marker 配对保持**位置**语义，不按 `run_id`；不加 `FinishRunIdMismatch` | Aleph 自家的 closer（`abandoned-*` / `delegated-*` / split）用的就是外来 id；`event_snap.rs` 的按 id 配对是压缩侧的另一份推导，本轮**记录分歧**不改它 |
| A3 | ④ 优先级：model / think / mode / memory = **快照 > 会话 > 全局**（快照经 validate-then-degrade）；**exec_tier = most_restrictive(快照, 当下)**（恢复只能收紧，判据 #14）；崩溃后的 `select_model` pin **被快照压过 + 可见告警** | 日志是唯一真源且 `identity_meta.custom` 无逐键时间戳，「快照胜 + 告警」是**唯一可从日志派生**的规则；codex 同样把 live 安全策略留给现在、把模型漂移做成告警 |
| A4 | ④ 载体：model 走 `RunRequest.model_override`（本来就永不盖回会话），四根 knob 走既有 metadata 键 + **resume 请求跳过 stamp-on-carry** | ④-D8：否则恢复会撤销用户崩溃后为驯服失控 run 所做的改动 |
| A5 | ④ 不动 `RunStarted` 追加失败的 warn-and-continue（③-D2 的写者侧）、不 join 两套 run id（④-D4）、不持久化 MoA（④-D5） | 三者各是一份独立契约；本轮把「没有快照」计成 `ResumeReport.unsnapshotted` 让第一次真机 boot 报出真实规模（判据 #11） |
| A6 | B 不落盘水位列；进程内保留丢失 seq 集合 + boot 按**活动窗口**枚举候选 + doctor 兜底遗留洞 | 上一轮 §8.6 同一理由：落盘缓存是「同一事实第二份表述」的教科书形状；等 QA burst 阶段量到真实洞数再决定 |
| A7 | C 不合并三套词汇（`RunPhase` / `TaskRunStatus` / `JobPhase`），做读侧合并；不加子 agent `resume_from` 动词 | 前者改 on-disk sidecar 形状与 swarm schema（产品裁定）；后者 R7：模型看到既证事实后自己决定怎么重派，给它事实不给它按钮 |
| A8 | 面：只做**显示**，不做 Panel/TUI 的「Resume」按钮；`SessionInfo.state` 保留但 doc 标「生命周期提示，非 run 状态，勿渲染」 | 两者都是产品裁定；F4 的 boot 清扫留给下一轮 |
| A9 | `project_root` 消失时**继续**回退到 agent workspace，但计入 `degraded` 并对模型说一句；FEATURE_LOCATOR L345 改成与代码一致 | 「不静默」是文档要的，「不回退」会让恢复在 mid-tool-call 失败——前者做得到 |
| A10 | `chat.history` 的 `parse_before` fail-open、codex 式 pending approval 重投递、`sessions.list` 成本上限 | 与崩溃恢复不同属，列入 §7 |
| A11 | 真机 QA 夹具用 **Node**（本机无可用 Python，见记忆 `windows-python-heredoc-noop`）；旧的 `crash` / `attribute` 两个 Python 阶段不重写，本轮新阶段全部 `.mjs` | 判据 #18：仪器先怀疑自己——同一 run.sh 里两种语言不是问题，两种断言口径才是 |

## 4. 设计

### 4.1 ③ 日志矛盾闭集（`src/session/reduction.rs`）

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LogContradiction {
    // REJECT — the slice cannot be reduced; consumers must never read `Clean` out of it
    OutOfOrderSlice { at_seq: EventSeq },
    NonMarkerInMarkerSlice { seq: EventSeq },
    // REPORT — reduced with a corrected reading
    UnmarkedActivity { first_seq: EventSeq },            // activity after the last RunFinished with no RunStarted
    FinishWithoutStart { seq: EventSeq, run_id: String }, // info: split / abandon / delegated / retire produce it by design
    DuplicateDispatch { call_id: String, seqs: Vec<EventSeq> },
    ReceiptWithoutDispatch { call_id: String, seq: EventSeq },
    DuplicateReceipt { call_id: String, seqs: Vec<EventSeq> },
    DanglingDeniedCall { call_id: String, seq: EventSeq },
    ClockAnomaly { seq: EventSeq },                       // created_at_ms == 0 or < previous
}
impl LogContradiction { pub fn rejects(&self) -> bool; pub fn tag(&self) -> &'static str /* "session-log-<kind>" */ }

pub struct DanglingCall { call_id, tool_name, turn_id, seq: EventSeq, provenance, denied: bool }
pub struct RunStartFacts { seq: EventSeq, run_id: String, project_root: Option<String>, envelope: Option<RunEnvelopeSnapshot> /* ④ */ }
pub struct RunReduction {
    disposition, run_anchor /* scope: last RunStarted */, run_id,
    open_run: Option<RunStartFacts>,   // the last RunStarted iff no RunFinished follows it — provenance + ④ read THIS
    dangling, progress, contradictions: Vec<LogContradiction>,
}
pub fn validate_slice(events) -> Result<(), LogContradiction>;            // ascending seq (REJECT)
pub fn reduce_disposition(markers) -> Result<RunDisposition, LogContradiction>; // both REJECT kinds replace the two debug_asserts
pub fn reduce_run(events) -> Result<RunReduction, LogContradiction>;          // Err only for REJECT kinds
```

纠正读法（load-bearing）：
- **配对**：单次升序扫描维护 `open: Vec<Dispatch{seq, call_id, answered: Option<seq>, denied}>`；回执答**最近前驱的未答派发**（替换全日志 `HashSet`）。无未答派发 → `ReceiptWithoutDispatch`；最近派发已答 → `DuplicateReceipt`；同 id 仍开着又派发 → `DuplicateDispatch`（两条各自可配对）。修 ③-D1。
- **锚点**：`run_anchor` 仍是最后一个 `RunStarted`（作用域）；新增 `open_run` = 最后一个 `RunStarted` **当且仅当**其后没有 `RunFinished`。provenance：`open_run` 存在且 `seq > open_run.seq` ⇒ `ThisRestart`，否则 `EarlierRun`。修 ③-D2 的误归属；`UnmarkedActivity` 报出那一形状。
- **Denied**：`ToolCallDenied{call_id}` 标 `denied = true`；被拒且无回执的派发仍算悬空，但边界修复文本另出一句「this call was denied by the approval gate and did not run」（生产者与消费者同一提交）。
- **时钟**：`ClockAnomaly` 使 recency 的 age **未知**；`handle_interrupted` 既不 abandon 也不 retrigger → `ResumeReport.skipped_unknown_age`。recency 本身改为 `max(last_marker.created_at_ms, progress.last_activity_at)`（③-D8）——这要求 `handle_interrupted` **顶部归约一次**并把 `RunReduction` 传给 `repair_boundary`（单一推导，判据 #6 / #12）。

消费者：`resume_from_markers` 把 `Err(c)` 记入 `ResumeReport.refused: Vec<(SessionId, ResumeRefusal)>`，`enum ResumeRefusal { LogInconsistent(LogContradiction), AgentMissing, ProviderMissing, BoundaryRepairFailed(String), RetriggerFailed(String) }`（L600-635 的 warn-and-skip 臂全部变成条目）；`status_of` 在 `not_resumed` **之前**判 `log_inconsistent`。`ProjectionReconciler` / B 的修补**不设闸**（它们只要 disposition，被拒的 resume 仍配得上完整转录）。`recovery.rs` 在子日志上把 `contradictions` 与 progress 并列输出。

写者侧（reducer 看不见退休行）：`chat.rewind` 与 `truncate` 共用 `session::marker_balance::close_open_run_after_retire(store, session, running_set)`——`retire_from(seq)` 之后读活 marker 尾，若是开着的 `RunStarted` 且会话不在运行集，追加 `RunFinished{Cancelled, run_id: <that RunStarted's id>}`。

Doctor：`src/diagnostics/checks/session_log.rs` → `core/session-log`，**不可修**：候选 = `load_run_markers()` 的会话；每会话 `reduce_run` → 每种矛盾一条 `Finding::problem` 标 `session-log-<kind>`；另加一条只有 store 面看得见的 rewind 形状查询（活 `RunStarted` 之后有退休的 `RunFinished`）。

### 4.2 边界修复搬家（`src/session/boundary_repair.rs`，NEW）

`repairs_for` + `boundary_repair_text` + `repair_boundary` 从 `resume_coordinator.rs` 搬出：`pub async fn repair_boundary(store: &dyn SessionEventStore, session: &SessionId, reduction: &RunReduction, degrade: Option<&DegradeNote>) -> Result<RepairReport, SessionError>`。纯基于传入的 reduction，靠**重读日志**幂等（第二次调用无悬空 → 0 追加）。`in_flight` 槽留在 coordinator（顶层路径）；团队路径由 reclaim 持任务行的 dispatcher 锁。文本三臂：`ThisRestart` / `EarlierRun` / `denied`，共用尾巴，五个语义要点每臂各查。

### 4.3 ④ 崩溃时配置快照

```rust
// src/session/events.rs — beside project_root; legacy 2-/3-field logs still deserialise
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RunEnvelopeSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")] pub exec_tier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub session_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub think_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub memory_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub model_provider: Option<String>,
}
SessionEvent::RunStarted { run_id, at, project_root, #[serde(default, skip_serializing_if = "Option::is_none")] envelope: Option<RunEnvelopeSnapshot> }
```

字段用 **String**（与 `identity_meta.custom` 同一字面词表、与 `SessionSnapshot` 六键同名），由 census 测试钉住「第七根旋钮必须同时出现在两处」。

- **EMIT**（`runner_impl.rs:867-880`）：该行作用域内已有 `envelope.exec_tier / session_mode / memory_mode`、`think_level`、`routing_directive`（provider 校验**之后**的 (provider, model)——记录实际绑定的，不是校验前的提示）。子 agent / split 生产者传 `None` = 「未解析」；`RunStarted` 构造点 census（`session_split.rs`、`event_snap.rs`、`compact/manual.rs`、`store.rs`、tests）。
- **EXTRACT**：`reduce_run(...).open_run.envelope`（同一锚点，判据 #6）；`resume_coordinator::latest_project_root` 删除，改读 `open_run.project_root`。
- **CONSUME**（`retrigger`）逐槽：model → `validate_snapshot_model(provider, model)`（`lifecycle_for` 退役有继任 → 继任 + `degraded`；无继任或 provider 不可 pin → 落回今日行为 + `degraded`；否则保留）→ `RunRequest.model_override = ModelOverride::from_voice(provider, model)`（永不盖回；崩溃后的 `select_model` 从**下一** run 生效，正是 `select_model.rs:183-184` 承诺的）。exec_tier → `ExecTier::most_restrictive(snapshot, resolve(当下会话, 全局))` 作 `metadata["exec_tier"]`（请求档，非 operator 上限与 channel clamp 仍在其后生效）。session_mode / think_level / memory_mode → 既有 metadata 键。`RunRequest::is_resume()`（从 `inner.rs:1278` / `execute.rs:130` 两处既有判断抽出）让四个 `turn_*.rs` 的 stamp 分支跳过。
- **对模型说**：降级时一句「This run was served by <A>; it resumes on <B> because <reason>」——有悬空修复时附在第一条修复 `ToolError` 上；没有时追加一条 `SessionEvent::SystemMessage`。同一句也用于 `project_root` 回退（A9）。
- `carry_policy_metadata`（续跑孪生）**不**继承旋钮——续跑是新 turn；`resume_metadata` 与它成为同一模块里两份具名 allow-list。

### 4.4 B 投影不再有损

- **drain 不丢**：`MessageProjector.missed: Arc<Mutex<BTreeMap<SessionId, BTreeSet<EventSeq>>>>`（只存 seq，载荷在 SSOT）。`Full` → 记 seq + `projector_deferred` 计数；`Closed` → 记 seq + `error!` + `ensure_drain()` 惰性重启。通道消息 `enum ProjectorMsg { Event(SessionEventRecord), Repair(SessionId, oneshot::Sender<RepairReport>), Flush(oneshot::Sender<()>) }`。
- **一个谓词**：`project_event(store, id, rec, present: &dyn Fn(EventSeq) -> bool, bus)` 替换 `materialized_through: Option<u64>`。drain 传 `|_| false`；修补传 `|s| transcript_seqs.contains(&s)`。`event_retired` 返回 `Result<bool>`；`Err` → seq 进 `missed` 并跳过（fail-closed **且**会重试，修 B-D1c）。
- **修补 = 自愈同一条路**：drain 每处理完一条与收到 `Repair` 时跑 `heal_session(id)`：`transcript_seqs` 来自 `get_history` 的 `parse_source_seq`；`events = load_events_range(id, min(missed)|1, None)`；投影每条不在集合里的。drain 是每会话单写者 ⇒ B-D5 的重复窗按构造关闭。`ProjectionReconciler::reconcile_interrupted` 缩成 boot 驱动：`for id in candidates { projector.request_repair(id).await }`，报告 `ReconcileReport { scanned, holes_filled, stamps_reapplied, usage_rebilled, skipped_up_to_date, errored }`；删 `Clean` 闸与 `skipped_clean`。
- **候选按活动不按 marker**：子会话 `sub-bg-*` 与 cron 会话没有 `RunStarted`（C8）。候选 = `last_active_at ≥ now − resume.max_age_secs` 的会话 ∪ `load_run_markers()` 里 `Interrupted` 的会话。无界扫描交给 doctor `core/projection-holes`（`src/diagnostics/checks/projection_holes.rs`，repairable = true，repair = `request_repair`）。
- **seq 定位的 stamp + 幂等计费**（B-D3/B-D4）：projector 记 `run_start_seq: HashMap<SessionId, EventSeq>`；`stamp_last_assistant_metadata` → `stamp_assistant_metadata_in_range(key, after_seq, before_seq, meta) -> StampOutcome::{Stamped, AlreadyStamped, NoRowInRange}`（SQLite 按 `source_seq` 范围 `ORDER BY source_seq DESC LIMIT 1`；file backend `rfind` 同范围）。`NoRowInRange` ⇒ RunMeta 的 seq 进 `missed`、**不**计费；`update_session_usage` 只在 stamp 把行从 `run_id: None` 变成 `Some` 时跑（一个事务）——重放不重复计费。`messages(session_key, source_seq)` 加索引 `IF NOT EXISTS`。
- **flush 屏障**：`MessageProjector::flush(timeout) -> Result<(), FlushTimeout>`；服务端 shutdown 路径在 drop store 前 await 它。

客户端不动：Panel `hydrate_session_history` 与 TUI `apply_history(Replace)` 都从 `chat.history` 全量重建，投影完整即可。`last_seen_seq` 增量回放列为未来 wire 契约（判据 #10）。

### 4.5 C 子 agent 既证事实

1. **C1 合并不替换**：`Recovered::Sidecar { record, child_session: Option<SessionId>, progress: Option<RunProgress>, in_flight: Vec<InFlightCall{tool_name, call_id, denied}>, contradictions: Vec<LogContradiction> }`；`child_session = background_child_session_key(agent, request_id)`；phase ≠ Settled 的记录从 `reduce_run(own_scope(child_events))` 取 progress / in_flight。enrichment 循环同时匹配 `Interrupted` 与 `Sidecar`；`to_json` 两臂共用 `progress_json` 与 `in_flight_json`（键名一处）。**只在详情面**（`check_status` / `wait`）读子日志，`list` 填 `progress: null`（「没问」）。
2. **C2 标签读 outcome**：`settled_label(record) -> &'static str`（completed / failed / timed_out / cancelled）；`to_json` 的「FINISHED … do NOT re-run」只在 `outcome == completed`，其余渲染「ended without success: <outcome>; result text follows; decide whether to re-run」；`summarize_orphans` 分区与 `check_status` TTL 后同理。`process_journal::JobPhase` 孪生在同一提交回答同一问题（形状统一、词不统一）。
3. **C4 own scope**：`reduction::own_work_start(events) -> usize` = 最后一个 `SessionForked` 之后第一个 `TurnStarted` 的下标，否则 0；`resolve_forgotten` 与 `extract_run_result` 的 `own_turn` 共用（判据 #9）。
4. **C5 说出在飞的调用**：中断 note 改为「Calls that recorded a result have landed. These calls were dispatched with no recorded result — their outcome is unknown: [tool_name…]. Read child_session before deciding whether to spawn the task again.」（provenance 对子日志刻意不渲染——恒 `EarlierRun`）。
5. **C3/C9 委托会话的边界修复归 re-dispatch 的调度器**：`reclaim_orphaned` 在把任务重置为 Pending **之前**对 `SessionKey::task(...)` 调 `boundary_repair::repair_boundary`，再追加 `RunFinished{Abandoned, run_id: <open RunStarted's id>}`。resume 扫描的委托臂改为：会话在**运行集**里 → 跳过并计 `busy`；否则 `repair_boundary` + 关 marker（此臂从此服务 cron / heartbeat——它们没有别的 owner）。团队任务会话只剩一个写者（dispatcher），cron/heartbeat 只剩一个（scan），C9 的「Abandoned 追加在活 RunStarted 之后」按构造不可能。
6. **C6 崩溃行的部分产出**：同一 reclaim 循环里 `summary = fetch_last_reply(task_session)` 或 `RunProgress` 的一行渲染，经 `TaskStore::stamp_abandoned_run_summary(task_id, summary)` 写入；`build_recovery_section` 不改，「partial output (incomplete)」槽从此有值。
7. **C7/C10 announce 是尝试不是回执**：`PersistedRun.announced: bool` → `announce_attempts: u8` + `announced_boot: Option<u64>`（serde default 让旧记录读作 0 次）；`init_and_reconcile` 递增并返回 attempts < 3 的记录；`on_delivered` 给批次里每个 id 盖章；分组 `SubAgentCompleted` 增 `request_ids: Vec<String>`（`shared/protocol/src/events.rs`，判据 #10）；`announce_one` 头改为「N background runs settled while the daemon was down: K finished, M ended without success, J interrupted」，不再对整批下一个 id 的判决。
8. **C8 指针有内容**：`to_json` 带 `child_tail: Vec<{role, text≤400}>` = 子日志最后 3 条 assistant/tool（直接读 `session_events` 的 `load_events_range(head-50, head)`，不读有损投影）。

### 4.6 三张脸（`shared/protocol` 一份形状）

```rust
// shared/protocol/src/session_thread.rs — a VIEW over src/session/reduction.rs; never recomputed client-side
pub struct LastRunState {
    pub disposition: String,               // consts: CLEAN / INTERRUPTED / NEVER_RAN / LOG_INCONSISTENT
    pub run_id: Option<String>,
    pub trailing_starts: u32,
    pub dangling: Vec<DanglingCallView>,   // meaningful only when `inspected`
    pub progress: Option<RunProgressView>,
    pub contradictions: Vec<String>,       // LogContradiction tags
    pub inspected: bool,                   // false = the list face filled only `disposition`
}
impl LastRunState { pub fn disposition(&self) -> LastRunDisposition /* closed enum + Unrecognized */; pub fn dangling(&self) -> Option<&[DanglingCallView]> /* None when !inspected */ }
pub struct DanglingCallView { call_id, tool_name, provenance: String, denied: bool }
pub struct RunProgressView { tool_calls_dispatched: u32, tool_calls_answered: u32, assistant_messages: u32, last_activity_ms: Option<i64> } // key names == recovery.rs keys
pub struct SessionSnapshot { …, #[serde(default, skip_serializing_if = "Option::is_none")] pub last_run: Option<LastRunState> }

// shared/protocol/src/resume.rs (NEW)
pub struct ResumeReceipt { status, session_key: Option<String>, scanned, resumed, abandoned, skipped, busy, delegated, refused: Vec<RefusedEntry{session_key, reason, detail}>, contradictions: u32, degraded: u32, unsnapshotted: u32, skipped_unknown_age: u32, error: Option<String>, agent_id: Option<String> }
impl ResumeReceipt { pub const RESUMED/ABANDONED/ALREADY_FINISHED/NOT_RESUMED/NO_RUNS/DELEGATED/ALREADY_RESUMING/LOG_INCONSISTENT/UNAVAILABLE/FAILED/NOT_FOUND/INVALID_SESSION_KEY/AGENT_FORBIDDEN; pub fn outcome(&self) -> ResumeStatus }

// shared/protocol/src/sessions.rs (NEW) — today's alephcore SessionInfo moved verbatim + last_run
pub struct SessionListRow { …23 fields…, #[serde(default)] pub last_run: Option<LastRunState> }
// shared/protocol/src/metrics.rs (NEW)
pub struct RunConcurrencyMetrics { run_concurrency: u32, running_sessions: Vec<String>, busy_queue: BusyQueueMetrics{ total_waiting: u32, per_session: … } }
```

服务端：`status_of` 返回常量并在 `not_resumed` 前判 `LOG_INCONSISTENT`；`ResumeOutcome::to_json` 与 admin `ResumeResponse` 都换成 `json!(ResumeReceipt::from(&ResumeReport))`（一个构造器，F1 关闭）；`handle_list_db` 构造 `Vec<SessionListRow>`，`last_run{disposition, inspected:false}` 由**一次** `load_run_markers()` 按会话分组填充；`chat.history` 由 `session_snapshot::last_run_from_events(&events)`（包 `reduce_run`；`Err` → `LOG_INCONSISTENT` + tag）填满；`gateway_metrics.rs` 构造 `RunConcurrencyMetrics`；`global_session_event_store()` 为 `None` 时 `last_run: None`（「没问」，永不 CLEAN）。

客户端：Panel `SessionRow` → 解析 `SessionListRow`（`knobs()` 留作扩展 impl）；`hydrate_session_history` 在 `INTERRUPTED` 时推一条 `SystemNoticeRow`「上一轮运行被中断 — {answered}/{dispatched} 次工具回执已落盘，{dangling} 次结果未知」，`LOG_INCONSISTENT` 时「会话日志不一致（{tag}）— 恢复已拒绝，见 doctor」；侧栏 `mode_badge` 旁与 phone `cell-sub` 从 list 面的 disposition 打标；`RunConcurrencyMetrics` 改用共享类型（F7）。TUI `apply_history` 在 `adopt_active_run` 后读 `last_run` 并 `add_system_message` 同一句；`session_entry_from_json` 解析 `SessionListRow`（F3 因读真实 `topic` / `label` 关闭），选择器标签加 `[interrupted]`。CLI `resume.rs` 对 `ResumeStatus` **穷举** match（加变体即编译红，这是想要的棘轮）。

## 5. 验证纪律

- **单测 + 变异证伪**（每条守卫写下「在什么情况下会变红」，并至少变异一次记录红的名单）：
  - ③：14 个矛盾用例 + **合法轨迹每个前缀都绿**（crash-loop / split / abandon / delegated closer / steering mid-gap / fork seed 各一条）；G1 proptest 保留并改为 `Result`；census：`src/` 里没有 `reduce_run(` 后接 `unwrap_or`。
  - ④：`RunStarted` 三代反序列化；`retrigger` 携带快照的集成断言（`tests/resume_coordinator_integration.rs` 今天只断言 `caller_role`）；退役模型 fixture 走 degrade；`most_restrictive` 只收紧；resume 请求不盖回会话行。
  - B：`clean_session_with_hole_is_repaired`（替换 `clean_session_is_skipped`）、水位以下的洞、`event_retired` Err 进 `missed`、RunMeta 落在正确 seq 范围、重放不重复计费、flush 屏障、drain 死后 `ensure_drain`。
  - C：Sidecar 携带 progress/in_flight/child_session；failed 记录不说 completed；fork 子 own scope（经 `fork::seed` 构造，不手写）；reclaim 前先修边界（计数 mock）；announce 三次封顶；`request_ids` 解码。
  - 面：从 `ResumeReport` 构造 `ResumeReceipt` 并断言 JSON 键集 == struct 字段（不是字面量）；`LastRunState::dangling()` 在 `!inspected` 时 `None`；Panel 通知只在 INTERRUPTED 推一条；TUI 三态（absent / null / value）；CLI 穷举编译守卫。
- **最小可信验证集**（CLAUDE.md 六条，本机口径见 plan §0）：`cargo test -p alephcore --lib` 的失败名单与基线 `scratchpad/baseline_failures.txt`（18 条，全部环境/上游）**按名字**比对；`--bins`；`cargo check -p alephcore --features test-helpers --all-targets`；`cargo test -p aleph-protocol -p aleph-tui -p aleph-cli`；`cargo test -p aleph-panel --lib` + `just wasm`；`cargo clippy --workspace --all-targets`（先 `just _stage-shell-placeholders`）。
- **真机 QA**（Node，`qa/resume_boundary/run.sh` 新阶段）：`claims`（崩溃→重启→`chat.history.session.last_run` 经 WS tap = interrupted + dangling 工具名 + progress 数字；`aleph-server resume --json` 每个计数都在；之后 CLEAN）、`denied`（被拒调用另一句）、`rewind`（回退后 marker 平衡，下一 boot 不 resume）、`knobs`（崩溃前 `select_model` + 执行档，重启后 mock 收到的请求用的是快照模型 / 收紧后的档位）、`holes`（压满队列后 run 正常结束，重启后投影补齐且不重复计费）。

## 6. 熵减清单（本轮删除）

`resume_coordinator.rs`：`latest_project_root`、`repairs_for` / `boundary_repair_text` / `repair_boundary`（搬家）、`close_delegated_marker` 的「Only the marker」段、L108-110 / L461-468 的「provider API error on every later turn」句（自 7929bbda6 起为假）。`reduction.rs`：两处 `debug_assert`、全日志 `answered: HashSet`。`projection_reconciler.rs`：`Clean` 闸、`skipped_clean`、`clean_session_is_skipped`。`session_projector.rs`：`materialized_through` 参数与 `rec.seq <= w`、模块 doc 里「the event is dropped from this projection」。`session_store`：两个后端的 `stamp_last_assistant_metadata`。`recovery.rs`：sidecar 替换、「has landed」句。`background_persistence.rs`：`Settled => "completed"` 常量、`announced: bool` 与投递前的 `announced: true` 写。`handlers/resume.rs`：`ResumeOutcome::to_json` 的 `json!`；`admin_api/resume.rs`：`ResumeResponse`；`db_handlers/types.rs`：`SessionInfo`（搬到 protocol）；Panel `api/sessions.rs::SessionRow` 与其自写字面量测试、`api/system.rs::RunConcurrencyMetrics` 镜像；TUI `v.get("name")`；shared/client `session_resolve.rs` 私有 `SessionRow`；CLI `other =>` 兜底臂。说谎文档逐条见 plan 各任务的 **Docs** 项。

## 7. 刻意不做（留给下一轮，附理由）

1. 持久化投影水位列 `sessions.projected_through`（A6，等 `holes` 阶段的数字）。
2. `RunStarted` 追加失败改成 fail-closed 中止 run（A5；写者侧产品裁定）。
3. 两套 run id 的 join、MoA 预设持久化（A5）。
4. 三套子 agent 词汇合并为一份日志派生态；子 agent `resume_from` 动词（A7）。
5. Panel / TUI 的「Resume」按钮（F5）；`SessionInfo.state` 的 boot 清扫（F4）（A8）。
6. `chat.history.parse_before` 的 fail-open；重连时重投递 pending approval / clarify（codex `outgoing_message.rs` 的孪生，§6.9 后续）；`sessions.list` 成本上限（A10）。
7. `last_seen_seq` 增量回放（判据 #10 的 wire 契约，等投影完整之后再谈）。
8. 压缩侧 `event_snap.rs` 的按 `run_id` 配对与本轮位置配对的分歧（A2，只记录）。

## 8. 环境假设

1. 本机 Windows，worktree `D:\Workspace\Aleph\.claude\worktrees\crash-recovery-r2`，共享 `CARGO_TARGET_DIR=D:/Workspace/Aleph/target`（`check` / `--lib` / clippy 可用；`--test '*'` 需 `-j 1` 约 36 min，见记忆 `alephcore-integration-tests-need-j1`）。
2. alephcore lib-test 一次完整编译实测 **16m30s**，超过 Bash 工具 10 min 上限——**必须**用 plan §0 的分离式启动 + Monitor 等待；`CARGO_PROFILE_TEST_DEBUG=line-tables-only` 全程一致。
3. 基线（改动前）`cargo test -p alephcore --lib`：17731 passed / 18 failed / 17 ignored；18 条名单存于 scratchpad `baseline_failures.txt`，与记忆 `windows-alephcore-lib-baseline-2026-09-02` 完全一致。
4. 全程不碰 `main`；本轮**不合并**，只在分支上提交并报告。
