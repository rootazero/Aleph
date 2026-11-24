# NSH Panel S1-B — Windows Named Pipe 实施与原生验收 Prompt / Windows native implementation and verification handoff

你在 **原生 Windows**（物理机或运行 Windows 的可丢弃 VM）上接手 Aleph NSH Panel 的 S1-B Windows 分项。**当前基线没有 Named Pipe，只会拒绝。** 先确认这个基线，再按现有 S1-B 契约实现 Windows 传输并验收。拒绝路径通过不等于 Named Pipe 可用；交叉编译、WSL、假管道服务或只跑 mock 不算原生交付。

**English scope:** Implement and verify the opt-in, read-only Windows Named Pipe transport against the real Gateway and NSH binaries. Preserve Unix and legacy WebSocket behavior. This is not a desktop UI, installer, service-management, or release task.

## 接手与隔离 / Starting point and isolation

1. 接收交接方的本地 `nsh-panel-refactor` 检出或 Git bundle。从 `8c02bb13582d8535b67f04075f58c8a80e68f58b` 或已核实的后继提交创建独立 worktree 和分支（如 `s1b-windows-named-pipe`）。运行 `git merge-base --is-ancestor 06e2c1bad3f23eb6647e4f3fe4f37b3c39ba0c43 HEAD`，记录 `git status --short --branch`、实际提交与 tree、`Cargo.lock` SHA-256。基线取不到就报告阻塞，不从远端 `main` 猜测、不 reset/rebase 或覆盖用户改动。
2. 阅读仓库 `AGENTS.md`、[S1-B 计划 Task 4](../plans/2026-11-23-nsh-panel-s1b-desktop-ipc.md#task-4-named-pipe-platform-receipts-and-final-review--windows-与证据)、[S1-B 回执及 Windows 拒绝复跑](../runs/2026-11-23-nsh-panel-s1b-desktop-ipc-verification.md)、[协议](../../research/NSH_PROTOCOL_V1.md)、[Windows 构建说明](../../reference/WINDOWS_RUNTIME.md#build-from-source) 和[验证规范](../../reference/DEVELOPMENT.md)。Windows API 以 [Microsoft Named Pipe Security and Access Rights](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights) 及当前锁定的 Tokio、Windows binding 为准。
3. 使用授权的非管理员测试账户、全新隔离 `ALEPH_HOME` 与工作目录、独占的随机本地管道名、非默认 loopback 端口。只管理本次夹具的子进程、管道和文件。第二账户测试只用事先授权的隔离身份；未经授权不创建用户、不修改系统策略、防火墙、服务、注册表或正式自启。不得读取真实会话或凭据。记录 Windows 构建号、CPU/目标三元组、Rust/Cargo、MSVC、`protoc`、Python；锁定 NSH `=0.6.0`，依赖缺失先报告，不静默安装或升级。

## 先跑拒绝基线 / Record refusal baseline

从已核对的 worktree 根，在无 `ALEPH_NSH_QA_*` / `ALEPH_PANEL_QA_STATUS` 的独立 PowerShell 会话构建真实二进制。确认使用默认 Cargo target 目录；如果有本机 `CARGO_TARGET_DIR` override，将下列路径换成实际产物位置。每次 Cargo 前运行 `python qa/browser_live/check_memory.py`，只在 `ALLOW` 后继续；默认 4 GiB，大链接建议预留 10 GiB。串行 `--locked --jobs 2`。下例仅在确认源码仍有早期拒绝分支时运行；给两次调用设外部超时，保留输出。若出现服务启动等异常，只退出本次夹具进程：

```powershell
function Invoke-GatedCargo {
    & python qa/browser_live/check_memory.py
    if ($LASTEXITCODE -ne 0) { throw "Memory gate refused: $LASTEXITCODE" }
    & cargo @args
    if ($LASTEXITCODE -ne 0) { throw "Cargo failed: $LASTEXITCODE" }
}
Invoke-GatedCargo build --locked --jobs 2 -p alephcore --bin aleph-server
Invoke-GatedCargo build --locked --jobs 2 -p aleph-nsh-shell
$fixture = Join-Path ([IO.Path]::GetTempPath()) ("aleph-s1b-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $fixture -ErrorAction Stop | Out-Null
$env:ALEPH_HOME = Join-Path $fixture 'profile'
$pipe = '\\.\pipe\aleph-s1b-' + [guid]::NewGuid()
& .\target\debug\aleph-server.exe --bind 127.0.0.1 --port 0 --desktop-ipc $pipe start
$serverRefusal = $LASTEXITCODE
& .\target\debug\aleph-nsh-shell.exe --desktop-ipc-status $pipe
$hostRefusal = $LASTEXITCODE
if ($serverRefusal -eq 0 -or $hostRefusal -eq 0) { throw 'Expected baseline refusal on both sides' }
```

两者都应报 `Desktop IPC is unavailable on this platform`。另核对未创建管道、未启动 Gateway、未触及 profile 或持有单实例锁。若只得到参数错误、二进制没启动或命中了其他服务，记录为失败或阻塞，不作通过证据。

## 实现边界 / Implementation contract

- **先写可失败的原生验收，然后接通真实入口。** 从 `src/bin/aleph-server/{main.rs,cli.rs,commands/start/mod.rs}`、`src/gateway/server/mod.rs::run_with_desktop_ipc` 和 `desktop/nsh-shell/src/ipc_status.rs` 跟真实调用图；只在生产 Windows 路径可用后移除对应拒绝分支。保留未 opt-in、其他不支持平台及 `--desktop-ipc-status` 混合/QA 拒绝；不做自动启动，没有 TCP/WS fallback。管道名只接受显式本地 `\\.\pipe\…`，不接受远端或其他设备命名空间，不把它交给 Unix 文件路径安全策略。
- **双向证明同一个账户，不证明应用身份。** 创建时使用只允许预期账户 SID 的限制性 DACL，在连接层拒绝远程客户端。占用名必须失败且不干扰原管道；首实例检查与实例补充必须保证没有可被第三方接管的空档，否则停止服务而不是降级。Gateway 根据实际 pipe peer 的 Windows token 校验客户端 SID，在认证前只允许必要且有超时/资源上限的 I/O，不解析业务或执行 RPC。使用 impersonation 时限定为同步短作用域，在所有退出路径恢复线程身份，不跨 `.await`。客户端也要对已连接管道的服务端账户做内核身份核对；若依赖 PID/token，论证查询竞态处理，不把名称、PID 字面值或客户端自报 SID 当成证明。任何无法访问或核验的 token/SID 拒绝。沿用锁定 Windows binding，避免复制 OS wrapper。
- **接同一条协议和业务路径。** 复用 `shared/client/src/local_ipc.rs` 的帧/JSON 边界、`AlephClient::connect_local` 的逐连接请求关联与超时语义，以及 `src/gateway/server/connection/local_ipc.rs` 的 hello/RPC 生命周期。避免在 Unix 与 Windows 各复制一套状态机。同账户准入后只映射 `src/gateway/server/connection/dispatch.rs::active_desktop_owner`，使用共享 `RpcExecutor` 和 `cleanup::run_cleanup`；不信任 loopback、已配对设备或参数宣称的身份。不扩充 allowlist：仅 `health`、`sessions.list`，首 application RPC 必须是 `system.hello`。保持 4-byte BE JSON framing、8 MiB 帧、8 个连接、每连接最多 8 个在途请求、16 MiB 聚合入站帧预算，以及共享结果形状检测；不广告 replay/snapshot。记录 peer 断开、listener 停止、服务 future 取消/握手失败时的实例释放和共享状态清理。

## 原生验收与回传 / Native acceptance and return

在原生 Windows 用同一最终提交串行跑锁定 Client 测试和 Clippy `-D warnings`、CLI/TUI 检查、Gateway server lib 子集、`gateway_ui_ingress`、`desktop_ipc_is_explicitly_opt_in`、NSH host tests，以及两真实二进制的构建。测试命令可复用 [S1-B Linux 命令清单](2026-11-23-nsh-panel-s1b-linux-native-runner.md#执行--run)的 Cargo 选择器，改用上述 PowerShell 闸门。不要在 Windows 运行 Unix smoke Python；增加独立 Windows 夹具，结束时只回收自己启动的进程。至少提供以下实效：

| 验收项 / Gate | 必须观察 / Required evidence |
|---|---|
| 实际端到端 | 直启独占 Gateway → 真 `aleph-nsh-shell.exe --desktop-ipc-status` 返回 canonical `status=healthy` 和时间戳 → Gateway 仍存活。无管道、QA、混合参数均拒绝，日志/产物不含身份凭据。 |
| 准入 | 核对创建实例的实际 DACL 及 remote rejection；原生证明同账户成功、已授权的隔离异账户被拒、客户端拒绝无法验证的服务端身份，以及占用名拒绝且原实例保持正常。静态选项检查不能充当未跑的异账户或网络来源验证。 |
| 协议与作用域 | 先 hello，版本/形状/边界错误和不允许的方法被拒；真实 `sessions.list` 只读 own fixtures；乱序关联正确；重复活跃 ID 被拒；超时不提前归还已发送槽位，EOF/服务取消释放连接行；旧 `/ws` `connect` 和实际 ingress 保持不变。 |
| 正常退出 | 使用仓库支持的前台正常退出路径，对拥有的 Gateway 验证 peer EOF、监听实例释放、后续探针拒绝；新测试实例可重新启动。若只验证了强制终止，分开记录，不计作正常清理。 |

交一份 Windows-only 回执至 `docs/superpowers/runs/`：起止提交/tree、锁文件与实际二进制 SHA-256、OS/架构/工具版本、脱敏命令/退出码/工件哈希、逐项 `PASS` / `FAIL` / `NOT RUN`、缺失的原生测试条件和复跑方法。未证明的安全边界则维持不可用，不用 mock 或 macOS 结果替代。按仓库说明对精确 diff 做独立只读审阅（如获得委派授权）；重点看 DACL/SID 证明、异步 impersonation 边界、实例所有权和复用的 cleanup。更新当前状态文档时只更改有证据的 Windows 分项；可本地提交，不推送、不合并、不进 `main`。

**Do not claim:** Linux 验收、三平台整体通过、Windows GUI 或 WebView2 功能、S2、切默认、发布、安装、自启或 Tauri 退场。
