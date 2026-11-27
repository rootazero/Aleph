# Linux 真机验收 Prompt / Linux real-machine QA prompt

把本文件交给 **Arch-hypr 本机、登录了真实图形会话**的测试代理。/ Give this prompt to the testing agent on Arch-hypr with an active graphical session.

## 任务 / Mission

你负责 Aleph R7 的 Linux 发版候选验收。用 GitHub `origin/main` 的同一提交完成构建、官方 NSH 打包和启动；以真实 UI 行为为准，不以 `cargo run`、SSH 无头启动或单独启动服务替代。**不删除 Tauri、不发版、不提交修复。** / Validate the exact GitHub commit, using the production NSH package in a real desktop session. Do not remove Tauri, publish, or commit fixes.

## 准备与隔离 / Setup and isolation

1. 在现有仓库执行 `git fetch origin main`、`git rev-parse FETCH_HEAD`。用 `git worktree add --detach <新目录> FETCH_HEAD` 建独立检出，确认包含 R7，记录完整 SHA 和工作树状态。不要重置 `main`、覆盖改动或复用 LAN 预检旧包。/ Fetch from GitHub and test a clean detached worktree at the recorded SHA.
2. 记录发行版、架构、Rust 和 `nsh --version`（必须为 `0.6.0`）、WebKitGTK **6.0 API**（Arch 包 `webkitgtk-6.0`，`pkg-config --modversion webkitgtk-6.0`）、合成器和托盘宿主。沿用实际图形会话的 `WAYLAND_DISPLAY`、`XDG_RUNTIME_DIR`、用户 D-Bus；Hyprland + Waybar 时核对 StatusNotifierWatcher。缺依赖或权限先报告，不静默改系统。/ Record toolchain and session; report missing dependencies instead of changing the system silently.
3. 另建临时 `HOME`、`ALEPH_HOME`、XDG 配置/数据目录；保留原会话的运行时 socket。使用合成数据或只读 SQLite backup 复制 `sessions.db` 和 `teams.db` 到临时目录；不复制凭据，不输出正文、标题、token 或配置秘密。打包和运行前核对 `127.0.0.1:18790` 进程归属：如被现用服务占用，不停它；先做高端口 Remote 预检，Local 标 `BLOCKED`，改用空闲的隔离桌面环境完成才计入通过。/ Keep data and credentials isolated; never stop the existing production service.

## 必验项 / Required checks

- **L1 构建与包 / Build and archive.** 从检出的根目录执行 `scripts/nsh_package.sh --target linux --out "$PWD/target/r7-linux-qa"`。检查归档 SHA-256、解包后的 `Aleph` 和相邻 `aleph-server`、桌面文件及其 `Icon=ai.aleph.desktop` 对应图标资源。按 `AGENTS.md`/开发指南运行 `cargo test -p aleph-nsh-shell -p aleph-desktop-host -p aleph-protocol`，汇报失败或跳过。只启动该次归档解出的包，不从源码启动 UI、不沿用旧 sidecar。/ Build, hash, unpack, and run exactly this archive.
- **L2 Local 首启 / Local cold start.** 暂无保存目标，启动 NSH；观察 Starting → Panel，记录冷启动耗时、窗口和两个进程的路径。确认监听 18790 的是包内 sidecar，`/health`、`/ready` 为 200，`/ready` 有 `x-aleph-panel-shell: nsh-v1`，`/` 返回随包的 Panel。WebView 实际加载、已授权回环不要求 token、可发送一条非敏感消息。短暂运行测试 Agent，验证运行数 `0 → 1 → 0`。在隔离环境制造一次启动失败并移除原因，验证 Retry 恢复且无重复 sidecar；重启后打开旧单聊，再进入群聊、回到同一单聊，验证原内容和历史翻页恢复；切换 Agent 验证对应群聊更新。/ Prove readiness, chat, history, groups, and live run state in the actual UI.
- **L3 单实例、托盘与品牌 / Single instance, tray, branding.** 连开两次同一二进制，按可执行文件路径核对仅一个进程和一个窗口，已有实例被唤回。关窗只隐藏；从托盘恢复，支持时测试全局快捷键（默认 `mod+shift+a`，即 `Ctrl+Shift+A`）。确认启动器与 Waybar/托盘实际显示 Aleph 图标而非 NSH 占位符；记录可见截图和该 PID 的托盘注册。验证菜单 Reload、Open in Browser、连接设置及普通外链/产物链接在外部打开。/ Verify behavior and rendered logos, not just the files in the archive.
- **L4 连接与授权 / Targets and authorization.** 在同机高端口启动第二个隔离 Gateway。切换 Remote，确认 Local 已停、Remote 仍在线；重启保留目标，模拟 Remote 不可达显示连接表单且不误启 Local，切回 Local 可恢复。若有受信任 HTTPS 测试 Gateway，验证未经授权要配对，配对后重启保持、移除仅测试凭据；非回环明文 HTTP 与不受系统信任的 TLS 必须被拒绝。不关闭 TLS 校验或系统安全设置。/ Exercise target persistence, fail-closed TLS, and credential lifecycle in test scope.
- **L5 生命周期 / Lifecycle.** `Quit & Stop Aleph` 停掉已接纳的测试 Local，确认测试 shell、sidecar 和监听都消失；Remote 时只退出 shell、Remote 保持工作。若可用，另用隔离 v26.9.1 验证旧服务提示换宿主、不被 NSH 接管或停止。验证自启动打开、系统登记、关闭复原；仅在有麦克风/权限时验证授权与实际录音输入。检查通知点击回到应用；如已在临时环境注册 `aleph://open`，验证冷启动和已有实例唤回。缺平台实现或设备就写 `SKIP`/`FAIL` 及原因，不冒充通过。/ Verify ownership boundaries; restore any QA-only OS settings.
- **L6 更新与回退 / Update and rollback.** 核对 `docs/reference/RELEASE.md` 中 v26.9.1 Linux 签名回退资产的名称与 SHA-256，按文档用 v26.9.1 公钥验证同名签名；仅在有测试签名夹具时演练隔离 N→N+1 更新、服务替换后重新就绪及坏签名拒绝，不接触生产密钥或发布 feed。否则记录 `SKIP`。/ Verify the documented rollback asset; use only disposable update fixtures.

## 回执 / Receipt

逐项列 `PASS / FAIL / BLOCKED / SKIP`，给出 SHA、完整命令、工具/桌面版本、包哈希、冷启动秒数、进程路径、脱敏日志与截图路径、复现步骤和清理结果。**L1–L3 都通过才报告 Linux 打包启动门禁通过**；其他项的失败/跳过独立列出，不能遗漏。只清理自己启动的进程、测试目标和测试 OS 设置；不给「删除 Tauri」下指令。/ Report evidence, limits, and cleanup; do not authorize Tauri removal.
