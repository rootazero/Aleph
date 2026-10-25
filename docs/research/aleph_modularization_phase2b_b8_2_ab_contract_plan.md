# Aleph Phase 2B B8.2 A/B 契约计划
# Contract Plan — Descriptor/Kind and Lease/Generation

## 1. 状态与目标 / Status and objective

**状态：下一实施会话的 source-backed contract closure 计划。**

当前 B8.2 Feishu provider lifecycle slice 已合并到 `main`；已知提交链为 feature `492cf56b0` → merge `44ca7c918`，但下一会话必须重新读取实际 `HEAD`、分支和工作树，不能把这两个提交当作未经核对的当前基线。

本阶段只关闭两个决策门：

- **B8.2-A：descriptor / `CapabilityKind`**——证明 Feishu 的真实外部能力能由一个既有 kind、一个既有 descriptor 契约和一个唯一 projection source 表达；或证明不能表达并停止。
- **B8.2-B：lease / generation**——证明 capability identity、provider instance/generation、`OwnershipTree` owner generation、visibility/reference 和 stale/replacement 语义能够在不复制 authority 的前提下绑定；或证明不能绑定并停止。

本阶段的成功标准是得到可审计的 **GO 或 NO-GO/deferred**，以及一份 source-backed 契约映射。成功不等于 Feishu 已成为 Zahir Capability，也不等于已经有 production mount。

## 2. 范围与非目标 / Scope and non-goals

### 2.1 允许做的事

- 读取并核对当前源码、既有审计、closeout、reference 和测试；
- 为九个既有 `CapabilityKind` 做逐项语义审查；
- 绘制 descriptor、revision、fingerprint、visibility、owner、provider generation、owner generation 和 lease 的调用图；
- 在 A/B 均已有完整 source-backed 证明后，做**最小必要的通用契约化**，限于现有 `src/capability/**` 契约，且不接入 Feishu；
- 为上述契约增加效果级测试，测试真实拒绝、可见性和 stale 行为，不只断言函数被调用；
- 更新本阶段交付报告或本计划中的事实记录。代码、测试和文档的具体修改范围必须由 A/B 证据约束。

### 2.2 明确不做的事

- 不实现 B8.2-C callable/effect；
- 不实现 B8.2-D observer/recovery；
- 不实现 Feishu descriptor adapter、Feishu façade glue、Feishu `resolve` route 或 production mount；
- 不新增 `CapabilityKind`，不把 Feishu 伪装成 `Tool`；
- 不新增 callable registry、enabled/actual-state authority、owner/generation/disposer map、durable store；
- 不新增 `EffectClaim` producer、Safe Replay、universal scheduler 或 external exactly-once；
- 不修改 `src/harness/**`；
- 不把 `ChannelRegistry`、`ProviderHandle`、`OwnershipTree`、`api_handle` 或 `SessionEventStore` 的职责互相转移；
- 不把静态 in-process provider 描述成 hot unload，不把通知广播描述成 durable recovery。

如果 A 或 B 不能证明，立即记录 **NO-GO/deferred**，停止实现，不用 façade 胶水、复制状态或测试旁路绕过决策门。

## 3. 当前事实冻结 / Current source-backed facts

下一会话必须用当前源码重新确认下列事实，旧文档与源码冲突时以源码为准并记录冲突：

| 事实 | 当前权威或证据位置 | 本阶段含义 |
|---|---|---|
| `ChannelRegistry` 持有 Gateway channel instance、config/enabled intent、desired state 与生命周期串行边界 | `src/gateway/channel_registry.rs::ChannelRegistry`、`register_for_boot`、`start_all`、`stop_channel_inner`、`lifecycle_sync` | Feishu 的 config/enabled/desired/lifecycle 不能在 Capability 层复制 |
| `ProviderHandle` 持有 provider state、provider instance identity、generation 和 call admission fence | `src/capability/provider.rs::ProviderHandle` | provider generation 是 provider 域的闸，不是 Gateway registry 或 owner generation |
| Feishu lifecycle 已有 provider/API admission、publication fence、revoke/stop/replace residual 语义 | `src/gateway/interfaces/feishu/mod.rs`、`api.rs`、`api_handle.rs`、`feishu_inbound/webhook_server.rs` | 这些是可供契约映射的 provider 事实，不是 Feishu Capability descriptor/lease |
| `api_handle` 是 `Weak` client/config/provider instance/generation 的 derived cache | `src/gateway/interfaces/feishu/api_handle.rs` | 不能作为 descriptor、authority、owner map 或 durable recovery source |
| `CapabilityKind` 当前只有九个既有变体：`Tool`、`Skill`、`Agent`、`Task`、`Resource`、`EventSource`、`Subscription`、`Plugin`、`Hook` | `src/capability/descriptor.rs::CapabilityKind` | 必须逐项审查；禁止新增第十类 |
| 当前 live backend、descriptor fingerprint 生成和 production Zahir host 是 Tool 路径 | `src/capability/backend.rs`、`src/capability/zahir_facade.rs`、`src/capability/projection_host.rs` | Tool-only 生产挂载不能当作 Feishu 证据 |
| `BackendLease` 已包含 descriptor、`owner_generation`、`provider_generation`，但其 `canonical_entry` 仍是 Tool registry 的 capture | `src/capability/facade.rs::BackendLease` | B 必须证明能否最小通用化；不能为 Feishu 伪造 Tool lease |
| `OwnershipTree::generation(&CapabilityId, &VisibilityScope)` 是 per-binding owner generation 读取入口 | `src/capability/ownership.rs::OwnershipTree::generation` | owner generation 只能从此 authority 推导，不能再造 map |
| registry revision、provider generation、owner generation、`SessionEvent.seq` 是不同的计数域 | `src/capability/backend.rs`、`src/capability/provider.rs`、`src/capability/ownership.rs`、`src/session/store.rs` | 不能把 revision/generation/seq 互相替代 |
| `SessionEventStore` 是 session durable authority；live broadcast 和 Feishu cache 不是 durable recovery | `src/session/**`、`docs/reference/FEATURE_LOCATOR.md` | 本阶段不扩展到 provider lifecycle durable producer |

## 4. 调查顺序 / Source-backed investigation order

### Phase 0：baseline、规则和事实校准

1. 先记录：

   ```bash
   pwd
   git rev-parse --show-toplevel
   git rev-parse HEAD
   git status --short --branch
   git log --oneline --decorate -8
   git worktree list --porcelain
   ```

2. 确认 B8.2 lifecycle slice 的 merge/feature 历史确实在当前分支；不得假定 HEAD。
3. 建立 todo，保持每次只有一个任务 `in_progress`。
4. 先读本计划、既有 B8.2 plan/prompt/closeout/audit、当前 B8.2 lifecycle 相关记录，再读以下 reference：
   - `docs/reference/FEATURE_LOCATOR.md`
   - `docs/reference/ARCHITECTURE.md`
   - `docs/reference/TOOL_SYSTEM.md`
   - `docs/reference/DESIGN_PATTERNS.md`
   - `docs/reference/DOMAIN_MODELING.md`
   - `docs/reference/REDLINES.md`
   - `docs/reference/GATEWAY.md`
   - `docs/reference/HARNESS_PHILOSOPHY.md`
5. 记录旧文档中已经过期的断言，例如“没有生产 `ZahirFacade` mount”；当前生产 Tool mount 与 Feishu 不在 mount surface 必须分开表述。

### Phase A：九个既有 kind 的语义审查

对每个 kind 都必须回答同一组问题，禁止只凭名字选择：

| 既有 kind | 必须核对的语义问题 | 可能结论 |
|---|---|---|
| `Tool` | Feishu 是否真的满足 tool 的 callable/schema/handler registry 语义？是否会因此伪装 webhook、channel lifecycle 或 outbound transport？ | 只有完整语义匹配才可选；否则拒绝 |
| `Skill` | 是否只是模型可发现的指令/资源，而非 Feishu provider/transport？descriptor 和 invocation 是否已有事实源？ | 若无完整语义，拒绝 |
| `Agent` | 是否代表一个可独立调度的 agent，而不是 channel provider？是否有唯一 agent authority？ | 若无，拒绝 |
| `Task` | 是否代表可持久化/调度的 task，而不是 channel lifecycle？是否会偷渡 scheduler？ | 若无，拒绝 |
| `Resource` | Feishu 是否是被读取的 resource，还是带 admission/effect 的 live transport？ | 若不能表达 live semantics，拒绝 |
| `EventSource` | 是否只表达 Feishu inbound event source，能否覆盖 identity、visibility、owner、provider generation 而不伪造 callable contract？ | 只能表达真实的那部分；若整体 capability 仍需未有契约的 route，拒绝 |
| `Subscription` | 是否真的是订阅关系，而非 provider 本身？是否会把 live notification 误称 durable subscription？ | 若无 subscription authority，拒绝 |
| `Plugin` | 是否有 plugin lifecycle/mount authority，还是把 Gateway channel 错归为 extension plugin？ | 若无，拒绝 |
| `Hook` | 是否是 hook registration/dispatch，而非 Feishu channel transport？是否已有 hook owner/descriptor authority？ | 若无，拒绝 |

审查必须保留以下负面结论：若九类都不能表达 Feishu 的真实外部 composability，**正确结果是 A=NO-GO/deferred**；不能新增 kind、把 Feishu 伪装成 Tool，或把多种语义拼成一个“万能 kind”。

### Phase B：descriptor 与 projection mapping

在选择 kind 之前，逐字段追踪真实来源：

- `CapabilityId`：稳定 identity 是 channel、provider instance 还是 capability name；何处创建、何处替换；
- `kind`：只能来自 A 的既有语义结论；
- schema/version/fingerprint：canonical source、canonical serialization、SHA-256 计算点；
- `revision`：哪个 authority mutation 分配；必须与 provider/owner generation 分离；
- `visibility`：`Scope.visibility` / `Reference.visibility` 和 `OwnershipTree` 的 binding 如何关联；
- `owner` / `lifetime`：由哪个 `OwnershipTree` binding 推导；
- route metadata：当前 descriptor 是否能表达真实 Feishu route；若需要复制 webhook path、host/port、enabled 或 generation，A 立即停止；
- services/metadata：是否是纯 projection 数据，是否会产生第二事实源。

`describe`、`list`、`project`、`resolve` 相关 read-side 必须保持纯读取；本阶段不得用 descriptor 读取来创建 token、HTTP client、listener、WS runtime 或 Feishu authentication。

### Phase C：lease、generation 与 replacement mapping

以调用图证明三类 generation 的边界：

```text
ChannelRegistry desired/instance/lifecycle authority
  -> FeishuChannel / ProviderHandle provider state + provider generation
  -> OwnershipTree binding(owner, visibility, lifetime, owner generation)
  -> descriptor/reference snapshot
  -> BackendLease
  -> validate/admission boundary
```

必须明确：

1. `ChannelRegistry` 的 instance/config/enabled/desired state 不进入第二份 Capability registry；
2. `ProviderHandle` generation 表示 provider instance start/restart/admission；它不能替代 owner generation；
3. `OwnershipTree::generation()` 是 owner binding generation 的唯一读取 authority；它不能被 `BackendLease`、descriptor 或本地 Feishu map 重新计数；
4. capability registry revision 是 descriptor/publication 变化的 cursor，不能替代 provider generation 或 owner generation；
5. `BackendLease` 必须能识别 capability identity、provider instance/generation、owner binding/generation、visibility/reference 和 descriptor freshness；
6. revoke、stop、replacement、stale lease、late completion 必须 fail-closed；旧 generation 不得撤回、覆盖或调用继任者；
7. 若当前 `canonical_entry: CanonicalEntryLease` 的 Tool-only 形状无法承载非-Tool capability，必须明确最小通用化边界；不能把 Feishu 包装成 Tool entry；
8. `api_handle` 只可作为已有 provider/API publication 的 derived cache，不能作为 lease authority 或 owner map。

## 5. 契约映射表模板 / Contract mapping template

下一会话必须填写下表，所有“来源”和“拒绝路径”都要带仓库相对路径、符号和必要的行号：

| 契约字段/行为 | 唯一事实源 | 读取/派生入口 | mutation/替换入口 | 计数域或 fingerprint | read-side effect | stale/revoke 失败语义 | 证据 |
|---|---|---|---|---|---|---|---|
| capability identity |  |  |  |  | none | unknown/revoked |  |
| existing kind |  |  |  | n/a | none | kind mismatch fail-closed |  |
| descriptor schema |  |  |  |  | none | freshness mismatch |  |
| route metadata |  |  |  |  | none | no source ⇒ NO-GO |  |
| capability revision |  |  |  | registry revision | none | stale snapshot/unknown |  |
| visibility/reference |  |  |  | owner binding key | none | `NotVisible`/`Unknown` |  |
| owner/lifetime |  |  |  | `OwnerGeneration` | none | `Revoked`/`StaleOwner` |  |
| provider instance |  |  |  | instance identity | none | `StaleProvider` |  |
| provider generation |  |  |  | provider generation | none | `StaleProvider` |  |
| lease capture |  |  |  | all above captured | none | no forged lease |  |
| descriptor fingerprint |  |  |  | canonical SHA-256 | none | mismatch ⇒ fail-closed |  |
| late completion |  |  |  | old provider/owner generation | none | cannot publish/withdraw successor |  |

另外填写一张“禁止复制”表：

| 不能复制的事实 | 原 authority | 任何新增副本的位置 | 判定 |
|---|---|---|---|
| channel config/enabled/desired state | `ChannelRegistry` |  |  |
| actual lifecycle instance | `ChannelRegistry` + channel |  |  |
| provider state/generation | `ProviderHandle` |  |  |
| owner generation/visibility/lifetime | `OwnershipTree` |  |  |
| descriptor revision/fingerprint | selected backend/source |  |  |
| durable session events | `SessionEventStore` |  |  |

## 6. 阶段任务、依赖与停止点 / Tasks, dependencies, stop points

### Task 0 — baseline and source inventory

**依赖：无。**只读。交付当前 HEAD/status/worktree、历史链、source index 和过期文档清单。

### Task 1 — A kind audit

**依赖：Task 0。**只读。完成九类语义表和候选 kind 的证据。若没有 kind 可表达，输出 A=NO-GO/deferred，停止后续实现。

### Task 2 — descriptor contract map

**依赖：Task 1 的候选 kind。**只读。完成字段映射表，证明 descriptor/route metadata/fingerprint 是唯一 source 或明确缺口。任一字段需要第二事实源，A=NO-GO/deferred。

### Task 3 — B lease/generation map

**依赖：Task 2 的 A=GO 草案。**只读。完成 provider generation、owner generation、visibility/reference、provider instance 和 stale/replacement/late completion 矩阵。若 Tool-only lease 无法在不伪装 Feishu 的情况下通用化，B=NO-GO/deferred。

### Task 4 — minimal genericization proposal

**依赖：Task 1–3 的完整 GO 证据。**仅在不改变事实源、不接 Feishu、不实现 C/D 的前提下，实施最小必要通用化；可以先只写契约测试和设计 diff。任何需要 adapter、facade glue、production mount、EffectClaim 或第二 registry 的方案都必须停止并改为 deferred。

### Task 5 — A/B re-gate and report

**依赖：Task 4（若有代码）或 Task 3（若无代码）。**重新逐条验收，输出 A/B GO 或 NO-GO/deferred。A/B 任一 NO-GO 时，本阶段终止，不能顺带做 C/D。

依赖关系：

```text
Task 0 → Task 1 → Task 2 → Task 3 → (A/B both GO ? Task 4 : stop) → Task 5
```

## 7. 效果级验收测试 / Effect-level acceptance tests

测试必须证明效果到达了 authority 和拒绝边界，不能只验证“调用了某函数”。A/B 阶段至少设计并在允许范围内执行：

| 场景 | outcome | publication/descriptor | resource | observer/recovery |
|---|---|---|---|---|
| 选定 kind 与真实 Feishu 语义匹配 | 能明确列出可组合行为和未覆盖行为 | 一个 descriptor source，无 Tool 伪装 | 不启动 Feishu runtime | 不把 live notification 当 durable |
| 非选定 kind / namespace resolve | resolve 失败 | 不生成伪造 descriptor/lease | 不创建 client/listener | 无通知伪造 |
| describe/list/project | 返回同一 source 的纯 projection | revision、schema fingerprint 可复核 | 无 token、认证、bind、WS、task | 不读 broadcast/cache 做恢复 |
| visibility/reference 不匹配 | `NotVisible`/`Unknown`，按现有闭集语义 | 不泄露 descriptor | 不触发 runtime | 无 side effect |
| provider not Ready | resolve/use fail-closed | 不发布 executable lease | 不启动 runtime | 不伪造 Ready |
| provider restart | 旧 provider generation 的 lease 失效 | 新 generation 不能被旧 publication 覆盖 | 旧 call 不继续被 admission | 不产生“成功 no-op” |
| owner bump | 旧 owner generation 的 lease 失效 | 新 owner binding 不被旧 lease 接管 | 不执行 late completion | 只允许 authority resync 语义 |
| revoke/dispose | `Revoked`/`Unknown`/等价 fail-closed | 旧 descriptor/lease 不可重新包装 | 不释放或调用继任者 | 不把通知当回放 |
| replacement + late publish/withdraw | incumbent/successor 隔离 | 旧实例不能覆盖或撤回 successor | 由真实 owner 负责 drain/residual | 无历史伪造 |
| revision/schema fingerprint 改变 | freshness 校验失败 | fingerprint 必须由 canonical source 派生 | 无隐式重新启动 | 无 durable claim |

明确不在本阶段执行或声称通过：Feishu callable route、EffectClaim、external effect exactly-once、Capability observer、Feishu provider durable recovery、production mount。

## 8. 决策门 / Decision gates

### B8.2-A GO 条件

必须同时满足：

1. 九个既有 kind 已逐项审查，选定 kind 的语义与 Feishu 真实外部能力一致；
2. 不能用 Tool 伪装 inbound transport、channel lifecycle 或外部 effect；
3. descriptor 的 identity、kind、schema、revision、visibility、owner、lifetime、metadata、fingerprint 各有唯一来源；
4. route metadata 若属于 capability contract，有现有唯一事实源；否则明确 route 留在 Gateway control plane，不能复制；
5. `describe/list/project` 是纯 projection，不启动、认证或连接 Feishu；
6. 没有新增 kind、descriptor authority、registry 或 callable registry。

任一条件不满足：**A=NO-GO/deferred**。

### B8.2-B GO 条件

必须同时满足：

1. lease 能绑定 capability identity、provider instance/generation、owner binding/generation、visibility/reference 和 descriptor freshness；
2. provider generation 只由 `ProviderHandle` 推导，owner generation 只由 `OwnershipTree` 推导，registry revision 只服务 descriptor/publication cursor；
3. revoke、stop、replacement、stale lease、late completion 的真实拒绝路径均 fail-closed；
4. 旧 generation 不能覆盖、撤回或调用 successor；
5. 不新增 owner/generation/disposer map，不把 `api_handle` 变成 authority；
6. 对现有 Tool-only `CanonicalEntryLease` 的任何通用化都是最小、可证明、与 Feishu adapter/mount 解耦的契约改动。

任一条件不满足：**B=NO-GO/deferred**。

### 进入后续阶段的前置条件

只有 A/B 都 GO，且主 Agent 在最终报告中逐条附上源码证据，才可以为未来独立批次制定 C/D 或 production mount 计划。本阶段不自动开启那些工作。

## 9. 验证纪律 / Verification discipline

本设计任务本身不运行 cargo。下一实施会话每条 cargo 命令前，必须**分别**运行并记录：

```bash
python qa/browser_live/check_memory.py --self-test
python qa/browser_live/check_memory.py
```

只有两个命令都 exit 0，且 `available_bytes >= 4294967296`，才允许执行后续 cargo。每次记录 exit、available bytes 和 warning；禁止用其他内存命令替代。

建议按变更范围串行执行：

```bash
cargo fmt --all --check
cargo test -p alephcore --lib capability
cargo test -p alephcore capability --lib
cargo check -p alephcore -p aleph-slot-api --all-targets
cargo clippy -p alephcore --lib --all-targets
```

若修改跨越 capability 与 gateway，增加：

```bash
cargo test -p alephcore --lib feishu
cargo test -p alephcore --lib channel_registry
cargo test -p alephcore --lib
```

不要在本阶段运行 `qa/channels/run.sh all` 来暗示 C/D 已验证；若会话另行执行真实 QA，必须单独记录其范围、依赖和 BLOCKED/UNRUN 原因。

## 10. 最终报告字段 / Final report fields

最终报告必须包含：

- 实际 `HEAD`、branch、status、worktree 和历史确认；
- 读取过的审计/closeout/reference 文件；
- B8.2-A 的九类 kind 语义审查与 descriptor mapping；
- B8.2-B 的 lease/generation/ownership 调用图与 stale matrix；
- 唯一事实源表和禁止复制表；
- A/B 各自 GO 或 NO-GO/deferred，以及每一条失败条件的路径、符号和证据；
- 修改文件、未修改范围和任何最小通用化 diff；
- 每条验证的 memory gate、exit、测试数量、warning 和未运行原因；
- 对 C/D、production mount、EffectClaim、durable recovery 的明确未做声明；
- North Star conformance：disabled/read-side isolation、failure containment、runtime composability 边界；
- 残余风险、最小后续工作和工作树最终状态。

## 11. 下一会话交付物 / Deliverables

至少交付：

1. 一份 source-backed B8.2-A/B contract mapping；
2. 九个既有 kind 的逐项语义审查表；
3. descriptor/lease/generation/replacement/stale 的调用图和效果级验收矩阵；
4. A/B 决策门结果；
5. 若 NO-GO/deferred：不实现 adapter/mount，并列出最小前置工作；
6. 若 A/B 均 GO：只交付经过证明的最小通用契约改动与测试，仍不实现 C/D 或 production mount。

## 12. 风险与未做事项 / Risks and explicit negatives

- 当前 Tool backend 的 descriptor/fingerprint 和 `BackendLease` 仍与 Tool registry 有强耦合；这可能使 B 在不扩大范围的情况下仍然 NO-GO。
- 当前 Feishu channel route、outbound admission、inbound external effect 和 capability descriptor 的边界尚未证明；不要通过复制 route metadata 伪造完整性。
- provider generation、owner generation、registry revision 和 session event sequence 的数值相似不代表语义相同。
- `ProviderHandle` 的 admission fence 不等于完整 accepted-handler ownership/join，也不等于 external-effect exactly-once。
- 本阶段不验证或实现 C/D、EffectClaim producer、Safe Replay、scheduler、durable Feishu provider recovery、production mount、其他 kind backend、`src/harness/**`。
- 未通过当前会话的 A/B 证据，不得把 Feishu 描述为 live Capability 或宣称可以进入 production mount。
