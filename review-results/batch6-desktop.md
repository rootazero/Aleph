# Batch 6 静态审查候选清单：desktop 模块

> **基线**：worktree `feat/severed-wire-audit-batch6`，与 `main` 一致。
> **方法**：每种 seam 独立 grep-diff（定义/注册端 vs 消费/派发端）+ read-before-write 复核生产调用。
> **审查日期**：2026-08-04
> **审查者**：subagent (severed-wire-audit)

## 范围

- `desktop/shared/src/**`
- `desktop/linux/src/**`
- `desktop/macos/src/**` + `desktop/macos/bridge/Sources/AlephBridge/**`
- `desktop/windows/src/**`
- `desktop/shell/src/**`

## 新增 / DECIDE 候选

### 1. `MediaTool::is_capture_action` 漏 `record_audio_start/stop`（DECIDE）
- **producer**: `src/builtin_tools/media_tool.rs:45-47, 161-188`
- **consumer side**: `voice::handle_record_start/stop` (gateway/handlers/voice.rs:206,245) 直接走 `media.record_audio_start/stop()`，**没有经过 `MediaTool`/`ApprovalPolicy`**
- **form**: 3 (classifier vs handler 不一致)
- **triage**: **DECIDE** — push-to-talk 是否纳入 MediaCapture 审批门
- **rationale**: `batch5-desktop-shared.md` 11 号发现指出 `MediaTool` 审批闭合；本轮新增 push-to-talk 时未沿用同一通路。CONNECT 端要求将 push-to-talk 视为 capture（把 `record_audio_start`/`record_audio_stop` 加入 `is_capture_action`）；CUT 端认为 push-to-talk 应保留 0 阻塞。
- **fix sketch (CONNECT)**: `media_tool.rs:45-47` 扩成 `matches!(action, "camera_snap"|"camera_clip"|"record_audio"|"record_audio_start"|"record_audio_stop")` 并在 gated match 增加对应分支。

### 2. 协议预埋通知常量 `ax.mutation`/`perm.status_changed` 无 live caller（DECIDE）
- **producer**: `shared/protocol/src/desktop_bridge/methods/ax.rs:21` (`ax.mutation`)；`perm.rs:15` (`perm.status_changed`)
- **consumer side**: 0 个生产引用。Rust 桥 `Message::Notification(_)` 在 `desktop/shared/src/bridge/client.rs:237-239` 显式 `// Notifications handled by later stages — ignore for now`
- **form**: 4 (client ghost) + 6 (never-compiled far-end)
- **triage**: **DECIDE** — 是否删（cut）或保留预埋（keep）
- **fix sketch (CUT)**: `ax.rs:21` 删除 `NOTIFY_MUTATION`；`perm.rs:15` 删除 `NOTIFY_STATUS_CHANGED`；同步移除 `Message::Notification(_)` 在 client.rs 的 no-op 注释。

### 3. `notify.rs` wss 不走 `cert_trust::TrustStore`（DECIDE）
- **producer**: `desktop/shell/src/notify.rs:64-122` — `tokio_tungstenite::connect_async` 使用 `native-tls`
- **form**: 6 (design choice)
- **triage**: **DECIDE** — 产品决定：维持 CA 信任 vs 接入 TrustStore
- **fix sketch**: 改用 `connect-rustls` feature 并注入 TrustStore。

### 4. `LineReader` buffer 无上限（DECIDE，batch5 Medium 4 未修）
- **producer**: `desktop/macos/bridge/Sources/AlephBridge/RPC/Server.swift:25-37, 87-122`
- **form**: 6 (内存膨胀风险)
- **triage**: **DECIDE** — 50MB / 64MB / 96MB 阈值选择
- **fix sketch**: `nextLine` 累加超过 64MB 直接丢弃 buffer 并向 stdout 写错误响应。

### 5. macOS AX `as!` force-cast 多处（DECIDE，batch5 Medium 1-3 未修）
- **producer**: `desktop/macos/bridge/Sources/AlephBridge/RPC/InputSession.swift:675-748`；`RPC/AxSession.swift:478, 596`
- **triage**: **DECIDE** — 把 `as!` 改成 `as?` + guard

## KEEP（已确认正确或已修复）

- linux 模块: 0 新发现
- windows UIA `clamp_max_nodes` 已 CONNECT（PR 0847f1c5）
- PermissionKind 14 种 + 各平台 `check_all` 实现完整
- `update::control_action` 接受 target.origin guard 已 CONNECT（2f0f0cc6d）
- Tauri 命令注册 compile-time 验证
- `ax_secure.rs` 的 `is_password_like` 2FA/OTP 扩展已 CONNECT

## 与 batch5 重叠的状态

| Batch 5 finding | Batch 6 状态 |
|---|---|
| `batch5-desktop-shared.md` Medium 4 — screenshot 缺 scale_factor | 已修 |
| `batch5-desktop-shared.md` Medium 5 — retry 写失败 inflight 泄漏 | 已修 |
| `batch5-desktop-shared.md` Medium 6 — take_screenshot 始终 scale_factor: None | 已修 |
| `batch5-desktop-shared.md` Low 8 — decode_line 泄露 OCR raw | 已修 |
| `batch5-desktop-macos.md` Medium 1-3 — AX as! force-cast | 未修（见候选 #5） |
| `batch5-desktop-macos.md` Medium 4 — LineReader 长度无上限 | 未修（见候选 #4） |
| `batch5-desktop-shell.md` Medium 1 — update applying 锁 | 已修 |
| `batch5-desktop-shell.md` Medium 2 — update 控制流 origin guard | 已修 |
| `batch5-desktop-shell.md` Medium 3 — wss 不走 TrustStore | 未修（见候选 #3） |
| `batch5-desktop-shell.md` Low 4 — deeplink 路径脱敏 | 已修 |
| `batch5-desktop-windows.md` Medium 2 — CoInitializeEx | 已修 |
| `batch5-desktop-windows.md` High 1 — escape listener 钩子无消息循环 | 已修 |
| `batch5-desktop-linux.md` M1-M3 — clipboard / settings / pkill | 已修 |

## 总结

desktop 模块本轮**未发现新的"两端都在但连线断"的事故级 severed wire**。现存 DECIDE 候选均为 wire 接通但需产品判断的边界。KEEP 的 batch5 修复均已重检。

## 未做

1. 没有运行 cargo build / cargo test / cargo clippy（worktree read-only 限制）
2. 没有触碰 e2e 测试目录
3. 没有动 desktop 之外模块
4. 没有把 batch5 已修项重新建议修改
5. 没有把 `MediaTool::is_capture_action` 在没有产品决定前硬改（提作 DECIDE）
6. 没有把协议预埋的通知常量在产品决定前删除（提作 DECIDE）