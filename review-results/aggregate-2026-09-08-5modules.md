# Aggregate Review — 5 Modules (2026-09-08)

**Source commits**: 5 branches forked from `070120341` (main pre-review HEAD)
**Review agents**: 5 (parallel, in isolated worktrees)
**Worktrees**: `.worktrees/review-{a2a,acp,agents,approval,artifacts}/`
**Per-module reports**: `review-results/{mod}-static-review.md` in main

## Headline counts

| Module | P0 | P1 fixed/total | P2 | Hard redline |
|--------|----|----|----|--------------|
| src/a2a | 1 | 5/5 | 10 | — |
| src/acp | 0 | 2/2 | 5 | — |
| src/agents | 0 | 2/5 (3 cross-crate deferred) | 21 | R10 surface |
| src/approval | **1** | 1/4 (3 cross-crate deferred) | 2 | R10 + default-deny |
| src/artifacts | 0 | 1/1 | 3 | — |
| **total** | **2** | **11/17** | **41** | **0 hard** |

## Cross-module findings (deferred or contained)

1. **approval::compile_rules_grouped fail-open P0** — `src/approval/config.rs:148` previously skipped rules whose glob patterns failed to compile. For a default-Allow action family (BrowserNavigate, BrowserClick, …) a typo'd blocklist rule silently fell through to `Allow`. **Fixed in this PR**: blocklist compile failures now insert a synthetic `(?s).*` match-anything rule (fail-closed), while allowlist failures keep their warn-and-skip behavior (fail-open is acceptable: defaults remain in force). Covered by two new tests in `approval-static-review.md`.

2. **a2a::A2AClient::new panic-vs-default P0** — `src/a2a/adapter/client/http_client.rs:64` previously fell back to `reqwest::Client::new()` on builder error, silently dropping the no-redirect security policy. **Fixed in this PR**: panic with documented invariant — misconfiguration is process-startup, not runtime.

3. **agents::17-public-enum non_exhaustive gap** — All public enums in `src/agents/{types,loader,runtime,background_tracker,swarm}/` lack `#[non_exhaustive]`. Same shape as the aggregate's "shared::58-enum non_exhaustive gap". **Defer** — needs a coordinated crate-spanning migration across `alephcore`, `aleph-cli`, `aleph-tui`, `aleph-panel`, `shared-ui-logic`. Tracked under "Cross-module findings" in the pre-existing aggregate.md.

4. **approval::AcpOperationError public fields** — same non-exhaustive gap. Tracked under item 3.

## Fix plan (worktree order — what actually shipped)

### worktree: `review/a2a` (commit `bc357cb35`)
- **P0** `adapter/client/http_client.rs:61-69` — panic on reqwest builder failure rather than silently dropping the no-redirect security policy
- **P1** `adapter/server/bridge.rs` — 5 sites: log `update_status` / `broadcast_status` / `cleanup_task` failures instead of swallowing with `let _ = ...await`
- **P1** `adapter/server/request_processor.rs:312-326` — log cancel-event `broadcast_status` failure

### worktree: `review/acp` (commit `402c423e6`)
- **P1** `incoming.rs:505` — replace `.expect()` with `.unwrap_or_default()` in `apply_line_window`; function is documented as "never panics" and the empty-string fallback matches 0-line-file behavior
- **P1** `manager/harness_admin.rs` — extract `kill_and_emit_removed` helper, eliminating 3× duplicated (kill-loop + emit-loop) blocks across `unregister_harness` / `update_harness` / `disable` paths; lock-ordering invariant preserved

### worktree: `review/agents` (commit `626f989b7`, aborted at turn limit; partial fixes committed by main agent)
- **P1** `runtime.rs:44` — `#[derive(Debug)]` on `AgentRuntimeConfig` (all fields already impl Debug)
- **P1** `loader.rs:139` — fix stale comment that referenced a `with_mode` builder method which no longer exists
- 3 P1 deferred (non_exhaustive migration, god-struct refactor, dead `SubagentTranscript` re-export) — see `agents-static-review.md` §"State of negative"

### worktree: `review/approval` (commit `aa98b4dd2`)
- **P0** `config.rs:148` — `compile_rules_grouped` is now fail-closed for blocklist, fail-open for allowlist. New tests `a_blocklist_rule_that_fails_to_compile_denies_all_matching_actions` and `an_allowlist_rule_that_fails_to_compile_is_silently_skipped` cover both halves.
- 3 P1 deferred (public-enum non_exhaustive, public-fields, async-trait on dyn-Trait-eligible surface) — see `approval-static-review.md`

### worktree: `review/artifacts` (commit `7ba0d83c4`)
- **P1** `store.rs:302-313` — `evict_overflow`: log per-record removal failures (was `let _ = fs::remove_file(...).await`). Matches the function's doc contract ("logged, never surfaced").

## Post-merge verification

```bash
CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1 cargo check --workspace
```
(memory-limited: ~11GB available; full check OOM-killed in the past)

## State of negative (explicit non-actions)

- **Not** running per-module `cargo check` (forbidden at subagent stage; unified check runs once after all merges).
- **Not** adding `#[non_exhaustive]` to 17 public enums across agents/approval — deferred to cross-crate migration (see `aggregate.md` §"Cross-module findings #1").
- **Not** refactoring `AgentRuntime` (25 fields) or `SubagentTool` — blast radius covers every spawn path; documented and works.
- **Not** running `cargo test` — subagents were instructed to skip; tests for approval's P0 fix will surface during unified check.
- **Not** addressing the documented stale-snapshot race in `acp/manager/persistence.rs::wire_persistence` — the file-lock stops the rename race but not the temporal ordering; code's own comment acknowledges; out of scope.
- **Not** refactoring `request`/`request_streaming` duplication in `acp/transport.rs` — branches on different notification-handling semantics; extracting would couple more than it saves.
- **Not** removing the redundant `start_kill` in `acp/session.rs::Drop` — `kill_on_drop(true)` defers to runtime shutdown ordering; explicit `start_kill` runs synchronously when `AcpSession` goes out of scope (earlier than runtime shutdown). Kept intentionally.
- **Not** adding graph-fetch path sanitization to `artifacts/store.rs` — out of scope for this pass; needs separate audit (deferred).
- **Not** running `cargo clippy --workspace --all-targets` (would OOM at 11GB available).