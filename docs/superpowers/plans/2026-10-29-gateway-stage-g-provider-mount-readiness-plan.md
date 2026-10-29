# Stage G Provider-Neutral Production Mount Readiness Plan
# Stage G Provider-Neutral Production Mount 就绪计划

- Date / 日期: 2026-10-29
- Base / 基线: actual `main` after Stage F merge; verify HEAD at session start
- Suggested branch / 建议分支: `stage-g-provider-mount`
- Scope / 范围: provider-neutral production mount readiness, with a gated minimal `ZahirFacade` mount only if existing authorities are sufficient

## Intent / 意图

Stage E proved the Extension-owned Extism instance boundary. Stage F proved that provider state, generation, admission, canonical publication and exact guest-instance ownership are exercised through one existing Extension lifecycle path. The next architectural question is whether that already-proven provider boundary can be mounted into a production provider-neutral capability host without introducing another registry, owner map, lifecycle, or cleanup authority.

Stage E 证明了 Extension-owned Extism instance boundary。Stage F 证明 provider state、generation、admission、canonical publication 与 exact guest-instance ownership 由同一条现有 Extension lifecycle 路径约束。下一阶段的架构问题是：能否在不新增 registry、owner map、lifecycle 或 cleanup authority 的情况下，把已经验证的 provider boundary 挂载到 production provider-neutral capability host。

The ultimate product direction is a lightweight Capability Core with on-demand, stoppable and replaceable providers for Gateway channels, Terminal, Memory Graph and Sandbox. This stage deliberately prepares the shared host boundary before wiring any concrete provider.

终极产品方向是轻量 Capability Core，加上 Gateway channel、Terminal、Memory Graph、Sandbox 等可按需启动、可停止、可替换的 provider。本阶段先准备共享 host boundary，刻意不接入具体 provider。

## Current baseline / 当前基线

- `main` contains the Stage F merge. The next session must inspect actual HEAD rather than trust this document’s hashes.
- `ZahirFacade` is the existing composition of the canonical `ToolHandlerRegistry` adapter and `Arc<OwnershipTree>`; it does not own a second descriptor, handler, owner or generation map.
- `ZahirFacade::new` currently has no production mount. Existing construction sites are tests and projection-host evidence.
- `ExtensionManager` / `plugin_state.rs` own durable plugin discovery, enabled/disabled state and mount/unmount lifecycle.
- `ProviderHandle` / `ProviderLifecycle` own provider state, per-handle generation, call admission and transition notification.
- `RuntimeOwner` owns the provider/runtime composition, owner generation, identity, receipt, admission and disposer.
- Stage F leaves production Zahir mount, concrete channel integration, universal provider-kind rollout, native unloading, OS mappings, threads and RSS claims deferred.

## Goals / 目标

1. Locate the real production owners of `ToolHandlerRegistry`, `OwnershipTree`, provider lifecycle and capability projection.
2. Decide whether an existing host composition point can mount the existing `ZahirFacade` without changing authority ownership.
3. Write a provider-neutral design and behavior RED before implementation.
4. If the readiness gate passes, implement the smallest mount seam that delegates all state, generation, admission, publication and disposal decisions to existing authorities.
5. Prove cold/mounted/activation/replacement/failure behavior with a real fixture while keeping the scope provider-neutral.
6. If the gate fails, produce a precise NO-GO architecture decision request instead of inventing a production owner or wiring a concrete channel.

## Non-goals / 非目标

- No Feishu, R2, Terminal, Memory, Sandbox or built-in channel wiring.
- No universal provider registry, provider-kind rollout or channel migration.
- No new global loader/cache, runtime, process isolation, native unloading or OS unload claim.
- No second `OwnershipTree`, owner map, generation map, descriptor catalog or cleanup authority.
- No production facade mount if the existing owner/lifecycle contract is insufficient.
- No claim that a facade projection, ProviderState, generation or notification proves an Extism instance drop.

## Architecture gate / 架构闸门

The gate is the first deliverable, before production code changes.

闸门是第一个交付物，必须先于 production code 修改。

### Pass criteria / 通过条件

The gate passes only if all of the following are true:

1. A production `ToolHandlerRegistry` source already exists and can be borrowed or shared by the facade.
2. A production `OwnershipTree` owner already exists, or its creation is explicitly part of the same existing lifecycle authority rather than a new parallel owner.
3. Mount/unmount can be attached to an existing lifecycle boundary whose stop/replacement/dispose ordering is already authoritative.
4. The facade can use existing provider admission and canonical publication without duplicating generation, owner, descriptor or cleanup state.
5. The dependency direction remains compatible with the repository constitution.
6. The resulting behavior can be tested without wiring a concrete channel.

### Fail criteria / 失败条件

The gate is NO-GO if any of the following is required:

- creating a second production `OwnershipTree` with independent ownership semantics;
- adding a new registry or global provider catalog;
- introducing a facade-owned lifecycle state machine or generation counter;
- changing public plugin lifecycle APIs solely to make the mount testable;
- adding a detached cleanup/recovery authority;
- making the Gateway or a concrete channel the canonical owner of all capabilities without an approved architecture decision.

A NO-GO result is a valid Stage G outcome. It must record the missing owner, the conflicting authority, the smallest architectural choices, and the decision required from the user.

## Approaches considered / 已考虑方案

### A. Mount the existing facade at an existing host point — recommended / 方案 A：在现有 host 点挂载 facade — 推荐

Reuse the production canonical registry and existing ownership source, then expose the existing `ZahirFacade` from the host that already owns their lifecycle. The facade remains a projection/composition surface; it does not become a new lifecycle authority.

Trade-off: this may require tracing several existing construction and shutdown paths before the mount point is obvious. It preserves the desired single-source architecture and is the only approach that should be attempted first.

### B. Add a thin provider-neutral host wrapper / 方案 B：增加薄的 provider-neutral host wrapper

Add a small wrapper only if the current host has no named composition point, with references to existing registry/tree/provider authorities and no independent state machine, generation, owner map or disposer. The wrapper may coordinate exposure, but it must delegate lifecycle transitions and cleanup to the existing owner.

Trade-off: even a thin wrapper can accidentally become a second lifecycle authority. It requires explicit source-level proof and a focused review. Use only if Approach A cannot express the existing composition cleanly.

### C. Wire a concrete channel or add a new provider registry — rejected / 方案 C：接入具体 channel 或新增 provider registry — 拒绝

This would provide a visible production consumer but would decide the shared Core shape from one channel and expand scope into Feishu/R2 or other provider integration. It violates the current stage boundary and is deferred.

## Execution steps / 执行步骤

### Task 1 — Verify baseline and census / 任务 1：验证基线并做 ownership census

- Verify actual `main`, clean worktree, merge commit and origin relation.
- Create `stage-g-provider-mount` or another short branch after the census starts.
- Read the required reference documents and source files.
- Trace production creation, retention, publication, replacement, stop and disposal for `ToolHandlerRegistry`, `OwnershipTree`, `ZahirFacade`, `ProviderHandle` and `RuntimeOwner`.
- Record the exact production host candidate and dependency direction.
- If no valid host exists, stop at the architecture gate and write the NO-GO decision request.

### Task 2 — Write the bounded design and RED / 任务 2：写 bounded design 与 RED

Before production implementation, write a bilingual design document under `docs/superpowers/specs/` covering intent, goals, non-goals, approaches, gate result, authority ownership and failure behavior. Obtain approval if the design introduces a new host wrapper or production mount.

Then add behavior RED against the real composition, covering:

- cold/unmounted list, descriptor and projection: zero guest-byte reads and zero runtime/instance creation;
- provider-neutral mount: facade sees the existing canonical source and does not create a second registry/tree/generation state;
- activation through existing ExtensionManager: payload A executes through the same provider admission as the canonical handler;
- replacement: old facade/provider/binding identity fails closed, payload B executes, exact receipts remain tied to real `LoadedWasmPlugin` incarnations;
- failure: failed read/build/publication/dispose keeps real residuals and does not report clean mount or unload.

### Task 3 — Implement only the approved minimal seam / 任务 3：只实现已批准的最小 seam

- Prefer Approach A and existing test-visible composition.
- If a wrapper is unavoidable, make it provider-neutral, thin and delegating; do not give it independent state, generation, owner or disposer authority.
- Keep production comments in English and new documentation bilingual.
- Remove temporary seams that are not required by the final evidence.
- Do not add concrete channel adapters or production integrations.

### Task 4 — Verify lifecycle and regressions / 任务 4：验证 lifecycle 与回归

Every Cargo command must be preceded by:

```bash
python qa/browser_live/check_memory.py
```

The gate must exit 0 and print `ALLOW`.

Run the focused provider-neutral mount tests three times serially, then the relevant WASM, Stage E/F, B3, capability and gateway narrow regressions, followed by `cargo check`, clippy, fmt and diff checks. A zero-test filter is no evidence.

Record separately: manifest read, guest-byte read, instance create/drop, facade mount state, ProviderState, provider generation, RuntimeOwner owner generation, canonical publication, stale binding result, OS mapping, thread and RSS limits.

### Task 5 — Independent review and documentation / 任务 5：独立审查与文档

Submit the complete diff and logs to an independent read-only reviewer. The review must check:

- single-source registry/tree/provider authority;
- no second lifecycle, owner, generation or cleanup authority;
- mount/unmount ordering and retained Arc release;
- stale replacement and failure residuals;
- exact Extism drop versus OS/JIT/thread/RSS claims;
- no concrete channel or production integration expansion.

Fix Critical/Important findings before closure. Only after GREEN and review should the session update the bilingual run record, `FEATURE_LOCATOR.md` and reference documents. If the gate is NO-GO, document the decision request instead of adding an entry claiming production mount.

## Acceptance criteria / 验收标准

Stage G is GREEN only if:

- actual production ownership and dependency direction are source-backed;
- the architecture gate passes without a second authority, or the NO-GO result is explicitly documented;
- provider-neutral facade mount, if implemented, delegates state/generation/admission/publication/disposal to existing authorities;
- cold, activation, replacement, stale-call and failure behavior are tested with real lifecycle composition;
- exact Extism receipt/drop evidence remains separate from ProviderState and OS/native unload claims;
- no concrete channel or unrelated provider integration is connected;
- focused tests, narrow regressions, static checks, diff check and memory gates are recorded;
- reviewer findings are resolved or the stage is explicitly left NO-GO with the required architecture decision.

## Expected deliverables / 预期交付物

- Next-session prompt: `docs/superpowers/prompts/2026-10-29-gateway-stage-g-provider-mount-prompt.md`
- This plan: `docs/superpowers/plans/2026-10-29-gateway-stage-g-provider-mount-readiness-plan.md`
- If implementation is approved: bilingual design, tests, implementation diff and run record.
- If readiness is NO-GO: bilingual census and architecture decision request, with no production mount claim.
