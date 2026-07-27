# Linux 桌面端深度硬化与 AT-SPI 无障碍层 (Design)

- **日期**: 2026-07-28
- **范围**: FEATURE_LOCATOR §7.1–§7.4，**仅 Linux 平台**
- **分支**: `worktree-linux-desktop-hardening`
- **红线**: R1（大脑-四肢分离）/ R3（核心轻量化）/ P6（KISS·YAGNI）/ P7（防御性设计）

---

## 1. 问题陈述

Aleph 的 Linux 桌面端是三平台里最薄的一条腿。它不是"没写"，而是**写了但连不上**：

| 症状 | 根因 |
|---|---|
| `window_list` / `focus_window` / `move_window` / `resize_window` 在多数机器上全炸 | `action/window.rs` 的 Linux 臂**只认 `wmctrl`**，无任何回退；`xdotool`（远更常见）零使用；Wayland 合成器零覆盖 |
| 装了 wl-clipboard 的 X11 机器上写剪贴板**静默 no-op** | 剪贴板有**两份实现**，且两者的 `.or_else` 只在 **spawn 失败**时回退、**不检查退出码** |
| Wayland 上 `scroll` 静默失败 | `wayland_input` 覆盖了 click/type/key/drag/hover/mouse_button，**独漏 `scroll` 与 `cursor_position`** |
| `permission(request \| open_settings)` 挂住整个 turn | `open_settings_panel` 用 `.status()` 等 GUI 进程退出 |
| `quit_app("chrome")` 可能误杀无关进程 | `LinuxSystem::quit_app` 用 `pkill -f`（匹配整条命令行），与 `action::quit_app` 的 `pkill -x` 语义分叉 |
| `launch_app("firefox")` 基本不工作 | `action::launch_app` 裸 `xdg-open <app_name>`（xdg-open 只吃 file/URL） |
| 摄像头卡住时挂到 harness 300s 上限 | `media.rs::run_ffmpeg` 无超时（同 crate 的 `automation.rs` 已有 `output_capped` 却未复用） |
| **Linux 上不存在密码框硬拒** | `focus_gate` 依赖 `platform.ax()`，而 `LinuxPlatform::ax() = None` |
| `ax_snapshot` / `ax_query` / `ax_action` / `set_value` 四工具 + `desktop_som` 语义模式 + `observe.post_state` 在 Linux 全部不可用 | 同上：**无无障碍层** |
| 改一处会话探测漏三处 | 会话类型在 **3 处各写一份**，剪贴板还靠 spawn 顺序隐式探测 |
| `list_running_apps` 名字截断到 15 字符、`is_active` 恒 false | `ps -eo comm` |

对标参考项目（openclaw / orca / UI-TARS-desktop / open-codex-computer-use）的通行做法：**多后端探测 + 单一会话真源 + 无障碍树作为一等定位手段**。Aleph 在 macOS / Windows 上都已具备这三样，Linux 上一样都没有。

> **参考项目源码本轮不可达**（`smb://mac-mini-m4.local/tbu4/` 未挂载）。对标依据为 FEATURE_LOCATOR 已记录的两轮 6-项目 gap analysis（2026-07-15 / 2026-07-17）与既有认知，不含新的逐行比对。

---

## 2. 设计

### L1 · Linux 会话与工具探测：单一真源

**新增** `desktop/shared/src/linux/session.rs`（`#[cfg(target_os = "linux")]` 生产 + 纯逻辑跨平台可测）：

```rust
pub enum SessionKind { X11, Wayland, Unknown }
pub enum Compositor  { Sway, Hyprland, Kde, Gnome, Other, None }

pub struct LinuxSession { kind: SessionKind, compositor: Compositor }
pub struct ToolBox { /* xdotool, wmctrl, xclip, xsel, wl_copy, wl_paste, ydotool, … */ }

pub fn session() -> &'static LinuxSession;  // OnceLock
pub fn tools()   -> &'static ToolBox;       // OnceLock，一次 PATH 扫描
```

**为什么落在 `shared` 而不是 `desktop/linux`**：依赖方向是 `linux → shared`，而 `action/` `perception/` 的 Linux 臂住在 `shared` 里。真源必须在被依赖的一侧，否则两边各写一份就是今天的病。

**收口**（删除 3 份重复 + 1 处隐式探测）：
- `action/wayland_input.rs::is_wayland_session` / `ydotool_available`
- `desktop/linux/src/permission.rs::detect_session`（`SessionType` 枚举整个删除，改用 `SessionKind`）
- `desktop/linux/src/system.rs::user_idle_seconds` 内联的 `XDG_SESSION_TYPE` 判断
- `desktop/linux/src/clipboard.rs` 靠 spawn 顺序的隐式探测

**错误信封统一**：缺工具时报 `DesktopError::NotImplemented`/`PlatformError` 并**指名装什么**（`sudo apt install xdotool`），不再让调用方看到一条裸 `No such file or directory`。

### L2 · 窗口子系统：三后端探测（含 Wayland 合成器）

**新增** `desktop/shared/src/action/window_linux.rs`，从 `window.rs` 抽出 Linux 臂：

```rust
enum WindowBackend {
    Xdotool,        // X11 首选：装机率最高
    Wmctrl,         // X11 次选：保留既有能力（EWMH 桌面号等）
    Sway,           // Wayland：swaymsg -t get_tree / [con_id=N] focus|move|resize
    Hyprland,       // Wayland：hyprctl -j clients / dispatch focuswindow|movewindow|resizewindowpixel
    Kdotool,        // Wayland(KDE)：kdotool search/windowactivate/…（xdotool 兼容 CLI）
}
```

- **选择规则**：`session().kind` + `compositor` + `tools()` 三者交，纯函数 `pick_backend()`，单测覆盖矩阵。
- **每后端四动作**（list / focus / move / resize）各自的 **argv 构建 + 输出解析都是纯函数**，无显示器可测。
- **窗口 id 语义**：X11 后端沿用 XID（`u64`，变宽 hex，禁止定宽截断——这是 2026-06-19 修过的老伤）；sway 用 `con_id`，Hyprland 用 `address`（hex u64）。三者都落进同一个 `WindowInfo.id: u64` 且**在同一台机器上不会共存**（后端唯一），故无歧义。
- **能力诚实**：某后端做不到的动作（如 sway 无法按像素移动浮动窗口以外的窗口）返回 `NotImplemented` 并说明原因，**不假装成功**。

**删除**：`window.rs` 里的 `linux_window_list` / `linux_focus_window` / `linux_move_window` / `linux_resize_window` / `linux_wmctrl_geometry` / `parse_wmctrl_line`（迁入新模块，wmctrl 解析原样保留为 `Wmctrl` 后端）。

### L3 · 剪贴板：单一真源 + 退出码校验

**新增** `desktop/shared/src/linux/clipboard.rs`：按 `session()` 选工具顺序（Wayland → `wl-copy`/`wl-paste` 优先；X11 → `xclip`/`xsel` 优先），**逐候选检查退出码**，失败才继续下一个，全失败报"装什么"。

- **删除** `action/input.rs` 的 `#[cfg(target_os = "linux")]` 剪贴板臂（改委派）。
- `desktop/linux/src/clipboard.rs` **只保留图像读取**（`pick_image_target` / `to_png_base64` 纯函数原样保留，已有测试全部保留），文本路径改委派 L3 真源。

### L4 · Wayland 输入补全

- `wayland_input.rs` 补 `scroll`：`ydotool mousemove --wheel -x <h> -y <v>`（纯 argv 构建 + 单测）。
- `input.rs::scroll` 补 `should_use_ydotool()` 分支。
- `input.rs::cursor_position` 在 Wayland 显式返回 `NotImplemented`（Wayland 无通用指针查询；今天是静默走 enigo 失败，报一条无从下手的错）。

### L5 · 应用启动 / 退出：单一实现

**新增** `desktop/shared/src/linux/app.rs`：

- **launch**：① 解析 `.desktop`（遍历 `XDG_DATA_HOME` + `XDG_DATA_DIRS` 的 `applications/`，按 `desktop-file-id` / `Name=` / 文件名匹配）→ `gtk-launch <id>`；② 否则 PATH 上的可执行 → 直接 detached spawn；③ 仅当参数看起来是 URL 或存在的路径时才 `xdg-open`。
- **quit**：① 优先 WM 优雅关闭（活动后端的 close 动作，让应用走正常退出/保存流程）；② 回退 `pkill -x --`（精确 comm 匹配）；③ **永不 `pkill -f`**。
- `action/app_launch.rs` 的 Linux 臂与 `desktop/linux/src/system.rs::{launch_app, quit_app}` **双双委派**同一实现。**删除** `killall` / `pkill -f` 分叉。

### L6 · 单窗口截图

`perception/screenshot.rs` 新增 `take_screenshot_window(window_id)`，走 `xcap::Window`（按 id 匹配），点亮 Linux 上的 `screenshot { window_id }`。不可用时返回 `NotImplemented` 说明原因，不静默退化成全屏（那会把别的窗口内容泄漏给模型）。

### L7 · AT-SPI2 无障碍层（本轮最大能力增益）

**新增** `desktop/linux/src/ax.rs`，实现 `AccessibilityCapability`。依赖 `atspi` crate（跑在**已在 Cargo.lock 的 `zbus 5.16`** 之上），**只进 `desktop/linux`**（四肢 crate，R1/R3 合规）。

严格镜像 `desktop/windows/src/ax.rs` 的两条设计（它就是为同一问题写的）：

1. **role 词汇 macOS 化** —— `atspi_role_to_ax_role()` 把 AT-SPI role 映射到 `"AX*"`：

   | AT-SPI | AX | 是否 interactable |
   |---|---|---|
   | `push_button` | `AXButton` | ✓ |
   | `toggle_button` / `check_box` | `AXCheckBox` | ✓ |
   | `radio_button` / `page_tab` | `AXRadioButton` | ✓ |
   | `combo_box` | `AXComboBox` | ✓ |
   | `entry` / `text` | `AXTextField` | ✓ |
   | **`password_text`** | **`AXSecureTextField`** | ✓（且 `secure = Some(true)`） |
   | `link` | `AXLink` | ✓ |
   | `menu_item` / `check_menu_item` / `radio_menu_item` | `AXMenuItem` | ✓ |
   | `slider` | `AXSlider` | ✓ |
   | `spin_button` | `AXIncrementor` | ✓ |
   | `frame` / `window` / `dialog` | `AXWindow` | ✗ |
   | `panel` / `filler` | `AXGroup` | ✗ |
   | `label` / `static` | `AXStaticText` | ✗ |
   | 其余 | `AXUnknown` | ✗ |

   于是 `interactable.rs::INTERACTABLE_ROLES` / `desktop_som` / `gui_locate` / `focus_gate` **零改动**点亮。

2. **无状态 locator** —— 每次调用现连 a11y bus，不跨调用持句柄（与 macOS Swift 侧同形，杜绝跨 IPC 句柄失效）。

**实现的方法**：`query_focused` / `query_tree` / `query_by_role` / `set_value`（AT-SPI `EditableText` + 读回验证）/ `perform_action`（AT-SPI `Action` 接口，`AXPress → click|activate|press`）。

**关键护栏 —— `ax()` 仍可能返回 `None`**：
`LinuxPlatform::new()` 探测一次 a11y bus 可达性（`AT_SPI_BUS` 环境变量 / `org.a11y.Bus` D-Bus 名 / `/run/user/*/at-spi/bus*`）。**不可达就返回 `None`**，而不是一个恒错的 `Some`。理由：`focus_gate::check` 对 `Err` 是 fail-open 但会打 `warn!`，一个恒错的 AX 层会让每次 `type_text` 都吐一条无用告警——比今天更糟。

**熵减**：把 `desktop/windows/src/ax.rs` 里的纯 `rank_candidates` + `RankCandidate` 上提到 `desktop/shared/src/ax_rank.rs`，Windows / Linux 共用一份。否则这是第三次手抄同一个打分器（macOS 在 Swift 侧有一份，无法共享，故只收敛 Rust 侧两份）。

**副作用（有意）**：`ax()` 返回 `Some` 后，`focus_gate` 在 Linux **自动生效**——密码框硬拒（`force:true` 也压不下去）、无焦点拒绝、不可写元素拒绝，与 macOS / Windows 语义一致。AT-SPI 未开启的应用（未加载 atk-bridge）报告 `settable: None` → **fail-open**，不会误伤。

### L8 · 系统类工具打磨（§7.4）

| 位置 | 改动 |
|---|---|
| `desktop/linux/src/permission.rs::open_settings_panel` | `.status()` → `.spawn()`（非阻塞）；候选表补 XFCE (`xfce4-settings-manager`)；`SessionType` 换成 L1 的 `SessionKind` |
| `desktop/linux/src/media.rs::run_ffmpeg` | 复用 `script_exec::output_capped` 加超时（`duration + 余量`），与 `automation.rs` 同源 |
| `desktop/linux/src/system.rs::list_running_apps` | `ps -eo comm`（15 字符截断）→ 读 `/proc/*/comm` + `/proc/*/cmdline` 不截断；`is_active` 由活动窗口真填；GUI app 由窗口列表的 pid 集合标注 |

---

## 3. 验证策略

**纯函数单测（无显示器、任意 host 可跑）** —— 本轮所有新逻辑的主力：
- 会话/合成器探测矩阵（env 组合 → `SessionKind` / `Compositor`）
- `pick_backend()` 选择矩阵（session × compositor × 可用工具）
- 五后端各自的 argv 构建 + 输出解析（xdotool 的 `getwindowgeometry` 文本、`swaymsg -t get_tree` JSON、`hyprctl -j clients` JSON、wmctrl 行——既有测试全部保留）
- `.desktop` 解析与应用名匹配
- AT-SPI role 映射表（含 `password_text → AXSecureTextField` + `secure` 断言）
- ydotool `scroll` argv

**本机 X11 端到端**（XFCE / `DISPLAY=:10.0`，装了 `xdotool ffmpeg notify-send gdbus gtk-launch xprop`，**没装** `wmctrl xclip xsel wl-clipboard tesseract`）：
- `window_list` / `focus_window` 经 Xdotool 后端
- 单窗口截图
- AT-SPI 快照（需要在测试进程环境里开 `GTK_MODULES=gail:atk-bridge`，**不改用户系统 gsettings**）
- 剪贴板：本机三个工具全无 → 必须给出"装什么"的可执行错误，**而不是崩或静默成功**。这正是多后端探测要证明的性质。

**命令**：`cargo test -p aleph-desktop -p aleph-desktop-linux` + `cargo clippy`，按本机内存约束加 `CARGO_PROFILE_TEST_DEBUG=0 -j2`。

---

## 4. 风险与边界

1. `atspi` crate 需联网拉取（crates.io 已确认可达），API 形状需在实现时对齐 zbus 5.16——若版本冲突不可解，退路是用 `zbus` 直接写 AT-SPI proxy（协议是稳定的 D-Bus 接口，工作量增加但无阻塞）。
2. Wayland 三后端（sway / Hyprland / kdotool）本机无法端到端验证（本机是 X11），本轮只有单测覆盖 argv 与解析。**这一点必须写进 FEATURE_LOCATOR，不得声称"已验证"**。
3. `xcap::Window` 的 Linux 后端能力需实现时确认；若不支持按 id 捕获，L6 降级为 `NotImplemented` + 说明（而非静默全屏）。
4. 本轮**不做**：`mic_level` 实时表、PIM 的 EDS(Evolution Data Server) 日历/联系人、Linux 的定向输入轨道（X11 的 `XSendEvent` 会被多数应用忽略，做了也不可靠——保持全局轨 + `delivery: "global"` 的诚实报告）。

---

## 5. 完成定义

- 上述 L1–L8 全部落地，`cargo test` / `cargo clippy` 干净。
- 被替换的旧代码**删除**而非注释（熵减原则）。
- FEATURE_LOCATOR §7.1–§7.4 更新，含"Wayland 后端仅单测覆盖"的诚实标注。
- CLAUDE.md 若有受影响的注记（桌面分层描述）同步订正。
