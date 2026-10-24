# Aleph Phase 2B B8.2 Prompt

你是 Aleph 模块化 Phase 2B B8.2 的主 Agent / Lead Orchestrator。

工作目录：`.`。

目标：在已提交的 B8.1 Feishu closeout 之后，先完成 Feishu → Zahir live Capability 的 source-backed go/no-go 审查，再决定是否实现 facade 接线。完整计划见：

`docs/research/aleph_modularization_phase2b_b8_2_plan.md`

## 起点核对

先执行并记录：

```bash
pwd
git rev-parse --show-toplevel
git rev-parse --short HEAD
git status --short --branch
git log --oneline --decorate -8
git worktree list --porcelain
```

确认 B8.1、B7、B6、B5 都在当前 `main` 历史中，工作树干净。不要 reset、checkout 覆盖、pull、push、rebase 或 stash。

## 先做 B8.2.0，只读审查

读取 B8.2 plan、B8.1/B8 记录、B7 reference，以及：

- `src/capability/{descriptor,facade,backend,ownership,effect_claim,projection_host,zahir_facade}.rs`
- `src/capability/provider.rs`
- `src/gateway/channel_registry.rs`
- `src/gateway/interfaces/feishu/**`
- `src/session/{actor,store}.rs`
- `qa/channels/**` 和相关 capability tests

输出 source-backed 调用图，重点回答：

1. descriptor 与现存 `CapabilityKind` 是否有唯一事实源；
2. `ProviderHandle` generation、`BackendLease.owner_generation`、`OwnershipTree` generation 如何绑定；
3. visibility/reference 是否会复制 enabled/lifecycle 状态；
4. callable route 是否经过真实 `ChannelRegistry`、provider admission、generation、revoke 和 drain；
5. observer live invalidation/resync 与 `SessionEventStore` durable recovery 如何分界。

在 B8.2-A 至 B8.2-D 确认前只做读取、调用图、测试设计和文档草案，不改生产实现。

## 决策门

请提出并等待确认：

- **B8.2-A：descriptor/kind**。复用现有 kind 和 descriptor authority，还是 NO-GO/deferred？
- **B8.2-B：lease/generation**。是否能在不新增 owner/generation/disposer map 的前提下绑定 stale/replacement/late completion？
- **B8.2-C：callable/effect**。是否能经过真实 Feishu admission fence？若需要 EffectClaim producer、Safe Replay、scheduler 或 external exactly-once，必须 deferred。
- **B8.2-D：observer/recovery**。是否保持 subscribe-before-snapshot、lag→Invalidated/resync、broadcast live-only、SessionEventStore durable-only？

默认建议：若任一项无法证明，选择 NO-GO/deferred，不强行接入 Zahir。

## 实施约束

只有用户确认 GO 且四项契约均有唯一事实源后，才允许修改代码。允许范围仅为 Feishu slice、现有 Gateway observer 接线、既有 Capability facade/backend/ownership 的必要适配、必要测试和三份 reference 文档。

禁止：

- 第二 callable registry、第二 enabled/actual-state authority、第二 durable store；
- 第二 owner/generation/disposer map；
- 新 Capability kind；
- MCP lazy、Safe Replay、universal scheduler、EffectClaim producer、external exactly-once；
- 修改 `src/harness/`；
- 将静态 in-process provider 描述为 hot unload；
- read-side catalog/describe/resolve/list/doctor/observer 隐式启动或认证 Feishu。

保留 B8.1 已确认边界：ChannelRegistry 是唯一 Gateway lifecycle/config/enabled authority；ProviderHandle 只做 generation/admission fence；api_handle 是 derived cache；accepted Axum handler 的完整独立 ownership/join 仍是结构化 residual。

## 验收

每项记录 `outcome`、`publication`、`resource`、`observer`：

- descriptor snapshot/read-side isolation；
- enabled/disabled publication；
- resolve/lease/callable route；
- replacement/stale lease/late completion；
- revoke/stop/drain；
- observer lag/resync/subscriber restart；
- SessionEventStore durable recovery；
- Feishu real fixture；
- webhook handler residual。

每条 cargo 命令前分别执行：

```bash
python qa/browser_live/check_memory.py --self-test
python qa/browser_live/check_memory.py
```

仅在两个命令都 exit 0 且 `available_bytes >= 4294967296` 时运行 cargo。不要用未运行的命令代替实际结果。真实 QA 继续使用 `qa/channels/run.sh all`；缺少 Python 依赖时如实记录 BLOCKED/UNRUN 或实际使用的本地 fixture 依赖来源。

## 最终报告

必须包含：起点 HEAD/status/worktree、B8.2.0 调用图、B8.2-A 至 B8.2-D、GO/NO-GO 结果、唯一事实源映射、failure matrix 的四语义、每条验证的 memory gate/exit/count/warnings、North Star conformance、残余风险、未做事项和工作树状态。

不要自动 commit、push、reset、rebase、pull 或 stash；除非用户明确要求。
