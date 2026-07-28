# Windows Runtime & Deployment

How `aleph-server` is deployed and operated on Windows. This is the runtime
(end-user / operator) counterpart to the build-from-source notes in
[CLAUDE.md → Windows 构建](../../CLAUDE.md). Behaviour documented here is
verified against the current code, not aspirational.

## Install

Two supported shapes (same daemon binary underneath):

| Shape | How | Where the binary lands |
|-------|-----|------------------------|
| **Full desktop App** | NSIS `.exe` / `.msi` installer from a GitHub Release | `%LOCALAPPDATA%\Aleph\` (Tauri shell + bundled `aleph-server.exe`) |
| **Standalone server** | `irm https://github.com/rootazero/Aleph/releases/latest/download/install.ps1 \| iex` | `%LOCALAPPDATA%\Aleph\aleph-server.exe` (added to user PATH) |

`install.ps1` mirrors the Unix `install.sh`: it resolves the
`aleph-server-x86_64-pc-windows-msvc.exe` release asset, drops it where the App
daemon also lives, stops any running instance before overwriting (Windows
cannot overwrite a running `.exe`), and prints start + LAN guidance. Only
`x86_64` ships a prebuilt server; arm64 must build from source.

## Data directory

All state lives under `%USERPROFILE%\.aleph` (resolved via `dirs::home_dir()`,
identical resolution on every platform): `config.toml`, `data/` (SQLite +
vault + `aleph.lock`), `logs/`.

## Running

```powershell
aleph-server start      # foreground; Ctrl+C to stop
aleph-server status     # report running state
aleph-server stop       # see caveat below
```

### Background / supervised operation

Windows has **no Unix double-fork daemon**. `--daemon` returns
`"Daemon mode is only supported on Unix systems"` by design (`daemonize()` is
`#[cfg(unix)]`; `fork`/`setsid`/`dup2` have no Windows equivalent). Run the
server in the background one of three ways instead:

1. **Full App** — the Tauri shell supervises `aleph-server.exe` for you and
   relaunches it on exit. This is the zero-config path.
2. **Task Scheduler** — register `aleph-server start` as a logon task for an
   unattended/server box.
3. **A service wrapper** (e.g. NSSM) — wrap `aleph-server start` as a Windows
   service.

> The agent-launched-GUI-into-Session-0 pitfall only affects the *App* (a
> windowed process started by an automated/service context is invisible to the
> interactive desktop). The standalone `aleph-server` is headless and unaffected.

### `stop` / `status` caveat on Windows

`stop` and `status` read the `--pid-file` (`~/.aleph/gateway.pid`), which is
written **only** by the Unix `daemonize()` path. A foreground or
supervisor-launched server on Windows therefore has no `gateway.pid`, so `stop`
reports *"no daemon running"* and does not terminate it. To stop such a server:

- Foreground: `Ctrl+C` in its terminal.
- Supervised: quit the App, or `taskkill /IM aleph-server.exe /F`.

The singleton lock (below) is the authoritative liveness record on Windows, and
its holder-PID liveness probe is now cross-platform
(`src/utils/process_alive.rs`, via `sysinfo`) — so stale-lock diagnostics are
accurate on Windows. Wiring `stop`/`status` to fall back to the lock's holder
PID is a tracked follow-up (see Gap Analysis).

## Singleton lock

Enforced by an OS-level `LockFileEx` exclusive lock on
`%USERPROFILE%\.aleph\data\aleph.lock` (`fs2`, acquired as the first action on
the `start` path — `main.rs`). A second `start` exits with code **64** and
prints the holder PID. The OS releases the lock on any process exit (normal,
panic, `taskkill /F`), so there is no stale lock after a hard kill and no sleep
is needed before restarting.

> On Windows `LockFileEx` is mandatory (not advisory like Unix `flock`), so the
> holder-PID *readback while the lock is held* is unavailable — only the mutual
> exclusion is. This is a diagnostic limitation, not a correctness one.

## Trust model (LAN-trust)

No auth step — the trust boundary is the network boundary. Default bind is
`127.0.0.1` (this machine only). To open the whole LAN, add to
`%USERPROFILE%\.aleph\config.toml`:

```toml
[gateway]
host = "0.0.0.0"
```

Any LAN device then gets full control of the agent (incl. PTY/shell). The only
protocol guardrail is WS Origin validation. See
[SECURITY.md#auth-ux](SECURITY.md#auth-ux).

## Desktop control on Windows (坐标空间与四肢)

桌面工具（`desktop` / `desktop_som` / `desktop_ax_*` / `system` / `permission` /
`media` / `pim`）在 Windows 上的实现分布见
[FEATURE_LOCATOR §7](FEATURE_LOCATOR.md#7-desktop桌面端)。这里只记运维层面必须知道的四件事。

### 1. 进程 DPI 感知 = 坐标一致性的前提

`aleph-server.exe` 在 `NativeScreen::new()` 与 `WindowsPlatform::new()`
（两者都调 `desktop/shared/src/win_dpi.rs::ensure_process_dpi_aware`，`OnceLock` latch）
一次性 opt-in **Per-Monitor-Aware V2**。这不是优化而是正确性前提：DPI-unaware 进程
拿到的 `GetWindowRect` / `GetCursorPos` / `SendInput` 绝对坐标 / UIA
`CurrentBoundingRectangle` **全部被系统虚拟化**（除以显示器缩放比），而屏幕截图走
显示驱动、**不被虚拟化**。在 Windows 默认的 150% 缩放笔记本屏上，这意味着"模型在截图里
看到按钮的位置"和"点击真正落点"差 1.5 倍，且事后无法补救——两个数字一样合理。

> **为什么是两个调用点**：`WindowsPlatform::new()` 是桌面工具路径上最早的一点，但不是
> 唯一的门 —— `src/vision/providers/platform_ocr.rs` 直接构造 `NativeScreen`，视觉请求
> 先到时进程还是 unaware，于是同一块屏在一次运行里给 OCR 路径和桌面工具路径**报出两个
> 不同的 `scale_factor`**（`coordinate_scale` 读的是**实时**等级）。两处调同一个 latch，
> 谁先谁算，另一个免费。

Rust 二进制默认不带 application manifest，所以不显式 opt-in 就是 unaware。日志里会
出现一行：

```
desktop: process is per-monitor DPI aware; screen geometry is physical pixels
```

如果看到的是 `WARN … process is DPI-unaware and the opt-in did not take`，说明有别的
东西（manifest / 宿主进程）已经把等级钉死了；此时 `DisplayInfo.scale_factor` 会如实
回报显示器 DPI 比而不是 1.0，坐标偏差**仍然存在**且是已知限制。

### 1b. 写坐标的两条轨道：指针与窗口

DPI 只解决了"数字的单位"。还有两处**读写不同源**，各自会把正确的数字送到错的地方：

- **指针**：绝对定位不走 enigo，走 `desktop/shared/src/win_input.rs`
  （`SendInput` + `MOUSEEVENTF_VIRTUALDESK`，按 `SM_XVIRTUALSCREEN`/`SM_CXVIRTUALSCREEN`
  归一化）。enigo 0.3 按 `SM_CXSCREEN`（**主屏**）归一化且不带 VIRTUALDESK（其源码里
  就写着 `// TODO`），所以副屏上的点要么归一化超过 65535 被钉在主屏右缘、要么（左/上侧
  显示器的负全局坐标）被钉在左上角 —— **多显示器上瞄准副屏的每一次点击都落在主屏**，
  而 `cursor_position`（`GetCursorPos`）读回来的是真正的虚拟桌面坐标，读写互相矛盾。
- **窗口**：`window_list` 报的 `bounds` 是 DWM **扩展帧**（去掉不可见抓边），而
  `SetWindowPos` 吃的是**原始窗口矩形**。把前者直接喂给后者，窗口每次右下偏一个边框宽；
  `resize` 则每次视觉上缩窄两个边框宽。现由 `win_window::FramePadding` 差值补偿；
  最大化窗口先经 `SetWindowPlacement(SW_SHOWNOACTIVATE)` 退出最大化（**不是**
  `ShowWindow(SW_RESTORE)`——那个会抢焦点，违 R5）。

本机实测（3024×1898 物理 / 200% 缩放单屏）：五个探测点（含两个角）指针往返**逐像素相等**；
两个普通窗口的不可见边框各为 10–11 px/边。两个 live 探针都在仓库里，默认 `#[ignore]`：

```powershell
cargo test -p aleph-desktop --test win_pointer_live -- --ignored --nocapture
cargo test -p aleph-desktop-windows --test uia_live -- --ignored --nocapture
```

> **未端到端验证的部分（诚实标注）**：多显示器分支只有单测覆盖（归一化算术 + 原点平移），
> 本机是单屏，无法端到端验证。单屏路径与 enigo 旧公式**逐字节等价**，有回归测试钉住。

### 2. 子进程一律无控制台窗口

daemon 模式（`--daemon` / 由桌面壳拉起）下 `aleph-server.exe` 自己没有控制台，因此任何
控制台子进程都会**自己分配一个**——用户屏幕上闪一个黑框。桌面层所有 `Command` 必须经
`aleph_desktop::script_exec::hidden_command` / `hidden_std_command`（core 侧对应
`src/utils/no_window.rs`）。新增 shell-out 忘了走它 = 用户每次调用看到闪窗。

### 3. 权限与能力边界

- **无 TCC**：截屏与合成输入在 Windows 桌面进程上**不需要任何授权**，`permission` 工具
  对这两类恒回 `Granted`。有 consent 门的是摄像头 / 麦克风 / 定位三项，读自
  `HKCU\…\CapabilityAccessManager\ConsentStore`（直读注册表，不再 shell-out）。
  桌面 App 无法用 API 触发授权弹窗，`request` 只能打开对应 `ms-settings:` 页面。
- **通知不是"无门"**：`Notifications` 曾也硬编码 `Granted`——Windows 确实没有**逐 app**
  的 consent 弹窗，但有一个**全机总开关**（`HKCU\…\PushNotifications\ToastEnabled`）。
  关着的时候 `send_notification` 照样成功、用户什么也看不见，而权限探针还说一切正常。
  现在这一项读那个开关（值缺失 = 从没关过 = `Granted`），`request` / `guide` 指向
  `ms-settings:notifications`（隐私页上根本没有通知开关）。
- **前台锁**：`SetForegroundWindow` 会被 Windows 的前台锁拒绝（用户正在别处打字时），
  `focus_window` 因此**轮询校验 500ms** 后如实报失败，而不是假装成功。够不着前台时
  改用 `set_value` / `ax_action`（UIA，不需要前台、不动光标）。
- **`ffmpeg`**：`media` 的相机 / 录音 / **录屏**走 DirectShow / gdigrab，设备被别的程序
  （视频会议）占用时 ffmpeg 会**无限阻塞**（`-t` 根本轮不到开始计时），故所有调用都带
  `duration + 45s` 上限并杀子进程。录屏此前是唯一漏网的一条（裸 `.output()`），现走
  `script_exec::output_capped_blocking`。相机单帧会**先丢 5 帧预热**——dshow 的第一帧
  通常是自动曝光未收敛的黑帧，而模型无法把"设备问题"和"房间很暗"分开。

### 4. Outlook（`pim`）与开始菜单（`automation`）

- **Outlook COM 有超时了**：`New-Object -ComObject Outlook.Application` 在 Outlook 弹
  配置文件选择框 / 密码框 / 首次运行向导时**不会失败，会一直等**。这条此前是全仓最后
  一条裸 `.output()` 的捕获路径，一次 `mail_search` 能把整个 turn 挂到 harness 上限并
  留下孤儿 `powershell.exe`。现走 `output_capped`（120s，与 `run_script` 同一常量）。
- **文件夹 id 契约**：`mail_folders` 返回的 `id` 是**全路径**（`Store\Sub\Leaf`），而
  `mail_search` 此前只按**叶子名**匹配 —— 把前一个工具的 id 喂给后一个会**静默落回默认
  收件箱**（有结果、来自错的文件夹、没有任何信号）。现在两种写法都匹配。
- **搜索交给存储层**：`mail_search` 现用 `Items.Restrict`（DASL）让消息存储用自己的索引
  过滤；被拒绝（部分 PST / IMAP 存储没有全文索引）时回落到**有上限的**线性扫描
  （5000 条）。回落路径保留同一个 `-like` 判断，所以两条路的结果集相同。
- **开始菜单有两个根**：`list_shortcuts` / `run_shortcut` 此前只扫 `$env:APPDATA`
  （**per-user** 开始菜单）。绝大多数程序是 all-users 安装，快捷方式在
  `$env:ProgramData` —— 于是一台装了几百个程序的机器只列出十几条，而"开始菜单上明明
  就有"的程序报 "no Start-menu shortcut named X was found"。现在两个根都扫。
  运行改为 `Start-Process` **那个 `.lnk`**（保留发布者设定的工作目录 / 窗口样式，且不再
  等控制台目标退出——此前最长能占满 120s 的脚本上限）。

## Refreshing the daemon binary (App installs)

Windows cannot overwrite a running `.exe`, so **stop first**:

```powershell
aleph-server stop   # or: taskkill /IM aleph-server.exe /F
Copy-Item target\release\aleph-server.exe "$env:LOCALAPPDATA\Aleph\aleph-server.exe" -Force
# restart Aleph.exe — the supervisor relaunches the new binary and reloads the webview
```

See [CLAUDE.md → Panel ↔ Daemon 资源嵌入链](../../CLAUDE.md) for the full
WASM → server-rebuild → relaunch chain (panel changes require recompiling the
server binary, since the panel is `rust_embed`-baked into it).

## Build from source

macOS's `just shell-build` / `just shell-dev` work on Windows unchanged —
the justfile guards macOS-only steps (Swift bridge, etc.) with
`$OSTYPE == darwin*` and automatically appends `.exe` to artifacts. No
separate Windows recipe is needed. The output is an NSIS `.exe` + `.msi`
installer (`tauri.conf.json` `bundle.targets = "all"`) with `aleph-server.exe`
bundled via `externalBin`, zero local config required.

### One-time prerequisites

CI runners have these pre-installed; first-time local builds require manual
setup. The justfile recipes execute via Git Bash (`set shell := ["bash", …]`),
so Git for Windows is required.

| Dependency | How to install | Notes |
|------------|----------------|-------|
| MSVC C++ build tools | Visual Studio "Desktop development with C++" workload | Provides the `x86_64-pc-windows-msvc` linker |
| WebView2 Runtime | Bundled with Windows 11 | Tauri webview host |
| `protoc` | `scoop install protobuf` (or `choco install protoc`) | protobuf build dep for `aleph-server` (CI also installs this) |
| wasm target | `rustup target add wasm32-unknown-unknown` | Compiles the Panel WASM |
| `wasm-bindgen-cli` | `cargo install wasm-bindgen-cli --version 0.2.122 --locked` | **Version must exactly match the `wasm-bindgen` in `Cargo.lock`**; mismatches cause bindgen to refuse output |
| `cargo-tauri` | `cargo install tauri-cli --version "^2.11" --locked` | Provides `cargo tauri build/dev`; major version tracks Cargo.lock `tauri` |
| `just` | `scoop install just` (or `cargo install just`) | Runs justfile recipes |
| Git for Windows `usr\bin` on PATH | Add `…\git\current\usr\bin` to PATH | Shebang recipes (`wasm`, etc.) need `cygpath`; scoop does not add this by default |
| `wasm-opt` (optional) | `scoop install binaryen` | When absent, `just wasm` skips WASM compression; functionality unaffected |

### Full build + run (PowerShell)

```powershell
just shell-build                          # WASM → release server → cargo tauri build
# Installer artifacts:
#   target\release\bundle\nsis\Aleph_<ver>_x64-setup.exe
#   target\release\bundle\msi\Aleph_<ver>_x64_en-US.msi
.\target\release\aleph-desktop-shell.exe  # run the packaged shell directly (aleph-server.exe in same dir); no installer needed
```

Dev mode: `just shell-dev` (debug build + staging daemon, hot-running, no installer output).

---

## Gap Analysis & follow-ups (Windows runtime)

Snapshot of what was reviewed and what remains.

### Reference comparison — `openai/codex` `app-server-daemon`

Codex's daemon manager (`codex-rs/app-server-daemon/src/backend/pid.rs`) is the
closest reference for Aleph's daemon-lifecycle / singleton code. Two takeaways:

- **Aleph already surpasses it on portability.** Codex's entire PID backend is
  `#[cfg(unix)]`: `start`, `try_lock_file`, `process_matches_record`,
  `read_process_start_time` and `force_terminate_process_group` all `bail!`
  ("unsupported on this platform") / return `false` on Windows, and liveness
  shells out to `ps -p <pid> -o lstart=`. Aleph's `instance_lock` uses `fs2`
  (`LockFileEx` on Windows) + `sysinfo`, so the singleton works cross-platform
  with no subprocess.
- **One pattern worth adopting: PID-reuse resistance.** Codex stores
  `PidRecord { pid, process_start_time }` and re-checks the start time so a
  *recycled* PID isn't mistaken for the original process. Aleph's lock
  diagnostic previously used a bare liveness check. **Adopted and improved:**
  Aleph now records the holder's start time in `aleph.lock` and matches it via
  `sysinfo::Process::start_time()` — cross-platform (incl. Windows) and with no
  `ps` fork, surpassing the reference. Fail-safe + backward-compatible: legacy
  single-line lock files and platforms that don't report a start time fall back
  to the prior liveness-only behaviour.

(`openclaw/openclaw` — same product category, "Any OS. Any Platform" — was also
surveyed; its runtime is Node/Swift/Kotlin per-platform, not a shared Rust core,
so it offers product-shape parallels rather than directly portable core code.)

| Area | Status | Notes |
|------|--------|-------|
| Standalone server install on Windows | ✅ done | `scripts/install.ps1` added (was referenced by `cli.rs` but missing) |
| Cross-platform process liveness | ✅ done | `src/utils/process_alive.rs` unifies two `#[cfg]` checks that had opposite, both-wrong Windows fallbacks |
| PID-reuse-resistant lock diagnostic | ✅ done | `aleph.lock` now records holder start time; `process_matches` verifies it (mapped from Codex `PidRecord`, made cross-platform via `sysinfo`) |
| `bootstrap-runtime` parallelism | ⛔ rejected | `install()` mutates process-global `PATH` via `set_var`; parallelizing would race the env. Sequential install is the correct design. |
| `uv` venv post-install path (Windows) | ✅ verified correct | `expand_home` rewrites `/bin/python` → `\Scripts\python.exe` and expands repair args; no fix needed |
| `stop`/`status` for non-daemon Windows servers | ⏳ follow-up (design refined — see below) | `gateway.pid` is written only by the Unix `daemonize`, so `stop`/`status` can't see a foreground/supervised server |

### Follow-up design note: why `stop`/`status` is non-trivial on Windows

The obvious fix — "when `gateway.pid` is absent, read the holder PID from
`aleph.lock` instead" — **does not work on Windows**, and the trap is
non-obvious:

- `instance_lock` takes an exclusive whole-file lock via `fs2`, which is
  `LockFileEx` on Windows. Unlike Unix advisory `flock`, that lock is
  **mandatory**: while the server holds it, any *other* process's `ReadFile`
  on the locked range fails with a lock violation. So a separate `aleph status`
  process cannot read `aleph.lock`'s contents while the server is running —
  exactly the case we'd want to detect. (This is already documented in
  `instance_lock`'s `#[cfg(not(windows))]` test gates.)

Therefore the correct fix is **not** a lock-file read. It is to write a
separate, *unlocked* PID file (`gateway.pid`) on the foreground / all-platform
`start` path — not just inside the Unix `daemonize` — so `stop`/`status` read
it on every platform. That is a startup-path change (interacts with the
singleton-lock PID rewrite and exit cleanup) and warrants compile + runtime
verification, so it is deferred rather than shipped unverified. Auto-killing is
intentionally *not* part of it: a supervised instance (the Aleph app's Tauri
supervisor) would just relaunch, so `stop` should report the PID and the
platform-appropriate command rather than force-terminate.
