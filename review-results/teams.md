# Module: teams

**Date**: 2026-07-19
**Reviewers**: 4 parallel agents (security × 2, logic, quality)

## Summary
- Path: `src/teams/` (~38k LOC across ~45 files)
- Raw issues found: ~80
- After filtering (high-confidence only): 2

## High-Confidence Issues (will fix)

### 1. `set_protocol` accepts unbounded text → leader prompt injection — MEDIUM (security)
- **File**: `src/teams/store.rs:667-693`
- **Description**: `team.protocol` is rendered verbatim into the leader's system prompt by `leader_prompt::build`. An unbounded `set_protocol` lets whoever can call it (anyone with team.modify permission) inject arbitrary instructions into the leader context — a prompt-injection sink that persists across the team's lifetime.
- **Fix**: Cap normalized protocol text at 32 KiB before storing.

### 2. `create_artifact` accepts unbounded content — MEDIUM (security/DoS)
- **File**: `src/teams/artifacts.rs:381`
- **Description**: `NewArtifact.content` is stored as TEXT in SQLite with no application-level size cap. A single artifact with a multi-GB content string causes unbounded DB growth and synchronous materialisation into memory on every `read_artifact_row` (which deserialises the full content into a Rust `String`).
- **Fix**: Reject artifacts whose content exceeds 1 MiB before INSERT.

## Skipped Issues (low signal / design choices / high risk)

- **MessageRouter `from_agent` spoofing** — the router is an in-process singleton used by trusted internal call sites; adding membership checks would be a structural change. Deferred to broader auth model review.
- **Plan approval workflow bypass** (plans.rs:101,147) — `leader_id` parameter is unverified. PlanManager doesn't hold a TeamStore reference, so a structural change is needed. Deferred.
- **Session creation authorization gap** (sessions/coordinator.rs:42) — `start_session` accepts arbitrary participants / leader_id. Same structural fix needed as plan approval.
- **ACP runner accepts arbitrary cwd without worktree isolation** (runner.rs:76) — design decision for ACP harness integration.
- **ACP sessions reused across teams/tasks** (runner.rs:412) — same design decision.
- **Handoff / broadcast prompt injection from member content** (handoff.rs, broadcast/member_prompt.rs) — defensive prompt construction; broader design question of how to fence untrusted inter-member content.
- **Dispatcher scheduling races** (schedule/{mod,select,reclaim,failure}.rs) — multi-step atomic CAS operations across SQLite/Redis; requires architectural refactor.
- **Workflow settlement cross-team merge** (settle.rs:133) — design choice of using run-id as global grouping key.
- **Snapshot store / restore non-transactional** (snapshots/operations.rs:231) — needs broader audit of restore semantics.
- **mark_read / row reader silent failures** — pervasive pattern; needs a project-wide `.ok_or_default()` audit rather than a per-site fix.
- **P2 file-size violations** in store.rs (1167), artifacts.rs (1031), sessions/coordinator.rs (570), messages/store.rs (1264), messages/router.rs (503), messages/aggregator.rs (539), dispatcher/schedule/select.rs (858), broadcast/mod.rs (655), plans.rs (~600 with deps) — refactor would touch every test.
- **Function-length violations** — coordination functions; refactor risk.
- **Message materialization duplication** in messages/store.rs (3 sites) — mechanical refactor with divergence risk.
- **Participant authorization duplication** in coordinator.rs (3 sites) — refactor risk.
- **`integration_tests` exposed as `pub mod`** — cosmetic.
- **Dead code**: `TaskStatus::can_transition_to`, `complete_task`, `update_status` (only test callers), `flush_team`/`router` on Aggregator, `TeamStatus::as_str`, `_types_used`. Needs wider audit before removal.
- **`acp_member_id` no input validation** — lowercase cosmetic; routed via store.
- **Mention parser** — no bugs found; correctness tests pass.

## Status
- 2 high-confidence issues fixed.
- Committed without per-module `cargo check` per user instruction.
- Full project `cargo check` deferred to end of sweep.