# Aleph Panel — Native Phone Chat Screen (single-agent v1)

**Date:** 2026-06-26
**Status:** Design approved → ready for implementation plan
**Scope owner:** panel (`aleph-panel`, `interfaces/webchat/`)

## 1. Problem

The phone form-factor (`<640px` → `FormFactor::Phone`) has native iOS screens only for
**Settings**. The **Chat** tab falls through to the wide (desktop) two-pane layout —
a session sidebar + chat surface squeezed onto a narrow column (cramped, not native).
Goal: a native single-column phone Chat that mirrors the Settings drill-in pattern,
**reusing the existing chat data layer** (R4: interface = pure I/O).

## 2. Scope

**In (v1):**
- Session **list** (landing) → open a conversation (**thread**) → **send** message →
  **stream** the reply → **stop** (abort) → **new chat**.
- Full message rendering (markdown, tool cards, day grouping, stick-to-bottom) via the
  **reused** `MessageList`.

**Out (deferred to later passes):**
- Teams / 群聊 UI (participants roster + task drawer) — **team sessions hidden from the
  phone list** in v1.
- Attachments, slash-commands, @-mentions, per-chat model override.
- Workspace / split pane, file drop-zone, session pin / rename / delete.

## 3. Architecture decision: hybrid reuse (R4-clean)

The data layer is reused untouched; only the **presentation** differs on phone.

| Reuse as-is | New (phone presentation) | Surgical edits to existing code |
|---|---|---|
| `ChatState` (app-root context, `Copy`) | session-list landing (iOS `.list`/`.cell`) | `MessageList` visibility `pub(super)` → `pub(crate)` |
| `ChatApi::{send, abort, history, new_session}` (`api/chat.rs`) | `PhoneComposer` (textarea + send + stop) | `PhoneTabBar` dynamic active state (today hardcodes Settings — `shell.rs:34`) |
| `sessions.list` RPC + `SessionEntry` shape | — | `PhoneShell` optional `footer` slot (pin composer above TabBar) |
| `subscribe_run_events` + `subscribe_topic("stream.*")` (`platform/wide/views/chat/events.rs:193`, `view.rs:36/53`) | — | `app.rs` Chat form-factor branch (parallel to `SettingsRouter`) |
| `MessageList` (`platform/wide/views/chat/messages.rs:106` — layout-agnostic) | — | `platform/phone/mod.rs` add `pub mod chat;` |

Rationale: the user chose a **minimal** composer, so reusing the heavy desktop `InputArea`
(which carries attachments / slash / @-mention palettes + floating-glass layout) is a poor
fit; a small new `PhoneComposer` calling `ChatApi` directly is cleaner and avoids stripping
machinery. `MessageList` *is* reused (it has no width/height assumptions; the flex parent
sizes it) so message rendering has full parity with zero duplication (honours DRY / 3× rule).

## 4. Files

```
src/platform/phone/chat/
├── mod.rs      — module wiring + landing/thread dispatch glue
├── list.rs     — PhoneChatList   (landing: sessions.list → iOS list, "+" new-chat)
├── thread.rs   — PhoneChatThread (subscribe on mount, MessageList + PhoneComposer, back)
└── composer.rs — PhoneComposer   (minimal: text + send + stop)
```

Edits: `app.rs`, `platform/phone/mod.rs`, `platform/phone/shell.rs`,
`platform/wide/views/chat/messages.rs` (visibility only).

## 5. Navigation (route-based, mirrors Settings)

The active session already lives in `ChatState.session_key` (not the URL), so routes carry
no session key:

- `/`     → Phone: `PhoneChatList`  (wide unchanged: `ChatView`)
- `/chat` → Phone: `PhoneChatThread`, back button → `/`  (wide: must also map `/chat` to
  chat mode — verify/extend `PanelMode::from_path`)
- Both routes keep the **Chat** tab highlighted in `PhoneTabBar`.
- Tap a session → load it into `ChatState` (`session_key`, `agent_id`, `project_root`) +
  `ChatApi::history` → `navigate("/chat")`.
- "**+**" (new chat) → `ChatApi::new_session(current_session_key, None)` → reset
  `ChatState` (messages cleared, new `session_key`) → `navigate("/chat")`.

Drill-in needs **no new master-detail signal** — like Settings, plain router navigation
does it. Session selection should reuse the canonical "select session" steps used by the
wide sidebar (`components/chat_sidebar.rs`); prefer extracting a shared `select_session`
helper over duplicating the steps.

## 6. Data flow & streaming (the critical wiring)

`subscribe_run_events` + `subscribe_topic("stream.*")` are set up **inside the wide
`ChatView`** (`view.rs:36`, `:53`). On phone `ChatView` is never mounted, so:

- **Thread mount:** call `subscribe_run_events(dashboard, chat, workspace)` +
  `subscribe_topic("stream.*")`; **tear down the returned sub id on unmount**. This mirrors
  `ChatView` and keeps the wide path 100% untouched (chosen over lifting the subscription
  to app-root, which is DRYer but changes wide lifecycle/teardown — avoided as risk).
- `subscribe_run_events` requires a `WorkspaceState` (desktop-only; it only mirrors tool
  payloads into a pane phone doesn't render) → phone passes a **throwaway
  `WorkspaceState::new()`** (or the app-root instance if one is provided there — confirm in
  plan). The tool payloads it collects are simply never displayed on phone.
- **Send:** non-empty guard → `ChatApi::send(state, text, Some(session_key), vec![], agent_id, project_root, None)` →
  streamed deltas flow through the subscription into `ChatState.messages` → `MessageList`
  re-renders (stick-to-bottom handles scroll).
- **Stop:** when `ChatState.phase` is Thinking/Streaming, show a stop control →
  `ChatApi::abort(state, run_id)` (run id from `ChatState.active_run_id`).

- **List mount:** `rpc_call("sessions.list", {})` → parse `SessionEntry[]` → **filter out
  team sessions** (see §9) → sort by `updated_at` desc → render. States: loading /
  empty ("No conversations yet") / error + retry / socket-not-ready ("Connecting…").

## 7. Components & layout

- **PhoneChatList** — wrapped in `PhoneShell title="Chat"` (landing, no back), with a "+"
  new-chat affordance in the top bar. Body: iOS `.list` of `.cell` rows — each cell shows
  `topic` (fallback: derived/"New chat"), a subtitle (last-activity / message count), a
  `.cell-chevron`. Tap → select + navigate.
- **PhoneChatThread** — top bar (title = session topic, back → `/`) + flex column:
  `MessageList` (flex-1, scrolls) + **`PhoneComposer` pinned above the TabBar**. Reuses
  `PhoneShell`'s top bar + bottom TabBar via a new optional `footer` slot on `PhoneShell`
  (so the composer pins without scrolling). The wide `SessionTabs` / `TeamParticipants` /
  `WorkspacePanel` / drop-overlay are **not** rendered.
- **PhoneComposer** — auto-resizing `<textarea>` + send button; send disabled while empty;
  while a run is active the send button becomes a **stop** button. No palettes, no
  attachments, no floating glass; it's a normal column sibling of `MessageList`.

## 8. Styling

iOS classes from `styles/ios.css`: `.list`, `.cell`, `.cell-leading`, `.cell-body`,
`.cell-title`, `.cell-sub`, `.cell-chevron`, `.tabbar`, `.tabitem`, `.tabitem-active`;
`PhoneShell` chrome + safe-area insets. The composer matches the iOS input idiom
(rounded field, send/stop affordance) above the home indicator. **Desktop layout bytes
unchanged.**

## 9. Open item to resolve in the plan

**Identifying team sessions** to hide them from the phone list. `sessions.list` may or may
not flag teams. Resolve during planning by inspecting the `sessions.list` payload / server
handler: a dedicated field, or an `agent_id` / `key` convention, or cross-referencing
`team.*` membership. Fallback if no clean signal exists: show all sessions and note the
limitation (team sessions would open in the single-agent thread).

## 10. Error handling

`sessions.list` fail → inline error + retry button; `ChatApi::history` fail → banner in the
thread; `ChatApi::send` fail → banner (reuse `ChatState.send_error` where natural); socket
not yet connected → "Connecting…" placeholder on the list.

## 11. Testing

Leptos view internals are not unit-testable; extract **pure helpers** and unit-test them
(`#[cfg(test)] mod tests`):
- team-session filter (given `SessionEntry[]` → non-team subset),
- sort by `updated_at` desc (stable, `None` last),
- `SessionEntry` deserialize from a `sessions.list` payload fixture,
- new-session state reset (messages cleared, session_key swapped).
Follow the existing phone-screen test style (Phase 1/2 added real unit tests in the crate).
Desktop bytes must remain byte-identical (no behavioural change to wide).

## 12. Non-goals / YAGNI

No teams UI, no attachments, no slash/@-mention palettes, no model-override picker, no
workspace pane, no drop-zone, no pin/rename/delete, no per-session URL routing, no
app-root subscription refactor. Each is a separate later decision.
