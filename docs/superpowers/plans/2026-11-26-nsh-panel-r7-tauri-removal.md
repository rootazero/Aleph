# NSH Panel R7：nsh 官方工具链与删除 Tauri 实施计划 / Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 `desktop/nsh-shell` 改成 `nsh package` 能打包的 Rust app，补齐用户依赖的 Tauri 功能，然后删除 Tauri 宿主及其全部引用。

**Architecture:** `aleph-nsh-shell` 变成 lib + bin：lib 导出无 props 的根组件 `App`，供 nsh 生成的 release runner 调用（`Root::of::<aleph_nsh_shell::App>()`）；bin 只留 CLI 模式与 QA/开发入口。启动参数改为"预设 + 回退"：bin 预设，runner 下从连接目标文件与环境变量读取。打包由 `nsh package` 完成，`scripts/nsh_package.sh` 补一步：放入 `aleph-server`、重签、重打归档、签更新清单。

**Tech Stack:** Rust 2021、nsh 0.6.0（`nsh-core` / `nsh-base` / `nsh-platform-*` / `nsh-updater`，全部 `=0.6.0`）、Leptos Panel（`interfaces/webchat`）、`mdns-sd`、bash、GitHub Actions。

**Spec:** `docs/superpowers/specs/2026-11-26-nsh-panel-r7-tauri-removal-design.md`（提交 `0d0db1f6c`）

## Global Constraints

- 只在 worktree `.worktrees/nsh-panel-r7`（分支 `nsh-panel-r7-tauri-removal`）工作，不动 main。
- app id `ai.aleph.desktop`，显示名 `Aleph`，URL scheme `aleph`，窗口 1180×800；版本与 workspace 一致（`Cargo.toml:36` `26.9.1`）。
- nsh 依赖一律 `=0.6.0`；不引入 nsh 以外的新打包工具。
- `aleph-server` 位置：macOS `Aleph.app/Contents/MacOS/aleph-server`；Windows 包根 `aleph-server.exe`；Linux 包根 `aleph-server`。
- 连接目标文件 `~/.aleph/.desktop-shell-target`，缺失或无法解析＝`ConnectionTarget::Local`；类型只用 `shared/client/src/connection_target.rs` 的 `ConnectionTarget`。
- 远程 Gateway 只接受系统信任的证书；不实现任何证书放行。
- 宿主与 Panel 的消息只在 `shared/protocol/src/panel_shell.rs` 定义一次（判据 10）。
- 更新：首查在启动后 90 s，之后每 6 h；清单文件不叫 `latest.json`；私钥 GitHub secret 名 `NSH_UPDATE_SECRET_KEY`。
- 删除放最后（Task 11），在此之前 `just shell-build` 必须仍能构建 Tauri 版。
- 代码注释用英文；文档中英双语；每处删除写进提交说明。
- 真机验收只认 `nsh package` + 后处理的产物，不用 `cargo run` 代替（判据 18）。

## Review Focus

1. **启动时 Remote 不可达**：窗口应显示连接表单与错误，不能白屏或退出。→ Task 7 测 `startup_view(target, reachable=false) == View::Connect { error: Some(_) }`。
2. **端口 18790 被旧版或其他进程占用**：`ensure_compatible_ready` 报错后应显示错误与"重试"，不能静默退出。→ Task 2 测 `ServerPhase::Failed` 渲染重试按钮，且重试后再次调用启动器。
3. **更新时本地 `aleph-server` 正在运行**：Windows 上运行中的 exe 无法改名，macOS 会留下旧进程。安装前必须先停本地服务。→ Task 8 测 `install_plan(target=Local)` 的第一步是 `StopLocalServer`，Remote 下没有这一步。
4. **应用已在运行时再次打开（`aleph://open` 或双击）**：应聚焦已有窗口，不起第二个实例或第二个服务。→ Task 5 测 `deep_link_action("aleph://open") == Some(DeepLink::Open)`，Windows/Linux 在 Task 12 真机测第二次启动。
5. **目标文件损坏或是 lite 版留下的内容**：回退 Local 并记日志，不崩溃。→ Task 3 测空文件、乱码、`https://` 以外的 scheme、只有空白，四种都得到 `Local`。

---

### Task 1：宿主改为 nsh Rust app（lib + bin），并证明 runner 路径可行

这是止损点：Step 7 的真机检查任何一项失败，就停下来汇报，不继续后续任务。

**Files:**
- Delete: `desktop/nsh-shell/src/{connection.rs,session.rs,supervisor.rs,runtime.rs}`、`desktop/nsh-shell/src/components/`（从未挂载的原型）
- Create: `desktop/nsh-shell/src/lib.rs`、`desktop/nsh-shell/src/launch.rs`、`desktop/nsh-shell/nsh.json`
- Modify: `desktop/nsh-shell/src/main.rs`（收缩为 CLI 与 QA 入口）、`desktop/nsh-shell/src/shell_host.rs:2`、`desktop/nsh-shell/Cargo.toml`、根 `Cargo.toml:12-14`（去掉重复的 `desktop/nsh-shell` 成员）
- Test: `desktop/nsh-shell/src/launch.rs`（内联单测）、`desktop/nsh-shell/tests/launch_arguments.rs`（保持通过）

**Interfaces:**
- Produces: `pub use app::App`（`#[component] pub fn App() -> impl IntoElement`，无 props）；`launch::Launch { settings: HostSettings, qa: Option<QaLaunch> }`；`pub fn launch::preset(launch: Launch)`（只能调用一次，第二次 panic）；`pub(crate) fn launch::current() -> Launch`（有预设返回预设，否则从环境与目标文件构造，Task 3 之前目标视为 Local）；`shell_host::APP_ID == "ai.aleph.desktop"`。

- [ ] **Step 1: 写失败的测试** `launch.rs`：`current_without_preset_reads_environment`（未预设时，无 QA 环境变量得到 `qa == None`）；`preset_wins_over_environment`；`second_preset_panics`（`#[should_panic]`）。用 `std::sync::OnceLock`，测试各自在子进程或独立 `OnceLock` 实例中运行，不要共享全局状态。
- [ ] **Step 2:** `cargo test -p aleph-nsh-shell --lib launch` → FAIL（模块不存在）。
- [ ] **Step 3: 实现。** 把 `main.rs:165-305` 的 `App` 与 `AppProps` 搬到 `lib.rs`（或 `src/app.rs`），`AppProps` 的字段改由 `launch::current()` 提供。`main.rs` 只保留：参数解析、`--help`、`--autostart-status`、`--desktop-ipc-status`、QA 环境变量校验，然后 `launch::preset(..)`，再 `run_app(AppSource::Rust(Root::of::<App>()), AppOptions{..现有值..})`。删掉 `main.rs` 中"UI 之前阻塞调用 `ensure_compatible_ready`"的那段（Task 2 接手）。`Cargo.toml` 加 `[lib]`，加 `default = ["embedded-core"]`（nsh runner 只用默认 feature 构建 app crate）。`APP_ID` 改为 `ai.aleph.desktop`；QA 身份 `"{APP_ID}.qa.{nonce}"` 自动跟随。
- [ ] **Step 4: 写 `nsh.json`。** `{"name":"Aleph","id":"ai.aleph.desktop","version":"26.9.1","rust":".","root":"App","schemes":["aleph"],"singleInstance":true,"window":{"width":1180,"height":800}}`。暂不写 `capabilities` 与 `updater`（Task 4、8）。`window` 的确切键名以 nsh 0.6.0 `website/docs` 为准。
- [ ] **Step 5:** `cargo test -p aleph-nsh-shell` 与 `cargo clippy -p aleph-nsh-shell --all-targets -- -D warnings` → PASS；`tests/launch_arguments.rs` 全部通过。检查 `qa/nsh_panel_launcher.py` 的构建命令：若显式关了 `embedded-core`，改为 `--no-default-features` 加原 feature 列表。
- [ ] **Step 6:** `cd desktop/nsh-shell && nsh package --target macos -o ../../target/nsh-out` → 产出 `Aleph.app` 与 tar.gz。
- [ ] **Step 7: 真机检查（止损点）。** 在已运行 `aleph-server` 的前提下打开 `target/nsh-out/Aleph.app`：① 窗口显示 Panel；② 关窗后进程仍在，托盘"Show Aleph"与 `mod+shift+a` 唤回同一窗口；③ 托盘图标存在。任何一项失败就停止并汇报。
- [ ] **Step 8: Commit** `refactor(nsh-shell): lib root App for the nsh runner; drop unmounted prototypes`（说明中列出删掉的 5 个原型路径与重复的 workspace 成员）。

### Task 2：本地服务改为后台启动，失败可重试

**Files:**
- Create: `desktop/nsh-shell/src/local_service.rs`
- Modify: `desktop/nsh-shell/src/lib.rs`（App 按阶段渲染）
- Test: `desktop/nsh-shell/src/local_service.rs`（内联）

**Interfaces:**
- Consumes: `aleph_desktop_host::local_server::{ensure_compatible_ready, wait_until_port_closed}`、`Launch`。
- Produces: `pub(crate) enum ServerPhase { Starting, Ready, Failed(String) }`；`pub(crate) struct LocalService`，含 `fn start(&self)`（后台线程跑启动器，更新阶段）、`fn phase(&self) -> ServerPhase`、`fn stop(&self)`（停服务并等端口关闭，供 Task 7、8、9 使用）。启动器以 trait 对象 `Box<dyn Fn() -> Result<(), String> + Send + Sync>` 注入，便于测试。

- [ ] **Step 1: 失败测试。** `phase_moves_starting_to_ready`；`failure_is_reported_not_exited`（启动器返回 `Err("port 18790 busy")` → `Failed("port 18790 busy")`）；`retry_calls_launcher_again`（失败后 `start()` 再调一次启动器，计数为 2）；`concurrent_start_runs_launcher_once`。
- [ ] **Step 2:** `cargo test -p aleph-nsh-shell --lib local_service` → FAIL。
- [ ] **Step 3: 实现。** App 在 Local 目标下首帧调用 `start()`；`Starting` 显示"正在启动本地服务"，`Failed` 显示错误文本与"重试"按钮，`Ready` 显示 WebView。Remote 目标不调用 `start()`。
- [ ] **Step 4:** 测试 PASS；clippy 通过。
- [ ] **Step 5: 真机。** 先手动占用 18790（`python3 -m http.server 18790`），打包后启动 → 显示错误与重试；释放端口、点重试 → 显示 Panel。
- [ ] **Step 6: Commit** `feat(nsh-shell): start the local server in the background with a retry screen`。

### Task 3：连接目标持久化

**Files:**
- Create: `desktop/nsh-shell/src/target_store.rs`
- Modify: `desktop/nsh-shell/src/launch.rs`（未预设时读目标）
- Test: `desktop/nsh-shell/src/target_store.rs`（内联；从 `desktop/shell/src/connection.rs` 移植 marker 测试与 `test_home` 夹具）

**Interfaces:**
- Produces: `pub(crate) fn load_target(home: &Path) -> ConnectionTarget`；`pub(crate) fn save_target(home: &Path, target: &ConnectionTarget) -> std::io::Result<()>`（写入 `<home>/.aleph/.desktop-shell-target`，先写临时文件再改名；`Local` 时删除文件）。生产调用传真实 home；测试只传临时目录，函数本身不读 home，从结构上隔离测试。

- [ ] **Step 1: 失败测试。** `missing_file_is_local`；`remote_round_trips`（`https://gw.example.com` 保存后读回相等）；`saving_local_removes_file`；Review Focus 5 的四种坏输入（空文件、`\u{0}garbage`、`ftp://x`、`"   \n"`）都得到 `Local`。
- [ ] **Step 2:** FAIL。
- [ ] **Step 3: 实现。** 解析用 `ConnectionTarget::parse`、写入用 `to_persisted`，不另写格式。
- [ ] **Step 4:** PASS。
- [ ] **Step 5: Commit** `feat(nsh-shell): persist the connection target in ~/.aleph/.desktop-shell-target`。

### Task 4：打包后处理、能力声明与麦克风

**Files:**
- Create: `scripts/nsh_package.sh`、`scripts/tests/test_nsh_package.py`
- Delete: `desktop/nsh-shell/macos/{bundle-local.sh,test_bundle_local.py,Info.plist}`（被 `nsh package` 取代；先 `rg` 确认 Info.plist 没有其他引用）
- Modify: `desktop/nsh-shell/nsh.json`（`capabilities`）、`justfile`（新增 `nsh-package` 与 `nsh-dev`，暂不删 `shell-*`）、`interfaces/webchat/src/platform/wide/views/voice/audio.rs:172-200`

**Interfaces:**
- Produces: `scripts/nsh_package.sh --target <macos|windows|linux> --out <dir> [--update-key <secret file>]`，输出 `<dir>/Aleph-<version>-<os>-<arch>.{tar.gz|zip}`，提供 `--update-key` 时另写 `<dir>/aleph-update.json` 与 `.sig`（Task 8 定名）。webchat 中 `fn native_shell_marker(data_shell: Option<&str>) -> bool`，接受 `"aleph-nsh"`（Tauri 删除前也接受 `"aleph-tauri"`，Task 10 去掉）。

- [ ] **Step 1: 失败测试。** `test_nsh_package.py`（仅 macOS 运行，其他平台 skip）：跑脚本后断言 `Aleph.app/Contents/MacOS/aleph-server` 存在且可执行；`codesign --verify --deep --strict Aleph.app` 返回 0；归档内含 `Aleph.app/Contents/MacOS/aleph-server`，且没有 `._*` 文件。webchat：`native_shell_marker(Some("aleph-nsh"))` 为 true，`None` 与 `Some("other")` 为 false。
- [ ] **Step 2:** 运行两组测试 → FAIL。
- [ ] **Step 3: 实现脚本。** 步骤按设计 §4.3：`cargo build --release -p aleph-server`；`nsh package --target <os> -o <out>`；放入 `aleph-server`；macOS 依次 `codesign --force --sign - --identifier ai.aleph.server`（server）、`codesign --force --sign -`（.app）、`codesign --verify --deep --strict`；重打归档（macOS 用 `COPYFILE_DISABLE=1`）并覆盖 nsh 原来的归档。`set -euo pipefail`，任何一步失败即退出。
- [ ] **Step 4: 写能力声明。** `"capabilities": {"network":{"hosts":["*"]},"webview":{"hosts":["*"],"media":["microphone"]},"notifications":true,"openUrl":{"schemes":["https","http","mailto"]},"windows":{},"application":true}`。重新打包，确认 `Info.plist` 含 `NSMicrophoneUsageDescription`，entitlement 含 `com.apple.security.device.audio-input`（`codesign -d --entitlements - Aleph.app`）。
- [ ] **Step 5: 实现 `native_shell_marker` 并在 `audio.rs:197` 使用。** 设计 §4.2 中提到的 `resolve_daemon_binary` 无需改动：`desktop/host/src/local_server.rs:248-252` 已在宿主 exe 同目录查找 `aleph-server`（Windows 为 `.exe`），且不带三元组后缀。Step 7 的真机检查顺带确认它找到的是包内的那个副本。
- [ ] **Step 6:** 测试 PASS；`cargo check -p aleph-panel --target wasm32-unknown-unknown`（以仓库现有命令为准）。
- [ ] **Step 7: 真机。** 用打包产物在 Panel 里开语音：首次弹出系统麦克风授权；拒绝后 Panel 显示 NSH 的恢复提示。
- [ ] **Step 8: Commit** `feat(nsh-shell): post-package step, capabilities and microphone grant`（说明中列出删除的三个 macos 文件）。

### Task 5：深链与单实例

**Files:**
- Create: `desktop/nsh-shell/src/deep_link.rs`
- Modify: `desktop/nsh-shell/src/lib.rs`、`desktop/nsh-shell/src/panel_bridge.rs:84-93`（`aleph://open` 的处理挪到 `deep_link.rs`）

**Interfaces:**
- Produces: `pub(crate) enum DeepLink { Open }`；`pub(crate) fn deep_link_action(url: &str) -> Option<DeepLink>`。App 启动后循环 `next_url_scheme_open().await`，`Open` → 显示并聚焦主窗口，再走 `panel_bridge` 已有的 `open_chat`。

- [ ] **Step 1: 失败测试。** `aleph://open` 与 `aleph://open/` → `Some(Open)`；`aleph://evil`、`https://open`、`aleph:open`、空串 → `None`。
- [ ] **Step 2:** FAIL。 **Step 3:** 实现。 **Step 4:** PASS。
- [ ] **Step 5: 真机（macOS）。** 应用运行时 `open aleph://open` → 聚焦已有窗口；应用未运行时 → 启动并打开。`ps` 只见一个 `Aleph` 与一个 `aleph-server`。
- [ ] **Step 6: Commit** `feat(nsh-shell): handle aleph://open through the nsh URL scheme stream`。

### Task 6：开机自启的写路径

**Files:**
- Modify: `shared/protocol/src/panel_shell.rs`、`desktop/nsh-shell/src/autostart.rs`、`desktop/nsh-shell/src/panel_bridge.rs:144-230`、`desktop/nsh-shell/src/panel_bridge.js`、`interfaces/webchat/src/platform/wide/views/settings/desktop_autostart.rs`
- Test: `shared/protocol` 内联序列化测试、`desktop/nsh-shell/tests/panel_bridge.test.mjs`

**Interfaces:**
- Produces: `PanelReply::AutostartSet { request: u32, document: String, enabled: bool }`（kebab-case 线名 `autostart-set`）；宿主回复复用 `ShellRequest::AutostartReply`，`status` 为写入后重新读取的状态。`autostart::Reader::set(&self, enabled: bool) -> AutostartStatus`。JS 桥 `__alephShellBridge.autostartSet(enabled: boolean): Promise<AutostartStatus>`。
- 读取保留现有 macOS 读取器（它能报 `RequiresApproval`）；写入在三个平台都调用 nsh `set_launch_at_login(enabled)`。Windows/Linux 的读取改用 nsh `launch_at_login()`，结果映射为 `Enabled {}` / `Disabled {}`，替换 `UnsupportedPlatform`。

- [ ] **Step 1: 失败测试。** 协议：`{"kind":"autostart-set","request":1,"document":"<32 hex>","enabled":true}` 能反序列化；多余字段被拒（`deny_unknown_fields`）。JS：`autostartSet(true)` 发出上述消息，收到 `autostart-reply` 后 resolve；参数不是布尔值时直接 reject，不发消息。
- [ ] **Step 2:** FAIL。 **Step 3:** 实现宿主与 JS 桥。
- [ ] **Step 4: Panel。** `desktop_autostart.rs` 删除 `invoke`、`get_autostart`、`set_autostart` 与 `__TAURI__` 解析，读写都走 `__alephShellBridge`；`Unavailable`、`Failed`、`RequiresApproval` 时禁用开关的现有逻辑保持不变。
- [ ] **Step 5:** `cargo test -p aleph-protocol -p aleph-nsh-shell`、`node --test desktop/nsh-shell/tests/panel_bridge.test.mjs` → PASS。
- [ ] **Step 6: 真机（macOS 打包产物）。** 在设置里开启 → 系统设置的"登录项"中出现 Aleph；关闭 → 消失。
- [ ] **Step 7: Commit** `feat(panel-shell): typed autostart write path through nsh launch_at_login`。

### Task 7：原生连接表单、mDNS 发现与探测

**Files:**
- Create: `desktop/nsh-shell/src/gateway_probe.rs`（从 `desktop/shell/src/gateway_probe.rs` 移植，去掉 Tauri 类型）、`desktop/nsh-shell/src/connect_form.rs`
- Modify: `shared/protocol/src/lib.rs`（新增 `pub const GATEWAY_MDNS_SERVICE: &str = "_aleph._tcp.local.";`）、`src/gateway/mdns_broadcaster.rs:57`（改用该常量）、`desktop/nsh-shell/Cargo.toml`（`mdns-sd`，版本与 `desktop/shell/Cargo.toml` 相同）、`desktop/nsh-shell/src/lib.rs`

**Interfaces:**
- Consumes: Task 2 `LocalService::{start, stop}`、Task 3 `load_target/save_target`。
- Produces: `pub(crate) async fn target_reachable(target: &ConnectionTarget) -> bool`、`pub(crate) fn target_unreachable_message(target: &ConnectionTarget) -> String`（移植）；`pub(crate) fn discover(timeout: Duration) -> Vec<String>`；`pub(crate) enum View { Starting, Connect { error: Option<String> }, Panel { url: String } }`；`pub(crate) fn startup_view(target: &ConnectionTarget, reachable: bool) -> View`。

- [ ] **Step 1: 失败测试。** 移植 `gateway_probe.rs` 原有单测；新增 Review Focus 1：`startup_view(&Remote(url), false)` 得到 `Connect { error: Some(_) }`，`startup_view(&Local, _)` 得到 `Starting`；证书错误的探测结果返回含"受信任的证书 / trusted certificate"的提示。
- [ ] **Step 2:** FAIL。
- [ ] **Step 3: 实现。** 表单：发现列表（打开时扫描 3 s）+ URL 输入框 + "连接"按钮 + "使用本机"按钮。连接时先探测；成功后 `save_target`，切到 Remote 时 `LocalService::stop()`，切回 Local 时 `start()`，然后用新 `panel_url()` 重建 WebView。入口：托盘与菜单的"连接…"（Task 9 加菜单项）、启动时 Remote 不可达。
- [ ] **Step 4:** PASS；`cargo test -p aleph-server`（只跑 mdns 相关测试）仍通过。
- [ ] **Step 5: 真机。** 一台机器上用第二个端口起 Gateway 并广播 → 表单能发现它，连接后 Panel 指向它，本地服务已停；切回本机 → 本地服务重新起来。重启应用后目标保留。
- [ ] **Step 6: Commit** `feat(nsh-shell): native connect form with mDNS discovery and probing`。

### Task 8：自动更新

**Files:**
- Create: `desktop/nsh-shell/src/update.rs`
- Modify: `desktop/nsh-shell/Cargo.toml`（`nsh-updater = "=0.6.0"`）、`desktop/nsh-shell/nsh.json`（`updater`）、`scripts/nsh_package.sh`（清单步骤）

**Interfaces:**
- Consumes: `LocalService::stop`、`load_target`。
- Produces: `pub(crate) fn next_check_delay(since_launch: Duration, last_check: Option<Duration>) -> Duration`（首次在启动后 90 s，之后间隔 6 h）；`pub(crate) enum InstallStep { StopLocalServer, Install, Restart }`；`pub(crate) fn install_plan(target: &ConnectionTarget) -> Vec<InstallStep>`；`pub(crate) struct Updater`，含 `fn available(&self) -> bool`（`Installation::current()` 失败时为 false，入口隐藏）、`async fn check(&self) -> Option<Version>`、`async fn install_and_restart(&self)`。

- [ ] **Step 1: 失败测试。** `first_check_waits_90s`；`later_checks_every_6h`；`local_install_stops_server_first`（Review Focus 3：`install_plan(&Local)[0] == StopLocalServer`）；`remote_install_does_not_touch_server`。
- [ ] **Step 2:** FAIL。
- [ ] **Step 3: 定清单名并核实。** 用 spike（`/tmp/aleph-nsh-spike`，已有密钥与本地服务器）验证：把 `nsh update-manifest` 输出的 `latest.json` / `latest.json.sig` 改名为 `aleph-update.json` / `aleph-update.json.sig`，`nsh.json` 的 `updater.url` 指向新名，1.0.0 → 1.1.0 升级仍成功。若 `.sig` 地址不是 `url + ".sig"`，按 `Config::signature_url` 的规则命名。失败就改用独立的 release 资产路径，并在提交说明中写明。
- [ ] **Step 4: 实现。** 只调 `nsh-updater` 原语（`Config::parse`、`check`、`Download`、`Installation::current/install`、`Staged::finish`），重启用 `std::process::Command` 启动新的 exe 后 `quit()`。有新版本时发系统通知，点击后执行 `install_plan`。`nsh keygen` 生成密钥：公钥写入 `nsh.json`，私钥只交给用户设置 `NSH_UPDATE_SECRET_KEY`，不提交、不写进日志。`updater.url` = `https://github.com/rootazero/Aleph/releases/latest/download/aleph-update.json`。
- [ ] **Step 5:** PASS。
- [ ] **Step 6: 真机。** 本地 loopback 服务器上打 `26.9.1` 和临时 `26.9.2` 两个包，在 `26.9.1` 中"检查更新" → 通知 → 安装并重启，重启后版本为 `26.9.2`；`ps` 中旧的 `aleph-server` 已退出，新的已启动。
- [ ] **Step 7: Commit** `feat(nsh-shell): nsh-updater checks, notification and install with server stop`。

### Task 9：菜单、托盘、外链、退出并停服务、TCC 监视

**Files:**
- Create: `desktop/nsh-shell/src/external_link.rs`（移植 `desktop/shell/src/external_link.rs` 的 `is_internal` 与点击拦截脚本）、`desktop/nsh-shell/src/menu.rs`、`desktop/nsh-shell/src/perm_monitor.rs`（`#[cfg(target_os = "macos")]`，移植 `desktop/shell/src/perm_monitor.rs`）
- Modify: `desktop/nsh-shell/src/lib.rs`（托盘）、`desktop/nsh-shell/src/panel_bridge.rs` / `panel_bridge.js`、`shared/protocol/src/panel_shell.rs`

**Interfaces:**
- Produces: `PanelReply::OpenExternal { document: String, url: String }`（线名 `open-external`）；`pub(crate) fn is_internal(url: &Url, gateway: &ConnectionTarget) -> bool`。宿主收到 `OpenExternal` 后重新判定，非内部链接才调用 nsh `open_url`。
- 菜单（`set_application_menu`，macOS）与托盘项：显示 Aleph、在浏览器中打开、连接…、检查更新（仅 `Updater::available()`）、重新载入 Panel、退出、退出并停止服务。不做 DevTools、仓库、报告问题。

- [ ] **Step 1: 失败测试。** 移植 `external_link.rs` 原有测试，`is_internal` 改接收 `ConnectionTarget`；新增：Remote 目标下同源链接为内部链接，`https://github.com/...` 为外部链接；`javascript:` 与 `file:` 链接永不交给 `open_url`。协议：`open-external` 序列化往返。
- [ ] **Step 2:** FAIL。 **Step 3:** 实现。"退出并停止服务"在 Local 下调 `LocalService::stop()` 后 `quit()`；Remote 下直接退出。TCC 监视在检测到授权变化后调用 `LocalService::stop()` + `start()`。
- [ ] **Step 4:** PASS。
- [ ] **Step 5: 真机。** 逐项点菜单与托盘；Panel 中点外部链接 → 系统浏览器打开，Panel 不跳转；"退出并停止服务"后 18790 端口关闭。
- [ ] **Step 6: Commit** `feat(nsh-shell): menu, tray, external links, quit-and-stop, TCC monitor`。

### Task 10：清理 Panel 侧的 Tauri 残留

**Files:**
- Modify: `interfaces/webchat/src/api/native_credentials.rs`（删 `__TAURI_INTERNALS__` 分支，`native_expected_from` 去掉 `tauri_internals` 参数）、`interfaces/webchat/src/platform_host.rs:91-182`（`aleph-tauri` 选择器守卫改为 `aleph-nsh`）、`interfaces/webchat/src/platform/wide/views/voice/audio.rs`（去掉 `aleph-tauri`）、所有 `data-tauri-drag-region` 属性（`rg -l 'data-tauri-drag-region' interfaces/webchat/src`）、`interfaces/webchat/src/state/hotkey.rs:9`、`interfaces/webchat/src/app.rs:464`、相关注释

- [ ] **Step 1:** 先改测试：`native_credentials.rs` 与 `platform_host.rs` 的现有测试改成新签名与 `aleph-nsh` 选择器 → 运行应 FAIL。
- [ ] **Step 2:** 实现删除。拖拽区若在 NSH 下需要，改用 NSH 的对应写法；否则直接删除属性。
- [ ] **Step 3:** `cargo test -p aleph-panel`（已知在 main 上也失败的 2 个 galaxy guard 测试除外，记录其名称）；`rg -n -i 'tauri' interfaces/webchat/src` 只剩说明历史的注释或为空。
- [ ] **Step 4: Commit** `refactor(panel): remove Tauri IPC paths and markers`（说明中列出每个文件）。

### Task 11：删除 Tauri 并改写发版流水线

**Files:**
- Delete: `desktop/shell/`、`desktop/host/src/updater.rs`（及 `lib.rs` 中的 `mod`）、`scripts/check_webview_baseline.mjs`、`scripts/stage_shell_placeholders.sh`、`qa/native_ipc/`、`qa/webview_compat` 中的 Tauri 部分
- Modify: 根 `Cargo.toml`（成员与 `:123` 注释）、`justfile`（删 `shell_dir`、`_stage-shell-binaries`、`shell-*`，以及 `:469`、`:670` 的引用）、`.github/workflows/aleph-app-release.yml`（三平台 `scripts/nsh_package.sh`，上传归档与 `aleph-update.json*`）、`.github/workflows/aleph-core-ci.yml:19-20,41-42,586-599`、`.github/workflows/rust-doctor.yml`、`install.sh` / `install.ps1`（下载归档并解压）、`qa/nsh_panel_launcher.py:1154`、`scripts/doc_wiring_audit*`

- [ ] **Step 1: 删除前普查。** `rg -n -i -e tauri -e 'desktop/shell' --glob '!docs/superpowers/**' --glob '!docs/reports/**' --glob '!docs/engineering-reports/**' --glob '!docs/research/**' --glob '!target/**' .` 并保存输出，作为对照清单。
- [ ] **Step 2:** 逐项删除与修改。发版工作流：matrix 保留 `macos-latest` / `ubuntu-24.04` / `windows-latest`；装 nsh 0.6.0；Linux 装 WebKitGTK 6.0；Windows 用 Git Bash 跑脚本；`NSH_UPDATE_SECRET_KEY` 缺失时只出归档、不出清单（与今天 Tauri 签名门的行为一致）。
- [ ] **Step 3:** 再跑 Step 1 的普查 → 只剩文档与提交历史说明（Task 13 处理文档）。`cargo check --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`、`just --list`、`actionlint`（如已安装）全部通过。
- [ ] **Step 4: Commit** `chore: remove the Tauri desktop host`（说明中按目录列出每处删除与改动）。

### Task 12：三平台编译与启动证据

- [ ] **Step 1:** `git push -u origin nsh-panel-r7-tauri-removal`。
- [ ] **Step 2: Linux**（`ssh Arch-hypr`，`/home/zou/data/workspace/Aleph`）：`git fetch && git worktree add ../Aleph-r7 origin/nsh-panel-r7-tauri-removal`；确认已装 WebKitGTK 6.0（`pacman -Q webkitgtk-6.0`，没有就停下来问用户）；`scripts/nsh_package.sh --target linux --out target/nsh-out`；在 Hyprland 会话中启动（`WAYLAND_DISPLAY` 取自 tty1 会话），记录进程列表、`curl 127.0.0.1:18790` 的状态码与日志尾部；再启动一次，确认只有一个实例（Review Focus 4）。
- [ ] **Step 3: Windows**（`ssh Windows11`，用 `powershell -NoProfile -Command -` + heredoc；PATH 加 `C:\Users\zou\scoop\apps\rustup\current\.cargo\bin`）：在 `D:\Workspace\Aleph` 旁新建 worktree（不动那边 main 的未提交改动）；用 Git Bash 跑打包脚本；启动并记录与 Linux 相同的证据，包括单实例。Windows 本地更新演练（Task 8 Step 6 的做法）：能做就做，做不到在回执中标为"未验证"。
- [ ] **Step 4: macOS** 完整验收：设计 §5.2 的全部项目，用打包产物。
- [ ] **Step 5:** 回执 `docs/superpowers/runs/2026-11-26-nsh-panel-r7-receipt.md`，记录每个平台的命令、提交哈希、产物哈希与结果；commit `docs(runs): R7 three-platform receipt`。

### Task 13：文档

- [ ] **Step 1: 回退说明。** 确认最后一个 Tauri 版本（`gh release view v26.9.1 --json assets`），在 `docs/reference/RELEASE.md` 写明：tag、资产名、如何装回 Tauri 版、旧用户如何手动改装 nsh 版。
- [ ] **Step 2:** 更新 `docs/reference/DESKTOP_SHELL.md`（双轨改为单宿主；打包、更新、深链、菜单、连接各节）、`FEATURE_LOCATOR.md:249`、`PRODUCT_TOPOLOGY.md:41,112,148`、`ARCHITECTURE.md:661`、`WINDOWS_RUNTIME.md`、`SECURITY.md`（删自签 TOFU，写明远程须系统信任证书）、`BROWSER_LIVE_VIEWER.md`、`AGENTS.md` 与 `CLAUDE.md` 的"桌面宿主：双轨现状"小节（两份保持一致）。
- [ ] **Step 3:** 对 `docs/reference/` 跑 Task 11 Step 1 的普查 → 无指向 `desktop/shell` 的现行说明；`scripts/doc_wiring_audit*`（若存在）通过。
- [ ] **Step 4: Commit** `docs: single NSH desktop host, nsh packaging and rollback steps`。
