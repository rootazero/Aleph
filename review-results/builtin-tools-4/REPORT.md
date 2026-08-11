# Builtin Tools Batch 4 — browser_tools/* Code Review

**Date**: 2026-08-11
**Path**: `src/builtin_tools/browser_tools/*` (27 files, ~6457 lines)
**Reviewer**: static (security / logic / architecture / quality)
**Threshold**: all findings actionable; no scoring pass.

## Module Totals

| Critical | High | Medium | Low | Total |
|---------:|-----:|-------:|----:|------:|
|        0 |    1 |     1 |   2 |    4 |

---

## Findings

### [HIGH] browser_tools/evaluate.rs:79 — `browser_evaluate` has no upper bound on `script` size
- **Category**: DoS / security
- **Description**: `BrowserEvaluateArgs.script: String` is forwarded verbatim into the browser backend via `backend.evaluate(&tab_id, &args.script)`. The approval policy *gates* whether the call runs, but does *not* bound the payload — a malicious or runaway model can submit a multi-MB script and either block the backend's serializer or starve the browser process. This is the most powerful browser action (arbitrary JS), and the absence of a size cap is a defense-in-depth miss.
- **Suggested fix**: At the top of `BrowserEvaluateTool::call`, before the approval check, reject scripts longer than `MAX_EVAL_SCRIPT_CHARS` (suggest 64 KB — generous for any plausible DOM query / one-off automation snippet). Return `Ok(BrowserEvaluateOutput { success: false, message: Some(...) })` with a message naming the cap and pointing at chunked `snapshot`/`click` flows for genuinely large automation.

### [MEDIUM] browser_tools/batch.rs — `MAX_BATCH_ACTIONS = 50`, `MAX_BATCH_BUDGET_MS = 600_000` are correct, but per-action payload sizes inside the batch are unbounded
- **Category**: DoS
- **Description**: `BatchAction::Type { text }`, `BatchAction::Fill { value }`, `BatchAction::Select { value }` etc. flow into the per-action backend calls without a per-action length cap. A 50-action batch × 100 KB per type is still 5 MB pushed through `chrome devtools` in one call. The outer count and budget cap protect CPU, not memory.
- **Suggested fix**: Add a single `MAX_BATCH_TEXT_CHARS` (suggest 64 KB) constant at `browser_tools/mod.rs` and check it inside the `execute` loop in `batch.rs` before each action runs. Surface the same refusal shape as the MAX_BATCH_ACTIONS check (line 283-289).

### [LOW] browser_tools/cookies.rs — cookie value has no per-cookie size cap
- **Category**: DoS
- **Description**: `BrowserCookiesArgs` accepts `Vec<CookieSpec>`; each spec carries a `value: String`. Standard cookie limits are 4 KB per cookie, ~50 per domain — a tool caller can submit a 10 MB value and have the backend attempt to set it.
- **Suggested fix**: Reject cookies whose `value.len() > 4096` per RFC 6265 §6.1 with a clear "exceeds per-cookie value cap" message.

### [LOW] browser_tools/evaluate.rs:81 — `process_evaluate_result` may wrap a giant JSON value into the tool result
- **Category**: quality
- **Description**: `process_evaluate_result` wraps the raw eval result into a JSON envelope (`truncate.ts` mirrors this), but does not apply `bound_content` to the resulting string. A page that returns a megabyte-long string from `document.body.innerText` lands in the tool result untruncated.
- **Suggested fix**: Pass the wrapped string through `super::bound_content(..., super::DEFAULT_CONTENT_MAX_CHARS)` before returning. Pure hardening; the existing `DEFAULT_CONTENT_MAX_CHARS = 30_000` constant is the right cap.

---

## Strengths

- `session.rs::resolve_session_path` is a textbook example of strict input validation: `is_ascii_alphanumeric || - _ .` plus a leading-dot rejection, with the directory join rooted in `aleph_home_dir()`. Path traversal cannot escape.
- `batch.rs` has a *spec-level* cap (`MAX_BATCH_ACTIONS = 50`, `MAX_BATCH_BUDGET_MS = 600_000`) and tests for both (lines 655, 687).
- `dialog.rs` runs `check_input_secret_block` on `prompt_text` before the approval gate — secret-blocking happens regardless of policy, which is the right ordering.
- `wait_for.rs::clamp_timeout(u64::MAX)` returns the documented `MAX_TIMEOUT_MS` (line 217) — saturating arithmetic, no panic.
- `mod.rs` centralizes redaction (`redact_wrap`, `redact_and_wrap`, `redact_and_wrap_log`), screenshot bounding (`bound_screenshot_png`, `MAX_SCREENSHOT_EDGE`, `MAX_SCREENSHOT_BYTES`), and content capping (`DEFAULT_CONTENT_MAX_CHARS`, `bound_content`, `bound_content_head_tail`) so every per-action tool inherits them.

The single gap is that the *highest-trust* tool (`browser_evaluate`) is the one without an input-size guard.