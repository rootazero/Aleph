# Restart prompt: Panel native runtime completion
# 重启会话提示：Panel native runtime 完成

你正在 `D:/Workspace/Aleph` 的本地 `main` 上继续 Aleph Panel authenticated project context 验收。

当前基线：

- 本地 `main` 已 fast-forward 合并上一阶段 `native-context-qa`，HEAD 为 `f6e18cdf8`。
- 没有 push；开始工作前确认 `git status --short` 干净。
- 上一阶段已完成 QA-only disposable Gateway、feature-gated authenticated RPC observer、redacted receipts、bounded project DOM actions、fail-closed runner 和 exact cleanup adapter。
- 上一阶段实际启动过一次 QA Windows Tauri/WebView2，使用 120 秒、最多一次 launch 的明确预算。WebView2 monitor 成功连接，但真实页面没有 TokenWall 或 Browser pairing notice，最终为 `protected_pairing: BLOCKED / pairing_surface_unavailable`。
- Runner 返回 `AMBIGUOUS`，因为 helper 没有输出 contract 要求的 stopped facts；外部 CIM 加启用 `qa-service` 的 exact reaper 已证明进程和 credential absence，随后才删除 owned root。
- 没有 native `panel_credential_save/load/delete` trace、protected OS persistence、same-device reload、native A/B/Global flow、endpoint/lease transition 或 native revocation boundary 证据。
- 最新实现已经修正了 cleanup 生命周期、receipt cursor、artifact manifest hash、120 秒 deadline、cleanup phase 和 connect authorization 派生；这些修正需要重新验证。

先读：

1. `AGENTS.md` 以及当前仓库适用的 `docs/reference/*`。
2. `docs/superpowers/plans/2026-11-02-panel-native-runtime-completion.md`。
3. `docs/superpowers/runs/2026-11-01-panel-native-project-context-acceptance.md`。
4. `docs/reference/FEATURE_LOCATOR.md` §6.11.2。
5. `qa/native_ipc/native_project_native_io.mjs`、`native_project_runner.mjs`、`native_project_cleanup.mjs`、`native_project_evidence.mjs`、`native_project_protocol.mjs`。
6. `desktop/shell/src/main.rs`、`desktop/shell/src/connection.rs`、`desktop/shell/src/keyring_bridge.rs`、`interfaces/webchat/src/context.rs`、`interfaces/webchat/src/api/native_credentials.rs`、`interfaces/webchat/src/components/token_wall.rs` 和 `interfaces/webchat/src/platform/wide/views/browser/viewer.rs`。

执行顺序：

1. 先运行只读状态检查和现有 Node/docs/diff 验证。
2. 先用源码和测试补齐 pairing surface 的 RED/GREEN 诊断，不启动 WebView2。必须确认 fresh Panel 页面为什么没有 TokenWall 或 PairingNotice，以及是否存在已实现的产品支持路径。
3. 修复并测试 runner 的 helper stop/revoke 生命周期、receipt `seq > cursor`、实际 deadline、cleanup phase 和 artifact manifest 比对。不得通过 self-pinning 当前文件伪造 provenance。
4. 重新验证默认 Cargo 与 `native-qa` Cargo；每次 Cargo 前从 worktree root 运行 `python qa/browser_live/check_memory.py`，固定 `CARGO_BUILD_JOBS=1`，不使用 `bash -lc` 改环境。
5. 只有在完成 reviewable preflight report 后，向用户请求新的明确 WebView2 launch budget。上一阶段的 120 秒预算已经消耗，不能默认复用。没有新批准时不得启动 WebView2、写 OS credential 或执行真实 native runner。
6. 若获批准，最多按批准次数启动，证据必须分层记录：native Rust trace、Gateway receipts、DOM monitor、artifact/process provenance、exact cleanup。任何 `BLOCKED`、`NOT_RUN` 或 cleanup `AMBIGUOUS` 都不能成为 PASS。
7. 依次验证 protected save/load/delete、same-device reload、native Global/A/B、revocation next-boundary；endpoint/lease 只能使用真实现有产品路径。若产品没有支持的 bounded transition，保持 `BLOCKED` 并写明源码依据，不添加隐藏 UI、direct invoke、browser storage token、actor=None 或测试产品 endpoint。
8. 最后更新双语 run ledger 和 `FEATURE_LOCATOR.md`，做 full Node/docs/Cargo/rustfmt/diff validation，提交代码后发起 fresh read-only whole-branch review。SIGTERM/timeout 必须披露，不能称 clean review。

硬约束：

- 不连接、停止、替换日常 daemon 或 port `18790`。
- 不读或打印旧的 token-bearing `.panel-live/control/connection.json`。
- 不枚举或 wildcard 删除 credential；只允许本次 owned `(service, account)` exact reaper。
- 不按进程名或端口 kill；只处理记录过的 owned PID/process tree。
- 不把 Node WS/HTTP smoke、helper receipts 或纯 DOM VM tests 说成 native WebView2 proof。
- 不把 shell binary 当前即时 hash 当作独立 freshness proof；使用 prelaunch manifest 比对。
- 不 merge/push，除非用户在新的会话明确要求。

交付目标：完成真实 native proof，或者提交一份精确、可复现、没有伪造 PASS 的 BLOCKED/NOT_RUN 结果，并保留 tracked-clean branch、完整证据分层、cleanup 结论和剩余架构决策。
