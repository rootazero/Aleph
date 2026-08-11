# Builtin Tools Batch 5 — note_manage + sessions + agent_manage + task_manage + hub

**Date**: 2026-08-11
**Path**: `src/builtin_tools/{note_manage,sessions,agent_manage,task_manage,hub}/*` (~45 files, ~11000 lines)
**Reviewer**: static (security / logic / architecture / quality)
**Threshold**: all findings actionable; no scoring pass.

## Module Totals

| Critical | High | Medium | Low | Total |
|---------:|-----:|-------:|----:|------:|
|        0 |    2 |     3 |   3 |    8 |

---

## Findings

### [HIGH] sessions/send_tool.rs — `SessionsSendArgs.message: String` has no upper bound
- **Category**: DoS
- **Description**: `message` flows through `RunRequest` into the cross-session execution engine without any length cap at the dispatcher. A 10 MB message blocks the gateway, consumes the cross-session inbox, and the eventual 30 s default `timeout_seconds` waits for a reply from a runner stuck decoding a multi-MB prompt.
- **Suggested fix**: Add `const MAX_CROSS_SESSION_MESSAGE_CHARS: usize = 64 * 1024;` to `sessions/send_tool.rs` (or `sessions/mod.rs`) and reject at the top of `call`. Mirror the same cap in `sessions_send_tool` and any other surface that accepts an LLM-written message body.

### [HIGH] sessions/new_tool.rs, sessions/compact_tool.rs — payloads (`initial_message`, `summary_to_inject`) lack size caps
- **Category**: DoS
- **Description**: `sessions_new`'s `initial_message` and `compact_tool`'s `summary_to_inject` are written verbatim into the new session's history. No cap exists at the dispatcher. A runaway model can fill a session with a multi-MB opening message and the next reader pays for it on every subsequent turn.
- **Suggested fix**: Same `MAX_CROSS_SESSION_MESSAGE_CHARS` constant, checked at the top of `call`. The cap is generous enough for any real onboarding blurb.

### [MEDIUM] sessions/list_tool.rs:299 — `message_limit: u32` is `.min(20)`, not `.clamp(1, 20)`; 0 falls through to the "fetch messages" branch
- **Category**: logic
- **Description**: `let message_limit = args.message_limit.unwrap_or(0).min(20) as usize;` means `message_limit=0` is the *sentinel* for "skip fetching" (the `if message_limit > 0` branch). That is correct *today*, but it makes the user-facing `message_limit` a boolean-in-disguise: a caller passing `message_limit=0` actually expects "0 messages" and is surprised by "no fetching". A single-line simplification: rename to `include_messages: bool` (default `false`) or treat 0 as a sentinel in the doc comment.
- **Suggested fix**: Add a doc comment on the field explaining the 0-sentinel contract; consider renaming the field for clarity.

### [MEDIUM] note_manage/write.rs — note body has no per-note size cap at the dispatcher
- **Category**: DoS
- **Description**: `PER_NOTE_MAX_CHARS = 4_000` exists in `read.rs` as a *read* cap, but `write.rs` accepts arbitrary-length `content` and persists it. A 100 MB note is then read back, paginated, and chunked — but every read of that note pays the full disk read. The store does not refuse the write.
- **Suggested fix**: Mirror the read cap at the write site: `if args.content.len() > MAX_NOTE_BODY_CHARS` (suggest 1 MB), reject with a clear message. Notes are knowledge artifacts, not bulk data stores; bulk data has `file_write`.

### [MEDIUM] task_manage/wait.rs — `MAX_WAIT_SECS = 600` is correct, but `wait` resolves on a polling loop that has no per-tick cap
- **Category**: DoS
- **Description**: `wait` polls the task store at some interval until either the task reaches a terminal status or the clamp'd `MAX_WAIT_SECS` elapses. The interval is small enough that 600 s = thousands of polls, which is fine; but there is no upper bound on the *poll cost* per tick (each tick does a DB read on every task this session is watching). A model issuing many `task_wait` calls in parallel multiplies this.
- **Suggested fix**: Add a `MIN_POLL_INTERVAL_MS` floor in `wait.rs::run_wait_loop` (suggest 250 ms, mirroring `wait_visual`'s `DEFAULT_POLL_MS = 250`). Pure perf nit; not blocking.

### [LOW] agent_manage/validation.rs — `validate_agent_id` uses `id.len()` not `id.chars().count()`
- **Category**: correctness
- **Description**: ASCII-only is the contract, so `.len() == .chars().count()` is true for any valid input. But if a caller smuggles a non-ASCII byte into the ID, `len()` returns the byte count and `id.len() > 64` rejects at 64 *bytes* (≈ 21 multi-byte chars). The display message then says "max 64" while the practical cap is "max 21 non-ASCII chars". Cosmetic, but the model sees a misleading limit.
- **Suggested fix**: Either `id.chars().count() > 64` (matches the char-level intent) or rename the limit in the error to "64 bytes".

### [LOW] hub/install_run.rs — `gate()` is documented as "system-enforced" but lives in the same module as the tool
- **Category**: architecture
- **Description**: `gate(ack_required, is_oci) -> GateOutcome` is the security core; the docstring says "The system-enforced install gate. The agent has NO way to satisfy the ack." Correct in *effect* but the gate function and the tool that uses it share a module, so a refactor that inlines or shadows the gate is one careless edit away. The function's `pub fn` is exposed for tests but not for production callers beyond the same file.
- **Suggested fix**: Move `gate()` and `requires_user_consent()` into a new `hub/trust.rs` submodule alongside `scan_for_injection`, so the "agent cannot bypass" property is structural (different module, no accidental inlining) rather than comment-only.

### [LOW] note_manage/helpers.rs — `validate_note_id` reject-on-traversal already in place
- **Category**: quality (positive observation)
- **Description**: This is a *strength*, not a finding — flagging it so the next reviewer does not re-add it. The 50-char reject + traversal/separator check at line 50 is the right shape and worth porting to other ID-accepting tools if they do not have an equivalent.
- **Suggested fix**: None.

---

## Strengths

- `agent_manage/validation.rs::validate_agent_id` is the single source of truth for ID grammar; both `agent_create` and `AgentManager` route through it, which is the right shape for shared grammar.
- `hub/install_run.rs::gate` is a tiny pure function with no panic surface; its placement in the same file as the tool is the only ergonomic nit.
- `hub/fetch_docs.rs` uses two distinct byte caps with a compile-time assert that the abort ceiling exceeds the clip budget (line ~37) — exactly the right pattern.
- `note_manage/read.rs` has both per-note (`PER_NOTE_MAX_CHARS = 4_000`) and total (`TOTAL_CONTENT_MAX_CHARS = 24_000`) caps that compose correctly; the truncation message tells the model what to do next ("…query with a smaller limit or read them individually").
- `task_manage/update.rs` rejects an unknown status explicitly rather than silently leaving the task unchanged — the comment at lines 119-122 is the right rationale.
- `sessions/list_tool.rs` caps `message_limit` at 20 even if the caller passes `u32::MAX`.

---

## Recommended Single Fix

A shared `sessions/limits.rs` module exposing `MAX_CROSS_SESSION_MESSAGE_CHARS` and `MAX_NOTE_BODY_CHARS` plus a small `bounded_text(value: &str, cap: usize, name: &str) -> Result<(), AlephError>` helper would close HIGH #1, HIGH #2, and MEDIUM #3 in one place. Estimated 25 lines, no behavior change beyond the new refusals.