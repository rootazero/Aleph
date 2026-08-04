# Batch 6 静态审查候选清单：interfaces 模块

> **基线**：worktree `feat/severed-wire-audit-batch6`，与 `main` 一致。
> **方法**：每种 seam 独立 grep-diff + read-before-write 复核。
> **审查日期**：2026-08-04

## 范围

- `interfaces/cli/src/**`
- `interfaces/tui/src/**`
- `interfaces/webchat/src/**`

## 客户端幽灵数: 0（所有客户端调用都有对应handler）

## CONNECT 候选（必须修复）

### 1. [CLI] 6 处 `result.as_array()` shape 漂移
- **file**: `interfaces/cli/src/commands/{providers,services,workspace,plugins,mcp,exec}_cmd.rs`
- **服务端**: 返回 `{ "providers": [...] }` 等对象，但 CLI 解析顶层 array
- **影响**: CLI 命令永远打印 "0 rows" / "No pending approval requests."
- **fix**:
  - `providers_cmd.rs:14`: `result.get("providers").and_then(|v| v.as_array())`
  - `services_cmd.rs:29`: `result.get("services").and_then(|v| v.as_array())`
  - `workspace_cmd.rs:15`: `result.get("workspaces").and_then(|v| v.as_array())`
  - `plugins_cmd.rs:67`: `result.get("plugins").and_then(|v| v.as_array())`
  - `mcp_cmd.rs:19`: `result.get("pending").and_then(|v| v.as_array())`
  - `exec_cmd.rs`: 同样检查

### 2. [WEBCHAT] `ProviderInfo` 字段缺省导致面板默认 disabled
- **file**: `interfaces/webchat/src/api/providers.rs:5-29`
- **服务端**: 发 `ProviderInfo` 完整 DTO
- **客户端 DTO**: 字段缺失 → serde 丢字段但不报错 → 面板永远 disabled
- **fix**: 给所有字段补 `#[serde(default)]` + `provider_type` 默认 `"unknown"`，`color` 默认值

### 3. [WEBCHAT] `WorkspaceApi` 模块注释误导
- **file**: `interfaces/webchat/src/api/workspace.rs:1-41`
- **注释**: 自称 "the original workspace.list method was dead"
- **实际**: `agent_bindings`/`set_channel_agent` 仍 active
- **fix**: 改注释

### 4. [WEBCHAT] `SettingsTab::i18n_label` mixed 语种
- **file**: `interfaces/webchat/src/components/settings_sidebar.rs:77-102`
- **问题**: 中文/英文直字面量穿插在 i18n macro 间
- **fix**: 补 en.json key + 全部走 `t_string!`

## CUT 候选（必须删除）

### 1. [CLI] `proxy.{set,clear}` + `webhook.{add,remove}` stub
- **file**: `interfaces/cli/src/commands/{proxy,webhook}_cmd.rs` `print_unimplemented` 调用
- **fix**: 删函数，enum 变体直接返回 `CliError::Other("not implemented yet")`

### 2. [WEBCHAT] API wrapper orphans (3 个)
- `interfaces/webchat/src/api/agents.rs:178` `AgentsApi::files_delete` — 0 caller
- `interfaces/webchat/src/api/teams.rs:461` `TeamsApi::task_journal_get` — 0 caller
- **fix**: 删除函数

### 3. [WEBCHAT] `canvas_radial_navigation` 死字段
- **producer**: `interfaces/webchat/src/context.rs:354, 587`
- **saver**: `interfaces/webchat/src/api/settings.rs:54` `save_canvas_radial_navigation(value: bool)` — 0 caller
- **fix**: 删 signal + saver；保留 localStorage 读端

### 4. [WEBCHAT] 6 个 dead helper
- `interfaces/webchat/src/models.rs:99` `TraceNode::type_class()` — 0 caller
- `interfaces/webchat/src/models.rs:122` `TraceNode::type_icon()` — 0 caller
- `interfaces/webchat/src/platform/wide/views/chat/state.rs:110` `ChatSendErrorCode::severity_class()` — 0 caller
- `interfaces/webchat/src/platform/wide/views/chat/state.rs:1118` `ChatState::model_for_run()` — 0 caller
- **fix**: 删除函数

### 5. [WEBCHAT] `chat_sidebar` 误订阅 `stream.session_updated`
- **file**: `interfaces/webchat/src/components/chat_sidebar.rs:565-600`
- **fix**: 删除整段 `subscribe_topic` 块

### 6. [WEBCHAT] `artifacts.rs` `read_text`/`ping_is_for_session` orphan
- **file**: `interfaces/webchat/src/api/artifacts.rs:175-200`
- **fix**: 删函数（`list`/`export_html` 保留）

### 7. [WEBCHAT-WIDE] `GenerationView` 死组件
- **file**: `interfaces/webchat/src/platform/wide/views/settings/generation.rs` 整个文件
- **fix**: 删除整个文件（`generation_providers/settings_panel.rs` 已覆盖功能）

### 8. [WEBCHAT-WIDE] `Cron.toggle` orphan wrapper
- **file**: `interfaces/webchat/src/api/cron.rs:209-223`
- **fix**: 删 wrapper（panel 用 `cron.update`）

## DECIDE 候选（产品判断）

### 1. [CLI] `skills.toggle` 隐式别名
- 服务端没注册 `skills.toggle`，但 `commands/skills_cmd.rs` 在 bundled.sync 中调用
- 面板用更宽 `skills.update`
- **decide**: 切到 `skills.update` 或保留 alias

### 2. [TUI] `Action::*` 5 个变体无触发
- `Action::ToggleHelp/ToggleVerbose/CyclePalette/Reconnect` 在 keys.rs 中无分支
- **decide**: 移除变体或接通 keys

### 3. [TUI/CLI] MCP handler stub
- `src/gateway/handlers/mcp.rs:319-378` `handle_list_pending_approvals` 等返回 `{"success": true}` 但无 effect
- CLI mcp_cmd 全部 caller
- **decide**: 接入 `ExecApprovalManager` 或 CUT MCP 路径

### 4. [WEBCHAT] `/settings/cron` 空 fall-through
- `interfaces/webchat/src/app.rs:610-655` `desktop_settings_body` 无 `/settings/cron` 分支
- **decide**: 加视图或保留回退

### 5. [WEBCHAT-WIDE] runtime install topic drift（待验证）

## 总结表

| 候选 | 模块 | form | triage |
|---|---|---|---|
| 6 处 `result.as_array()` shape 漂移 | CLI | 6 | **CONNECT** |
| `ProviderInfo` 字段缺省 | webchat | 6 | **CONNECT** |
| `WorkspaceApi` 注释误导 | webchat | 7 | **CONNECT** |
| `SettingsTab::i18n_label` mixed 语种 | webchat | 6 | **CONNECT** |
| `proxy.{set,clear}` + `webhook.{add,remove}` | CLI | 7 | **CUT** |
| 3 个 API wrapper orphan | webchat | 1 | **CUT** |
| `canvas_radial_navigation` 死字段 | webchat | 7+3 | **CUT** |
| 6 个 dead helper | webchat | 1 | **CUT** |
| `chat_sidebar` 误订阅 | webchat | 7 | **CUT** |
| `artifacts.rs` orphan | webchat | 1 | **CUT** |
| `GenerationView` 死组件 | webchat | 1 | **CUT** |
| `Cron.toggle` orphan wrapper | webchat | 1+6 | **CUT** |

## 未做

- 服务端源码 `src/` 下的 stub 注册（不在 interfaces 范围）
- i18n key 翻译文本一致性校验
- Tauri IPC `plugin:autostart` 已验证无 severed wire
- `memory.clear` 等 handler stub 在 server-side（不在 interfaces 范围）