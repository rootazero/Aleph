# TSX Panel Ask User Option Preview Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add guarded `ask_user` questionnaires and expandable option previews to both TSX Panel hosts.

**Architecture:** Core projects an optional preview into the shared option DTO. TSX Chat uses the same Gateway read and write methods as the Leptos Panel. A chat-scoped questionnaire controller uses notifications to refresh the authoritative snapshot, and the shared TSX card renders and submits the drafts.

**Tech Stack:** Rust, Serde, NSH TSX, existing PanelSdk, Gateway JSON-RPC.

**Spec:** [TSX Panel questionnaire and preview design](../specs/2026-11-27-tsx-ask-user-option-preview-design.md).

## Global Constraints

- New preview text is limited to 16 KiB UTF-8 per tool call. Existing labels, descriptions, and answer wire shapes retain their semantics.
- Desktop and web share one TSX card. Use NSH Markdown with HTML, MDX, external image loading, and link navigation disabled.
- Reads come from `clarification.pending`. Writes are guarded by the session key, request ID, and first outstanding question ID. Reconciliation reads only.

## Review Focus

- Multibyte text crosses the preview limit in bytes before its character count reaches it. Task 1 tests this at the tool boundary.
- A notification for an older request can arrive after a newer one. Task 2 tests overlapping reads and fetches the authoritative snapshot on notifications.
- Another surface can answer the first question while the user is reviewing. Task 2 tests dropping its draft and preserving later drafts.
- Toggling the preview on an unselected option must preserve selection. Task 2 tests through the rendered card and the recorded RPCs.
- A secret field can be filled, reviewed, and returned to editing. Task 2 tests masking and absence of the secret in rendered review text.

---

### Task 1: Extend the Core read projection

**Files:** `src/builtin_tools/ask_user.rs`, `src/clarification/mod.rs`, `shared/protocol/src/clarification.rs`, and existing DTO fixture literals if required.

**Interfaces:** `AskUserChoice::Detailed` adds `preview: Option<String>`. `AskUserOption` adds `preview: Option<String>` with a default and omission when absent. `question_view(&ClarificationQuestion) -> AskUserQuestion` remains the common projection. Existing writes keep their types.

- [ ] Add a focused tool test that deserializes existing and preview-equipped options, checks the projected option's label/description/preview, and checks aggregate multibyte oversize and the exact limit. Verify the preview is present in both the event and pending snapshot.
- [ ] Run the focused test and confirm the missing preview behavior.
- [ ] Normalize blank previews and reject aggregate byte counts over `16 * 1024` during `AskUserTool::build_request` before a wait can start. Carry the preview through `ClarificationOption` and `question_view`.
- [ ] Run focused `ask_user`, Gateway clarification, and `aleph-protocol` tests. Commit the read projection and its tests once they pass.

### Task 2: Mount the shared TSX questionnaire

**Files:** `interfaces/panel/modules/Chat.tsx`, new `interfaces/panel/modules/chat/Questionnaire.tsx` and `interfaces/panel/modules/chat/clarification.ts`, TSX tests alongside the Chat module tests.

**Interfaces:** `Questionnaire` receives `sdk: PanelSdk` and `session: Signal<string | null>` from `Chat`. The controller reads `clarification.pending` and sends `clarification.resolve` with `{session_key, expected_request_id, expected_question_id, action}`. `action` is `{kind:"cancel"}` or `{kind:"submit", answers:[{question_id,answer}]}`, using `answer` of `{kind:"choices",indices}` or `{kind:"text",text}`.

- [ ] Test the mounted Chat path with a fake SDK. Verify notifications and reconnect trigger reads, delayed snapshots cannot replace newer state, lost responses do not retry a mutation, and the same request preserves still-outstanding drafts across cursor changes.
- [ ] Add interaction tests that expand preview without selecting, submit multi-choice indices and numeric custom text together after review, guard duplicate submission, and cancel with the right IDs. Cover secret review and session-switch cleanup.
- [ ] Implement the controller with generation checks on reads, one in-flight mutation per presented request, and state scoped to the selected session and request.
- [ ] Implement the shared card with accessible independent choice and preview controls, masked secret editing, and explicit review/submit/cancel. Mount it above the composer in Chat and guard ordinary send while the question is active. Leave the composition draft intact.
- [ ] Run `just panel-tsx-test` and commit the TSX card and tests once they pass.

### Task 3: Verify both host entry points and document support

**Files:** `docs/reference/FEATURE_LOCATOR.md` and `qa/README.md` near existing questionnaire guidance. Preserve the preceding iteration's historical design.

- [ ] Run `just panel-tsx-bundle`, `just panel-tsx-web`, and the relevant Leptos/TUI and Gateway questionnaire tests. Confirm both TSX artifacts contain the mounted questionnaire.
- [ ] Exercise packaged TSX against an isolated real Gateway. Confirm expand, guarded submission, exact returned choice/text, and disappearance after a terminal state the client missed while disconnected. Record which host was exercised and which was built.
- [ ] Update the current reference and QA guidance in Chinese and English. Run `git diff --check` and commit the verification documentation.
