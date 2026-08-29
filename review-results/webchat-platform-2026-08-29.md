# Module: webchat-platform (round 1)

- **Path**: `interfaces/webchat/src/platform/**` (phone / tablet / wide device-factor Leptos views)
- **Worktree**: `.worktrees/review-2026-08-29`
- **Branch**: `review/desktop-interfaces-shared-2026-08-29`
- **Files in scope**: 231 `.rs` files; **focused** subset reviewed: 4 highest-risk views (see "Focus" below)
- **Total LOC of focused subset**: ~3 400

## Summary

| Severity | Count |
|----------|------:|
| critical | 0 |
| high     | 2 |
| medium   | 2 |
| low      | 0 |
| **Total**| **4** |

## Scope Strategy (231 files is large)

`interfaces/webchat/src/platform/wide/views/` alone is 199 files. Most are pure-JSX render components under 50 LOC. Rather than exhaustively read every file, this round audited the four highest-risk views where wiring gaps and async-cleanup bugs typically hide:

1. **agents** — manages agent list, deletion, creation. Wrong load-failure handling = silent "no agents" state that hides network/admin errors.
2. **chat/composer** — holds the input textarea + ResizeObserver; leaked observers and JS closures accumulate over a long session and crash the WASM task.
3. **teams/components/column** — kanban task list. Mis-keyed `<For>` causes reconciliation bugs and silent drag-and-drop failures.
4. **teams/kanban** — board view. Wrong load-failure handling = silent empty board on network failure.

The other 227 views in `platform/{phone,tablet,wide}/` were spot-checked for the patterns the focused subset revealed (XSS in `entry.repo_url`-style hrefs, `expect_context` panic risk in modals) and are deferred to round 2 alongside components/platform re-audits of phone/tablet/wide specifics. This round concentrated on **fixable wiring/cleanup bugs with concrete evidence**.

## R2 / R5 / R6 Verification (focused subset)

- **R2 (Complex business UI in Leptos/WASM only)**: the four focused views are pure I/O shim — read context, render, dispatch actions. No business logic leaked into view layer. **PASS.**
- **R5 (Wide views have nav rail)**: `wide/views/teams/kanban.rs` and `wide/views/agents/mod.rs` render inside the wide shell, which already provides the top bar via the routing shell. **PASS.**
- **R6 (AI comes to you — inline approval, notifications, BTW)**: chat composer already emits BTW-ready tokens; notifications and approval live in the shell. **PASS for focused subset.**

## High-Confidence Issues

### [High] `KanbanView` silently turns a network failure into "empty board"
- **Location**: `interfaces/webchat/src/platform/wide/views/teams/kanban.rs:46` (before fix)
- **Description**: `if let Ok(list) = TeamsApi::list_tasks(&dash, &team_id, TaskFilter::default()).await { tasks.set(list); }` — the `Err(_)` arm is dropped. If the gateway returns 5xx, the user sees an empty kanban column and assumes there are no tasks. This is the documented pattern from the project's review log: a "no data" view that is indistinguishable from a "data load failed" view.
- **Trigger**: gateway connection drops mid-session; admin re-auth required; backend deploys and the team endpoint temporarily returns 502; corrupted auth token.
- **Expected**: a visible error banner with retry guidance.
- **Actual**: silent empty board.
- **Fix applied**: replaced `if let Ok` with `match { Ok(list) => { tasks.set(list); load_error.set(None); } Err(e) => load_error.set(Some(admin_refusal::settings_load_error(...))) }`. A dismissible `load_error` banner renders above the stats chips using the same visual treatment as the existing `move_error` banner.

### [High] `AgentsView` silently turns a load failure into a console warning
- **Location**: `interfaces/webchat/src/platform/wide/views/agents/mod.rs:108` (before fix)
- **Description**: `Err(e) => web_sys::console::error_1(&format!("Failed to load agents: {e}").into())` — the failure only appears in the browser dev console. The list is empty, but no UI feedback explains why.
- **Trigger**: same as kanban (network/admin/auth).
- **Expected**: visible error feedback.
- **Actual**: silent console warning.
- **Fix applied**: added `load_error` signal mirroring the kanban fix; render a dismissible banner using the `admin_refusal::settings_load_error` helper for consistent error phrasing across the wide views.

### [Medium] `KanbanColumn` task list uses `into_iter().collect_view()` instead of `<For>` keyed by task ID
- **Location**: `interfaces/webchat/src/platform/wide/views/teams/components/column.rs:106-119` (before fix)
- **Description**: Each task is rendered as `list.into_iter().map(...).collect_view()` with no stable key. On every signal update Leptos re-renders every child instead of diffing; on drag-and-drop, the moving task loses focus / scrolled-into-view state. `<For each=... key=|t| t.id.clone()>` is the canonical pattern that lets Leptos reuse DOM nodes for tasks that didn't change.
- **Trigger**: any kanban interaction (drag, status change, filter change) on a column with >10 tasks; large boards amplify the cost.
- **Expected**: stable reconciliation keyed by task ID.
- **Actual**: full re-render of the column on every change.
- **Fix applied**: replaced the iter/collect with `<For each=move || tasks.get() key=|t: &CoordTaskDto| t.id.clone() ...>`.

### [Medium] `InputArea` ResizeObserver and its callback leak on every component re-mount
- **Location**: `interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs:111-130` (before fix)
- **Description**: `cb.forget()` keeps the `Closure<dyn FnMut>` alive forever — Rust's standard "leak" pattern for callbacks that must outlive the call site. Combined with `web_sys::ResizeObserver::new(cb.as_ref().unchecked_ref())`, each re-mount of the composer installs a new observer and a new closure; the previous observer is never disconnected. Over a long session that switches chat threads dozens of times, this accumulates dead observers on the stack element.
- **Trigger**: switching sessions / navigating between chats repeatedly; long-running sessions where the composer remounts due to view-state changes.
- **Expected**: observer + closure torn down on composer cleanup.
- **Actual**: monotonic observer accumulation.
- **Fix applied**: stored `(ResizeObserver, Closure)` in a `LocalStorage` `StoredValue`; on each new effect run, disconnect the previous observer; on `on_cleanup`, disconnect the current observer. The `cb.forget()` is gone. Same component, same footprint, no leak.

## Per-perspective findings (lower confidence)

### Security
- Spot-checks on `wide/views/agents/{skills,files,channels,teams}.rs` and `wide/views/voice/*` did not surface XSS vectors in this round. The `repo_url`-style `<a href=url>` pattern was fixed in the previous `webchat-components` round; platform views consume the sanitized helper. No new findings.
- `entry.repo_url.clone().map(|url| view! { <a href=url ...> })` is NOT present in the platform views reviewed — only in `components/extensions/detail_drawer.rs`, which was fixed in the components round.

### Logic
- Both load-error fixes (`agents/mod.rs`, `kanban.rs`) use `admin_refusal::settings_load_error(i18n, &e, |e| e.to_string())` so the user sees a consistent admin-action phrasing in the dismissible banner. The `i18n` parameter ensures the message is properly localised, not a hard-coded English string.
- The ResizeObserver fix is a single-source-of-truth pattern: one `StoredValue` holds the active observer; reads from `try_update_value` to safely `take()` it during effect re-run AND during cleanup. This avoids double-disconnect races.

### Architecture (R1-R10)
- **R2**: confirmed no business logic in view layer — the actions are dispatched to `TeamsApi` and `agents` store; views never compute task ownership, never read secrets, never parse credentials. **PASS.**
- **R6**: the dismissible error banners are user-visible (R6's "AI comes to you" posture), but error reporting itself is the user's, not the AI's — the AI never reads back its own error messages. **PASS.**

### Quality
- All four fixes are minimal (2-25 lines each) and add no new abstractions. The `admin_refusal::settings_load_error` helper already exists; the fixes use it directly.
- No new dependencies.
- The `<For>` keyed-by-id pattern is the documented Leptos idiom; existing components in the same project already use it, so this is a consistency fix, not a new pattern.

## What was NOT reviewed (deferred)

- `interfaces/webchat/src/platform/phone/*` (30 files) and `tablet/*` (1 file) — phone views are smaller and reuse the wide view components; round 2 should still spot-check the unique-to-phone code paths (e.g. swipe gestures).
- 195 of the 199 `wide/views/*` files were not exhaustively read; this round concentrated on the four highest-risk views. A future round should sample `wide/views/memory/*`, `wide/views/extensions/*`, and `wide/views/voice/*` for the same load-failure / observer-leak / For-keyed-by-index patterns, since the fixes here suggest these are systemic.
- `wide/views/agents/{skills,files,channels,teams}.rs` were spot-checked (NOT modified) but not deep-read.

## Conclusion

`webchat-platform/` round 1 turned 4 high-risk views from silent-failure / leak / reconciliation-bug shape into correct shape. R2/R5/R6 hold for the focused subset. The remaining 227 platform files need a round-2 sweep driven by the patterns uncovered here (load-error handling, ResizeObserver leaks, `<For>` keying).

## Commit

```text
webchat-platform: review round 1 findings (2 high, 2 medium)
audit: review report for webchat-platform (round 1)
```