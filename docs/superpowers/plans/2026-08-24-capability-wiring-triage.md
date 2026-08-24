# Task 3 triage — guards that newly see previously invisible code

Each row predates this round: the guard existed, the code existed, the guard
could not read it. Verdicts are CONNECT (wire it), CUT (delete the dead
abstraction), or REPORT (needs a human decision — do not guess).

## Result: zero new exposures among the 12 corpus-growing guards

`task-3-expected-exposure.md` named 12 guards whose visible corpus actually
grows from this migration (4 corpus walkers + 8 `include_str!` guards). None
of them flipped from `ok` to `FAILED`. In particular the predicted likeliest
hit — `dispatchable.rs`'s builtin-tool-dispatchability census, which gained
~62% more of `definitions.rs` to scan — stayed green.

**Update, retracted by the plan author after independently replicating the
guard's own extraction over both the old-truncated and full source:** the
reason is not "no bug happened to be hiding there" but structural —
`definitions.rs`'s catalogue rows end before the 37% mark, so the 101,980
bytes the old cut discarded are 16 `#[test]` functions and six further
`#[cfg(test)]` markers, containing zero catalogue rows and zero dispatch
arms. Both passes over `definitions.rs` / `core_tools.rs` / `tool_registry_impl.rs`
(old and new) produced identical counts: `catalog hits=162 advertised=172
dispatch arms=189 dispatchable=183 gap=0`. `dispatchable.rs` still belongs on
the list of 12 whose *byte* visibility grows — what was wrong was inferring
"more bytes visible" implies "more matches possible" for this specific guard,
which the guard's own shape rules out. Recorded here so anyone reading this
ledger does not go looking for a finding that was never there to find.

Command and full before/after diff:

```
$ cargo test -p alephcore --lib 2>&1 | tail -5
test result: FAILED. 17016 passed; 1 failed; 17 ignored; 0 measured; 0 filtered out; finished in 31.86s

$ diff <(sort baseline-tests.txt) <(sort after-tests.txt)
16763c16763
< test utils::source_scan::tests::no_module_hand_rolls_the_cfg_test_prefix_cut ... FAILED
---
> test utils::source_scan::tests::no_module_hand_rolls_the_cfg_test_prefix_cut ... ok
16766c16766
< test utils::source_scan::tests::production_prefix_recovers_code_the_old_cut_discarded ... ok
---
> test utils::source_scan::tests::production_prefix_recovers_code_the_old_cut_discarded ... FAILED
```

`17016 passed / 1 failed` in both the baseline and the after-migration run —
the same count, because exactly one test flipped RED→GREEN (guard 3, by
design) and exactly one other flipped GREEN→RED (below). No other test
anywhere in the 17,034-test `--lib` run changed status. This is not "a silent
empty ledger and an unrun diff" — the diff was run, it is reproduced above in
full, and it contains exactly two lines.

## The one newly-failing test, and why it is not one of the 12

| # | Failing test | File it newly reads | What it found | Verdict | Task |
|---|---|---|---|---|---|
| 1 | `utils::source_scan::tests::production_prefix_recovers_code_the_old_cut_discarded` | N/A — see below | A stale floor, not new code | REPORT | Task 13 or immediate follow-up, human's call |

This guard is **not** one of the 12 named in `task-3-expected-exposure.md`,
and it did not newly *see* anything — `production_prefix`'s implementation in
`src/utils/source_scan.rs` was not touched by this round at all. What
changed is the guard's own **input corpus**, because that corpus is the
literal text of every file under `src/` — including the 35 files this round
edited.

### Root cause, isolated by A/B diagnostic

The guard (`production_prefix_recovers_code_the_old_cut_discarded`, Task 2's
"guard 2") walks every source file and counts how many have
`production_prefix(text).len() > old_prefix_cut(text).len()`, where
`old_prefix_cut` is a **local test-only reimplementation** of the exact naive
`text.split("#[cfg(test)]").next()` idiom this round eliminated from
production sites. The floor was `>= 213`, "measured directly against the
shipped `production_prefix` on 2026-08-24" — i.e. calibrated against the
pre-Task-3 corpus, which still contained all 35 hand-rolled occurrences as
literal text.

Several of the 35 migrated sites are **not** inside their file's own
`#[cfg(test)]` module — `dispatchable.rs`'s `production_source`,
`source_census.rs`'s `production_prefix`, and similar top-level helpers are
production code. Their old bodies contained the literal string
`"#[cfg(test)]"` as a `.split()` argument, sitting textually *before* that
file's own real `#[cfg(test)]` boundary. `old_prefix_cut`'s naive whole-text
match has no line anchoring, so on the **pre-migration** file it matched that
embedded literal first and truncated the file far too early — which is
exactly the class of bug this whole subsystem exists to fix, so those files
counted as "recovered" (`new > old`) in guard 2's tally, but for the wrong
reason: not because of a genuine mid-file test item, but because the file's
own now-deleted hand-rolled code was fooling the naive comparison baked into
the guard's test harness.

Isolated with a temporary diagnostic test (added, run, and reverted — not
committed; `git stash` / `git stash pop` around the two runs) that lists
which of the 35 target files satisfy `new > old` against `all_sources()`:

**Before migration** (10 of the 35 files were counted as "recovered"):
```
RECOVERED src/agents/subagent_spawner/fork/tests.rs: old=4206 new=19040 diff=14834
RECOVERED src/teams/mod.rs: old=546 new=1212 diff=666
RECOVERED src/executor/builtin_registry/dispatchable.rs: old=2336 new=7363 diff=5027
RECOVERED src/orchestrator/tests/loader.rs: old=3283 new=6739 diff=3456
RECOVERED src/builtin_tools/browser_tools/mod.rs: old=28063 new=29303 diff=1240
RECOVERED src/gateway/execution_engine/btw_wire_tests.rs: old=10643 new=102916 diff=92273
RECOVERED src/gateway/execution_engine/btw_promote/tests.rs: old=12944 new=17773 diff=4829
RECOVERED src/gateway/execution_engine/tests.rs: old=90727 new=91872 diff=1145
RECOVERED src/gateway/source_census.rs: old=954 new=2453 diff=1499
RECOVERED src/gateway/handlers/memory.rs: old=33752 new=34124 diff=372
```

**After migration** (6 remain — these still have a genuine mid-file recovery
independent of the removed literal):
```
RECOVERED src/agents/subagent_spawner/fork/tests.rs: old=4302 new=18989 diff=14687
RECOVERED src/teams/mod.rs: old=546 new=1212 diff=666
RECOVERED src/executor/builtin_registry/dispatchable.rs: old=2336 new=7246 diff=4910
RECOVERED src/builtin_tools/browser_tools/mod.rs: old=28063 new=29303 diff=1240
RECOVERED src/gateway/source_census.rs: old=954 new=2278 diff=1324
RECOVERED src/gateway/handlers/memory.rs: old=33752 new=34124 diff=372
```

Exactly **4** files dropped out —
`src/orchestrator/tests/loader.rs`, `src/gateway/execution_engine/btw_wire_tests.rs`,
`src/gateway/execution_engine/btw_promote/tests.rs`, and
`src/gateway/execution_engine/tests.rs` — matching the guard's count drop of
213 → 209 exactly (`213 - 4 = 209`). For each of these four, removing the
site's own hand-rolled `.split("#[cfg(test)]")` text made `old_prefix_cut`
land on the same real boundary `production_prefix` already found, so the two
now agree and the file no longer registers as "recovered."

### Why this is not a regression in the sense flagged by the brief

`task-3-expected-exposure.md` distinguishes: a newly-RED guard from the 12 is
a new exposure to triage; one from outside the 12 is "a regression — the
migration changed behaviour where the extractor's output is identical."
`production_prefix`'s output is in fact identical for every file in the
corpus (nothing in `src/utils/source_scan.rs` changed). What changed is the
raw byte content of the four files above, and changing exactly that content
was the explicit purpose of this task. The guard's floor was measured against
a corpus that still contained the bug this task was assigned to remove — the
floor is now stale, not the extractor broken.

### Recommendation (not acted on — REPORT, per instructions)

`recovered >= 213` in `src/utils/source_scan.rs` should become `>= 209`, with
a comment noting that Task 3's migration is what moved it (209 measured
2026-08-24 post-migration; do not confuse this with the 213 figure that
preceded Task 3, the same way that comment already warns not to confuse 213
with the pre-fix 276).

**Done in Fix round 1** (see below): the plan author authorised editing
`src/utils/source_scan.rs` for this. Re-measured with the shipped Rust
extractor (not the plan author's Python replication, which disagreed —
198 vs 209 — because it lacks `code_only`/`char_literal_len` and so
miscounts braces inside string literals; the Rust re-measurement reproduced
209 exactly, confirming the original number and the direction of the
Python instrument's error). Floor is now `>= 209`, with the mechanism
written into the doc comment above the assertion.

## Files scanned but out of Task 3's authoritative list

`tests/canvas_wire.rs:379` hand-rolled the identical
`.split("#[cfg(test)]").next()` idiom. Guard 3 (`no_module_hand_rolls_the_cfg_test_prefix_cut`)
only walked `src/` (via `rust_sources_under(.../src)`), so this integration-test
file was structurally invisible to it and correctly absent from the
authoritative 35-line failure output used as this task's original work list.

**Done in Fix round 1**: guard 3 now walks `tests/` in addition to `src/`
(chained `rust_sources_under` calls; `rust_sources_under` reports paths
relative to `CARGO_MANIFEST_DIR` regardless of which root it walked, so the
existing `src/utils/source_scan.rs` self-exemption string match is
unaffected by the second root). Offender count with the extended walk: **1**
(`tests/canvas_wire.rs:379`) before migrating it, **0** after. Migrated the
same way as the 35 sites above — this crate's `production_prefix` is `pub`
and this file is an integration test of the same `alephcore` crate, so it
calls `alephcore::utils::source_scan::production_prefix(&src)` (the file
already strips comments upstream via its own `read_source_without_comments`
helper, shared with a second, unrelated call site — left untouched, since it
does not hand-roll the `#[cfg(test)]` cut and touching it would have widened
scope beyond the one flagged line).

## Sites outside `alephcore` — REPORT, not migrated this round

Four more sites hand-roll the identical idiom, all outside the `alephcore`
crate that owns `utils::source_scan`:

| # | Site | Crate | Notes |
|---|---|---|---|
| 1 | `interfaces/webchat/src/disposed_reads.rs:411` | `aleph-panel` (webchat, wasm) | See below — the most interesting finding of this round |
| 2 | `interfaces/tui/src/tui/commands.rs:1159` | `aleph-tui` | Corpus walker scanning for `BtwTurn::resolve(` calls |
| 3 | `interfaces/webchat/src/platform/wide/views/settings/network/cluster.rs:546` | `aleph-panel` (webchat, wasm) | Pins that the cluster settings page holds no client-side role gate |
| 4 | `interfaces/webchat/src/platform/wide/views/canvas/shape_view.rs:965` | `aleph-panel` (webchat, wasm) | Local `production_code()` helper for a forbidden-token scan |

None were migrated. `production_prefix`/`strip_comment_lines` live in
`alephcore::utils::source_scan`; `interfaces/tui` and `interfaces/webchat`
are separate workspace crates that do not (and, for `webchat`'s wasm target,
structurally should not) depend on the full server library `alephcore` —
adding that dependency just to reach two functions would be a real
architectural change (R1/R3 territory: a wasm frontend crate pulling in the
whole core), not something this round scoped or should improvise. The fix is
moving `source_scan` (or just `production_prefix`/`strip_comment_lines`) into
a crate all four already share — `shared/protocol` or a new tiny
`shared/source_scan` — which is genuine new scope. **Verdict: REPORT** for
all four; a human needs to decide where that shared crate lives and whether
it is worth minting for four call sites.

Checked and clean: `grep -rn` for the same three patterns
(`.split`/`.find`/`.split_once("#[cfg(test)]")`) across `interfaces/cli/`,
`shared/`, and `desktop/` returns zero hits — the four sites above are the
complete cross-crate list, not a sample of it.

### Site 1 is the interesting one: it is itself a census guard, and it is blind

`interfaces/webchat/src/disposed_reads.rs` is not an ordinary file that
happens to contain a hand-rolled cut — its whole reason to exist, stated in
its own module doc, is: **"no plain `get_untracked()` past an `.await`
inside `spawn_local`. `RwSignal::get_untracked` unwraps — reading a
*disposed* signal panics, and a panic in the panel takes the whole page to
the recovery overlay."** That is the same failure class §7 of CLAUDE.md's
judgement criteria already has an entry for (`preset_picker.rs`, `preview.rs`
— an Escape key crashing the whole Panel because a listener outlived its
owner) — this file is the guard that exists specifically to stop the next
one.

The hand-rolled cut at line 411 is not in that primary guard
(`no_plain_untracked_read_survives_an_await`, which walks the source with
purpose-built lexing — `awaiting_blocks`/`late_untracked_reads` — not a naive
split) but in its sibling in the same file,
`window_listener_tests::every_window_listener_is_removed_or_declared_permanent`
— the guard for exactly the class of bug this module's doc comment cites as
precedent (the 2026-08-18 Escape-crash). So within the one file whose entire
purpose is "stop the panel from crash-panicking on stale reactive state,"
one of its two guards carries precisely the blindness this Task 3 round
exists to remove — invisible to its own comparison the same way the 35 sites
in `alephcore` were, for the same reason, and nothing in this round fixes
it, because it is one crate away from the extractor this round shipped.
