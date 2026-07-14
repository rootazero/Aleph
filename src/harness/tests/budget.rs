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
/// **5997 → 5863 (2026-07-14).** Lowered — the ratchet turning the way it is
/// supposed to. Nothing was moved to buy this: every line below is a deletion
/// of code that had no production consumer, plus one merge of two arms that
/// were byte-for-byte the same.
///
///   - `trait_def.rs` −56: the `Harness` trait itself, including its default
///     `run()` loop. `AgentHarness` was the only impl and overrode `run`; the
///     real polymorphic seams are `SessionDriver` and `Arc<dyn HarnessRunner>`.
///     The one doctest that "proved" object-safety was the only caller.
///   - `chain_context.rs` −21: `with_max_depth` (called only from `#[cfg(test)]`)
///     and `Display` (called only by a test asserting its own format).
///   - `callback.rs` −11 / `agent.rs` −5 / `act.rs` −5: the `on_complete` and
///     `on_tool_call` callback channels. Nine emit sites in the loop, zero
///     production listeners — every terminal consumer already rides
///     `on_complete_with_outcome`, every tool listener `on_tool_call_start`.
///   - `trace_sink.rs` −10: `on_init_seam`, a Stage-7 telemetry channel whose
///     only non-forwarding subscriber lived in a test.
///   - `think.rs` −21: the still-overflow and retry-error arms of
///     `reactive_fit_and_retry` merged (they differed only in which error
///     surfaced — and the still-overflow arm was silently discarding a *billed*
///     response without accounting it), and `fire_grace_turn` folded into
///     `fire_boundary_grace_turn` (the diminishing-returns site was the sole
///     caller and it judged the grace turn from a stale event log).
///
/// Measured, not hand-counted: this test is the measurement. The number here is
/// whatever `the_harness_line_budget_does_not_grow` prints when it fails, and
/// nothing else — that is the whole point of the file.
const CEILING: usize = 5863;

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
