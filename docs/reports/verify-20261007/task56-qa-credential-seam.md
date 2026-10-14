# Task #56 QA credential seam (design + safety boundary) / Task #56 QA 凭据接缝（设计与安全边界）

Status / 状态：**UNCOMPILED AND UNRUN SEAM.** Nothing here has been built, tested or executed; no Cargo, shell, Gateway, browser, keyring, registry or process command was run while writing it. Overall verdict unchanged: **PARTIAL, not production ready**. / **未编译、未运行的接缝。** 编写时未执行任何 Cargo / shell / Gateway / 浏览器 / keyring / 注册表 / 进程命令；整体结论不变：**PARTIAL，非 production ready**。

## Goal / 目标

Let a *later* harness launch only a debug `--no-default-features --features qa-owned-root` shell against a harness-owned random TempDir **and** a random QA-only Windows Credential Manager service namespace, so the panel's real `panel_credential_*` Tauri commands can be exercised against the real OS backend without ever touching the production `aleph-client.device-credential` entries. Production builds and the existing in-memory `qa/native_ipc/run.mjs` mode are unchanged. / 让后续 harness 只用 debug `qa-owned-root` 构建，在 harness 自有的随机 TempDir 与随机 QA 专用 Credential Manager service 下启动 shell，真实执行 `panel_credential_*` 命令，且绝不触碰生产条目。生产构建与现有内存模式不变。

## What changed (source only) / 改动（仅源码）

| File / 文件 | Change / 改动 |
|---|---|
| `shared/client/Cargo.toml` | New feature `qa-service = []` (not default). |
| `shared/client/src/credential_store.rs` | `#[cfg(feature = "qa-service")] pub fn DeviceCredentialStore::for_gateway_in_service(gateway_url, service)`: real `OsKeyring`, same `account_for_gateway` + `*_for_connect` transport rules. `validate_qa_service` accepts exactly `aleph-qa.device-credential.` + 32..=128 lowercase hex (so never the production `aleph-client.device-credential`); error text does not echo the input. Pure fake-backend tests added (`qa_service_*`). `for_gateway` / production path untouched. |
| `desktop/shell/Cargo.toml` | `qa-owned-root = ["aleph-client/qa-service"]`; no direct `keyring` dependency. `Cargo.lock` not edited (a pure feature adds no package; verify at build). |
| `desktop/shell/src/qa_root.rs` | Optional `--qa-credential-backend memory\|os` (default `memory`). `os` requires the proven `--qa-nonce` to be 32..=128 **lowercase** hex (memory keeps the legacy 16..=128 hex rule); service = `aleph-qa.device-credential.<nonce>`. Duplicate flag, unknown/empty/flag-like value, `--flag=value` form and weak/uppercase `os` nonce are refused. No env / HOME fallback. Published (`credential_backend()`) only after the root + nonce are proven. New unit tests `backend_*`. |
| `desktop/shell/src/keyring_bridge.rs` | `#[cfg(feature = "qa-owned-root")] QaOsKeyringBackend` using the gated constructor. `production()` picks it only for `Some(Os{service})`; `Memory`/`None` keep `InMemoryQaBackend`. A QA build never selects `ClientStoreBackend`; non-QA builds still use it exclusively. |
| `desktop/shell/src/main.rs` | **No change needed**: `qa_root::init_from_args(std::env::args_os())` already aborts startup (exit 2) on any `QaRootError`, including the new variants. Tauri command surface unchanged. |

## Safety boundaries / 安全边界

- QA-only reach: `for_gateway_in_service` exists only with `qa-service`, which only `qa-owned-root` enables; `qa-owned-root` is debug-lite-only and never default. A release/production build has no way to name a non-production service.
- Namespace isolation: the service name is derived from a nonce the harness already proved by the `.qa-owned` file in an `aleph-shell-qa-*` direct child of the OS temp dir; the client re-validates the exact form, so a bug in the shell cannot widen it to the production service.
- No secrets in argv/config/logs/URL: only the (non-secret) nonce is in argv; errors are fixed strings; the token never leaves the existing DTO path.
- No success mocks / synthetic grants: authorization (origin / scope / lease / epoch) above the backend is the unchanged production code.
- Fail closed: invalid backend/nonce aborts startup; a backend call failure maps to `BridgeError::Storage`.

## NOT done / 未做

- Not compiled; `cargo check` / `clippy` / tests not run. New tests (client `qa_service_*`, shell `backend_*`) are unexecuted.
- No harness/monitor: nothing launches the shell with `--qa-credential-backend os`, mints the random nonce/TempDir, or **cleans the Credential Manager entries** the `os` mode will create under `aleph-qa.device-credential.<nonce>` (the harness must enumerate/delete only that exact service and verify absence). Until that exists, running `os` mode would leave residue.
- `qa/native_ipc/run.mjs` untouched (still memory mode; default is memory so it should remain compatible — unverified).
- Broad docs (`docs/reference/*`, `qa/README.md`) not updated; `qa_root.rs` module header still describes only the memory backend.
- Unverified: the `with_backend_in_service` / helper `cfg(any(test, feature))` gating compiles warning-free in every feature combination (default, `qa-service`, tests); `Cargo.lock` unchanged under the new feature; Windows-only real round trip under the QA namespace.
