# Loop Engineering Round 3 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Implement unattended-run secret redaction, per-goal wall-clock timeout, and goal-lessons→memory promotion.

**Architecture:** Wiring-first extensions in the loop layer. `src/harness/` untouched (R10).

**Tech Stack:** Rust, Tokio, rusqlite, `SecretMasker`, `TraceSink` decorator, `DreamStage` pipeline.

**Resource governance:** Write tests but DO NOT run `cargo check`/`test` locally. Commit after each task.
Code comments in English. Mechanically verify every changed struct-literal / call-site (Round 2 lesson:
self-reports are unreliable without a compiler).

---

## Feature 2 — Per-goal wall-clock timeout

### Task 1: Add `deadline_ms` to the `Goal` struct

**Files:**
- Modify: `src/goal/types.rs`

- [ ] **Step 1: Add the field** after the `lessons` field (after line 73, before the closing `}` at line 74):

```rust
    /// Optional wall-clock deadline (Unix epoch ms). When set and exceeded, the
    /// autonomous loop stops re-pursuing and blocks the goal for the user — a
    /// structural stop condition alongside the iteration/token caps (R7: no
    /// judgment, pure time comparison). `#[serde(default)]` → old payloads read
    /// `None`.
    #[serde(default)]
    pub deadline_ms: Option<u64>,
```

- [ ] **Step 2: Initialize in `Goal::new`** — add `deadline_ms: None,` after `lessons: Vec::new(),` (line 101):

```rust
            lessons: Vec::new(),
            deadline_ms: None,
```

- [ ] **Step 3: Add the mutator** after `with_pursuit` (after line 173):

```rust
    /// Configuration, not a lifecycle transition — deliberately does not bump
    /// `updated_at_ms` (mirrors `with_budget`/`with_pursuit`). `None` clears.
    #[must_use]
    pub const fn with_deadline_ms(mut self, deadline_ms: Option<u64>) -> Self {
        self.deadline_ms = deadline_ms;
        self
    }
```

- [ ] **Step 4: Add tests** in the `tests` module (before the final closing `}`):

```rust
    #[test]
    fn new_goal_has_no_deadline() {
        assert_eq!(sample().deadline_ms, None);
    }

    #[test]
    fn with_deadline_ms_sets_without_bumping_updated_at() {
        let g = sample();
        let after = g.clone().with_deadline_ms(Some(99_999));
        assert_eq!(after.deadline_ms, Some(99_999));
        assert_eq!(after.updated_at_ms, g.updated_at_ms, "config, no bump");
        assert_eq!(g.deadline_ms, None, "original unchanged");
    }

    #[test]
    fn old_payload_without_deadline_deserializes_none() {
        let json = r#"{"id":"goal-1","session_id":"s","objective":"o",
            "status":"active","token_budget":null,"tokens_at_start":0,
            "pursuit":{"mode":"passive"},"created_at_ms":1,"updated_at_ms":1,
            "note":null,"continuations_used":0,"gate_outcome":"unchecked"}"#;
        let g: Goal = serde_json::from_str(json).expect("deserialize old payload");
        assert_eq!(g.deadline_ms, None);
    }
```

- [ ] **Step 5: Commit**

```bash
git add src/goal/types.rs
git commit -m "goal: add optional wall-clock deadline_ms field"
```

---

### Task 2: Fold the deadline into the pursuit stop predicates

**Files:**
- Modify: `src/tasks/goal_pursuit.rs`

- [ ] **Step 1: Change `should_continue` signature + add deadline check.** Replace the function (lines 31–49) with:

```rust
/// Pure decision: should this goal get one more autonomous continuation?
/// `tokens_now` is the session's current total-token count (pass 0 when a
/// live counter isn't available — then only the iteration cap applies).
/// `now_ms` is the current wall-clock (Unix epoch ms); pass 0 when no clock is
/// available — a set deadline is then NOT enforced (the iteration cap remains
/// the backstop), keeping clock-less callers behavior-identical.
#[must_use]
pub fn should_continue(goal: &Goal, tokens_now: u64, now_ms: u64) -> bool {
    let PursuitMode::Active { max_iterations } = goal.pursuit else {
        return false; // Passive goals never self-continue.
    };
    if goal.status != GoalStatus::Active {
        return false; // complete / blocked / paused → stop.
    }
    if goal.continuations_used >= max_iterations {
        return false; // structural backstop (hermes max_turns parity).
    }
    if goal.over_budget(tokens_now) {
        return false; // soft budget becomes a hard stop for autonomous runs.
    }
    if let Some(deadline) = goal.deadline_ms {
        if now_ms != 0 && now_ms > deadline {
            return false; // wall-clock budget exhausted.
        }
    }
    true
}
```

- [ ] **Step 2: Thread `now_ms` through `exhausted_while_active`.** Replace (lines 103–108):

```rust
#[must_use]
pub fn exhausted_while_active(goal: &Goal, tokens_now: u64, now_ms: u64) -> bool {
    matches!(goal.pursuit, PursuitMode::Active { .. })
        && goal.status == GoalStatus::Active
        && !should_continue(goal, tokens_now, now_ms)
}
```

- [ ] **Step 3: Add the deadline-specific block note** after `cap_reached_note` (after line 122). Leave `cap_reached_note` UNCHANGED:

```rust
/// Note stamped when autonomous pursuit is cut off specifically by the
/// wall-clock deadline (distinct from the iteration cap). The continuation hook
/// picks this over `cap_reached_note` when the deadline was the binding stop.
#[must_use]
pub fn deadline_reached_note(_goal: &Goal) -> String {
    "Autonomous pursuit reached its wall-clock budget without completing. \
     Blocked for your guidance — review progress, then clear or re-set the \
     goal to continue."
        .to_string()
}
```

- [ ] **Step 4: Update every in-file call site.** In the `tests` module, EVERY call to
  `should_continue(&g, 0)` becomes `should_continue(&g, 0, 0)` and EVERY call to
  `exhausted_while_active(&g, 0)` becomes `exhausted_while_active(&g, 0, 0)`. The affected lines are
  approximately: 205, 211, 218, 224, 231 (should_continue) and 265, 269, 275, 278 (exhausted_while_active).
  Verify by grep afterward that NO `should_continue(` or `exhausted_while_active(` call has the old arity.

- [ ] **Step 5: Add deadline tests** to the `tests` module:

```rust
    #[test]
    fn stops_when_past_deadline() {
        let g = active_goal(5).with_deadline_ms(Some(1_000));
        assert!(should_continue(&g, 0, 999), "before deadline → continue");
        assert!(!should_continue(&g, 0, 1_001), "past deadline → stop");
    }

    #[test]
    fn deadline_ignored_without_clock() {
        // now_ms == 0 means "no clock" — a set deadline must NOT fire, so
        // clock-less callers keep iteration-cap-only behavior.
        let g = active_goal(5).with_deadline_ms(Some(1_000));
        assert!(should_continue(&g, 0, 0));
    }

    #[test]
    fn exhausted_when_past_deadline_even_with_iterations_left() {
        let g = active_goal(5).with_deadline_ms(Some(1_000));
        // iterations remain (0/5) but the wall-clock budget is spent.
        assert!(exhausted_while_active(&g, 0, 2_000));
    }

    #[test]
    fn deadline_reached_note_mentions_wall_clock() {
        let g = active_goal(5);
        assert!(deadline_reached_note(&g).to_lowercase().contains("wall-clock"));
    }
```

- [ ] **Step 6: Commit**

```bash
git add src/tasks/goal_pursuit.rs
git commit -m "goal_pursuit: enforce per-goal wall-clock deadline in stop predicates"
```

---

### Task 3: Pass `now_ms` at the continuation hook call sites

**Files:**
- Modify: `src/gateway/execution_engine/execute.rs`

- [ ] **Step 1: Update `should_continue` call** (line 716). Change:

```rust
                                } else if crate::tasks::goal_pursuit::should_continue(&goal, 0) {
```

to:

```rust
                                } else if crate::tasks::goal_pursuit::should_continue(
                                    &goal, 0, now_ms,
                                ) {
```

- [ ] **Step 2: Update `exhausted_while_active` call + deadline-aware note** (lines 735–742). Change:

```rust
                                } else if crate::tasks::goal_pursuit::exhausted_while_active(
                                    &goal, 0,
                                ) {
                                    let note = crate::tasks::goal_pursuit::cap_reached_note(&goal);
```

to:

```rust
                                } else if crate::tasks::goal_pursuit::exhausted_while_active(
                                    &goal, 0, now_ms,
                                ) {
                                    // Distinguish wall-clock exhaustion from the
                                    // iteration cap so the user sees the real
                                    // stop reason on their next turn.
                                    let note = if goal
                                        .deadline_ms
                                        .is_some_and(|d| now_ms != 0 && now_ms > d)
                                    {
                                        crate::tasks::goal_pursuit::deadline_reached_note(&goal)
                                    } else {
                                        crate::tasks::goal_pursuit::cap_reached_note(&goal)
                                    };
```

- [ ] **Step 3: Commit**

```bash
git add src/gateway/execution_engine/execute.rs
git commit -m "execution_engine: pass wall-clock now_ms into goal stop predicates"
```

---

### Task 4: Expose `timeout_minutes` on the goal tool (R8)

**Files:**
- Modify: `src/builtin_tools/goal.rs`

- [ ] **Step 1: Add the arg field** to `GoalArgs` after `lesson` (after line 55):

```rust
    /// For `set`: wall-clock budget in minutes. Converted to an absolute
    /// deadline (now + minutes) at set time. None = no time limit.
    pub timeout_minutes: Option<u32>,
```

- [ ] **Step 2: Apply it in the `Set` handler.** After the `with_gate_command` line (line 199), before
  `self.store.put(&goal)?;` (line 200), add:

```rust
                if let Some(minutes) = args.timeout_minutes {
                    let deadline = now.saturating_add(u64::from(minutes).saturating_mul(60_000));
                    goal = goal.with_deadline_ms(Some(deadline));
                }
```

- [ ] **Step 3: Render the deadline.** In `render` (after the `pursuit` block, after line 104, before
  the `note` block), add:

```rust
        if goal.deadline_ms.is_some() {
            s.push_str(", deadline set");
        }
```

- [ ] **Step 4: Update DESCRIPTION** — extend the parenthetical about `set` options. Change the text
  `(optionally with a token_budget, and pursuit_max_iterations to let the system continue autonomously)`
  to `(optionally with a token_budget, pursuit_max_iterations to let the system continue autonomously, and timeout_minutes to cap wall-clock pursuit)`.

- [ ] **Step 5: Add an example** to `examples()` (after the `set ... token_budget` example, line 159):

```rust
            "goal(action='set', objective='Triage failing CI', pursuit_max_iterations=10, timeout_minutes=30)".into(),
```

- [ ] **Step 6: Update EVERY `GoalArgs { ... }` literal** in the file to include `timeout_minutes: None,`.
  There are literals in the test module at approximately lines 267, 281, 300, 313, 332, 345, 367, 387,
  404, 416, 429, 447, 460, 474, 483. After editing, grep-verify that the count of `timeout_minutes:`
  occurrences equals the count of `GoalArgs {` occurrences (plus the field definition). MISSING ONE IS A
  SILENT COMPILE ERROR (no `cargo` to catch it).

- [ ] **Step 7: Add a test** for the new field (in the `tests` module):

```rust
    #[tokio::test]
    async fn set_with_timeout_minutes_sets_deadline() {
        let (tool, _d) = tool_with_session("sess-timeout");
        tool.call(GoalArgs {
            action: GoalAction::Set,
            objective: Some("bounded run".into()),
            status: None, note: None, token_budget: None,
            pursuit_max_iterations: Some(5), gate_command: None, lesson: None,
            timeout_minutes: Some(30),
        })
        .await
        .unwrap();
        let out = tool
            .call(GoalArgs {
                action: GoalAction::Get,
                objective: None, status: None, note: None,
                token_budget: None, pursuit_max_iterations: None,
                gate_command: None, lesson: None, timeout_minutes: None,
            })
            .await
            .unwrap();
        assert!(out.message.contains("deadline set"));
    }
```

- [ ] **Step 8: Commit**

```bash
git add src/builtin_tools/goal.rs
git commit -m "goal tool: expose timeout_minutes to set a per-goal wall-clock deadline"
```

---

## Feature 1 — Unattended-run secret redaction

### Task 5: `UnattendedRedactingSink` trace decorator + run_loop wiring

**Files:**
- Create: `src/gateway/execution_engine/unattended_redacting_sink.rs`
- Modify: `src/gateway/execution_engine/mod.rs` (module decl + re-export)
- Modify: `src/gateway/execution_engine/run_loop.rs`

- [ ] **Step 1: Write the decorator** at `src/gateway/execution_engine/unattended_redacting_sink.rs`:

```rust
//! UnattendedRedactingSink — secret redaction for unattended autonomous runs.
//!
//! Round 2 made unattended runs fail closed on tool confirmation. This closes
//! the observability side: when no human is watching, model-authored trace text
//! (which could echo a secret the loop just read) is run through `SecretMasker`
//! before it reaches persistence, the channel progress push, or the WebSocket
//! stream. Attended runs are never wrapped, so their trace path is unchanged.
//!
//! Lives in `src/gateway/` (a TraceSink consumer), not `src/harness/` (R10).

use std::sync::Arc;

use crate::exec::masker::SecretMasker;
use crate::harness::trace::LoopTraceEvent;
use crate::harness::TraceSink;

/// Wraps an inner `TraceSink`, redacting model-authored text on the two
/// text-bearing `LoopTraceEvent` variants. All other variants forward by
/// reference, unchanged (`#[non_exhaustive]`-safe wildcard).
pub struct UnattendedRedactingSink {
    inner: Arc<dyn TraceSink>,
    masker: SecretMasker,
}

impl UnattendedRedactingSink {
    #[must_use]
    pub fn new(inner: Arc<dyn TraceSink>) -> Self {
        Self {
            inner,
            masker: SecretMasker::new(),
        }
    }
}

impl TraceSink for UnattendedRedactingSink {
    fn on_trace(&self, event: &LoopTraceEvent) {
        match event {
            LoopTraceEvent::TextEmitted {
                iteration,
                stream,
                text,
            } => {
                let redacted = self.masker.mask(text);
                if redacted == *text {
                    self.inner.on_trace(event);
                } else {
                    self.inner.on_trace(&LoopTraceEvent::TextEmitted {
                        iteration: *iteration,
                        stream: *stream,
                        text: redacted,
                    });
                }
            }
            LoopTraceEvent::SessionCompleted {
                final_text: Some(t),
                ..
            } => {
                let redacted = self.masker.mask(t);
                if redacted == *t {
                    self.inner.on_trace(event);
                } else {
                    // Clone the whole event and overwrite only final_text;
                    // the other fields (outcome, tokens, timeline…) are
                    // preserved verbatim.
                    let mut ev = event.clone();
                    if let LoopTraceEvent::SessionCompleted { final_text, .. } = &mut ev {
                        *final_text = Some(redacted);
                    }
                    self.inner.on_trace(&ev);
                }
            }
            other => self.inner.on_trace(other),
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }

    fn on_init_seam(&self, stage: &'static str, seam: &'static str, configured: bool) {
        self.inner.on_init_seam(stage, seam, configured);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::trace::{LoopTraceSessionOutcome, LoopTraceTextKind};
    use std::sync::Mutex;

    #[derive(Default)]
    struct CaptureSink {
        events: Mutex<Vec<LoopTraceEvent>>,
    }
    impl TraceSink for CaptureSink {
        fn on_trace(&self, event: &LoopTraceEvent) {
            self.events.lock().unwrap().push(event.clone());
        }
        fn flush(&self) {}
    }

    #[test]
    fn redacts_secret_in_text_emitted() {
        let cap = Arc::new(CaptureSink::default());
        let sink = UnattendedRedactingSink::new(cap.clone());
        sink.on_trace(&LoopTraceEvent::TextEmitted {
            iteration: 1,
            stream: LoopTraceTextKind::Final,
            text: "the key is sk-ant-api03-AAAABBBBCCCCDDDD".into(),
        });
        let events = cap.events.lock().unwrap();
        match &events[0] {
            LoopTraceEvent::TextEmitted { text, .. } => {
                assert!(!text.contains("sk-ant-api03-AAAABBBBCCCCDDDD"), "secret leaked: {text}");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn passes_clean_text_through_unchanged() {
        let cap = Arc::new(CaptureSink::default());
        let sink = UnattendedRedactingSink::new(cap.clone());
        sink.on_trace(&LoopTraceEvent::TextEmitted {
            iteration: 1,
            stream: LoopTraceTextKind::Final,
            text: "just a normal sentence".into(),
        });
        let events = cap.events.lock().unwrap();
        match &events[0] {
            LoopTraceEvent::TextEmitted { text, .. } => assert_eq!(text, "just a normal sentence"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn redacts_secret_in_session_completed_final_text() {
        let cap = Arc::new(CaptureSink::default());
        let sink = UnattendedRedactingSink::new(cap.clone());
        sink.on_trace(&LoopTraceEvent::SessionCompleted {
            outcome: LoopTraceSessionOutcome::Completed,
            iterations: 1,
            tool_calls_made: 0,
            total_tokens: 0,
            hit_limit: false,
            final_text: Some("done, token AKIAIOSFODNN7EXAMPLE used".into()),
            terminate_reason: None,
            duration_ms: None,
            token_breakdown: None,
            tool_timeline: Vec::new(),
        });
        let events = cap.events.lock().unwrap();
        match &events[0] {
            LoopTraceEvent::SessionCompleted { final_text, .. } => {
                assert!(!final_text.as_ref().unwrap().contains("AKIAIOSFODNN7EXAMPLE"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
```

> NOTE for implementer: the test constructs `LoopTraceEvent::SessionCompleted` and references
> `LoopTraceSessionOutcome` + `LoopTraceTextKind`. Confirm the exact variant name of the outcome enum
> in `src/harness/trace.rs` (search `enum LoopTraceSessionOutcome`) and use a real variant (e.g.
> `Completed`/`Success`); adjust the import/variant if the name differs. If `SecretMasker`'s default
> patterns do not match the exact sample token, pick a token that one of `secret_masker_patterns()`
> regexes matches (e.g. a `sk-ant-` or `AKIA…` shape) — verify against `src/exec/secret_patterns.rs`.

- [ ] **Step 2: Register the module** in `src/gateway/execution_engine/mod.rs`. Add the module
  declaration alongside the other `mod` lines and re-export the type next to `GatewayTraceSink`/
  `AgentTraceEmitSink` (search those identifiers to find the existing `pub use`/`mod` block, and mirror
  the pattern):

```rust
mod unattended_redacting_sink;
pub use unattended_redacting_sink::UnattendedRedactingSink;
```

- [ ] **Step 3: Wire it into the run loop.** In `src/gateway/execution_engine/run_loop.rs`, immediately
  AFTER the `AgentTraceEmitSink` wrap (after line 861, before the `// SubagentTool construction`
  comment at line 863), insert:

```rust
            // Unattended security-tax (observability side): when no human is
            // watching, redact model-authored text before it reaches
            // persistence / the channel push / the WebSocket. Outermost wrap so
            // it sees every event first; attended runs are never wrapped.
            let trace_sink: Arc<dyn crate::harness::TraceSink> = if unattended {
                Arc::new(super::UnattendedRedactingSink::new(trace_sink))
            } else {
                trace_sink
            };
```

- [ ] **Step 4: Commit**

```bash
git add src/gateway/execution_engine/unattended_redacting_sink.rs src/gateway/execution_engine/mod.rs src/gateway/execution_engine/run_loop.rs
git commit -m "execution_engine: redact secrets in unattended-run trace stream"
```

---

## Feature 3 — Promote goal lessons into long-term memory

### Task 6: `GoalStore::list_all`

**Files:**
- Modify: `src/goal/store.rs`

- [ ] **Step 1: Add the enumeration method** after `delete` (after line 83, before the closing `}` of the
  `impl` block at line 84):

```rust
    /// Enumerate all stored goals (one row per session). Corrupt rows are
    /// skipped (fail-safe, mirroring `get`). Used by the dream lessons-promotion
    /// stage to sweep lessons into long-term memory.
    pub fn list_all(&self) -> Result<Vec<Goal>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT json FROM goals")
            .map_err(|e| AlephError::other(format!("goal list_all prepare: {e}")))?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| AlephError::other(format!("goal list_all query: {e}")))?;
        let mut goals = Vec::new();
        for row in rows {
            let json = row.map_err(|e| AlephError::other(format!("goal list_all row: {e}")))?;
            if let Ok(goal) = serde_json::from_str::<Goal>(&json) {
                goals.push(goal); // corrupt rows skipped, like `get`.
            }
        }
        Ok(goals)
    }
```

- [ ] **Step 2: Add tests** in the `tests` module:

```rust
    #[test]
    fn list_all_returns_every_session_goal() {
        let (store, _d) = temp_store();
        store.put(&Goal::new("sess-1", "a", 0, 0)).unwrap();
        store.put(&Goal::new("sess-2", "b", 0, 0)).unwrap();
        let all = store.list_all().unwrap();
        assert_eq!(all.len(), 2);
        let mut objs: Vec<&str> = all.iter().map(|g| g.objective.as_str()).collect();
        objs.sort_unstable();
        assert_eq!(objs, vec!["a", "b"]);
    }

    #[test]
    fn list_all_empty_when_no_goals() {
        let (store, _d) = temp_store();
        assert!(store.list_all().unwrap().is_empty());
    }
```

- [ ] **Step 3: Commit**

```bash
git add src/goal/store.rs
git commit -m "goal store: add list_all to enumerate goals for lessons promotion"
```

---

### Task 7: `GoalLessonsPromoteStage` dream stage

**Files:**
- Create: `src/memory/dreaming/stages/goal_lessons_promote.rs`
- Modify: `src/memory/dreaming/stages/mod.rs` (module decl + re-export)
- Modify: `src/memory/dreaming/mod.rs` (Consolidate registration + GLOBAL_ONLY_STAGES + pipeline test)
- Modify: `src/memory/dreaming/report.rs` (report metric)

- [ ] **Step 1: Add the report metric.** In `src/memory/dreaming/report.rs`, in the `DreamReport`
  struct, after the `notes_woven` field, add:

```rust
    /// Goal lessons promoted into long-term notes by `GoalLessonsPromoteStage`.
    #[serde(default)]
    pub goal_lessons_promoted: u32,
```

- [ ] **Step 2: Write the stage** at `src/memory/dreaming/stages/goal_lessons_promote.rs`:

```rust
//! GoalLessonsPromoteStage — graduate goal "lessons" into long-term memory.
//!
//! `Goal.lessons` is a ring buffer (cap `MAX_LESSONS`) injected into
//! continuation prompts but otherwise ephemeral: dropped past the cap and gone
//! when the goal is cleared. This stage promotes each goal's current lessons
//! into a per-goal note so they survive the ring and the goal's deletion, and
//! can inform future goals (R9 — the article's "state file" becomes durable).
//!
//! Idempotency: `append_to_note` does NOT dedup facts, so the stage reads the
//! existing note's facts and appends only genuinely-new ones. Stable when
//! nothing is new; union-preserving across cycles (a promoted lesson stays even
//! after the ring drops it). Goals are reached via the process-global
//! `crate::goal::global()` (no DreamContext wiring); a store may be injected for
//! tests. Global-only (goals are not project-namespaced).

use async_trait::async_trait;
use tracing::{info, warn};

use crate::error::AlephError;
use crate::goal::GoalStore;
use crate::memory::dreaming::DreamContext;
use crate::memory::notes::KnowledgeNote;
use crate::sync_primitives::Arc;

use super::DreamStage;

/// Category (directory) under which per-goal lesson notes are written.
const LESSONS_CATEGORY: &str = "goal-lessons";

#[derive(Default)]
pub struct GoalLessonsPromoteStage {
    /// Test-injectable goal store. `None` → resolve the process global.
    pub store: Option<Arc<GoalStore>>,
}

impl GoalLessonsPromoteStage {
    fn resolve_store(&self) -> Option<Arc<GoalStore>> {
        self.store.clone().or_else(crate::goal::global)
    }
}

#[async_trait]
impl DreamStage for GoalLessonsPromoteStage {
    fn name(&self) -> &'static str {
        "goal_lessons_promote"
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let Some(store) = self.resolve_store() else {
            return Ok(ctx); // no goal store wired (e.g. tests) → no-op.
        };
        let goals = match store.list_all() {
            Ok(g) => g,
            Err(e) => {
                warn!(error = %e, "GoalLessonsPromote: goal enumeration failed");
                return Ok(ctx);
            }
        };

        let mut promoted = 0u32;
        for goal in goals {
            if goal.lessons.is_empty() {
                continue;
            }
            // Deterministic, filesystem-safe path: goal.id is a stable hash.
            let path = format!("{LESSONS_CATEGORY}/{}", goal.id);

            // Read existing facts to dedup (append_to_note does NOT dedup facts).
            let existing: Vec<String> = match ctx.load_content(&path).await {
                Some(md) => KnowledgeNote::from_markdown(&goal.id, &md)
                    .map(|n| n.facts)
                    .unwrap_or_default(),
                None => Vec::new(),
            };

            // Desired facts: the objective (for human context) + each lesson.
            let mut desired: Vec<String> = Vec::with_capacity(goal.lessons.len() + 1);
            desired.push(format!("Objective: {}", goal.objective));
            desired.extend(goal.lessons.iter().cloned());

            let new_facts: Vec<String> = desired
                .into_iter()
                .filter(|f| !existing.contains(f))
                .collect();
            if new_facts.is_empty() {
                continue; // already promoted; idempotent no-op.
            }

            match ctx
                .indexer
                .append_to_note(&ctx.agent_id, &path, &new_facts, &[])
                .await
            {
                Ok(()) => {
                    promoted += new_facts.len() as u32;
                    // Evict the now-stale cached content (mirrors NoteWeave).
                    ctx.note_contents.remove(&path);
                }
                Err(e) => warn!(path = %path, error = %e, "GoalLessonsPromote: append failed"),
            }
        }

        ctx.report.goal_lessons_promoted = promoted;
        if promoted > 0 {
            info!(promoted, "GoalLessonsPromote completed");
        }
        Ok(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal::Goal;
    use crate::memory::dreaming::{DreamContext, DreamReport, DreamStrategy};
    use crate::memory::embedding_provider::EmbeddingProvider;
    use crate::memory::notes::NoteIndexer;
    use crate::memory::store::SqliteMemoryBackend;
    use crate::providers::mock::MockProvider;

    struct StubEmbedder;
    #[async_trait::async_trait]
    impl EmbeddingProvider for StubEmbedder {
        async fn embed(&self, _t: &str) -> Result<Vec<f32>, AlephError> {
            Ok(Vec::new())
        }
        async fn embed_batch(&self, _t: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
            Ok(Vec::new())
        }
        fn dimensions(&self) -> usize {
            0
        }
        fn model_name(&self) -> &str {
            "stub"
        }
        fn provider_id(&self) -> &str {
            "stub"
        }
    }

    async fn build_ctx() -> (DreamContext, std::path::PathBuf) {
        let temp = std::env::temp_dir().join(format!("aleph_lessons_{}", uuid::Uuid::new_v4()));
        let store = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());
        let indexer = NoteIndexer::new(temp.clone(), store.clone());
        let provider: std::sync::Arc<dyn crate::providers::AiProvider> =
            std::sync::Arc::new(MockProvider::new("{}"));
        let embedder: std::sync::Arc<dyn EmbeddingProvider> = std::sync::Arc::new(StubEmbedder);
        let ctx = DreamContext {
            notes: Vec::new(),
            note_contents: std::collections::HashMap::new(),
            agent_id: "default".into(),
            database: store.clone(),
            indexer,
            provider,
            embedder,
            report: DreamReport::default(),
            pipeline_type: "consolidate".into(),
            activity_checker: std::sync::Arc::new(|| false),
            strategy: DreamStrategy::Consolidate,
            orientation: None,
        };
        (ctx, temp)
    }

    fn goal_store_with(goals: &[Goal]) -> (Arc<GoalStore>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(GoalStore::open(&dir.path().join("g.db")).unwrap());
        for g in goals {
            store.put(g).unwrap();
        }
        (store, dir)
    }

    #[tokio::test]
    async fn promotes_lessons_into_a_note() {
        let (ctx, _t) = build_ctx().await;
        let goal = Goal::new("sess-1", "Migrate auth", 0, 0)
            .with_lesson_appended("run migrations first".into(), 1);
        let (gstore, _gd) = goal_store_with(&[goal]);
        let stage = GoalLessonsPromoteStage {
            store: Some(gstore),
        };
        let out = stage.execute(ctx).await.unwrap();
        // Objective + 1 lesson = 2 new facts promoted.
        assert_eq!(out.report.goal_lessons_promoted, 2);
    }

    #[tokio::test]
    async fn second_run_is_idempotent() {
        let (ctx, _t) = build_ctx().await;
        let goal = Goal::new("sess-1", "Migrate auth", 0, 0)
            .with_lesson_appended("run migrations first".into(), 1);
        let (gstore, _gd) = goal_store_with(&[goal]);
        let stage = GoalLessonsPromoteStage {
            store: Some(gstore),
        };
        let ctx = stage.execute(ctx).await.unwrap();
        // Re-run over the same ctx (note already on disk) → nothing new.
        let out = stage.execute(ctx).await.unwrap();
        assert_eq!(out.report.goal_lessons_promoted, 0, "no duplicate facts");
    }

    #[tokio::test]
    async fn goal_without_lessons_is_skipped() {
        let (ctx, _t) = build_ctx().await;
        let goal = Goal::new("sess-1", "no lessons yet", 0, 0);
        let (gstore, _gd) = goal_store_with(&[goal]);
        let stage = GoalLessonsPromoteStage {
            store: Some(gstore),
        };
        let out = stage.execute(ctx).await.unwrap();
        assert_eq!(out.report.goal_lessons_promoted, 0);
    }

    #[tokio::test]
    async fn no_goal_store_is_graceful_noop() {
        let (ctx, _t) = build_ctx().await;
        let stage = GoalLessonsPromoteStage::default(); // None → global (unset in test)
        let out = stage.execute(ctx).await.unwrap();
        assert_eq!(out.report.goal_lessons_promoted, 0);
    }
}
```

> NOTE for implementer: confirm `KnowledgeNote` is re-exported from `crate::memory::notes` (the
> note_weave test imports `crate::memory::notes::KnowledgeNote`). Confirm `crate::goal::global` and
> `crate::goal::GoalStore` import paths (the goal builtin tool imports `crate::goal::GoalStore`; the
> continuation hook calls `crate::goal::global()`). If `MockProvider::new` requires a non-empty
> response, `"{}"` is fine — this stage never calls the provider. If `from_markdown`'s first arg is a
> title rather than filename, `&goal.id` is still acceptable (it is only used to label the parsed note).

- [ ] **Step 3: Export the stage** in `src/memory/dreaming/stages/mod.rs`. Add the module decl
  (alphabetically near the others, after `pub mod feedback_distill;`):

```rust
pub mod goal_lessons_promote;
```

  and the re-export (after `pub use feedback_distill::FeedbackDistillStage;`):

```rust
pub use goal_lessons_promote::GoalLessonsPromoteStage;
```

- [ ] **Step 4: Register on the Consolidate pipeline.** In `src/memory/dreaming/mod.rs`
  `from_strategy`, in the `DreamStrategy::Consolidate` vec, after the `SkillLifecycleStage { ... }`
  entry (the last element, ending around line 192), add as the new final element:

```rust
                // Graduate goal lessons (Round 2 state file) into durable notes
                // so insights survive the ring buffer and goal deletion. Cheap
                // no-op when no goal has new lessons. Global-only (goals are not
                // project-namespaced).
                Box::new(stages::GoalLessonsPromoteStage::default()),
```

- [ ] **Step 5: Mark it global-only.** In the `GLOBAL_ONLY_STAGES` const (line 238), add:

```rust
        "goal_lessons_promote",
```

- [ ] **Step 6: Update the Consolidate pipeline enumeration test** (`mod.rs` line ~1154). Add
  `"goal_lessons_promote",` as the new final entry after `"skill_lifecycle",`:

```rust
                "note_weave",
                "note_decay",
                "skill_lifecycle",
                "goal_lessons_promote",
```

- [ ] **Step 7: Commit**

```bash
git add src/memory/dreaming/stages/goal_lessons_promote.rs src/memory/dreaming/stages/mod.rs src/memory/dreaming/mod.rs src/memory/dreaming/report.rs
git commit -m "dream: promote goal lessons into long-term notes (global-only consolidate stage)"
```

---

## Final verification (controller, static — no cargo)

1. `grep -n "should_continue(" src/` and `grep -n "exhausted_while_active(" src/` → EVERY call has 3
   args (goal, tokens, now_ms). No old 2-arg arity anywhere.
2. `grep -c "GoalArgs {" src/builtin_tools/goal.rs` vs `grep -c "timeout_minutes:" src/builtin_tools/goal.rs`
   → every literal has the field (count matches, accounting for the struct definition line).
3. Consolidate pipeline test vec includes `goal_lessons_promote`; `GLOBAL_ONLY_STAGES` includes it.
4. `deadline_ms` field has `#[serde(default)]`; new dream report field has `#[serde(default)]`.
5. `UnattendedRedactingSink` is in `src/gateway/`, not `src/harness/`. No file under `src/harness/`
   changed.
