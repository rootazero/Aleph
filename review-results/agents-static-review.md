# agents/ — Static Review (2026-09)

Source branch: `review/agents` (forked from `main`)
Scope: `src/agents/` (15 root files + 6 subdirs, 22 488 LoC, no swarm/tasks/store/tests + subagent_tool/tests + subagent_spawner/tests counted).

## Headline counts

| File                                | P0 | P1 | P2 | Notes |
|-------------------------------------|----|----|----|-------|
| mod.rs                              | 0  | 0  | 1  | god-re-export layer |
| loader.rs                           | 0  | 0  | 1  | stale comment fixed |
| registry.rs                         | 0  | 1  | 2  | builtins, plugin global |
| runtime.rs                          | 0  | 2  | 1  | public type missing Debug, god struct |
| types.rs                            | 0  | 0  | 3  | public fields, non_exhaustive gap |
| background_tracker.rs               | 0  | 1  | 2  | god struct, non_exhaustive on 3 enums |
| background_persistence.rs           | 0  | 0  | 2  | orphan recovery paths |
| forwarding_trace_sink.rs            | 0  | 0  | 0  | clean |
| subagent_spawner/                   | 0  | 0  | 2  | SpawnerBase god struct, fork.rs |
| subagent_tool/                      | 0  | 0  | 2  | SubagentTool god struct, parse rigor |
| run_context.rs                      | 0  | 0  | 0  | clean (task_local, best-effort) |
| progress.rs                         | 0  | 1  | 1  | Serialize-only public type |
| teammates.rs                        | 0  | 0  | 0  | clean |
| thinking.rs                         | 0  | 0  | 1  | alias table duplication |
| tool_sets.rs                        | 0  | 0  | 0  | clean |
| allowlist_tool_service.rs           | 0  | 0  | 0  | clean, well-documented |
| subagent_tree_events.rs             | 0  | 0  | 0  | clean, fire-and-forget |
| swarm/                              | 0  | 0  | 3  | non_exhaustive gap, AsyncMutex held across ? |
| **total**                           | **0** | **5** | **21** | |

## Findings fixed in this PR

#### [P2] stale-doc-comment-with-dead-reference
- File: src/agents/loader.rs:139
- Rule: documentation rot (R10-adjacent)
- Context: Reserved-id guard comment for `builtin_primary_ids` referred to `with_mode` builder that no longer exists. Actual mode coercion is `AgentDef::new(&fm.id, AgentMode::SubAgent)` directly below.
- Before: `// forced to AgentMode::SubAgent (see with_mode below)`
- After: `// forced to AgentMode::SubAgent via AgentDef::new(&fm.id, AgentMode::SubAgent) on the def initializer just below.`
- Why: future reader following the pointer hits a non-existent method and wastes time.

#### [P1] public-type-missing-Debug
- File: src/agents/runtime.rs:44
- Rule: public API (rust-doctor §API)
- Context: `AgentRuntimeConfig` is the public input type to `AgentRuntime::run`. All fields are simple (`String`, `Option<String>`, `Option<ForkSource>`, `AgentDef`, `u64`); `ForkSource` already derives `Debug` (`subagent_spawner/fork.rs:58`); `AgentDef` derives `Debug` (`types.rs:204`). The struct itself has no `Debug`.
- Before: `pub struct AgentRuntimeConfig { ... }`
- After: `#[derive(Debug)] pub struct AgentRuntimeConfig { ... }`
- Why: callers cannot `tracing::debug!` or format errors when a config is rejected; panic messages lose structure.

## Findings reported but NOT fixed (deferred / out-of-scope)

#### [P1] public-enum-missing-non_exhaustive — registry
- File: src/agents/types.rs:10 (`AgentSource`), :26 (`AgentMode`), :48 (`IsolationMode`), :73 (`McpServerSpec`), :85 (`ContextMode`); src/agents/loader.rs:14 (`LoaderError`); src/agents/runtime.rs:79 (`TranscriptOutcome`); src/agents/background_tracker.rs:264 (`CompletedOutcome`), :311 (`WaitOutcome`), :325 (`WaitAnyOutcome`); src/agents/swarm/tasks/mod.rs:31 (`Priority`), :82 (`CoordTaskStatus`), :371 (`TaskRunStatus`), :431 (`ReviewerKind`), :443 (`ReviewVerdict`); src/agents/swarm/tasks/retry.rs:169 (`RetryDecision`).
- Rule: public lib API (R10-adjacent)
- Reason for deferral: same as the aggregate's "shared::58-enum non_exhaustive gap" — adding `#[non_exhaustive]` breaks every exhaustive `match` across `alephcore`, `aleph-cli`, `aleph-tui`, `aleph-panel`, `shared-ui-logic`. Crate-spanning migration requires a dedicated change. **Defer.**

#### [P1] SubagentTranscript public but no consumers
- File: src/agents/runtime.rs:90; pub re-export at mod.rs:43.
- Rule: dead public API (R10)
- Context: code comment "B1-05 (R10): CUT transcript persistence" at runtime.rs:499-506 already notes the file-persistence side was removed. The struct itself is still publicly re-exported and exposes `TranscriptOutcome` (`pub enum` with `Success`/`Error(String)`/`Timeout` variants — same `non_exhaustive` concern as above).
- Reason for deferral: needs to be combined with the non_exhaustive work and a deprecation cycle. **Defer** — file-persistence cut was already done; this is the residual cleanup.

#### [P1] SubagentProgress Serialize-only
- File: src/agents/progress.rs:12.
- Rule: API design (rust-doctor)
- Context: `SubagentProgress` is `#[derive(Debug, Clone, serde::Serialize)]` only; no `Deserialize`. `loop_tool.rs:1784` serializes `Vec<SubagentProgress>` into `check_status` output via `json!`. Any IPC consumer that round-trips the field cannot.
- Reason for deferral: would also need `Deserialize` on `SubagentProgress.preview`/`timestamp` (SystemTime needs a `serde` feature) and on the nested `CompletedOutcome` to be useful. Touches the same surface as the non_exhaustive migration. **Defer.**

#### [P2] god-struct — AgentRuntime
- File: src/agents/runtime.rs:114 (24 fields).
- Rule: rust-doctor architecture
- Context: every `with_*` builder threads one field; `execute_via_harness` (runtime.rs:588-630) clones ~14 fields into `SpawnerBase`. Adding a new parent-side knob means three edits.
- Reason for deferral: refactor of `AgentRuntime` would cascade through every spawn path. **Defer** as documented tech debt.

#### [P2] god-struct — SubagentTool
- File: src/agents/subagent_tool/mod.rs:71 (25 fields).
- Same profile as `AgentRuntime`. **Defer.**

#### [P2] runtime.rs:455 silent u64::MAX fallthrough
- File: src/agents/runtime.rs:455: `let duration_ms = start.elapsed().as_millis().try_into().unwrap_or(u64::MAX);`
- A duration larger than ~584M years saturating to `u64::MAX` is technically fine but hides clock anomalies; an `i64::try_into` check would surface real bugs sooner.
- Reason for deferral: harmless overflow path. **Defer.**

#### [P2] registry: process-global plugin map
- File: src/agents/registry.rs:18 `static PLUGIN_SUBAGENTS: OnceLock<RwLock<Arc<[AgentDef]>>>`.
- `publish_plugin_subagents` is a process-wide replace; the per-registry view of "available agent ids" depends on call order. Tests rely on this being global (registry.rs:836 test publishes, then another test reads). The current implementation is correct but tightly couples tests.
- Reason for deferral: working as designed (documented); no behaviour bug. **Defer.**

#### [P2] subagent_semaphore_for locks global Mutex during retain
- File: src/agents/subagent_tool/types.rs:68 (`subagent_semaphore_for`).
- Holds the `SESSION_SEMAPHORES` `std::sync::Mutex` across an O(n) `HashMap::retain` while inserting. Under a steady stream of new sessions the lock is briefly contended. Synchronous code so not blocking any `await`, but the lock is the only point where a brand-new session pays the full sweep.
- Reason for deferral: synchronous; contention is O(n) of dead entries (small in practice). **Defer.**

#### [P2] swarm: AsyncMutex held across `.await`-adjacent DB I/O
- File: src/agents/swarm/tasks/store/runs.rs (and friends). Every method does `let conn = store.conn.lock().await;` then `rusqlite` sync calls. The Mutex guard's lifetime is the function body; release at the end is correct. Pattern is consistent.
- Reason: not a bug. **Defer** as observation.

#### [P2] thinking.rs alias table duplication
- File: src/agents/thinking.rs + src/agents/registry.rs `normalize_agent_alias` (registry.rs:329).
- Two similar normalization tables; `normalize_think_level` and `normalize_agent_alias` exist side by side. Not redundant (different axes) but worth noting if either needs a third surface.
- Reason: working as designed. **Defer.**

#### [P2] AgentDef all-public fields, no builder-only invariant guard
- File: src/agents/types.rs:206.
- All fields `pub`; mutation after construction is allowed and the `allowed_tools_explicit` flag is mutated by builders only by convention.
- Reason: rust-doctor rule of thumb violated, but mutability is needed for the `with_*` builder pattern. **Defer.**

#### [P2] background_persistence: orphan-recovery semantic
- File: src/agents/background_persistence.rs (1243 LoC).
- Reads `data/transcripts/` via `std::fs::read_to_string` (sync) at lines 831, 1004. Called from async path (`recover_from_log` in subagent_tool/recovery.rs:700). Each call is one small file but they accumulate across `batch_tasks` fan-out.
- Reason: small reads; under load per-task. Could be moved to `spawn_blocking` if observed slow. **Defer** until measured.

#### [P2] AgentMode lacks Default
- File: src/agents/types.rs:25 (`AgentMode`).
- `AgentSource` derives `Default` (Builtin) but `AgentMode` does not. `AgentDef::new` always sets mode explicitly, so callers can't accidentally forget — no bug, just asymmetry.
- Reason: defensive choice. **Defer.**

## State of negative (explicit non-actions)

- **Did not** add `#[non_exhaustive]` to the 17 public enums across this module — would break exhaustive match sites across `alephcore`, `aleph-cli`, `aleph-tui`, `aleph-panel`, `shared-ui-logic`. Defers to the cross-crate migration planned for the shared::58-enum gap (aggregate.md §"Cross-module findings").
- **Did not** refactor `AgentRuntime` or `SubagentTool` (24/25 fields) into smaller role-grouped structs — blast radius covers every spawn path; current shape is documented and works.
- **Did not** touch the `process-global` `PLUGIN_SUBAGENTS` `OnceLock<RwLock<...>>` (registry.rs:18) — the per-process plugin state is by design (see module doc); per-registry injection would change every caller that hits `plugin_subagents()` from a context with no agent registry handle.
- **Did not** run `cargo check` or `cargo build` (explicitly forbidden at this stage; unified check runs after all modules).
- **Did not** audit `subagent_tool/tests.rs`, `subagent_spawner/tests.rs`, `subagent_spawner/fork/tests.rs`, `background_tracker.rs` tests, `subagent_tool/recovery.rs` tests, `swarm/tasks/store/tests.rs`, `swarm/tasks/dag.rs` tests, or `loader.rs` tests — every `.unwrap()`/`.expect()` reported in the initial sweep is `#[cfg(test)]` and uses well-named invariants.
- **Did not** touch `mod.rs` (re-exports only; R10 surface — leaving the public API stable is the goal).
- **Did not** remove `SubagentProgress.timestamp: SystemTime` — making it `Serialize+Deserialize` would require the `serde` `chrono`/`time` feature and is a separate API decision.