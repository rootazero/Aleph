# ACP Harness Management — Design Document

**Date**: 2026-03-16
**Status**: Approved
**Scope**: ACP harness 可视化配置与管理系统

## Background

Aleph 已完成 ACP 基础设施（协议层、传输层、会话管理、3 个 harness 适配器、Manager、4 个内置工具）。当前 ACP 作为工具暴露给 LLM，定位为"四肢"而非"大脑"。

**关键决策**：ACP 定位为 **工具管理**（非 AI 供应商）。原因：
1. AiProvider 要求 tool_use、structured output、system prompt，ACP harness 是纯文本 in/out
2. 如果 CLI 是 provider，Aleph 的记忆、工具、人格系统全部被绕过
3. 用户选 Aleph 要它的编排能力，不是 CLI wrapper

## Architecture — 方案一：扩展现有 AcpHarness Trait

在现有 ACP 基础设施上增量扩展。复用 90% 现有代码。

## Part 1: Data Model & Configuration

### AcpHarnessConfig

```rust
// src/config/types/acp.rs

pub struct AcpConfig {
    pub enabled: bool,
    pub harnesses: HashMap<String, AcpHarnessConfig>,
}

pub struct AcpHarnessConfig {
    pub display_name: String,
    pub executable: String,
    pub args: Vec<String>,
    pub mode: HarnessMode,            // NativeAcp | Oneshot
    pub output_format: OutputFormat,  // PlainText | Json { field }
    pub env: HashMap<String, String>,
    pub cwd: Option<String>,
    pub timeout_seconds: u64,         // default 300
    pub enabled: bool,
    pub preset: Option<String>,       // "claude-code" | "codex" | "gemini" | None
}

pub enum OutputFormat {
    PlainText,
    Json { field: String },
}
```

### Preset Defaults

| Preset | executable | args | mode | output_format |
|--------|-----------|------|------|---------------|
| Claude Code | `claude` | `["--print", "--output-format", "json"]` | Oneshot | `Json { field: "result" }` |
| Codex | `codex` | `["exec"]` | Oneshot | PlainText |
| Gemini | `gemini` | `["--acp"]` | NativeAcp | PlainText |

## Part 2: Gateway RPC Handlers

New file: `src/gateway/handlers/acp_config.rs`

### API Methods

| Method | Description |
|--------|-------------|
| `acp.list` | List all harnesses (preset + custom), includes availability check |
| `acp.get(id)` | Get single harness detail |
| `acp.create(config)` | Add custom harness |
| `acp.update(id, config)` | Update harness config |
| `acp.delete(id)` | Delete custom harness (presets cannot be deleted) |
| `acp.test(id)` | Test CLI availability and connectivity |
| `acp.set_enabled(id, bool)` | Quick enable/disable toggle |
| `acp.presets` | Return 3 preset default configs |

### Return Types

```rust
pub struct AcpHarnessInfo {
    pub id: String,
    pub display_name: String,
    pub executable: String,
    pub mode: String,           // "native_acp" | "oneshot"
    pub enabled: bool,
    pub available: bool,        // CLI found in PATH
    pub preset: Option<String>,
    pub config: AcpHarnessConfig,
}

pub struct AcpTestResult {
    pub success: bool,
    pub message: String,        // success: version; failure: error
    pub duration_ms: u64,
}
```

### Key Behaviors

- `acp.list` auto-detects `available` via `is_available()` check
- `acp.test` runs simple prompt for Oneshot, full initialize→prompt for NativeAcp
- `acp.create`/`acp.update` triggers `AcpHarnessManager` hot-reload
- `acp.delete` rejects preset harness deletion (return error)
- Config changes broadcast `config.acp.changed` event

## Part 3: AcpHarnessManager Extension

### Dynamic Registration

```rust
pub struct AcpHarnessManager {
    harnesses: RwLock<HashMap<String, Arc<dyn AcpHarness>>>,
    sessions: RwLock<HashMap<String, AcpSession>>,
    configs: RwLock<HashMap<String, AcpHarnessConfig>>,  // new
}

// New methods
impl AcpHarnessManager {
    pub fn register_harness(&self, id: &str, config: AcpHarnessConfig) -> Result<()>;
    pub fn unregister_harness(&self, id: &str) -> Result<()>;
    pub fn update_harness(&self, id: &str, config: AcpHarnessConfig) -> Result<()>;
    pub fn get_config(&self, id: &str) -> Option<AcpHarnessConfig>;
    pub fn list_configs(&self) -> Vec<(String, AcpHarnessConfig)>;
}
```

### CustomHarness

New file: `src/acp/harnesses/custom.rs`

```rust
pub struct CustomHarness {
    id: String,
    config: AcpHarnessConfig,
}

impl AcpHarness for CustomHarness {
    // Build HarnessConfig from AcpHarnessConfig fields
    // execute_oneshot: parse output per output_format (PlainText / Json)
}
```

### Initialization Flow

Startup loads all harness configs (preset + custom) from `AcpConfig`, creates appropriate harness instances (preset → dedicated harness impl, custom → CustomHarness), registers all.

## Part 4: Panel UI Design

### Page Location

Settings → Extensions → **ACP** tab (alongside MCP / Plugins / Skills)

### Layout — Split-Pane

```
┌─────────────────────────────────────────────────────────┐
│ ACP Agent CLI                              [global toggle]│
├──────────────────────┬──────────────────────────────────┤
│                      │                                  │
│  ── Preset CLI ──    │   Detail Panel                   │
│  [Card] [Card]       │   - Name, status, version        │
│  [Card]              │   - Core config (executable,     │
│                      │     mode, timeout)                │
│  ── Custom CLI ──    │   - Advanced (collapsed):        │
│  [Card]              │     args, output format, env,    │
│                      │     cwd                          │
│  [+ Add Custom CLI]  │   - [Test] [Enable/Disable]     │
│                      │                                  │
└──────────────────────┴──────────────────────────────────┘
```

### Left Panel

- **Preset card grid** (3 cards): icon, name, availability badge (installed ✓ / not installed ✗), enabled indicator
- **Custom CLI list**: same card style
- **"+ Add Custom CLI"** button

### Right Panel (on selection)

- **Basic info**: name, status (installed + version / not installed)
- **Core config**: executable path, mode dropdown, timeout
- **Advanced settings** (collapsible, default collapsed):
  - Args (tag list input)
  - Output format (PlainText / Json + field name), only for Oneshot mode
  - Environment variables (key-value list)
  - Working directory
- **Actions**: Test Connection, Enable/Disable, Delete (custom only)

### Interaction

- Preset cards: cannot delete, only disable
- Test connection: success → toast with version; failure → toast with error
- Auto-save on change (debounced)
- Advanced settings collapsed by default for presets

## Part 5: Error Handling & Edge Cases

### CLI Unavailable
- `acp.list` returns `available: false`, card shows gray + "Not Installed" badge
- Config allowed even when CLI not installed (configure first, install later)
- Tool invocation returns friendly error to LLM

### Process Failures
- Oneshot timeout → kill process, return timeout error
- NativeAcp session crash → `ensure_session` auto-restart
- Bad executable path → `acp.test` returns "command not found"

### Config Validation
- Custom harness ID cannot conflict with preset IDs (`claude-code`/`codex`/`gemini`)
- Custom harness ID restricted to `[a-z0-9-]`
- Validated at Gateway handler level

### Hot Reload
- Config change → kill active NativeAcp sessions, rebuild
- Oneshot: stateless, next invocation uses new config automatically
- Broadcast `config.acp.changed` event for Panel refresh
