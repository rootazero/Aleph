# 从本地 main 实施 Phase9 的重启 Prompt / Phase9 Main Restart Prompt

将下方内容完整交给新会话。相对路径均相对当前仓库的 git root；不要把上一台机器的绝对路径当作入口。
Use the entire prompt below in the next session. Resolve every relative path from the current Git root.

---

请从 Aleph 的**本地 `main`** 接续，实施 **Phase9：Runtime Proof & Dependency Closure / 运行时证据与依赖闭合**。用户已于 2026-11-16 明确要求“为下一步写一个 prompt……合并此次任务分支到 main，我重启会话从 main 开始实施”。这授权下一会话执行本文所列 Phase9 范围，**不是重新规划后再次索取相同范围批准**。范围记录在 `docs/superpowers/runs/2026-11-16-phase9-task5-authorization.md`。Phase9 名称是本次明确采用的工作标签，不冒充更早存在的官方 roadmap。

This prompt records authorization for next-session execution within the stated boundary. Follow the reviewed plan; do not restart Phase8 or request the same authorization again.

## 1. 先确认检出与证据 / Establish the checkout

1. 用 `git rev-parse --show-toplevel` 定位仓库，确认当前分支、HEAD、状态及 worktree。从本地 `main` 开始；不要 pull、reset、删除历史 worktree/分支/证据，也不要把全局 `origin/dev` 默认规则带入本任务。
2. 用 `git log -1 --format='%H %s' -- docs/superpowers/prompts/2026-11-16-phase9-main-restart.md` 定位本次 handoff 提交，确认它是当前 `main` 的祖先。交接提交见 `docs/superpowers/runs/2026-11-16-phase8-main-handoff.md`；源文件摘要见同目录 `2026-11-16-phase8-handoff-manifest.json`。先验证这些文件的哈希，不要把未检验的新改动归入旧证据。
3. 原始 Phase8 起点是 `e08af3789dbf194f02f08461c215eb3c7701557f`；Phase7 handoff `2295330e51c77b20aaa9a6ce55f6fca666a810fc` 是其祖先。不要把后续 `HEAD` 当成 Cargo.lock 的原始比较基线。
4. 可以从当前本地 `main` 新建短任务分支；分支最多三个词，无 `/`。保持一个 source writer、一个 Cargo owner，独立审查只读。保留已有任务分支和证据。
5. 不读取或改动 `.claude/settings.json`、根 `config.lock`、根 `config.toml`。`Cargo.lock` 是本任务明确授权审计的依赖锁，和受保护的根 `config.lock` 不同。

## 2. 必须先读 / Required reading

按当前 harness 的 skill 规则加载相关 skill；已有已批准计划，直接使用 executing-plans；定位失败用 systematic-debugging，写测试前读适用测试指南，完成前用 verification-before-completion。不要因历史计划写着“下次批准”而忽略本 prompt 记录的新授权。

首先读：

- `AGENTS.md`、当前子目录适用的说明；`CLAUDE.md` 是需保持同步的宪法入口，不能静默改变其中一边。
- `docs/reference/SUBSYSTEM_ROUTING.md`
- `docs/reference/REDLINES.md`
- `docs/reference/DEVELOPMENT.md`、`docs/reference/WORKSPACE.md`
- **完整 NSH 总纲** `docs/superpowers/specs/2026-11-05-nsh-desktop-refactor-design.md`。它是历史 v0.4 草稿，不能当 v0.5 API 或完成证据；保留其中 Phase0–6 和 Phase4 parity + 两周双运行后才移除 Tauri 的总方向。
- **当前 Phase9 spec** `docs/superpowers/specs/2026-11-15-phase9-runtime-proof-and-dependency-closure.md`
- **当前 Phase9 执行计划** `docs/superpowers/plans/2026-11-15-phase9-runtime-proof-and-dependency-closure.md`
- `docs/superpowers/runs/2026-11-15-phase9-entry-inventory.md`
- `docs/superpowers/runs/2026-11-15-phase9-spec-review.md` 和 `docs/superpowers/runs/2026-11-16-phase9-quality-review.md`，包括审查来源修正和后续独立结论。
- `docs/superpowers/runs/2026-11-16-phase8-main-handoff.md`：本次集成状态、最新验证、仍开放的问题。

Phase8 证据与边界：

- `docs/superpowers/runs/2026-11-15-phase8-closeout.md`
- `docs/superpowers/runs/2026-11-15-phase8-security-execution-receipt.md`
- `docs/superpowers/runs/2026-11-15-phase8-cargo-lock-audit.md`
- `docs/superpowers/runs/2026-11-15-phase8-security-evidence-matrix.md`
- `docs/superpowers/runs/2026-11-15-phase8-task5-approval.md`
- `docs/superpowers/specs/2026-11-15-phase8-integration-security-design.md`
- `docs/superpowers/plans/2026-11-15-phase8-integration-security-implementation.md`
- `docs/superpowers/runs/2026-11-15-phase8-nsh-spec-review.md`、`docs/superpowers/runs/2026-11-15-phase8-nsh-quality-review.md`

NSH 的实施约束与数据路径：

- `docs/superpowers/runs/2026-11-15-nsh05-migration-inventory.md`
- `docs/superpowers/runs/2026-11-15-nsh05-upstream-evidence.md`
- `docs/superpowers/specs/2026-11-15-nsh-panel-parity-slice.md`
- `docs/superpowers/plans/2026-11-15-nsh-panel-parity-slice.md`
- `docs/superpowers/specs/2026-11-15-nsh-r2-native-ui-decision.md`
- `docs/reference/DESKTOP_SHELL.md`、`docs/reference/DESKTOP_BRIDGE.md`、`docs/reference/WINDOWS_RUNTIME.md`、`docs/reference/LINUX_DESKTOP.md`
- `docs/research/NSH_ADR-001~009.md`、`docs/research/NSH_PROTOCOL_V1.md`：历史设计与状态，不是现行实现证明；ADR-003/004/005 Accepted，006/007/008/009 Proposed。
- `qa/nsh_panel_parity.md`、`qa/nsh_panel_parity.py`、`qa/nsh_panel_parity.schema.json`

## 3. Graphify、执行与验证纪律 / Execution discipline

读源代码前必须先用现有图定向；有图时走 fast path，不重新全仓构图：

`graphify query "Phase9 extension cache lifecycle NSH canonical Chat Artifacts reconnect shutdown dependency closure" --budget 2200`

将下面原文放进每个涉及源代码探索的子代理 brief：

> MANDATORY: graphify-out/graph.json exists. You MUST run graphify before reading source files. Use: graphify query "<question>" (scoped subgraph), graphify explain "<concept>", or graphify path "<A>" "<B>". Only read raw files after graphify has oriented you, or to modify/debug specific lines. This rule applies to subagents too — include it in every subagent prompt involving code exploration.

每次 Cargo 命令前运行 `python3 qa/browser_live/check_memory.py --required-bytes 4294967296`；只有 exit 0/ALLOW 才能启动。构建/测试使用独立 task-specific `CARGO_TARGET_DIR`、`--offline --locked --jobs 1`。先分开处理依赖获取/锁生成，不能静默联网或把 locked/offline 失败写成测试通过。长任务用受控后台进程、原始日志和可观测退出码；不要因工具 600s 上限或“暂时没输出”就认定源码失败或绿灯，不留下孤儿进程。不要运行不存在的根 `bun check`/`bun run check`，也不要套用别的项目的 `dev:live`。有疑问先查当前 package scripts。

## 4. 已完成的 Phase8，准确继承 / Phase8 evidence

结论是 **PASS WITH RECORDED LIMITS**，仅指受控范围，不是全工作区、所有后端、live NSH 或 release 验收。

已通过的历史证据：

- `alephcore` lib check、`aleph-server` check。
- 六个 Phase8 单测各 1/1：`foreign_scope_denial_records_policy_violation_without_handler_effect`、`stale_lease_after_owner_bump_fails_and_fresh_resolve_succeeds`、`ui_surface_cancel_after_dispatch_joins_worker_and_clears_pending`、`projection_lag_requires_snapshot_before_reconnect_resolve`、`disable_closes_owned_surface_and_rejects_old_lease`、`captured_terminal_rejects_provider_replacement_before_invoke`。
- NSH 包 27 shell tests + 1 v0.5 Window/WindowRef API case，28/28。这不是窗口实际打开的证据。
- 当前源快照的 WebChat 重建产物：`panel:tabs:test` 6/6、`panel:tabs:artifacts:test` 12/12、protocol fixture PASS；fixture 不证明 live Gateway。产物绑定交接前基线 revision，新提交后不能无条件复用 provenance 为新 HEAD 的证明。
- parity reducer 的接受 exit 0、有效但不达标 exit 3 smoke；Python/JSON/定点 rustfmt/diff 检查。

全量 lib 测试曾超时，无最终 `test result`，旧日志有九个长跑标记，无已捕获 FAILED/panic；只能记 **UNRESOLVED_TIMEOUT**。不能靠跳过测试结项。本次交接复验已出现比旧九项更多的 extension/hook 长跑测试，且一项 Phase8 正向控制的 300s fresh rerun 也未取得终态；先枚举 `docs/superpowers/runs/2026-11-16-phase8-handoff-verification.json` 的完整当前清单，不要把调查限制在旧九个 filter，也不要把之前的通过结果覆盖或用来遮蔽新的 timeout。最新交接验证见 handoff 文件，优先保留更近的实际结果。

## 5. Phase9 顺序与明确产物 / Work order

**A. 全量测试闭合。** 按计划新增 `qa/phase9_test_closure.py`；九个 slow filter、600s timeout、cwd/env/argv、夹具与清理、stdout/stderr、退出码都写回执。源码集中在 `src/extension/claude_cache_tests.rs` 和 `src/extension/context_adapter.rs`。先查是否锁、并发环境、真实生命周期缺陷或环境阻塞，再做小范围修复；六个 Phase8 case 是独立正向控制。区分 PASS / FAIL / BLOCKED / UNRESOLVED_TIMEOUT，不能把源码证明的 teardown 违约记成 BLOCKED。

**B. Cargo.lock 决策。** 先保存 retained graph，再用 `tomllib` helper `qa/cargo_lock_delta.py` 生成 `frozen_to_retained` / `retained_to_candidate` 的完整机器可读 added/removed/changed 数组。原始基线冻结为：

- commit `e08af3789dbf194f02f08461c215eb3c7701557f`
- 原始 lock SHA-256 `bb1b647bb8f53785467c2dd47b40ed0bd46f2514c9de4e6e94bc29fccc900345`
- retained lock SHA-256 `d6f2e99dd2df5868b15f2940d74cf8845ea7c26d3ff69819ce42422180bb1cdb`
- NSH Git rev `f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1`，源 `https://github.com/longbridge/nsh.git`，v0.5.0 peeled commit；不是浮动 main/v0.5.1，也不是旧本地 v0.4 checkout。

现 retained graph 1,596 entries，旧 1,537；identity +328/-269；name-level 37 added/12 removed/285 changed version/source sets，7 NSH Git entries。旧“280”是较早不同统计口径，审计以明示身份/口径为准。最小手拼曾遭 `webpki-roots` 1.0.9/1.0.12 和 `toml` 0.8.2/0.8.23 冲突，**不证明任何 Cargo resolver 驱动的最小更新都不可能**。可验证最小化；若不安全，保留完整 graph 并明确风险，不能手编假锁。只在可复现 locked metadata + 相关编译/测试后决定。

**C. NSH live/runtime。** 按计划补 `qa/nsh_panel_launcher.py`、runtime schema、fixtures 和 debug QA markers，实际打开 macOS 原生窗口；验证 ready → hello → accepted → streamed → complete → artifact-refreshed → reconnected → closed。每个事件关联 session/run/connection epoch，旧 epoch、错 session、畸形 topic 均不能污染当前 projection。当前 `SessionManager` 有真实 RPC，但**尚未证明**安全重连、重复/迟到事件、owned drain、单源 transport、R2 合规与完整 Panel parity；将其视为待验证/可修复点，不能由编译 PASS 推导。

重要源码入口：`desktop/nsh-shell/src/{main,runtime,connection,session,supervisor}.rs`、`shared/client/src/connection.rs`、`shared/protocol/src/events.rs`、`interfaces/webchat/src/api/{chat,artifacts}.rs`、`interfaces/webchat/src/components/artifacts/mod.rs`、`desktop/shell/src/qa_root.rs`。当前存在状态连接与 SessionManager 连接两条路径，需以源码和运行证据判断及修复，不能再添第三套权威。

Chat `/` + Artifacts 保持 canonical `sessions.list` / `chat.history` / `chat.send` / `artifacts.list`。`session.artifact` 是失效通知，要 refetch，不是完整 artifact 值。区分内部 run topic、`stream.*` wire method、shared `StreamEvent`；Queued 不等于 Accepted/streaming。Core/Server 保存事实，Panel/Leptos/WASM 的 R2 边界仍有效；如 native business UI 与之冲突，先明确 ADR/宪法决策，不能静默扩大本授权。

用户对还原的要求：以现 Tauri Panel 的可观察信息架构和交互为参考；不是 1:1 像素/实现照抄。能保留的保留，冲突按 NSH 单引擎/窗口/生命周期重构，以性能为优先。不要为外观复制第二套业务状态源。

**D. debug parity。** Tauri QA 仅 debug-only lite：`--no-default-features --features aleph-desktop-shell/qa-owned-root`。先读实际 `qa_root.rs`：OS-temp 直属 `aleph-shell-qa-*`，32-byte lowercase-hex nonce，`.qa-owned` 为 nonce + newline，argv arrays + `shell=False`。NSH 当前没有 `--qa-root` / `--qa-nonce` CLI，不能虚构；launcher 只能证明其直接子进程退出/清理，外部 daemon 不属 shell。

运行事件/schema → metrics → 现 reducer 是三个边界，必须实际打通。Tauri first、NSH second，10 warmups + 30 measured，p95 one-based rank 29，100ms sampling；latency median ≤ baseline×1.10 / p95 ≤×1.20，RSS median ≤×1.10，threads median ≤ baseline+2。有效但超阈值保留 NOT_ACCEPTED，不能改成 invalid/BLOCKED。debug 通过不等于 release 性能通过。

**E. 独立审查与结项。** 按先 SPEC、后 QUALITY 的顺序审查实际最终 diff 和原始证据，修复实质问题；只写真实 reviewer verdict 和明确范围，不把父代理修复意见冒写为独立 PASS。完成后写 Phase9 execution receipt/closeout；未闭合的闸门继续开放，不因有限受控 PASS 伪称全 Phase9 通过。

## 6. 授权边界与不做事项 / Authorization and exclusions

本 prompt 授权上述 Phase9 实施、必要局部修复与验证，以及已约定的只读独立 SPEC/QUALITY；不需要重复询问相同范围。遇到新重大架构选择则给具体可审结果后询问。

**未授权/本轮不做：** Tauri/WebChat/workspace-member 移除；接管 detached daemon 生命周期；静默更改 R2/宪法；全平台 NSH 完成；release-profile 性能验收；Safe Replay、external-effect exactly-once、universal durable scheduler；push、发布、release。Phase4 parity + 两周双运行之前仍不得移除 Tauri。Swift 截图/语音/相机、i18n、GPUI 继续按历史单独范围处理。

继续遵守同一个 `OwnershipTree` / registry / descriptor/source authority；lease generation 不重取当前代冒充旧 capture；`ZahirFacade::subscribe` 忽略 caller cursor 并锚定当前 snapshot，lag 本身不撤销 lease，真实 owner/provider/source 变化才触发旧 lease 失效；broadcast 是 notification，不是 durable recovery。Contract A 是 canonical metadata before successful addressed-key `chat.send`/`agent.run`；reachable `SimpleExecutionEngine` 的模拟 transcript emit failure P2 不被本 prompt 偷换成已修复。UI `UiRenderAck` 与 PTY reply/event 不混同。

开始时先用中文说明已确认的基线、首个将执行的步骤和设计代价，然后创建/恢复 todo 并实施；不要只复述计划就结束。最终报告用中文，明确 changed/why/evidence/limits，并列出未执行的事项。
