//! R10 budget guard — the redline that could not be checked.
//!
//! `src/harness/CLAUDE.md` caps this directory at **12 files** and holds the
//! line count with a ratchet ([`CEILING`]), not a fixed floor. It defines the
//! measurement precisely: *lines from the start of each file up to its first
//! `#[cfg(test)]`* — inline tests do not count, and `tests/` (where this file
//! lives) is outside the budget entirely.
//!
//! Two things were missing, and together they let the budget drift unnoticed:
//!
//! 1. The one automated check that existed (`scripts/graph-audit.mjs`, check
//!    `redline-r10`) counted only the **file count** — the single number that
//!    has been exactly 12 since the rule was written and therefore can never
//!    move. It never counted lines. It also needs a generated knowledge-graph
//!    artifact to run, and is wired into no gate.
//! 2. The line count was being measured by hand, and the obvious reading of
//!    "up to the first `#[cfg(test)]`" is **wrong** — see [`budgeted_lines`].
//!    It cut `agent.rs` at line 215 (a `#[cfg(test)]` on a test-only accessor
//!    sitting in the middle of a production `impl`) and threw away the 846
//!    lines after it. That is the entire gap between the recorded status —
//!    "2026-07-04: 5077 lines, 177 over ceiling" — and reality: the harness was ~1100
//!    lines over, not 177.
//!
//! A redline whose status line is computed by hand, from an ambiguous rule, is
//! decoration. This test pins the measurement in code and runs it inside the
//! gate everyone already runs (`cargo test -p alephcore --lib`).
//!
//! ## The ratchet IS the redline (2026-07-15)
//!
//! There is no separate target below [`CEILING`] anymore. The old `TARGET = 4900`
//! was retired: it was never a measured floor, only the residue of the same
//! hand-count bug this file exists to kill (an indented `#[cfg(test)]` truncated
//! `agent.rs` and hid 846 lines, so ~5997 was mis-recorded as ~4900). Defending
//! that number meant defending the bug. The honest, reproducible figure is
//! [`CEILING`], and it *is* the redline: pay it down by *lowering* it; raising it
//! is permitted only as a deliberate act justified in the commit against R10's
//! three questions. That discipline — down-only without a written reason — is the
//! whole mechanism, and it is untouched. What changed is that the loop no longer
//! carries a phantom debt against a floor that never really existed.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// The 12 files R10 names: 8 top-level + 4 under `agent/`.
const BUDGETED: [&str; 12] = [
    "src/harness/mod.rs",
    "src/harness/agent.rs",
    "src/harness/deps.rs",
    "src/harness/trait_def.rs",
    "src/harness/callback.rs",
    "src/harness/chain_context.rs",
    "src/harness/trace.rs",
    "src/harness/trace_sink.rs",
    "src/harness/agent/think.rs",
    "src/harness/agent/act.rs",
    "src/harness/agent/guardrails.rs",
    "src/harness/agent/prompt.rs",
];

/// What the 12 files actually total today, under the documented measurement.
/// Frozen, so the overrun cannot keep growing the way it grew to here.
///
/// **P2 Task 6 (2026-08-06): 5127 → 5103 (−24).** Speaker labels for multi-human
/// project rooms, and the first time the ratchet caught the *ceiling itself*
/// having drifted: `CEILING` read 5146 while the measured total was 5127, so a
/// +5 change would have passed silently. It is lowered to the measurement here,
/// which is what the ratchet is for.
///
/// The change had to reach `prompt.rs` — the events→`UnifiedMessage` walk is the
/// one place a user turn becomes something the model reads — so it went in as
/// **one call, not one more branch**. `user_turn_text` (in `thinker/nudges.rs`,
/// the sink this file's own list names for model-facing copy) absorbed the
/// `user_interjection_note` `if/else` *and* the new label, turning 10 lines of
/// wording decisions into 6 lines of delegation: **prompt.rs −4**. The wording
/// itself — which decoration wraps which, and why that order keeps
/// `is_synthetic_reminder` byte-identical — is cognition and now lives in one
/// place instead of straddling the boundary.
///
/// The rest is `SessionEvent::synthetic_user`: three hand-built literals of the
/// same harness-authored message (`agent.rs` soft-failure warning, `think.rs`
/// stop-hook halt and verifier veto) collapsed to one constructor in
/// `session/events.rs`, **−23**. That is the third copy, so P6's rule of three
/// fired on schedule; the reason to collapse it rather than add
/// `author_user_id: None` three times is that a harness-authored message having
/// no human author is now true *by construction*. A fourth synthetic message in
/// the loop cannot get it wrong, and cannot spend budget getting it right.
///
/// Net **−24**, so no R10 three-question answer is owed. For the record the new
/// field would have passed it anyway: (1) scaffolding — "who typed this" is a
/// runtime fact of the log, and no judgment is made about it; (2) a stronger
/// model needs it *more*, since nothing about model strength lets it infer
/// authorship from an unlabelled merged transcript; (3) two consumers today, the
/// prompt and (P2 Task 8) the Panel bubble.
///
/// **Batch 1 (2026-07-14): 5997 → 5863.** Pure deletion — dead trait (`Harness`
/// and its default `run()`), dead callback channels (`on_complete`,
/// `on_tool_call`), dead telemetry (`on_init_seam`), and two byte-identical
/// arms merged. Nothing was moved to buy it.
///
/// **Batch 2 (2026-07-14): 5863 → 5739.** The interesting one for honest
/// bookkeeping: it is the first batch that *added* production code to the loop
/// and still came out ahead. Three bug fixes and a concurrency guard cost **+21**
/// — and they were paid for out in the open, by putting cognition where R9 says
/// it belongs and by sinking a guardrail into the guardrail layer.
///
///   - **−90, wording sink.** The nine model-facing strings the loop injected
///     (`MAX_STEPS_HINT`, `MAX_OUTPUT_TOKENS_RESUME_NUDGE`, `INTERRUPTION_NOTE`,
///     two synthetic tool-error causes, the deferred-result reason, three
///     interpolating note builders) moved to `src/thinker/nudges.rs`:
///     `think.rs` −30, `prompt.rs` −36, `act.rs` −24. Prompt copy is cognition
///     (R9); the harness is scaffolding (R10). A pure relocation — the rendered
///     strings are byte-identical, pinned by golden tests in `nudges.rs`.
///   - **−55, guardrail sink.** The input guardrail left the loop for
///     `GuardrailRegistry::screen_session_input` (`agent/guardrails.rs` −40,
///     `agent.rs` −14, `think.rs` −1). It had screened only the tail's newest
///     user message while `build_prompt` replays the *whole* log every turn, so
///     a sanitised secret went back on the wire in cleartext from turn 2 onward.
///     A `Block` on a replayed message degrades to redaction: events are
///     immutable and re-screened forever, so a symmetric block would end every
///     future turn and brick the session permanently.
///   - **+8, `think.rs`.** The `max_output_tokens` resume loop kept only the
///     final continuation, so a long answer was persisted — and re-prompted —
///     starting mid-sentence. Partials now accumulate and are concatenated
///     *before* the output guardrail, which therefore also screens the first half.
///   - **+11, `prompt.rs`.** `SessionEvent::SystemMessage` fell into `_ => {}`,
///     silently erasing the `[Context Summary]` head a split child session is
///     rebuilt from. (The plan estimated +6; rustfmt expands the match arm to 8
///     lines and 3 more are the comment naming the bug. Recorded at its real
///     cost, because an estimate that quietly absorbs the difference is exactly
///     the bookkeeping this file exists to prevent.)
///   - **+2, `act.rs`.** Parallel admission derives its disjointness proof from
///     the model's *original* args, but PASS 1 executes the guardrail-*rewritten*
///     ones. A PII mask collapses two distinct paths onto one `[PHONE]`
///     placeholder — turning two calls admitted as disjoint into two concurrent
///     truncating writes to the same file. Any rewrite now serializes the batch.
///
/// Net **−124**. The debt to the old 4900 target was then **839** lines, not 963.
///
/// **Batch 3 (2026-07-14): 5739 → 5593.** The Act-period wall clock left the
/// loop. It was never scaffolding: deciding *how long a tool may run* is a
/// per-tool property the tool itself declares, and the harness was second-
/// guessing it with a run-level `turn_timeout` — then converting the overrun
/// into `StalledTurn`, which **kills the run**. Sinking the clock to the tool
/// chokepoint (`src/tools/scoped/dispatch.rs::execute_inner`, below every gate
/// that can wait on a human) turns the same overrun into a recoverable
/// `ToolError::Timeout` the next Think turn reads, and lets the loop drop the
/// machinery that existed only to run that clock.
///
///   - **−149, `act.rs`.** `resolve_effective_budget`, both `describe()` budget
///     probes, both `tokio::time::timeout` wrappers, the serial `StalledTurn`
///     recovery block, the parallel `budgets` vector / `Err(elapsed)` arm /
///     `first_stall`, and the `TurnPhase` / `STALLED_CALL_CAUSE` /
///     `budget_overrun_cause` imports. `ExecOutcome` collapses to
///     `Result<ToolOutput, ToolError>`.
///   - **+2, `deps.rs`; +1, `trait_def.rs`.** Both files *asserted* that
///     `turn_timeout` bounds Act. It no longer does — a doc that names an
///     invariant has to be true, so both now say Act is bounded per-tool and
///     `turn_timeout` bounds only Think.
///
/// Net **−146**, all of it real deletion (the +3 is documentation of the
/// invariant that moved). The debt to the old 4900 target was then **693** lines.
///
/// **Batch 4 (2026-07-15): 5593 → 5037.** The two largest relocations of the
/// campaign, and the first to remove whole *dependencies* rather than lines.
/// Both are moves, so the only acceptable behaviour delta was zero, and both
/// diffs were audited line-by-line against `HEAD` to prove it.
///
///   - **−221, `trace.rs` (465 → 244).** The six
///     `From<LoopTrace*> for aleph_protocol::AgentTrace*` impls moved to
///     `src/gateway/trace_protocol.rs`, next to the only three call sites that
///     ever used them. Serialising for a transport the loop knows nothing about
///     was never scaffolding *for the loop*. The prize is not the 221 lines:
///     `rg aleph_protocol src/harness/` now returns **nothing**, so the
///     Think→Act loop no longer depends on the gateway wire protocol at any
///     level. A pure excision — the diff is 0 lines added, 221 deleted, and the
///     moved bodies are byte-for-byte the originals.
///   - **−335, `agent/think.rs` (1844 → 1509).** The reactive-compaction rescue
///     cluster (`drain_context_overflow`, `try_reactive_compact_and_retry`,
///     `reactive_fit_and_retry`, the `MAX_REACTIVE_COMPACT_ATTEMPTS` cap, and
///     the single-caller `compact_to_fit_in_place` wrapper, deleted outright)
///     moved to `src/context/compact/rescue.rs`. It is mechanism, not cognition:
///     the compact-or-not decision is entirely `llm_retry::classify`'s
///     `CompactAndRetry` verdict, produced by the providers layer. So this is not
///     R10's fifth "don't" — the harness still selects no recovery strategy — and it
///     is not a retreat from A2: the model still sees the error and self-heals.
///
/// The seam is what makes it a sink rather than a shuffle: `RescueHost` is
/// declared in the **context** layer and implemented by the harness (P4), with an
/// associated `Fatal: From<AlephError>` so `src/context/` never names
/// `HarnessError`. `rg "crate::harness" src/context/` returns nothing. Task 8's
/// standing verdict — "BLOCKED: depends on private `&self` state, not
/// parameterisable `self.deps.X` fields" — turned out to be wrong. Only five
/// handles on run state were ever needed (LLM call, rescue slot, token
/// accounting, trace, terminate reason), and they fit in a 52-line adapter.
///
/// That adapter, plus the `RescueCx` construction, is why the sink netted −335
/// and not the −367 the plan predicted. Recorded at its real cost, as always.
///
///   - **+6, `agent.rs`.** The sink exposed a lie the move would otherwise have
///     preserved: `MAX_REACTIVE_COMPACT_ATTEMPTS` was decorative. The real cap
///     was a hardcoded `compare_exchange(0, 1)` in the slot, so raising the
///     const would have changed nothing — and after S2 the const sits in the
///     *context* layer while the slot sits here, which makes an ignored cap not
///     merely a footgun but a contradiction of the seam that was just built
///     (policy in context, state in the harness). The slot now reads the cap.
///     Pinned by `the_rescue_slot_is_bounded_by_the_context_layers_cap_not_a_hardcoded_one`.
///
/// Net **−550**, taking the loop to **5043** — the closest it had ever come to
/// the old 4900 target, though still above it.
///
/// **Redline re-determination (2026-07-15).** The 4900 target is retired (see the
/// header): it was a mismeasurement, not a measured floor, so the "143-line debt"
/// was owed to a number that never really existed. [`CEILING`] below is now the
/// redline itself — the honest, ratcheted figure. The discipline is unchanged: it
/// only moves down without a written reason.
///
/// **Batch 5 (2026-07-15): 5043 → 5035.** Not a relocation campaign — a fifth
/// gap-analysis pass (vs the Lilian Weng harness article + codex/pi/hermes) found
/// the loop already leads its reference set, so this is the honest small residue:
/// one vestige removed and one latent bug fixed, netting **−8**.
///
///   - **−~20, `trait_def.rs` + threading.** `TurnPhase::Act { tool_name }` and
///     its `Display` arm went dead when Batch 3 sank the Act wall clock to the
///     tool layer — `StalledTurn.phase` has been invariably `Think` since. The
///     whole `TurnPhase` enum is deleted, `HarnessError::StalledTurn` drops its
///     `phase` field (the `#[error]` string hardcodes "Think"), the two agent.rs
///     consumers feed the separate cross-layer `TerminateReason::TurnTimeout {
///     phase: String }` contract the literal "Think", and the `mod.rs` re-export
///     is dropped. Unconstructable variant → zero behaviour change.
///   - **+~12, `guardrails.rs`.** The tool-call `Sanitize` arm reparsed the
///     redacted-JSON string and, on failure, fell back to `Value::String(text)` —
///     silently replacing a structured args *object* with one opaque string arg.
///     A parse failure now keeps the model's original structured args and warns;
///     a shape change is worse than an un-applied sanitize.
///
/// Net **−8**, taking the loop to **5035**.
///
/// **Batch 6 (2026-07-17): 5035 → 4996.** Pure deletion, no relocation of live
/// code — the one-shot turn driver `AgentHarness::run_turn` (and its two counter
/// helpers `count_assistant_messages` / `count_tool_calls`, used by nothing else)
/// had **zero production callers**: production drives the loop only through
/// `run` → `run_turn_internal`. It existed solely so tests could fire a single
/// turn. A test affordance was costing the loop's line budget, so it moved to
/// `src/harness/tests/harness_ext.rs` as an extension trait
/// (`AgentHarnessTestExt::run_turn`) — outside the 12-file / CEILING budget — and
/// the ~50 call sites keep the identical `harness.run_turn(…)` shape via one
/// import. Answering R10's three questions: it is scaffolding, not cognition; a
/// stronger model does not need it; it has zero *production* consumers. Net
/// **−39** from `agent.rs` (1042 → 1003 budgeted lines).
///
/// Also removed the dead `LoopTraceTurnMetrics.consecutive_errors` field: both
/// emit sites in `think.rs` hardcoded `0`, the wire DTO forwarded the constant,
/// and **no consumer ever read it** (the harness tracks the real
/// `consecutive_failure_turns` in `run()` but never threaded it here — and
/// threading it would grow the loop, which R10 forbids for a signal nobody reads).
/// Deleted end-to-end: core `trace.rs` field, `AgentTraceTurnMetrics` wire field
/// (`shared/protocol`), the `From` conversion, and three test constructions.
/// Net **−3** more (`trace.rs` −1, `think.rs` −2).
///
/// Net **−42** this batch. The ratchet's own measurement now reads **4988** —
/// the prior 5035 ceiling was itself a few lines stale against the live files, so
/// the honest figure landed 5 below the arithmetic (5030-ish − 42). The number is
/// the measurement, not the estimate; that is the whole point of the file.
///
/// **Batch 7 (2026-07-17): 4988 → 5072 (+80).** The first deliberate raise
/// since the ratchet was pinned — two deferred tool-concurrency items land in
/// `act.rs`, both answering R10's three questions in the open:
///
///   - **+~30, ambient call identity.** Act scopes an
///     `approval::CallIdentity { turn_id, call_id }` task-local around each
///     execute future (serial + parallel), replacing the session-log
///     name-scan correlation (`newest_tool_call`, deleted outright from
///     `src/tools/scoped/dispatch.rs`) that had forced every approval-gated
///     call to claim `Exclusive::Global`. Scaffolding, not cognition —
///     identity threading carries no judgement; a stronger model still needs
///     its approval cards correlated; consumers today: every approval card
///     stamp, every session-log `ToolCallApproved/Denied`, and
///     sandbox-elevation records raised mid-execution. The serial wrap is
///     `Box::pin`ed — a bare task-local scope future trips rustc's "`Send` is
///     not general enough" HRTB limitation once the future crosses a
///     `tokio::spawn` chain.
///   - **+~50, completion-order live events.** `buffered` →
///     `buffer_unordered` plus a completion drive loop: each live "done"
///     callback fires the moment ITS call resolves (real wall-clock
///     durations, no head-of-line blocking, per-completion stall-tracker
///     activity), while the transcript — SessionEvent, trace, Layer-3
///     budget, timeline — stays strictly input-order in PASS 2 (the old
///     `emit_*` bodies split into `compose_tool_error_msg` +
///     `persist_tool_*`). The live/transcript split pi, openclaw and codex
///     all share. Scaffolding (event plumbing); model-independent (latency
///     UX); consumer: the Panel live stream (`BroadcastCallback` →
///     `FlowStreamEvent::ToolCallDone`).
///
/// Net **+80**, taking the loop to **5072** — the test's own measurement;
/// the +80 arithmetic said 5068, the same few-lines staleness Batch 6
/// recorded. The discipline is unchanged: down-only without a written
/// reason — this paragraph is that reason.
///
/// Measured, not hand-counted: this test is the measurement. The number here is
/// whatever `the_harness_line_budget_does_not_grow` prints when it fails, and
/// nothing else — that is the whole point of the file.
///
/// 5072 → 5070 (−2): `stream_llm_call` dropped its `as_http_provider()` downcast
/// + one-shot fallback branch for a single polymorphic `execute_streaming_dyn`
///   call. That downcast reached the raw inner `HttpProvider`, skipping the
///   `ThinkLevelProvider`/`MeteringProvider` decorators on every streamed turn
///   (declared `think_level` dropped; no `ProviderUsage` emitted). The streaming
///   side effects moved OUT of the loop into the decorators in `src/providers/`
///   (trait default + three overrides) — cognition/policy stays outside the
///   harness (R10). Down-only ratchet: paid down, no 3-question answer required.
///
/// 5070 → 5008 (−62): removed the `DiminishingReturnsDetector` hard-stop (R10's
/// "5 don'ts" #3 — the loop must make no completion judgement of its own). Gone from
/// `think.rs`: the `after_turn` consumer, its `output_tokens` read, the
/// `GraceReason::Diminishing` grace-turn path, and the `use LoopDirective`
/// import. Deleted outside this budget: the detector, `after_turn`, the
/// `StopDiminishing` directive and `TurnMetrics` in `src/context/budget/`, and
/// `GRACE_NUDGE_DIMINISHING` in `src/thinker/nudges.rs`. A genuinely-stuck run
/// now stops on the harder caps (`max_iterations` / `ToolLoopVerifier` /
/// consecutive-failure) or the model's own judgement — never a middleware
/// heuristic. Down-only ratchet: paid down, no 3-question answer required.
///
/// 5008 → 5082 (+74): **raised without a written reason, and the debt is settled
/// here rather than quietly inherited.** `396c6d200` ("adjust line budget
/// CEILING") moved this constant to make a red test green; the +79 it was
/// absorbing had landed one commit earlier in `c648b5ea4`, and `9241dd193` +
/// `396c6d200` trimmed −5. Raising is *permitted* — silently is not, and the
/// docs then drifted for two days (root `CLAUDE.md` and `src/harness/CLAUDE.md`
/// both still said 5008), which is this file's own failure mode reappearing one
/// layer up. The four changes, against R10's three questions:
///
/// 5066 → 5062 (−4): paid down by the tool-output hygiene round. The Layer-3
/// turn spill stopped calling `ToolResultStore::persist_if_large` directly and
/// now reuses `result_processing::recovery_footer`, which offloads *and* indexes
/// *and* appends the `ctx_search` hint — the same recovery handle Layer 2 emits.
/// The spill previously handed the model a marker over an unindexed blob, so the
/// only way back to the output was re-reading the whole file. Folding the two
/// call sites into one closure paid for the change and −4 besides; the dead
/// `ToolOutputMetadata.truncated` write went with it (write-only field, cut).
/// Down-only ratchet: no 3-question answer required.
///
///   - **`think.rs`, grace-turn wall-clock cap (+~14).** `race_llm_call`'s
///     timeout arm exists only when `deps.turn_timeout` is `Some`, so with turn
///     timeouts disabled a hung provider could hang a run that was already
///     trying to terminate. Now capped by `GRACE_TIMEOUT_BUDGET`.
///     Scaffolding (a clock, not a judgement); a stronger model cannot un-hang
///     a socket; consumer: every `turn_timeout = None` deployment.
///   - **`agent.rs`, split-turn watchdog skip (+6).** A split turn's tail lives
///     in the CHILD session, so the consecutive-failure watchdog's parent-
///     watermark fetch read an empty tail as a clean turn and silently reset
///     the streak. Scaffolding (counter correctness); model-independent — a
///     stronger model does not fix a read of the wrong session; consumer: every
///     split turn.
///   - **`act.rs`, steer checkpoint hoisted to once per group (net −).** Was
///     per tool call, paying a seq-ranged session-store read for every call in
///     the batch. Same mechanical watermark compare, one read per group.
///     Consumer: every serial group.
///   - **`act.rs`, per-batch `canonical` / `claims` threading (+~50).** Arg
///     signatures and concurrency claims computed once in `act()` instead of
///     recomputed per group. Scaffolding; model-independent. **But question 3
///     failed on half of it** — see below.
///
/// 5082 → 5055 (−27): the answer to question 3 above, applied. The threading
/// arrived as `Option<&[..]>` with a "recompute on demand" arm on every `None`,
/// and that arm had **zero consumers**: the only caller passing `None` is
/// `act()`'s fast path, entered on `!parallel_enabled || tool_calls.len() < 2`,
/// which is the very condition `can_parallel_dispatch` rejects on at its
/// `par_n < 2 || tool_calls.len() < 2` guard — it returns `false` before either
/// value is read. No test reached it either. R10 says withdraw a zero-consumer
/// abstraction rather than leave a door open, so `can_parallel_dispatch` and
/// `act_parallel` now take plain slices and `dispatch_group` short-circuits to
/// the serial loop when the batch data is absent. Behaviour is unchanged by
/// construction: the removed branches were unreachable. Down-only ratchet, but
/// recorded because it is the *conclusion* of the answer above, not a separate
/// cleanup. All −27 are the withdrawal: `act.rs` is otherwise byte-identical to
/// `main`, so the figure is not padded with drive-by reformatting.
///
/// **Round 7 (2026-07-29): 5055 → 5066 (+11).** One bug fix in, two dead enum
/// variants out. Net +11, and here is the raise's written reason:
///
///   - **+~13, `guardrails.rs`: a blocked tool call now reaches the timeline.**
///     Every other Act outcome — success, error, within-batch memo hit,
///     cross-batch refusal — ends in `push_tool_invocation`. The tool-call
///     guardrail's `Block` arm was the only one that did not, so a blocked call
///     was absent from `tool_timeline` → `FlowOutcome` → `RunSummary
///     .tool_summaries`. That list is the AUTHORITATIVE terminal state
///     consumers reconcile against precisely because the `agent_trace` mirror
///     is deliberately lossy (`AgentTraceEmitSink` = bounded `mpsc(256)` +
///     `try_send`); a block was therefore the one class of call with no
///     backstop — drop its single live frame and the Panel row stayed "running"
///     forever. The run digest under-counted and `deps.tool_signal_sink` (the
///     dream cycle's `insights.tools` feed) never saw the attempt either.
///     Against R10's three questions: (1) scaffolding — it records an event
///     that already happened, it judges nothing; (2) yes after a model upgrade
///     — a terminal ledger is model-independent, and a stronger model's blocked
///     calls still have to appear in it; (3) three real consumers today
///     (`tool_summaries` / runtime footer digest / tool-signal sink).
///   - **−2, `trace.rs`: `LoopTraceTurnOutcome::{HitLimit, Cancelled}` cut.**
///     Zero producers — `think.rs` only ever emits `Continue` / `Stop`, because
///     caps and cancellation are SESSION-level exits
///     (`LoopTraceSessionOutcome`). Their only mention was the `From` arm in
///     `gateway::trace_protocol`, translating variants nothing constructed.
///     `LoopTraceEvent` is never deserialized in production (serialize-only,
///     over an in-process mpsc), so nothing reads back an old blob through this
///     enum; the protocol-side `AgentTraceTurnOutcome` keeps its wider set for
///     stored blobs, exactly as `AgentTraceTextKind::Intermediate` does.
///
/// **Round 8 (2026-08-02): 5062 → 5084 (+22).** Two Act-path deferrals from the
/// 2026-08-02 tool-layer round, both booked against R10's three questions
/// before a line was written (FEATURE_LOCATOR §3.3 ⑥ (a) and (b) recorded them
/// as "owed, not done" precisely because they land in this budget):
///
///   - **+~16, `act.rs`: the group loop checks `run_cancel`.** After `/stop`,
///     `act()`'s multi-group loop still walked every remaining group. Each one
///     logged a `ToolCallRequested`, registered in-flight, dispatched (taking an
///     instant cancellation error) and logged a `ToolError`. Those are phantom
///     failures: the model reads them in the next prompt as calls that ran and
///     failed, and `tool_summaries` counts them as real errors. The remaining
///     `tool_use` blocks still need their pairing — that is the only reason the
///     old behaviour was survivable — so the checkpoint closes them via the
///     existing `close_unexecuted_tool_uses` rather than just breaking. Three
///     questions: (1) scaffolding — honouring an external stop signal is
///     plumbing, and it is explicitly NOT R10's forbidden completeness
///     judgement (the model did not decide anything; the user pressed stop);
///     (2) yes after a model upgrade — a cancelled run is a runtime fact, no
///     amount of model capability makes those events true; (3) three real
///     consumers (the session event log the next prompt is rebuilt from,
///     `RunSummary.tool_summaries`, `deps.tool_signal_sink`).
///   - **+~6, `act.rs`: the parallel clock starts on first poll.** PASS 0
///     stamped one `Instant` per call, but PASS 1 feeds them through
///     `buffer_unordered(parallelism)`, which polls at most `parallelism` at a
///     time — so every call past the cap was billed for the time it spent
///     queued, not running. The completion-order loop's own comment already
///     claimed the opposite ("its `duration_ms` is the tool's real wall clock
///     instead of being inflated by the wait"), which is the shape this repo
///     keeps paying for: a comment asserting an invariant the code does not
///     hold. The future now carries its own duration. Three questions:
///     (1) scaffolding — a measurement, not a decision; (2) yes after a model
///     upgrade — wall clock is model-independent; (3) two real consumers
///     (`callback.on_tool_call_done` → the Panel tool row, and
///     `persist_tool_success`'s `dur_ms` → `tool_timeline` →
///     `RunSummary.tool_summaries`).
///
/// Deliberately NOT fixed in the same pass, so the raise stays honest about
/// what it bought: `on_tool_call_start` still fires for all N calls in PASS 0,
/// so a queued call is *announced* running before it is polled. Moving that
/// into the futures needs `&mut dyn HarnessCallback` across `'static` boundaries
/// — new machinery in the loop for a state the completion event settles
/// milliseconds later, and the transcript's linear `ToolCallRequested` order is
/// deliberate. Fails question 1.
///
/// **Round 9 (2026-08-03): 5084 → 5109 (+25).** Two prompt-cache correctness
/// fixes off FEATURE_LOCATOR §2.18's follow-up ledger (items 3 and 7). Both are
/// about the *shape of the bytes on the wire*, which is why neither could be
/// paid for outside the loop: `prompt.rs` and `think.rs` are where the request
/// is assembled. Measured, not arithmetic.
///   - **+~19, `prompt.rs`: the orphan scan stops at its own turn.** Rebuilding
///     an assistant message scanned `events[idx + 1..]` to the end of the log
///     for a matching `ToolResult`/`ToolError`. A call id reused by a *later*
///     turn — weaker and proxied models do reuse them — therefore reached back
///     and un-orphaned a `tool_use` block in an assistant message the provider
///     had already cached: the same history rendered differently on a later
///     turn, so the whole message prefix was re-billed at `cache_creation`, and
///     the resurrected block was still unpaired on the wire. Narrowing is safe
///     because every synthetic closure (`act::emit_deferred_tool_results`,
///     `think::close_unexecuted_tool_uses`) carries its *original* turn id —
///     verified before writing the narrowing, since getting that wrong would
///     start dropping legitimate pairs. Most of the +19 is the free function's
///     doc, which records exactly that precondition; trimming it back to hit a
///     number would be the accounting-cosmetics this file exists to prevent.
///     Three questions: (1) scaffolding — it decides nothing, it stops reading
///     bytes that belong to another turn; (2) yes after a model upgrade — a
///     stronger model does not make id reuse or a late result impossible, and
///     the wire-level pairing rule is the provider's, not the model's; (3) one
///     real consumer, the request every turn is built from.
///   - **+~6, `think.rs`: the boundary grace turn keeps the tools array.** It
///     sent `tools: None` to stop itself acting, while its own comment claimed
///     the call "turns into a cache hit". It cannot: Anthropic builds its prefix
///     tools → system → messages, so a request with no tools array shares no
///     prefix with the turn that just ran — and the grace turn replays the
///     entire history, right after a turn that warmed it. It now threads the
///     schema and disables tool use with `ToolChoice::None`, which all four
///     adapters honour. (The §2.18 ledger prescribed `tool_choice: none` alone;
///     that was not enough — the Anthropic adapter implemented `None` by
///     *deleting* the tools array, i.e. the identical wire shape. Fixed there
///     too, outside this budget.) Three questions: (1) scaffolding — request
///     shape, no judgement; (2) yes — prefix construction is a provider fact;
///     (3) one real consumer, all six grace sites funnel through here.
///
/// **Round 10 (2026-08-03): 5109 → 5142 (+33).** The last of FEATURE_LOCATOR
/// §2.18's follow-up ledger (item 2), and the one the ledger itself deferred as
/// "touches the R10 budget, needs its own proposal". Measured, not arithmetic.
///   - **+~24 `prompt.rs` / +~9 `think.rs`: the protected tail counts persisted
///     messages again.** The preflight cheap passes rewrite everything below
///     `len - fresh_tail`, and they were measuring that on a vector whose last
///     entries are not history: `build_prompt` appends up to four
///     `<system-reminder>` nudges and `think.rs` then pushes the recall message.
///     Five synthetic entries against a six-message guard means the guard could
///     shrink to one, and the passes would rewrite a message the model had just
///     read — re-billing the whole message prefix at `cache_creation` for
///     content that was one turn old. `build_prompt_with_transient_tail` returns
///     the count; the plain `build_prompt` stays as the form that discards it,
///     so the ~20 test call sites are untouched and the diff shows only the
///     change. A second effect worth naming: the cut becomes
///     `persisted_len - fresh_tail`, i.e. independent of how many nudges fired,
///     so it also stops jittering across the quantum edge that Round 2's
///     `quantized_tail` installed — the ledger flagged that as unresolved fallout
///     of this same item.
///     Three questions: (1) **scaffolding** — it is an off-by-N in a boundary
///     computation; nothing here judges a message, it only counts which ones the
///     log will still have next turn. (2) **Yes after a model upgrade** — which
///     messages are persisted is a property of the event log, not of the model;
///     a stronger model still must not be shown a rewritten copy of what it read
///     one turn ago. (3) **One real consumer**, `PreflightPipeline::run`'s
///     `fresh_tail_count`, and it is the only caller that needs the count — which
///     is exactly why the count is returned by a second function rather than
///     forced on every caller.
///
/// +12 (5089 → 5101): `LoopTraceEvent::CacheHealthDegraded` — the cache
///     watchdog's rising-edge alarm riding the trace stream.
///     Three questions: (1) **scaffolding** — a data-only event variant (5
///     fields, zero loop logic); the detection cognition lives outside
///     src/harness/ in `prompt_builder::cache_monitor`. (2) **Yes after a
///     model upgrade** — provider cache accounting is a property of the
///     provider contract, not of the model. (3) **Four real consumers** on
///     day one: `task_traces` persistence, the TUI reasoning feed, the Panel
///     trace tree, and the `core/cache-health` doctor check.
/// ±0 (5101 → 5101, context-compaction round): the transient-tail fix that
///     Round 10 gave `PreflightPipeline` was extended to the compactor and the
///     deterministic floor, which cost **+7** here — one argument at the
///     `apply_budget_directive` call, one field in `RescueCx`, and one
///     `transient_tail += 1` beside the max-iterations hint push (the count has
///     to keep describing the vector the rescue later compacts). Paid for, not
///     absorbed: the hint block was flattened from a nested
///     `if let Some(cap) { if cap > 0 && … }` plus a six-line `UnifiedMessage`
///     literal into one `is_some_and` guard plus `UnifiedMessage::user`, **−7**.
///     Recorded because "the ceiling did not move" and "nothing was added" are
///     different statements, and only the second one is free.
const CEILING: usize = 5101;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Production lines: everything before the file's inline test module.
///
/// The cut must be the **top-level** (column-0) `#[cfg(test)]` — the one that
/// opens `mod tests`. Matching *any* `#[cfg(test)]`, including an indented one,
/// is the bug that hid the overrun for months: `agent.rs` carries a `#[cfg(test)]`
/// on a 4-line test-only accessor at line 215 of 1060, and cutting there
/// silently excluded **846 lines of production harness code** — the whole file
/// past that point. It reported the harness as 177 lines over budget when it was
/// really ~1100 over.
///
/// A file with no inline test module (as `agent.rs` now is — its tests moved to
/// `tests/agent.rs` in `448ce1c03`) counts whole.
fn budgeted_lines(body: &str) -> usize {
    body.lines()
        .position(|l| l.starts_with("#[cfg(test)]"))
        .unwrap_or(body.lines().count())
}

/// Every `.rs` sitting directly in `src/harness/` or `src/harness/agent/` — the
/// budgeted surface. `src/harness/tests/` is excluded by construction.
fn harness_sources() -> BTreeSet<String> {
    let root = repo_root();
    let mut found = BTreeSet::new();
    for dir in ["src/harness", "src/harness/agent"] {
        let entries =
            std::fs::read_dir(root.join(dir)).unwrap_or_else(|e| panic!("cannot read {dir}: {e}"));
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                found.insert(format!("{dir}/{name}"));
            }
        }
    }
    found
}

/// A 13th file is the loudest possible R10 violation: the whole point of the 12
/// is that the loop has no room for another *concern*. R10 requires a new file
/// to arrive with a written reason why it cannot live in one of the existing 12.
#[test]
fn the_harness_is_still_exactly_the_twelve_files_r10_names() {
    let actual = harness_sources();
    let expected: BTreeSet<String> = BUDGETED.iter().map(|s| (*s).to_string()).collect();

    let added: Vec<_> = actual.difference(&expected).collect();
    let removed: Vec<_> = expected.difference(&actual).collect();

    assert!(
        added.is_empty() && removed.is_empty(),
        "src/harness/ no longer matches R10's 12 files.\n  \
         added:   {added:?}\n  removed: {removed:?}\n\n\
         A new file means a new concern landed in the loop. Per R10 the 12 \
         harness modules each have a home OUTSIDE src/harness/ — put it there \
         (src/harness/CLAUDE.md lists the sinks). If it genuinely cannot go \
         anywhere else, say why in the commit and update both BUDGETED here and \
         src/harness/CLAUDE.md."
    );
}

/// The half nobody was watching.
#[test]
fn the_harness_line_budget_does_not_grow() {
    let root = repo_root();
    let mut total = 0usize;
    let mut per_file = Vec::new();
    for rel in BUDGETED {
        let body = std::fs::read_to_string(root.join(rel))
            .unwrap_or_else(|e| panic!("cannot read {rel}: {e}"));
        let n = budgeted_lines(&body);
        per_file.push((rel, n));
        total += n;
    }

    assert!(
        total <= CEILING,
        "src/harness/ grew to {total} budgeted lines, over the frozen ceiling of \
         {CEILING}.\n\n\
         Before raising CEILING, answer R10's three questions in the commit:\n  \
         1. Is this scaffolding or cognition? Cognition belongs in the prompt.\n  \
         2. Will a stronger model still need it? If not, delete it.\n  \
         3. How many real consumers does it have today? Zero means withdraw it.\n\n\
         (CEILING is the redline itself — the honest ratcheted figure, not a floor \
         to grow up to. Lower it when you delete; do not raise it without a written \
         reason. Moving inline tests to src/harness/tests/ does NOT help: they were \
         never counted.)\n\n\
         per file: {per_file:#?}"
    );
}
