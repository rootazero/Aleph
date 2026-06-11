# Loop Engineering Round 2 Hardening — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.
>
> **Resource-governance constraint (this round):** do NOT run `cargo check` /
> `cargo test` / `cargo clippy`. Write the tests but do not execute them. The
> controller static-reviews each diff before commit. "Run test" steps below are
> therefore marked **DEFERRED** — write the test, then commit.

**Goal:** Harden the autonomous goal loop with per-goal gate commands,
accumulated lessons reback, and unattended fail-closed approvals.

**Architecture:** Pure loop-layer wiring. `Goal` gains two `#[serde(default)]`
fields; `goal_pursuit` injects lessons; the continuation hook assembles an
effective gate (global ⧺ per-goal); the per-run `ScopedToolService` fails closed
when the run is unattended. `src/harness/` is untouched.

**Tech Stack:** Rust, Tokio, rusqlite (JSON-blob goal store), existing
`ShellStopHook` / `ApprovalRequester` / `RunRequest.metadata`.

---

### Task 1: Goal data — `gate_command` + `lessons` fields and mutators

**Files:**
- Modify: `src/goal/types.rs`

- [ ] **Step 1: Add the two fields to the `Goal` struct**

In `src/goal/types.rs`, after the `gate_outcome` field (currently the last field
of `Goal`, ~line 61), add:

```rust
    /// Optional per-goal objective gate: a shell command evaluated like a
    /// `config.toml [[stop_hooks]]` entry (exit 0 = passed, exit 2 = vetoed,
    /// stdout = reason). Supplements the global gate (logical AND) — see
    /// the continuation hook. `#[serde(default)]` → old payloads read `None`.
    #[serde(default)]
    pub gate_command: Option<String>,
    /// Accumulated lessons (the article's "state file"): gate-failure reasons
    /// and model-authored insights, fed back into the continuation prompt so
    /// the loop does not repeat mistakes. Ring-capped at `MAX_LESSONS` (newest
    /// kept). `#[serde(default)]` → old payloads read empty.
    #[serde(default)]
    pub lessons: Vec<String>,
```

- [ ] **Step 2: Add the `MAX_LESSONS` constant**

Above the `impl Goal` block (after the `Goal` struct, ~line 63):

```rust
/// Ring cap on accumulated lessons kept per goal (newest retained). Bounds the
/// state file so an unbounded loop cannot grow the goal row without limit.
pub const MAX_LESSONS: usize = 5;
```

- [ ] **Step 3: Initialize both fields in `Goal::new`**

In `Goal::new`, in the returned `Self { ... }` literal, after
`gate_outcome: GateOutcome::Unchecked,` add:

```rust
            gate_command: None,
            lessons: Vec::new(),
```

- [ ] **Step 4: Add the two mutators**

After `with_gate_outcome` (~line 117), add (NOT `const` — `Option<String>` /
`Vec<String>` assignment drops the old value):

```rust
    /// Configuration, not a lifecycle transition — deliberately does not bump
    /// `updated_at_ms` (mirrors `with_budget`/`with_pursuit`).
    #[must_use]
    pub fn with_gate_command(mut self, gate_command: Option<String>) -> Self {
        self.gate_command = gate_command;
        self
    }

    /// Append a lesson to the state file, keeping at most `MAX_LESSONS` (newest).
    /// Appending a lesson is progress, so it bumps `updated_at_ms` (like
    /// `with_note`). Returns a new `Goal` (§不可变性).
    #[must_use]
    pub fn with_lesson_appended(mut self, lesson: String, now_ms: u64) -> Self {
        self.lessons.push(lesson);
        if self.lessons.len() > MAX_LESSONS {
            let drop = self.lessons.len() - MAX_LESSONS;
            self.lessons.drain(0..drop);
        }
        self.updated_at_ms = now_ms;
        self
    }
```

- [ ] **Step 5: Add tests** (write, do not run — DEFERRED)

In the `#[cfg(test)] mod tests` of `types.rs`, add:

```rust
    #[test]
    fn new_goal_has_no_gate_command_and_no_lessons() {
        let g = sample();
        assert_eq!(g.gate_command, None);
        assert!(g.lessons.is_empty());
    }

    #[test]
    fn with_gate_command_sets_without_bumping_updated_at() {
        let g = sample();
        let after = g.clone().with_gate_command(Some("cargo test".into()));
        assert_eq!(after.gate_command.as_deref(), Some("cargo test"));
        assert_eq!(after.updated_at_ms, g.updated_at_ms, "config, no bump");
        assert_eq!(g.gate_command, None, "original unchanged");
    }

    #[test]
    fn with_lesson_appended_keeps_last_five_and_bumps_updated_at() {
        let mut g = sample();
        for i in 0..7 {
            g = g.with_lesson_appended(format!("lesson {i}"), 1_000 + i as u64);
        }
        assert_eq!(g.lessons.len(), MAX_LESSONS);
        assert_eq!(g.lessons.first().unwrap(), "lesson 2", "oldest dropped");
        assert_eq!(g.lessons.last().unwrap(), "lesson 6", "newest kept");
        assert_eq!(g.updated_at_ms, 1_006);
    }

    #[test]
    fn old_payload_without_new_fields_deserializes_defaults() {
        let json = r#"{"id":"goal-1","session_id":"s","objective":"o",
            "status":"active","token_budget":null,"tokens_at_start":0,
            "pursuit":{"mode":"passive"},"created_at_ms":1,"updated_at_ms":1,
            "note":null,"continuations_used":0,"gate_outcome":"unchecked"}"#;
        let g: Goal = serde_json::from_str(json).expect("deserialize old payload");
        assert_eq!(g.gate_command, None);
        assert!(g.lessons.is_empty());
    }
```

- [ ] **Step 6: Commit**

```bash
git add src/goal/types.rs
git commit -m "goal: add per-goal gate_command and lessons state-file fields"
```

---

### Task 2: `goal_pursuit` — lessons reback + reopen appends lesson

**Files:**
- Modify: `src/tasks/goal_pursuit.rs`

- [ ] **Step 1: Add a private lessons renderer**

After the imports (~line 14), add:

```rust
/// Render accumulated lessons (the state file) for injection into a
/// continuation prompt. Empty → empty string (regression-safe: no prompt change
/// when there are no lessons). Newest last, matching their append order.
fn render_lessons(goal: &Goal) -> String {
    if goal.lessons.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n\nLessons from earlier iterations (avoid repeating these):\n");
    for lesson in &goal.lessons {
        let trimmed: String = lesson.chars().take(300).collect();
        s.push_str(&format!("- {trimmed}\n"));
    }
    s
}
```

- [ ] **Step 2: Inject lessons into `continuation_prompt`**

In `continuation_prompt`, change the two `format!` blocks to append lessons.
Replace the `is_final` branch's `format!(...)` so the objective line is followed
by lessons. Concretely, bind the lessons once at the top of the function (after
the `let is_final = ...;` line) and append to each returned string:

```rust
    let lessons = render_lessons(goal);
    if is_final {
        format!(
            "[Final autonomous iteration {this_iter}/{max_iter} toward your \
             standing goal]\nGoal: {}{lessons}\n\nThis is your LAST autonomous step — no \
             further continuations will run after it. Wrap up now: if the goal is \
             achieved, call goal(action='update', status='complete'); if work \
             remains, call goal(action='update', status='blocked') with a note on \
             what's left so the user can take over. Do not begin anything you \
             cannot finish in this step.",
            goal.objective,
        )
    } else {
        let remaining = max_iter.saturating_sub(this_iter);
        format!(
            "[Continuing toward your standing goal — autonomous iteration \
             {this_iter}/{max_iter}]\nGoal: {}{lessons}\n\nTake the next concrete step; \
             pace yourself against the {remaining} continuation(s) remaining after \
             this one. If you have achieved the goal, call goal(action='update', \
             status='complete') and stop. If you are blocked and need the user, \
             call goal(action='update', status='blocked') and stop.",
            goal.objective,
        )
    }
```

(Only the `{lessons}` insertion after `Goal: {}` is new in each branch.)

- [ ] **Step 3: `reopen_after_gate_failure` appends the gate reason as a lesson**

In `reopen_after_gate_failure`, in BOTH branches, chain
`.with_lesson_appended(...)` so the gate reason is preserved across the ring cap.
Replace the function body's two construction expressions:

```rust
    let trimmed_lesson: String = format!("Objective gate vetoed: {}", reason)
        .chars()
        .take(300)
        .collect();
    if cap_spent {
        let note = cap_reached_note(goal);
        goal.clone()
            .with_status(GoalStatus::Blocked, now_ms)
            .with_note(Some(note), now_ms)
            .with_gate_outcome(GateOutcome::Unchecked, now_ms)
            .with_lesson_appended(trimmed_lesson, now_ms)
    } else {
        let trimmed: String = reason.chars().take(300).collect();
        let note = format!("Objective gate vetoed completion: {trimmed}");
        goal.clone()
            .with_status(GoalStatus::Active, now_ms)
            .with_note(Some(note), now_ms)
            .with_gate_outcome(GateOutcome::Unchecked, now_ms)
            .with_lesson_appended(trimmed_lesson, now_ms)
    }
```

- [ ] **Step 4: Inject lessons into `gate_failure_prompt`**

In `gate_failure_prompt`, bind lessons and insert after the `Goal:` line:

```rust
pub fn gate_failure_prompt(goal: &Goal, reason: &str) -> String {
    let trimmed: String = reason.chars().take(600).collect();
    let lessons = render_lessons(goal);
    format!(
        "[Your standing goal is NOT done — the objective gate rejected your \
         completion claim]\nGoal: {}{lessons}\n\nThe automated gate (tests / build / \
         lint) failed with:\n{trimmed}\n\nThis is an objective signal, not an \
         opinion. Fix what the gate flagged, then call goal(action='update', \
         status='complete') again only when the work truly passes. If you \
         cannot resolve it, call goal(action='update', status='blocked') with \
         a note describing what remains.",
        goal.objective,
    )
}
```

Note: `gate_failure_prompt` is called in `execute.rs` BEFORE `reopen_after_gate_failure`
persists the new lesson, so the failing reason for THIS iteration arrives via the
explicit `{reason}` block; `{lessons}` carries the PRIOR iterations' lessons.
This is intended (no duplication of the current reason).

- [ ] **Step 5: Add tests** (write, do not run — DEFERRED)

Add to the `tests` module:

```rust
    #[test]
    fn reopen_after_gate_failure_appends_lesson() {
        let g = active_goal(5).with_status(GoalStatus::Complete, 1);
        let r = reopen_after_gate_failure(&g, "tests failed: 3 errors", 9);
        assert_eq!(r.lessons.len(), 1);
        assert!(r.lessons[0].contains("tests failed: 3 errors"));
        assert!(r.lessons[0].contains("Objective gate vetoed"));
    }

    #[test]
    fn continuation_prompt_includes_prior_lessons() {
        let g = active_goal(5)
            .with_lesson_appended("forgot to run migrations".into(), 2);
        let p = continuation_prompt(&g);
        assert!(p.contains("Lessons from earlier iterations"), "got: {p}");
        assert!(p.contains("forgot to run migrations"));
    }

    #[test]
    fn continuation_prompt_unchanged_when_no_lessons() {
        let g = active_goal(5);
        assert!(!continuation_prompt(&g).contains("Lessons from earlier"));
    }

    #[test]
    fn gate_failure_prompt_includes_prior_lessons() {
        let g = active_goal(5).with_lesson_appended("missing index".into(), 2);
        let p = gate_failure_prompt(&g, "still red");
        assert!(p.contains("missing index"));
        assert!(p.contains("still red"));
    }
```

- [ ] **Step 6: Commit**

```bash
git add src/tasks/goal_pursuit.rs
git commit -m "goal_pursuit: reback accumulated lessons into continuation prompts"
```

---

### Task 3: `goal` tool — `gate_command` on set, `lesson` on update, render

**Files:**
- Modify: `src/builtin_tools/goal.rs`

- [ ] **Step 1: Extend `GoalArgs`**

Add two fields after `pursuit_max_iterations` (~line 48):

```rust
    /// Optional per-goal objective gate shell command — for `set`. Evaluated
    /// like a stop hook (exit 0 = pass, exit 2 = veto). Supplements the global
    /// gate. Use a real pass/fail command (tests/build/lint), not prose.
    pub gate_command: Option<String>,
    /// Optional lesson to append to the goal's state file — for `update`.
    /// Record what you learned so future autonomous iterations don't repeat it.
    pub lesson: Option<String>,
```

- [ ] **Step 2: Wire `gate_command` into `Set`**

In `GoalAction::Set`, after the `.with_note(args.note.clone(), now)` and the
`pursuit` block, before `self.store.put(&goal)?;`, add:

```rust
                goal = goal.with_gate_command(args.gate_command.clone());
```

(Change the `let mut goal = ...` chain to keep `goal` mutable — it already is
`let mut goal`.)

- [ ] **Step 3: Wire `lesson` into `Update`**

In `GoalAction::Update`, after the `if args.note.is_some() { ... }` block and
before `self.store.put(&goal)?;`, add:

```rust
                if let Some(lesson) = args.lesson.clone() {
                    goal = goal.with_lesson_appended(lesson, now);
                }
```

- [ ] **Step 4: Surface in `render`**

In `GoalTool::render`, before the final `s` return, after the `note` block, add:

```rust
        if goal.gate_command.is_some() {
            s.push_str("\ngate: per-goal command set");
        }
        if !goal.lessons.is_empty() {
            s.push_str(&format!(
                "\nlessons ({}): {}",
                goal.lessons.len(),
                goal.lessons.last().map(String::as_str).unwrap_or_default()
            ));
        }
```

- [ ] **Step 5: Update tool description + examples (R8 discoverability)**

In `DESCRIPTION`, after the sentence about `pursuit_max_iterations`, append:

```text
 Optionally attach a gate_command (a shell test like 'cargo test' that must \
exit 0 before an autonomous goal is accepted as complete). On action='update' \
you may also pass a lesson to record what you learned for future iterations.
```

In `examples()`, add one entry:

```rust
            "goal(action='update', lesson='remember to run db migrations before tests')".into(),
```

- [ ] **Step 6: Fix existing test call sites**

Every `GoalArgs { ... }` literal in the `tests` module now needs the two new
fields. Add `gate_command: None, lesson: None,` to EACH of the existing
`GoalArgs { ... }` constructions (there are 9 across the test fns:
`set_then_get_returns_objective` (2), `update_complete_marks_status` (2),
`pursuit_iterations_are_capped` (2), `get_with_no_goal_is_graceful` (1),
`set_requires_objective` (1), `clear_removes_goal` (3) — add to every one).

- [ ] **Step 7: Add new tests** (write, do not run — DEFERRED)

```rust
    #[tokio::test]
    async fn set_with_gate_command_is_rendered() {
        let (tool, _d) = tool_with_session("sess-gate");
        tool.call(GoalArgs {
            action: GoalAction::Set,
            objective: Some("Ship X".into()),
            status: None,
            note: None,
            token_budget: None,
            pursuit_max_iterations: Some(3),
            gate_command: Some("cargo test".into()),
            lesson: None,
        })
        .await
        .unwrap();
        let out = tool
            .call(GoalArgs {
                action: GoalAction::Get,
                objective: None, status: None, note: None,
                token_budget: None, pursuit_max_iterations: None,
                gate_command: None, lesson: None,
            })
            .await
            .unwrap();
        assert!(out.message.contains("per-goal command set"));
    }

    #[tokio::test]
    async fn update_with_lesson_appends_and_renders() {
        let (tool, _d) = tool_with_session("sess-lesson");
        tool.call(GoalArgs {
            action: GoalAction::Set,
            objective: Some("Y".into()),
            status: None, note: None, token_budget: None,
            pursuit_max_iterations: None, gate_command: None, lesson: None,
        })
        .await
        .unwrap();
        let out = tool
            .call(GoalArgs {
                action: GoalAction::Update,
                objective: None, status: None, note: None,
                token_budget: None, pursuit_max_iterations: None,
                gate_command: None, lesson: Some("don't skip lint".into()),
            })
            .await
            .unwrap();
        assert!(out.message.contains("lessons (1)"));
        assert!(out.message.contains("don't skip lint"));
    }
```

- [ ] **Step 8: Commit**

```bash
git add src/builtin_tools/goal.rs
git commit -m "goal tool: expose per-goal gate_command and lesson recording (R8)"
```

---

### Task 4: Continuation hook — effective gate (global ⧺ per-goal)

**Files:**
- Modify: `src/verification/stop_hooks.rs` (add `effective_gate` helper)
- Modify: `src/gateway/execution_engine/execute.rs` (use it)

- [ ] **Step 1: Add `effective_gate` to `stop_hooks.rs`**

After `build_from_config` (~line 96), add:

```rust
/// Assemble the effective objective gate for a goal: the global config hooks
/// (if any) PLUS a per-goal ad-hoc [`ShellStopHook`] built from
/// `goal_gate_command` (if any). Returns `None` only when neither source is
/// present (caller then treats a `complete` claim as terminal — Round 1
/// behavior). AND semantics: the combined vector runs through
/// `execute_stop_hooks_arc`, which vetoes on the first block, so either source
/// can veto completion.
#[must_use]
pub fn effective_gate(
    global: Option<&Arc<Vec<Arc<dyn StopHookHandler>>>>,
    goal_gate_command: Option<&str>,
) -> Option<Arc<Vec<Arc<dyn StopHookHandler>>>> {
    match (global, goal_gate_command) {
        (None, None) => None,
        (Some(g), None) => Some(g.clone()),
        (g, Some(cmd)) => {
            let mut hooks: Vec<Arc<dyn StopHookHandler>> =
                g.map(|v| v.as_ref().clone()).unwrap_or_default();
            hooks.push(Arc::new(ShellStopHook::new("goal_gate", cmd)) as Arc<dyn StopHookHandler>);
            Some(Arc::new(hooks))
        }
    }
}
```

- [ ] **Step 2: Add a unit test for `effective_gate`** (write, do not run — DEFERRED)

In the `tests` module of `stop_hooks.rs`:

```rust
    #[test]
    fn effective_gate_combines_sources() {
        // Neither → None (Round 1 terminal behavior).
        assert!(effective_gate(None, None).is_none());
        // Global only → the global vector (length preserved).
        let global: Arc<Vec<Arc<dyn StopHookHandler>>> =
            Arc::new(vec![Arc::new(ShellStopHook::new("g", "true")) as Arc<dyn StopHookHandler>]);
        assert_eq!(effective_gate(Some(&global), None).unwrap().len(), 1);
        // Per-goal only → one hook.
        assert_eq!(effective_gate(None, Some("cargo test")).unwrap().len(), 1);
        // Both → global ⧺ per-goal.
        assert_eq!(
            effective_gate(Some(&global), Some("cargo test")).unwrap().len(),
            2
        );
    }
```

- [ ] **Step 3: Use `effective_gate` in `execute.rs`**

In `src/gateway/execution_engine/execute.rs`, in the continuation-hook
`awaiting_gate` arm (~line 638), change the gate-configured computation and the
gate construction. Replace:

```rust
                                if crate::tasks::goal_pursuit::awaiting_gate(
                                    &goal,
                                    cont_deps.gate.is_some(),
                                ) {
                                    let gate = cont_deps.gate.clone().expect("is_some checked");
```

with:

```rust
                                let gate_configured = cont_deps.gate.is_some()
                                    || goal.gate_command.is_some();
                                if crate::tasks::goal_pursuit::awaiting_gate(
                                    &goal,
                                    gate_configured,
                                ) {
                                    let gate = crate::verification::stop_hooks::effective_gate(
                                        cont_deps.gate.as_ref(),
                                        goal.gate_command.as_deref(),
                                    )
                                    .expect("gate_configured implies effective_gate is Some");
```

(The rest of the arm — `StopHookContext`, `execute_stop_hooks_arc(&gate, ...)`,
the `vetoed` match — is unchanged.)

- [ ] **Step 4: Commit**

```bash
git add src/verification/stop_hooks.rs src/gateway/execution_engine/execute.rs
git commit -m "loop: run per-goal gate command AND-combined with global gate"
```

---

### Task 5: Unattended security-tax — fail-closed approvals + audit

**Files:**
- Modify: `src/gateway/execution_engine/execute.rs` (stamp `unattended` metadata)
- Modify: `src/tools/scoped/mod.rs` (field)
- Modify: `src/tools/scoped/builder.rs` (init + `with_unattended`)
- Modify: `src/tools/scoped/dispatch.rs` (fail-closed in `confirm_with_memory`)
- Modify: `src/gateway/execution_engine/tool_service_builder.rs` (param)
- Modify: `src/gateway/execution_engine/run_loop.rs` (compute + pass)

- [ ] **Step 1: Stamp `unattended` on continuation runs**

In `spawn_continuation_run` (`execute.rs`, ~line 848), change the
`metadata: std::collections::HashMap::new(),` line in the `RunRequest` literal to:

```rust
        metadata: {
            // Unattended security-tax: this autonomous run has no human on the
            // channel to approve anything. The per-run ScopedToolService reads
            // this marker and fails closed on confirm-gated tools.
            let mut m = std::collections::HashMap::new();
            m.insert("unattended".to_string(), "true".to_string());
            m
        },
```

- [ ] **Step 2: Add the `unattended` field to `ScopedToolService`**

In `src/tools/scoped/mod.rs`, add to the struct (after `tool_permissions`'s
field — locate the last `pub(super)` field; add a new one):

```rust
    /// True when this service serves an UNATTENDED run (an autonomous goal
    /// continuation — no human on the channel). Confirm-gated tools fail closed
    /// (auto-denied) instead of awaiting an approval that can never arrive.
    /// Defaults `false`; interactive turns are unaffected.
    pub(super) unattended: bool,
```

- [ ] **Step 3: Initialize the field + add the builder in `builder.rs`**

In `ScopedToolService::new`'s `Self { ... }` literal (after `tool_permissions: None,`):

```rust
            unattended: false,
```

After the `with_tool_permissions` method (or any `with_*`), add:

```rust
    /// Mark this service as serving an unattended (autonomous continuation)
    /// run. Confirm-gated tools then fail closed. See [`Self::unattended`].
    #[must_use]
    pub fn with_unattended(mut self, unattended: bool) -> Self {
        self.unattended = unattended;
        self
    }
```

- [ ] **Step 4: Fail-closed short-circuit in `confirm_with_memory`**

In `src/tools/scoped/dispatch.rs`, at the very TOP of the `confirm_with_memory`
function body (immediately after the opening `{` at ~line 365, before the
`let fingerprint = ...` denial-ledger logic), insert:

```rust
        // Unattended security-tax: an autonomous continuation run has no human
        // on the channel to approve anything. Fail closed — auto-deny any
        // confirm-gated tool (`requires_confirmation` ∪ `Ask`-tier permission ∪
        // operator-override `confirm_tools`, all of which funnel here) with an
        // audit line, rather than awaiting an approval that can never arrive.
        // Interactive turns leave `unattended = false` and are unaffected.
        if self.unattended {
            tracing::warn!(
                tool = %name,
                "unattended run: auto-denied confirm-gated tool (no human to approve)"
            );
            return Err(ConfirmDenial {
                outcome: ApprovalOutcome::Denied,
                hint: Some(
                    "This run is unattended (autonomous continuation) — \
                     interactive approval is unavailable, so confirm-gated tools \
                     are auto-denied. Use a non-interactive approach, or call \
                     goal(action='update', status='blocked') to hand back to the \
                     user."
                        .to_string(),
                ),
            });
        }
```

(`ApprovalOutcome` and `ConfirmDenial` are already in scope in this file.)

- [ ] **Step 5: Add the `unattended` parameter to `build_request_tool_service`**

In `src/gateway/execution_engine/tool_service_builder.rs`, add `unattended: bool`
as the LAST parameter of `build_request_tool_service` (after `tool_permissions`).
Update the doc comment with a one-line bullet:

```rust
/// * `unattended` — true for an autonomous continuation run; makes the service
///   fail closed on confirm-gated tools (no human to approve).
```

In the body, after the `tool_permissions` block (before the result_store seam),
thread it:

```rust
    svc = svc.with_unattended(unattended);
```

Fix the two in-file test call sites (~lines 169, 181) by appending `, false` as
the final argument:

```rust
        build_request_tool_service(registry, BTreeSet::new(), None, None, None, None, "", None, false);
```
```rust
        let svc = build_request_tool_service(registry, allowed, None, None, None, None, "", None, false);
```

- [ ] **Step 6: Compute + pass `unattended` in `run_loop.rs`**

In `src/gateway/execution_engine/run_loop.rs`, near the top of the function that
holds the two `build_request_tool_service` call sites (it has `request:
&RunRequest` in scope), compute once — place this binding before the first call
site (~line 807), e.g. right after `allowed_names` is available:

```rust
            let unattended =
                request.metadata.get("unattended").map(String::as_str) == Some("true");
```

Then add `unattended,` as the final argument to BOTH
`build_request_tool_service(...)` calls (the parent-view call ~line 807 and the
main call ~line 955):

```rust
                    tool_permissions.clone(),
                    unattended,
                );
```

(If the two call sites are in different scopes such that one binding isn't
visible at both, bind `unattended` separately at each site with the same
expression — it is a cheap pure read.)

- [ ] **Step 7: Commit**

```bash
git add src/gateway/execution_engine/execute.rs src/tools/scoped/mod.rs \
        src/tools/scoped/builder.rs src/tools/scoped/dispatch.rs \
        src/gateway/execution_engine/tool_service_builder.rs \
        src/gateway/execution_engine/run_loop.rs
git commit -m "loop: fail closed on confirm-gated tools for unattended autonomous runs"
```

---

## Final review

After all tasks, the controller dispatches a final code reviewer over the whole
diff, then uses `superpowers:finishing-a-development-branch`.

**Self-review checklist (controller, before final review):**
- `Goal` new fields are `#[serde(default)]` → JSON-blob store round-trips.
- `effective_gate` AND semantics: any veto vetoes (relies on `execute_stop_hooks`
  returning the first halt/block — verify unchanged).
- `unattended` defaults `false` everywhere → interactive turns unaffected (the
  only behavior change is for runs stamped by `spawn_continuation_run`).
- `with_gate_command` / `with_lesson_appended` are NOT `const` (they drop owned
  values).
- All `GoalArgs { ... }` literals updated for the two new fields (compile-blocking
  if missed — flagged in review since no `cargo check`).
