# Aleph Phase 2B B8.2 实施计划
# Implementation Plan — Feishu → Zahir Live Capability

## 0. 计划状态与核心决策

本计划承接 B8.2.0 source-backed audit。当前审计结论为：

- B8.2-A descriptor/kind：NO-GO / deferred；
- B8.2-B lease/generation/replacement：NO-GO / deferred；
- B8.2-C callable/effect：NO-GO / deferred；
- B8.2-D observer/recovery：NO-GO / deferred。

因此实施顺序不是先写 `FeishuCapabilityAdapter`，而是先关闭四个契约缺口，再重新运行四项决策门。任何一项仍不能证明，停止在该阶段，不通过增加 facade 胶水或复制状态绕过决策。

本计划的推荐架构是：

1. `ChannelRegistry` 继续是 Gateway config、enabled、desired state、actual lifecycle 的唯一 authority；
2. `ProviderHandle` 继续只负责 provider state、generation 和 admission fence；
3. `OwnershipTree` 继续是 owner generation、visibility、lifetime 的唯一 authority；
4. `ToolHandlerRegistry` 继续是 Tool callable registry；Feishu 不建立第二 callable registry；
5. `api_handle` 继续是 Feishu derived cache，不得成为 authority；
6. Capability facade/backend 必须通过现有 kind 和通用契约表达 Feishu。不能在现有九个 kind 中找到语义匹配，或必须新增 route/effect 事实源时，保持 deferred；不得新增 CapabilityKind；
7. broadcast 只负责 live notification；durable recovery 只使用已经存在的 durable authority。不能证明 Feishu provider lifecycle 可以由现有 durable authority 表达时，保留为 deferred。

## 1. 交付边界

### 1.1 允许修改

- `src/gateway/interfaces/feishu/**`；
- `src/gateway/channel_registry.rs` 及必要的 Gateway handler/observer 接线；
- `src/capability/{descriptor,facade,backend,ownership,projection_host,zahir_facade}.rs` 的必要通用化；
- 必要的 Feishu、Gateway、Capability 测试和 `qa/channels/**` fixture；
- `docs/reference/{FEATURE_LOCATOR,ARCHITECTURE,TOOL_SYSTEM}.md`；
- 本计划和实施记录。

### 1.2 明确禁止

- 新增 `CapabilityKind`；
- 第二 callable registry；
- 第二 enabled/actual-state authority；
- 第二 owner/generation/disposer map；
- 第二 durable store；
- MCP lazy、Safe Replay、universal scheduler；
- EffectClaim producer、external exactly-once；
- 把静态 in-process Rust provider 描述为 hot unload；
- 修改 `src/harness/`；
- 让 catalog、describe、resolve、list、doctor 或 observer 隐式启动、连接或认证 Feishu；
- 把 `api_handle` 或 broadcast 当作 authority 或 durable recovery source。

### 1.3 实施完成定义

只有以下条件全部满足，才可以声明 B8.2 完成：

- 四项决策门全部 GO；
- Feishu 的 descriptor、reference、visibility、owner generation、lease、callable route、effect boundary 和 observer 均能从唯一事实源推导；
- disabled、start、stop、replacement、stale lease、late completion、provider failure、observer lag/resync 都有真实 authority 和可复现测试；
- read-side 不产生 Feishu runtime effect；
- 真实 `qa/channels/run.sh all` 的可运行阶段通过，缺失依赖则以 BLOCKED/UNRUN 交付；
- 文档与代码中的“已接入”“已支持”“deferred”状态一致。

## 2. 阶段顺序与决策门

### Phase 0 — Baseline、事实冻结与文档修正

目标：让实现会话从真实状态开始，并消除旧文档对架构决策的误导。

步骤：

1. 记录 `HEAD`、branch、status、worktree、B5/B6/B7/B8 历史；不 reset、pull、push、rebase、stash。
2. 复核以下事实：
   - `ZahirFacade` 已有生产 Tool mount，但 Feishu 不在 mount surface；
   - `ChannelRegistry::register` replacement 不停止旧实例；
   - `api_handle::publish` 没有与 `withdraw` 对称的 instance/generation fence；
   - Feishu config 的 `enabled=false` 没有进入唯一 Gateway enabled 判定；
   - webhook stop 只保证 enqueue quiescence，不保证 accepted handler response join；
   - Feishu 没有 provider lifecycle durable producer。
3. 修正文档中的 stale claim：生产 `ZahirFacade::new` 已存在、`ChannelRegistry` 没有 subscribe-before-snapshot、Feishu 仍是 provider lifecycle slice。
4. 建立本阶段的 failure matrix，四列固定为 `outcome`、`publication`、`resource`、`observer`。

验收：

- 只修改 reference/record 文档；
- 没有生产代码变化；
- 事实引用均带 `path:line`；
- 若代码与旧文档冲突，以当前源代码为准并记录冲突。

### Phase 1 — Gateway authority 与 Feishu lifecycle closure

目标：先使 Feishu provider slice 的 enabled、replacement、publication、stop 边界可证明；此阶段不接 Zahir。

#### 1A. Enabled authority

- 从现有 Gateway config 解析显式 enabled，并由 `ChannelRegistry` 做唯一判定；
- 删除或禁止 Feishu 内部另行解释 enabled 的路径；`FeishuConfig::is_account_enabled` 只能作为纯配置 helper，不能成为第二 authority；
- disabled channel 不创建 token manager、API client、webhook listener、WS runtime、emitter cache 或 callable publication；
- boot 和显式 enable 必须走同一个 registry/lifecycle owner；
- read-side status/list/describe 不得触发 start 或认证。

测试：

- disabled boot：无 Feishu registration、无 token request、无 listener bind、无 `api_handle` entry；
- enabled boot：仅在显式 enabled 时创建 runtime；
- boot/enable race：disable 先于 Ready commit 时不产生 publication；enable 只生成一个 owner/generation；
- read-side isolation：list/status/doctor/catalog 不产生网络请求和 runtime task。

#### 1B. Replacement 与 publication fence

- replacement 必须先封闭旧实例 admission，再等待约定的 drain/join 边界，最后移除旧 instance；
- 不在 `channels` lock 跨 await；明确 stop、registry lock、forwarder Arc 的锁序；
- 旧 Feishu webhook/WS task 必须可观察地停止，或保留明确的 structured residual；不能只卸载共享 `WebhookMountTable`；
- `api_handle::publish` 必须验证 provider instance 和 generation，不能让旧 late completion 覆盖新 entry；
- `withdraw`、`publish`、`get` 的 fence 推导必须来自同一 provider/owner generation，不得添加第三张 map。

测试：

- replacement 前后旧 listener 不再占端口；
- 旧 generation 的 late start、publish、withdraw 不影响继任者；
- 旧 handle 在 revoke/stop/replacement 后 fail-closed；
- provider failure 保留真实 Failed/residual，不报 false Stopped。

#### 1C. Webhook handler lifecycle

- `revoke` 必须先于 drain/join/dispose；
- 区分顶层 accept task、accepted handler、已准入 API call 三个生命周期；
- `FeishuApi::drain()` 的证明范围只写为 admission/enqueue quiescence；
- 如果不能在现有 runtime 中安全 join response tail，建立结构化 residual 和测试证据，禁止把 abort + await 顶层 task 宣称为完整 handler drain。

### Phase 2 — Capability contract closure

目标：在不新增 kind、registry 或 authority 的前提下，确定 Feishu 是否能进入 Zahir。

#### 2A. Kind 与 descriptor

先做决策，不直接编码：

- 选择一个现有 `CapabilityKind`，并说明其语义能否表达 Feishu 的对外能力；
- 如果 `EventSource` / `Subscription` 等现有 kind 只能表达 inbound stream，而不能表达 lifecycle/control/outbound route，必须明确哪些行为仍属于 Gateway control surface；
- 禁止把 Feishu 错误地伪装成 Tool 以复用 `ToolHandlerRegistry`；
- descriptor 必须由一个 adapter 从真实 authority 生成；不能复制 `webhook_path`、port、enabled、generation 或 owner state；
- `describe/list/project` 必须为纯 projection，不创建 Feishu runtime。

必须产出一张契约表：

| 字段 | 唯一来源 | 派生规则 | 读侧副作用 |
|---|---|---|---|
| `CapabilityId` | ChannelRegistry/Feishu channel identity | stable channel/provider identity | none |
| `kind` | 选定的既有 kind | 固定、不可由配置任意改变 | none |
| `revision` | owning registry | 每次成功 mutation 递增 | none |
| `visibility` | OwnershipTree | 从 owner scope 派生 | none |
| `owner` | OwnershipTree | `OwnerRef` 派生 | none |
| route metadata | 仍待决策 | 不允许第二份 route authority | none |
| fingerprint | descriptor canonical form | `to_descriptor` 同一算法 | none |

若不能完成这张表，B8.2-A 仍为 NO-GO。

#### 2B. Lease、generation 与 ownership

- 使 lease proof 与 capability kind 无关，或证明现有 lease 形状确实能表达 Feishu；
- 不让 `BackendLease` 永久绑定 `CanonicalEntryLease`，除非 Feishu 被证明语义上就是 Tool；
- lease 必须同时绑定：capability identity、provider instance/generation、owner generation、visibility/reference；
- `OwnershipTree::generation()` 是 owner generation 唯一来源；Feishu 不维护本地 generation map；
- revoke、stop、replacement、stale lease、late completion 的每条拒绝路径必须 fail-closed；
- `resolve()` 只读取 publication snapshot 和 authority，不启动或认证 Feishu。

验收：

- old lease 无法调用新 generation；
- old completion 无法撤销新 publication；
- owner revoke 后旧 lease 不能被重新包装为有效 lease；
- visibility/reference 改变不会复制 enabled/lifecycle state。

### Phase 3 — Callable route 与 effect boundary

目标：证明 Capability callable route 不绕过 Gateway admission；同时明确不实现被禁止的 effect machinery。

#### 3A. Read-side

- catalog、describe、list、resolve、project、doctor 只读取 descriptor/publication/ownership snapshot；
- 不调用 Feishu token API，不 bind listener，不创建 runtime；
- 任何需要 `api_handle::get()` 的路径必须被视为 effectful，不能放入 read-side。

#### 3B. Outbound callable

- route 必须经过：Capability resolve → lease validation → `ChannelRegistry` actual instance → `ProviderHandle` generation admission → `FeishuApi` admission → outbound call；
- stop/revoke 关闭新调用，drain 只等待已准入工作；
- route 失败必须暴露真实 stale/not-ready/provider failure，不转成成功 no-op；
- 不建立第二 callable registry。

#### 3C. Inbound external effect

- 记录 Feishu webhook 是外部 effect；
- 本阶段不新增 `EffectClaim` producer、Safe Replay、external exactly-once 或 universal scheduler；
- 如果现有 capability contract 无法在不新增这些机制的情况下表达 inbound effect，保持 inbound 为 Gateway-owned event path，并将 B8.2-C 标为 deferred；
- 不把 `SessionEventStore` 的会话内容事件误称为 Feishu provider lifecycle effect claim。

### Phase 4 — Observer 与 recovery closure

目标：建立 live notification 与 durable recovery 的清晰边界。

#### 4A. Live observer

- 先订阅，再取 snapshot；
- `ChannelRegistry` 变更必须产生 revision/epoch 或等价的 invalidation source；
- broadcast lag 只产生 `Invalidated`，receiver 重新从 authority 取 snapshot；
- subscriber restart 不能从 in-memory `api_handle` 或 broadcast 回放历史；
- observer 不启动或认证 Feishu。

#### 4B. Durable recovery

- 不创建第二 durable store；
- 明确 `SessionEventStore` 只恢复 session content，还是允许已有 event contract 表达 provider lifecycle；
- 如果不能由现有 durable authority 表达 Feishu provider lifecycle，则只交付 live observer，provider recovery 保持 deferred；
- 任何文档不得声称 broadcast 或 `api_handle` 是 recovery source。

测试：

- subscribe-before-snapshot race；
- registration/replacement/stop 期间 lag；
- lag 后 invalidation + fresh snapshot；
- subscriber restart；
- durable replay 与 live notification 分离；
- 不发生逐条 broadcast replay 假象。

### Phase 5 — B8.2 re-gate 与有限实现

只有 Phase 1–4 的契约测试和文档都完成后，重新评估 A-D：

- A：descriptor/kind 唯一、可表达、无第二 route authority；
- B：lease/generation/ownership 唯一、旧句柄 fail-closed；
- C：outbound callable 经过真实 fence，inbound effect 边界明确；
- D：live observer 可 invalidation/resync，durable boundary 明确。

若四项全部 GO，才允许实现 Feishu → Zahir adapter 和 production mount。实现顺序必须是：

1. generic contract / backend adapter；
2. authority snapshot 与 descriptor projection；
3. resolve/lease validation；
4. callable route；
5. live observer；
6. production mount；
7. fixture/QA。

任一项重新为 NO-GO，停止在该项，并只更新 reference 文档和审计记录。

## 3. 验证纪律

每条 cargo 命令前都必须独立执行并记录：

```bash
python qa/browser_live/check_memory.py --self-test
python qa/browser_live/check_memory.py
```

只有两个命令都 exit 0 且 `available_bytes >= 4294967296` 才能运行 cargo。cargo 必须串行执行：

```bash
cargo fmt --all --check
cargo test -p alephcore --test feishu_contract_test --test feishu_fixture_test --test feishu_protocol_test
cargo test -p alephcore --lib capability
cargo test -p alephcore --lib feishu
cargo test -p alephcore --lib
cargo check -p alephcore -p aleph-slot-api --all-targets
cargo clippy -p alephcore --lib --all-targets
```

真实 channel 验证：

```bash
qa/channels/run.sh all
```

如果 Python 依赖缺失，记录 `BLOCKED/UNRUN`、错误文本和实际通过的阶段；不得修改依赖或 fixture 来伪造通过。

## 4. 最终交付物

- 更新后的 B8.2 implementation record；
- source-backed 调用图和唯一事实源表；
- A-D re-gate 结果；
- 四语义 failure matrix：`outcome` / `publication` / `resource` / `observer`；
- North Star conformance；
- 每条验证的 memory gate、exit、count、warnings；
- 修改文件和未修改范围；
- 残余风险、BLOCKED/UNRUN 项、未做事项；
- 明确是否允许进入 production mount。

不得自动 commit、push、pull、reset、rebase 或 stash。
