# Crash Recovery Round 2 — Implementation Plan

> **For agentic workers:** 逐任务执行；每个任务由一个 agent 独立完成（读 spec + 本任务段 + 它列出的文件），TDD（先红后绿），自测，提交，报告。任务之间**串行**且有依赖，顺序不可调换。步骤用 `- [ ]` 复选框。

**Goal:** 把上一轮留下的五个口（B 投影有损 / ③ 日志矛盾 / ④ 崩溃时配置 / C 子 agent 既证事实 / 三张脸）一次做完，且不引入第二份推导：所有面都从 `src/session/reduction.rs` 一处派生。

**Spec:** `docs/superpowers/specs/2026-09-02-crash-recovery-r2-design.md`（设计、裁定 A0–A11、熵减清单、刻意不做）。**先读 spec 再读任务**。

**Tech Stack:** Rust（`alephcore` / `aleph-protocol` / `aleph-panel`(Leptos WASM) / `aleph-tui` / `aleph-cli`）、tokio、serde、proptest；bash + Node（`.mjs`）的 `qa/` 真机装置。

---

## 0. Global Constraints & 本机验证配方（每个任务都适用）

- **分支隔离**：全程在 worktree `D:\Workspace\Aleph\.claude\worktrees\crash-recovery-r2`（Bash 路径 `/d/Workspace/Aleph/.claude/worktrees/crash-recovery-r2`），分支 `worktree-crash-recovery-r2`。**严禁触碰 `main`**、严禁 `git checkout main`、严禁在 `D:\Workspace\Aleph` 主检出里编辑。
- **R10**：`src/harness/` 一个字节都不改（`src/harness/agent/prompt.rs` 只读）。**R7**：归约只陈述事实，不替模型做重跑决策。**判据 #10**：wire 键集放进 `shared/protocol` 并用它构造响应，不写 `json!` 字面量。
- **熵减**：每个任务里列出的「删除」项必须真的删掉；删 `pub fn` / 字段的同一笔里跑 `--lib` 测试构建（`cargo check` 看不见 `#[cfg(test)]`）。
- **提交**：英文，格式 `<scope>: <description>`，正文末尾加一行 `Claude-Session: https://claude.ai/code/session_016KwvvvKyN52JZiSRnhyU4S`。每个任务结束时 `git status --porcelain` 为空（只提交本任务的文件；不 `git add -A` 也不 stash/checkout 别人的文件）。
- **注释与标识符英文**；spec/plan 中文。
- **禁止**在 `reduce_run(` 结果上 `unwrap_or` / `unwrap_or_default` / `.ok()` 吞错——`Err` 只有资格说「我不知道」（判据 #8）。

### 本机 cargo 配方（Windows；alephcore lib-test 一次完整编译实测 16m30s，超过 Bash 工具 10 min 上限）

所有 cargo 命令必须带这两个 env（值不可变，否则 fingerprint 失配全量重编）：

```
CARGO_TARGET_DIR=D:/Workspace/Aleph/target
CARGO_PROFILE_TEST_DEBUG=line-tables-only
```

| 目的 | 命令 | 怎么跑 |
|---|---|---|
| 快速类型检查（不含 tests） | `cargo check -p alephcore` / `cargo check -p aleph-protocol` | Bash 前台，timeout 600000（1–4 min） |
| 运行某模块单测 | `cargo test -p alephcore --lib <module::path>` | **分离式**（见下），Monitor 等待 |
| 集成面类型检查（替代 `--test '*' --no-run`） | `cargo check -p alephcore --features test-helpers --all-targets` | 分离式（约 8 min） |
| protocol / tui / cli / bins | `cargo test -p aleph-protocol`；`cargo test -p aleph-tui`；`cargo test -p aleph-cli`；`cargo test -p alephcore --bins` | Bash 前台 timeout 600000 通常够；超时就改分离式 |
| Panel（宿主测试） | `cargo test -p aleph-panel --lib`（harness 在**第一个**失败处中止——用 `-- --skip <name>` 看其余） | 分离式 |
| Panel 出厂形态 | `just wasm`（**必须用 Bash 工具**跑 just，PowerShell 缺 cygpath） | Bash 前台 timeout 600000 |
| clippy | `just _stage-shell-placeholders && cargo clippy --workspace --all-targets`（Windows 上排除 `-p aleph-desktop-macos -p aleph-desktop-linux` 若报错） | 分离式（~6 min+） |

**分离式启动**（PowerShell 工具）——把 `<filter>` 与 `<name>` 换掉：

```powershell
$S='C:\Users\zou\AppData\Local\Temp\claude\D--Workspace-Aleph\b74ab72b-bdba-47f0-8c52-d312539aa909\scratchpad'; $out="$S\<name>.txt"; Remove-Item -Force -ErrorAction SilentlyContinue $out,"$out.done"
Start-Process pwsh -ArgumentList "-NoProfile","-Command","`$env:CARGO_TARGET_DIR='D:/Workspace/Aleph/target'; `$env:CARGO_PROFILE_TEST_DEBUG='line-tables-only'; cargo test -p alephcore --lib <filter> *> '$out'; 'EXIT='+`$LASTEXITCODE | Out-File '$out.done'" -WorkingDirectory 'D:\Workspace\Aleph\.claude\worktrees\crash-recovery-r2' -WindowStyle Hidden
```

**等待**——**前台 Bash 轮询，`timeout: 600000`**，一次最多等 9.5 分钟；没等到就**再调一次同样的命令**，直到 `.done` 出现。**绝对不要用 Monitor 等**：Monitor 要结束本回合才能收到事件，而 Workflow agent 一结束回合就会被强制交最终报告——本轮三个 agent 都是这样被截断的（2026-09-02 实测）。也不要用 `run_in_background` 再 `TaskOutput` 之外的任何"回头再看"方式。

```bash
S=/c/Users/zou/AppData/Local/Temp/claude/D--Workspace-Aleph/b74ab72b-bdba-47f0-8c52-d312539aa909/scratchpad
for i in $(seq 1 28); do [ -f "$S/<name>.txt.done" ] && break; sleep 20; done
[ -f "$S/<name>.txt.done" ] && { echo "DONE $(cat "$S/<name>.txt.done")"; grep -E "^test result:|^error(\[|:)|panicked at|FAILED" "$S/<name>.txt" | head -20; } || echo "STILL RUNNING — call this again"
```

完成后用 Read / `sed -n` 读 `<name>.txt` 看细节。**永远不要 kill 一个在跑的 cargo**（会毁掉增量产物，下一次更慢）；一次只跑一个 cargo（共享 target dir 会串行化，且本机 RAM 撑不住两个 rustc）——启动前 `tasklist //FI "IMAGENAME eq rustc.exe"` 确认没有别的 rustc 在跑。先用 `cargo check` 消灭非测试编译错误，再上分离式测试构建。

**基线失败名单**（改动前，18 条，全部环境/上游）在 `<scratchpad>/baseline_failures.txt`；全量 `--lib` 跑完后用 `comm -3` 按**名字**比对，多出来的才是你的。

---

## File Structure（本轮触碰面）

| 文件 | 职责 | 动作 | 任务 |
|---|---|---|---|
| `src/session/reduction.rs` | `LogContradiction` 闭集 · 最近前驱配对 · `open_run` · `RunStartFacts` · `own_work_start` · `Result` 面 | 修改 | T1, T4, T6 |
| `src/session/boundary_repair.rs` | `repair_boundary` / `repairs_for` / `boundary_repair_text`（三臂）· `DegradeNote` | **新建** | T2 |
| `src/session/marker_balance.rs` | `close_open_run_after_retire` | **新建** | T2 |
| `src/session/mod.rs` | 声明两个新模块 | 修改 | T2 |
| `src/session/events.rs` | `RunEnvelopeSnapshot` + `RunStarted.envelope` | 修改 | T4 |
| `src/gateway/resume_coordinator.rs` | 单次归约 · `refused` 桶 · recency · 委托臂 · 快照消费 | 修改 | T2, T4, T6 |
| `src/gateway/handlers/resume.rs` · `src/gateway/admin_api/resume.rs` · `src/bin/aleph-server/commands/resume.rs` | `ResumeReceipt` 构造 · 穷举 | 修改 | T3 |
| `src/gateway/handlers/chat.rs` · `src/gateway/handlers/session/db_handlers/{modify,query,types}.rs` | rewind/truncate 平衡 marker · `SessionListRow` · `last_run` | 修改 | T2, T3 |
| `src/gateway/session_snapshot.rs` | `last_run_from_events` / `last_run_from_markers` | 修改 | T3 |
| `src/gateway/handlers/gateway_metrics.rs` | `RunConcurrencyMetrics` | 修改 | T3 |
| `src/diagnostics/checks/{session_log,projection_holes}.rs` + `checks/mod.rs` | doctor 两个检查 | **新建** | T2, T5 |
| `src/gateway/session_projector.rs` · `src/gateway/projection_reconciler.rs` · `src/gateway/session_store/{mod,sqlite_backend/mod,file_backend/mod}.rs` · `src/gateway/session_manager/mod.rs` · `src/bin/aleph-server/commands/start/mod.rs` | 不丢 drain · seq 集合修补 · 范围 stamp · flush · boot 候选 | 修改 | T5 |
| `src/orchestrator/harness_bridge/runner_impl.rs` · `src/gateway/execution_engine/{turn_permissions,turn_mode,turn_thinking,turn_memory}.rs` · `src/gateway/execution_engine/mod.rs` | 快照 emit · skip-stamp-on-resume · `RunRequest::is_resume` | 修改 | T4 |
| `src/agents/subagent_tool/recovery.rs` · `src/agents/background_persistence.rs` · `src/agents/subagent_announce.rs` · `src/agents/subagent_spawner/mod.rs` · `src/builtin_tools/process_journal.rs` · `src/teams/dispatcher/schedule/reclaim.rs` · `src/agents/swarm/tasks/store/runs.rs` | C 片 | 修改 | T6 |
| `shared/protocol/src/{session_thread.rs, resume.rs, sessions.rs, metrics.rs, events.rs, lib.rs}` | wire 类型 | 新建/修改 | T3, T6 |
| `interfaces/webchat/src/api/{sessions,chat,system}.rs` · `components/chat_sidebar.rs` · `platform/phone/chat/history.rs` · `platform/wide/views/chat/messages.rs` | Panel 面 | 修改 | T7 |
| `interfaces/tui/src/tui/commands.rs` · `app/mod.rs` · `widgets/session_picker.rs` · `shared/client/src/session_resolve.rs` | TUI / client 面 | 修改 | T7 |
| `qa/resume_boundary/run.sh` · `qa/resume_boundary/drive_r2.mjs` | 真机阶段 claims / denied / rewind / knobs / holes | 新建/修改 | T8 |
| `docs/reference/FEATURE_LOCATOR.md` · `SESSION_KNOBS.md` · `SESSION_SERVICE.md` · `GATEWAY.md` · `MULTI_AGENT_SYSTEM.md` · `qa/README.md` · `CLAUDE.md`(路由表一行) | 文档 | 修改 | T9 |

---

### T1: ③-core — `LogContradiction` 闭集、最近前驱配对、`open_run`、`Result` 面

**Files:** Modify `src/session/reduction.rs`；所有调用者改为处理 `Result`：`src/gateway/resume_coordinator.rs`、`src/gateway/projection_reconciler.rs`、`src/agents/subagent_tool/recovery.rs`、`tests/resume_coordinator_integration.rs`、其它 `grep -rn "reduce_run\|reduce_disposition" src tests` 命中处。

**Interfaces（spec §4.1 原文为准）:** `LogContradiction`（9 变体，`rejects()`、`tag()`）、`DanglingCall{+seq,+denied}`、`RunStartFacts{seq, run_id, project_root, envelope: Option<RunEnvelopeSnapshot>}`（本任务 `envelope` 先用 `Option<()>` 占位**不行**——直接在本任务里于 `events.rs` 加空的 `RunEnvelopeSnapshot` 结构体与 `RunStarted.envelope` 字段（全 `None`），T4 再填字段与 emit；这样 T1 不留死占位）、`RunReduction{+open_run, +contradictions}`、`validate_slice`、`reduce_disposition -> Result`、`reduce_run -> Result`。

- [ ] **Step 1（红）**：先写测试。矛盾用例每种一条（两条 REJECT 各一条断 `Err`；七种 REPORT 各一条断 `contradictions` 含该项**且读法被纠正**：例如 `DuplicateDispatch` 用例断第二次派发**仍是悬空**，`DanglingDeniedCall` 断 `dangling[i].denied == true`，`UnmarkedActivity` 断 provenance 为 `EarlierRun` 与 `open_run.is_none()`）。再写**合法轨迹前缀全绿**测试：六条合法日志（正常 run；crash-loop 两个 RunStarted；`session_split` 形状；`abandoned-<uuid>` closer；`delegated-<uuid>` closer；steering 在工具 gap 中插入 `UserMessage`）——每条的**每个前缀**都 `Ok` 且 `contradictions` 里没有 REJECT 与 `DuplicateDispatch/ReceiptWithoutDispatch/DuplicateReceipt`（`FinishWithoutStart` 在 split/abandon/delegated 形状里允许出现，测试要显式列出哪些形状允许它）。G1 proptest 改为比较 `Result`。census 测试：`grep` `src/`（剥注释行）里 `reduce_run(`/`reduce_disposition(` 之后 5 行内不得出现 `unwrap_or`。
- [ ] **Step 2（绿）**：实现。配对用单次升序扫描的 `open: Vec<Dispatch>`；`open_run` 规则见 spec；`ClockAnomaly` 检测 `created_at_ms == 0 || < prev`。**不要**改 `run_anchor` 的语义（它仍是作用域）。
- [ ] **Step 3**：调用者。`resume_coordinator.rs`：`Err` → 暂时 `warn!` + `skip`（T2 换成 `refused` 桶；本任务只保证编译与行为不倒退）；`projection_reconciler.rs`：`Err` → 计 `errored`；`recovery.rs`：`Err` → `progress: None` 并把 `contradictions` 暂存（T6 渲染）。
- [ ] **Step 4（变异证伪）**：至少三次变异各记录红的测试名：① 把配对换回全日志 `HashSet`；② `open_run` 规则去掉「其后无 RunFinished」；③ REJECT 臂改回 `break`。名单写进提交正文。
- [ ] **Step 5**：`cargo check -p alephcore` → 分离式 `cargo test -p alephcore --lib session::reduction` → 分离式 `cargo test -p alephcore --lib gateway::resume_coordinator gateway::projection_reconciler agents::subagent_tool::recovery`。
- [ ] **Step 6**：改写 `reduction.rs` 模块 doc 与 L54-57 的 `EarlierRun` 段（现在为真）；提交 `session: LogContradiction closed set + nearest-preceding pairing + open-run anchor`。

### T2: ③-writers — 边界修复搬家、`refused` 桶、recency、rewind/truncate 平衡、doctor `core/session-log`

**Files:** Create `src/session/boundary_repair.rs`, `src/session/marker_balance.rs`, `src/diagnostics/checks/session_log.rs`；Modify `src/session/mod.rs`, `src/gateway/resume_coordinator.rs`, `src/gateway/handlers/resume.rs`（`status_of` 增 `log_inconsistent` 臂——字符串常量本任务先放 `handlers/resume.rs`，T3 搬到 protocol）, `src/gateway/handlers/chat.rs`（rewind）, `src/gateway/handlers/session/db_handlers/modify.rs`（truncate）, `src/diagnostics/checks/mod.rs`, `tests/resume_coordinator_integration.rs`。

**Interfaces:**
- `boundary_repair::{repair_boundary(store: &dyn SessionEventStore, session: &SessionId, reduction: &RunReduction, degrade: Option<&DegradeNote>) -> Result<RepairReport, SessionError>, repairs_for(reduction, degrade) -> Vec<SessionEvent>, boundary_repair_text(tool, provenance, denied, degrade) -> String, RepairReport{appended: usize}, DegradeNote{sentence: String}}`。三臂文本：`ThisRestart` / `EarlierRun` / `denied`（「this call was denied by the approval gate and did not run」）；共享尾巴；守卫断**语义**（含否定句 + 工具名）不断字节。
- `marker_balance::close_open_run_after_retire(store, session, is_running: impl Fn(&SessionId)->bool) -> Result<Option<String /*run_id closed*/>, SessionError>`。
- `ResumeReport{+refused: Vec<(SessionId, ResumeRefusal)>, +skipped_unknown_age: usize, +contradictions: usize}`；`enum ResumeRefusal { LogInconsistent(LogContradiction), AgentMissing, BoundaryRepairFailed(String), RetriggerFailed(String) }`。`status_of`：`busy` → `refused 非空且首条 LogInconsistent` → `"log_inconsistent"` → 其余照旧；`not_resumed` 现在只剩「repair 或 retrigger 失败」且两者都已在 `refused` 里有条目。
- `handle_interrupted` **顶部 `reduce_run` 一次**；recency 用 `max(last_marker.created_at_ms, reduction.progress.last_activity_at)`；`ClockAnomaly` ⇒ `skipped_unknown_age`；把 reduction 传给 `repair_boundary`。

- [ ] **Step 1（红）**：`boundary_repair` 的三臂测试（含 denied）；`marker_balance` 测试（开着的 RunStarted + 不在运行集 → 追加 Cancelled；在运行集 → 不追加；尾是 RunFinished → 不追加）；`resume_coordinator` 测试：REJECT 日志 → `refused` 含 `LogInconsistent` 且 `status_of == "log_inconsistent"`；`created_at_ms == 0` → `skipped_unknown_age`；「RunStarted 很老但最后活动很新」→ 被 resume 而非 abandon（这条是 ③-D8 的证伪臂）。rewind 的 handler 测试：回退到 run 中间 → marker 尾 Clean。
- [ ] **Step 2（绿）**：搬家 + 接线。`resume_coordinator.rs` 里删 `repairs_for` / `boundary_repair_text` / `repair_boundary` 本体（保留 `in_flight` 槽与 `ResumeSlot`）。删除 L108-110、L461-468 那句「provider API error on every later turn」（改成「prompt.rs 会降级成文本噪音」的真话）。
- [ ] **Step 3**：doctor `core/session-log`（模板 `src/diagnostics/checks/sqlite_integrity.rs`；**不可修**）：每会话 `reduce_run` → 每种矛盾一条 finding `session-log-<kind>`；另一条 store 面查询「活 RunStarted 之后存在退休的 RunFinished」。注册进 `checks/mod.rs` 的列表并让既有 census（若有）通过。
- [ ] **Step 4（变异）**：① `status_of` 把 `log_inconsistent` 臂挪到 `not_resumed` 之后 → 哪条红；② recency 改回只看 marker → 哪条红；③ `marker_balance` 去掉运行集判断 → 哪条红。
- [ ] **Step 5**：`cargo check -p alephcore` → 分离式 `--lib session:: gateway::resume_coordinator gateway::handlers::resume gateway::handlers::chat diagnostics::checks` → 分离式 `cargo check -p alephcore --features test-helpers --all-targets`。
- [ ] **Step 6**：提交 `session: move boundary repair out of the coordinator; refused bucket, activity-based recency, marker balance on rewind, core/session-log doctor check`。

### T3: faces-wire（服务端半边）— `shared/protocol` 一份形状

**Files:** Create `shared/protocol/src/resume.rs`, `shared/protocol/src/sessions.rs`, `shared/protocol/src/metrics.rs`；Modify `shared/protocol/src/{lib.rs, session_thread.rs}`, `src/gateway/handlers/resume.rs`, `src/gateway/admin_api/resume.rs`, `src/bin/aleph-server/commands/resume.rs`, `src/gateway/session_snapshot.rs`, `src/gateway/handlers/chat.rs`（`chat.history` 填 `session.last_run`）, `src/gateway/handlers/session/db_handlers/{query.rs,types.rs}`（`SessionInfo` → protocol `SessionListRow`；`grep -rn SessionInfo src` 全部改引用）, `src/gateway/handlers/gateway_metrics.rs`。

**Interfaces（spec §4.6）:** `LastRunState` + `LastRunDisposition` + `DanglingCallView` + `RunProgressView`；`SessionSnapshot.last_run`；`ResumeReceipt` + `ResumeStatus`（闭集 + `Unrecognized`）+ `RefusedEntry`；`SessionListRow`（= 今天的 `SessionInfo` 逐字段搬入 + `last_run`；`state` 字段 doc 写「lifecycle hint, not run state; do not render」）；`RunConcurrencyMetrics` + `BusyQueueMetrics`（照 `gateway_metrics.rs:183-191` 的实际键名）。`session_snapshot::last_run_from_events(&[SessionEventRecord]) -> LastRunState`（包 `reduce_run`，`Err` → `LOG_INCONSISTENT` + tag，`inspected: true`）与 `last_run_from_markers(&[SessionEventRecord]) -> LastRunState`（`inspected: false`）。

- [ ] **Step 1（红）**：protocol 测试：`ResumeReceipt::outcome` 对每个常量映射 + 外来词 → `Unrecognized`；`LastRunState::dangling()` 在 `!inspected` 时 `None`；`SessionListRow` 每个字段 `#[serde(default)]`。服务端测试：从**每个计数非零**的 `ResumeReport` 构造 `ResumeReceipt`，断言序列化后的键集 == struct 的 serde 字段名列表（用 `serde_json::to_value` 后取 keys，对照一个由 `schemars` 或手写常量数组给出的字段名清单——不是 `json!` 字面量）；`last_run_from_events` 与 `reduce_run` 的 fixture 数字一致；`sessions_list_row_is_the_protocol_type`（构造 → 序列化 → 用 `SessionListRow` 解析，`topic`/`label` 在）。
- [ ] **Step 2（绿）**：实现；`status_of` 返回 protocol 常量；`ResumeOutcome::to_json` 与 admin `ResumeResponse` 都换成 `serde_json::to_value(ResumeReceipt::from(...))`；`handle_list_db` 一次 `load_run_markers()` 分组填 `last_run{disposition, inspected:false}`；`chat.history` 填满；`global_session_event_store()` 为 `None` → `last_run: None`。CLI `resume.rs` 对 `ResumeStatus` **穷举** match（删 `other =>`），并打印 `refused` 条目与 `degraded`/`unsnapshotted`（后两者 T4 才非零，字段先到）。
- [ ] **Step 3（删除）**：`ResumeOutcome::to_json` 的 `json!`；admin `ResumeResponse`；alephcore `SessionInfo`；shared/client `session_resolve.rs` 私有 `SessionRow` 改用 `SessionListRow`（只解析 `key` / `last_active_at` 的调用点不变）。文档：`admin_api/resume.rs` L33-34、`handlers/resume.rs` L3-5 的「Panel / WS clients」（改成「no client calls it yet; CLI over admin HTTP does」）。
- [ ] **Step 4**：`cargo test -p aleph-protocol`；`cargo check -p alephcore`；分离式 `--lib gateway::handlers::resume gateway::session_snapshot gateway::handlers::session gateway::handlers::gateway_metrics gateway::handlers::chat`；`cargo test -p alephcore --bins`；`cargo check -p aleph-cli -p aleph-tui`（它们依赖 `aleph-protocol`，确认没被搬家弄红；Panel 在 T7 处理，此处只 `cargo check -p aleph-panel --target wasm32-unknown-unknown` 确认能编）。
- [ ] **Step 5**：提交 `protocol: ResumeReceipt / LastRunState / SessionListRow / RunConcurrencyMetrics; server constructs every resume and session face from the shared types`。

### T4: ④ — `RunStarted.envelope` 快照与恢复时回放

**Files:** Modify `src/session/events.rs`（`RunEnvelopeSnapshot` 六字段 + 三代反序列化测试）, `src/orchestrator/harness_bridge/runner_impl.rs`（emit）, `src/session/reduction.rs`（`open_run.envelope` 已在；加 `RunStartFacts` 的测试）, `src/gateway/resume_coordinator.rs`（`retrigger` 消费；`latest_project_root` 删除；`ResumeReport{+degraded, +unsnapshotted}`）, `src/gateway/execution_engine/mod.rs`（`RunRequest::is_resume`）, `src/gateway/execution_engine/{turn_permissions,turn_mode,turn_thinking,turn_memory}.rs`（stamp 分支跳过 resume）, `src/gateway/execution_engine/inner.rs` + `execute.rs`（两处既有 `metadata["resume"]` 判断改调 `is_resume()`；`carry_policy_metadata` doc）, `src/session/boundary_repair.rs`（`DegradeNote` 已有接口，接上）, `src/gateway/handlers/resume.rs` / protocol `ResumeReceipt`（`degraded` / `unsnapshotted` 计数已有字段，接上）, `tests/resume_coordinator_integration.rs`（新增：resume 请求携带快照；退役模型 degrade；exec_tier 只收紧；不盖回会话行）, `docs/reference/SESSION_KNOBS.md`（表下加「崩溃恢复：快照 > 会话 > 全局；exec_tier 只收紧；MoA 未持久化」段）。

**Interfaces:** spec §4.3。`validate_snapshot_model(catalog_view, provider: Option<&str>, model: &str) -> SnapshotModel::{Keep(provider, model), Successor{from, to}, Drop{reason}}`，复用 `select_model.rs:215-236` 的 `lifecycle_for` 与 `pinnable_providers()`——**不要**第二份校验逻辑。`ExecTier::most_restrictive` 已在 `src/config/types/policies/exec_tier.rs:281`，直接用。`RunRequest::is_resume(&self) -> bool`。

- [ ] **Step 1（红）**：`events.rs`：三代 JSON（2 字段 / 3 字段 / 带 envelope）都反序列化；census：`RunEnvelopeSnapshot` 字段名集合 == `session_snapshot.rs` 的六个 knob 键名（从那边导出常量数组，两边共用）。`resume_coordinator` 单测/集成：快照 `{model: "m-old", model_provider: "p", exec_tier: "full"}` + 当下会话 `exec_tier: "ask"` → 请求 `model_override == from_voice("p","m-old")`、`metadata["exec_tier"] == "ask"`（收紧）；快照模型退役有继任 → 继任 + `degraded == 1` + 第一条修复 ToolError 文本含「resumes on」；无快照 → `unsnapshotted == 1`；resume 后会话行的 knob 值**不变**（stamp 被跳过）。`runner_impl` 测试：emit 的 `RunStarted.envelope` 与传入 envelope/directive 一致（找现有 runner_impl 测试的构造方式）。
- [ ] **Step 2（绿）**：实现。`retrigger` 里没有 `open_run`（RunStarted 追加曾失败）→ 今日行为 + `unsnapshotted`。降级句：有悬空 → 附到第一条修复；无悬空 → 追加 `SessionEvent::SystemMessage`。`project_root` 消失 → 同一 `DegradeNote` 机制 + `degraded`（A9）。
- [ ] **Step 3（删除/改写）**：`latest_project_root`；`model_override.rs` L33-42 过时段；`runner_impl.rs` L856-860 注释改成如实陈述两套 id 未 join（A5）；`resume_coordinator.rs` L745-750 doc 改写；`select_model.rs` L183-184 承诺保留并加跨崩溃测试引用。
- [ ] **Step 4（变异）**：① `most_restrictive` 换成取快照 → 哪条红；② stamp 跳过去掉 → 哪条红；③ `validate_snapshot_model` 恒 `Keep` → 哪条红。
- [ ] **Step 5**：`cargo check -p alephcore` → 分离式 `--lib session::events session::reduction gateway::resume_coordinator gateway::execution_engine orchestrator::harness_bridge` → 分离式 `cargo check -p alephcore --features test-helpers --all-targets`（集成测试改动必须过这条）→ 前缀缓存守卫：`grep -rn "prefix_cache\|stable_layer" src/thinker src/context | head` 找到那组测试名后跑它们（判据：per-run-varying bytes 不得进 system prompt——快照只应流入 Dynamic 层）。
- [ ] **Step 6**：提交 `resume: snapshot the effective envelope on RunStarted and replay it (validate-then-degrade, tighten-only exec tier, no stamp on resume)`。

### T5: B — 投影不再有损

**Files:** Modify `src/gateway/session_projector.rs`, `src/gateway/projection_reconciler.rs`, `src/gateway/session_store/mod.rs`（trait：`stamp_assistant_metadata_in_range`、`update_session_usage` 幂等条件）, `src/gateway/session_store/sqlite_backend/mod.rs`, `src/gateway/session_store/file_backend/mod.rs`, `src/gateway/session_manager/mod.rs`（索引）, `src/bin/aleph-server/commands/start/mod.rs`（boot 候选 + shutdown flush；注意 `--bins` 里的 boot census 测试）；Create `src/diagnostics/checks/projection_holes.rs`。

**Interfaces（spec §4.4）:** `ProjectorMsg`；`MessageProjector::{request_repair(id) -> RepairReport, flush(timeout), ensure_drain()}`；`project_event(store, id, rec, present: &dyn Fn(EventSeq)->bool, bus)`；`event_retired -> Result<bool>`；`SessionStore::stamp_assistant_metadata_in_range(...) -> StampOutcome`；`ReconcileReport{scanned, holes_filled, stamps_reapplied, usage_rebilled, skipped_up_to_date, errored}`；boot 候选 = 活动窗口 ∪ Interrupted。

- [ ] **Step 1（红）**：`clean_session_with_hole_is_repaired`（替换并删除 `clean_session_is_skipped`）；水位以下的洞被补（seq 10 缺、11-12 在）；`event_retired` Err → seq 进 `missed` 且下一次 heal 补上；RunMeta 落在 `(run_start_seq, meta_seq)` 范围内最后一条 assistant 行，若该行缺失 → `NoRowInRange` 且不计费；同一 RunMeta 重放两次只计费一次；`flush` 在 drain 清空后返回；drain 任务被 abort 后 `ensure_drain` 重启且期间的 seq 不丢；boot 候选包含无 marker 的 `sub-bg-*` 会话（构造一个只有 `TurnStarted`/`AssistantMessage` 的子会话，活动在窗口内）。
- [ ] **Step 2（绿）**：实现。drain 是每会话单写者；`heal_session` 与 `Repair` 共用；reconciler 变 boot 驱动。索引 `CREATE INDEX IF NOT EXISTS idx_messages_source_seq ON messages(session_key, source_seq)`（file backend 无索引概念，跳过）。
- [ ] **Step 3**：doctor `core/projection-holes`（repairable = true，repair 调 `request_repair`；无界扫描只在这里）。
- [ ] **Step 4（删除/改写）**：`materialized_through` 与 `rec.seq <= w`；两后端 `stamp_last_assistant_metadata`；`Clean` 闸与 `skipped_clean`；`session_projector.rs` L8-25 与 `projection_reconciler.rs` L26-32、`session_store/mod.rs` L80-110 的「永久丢失/只补尾巴」段改写为真话（保留「进程内 `missed` 是进程内存：崩溃后交给活动窗口扫描与 doctor」这一诚实边界）。`docs/reference/SESSION_SERVICE.md` L77 指向不存在的 `src/session/shim.rs`——改正。
- [ ] **Step 5（变异）**：① `present` 谓词恒 `false` → 重复行测试红；② `NoRowInRange` 仍计费 → 幂等测试红；③ boot 候选去掉活动窗口 → 子会话测试红。
- [ ] **Step 6**：`cargo check -p alephcore` → 分离式 `--lib gateway::session_projector gateway::projection_reconciler gateway::session_store gateway::session_manager diagnostics::checks` → `cargo test -p alephcore --bins`（boot census）→ 分离式 `--features test-helpers --all-targets` check。
- [ ] **Step 7**：提交 `projection: never drop — retain missed seqs, heal by seq-set difference, seq-ranged RunMeta stamp with idempotent billing, flush barrier, activity-window boot repair, core/projection-holes doctor check`。

### T6: C — 子 agent 既证事实

**Files:** Modify `src/agents/subagent_tool/recovery.rs`, `src/agents/background_persistence.rs`, `src/agents/subagent_announce.rs`, `src/agents/subagent_spawner/mod.rs`（`extract_run_result` 的 `own_turn` 改用 `own_work_start`）, `src/session/reduction.rs`（`own_work_start`）, `src/builtin_tools/process_journal.rs`（`JobPhase` 孪生：标签读 outcome、`announced` → attempts）, `src/teams/dispatcher/schedule/reclaim.rs`（先修边界、关 marker、stamp summary）, `src/agents/swarm/tasks/store/runs.rs`（`stamp_abandoned_run_summary`）, `src/gateway/resume_coordinator.rs`（委托臂：运行集跳过 + `repair_boundary` + 关 marker），`shared/protocol/src/events.rs`（`SubAgentCompleted.request_ids: Vec<String>` `#[serde(default)]`），docs `MULTI_AGENT_SYSTEM.md` L207-213「three verdicts」→ 四种。

**Interfaces（spec §4.5）:** `Recovered::Sidecar{record, child_session, progress, in_flight, contradictions}`；`settled_label(&PersistedRun) -> &'static str`；`reduction::own_work_start`；`PersistedRun{announce_attempts: u8, announced_boot: Option<u64>}`（删 `announced: bool`；serde default）；`TaskStore::stamp_abandoned_run_summary`；`SubAgentCompleted.request_ids`。**已知基线 flake**：`the_directory_face_reads_only_the_parent_log` 在全量并行跑时偶红（计数 mock 是 `Arc<AtomicUsize>` 局部的，但全量跑时读到 2）——本任务顺手查明原因（怀疑 `resolve_forgotten` 在某条并行路径里被调两次或 `counting_tool` 共享了全局 store），修掉并在提交正文里写明。

- [ ] **Step 1（红）**：Sidecar 命中且 phase=Running 的中断子 → JSON 里有 `child_session` / `progress` / `in_flight_calls`（detail 面）而 `list` 面 `progress: null` 且 `get_events` 只调一次；`outcome: failed` 的 Settled 记录 → note 含「ended without success」不含「do NOT re-run」；fork 子（经 `fork::seed` 真实构造，不手写）的 progress 不含父的调用；reclaim：任务会话有悬空 → 重置前追加了修复 ToolError 与 `RunFinished{Abandoned}`（计数 mock）且 summary 非空；resume 扫描委托臂：会话在运行集 → `busy`，不在 → 修复 + 关 marker；`announce_attempts` 三次后不再返回；`SubAgentCompleted` 带 `request_ids` 解码（protocol 测试）+ 头文本不含单个 id 的判决。
- [ ] **Step 2（绿）**：实现。`child_tail` 直接读 `session_events`（`load_events_range`）。
- [ ] **Step 3（删除）**：sidecar 替换（L434-441）；「has landed」句；`Settled => "completed"` 常量与 L426-428 无条件文本；`announced: bool` 与投递前写；`close_delegated_marker` 的「Only the marker」段；`runner.rs` L517-524「fixed」叙述改写；FEATURE_LOCATOR §4.13a ⑨ 的字段可达性说法留给 T9。
- [ ] **Step 4（变异）**：① enrichment 只匹配 `Interrupted` → 哪条红；② `settled_label` 恒 "completed" → 哪条红；③ reclaim 去掉 `repair_boundary` → 哪条红。
- [ ] **Step 5**：`cargo test -p aleph-protocol`；`cargo check -p alephcore`；分离式 `--lib agents:: teams::dispatcher session::reduction gateway::resume_coordinator builtin_tools::process_journal`；分离式 `--features test-helpers --all-targets` check。
- [ ] **Step 6**：提交 `subagents: recovery merges sidecar with the child log (progress, in-flight, tail), outcome-aware settled labels, own-scope for fork children, boundary repair before team re-dispatch, announce attempts`。

### T7: faces-render — Panel / TUI / client 半边

**Files:** Modify `interfaces/webchat/src/api/sessions.rs`（`SessionRow` → `aleph_protocol::SessionListRow` 解析；`knobs()` 留作 extension trait/impl），`interfaces/webchat/src/api/chat.rs`（history 解析 `session.last_run`），`interfaces/webchat/src/api/system.rs`（`RunConcurrencyMetrics` 改用 protocol），`interfaces/webchat/src/components/chat_sidebar.rs`（`hydrate_session_history` 推 `SystemNoticeRow`；`mode_badge` 旁的 interrupted 徽标），`interfaces/webchat/src/platform/phone/chat/history.rs`（`cell-sub`），`interfaces/webchat/src/platform/wide/views/chat/messages.rs`（若 `SystemNoticeRow` 需要新 kind）；`interfaces/tui/src/tui/commands.rs`（`apply_history` 读 `last_run` → `add_system_message`；`session_entry_from_json` 改解析 `SessionListRow` 读 `topic`/`label` + `[interrupted]`），`interfaces/tui/src/app/mod.rs`（`SessionEntry` doc L412-413）。**先看警告再看错误**（`interfaces/webchat/` 的语义合并冲突是常态形状：`unused variable` 说明那半边没有调用者，正解是 CUT）。

- [ ] **Step 1（红）**：Panel：`history_with_interrupted_last_run_pushes_one_system_notice` / `clean_last_run_pushes_none` / `log_inconsistent_pushes_doctor_notice`；`sidebar_row_shows_interrupted_badge_from_list_face`；`run_concurrency_decodes_the_protocol_type`（替换自写字面量测试——构造 protocol 类型 → 序列化 → 解析）。TUI：`apply_history_emits_interrupted_line`（三态 absent/null/value）、`session_entry_label_uses_topic_not_name`、`picker_marks_interrupted`。
- [ ] **Step 2（绿）**：实现。通知文案（中文）一处一份：Panel 一处、TUI 一处；数字来自 `RunProgressView` 与 `dangling().map(len)`；`!inspected` 时**不**渲染数字。
- [ ] **Step 3（删除）**：Panel `SessionRow` 与 L177-214 字面量测试；`api/system.rs` L70-81 镜像；TUI `v.get("name")`；`api/sessions.rs` L115-121「One decoder, one place」段改写为指向 protocol。
- [ ] **Step 4**：分离式 `cargo test -p aleph-panel --lib`（第一个失败会中止——用 `--skip` 逐个看）；`cargo test -p aleph-tui`；`cargo test -p aleph-cli`；Bash `just wasm`（出厂形态，唯一可信的 Panel 编译）。
- [ ] **Step 5**：提交 `panel/tui: render last_run from the shared types — interrupted notice with landed/unknown counts, sidebar and picker badges; drop the four hand-mirrored row shapes`。

### T8: 真机 QA（Node）+ 全量验证集

**Files:** Create `qa/resume_boundary/drive_r2.mjs`（模仿 `qa/agents_viz/drive_agents_viz.mjs` 的连接/WS tap/mock provider 用法；复用 `qa/lib/build.sh`、`qa/lib/scratch_home.sh`、`qa/busy_input` 的 mock provider 若它是 Node，否则用 `qa/agents_viz` 的）；Modify `qa/resume_boundary/run.sh`（新增阶段 `claims` / `denied` / `rewind` / `knobs` / `holes`；旧 `crash` / `attribute` 保持 Python 不动）；`qa/README.md` 每阶段在证明什么。

- [ ] **Step 1**：读 `qa/agents_viz/run.sh` + `drive_agents_viz.mjs` + `qa/resume_boundary/run.sh` 全文，弄清 mock provider 的请求日志与 kill -9 的做法（`drive_dangle.py --mode send` 轮询事件日志再 kill）。用 Node 复刻 `send`/`kill` 原语。
- [ ] **Step 2**：五个阶段各自的断言全部是**效果**：`claims` = WS tap 上 `chat.history` 的 `session.last_run` 字段值 + `aleph-server resume --json` 的每个计数键 + resume 后 `last_run.disposition == clean`；`denied` = mock 的下一次请求里含「denied by the approval gate」；`rewind` = rewind 后 marker 尾 Clean 且重启后 `resume --json` 报 `already_finished`；`knobs` = 崩溃前 `select_model` 到模型 B、执行档 `full`，会话行改 `ask`，重启后 mock 收到的请求 `model == A（快照）` 且工具审批走 `ask`；`holes` = 单轮内高频工具调用压满 4096 队列（mock 一次返回 5000 个 tool_use 或循环）、日志出现 `deferred`、run 正常结束、重启后 `chat.history` 行数 == `session_events` 中可投影事件数且 `sessions` 行 token 只计一次。
- [ ] **Step 3**：`SKIP_BUILD=0` 构建 release 二进制（`qa/lib/build.sh`；分离式，可能 20+ min）并跑五个阶段；每个阶段的 rc 与关键行写进提交正文；失败的阶段**如实报告**不遮掩。
- [ ] **Step 4（全量验证集）**：分离式全量 `cargo test -p alephcore --lib`（与 `baseline_failures.txt` 按名比对）；`cargo test -p alephcore --bins`；分离式 `cargo check -p alephcore --features test-helpers --all-targets`；`cargo test -p aleph-protocol -p aleph-tui -p aleph-cli`；分离式 `cargo test -p aleph-panel --lib`；Bash `just wasm`；分离式 `just _stage-shell-placeholders && cargo clippy --workspace --all-targets`（Windows 上若 macos/linux desktop crate 报错则 `--exclude` 它们并写明）。
- [ ] **Step 5**：提交 `qa: resume_boundary r2 stages (claims/denied/rewind/knobs/holes) in Node; full verification record`。

### T9: 文档

**Files:** `docs/reference/FEATURE_LOCATOR.md`（§4.13a 增 ⑩–⑯：闭集 / 搬家 / refused / 快照回放 / 委托修复 / 三张脸；§6.9 投影段改写；§4.11 子 agent 恢复段；附录 D 叙事 + 附录 E.0/E.4/E.7 触发器：「sidecar 盖过日志」「按位置 stamp」「先盖 announced 后投递」「会拒掉自家 closer 的闭集」「恢复只能收紧」；L345 与代码一致；L2309/L2310/L2315/L3811/L4043 逐条改），`SESSION_KNOBS.md`（T4 已加段；复核），`SESSION_SERVICE.md`（L38 PK 说法、L77 路径），`GATEWAY.md`（投影段），`MULTI_AGENT_SYSTEM.md`（四种裁决），`qa/README.md`，`CLAUDE.md` 路由表 `src/gateway/` 行加 `qa/resume_boundary/run.sh {claims,denied,rewind,knobs,holes}`，spec `2026-08-31` §8 每项加「→ 2026-09-02 已做」指针。**只在出现新形状时**给 CLAUDE.md 判据索引加行（本轮候选：无——五个形状都落在既有 #3/#5/#8/#9/#14/#16/#17 下，触发器进附录 E）。

- [ ] 逐文件改写；每一处「说谎的文档」都对应 spec §6 的清单；提交 `docs: FEATURE_LOCATOR crash-recovery r2 (projection heal, log contradictions, envelope replay, subagent facts, faces)`。
