# Batch 8 静态审查 — `interface` + `shared`

> **基线**：worktree `review/interface-shared` (基于 main `d61612690`)。
> **方法**：graphify 探索 + 静态 grep-diff + read-before-write 复核。
> **审查日期**：2026-08-13

## 范围

- `shared/client/src/**` (aleph-client)
- `shared/logging/src/**` (aleph-logging)
- `shared/protocol/src/**` (aleph-protocol)
- `shared/ui_logic/src/**` (shared-ui-logic)
- `interfaces/cli/src/**` (aleph-cli)
- `interfaces/tui/src/**` (aleph-tui)
- `interfaces/webchat/src/**` (aleph-panel)

## 修复摘要

| ID  | 模块 | Sev | 标题 | 状态 |
|----:|------|----:|------|:----:|
| B8-01 | shared/client | Med | `anyhow` 死依赖 + 无 caller 的 `From<anyhow::Error>` impl | fixed |
| B8-02 | shared/client | Low | `ManifestConfig` 三个字段（`tool_categories`/`specific_tools`/`excluded_tools`）无 caller | fixed (字段加 `#[allow(dead_code)]` 并保留 serde 字段以避免 wire-shape 漂移) |
| B8-03 | shared/client | Low | `GatewayClient::with_ca_cert` setter 唯一调用点缺失 | fixed (字段加 `#[allow(dead_code)]` 并保留以备未来调用) |

## CUT 候选 (复核)

### shared/client
- `CliError::WebSocket` 变体:  **不存在**（已删），From impl 现在的目标是 `Connection` ✓
- `anyhow` 依赖:  死（无 caller）→ **B8-01 修复**
- `ManifestConfig` struct: 仍在使用（CliConfig.manifest）✓ 但字段 dead → **B8-02 修复**

### shared/logging
- `PiiScrubbingLayer` 已加 `#[deprecated]` ✓ (batch6 未修，现已修)

### shared/protocol
- `ConfigChangedEvent`/`UncertaintyAction`/`ToolSummaryItem`/`ToolErrorItem`/`TokenBreakdownView`: 均有 caller ✓ (batch6 报告过时)
- `discovery`/`invitation` 模块: 不存在 ✓ (已删)
- JSON-RPC 错误码常量: 全部被使用 ✓
- `desktop_bridge/errors.rs` 错误码: 全部被使用 ✓
- `ModelInfo`: 仍在 events.rs 中使用 ✓

### shared/ui_logic
- 空模块: 已清理 ✓
- `ConnectionError::UrlError`: 不存在 ✓ (batch6 报告过时)
- `RpcError::Timeout`: ui_logic 中没有 `RpcError` ✓ (模块已删)
- `ReconnectStrategy::reset`: 不存在 ✓
- `Cargo.toml::uuid` 死依赖: 不存在 ✓ (uuid 依赖已删)
- `prelude` 模块: 不存在 ✓ (已删)
- `RpcClient` 整个模块: 已删 ✓

### interfaces/cli
- 6 处 `result.as_array()` shape 漂移: 已修 ✓ (使用 `result.get("...").and_then(|v| v.as_array())`)
- `proxy.{set,clear}`/`webhook.{add,remove}` stub: 已改写为 gap surface（设计性占位）✓
- `anyhow` 依赖: 实际使用（trace_cmd.rs 中 `anyhow::anyhow!`）✓

### interfaces/webchat
- `AgentsApi::files_delete`/`TeamsApi::task_journal_get`: 已删 ✓
- `canvas_radial_navigation` 死字段: 已删 ✓
- `chat_sidebar` 误订阅 `stream.session_updated`: 已修（现在用 `run.session_updated`）✓
- `artifacts.rs::read_text`/`ping_is_for_session`: read_text 已删, ping_is_for_session 仍使用 ✓
- `GenerationView` 死组件: 文件已删 ✓
- `Cron.toggle` orphan wrapper: 已删 ✓
- `TraceNode::type_class`/`type_icon`: 已删 ✓
- `ChatSendErrorCode::severity_class`: 已删 ✓
- `ChatState::model_for_run`: 已删 ✓

### interfaces/tui
- `Action::*` 死变体（`ToggleHelp`/`ToggleVerbose`/`CyclePalette`/`Reconnect`）: 不存在 ✓ (batch6 报告过时)

## 修复决策说明

### B8-02 决策：保留 ManifestConfig 字段

`ManifestConfig` 是公开类型（被 `lib.rs` re-export），其三个字段是 wire-shape 的一部分。
即使现在没有 caller 删除字段也会让 `aleph-client` 的 serde schema 减少键，
任何持有旧 `~/.aleph/cli.toml` 的用户会看到键被静默丢弃（保留 `#[serde(default)]`
能避免这种情况但仍可能让远程配置检查工具误报丢失）。

选择最小破坏性方案：保留字段，加 `#[allow(dead_code)]` 抑制未使用警告，
并在 `manifest` 字段上加 doc-comment 说明现状（"无 caller，等产品设计后再激活"）。
这样：
- 不破坏 ABI / wire-shape
- 不触发 unused 警告
- 给未来的代码留下重新启用的钩子

### B8-03 决策：保留 GatewayClient::ca_cert + with_ca_cert

`with_ca_cert` 的 doc-comment 已经解释了它的设计意图：
"this client is reached through `aleph-server gateway call`, which by
construction talks to the server on the same machine, and
[`crate::tls::connector_for`] already finds that server's own certificate
for a loopback URL with no configuration at all. The setter exists so a
future non-loopback caller is a one-line change instead of a rediscovery
of this whole problem."

也就是说这是预留接口。保留并加 `#[allow(dead_code)]`。

## 未做

1. 未运行 cargo build / cargo check (按协议)
2. 未审查测试代码
3. 未审查 `archive/` 目录