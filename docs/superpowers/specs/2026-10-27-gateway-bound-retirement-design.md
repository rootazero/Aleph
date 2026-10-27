# Gateway Bound Channel Retirement, Replacement, and Unregister

**日期 / Date:** 2026-10-27
**状态 / Status:** Approved; implemented and verified for the bounded Generic Webhook/loopback composition
**范围 / Scope:** Stage B of Gateway source-owned runtime authority

## Intent / 意图

本阶段要闭合 Bound channel 在首次启动之后的生命周期边界：retirement、replacement、unregister，以及 descriptor/schema/lifecycle contract 的 publication change。目标是让 Gateway 对旧 work、旧 completion、incumbent、successor 和 source publication revision 的状态都保持可证明且可观察。

This stage closes the post-start lifecycle of a Bound channel: retirement, replacement, unregister, and descriptor/schema/lifecycle-contract publication changes. The goal is to keep old work, old completions, the incumbent, the successor, and the source publication revision truthful and externally observable.

本阶段只扩展既有 `ChannelRegistry`、entry-local runtime 和 `binding.rs`。`ChannelRegistry` 仍是唯一的 Gateway association/publication authority；`ProviderHandle` 仍只拥有 provider instance、generation 和 call admission/completion guard；`OwnershipTree` 仍只拥有 owner、lifetime、generation、revoke/dispose authority。

This stage extends only the existing `ChannelRegistry`, entry-local runtime, and `binding.rs`. `ChannelRegistry` remains the sole Gateway association/publication authority; `ProviderHandle` remains authoritative for provider instance, generation, and call admission/completion guards; `OwnershipTree` remains authoritative for owner, lifetime, generation, revoke, and dispose state.

## Goals / 目标

- Bound retirement must fence new work, drain admitted work, and wait for cleanup that is actually observable through the real loopback channel, forwarder, task, and resource state.
- A completion captured from an original entry must be checked in the order source → ownership tree → provider → original entry/ticket. It must never look up a successor by textual channel ID.
- A successor must be privately prepared and preflight-validated, then published as one complete tuple in a short source-gate commit.
- Replacement and unregister failures must retain the incumbent identity, operation marker, mount/dispatch fence, and any real cleanup residual.
- Source revision exhaustion must be checked before any destructive retirement, map mutation, or publication. No wrap and no half-publish are allowed.
- The six required contract tests must observe real barriers, loopback requests, entry/task/forwarder state, and publication boundaries.

## Non-goals / 非目标

本阶段不实现或授权：

- Feishu、R2、production Bound opt-in、production mount、Zahir route 或其他子系统迁移。
- Public send/ingress API、EffectClaim producer、Safe Replay、external exactly-once、universal durable scheduler、durable/crash recovery。
- 新 registry、adapter、facade glue、owner map、handler/descriptor map、generalized envelope、`CapabilityKind` 或 `src/harness/**` 修改。
- 用 test-only receipt 作为 production permission/ownership/completion authority，或用 `Drop` 推断 network operation 已完成。
- 将 Bound lifecycle 事实迁移到 `ProviderHandle` 或 `OwnershipTree`。

This stage does not implement or authorize Feishu/R2, production Bound opt-in or mount, Zahir routing, public send/ingress APIs, EffectClaim production, Safe Replay, external exactly-once, a universal durable scheduler, durable/crash recovery, a new registry/adapter/facade/owner map/descriptor map/envelope/CapabilityKind, `src/harness/**`, test-only receipts as production authority, `Drop`-based network completion inference, or moving Gateway lifecycle facts into `ProviderHandle` or `OwnershipTree`.

## Anti-goals / 反目标

- 不把“stop 调用了”当作 cleanup 已完成。
- 不把 empty/no-op cleanup 或 revision wrap 报告为成功。
- 不允许 successor 先发布、再补旧 entry 的清理。
- 不允许旧 completion 因为 textual ID 相同而获得 successor 的权限。
- 不允许 source gate 在 channel/factory/transport/stop/close/join I/O 期间保持锁定。
- 不允许复制 generation、owner map、descriptor fingerprint 或 runtime registry 作为第二份事实。

## Constraints / 约束

- 所有真实 I/O、transport close、forwarder stop、task join 和 failure injection 都在 source gate 之外执行。
- retirement marker 必须覆盖 unlocked drain/cleanup phase，并在最后一次成功 publication 或失败 residual commit 后释放。
- publication 只能提交完整 successor tuple：entry identity、provider instance/generation、descriptor/fingerprint、contract、ownership binding、desired intent 和 source revision。
- 旧 completion 在 successor publication 后必须得到 `DiscardedStale`，且 original ticket 只能结算一次。
- revision exhaustion 的 preflight 必须位于任何 destructive retirement 之前；cleanup 阶段发生的 revision 消耗必须触发 final revalidation，而不能绕过 checked allocation。
- 所有 failure path 必须保留可观察 residual，并使 API 返回 failure；不能返回“成功但没有效果”。

## Approaches considered / 方案比较

### 1. Extend the existing Bound runtime — selected / 扩展现有 Bound runtime——选定

在现有 `EntryRuntime`/`binding.rs` 增加 retirement phase、successor preparation、drain/join residual 和 source-owned publication marker；在 `ChannelRegistry` 增加最小的 replacement/unregister 编排与完整 tuple commit。现有 map、`ProviderHandle` 和 `OwnershipTree` 继续分别持有自己的事实。

This keeps lifecycle state close to the Bound entry, preserves the existing authority split, and makes the source-gate boundary local and reviewable. Its cost is that Bound lifecycle phases become more explicit in the existing entry-local runtime, but that is the fact that Gateway already owns.

### 2. Put all orchestration in `ChannelRegistry` / 全部编排放入 `ChannelRegistry`

Registry 负责 provider/ownership cleanup 的全部细节，而 `binding.rs` 只提供 primitives。该方案会让 source authority 重新了解 provider 和 ownership 内部事实，扩大跨文件线性化协议，并提高半发布风险，因此不选。

This would make the registry depend on provider and ownership details, broaden the cross-file linearization protocol, and increase half-publish risk. It is rejected.

### 3. Make `OwnershipTree` or provider lifecycle own retirement / 让 tree 或 provider 生命周期拥有 retirement

该方案能减少 Gateway 代码，但会把 entry identity、source revision、forwarder/mount cleanup 等 Gateway 事实推入错误 authority，破坏现有分工，因此不选。

This would move Gateway facts such as entry identity, source revision, and forwarder/mount cleanup into the wrong authority. It is rejected.

## Lifecycle contract / 生命周期契约

### Preparation and preflight / 准备与预检

1. Read the incumbent under the existing source map and require a Bound entry with an operation marker that permits the requested operation.
2. Before changing the incumbent, check that the required source publication revision(s) can be allocated without wrap or exhaustion.
3. Prepare the successor privately: construct or validate the provider handle, descriptor/fingerprint, contract, ownership binding, desired intent, and entry-local runtime. Preparation must not change the visible source map or publication revision.
4. Validate provider/tree/entry identity and all cross-authority binding proofs before entering the source gate.
5. If any preflight fails, return failure with the incumbent untouched.

### Fence, drain, and cleanup / 闸、排空与清理

1. In one short source-gate commit, mark the incumbent as retiring and fence new admission/dispatch. The marker is the durable in-memory operation boundary for the unlocked phase.
2. Outside the gate, stop new dispatch, wait for admitted work to reach the real drain boundary, stop the forwarder/driver, close the transport, and join relevant tasks. Tests must observe a real loopback request and barrier.
3. If cleanup succeeds, retain the retirement marker until final revalidation. If cleanup blocks or fails, retain the incumbent, marker, fence, and exact residual (for example a still-running task, forwarder, transport, or join failure); return failure and do not publish a successor.
4. A cleanup operation is complete only when the actual observed task/forwarder/resource state proves it. Calling stop/close is insufficient.

### Final revalidation and publication / 最终复核与发布

1. Re-enter the source gate and revalidate the original incumbent identity, retirement marker, source revision, provider/tree binding, and cleanup result.
2. Allocate the final checked source revision only if it is still available. If another entry consumed the revision during cleanup, fail closed while retaining the incumbent and residual state; do not remove or replace it.
3. For replacement, commit the complete successor tuple atomically: new entry identity, provider instance/generation, descriptor/fingerprint, contract, ownership binding, desired intent, and source revision. No intermediate tuple is observable.
4. For unregister, remove the incumbent only in the same final successful commit after cleanup is proven. A failed cleanup never reports successful removal.
5. Release the marker only after the final successful publication/removal commit or after the failure residual commit.

## Completion verdict matrix / Completion 判定矩阵

| Situation | Required verdict | State mutation allowed |
|---|---|---|
| Source publication no longer matches original capture | `DiscardedStale` | Original ticket settles once; no successor/source mutation |
| Ownership binding revoked/disposed or generation changed | `DiscardedStale` | No provider, owner, or successor mutation |
| Provider generation/instance changed | `DiscardedStale` | No provider or entry mutation |
| Original entry identity/retirement marker no longer matches | `DiscardedStale` | No textual-ID successor lookup; no successor mutation |
| All source → tree → provider → original-entry checks pass before retirement | Existing completion verdict | Only original entry/ticket may be updated |
| Cleanup or final revision allocation fails | Operation failure with residual | Incumbent, marker, fence, and residual remain observable |

The order is mandatory: source → ownership tree → provider → original entry/ticket. A successor lookup by textual ID cannot replace the original-entry check.

## Revision exhaustion / Revision 耗尽

Revision availability is checked before the initial retirement fence and again before the final publication/removal commit. The implementation must never use an unchecked `expect`, wrap the revision counter, delete the incumbent, or publish only part of the successor tuple. If another entry consumes the available revision during unlocked cleanup, final revalidation fails closed and leaves the incumbent plus residual/marker state intact.

## Authority ownership / Authority 分工

| Fact | Sole authority | Stage B responsibility |
|---|---|---|
| Channel ID association, visible incumbent/successor tuple, source revision, operation marker, retirement phase, residual | `ChannelRegistry` entry/map | Add only entry-local retirement and atomic publication state |
| Provider instance, provider generation, call admission/completion guard | `ProviderHandle` | Reuse existing guard; do not copy generation |
| Owner/lifetime/generation/revoke/dispose and binding validation | `OwnershipTree` | Reuse existing APIs such as current-binding validation; do not add owner map |
| Descriptor canonical fingerprint | Existing descriptor/canonicalization path | Store/use the published tuple value; do not create a descriptor catalog |
| Actual transport/forwarder/task completion | Existing Bound runtime/channel driver | Observe real state and retain residual on uncertainty/failure |

## Test-first acceptance criteria / 先写 RED 的验收标准

Before production behavior changes, add the following tests in the real Gateway authority test modules. First correct parser/import/insertion/format errors; only a contract failure counts as RED.

1. `bound_retirement_discards_original_completion`
   - A request reaches the real loopback peer.
   - Retirement/replacement occurs before completion handoff.
   - The old completion returns `DiscardedStale`; the original ticket settles exactly once.
   - The successor is unchanged.

2. `bound_replace_publishes_whole_tuple`
   - Provider, descriptor, contract, ownership, and successor entry are privately prepared.
   - Before publication the old entry is the only visible incumbent.
   - After publication the source revision, entry identity, provider instance, and descriptor fingerprint change together.
   - No partial tuple is observable at the publication boundary.

3. `bound_unregister_waits_for_cleanup`
   - Real outbound work holds a response barrier.
   - Unregister waits for the original work, forwarder, and observable resource cleanup.
   - Cleanup not yet confirmed cannot remove the incumbent or report success.

4. `bound_contract_update_retires_old_publication`
   - A descriptor/schema/fingerprint or lifecycle-contract change invalidates the old capture/lease.
   - The same textual ID cannot make the old provider/entry proof acceptable.

5. `bound_failed_cleanup_retains_incumbent`
   - A real stop/close/join failure is injected at the existing test-support boundary.
   - The incumbent, operation marker, residual, and fence remain; no successor publishes.
   - The operation does not report clean success or automatic `Ready`.

6. `bound_revision_exhaustion_never_half_publishes`
   - Source revision exhaustion is induced before destructive retirement, including a revision consumed by another entry during cleanup.
   - No teardown, deletion, successor publication, or revision wrap occurs after failure.

All tests must inspect actual entry/task/forwarder state and publication boundaries. No test-only receipt may grant permission or ownership.

## Risks and limits / 风险与限制

- This stage proves process-local lifecycle and publication behavior only. It does not prove clustered ownership, crash recovery, durable replay, or external exactly-once effects.
- Cleanup failure injection must correspond to a real stop/close/join boundary; an artificial boolean that bypasses the runtime is insufficient.
- Existing Legacy compatibility wrappers may require a narrow Bound branch so Bound retirement does not enter a Legacy success path. That change must preserve Legacy semantics and remain covered by the same source gate.
- R1 remains independently NO-GO/deferred. Stage B completion does not authorize Feishu/R2 or production mount.

## Required evidence updates / 必须更新的证据

After implementation and verification, update the Stage B plan/spec/run record in bilingual form with each actual command, exit status, test count, failure count, and nonfatal warning. Reconcile `docs/reference/FEATURE_LOCATOR.md` and the relevant reference documents with the final bounded contract, authority split, evidence, and remaining deferred work. Do not record unrun checks as passed.

## Approval gate / 审批闸门

## Approval gate / 审批闸门

The design was approved before implementation. The historical gate was honored: the six RED tests were written first, then the smallest source-owned implementation was added. The final implementation and command evidence are recorded in [`docs/superpowers/runs/2026-10-27-gateway-bound-retirement-run.md`](../runs/2026-10-27-gateway-bound-retirement-run.md).

本设计已在实现前获得批准。历史审批闸门已遵守：先写入六个 RED 测试，再添加最小 source-owned 实现。最终实现与命令证据记录于 [`docs/superpowers/runs/2026-10-27-gateway-bound-retirement-run.md`](../runs/2026-10-27-gateway-bound-retirement-run.md)。

## Final implementation status / 最终实施状态

Implemented in `src/gateway/channel_registry.rs`, `src/gateway/channel_registry/binding.rs`, and `src/gateway/channel_registry/test_support.rs`. `EntryRuntime.retiring` fences admission and completion; replacement and unregister use private preparation, checked preflight, unlocked drain/join/cleanup, final revalidation, and one short publication/removal commit. `complete_bound_work` validates source identity, then ownership tree, provider generation, and the original entry/ticket. The six required tests are GREEN, and the final evidence records registry 59/59, binding 3/3, webhook 51/51, capability 222/222, check, clippy, fmt, and diff results.

实现位于 `src/gateway/channel_registry.rs`、`src/gateway/channel_registry/binding.rs` 与 `src/gateway/channel_registry/test_support.rs`。`EntryRuntime.retiring` fence admission 与 completion；replacement/unregister 使用 private preparation、checked preflight、锁外 drain/join/cleanup、final revalidation 和一次短 publication/removal commit。`complete_bound_work` 按 source identity、ownership tree、provider generation、original entry/ticket 顺序校验。六个必需测试均 GREEN；最终证据记录 registry 59/59、binding 3/3、webhook 51/51、capability 222/222、check、clippy、fmt 和 diff 结果。

Independent review was attempted but unavailable because the ACP proxy circuit was open; this is recorded as unavailable and not as a passed review. Manual review covered half-publication, source-gate/I/O discipline, stale completion noninterference, revision exhaustion, residual truth, and authority duplication. R1 remains NO-GO/deferred, and Feishu/R2, production mount/Zahir route, public send/ingress, EffectClaim, Safe Replay, durable recovery, universal scheduler, and remaining provider kinds remain deferred.

独立 review 已尝试但因 ACP proxy circuit open 不可用；文档将其记录为 unavailable，不计为通过。手工审查覆盖 half-publication、source-gate/I/O 锁纪律、stale completion 不干扰 successor、revision exhaustion、residual 真实性和 authority duplication。R1 仍为 NO-GO/deferred；Feishu/R2、production mount/Zahir route、public send/ingress、EffectClaim、Safe Replay、durable recovery、universal scheduler 及其余 provider kind 仍 deferred。
