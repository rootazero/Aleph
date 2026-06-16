# Cooperative Steer Checkpoint Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When a new non-synthetic user message lands in the session mid-turn (the user changes their mind while the agent is running a tool batch), stop dispatching the batch's not-yet-started tools and return control to Think so the LLM sees the message and decides to pivot or resume.

**Architecture:** Pure wiring in the harness Act phase. The mid-turn-arrival predicate `AgentHarness::has_unanswered_user_message` (agent.rs:392) already exists and is reused **in place** (no extraction — `agent/act.rs` is a child module of `agent.rs`, so it can call the parent's private method directly). The Act batch loop calls this predicate before dispatching each serial tool call, and before each parallel group; on a hit it emits a synthetic "deferred" `ToolResult` for every skipped tool (preserving the `tool_use`↔`tool_result` API pairing) and breaks. The LLM, next Think, sees the new message + deferred results and decides (R7). No new files, no new harness config field — gated implicitly because when gateway-side `mid_turn_steering` is off, no non-synthetic message is ever injected mid-turn, so the predicate never fires.

**Tech Stack:** Rust, Tokio, `async_trait`. Files: `src/harness/agent/act.rs` (+ unit tests in `src/harness/tests/act.rs`).

**Deviations from the design spec (intentional, documented):**
- Spec §4.2 proposed *extracting* the predicate into a shared free function. Not needed: child-module visibility lets `act.rs` call the existing method as-is, so there is **no** divergent copy to guard against. Leaner, zero churn.
- Spec §4.4 proposed checkpointing the parallel path *between buffered waves*. Instead we checkpoint at **group boundaries** in `act()` (groups already run sequentially at act.rs:193). A single in-flight parallel group (typically read-only fan-out) runs to completion — low value to interrupt, and it avoids restructuring the `.buffered()` stream. The realistic "悔改" case (expensive serialized side-effecting calls like `bash`/`deploy`) lands in *later* groups, which the group-boundary check stops before.

**Project constraint (from task protocol):** Do NOT run `cargo check`/`cargo test` after implementation — commit directly. Test code below is written and committed as a contract artifact; the "run the test" steps are therefore marked **(do not execute — project constraint)**. Preserve TDD *structure* (test written with the change) without local execution.

---

## File Structure

| File | Responsibility | Change |
|------|----------------|--------|
| `src/harness/agent/act.rs` | Act phase tool-batch dispatch | Add `emit_deferred_tool_results` helper; add serial-loop checkpoint in `dispatch_group`; add group-boundary checkpoint in `act` |
| `src/harness/tests/act.rs` | Act-phase unit tests | Add 3 tests: defer-all, no-steer regression, helper shape |

No other files change. `has_unanswered_user_message` (agent.rs:392) and `last_prompt_log_len` (agent.rs:140, set in think.rs:392) are reused unchanged.

---

## Task 1: Add the `emit_deferred_tool_results` helper

**Files:**
- Modify: `src/harness/agent/act.rs` (add method inside `impl AgentHarness`, near the other `emit_*` helpers — e.g. just after `act`/before `dispatch_group`)
- Test: `src/harness/tests/act.rs`

- [ ] **Step 1: Write the failing test**

Add to `src/harness/tests/act.rs` (inside the existing `#[cfg(test)] mod` / test area; reuse the file's `MockSession`, `ScriptedTools`, and the existing harness-builder used by other tests in this file):

```rust
#[tokio::test]
async fn deferred_results_emit_one_tool_result_per_skipped_call() {
    // Two tool calls the checkpoint will skip.
    let calls = vec![
        NativeToolCall {
            id: "call_a".into(),
            name: "read".into(),
            arguments: serde_json::json!({"path": "a"}),
        },
        NativeToolCall {
            id: "call_b".into(),
            name: "read".into(),
            arguments: serde_json::json!({"path": "b"}),
        },
    ];

    let session = MockSession::new(vec![]);
    let harness = build_test_harness(session.clone()); // existing helper in this file
    let sid = SessionId::from("s1");
    let turn = crate::session::events::TurnId::new_v4(); // match how other tests mint TurnId

    harness
        .emit_deferred_tool_results(&sid, turn, &calls)
        .await
        .expect("emit deferred");

    let events = session.snapshot().await;
    let results: Vec<_> = events
        .iter()
        .filter_map(|r| match &r.event {
            SessionEvent::ToolResult { call_id, output, .. } => Some((call_id.clone(), output.clone())),
            _ => None,
        })
        .collect();

    assert_eq!(results.len(), 2, "one ToolResult per skipped call");
    assert_eq!(results[0].0, "call_a");
    assert_eq!(results[1].0, "call_b");
    assert_eq!(results[0].1.value["deferred"], serde_json::json!(true));
}
```

> Note: if `build_test_harness` / the exact `TurnId` constructor differ in this file, match the existing tests (`test_*` already construct a harness + `TurnId`). Do NOT invent a new builder.

- [ ] **Step 2: Run test to verify it fails** — **(do not execute — project constraint)**. Expected if run: compile error "no method `emit_deferred_tool_results`".

- [ ] **Step 3: Write minimal implementation**

In `src/harness/agent/act.rs`, inside `impl AgentHarness`, add:

```rust
/// Emit a synthetic "deferred" `ToolResult` for each tool call the
/// cooperative steer checkpoint skipped. Every `tool_use` block in the
/// turn's `AssistantMessage` must have a matching `tool_result` or the
/// provider rejects the next request, so a skipped call still gets a
/// result — a marker the model can re-issue from on its next turn.
///
/// R10-safe: pure mechanical bookkeeping. Whether a deferred call is
/// re-run is the model's decision next Think, not the harness's.
async fn emit_deferred_tool_results(
    &self,
    session_id: &SessionId,
    turn_id: TurnId,
    calls: &[NativeToolCall],
) -> Result<(), HarnessError> {
    for call in calls {
        let output = ToolOutput {
            value: serde_json::json!({
                "deferred": true,
                "reason": "superseded by a new user message that arrived mid-turn; \
                           re-issue this call if it is still needed",
            }),
            metadata: crate::session::events::ToolOutputMetadata::default(),
        };
        let event = SessionEvent::ToolResult {
            turn_id,
            call_id: call.id.clone(),
            output,
            at: now_ms(),
        };
        self.deps.session.emit_event(session_id, event).await?;
    }
    Ok(())
}
```

> `NativeToolCall`, `TurnId`, `ToolOutput`, `SessionEvent`, `now_ms`, `HarnessError` are already in scope in this file (used by `act`/`dispatch_group`). `ToolOutputMetadata` is referenced by full path to avoid touching the import block.

- [ ] **Step 4: Run test to verify it passes** — **(do not execute — project constraint)**.

- [ ] **Step 5: Commit**

```bash
git add src/harness/agent/act.rs src/harness/tests/act.rs
git commit -m "harness: add deferred-tool-result helper for steer checkpoint"
```

---

## Task 2: Serial-loop checkpoint in `dispatch_group`

**Files:**
- Modify: `src/harness/agent/act.rs:263` — change the `for mut call in tool_calls` loop in `dispatch_group` into a `while let` over an explicit iterator and add the checkpoint at the top.
- Test: `src/harness/tests/act.rs`

- [ ] **Step 1: Write the failing tests**

Add to `src/harness/tests/act.rs`:

```rust
// Seeds a session that already has an AssistantMessage (so the predicate's
// "at least one assistant turn" guard passes) plus a non-synthetic UserMessage
// sitting *beyond* the watermark — i.e. a steer that arrived mid-turn.
fn seed_with_midturn_steer() -> Arc<MockSession> {
    MockSession::new(vec![
        SessionEvent::UserMessage {
            turn_id: crate::session::events::TurnId::new_v4(),
            content: MessageContent::text("original request"),
            at: now_ms(),
            synthetic: false,
        },
        SessionEvent::AssistantMessage {
            turn_id: crate::session::events::TurnId::new_v4(),
            content: MessageContent::text("working on it"),
            at: now_ms(),
            // match the AssistantMessage field set used elsewhere in this file
            ..Default::default()
        },
        // The steer:
        SessionEvent::UserMessage {
            turn_id: crate::session::events::TurnId::new_v4(),
            content: MessageContent::text("actually, stop and do X instead"),
            at: now_ms(),
            synthetic: false,
        },
    ])
}

#[tokio::test]
async fn serial_batch_defers_all_when_steer_present_before_first_call() {
    let session = seed_with_midturn_steer();
    let harness = build_test_harness_serial(session.clone()); // parallel_tool_concurrency: None
    let sid = SessionId::from("s1");
    let turn = crate::session::events::TurnId::new_v4();

    // Watermark = number of events BEFORE the steer (original + assistant = 2).
    // The steer sits at index 2 >= watermark, so the predicate fires.
    harness.last_prompt_log_len.store(2, std::sync::atomic::Ordering::Relaxed);

    let calls = vec![
        NativeToolCall { id: "c1".into(), name: "read".into(), arguments: serde_json::json!({}) },
        NativeToolCall { id: "c2".into(), name: "read".into(), arguments: serde_json::json!({}) },
        NativeToolCall { id: "c3".into(), name: "read".into(), arguments: serde_json::json!({}) },
    ];

    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    let executed = harness
        .act(&sid, turn, calls, &mut cb, 0, &cancel)
        .await
        .expect("act");

    assert_eq!(executed, 0, "no tools run when steer precedes the batch");

    let events = session.snapshot().await;
    let deferred = events.iter().filter(|r| matches!(&r.event,
        SessionEvent::ToolResult { output, .. } if output.value["deferred"] == serde_json::json!(true)
    )).count();
    assert_eq!(deferred, 3, "all three calls deferred with marker results");
}

#[tokio::test]
async fn serial_batch_runs_full_when_no_midturn_steer() {
    // Only original user + assistant; nothing beyond the watermark.
    let session = MockSession::new(vec![
        SessionEvent::UserMessage {
            turn_id: crate::session::events::TurnId::new_v4(),
            content: MessageContent::text("original request"),
            at: now_ms(),
            synthetic: false,
        },
        SessionEvent::AssistantMessage {
            turn_id: crate::session::events::TurnId::new_v4(),
            content: MessageContent::text("working on it"),
            at: now_ms(),
            ..Default::default()
        },
    ]);
    let harness = build_test_harness_serial_with_outcomes(
        session.clone(),
        vec![
            Ok(ToolOutput { value: serde_json::json!("ok1"), metadata: Default::default() }),
            Ok(ToolOutput { value: serde_json::json!("ok2"), metadata: Default::default() }),
        ],
    );
    let sid = SessionId::from("s1");
    let turn = crate::session::events::TurnId::new_v4();
    harness.last_prompt_log_len.store(2, std::sync::atomic::Ordering::Relaxed);

    let calls = vec![
        NativeToolCall { id: "c1".into(), name: "read".into(), arguments: serde_json::json!({"p":"a"}) },
        NativeToolCall { id: "c2".into(), name: "read".into(), arguments: serde_json::json!({"p":"b"}) },
    ];
    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    let executed = harness.act(&sid, turn, calls, &mut cb, 0, &cancel).await.expect("act");

    assert_eq!(executed, 2, "both tools run when no mid-turn steer");
    let events = session.snapshot().await;
    let deferred = events.iter().filter(|r| matches!(&r.event,
        SessionEvent::ToolResult { output, .. } if output.value["deferred"] == serde_json::json!(true)
    )).count();
    assert_eq!(deferred, 0, "no deferral on the happy path");
}
```

> The two `build_test_harness_serial*` helpers should reuse the existing harness-construction pattern in this file with `parallel_tool_concurrency: None`. If the file already has a single builder taking outcomes, use it and drop the second helper. Match `AssistantMessage`'s actual fields (the `..Default::default()` is a placeholder for "set the same fields existing tests set"); if `SessionEvent` variants are not `Default`, construct the assistant event exactly as other tests in this file do.

- [ ] **Step 2: Run tests to verify they fail** — **(do not execute — project constraint)**. Expected if run: `serial_batch_defers_all_*` fails (currently all 3 execute, 0 deferred).

- [ ] **Step 3: Write minimal implementation**

In `src/harness/agent/act.rs`, in `dispatch_group`, replace the loop header at line 263:

```rust
        for mut call in tool_calls {
```

with an explicit iterator + checkpoint:

```rust
        let mut tool_iter = tool_calls.into_iter();
        while let Some(mut call) = tool_iter.next() {
            // Cooperative steer checkpoint. If a non-synthetic user message
            // arrived after this turn's prompt boundary (the user changed
            // their mind mid-batch), stop launching further tools and defer
            // the current call + everything still pending so the model sees
            // the message next Think and decides to pivot or resume.
            // R7/R10: mechanical — no intent judgement here.
            if self.has_unanswered_user_message(session_id).await {
                let mut deferred = Vec::with_capacity(1);
                deferred.push(call);
                deferred.extend(tool_iter.by_ref());
                self.emit_deferred_tool_results(session_id, turn_id, &deferred)
                    .await?;
                if let Some(ref tracker) = self.stall_tracker {
                    // Responding to the user IS progress; keep the stall
                    // detector from misreading a deferral as a hung turn.
                    tracker.record_activity().await;
                }
                break;
            }
```

The existing loop body (lines 264 onward: name repair, `callback.on_tool_call`, guardrails, dedup, execution, result emission) is unchanged and still ends with the loop's closing brace. The `continue` statements inside the body work identically under `while let`. Confirm nothing after the loop references `tool_calls` (the original `for … in tool_calls` already moved it, so this is safe).

- [ ] **Step 4: Run tests to verify they pass** — **(do not execute — project constraint)**.

- [ ] **Step 5: Commit**

```bash
git add src/harness/agent/act.rs src/harness/tests/act.rs
git commit -m "harness: cooperative steer checkpoint in serial tool loop"
```

---

## Task 3: Group-boundary checkpoint in `act`

**Files:**
- Modify: `src/harness/agent/act.rs:193-207` — the multi-group dispatch loop in `act`.
- Test: `src/harness/tests/act.rs`

- [ ] **Step 1: Write the failing test**

Add to `src/harness/tests/act.rs` (only if the file already supports a parallel-enabled builder + multi-group partition; if multi-group setup is heavy, keep this single assertion focused):

```rust
#[tokio::test]
async fn group_boundary_defers_remaining_groups_when_steer_present() {
    let session = seed_with_midturn_steer();
    // parallel_tool_concurrency: Some(2) so act() takes the multi-group path.
    let harness = build_test_harness_parallel(session.clone());
    let sid = SessionId::from("s1");
    let turn = crate::session::events::TurnId::new_v4();
    harness.last_prompt_log_len.store(2, std::sync::atomic::Ordering::Relaxed);

    // Two reads (parallelizable) + one bash (serial boundary) → >= 2 groups.
    let calls = vec![
        NativeToolCall { id: "r1".into(), name: "read".into(), arguments: serde_json::json!({"p":"a"}) },
        NativeToolCall { id: "r2".into(), name: "read".into(), arguments: serde_json::json!({"p":"b"}) },
        NativeToolCall { id: "b1".into(), name: "bash".into(), arguments: serde_json::json!({"cmd":"x"}) },
    ];
    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    let executed = harness.act(&sid, turn, calls, &mut cb, 0, &cancel).await.expect("act");

    // Steer present before the batch → first group-boundary check fires,
    // every group deferred, nothing executed.
    assert_eq!(executed, 0);
    let events = session.snapshot().await;
    let deferred = events.iter().filter(|r| matches!(&r.event,
        SessionEvent::ToolResult { output, .. } if output.value["deferred"] == serde_json::json!(true)
    )).count();
    assert_eq!(deferred, 3);
}
```

> If the file's existing mocks make a clean read/bash partition awkward, it is acceptable to assert the simpler invariant (steer-before-batch ⇒ 0 executed, all deferred) which both the serial and group paths satisfy, and note in a code comment that the group-boundary branch shares the predicate+helper with the serial path. Do not fabricate partition behavior the mocks don't support.

- [ ] **Step 2: Run test to verify it fails** — **(do not execute — project constraint)**.

- [ ] **Step 3: Write minimal implementation**

In `src/harness/agent/act.rs`, replace the multi-group loop (lines 193-207):

```rust
        for (start, end) in groups {
            let group: Vec<NativeToolCall> = remaining.by_ref().take(end - start).collect();
            executed_count = executed_count.saturating_add(
                self.dispatch_group(
                    session_id,
                    turn_id,
                    group,
                    callback,
                    iteration,
                    &budget_turn_id,
                    run_cancel,
                )
                .await?,
            );
        }
```

with a group-boundary checkpoint:

```rust
        for (start, end) in groups {
            // Cooperative steer checkpoint at the group boundary. Groups run
            // sequentially; a parallel group's `.buffered()` wave is not
            // interruptible mid-flight by design, so we stop *before*
            // launching the next group when a mid-turn user message arrived,
            // and defer everything still pending. R7/R10: mechanical.
            if self.has_unanswered_user_message(session_id).await {
                let deferred: Vec<NativeToolCall> = remaining.by_ref().collect();
                if !deferred.is_empty() {
                    self.emit_deferred_tool_results(session_id, turn_id, &deferred)
                        .await?;
                }
                if let Some(ref tracker) = self.stall_tracker {
                    tracker.record_activity().await;
                }
                break;
            }
            let group: Vec<NativeToolCall> = remaining.by_ref().take(end - start).collect();
            executed_count = executed_count.saturating_add(
                self.dispatch_group(
                    session_id,
                    turn_id,
                    group,
                    callback,
                    iteration,
                    &budget_turn_id,
                    run_cancel,
                )
                .await?,
            );
        }
```

- [ ] **Step 4: Run test to verify it passes** — **(do not execute — project constraint)**.

- [ ] **Step 5: Commit**

```bash
git add src/harness/agent/act.rs src/harness/tests/act.rs
git commit -m "harness: cooperative steer checkpoint at parallel group boundary"
```

---

## Task 4: Doc note in `act` module header

**Files:**
- Modify: `src/harness/agent/act.rs` (module-level `//!` doc comment, top of file)

- [ ] **Step 1: Add the doc note** (no test — documentation only)

Append to the module doc comment a short paragraph:

```rust
//! ## Cooperative steer checkpoint
//!
//! Before dispatching each serial tool call (and before each parallel
//! group), Act re-checks [`AgentHarness::has_unanswered_user_message`]. If a
//! non-synthetic user message arrived after this turn's prompt boundary
//! (`last_prompt_log_len` watermark) — the user changed their mind mid-batch
//! — the remaining not-yet-started tools are skipped, each gets a synthetic
//! "deferred" `ToolResult` (so the `tool_use`↔`tool_result` pairing the
//! provider requires stays intact), and Act returns. The next Think surfaces
//! the new message + deferred results; the model decides to pivot or re-issue
//! (R7). In-flight tools are never killed (use `/stop` / `Interrupt` mode for
//! that). When gateway-side `mid_turn_steering` is off no message is injected
//! mid-turn, so the predicate never fires and behaviour is unchanged.
```

- [ ] **Step 2: Commit**

```bash
git add src/harness/agent/act.rs
git commit -m "docs: document cooperative steer checkpoint in act module"
```

---

## Self-Review

**Spec coverage:**
- §2 goal (skip not-yet-started tools, return to Think) → Tasks 2 + 3.
- §4.1 Pull/watermark signal → reuse `has_unanswered_user_message` (Tasks 2/3).
- §4.2 avoid divergent copy → achieved by reuse-in-place (documented deviation; no extraction).
- §4.3 serial checkpoint → Task 2.
- §4.4 parallel checkpoint → Task 3 (group-boundary; documented deviation).
- §4.5 deferred `tool_result` contract → Task 1 helper + asserted in Tasks 1/2/3.
- §4.6 turn returns Continue, next Think consumes → unchanged caller (think.rs:1214 already sets `Continue`); covered by `executed` count flowing through.
- §6 guards: synthetic-exclusion + watermark + AssistantMessage guard → inherited from the reused predicate; `mid_turn_steering=false` inertness documented (Task 4) and asserted by the no-steer regression test (Task 2).
- §7 entropy: no dead code added; no copy created. Documented why no extraction.
- §8 tests: predicate correctness is the reused method's existing concern; deferral/contract/regression → Tasks 1–3.

**Placeholder scan:** No TBD/TODO. The `..Default::default()` in test `AssistantMessage` seeds is explicitly flagged as "match existing tests' field set" with instruction not to invent fields — acceptable because the exact variant fields live in this test file's existing tests.

**Type consistency:** `emit_deferred_tool_results(&self, &SessionId, TurnId, &[NativeToolCall]) -> Result<(), HarnessError>` used identically in Tasks 1/2/3. `ToolOutput { value, metadata }`, `SessionEvent::ToolResult { turn_id, call_id, output, at }`, and the predicate `has_unanswered_user_message(&self, &SessionId) -> bool` match the real signatures read from source.

**Regression safety:** `mid_turn_steering=false` ⇒ no mid-turn injection ⇒ predicate returns false every call ⇒ batch runs fully (the spec's "byte-identical" is refined to "no deferral occurs; the only delta is bounded extra in-memory event reads" — stated honestly in the spec note and Task 4 doc).
