# Batch 6 静态审查候选清单：shared 模块

> **基线**：worktree `feat/severed-wire-audit-batch6`，与 `main` 一致。
> **方法**：每种 seam 独立 grep-diff + read-before-write 复核。
> **审查日期**：2026-08-04

## 范围

- `shared/client/src/**`
- `shared/config/**` （docs-only 目录）
- `shared/logging/src/**`
- `shared/protocol/src/**`
- `shared/ui_logic/src/**`

## 统计

| 子模块 | candidates | CUT | CONNECT | DECIDE |
|--------|-----------:|----:|--------:|-------:|
| shared/client | 3 | 2 | 0 | 1 |
| shared/config | 0 | 0 | 0 | 0 |
| shared/logging | 1 | 1 | 0 | 0 |
| shared/protocol | 17 | 13 | 1 | 3 |
| shared/ui_logic | 8 | 8 | 0 | 0 |
| **合计** | **29** | **24** | **1** | **4** |

## CUT 候选

### shared/client
1. **`CliError::WebSocket` 变体从未被构造** — `shared/client/src/error.rs:12,46`
   - `From<tungstenite::Error>` impl 存在但调用栈总先转成 `Connection`
   - fix: 删 variant + From impl

2. **`CliError::Other` anyhow 死依赖** — `shared/client/src/error.rs:39-43` + `Cargo.toml:19`
   - fix: 删 `anyhow` 依赖 + From impl

### shared/logging
3. **`PiiScrubbingLayer` 公开 no-op** — `shared/logging/src/pii_filter.rs:22-35`
   - 公开但行为 passthrough + 一次性 warn
   - fix: 加 `#[deprecated]` 或删除

### shared/protocol
4. **`aleph_protocol::ConfigChangedEvent` 死代码** — `shared/protocol/src/events.rs:716-724`
5. **`aleph_protocol::UncertaintyAction` 死代码** — `shared/protocol/src/events.rs:200-212`
6. **`aleph_protocol::ToolSummaryItem` / `ToolErrorItem`** — `shared/protocol/src/events.rs:728-742`
7. **`ToolResult::with_metadata()` builder** — `shared/protocol/src/events.rs:622-626`
8. **`discovery::DiscoveredInstance` 整个模块** — `shared/protocol/src/discovery.rs`
9. **`invitation::{Invitation, ...}` 整个模块** — `shared/protocol/src/invitation.rs`
10. **JSON-RPC 错误码常量 4 个未消费** — `shared/protocol/src/jsonrpc.rs:28,36,38,40`
    - `SESSION_NOT_FOUND`, `PROVIDER_ERROR`, `MEMORY_ERROR`, `CONFIG_ERROR`
11. **`Cargo.toml::thiserror` 死依赖** — `shared/protocol/Cargo.toml:17`
12. **`desktop_bridge/errors.rs` 5 个 ERR_* 未消费** — `shared/protocol/src/desktop_bridge/errors.rs:3-7`
    - `ERR_PARSE`, `ERR_INVALID_REQUEST`, `ERR_METHOD_NOT_FOUND`, `ERR_INVALID_ARGUMENT`, `ERR_INTERNAL`
13. **`events.rs::ModelInfo` re-export 未消费** — `shared/protocol/src/events.rs:186-198`
14. **`shared/protocol/src/lib.rs` 多个 dead re-export** — 需要 grep 后清理

### shared/ui_logic
15. **4 个空模块** — `shared/ui_logic/src/{api,observability}/mod.rs`, `shared/ui_logic/src/protocol/{events,streaming}.rs`
16. **`protocol::rpc::RpcClient` 整个模块** — `shared/ui_logic/src/protocol/rpc.rs` (96 行)
17. **`ConnectionError::UrlError` 死变体** — `shared/ui_logic/src/connection/connector.rs:30`
18. **`RpcError::Timeout` 死变体** — `shared/ui_logic/src/protocol/rpc.rs:18-19`
19. **`ReconnectStrategy::next_delay` 无外部 caller** — `shared/ui_logic/src/connection/reconnect.rs:21-32`
20. **`ReconnectStrategy::reset` 无 caller** — `shared/ui_logic/src/connection/reconnect.rs:45-47`
21. **`Cargo.toml::uuid` 死依赖** — `shared/ui_logic/Cargo.toml:14`
22. **`prelude` 模块导出死代码** — `shared/ui_logic/src/lib.rs:12-17`

## CONNECT 候选

### shared/protocol
1. **`METHOD_PING` 声明但调用方用字面量** — `shared/protocol/src/desktop_bridge/methods/bridge.rs:5`
   - `desktop/shared/src/bridge/client.rs` 4 处用 `"bridge.ping"` 字面量
   - fix: 字面量改常量

## DECIDE 候选

### shared/client
1. **`GatewayClient::call_raw` 握手响应未校验** (batch5 已知，未修)

### shared/protocol
2. **`AgentTraceEvent::WorktreeCreated`/`WorktreeCleanedUp`** — TUI `_ => {}` ignore
3. **`AgentTraceEvent::McpScopeAttached`/`McpScopeCleaned`** — 同上
4. **`NOTIFY_STATUS_CHANGED` 等协议预埋常量** — 0 消费方

## 复核 Batch5

| 历史问题 | 状态 |
|---|---|
| `PiiScrubbingLayer` no-op | **未修** |
| `jsonrpc.rs:302` uuid 依赖 | **已修**（private `ids.rs` + `AtomicU64`） |
| `events.rs` 980 行 | 仍 991 行（略增），本审计 6+ 死变体可削减 |
| `trace_presentation.rs` 965 行 | 未发现新增死分支 |
| `shared/ui_logic` 空模块 | **未修**（4 个空 mod） |
| `RpcError::Timeout` | **未修** |
| `uuid` 依赖 | **未修** |
| `CliError::WebSocket` | **未修** |
| `gateway_client.rs` 握手响应未校验 | **未修** |
| `Cargo.toml::thiserror` | **未修** |

## 未做

1. 未运行 cargo build / cargo check
2. 未检查 desktop/* 内部 wire
3. 未检查 interfaces 内部 wire
4. 未验证 `serde_json::Value` 在 events.rs MoA 测试的真实性
5. 未追踪 `desktop_bridge::methods::*` 在 Swift helper 端实现