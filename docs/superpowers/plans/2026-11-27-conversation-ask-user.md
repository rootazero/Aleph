# Conversational `ask_user` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让现役 Leptos Panel、TUI、Channel 的选择、自填、提交和取消准确到达等待中的 `ask_user`，不误答新卡。 / Deliver exact, addressed answers or cancellation to the parked `ask_user` on the three current surfaces.

**Architecture:** Keep one Core waiter and the existing RPC; typed rich submissions, guarded legacy input, and channel replies enter the same transition logic. Finish Core before splitting client work; reuse the Panel and TUI interaction owners, and keep channel text sequential.

**Tech Stack:** Rust, serde, Tokio, JSON-RPC, Leptos/WASM, Ratatui, existing Python QA drivers.

**Spec:** [2026-11-27-conversation-ask-user-design.md](../specs/2026-11-27-conversation-ask-user-design.md)（已批准 / approved）。

## Global Constraints

- 只在 `refactor/conversation-ask-user` 的隔离 worktree 操作；不切换、修改或合并 `main`。 / Work and commit only on the isolated branch.
- 一到四题、600 秒时限；`clarify:<32-hex-request-uuid>:<cursor>:<1-based-index>` 不超过 64 字节。 / One to four questions; 600-second deadline; callbacks fit in 64 bytes.
- 同一 Core manager、Tool Capability、RPC 及可见性；不得新增依赖、注册表、回放器或执行授权。 / One manager, capability, existing RPC and visibility gate; no new dependency, registry, replay, or authorization path.
- 不迁移 TSX、预览、批注；不改变 scratchpad 计划决策、执行审批或 workflow `clarify` 的语义。 / TSX, previews, and notes stay out; preserve the other gates.
- Doc copy bilingual, code comments English. Do not log or snapshot drafts containing credentials. Disclose any skipped runtime gate.
- One writer per worktree; after the Core commits, independent client work may use separate worktrees rooted at the same commit. Integrate by review and cherry-pick; the parent owns cross-branch review and publishes final verification. Do not concurrently run jobs that overwrite a QA build or fixed port.

## Review Focus

1. 上一题的 `cancel` 在另一个窗口刚推进后才到：不能取消新题。 / Late cancel after another client advances: Task 1.
2. 旧 RPC 带两个空字段，不能误吃第一题；第二个批量答案无效时首题也不推进。 / Shape ambiguity and invalid second answer: Task 1.
3. 无身份终止和迟到 push / fetch 交错：不能移除后继或复活旧卡。 / Legacy terminal, reorder, and stale fetch: Tasks 3–4.
4. Panel 手机/宽屏网络失败和重连：不能回落到 `chat.send`，不能自动重发敏感草稿。 / Both composers and private draft recovery: Task 3.
5. Channel 收到下一题的送达错误，用户能主动重显；陈旧按钮不能通过普通消息绕回。 / Next-delivery failure and callback fallback: Tasks 5–6.

---

## File Map / 文件分工

| 层 / Layer | 文件 / Files | 职责 / Ownership |
|---|---|---|
| Wire / Core | `shared/protocol/src/{lib,clarification}.rs`; `src/clarification/{session,mod}.rs`; `src/gateway/handlers/clarification.rs` | Typed action DTO, one transition and result, visible-session boundary |
| Tool | `src/builtin_tools/ask_user.rs`; `src/clarification/ask.rs` | Validate model args; preserve lifecycle and initial delivery |
| Panel | `interfaces/webchat/src/api/clarification.rs`; `interfaces/webchat/src/components/ask_user_card.rs`; `interfaces/webchat/src/{context.rs,state/notifications.rs}` | Shared DTO, keyed draft and pending reconciliation |
| Panel consumers | `interfaces/webchat/src/platform/wide/views/chat/{events.rs,view.rs,messages.rs,composer/mod.rs}`; `interfaces/webchat/src/platform/phone/chat/{mod.rs,composer.rs}` | Subscribe/refresh, render the same card, remove both composer intercepts |
| TUI | `interfaces/tui/src/tui/{interaction.rs,mod.rs,widgets/dialog.rs,app/mod.rs,app/events.rs,app/tests.rs}` | Questionnaire owns drafts, keyboard, ticket, and acknowledgements |
| Channel | `src/clarification/{render,session,ask}.rs`; `src/gateway/inbound_router/{mod,command_handler}.rs` | Render addressed buttons, handle text/commands, recover from failed next delivery |
| Evidence | `qa/human_gate/{drive.py,run.sh,ask.tools.json}`; `qa/plan_handoff/drive_plan_handoff.py`; `qa/README.md`; `docs/reference/{FEATURE_LOCATOR,TOOL_SYSTEM}.md` | Assert tool results through the real gateway and document the shipped path |

### Task 1: Enforce the shared contract in Core / 在 Core 落实回传契约

**Files:** Wire / Core above. Tests inline in `shared/protocol/src/clarification.rs`, `src/clarification/session.rs` and `src/gateway/handlers/clarification.rs`.

**Interfaces:** In `aleph_protocol`, add `ClarificationActionParams { session_key: String, expected_request_id: String, expected_question_id: String, action: ClarificationAction }`; `ClarificationAction::{Answer { answer: ClarificationAnswerInput }, Submit { answers: Vec<ClarificationAnswerInput> }, Cancel {}}`; `ClarificationAnswerInput { question_id: String, answer: ClarificationInput }`; `ClarificationInput::{Text { text: String }, Choices { indices: Vec<u32> }}`; `ClarificationActionResponse { status: ClarificationActionStatus, resolved: bool, pending_questions: usize, stale: bool }`; `ClarificationActionStatus::{Advanced, Answered, Cancelled, Stale}`. Keep legacy DTOs readable until their in-tree callers migrate. Core exports `pub(crate) async fn resolve_action(&self, params: ClarificationActionParams) -> ResolveOutcome`. Add `ResolveOutcome::{Invalid(&'static str), Cancelled}` and update exhaustive matches; keep current `More` and waiter behavior. Gateway still exposes only `clarification.resolve` and `clarification.pending`.

- [ ] Write `typed_action_contract_round_trips`: deserialize the approved JSON sample, assert `Text { text: "1" }` vs `Choices { indices: vec![0, 2] }`, exact snake_case tags, and reject `action` with any legacy `reply` or `answers` field. Assert that `cancelled` projects to `resolved: false, pending_questions: 0, stale: false`.
- [ ] Write `typed_batch_is_atomic_and_preserves_intent` and `guarded_resolve_decides_once`: assert invalid Q2 (duplicate, out of range, wrong cardinality, wrong/duplicate/missing question ID, out-of-order, or extra) leaves `answered == 0` and the actual receiver pending; then valid submit yields raw `"1"` with `[]` and the exact registered values/indices for `[0,2]`. Race two guarded answers to Q1, assert `More` once and `Stale` once; late Q1 cancel must not cancel Q2. Close the receiver after liveness inspection and assert both terminal paths are stale. Use explicit synchronization, not timing sleeps.
- [ ] Write `resolve_rejects_unguarded_or_ambiguous_input_without_advancing` in Gateway tests: missing/blank either ID, missing payload, old `reply` and `answers` together (including empty fields), blank text, and extra answers cannot advance; guarded legacy single reply and valid partial batch still advance; mismatched valid identities return stale; a hidden/nonexistent session gets the existing not-found behavior.
- [ ] Run `cargo test --locked -p aleph-protocol --lib clarification` and `cargo test --locked -p alephcore --lib clarification::`; expect missing new types or the old behavior to fail. A known unrelated compile failure is not a red test; record it separately.
- [ ] Add the typed serde DTOs in `shared/protocol/src/clarification.rs` and re-export them in `shared/protocol/src/lib.rs`. Use `kind` tags, snake_case, and strict shapes; leave the read DTOs unchanged. For the temporary in-tree legacy request serializer, omit the unused `reply` or `answers` field rather than emitting both. Older binaries emitting both must upgrade, as must clients missing either guard. In `src/gateway/handlers/clarification.rs`, discriminate by *presence* of `action` before decoding; no malformed typed request may fall through to legacy. Honor visibility before manager access; validate exactly one supplied nonempty legacy field, then map `Invalid` to `INVALID_PARAMS`. Derive all response fields from one outcome.
- [ ] Implement `ClarificationManager::resolve_action` in `src/clarification/session.rs`. Reuse one write-locked transition for typed and old text input; compare expected identity and cursor before validation, prevalidate/build a whole vector before applying it, and keep expired/abandoned retirement. `Answer` requires the current question ID; `Submit` zips the ordered outstanding suffix only if lengths match. `Cancel` discards partial answers and uses the real sender; share the terminal removal and liveness logic rather than calling the producer's request-only cancel after unlocking. Publish frames after unlock. Do not duplicate `interpret_reply` for workflow.
- [ ] Run both test commands again and `cargo test --locked -p alephcore --lib gateway::handlers::clarification`; expect all passing, including existing scoped and abandonment tests. `git diff --check`, then commit Wire/Core files as `feat(clarification): apply addressed answers atomically`.

### Task 2: Validate tool input without tightening shared producers / 收紧工具入口

**Files:** `src/builtin_tools/ask_user.rs` and stale type comments in `src/clarification/mod.rs`. Keep the production `AskUserTool::new` registration unchanged.

**Interfaces:** Preserve `AskUserArgs`, `AskUserAnswer`, `AskUserOutput` shapes and `AskUserTool` execution; build `fn validate_options(options: &[ClarificationOption]) -> Result<(), String>` in the tool module, not the shared constructors.

- [ ] Write `ask_rejects_mixed_shapes_and_ambiguous_options_before_parking`: both grouped and populated flat fields, explicit blank ID, duplicate ID, blank prompt, blank label/value, case-folded duplicate display name, and cross-option value alias produce input error before registration. Same option with identical label/value is valid; one to four valid questions retain their order and `q1` fallback. Assert valid answer still yields `answer` and `answers[].selected_indices`, and cancelled yields no partial answers.
- [ ] Run `cargo test --locked -p alephcore --lib builtin_tools::ask_user`; expect new validation assertions to fail against the old behavior.
- [ ] Validate the exact production input shapes before calling `clarification::ask`; fold aliases using the same lowercase comparison as the text interpreter, but do not change its fallbacks or the `ClarificationRequest` constructors. Correct stale comments that say no production caller uses batches.
- [ ] Re-run the tool suite and `cargo test --locked -p alephcore --lib clarification::`; expect pass including unattended, timeout, withheld secret and receiver cleanup. `git diff --check`, then commit `fix(ask_user): validate model-supplied question shapes`.

### Task 3: Wire the current Leptos Panel / 接通现役面板

**Files:** Panel and Panel consumers in the file map; add strings to `interfaces/webchat/locales/{en,zh}.json`. Keep one `AskUserCard` for phone and wide.

**Interfaces:** `ClarificationApi::resolve_action(state: &DashboardState, params: ClarificationActionParams) -> Result<ClarificationActionResponse, String>` and `ClarificationApi::refresh_pending(state: &DashboardState) -> Result<(), String>` (retain internal list decoding only if it adds view projection). Use a single applied-snapshot generation for both chat layouts. Internal draft uses `ClarificationInput` keyed by question ID and tied to request/current question.

- [ ] Write `ending_a_does_not_remove_b_or_resurrect_a`, `older_fetch_never_overwrites_new_push`, and `question_submit_is_guarded_and_masks_secret_summary`: assert matched terminal discards A only, an identity-less terminal requests a snapshot, a lower cursor cannot regress, a superseded fetch does not change the view, a repeated activation creates one ticket, `Text { text: "1" }` is emitted unmodified, and a secret draft's summary never includes its contents. Pin both composer paths: no automatic answer/send while a card owns a pending question and no draft clearing or raw transcript echo; after an uncertain mutation only the read RPC is retried.
- [ ] Run `cargo test --locked -p aleph-panel --lib`; expect the new regression tests to fail against the old state behavior.
- [ ] Add the two API methods and share `PendingListResponse`/typed action DTOs. Remove the old resolve DTO, hand-written request variants and API-side outcome duplication once unused. Guard snapshots with revision/generation at start and apply; on terminal use `session_key` + nonempty `request_id`, retain a bounded recent-ended fence through the outstanding async work, and reconcile conflicts instead of deciding their order from UUIDs. Reuse existing mount, subscribe/reconnect and event ownership in both layouts. Event processing must not install an older cursor for the same identity.
- [ ] Update `AskUserCard` to stage all unanswered answers, preserve the same-key drafts, and send one `submit` or `cancel`. Disable all triggers and synchronously acquire the submission latch before spawning. On terminal ack apply only to its owner; on stale/transport error refresh pending without resending or converting anything into chat. Mask secret input and review, and release private drafts on replacement/advance/end. Drop string toggle and comma-encoding helpers.
- [ ] Remove implicit answer, echo and fallback from **both** `composer/mod.rs` (wide) and `composer.rs` (phone). Preserve unfinished ordinary chat text and show the existing card as the place to answer; check both mounts in a Panel run.
- [ ] Re-run `cargo test --locked -p aleph-panel --lib` and `just wasm`; expect the Panel tests and production artifact build to succeed. `git diff --check`, then commit `feat(panel): submit and reconcile addressed questionnaires`.

### Task 4: Wire the TUI questionnaire / 接通终端问卷

**Files:** TUI paths in the map. Avoid touching approval modal input and ownership.

**Interfaces:** Keep `InteractionController::submit(&mut self) -> Option<Submission>` and add `InteractionController::cancel_question(&mut self) -> Option<Submission>`. Change `Submission.params` to `ClarificationActionParams`; keep the existing generation/nonce ticket and feed `ClarificationActionResponse` through existing acknowledgement/reconcile. Stage independent drafts in the existing question-card owner, not shared with ordinary input dialogs.

- [ ] Write `questionnaire_stage_review_and_cancel_are_addressed`, `questionnaire_terminal_and_uncertain_ack_do_not_retarget`, and add to existing overlay tests: Enter in edit stages a choice or exact custom text, enables review only once every pending question has a draft, and review submits ordered IDs once. Assert Q1 can be edited before submit; secret review uses a filled marker, not `top-secret`; Esc hides/reopens, and explicit cancel sends `ClarificationAction::Cancel {}` scoped to displayed Q1. A terminal A plus stale fetch/late ack cannot affect B or resurrect A; uncertain writes never replay and reconciling the same live cursor preserves its masked draft without sending.
- [ ] Run `cargo test --locked -p aleph-tui --lib`; expect the new questionnaire tests to fail initially.
- [ ] Use typed drafts and `Submit` in `interfaces/tui/src/tui/interaction.rs`; reuse the nonce/generation acknowledgement ownership and route `Cancel` via `clarification.resolve` in `interfaces/tui/src/tui/mod.rs`. Preserve one-at-a-time editing as a local UI choice, with previous/next choices and a review step: Enter stages/advances (or submits from review), Esc still hides, use a distinct displayed shortcut from the dialog for Cancel, and keep approval keys isolated. The app-level ordinary composer is not an answer path. Reconcile after stale/uncertain results or identity-less terminals; old zero-identity projections are read-only until snapshot supplies both IDs.
- [ ] Run `cargo test --locked -p aleph-tui --lib` and `cargo test --locked -p aleph-tui --bin aleph-tui`; expect pass. `git diff --check`, then commit `feat(tui): review and submit addressed questionnaires`.

### Task 5: Bind Channel controls to the current question / 为通道按钮绑定身份

**Files:** Channel paths in the map. Keep `src/workflow/clarify.rs` on text-only `render`.

**Interfaces:** In `src/clarification/render.rs`, add `pub(crate) fn render_addressed(question: &ClarificationQuestion, cursor: usize, total: usize, request_id: &str) -> RenderedQuestion` and `pub(crate) fn parse_button_callback(payload: &str) -> Option<ClarificationButton>`, with `ClarificationButton { request_id: uuid::Uuid, cursor: usize, index: u32 }` (zero-based `index` after parsing). Manager provides `pub(crate) async fn resolve_channel_button(&self, session_key: &str, button: ClarificationButton) -> ResolveOutcome`, `pub(crate) async fn resolve_channel_text(&self, session_key: &str, reply: &str) -> ChannelReplyDisposition`, `pub(crate) async fn pending_channel_question(&self, session_key: &str) -> Option<RenderedQuestion>` and `pub(crate) async fn cancel_channel_question(&self, session_key: &str) -> ResolveOutcome`. Disposition distinguishes `NoQuestion`, `PrivateQuestion` (handled, never forwarded), and `Resolved(ResolveOutcome)` (apply only if consumed). Reuse Task 1 transition and locks.

- [ ] Write `old_or_malformed_callbacks_never_answer_or_become_chat`, `addressed_buttons_select_by_index_on_current_cursor`, and `question_commands_recover_failed_delivery_without_answering`: run A→B and Q1→Q2 stale cases, reject old and malformed reserved callbacks before generic or workflow dispatch, parse the max supported button within 64 bytes, and show numeric label `"2"` cannot steal the second indexed click. Simulate a failed Q2 send; `/question` renders live Q2 without consuming it, `/cancel-question` unblocks the actual waiter with `Cancelled`, and `/help` and `/stop` still bypass clarification. For a private question, no input or redisplay reaches chat and Channel cannot mutate it.
- [ ] Run `cargo test --locked -p alephcore --lib gateway::inbound_router` and `cargo test --locked -p alephcore --lib clarification::`; expect regression failures around index-only buttons or missing commands.
- [ ] Build addressed keyboards in `render_addressed`, use it from both initial channel send in `src/clarification/ask.rs` and live advance/re-display in `src/clarification/session.rs`. Keep shared text rendering without identity free of active keyboards; no map for handles. Parse only the issued shape, then convert from 1-based; compare UUID and cursor under the manager write lock, and delegate selection to typed validation. Do not make arbitrary input to the ordinary chat parser into an indexed button.
- [ ] In `src/gateway/inbound_router/mod.rs`, handle the `clarify:` namespace before ordinary text (legacy shapes get safe stale behavior). Replace the current `has_pending` + unguarded text write with channel-aware resolution under a single lock so a secret question cannot consume external text or leak it to chat. Use that same origin restriction for re-display/cancel. In `src/gateway/inbound_router/command_handler.rs`, add `pub(super) fn classify_clarification_command(text: &str) -> Option<ClarificationCommand>` with `ClarificationCommand::{Show, Cancel}` for exact `/question` and `/cancel-question`. Call it before slash bypass, append the verbs to `ROUTER_OWNED_HELP_LINES`, and make its ownership test consult the same classifier. Retain logging of failed next delivery without automatically resending. Remove `parse_clarify_index` and its tests in `command_handler.rs`; use addressed fixtures in router tests.
- [ ] Re-run both Core commands, plus `cargo test --locked -p alephcore --lib workflow::clarify`; expect pass and text-only workflow unchanged. `git diff --check`, then commit `fix(channel): address clarification controls to their question`.

### Task 6: Verify integration, clean compatibility residue, and document / 验证集成并更新参考文档

**Files:** QA and reference paths in the file map; revisit legacy DTO/helper exports only with `git grep` evidence. Read `docs/reference/DEVELOPMENT.md` and `qa/README.md` before invoking real QA. Correct current-state docs, not historical evidence.

**Interfaces:** QA uses the typed wire actions and current IDs from `clarification.pending`. `qa/plan_handoff` can use guarded legacy `reply` to cover compatibility, adding the live `expected_question_id`. Preserve existing QA drivers and scratch HOME; no production deploy.

- [ ] Update `qa/human_gate/drive.py` and fixture: stage `ask` checks returned `status: answered` **and** the mock provider's subsequent tool-result payload contains exact q1 selected indices/values and q2 custom `"1"` with no indices. Stage `stale` checks `status: stale` with original request and question IDs and that no tool result is added by the stale submission. Add `cancel` stage in `run.sh`/driver asserting actual tool result `status: cancelled` and no partial answers, with run continuation. Keep `text` sequential and `unattended` empty-pending checks; add useful recovery assertions without manufacturing transport availability. Migrate plan-handoff to both guards.
- [ ] Check each shared legacy DTO/helper with `git grep` after integrating both clients. Delete obsolete encoders/parsers and private helpers with no owners; keep intentionally supported legacy formats and round-trip tests. Run `cargo test --locked -p alephcore --lib --no-run` after API removal. Check formatting for touched Rust files per repository practice and run `git diff --check`.
- [ ] Run `just test-shared`, the Core `clarification::`, `builtin_tools::ask_user`, Gateway handler/router, and workflow suites, `cargo test --locked -p aleph-panel --lib`, and `just wasm`; expect all pass. Build the debug Gateway binary with `cargo build --locked -p alephcore --bin aleph-server` if not built from the integration HEAD, then run `./qa/human_gate/run.sh` with `ask`, `stale`, `cancel`, `text`, `unattended` and `./qa/plan_handoff/run.sh` with `handoff`, `deny`, `floor`; expect clean exit and real waiter/tool-result evidence for each named assertion. Before claiming UI runtime coverage, exercise the Leptos card (wide and phone) and TUI control flow against an available test Gateway; fixture/probe evidence is not third-party device coverage.
- [ ] Update `docs/reference/FEATURE_LOCATOR.md` §5.3, `docs/reference/TOOL_SYSTEM.md` ask_user entry and `qa/README.md` to name what ships, what was removed, backward-compat behavior, why authorization stays separate, and which platforms are not wired or verified. Use repo-relative paths. Commit as `test(clarification): verify staged answers through the gateway`, then run read-only whole-branch review on the integrated HEAD; fix any blocker and re-run the affected gates before handoff. Report worktree/commit(s), exact evidence, and what was not changed or validated.
