# Teams Kanban Lifecycle Wiring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Teams Kanban Panel faithfully surface all stored task statuses and reach every existing lifecycle RPC, fixing the "tasks vanish in waiting_review/paused/skipped" bug and connecting 4 dead API wrappers.

**Architecture:** Pure last-mile wiring in the `aleph-panel` (Leptos/WASM) crate. The core 10-state machine (`src/agents/swarm/tasks/mod.rs`) and 6 gateway lifecycle RPCs (`teams.task.{pause,resume,retry,skip}`, `teams.workflow.{approve,reject}_step`) already exist and are unchanged. We add 3 board columns, 2 thin API wrappers (`task_approve`/`task_reject`), and replace the drawer's 4 fixed buttons with a status-gated action set whose validity is computed by one pure, unit-tested function. Side-effecting transitions route through the dedicated backend verbs; only the no-side-effect status writes (start/complete/fail/cancel) keep the generic `teams.update_task` PATCH.

**Tech Stack:** Rust, Leptos 0.6 (CSR/WASM), `leptos_i18n` 0.6 (build-time locale codegen), `serde_json`. Crate: `aleph-panel` at `interfaces/webchat/`.

## Global Constraints

- **Scope:** `interfaces/webchat/` only. **Zero `src/` (core) changes. Zero new dependencies.** Verbatim from spec §1/§8.
- **Branch isolation:** All work in a new git worktree branch (e.g. `feat/teams-kanban-lifecycle-wiring`). **Never commit to `main`.** (Task protocol + project EnterWorktree convention.)
- **Commit format:** English, `<scope>: <description>` (e.g. `teams: surface waiting_review/paused/skipped kanban columns`).
- **节制 cargo (project mandate):** Scoped `cargo check -p aleph-panel` at task milestones only — never a full-workspace build, never `cargo test --all`, never an `alephcore` build (no core change → no core risk). One `cargo test -p aleph-panel actions_for_status` for the single unit test.
- **Transition routing (spec §3):** Side-effecting transitions (pause/resume/skip/retry/approve/reject) MUST route through their dedicated backend verbs. **Never widen `teams.update_task` PATCH** to carry these (avoids drift from the core state machine).
- **i18n parity (leptos_i18n):** Every new key MUST be added to **both** `locales/en.json` AND `locales/zh.json`. A key present in one locale but missing in the other fails the build-time codegen. Referencing a key that exists in neither is a compile error.
- **Redlines:** R8 (we only expose existing RPCs as buttons — no GUI config forms) · R7/R10 (no deterministic recovery/judgment logic — the buttons are dumb verb dispatchers) · R2/R4 (Panel stays pure I/O).

---

### Task 1: i18n keys for new columns + actions (en + zh)

Adds the locale strings the board (Task 3) and drawer (Task 4) will reference. Must land first so later tasks compile.

**Files:**
- Modify: `interfaces/webchat/locales/en.json` (the `teams.kanban.columns` block at ~line 1604, `teams.kanban.actions` block at ~line 1628)
- Modify: `interfaces/webchat/locales/zh.json` (same blocks, same line numbers — file mirrors en.json)

**Interfaces:**
- Produces (i18n keys consumed by Tasks 3 & 4):
  - `teams.kanban.columns.waiting_review`, `teams.kanban.columns.paused`, `teams.kanban.columns.skipped`
  - `teams.kanban.actions.pause`, `.resume`, `.skip`, `.retry`, `.approve`, `.reject`, `.reject_reason_placeholder`

- [ ] **Step 1: Add the 3 new columns + 6 new actions + reject placeholder to `en.json`**

In `interfaces/webchat/locales/en.json`, change the `columns` block (currently ends at `"cancelled": "Cancelled"`):

```json
      "columns": {
        "pending": "Pending",
        "blocked": "Blocked",
        "in_progress": "In Progress",
        "waiting_review": "Waiting Review",
        "paused": "Paused",
        "completed": "Completed",
        "skipped": "Skipped",
        "failed": "Failed",
        "cancelled": "Cancelled"
      },
```

And change the `actions` block (currently `start`/`complete`/`fail`/`cancel`/`new_task`) to:

```json
      "actions": {
        "start": "Start",
        "complete": "Complete",
        "fail": "Fail",
        "cancel": "Cancel",
        "pause": "Pause",
        "resume": "Resume",
        "skip": "Skip",
        "retry": "Retry",
        "approve": "Approve",
        "reject": "Reject",
        "reject_reason_placeholder": "Reason (optional)…",
        "new_task": "New Task"
      },
```

- [ ] **Step 2: Add the identical keys (translated) to `zh.json`**

In `interfaces/webchat/locales/zh.json`, change the `columns` block to:

```json
      "columns": {
        "pending": "待处理",
        "blocked": "受阻",
        "in_progress": "进行中",
        "waiting_review": "待审批",
        "paused": "已暂停",
        "completed": "已完成",
        "skipped": "已跳过",
        "failed": "失败",
        "cancelled": "已取消"
      },
```

And change the `actions` block to (keep the existing `new_task` value already present in zh.json):

```json
      "actions": {
        "start": "开始",
        "complete": "完成",
        "fail": "标记失败",
        "cancel": "取消",
        "pause": "暂停",
        "resume": "恢复",
        "skip": "跳过",
        "retry": "重试",
        "approve": "批准",
        "reject": "驳回",
        "reject_reason_placeholder": "驳回理由（可选）…",
        "new_task": "新建任务"
      },
```

> Note: the existing `en.json`/`zh.json` `actions` blocks already contain `start`/`complete`/`fail`/`cancel`/`new_task` with translations — preserve those exact values; only insert the 6 new verbs + placeholder. Match each file's existing indentation (6 spaces for the block key, 8 for entries).

- [ ] **Step 3: Verify the locale JSON is valid and codegen accepts it**

Run: `cargo check -p aleph-panel`
Expected: PASS (compiles clean). The `leptos_i18n` build script regenerates typed accessors from both locale files; mismatched keys across locales would fail here. No code references the new keys yet, so this only proves the JSON parses and locales stay in parity.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/locales/en.json interfaces/webchat/locales/zh.json
git commit -m "teams: add i18n keys for kanban waiting_review/paused/skipped columns and lifecycle actions"
```

---

### Task 2: `task_approve` + `task_reject` API wrappers

Adds the 2 missing thin client wrappers over the existing `teams.workflow.{approve,reject}_step` RPCs. The 4 sibling wrappers (`task_pause/resume/retry/skip`) already exist at `api/teams.rs:388-417` and are unchanged (Task 4 gives them their first callers).

**Files:**
- Modify: `interfaces/webchat/src/api/teams.rs` (insert after `task_skip`, i.e. after line 417, before the `task_trace` doc-comment at line 419)

**Interfaces:**
- Consumes: `DashboardState::rpc_call(&self, method: &str, params: serde_json::Value) -> Result<Value, String>` (existing); `json!` macro already imported in this file (used by the sibling wrappers).
- Produces (consumed by Task 4):
  - `TeamsApi::task_approve(state: &DashboardState, task_id: &str) -> Result<(), String>`
  - `TeamsApi::task_reject(state: &DashboardState, task_id: &str, reason: Option<&str>) -> Result<(), String>`

Backend contract (verified in `src/gateway/handlers/teams/workflow.rs:220-372`): both RPCs accept `WorkflowStepReviewParams { task_id: String, reviewer_kind: String (default "user"), reviewer_id: Option, comment: Option }`. `reviewer_kind` defaults to `"user"` — exactly the panel reviewer — so the wrappers omit it. `task_reject`'s optional `reason` maps to `comment` (recorded as the task result + a review comment).

- [ ] **Step 1: Add both wrappers**

Insert into the `impl TeamsApi` block in `interfaces/webchat/src/api/teams.rs`, immediately after the `task_skip` function (after line 417):

```rust
    /// teams.workflow.approve_step — approve a waiting-review task; the
    /// backend stamps the latest run Approved and transitions the task to
    /// Completed (downstream dependents unblock). `reviewer_kind` defaults
    /// to "user" on the server, so the panel sends only the task id.
    pub async fn task_approve(state: &DashboardState, task_id: &str) -> Result<(), String> {
        state
            .rpc_call(
                "teams.workflow.approve_step",
                json!({ "task_id": task_id }),
            )
            .await
            .map(|_| ())
    }

    /// teams.workflow.reject_step — reject a waiting-review task; the backend
    /// stamps the latest run Rejected and transitions the task to Failed
    /// (dependents stay blocked). An optional `reason` is recorded as the
    /// task result and a review comment.
    pub async fn task_reject(
        state: &DashboardState,
        task_id: &str,
        reason: Option<&str>,
    ) -> Result<(), String> {
        let mut params = serde_json::Map::new();
        params.insert("task_id".to_string(), Value::String(task_id.to_string()));
        if let Some(r) = reason.filter(|s| !s.trim().is_empty()) {
            params.insert("comment".to_string(), Value::String(r.to_string()));
        }
        state
            .rpc_call("teams.workflow.reject_step", Value::Object(params))
            .await
            .map(|_| ())
    }
```

> `Value` and `json!` are already imported at the top of this file (the existing wrappers use both). No new imports.

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p aleph-panel`
Expected: PASS. (The wrappers have no callers yet — that's fine; they're `pub` so no dead-code warning. Task 4 wires them.)

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/api/teams.rs
git commit -m "teams: add task_approve/task_reject panel API wrappers over workflow review RPCs"
```

---

### Task 3: 9-column board (surface waiting_review / paused / skipped)

Fixes the visibility bug: tasks in `waiting_review`, `paused`, `skipped` currently match no column filter and vanish. Adds their columns and corrects the stale doc comment.

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/teams/components/board.rs` (whole file — it's 84 lines)

**Interfaces:**
- Consumes: i18n keys from Task 1 (`teams.kanban.columns.{waiting_review,paused,skipped}`); existing `KanbanColumn` component; existing `tasks_with_status` helper (line 77).
- Produces: nothing other tasks consume.

- [ ] **Step 1: Update the doc comment and add 3 derived signals**

In `interfaces/webchat/src/platform/wide/views/teams/components/board.rs`, change line 1 from:

```rust
//! `KanbanBoard` — five-column responsive layout grouping tasks by derived status.
```

to:

```rust
//! `KanbanBoard` — nine-column responsive layout grouping tasks by stored
//! status. Every stored `CoordTaskStatus` maps to exactly one column so no
//! task is ever silently dropped from the board (`unsatisfiable` folds into
//! Blocked, matching the core "derived blocked" semantics).
```

Then, immediately after the `in_progress` signal (line 27) and before `completed` (line 28), add:

```rust
    let waiting_review = Signal::derive(move || tasks_with_status(&tasks.get(), "waiting_review"));
    let paused = Signal::derive(move || tasks_with_status(&tasks.get(), "paused"));
```

And immediately after the `completed` signal (line 28) and before `failed` (line 29), add:

```rust
    let skipped = Signal::derive(move || tasks_with_status(&tasks.get(), "skipped"));
```

- [ ] **Step 2: Render the 3 new columns in lifecycle order**

In the same file's `view!` block, insert a `Waiting Review` and `Paused` column between the existing `in_progress` column (ends line 54) and the `completed` column (starts line 55):

```rust
            <KanbanColumn
                title=t_string!(i18n, teams.kanban.columns.waiting_review).to_string()
                tasks=waiting_review
                on_card_click=on_card_click
                empty_label=empty_label()
            />
            <KanbanColumn
                title=t_string!(i18n, teams.kanban.columns.paused).to_string()
                tasks=paused
                on_card_click=on_card_click
                empty_label=empty_label()
            />
```

And insert a `Skipped` column between the existing `completed` column (ends line 60) and the `failed` column (starts line 61):

```rust
            <KanbanColumn
                title=t_string!(i18n, teams.kanban.columns.skipped).to_string()
                tasks=skipped
                on_card_click=on_card_click
                empty_label=empty_label()
            />
```

Final column order top-to-bottom in the markup: pending · blocked · in_progress · **waiting_review** · **paused** · completed · **skipped** · failed · cancelled. The grid (`repeat(auto-fit, minmax(220px, 1fr))`) wraps responsively — no layout change needed.

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p aleph-panel`
Expected: PASS. If it fails with an unknown-i18n-key error, Task 1 didn't land both locales — fix there first.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/platform/wide/views/teams/components/board.rs
git commit -m "teams: surface waiting_review/paused/skipped kanban columns (fix vanishing tasks)"
```

---

### Task 4: Status-gated drawer actions (pure gating fn + wire 6 verbs + reject flow)

Replaces the drawer's 4 fixed buttons with a status-gated action set. Adds one pure, unit-tested function (`actions_for_status`) that decides which actions a status exposes, then renders + dispatches them. Side-effecting verbs route through the dedicated wrappers (Task 2 + existing); start/complete/fail/cancel keep `update_task`.

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/teams/components/task_drawer.rs` (add enums + pure fn + test near top; rewrite the transient-state reset effect, add `run_verb`/`submit_reject` closures, and replace the footer markup at lines 176-271)

**Interfaces:**
- Consumes: `TeamsApi::{task_pause,task_resume,task_skip,task_retry}` (existing, `api/teams.rs:388-417`); `TeamsApi::{task_approve,task_reject}` (Task 2); i18n action keys (Task 1); existing `patch_status` closure, `busy`/`error`/`open_for`/`on_changed` signals, `ActionButton` component, `event_target_value` (leptos prelude).
- Produces: `fn actions_for_status(status: &str) -> Vec<TaskAction>` (unit-tested); `enum TaskAction`; `enum Verb`.

- [ ] **Step 1: Write the failing unit test for the pure gating function**

At the very bottom of `interfaces/webchat/src/platform/wide/views/teams/components/task_drawer.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use super::{actions_for_status, TaskAction};

    #[test]
    fn gating_matches_lifecycle_rules() {
        use TaskAction::*;
        assert_eq!(actions_for_status("pending"), vec![Start, Pause, Skip, Cancel]);
        assert_eq!(actions_for_status("blocked"), vec![Pause, Skip, Cancel]);
        // "unsatisfiable" (derived blocked) mirrors blocked exactly.
        assert_eq!(actions_for_status("unsatisfiable"), actions_for_status("blocked"));
        assert_eq!(actions_for_status("in_progress"), vec![Complete, Fail, Pause, Cancel]);
        assert_eq!(actions_for_status("waiting_review"), vec![Approve, Reject, Skip, Cancel]);
        assert_eq!(actions_for_status("paused"), vec![Resume, Cancel]);
        assert_eq!(actions_for_status("failed"), vec![Retry]);
        // Terminal + unknown statuses expose no actions.
        for s in ["completed", "skipped", "cancelled", "garbage"] {
            assert!(actions_for_status(s).is_empty(), "{s} must be terminal/inert");
        }
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p aleph-panel actions_for_status`
Expected: FAIL to **compile** with "cannot find function `actions_for_status`" / "cannot find type `TaskAction`" (they don't exist yet).

- [ ] **Step 3: Add the `TaskAction` / `Verb` enums and the pure `actions_for_status` function**

Near the top of `task_drawer.rs`, after the `use` block (after line 13), add:

```rust
/// A lifecycle action the drawer can offer for a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskAction {
    Start,
    Complete,
    Fail,
    Cancel,
    Pause,
    Resume,
    Skip,
    Retry,
    Approve,
    Reject,
}

/// The dedicated side-effecting backend verbs (everything except the four
/// plain status writes, which go through `update_task`, and Reject, which
/// needs a reason and is handled separately).
#[derive(Debug, Clone, Copy)]
enum Verb {
    Pause,
    Resume,
    Skip,
    Retry,
    Approve,
}

/// Which lifecycle actions the drawer offers for a task in `status`.
///
/// Pure + total so it is unit-testable without a DOM. The backend remains the
/// final authority on transition validity; this only hides the obviously
/// invalid actions so the UI never offers a guaranteed no-op. Terminal and
/// unknown statuses expose nothing.
fn actions_for_status(status: &str) -> Vec<TaskAction> {
    use TaskAction::*;
    match status {
        "pending" => vec![Start, Pause, Skip, Cancel],
        "blocked" | "unsatisfiable" => vec![Pause, Skip, Cancel],
        "in_progress" => vec![Complete, Fail, Pause, Cancel],
        "waiting_review" => vec![Approve, Reject, Skip, Cancel],
        "paused" => vec![Resume, Cancel],
        "failed" => vec![Retry],
        _ => vec![],
    }
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p aleph-panel actions_for_status`
Expected: PASS (`test tests::gating_matches_lifecycle_rules ... ok`).

- [ ] **Step 5: Add reject-flow state and reset it when the drawer target changes**

In the `TaskDetailDrawer` component body, after the `comment_busy` signal (line 33), add:

```rust
    // Reject flow: clicking Reject reveals an inline optional-reason input.
    let reject_open = RwSignal::new(false);
    let reject_reason: RwSignal<String> = RwSignal::new(String::new());
```

Then inside the existing reset `Effect` (lines 37-47), after `new_comment.set(String::new());` (line 42), add:

```rust
        reject_open.set(false);
        reject_reason.set(String::new());
```

- [ ] **Step 6: Add the `run_verb` and `submit_reject` closures**

In the same component, immediately after the `patch_status` closure (after line 144, before the `view! {` at line 146), add:

```rust
    // Generic runner for the dedicated side-effecting verbs (Result<(), String>).
    // Mirrors `patch_status`'s busy-lock + success/error handling.
    let run_verb = move |verb: Verb| {
        if busy.get_untracked() {
            return;
        }
        let Some(task) = open_for.get_untracked() else {
            return;
        };
        let id = task.id;
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            let res = match verb {
                Verb::Pause => TeamsApi::task_pause(&dash, &id).await,
                Verb::Resume => TeamsApi::task_resume(&dash, &id).await,
                Verb::Skip => TeamsApi::task_skip(&dash, &id).await,
                Verb::Retry => TeamsApi::task_retry(&dash, &id).await,
                Verb::Approve => TeamsApi::task_approve(&dash, &id).await,
            };
            match res {
                Ok(()) => {
                    busy.set(false);
                    on_changed.run(());
                    open_for.set(None);
                }
                Err(e) => {
                    busy.set(false);
                    error.set(Some(e));
                }
            }
        });
    };

    // Reject needs an optional reason captured from the reveal-on-click input.
    let submit_reject = move |_ev: web_sys::MouseEvent| {
        if busy.get_untracked() {
            return;
        }
        let Some(task) = open_for.get_untracked() else {
            return;
        };
        let id = task.id;
        let reason = reject_reason.get_untracked().trim().to_string();
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            let reason_opt = if reason.is_empty() { None } else { Some(reason.as_str()) };
            match TeamsApi::task_reject(&dash, &id, reason_opt).await {
                Ok(()) => {
                    busy.set(false);
                    on_changed.run(());
                    open_for.set(None);
                }
                Err(e) => {
                    busy.set(false);
                    error.set(Some(e));
                }
            }
        });
    };
```

> `run_verb`, `patch_status`, and `submit_reject` capture only `Copy` values (`RwSignal`s, the `Callback` `on_changed`, and `DashboardState` `dash` — proven `Copy` by the existing `let dash_runs = dash;` copies at lines 50-51), so each is itself `Copy` and can be moved into multiple per-button event closures below.

- [ ] **Step 7: Replace the fixed-button footer with the status-gated action set**

Delete the now-unused gating locals (lines 177-188: the `// The drawer only offers...` comment through `cancel_disabled`) and the per-action labels that are no longer pre-bound individually — instead replace the whole `<footer>...</footer>` block (lines 250-271) with the dynamic version below.

First, delete lines 171-174 (the four pre-bound `start_label`/`complete_label`/`fail_label`/`cancel_label` locals) and lines 177-188 (the `start_locked`/`terminal_locked`/`*_disabled` locals) — they are replaced by per-action rendering.

Then replace the `<footer ...> ... </footer>` block (lines 250-271) with:

```rust
                            <footer class="px-4 py-3 border-t border-border flex flex-col gap-2">
                                {move || reject_open.get().then(|| view! {
                                    <div class="flex gap-1.5 items-start">
                                        <textarea
                                            class="flex-1 text-xs p-1.5 rounded border border-border bg-surface-sunken resize-y min-h-[2.5rem]"
                                            placeholder=move || t_string!(i18n, teams.kanban.actions.reject_reason_placeholder).to_string()
                                            prop:value=move || reject_reason.get()
                                            on:input=move |ev| reject_reason.set(event_target_value(&ev))
                                        />
                                        <button
                                            class="px-2 py-1 text-xs rounded bg-danger/10 text-danger hover:bg-danger/20 cursor-pointer"
                                            disabled=move || busy.get()
                                            on:click=move |ev| submit_reject(ev)
                                        >
                                            {t_string!(i18n, teams.kanban.actions.reject).to_string()}
                                        </button>
                                    </div>
                                })}
                                <div class="flex gap-2 flex-wrap">
                                    {actions_for_status(&status).into_iter().map(|action| {
                                        let label = match action {
                                            TaskAction::Start => t_string!(i18n, teams.kanban.actions.start),
                                            TaskAction::Complete => t_string!(i18n, teams.kanban.actions.complete),
                                            TaskAction::Fail => t_string!(i18n, teams.kanban.actions.fail),
                                            TaskAction::Cancel => t_string!(i18n, teams.kanban.actions.cancel),
                                            TaskAction::Pause => t_string!(i18n, teams.kanban.actions.pause),
                                            TaskAction::Resume => t_string!(i18n, teams.kanban.actions.resume),
                                            TaskAction::Skip => t_string!(i18n, teams.kanban.actions.skip),
                                            TaskAction::Retry => t_string!(i18n, teams.kanban.actions.retry),
                                            TaskAction::Approve => t_string!(i18n, teams.kanban.actions.approve),
                                            TaskAction::Reject => t_string!(i18n, teams.kanban.actions.reject),
                                        }.to_string();
                                        view! {
                                            <ActionButton
                                                label=label
                                                disabled=Signal::derive(move || busy.get())
                                                on_click=move |_| match action {
                                                    TaskAction::Start => patch_status("in_progress"),
                                                    TaskAction::Complete => patch_status("completed"),
                                                    TaskAction::Fail => patch_status("failed"),
                                                    TaskAction::Cancel => patch_status("cancelled"),
                                                    TaskAction::Pause => run_verb(Verb::Pause),
                                                    TaskAction::Resume => run_verb(Verb::Resume),
                                                    TaskAction::Skip => run_verb(Verb::Skip),
                                                    TaskAction::Retry => run_verb(Verb::Retry),
                                                    TaskAction::Approve => run_verb(Verb::Approve),
                                                    TaskAction::Reject => reject_open.set(true),
                                                }
                                            />
                                        }
                                    }).collect_view()}
                                </div>
                            </footer>
```

> `status` is the plain `String` bound at the top of the `Some(task)` match arm (line 150). The action set is computed once per drawer render; the reject-reason input is independently reactive via `reject_open`. Clicking a terminal-state card shows zero action buttons (correct — view-only). On backend rejection of a raced transition, `error` is set and the drawer stays open (existing error banner at lines 245-249 renders it).

- [ ] **Step 8: Verify the whole crate compiles and the unit test still passes**

Run: `cargo test -p aleph-panel actions_for_status`
Expected: PASS (compiles the full crate incl. board + drawer + wrappers; runs the gating test green). If the compiler complains that a closure is not `Copy`/`FnMut`, confirm `patch_status`/`run_verb`/`submit_reject` capture only `Copy` values (they do) — do not add `.clone()` inside the `.map`; the captures are `Copy` and copy per iteration.

- [ ] **Step 9: Confirm the previously-dead wrappers now have callers (entropy check)**

Run: `git grep -n "TeamsApi::task_pause\|TeamsApi::task_resume\|TeamsApi::task_retry\|TeamsApi::task_skip\|TeamsApi::task_approve\|TeamsApi::task_reject" interfaces/webchat/src`
Expected: each appears in `task_drawer.rs` (the `run_verb`/`submit_reject` match arms) — i.e. zero remaining dead lifecycle wrappers.

- [ ] **Step 10: Commit**

```bash
git add interfaces/webchat/src/platform/wide/views/teams/components/task_drawer.rs
git commit -m "teams: wire full task lifecycle actions into kanban drawer (status-gated, routed through backend verbs)"
```

---

## Manual / E2E Verification (run when an aleph-server is up; documented, not blocking compile gates)

Leptos CSR views are not unit-tested in this crate (only pure logic like `actions_for_status` is). Validate the rendered behavior manually:

1. Build + run the panel against a server (`just dev`, or rebuild the binary per the CLAUDE.md embed chain so the panel is re-embedded).
2. Create a team and a few tasks (ask the assistant, or via the New Task form).
3. Drive one task into each new state and confirm the column + gated actions:
   - **waiting_review**: a task with `lead_review_required` that finishes a run → lands in **Waiting Review** column; drawer shows Approve · Reject · Skip · Cancel. Click **Reject** → reason input appears → submit → task moves to Failed, dependents stay blocked. Re-run another, click **Approve** → task Completed, dependents unblock.
   - **paused**: from an active task click **Pause** → lands in **Paused** column; drawer shows Resume · Cancel; **Resume** → back to Pending.
   - **skipped**: click **Skip** on a pending/blocked task → lands in **Skipped** column; its dependents unblock (skip satisfies deps).
4. Confirm no task ever disappears from the board, and terminal tasks (completed/skipped/cancelled) show no action buttons.

---

## Self-Review (completed during planning)

- **Spec coverage:** §1 problem → Tasks 1-4 collectively; §2 goal 1 (no vanishing) → Task 3; goal 2 (6 verbs reachable, routed through verbs) → Tasks 2+4; goal 3 (dead wrappers consumed + approve/reject added) → Tasks 2+4 (Step 9 verifies); goal 4 (entropy: doc fix + no new dead code) → Task 3 Step 1 + Task 4 Step 9. §4.4 task_card no-change → reflected (not a task). Non-goals (scheduled/archived/decompose/bridge/Office) → excluded. ✔
- **Placeholder scan:** none — every code step shows complete content; commands have expected output. ✔
- **Type consistency:** `actions_for_status(&str) -> Vec<TaskAction>` used identically in test (Step 1) and impl (Step 3) and view (Step 7); `TaskAction`/`Verb` variants match across all three; `task_approve(state, task_id)` / `task_reject(state, task_id, reason: Option<&str>)` signatures defined in Task 2 and called identically in Task 4. ✔
