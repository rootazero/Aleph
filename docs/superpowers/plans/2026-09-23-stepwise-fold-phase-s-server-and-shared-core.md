# Stepwise Transcript Folding — Phase S (Server + Shared Core) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Put every medium-independent piece of per-iteration transcript folding in place — one ordered event pipeline on the server (so `agent_trace` frames can no longer trail the text they precede), a `ReasoningEmitted` trace event that gives replay the model's per-iteration thinking, and the `shared-ui-logic::transcript` `Step` container plus the pure `Transcript` reducer that both legs (live frames, `trace.by_runs` rows) fold through — so Phase T (TUI) and Phase P (Panel) are painting only.

**Architecture:** `AgentTraceEmitSink` stops spawning its own drain task and sends step events onto the run's existing `broadcast<FlowStreamEvent>` channel as `FlowStreamEvent::Trace(AgentTraceEvent)`; the one drain assigns `seq` in program order. The harness emits `LoopTraceEvent::ReasoningEmitted { iteration, text }` beside `TextEmitted{Final}` at its two producers, which reaches `task_traces` (replayed by `trace.by_runs` with zero new code) and the wire through the existing sink chain. In `shared-ui-logic::transcript`, `TranscriptEntry::Step(StepEntry)` holds one Think→Act iteration (thinking + text + tool rows + notes); `reducer::Transcript` turns `StreamEvent`s and `AgentTraceEvent`s into entries through one internal `apply_trace`, hoists the final answer at run end, and reports `NeedsResync` when `RunSummary.loops` disagrees with the steps it saw. Nothing renders any of this in Phase S — a sanctioned, dated zero-consumer interval closed by Phase T.

**Tech Stack:** Rust 1.96 (MSRV 1.95), tokio `broadcast`, serde, `unicode-width` 0.2 (already a dep), `proptest` 1.4 (new dev-dep of `shared-ui-logic` only).

**Spec:** `docs/superpowers/specs/2026-09-23-stepwise-transcript-folding-design.md` (rulings R1–R9 in §2; R9 = reasoning via a harness trace variant, committed `8735a3180`).

## Global Constraints

- Work in a worktree created by `superpowers:using-git-worktrees` (suggested name `stepwise-fold-s` → `D:\Workspace\Aleph\.claude\worktrees\stepwise-fold-s`, Bash path `/d/Workspace/Aleph/.claude/worktrees/stepwise-fold-s`). Single-branch project: the branch merges into `main` by fast-forward at the end; the worktree is kept, never removed from inside the session.
- Commit messages: English, `<scope>: <description>`. No attribution trailers (disabled globally).
- **R10 ratchet.** `src/harness/tests/budget.rs::CEILING` is `5250` at branch start. Task 5 is the only task that adds lines under `src/harness/` (one enum variant + two emit sites); it raises `CEILING` by the **measured** delta and answers the three R10 questions in its commit body. Task 3 edits one comment in `src/harness/tests/budget.rs` with the same line count (zero delta; `tests/` is outside the budget anyway — `harness_sources()` walks only `src/harness/*.rs` and `src/harness/agent/*.rs`).
- **Wire.** `aleph_protocol::StreamEvent` gains no variant and no field; `frame_census.rs` is untouched (measured: it scans only `frame.rs` and `pub enum StreamEvent {`). `aleph_protocol::AgentTraceEvent` gains exactly one variant, `ReasoningEmitted { iteration: usize, text: String }`, `kind = "reasoning_emitted"`. Older clients ignore unknown kinds (the Panel routes by `kind` string; the TUI gets its arm in Task 4).
- **No wildcard arms.** `FlowStreamEvent`, `LoopTraceEvent` and `AgentTraceEvent` are matched exhaustively on purpose; every site the compiler names gets a real arm. Never add `_ =>`, `other =>` or `..` to make one compile.
- **Fail-closed (判据 §8).** `None` means "unknown". A step with no `TurnStarted` has `iteration: None` and is never renumbered. `Running` never survives a replay (`settle_resumed`).
- `shared-ui-logic::transcript` must compile under `default-features = false` (the TUI build): no feature gates, no leptos, no web-sys. Verify with `cargo build -p shared-ui-logic --no-default-features` in Task 12.
- Masking discipline: reasoning is model-authored text. Unattended runs mask it at write (`mask_trace_event`, Task 5) exactly like `TextEmitted`; the live emitter already masks `StreamEvent::Reasoning` (`redacting.rs:101`). Do not add a third masker.
- Verification instruments (from memory, re-checked here): run cargo from the **Bash tool** (Git Bash; PowerShell PATH lacks `cygpath` and a Unix `cat`). For `check` / `--lib` / clippy from the worktree use `CARGO_TARGET_DIR=D:/Workspace/Aleph/target`; if a build in the shared dir reports `unresolved import aleph_protocol::…` or `no field … on ToolResult` in an untouched crate, run `cargo clean -p aleph-protocol -p shared-ui-logic` first (artifact collision, memory `shared-cargo-target-dir-across-worktrees`). `cargo test -p alephcore --features test-helpers --test '*' --no-run` needs `-j 1` (E0463 under parallelism is a resource limit). Compare `--lib` failure **names**, not counts, against the baseline captured in Task 0. `python - <<EOF` silently no-ops on this host; apply edits with the editor. `rustfmt --edition 2021 <file>` per touched file, never `cargo fmt -p alephcore`.

## Review Focus

Inputs the spec implies but no task's tests would exercise unless pinned here. Each line names the owning task and the test added there.

1. **A `reasoning`/`response_chunk` frame arrives before any `turn_started`** (a client on an older server, or the `simple.rs` / slash paths that never emit trace events). Expected: it lands in a step with `iteration: None`, and a later `TurnStarted` opens a *new* step rather than adopting it. → Task 9, `a_frame_before_any_turn_started_opens_an_unnumbered_step_that_is_never_renumbered`.
2. **Parallel tool calls in one iteration** (several `tool_start`s before any `tool_end`). Expected: one step, every row settles by its own id, none is reset by the other's start. → Task 9, `parallel_tool_calls_in_one_iteration_share_the_step_and_settle_independently`.
3. **An iteration that produced thinking and nothing else** (verifier veto forced a continue), followed by an iteration with the answer. Expected: step 1 survives as a thinking-only step, the final answer is hoisted from step 2, no empty step is left behind. → Task 9, `run_complete_hoists_the_final_text_and_keeps_a_thinking_only_step` (one of four cells in the hoist matrix).
4. **A dropped `turn_started` under broadcast lag.** Expected: at `run_complete`, `summary.loops` exceeds the steps seen, the reducer reports `NeedsResync` and leaves the entries alone (never patches them). → Task 9, `fewer_turn_starts_than_summary_loops_reports_needs_resync_without_touching_entries`.
5. **Thinking that contains a PEM private key on an unattended run.** Expected: the `task_traces` row and the wire frame both carry the masked text; the diff-specific `Unavailable::Redacted` label is *not* borrowed for it. → Task 5, `reasoning_emitted_is_masked_before_persistence_on_unattended_runs`; Task 6, `a_reasoning_row_replays_masked_on_the_by_runs_leg`.
6. **`response.thinking` is `Some("")`** (providers that send an empty thinking block). Expected: no `ReasoningEmitted` is emitted, so replay does not gain an empty row that would render as "Thought for 0s". → Task 5, `an_empty_thinking_block_emits_no_reasoning_event`.

---

## File Structure

**Create**
- `shared/ui_logic/src/transcript/step.rs` — `StepEntry`, `StepStatus`, `ThinkingBlock`, `Note`, `NoteKind`, `StepTool`, `Headline`, `HeadlineSource`, `first_sentence`, `step_headline`, `step_tally`.
- `shared/ui_logic/src/transcript/detail.rs` — `DetailLevel`, `effective_open`.
- `shared/ui_logic/src/transcript/reducer.rs` — `Transcript`, `Change`, the live/replay fold.

**Modify**
- `src/orchestrator/dispatch.rs` — `FlowStreamEvent::Trace`, `FlowRequest.event_tx`, `flow_event_channel()`, Step 7 subscribes to a supplied channel.
- `src/gateway/execution_engine/agent_trace_emit_sink.rs` — rewritten around a `broadcast::Sender`; module doc rewritten.
- `src/gateway/execution_engine/event_drain.rs` — `Trace` arm; two stale "lossy mirror" comments.
- `src/gateway/execution_engine/run_loop/inner.rs` — creates the channel, builds the sink from it, hands it to the request.
- `src/orchestrator/tests/dispatch.rs` — eight `FlowRequest` literals gain `event_tx: None`; one new test.
- `src/gateway/event_emitter/mod.rs`, `types.rs` — CUT `emit_agent_trace` and `StreamEvent::agent_trace` once they have zero callers.
- `src/gateway/execution_engine/unattended_redacting_sink.rs` — `ReasoningEmitted` arm in `mask_trace_event`; one stale comment.
- `src/harness/trace.rs`, `src/harness/agent/think.rs`, `src/harness/tests/budget.rs` — the variant, two emits, ratchet.
- `src/gateway/trace_protocol.rs` — `From` arm.
- `shared/protocol/src/events.rs`, `shared/protocol/src/trace_presentation.rs` — the protocol variant, `kind()`, a presentation arm and label.
- `interfaces/tui/src/tui/app/trace.rs` — one compile-only arm (Phase T replaces it).
- `interfaces/webchat/src/platform/wide/views/agent_trace_model.rs` — an arm only if the compiler names it (its match has one wildcard today; verify).
- `src/gateway/handlers/trace_replay.rs` — one test (replay of the new row); no production change.
- `shared/ui_logic/Cargo.toml`, `src/transcript/mod.rs`, `view_model.rs`, `group.rs`, `turn_summary.rs` — dev-dep, exports, `TranscriptEntry::Step`, `group_tool_rows`, duration from `RowStatus`.
- `docs/reference/TRANSCRIPT_RENDERING.md`, `docs/reference/FEATURE_LOCATOR.md`, `docs/reference/SESSION_KNOBS.md`, the spec's status line.

**Test** — each new file carries `#[cfg(test)] mod tests`. Cross-crate guards: G1 (order) in `agent_trace_emit_sink.rs`; G2 (two legs, one fold) in `reducer.rs`; G3 (masking) in `unattended_redacting_sink.rs` + `trace_replay.rs`; the harness emit in `src/harness/tests/think.rs`.

---

## Part 0 — Baseline

### Task 0: Capture the branch-start baseline

**Files:** none (scratchpad only)

- [ ] **Step 1: Record the tip and the ratchet**

```bash
cd /d/Workspace/Aleph/.claude/worktrees/stepwise-fold-s
git rev-parse --short HEAD
grep -n "^const CEILING" src/harness/tests/budget.rs
```

Expected: the sha (write it into every later "measured at" note) and `const CEILING: usize = 5250;`.

- [ ] **Step 2: Capture the alephcore `--lib` failure names (detached, ~13 min build on this host)**

```bash
mkdir -p "$SCRATCH/baseline"
CARGO_TARGET_DIR=D:/Workspace/Aleph/target CARGO_PROFILE_TEST_DEBUG=line-tables-only \
  cargo test -p alephcore --lib -- --test-threads=1 2>&1 | tee "$SCRATCH/baseline/alephcore-lib.out" | tail -5
grep -E "^test .* \.\.\. FAILED" "$SCRATCH/baseline/alephcore-lib.out" | sed 's/^test \(.*\) \.\.\. FAILED/\1/' | sort > "$SCRATCH/baseline/failed-names.txt"
wc -l "$SCRATCH/baseline/failed-names.txt"
```

`$SCRATCH` is this session's scratchpad directory. Run with `run_in_background: true`; the foreground Bash ceiling is 10 min. The count is not the deliverable — the sorted **names** file is. Memory `windows-alephcore-lib-baseline-2026-09-02` lists the known Windows/Git-Bash reds; do not investigate them here.

- [ ] **Step 3: Confirm the other crates are green at the tip**

```bash
cargo test -p shared-ui-logic --no-default-features 2>&1 | tail -3
cargo test -p aleph-protocol 2>&1 | tail -3
cargo test -p aleph-tui 2>&1 | tail -3
```

Expected: `test result: ok` for all three (last measured: shared-ui-logic 154, aleph-protocol 360, aleph-tui 434 — re-read the numbers you get; they are the branch-start figures Task 12 diffs against).

---

## Part 1 — One pipeline (spec §5.1)

### Task 1: `FlowStreamEvent::Trace` and its drain arm

**Files:**
- Modify: `src/orchestrator/dispatch.rs:43-85` (the enum)
- Modify: `src/gateway/execution_engine/event_drain.rs:58-243` (`emit_flow_event`), tests at `:507+`

**Interfaces:**
- Produces: `FlowStreamEvent::Trace(aleph_protocol::AgentTraceEvent)`; `emit_flow_event` turns it into `StreamEvent::AgentTrace { run_id, seq, event }` with the run's next `seq`.

- [ ] **Step 1: Write the failing test**

In `src/gateway/execution_engine/event_drain.rs`, inside `mod tests`, after `delta_goes_to_emitter_text_delta`:

```rust
    /// A trace event on the flow channel becomes an `AgentTrace` frame with
    /// the run's NEXT seq — the same counter the surrounding text/tool frames
    /// draw from. Before this arm existed, trace frames took their seq on a
    /// separate task and could land after the text they logically preceded.
    #[tokio::test]
    async fn trace_goes_to_emitter_agent_trace_with_the_next_seq() {
        let (inner, emitter) = make_emitter();
        let state = make_state();

        emit_flow_event(
            FlowStreamEvent::Delta("before".to_string()),
            &emitter,
            "run-1",
            &state,
        )
        .await
        .expect("emit ok");
        emit_flow_event(
            FlowStreamEvent::Trace(aleph_protocol::AgentTraceEvent::TurnStarted { iteration: 3 }),
            &emitter,
            "run-1",
            &state,
        )
        .await
        .expect("emit ok");

        let events = inner.events().await;
        assert_eq!(events.len(), 2, "one chunk, one trace frame: {events:?}");
        let chunk_seq = match &events[0] {
            StreamEvent::ResponseChunk { seq, .. } => *seq,
            other => panic!("expected ResponseChunk first, got {other:?}"),
        };
        match &events[1] {
            StreamEvent::AgentTrace {
                run_id,
                seq,
                event: aleph_protocol::AgentTraceEvent::TurnStarted { iteration },
            } => {
                assert_eq!(run_id, "run-1");
                assert_eq!(*iteration, 3);
                assert!(*seq > chunk_seq, "trace seq {seq} must follow the chunk's {chunk_seq}");
            }
            other => panic!("expected AgentTrace(TurnStarted), got {other:?}"),
        }
    }
```

- [ ] **Step 2: Run it to see it fail**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib gateway::execution_engine::event_drain::tests::trace_goes_to_emitter -- --exact 2>&1 | tail -20
```

Expected: compile error `no variant named Trace found for enum FlowStreamEvent`.

- [ ] **Step 3: Add the variant**

In `src/orchestrator/dispatch.rs`, inside `pub enum FlowStreamEvent`, after the `SafetyBlock { reason: String },` variant and before `Complete(FlowOutcome)`:

```rust
    /// A step-relevant harness trace event, mirrored onto THIS channel so it
    /// takes its `seq` from the same serial drain as the text / thinking /
    /// tool frames around it. Carried as the protocol type: the sink filters
    /// (`is_step_event`) and converts before sending, so the drain only
    /// stamps `run_id` + `seq`. Until 2026-09-23 these frames rode a separate
    /// `mpsc` + spawned task and could be sequenced after the `ResponseChunk`s
    /// of the iteration they open — the race the Panel's `begin_step` patched
    /// around.
    Trace(aleph_protocol::AgentTraceEvent),
```

`AgentTraceEvent` is `Debug + Clone` (`shared/protocol/src/events.rs:456`), which the enum's derive needs.

- [ ] **Step 4: Add the drain arm**

In `src/gateway/execution_engine/event_drain.rs`, inside `emit_flow_event`'s `match event`, after the `SafetyBlock` arm and before `Complete`:

```rust
        FlowStreamEvent::Trace(event) => {
            // Same counter, same task as every other frame of this run: that
            // is the whole reason the trace mirror moved onto this channel.
            let seq = emitter.next_seq();
            emitter
                .emit(StreamEvent::AgentTrace {
                    run_id: run_id.to_string(),
                    seq,
                    event,
                })
                .await?;
        }
```

- [ ] **Step 5: Run the test to see it pass, then the drain module**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib gateway::execution_engine::event_drain -- --test-threads=1 2>&1 | tail -5
```

Expected: the new test passes; every pre-existing `event_drain` test still passes. If the compiler names any *other* `match` over `FlowStreamEvent` (it should not — `helpers.rs:355` binds `Ok(event)` for everything but `Complete`; the harness_bridge tests use `other => panic!`), add a real arm there, never a wildcard.

- [ ] **Step 6: Commit**

```bash
git add src/orchestrator/dispatch.rs src/gateway/execution_engine/event_drain.rs
git commit -m "orchestrator: add FlowStreamEvent::Trace and its inline drain arm

A step-relevant AgentTraceEvent can now ride the run's own broadcast
channel and take its seq from the serial drain, beside the text and tool
frames. Nothing sends it yet — the sink moves onto the channel in the
next commit."
```

### Task 2: The sink sends on the run's channel; the request carries the channel

**Files:**
- Modify: `src/orchestrator/dispatch.rs` (`CHANNEL_BUFFER_SIZE` at `:589`, `FlowRequest` at `:427-`, Step 7 at `:1047-1050`)
- Modify: `src/gateway/execution_engine/agent_trace_emit_sink.rs` (whole file)
- Modify: `src/gateway/execution_engine/run_loop/inner.rs:1008-1019` and `:1371-1381`
- Modify: `src/orchestrator/tests/dispatch.rs` (eight `FlowRequest {` literals: `:118`, `:158`, `:192`, `:230`, `:314`, `:347`, `:522`, `:565`)

**Interfaces:**
- Produces: `crate::orchestrator::flow_event_channel() -> (broadcast::Sender<FlowStreamEvent>, broadcast::Receiver<FlowStreamEvent>)`; `FlowRequest.event_tx: Option<broadcast::Sender<FlowStreamEvent>>`; `AgentTraceEmitSink::new(inner: Arc<dyn TraceSink>, tx: broadcast::Sender<FlowStreamEvent>) -> Self`.
- Consumes: `FlowStreamEvent::Trace` (Task 1).

- [ ] **Step 1: Write the failing sink test (G1a — the publish is synchronous)**

Replace the whole `mod tests` of `src/gateway/execution_engine/agent_trace_emit_sink.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::trace::{LoopTraceSessionOutcome, LoopTraceTextKind};
    use crate::harness::trace_sink::NoopTraceSink;
    use crate::orchestrator::flow_event_channel;

    #[test]
    fn forwards_turn_and_text_and_tool_events() {
        assert!(is_step_event(&LoopTraceEvent::TurnStarted { iteration: 1 }));
        assert!(is_step_event(&LoopTraceEvent::TextEmitted {
            iteration: 1,
            stream: LoopTraceTextKind::Final,
            text: "hi".into(),
        }));
        assert!(is_step_event(&LoopTraceEvent::TurnStarted { iteration: 2 }));
    }

    #[test]
    fn forwards_recovery_and_watchdog_events() {
        // The two "why did the loop change course" moments must reach the wire.
        assert!(is_step_event(
            &LoopTraceEvent::ReactiveCompactionAttempted {
                token_gap: Some(1200),
                succeeded: true,
            }
        ));
        assert!(is_step_event(&LoopTraceEvent::VerifierVeto {
            iteration: 3,
            reason: "- [ ] ship auth".into(),
        }));
    }

    #[test]
    fn drops_non_step_events() {
        // Session metrics are not Panel-relevant — must not hit the wire.
        let session_completed = LoopTraceEvent::SessionCompleted {
            outcome: LoopTraceSessionOutcome::Completed,
            iterations: 2,
            tool_calls_made: 1,
            total_tokens: 10,
            hit_limit: false,
            final_text: None,
            terminate_reason: None,
            duration_ms: None,
            token_breakdown: None,
            tool_timeline: Vec::new(),
        };
        assert!(!is_step_event(&session_completed));
        let (tx, mut rx) = flow_event_channel();
        let sink = AgentTraceEmitSink::new(Arc::new(NoopTraceSink), tx);
        sink.on_trace(&session_completed);
        assert!(
            rx.try_recv().is_err(),
            "a non-step event must not be published on the flow channel"
        );
    }

    /// The mirror publishes SYNCHRONOUSLY onto the run's flow channel — no
    /// spawned task, no `.await`. The previous design handed the event to a
    /// background task, so nothing was observable until the test yielded;
    /// this `try_recv` with no runtime at all is what went red on it.
    #[test]
    fn on_trace_publishes_onto_the_flow_channel_before_returning() {
        let (tx, mut rx) = flow_event_channel();
        let sink = AgentTraceEmitSink::new(Arc::new(NoopTraceSink), tx);

        sink.on_trace(&LoopTraceEvent::TurnStarted { iteration: 2 });

        match rx.try_recv() {
            Ok(FlowStreamEvent::Trace(aleph_protocol::AgentTraceEvent::TurnStarted {
                iteration,
            })) => assert_eq!(iteration, 2),
            other => panic!("expected the turn boundary on the flow channel, got {other:?}"),
        }
    }

    /// `ProviderUsage` must REACH the wire, not merely satisfy the predicate —
    /// the TUI's live `cache N%` cell is built from it. Drained through the
    /// real `emit_flow_event` so the assertion is on the delivered frame.
    #[tokio::test]
    async fn provider_usage_reaches_the_emitter() {
        use crate::gateway::event_emitter::{CollectingEventEmitter, StreamEvent};
        use crate::gateway::execution_engine::event_drain::{emit_flow_event, DrainState};

        let (tx, mut rx) = flow_event_channel();
        let sink = AgentTraceEmitSink::new(Arc::new(NoopTraceSink), tx);
        sink.on_trace(&LoopTraceEvent::ProviderUsage {
            agent_id: "main".into(),
            input_tokens: 120,
            output_tokens: 30,
            cache_read_tokens: Some(4_000),
            cache_creation_tokens: Some(0),
            thinking_tokens: None,
        });

        let inner = Arc::new(CollectingEventEmitter::new());
        let emitter: Arc<dyn EventEmitter> = inner.clone();
        let state = Arc::new(tokio::sync::Mutex::new(DrainState::default()));
        while let Ok(ev) = rx.try_recv() {
            emit_flow_event(ev, &emitter, "run-cache", &state)
                .await
                .expect("drain ok");
        }
        assert!(
            inner
                .events()
                .await
                .iter()
                .any(|e| matches!(e, StreamEvent::AgentTrace { .. })),
            "ProviderUsage must be mirrored to the wire — the live cache cell reads it"
        );
    }
}
```

`EventEmitter` is still imported by the production half until Step 3 rewrites it; keep `use crate::gateway::event_emitter::EventEmitter;` only if something in the file still names it (after Step 3 nothing does — drop the import then).

- [ ] **Step 2: Run to see it fail**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib gateway::execution_engine::agent_trace_emit_sink -- --test-threads=1 2>&1 | tail -20
```

Expected: compile errors — `flow_event_channel` not found in `crate::orchestrator`, `AgentTraceEmitSink::new` takes 3 arguments.

- [ ] **Step 3: Rewrite the sink**

Replace everything above `#[cfg(test)]` in `src/gateway/execution_engine/agent_trace_emit_sink.rs` with:

```rust
//! `AgentTraceEmitSink` — forwards the harness `LoopTraceEvent` stream to the
//! WebSocket event stream as `StreamEvent::AgentTrace`.
//!
//! ## Why this exists
//!
//! The harness emits a structured trace stream (`TurnStarted`, `TextEmitted`,
//! `ToolCallStarted/Completed`, …) via the [`TraceSink`] for persistence and
//! channel progress. The Panel's per-step segmentation and the TUI's step
//! folding consume those events on the wire as `agent_trace` notifications
//! (each carries an `iteration`, the per-step key). The gateway run path
//! drains the *separate* `FlowStreamEvent` stream into `response_chunk` /
//! `tool_start` / `tool_end`; without this sink no `agent_trace` frame would
//! ever be emitted.
//!
//! ## Why it sends on the flow channel (2026-09-23)
//!
//! Until then this sink queued events on its own `mpsc` and emitted them from
//! a spawned task. `seq` is assigned at emit time, so a `turn_started` could
//! take a LATER seq than the `response_chunk`s of the iteration it opens —
//! the race the Panel's `begin_step` patched around. Now `on_trace` does a
//! synchronous `broadcast::Sender::send` of [`FlowStreamEvent::Trace`] onto
//! the run's one channel, and the one serial drain (`helpers.rs`) stamps the
//! seq. `on_trace` and the harness callback (`on_delta` / `on_reasoning`) run
//! on the same task in program order, so seq order is now logical order.
//!
//! * It adds **zero** new emit points — it only mirrors events already in
//!   the trace stream, filtered by [`is_step_event`].
//! * It never blocks: `broadcast::send` is synchronous and lossless for the
//!   sender; a receiver that falls behind sees `Lagged`, which the drain
//!   logs (`helpers.rs`) and the client catches at `run_complete` via
//!   `RunSummary.loops` (spec §5.1).
//! * A send with zero receivers (the drain already returned) is ignored,
//!   exactly as `BroadcastCallback` ignores it.
//! * It always forwards the original event to the inner sink, so trace
//!   persistence + scratchpad progress are unaffected.
//! * On unattended runs `UnattendedRedactingSink` wraps OUTSIDE this sink
//!   (`run_loop/inner.rs`), so every event is masked before it is published;
//!   `RedactingEmitter` relies on that and passes `AgentTrace` through
//!   (`redacting.rs`), pinned by a test that reads `mask_trace_event`.
//!
//! Only the step-relevant variants are forwarded (see [`is_step_event`]) —
//! the heavy/internal ones (session metrics, worktree/MCP lifecycle) carry no
//! user-facing meaning and would only add wire noise.
//!
//! `ProviderUsage` is an explicit exception to the "internal" rule: it is the
//! sole source of the live prompt-cache reading, and it fires once per LLM
//! call rather than per delta, so the wire-noise argument does not apply.
//! `CacheHealthDegraded` rides along for the same reason: it is the alarm
//! *about* that number, fires once per miss-streak (rising edge), and was a
//! bare log line before — the one signal in this domain that must not stay
//! invisible.

use crate::sync_primitives::Arc;
use tokio::sync::broadcast;

use crate::harness::trace::LoopTraceEvent;
use crate::harness::TraceSink;
use crate::orchestrator::dispatch::FlowStreamEvent;

/// True for the trace variants the clients consume as `agent_trace`: turn
/// boundaries, authoritative per-step text, tool lifecycle, plus the two
/// recovery/watchdog moments that explain *why* the loop changed course —
/// reactive context compaction (problem: context overflow → handled: history
/// compacted → next: retried) and a structural goal-loop veto (problem:
/// checklist incomplete → next: forced continue). Also the three lightweight
/// MoA fan-out moments (advisor answer, aggregator hand-off, advisor spend) —
/// `MoaTurnTrace` is deliberately excluded: it carries the full advisor I/O
/// payload and is persisted-only, never wire.
///
/// And `ProviderUsage`, which is what the TUI's `cache N%` cell is built from
/// (`interfaces/tui/.../app/trace.rs` → `AppState.cache_stat` →
/// `widgets/status_bar.rs`). It was previously dropped here as "internal",
/// which left that cell — the product's only *live* prompt-cache indicator —
/// unable to fire during a run: it could only appear when a user manually
/// replayed a persisted trace, after the fact. A broken prefix is silent by
/// nature (the symptom is the bill), so this is the one number that has to
/// reach a live surface. Volume is one event per LLM call, not per delta.
///
/// Everything else is dropped — it carries no user-facing meaning.
pub(crate) const fn is_step_event(event: &LoopTraceEvent) -> bool {
    matches!(
        event,
        LoopTraceEvent::TurnStarted { .. }
            | LoopTraceEvent::TextEmitted { .. }
            | LoopTraceEvent::ToolCallStarted { .. }
            | LoopTraceEvent::ToolCallCompleted { .. }
            | LoopTraceEvent::ReactiveCompactionAttempted { .. }
            | LoopTraceEvent::VerifierVeto { .. }
            | LoopTraceEvent::MoaAdvisor { .. }
            | LoopTraceEvent::MoaAggregating { .. }
            | LoopTraceEvent::MoaAdvisorSpend { .. }
            | LoopTraceEvent::ProviderUsage { .. }
            | LoopTraceEvent::CacheHealthDegraded { .. }
    )
}

/// Decorator over a parent [`TraceSink`] that mirrors step-relevant trace
/// events onto the run's flow channel, while always forwarding the original
/// event to the inner sink.
pub struct AgentTraceEmitSink {
    inner: Arc<dyn TraceSink>,
    tx: broadcast::Sender<FlowStreamEvent>,
}

impl AgentTraceEmitSink {
    /// Wrap `inner`. `tx` must be the SAME sender the run's harness callback
    /// publishes on (`FlowRequest.event_tx`, created by
    /// `orchestrator::flow_event_channel`) — a second channel would put the
    /// ordering race straight back.
    pub fn new(inner: Arc<dyn TraceSink>, tx: broadcast::Sender<FlowStreamEvent>) -> Self {
        Self { inner, tx }
    }
}

impl TraceSink for AgentTraceEmitSink {
    fn on_trace(&self, event: &LoopTraceEvent) {
        if is_step_event(event) {
            // The only send error is "zero receivers" (the drain is gone); it
            // must not abort the harness loop — same rule as
            // `BroadcastCallback`.
            let _ = self
                .tx
                .send(FlowStreamEvent::Trace(event.clone().into()));
        }
        self.inner.on_trace(event);
    }

    fn flush(&self) {
        self.inner.flush();
    }
}
```

`event.clone().into()` is the existing `impl From<LoopTraceEvent> for aleph_protocol::AgentTraceEvent` at `src/gateway/trace_protocol.rs:21`.

- [ ] **Step 4: Add `flow_event_channel` and `FlowRequest.event_tx`**

In `src/orchestrator/dispatch.rs`, directly after `const CHANNEL_BUFFER_SIZE: usize = 256;` (line 589):

```rust
/// The run's flow-event channel, at the one capacity every run uses.
///
/// Callers that must publish onto the run's channel BEFORE `dispatch` runs
/// (the gateway's `AgentTraceEmitSink`, built in `run_loop/inner.rs` inside
/// the unattended redacting wrap) create the channel here and hand the sender
/// through [`FlowRequest::event_tx`]; `dispatch` subscribes to it instead of
/// creating its own. One channel per run, never two.
pub fn flow_event_channel() -> (
    broadcast::Sender<FlowStreamEvent>,
    broadcast::Receiver<FlowStreamEvent>,
) {
    broadcast::channel::<FlowStreamEvent>(CHANNEL_BUFFER_SIZE)
}
```

Re-export it: in `src/orchestrator/mod.rs` next to where `FlowRequest` is re-exported (`rg -n "pub use dispatch::" src/orchestrator/mod.rs`), add `flow_event_channel` to that list.

In `pub struct FlowRequest`, directly after the `trace_sink` field (`:482`):

```rust
    /// The run's flow-event channel, when the caller already publishes on it.
    /// The gateway's `AgentTraceEmitSink` must send `FlowStreamEvent::Trace`
    /// on the same channel the harness callback uses, or trace frames lose
    /// their place in the run's `seq` order — and that sink is built in
    /// `run_loop/inner.rs`, INSIDE the unattended redacting wrap, before
    /// `dispatch` runs. `None` = `dispatch` creates the channel itself
    /// (tests, callers with no trace mirror). Always from
    /// [`flow_event_channel`], so the capacity is one number.
    /// Not included in `Debug` output.
    pub event_tx: Option<broadcast::Sender<FlowStreamEvent>>,
```

The hand-written `impl std::fmt::Debug for FlowRequest` (`:562`) lists fields by name; leave it — the new field is deliberately not printed.

In Step 7 (`:1047-1048`), replace

```rust
        let (event_tx, event_rx) = broadcast::channel::<FlowStreamEvent>(CHANNEL_BUFFER_SIZE);
```

with

```rust
        // One channel per run. A caller that already publishes on it (the
        // gateway's trace mirror) hands the sender in; we subscribe. The
        // subscription must happen before the harness task starts — a
        // broadcast receiver only sees sends made after it exists.
        let (event_tx, event_rx) = match req.event_tx.clone() {
            Some(tx) => {
                let rx = tx.subscribe();
                (tx, rx)
            }
            None => flow_event_channel(),
        };
```

- [ ] **Step 5: Build the channel in the run loop and hand it to both owners**

In `src/gateway/execution_engine/run_loop/inner.rs`, replace lines 1008-1019 (the comment block + `AgentTraceEmitSink::new(trace_sink, Arc::clone(&emitter), run_id.to_string())`) with:

```rust
            // Forward the harness trace stream to this run's WebSocket as
            // `agent_trace` notifications so the clients can segment the
            // transcript per Think→Act step. The sink publishes on the run's
            // OWN flow channel (created here, handed to `dispatch` through
            // `FlowRequest.event_tx`) so trace frames take their `seq` from
            // the same serial drain as the text and tool frames — a second
            // channel is the ordering race this replaced. Forwards to the
            // inner (persistence + scratchpad) sink unchanged; on unattended
            // runs the redacting sink below wraps OUTSIDE this one, so every
            // event is masked before it is published.
            let (event_tx, _initial_rx) = crate::orchestrator::flow_event_channel();
            let trace_sink: Arc<dyn crate::harness::TraceSink> = Arc::new(
                super::super::AgentTraceEmitSink::new(trace_sink, event_tx.clone()),
            );
```

`_initial_rx` is dropped at once; nothing is published before `dispatch` subscribes (the harness has not started), and a send with zero receivers is ignored by the sink.

In the `FlowRequest` literal at `:1371`, directly after `trace_sink: Some(trace_sink),`:

```rust
                event_tx: Some(event_tx.clone()),
```

Both lines are inside the retry `loop` (`:680`), like the sink chain — one channel per attempt, matching one sink per attempt.

- [ ] **Step 6: Fix the eight test literals and add the subscription test**

In `src/orchestrator/tests/dispatch.rs`, every `FlowRequest {` literal (`:118`, `:158`, `:192`, `:230`, `:314`, `:347`, `:522`, `:565`) gains, directly after its `trace_sink: None,` line:

```rust
            event_tx: None,
```

Then add, after `dispatch_happy_path_returns_handle_and_completes`:

```rust
/// A caller-supplied channel is the one `dispatch` subscribes to. Anything
/// the caller publishes on its sender must reach `handle.events` — that is
/// what lets the gateway's trace mirror share the harness callback's seq
/// order instead of racing it on a second channel.
#[tokio::test]
async fn dispatch_subscribes_to_a_caller_supplied_channel() {
    let (orch, _invocations) = fixture_orchestrator();
    let (tx, _rx) = crate::orchestrator::flow_event_channel();
    let handle = orch
        .dispatch(FlowRequest {
            flow_id: None,
            agent_id: "main".into(),
            input: FlowInput::Prompt("hello".into()),
            channel: None,
            session_hint: None,
            scope: crate::scope::FlowScope::unscoped(),
            parent_session: None,
            depth: 0,
            tool_service: None,
            trace_sink: None,
            event_tx: Some(tx.clone()),
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
        .expect("dispatch ok");

    // Published on the CALLER's sender, after dispatch subscribed.
    tx.send(crate::orchestrator::dispatch::FlowStreamEvent::Reasoning(
        "from-caller".into(),
    ))
    .expect("the handle's receiver must be subscribed");

    // The mock harness sends its own `Delta` + `Complete` on the same channel
    // from a spawned task, in an order we do not control relative to the
    // send above — so read until OUR frame shows up (it was sent after the
    // subscription, so it must), bounded so a broken subscription fails
    // instead of hanging.
    let mut events = handle.events;
    let mut saw_caller_frame = false;
    for _ in 0..8 {
        match tokio::time::timeout(std::time::Duration::from_secs(2), events.recv()).await {
            Ok(Ok(crate::orchestrator::dispatch::FlowStreamEvent::Reasoning(t)))
                if t == "from-caller" =>
            {
                saw_caller_frame = true;
                break;
            }
            Ok(Ok(_)) => {}
            Ok(Err(_)) | Err(_) => break,
        }
    }
    assert!(saw_caller_frame, "the caller's frame never reached handle.events");
    let _ = handle.completion.await;
}
```

(`tokio` with the `time` feature is already a dependency of alephcore — `helpers.rs` uses `tokio::time`.)

- [ ] **Step 7: Build and run the three affected modules**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib gateway::execution_engine::agent_trace_emit_sink orchestrator::tests::dispatch gateway::execution_engine::event_drain -- --test-threads=1 2>&1 | tail -8
```

Expected: all green, including the five sink tests, the new dispatch test and every pre-existing dispatch test. Also run the source-reading guard that pins the masking order:

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib gateway::event_emitter::redacting::tests::the_agent_trace_exemption_is_backed_by_an_exhaustive_upstream_match -- --exact 2>&1 | tail -3
```

Expected: PASS (nothing in this task touches `mask_trace_event`).

- [ ] **Step 8: Commit**

```bash
git add src/orchestrator/dispatch.rs src/orchestrator/mod.rs src/gateway/execution_engine/agent_trace_emit_sink.rs src/gateway/execution_engine/run_loop/inner.rs src/orchestrator/tests/dispatch.rs
git commit -m "gateway: trace mirror publishes on the run's flow channel, not a spawned drain

AgentTraceEmitSink used to queue step events on its own mpsc(256) and
emit them from a tokio::spawn'd task; seq is assigned at emit time, so
a turn_started could be sequenced after the response_chunks of the
iteration it opens (the race the Panel's begin_step patches around).
The sink now sends FlowStreamEvent::Trace synchronously on the same
broadcast channel the harness callback uses. run_loop/inner.rs creates
that channel with orchestrator::flow_event_channel and hands the sender
to both the sink and FlowRequest.event_tx; dispatch subscribes to a
supplied channel instead of creating a second one. Redacting wrap order
is unchanged (the sink stays inside UnattendedRedactingSink)."
```

### Task 3: G1 — order through the drain; CUT the dead emit path; fix the four stale comments

**Files:**
- Modify: `src/gateway/execution_engine/agent_trace_emit_sink.rs` (one test)
- Modify: `src/gateway/event_emitter/mod.rs:119-132` (CUT `emit_agent_trace`), `src/gateway/event_emitter/types.rs:396-407` (CUT `StreamEvent::agent_trace` if zero callers)
- Modify: `src/gateway/execution_engine/event_drain.rs:37-44` and `:143-147`, `src/gateway/execution_engine/unattended_redacting_sink.rs:78-84`, `src/harness/tests/budget.rs:386-390` (comments only)

- [ ] **Step 1: Write the failing order test (G1b)**

Append to `mod tests` in `agent_trace_emit_sink.rs`:

```rust
    /// G1. Program order — thinking, then the turn boundary, then text —
    /// survives into seq order once every frame goes through the one drain.
    /// On the pre-2026-09-23 design the `TurnStarted` frame took its seq on
    /// a spawned task and could not be sequenced between the two sends
    /// made here without a yield; the sink test above pins the mechanism,
    /// this one pins the effect.
    #[tokio::test]
    async fn trace_frames_keep_their_place_in_the_run_seq_order() {
        use crate::gateway::event_emitter::{CollectingEventEmitter, StreamEvent};
        use crate::gateway::execution_engine::event_drain::{emit_flow_event, DrainState};

        let (tx, mut rx) = flow_event_channel();
        let sink = AgentTraceEmitSink::new(Arc::new(NoopTraceSink), tx.clone());
        tx.send(FlowStreamEvent::Reasoning("thinking".into())).unwrap();
        sink.on_trace(&LoopTraceEvent::TurnStarted { iteration: 2 });
        tx.send(FlowStreamEvent::Delta("hello".into())).unwrap();

        let inner = Arc::new(CollectingEventEmitter::new());
        let emitter: Arc<dyn EventEmitter> = inner.clone();
        let state = Arc::new(tokio::sync::Mutex::new(DrainState::default()));
        while let Ok(ev) = rx.try_recv() {
            emit_flow_event(ev, &emitter, "run-order", &state)
                .await
                .expect("drain ok");
        }

        let frames: Vec<(&str, u64)> = inner
            .events()
            .await
            .iter()
            .map(|e| match e {
                StreamEvent::Reasoning { seq, .. } => ("reasoning", *seq),
                StreamEvent::AgentTrace { seq, .. } => ("agent_trace", *seq),
                StreamEvent::ResponseChunk { seq, .. } => ("response_chunk", *seq),
                other => panic!("unexpected frame {other:?}"),
            })
            .collect();
        assert_eq!(
            frames.iter().map(|f| f.0).collect::<Vec<_>>(),
            ["reasoning", "agent_trace", "response_chunk"],
            "{frames:?}"
        );
        assert!(
            frames.windows(2).all(|w| w[0].1 < w[1].1),
            "seq must be strictly increasing in program order: {frames:?}"
        );
    }
```

`EventEmitter` must be in scope for this test: `use crate::gateway::event_emitter::EventEmitter;` inside the test module.

- [ ] **Step 2: Run it**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib gateway::execution_engine::agent_trace_emit_sink::tests::trace_frames_keep -- --exact 2>&1 | tail -5
```

Expected: PASS on the Task 2 code. (It is red on the pre-Task-2 code: `on_trace` there did not touch `rx`, so the `agent_trace` frame is absent from `frames` — this test travels with G1a, which is the deterministic half.)

- [ ] **Step 3: CUT the two dead emit helpers**

```bash
rg -n "emit_agent_trace|StreamEvent::agent_trace\(" src/ interfaces/ --type rust
```

Expected: only the definitions (`event_emitter/mod.rs:120`, `types.rs:397`). Delete `async fn emit_agent_trace` (mod.rs lines 119-132, including its doc line) and `pub fn agent_trace` (types.rs 396-407, including its doc line). If the grep shows a caller you did not expect, that caller is a consumer this plan miscounted — keep the helper and record the site in the commit body.

- [ ] **Step 4: Fix the four comments that describe the old mechanism**

`src/gateway/execution_engine/event_drain.rs:39-40` — replace `reconcile a checklist whose live frames the lossy \`agent_trace\` WS` / `mirror dropped` with:

```rust
    /// reconcile a checklist whose live `agent_trace` frames a lagging
    /// receiver dropped — the same authoritative-terminal-state contract
```

(keep the surrounding lines as they are).

`event_drain.rs:144-146` — replace the three comment lines with:

```rust
                // Latch the terminal execution list. This drain is in-process
                // and reads every frame the channel delivered, so what we
                // capture here is authoritative.
```

`src/gateway/execution_engine/unattended_redacting_sink.rs:80-81` — replace `channel and \`AgentTraceEmitSink\` puts on the WS)` with `channel and \`AgentTraceEmitSink\` publishes to the run's flow channel)`. Same line count.

`src/harness/tests/budget.rs:388` — replace `is deliberately lossy (\`AgentTraceEmitSink\` = bounded \`mpsc(256)\` +` / `\`try_send\`); a block was therefore the one class of call with no` with:

```rust
///     can lag (`AgentTraceEmitSink` publishes on the run's broadcast flow
///     channel); a block was therefore the one class of call with no
```

Two lines replace two lines. `tests/` is outside `harness_sources()` regardless.

Then verify nothing else still names the old mechanism:

```bash
rg -n "mpsc\(256\)|try_send|spawned task|lossy .*mirror" src/gateway/execution_engine/ src/harness/tests/budget.rs
```

Expected: no hits that describe `AgentTraceEmitSink` (other `try_send`s in unrelated sinks may exist — read each hit).

- [ ] **Step 5: Run the touched modules + clippy on the crate**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib gateway::execution_engine gateway::event_emitter orchestrator -- --test-threads=1 2>&1 | tail -5
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo clippy -p alephcore --lib 2>&1 | grep -E "^(warning|error)" | sort | uniq -c | sort -rn | head
```

Expected: green; no new clippy warnings in files this part touched (the pre-existing 31 warnings live elsewhere — memory `cc-render-r1-round`).

- [ ] **Step 6: Commit**

```bash
git add src/gateway/execution_engine/agent_trace_emit_sink.rs src/gateway/event_emitter/mod.rs src/gateway/event_emitter/types.rs src/gateway/execution_engine/event_drain.rs src/gateway/execution_engine/unattended_redacting_sink.rs src/harness/tests/budget.rs
git commit -m "gateway: pin trace-frame seq order (G1); cut the dead emit_agent_trace path

The order guard drives thinking → turn_started → text through the one
drain and asserts the seq order. emit_agent_trace and
StreamEvent::agent_trace had zero callers after the sink moved onto the
flow channel. Four comments still described the retired mpsc(256) +
spawned-task mirror; rewritten at the same line count (budget.rs is a
test file and outside the ratchet either way)."
```

---

## Part 2 — `ReasoningEmitted` (spec §5.2, ruling R9)

### Task 4: The protocol variant and every consumer arm outside the harness

**Files:**
- Modify: `shared/protocol/src/events.rs:456-647` (`AgentTraceEvent` + `kind()`)
- Modify: `shared/protocol/src/trace_presentation.rs:75-98` (labels), `:103` (`english()`), `:228-` (`present_agent_trace_event`)
- Modify: `interfaces/tui/src/tui/app/trace.rs:284-320` (the presentation dispatch match), `interfaces/webchat/src/platform/wide/views/agent_trace_model.rs` (only if the compiler names it)

**Interfaces:**
- Produces: `aleph_protocol::AgentTraceEvent::ReasoningEmitted { iteration: usize, text: String }`, JSON `{"kind":"reasoning_emitted","iteration":N,"text":"…"}`; `AgentTracePresentationLabels.reasoning_emitted: String` (English `"Reasoning"`).

- [ ] **Step 1: Write the failing protocol tests**

In `shared/protocol/src/events.rs`, inside the file's `mod tests` (the one holding `test_reasoning_block_serialization` at `:1140`):

```rust
    #[test]
    fn reasoning_emitted_round_trips_with_its_kind_tag() {
        let event = AgentTraceEvent::ReasoningEmitted {
            iteration: 4,
            text: "Considering the timezone bug first.".into(),
        };
        assert_eq!(event.kind(), "reasoning_emitted");
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"kind\":\"reasoning_emitted\""), "{json}");
        assert!(json.contains("\"iteration\":4"), "{json}");
        let back: AgentTraceEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, event);
    }
```

In `shared/protocol/src/trace_presentation.rs`'s `mod tests`:

```rust
    #[test]
    fn reasoning_emitted_presents_under_its_own_label_and_is_clipped() {
        let event = AgentTraceEvent::ReasoningEmitted {
            iteration: 2,
            text: "x".repeat(10_000),
        };
        let p = present_agent_trace_event_with_preset(&event, AgentTracePresentationPreset::TuiDebug)
            .expect("reasoning has a presentation");
        assert_eq!(p.kind, "reasoning_emitted");
        assert_eq!(p.status, AgentTracePresentationStatus::Info);
        assert!(p.content.starts_with("[Reasoning] iter 2: "), "{}", p.content);
        let limit = AgentTracePresentationPreset::TuiDebug.options().content_limit;
        assert!(
            p.content.chars().count() < limit + 40,
            "content must be clipped by content_limit ({limit}), got {}",
            p.content.chars().count()
        );
    }
```

- [ ] **Step 2: Run to see them fail**

```bash
cargo test -p aleph-protocol reasoning_emitted 2>&1 | tail -8
```

Expected: `no variant named ReasoningEmitted`.

- [ ] **Step 3: Add the variant and its `kind()`**

In `shared/protocol/src/events.rs`, inside `pub enum AgentTraceEvent`, directly after the `TextEmitted { .. }` variant:

```rust
    /// The model's thinking for one Think iteration, authoritative and whole
    /// (the provider's summarized reasoning where the provider summarizes;
    /// raw chain-of-thought otherwise — the wire cannot tell). Emitted beside
    /// `TextEmitted{Final}` by the same two producers, only when the block is
    /// non-empty. Live thinking still streams as `StreamEvent::Reasoning`
    /// deltas; this is the per-iteration record a replay reconstructs a
    /// folded step from. Absent on runs recorded before 2026-09-23.
    ReasoningEmitted { iteration: usize, text: String },
```

In `kind()`, after `Self::TextEmitted { .. } => "text_emitted",`:

```rust
            Self::ReasoningEmitted { .. } => "reasoning_emitted",
```

- [ ] **Step 4: Add the label and the presentation arm**

In `shared/protocol/src/trace_presentation.rs`, in `pub struct AgentTracePresentationLabels`, after `pub text_final: String,`:

```rust
    /// Label for the per-iteration reasoning record (`ReasoningEmitted`).
    pub reasoning_emitted: String,
```

In `english()`, after the `text_final:` initializer:

```rust
            reasoning_emitted: "Reasoning".into(),
```

(The only constructor outside this file is `AgentTracePresentationLabels::english()` at `interfaces/webchat/.../agent_trace_model.rs:25`; a struct literal elsewhere would now fail to compile — that is the census.)

In `present_agent_trace_event`, after the `TextEmitted` arm:

```rust
        AgentTraceEvent::ReasoningEmitted { iteration, text } => Some(AgentTracePresentation {
            kind: event.kind().into(),
            status: AgentTracePresentationStatus::Info,
            content: format!(
                "[{}] iter {}: {}",
                labels.reasoning_emitted,
                iteration,
                truncate(text, options.content_limit)
            ),
            duration_ms: None,
        }),
```

- [ ] **Step 5: Give the TUI its compile-only arm**

`cargo check -p aleph-tui` now fails in `interfaces/tui/src/tui/app/trace.rs` (its match over `AgentTraceEvent` is exhaustive). In the `default_trace_presentation` dispatch (the match at `:284-320` whose arms call `append_reasoning_entry` / `{}`), add beside `TextEmitted`:

```rust
            // Phase S: the per-iteration reasoning record. The live deltas
            // already reached the transcript through `StreamEvent::Reasoning`;
            // until Phase T folds steps, the authoritative copy is ignored
            // here rather than appended twice.
            AgentTraceEvent::ReasoningEmitted { .. } => {}
```

and, if the second match in `apply_agent_trace_event` (`:340-420`) is also exhaustive, the same arm returning `Action::None`. Then:

```bash
cargo check --workspace --exclude aleph-desktop-macos --exclude aleph-desktop-linux 2>&1 | grep -E "^error" -A 6 | head -40
```

Expected: any remaining `non-exhaustive patterns` error names a file; add a real arm there (the Panel's `agent_trace_model.rs` match currently carries one wildcard, so it most likely compiles unchanged — confirm by reading its match rather than by the absence of an error). `src/gateway/trace_protocol.rs` does NOT need an arm yet: its match is over `LoopTraceEvent` (Task 5).

- [ ] **Step 6: Run the protocol and TUI suites**

```bash
cargo test -p aleph-protocol 2>&1 | tail -3
cargo test -p aleph-tui 2>&1 | tail -3
cargo build -p aleph-tui 2>&1 | tail -2
```

Expected: protocol count = branch-start + 2; aleph-tui unchanged; non-test build clean (a test build alone hides dead fields — memory `cc-render-r1-round`).

- [ ] **Step 7: Commit**

```bash
git add shared/protocol/src/events.rs shared/protocol/src/trace_presentation.rs interfaces/tui/src/tui/app/trace.rs
git commit -m "protocol: AgentTraceEvent::ReasoningEmitted — the per-iteration reasoning record

kind = reasoning_emitted; presentation arm under its own label. The
TUI's exhaustive match gets a compile-only arm (Phase T folds it into
steps). No producer yet — the harness emits it in the next commit."
```

### Task 5: The harness emits it; the core arms; the ratchet

**Files:**
- Modify: `src/harness/trace.rs:24-31` (variant), `src/harness/agent/think.rs:896-901` and `:1335-1339` (two emits), `src/harness/tests/budget.rs:687` (`CEILING`)
- Modify: `src/gateway/trace_protocol.rs:24-32` (`From` arm), `src/gateway/execution_engine/unattended_redacting_sink.rs:48-52` (`mask_trace_event` arm), `src/gateway/execution_engine/agent_trace_emit_sink.rs:66-81` (`is_step_event`)
- Test: `src/harness/tests/think.rs`, `src/gateway/execution_engine/unattended_redacting_sink.rs` tests, `src/gateway/trace_protocol.rs` tests

**Interfaces:**
- Produces: `crate::harness::trace::LoopTraceEvent::ReasoningEmitted { iteration: usize, text: String }`; conversion to the Task 4 protocol variant; masking on unattended runs; wire mirroring via `is_step_event`.

- [ ] **Step 1: Write the failing harness test (the emit exists and fires once per non-empty thinking block)**

In `src/harness/tests/think.rs`, after `think_with_no_tool_use_returns_done`:

```rust
/// One Think iteration with a thinking block records exactly one
/// `ReasoningEmitted` beside its `TextEmitted{Final}`, carrying the same
/// iteration and the whole block. This is the record `trace.by_runs`
/// replays a folded step from.
#[tokio::test]
async fn a_think_turn_with_thinking_emits_one_reasoning_record() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("hello")]);
    let (sink, recorded) = super::stability::RecordingTraceSink::new();
    let provider = Arc::new(FixedProvider {
        response: ProviderResponse {
            text: Some("hi".into()),
            thinking: Some("Weighing the two readings of the question.".into()),
            ..Default::default()
        },
    });
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(EmptyTools),
        llm: provider,
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: Some(sink as Arc<dyn crate::harness::TraceSink>),
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
        guardrails: None,
        max_iterations: None,
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        turn_budget: None,
        result_store: None,
        session_epoch_registrar: None,
        tool_signal_sink: std::sync::Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
    };
    let harness = AgentHarness::new(deps);
    harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn should succeed");

    let events = recorded.lock().unwrap_or_else(|e| e.into_inner());
    let reasoning: Vec<(usize, String)> = events
        .iter()
        .filter_map(|e| match e {
            LoopTraceEvent::ReasoningEmitted { iteration, text } => Some((*iteration, text.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        reasoning,
        vec![(1, "Weighing the two readings of the question.".to_string())],
        "exactly one reasoning record, on iteration 1: {events:?}"
    );
    let text_iteration = events.iter().find_map(|e| match e {
        LoopTraceEvent::TextEmitted { iteration, .. } => Some(*iteration),
        _ => None,
    });
    assert_eq!(text_iteration, Some(1), "the text record shares the iteration");
}

/// `Some("")` — a provider that sends an empty thinking block — must not
/// produce a record that would later render as "Thought for 0s".
#[tokio::test]
async fn an_empty_thinking_block_emits_no_reasoning_event() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("hello")]);
    let (sink, recorded) = super::stability::RecordingTraceSink::new();
    let provider = Arc::new(FixedProvider {
        response: ProviderResponse {
            text: Some("hi".into()),
            thinking: Some(String::new()),
            ..Default::default()
        },
    });
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(EmptyTools),
        llm: provider,
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: Some(sink as Arc<dyn crate::harness::TraceSink>),
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
        guardrails: None,
        max_iterations: None,
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        turn_budget: None,
        result_store: None,
        session_epoch_registrar: None,
        tool_signal_sink: std::sync::Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
    };
    let harness = AgentHarness::new(deps);
    harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn should succeed");

    let events = recorded.lock().unwrap_or_else(|e| e.into_inner());
    assert!(
        !events.iter().any(|e| matches!(e, LoopTraceEvent::ReasoningEmitted { .. })),
        "an empty thinking block must not be recorded: {events:?}"
    );
}
```

`use crate::harness::trace::LoopTraceEvent;` at the top of `think.rs` if not already imported; `super::stability` is a sibling module under `harness::tests` (`src/harness/mod.rs:30`) and `RecordingTraceSink::new` is `pub(super)` there. `ProviderResponse` has `text: Option<String>` and `thinking: Option<String>` (used at `think.rs:836,908`).

- [ ] **Step 2: Run to see them fail**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib harness::tests::think::a_think_turn_with_thinking harness::tests::think::an_empty_thinking_block 2>&1 | tail -8
```

Expected: `no variant named ReasoningEmitted found for enum LoopTraceEvent`.

- [ ] **Step 3: The variant (`src/harness/trace.rs`)**

Inside `pub enum LoopTraceEvent`, directly after the `TextEmitted { .. }` variant:

```rust
    /// The model's whole thinking block for this iteration (see the
    /// protocol twin's doc). Emitted only when non-empty.
    ReasoningEmitted { iteration: usize, text: String },
```

- [ ] **Step 4: The two emits (`src/harness/agent/think.rs`)**

At `:896-901`, the block currently reads:

```rust
            self.emit(|| crate::harness::trace::LoopTraceEvent::TextEmitted {
                iteration: iterations,
                stream: crate::harness::trace::LoopTraceTextKind::Final,
                text: text.clone(),
            });
        }
```

Directly AFTER the closing `}` of that `if !text.is_empty() { … }` block, and BEFORE `let blocks = super::tool_use_blocks(&response.tool_calls);`, insert:

```rust
        if let Some(thinking) = response.thinking.as_deref().filter(|t| !t.is_empty()) {
            self.emit(|| crate::harness::trace::LoopTraceEvent::ReasoningEmitted {
                iteration: iterations,
                text: thinking.to_string(),
            });
        }
```

At `:1335-1339` (the grace turn's `TextEmitted` emit inside `fire_boundary_grace_turn`), insert the same six lines directly after that emit, with `resp` in place of `response`:

```rust
        if let Some(thinking) = resp.thinking.as_deref().filter(|t| !t.is_empty()) {
            self.emit(|| crate::harness::trace::LoopTraceEvent::ReasoningEmitted {
                iteration: iterations,
                text: thinking.to_string(),
            });
        }
```

`self.emit(|| …)` is the existing lazy trace emit (`think.rs:896`).

- [ ] **Step 5: The three arms outside the harness**

`src/gateway/trace_protocol.rs`, in `impl From<LoopTraceEvent> for aleph_protocol::AgentTraceEvent`, after the `TextEmitted` arm (`:24-32`):

```rust
            LoopTraceEvent::ReasoningEmitted { iteration, text } => {
                Self::ReasoningEmitted { iteration, text }
            }
```

`src/gateway/execution_engine/unattended_redacting_sink.rs`, in `mask_trace_event`, after the `TextEmitted` arm (`:48-52`):

```rust
        LoopTraceEvent::ReasoningEmitted { iteration: _, text } => mask_in_place(masker, text),
```

`src/gateway/execution_engine/agent_trace_emit_sink.rs`, in `is_step_event`, after `| LoopTraceEvent::TextEmitted { .. }`:

```rust
            | LoopTraceEvent::ReasoningEmitted { .. }
```

Then `cargo check -p alephcore` and add a real arm at every other site the compiler names (candidates with exhaustive matches over `LoopTraceEvent`: none known besides these — `is_step_event` is a `matches!`, `routing/mod.rs` uses `matches!`/`if let`, `forwarding_trace_sink.rs` has a wildcard, `moa/provider.rs` and `metering.rs` construct only).

- [ ] **Step 6: Write the masking and conversion guards (G3, Review Focus 5)**

In `unattended_redacting_sink.rs` `mod tests`, modelled on the `TextEmitted` masking test at `:276`:

```rust
    #[test]
    fn reasoning_emitted_is_masked_before_persistence_on_unattended_runs() {
        let (inner, seen) = RecordingSink::new();
        let sink = UnattendedRedactingSink::new(inner);
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIEow\nAAAA\n-----END RSA PRIVATE KEY-----";
        sink.on_trace(&LoopTraceEvent::ReasoningEmitted {
            iteration: 1,
            text: format!("the key is {pem} and sk-ant-api03-AAAABBBBCCCCDDDD"),
        });
        let events = seen.lock().unwrap_or_else(|e| e.into_inner());
        let text = match &events[0] {
            LoopTraceEvent::ReasoningEmitted { text, .. } => text.clone(),
            other => panic!("expected ReasoningEmitted, got {other:?}"),
        };
        assert!(!text.contains("MIIEow"), "PEM body must be masked as one string: {text}");
        assert!(!text.contains("sk-ant-api03-AAAABBBBCCCCDDDD"), "{text}");
        assert!(text.contains("the key is"), "non-secret prose survives: {text}");
    }
```

Use the same recording-sink helper the `:276` test uses (read that test; if its helper is named differently, use that name — do not add a second recorder).

In `src/gateway/trace_protocol.rs` `mod tests` (create the module if the file has none, with `use super::*;`):

```rust
    #[test]
    fn reasoning_emitted_converts_field_for_field() {
        let wire: aleph_protocol::AgentTraceEvent = LoopTraceEvent::ReasoningEmitted {
            iteration: 7,
            text: "why".into(),
        }
        .into();
        assert_eq!(
            wire,
            aleph_protocol::AgentTraceEvent::ReasoningEmitted {
                iteration: 7,
                text: "why".into()
            }
        );
    }
```

In `agent_trace_emit_sink.rs` `mod tests`, extend `forwards_turn_and_text_and_tool_events` with:

```rust
        assert!(is_step_event(&LoopTraceEvent::ReasoningEmitted {
            iteration: 1,
            text: "why".into(),
        }));
```

- [ ] **Step 7: Run the harness, gateway and redacting suites; then measure the ratchet**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib harness::tests::think gateway::execution_engine::unattended_redacting_sink gateway::trace_protocol gateway::execution_engine::agent_trace_emit_sink gateway::event_emitter::redacting -- --test-threads=1 2>&1 | tail -6
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib harness::tests::budget -- --test-threads=1 2>&1 | grep -E "grew to|test result" | head -3
```

Expected: every suite green except `harness::tests::budget::the_harness_line_budget_does_not_grow`, whose message reads `grew to N budgeted lines, over the frozen ceiling of 5250`. Take **N** from that message (do not compute it by hand — `budgeted_lines` cuts each file at its top-level `#[cfg(test)]`).

- [ ] **Step 8: Raise the ratchet by the measured delta, answering the three questions**

In `src/harness/tests/budget.rs:687`, set `const CEILING: usize = N;` and add, at the end of the doc comment above it, following its existing entries' shape:

```rust
///     raised 2026-09-23 from 5250 → N: `LoopTraceEvent::ReasoningEmitted`
///     (3 lines) + its two emits in `agent/think.rs` beside the two
///     `TextEmitted{Final}` producers (6 lines each). R10 ①: what the
///     clients fold a step from must come from the loop's own iteration
///     counter — nothing outside the harness knows which iteration a
///     thinking block belongs to. ②: not a Skill/MCP — it is an
///     observability record of the loop itself. ③: nothing was deleted to
///     absorb it; the harness's trace vocabulary grew by one concern that
///     has no existing variant.
```

Re-run the budget test: PASS.

- [ ] **Step 9: Commit (the three answers live in the body too)**

```bash
git add src/harness/trace.rs src/harness/agent/think.rs src/harness/tests/budget.rs src/harness/tests/think.rs src/gateway/trace_protocol.rs src/gateway/execution_engine/unattended_redacting_sink.rs src/gateway/execution_engine/agent_trace_emit_sink.rs
git commit -m "harness: emit ReasoningEmitted beside TextEmitted{Final} (R9)

The per-iteration thinking block now enters the trace stream from the
loop's own iteration counter: think.rs emits it at both TextEmitted
producers (the Think turn and the grace turn), only when non-empty.
It reaches task_traces through GatewayTraceSink (trace.by_runs replays
it with no new code), the wire through is_step_event, and is masked at
write on unattended runs (mask_trace_event's exhaustive match gained
its arm).

R10 ratchet: CEILING 5250 -> N (measured). Why the harness: the
iteration a thinking block belongs to is a fact only the loop holds.
Why not a Skill/MCP: it is a record of the loop, not a capability.
What was deleted: nothing; a new concern with no existing variant."
```

### Task 6: Replay carries the row (zero new code) — prove it, masked

**Files:**
- Test: `src/gateway/handlers/trace_replay.rs` `mod tests` (`:734+`)

- [ ] **Step 1: Write the replay test**

After `by_runs_masks_the_replayed_presentation` (`:1511`):

```rust
    /// A persisted `ReasoningEmitted` row replays as its own `kind` beside
    /// the text row of the same iteration. Nothing in this handler changed
    /// for it — the assertion is that the row survives the trace store and
    /// the handler's per-event serialization untouched.
    #[tokio::test]
    async fn by_runs_replays_a_persisted_reasoning_row() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        db.insert_agent_task(&AgentTask::new("run-reason", "s", "coder", "x", RiskLevel::Low))
            .await
            .unwrap();
        db.insert_trace(&TaskTrace::new(
            "run-reason",
            0,
            AgentTraceEvent::ReasoningEmitted {
                iteration: 1,
                text: "Checking the timezone handling first.".into(),
            },
        ))
        .await
        .unwrap();
        db.insert_trace(&TaskTrace::new(
            "run-reason",
            1,
            AgentTraceEvent::TextEmitted {
                iteration: 1,
                stream: AgentTraceTextKind::Final,
                text: "Fixed.".into(),
            },
        ))
        .await
        .unwrap();

        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let key = SessionKey::main("conv-reasoning");
        seed_session(&sessions, &key, "u-alice", &["run-reason"]).await;

        let resp = CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_by_runs(
                    req(json!({
                        "session_key": key.to_key_string(),
                        "run_ids": ["run-reason"],
                    })),
                    db,
                    sessions,
                ),
            )
            .await;

        let result = resp.result.expect("success");
        let events = result
            .get("runs")
            .and_then(|r| r.get("run-reason"))
            .and_then(Value::as_array)
            .expect("run present")
            .clone();
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!(events[0]["kind"], "reasoning_emitted");
        assert_eq!(events[0]["iteration"], 1);
        assert_eq!(events[0]["text"], "Checking the timezone handling first.");
        assert_eq!(events[1]["kind"], "text_emitted");
    }
```

- [ ] **Step 2: Run it**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib gateway::handlers::trace_replay -- --test-threads=1 2>&1 | tail -4
```

Expected: PASS without touching the handler. (If it fails, the failure is real: something between `insert_trace` and `serde_json::to_value(&event)` rejects the variant — fix that, not the test.)

- [ ] **Step 3: Commit**

```bash
git add src/gateway/handlers/trace_replay.rs
git commit -m "gateway: pin that trace.by_runs replays a ReasoningEmitted row unchanged"
```

---

## Part 3 — Shared core (spec §4, §6)

### Task 7: `step.rs` — the iteration as data, its headline, its tally; `TranscriptEntry::Step`

**Files:**
- Create: `shared/ui_logic/src/transcript/step.rs`
- Modify: `shared/ui_logic/Cargo.toml` (new `[dev-dependencies]`), `shared/ui_logic/src/transcript/mod.rs` (exports), `view_model.rs:185-214` (`Step` variant), `group.rs` (`group_tool_rows`), `turn_summary.rs` (`summarize_rows`; duration from `RowStatus`)
- Modify: `interfaces/tui/src/tui/widgets/chat_area.rs` (compile-only arms), `interfaces/tui/src/tui/app/tests.rs:2174` (one arm)

**Interfaces:**
- Produces (all re-exported from `shared_ui_logic::transcript`): `StepEntry { id: String, iteration: Option<u32>, thinking: Option<ThinkingBlock>, text: Option<String>, tools: Vec<ToolRow>, notes: Vec<Note>, status: StepStatus, started_ms: Option<u64>, ended_ms: Option<u64> }`; `ThinkingBlock { text: String, streaming: bool }`; `Note { kind: NoteKind, text: String }`; `NoteKind { ToolSummary, VerifierVeto, ReactiveCompaction, MoaAdvisor, MoaAggregating, MoaAdvisorSpend }`; `StepStatus { Live, Settled, Pending }`; `StepTool { Row(ToolRow), Group(ToolGroup) }`; `Headline { text: String, source: HeadlineSource }`; `HeadlineSource { Thinking, Text, Tally }`; `HEADLINE_MAX_COLS: u16 = 80`; `first_sentence(&str, u16) -> Option<String>`; `step_headline(&StepEntry, u16) -> Option<Headline>`; `step_tally(&StepEntry) -> Option<TurnSummaryEntry>`; `StepEntry::{is_empty, find_tool_mut}`; `group_tool_rows(&[ToolRow]) -> Vec<StepTool>`; `summarize_rows(&[ToolRow]) -> Option<TurnSummaryEntry>`; `TranscriptEntry::Step(StepEntry)`.
- Deviation from spec §4, recorded here: `tools` holds raw `ToolRow`s and grouping is a paint-time projection (`group_tool_rows`), so the reducer's id lookup stays a flat scan and grouping cannot hide a row from `find_tool_mut` (the report's warning about `group_entries` not looking inside containers).

- [ ] **Step 1: Add the dev-dependency**

Append to `shared/ui_logic/Cargo.toml` (after the `[features]` table):

```toml
[dev-dependencies]
# Same 1.4 alephcore pins as a dev-dep (root Cargo.toml `[dev-dependencies]`).
# Only `transcript::step`'s width/boundary property tests use it.
proptest = "1.4"
```

- [ ] **Step 2: Write the failing tests for `step.rs`**

Create `shared/ui_logic/src/transcript/step.rs` with ONLY this test module first (the production half comes in Step 4):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{RowStatus, ToolRow};
    use aleph_protocol::ToolResult;
    use proptest::prelude::*;
    use serde_json::json;
    use unicode_width::UnicodeWidthChar;

    fn finished(id: &str, tool: &str, args: serde_json::Value, ok: bool, ms: u64) -> ToolRow {
        let mut r = ToolRow::new(id, tool, &args);
        r.start(1_000);
        let res = if ok {
            ToolResult::success("out")
        } else {
            ToolResult::error("boom")
        };
        r.finish(&res, ms, 1_000 + ms);
        r
    }

    fn step(thinking: Option<&str>, text: Option<&str>, tools: Vec<ToolRow>) -> StepEntry {
        StepEntry {
            id: "step-1".into(),
            iteration: Some(1),
            thinking: thinking.map(|t| ThinkingBlock {
                text: t.into(),
                streaming: false,
            }),
            text: text.map(str::to_string),
            tools,
            notes: Vec::new(),
            status: StepStatus::Settled,
            started_ms: None,
            ended_ms: None,
        }
    }

    #[test]
    fn first_sentence_stops_at_cjk_and_ascii_terminators_and_newlines() {
        assert_eq!(
            first_sentence("先看 CI 日志定位失败的测试。然后修。", 80).as_deref(),
            Some("先看 CI 日志定位失败的测试。")
        );
        assert_eq!(
            first_sentence("Reading src/parse.rs to find the bug. Then fixing it.", 80).as_deref(),
            Some("Reading src/parse.rs to find the bug."),
            "a dot inside a path is not a sentence end"
        );
        assert_eq!(
            first_sentence("first line\nsecond line", 80).as_deref(),
            Some("first line")
        );
        assert_eq!(first_sentence("   \n  ", 80), None);
    }

    #[test]
    fn first_sentence_clips_to_the_column_budget_with_an_ellipsis() {
        let h = first_sentence(&"很长的一句话".repeat(20), 12).unwrap();
        assert!(h.ends_with('…'), "{h}");
        let cols: usize = h.chars().map(|c| c.width().unwrap_or(0)).sum();
        assert!(cols <= 12, "{cols} columns: {h}");
    }

    proptest! {
        /// Any input: no panic, never over budget, never a half character,
        /// `None` exactly when the input is blank.
        #[test]
        fn first_sentence_never_panics_and_respects_the_width(s in "\\PC*", cols in 1u16..120) {
            match first_sentence(&s, cols) {
                Some(h) => {
                    let w: usize = h.chars().map(|c| c.width().unwrap_or(0)).sum();
                    prop_assert!(w <= usize::from(cols), "{w} > {cols}: {h:?}");
                    prop_assert!(!h.trim().is_empty());
                }
                None => prop_assert!(s.trim().is_empty()),
            }
        }
    }

    #[test]
    fn headline_prefers_thinking_then_text_then_tally() {
        let tools = vec![finished("t1", "file_read", json!({"path": "a.rs"}), true, 5)];
        let with_thinking = step(Some("Checking the tz code. More."), Some("Let me look."), tools.clone());
        let h = step_headline(&with_thinking, 80).unwrap();
        assert_eq!(h.source, HeadlineSource::Thinking);
        assert_eq!(h.text, "Checking the tz code.");

        let text_only = step(None, Some("Let me look at it. Now."), tools.clone());
        let h = step_headline(&text_only, 80).unwrap();
        assert_eq!(h.source, HeadlineSource::Text);
        assert_eq!(h.text, "Let me look at it.");

        let tally_only = step(None, None, tools);
        let h = step_headline(&tally_only, 80).unwrap();
        assert_eq!(h.source, HeadlineSource::Tally);
        // A 5 ms read: `Read 1 file · 0.0s` (`fmt_duration_ms(5)` is "0.0s").
        assert_eq!(h.text, "Read 1 file · 0.0s");
    }

    #[test]
    fn a_blank_thinking_block_falls_through_to_text() {
        let s = step(Some("   \n"), Some("Actually here."), Vec::new());
        let h = step_headline(&s, 80).unwrap();
        assert_eq!(h.source, HeadlineSource::Text);
    }

    #[test]
    fn an_empty_step_has_no_headline_and_reports_empty() {
        let s = step(None, None, Vec::new());
        assert!(s.is_empty());
        assert_eq!(step_headline(&s, 80), None);
    }

    #[test]
    fn a_step_with_only_a_note_is_not_empty() {
        let mut s = step(None, None, Vec::new());
        s.notes.push(Note {
            kind: NoteKind::VerifierVeto,
            text: "checklist incomplete".into(),
        });
        assert!(!s.is_empty());
    }

    #[test]
    fn step_tally_counts_one_tool_where_the_turn_summary_would_not() {
        let s = step(None, None, vec![finished("t1", "bash", json!({"command": "ls"}), true, 40)]);
        let t = step_tally(&s).expect("one tool is enough for a step tally");
        assert_eq!(t.commands, 1);
        assert_eq!(t.duration_ms, 40, "duration comes from RowStatus, not the clocks");
    }

    #[test]
    fn step_tally_is_unknown_while_no_row_is_terminal() {
        let mut r = ToolRow::new("t1", "bash", &json!({"command": "ls"}));
        r.start(5);
        let s = step(None, None, vec![r]);
        assert_eq!(step_tally(&s), None);
    }

    #[test]
    fn find_tool_mut_reaches_a_row_by_id() {
        let mut s = step(None, None, vec![finished("t9", "bash", json!({}), false, 1)]);
        assert!(matches!(s.find_tool_mut("t9").map(|r| &r.status), Some(RowStatus::Err { .. })));
        assert!(s.find_tool_mut("nope").is_none());
    }
}
```

- [ ] **Step 3: Run to see it fail**

```bash
cargo test -p shared-ui-logic --no-default-features transcript::step 2>&1 | tail -6
```

Expected: `module step is private`/unresolved — the module is not declared yet.

- [ ] **Step 4: Write the production half of `step.rs` (above the test module)**

```rust
//! One Think→Act iteration as data. The unit the transcript folds by
//! (spec §4): thinking + interstitial text + the tool rows it issued + the
//! loop's own notes about that iteration. Both surfaces paint a settled step
//! as one line (`step_headline`) and expand it on demand; the reducer
//! (`reducer.rs`) is the only writer.

use unicode_width::UnicodeWidthChar;

use super::turn_summary::summarize_rows;
use super::view_model::{ToolGroup, ToolRow, TurnSummaryEntry};

/// Columns a headline may occupy before it is clipped with `…`.
pub const HEADLINE_MAX_COLS: u16 = 80;

/// The provider's thinking for one iteration. `streaming` is true while
/// `Reasoning` deltas are still arriving; the authoritative
/// `ReasoningEmitted` record replaces the text and clears it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThinkingBlock {
    pub text: String,
    pub streaming: bool,
}

/// Step-scoped narration from the loop itself — never the model's thinking,
/// never shown unless the step is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteKind {
    ToolSummary,
    VerifierVeto,
    ReactiveCompaction,
    MoaAdvisor,
    MoaAggregating,
    MoaAdvisorSpend,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub kind: NoteKind,
    pub text: String,
}

/// `Live` while it is the run's current iteration; `Settled` once the next
/// `TurnStarted` or the run's end closed it; `Pending` when restored from a
/// replay that never saw it end — "unknown", never a fabricated success and
/// never a spinner (the `settle_resumed` rule, one level up).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Live,
    Settled,
    Pending,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepEntry {
    pub id: String,
    /// `None` = the server never said which iteration this is (a frame that
    /// arrived before any `TurnStarted`, or a path that emits no trace). Such
    /// a step is never renumbered by a later `TurnStarted` — that opens a new
    /// step (判据 §8).
    pub iteration: Option<u32>,
    pub thinking: Option<ThinkingBlock>,
    pub text: Option<String>,
    /// Raw rows in issue order. Grouping (`Explored N calls`) is a paint-time
    /// projection — see `group_tool_rows` — so a row is always reachable by
    /// id here.
    pub tools: Vec<ToolRow>,
    pub notes: Vec<Note>,
    pub status: StepStatus,
    pub started_ms: Option<u64>,
    pub ended_ms: Option<u64>,
}

impl StepEntry {
    /// Nothing to show and nothing to count: such a step is removed at run
    /// end rather than rendered as "Step N: (nothing)" (spec §9).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.thinking.as_ref().is_none_or(|t| t.text.trim().is_empty())
            && self.text.as_deref().is_none_or(|t| t.trim().is_empty())
            && self.tools.is_empty()
            && self.notes.is_empty()
    }

    pub fn find_tool_mut(&mut self, tool_id: &str) -> Option<&mut ToolRow> {
        self.tools.iter_mut().find(|r| r.id == tool_id)
    }
}

/// What a step's tool list becomes at paint time: consecutive read-only rows
/// fold into one `Explored N calls` group (same rule as `group_entries`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepTool {
    Row(ToolRow),
    Group(ToolGroup),
}

/// Where a headline's text came from — the surface may style them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlineSource {
    Thinking,
    Text,
    Tally,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Headline {
    pub text: String,
    pub source: HeadlineSource,
}

/// The first sentence of `s`, clipped to `max_cols` columns.
///
/// A sentence ends at `。！？!?`, at a newline, or at a `.` that is followed
/// by whitespace or the end (so `src/parse.rs` is not a sentence end).
/// `None` for blank input — a blank headline would read as "nothing was
/// thought", which is not known.
#[must_use]
pub fn first_sentence(s: &str, max_cols: u16) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut end = s.len();
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '\n' => {
                end = i;
                break;
            }
            '。' | '！' | '？' | '!' | '?' => {
                end = i + c.len_utf8();
                break;
            }
            '.' => {
                let next_is_break = matches!(chars.peek(), None | Some((_, ' ' | '\t' | '\n')));
                if next_is_break {
                    end = i + c.len_utf8();
                    break;
                }
            }
            _ => {}
        }
    }
    let sentence = s.get(..end).unwrap_or(s).trim_end();
    if sentence.is_empty() {
        return None;
    }
    Some(clip_cols(sentence, max_cols))
}

/// Clip to `max_cols` display columns, ending in `…` (one column) when
/// anything was removed. Never splits a character.
fn clip_cols(s: &str, max_cols: u16) -> String {
    let max = usize::from(max_cols.max(1));
    let total: usize = s.chars().map(|c| c.width().unwrap_or(0)).sum();
    if total <= max {
        return s.to_string();
    }
    let budget = max.saturating_sub(1);
    let mut width = 0usize;
    let mut cut = 0usize;
    for (i, c) in s.char_indices() {
        let w = c.width().unwrap_or(0);
        if width + w > budget {
            break;
        }
        width += w;
        cut = i + c.len_utf8();
    }
    format!("{}…", s.get(..cut).unwrap_or(""))
}

/// `Read 2 files, ran 1 command · 8s` for ONE step — no minimum-tool gate,
/// unlike the run-level `summarize_turn`. `None` while no row is terminal
/// (unknown, not "nothing ran").
#[must_use]
pub fn step_tally(step: &StepEntry) -> Option<TurnSummaryEntry> {
    summarize_rows(&step.tools)
}

/// Ruling R1: thinking's first sentence → text's first sentence → the tool
/// tally. `None` only for an empty step.
#[must_use]
pub fn step_headline(step: &StepEntry, max_cols: u16) -> Option<Headline> {
    if let Some(text) = step
        .thinking
        .as_ref()
        .and_then(|t| first_sentence(&t.text, max_cols))
    {
        return Some(Headline {
            text,
            source: HeadlineSource::Thinking,
        });
    }
    if let Some(text) = step
        .text
        .as_deref()
        .and_then(|t| first_sentence(t, max_cols))
    {
        return Some(Headline {
            text,
            source: HeadlineSource::Text,
        });
    }
    step_tally(step).map(|t| Headline {
        text: clip_cols(&super::turn_summary::turn_summary_text(&t), max_cols),
        source: HeadlineSource::Tally,
    })
}
```

`Option::is_none_or` is stable since Rust 1.82 (MSRV 1.95).

- [ ] **Step 5: `summarize_rows` + duration from `RowStatus` (`turn_summary.rs`)**

Replace `pub fn summarize_turn` with:

```rust
/// Run-level summary: needs at least [`MIN_TOOLS_FOR_SUMMARY`] rows (one
/// tool is not a "turn worth summarizing" at the trailer), then defers to
/// [`summarize_rows`].
#[must_use]
pub fn summarize_turn(rows: &[ToolRow]) -> Option<TurnSummaryEntry> {
    if rows.len() < MIN_TOOLS_FOR_SUMMARY {
        return None;
    }
    summarize_rows(rows)
}

/// Count what `rows` did, with no minimum — a single step's tally.
///
/// Every field is derived over the SAME row set: terminal (Ok/Err) rows
/// only. A `Pending` row is an outcome we refuse to vouch for (see
/// `ToolRow::settle_resumed`); it is skipped entirely — understating is the
/// fail-closed direction. Duration is the wire's `duration_ms` carried on
/// `RowStatus`, not `ended_ms - started_ms`: a row restored from a replay
/// has no clocks and must still add up to the same number a live row does
/// (the two-legs guard in `reducer.rs` compares them).
#[must_use]
pub fn summarize_rows(rows: &[ToolRow]) -> Option<TurnSummaryEntry> {
    let mut e = TurnSummaryEntry {
        commands: 0,
        reads: 0,
        edits: 0,
        writes: 0,
        others: 0,
        failed: 0,
        duration_ms: 0,
    };
    let mut read_paths = std::collections::HashSet::new();
    let mut edit_paths = std::collections::HashSet::new();
    let mut write_paths = std::collections::HashSet::new();
    let mut any_terminal = false;
    for r in rows {
        let duration_ms = match &r.status {
            RowStatus::Ok { duration_ms } | RowStatus::Err { duration_ms, .. } => *duration_ms,
            RowStatus::Pending | RowStatus::Running { .. } => continue,
        };
        any_terminal = true;
        match r.summary.display_name.as_str() {
            "Bash" => e.commands += 1,
            "Read" => {
                read_paths.insert(r.summary.args_text.clone());
            }
            "Edit" | "Patch" => {
                edit_paths.insert(r.summary.args_text.clone());
            }
            "Write" => {
                write_paths.insert(r.summary.args_text.clone());
            }
            _ => e.others += 1,
        }
        if let RowStatus::Err { .. } = r.status {
            e.failed += 1;
        }
        e.duration_ms += duration_ms;
    }
    // Every row unresolved is "we don't know yet", not "nothing happened":
    // a zeroed entry ("Ran 0 commands · 0s") would assert the latter.
    if !any_terminal {
        return None;
    }
    e.reads = read_paths.len() as u32;
    e.edits = edit_paths.len() as u32;
    e.writes = write_paths.len() as u32;
    Some(e)
}
```

Run `cargo test -p shared-ui-logic --no-default-features transcript::turn_summary`. The existing five tests must stay green; if one asserted a clock-derived duration that differs from the status duration, its fixture was inconsistent — make the fixture's `finish(…, duration, now)` consistent, do not weaken the assertion.

- [ ] **Step 6: `group_tool_rows` (`group.rs`)**

Append to `group.rs`:

```rust
use super::step::StepTool;
use super::view_model::ToolRow;

fn flush_run(run: &mut Vec<ToolRow>, out: &mut Vec<StepTool>) {
    if run.len() >= MIN_GROUP {
        out.push(StepTool::Group(ToolGroup {
            rows: std::mem::take(run),
            expanded: false,
        }));
    } else {
        out.extend(run.drain(..).map(StepTool::Row));
    }
}

/// The step-internal twin of [`group_entries`]: consecutive read-only rows
/// (≥ [`MIN_GROUP`]) become one group; anything else breaks the run. There
/// are no interleaved texts inside a step, so no gap rule applies.
#[must_use]
pub fn group_tool_rows(rows: &[ToolRow]) -> Vec<StepTool> {
    let mut out = Vec::new();
    let mut run: Vec<ToolRow> = Vec::new();
    for r in rows {
        if r.is_read_only() {
            run.push(r.clone());
        } else {
            flush_run(&mut run, &mut out);
            out.push(StepTool::Row(r.clone()));
        }
    }
    flush_run(&mut run, &mut out);
    out
}
```

Add to `group.rs` tests:

```rust
    #[test]
    fn group_tool_rows_folds_consecutive_reads_and_breaks_on_an_edit() {
        let read = |id: &str| {
            let mut r = ToolRow::new(id, "file_read", &serde_json::json!({"path": "a.rs"}));
            r.start(0);
            r
        };
        let edit = {
            let mut r = ToolRow::new("e", "file_edit", &serde_json::json!({"file_path": "a.rs"}));
            r.start(0);
            r
        };
        let out = group_tool_rows(&[read("r1"), read("r2"), edit, read("r3")]);
        assert!(matches!(&out[0], StepTool::Group(g) if g.rows.len() == 2));
        assert!(matches!(&out[1], StepTool::Row(r) if r.id == "e"));
        assert!(matches!(&out[2], StepTool::Row(r) if r.id == "r3"), "a run of one dissolves");
        assert_eq!(out.len(), 3);
    }
```

- [ ] **Step 7: `TranscriptEntry::Step` and the exports**

`view_model.rs`: add `use super::step::StepEntry;` and, in `pub enum TranscriptEntry`, after `ToolGroup(ToolGroup),`:

```rust
    /// One Think→Act iteration (spec §4). Phase S adds the variant; Phase T
    /// is its first producer and renderer.
    Step(StepEntry),
```

`mod.rs`: after the `mod group; mod view_model;` block add

```rust
mod step;
pub use step::{
    first_sentence, step_headline, step_tally, Headline, HeadlineSource, Note, NoteKind,
    StepEntry, StepStatus, StepTool, ThinkingBlock, HEADLINE_MAX_COLS,
};
```

extend the `group` export to `pub use group::{group_entries, group_tool_rows, MAX_GAP_TEXT, MIN_GROUP};` and the `turn_summary` export to include `summarize_rows`.

- [ ] **Step 8: Keep the TUI compiling (arms, not renderers)**

```bash
cargo check -p aleph-tui 2>&1 | grep -E "^error" -A 8 | head -60
```

Every `non-exhaustive patterns: TranscriptEntry::Step(_) not covered` names a site. Known: `interfaces/tui/src/tui/widgets/chat_area.rs` — the `MessageKind` fingerprint match (`:176-191`) and the render match (`:780-810`); `interfaces/tui/src/tui/app/tests.rs:2174`. Add:

- to `enum MessageKind`: `Step,`
- fingerprint arm: `TranscriptEntry::Step(s) => (MessageKind::Step, content_fingerprint(&s.id)),`
- render arm (with the same comment on all three):

```rust
        // Phase S: the variant exists, no TUI path constructs it yet. Phase T
        // replaces this with the step row; until then an entry that cannot
        // exist renders nothing rather than a guessed shape.
        TranscriptEntry::Step(_) => {}
```

- tests.rs arm: whatever that match extracts (it maps entries to text), `TranscriptEntry::Step(_) => None,` (if it is `find_map`) or the equivalent no-content arm.

No `_ =>`.

- [ ] **Step 9: Run everything this task touched**

```bash
cargo test -p shared-ui-logic --no-default-features 2>&1 | tail -3
cargo test -p shared-ui-logic 2>&1 | tail -3
cargo test -p aleph-tui 2>&1 | tail -3 && cargo build -p aleph-tui 2>&1 | tail -1
cargo clippy -p shared-ui-logic --no-default-features -- -D warnings 2>&1 | tail -3
```

Expected: all green in both feature shapes; aleph-tui unchanged; clippy clean.

- [ ] **Step 10: Commit**

```bash
git add shared/ui_logic/Cargo.toml shared/ui_logic/src/transcript/ interfaces/tui/src/tui/widgets/chat_area.rs interfaces/tui/src/tui/app/tests.rs
git commit -m "shared-ui-logic: StepEntry — the iteration as data, its headline and tally

TranscriptEntry::Step, step_headline (thinking first sentence -> text
first sentence -> tool tally, ruling R1), first_sentence with CJK/ASCII
terminators and a column clip (proptest-pinned), step_tally via a new
summarize_rows (no minimum-tool gate). Durations now come from
RowStatus, not ended_ms - started_ms, so a replayed row adds up to the
same number a live one does. group_tool_rows is the step-internal twin
of group_entries. The TUI gains compile-only arms; nothing paints a
step until Phase T."
```

### Task 8: `detail.rs` — the per-device level and the per-entry override

**Files:**
- Create: `shared/ui_logic/src/transcript/detail.rs`
- Modify: `shared/ui_logic/src/transcript/mod.rs`

**Interfaces:**
- Produces: `DetailLevel { Brief, Full }` (`Default = Brief`, `as_str() -> "brief" | "full"`, `parse(&str) -> Option<DetailLevel>`, `default_open(self) -> bool`); `effective_open(level, overrides: &HashSet<String>, id: &str) -> bool`.

- [ ] **Step 1: Write the failing tests**

Create `shared/ui_logic/src/transcript/detail.rs` containing only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn brief_is_the_default_and_round_trips_through_its_name() {
        assert_eq!(DetailLevel::default(), DetailLevel::Brief);
        for level in [DetailLevel::Brief, DetailLevel::Full] {
            assert_eq!(DetailLevel::parse(level.as_str()), Some(level));
        }
        assert_eq!(DetailLevel::parse("verbose"), None, "an unknown name is unknown, not Brief");
    }

    #[test]
    fn an_override_flips_the_level_default_for_that_entry_only() {
        let mut overrides = HashSet::new();
        overrides.insert("step-2".to_string());
        assert!(!effective_open(DetailLevel::Brief, &overrides, "step-1"));
        assert!(effective_open(DetailLevel::Brief, &overrides, "step-2"));
        assert!(effective_open(DetailLevel::Full, &overrides, "step-1"));
        assert!(!effective_open(DetailLevel::Full, &overrides, "step-2"));
    }
}
```

- [ ] **Step 2: Run to see it fail**

```bash
cargo test -p shared-ui-logic --no-default-features transcript::detail 2>&1 | tail -4
```

Expected: unresolved module.

- [ ] **Step 3: Write the production half (above the tests)**

```rust
//! The per-device detail level (ruling R7) and the per-entry override.
//!
//! `Brief` folds every settled step to one line; `Full` opens them. A click
//! or `Enter` on one step records an OVERRIDE for that entry — an XOR against
//! the level's default, the same shape the Panel already uses for tool rows
//! (`WorkspaceState.expanded_events`) — so switching level does not have to
//! rewrite every remembered choice. Persistence is the surface's (Panel
//! `localStorage`, TUI `<aleph_home>/tui-detail`); this module only decides.

use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetailLevel {
    #[default]
    Brief,
    Full,
}

impl DetailLevel {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Brief => "brief",
            Self::Full => "full",
        }
    }

    /// `None` for anything but the two names — an unreadable preference file
    /// falls back to the default at the CALLER, which must not read `None`
    /// as `Brief` silently without saying so.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "brief" => Some(Self::Brief),
            "full" => Some(Self::Full),
            _ => None,
        }
    }

    #[must_use]
    pub const fn default_open(self) -> bool {
        matches!(self, Self::Full)
    }
}

/// Whether the entry `id` is open under `level`, given the entries the user
/// toggled away from the level's default.
#[must_use]
pub fn effective_open(level: DetailLevel, overrides: &HashSet<String>, id: &str) -> bool {
    level.default_open() ^ overrides.contains(id)
}
```

`mod.rs`: `mod detail; pub use detail::{effective_open, DetailLevel};`

- [ ] **Step 4: Run, then commit**

```bash
cargo test -p shared-ui-logic --no-default-features transcript::detail 2>&1 | tail -3
git add shared/ui_logic/src/transcript/detail.rs shared/ui_logic/src/transcript/mod.rs
git commit -m "shared-ui-logic: DetailLevel (brief/full) and the per-entry open override"
```

### Task 9: `reducer.rs` — the live leg

**Files:**
- Create: `shared/ui_logic/src/transcript/reducer.rs`
- Modify: `shared/ui_logic/src/transcript/mod.rs`

**Interfaces:**
- Produces: `Transcript::new()`, `entries(&self) -> &[TranscriptEntry]`, `push_user(&mut self, text, at_ms: Option<u64>, attachments: Vec<TuiAttachment>) -> Change`, `apply_live(&mut self, ev: &aleph_protocol::StreamEvent, now_ms: u64) -> Vec<Change>`; `Change { Inserted(String), Updated(String), Removed(String), NeedsResync }`; private `apply_trace(&mut self, ev: &AgentTraceEvent, now_ms: Option<u64>) -> Vec<Change>` shared with Task 10.
- Consumes: Task 7 types; `aleph_protocol::{StreamEvent, AgentTraceEvent, AgentTraceToolResult, RunSummary, ToolResult}`; `aleph_protocol::trace_presentation::{present_agent_trace_event_with_preset, AgentTracePresentationPreset}`.

Semantics ported from the TUI's live path (`interfaces/tui/src/tui/app/{events.rs:352-574, trace.rs:107-420}`), which this replaces in Phase T:

| Frame | Effect |
|---|---|
| `RunAccepted { run_id }` | open a run; nothing rendered |
| `Reasoning { content }` / `ReasoningBlock { content }` | append to the open step's thinking (open an unnumbered step if none); mark `thinking_streamed` |
| `ResponseChunk { content }` | append to the open step's text; `turn_streamed_len += content.len()` |
| `ToolStart` | `start_tool`: an existing row that is `Pending` starts; an existing `Running`/terminal row is left alone (the two faces are idempotent by id); an unknown id creates a `Running` row in the open step |
| `ToolUpdate { progress }` | body = `Text(progress)` while `Running` |
| `ToolEnd { result, duration_ms }` | `finish_tool`; unknown id with no name is a no-op (`RunComplete` reconciles) |
| `AgentTrace { event }` | `apply_trace` (below) |
| `RunComplete { summary, total_duration_ms }` | reconcile rows from `summary.tool_summaries` + `errors` (reconstruct missing rows header-only), settle orphans, fallback text from `summary.final_response` if the run rendered none, **hoist** the open step's text to a top-level `AssistantText`, drop the step if now empty, push `TurnSummary` + `SystemNotice(worked_for · N steps)`, then the `loops` check → `NeedsResync` |
| `RunError { error }` | settle orphans, hoist, `SystemNotice("Error: …")` |
| everything else | `vec![]` (cost, plan, gauge, ask_user, retries stay the client's) |

`apply_trace`:

| Event | Effect |
|---|---|
| `TurnStarted { iteration }` | close the open step (`Settled`, `ended_ms`), open a numbered one; reset `turn_streamed_len`, `thinking_streamed`; `steps_seen += 1` |
| `ReasoningEmitted { text }` | authoritative: replace the open step's thinking text, `streaming = false` (open an unnumbered step if none) |
| `TextEmitted { text, .. }` | append `text[turn_streamed_len..]` (the TUI's de-dup against the streamed deltas) |
| `ToolCallStarted { call }` | `start_tool(call.tool_id, call.tool_name, &call.input)` |
| `ToolCallCompleted { call, result }` | `finish_tool` with the result converted exactly as the TUI's `trace_result_to_wire` (ported verbatim) and `call.presentation` |
| `ToolSummary` · `VerifierVeto` · `ReactiveCompactionAttempted` · `MoaAdvisor` · `MoaAggregating` · `MoaAdvisorSpend` | a `Note` on the open step; text from `present_agent_trace_event_with_preset(ev, TuiDebug).content` — one derivation, shared with the TUI's debug entries |
| `CacheHealthDegraded` | top-level `SystemNotice` with that presentation's content (run-scoped) |
| `SessionCompleted { final_text, .. }` | if the open step has no text and `final_text` is non-empty, append it |
| `TurnStateEntered` · `TurnCompleted` · `Worktree*` · `McpScope*` · `ProviderUsage` · `MoaTurnTrace` | `vec![]` |

- [ ] **Step 1: Write the failing tests**

Create `reducer.rs` with only this test module (production half in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::events::{ToolErrorItem, ToolSummaryItem};
    use aleph_protocol::{
        AgentTraceEvent, AgentTraceTextKind, AgentTraceToolCallEnd, AgentTraceToolCallStart,
        AgentTraceToolResult, RunSummary, StreamEvent, ToolResult,
    };
    use serde_json::json;

    const RUN: &str = "run-1";

    fn accepted() -> StreamEvent {
        StreamEvent::RunAccepted {
            run_id: RUN.into(),
            session_key: "s".into(),
            accepted_at: "t".into(),
        }
    }
    fn reasoning(s: &str) -> StreamEvent {
        StreamEvent::Reasoning {
            run_id: RUN.into(),
            seq: 0,
            content: s.into(),
            is_complete: false,
        }
    }
    fn chunk(s: &str) -> StreamEvent {
        StreamEvent::ResponseChunk {
            run_id: RUN.into(),
            seq: 0,
            content: s.into(),
            chunk_index: 0,
            is_final: false,
            is_intermediate: false,
        }
    }
    fn trace(ev: AgentTraceEvent) -> StreamEvent {
        StreamEvent::AgentTrace {
            run_id: RUN.into(),
            seq: 0,
            event: ev,
        }
    }
    fn turn(i: usize) -> StreamEvent {
        trace(AgentTraceEvent::TurnStarted { iteration: i })
    }
    fn tool_start(id: &str, name: &str) -> StreamEvent {
        StreamEvent::ToolStart {
            run_id: RUN.into(),
            seq: 0,
            tool_name: name.into(),
            tool_id: id.into(),
            params: json!({"command": "ls"}),
        }
    }
    fn tool_end(id: &str, ms: u64) -> StreamEvent {
        StreamEvent::ToolEnd {
            run_id: RUN.into(),
            seq: 0,
            tool_id: id.into(),
            result: ToolResult::success("ok"),
            duration_ms: ms,
        }
    }
    fn complete(loops: u32, summaries: Vec<ToolSummaryItem>) -> StreamEvent {
        StreamEvent::RunComplete {
            run_id: RUN.into(),
            seq: 0,
            summary: RunSummary {
                loops,
                tool_summaries: summaries,
                ..Default::default()
            },
            total_duration_ms: 32_000,
        }
    }
    fn item(id: &str, name: &str, ms: u64, ok: bool) -> ToolSummaryItem {
        ToolSummaryItem {
            tool_id: id.into(),
            tool_name: name.into(),
            emoji: String::new(),
            duration_ms: ms,
            success: ok,
        }
    }
    fn steps(t: &Transcript) -> Vec<&StepEntry> {
        t.entries()
            .iter()
            .filter_map(|e| match e {
                TranscriptEntry::Step(s) => Some(s),
                _ => None,
            })
            .collect()
    }
    fn finals(t: &Transcript) -> Vec<String> {
        t.entries()
            .iter()
            .filter_map(|e| match e {
                TranscriptEntry::AssistantText { markdown, .. } => Some(markdown.clone()),
                _ => None,
            })
            .collect()
    }
    fn drive(t: &mut Transcript, frames: &[StreamEvent]) -> Vec<Change> {
        let mut out = Vec::new();
        for (i, f) in frames.iter().enumerate() {
            out.extend(t.apply_live(f, 1_000 + i as u64));
        }
        out
    }

    #[test]
    fn a_frame_before_any_turn_started_opens_an_unnumbered_step_that_is_never_renumbered() {
        let mut t = Transcript::new();
        drive(&mut t, &[accepted(), reasoning("early"), turn(1), chunk("hi")]);
        let s = steps(&t);
        assert_eq!(s.len(), 2, "{:?}", t.entries());
        assert_eq!(s[0].iteration, None);
        assert_eq!(s[0].thinking.as_ref().map(|b| b.text.as_str()), Some("early"));
        assert_eq!(s[1].iteration, Some(1));
        assert_eq!(s[1].text.as_deref(), Some("hi"));
    }

    #[test]
    fn turn_started_closes_the_previous_step_and_opens_the_next() {
        let mut t = Transcript::new();
        drive(&mut t, &[accepted(), turn(1), chunk("a"), turn(2), chunk("b")]);
        let s = steps(&t);
        assert_eq!(s[0].status, StepStatus::Settled);
        assert!(s[0].ended_ms.is_some());
        assert_eq!(s[1].status, StepStatus::Live);
        assert_eq!((s[0].text.as_deref(), s[1].text.as_deref()), (Some("a"), Some("b")));
    }

    #[test]
    fn streamed_thinking_is_replaced_by_the_authoritative_record() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                reasoning("Weigh"),
                reasoning("ing…"),
                trace(AgentTraceEvent::ReasoningEmitted {
                    iteration: 1,
                    text: "Weighing the two readings.".into(),
                }),
            ],
        );
        let b = steps(&t)[0].thinking.clone().unwrap();
        assert_eq!(b.text, "Weighing the two readings.");
        assert!(!b.streaming);
    }

    #[test]
    fn final_text_from_the_trace_is_deduplicated_against_the_streamed_chunks() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                chunk("Hel"),
                chunk("lo"),
                trace(AgentTraceEvent::TextEmitted {
                    iteration: 1,
                    stream: AgentTraceTextKind::Final,
                    text: "Hello world".into(),
                }),
            ],
        );
        assert_eq!(steps(&t)[0].text.as_deref(), Some("Hello world"));
    }

    #[test]
    fn parallel_tool_calls_in_one_iteration_share_the_step_and_settle_independently() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                tool_start("a", "bash"),
                tool_start("b", "bash"),
                // the trace face repeats the start: must not reset the row
                trace(AgentTraceEvent::ToolCallStarted {
                    iteration: 1,
                    call: AgentTraceToolCallStart {
                        tool_id: "a".into(),
                        tool_name: "bash".into(),
                        input: json!({"command": "ls"}),
                    },
                }),
                tool_end("b", 7),
            ],
        );
        let s = steps(&t);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].tools.len(), 2);
        assert!(matches!(s[0].tools[0].status, RowStatus::Running { .. }));
        assert_eq!(s[0].tools[1].status, RowStatus::Ok { duration_ms: 7 });
    }

    #[test]
    fn a_tool_end_for_an_unknown_id_is_a_no_op_until_the_summary_names_it() {
        let mut t = Transcript::new();
        drive(&mut t, &[accepted(), turn(1), tool_end("ghost", 3)]);
        assert!(steps(&t)[0].tools.is_empty());
        drive(&mut t, &[complete(1, vec![item("ghost", "bash", 3, true)])]);
        let s = steps(&t);
        assert_eq!(s[0].tools.len(), 1, "the authoritative record reconstructs it");
        assert_eq!(s[0].tools[0].status, RowStatus::Ok { duration_ms: 3 });
        assert_eq!(s[0].tools[0].body, RowBody::None, "header-only: it knows the call, not its output");
    }

    #[test]
    fn run_complete_hoists_the_final_text_and_keeps_a_thinking_only_step() {
        // Step 1: thinking only (a veto forced a continue). Step 2: the answer.
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                reasoning("first try"),
                trace(AgentTraceEvent::VerifierVeto {
                    iteration: 1,
                    reason: "- [ ] tests".into(),
                }),
                turn(2),
                reasoning("second"),
                chunk("Done."),
                complete(2, vec![]),
            ],
        );
        let s = steps(&t);
        assert_eq!(s.len(), 2, "{:?}", t.entries());
        assert_eq!(s[0].notes.len(), 1);
        assert_eq!(s[0].notes[0].kind, NoteKind::VerifierVeto);
        assert_eq!(s[1].text, None, "hoisted out of the step");
        assert_eq!(s[1].thinking.as_ref().map(|b| b.text.as_str()), Some("second"));
        assert_eq!(finals(&t), vec!["Done.".to_string()]);
        let hoisted_after_step = t.entries().iter().position(|e| matches!(e, TranscriptEntry::AssistantText { .. }));
        let last_step = t.entries().iter().rposition(|e| matches!(e, TranscriptEntry::Step(_)));
        assert!(hoisted_after_step > last_step, "the answer follows the steps");
    }

    #[test]
    fn run_complete_removes_a_step_that_held_only_the_hoisted_text() {
        let mut t = Transcript::new();
        drive(&mut t, &[accepted(), turn(1), chunk("Just an answer."), complete(1, vec![])]);
        assert!(steps(&t).is_empty(), "{:?}", t.entries());
        assert_eq!(finals(&t), vec!["Just an answer.".to_string()]);
    }

    #[test]
    fn run_complete_drops_a_step_that_held_nothing() {
        // The fourth hoist cell: an iteration that produced no thinking, no
        // text, no tool and no note (e.g. an empty retried response). It is
        // not rendered as "Step N: (nothing)".
        // Step 1 is closed empty by `TurnStarted 2` and dropped there; step 2
        // holds only the answer, which the hoist takes, so it is dropped too.
        let mut t = Transcript::new();
        drive(&mut t, &[accepted(), turn(1), turn(2), chunk("Answer."), complete(2, vec![])]);
        assert!(steps(&t).is_empty(), "{:?}", t.entries());
        assert_eq!(finals(&t), vec!["Answer.".to_string()]);
        assert!(
            t.entries().iter().any(|e| matches!(e, TranscriptEntry::SystemNotice { text, .. } if text.ends_with("· 2 steps"))),
            "the trailer still counts the iterations the loop ran: {:?}",
            t.entries()
        );
    }

    #[test]
    fn run_complete_with_thinking_and_text_keeps_the_thinking_row_beside_the_answer() {
        let mut t = Transcript::new();
        drive(&mut t, &[accepted(), turn(1), reasoning("why"), chunk("Answer."), complete(1, vec![])]);
        let s = steps(&t);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].thinking.as_ref().map(|b| b.text.as_str()), Some("why"));
        assert_eq!(s[0].text, None);
        assert_eq!(finals(&t), vec!["Answer.".to_string()]);
    }

    #[test]
    fn run_complete_with_nothing_rendered_falls_back_to_final_response() {
        let mut t = Transcript::new();
        drive(&mut t, &[accepted(), turn(1)]);
        let ev = StreamEvent::RunComplete {
            run_id: RUN.into(),
            seq: 0,
            summary: RunSummary {
                loops: 1,
                final_response: Some("From the summary.  ".into()),
                ..Default::default()
            },
            total_duration_ms: 10,
        };
        t.apply_live(&ev, 5_000);
        assert_eq!(finals(&t), vec!["From the summary.".to_string()]);
    }

    #[test]
    fn run_complete_produces_the_turn_summary_and_the_worked_for_notice() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                tool_start("a", "bash"),
                tool_end("a", 100),
                turn(2),
                tool_start("b", "file_read"),
                tool_end("b", 50),
                chunk("Done."),
                complete(2, vec![item("a", "bash", 100, true), item("b", "file_read", 50, true)]),
            ],
        );
        let summary = t.entries().iter().find_map(|e| match e {
            TranscriptEntry::TurnSummary(s) => Some(s.clone()),
            _ => None,
        });
        let summary = summary.expect("a turn summary");
        assert_eq!((summary.commands, summary.reads, summary.duration_ms), (1, 1, 150));
        let notice = t.entries().iter().rev().find_map(|e| match e {
            TranscriptEntry::SystemNotice { text, .. } => Some(text.clone()),
            _ => None,
        });
        assert_eq!(notice.as_deref(), Some("✻ Worked for 32s · 2 steps"));
    }

    #[test]
    fn fewer_turn_starts_than_summary_loops_reports_needs_resync_without_touching_entries() {
        let mut t = Transcript::new();
        drive(&mut t, &[accepted(), turn(1), chunk("a")]);
        let before = t.entries().to_vec();
        let changes = t.apply_live(&complete(3, vec![]), 9_000);
        assert!(changes.contains(&Change::NeedsResync), "{changes:?}");
        // The run still completes normally (hoist etc.) — resync is the
        // client's next move, not a reason to leave the run half-open.
        assert_eq!(finals(&t), vec!["a".to_string()]);
        assert!(before.len() <= t.entries().len());
    }

    #[test]
    fn a_run_with_zero_loops_never_asks_for_a_resync() {
        // simple.rs / slash paths: no trace, no iterations counted.
        let mut t = Transcript::new();
        drive(&mut t, &[accepted(), reasoning("x"), chunk("y")]);
        let changes = t.apply_live(&complete(0, vec![]), 9_000);
        assert!(!changes.contains(&Change::NeedsResync));
    }

    #[test]
    fn run_error_settles_rows_hoists_the_partial_answer_and_leaves_a_notice() {
        let mut t = Transcript::new();
        drive(&mut t, &[accepted(), turn(1), tool_start("a", "bash"), chunk("half an ans")]);
        let ev = StreamEvent::RunError {
            run_id: RUN.into(),
            seq: 0,
            error: "provider down".into(),
            error_code: None,
        };
        t.apply_live(&ev, 5_000);
        assert_eq!(steps(&t)[0].tools[0].status, RowStatus::Pending, "never a spinner after the run ended");
        assert_eq!(finals(&t), vec!["half an ans".to_string()]);
        let notice = t.entries().iter().rev().find_map(|e| match e {
            TranscriptEntry::SystemNotice { text, .. } => Some(text.clone()),
            _ => None,
        });
        assert_eq!(notice.as_deref(), Some("Error: provider down"));
    }

    #[test]
    fn a_tool_summary_becomes_a_note_on_the_open_step() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                trace(AgentTraceEvent::ToolSummary {
                    iteration: 1,
                    summary: "Read the failing test.".into(),
                }),
            ],
        );
        let n = &steps(&t)[0].notes[0];
        assert_eq!(n.kind, NoteKind::ToolSummary);
        assert!(n.text.contains("Read the failing test."), "{}", n.text);
    }

    #[test]
    fn cache_health_is_a_run_level_notice_not_a_step_note() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                trace(AgentTraceEvent::CacheHealthDegraded {
                    scope: "main".into(),
                    streak: 3,
                    reads: 0,
                    writes: 900,
                    prefix_changed: Some(true),
                }),
            ],
        );
        assert!(steps(&t)[0].notes.is_empty());
        assert!(t.entries().iter().any(|e| matches!(e, TranscriptEntry::SystemNotice { .. })));
    }

    #[test]
    fn a_frame_for_another_run_is_ignored() {
        let mut t = Transcript::new();
        drive(&mut t, &[accepted(), turn(1)]);
        let foreign = StreamEvent::ResponseChunk {
            run_id: "run-other".into(),
            seq: 0,
            content: "nope".into(),
            chunk_index: 0,
            is_final: false,
            is_intermediate: false,
        };
        assert!(t.apply_live(&foreign, 1).is_empty());
        assert_eq!(steps(&t)[0].text, None);
    }

    #[test]
    fn push_user_appends_a_user_row_with_its_time() {
        let mut t = Transcript::new();
        let c = t.push_user("hi", Some(42), vec![]);
        assert!(matches!(c, Change::Inserted(_)));
        assert!(matches!(
            &t.entries()[0],
            TranscriptEntry::UserText { text, at_ms: Some(42), .. } if text == "hi"
        ));
    }

    #[test]
    fn a_tool_call_completed_with_a_presentation_lands_it_on_the_row() {
        use aleph_protocol::file_change::{FileChange, FileChangeKind, Presentation, Unavailable};
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                trace(AgentTraceEvent::ToolCallStarted {
                    iteration: 1,
                    call: AgentTraceToolCallStart {
                        tool_id: "e".into(),
                        tool_name: "file_edit".into(),
                        input: json!({"file_path": "a.rs"}),
                    },
                }),
                trace(AgentTraceEvent::ToolCallCompleted {
                    iteration: 1,
                    call: AgentTraceToolCallEnd {
                        tool_id: "e".into(),
                        tool_name: "file_edit".into(),
                        input: json!({"file_path": "a.rs"}),
                        duration_ms: 9,
                        presentation: Some(Presentation::FileChanges {
                            changes: vec![FileChange::unavailable(
                                "a.rs",
                                FileChangeKind::Modified,
                                Unavailable::TooLarge,
                            )],
                        }),
                    },
                    result: AgentTraceToolResult::Success { output: json!("ok") },
                }),
            ],
        );
        let row = &steps(&t)[0].tools[0];
        assert!(matches!(row.body, RowBody::FileChanges(ref c) if c.len() == 1));
        assert_eq!(row.status, RowStatus::Ok { duration_ms: 9 });
    }
}
```

- [ ] **Step 2: Run to see them fail**

```bash
cargo test -p shared-ui-logic --no-default-features transcript::reducer 2>&1 | tail -4
```

Expected: unresolved module.

- [ ] **Step 3: Write the production half (above the tests)**

```rust
//! The fold. Live frames (`apply_live`) and replay rows (`apply_replay`,
//! Task 10) become the same `Vec<TranscriptEntry>` through one internal
//! `apply_trace` — that is the "two legs, one derivation" rule of
//! TRANSCRIPT_RENDERING §2, made structural.
//!
//! Pure over its inputs: the wall clock and the run's frames are arguments;
//! the entry-id counter is internal and deterministic. It replaces the
//! TUI's `app/trace.rs` and the Panel's `begin_step` / `set_step_text`
//! family (Phases T and P); the rules below are theirs, ported.
//!
//! What it does NOT decide: cost, context gauge, plan snapshots, halt
//! wording (locale) — the client reads those from the same frame.

use aleph_protocol::trace_presentation::{
    present_agent_trace_event_with_preset, AgentTracePresentationPreset,
};
use aleph_protocol::{
    AgentTraceEvent, AgentTraceToolResult, RunSummary, StreamEvent, ToolResult,
};

use super::affordance::worked_for;
use super::step::{Note, NoteKind, StepEntry, StepStatus, ThinkingBlock};
use super::turn_summary::summarize_turn;
use super::view_model::{RowBody, RowStatus, ToolRow, TranscriptEntry, TuiAttachment};

/// What a fold step changed, by entry id — enough for a keyed re-render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Inserted(String),
    Updated(String),
    Removed(String),
    /// `RunSummary.loops` counted more iterations than this transcript saw
    /// `TurnStarted` frames for: something was dropped on the way. The
    /// entries are NOT patched — the client re-pulls the run through
    /// `trace.by_runs` and replaces them (spec §5.1, §9).
    NeedsResync,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct RunState {
    run_id: String,
    /// Index into `entries` of the `Live` step, if any.
    open_step: Option<usize>,
    /// Bytes of `ResponseChunk` text appended this iteration; the trace's
    /// authoritative `TextEmitted` is de-duplicated against it.
    turn_streamed_len: usize,
    /// Whether `Reasoning` deltas were appended this iteration; the
    /// authoritative `ReasoningEmitted` replaces them.
    thinking_streamed: bool,
    /// Whether any assistant text was rendered this run (gates the
    /// `final_response` fallback at `RunComplete`).
    text_rendered: bool,
    /// `TurnStarted` frames seen — compared with `RunSummary.loops`.
    steps_seen: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Transcript {
    entries: Vec<TranscriptEntry>,
    next_id: u64,
    run: Option<RunState>,
}

impl Transcript {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn entries(&self) -> &[TranscriptEntry] {
        &self.entries
    }

    fn next_id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}-{}", self.next_id)
    }

    pub fn push_user(
        &mut self,
        text: &str,
        at_ms: Option<u64>,
        attachments: Vec<TuiAttachment>,
    ) -> Change {
        let id = self.next_id("user");
        self.entries.push(TranscriptEntry::UserText {
            id: id.clone(),
            text: text.to_string(),
            at_ms,
            attachments,
        });
        Change::Inserted(id)
    }

    // ---- live leg --------------------------------------------------------

    pub fn apply_live(&mut self, ev: &StreamEvent, now_ms: u64) -> Vec<Change> {
        if let StreamEvent::RunAccepted { run_id, .. } = ev {
            self.run = Some(RunState {
                run_id: run_id.clone(),
                ..RunState::default()
            });
            return Vec::new();
        }
        let mine = self
            .run
            .as_ref()
            .is_some_and(|r| r.run_id == ev.run_id());
        if !mine {
            return Vec::new();
        }
        let now = Some(now_ms);
        match ev {
            StreamEvent::Reasoning { content, .. }
            | StreamEvent::ReasoningBlock { content, .. } => self.append_thinking(content, now),
            StreamEvent::ResponseChunk { content, .. } => {
                if let Some(r) = self.run.as_mut() {
                    r.turn_streamed_len += content.len();
                }
                self.append_text(content, now)
            }
            StreamEvent::ToolStart {
                tool_id,
                tool_name,
                params,
                ..
            } => self.start_tool(tool_id, tool_name, params, now),
            StreamEvent::ToolUpdate {
                tool_id, progress, ..
            } => self.update_tool(tool_id, progress),
            StreamEvent::ToolEnd {
                tool_id,
                result,
                duration_ms,
                ..
            } => self.finish_tool(tool_id, None, result, *duration_ms, now),
            StreamEvent::AgentTrace { event, .. } => self.apply_trace(event, now),
            StreamEvent::RunComplete {
                summary,
                total_duration_ms,
                ..
            } => self.complete_run(summary, *total_duration_ms, now),
            StreamEvent::RunError { error, .. } => self.fail_run(error, now),
            StreamEvent::RunAccepted { .. }
            | StreamEvent::RunQueued { .. }
            | StreamEvent::AskUser { .. }
            | StreamEvent::ClarificationEnded { .. }
            | StreamEvent::UncertaintySignal { .. }
            | StreamEvent::RunRetrying { .. }
            | StreamEvent::ModelResolved { .. }
            | StreamEvent::ContextGauge { .. }
            | StreamEvent::SessionUserMessage { .. } => Vec::new(),
        }
    }

    // ---- the shared leg --------------------------------------------------

    /// Both legs end here. `now_ms` is `None` on replay: rows then carry no
    /// clocks, only the durations the wire recorded.
    fn apply_trace(&mut self, ev: &AgentTraceEvent, now_ms: Option<u64>) -> Vec<Change> {
        match ev {
            AgentTraceEvent::TurnStarted { iteration } => {
                let closing = self.run.as_ref().and_then(|r| r.open_step);
                let mut changes = self.close_open_step(StepStatus::Settled, now_ms);
                if let Some(idx) = closing {
                    // An iteration that produced nothing is not a step
                    // (spec §9) — dropped here, where it is provably over.
                    changes.extend(self.drop_step_if_empty(idx));
                }
                changes.push(self.open_step(Some(*iteration as u32), now_ms));
                if let Some(r) = self.run.as_mut() {
                    r.turn_streamed_len = 0;
                    r.thinking_streamed = false;
                    r.steps_seen += 1;
                }
                changes
            }
            AgentTraceEvent::ReasoningEmitted { text, .. } => {
                let idx = self.ensure_open_step(now_ms);
                let TranscriptEntry::Step(step) = &mut self.entries[idx] else {
                    return Vec::new();
                };
                step.thinking = Some(ThinkingBlock {
                    text: text.clone(),
                    streaming: false,
                });
                if let Some(r) = self.run.as_mut() {
                    r.thinking_streamed = false;
                }
                vec![Change::Updated(step.id.clone())]
            }
            AgentTraceEvent::TextEmitted { text, .. } => {
                let streamed = self.run.as_ref().map_or(0, |r| r.turn_streamed_len);
                let fresh = text.get(streamed..).unwrap_or("");
                self.append_text(fresh, now_ms)
            }
            AgentTraceEvent::ToolCallStarted { call, .. } => {
                self.start_tool(&call.tool_id, &call.tool_name, &call.input, now_ms)
            }
            AgentTraceEvent::ToolCallCompleted { call, result, .. } => {
                let wire = trace_result_to_wire(result, call.presentation.as_ref());
                self.finish_tool(
                    &call.tool_id,
                    Some(&call.tool_name),
                    &wire,
                    call.duration_ms,
                    now_ms,
                )
            }
            AgentTraceEvent::ToolSummary { .. } => self.note(ev, NoteKind::ToolSummary, now_ms),
            AgentTraceEvent::VerifierVeto { .. } => self.note(ev, NoteKind::VerifierVeto, now_ms),
            AgentTraceEvent::ReactiveCompactionAttempted { .. } => {
                self.note(ev, NoteKind::ReactiveCompaction, now_ms)
            }
            AgentTraceEvent::MoaAdvisor { .. } => self.note(ev, NoteKind::MoaAdvisor, now_ms),
            AgentTraceEvent::MoaAggregating { .. } => {
                self.note(ev, NoteKind::MoaAggregating, now_ms)
            }
            AgentTraceEvent::MoaAdvisorSpend { .. } => {
                self.note(ev, NoteKind::MoaAdvisorSpend, now_ms)
            }
            AgentTraceEvent::CacheHealthDegraded { .. } => {
                match presentation_text(ev) {
                    Some(text) => vec![self.push_notice(text)],
                    None => Vec::new(),
                }
            }
            AgentTraceEvent::SessionCompleted { final_text, .. } => {
                let has_text = self.open_step_ref().is_some_and(|s| {
                    s.text.as_deref().is_some_and(|t| !t.trim().is_empty())
                });
                match final_text.as_deref().filter(|t| !t.trim().is_empty()) {
                    Some(t) if !has_text => self.append_text(t, now_ms),
                    _ => Vec::new(),
                }
            }
            AgentTraceEvent::TurnStateEntered { .. }
            | AgentTraceEvent::TurnCompleted { .. }
            | AgentTraceEvent::WorktreeCreated { .. }
            | AgentTraceEvent::WorktreeCleanedUp { .. }
            | AgentTraceEvent::McpScopeAttached { .. }
            | AgentTraceEvent::McpScopeCleaned { .. }
            | AgentTraceEvent::ProviderUsage { .. }
            | AgentTraceEvent::MoaTurnTrace { .. } => Vec::new(),
        }
    }

    // ---- step bookkeeping ------------------------------------------------

    fn open_step_ref(&self) -> Option<&StepEntry> {
        let idx = self.run.as_ref()?.open_step?;
        match self.entries.get(idx) {
            Some(TranscriptEntry::Step(s)) => Some(s),
            _ => None,
        }
    }

    fn open_step(&mut self, iteration: Option<u32>, now_ms: Option<u64>) -> Change {
        let id = self.next_id("step");
        self.entries.push(TranscriptEntry::Step(StepEntry {
            id: id.clone(),
            iteration,
            thinking: None,
            text: None,
            tools: Vec::new(),
            notes: Vec::new(),
            status: StepStatus::Live,
            started_ms: now_ms,
            ended_ms: None,
        }));
        let idx = self.entries.len() - 1;
        match self.run.as_mut() {
            Some(r) => r.open_step = Some(idx),
            None => {
                // A frame with no `RunAccepted` before it (replay, or a
                // client attaching mid-run): fold it anyway, unattributed.
                self.run = Some(RunState {
                    open_step: Some(idx),
                    ..RunState::default()
                });
            }
        }
        Change::Inserted(id)
    }

    /// The open step's index, opening an UNNUMBERED one if there is none —
    /// a frame that arrives before any `TurnStarted` is real and must land
    /// somewhere, but never on a step number the server did not say.
    fn ensure_open_step(&mut self, now_ms: Option<u64>) -> usize {
        if let Some(idx) = self.run.as_ref().and_then(|r| r.open_step) {
            if matches!(self.entries.get(idx), Some(TranscriptEntry::Step(_))) {
                return idx;
            }
        }
        self.open_step(None, now_ms);
        self.run.as_ref().and_then(|r| r.open_step).unwrap_or(0)
    }

    fn close_open_step(&mut self, status: StepStatus, now_ms: Option<u64>) -> Vec<Change> {
        let Some(idx) = self.run.as_ref().and_then(|r| r.open_step) else {
            return Vec::new();
        };
        let mut changes = Vec::new();
        if let Some(TranscriptEntry::Step(step)) = self.entries.get_mut(idx) {
            step.status = status;
            step.ended_ms = now_ms;
            if let Some(t) = step.thinking.as_mut() {
                t.streaming = false;
            }
            changes.push(Change::Updated(step.id.clone()));
        }
        if let Some(r) = self.run.as_mut() {
            r.open_step = None;
        }
        changes
    }

    /// Remove the step at `idx` when it holds nothing. Only called on a step
    /// that is already closed, so no index in `RunState` points at it.
    fn drop_step_if_empty(&mut self, idx: usize) -> Vec<Change> {
        match self.entries.get(idx) {
            Some(TranscriptEntry::Step(s)) if s.is_empty() => {
                let id = s.id.clone();
                self.entries.remove(idx);
                vec![Change::Removed(id)]
            }
            _ => Vec::new(),
        }
    }

    fn append_thinking(&mut self, s: &str, now_ms: Option<u64>) -> Vec<Change> {
        if s.is_empty() {
            return Vec::new();
        }
        let idx = self.ensure_open_step(now_ms);
        let TranscriptEntry::Step(step) = &mut self.entries[idx] else {
            return Vec::new();
        };
        match step.thinking.as_mut() {
            Some(t) => t.text.push_str(s),
            None => {
                step.thinking = Some(ThinkingBlock {
                    text: s.to_string(),
                    streaming: true,
                });
            }
        }
        if let Some(r) = self.run.as_mut() {
            r.thinking_streamed = true;
        }
        vec![Change::Updated(step.id.clone())]
    }

    fn append_text(&mut self, s: &str, now_ms: Option<u64>) -> Vec<Change> {
        if s.is_empty() {
            return Vec::new();
        }
        let idx = self.ensure_open_step(now_ms);
        let TranscriptEntry::Step(step) = &mut self.entries[idx] else {
            return Vec::new();
        };
        step.text.get_or_insert_with(String::new).push_str(s);
        if let Some(r) = self.run.as_mut() {
            r.text_rendered = true;
        }
        vec![Change::Updated(step.id.clone())]
    }

    fn note(&mut self, ev: &AgentTraceEvent, kind: NoteKind, now_ms: Option<u64>) -> Vec<Change> {
        let Some(text) = presentation_text(ev) else {
            return Vec::new();
        };
        let idx = self.ensure_open_step(now_ms);
        let TranscriptEntry::Step(step) = &mut self.entries[idx] else {
            return Vec::new();
        };
        step.notes.push(Note { kind, text });
        vec![Change::Updated(step.id.clone())]
    }

    fn push_notice(&mut self, text: String) -> Change {
        let id = self.next_id("notice");
        self.entries.push(TranscriptEntry::SystemNotice {
            id: id.clone(),
            text,
        });
        Change::Inserted(id)
    }

    // ---- tool rows (flat scan over every step: ids are unique per run) ---

    fn find_tool(&mut self, tool_id: &str) -> Option<(String, &mut ToolRow)> {
        self.entries.iter_mut().rev().find_map(|e| match e {
            TranscriptEntry::Step(s) => {
                let id = s.id.clone();
                s.find_tool_mut(tool_id).map(|r| (id, r))
            }
            _ => None,
        })
    }

    fn start_tool(
        &mut self,
        tool_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
        now_ms: Option<u64>,
    ) -> Vec<Change> {
        if let Some((step_id, row)) = self.find_tool(tool_id) {
            // The two faces (`tool_start` and `agent_trace.tool_call_started`)
            // both announce the same call; the second sighting must not reset
            // a running or finished row.
            if row.status == RowStatus::Pending {
                row.start(now_ms.unwrap_or(0));
                return vec![Change::Updated(step_id)];
            }
            return Vec::new();
        }
        let idx = self.ensure_open_step(now_ms);
        let TranscriptEntry::Step(step) = &mut self.entries[idx] else {
            return Vec::new();
        };
        let mut row = ToolRow::new(tool_id, tool_name, args);
        if let Some(now) = now_ms {
            row.start(now);
        } else {
            row.status = RowStatus::Running { since_ms: 0 };
        }
        step.tools.push(row);
        vec![Change::Updated(step.id.clone())]
    }

    fn update_tool(&mut self, tool_id: &str, progress: &str) -> Vec<Change> {
        match self.find_tool(tool_id) {
            Some((step_id, row)) if matches!(row.status, RowStatus::Running { .. }) => {
                row.body = RowBody::Text(progress.to_string());
                vec![Change::Updated(step_id)]
            }
            _ => Vec::new(),
        }
    }

    fn finish_tool(
        &mut self,
        tool_id: &str,
        tool_name: Option<&str>,
        result: &ToolResult,
        duration_ms: u64,
        now_ms: Option<u64>,
    ) -> Vec<Change> {
        if let Some((step_id, row)) = self.find_tool(tool_id) {
            let started = row.started_ms;
            row.finish(result, duration_ms, now_ms.unwrap_or(0));
            if now_ms.is_none() {
                // Replay: no clocks. `RowStatus` still carries the duration.
                row.started_ms = started;
                row.ended_ms = None;
            }
            return vec![Change::Updated(step_id)];
        }
        // A result whose start was dropped: the trace face names the tool
        // and can reconstruct the row; the `tool_end` face cannot and leaves
        // it to `RunComplete`'s authoritative list.
        let Some(name) = tool_name else {
            return Vec::new();
        };
        let idx = self.ensure_open_step(now_ms);
        let TranscriptEntry::Step(step) = &mut self.entries[idx] else {
            return Vec::new();
        };
        let mut row = ToolRow::new(tool_id, name, &serde_json::Value::Null);
        row.finish(result, duration_ms, now_ms.unwrap_or(0));
        if now_ms.is_none() {
            row.ended_ms = None;
        }
        step.tools.push(row);
        vec![Change::Updated(step.id.clone())]
    }

    // ---- run end ---------------------------------------------------------

    fn complete_run(&mut self, summary: &RunSummary, total_ms: u64, now_ms: Option<u64>) -> Vec<Change> {
        let mut changes = Vec::new();
        // 1. The authoritative terminal record fills what the stream dropped.
        for item in &summary.tool_summaries {
            let error = summary
                .errors
                .iter()
                .find(|e| e.tool_id == item.tool_id)
                .map(|e| e.error.clone());
            let wire = ToolResult {
                success: item.success,
                output: None,
                error,
                presentation: None,
            };
            // `map` ends the mutable borrow before the `None` arm needs
            // `self` again (an `if let … else` here is E0499).
            let existing = self.find_tool(&item.tool_id).map(|(step_id, row)| {
                // Keep whatever body the live stream delivered; the record
                // carries none.
                let body = row.body.clone();
                row.finish(&wire, item.duration_ms, now_ms.unwrap_or(0));
                if matches!(row.body, RowBody::None) {
                    row.body = body;
                }
                step_id
            });
            match existing {
                Some(step_id) => changes.push(Change::Updated(step_id)),
                None => changes.extend(self.finish_tool(
                    &item.tool_id,
                    Some(&item.tool_name),
                    &wire,
                    item.duration_ms,
                    now_ms,
                )),
            }
        }
        // 2. Nothing spins after the run ended.
        changes.extend(self.settle_orphans());
        // 3. A run that streamed no text still has an answer in the record.
        let rendered = self.run.as_ref().is_some_and(|r| r.text_rendered);
        if !rendered {
            if let Some(t) = summary.final_response.as_deref() {
                changes.extend(self.append_text(t.trim_end(), now_ms));
            }
        }
        // 4. The last step's text IS the answer: hoist it out (ruling R5).
        changes.extend(self.hoist_open_step_text(now_ms));
        // 5. The trailers.
        let ids: Vec<&str> = summary.tool_summaries.iter().map(|i| i.tool_id.as_str()).collect();
        let rows: Vec<ToolRow> = self
            .entries
            .iter()
            .filter_map(|e| match e {
                TranscriptEntry::Step(s) => Some(s.tools.iter()),
                _ => None,
            })
            .flatten()
            .filter(|r| ids.contains(&r.id.as_str()))
            .cloned()
            .collect();
        if let Some(entry) = summarize_turn(&rows) {
            self.entries.push(TranscriptEntry::TurnSummary(entry));
            changes.push(Change::Inserted("turn-summary".into()));
        }
        let steps_seen = self.run.as_ref().map_or(0, |r| r.steps_seen);
        changes.push(self.push_notice(format!("{} · {} steps", worked_for(total_ms), steps_seen)));
        // 6. Effect reached? Fewer turn boundaries than the loop counted
        //    means frames were lost — say so, patch nothing.
        if summary.loops > 0 && steps_seen < summary.loops {
            changes.push(Change::NeedsResync);
        }
        self.run = None;
        changes
    }

    fn fail_run(&mut self, error: &str, now_ms: Option<u64>) -> Vec<Change> {
        let mut changes = self.settle_orphans();
        changes.extend(self.hoist_open_step_text(now_ms));
        changes.push(self.push_notice(format!("Error: {error}")));
        self.run = None;
        changes
    }

    fn settle_orphans(&mut self) -> Vec<Change> {
        let mut changes = Vec::new();
        for e in &mut self.entries {
            if let TranscriptEntry::Step(s) = e {
                let mut touched = false;
                for r in &mut s.tools {
                    if matches!(r.status, RowStatus::Running { .. }) {
                        r.settle_resumed();
                        touched = true;
                    }
                }
                if touched {
                    changes.push(Change::Updated(s.id.clone()));
                }
            }
        }
        changes
    }

    /// Close the open step, move its text out as the run's `AssistantText`,
    /// and remove the step if that was all it held.
    fn hoist_open_step_text(&mut self, now_ms: Option<u64>) -> Vec<Change> {
        let Some(idx) = self.run.as_ref().and_then(|r| r.open_step) else {
            return Vec::new();
        };
        let mut changes = self.close_open_step(StepStatus::Settled, now_ms);
        let (text, step_id, empty) = match self.entries.get_mut(idx) {
            Some(TranscriptEntry::Step(s)) => {
                let text = s.text.take().filter(|t| !t.trim().is_empty());
                (text, s.id.clone(), s.is_empty())
            }
            _ => return changes,
        };
        if empty {
            self.entries.remove(idx);
            changes.push(Change::Removed(step_id));
        }
        if let Some(markdown) = text {
            let id = self.next_id("assistant");
            self.entries.push(TranscriptEntry::AssistantText {
                id: id.clone(),
                markdown,
                streaming: false,
            });
            changes.push(Change::Inserted(id));
        }
        changes
    }
}

/// The trace's result shape → the wire `ToolResult` a row settles from.
/// Ported verbatim from the TUI (`app/trace.rs::trace_result_to_wire`).
fn trace_result_to_wire(
    result: &AgentTraceToolResult,
    presentation: Option<&aleph_protocol::file_change::Presentation>,
) -> ToolResult {
    let (success, output, error) = match result {
        AgentTraceToolResult::Success { output } => (
            true,
            match output {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Null => None,
                other => Some(other.to_string()),
            },
            None,
        ),
        AgentTraceToolResult::Error { error, .. } => (false, None, Some(error.clone())),
    };
    ToolResult {
        success,
        output,
        error,
        presentation: presentation.cloned(),
    }
}

/// One derivation of the loop's narration text, shared with the TUI's
/// debug entries: the protocol's own presenter.
fn presentation_text(ev: &AgentTraceEvent) -> Option<String> {
    present_agent_trace_event_with_preset(ev, AgentTracePresentationPreset::TuiDebug)
        .map(|p| p.content)
}
```

`mod.rs`: `mod reducer; pub use reducer::{Change, Transcript};`

Two things the implementer must verify while wiring, not assume: (a) `Option::is_some_and` / `is_none_or` are available at MSRV 1.95 (they are: 1.70 / 1.82); (b) `TuiAttachment` and `RowBody` re-exports already exist in `mod.rs` (they do — `view_model` export block).

- [ ] **Step 4: Run the reducer tests**

```bash
cargo test -p shared-ui-logic --no-default-features transcript::reducer 2>&1 | tail -30
```

Expected: all green. If `run_complete_removes_a_step_that_held_only_the_hoisted_text` fails because `close_open_step` cleared `open_step` before the hoist read it: `hoist_open_step_text` captures `idx` BEFORE calling `close_open_step` (as written) — check the order, do not weaken the test.

- [ ] **Step 5: Mutation check, by name**

Temporarily change `if summary.loops > 0 && steps_seen < summary.loops` to `if false` and run the reducer tests: exactly `fewer_turn_starts_than_summary_loops_reports_needs_resync_without_touching_entries` must go red. Revert. Temporarily make `start_tool` reset an existing running row (`*row = ToolRow::new(..)`): exactly `parallel_tool_calls_in_one_iteration_share_the_step_and_settle_independently` must go red. Revert. Apply the mutations with the editor (heredoc python is a no-op on this host) and confirm each with `git diff` before running.

- [ ] **Step 6: Commit**

```bash
git add shared/ui_logic/src/transcript/reducer.rs shared/ui_logic/src/transcript/mod.rs
git commit -m "shared-ui-logic: the Transcript reducer — live leg

One pure fold from StreamEvent to Vec<TranscriptEntry>: steps open on
turn_started, thinking and text append to the open step, both tool
faces settle rows by id without resetting each other, run_complete
reconciles from the authoritative record, settles orphans, hoists the
final answer out of the last step (R5) and reports NeedsResync when
RunSummary.loops counted more iterations than turn_started frames
arrived. Frames before any turn_started fold into an unnumbered step
that is never renumbered. Ported from the TUI's app/trace.rs, which
Phase T deletes."
```

### Task 10: `reducer.rs` — the replay leg and G2

**Files:**
- Modify: `shared/ui_logic/src/transcript/reducer.rs`

**Interfaces:**
- Produces: `Transcript::apply_replay(&mut self, ev: &AgentTraceEvent) -> Vec<Change>`; `Transcript::finish_replay(&mut self) -> Vec<Change>`.
- Consumes: Task 9's `apply_trace`; the rows `trace.by_runs` returns are `AgentTraceEvent`s (Task 6 proved `ReasoningEmitted` is among them).

Replay rules: every row goes through `apply_trace(ev, None)` (no clocks). `finish_replay` closes the open step as `Pending` when no `SessionCompleted` arrived (an unfinished run is unknown, not settled), settles every `Running` row to `Pending`, hoists the last step's text as the answer, pushes the turn summary over ALL rows (there is no `tool_summaries` list on this leg — the rows themselves are the record) and the `worked_for` notice when `SessionCompleted.duration_ms` was present. Live and replay must produce the same entries for the same run modulo clocks — that is G2, and it is what makes "refresh rebuilds the same folds" (ruling R4) a tested claim rather than a hope.

- [ ] **Step 1: Write the failing tests**

Append to `reducer.rs`'s `mod tests`:

```rust
    /// A replay row is the same `AgentTraceEvent` a live `agent_trace`
    /// frame carries. The two legs share `apply_trace`; this fixture checks
    /// the parts that DIFFER around it: no `RunAccepted`, no deltas, no
    /// `RunComplete`, and `finish_replay` doing what `complete_run` does.
    fn full_run_trace() -> Vec<AgentTraceEvent> {
        vec![
            AgentTraceEvent::TurnStarted { iteration: 1 },
            AgentTraceEvent::ReasoningEmitted {
                iteration: 1,
                text: "Look at the failing test first.".into(),
            },
            AgentTraceEvent::ToolCallStarted {
                iteration: 1,
                call: AgentTraceToolCallStart {
                    tool_id: "r1".into(),
                    tool_name: "file_read".into(),
                    input: json!({"path": "tests/a.rs"}),
                },
            },
            AgentTraceEvent::ToolCallCompleted {
                iteration: 1,
                call: AgentTraceToolCallEnd {
                    tool_id: "r1".into(),
                    tool_name: "file_read".into(),
                    input: json!({"path": "tests/a.rs"}),
                    duration_ms: 12,
                    presentation: None,
                },
                result: AgentTraceToolResult::Success { output: json!("fn a() {}") },
            },
            AgentTraceEvent::TurnStarted { iteration: 2 },
            AgentTraceEvent::ReasoningEmitted {
                iteration: 2,
                text: "The timezone is the bug.".into(),
            },
            AgentTraceEvent::TextEmitted {
                iteration: 2,
                stream: AgentTraceTextKind::Final,
                text: "Fixed the timezone handling.".into(),
            },
            AgentTraceEvent::SessionCompleted {
                outcome: aleph_protocol::AgentTraceSessionOutcome::Completed,
                iterations: 2,
                tool_calls_made: 1,
                total_tokens: 100,
                hit_limit: false,
                final_text: Some("Fixed the timezone handling.".into()),
                terminate_reason: None,
                duration_ms: Some(32_000),
                token_breakdown: None,
                tool_timeline: Vec::new(),
            },
        ]
    }

    /// The live leg for the same run: the same trace frames interleaved
    /// with the deltas and the lifecycle frames a client actually receives.
    fn full_run_live() -> Vec<StreamEvent> {
        let mut out = vec![accepted()];
        for ev in full_run_trace() {
            match &ev {
                AgentTraceEvent::ReasoningEmitted { text, .. } => {
                    // deltas first, then the authoritative record
                    let (a, b) = text.split_at(text.len() / 2);
                    out.push(reasoning(a));
                    out.push(reasoning(b));
                    out.push(trace(ev));
                }
                AgentTraceEvent::TextEmitted { text, .. } => {
                    let (a, b) = text.split_at(text.len() / 2);
                    out.push(chunk(a));
                    out.push(chunk(b));
                    out.push(trace(ev));
                }
                AgentTraceEvent::ToolCallStarted { call, .. } => {
                    out.push(StreamEvent::ToolStart {
                        run_id: RUN.into(),
                        seq: 0,
                        tool_name: call.tool_name.clone(),
                        tool_id: call.tool_id.clone(),
                        params: call.input.clone(),
                    });
                    out.push(trace(ev));
                }
                AgentTraceEvent::ToolCallCompleted { call, .. } => {
                    out.push(StreamEvent::ToolEnd {
                        run_id: RUN.into(),
                        seq: 0,
                        tool_id: call.tool_id.clone(),
                        result: ToolResult::success("fn a() {}"),
                        duration_ms: call.duration_ms,
                    });
                    out.push(trace(ev));
                }
                _ => out.push(trace(ev)),
            }
        }
        out.push(StreamEvent::RunComplete {
            run_id: RUN.into(),
            seq: 0,
            summary: RunSummary {
                loops: 2,
                tool_summaries: vec![item("r1", "file_read", 12, true)],
                final_response: Some("Fixed the timezone handling.".into()),
                ..Default::default()
            },
            total_duration_ms: 32_000,
        });
        out
    }

    /// Clocks are the one thing the replay leg cannot know.
    fn strip_clocks(entries: &[TranscriptEntry]) -> Vec<TranscriptEntry> {
        entries
            .iter()
            .cloned()
            .map(|e| match e {
                TranscriptEntry::Step(mut s) => {
                    s.started_ms = None;
                    s.ended_ms = None;
                    for r in &mut s.tools {
                        r.started_ms = None;
                        r.ended_ms = None;
                        if let RowStatus::Running { since_ms } = &mut r.status {
                            *since_ms = 0;
                        }
                    }
                    TranscriptEntry::Step(s)
                }
                other => other,
            })
            .collect()
    }

    /// G2. Two legs, one fold.
    #[test]
    fn the_live_leg_and_the_replay_leg_fold_to_the_same_entries() {
        let mut live = Transcript::new();
        drive(&mut live, &full_run_live());

        let mut replay = Transcript::new();
        for ev in full_run_trace() {
            replay.apply_replay(&ev);
        }
        replay.finish_replay();

        let (l, r) = (strip_clocks(live.entries()), strip_clocks(replay.entries()));
        assert_eq!(l, r, "\nLIVE:   {l:#?}\nREPLAY: {r:#?}");

        // And the shape is the one the surfaces will paint:
        let s = steps(&replay);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].iteration, Some(1));
        assert_eq!(s[0].tools[0].status, RowStatus::Ok { duration_ms: 12 });
        assert_eq!(s[1].thinking.as_ref().map(|b| b.text.as_str()), Some("The timezone is the bug."));
        assert_eq!(finals(&replay), vec!["Fixed the timezone handling.".to_string()]);
        assert_eq!(
            step_headline(s[0], 80).map(|h| h.text),
            Some("Look at the failing test first.".to_string())
        );
    }

    #[test]
    fn a_replayed_run_without_a_session_completed_row_is_pending_not_settled() {
        let mut t = Transcript::new();
        let rows = full_run_trace();
        // The first four rows: TurnStarted 1, ReasoningEmitted 1,
        // ToolCallStarted r1, ToolCallCompleted r1 — the log stops there.
        for ev in &rows[..4] {
            t.apply_replay(ev);
        }
        t.apply_replay(&AgentTraceEvent::TurnStarted { iteration: 2 });
        t.apply_replay(&AgentTraceEvent::ToolCallStarted {
            iteration: 2,
            call: AgentTraceToolCallStart {
                tool_id: "b".into(),
                tool_name: "bash".into(),
                input: json!({"command": "cargo test"}),
            },
        });
        t.finish_replay();
        let s = steps(&t);
        assert_eq!(s[0].status, StepStatus::Settled, "a step the next turn closed is settled");
        assert_eq!(s[1].status, StepStatus::Pending, "the last step never ended: unknown");
        assert_eq!(s[1].tools[0].status, RowStatus::Pending, "never a spinner from a log");
        assert!(finals(&t).is_empty(), "no answer was recorded, none is invented");
    }

    #[test]
    fn replay_rows_carry_no_clocks_but_the_recorded_durations() {
        let mut t = Transcript::new();
        for ev in full_run_trace() {
            t.apply_replay(&ev);
        }
        t.finish_replay();
        let row = &steps(&t)[0].tools[0];
        assert_eq!((row.started_ms, row.ended_ms), (None, None));
        let summary = t.entries().iter().find_map(|e| match e {
            TranscriptEntry::TurnSummary(s) => Some(s.duration_ms),
            _ => None,
        });
        assert_eq!(summary, None, "one tool is below the turn-summary gate");
        assert_eq!(step_tally(steps(&t)[0]).map(|s| s.duration_ms), Some(12));
    }
```

`step_headline` and `step_tally` come from `use super::super::step::{step_headline, step_tally};` (or the crate re-exports) inside the test module.

- [ ] **Step 2: Run to see them fail**

```bash
cargo test -p shared-ui-logic --no-default-features transcript::reducer::tests::the_live_leg 2>&1 | tail -6
```

Expected: `no method named apply_replay`.

- [ ] **Step 3: Add the replay leg**

In `impl Transcript`, after `apply_live`:

```rust
    // ---- replay leg ------------------------------------------------------

    /// One `trace.by_runs` row. The same `apply_trace` the live leg uses;
    /// the only difference is the absent clock.
    pub fn apply_replay(&mut self, ev: &AgentTraceEvent) -> Vec<Change> {
        if self.run.is_none() {
            self.run = Some(RunState::default());
        }
        if let AgentTraceEvent::SessionCompleted { duration_ms, .. } = ev {
            // Remember the wall clock for the trailer; `complete_replay`
            // reads it back after the last row.
            self.replay_duration_ms = *duration_ms;
            self.replay_completed = true;
        }
        self.apply_trace(ev, None)
    }

    /// After the last row: what `complete_run` does for a live run, with
    /// what a log can honestly say. No `tool_summaries` exist on this leg —
    /// the rows ARE the record — and a run whose log has no
    /// `SessionCompleted` stays `Pending`.
    pub fn finish_replay(&mut self) -> Vec<Change> {
        let completed = self.replay_completed;
        let mut changes = Vec::new();
        for e in &mut self.entries {
            if let TranscriptEntry::Step(s) = e {
                for r in &mut s.tools {
                    r.settle_resumed();
                }
            }
        }
        if completed {
            changes.extend(self.hoist_open_step_text(None));
            let rows: Vec<ToolRow> = self
                .entries
                .iter()
                .filter_map(|e| match e {
                    TranscriptEntry::Step(s) => Some(s.tools.iter()),
                    _ => None,
                })
                .flatten()
                .cloned()
                .collect();
            if let Some(entry) = summarize_turn(&rows) {
                self.entries.push(TranscriptEntry::TurnSummary(entry));
                changes.push(Change::Inserted("turn-summary".into()));
            }
            if let Some(ms) = self.replay_duration_ms {
                let steps_seen = self.run.as_ref().map_or(0, |r| r.steps_seen);
                changes.push(self.push_notice(format!("{} · {} steps", worked_for(ms), steps_seen)));
            }
        } else {
            changes.extend(self.close_open_step(StepStatus::Pending, None));
        }
        self.run = None;
        self.replay_completed = false;
        self.replay_duration_ms = None;
        changes
    }
```

Add the two fields to `Transcript` (both `Default`):

```rust
    /// Replay leg only: set by the `SessionCompleted` row, read by
    /// `finish_replay`.
    replay_completed: bool,
    replay_duration_ms: Option<u64>,
```

- [ ] **Step 4: Run G2 and read the diff it prints on failure**

```bash
cargo test -p shared-ui-logic --no-default-features transcript::reducer 2>&1 | tail -40
```

Expected: green. If G2 prints a difference, it is one of these, in order of likelihood — fix the REDUCER (or the fixture where the fixture is the one lying), never the assertion:
- entry ids differ because one leg minted an extra id (e.g. the live leg's `RunAccepted` does not mint; a stray `open_step` does) — count `next_id` calls per leg;
- the live leg's `TurnSummary` used the `tool_summaries` ids while the replay used all rows — for this fixture both sets are `{r1}`, so a difference here means the live filter dropped a row;
- `worked_for` text differs — both legs must print `✻ Worked for 32s · 2 steps`;
- a `ThinkingBlock.streaming` left `true` on the live leg — `close_open_step` clears it, so the open step at `RunComplete` must go through `hoist_open_step_text → close_open_step`.

- [ ] **Step 5: Commit**

```bash
git add shared/ui_logic/src/transcript/reducer.rs
git commit -m "shared-ui-logic: the Transcript reducer — replay leg + G2 (two legs, one fold)

apply_replay feeds trace.by_runs rows through the same apply_trace the
live leg uses; finish_replay settles, hoists and adds the trailers a
log can honestly support (Pending when no SessionCompleted row). G2
drives one run through both legs and asserts the entries are equal
modulo clocks — the tested form of ruling R4 (refresh rebuilds the same
folds)."
```

---

## Part 4 — Documentation and verification

### Task 11: Documentation — the dated zero-consumer interval, and the dangling §6.13

**Files:**
- Modify: `docs/reference/TRANSCRIPT_RENDERING.md` (new §7; §5.1 note; §0 table row)
- Modify: `docs/reference/FEATURE_LOCATOR.md` (create `### 6.13`; new appendix-D entries)
- Modify: `docs/reference/SESSION_KNOBS.md` (one line)
- Modify: `docs/superpowers/specs/2026-09-23-stepwise-transcript-folding-design.md` (Status line)

Measured while planning (2026-09-23): `FEATURE_LOCATOR.md` has **eleven** references to `§6.13` (CLAUDE.md's routing table, TRANSCRIPT_RENDERING and eight appendix-D entries all point at it) and **no `### 6.13` heading** — `grep -n -E "^#{2,4} .*6\.13" docs/reference/FEATURE_LOCATOR.md` is empty; the `## 6.` section ends at `### 6.12` (line 4425) before `## 7.` (line 4468). The section named everywhere was never written, or was lost in a merge. This task creates it rather than moving eleven pointers.

- [ ] **Step 1: `TRANSCRIPT_RENDERING.md` — §7 and the two touch-ups**

Append before `## 6. 改这一层之前 / Working notes` a new section (Chinese body, English summary, as the file's other sections):

```markdown
## 7. 按迭代折叠：Step · reducer · 单管道 · `ReasoningEmitted`（Phase S，2026-09-23）

> English summary: Phase S of the stepwise-folding design (spec 2026-09-23). Server: the
> agent-trace mirror publishes on the run's own broadcast channel (`FlowStreamEvent::Trace`),
> so `agent_trace` frames take their `seq` from the same serial drain as text and tool frames;
> the harness emits `ReasoningEmitted { iteration, text }` beside `TextEmitted{Final}`, which
> `task_traces` persists and `trace.by_runs` replays unchanged. Shared core: `TranscriptEntry::Step`
> (one Think→Act iteration), `step_headline` (thinking → text → tally), `DetailLevel`, and the
> pure `Transcript` reducer whose live and replay legs meet in one `apply_trace`.
> **Nothing renders it yet** (dated debt below).

### 7.1 交付了什么

| 交付 | 是什么 | 落点 |
|---|---|---|
| **一条管道** | `AgentTraceEmitSink` 不再自起排水任务；`on_trace` 同步 `send` `FlowStreamEvent::Trace` 进 run 自己的 broadcast 通道，drain 原地取 `seq`。通道由 `run_loop/inner.rs` 用 `orchestrator::flow_event_channel()` 创建，经 `FlowRequest.event_tx` 交给 `dispatch`（有则 `subscribe`，无则自建） | `src/gateway/execution_engine/agent_trace_emit_sink.rs` · `src/orchestrator/dispatch.rs` · `src/gateway/execution_engine/run_loop/inner.rs` |
| **`ReasoningEmitted`** | `LoopTraceEvent` / `AgentTraceEvent` 各加一个变体（`kind = "reasoning_emitted"`）；`think.rs` 在 `TextEmitted{Final}` 的两个生产点旁各发一次，只在 thinking 非空时发；无人值守时 `mask_trace_event` 写前脱敏；`is_step_event` 放行上线 | `src/harness/trace.rs` · `src/harness/agent/think.rs` · `src/gateway/trace_protocol.rs` · `shared/protocol/src/events.rs` |
| **Step 与 reducer** | `StepEntry{iteration, thinking, text, tools, notes, status}` · `step_headline` · `DetailLevel` · `Transcript::{apply_live, apply_replay, finish_replay}` | `shared/ui_logic/src/transcript/{step,detail,reducer}.rs` |

### 7.2 三条裁定，写在这里因为别处会被"统一"掉

- **两条腿一处派生是结构性的，不是纪律性的**：`StreamEvent::AgentTrace { event }` 与 `trace.by_runs` 的行是同一个 `AgentTraceEvent`，reducer 的两条腿都进 `apply_trace`。G2（`the_live_leg_and_the_replay_leg_fold_to_the_same_entries`）比较的是去掉时钟后的整份条目。
- **`iteration: None` 永不被追认**：`TurnStarted` 之前到达的帧（旧服务端、`simple.rs`/slash 路径）落进一个无编号 step；后来的 `TurnStarted` 开新 step。猜步号是判据 §8 那类反转。
- **`RunSummary.loops` 是终局闸**：broadcast `Lagged` 丢掉的帧没分配过 `seq`，`MissedSeqs` 看不见；reducer 在 `RunComplete` 时比较 `steps_seen < loops` 报 `NeedsResync`，**不**修补条目。

### 7.3 🛑 零消费者区间（有日期的债）

**测量（2026-09-23，Phase S 合并时）**：`transcript::reducer` / `step` / `detail` 在 `interfaces/` 里零调用者；`TranscriptEntry::Step` 在 TUI 里只有编译臂。`AgentTraceEvent::ReasoningEmitted` 在 TUI 是 `=> {}`，在 Panel 被 `kind` 分派静默忽略。这是 spec §11 裁定的顺序（S → T → P）。
**关闭条件**：Phase T（TUI）落地即关闭 reducer/step/detail 这一条；Phase P 关闭 Panel 那一条。**若 T 与 P 都不发生**，reducer 是一棵带 30 余条测试、没有消费者的树——届时删除它比重新推导便宜，前提是有人被告知它在这里。

### 7.4 这一层还没做的

- 老 run（R9 之前录的）在 `task_traces` 里没有 reasoning——冷加载时那些 step 的标题退到 text 首句 / 工具计数，不回 session 日志去猜（spec §13）。
- `AgentTraceEmitSink` 的 sender 随 sink Arc 存活；后台子代理经 `SubagentTool::with_trace_sink` 持有同一 sink。对**已完成**的 run 无害（drain 已在 `Complete` 上退出，无接收者的 `send` 被忽略）；对"loop 之前失败"的 run，没有子代理能存在（loop 没跑），所以 `helpers.rs:413` 那个无界 `drain.await` 今天不可达。**重访条件**：任何在 loop 之前就 spawn 子代理的路径出现。
```

In §0's delivery table add one row after the shared-core row: `| **按迭代折叠（Phase S）** | 单管道 · \`ReasoningEmitted\` · Step/reducer——见 §7 | \`shared/ui_logic/src/transcript/{step,detail,reducer}.rs\` |`. In §5.1's leading blockquote, after `Panel（Phase C）未动。`, add: `**2026-09-23 起本节的形状复现在 §7.3**（reducer/step/detail 的零消费者区间）——同一形状、新的日期，不是同一笔债。`

- [ ] **Step 2: `FEATURE_LOCATOR.md` — create `### 6.13`**

Insert after the `### 6.12 …` section (i.e. immediately before `## 7. Desktop（桌面端）` at line 4468):

```markdown
### 6.13 转录呈现：文件变更侧信道 · 共享渲染核 · 按迭代折叠 (Transcript Rendering · Stepwise Folding)

**真源** [TRANSCRIPT_RENDERING.md](TRANSCRIPT_RENDERING.md)（§0–§6 = 2026-09-06 CC-render 轮的侧信道、两个 RPC、共享核；§7 = 2026-09-23 按迭代折叠 Phase S）。
**状态（2026-09-23）**：Phase A/B 已合并（TUI 是共享核的客户端）；Phase S 已合并——服务端单管道 + `ReasoningEmitted` + `Step`/reducer，**零渲染器**（§7.3 有日期的债）；Phase T/P 未建。
**这一节曾经只是一个悬空指针**：本文件里十一处 `§6.13`（附录 D.0.181–183、D.3.35–36 等）从 2026-09-07 起指向一个不存在的标题；2026-09-23 Phase S 的文档任务补上了这一节。指针悬空而没人报错，是因为 markdown 的 `§6.13` 不是链接（判据 §7：唯一的搜索命中常常是一句撒谎的引用）。
**判据实例**：附录 D.0.181–183（侧信道脱敏 / 占位符可达性 / 合并规则 vs 整条替换）· D.3.35–36（blob 前缀 / 硬链接）· 本轮新增见附录 D 的 2026-09-23 条目。
```

Then add the appendix-D entries this phase earned (each in the file's existing `**附录 D.x.N** · **一句话** —— 正文 → §6.13` shape; number them after the current last entry of their group):

- **D.0 (跨子系统)**: "两条异步管道各自分配序号，序号就不是顺序" — the `AgentTraceEmitSink` mpsc + spawn race; fix = one channel, not a stamp; the guard is a `try_recv` with no runtime (G1a) because the effect test alone (G1b) passes by luck on a yield.
- **D.0**: "一个 spec 写下的前提，计划期要先量再用" — the 09-23 spec asserted the session log carried per-iteration reasoning with a join key; `turn_id` is the user turn's uuid; ruling R9 replaced the read leg with a trace variant.
- **D.3 (工具)**: "`RunSummary.loops` 是终局闸，因为 `Lagged` 丢的帧没有序号" (spec §5.1).
- **D.0**: "一个文件里十一处引用指向不存在的标题" (the §6.13 finding).

Also add the triggers to appendix E.0 as one-liners pointing at the D entries (E.0 is the group every change reads).

- [ ] **Step 3: `SESSION_KNOBS.md` — one line**

After the table (before `> **别急着给 Busy Input 加参数**`), add:

```markdown
> **显示密度 `brief`/`full` 刻意不是会话旋钮**（2026-09-23 裁定 R7）：它是客户端 chrome，住在每台设备上（Panel `appearance` 的 localStorage 轴、TUI `<aleph_home>/tui-detail`），判定在 `shared_ui_logic::transcript::detail`。哪天要 R8 会话式入口，落点见 spec 2026-09-23 §13——先记落点，不预建。
```

- [ ] **Step 4: Spec status line**

In the spec's header, change `**Status:** 七节设计逐节获用户认可（2026-09-23），待 spec 文件审阅` to `**Status:** 已审阅；Phase S 实施中（计划 \`docs/superpowers/plans/2026-09-23-stepwise-fold-phase-s-server-and-shared-core.md\`）`.

- [ ] **Step 5: Grep for the fact, not the site**

```bash
rg -n "零客户端|zero.consumer|没有渲染器|mpsc\(256\)" docs/reference/TRANSCRIPT_RENDERING.md docs/reference/FEATURE_LOCATOR.md CLAUDE.md
```

Read every hit: each must describe a CURRENT fact with its date. The CLAUDE.md routing row (`trace.tool_output 仍零客户端`) is still true and stays.

- [ ] **Step 6: Commit**

```bash
git add docs/reference/TRANSCRIPT_RENDERING.md docs/reference/FEATURE_LOCATOR.md docs/reference/SESSION_KNOBS.md docs/superpowers/specs/2026-09-23-stepwise-transcript-folding-design.md
git commit -m "docs: TRANSCRIPT_RENDERING §7 (Phase S), FEATURE_LOCATOR §6.13 created, SESSION_KNOBS note

FEATURE_LOCATOR had eleven references to §6.13 and no such heading;
the section now exists and says so. §7 records the one-pipeline fix,
ReasoningEmitted, the Step/reducer core, and the dated zero-consumer
interval Phase T closes."
```

### Task 12: Full verification, baseline diff, hand-off

**Files:** none

- [ ] **Step 1: The minimum verification set (CLAUDE.md's six + this phase's two)**

Run each from the worktree root through the Bash tool; long ones detached (`run_in_background: true`):

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --lib --no-run 2>&1 | tail -2
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo test -p alephcore --bins 2>&1 | tail -3
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo check -p alephcore --features test-helpers --all-targets 2>&1 | tail -2
cargo test -p aleph-panel --lib --no-run 2>&1 | tail -2
CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo check -p aleph-desktop-windows 2>&1 | tail -2
just _stage-shell-placeholders && CARGO_TARGET_DIR=D:/Workspace/Aleph/target cargo clippy --workspace --all-targets --exclude aleph-desktop-macos --exclude aleph-desktop-linux 2>&1 | grep -E "^warning|^error" | sort | uniq -c | sort -rn > "$SCRATCH/clippy-tip.txt"; wc -l "$SCRATCH/clippy-tip.txt"
cargo build -p shared-ui-logic --no-default-features 2>&1 | tail -1
cargo build -p aleph-tui 2>&1 | tail -1
```

The third line is the memory-sanctioned substitute for `--test '*' --no-run` on a shared target dir; if you have the disk and 40 minutes, also run `cargo test -p alephcore --features test-helpers --test '*' --no-run -j 1` from the worktree's own target dir. Windows: `aleph-desktop-macos` / `-linux` are excluded (memory `windows-full-verify-scope`).

- [ ] **Step 2: The crate suites**

```bash
cargo test -p shared-ui-logic --no-default-features 2>&1 | tail -3
cargo test -p shared-ui-logic 2>&1 | tail -3
cargo test -p aleph-protocol 2>&1 | tail -3
cargo test -p aleph-tui 2>&1 | tail -3
cargo test -p aleph-cli 2>&1 | tail -3
```

Expected: shared-ui-logic = branch-start + (Task 7: 9 + 1 proptest + 1 group; Task 8: 2; Task 9: 20; Task 10: 3) — count the ones you actually added; aleph-protocol = branch-start + 2; aleph-tui and aleph-cli unchanged.

- [ ] **Step 3: alephcore `--lib`, names against the baseline**

```bash
CARGO_TARGET_DIR=D:/Workspace/Aleph/target CARGO_PROFILE_TEST_DEBUG=line-tables-only \
  cargo test -p alephcore --lib -- --test-threads=1 2>&1 | tee "$SCRATCH/tip/alephcore-lib.out" | tail -5
grep -E "^test .* \.\.\. FAILED" "$SCRATCH/tip/alephcore-lib.out" | sed 's/^test \(.*\) \.\.\. FAILED/\1/' | sort > "$SCRATCH/tip/failed-names.txt"
comm -13 "$SCRATCH/baseline/failed-names.txt" "$SCRATCH/tip/failed-names.txt"   # NEW reds
comm -23 "$SCRATCH/baseline/failed-names.txt" "$SCRATCH/tip/failed-names.txt"   # GONE reds
```

Expected: `NEW` is empty. Any name in it belongs to this branch until proven otherwise. `GONE` should be empty too (this phase fixes no baseline red; if one disappears, say which and why in the report — do not claim it).

- [ ] **Step 4: rustfmt, per touched file**

```bash
git diff --name-only 9f4e905dd..HEAD -- '*.rs' | xargs rustfmt --edition 2021 --check
```

Expected: no output. (`cargo fmt --check` over-reports ~90 untouched files here; per-file is the instrument.)

- [ ] **Step 5: The severed-wire sweep this phase owes (three rounds running)**

```bash
rg -n "ReasoningEmitted" --type rust src/ shared/ interfaces/ | grep -v "test" | awk -F: '{print $1}' | sort | uniq -c
rg -n "apply_replay|apply_live|finish_replay|step_headline|effective_open" --type rust interfaces/ src/ | grep -v "shared/ui_logic"
```

First command: every producer and consumer of the new variant (harness emit ×2, trace_protocol, unattended sink, emit sink filter, protocol kind/presentation, TUI arm). Second command: EXPECTED EMPTY — the zero-consumer interval is the sanctioned state, and Task 11 §7.3 says so with a date. If it is not empty, something rendered this without a plan task — read it.

- [ ] **Step 6: Report**

Write the final report with: the tip sha; every number with its predicate and command; the `NEW`/`GONE` name lists; the CEILING before/after; the clippy warning count in touched files (must be 0); the list of everything NOT done (real-machine `qa/transcript_fold` — Phase T; Panel arm for `reasoning_emitted` — Phase P; the subagent-lifetime residue — §7.4). Then merge by fast-forward into `main` from the main checkout (`ExitWorktree{action:"keep"}` first — a worktree-isolated session cannot merge; memory `cc-render-r1-round`).

---

## Leftovers this plan declares (not tasks)

- `qa/transcript_fold/run.sh {order,replay}` (spec §11) needs a rendered client to be worth running; it is a Phase T deliverable and is recorded there.
- The Panel's `apply_trace_event` silently drops `reasoning_emitted` until Phase P adds its arm — by design of that dispatcher (string `kind`), not a compile error; Phase P's plan must grep for it.
- `RunSummary.loops` vs `TurnStarted` count: `TurnStarted` is emitted per `run_turn_internal` (`think.rs:321`), `iterations` is bumped in the Continue arm (`agent.rs:611`) and the follow-up arm (`:791`); the grace turn emits neither. Task 9's guard fires only when `steps_seen < loops`. If a real run ever shows `loops > steps_seen` with no lag (a bump without a `TurnStarted`), the guard is a false positive and the client's resync is wasted, not wrong — measure on the first real-machine run in Phase T and record the answer beside the guard.

