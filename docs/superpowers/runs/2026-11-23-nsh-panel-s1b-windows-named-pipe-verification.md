# S1-B: Windows Named Pipe 原生验收 / Windows-native receipt

**Windows 生产入口保持拒绝 / Production remains fail closed.** This receipt distinguishes the unchanged refusal baseline, a locally enabled diagnostic build, and the guarded candidate source. The diagnostic build is not a distributable artifact. No Linux, macOS, Windows GUI, NSH default, installer, release or Tauri-retirement conclusion follows from this work.

## 环境与来源 / Environment and provenance

- Native Windows 11 `10.0.28000.0`, x64; Intel Core i9-13900H. Medium-integrity interactive token, not elevated; the Administrators SID was deny-only. The account is an Administrators member, so this is not evidence from a dedicated standard-user account. Only this fixture identity was authorized; no second Windows identity or credentials were provisioned.
- Rust `1.99.1 (58cf8f9c3)`, `x86_64-pc-windows-msvc`; Cargo `1.99.1 (5f94df478)`; MSVC 14.50.35717 (`cl.exe` 19.50.35722.0), protoc `36.2`, Python `3.14.7`. NSH `0.6.0`, Tokio `1.53.3`, Mio `1.2.4`, Windows bindings `windows-sys 0.61.2`.
- Isolated `s1b-windows-named-pipe` worktree. Start commit `8c02bb13582d8535b67f04075f58c8a80e68f58b`, tree `0d43587d22883bc1d31402da51ad8d379738d7ab`; required ancestor `06e2c1bad3f23eb6647e4f3fe4f37b3c39ba0c43`. Baseline `Cargo.lock` SHA-256 `a608c2a5187db760c47458f0f0e8d7d9efd599aa1ffa941436cf7d7485466083`; candidate lock SHA-256 `df67b7946ee499fc2533d2379461df9102b8b096f721bf90c33d36922e939b1a`, adding the Client's edge to the already locked Windows bindings.
- 启用候选的验证只使用随机管道名、临时 `ALEPH_HOME` 和非默认回环端口。No service registration, installation, real credentials, account/registry changes, push or merge.

## 基线 / Refusal baseline — PASS

```text
python qa/browser_live/check_memory.py
cargo build --locked --jobs 2 -p alephcore --bin aleph-server
python qa/browser_live/check_memory.py
cargo build --locked --jobs 2 -p aleph-nsh-shell
python qa/nsh_desktop_ipc_windows.py baseline target/debug/aleph-server.exe target/debug/aleph-nsh-shell.exe
```

Both builds and the baseline fixture exited `0`. The real Gateway (`--bind 127.0.0.1 --port 0 --desktop-ipc <random local pipe> start`) and real NSH (`--desktop-ipc-status <same pipe>`) each exited `1` with `Desktop IPC is unavailable on this platform`, empty stdout, no remaining pipe (`WaitNamedPipeW`: `ERROR_FILE_NOT_FOUND`), and untouched fixture directory before profile or lock creation. The positive smoke command against these same binaries failed as expected (fixture exit `1`, Gateway exit `1`, same refusal diagnostic).

| Baseline binary | SHA-256 |
|---|---|
| `target/debug/aleph-server.exe` | `cc1bf504f545c50fd222cfb67f5af092e861924b13a8096a19764aa5e9f720fe` |
| `target/debug/aleph-nsh-shell.exe` | `2d9a4d6600e7bad0a1a644a293e051d6ad2842d9fecf8781c4bde3bac7cc7abc` |

## 安全边界 / Security boundary

The candidate accepts a literal local `\\.\pipe\name` only. The initial instance uses `FILE_FLAG_FIRST_PIPE_INSTANCE`, same-SID owner/protected DACL, and `PIPE_REJECT_REMOTE_CLIENTS`. The listener retains at least one live instance while running. During replacement, only transient `ERROR_PIPE_BUSY` is retried (up to the hello timeout, interruptible by shutdown). Exhaustion or another error stops the listener without weakening its security settings. A bounded prelude invokes Win32 impersonation solely to open the peer's token for identification; the thread reverts synchronously before SID extraction or any `.await`. The Client checks the owner of the open pipe object, pins the reported server process handle, reads its token SID, and rechecks PID and liveness. It proves an OS account, not an executable, and does not protect against another process already running as that account or a privileged administrator.

A successful prelude reaches the shared, bounded `system.hello` and RPC loop, the live Core owner mapping, and the common executor and cleanup. The allowlist is unchanged (`health`, `sessions.list`); WebSocket and Unix remain separate transports. The Windows production opt-in remains unavailable unless native security gates are satisfied.

## 首轮真实二进制候选 / Initial real-binary diagnostic build — PASS

A temporary, local-only source change set `shared/client/src/local_ipc.rs::AVAILABLE` to `cfg!(any(unix, windows))` for this build. The code is now back to `cfg!(unix)`; no release or runtime bypass was added. The two gated `cargo build --locked --jobs 2` commands above completed; `python qa/nsh_desktop_ipc_windows.py smoke <gateway.exe> <host.exe> --gateway-log <new log>` returned `0`.

| Diagnostic binary | SHA-256 |
|---|---|
| `aleph-server.exe` | `a8121abb37f76ffb645f72f5658a71525bb4fcb7e3174efa7919a18b910e02c2` |
| `aleph-nsh-shell.exe` | `225c71f26394b1e8d13755526c1f5d288332fe7feff0c56082a0ef1a62d4f31c` |

- **PASS** 同账户真 Gateway/NSH：两轮真实 `--desktop-ipc-status` 均退出 `0`，返回且只返回 `status=healthy` 和非空 `timestamp`，Gateway 未被探针停止。
- **PASS** Mixed flags, QA mode, a missing pipe and a remote endpoint argument were refused; namespace validation on both CLI entrypoints preceded profile creation.
- **PASS** Remote clients: the same account opened an independent owner-only control over real SMB loopback (`127.0.0.1`); the candidate refused the same route with Win32 `ERROR_ACCESS_DENIED` (`5`). This is not a distinct-user test.
- **PASS** On the exact same diagnostic binaries, eight native peer handles with no prelude occupied the listener; the ninth `CreateFileW` got `ERROR_PIPE_BUSY` (`231`). Closing the eight restored a real NSH health response. The extra diagnostic invokes the existing isolated smoke and owned Ctrl-C teardown.
- **PASS** 正常前台退出：两次向该次启动的独占 Gateway 控制台发送真实 Ctrl-C，均退出 `0`；完成 `system.hello` 的 peer 经原生 `PeekNamedPipe` 观察到断连，`WaitNamedPipeW` 返回不存在，NSH 探针失败；第二次独立 Gateway 重绑同名管道并返回真实健康状态。A first peer-EOF fixture used the CRT and reported `EINVAL`; its retry used the native Win32 result, accepting only `ERROR_BROKEN_PIPE` or `ERROR_PIPE_NOT_CONNECTED`.
- **NOT RUN — 本轮只允许当前用户 / current user only.** The user directed testing only as the current Windows account and did not authorize creating or using another. Anonymous identification and SID-mismatch fixtures are not substitutes for an actual second account. If that scope changes, first verify that a pre-existing authorized independent account can open an intentionally permissive control and host a pipe that the foreground account can open; then record Win32 denial on the protected candidate and authentication refusal from the real NSH path before it writes application data. No account or machine policy was changed.

原始证据 / Raw local evidence (`target/s1b-evidence/`, ignored):

| File | SHA-256 |
|---|---|
| `baseline.jsonl` | `724844e9c2a2527c2e5cca5bc9bd987185cc68443e977efa0e62d2c724b18e41` |
| `candidate-retry.log` | `a4b856415416afbc62338b2295d12b8c231f5ff5ce62938353bbd76b6f440f99` |
| `candidate-retry-gateway.log` | `323e0a385d5f77af3699643f8d3f7de3d68e057c3dcc117ade8d4cdbd1d02ee8` |
| `candidate-eof-retry.log` | `f65ec08efd0eedbe92bbc4bcff625dffd37c5e6542cf35f107a04032309356f2` |
| `candidate-eof-retry-gateway.log` | `c133245aba35cd12ced07205aae999aa819b29fe352ed96d74388fbf662cf883` |
| `diagnostic-capacity.py` | `7022765c56d363f23e012f729906f2f36dd9b57300f05902f263b20c79591f5b` |
| `diagnostic-capacity.log` | `1ed65835646784757a6950a1510df0e386db8123e92dbd83a649800d9e717691` |
| `diagnostic-capacity-gateway.log` | `b6157adc8bf209bcf0d6b26501a69e421cee98a10e1c9408d9e4c25f22b8679d` |
| `diagnostic-capacity-first.py` | `15e862f576dcc2a47d3aee9919672feae8caa008c3732b72aa57171669ec64d3` |
| `diagnostic-capacity-first.log` | `eb17bd176131c3c553ae884610316d5023d1850f49bc3865ae4d27f9214f2aa3` |
| `diagnostic-capacity-first-gateway.log` | `b14324e86edbf41f0e0b248f42177bf081c5f851650382664029f643a622fa6e` |

The raw Gateway logs were generated with isolated, credential-free profiles. A scan for bearer values and common API token prefixes found no matches; it is not a general secrets audit.

## 修复后真实二进制 / Replenishment fix on real binaries — PASS

Both locked native builds and the real smoke (with the additional eight-native-peer and occupied-first-instance sequence from `diagnostic-capacity-first.py`) exited `0`. Two fresh Gateways returned canonical health through real NSH; Win32 SMB denial, ninth-client refusal, reconnect after closing eight peers, `Ctrl-C`/established peer EOF and same-name rebind all passed. These executables are retained only under `target/s1b-evidence/final-diagnostic-not-for-distribution/`.

| Diagnostic binary or log | SHA-256 |
|---|---|
| `aleph-server.exe` | `bc5b51ea848910262da8fc93ce6343039f816bf9d1d4b30a71b581bcc392bc8c` |
| `aleph-nsh-shell.exe` | `1827063de896cc41da81c21bce2cf968421bfde1080d2a25c6617e7040d92e1d` |
| `final-diagnostic-smoke.log` | `e13af9fca57707061b7dd27cc75a730a5e153d5b135d434514e20fd0a969be88` |
| `final-diagnostic-gateway.log` | `646209275d40d40cc1d15277f8b6b44b0860b9f08c4b2cbdb757fdc19e53240b` |

## 守卫版验证历史 / Guarded source verification history

The results in this section precede the Windows PTY fixes; the follow-up below supersedes their regression verdict.

Commands below run serially on Windows with the guarded source, `--locked --jobs 2`, and `python qa/browser_live/check_memory.py` before **each** Cargo command. Use a fresh profile and isolated home/config directories for runtime tests. The enabled diagnostic binaries must never be mistaken for the guarded binaries; retained diagnostics are under `target/s1b-evidence/diagnostic-not-for-distribution/`.

```text
cargo test --locked --jobs 2 -p aleph-client
cargo clippy --locked --jobs 2 -p aleph-client --all-targets -- -D warnings
cargo check --locked --jobs 2 -p aleph-cli -p aleph-tui
cargo test --locked --jobs 2 -p alephcore --lib gateway::server:: -- --test-threads=1
cargo test --locked --jobs 2 -p alephcore --test gateway_ui_ingress
cargo test --locked --jobs 2 -p alephcore --bin aleph-server desktop_ipc_is_explicitly_opt_in
cargo test --locked --jobs 2 -p aleph-nsh-shell
cargo build --locked --jobs 2 -p alephcore --bin aleph-server
cargo build --locked --jobs 2 -p aleph-nsh-shell
python qa/nsh_desktop_ipc_windows.py baseline target/debug/aleph-server.exe target/debug/aleph-nsh-shell.exe
```

- **PASS** Client Clippy (`-D warnings`) and CLI/TUI `check`, both exit `0`. The Gateway CLI opt-in selector passed `1/1`; the NSH host suites passed `20 + 6`, both exit `0`.
- **FAIL** `gateway_ui_ingress` compiled, but exited `101`: PTY child output did not reach the screen before the test's first UI RPC (`tests/gateway_ui_ingress.rs:274`, `Elapsed(())`); teardown also reported `TerminalResidual` with Windows OS error `0`. A detached, unchanged checkout at `8c02bb13582d8535b67f04075f58c8a80e68f58b` repeated the exact locked selector on this machine: exit `101`, same PTY timeout at line `274`, same teardown site at line `186` and Windows OS error `0`. This establishes that the failure also occurs without the change, not that actual UI ingress passed. `final-gateway-ui.log` SHA-256 `37c9e71cd211003ac2813495a9e30c5edf78bd4faae3dd5b1cd841d107a2f1df`; `compare-ingress-baseline.log` SHA-256 `9170d4c3a47c55c295f3240d9f1b9a2f1ab5ec51a55062ac2c99756cf0f6f471`.
- **PASS** Client locked rerun exited `0`: 72 passed, 1 ignored. Native tests checked the actual protected single-SID DACL, same-account authentication, original instance remaining usable after a second first-instance attempt, anonymous-peer rejection with restored thread token, and connected-pipe verification with a mismatched expected SID. The shared client test covered out-of-order replies and retention of an already-sent request's slot across timeout. First attempt exited `101` for a missing `serde_json::Value` test import, corrected before Clippy and the rerun.
- **FAIL (ordinary test profile)** Gateway server lib selector exited `101` before running tests: `rustc-LLVM ERROR: out of memory`, `Allocation failed`. A resource-reduced retry preserves the test profile and selector, changing code generation only:

```text
cargo rustc --locked --jobs 2 -p alephcore --profile test --lib --message-format=json -- -C debuginfo=0 -C codegen-units=256
<compiler-artifact executable> gateway::server:: --test-threads=1
```

The first executable retry ran 241 selected cases: 238 passed, 2 failed, 1 ignored. The real Windows pipe protocol/scope and server-cancellation tests passed, as did `test_legacy_ws_first_frame_and_rpc_dispatch`. The saturation test verified the ninth client was denied before failing because it assumed the attempted first-instance creation must return `ERROR_ACCESS_DENIED`. A separate direct Win32 probe with all eight instances allocated returned `ERROR_PIPE_BUSY` (`231`). A second executable rerun accepted the correct capacity error, but then timed out reopening after dropping Tokio clients in the fixture. The separate diagnostic repeated the same saturation and first-instance probes with plain Win32 handles and recovered the real NSH health probe. Changing the Rust fixture to plain OS handles did not resolve the timeout. A smaller native probe linked the actual listener and actual Client transport (application RPC omitted in that probe) and reproduced the listener returning `ERROR_PIPE_BUSY` right after retiring a handle. Mio's server-side overlapped I/O can delay final handle close after Rust drop. The fixed listener waits a bounded time for the kernel slot while retaining the other instances. The smaller probe then accepted a new client and shut down cleanly on all 12 runs (`listener-probe-fixed.log`, SHA-256 `824d6240eeaed3749a171e5a6cdf391485b90733faf91ff4cf3b4d9ff2ab1617`). On the corrected guarded source, all three real Windows transport cases passed, including saturation recovery, protocol/scope, and future cancellation; legacy `/ws` dispatch also passed. The three transport cases independently passed eight further runs (`final-gateway-local-repetitions.log`, SHA-256 `ce80b9c1194f5ce864b3d33228b2cc4e43ab39e9f503d1adffe1e86b5774d5d0`). The full selector remains **FAIL**: 239 passed, 1 failed, 1 ignored, exit `101`. The remaining failure is `terminal_enabled_router_uses_live_intent_and_one_driver` at `src/gateway/server/mod.rs:1755` (`applied.contains(&"policies.terminal")`); the same assertion also failed in an isolated run of the previous candidate. These runs do not establish the cause. Final test executable SHA-256 `bda8f20e9a62a581eb5d49376d73dc360da3f930d74af89c459e944d1f4cf58c`; `server-lib-replenish.log` SHA-256 `164c46b36738ed6c2091875e59c6a1895e527d4097faf8636c9106591e146293`.

### 首轮守卫版实物 / Initial guarded binaries — PASS

Both ordinary locked native builds exited `0`. The refusal fixture exited `0`: both actual opt-ins exited `1` with the exact unavailable diagnostic, no stdout, no pipe, no directory changes and no profile or lock. After the replenishment fix, a timestamp-preserving restore of the source let Cargo reuse the diagnostic executables; the refusal fixture caught this (exit `1`, `final-guarded-refusal-cargo-cache.log` SHA-256 `b2ab81d9b8defb9e753769b98698ea79eaff07649c9a8e2a3e5137669578d6b9`). Those files are quarantined as diagnostic, not guarded binaries. The follow-up build invalidated the restored source timestamp and verified that neither resulting executable matched its diagnostic counterpart before rerunning actual refusal.

| Guarded binary or log | SHA-256 |
|---|---|
| `aleph-server.exe` | `7aae575e16ea70b3ba6dbb028ddf85d93576c155de23d4a0870b31eb90d29751` |
| `aleph-nsh-shell.exe` | `e1380a537ec322fa9c96e8b27eac392fca5fdfeebd5ccb311f5c81070a37a6f0` |
| `final-baseline.log` | `7092a8ab063ba663dbe575d934fde3fb08d3060c026e8b1284cc7b4d7c89fce6` |
| `final-matrix.log` | `8ff14065a67819fbaf0ce214ad1280eccac3d23bda617cc10862216cac8ed7d4` |
| `final-client-rerun.log` | `56e3b4a19015e9549a57604dc59a1a6cbca2c08fcc9149f4bf1a5e4c7a5386ad` |
| `final-cli-opt-in.log` | `d5dd0cc127c0cc6b6443e1cc33c731f8975efb760befb86d173a1aff27c10d5f` |
| `final-nsh-tests.log` | `343639e49b8e5ad17a482c57e584940d4a7888ae572111d8ecf3af1702e67057` |
| `server-lib-first-executed.log` | `46c7c2e5c681b47eed8fe416a67f41a3eee32543d1a93c09f88eec135eb6d9c8` |

The first matrix's aggregate exit was `1`; it includes the failed first Client attempt, the ordinary Gateway unit build OOM, and the UI ingress failure. The rerun on the corrected guarded source passed Client tests (`72` passed, `1` ignored), Client Clippy, CLI/TUI check, Gateway CLI opt-in (`1/1`), and NSH tests (`20 + 6`). `gateway_ui_ingress` still failed at the same PTY timeout and cleanup; this rerun also exited `1`. `guarded-rerun.log` SHA-256 `330685d154dea9e2217fbe3dbda2922c4dd6981715dcfe703748edd23b547d71`; `guarded-rerun-gateway-ui.log` SHA-256 `82c457300671dfe16c4987c7804065553beb34466c4e03a1ac2d98e2e5259e72`.

### PTY 修复前守卫版实物 / Pre-PTY guarded binaries — PASS

Both locked builds and the refusal fixture exited `0`. Each executable exited `1` with `Desktop IPC is unavailable on this platform`, no stdout and no pipe, profile or lock created. The source has `cfg!(unix)` and no diagnostic runtime switch. The binaries are under `target/s1b-evidence/final-guarded/`.

| Final guarded binary or log | SHA-256 |
|---|---|
| `aleph-server.exe` | `e34d4cfb77f3d84e2d59b92fe7c6b792d89de9d054c3970b5354ac14eb91f57e` |
| `aleph-nsh-shell.exe` | `6d03a2e699b49240e33768c17c6cb865a0fca41ca92effe2780a769e0baf881a` |
| `final-guarded-refusal.log` | `e38f49a7d9b6a3b6c79c1a7fa8de70c2859e6c3adc968660af806e29d4f05b68` |

## Windows PTY 复验 / Windows PTY follow-up — PASS

修复了 ConPTY 启动时未应答的光标查询，以及进程终止结果误报；真实 UI ingress 复跑通过。 / The native ConPTY probe reproduced a missing output marker without a reply to `CSI 6 n` and observed the marker with a `CSI 1;1 R` reply. The screen now queues bounded DSR responses, which the PTY reader writes back after releasing the screen lock. For Windows PTY teardown, an owned process handle uses the actual `TerminateProcess` result instead of `portable-pty` 0.9.0's inverted Windows killer result. After those fixes, UI ingress reached the renderer-timeout case before failing: its LF-only Windows probe could match input echo without executing a command. The revised fixture sends a CR-terminated `cmd.exe` command and checks for a marker which is absent from the input echo. It exercises all existing attach, ack, timeout, and disconnect assertions; none were skipped.

| Windows selector | Result | Log SHA-256 |
|---|---|---|
| `gateway::pty::` | PASS: 147 passed, exit `0` | `d56a50c73f1ba63dc246b9de1688da54be24d7ee4f2bdf70544a17bd23007423` |
| `gateway::server::` | PASS: 240 passed, 1 ignored, exit `0`; includes the terminal policy regression | `461e08780769aad4354216e6038959d73c4730ce89ef1f7170bcc135fbf48018` |
| `gateway_ui_ingress` | PASS: 1 passed, exit `0` | `baf9e363b6866e715dd456257d8497fca53f59fced7102be0f5baf37762d25c6` |

The PTY and server selectors used the resource-reduced test binary built with the command above (SHA-256 `251863c9e0b0246940e51f624f3112aa368625c007b1faa7d7b48bb44b6e9e47`). The ingress test used the locked default test profile. Between that reduced-memory build and final verification, the PTY implementation changed only in Rust formatting; the integration fixture also changed its command and assertion as noted above.

**PASS — 当前用户回归 / Current-user transport smoke.** Replayed the real two-process smoke using the retained post-replenishment diagnostic binaries listed above, not the guarded or post-PTY binaries. Both launches returned canonical NSH health; mixed/QA and remote names were refused; the SMB control opened and the candidate returned Win32 `5`; both normal Ctrl-C shutdowns retired the pipe and disconnected the established peer; a fresh Gateway rebound the same name. `current-user-final-diagnostic-smoke.log` SHA-256 `ac6702af5907fa49a9691bf5bd7bc5182cb3194f55450036221e75a1ca294815`; `current-user-final-diagnostic-gateway.log` SHA-256 `9b2f540ae7a6b6f32ced1ccdf592cfd8b8538e99e5ce533ebcc8eb50eca142a9`. This is not alternate-account evidence.

### PTY 修复后守卫版实物 / Post-PTY guarded binaries — PASS

The follow-up locked matrix exited `0`: Client 72 passed and 1 ignored, Client Clippy `-D warnings`, CLI/TUI check, UI ingress 1/1, Gateway CLI opt-in 1/1, NSH 20 + 6, native Gateway and NSH builds, and the production refusal fixture. Both newly checked artifacts refused their opt-in commands with exit `1` and no stdout, pipe, profile or lock. The resource-reduced unit selectors are reported separately above.

| Guarded binary or log | SHA-256 |
|---|---|
| `target/debug/aleph-server.exe` | `68d48909115fee55c1bc8cbe236f522af3afbf8c041557bfa531ab31ac191067` |
| `target/debug/aleph-nsh-shell.exe` | `6d03a2e699b49240e33768c17c6cb865a0fca41ca92effe2780a769e0baf881a` |
| `pty-guarded-matrix/matrix.log` | `15eb1faedb13752b8d7c388c0fee113447ee41634307172f96861a7efe239352` |
| `pty-guarded-matrix/final-baseline.log` | `a3a25cd920777b5ecc2b4d5d80d9c50437ff3177aa8c14e391655c2c35a35481` |

## 交付范围 / Handoff

End `HEAD` remains `8c02bb13582d8535b67f04075f58c8a80e68f58b`: this is a local worktree delivery, not a commit. The guarded source tree excluding this receipt is `5865c51367699756482e4e3b0ce3480dbb9d24f2` (alternate Git index); `post-pty-source.patch` SHA-256 is `a62b5983fb814585867c1d3ee3b7b6c7b56ab7165ece8dee6446f2fc3834b12f`. The pre-PTY guarded tree was `1a1118016ce9482d93355f9a32d8223ff2c00690` (`guarded-source.patch` SHA-256 `6c612f6ca7942da64496c9afdf51dc3ed68dc8a2deb993381972df683f4ceae3`). The primary index and `main` were not changed. `git diff --check`, the checks for the edited PTY Rust modules, and Python compilation passed. Workspace formatting shows unchanged drift; no commit or hook bypass was attempted. **NOT RUN** Independent review (no delegation authorization), alternate-account validation, and Unix runtime regression on this Windows machine. Windows production remains disabled because current-user tests do not prove the alternate-account gate.
