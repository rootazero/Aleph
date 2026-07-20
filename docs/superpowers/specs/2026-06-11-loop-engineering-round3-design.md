# Loop Engineering Round 3 — Unattended Redaction / Per-Goal Timeout / Lessons→Memory

**Goal:** Implement the three features Round 2 explicitly deferred as out-of-scope future candidates:
unattended-run log secret-redaction, per-goal wall-clock timeout, and promotion of goal lessons
into long-term memory via the Dream daemon.

**Architecture:** All three are wiring-first extensions of existing infrastructure. They land in the
loop layer (`src/goal/`, `src/tasks/goal_pursuit.rs`, `src/gateway/execution_engine/`,
`src/memory/dreaming/`). `src/harness/` is **untouched** (R10 12-file redline preserved).

**Tech Stack:** Rust, Tokio, rusqlite (JSON-blob goal store), the existing `SecretMasker`, `TraceSink`
decorator pattern, and the `DreamStage` pipeline.

---

## Source & lineage

Continues the loop-engineering layer (Round 1 = objective gate; Round 2 = per-goal gate command +
lessons state-file + unattended fail-closed). Round 2 closed by naming three out-of-scope candidates;
the user confirmed (`确认再起`) to implement all three now. Article driver unchanged:
*Loop engineering — the 14-step roadmap*.

---

## Feature 1 — Unattended-run secret redaction (the security-tax, observability side)

### Problem
Round 2 made unattended runs fail **closed on tool confirmation** (`ScopedToolService.unattended`).
But an unattended run's **trace stream** (model text the loop emits) still flows verbatim into
persistence, the channel progress push, and the WebSocket stream. If an autonomous loop reads a secret
and echoes it in its reasoning, that secret is persisted/streamed with no human watching. `tracing::`
log lines already pass the global PII layer (`src/logging/pii_filter.rs`); the **gap is the
trace-event stream**.

### Design
A `TraceSink` **decorator** — `UnattendedRedactingSink` — installed as the **outermost** sink in the
run-loop chain **only when `unattended == true`**. It redacts the two model-text-bearing variants of
`LoopTraceEvent` before forwarding to the inner chain (persistence + scratchpad push + WS emit):

- `TextEmitted { text }` → `SecretMasker::mask(text)`
- `SessionCompleted { final_text: Some(t) }` → `final_text = mask(t)`
- all other variants forwarded by reference, unchanged (wildcard arm — `#[non_exhaustive]` safe).

Tool-result text is **deliberately not** redacted here: sandbox exec output is already scrubbed at the
sandbox boundary (`src/sandbox/scrub.rs`), and redacting `ToolResult` would add brittle structural
coupling for marginal gain. Scope = model-authored text.

Redactor uses `SecretMasker` (self-contained, deterministic, pattern-based — `sk-*`, `AKIA*`, Bearer,
PEM blocks, etc.). No global state, no init dependency → safe in tests and headless cron runs.

### Wiring (zero new threading)
`unattended` is already bound at `run_loop.rs:721`. The trace-sink chain is built inner→outer at
`run_loop.rs:826–861`, ending with `AgentTraceEmitSink`. A single insertion **after line 861**
(before the sink is cloned into `SubagentTool` and `FlowRequest`) wraps it when unattended. The new
decorator lives in `src/gateway/execution_engine/` (a consumer module, **not** `src/harness/`).

### Redline compliance
- R10: decorator in `src/gateway/`, not `src/harness/`. Zero harness file changes.
- Default path (attended) is byte-for-byte unchanged: the wrap is `if unattended { ... }`.

---

## Feature 2 — Per-goal wall-clock timeout

### Problem
A goal can carry an iteration cap (`PursuitMode::Active { max_iterations }`) and a soft token budget,
but **no wall-clock bound**. An autonomous goal whose iterations are slow can run for hours of real
time before the iteration cap is hit. The user should be able to say "pursue this for at most 30
minutes, then hand back".

### Design
Add one field to `Goal` (`src/goal/types.rs`):

```rust
/// Optional wall-clock deadline (Unix epoch ms). When set and exceeded, the
/// autonomous loop stops re-pursuing and blocks the goal for the user — a
/// structural stop condition alongside the iteration/token caps (R7: no
/// judgment, pure time comparison). `#[serde(default)]` → old payloads read None.
#[serde(default)]
pub deadline_ms: Option<u64>,
```

Plus a configuration-style mutator (no `updated_at_ms` bump, mirroring `with_budget`/`with_pursuit`):

```rust
pub const fn with_deadline_ms(mut self, deadline_ms: Option<u64>) -> Self { ... }
```

**Single-source the stop condition (P2 high cohesion):** fold the deadline into the existing
`should_continue` predicate rather than checking it separately. This means `should_continue` and
`exhausted_while_active` gain a `now_ms: u64` parameter:

```rust
pub fn should_continue(goal: &Goal, tokens_now: u64, now_ms: u64) -> bool {
    // ... existing iteration + token checks ...
    if let Some(deadline) = goal.deadline_ms {
        if now_ms != 0 && now_ms > deadline {
            return false; // wall-clock budget exhausted
        }
    }
    true
}
```

The `now_ms != 0` guard keeps existing call sites that pass `0` (no live clock) behavior-identical —
a `None` deadline never triggers anyway, so the guard only matters for the rare "deadline set but
caller has no clock" path, which conservatively does NOT stop (the iteration cap is the backstop).

Because the deadline is folded into `should_continue`, `exhausted_while_active` (which is
`Active pursuit && Active status && !should_continue`) **automatically** transitions a deadline-expired
goal to `Blocked` via the existing exhaustion branch in `execute.rs` — no new branch needed. The
continuation hook call sites change from `should_continue(&goal, 0)` / `exhausted_while_active(&goal, 0)`
to pass the already-computed `now_ms` (`execute.rs:630-632`).

### LLM interface (R8 everything-is-a-tool)
`GoalArgs` (`src/builtin_tools/goal.rs`) gains:

```rust
/// For `set`: wall-clock budget in minutes. Converted to an absolute
/// deadline (now + minutes) at set time. None = no time limit.
pub timeout_minutes: Option<u32>,
```

The `Set` handler computes `deadline_ms = now_ms() + (minutes as u64 * 60_000)` and calls
`with_deadline_ms`. `render()` shows "deadline: in N min" when set; DESCRIPTION + one example mention it.

### Blocking note
`cap_reached_note` distinguishes the reason: when the deadline is the cause, the note says the
**wall-clock budget** was reached (not the iteration cap), so the user sees why the loop stopped.
A small helper inspects whether the deadline was the binding constraint.

### Redline compliance
- R7: time comparison is structural, not judgment.
- R10: lives in `src/goal/` + `src/tasks/` + `src/gateway/`, not `src/harness/`.
- Backward compat: `#[serde(default)]` → old goals read `deadline_ms = None`, behavior unchanged.

---

## Feature 3 — Promote goal lessons into long-term memory (Dream daemon)

### Problem
`Goal.lessons` (Round 2) is a ring buffer capped at `MAX_LESSONS = 5`, injected into continuation
prompts but **ephemeral**: dropped past the cap and gone when the goal is cleared. Hard-won insights
("forgot to run migrations", gate-veto reasons) never reach long-term memory to inform future goals.

### Design
A new Dream stage — `GoalLessonsPromoteStage` (`name() = "goal_lessons_promote"`) — that on each dream
cycle promotes the current lessons of every goal into a per-goal note, preserving them past the ring
buffer and past goal deletion.

**Goal-store access:** the stage reaches goals via the **process-global singleton**
`crate::goal::global() -> Option<Arc<GoalStore>>` (the same accessor the continuation hook uses,
`execute.rs:625`). **No DreamContext wiring needed** — this avoids threading `GoalStore` through the
dream dependency context.

**Enumeration:** `GoalStore` currently has only `put`/`get`. Add:

```rust
/// Enumerate all stored goals (one row per session). Corrupt rows are
/// skipped (fail-safe), mirroring `get`. Used by the dream lessons-promotion
/// stage to sweep lessons into long-term memory.
pub fn list_all(&self) -> Result<Vec<Goal>> { ... }
```

**Note shape:** one note per goal at deterministic path `goal-lessons/<sanitized objective>` (so
re-runs target the same note). Facts = the lesson strings; tag `goal-lesson`.

**Idempotency (verified, not assumed):** `NoteIndexer::append_to_note` deduplicates **links but NOT
facts** (`indexer.rs:455` — `note.facts.extend(...)`). So the stage must dedup itself: it reads the
existing note's facts (via `DreamContext::load_content` + `KnowledgeNote::from_markdown`) and appends
**only lessons not already present**. This is idempotent (stable when nothing new) and **union-preserving
across cycles**: once a lesson is promoted it stays in the note even after the ring buffer drops it —
exactly the "survive past the cap" goal. Cheap no-op when there is nothing new.

**Registration:** appended to the **Consolidate** pipeline (the daily/frequent strategy) in
`DreamPipeline::from_strategy`, after `skill_lifecycle` (independent of note linking/decay). The
pipeline enumeration test (`mod.rs:1155`) is updated to include the new stage name.

**Report metric:** `DreamReport` gains `#[serde(default)] pub goal_lessons_promoted: u32`, incremented
by the stage.

### Redline compliance
- R3 core-minimalism / wiring-first: reuses `goal::global()`, `append_to_note`, the `DreamStage` trait.
  No new subsystem.
- R10: lives in `src/memory/dreaming/` + `src/goal/store.rs`, not `src/harness/`.
- R9: lessons (the article's "state file") graduate from prompt-ephemeral to durable memory.

---

## Cross-cutting

- **Entropy reduction:** no dead code introduced. The `now_ms` parameter unifies the stop conditions
  (does not duplicate them).
- **Testing (resource governance):** tests are written per TDD but **not executed locally** (no
  `cargo check`/`test` — shared target-dir governance). Controller performs static review +
  mechanical per-call-site verification of the `should_continue`/`exhausted_while_active` signature
  change (the Round 2 "trust-no-self-report, verify every literal" lesson).
- **Branch isolation:** all work in worktree `loop-engineering-round3`; merge to main via `--no-ff`.

## Out of scope (do not start)
- Per-lesson provenance/timestamps (lessons stay plain strings).
- Redacting tool-result trace variants (sandbox already scrubs exec output).
- Promoting lessons that were dropped from the ring *before* the first dream cycle (unrecoverable by a
  poll-time stage — accepted limitation).
