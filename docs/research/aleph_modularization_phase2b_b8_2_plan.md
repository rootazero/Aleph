# Aleph Phase 2B B8.2 Plan

## 目标

在 B8.1 已完成 B7 Feishu closeout 的基础上，评估 Feishu 是否可以安全暴露为 Zahir live Capability，并在新的决策门后实现或明确 deferred。

B8.2 的成功标准不是“把 Feishu 挂到 Zahir”，而是证明以下契约只有一个事实源：

- descriptor 与 capability kind；
- lease 与 provider generation；
- visibility scope 与 reference；
- owner generation 与 replacement/revocation；
- effect scope / effect claim；
- callable route 与实际 Feishu admission fence；
- live observer 与 durable recovery。

如果任一契约需要在 Zahir、Gateway、Feishu 或 projection 层复制，停止在 go/no-go 审查，不实现 facade 接线。

## 起点与边界

新会话从已提交 B8.1 closeout 的干净 `main` 开始，先记录：

```bash
pwd
git rev-parse --show-toplevel
git rev-parse --short HEAD
git status --short --branch
git log --oneline --decorate -8
git worktree list --porcelain
```

确认 B8.1、B7、B6、B5 都在当前历史中。不得 reset、pull、push、rebase、stash 或覆盖用户改动。

允许读取：

- `src/capability/{descriptor,facade,backend,ownership,effect_claim,projection_host,zahir_facade}.rs`；
- `src/capability/provider.rs`；
- `src/gateway/channel_registry.rs`；
- `src/gateway/interfaces/feishu/**`；
- `src/session/{actor,store}.rs`；
- `docs/reference/{FEATURE_LOCATOR,ARCHITECTURE,TOOL_SYSTEM}.md`；
- B8.1 的测试、fixture 与 reference 文档。

未经 B8.2-A 至 B8.2-D 确认，不修改生产实现；只做调用图、契约映射、测试设计和文档草案。

## B8.2 只读决策门

### B8.2-A：descriptor 与 kind

确定 Feishu 对外暴露的能力是否已经有现存 `CapabilityKind` 和 descriptor 形状可复用。

必须回答：

- Feishu 暴露的是现有 Tool capability、provider capability，还是当前模型无法表达的能力；
- descriptor 的 `id`、kind、metadata、scope、visibility 和 fingerprint 由谁派生；
- 是否会出现第二份 Feishu descriptor 或第二个 callable registry；
- `describe/list/project` 是否仍是 projection/read-side，不隐式启动或认证 Feishu。

无法复用现有 kind 或无法指出唯一 descriptor authority 时，结论为 deferred。

### B8.2-B：lease、generation 与 replacement

把 `ProviderHandle` 的 generation、`BackendLease.owner_generation`、`OwnershipTree` generation 和 descriptor/reference 之间的关系画成 source-backed 调用图。

必须证明：

- Zahir resolve 得到的 lease 绑定当前 provider instance 和 owner generation；
- revoke、stop、replacement、stale lease、late completion 都 fail-closed；
- 旧 generation 不能撤回继任者；
- 不新增独立 owner map、generation map 或 disposer map；
- capability visibility 与 provider lifecycle 不产生第二个 enabled 判定。

无法形成单一 generation 推导时，禁止 facade 实现。

### B8.2-C：callable route 与 effect boundary

检查 Feishu callable route 是否能直接经过现有 admission fence，并区分：

- catalog/describe/resolve/list/doctor 等 read-side projection；
- Feishu outbound call、inbound admission、runtime stop 等 effectful route；
- EffectClaim producer、external exactly-once、Safe Replay 和 durable scheduler。

必须证明 callable route 不绕过 `ChannelRegistry`、`ProviderHandle`、generation、revoke 或 `FeishuApi::drain()`。

如果需要新增 EffectClaim producer 或 external exactly-once 才能完成接线，本阶段明确 deferred，不偷渡实现。

### B8.2-D：observer 与 durable recovery

确认 Zahir live subscription 与 Gateway/Feishu observer 的边界：

- receiver 必须先于 snapshot；
- broadcast 只提供 live notification；
- lag/head advancement 只产生 fresh snapshot + `Invalidated`；
- `SessionEventStore` 仍是 durable recovery 的唯一事实源；
- observer 不从 in-memory Feishu cache 或 broadcast 恢复 durable state。

必须给出 replacement、lag、resync、subscriber restart 和 durable replay 的验收矩阵。

## Go / No-Go 标准

只有同时满足以下条件才允许 B8.2-E 实现：

1. descriptor/kind 有唯一事实源；
2. lease、provider generation 和 owner generation 可以无复制地绑定；
3. visibility/reference 不复制 enabled 或 lifecycle 状态；
4. callable route 经过真实 Feishu admission fence；
5. observer 与 durable recovery 严格分界；
6. 没有引入第二 capability kind、registry、authority、disposer map、EffectClaim producer 或 external exactly-once；
7. 有真实 provider、真实 authority、真实 fixture 的 replacement/stale/lag/resync 证据设计。

任一项无法证明，结论为 **NO-GO / deferred**，并将缺口写入三份 reference 文档，不修改 `src/harness/`。

## 若获准实现的范围

仅允许修改：

- Feishu slice；
- 现有 Gateway observer 接线；
- 既有 Capability facade/backend/ownership 的必要适配；
- 必要测试；
- `ARCHITECTURE.md`、`FEATURE_LOCATOR.md`、`TOOL_SYSTEM.md`。

禁止：

- 第二 callable registry；
- 第二 enabled/actual-state authority；
- 第二 durable store；
- 第二 owner/generation/disposer map；
- 新 Capability kind；
- MCP lazy、Safe Replay、universal scheduler、EffectClaim producer、external exactly-once；
- `src/harness/` 修改；
- 把静态 in-process provider 描述成 hot unload。

## 验收矩阵

每项记录 `outcome`、`publication`、`resource`、`observer` 四个语义：

- descriptor snapshot 与 read-side isolation；
- enabled/disabled publication；
- resolve → lease → callable route；
- replacement 与 stale lease；
- revoke/stop 与 late completion；
- observer subscribe-before-snapshot；
- lag → Invalidated/resync；
- subscriber restart；
- SessionEventStore durable recovery；
- Feishu webhook/WS admission 与 outbound drain；
- handler graceful shutdown residual。

每条 cargo 命令前分别运行：

```bash
python qa/browser_live/check_memory.py --self-test
python qa/browser_live/check_memory.py
```

只有两个命令都 exit 0 且 `available_bytes >= 4294967296` 时才运行 cargo。保持串行验证：

```bash
cargo fmt --all --check
cargo test -p alephcore capability --lib
cargo test -p alephcore feishu --lib
cargo test -p alephcore --lib
cargo check -p alephcore -p aleph-slot-api --all-targets
cargo clippy -p alephcore --lib --all-targets
```

真实 Feishu fixture 继续使用 `qa/channels/run.sh all`；Python `websockets` 缺失时记录实际环境和依赖来源，不修改仓库掩盖问题。

## 交付物

- B8.2 source-backed descriptor/lease/route/observer 调用图；
- B8.2-A 至 B8.2-D 决策；
- 明确 GO 或 NO-GO/deferred 结果；
- 若 GO，先提交实现前的契约映射与测试计划；
- 若 NO-GO，更新三份 reference 文档并列出下一步所需的最小前置工作；
- 起点 HEAD/status、修改文件、验证命令、memory gate、failure matrix、North Star conformance、残余风险和未做事项。
