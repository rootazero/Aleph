# Phase11 Windows 原生 Runner 执行 Prompt / Windows native runner handoff

请直接在 **本机 Windows 原生桌面**（具备交互式用户会话，不要求虚拟机）继续 Aleph Phase11 的 Windows 部分。任务是执行验证并按证据修复，不是重新写计划。不要代做 Linux、全局切换默认、发布或删除 Tauri。

## 接手身份 / Starting identity

- 从交接方的**本地 `main`** 接手。最低含验证检查点 `a9a5aff6ba5f1c035f4117e8306157b769e7cd2e`；如收到更新的交接 HEAD，确认该检查点是其祖先。未推送提交需要先由交接方提供仓库副本或 Git bundle；不要假定 `origin/main` 已含它们，也不要为补齐而 reset、pull、rebase 或覆盖本地改动。
- 核对 `git rev-parse --show-toplevel`、`git branch --show-current`、`git status --short`、`git log -5 --oneline`。保留用户改动；基线不对或版本不明时先记录阻塞，别从陈旧源码开始。
- 阅读仓库 `AGENTS.md`，查询已有 Graphify 图（若未随仓转移则记缺失，不重造历史图），然后看 [Phase11 计划](../plans/2026-11-21-phase11-nsh-cutover.md)、[契约](../specs/2026-11-21-phase11-nsh-cutover.md)、[调用点清单](../runs/2026-11-21-phase11-nsh-inventory.json)、[Windows Runtime](../../reference/WINDOWS_RUNTIME.md)、[Release](../../reference/RELEASE.md) 和 [验证规范](../../reference/DEVELOPMENT.md)。macOS [Quit 回执](../runs/2026-11-22-phase11-nsh-quit-survival-receipt.json)不算 Windows 证据。

## 执行 / Execute

1. **证明锁定版可跑。** 记录 Windows 版本和 CPU、Rust 架构与工具链、WebView2 Runtime 版本、是否为交互式 Runner。用 `Cargo.lock` 和对应 Cargo registry 源确认官方 `nsh-platform-windows =0.6.0` 的 Rust-hosted `run_app`、WebView2 和平台接口。先做可撤销的最小本机探测：打开真实窗口、渲染 WebView、实际往返固定类型消息、关闭并观察 native loop 返回。仅编译、CI 服务会话或普通浏览器不算。不能用旧 fork、替代 WebView 或静默升级 toolkit。
2. **测实际 Aleph 入口和服务边界。** 在本机用全新测试 profile、自有且独占的 Gateway 和随机 QA 凭据命名空间，不使用现有安装、服务或生产凭据。用真实共享 Leptos/WASM Panel 验证 `desktop/nsh-shell` 的现有 full/lite 入口。分别记录 full 随包 `aleph-server.exe` 的确切查找位置、冷启动、既有服务复用、冲突端口拒绝和监听进程归属；lite/remote 不得启动或停止本地 Gateway。只有确认默认端口及其服务状态不属于现有安装时，才用它做本机验收；否则记录该用例未运行，改用独占回环端口验证 lite / remote，不接触已安装服务。观察普通 Quit 后同一实例仍就绪；显式 Quit-and-stop、恢复、目标切换若缺生产 handler，应报告缺口，不能用手动停止代替。区分 OS 停止机制是否强制，不把关闭端口当进程退出。
3. **逐项对照原生消费者。** 实测 WebView2 固定 document/request IPC、隔离 OS 凭据保存/重载/删除、关闭与正常退出、托盘/菜单/热键、URL scheme 和第二实例、通知、权限与外链。单实例和 URL 转发要证明 **Rust 宿主/实际包**端到端生效，不能用官方 runner 的实现代替。核对 Windows 自启状态语义（宿主登录启动 ≠ Gateway 计划任务）、full/lite 安装布局、原生辅助件、Updater 与回退的当前 owner；保留现有 minisign 信任和 feed。在 QA 环境以外不注册协议、自启或安装正式服务，不使用生产凭据或签名密钥。
4. **对缺口逐小片修复。** 按真实调用点先做 SPEC，再做独立 QUALITY 检查及 RED/GREEN，写实际效果检查。沿用现有 owner、Panel 和闭合 wire；不要造 `__TAURI_INTERNALS__`、替代业务 UI、第二套服务控制器或混入 Linux/macOS 实现。新增平台性能采样前先冻结合同。每次 Cargo 前跑 `python qa/browser_live/check_memory.py --required-bytes 4294967296` 并确认 `ALLOW`，串行 `--offline --locked --jobs 1`（metadata 无 jobs）；若脚本或依赖缓存不支持本机，保留错误，先核实本机量具/依赖准备方案，不静默绕过。构建与 native QA 不并行。

## 交付 / Return

交一份 Windows-only 机器回执和简短说明：起止提交及源/锁文件身份、实际包/二进制及原始证据 SHA-256、版本与环境、逐项 `PASS` / `FAIL` / `NOT RUN`、阻塞命令和退出码、真实窗口/消息/服务或原生效果、仅所拥有夹具的退出和清理证据。保留失败样本，不复制秘密或本机绝对路径到可移植交付。记录 #12 的 Windows 分项，未通过就保持开放，不能单方关闭 #12 或把 P11-A/B 标成通过。只提交本地 conventional commits，不推送、打标签、发布、切默认或删 Tauri；真实三平台共同合格后才开始至少 14 个日历日双轨，不能提前记日期。不要读写 `.claude/settings.json`、根 `config.lock` 或根 `config.toml`。
