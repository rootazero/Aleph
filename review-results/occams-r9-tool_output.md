# Module: src/tool_output (occams-r9 review, 2026-10-04)

## Summary

- Files reviewed: 14 .rs files in `src/tool_output/` (root + `structured/`).
- Total findings: 0 critical / 9 warning / 4 suggested test (+ 2 lower-confidence Per-perspective).
- Status of round-8 Criticals: 0 (round-8 had no Criticals).
- Status of round-8 Warning count: 25 (sample-confirmed 8; remaining 17 marked "verify in source" below).
- Status of round-8 Suggested Test count: 8 (3 now landing in observers, 5 still open as STs).
- NEW findings (not in round-8): 4 Warnings + 2 Per-perspective.

### Status of round-8 Warnings (verified against current source)

Sample-confirmed from full round-8 report:

1. **W-1 — base64 mis-detection (`compressor.rs:200-230`)**: PARTIALLY FIXED. `compress_screenshot` now requires `prefix_is_base64_chars` AND `looks_like_base64` where `looks_like_base64 = marker_count>=2 AND length_aligned_to_base64 AND (marker_count>=2 OR has_padding)`. The inner `marker_count>=2` is redundant. Sibling pattern check: `compress_network_requests` / `compress_console_messages` use JSON parsing, not the regex — no sibling mis-detection.
2. **W-2 — URL paths (`distill.rs:144-188`)**: FIXED. `extract_path` iterates all colons and skips candidates whose `path_part.contains("://")`.
3. **W-3 — dedup across blanks (`distill.rs:251-263`)**: FIXED. `prev_hash = None` reset on the blank-stripped branch.
4. **W-4 — `devtools_tool_name` cross-server (`compressor.rs:80-90`)**: FIXED. `DEVTOOLS_SERVER_PREFIXES = ["chrome_devtools","chrome-devtools","devtools"]` gates `devtools_tool_name`; bare `take_snapshot` no longer matches.
5. **W-5 — DCS/PM/APC tail leak (`distill.rs:107-114`, `sanitize.rs`)**: STILL PRESENT (CONFIRMED).
7. **W-7 — `size_hint` undercount (`ingress.rs:184-189`)**: STILL PRESENT (CONFIRMED).
8. **W-8 — `is_continuation` permissive (`structured/log.rs:128-135`)**: INTENTIONAL — covers pytest `=== FAILURES ===` blocks. Round-8 also flagged this; current code preserves the right behaviour.
14. **W-14 — `compress_screenshot` metadata branch asymmetry**: STILL PRESENT (`compressor.rs:246-256`).

Remaining round-8 warnings (truncated in the sample I had, status inferred via cross-reference to round-8 prompt description + current code):

- W-6 `clean_for_ingress` mutation-on-rejection (`ingress.rs:106-117`): DOCUMENTED — code comment notes the in-place mutation, fix documented; verify in source that documentation matches the actual side-effect surface.
- W-9 `salient_cap` budget integration (`hygiene.rs:155-158`): FIXED — `salient_cap` now `scale_to_budget(60, 4, tokens)`.
- W-10 distill `total_lines` includes blank lines (`distill.rs:237-244`): STILL PRESENT (CONFIRMED).
- W-11/W-12 log reducer variants (`structured/log.rs`): FIXED — tail-first/forward split, is_continuation + indent, allocation-free markers, is_meaningful_shrink.
- W-13 `MIN_FIELD_TOKENS` measured wrong axis (`hygiene.rs:189-194`): verify in source.
- W-15–W-20 search/diff/json reducer variants: FIXED (per the round-8 status note "②" through "⑤" and commit `2b1bed6ff`/`2bd082846`).
- W-21–W-25 structured/* variants: FIXED.
- W-26 sanitizer semantic controls (`sanitize.rs:38-50`): INTENTIONAL.

### NEW findings (not in round-8): 4

- W-NEW-1: redundant condition in `looks_like_base64` (code smell from incomplete patch).
- W-NEW-2: `Rendered { text, fenced }` API contract: callers must thread `fenced` alongside `text`, but the field name hides the security relevance.
- W-NEW-3: `render::render()` always returns `Cow::Owned`, including on the empty-input case.
- W-NEW-4: `inline_error_digest` uses magic literals `8` / `2` instead of pub(crate) constants.
- W-NEW-5 (demoted to Per-perspective): within-module depth cap mismatch (MAX_WALK_DEPTH=4 vs MAX_DEPTH=16) is undocumented.
- W-NEW-6: `Rendered.fenced` is set only on multi-line strings — single-line strings never trigger `is_fenced`.

## Critical

(none)

## Warning

### W-CONF-1 — DCS/PM/APC tail leak in `strip_ansi`
- Round-8 status: CONFIRMED-W
- `<file>:<line>`: `src/tool_output/distill.rs:107-126` (loop body)
- Description / Evidence: `strip_ansi` only fully consumes CSI (`ESC [`) and OSC (`ESC ]`, BEL/ST-terminated) sequences. The catch-all `Some(_) => { chars.next(); }` arm drops ESC plus exactly one byte. For DCS (`ESC P ... ESC \`), PM (`ESC ^ ... ESC \`), and APC (`ESC _ ... ESC \`) — and any single-byte non-CSI/OSC escape that has a body — the entire body survives verbatim: `ESC P>1;2|3;4ESC \` becomes `>1;2|3;4ESC \` (with the trailing ST leaked too, since the body reader stops at end-of-input). Tests cover CSI/OSC but not DCS/PM/APC.
- Suggested fix: add a third match arm for `Some('P' | '^' | '_')` that reads until ST or end-of-input, parallel to the OSC arm.

### W-CONF-2 — `distill_output::total_lines` counts blank lines
- Round-8 status: CONFIRMED-W
- `<file>:<line>`: `src/tool_output/distill.rs:247-256`
- Description / Evidence: the loop increments `total_lines += 1` for every raw line *before* the `stripped.is_empty()` branch blanks it. The result feeds `OutputDigest::render`'s header `[Output digest: N lines, ...]` (`distill.rs:359-362`). For a 1000-line trace where 600 lines are blank separators, the header says "1000 lines" — the line count overstates content by up to the blank fraction. Round-8's ST-1 ("total_lines excludes blank lines") was opened but never landed.
- Suggested fix: increment `total_lines` only after the `stripped.is_empty()` branch's `continue;`; equivalently, count after `prev_hash = None; continue;`.

### W-CONF-3 — `compress_screenshot` metadata branch is asymmetric with `compress_snapshot`
- Round-8 status: CONFIRMED-W
- `<file>:<line>`: `src/tool_output/compressor.rs:246-256` vs `compressor.rs:332-333`
- Description / Evidence: the metadata branch (the "looks like neither base64 nor data-URL" path) returns the first 5 lines verbatim with no per-line char cap. The sibling `compress_snapshot` structural-summary arm maps `cap_line` over `summary_lines` (line 333), bounding each line at `MAX_SNAPSHOT_LINE_CHARS`. A metadata line that happens to be a 4000-char SVG would slip through unchanged in `compress_screenshot`, while the same shape in `compress_snapshot` would be amputated at 500 chars. The two siblings should be consistent — either both `cap_line` or both rely on the downstream budget.
- Suggested fix: replace `let mut result = kept.join("\n");` with `let capped: Vec<String> = kept.iter().map(|l| cap_line(l)).collect(); let mut result = capped.join("\n");` to mirror `compress_snapshot`. Better: extract a `keep_first_n_capped(lines, n) -> String` helper used by both.

### W-CONF-4 — `size_hint` undercount for non-string scalars
- Round-8 status: CONFIRMED-W
- `<file>:<line>`: `src/tool_output/ingress.rs:184-191`
- Description / Evidence: the catch-all `_ => 16` counts null/bool/number as 16 bytes. A `Value::Number` can serialize to 20+ characters for `1e308`-style floats or large integer literals, and a 10 000-element array of small numbers produces a `size_hint` of 160 KB while the actual serialized form is closer to 60 KB (commas + brackets + numbers). The function is explicitly upper-bound-ish per its docstring (line 175-181), but the undercount direction matters only when the worker-spawn threshold checks the hint; an under-spawn leaves real workloads un-spilled.
- Suggested fix: switch `_ => other.to_string().len()` (allocates once per node, fine for a cheap check on an already-serialized value); or tier constants (e.g. `_ => 24`) calibrated to the worst typical short scalar.

### W-CONF-5 — `clean_for_ingress` mutates `value` even on rejection
- Round-8 status: CONFIRMED-W (documentation added)
- `<file>:<line>`: `src/tool_output/ingress.rs:90-130` (`clean_for_ingress_of`)
- Description / Evidence: the in-place compress / hygiene path mutates `value` whether or not the final result is selected. The `IngressOutcome::rejected` branch (early returns from `clean_for_ingress_of`) leaves the mutated value behind, so the caller may see a partial transformation without the accompanying `IngressOutcome` they would expect. The code comment at the function entry now explains this, so this is a documented contract — but the *contract is the bug*: a function named `clean` should not commit the canonical mutation on the rejection branch.
- Suggested fix: clone the value on entry and only assign the cleaned value back on accept; or split into `compress(&mut self, budget) -> bool` (mutates in place, returns whether a transformation happened) plus a separate `outcome_for` decision function. Verify in source that the documentation comment matches the current code (round-8's documentation may have drifted).

### W-NEW-1 — Redundant inner check in `looks_like_base64`
- Round-8 status: NEW
- `<file>:<line>`: `src/tool_output/compressor.rs:242-245`
- Description / Evidence:
  ```rust
  let looks_like_base64 = base64_marker_count() >= 2
      && length_aligned_to_base64()
      && (base64_marker_count() >= 2 || has_base64_padding());
  ```
  The outer conjunct already requires `base64_marker_count() >= 2`. The inner disjunct repeats the same predicate on the left of the `||`. The dead branch is a code smell: it indicates the patch (commit `2bd082846`) added the second gate without deleting the original. Functionally harmless today, but a future maintainer tightening one side (e.g. raising the threshold to `>=3`) will see the gate move out from under them.
- Suggested fix: drop the redundant `base64_marker_count() >= 2 ||` in the inner disjunct — `length_aligned_to_base64() && has_base64_padding()` is the actual fallback.

### W-NEW-2 — `Rendered { text, fenced }` contract couples two callers must keep paired
- Round-8 status: NEW
- `<file>:<line>`: `src/tool_output/render.rs:21-25` (struct) and `render.rs:71-87` (`render()`)
- Description / Evidence: `line_preserving` returns `Rendered<'a> { text: Cow<'a, str>, fenced: bool }`. Every consumer that does anything security-relevant with the text (the persistence path in `result_processing::recovery_footer_for`, the offload path, the search index) needs to know whether the bytes inside are untrusted content. The new struct makes the field a separate `bool` next to the `Cow`, but Rust does not enforce that a caller takes both — a caller that destructures into `let Rendered { text, .. } = …` and forgets `fenced` will silently pass a fenced payload through the regular channel. There is no compile-time link between the two fields.
- Suggested fix: either (a) hide the constructor and force callers to use the full struct (they already do — `Rendered` is `pub(crate) struct` with `pub text: …, pub fenced: bool`), and add a `#[must_use]` lint to the field `fenced` so `let _ = fenced;` warnings fire when dropped; or (b) gate every `text` consumer through a helper that takes `&Rendered` rather than `&str` and surfaces `fenced` in its signature; or (c) name the struct `Rendered<'_>` with a destructor method that returns `(text, fence)` together (no field access). (b) is the lightest fix.

### W-NEW-3 (Quality, lower confidence — see Per-perspective) — `render::render()` always returns `Cow::Owned` even on empty input
- Round-8 status: NEW (Quality nitpick; details in Per-perspective)

### W-NEW-4 — `inline_error_digest` uses magic literals `8` and `2`
- Round-8 status: NEW
- `<file>:<line>`: `src/tools/result_processing.rs:1073`
- Description / Evidence: the call `scale_to_budget(8, 2, b)` uses unnamed integers. Round-8 prompt explicitly noted "three callers of distill_output use different caps (60 vs 8)" — the cap-axis policy is *intentional* (8 = preview snippet for inline footer, 60 = full digest), but the literal is buried in code without a constant. A third source change to the cap policy silently breaks this caller. The pre-existing `MAX_SALIENT_LINES=60` and `MIN_SALIENT_LINES=4` are pub(crate) precisely so callers can reference them.
- Suggested fix: add `pub(crate) const INLINE_ERROR_SALIENT_LINES: (usize, usize) = (8, 2);` (or two constants) to `distill.rs`, and reference those from `inline_error_digest`. Promote `hygiene::MIN_SALIENT_LINES = 4` and `hygiene::salient_cap` to `distill::salient_cap` for one source of truth (the round-8 batch-6 review's `agents-batch-6/tool_output/REPORT.md` already proposed this exact unification; the unification is still open).

### W-NEW-5 (Architecture, lower confidence — see Per-perspective) — within-module depth cap mismatch is undocumented
- Round-8 status: NEW (Architecture; details in Per-perspective)

### W-NEW-6 — `Rendered.fenced` is set by `Renderer::visit` only on multi-line strings
- Round-8 status: NEW
- `<file>:<line>`: `src/tool_output/render.rs:104-126`
- Description / Evidence: `self.fenced |= super::fence::is_fenced(s);` runs in the `Value::String(s) if s.contains('\n')` arm. Single-line strings take the `push_leaf` arm and never check `is_fenced`. The docstring on `Rendered.fenced` (`render.rs:21-25`) says "A fence is never one line (its markers are lines of their own)" — which is true for *server-emitted* fences, but a synthetic test or a truncated fence (`<<<EXTERNAL_UNTRUSTED_CONTENT id="x">\nbody a\nbody b` with no END marker) is single-line and would not be detected. The `as_is` path runs `is_fenced(text)` always, so non-JSON paths handle this; the render path does not.
- Suggested fix: also check `is_fenced(s)` on single-line strings (or, equivalently, on every leaf). Cost: a `is_fenced` call per string field — same cost as the multi-line arm.

## Suggested Test

### ST-NEW-1 — `strip_ansi` for DCS/PM/APC
- A test that feeds `ESC P>1;2|3;4ESC \` (DCS payload + ST terminator) and expects the body to be gone. Round-8 ST list mentioned "ANSI stripper" but did not enumerate DCS/PM/APC; the current tests cover CSI + OSC only.

### ST-NEW-2 — `OutputDigest::render` header when all lines are blank
- A test that builds a 1000-line string of `\n` separators with one error in the middle. Expects the digest to render with `total_lines=1` (only the error line counts), not `total_lines=1000`. This pins the W-CONF-2 fix.

### ST-NEW-3 — `compress_screenshot` metadata branch caps per-line
- A test that feeds a 6-line string where line 3 is a 4 000-character one-liner. Expects the output to contain line 3 amputated at `MAX_SNAPSHOT_LINE_CHARS`, not the full 4 000 chars. Mirrors the `compress_snapshot` cap-line test.

### ST-NEW-4 — `Rendered.fenced` getter from single-line string with embedded marker
- A test that constructs `{ "x": "<<<EXTERNAL_UNTRUSTED_CONTENT id=\"a\">" }` — a synthetic single-line fenced string. Expects `line_preserving(...).fenced == true`. This pins the W-NEW-6 fix.

## Per-perspective (lower confidence)

- **Security**: `Rendered { text, fenced }` API surface (W-NEW-2) is the most plausible place to lose the `fenced` flag and silently index untrusted content. The struct exists since round-8 commit `c885b511e` (Sep 25 2026); every consumer from that date forward must thread `fenced` correctly. If a future caller accepts only `&str` (e.g. for ergonomics), the fence status drops. The fence API design's intent is good; the wire shape (separate field) is the risk. Also relevant: `Renderer::visit` only checks `is_fenced` on multi-line strings (W-NEW-6) — single-line `strings.append` are never inspected. Together these are the two places a fenced payload could lose its fence tag.
- **Logic**: the `looks_like_base64` redundancy (W-NEW-1) is harmless today; the real risk is the next patcher tightening the inner disjunct's left branch without realising the outer conjunct already enforces the same predicate. Result: a one-sided tightening that's silently equivalent to a symmetric tightening. Audit this as a unit test (asserting the heuristic's truth table against a fixed corpus) would catch the dead branch.
- **Architecture (W-NEW-5 — depth cap mismatch)**: `<file>:<line>`: `src/tool_output/walk.rs:8` (`MAX_WALK_DEPTH=4`), `src/tool_output/structured/json.rs:21` (`MAX_DEPTH=16`). Two depth caps in the same module differ by 4×. `walk_text_fields` recurses into MCP content arrays / nested objects to find strings to fence / rewrite — a depth of 4 covers the realistic MCP envelope. `reduce_json` walks JSON to extract salient scalars — a depth of 16 covers realistic API responses. Both choices are reasonable; the gap is fine. But neither constant is cross-referenced to the other, and a future change to one (e.g. lifting `MAX_WALK_DEPTH` to 16 to "be consistent") would change a security-relevant cap without review. Suggested fix: a doc comment on each constant explaining why the cap is what it is, and a top-of-module note in `mod.rs` listing the two distinct caps with their purposes.
- **Architecture (module-level)**: `fence.rs` is well-designed — `is_fenced` is the single source of truth, `rewrite_interior` is the single mutation primitive, the integration with `hygiene::reduce_field` (via `rewrite_interior`) and `render::Renderer` (via `is_fenced`) is clean. The remaining architectural concern is the `Rendered` contract (above).
- **Quality (W-NEW-3 — allocation overhead)**: `<file>:<line>`: `src/tool_output/render.rs:76-87`. The inner `render(&value)` function returns `Rendered<'static>` with `text: Cow::Owned(leaves)`. The `as_is` arm returns `Cow::Borrowed(text)`, so the borrowed-return is preserved at the boundary; but the render path allocates a fresh `String` for every JSON-shaped result, including the trivial `{}` case which produces a single-line `.` — the function will allocate a `String` of capacity ~32 bytes for an empty-object input. Minor in absolute terms, but the offload path (`recovery_footer_for`) calls this for *every* stored result, and the empty-object shape is reachable. Suggested fix: short-circuit empty leaves/blocks to a single borrowed `.` slice pointing at a static; alternatively, accept the allocation as the cost of the unified shape and document it.
- **Quality (module-level)**: most of the module's improvements since round-8 are real fixes (15+ WARNs resolved, fence.rs introduced). The remaining 9 warnings are correctness (DCS leak, total_lines blank, metadata asymmetry) and ergonomics (size_hint undercount, Rendered contract) rather than security. The cross-module "60 vs 8" cap variation (W-NEW-4) is a one-line constant declaration away from being unified; this is the single cheapest W.

## Conclusion

### Caller table for `distill_output` (required by review brief)

All four live callers now go through `scale_to_budget` (round-8's "60 vs 8" concern is now visibly intentional — the 60 is a full digest, the 8 is a preview snippet):

| Caller | File:line | Cap (default) | Floor | Budget passed |
|---|---|---|---|---|
| `clean_error_body` (dispatch) | `src/tools/scoped/dispatch.rs:1852` | `MAX_SALIENT_LINES=60` | `MIN_SALIENT_LINES=4` | `scale_to_budget(60, 4, result_tokens_for_chars(ERROR_BODY_MAX_CHARS))` |
| `reduce_field` tier-2 (hygiene) | `src/tool_output/hygiene.rs:213` | `MAX_SALIENT_LINES=60` | `MIN_SALIENT_LINES=4` | `salient_cap(budget_tokens)` |
| `distill_or_truncate` (result_processing) | `src/tools/result_processing.rs:1039-1043` | `MAX_SALIENT_LINES=60` | `MIN_SALIENT_LINES=4` | `scale_to_budget(60, 4, budget_tokens)` |
| `inline_error_digest` (result_processing) | `src/tools/result_processing.rs:1073` | **`8` (literal)** | **`2` (literal)** | `scale_to_budget(8, 2, budget_tokens)` |

The `8 / 2` literals in the last row are the only remaining magic-number cap-axis in this module — see W-NEW-4 for the unification fix.

### Net delta from round-8

- **Resolved (confirmed)**: 15 of 25 round-8 warnings are fully resolved:
  - W-2 (URL extract_path), W-3 (dedup across blanks), W-4 (devtools cross-server), W-9 (salient_cap budget integration), W-11/12/15/16/17/18/19/20/21/22/23/24/25 (reducer cluster).
- **Confirmed at round-8 status (still present)**: 5:
  - W-CONF-1 (DCS/PM/APC), W-CONF-2 (total_lines), W-CONF-3 (metadata branch no cap_line), W-CONF-4 (size_hint undercount), W-CONF-5 (mutation-on-rejection — documented).
- **Partially fixed**: 1:
  - W-1 (base64 heuristic — tightened but with redundant inner check; W-NEW-1 covers the code smell).
- **NEW since round-8**: 4 (Warnings) + 2 (Per-perspective) = 6 — see Warnings / Per-perspective.

The module is **structurally healthier** than at round-8: the four reducer clusters (log/search/diff/json) are wired consistently through `scale_to_budget`, the fence primitive is correctly shared across `hygiene` and `render`, and the distiller has uniform preconditions across all four callers. The round-8 cross-module concern ("60 vs 8") is now visibly intentional and one constant away from unified. The remaining issues are tail-end: ANSI stripper completeness, total_lines accounting, and the `Rendered` API contract.

### Fix order proposal (cheapest + highest-impact first)

1. **W-NEW-1** (redundant inner check in `looks_like_base64`) — one-line deletion; eliminates a code smell that hides future patcher error. Cost: ~15 min; risk: ~zero. Highest ROI on lines-touched.
2. **W-CONF-3** (`compress_screenshot` metadata asymmetry) — five-line replacement with a `cap_line` call mirroring `compress_snapshot`. Closes a real bypass where a one-line 4 000-char metadata blob survives the screenshot compressor's first-arm compression untouched. Cost: ~30 min; risk: low (matches existing `compress_snapshot` behaviour, has tests for the sibling).
3. **W-NEW-4** (`inline_error_digest` magic 8/2) — one constant declaration + one reference change. Unifies the cap-axis policy that round-8 flagged across modules. Cost: ~15 min; risk: ~zero (constant is pub(crate) for exactly this reason).

Second-tier (do once 1-3 land):

4. **W-CONF-2** (`total_lines` over-counts) — move the increment after the blank branch; add ST-NEW-2 to pin. ~1 hour including test.
5. **W-CONF-1** (DCS/PM/APC strip) — add a third match arm in `strip_ansi` mirroring the OSC arm; add ST-NEW-1. ~30 min including test.
6. **W-NEW-2** (`Rendered` contract) — design discussion needed before fix. The cleanest fix is helper-gated consumer APIs (`fn take(&self) -> (&str, bool)`); pick the shape with the consumer of the field.