# NSH Panel 第七轮：改用 nsh 官方工具链并删除 Tauri / NSH Panel R7: Official nsh Toolchain and Tauri Removal

- 状态 / Status: 草案，待用户批准 / Draft, awaiting approval
- 分支 / Branch: `nsh-panel-r7-tauri-removal`（worktree `.worktrees/nsh-panel-r7`，基于 main `0386bdfff`）
- 依据 / Inputs: nsh v0.6.0 源码与文档（`/Volumes/TBU4/Github/nsh`）、Tauri 功能清单、`/tmp/aleph-nsh-spike` 打包与更新实验

## 1. 目标 / Goal

今天桌面端有两个宿主：仍在发版的 Tauri 宿主 `desktop/shell`，和目标形态的 NSH 宿主 `desktop/nsh-shell`。
本轮把 NSH 宿主改成 nsh 官方工具链能打包的 Rust app（`nsh.json` + `nsh package`）。链接、更新、签名、打包都交给
nsh；Aleph 自己只保留一步打包后处理（放入 `aleph-server`、重签、重打归档）。之后补齐用户每天会碰到的 Tauri 功能，
并删除 Tauri 宿主及其 CI、脚本、QA 和文档引用。

Today two desktop hosts ship side by side. This round turns `desktop/nsh-shell` into an nsh Rust app that `nsh package`
builds, so nsh owns linking, updates, signing and packaging, and Aleph keeps one post-package step. It then ports the
Tauri features users rely on and deletes the Tauri host with every reference to it.

非目标 / Non-goals:
- 不把 TSX Panel 设为默认；三平台发版仍用 Leptos Panel（`aleph-server` 在 `127.0.0.1:18790` 提供）。`tsx-panel` 仍是 macOS 开发开关，不进发版包。
- 不做 DMG / MSI / AppImage / deb：nsh 0.6.0 不产出这些格式（见 §7 风险）。
- 不做 Apple Developer ID 签名与公证：今天的 Tauri 流水线也没有。
- 不做旧 Tauri 用户的自动迁移：用户已决定在最后一个 Tauri 版本的发布说明里引导手动重装。

## 2. 已定决策 / Settled decisions

| # | 决策 | 来源 |
|---|---|---|
| D1 | 不等 14 天双轨。删除 Tauri 的条件：① macOS 真机验收；② Windows（`ssh Windows11`，`D:\Workspace\Aleph`）与 Linux（`ssh Arch-hypr`，`/home/zou/data/workspace/Aleph`）经 GitHub 同步后编译并启动；③ 保留最后一个 Tauri 签名版本与恢复步骤 | 用户选 B |
| D2 | 改用 nsh 官方工具链（`nsh package` + `nsh.json`），自维护打包代码最少 | 用户 |
| D3 | `aleph-server` 由打包后处理放进包里，随后重签、重打归档，再用 `nsh update-manifest` 签更新清单 | 用户选 A；spike 通过 |
| D4 | 只发一个完整版：内含 `aleph-server`，运行时可切换 Local / Remote；原生连接表单 + mDNS 发现。lite 版（`ai.aleph.panel`）取消 | 用户 |
| D5 | 删除自签证书 TOFU（`cert_trust/`）：远程 Gateway 必须用系统信任的证书 | 用户 |
| D6 | app id = `ai.aleph.desktop`（沿用 Tauri 完整版）；不做 Tauri→nsh 更新桥 | 用户 |

## 3. 差距表 / Gap table

"处置"一列：**接**＝接通 nsh 已有机制；**移**＝从 Tauri 移植逻辑；**删**＝不再需要。

| Tauri 功能（文件） | nsh 0.6.0 机制 | NSH 宿主现状 | 处置 |
|---|---|---|---|
| 打包、安装包（tauri-cli、`externalBin`） | `nsh package`：macOS ad hoc 签名的 `.app` + tar.gz；Windows 目录 + zip（装了 Inno Setup 时另出 exe）；Linux 目录 + tar.gz | 无 | **接** + 打包后处理（§4.3） |
| 自动更新（`update.rs`，90 s 首查、每 6 h、托盘项、Panel 横幅） | `nsh-updater`：`Config::parse/check`、`Download`、`Installation::current/install/stage`、`Staged::finish`；Rust app 无 JS 更新门面 | 无 | **接**（§4.4） |
| 深链（`deeplink.rs`，派发 `aleph:deep-link` DOM 事件） | `Root.url_schemes` + `next_url_scheme_open()`；macOS 由 `nsh package` 写 `CFBundleURLTypes` | 仅 `aleph://open`（`panel_bridge.rs:84-93`），且 `main.rs` 没有处理 open 事件 | **接** `aleph://open`；`aleph:deep-link` 事件在 Rust 侧无消费者，**删** |
| 单实例（single-instance 插件） | Windows/Linux `AppOptions.single_instance` / `nsh.json` `singleInstance`；macOS 由 LaunchServices 保证 | 无 | **接** |
| macOS 菜单（`menu.rs`：显示、浏览器打开、连接远程/本地、检查更新、重载 Panel、DevTools、仓库、报告问题、退出、退出并停服务） | `run_app` 装标准 App/Edit/Window 菜单；`set_application_menu` / `MenuRole` 自定义 | 只有标准菜单 | **移**：保留显示、浏览器打开、连接、检查更新、重载、退出、退出并停服务；DevTools/仓库/报告问题 **删** |
| 托盘（`tray.rs`） | `set_tray` | 3 项（显示 / 开启通知 / 退出） | **移**：加 "检查更新"、"连接…" |
| 全局快捷键（`hotkey.rs`） | `register_global_shortcut` | 已有（`ALEPH_SHELL_HOTKEY`，默认 `mod+shift+a`） | 保持 |
| 凭据（`keyring_bridge.rs`） | — | 已有（`credentials.rs`，typed `CredentialCall`） | 保持 |
| 开机自启（autostart 插件） | `AppOptions.login_item` + `launch_at_login` / `set_launch_at_login` | 只读状态（`autostart.rs`） | **接**写路径：`panel_shell` 契约增加 `AutostartSet`，删除 Panel 里的 `plugin:autostart` 调用 |
| 连接目标（`connection.rs`、`connect_setup.rs`、`gateway_probe.rs`） | — | 仅启动参数 `--connect` | **移**（§4.5） |
| 自签证书（`cert_trust/`，8 个文件） | WebView 无 TLS 钩子 | 无 | **删**（D5） |
| 外链（`external_link.rs`，点击拦截脚本） | `open_url` + `openUrl` 能力 | 无 | **移**：注入脚本把非 Gateway 源的链接经桥发给宿主，宿主调 `open_url` |
| 麦克风（`webview_perms.rs`） | `capabilities.webview.media: ["microphone"]`；打包写 `NSMicrophoneUsageDescription` 与 entitlement | 无授权；`voice/audio.rs:197` 只认 `data-shell == "aleph-tauri"` | **接** + 修 bug |
| 通知（`notify.rs`，宿主自己订阅 daemon `ws://127.0.0.1:18790/ws`） | `Notifications` | Panel 经 `PanelReply::Notify` 转发 | 保持 Panel 转发（R2：不在宿主里开第二个 Gateway 客户端）。窗口隐藏时 Panel 仍在运行，真机验证 |
| TCC 授权监视（`perm_monitor.rs`，授权后重启 daemon） | — | 无 | **移**（macOS only，约 200 行，复用 `desktop/host` 的重启） |
| daemon 管理（`daemon.rs`：退出并停服务、版本对账、旧 keep-alive 服务清理） | — | `ensure_compatible_ready` 已做版本兼容 | **移**"退出并停服务"；旧 keep-alive 清理 **删**（D6 不迁移） |
| 窗口状态（window-state 插件） | — | 已有 `restore_bounds` | 保持 |
| QA（`qa_root.rs`、`qa_trace.rs`、`phase9_qa.rs`、`qa/native_ipc/`） | — | NSH QA 已有（`qa/nsh_panel_launcher.py`） | **删** Tauri 部分 |

NSH 宿主中从未挂载的原型（约 1950 行）：`desktop/nsh-shell/src/{connection.rs,session.rs,supervisor.rs,runtime.rs,components/}`。**删**；§4.5 的连接目标从 Tauri 版移植，不复活这些原型。

## 4. 设计 / Design

### 4.1 宿主改为 nsh Rust app / Host as an nsh Rust app

- `desktop/nsh-shell` 拆成 `src/lib.rs`（导出根组件 `App`）+ 薄 `src/main.rs`（`cargo run` 开发与 QA 入口，nsh `--template rust` 的同款形状）。
- 新增 `desktop/nsh-shell/nsh.json`：`name "Aleph"`、`id "ai.aleph.desktop"`、`version`（与 workspace 一致，`26.9.1`）、`"rust": "."`、`"root": "App"`、`schemes ["aleph"]`、`singleInstance`、`updater`、`capabilities`（§4.6）、`window`（1180×800）。
- 只有 CLI 的模式（`--autostart-status`、`--desktop-ipc-status`）留在 `main.rs`；nsh 生成的 runner 不传这些参数。
- 启动逻辑从 `main()` 挪进 `App`：读连接目标、QA 环境变量、Local 模式下 `ensure_compatible_ready()`。后者今天在 UI 之前阻塞并在失败时退出；改为在后台线程跑，窗口先显示"正在启动本地服务"，失败时显示错误并提供重试，不再静默退出。

取舍 / Trade-off：发版用 nsh 生成的 runner（`<lib>::run_rust(app_root())`），QA 用自己的 `main.rs`。runner 的 `AppOptions` 只来自 `app.json`（id、name、dataDir、appearance、activationPolicy、`login_item`），所以 `keep_alive_on_close` 与 QA 的 `data_dir` 在发版包里设不了。关窗隐藏本来就由组件处理（`desktop/nsh-shell/src/main.rs:181` `window.on_close_press(move |_| close_window.hide())`）；计划第一步在真机确认：没有 `keep_alive_on_close` 时，关窗后托盘与快捷键仍能唤回同一窗口。不行就停下来汇报。

已核实的 nsh 0.6.0 API：`next_url_scheme_open`（`nsh-core/src/element/ambient.rs:1100`）、`open_url`（`ambient.rs:701`）、`launch_at_login` / `set_launch_at_login`（`element/desktop.rs:1082/1101`）、`set_application_menu`（`desktop.rs:1395`）；Windows/Linux runner 从 `app.json` 读 `schemes` 与 `singleInstance`。

### 4.2 aleph-server 的位置 / Where aleph-server lives

| 平台 | 位置 | 原因 |
|---|---|---|
| macOS | `Aleph.app/Contents/MacOS/aleph-server` | spike 已验证：重签后通过 `codesign --verify --deep --strict`，更新时整包替换 |
| Windows | 包根目录 `aleph-server.exe` | `nsh-updater` 的 `swap_entries` 按根目录项改名，无法改名含运行中 exe 的子目录（读源码所得，未实测） |
| Linux | 包根目录 `aleph-server` | 与 Windows 一致 |

`desktop/host` 的 `resolve_daemon_binary` 改为在宿主 exe 同目录查找（macOS 即 `Contents/MacOS`），删除 Tauri `externalBin` 的三元组后缀规则。

### 4.3 打包后处理 / Post-package step

新增 `scripts/nsh_package.sh`（三平台在 CI 用 bash 跑；Windows 用 Git Bash）：
1. `cargo build --release -p aleph-server`；`nsh package --target <os> -o target/nsh-out`。
2. 放入 `aleph-server`（§4.2）。
3. macOS：`codesign --force --sign - --identifier ai.aleph.server` 签 `aleph-server`，再 `codesign --force --sign -` 签整个 `.app`，最后 `codesign --verify --deep --strict`。不重签时更新会被拒（"its code signature is not valid"，spike 已证）。
4. 重打归档（macOS 用 `COPYFILE_DISABLE=1`），替换 `nsh package` 原来的 tar.gz / zip。
5. `nsh update-manifest <archive> --entry app --key <secret> -o <dir>` 写更新清单与 `.sig`。

### 4.4 更新 / Updates

- 用 `nsh keygen` 生成新密钥。公钥写入 `nsh.json` 的 `updater.publicKey`；私钥存为 GitHub secret `NSH_UPDATE_SECRET_KEY`。Tauri 的 minisign 密钥与清单无法复用。
- 清单地址：`https://github.com/rootazero/Aleph/releases/latest/download/<清单名>`。清单不叫 `latest.json`，因为旧 Tauri 客户端轮询的就是这个文件名，换个名字可以避免旧客户端解析到 nsh 格式的清单。计划里核实 `update-manifest` 输出能否改名而不破坏签名；不能就改用不同的 release 资产路径。
- 宿主新模块 `update.rs` 只调 `nsh-updater` 原语：启动 90 s 后首次检查，之后每 6 h；托盘与菜单提供"检查更新"；有新版本时发通知，由用户点"安装并重启"。重启前先等下载与校验完成；`Installation::current()` 失败（如开发构建）时隐藏更新入口。
- 不在 Panel 里做更新横幅（Tauri 的 `/__aleph-shell/update/*` 一并删除）。

### 4.5 连接目标 / Connection target

- 持久化文件沿用 `~/.aleph/.desktop-shell-target`，文件缺失＝Local。旧 Tauri 完整版用户的选择因此原样保留；lite 版的 `.desktop-shell-panel-*` 不再读取。
- 用 `shared/client/src/connection_target.rs` 的 `ConnectionTarget`，宿主不另定义类型（判据 1）。
- 原生连接表单（nsh 组件）：mDNS 发现列表（`mdns_sd`，服务类型与 `src/gateway/mdns_broadcaster.rs` 同源）+ 手填 URL + 探测（移植 `gateway_probe.rs`）。切换时重建 WebView，并按目标停止或启动本地服务。
- Remote 只接受系统信任的证书（D5）；探测遇到证书错误时，提示"需要受信任的证书"。

### 4.6 能力声明 / Capabilities

麦克风授权（`webview.media`）必须显式声明，而一旦写了 `capabilities`，app 就只拥有列出的能力。因此声明全集：
`network.hosts ["*"]`（Remote 可以是任意主机）、`webview { hosts ["*"], media ["microphone"] }`、`notifications`、
`openUrl { schemes ["https", "http", "mailto"] }`、`windows`、`application`、`host`。打包时 nsh 自动写入麦克风用途说明与 entitlement。
同时修 `interfaces/webchat/src/platform/wide/views/voice/audio.rs:197`，让它也认 `data-shell == "aleph-nsh"`。

### 4.7 Panel 侧的 Tauri 残留 / Tauri leftovers in the Panel

- `interfaces/webchat/src/api/native_credentials.rs`：删除 `__TAURI_INTERNALS__` 分支，只走 `shell_bridge`。
- `settings/desktop_autostart.rs:44-56`：写入改走 `ShellRequest`/`PanelReply` 新增的自启动设置消息（`shared/protocol/src/panel_shell.rs` 唯一形状，判据 10）。
- `platform_host.rs:106-182` 的 Tauri 标记与启动页豁免、`data-tauri-drag-region` 属性、`state/hotkey.rs:9`、`app.rs:464`：删除或改为 NSH 等价物。

### 4.8 删除清单 / Deletion list

`desktop/shell/` 整个目录；`Cargo.toml` 的成员行，以及重复的 `desktop/nsh-shell` 成员（第 12、14 行）；justfile 的 `shell_dir` 与 `shell-*` 目标（改为 `nsh-package` / `nsh-dev`）；`.github/workflows/aleph-app-release.yml` 重写为三平台 `nsh package`；
`aleph-core-ci.yml`、`rust-doctor.yml` 的 Tauri 步骤；`scripts/check_webview_baseline.mjs`、`stage_shell_placeholders.sh`；`install.sh` / `install.ps1` 的下载路径；`qa/native_ipc/`、`qa/webview_compat` 的 Tauri 部分；
`desktop/host/src/updater.rs`（无调用者，minisign 已由 nsh 取代）。每处删除都写进提交说明。

## 5. 验证 / Verification

1. 单测与静态检查：`cargo check/clippy/test -p aleph-nsh-shell -p aleph-desktop-host -p aleph-protocol`，webchat 受影响模块。
2. macOS 真机：`nsh package` + 后处理 → 启动 → Local 聊天 → 切 Remote 再切回 → `aleph://open` → 关窗隐藏、托盘唤回 → 自启动开关 → 麦克风授权 → 外链 → 本地更新服务器上 1 → 2 升级（spike 方法）。
3. Windows / Linux：推送分支后经 ssh 编译、`nsh package`、启动，截取进程与日志证据。Windows 在独立 worktree 里构建（那边的 main 有未提交改动）。
4. 回退：最后一个 Tauri 签名版本的 tag 与恢复步骤写进 `docs/reference/RELEASE.md`。
5. 文档：校对 `FEATURE_LOCATOR.md`、`DESKTOP_SHELL.md`、`PRODUCT_TOPOLOGY.md`、`ARCHITECTURE.md`、`WINDOWS_RUNTIME.md`、`SECURITY.md`、`AGENTS.md`/`CLAUDE.md` 的双轨小节。

## 6. 顺序 / Order

先接通已有机制（4.1 → 4.2 → 4.3 → 4.6 → 深链、单实例、自启动写入），再移植（4.5 连接目标 → 4.4 更新 → 菜单/托盘 → 外链 → TCC 监视 → 退出并停服务），最后删除（4.7 → 4.8）并完成验证与文档。删除放在最后，是为了 macOS 真机验收前 Tauri 仍可构建。

## 7. 风险与此设计让哪些事变难 / Risks and what this makes harder

- **安装体验倒退**：Linux 失去 deb/AppImage，macOS 失去 DMG，只剩归档；Windows 只有在 CI 装了 Inno Setup 时才有安装程序。`install.sh` / `install.ps1` 改为下载并解压归档。
- **ad hoc 签名**：首次打开会遇到 Gatekeeper 拦截，与今天相同；以后如果上 Developer ID，后处理脚本里的签名步骤需要改为真实身份并加公证。
- **锁定 nsh 0.6.0 的 runner 行为**：`AppOptions` 只能经 `app.json` 设置。以后要用 runner 不读的选项，只能等 nsh 上游支持或改用自写 runner，后者会退回 D2 之前的路线。
- **两个入口**：QA 走 `main.rs`、发版走 nsh runner，两者的 `AppOptions` 不同。真机验收必须用 `nsh package` 的产物，不能用 `cargo run` 的结果代替（判据 18）。
- **Windows 更新路径**只经源码阅读，未实测；真机启动之外，计划里加一次 Windows 本地更新演练，做不到就标为"未验证"。
