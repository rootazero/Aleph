# 静态代码审查报告 — desktop-windows

- **审查路径**: `desktop/windows`
- **审查日期**: 2026-07-22（基于 main worktree 全量静态阅读，非 PR diff）
- **统计**: 9 个 Rust 文件（8 个 src + 1 个集成测试），共 3441 LOC
  - `ax.rs` 1030 · `media.rs` 512 · `permission.rs` 432 · `pim.rs` 381 · `system.rs` 363 · `escape_listener.rs` 245 · `automation.rs` 180 · `lib.rs` 132 · `sleep_inhibitor.rs` 116 · `tests/sleep_inhibitor.rs` 50
- rust-doctor 诊断 JSON（`/tmp/rd-desktop.json`）为空文件，无可用线索；全部结论来自人工阅读。

## 历史问题核对

| 历史问题（2026-07-20） | 现状 |
|---|---|
| `escape_listener.rs` 键盘钩子 use-after-free | **主体已修复**：新增 `Drop`（drop 即 stop）；`stop()` 先 unhook、清零 `LISTENER_PTR` 再释放堆状态。仍有残余竞态，见 Low-4。 |
| 钩子无消息循环 | **未修复**，且影响比旧报告更严重：回调永不派发 → 功能静默失效。见 High-1。 |
| `ax.rs` COM 初始化错误忽略 | **部分存在**：`CoInitializeEx` 的 HRESULT 仍被忽略，且 `Drop` 无条件 `CoUninitialize`，失败路径下会失衡。见 Medium-2。 |
| `ax.rs` AX 解析回退到前台进程 | **已修复**：`resolve_root_hwnd`（ax.rs:360-379）在显式给定 pid 但无可见窗口时返回 `NotAvailable`，注释明确说明不再静默回退到前台应用；仅 `pid=None` 时使用前台窗口（设计意图）。不再报告。 |

## 发现列表（按严重级排序）

### High-1 · `escape_listener.rs:97-104` — 全局键盘钩子安装在无消息循环的线程上，Escape 紧急中止静默失效

`SetWindowsHookExW(WH_KEYBOARD_LL, ..., dwThreadId=0)` 安装的进程级低级键盘钩子，其回调由系统**向安装线程投递消息**的方式触发，安装线程必须有 Win32 消息循环（GetMessage/DispatchMessage）回调才会执行。而 `start()` 的唯一调用点 `src/builtin_tools/desktop/mod.rs:204` 在 tokio async 上下文中首次调用——tokio worker 线程永不 pump Win32 消息，模块内也没有创建带消息循环的专用线程。结果：钩子安装"成功"、`start()` 返回 `Ok`，但 `keyboard_hook_proc` 永远不会被调用，Escape 紧急中止（文档定位为"用户紧急停止"安全功能）在 Windows 上**完全失效且无任何告警**（调用方仅在 `Err` 时 warn）。

**建议**：`start()` 内 spawn 一个专用线程，在该线程上安装钩子并运行 `GetMessage` 消息循环（`WM_QUIT` 退出，配合 `PostThreadMessage` 实现 `stop`）；这也顺带解决 Low-4 的跨线程释放竞态。

### Medium-2 · `ax.rs:333-345` — `ComGuard` 忽略 `CoInitializeEx` 失败，Drop 无条件 `CoUninitialize` 可失衡

```rust
unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
```

注释称 `S_OK`/`S_FALSE` 都是成功——对，但 HRESULT 还有失败值。关键是 `RPC_E_CHANGED_MODE`：当线程已被其他代码以 STA 初始化时本次调用**失败、不增加引用计数**，而 `Drop` 仍调用 `CoUninitialize()`，会把别人初始化的 apartment 引用计数误减，可能导致同一（被 tokio blocking 池复用的）线程上后续 COM 使用者在公寓被提前卸载后崩溃或诡异失败。这些调用运行在 `spawn_blocking` 池线程上，线程长期复用，被其他组件先初始化的场景并非纯理论。

**建议**：记录 `CoInitializeEx` 的 HRESULT，仅在 `S_OK`/`S_FALSE` 时在 Drop 中 `CoUninitialize`；`RPC_E_CHANGED_MODE` 下不 uninit（UIA 在 STA 下亦可用，可继续）。

### Medium-3 · `automation.rs:122-152` — `run_shortcut` 无超时且同步等待被启动应用退出

脚本 `& $shortcut.TargetPath $shortcut.Arguments` 以调用运算符同步启动目标进程，PowerShell 会等目标退出才返回；`cmd.output().await` 没有任何超时。启动一个 GUI 程序（如记事本的 .lnk）时，该工具调用会一直阻塞到用户关闭该应用，占用 tokio 阻塞资源和工具调用回合。对照同文件 `run_script` 使用 `output_capped(..., RUN_SCRIPT_TIMEOUT)`，此处缺了同等保护。

**建议**：改用 `Start-Process`（异步启动即返回），或至少套用与 `run_script` 相同的超时上限。

### Low-4 · `escape_listener.rs:146-156` — `stop()` 清零指针后仅 `yield_now` 即释放，残余 UAF 竞态

`stop()` 顺序为：unhook → `LISTENER_PTR.store(0)` → `yield_now()` → 释放堆上 `ListenerState`。若另一线程上的回调已 `load` 到旧地址但尚未解引用（hook proc 的 load 与 deref 之间不是原子的），`yield_now` 只是降低概率而非同步保证，理论上仍可能解引用已释放内存。当前因 High-1（回调实际永不运行）不可达，一旦修复消息循环问题，此窗口即变为真实。**建议**：随 High-1 的专用线程方案一并解决（回调与 unhook 同线程后竞态天然消失），或改用 `Arc<ListenerState>` + `AtomicPtr` 并让回调侧只读取原子字段（`AtomicBool` 本身 `Copy` 化后甚至可放入静态）。

### Low-5 · `pim.rs:159` — `mail_search` 的 query 直接进入 `-like` 通配符模式

`if ($subject -like "*{escaped_query}*" ...)`：`ps_escape_dq` 防住了 PowerShell 命令注入，但 `-like` 的通配符（`*`、`?`、`[...]`）未做字面化处理。用户搜索 `foo[0]`、`50%*off` 之类的字符串时会被当作模式匹配，结果错误（功能性 bug，非安全问题）。**建议**：对 query 中的 `` ` ``、`*`、`?`、`[` 用反引号做 `-like` 转义，或改用 `.Contains()`。

### Low-6 · 文件规模 — `ax.rs` 1030 行、`media.rs` 512 行，超过 500 行准则

`ax.rs` 中纯函数角色映射表（`role_map`）、COM `imp` 模块、约 190 行测试混在一起；建议拆为 `ax/mod.rs`、`ax/role_map.rs`、`ax/imp.rs`（`cfg(windows)`）、测试随行。`media.rs` 略超，可把 dshow 解析 + 测试拆出。

## 安全项核对（确认无问题）

- **PowerShell 注入**：`pim.rs:11-13` `ps_escape_dq` 依次转义反引号、`$`、双引号，处理 `mail_search`/`mail_get` 全部插值点；`automation.rs:130` 与 `system.rs:161-162` 对单引号串用 `''` 转义，正确；`system.rs:158-159` 通知内容经 `CreateTextNode` 注入 XML，无 XML 注入。
- **参数注入**：`media.rs` ffmpeg 全部以参数数组传递（无 shell），设备名来自环境变量或 ffmpeg 枚举输出；`permission.rs:179` ConsentStore 查询的 capability 名是固定白名单字面量。
- **panic 面**：生产代码无 `unwrap()`/`expect()`（仅存于 `#[cfg(test)]`，clippy.toml 明确允许）；互斥锁中毒用 `PoisonError::into_inner` 恢复；`spawn_blocking` join 错误统一 flatten 为 `DesktopError`。
- **资源管理**：`sleep_inhibitor.rs` power request 句柄 RAII 释放且 `PowerSetRequest` 失败路径补 `CloseHandle`；`media.rs` camera_snap 临时文件读后删除；`escape_listener.rs` HHOOK 在 stop/Drop 中 unhook。
- **边界**：`ax.rs` 树遍历有 `MAX_NODES=4000` 与深度双上限；`system.rs:334-338` 的 32 位 tick 回绕重建逻辑正确。

## 架构红线合规快照

| 红线 | 状态 | 说明 |
|---|---|---|
| R1（core 不调平台 API） | ✅ | 本 crate 正是 R1 所指的平台实现层（`DesktopCapability` trait impl），Win32/COM/PowerShell 调用集中于此，core 侧经 trait/IPC 使用。 |
| R2（复杂 UI 在 Leptos/WASM） | ✅ 不涉及 | 无 UI 代码。 |
| R3（core 极简） | ✅ | 依赖克制：`windows` crate 仅 `cfg(windows)`；`base64`/`image` 复用 workspace 既有依赖（Cargo.toml 有注释说明）；`temp_path` 特意不引 `tempfile`。 |
| R4（接口层纯 I/O） | ✅ 不涉及 | 非接口层。 |
| R7（Rust Core 唯一大脑） | ✅ | 无业务决策；角色映射是为对接既有 core 侧 `INTERACTABLE_ROLES` 词汇表（ax.rs 模块文档明确说明）。 |
| R8（LLM 负责意图/路由） | ✅ | 正则表示仅用于解析 ffmpeg 机器输出（`parse_dshow_devices`）。 |
| R9（可配置项暴露为工具） | ✅ | `ALEPH_CAMERA_DEVICE`/`ALEPH_AUDIO_DEVICE` 环境变量为设备覆盖入口。 |
| R10（智能在 prompt） | ✅ 不涉及 | 无 prompt/middleware。 |

## 验证方式说明

纯静态阅读（任务禁止 cargo check/clippy/test）；对 High-1 额外追读了调用方 `src/builtin_tools/desktop/mod.rs:189-237` 以确认 `start()` 的运行线程上下文。未在 Windows 主机上实际运行，High-1 的结论依据 Win32 文档化的 `WH_KEYBOARD_LL` 回调派发模型。
