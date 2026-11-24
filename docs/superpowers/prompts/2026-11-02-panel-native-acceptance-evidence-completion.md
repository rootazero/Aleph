# Next-session prompt: Panel native acceptance evidence completion

你正在 `D:/Workspace/Aleph` 的本地 `main` 上继续 Aleph Panel authenticated project context native acceptance。

先读：

1. `AGENTS.md` 以及本任务适用的 desktop/Panel/QA reference。
2. `docs/superpowers/plans/2026-11-02-panel-native-acceptance-evidence-completion.md`。
3. `docs/superpowers/runs/2026-11-01-panel-native-project-context-acceptance.md`。
4. `docs/reference/FEATURE_LOCATOR.md` §6.11.2。
5. `qa/native_ipc/native_project_native_io.mjs`、`native_project_run_core.mjs`、`native_project_evidence.mjs`、`native_project_runner.mjs`、`native_project_run.mjs`、`real_run_io.mjs`。
6. `desktop/shell/src/main.rs`、`desktop/shell/src/keyring_bridge.rs`、`src/bin/aleph-panel-native-qa-gateway/main.rs`、`interfaces/webchat/src/api/native_credentials.rs`。

当前事实：

- 当前 `main` 已包含本次任务提交 `24b965423 docs(qa): record third native attempt` 和实现提交 `aefd44e8c fix(qa): finish native project evidence hardening`；不要回退或重写历史。
- 三次真实 WebView2 启动均已消耗；第三次针对当前 HEAD 重建 artifact/provenance 后仍为 `BLOCKED/native_run_failed`。
- 第三次 cleanup 六步全部为 `true`：shell stopped、helper stopped、exact devices revoked、credential deleted exact、credential absent exact、owned root removed。
- 三次运行均没有可用的 helper messages、Gateway receipts 或 native trace；没有 native credential save/load/delete、protected persistence、same-device reload、native Global/A/B、revocation boundary 或 endpoint/lease 证据。
- 第三次报告在 `.superpowers/sdd/2026-11-02-panel-native-runtime-completion-implementation/native-launch-3.log`。
- `native_run_failed` 目前过于粗糙。下一任务的第一目标是补足证据和失败定位，不要盲目重复启动。
- 当前工作树应先确认 clean。不要读取旧 token-bearing connection file。

硬约束：

- 不新增 pairing UI、隐藏 selector、direct Tauri invoke、browser storage credential、fake actor、`actor=None` 或 test endpoint。
- 复用既有 Projects/Global → `DashboardState::select_panel_project` → `PANEL_PROJECT_SELECT_METHOD` 产品路径。
- 保持六步 exact cleanup、root-last 和独立 provenance manifest。
- 只允许 exact `(service, account)` credential reaper；不 wildcard 删除、不按进程名或端口 kill。
- 不连接、停止或替换日常 daemon，不触碰 `18790`。
- 不把 Node WS/HTTP、VM、helper receipts、DOM 观察或 cleanup proof 当作 native acceptance。
- 每个 Cargo 命令前从仓库根运行 `python qa/browser_live/check_memory.py`，使用 `CARGO_BUILD_JOBS=1`，不要用 `bash -lc` 改环境。
- WebView2 是独立 launch gate。完成 evidence changes、RED/GREEN、fresh artifacts、独立 manifest 和 reviewable preflight 后，最多先使用一个新的 120 秒业务窗口；第二次只能在第一份报告证明是局部证据缺口且重新授权后使用。

执行重点：

1. 逐一盘点 runner 的 native failure boundary，把 monitor、helper、shell trace、credential bridge、receipt 和 cleanup 的失败转换成固定脱敏 reason。
2. 为每个 boundary 添加确定性 RED/GREEN 测试。特别覆盖“helperMessages/receipts/nativeTrace 全空”的情况，必须指出最后到达的 boundary 和缺失层，不能只报 `native_run_failed`。
3. 复核并补齐 QA shell trace：至少能区分 `panel_credential_load/save/delete` 是否到达、是否成功，以及 `credentialPresent`；不能打印 credential 内容、token、raw ID 或 raw OS error。
4. 复核 monitor command 与 helper command 的发送/响应记录，记录固定命令类别和 cursor，不记录任意 payload。
5. 重新运行完整 Node/docs/diff 验证，重新构建当前 HEAD 的 Panel/helper/lite shell/reaper，exclusive 生成并校验 provenance，生成 preflight。
6. 只有 preflight 可审阅时才执行新的明确授权 launch。按 save → same-device load → project Global/A/B → revoke boundary → cleanup 顺序收集分层证据。
7. 最终若仍 BLOCKED，必须交付可复现的最后 boundary、固定 reason、缺少的证据层、完整 cleanup facts 和下一项架构决策；不得以 generic `native_run_failed` 收尾。

最终回复必须用中文，明确列出：完成内容、实际验证、native 结果、证据层边界、cleanup 结论、未完成项、review/format 限制，以及是否仍需新的架构或 launch 授权。
