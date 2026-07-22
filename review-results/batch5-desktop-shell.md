# Batch 5 静态审查报告 — desktop-shell（desktop/shell）

## 审查元信息

- **单元名**：desktop-shell
- **路径**：`desktop/shell`
- **审查方式**：无 diff 全量静态审查（基于 `/tmp/aleph-review-batch-5` worktree）
- **审查范围**：`desktop/shell/src/**/*.rs`、`desktop/shell/build.rs`、`desktop/shell/Cargo.toml`、`desktop/shell/capabilities/default.json`
- **代码统计**：
  - Rust 源文件：22 个
  - 代码行数（含注释、空行）：约 6,013 LOC
  - 最大单文件：`src/main.rs`（1,347 行）

## 历史问题验证

| 历史问题 | 现状 | 结论 |
|---|---|---|
| `cert_trust/pending.rs` host-only 校验导致证书审批竞态 | `approve_cert` 在 `host` 相等基础上额外校验 `fingerprint` 严格匹配（`desktop/shell/src/cert_trust/pending.rs:84`），发现记录变化时拒绝并保留原记录。 | **已修复** |
| `webview_perms.rs` 权限授予过宽 | Linux 端 gate：audio-only + `is_internal` origin（`desktop/shell/src/webview_perms.rs:64-70`）；Windows 端 gate：仅 mic + origin 校验（`desktop/shell/src/webview_perms.rs:117-132`）。 | **已修复** |
| `deeplink.rs` 日志泄漏 token | `handle_url` 在 info 级别使用 `redacted_for_log()` 剥离 query 与 fragment（`desktop/shell/src/deeplink.rs:38-43`），完整 URL 仅在 DEBUG 输出。 | **已修复** |
| `notify.rs` ws:// 明文 + 跳过证书 pin | 远程目标非 https/wss 时不再发送 Gateway token（`desktop/shell/src/notify.rs:175-180`）；但 wss 连接使用默认 `tokio-tungstenite` TLS（webpki-roots），**不参与 `cert_trust` TOFU pin**。对自签名远程 Gateway，通知桥永远连不上；对 CA 签发的远程 Gateway 则无 pin。 | **部分修复**（见新发现 #3） |
| `update.rs` 无并发锁 | 新增 `applying: Mutex<bool>` 并发锁（`desktop/shell/src/update.rs:200`），避免重复下载/安装。 | **已修复**（但见新发现 #1） |
| `external_link.rs` allow-list 只比 hostname | 现使用 `url.origin().unicode_serialization()` 完整 origin（scheme+host+port）匹配（`desktop/shell/src/external_link.rs:46,91-94`），并附有 scheme/port 绕过回归测试。 | **已修复** |
| `perm_monitor.rs` 进程名不匹配 | 优先查找 bundled `AlephBridge`，再 fallback `aleph-bridge`（`desktop/shell/src/perm_monitor.rs:127-148`）。 | **已修复** |

## 发现列表（按严重级排序）

### Critical（0 条）

无。

### High（0 条）

无。

### Medium（3 条）

1. **`update.rs:269-344` — 更新应用失败一次后 `applying` 锁终身未释放，用户无法重试**
   - `apply_staged_update` 在 `applying` 上锁后将其设为 `true`，但所有早期返回分支（updater 不可用、下载失败、安装失败、package-manager 提示）均**未将 `applying` 重置为 `false`**。一旦某次更新应用失败，当前会话内再次点击「Restart to update」会被永久忽略，直到重启应用。
   - 建议：在 `apply_staged_update` 的 async block 入口使用 `defer`/guard pattern，或在每个返回点重置 `applying`；也可将锁生命周期限定到真正进入安装流程的时段。

2. **`main.rs:386-394` + `update.rs:52-58` — 更新控制 sentinel path 任意来源可触发，可强制应用重启**
   - `on_navigation` 先匹配 `update::control_action(url)`，再调用 `external_link::route(url)`。`control_action` 仅比较 URL path（`/__aleph-shell/update/apply`、`/__aleph-shell/update/dismiss`），**不校验 origin**。
   - 这意味着 Panel 中渲染的任意链接（包括 LLM 生成的 markdown 链接、聊天消息中的 `http://evil.com/__aleph-shell/update/apply`）被点击后，会直接进入下载、安装、重启流程。`Dismiss` 为低风险，`Apply` 可导致用户在未明确选择时被迫重启应用。
   - 建议：`control_action` 或 `on_navigation` 中增加 `is_internal(url)` 校验，仅对 Panel 内部来源或 sentinel origin 响应控制链接。

3. **`notify.rs:135-149` + `cert_trust` — 桌面通知桥 wss 连接不参与 TOFU 证书 pin，自签名远程 Gateway 下通知功能失效**
   - `ws_url()` 将 https 远程目标映射为 `wss://`（`notify.rs:139`），`tokio_tungstenite::connect_async` 使用系统/webpki 根证书验证（未配置自定义 connector），**从不读取 `cert_trust::TrustStore`**。
   - 影响：
     - 对 `cert_trust` 设计的自签名远程 Gateway，通知桥 TLS 握手直接失败，无法完成 `connect`/`subscribe`，桌面通知持续不可用。
     - 对 CA 签发证书，token 仅依赖系统根证书，未被 `cert_trust` TOFU 机制加固。
   - 建议：为 `tokio-tungstenite` 配置与 webview 共享的 `TrustStore`-aware TLS connector；若暂不可行，应在文档/代码注释中明确说明桌面通知桥不支持自签名远程 Gateway，并提示用户。

### Low（5 条）

4. **`deeplink.rs:38` — 日志脱敏仅剥离 query/fragment，path 仍可能携带敏感 token**
   - `redacted_for_log()` 返回完整 path（`desktop/shell/src/deeplink.rs:57-74`）。若 deep link 采用 `aleph://oauth/<token>` 这类 path 形式，info 日志仍会记录 token。
   - 建议：仅保留 scheme 与 authority（或只保留 `aleph://...` 形式），路径级 token 同样脱敏。

5. **`notify.rs:101-104` — 解析失败的 JSON-RPC 帧在 warn 级别输出完整 `text`**
   - `tracing::warn!(... raw={text:?})` 可能把 Gateway 事件内容（含 approval 标题/正文、运行结果摘要等）写入 warn 日志。
   - 建议：在 warn 日志中只输出前 N 个字符或结构摘要；完整帧仅在 debug/trace 输出。

6. **`perm_monitor.rs:73-108` — 代码实现与注释不符，且持续高频 spawn 外部进程**
   - 注释称「while the app is in foreground」轮询，但代码无条件每 3 秒执行一次（`POLL_INTERVAL`），每次循环为两种权限各 spawn 一次 `aleph-bridge` 子进程，贯穿应用生命周期。
   - 建议：增加前台/后台状态判断，或降低轮询频率/改为事件驱动；并修正注释。

7. **`main.rs:735-744`、`main.rs:1032-1046` — daemon/connection 错误信息使用手写 JS 转义，不一致且易折断**
   - `show_daemon_error`/`show_connection_page` 只转义 `\` 与 `'`（`main.rs:742,1042`）。若 daemon 启动失败 stderr 包含换行符（常见），`eval(...)` 会注入未转义的换行，导致 JS 语法错误；与 `deeplink.rs` 使用 `serde_json::to_string` 的写法不一致。
   - 建议：统一使用 `serde_json::to_string` 对错误消息编码后再嵌入 JS。

8. **`main.rs` 整体超过 500 行（1,347 行）**
   - 文件同时包含窗口构建、supervisor 状态机、daemon/remote 生命周期、路由/IPC helper 等，已超出「可快速理解」的阈值。
   - 建议：将 supervisor 状态机与 action 处理拆入独立模块（如 `supervisor.rs`），路由/helper 拆入 `routing.rs`。

### 补充说明（非独立报告项）

- `daemon.rs:336-352` 在找不到 bundled `aleph-server` 时会 fallback 到 `PATH` 搜索并执行任意同名二进制。这是 dev 便利行为，但在生产环境下若 bundle 损坏或遭 PATH 污染，可能拉起非预期 daemon。建议文档化或在 release 模式下禁用 PATH fallback。
- `webview_perms.rs` Linux gate 中 `audio_only && origin_ok` 仅在该 permission handler 被触发时生效；WebKitGTK 的 `enable-media-stream` 被全局打开（`desktop/shell/src/webview_perms.rs:55`），无其他媒体类型 handler，攻击面可控。

## 架构红线合规快照

| 红线 | 结论 | 说明 |
|---|---|---|
| R1 core 不调用平台 API | N/A / 合规 | 本 crate 即为平台 shell，调用 Tauri/WebView/OS API 是其职责。 |
| R2 复杂业务 UI 在 Leptos/WASM | 合规 | shell 仅做窗口/托盘/热键/更新横幅容器；业务 UI 在 Panel。 |
| R3 core 极简，非核心功能不得引入重依赖 | N/A | shell 不在 core。 |
| R4 接口层为纯 I/O，无业务逻辑 | 基本合规 | 唯一 `invoke_handler` 仅暴露连接目标配置命令（`connection::*`、`connect_setup::*`、`cert_trust::pending::*`），符合 spec §5.2 例外。 |
| R7 Rust Core 是唯一大脑 | 合规 | 通知策略、approval 内容、R5 中断逻辑均已下沉到 Gateway/core（`surface.notify`、`surface.approval`）。 |
| R8 LLM 负责意图/路由，正则仅用于机器格式 | 合规 | shell 无正则意图解析。 |
| R9 所有可配置项暴露为工具 | 基本合规 | 热键仍依赖 `ALEPH_SHELL_HOTKEY` 环境变量，非工具/配置 UI 暴露，与 R9 精神略有偏差。 |
| R10 智能在 prompt 中 | 合规 | shell 无中间件智能决策。 |

## 统计

- 审查文件数：22
- 约 LOC：6,013
- Critical：0
- High：0
- Medium：3
- Low：5
