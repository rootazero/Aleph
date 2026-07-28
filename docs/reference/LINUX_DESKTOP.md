# LINUX_DESKTOP.md — Linux 桌面端能力矩阵与运维

> 本文回答三个问题：**这台机器上 Aleph 的桌面能力能到哪一步**、**缺了什么装什么**、**出问题先看哪里**。
> 架构分层见 [FEATURE_LOCATOR.md §7](FEATURE_LOCATOR.md)；Windows 侧的对位文档是 [WINDOWS_RUNTIME.md](WINDOWS_RUNTIME.md)。

---

## 1. 一句话架构

Linux 的桌面实现**跨两个 crate**，找错目录会以为功能不存在：

| 能力 | 实现位置 |
|---|---|
| 会话/合成器/可用工具探测（**单一真源**） | `desktop/shared/src/linux/session.rs` |
| 窗口管理 | `desktop/shared/src/action/window_linux/`（`x11.rs` 原生 EWMH · `sway.rs` · `hyprland.rs`） |
| 剪贴板 | `desktop/shared/src/linux/clipboard.rs` |
| 启动 / 退出应用、进程枚举 | `desktop/shared/src/linux/{app,proc}.rs` |
| 鼠标键盘输入 | `desktop/shared/src/action/input.rs`（X11 走 enigo/XTEST）+ `wayland_input.rs`（Wayland 走 ydotool） |
| 截图 / 录屏 / OCR | `desktop/shared/src/perception/`（xcap · ffmpeg x11grab · wf-recorder · tesseract） |
| shell-out 死线（同步侧） | `desktop/shared/src/script_exec.rs::output_capped_blocking` |
| **无障碍树 (AT-SPI2)** | `desktop/linux/src/ax/`（`cache.rs` 批量预取 · `budget.rs` 墙钟预算 · `bus.rs` 共享连接） |
| 系统 / 权限 / 媒体 / 自动化 / PIM | `desktop/linux/src/` |

依赖方向是 `desktop/linux → desktop/shared`，所以任何两边都要用的东西（会话类型、剪贴板、启动器）**必须落在 shared**。

---

## 2. 能力矩阵：X11 vs Wayland

| 能力 | X11 | Wayland | 说明 |
|---|---|---|---|
| 截图（全屏 / 区域） | ✅ | ✅ | `xcap`；Wayland 经 xdg-desktop-portal，首次会弹授权对话框 |
| 单窗口截图 `screenshot{window_id}` | ✅ | ❌ | `xcap::Window` 在 Wayland 无法枚举窗口，报"没有该 id 的窗口"而非误截全屏 |
| OCR | ✅ | ✅ | 需 `tesseract`；缺失时报错点名要装的包 |
| 录屏 | ✅ | sway ✅ · Hyprland ✅ 需 `wf-recorder` · GNOME ❌ · KDE ❌ | 后端**按会话类型选，不按 `DISPLAY` 在不在**——几乎每个 Wayland 会话都为 XWayland 导出 `DISPLAY`，照那个判就会用 x11grab 录下一个空的 XWayland 根窗口（黑屏视频，报成功） |
| 鼠标 / 键盘 / 滚轮 | ✅ | ⚠️ 需 ydotool | Wayland 合成器屏蔽 XTEST；`ydotool` + `ydotoold` 走内核 uinput 绕过。**没装 ydotool 时直接报错**，不再回落到必然无效的 XTEST 并报成功 |
| 读取指针位置 | ✅ | ❌ | Wayland 协议不向应用暴露全局指针；显式报 `NotImplemented` 并指路 SOM/gui_locate |
| 窗口枚举 / 聚焦 / 移动 / 缩放 / 关闭 | ✅ 原生 EWMH，**零外部依赖** | sway ✅ · Hyprland ✅ · GNOME ❌ · KDE ❌ | 见下节 |
| 剪贴板（文本 + 图片） | ✅ 需 `xclip`/`xsel` | ✅ 需 `wl-clipboard` | 候选顺序按会话类型定；**写失败一律报错，绝不静默成功** |
| 通知 | ✅ | ✅ | `notify-send` |
| 空闲时长 | ✅ 需 `xprintidle` | ⚠️ 仅 GNOME | Wayland 侧走 Mutter `IdleMonitor`（`gdbus`） |
| **无障碍树 / SOM / 密码框硬拒** | ✅ | ✅ | AT-SPI2 与显示服务器无关，取决于应用是否加载了 a11y 桥。**前台应用**在没有窗口管理 IPC 的会话（GNOME/KDE Wayland）由 AT-SPI 的 `State::Active` 顶层 frame 回答，不再依赖合成器 |
| 前台应用识别（密码管理器硬阻断的输入） | ✅ EWMH | sway/Hyprland ✅ · GNOME/KDE ✅ 经 AT-SPI | 见上一行；此前在 GNOME/KDE Wayland 恒 `false`，硬阻断等于没有 |
| 摄像头 / 录音 | ✅ | ✅ | `ffmpeg` + V4L2 / PulseAudio·PipeWire |
| 防休眠 | ✅ | ✅ | `systemd-inhibit` / `gnome-session-inhibit` |
| Escape 中止 | ⚠️ 文件哨兵 | ⚠️ 文件哨兵 | Linux 无可移植全局热键；`touch ~/.aleph/desktop-abort` 中止 |

### Wayland 窗口管理的真相

Wayland **刻意没有** X11 EWMH 那样的跨合成器窗口协议。所以：

- **sway / wlroots 系** → `swaymsg`（随 sway 安装）
- **Hyprland** → `hyprctl`（随 Hyprland 安装）
- **GNOME / KDE / 其他** → **不可用**，且这是诚实的答案：合成器根本不向普通客户端暴露窗口管理接口（GNOME 需要 shell 扩展）。此时报错会指出替代路径——截图 + `gui_locate` + AT-SPI 全都照常工作，只是拿不到窗口框。

> **为什么不接 KDE 的 `kdotool`**：它的窗口句柄是不透明 UUID 字符串，塞不进跨平台的 `WindowInfo.id: u64`，除非加一张有状态句柄表——那正是无障碍层设计上刻意避免的"跨调用句柄"。

---

## 3. 装什么

### 最小可用（X11）

```sh
sudo apt install xclip                    # 剪贴板
```

窗口管理**不需要装任何东西**（原生 EWMH）。

### 推荐完整（X11）

```sh
sudo apt install xclip tesseract-ocr tesseract-ocr-chi-sim \
                 ffmpeg libnotify-bin xprintidle at-spi2-core
```

### Wayland

```sh
sudo apt install wl-clipboard ydotool tesseract-ocr ffmpeg libnotify-bin at-spi2-core
sudo systemctl enable --now ydotoold      # 输入注入需要这个守护进程
sudo apt install wf-recorder             # 录屏（仅 sway / Hyprland 等 wlroots 系有效）
```

外加合成器自带的 `swaymsg` / `hyprctl`（若在 sway / Hyprland 下）。

> `wf-recorder` 走 wlroots 的 `wlr-screencopy` 协议，GNOME (Mutter) / KDE (KWin) **不实现**它——在那两个桌面上装了也没用，录屏会如实报不可用并指路"改用周期性截图"（截图走 portal，在那里是好的）。

### 无障碍层的开销（供容量判断）

AT-SPI 每次调用的成本由**往返次数**决定，不由 CPU 决定。本机 XFCE/X11 实测：

| | 之前 | 现在 |
|---|---|---|
| 连接（每次调用） | 424 ms（每次新建） | 付一次（次调 45 ms vs 首调 1.90 s） |
| ~70 节点窗口全深度快照 | 1.88 s | **0.32 s** |
| 边际成本 | ~27 ms/节点 | **~4.6 ms/节点** |

来源是三件事：`Cache.GetItems` 一次拿回整棵树（role/name/states/interfaces/parent）、剩余的几何与动作名并发发出（32 路）、总线连接共享。**墙钟预算 5s** (`ax/budget.rs`) 仍在，用来对付「卡死的应用永远不回 D-Bus」——超时返回**已读到的部分**而不是报错。

不提供缓存的应用（老 Qt、桥只加载了一半）自动回落逐属性读：**慢，但不会空**。

### 打开无障碍层（AT-SPI2）

无障碍树是 SOM 语义定位、`ax_*` 四工具、`set_value`/`ax_action`，以及 **`type_text` 密码框硬拒** 的共同基础。应用只有加载了工具包的 a11y 桥才会出现在 AT-SPI 总线上：

```sh
# GNOME / 大多数 GTK 桌面
gsettings set org.gnome.desktop.interface toolkit-accessibility true

# 通用（对单个应用或整个会话）
export GTK_MODULES=gail:atk-bridge      # GTK2/3
export QT_ACCESSIBILITY=1               # Qt5/6
export QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1
```

**改完必须重启目标应用**——桥是在进程启动时加载的。

---

## 4. 诊断顺序

**"窗口操作报错"**
1. `echo $XDG_SESSION_TYPE` —— Wayland 且不是 sway/Hyprland ⇒ 预期不可用，走截图 + `gui_locate`。
2. X11 下报"Cannot reach an X server" ⇒ daemon 的环境里没有 `DISPLAY`。Aleph 作为 systemd 用户服务启动时尤其常见。
3. 报"No managed window with id …" ⇒ 窗口 id 过期，重新 `window_list`。

**"剪贴板没反应"**
- 读到空串：多半剪贴板真的是空的（所有已安装工具都非零退出时按空处理，见 `linux/clipboard.rs` 模块文档）。
- **写报错并列出试过的工具**：装的工具和会话类型对不上（X11 机器上只装了 `wl-clipboard`，或反之）。装匹配的那个。
- 一个工具都没装 ⇒ 错误直接点名装什么。

**"AX / SOM 说不可用"**
1. `ls $XDG_RUNTIME_DIR/at-spi/` —— 没有 `bus*` 说明 a11y 总线没起，`ax()` 整体返回 `None`。
2. 总线在但某个应用查不到 ⇒ 那个应用没加载 a11y 桥（见上节），**重启它**。
3. 错误信息本身会说这两件事，不必猜。

**"Wayland 上点击/打字没反应"**
- 现在**不会**"没反应"了：Wayland 会话上没装 `ydotool` 时输入动作直接报错并给出装法。若拿到的是那条错误，照它做即可。
- 报错说 `ydotool exited with …` ⇒ 客户端在但 `ydotoold` 没跑，或用户不在 `input` 组、够不到 socket。
- 结果里的 `delivery` 字段说明实际走了哪条轨道，别信请求信结果。

**"某个桌面操作报 `exceeded Ns and was terminated`"**
- 所有桌面 shell-out 现在都带死线（查询类 5s、ydotool 10s、录屏 `duration + 30s`、录屏收尾 20s），超时**不是** Aleph 慢，而是它等的那个桌面服务卡住了——错误里点名了是哪一个。
- 最常见的三个：剪贴板读（`xclip -o` 要等**当前持有选区的那个应用**交出内容，卡死的 Electron 应用永远不交）、`notify-send`（等通知守护进程的 D-Bus 回复）、`swaymsg`（等合成器 socket）。
- 死线之前这些都会把整个 turn 挂到 harness 的 300s 上限并泄漏子进程。

**"退出应用没生效 / 找不到进程"**
- 匹配的是**可执行名**，精确匹配，永不匹配命令行（这是安全约束：`pkill -f` 曾能杀掉任何参数里提到该名字的进程，包括 agent 自己）。
- 用 `system` 工具的应用列表拿准确名字。

---

## 5. 验证状态（诚实标注）

| 部分 | 验证方式 |
|---|---|
| X11 窗口枚举 / 聚焦 / 几何 / 活动窗口 | ✅ 本机 XFCE/X11 端到端实测 |
| 单窗口截图 | ✅ 端到端实测 |
| AT-SPI 三查询 + 角色映射 + 动作名 | ✅ 端到端实测（20 应用、124 节点树） |
| 会话探测 / 剪贴板选序 / `.desktop` 解析 / `/proc` 匹配 / ydotool argv | ✅ 纯函数单测 |
| 输入轨道选择矩阵（X11 / Wayland±ydotool） | ✅ 纯函数单测 |
| 录屏后端选择矩阵 + `wf-recorder` argv | ✅ 纯函数单测 |
| 应用列表折叠（活动标记 / 代表 pid / 枚举序无关） | ✅ 纯函数单测 |
| shell-out 死线（超时杀进程 / 大输出不死锁 / stdin 投喂） | ✅ 单测（真起子进程） |
| AT-SPI 批量缓存 / 并发富化 / 共享连接 | ✅ 本机端到端实测（含缓存值与实时属性逐字段一致性断言、墙钟上限断言） |
| 密码词表启发式 / `set_value` 回读脱敏 / 拖拽插值 / `/dev/video` 择位 / inhibitor 存活判定 | ✅ 纯函数单测 |
| **sway / Hyprland 后端** | ⚠️ **仅单测覆盖 argv 与 JSON 解析**——开发机是 X11，没有端到端验证 |
| **AT-SPI `Value` 接口写路径（滑块/微调框）** | ⚠️ 无端到端验证——本机没有暴露该接口的应用在跑 |
| **GNOME/KDE Wayland 的 `State::Active` 前台回落** | ⚠️ 同上（开发机是 X11） |
| **摄像头（预热丢帧 / `/dev/video` 探测）** | ⚠️ 本机无摄像头，`/sys/class/video4linux` 不存在；选择逻辑有纯函数单测 |
| Wayland 输入 / 截图 / 剪贴板路径 | ⚠️ 同上 |
| **`wf-recorder` 录屏** | ⚠️ 同上（argv + 后端选择有单测，真实录制未验；SIGINT 收尾逻辑无端到端覆盖） |
