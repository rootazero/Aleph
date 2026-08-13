# Batch 7 静态审查报告 — desktop（severed-wire-audit 后续轮）

> **基线**：worktree `review/desktop` 分支，基于 `main` commit `0bca9264a`。
> **方法**：在 batch5（2026-07-22）+ batch6（2026-08-04）两轮 desktop 审计之后逐文件
> 复核 desktop 工作区，按风险模式扫面（panic-in-prod / unwrap-outside-test /
> mutex-across-await / 无界资源 / 输入校验 / 错误处理 / 资源泄漏）。
> **审查日期**：2026-08-13
> **审查者**：severed-wire-audit（sequential, no subagent）

## 范围

- `desktop/shared/src/**`（30+ 文件，~14.8K 行）
- `desktop/linux/src/**` + `desktop/linux/src/ax/**`（18 文件，~5.6K 行）
- `desktop/macos/src/**`（11 文件，~1.7K 行）
- `desktop/windows/src/**`（8 文件，~2K 行）
- `desktop/shell/src/**` + `desktop/shell/src/cert_trust/**`（21 文件，~6.2K 行）
- `desktop/macos/bridge/Sources/AlephBridge/**`（19 Swift 文件，~4.6K 行）

## 总览

| 指标 | 数值 |
|------|-----:|
| Critical | 0 |
| High | 0 |
| Medium | 0 |
| Low | 2 |
| 总计 actionable | **2** |
| DECIDE（保留 | 批次内记录） | 4 |

## 历史问题验证

| 历史问题 | 状态 | 说明 |
|----------|------|------|
| batch5 `pkill -f` 误杀（linux/app.rs） | 已修 | 走 `proc::matches_name` 精确匹配进程可执行文件名 |
| batch5 `scroll amount=i32::MAX` 阻塞 | 已修 | `MAX_SCROLL_CLICKS = 10_000` 上限 |
| batch5 macOS SCRecordingOutput 假成功 | 已修 | `verify_recording_output` 校验文件大小 |
| batch5 screenshot `scale_factor=None` | 已修 | 走 `xcap::Monitor::scale_factor()` |
| batch5 bridge retry 写失败 inflight 泄漏 | 已修 | `inflight.cancel(retry_id)` 释放 slot |
| batch5 Windows coordinate 同时两种解释 | 已修 | 拆分输入输出路径，统一语义 |
| batch5 screen_record region `u32::MAX` | 已修 | `clamp_region_to_display` 钳制到显示器 |
| batch5 screen_record 录屏超时无限 | 已修 | `FFMPEG_RECORD_OVERHEAD` / `RECORDER_FINALISE_GRACE` / `RECORD_STARTUP_MARGIN` |
| batch5 record region 未钳制 | 已修 | 同上 |
| batch5 bridge `kill_on_drop(true)` 漏配 | 已修 | `cmd.kill_on_drop(true)` + `Command::new(...).kill_on_drop(true)` |
| batch5 Wayland `ydotool` 缺失 silent no-op | 已修 | `pick_rail` 返回 `NotAvailable` 并提示安装 |
| batch5 update applying 锁 | 已修 | `Updater::try_begin_apply / end_apply` 显式闩 |
| batch5 update 控制流 origin guard | 已修 | `control_action` 验证 `target.serves_origin(url)` |
| batch5 update wss 不走 TrustStore | 未修 | **DECIDE #1**（保留：需产品决策是否接 TrustStore） |
| batch5 media `Duration::from_secs_f64` NaN panic | 已修 | `clamped()` 中先 `is_finite()` 检查 |
| batch5 cmd.exe 注入（open_path/app_launch） | 已修 | Windows 路径改走 `ShellExecuteW`，无 `cmd.exe` |
| batch5 input drag duration 无上限 | 已修 | `drag_path` 中限制 10_000 ms / 600 步 |
| batch6 macOS AX `as!` force-cast 多处 | **本轮修复** | 见 finding L-1 |
| batch6 LineReader buffer 无上限 | **本轮修复** | 见 finding L-2 |
| batch6 `MediaTool::is_capture_action` 漏 push-to-talk | DECIDE #2 | 保留：需产品决策 |
| batch6 `ax.mutation` / `perm.status_changed` 无 live caller | DECIDE #3 | 保留：协议预埋，未消费 |

## 新发现（按严重级）

### Low

#### L-1. macOS AX `as!` force-cast 可能在 AX 返回异常类型时崩溃 helper 进程
- **producer**:
  - `desktop/macos/bridge/Sources/AlephBridge/RPC/AxSession.swift:280` (`withTimeout(el as! AXUIElement)`)
  - `desktop/macos/bridge/Sources/AlephBridge/RPC/AxSession.swift:600-602`（`boundsOf` 的 `pv as! AXValue` / `sv as! AXValue`）
  - `desktop/macos/bridge/Sources/AlephBridge/RPC/InputSession.swift:862-864`（`boundsOf` 同模式）
- **风险**：Apple 文档声明 `kAXPositionAttribute` / `kAXSizeAttribute` / `kAXFocusedUIElementAttribute` 分别返回 `AXValue` / `AXValue` / `AXUIElement`，但一个行为异常的 element 可以违反合约返回一个不同的 `CFType`。`as!` 在运行时崩溃 helper 子进程；bridge client 读 helper 崩溃等同于「拒绝执行」——这正是 AX 阶梯被设计出来诊断的 silent rung-failure 形态。
- **fix**: 全部改为 `as?` + guard + bail to `nil`，调用方看到「bounds 缺失 / element 无效」并尝试上一级 rung。
- **status**: 已修（commit `36181eb4b`）

#### L-2. macOS Swift bridge `LineReader` buffer 无上限，单个恶意长行可钉住 helper 数百 MB
- **producer**: `desktop/macos/bridge/Sources/AlephBridge/RPC/Server.swift:71-99`（`LineReader.nextLine` 累积 `buffer` 直到 `\n`）
- **风险**：helper 是长驻进程，单个不带 `\n` 的恶意 JSON-RPC 行就足以让 `Data` 累积到数百 MB。
- **fix**: 在 `nextLine` 累加时检查 `buffer.count > 64 MB`，超限则丢弃，调用方在下次迭代看到 EOF 自动拆 session。64 MB 远超最大合法 params（base64 截图 1.5 MB 上限），远低于 Data 本身成为内存压力的阈值。
- **status**: 已修（commit `36181eb4b`）

### DECIDE（保留 — 不在本轮自动处理）

#### DECIDE #1: notify.rs 的 wss 不走 TrustStore
- 现状：`tokio_tungstenite::connect_async` 用 `native-tls` 平台根；自签名远程 Gateway 上 bridge TLS 握手失败而 Panel 可以连接（`cert_trust/` 只钩 webwebview 的 certificate challenge）。
- 处理：保留现状，等产品决定是否要 bridge 也走 TrustStore pin。

#### DECIDE #2: MediaTool::is_capture_action 是否纳入 push-to-talk
- 现状：`media_tool.rs:45-47` 的 `is_capture_action` 不含 `record_audio_start`/`record_audio_stop`；`voice::handle_record_start/stop` 直接调 `media.record_audio_*()`，不经 `MediaTool`/`ApprovalPolicy`。
- 处理：CONNECT 端 vs CUT 端对 push-to-talk 是否视为 capture 有分歧；保留为产品决策。

#### DECIDE #3: 协议预埋通知常量 `ax.mutation`/`perm.status_changed` 无 live caller
- 现状：`shared/protocol/src/desktop_bridge/methods/{ax,perm}.rs` 定义 `NOTIFY_MUTATION`、`NOTIFY_STATUS_CHANGED`；Rust bridge `client.rs:277-279` 对 `Message::Notification(_)` 显式 no-op；全代码库无 producer 发送这两类通知。
- 处理：保留为协议预埋；删/用由产品决策。

#### DECIDE #4: macOS `SCRecordingOutput` 路径硬编码 `scale = 2`
- 现状：`perception/screen_record.rs:397` 写死 `let scale: u32 = 2;`，1x 外接显示器上 region 越界判断与输出像素映射双重错位。
- 处理：本轮 batch5/batch6 均标记为 DECIDE（需要 NSScreen/CGDisplay 接入），未修复。本轮复核确认状态未变，仍为 DECIDE。

## 与 batch5 重叠的状态

| Batch 5 finding | Batch 7 状态 |
|---|---|
| Linux pkill -f 误杀 | 已修（`linux/app.rs::quit` → `proc::matches_name`） |
| scroll amount 上限 | 已修（`MAX_SCROLL_CLICKS = 10_000`） |
| macOS SCRecordingOutput 假成功 | 已修（`verify_recording_output`） |
| screenshot `scale_factor=None` | 已修（`take_screenshot` / `take_screenshot_display` 写入 `coordinate_scale`） |
| bridge retry 写失败 inflight 泄漏 | 已修（`inflight.cancel(retry_id)`） |
| 各种 region / 录屏超时 / clamp | 全部已修 |
| macOS AX `as!` 多处 | **本轮修复**（`as?` + guard） |
| LineReader buffer 无上限 | **本轮修复**（64 MB 上限） |
| MediaTool::is_capture_action | 保留为 DECIDE |
| 协议预埋通知常量 | 保留为 DECIDE |
| notify.rs wss TrustStore | 保留为 DECIDE |

## 总结

desktop 工作区在 batch5（Jul）+ batch6（Aug 初）两轮审计之后已经处于非常
健康的状态：所有已知的 severity ≥ Medium 的问题都已修复，所有 production
路径的 `unwrap()` / `expect()` / `panic!` 都已清理，所有 `tokio::sync::Mutex`
均未跨 `.await`，所有从 IPC/LLM 输入的数据路径都做了 explicit validation，
所有 shell-out 都有 `output_capped_blocking` / `output_capped` 限时。

本轮（batch7）发现并修复 2 处 Low：
- macOS Swift bridge 中 4 处 `as!` force-cast 改为 `as?` + guard（崩溃 → 优雅返回 nil）
- macOS Swift bridge `LineReader` buffer 上限（潜在内存炸弹）

剩下的 DECIDE 项目（4 处）都是产品决策或平台特定调研工作，不在本轮 auto-fix 范围。

## 未做

1. **没有跑 cargo build / cargo test / cargo clippy**（worktree 内不编译，按协议最后统一一次）
2. **没有动 `desktop/shared`/`desktop/linux`/`desktop/macos`/`desktop/windows`/`desktop/shell` 中除已修改文件外的文件**
3. **没有触碰 e2e 测试目录**
4. **没有触碰协议层**（`shared/protocol`）—— DECIDE #3 的协议预埋通知需要单独审
5. **没有触碰 builtin_tools**（MediaTool::is_capture_action 属其中）—— 已在 batch6 DECIDE 列出，本轮未重审
6. **没有触碰 Swift 测试**（desktop/macos/bridge/Tests/AlephBridgeTests）—— 测试已有，但没有 `as!`/buffer cap 的回归 pin
7. **没有处理 macOS `SCDisplay.width` 返回值的歧义**（DECIDE #4）—— 需在真机上对照 Apple 文档核实后才能改
8. **没有改 `MediaTool::is_capture_action`**（DECIDE #2）—— 需产品决定 push-to-talk 是否视为 capture