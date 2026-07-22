# 静态代码审查报告 — desktop-linux

- **审查单元**: `desktop/linux` — Linux 平台实现（unsafe/FFI、X11/Wayland 边界、错误处理）
- **审查日期**: 2026-07-22
- **基线**: `/tmp/aleph-review-batch-5`（与 main 一致的 git worktree）
- **方法**: 全量人工静态阅读（rust-doctor JSON 为空文件，无辅助线索）

## 统计

| 指标 | 值 |
|------|-----|
| 源文件数 | 9（含 `lib.rs`） |
| 总行数 | 2354 |
| 最大文件 | `permission.rs`（540 行，其中测试+模块文档约 340 行） |
| unsafe 代码 | **0 处**（无任何 FFI/unsafe，全部经 shell-out 或 std API） |
| 生产代码 `unwrap()`/`expect()` | 0 处（仅测试内使用） |

文件清单：`lib.rs` (118)、`permission.rs` (540)、`system.rs` (383)、`media.rs` (356)、`pim.rs` (319)、`clipboard.rs` (227)、`escape_listener.rs` (175)、`automation.rs` (162)、`sleep_inhibitor.rs` (74)。

## 发现列表（按严重级排序）

### Critical
无。

### High
无。

### Medium

**M1. `clipboard.rs:73-94`、`clipboard.rs:36-70` — 剪贴板工具按"二进制存在"而非会话类型选择，X11 上装有 wl-clipboard 时读返回空、写静默失败**

`read_text()` 与 `write()` 都用 `.output()/.spawn()` 的 **spawn 是否成功** 做降级判断（`or_else`）。wl-clipboard 在 X11 桌面发行版上经常已安装：`wl-paste` 能 spawn 成功但退出码非零（"no Wayland display"），于是：

- `read_text()` 直接返回 `Ok("")`（空 stdout），永远不会 fallback 到 xclip/xsel → 剪贴板读取在 X11 上会话静默返回空串。
- `write()` 中 `child.wait()` 的**退出码被完全忽略**（`clipboard.rs:65-69`），wl-copy 失败也返回 `Ok(())` → 写剪贴板静默无操作（数据丢失且调用方无感知）。

**建议**: 选择工具时先检查会话（`WAYLAND_DISPLAY`/`XDG_SESSION_TYPE`），或在 spawn 成功后也检查退出码，非零时继续 fallback；`write()` 必须检查 `wait()` 返回的 `ExitStatus.success()`。

**M2. `permission.rs:309` — `open_settings_panel()` 用 `.status().await` 等待设置程序退出，阻塞直到用户关闭窗口**

`Command::new("gnome-control-center").status().await` 会等待子进程**退出**。GNOME 控制中心窗口一直开着进程就不退出，于是 `open_settings()` / `request()`（`permission.rs:353` 也调用它）会一直阻塞当前 async 任务，可能挂住整个 agent turn，直到用户手动关闭设置窗口。

**建议**: 改用 `spawn()`（fire-and-forget）并视 spawn 成功为已派发；或对 `status()` 加短超时后 `kill`（gnome-control-center 通常 daemonize/fork，先 spawn 再等极短时间即可）。

**M3. `system.rs:68-69` — `quit_app` fallback 到 `pkill -f <app_name>`，正则子串匹配可误杀无关进程**

`app_name`（LLM/用户输入）作为 ERE 传给 `pkill -f`，对**整条命令行**做子串/正则匹配。`killall`（精确进程名）失败后进入此分支，传入如 `"aleph"`、`"sh"` 之类的名字会杀掉所有 cmdline 含该子串的进程——可能包括 Aleph 自身或其父进程（项目 AGENTS.md 本身就对 `pkill -f` 误匹配发出过 vault 数据丢失警告）。另外 `app_name` 中的正则元字符会被当作模式解释。

**建议**: 收窄匹配（`pkill -x` 精确名、或 `-f "^..."` 锚定 + 先转义正则元字符），或在文档/指南中明确该 fallback 的误杀面；至少对空串/过短输入拒绝执行。

### Low

**L1. `media.rs:124-136` — `camera_snap` 读帧失败路径泄漏临时文件**

`std::fs::read(&out)` 失败时直接返回 Err，`remove_file` 在成功路径之后才执行，`/tmp/aleph-media-*.jpg` 残留。建议在读取后无条件清理（先 read 再 remove，把 read 错误延后返回）。

**L2. `pim.rs:85` — `find_next_from_line` 循环边界 off-by-one**

`while i + 6 < bytes.len()` 访问到 `bytes[i+5]`，正确条件应为 `i + 5 < bytes.len()`（或 `i + 6 <= bytes.len()`）。当前写法漏检位于文件最末尾 6 字节处的 `\nFrom ` 分隔符（会让最后一条消息吞掉一个尾部空 "From " 行）。实际危害极小。

**L3. `pim.rs:269` — `mail_get` 用 `splitn(2, ':')` 拆分 `folder_id:offset`，folder_id 含 `:` 时解析错位**

`message_id` 由 `mail_search` 以 `format!("{}:{}", folder_id, offset)` 生成（`pim.rs:250`），folder_id 来自相对路径——Linux 文件名允许 `:`（Thunderbird 文件夹名同样允许）。一旦含冒号，`splitn(2, ':')` 会在第一个冒号处切开，folder_id 截断 → 报 "Folder not found"。建议改用 `rsplit_once(':')`。

**L4. `permission.rs`（540 行）— 超过 500 行超大文件阈值**

其中约 190 行为实现代码、其余为测试与模块文档，严重性低。如需收敛可将纯映射函数（`ungated_status`/`steps`/`rationale`）拆到 `permission/mapping.rs`。

**L5. `pim.rs:198-217` — `mail_folders`/`mail_search` 全量读入并解析每个 mbox 文件**

对每个 mbox 调 `read_to_string` 全量加载（大邮箱可达数百 MB），且 `mail_folders` 仅为计数就完整解析全部消息。属性能问题而非正确性问题；如该工具被频繁调用会放大内存占用。建议按行扫描计数 / 流式搜索。

## 架构红线合规快照

| 红线 | 状态 | 说明 |
|------|------|------|
| R1 core 不调平台 API | ✅ | 本 crate 即平台侧实现，实现 `desktop/shared` 定义的 trait（`LinuxPlatform` 聚合各 capability） |
| R2 原生 shell 仅窗口容器 | ✅ | 无 UI 代码 |
| R3 core 极简、无重依赖 | ✅ | 无 X11/Wayland client 绑定、无 zbus；全部 shell-out 到 freedesktop 标准工具（wl-clipboard/xclip/ffmpeg/pactl/gdbus）；新增运行依赖仅 `base64`/`image`（workspace 已有） |
| R4 接口层纯 I/O | ✅ | 不涉及 |
| R7 Rust Core 唯一大脑 | ✅ | 仅 capability 实现，无决策逻辑 |
| R8 正则仅用于机器格式 | ✅ | `pim.rs` 的 mbox/`From ` 解析、`pactl` 输出解析均为机器格式；无意图/路由逻辑 |
| R9 可配置项暴露为工具 | ✅ | 仅 `ALEPH_CAMERA_DEVICE` env 覆盖，与 macOS 端惯例一致 |
| R10 智能在 prompt 中 | ✅ | 无启发式智能逻辑 |

## 其他核查结论（确认无问题）

- **命令注入**: 所有外部命令均走 `Command::new(bin).args(...)`，无 shell 拼接；`run_script` 的 `bash -c <source>` 是 API 契约本身（执行任意脚本），非缺陷。
- **panic 面**: 生产路径无 `unwrap`/`expect`/`panic!`；`parse::<u64>().unwrap_or(0)` 等均有兜底。
- **子进程生命周期**: `clipboard::write` 的 `wait()` 会先关闭 stdin（std 保证），无 EOF 死锁；`sleep_inhibitor` drop 中 `kill + wait` 正确回收 `sleep infinity`；`automation.rs` 统一走 `output_capped`（超时+kill_on_drop）。
- **资源泄漏**: `InhibitorGuard`、sentinel 文件（`start/stop/reset` 均清理）处理正确；唯一遗漏见 L1。
- **escape_listener**: 文档化的 sentinel 方案在 X11/Wayland/headless 行为一致，`is_aborted` 无竞态后果。
- **temp_path**（`media.rs:57`）: pid + 纳秒时间戳，碰撞/可预测符号链接攻击风险可忽略。
