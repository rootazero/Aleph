# 静态审查报告：desktop-macos

## 审查范围

- **单元名**：desktop-macos
- **路径**：`desktop/macos/`
- **关注点**：macOS 平台实现（AppKit/Vision/CoreGraphics/AVFoundation/FFI），重点审查 unsafe/FFI 边界、内存管理、线程安全
- **统计**：
  - Rust 生产源码：13 文件 / 3,526 LOC（`desktop/macos/src/**/*.rs`）
  - Swift bridge 生产源码：32 文件 / 6,017 LOC（`desktop/macos/bridge/Sources/**/*.swift`）
  - 测试文件 7 个未计入
  - **合计：45 文件 / 9,543 LOC**

---

## 摘要

| 严重级 | 数量 |
|--------|------:|
| Critical | 0 |
| High     | 0 |
| Medium   | 4 |
| Low      | 2 |
| **合计** | **6** |

**历史问题复查结论**：
- `desktop/macos/src/lib.rs` 旧问题「media 错误扁平化为 `BridgeFailed`」已通过 `preserve_typed` 修复，当前不再报告。
- `desktop/shared/src/media_types.rs` 旧问题「NaN duration 在 macOS 触发 `Duration::from_secs_f64` panic」已通过 `clamped()` 中的 `is_finite()` 守卫修复，macOS 调用端已使用 `.clamped()`。

---

## 发现问题（按严重级排序）

### Medium

#### 1. 跨进程 Accessibility 返回值被强制解包，可导致 bridge 崩溃

- **文件:行号**：
  - `desktop/macos/bridge/Sources/AlephBridge/RPC/InputSession.swift:749-751`
  - `desktop/macos/bridge/Sources/AlephBridge/RPC/AxSession.swift:210`
  - `desktop/macos/bridge/Sources/AlephBridge/RPC/AxSession.swift:494-496`
- **严重级**：Medium
- **问题描述**：
  上述代码对 `AXUIElementCopyAttributeValue` 返回的跨进程属性值使用 `as! AXValue` / `as! AXUIElement` 强制解包。Accessibility 属性值来自目标进程，恶意或实现不规范的目标应用可能返回非预期类型（例如将 `kAXPositionAttribute` 设为非 `AXValue`）。强制解包会直接导致 `AlephBridge` 子进程崩溃。虽然 Rust 侧可以重启 bridge，但针对 AX 目标的反复崩溃会造成可用性拒绝服务。
- **建议修法**：
  将 `as!` 改为可选转换 `as? AXValue` / `as? AXUIElement`，并在转换失败时当作「无法读取该属性」处理（返回 `nil` 或抛出结构化 `RpcError`），不要信任跨进程 AX 返回值的类型。

#### 2. PIM 权限请求在串行队列上阻塞，等待系统 TCC 弹窗

- **文件:行号**：
  - `desktop/macos/bridge/Sources/AlephBridge/CalendarCommands.swift:13-40`
  - `desktop/macos/bridge/Sources/AlephBridge/RemindersCommands.swift:12-40`
  - `desktop/macos/bridge/Sources/AlephBridge/ContactsCommands.swift:12-30`
- **严重级**：Medium
- **问题描述**：
  `requireCalendarAccess()` / `requireRemindersAccess()` / `requireContactsAccess()` 在串行的 `pimQueue` 上通过 `DispatchSemaphore.wait()` 同步等待用户响应系统 TCC 弹窗。只要任一 PIM 请求触发了弹窗且用户未立即处理，整个 `pimQueue` 就会被阻塞，后续所有 PIM 调用（包括不相关的 notes、contacts、calendar、reminders、mail 列表等）都会挂起，直到弹窗被处理或超时。这是一个串行资源上的长时间阻塞。
- **建议修法**：
  将权限请求改为异步完成后再进入队列，或在队列外预先完成权限请求，避免在全局串行 PIM 队列上同步等待用户交互。

#### 3. Server 对每个 JSON-RPC 请求无限制地创建并发任务

- **文件:行号**：`desktop/macos/bridge/Sources/AlephBridge/RPC/Server.swift:28-36`
- **严重级**：Medium
- **问题描述**：
  `Server.run()` 每读取一行 JSON-RPC 请求就通过 `group.addTask` 启动一个并发 handler。没有并发上限，如果 Rust 侧在短时间内发送大量请求（例如批量 OCR、截图或 PIM 调用），可能导致 `AlephBridge` 进程线程/任务数激增、内存上涨或被系统资源限制。
- **建议修法**：
  为处理任务添加并发限制（例如使用 `withTaskGroup` + 信号量，或限制未完成任务数量），确保 helper 不会被请求洪流压垮。

#### 4. LineReader 对单条消息长度无上限，存在内存膨胀风险

- **文件:行号**：`desktop/macos/bridge/Sources/AlephBridge/RPC/Server.swift:87-121`
- **严重级**：Medium
- **问题描述**：
  `LineReader.nextLine()` 会一直累积输入缓冲直到遇到 `\n` 或 EOF。如果上游发送一条超长且无换行的消息（例如异常巨大的 base64 截图或错误数据），`buffer` 会无界增长，可能导致 helper OOM。
- **建议修法**：
  为单条消息设置最大长度限制（例如 50 MB），超过限制时丢弃该行并返回错误，避免无界内存增长。

### Low

#### 5. 录制失败时未清理已创建的临时媒体文件

- **文件:行号**：
  - `desktop/macos/bridge/Sources/AlephBridge/RPC/CameraSession.swift:117-157`
  - `desktop/macos/bridge/Sources/AlephBridge/RPC/AudioSession.swift:57-125`
- **严重级**：Low
- **问题描述**：
  `CameraSession.clip()` 和 `AudioSession.record()` 在开始前即在 `~/.aleph/data/_media/` 创建输出文件 URL。如果后续录制失败或超时退出，代码会调用 `session.stopRunning()` / `recorder.stop()`，但并不会删除已生成的部分文件。长期运行会在媒体目录中累积废弃的 `.mov`/`.m4a` 文件。
- **建议修法**：
  在错误退出路径中删除部分写入的输出文件（或将其移入临时文件并在成功后才重命名到最终路径）。

#### 6. IOPMAssertionRelease 返回值被忽略，失败时无提示

- **文件:行号**：`desktop/macos/src/sleep_inhibitor.rs:70`
- **严重级**：Low
- **问题描述**：
  `InhibitorGuard` 的清理闭包中调用 `IOPMAssertionRelease(id_copy)` 并忽略返回值。如果断言释放失败（例如 id 已失效），睡眠抑制会一直保持，但调用方和日志都不会感知到。
- **建议修法**：
  检查返回状态并在失败时记录 `tracing::warn!`（无法恢复，但至少暴露问题）。

---

## 架构红线合规快照

| 红线 | 合规状态 | 说明 |
|------|----------|------|
| R1 — core 不调用平台 API，平台实现经 IPC/trait | ✅ 合规 | Rust `desktop/macos` 是平台层，允许直接调用平台 API；camera/audio/speech/OCR/AX/screen/PIM/permission 均通过 `SwiftBridge` JSON-RPC 与 Swift helper 交互，或经 trait 由 `aleph_desktop` 提供抽象。 |
| R2 — 复杂业务 UI 在 Leptos/WASM | ✅ 不适用 | 原生 shell 未在本次审查范围内；本层只做能力代理。 |
| R3 — core 极简，非核心功能不引入重依赖 | ✅ 合规 | macOS crate 依赖均为平台绑定所必需（objc2 系列、core-foundation、AVFoundation 等通过 Swift bridge），未引入无关重依赖。 |
| R4 — 接口层纯 I/O | ✅ 不适用 | CLI/TUI/Web 接口不在本路径。 |
| R7 — Rust Core 是唯一大脑 | ✅ 合规 | macOS 层仅实现 `DesktopPlatform` trait，无模型/意图/路由逻辑。 |
| R8 — LLM 负责意图/路由，正则只用于机器格式 | ✅ 合规 | 未发现用正则解析自然语言；字符串匹配仅用于按键名、AX action 名等机器格式。 |
| R9 — 所有可配置项暴露为工具 | ✅ 未发现违规 | 本层无可配置业务逻辑。 |
| R10 — 智能在 prompt 中 | ✅ 未发现违规 | 无中间层语义判断。 |

---

## 代码质量观察（未达严重级）

- Rust 生产代码中未发现 `unwrap()`/`expect()`（`#[cfg(test)]` 除外），错误处理符合项目规范。
- Swift bridge 大量使用 `@unchecked Sendable` 与 actor/NSLock 保护可变状态，未发现明显的数据竞争。
- `MacOSPlatform::resolve_helper_path` 依赖 `ALEPH_BRIDGE_PATH` 环境变量且未校验路径合法性；环境由父进程控制，当前未构成独立漏洞，但属于信任边界，建议在文档中显式说明。
