# Aleph Phase 2B B8.2 实施 Prompt
# Implementation Prompt — Feishu → Zahir Live Capability

你是 Aleph 模块化 Phase 2B B8.2 的主 Agent / Lead Implementer。

工作目录：`.`。

完整实施计划：

`docs/research/aleph_modularization_phase2b_b8_2_implementation_plan.md`

审计输入：

- `docs/research/aleph_modularization_phase2b_b8_2_plan.md`
- `docs/research/aleph_modularization_phase2b_b8_2_prompt.md`
- `docs/research/aleph_modularization_phase2b_b8_audit.md`
- `docs/research/aleph_modularization_phase2b_b8_plan.md`
- `docs/reference/{FEATURE_LOCATOR,ARCHITECTURE,TOOL_SYSTEM}.md`

## 当前事实

上一次 source-backed audit 的实际状态：

- 当前检出预期为 `HEAD=eba5a0349132d618334c7a634b964de014b2d8f7`，但必须重新读取实际 HEAD；
- `main` working tree 应保持 clean；
- B8.2-A descriptor/kind：NO-GO / deferred；
- B8.2-B lease/generation/replacement：NO-GO / deferred；
- B8.2-C callable/effect：NO-GO / deferred；
- B8.2-D observer/recovery：NO-GO / deferred；
- 当前没有 Feishu → Zahir production adapter；
- `ChannelRegistry` 是 Gateway config/enabled/desired lifecycle authority；
- `ProviderHandle` 只负责 provider state、generation 和 admission fence；
- `OwnershipTree` 是 owner generation、visibility、lifetime authority；
- `api_handle` 是 derived cache，不是 authority；
- Tool capability 已有生产 Zahir mount，但 Feishu 不在 Tool projection surface；
- `SessionEventStore` 是 durable session authority；Feishu provider lifecycle 没有 durable producer；
- `qa/channels/run.sh all` phase 1/2 已通过，phase 3 曾因缺少 `websockets` 被 BLOCKED；
- 静态 in-process provider 不得被描述为 hot unload。

不要把这些事实改写成“Feishu 已接入 Capability”。

## 总目标

在不新增 Capability kind、callable registry、enabled/actual-state authority、owner/generation/disposer map、durable store、EffectClaim producer、Safe Replay、universal scheduler 或 external exactly-once 的前提下，完成 Feishu provider slice 的契约闭合，并重新判断它是否可以安全暴露为 Zahir live Capability。

如果任一契约仍无法证明，停止在该门，保留 provider lifecycle slice，更新审计/reference 文档并报告 NO-GO/deferred。不要用 facade 胶水、复制状态或测试专用旁路绕过决策门。

## 执行顺序

### 1. 先核对工作树和文档

执行并记录：

```bash
pwd
git rev-parse --show-toplevel
git rev-parse HEAD
git status --short --branch
git log --oneline --decorate -8
git worktree list --porcelain
```

确认 B5/B6/B7/B8 在当前历史中。不要执行 reset、pull、push、rebase、stash 或覆盖用户改动。

读取实施计划和上述审计输入。先更新实现 todo；每次只保留一个任务为 in progress。

### 2. Phase 1：先修 Gateway lifecycle authority

按实施计划处理以下问题，先不挂 Zahir：

- `[channels.feishu].enabled` 必须进入 `ChannelRegistry` 唯一判定；Feishu 内部不能复制 enabled authority；
- disabled 不得创建 token/API/webhook/WS/emitter runtime 或 publication；
- boot、显式 enable、stop、replacement 使用同一 owner/registry；
- replacement 必须关闭旧 admission，并处理旧 Feishu 独立 webhook/WS task；
- `api_handle::publish` 与 `withdraw` 使用同一 instance/generation fence；旧 late completion 不能覆盖新 publication；
- 明确顶层 accept task、accepted handler、已准入 API call 的生命周期；`FeishuApi::drain()` 只宣称它实际证明的范围；
- stop 不能把 abort 顶层 task 宣称为完整 handler response join；无法完整 join 时留下结构化 residual。

为每个修改增加效果测试，而不是只测试函数是否被调用：

- disabled boot 无 token request、无 listener bind、无 cache entry；
- replacement 后旧端口释放，旧 generation 的 publish/withdraw 不影响新实例；
- stop/revoke 后新调用失败，已准入调用按真实 drain/join 语义结束；
- provider failure 保留真实 Failed/residual。

### 3. Phase 2：关闭 descriptor/kind 与 lease/generation 门

先写契约映射，再写代码。

必须明确：

- Feishu 选择哪个**既有** `CapabilityKind`，以及该 kind 是否能表达它的真实语义；
- 不能把 Feishu 错误伪装成 Tool 以绕过 descriptor/lease 问题；
- descriptor、route metadata、visibility、owner、revision、fingerprint 的唯一来源；
- descriptor 生成不能启动或认证 Feishu；
- `BackendLease` 不得继续以 Tool-only `CanonicalEntryLease` 作为 Feishu 的伪造证明；若需通用化，只能复用现有 facade/backend/ownership 契约；
- lease 必须绑定 capability identity、provider generation、owner generation、visibility/reference；
- `OwnershipTree::generation()` 是 owner generation 唯一来源；
- revoke、stop、replacement、stale lease、late completion 都必须 fail-closed。

如果现有九个 kind 无法在不新增事实源的情况下表达 Feishu，立即停止 B8.2-A/B，不实现 adapter。

### 4. Phase 3：关闭 callable/effect 门

read-side 必须保持纯 projection：

- catalog、describe、list、resolve、project、doctor 不得触发 token、listener、WS 或 Feishu API；
- outbound callable 必须经过：

```text
Capability resolve
  -> lease validation
  -> ChannelRegistry actual instance
  -> ProviderHandle generation admission
  -> FeishuApi admission
  -> outbound Feishu call
```

- stop/revoke 关闭新调用，drain 等待真实已准入工作；
- 不建立第二 callable registry；
- 明确 webhook 是 external effect；本阶段禁止新增 EffectClaim producer、Safe Replay、scheduler 或 external exactly-once；
- 如果 inbound effect 无法在现有契约中安全表达，则保留 Gateway-owned event path，并把 B8.2-C 标为 deferred。

### 5. Phase 4：关闭 observer/recovery 门

- 先 subscribe，再 snapshot；
- ChannelRegistry 的 lifecycle change 必须有 revision/epoch 或等价 invalidation source；
- broadcast lag 只产生 `Invalidated`，receiver 从 authority 重新取 snapshot；
- observer 不从 `api_handle` 或 broadcast 恢复 durable state；
- 不创建第二 durable store；
- 明确 SessionEventStore 只恢复 session content，还是现有 event contract 能表达 provider lifecycle；不能证明时，不声称 Feishu provider recovery 已完成。

测试：

- subscribe-before-snapshot race；
- registration/replacement/stop 期间 lag；
- lag 后 invalidation + fresh snapshot；
- subscriber restart；
- live notification 与 durable replay 分离；
- 没有逐条 broadcast replay 假象。

### 6. 重新运行 A-D 决策门

只有以下条件全部成立，才允许 production Feishu → Zahir mount：

1. descriptor/kind 唯一、可表达；
2. lease/provider generation/owner generation 无复制地绑定；
3. callable route 经过真实 Feishu admission fence；
4. effect boundary 明确且没有偷渡禁止机制；
5. live observer 支持 snapshot/invalidation/resync；
6. durable recovery 边界明确；
7. disabled/read-side/replacement/stale/late completion 有效果级证据。

全部 GO 后，按以下顺序实现：

1. generic capability backend contract；
2. authority snapshot 与 descriptor projection；
3. resolve/lease validation；
4. callable route；
5. live observer；
6. production mount；
7. real fixture and QA。

任何一项重新为 NO-GO，都停止，不扩大 scope。

## 硬约束

禁止：

- 新 Capability kind；
- 第二 callable registry；
- 第二 enabled/actual-state authority；
- 第二 owner/generation/disposer map；
- 第二 durable store；
- MCP lazy、Safe Replay、universal scheduler；
- EffectClaim producer、external exactly-once；
- 修改 `src/harness/`；
- 把静态 in-process provider 描述成 hot unload；
- read-side 隐式启动或认证 Feishu；
- 用 api_handle、broadcast 或测试 fixture 充当 authority；
- 自动 commit、push、pull、reset、rebase 或 stash。

## 验证纪律

每条 cargo 命令前必须单独执行：

```bash
python qa/browser_live/check_memory.py --self-test
python qa/browser_live/check_memory.py
```

只有两个命令都 exit 0 且 `available_bytes >= 4294967296` 才运行 cargo。记录每个 gate 的 exit、available bytes 和 warning；不得用 `/usr/bin/memory_pressure -Q` 替代这两个命令。

按变更范围串行执行：

```bash
cargo fmt --all --check
cargo test -p alephcore --test feishu_contract_test --test feishu_fixture_test --test feishu_protocol_test
cargo test -p alephcore --lib capability
cargo test -p alephcore --lib feishu
cargo test -p alephcore --lib
cargo check -p alephcore -p aleph-slot-api --all-targets
cargo clippy -p alephcore --lib --all-targets
qa/channels/run.sh all
```

`qa/channels/run.sh all` 是真实 channel QA。缺少 `websockets` 或其他 Python 依赖时，报告 `BLOCKED/UNRUN` 和原始错误，不修改仓库来伪造通过。

## 最终报告必须包含

- 实际 HEAD、branch、status、worktree；
- 修改文件和未修改范围；
- source-backed 调用图；
- 唯一事实源映射；
- B8.2-A/B/C/D 的 GO/NO-GO/deferred；
- failure matrix：`outcome`、`publication`、`resource`、`observer`；
- 每条验证的 memory gate、exit、count、warnings；
- North Star conformance；
- residual、BLOCKED/UNRUN 和未做事项；
- 是否具备 production mount 条件。

如果任一门无法证明，最终状态必须明确写为 NO-GO/deferred，并指出具体文件、代码路径、失败语义和下一步最小前置工作。
