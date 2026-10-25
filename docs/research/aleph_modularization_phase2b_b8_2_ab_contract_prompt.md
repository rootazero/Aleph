# Aleph Phase 2B B8.2 A/B 契约实施 Prompt
# Implementation Prompt — Close Descriptor/Kind and Lease/Generation Gates

你是 Aleph 模块化 Phase 2B B8.2 的主 Agent / Lead Orchestrator。你的任务是完成 **B8.2-A descriptor/kind 与 B8.2-B lease/generation 的 source-backed 契约审查和最小必要通用化**。

完整计划：

`docs/research/aleph_modularization_phase2b_b8_2_ab_contract_plan.md`

工作目录：`.`。所有引用使用仓库相对路径，不写死机器绝对路径。

## 0. 任务边界与停止规则

当前 Feishu provider lifecycle slice 已合并到 `main`；已知历史链为 feature `492cf56b0` → merge `44ca7c918`，但必须读取实际 `HEAD`。当前基线预期为：

- `ChannelRegistry` 是 config/enabled/desired/instance/lifecycle authority；
- `ProviderHandle` 负责 provider state、provider instance identity、generation 和 admission；
- `OwnershipTree` 是 owner generation、visibility、lifetime authority；
- `api_handle` 是 derived cache；
- 当前没有 Feishu → Zahir production adapter；
- 当前 production Zahir/Tool mount 只证明 Tool capability；
- B8.2-A/B 仍是 NO-GO/deferred，除非本会话用当前源码重新证明。

本会话只关闭 A/B。禁止把“provider lifecycle fence 已完成”写成“Feishu 已是 live Capability”。

如果出现以下任一情况，立即停止实现并交付 NO-GO/deferred：

- 九个既有 `CapabilityKind` 都不能表达 Feishu 的真实外部能力；
- 需要新增 kind，或需要把 Feishu 伪装成 `Tool`；
- descriptor、route metadata、revision、fingerprint、visibility、owner 或 lifecycle 需要第二份 authority；
- `BackendLease` 的 Tool-only `CanonicalEntryLease` 无法在不伪造 Tool 语义的情况下绑定 Feishu；
- provider generation 与 owner generation 无法在不复制 map 的情况下分别证明；
- stale lease、revoke、stop、replacement 或 late completion 不能 fail-closed；
- 需要实现 C/D、production mount、EffectClaim 或任何被禁止的机制才能继续。

## 1. 硬约束 / Hard constraints

严格禁止：

- 新增 `CapabilityKind`；
- 把 Feishu 伪装成 Tool；
- 新增 callable registry、descriptor registry、enabled/actual-state authority；
- 新增 owner/generation/disposer map；
- 新增 durable store、Feishu provider lifecycle producer 或 recovery log；
- 实现 B8.2-C callable/effect 或 B8.2-D observer/recovery；
- 实现 Feishu adapter、Zahir façade glue、Feishu `resolve` route 或 production mount；
- 新增 `EffectClaim` producer、Safe Replay、universal scheduler、external exactly-once；
- 修改 `src/harness/**`；
- 让 read-side `describe/list/project/resolve/doctor` 启动、认证或连接 Feishu；
- 把 `api_handle`、broadcast、测试 fixture 或 `SessionEventStore` 充当 Feishu capability authority；
- 把静态 in-process provider 描述为 hot unload；
- `reset`、`pull`、`push`、`rebase`、`stash`、`clean`、覆盖用户改动或自动 commit。

如需修改代码，只允许在 A/B 证据完整之后，做现有 `src/capability/**` 契约的最小通用化和对应效果测试；不跨入 Feishu adapter/mount。无法证明就只读审查、文档报告和测试设计。

## 2. 起点核对、计划和委派 / Baseline, todo, delegation

先执行并记录：

```bash
pwd
git rev-parse --show-toplevel
git rev-parse HEAD
git status --short --branch
git log --oneline --decorate -8
git worktree list --porcelain
```

确认 feature/merge 历史、当前分支和工作树状态。若工作树有用户改动，先记录并避开；不要清理或覆盖。

随后：

1. 读取 `docs/research/aleph_modularization_phase2b_b8_2_ab_contract_plan.md`；
2. 读取既有输入：
   - `docs/research/aleph_modularization_phase2b_b8_2_plan.md`
   - `docs/research/aleph_modularization_phase2b_b8_2_prompt.md`
   - `docs/research/aleph_modularization_phase2b_b8_2_closeout.md`
   - `docs/research/aleph_modularization_phase2b_b8_audit.md`
   - `docs/research/aleph_modularization_phase2b_b8_2_implementation_plan.md`
   - `docs/reference/FEATURE_LOCATOR.md`
   - `docs/reference/ARCHITECTURE.md`
   - `docs/reference/TOOL_SYSTEM.md`
   - `docs/reference/DESIGN_PATTERNS.md`
   - `docs/reference/DOMAIN_MODELING.md`
   - `docs/reference/REDLINES.md`
   - `docs/reference/GATEWAY.md`
   - `docs/reference/HARNESS_PHILOSOPHY.md`
3. 建立 todo，保持每次只有一个任务 `in_progress`；
4. 使用 subagent 做至少两项独立只读核对：一项核对九个 kind/descriptor/Tool-only backend，一项核对 provider/owner/lease/replacement 语义。委派结果必须由主 Agent 回到源码复核，不能把 subagent 结论当作证据；不要递归调用其他 AI CLI 或递归 agent。

## 3. Source-backed A 审查：九个 kind 和 descriptor

读取并核对：

- `src/capability/descriptor.rs`
- `src/capability/backend.rs`
- `src/capability/facade.rs`
- `src/capability/zahir_facade.rs`
- `src/capability/projection_host.rs`
- 当前 Feishu lifecycle 文件：`src/gateway/channel_registry.rs`、`src/gateway/interfaces/feishu/**`

逐项审查全部九类：

```text
Tool, Skill, Agent, Task, Resource, EventSource, Subscription, Plugin, Hook
```

每一类都必须记录：语义定义、现有 live backend/authority、Feishu 真实行为是否完全匹配、缺失字段/行为、选择或排除理由。

必须回答：

1. Feishu 是 Tool、EventSource、Subscription 或其他已有 kind 的真实语义，还是当前九类均不能表达？
2. 若选择某 kind，是否会把 channel lifecycle、inbound external effect、webhook route 或 outbound transport 伪装成另一种能力？
3. `CapabilityDescriptor` 的 identity、kind、schema/version、revision、lifetime、visibility、owner、services、metadata 和 fingerprint 的唯一来源是什么？
4. Feishu route metadata 是否已经有唯一来源和现有 descriptor 表达？若需要复制 `webhook_path`、host/port、enabled、provider generation 或 channel state，A 必须 NO-GO。
5. `describe/list/project` 是否完全是 read-side projection？不得创建 TokenManager、HTTP client、listener、WS runtime、refresh task 或 Feishu authentication。
6. 当前 `ToolBackendAdapter::to_descriptor`、Tool namespace、`ToolHandlerRegistry` 和 production `ProjectionHost`/`ZahirFacade` mount 是否只能证明 Tool？必须把 Tool 证据与 Feishu 证据分开。

A 的交付必须包含九行语义表和 descriptor 契约映射。无法选择既有 kind 或无法给出唯一 descriptor authority 时，**A=NO-GO/deferred，停止写代码**。

## 4. Source-backed B 审查：lease、generation 和 ownership

读取并核对：

- `src/capability/facade.rs::BackendLease`
- `src/capability/provider.rs::ProviderHandle`、`ProviderLifecycle`、`ProviderState`
- `src/capability/ownership.rs::OwnershipTree`
- `src/capability/zahir_facade.rs::resolve`、`validate_lease`
- `src/gateway/channel_registry.rs`
- `src/gateway/interfaces/feishu/mod.rs`
- `src/gateway/interfaces/feishu/api.rs`
- `src/gateway/interfaces/feishu/api_handle.rs`

必须画出并逐段引用以下关系：

```text
ChannelRegistry instance/config/enabled/desired/lifecycle authority
  -> FeishuChannel + ProviderHandle provider state/instance/generation/admission
  -> OwnershipTree (owner, visibility, lifetime, owner generation)
  -> descriptor/reference snapshot
  -> BackendLease
  -> use-time validation/admission
```

必须明确四个不同的计数/身份域：

- registry/capability revision：descriptor/publication cursor；
- provider instance identity + provider generation：`ProviderHandle` start/restart/admission fence；
- owner binding identity + `OwnerGeneration`：`OwnershipTree::generation(...)`、bump/revoke/dispose；
- `SessionEvent.seq`：session durable event cursor，不能用于 capability lease。

必须回答：

1. lease 如何绑定 capability identity、provider instance/generation、owner binding/generation、visibility/reference 和 descriptor freshness？
2. 当前 `BackendLease.canonical_entry` 的 Tool registry 绑定是否阻止非-Tool/Feishu lease？若阻止，最小通用化边界是什么？若只能通过复制 registry/lease/authority 解决，B=NO-GO。
3. `ProviderHandle` generation 与 `OwnershipTree` generation 的边界在哪里？不能新增任何 generation map 或把一个 generation 复制到另一个 authority。
4. provider `Ready`、owner binding、visibility 和 descriptor freshness 的检查顺序是什么？每个失败必须落入现有闭集错误或等价 fail-closed 语义。
5. stop、revoke、replacement、stale lease、late completion 的效果是什么？旧实例/旧 generation 是否能覆盖、撤回、重新包装或调用 successor？
6. `api_handle` 是否始终只是 derived cache？必须证明它不能成为 descriptor、lease、owner map 或 lifecycle authority。

B 的交付必须包含 lease 字段表和 failure matrix。无法完整绑定或无法证明所有 stale/replacement/late paths fail-closed 时，**B=NO-GO/deferred，停止写代码**。

## 5. 契约映射交付格式 / Required mapping

最终映射至少包含：

| 字段/行为 | 唯一事实源 | 读取/派生入口 | mutation/替换入口 | 计数域/fingerprint | read-side effect | fail-closed 结果 | 源码证据 |
|---|---|---|---|---|---|---|---|
| capability identity |  |  |  |  | none |  |  |
| selected existing kind |  |  |  | n/a | none |  |  |
| descriptor schema/fingerprint |  |  |  | canonical SHA-256 | none |  |  |
| route metadata |  |  |  |  | none | missing source ⇒ NO-GO |  |
| capability revision |  |  |  | registry revision | none | stale/unknown |  |
| visibility/reference |  |  |  | binding key | none | NotVisible/Unknown |  |
| owner/lifetime |  |  |  | OwnerGeneration | none | Revoked/StaleOwner |  |
| provider instance/generation |  |  |  | ProviderHandle domain | none | StaleProvider |  |
| lease capture/freshness |  |  |  | combined proof | none | Unknown/Stale* |  |
| replacement/late completion |  |  |  | old vs successor identity | none | cannot mutate successor |  |

必须另列“禁止复制”的事实表，至少覆盖：`ChannelRegistry` enabled/desired/instance、`ProviderHandle` state/generation、`OwnershipTree` owner/visibility/lifetime/generation、descriptor revision/fingerprint、`SessionEventStore` durability。

## 6. 只允许的最小实现 / Minimal implementation allowed

只有在 A 和 B 的 source-backed mapping 都达到 GO 后，才可考虑以下顺序：

1. 先写契约测试和最小设计 diff；
2. 仅在既有 `src/capability/**` 中解除明确的 Tool-only 类型耦合；
3. 保留现有 Tool backend 的唯一 authority 和行为；
4. 为 `BackendLease` 的 identity/provider/owner/reference 绑定补充效果级验证；
5. 验证通过后停止在 A/B。

本阶段不得：

- 创建任何 Feishu descriptor、Feishu backend、Feishu adapter、Feishu callable route 或 production mount；
- 把泛化后的契约立即接到 `ChannelRegistry`；
- 实现 C/D 的 route/effect/observer/recovery；
- 为未来 kind 建 placeholder registry 或伪造 publication。

如果最小通用化不能在这些限制内完成，保持代码不变，报告 NO-GO/deferred 和最小前置工作。

## 7. 效果级验收 / Effect-level acceptance

不要只测试函数调用。至少设计并执行适用的测试：

- 非选定 kind、foreign namespace 或 visibility mismatch 不会 resolve 出 lease；
- descriptor snapshot 与 fingerprint 来自同一 source/revision，且 read-side 不启动、认证或连接 Feishu；
- provider 非 `Ready` 或 provider generation 改变时，旧 lease/use path fail-closed；
- owner bump/revoke/dispose 后，旧 owner generation 的 lease fail-closed；
- descriptor revision/schema fingerprint 改变时，旧 capture 不被接受；
- replacement 后旧 provider instance 的 publish/withdraw/late completion 不能影响 successor；
- 任何 failure 不会被包装成成功 no-op，不会生成伪造 descriptor 或替代 authority；
- registry revision、provider generation、owner generation、session event sequence 不会互换使用。

每个测试记录 `outcome`、`publication/descriptor`、`resource`、`observer/recovery` 四列。C/D 相关项只能记录为未实现/未验证，不得声称通过。

## 8. 验证命令与 memory gate / Verification

本会话每条 cargo 命令前都必须单独执行：

```bash
python qa/browser_live/check_memory.py --self-test
python qa/browser_live/check_memory.py
```

两个命令必须都 exit 0，且 `available_bytes >= 4294967296`；否则停止 cargo 并在报告中写 BLOCKED。记录每个 gate 的 exit、available bytes 和 warnings。

按变更范围串行执行，至少包括：

```bash
cargo fmt --all --check
cargo test -p alephcore --lib capability
cargo test -p alephcore capability --lib
cargo check -p alephcore -p aleph-slot-api --all-targets
cargo clippy -p alephcore --lib --all-targets
```

如果确实修改了共享 capability 契约并影响 Feishu 编译路径，再执行：

```bash
cargo test -p alephcore --lib feishu
cargo test -p alephcore --lib channel_registry
cargo test -p alephcore --lib
```

未运行的命令必须报告为未运行；不得用旧日志替代当前结果。不要为了本阶段运行 `qa/channels/run.sh all` 并暗示 C/D 已关闭；若另行运行，单独报告其 phase、依赖和 BLOCKED/UNRUN 原因。

## 9. 最终交付报告 / Final report

最终报告必须包含：

1. 实际 `HEAD`、branch、status、worktree、历史链和用户已有改动；
2. 读取的 plan、audit、closeout、reference 和源码清单；
3. source-backed B8.2-A 调用图、九类 kind 语义表、descriptor mapping；
4. source-backed B8.2-B 调用图、lease/generation/ownership mapping、stale/replacement/late completion matrix；
5. 唯一事实源表和禁止复制表；
6. A、B 各自的 GO 或 NO-GO/deferred；若失败，给出确切路径、符号、缺失契约和最小前置工作；
7. 修改文件、未修改范围和关键 diff；若无代码修改要明确写出；
8. 每条验证的 memory gate、exit、测试 count、warnings 和 BLOCKED/UNRUN；
9. North Star conformance：disabled/read-side 不产生 runtime effect、失败不拖垮其他 capability、provider/owner 生命周期可组合边界；
10. 明确未做：C/D、Feishu adapter、facade glue、production mount、EffectClaim、Safe Replay、scheduler、external exactly-once、durable Feishu recovery、其他 kind backend、`src/harness/**`；
11. 工作树最终状态。

最终状态只有两种合法表述：

- **A/B GO for a separate future phase**：A/B 契约已证明，但本会话仍未实现 Feishu adapter/mount；
- **A/B NO-GO / deferred**：指定哪一个字段、kind、lease 或 generation 证明失败，并停止扩大范围。

不得输出“Feishu 已接入 Zahir”“production mount ready”或任何由 Tool-only 证据推导出的 Feishu 完成声明。
