# Loop Engineering — Round 2 Hardening Design

> Continuation of `2026-06-11-loop-engineering-goal-gate-design.md` (Round 1 wired
> the objective gate into the autonomous goal loop). This round implements the
> three items Round 1 explicitly deferred.

**Date:** 2026-06-11
**Scope:** loop layer only (`src/goal/`, `src/tasks/goal_pursuit.rs`,
`src/gateway/execution_engine/`, `src/tools/scoped/`, `src/builtin_tools/goal.rs`).
**`src/harness/` is untouched** (R10 12-file / ~4900-line budget unchanged).

---

## Background

Round 1 introduced the maker/checker type-state (`GateOutcome`) and ran a
**global** `config.toml [[stop_hooks]]` gate when an autonomous goal claimed
`complete`. Three hardening gaps remained, each named by the Loop-Engineering
article and confirmed against Aleph's current code:

1. **Per-goal gate command** — the article frames each `/goal` as carrying its
   own pass/fail test. Aleph's gate is one global hook list shared by every
   goal; a `Goal` cannot declare its own check.
2. **State-file lessons reback** — the article's "state file" lets the agent
   record lessons so the next iteration does not repeat mistakes. Aleph's `note`
   is a single overwritten slot, and `gate_failure_prompt` injects only the
   *latest* failure; accumulated lessons are lost.
3. **Unattended security-tax** — the article warns that unattended loops need
   extra guarding (no human to approve dangerous actions). Aleph's continuation
   `RunRequest` carries no "unattended" marker, so the approval gate treats an
   autonomous continuation exactly like an interactive turn — an `Ask`-tier tool
   would block forever or (worse) be mis-handled with no human present.

All three are **wiring into existing infrastructure** (`ShellStopHook`,
`ApprovalRequester`, `RunRequest.metadata`, the continuation hook). No new
subsystem is introduced.

---

## Redline compliance

- **R7 (LLM sovereignty) / R10 (dumb loop):** every check remains a structural
  shell exit code — never an LLM judge. The per-goal command is a shell command
  (the article's "a test that passes/fails, not an opinion"), not a natural-
  language condition. Completion judgment still lives only in the prompt.
- **R8 (everything-is-a-tool):** per-goal gate command and model-authored
  lessons are exposed through the existing `goal` tool, so the LLM configures
  them by natural-language conversation.
- **R9 (intelligence in the prompt):** accumulated lessons are injected into the
  continuation prompt — the loop's "memory" lives in the prompt text, not in
  Rust control flow.

---

## Feature 1 — Per-goal gate command (AND-supplement)

**Decision:** the per-goal command **supplements** the global gate (logical AND).
Both run; **either** veto vetoes completion. No gate configured anywhere → the
goal's `complete` claim terminates immediately (Round 1 behavior preserved).

**Data:** `Goal` gains `gate_command: Option<String>` (`#[serde(default)]` — the
JSON-blob store round-trips with zero migration). A `set` on the `goal` tool
accepts an optional `gate_command`. The command is a shell line evaluated like a
`[[stop_hooks]]` entry: exit 0 = passed, exit 2 = vetoed (stdout = reason).

**Decision wiring:** `awaiting_gate` already takes a `gate_configured: bool`. The
continuation hook computes it as `global_gate.is_some() || goal.gate_command
.is_some()`, so a goal with its own command is gated even when no global hooks
exist. When the gate must run, the hook assembles an **effective gate vector** =
global hooks (if any) ⧺ a fresh `ShellStopHook` built from `goal.gate_command`
(if any), then runs the combined vector through the existing
`execute_stop_hooks_arc` (which already returns the first halt/block reason —
i.e. AND semantics).

**Why not touch the harness:** the gate runs in the cross-run continuation hook
(`execute.rs`), which is loop-layer, not `src/harness/`.

---

## Feature 2 — State-file lessons reback (auto + model, last 5)

**Decision:** lessons are appended **both** automatically (every gate veto
records its reason) **and** by the model (the `goal` tool's `update` action gains
an optional `lesson` string). A ring cap of **5** keeps the most recent lessons
and prevents unbounded growth.

**Data:** `Goal` gains `lessons: Vec<String>` (`#[serde(default)]`). A new
immutable mutator `with_lesson_appended(self, lesson, now_ms)` pushes the lesson
and truncates the front so at most `MAX_LESSONS = 5` remain. Appending a lesson
is a progress event, so it bumps `updated_at_ms` (like `with_note`).

**Reback:** `reopen_after_gate_failure` appends the gate reason as a lesson (in
addition to the existing single-slot `note`). `continuation_prompt` and
`gate_failure_prompt` render the accumulated lessons (most-recent-last) into the
next autonomous prompt so the loop "remembers" across iterations. Empty lessons
→ no prompt change (regression-safe).

**Surfacing:** `GoalTool::render` shows the lesson count and the most recent
lesson so `goal(get)` reflects the state file.

---

## Feature 3 — Unattended security-tax (fail-closed approvals + audit)

**Decision:** an autonomous continuation run is **unattended** — there is no
human on the channel to approve anything. Such a run **fails closed**: any tool
that would prompt for confirmation (`confirm_tools` ∪ `requires_confirmation` ∪
`Ask`-tier permission) is **auto-denied** with an audit log line and an agent
hint, instead of blocking on an approval that can never arrive.

**Wiring (5 points, all existing seams):**

1. `spawn_continuation_run` stamps `metadata["unattended"] = "true"` on the
   continuation `RunRequest` (covers both the normal-continuation and
   gate-failure-rerun paths — they share this one function).
2. `build_request_tool_service` gains an `unattended: bool` parameter, threaded
   to `ScopedToolService::with_unattended`.
3. `run_loop.rs` computes `unattended` once from `request.metadata` and passes it
   at both `build_request_tool_service` call sites.
4. `ScopedToolService` gains an `unattended: bool` field + `with_unattended`
   builder (default `false`).
5. `confirm_with_memory` short-circuits at its top: when `self.unattended`, it
   logs an audit `warn!`, and returns `ConfirmDenial { outcome: Denied, hint }`
   **before** any approval await. Because the confirmation gate
   (`requires_confirmation` ∪ `Ask`-tier) is the single funnel through
   `confirm_with_memory`, this one point covers all interactive-approval tools.

**Why fail-closed is correct, not a regression:** the user opted into autonomous
pursuit (`pursuit_max_iterations`). The security tax is the price of running
unattended: the loop cannot silently escalate to a confirm-gated action. The
denial hint tells the model to either find a non-gated path or
`goal(update, status='blocked')` for human guidance — the article's intended
guardrail. Interactive turns (no `unattended` marker) are entirely unaffected;
the field defaults `false`, so the hot path is a single bool check.

**The process-wide `ApprovalGate` is NOT touched** — it is shared across all
runs and has no per-run context. The fail-close happens at the per-run
`ScopedToolService`, which already carries per-run state.

---

## Testing strategy

Pure decision functions (`goal_pursuit`, `Goal` mutators) and the `goal` tool
get unit tests. Per the resource-governance constraint this round, tests are
**written but not executed locally** (no `cargo check`); correctness is verified
by static review (type/borrow/path resolution) before commit, matching Round 1.

---

## Out of scope (future)

- Log secret-redaction for unattended runs (a cross-cutting infra concern, a
  different subsystem from the loop layer).
- Per-goal *timeout* / wall-clock budget.
- Lessons → long-term memory promotion (Dream daemon integration).
