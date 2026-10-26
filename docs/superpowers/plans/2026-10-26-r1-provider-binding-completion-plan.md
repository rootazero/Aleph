# R1 Provider Binding / Completion Proof Implementation Plan

> 下一会话只执行本计划。R1 仍是唯一允许的 gate；R2 Feishu A/B 审计和任何 Feishu Rust wiring 必须保持 deferred。

## Architecture north star

`Everything externally composable is a Capability.` 一切需要被 Agent、Runtime 或其他系统动态发现、调用、替换、订阅或管理的能力，都是 Capability。Aleph 的目标是从“一个集成很多功能的进程”演进成“一个轻量 Capability Core，加上一组按需启动、可停止、可替换的能力提供者”。Gateway 下十几个社交媒体通道、Terminal、Memory/Nebula graph、Sandbox 都必须最终满足这个模型：后台关闭的 provider 不应在 Aleph 启动时把线程、连接、数据库、缓存、token client、listener 或 websocket 初始化到进程中。

## Goal

在现有 authority 上闭合 R1 的下一段 provider/source proof：把 `ChannelRegistry` 的 source-owned `ChannelCapture` 与实际 `ProviderHandle` instance/generation、descriptor publication identity、`OwnershipTree` owner generation 和 admitted-work completion acceptance 的候选边界对应起来，或者用证据明确保持 R1 NO-GO。目标是得到可审查的契约和最小 RED→GREEN 证明，不创建 universal lease 或第二份 authority。

## Current baseline

- `ChannelRegistry` 已有 crate-private `ChannelCapture`，捕获注册 channel 的 `Arc` identity 与 source publication revision，并通过 `Arc::ptr_eq` fail-closed 验证 replacement/unregister stale capture。
- `ProviderHandle` 仍是 provider lifecycle/state/instance/generation authority。
- `OwnershipTree` 仍是 visibility/owner generation/revoke/dispose authority。
- `CapabilityBackend` / `ToolHandlerRegistry` 仍是 Tool-only read and callable authority；Tool `BackendLease::canonical_entry` 不得泛化。
- R1 overall remains **NO-GO / deferred**. No Feishu descriptor, resolve route, production mount, or Zahir route exists.

## Hard constraints

- 不修改 `src/harness/**`。
- 不新增 `CapabilityKind`。
- 不新增 capability/source/provider/descriptor/callable/handler/owner/generation/disposer registry 或 durable store。
- 不创建 Feishu adapter、facade glue、resolve bypass、production mount 或 Zahir route。
- 不进入 R2；本计划完成后必须重新裁决 R1，不能自动继续。
- 不实现 EffectClaim producer、Safe Replay、external exactly-once、universal scheduler 或 durable provider recovery。
- 不把 provider generation、owner generation、source revision、`SessionEvent.seq` 合并为一个 counter。
- 不把 live broadcast 当作 recovery source。
- 不把测试 fixture 当成 production authority。
- 先写 failing test，观察真实 RED，再做最小 GREEN。

## Authority contract to close

The implementation/review must answer these questions with concrete existing owners and file paths:

1. Who mints the source capture and owns the captured channel instance? The answer must remain `ChannelRegistry`.
2. Where is the actual `ProviderHandle` instance/generation obtained and validated? Do not infer identity from a textual id or generation alone.
3. Which canonical source publication supplies descriptor identity/schema/fingerprint and source revision? Do not add a descriptor mirror.
4. How is `OwnershipTree` owner generation observed independently from provider/source revisions?
5. What object or source-owned token is retained by admitted work?
6. At what linearization boundary does the source owner reject a completion from a replaced/unregistered/revoked/disposed instance?
7. What resource receipts prove cold/discovered/disabled read paths do not initialize runtime resources?

If the existing authorities cannot answer these questions without a new dispatch registry or copied state, stop and record R1 NO-GO/deferred. Do not invent a generalized envelope to hide the gap.

## Task 1 — Reconfirm current contracts and write the ledger

Read:

- `src/gateway/channel_registry.rs`
- `src/capability/provider.rs`
- `src/capability/ownership.rs`
- `src/capability/backend.rs`
- `src/capability/zahir_facade.rs`
- `src/tools/registry.rs`
- `src/session/store.rs`

Update a new SDD ledger before code edits. Record exact owners, existing signatures, lock/linearization boundaries, and rejected alternatives. Preserve the four-domain independence table.

Acceptance:

- There is one named source owner and one named provider owner.
- The ledger states whether a concrete provider binding path exists.
- No implementation begins before the first unclosed contract is explicit.

## Task 2 — RED tests for provider/source binding

Add tests only where the existing authority lives. Prefer a real `ChannelRegistry` test module plus existing provider/ownership tests; do not add a fake Core registry.

Required behaviors:

- A capture remains read-only and does not start/authenticate/network.
- A replacement with the same textual channel id rejects the old source capture.
- A provider restart changes provider generation independently of channel source revision.
- An owner generation bump/revoke/dispose rejects the old owner observation independently of provider generation and source revision.
- A candidate accepted work item cannot publish a completion after source replacement or provider generation change.
- A failed stop/replacement retains the canonical incumbent and its existing runtime surface according to the current lifecycle contract.

Run each new focused test before implementing its missing contract. A compile failure for an absent API is useful RED; a parser error or malformed test insertion is not evidence and must be corrected first.

## Task 3 — Implement only the existing-authority proof

If the RED tests identify a missing production behavior, implement it at the owning authority:

- source identity/revision in `ChannelRegistry`;
- provider identity/generation in `ProviderHandle`;
- owner generation and tombstones in `OwnershipTree`;
- descriptor publication in its existing source;
- completion acceptance in the provider/source owner that owns admitted work.

Do not add a cross-authority mutable map. A lease/capture may carry immutable observations or an opaque source-owned proof, but validation must route back to the original authority. Do not make `CapabilityBackend` universal merely to avoid a source-specific boundary.

If no existing authority owns admitted work or completion acceptance, do not implement a simulated completion fence. Document the exact missing authority and leave R1 NO-GO/deferred.

## Task 4 — Resource receipt proof

Use deterministic test-only receipts at the real allocation/admission boundary. The fixture must prove:

- discovery, describe/list/project, cold resolve, and disabled reads produce zero worker/thread/task, token/auth client, listener, connection, websocket, database, and cache receipts;
- explicit activation is the first positive control;
- replacement/stop drains or rejects admitted work according to an owned contract;
- old completion cannot mutate the successor.

Do not use unchanged `ProviderState` as a substitute for receipts. Do not use sleeps or timing races. If no real allocation boundary can be instrumented without introducing a production registry, record the proof as open.

## Task 5 — Gate review and decision

Review the diff for authority duplication and verify that every claim is source-backed. Update:

- `docs/superpowers/specs/2026-10-26-r1-provider-source-lease-design.md`
- `docs/superpowers/plans/2026-10-25-capability-core-remaining-roadmap.md`
- this SDD ledger

R1 may become GO only if the implementer can describe a non-Tool provider with:

- source-owned capture;
- actual provider instance/generation binding;
- descriptor/source publication binding;
- independent owner generation;
- explicit use and completion acceptance;
- resource receipt proof;
- no parallel registry, owner map, handler map, or durable store.

Otherwise report **R1 NO-GO / deferred**, name the first unclosed contract, and make the next entry only the smallest contract revision needed to close it.

## Verification

Run from the repository root only for checks that are allowed there; run tests from `packages`/crate locations as project instructions require:

```bash
cargo fmt --all --check
git diff --check
cargo check -p alephcore --tests
cargo clippy -p alephcore --tests -- -D warnings
cargo test -p alephcore --lib capability::
cargo test -p alephcore --lib gateway::channel_registry::tests::
```

Add focused commands for every new test. Record exact pass/fail counts and the existing macOS linker warning if it appears. Do not claim full workspace validation unless it is actually run.

## Deliverables

- Updated R1 design and roadmap evidence.
- New SDD ledger with RED/GREEN commands and authority decisions.
- Focused tests or a precise NO-GO evidence record.
- Final report listing changed files, commands, deferred items, and destructive/external actions not performed.
- No commit/push is part of the next implementation session unless separately authorized.
