# TUI streaming and interaction implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Connect Aleph's shared transcript and clarification infrastructure to a responsive TUI, including the approved sticky last-user message and jump action.

**Architecture:** The shared pure reducer owns transcript facts; Core owns clarification identity and answer interpretation. The TUI renders and schedules bounded asynchronous I/O, with generation-scoped recovery and one modal owner. Two isolated worker branches integrate into `feat/tui-streaming-refactor`; main remains untouched.

**Tech Stack:** Rust, Tokio, Ratatui, existing shared protocol/client/ui_logic. No new framework or runtime.

**Spec:** `docs/superpowers/specs/2026-10-14-tui-streaming-interaction-refactor-design.md` (approved by user 2026-10-14, including sticky visibility default).

## Global Constraints

- Replies Chinese, code comments English; reference documentation bilingual.
- Pure I/O interfaces; no Core dependency in the TUI. No inferred success, no replay/live stitching without a watermark.
- Every git porcelain command uses `git -c core.autocrlf=true`; no renormalize.
- Before any Cargo/test runner: statically audit selected tests/build runners for HOME/USERPROFILE, real `.aleph`, keyring, SQLite/vault, deletion and environment changes. Run only proven contained tests; record omissions.
- One Cargo process at a time. Check Windows available physical memory before every invocation; below 4GiB defer. `CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0`. Batch 2–3 changes before verification.
- Work only in isolated worktrees. Integration branch is the target of worker merges. Hook-triggered Cargo is subject to the same constraints; documentation-only commits bypass the unrelated existing workspace hook and state that explicitly.

## Review Focus

- Late history/RPC results after switching sessions must not replace the current screen.
- Same-session supersede and cursor advances must reject an already dispatched old answer under the Core lock.
- Duplicate user text, width reflow, and Step folding must preserve jump identity and reading position.
- Queued approvals/questions must route keyboard input to the actually rendered modal; failed submit preserves secret drafts only in memory.
- Stream channel loss and run termination without the last notification must recover from snapshots without blocking RPC responses.

## File structure and ownership

Line 1 owns `shared/ui_logic/src/transcript/**`, TUI `app/{mod,events,trace,tests}.rs`, `widgets/chat_area.rs`, `slash.rs`, `commands.rs`, and transcript hydration module. Line 2 first owns `shared/protocol/src/{events,clarification}.rs`, `src/clarification/**`, `src/gateway/handlers/clarification.rs`, and Core stream conversions; it then owns TUI modal/dialog/keys integration after line 1 merges. The coordinator owns `shared/client/src/connection.rs`, integration in `interfaces/tui/src/tui/mod.rs`, documentation, reviews, and serialized validation. Shared files are never edited concurrently.

### Task 1: Reducer adoption, import, and Step consumer (line 1)

**Files:** Modify shared reducer modules/tests; TUI app state/events/trace/tests, chat_area renderer and slash/commands; create focused hydration/detail modules as needed.

**Interfaces:** Keep `Transcript::entries`, `push_user`, `apply_live`, `apply_replay`, `finish_replay`. Add `adopt_live_run(&mut self, run_id: &str, now_ms: u64) -> Vec<Change>` and an import method for legacy history/system rows; `apply_replay` against live returns `NeedsResync`. Expose a TUI hydration result for the coordinator without performing RPC in AppState.

- [ ] Add behavioral tests for adopt idempotence, cross-run settlement, live/replay separation, final answer and nested tools. Inspect test source before deciding whether runnable.
- [ ] Make AppState's transcript a single reducer, move auxiliary trace effects out of the old projection, and delete duplicate transcript mutation code in the same change.
- [ ] Render Step with shared headlines/folding/tool/diff helpers; wire `/detail brief|full`, persisted device setting with injected-root tests, clear Step overrides on mode changes. Update nested tool traversal and leak assertions.
- [ ] Build completed-run hydration from ordered history and trace results with legacy fallback, active-run exclusion and explicit recovery state; stage replacement by session/generation.
- [ ] Fix cache invalidation for detail/expansion/theme/highlight/content; avoid hashing historical full text every frame.
- [ ] Coordinator reviews diff; after Task 2 batch, run audited shared-ui-logic/TUI targets, fix and commit independently.

### Task 2: Clarification identity and typed contracts (line 2)

**Files:** Create `shared/protocol/src/clarification.rs`, export from lib; modify protocol events, `src/clarification/{session,ask}.rs`, gateway handler and all producer/conversion sites found by exhaustive search.

**Interfaces:** Shared serializable resolve params/response and pending views preserve current wire names. Request identity is generated at register, stored on the entry and forwarded with run_id/question id. Optional expected request/question identity on resolve is checked while holding the same registry lock that advances the cursor. Existing clients omitting fields remain compatible.

- [ ] Add tests proving old wire fixtures decode, new identity survives producer→wire→consumer, stale request/cursor cannot consume a new waiter, and permission checks remain before mutation.
- [ ] Implement typed shared views; Core uses those exact shapes. Preserve Core-only option values and existing answer semantics.
- [ ] Include request_id on AskUser and ClarificationEnded; pending carries run_id. Update every enum constructor/pattern (including tests and other interfaces) to compile.
- [ ] Audit all selected Core test helpers and build scripts; report exact safe filters to the coordinator. Do not run Cargo independently.
- [ ] Coordinator reviews, integrates this branch before dependent modal work, validates with Task 1 batch, and commits.

### Task 3: Sticky user message and stable scrolling (line 1, after Task 1)

**Files:** TUI `widgets/chat_area.rs`, focused viewport module, `regions.rs`, app state/actions, `keys.rs`, slash/commands/help.

**Interfaces:** Add action `JumpToUser` carrying stable entry identity plus session/transcript generation for mouse hits; `/jump-user` resolves latest user through the same function. `build_visible_lines` produces measured user positions from the same pass used to render; identity anchors are resolved against current positions after reflow.

- [ ] Add renderer/action tests for identical user texts with different identities, visible/above/below target, multi-line CJK/emoji, narrow/tiny rectangles and overlay hit priority.
- [ ] Paint a one-row sticky message only when the entire latest user message is above the viewport; retain literal `[click to jump]`. Hide if that label cannot fit or only one chat row exists.
- [ ] Block click-through over the whole sticky row; modal routing supersedes chat regions. Reject stale session/generation targets.
- [ ] Jump places the original first line at the top subject to clamp; preserve identity plus intra-entry physical offset across streaming, folding and resize. Back-to-bottom clears the anchor.
- [ ] Add `/jump-user` help and completion; test actual mouse and command dispatch, then commit after scoped review and shared validation batch.

### Task 4: One modal owner and acknowledged submission (line 2, after Task 2 and Task 1 integration)

**Files:** New focused TUI interaction controller; `widgets/dialog.rs`, approval/render/keys, app events/state, and coordinator integration in `tui/mod.rs`.

**Interfaces:** Editing/submitting/reconciling/error state is keyed by session/request/question. Controller selects one active modal for both drawing and keys, queues requests, preserves drafts and supplies the expected ids for resolve. RPC effects return typed results tagged with generation.

- [ ] Add pure controller tests for simultaneous approval/question, queue/supersede, stale terminal frame and session drafts; renderer/key tests assert visible target equals action target.
- [ ] Replace overwrite-on-AskUser and close-all behavior with identity-scoped updates; display descriptions/header, multiselect and free text through existing question representation.
- [ ] Process resolve acknowledgement: close only resolved/nonstale/zero remaining; otherwise refresh pending. Failures preserve drafts; uncertain results reconcile without automatic resend.
- [ ] Esc hides and preserves a reopening path; Ctrl+C targets the dialog's run_id and refuses to guess missing ids. Release secret drafts on final/supersede and never echo answers into transcript/logs.
- [ ] Integrate attach/reconnect/completion pending refresh with bounded jobs, review behavior and commit.

### Task 5: Responsive I/O, stream-loss recovery and paginated tool output (coordinator with line 2)

**Files:** `shared/client/src/connection.rs`, TUI event loop, bounded background-RPC module, tool-output view/controller and existing trace RPC call sites.

**Interfaces:** Independent watch/counter exposes stream loss and connection generation; notification must not share the full stream queue. UI jobs carry session/request/generation, with one recovery and one page per identity in flight.

- [ ] Add tests proving full stream consumer never stalls a subsequent RPC response and that dropped frames advance the independent loss signal.
- [ ] Budget stream/topic drains (initial cap 64 per wake); continue remaining events on next wake. Move resolve/pending/approval poll/history/replay/tool output out of the UI input branch into bounded asynchronous work.
- [ ] Refresh pending and mark/recover transcript on loss/reconnect; defer active transcript rebuild until terminal. Ignore late results after session changes; preserve old display on failure.
- [ ] On explicit tool-detail open request `trace.tool_output` by session/tool id; render input/output, byte progress, truncated/source/expired/error; load-more is explicit, bounded and generation-scoped.
- [ ] Test slow I/O with burst streaming, cancellation/reconnect, page ordering/expired and terminal recovery; review and validate batch before commit.

### Task 6: Cleanup, documentation and final verification (coordinator)

**Files:** `docs/reference/{FEATURE_LOCATOR,TRANSCRIPT_RENDERING,SUBSYSTEM_ROUTING}.md`, applicable `qa/transcript_fold` scripts/docs, plan and validation ledger.

- [ ] Search old producer/filter/verbose/dedup paths; remove only after verifying current consumers. Retain shared Reasoning enum if other interfaces need it.
- [ ] Record actual production call sites, commands, recovery limitations, sticky behavior and CUT locations in bilingual reference notes.
- [ ] Run audited package/feature checks covering protocol/client/shared-ui-logic no-default and default/TUI/CLI; Core compile and safe clarification tests. Run formatting only for changed files; report baseline-wide failures separately. Run appropriate clippy and applicable QA when safe/available.
- [ ] Fresh final review against the approved spec; fix verified findings, repeat only affected checks, record missing live-server/manual terminal evidence explicitly.
- [ ] Commit final changes on integration branch, verify clean worktree/main untouched and report commit ids/results. No push or main merge.
