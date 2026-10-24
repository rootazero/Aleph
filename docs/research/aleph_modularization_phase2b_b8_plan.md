# Aleph Phase 2B B8 Plan

## 目标

在 B7 Feishu provider slice 已合并到 `main` 后，先把 B7 的“DONE_WITH_CONCERNS”收敛为可证明的生命周期闭环，再决定是否把 Feishu 暴露为 Zahir live Capability。B8 不把 Gateway、Terminal、Memory 一次性统一，也不引入第二份 authority。

Success means:

- disabled、enabled、boot/enable race、disable race、replacement、provider failure、observer lag/resync 都有真实 provider、真实 authority 和可复现 fixture 证据；
- webhook stop 的 handler 生命周期有明确的 join 语义，或经过架构审查后形成可接受的结构化 residual；
- disabled/enable 判定仍由现有 Gateway 配置和 `ChannelRegistry` 驱动，没有第二份 enabled state；
- observer snapshot、live invalidation 和 durable recovery 的事实源保持分离；
- 最终明确 Feishu 是否进入 Zahir facade，若不进入则把原因和下一步写入 reference 文档。

## 起点与边界

新会话必须从已合并 B7 的干净 `main` 开始，先记录：

```bash
pwd
git rev-parse --show-toplevel
git rev-parse --short HEAD
git status --short --branch
git log --oneline --decorate -8
```

确认 B7 commit、B6、B5 均在当前历史中。不得 reset、pull、push、rebase 或覆盖用户改动。不得修改 `src/harness/`。

B8 只允许修改 Feishu slice、现有 Gateway observer 接线、必要测试和三份 reference 文档。禁止新增 callable registry、enabled 判定、durable store、actual-state authority、独立 disposer map、第二个 Capability kind、MCP lazy、Safe Replay、universal scheduler、EffectClaim producer 或 external exactly-once。

## B8.0 只读审查与决策

先读取：

- `docs/research/aleph_modularization_phase2b_b8_plan.md`
- `docs/research/aleph_modularization_phase2b_b8_prompt.md`
- `docs/research/aleph_modularization_phase2b_b7_plan.md`
- `docs/reference/FEATURE_LOCATOR.md` §3.5g
- `src/capability/provider.rs`
- `src/capability/facade.rs`
- `src/capability/zahir_facade.rs`
- `src/gateway/channel_registry.rs`
- B7 Feishu source and QA fixture

输出 source-backed 调用图，并在写生产代码前确认：

1. **B8-A，收敛范围**：先完成 B7 closeout，还是直接做 Zahir facade。默认选择先完成 closeout；Zahir 接线必须另有清晰的 descriptor、lease、scope、publication 和 effect claim 契约。
2. **B8-B，handler stop 边界**：使用现有 Gateway/axum 能力跟踪已接受 handler 并 graceful join，还是保留明确 residual。不得把顶层 accept task 的 abort+await 写成完整 handler drain。
3. **B8-C，disabled/enable authority**：disabled 和显式 enable 的唯一判定、启动 owner、generation 和 publication 由谁持有。不得在 Feishu 内部复制配置判定。
4. **B8-D，observer/recovery**：observer snapshot、live lag invalidation/resync、durable recovery 的唯一事实源分别是什么。broadcast 不得承担 durable recovery。

## B8.1 实施顺序

1. **Disabled 与 enable**
   - 从现有配置和 `ChannelRegistry` 派生 disabled 状态。
   - disabled 不创建 runtime effect、不 publication、不 callable route、不进入 `Ready`。
   - 真实 boot 和显式 enable 只能走既有 owner/registry 入口。
   - 增加 disabled startup 与 boot/enable race 的真实 fixture，记录 outcome、publication、resource、observer 四个语义。

2. **Webhook handler 生命周期**
   - 优先复用现有 task ownership，补 accepted handler 的显式跟踪和 stop 时 graceful shutdown。
   - 让 revoke 先阻止新 admission，再等待已准入入站的 enqueue/send，最后 dispose。
   - 若运行时无法安全提供完整 handler join，保留结构化 residual，并增加测试证明入队静止点、response tail 和资源清理边界。

3. **Observer**
   - 复用 `ChannelRegistry` 的 subscribe-before-snapshot 语义。
   - 用真实 Feishu registration/replacement 触发 observer lag，断言只产生 invalidation/resync，不伪造逐条 durable replay。
   - durable recovery 只验证 `SessionEventStore`，不从 broadcast 恢复历史。

4. **Zahir go/no-go**
   - 只有当 D-D 明确了 Feishu descriptor、lease、visibility、owner generation、effect scope 和 callable route 的单一事实源，才实现 facade 接线。
   - 如果这些契约仍然缺失，保持 Feishu 为 provider lifecycle slice，并在文档中明确 deferred。

## 验收

每个验收项必须同时记录 outcome、publication、resource、observer：

- disabled startup；
- enabled startup；
- read-side isolation；
- boot/enable race；
- disable race with in-flight operation；
- replacement/stale generation/late completion；
- provider failure cleanup；
- observer lag/resync；
- webhook stop 的 accepted-handler join 或结构化 residual。

每条 cargo 命令前独立执行：

```bash
python qa/browser_live/check_memory.py --self-test
python qa/browser_live/check_memory.py
```

只有两个命令都 exit 0 且 `available_bytes >= 4294967296` 才允许运行 cargo。cargo 串行执行，至少运行：

```bash
cargo fmt --all --check
cargo test -p alephcore feishu --lib
cargo test -p alephcore --lib
cargo check -p alephcore -p aleph-slot-api --all-targets
cargo clippy -p alephcore --lib --all-targets
```

真实 QA 必须使用 `qa/channels/run.sh` 和真实 `aleph-server`。如果 `websockets` 缺失，记录 BLOCKED/UNRUN 或使用已存在的本地 fixture 依赖，不修改仓库来掩盖环境问题。

## 交付物

- B8.0 source-backed 调用图和 B8-A 至 B8-D 决策；
- 一个明确的 B7 closeout 或 Zahir go/no-go 结果；
- 真实 fixture 与 failure matrix；
- 三份 reference 文档的边界更新；
- 起点 HEAD/status、修改文件、真实调用链、每条验证的 memory gate/exit/count/warnings、North Star conformance、残余风险、未做事项和未提交状态。

## 当前已知风险

- B7 当前只保证 Feishu 入站的 enqueue quiescence，尚未显式 join 每个已接受 webhook handler 的完整 HTTP response 生命周期。
- 当前生产路径没有独立 Feishu `Disabled` transition 或显式 enable race fixture，必须先确认现有 Gateway authority 如何表达它们。
- Feishu 目前不是 Zahir live facade，未提供 capability descriptor、lease、subscribe 或 project。
- `qa/channels` 的裸 Python 环境可能缺少 `websockets`。
