# Severed-Wire Audit — `src/tool_output`

- **Batch:** agents-batch-6
- **Module:** `src/tool_output` (13 files, 5,449 LOC)
- **Date:** 2026-08-16
- **Reviewer:** static (severed-wire-audit skill)
- **Totals:** 1 finding (0 critical / 0 high / 1 medium / 0 low) — 1 CONNECT, 0 CUT, 0 DECIDE

## Scope & method

Scanned all 13 `.rs` files under `src/tool_output/` (including `structured/`) with the
seven seam lenses (registration, call-vs-handler, classifier-vs-handler, emit-vs-subscribe,
config-reader, path/route, stub sweep). Every candidate was triaged read-first by grepping
the consumer side for a live caller before any CONNECT/CUT/DECIDE verdict.

### What is clean (verified, not assumed)

- **No dead scaffolding to CUT.** Every production `pub`/`pub(crate)` producer — `scale_to_budget`,
  `compress_result_value`, `distill_output`, `OutputDigest::render`, `strip_ansi`,
  `sanitize_command_output`, `clean_result_value`, `clean_for_ingress`, `size_hint`,
  `reduce_within`, `Profile::for_token_budget`, `render_selected`, `contains_ignore_ascii_case`,
  `rewrite_interior`, `walk_text_fields`, etc. — was grepped to a live caller
  (`src/tools/scoped/dispatch.rs`, `src/tools/result_processing.rs`,
  `src/context/budget/cheap_passes/tool_result_pruning.rs`, `src/builtin_tools/{code_exec,partial_output}.rs`).
- **Stub sweep clean.** No `// TODO`, `unimplemented!`, `todo!`, `FIXME`, or `#[allow(dead_code)]`
  anywhere in the module.
- **No name drift.** The compressor's `{server}__{tool}` prefix split (`devtools_tool_name`,
  `rsplit("__")`) matches the real MCP adapter separator (`mcp.rs:109 format!("{}__{}", …)`);
  `ContentKind::label()` strings are only emitted by `Reduction::render`, never parsed elsewhere.
- **Test-only APIs are gated.** `reduce`, `classify`, `compress_tool_output`, `Tally::kept/total`
  are `#[cfg(test)]` with documented rationales, so they compile out of production (not form-6
  never-compiled far-ends).

---

## Findings

### [MEDIUM] src/tool_output/distill.rs:301 — `OutputDigest::render(max_salient)` budget hook left unthreaded at two live callers

- **Category:** logic
- **Decision:** CONNECT
- **Related:** `src/tools/result_processing.rs:471`, `src/tools/scoped/dispatch.rs:1654`,
  `src/tool_output/hygiene.rs:156`, `src/tools/result_processing.rs:501`

**Description**

`OutputDigest::render(&self, max_salient: usize)` documents `max_salient` as the budget
hook — "the caller can shrink this to honour a token budget" — and `MAX_SALIENT_LINES`
(60) is `pub(crate)` specifically so callers can scale it down via `scale_to_budget`.

Two of the four live consumers now do exactly that:
- `hygiene.rs::salient_cap` → `scale_to_budget(MAX_SALIENT_LINES, MIN_SALIENT_LINES, tokens)` (hygiene.rs:156)
- `result_processing.rs::inline_error_digest` → `scale_to_budget(8, 2, b)` (result_processing.rs:501)

But two further **live production** consumers still pass `digest.salient.len()` — i.e. no cap at all:

- `distill_or_truncate(text, budget_tokens)` → `digest.render(digest.salient.len())` (result_processing.rs:471)
- `clean_error_body(body)` → `digest.render(digest.salient.len())` (dispatch.rs:1654)

In both, the *full* digest is rendered first and only then measured against a downstream
token/char guard. When the digest overshoots, the signal-aware digest is **discarded** and the
caller falls back to `truncate_with_budget` / head+tail bounding — which drops precisely the
middle-of-stream errors the distiller exists to surface. So for a tool that declares a small
budget (the exact case the hook exists for) the distiller's entire value proposition is
silently nullified. `hygiene.rs`'s own doc already names this shape as a defect ("a documented
budget hook with no caller using it as one"), but it only fixed the hygiene path, not these two
sibling consumers.

This is the config-reader-parity severed-wire shape: the knob (`max_salient`) exists, a live
reader exists, but the reader reaches for a hardcoded "no cap" instead of the budget that is
already in scope (`budget_tokens` is `distill_or_truncate`'s sole parameter).

**Suggested fix**

Thread the budget already in scope into the hook:
```rust
// distill_or_truncate
let cap = scale_to_budget(MAX_SALIENT_LINES, MIN_SALIENT_LINES, budget_tokens);
let rendered = digest.render(cap);
```
and analogously in `clean_error_body` (deriving a line cap from `ERROR_BODY_MAX_CHARS`
rather than tokens). Optionally promote `hygiene::salient_cap` to a shared
`distill::salient_cap(budget_tokens)` so the "how many salient lines" policy has one source
of truth instead of three divergent copies (60/floor-4, 8/floor-2, and the two uncapped sites).

**Why not CUT/DECIDE:** the wire is genuinely load-bearing (two live callers), it is not
painless in the long run (small-budget tools get blind head+tail instead of signal), and the
budget is already in scope, so CONNECT is a one-call addition with no new coupling. The actual
edit lands in `src/tools/…` (outside this module's read-only scope), which is why this is
reported here against the producer side in `distill.rs`.

---

## State the negative

- **Not checked:** `cargo test --no-run` (the skill's phase-4 compile-of-test-code verification)
  — this was a read-only audit; no code was changed, so no regression check was run.
- **Not asserted as exhaustive:** the graph.json code graph was used only for navigation
  orientation (community/file hints), not as a source of truth for caller enumeration; the
  live-caller greps above are the authoritative check.
- **Out-of-scope severed ends:** the two unthreaded call sites live in `src/tools/…`, which is
  outside `src/tool_output/` and therefore out of this batch's edit scope.
