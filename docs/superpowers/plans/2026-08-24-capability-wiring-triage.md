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

**Superseded — the floor is now `>= 193`, not 209.** Fix round 4
(`8d963222a`) taught `LexState` to lex raw strings, 16 more files stopped
returning early, and the number came down again for the same reason 276 came
down to 213. See "Row 1" under *Adjudication — Task 13* at the end of this
file for the verified provenance chain and for today's re-measurement (196).
Do not quote the 209 below as current.

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

> ⚠️ **False, and corrected in Task 13.** Three literal spellings are a list of
> the shapes that existed the day the grep was written, not the class. Searching
> for the *shape* — a string literal opening with `#[cfg(test)]` after any
> leading `\n`/`\r\n` escapes — finds two more outside `alephcore`
> (`i18n_census.rs:155`, which is the correct item-walking extractor, and
> `components/admin_refusal.rs:628`, a genuinely different shape) and **five**
> inside it that `alephcore`'s own guard 3 had never seen. See "New row 6" and
> "New row 7" under *Adjudication — Task 13*.

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

## Fix round 2 — the bare-`*` predicate in `strip_comment_lines`, and its exposure event

**Correction, added in Fix round 3 — do not trust the "5 genuine comment
continuations" claim below.** The re-review opened all five and found
**none of them is a comment.** Two are rustfmt's leading-binary-operator
continuation style for a wrapped multiplication expression
(`    * cfg.warning_threshold.clamp(0.0, 1.0)`); the rest are similarly
ambiguous prose the narrower predicate still could not tell from code. The
true classification of the 479 matched lines was **479 false positives, 0
confirmed comments** — `is_block_comment_continuation` narrowed the defect
without removing it, because a block-comment continuation and a leading
binary-operator continuation have *identical* single-line shape; no
stateless per-line predicate can separate them. This section's own
"distinguishing fact" — *whitespace after `*` means comment, an identifier
means dereference* — is false on its own measured population: `* cfg.
warning_threshold` is whitespace-followed and is multiplication, not a
comment. Fix round 3 replaced `is_block_comment_continuation` with a
stateful implementation reusing `code_only`'s `LexState` (the same
`in_block_comment` tracking `end_of_item` already threads across lines),
which is the only correct answer to "is this line inside a comment right
now" — see the "Fix round 3" section below for the full account, including
why the fix's own state must also track `in_str` (a raw-string CSS/JSON
payload reads exactly like a comment continuation to a state-blind
scanner, the same class of ambiguity one level up).
**Commits `64c33d427` and `b1c2bd666` both repeat the "5 kept as
comments" claim below and are not rewritten** — this correction exists so
a reader who greps those commit messages finds it before believing them.

Task 1's `strip_comment_lines` dropped any line whose trimmed form started
with `*`, intended to catch block-comment continuations (` * text`, a bare
`*`, the closing `*/`). Measured directly against this repo's `src/` tree
(independently reproduced with a temporary diagnostic test — added, run,
reverted, same discipline as the guard-2 floor re-measurement above):

```
$ cargo test -p alephcore --lib utils::source_scan::diag_temp_bare_star -- --nocapture
DIAG genuine=5 not_comment_kept=474
```

**5 genuine comment continuations, 474 real Rust lines wrongly matched** —
every `*count += 1;`, `*vendor,`, `*ref_val = …`, `*self.captured.lock()…`
was silently dropped from what every one of the 36 guards sharing
`strip_comment_lines` could see. Fixed with `is_block_comment_continuation`:
a `*` opens a comment continuation only when what follows is whitespace, end
of line, or `/` (the closing delimiter) — anything else is a dereference or
a glob. Re-measured after the fix with the same diagnostic: **5 kept as
comments (unchanged), 0 real code dropped** (the 474 are now correctly kept
as code). Independently confirmed the 5-vs-474 count matched the plan
author's own measurement exactly before applying the fix.

Two permanent regression tests added
(`strip_comment_lines_keeps_dereferences_and_globs`,
`strip_comment_lines_still_drops_every_continuation_shape`), and the
5-vs-474 measurement is written into `is_block_comment_continuation`'s doc
comment, per the instruction that a future reader who simplifies the
predicate back to `starts_with('*')` needs to see the number that makes that
a regression.

### This is an exposure event — treated like Task 3 itself

Full `--lib` diff against the current Fix-round-1 state (17017 passed / 0
failed / 17 ignored) is exactly two added lines — my own two new tests, both
`ok` — and **zero existing tests changed status**:

```
$ diff <(sort fixround1-tests.txt) <(sort fixround2-tests.txt)
16768a16769,16770
> test utils::source_scan::tests::strip_comment_lines_keeps_dereferences_and_globs ... ok
> test utils::source_scan::tests::strip_comment_lines_still_drops_every_continuation_shape ... ok

$ cargo test -p alephcore --lib 2>&1 | tail -3
test result: ok. 17019 passed; 0 failed; 17 ignored; 0 measured; 0 filtered out; finished in 32.31s
```

**Zero newly-failing tests.** Widening `strip_comment_lines`'s visibility by
474 previously-hidden lines, across all 36 guards that consume it (directly
or via `production_prefix` composition), surfaced no pre-existing defect
anywhere in the corpus. This is reported plainly per the instruction that an
empty result and an unrun comparison must not look identical: the command
was run, the diff is reproduced above in full, and it contains exactly the
two lines this round's own new tests account for.

### Guard states, confirmed rather than assumed

- **Guard 1** (`production_prefix_agrees_with_the_old_cut_where_the_old_cut_was_right`): `ok`, unaffected — it does not call `strip_comment_lines`.
- **Guard 2** (`production_prefix_recovers_code_the_old_cut_discarded`): `ok`, floor unchanged at `>= 209` (later `>= 193`, see the Task 13 adjudication) — it compares `production_prefix` against `old_prefix_cut` directly, neither of which calls `strip_comment_lines`.
- **Guard 3** (`no_module_hand_rolls_the_cfg_test_prefix_cut`): `ok`, offender count still 0 — confirmed rather than assumed, per the instruction: none of its three literal patterns (`split("#[cfg(test)]")`, `find("#[cfg(test)]")`, `split_once("#[cfg(test)]")`) begin with `*`, so widening the bare-`*` predicate cannot change what this guard's own whole-file scan matches.

## Fix round 3 — `strip_comment_lines` made stateful; `is_block_comment_continuation` deleted

### Why narrowing could not work

A block-comment continuation line (` * text`) and rustfmt's own style for a
wrapped multi-line expression (`    * cfg.warning_threshold.clamp(0.0, 1.0)`)
have identical single-line shape — a `*`, then whitespace, then text. That
is a property of the two shapes, not a gap in any one heuristic, so no
predicate that looks at one line in isolation can separate them. The only
question that actually answers "is this a comment" is stateful: was the
lexer already inside a `/* */` when it reached this line? `code_only`
already tracks exactly that (`LexState.in_block_comment`, threaded across
lines for `end_of_item`'s brace-counting) — `strip_comment_lines` was
hand-rolling a stateless approximation of a question the same file already
answers correctly, the same shape of defect this repo's own Task-1-round-2
history already has an entry for (the `'{'`/`'}'` special case removed from
`code_only` rather than kept beside the grammar).

### The fix

`strip_comment_lines` now threads one `LexState` across the whole file and
calls the *existing, unmodified* `code_only` per line — no new lexer, no
duplicated character walk. A line is dropped only when:

- it is not blank, and
- the lexer did **not** enter this line already inside an open string
  (`state.in_str` checked *before* calling `code_only`), and
- `code_only`'s output for this line, trimmed, is empty.

That third condition covers the three cases the old code covered
(`//` lines — `code_only` breaks at `//`, so `out` is empty; a line the
lexer is already inside a block comment for; a line that opens and closes
one with nothing else on it), driven entirely by state `code_only` already
carries — no new pattern-matching. The `in_str` check exists because
`code_only`'s exclusion of string interior (correct for its one caller,
brace-counting — a `{` written inside a string must not count) makes a line
wholly inside an open string produce the *same* empty output a comment line
does; only knowing "was I already inside a string when this line started"
tells the two apart. Raw strings make this common, not exotic: `LexState`
does not lex them specially (documented on `code_only` since Task 1), so a
multi-line raw-string CSS or JSON payload reads exactly like an ordinary
open string, one line at a time — this is precisely the shape of the
CSS-selector false-drop found below.

`is_block_comment_continuation` is deleted entirely.

### RED-first, confirmed before the fix

Two new tests were written to fail under the code as it stood after Fix
round 2 (`is_block_comment_continuation`), confirmed RED, then confirmed
GREEN after applying the stateful fix (`git checkout`/re-apply the patch
around the two runs, same discipline as every other measurement in this
ledger — not committed in the RED state):

```
$ cargo test -p alephcore --lib utils::source_scan::tests::diag_temp -- --nocapture
  (run against the Fix-round-2 code, is_block_comment_continuation still in place)
thread '...diag_temp_multiplication_survives' panicked: a leading-multiplication continuation line was wrongly dropped as a comment
thread '...diag_temp_css_in_raw_string_survives' panicked: a CSS universal-selector line inside a raw string was wrongly dropped as a comment
test result: FAILED. 0 passed; 2 failed
```

Both confirmed RED. Renamed and kept as permanent regression tests
(`strip_comment_lines_keeps_a_leading_multiplication_continuation`,
`strip_comment_lines_keeps_a_css_line_inside_a_raw_string`) after applying
the stateful fix; both pass.

`strip_comment_lines_still_drops_every_continuation_shape` (Fix round 2) is
**replaced** — the reviewer showed it passes unchanged under the OLD rule
too, so it never distinguished the fix from the bug it was meant to catch.
Its fixture is kept (a genuine multi-line block comment, continuation lines
and closing `*/` all dropped — the case statefulness actually buys, as
opposed to the two cases above that a stateless predicate got right only by
accident) under the accurate name
`strip_comment_lines_drops_a_genuine_multi_line_block_comment`.

One collateral fix: the pre-existing, untouched-until-now
`strip_comment_lines_drops_line_and_block_comment_lines` (Task 1) asserted
that a bare `* continued` line gets dropped when its own fixture's block
comment (`/* block */`) had already **closed on the same physical line**
before that line was reached — i.e. its own fixture proved the exact
ambiguity this round's fix refuses to guess at. The stateful implementation
correctly keeps that line (no comment is open when the lexer reaches it),
which is the correct behavior, not a regression, so the test's fixture was
corrected to open a comment that actually stays open into the continuation
line, preserving the test's original assertions and intent.

### Post-fix classification of all 479 previously-matched lines

Independently re-measured with a temporary diagnostic (added, run, reverted
— not committed), replicating `strip_comment_lines`'s per-line decision
directly (not by comparing output text, which a HashSet-based check would
get wrong on duplicate lines) across every line the OLD `t.starts_with('*')`
criterion matched in `src/`:

```
$ cargo test -p alephcore --lib utils::source_scan::diag_temp_recount -- --nocapture
DIAG post_fix kept=479 dropped=0
```

**All 479 are now kept.** This matches the reviewer's finding exactly:
there were zero confirmed comments in that population, so the correct
outcome is that all 479 survive — the true "before vs after" is
**479 wrongly dropped → 0 wrongly dropped**, not the 5-vs-474 framing Fix
round 2 used.

One measurement pitfall worth recording: the diagnostic first ran at
`kept=482`, not 479, because two of this round's own new test fixtures were
formatted across physical source lines using Rust's `\`-continuation
syntax, and two of those physical lines happened to start with `*`
themselves (deliberately — they're testing that exact shape) — polluting
the very census being measured with its own new fixtures. Reformatted both
onto single physical lines (matching the existing multi-line-block-comment
test's style) to keep the measurement clean; the two RED-first tests'
assertions are unchanged.

### Exposure event — zero newly-failing tests

```
$ diff <(sort fixround2-tests.txt) <(sort fixround3-tests.txt)
16767a16768
> test utils::source_scan::tests::strip_comment_lines_drops_a_genuine_multi_line_block_comment ... ok
16768a16770,16771
> test utils::source_scan::tests::strip_comment_lines_keeps_a_css_line_inside_a_raw_string ... ok
> test utils::source_scan::tests::strip_comment_lines_keeps_a_leading_multiplication_continuation ... ok
16770d16772
< test utils::source_scan::tests::strip_comment_lines_still_drops_every_continuation_shape ... ok

$ cargo test -p alephcore --lib
test result: ok. 17021 passed; 0 failed; 17 ignored; 0 measured; 0 filtered out; finished in 31.56s
```

The only changes anywhere in the 17,038-test corpus are this round's own
test churn (net +2: three tests added, one replaced) — `strip_comment_lines_drops_line_and_block_comment_lines`
stays `ok` under its corrected fixture (same name, so it does not appear as
a diff line, but its body changed — see above). **Zero newly-failing
tests**, and none outside expectation: widening `strip_comment_lines` a
second time, this time correctly, again surfaced no pre-existing defect
anywhere in the guard corpus.

### Guard 1/2/3 — re-confirmed

- **Guard 1**: `ok`, unaffected (still does not call `strip_comment_lines`).
- **Guard 2**: `ok`, floor unchanged at `>= 209` (later `>= 193`, see the Task 13 adjudication; still does not call `strip_comment_lines`).
- **Guard 3**: `ok`, offender count still 0 (none of its three literal patterns begin with `*`; the stateful rewrite changes nothing about what text those patterns match).

---

# Adjudication — Task 13 (2026-08-25)

Every row above is now dispositioned. Two of them changed verdict on contact
with the code, one claim above turned out to be false, and the widened search
that proved it false found a sixth row nobody had opened. Each is below with
what was measured, not what was expected.

## Row 1 — the stale floor. Verdict: CLOSED, and it was stale a second time.

The row above says the fix landed at `>= 209`. **The floor in the code is
`>= 193`.** It moved again in `8d963222a` (fix round 4) when `LexState` learned
to lex raw strings: a `r#"{ … }"#` payload inside a `#[cfg(test)]` item stopped
feeding its braces to `end_of_item`, 16 files stopped returning early, and the
tail of their test modules stopped being counted as recovered production code.
That is the same direction as the 276 → 213 move — a number coming down because
the extractor stopped MIS-recognising a shape — and the doc comment above the
assertion says so, step by step.

What was verified is the provenance, not the integer: the assertion's failure
message carries the unbroken chain `193 ← 209 ← 213 ← 276` with the reason for
each step, and the doc comment above it explains each move's mechanism and the
counter-checks (`compared` 2 235 → 2 251 moving the other way in step, `worst`
unchanged at 62 222 bytes). That is a measurement. A floor that had simply
dropped to 193 with no account would be a released ratchet, and the two are
told apart by reading the message, not the number.

**Re-measured today: the actual value is 196, against a floor of 193.**
Obtained by mutating the floor to `>= 999_999` and reading `saw {recovered}`
from the panic (mutation diffed before the run). The floor is therefore three
files of slack below the measurement.

**It was deliberately NOT re-pinned to 196**, and that is the one judgement
call in this row. Every number in this floor's chain is an explained
measurement, and at the time of writing I could not explain 196.

**The mechanism is now known (fix round 1).** Today's extractor reproduces 193
exactly on the `8d963222a` corpus, so the extractor is exonerated and the whole
+3 is corpus drift. But only **one** of the three is a genuine recovery:
`capability/mod.rs`, which really does hold a mid-file
`#[cfg(test)] pub(crate) mod census;`. The other two —
`extension/manager_global.rs` and `mcp/sampling_bridge.rs` — recover only
because they gained **doc comments** that mention the attribute, and
`old_prefix_cut` is an unanchored whole-text match that truncates on prose
exactly as it does on code.

Two consequences, both now written into the doc comment above the assertion:

* the honest re-pin under "genuine recoveries" is **194**, not 196; and
* **this metric drifts upward on documentation churn**, so "the number went up"
  is exactly as uninformative as the doc already says "the number went down" is.

193, 194 and 196 are all correct under three different predicates — the last
explained measurement, the count of genuine recoveries, and what the assertion
literally computes. The floor stays at the first, and the rule this round keeps
re-learning is written beside it: **state the predicate beside whatever number
you write.**

**The right way to close that gap is a guard this round does not have**: a
tree-wide assertion that no `#[cfg(test)]` code leaks into the production half.
Fix round 4 verified exactly that property *by hand, once* ("every one of them
at or after its own file's `#[cfg(test)]` attribute … by an independent
formatting oracle"), and nothing keeps it true. It was scoped and abandoned
here for a measured reason: the obvious oracle — "no `#[test]` attribute
survives into the production half" — has **2 267** violations in `src/` today,
because 120-odd whole-test files carry no `#[cfg(test)]` of their own (their
parent declares them `#[cfg(test)] mod x;`). `cfg_test_portion`'s doc already
declares that blind spot. A correct version needs module-graph resolution.
Recorded as follow-up, not attempted.

## Rows 2–5 — the four cross-crate sites. Three CONNECT, one REPORT.

The recommendation above was "mint a shared crate, a human decides." The spec
settles the crate question (non-goal 1, 不拆 crate) — but the premise under it
turned out to be wrong for three of the four rows.

**`aleph-panel` already owns a correct answer to this question.**
`interfaces/webchat/src/i18n_census.rs::production_lines` is `pub(crate)`, walks
`#[cfg(test)]`-gated **items** rather than cutting at the first marker, strips
whole-line comments, and normalises `\r`. Its own doc records why it is
item-walking: the cut version hid **2 266 lines** including two whole view
modules. Three of the four sites live in that same crate, so their fix crosses
no crate boundary and mints nothing.

| # | Site | Verdict | What was done |
|---|---|---|---|
| 1 | `interfaces/webchat/src/disposed_reads.rs:411` | **CONNECT** | now calls `i18n_census::production_lines`; the annotation look-back still reads RAW lines, because the `// window-listener-permanent:` note is a comment `production_lines` strips |
| 2 | `interfaces/tui/src/tui/commands.rs:1159` | **REPORT + marker + self-check** | `aleph-tui` has no in-crate extractor and cannot reach `alephcore`'s. Keeps the cut, gains the three-part marker D3 asks for and an assertion over the discarded region |
| 3 | `.../settings/network/cluster.rs:546` | **CONNECT** | as row 1; the hand-rolled comment filter went with it |
| 4 | `.../canvas/shape_view.rs:965` | **CONNECT** | as row 1 — and this is the iframe-sandbox guard, so its blindness was a security guard's blindness |

The crate-boundary ruling D3 asks to make durable is now recorded **once**, on
`production_lines` itself, rather than three times at three call sites: what
`alephcore::utils::source_scan` is, why `aleph-panel` cannot reach it (wasm
frontend; R1/R3; spec non-goal 1), and that this is therefore a deliberate
second implementation kept to one per crate instead of one per guard.

### Measured: none of the four was wrong today, all four were blind

Every occurrence of each guard's own needle was listed and each hidden one read
by hand. Production hits hidden from the naive cut: **zero**, in all four. The
hidden occurrences are the scanners' own literals and prose. So this was not a
live bug — it was four guards reporting a clean pass over a region they had not
read, in the one direction (`under`-scan) that a prefix cut can fail.

Sizes of that unread region: 13 files in `interfaces/webchat/src` and 5 in
`interfaces/tui/src` carry more than one `#[cfg(test)]` occurrence, discarding
between **728 and 60 616** bytes each. (An earlier draft of this row said
"48 825" — that was the webchat maximum quoted as if it were the whole span.
The file counts are exact.)

The two worth naming, because they belong to the one site left on REPORT — the
`aleph-tui` census, which walks `interfaces/tui/src`:

* `tui/app/mod.rs` declares `#[cfg(test)] mod tests;` on **line 11 of 1 470**.
  The census reads eleven lines and skips 1 458 — **99% of the file unread**.
* `tui/btw_overlay.rs`'s first marker is an indented `#[cfg(test)]` at line 97,
  hiding 30 159 bytes: **the BTW subsystem's own view file is ~86% unread by
  the BTW census.**

Whoever picks up that REPORT verdict starts from these numbers rather than
measuring them again.

### And a rule, so the fix is not three edits that can be undone one at a time

`i18n_census::no_guard_in_this_crate_hand_rolls_the_cfg_test_cut` now scans the
whole `aleph-panel` source tree for the shape. One named, size-pinned exemption
(`components/admin_refusal.rs`, below). The shared walker
`disposed_reads::rust_sources` deliberately skips `disposed_reads.rs` — for the
*other* guard's reason — so that file is added back explicitly, with an
assertion that it is still there; otherwise the new rule would silently not
cover one of the three files it was written for.

### The tui self-check flagged itself, and only falsification showed it

Worth recording because it is the failure D7 names. The first version skipped an
occurrence only when the character immediately before it was `"`. That covers
the scanner's own `line.find("BtwTurn::resolve(")`, and it does **not** cover the
assertion message, where the needle sits inside a literal but behind a backtick.
So the guard reported its own failure text as an offender. It is now an
"is this offset inside a double-quoted literal on this line" check — per line and
resetting at each newline, so a miscount costs one line instead of silently
blanking the rest of the file, which is the unbounded version's failure mode and
the one Task 1's round-3 notes above already paid for.

## New row 6 — the ledger's completeness claim is false. Verdict: REPORT.

Above: *"the four sites above are the complete cross-crate list, not a sample of
it."* That claim rests on `grep`-ing three literal spellings
(`.split(`/`.find(`/`.split_once("#[cfg(test)]")`). Searching for the **shape**
instead — a string literal that opens with the attribute, after any leading
`\n`/`\r\n` escapes — finds two more outside `alephcore`:

* `interfaces/webchat/src/i18n_census.rs:155` — **not a defect.** This is
  `production_lines` itself, the item-walking extractor rows 1/3/4 now use.
* `interfaces/webchat/src/components/admin_refusal.rs:628` —
  `.or_else(|| body[1..].find("\n#[cfg(test)]"))`. A genuinely different shape:
  it bounds one **function body** (the next `\npub fn `, with the attribute as a
  fallback for the last function in the file), not a file's production region.
  Its failure mode is a window that runs long, not a scan that stops early.
  Left in place and named as the single exemption in the new rule, rather than
  migrated — that file carries eight `#[cfg(test)]` markers and rewiring its
  offset arithmetic is a separate piece of work.

## New row 7 — `alephcore`'s own guard 3 had the same enumeration bug. Verdict: CONNECT (the guard).

`no_module_hand_rolls_the_cfg_test_prefix_cut` searched for those same three
spellings, which is a list of the shapes that existed the day it was written.
Measured against the tree with the shape rule instead, **five live sites in
`src/` used a spelling none of the three could match**, all of them older than
Task 3:

| Site | Spelling |
|---|---|
| `src/gateway/btw/guard_tests.rs:154` | `const ATTR` + `find(ATTR)` |
| `src/gateway/continuation_lifecycle.rs:684` | same shape, an already-drifted copy of the one above |
| `src/gateway/execution_engine/run_loop/tests.rs:521` | `.split("#[cfg(test)]\nmod tests")` |
| `src/session/steer_signal.rs:534` | `.split("#[cfg(test)]\nmod ")` |
| `src/harness/tests/budget.rs:648` | `.position(\|l\| l.starts_with("#[cfg(test)]"))` |

Guard 3 reported **zero offenders** throughout, which is what a list of
spellings eventually reports. Its detector is now the shape rule. The five are
**registered, not migrated** — migrating each is a behaviour question rather
than a rename (two of them cut only at `#[cfg(test)] mod`, deliberately *not* at
a gated `use`; one drives the R10 harness line ratchet, whose number would move)
and Task 13's scope fence forbids re-opening Task 3's fix rounds. They live in
`KNOWN_UNMIGRATED_CUTS` with a reason each, and the assertion pins the list's
size so it can only shrink deliberately: a sixth site fails as an offender, and
deleting a row without migrating fails too.

Guard 3's own doc previously opened *"The rule, not an exemption list"* while
the code was three literals. That sentence is corrected in place.

## Row 8 — `providers::route_handle::GLOBAL` (D4). Verdict: stays raw, ruling in the code.

The ruling now lives on the `static` itself: first-caller-wins over a `cfg`
parameter, `CapabilitySlot::install` takes a value rather than a thunk, so
migrating would either reshape `Outcome` or move the computation before the race
is decided — and that second one changes which caller's config wins, on the path
every request's failover walk reads. It also tells a reader not to take
`1 raw, 45 slots` for unfinished work.

`capability::census`'s Guard A doc and `capability::ALL_SLOTS`'s doc now point
at the static instead of restating the reasoning, and `ALL_SLOTS`'s stale line
("Task 13 adjudicates it, not this list") is replaced by the adjudication.
Nothing about the exemption's enforcement moved: the named predicate at
`census.rs:782` is unchanged, and the `exempted.len() == 1` assertion that
forbids widening it is untouched.
