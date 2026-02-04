# Proposal: Rename Builtin MCP to System Tools

**Status**: Deployed
**Author**: Claude
**Created**: 2025-01-09
**Updated**: 2025-01-09
**Deployed**: 2025-01-09

## Summary

Establish a clear two-tier tool architecture that correctly separates **System Built-ins** (Tier 1) from **MCP Extensions** (Tier 2). This affects code organization, config structure, and most importantly, the user-facing command namespace.

## Three-Layer Identity Problem

The fs, git, shell, system_info tools have three different identities:

| Layer | Identity | Reality |
|-------|----------|---------|
| **Code** | Rust Libraries | `git2`, `std::fs`, `tokio::fs`, `sysinfo` crates |
| **Protocol** | MCP-compatible | JSON interface for LLM tool invocation |
| **User (UI)** | System Built-ins | Top-level commands, NOT nested under `/mcp` |

### Current Confusion

```
/ (Command Root)
├── /mcp              ← All tools lumped together!
│   ├── fs            ← Should be top-level
│   ├── git           ← Should be top-level
│   ├── shell         ← Should be top-level
│   ├── system        ← Should be top-level
│   ├── linear        ← Correctly under /mcp
│   └── postgres      ← Correctly under /mcp
```

Users typing `/mcp/fs/read` feels unnatural - reading files is a **system capability**, not a "plugin".

## Solution: Two-Tier Tool Architecture

### Target Command Tree

```
/ (Root)
├── /fs           [Tier 1: System] → aleph-fs
│   ├── read
│   ├── write
│   └── list
├── /git          [Tier 1: System] → aleph-git
│   ├── status
│   ├── commit
│   └── diff
├── /sys          [Tier 1: System] → aleph-sys
│   ├── info
│   └── processes
├── /shell        [Tier 1: System] → aleph-shell
│   └── run
├── /mcp          [Tier 2: Extensions] → User-installed external servers
│   ├── linear
│   ├── brave-search
│   └── postgres
├── /search       [Tier 1: Capability] → Built-in search
├── /video        [Tier 1: Capability] → Built-in video
└── /skill        [Tier 1: Skills] → Prompt templates
```

### Tier Definitions

| Tier | Name | Characteristics | Examples |
|------|------|-----------------|----------|
| **Tier 1** | System Built-ins | Always available, native Rust code, no installation required, top-level commands | `/fs`, `/git`, `/sys`, `/shell`, `/search`, `/video` |
| **Tier 2** | MCP Extensions | User-installed, external processes, may need API keys/runtimes, under `/mcp/` namespace | `/mcp/linear`, `/mcp/postgres`, `/mcp/brave-search` |

### Why This Architecture?

1. **Native First**: System tools feel like shell commands, not plugins
2. **Clean Namespace**: External extensions grouped under `/mcp/` prevent pollution
3. **User Expectation**: `/fs/read` is intuitive, `/mcp/fs/read` is clunky
4. **Security Model**: Tier 1 = trusted core, Tier 2 = sandboxed extensions

## Design Decisions

### D1: Trigger Command Naming
- **Chosen**: `/fs`, `/git`, `/sys`, `/shell` (short, shell-like)
- **Alternative**: `/file`, `/git`, `/system`, `/exec`
- **Rejected**: `/mcp/builtin/fs` (too verbose)

### D2: Config Section Structure
```toml
[tools]                  # Tier 1: System Built-ins
fs_enabled = true
git_enabled = true
shell_enabled = false    # Disabled by default for security
sys_enabled = true

[mcp]                    # Tier 2: MCP Extensions (external servers only)
enabled = true
[[mcp.servers]]
name = "linear"
command = "npx"
args = ["-y", "@anthropic-ai/mcp-server-linear"]
```

### D3: Code Organization
```
services/
├── fs/              ← Foundation implementation
├── git/             ← Foundation implementation
├── system_info/     ← Foundation implementation
└── tools/           ← Tool adapters (MCP-like JSON interface)
    ├── mod.rs
    ├── fs_tool.rs       ← Wraps services/fs for LLM
    ├── git_tool.rs      ← Wraps services/git for LLM
    ├── shell_tool.rs    ← Shell execution
    └── sys_tool.rs      ← Wraps services/system_info for LLM

mcp/                 ← Pure MCP protocol (Tier 2 only)
├── external/        ← External server management
├── client.rs        ← JSON-RPC client
└── transport/       ← stdio/SSE transport
```

## Scope

### In Scope
- Restructure command tree: move `/mcp/fs`, `/mcp/git`, etc. to top-level
- Rename config section `[mcp.builtin]` → `[tools]`
- Update `trigger_command` from `/mcp/fs` → `/fs`
- Update UI labels: "Built-in Services" → "System Tools"
- Keep MCP protocol code unchanged (only for external servers)

### Out of Scope
- Changing the actual tool implementations (fs, git, shell, sys)
- Adding new tools
- Changing MCP JSON-RPC protocol

## Success Criteria

1. Command tree shows `/fs`, `/git`, `/sys`, `/shell` at top level
2. `/mcp/` only contains user-installed external servers
3. Config uses `[tools]` for system tools, `[mcp]` for extensions
4. UI Settings shows "System Tools" and "MCP Extensions" sections
5. All existing functionality preserved

## Migration

### Config Migration
```rust
// Auto-migrate old config
if config.mcp.builtin.is_some() {
    config.tools = config.mcp.builtin.take();
    warn!("[mcp.builtin] is deprecated, migrated to [tools]");
}
```

### Command Migration
| Old | New |
|-----|-----|
| `/mcp/fs/read` | `/fs/read` |
| `/mcp/git/status` | `/git/status` |
| `/mcp/shell/run` | `/shell/run` |
| `/mcp/system/info` | `/sys/info` |
