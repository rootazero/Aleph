# rust-doctor static review — `src/a2a`, `src/acp`, `src/agents`, `src/approval`, `src/artifacts`

**Date**: 2026-10-29
**Branch**: `main` (fast-forwarded from the isolated worktree branch `rust-doctor-modules`)
**Worktree used for development**: `.worktrees/rust-doctor-modules` (branch `rust-doctor-modules`, based on `origin/main` @ `04fba2229`)
**Base commit**: `04fba2229`

## Method

| Stage | Tool / actor | Output |
|---|---|---|
| Machine scan | `rust-doctor 0.9.0` (whole workspace, `cargo clippy --workspace --no-deps`) | 195 diagnostics inside the five modules (28 actionable, 167 structure/maintainability) |
| Semantic map | `graphify-out/graph.json` (139k nodes) — per-module subgraph: files, cross-module in/out edges, hub symbols | `.review/graph-map.md` (working copy), fed to every reviewer |
| Review | 9 parallel `reviewer` subagents (`minimax-cn/MiniMax-M3.1-Flash`, thinking=high/medium), one per module or per module slice | 126 findings (P0 1 · P1 30 · P2 79 · P3 16) |
| Triage | **Jev** (`jev-latest`) — four calibrated `noul` batches over the 87 P0/P1 + curated P2/P3 candidates | fix-now / verify-then-fix / defer split |
| Fix | 10 parallel `worker` subagents (`minimax-cn/MiniMax-M3.1-Flash`), file-disjoint per module | 8 commits on `main` |
| Verification | one unified `cargo check -p alephcore --all-targets` (`CARGO_BUILD_JOBS=2`, `lld` linker) | see below |

Each module was reviewed **without a diff** (whole-module static review), and each reviewer was given the
September-2026 prior reports (`review-results/{a2a,acp,agents,approval,artifacts}-static-review.md`,
`review-results/agents-batch-1..6/`) so that already-known findings were re-verified rather than
re-reported.

### Which steps used Jev

Jev was used for **classification, filtering and routing only** — never for writing code:

1. **Classification** of every P0/P1 candidate (26 items) into "fix in this commit / needs design review /
   defer / reject as false positive".
2. **Filtering** of 24 curated P2/P3 quick wins (silent error discards, unbounded reads, panics, doc drift)
   against the same criterion.
3. **Filtering** of the 24 `src/agents/swarm` findings (task logic + SQLite store).
4. **Routing** of the accepted findings into per-module fix briefs (file-disjoint batches), including the
   "verify-then-fix" band, so a worker only implements an item after proving the defect is real.

Jev was deliberately *not* asked to judge correctness of Rust semantics — that stayed with the
reasoning models.

## Findings and disposition

| Module | Findings | Fixed in this pass | Deferred |
|---|---|---|---|
| `src/a2a` | 24 (P0 1, P1 9, P2 14) | 12 | 12 |
| `src/acp` | 18 (P1 4, P2 11, P3 3) | 7 | 11 |
| `src/agents` (runtime + subagent tool) | 27 (P1 6, P2 15, P3 6) | 9 | 18 |
| `src/agents/swarm` (task logic + store) | 24 (P1 5, P2 17, P3 2) | 14 | 10 |
| `src/approval` | 15 (P1 5, P2 10) | 10 | 5 |
| `src/artifacts` | 18 (P1 1, P2 12, P3 5) | 5 | 13 |
| **total** | **126** | **57** | **69** |

## What was fixed

### `src/a2a`

- **Loopback trust behind a reverse proxy (P0)** — a request carrying a forwarding header is no longer
  classified as `Local`, because the peer address is then the proxy's, not the caller's
  (`adapter/auth/tiered.rs`).
- **Local bypass is off by default** (`config.rs`) — the `Local` tier carries administrative permissions,
  so an unauthenticated peer must not reach it out of the box. **Operator-visible**: a deployment that
  relies on the unauthenticated local path now has to set `[a2a.server.security] local_bypass = true`.
- A failed post-execution `update_status` no longer skips the terminal event, and a panicking run now
  leaves the task out of `Working` (`adapter/server/bridge.rs`).
- `MAX_TASKS` is enforced even when every task is still in flight; the push-notification config map is
  bounded and rejects a config for an unknown task id (`adapter/server/task_store.rs`,
  `service/notification.rs`).
- A stream that ends without a terminal event is an error instead of a "completed" remote task; a
  malformed SSE frame is logged instead of dropped silently (`adapter/client/sse_stream.rs`).
- The push-notification path now has a `with_ssrf_policy` constructor and warns loudly that the operator
  webhook SSRF policy is not applied on that path yet (`service/notification.rs`).
- The default `claim_task` no longer maps a store failure to "another run is in flight"
  (`port/task_manager.rs`); SSE frame serialization no longer degrades to an empty frame
  (`adapter/server/routes.rs`).

### `src/acp`

- The permission guard now inspects the tool arguments **where the ACP spec puts them**
  (`toolCall.rawInput`), so a command or an out-of-workspace path can no longer be smuggled past a
  read-scoped permission request (`incoming.rs`).
- The timeout path no longer discards agent→client requests while draining the event channel; they are
  answered through the normal dispatch path (`transport.rs`).
- The NDJSON frame write is handed to a task that owns the stdin lock, so a cancelled caller cannot leave
  a half-written frame in the child's stdin (`transport.rs`).
- The oneshot harness capture is bounded, like the ACP fs payloads, so a runaway harness can no longer
  exhaust the server's memory (`adapters/mod.rs`).
- The generic adapter passes the configured environment and working directory to the harness
  (`adapters/generic.rs`); the persistence hook logs a failed enqueue instead of discarding it
  (`manager/persistence.rs`); the current-directory fallback is consistent and can no longer leave the
  workspace root empty (`incoming.rs`).

### `src/agents`

- The subagent tool parser rejects wrong-typed scalars and non-string request ids instead of silently
  defaulting or dropping them (`subagent_tool/parse.rs`).
- The `default`-agent fallback uses the same spawnability predicate as the explicit `agent_type` path
  (`subagent_tool/loop_tool.rs`); a child that ends without final text is no longer recorded as a
  completed run with an empty body (`subagent_spawner/mod.rs`).
- Registering a duplicate agent id warns (and respects the source precedence) instead of silently
  replacing the earlier definition (`registry.rs`).
- The recovered-orphan summary is bounded in run count and size before it is injected into a parent
  prompt; `record_settled` proves index membership before writing (`background_persistence.rs`).
- The `READ_ONLY` / `INVESTIGATION` / `ASYNC_SAFE` tool-set documentation now states the truth: the
  write side of the bundled `file_ops` tool is **not** gated by `denied_tools` (`tool_sets.rs`).
- Swarm: acceptance criteria are bounded and can no longer break out of their list item; the operator
  retry clears a pending backoff; the grounding gate fails closed when the store errors; the
  `CoordTaskStore` read-side defaults no longer hide a missing backend; the store keeps FK enforcement
  on after a failed migration, bounds its history reads, keeps `created_at` as the journal insert time
  and orders runs deterministically.

### `src/approval`

- The cluster-node approval leg can no longer mint a session-scoped grant (`node_requester.rs`).
- The grounding guard no longer treats a projected summary as if the judge had seen the whole action
  (`guardian_requester.rs`).
- An `Ask` decision is audited, and the audit truncation is char-boundary safe instead of panicking on a
  multi-byte target (`policy.rs`, `config.rs`).
- The raw approval prompt is no longer logged verbatim at INFO (`mod.rs`).
- The final approval frame publish, a glob compile failure and a const-slice `expect` are no longer
  swallowed (`operator_requester.rs`, `config.rs`, `mod.rs`); the channel-adapter documentation now
  matches the `Unavailable` outcome it actually returns (`adapters.rs`).

### `src/artifacts`

- `put` keeps the record it just published out of its own eviction candidate set, so a newly written
  artifact can no longer be deleted by the same call (`store.rs`).
- Eviction no longer removes the blob when the sidecar removal failed; `read` rejects a blob whose
  length disagrees with the sidecar; the session-key encoding is injective again (the empty key no
  longer collides with `_`), matching the module documentation (`store.rs`, `mod.rs`).
- The temp-file cleanup failure is logged instead of discarded.

## Deferred (with the reason)

| Finding | Why deferred |
|---|---|
| `src/a2a`: the streaming fallback retries with the same task id | protocol/idempotency semantics — needs an A2A protocol decision, not a local patch |
| `src/a2a`: the operator webhook SSRF policy is not plumbed into the push path | the constructor exists and warns; wiring the operator policy into the service is a configuration-plumbing change |
| `src/a2a`: public enums lack `#[non_exhaustive]`, public fields on the domain types | semver-breaking for every downstream match in the workspace; a separate API pass |
| `src/acp`: `Command::output()` replaced by a bounded capture — done; JSON-RPC string ids, `lines()` UTF-8 fatality, unbounded NDJSON frame length | protocol/transport redesign beyond a local patch |
| `src/agents`: a failed child emits no durable terminal event | needs a new `SessionEvent` variant in `src/session/events.rs` (outside the reviewed modules) |
| `src/agents`: splitting the bundled `file_ops` tool into read/write | changes the builtin tool catalog and the builtin agent definitions; the documentation now states the gap |
| `src/agents/swarm`: metadata has no compare-and-set | needs a schema column plus ~37 call sites outside the module |
| `src/agents/swarm`: `list_tasks` needs a cursor | a `CoordTaskStore` trait change (the module owns neither the trait nor the dispatcher settle sweep that needs the complete set); the cap is 10k and the truncation warns loudly |
| `src/agents/swarm`: `coord_task_runs` / `coord_task_comments` are never pruned | a retention job is a separate change; only the reads are bounded now |
| `src/agents/swarm`: `TaskExitJournal::created_at` still means "last written" in the public type | the field lives in `swarm/tasks/mod.rs`, owned by another change in this pass |
| `src/approval`: `ActionRequest` / `CallIdentity` public fields | a public API change; the approval binding still needs its own pass |
| `src/approval`: the policy file is a startup snapshot, the guardian auto-approves a model-authored payload | design questions (LLM sovereignty, reload semantics) — not a local patch |
| `src/artifacts`: symlink-following session directory, `read` materialising the whole blob | both need a public API change (`read` returning a reader) |
| all modules: the `rust-doctor` structure findings (duplicate function bodies, oversized units, complex functions) | maintainability P3 noise; a refactor of the touched files belongs to its own change |

## Verification

```
CARGO_BUILD_JOBS=2 cargo check -p alephcore --all-targets
```

Result: **clean — 0 errors, 0 warnings** (10m12s first run; 8m46s after the two warning fixes below).
The check ran **once, after every module had been reviewed and every fix applied**, with `-j2` and the
repository's `lld` linker configuration to keep the link memory bounded (the machine has 19 GB RAM and
the `alephcore` check peaks at ~17 GB resident with two jobs).

No `cargo check` was run while the review was in flight (per the task instruction), so the fix workers
could not compile-verify their edits. The first check therefore surfaced two warnings introduced by the
pass — an unused import in `src/a2a/adapter/server/routes.rs` and an unused binding in
`src/a2a/port/task_manager.rs` — which were fixed and re-verified by a second (incremental) run.
The residual compile risk each worker reported was reviewed by hand; the check is the arbiter.

## State of the negative (what was NOT done)

- **No tests were run.** Only `cargo check` — the new behaviour has unit tests where the worker could add
  them cheaply (the artifacts eviction and size checks, the A2A forwarded-header trust, the ACP
  `rawInput` permission cases, the swarm acceptance bound), but the full test suite was out of scope.
- **No `cargo clippy` / `cargo fmt` pass** over the changed files after the fixes; the rustfmt style was
  matched by hand and one long `warn!` line per module may exceed the project's line length.
- **No other workspace crate was re-checked** beyond what `cargo check -p alephcore --all-targets`
  covers; the panel / CLI / SDK crates that consume the changed public types were not re-verified.
- **No behaviour change was made to the public API** of the five modules: no enum variant was added or
  removed, no public field was renamed, and no `#[non_exhaustive]` was added (that would break every
  downstream match in the workspace).
- **The operator-visible default change** in `src/a2a` (`local_bypass` now defaults to `false`) is the
  one change in this pass that alters runtime behaviour for an existing deployment; it is called out in
  the commit message and in `src/a2a/domain/security.rs`.
- **The pre-existing September-2026 findings** that this pass re-verified as still present but did not
  fix are listed above; the reports themselves were left untouched.
