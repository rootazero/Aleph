# ACP Agents & CLI/TUI Split Design

**Date**: 2026-03-13
**Status**: Approved

## Overview

为 Aleph 引入 ACP (Agent Client Protocol) Agent 系统，使 Aleph 能主动 spawn 和管理外部 CLI 工具（Claude Code CLI、Codex CLI、Gemini CLI）作为执行手臂。同时将现有 `apps/cli/` 拆分为 `aleph-cli`（纯命令行）和 `aleph-tui`（交互式终端界面）。

## Key Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| ACP 定位 | Client/Harness Spawner（主动模式） | Aleph 主动管理外部 CLI，而非被 IDE 调用 |
| Agent 角色 | 混合模式（Tool + 独立 Agent） | LLM 可调度 + 用户可直接切换 |
| 协议 | 标准 ACP（NDJSON over stdio） | 兼容性强，社区标准 |
| 在架构中的位置 | Core 层内部组件 | 非 Channel，任何 Channel 的用户都可通过 LLM 触发 |
| 生命周期 | 懒启动 + 持久保活 | 首次使用时 spawn，保持运行直到 Aleph 关闭 |
| CLI/TUI | 拆分为两个独立二进制 | 职责分离：管理命令 vs 交互式聊天 |
| 初期范围 | Claude Code + Codex + Gemini 三个都做 | 设计好 trait，三个适配器并行实现 |

## Architecture

### ACP Module Structure

```
src/acp/
├── mod.rs                  // Module entry, pub exports
├── protocol.rs             // ACP protocol types (NDJSON messages)
├── harness.rs              // AcpHarness trait + StdioTransport
├── manager.rs              // AcpHarnessManager — multi-harness lifecycle
├── session.rs              // AcpSession — single CLI session state
├── harnesses/
│   ├── mod.rs
│   ├── claude_code.rs      // Claude Code CLI adapter
│   ├── codex.rs            // Codex CLI adapter
│   └── gemini.rs           // Gemini CLI adapter
```

### Core Trait

```rust
#[async_trait]
pub trait AcpHarness: Send + Sync {
    /// Harness identifier ("claude-code", "codex", "gemini")
    fn id(&self) -> &str;

    /// Spawn CLI subprocess, return session
    async fn spawn(&self, config: &HarnessConfig) -> Result<AcpSession>;

    /// Check if CLI is installed and available
    async fn is_available(&self) -> bool;
}
```

### AcpHarnessManager

- Holds registered harnesses: `HashMap<String, Box<dyn AcpHarness>>`
- Manages active sessions: `HashMap<SessionId, AcpSession>`
- Lazy start: `get_or_spawn(harness_id)` spawns on first call
- `shutdown_all()` cleans up all subprocesses on Aleph shutdown

### AcpSession

- Holds subprocess handle (`tokio::process::Child`)
- NDJSON read/write streams via `StdioTransport`
- Session state: idle / busy / error
- Message history for Tool mode context passing

## Protocol Layer

### ACP Messages

```rust
/// Requests sent to CLI
#[derive(Serialize, Deserialize)]
#[serde(tag = "method")]
pub enum AcpRequest {
    Initialize { id: String, params: InitializeParams },
    NewSession { id: String, params: NewSessionParams },
    Prompt { id: String, params: PromptParams },
    Cancel { id: String, params: CancelParams },
}

/// Events received from CLI
#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AcpEvent {
    Message { session_id: String, content: MessageContent },
    ToolCall { session_id: String, tool: ToolCallInfo },
    Result { id: String, result: serde_json::Value },
    Error { id: String, error: AcpError },
}
```

### StdioTransport

```rust
pub struct StdioTransport {
    stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
}

impl StdioTransport {
    pub async fn send(&mut self, req: &AcpRequest) -> Result<()>;
    pub fn events(&self) -> impl Stream<Item = Result<AcpEvent>>;
}
```

Each harness adapter only handles: CLI executable path/args, initialization handshake differences, and event mapping quirks.

## Interaction Modes

### Tool Mode

Three independent tools registered for LLM to call:

- `claude_code` — delegate task to Claude Code CLI
- `codex` — delegate task to Codex CLI
- `gemini_cli` — delegate task to Gemini CLI

**Tool input schema (example):**

```rust
pub struct ClaudeCodeToolInput {
    pub prompt: String,
    pub cwd: Option<String>,
}
```

**Flow:** LLM calls tool → Manager spawns/reuses CLI → ACP Prompt → stream collect → return result to LLM

### Agent Mode (Direct Conversation)

User switches via natural language, LLM calls `acp_switch` tool:

```rust
pub struct AcpSwitchToolInput {
    /// Target harness id, or "aleph" to switch back
    pub target: String,
}
```

- Session gains `active_acp_harness: Option<String>` field
- Gateway main loop: if set, forward messages directly to ACP session (skip Aleph LLM)
- If unset, normal LLM main loop
- "Switch back" detected by lightweight LLM check (not keyword matching, per R8)
- TUI provides a hotkey for force-switch-back as failsafe

## Data Flow

### Tool Mode

```
User (any Channel)
  → Gateway main loop
    → LLM Think: "this task suits Codex"
      → LLM Act: call codex tool { prompt: "refactor this function" }
        → AcpHarnessManager::get_or_spawn("codex")
          → StdioTransport::send(Prompt)
            → Codex CLI executes
            ← AcpEvent stream (Message, ToolCall, Result)
          ← Aggregated tool output
        ← Return to LLM main loop
      → LLM integrates result, replies to user
    ← Gateway pushes to user's Channel
```

### Agent Mode

```
User: "switch to Claude Code"
  → LLM calls acp_switch { target: "claude-code" }
  → session.active_acp_harness = Some("claude-code")
  ← "Switched to Claude Code"

User: "write me an HTTP server"
  → Gateway detects active_acp_harness
  → Forward directly to Claude Code CLI (skip Aleph LLM)
  ← CLI streams response back to user's Channel

User: "switch back to Aleph"
  → Gateway lightweight LLM check / TUI hotkey
  → session.active_acp_harness = None
  ← Normal main loop restored
```

## Harness Adapters

### Claude Code CLI

```rust
pub struct ClaudeCodeHarness { executable: String } // default: "claude"
// Spawn: claude --acp --cwd <path>
```

### Codex CLI

```rust
pub struct CodexHarness { executable: String } // default: "codex"
// Spawn: codex --acp
```

### Gemini CLI

```rust
pub struct GeminiHarness { executable: String } // default: "gemini"
// Spawn: gemini --acp (pending confirmation of ACP support)
```

### Configuration

```toml
[acp]
enabled = true

[acp.harnesses.claude-code]
executable = "claude"
enabled = true

[acp.harnesses.codex]
executable = "codex"
enabled = true

[acp.harnesses.gemini]
executable = "gemini"
enabled = true
```

Tools are only registered for enabled + available harnesses.

## Error Handling

| Scenario | Handling |
|----------|---------|
| CLI not installed | `is_available()` returns false → tool not registered / friendly error on call |
| CLI process crash | Detect stdin/stdout close → clean session → re-spawn on next call |
| CLI response timeout | Configurable timeout (default 5 min) → send `cancel` → return timeout error |
| ACP protocol error | Parse failure → log → return error to LLM, never panic |
| CLI crash in Agent mode | Auto-switch back to Aleph main loop → notify user |

## CLI / TUI Split

### Current State

`apps/cli/` is a hybrid: management commands + interactive TUI chat.

### New Structure

```
apps/
├── cli/                    # Pure command-line tool (no interactive UI)
│   └── src/
│       ├── main.rs         # aleph-cli binary
│       ├── commands/       # Subcommands: config, plugins, channels, health, ...
│       └── output.rs       # JSON / human-readable formatting
│
├── tui/                    # Interactive terminal interface
│   └── src/
│       ├── main.rs         # aleph-tui binary
│       ├── app.rs          # Application state
│       ├── render.rs       # Terminal rendering
│       ├── widgets/        # Chat area, input, status bar, etc.
│       ├── markdown.rs     # Markdown rendering
│       └── commands.rs     # Slash commands within TUI
```

### Binary Names

- `aleph` — server (unchanged)
- `aleph-cli` — pure command-line management tool
- `aleph-tui` — interactive terminal chat

### Responsibility Split

| Feature | aleph-cli | aleph-tui |
|---------|-----------|-----------|
| config get/set/edit | Yes | No |
| plugins install/list | Yes | No |
| channels list/status | Yes | No |
| health check | Yes | No |
| Interactive chat | No | Yes |
| Markdown rendering | No | Yes |
| Command palette | No | Yes |
| ACP Agent switching | No | Yes |
| Session management (interactive) | No | Yes |

Both share the same `aleph-protocol` crate for WebSocket + JSON-RPC communication.

## Testing Strategy

| Layer | What | How |
|-------|------|-----|
| Protocol | NDJSON serialization/deserialization | Unit tests, fixed JSON fixtures |
| StdioTransport | Read/write stream correctness | Unit tests, mock stdin/stdout |
| Harness adapters | Spawn args, is_available | Integration tests (skip if CLI not installed) |
| Manager | Lazy start, session reuse, shutdown cleanup | Unit tests, mock AcpHarness trait |
| Tool integration | LLM call → result return | End-to-end tests with mock CLI process |
| Agent mode | Switch/switch-back/crash recovery | Integration tests |

**Mock CLI strategy:** A simple mock ACP server (few dozen lines of Rust) implementing `initialize` + `prompt` → fixed reply. Tests spawn this mock instead of real CLIs, ensuring CI independence from external tool installation.
