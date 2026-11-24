# Next-session implementation prompt — R1 provider binding and completion proof

你是 Aleph Capability Core 的主实现 agent。请从现有仓库继续执行 **R1 的下一段 provider binding/completion proof**，不要进入 R2 Feishu，也不要把多个 provider migration 混在一个阶段。

仓库：
`/Volumes/TBU4/Workspace/Aleph`

## 架构终极目标

`Everything externally composable is a Capability.`

一切需要被 Agent、Runtime 或其他系统动态发现、调用、替换、订阅或管理的能力，都是 Capability。Aleph 要从“一个集成很多功能的进程”演进成“一个轻量 Capability Core，加上一组按需启动、可停止、可替换的能力提供者”。Gateway 下的十几个社交媒体通信通道、Terminal、Memory/Nebula graph、Sandbox 都是后续 provider；后台关闭的 provider 不应在启动 Aleph 时把线程、连接、数据库、缓存、token client、listener 或 websocket 拉入运行时。

## 必读文件

- `AGENTS.md`
- `docs/superpowers/plans/2026-10-26-r1-provider-binding-completion-plan.md`
- `docs/superpowers/plans/2026-10-25-capability-core-remaining-roadmap.md`
- `docs/superpowers/specs/2026-10-26-r1-provider-source-lease-design.md`
- `docs/superpowers/specs/2026-10-25-capability-core-provider-contract-design.md`
- `docs/superpowers/runs/2026-10-26-capability-core-r1-capture-ledger.md`
- `docs/research/Everything-externally-composable-is-a-Capability.md`
- `docs/reference/ARCHITECTURE.md`
- `docs/reference/FEATURE_LOCATOR.md`
- `docs/reference/TOOL_SYSTEM.md`
- `docs/reference/REDLINES.md`
- `docs/reference/DESIGN_PATTERNS.md`
- `docs/reference/DOMAIN_MODELING.md`

## 当前状态

- R0 已通过。
- R1 common `ChannelRegistry` teardown 已闭合 bounded slice。
- `src/gateway/channel_registry.rs` 已有 crate-private `ChannelCapture`：捕获 channel `Arc` identity 与 source publication revision；replacement/unregister 后旧 capture fail-closed。
- capture 专属测试 3 passed；完整 `gateway::channel_registry::tests::` 过滤集 44 passed。
- `cargo check -p alephcore --tests`、strict Clippy、fmt check、diff check 已通过。
- R1 overall 仍为 **NO-GO / deferred**。
- 尚未闭合：ProviderHandle instance/generation binding、descriptor identity/schema/revision binding、OwnershipTree owner generation observation、admitted-work completion acceptance、resource receipts。
- Feishu 尚未接入 descriptor、resolve、production mount 或 Zahir route。
- 当前工作不授权 R2、R3、universal lease、EffectClaim 或 durable recovery。

## 本轮唯一目标

继续 R1，回答并验证：

1. `ChannelRegistry::ChannelCapture` 如何与真实 `ProviderHandle` instance/generation 对应？
2. descriptor identity/schema/fingerprint/source revision 如何从既有 canonical source 派生，而不复制 descriptor registry？
3. `OwnershipTree` owner generation 如何作为独立 observation 参与 validation？
4. admitted work 保留什么 source-owned proof？replacement/unregister/revoke/dispose 后，completion 在哪个线性化边界被拒绝？
5. disabled/discovered/cold 的 describe/list/project/resolve 如何证明没有初始化线程、连接、数据库、缓存、token client、listener 或 websocket？

如果现有 authority 不能回答这些问题，必须停止并交付 **R1 NO-GO / deferred**，指出第一个未闭合 contract。禁止用新 registry、adapter、facade glue、测试旁路或 generalized envelope 掩盖缺口。

## 强制约束

- 不得修改 `src/harness/**`。
- 不得新增 `CapabilityKind`。
- 不得新增 capability/source/provider/descriptor/callable/handler/owner/generation/disposer registry 或 durable store。
- 不得创建 Feishu adapter、Feishu facade glue、Feishu resolve bypass、production mount 或 Feishu→Zahir route。
- 不得在 R1 GO 前进入 R2 Feishu A/B 审计结论之外的实现。
- 不得泛化 Tool-specific `BackendLease::canonical_entry`。
- 不得合并 provider generation、owner generation、source revision、`SessionEvent.seq`。
- 不得把 live broadcast 当作 durable recovery source。
- 不得实现 EffectClaim producer、Safe Replay、external exactly-once、universal scheduler 或 durable Feishu recovery。
- 不得将 query `Scope` 当 disposer，也不得让 shared provider 因单个 binding 被撤销而隐式停止。
- 不得执行 commit、push、reset、rebase、stash 或 clean；本轮只做实现和验证，提交另行授权。

## 执行顺序

1. 阅读上述文件并建立/更新 SDD ledger。
2. 在真实 authority 的测试模块中先写 failing tests，观察 RED；修正测试插入错误后再判断 RED。
3. 只在拥有事实的 authority 中实现最小 GREEN。
4. 对每个新增 claim 运行专属测试并检查 authority duplication。
5. 更新 R1 spec、roadmap 和 ledger，明确 PASS 的 bounded slice、整体 GO/NO-GO、首个剩余 contract。
6. 运行：

```bash
cargo fmt --all --check
git diff --check
cargo check -p alephcore --tests
cargo clippy -p alephcore --tests -- -D warnings
cargo test -p alephcore --lib capability::
cargo test -p alephcore --lib gateway::channel_registry::tests::
```

同时运行每个新增测试的 focused command。不要把未运行的 full workspace 或真实 provider QA 写成已验证。

## 交付要求

最终报告必须列出：

- 已完成的 bounded R1 slice 和证据；
- 修改文件；
- RED/GREEN 测试命令与实际结果；
- fmt/check/clippy/test 结果；
- authority duplication 审查结果；
- 仍 deferred 的 provider/kind 和第一个未闭合 contract；
- 没有执行的 destructive/external action；
- 下一阶段唯一推荐入口。

只有在 source capture、ProviderHandle generation、descriptor publication、owner generation、completion acceptance 和 resource receipts 全部由现有 authority 证明后，才可以把 R1 改为 GO。即使 R1 GO，也必须另开 R2 Feishu A/B re-gate，不得直接写 Feishu Rust wiring。
