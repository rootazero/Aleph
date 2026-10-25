# Aleph Phase 2B B8.2 Feishu Capability Design

## Intent
在不伪造 Capability 接入的前提下，闭合 Feishu provider slice 的生命周期、Capability 契约、调用/效果边界和 observer/recovery 证据，并据此重新判断是否允许 Feishu → Zahir production mount。North Star 是生命周期隔离与运行时可组合性：disabled capability 不初始化资源，失败隔离，旧 handle 失效，核心不依赖具体 backend。

## Goals
- 以 ChannelRegistry 作为唯一 Gateway lifecycle authority，统一 enabled、boot、explicit enable、stop、replacement。
- 修复并用效果级测试证明 disabled boot、replacement、publication fence、admission/drain/residual/provider failure。
- source-backed 判断既有 CapabilityKind 是否能表达 Feishu；descriptor、route metadata、visibility、owner、revision、fingerprint 只从唯一事实源推导。
- 复用 ProviderHandle generation、BackendLease、OwnershipTree::generation() 和现有 Zahir/Tool backend 契约；stale/revoked/replaced/late completion fail-closed。
- read-side 纯 projection；outbound 按 resolve → lease validation → ChannelRegistry actual instance → ProviderHandle admission → FeishuApi admission → outbound call；live observer 先 subscribe 后 snapshot，lag 只 invalidation + authority resync。
- 若 A-D 任一门不能证明，停止在该门，保留 provider lifecycle slice，更新 FEATURE_LOCATOR/reference/audit 状态并报告 NO-GO/deferred。

## Non-goals
本阶段不承诺 Feishu 已成为 Zahir live Capability；不新增 Capability kind、callable registry、enabled/actual-state authority、owner/generation/disposer map、durable store；不实现 EffectClaim producer、Safe Replay、universal scheduler、external exactly-once、MCP lazy；不修改 src/harness/；不把静态 in-process provider称为 hot unload。

## Anti-goals
不得把 Feishu 伪装成 Tool；不得让 catalog/describe/list/resolve/project/doctor/observer 启动或认证 Feishu；不得用 api_handle、broadcast 或测试 fixture 充当 authority/durable recovery；不得用 facade 胶水或复制状态绕过决策门；不得自动 commit/push/pull/reset/rebase/stash。

## Constraints and current facts
当前基线预期 HEAD 为 eba5a0349132d618334c7a634b964de014b2d8f7，实际值必须重新记录；main working tree 当前有两个未跟踪 B8.2 implementation research 文件，必须保留。B5/B6/B7/B8 历史存在。ChannelRegistry 是 Gateway authority；ProviderHandle 只负责 provider state/generation/admission；api_handle 是 derived cache；OwnershipTree::generation() 是 owner generation 唯一来源；SessionEventStore 是 durable session authority。现有 CapabilityKind 九种（Tool、Skill、Agent、Task、Resource、EventSource、Subscription、Plugin、Hook），当前 ToolBackendAdapter 明确投影 Tool，不能假定 Feishu 可复用 Tool。BackendLease 含 descriptor、owner_generation、provider_generation、canonical_entry；现有 ZahirFacade lease validation、snapshot-anchored subscribe 和 project 不等于 Feishu 已接线。用户指定的 docs/research/aleph_modularization_phase2b_b8_2_audit.md 在当前目录未发现；上一阶段 b8 audit 存在，不能混用。

## Approaches
1. Lifecycle closure then gate (recommended): 先闭合 Phase 1，补效果测试和 source-backed contract map，再按 A-D 停/行；若既有 kind 无法准确表达 Feishu，停在 A/B，保留 provider slice。优点是尊重唯一 authority 和 fail-closed；代价是可能完成 lifecycle 后仍 NO-GO。
2. Minimal evidence closure: 只补最小 lifecycle/lease/observer 证据和文档，尽早得到 deferred 结论，不继续改生产路由。优点是改动最小；代价是无法完成真实 lifecycle closure 时仍留缺口。
3. Adapter-first mount: 先造 generic adapter 并挂到 Tool。排除：会复制 descriptor/owner/callable authority或伪装 Feishu 语义，违反硬约束。

## Design by phase
### Phase 1 Gateway lifecycle authority
Feishu 内部不得复制 enabled authority；disabled boot 无 token/API/webhook/WS/emitter runtime、listener bind、cache entry 或 publication。boot/enable/stop/replacement 使用同一 registry/owner。replacement 先关闭旧 admission，再关闭旧独立 webhook/WS task，publication 使用 instance/generation fence；旧 late publish/withdraw 不能影响新实例。明确顶层 accept task、accepted handler、已准入 API call 的 ownership；drain/join 只宣称真实证明范围，不能完整 join 时保留结构化 residual。provider failure 必须保留 Failed/residual。

### Phase 2 descriptor/kind and lease/generation
先写映射表：kind 是否表达真实 Feishu 语义、descriptor 唯一来源、route metadata、visibility/reference、revision/fingerprint。descriptor 生成不能启动/认证 Feishu。lease 绑定 capability identity、provider generation、owner generation、visibility/reference；OwnershipTree::generation() 唯一提供 owner generation；revoke/stop/replacement/stale/late completion fail-closed。若不能表达则停止 A/B。

### Phase 3 callable/effect
catalog/describe/list/resolve/project/doctor/observer 纯 projection。outbound 必须经过完整 admission chain。stop/revoke 关闭新调用并真实 drain 已准入工作。webhook 是 external effect；本阶段不新增 EffectClaim/Safe Replay/scheduler/exactly-once；无法安全表达 inbound effect 时保留 Gateway-owned event path并将 C deferred。

### Phase 4 observer/recovery
先 subscribe 再 snapshot；lifecycle change 必须有 revision/epoch 或等价 invalidation；broadcast lag 只发 Invalidated 并从 authority 取 fresh snapshot；observer 不从 api_handle/broadcast 恢复 durable state；不新增 durable store。SessionEventStore 只恢复 session content，除非现有 event contract 明确能表达 provider lifecycle；不能证明则 D deferred。覆盖 subscribe-before-snapshot、registration/replacement/stop lag、invalidation/resync、subscriber restart、live notification与durable replay分离。

## Failure matrix
| outcome | publication | resource | observer |
|---|---|---|---|
| disabled | no Ready / no callable | no token/API/listener/cache/runtime | no publication; snapshot excludes it |
| start failure | no Ready; Failed/residual | partial resources explicitly disposed or residual | lifecycle failure invalidates/resyncs |
| replacement | old generation withdrawn; new only | old admission closed; old webhook/WS handled; residual explicit | invalidation then new snapshot |
| stop/revoke | no new calls; old leases fail closed | admitted work drains per actual semantics; residual if not joinable | invalidation then absent/stopped snapshot |
| stale/late completion | cannot publish/withdraw new instance | cannot mutate new resource | no stale notification as current state |
| observer lag/restart | no durable replay claim | resync from authority | Invalidated + fresh snapshot |

## Decision gates
A GO only if an existing kind is semantically exact and all descriptor/visibility/revision/fingerprint sources are unique; otherwise NO-GO/deferred. B GO only if BackendLease/provider generation/owner generation/reference are bound without duplicate authority and all stale/revoke/replacement paths fail closed. C GO only if callable route uses real Feishu admission and effect boundary needs no forbidden mechanism; otherwise Gateway-owned/deferred. D GO only if subscribe-before-snapshot, invalidation/resync and durable boundary are proven; otherwise deferred. Production mount requires all A-D GO plus effect-level evidence for disabled/read-side/replacement/stale/late/failure.

## Source-backed investigation and verification
Read/trace src/gateway/channel_registry.rs, src/gateway/interfaces/feishu/**, src/gateway/inbound_router/executor.rs, src/capability/{descriptor,facade,backend,ownership,provider,zahir_facade,projection_host}.rs, src/tools/{registry,service}.rs, and graphify-out/ as navigation aid; source is authority. Before every cargo command run python qa/browser_live/check_memory.py --self-test and then python qa/browser_live/check_memory.py; require both exit 0 and available_bytes >= 4294967296, record warnings. Run requested cargo commands serially and qa/channels/run.sh all; missing websockets or other deps is BLOCKED/UNRUN, never fabricated pass.

## Documentation and delivery
After implementation or a gate stop, update docs/reference/FEATURE_LOCATOR.md, docs/reference/ARCHITECTURE.md, docs/reference/TOOL_SYSTEM.md and the appropriate B8.2 audit/closeout document with actual source-backed status, failure matrix, exact residuals, verification evidence and next smallest prerequisite. Do not claim production mount unless all gates pass.

## Builder tasks
The builder must implement only the approved phase order, keep one task in progress, add effect-level tests for each changed behavior, and stop immediately at the first unprovable gate. Main agent performs independent review and final acceptance.

## Residual risks
Accepted-handler ownership/join may remain incomplete; static in-process provider remains distinct from hot unload; current capability backend may not semantically express Feishu; requested B8.2 audit path may be missing. These are explicit stop conditions, not assumptions.
