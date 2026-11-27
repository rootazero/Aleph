# Windows 真机验收 Prompt / Windows real-machine QA prompt

把本文件交给 **Windows11 本机、登录了真实交互桌面**的测试代理。/ Give this prompt to the testing agent on Windows11 with an active interactive desktop.

## 任务 / Mission

你负责 Aleph R7 的 Windows 发版候选验收。用 GitHub `origin/main` 的同一提交构建、官方 NSH 打包并启动；不能用 `cargo run`、SSH Session 0 的进程、单独手启 sidecar 或旧品牌 ZIP 代替。**不删除 Tauri、不发版、不提交修复。** / Validate the exact GitHub commit using the production package on the interactive desktop. Do not remove Tauri, publish, or commit fixes.

## 准备与隔离 / Setup and isolation

1. 在本地仓库执行 `git fetch origin main` 和 `git rev-parse FETCH_HEAD`，用 `git worktree add --detach <新目录> FETCH_HEAD` 建独立检出。保留现有 `main` 的未提交修改、应用和数据，记录完整 SHA 与干净工作树。/ Fetch from GitHub and test a clean detached worktree without modifying the existing checkout.
2. 记录 Windows 版本、架构、Rust/MSVC 工具链、WebView2 Runtime 版本、`nsh --version`（必须为 `0.6.0`）及 Git Bash。Scoop 机器找不到 Cargo 时，核对 `C:\Users\zou\scoop\persist\rustup\.cargo`、`.rustup` 和其 `bin`；NSH 通常在 `%LOCALAPPDATA%\nsh\bin`。不要为测试静默改系统配置。/ Verify the toolchain and settings in the actual desktop session.
3. 使用独立测试账户或临时用户配置和 `ALEPH_HOME`；先确认 NSH 不读取生产 `~/.aleph/.desktop-shell-target`。不导入生产凭据。需要迁移样本时，只读 SQLite backup 复制 `sessions.db` 与 `teams.db`，或用合成夹具。确认 `Get-NetTCPConnection -State Listen -LocalPort 18790` 所有权；如果生产服务占用，不停它，换空闲隔离环境做 Local，高端口 Remote 只算预检。避免通知、启动项或协议关联覆盖现有安装。/ Isolate the profile and data, preserve any production service, and never expose credential or conversation contents.

## 必验项 / Required checks

- **W1 构建与包 / Build and archive.** 在检出根目录用 Git Bash 执行 `scripts/nsh_package.sh --target windows --out "$PWD/target/r7-windows-qa"`。用 `Get-FileHash -Algorithm SHA256` 记录 ZIP，解包检查 `Aleph.exe`、同目录 `aleph-server.exe`、PE 关联图标及托盘资产。Inno Setup 存在时可另测安装包；没有时记录并以 ZIP 验收。按项目开发指南运行 `cargo test -p aleph-nsh-shell -p aleph-desktop-host -p aleph-protocol`，不得隐藏失败。只启动本次归档解出的成品。/ Build, hash, unpack, and launch exactly this archive.
- **W2 Local 冷启动（重点） / Local cold start (priority).** 目标为 Local，确认 18790 起初没有监听。在交互桌面记下启动时间、应用 PID/SessionId/窗口句柄及 sidecar 路径；用 `curl.exe` 查 `/health`、`/ready`、`/`，确认包内 sidecar 监听，HTTP 200，`/ready` 有 `x-aleph-panel-shell: nsh-v1`，WebView 进入 Panel。在隔离环境验证失败界面的 Retry 能恢复且无重复 sidecar；至少一次停止后重新冷启，明确复验 `5c9ce5d46`：Windows 曾在约 2.01–2.07 秒才返回 `ConnectionRefused`，旧 2 秒探测导致直接报 `cannot establish compatible local Gateway readiness`。若失败，保留窗口的错误文字、端口归属、日志和计时；先记录包内自动启动的失败，再用另一个高端口手启做诊断，不把手启算通过。/ Confirm the packaged shell starts its sidecar on a free port without manual help.
- **W3 单实例、托盘与品牌 / Single instance, tray, branding.** 二次打开同一路径，核对第二进程退出、仅一个实例/窗口存活并被唤回。验证关窗只隐藏、通知区域（含溢出区）实际显示 Aleph 图标、托盘唤回、支持时全局快捷键（默认 `mod+shift+a`，即 `Ctrl+Shift+A`）；检查资源管理器/任务栏图标不是占位符。检查菜单 Reload、Open in Browser、连接设置，以及普通外链和产物通过默认浏览器打开。/ Capture actual desktop behavior and icon rendering.
- **W4 Panel 回归 / Panel regressions.** 已授权本机回环直进 Panel，无 token 弹窗；可以发送一条测试消息。使用隔离历史同时看到单聊与群聊，打开旧单聊 → 进入群聊 → 返回同一单聊，验证原内容和历史翻页恢复，切 Agent 后群聊列表更新。触发并结束一个测试 Agent run，确认计数 `0 → 1 → 0`。不截图或记录真实标题、内容、token。/ Verify authorized loopback, direct and group history, and live activity.
- **W5 连接、权限与退出 / Targets, permissions, and quit.** 连接同机高端口的测试 Gateway，验证 Local 停止、Remote 不受影响、重启保持、Remote 不可达展示连接表单且不误启 Local、切回 Local 后恢复。若有受信任 HTTPS 夹具，验证配对/持久化/仅撤销测试凭据；非回环 HTTP 与不受信任的 TLS 必须拒绝，不绕过校验。Local 的 `Quit & Stop Aleph` 必须退出应用、停掉已接纳 sidecar 并释放端口；Remote 时只退出应用。不让 NSH 停掉不兼容旧服务：可用隔离 v26.9.1 观察提示和旧进程存活。验证自启动打开、系统登记、关闭复原；测试实际通知点击、可用麦克风录音、临时注册的 `aleph://open` 冷/热启动。权限或平台能力不足要说明。/ Exercise target persistence, fail-closed admission, and desktop integration.
- **W6 更新与回退 / Update and rollback.** 核对 `docs/reference/RELEASE.md` 中 v26.9.1 Windows 签名回退资产的名称与 SHA-256，按文档用 v26.9.1 公钥验证同名签名。优先在一次性目录和本地测试 feed 演练 N→N+1：签名验过后停止旧服务、替换、重启并确认新服务来自新包，坏签名拒绝且旧包不变。无测试签名夹具则标 `SKIP`；不读生产密钥、不改发布 feed、不在线发版。/ Test the Windows updater only with disposable fixtures; record anything unverified.

## 回执 / Receipt

逐项列 `PASS / FAIL / BLOCKED / SKIP`，附 SHA、完整命令、工具/系统版本、包哈希、冷启动时长、PID/进程路径/端口归属、脱敏日志和截图路径、复现与清理。**W1–W4 都通过才报告 Windows 打包启动门禁通过**；W5–W6 各自列结果。只终止本次 QA 进程，撤销测试启动项/协议关联/权限和测试凭据，不给「删除 Tauri」下指令。/ Report results and remaining limits; clean up only QA-owned state.
