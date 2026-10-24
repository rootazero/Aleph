# Aleph Phase 2B B7 实施 Prompt

你是 Aleph 模块化 Phase 2B B7 的主 Agent / Lead Orchestrator。

工作目录：`.`。

目标：在已经合并 B5/B6 的干净 `main` 上，把同一 lifecycle contract 接入**一个真实 Gateway capability provider vertical slice**。完整契约见：

`docs/research/aleph_modularization_phase2b_b7_plan.md`

## 起点核对

先执行并记录：

```bash
pwd
git rev-parse --show-toplevel
git rev-parse --short HEAD
git status --short --branch
git log --oneline --decorate -6
```

B5/B6 必须已经在当前 `main` 的提交历史中。不要 reset、checkout 覆盖、pull、push、rebase 或 stash。

## 先做 B7.0，不要直接写生产代码

按 B7 plan 读取：

- `docs/research/aleph_modularization_phase2b_b7_plan.md`
- `docs/research/aleph_modularization_phase2b_b6_plan.md`
- `docs/reference/ARCHITECTURE.md`
- `docs/reference/TOOL_SYSTEM.md`
- `docs/reference/FEATURE_LOCATOR.md`
- `src/capability/provider.rs`
- `src/capability/facade.rs`
- `src/capability/zahir_facade.rs`
- `src/capability/ownership.rs`
- `src/capability/effect_claim.rs`

然后 source-backed 审计所有候选 Gateway/channel/terminal/memory provider 的启动、停止、调用、authority、observer 和 durable state 入口。

输出并等待用户确认以下决策：

1. D-A：只选哪个 provider vertical slice，为什么有真实 fixture 和可观察 effect。
2. D-B：in-process static 还是已有 Bridge boundary；没有现成边界时不得偷偷引入 sidecar/dylib/subprocess。
3. D-C：per-provider fail-closed 或 fail-fast；不能静默改变 B6 的聚合语义。
4. D-D：canonical authority、publication、observer snapshot、durable recovery 各自的唯一事实源。

在 D-A 至 D-D 获得确认前，只做读取、调用图和计划记录，不改生产实现。

## 实施约束

确认后只实现一个 provider：

- disabled 不得 activation、publication、callable route、provider `Ready` 或 retained runtime effect。
- enabled 只能通过真实 boot 或显式 enable 激活。
- 复用已有 `ProviderLifecycle`、owner/generation、authority、lease、scope、publication、drain/dispose 和 stale fencing。
- 读侧 catalog/describe/resolve/list/doctor/observer 不得隐式启动。
- disable 先撤权，再 drain/join，再 dispose；失败保留结构化 residual。
- replacement 和 late completion 必须 fail-closed，旧 handle 不得撤回新 entry。
- observer lag 只能 invalidation/resync，不得把 broadcast 当 durable recovery。

## 禁止事项

- 不改 `src/harness/`。
- 不建第二 callable registry、第二 enabled 判定、第二 durable store、第二 actual-state authority 或独立 disposer map。
- 不接入第二个 Capability kind。
- 不偷渡 MCP lazy、Safe Replay、universal scheduler、EffectClaim producer、external exactly-once。
- 不把静态链接 Rust 模块描述成可热卸载或代码页未加载。
- 不自动 commit、push、reset、rebase、pull 或 stash。除非用户明确要求，保持修改未提交。

## 验收

使用真实 provider 和真实 boundary/fixture。每项都断言 outcome、publication、resource、observer：

- disabled startup
- enabled startup
- read-side isolation
- boot/enable race
- disable race with in-flight operation
- replacement/stale generation/late completion
- provider failure cleanup
- observer lag/resync

每条 cargo 命令前单独执行：

```bash
python qa/browser_live/check_memory.py --self-test
python qa/browser_live/check_memory.py
```

只在 self-test exit 0、probe exit 0 且 `available_bytes >= 4294967296` 时运行 cargo；cargo 串行。至少验证格式、选中 provider 定向测试、`cargo test -p alephcore --lib`、`cargo check -p alephcore -p aleph-slot-api --all-targets` 和 `cargo clippy -p alephcore --lib --all-targets`。缺失 fixture/submodule/资源必须如实标记 BLOCKED 或 UNRUN。

最终报告必须包含：起点 HEAD/status、B7.0 调用图和 D-A 至 D-D、修改文件和真实调用链、failure matrix 四语义、每条验证的 memory gate/exit/count/warnings、North Star conformance、残余风险、明确未做事项，以及未提交状态。
