# Builtin Tools Batch 3 — team/* Code Review

**Date**: 2026-08-11
**Path**: `src/builtin_tools/team/*` (31 files, ~7503 lines)
**Reviewer**: static (security / logic / architecture / quality)
**Threshold**: all findings actionable; no scoring pass.

## Module Totals

| Critical | High | Medium | Low | Total |
|---------:|-----:|-------:|----:|------:|
|        0 |    0 |     1 |   3 |    4 |

---

## Findings

### [MEDIUM] team/delegate.rs:411,558 — delegated run `depth: 1` is hard-coded, not derived
- **Category**: architecture
- **Description**: The leader-driven `team_delegate` path registers the member run under `SpawnMeta { parent_id: None, depth: 1, ... }` unconditionally. The depth field is what `background_tracker` uses to refuse cycles and cap spawn depth; hardcoding `1` makes "depth 2" (e.g. a member that itself delegates) report as depth-1 in the tracker, which then *under*-estimates the true nesting. A pathological chain of delegations could therefore slip past depth-based limits while appearing shallow.
- **Suggested fix**: Read the leader's own depth from `acting_agent::depth()` (or equivalent) and pass `depth: parent_depth + 1`. If a true depth API does not exist, thread it through `ToolContext` the same way `acting_agent_id` already is.

### [LOW] team/message_send.rs, task_submit.rs, task_comment.rs — payload (`content` / `body`) has no size cap
- **Category**: DoS
- **Description**: `MessageSendArgs.content`, `TaskSubmitArgs.content`, and the comment `body` fields are `String` with no length bound at the dispatcher. A model can stuff hundreds of MB into one message and have the message router fan it out to every member before anyone notices.
- **Suggested fix**: Add a single constant (suggest 256 KiB — well past a real deliverable) at the top of `team/mod.rs` and check it in each tool's `call`. Reject with `AlephError::tool(...)` and a hint pointing at chunked artifact delivery for genuinely large outputs.

### [LOW] team/task_control.rs — `cancel` exposes `cancel_allows_in_progress_but_rejects_settled` (test only); production path lacks a `MAX_RETRY_ATTEMPTS`-like audit
- **Category**: architecture / observability
- **Description**: `cancel` is gated by task status, which is correct, but there is no log when a `cancel` is issued against a task that the dispatcher is *already* settling. A cancel that races the settle produces a transient "Skipped stays allowed" branch (comment at line 259) that swallows the outcome; reviewers see no evidence either way.
- **Suggested fix**: At the swallowed-error branch, emit `tracing::info!(task_id, previous_status, "cancel raced settle")`. Pure observability nit; no behavior change.

### [LOW] team/delegate.rs — `MemberRunStatus::Busy` deferral has no upper bound on re-attempts
- **Category**: architecture
- **Description**: When the member agent is busy, `team_delegate` returns `DelegateStatus::Busy` and leaves the task claimable; the dispatcher re-queues. There is no exponential backoff or max-attempt counter; a busy member under sustained load would re-queue forever and the leader's `team_delegate` returns would never reflect a real failure.
- **Suggested fix**: Add a `defer_count` field on the task row (or in the coord store); refuse re-claim after N deferrals and surface `DelegateStatus::Abandoned` instead. Mirrors the worktree-budget pattern in `delegate.rs:387`.

---

## Strengths

- `delegate.rs` correctly uses RAII for the running-registration fence (`settle_fence.defuse()` + `drop(running_reg)`), and the `goal_budget::check_and_enroll_delegation` preflight is the right shape for cross-agent budget enforcement.
- `task_review.rs` is fail-closed on unknown leadership (`authz_allows_leader_blocks_other_allows_unknown` test): unverifiable team → allow (prompt-gating) is the documented escape, and the verdict vocabulary stays aligned with `loop_graph` anchors.
- `workflow_canvas.rs` bounds the topological pass at `work.len() + 1` — exactly the right cap for cycle detection ("if a pass ran without reducing `work.len()`, you have a cycle").
- `message_send.rs` silently drops unknown `@mention` handles rather than erroring, which is the right UX choice for chat content.
- `task_control.rs` has the `CancelAllowsInProgressButRejectsSettled` invariant under test (line 592) — meaning the cancel state machine was designed against an explicit spec, not bolted on.
- `acp_member.rs` validates agent refs structurally before lookup, never treating an ACP reference as a registry member and vice-versa.

---

## Single Recommended Fix

The MEDIUM finding (hardcoded `depth: 1`) is the only one with a real architectural consequence. The three LOW findings are observations a future hardening pass can absorb together with similar ones from `sessions/` and `task_manage/`. None are blockers for a `team` review pass.