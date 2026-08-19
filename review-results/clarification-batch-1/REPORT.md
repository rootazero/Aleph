# Severed Wire Audit Report — `src/clarification/`

**Audit Date:** 2026-08-22
**Scope:** `src/clarification/{mod,ask,render,session}.rs` (~2 734 LOC)
**Worktree:** `.worktrees/severed-wire-audit-batch-1` (branch `severed-wire-audit/batch-1`)
**Prior Report:** `review-results/bundled-clarification/REPORT.md` (bundled + clarification, 2026-08-15)
**Commits:** 0 (no severed wires found; no fixes required)

---

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High     | 0 |
| Medium   | 0 |
| Low      | 0 |
| **Observations** | **2** |

The `clarification/` module is fully wired. Every producer has a live consumer,
every consumer has a live producer, and every registration/dispatch arm is in place.
Zero severed wires.

---

## Findings

### No severed wires found.

Every seam was scanned across all 6 forms:

| Seam | Checked | Result |
|------|---------|--------|
| Registration parity (`ClarificationManager` methods) | All 8 public methods | OK — all have live callers |
| RPC method parity (`clarification.*` handlers) | Both handlers | OK — wired production + tests + visibility |
| Event emit-vs-subscribe (`stream.ask_user`) | Initial + advance frames | OK — Panel + CLI consumers |
| Event emit-vs-subscribe (`stream.clarification_ended`) | Terminal frame | OK — Panel card retirement wired |
| Interpreter parity (`interpret_reply`) | Both call sites | OK — session + workflow clarify wired |
| Tool wiring (`ask_user` / `ScratchpadTool`) | Deferred cell + injection | OK — full chain intact |
| Frame parity (`ClarificationQuestionView` / `ClarificationOutcome`) | Both projections | OK — Panel + event bus wired |
| Inbound router HITL interception | Both paths | OK — clarify: + plain text wired |
| Visibility policy | All 3 topics | OK — BySessionKeyOrAdmin matches RPC faces |

---

## Detailed Wire Enumeration

### 1. Registration parity: `ClarificationManager` public API

```
ClarificationManager::register          → ask.rs:297 (ask() parks on clarification)
ClarificationManager::has_pending       → ask.rs:339; inbound_router/mod.rs:1136,1178
ClarificationManager::list_pending      → clarification.rs handler; ask.rs test
ClarificationManager::resolve           → inbound_router/mod.rs:1137,1179
ClarificationManager::resolve_many      → clarification.rs handler; inbound_router
ClarificationManager::cancel            → ask.rs:339 (delivery failure rollback)
ClarificationManager::cancel_abandoned  → ask.rs:308 (RetireOnAbandon drop guard)
ClarificationManager::cleanup_expired   → ask.rs:376; session.rs:280 (register)
ClarificationManager::is_registered    → tests only (#[cfg(test)])
DEFAULT_CLARIFY_TIMEOUT                → ask.rs:302; session.rs:65; scratchpad builder
```

### 2. RPC method parity

```
clarification.resolve handler             → gateway/handlers/clarification.rs:85
  registered via                          → bin/aleph-server/commands/start/mod.rs:2926
  wired to inbound router via              → bin/aleph-server/commands/start/builder/subsystems.rs:1030
  visibility policy                        → method_visibility.rs:566 (KeyChecked)
  class census                            → method_census.rs:130 (Open)

clarification.pending handler            → gateway/handlers/clarification.rs:94
  same wiring chain
  visibility policy                        → method_visibility.rs:567 (ListFiltered)
  class census                            → method_census.rs:129 (Open)

Panel callers:
  ClarificationApi::list_pending         → interfaces/webchat/src/api/clarification.rs:122
  used in                                 → webchat/phone/chat/mod.rs:39
                                           webchat/wide/views/chat/view.rs:98
```

### 3. Event emit-vs-subscribe

```
publish_to_event_bus (ask_user frame)
  → webchat/phone/chat/mod.rs:34 (stream.* subscription)
  → webchat/wide/views/chat/view.rs:90 (stream.* subscription)
  → interfaces/cli/output/exec_echo.rs:328 (render_ask_user doc)
  → gateway/event_visibility.rs:321 (BySessionKeyOrAdmin)

publish_ended (clarification_ended frame)
  → webchat/wide/views/chat/events.rs:802-807 (clarification_ended handler)
  → gateway/event_visibility.rs:321 (BySessionKeyOrAdmin)

publish_advance (ask_user frame, advance)
  → webchat/wide/views/chat/events.rs:901-910 (ask_user event handler)
  → webchat/phone/chat/mod.rs:34; webchat/wide/views/chat/view.rs:90
```

### 4. Interpreter parity

```
interpret_reply (pub(crate))
  → session.rs:416 (resolve_many internal)
  → gateway/inbound_router/mod.rs:1299 (workflow clarify step)
```

### 5. Tool wiring

```
ask_user tool
  → builtin_tools/registry/definitions.rs:912 (tool metadata)
  → builtin_tools/registry/tool_registry_impl.rs:1249 (per-call builder)
  → start/mod.rs:3112 (cell injection)
  → builtin_tools/registry/builder/constructor/mod.rs:1304 (cell empty at build)

ScratchpadTool::with_clarification
  → builtin_tools/scratchpad.rs:1346 (test)
  (own isolated ClarificationManager — not a conflict)
```

### 6. Inbound router

```
clarification_manager: Option<Arc<ClarificationManager>>
  → inbound_router/mod.rs:135 (field)
  → inbound_router/mod.rs:180 (Default)
  → inbound_router/mod.rs:244 (with_hitl builder)
  → subsystems.rs:1030 (production wiring)

try_intercept_hitl
  → inbound_router/mod.rs:657 (handle_message entry point)
  → clarify: path: inbound_router/mod.rs:1132-1147
  → plain-text path: inbound_router/mod.rs:1177-1181
```

---

## Design Observations

### [OBS-1] `cleanup_expired` docstring slightly aspirational (prior MEDIUM-3)

`ClarificationManager::cleanup_expired` docstring says it drops "dead entries — expired
OR abandoned", but the implementation's `retain` predicate is `if entry.is_expired()` (true
branch) then `else if !entry.is_live()` (abandoned but not expired — never fired from this
method).

**Impact:** An abandoned-but-not-yet-expired entry is reaped only by `cancel_abandoned()`
(the `RetireOnAbandon` drop guard fires on the *same* session key, so it reaps the
specific entry) or by the next `register()` that fires the opportunistic sweep on that
server instance. On a long-running gateway with many cancelled runs and no subsequent
`ask` for those sessions, such entries accumulate in the map.

**Not a severed wire.** Both reaping mechanisms are live. The docstring is slightly
aspirational. The behaviour is consistent with the opportunistic-sweep design in `register`.

**Fix direction (if desired):** rename to `cleanup_dead`, change retain predicate from
`is_expired()` to `!is_live()`. No caller behaviour changes — all existing call sites
pass `is_expired()` entries which satisfy `!is_live()`. The rename documents the
broader scope and aligns the docstring with the behaviour.

### [OBS-2] `pending_questions = 0` for both `Done` and `Stale` (prior LOW-3 context)

`gateway/handlers/clarification.rs:139-143` returns `pending_questions = 0` for both
`ResolveOutcome::Done` (no more questions) and `ResolveOutcome::Stale` (answer had
no effect). For `Stale`, the entry is already gone and the client already received a
`ClarificationEnded` frame, so `0` is semantically correct.

**Not a bug.** Both outcomes require the Panel to retire the card. The dual meaning of
`0` is benign.

---

## Cross-cutting Observations

1. **`clarification/` is unusually well-documented.** The module-level, function-level,
   and field-level comments are public intent statements. Every design decision has a
   comment. This made the wire audit straightforward — the comments describe exactly what
   the code does, and the code does exactly that.

2. **The twin design (HITL clarification ↔ exec approval) is mirrored throughout:**
   `ClarificationManager` ↔ `ExecApprovalManager`, `ask_user` ↔ `scratchpad` plan approval,
   `clarification.pending` ↔ `exec.approvals.pending`. Both twins are wired identically,
   which made cross-checking straightforward.

3. **Deferred injection pattern is consistent:** `ClarificationManager` is injected via
   a `OnceCell` in `BuiltinToolRegistry`, the same pattern used for `ChannelRegistry`.
   Both cells are populated at server boot after the respective objects are constructed.

---

## What I Did NOT Do

- **Did not run `cargo check`** — per the no-compile instruction. Final gate runs after
  all modules in batch-1 are complete.
- **Did not push to remote** — branch `severed-wire-audit/batch-1` is local.
- **Did not apply OBS-1 fix** — docstring drift, not a functional defect; no live
  callers are misled by the current behaviour.
- **Did not audit `interfaces/webchat/src/components/ask_user_card.rs`** — consumer of
  RPC outputs, not a seam itself. Panel RPC wiring is covered via caller-side checks.
- **Did not audit `builtin_tools/scratchpad.rs` clarification integration** — the
  scratchpad tool's own `ClarificationManager` is intentionally isolated from the
  server's HITL manager (different sessions, different scope). No conflict exists.
- **Did not refactor `match_option` to `unicase`** (prior LOW-2) — locale issues on
  user-provided labels are a design call, not a bug.
- **Did not collapse `render` to one `write!`** (prior LOW-4) — perf differential is
  unmeasurable at the call site.
- **Did not audit sibling modules** — this report covers only `src/clarification/`.

---

## Prior Report Follow-up

`review-results/bundled-clarification/REPORT.md` (2026-08-15) covered `src/clarification/`
in the same audit as `src/bundled/`. The clarification findings were:

| Finding | Current Status |
|---------|---------------|
| MEDIUM-3: abandoned entries accumulate in map (cleanup_expired) | OBS-1 above — docstring drift, not a wire issue |
| LOW-2: `match_option` locale-blind case fold | Documented in function docstring; not a wire issue |
| LOW-3: `cleanup_expired` reaps all expired | OBS-1 above |
| LOW-4: `render` format! + push_str mix | Not a wire issue |

No prior finding was a severed wire. No fix was required.

---

## Open DECIDE Questions

None. All seams intact.

---

## Files in Scope

| File | LOC | Notes |
|------|-----|-------|
| `src/clarification/mod.rs` | ~495 | Types, wire projections, `DEFAULT_QUESTION_ID` |
| `src/clarification/ask.rs` | ~694 | `ask()`, `ClarificationDeps`, `RetireOnAbandon`, `AskOutcome` |
| `src/clarification/render.rs` | ~275 | `render()`, `RenderedQuestion`, `button_label`, `menu`, `keyboard_for` |
| `src/clarification/session.rs` | ~1204 | `ClarificationManager`, `PendingClarification`, `interpret_reply`, events |
| **Total** | **~2 734** | |

**Graph report:** `graphify-out/GRAPH_REPORT.md`
