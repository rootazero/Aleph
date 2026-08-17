# Severed-Wire Audit — `src/markdown`

- Audit: `severed-wire-audit` / 2026-08-17
- Module: `src/markdown` (2 files, ~589 LOC)
- Method: PRODUCED–CONSUMED symbol parity (`rg` across `src/`, `bin/` (=`src/bin/`), `interfaces/`, `shared/`), read-before-write triage
- Read-only review; no source files modified, no cargo runs.

## Files scanned

- `src/markdown/mod.rs` (11 lines)
- `src/markdown/fences.rs` (~589 lines, incl. ~34 tests)

## Module wiring summary (verified, NOT severed)

The module is fully wired into the gateway's canonical message splitter — it was **not** superseded by another markdown implementation:

```
MessageFormatter::split            src/gateway/formatter/mod.rs:109 (split_message(text, max_len) at :113)
  └─ split_message (canonical)     src/gateway/formatter/splitting.rs:22
       ├─ parse_fence_spans        splitting.rs:38
       ├─ get_fence_split          splitting.rs:73  → FenceSplit fields close_line/reopen_line at :81, :86
       ├─ find_fence_at            splitting.rs:115 (fence.start() at :116-117)
       ├─ is_safe_fence_break      splitting.rs:141
       └─ FenceSpan                splitting.rs:11 (import), :110, :138 (param types)
            ├─ FenceSpan::start()  splitting.rs:46
            ├─ FenceSpan::end()    splitting.rs:46
            ├─ FenceSpan::close_line() splitting.rs:47
            └─ FenceSpan::contains()  internal: fences.rs:228 (is_safe_fence_break), :234 (find_fence_at)
```

`MessageFormatter::split` consumers (production, all outbound paths): `src/gateway/interfaces/irc/message_ops.rs:170`, `src/gateway/interfaces/mattermost/message_ops.rs:101`, `src/gateway/interfaces/signal/message_ops.rs:225`, `src/gateway/interfaces/slack/message_ops/api.rs:94`, `src/gateway/reply_emitter/emitter/helpers.rs:388` (ReplyEmitter::split_message, called from helpers.rs:322).

No superseding implementation exists for this concern. Other markdown code is distinct:
- `src/tool_output/fence.rs` — rewriting untrusted fenced payloads (web_fetch/browser/MCP) via `security::content_sanitizer::split_external_fence`; sanitization concern, does not import `crate::markdown`.
- `src/export/markdown.rs`, `src/gateway/formatter/markdown_to_platform.rs`, `interfaces/{tui,cli,webchat}/.../markdown*` — HTML rendering / platform-format conversion, not fence-aware byte chunking.
- `splitting.rs:7-10` explicitly documents `crate::markdown::fences` as the single canonical parser.

No `#[allow(dead_code)]` and no `#[deprecated]` items in the module (rg: 0 hits).

---

## Findings

### sw-md-1 — Dead re-export at crate-root path `crate::markdown::<sym>` — CUT

**Form 1** (visible symbol with zero production consumers; also orphaned pub API per form 6 — re-exported but unused). **Severity: low.**

- **Produced:** `pub use fences::{find_fence_at, get_fence_split, is_safe_fence_break, parse_fence_spans, FenceSpan, FenceSplit};` — `src/markdown/mod.rs:8-10`
- **Consumers:** none found. Every real consumer imports via the `fences` path:
  - `src/gateway/formatter/splitting.rs:10` → `use crate::markdown::fences::{find_fence_at, get_fence_split, is_safe_fence_break, parse_fence_spans, FenceSpan};`
  - doctest `src/markdown/fences.rs:133` → `use alephcore::markdown::fences::parse_fence_spans;`

**`rg` evidence:**

```
$ rg -n "markdown::(parse_fence_spans|is_safe_fence_break|find_fence_at|get_fence_split|FenceSpan|FenceSplit)" src/ bin/ interfaces/ shared/
(0 matches)
```

Catch-all sweep of every `markdown::` reference repo-wide (`rg -n "markdown::" src/ bin/ interfaces/ shared/`): all 14 hits are either the `crate::markdown::fences::` path (splitting.rs:7,10,109; fences.rs:133 doctest) or *other modules/crates' own* markdown submodules (`src/export/…::markdown`, `src/gateway/interfaces/wechat/outbound::markdown`, `interfaces/tui::tui::markdown`, `interfaces/cli::output::markdown`, `interfaces/webchat::components::markdown`). **No code references the `crate::markdown::<symbol>` re-export path.**

- **Rationale:** the re-export block creates a second, unused path for the same six items. The module-level `pub` (lib.rs:91) is exercised only by the doctest at fences.rs:133 (test consumer) — the `pub use` itself has zero consumers, production or test.
- **Proposed change:** delete `src/markdown/mod.rs:8-10` (the `pub use` block). Keep `pub mod fences;` (mod.rs:6) — it is the path everything uses. `FenceSplit` remains reachable at `crate::markdown::fences::FenceSplit` (it is not even *named* in splitting.rs's import list; its fields are read via `get_fence_split`'s return value at splitting.rs:81,86).
- **Risk:** none. No production or test code resolves any of the six symbols through the re-export path; removal cannot change runtime behavior or break compilation. Optional follow-up (do NOT bundle here): if API minimization is desired, `pub mod markdown;` at `src/lib.rs:91` could become `mod markdown;`, but that would break the public doctest example at fences.rs:133 — leave pub.
- **Verification:** re-run the two `rg` commands above (still 0) + `cargo check -p alephcore` / `cargo test -p alephcore --lib markdown` (fixer side).

---

### sw-md-2 — `FenceSpan::marker()` / `indent()` / `language()` accessors: consumed only by tests — DECIDE

**Form 4** (produced but consumed only by tests). **Severity: low.**

- **Produced:**
  - `FenceSpan::marker(&self) -> &str` — `src/markdown/fences.rs:58-60`
  - `FenceSpan::indent(&self) -> &str` — `src/markdown/fences.rs:64-66`
  - `FenceSpan::language(&self) -> Option<&str>` — `src/markdown/fences.rs:70-72`
- **Consumers:** only `#[cfg(test)]` tests inside `src/markdown/fences.rs` — `marker()`: :260, :271, :417; `indent()`: :261, :280; `language()`: :259, :270, :281, :299, :300, :309, :353, :441, :452, :541, :549, :573 (plus the doctest example at :138).

**`rg` evidence:**

```
$ rg -n "\.marker\(\)" src/ bin/ interfaces/ shared/   # outside src/markdown/: only
src/builtin_tools/web_fetch/mod.rs:643: let marker_text = &out.content[..marker_end];   # local String var, NOT FenceSpan

$ rg -n "\.indent\(\)" src/ bin/ interfaces/ shared/    # outside src/markdown/: 0 hits

$ rg -n "\.language\(\)" src/ bin/ interfaces/ shared/  # outside src/markdown/: all hits are config/media language
                                                         # fields (config/patcher.rs, media/, shared/protocol/...),
                                                         # none on FenceSpan

$ rg -n "marker|indent|language" src/gateway/formatter/splitting.rs
# only doc comments (:7, :8, :20) — no accessor calls. The production consumer uses
# start()/end() (splitting.rs:46, :116-117), close_line() (:47) and contains() (internal, fences.rs:228, :234).
```

- **Rationale:** the three accessors are the only public read path for the `pub(crate)` fields `marker`/`indent`/`language` on a `pub` struct in the `alephcore` lib crate (which has external dependents: `shared/client`, `interfaces/cli`, `interfaces/tui` per their Cargo.tomls — none currently use `alephcore::markdown`). Cutting them is safe *in-crate* but shrinks public API; keeping them is harmless, documented surface consistent with `start()`/`end()` (which ARE used in production). Genuine judgment call — do not delete without sign-off.
- **Options:**
  1. **Keep as-is (recommended):** cheap, documented accessors on a public struct; they double as the test read path. No diff.
  2. **CUT all three** (`fences.rs:58-72`, keeping `start()`/`end()`), and optionally downgrade the fields `marker`/`indent`/`language` from `pub(crate)` to private (`fences.rs:25,27,29`) — then rewrite the affected test assertions to use field access via `super::*`. Safe in-crate, but removes public read access for any future external consumer.
- **Risk:** option 2 is behavior-neutral today (no production consumer), but it is a public-API removal in a lib crate — hence DECIDE, not CUT.
- **Verification:** `cargo test -p alephcore --lib markdown` after any change; re-run the accessor `rg` sweeps.

---

## Checked and clean (no finding)

- **Form 2 (unwired stubs):** none. Every public fn is reachable from `splitting.rs` or the internal helpers; no skeleton code.
- **Form 3 (stale references / renamed symbols):** none. All names imported at `splitting.rs:10-11` exist; no dead-path references to old names found by the catch-all `markdown::` sweep.
- **Form 5 (name-drift):** none. Doc comments (`mod.rs:1-4`, `fences.rs:82-89` caller contract, `splitting.rs:4-8`) describe reality; no constants point at stale formats/paths.
- **Form 6 (`#[allow(dead_code)]` / `#[deprecated]`):** none in module.
- **Private items:** `FENCE_REGEX` (fences.rs:16), `OpenFence` (fences.rs:34), `FenceSpan.info` (fences.rs:30) — all consumed internally (regex/state used by `parse_fence_spans`; `info` read by `reopen_line()` at :105). Fine.
- **`FenceSplit`** (fences.rs:114-119): wired — returned by `get_fence_split` and its `pub` fields read at splitting.rs:81,86.

## Skipped (deliberately)

- No `cargo check/test` runs (protocol constraint).
- `graphify-out/graph.json` / `GRAPH_REPORT.md` not used — protocol flags them stale; `rg` parity was decisive here.
- Full read of `src/export/markdown.rs`, `src/gateway/formatter/markdown_to_platform.rs`, `interfaces/{tui,cli,webchat}` markdown renderers — confirmed via headers + import paths to be rendering/format-conversion with no fence-chunking overlap; out of scope for this module's parity.

## Totals

| Severity | Count |
|----------|-------|
| critical | 0 |
| high     | 0 |
| medium   | 0 |
| low      | 2 |

| Decision | Count |
|----------|-------|
| CUT      | 1 (sw-md-1) |
| CONNECT  | 0 |
| DECIDE   | 1 (sw-md-2) |
