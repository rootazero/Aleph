# Review Report — Batch 3: `src/exec/approval/*` + `src/exec/bridge.rs` + `src/exec/socket.rs`

**Date:** 2026-08-11
**Scope:** `src/exec/approval/{mod.rs,types.rs,channel_bridge.rs}` (340 lines) +
`src/exec/bridge.rs` (218 lines) + `src/exec/socket.rs` (133 lines) — 691 lines total
**Reviewer:** static (security / logic / architecture / quality)
**Worktree:** `/tmp/aleph-review-exec-executor` (branch `review/exec-executor`)

## Summary

| Critical | High | Medium | Low | Total |
|---------:|-----:|-------:|----:|------:|
|        0 |    2 |     3 |   2 |    7 |

This batch is the wire between the approval manager and the channels (Telegram,
Panel, Webchat, …). The bugs here are also **silent and one-sided**: an
approval card the human never sees is a "deny" by timeout. Most of the
findings are about boundary checks that the surface must always get right
because the surface is reached from at least four call sites, and one
typo in a wire format breaks all four.

## Findings

### [HIGH] `bridge.rs:parse_callback` (line 51-72) — `rsplit_once` accepts any number of colons in the id, including trailing ones
**Category:** security
**Confidence:** High

**Description.** `parse_callback` is the only path that converts a Telegram
button click into a `resolve` call. The format is
`"approve:{id}:{decision}"` where `id` is the manager's `record.id`
(typically a UUID v4 string from `uuid::Uuid::new_v4().to_string()`).

The parser does:
```rust
let (prefix, rest) = data.split_once(':')?;
if prefix != "approve" { return None; }
let (id, decision_str) = rest.rsplit_once(':')?;
if id.is_empty() { return None; }
```

This is mostly right, but:
- The `id.is_empty()` check at the end guards against a malformed
  callback like `"approve::deny"`, where `rsplit_once` would yield
  `("", "deny")`. OK.
- BUT: an id that is a single `:` (one colon only) is treated as a
  legitimate id. The manager's id is a UUID with no colons, so this is
  not a real attack surface today. The risk is **future**: if a
  manager-id format ever contains a colon (e.g. a session-prefixed id
  for debugging), `rsplit_once` will silently truncate it. The
  Telegram callback is also a **public** surface (any user with access
  to the bot can craft a callback), so the parser is the only line of
  defence against spoofed callbacks.

The actual gap is the **decision-string validation** after the parse:
the function accepts `"once" | "session" | "always" | "deny"` but does
NOT reject other decision values that share the prefix. A callback
`"approve:abc:once:"` would `rsplit_once` to `("approve:abc:once",
"")` — `decision_str = ""`, falls through to the wildcard match,
returns `None`. OK.

The HIGH is about the **forward compatibility**: a future caller that
emits a `:` in the id will have `rsplit_once` silently truncate it,
and the manager's `resolve` will look up a non-existent id. The fix
is to use a fixed-width delimiter (e.g. `data.splitn(3, ':')` and
reject if there are more than 3 fields) or to validate the id against
the canonical UUID format.

**Suggested fix.** Replace `rsplit_once` with a three-field split:

```rust
let mut parts = data.splitn(3, ':');
let prefix = parts.next()?;
if prefix != "approve" { return None; }
let id = parts.next()?;
let decision_str = parts.next()?;
if id.is_empty() { return None; }
if parts.next().is_some() { return None; } // trailing junk
```

This makes the wire format `"approve:{id}:{decision}"` strict, and a
future caller that emits a `:` in the id is rejected at the wire.

### [HIGH] `approval/channel_bridge.rs:deliver_routed` (line 184-216) — text-fallback message overflows channel limits
**Category:** logic
**Confidence:** Medium

**Description.** When the channel has no `approval_capability` (e.g.
Webchat's plain-text reply path), the bridge sends a fallback text
message that includes `action.summary` (the redacted tool name +
command) inside a `\`\`\`` code fence. The text is:

```
⚠️ 工具 `{tool_name}` 需要你的授权。
```
{action.summary}
```
{reason}

回复 /approve 批准本次、/approve session 本会话内不再询问、/deny 拒绝（可附原因：/deny 原因…，会转告给 agent）。
```

`action.summary` is not bounded. `ExecApprovalRecord::display_line` (in
`manager.rs:670`) truncates at 120 chars, but **the text-fallback path
uses `action.summary` directly**, not the truncated display line. A
summary of 4 KB would still be in the message; channels with a 4096-
char limit (Telegram's actual limit is 4096) would silently truncate
or refuse to send.

**Suggested fix.** Reuse the manager's `display_line` truncation, or
add a `MAX_SUMMARY_CHARS` constant here:

```rust
const MAX_SUMMARY_CHARS: usize = 1000;
let summary: String = action.summary.chars().take(MAX_SUMMARY_CHARS).collect();
if action.summary.chars().count() > MAX_SUMMARY_CHARS {
    summary.push('…');
}
```

### [MEDIUM] `bridge.rs:parse_callback` (line 65-72) — `AllowAlways` is parsed but never produced; the path is dead but still in the wire surface
**Category:** logic
**Confidence:** High

**Description.** `parse_callback` returns `ApprovalDecisionType::AllowAlways`
for the `"always"` decision string. `ApprovalDecisionType::clamped()` (in
`socket.rs`) then narrows it to `AllowSession`. The path is functionally
fine — `AllowAlways` and `AllowSession` are the same outcome at the
manager level — but:

- The inline-keyboard **never renders an "always" button**
  (`build_approval_keyboard` in `bridge.rs:32-44` skips it). So the
  decision string `"always"` cannot reach `parse_callback` from any
  current UI. It is a dead wire path.
- The dead path is still in the wire surface, so an old Telegram
  client that had a pinned callback (`"approve:abc:always"`) would
  be accepted by `parse_callback` and would produce an
  `AllowAlways` decision that the manager narrows to `AllowSession`.
  The user sees a session grant when the legacy client labelled the
  button "Allow always".

The comment on `AllowAlways` in `socket.rs:24-27` documents this as
intentional: "Kept only so in-flight callback payloads and external
clients still deserialize." OK — but the dead path is also a wire
attack surface. A malicious actor who can craft a callback can
submit `"approve:uuid-of-a-real-card:always"` and the manager will
honour it as a session grant (cascading to other cards with the
same `grant_key`).

**Suggested fix.** Two options:

1. Accept the dead path but document the wire risk: a callback from
   a non-UI source that says `"always"` is honoured as a session
   grant. Add a test asserting this is the actual behaviour so a
   future refactor cannot quietly drop it.
2. Refuse `"always"` at `parse_callback` (return `None`) since the
   UI never produces it; legacy clients that need the path can
   upgrade. The manager-side `clamped()` still narrows the new
   `AllowSession` correctly.

The conservative pick is (1) with a one-line comment that the dead
path is a deliberate wire-compat choice.

### [MEDIUM] `approval/channel_bridge.rs:request_for_tool` (line 75-90) — `record_session_key` synthetic key collides with real session keys
**Category:** logic
**Confidence:** Medium

**Description.** When `session_key` is empty, the bridge synthesizes
`format!("{}:{}", channel_id.as_str(), conversation_id.as_str())` and
uses that as the record's `session_key`. A real session key from
`SessionKey::to_key_string()` is `agent:<id>:<rest>`, which never
contains a bare `:` separator that would collide. So the synthetic
key is reachable only via button callback (it has no AgentToAgent
text reply path). But:

- The synthetic key is stored on the record. `resolve_for_session`
  looks up by `session_key`, so a `/approve` text reply addressed to
  the same channel+conversation would NOT find a synthetic-keyed
  record. OK.
- The synthetic key IS reachable via `record_originator(id)` (the
  by-id gate), so a button click from anyone other than the
  originator is refused. OK.

The MEDIUM is about a **future risk**: a future caller that hands
the bridge a real `session_key` and a future change to the
synthetic-key format could collide. The current `format!` is fine.

**Suggested fix.** No code change; document the contract on
`record_session_key`:

```rust
// Synthetic key shape `channel_id:conversation_id` is unreachable from
// `resolve_for_session` because real SessionKey values never match
// this pattern (SessionKey::to_key_string always starts with the
// namespace prefix). A future change to the key format must keep this
// invariant.
let record_session_key = if session_key.is_empty() {
    format!("{}:{}", channel_id.as_str(), conversation_id.as_str())
} else {
    session_key.to_string()
};
```

### [MEDIUM] `socket.rs:ApprovalDecisionType::to_outcome` (line 95-110) — `AllowAlways` arm is documented as "Unreachable post-clamp" but the compiler cannot prove it
**Category:** logic
**Confidence:** Low

**Description.** `to_outcome` calls `self.clamped()` and matches the
result. The `AllowAlways` arm at line 105 is unreachable at runtime
because `clamped()` narrows it to `AllowSession`. The compiler keeps
the arm because removing it would require either a wildcard or a
`#[allow(unreachable_patterns)]`. The code IS correct; the
documentation comment is right that the arm is unreachable.

The MEDIUM is that the unreachable arm is a **silent no-op** if
someone refactors `clamped()` to remove the narrowing: the
`AllowAlways` case in `to_outcome` would suddenly become reachable
and would produce `ApprovedForSession` as a hidden side effect of
the unreachability. A test that asserts `AllowAlways.to_outcome() ==
ApprovedForSession` (which `socket.rs:144-149` does) catches this
only if the test is run, and only if the narrowing rule itself
is the changed.

**Suggested fix.** Leave, but consider extracting the
`AllowAlways → AllowSession` narrowing as a single const-evaluated
helper and having BOTH `clamped` and `to_outcome` go through it, so
the rule cannot diverge in two places. (This is a refactor, not a
fix.)

### [LOW] `bridge.rs:build_approval_keyboard` (line 32-44) — no test covers the `deny`-only row case for non-empty `allow_row`
**Category:** quality
**Confidence:** High (no test gap, just a documentation gap)

**Description.** The function builds up to two rows: an "allow" row
(once / session buttons, only if `allowed` permits), and a "deny"
row (only if `allowed` contains `Deny`). The tests cover:
- The full set (all three buttons) — session is rendered, "always"
  is not.
- `Deny`-only set — single deny row.
- (Implicit) empty `allowed` — never tested, would produce an empty
  keyboard. The function returns an empty `InlineKeyboard` in that
  case, which a Panel rendering path would render as nothing — the
  user sees a request with no buttons and no clear "decline"
  affordance.

**Suggested fix.** Either reject empty `allowed` at the call site
(refuse to deliver an unanswerable card), or render a single
"Decline" button when `allow_row` is empty and `Deny` is missing.
Both options are wider than a Low; the current behaviour is
documented in the function-level doc comment.

### [LOW] `approval/types.rs:ApprovalRequest` (line 7-12) — `Capability` variant deleted but the comment about it is still present
**Category:** quality
**Confidence:** Low

**Description.** The comment at line 5-7 says:
> Kept as a tagged enum rather than a bare `CommandApprovalRequest` so
> the serde shape stays stable for any consumer holding serialized
> payloads. The `Capability` variant was removed: no production code
> ever constructed it — only tests did.

The comment is fine, but the enum shape is now strictly `Command`. A
future variant addition would change the serde shape, and a future
test that holds a serialized `"type": "capability"` payload would
fail to deserialize. The "for any consumer holding serialized
payloads" rationale is now an aspirational comment.

**Suggested fix.** Either remove the comment (the serde-shape
stability is now a single-variant guarantee), or simplify the type
to `pub struct CommandApprovalRequest { ... }` and remove the
wrapper. The latter is the cleaner refactor.

## Cross-References

- `bridge.rs:parse_callback:51` — wire format is `approve:{id}:{decision}`.
  The Telegram callback is a **public** surface; the parser is the only
  line of defence against spoofed callbacks. The `to_outcome` path
  narrows `AllowAlways` → `AllowSession`, so a callback that says
  `"always"` produces a session grant. See
  `src/exec/socket.rs:ApprovalDecisionType::clamped` for the narrowing
  rule.
- `approval/channel_bridge.rs:request_for_tool:75-90` — the synthetic
  `record_session_key` is documented as unreachable from the text-reply
  FIFO. The `record_originator` gate (in `src/exec/manager.rs:402-415`)
  is the only thing that protects a synthetic-keyed record against
  spoofed button clicks. The two must move in lock-step.
- `socket.rs:to_outcome:95-110` — the unreachable `AllowAlways` arm
  is a deliberate "compiler-kept-but-runtime-unreachable" choice. The
  `clamped()` helper is the single source of the `AllowAlways` →
  `AllowSession` narrowing; `to_outcome` and `clamped` must move
  together.
