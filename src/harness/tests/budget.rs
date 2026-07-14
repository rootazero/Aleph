//! R10 budget guard — the redline that could not be checked.
//!
//! `src/harness/CLAUDE.md` caps this directory at **12 files / ~4900 lines**
//! and defines the measurement precisely: *lines from the start of each file up
//! to its first `#[cfg(test)]`* — inline tests do not count, and `tests/` (where
//! this file lives) is outside the budget entirely.
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
//!    "2026-07-04: 5077 行, 超 177 行" — and reality: the harness was ~1100
//!    lines over, not 177.
//!
//! A redline whose status line is computed by hand, from an ambiguous rule, is
//! decoration. This test pins the measurement in code and runs it inside the
//! gate everyone already runs (`cargo test -p alephcore --lib`).
//!
//! ## This is a ratchet, not the constitution
//!
//! [`CEILING`] is today's real, reproducible figure, so the existing debt is
//! frozen and visible instead of compounding silently. [`TARGET`] is what R10
//! actually asks for. Paying the gap down means *lowering* `CEILING`. Raising it
//! is permitted, but only as a deliberate act justified in the commit — which is
//! the entire point: the growth to 5990 happened without anyone ever having to
//! say so.

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

/// What `src/harness/CLAUDE.md` asks for. Not currently met — see [`CEILING`].
const TARGET: usize = 4900;

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
/// Net **−124**. The debt to R10's [`TARGET`] is now **839** lines, not 963.
///
/// Measured, not hand-counted: this test is the measurement. The number here is
/// whatever `the_harness_line_budget_does_not_grow` prints when it fails, and
/// nothing else — that is the whole point of the file.
const CEILING: usize = 5739;

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
         (R10's target is {TARGET}. The harness has been over it for a long time; \
         CEILING freezes that debt — it is not a licence to add to it. Moving \
         inline tests to src/harness/tests/ does NOT help: they were never \
         counted.)\n\n\
         per file: {per_file:#?}"
    );
}
