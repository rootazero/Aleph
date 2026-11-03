# Memory source-owned boundary design / Memory 源所有边界设计

- Date / 日期: 2026-11-03
- Status / 状态: Approved; bounded implementation and verification complete, final independent review found no findings; local delivery pending / 已批准；有界实施与验证完成，最终独立审阅无具体问题；本地交付待完成
- Scope / 范围: Derived Memory reconciler authority, derived compression fallback, callback admission, and FlushRegistry-to-derived-compression boundary

## Intent / 意图

The current `MemoryRuntime` already owns one derived SQLite backend, its activation bundle, retained Compression/Dream/Producer/Reconciler workers, startup operations, and the NoteVaultWatcher. The remaining lifecycle question is narrower: which production paths can still create or continue derived work without that owner, and how should raw archival remain independent while derived work becomes fail-closed at the runtime boundary?

当前 `MemoryRuntime` 已经拥有一套派生 SQLite backend、activation bundle、Compression/Dream/Producer/Reconciler 保留 worker、启动 operation 和 NoteVaultWatcher。本设计解决一个更窄的问题：哪些生产路径仍能在没有该 owner 的情况下创建或继续派生工作，以及如何在保持 raw archive 独立的同时，让 derived work 在 runtime 边界上 fail-closed。

## Current evidence / 当前证据

The source census classified the relevant production shapes as follows:

| Shape / 形状 | Current owner / 当前 owner | Admission / 准入 | Retention and stop / 保留与停止 | Decision / 决定 |
|---|---|---|---|---|
| Derived backend SQL | `src/memory/runtime/bundle.rs:27` creates `SqliteMemoryBackend::new(...).with_runtime(owner)` | `src/memory/store/sqlite/mod.rs:221-232` obtains `self.operation()` before `spawn_blocking` | Bundle drops services, then closes the backend; runtime operations are drained | Keep under `MemoryRuntime`; add effect evidence where a callback or fallback reaches it |
| Owned daemons | `src/memory/runtime/bundle.rs:153-159` retains Compression/Dream/Producer/Reconciler handles | Each tick obtains `runtime.operation()` and scopes the work | `MemoryTasks` retains handles; bundle stop receiver ends future ticks and admitted work drains | Keep the existing owner; do not add a second supervisor |
| NoteVaultWatcher | `src/memory/runtime/bundle.rs:45-58`; watcher receives `Weak<MemoryRuntime>` | Consumer scopes work when the runtime is alive; native stop helper is separate | `shutdown()` joins native stop and consumer handles | Keep as a source-owned worker; test the optional-runtime fallback boundary separately |
| MemoryReflector callback | `src/memory/runtime/services.rs:195-212` captures `Arc<SqliteMemoryBackend>` and calls `record_signals` | Backend operation is acquired when the callback writes | Callback is held by the runtime-owned service bundle | Keep the callback, but pin fail-closed behavior after stop with a real effect test |
| Compression fallback | `src/memory/compression/service.rs:405-413,710-748` calls `tokio::spawn(task)` when no operation exists | No runtime admission in the fallback branch | JoinHandle is not retained by `MemoryTasks` | Runtime-bound services must reject this branch; standalone compatibility may remain explicit and isolated |
| Production reconciler | Runtime-owned daemon at `src/memory/events/handler.rs:819-848`; server-owned daemon at `:864-930`, called from `src/bin/aleph-server/commands/start/mod.rs:2003-2034` | Runtime variant admits each scan; server variant has no runtime admission | Runtime variant is retained; server variant is only abortable through a server handle | Make the runtime variant the sole production authority; remove the server-owned production start |
| Flush coordination | Process-global `OnceLock<FlushRegistry>` in `src/memory/flush/mod.rs:21-64` | Guard is acquired before fire-and-forget flush; derived writes currently rely indirectly on backend operation | Registry tracks raw-to-derived flush readiness, not worker ownership | Keep it as raw-session coordination; ensure the derived compression it invokes is runtime-bound and fail-closed |
| Raw archive/session persistence | `RawMemoryStore`, `MarkdownSessionStore`, `session_resume::writer`, and `tool_signal_sink` | Raw callers have their own admission/ownership | Continues while Memory is Off | Preserve independent ownership; do not route it through `MemoryRuntime` |
| Pure CPU/test/documentation sites | `std::thread::scope` graph algorithms, test-only `tokio::spawn`, and comments in census targets | No persistent Memory write or runtime worker | No lifecycle ownership needed | Exclude from production migration |

The census also found that `src/memory/dreaming/mod.rs:877,884` and `src/memory/extensions/scheduler.rs:41` public unowned APIs have no non-test production caller. They remain available for standalone compatibility and tests, but the production bundle uses `start_owned_background_task` and `spawn_owned`.

The most concrete unresolved production defect is the duplicate reconciler path. Runtime services construct a `MemoryCommandHandler` with the runtime-created `NoteIndexer` at `src/memory/runtime/services.rs:265-266`, and the bundle retains its owned daemon at `src/memory/runtime/bundle.rs:153-159`. Separately, startup constructs another handler and calls the public unowned `spawn_reconciler_daemon` at `src/bin/aleph-server/commands/start/mod.rs:2003-2034`. Both can scan the same `StateDatabase` and notes tree, while only one is part of the Memory lifecycle.

## Goals / 目标

- Make the runtime-owned reconciler the only production reconciler daemon.
- Make the Admin API observe the current runtime bundle rather than retain a stale handler across runtime restart.
- Ensure runtime-bound derived compression cannot silently fall back to an unowned `tokio::spawn`.
- Prove that the `MemoryReflector` callback and FlushRegistry-triggered derived work respect runtime admission and fail closed after stop.
- Preserve raw chat, raw tool signals, Markdown archival, and session resume persistence while Memory is Off.
- Record every affected production owner, admission boundary, retained handle, and deterministic test in the census.

## Non-goals / 非目标

- No universal Memory writer stop claim.
- No shutdown of raw archive or chat while derived Memory is Off.
- No new registry, capability kind, generation authority, provider proxy, or universal supervisor.
- No native unload, RSS reclamation, process isolation, or external-effect exactly-once claim.
- No migration of pure CPU stages, test-only tasks, synchronous session resume writes, or raw `MarkdownSessionStore` ownership into `MemoryRuntime`.
- No Feishu, Channel, Terminal, MCP lazy, EffectClaim producer, or Safe Replay expansion.
- No automatic replay or repair of reconciler divergences.

## Anti-goals / 反目标

- Do not leave two production reconciler loops merely because both can report through a similarly named handler.
- Do not treat a backend `Arc` capture as proof that a callback is admitted; the effect must be observed after the runtime transitions to Draining or Off.
- Do not replace the existing source-owned runtime with a process-global task hub.
- Do not make a raw session-end archive wait for derived compression to succeed.

## Approaches considered / 已考虑方案

### Census-only documentation / 仅 census 文档

This would document every owner and gap without changing production behavior. It has the lowest regression risk, but leaves the duplicate reconciler and unowned compression fallback active. It is insufficient after the source scan identified a concrete production owner conflict.

### Source-owned boundary closure / 按 source owner 收紧边界 — chosen / 已选择

Keep raw/session/archive ownership independent, make the runtime bundle the sole production derived owner, and close only the four derived boundaries identified by the census: reconciler authority, compression fallback, reflector callback admission, and FlushRegistry-to-derived compression. Retain legacy public APIs only where they have standalone/test callers and explicitly do not use them in production.

This keeps the current architecture legible: the runtime owns derived work, raw stores own archival work, and the existing backend operation remains the admission point. It requires precise tests for restart and stop behavior, but does not introduce a new authority.

### Universal Memory supervisor / 统一 Memory supervisor

This would make one process-level controller own raw storage, derived storage, flush coordination, and every background task. It conflicts with the established Off semantics and would make raw archival unavailable when derived Memory is stopped. It is rejected by the capability and lifecycle boundaries.

## Chosen design / 选定设计

### 1. One production reconciler authority / 唯一生产 reconciler authority

The runtime-owned `MemoryCommandHandler` and its `NoteIndexer` become the only production reconciler pair.

The implementation should expose the current bundle’s reconciler through a narrow `MemoryRuntime` accessor or a runtime-backed Admin API view. `AdminApiState` should hold the `Arc<MemoryRuntime>` needed to resolve the current handler per request, rather than storing an `Arc<MemoryCommandHandler>` captured during boot. `/reconciler/latest` then returns the current bundle’s report when Memory is Running and returns the existing service-unavailable response when the runtime is Off, unavailable, or configured without a reconciler.

Startup must remove the direct construction and call to `spawn_reconciler_daemon` at `src/bin/aleph-server/commands/start/mod.rs:2003-2034`. The runtime bundle continues to retain `spawn_owned_reconciler_daemon`. The public `spawn_reconciler_daemon` API may remain for standalone callers and tests, but a static source guard must ensure it has no production startup call site.

Resolving the handler per request is required because a runtime restart creates a fresh derived bundle and old derived handles do not survive restart. The Admin API must not retain the previous generation’s handler or report. The current handler can remain inside `MemoryServices`; the accessor must not create a second handler or second indexer.

The endpoint contract is explicit:

- `Running` with `memory.reconciler.enabled = true` and no completed scan returns `204 No Content`.
- `Running` with the reconciler enabled and a completed report returns `200 OK` with that current-generation report.
- `Off`, `Starting`, `Draining`, and `Failed` return `503 Service Unavailable`.
- A disabled reconciler returns `503 Service Unavailable`, even if the state database exists.
- After a runtime restart, the previous generation’s report is unavailable until the new generation completes a scan.

### 2. Runtime-bound compression fallback / runtime-bound compression fallback

`CompressionService` needs an explicit runtime-bound mode used by `MemoryServices::initialize`. In that mode, the `Ok(None)` branches at `src/memory/compression/service.rs:405-413` and `:710-748` must log and return without spawning an unowned derived task. The standalone constructor used by focused compression tests may retain its compatibility behavior, but production runtime construction must opt into the fail-closed mode.

The distinction must be explicit at the constructor or service boundary, not inferred from whether a `Weak<MemoryRuntime>` happens to upgrade. A runtime-bound service with Memory Off and a standalone service without a runtime have different contracts and must not share an ambiguous fallback.

The already admitted operation still completes its work after stop according to the existing drain semantics. The change only prevents a new derived task from escaping admission after the operation is unavailable.

### 3. Callback admission / callback 准入

Keep `MemoryReflector` owned by `MemoryServices` and keep its callback backed by the same `SqliteMemoryBackend`. Add an effect test that starts the runtime, invokes the callback through the real reflector path, stops the runtime, then attempts another callback write through a deterministic channel/Notify handshake. The post-stop write must fail closed and must not create a new recall-signal row.

The callback must not capture a strong `MemoryRuntime`; the backend’s existing weak runtime link and `operation()` remain the admission path. Dropping the service bundle during stop must release the callback and provider capture after admitted work drains.

### 4. FlushRegistry boundary / FlushRegistry 边界

Keep `FlushRegistry` process-global because it coordinates raw session-end readiness and is not itself a Memory worker. Keep synchronous guard acquisition before spawning, as documented in `src/memory/flush/mod.rs:21-64`.

`MemoryOperation::spawn_session_end` remains responsible for admitting the session-end work. It may continue to archive raw data while derived compression is unavailable. The compression supplied to `flush_agent_memory` must be the current runtime-bound service. When the runtime is Off, the flush may observe the guard and complete its raw-side bookkeeping, but derived compression must fail closed rather than create an unowned task. `await_ready` remains a bounded readiness hint for the next session; it is not a durable scheduler or a second ownership source.

Add deterministic tests for these effects:

- raw session-end archival succeeds while derived compression is unavailable;
- a queued flush cannot create a new derived task after runtime stop;
- a running admitted flush is allowed to finish according to the existing operation drain contract;
- `await_ready` observes the guard lifecycle without using timing sleeps as the proof.

## Acceptance evidence / 验收证据

The implementation plan must produce all of the following:

1. A source census table listing every production row in the four affected categories with file/function, owner, admission, retention, stop behavior, and test.
2. A static boot guard proving startup no longer constructs or starts the public unowned reconciler.
3. A real Admin API effect test proving the endpoint reads the runtime-owned handler and becomes unavailable after runtime stop/restart as specified.
4. A real runtime-bound compression test proving no unowned task is spawned when operation acquisition returns unavailable.
5. A real MemoryReflector callback test proving post-stop writes fail closed.
6. Real raw/derived flush tests using actual store/runtime objects and Notify/channel handshakes, with no global mocks or sleep races.
7. Existing standalone/test callers of public unowned APIs classified and preserved only where their contract is explicit.
8. Documentation updates to `docs/reference/MEMORY_SYSTEM.md`, the relevant lifecycle plan/run records, and `FEATURE_LOCATOR.md` only after implementation evidence exists.

Every Cargo command must first run `python qa/browser_live/check_memory.py` and require `ALLOW` with at least `4294967296` available bytes, then use `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0` and select a positive number of tests.

## Remaining builder decisions / Builder 可决定事项

The builder may choose the narrowest internal accessor shape for resolving the current reconciler from `MemoryRuntime`, the exact error type used by runtime-bound compression fallback, and the names of the deterministic test helpers. These choices must preserve one runtime owner, one backend admission path, and the source/effect distinctions above.

The builder must not decide to migrate raw storage, add a universal supervisor, keep two production reconcilers, or broaden the claim to universal Memory stop. Those are outside this design.

## Implementation handoff / 实施交接

This document is a design gate. After user approval, the next step is to invoke the writing-plans workflow and create a separate bounded implementation plan. Until that approval, do not modify production Rust, add tests, install dependencies, or mark the old 11-task lifecycle plan complete.
