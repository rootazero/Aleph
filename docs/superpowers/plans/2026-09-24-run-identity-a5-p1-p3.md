# Run 身份单一来源 P1 + P3 实施计划 (Run Identity A5 — P1 + P3 Implementation Plan)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 一个引擎 run 写进会话日志的 marker 与 meta 用同一个 run id（P1），并让「戳」与「计费」成为一个存储操作，使「已戳未计」无法产生（P3）。
*A run's markers and its billing meta carry one id (P1); stamp and bill become one store operation, so a stamped-but-unbilled row cannot be produced (P3).*

**Architecture:** P1 把引擎的 `RunRequest.run_id` 经 `FlowRequest.run_id` → `HarnessRunner::run(.., run_id)` 下行到 harness bridge，bridge / hookstop / split / abandon 全部沿用它；按 id 的读者（`snap_out_of_open_run`）改成「每个闭配 cut 之前同 id 的最后一个开」。P3 把 `SessionStore::stamp_assistant_metadata_in_range` 改名为 `stamp_and_bill_in_range(.., bill: Option<&RunBill>)`：SQLite 一个事务，file 后端在同一把 `MetaGuard` 下先写 transcript、再写 `metadata.json`，失败则回滚戳；投影器先 fold 再戳，`Projected::Stamped { bill: BillOutcome }` 取代两义的 `billed: bool`。

**Tech Stack:** Rust (tokio, rusqlite, serde_json, async-trait) · `alephcore` crate · Node QA 装置 `qa/resume_boundary/`

**Spec:** [`docs/superpowers/specs/2026-09-24-run-identity-a5-design.md`](../specs/2026-09-24-run-identity-a5-design.md)（§2 = P1，§4 = P3，§3 = P2 **延后**，U4）

## Global Constraints

- 裁定 **U1**：只管往后——读侧永久容忍「marker id ≠ meta id」的旧日志；历史双计不修。钉住旧形状的测试**保持绿**，只改注释。
- 裁定 **U2**：引擎 id 下行——bridge 与 split 都沿用 `RunRequest.run_id`。`FlowRequest.run_id` 是必填 `String`，**不给 `Default`**（判据 #17）。
- 裁定 **U3**：file 后端先戳后计；崩溃恰落两写之间 ⇒ 少计一次，永不重计。
- 裁定 **U4**：P2（§3：adopt / F26 / F29 / F30）**延后**，本计划不碰 `SessionRunRegistry`、`SessionEpochRegistrar`、`execute.rs` 的 meta 落点。
- 裁定 **U5**：合成戳先落、同 id 的 meta 后到 ⇒ token 计一次，cost / model / 仪表丢失——接受并写明。
- `fast_path.rs` 的 `slash-*` 自铸 id **不动**（唯一例外，只加一句注释）。
- `src/harness/` 不改；`harness/tests/budget.rs::CEILING` 不应变化（V7）。
- 回复中文、代码注释英文、文档中英双语；commit 英文 `<scope>: <description>`，**无** attribution trailer。
- 单分支开发：每期一个 worktree（`worktree-run-identity-p1` / `worktree-run-identity-p3`），各自 fast-forward 合并进 main；会话内**只合并不删除** worktree。
- **CRLF**：`src/` 下被改的 `.rs` 文件与 `qa/resume_boundary/drive_r2.mjs` 都是 CRLF。只用 Edit 工具改；**禁止** `cargo fmt -p alephcore`、对 `mod.rs`/`lib.rs` 裸跑 `rustfmt`、MSYS `sed -i`。格式检查用 `rustfmt --edition 2021 --check <file>`（只读）。每次提交前：`for f in $(git diff --name-only -- '*.rs' '*.mjs'); do echo "$f $(tr -cd '\r' < $f | wc -c) $(wc -l < $f)"; done`，两数应相等。
- 基线按**名字**比，不按条数。本机已知 `--lib` 基线红（2026-09-24）：`capability::census::every_installed_global_is_a_capability_slot` · `executor::builtin_registry::definitions::tests::catalog_description_bytes_ratchet` · `browser::page_state::build::tests::the_thirteen_node_fixture_renders_the_golden_tree` · `gateway::event_scope::tests::every_node_topic_the_center_publishes_is_refused_to_a_member`。
- 真机 QA 跑在 clippy **之前**（`clippy --workspace` 会清掉 server 二进制）；任何 `.rs` 改动之后 `SKIP_BUILD=1` 无效。
- 改 `session_projector.rs` / `projection_reconciler.rs` / `usage_fold.rs` 的那一期，review 前必须真机跑 `qa/resume_boundary/run.sh holes knobs claims`（FL 规则）。
- RELEASE NOTE 文字写进每期最后一个 commit 的 body（CHANGELOG 在发版时由 git log 生成）。
- cargo 命令：`CARGO_TARGET_DIR=D:/Workspace/Aleph/target CARGO_PROFILE_TEST_DEBUG=line-tables-only`（共享 target 让 `--lib` 快）；`--test '*'` 用 worktree 自己的 target 且 `-j 1`（共享 target 会让它报假 E0463）。`--lib` 全量构建约 13 分钟，超过 Bash 10 分钟上限——用 `run_in_background` 跑、日志落文件、日志第一行写命令本身；**不要** `cargo test | grep` 管道（泄漏的 probe server 会挂住管道）。

## Review Focus

1. **引擎重试循环在一份日志里留下多组同 id 的 `R 开 → R 闭`**（`MAX_FALLBACK_ATTEMPTS`，spec §1）——期望：每组各计一次，两次 repair 不再加任何 token；`snap_out_of_open_run` 配的是 cut 之前**最后**一个同 id opener。钉在 Task 6（snap）与 Task 7（`a_retried_run_bills_each_bracket_once`）。
2. **同一会话里新旧形状混排**（升级前的 run：marker ≠ meta id；升级后的 run：同 id）——期望：两个 run 各计一次、各自的行带各自 meta 的 id，repair 不加 token。钉在 Task 7（`a_legacy_run_and_a_same_id_run_in_one_log_are_each_billed_once`）。
3. **要计费、但会话行 / `metadata.json` 不存在**——期望：返回 `Err`，行**未**被戳（否则戳成了永久的「已计」闸），下次重放在行恢复后照常计费。钉在 Task 9（SQLite）与 Task 10（file）。
4. **meta 到达时它的 opener 已被 `/compact` 退休**（fold 无锚）——期望：行被戳、不计费、`warn!`、heal 报告 `stamps_reapplied 1 / usage_rebilled 0`，而不是默默算「没计」。钉在 Task 11（`a_heal_counts_an_unfoldable_stamp_as_not_rebilled`）。
5. **`stamp_and_bill` 之后重放同一 meta**——期望：`AlreadyStamped`，会话计数不变，两个后端一致。钉在 Task 9 与 Task 10 的第一个测试。

---

## 文件地图 (File Structure)

| 文件 | 期 | 责任 |
|---|---|---|
| `src/orchestrator/dispatch.rs` | P1 | `FlowRequest.run_id` 字段 + Debug；`HarnessRunner::run` 末尾参数 `run_id`；`dispatch()` 转发 |
| `src/gateway/execution_engine/run_loop/inner.rs` | P1 | 唯一生产构造点填 `run_id` |
| `src/orchestrator/harness_bridge/runner_impl.rs` | P1 | marker 用传入的 id；扫描注释 |
| `src/orchestrator/harness_bridge/tests.rs` | P1 | bridge 从日志读回 marker id 的测试 |
| `src/orchestrator/tests/dispatch.rs` · `src/gateway/handlers/{chat,flow_admin}.rs` · `tests/gateway_chat_common/mod.rs` · `tests/orchestrator_e2e.rs` | P1 | 7 个测试替身加参数、13 处测试字面量加字段、一条转发测试 |
| `src/gateway/execution_engine/run_loop/mod.rs` | P1 | hookstop 用 `request.run_id` |
| `src/context/compact/session_split.rs` | P1 | 沿用父 open run 的 id；滤掉尾巴里父自己的 opener（F14） |
| `src/gateway/resume_coordinator.rs` · `tests/resume_coordinator_integration.rs` | P1 | `abandon` 以 open run 的 id 闭 |
| `src/context/compact/event_snap.rs` | P1 | 每个闭配最后一个同 id 开，取最小 |
| `src/gateway/execution_engine/fast_path.rs` | P1 | 一句注释：唯一保留的自铸 |
| `src/gateway/session_projector.rs` · `projector_sub/{run_span,missed_seqs}.rs` | P1（测试 + 注释）· P3（逻辑） | 消费侧钉子；fold 先行、`BillOutcome` |
| `src/gateway/session_store/mod.rs` | P3 | `RunBill`、trait 方法改名加参 |
| `src/gateway/session_store/sqlite_backend/mod.rs` · `src/gateway/session_manager/ops/{mod,modify}.rs` | P3 | 事务化戳 + 计；`add_usage` 共用 SQL |
| `src/gateway/session_store/file_backend/{mod,meta}.rs` | P3 | `MetaGuard::write_back`、测试 failpoint、回滚 |
| `qa/resume_boundary/drive_r2.mjs` · `qa/README.md` | P1 | `holes` 加一条「meta 的 id = 它前面 opener 的 id」真机断言 |
| `docs/reference/FEATURE_LOCATOR.md` · spec · r3 计划 FOLLOW-UP | P1 · P3 | 文档与 FOLLOW-UP 收口 |

---

# Phase P1 — 单一 run id（spec §2）

### Task 1: worktree、基线、V4 普查、V7 实测

**Files:**
- Modify: `docs/superpowers/specs/2026-09-24-run-identity-a5-design.md`（§6 V4 / V7 行写结论）

**Interfaces:**
- Consumes: 无
- Produces: worktree `D:\Workspace\Aleph\.claude\worktrees\run-identity-p1`（分支 `worktree-run-identity-p1`，基于 main）；基线红名单文件 `$SCRATCH/p1-baseline-reds.txt`；V7 基线数

- [ ] **Step 1: 建 worktree**

```bash
cd /d/Workspace/Aleph && git status --porcelain
git worktree add .claude/worktrees/run-identity-p1 -b worktree-run-identity-p1 main
cd /d/Workspace/Aleph/.claude/worktrees/run-identity-p1 && git log --oneline -1
```
Expected: `git status --porcelain` 空（spec 修订已先在 main 上提交）；worktree HEAD = main HEAD。

- [ ] **Step 2: 跑 `--lib` 基线（后台，约 13 分钟）**

```bash
cd /d/Workspace/Aleph/.claude/worktrees/run-identity-p1
LOG=$SCRATCH/p1-baseline.log
echo 'CARGO_TARGET_DIR=D:/Workspace/Aleph/target CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib' > $LOG
CARGO_TARGET_DIR=D:/Workspace/Aleph/target CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib >> $LOG 2>&1
grep -E '^test .* FAILED$' $LOG | sed 's/^test //; s/ \.\.\. FAILED$//' | sort > $SCRATCH/p1-baseline-reds.txt
cat $SCRATCH/p1-baseline-reds.txt
```
Expected: 名单与 Global Constraints 里的 4 条一致。多出来的名字先查是否与本计划的文件相交（按文件不相交 + 失败信息相同来判定为环境性），写进基线文件的注释行，不修。

- [ ] **Step 3: V7——量 harness 预算**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib harness::tests::budget -- --nocapture 2>&1 | tail -20
```
记下实测值与 `CEILING`。

- [ ] **Step 4: V4——普查以 run id 为键的读者与写者（剥掉测试模块）**

```bash
# Every production reader/writer of a marker's run_id (single- and multi-line forms).
rg -n -U "Run(Started|Finished) \{\s*(//[^\n]*\s*)*run_id" src --type rust | grep -v "tests\.rs\|/tests/"
rg -n "Run(Started|Finished) \{[^}]*run_id" src --type rust | grep -v "run_id: _\|tests\.rs\|/tests/"
# Readers of the reduced run id outside the log.
rg -n "open_run\b.*run_id|\.run_id\b" src/gateway/resume_coordinator.rs src/session src/diagnostics
```
对每个命中确认它在 `#[cfg(test)]` 之外（看文件里 `mod tests` 的行号）。计划编写时（main `4873c347e` 之后）测得的**生产**集合，普查结果应与之一致；不一致就停下，先把新成员写进 spec 再继续：

| 角色 | 位置 | P1 之后 |
|---|---|---|
| 写 | `runner_impl.rs` 开/闭 | 引擎 id（Task 2） |
| 写 | `run_loop/mod.rs` hookstop | 引擎 id（Task 3） |
| 写 | `session_split.rs` 父闭 / 子开 | 父 open run 的 id（Task 4） |
| 写 | `resume_coordinator.rs::abandon` | open run 的 id / Unanswered 臂保留合成（Task 5） |
| 写 | `marker_balance.rs:108` · `boundary_repair.rs:289` | 已用 open run 的 id，不动 |
| 写 | `fast_path.rs` `slash-*` | 不动（spec §2 唯一例外） |
| 按 id 读 | `event_snap.rs::snap_out_of_open_run` | Task 6 |
| 按 id 读 | `runner_impl.rs` 扫描起点 `rposition` | 已取最近，Task 2 只改注释 |
| 按 id 读 | `sqlite_backend::already_stamped_by`（两后端共用） | 不改；P1 让它认得同 id |
| 按位置 | `reduction.rs` · `run_span.rs::collect_run_spans` · `usage_fold.rs` · `store.rs::load_retired_run_anchors` | 不受影响（U1 的依据） |
| 仅日志 | `resume_coordinator.rs:703`（`run_id = %facts.run_id`） | 无键语义 |

日志之外（tracker、`agent.run_*`、`task_traces`、Panel、busy queue durable）今天已用引擎 id；普查确认**没有任何一处把 marker id 当键**读回——这正是 P1 不需要迁移它们的依据。

- [ ] **Step 5: 把 V4 / V7 结论写回 spec §6 并提交**

在 spec §6 的 V4 行末尾追加 `**已结**（Task 1，<sha>）：生产写者 N、按 id 读者 2、日志外无 marker id 键；表见计划 Task 1`，V7 行追加实测值。

```bash
git add docs/superpowers/specs/2026-09-24-run-identity-a5-design.md
git commit -m "docs: record the V4 run-id census and the V7 harness budget baseline"
```

---

### Task 2: `FlowRequest.run_id` 下行，bridge 的 marker 用引擎 id

**Files:**
- Modify: `src/orchestrator/dispatch.rs`（`FlowRequest` ~436、Debug ~586、`HarnessRunner::run` ~713、`dispatch()` ~1133 与 ~1233）
- Modify: `src/gateway/execution_engine/run_loop/inner.rs:1382`（`FlowRequest` 字面量）
- Modify: `src/orchestrator/harness_bridge/runner_impl.rs`（`run` 签名 104-121；960-968；1101-1104）
- Modify（测试替身 7 个）：`src/orchestrator/tests/dispatch.rs:22,354,479,687` · `src/gateway/handlers/chat.rs:1991` · `src/gateway/handlers/flow_admin.rs:140` · `tests/gateway_chat_common/mod.rs:127`
- Modify（测试字面量 13 处）：`src/orchestrator/tests/dispatch.rs`（10 处）· `tests/gateway_chat_common/mod.rs:239` · `tests/orchestrator_e2e.rs:18,84`
- Test: `src/orchestrator/harness_bridge/tests.rs` · `src/orchestrator/tests/dispatch.rs`

**Interfaces:**
- Consumes: 无
- Produces: `pub struct FlowRequest { .., pub run_id: String, .. }`；`HarnessRunner::run(&self, session_key, spec, input, sandbox, events, cancel, tool_service_override, trace_sink, interaction_manifest, workspace_override, max_iterations_override, transient_context, think_level, envelope, model_directive, run_id: String)`——**新参数在最后**。

- [ ] **Step 1: 加字段（先不使用）**

`src/orchestrator/dispatch.rs`，`pub agent_id: AgentId,` 之后插入：

```rust
    /// The engine's id for the run this dispatch serves (`RunRequest.run_id`).
    /// The harness bridge writes it on the run's `RunStarted` / `RunFinished`
    /// markers, and `execute()` stamps the same id on the run's
    /// `AssistantRunMeta` — so the log's markers and its meta name one run.
    /// Required and deliberately not defaulted: a placeholder id would be a
    /// marker that joins nothing. The fallback retry loop rebuilds this
    /// request under the SAME id, so one run can leave several marker
    /// brackets in one log.
    pub run_id: String,
```

Debug impl 里 `.field("agent_id", &self.agent_id)` 之后加 `.field("run_id", &self.run_id)`。

- [ ] **Step 2: 加 trait 参数、转发、所有实现者与字面量**

`HarnessRunner::run` 在 `model_directive: Option<..>,` 之后加：

```rust
        // The engine's run id (`FlowRequest::run_id`). Implementations that
        // write run markers MUST write this id on them — see that field's doc.
        run_id: String,
```

`dispatch()`：在 `let model_directive = req.model_directive.clone();` 之后加
```rust
        // rust-doctor-disable-next-line excessive-clone
        let run_id = req.run_id.clone();
```
并在 `harness.run(..)` 的实参末尾 `model_directive,` 之后加 `run_id,`。

`run_loop/inner.rs:1382` 的字面量里 `agent_id: agent.id().to_string(),` 之后加：
```rust
                // The same id `execute()` stamps on this run's meta; the
                // bridge writes it on the run's markers (FOLLOW-UP F1).
                run_id: run_id.to_string(),
```
（`run_id: &str` 是 `run_agent_loop` 的参数，= `request.run_id`，见 `execute.rs:361`。）

7 个测试替身的 `run` 签名末尾（`_turn_model: ..,` 之后）加 `_run_id: String,`；`runner_impl.rs` 的生产实现末尾（`turn_model: ..,` 之后）加 `run_id: String,`，此步暂不使用——在 `let run_marker_id = uuid::Uuid::new_v4().to_string();` 上方加一行 `let _ = &run_id;` 以免 unused 警告（Step 5 删掉）。

13 处测试字面量加字段：`src/orchestrator/tests/dispatch.rs` 全部 `run_id: "test-run".into(),`；`tests/gateway_chat_common/mod.rs:239`（`basic_request`）用 `run_id: "run-1".into(),`（与该套测试传给 `run_dispatch_and_drain` 的 `"run-1"` 一致）；`tests/orchestrator_e2e.rs` 两处 `run_id: "e2e-run".into(),`。`src/orchestrator/harness_bridge/tests.rs::run_until_it_fails`（~1057）的 `.run(..)` 实参末尾加 `"probe-run".to_string(),`。

`StubContext`（`tests/gateway_chat_common/mod.rs`）加 `pub run_id: String,`，`StubHarnessRunner::run` 把 `_run_id` 改名为 `run_id` 并填进 `StubContext { .., run_id }`。

- [ ] **Step 3: 编译**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib --no-run 2>&1 | tail -5
```
Expected: 编译通过。报错只会是漏掉的字面量或替身——按编译器列出的位置补齐（判据 #6：编译器的名单才是全集）。

- [ ] **Step 4: 写 bridge 的失败测试**

`src/orchestrator/harness_bridge/tests.rs` 末尾（沿用本模块 `run_until_it_fails` 已用的 import：`fresh_service`、`runner_with_failing_provider`、`probe_spec`、`SessionKey`、`SessionEvent`、`broadcast`、`FlowStreamEvent`、`CancellationToken`、`FlowInput`）：

```rust
/// The run's markers carry the id the ENGINE gave the run — the one
/// `execute()` stamps on its `AssistantRunMeta` — read back from the log,
/// not from the call. A locally-minted marker id (ruling A5, until
/// 2026-09-24) made every by-id reader miss: a synthesized stamp and the
/// late meta read as two runs and the session was billed twice (F1).
/// The provider fails, so the run ends on the error arm — which is also
/// the arm that must still close its bracket.
#[tokio::test]
async fn the_run_markers_carry_the_engine_run_id() {
    let service = fresh_service();
    let runner = runner_with_failing_provider(service.clone(), "marker-id-probe");
    let (tx, _rx) = broadcast::channel::<FlowStreamEvent>(256);
    let key = SessionKey::ephemeral("marker-id-probe");

    let _ = runner
        .run(
            key.to_key_string(),
            probe_spec("marker-id-probe"),
            FlowInput::Prompt("x".into()),
            std::sync::Arc::new(crate::sandbox::factory::NoopSandbox),
            tx,
            CancellationToken::new(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            crate::thinker::TurnEnvelope::default(),
            None,
            "engine-run-42".to_string(),
        )
        .await;

    let markers: Vec<(&'static str, String)> = service
        .get_events(&key, None, None)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|r| match r.event {
            SessionEvent::RunStarted { run_id, .. } => Some(("start", run_id)),
            SessionEvent::RunFinished { run_id, .. } => Some(("finish", run_id)),
            _ => None,
        })
        .collect();
    assert_eq!(
        markers,
        vec![
            ("start", "engine-run-42".to_string()),
            ("finish", "engine-run-42".to_string()),
        ],
        "one bracket, both markers under the engine's run id"
    );
}
```

- [ ] **Step 5: 跑它，确认红**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib the_run_markers_carry_the_engine_run_id 2>&1 | tail -15
```
Expected: FAIL，左值是两个 UUID（marker 仍是自铸）。若 `get_events` 的签名不同，照本模块其他读日志的测试改调用，不改断言。

- [ ] **Step 6: 用引擎 id**

`runner_impl.rs` 把 960-968 那段（从 `// Resume run markers. \`run_id\` is a locally-minted UUID,` 到 `let run_marker_id = uuid::Uuid::new_v4().to_string();`，以及 Step 2 加的 `let _ = &run_id;`）替换为：

```rust
        // Resume run markers carry the ENGINE's run id — the one `execute()`
        // stamps on this run's `AssistantRunMeta` — so every by-id reader
        // (`already_stamped_by`, `snap_out_of_open_run`, the scan below)
        // pairs a marker with its meta. Minting a local id here (ruling A5,
        // until 2026-09-24) left the two unjoinable: a synthesized stamp and
        // the late meta read as two runs and billed the run twice (F1).
        // One engine run can write several brackets under this id — the
        // fallback retry loop re-dispatches under the same run — so a by-id
        // reader pairs each closer with the LAST opener before it.
        let run_marker_id = run_id;
```

1101-1104 的注释（`// Marker emitted at ... byte-identical to the prior behaviour on that path.`）改为：

```rust
        // Marker emitted at `SessionEvent::RunStarted { run_id: run_marker_id }`
        // just before `harness.run`; all seeded history/user events precede it.
        // `rposition` takes the LAST opener under this id: the fallback retry
        // loop can leave earlier brackets of the same run in this log. On a
        // compaction-driven session split the adopted child's log carries the
        // split's own opener under this same id AFTER the carried tail (the
        // parent's copied opener is filtered out of the tail, F14), so the
        // scan counts only what the run produced after the split; the pre-split
        // part stays in the parent's log.
```

- [ ] **Step 7: 跑，确认绿**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib harness_bridge 2>&1 | tail -8
```
Expected: `the_run_markers_carry_the_engine_run_id` PASS；`harness_bridge` 其余测试全绿（含 `runner_impl.rs:1785` 附近的 envelope 普查——它只数带 `envelope:` 的构造点，不受影响）。

- [ ] **Step 8: dispatch 转发的钉子**

`src/orchestrator/tests/dispatch.rs`：`CapturingHarness` 加字段
```rust
    received_run_id: Arc<Mutex<Option<String>>>,
```
其 `run` 的末参改为 `run_id: String,`，函数体开头加
```rust
        *self
            .received_run_id
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(run_id);
```
`fixture_capturing_orchestrator()` 返回类型加第 4 个元素 `Arc<Mutex<Option<String>>>`，构造 `let received_run_id = Arc::new(Mutex::new(None::<String>));`，传进 `CapturingHarness { .., received_run_id: received_run_id.clone() }`，元组末尾返回它。两个现有调用者（`dispatch_forwards_tool_service_override`、`dispatch_forwards_trace_sink`）的解构各加一个 `_received_run_id`。新增测试：

```rust
/// The run id reaches the runner — the one fact the bridge needs to write
/// markers the meta can join. Asserted at the runner, not on the request.
#[tokio::test]
async fn dispatch_forwards_run_id() {
    let (orch, _tool_service, _trace_sink, received_run_id) = fixture_capturing_orchestrator();
    let handle = orch
        .dispatch(FlowRequest {
            flow_id: None,
            agent_id: "main".into(),
            run_id: "engine-run-7".into(),
            input: FlowInput::Prompt("test".into()),
            channel: None,
            session_hint: None,
            scope: crate::scope::FlowScope::unscoped(),
            parent_session: None,
            depth: 0,
            tool_service: None,
            trace_sink: None,
            event_tx: None,
            interaction_manifest: None,
            sandbox_override: None,
            workspace_override: None,
            max_iterations_override: None,
            transient_context: None,
            think_level: None,
            envelope: crate::thinker::TurnEnvelope::none(),
            model_directive: None,
        })
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .expect("dispatch ok");
    // rust-doctor-disable-next-line unwrap-in-production
    let _ = handle.completion.await.unwrap();
    assert_eq!(
        received_run_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_deref(),
        Some("engine-run-7"),
        "dispatch must hand the runner the request's run id"
    );
}
```

- [ ] **Step 9: 变异确认 Step 8 能红**

临时把 `dispatch()` 里的 `let run_id = req.run_id.clone();` 改成 `let run_id = String::new();`，跑 `cargo test -p alephcore --lib dispatch_forwards_run_id`，Expected FAIL；**恢复**，再跑 Expected PASS。

- [ ] **Step 10: 格式、CRLF、提交**

```bash
for f in src/orchestrator/dispatch.rs src/orchestrator/harness_bridge/runner_impl.rs src/orchestrator/harness_bridge/tests.rs src/orchestrator/tests/dispatch.rs src/gateway/execution_engine/run_loop/inner.rs; do rustfmt --edition 2021 --check $f >/dev/null || echo "FMT $f"; done
git add -u src tests
git commit -m "orchestrator: the bridge writes run markers under the engine's run id (F1)"
```

---

### Task 3: hookstop 用引擎 id

**Files:**
- Modify: `src/gateway/execution_engine/run_loop/mod.rs:478`（`hook_stop_receipt`）与测试 ~948

**Interfaces:**
- Consumes: `RunRequest.run_id`（已有字段）
- Produces: 无新接口

- [ ] **Step 1: 改断言（先红）**

`run_loop/mod.rs` 测试里：
```rust
        assert!(
            r0.starts_with("hookstop-"),
            "a locally-minted marker id, got {r0}"
        );
```
替换为
```rust
        assert_eq!(
            r0, "test-run",
            "the bracket carries the engine's run id — the meta `execute()` \
             stamps for this Ok run carries the same one"
        );
```

- [ ] **Step 2: 确认红**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib run_loop::tests 2>&1 | tail -10
```
Expected: 该测试 FAIL，左值 `hookstop-<uuid>`。

- [ ] **Step 3: 改实现**

`let run_id = format!("hookstop-{}", uuid::Uuid::new_v4());` 替换为：
```rust
    // The engine's id: this path returns `Ok`, so `execute()` stamps an
    // `AssistantRunMeta` under `request.run_id` for it — a minted id here
    // would leave that meta unjoinable to this bracket (F1).
    let run_id = request.run_id.clone();
```

- [ ] **Step 4: 确认绿，提交**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib run_loop 2>&1 | tail -5
git add src/gateway/execution_engine/run_loop/mod.rs
git commit -m "gateway: a hook-stopped run brackets its receipt under the engine's run id"
```

---

### Task 4: split 沿用父 open run 的 id，并滤掉复制进尾巴的父 opener（F14）

**Files:**
- Modify: `src/context/compact/session_split.rs:160-205`（`perform_session_split` 第 3、4 步）
- Test: 同文件 `split_seeds_child_with_forked_summary_and_fresh_tail`（~835）、`a_parents_opener_copied_inside_the_tail_still_yields_the_split_opener_last`（~1104）、`parent_log` 的 doc（引用了后者的名字）

**Interfaces:**
- Consumes: `reduce_run(events)?.open_run: RunStartFacts { seq, run_id, project_root, envelope }`（已在 :118 算出）
- Produces: 无新接口

- [ ] **Step 1: 改测试（先红）**

(a) `split_seeds_child_with_forked_summary_and_fresh_tail` 的 `child_rows[3]` 臂，`assert_eq!(run_id, closer_id, ..)` 之后加：
```rust
                assert_eq!(
                    run_id, "run-parent",
                    "the split closes and reopens the parent's OWN run — the \
                     engine's id, which its meta carries — not a new one"
                );
```

(b) 把 `a_parents_opener_copied_inside_the_tail_still_yields_the_split_opener_last` 改名为 `a_parents_opener_inside_the_tail_is_not_copied_into_the_child`，doc 改为：
```rust
    /// The bridge's real order — the turn is seeded, THEN `RunStarted` is
    /// emitted — puts the parent's own opener inside the carried tail whenever
    /// the split fires before the run's first assistant message. Copying it
    /// gave the child two openers for one run (F14). The child gets exactly
    /// one: the split's, under the parent's run id, carrying the parent's
    /// envelope and project root. Read back through the service, in emission
    /// order.
```
测试体从 `assert_eq!(openers.len(), 2, ..)` 起到函数末尾替换为：
```rust
        assert_eq!(
            openers,
            vec![(
                "run-parent".to_string(),
                Some(PARENT_ROOT.to_string()),
                Some(parent_envelope())
            )],
            "one opener in the child — the split's, under the parent's run id, \
             inheriting its envelope and root; the tail's copy is filtered out"
        );
```
（`RunEnvelopeSnapshot` 需 `PartialEq + Debug`——原测试已对它 `assert_eq!`，成立。）

`parent_log` 的 doc 里 `` `a_parents_opener_copied_inside_the_tail_still_yields_the_split_opener_last` `` 改为 `` `a_parents_opener_inside_the_tail_is_not_copied_into_the_child` ``。

- [ ] **Step 2: 确认红**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib session_split 2>&1 | tail -15
```
Expected: 两个测试 FAIL（一个是 UUID ≠ `run-parent`，一个是两枚 opener）。

- [ ] **Step 3: 改实现**

`let split_run_id = uuid::Uuid::new_v4().to_string();` 替换为：
```rust
    // The parent's open run continues on the child under the SAME id — the
    // engine's `RunRequest.run_id`, which the bridge wrote on the parent's
    // opener and `execute()` stamps on the run's meta. A fresh id here left
    // the parent's opener and closer unpaired and the child's span unjoinable
    // to the meta (F1).
    let run_id = open_run.run_id.clone();
    let opener_seq = open_run.seq;
```
第 3 步注释里 `The run_id need only correlate the closer with the child's opener — the resume scan is positional.` 改为 `It carries the open run's own id, so a by-id reader pairs it with the parent's opener.`；`run_id: split_run_id.clone(),` → `run_id: run_id.clone(),`。

`child_batch.extend(tail.iter().map(|record| record.event.clone()));` 替换为：
```rust
    // The parent's own opener can sit inside the tail (the bridge seeds the
    // turn, THEN emits `RunStarted`). Copying it gave the child two openers
    // for one run (F14); the split opener below is the only one it gets.
    child_batch.extend(
        tail.iter()
            .filter(|record| record.seq != opener_seq)
            .map(|record| record.event.clone()),
    );
```
子 opener 的 `run_id: split_run_id,` → `run_id,`。

- [ ] **Step 4: 确认绿，提交**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib context::compact 2>&1 | tail -5
git add src/context/compact/session_split.rs
git commit -m "compact: a session split reuses the open run's id and stops copying its opener (F1, F14)"
```

---

### Task 5: `abandon` 以 open run 的 id 闭

**Files:**
- Modify: `src/gateway/resume_coordinator.rs`（`abandon` ~1901；调用者 1574、1594、1832、1848）
- Test: `tests/resume_coordinator_integration.rs`

**Interfaces:**
- Consumes: `reduction.open_run: Option<RunStartFacts>`（`handle_interrupted` 作用域内已有）
- Produces: `async fn abandon(&self, session_id: &SessionId, what: Abandoned, closes: Option<&str>, reason: &str)`

- [ ] **Step 1: 写失败测试**

`tests/resume_coordinator_integration.rs`，紧跟 `crash_loop_cap_abandons_instead_of_retriggering` 之后：
```rust
/// An abandoned interrupted run is closed under ITS OWN run id, so a by-id
/// reader (`snap_out_of_open_run`) pairs the closer with its opener. The old
/// `abandoned-<uuid>` closer paired with nothing.
#[tokio::test]
async fn an_abandoned_interrupted_run_is_closed_under_its_own_run_id() {
    let store = store();
    let sid = SessionKey::main("abandon-id-agent");
    seed_crash_looped_run(&store, &sid, 3).await;

    let adapter = Arc::new(RecordingAdapter::new());
    let registry = registry_with_agent(sid.agent_id()).await;
    let coordinator = Arc::new(ResumeCoordinator::new(
        store.clone(),
        ResumeConfig::default(),
        adapter as Arc<dyn ExecutionAdapter>,
        registry,
        sessions(),
        test_bus(),
    ));
    let report = coordinator.resume_interrupted_runs().await;
    assert_eq!(report.abandoned, 1, "{report:?}");

    let closers: Vec<String> = store
        .load_all_events(&sid)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|r| match r.event {
            SessionEvent::RunFinished {
                run_id,
                outcome: RunOutcome::Abandoned,
                ..
            } => Some(run_id),
            _ => None,
        })
        .collect();
    assert_eq!(
        closers,
        vec!["r1".to_string()],
        "the abandon closer names the run it closes"
    );
}
```

- [ ] **Step 2: 确认红**

```bash
cd /d/Workspace/Aleph/.claude/worktrees/run-identity-p1
cargo test -p alephcore --features test-helpers --test resume_coordinator_integration -j 1 an_abandoned_interrupted_run 2>&1 | tail -10
```
（worktree 自己的 target，`-j 1`。）Expected: FAIL，左值 `["abandoned-<uuid>"]`。

- [ ] **Step 3: 改实现**

`abandon` 签名改为 `async fn abandon(&self, session_id: &SessionId, what: Abandoned, closes: Option<&str>, reason: &str)`；doc 末尾加一段：
```rust
    ///
    /// `closes` is the open run's own id when there is one — the same
    /// derivation `marker_balance` and `boundary_repair` use — so a by-id
    /// reader pairs the closer with its opener. The unanswered arm has no open
    /// run (it gives up on a user message nobody answered), so its closer keeps
    /// a synthesized id that pairs with nothing, by design.
```
函数体里 `run_id: format!("abandoned-{}", uuid::Uuid::new_v4()),` 改为
```rust
            run_id: closes.map_or_else(
                || format!("abandoned-{}", uuid::Uuid::new_v4()),
                str::to_string,
            ),
```
调用者：1574、1594（`handle_interrupted`）在 `Abandoned::InterruptedRun,` 之后加 `reduction.open_run.as_ref().map(|o| o.run_id.as_str()),`（若编译器报 `reduction` 已被移动，改从 `run_started` 记录取：`match &run_started.event { SessionEvent::RunStarted { run_id, .. } => Some(run_id.as_str()), _ => None }`——同一个 run，因为 disposition 保证 anchor 就是 open run 的 opener）。1832、1848（unanswered 臂）加 `None,`。

- [ ] **Step 4: 确认绿，跑整份集成测试，提交**

```bash
cargo test -p alephcore --features test-helpers --test resume_coordinator_integration -j 1 2>&1 | tail -5
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib resume_coordinator 2>&1 | tail -5
git add src/gateway/resume_coordinator.rs tests/resume_coordinator_integration.rs
git commit -m "gateway: an abandoned interrupted run is closed under its own run id"
```

---

### Task 6: `snap_out_of_open_run` 每个闭配最后一个同 id 开

**Files:**
- Modify: `src/context/compact/event_snap.rs:37-72`
- Test: 同文件 `mod tests`（沿用 `run(seq, id, finished)` 与 `user(seq)`）

**Interfaces:**
- Consumes / Produces: 签名不变 `pub(crate) fn snap_out_of_open_run(events: &[SessionEventRecord], cut: usize) -> usize`

- [ ] **Step 1: 写测试**

```rust
    /// One engine run can leave several brackets under ONE id (the fallback
    /// retry loop re-dispatches under the same run). The closer after the cut
    /// pairs with the LAST opener before it — index 3 — not the first one: the
    /// first-match scan pulled the cut back to 0 and compacted nothing.
    #[test]
    fn a_reused_run_id_snaps_to_the_opener_its_closer_pairs_with() {
        let events = vec![
            run(1, "r", false),
            user(2),
            run(3, "r", true),
            run(4, "r", false),
            user(5),
            user(6),
            run(7, "r", true),
        ];
        assert_eq!(snap_out_of_open_run(&events, 5), 3);
    }

    /// Two runs close after the cut. Each closer pairs with its own id's last
    /// opener, and the cut goes back to the EARLIER of the two — "the nearest
    /// matching opener" (b's, index 2) would leave a's opener in the retired
    /// prefix while its closer stays live.
    #[test]
    fn two_runs_closed_after_the_cut_snap_to_the_earlier_opener() {
        let events = vec![
            run(1, "a", false),
            user(2),
            run(3, "b", false),
            user(4),
            run(5, "a", true),
            run(6, "b", true),
        ];
        assert_eq!(snap_out_of_open_run(&events, 4), 0);
    }
```

- [ ] **Step 2: 确认第一条红、第二条绿**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib event_snap 2>&1 | tail -10
```
Expected: `a_reused_run_id_snaps_to_the_opener_its_closer_pairs_with` FAIL（left 0, right 3）；`two_runs_closed_after_the_cut_snap_to_the_earlier_opener` PASS（它钉住的是「不许改成朴素的 `rposition`」，旧代码碰巧也对）。

- [ ] **Step 3: 改实现**

函数体从 `events[..cut]` 那段替换为：
```rust
    // Each closer pairs with the LAST opener under its id before the cut: one
    // engine run can leave several brackets under one id (the fallback retry
    // loop), and only the last one is the bracket this closer ends. The cut
    // then goes back to the earliest such opener, so every run closed in the
    // kept tail keeps its opener too.
    let mut earliest: Option<usize> = None;
    for id in &closed_after_cut {
        let opener = events[..cut].iter().rposition(|r| {
            matches!(&r.event, SessionEvent::RunStarted { run_id, .. } if run_id == id)
        });
        if let Some(pos) = opener {
            earliest = Some(earliest.map_or(pos, |e| e.min(pos)));
        }
    }
    earliest.unwrap_or(cut)
```
（`closed_after_cut` 是 `HashSet<&str>`，`id: &&str`，`run_id == id` 需要解引用时写 `run_id.as_str() == *id`——以编译器为准。）

- [ ] **Step 4: 确认绿，提交**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib context::compact 2>&1 | tail -5
git add src/context/compact/event_snap.rs
git commit -m "compact: pair each closer with the last opener under its id when snapping a cut"
```

---

### Task 7: 投影器消费侧钉子 + 旧形状注释

**Files:**
- Modify: `src/gateway/session_projector.rs`（测试：`marker_of` ~1326 doc、`one_billed_run` doc、~2125、~2164、~2262 的 doc；新增 4 个测试）
- Modify: `src/gateway/session_projector/projector_sub/run_span.rs`（doc :24、:71-83、:190-217）
- Modify: `src/gateway/session_projector/projector_sub/missed_seqs.rs`（`stamps_synthesized` doc 里 split 那句）
- Modify: `src/gateway/execution_engine/fast_path.rs:34-42`（一句注释）
- Test: `src/gateway/session_projector.rs`

**Interfaces:**
- Consumes: 测试辅助 `rec`、`run_started`、`run_finished`、`run_meta`、`assistant_msg_billed`、`own_event_log`、`sqlite_store`、`synthesis_ends`、`MessageProjector::with_event_store`、`on_appended`、`flush`、`request_repair`
- Produces: 无（P3 Task 11 会改这些测试里 `Projected::Stamped` 的形状——本任务的新测试不断言 `Projected`，只断言存储读回，所以 P3 不需要改它们）

这些测试钉的是**消费侧**：P1 之后日志的新形状被正确读取。它们在到达时就是绿的（`already_stamped_by` 本来就认同 id）；F1 的红在 Task 2。

- [ ] **Step 1: 同 id 的孪生（span 层）**

紧跟 `a_meta_marks_the_span_it_follows_even_though_its_id_is_not_the_markers` 之后：
```rust
    /// The shape every log written since 2026-09-24 carries: the markers and
    /// the meta share the engine's run id. Same answer as the legacy twin
    /// above — the join is positional, so it never depended on the ids.
    #[test]
    fn a_meta_under_its_markers_own_id_marks_the_span_it_follows() {
        let tid = uuid::Uuid::new_v4();
        assert_eq!(
            synthesis_ends(&[
                (1, run_started("engine-X")),
                (2, assistant_msg_billed(tid, 45, 25)),
                (3, run_finished("engine-X")),
                (4, run_meta(tid, "engine-X")),
            ]),
            vec![None],
        );
    }
```

- [ ] **Step 2: F1 回归 + U5 已知限制**

```rust
    /// F1, at the store: `request_repair` runs in the one-append window between
    /// a run's `RunFinished` and its meta, synthesizes the stamp and bills the
    /// run's tokens; the meta then lands under the SAME id, reads
    /// `AlreadyStamped` and bills nothing. Tokens once. Before the markers
    /// carried the engine id, the meta read the synthesized stamp as "another
    /// run's", overwrote it and billed a second time. The cost and model the
    /// meta carried are lost — ruling U5, asserted so a change to it is seen.
    #[tokio::test]
    async fn a_synthesized_stamp_then_its_own_meta_bills_the_run_once() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "synth_then_meta.db");
        let id = SessionId::ephemeral("synth-then-meta");
        store.get_or_create(&id).await.unwrap();

        let tid = uuid::Uuid::new_v4();
        let run = [
            (1, run_started("engine-r")),
            (2, assistant_msg_billed(tid, 45, 25)),
            (3, run_finished("engine-r")),
        ];
        let log = own_event_log(&id, &run).await;
        let projector = MessageProjector::with_event_store(store.clone(), None, Some(log.clone()));
        for (seq, ev) in &run {
            projector.on_appended(&id, &rec(*seq, ev.clone()));
        }
        projector.flush(Duration::from_secs(5)).await.unwrap();
        let repair = projector.request_repair(&id).await;
        assert_eq!(
            (repair.stamps_synthesized, repair.usage_rebilled),
            (1, 1),
            "the repair ran inside the window and synthesized: {repair:?}"
        );

        let meta_ev = run_meta(tid, "engine-r");
        log.append(&id, 4, &meta_ev, 0).await.unwrap();
        projector.on_appended(&id, &rec(4, meta_ev));
        projector.flush(Duration::from_secs(5)).await.unwrap();

        let meta = store.get_metadata(&id).await.unwrap().unwrap();
        assert_eq!((meta.input_tokens, meta.output_tokens), (45, 25), "tokens once");
        assert_eq!(
            (meta.estimated_cost_usd, meta.model.as_deref()),
            (0.0, None),
            "U5: the late meta's cost and model are not accumulated"
        );
    }
```

- [ ] **Step 3: 重试循环的多组同 id bracket（Review Focus 1）**

```rust
    /// One engine run whose first attempt failed and was retried under the
    /// same run id: two brackets, one meta after the last. The meta bills the
    /// last bracket; the first — finished, with a row, no meta — is
    /// synthesized and billed once from its own messages; a second repair adds
    /// nothing. The provider billed both attempts, so both are counted.
    #[tokio::test]
    async fn a_retried_run_bills_each_bracket_once() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "retried_run.db");
        let id = SessionId::ephemeral("retried-run");
        store.get_or_create(&id).await.unwrap();

        let tid = uuid::Uuid::new_v4();
        let events = vec![
            (1, run_started("engine-r")),
            (2, assistant_msg_billed(tid, 10, 5)),
            (
                3,
                SessionEvent::RunFinished {
                    run_id: "engine-r".into(),
                    outcome: crate::session::events::RunOutcome::Errored,
                    at: 2,
                },
            ),
            (4, run_started("engine-r")),
            (5, assistant_msg_billed(tid, 45, 25)),
            (6, run_finished("engine-r")),
            (7, run_meta(tid, "engine-r")),
        ];
        let log = own_event_log(&id, &events).await;
        let projector = MessageProjector::with_event_store(store.clone(), None, Some(log));
        for (seq, ev) in &events {
            projector.on_appended(&id, &rec(*seq, ev.clone()));
        }
        projector.flush(Duration::from_secs(5)).await.unwrap();
        let tokens = |m: &crate::gateway::session_store::types::SessionMetadata| {
            (m.input_tokens, m.output_tokens)
        };
        assert_eq!(
            tokens(&store.get_metadata(&id).await.unwrap().unwrap()),
            (45, 25),
            "the live meta billed the bracket it closes"
        );

        let first = projector.request_repair(&id).await;
        assert_eq!((first.stamps_synthesized, first.usage_rebilled), (1, 1), "{first:?}");
        let second = projector.request_repair(&id).await;
        assert!(second.up_to_date, "{second:?}");
        assert_eq!(
            tokens(&store.get_metadata(&id).await.unwrap().unwrap()),
            (55, 30),
            "each bracket billed exactly once"
        );
    }
```

- [ ] **Step 4: 新旧形状混排（Review Focus 2）**

```rust
    /// A session that straddles the upgrade: run a was written before the
    /// markers carried the engine id (marker ≠ meta id), run b after (same id).
    /// Both are billed once live, and two repairs add nothing.
    #[tokio::test]
    async fn a_legacy_run_and_a_same_id_run_in_one_log_are_each_billed_once() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "mixed_shapes.db");
        let id = SessionId::ephemeral("mixed-shapes");
        store.get_or_create(&id).await.unwrap();

        let tid = uuid::Uuid::new_v4();
        let mut events = one_billed_run(tid, "a");
        events.extend([
            (5, run_started("b")),
            (6, assistant_msg_billed(tid, 10, 5)),
            (7, run_finished("b")),
            (8, run_meta(tid, "b")),
        ]);
        let log = own_event_log(&id, &events).await;
        let projector = MessageProjector::with_event_store(store.clone(), None, Some(log));
        for (seq, ev) in &events {
            projector.on_appended(&id, &rec(*seq, ev.clone()));
        }
        projector.flush(Duration::from_secs(5)).await.unwrap();
        for pass in 1..=2 {
            let repair = projector.request_repair(&id).await;
            assert!(repair.up_to_date, "repair {pass}: {repair:?}");
        }
        let meta = store.get_metadata(&id).await.unwrap().unwrap();
        assert_eq!((meta.input_tokens, meta.output_tokens), (55, 30));
        let ids: Vec<Option<String>> = store
            .get_history(&id, None)
            .await
            .unwrap()
            .into_iter()
            .filter(|m| m.role == "assistant")
            .map(|m| {
                m.metadata
                    .as_ref()
                    .and_then(|v| v.get("run_id"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .collect();
        assert_eq!(ids, vec![Some("a".to_string()), Some("b".to_string())]);
    }
```

- [ ] **Step 5: 跑新测试**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib session_projector 2>&1 | tail -8
```
Expected: 全部 PASS。任何一条红 ⇒ 停下：它说明 P1 的新形状在消费侧没被正确读取，是设计缺陷，不是测试写错——带着红的名字与输出回报，不改断言。

- [ ] **Step 6: 旧形状测试的注释（保持绿，只改措辞）**

- `marker_of` 的 doc 改为：`The marker id a log written BEFORE 2026-09-24 carries on its RunStarted / RunFinished: the bridge minted its own, never the engine id the meta carries (ruling A5, reversed by F1). Fixtures that pin the legacy shape derive the marker id here; the same-id shape every newer log carries has its own twins below (U1: both shapes stay readable).`
- `one_billed_run` 的 doc 末句改为 `The markers carry [`marker_of`]`(run)`, the meta carries `run` — the legacy shape.`
- `a_meta_marks_the_span_it_follows_even_though_its_id_is_not_the_markers`、`a_historical_meta_that_landed_after_the_next_opener…`、`a_live_run_with_its_meta_is_billed_once_and_two_repairs_add_nothing` 的 doc 里「the two are never equal (ruling A5)」/「markers with the marker id, meta with the engine id」一类表述，改成「in a log written before the markers carried the engine id」。
- `grep -n "marker id\|never equal\|A5" src/gateway/projection_reconciler.rs`——若 `:1374` `:1436` 附近的 doc 把「marker ≠ meta id」说成生产现状，同样改成「P1 之前写下的日志形状」。

- [ ] **Step 7: 生产注释**

`run_span.rs`：
- :24 附近 `meta carries (ruling A5: the two are never joined)` → `meta carries — equal since 2026-09-24 (F1), different in older logs; the join is positional, so either reads the same`。
- :71-83 的 split 段：`RunStarted { split_run_id }` → `RunStarted { R }`（split 沿用父 run 的 id）；`markers carry the harness-minted marker id … never equal on a live log (ruling A5)` → `older logs' markers carry a harness-minted id that is never the meta's; newer logs carry the engine id on both (F1). The join below is positional and reads both`。
- :190-217（`synthesize_missing_stamps` doc）删掉 `Against a later heal only — the stamp carries the MARKER id …` 那句，race 段改为：
```rust
/// The boot reconciler runs before any run is live, so it cannot race a meta
/// that is about to be appended. A `request_repair` on a live session (the
/// doctor's repair) can, in the window between a run's `RunFinished` and its
/// meta: the synthesized stamp lands first, under the run's own id since the
/// markers carry the engine id (F1), so the meta then reads `AlreadyStamped`
/// and bills nothing. The run's tokens are billed once; the cost and model
/// the meta carried are not (ruling U5). Logs written before F1 still carry
/// a marker id there, and for them the window still double-bills — U1.
```
`missed_seqs.rs` `stamps_synthesized` doc 的 split 句保持事实（meta 仍落在父上——P2 延后），只把 `split_run_id` 字样（如有）改为「the run's own id」。

`fast_path.rs` 在铸 `slash-` id 的那一行上方加：
```rust
        // The one marker id still minted locally: a slash-command turn is not
        // an engine run — no `execute()`, no `AssistantRunMeta` — so there is
        // nothing for this bracket's id to join (spec 2026-09-24 §2).
```

- [ ] **Step 8: 跑、提交**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib gateway::session_projector gateway::projection_reconciler fast_path 2>&1 | tail -8
git add -u src
git commit -m "gateway: pin the same-id run shape at the projector and relabel the legacy-shape fixtures"
```

---

### Task 8: P1 文档、真机断言、验证、变异、合并

**Files:**
- Modify: `qa/resume_boundary/drive_r2.mjs`（`cmdHoles`）· `qa/README.md`（`holes` 条目）
- Modify: `docs/reference/FEATURE_LOCATOR.md`（§4.13a ⑳、㉖；附录 D.0.185；附录 E 引 D.0.185 的那一行）
- Modify: `docs/superpowers/plans/2026-09-12-persistence-wal-recovery-r3.md`（FOLLOW-UP F1、F14；F26/F29/F30 注明随 P2 延后）
- Modify: `docs/superpowers/specs/2026-09-24-run-identity-a5-design.md`（§8 P1 行标「已落地 <sha>」）

**Interfaces:**
- Consumes: Task 2–7 的全部提交
- Produces: main 上的 P1

- [ ] **Step 1: 真机断言——meta 的 id 等于它前面 opener 的 id**

`drive_r2.mjs` 的 `cmdHoles` 里，`const stateFile = …` 之前插入（CRLF 文件，用 Edit 工具）：
```js
  // Run identity (spec 2026-09-24 §2): every live meta names the run whose
  // opener precedes it. Read off the log the real binary wrote, so it covers
  // the one link no unit test drives — the engine handing its run id to the
  // bridge. At least one meta, or the claim is vacuous and says so.
  const starts = rowsOfKind(key, "run_started");
  const metas = rowsOfKind(key, "assistant_run_meta");
  const mismatched = metas.filter((m) => {
    const opener = [...starts].reverse().find((s) => s.seq < m.seq);
    return !opener || opener.payload.run_id !== m.payload.run_id;
  });
  check(
    metas.length > 0,
    `[${phase}] the burst session carries at least one run meta to join`,
    `metas ${metas.length}`,
  );
  check(
    mismatched.length === 0,
    `[${phase}] every run meta carries the run id of the opener before it`,
    show(mismatched.map((m) => ({ seq: m.seq, run_id: m.payload.run_id })), 300),
  );
```
`qa/README.md` 的 `holes` 条目加一句：`Also claims that every live AssistantRunMeta carries the run id of the RunStarted before it — the engine→bridge run-id link (2026-09-24 §2), which no unit test drives end to end.`

- [ ] **Step 2: FEATURE_LOCATOR**

- §4.13a ⑳：「三种 `RunStarted.run_id` 拼法」改为两种——引擎 `RunRequest.run_id`（bridge / hookstop / split / abandon 有 open run 时）与 `slash-*`（fast path，非引擎 run，无 meta）；Unanswered 臂的 abandon 仍是 `abandoned-*`。
- §4.13a ㉖：已知限制 (b)（doctor 竞争窗双计）改为 U5（token 一次、cost/model/仪表丢失）；FOLLOW-UP「A5 join」标 **P1 已落地（<sha>）**；(c) 留给 P3。
- 附录 D.0.185 末尾加一行：`2026-09-24 修正：marker 改用引擎 id（F1），id 与位置两种 join 从此给出同一答案；旧日志仍只能按位置读（U1）。` 附录 E 引 D.0.185 的那一行同步一句。
- 用 `grep -n "三种 RunStarted\|A5 join\|D\.0\.185" docs/reference/FEATURE_LOCATOR.md` 定位；只用 Edit 工具。

- [ ] **Step 3: r3 计划 FOLLOW-UP**

F1、F14 两条按已有格式就地划掉并写 `**已关闭**（<sha>）：…一句机制…`（参考 F5 行的写法）。F26、F29、F30 末尾追加 `〔2026-09-24〕随 run-identity spec P2 延后（U4：默认 file 后端上 split 不可达）`。

- [ ] **Step 4: 验证集（按顺序；QA 在 clippy 之前）**

```bash
cd /d/Workspace/Aleph/.claude/worktrees/run-identity-p1
export CARGO_TARGET_DIR=D:/Workspace/Aleph/target CARGO_PROFILE_TEST_DEBUG=line-tables-only
# 1. full --lib, background, names diffed against the baseline
cargo test -p alephcore --lib > $SCRATCH/p1-final.log 2>&1
grep -E '^test .* FAILED$' $SCRATCH/p1-final.log | sed 's/^test //; s/ \.\.\. FAILED$//' | sort | diff $SCRATCH/p1-baseline-reds.txt -
# 2.
cargo test -p alephcore --bins
# 3. worktree-local target, -j 1
env -u CARGO_TARGET_DIR cargo test -p alephcore --features test-helpers --test '*' --no-run -j 1
env -u CARGO_TARGET_DIR cargo test -p alephcore --features test-helpers --test resume_coordinator_integration --test gateway_chat_through_orchestrator --test orchestrator_e2e -j 1
# 4.
cargo test -p aleph-panel --lib --no-run
# 5. V7
cargo test -p alephcore --lib harness::tests::budget
```
Expected：(1) diff 为空；(2)(3)(4)(5) 绿；(5) 的实测值与 Task 1 相同。`just test-shared` 与 `cargo check -p aleph-desktop-*` **不跑**：本期没碰 `shared/` `interfaces/` `desktop/`（spec §7）——在最终报告里写明。

- [ ] **Step 5: 真机 QA（在 clippy 之前）**

```bash
bash qa/resume_boundary/run.sh holes knobs claims 2>&1 | tee $SCRATCH/p1-qa.log | tail -40
```
Expected: 三个阶段全 PASS，`holes` 里新的两条 claim 出现且为 PASS。任何 INSTRUMENT FAILURE 先按 `qa/README.md` 排查装置，不把它读成产品结论。

- [ ] **Step 6: clippy**

```bash
just _stage-shell-placeholders
cargo clippy -p alephcore --all-targets 2>&1 | tail -20
```
Expected: 本期触及的文件零新警告（与 main 同命令对比告警文件名）。

- [ ] **Step 7: 变异检查（判据 #18：先看红的名单是不是预期那份）**

逐个临时改动 → 跑 `cargo test -p alephcore --lib <filter>` → 记录红的**名字** → `git checkout -- <file>` 恢复：

| 变异 | 预期恰好变红 |
|---|---|
| `runner_impl.rs`：`let run_marker_id = uuid::Uuid::new_v4().to_string();` | `the_run_markers_carry_the_engine_run_id` |
| `dispatch.rs`：`let run_id = String::new();` | `dispatch_forwards_run_id` |
| `session_split.rs`：删 `.filter(|record| record.seq != opener_seq)` | `a_parents_opener_inside_the_tail_is_not_copied_into_the_child` |
| `session_split.rs`：`let run_id = uuid::Uuid::new_v4().to_string();` | `split_seeds_child_with_forked_summary_and_fresh_tail`、`a_parents_opener_inside_the_tail_is_not_copied_into_the_child` |
| `event_snap.rs`：`rposition` → `position` | `a_reused_run_id_snaps_to_the_opener_its_closer_pairs_with` |
| `run_loop/mod.rs`：恢复 `format!("hookstop-…")` | hookstop 那条 `run_loop::tests` |

`abandon` 的变异跑集成测试（worktree target，`-j 1`），预期恰好 `an_abandoned_interrupted_run_is_closed_under_its_own_run_id` 红。结果表写进最终报告。

- [ ] **Step 8: 提交文档（body 带 RELEASE NOTE）并合并**

用 Write 工具把 commit message 写进 `$SCRATCH/p1-msg.txt`（Bash 工具拒绝提到 `git` 的 heredoc）：
```
docs: record the single run identity (P1) in the locator, the QA stage and the r3 follow-ups

RELEASE NOTE:
- A run's start/finish markers and its billing record now carry the same run id. Session logs written before this release are read as they are; nothing is migrated.
- The one run that straddles the upgrade (old markers, new billing record) may still be billed twice; dev/QA data that was double-billed between 027eeb7d9 and 26c08d6ef is not corrected.
- If `doctor`'s repair runs in the moment between a run finishing and its billing record landing, the run's tokens are counted once but its cost and model name are not recorded.
```
```bash
git add -A docs qa
git commit -F $SCRATCH/p1-msg.txt
cd /d/Workspace/Aleph && git merge --ff-only worktree-run-identity-p1 && git log --oneline -1
```
Expected: fast-forward 成功。**不删** worktree（会话内 `git worktree remove` 会损坏 Shell）。

---

# Phase P3 — 戳与计是一个存储操作（spec §4）

> 在 P1 合并之后开始：P3 会改 Task 7 刚动过的 `session_projector.rs`，并行做只会制造合并冲突。

### Task 9: `RunBill`、trait 改名、SQLite 事务化

**Files:**
- Modify: `src/gateway/session_store/mod.rs`（`StampOutcome` doc :20；trait 方法 ~457-497）
- Modify: `src/gateway/session_store/sqlite_backend/mod.rs`（:591-633 实现；:432 注释；测试模块）
- Modify: `src/gateway/session_manager/ops/modify.rs:504`（`update_session_usage`）· `src/gateway/session_manager/ops/mod.rs`（re-export）
- Modify: `src/gateway/session_store/file_backend/mod.rs`（签名改名；暂时拒绝 `Some(bill)`）
- Modify: `src/gateway/session_projector.rs:801` · `src/gateway/session_projector/projector_sub/run_span.rs:238`（改名、传 `None`，行为不变）
- Test: `src/gateway/session_store/sqlite_backend/mod.rs` 测试模块

**Interfaces:**
- Consumes: 无
- Produces:
  - `pub struct RunBill { pub input_tokens: i64, pub output_tokens: i64, pub cost_usd: f64, pub model: Option<String>, pub model_provider: Option<String> }`（`crate::gateway::session_store::RunBill`，`#[derive(Debug, Clone, PartialEq)]`）
  - `async fn stamp_and_bill_in_range(&self, key: &SessionKey, after_seq: u64, before_seq: u64, metadata: &serde_json::Value, bill: Option<&RunBill>) -> Result<StampOutcome, SessionStoreError>`（默认 `Ok(StampOutcome::NoRowInRange)`）
  - `pub(crate) fn add_usage(conn: &rusqlite::Connection, key_str: &str, bill: &RunBill) -> rusqlite::Result<usize>`（`crate::gateway::session_manager::ops::add_usage`）
  - `SessionManager::notify_usage_updated(&self, key_str: &str)`（`pub(crate)`）

- [ ] **Step 0: P3 的 worktree 与基线（基于已合并 P1 的 main）**

```bash
cd /d/Workspace/Aleph && git status --porcelain && git log --oneline -1
git worktree add .claude/worktrees/run-identity-p3 -b worktree-run-identity-p3 main
```
然后在新 worktree 里重跑 Task 1 Step 2 的基线，结果存 `$SCRATCH/p3-baseline-reds.txt`；再量一次 V7（Task 1 Step 3）。

- [ ] **Step 1: `RunBill` 与 trait 改名**

`session_store/mod.rs`，`StampOutcome` 之后加：
```rust
/// One run's spend, accumulated onto the session row in the same operation
/// that stamps the run's row ([`SessionStore::stamp_and_bill_in_range`]).
#[derive(Debug, Clone, PartialEq)]
pub struct RunBill {
    pub input_tokens: i64,
    pub output_tokens: i64,
    /// The run's priced cost; 0.0 when it could not be priced.
    pub cost_usd: f64,
    pub model: Option<String>,
    pub model_provider: Option<String>,
}
```
`StampOutcome` 的 doc 首行改指 `stamp_and_bill_in_range`。trait 方法改名为 `stamp_and_bill_in_range`，参数末尾加 `_bill: Option<&RunBill>`；doc 保留原文（把 `stamp_assistant_metadata_in_range` 字样随之改名），在 `[`StampOutcome::AlreadyStamped`] is what makes billing idempotent` 那段之后加：
```rust
    ///
    /// `bill`, when present, is accumulated onto the session row in the SAME
    /// operation, and only on [`StampOutcome::Stamped`] — a replay reads
    /// `AlreadyStamped` and adds nothing. The stamp is the bill's idempotence
    /// guard, so the two must land together: a stamp that landed without its
    /// bill could never be billed again (FOLLOW-UP F10). If the bill cannot be
    /// applied — including a session with no row to add it to — the stamp
    /// must not land either, and the answer is `Err`, never a silent `Ok`.
    /// SQLite does both in one transaction; the file backend writes the
    /// transcript, then `metadata.json`, and rolls the stamp back under the
    /// same lock if the second write fails (a crash between the two writes
    /// under-bills once — ruling U3).
```

- [ ] **Step 2: `add_usage` 与 `notify_usage_updated`**

`ops/modify.rs`，`update_session_usage` 之前加自由函数与方法，并让 `update_session_usage` 复用它：
```rust
/// The session-row half of billing a run: add its tokens and cost, and pin
/// the model it ran on when it names one. Returns the rows changed — 0 means
/// there is no session row to bill, which the caller must not read as done.
/// Shared by `update_session_usage` and the transactional
/// `stamp_and_bill_in_range`, so the two cannot drift on what "billing" adds.
pub(crate) fn add_usage(
    conn: &rusqlite::Connection,
    key_str: &str,
    bill: &crate::gateway::session_store::RunBill,
) -> rusqlite::Result<usize> {
    let total = bill.input_tokens + bill.output_tokens;
    let mut sql = String::from(
        "UPDATE sessions SET input_tokens = input_tokens + ?, output_tokens = output_tokens + ?, total_tokens = total_tokens + ?, estimated_cost_usd = estimated_cost_usd + ?"
    );
    let mut params: Vec<&dyn rusqlite::ToSql> =
        vec![&bill.input_tokens, &bill.output_tokens, &total, &bill.cost_usd];
    if bill.model.is_some() {
        sql.push_str(", model = ?");
    }
    if bill.model_provider.is_some() {
        sql.push_str(", model_provider = ?");
    }
    sql.push_str(" WHERE key = ?");
    if let Some(ref m) = bill.model {
        params.push(m);
    }
    if let Some(ref mp) = bill.model_provider {
        params.push(mp);
    }
    params.push(&key_str);
    conn.execute(&sql, params.as_slice())
}
```
`update_session_usage` 的函数体改为（签名与 doc 不变，行为不变——它历来忽略 0 行）：
```rust
        let key_str = key.to_key_string();
        let bill = crate::gateway::session_store::RunBill {
            input_tokens,
            output_tokens,
            cost_usd,
            model: model.map(str::to_string),
            model_provider: model_provider.map(str::to_string),
        };
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;
        add_usage(&conn, &key_str, &bill)
            .map_err(|e| SessionManagerError::DatabaseError(format!("Usage update failed: {e}")))?;
        drop(conn);
        self.emit_session_updated(&key_str);
        Ok(())
```
同一个 `impl SessionManager` 块里加：
```rust
    /// Publish `session_updated` after a bill landed outside this module
    /// (`stamp_and_bill_in_range` lives on the store trait impl, which cannot
    /// reach `emit_session_updated`).
    pub(crate) fn notify_usage_updated(&self, key_str: &str) {
        self.emit_session_updated(key_str);
    }
```
`ops/mod.rs` 加 `pub(crate) use modify::add_usage;`。

- [ ] **Step 3: SQLite 实现（事务）**

`sqlite_backend/mod.rs` 的实现改名加参，函数体改为：
```rust
        let key_str = key.to_key_string();
        let run_id = metadata.get("run_id").and_then(|v| v.as_str());
        let metadata_json = serde_json::to_string(metadata)
            .map_err(|e| SessionStoreError::DatabaseError(format!("serialize metadata: {e}")))?;
        let db = |e: rusqlite::Error| SessionStoreError::DatabaseError(e.to_string());
        {
            let mut conn = self
                .conn
                .lock()
                .map_err(|e| SessionStoreError::DatabaseError(format!("Lock error: {e}")))?;
            // One transaction: the stamp and the bill land together or not at
            // all. Every early return below drops `tx` uncommitted — a rollback
            // of whatever it did, which on those paths is nothing or the stamp.
            let tx = conn.transaction().map_err(db)?;
            let target: Option<(i64, Option<String>)> = tx
                .query_row(
                    "SELECT id, metadata FROM messages
                      WHERE session_key = ?1 AND role = 'assistant'
                        AND source_seq IS NOT NULL AND source_seq > ?2 AND source_seq <= ?3
                      ORDER BY source_seq DESC LIMIT 1",
                    params![key_str, after_seq as i64, before_seq as i64],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(db)?;
            let Some((row_id, existing)) = target else {
                return Ok(StampOutcome::NoRowInRange);
            };
            if already_stamped_by(existing.as_deref(), run_id) {
                return Ok(StampOutcome::AlreadyStamped);
            }
            tx.execute(
                "UPDATE messages SET metadata = ?1 WHERE id = ?2",
                params![metadata_json, row_id],
            )
            .map_err(db)?;
            if let Some(bill) = bill {
                let changed = crate::gateway::session_manager::ops::add_usage(&tx, &key_str, bill)
                    .map_err(db)?;
                if changed == 0 {
                    return Err(SessionStoreError::NotFound(format!(
                        "no session row for {key_str}; the run is not billed and its row is not stamped"
                    )));
                }
            }
            tx.commit().map_err(db)?;
        }
        if bill.is_some() {
            self.notify_usage_updated(&key_str);
        }
        Ok(StampOutcome::Stamped)
```
方法 doc 追加一句：`Stamp and bill share one transaction on the connection mutex, so a failed bill rolls the stamp back (F10).` `:432` 的注释里 `stamp_assistant_metadata_in_range` 字样改名。`use` 行加 `RunBill`。

- [ ] **Step 4: file 后端先改名（暂拒 `Some(bill)`）与两处调用者**

`file_backend/mod.rs` 实现改名加参 `bill: Option<&RunBill>`，函数体第一行加：
```rust
        if bill.is_some() {
            // Task 10 lands the file backend's bill; until then refuse rather
            // than stamp and report a bill that was never written.
            return Err(SessionStoreError::Unsupported);
        }
```
文件内 5 处测试调用（`grep -n "stamp_assistant_metadata_in_range" src/gateway/session_store/file_backend/mod.rs`）改名并在末尾加 `None`；:310、:703 的注释改名。

`session_projector.rs:801`：`.stamp_assistant_metadata_in_range(id, ctx.run_start, rec.seq, &meta)` → `.stamp_and_bill_in_range(id, ctx.run_start, rec.seq, &meta, None)`；`run_span.rs:238` 同样加 `None`。两处的 doc 字样随之改名（`grep -rn stamp_assistant_metadata_in_range src` 应为零命中）。本任务里投影器仍走旧的「戳后 `bill_run_from_fold`」，行为不变——Task 11 才切换。

- [ ] **Step 5: 写 SQLite 测试**

`sqlite_backend/mod.rs` 测试模块末尾：
```rust
    fn stamp_bill(input: i64, output: i64) -> RunBill {
        RunBill {
            input_tokens: input,
            output_tokens: output,
            cost_usd: 0.12,
            model: Some("claude".into()),
            model_provider: Some("anthropic".into()),
        }
    }

    /// A session with one assistant row at source seq 2, as the projector
    /// writes it.
    async fn store_with_row(temp: &tempfile::TempDir, key: &SessionKey) -> SessionManager {
        let store = test_store(temp);
        SessionStore::get_or_create(&store, key).await.unwrap();
        SessionStore::append_message(
            &store,
            key,
            crate::gateway::session_store::types::MessageRecord {
                id: crate::session::projection::row_id(&key.to_key_string(), 2),
                role: "assistant".into(),
                content: "hello".into(),
                timestamp: 2,
                metadata: None,
                input_tokens: 0,
                output_tokens: 0,
                tool_call_id: None,
                tool_name: None,
            },
        )
        .await
        .unwrap();
        store
    }

    async fn row_run_id(store: &SessionManager, key: &SessionKey) -> Option<String> {
        SessionStore::get_history(store, key, None)
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.role == "assistant")
            .and_then(|m| m.metadata)
            .and_then(|m| m.get("run_id").and_then(|v| v.as_str()).map(str::to_string))
    }

    async fn tokens(store: &SessionManager, key: &SessionKey) -> (i64, i64) {
        let m = SessionStore::get_metadata(store, key).await.unwrap().unwrap();
        (m.input_tokens, m.output_tokens)
    }

    /// Review Focus 5: the stamp and the bill land together, and a replay of
    /// the same meta is `AlreadyStamped` and adds nothing.
    #[tokio::test]
    async fn a_stamp_and_its_bill_land_together_and_a_replay_adds_nothing() {
        let temp = tempdir().unwrap();
        let key = SessionKey::from_key_string("agent:stampbill:main").unwrap();
        let store = store_with_row(&temp, &key).await;
        let meta = serde_json::json!({ "run_id": "r" });
        let bill = stamp_bill(45, 25);
        for (pass, expected) in [(1, StampOutcome::Stamped), (2, StampOutcome::AlreadyStamped)] {
            assert_eq!(
                SessionStore::stamp_and_bill_in_range(&store, &key, 1, 4, &meta, Some(&bill))
                    .await
                    .unwrap(),
                expected,
                "pass {pass}"
            );
            assert_eq!(tokens(&store, &key).await, (45, 25), "pass {pass}");
        }
        let m = SessionStore::get_metadata(&store, &key).await.unwrap().unwrap();
        assert_eq!((m.estimated_cost_usd, m.model.as_deref()), (0.12, Some("claude")));
        assert_eq!(row_run_id(&store, &key).await.as_deref(), Some("r"));
    }

    /// F10: a bill the database refuses rolls the stamp back, so the next
    /// replay still finds the row unstamped and bills it.
    #[tokio::test]
    async fn a_refused_bill_leaves_the_row_unstamped_and_the_replay_bills_it() {
        let temp = tempdir().unwrap();
        let key = SessionKey::from_key_string("agent:refusedbill:main").unwrap();
        let store = store_with_row(&temp, &key).await;
        store
            .conn
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER fail_bill BEFORE UPDATE OF input_tokens ON sessions \
                 BEGIN SELECT RAISE(ABORT, 'injected bill failure'); END;",
            )
            .unwrap();
        let meta = serde_json::json!({ "run_id": "r" });
        let bill = stamp_bill(45, 25);
        assert!(
            SessionStore::stamp_and_bill_in_range(&store, &key, 1, 4, &meta, Some(&bill))
                .await
                .is_err()
        );
        assert_eq!(row_run_id(&store, &key).await, None, "the stamp rolled back with the bill");
        assert_eq!(tokens(&store, &key).await, (0, 0));

        store.conn.lock().unwrap().execute_batch("DROP TRIGGER fail_bill;").unwrap();
        assert_eq!(
            SessionStore::stamp_and_bill_in_range(&store, &key, 1, 4, &meta, Some(&bill))
                .await
                .unwrap(),
            StampOutcome::Stamped
        );
        assert_eq!(tokens(&store, &key).await, (45, 25));
    }

    /// Review Focus 3: a bill for a session with no row changes nothing and is
    /// an `Err` — never a stamp that would then guard a bill that never landed.
    #[tokio::test]
    async fn a_bill_for_a_session_with_no_row_is_refused_and_nothing_is_stamped() {
        let temp = tempdir().unwrap();
        let key = SessionKey::from_key_string("agent:norowbill:main").unwrap();
        let store = store_with_row(&temp, &key).await;
        store
            .conn
            .lock()
            .unwrap()
            .execute_batch(&format!(
                "PRAGMA foreign_keys = OFF; DELETE FROM sessions WHERE key = '{}';",
                key.to_key_string()
            ))
            .unwrap();
        let rows: i64 = store
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "precondition: the message row survived the delete");

        let result = SessionStore::stamp_and_bill_in_range(
            &store,
            &key,
            1,
            4,
            &serde_json::json!({ "run_id": "r" }),
            Some(&stamp_bill(45, 25)),
        )
        .await;
        assert!(matches!(result, Err(SessionStoreError::NotFound(_))), "{result:?}");
        let metadata: Option<String> = store
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT metadata FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(metadata, None, "the stamp rolled back");
    }
```
（`get_history` 在 `messages` 行的 session 被删后可能报错——第三个测试因此直接读 `messages` 表。）

- [ ] **Step 6: 跑、确认绿；再变异确认能红**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib session_store session_manager session_projector 2>&1 | tail -8
```
Expected: 全绿（投影器行为未变）。两条变异，各自跑 `cargo test -p alephcore --lib sqlite_backend`、记下红的名字、`git checkout -- src/gateway/session_store/sqlite_backend/mod.rs` 恢复：

| 变异 | 预期恰好变红 |
|---|---|
| 删掉 `if changed == 0 { return Err(..) }` 块 | `a_bill_for_a_session_with_no_row_is_refused_and_nothing_is_stamped` |
| 把 `if let Some(bill) = bill { .. }` 整块移到 `tx.commit().map_err(db)?;` **之后**，并把其中的 `&tx` 换成 `&conn`（戳先提交、计另写——恢复成两步） | `a_refused_bill_leaves_the_row_unstamped_and_the_replay_bills_it`、`a_bill_for_a_session_with_no_row_is_refused_and_nothing_is_stamped` |

- [ ] **Step 7: 提交**

```bash
git add -u src
git commit -m "session_store: stamp_and_bill_in_range bills in the stamp's transaction on SQLite (F10)"
```

---

### Task 10: file 后端——同一把锁下戳、计、失败回滚

**Files:**
- Modify: `src/gateway/session_store/file_backend/meta.rs`（`MetaGuard::write_back`；`#[cfg(test)] failpoint`；`write()` 查 failpoint）
- Modify: `src/gateway/session_store/file_backend/mod.rs`（`update_session_usage` 1398 抽 `add_bill`；`stamp_and_bill_in_range` 1488；新 `write_transcript_locked`）
- Test: 同文件 `mod default_backend_parity_guards`；`meta.rs` 测试模块

**Interfaces:**
- Consumes: `RunBill`（Task 9）
- Produces: `MetaGuard::write_back(&mut self) -> Result<(), SessionStoreError>`；`meta::failpoint::fail_next_write(path: &Path)`（仅测试）

- [ ] **Step 1: `write_back` 与 failpoint**

`meta.rs`，`commit` 之后：
```rust
    /// Write the document back WITHOUT releasing the lock. For a caller that
    /// must still act under the same lock after the write — the file
    /// backend's `stamp_and_bill_in_range` rolls its transcript stamp back if
    /// this write fails, and that rollback must not race another writer.
    pub(crate) async fn write_back(&mut self) -> Result<(), SessionStoreError> {
        let Some(meta) = self.meta.as_ref() else {
            return Err(SessionStoreError::DatabaseError(
                "write_back with no metadata to write".to_string(),
            ));
        };
        write(&self.path, meta).await
    }
```
文件末尾（`mod tests` 之前）：
```rust
/// Test-only failure injection for `write`, keyed by path so parallel tests
/// cannot trip each other's.
#[cfg(test)]
pub(crate) mod failpoint {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    static ARMED: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

    /// The next `write` to `path` fails once.
    pub(crate) fn fail_next_write(path: &Path) {
        ARMED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(HashSet::new)
            .insert(path.to_path_buf());
    }

    pub(super) fn take(path: &Path) -> bool {
        ARMED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
            .is_some_and(|armed| armed.remove(path))
    }
}
```
`write()` 函数体第一行：
```rust
    #[cfg(test)]
    if failpoint::take(path) {
        return Err(SessionStoreError::DatabaseError(
            "injected metadata write failure".to_string(),
        ));
    }
```
`meta.rs` 测试模块加：
```rust
    /// `write_back` persists and keeps the lock: a second `lock` on the same
    /// key waits until the guard is dropped.
    #[tokio::test]
    async fn a_written_back_guard_still_holds_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s").join("metadata.json");
        let locks = MetaLocks::new();
        let mut guard = locks.lock("k", path.clone()).await.unwrap();
        guard.insert(meta_with_title("written back"));
        guard.write_back().await.unwrap();
        assert_eq!(
            read(&path).await.unwrap().unwrap().derived_title.as_deref(),
            Some("written back")
        );
        let second = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            locks.lock("k", path.clone()),
        )
        .await;
        assert!(second.is_err(), "the lock is still held after write_back");
        drop(guard);
        locks.lock("k", path).await.unwrap();
    }
```

- [ ] **Step 2: `add_bill` 与 `write_transcript_locked`**

`file_backend/mod.rs`：模块级自由函数
```rust
/// The metadata half of billing a run — the file twin of SQLite's
/// `session_manager::ops::add_usage`.
fn add_bill(meta: &mut SessionMetadata, bill: &RunBill) {
    meta.input_tokens += bill.input_tokens;
    meta.output_tokens += bill.output_tokens;
    meta.total_tokens += bill.input_tokens + bill.output_tokens;
    // The file backend serializes the whole struct, so unlike SQLite it
    // always HAD somewhere to put this — it just never had a writer.
    meta.estimated_cost_usd += bill.cost_usd;
    if let Some(m) = &bill.model {
        meta.model = Some(m.clone());
    }
    if let Some(mp) = &bill.model_provider {
        meta.model_provider = Some(mp.clone());
    }
}
```
`update_session_usage` 的 `if let Some(meta) = guard.existing_mut() { … }` 改为构造 `RunBill` 后 `add_bill(meta, &bill); guard.commit().await?;`（行为不变）。

`impl FileSessionStore` 里（`read_transcript` 附近）：
```rust
    /// Rewrite the whole transcript. Takes the session's `MetaGuard` by
    /// reference so it can only be called under the write lock — a whole-file
    /// rewrite without it silently loses a concurrent `append_message`.
    async fn write_transcript_locked(
        &self,
        _lock: &meta::MetaGuard,
        key_str: &str,
        messages: &[MessageRecord],
    ) -> Result<(), SessionStoreError> {
        let mut contents = String::new();
        for msg in messages {
            let line = serde_json::to_string(msg)
                .map_err(|e| SessionStoreError::DatabaseError(format!("Serialize failed: {e}")))?;
            contents.push_str(&line);
            contents.push('\n');
        }
        crate::utils::atomic_write::atomic_write_file(&self.transcript_path(key_str), &contents)
            .await
            .map_err(|e| SessionStoreError::DatabaseError(format!("Write transcript failed: {e}")))
    }
```

- [ ] **Step 3: 写 file 测试（先红：Task 9 的实现对 `Some(bill)` 返回 `Unsupported`）**

`default_backend_parity_guards` 模块末尾：
```rust
    fn file_bill() -> RunBill {
        RunBill {
            input_tokens: 45,
            output_tokens: 25,
            cost_usd: 0.12,
            model: Some("claude".into()),
            model_provider: None,
        }
    }

    async fn store_with_row(key_str: &str) -> (FileSessionStore, tempfile::TempDir, SessionKey) {
        let (store, dir) = temp_store();
        let key = SessionKey::from_key_string(key_str).unwrap();
        store.get_or_create(&key).await.unwrap();
        let mut record = msg("assistant", "hello");
        record.id = crate::session::projection::row_id(key_str, 2);
        store.append_message(&key, record).await.unwrap();
        (store, dir, key)
    }

    async fn stamped_id(store: &FileSessionStore, key_str: &str) -> Option<String> {
        store
            .read_transcript(key_str, None)
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.role == "assistant")
            .and_then(|m| m.metadata)
            .and_then(|m| m.get("run_id").and_then(|v| v.as_str()).map(str::to_string))
    }

    /// Review Focus 5 on the DEFAULT backend: stamp and bill in one call; a
    /// replay is `AlreadyStamped` and adds nothing.
    #[tokio::test]
    async fn the_default_backend_stamps_and_bills_once_and_a_replay_adds_nothing() {
        let key_str = "agent:filebill:main";
        let (store, _dir, key) = store_with_row(key_str).await;
        let meta = serde_json::json!({ "run_id": "r" });
        let bill = file_bill();
        for (pass, expected) in [(1, StampOutcome::Stamped), (2, StampOutcome::AlreadyStamped)] {
            assert_eq!(
                store
                    .stamp_and_bill_in_range(&key, 1, 4, &meta, Some(&bill))
                    .await
                    .unwrap(),
                expected,
                "pass {pass}"
            );
            let m = store.get_metadata(&key).await.unwrap().unwrap();
            assert_eq!((m.input_tokens, m.output_tokens), (45, 25), "pass {pass}");
        }
        assert_eq!(stamped_id(&store, key_str).await.as_deref(), Some("r"));
    }

    /// F10 on the default backend: `metadata.json` refuses the bill, the stamp
    /// is rolled back under the same lock, and the replay bills.
    #[tokio::test]
    async fn a_refused_metadata_write_rolls_the_stamp_back() {
        let key_str = "agent:filerollback:main";
        let (store, _dir, key) = store_with_row(key_str).await;
        let meta = serde_json::json!({ "run_id": "r" });
        let bill = file_bill();
        meta::failpoint::fail_next_write(&store.metadata_path(key_str));
        assert!(store
            .stamp_and_bill_in_range(&key, 1, 4, &meta, Some(&bill))
            .await
            .is_err());
        assert_eq!(stamped_id(&store, key_str).await, None, "rolled back");
        assert_eq!(store.get_metadata(&key).await.unwrap().unwrap().input_tokens, 0);

        assert_eq!(
            store
                .stamp_and_bill_in_range(&key, 1, 4, &meta, Some(&bill))
                .await
                .unwrap(),
            StampOutcome::Stamped
        );
        assert_eq!(store.get_metadata(&key).await.unwrap().unwrap().input_tokens, 45);
    }

    /// Review Focus 3 on the default backend: no `metadata.json` to bill ⇒
    /// `Err` before the transcript is touched.
    #[tokio::test]
    async fn a_bill_for_a_session_with_no_metadata_is_refused_before_the_stamp() {
        let key_str = "agent:filenometa:main";
        let (store, _dir, key) = store_with_row(key_str).await;
        std::fs::remove_file(store.metadata_path(key_str)).unwrap();
        let result = store
            .stamp_and_bill_in_range(&key, 1, 4, &serde_json::json!({ "run_id": "r" }), Some(&file_bill()))
            .await;
        assert!(matches!(result, Err(SessionStoreError::NotFound(_))), "{result:?}");
        assert_eq!(stamped_id(&store, key_str).await, None);
    }
```
（子模块能访问父模块的私有 `metadata_path` / `read_transcript`；若 `use` 缺 `RunBill` / `meta` / `SessionStoreError`，在该测试模块的 `use` 行补上。）

- [ ] **Step 4: 确认红**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib file_backend 2>&1 | tail -12
```
Expected: 三条新测试 FAIL（`Unsupported`）；`a_written_back_guard_still_holds_the_lock` PASS。

- [ ] **Step 5: 实现**

`stamp_and_bill_in_range` 函数体替换为（保留原 doc，并把「The guard is deliberately dropped WITHOUT `commit()`」那段改为「The guard is written back only when a bill rides along — see the rollback below」）：
```rust
        let key_str = key.to_key_string();
        let mut guard = self.lock_metadata(&key_str).await?;
        if bill.is_some() && guard.existing_mut().is_none() {
            // Nothing to bill onto. Refuse BEFORE the stamp: a stamp is the
            // bill's idempotence guard, and one landed without its bill could
            // never be billed again.
            return Err(SessionStoreError::NotFound(format!(
                "no metadata.json for {key_str}; the run is not billed and its row is not stamped"
            )));
        }
        let mut messages = self.read_transcript(&key_str, None).await?;
        let run_id = metadata.get("run_id").and_then(|v| v.as_str());
        let Some(idx) = messages.iter().rposition(|m| {
            m.role == "assistant"
                && crate::session::projection::parse_source_seq(&m.id, &key_str)
                    .is_some_and(|s| s > after_seq && s <= before_seq)
        }) else {
            return Ok(StampOutcome::NoRowInRange);
        };
        let existing = messages[idx]
            .metadata
            .as_ref()
            .map(serde_json::Value::to_string)
            .filter(|s| s != "null");
        if crate::gateway::session_store::sqlite_backend::already_stamped_by(
            existing.as_deref(),
            run_id,
        ) {
            return Ok(StampOutcome::AlreadyStamped);
        }
        let previous = std::mem::replace(&mut messages[idx].metadata, Some(metadata.clone()));
        self.write_transcript_locked(&guard, &key_str, &messages).await?;
        let Some(bill) = bill else {
            return Ok(StampOutcome::Stamped);
        };
        if let Some(meta) = guard.existing_mut() {
            add_bill(meta, bill);
        }
        if let Err(e) = guard.write_back().await {
            // Two files, two atomic writes, no atomicity between them. Undo the
            // stamp under the same lock so the replay finds the row unstamped
            // and bills it (F10). If the undo fails too, the row is stamped and
            // unbilled — the same direction as a crash between the writes (U3).
            messages[idx].metadata = previous;
            if let Err(undo) = self.write_transcript_locked(&guard, &key_str, &messages).await {
                tracing::warn!(
                    session = %key_str,
                    error = %undo,
                    "stamp rollback failed after a refused bill; this run's row is stamped but not billed"
                );
            }
            return Err(e);
        }
        Ok(StampOutcome::Stamped)
```
删掉 Task 9 加的 `Unsupported` 分支。

- [ ] **Step 6: 确认绿；变异确认能红**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib file_backend session_store 2>&1 | tail -8
```
Expected: 全绿。变异：删掉 `if let Err(e) = guard.write_back()` 臂里的回滚两行（只 `return Err(e)`），预期**恰好** `a_refused_metadata_write_rolls_the_stamp_back` 红；恢复。

- [ ] **Step 7: 提交**

```bash
git add -u src
git commit -m "session_store: the file backend bills under the stamp's lock and rolls the stamp back on failure (F10)"
```

---

### Task 11: 投影器先 fold 再戳，`BillOutcome` 取代 `billed: bool`

**Files:**
- Modify: `src/gateway/session_projector.rs`（`Projected` ~681；heal 消费 ~500；meta 臂 ~764-855；`bill_run_from_fold` ~900-980 → `fold_run_bill`）
- Modify: `src/gateway/session_projector/projector_sub/run_span.rs`（`synthesize_missing_stamps` 220-260；尾部 `use`）
- Modify: `src/gateway/session_projector/projector_sub/missed_seqs.rs:41`（`usage_rebilled` doc）
- Test: `src/gateway/session_projector.rs`（改 4 处断言；新增 3 个测试 + 1 个替身）

**Interfaces:**
- Consumes: `SessionStore::stamp_and_bill_in_range`、`RunBill`（Task 9）
- Produces:
  - `pub(crate) enum BillOutcome { Billed, NothingToBill, Unfoldable }`（`Debug, Clone, Copy, PartialEq, Eq`）
  - `Projected::Stamped { bill: BillOutcome }`
  - `enum FoldedBill { Owe(RunBill), Nothing, Unfoldable, ReadFailed }` + `fn plan(self) -> Option<(Option<RunBill>, BillOutcome)>`（`ReadFailed` ⇒ `None`）
  - `async fn fold_run_bill(id, meta_seq, ctx: &ProjectionCtx<'_>, run_id, cost_usd, model, provider) -> FoldedBill`

- [ ] **Step 1: 先改 4 处旧断言并加新测试（红）**

旧断言（`grep -n "Projected::Stamped {" src/gateway/session_projector.rs`）：
- `a_run_meta_stamps_the_row_inside_its_own_run`（~1663，`events: None`）→ `Projected::Stamped { bill: BillOutcome::Unfoldable }`；上方注释改为 `No log is pinned: this test is about WHERE the stamp lands; without a log there is nothing to fold, which the outcome names (Unfoldable), rather than reading as "billed nothing".`
- `a_meta_without_a_gauge_stamps_the_run_id_alone`（~1841）→ `BillOutcome::NothingToBill`
- `replaying_one_run_meta_bills_once`（~1911）→ `BillOutcome::Billed`
- `an_unanchored_meta_stamps_but_does_not_bill`（~1964）→ `BillOutcome::Unfoldable`

新替身（放在 `UnreadableRetirement` 之后，照抄它的方法集，只改两处）：
```rust
    /// An event log whose retirement answer is "live" but whose range read
    /// fails — the fold's read, not the retirement check, is what breaks.
    struct UnreadableRange;

    #[async_trait::async_trait]
    impl SessionEventStore for UnreadableRange {
        // append_batch / load_all_events / load_head_seq / retire_from /
        // load_run_markers: copied verbatim from `UnreadableRetirement`.
        async fn load_events_range(
            &self,
            _id: &SessionId,
            _from: Option<EventSeq>,
            _to: Option<EventSeq>,
        ) -> Result<Vec<SessionEventRecord>, SessionError> {
            Err(SessionError::Storage("event log unreadable".into()))
        }
        async fn is_retired(&self, _id: &SessionId, _seq: EventSeq) -> Result<bool, SessionError> {
            Ok(false)
        }
    }
```
（注释里那行「copied verbatim」是给实现者看的：把 `UnreadableRetirement` 的另外五个方法原样复制进来，然后删掉这行注释。）

新测试：
```rust
    /// F10: the fold is read BEFORE the stamp. A read that fails writes
    /// nothing — the seq is retried — instead of landing a stamp whose bill
    /// can then never be retried.
    #[tokio::test]
    async fn an_unreadable_fold_is_retried_and_stamps_nothing() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "unreadable_fold.db");
        let id = SessionId::ephemeral("unreadable-fold");
        store.get_or_create(&id).await.unwrap();
        append_assistant_row(&store, &id, 2).await;
        let events: Arc<dyn SessionEventStore> = Arc::new(UnreadableRange);
        let never = |_: EventSeq| false;
        let ctx = ProjectionCtx {
            store: &store,
            events: Some(&events),
            present: &never,
            run_start: 1,
            bus: None,
        };
        let tid = uuid::Uuid::new_v4();
        assert_eq!(
            project_event(&id, &rec(4, run_meta(tid, "run_a")), &ctx).await,
            Projected::Retry
        );
        let row = store
            .get_history(&id, None)
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.role == "assistant")
            .expect("the row is there");
        assert!(row.metadata.is_none(), "nothing stamped: {:?}", row.metadata);
    }

    /// F10 through the projector: the store refuses the bill, the meta is
    /// retried with its row unstamped, and the retry bills it once.
    #[tokio::test]
    async fn a_refused_bill_is_retried_and_the_retry_bills_once() {
        let temp = tempdir().unwrap();
        let manager = Arc::new(
            SessionManager::new(SessionManagerConfig {
                db_path: temp.path().join("refused_bill.db"),
                max_messages: 10_000,
                compaction_keep: 5_000,
                ..Default::default()
            })
            .unwrap(),
        );
        let store: Arc<dyn SessionStore> = manager.clone();
        let id = SessionId::ephemeral("refused-bill");
        store.get_or_create(&id).await.unwrap();
        append_assistant_row(&store, &id, 2).await;
        let tid = uuid::Uuid::new_v4();
        let log = own_event_log(&id, &one_billed_run(tid, "run_a")).await;
        let never = |_: EventSeq| false;
        let ctx = ProjectionCtx {
            store: &store,
            events: Some(&log),
            present: &never,
            run_start: 1,
            bus: None,
        };
        manager
            .conn
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER fail_bill BEFORE UPDATE OF input_tokens ON sessions \
                 BEGIN SELECT RAISE(ABORT, 'injected bill failure'); END;",
            )
            .unwrap();
        let meta_rec = rec(4, run_meta(tid, "run_a"));
        assert_eq!(project_event(&id, &meta_rec, &ctx).await, Projected::Retry);

        manager.conn.lock().unwrap().execute_batch("DROP TRIGGER fail_bill;").unwrap();
        assert_eq!(
            project_event(&id, &meta_rec, &ctx).await,
            Projected::Stamped { bill: BillOutcome::Billed }
        );
        let meta = store.get_metadata(&id).await.unwrap().unwrap();
        assert_eq!((meta.input_tokens, meta.output_tokens), (45, 25));
    }

    /// Review Focus 4: a meta whose opener is gone (retired by `/compact`)
    /// stamps its row and bills nothing — and the heal SAYS so: one stamp
    /// re-applied, zero rebilled, tokens untouched.
    #[tokio::test]
    async fn a_heal_counts_an_unfoldable_stamp_as_not_rebilled() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "unfoldable_heal.db");
        let id = SessionId::ephemeral("unfoldable-heal");
        store.get_or_create(&id).await.unwrap();
        let tid = uuid::Uuid::new_v4();
        let log = own_event_log(
            &id,
            &[(2, assistant_msg_billed(tid, 45, 25)), (4, run_meta(tid, "run_a"))],
        )
        .await;
        let pinned: Option<Arc<dyn SessionEventStore>> = Some(log);
        let missed = Arc::new(StdMutex::new(MissedSeqs::default()));
        let mut run_start = HashMap::new();
        let report = heal_session(
            &store,
            &id,
            &missed,
            &pinned,
            &mut run_start,
            HealScope::WholeSession,
        )
        .await;
        assert_eq!(
            (report.holes_filled, report.stamps_reapplied, report.usage_rebilled),
            (1, 1, 0),
            "{report:?}"
        );
        let meta = store.get_metadata(&id).await.unwrap().unwrap();
        assert_eq!((meta.input_tokens, meta.output_tokens), (0, 0));
    }
```
（`SessionManager` / `SessionManagerConfig` 已在测试模块里被 `sqlite_store` 使用；`conn` 是 `pub(super)`（`gateway` 子树可见），`session_projector` 在该子树内。）

- [ ] **Step 2: 确认红（编译失败即红）**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib session_projector --no-run 2>&1 | grep -E "^error" | head
```
Expected: `BillOutcome` 未定义、`Stamped` 没有 `bill` 字段。

- [ ] **Step 3: 类型**

`Projected` 之前加 `BillOutcome`：
```rust
/// What happened to a run's spend when its stamp landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BillOutcome {
    /// The fold found spend, and it was accumulated with the stamp.
    Billed,
    /// Nothing to add: no usage on the run's messages and no price.
    NothingToBill,
    /// Nothing to fold from — no event log installed, or no `RunStarted`
    /// before the meta (a `/compact` retired the opener, or a legacy log).
    /// The row is stamped and the spend is NOT accumulated, said at `warn!`
    /// rather than read as "billed nothing" (criterion #8).
    Unfoldable,
}
```
`Projected::Stamped` 改为：
```rust
    /// An `AssistantRunMeta` stamp landed on a row that had none, together
    /// with whatever the run owed (`bill`).
    Stamped { bill: BillOutcome },
```
heal 消费处（~502）：
```rust
            Projected::Stamped { bill } => {
                report.stamps_reapplied += 1;
                if bill == BillOutcome::Billed {
                    report.usage_rebilled += 1;
                }
            }
```

- [ ] **Step 4: `fold_run_bill` 取代 `bill_run_from_fold`**

删 `bill_run_from_fold`，换成：
```rust
/// What the usage fold says a run owes, read BEFORE its stamp is written.
pub(crate) enum FoldedBill {
    Owe(RunBill),
    Nothing,
    Unfoldable,
    /// The slice could not be read. Nothing may be written: the stamp is the
    /// bill's idempotence guard, so stamping now would forfeit the bill.
    ReadFailed,
}

impl FoldedBill {
    /// The bill to hand the store and the outcome to report if the stamp
    /// lands; `None` = the fold failed and the caller writes nothing.
    pub(crate) fn plan(self) -> Option<(Option<RunBill>, BillOutcome)> {
        match self {
            Self::Owe(bill) => Some((Some(bill), BillOutcome::Billed)),
            Self::Nothing => Some((None, BillOutcome::NothingToBill)),
            Self::Unfoldable => Some((None, BillOutcome::Unfoldable)),
            Self::ReadFailed => None,
        }
    }
}

/// Fold the run's spend from the log slice `[ctx.run_start, meta_seq)`,
/// anchored on the last `RunStarted` in it
/// ([`crate::session::usage_fold::run_usage_totals`]). The cost and model are
/// the meta's own — `None` from [`synthesize_missing_stamps`], which has no
/// meta and passes the run's `RunFinished` seq as `meta_seq`. Read BEFORE the
/// stamp (F10): the log is append-only, so the slice is fixed once the meta
/// exists, and a read that fails leaves nothing half-written.
///
/// `ctx.run_start == 0` ⇒ the slice starts at the log head and the fold
/// anchors on the last `RunStarted` it finds — the restarted-drain case,
/// where this process never saw the marker go by.
pub(crate) async fn fold_run_bill(
    id: &SessionId,
    meta_seq: EventSeq,
    ctx: &ProjectionCtx<'_>,
    run_id: &str,
    cost_usd: Option<f64>,
    model: Option<&str>,
    provider: Option<&str>,
) -> FoldedBill {
    let Some(events) = ctx.events else {
        return FoldedBill::Unfoldable;
    };
    let slice = match events
        .load_events_range(id, Some(ctx.run_start), Some(meta_seq))
        .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(session = ?id, run_id, error = %e, "projector: usage fold read failed; nothing written, retried");
            return FoldedBill::ReadFailed;
        }
    };
    let Some(totals) = crate::session::usage_fold::run_usage_totals(&slice) else {
        return FoldedBill::Unfoldable;
    };
    if totals.without_usage > 0 {
        tracing::debug!(
            session = ?id,
            run_id,
            with_usage = totals.with_usage,
            without_usage = totals.without_usage,
            "projector: run bill is a floor; some assistant messages carried no usage"
        );
    }
    if totals.input == 0 && totals.output == 0 && cost_usd.is_none() {
        return FoldedBill::Nothing;
    }
    FoldedBill::Owe(RunBill {
        input_tokens: i64::try_from(totals.input).unwrap_or(i64::MAX),
        output_tokens: i64::try_from(totals.output).unwrap_or(i64::MAX),
        cost_usd: cost_usd.unwrap_or(0.0),
        model: model.map(str::to_string),
        model_provider: provider.map(str::to_string),
    })
}
```
（`use crate::gateway::session_store::RunBill;` 加到文件顶部的 `use` 里。）

- [ ] **Step 5: meta 臂**

`let Some(meta) = …build_message_metadata… else { return Projected::Nothing; };` 之后、`match ctx.store.stamp_and_bill_in_range(..)` 之前插入：
```rust
            // Fold BEFORE the stamp (F10): the stamp is the bill's idempotence
            // guard, so a stamp that landed without its bill could never be
            // billed again. A failed read writes nothing and retries.
            let Some((bill, if_stamped)) = fold_run_bill(
                id,
                rec.seq,
                ctx,
                run_id,
                *cost_usd,
                model.as_deref(),
                model_provider.as_deref(),
            )
            .await
            .plan()
            else {
                return Projected::Retry;
            };
```
调用改为 `.stamp_and_bill_in_range(id, ctx.run_start, rec.seq, &meta, bill.as_ref())`。`Ok(StampOutcome::Stamped)` 臂整个替换为：
```rust
                Ok(StampOutcome::Stamped) => {
                    // The spend landed with the stamp (or there was none):
                    // one store operation, so a replay reads `AlreadyStamped`
                    // and never reaches this arm again.
                    if if_stamped == BillOutcome::Unfoldable {
                        tracing::warn!(
                            session = ?id,
                            run_id = %run_id,
                            "projector: run meta stamped, but its spend cannot be folded \
                             (no event log, or no RunStarted before it); not accumulated"
                        );
                    }
                    Projected::Stamped { bill: if_stamped }
                }
```
`Err(e)` 臂的消息改为 `"projector: stamp-and-bill of run-meta failed; nothing landed, retried"`。`NoRowInRange` 臂注释里「Not billed here: the stamp is the bill's idempotence guard and it has not landed — the pass that lands it bills.」保留。

- [ ] **Step 6: `synthesize_missing_stamps`**

循环体里 `let Some(meta) = …` 之后到 `match` 结束替换为：
```rust
        let ctx = ProjectionCtx {
            store,
            events: Some(event_store),
            present,
            run_start: span.start,
            bus: None,
        };
        // Folded first, for the same reason as the meta arm (F10).
        let Some((bill, if_stamped)) = fold_run_bill(id, end, &ctx, &span.run_id, None, None, None)
            .await
            .plan()
        else {
            report.errored = true;
            continue;
        };
        match store
            .stamp_and_bill_in_range(id, span.start, end, &meta, bill.as_ref())
            .await
        {
            Ok(StampOutcome::Stamped) => {
                report.stamps_synthesized += 1;
                if if_stamped == BillOutcome::Billed {
                    report.usage_rebilled += 1;
                }
            }
            Ok(StampOutcome::AlreadyStamped | StampOutcome::NoRowInRange) => {}
            Err(e) => {
                tracing::warn!(
                    session = ?id,
                    run_id = %span.run_id,
                    error = %e,
                    "heal: synthesized stamp-and-bill failed"
                );
                report.errored = true;
            }
        }
```
尾部 `use super::super::{bill_run_from_fold, ProjectionCtx};` → `use super::super::{fold_run_bill, BillOutcome, ProjectionCtx};`。doc 里「The bill is [`bill_run_from_fold`] with `run_start = start` …」改为「The bill is [`fold_run_bill`] over `(start, end]`, handed to `stamp_and_bill_in_range` so it lands with the stamp …」，并加一句：「A fold that cannot be read synthesizes nothing and sets `errored`.」

`missed_seqs.rs` 的 `usage_rebilled` doc 末尾加：`Only a stamp whose bill was Billed counts; NothingToBill and Unfoldable stamps are counted in the stamp counters alone.`

- [ ] **Step 7: 跑、确认绿**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib gateway::session_projector gateway::projection_reconciler session_store 2>&1 | tail -10
```
Expected: 全绿，包括 P1 Task 7 的 4 个新测试与 `projection_reconciler.rs` 的两条（它们只看 `RepairReport`）。

- [ ] **Step 8: 变异确认能红**

| 变异 | 预期恰好变红 |
|---|---|
| meta 臂：`.plan()` 的 `else { return Projected::Retry; }` 改成先 `stamp_and_bill_in_range(.., None)` 再 fold（即恢复「先戳后计」的顺序，账单走 `update_session_usage`） | `an_unreadable_fold_is_retried_and_stamps_nothing`、`a_refused_bill_is_retried_and_the_retry_bills_once` |
| `FoldedBill::plan`：`ReadFailed => Some((None, BillOutcome::NothingToBill))` | `an_unreadable_fold_is_retried_and_stamps_nothing` |
| heal：`if bill == BillOutcome::Billed` → `if bill != BillOutcome::NothingToBill` | `a_heal_counts_an_unfoldable_stamp_as_not_rebilled` |

每条跑完 `git checkout -- <file>`。

- [ ] **Step 9: 提交**

```bash
git add -u src
git commit -m "gateway: the projector folds before it stamps and reports what the stamp billed (F10)"
```

---

### Task 12: P3 文档、QA、验证、合并

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md`（§4.13a ㉖ 的已知限制 (c)；§6.9 若提到 `stamp_assistant_metadata_in_range` 则改名）
- Modify: `docs/superpowers/plans/2026-09-12-persistence-wal-recovery-r3.md`（FOLLOW-UP F10）
- Modify: `docs/superpowers/specs/2026-09-24-run-identity-a5-design.md`（§8 P3 行标「已落地 <sha>」）

**Interfaces:**
- Consumes: Task 9–11
- Produces: main 上的 P3

- [ ] **Step 1: 确认在 P3 worktree（Task 9 Step 0 建的）**

```bash
cd /d/Workspace/Aleph/.claude/worktrees/run-identity-p3 && git status --porcelain && git log --oneline -5
```

- [ ] **Step 2: 文档**

- FL §4.13a ㉖：已知限制 (c)「billed:false 的 meta 无修复路径」改为「已关闭（<sha>）：`stamp_and_bill_in_range` 让戳与计同落；fold 先行；`BillOutcome`」，并写明 U3（file 后端两写之间崩溃少计一次）。
- `grep -rn "stamp_assistant_metadata_in_range\|bill_run_from_fold\|billed: bool\|Stamped { billed" docs src` 应为零命中（历史 spec/plan 除外——它们是带日期的记录，不改）。
- r3 计划 F10 就地划掉，`**已关闭**（<sha>）：…`。

- [ ] **Step 3: 验证集**

同 Task 8 Step 4，把 `p1` 换成 `p3`；集成测试挑 `resume_coordinator_integration` 与 `gateway_chat_through_orchestrator`。另跑 `cargo test -p alephcore --lib file_backend::meta`。Expected: `--lib` 红名单与 P3 基线 diff 为空；V7 值不变。

- [ ] **Step 4: 真机 QA（FL 规则：改了 `session_projector.rs`，review 前必跑；在 clippy 之前）**

```bash
bash qa/resume_boundary/run.sh holes knobs claims 2>&1 | tee $SCRATCH/p3-qa.log | tail -40
```
Expected: 全 PASS。`holes` 的「heal pass added no tokens」与 P1 加的 run-id 两条都必须在。

- [ ] **Step 5: clippy**

同 Task 8 Step 6。

- [ ] **Step 6: 提交文档（RELEASE NOTE）并合并**

`$SCRATCH/p3-msg.txt`（Write 工具）：
```
docs: record stamp-and-bill as one store operation (P3) and close F10

RELEASE NOTE:
- A run's billing now lands in the same store operation that marks its message as billed. A transient storage error no longer leaves a run marked as billed with its tokens never added; the run is retried and billed once.
- With the default file session store, a process crash that lands exactly between writing the transcript and writing the session metadata still under-bills that one run (never double-bills).
```
```bash
git add -A docs
git commit -F $SCRATCH/p3-msg.txt
cd /d/Workspace/Aleph && git merge --ff-only worktree-run-identity-p3 && git log --oneline -1
```

- [ ] **Step 7: 记忆**

更新 `C:\Users\zou\.claude\projects\D--Workspace-Aleph\memory\persistence-r3-round.md` 的 2026-09-24 段：F1 F14 F10 已关（写 sha），F26 F29 F30 随 P2 延后（U4）；`MEMORY.md` 索引行同步。
