# CLI Infrastructure Enhancement Design

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Complete the Aleph CLI foundation — fill in missing RPC methods, add config management, daemon control, and shell completion so the CLI is truly usable end-to-end.

**Architecture:** Layered extension (Gateway RPC → CLI command → TUI slash command). Each module is end-to-end complete. CLI remains a pure protocol client per R4.

**Tech Stack:** Rust (alephcore + aleph-cli), JSON-RPC 2.0, clap + clap_complete, aleph-protocol

---

## Background

After the TUI upgrade (ratatui split-screen, 17 slash commands, markdown rendering, 101 tests), the CLI has a polished UI but several slash commands are stubs (`/usage`, `/memory`, `/compact`). Additionally, essential infrastructure commands (config, daemon, shell completion) are missing.

This design fills these gaps with ~820 lines across 6 new files and 5 modified files.

## Module 1: Config Management

### Gateway RPC (core/)

Three new RPC methods in `core/src/gateway/handlers/config_mgmt.rs`:

**`config.get`**
```json
Request:  { "path": "gateway.port" }       // optional, empty = full config
Response: { "value": 18789 }               // or full config JSON
```

**`config.set`**
```json
Request:  { "path": "gateway.port", "value": 18790 }
Response: { "success": true, "previous": 18789 }
```
Writes to disk via `config::save`. Values parsed as JSON literals (true, 18789, "string").

**`config.validate`**
```json
Request:  {}
Response: { "valid": true, "errors": [] }
```
Uses existing `config::validate` module.

### CLI Commands (apps/cli/)

New file: `commands/config_cmd.rs` (~120 lines)

```bash
aleph config file              # Print config file path (local, no RPC)
aleph config get [PATH]        # Get config value (no PATH = all)
aleph config set PATH VALUE    # Set config value
aleph config validate          # Validate config
```

Path format: dot-separated (e.g., `gateway.port`, `channels.telegram.enabled`).

## Module 2: Usage & Memory & Compact

### Gateway RPC — New

**`usage.current`** — New file: `handlers/usage.rs` (~80 lines)
```json
Request:  { "session_key": "chat-abc123" }
Response: {
  "session_tokens": { "input": 12345, "output": 6789, "total": 19134 },
  "model": "claude-3-sonnet",
  "messages": 42,
  "tools_used": 7
}
```
Source: SessionManager message history, estimate tokens from message content length.

**`sessions.compact`** — Added to `handlers/session.rs` (~80 lines)
```json
Request:  { "session_key": "chat-abc123" }
Response: { "success": true, "before_messages": 42, "after_messages": 15, "tokens_saved": 8000 }
```
Strategy: Keep recent N messages + generate summary prefix for older messages. Initial version uses simple truncation with summary header.

### Gateway RPC — Already Implemented

- `memory.search` — Hybrid vector + FTS search (handlers/memory.rs)
- `memory.stats` — Memory statistics (handlers/memory.rs)

### CLI/TUI Side

Replace stubs in `tui/mod.rs` `execute_slash_command()`:

- `/usage` → calls `usage.current` RPC, displays token stats
- `/memory <query>` → calls `memory.search` RPC, displays results
- `/compact` → calls `sessions.compact` RPC, displays before/after

## Module 3: Daemon Control

### Gateway RPC — New

New file: `handlers/daemon_status.rs` (~120 lines)

**`daemon.status`**
```json
Request:  {}
Response: {
  "uptime_secs": 86400,
  "version": "0.1.0",
  "connections": 3,
  "sessions_active": 5,
  "memory_mb": 120,
  "config_path": "/Users/xxx/.aleph/aleph.toml"
}
```
Source: GatewayServer connections map, SessionManager, std::process.

**`daemon.shutdown`**
```json
Request:  { "graceful": true }
Response: { "status": "shutting_down" }
```
Sets shutdown flag, waits for active requests, closes listener.

**`daemon.logs`**
```json
Request:  { "lines": 50, "level": "warn" }
Response: { "logs": ["2026-03-04T12:00:00Z WARN ...", ...] }
```
Reads from aleph-logging log directory (~/.aleph/logs/).

### CLI Commands

New file: `commands/daemon.rs` (~180 lines)

```bash
aleph daemon status            # Show Gateway status
aleph daemon start             # Start Gateway service
aleph daemon stop              # Stop Gateway service
aleph daemon restart           # Restart Gateway service
aleph daemon logs [--lines N] [--level LEVEL]  # View logs
```

**start implementation:**
- macOS: `launchctl bootstrap` (if launchd plist exists) or `std::process::Command::new("aleph").arg("serve").spawn()`
- Writes PID to `~/.aleph/aleph.pid`

**stop implementation:**
1. Try RPC `daemon.shutdown` via WebSocket
2. Fallback: Read PID file, send SIGTERM
3. Verify process exited

**status implementation:**
1. Try WebSocket connection → call `daemon.status` RPC → display rich info
2. Connection fails → check PID file / process existence → report "not running"

## Module 4: Shell Completion

New dependency: `clap_complete = "4"` in Cargo.toml.

New file: `commands/completion.rs` (~30 lines)

```bash
aleph completion bash           # Output bash completion script
aleph completion zsh            # Output zsh completion script
aleph completion fish           # Output fish completion script
```

Pure local operation using `clap_complete::generate()`. No Gateway connection needed.

## File Structure

### Gateway (core/)

```
core/src/gateway/handlers/
├── config_mgmt.rs     (NEW)  config.get/set/validate      ~150 lines
├── usage.rs           (NEW)  usage.current                 ~80 lines
├── daemon_status.rs   (NEW)  daemon.status/shutdown/logs   ~120 lines
├── session.rs         (MOD)  add handle_compact            ~80 lines
└── mod.rs             (MOD)  register 6 new RPC methods
```

### CLI (apps/cli/)

```
apps/cli/
├── Cargo.toml         (MOD)  add clap_complete
├── src/
│   ├── main.rs        (MOD)  add Config/Daemon/Completion commands
│   ├── commands/
│   │   ├── config_cmd.rs  (NEW)  config get/set/file/validate  ~120 lines
│   │   ├── daemon.rs      (NEW)  daemon status/start/stop/logs ~180 lines
│   │   ├── completion.rs  (NEW)  shell completion generation   ~30 lines
│   │   └── mod.rs         (MOD)  export new modules
│   └── tui/
│       └── mod.rs     (MOD)  replace /usage /memory /compact stubs ~60 lines
```

### Estimated Total

| Location | New Files | Modified | Lines |
|----------|-----------|----------|-------|
| Gateway handlers | 3 | 2 | ~430 |
| CLI commands | 3 | 2 | ~330 |
| CLI TUI | — | 1 | ~60 |
| **Total** | **6** | **5** | **~820** |

## Testing Strategy

- Gateway handlers: Unit tests with mock SessionManager/MemoryStore
- CLI commands: Compilation tests (pure RPC calls, no complex logic)
- TUI integration: Existing 101 tests remain passing + new tests for RPC param construction

## Architecture Compliance

| Red Line | Status |
|----------|--------|
| R1: Brain-Limb Separation | ✅ CLI depends only on aleph-protocol |
| R4: I/O-Only Interface | ✅ All logic in Gateway, CLI is pure I/O |
| P1: Low Coupling | ✅ New handlers follow existing registry pattern |
| P6: Simplicity | ✅ ~820 lines total, no over-engineering |
