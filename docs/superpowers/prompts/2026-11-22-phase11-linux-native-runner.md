# Phase11 Linux 原生 Runner 执行 Prompt / Linux native runner handoff

请在 **Linux 原生桌面**（物理机或可丢弃 VM，有交互式桌面及用户总线）继续 Aleph Phase11 的 Linux 部分。任务是执行验证并按证据修复，不是重新写计划。不要代做 Windows、全局切换默认、发布或删除 Tauri。

## 接手身份 / Starting identity

- 从交接方的**本地 `main`** 接手。最低含验证检查点 `a9a5aff6ba5f1c035f4117e8306157b769e7cd2e`；如收到更新的交接 HEAD，确认该检查点是其祖先。未推送提交需要先由交接方提供仓库副本或 Git bundle；不要假定 `origin/main` 已含它们，也不要 reset、pull、rebase 或覆盖本地改动。
- 核对 `git rev-parse --show-toplevel`、`git branch --show-current`、`git status --short`、`git log -5 --oneline`。保留用户改动；基线错误或版本不明时先记录阻塞。
- 阅读仓库 `AGENTS.md`，查询已有 Graphify 图（若未随仓转移则记缺失，不重造历史图），然后看 [Phase11 计划](../plans/2026-11-21-phase11-nsh-cutover.md)、[契约](../specs/2026-11-21-phase11-nsh-cutover.md)、[调用点清单](../runs/2026-11-21-phase11-nsh-inventory.json)、[Linux Desktop](../../reference/LINUX_DESKTOP.md)、[Release](../../reference/RELEASE.md) 和 [验证规范](../../reference/DEVELOPMENT.md)。macOS [Quit 回执](../runs/2026-11-22-phase11-nsh-quit-survival-receipt.json)仅作边界参考，不算 Linux 证据。

## 执行 / Execute

1. **证明官方锁定版可跑。** 记录发行版/版本、架构、Rust 工具链、桌面/合成器、`XDG_SESSION_TYPE`、GTK4、WebKitGTK 6、D-Bus、portal 和 Secret Service 的实际可用性。用 `Cargo.lock` 与对应 registry 源验证 `nsh-platform-linux =0.6.0` 带 `webview` feature 的 Rust-hosted `run_app`。此前 macOS 上的离线 Linux 检查缺 `cairo-rs 0.22.11`，不是 Linux 后端失败。先核对本机缓存和实际系统依赖；缺件记包名、版本和退出码，准备依赖前按适用权限处理，不改 pin 或使用旧 fork。用真实窗口、WebView、固定消息往返及 native loop 返回的小实验证明基本可行性。
2. **分别验证真实桌面会话。** 在可用的 X11 和 Wayland 会话分别记录结果；只有一种就把另一种标 `NOT RUN`，不能把 Xvfb、XWayland 或交叉编译当作 Wayland 原生证据。用 `desktop/nsh-shell` 和真实共享 Leptos/WASM Panel 验证 document/request IPC、隔离 OS 存储中的保存/重载/删除、关闭/正常退出、托盘/菜单、热键、URL scheme/第二实例、通知、权限及外链。检查 D-Bus/portal 拒绝或不可用的可见结果；不能用网页结果或 `false` 伪装 OS 功能已验证。单实例和 URL 转发要证明 **Rust 宿主/实际包**端到端生效，不拿官方 runner 代码当结果。
3. **测服务和安装路径。** 在专用测试账户或可丢弃环境隔离 HOME/XDG、WebView profile、Gateway root 和 QA 凭据命名空间；遵守平台真实登录上下文和用户总线要求，不靠伪造 HOME 代替 OS 凭据隔离。full 测随包 `aleph-server` 的真实位置、冷启动、既有服务复用、冲突端口拒绝和监听进程归属；lite/remote 不得启动或停止本地服务。普通 Quit 后须证明同一既有 Gateway 仍就绪。显式 Quit-and-stop、恢复或目标切换若没有生产 handler，如实记缺口，不用手动操作冒充。验证包布局、desktop entry/协议注册、原生辅助件、自启 owner（宿主登录启动 ≠ systemd user Gateway service）、Updater 与回退；保留现有 minisign 信任及 feed。仅在隔离环境使用默认端口与自启/协议注册，不碰已安装服务、真实凭据或系统签名信任。
4. **对实际缺口逐小片修复。** 按调用点先做 SPEC，再做独立 QUALITY 检查及 RED/GREEN，断言实际效果；沿用既有 owner、Panel 和闭合 wire，不加 `__TAURI_INTERNALS__`、第二套服务控制器或替代业务 UI。新增平台性能采样前先冻结合同。每次 Cargo 前跑 `python3 qa/browser_live/check_memory.py --required-bytes 4294967296` 并确认 `ALLOW`，串行 `--offline --locked --jobs 1`（metadata 无 jobs）；构建不与 native QA 并行。离线依赖缺失时不偷偷改 flags 或锁文件，保留命令结果。

## 交付 / Return

交一份 Linux-only 机器回执和简短说明：起止提交及源/锁文件身份、实际包/二进制及原始证据 SHA-256、发行版和桌面依赖、X11/Wayland 分列的 `PASS` / `FAIL` / `NOT RUN`、阻塞命令及退出码、实际原生效果和仅所拥有夹具的退出/清理。保留失败样本；不把秘密或本机绝对路径放进可移植交付。记录 #12 的 Linux 分项，未通过保持开放，不能单方关闭 #12 或宣布 P11-A/B 通过。只提交本地 conventional commits，不推送、打标签、发布、切默认或删 Tauri；真实三平台共同合格后才开始至少 14 个日历日双轨，不能提前记日期。不要读写 `.claude/settings.json`、根 `config.lock` 或根 `config.toml`。
