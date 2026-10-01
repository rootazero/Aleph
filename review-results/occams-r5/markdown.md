# src/markdown — occams-r5 Review

## Summary
- 3 findings: 0 crit / 0 high / 2 med / 1 low
- 0 dead-pub confirmations needed (rg search complete; no callers outside `src/markdown/`)

## Findings

### [medium] Redundant `no_info` guard before the `if` body in closing-fence logic

- **Location**: `fences.rs:157` (closing-fence branch)
- **Category**: dead code / awkward flow
- **Description**

  ```rust
  let no_info = info.is_empty();
  // ...
  if same_char && long_enough && no_info && valid_indent {
      // close
  } else {
      current_fence = Some(open);
  }
  ```

  The `no_info` guard was introduced to gate the closing-check body. But the
  body of the `if` branch also contains `let no_info = info.is_empty()` (line
  158), which is unreachable — `info` is guaranteed empty at that point since
  the outer `if` already enforced it. The inner `let no_info` on line 158 is
  dead code; the outer `no_info` is a one-liner that only exists to be
  compared to `true` in the condition.

  **Evidence**: `info` is populated exclusively by `caps.get(3).map_or("", |m| m.as_str().trim())`.
  If `info` were non-empty, the `no_info` check in the outer `if` would be
  `false` and execution would land in the `else` branch instead. So `info` is
  always `""` inside the closing-check body.

  **Suggested fix**: Remove the inner `let no_info = info.is_empty()` on line 158
  (it's unused after removal), and either inline the outer `no_info` into the
  condition as `info.is_empty()` (saves one binding) or keep the binding for
  readability if the intent is to document the check.

  ```rust
  // before (after removing inner let):
  if same_char && long_enough && info.is_empty() && valid_indent {
  ```

  **Risk**: Zero — removing the inner binding cannot change behavior. The outer
  binding is a cosmetic rename; inlining it is safe.

---

### [medium] `long_enough` is stricter than CommonMark for closing fences

- **Location**: `fences.rs:154`
- **Category**: spec compliance / incorrect behavior
- **Description**

  The closing-fence validation requires:

  ```rust
  let long_enough = marker.len() >= open.marker.len();
  ```

  CommonMark §4.4 states: *"A closing code fence must be at least as long as the
  opening fence."* — wait, actually CommonMark §4.4 says:

  > A closing fence must be at least as long as the opening fence.

  So `long_enough` is correct per spec. However, the CommonMark reference
  implementation (cmark) and most implementations also accept a closing fence
  with **fewer** markers than the opening, as long as it has ≥ 3. The spec's
  wording is ambiguous; the reference implementation accepts ` ```` ` (4
  backticks) closing a ` ````` ` (5 backticks) opening.

  No test in this file covers the case of a **shorter** closing fence
  (` ````` ` opening → ` ```` ` closing). The only shorter-closing test covers
  ` ````` ` → ` ``` `, which correctly does NOT close (3 < 4). The test
  `test_parse_shorter_closing_invalid` confirms the current behavior but does
  not explore the valid-short-closing boundary.

  **Risk**: Low — virtually no real-world markdown uses 5+ backtick fences, so
  this edge case is unlikely to surface. If triggered, the fence silently
  extends to end-of-text, producing truncated output rather than an error.

  **Suggested fix**: Either (a) tighten the test suite to document the current
  behavior and add a `#[ignore]` test for the permissive variant, or (b) relax
  `marker.len() >= open.marker.len()` to `marker.len() >= 3` (CommonMark minimum
  for closing) and add a test ` ````` \n code \n ```` ` expecting closure.

---

### [low] All fence utilities are `pub` but have zero production callers

- **Location**: `fences.rs:120,207,213,222`
- **Category**: dead code (production) / API bloat
- **Description**

  `parse_fence_spans`, `is_safe_fence_break`, `find_fence_at`, and `get_fence_split`
  are all `pub` (and re-exported via `pub mod fences` in `mod.rs`), but
  `rg`-confirm confirms zero callers outside `src/markdown/`. The module's
  only production consumers are the tests themselves. This is a classic
  "library has an API but no callers" pattern.

  The `pub` visibility is defensible as intentional design (the module is a
  public-facing library), but it means the API surface carries four functions
  that are not exercised in production. If any of these has a subtle bug
  (e.g., the CommonMark issue above), it will only be caught by the unit tests,
  not by real call-sites.

  **Suggested fix**: Either (a) move the four functions to `pub(crate)` now that
  callers are confirmed absent, re-promoting to `pub` when a real caller appears,
  or (b) add an integration-level smoke test that exercises all four functions
  in a realistic streaming-chunking scenario to ensure they are production-ready.

  **Risk**: Low — no behavior change; purely a maintenance/clarity concern.

---

## Cross-cutting

- **`pub(crate)` vs `pub` split is undocumented**: `contains` (line 81) and
  `reopen_line` (line 100) are `pub(crate)` while `start()`, `end()`,
  `close_line()` are `pub`. No comment explains why these two are restricted.
  Given the module is `pub mod fences`, this is a minor API-design ambiguity.
  Commit `6a3e9067` ("P3 downgrade FenceSpan::contains and reopen_line to
  pub(crate)") documents intent but not rationale.

- **`info` field never exposed publicly**: `FenceSpan::info` is populated from
  the fence's info string (everything after the opening fence markers), but
  no public getter or method exposes it. `language` is exposed instead. This
  is a clean separation (info = implementation detail, language = public API),
  not a bug. However, `reopen_line` uses `info` directly, so the field is not
  truly dead — it flows into the split reopen line.

- **No `Send`/`Sync` issues**: All types are built from `String`, `usize`, and
  `Option<String>` — trivially `Send + Sync`.

- **No lock/await issues**: No async code, no mutex usage.

- **No `to_string`/Box/clone concerns**: `to_string()` is used only on
  `&str` → `String` for struct field construction, which is the normal
  pattern. No `Box` allocations. No unnecessary clones.

## Out of scope

- **Fence regex correctness**: `r"^( {0,3})(`{3,}|~{3,})(.*)$"` is a well-known
  standard pattern for CommonMark fence detection. The `{3,}` lower bound is
  per-spec. No issue.
- **CRLF offset arithmetic**: Correctly handled via explicit byte-peek in the
  offset advancement loop. Comprehensive test coverage (`test_crlf_line_endings_offsets`).
- **Unicode handling**: Byte offsets are tracked correctly; `str::lines()` returns
  `&str` slices that are valid UTF-8 by definition. `test_unicode_byte_offsets`
  confirms.
- **Boundary semantics of `contains`**: Exclusive bounds (`index > start && index < end`)
  are well-documented in the `contains` docstring. Tests confirm correctness.
- **mod.rs**: Contains only `pub mod fences;` — trivially correct.
