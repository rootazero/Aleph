# R1 Source-Owned Provider Binding Contract Revision Plan

> 下一阶段只修订并验证 R1 provider/source binding contract。不得进入 R2 Feishu，不得把 Tool authority 复制成 Gateway authority，也不得在 authority 未闭合时实现生产 GREEN。

## Goal

在既有 authority 上回答 `ChannelRegistry::ChannelCapture` 如何与真实 provider instance/generation、descriptor publication、owner generation、admitted-work completion 和 resource receipts 建立同一份 source-owned proof。

本阶段的成功结果有两种：

1. 找到一个现有 provider/source owner，能够在不新增 registry、adapter、facade glue、owner map、handler map 或 durable store 的前提下闭合全部 contract，并实现最小 RED→GREEN；或
2. 用真实 authority 的 RED 证据证明首个 contract 仍未闭合，更新 ledger/spec/roadmap，保持 R1 **NO-GO / deferred**。

本阶段不得因为已有 Tool 测试通过而把 R1 改为 GO。

## Current evidence

- `src/gateway/channel_registry.rs:55` 的 `ChannelCapture` 当前保存 channel id、source publication revision 和注册 channel `Arc`。
- `src/capability/provider.rs:234` 的 `ProviderHandle` 是 provider lifecycle、instance identity、generation 和 call admission authority。
- `src/capability/ownership.rs:334` 的 `OwnershipTree::generation` 是独立 owner-generation authority。
- `src/tools/registry.rs` 与 `src/capability/zahir_facade.rs` 已有 Tool-specific descriptor publication、lease validation 和 late-completion rejection，但不能作为 Gateway channel 的第二份 authority。
- 已执行的 RED probe 证明 `ChannelCapture` 没有 `provider_handle` 或 `provider_generation`，且当前没有 Gateway admitted-work/completion owner 或真实 resource receipt contract。

## Hard constraints

- 不修改 `src/harness/**`。
- 不新增 `CapabilityKind`。
- 不新增 capability/source/provider/descriptor/callable/handler/owner/generation/disposer registry 或 durable store。
- 不创建 Feishu adapter、facade glue、resolve bypass、production mount 或 Feishu→Zahir route。
- 不泛化 `BackendLease::canonical_entry`。
- 不合并 provider generation、owner generation、source revision、`SessionEvent.seq`。
- 不把 live broadcast 当作 durable recovery source。
- 不实现 EffectClaim producer、Safe Replay、external exactly-once、universal scheduler 或 durable provider recovery。
- 不把 query `Scope` 当 disposer，也不让 shared provider 因单个 binding 被撤销而停止。
- 不把 test fixture counter 当成 production resource receipt。
- 先在真实 authority 的测试模块写 RED，再决定是否有合法 GREEN。

## Required reading

1. `AGENTS.md`
2. `docs/superpowers/plans/2026-10-26-r1-provider-binding-completion-plan.md`
3. `docs/superpowers/plans/2026-10-25-capability-core-remaining-roadmap.md`
4. `docs/superpowers/specs/2026-10-26-r1-provider-source-lease-design.md`
5. `docs/superpowers/specs/2026-10-25-capability-core-provider-contract-design.md`
6. `docs/superpowers/runs/2026-10-26-capability-core-r1-capture-ledger.md`
7. `src/gateway/channel_registry.rs`
8. `src/capability/provider.rs`
9. `src/capability/ownership.rs`
10. `src/capability/backend.rs`
11. `src/capability/zahir_facade.rs`
12. `src/tools/registry.rs`
13. `src/session/store.rs`

## Execution

### 1. Establish the authority map

Record exact owners and signatures for:

- source capture minting and validation;
- provider instance and generation;
- descriptor identity/schema/fingerprint/publication revision;
- owner generation and revoke/dispose state;
- admitted work and completion acceptance;
- cold/discovered/disabled resource allocation and teardown.

Use file and line references. Explicitly distinguish Tool-only authorities from any common Gateway authority.

### 2. Write RED tests first

Tests must live in the existing owner’s test module. The tests must probe:

- source capture remains read-only and does not initialize runtime resources;
- provider restart changes provider generation independently from channel source revision;
- owner generation bump/revoke/dispose rejects old owner observations independently;
- descriptor replacement changes descriptor publication without copying a second descriptor map;
- admitted work retains source-owned proof and rejects completion after replacement/unregister/revoke/dispose at the actual linearization boundary;
- cold/discovered/disabled describe/list/project/resolve paths produce zero real allocation receipts;
- explicit activation is the first positive allocation control.

If a candidate API is absent, preserve the exact compiler RED and remove the probe unless the API belongs to an existing authority and has an approved implementation path. Correct parser/test insertion failures before treating a failure as evidence.

### 3. Decide whether a legal GREEN exists

A GREEN implementation is allowed only when an existing authority already owns the relevant fact and can validate it without a second mutable map. Immutable observations or opaque source-owned proofs are acceptable only when validation returns to the original authority.

If no existing authority owns admitted work and completion acceptance for Gateway channels, stop. Do not implement a simulated completion fence in tests or in a new facade.

### 4. Review resource receipts

Prove receipts at the real allocation boundary, covering worker/task, thread, auth/token client, listener, connection, websocket, database, and cache categories. The proof must distinguish zero cold/disabled reads from explicit start. If this boundary cannot be instrumented without adding a forbidden registry or changing provider ownership, record the receipt contract as open.

### 5. Update the decision records

Update:

- `docs/superpowers/specs/2026-10-26-r1-provider-source-lease-design.md`
- `docs/superpowers/plans/2026-10-25-capability-core-remaining-roadmap.md`
- `docs/superpowers/runs/2026-10-26-capability-core-r1-capture-ledger.md`

The final decision must name the first unclosed contract and preserve R1 NO-GO/deferred unless all required evidence is source-backed.

## Verification

```bash
cargo fmt --all --check
git diff --check
cargo check -p alephcore --tests
cargo clippy -p alephcore --tests -- -D warnings
cargo test -p alephcore --lib capability::
cargo test -p alephcore --lib gateway::channel_registry::tests::
```

Run one focused command for every new test. Record actual counts and any existing linker warning. Do not claim full workspace or real provider QA unless run.

## Deliverables

- Authority map with exact source owners and signatures.
- RED evidence for each missing contract.
- Minimal source-owned GREEN only if an existing authority can own it.
- Updated R1 spec, roadmap, and ledger.
- Final report with changed files, validation results, duplication review, deferred items, and actions not performed.
- No commit, push, merge, worktree cleanup, or external provider action unless separately authorized.
