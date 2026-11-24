# NSH Panel S1-B Desktop IPC — verification / 验证

**Scope / 范围:** Opt-in, read-only UDS shared by the standalone Gateway and NSH status command; not the Panel data plane. / 显式启用的只读 Unix 传输，不切换现有 Panel。

**Plan / 计划:** [S1-B Desktop IPC](../plans/2026-11-23-nsh-panel-s1b-desktop-ipc.md). Baseline `b2462006fe9e1437654e071579f6c2ef4b2a58fd`. Verified source `06e2c1bad3f23eb6647e4f3fe4f37b3c39ba0c43` (tree `7f13748409ee1664a43c7dab78cdb4d5f1b08214`).

## Mounted contract / 已接通的契约

- [`aleph-server --desktop-ipc <fresh-absolute-socket> start`](../../../src/bin/aleph-server/cli.rs) opts into a [Gateway listener](../../../src/gateway/server/connection/local_ipc.rs) in a caller-owned mode-0700 directory. Existing entries are refused. Gateway closes the listener and peers but never chmods or unlinks the published entry. The caller supplies a fresh path on the next launch and owns directory maintenance. / 退出后旧 socket 不可连接，目录项由调用方管理。
- Both peers check the kernel-attested effective user. Gateway maps that account to the instance's active Core owner, checked again for admitted RPCs. This is account identity, not executable identity, paired-device identity, or legacy loopback authorization. / 同账户不是二进制证明，也不获得已配对设备身份。
- First frame `system.hello`, then only `health` or `sessions.list` through the existing shared Gateway middleware and Core handlers. Limits: 8 MiB per frame, eight local connections, eight in-flight requests per connection, and a 16 MiB inbound wire-byte budget per connection. Received requests and sent responses share the JSON-shape bounds. An oversized result returns an error for that ID; a handshake diagnostic that exceeds the bound keeps its original error code and ID. A timed-out IPC call does not free its peer slot before response or connection retirement. / 不开放写 RPC、事件订阅或回放。
- [`aleph-nsh-shell --desktop-ipc-status <absolute-socket>`](../../../desktop/nsh-shell/src/ipc_status.rs) uses the [shared Client](../../../shared/client/src/connection.rs) to read real `health`. It does not start or stop a service, change credentials, open the Panel or fall back to TCP/WS. / 仅查询运行中的 Gateway。

## Native evidence / 原生证据

On macOS `27.2.1` arm64, Rust/Cargo `1.99.1` and `libprotoc 36.2`. All Cargo invocations used `--locked` and the repository memory gate. Gateway ran serially with an isolated `ALEPH_HOME`.

- Client **68/68**, Client Clippy with `-D warnings`, and CLI/TUI check passed. Gateway **237/237**, including native mounted UDS and legacy first-frame coverage; `gateway_ui_ingress` **1/1**; server option test **1/1**; NSH host **30/30** across three suites. Both the server and NSH debug builds succeeded.
- [`qa/nsh_desktop_ipc_smoke.py`](../../../qa/nsh_desktop_ipc_smoke.py) passed on the two debug executables. It checked missing, mixed and QA arguments; real Gateway `health` over UDS; service survival after the probe; then SIGTERM, retained pathname and refusal to reconnect. SHA-256: server `027fe45ba48750d5af1b53212bd6fe63e0a9d6866d40dab3dca048a7a15e29ba`; NSH `b03a3c5753b9714b0d71f1509cf3a9e4d52738344592afe139a736ced9e2d183`.
- The existing macOS compact-unwind linker warning persisted; all test and build commands exited zero. Changed Rust lines conform to `rustfmt`; one pre-existing formatting hunk in `src/bin/aleph-server/cli.rs` remains untouched.

| Platform / 平台 | State / 状态 | Remaining / 待验 |
| --- | --- | --- |
| macOS arm64 | **Pass** for explicit, read-only UDS | Debug executable, not GUI or signed package acceptance |
| Linux | Unix path implemented, **not run natively** | Execute the native fixtures and binary smoke below on Linux |
| Windows | **Unavailable / fail closed** | No Named Pipe transport. Implement and validate same-account DACL/SID, remote rejection, occupied name, framing/concurrency and lifecycle on a native Windows machine |

## Replay / 复验

Run from the repository root, with the locked toolchain and dependencies. The smoke script creates a transient `HOME` and `ALEPH_HOME`, chooses a non-default loopback port, and acts only on its own child process. / 在干净的原生检出中复验，不探测已安装服务或默认端口。

```bash
export CARGO_BUILD_JOBS=2
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
profile=$(mktemp -d /tmp/aleph-ipc-verify.XXXXXX)
export ALEPH_HOME="$profile"
gated() { python qa/browser_live/check_memory.py && cargo "$@"; }
gated test --locked -p aleph-client &&
gated clippy --locked -p aleph-client --all-targets -- -D warnings &&
gated check --locked -p aleph-cli -p aleph-tui &&
gated test --locked -p alephcore --lib gateway::server:: -- --test-threads=1 &&
gated test --locked -p alephcore --test gateway_ui_ingress &&
gated test --locked -p alephcore --bin aleph-server desktop_ipc_is_explicitly_opt_in &&
gated build --locked -p alephcore --bin aleph-server &&
gated test --locked -p aleph-nsh-shell &&
gated build --locked -p aleph-nsh-shell &&
python qa/nsh_desktop_ipc_smoke.py "$CARGO_TARGET_DIR/debug/aleph-server" \
  "$CARGO_TARGET_DIR/debug/aleph-nsh-shell" "$profile/native-server.log"
```

Linux acceptance also requires the same UID/owner checks, real scoped response, out-of-order IDs, duplicate refusal, cancelled-service cleanup, retained unusable retired path, and no fallback. / Linux 需在原生环境重跑，不借用 macOS 结果。

Windows refusal-only replay (unrun). Use a clean PowerShell session with no NSH QA variables and the default Cargo target directory. The current code must refuse before acquiring or changing any service. / 以下仅证明显式拒绝，不是 Named Pipe 验收。

```powershell
python qa/browser_live/check_memory.py
if ($LASTEXITCODE -ne 0) { throw 'memory gate failed' }
cargo build --locked -p alephcore --bin aleph-server -p aleph-nsh-shell --bin aleph-nsh-shell
if ($LASTEXITCODE -ne 0) { throw 'build failed' }
$endpoint = Join-Path $PWD 'ipc-disabled.sock'
& .\target\debug\aleph-server.exe --desktop-ipc $endpoint start
if ($LASTEXITCODE -eq 0) { throw 'server should refuse Desktop IPC' }
& .\target\debug\aleph-nsh-shell.exe --desktop-ipc-status $endpoint
if ($LASTEXITCODE -eq 0) { throw 'host should refuse Desktop IPC' }
```

Both should report `Desktop IPC is unavailable on this platform` and leave no service, credential or path changes. Named Pipe evidence requires a subsequent implementation and real Windows runner.

## Review and non-claims / 审阅与边界

Independent reviews: `440e26d0-4ea8-4536-bde0-83e19a654a02` blocked the initial candidate; `25a5b4ee-600f-4684-a9d7-4a9c8c640500` closed path mutation, sent-slot retention and cancellation cleanup, and verified the bounded business response. It found a handshake response still could exceed the shared shape limit. `8ae6679b-ea1e-4c4b-ba81-48d4ed58417d` verified the fixed handshake preserves the original RPC ID and error code, with no remaining Critical/Important finding. These were read-only reviews; see native evidence above for execution. / 审阅不替代实测。

No full S1-B three-platform acceptance, S1-C, WebView migration, service installation, default switch, release or Tauri retirement. / 平台缺口不计通过；没有改变旧入口或发布行为。
