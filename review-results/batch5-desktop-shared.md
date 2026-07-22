# Batch 5 静态审查报告：desktop-shared | desktop/shared

> 审查范围：`desktop/shared/src/**/*.rs`（DesktopCapability trait 与 IPC 协议层）。
> 审查日期：2026-07-22。

## 统计

| 指标 | 数值 |
|------|------|
| 审查文件数 | 39 |
| 代码行数（含注释/空行） | ≈ 8,815 |
| Critical | 0 |
| High | 2 |
| Medium | 4 |
| Low | 4 |

## 历史问题验证

| 历史问题 | 状态 | 说明 |
|----------|------|------|
| `media_types.rs` 相机/音频 `duration` NaN panic | 已修复 | `CameraClipConfig::clamped()` 与 `AudioRecordConfig::clamped()` 在 `clamp` 前先判断 `is_finite()`，并替换为默认值 |
| `action/open_path.rs` 与 `action/app_launch.rs` Windows `cmd.exe` 注入 | 已修复 | Windows 路径均改为 `ShellExecuteW`/`NSWorkspace`，不再经过 `cmd.exe` 或字符串拼接 |
| `action/input.rs` drag duration 无上限 | 已修复 | `drag()` 中 `duration_ms` 被限制为 `10_000` ms，步数限制为 600 步 |
| `perception/screen_record.rs` 忽略 `region` 配置 | 已修复 | macOS 用 `setSourceRect`；Linux x11grab 用 `-video_size`/`+x,y`；Windows gdigrab 用 `-offset_x`/`-offset_y`/`-video_size` |

## 发现列表（按严重级排序，高置信度）

### High

1. **`action/app_launch.rs:152` — Linux `quit_app` 使用 `pkill -f <app_name>` 可误杀/恶意杀进程**
   - **问题**：`app_name` 来自 IPC/LLM 工具参数，直接被作为 `pkill -f` 的扩展正则匹配所有进程的完整命令行。`pkill -f ".*"` 可匹配并终止当前用户几乎所有进程，包括 Aleph 自身。Windows/macOS 路径都做了精确匹配（exe stem / bundle ID），Linux 路径没有。
   - **建议修法**：改为精确匹配，如 `pkill -x <exact_name>`，或先通过 `pgrep -x` 校验目标存在再发送信号；若必须支持子串，至少对输入做正则转义并限制只能命中一个目标进程。

2. **`action/input.rs:221` — `scroll()` 未限制 `amount`，Linux 下可致 worker 线程无响应**
   - **问题**：`amount` 是 `i32` 且未做上限校验。enigo 0.3 在 Linux（xdo/x11rb）实现中会把 scroll 转换为 `for _ in 0..|amount| { button.click() }`。传入 `i32::MAX` 会触发约 21 亿次点击事件，阻塞 `tokio::task::spawn_blocking` 的 worker 线程直到进程被强制终止。`drag()` 已在修复时明确按“不可信 IPC”做了上限，scroll 的边界条件被遗漏。
   - **建议修法**：按合理物理范围限制 `amount`（例如 `±10_000`），或像 drag 一样在边界处截断并返回错误。

### Medium

3. **`perception/screen_record.rs:318` — macOS `SCRecordingOutput` 路径硬编码 `scale = 2`，非 Retina 显示器映射错误**
   - **问题**：`let scale: u32 = 2;` 被写死，region 的像素Clamp 以 `display_pts × 2` 为基准。外接 1x 显示器上，物理像素等于 points，会导致 region 输出尺寸翻倍、且 region 越界判断错误（右侧/底部像素被错误视为在屏内）。
   - **建议修法**：通过 `NSScreen` / `CGDisplayCopyDisplayMode` 查询真实 backing scale，或至少用 `display_width_px == display_width_pts` 判断 1x 屏并自动降级为 `scale = 1`。

4. **`bridge/client.rs:454` — retry 写失败时 inflight slot 泄漏**
   - **问题**：`call_with_timeout` 在 write 失败后会用新的 `retry_id` 重新注册 oneshot（line 454）。如果 `proc.stdin.write_all` / `flush` 再次失败（line 462/468 的 `?`），函数提前返回，但 `retry_id` 对应的 slot 从未被 `cancel` 或 `fail`，会一直留在 `InflightTable` 中，直到 helper stdout EOF 触发 `fail_all`。
   - **建议修法**：在 retry 写失败的错误路径上调用 `self.inflight.cancel(retry_id).await` 后再返回错误。

5. **`perception/screenshot.rs:67` / `:184` — `take_screenshot` / `take_screenshot_display` 未填充 `scale_factor`**
   - **问题**：`Screenshot` 类型专门携带 `scale_factor` 字段（文档说明用于把像素映射回逻辑 points），但 `take_screenshot` 和 `take_screenshot_display` 始终写 `scale_factor: None`。`xcap::Monitor` 已提供 `scale_factor()`（`list_displays` 中使用了它）。在 Retina 等 HiDPI 屏上，下游若按 1.0 解析像素坐标会产生 2 倍偏移。
   - **建议修法**：从 `monitor.scale_factor()` 读取并写入 `Screenshot.scale_factor`。

6. **`perception/screen_record.rs:564` / `:681` — Linux/Windows 录屏 region 未钳制到显示边界**
   - **问题**：`build_x11grab_args` 与 `build_gdigrab_args` 直接把 `region.width/height` 传给 ffmpeg `-video_size`。macOS 路径已通过 `sck_region_rect` 做了 Clamp。Linux/Windows 收到极大 region（`u32::MAX`）时，ffmpeg 可能尝试分配巨大帧缓冲，造成内存压力或 OOM。
   - **建议修法**：在参数构建前按主显示器尺寸 Clamp region；可复用/导出与 macOS 类似的钳制逻辑。

### Low

7. **`bridge/client.rs:112` / `:424` — 重置 bridge state 时未主动终止旧 helper 进程，可能遗留孤儿进程**
   - **问题**：`BridgeProcess.child` 未设置 `.kill_on_drop(true)`。write-failure 路径把 `*guard = None`，`tokio::process::Child` 默认 drop 不杀子进程；旧 helper 只有在其自身读到 stdin EOF 或 getppid 变化时才会退出。同时 `BridgeProcess` 注释“held to keep the subprocess alive via Drop”容易误导为 drop 会终止进程。
   - **建议修法**：在 `spawn_process` 的 `Command` 上设置 `.kill_on_drop(true)`，或显式在重置 state 前 `child.kill()`，并修正注释。

8. **`bridge/codec.rs:16` / `bridge/client.rs:223` — 解码失败时原始桥接响应可能进入日志，存在 PII 泄露风险**
   - **问题**：`decode_line` 在出错信息中嵌入 `raw={line:?}`；reader loop 也以 `tracing::warn!` 打印 `raw={line:?}`。桥接响应可能包含 OCR 文本、窗口标题、PIM 数据等敏感内容，decode 失败时会被写入日志。
   - **建议修法**：只在最详细（如 `trace`）级别记录原始 payload，或截断/哈希敏感字段。

9. **`desktop/shared/src/lib.rs:18-21` — 文档声称“Real platform API calls never live here”与代码实际不符**
   - **问题**：`action/input.rs` 直接调用 `enigo`，`perception/screenshot.rs` 直接调用 `xcap`，`action/app_launch.rs` 直接调用 `NSWorkspace`，`action/window_ax.rs` 直接做 AX FFI。`desktop/shared` 并非纯 trait/protocol 层，文档描述已过时。
   - **建议修法**：更新文档，说明 `desktop/shared` 提供跨平台共享实现与 trait，平台 crate 负责平台特化与桥接装配。

10. **`error.rs:10` — `DesktopError::NotAvailable` 变体在本 crate 内未被构造**
   - **问题**：`NotAvailable(String)` 只被定义，未在 `desktop/shared` 任何代码路径中使用（其它变体均有使用）。若仅作为公共 API 供平台 crate 消费，建议加注释说明；否则可能是历史遗留。
   - **建议修法**：确认是否有平台实现依赖该变体；无则删除或标记保留原因。

## 架构红线合规快照

| 红线 | 合规状态 | 说明 |
|------|----------|------|
| R1 core 不调用平台 API | 合规 | 本平台 crate（`desktop/shared`）直接调用 enigo/xcap/Win32/objc2/AX 等，但 `alephcore` 通过 trait/DIP 依赖它，符合“core 不调用平台 API”的分层 |
| R2 复杂业务 UI 在 Leptos/WASM | 不涉及 | 本 crate 无 UI 逻辑 |
| R3 core 极简 | 基本合规 | 新增依赖均围绕屏幕捕获/输入自动化，无“重依赖”投机引入；录屏复用已有 `ffmpeg` |
| R4 接口层纯 I/O | 不涉及 | 本 crate 是 capability 实现层，非 CLI/TUI/Web |
| R7 One core, many shells | 合规 | `DesktopPlatform` trait + 聚合器模式支持多平台 shell |
| R8 LLM 负责意图/路由 | 不涉及 | 本层无意图解析 |
| R9 可配置项暴露为工具 | 轻微偏差 | Windows 录屏音频设备仅通过环境变量 `ALEPH_AUDIO_DEVICE` 配置，未作为 tool 参数暴露 |
| R10 智能在 prompt 中 | 不涉及 | 本层无 prompt 工程 |

## 质量备注

- 所有生产代码中的 `.unwrap()` / `.expect()` 均位于 `#[cfg(test)]` 或 doc example 中，未发现违规。
- 未发现 >500 行的超大文件外的明显 DRY 问题；`bridge/client.rs`、`perception/screen_record.rs`、`action/window.rs` 较大，但职责集中。
- 错误处理整体遵循 `thiserror` + `Result` 模式；`pkill -f` 与 scroll amount 是本层少数几个输入校验缺口。
