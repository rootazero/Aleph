# Design: Two-Tier Tool Architecture

## Architectural Philosophy

### The Three-Layer Identity

Every tool in Aether has three distinct identities:

```
┌─────────────────────────────────────────────────────────┐
│  Layer 3: USER PERCEPTION (UI)                          │
│  ┌─────────────────┐  ┌─────────────────────────────┐   │
│  │ System Built-ins│  │     MCP Extensions          │   │
│  │ /fs /git /sys   │  │ /mcp/linear /mcp/postgres   │   │
│  │ Top-level, native│  │ Nested, plugin-like        │   │
│  └─────────────────┘  └─────────────────────────────┘   │
├─────────────────────────────────────────────────────────┤
│  Layer 2: PROTOCOL (LLM Interface)                      │
│  ┌─────────────────────────────────────────────────┐    │
│  │  MCP-compatible JSON Interface                   │    │
│  │  tools/list, tools/call, resources/read          │    │
│  │  (Same interface for both tiers)                 │    │
│  └─────────────────────────────────────────────────┘    │
├─────────────────────────────────────────────────────────┤
│  Layer 1: CODE (Implementation)                         │
│  ┌─────────────────┐  ┌─────────────────────────────┐   │
│  │ Rust Native     │  │    External Process         │   │
│  │ In-process call │  │    JSON-RPC over stdio      │   │
│  │ Zero IPC latency│  │    Process spawn + marshalling│   │
│  └─────────────────┘  └─────────────────────────────┘   │
└─────────────────────────────────────────────────────────┘
```

### Why This Matters

**Problem**: Current architecture treats all tools as "MCP":
```
/mcp/fs/read      ← fs is NOT an external MCP server!
/mcp/git/status   ← git is NOT an external MCP server!
```

**Solution**: Respect each layer's identity:
- **Code**: Rust native vs External process (technical truth)
- **Protocol**: Unified MCP-like JSON (for LLM compatibility)
- **User**: Top-level vs Nested (psychological perception)

## Tier Classification

### Tier 1: System Built-ins

| Tool | Trigger | Implementation | Characteristics |
|------|---------|----------------|-----------------|
| `/fs` | `/fs/read`, `/fs/write` | `git2` crate | Always available, no config |
| `/git` | `/git/status`, `/git/diff` | `std::fs`, `tokio::fs` | Always available, no config |
| `/sys` | `/sys/info`, `/sys/processes` | `sysinfo` crate | Always available, no config |
| `/shell` | `/shell/run` | `tokio::process` | Disabled by default (security) |
| `/search` | `/search` | `reqwest` + APIs | Built-in capability |
| `/video` | `/video` | `yt-dlp` | Built-in capability |

**Key Properties:**
- Compiled into the Aether binary
- Zero IPC overhead (direct function call)
- Top-level command namespace
- "Feels like shell commands"

### Tier 2: MCP Extensions

| Tool | Trigger | Implementation | Characteristics |
|------|---------|----------------|-----------------|
| `/mcp/linear` | `/mcp/linear/...` | External npx process | User-installed |
| `/mcp/postgres` | `/mcp/postgres/...` | External process | User-installed |
| `/mcp/brave-search` | `/mcp/brave-search/...` | External process | Requires API key |

**Key Properties:**
- External processes spawned on demand
- JSON-RPC over stdio communication
- Nested under `/mcp/` namespace
- "Feels like plugins"

## Routing Architecture

### Command Dispatcher

```rust
async fn dispatch_command(cmd: &str) -> Result<Response> {
    let parts: Vec<&str> = cmd.split('/').collect();

    match parts.get(1) {
        // --- Tier 1: System Tools (direct call) ---
        Some("fs") => system_tools::fs::handle(parts).await,
        Some("git") => system_tools::git::handle(parts).await,
        Some("sys") => system_tools::sys::handle(parts).await,
        Some("shell") => system_tools::shell::handle(parts).await,

        // --- Tier 2: MCP Extensions (JSON-RPC) ---
        Some("mcp") => {
            let server_name = parts.get(2).ok_or(Error::MissingServer)?;
            mcp_manager::dispatch(server_name, &parts[3..]).await
        },

        _ => Err(Error::UnknownCommand)
    }
}
```

### Performance Comparison

| Operation | Tier 1 (System) | Tier 2 (MCP) |
|-----------|-----------------|--------------|
| `/fs/read` | ~1ms (direct call) | N/A |
| `/git/status` | ~5ms (libgit2) | ~50ms (if external) |
| `/mcp/linear/create` | N/A | ~200ms (IPC + network) |

## Config Structure

### Before (Confusing)

```toml
[mcp]
enabled = true

[mcp.builtin]           # ← Confusing: "builtin MCP" is an oxymoron
fs_enabled = true
git_enabled = true
shell_enabled = false

[[mcp.external_servers]]
name = "linear"
command = "npx"
```

### After (Clear)

```toml
# Tier 1: System Built-ins
[tools]
fs_enabled = true
git_enabled = true
shell_enabled = false
sys_enabled = true
allowed_roots = ["/Users/me/projects"]
allowed_commands = ["ls", "cat", "pwd"]

# Tier 2: MCP Extensions
[mcp]
enabled = true

[[mcp.servers]]
name = "linear"
command = "npx"
args = ["-y", "@anthropic-ai/mcp-server-linear"]
env = { LINEAR_API_KEY = "..." }
```

## UI Design

### Settings View Structure

```
┌─────────────────────────────────────────────────────────┐
│ Tools & Extensions Settings                             │
├─────────────────────────────────────────────────────────┤
│                                                         │
│ ◆ System Tools                    [Always Available]    │
│ ├─ ☑ File System (/fs)                                 │
│ │    Allowed paths: ~/projects, ~/documents            │
│ ├─ ☑ Git (/git)                                        │
│ │    Allowed repos: ~/projects/*                       │
│ ├─ ☐ Shell (/shell)              [Security Warning]    │
│ │    Allowed commands: ls, cat, pwd                    │
│ └─ ☑ System Info (/sys)                                │
│                                                         │
│ ◆ MCP Extensions                  [User Installed]      │
│ ├─ ● linear         Running       [Manage]             │
│ ├─ ○ postgres       Stopped       [Start]              │
│ └─ + Add MCP Server...                                 │
│                                                         │
└─────────────────────────────────────────────────────────┘
```

### Command Palette

```
┌─────────────────────────────────────────────────────────┐
│ /                                                       │
├─────────────────────────────────────────────────────────┤
│ ◇ fs        File system operations    [System]         │
│ ◇ git       Git repository tools      [System]         │
│ ◇ sys       System information        [System]         │
│ ◇ shell     Execute shell commands    [System]         │
│ ─────────────────────────────────────────────────────  │
│ ◇ search    Web search                [Capability]     │
│ ◇ video     Video transcripts         [Capability]     │
│ ─────────────────────────────────────────────────────  │
│ ◇ mcp       MCP Extensions →          [Extensions]     │
│   ├─ linear                                            │
│   └─ postgres                                          │
└─────────────────────────────────────────────────────────┘
```

## Security Model

### Tier 1: Trusted Core
- Runs in-process with full Rust safety guarantees
- Sandboxed by `allowed_roots`, `allowed_commands` config
- Shell disabled by default (opt-in)

### Tier 2: Sandboxed Extensions
- Each server is a separate subprocess
- Communication only via JSON-RPC
- No direct filesystem/network access beyond declared capabilities
- User must explicitly install and enable

## Migration Path

### Automatic Config Migration

```rust
impl Config {
    fn migrate_legacy_format(&mut self) {
        // Migrate [mcp.builtin] → [tools]
        if let Some(builtin) = self.mcp.builtin.take() {
            self.tools = builtin.into();
            tracing::warn!(
                "Migrated config: [mcp.builtin] → [tools]. \
                 Please update your config.toml."
            );
        }
    }
}
```

### Command Alias Period

During transition, support both old and new commands:

```rust
fn resolve_command(cmd: &str) -> &str {
    match cmd {
        "/mcp/fs" => "/fs",       // Legacy → New
        "/mcp/git" => "/git",     // Legacy → New
        "/mcp/shell" => "/shell", // Legacy → New
        "/mcp/system" => "/sys",  // Legacy → New (also renamed!)
        _ => cmd
    }
}
```

## Alternatives Considered

### A1: Keep Everything Under /mcp
- **Rejected**: Violates "native first" principle
- Users shouldn't think of file reading as a "plugin"

### A2: Separate /builtin Namespace
- **Rejected**: Still verbose (`/builtin/fs` vs `/fs`)
- Adds unnecessary namespace layer

### A3: No Namespace for Extensions
- **Rejected**: Would pollute global command space
- `/linear`, `/postgres` feel like system commands (they're not)
