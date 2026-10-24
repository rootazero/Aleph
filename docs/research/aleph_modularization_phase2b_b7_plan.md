# Aleph 模块化 Phase 2B — B7 计划
# Phase 2B B7 Plan — Onboarding One Gateway Capability Provider

> 状态：规划稿，基于已合并的 B5/B6。B7 实施必须从 B7.0 source-backed audit 开始；本文件不预先宣称任何 Gateway、channel、terminal 或 memory provider 已接入。

## 0. 前置事实

B5 已冻结 Tool/Plugin lifecycle 的 authority、publication、lease、drain、dispose 和 stale fencing。B6 已将 eligible tool-only WASM 接入真实 `ExtensionManager::ensure_loaded` boot 路径，并验证了 `Cold → Starting → Ready`、canonical publication、retained scope、并发 `ensure_loaded` 和 per-plugin fail-closed。

B7 的目标是把同一 lifecycle contract 复用于**一个**真实 Gateway provider vertical slice。B7 不创建第二套 provider 生命周期、第二个 callable/authority registry 或第二个 enabled/durable state。若选中的 provider 是静态链接的 in-process Rust 模块，只能验证 authority/effect 的撤销和恢复，不能声称代码页未映射或可热卸载。若产品要求 disabled 时连代码都不在进程内，必须另立 sidecar/dylib/subprocess 设计并停止在本计划范围之外。

## 1. North Star

完成一个真实 Gateway provider 的 startup / enable / disable / replacement 验收：

- disabled provider 不进入 activation、publication、callable route、provider `Ready` 或 retained runtime effect；catalog/describe/resolve/observer/doctor 等读侧不隐式启动它。
- enabled 且 eligible provider 才通过真实 boot 或显式 enable 路径进入 `Ready`，并产生可观察的 canonical effect。
- disable / revoke / drain / dispose 后 authority 与 runtime effect 都被撤销；旧 provider generation、lease、binding、handler 和 late completion fail-closed。
- B7 只验收选中的一个 Gateway provider，不声称其他 Capability kind 已接入。

## 2. B7.0 决策门：必须先完成 source-backed audit

在写生产代码前，读取并记录：

1. `src/capability/provider.rs`、`src/capability/facade.rs`、`src/capability/zahir_facade.rs`、`src/capability/ownership.rs`、`src/capability/effect_claim.rs`。
2. 所有候选 Gateway/channel/terminal/memory provider 的注册、启动、停止、调用和 observer 入口。
3. 现有 authority、配置/durable desired state、publication、scope、generation、lease 和 dispose 的唯一事实源。
4. 当前启动入口、读侧入口和测试 fixture；确认是否已有真实 test seam。
5. B6 的 `ensure_loaded`、`boot_guard`、`owner_activation_proof` 与 provider lifecycle 是否可以复用，不能复制成第二份判定。

B7.0 必须产出以下决策，每项写成“选项 / 证据 / 推荐 / 代价”，并在继续编码前由用户确认：

- **D-A：provider vertical slice**：候选 provider 中只选一个，理由必须来自真实代码与可验证 fixture。
- **D-B：execution boundary**：in-process static provider 还是已有 Bridge boundary。没有现成边界时不引入 sidecar/dylib/subprocess 作为 B7 偷渡范围。
- **D-C：boot failure policy**：沿用 per-provider fail-closed，还是需要 fail-fast；默认不能静默改变 B6 的聚合语义。
- **D-D：authority/publication seam**：选中 provider 的 canonical authority、observer snapshot 和 durable recovery 分别由谁拥有；必须证明没有第二份权威。

若 B7.0 发现候选 provider 没有真实 boundary、没有可观察 effect 或需要新增跨 Capability 的架构，则停止并报告证据，不进入实现。

## 3. B7.1 实施范围

在 D-A 至 D-D 确认后，只实现选中的一个 provider：

1. discovery / admission：disabled provider 只进入可观察的 declared state，不创建 runtime effect。
2. boot / explicit enable：复用已有 `ProviderLifecycle`、owner/generation、authority 和 effect scope；保持 activation 在正确 guard 外执行。
3. canonical publication：provider `Ready`、row/status、publication、lease/receipt 和 scope 必须原子地满足同一成功判据。
4. disable：先撤销 callable/publication，再 drain/join，在途操作结束后 dispose；失败时保留结构化 residual，不报 false success。
5. replacement/reload：新 generation 和新 identity 只能在旧 scope 完成规定的 stop/dispose 后接管；旧 handle 和 late completion 不得删除新 entry。
6. observer：subscribe-before-snapshot；lag 只触发 invalidation/resync，不把 live broadcast 当 durable recovery。

## 4. B7.2 验收矩阵

至少覆盖以下真实效果，测试必须使用真实 provider/authority/fixture，不以调用次数代替效果：

- disabled startup：无 provider `Ready`、无 callable/publication、无 retained scope、无 runtime effect。
- enabled startup：provider `Ready`、canonical effect 存在、status 与 observer 一致。
- read-side isolation：catalog/describe/resolve/list/doctor 前后 runtime state 与 publication revision 不变。
- enable/boot race：单一 owner/generation，一次 canonical publication，无 duplicate start。
- disable race：撤权先于 dispose；在途操作完成或返回结构化 stale；无残留 callable entry。
- replacement：旧 generation、lease、binding、late disposer 全部 fail-closed；新 entry 不被旧 completion 撤回。
- provider failure：per-provider failed outcome、结构化 failure、清理结果和 observer 状态一致；其他 provider 不被错误拖垮。
- observer lag/resync：Invalidated 后重新 snapshot，不伪造 durable replay。

每个验收项必须同时记录 outcome、publication、resource、observer 四个语义。

## 5. 禁止事项

- 不修改 `src/harness/`。
- 不创建第二个 callable registry、第二个 enabled 判定、第二个 durable store、第二个 actual-state authority 或独立 disposer map。
- 不把读侧改成隐式 activation，不通过 name/metadata/role/TurnContext 合成 authority。
- 不在 B7 偷渡 MCP lazy、Safe Replay、universal scheduler、EffectClaim producer、external exactly-once 或多个 Capability kind。
- 不把静态链接模块描述成可热卸载或“代码不在内存”。
- 不自动 pull/push/reset/rebase/stash；提交和 merge 由用户另行要求。

## 6. 验证纪律

每条 cargo 命令前独立执行：

```bash
python qa/browser_live/check_memory.py --self-test
python qa/browser_live/check_memory.py
```

只有 self-test 和 memory probe 都成功，且 `available_bytes >= 4294967296` 才运行 cargo。cargo 串行执行。至少根据实际 diff 运行：

- `cargo fmt --all --check`
- 选中 provider 的定向测试
- `cargo test -p alephcore --lib`
- `cargo check -p alephcore -p aleph-slot-api --all-targets`
- `cargo clippy -p alephcore --lib --all-targets`

缺失 fixture、submodule、桌面资源或 workspace 依赖必须记录为 BLOCKED/UNRUN，不得伪称通过。

## 7. 交付物

- B7.0 source-backed 调用图和 D-A/D-B/D-C/D-D 决策记录。
- 选中 Gateway provider 的最小生产实现和真实验收测试。
- lifecycle failure matrix 的 outcome/publication/resource/observer 映射。
- `docs/reference/FEATURE_LOCATOR.md`、`ARCHITECTURE.md`、`TOOL_SYSTEM.md` 的 B7 状态与 deferred 边界更新。
- 最终报告包含起点 HEAD/status、修改文件、真实调用链、每条验证命令的 memory gate/exit/count/warnings、North Star conformance、残余风险、未做事项和未提交状态。
