# webchat-core 静态审查报告

## 审查单元

- **单元名**: `webchat-core`
- **路径**: `interfaces/webchat/src/api/` + `interfaces/webchat/src/state/` + `interfaces/webchat/src/canvas_engine/` + `interfaces/webchat/src/*.rs`（根目录文件）
- **关注点**: Leptos/WASM web 聊天核心：API 层、状态管理、canvas 引擎

## 统计

| 区域            | 文件数 | LOC  |
|-----------------|--------|------|
| 根目录 `*.rs`   | 12     | 4051 |
| `api/`          | 36     | 5934 |
| `state/`        | 11     | 2493 |
| `canvas_engine/`| 6      | 706  |
| **合计**        | **65** | **13184** |

## 发现列表（按严重级排序）

### Medium

#### M1 — `panic_overlay.rs:93` 崩溃日志可能把 URL 中的 Gateway token 持久化到 localStorage

- **文件**: `interfaces/webchat/src/panic_overlay.rs:93`（`current_url()`）
- **严重级**: Medium
- **问题描述**: 恐慌恢复钩子会把当前页面 URL 写入 `aleph.panel.crashes`。如果页面在 WebSocket 握手完成前发生 panic，URL 中的 `?token=` 或 `?bt=` 尚未被 `scrub_credentials_from_url()` 清除，token 就会被持久化到 localStorage 崩溃环状缓冲区。`clear_credentials()` 不会清理该缓冲区，导致凭据残留在另一个 localStorage key 中，XSS 场景下可被读取。
- **建议修法**: 在 `current_url()` 或 `persist_crash()` 中复用 `context::strip_params` 先移除 `token=`、`bt=` 等敏感查询参数，再写入崩溃日志。

### Low

#### L1 — 根目录/多个 API 文件超过 500 行

- **文件**: `interfaces/webchat/src/context.rs`（1448 行）、`interfaces/webchat/src/appearance.rs`（838 行）、`interfaces/webchat/src/api/teams.rs`（621 行）、`interfaces/webchat/src/api/memory_config.rs`（614 行）、`interfaces/webchat/src/state/sessions.rs`（579 行）
- **严重级**: Low
- **问题描述**: 这些文件均超过 500 LOC。`context.rs` 同时承担 WebSocket 连接、认证握手、RPC 分发、事件订阅和 alert/approval 订阅，职责较重，长期维护成本高。
- **建议修法**: 后续迭代考虑将 `context.rs` 拆分为 `connection.rs`/`handshake.rs`/`events.rs`；`appearance.rs` 的六个 axis 可拆成独立子模块；`teams.rs` 的 task API 与 team API 可拆分。

#### L2 — `state/sessions.rs:69` 生产代码使用 `.expect()`

- **文件**: `interfaces/webchat/src/state/sessions.rs:69`
- **严重级**: Low
- **问题描述**: `SessionMap::new()` 使用 `.expect("SessionMap::new must run under a reactive owner")`。项目风格要求生产代码禁止 `unwrap()`/`expect()`（测试除外）。虽然这是一个明确的编程约定错误，但若组件挂载顺序异常会导致用户面板直接 panic。
- **建议修法**: 改为返回 `Option<Self>` 或 `Result<Self, String>`，由调用方优雅降级（例如弹提示或 fallback 到一个默认 owner）。

#### L3 — `context.rs:571`/`585` 事件派发时持有 Mutex 锁调用回调，存在同线程重入死锁风险

- **文件**: `interfaces/webchat/src/context.rs:571`、`interfaces/webchat/src/context.rs:585`
- **严重级**: Low
- **问题描述**: `subscribe_events`/`unsubscribe_events` 与 `dispatch_event` 共用同一把 `std::sync::Mutex`。`dispatch_event` 在持有锁的同时调用所有 handler；若某个 handler 内部调用 `subscribe_events`/`unsubscribe_events`（Wasm 单线程），会导致同线程重入死锁。
- **建议修法**: 派发前先把 handler 列表 `clone()` 到局部变量，释放锁后再遍历调用。

#### L4 — `canvas_engine/markdown_excerpt.rs:141` 协议相对 URL 可穿过链接白名单

- **文件**: `interfaces/webchat/src/canvas_engine/markdown_excerpt.rs:141`
- **严重级**: Low
- **问题描述**: `sanitize_link_url` 只检查 `scheme:` 形式，像 `//evil.com` 这类协议相对 URL 不含冒号，会原样进入 `href`，并带有 `target="_blank" rel="noopener"`。没有 XSS（属性值已转义），但会允许导航到外部域。
- **建议修法**: 对无 scheme 但开头为 `//` 的 URL 拒绝，或统一要求 `http/https/mailto` 并以 `:` 开头。

#### L5 — `state/memory.rs` `MemoryState::new()` 在构造时直接创建持久化 Effect，非 wasm 环境可能行为不确定

- **文件**: `interfaces/webchat/src/state/memory.rs:65-80`
- **严重级**: Low
- **问题描述**: `MemoryState::new()` 无条件调用 `web_sys::window()` 并创建 `Effect` 写 localStorage。当前面板目标为 wasm32，但如果被误在非浏览器 host 测试或 SSR 初始化，`web_sys::window()` 的语义不稳定，且 `Effect` 未绑定到明确 owner。
- **建议修法**: 将 Effect 创建移到 `AppContent` 中显式 owner 下，或添加 `#[cfg(target_arch = "wasm32")]` 保护（其余 fallback 为只读默认值）。

## 架构红线合规快照

| 红线 | 状态 | 说明 |
|------|------|------|
| R1 core 不调用平台 API | ✅ | webchat 为 Leptos/WASM 前端，平台 API 仅通过 `web_sys` 在接口层使用，Core RPC 不接触平台 API。 |
| R2 复杂业务 UI 在 Leptos/WASM | ✅ | 复杂设置页、聊天、canvas、团队看板等均在 Leptos 组件中实现。 |
| R3 core 极简，非核心不重依赖 | ✅ | API 层为薄 JSON-RPC 包装，未引入新的重依赖。 |
| R4 接口层纯 I/O | ✅ | `api/` 模块只做 RPC 调用、序列化和少量 wire-shape 转换（如 heartbeat 触发条件字符串映射），无业务决策。 |
| R5 Menu bar first | N/A | 本单元为 web 面板，未涉及原生 shell。 |
| R6 AI 主动到达 | ✅ | 通过 `alerts.**`、`approval.**`、`stream.*` 事件订阅驱动通知与卡片。 |
| R7 Rust Core 是唯一大脑 | ✅ | 所有配置/运行/记忆/权限状态均来自 Core RPC。 |
| R8 LLM 负责意图/路由，正则只用于机器格式 | ✅ | 路由规则在 Core 侧维护；面板侧未用正则处理自然语言。 |
| R9 所有可配置项暴露为工具 | ✅ | 设置页均通过 RPC 读写 Core 配置。 |
| R10 智能在 prompt 中 | ✅ | 面板仅做渲染，几乎无中间件逻辑。 |

## 备注

- rust-doctor 辅助 JSON（`/tmp/rd-interfaces.json` 等）为空，未采用其线索。
- 本次审查未运行 cargo check/clippy，未修改源代码，未执行 git 操作。
- 未发现 Critical/High 级问题。
