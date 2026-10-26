# Next Session Prompt — R1 Provider Binding Contract Revision

你是 Aleph Capability Core 的主实现 agent。请在当前仓库继续执行 R1 的下一阶段：**只修订并验证 source-owned provider binding contract**。不要进入 R2 Feishu。

仓库：`/Volumes/TBU4/Workspace/Aleph`

先阅读：

- `AGENTS.md`
- `docs/superpowers/plans/2026-10-26-r1-provider-binding-contract-revision-plan.md`
- `docs/superpowers/plans/2026-10-26-r1-provider-binding-completion-plan.md`
- `docs/superpowers/plans/2026-10-25-capability-core-remaining-roadmap.md`
- `docs/superpowers/specs/2026-10-26-r1-provider-source-lease-design.md`
- `docs/superpowers/specs/2026-10-25-capability-core-provider-contract-design.md`
- `docs/superpowers/runs/2026-10-26-capability-core-r1-capture-ledger.md`
- `src/gateway/channel_registry.rs`
- `src/capability/provider.rs`
- `src/capability/ownership.rs`
- `src/capability/backend.rs`
- `src/capability/zahir_facade.rs`
- `src/tools/registry.rs`
- `src/session/store.rs`

当前事实：

- `ChannelRegistry::ChannelCapture` 只保存 channel id、source publication revision 和 channel `Arc`，并能在 replacement/unregister 后 fail-closed。
- `ProviderHandle` 拥有 provider instance identity、lifecycle generation 和 call admission。
- `OwnershipTree` 拥有 owner generation、revoke、dispose。
- Tool 路径有 descriptor publication 和 late-completion validation，但不能复制成 Gateway authority。
- RED probe 已准确证明 `ChannelCapture` 没有 `provider_handle` / `provider_generation`，当前没有 Gateway admitted-work completion owner，也没有真实 resource receipt contract。
- R1 仍为 **NO-GO / deferred**。

本阶段必须回答：

1. `ChannelCapture` 如何与真实 `ProviderHandle` instance/generation 同代绑定？
2. descriptor identity/schema/fingerprint/source revision 从哪个既有 canonical source 派生？
3. `OwnershipTree` owner generation 如何作为独立 observation 参与 validation？
4. admitted work 保留什么 source-owned proof？completion 在哪个真实线性化边界被拒绝？
5. disabled/discovered/cold 的 describe/list/project/resolve 如何证明没有初始化线程、连接、数据库、缓存、token client、listener 或 websocket？

执行纪律：

- 先在真实 authority 的测试模块写 failing tests，并运行 focused RED。
- parser error 或测试插入错误不是 contract RED，必须先修正。
- 只有现有 authority 已拥有事实并能在不复制状态的情况下验证时，才实现最小 GREEN。
- 如果没有现有 authority 拥有 Gateway admitted work/completion acceptance，立即停止实现，保留精确 RED，交付 R1 NO-GO/deferred。
- 禁止新增 registry、adapter、facade glue、owner map、handler map、generalized envelope 或 durable store。
- 禁止修改 `src/harness/**`、新增 `CapabilityKind`、泛化 `BackendLease::canonical_entry`、进入 Feishu、创建 production mount 或 Zahir route。
- 禁止实现 EffectClaim producer、Safe Replay、external exactly-once、universal scheduler 或 durable recovery。

完成后更新：

- `docs/superpowers/specs/2026-10-26-r1-provider-source-lease-design.md`
- `docs/superpowers/plans/2026-10-25-capability-core-remaining-roadmap.md`
- `docs/superpowers/runs/2026-10-26-capability-core-r1-capture-ledger.md`

运行并记录：

```bash
cargo fmt --all --check
git diff --check
cargo check -p alephcore --tests
cargo clippy -p alephcore --tests -- -D warnings
cargo test -p alephcore --lib capability::
cargo test -p alephcore --lib gateway::channel_registry::tests::
```

最终报告必须列出：bounded slice、RED/GREEN 命令和实际结果、修改文件、authority duplication 审查、首个未闭合 contract、deferred provider/kind、未运行的验证，以及没有执行的 destructive/external action。只有 source capture、ProviderHandle generation、descriptor publication、owner generation、completion acceptance 和 resource receipts 全部由现有 authority 证明后，才可重新考虑 R1 GO；即使 GO，也必须另开 R2 Feishu A/B re-gate。
