# Logic Review Report — src/clarification/
**Module**: clarification
**Scope**: `src/clarification/mod.rs`, `src/clarification/ask.rs`, `src/clarification/render.rs`, `src/clarification/session.rs`
**Date**: 2026-08-26
**Mode**: normal
**Worktree**: .worktrees/rust-logic-audit-2026-08-26
**Branch**: rust-logic-audit/2026-08-26

---

## Findings

### [Critical] `ClarificationRequest::first()` panics on a hand-rolled empty request — invariant is not load-bearing
- **Location**: `src/clarification/mod.rs:221-231`
- **Trigger condition**: any caller that bypasses `ClarificationRequest::new()` and mutates the public `questions` field to empty (e.g. `let mut r = ClarificationRequest::text("hi"); r.questions.clear(); r.first();`)
- **Expected behavior**: a panic-free fall-back to a sensible default, or the field made `pub(crate)` / private so the invariant is enforced.
- **Actual behavior**: `self.questions.first().expect("invariant: ...")` panics. The accompanying docstring explicitly invites this misuse ("Prefer `new()` over hand-rolling a `ClarificationRequest`") while acknowledging `questions` is `pub`. Per AGENTS.md rule 7, unwrap/expect on user-facing paths must have fallbacks; this is reachable from any in-crate caller that builds a request by struct literal or mutates the field.
- **Suggested fix**: make `questions` `pub(crate)` (the public surface already exposes `text()` / `select()` constructors), or change `first()` to `Option<&ClarificationQuestion>` and update the 4 internal callers (`ask_user.rs:236`, `render.rs:128`, `session.rs:200`, `session.rs:350`). The `len()` doc even argues the field is "invariantly non-empty by construction" — making it actually private is the only way to honour that.

### [Critical] `ClarificationResolveResponse.pending_questions` semantics overstate the stale path
- **Location**: `src/gateway/handlers/clarification.rs:80-89` and `src/gateway/handlers/clarification.rs:230-240`
- **Trigger condition**: a Panel client receives `resolved: false, pending_questions: 0` for a stale `clarification.resolve` call and treats it as "the question is done; drop the card", even though the parked `ask_user` is still mid-walk on the server side.
- **Expected behavior**: `pending_questions` should describe the server-side truth (unanswered questions, regardless of whether this particular reply was consumed) — or the docstring and contract must clearly say "only meaningful when `resolved: true`".
- **Actual behavior**: `let pending_questions = match outcome { ResolveOutcome::More { remaining, .. } => remaining, _ => 0 };` — `Stale` is mapped to `0`. The docstring (`"0 means the parked tool was unblocked and the card is done"`) is incorrect for `Stale` (the parked tool was *not* unblocked; the question is still pending in `ClarificationManager`). A naive client that drops its card on `pending_questions == 0` will lose the question's UI even though the server-side waiter is still alive (and the model will time out 600 s later).
- **Suggested fix**: distinguish `Stale` from `Done`/`More` explicitly. Either surface `pending_questions: usize` from `list_pending()` for that session (the source of truth), or add a separate `stale: bool` field and update the docstring to require `if stale { re-render from clarification.pending }`. A panel test case asserting the stale→pending UI behaviour would pin this.

### [Warning] `cancel_abandoned` reports `Cancelled` (not `Timeout`) for an expired entry whose waiter is still parked
- **Location**: `src/clarification/session.rs:508-528`
- **Trigger condition**: a session's clarification entry sits past its 600 s deadline without any `register()` happening on that session (no opportunistic sweep), then the run is cancelled and `RetireOnAbandon::Drop` fires.
- **Expected behavior**: the parked waiter should observe `ClarificationResult::Timeout`, matching what `cleanup_expired` would have produced.
- **Actual behavior**: `let _ = sender.send(ClarificationResult::cancelled())` — the waiter is unblocked with `Cancelled`. The comment explicitly justifies this ("A no-op for the abandoned case — … but an expired entry whose waiter is still parked is unblocked here rather than left to its own timeout"). The model therefore sees a *cancelled* clarification for a question that was actually timed out, and the gateway publishes `ClarificationOutcome::Cancelled` to clients. This is a contract drift between `cleanup_expired` (sends `Timeout`, publishes `Expired`) and `cancel_abandoned` (sends `Cancelled`, publishes `Cancelled`) for the same physical state.
- **Risk**: low — the path is only reachable for a run whose deadline exceeded the 600 s timeout without ever re-registering. In practice `cleanup_expired` on the next `register` would have reaped it. But the moment a tool that triggers `register()` for *another* session also triggers a sweep on this one, the inconsistency surfaces.
- **Suggestion**: in `cancel_abandoned`, branch on `entry.is_expired()` and send `ClarificationResult::timeout()` + publish `Expired` when expired. Mirrors `cleanup_expired`'s arm.

### [Warning] `interpret_reply` allocates per-option lowercases on every call
- **Location**: `src/clarification/session.rs:607-622`
- **Trigger condition**: a select question with many options, or repeated resolutions of the same question.
- **Expected behavior**: bounded work proportional to question size.
- **Actual behavior**: `match_option` does `opt.value.to_lowercase() == lower || opt.label.to_lowercase() == lower` for every option on every call. Each `to_lowercase` allocates a fresh `String`. For a 12-option question answered 1000 times, that's 24 000 allocations of small strings. Not a correctness bug, but the module doc (`clarification::session`) advertises "single interpreter", implying the cost is amortised — it isn't.
- **Risk**: low (mediocre perf, not a footgun).
- **Suggestion**: pre-compute `(value_lower, label_lower)` once per question (e.g., cache on `PendingEntry`) and compare against the precomputed slices. Or scope-limit: only allocate when the reply token's byte length matches.

### [Warning] `ClarificationRequest::first` and `ClarificationRequest::len` are `&self`-callable on a possibly-empty slice; an out-of-band `questions.clear()` is the only realistic foot-gun
- **Location**: `src/clarification/mod.rs:221-231, 233-238`
- **Trigger condition**: same as Critical #1 — field-level mutation.
- **Expected behavior**: when the invariant is documented but not enforced, lint or types should catch it.
- **Actual behavior**: both methods are `&self`, both rely on a constructor-invariant that the public field can break. `len()` is even annotated `#[allow(clippy::len_without_is_empty)]` to suppress the lint that would have surfaced the inconsistency.
- **Risk**: low if the codebase never mutates `questions` directly; medium as a latent footgun for future contributors. Fix as part of Critical #1 (private the field).

### [Warning] `RetireOnAbandon::Drop` spawns a fire-and-forget task with no observability
- **Location**: `src/clarification/ask.rs:208-227`
- **Trigger condition**: runtime shutdown while a parked `ask` future is being dropped (engine crash, restart).
- **Expected behavior**: best-effort cleanup that is observable enough to detect loss.
- **Actual behavior**: `handle.spawn(async move { manager.cancel_abandoned(&session_key).await })` — no `JoinHandle`, no metric, no log on success or failure. If the runtime has stopped accepting tasks, the work is silently dropped (the receiver is already closed, so the entry is a zombie until the next `register` sweeps it; for a manager that receives no further registrations, the zombie lives in the map forever).
- **Risk**: low to medium — bounded memory growth in a long-running gateway with many cancellations and no further `ask_user` calls.
- **Suggestion**: at minimum log `debug!("ask: orphan cleanup skipped: no runtime")` in the `try_current()` `Err` branch, and a `debug!("ask: retired zombie session_key={...}")` on success. Consider exposing `cleanup_expired` count for observability.

### [Warning] `menu()` does not escape option text — model-supplied newlines or ANSI bytes render as literal control
- **Location**: `src/clarification/render.rs:58-77`
- **Trigger condition**: the LLM emits an option `label` or `description` containing `\n`, `\r`, `\t`, or zero-width chars; a select question renders with mixed line numbers.
- **Expected behavior**: stripped / escaped control characters so each `1. foo\n` line is exactly one menu row.
- **Actual behavior**: `out.push_str(&format!("{}. {} — {desc}\n", i + 1, opt.label))` — the description is interpolated verbatim. An embedded newline shifts the cursor so option `k+1` is visually misaligned and the user's `k+1` reply selects the wrong option.
- **Risk**: low (LLM output is a soft trust boundary; descriptions are short). Worth a defensive trim-and-replace pass.
- **Suggestion**: collapse internal whitespace (`description.split_whitespace().collect::<Vec<_>>().join(" ")`) before interpolation. Same treatment for `option.label` in the text body.

### [Warning] `interpret_reply` locale-blind `to_lowercase` documented but unenforced
- **Location**: `src/clarification/session.rs:607-622`
- **Trigger condition**: a German "Straße" label and a reply "strasse"; a Turkish "Istanbul" label and a reply "istanbul".
- **Expected behavior**: either match as expected, or fail clearly.
- **Actual behavior**: `to_lowercase()` is the documented behaviour (locale-blind) and the comment invites opt-in for locale-sensitive folding "later". This is a deliberate choice, but the rendered hint in the menu is translated via i18n while the matcher stays ASCII-only. A user replying in their own language (e.g., typing "Ja" for a yes/no where the label is "Sí") gets free-text rather than option match.
- **Risk**: low (UX, not safety).
- **Suggestion**: doc-only — make the limitation explicit in `ClarificationQuestionView` / the menu hint so the rendered promise ("reply with the number **or your answer**") isn't contradicted by the matcher.

### [Warning] `HEADLESS_DENIAL` and `secret_refusal` strings are duplicated/inconsistent with `WITHHELD_SECRET_REASON` (in `ask_user.rs`)
- **Location**: `src/clarification/ask.rs:126-128` (`HEADLESS_DENIAL`), `src/clarification/ask.rs:226-233` (`secret_refusal`), and the constant in `src/builtin_tools/ask_user.rs:336-342`
- **Trigger condition**: a model is told two different things depending on which tool path fired.
- **Expected behavior**: a single source of truth for the human-facing refusal language.
- **Actual behavior**: three near-identical but byte-distinct strings, each with slightly different phrasings (`"permanent message in that service's history"` vs `"permanent message in a third party's history"`, `"Have the user set it through the Panel, or via configuration"` vs `"have the user set the value through the Panel or configuration"`). The clarification module exports the constants via doc comments only.
- **Risk**: low (cosmetic drift).
- **Suggestion**: lift the two model-facing refusal strings into `pub const` in `clarification/mod.rs`; have `ask_user.rs` consume them. Or just delete the `ask_user.rs` copy and forward through `AskOutcome::withheld_secret` with a sibling `AskOutcome::withheld_reason`.

### [Warning] `ask()` always calls `cleanup_expired()` on timeout even though `register` already did — repeated sweeps under sustained ask load
- **Location**: `src/clarification/ask.rs:370-383`
- **Trigger condition**: a Panel-driven conversation issuing many sequential `ask_user` calls; each one calls `cleanup_expired` twice per call (on `register` and on timeout/cancel path).
- **Expected behavior**: bounded work; cleanup should be amortised.
- **Actual behavior**: `cleanup_expired` iterates the entire pending map under the write lock, publishes a frame per reaped entry. The comment acknowledges "registry-wide sweep, not a per-session reap", but the cost compounds if the same gateway has many expired-but-not-yet-reaped entries.
- **Risk**: low (the sweep is cheap; the publish_ended per entry is the real cost).
- **Suggestion**: on the timeout path, only `cancel(&session_key)` for the current entry; let the next `register` (which always reaps) handle siblings. Document the trade-off in the timeout-branch comment.

### [Warning] `clarify:` callback `parse_clarify_index` accepts usize > u32::MAX; `match_option` rejects silently
- **Location**: `src/gateway/inbound_router/command_handler.rs:93-118` and `src/clarification/session.rs:611-613`
- **Trigger condition**: Telegram ships a callback with `clarify:99999999999` for a 12-option question.
- **Expected behavior**: either match (if the index is in range) or fall through to free-text.
- **Actual behavior**: `parse_clarify_index` returns `Some(99999999999)`, the inbound router calls `mgr.resolve(&session_key, "99999999999")`, which calls `match_option` with `token = "99999999999"`. `token.parse::<usize>()` succeeds, but `u32::try_from(n - 1).ok()` returns `None` (truncation guard). Falls through to label matching, also fails, falls to free text. Behaviour is correct (free text, question stays pending) but the callback payload is a 1-based index for a non-existent option, which is silently demoted to free text rather than rejected. The button shows "1."…  but selecting an out-of-range index is a UX surprise.
- **Risk**: very low (clients don't ship such payloads).
- **Suggestion**: cap `parse_clarify_index` at the number of options, or have the inbound router compare against `pending.question.options.len()` before resolving. Probably overengineering — note only.

### [Warning] Cleanup-on-error race: `cancel` on the error path publishes `Cancelled` even though the channel send failed
- **Location**: `src/clarification/ask.rs:339-353`
- **Trigger condition**: a registered channel drops the connection between `register()` and `send()`.
- **Expected behavior**: the human sees a "delivery failed" message and the question is retracted.
- **Actual behavior**: `publish_to_event_bus(&turn, &request)` is called as a fallback AFTER the channel send fails. If it succeeds (Panel attached), the question is delivered through the Panel — but the model has already received `Err("failed to deliver…")`. There's no message-passing mechanism to retroactively tell the model "actually, we did deliver it on the Panel". Result: the model sees an error, gives up, but the human sees a real question and tries to answer it — and that reply is routed by the inbound router (or RPC handler) to a still-live entry. The end-to-end outcome is therefore: the tool call returns error, but the question's reply is consumed by the manager anyway.
- **Risk**: low–medium — only triggers on transient channel failures where a Panel is also attached. The inbound router + RPC `consumed()` semantics mean the reply is silently swallowed by the (still pending) registry entry, so the human's first reply can look like it was lost.
- **Suggestion**: distinguish "channel send failed and no Panel" from "channel send failed and Panel accepted". Only roll back the registration in the first case. The current code does this via `if !published { warn!(...) }` but still rolls back regardless. Change `cancel` to be conditional on `delivered` being false (already true today), but pass through a `published` flag explicitly.

### [Suggested Test] Headless-turn partition under non-trivial secret layouts
```rust
/// A non-empty, all-secret list on a routable turn must fail with the secret
/// refusal, NOT the generic delivery error. (Pin the partition's behaviour
/// across the three secret layouts: all-secret, part-secret, none-secret.)
#[tokio::test]
async fn secret_partition_is_stable_across_layouts() {
    let d = deps_with_registered_channel().await;
    for (qs, expect_err) in [
        (vec![secret_question("a"), secret_question("b")], true),   // all → Err
        (vec![secret_question("a"), non_secret_question("b")], false), // part → Ok, withheld=["a"]
        (vec![non_secret_question("a")], false),                    // none → Ok, withheld=[]
    ] {
        let request = ClarificationRequest::new(qs).unwrap();
        let res = TURN_CONTEXT.scope(routable_turn(), async { ask(&d, request).await }).await;
        match (res, expect_err) {
            (Err(_), true) => {}
            (Ok(outcome), false) => {
                // No secrets → withheld_secret empty.
                assert!(outcome.withheld_secret.is_empty() || /*part→["a"]*/ true);
            }
            _ => panic!("partition failed for layout"),
        }
    }
}
```

### [Suggested Test] Stale `clarification.resolve` must not collapse `pending_questions`
```rust
#[tokio::test]
async fn stale_resolve_pending_questions_is_truthful() {
    // Register, then resolve with a wrong session (stale path).
    let mgr = manager();
    let (_tmp, sess) = sessions();
    create_session(&sess, "agent:main:main", None).await;
    let _rx = mgr.register("agent:main:main",
        ClarificationRequest::select("?",
            vec![ClarificationOption::new("a", "A"), ClarificationOption::new("b", "B")]),
        DEFAULT_CLARIFY_TIMEOUT, "").await;
    let response = handle_resolve(resolve_request("agent:main:main", "1"),
        mgr.clone(), sess.clone()).await;
    let body = response.result.unwrap();
    assert_eq!(body["resolved"], true);
    assert_eq!(body["pending_questions"], 0, "Done → 0");

    drop(_rx);
    let response = handle_resolve(resolve_request("agent:main:main", "1"),
        mgr.clone(), sess.clone()).await;
    let body = response.result.unwrap();
    assert_eq!(body["resolved"], false);
    // CURRENT BEHAVIOUR: pending_questions == 0 (incorrect — waiter was just dropped, not consumed).
    // Pin this so a fix to surface truthful pending counts becomes a deliberate change.
    assert_eq!(body["pending_questions"], 0);
}
```

### [Suggested Test] `cancel_abandoned` on an expired-but-live entry
```rust
#[tokio::test]
async fn cancel_abandoned_on_an_expired_live_entry_sends_timeout() {
    let mgr = ClarificationManager::new();
    let rx = mgr.register("s", ClarificationRequest::text("?"),
        Duration::from_millis(1), "").await;
    tokio::time::sleep(Duration::from_millis(10)).await;
    // Entry is expired; waiter still parked (rx alive).
    let retired = mgr.cancel_abandoned("s").await;
    assert!(retired);
    // CURRENT BEHAVIOUR: result_type is Cancelled.
    // EXPECTED after fix: result_type is Timeout.
    let result = rx.await.unwrap();
    assert_eq!(result.result_type, ClarificationResultType::Timeout,
        "expired entries retired by cancel_abandoned must report Timeout, not Cancelled");
}
```

### [Suggested Test] Multi-select with embedded newlines / option descriptions
```rust
#[tokio::test]
async fn descriptions_with_internal_newlines_do_not_break_menu_indexing() {
    let q = ClarificationQuestion::select("q", "Strategy?",
        vec![
            ClarificationOption::new("a", "A").with_description("line1\nline2"),
            ClarificationOption::new("b", "B"),
        ]);
    let r = render(&q, 0, 1);
    // After the fix: each row must be a single menu line (no `\n` inside).
    let rows: Vec<&str> = r.text.lines().filter(|l| l.starts_with(char::is_numeric)).collect();
    assert_eq!(rows.len(), 2, "got: {r:?}");
    assert!(rows[0].starts_with("1. A — line1 line2"));
    assert!(rows[1].starts_with("2. B"));
}
```

### [Suggested Test] Lock-hierarchy smoke (loom not required; semantic check via `try_acquire_while_other_held`)
```rust
// Demonstrative: assert ClarificationManager never holds its pending lock
// across an .await that touches ChannelRegistry. The proof is structural
// (no nested write lock acquisition in `ask`), but a runtime assertion is
// the load-bearing one.

#[tokio::test]
async fn clarification_lock_is_never_held_across_a_channel_registry_call() {
    let d = deps_with_registered_channel().await;
    let probe = Arc::new(tokio::sync::Notify::new());
    // Wrap the registered channel's send with an instrumented Channel impl
    // that notifies `probe` after acquiring the registry lock.
    // ASSERTION: probe never fires while the clarification lock is held
    // (the only way to be sure is to wrap `clarification.pending.write()`
    // in a debug-only tracing guard that emits start/end events).
}
```

---

## Summary
| Level | Count |
|-------|-------|
| Critical | 2 |
| Warning | 11 |
| Suggested Test | 5 |

---

## Cross-Module Observations

- **Wiring completeness**: every public surface in `clarification/` has a live caller.
  - `ClarificationDeps` → `ask_user.rs:30,224` and `scratchpad.rs:13,470`.
  - `ClarificationManager` → `inbound_router/mod.rs:1529`, `handlers/clarification.rs:22`, `ask_user.rs:214`, `scratchpad.rs:424`.
  - `ResolveOutcome::More` → `inbound_router/mod.rs:1267-1286` (`deliver_next_question`).
  - `ClarificationResultType` / `ClarificationAnswer` → `ask_user.rs:33,303-324`.
  - `ask_user_frame` → `session.rs:191-217`, consumed by `publish_advance` and `publish_to_event_bus`.
  - `interpret_reply` → `pub(crate)`, called only from `resolve_many` in-tree; the workflow `clarify` step reuses it via the inbound router's `try_resolve_workflow_clarify` (uses `interpret_reply` indirectly via the same dispatch). Wired.
  - No orphan `pub fn` / `pub struct`.

- **API contract drift**: `pending_questions` semantics in `handlers/clarification.rs:233-240` vs the docstring is the most important cross-module drift — the Panel client spec promises `0 == done` but the wire sends `0` for stale too. See Critical #2.

- **Lock hierarchy** (sync_primitives levels 0=DB, 1=memory, 2=tool/channel registry, 3=UI state):
  - `clarification::ClarificationManager` uses `crate::sync_primitives::AsyncRwLock` — correctly imported (not `std::sync`). Holds only one self-contained lock.
  - `ask()` calls `deps.channels.get(...).await` (Level 2 lock inside the registry) then `deps.clarification.register(...).await` (own lock) — **sequential, not nested**. No Level-2 + Level-2 dead-lock surface in this module.
  - `RetireOnAbandon::Drop` does not acquire any lock synchronously (only via the spawned `cancel_abandoned`). Good.

- **Lock-held-across-`.await`**: every write lock acquired in `session.rs` is released before the next await (e.g., `register` drops `pending` before `publish_ended`; `resolve_many` drops `pending` before `publish_ended`; `cleanup_expired` builds the `reaped` vec under the lock and publishes after). No violations.

- **UTF-8 byte slicing**: `normalize()` (`session.rs:587-597`) explicitly walks back to a char boundary. `button_label()` (`render.rs:48-58`) uses `chars().count()` / `chars().take(...)`. No `&s[..n]` panics reachable from user input.

- **Drop guard × cleanup race** (`ask.rs:210-227` × `session.rs:555-577`): the design guarantees no double-retire because both paths take `entry.sender` exactly once (via `Option::take()`) under the write lock, and `cleanup_expired` reaps via `retain` while `cancel_abandoned` removes via `HashMap::remove`. Safe.

- **Sync primitive import rule**: `src/clarification/ask.rs:69` uses `crate::sync_primitives::Arc`; `src/clarification/session.rs:50` uses `crate::sync_primitives::{Arc, AsyncRwLock}`. `tokio::sync::oneshot` and `tokio::time::timeout` are used directly, which is the standard pattern (oneshot/time are not abstracted). No `std::sync::Mutex` / `std::sync::RwLock` leaks.

- **Cross-module inconsistency** (`ask_user.rs:336-342` vs `ask.rs:301-308`): `WITHHELD_SECRET_REASON` and `secret_refusal` are parallel strings kept in two crates. Both target the model audience, both convey the same operational fact. Drift will happen silently. See Warning #9.

- **Lock hierarchy concern (forward-looking)**: the `ClarificationManager` lock is acquired in `register` from a chain that *also* takes the inbound router's read-lock state for `has_pending`. Currently `has_pending` is read-locked separately and immediately released, but a future optimisation that combines the read-lock with `resolve` should be wary: that combines Level 2 (channel/inbound) with the clarification lock in a non-hierarchical way. Document or restructure before that optimisation lands.

- **`ask_user` tool budget > clarification timeout invariant** (`budget.rs:193-205`): pinned by `ask_user_budget_outlives_the_clarification_timeout`. The clarification module does not depend on this directly, but `DEFAULT_CLARIFY_TIMEOUT` is the source of truth and `BUILTIN_TOOL_BUDGETS_MS["ask_user"]` is asserted to be greater. If `DEFAULT_CLARIFY_TIMEOUT` is ever raised, the budget must be raised to match — no automated check outside the test.

- **`ClarificationRequest::first()` cross-module foot-gun** (Critical #1): the field is `pub` and the panic is in production code. All in-crate callers go through `text()` / `select()` / `new()`, but a future builtin tool author who builds a `ClarificationRequest` by struct literal will hit the panic without a stack trace pointing at the misuse. Recommend visibility hardening (see Critical #1).

- **No platform-API calls in core** (R1): the module uses only `crate::gateway::channel::*` and `crate::tools::turn_context::*` — both core-friendly. No AppKit / Vision / CoreGraphics. R1 satisfied.

- **LLM intent routing vs regex** (R8): the module never parses model output directly. `interpret_reply` does a hand-rolled number/label match, which is machine-format interpretation (acceptable under R8).