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
const CEILING: usize = 5082;

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
