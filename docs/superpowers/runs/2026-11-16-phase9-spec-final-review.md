> Independent reviewer / 独立审查：`759f2b30-c679-41e7-a346-fac1385b334a`. Repository-absolute path prefixes normalized only; verdict/findings unchanged. Raw SHA-256: `c000cb8e361538d84108146dffde6852db31100421b8d35fc31205224fcac5f0`. / 仅作路径可移植归一化，不改结论。

# Phase9 Independent SPEC Re-review — Final

审查基线：`3892804c112fd6e574166a4c03a7c40a77d0b239`  
审查分支：`runtime-proof`

## Review

### Correct

- [Phase9 execution receipt](< docs/superpowers/runs/2026-11-16-phase9-execution-receipt.md:3>) 正确保持 `PARTIAL CLOSURE — A/B/C PASS IN SCOPE; D OPEN; E IN PROGRESS`，没有把当前状态冒写成整体 Phase9 PASS。
- A 测试闭合记录 `22,402 passed, 0 failed, 20 ignored`。B 依赖闭合记录候选 `Cargo.lock` 的 1,572 个条目及 SHA-256 `12300967333b45f8a0fa1f401cf13a559b5b8a881ee95550b1bbf1a8d42e21f5`，并保留七个 NSH Git pin。
- R2 边界保持正确：Panel/Leptos/WASM 继续作为业务 UI authority；NSH 负责 native shell、lifecycle 和 transport；没有把 detached Gateway/daemon 纳入 NSH ownership tree。依据：[SPEC](< docs/superpowers/specs/2026-11-15-phase9-runtime-proof-and-dependency-closure.md:78-85>)。
- 旧的 closed-socket no-op P1 已由当前实际 ingress seam 解决。`interfaces/webchat/src/context.rs:29-53` 的 `dispatch_received_notification` 被生产 WebSocket loop（`interfaces/webchat/src/context.rs:2347`）和 QA ingress（`interfaces/webchat/src/context.rs:80`）共同使用，并在 reducer dispatch 前检查 receipt 与 Gateway URL。
- `interfaces/webchat/src/context.rs:64-87` 的 QA ingress 仅在 `runtime-qa + wasm32` 下生成；`interfaces/webchat/src/context.rs:2261` 在成功 connector open 后保存当前 receipt；`interfaces/webchat/src/context.rs:4066-4084` 的测试验证旧 receipt 被拒绝、当前 receipt 被接受。
- `qa/nsh_panel_driver.js:43-55` 现在直接调用两个真实 ingress callback，并记录 `boundary:"panel_ingress"`、`handler_invoked:true`、`current_control_accepted:true`、`stale_frame_accepted:false`。这项替换触及共享生产 ingress，已闭合原先“只证明旧 socket 无效”的证据缺口。
- runtime15 已补齐 tightened provenance。`docs/superpowers/runs/2026-11-16-phase9-runtime-attempt15.json:2` 状态为 `PASS`；Tauri 与 NSH 两个 runtime document 均为 `PASS`，每个 surface 有 79 个 raw-correlated events，direct child exit 为 0 且未 forced。
- runtime15 的两个 runtime document 均包含四个真实 HTTP served payload：`index.html`、`aleph_panel.js`、`aleph_panel_bg.wasm`、`tailwind.css`。served WASM SHA-256 为 `9dd254d4b946ed3381b5738a29cf84e0191c7e8fb081b8fd4938b15b958769bb`，与独立 canonical QA build 一致。证据见：

  - `docs/superpowers/runs/2026-11-16-phase9-runtime-attempt15-output/tauri-0.json:4-20`
  - `docs/superpowers/runs/2026-11-16-phase9-runtime-attempt15-output/nsh-0.json:4-20`

- runtime15 Gateway cleanup 记录 `panel_assets_unchanged:true`、`fixture_removed:true`；model child 以 SIGTERM 正常退出，Gateway child exit 0 且未 forced。证据见 `docs/superpowers/runs/2026-11-16-phase9-runtime-attempt15.json` 的 `gateway_cleanup` 字段。
- 当前 `qa/nsh_panel_gateway.py:24-48` 会抓取并保留四个 identity-encoded HTTP payload；`qa/nsh_panel_gateway.py:134-142` 在运行前后比较 served assets；`qa/nsh_panel_launcher.py:147-176` 会重新 hash retained payload、binary、stdout、stderr，并重构 exact marker sequence。runtime15 满足这一 tightened verifier。
- 默认 Panel bundle 已恢复并通过 byte-exact restoration：`docs/superpowers/runs/2026-11-16-phase9-default-panel-restoration.json:2-8` 状态为 `PASS`，`qa_ingress_name_present:false`。独立 default canonical receipt 也记录 `runtime_qa:false`、`qa_ingress_name_present:false`、状态 `PASS`。
- 新 source manifest 覆盖 43 个文件，基线为 `3892804c112fd6e574166a4c03a7c40a77d0b239`，SHA-256 为 `10d6f3148c1fbe2d1457226217632418178830f0e4fda2f3a7edb527f191b4c5`，见 `docs/superpowers/runs/2026-11-16-phase9-source-manifest.json:1-5`。
- 父会话报告的当前 gateway/launcher 测试为 34 个 Python tests、3 个 Node tests，HTTP local-server 与 missing-assets 回归均从 RED 到 GREEN。本独立 reviewer 未重新执行这些测试，因此将其作为 reported evidence。

### Finding: P1 — D 性能验收仍然 OPEN

位置：

- [SPEC](< docs/superpowers/specs/2026-11-15-phase9-runtime-proof-and-dependency-closure.md:76>) 要求 Tauri first、NSH second、10 次 warmup、30 次 measured、100 ms RSS/thread sampling 及既定 latency/RSS/thread 阈值。
- [Execution receipt](< docs/superpowers/runs/2026-11-16-phase9-execution-receipt.md:31-32>) 明确记录 `OPEN / BLOCKED BY RESOURCE OWNERSHIP`，且没有声称实际 10+30 acceptance。
- [Resource ownership investigation](< docs/superpowers/runs/2026-11-16-phase9-resource-ownership-investigation.md:3-5>) 说明当前采样只覆盖直接启动的 native child；WebKit XPC 进程由 launchd 作为 POSIX parent，当前没有可验证的 per-application ownership。调查还记录了 `parentASN`/coalition 不可用，以及 `RBSRequestErrorDomain Code=5`、`NSPOSIXErrorDomain Code=1 "Operation not permitted"` 和 `Launchd job spawn failed`。

runtime15 的两个 surface 都正确记录了 `resource_complete:false`，而且 `docs/superpowers/runs/2026-11-16-phase9-runtime-attempt15.json` 的 `performance.status` 仍为 `OPEN`。因此 runtime15 关闭了 C 的行为与 provenance 缺口，但没有关闭 D。

这是 evidence-validity blocker，不是已证明的性能阈值失败。当前不能把部分 native-child RSS/thread 样本提升为完整 WebKit-owned resource comparison，也不能把 D 改成 `PASS`、`NOT_ACCEPTED` 或 `FAIL`。

最小后续动作：建立可验证的 WebKit 系统 ownership 机制，然后重新执行 SPEC 要求的 10 warmup + 30 measured parity run。若无法取得该 ownership 证据，D 应继续保持 `OPEN/BLOCKED`。

### Attempt14 finding 已关闭

上一版审查提出的 attempt14 provenance P1 已由 runtime15 关闭。attempt14 的 `gateway.json` 确实缺少 `panel_assets`，因此不能满足 tightened verifier；但 runtime15 已包含四个 retained served payload、前后 asset stability 检查、独立 hash 和 exact marker reconstruction。

因此：

- attempt14 保留为历史行为证据；
- runtime15 是当前 C 的 provenance-qualified runtime evidence；
- 不再把“runtime15 receipt 缺失”列为当前 finding。

## Validation limits

本次独立复审保持 read-only：

- 未修改、暂存或提交任何文件；
- 未运行 Cargo、Python、Node 测试；
- 未启动或终止任何进程；
- 未读取受保护的 `.claude/settings.json`、根目录 `config.lock` 或根目录 `config.toml`；
- Graphify 未重新执行，仅使用既有 `spec-final-graph.txt`；
- 未将 runtime15 的 `resource_complete:false` 解释成完整性能证据；
- 未声明 release-profile performance、全平台 NSH、Tauri/WebChat 移除、detached daemon ownership、Safe Replay、external-effect exactly-once 或 universal scheduler 已完成。

## Merge verdict: BLOCK / NOT COMPLETE

A/B/C 当前均有足够的源码、receipt 和 raw runtime evidence 支持：

- A：测试闭合 PASS；
- B：依赖闭合 PASS；
- C：Tauri 与 NSH runtime 行为、真实 served assets、Gateway cleanup、Panel ingress stale-frame rejection 和 default build separation PASS；
- 旧 closed-socket no-op P1 已解决；
- attempt14 provenance 缺口已由 runtime15 解决。

整体 Phase9 仍不能判定为 PASS，因为 D 的完整 WebKit resource ownership 与 10+30 debug parity acceptance 仍然 OPEN。