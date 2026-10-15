# Native keyring round trip (Windows Credential Manager) / 原生 keyring 往返探针

Status / 状态：**PASS for this single probe only.** Overall verdict unchanged: **PARTIAL, not production ready**. / 仅此单次探针通过；整体结论不变：**PARTIAL，非 production ready**。

Scope / 范围：Windows 单机；复跑并收窄 Task40 的探针（`.superpowers/sdd/2026-10-07-panel-browser-workspace/task-40-windows-keyring-report.md`，旧过滤方式 `36 filtered out`；本次精确过滤 `42 filtered out`，因 client lib 测试数增长）。 / Windows single machine; a rerun of the Task40 probe with an exact filter.

## Actually run / 实际执行

- Working directory / 工作目录：`D:/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256`（W）。
- Memory gate / 内存闸：`node -e os.totalmem` = `21452304384` >= `4294967296`（ALLOW）。
- Command / 命令（从 W 运行）：

```text
CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 cargo test -p aleph-client --lib --offline credential_store::tests::os_keyring_roundtrip_on_windows_credential_manager -- --ignored --exact --nocapture --test-threads=1
```

- Result / 结果：`1 passed; 0 failed; 42 filtered out`。

## What the test does / 测试做了什么

Test: `shared/client/src/credential_store.rs::tests::os_keyring_roundtrip_on_windows_credential_manager`（`#[cfg(windows)]`、`#[ignore]`）。

1. Generates a random `.invalid` gateway URL (`wss://aleph-keyring-probe-{pid}-{nanos}.invalid:443/ws`) and device id / token, hence a unique keyring account namespace; builds a public `DeviceCredentialStore::for_gateway`; asserts the service / account shape.
2. Asserts the unique account is absent before writing (never overwrites an existing entry); arms a cleanup guard before the write.
3. `save` -> `load`: loaded `device_id` and `persistable_parts()` equal the exact saved payload; the raw keyring payload (via `OsKeyring`) decodes with the expected `PAYLOAD_VERSION`, device id and token.
4. `delete` returns true; `load` is `None` (absence verified); a second `delete` returns false. The guard is disarmed only after all checks pass.

随机 `.invalid` gateway / device 命名空间；保存后加载得到完全一致的 payload；删除后确认条目不存在，且第二次删除返回 false。测试不打印 token，不发起网络请求。

## What this proves / 证明了什么

The **public `aleph-client` `DeviceCredentialStore` (production `OsKeyring` backend, no fake)** round-trips a device credential through the **real Windows Credential Manager** on this machine: save, exact load, delete, verified absence, idempotent second delete. / 公共 `aleph-client` `DeviceCredentialStore`（生产 `OsKeyring` 后端，非 fake）与真实 Windows Credential Manager 的 save / load / delete 往返在本机成立。

## What this does NOT prove / 未证明

- Tauri IPC (`panel_credential_*` against the real OS keyring) — the 2026-10-12 Native IPC QA used the in-memory backend with the OS keyring compile-excluded.
- Protected pairing chain (bootstrap ticket -> device token -> keyring -> reconnect).
- Full Panel / WebView2 / WASM behaviour with this store.
- macOS Keychain / Linux Secret Service or any non-Windows platform; other Windows accounts or policies.
- Managed companion (#48 / #53 blockers unchanged), TUI launcher E2E, Gateway-side verification, whole-branch approval.
- Interrupted-process cleanup (force-kill before the guard runs); no before/after Credential Manager enumeration was taken, so "no other entries touched" is not claimed beyond the test's own unique account.

## Notes / 备注

No source was modified for this report. No commit or reset. Real user credentials, `HOME` and `.aleph` were not touched by the documenting step; the probe itself only wrote and removed its own synthetic entry.
