# Capability Phase 4 H-pre：生产挂载与持续订阅设计

- **状态 / Status:** Written spec approved by user; implementation-plan review and execution-method selection pending
- **基点 / Baseline:** `947e1895130bfc84abca7f1dd7733b07e30519fb`
- **范围 / Scope:** Gate H-pre only; Gate H ACP inbound server and Gate I implementation are excluded
- **工作树 / Worktree:** `/Volumes/TBU4/Workspace/Aleph-capability-phase4-follow-up`

## 1. 目的与成功标准 / Purpose and success criteria

本 minislice 将已经 library-only 的 capability host 接入真实 Aleph runtime，并让至少一个真实 production projection 消费初始 snapshot 与持续变更。它不是 ACP server，不是 Safe Replay，也不是把 session durable event store 改造成 capability live source。

This minislice mounts the existing library-only capability host into the real Aleph runtime and gives at least one real production projection an initial snapshot plus continuous capability updates. It is not the ACP server, Safe Replay, or a conversion of the durable session event store into the live capability source.

成功标准：

1. 生产启动路径持有同一个 `Arc<ToolHandlerRegistry>` 与 `Arc<OwnershipTree>`；不复制 registry、descriptor、owner generation 或 handler map。
2. production projection attach 后先收到初始 snapshot 与 registry cursor，再接收 post-subscribe registry changes。
3. owner `bump`、`revoke`、`dispose` 能使真实 consumer 收到 invalidation 与 replacement snapshot；不能用 boot logger、GlobalBus 或 test consumer 代替。
4. registry lag / projection buffer overflow 通过 replacement snapshot + cursor 修复，不伪造缺失 delta。
5. attach、cancel、close 都有真实 completion/drain 证明；取消请求本身不被当作 quiescence。
6. registry cursor、owner generation、session event seq 保持三个独立域。
7. canonical tool dispatch、approval path、session durable event path 不改变。

## 2. 已核验基点 / Verified baseline

### 2.1 生产组装与 projection

- 唯一生产 `ToolHandlerRegistry` 创建点：`src/bin/aleph-server/commands/start/mod.rs:224`。
- 同一 registry 被安装到 MCP tool service slot（`start/mod.rs:229`）与 markdown-skill registry（`start/mod.rs:236`），并传给 MCP bridge。
- run-loop 在 `src/gateway/execution_engine/run_loop/inner.rs:819-820` 读取 registry snapshot，随后在 `inner.rs:1791` 加入 canonical tools；当前是每请求 snapshot，不是长生命周期 subscriber。
- metadata 最终出口位于 `src/tools/scoped/mod.rs:566`。
- 当前唯一生产 `ToolHandlerRegistry::subscribe()` 消费者为 `start/mod.rs:244-269` 的 boot log loop。它是诊断 tap，不是 capability delivery。

The production registry and projection path are single-source today, but the consumer side is request-scoped. The existing logger is explicitly not a delivery channel.

### 2.2 现有 host 缺口

- `ZahirFacade::new` 尚无生产调用点。
- `ZahirFacade::subscribe` 在 `src/capability/zahir_facade.rs:230-240` 同步返回纯值 `CapabilityChangeStream`；caller cursor 被忽略，不能保存 receiver 或返回 live handle。
- registry broadcast 是 256 槽、可 lag 的通知源。
- `OwnershipTree` 的 `bump/revoke/dispose` 没有局部通知源。
- `OwnershipTree` 是内存状态；其 generation 不是 registry revision，也不是 session seq。

因此，单纯把 `ZahirFacade::subscribe` 接到启动处不足以满足 H-pre；必须有独立的 production projection lifetime 与 ownership invalidation 接缝。

## 3. 设计决策 / Design decision

采用已批准的 **方案 A：共享 runtime host + 显式 ownership invalidation source**。

The approved approach is **Option A: a shared runtime host with an explicit ownership invalidation source**.

### 3.1 共享 authority

启动过程在现有 registry 组装处复用唯一的生产 registry，并创建与其生命周期一致的 `Arc<OwnershipTree>`。production projection host 只持有这两个 authority 的引用；它不得建立第二份 descriptor table、handler registry、owner counter 或 durable capability table。

At the existing boot assembly point, the host reuses the sole production registry and creates one `Arc<OwnershipTree>` with the same runtime lifetime. The production projection host may reference these authorities, but may not create a second descriptor table, handler registry, owner counter, or durable capability table.

### 3.2 Attach 与初始 snapshot

attach 的线性化顺序固定为：

1. 建立 registry receiver；
2. 在同一 host 生命周期内读取 registry/ownership projection snapshot；
3. 记录 snapshot 对应的 registry cursor 与 owner-generation observation；
4. 向真实 consumer 交付 snapshot；
5. drain attach 之后到达的 registry change。

The attach linearization order is fixed: subscribe first, read the combined projection snapshot, record its registry cursor and owner-generation observation, deliver the snapshot, then drain changes that arrived after subscription. A cursor supplied by an older incarnation must not be treated as durable session recovery.

snapshot 的 descriptor visibility 必须复用 `ZahirFacade::describe` 的同一 kind / namespace / visibility predicate；不能因 projection host 另写一份过滤规则而产生第二事实。

### 3.3 Registry change 与 lag recovery

registry change 只代表 live registry mutation，不代表 session event committed。正常 change 以 registry cursor 递增交付。若 receiver 返回 lagged，host 不重放伪造的逐事件 delta，而是：

1. 重新读取当前 projection snapshot；
2. 发送 `Invalidated` 或等价的 scope-wide invalidation；
3. 发送 replacement snapshot 与其 cursor；
4. 恢复正常接收。

replacement buffer 的容量是 production policy，不能把现有 256 槽直接当作所有 queue 的普适上限。验收必须制造超过当前实际 pending capacity 的变更，并验证真实 consumer 收到 replacement snapshot，而非只看到一个 lag warning。

### 3.4 Ownership invalidation

`OwnershipTree` 的 `bump/revoke/dispose` 必须通过独立、局部、可观察的 invalidation source 通知 host。该 source 的职责只有传播 ownership invalidation，不承担 durable session event、approval 或业务逻辑。

- `bump`：标识受影响 lifetime/binding 范围与新的 owner generation。
- `revoke`：标识 capability identity 已不可见且不可重新解析。
- `dispose`：标识受影响 lifetime 范围已退役。
- host 收到事件后重新读取 authority，向 consumer 发送 invalidation + replacement snapshot。
- 若 ownership mutation 与 notification 无法在同一 local linearization boundary 完成，则该 mutation 不能声称 H-pre 已满足；必须停在契约修订，不用 polling 或 GlobalBus 掩盖窗口。

The ownership source is a local invalidation channel only. It is not an approval channel, durable event store, or business-event bus.

### 3.5 生命周期、取消与 quiescence

host 的生命周期属于真实 runtime projection，而非测试 helper：

- attach 返回的控制面必须能发出 cancel/close；
- cancel 后停止接收新输入，并等待正在运行的 delivery 完成或显式终止；
- receiver、task、buffer 均完成 drain 后才报告 closed/quiescent；
- 关闭期间不得把已提交的 snapshot 重新解释为新 incarnation 的 cursor；
- startup/restart 时 generation 与 registry cursor 重新建立 incarnation 边界，不能承诺跨崩溃连续 cursor。

No sleep-based proof is valid. The proof must observe the actual task/receiver completion and verify that a post-close mutation cannot reach the consumer.

## 4. 真实 consumer 选择 / Real consumer selection

首个 consumer 必须是现有 production projection，而不是 logger、unit test 或 GlobalBus tap。已核验的最窄 surface 是 `src/gateway/execution_engine/run_loop/inner.rs:819-820` 到 `src/gateway/execution_engine/run_loop/inner.rs:1791`；但是它目前是每请求 snapshot，因此不能直接冒充 long-lived subscriber。

The first consumer must be an existing production projection, not a logger, unit test, or GlobalBus tap. The narrowest observed surface is the run-loop snapshot-to-canonical-tools path, but it is request-scoped and cannot itself be presented as a long-lived subscription.

本设计因此要求：

1. host 持有 long-lived subscription；
2. 真实 projection 从 host 读取最新 snapshot/replacement state；
3. per-request `LoopToolRegistry` 仍是请求内 materialization，不成为第二 authority；
4. `McpRegistryTool`、approval、handler invocation 与现有 canonical dispatch 不改；
5. 若具体 surface 无法持有 host 生命周期，必须在 plan 中改选另一个已存在的 production projection，并记录理由，不以新增 test consumer 充数。

该选择在 implementation plan 中要绑定到一个实际已有的 production caller path；本 spec 不把尚未验证的 API 名称当作既有事实。

## 5. 数据域与不变量 / Data domains and invariants

| 域 | 来源 | 用途 | 禁止混用 |
|---|---|---|---|
| Registry cursor | `ToolHandlerRegistry` revision/broadcast | live capability changes、lag replacement | 不作为 owner generation 或 session recovery seq |
| Owner generation | `OwnershipTree` per-binding generation | lease invalidation、owner replacement | 不作为 registry revision |
| Session event seq | `SessionEventStore` committed events | durable session observation/recovery | 不作为 live registry notification |

Additional invariants:

- `Zahir::resolve` 只产生带 owner generation 的 lease；dispatch 仍通过原 canonical path。
- owner invalidation 不能被描述为 handler invocation 成功。
- live registry notification 不能被描述为 durable event commit。
- retired incarnation 的 stale cursor 不能恢复新 incarnation。
- projection buffer overflow 必须 fail closed 到 replacement snapshot，而不是 silently continue。

## 6. 错误处理 / Error handling

- registry receiver closed：停止 host，完成 close/drain，并报告 closed；不得继续消费旧 snapshot。
- registry lagged：replacement snapshot + invalidation；不得合成中间事件。
- ownership source closed或无法读取 authority：consumer 进入 invalidated/closed fail-closed 状态，不继续使用旧 lease。
- snapshot 读取失败：不发送部分 projection；保留旧状态仅到明确的 close boundary，不宣称新 cursor 已提交。
- projection delivery error：隔离该 consumer，等待其 delivery future 结束；不阻塞 registry publisher，不重试业务 handler。
- cancel 与 mutation 竞态：以 host 定义的线性化点判定，关闭完成后来的 mutation 不得到达已关闭 consumer。

## 7. 验证设计 / Verification design

实现获批后，至少需要以下真实验证；本 spec 阶段不运行这些命令：

1. **Initial attach**：attach 前注册、attach 后注册、attach 与 mutation 竞态；证明 snapshot 不漏 post-subscribe change。
2. **Registry replacement**：超过实际 pending capacity 的变更；证明真实 consumer 收到 replacement snapshot + cursor。
3. **Ownership invalidation**：分别验证 bump、revoke、dispose；检查 lease/visibility 失效和 replacement snapshot 到达。
4. **Domain separation**：断言 registry cursor、owner generation、session seq 不互相推进或恢复。
5. **Lifecycle**：attach、cancel、close、drop；证明 delivery task 完成，且 close 后 mutation 不到达 consumer。
6. **Incarnation**：重建 host 后旧 cursor 不被当成 durable recovery；新 snapshot 建立新边界。
7. **Negative controls**：GlobalBus-only consumer、boot logger、test-only receiver 不得计为 production delivery。
8. **Canonical path**：projection 变化只影响可见 surface；真实 invoke 仍走原 handler/approval/session path。

按既有工程纪律，implementation 后使用 scoped cargo check/test/clippy，再执行要求的 fresh validation；历史 baseline failure 不与新 gate failure 混合，也不以 passed 数量替代 failure identity/message 差异。

## 8. Gate I 边界 / Gate I boundary

本设计不实施 I1 或 I2。I0 audit 的结论作为后续设计输入：

- `VerifyOnly` 是现有无字段判定，不新增 `VerifyOnly::effective_input`。
- `ToolCallEffectiveInput` 是 post-dispatch 事后收据，不能证明 dispatch 前 intent，也不能关闭 handler 已执行但结果/marker 未持久化的 crash window。
- `resume_coordinator` 当前把 `effective_input` 设为 `None`，因此 replay preparer fail-closed；本期不把它改成自动重放。
- claim budget、owner fence 与 durable append 若不能处于同一原子边界，I1 必须停止并先做最小契约设计。
- Safe Replay 必须另立 spec + plan + approval；本 H-pre spec 不授权其实现。

## 9. 非目标 / Non-goals

- ACP inbound server、ACP wire conformance、stdio transport 选择。
- Safe Replay、automatic replay、external-effect exactly-once 或 provider idempotency 承诺。
- 其它 capability kind backend。
- universal durable scheduler 或 harness 重构。
- 将 GlobalBus、boot log、MCP test server 当作 production capability delivery。
- 将 registry notification 写成 session committed event。
- 无关历史 clippy/test failure 的修复。
- main merge、push 或跨 worktree 自动 cherry-pick。

## 10. 交付物 / Deliverables

在本 spec 获批并形成 plan 后，H-pre implementation 才可产生：

1. 生产 runtime mount 与共享 authority wiring；
2. production projection host 及 ownership invalidation source；
3. attach/snapshot/change/lag/replacement/close 行为测试；
4. 真实 caller projection QA 日志与 failure identity；
5. `FEATURE_LOCATOR` 对实际 consumer path/status/deferred 的更新；
6. H-pre scoped review、negative report 与未覆盖边界。

本 spec 本身不授权编码，也不授权 Safe Replay。
