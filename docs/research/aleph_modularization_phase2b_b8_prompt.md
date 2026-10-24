# Aleph Phase 2B B8 Prompt

你是 Aleph 模块化 Phase 2B B8 的主 Agent / Lead Orchestrator。

工作目录：`.`。

目标：在 B7 Feishu provider slice 已合并到干净 `main` 后，先关闭 B7 的 disabled、boot/enable race、observer lag/resync 和 webhook handler lifecycle 验收缺口，再决定是否把 Feishu 接入 Zahir live Capability。完整计划见：

`docs/research/aleph_modularization_phase2b_b8_plan.md`

## 起点核对

先执行并记录：

```bash
pwd
git rev-parse --show-toplevel
git rev-parse --short HEAD
git status --short --branch
git log --oneline --decorate -8
```

确认 B7、B6、B5 均在当前 `main` 历史中，工作树干净。不要 reset、checkout 覆盖、pull、push、rebase 或 stash。

## 先做 B8.0，不要直接写生产代码

读取 B8 plan/prompt、B7 plan、FEATURE_LOCATOR §3.5g、ARCHITECTURE、TOOL_SYSTEM、`src/capability/provider.rs`、`facade.rs`、`zahir_facade.rs`、`src/gateway/channel_registry.rs`、Feishu source 和 `qa/channels/*`。

source-backed 审计：

- disabled/config/ChannelRegistry 的唯一判定和启动 owner；
- Feishu boot、explicit control、stop、replacement、observer 和 durable recovery；
- webhook accept task 与已接受 handler 的真实 ownership；
- `ChannelRegistry` observer 的 subscribe-before-snapshot、lag invalidation/resync；
- `SessionEventStore` durable recovery；
- Feishu 若接入 Zahir 所需的 descriptor、lease、visibility、owner generation、effect scope 和 callable route。

输出并等待用户确认：

1. B8-A：先做 B7 closeout，还是进入 Zahir facade；默认先 closeout。
2. B8-B：handler 是否能通过现有 runtime 做完整 graceful join；不能就定义结构化 residual 和证据边界。
3. B8-C：disabled/enable 的 canonical authority、publication、generation、observer snapshot 各自唯一事实源。
4. B8-D：observer lag/resync 与 durable recovery 的分界。

在 B8-A 至 B8-D 确认前只做读取、调用图、测试设计和计划记录，不改生产实现。

## 实施约束

确认后：

- 只修改 Feishu slice、现有 Gateway observer 接线、必要测试和 reference 文档；
- 不改 `src/harness/`；
- 不创建第二 callable registry、第二 enabled 判定、第二 durable store、第二 actual-state authority 或独立 disposer map；
- 不接入第二 Capability kind；
- 不偷渡 MCP lazy、Safe Replay、universal scheduler、EffectClaim producer 或 external exactly-once；
- disabled 不得 activation、publication、callable route、provider `Ready` 或 retained runtime effect；
- read-side catalog/describe/resolve/list/doctor/observer 不得隐式启动；
- revoke 必须先于 drain/join/dispose；
- replacement、stale generation、late completion 必须 fail-closed，旧代不能撤回继任者；
- observer lag 只能 invalidation/resync，broadcast 不能作为 durable recovery；
- 不把静态 in-process Rust provider 描述成热卸载。

如果进入 Zahir facade，先证明 descriptor、lease、scope、owner generation、effect claim 和 callable route 没有第二事实源；证明不了就停止在 B7 closeout，并把 deferred 原因写入文档。

## 验收

每项都使用真实 provider、真实 authority 和真实 fixture，同时记录 outcome、publication、resource、observer：

- disabled startup；
- enabled startup；
- read-side isolation；
- boot/enable race；
- disable race with in-flight operation；
- replacement/stale generation/late completion；
- provider failure cleanup；
- observer lag/resync；
- accepted webhook handler join 或结构化 residual。

每条 cargo 命令前单独执行：

```bash
python qa/browser_live/check_memory.py --self-test
python qa/browser_live/check_memory.py
```

仅在两个命令都 exit 0 且 `available_bytes >= 4294967296` 时运行 cargo。cargo 串行执行：

```bash
cargo fmt --all --check
cargo test -p alephcore feishu --lib
cargo test -p alephcore --lib
cargo check -p alephcore -p aleph-slot-api --all-targets
cargo clippy -p alephcore --lib --all-targets
```

真实 Feishu QA 使用：

```bash
cargo build --bin aleph-server
KEEP=1 SKIP_BUILD=1 bash qa/channels/run.sh all
```

脚本内 build 前也必须先通过 memory gate。缺少 fixture、submodule、桌面资源或 Python 依赖时如实标记 BLOCKED/UNRUN，不用替代结果掩盖。

## 最终报告

必须包含：起点 HEAD/status、B8.0 调用图、B8-A 至 B8-D、修改文件和真实调用链、failure matrix 的 outcome/publication/resource/observer 四语义、每条验证的 memory gate/exit/count/warnings、North Star conformance、残余风险、未做事项和未提交状态。

不要自动 commit、push、reset、rebase、pull 或 stash；除非用户明确要求。
