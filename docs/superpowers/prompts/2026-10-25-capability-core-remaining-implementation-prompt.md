# Next-session implementation prompt: Capability Core remaining roadmap

你是 Aleph Capability Core 的主实现 agent。请在现有仓库中按 roadmap 的 gate 顺序继续实施，不要把所有未完成子系统一次性混合修改。

仓库：
`/Volumes/TBU4/Workspace/Aleph`

先阅读：

- `AGENTS.md`
- `docs/superpowers/plans/2026-10-25-capability-core-remaining-roadmap.md`
- `docs/superpowers/specs/2026-10-25-capability-core-provider-contract-design.md`
- `docs/superpowers/plans/2026-10-25-capability-core-contract-first-vertical-slice.md`
- `docs/research/aleph_modularization_plan.md`
- `docs/research/Everything-externally-composable-is-a-Capability.md`
- `docs/research/aleph_modularization_phase2b_b8_2_ab_contract_plan.md`
- `docs/research/aleph_modularization_phase2b_b8_2_closeout.md`
- `docs/reference/ARCHITECTURE.md`
- `docs/reference/FEATURE_LOCATOR.md`
- `docs/reference/TOOL_SYSTEM.md`
- `docs/reference/REDLINES.md`
- `docs/reference/DESIGN_PATTERNS.md`
- `docs/reference/DOMAIN_MODELING.md`

当前已完成：

- `CapabilityKind` 保持九类。
- `CapabilityFacet` 已存在：`Callable`、`EventSource`、`Subscription`、`ProviderLifecycle`、`ExternalEffect`。
- Tool 只声明 `Callable`。
- `CapabilityBackend::snapshot(&Scope) -> CapabilitySnapshot` 已实现。
- `ZahirFacade::describe` 使用 source snapshot contract。
- cold provider 的 `describe`、`project`、`resolve` 不会隐式启动 provider。
- provider generation、owner generation、source revision 和旧 generation late completion 已有独立测试。
- Tool-specific `BackendLease::canonical_entry` 保持不变。
- `ProviderHandle`、`OwnershipTree`、`ToolHandlerRegistry`、`SessionEventStore` 仍是各自 authority。
- capability suite 当前为 214 passed。
- `src/harness/**` 未修改。
- Feishu 尚未接入 Capability descriptor、resolve、production mount 或 Zahir route。

本次总目标：

`Everything externally composable is a Capability.`

将 Aleph 演进为轻量 Capability Core，加上一组可以按需发现、启动、停止、替换、订阅和恢复的 provider。disabled provider 不能初始化线程、连接、数据库、缓存、token client、listener 或 websocket。

强制执行顺序：

1. R0：重新验证当前 Tool/Core baseline。
2. R1：完成通用 provider/source/lease 设计 gate。
3. R2：只读完成 Feishu A/B source-backed re-gate。
4. 只有 R2 明确 GO，才能进入 R3 Feishu production slice。
5. R4 独立关闭 EventSource/Subscription 语义。
6. R5 独立关闭 EffectClaim producer、Safe Replay、external exactly-once 和 durable recovery。
7. R6 逐个迁移 Terminal、Memory、Sandbox。
8. R7 做全 workspace 和真实边界验证。

绝对禁止：

- 不得把 Feishu 伪装成 Tool。
- 不得新增 CapabilityKind。
- 不得创建第二份 callable registry、descriptor registry、handler map、enabled-state map、owner/generation/disposer map 或 durable store。
- 不得创建 Feishu descriptor adapter、Feishu facade glue、Feishu resolve bypass 或 test-only production authority。
- 不得在 R2 未 GO 前创建 Feishu production mount 或 Feishu→Zahir route。
- 不得在第二个真实 callable provider 证明需要之前泛化 Tool-specific `BackendLease::canonical_entry`。
- 不得把 internal admission fence 自动标成 `ProviderLifecycle` 或 `ExternalEffect`。
- 不得把 live broadcast 当成 durable recovery source。
- 不得把 provider generation、owner generation、source/registry revision、`SessionEvent.seq` 合并成一个计数器。
- 不得实现 EffectClaim producer、Safe Replay、external exactly-once、universal scheduler 或 durable Feishu recovery，除非已经通过 R5 独立 design gate。
- 不得修改 `src/harness/**`。
- 不得执行 commit、push、reset、rebase、stash 或 clean，除非用户在当前会话明确授权。

R1 的输出必须包括：

- provider/source/descriptor/owner/lease 关系图；
- authority non-duplication 表；
- lifecycle state machine；
- replacement/revoke/dispose/late-completion failure matrix；
- `describe/list/project/resolve` no-start/no-auth/no-network 证明；
- universal non-Tool lease 的至少两种方案和明确取舍；
- source revision、provider generation、owner generation、`SessionEvent.seq` 的独立性证明；
- R1 GO 或 R1 NO-GO/deferred。

R2 的输出必须包括：

- 九类 `CapabilityKind` 对 Feishu 的逐项审计；
- Feishu descriptor identity、facet、source revision、visibility、owner、provider instance/generation 的源代码路径和行号；
- webhook admission、outbound API admission、accepted handler ownership/drain、replacement、unregister、late completion 的 call graph；
- read-side side-effect audit；
- A/B 唯一 decision：GO 或 NO-GO/deferred。

如果 R2 失败：

- 立即停止所有 Feishu implementation work；
- 输出第一个未闭合 contract；
- 不通过 facade glue、adapter、测试旁路或新增 registry 绕过；
- 将下一步限定为修订 Core contract 或补齐缺失 authority。

如果 R2 通过：

- 先创建独立 Feishu design spec；
- 等 design gate 通过后再修改 Rust；
- 让 descriptor 从现有 `ChannelRegistry`/Feishu provider canonical source 派生；
- 让 `ProviderHandle`、`OwnershipTree`、source revision 和 channel lifecycle 各自保留 authority；
- 先实现 read-side snapshot、visibility、provider generation 和 replacement fencing；
- 再实现 callable/event/subscription projection；
- `ExternalEffect` 继续保持 deferred，直到 R5 完成。

每个阶段都要：

- 建立/更新 SDD ledger；
- 先写 failing test，观察 RED；
- 实现最小 GREEN；
- 运行该阶段专属测试；
- 运行 `cargo fmt --all --check`、`git diff --check`、相关 `cargo check`、`cargo clippy` 和测试；
- 检查 diff 中没有 authority duplication；
- 更新对应设计/计划文档；
- 真实失败时停止并报告，不要自动跳到下一 gate。

最终报告必须列出：

- 已完成阶段和证据；
- 修改文件；
- 实际运行的命令与结果；
- 未运行项；
- 仍 deferred 的 CapabilityKind/provider；
- 没有做的 destructive 或 external action；
- 下一阶段唯一推荐入口。

先执行 R0，然后只进入 R1。不要直接开始 Feishu Rust wiring。
