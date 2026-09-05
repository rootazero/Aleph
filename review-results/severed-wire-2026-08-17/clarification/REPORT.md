# Severed-Wire Audit — `src/clarification`

- Date: 2026-08-17
- Tree: `/home/zou/data/workspace/Aleph/.worktrees/sw-batch5-bundled-canvas-cap-clar` (HEAD newer than the graphify graph at 9841b5b2; every claim re-verified with `rg`)
- Module: `src/clarification` (4 files, ~2,100 LOC: `ask.rs`, `mod.rs`, `render.rs`, `session.rs`)
- Method: PRODUCED–CONSUMED symbol parity (`rg` across `src/`, `bin/`, `interfaces/`, `shared/`, `desktop/`). No cargo runs. Read-only.
- Prior review refs: no prior severed-wire audit covers `src/clarification`; the module doc (`mod.rs:1-7`) and the `is_empty()` cut note (`mod.rs:262-267`) document a 2026-08-18 audit (sw-clarification-03) that already trimmed `ClarificationRequest::is_empty`. `existing_review_ref = null` throughout.

## Wiring verdicts

**Public symbols and their production reach:**

| Symbol | Production consumers | Evidence |
|---|---|---|
| `ClarificationOption`, `ClarificationQuestion`, `ClarificationRequest`, `ClarificationResult`, `ClarificationResultType`, `ClarificationAnswer` (mod.rs) | used by `ask_user` tool, `scratchpad` plan-approval, `workflow::clarify` build_request, `gateway::handlers::clarification` (handler tests only). | `rg -n "ClarificationOption\|ClarificationQuestion\|ClarificationRequest\|ClarificationResult\|ClarificationAnswer" src/ interfaces/ shared/ desktop/ src/bin/ --type rust` (full output in evidence below) |
| `DEFAULT_QUESTION_ID` (mod.rs:275) | NONE outside mod.rs | `rg -n "DEFAULT_QUESTION_ID" src/ interfaces/ shared/ desktop/ src/bin/ --type rust` → only mod.rs:222, 231, 275 (def + 2 uses in own module) |
| `ClarificationOptionView` (mod.rs:289) | NONE outside mod.rs (only embedded as field type of `ClarificationQuestionView`; never imported by name) | `rg -n "ClarificationOptionView\b" src/ interfaces/ shared/ desktop/ src/bin/ --type rust` → only mod.rs + a doc mirror comment |
| `ClarificationQuestionView` (mod.rs:301) | used as field type in `PendingClarification.questions` (session.rs:81), `AskUser` frame (gateway/events/frame.rs:130, gateway/event_emitter/types.rs:229) — wire serialization | (see below) |
| `RenderedQuestion` (render.rs:37) | used by `ask()` (ask.rs:343) and `ResolveOutcome::More.next` (session.rs:101); `.text` and `.keyboard` fields read by `deliver_next_question` (inbound_router/mod.rs:1275,1278) | `rg -n "RenderedQuestion" src/ --type rust` |
| `render` (render.rs:140) | used by `ask()` (ask.rs:343), `ClarificationManager::resolve_many` (session.rs:448), `ClarifyTaskMeta::rendered_prompt` (workflow/clarify.rs:169) | `rg -n "render::render\|crate::clarification::render::render" src/ --type rust` |
| `ClarificationDeps` (ask.rs:75) | constructed by `AskUserTool::new` (builtin_tools/ask_user.rs:224) and `ScratchpadTool::with_clarification` (builtin_tools/scratchpad.rs:470) | `rg -n "ClarificationDeps::new\|ClarificationDeps\b" src/ --type rust` |
| `AskOutcome` (ask.rs:138) | destructured by `AskUserTool::result_to_output` (ask_user.rs:307) and `ScratchpadTool::request_approval` (scratchpad.rs:885) | `rg -n "AskOutcome\b" src/ --type rust` |
| `ask` (ask.rs:271) | called by `AskUserTool::call` (ask_user.rs:365) and `ScratchpadTool::request_approval` (scratchpad.rs:888) | `rg -n "clarification::ask\|crate::clarification::ask\b" src/ --type rust` |
| `ClarificationManager` + `new`/`register`/`resolve`/`resolve_many`/`cancel`/`cancel_abandoned`/`cleanup_expired`/`has_pending`/`list_pending` (session.rs:255) | wired in `src/bin/aleph-server/commands/start/{mod.rs,builder/subsystems.rs}` (boots), `src/gateway/inbound_router/mod.rs` (resolve / has_pending on every channel reply), `src/gateway/handlers/clarification.rs` (list_pending / resolve_many), `src/clarification/ask.rs` (register / cancel / cancel_abandoned / cleanup_expired). The `pending` and `resolved` RPCs (`clarification.resolve` / `clarification.pending`) are registered by `aleph-server` (src/bin/aleph-server/commands/start/mod.rs:3210). | (see full `rg` sweep below) |
| `ResolveOutcome::More/Stale/Done`, `consumed()` (session.rs:93,113) | destructured in inbound_router (deliver_next_question, outcome.consumed() at lines 1187, 1229, 1270) and gateway/handlers/clarification.rs:163,173,178,181,184 | `rg -n "ResolveOutcome\|outcome\.consumed\(\)" src/ --type rust` |
| `PendingClarification` (session.rs:69) | used by `gateway::handlers::clarification::PendingListResponse.pending` (handlers/clarification.rs:83) | `rg -n "PendingClarification" src/ --type rust` |
| `DEFAULT_CLARIFY_TIMEOUT` (session.rs:65) | used by `tools::budget::BUILTIN_TOOL_BUDGETS_MS` (tools/budget.rs:101,265,268), `gateway/handlers/clarification.rs` (8 call sites) | `rg -n "DEFAULT_CLARIFY_TIMEOUT" src/ --type rust` |
| `ask_user_frame` (session.rs:191) | called by `publish_to_event_bus` (ask.rs:106), `publish_advance` (session.rs:237); the frame variant reaches the wire via `bus.publish_frame` | `rg -n "ask_user_frame" src/ --type rust` |
| `interpret_reply` (session.rs:679, `pub(crate)`) | called by `ClarificationManager::resolve_many` (session.rs:441) and the inbound router's `try_resolve_workflow_clarify` (inbound_router/mod.rs:1404) | `rg -n "interpret_reply" src/ --type rust` |
| `clarify_callback_payload "clarify:N"` (render.rs keyboard `callback_data`) | routed by `try_intercept_hitl` in inbound_router (parse_clarify_index at command_handler.rs:93, strip_prefix at mod.rs:1181) | `rg -n "clarify:" src/gateway/ --type rust` |

**Net severed wires: 2** (both low/medium), both `pub` items whose only consumer is the module that defines them.

## Findings

### sw-clar-01 — `ClarificationOptionView` is a `pub` type with zero external reach

- **Form**: 6 (orphaned public API surface — pub but only used inside its own module)
- **Severity**: low
- **Produced**: `pub struct ClarificationOptionView { pub label: String, pub description: Option<String> }` with `#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]` — `src/clarification/mod.rs:289-296`.
- **Consumers**: only inside `src/clarification/`:
  - field type of `ClarificationQuestionView.options` at `mod.rs:308`;
  - constructed in `impl From<&ClarificationQuestion> for ClarificationQuestionView` at `mod.rs:322-326`;
  - documented as a wire mirror in `shared/protocol/src/events.rs:872` (a comment, not an import).
  - No other crate imports the type by name; the TUI / Webchat deserialize their own mirror types from JSON (`AskOptionView`, `OptionItem`).
- **Evidence**:
  ```
  $ rg -n "ClarificationOptionView\b" src/ interfaces/ shared/ desktop/ src/bin/ --type rust
  shared/protocol/src/events.rs:872:/// Mirrors `alephcore::clarification::ClarificationOptionView`. The option's
  src/clarification/mod.rs:289:pub struct ClarificationOptionView {
  src/clarification/mod.rs:308:    pub options: Vec<ClarificationOptionView>,
  src/clarification/mod.rs:324:                .map(|o| ClarificationOptionView {
  ```
- **Rationale**: the type is reachable over the wire (it's part of the JSON shape `ClarificationQuestionView` serializes), but no other crate ever names it. The whole purpose of the `*View` types is to be the wire projection; `ClarificationQuestionView` is the actual wire entry-point and IS consumed (frame.rs:130, event_emitter/types.rs:229, session.rs:81, 219, 384). The outer type's `options` field needs SOME element type and `ClarificationOptionView` is what the project chose — but it could equally be a private struct (the visibility would not change the JSON bytes since serde doesn't read visibility).
- **Proposed change**: DECIDE. (a) CUT visibility by changing `pub struct` to `struct` (`mod.rs:289`); update the doc mirror in `shared/protocol/src/events.rs:872` to drop the path, leaving just "Mirrors the option's label/description on the wire". The wire shape is unchanged. (b) Keep as-is — the doc mirror in `shared/protocol/src/events.rs` is the only reason it's pub today (so the doc link resolves). Risk: nil at runtime; option (a) is a one-line visibility change + doc touch.
- **Risk**: nil.
- **Sanity check**: `ClarificationQuestionView` and its `From` impl still compile because the only accesses to `ClarificationOptionView` are inside `mod.rs` (the field type and the construction in the `From` impl body).

### sw-clar-02 — `DEFAULT_QUESTION_ID` is `pub` but only used inside `mod.rs`

- **Form**: 6 (orphaned public API surface — pub const with no external consumer)
- **Severity**: low
- **Produced**: `pub const DEFAULT_QUESTION_ID: &str = "answer";` — `src/clarification/mod.rs:275`.
- **Consumers**: only inside `src/clarification/mod.rs`:
  - `ClarificationRequest::text` uses it as the question id at line 222;
  - `ClarificationRequest::select` uses it as the question id at line 231.
  - The module doc at lines 272-274 explicitly documents it as the id for the single-question constructors, but no caller in the wider crate ever uses it.
- **Evidence**:
  ```
  $ rg -n "DEFAULT_QUESTION_ID" src/ interfaces/ shared/ desktop/ src/bin/ --type rust
  src/clarification/mod.rs:222:            questions: vec![ClarificationQuestion::text(DEFAULT_QUESTION_ID, prompt)],
  src/clarification/mod.rs:231:                DEFAULT_QUESTION_ID,
  src/clarification/mod.rs:275:pub const DEFAULT_QUESTION_ID: &str = "answer";
  ```
- **Rationale**: every external caller (`ask_user`, `scratchpad`, `workflow::clarify`) builds multi-question requests via `ClarificationRequest::new(vec![...])` or single-question requests via `ClarificationRequest::text(prompt)` / `ClarificationRequest::select(prompt, options)`, never naming the constant. The constant exists to give the single-question constructors a stable id (so a future caller reading `ClarificationResult::answers[0].question_id` sees `"answer"` rather than an empty string), but downstream code never needs to know that literal.
- **Proposed change**: DECIDE. (a) CUT the `pub` (downgrade to a private `const` inside `mod.rs`); the `text` / `select` constructors still reference it, and the wire id stays `"answer"`. (b) Keep as-is — it is a stable part of the wire contract even if no Rust caller names it (a JSON consumer could plausibly grep for `"answer"`). Risk: nil either way at runtime; option (a) is a one-keyword change.
- **Risk**: nil; nothing reads `DEFAULT_QUESTION_ID` by name outside the module.
- **Sanity check**: the constant remains referenced at `mod.rs:222` and `mod.rs:231` from `text()` and `select()` regardless of visibility.

## What is NOT severed (already-verified live wires)

The audit's primary goal was to confirm the headline wiring: every `pub` API the rest of the crate touches. Each of the following was individually verified with `rg` and has at least one production call site outside `src/clarification/`:

- `ask` (`ask.rs:271`) → `ask_user.rs:365`, `scratchpad.rs:888` (production tools).
- `ClarificationDeps` / `ClarificationDeps::new` → `ask_user.rs:224`, `scratchpad.rs:470`.
- `AskOutcome` → destructured at `ask_user.rs:307` and `scratchpad.rs:885`; field `withheld_secret` mapped into `WithheldQuestions` at `ask_user.rs:330-332`.
- `ClarificationManager::new` → `bin/aleph-server/commands/start/mod.rs:3180`.
- `ClarificationManager::register` → `clarification/ask.rs:322` (the one real producer of clarification requests), plus the `ask_user` tool's `call` path.
- `ClarificationManager::resolve` → `gateway/inbound_router/mod.rs:1186` (button callback) and `:1228` (free-text reply).
- `ClarificationManager::resolve_many` → `gateway/handlers/clarification.rs:213` (the `clarification.resolve` RPC handler).
- `ClarificationManager::cancel` → `clarification/ask.rs:367` (delivery-failed rollback).
- `ClarificationManager::cancel_abandoned` → `clarification/ask.rs:230` (Drop guard for abandoned `ask` futures).
- `ClarificationManager::cleanup_expired` → `clarification/ask.rs:404` (timeout reap on the parked future).
- `ClarificationManager::has_pending` → `gateway/inbound_router/mod.rs:1185, 1227` (the inbound router's "is there a live clarification" check before treating a channel reply as an answer).
- `ClarificationManager::list_pending` → `gateway/handlers/clarification.rs:216` (the `clarification.pending` RPC handler).
- `ResolveOutcome::{More, Stale, Done}` + `consumed()` → `gateway/inbound_router/mod.rs:1187, 1229, 1270` and `gateway/handlers/clarification.rs:163, 173, 178, 181, 184`.
- `PendingClarification` → `gateway/handlers/clarification.rs:83` (`PendingListResponse.pending`).
- `DEFAULT_CLARIFY_TIMEOUT` → `tools/budget.rs:268` (the `ask_user` budget must outlive this) and `gateway/handlers/clarification.rs:310,335,374,415,457,486,494`.
- `ask_user_frame` → `clarification/ask.rs:106` (publish on register) and `clarification/session.rs:237` (publish on advance). The frame variant then rides `bus.publish_frame` to clients (consumed by TUI, CLI, Webchat — `rg -n "GatewayEventFrame::AskUser" src/ interfaces/ --type rust`).
- `interpret_reply` (`pub(crate)`) → `clarification/session.rs:441` (manager's own dispatch) and `gateway/inbound_router/mod.rs:1404` (workflow-clarify reply resolution).
- `render` → `clarification/ask.rs:343`, `clarification/session.rs:448`, `workflow/clarify.rs:169` (the `ClarifyTaskMeta::rendered_prompt` builder consumed by `teams/dispatcher/clarify.rs:95`).
- `ClarificationQuestionView` → `PendingClarification.questions` (`session.rs:81`) and the AskUser frame variants (`gateway/events/frame.rs:130`, `gateway/event_emitter/types.rs:229`).
- `RenderedQuestion::text` / `::keyboard` → `gateway/inbound_router/mod.rs:1275, 1278` (`deliver_next_question` builds the outbound message).
- `ClarificationRequest::{new, text, select}` + `first` / `questions` / `len` → `builtin_tools/ask_user.rs:302`, `builtin_tools/scratchpad.rs:872`, `workflow/clarify.rs:145, 152`, plus test fixtures in `inbound_router/mod.rs` and `handlers/clarification.rs` (test-only).
- `ClarificationQuestion::{text, select, with_header, with_multi_select, with_secret, has_options}` → `builtin_tools/ask_user.rs:247, 272, 277, 278, 280`, `builtin_tools/scratchpad.rs:872-877`.
- `ClarificationOption::new` / `with_description` → `builtin_tools/ask_user.rs:88-90`, `builtin_tools/scratchpad.rs:870`, `workflow/clarify.rs:150`.
- `ClarificationResult::{answered, cancelled, timeout, value, selected_index}` + `ClarificationResultType::*` + `ClarificationAnswer::is_custom` + field reads → `builtin_tools/ask_user.rs:307-328`, `builtin_tools/scratchpad.rs:337-350`.

## What remains clean (per the protocol's checks)

- No `pub` items hide behind `#[allow(dead_code)]`.
- No `#[deprecated]` items.
- No `todo!()` / `unimplemented!()` in production paths of this module.
- No `#[cfg(feature = "…")]` items — the module compiles in one shape for every build.
- No stub handlers that validate-then-return-success.
- The 2026-08-18 `sw-clarification-03` cut (`ClarificationRequest::is_empty`) is already in the file as a comment (`mod.rs:262-267`) and there is no dead `is_empty()` method to remove today.

## Severity recap

| ID | Severity | Form | Decision |
|----|----------|------|----------|
| sw-clar-01 | low | 6 | DECIDE (visibility tweak + doc touch) |
| sw-clar-02 | low | 6 | DECIDE (one-keyword visibility downgrade) |

Both findings are cosmetic — neither breaks runtime, neither removes a wire contract. They are reported because the protocol requires it: a `pub` symbol with zero external consumers is form 6 even when it is load-bearing as a JSON DTO. The DECIDE call is left to a human because the wire-DTO argument has merit (the doc mirror comment in `shared/protocol/src/events.rs` is the only reason `ClarificationOptionView` is `pub`; `DEFAULT_QUESTION_ID` is the answer to a wire question downstream consumers do not actually ask).

No form 1-5 wires are severed. No critical/high/medium findings.