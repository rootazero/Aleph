# CLI TUI Upgrade Design

> Aleph CLI 从基础 stdin/stdout 升级为 ratatui 全功能分屏 TUI

**Date**: 2026-03-04
**Status**: Approved
**Scope**: `apps/cli/` — 纯协议客户端，0 依赖 core

---

## Background

Aleph CLI 当前是 ~1,700 行的基础命令行客户端，通过 JSON-RPC 2.0 WebSocket 连接 Gateway。已引入 ratatui 0.29 + crossterm 0.28 但未使用。对比 OpenClaw CLI 的 TUI REPL 体验，存在严重差距：

- 无分屏布局，纯 stdin/stdout 流式输出
- 仅 3 个 slash 命令 (/help, /clear, /session)
- 无 Markdown 渲染、无颜色主题
- 工具执行仅显示 ✓/✗，无进度、无耗时
- 无命令历史、无自动补全

## Constraints

- **R1/R4 红线**: CLI 保持纯协议客户端，仅依赖 `aleph-protocol`
- **P6 简洁性**: 不预留不需要的抽象，Markdown 只实现终端有意义的子集
- **兼容性**: `ask` 命令保持 one-shot 模式不变（管道/脚本场景）

---

## Architecture

### Core Pattern: Event → Action → State → Render

```
┌─────────────────────────────────────────────────┐
│                   App (主循环)                    │
│                                                   │
│  ┌──────────┐  ┌──────────┐  ┌───────────────┐  │
│  │ Terminal  │  │ Gateway  │  │   AppState     │  │
│  │  Events   │  │  Events  │  │               │  │
│  │(keyboard, │  │(stream,  │  │ messages[]    │  │
│  │ mouse,    │  │ tool,    │  │ input_buffer  │  │
│  │ resize)   │  │ error)   │  │ session_info  │  │
│  └────┬─────┘  └────┬─────┘  │ command_state │  │
│       │              │        └───────┬───────┘  │
│       ▼              ▼                │           │
│  ┌─────────────────────────┐         │           │
│  │     Event Router        │────────►│           │
│  │  event → Action dispatch│         │           │
│  └─────────────────────────┘         │           │
│                                      ▼           │
│  ┌─────────────────────────────────────────┐     │
│  │              Render (每帧)               │     │
│  │                                         │     │
│  │  AppState → Layout → Widgets → Frame    │     │
│  └─────────────────────────────────────────┘     │
└─────────────────────────────────────────────────┘
```

### Main Loop

```rust
loop {
    terminal.draw(|frame| ui::render(frame, &app_state))?;

    let action = tokio::select! {
        Some(term_event) = term_rx.recv() => {
            app_state.handle_terminal_event(term_event)
        }
        Some(gw_event) = gateway_rx.recv() => {
            app_state.handle_gateway_event(gw_event)
        }
        _ = tick_interval.tick() => Action::Tick,
    };

    match action {
        Action::Quit => break,
        Action::SendMessage(msg) => client.send_message(&msg).await?,
        Action::SlashCommand(cmd) => handle_slash_command(&mut app_state, &client, cmd).await?,
        Action::ScrollUp(n) => app_state.scroll_up(n),
        Action::None => {}
        // ...
    }
}
```

---

## Layout

Three-zone split layout using ratatui `Layout::vertical`:

```
┌─ Chat ──────────────────────────────────────┐
│                                              │
│  ┃ You                            14:32:01   │
│  │ Tell me about Rust generics               │
│                                              │
│  ┃ Aleph                          14:32:03   │
│  │ Rust generics allow you to write          │
│  │ **abstract code** that works with...      │
│  │                                           │
│  │  ┌ web_search ─────────────── 1.2s ✓ ┐   │
│  │  │ "Rust generics tutorial"           │   │
│  │  └────────────────────────────────────┘   │
│  │                                           │
│  │ Here's what I found...                    │
│  │  ▍ (streaming...)                         │
│                                              │
├─ Input ─────────────────────────────────────┤
│  > Type your message...    (Shift+Enter=NL)  │
│                                              │
├──────────────────────────────────────────────┤
│  ● claude-opus │ session:chat-abc │ 3.2k tok │
└──────────────────────────────────────────────┘
```

```rust
let chunks = Layout::vertical([
    Constraint::Min(5),        // Chat Area (fills remaining)
    Constraint::Length(height), // Input Area (dynamic 3-8 lines)
    Constraint::Length(1),      // Status Bar (fixed 1 line)
]).split(frame.area());
```

### Chat Area

- Messages identified by colored left bar: `┃` (user=blue, assistant=green, system=yellow)
- `scroll_offset` tracks position; `auto_scroll` disables when user scrolls up, re-enables at bottom
- Streaming: `is_streaming=true` shows cursor `▍` at tail
- Tool execution blocks inline with status icon (⟳/✓/✗), name, params, duration

### Input Area

- `tui-textarea` crate for multi-line editing
- Dynamic height: grows with content (min=3, max=8 lines)
- Enter=send, Shift+Enter=newline, Ctrl+U=clear, Ctrl+W=delete word
- ↑/↓ on empty input = browse send history
- `/` on empty input triggers command palette

### Status Bar

```
● claude-opus │ session:chat-abc │ 3.2k tok │ /help for commands
```

Left: model (green dot=connected, red=disconnected). Middle: session key. Right: token count + hint.

### Command Palette (floating overlay)

Appears above input area when `/` typed. Fuzzy-filters as user types. ↑/↓ to select, Tab/Enter to confirm, Esc to close.

```
  ┌─ Commands ───────────────────────────┐
  │  /new       Create new session       │
  │ >/model     Switch AI model          │
  │  /think     Set thinking level       │
  └──────────────────────────────────────┘
```

### AskUser Dialog (inline)

Number keys for quick selection, ↑/↓ + Enter, Esc to cancel.

```
  ┌─ Agent needs your input ────────────┐
  │ Execute `rm -rf ./build`?           │
  │  [1] Yes    [2] No    [3] Always    │
  └─────────────────────────────────────┘
```

---

## Slash Commands

All commands communicate via JSON-RPC; CLI contains no business logic.

| Command | Args | RPC Method | Description |
|---------|------|------------|-------------|
| `/new [name]` | optional name | `sessions.create` | Create & switch session |
| `/session <key>` | session key | local switch | Switch current session |
| `/sessions` | — | `sessions.list` | List all sessions |
| `/delete <key>` | session key | `sessions.delete` | Delete session |
| `/model <name>` | model name | `config.set` | Switch model |
| `/models` | — | `providers.list` | List available models |
| `/think <level>` | off/low/medium/high | local state | Set reasoning depth |
| `/usage` | — | `usage.current` | Show token usage |
| `/status` | — | `health` + local | Show system status |
| `/verbose` | — | local toggle | Toggle reasoning display |
| `/health` | — | `health` | Server health check |
| `/tools [filter]` | optional filter | `commands.list` | List available tools |
| `/memory <query>` | search term | `memory.search` | Search memory |
| `/compact` | — | `sessions.compact` | Compress session context |
| `/clear` | — | local | Clear screen |
| `/help` | — | local | Show command help |
| `/quit` | — | local | Exit TUI |

### New RPC Methods Required

These 3 Gateway handlers need to be added (independent work, not blocking TUI):

| RPC Method | Purpose |
|------------|---------|
| `usage.current` | Return token stats for current session |
| `memory.search` | Hybrid vector+FTS memory search |
| `sessions.compact` | Compress session context |

TUI gracefully handles "method not found" errors for these until implemented.

### Parser

```rust
enum SlashCommand {
    New { name: Option<String> },
    Session { key: String },
    Sessions,
    Delete { key: String },
    Model { name: String },
    Models,
    Think { level: ThinkingLevel },
    Usage,
    Status,
    Verbose,
    Health,
    Clear,
    Tools { filter: Option<String> },
    Memory { query: String },
    Compact,
    Help,
    Quit,
}
```

---

## Streaming & Tool Execution

### StreamEvent → AppState Mapping

| StreamEvent | State Mutation |
|-------------|----------------|
| `RunAccepted` | Create empty assistant message, set `current_run` |
| `Reasoning` | Append to reasoning buffer (visible when verbose) |
| `ResponseChunk` | Append to assistant content, show cursor if not final |
| `ToolStart` | Push `ToolExecution { status: Running }` |
| `ToolUpdate` | Update tool progress |
| `ToolEnd` | Set tool status to Success/Failed, record duration |
| `RunComplete` | Clear streaming state, update token usage |
| `RunError` | Add system error message |
| `AskUser` | Show inline confirmation dialog |

### Tool Block Rendering

Three visual states:

```
Running:   ┌ web_search ──────────────── ⟳ ┐
           │ "Rust generics tutorial"      │
           └───────────────────────────────┘

Success:   ┌ web_search ─────────── 1.2s ✓ ┐
           │ "Rust generics tutorial"      │
           └───────────────────────────────┘

Failed:    ┌ read_file ──────────── 0.4s ✗ ┐
           │ /nonexistent/path.rs          │
           │ Error: file not found         │
           └───────────────────────────────┘
```

Spinner animation: `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏` cycle, driven by Tick events at ~80ms.

### Streaming Markdown (Lazy Rendering)

```rust
struct StreamBuffer {
    raw: String,
    rendered_lines: Vec<StyledLine>,
    dirty_from: usize,
}
```

Only re-renders from the last incomplete paragraph. Completed paragraphs cache their rendered output.

### Supported Markdown Subset

- `**bold**`, `*italic*`, `` `inline code` `` (reverse background)
- ```` ```lang ``` ```` code blocks (bordered, language label)
- `# heading` (bold + underline)
- `- list item` (indented)
- `> quote` (gray bar)
- `[link](url)` (blue underline, text only)
- **Not supported**: tables, images, HTML — not meaningful in terminal

---

## Key Bindings

### Global

| Key | Action |
|-----|--------|
| `Ctrl+C` | Cancel active run → clear input → confirm quit (smart cascade) |
| `Ctrl+D` | Quit immediately |
| `Esc` | Close overlay/dialog/palette |
| `F1` | Help |

### Chat Area Focus

| Key | Action |
|-----|--------|
| `↑`/`k`, `↓`/`j` | Scroll 1 line |
| `PgUp`/`PgDn` | Scroll half page |
| `Home`/`g`, `End`/`G` | Top / bottom (re-enable auto_scroll) |
| `Tab` | Focus → Input |

### Input Area Focus (default)

| Key | Action |
|-----|--------|
| `Enter` | Send message |
| `Shift+Enter` | Insert newline |
| `↑` (empty, 2x) | Focus → Chat |
| `↑`/`↓` (empty) | Browse send history |
| `Ctrl+U` | Clear input |
| `Ctrl+W` | Delete previous word |
| `/` (empty) | Open command palette |

### Focus Flow

```
Input (default) ──↑↑──→ Chat ──Tab──→ Input
  │                                     ↑
  └──/──→ CommandPalette ──Esc/Enter──→─┘
                                        ↑
Gateway AskUser ──→ Dialog ──select──→──┘
```

### Ctrl+C Smart Behavior

1. Active run exists → cancel run (don't exit)
2. Input has content → clear input
3. Empty state → first press shows "press again to quit", second press quits

---

## Theme & Visual

### Color Scheme

```rust
pub struct Theme {
    // Roles
    pub user: Color,              // Blue
    pub assistant: Color,         // Green
    pub system: Color,            // Yellow

    // Tool status
    pub tool_running: Color,      // Yellow
    pub tool_success: Color,      // Green
    pub tool_failed: Color,       // Red
    pub tool_name: Color,         // Cyan
    pub tool_param: Color,        // DarkGray
    pub tool_duration: Color,     // DarkGray

    // Markdown
    pub code_bg: Color,           // DarkGray
    pub code_block_border: Color, // Gray
    pub heading: Color,           // White + Bold
    pub link: Color,              // Blue + Underline
    pub quote: Color,             // DarkGray

    // UI
    pub border: Color,            // Gray
    pub border_focused: Color,    // White
    pub status_bg: Color,         // DarkGray
    pub status_fg: Color,         // White
    pub connected: Color,         // Green
    pub disconnected: Color,      // Red

    // Text
    pub primary: Color,           // White
    pub muted: Color,             // DarkGray
    pub reasoning: Color,         // DarkGray
    pub error: Color,             // Red
    pub warning: Color,           // Yellow
}
```

### Terminal Compatibility

- `NO_COLOR` env → all colors disabled
- `COLORTERM=truecolor` → RGB values
- Otherwise → ANSI 256-color fallback

---

## File Structure

```
apps/cli/src/
├── main.rs                    # Entry: CLI args, dispatch to TUI or one-shot
├── client.rs                  # AlephClient (existing, keep)
├── config.rs                  # CliConfig (existing, keep)
├── error.rs                   # CliError (existing, keep)
├── commands/                  # One-shot commands (existing, keep)
│   ├── mod.rs
│   ├── ask.rs                 # One-shot mode (unchanged)
│   ├── chat.rs                # Modified: launches TUI instead of REPL
│   ├── connect.rs
│   ├── health.rs
│   ├── info.rs
│   ├── session.rs
│   ├── tools.rs
│   └── guests.rs
├── tui/                       # 🆕 TUI module
│   ├── mod.rs                 # Entry function tui::run()
│   ├── app.rs                 # AppState + Action + event dispatch (~300 lines)
│   ├── event.rs               # Terminal event collector (~80 lines)
│   ├── render.rs              # Main render: layout → widgets (~100 lines)
│   ├── widgets/
│   │   ├── mod.rs
│   │   ├── chat_area.rs       # Message list + scroll + streaming (~350 lines)
│   │   ├── input_area.rs      # tui-textarea wrapper + history (~150 lines)
│   │   ├── status_bar.rs      # Bottom status bar (~60 lines)
│   │   ├── command_palette.rs # Slash command overlay (~200 lines)
│   │   ├── dialog.rs          # AskUser dialog (~100 lines)
│   │   └── tool_block.rs      # Tool execution rendering (~80 lines)
│   ├── markdown.rs            # Markdown → Spans conversion (~250 lines)
│   ├── theme.rs               # Color definitions (~40 lines)
│   └── slash.rs               # SlashCommand parser (~150 lines)
└── ui/mod.rs                  # Delete (was placeholder)
```

### New Dependencies

```toml
tui-textarea = "0.7"     # Multi-line editor widget
unicode-width = "0.2"    # CJK/emoji width calculation
textwrap = "0.16"        # Word wrapping
```

### Estimated Size

~1,860 new lines. Total CLI: ~3,500 lines.

---

## Key Types

```rust
enum Action {
    None, Quit, Tick,
    SendMessage(String),
    SlashCommand(SlashCommand),
    CancelRun(String),
    ScrollUp(usize), ScrollDown(usize), ScrollToBottom,
    ScrollToBottomIfAutoScroll,
    SwitchSession(String),
    ToggleVerbose,
    RespondToDialog { run_id: String, choice: String },
}

enum ChatMessage {
    User { content: String, timestamp: DateTime<Utc> },
    Assistant {
        content: String,
        tools: Vec<ToolExecution>,
        reasoning: Option<String>,
        is_streaming: bool,
    },
    System { content: String },
}

struct ToolExecution {
    id: String,
    name: String,
    params: String,
    status: ToolStatus,
    duration: Option<Duration>,
    progress: Option<String>,
    error: Option<String>,
}

enum ToolStatus { Running, Success, Failed }

enum Focus { Input, Chat, CommandPalette, Dialog }
