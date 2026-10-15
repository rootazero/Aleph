# TUI `/browser` real-Gateway run / TUI `/browser` 真实 Gateway 运行报告

Date / 日期: 2026-10-07

## Label / 标注

**EN:** "real Gateway and TUI command path, instrumented OS runner". This is NOT an OS-launch proof: the test builds a real `ViewerLauncher`, but its `CommandRunner` only records the command (nothing is spawned, no browser/rundll32/window). Report field `os_launch_proved: false`.

**中文:** “真实 Gateway + TUI 命令路径，OS 运行器为插桩（仅记录）”。**不**证明 OS 层真实拉起：测试构造了真实 `ViewerLauncher`，但其 `CommandRunner` 只记录命令，不派生任何进程/浏览器/窗口。报告字段 `os_launch_proved: false`。

## What ran / 执行内容

- Command / 命令: `node qa/browser_live/tui-real-run.mjs --gateway ws://127.0.0.1:18790/ws --confirm-owned-pid 12704`
- Test / 测试: `tui::commands::tests::real_gateway::real_gateway_browser_command_path` (`#[ignore]`, `interfaces/tui/src/tui/commands.rs`), one Cargo runner, `--offline`, `CARGO_BUILD_JOBS=1`, `CARGO_PROFILE_DEV_DEBUG=0`.
- Pre-checks / 前置: `node --check` OK; memory gate `available_bytes=11350687744 required_bytes=4294967296 ALLOW`; owned PID 12704 (exe + `--config` exact, listener exact, `GET /` = 200, fixture = 200).
- No Native / HOME / keyring / OS-launch. Device token read in-memory by the test from a path only; no credential in argv/env/stdout/report.
- 未使用 Native / HOME / keyring / OS 拉起；令牌仅由测试进程按路径在内存中读取，argv/环境/输出/报告均不含凭据。

## Result / 结果

`test result: ok. 1 passed; 0 failed` (0.30 s), harness `PASS (cargo exit=0)`.

Sanitized report booleans / 脱敏证据: connected_with_device_token, initial_redaction_pending (revision 1), decided_redaction_safe, decided_revision_advanced, success_launch_count=1, success_notice_launched, success_url_on_trusted_origin, success_grant_live_until_dispose, failure_launch_count=1, failure_notice_launch_failed, failure_notice_cleanup_suffix_absent, **failure_probe_second_revoke_permission_denied=true**, tab_list_exact_matches=1, tab_close_success, own_tab_closed, cleanup_ok, scenario_ok — all `true`/expected; foreign_resource_events_ignored=0.

## Fix this round / 本轮修复

**EN:** Previous run failed only at the final probe, which expected `revoked:false` for a second revoke of an already-revoked grant. The Gateway (`src/gateway/handlers/browser_viewer.rs`, test `issuer_rpc_owned_issue_and_revoke_enforce_identity_without_oracle`) deliberately answers that with the generic `PERMISSION_DENIED` (-32008) / `"denied"` (no existence oracle). The assertion now accepts ONLY `CliError::Rpc { code == aleph_protocol::jsonrpc::PERMISSION_DENIED, message == "denied" }`; success, any other code/message, transport error or timeout still fails. Production helper semantics are unchanged; no server or security change. The error text is never echoed.

**中文:** 上一轮仅在最后一步探针失败：它期望对已撤销 grant 再次 revoke 返回 `revoked:false`。Gateway 有意对此返回通用 `PERMISSION_DENIED`(-32008)/`"denied"`（无存在性 oracle，见上述既有测试）。现断言只接受 `CliError::Rpc { code == PERMISSION_DENIED, message == "denied" }`；成功响应、其他错误码/消息、传输错误或超时仍判失败。生产 helper 语义未改，未改服务端或安全行为，错误文本不回显。

## Not done / 未做

- No OS-level launch of a viewer (proved only the command that would be run) / 未验证真实 OS 拉起。
- Only the Windows platform variant of the instrumented launcher; no cross-platform run / 仅 Windows 变体。
- Event RPC counts (`rpc_direct_calls`, `rpc_via_helper`) are client-side accounting, not server-observed / RPC 计数为客户端计数，非服务端观测。
- Single run; no repeat/flake sampling; no wider `cargo test`/clippy/fmt run this round / 仅单次运行，未跑全量 cargo test/clippy/fmt。
- Gateway PID 12704 left running (not started or stopped by this task) / 未启停 Gateway。
