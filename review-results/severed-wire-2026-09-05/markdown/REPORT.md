# Severed-Wire Audit — `src/markdown`

- Audit: `severed-wire-audit` / 2026-09-05
- Module: `src/markdown` (2 files, ~580 LOC)
- Method: PRODUCED–CONSUMED symbol parity (`rg` across `src/`, `bin/` (=`src/bin/`), `interfaces/`, `shared/`, `desktop/`), read-before-write triage. Prior `2026-08-17` audit consulted as historical context; **all claims re-verified with fresh `rg`**.
- Read-only review; no source files modified, no cargo runs.

## Files scanned

- `src/markdown/mod.rs` (6 lines)
- `src/markdown/fences.rs` (~577 lines, incl. ~34 `#[cfg(test)]` tests)

## Module wiring summary (verified, NOT severed)

The module is fully wired into the gateway's canonical message splitter — the wire introduced when this module was added is intact, the prior `sw-md-1` CUT has been applied (commit `e5f90f069`, 2026-08-17, removed the dead `pub use fences::{…}` re-export block from `mod.rs`):

```
MessageFormatter::split            src/gateway/formatter/mod.rs:109 (split_message(text, max_len) at :113)
  └─ split_message (canonical)    src/gateway/formatter/splitting.rs:22
       ├─ parse_fence_spans       splitting.rs:38
       ├─ get_fence_split         splitting.rs:73  → FenceSplit fields close_line/reopen_line at :81, :86
       ├─ find_fence_at           splitting.rs:115 (fence.start() at :116-117)
       ├─ is_safe_fence_break     splitting.rs:141
       └─ FenceSpan               splitting.rs:11 (import), :110, :138 (param types)
            ├─ FenceSpan::start() splitting.rs:46, :116, :117
            ├─ FenceSpan::end()   splitting.rs:46
            └─ FenceSpan::close_line() splitting.rs:47
            (FenceSpan::contains()/reopen_line() used internally only — see sw-md-3)
```

`MessageFormatter::split` consumers (production, all outbound paths; cross-checked with `rg -n 'MessageFormatter::split'`):

- `src/gateway/interfaces/irc/message_ops.rs:170`
- `src/gateway/interfaces/mattermost/message_ops.rs:101`
- `src/gateway/interfaces/signal/message_ops.rs:225`
- `src/gateway/interfaces/slack/message_ops/api.rs:105`
- `src/gateway/interfaces/matrix/outbound.rs:39, :80, :267`
- `src/gateway/interfaces/xmpp/message_ops/ops.rs:133`
- `src/gateway/event_emitter/origin_fanout.rs:106`
- `src/gateway/reply_emitter/emitter/helpers.rs:571` (via `ReplyEmitter::split_message`)
- Tests: `src/gateway/formatter/tests.rs:402, :409, :423, :444, :457, :485`

No superseding implementation exists for this concern. Other markdown code is distinct:

- `src/tool_output/fence.rs` — rewriting untrusted fenced payloads (web_fetch/browser/MCP) via `security::content_sanitizer::split_external_fence`; sanitization concern, does not import `crate::markdown`.
- `src/export/markdown.rs`, `src/gateway/formatter/markdown_to_platform.rs`, `interfaces/{tui,cli,webchat}/.../markdown*` — HTML rendering / platform-format conversion, not fence-aware byte chunking.
- `src/memory/assembler/rerank.rs:153 strip_json_fences` and `src/group_chat/coordinator.rs:197 strip_markdown_fences` — hand-rolled parallel strippers for LLM JSON output (see sw-md-4). Different ergonomic contract (`&str → &str`, no owned allocation), no dependency on `crate::markdown`.
- `splitting.rs:7-10` explicitly documents `crate::markdown::fences` as the single canonical parser.

No `#[allow(dead_code)]` and no `#[deprecated]` items in the module (`rg -n '#\[allow\(dead_code\)\]|#\[deprecated\]' src/markdown/` → 0 hits).

---

## Findings

### sw-md-1 — `FenceSpan::marker()` / `indent()` / `language()` accessors: consumed only by tests — DECIDE

**Form 4** (produced but consumed only by `#[cfg(test)]`). **Severity: low.**

Same shape as the prior review's `sw-md-2` (2026-08-17, archived DECIDE). Re-verified: no production caller materialized since then. The recommended posture from the prior audit is reproduced unchanged.

- **Produced:**
  - `FenceSpan::marker(&self) -> &str` — `src/markdown/fences.rs:56-58`
  - `FenceSpan::indent(&self) -> &str` — `src/markdown/fences.rs:62-64`
  - `FenceSpan::language(&self) -> Option<&str>` — `src/markdown/fences.rs:68-70`
- **Production consumers:** **0**. External-callable but unused by `splitting.rs` (which is the only `crate::markdown::fences` importer — see wiring summary).
- **Test consumers (only):** `src/markdown/fences.rs:136` (doctest), `:258, :259, :269, :278, :279, :297, :307, :415, :439, :450, :539, :547, :571` for `marker`/`indent`/`language` calls.

**`rg` evidence:**

```
$ rg -n '\.marker\(\)' src/ bin/ interfaces/ shared/ desktop/  # outside src/markdown/
src/builtin_tools/workflow_tool.rs:1007:                t.marker(),   # Talker/segment local — NOT FenceSpan
src/builtin_tools/web_fetch/mod.rs:643: let marker_text = &out.content[..marker_end];   # local String var — NOT FenceSpan

$ rg -n '\.indent\(\)' src/ bin/ interfaces/ shared/ desktop/   # outside src/markdown/: 0 hits

$ rg -n 'FenceSpan' src/gateway/formatter/splitting.rs           # only the import line (:11) and type-as-param (:110, :138)
$ rg -n 'marker|indent|language' src/gateway/formatter/splitting.rs   # only doc comments (:7, :8, :20) — no accessor calls
```

The production splitter uses `FenceSpan::start()` (`:46, :116, :117`), `FenceSpan::end()` (`:46`) and `FenceSpan::close_line()` (`:47`); `FenceSpan::contains()` and `FenceSpan::reopen_line()` are used only internally by sibling helpers — see sw-md-3.

- **Rationale:** three accessors on a public struct, zero production callers. Cheap to keep, cheap to remove. The alephcore lib has external dependents (`shared/client`, `interfaces/cli`, `interfaces/tui`, `interfaces/webchat` per their Cargo.tomls — none currently use `alephcore::markdown`), so any removal is a public-API change in a lib crate. The prior audit (2026-08-17) flagged this as DECIDE; no follow-up has occurred. Maintain that call.
- **Options:**
  1. **Keep as-is (recommended):** cheap, documented accessors; doubles as the test read path for `pub(crate)` fields. No diff.
  2. **CUT all three** (`fences.rs:56-70`, keeping `start()`/`end()`/`close_line()`), and optionally downgrade the fields `marker`/`indent`/`language` from `pub(crate)` to private (`fences.rs:22, :24, :26`) — then rewrite the affected test assertions to use field access via `super::*` (tests already have visibility). Behavior-neutral in-crate today; shaves public-API surface for lib consumers.
- **Risk:** option 2 is behavior-neutral today but is a public-API removal in a lib crate.
- **Verification:** re-run the accessor `rg` sweeps above (still 0 outside `src/markdown/`); `cargo test -p alephcore --lib markdown` after any change.

---

### sw-md-2 — `pub mod fences` re-export: verified wired, no finding

`pub mod fences;` at `src/markdown/mod.rs:6` is the canonical import path; the only external importer is `src/gateway/formatter/splitting.rs:10` (`use crate::markdown::fences::{…}`). The dead `pub use fences::{…}` block from the 2026-08-17 audit was removed in commit `e5f90f069` — no second pub path exists.

Catch-all sweep:

```
$ rg -n 'markdown::(parse_fence_spans|is_safe_fence_break|find_fence_at|get_fence_split|FenceSpan|FenceSplit)' src/ bin/ interfaces/ shared/ desktop/
(0 matches)   # every consumer goes through `markdown::fences::…`
```

No finding. Listed for completeness.

---

### sw-md-3 — `FenceSpan::contains()` / `FenceSpan::reopen_line()`: internal-only public — DECIDE

**Form 1** (pub method with no production caller; non-test internal use only). **Severity: low.**

- **Produced:**
  - `FenceSpan::contains(&self, index: usize) -> bool` — `src/markdown/fences.rs:78-91`
  - `FenceSpan::reopen_line(&self) -> String` — `src/markdown/fences.rs:94-103`
- **Internal callers (within `src/markdown/fences.rs`):**
  - `contains()` — `fences.rs:226` (`is_safe_fence_break`), `fences.rs:232` (`find_fence_at`), plus tests `:480-481, :491-492, :495-496, :499-500, :520-526, :573-574`
  - `reopen_line()` — `fences.rs:243` (`get_fence_split`), plus tests `:405`
- **Production (external) callers:** **0**. `src/gateway/formatter/splitting.rs` does not call either — it gets the same data through the `find_fence_at`/`get_fence_split` helpers.

**`rg` evidence:**

```
$ rg -n 'FenceSpan::contains|FenceSpan::reopen_line|\.contains\(\)|\.reopen_line\(\)' src/ bin/ interfaces/ shared/ desktop/   # outside src/markdown/: 0 hits
```

Note that `FenceSpan::contains` collides lexically with `String::contains` / `str::contains` / local `contains` closures elsewhere — the regex above returns many false positives from `str::contains` / `Vec::contains` calls; the only ones on `FenceSpan` are inside `src/markdown/fences.rs`.

- **Rationale:** both methods have legitimate intra-module callers, but every caller is in `fences.rs` itself; both are reachable as part of the `FenceSpan` public API. They are not "dead code" — they are public API surface used internally. A targeted refactor could downgrade them to `pub(crate)` (or even private) without behavior change, but since the struct is already `pub` and the rest of its methods are `pub`, keeping them `pub` for symmetry is reasonable.
- **Options:**
  1. **Keep as-is (recommended):** consistent with `start()`/`end()`/`close_line()` which are also `pub`; trivially justifiable for users who want to roll their own split logic.
  2. **Downgrade to `pub(crate)`** (`fences.rs:78-91, :94-103`): removes them from lib-crate public surface, callers in this module keep working. Behavior-neutral.
- **Risk:** option 2 is a public-API tightening; non-issue for the in-tree splitter. No callers elsewhere.
- **Verification:** re-run the `rg` sweeps above (still 0 outside `src/markdown/`).

---

### sw-md-4 — Parallel `strip_*_fences` helpers do not call `crate::markdown::fences::parse_fence_spans` — DECIDE

**Form 4** (parallel hand-rolled implementations of a concern the canonical parser already owns). **Severity: low.**

The canonical `crate::markdown::fences::parse_fence_spans` (`fences.rs:138`) is documented (`splitting.rs:7-10`) as "the single canonical parser". Two private helpers in the same codebase re-implement a stripped-down version for stripping ````json ... ```` wrappers from LLM JSON output:

- `src/group_chat/coordinator.rs:197-211 strip_markdown_fences(s: &str) -> &str`
  - Used by `parse_coordinator_plan` at `coordinator.rs:101`.
- `src/memory/assembler/rerank.rs:153-159 strip_json_fences(s: &str) -> &str`
  - Used by `parse_response` at `rerank.rs:100`.
  - Test coverage at `rerank.rs:253 markdown_fences_stripped`.

Both helpers share the same body shape (paraphrased):
```rust
fn strip_json_fences(s: &str) -> &str {
    let t = s.trim();
    let t = t.strip_prefix("```json").unwrap_or(t);
    let t = t.strip_prefix("```").unwrap_or(t);
    let t = t.strip_suffix("```").unwrap_or(t);
    t.trim()
}
```

They are **not** severed wires in the strict sense (the canonical parser *is* wired to `splitting.rs`; the helpers chose simpler heuristic), but they represent *parallel implementations of the same concern* that the canonical parser already covers. They diverge on:

- Indented fences (`  ```json … ````)
- Tilde fences (`~~~json … ~~~`)
- 4+ backtick fences (` ```` … `````)
- Whitespace between the language tag and the body
- Multiple concatenated fence pairs (helper leaves the second pair intact)
- Case-sensitivity of `json` (helper does not match ````JSON`)

In practice LLM output is overwhelmingly ````json{...}```` so the divergence has not produced observed bugs.

**`rg` evidence (absence of any canonical-parse call inside the helpers):**

```
$ rg -n 'parse_fence_spans' src/group_chat/ src/memory/assembler/rerank.rs   # 0 hits
$ rg -n 'crate::markdown' src/group_chat/ src/memory/assembler/rerank.rs      # 0 hits
$ rg -n 'strip_json_fences|strip_markdown_fences' src/ bin/ interfaces/ shared/ desktop/   # both helpers are file-private
src/group_chat/coordinator.rs:101:    let trimmed = strip_markdown_fences(raw.trim());
src/group_chat/coordinator.rs:197:fn strip_markdown_fences(s: &str) -> &str {
src/memory/assembler/rerank.rs:100:    let trimmed = strip_json_fences(raw);
src/memory/assembler/rerank.rs:153:fn strip_json_fences(s: &str) -> &str {
```

- **Rationale:** this is *not* a missing wire — it is a deliberate local simplification. The canonical parser returns owned `Vec<FenceSpan>` and is allocation-heavy; the helpers are zero-borrow, return `&str`. For the JSON-stripping use case the helper is simpler and faster. On the other hand, the canonical parser *does* already handle the edge cases, and using it would tighten the LLM-output tolerance without measurable overhead in those code paths (each function is called once per LLM JSON response).
- **Options:**
  1. **Keep as-is (recommended):** no observed production regression, ergonomic argument (zero-borrow) is sound.
  2. **CONNECT:** replace both helpers with a thin wrapper around `parse_fence_spans` (e.g. a new `pub fn first_fence_body(text: &str) -> Option<&str>` in `fences.rs`). Tighter parsing, but loses zero-borrow unless the wrapper returns `&str` sliced from input (requires `parse_fence_spans` to expose its lifetimes or compute spans lazily).
  3. **DECIDE→CONNECT:** add `fences::strip_outer_json_fence(text: &str) -> Option<&str>` to the canonical module and have both helpers delegate to it. Single source of truth, zero-borrow preserved.
- **Risk:** option 3 changes the behavior of both helpers in edge cases the canonical parser handles correctly. The change is strictly tightening — the canonical parser rejects more inputs than the current helpers (e.g. unclosed fences extend to end-of-text rather than yielding an unstripped prefix). Whether callers *want* that tighter behavior for LLM output is a product call.
- **Verification:** existing test coverage at `rerank.rs:253 markdown_fences_stripped` would need a sibling case for each divergence above if CONNECT is chosen; `coordinator.rs:255 test_parse_coordinator_plan_with_markdown_wrapper` already covers the happy path.

---

## Checked and clean (no finding)

- **Form 2 (unwired stubs):** none. Every public fn is reachable from `splitting.rs` or its internal helpers; no skeleton code.
- **Form 3 (stale references / renamed symbols):** none. All names imported at `splitting.rs:10-11` exist; no dead-path references to old names found by the catch-all `markdown::` sweep.
- **Form 5 (name-drift):** none. Doc comments (`mod.rs:1-4`, `fences.rs:80-89` caller contract, `splitting.rs:4-8`) describe reality; no constants point at stale formats/paths.
- **Form 6 (`#[allow(dead_code)]` / `#[deprecated]`):** none in module.
- **Private items:** `FENCE_REGEX` (fences.rs:11), `OpenFence` (fences.rs:33), `FenceSpan.info` (fences.rs:30) — all consumed internally (regex/state used by `parse_fence_spans`; `info` read by `reopen_line()` at :103). Fine.
- **`FenceSplit`** (fences.rs:112-119): wired — returned by `get_fence_split` and its `pub` fields read at splitting.rs:81, :86.
- **Module-level `pub mod fences`** (mod.rs:6): wired — sole importer is `splitting.rs:10`. Doctest at `fences.rs:131-138` uses the `alephcore::markdown::fences::` path through crate-root re-export (`pub mod markdown;` at `src/lib.rs`).
- **Prior audit findings (2026-08-17):**
  - `sw-md-1` (dead `pub use` re-export) — applied (commit `e5f90f069`). No second `pub` path exists.
  - `sw-md-2` (test-only `marker()`/`indent()`/`language()`) — re-classified here as `sw-md-1` (DECIDE, unchanged recommendation).

## Skipped (deliberately)

- No `cargo check/test` runs (protocol constraint).
- `graphify-out/graph.json` / `GRAPH_REPORT.md` consulted for community membership sanity-check; **every claim above re-verified with fresh `rg`** (protocol flag — graph.json may be stale).
- Full read of `src/export/markdown.rs`, `src/gateway/formatter/markdown_to_platform.rs`, `interfaces/{tui,cli,webchat}` markdown renderers — confirmed via headers + import paths to be rendering/format-conversion with no fence-chunking overlap; out of scope for this module's parity.
- `split_external_fence` in `src/security/content_sanitizer.rs` and `wrap_external_content` in `src/builtin_tools/browser_tools/mod.rs` — confirmed via `rg` to be sanitization/injection-defense concerns, not fence chunking. Not in scope for `crate::markdown::fences`.
- Memory-slot "fences" (`<memory>` / `<slot>` XML tags) in `src/memory/streaming_scrubber.rs`, `src/memory/assembler/render.rs`, `src/memory/dreaming/stages/*.rs` — different domain (custom memory markup), unrelated to ``` markdown code fences. Not in scope.

## Totals

| Severity | Count |
|----------|-------|
| critical | 0 |
| high     | 0 |
| medium   | 0 |
| low      | 3 |

| Decision | Count |
|----------|-------|
| CUT      | 0 |
| CONNECT  | 0 |
| DECIDE   | 3 (sw-md-1, sw-md-3, sw-md-4) |
| verified-clean | 1 (sw-md-2) |

Net delta since the prior 2026-08-17 audit on this module: one less DECIDE (`sw-md-1` CUT executed; current `sw-md-1` = prior `sw-md-2` re-flagged with refreshed evidence). Two new DECIDE observations (`sw-md-3` internal-only pub methods; `sw-md-4` parallel strippers) — neither rises above `low` and neither blocks shipping.