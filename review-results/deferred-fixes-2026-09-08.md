# Deferred Fixes — Round 2 (2026-09-08)

Follow-up to `aggregate-2026-09-08-5modules.md`. Three deferred items from
the initial review were completed in this round, all via isolated worktrees
and serial rebase onto main.

## Headline

| Item | Commit | Method |
|------|--------|--------|
| 1. `#[non_exhaustive]` × 23 enums | `897497c13` | subagent: add attributes + verify no exhaustive match sites via `cargo check` |
| 2. `wire_persistence` stale-snapshot race | `c39c1c136` | manual: rewrite as `mpsc::unbounded_channel` + single-worker pipeline |
| 3. God-struct decomposition (`AgentRuntimeConfig` 9 → 3 sub-configs, `AgentRuntime` 27 → 7, `SubagentTool` 33 → 9) | `2dc259762`, `c227d9d0e` | subagent: extract role-grouped sub-configs |

## Per-item detail

### 1. `#[non_exhaustive]` migration (commit `897497c13`)

Added `#[non_exhaustive]` to **23** public enums (the count grew from the
initial estimate of 17 once every file was enumerated):

- `src/agents/types.rs` (6): `AgentSource`, `AgentMode`, `IsolationMode`, `McpServerSpec`, `ContextMode`, `SpawnContext`
- `src/agents/loader.rs` (1): `LoaderError`
- `src/agents/runtime.rs` (1): `TranscriptOutcome`
- `src/agents/background_tracker.rs` (3): `CompletedOutcome`, `WaitOutcome`, `WaitAnyOutcome`
- `src/agents/background_persistence.rs` (1): `RunPhase`
- `src/agents/swarm/tasks/mod.rs` (5): `Priority`, `CoordTaskStatus`, `TaskRunStatus`, `ReviewerKind`, `ReviewVerdict`
- `src/agents/swarm/tasks/retry.rs` (1): `RetryDecision`
- `src/agents/progress.rs` (1): `ProgressKind`
- `src/agents/thinking.rs` (1): `ThinkLevel`
- `src/approval/types.rs` (3): `ActionType`, `ApprovalDecision`, `DefaultDecision`

**Match-site migration**: `cargo check -p alephcore --lib` is clean after
the attribute additions — zero E0004 errors. The 26 candidate files
identified in the prior survey were all reviewed and either:
- were already non-exhaustive (had a `_` arm), or
- used the enum in `==` / `HashMap::insert` / `format!` / `if let` (not
  exhaustive destructuring).

This means the cross-crate blast-radius warning in the original
`aggregate.md` was a worst-case projection; in practice the migration is
zero-call-site change. Downstream crates (`aleph-cli`, `aleph-tui`,
`aleph-panel`, `shared-ui-logic`) were not touched but should now compile
without changes if they ever build against this version.

### 2. `wire_persistence` stale-snapshot race fix (commit `c39c1c136`)

The previous implementation spawned a fresh `tokio::task` per persistence
event, each capturing its own `store.clone()` snapshot mid-flight. The
advisory `fs2` file lock only serialized the *rename* step, not the
snapshot capture — so two concurrent writers could land on disk in
capture-time order, not event order. An older snapshot could clobber a
newer one if it grabbed the file lock last (ACP-R4-03).

**Fix**: replaced the per-event `tokio::spawn` with an
`mpsc::unbounded_channel` feeding a single persistence worker. The worker:

1. Receives the event from the channel.
2. Applies it to the in-memory store under the existing `Mutex` and
   snapshots the post-application state in the same critical section.
3. Hands the snapshot to `spawn_blocking` for atomic disk write under
   the `fs2` file lock.

Events now process strictly in arrival order. The `fs2` lock is retained
as belt-and-suspenders for the case where the worker itself dies and a
replacement process is started mid-write.

**API change**: `wire_persistence` now returns the channel sender. The
single production caller (`aleph-server` start) was updated to `let _ =`;
tests in `wire_persistence_tests` (new `#[cfg(test)] mod`) drive events
through the returned sender.

**New tests** (`src/acp/manager/persistence.rs::wire_persistence_tests`):
- `burst_of_creates_no_event_lost` — 200 parallel Created events from N
  tasks, every `(harness_id, cwd)` must appear on disk exactly once.
- `interleaved_create_remove_no_cancelled_create` — 100 Created+Removed
  pairs, final store must be empty (no Removed lost to a stale snapshot).
- `repeated_created_preserves_created_at` — idempotent Created re-emit
  must preserve `created_at` (the original semantics, not a regression).

### 3. God-struct decomposition (commits `2dc259762`, `c227d9d0e`)

Decomposed three god structs into role-grouped sub-configs. No public
API change: constructors and `with_*` builders keep their signatures;
callers pass the same arguments; internal call sites read
`self.<group>.<field>` instead of `self.<flat_field>`.

**`AgentRuntimeConfig`** (9 fields → 3 sub-configs):
- `AgentIdentity` (agent_def, task, context_summary)
- `SpawnOverride` (spawn_context, fork_source, model, request_id)
- `Lifecycle` (timeout_secs)

**`AgentRuntime`** (27 fields → 7 sub-configs + 4 identity fields):
- `ProviderRouting` (provider, provider_overrides)
- `MemoryCapture` (raw_memory_writer, capture_registry, parent_agent_id, parent_session_id)
- `BackgroundConfig` (subagent_semaphore, plugin_registry)
- `PolicyInheritance` (guardrails, stall_config, consecutive_failure_cap, turn_timeout, strategy, session_mode)
- `BudgetInheritance` (default_max_iterations, parallel_tool_concurrency, context_budget_config, context_budget_refiner, primary_context_window, cheap_summary_provider, verifier_chain)
- `RoutingExperience` (routing_store)
- `TraceContext` (trace_sink)
- Direct fields (the four identity-of-the-runtime): child_chain, cancel_token, session, parent_tools

**`SubagentTool`** (33 fields → 9 sub-configs):
- `ProviderRouting` (provider, provider_overrides)
- `AgentResolution` (agent_registry, teammate_manager, message_router, inbox, parent_agent_id, plugin_registry)
- `BackgroundConfig` (background_tracker, subagent_semaphore, parent_cancel)
- `MemoryCapture` (raw_memory_writer, capture_registry, parent_session_id)
- `ToolingContext` (session, parent_tools, chain)
- `PolicyInheritance` (guardrails, strategy, session_mode, stall_config, consecutive_failure_cap, turn_timeout)
- `BudgetInheritance` (default_max_iterations, parallel_tool_concurrency, context_budget_config, context_budget_refiner, primary_context_window, cheap_summary_provider, verifier_chain)
- `RoutingExperience` (routing_store)
- `TraceContext` (trace_sink)

The sub-configs use `pub(super)` visibility — they're internal to the
`agents` module and don't leak outside. Each sub-config gets a
one-line `///` doc describing its responsibility.

### Rebase-cleanup commit (dropped automatically)

The god-struct subagent hit its turn limit and left two commits
containing genuine refactors alongside 22 `#[non_exhaustive]` deletions
and a 356-line revert of `src/acp/manager/persistence.rs` — all rebase
artifacts from picking up `c39c1c136` and `897497c13` (which post-date
the subagent's working tree).

A `ca9a5d05b` "restore #[non_exhaustive] + wire_persistence" commit was
added on the recovery branch; git's interactive rebase then dropped it
automatically when rebasing onto the now-current main, because the
restored content was already present upstream. Final tree: clean,
2 god-struct commits on top of `897497c13`.

## Verification

```bash
cargo check -p alephcore --lib        # 2m50s, 0 errors
cargo check --bin aleph-server        # 12.82s incremental, 0 errors
cargo check --workspace --bins --lib  # 3m06s, 0 errors
```

Pre-existing warnings (not introduced by these commits):
- `src/mcp/manager/handle.rs:417`: method `is_running` never used
- `src/pii/engine.rs:14`: constant `BUILTIN_RULE_COUNT` never used
- `src/verification/tool_loop_verifier.rs:86`: method `tier2_count` never used

Pre-existing test errors (out of scope, not touched):
- `src/builtin_tools/media_tools/understand.rs:248`, `src/media/processors/image.rs:155`, `src/media/mod.rs:75`, `src/vision/mod.rs:160,394` — `OcrResult` missing `lines` field (5 sites)
- `src/verification/extension_stop_gate.rs:412,443` — `veto_count` method missing (2 sites)

## State of negative (explicit non-actions)

- **Not** adding `#[non_exhaustive]` to enums outside the 23 listed — the survey found no others matching the criterion.
- **Not** refactoring `AgentRuntime::run` / `SubagentTool::execute_loop` logic — pure data decomposition only; behavior is byte-identical.
- **Not** changing `wire_persistence` API beyond returning the sender — the function name, signature shape (just appends a return), and hook contract are preserved.
- **Not** running `cargo test` on the new `wire_persistence_tests` — the unified `cargo check --workspace --tests` would surface them, but `--tests` mode also surfaces the 9 pre-existing test errors that are unrelated and block the run.
- **Not** running `cargo clippy --workspace --all-targets` — 11GB available, OOM risk; only `cargo check` was used per AGENTS.md memory guidance.
- **Not** deleting `ca9a5d05b` manually — git's interactive rebase dropped it automatically when the upstream already contained the restored content.