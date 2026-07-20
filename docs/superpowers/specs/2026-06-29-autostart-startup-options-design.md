# 开机自启动(Startup / Launch-at-login)设计

> Date: 2026-06-29 · Status: Approved (brainstorm) · 下一步: writing-plans

## 1. 背景与目标

为 Aleph 三个分发产品提供"开机自启动"能力,各平台用各自原生机制:

- **完整桌面 App**(Tauri 壳 + 内置 `aleph-server`):登录时拉起 App。开关放 **设置 → 通用**。
- **Panel 纯壳 App**(Tauri 壳,连局域网 server):登录时拉起 Panel 壳。开关放 **设置 → 通用** 的"本机 / This device"小节(与完整 App 同一套代码)。
- **独立 `aleph-server` 二进制**(`install.sh`/`install.ps1` 安装):开机拉起无头 daemon,**所有平台默认开启**。

### 核心区分:两套机制,不混用

| | App(完整 + Panel 纯壳) | 独立 server |
|---|---|---|
| 自启含义 | 登录时拉起 GUI | 开机/登录拉起无头 daemon |
| 机制 | `tauri-plugin-autostart`(已集成) | 系统服务(launchd / systemd / 任务计划) |
| 开关入口 | Panel → Tauri IPC | `aleph-server service` 子命令 + install 脚本 |
| 默认值 | **关**(手动开) | **开** |
| 实现层 | `desktop/shell/src/` + `interfaces/webchat/` | `src/bin/aleph-server/commands/service.rs` |

## 2. 现状(探索结论)

- `desktop/shell/Cargo.toml`:`tauri-plugin-autostart` v2 **已是依赖**;插件已在 `main.rs` 注册(`MacosLauncher::LaunchAgent`)。
- `desktop/shell/src/main.rs:911 ensure_autostart()`:**首次运行就自动开启**自启(写 marker 文件,之后尊重 OS 开关)。完整壳 marker = `~/.aleph/.desktop-shell-autostart`,lite 壳 = `~/.aleph/.desktop-shell-panel-autostart`。
- Tauri 命令已有:`connection::get/set/clear_connection_target`、`connection::is_lite_shell`、`connect_setup::*`。**说明 Panel→Tauri invoke 通路存在**(待规划期 grep 确认调用点)。
- Panel 设置:`interfaces/webchat/src/platform/wide/views/settings/general.rs` 的 `GeneralView`;设置走 `<domain>_config.get/update` RPC **打到 server**。
- 桌面检测:`is_native_shell()`(检 `data-shell="aleph-tauri"`)— 可门控桌面专属 UI。无既有"桌面专属设置"先例。
- 独立 server:`Scripts/install.sh` / `Scripts/install.ps1` **不注册任何自启**;`--daemon` 双 fork+setsid 是 **Unix-only**,Windows 明确报错;仓库无任何 `.service`/`.plist`/任务计划描述符(仅有未实现的 BDD 草稿 `tests/features/daemon/launchd.feature`)。CLI 已有嵌套子命令范式(`PluginAction`/`GatewayAction`/`SecretAction` …)。

## 3. App 产品设计(完整 + Panel 纯壳共用)

### 3.1 壳侧(`desktop/shell/src/`)

新增两个 Tauri 命令,在**完整壳与 lite 壳都注册**:

- `get_autostart() -> bool` → `app.autolaunch().is_enabled()`
- `set_autostart(enabled: bool) -> Result<()>` → `app.autolaunch().enable() / disable()`

**移除** `ensure_autostart()` 的首次运行自动开启(落实"默认关")。
- 不主动 `disable()` 老用户 → `is_enabled()` 如实反映 OS 状态,已开启用户的开关初始显示为"开"。
- marker 文件机制(`.desktop-shell-autostart` / `.desktop-shell-panel-autostart`)随之废弃。

### 3.2 Panel 侧(`interfaces/webchat/.../settings/general.rs`)

`GeneralView` 内新增"本机 / This device"小节:

- 用 `is_native_shell()` 门控:仅 Tauri 壳内显示;浏览器 / 远程访问时隐藏。
- 一个"登录时启动 Aleph"开关,读写走 `window.__TAURI__.invoke('get_autostart' / 'set_autostart')`,**不走 server 的 `general_config` RPC**。
- 文案按产品区分(由 `is_lite_shell()` 判定):
  - 完整 App:"登录时启动 Aleph(含本机服务)"
  - Panel 纯壳:"登录时启动 Aleph Panel"
- 视觉上明确与 server 配置区隔(Panel 纯壳里通用页其余项是远程大脑配置)。

### 3.3 架构决定(显式记录)

- **R4 读法**:Panel 一般只发 JSON-RPC 给 server;此处是**本机设备设置**特例走 Tauri IPC。UI 仍在 Panel(R2 满足)、系统调用仍在壳(R1 满足),非业务逻辑,不算违 R4。
- **前置核实(规划第一步)**:确认 Panel 能调 Tauri 命令(grep `connection::set_connection_target` 的调用点)。若不成立,Panel 纯壳回退**系统托盘菜单项**方案。

## 4. 独立 server 设计

### 4.1 `aleph-server service` 子命令

新增嵌套子命令(`src/bin/aleph-server/cli.rs` 加 `Command::Service` + `ServiceAction`;`src/bin/aleph-server/commands/service.rs` 加处理模块),沿用现有 `PluginAction`/`GatewayAction` 范式:

| 子命令 | 行为 |
|--------|------|
| `service install` | 写平台服务描述符 + enable + 立即启动 |
| `service uninstall` | 停止 + 删除描述符 |
| `service enable` / `disable` | 仅切换开机启动,不动当前进程 |
| `service status` | 报告已安装 / 已启用 / 运行中 |

**服务运行前台 `aleph-server start`(不带 `-d`)**,由 OS 服务管理器监管存活。收益:
- 绕开 "Windows 不支持 `--daemon`"(根本不用 daemon 模式)。
- 给 macOS 一个 launchd 管理的**稳定身份**,缓解 ad-hoc daemon 触发的本地网络隐私 TCC 拒绝。

二进制绝对路径用 `std::env::current_exe()` 取;config 照常读 `~/.aleph`(默认 127.0.0.1;`[gateway] host="0.0.0.0"` 开 LAN);服务不额外塞参数。

### 4.2 install 脚本集成(默认开)

- `install.sh` / `install.ps1` 放完二进制后调 `aleph-server service install`(默认开)。
- 提供 `--no-autostart` 退出开关。
- 关闭路径:`aleph-server service disable` 或 `service uninstall`。

### 4.3 架构决定(显式记录)

- **R1/R3 读法**:服务逻辑只用 `std::process::Command` 调 `launchctl/systemctl/schtasks` + 写描述符文件,**不链接任何平台 API crate**(无 objc / windows-rs);且在 bin crate 不在 core library,无重依赖。符合 R1 意图与 R3。

## 5. 各平台 server 机制(`service install` 落地细节)

| 平台 | 描述符 | 触发 | 备注 |
|------|--------|------|------|
| **macOS** | `~/Library/LaunchAgents/ai.aleph.server.plist`,label `ai.aleph.server`(与 App 的 `ai.aleph.desktop` 区分) | `RunAtLoad` + `KeepAlive`,登录即起 | LaunchAgent(per-user),非 LaunchDaemon(root/全局) |
| **Linux** | `~/.config/systemd/user/aleph-server.service`,`Type=simple` | `systemctl --user enable` + 尝试 `loginctl enable-linger` | linger 让服务**开机即起、无需登录**;若 linger 需 root 拿不到,回退"登录时起"并提示用户手动 `enable-linger` |
| **Windows** | 任务计划程序 logon task,名 `Aleph\aleph-server` | 触发器 = 用户登录,隐藏窗口跑前台 `aleph-server start` | 依 WINDOWS_RUNTIME.md 推荐路径;非 Windows Service(免 admin / NSSM) |

## 6. 边界 / 非目标 / 测试

### 边界
- **flock 单例**:同机既装完整 App 又装独立 server → 第二个抢不到 `~/.aleph/data/aleph.lock` 启动失败(已有行为)。文档提示即可,不在本设计处理。

### 非目标(明确不做)
- LLM 工具层(R8 "切换自启"工具)— 本期暂不做,UI + CLI 优先,后续单独加。
- Windows Service / NSSM 包装。
- Linux 系统级(root)service — 默认走 user + linger。

### 测试策略
- **server 侧**:描述符生成单测(plist / systemd unit / task XML 内容断言),无需真装真启。
- **App 侧**:`get_autostart` / `set_autostart` 往返;`is_enabled` 反映状态。
- **install 脚本**:`--no-autostart` 路径的结构性校验(bash -n / 命令存在性)。
- **Operator 验证(不进自动化)**:真机 enable + 重启 + 登录后进程拉起;macOS 本地网络隐私弹窗确认。

## 7. 影响文件(预估,规划期细化)

- `desktop/shell/src/main.rs` — 移除 `ensure_autostart` 自动开启;注册新命令。
- `desktop/shell/src/`(新增或现有 connection 模块旁)— `get_autostart` / `set_autostart` 命令实现。
- `interfaces/webchat/src/platform/wide/views/settings/general.rs` — "本机"小节 + Tauri invoke 封装。
- `src/bin/aleph-server/cli.rs` — `Command::Service` + `ServiceAction`。
- `src/bin/aleph-server/commands/service.rs`(新)+ `commands/mod.rs` 注册。
- `Scripts/install.sh` / `Scripts/install.ps1` — 调 `service install` + `--no-autostart` 开关。
- `docs/reference/WINDOWS_RUNTIME.md` / `PROCESS_MANAGEMENT.md` — 自启 + flock 单例提示(可选)。
