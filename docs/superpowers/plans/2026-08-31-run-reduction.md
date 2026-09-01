# Run Reduction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把「一次 run 处在什么状态」从三处各自为政的推导收敛成 `src/session/reduction.rs` 里的一个纯函数，让崩溃边界和进展证据第一次有单一真源。

**Architecture:** 新增纯函数 `reduce_disposition(&[SessionEventRecord]) -> RunDisposition` 与 `reduce_run(&[SessionEventRecord]) -> RunReduction`（零 I/O、零 async、零全局状态）。三个既有服务端消费者改用它：`ResumeCoordinator`（合并它自己的两处推导，并按归属给出两种措辞的边界修复）、`ProjectionReconciler`（换推导）、`subagent_tool::recovery`（`Interrupted` 携带进展证据，只在详情面加载）。删除 `classify_markers` / `ScanVerdict` / `compute_boundary_repairs`。

**Tech Stack:** Rust（`alephcore`）、tokio、serde、proptest 1.4（已在 `Cargo.toml:343` 的 dev-deps）、bash + python3 的 `qa/` 真机装置。

**Spec:** `docs/superpowers/specs/2026-08-31-run-reduction-design.md`

## Global Constraints

- **分支隔离**：全程在 worktree `../Aleph-run-reduction`、分支 `run-reduction`。**严禁触碰 `main`**。
- **worktree 必须自带 target dir**：`.cargo/config.toml` 钉了一个共享绝对 target dir。每次跑 cargo 前确保 `export CARGO_TARGET_DIR=/Volumes/TBU4/Workspace/Aleph-run-reduction/target`，否则「我测过了」测的是另一棵 worktree 的字节。
- **R10**：`src/harness/` 一个字节都不改。归约住在 `src/session/`。
- **R7**：归约只陈述事实，**不得**用进展证据替模型做重跑决策。
- 提交信息格式 `<scope>: <description>`，英文。例：`session: add RunReduction pure reducer`。
- 注释与代码标识符用英文；本计划与 spec 用中文。
- 每个任务结束时工作树必须干净（`git status --porcelain` 为空）。
- **禁止** `git checkout` / `git restore` / `git stash` 任何不是本任务写的文件。

---

## File Structure

| 文件 | 职责 | 动作 |
|---|---|---|
| `src/session/reduction.rs` | `RunDisposition` / `DanglingCall` / `DanglingProvenance` / `RunProgress` / `RunReduction` + 两个纯归约函数。整个仓库里「什么算中断」「哪些调用悬空」「这次 run 做成了什么」的唯一字面表达 | **新建** |
| `src/session/mod.rs` | 声明并 re-export `reduction` | 修改 |
| `src/gateway/resume_coordinator.rs` | 删两处推导，改用归约；`repair_text` 按归属出两种措辞 | 修改 |
| `src/gateway/projection_reconciler.rs` | 换推导；改写说谎的模块 doc | 修改 |
| `src/gateway/session_projector.rs` | 改写说谎的模块 doc | 修改 |
| `src/agents/subagent_tool/recovery.rs` | `Recovered::Interrupted` 携带 `SessionKey` 与 `Option<RunProgress>`；只在详情面加载子会话日志 | 修改 |
| `qa/resume_boundary/run.sh` | 真机装置驱动，两阶段 | **新建** |
| `qa/resume_boundary/drive_dangle.py` | 驱动一次会悬空的工具调用，并自证它真的悬空了 | **新建** |
| `qa/resume_boundary/assert_repairs.py` | 对事件日志与 mock 的 request log 下断言 | **新建** |
| `docs/reference/FEATURE_LOCATOR.md` | §4.13a 增补；附录 E.0 触发器 | 修改 |

---

### Task 1: 纯归约器的骨架 — 处置、锚点、悬空归属

**Files:**
- Create: `src/session/reduction.rs`
- Modify: `src/session/mod.rs:9-30`（模块声明区与 `pub use` 区）
- Test: `src/session/reduction.rs`（同文件 `#[cfg(test)] mod tests`，与 `projection.rs` 同惯例）

**Interfaces:**
- Consumes: `crate::session::events::{EventSeq, SessionEvent, SessionEventRecord, Timestamp, TurnId}`
- Produces:
  - `pub enum RunDisposition { Clean, Interrupted { trailing_starts: usize } }`（`Debug, Clone, Copy, PartialEq, Eq`）
  - `pub enum DanglingProvenance { ThisRestart, EarlierRun }`（`Debug, Clone, Copy, PartialEq, Eq`）
  - `pub struct DanglingCall { pub call_id: String, pub tool_name: String, pub turn_id: TurnId, pub provenance: DanglingProvenance }`
  - `pub struct RunProgress { pub tool_calls_dispatched: usize, pub tool_calls_answered: usize, pub assistant_messages: usize, pub last_activity_at: Option<Timestamp> }`（`Default`）
  - `pub struct RunReduction { pub disposition: RunDisposition, pub run_anchor: Option<EventSeq>, pub run_id: Option<String>, pub dangling: Vec<DanglingCall>, pub progress: RunProgress }`
  - `pub fn reduce_disposition(markers: &[SessionEventRecord]) -> RunDisposition`
  - `pub fn reduce_run(events: &[SessionEventRecord]) -> RunReduction`
- 本任务里 `RunProgress` 永远是 `RunProgress::default()`；填充在 Task 2。

- [ ] **Step 1: 建文件，写下类型与两个函数的签名（先只让 `reduce_disposition` 有真实现）**

新建 `src/session/reduction.rs`：

```rust
//! `RunReduction` — the one derivation of "what state is this run in".
//!
//! Three call sites used to answer this question in three different ways:
//! `resume_coordinator::classify_markers` (counted trailing `RunStarted`
//! markers), `resume_coordinator::compute_boundary_repairs` (scanned the whole
//! log for unanswered `ToolCallRequested`), and
//! `subagent_tool::recovery::classify` (matched `SubagentSpawned` against
//! `SubagentReturned`). None of them produced a named thing that said what
//! state the run was in, so "interrupted" meant three subtly different
//! predicates depending on who asked.
//!
//! Both functions here are **pure**: no I/O, no `async`, no globals. That is
//! what makes them falsifiable by mutation — a reduction that lived behind a
//! store trait would have one implementation per backend, and two shapes of
//! the same rule cancel each other out.
//!
//! Deliberately NOT in `src/harness/`: this is a read face over durable facts,
//! not Think→Act turn scheduling. R10's 12-file lock and `budget.rs::CEILING`
//! ratchet are untouched.

use crate::session::events::{EventSeq, SessionEvent, SessionEventRecord, Timestamp, TurnId};

/// How a session's run-marker tail reads.
///
/// **Deliberately two variants.** A third (`NeverStarted`, for a legacy log
/// with no run markers at all) was considered and rejected: no consumer today
/// would treat it differently from `Clean`, and a variant with no reader is a
/// claim the enum cannot honour — the same reason `ApprovalSource::Autoconfirm`
/// and six `ErrorKind` variants were removed (see `events.rs`). The next
/// variant arrives in the same commit as the consumer that reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunDisposition {
    /// Newest marker is `RunFinished` — nothing to recover.
    Clean,
    /// Interrupted; `trailing_starts` counts the consecutive `RunStarted`
    /// events after the last `RunFinished` (the crash-loop attempt counter).
    Interrupted { trailing_starts: usize },
}

/// Which run a dangling tool call belonged to.
///
/// This is the difference between a true sentence and a false one. Every
/// dangling call used to be told "the server restarted after this call was
/// dispatched", which is a lie about any call left over from an *earlier* run
/// that was never repaired — reachable when the crash happened while
/// `[resume] enabled = false`, or when a session aged past the recency filter
/// and was later resumed by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DanglingProvenance {
    /// Dispatched by the run that is being recovered right now.
    ThisRestart,
    /// Left over from an earlier run in the same session.
    ///
    /// Also the answer when the log carries no `RunStarted` at all (a legacy
    /// session, or a child that died before its run marker was durable): there
    /// is no current run for the call to belong to, so the weaker claim is the
    /// honest one. An unknown provenance must not be read as "this restart".
    EarlierRun,
}

/// A tool call that crossed the dispatch line and never got a receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DanglingCall {
    pub call_id: String,
    pub tool_name: String,
    pub turn_id: TurnId,
    pub provenance: DanglingProvenance,
}

/// What a run got done before it stopped. Scoped to the current run — see
/// [`reduce_run`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunProgress {
    pub tool_calls_dispatched: usize,
    /// Never greater than `tool_calls_dispatched`: this counts dispatched
    /// calls that got an answer, not answer events.
    pub tool_calls_answered: usize,
    pub assistant_messages: usize,
    /// `created_at_ms` of the last record in scope — the *recording* time, not
    /// a max over payload timestamps. The question is "when was it last
    /// alive", and recording order is the authoritative order.
    pub last_activity_at: Option<Timestamp>,
}

/// Everything the three consumers need to know about one session's runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReduction {
    pub disposition: RunDisposition,
    /// `seq` of the last `RunStarted`. A **seq**, not an index: `reduce_run` is
    /// fed both whole logs (`load_all_events`) and pages
    /// (`get_events(id, from, to)`), and an index means different things in
    /// the two while a seq means the same thing in both.
    pub run_anchor: Option<EventSeq>,
    /// `run_id` of the last `RunStarted`.
    pub run_id: Option<String>,
    pub dangling: Vec<DanglingCall>,
    pub progress: RunProgress,
}

/// The one derivation of "is this interrupted".
///
/// `markers` is a run-marker sequence in `seq` order — either straight from
/// `SessionEventStore::load_run_markers`, or the marker subsequence of a full
/// log (which is what [`reduce_run`] hands it, so the two can never drift).
#[must_use]
pub fn reduce_disposition(markers: &[SessionEventRecord]) -> RunDisposition {
    let mut trailing_starts = 0usize;
    for record in markers.iter().rev() {
        match &record.event {
            SessionEvent::RunStarted { .. } => trailing_starts += 1,
            SessionEvent::RunFinished { .. } => break,
            // `load_run_markers` only ever returns run markers, but a caller
            // may hand a full log: a non-marker breaks the trailing run.
            _ => break,
        }
    }
    if trailing_starts == 0 {
        RunDisposition::Clean
    } else {
        RunDisposition::Interrupted { trailing_starts }
    }
}

/// Reduce a session's event log to its run state.
///
/// Two passes, and no assumption that `events` is sorted by `seq`:
/// pass one finds the anchor and the answered set, pass two attributes the
/// dangling calls and (Task 2) counts progress.
#[must_use]
pub fn reduce_run(events: &[SessionEventRecord]) -> RunReduction {
    use std::collections::HashSet;

    // Pass 1: the anchor, the run id, and every call id that got an answer.
    let mut run_anchor: Option<EventSeq> = None;
    let mut run_id: Option<String> = None;
    let mut answered: HashSet<&str> = HashSet::new();
    let mut markers: Vec<SessionEventRecord> = Vec::new();
    for record in events {
        match &record.event {
            SessionEvent::RunStarted { run_id: rid, .. } => {
                run_anchor = Some(record.seq);
                run_id = Some(rid.clone());
                markers.push(record.clone());
            }
            SessionEvent::RunFinished { .. } => markers.push(record.clone()),
            SessionEvent::ToolResult { call_id, .. } | SessionEvent::ToolError { call_id, .. } => {
                answered.insert(call_id.as_str());
            }
            _ => {}
        }
    }

    // The disposition is not recomputed here — it is asked of the one function
    // that owns the question. G1 (proptest) pins that.
    let disposition = reduce_disposition(&markers);

    // Pass 2: attribute the dangling calls.
    let mut dangling = Vec::new();
    for record in events {
        if let SessionEvent::ToolCallRequested {
            turn_id,
            call_id,
            name,
            ..
        } = &record.event
        {
            if answered.contains(call_id.as_str()) {
                continue;
            }
            let provenance = match run_anchor {
                Some(anchor) if record.seq > anchor => DanglingProvenance::ThisRestart,
                _ => DanglingProvenance::EarlierRun,
            };
            dangling.push(DanglingCall {
                call_id: call_id.clone(),
                tool_name: name.clone(),
                turn_id: *turn_id,
                provenance,
            });
        }
    }

    RunReduction {
        disposition,
        run_anchor,
        run_id,
        dangling,
        progress: RunProgress::default(),
    }
}
```

- [ ] **Step 2: 在 `src/session/mod.rs` 声明模块并 re-export**

在模块声明区（`pub mod projection;` 之后，按字母序）加一行：

```rust
pub mod reduction;
```

在 `pub use projection::project_row;` 之后加：

```rust
pub use reduction::{
    reduce_disposition, reduce_run, DanglingCall, DanglingProvenance, RunDisposition, RunProgress,
    RunReduction,
};
```

- [ ] **Step 3: 写失败的测试（G1 / G2 / G2b + 处置基线）**

在 `src/session/reduction.rs` 末尾追加：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{MessageContent, RunOutcome};

    fn rec(seq: EventSeq, event: SessionEvent) -> SessionEventRecord {
        SessionEventRecord {
            seq,
            event,
            created_at_ms: seq as i64 * 10,
        }
    }

    fn started(run: &str) -> SessionEvent {
        SessionEvent::RunStarted {
            run_id: run.to_string(),
            at: 1,
            project_root: None,
        }
    }

    fn finished(run: &str) -> SessionEvent {
        SessionEvent::RunFinished {
            run_id: run.to_string(),
            outcome: RunOutcome::Completed,
            at: 2,
        }
    }

    fn requested(call: &str) -> SessionEvent {
        SessionEvent::ToolCallRequested {
            turn_id: TurnId::new_v4(),
            call_id: call.to_string(),
            name: "bash_exec".to_string(),
            input: serde_json::json!({}),
            at: 3,
        }
    }

    fn result_for(call: &str) -> SessionEvent {
        SessionEvent::ToolResult {
            turn_id: TurnId::new_v4(),
            call_id: call.to_string(),
            output: crate::session::events::ToolOutput {
                value: serde_json::json!("ok"),
                metadata: Default::default(),
            },
            at: 4,
        }
    }

    fn assistant(text: &str) -> SessionEvent {
        SessionEvent::AssistantMessage {
            turn_id: TurnId::new_v4(),
            content: MessageContent {
                text: text.to_string(),
                blocks: vec![],
                thinking: None,
                thinking_signature: None,
            },
            usage: None,
            at: 5,
        }
    }

    #[test]
    fn disposition_is_clean_when_the_newest_marker_finished() {
        let markers = vec![rec(1, started("a")), rec(2, finished("a"))];
        assert_eq!(reduce_disposition(&markers), RunDisposition::Clean);
    }

    #[test]
    fn disposition_counts_the_trailing_starts() {
        let markers = vec![
            rec(1, started("a")),
            rec(2, finished("a")),
            rec(3, started("b")),
            rec(4, started("c")),
        ];
        assert_eq!(
            reduce_disposition(&markers),
            RunDisposition::Interrupted { trailing_starts: 2 }
        );
    }

    /// G2 — the REACHABLE shape. Run `a` crashed while `[resume] enabled` was
    /// false, so nothing repaired `c1`; the user then sent a message, opening
    /// run `b`, which also crashed leaving `c2`. The two calls must not be
    /// told the same story.
    #[test]
    fn dangling_calls_are_attributed_to_their_own_run() {
        let events = vec![
            rec(1, started("a")),
            rec(2, requested("c1")),
            rec(3, started("b")),
            rec(4, requested("c2")),
        ];
        let r = reduce_run(&events);
        assert_eq!(r.run_anchor, Some(3));
        assert_eq!(r.run_id.as_deref(), Some("b"));
        assert_eq!(r.dangling.len(), 2);
        assert_eq!(r.dangling[0].call_id, "c1");
        assert_eq!(r.dangling[0].provenance, DanglingProvenance::EarlierRun);
        assert_eq!(r.dangling[1].call_id, "c2");
        assert_eq!(r.dangling[1].provenance, DanglingProvenance::ThisRestart);
    }

    /// G2b — the invariant-violation shape: a run that ended CLEANLY yet left a
    /// dangling call, i.e. one of `close_unexecuted_tool_uses` /
    /// `emit_deferred_tool_results` / the approval path failed to close it.
    /// The reduction must report the fact rather than swallow it, and must not
    /// upgrade it to "this restart".
    #[test]
    fn a_dangling_call_under_a_finished_run_is_reported_as_earlier() {
        let events = vec![
            rec(1, started("a")),
            rec(2, requested("c1")),
            rec(3, finished("a")),
            rec(4, started("b")),
        ];
        let r = reduce_run(&events);
        assert_eq!(
            r.disposition,
            RunDisposition::Interrupted { trailing_starts: 1 }
        );
        assert_eq!(r.dangling.len(), 1, "the fact must not be swallowed");
        assert_eq!(r.dangling[0].provenance, DanglingProvenance::EarlierRun);
    }

    #[test]
    fn an_answered_call_is_not_dangling() {
        let events = vec![
            rec(1, started("a")),
            rec(2, requested("c1")),
            rec(3, result_for("c1")),
        ];
        assert!(reduce_run(&events).dangling.is_empty());
    }

    #[test]
    fn a_log_with_no_run_marker_attributes_to_earlier_not_this_restart() {
        let events = vec![rec(1, requested("c1")), rec(2, assistant("hi"))];
        let r = reduce_run(&events);
        assert_eq!(r.run_anchor, None);
        assert_eq!(r.disposition, RunDisposition::Clean);
        assert_eq!(r.dangling[0].provenance, DanglingProvenance::EarlierRun);
    }

    /// G1 — the anti-drift device. `reduce_run` must ASK
    /// `reduce_disposition`, never re-derive. Falsify by adding any shortcut
    /// (e.g. "non-empty dangling implies Interrupted") to `reduce_run`.
    mod g1 {
        use super::*;
        use proptest::prelude::*;

        fn markers_of(events: &[SessionEventRecord]) -> Vec<SessionEventRecord> {
            events
                .iter()
                .filter(|r| {
                    matches!(
                        r.event,
                        SessionEvent::RunStarted { .. } | SessionEvent::RunFinished { .. }
                    )
                })
                .cloned()
                .collect()
        }

        /// 0 = RunStarted, 1 = RunFinished, 2 = ToolCallRequested,
        /// 3 = ToolResult, 4 = AssistantMessage.
        fn event_for(tag: u8, seq: EventSeq) -> SessionEvent {
            match tag % 5 {
                0 => started(&format!("r{seq}")),
                1 => finished(&format!("r{seq}")),
                2 => requested(&format!("c{seq}")),
                3 => result_for(&format!("c{seq}")),
                _ => assistant("x"),
            }
        }

        proptest! {
            #[test]
            fn reduce_run_asks_reduce_disposition(tags in prop::collection::vec(0u8..5, 0..40)) {
                let events: Vec<SessionEventRecord> = tags
                    .iter()
                    .enumerate()
                    .map(|(i, t)| rec(i as EventSeq + 1, event_for(*t, i as EventSeq + 1)))
                    .collect();
                prop_assert_eq!(
                    reduce_run(&events).disposition,
                    reduce_disposition(&markers_of(&events))
                );
            }
        }
    }
}
```

- [ ] **Step 4: 跑测试确认它先红**

```bash
export CARGO_TARGET_DIR=/Volumes/TBU4/Workspace/Aleph-run-reduction/target
cargo test -p alephcore --lib session::reduction 2>&1 | tail -20
```

Expected: 编译失败（`reduction` 模块尚未在 `mod.rs` 声明，或 `proptest` 未在 lib 测试可见）。若 `proptest` 在 `#[cfg(test)]` 下不可见，确认 `Cargo.toml:343` 的 `proptest = "1.4"` 在 `[dev-dependencies]` 下；是的话直接 `use proptest::prelude::*;` 即可。

- [ ] **Step 5: 让它变绿**

补齐 Step 2 的 `mod.rs` 改动，重跑：

```bash
cargo test -p alephcore --lib session::reduction 2>&1 | tail -20
```

Expected: PASS，含 `reduce_run_asks_reduce_disposition` 与六条单测。

- [ ] **Step 6: 证伪 G1、G2、G2b（各一次，看红的是不是预期那条）**

临时改 `reduce_run`，每改一次跑一次，记录红的测试名，然后**改回来**：

1. G1：把 `let disposition = reduce_disposition(&markers);` 换成
   `let disposition = if dangling.is_empty() { RunDisposition::Clean } else { RunDisposition::Interrupted { trailing_starts: 1 } };`
   （需把 pass 2 提到前面）→ 期望只有 `reduce_run_asks_reduce_disposition` 红。
2. G2：把 provenance 的 match 换成恒 `DanglingProvenance::ThisRestart`（＝今天的行为）→ 期望
   `dangling_calls_are_attributed_to_their_own_run`、
   `a_dangling_call_under_a_finished_run_is_reported_as_earlier`、
   `a_log_with_no_run_marker_attributes_to_earlier_not_this_restart` 三条红。
3. G2b：在 pass 2 里加 `if matches!(disposition, RunDisposition::Clean) { continue; }` 之外，改成「只收集 `record.seq > anchor` 的悬空」→ 期望 `a_dangling_call_under_a_finished_run_is_reported_as_earlier` 红。

把三次的红名单贴进提交信息正文。

- [ ] **Step 7: 提交**

```bash
cd /Volumes/TBU4/Workspace/Aleph-run-reduction
git add src/session/reduction.rs src/session/mod.rs
git commit -m "session: add RunReduction pure reducer (disposition, anchor, dangling provenance)"
```

---

### Task 2: 进展证据

**Files:**
- Modify: `src/session/reduction.rs`（`reduce_run` 的 pass 2 与 tests）

**Interfaces:**
- Consumes: Task 1 的 `RunReduction` / `RunProgress` / `run_anchor`
- Produces: `reduce_run` 返回的 `progress` 字段被真实填充。作用域规则：`run_anchor: Some(a)` → 只统计 `seq > a` 的记录；`run_anchor: None` → 统计全部。

- [ ] **Step 1: 写失败的测试（G4）**

在 `mod tests` 内追加：

```rust
    /// G4 — progress is scoped to the CURRENT run. A count that spans several
    /// runs names a different set.
    #[test]
    fn progress_counts_only_the_current_run() {
        let events = vec![
            rec(1, started("a")),
            rec(2, requested("old")),
            rec(3, result_for("old")),
            rec(4, assistant("run a said this")),
            rec(5, started("b")),
            rec(6, requested("c1")),
            rec(7, result_for("c1")),
            rec(8, requested("c2")),
            rec(9, assistant("run b said this")),
        ];
        let p = reduce_run(&events).progress;
        assert_eq!(p.tool_calls_dispatched, 2, "c1 and c2, not `old`");
        assert_eq!(p.tool_calls_answered, 1, "only c1 got a receipt");
        assert_eq!(p.assistant_messages, 1, "run a's message is not run b's");
        assert_eq!(p.last_activity_at, Some(90), "created_at_ms of seq 9");
    }

    #[test]
    fn answered_never_exceeds_dispatched() {
        // A stray receipt whose request lives in an earlier run must not push
        // `answered` above `dispatched`.
        let events = vec![
            rec(1, started("a")),
            rec(2, requested("old")),
            rec(3, started("b")),
            rec(4, result_for("old")),
            rec(5, requested("c1")),
        ];
        let p = reduce_run(&events).progress;
        assert_eq!(p.tool_calls_dispatched, 1);
        assert_eq!(p.tool_calls_answered, 0);
    }

    #[test]
    fn progress_covers_the_whole_log_when_there_is_no_run_marker() {
        let events = vec![
            rec(1, requested("c1")),
            rec(2, result_for("c1")),
            rec(3, assistant("hi")),
        ];
        let p = reduce_run(&events).progress;
        assert_eq!(p.tool_calls_dispatched, 1);
        assert_eq!(p.tool_calls_answered, 1);
        assert_eq!(p.assistant_messages, 1);
        assert_eq!(p.last_activity_at, Some(30));
    }
```

- [ ] **Step 2: 跑测试确认它先红**

```bash
cargo test -p alephcore --lib session::reduction 2>&1 | tail -20
```

Expected: FAIL — `progress_counts_only_the_current_run` 断言 `tool_calls_dispatched == 2` 但得到 `0`（Task 1 里 progress 恒为 default）。

- [ ] **Step 3: 在 `reduce_run` 的 pass 2 里填充 progress**

把 pass 2 的循环体改成同时算两件事：

```rust
    // Pass 2: attribute the dangling calls and count this run's progress.
    //
    // `in_scope` is the progress window: events after the anchor, or the whole
    // log when there is no anchor. That second case is not a fallback to
    // something looser — a log with no `RunStarted` holds exactly one run's
    // worth of events, so the whole log IS the scope.
    let mut dangling = Vec::new();
    let mut progress = RunProgress::default();
    let mut answered_in_scope: HashSet<&str> = HashSet::new();
    let mut dispatched_in_scope: Vec<&str> = Vec::new();
    for record in events {
        let in_scope = run_anchor.is_none_or(|anchor| record.seq > anchor);
        if in_scope {
            progress.last_activity_at = Some(record.created_at_ms);
        }
        match &record.event {
            SessionEvent::ToolCallRequested {
                turn_id,
                call_id,
                name,
                ..
            } => {
                if in_scope {
                    progress.tool_calls_dispatched += 1;
                    dispatched_in_scope.push(call_id.as_str());
                }
                if !answered.contains(call_id.as_str()) {
                    let provenance = match run_anchor {
                        Some(anchor) if record.seq > anchor => DanglingProvenance::ThisRestart,
                        _ => DanglingProvenance::EarlierRun,
                    };
                    dangling.push(DanglingCall {
                        call_id: call_id.clone(),
                        tool_name: name.clone(),
                        turn_id: *turn_id,
                        provenance,
                    });
                }
            }
            SessionEvent::ToolResult { call_id, .. } | SessionEvent::ToolError { call_id, .. } => {
                if in_scope {
                    answered_in_scope.insert(call_id.as_str());
                }
            }
            SessionEvent::AssistantMessage { .. } if in_scope => {
                progress.assistant_messages += 1;
            }
            _ => {}
        }
    }
    // Answered counts DISPATCHED calls that got a receipt, not receipt events:
    // a receipt for a call requested in an earlier run must not push this
    // number above `dispatched`.
    progress.tool_calls_answered = dispatched_in_scope
        .iter()
        .filter(|id| answered_in_scope.contains(*id))
        .count();
```

并把结构体末尾的 `progress: RunProgress::default()` 改成 `progress`。

> `is_none_or` 是 Rust 1.82+ 的 `Option` 方法；MSRV 是 1.95，可用。

- [ ] **Step 4: 跑测试确认它变绿**

```bash
cargo test -p alephcore --lib session::reduction 2>&1 | tail -20
```

Expected: PASS，全部十条 + proptest。

- [ ] **Step 5: 证伪 G4**

把 `progress.tool_calls_answered = ...` 那一段换成 `progress.tool_calls_answered = progress.tool_calls_dispatched;`，重跑 → 期望**只有** `progress_counts_only_the_current_run` 与 `answered_never_exceeds_dispatched` 两条红。改回来。

- [ ] **Step 6: 提交**

```bash
git add src/session/reduction.rs
git commit -m "session: count per-run progress evidence in RunReduction"
```

---

### Task 3: `ResumeCoordinator` 改用归约，并按归属给出两种措辞

**Files:**
- Modify: `src/gateway/resume_coordinator.rs`（删 `ScanVerdict` L157-164、`classify_markers` L166-185、`compute_boundary_repairs` L286-327；改 `boundary_repair_text` L276-284、`resume_from_markers` L528-547、`repair_boundary` L763-781；改 tests L1040-1290）
- Test: 同文件 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `crate::session::reduction::{reduce_disposition, reduce_run, DanglingProvenance, RunDisposition, RunReduction}`
- Produces: `pub(crate) fn repairs_for(reduction: &RunReduction) -> Vec<SessionEvent>` — 供本模块与其测试使用。`ScanVerdict` / `classify_markers` / `compute_boundary_repairs` **不再存在**。

- [ ] **Step 1: 写失败的测试（G3）**

把 `mod tests` 里三条旧的 `classify_*` 测试（L1040-1081）改成调用 `reduce_disposition`，并把 `repair_*` 三条（L1083-1131、L1270-1288）替换为下面这组：

```rust
    /// G3 — both arms must carry all four semantic points. Asserting on
    /// MEANING, not bytes: `!contains("failed")` gets hit by the text's own
    /// negation sentence, which is how the first version of this guard went
    /// red for the wrong reason (§4.13a).
    fn assert_four_points(error: &str, tool: &str) {
        assert!(
            error.contains("OUTCOME UNKNOWN"),
            "must state the outcome is unknown, got: {error}"
        );
        assert!(
            error.contains("NOT a report that the call failed"),
            "must explicitly deny that the call failed, got: {error}"
        );
        assert!(
            error.contains(tool),
            "must name the tool so the model knows what to verify, got: {error}"
        );
        assert!(
            error.contains("side effects"),
            "must warn that side effects may have landed, got: {error}"
        );
    }

    #[test]
    fn repairs_speak_a_different_sentence_per_provenance() {
        let events = vec![
            rec(1, run_started(10), 10),
            rec(2, tool_requested("c1"), 20),
            rec(3, run_started(30), 30),
            rec(4, tool_requested("c2"), 40),
        ];
        let repairs = repairs_for(&reduce_run(&events));
        assert_eq!(repairs.len(), 2, "BOTH provenances get a repair event");

        let mut texts = Vec::new();
        for ev in &repairs {
            let SessionEvent::ToolError { call_id, error, .. } = ev else {
                panic!("expected ToolError, got {ev:?}");
            };
            assert_four_points(error, "bash_exec");
            texts.push((call_id.clone(), error.clone()));
        }
        assert_eq!(texts[0].0, "c1");
        assert!(
            texts[0].1.contains("an earlier run in this session"),
            "the older dangle must not be blamed on this restart, got: {}",
            texts[0].1
        );
        assert_eq!(texts[1].0, "c2");
        assert!(
            texts[1].1.contains("the server restarted"),
            "this run's dangle must say so, got: {}",
            texts[1].1
        );
        assert_ne!(texts[0].1, texts[1].1, "two provenances, two sentences");
    }

    #[test]
    fn repairs_are_empty_when_every_call_is_answered() {
        let events = vec![
            rec(1, run_started(10), 10),
            rec(2, tool_requested("c1"), 20),
            rec(3, tool_result("c1"), 30),
        ];
        assert!(repairs_for(&reduce_run(&events)).is_empty());
    }

    #[test]
    fn a_tool_error_counts_as_an_answer() {
        let events = vec![
            rec(1, run_started(10), 10),
            rec(2, tool_requested("c1"), 20),
            rec(
                3,
                SessionEvent::ToolError {
                    turn_id: TurnId::new_v4(),
                    call_id: "c1".into(),
                    error: "prior failure".into(),
                    at: 30,
                },
                30,
            ),
        ];
        assert!(repairs_for(&reduce_run(&events)).is_empty());
    }
```

- [ ] **Step 2: 跑测试确认它先红**

```bash
cargo test -p alephcore --lib gateway::resume_coordinator 2>&1 | tail -20
```

Expected: 编译失败 —— `repairs_for` / `reduce_run` / `reduce_disposition` 未定义。

- [ ] **Step 3: 删两处旧推导，改写 `boundary_repair_text`，加 `repairs_for`**

删除 L157-185 的 `ScanVerdict` 与 `classify_markers` 整段。把 L276-327 的 `boundary_repair_text` + `compute_boundary_repairs` 整体替换为：

```rust
/// The sentence a dangling call is answered with.
///
/// Deliberately **not** a safety-level classifier. `ToolSafetyLevel` exists and
/// could sort read-only calls from destructive ones, but deciding "is this safe
/// to redo?" from a tool name and its arguments is exactly the reasoning R7
/// reserves for the model. State the fact; let it judge.
///
/// Two arms because there are two true sentences. Everything after the lead-in
/// is shared, so the four semantic points cannot drift apart between them.
fn boundary_repair_text(tool: &str, provenance: DanglingProvenance) -> String {
    let lead = match provenance {
        DanglingProvenance::ThisRestart => format!(
            "the server restarted after this `{tool}` call was dispatched but before its \
             result was recorded"
        ),
        DanglingProvenance::EarlierRun => format!(
            "an earlier run in this session ended without recording the result of this \
             `{tool}` call"
        ),
    };
    format!(
        "OUTCOME UNKNOWN — {lead}. This is NOT a report that the call failed: it may have \
         completed, and any side effects it has (file writes, commands, network calls, \
         external state) have already landed. Verify the current state before deciding \
         whether to repeat it."
    )
}

/// Turn a reduction's dangling set into appendable answer events.
///
/// **Both provenances get an event.** Leaving the older ones unanswered is not
/// the cheaper option: `build_prompt` drops an orphan `tool_use` whose result
/// never arrives, so the model stops seeing that the call ever happened — while
/// its side effects may still be on disk. A missing row reads as "there was no
/// value"; that is the reading this whole repair exists to prevent.
///
/// The answer is shaped as `ToolError` because there is no result to hand back:
/// a synthetic `ToolResult` would make an invented payload indistinguishable
/// from the tool's real output.
pub(crate) fn repairs_for(reduction: &RunReduction) -> Vec<SessionEvent> {
    let at = now_ms();
    reduction
        .dangling
        .iter()
        .map(|call| SessionEvent::ToolError {
            turn_id: call.turn_id,
            call_id: call.call_id.clone(),
            error: boundary_repair_text(&call.tool_name, call.provenance),
            at,
        })
        .collect()
}
```

在 imports 区加：

```rust
use crate::session::reduction::{
    reduce_disposition, reduce_run, DanglingProvenance, RunDisposition, RunReduction,
};
```

- [ ] **Step 4: 改两个调用点**

`resume_from_markers`（L528-547）：

```rust
        match reduce_disposition(markers) {
            RunDisposition::Clean => {
                report.skipped += 1;
            }
            RunDisposition::Interrupted { .. } if has_own_scheduler(session_id) => {
                tracing::info!(
                    session = ?session_id,
                    "resume: session has its own scheduler; handing recovery back to it"
                );
                self.close_delegated_marker(session_id).await;
                report.delegated += 1;
            }
            RunDisposition::Interrupted { trailing_starts } => {
                let project_root = latest_project_root(markers);
                self.handle_interrupted(session_id, markers, trailing_starts, project_root, report)
                    .await;
            }
        }
```

`repair_boundary`（L763-781）的前两行：

```rust
        let events = self.event_store.load_all_events(session_id).await?;
        let repairs = repairs_for(&reduce_run(&events));
```

- [ ] **Step 5: 跑测试确认它变绿**

```bash
cargo test -p alephcore --lib gateway::resume_coordinator 2>&1 | tail -20
cargo test -p alephcore --lib session::reduction 2>&1 | tail -5
```

Expected: 两条都 PASS。若 `tests/resume_coordinator_integration.rs` 引用了删掉的符号，一并修（它在 `tests/` 下，用 `cargo test -p alephcore --features test-helpers --test resume_coordinator_integration --no-run` 检查）。

- [ ] **Step 6: 证伪 G3**

把 `boundary_repair_text` 的 `EarlierRun` 臂改成与 `ThisRestart` 臂返回同一句（＝今天的行为），重跑 → 期望 `repairs_speak_a_different_sentence_per_provenance` 红，且红在 `assert_ne!` 或「an earlier run in this session」那一条上。再删掉共享尾巴里的 `"side effects"` 三个词，重跑 → 期望同一条测试红在 `assert_four_points` 上，**两条臂各红一次**。改回来。

- [ ] **Step 7: 提交**

```bash
git add src/gateway/resume_coordinator.rs
git commit -m "gateway: resume coordinator reads RunReduction; repair text names the right run"
```

---

### Task 4: `ProjectionReconciler` 换推导，并改写两处说谎的注释

**Files:**
- Modify: `src/gateway/projection_reconciler.rs:19`（import）、`:77-80`（match）、`:1-16`（模块 doc）
- Modify: `src/gateway/session_projector.rs:1-22`（模块 doc）

**Interfaces:**
- Consumes: `crate::session::reduction::{reduce_disposition, RunDisposition}`
- Produces: 无新符号。`ReconcileReport` 字段不变（`skipped_clean` 语义不变）。

- [ ] **Step 1: 换 import 与 match**

`src/gateway/projection_reconciler.rs:19`：

```rust
use crate::session::reduction::{reduce_disposition, RunDisposition};
```

`:77-80`：

```rust
            match reduce_disposition(&markers) {
                RunDisposition::Clean => report.skipped_clean += 1,
                RunDisposition::Interrupted { .. } => {
                    self.reconcile_session(&session_id, &mut report).await;
                }
            }
```

- [ ] **Step 2: 改写 `projection_reconciler.rs` 的模块 doc**

把 L12-15 的「Scope … file backend only」段落替换为：

```rust
//! Scope (see `docs/superpowers/specs/2026-07-04-projection-reconciler-p2-design.md`):
//! interrupted runs only, no schema change — the source seq is recovered from
//! the `"{key}:{seq}"` id embedded in each projector-written transcript row.
//!
//! **Both backends.** This used to say "file backend only". The SQLite backend
//! stores the projector's seq in its own `source_seq` column and rebuilds the
//! same id through `projection::row_id` on read
//! (`session_manager/ops/crud.rs`), so `parse_source_seq` succeeds there too
//! and the back-fill covers it. A comment that names another module's
//! behaviour freezes that module without telling it; this one had already
//! drifted.
//!
//! **What it does NOT cover**: a row the live drain dropped under back-pressure
//! in a run that later finished cleanly. `reduce_disposition` calls that
//! session `Clean`, so this pass skips it and the row is gone from the display
//! for good. The trigger condition here is "the run was interrupted"; the
//! failure condition is "the projection has a gap", and the two are not the
//! same set. Fixing it needs a durable projection watermark — see
//! `docs/superpowers/specs/2026-08-31-run-reduction-design.md` §8.1.
```

- [ ] **Step 3: 改写 `session_projector.rs` 的模块 doc**

把 L15-22 的「Consistency model … the SSOT is not.」段落替换为：

```rust
//! Consistency model: `session_events` is the single source of truth and is
//! unaffected — the agent replays the event log in full, so **recovery of the
//! agent's context is complete**. The `messages` table is an *eventually
//! consistent* read projection for the Panel.
//!
//! A boot-time reconciler exists
//! ([`crate::gateway::projection_reconciler`]) and back-fills the
//! un-materialised tail of any session whose run markers read as interrupted.
//! It does **not** catch a drop in a session whose run then finished cleanly:
//! that session classifies as `Clean` and is skipped, so the row is lost from
//! the display permanently. See
//! `docs/superpowers/specs/2026-08-31-run-reduction-design.md` §8.1 — the fix
//! is a durable projection watermark, not a wider marker scan.
```

并把 L352-354 那条 `TrySendError::Full` 的注释里「may lag until a P2 reconciler catches it up」改成：

```rust
            // Expected back-pressure. The event stays in the SSOT log (agent
            // recovery unaffected). The Panel projection may lose this row for
            // good if the run then finishes cleanly — see the module doc.
```

- [ ] **Step 4: 跑测试**

```bash
cargo test -p alephcore --lib gateway::projection_reconciler 2>&1 | tail -20
cargo test -p alephcore --lib gateway::session_projector 2>&1 | tail -10
```

Expected: 两条都 PASS，无行为变化（只换了推导来源与注释）。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/projection_reconciler.rs src/gateway/session_projector.rs
git commit -m "gateway: projection reconciler reads RunReduction; correct two stale module docs"
```

---

### Task 5: `subagent_tool::recovery` 携带进展证据

**Files:**
- Modify: `src/agents/subagent_tool/recovery.rs`（`Recovered::Interrupted` 变体、`classify`、`enumerate`、`to_json`、`to_list_row`、`resolve_forgotten`）
- Test: 同文件 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: Task 1/2 的 `reduce_run` 与 `RunProgress`；`crate::routing::session_key::SessionKey`（`Debug + Clone + PartialEq + Eq + Hash`，已确认）
- Produces:
  - `Recovered::Interrupted { child_session: SessionKey, flow: String, progress: Option<RunProgress> }`
    —— `child_session` 从 `String` 改成 `SessionKey`，因为构造点（`classify` / `enumerate`）手里本来就有这个值；把它降级成字符串再解析回来，就是给同一个事实造第二份表述。渲染点用 `.to_string()`，与今天的字节完全一致。
  - `Recovered::Completed` / `Recovered::Sidecar` 形状不变。

- [ ] **Step 1: 写失败的测试（G5）**

在 `mod tests` 追加：

```rust
    /// G5 — the directory face must not pay for progress. `list_from_log`
    /// serves dozens of rows; one extra child-log read per interrupted child
    /// turns a cheap directory into an N-read one.
    ///
    /// Asserted on the READ COUNT, not on the rows: "asked and got nothing"
    /// and "did not ask" render identically in the output.
    #[tokio::test]
    async fn the_directory_face_reads_only_the_parent_log() {
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let tool = counting_tool(counter.clone(), vec![
            rec(SessionEvent::SubagentSpawned {
                turn_id: TurnId::new_v4(),
                child_id: bg_child("agent-a", "req-1"),
                flow: "explore".into(),
                at: 1,
            }),
        ]);
        let rows = tool.list_from_log(&[], None).await;
        assert_eq!(rows.len(), 1);
        assert!(
            matches!(&rows[0].1, Recovered::Interrupted { progress: None, .. }),
            "the directory row carries no progress"
        );
        assert_eq!(
            counter.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "exactly one get_events: the parent log"
        );
    }

    /// The detail face DOES pay, because it is the answer to "tell me about
    /// this one" and already carries the child's whole text.
    #[tokio::test]
    async fn the_detail_face_loads_the_childs_progress() {
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let tool = counting_tool_with_child(counter.clone(), /* parent */ vec![
            rec(SessionEvent::SubagentSpawned {
                turn_id: TurnId::new_v4(),
                child_id: bg_child("agent-a", "req-1"),
                flow: "explore".into(),
                at: 1,
            }),
        ], /* child */ vec![
            rec(SessionEvent::RunStarted { run_id: "r1".into(), at: 1, project_root: None }),
            rec(SessionEvent::ToolCallRequested {
                turn_id: TurnId::new_v4(),
                call_id: "c1".into(),
                name: "bash_exec".into(),
                input: serde_json::json!({}),
                at: 2,
            }),
        ]);
        let out = tool.resolve_forgotten(&["req-1".to_string()], None).await;
        let Some(Recovered::Interrupted { progress: Some(p), .. }) = out.get("req-1") else {
            panic!("the detail face must carry progress, got {:?}", out.get("req-1"));
        };
        assert_eq!(p.tool_calls_dispatched, 1);
        assert_eq!(p.tool_calls_answered, 0);
    }
```

> `counting_tool` / `counting_tool_with_child` / `bg_child` / `rec` 是本测试模块的构造器。`bg_child(agent, rid)` 返回
> `SessionKey::Ephemeral { agent_id: agent.into(), ephemeral_id: format!("{SUBAGENT_BG_CHILD_PREFIX}{rid}") }`。
> 计数 `SessionService` 用一个实现 `SessionService` 的测试替身，`get_events` 里 `counter.fetch_add(1, SeqCst)` 后按 `SessionId` 分派返回父/子日志；其余方法 `unimplemented!()`。仓库里已有同形替身可参考：`src/gateway/projection_reconciler.rs::mem_event_store`。

- [ ] **Step 2: 跑测试确认它先红**

```bash
cargo test -p alephcore --lib agents::subagent_tool::recovery 2>&1 | tail -20
```

Expected: 编译失败 —— `Recovered::Interrupted` 没有 `progress` 字段。

- [ ] **Step 3: 改变体形状与两个构造点**

变体：

```rust
    /// The child was spawned and never returned — the process died while it
    /// was running. Its partial transcript lives in `child_session`.
    Interrupted {
        /// Kept as the key the emitter minted, not a string parsed back out of
        /// one: `classify` and `enumerate` both hold it already, and a second
        /// parse would be a second answer to "which session is this".
        child_session: SessionKey,
        flow: String,
        /// What the child got done before it stopped, when this face paid to
        /// find out. `None` means **this face did not ask** — never "no
        /// progress". Only the detail face (`resolve_forgotten`) fills it in;
        /// the directory (`list_from_log`) leaves it `None` on purpose.
        progress: Option<RunProgress>,
    },
```

`classify` 的 `SubagentSpawned` 臂：

```rust
                interrupted = Some(Recovered::Interrupted {
                    child_session: child_id.clone(),
                    flow: flow.clone(),
                    progress: None,
                });
```

`enumerate` 的 `SubagentSpawned` 臂同样改（`child_session: child_id.clone(), flow: flow.clone(), progress: None`）。

`to_json` / `to_list_row` 的 `Interrupted` 臂里 `"child_session": child_session` 改成 `"child_session": child_session.to_string()`。

`to_json` 的 `Interrupted` 臂增加一句只陈述、不判决的事实（R7）：

```rust
        Recovered::Interrupted {
            child_session,
            flow,
            progress,
        } => {
            let mut note = "This sub-agent was still running when the server restarted, so it \
                            never produced a result and is not running now. Whatever it had \
                            already done — including any file writes or commands — has landed. \
                            Its partial transcript is at child_session; read that before \
                            deciding whether to spawn the task again."
                .to_string();
            if let Some(p) = progress {
                use std::fmt::Write as _;
                let _ = write!(
                    note,
                    " Before it stopped it had dispatched {} tool calls, {} of which recorded a \
                     result, and produced {} assistant messages. Read the child transcript to \
                     judge what is done — this is a report of what happened, not a verdict on \
                     what is left.",
                    p.tool_calls_dispatched, p.tool_calls_answered, p.assistant_messages
                );
            }
            json!({
                "status": "interrupted",
                "request_id": request_id,
                "agent": flow,
                "child_session": child_session.to_string(),
                "progress": progress.as_ref().map(|p| json!({
                    "tool_calls_dispatched": p.tool_calls_dispatched,
                    "tool_calls_answered": p.tool_calls_answered,
                    "assistant_messages": p.assistant_messages,
                    "last_activity_ms": p.last_activity_at,
                })),
                "note": note,
            })
        }
```

- [ ] **Step 4: 在 `resolve_forgotten` 末尾补一段富化**

在 sidecar 循环之后、`out` 之前：

```rust
        // The detail face pays for progress; the directory does not (see the
        // `progress` field's doc). One extra read per interrupted child, and
        // only for the ids the caller actually named.
        for recovered in out.values_mut() {
            if let Recovered::Interrupted {
                child_session,
                progress,
                ..
            } = recovered
            {
                match self.session.get_events(child_session, None, None).await {
                    Ok(events) => {
                        *progress = Some(crate::session::reduction::reduce_run(&events).progress);
                    }
                    Err(error) => {
                        // Absent, not zero. A store that could not be read has
                        // not told us the child did nothing.
                        tracing::debug!(%error, "subagent recovery: child event log unreadable");
                    }
                }
            }
        }
```

imports 区加 `use crate::session::reduction::RunProgress;`。

- [ ] **Step 5: 跑测试确认它变绿**

```bash
cargo test -p alephcore --lib agents::subagent_tool 2>&1 | tail -20
```

Expected: PASS。若别处仍把 `child_session` 当 `String` 用，编译器会点名——按同样的 `.to_string()` 方式修。

- [ ] **Step 6: 证伪 G5**

在 `to_list_row` 的 `Interrupted` 臂里加一次 `self.session.get_events(...)`（需把它挪进 `impl`，或直接在 `list_from_log` 里对每行加一次读），重跑 → 期望 `the_directory_face_reads_only_the_parent_log` 红在 `counter == 1` 上。再把 Step 4 那段富化整段删掉 → 期望 `the_detail_face_loads_the_childs_progress` 红。两次都改回来。

- [ ] **Step 7: 提交**

```bash
git add src/agents/subagent_tool/recovery.rs
git commit -m "agents: interrupted sub-agent recovery reports what the child got done"
```

---

### Task 6: 真机装置 `qa/resume_boundary/`

**Files:**
- Create: `qa/resume_boundary/run.sh`（可执行）
- Create: `qa/resume_boundary/drive_dangle.py`
- Create: `qa/resume_boundary/assert_repairs.py`

**Interfaces:**
- Consumes: `qa/lib/scratch_home.sh::qa_redirect_home`、`qa/lib/build.sh::qa_build`、`qa/busy_input/mock_anthropic.py`（argv：`PORT PROBE PLAN TOOL_SPEC_PATH REQUEST_LOG`）、`qa/busy_input/patch_config.py`
- Produces: 两个阶段 `crash` / `attribute`。`tests/qa_fixture_hygiene.rs` 会从文件系统枚举 fixture，所以**必须**用 `qa_redirect_home`，不能手写 `export HOME=`。

- [ ] **Step 1: 写 `run.sh` 骨架**

```bash
#!/usr/bin/env bash
# Real-machine QA for the crash boundary's TEXT — the sentence a dangling tool
# call is answered with, and whether it reaches the model at all.
#
#   ./qa/resume_boundary/run.sh crash      # a dangling call gets OUTCOME UNKNOWN,
#                                          # and the model's NEXT request carries it
#   ./qa/resume_boundary/run.sh attribute  # a dangle left by an EARLIER run is not
#                                          # blamed on this restart
#   KEEP=1 ./qa/resume_boundary/run.sh crash
#
# Why a real machine. Unit tests assert on the bytes `boundary_repair_text`
# returns; they cannot show that those bytes entered a prompt. The oracle here
# is the mock provider's REQUEST LOG — what the model was actually handed —
# not the server log, which only proves the repair was synthesised.
#
# `attribute` is the falsifying arm for the defect this round fixes: run it on
# the parent commit and it must FAIL (both dangles read "the server restarted").
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
BUSY="$HERE/../busy_input"
STAGE="${1:-crash}"
QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-resume-XXXXXX")}"
KEEP="${KEEP:-0}"

GATEWAY_PORT="${GATEWAY_PORT:-18811}"
MOCK_PORT="${MOCK_PORT:-18812}"
REQUEST_LOG="$QA_ROOT/request_log.jsonl"

. "$HERE/../lib/scratch_home.sh"
. "$HERE/../lib/build.sh"
qa_redirect_home "$QA_ROOT"

export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

MOCK_PID=""
SERVER_PID=""
say() { printf '\n=== %s ===\n' "$*"; }

start_server() {
  "$BIN" start --port "$GATEWAY_PORT" >>"$QA_ROOT/server.log" 2>&1 &
  SERVER_PID=$!
  for _ in $(seq 1 80); do
    curl -sf "http://127.0.0.1:$GATEWAY_PORT/healthz" >/dev/null && return 0
    sleep 0.25
  done
  echo "server did not come up" >&2; return 1
}

# kill -9, not SIGTERM: a clean shutdown closes the dangling call and there is
# nothing left to repair — the fixture would then be measuring nothing.
hard_kill_server() {
  kill -9 "$SERVER_PID" 2>/dev/null
  wait "$SERVER_PID" 2>/dev/null
  SERVER_PID=""
}

cleanup() {
  [ -n "$SERVER_PID" ] && kill -9 "$SERVER_PID" 2>/dev/null
  [ -n "$MOCK_PID" ] && kill -9 "$MOCK_PID" 2>/dev/null
  if [ "$KEEP" = "1" ]; then echo "artifacts kept in $QA_ROOT"; else rm -rf "$QA_ROOT"; fi
}
trap cleanup EXIT

say "build"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  qa_build --bin aleph-server || { echo "build failed" >&2; exit 1; }
fi
TARGET_DIR="$(cd "$REPO" && HOME="$REAL_HOME" cargo metadata --format-version 1 --no-deps 2>/dev/null \
  | python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])')"
BIN="$TARGET_DIR/debug/aleph-server"
[ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 1; }
```

- [ ] **Step 2: 写会真的悬空的工具规格**

在 `run.sh` 的 build 之后：

```bash
# The tool must OUTLIVE the kill, or nothing dangles. `sleep 120` is dispatched,
# the server is killed at ~5s, and the call never gets a receipt.
cat >"$QA_ROOT/tool_spec.json" <<'JSON'
{"name": "bash_exec", "input": {"command": "sleep 120"}}
JSON

python3 "$BUSY/mock_anthropic.py" "$MOCK_PORT" /etc/hostname long-run \
  "$QA_ROOT/tool_spec.json" "$REQUEST_LOG" >"$QA_ROOT/mock.log" 2>&1 &
MOCK_PID=$!

python3 "$BUSY/patch_config.py" "$ALEPH_HOME/config.toml" "$MOCK_PORT"
python3 "$HERE/drive_dangle.py" --mode config-resume --enabled false \
  --config "$ALEPH_HOME/config.toml"
```

> `patch_config.py` 的参数以该脚本自身的 `--help` / 头部注释为准；若签名不同，照它的实际签名调用，**不要**在本 fixture 里另写一份配置改写。

- [ ] **Step 3: 写 `drive_dangle.py`**

```python
#!/usr/bin/env python3
"""Drive one turn that leaves a tool call dangling, and PROVE it dangled.

An instrument that cannot show it produced the state it claims to test is not
an instrument. `--assert-dangling` reads the session event log and fails loudly
when there is a `tool_result`/`tool_error` for the call — that means the kill
landed too late and any green from the stage that follows is meaningless.
"""
import argparse, json, pathlib, sys, time, urllib.request


def rpc(port, method, params):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    req = urllib.request.Request(
        f"http://127.0.0.1:{port}/rpc", data=body, headers={"content-type": "application/json"}
    )
    with urllib.request.urlopen(req, timeout=30) as fh:
        return json.load(fh)


def events(home, session_key):
    """Read the durable event log straight off disk (file backend)."""
    root = pathlib.Path(home) / "data" / "sessions"
    hits = sorted(root.rglob("events.jsonl"))
    out = []
    for path in hits:
        for line in path.read_text().splitlines():
            if line.strip():
                out.append(json.loads(line))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mode", required=True)
    ap.add_argument("--port", type=int, default=18811)
    ap.add_argument("--home")
    ap.add_argument("--config")
    ap.add_argument("--enabled")
    ap.add_argument("--session", default="qa-resume")
    args = ap.parse_args()

    if args.mode == "config-resume":
        # Flip `[resume] enabled` in place. Written here rather than in bash so
        # the two stages cannot disagree about the key's name.
        path = pathlib.Path(args.config)
        text = path.read_text()
        if "[resume]" in text:
            import re
            text = re.sub(r"(?m)^enabled\s*=.*$", f"enabled = {args.enabled}", text, count=1)
        else:
            text += f"\n[resume]\nenabled = {args.enabled}\n"
        path.write_text(text)
        return 0

    if args.mode == "send":
        rpc(args.port, "chat.send", {"session_key": args.session, "text": "run the probe"})
        # Give the harness time to dispatch the tool call.
        time.sleep(5)
        return 0

    if args.mode == "assert-dangling":
        evs = events(args.home, args.session)
        requested = {e["call_id"] for e in evs if e.get("type") == "tool_call_requested"}
        answered = {
            e["call_id"] for e in evs if e.get("type") in ("tool_result", "tool_error")
        }
        dangling = requested - answered
        if not dangling:
            print(
                f"FAIL instrument: no dangling call (requested={len(requested)}, "
                f"answered={len(answered)}). The kill landed too late.",
                file=sys.stderr,
            )
            return 1
        print(f"ok: {len(dangling)} dangling call(s): {sorted(dangling)}")
        return 0

    print(f"unknown mode {args.mode}", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
```

> RPC 方法名与 session 参数以 `qa/busy_input/drive_burst_drain.py` 的实际用法为准——照抄那里的方法名，**不要**猜。

- [ ] **Step 4: 写 `assert_repairs.py`**

```python
#!/usr/bin/env python3
"""Assert on WHAT THE MODEL WAS HANDED, not on what the server logged."""
import argparse, json, pathlib, sys

FOUR_POINTS = [
    "OUTCOME UNKNOWN",
    "NOT a report that the call failed",
    "side effects",
]
THIS_RESTART = "the server restarted"
EARLIER_RUN = "an earlier run in this session"


def request_bodies(path):
    return [json.loads(line)["body"] for line in pathlib.Path(path).read_text().splitlines() if line.strip()]


def repair_texts(bodies):
    """Every tool_result payload in every request, flattened to text."""
    out = []
    for body in bodies:
        for msg in body.get("messages", []):
            content = msg.get("content")
            if isinstance(content, list):
                for block in content:
                    text = json.dumps(block)
                    if "OUTCOME UNKNOWN" in text:
                        out.append(text)
            elif isinstance(content, str) and "OUTCOME UNKNOWN" in content:
                out.append(content)
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--request-log", required=True)
    ap.add_argument("--stage", required=True, choices=["crash", "attribute"])
    args = ap.parse_args()

    texts = repair_texts(request_bodies(args.request_log))
    if not texts:
        print("FAIL: no OUTCOME UNKNOWN reached the model", file=sys.stderr)
        return 1

    failures = []
    for text in texts:
        for point in FOUR_POINTS:
            if point not in text:
                failures.append(f"missing {point!r} in: {text[:200]}")

    if args.stage == "attribute":
        this = [t for t in texts if THIS_RESTART in t]
        earlier = [t for t in texts if EARLIER_RUN in t]
        if not earlier:
            failures.append(
                "FAIL: the dangle left by the EARLIER run was blamed on this restart "
                "(no 'an earlier run in this session' text reached the model). "
                "This is the pre-fix behaviour."
            )
        if not this:
            failures.append("FAIL: this run's own dangle did not say 'the server restarted'")

    for f in failures:
        print(f, file=sys.stderr)
    if failures:
        return 1
    print(f"PASS ({len(texts)} repair text(s) reached the model)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
```

- [ ] **Step 5: 把两个阶段接起来**

在 `run.sh` 末尾：

```bash
case "$STAGE" in
  crash)
    say "crash: dangle -> kill -9 -> restart with resume on"
    python3 "$HERE/drive_dangle.py" --mode config-resume --enabled true --config "$ALEPH_HOME/config.toml"
    start_server || exit 1
    python3 "$HERE/drive_dangle.py" --mode send --port "$GATEWAY_PORT"
    hard_kill_server
    python3 "$HERE/drive_dangle.py" --mode assert-dangling --home "$ALEPH_HOME" || exit 1
    start_server || exit 1
    sleep 15   # boot scan is detached; give it time to repair and re-trigger
    python3 "$HERE/assert_repairs.py" --request-log "$REQUEST_LOG" --stage crash
    ;;
  attribute)
    say "attribute: dangle with resume OFF, then a second dangle with resume ON"
    python3 "$HERE/drive_dangle.py" --mode config-resume --enabled false --config "$ALEPH_HOME/config.toml"
    start_server || exit 1
    python3 "$HERE/drive_dangle.py" --mode send --port "$GATEWAY_PORT"
    hard_kill_server
    python3 "$HERE/drive_dangle.py" --mode assert-dangling --home "$ALEPH_HOME" || exit 1

    # Restart with resume still OFF: nothing is repaired, the dangle survives.
    start_server || exit 1
    sleep 5
    python3 "$HERE/drive_dangle.py" --mode send --port "$GATEWAY_PORT"
    hard_kill_server

    # Now turn resume ON. The boot scan sees TWO dangles from TWO runs.
    python3 "$HERE/drive_dangle.py" --mode config-resume --enabled true --config "$ALEPH_HOME/config.toml"
    start_server || exit 1
    sleep 15
    python3 "$HERE/assert_repairs.py" --request-log "$REQUEST_LOG" --stage attribute
    ;;
  *) echo "unknown stage $STAGE (crash|attribute)" >&2; exit 2 ;;
esac
```

`chmod +x qa/resume_boundary/run.sh`。

- [ ] **Step 6: 先在父提交上跑 `attribute`，它必须 FAIL**

```bash
cd /Volumes/TBU4/Workspace/Aleph-run-reduction
# The true pre-round state, resolved from the branch point rather than counted
# back N commits: a task that needed an extra fixup commit would silently make
# `HEAD~N` point at the wrong tree, and the arm would then measure the fix.
BASE="$(git merge-base HEAD main)"
git worktree add --detach /tmp/aleph-prefix "$BASE"
cp -r qa/resume_boundary /tmp/aleph-prefix/qa/
cd /tmp/aleph-prefix \
  && CARGO_TARGET_DIR=/tmp/aleph-prefix/target ./qa/resume_boundary/run.sh attribute
```

Expected: **FAIL**，报「the dangle left by the EARLIER run was blamed on this restart」。
这一步把 §1.4 那个缺陷从推论变成观察。**如果它 PASS，说明 spec §1.4 推错了 —— 停下来汇报，不要继续**，并把 spec 的 §1.4 与 G2 一起撤掉。

⚠️ **不要在本会话里 `git worktree remove`**（CLAUDE.md：同会话删 worktree 会损坏 Shell）。跑完把 `/tmp/aleph-prefix` 留在原地，在最后一个任务之后由人工 `git worktree prune` 清理。

- [ ] **Step 7: 在本分支上跑两个阶段，都必须 PASS**

```bash
cd /Volumes/TBU4/Workspace/Aleph-run-reduction
export CARGO_TARGET_DIR=/Volumes/TBU4/Workspace/Aleph-run-reduction/target
./qa/resume_boundary/run.sh crash
./qa/resume_boundary/run.sh attribute
```

Expected: 两个都 PASS。把两次输出贴进提交信息。

- [ ] **Step 8: 让 fixture hygiene 守卫也绿**

```bash
cargo test -p alephcore --features test-helpers --test qa_fixture_hygiene 2>&1 | tail -10
```

Expected: PASS（该守卫从文件系统枚举 fixture，新目录会被它看见）。

- [ ] **Step 9: 提交**

```bash
git add qa/resume_boundary
git commit -m "qa: resume_boundary fixture asserts the repair text reaches the model"
```

---

### Task 7: 全量验证与守卫红名单

**Files:** 无代码改动（除非验证暴露问题）

- [ ] **Step 1: 跑最小可信验证集的四条**

```bash
cd /Volumes/TBU4/Workspace/Aleph-run-reduction
export CARGO_TARGET_DIR=/Volumes/TBU4/Workspace/Aleph-run-reduction/target
cargo test -p alephcore --lib 2>&1 | tail -20
cargo test -p alephcore --bins 2>&1 | tail -20
cargo test -p alephcore --features test-helpers --test '*' --no-run 2>&1 | tail -20
just _stage-shell-placeholders && cargo clippy --workspace --all-targets 2>&1 | tail -30
```

Expected: 四条全绿。`--bins` 那条会覆盖 `src/bin/aleph-server/commands/start/mod.rs` 的 boot census；`ProjectionReconciler` 的构造签名未变，所以它应当不受影响——若红了，说明 census 钉住了某个被删的符号，按它的报错修。

- [ ] **Step 2: 汇总六条守卫的证伪记录**

把 Task 1 Step 6、Task 2 Step 5、Task 3 Step 6、Task 5 Step 6、Task 6 Step 6 记录的红名单汇总成一张表，写进 spec 的实施记录节（新增 §11）：

| 守卫 | 变异 | 预期红 | 实测红 |
|---|---|---|---|

若任何一条的「实测红」与「预期红」不符——尤其是**没红**——先怀疑守卫，不要怀疑变异（判据 #18）。

- [ ] **Step 3: 数一遍删掉的三个符号有没有第四个消费者**

```bash
grep -rn "classify_markers\|compute_boundary_repairs" src/ tests/ interfaces/ shared/ 2>/dev/null
grep -rn "ScanVerdict" src/ tests/ 2>/dev/null | grep -v content_scanner
```

Expected: 两条都零命中（`content_scanner.rs` 的同名不同物除外）。有命中说明之前数少了（判据 #6：数错的方向永远是少一个）。

- [ ] **Step 4: 提交**

```bash
git add docs/superpowers/specs/2026-08-31-run-reduction-design.md
git commit -m "docs: record guard falsification results for run reduction"
```

---

### Task 8: 更新 FEATURE_LOCATOR

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md`（§4.13a、附录 E.0）

- [ ] **Step 1: 在 §4.13a 增补代码锚点与本轮结论**

在 §4.13a 的「代码锚点」行加入 `src/session/reduction.rs`（`reduce_disposition` 单一推导 · `reduce_run` · `DanglingProvenance` 两句措辞），并在状态行后追加一段：

```markdown
- **③ 崩溃边界的两处推导合并（2026-08-31，run-reduction 轮）**——`classify_markers`（只数尾部 `RunStarted`）与 `compute_boundary_repairs`（扫全日志）住在同一个文件里却互不知情，`ProjectionReconciler` 只复用了前者。现两者并入 `src/session/reduction.rs` 的纯函数 `reduce_disposition` / `reduce_run`，`reduce_run` **调用** `reduce_disposition` 而不重数，由 proptest 钉住（`∀log. reduce_run(log).disposition == reduce_disposition(markers_of(log))`）。`ScanVerdict` / `classify_markers` / `compute_boundary_repairs` 已删。
- **④ 悬空调用第一次带归属**——`compute_boundary_repairs` 扫全日志却正确，理由是**所有非崩溃的终止路径都会自己关闭未执行的调用**（`think::close_unexecuted_tool_uses` / `act::emit_deferred_tool_results` / 审批路径的 `ToolCallDenied` + `ToolError`）——这条依赖住在另外两个文件里，本侧没有断言钉住它。于是一次**更早**的、当时没被修复过的悬空（崩溃时 `[resume] enabled = false`，或超出 recency 后手动 `agent.resume`）会被说成「the server restarted」，在时间上是假的。现 `DanglingProvenance::{ThisRestart, EarlierRun}` 出两句，共享同一条尾巴，四个语义要点在两条臂上各查一遍。真机装置 `qa/resume_boundary/run.sh attribute` 在父提交上实测 FAIL、修后 PASS。
- **⑤ 「做了一半」可读**——`Recovered::Interrupted` 携带 `Option<RunProgress>`（派发/回执/assistant 条数 + 最后活动时刻），**只在详情面** `resolve_forgotten` 加载子会话日志；目录面 `list_from_log` 恒 `None` 并由计数 mock 钉住 `get_events` 只调一次。`None` 意为「这一面没查」，不是「没有进展」。
```

- [ ] **Step 2: 在附录 E.0 增补一条触发器**

```markdown
- **一个动词有几张脸就有几份推导** — 「interrupted」在 `session_events`（`RunDisposition`）、sidecar（`RunPhase`）、coord store（`TaskRunStatus`）各有一套词表与一套裁决。改其中一处前先问另外两处怎么回答同一问题。前两者已由 `src/session/reduction.rs` 统一到一份推导；coord store 仍独立（`docs/superpowers/specs/2026-08-31-run-reduction-design.md` §8.5）。
- **一个扫描的作用域是全日志还是本次 run** — `compute_boundary_repairs` 曾扫全日志并给每条悬空同一句「本次重启」。它当时正确，靠的是别的文件里的三条关闭路径；作用域一旦不是显式派生的，措辞就会在某个可达路径上变成假话。判据 #12 + #17。
```

- [ ] **Step 3: 提交**

```bash
git add docs/reference/FEATURE_LOCATOR.md
git commit -m "docs: record run-reduction round in FEATURE_LOCATOR 4.13a and appendix E.0"
```

---

## Self-Review

**Spec 覆盖检查：**

| Spec 节 | 对应任务 |
|---|---|
| §4 数据形状（含 §4.1 两变体、§4.2 seq、§4.3 单一推导） | Task 1 |
| §4「`RunProgress` 的作用域」 | Task 2 |
| §5.1 ResumeCoordinator + 两句措辞 + `repairs_for` 两种归属都出 | Task 3 |
| §5.2 ProjectionReconciler + 两处注释 | Task 4 |
| §5.3 subagent recovery 双面预算 | Task 5 |
| §6.1 真机装置两阶段 + 修复前 FAIL | Task 6 |
| §6 G1–G5 + §6.2 验证集 | Task 1/2/3/5 各自的证伪步 + Task 7 |
| §7 熵减清单 | Task 3（三个符号）、Task 4（两处注释） |
| §8 刻意不做 | Task 4 Step 2/3 把 D1 写进代码注释；Task 8 写进 FEATURE_LOCATOR |
| §9 环境假设 | Global Constraints |
| §10 实施顺序 | Task 1→8 |

**类型一致性检查：**
- `RunDisposition` / `DanglingProvenance` / `DanglingCall` / `RunProgress` / `RunReduction`：Task 1 定义，Task 2/3/5 使用，字段名一致（`tool_calls_dispatched` / `tool_calls_answered` / `assistant_messages` / `last_activity_at`）。
- `reduce_disposition(&[SessionEventRecord]) -> RunDisposition`：Task 1 定义，Task 3（`resume_from_markers`）、Task 4（`reconcile_interrupted`）调用。
- `reduce_run(&[SessionEventRecord]) -> RunReduction`：Task 1/2 定义，Task 3（`repair_boundary`）、Task 5（`resolve_forgotten`）调用。
- `repairs_for(&RunReduction) -> Vec<SessionEvent>`：Task 3 定义并自用。
- `Recovered::Interrupted { child_session: SessionKey, flow: String, progress: Option<RunProgress> }`：Task 5 唯一定义点，`classify` / `enumerate` / `to_json` / `to_list_row` / `resolve_forgotten` 五处使用。

**已知需要实施者现场确认的两处**（不是占位符，是「以现场为准」）：
1. Task 6 Step 2 的 `patch_config.py` 参数签名 —— 以该脚本自身的头部注释为准，不要另写一份配置改写。
2. Task 6 Step 3 的 RPC 方法名与 session 参数 —— 照抄 `qa/busy_input/drive_burst_drain.py` 的实际用法。
