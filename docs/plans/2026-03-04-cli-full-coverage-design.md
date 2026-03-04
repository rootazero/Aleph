# CLI Full RPC Coverage Design

> Date: 2026-03-04
> Status: Approved
> Scope: Cover remaining 29 Gateway RPC methods with CLI commands + Gateway handler wiring

## Context

Phase 1 covered ~40 high-frequency RPC methods as CLI commands. Phase 2 fixed bugs and unified --json. This phase covers the remaining 29 non-placeholder RPC methods across 7 command groups.

Excluded: `cron.*` and `group_chat.*` (placeholder stubs only), `commands.list` (TUI-specific, use `gateway call`).

## New CLI Commands

### 1. chat — Chat Control (4 methods)

```
aleph chat send <message> [--session <key>] [--stream] [--thinking <level>]
aleph chat abort <run-id>
aleph chat history <session-key> [--limit N] [--before <id>]
aleph chat clear <session-key> [--keep-system]
```

Maps to: `chat.send`, `chat.abort`, `chat.history`, `chat.clear`

Note: Existing `aleph chat` (interactive) and `aleph ask` (one-shot) remain unchanged. These subcommands expose direct RPC operations.

### 2. session — Extended (2 methods)

```
aleph session usage <key>
aleph session compact <key>
```

Maps to: `session.usage`, `session.compact`

Added to existing `SessionAction` enum.

### 3. poe — POE Execution Engine (8 methods)

```
aleph poe run <instruction> [--manifest <json>] [--stream]
aleph poe status <task-id>
aleph poe cancel <task-id>
aleph poe list
aleph poe prepare <instruction>
aleph poe sign <contract-id>
aleph poe reject <contract-id>
aleph poe pending
```

Maps to: `poe.run`, `poe.status`, `poe.cancel`, `poe.list`, `poe.prepare`, `poe.sign`, `poe.reject`, `poe.pending`

### 4. services — Background Service Management (4 methods)

```
aleph services list [--plugin <id>] [--state <state>]
aleph services status <plugin-id> <service-id>
aleph services start <plugin-id> <service-id>
aleph services stop <plugin-id> <service-id>
```

Maps to: `services.list`, `services.status`, `services.start`, `services.stop`

### 5. identity — Soul/Identity Management (4 methods)

```
aleph identity get
aleph identity set <manifest-json>
aleph identity clear
aleph identity list
```

Maps to: `identity.get`, `identity.set`, `identity.clear`, `identity.list`

### 6. vault — Key Management (4 methods)

```
aleph vault status
aleph vault store         # Interactive key input (rpassword), stdin for --json
aleph vault delete
aleph vault verify
```

Maps to: `vault.status`, `vault.storeKey`, `vault.deleteKey`, `vault.verify`

Security: `vault store` never accepts master key as CLI argument (shell history leak). Uses `rpassword::read_password()` for interactive input, stdin for `--json` mode.

### 7. mcp — MCP Approval Workflow (3 methods)

```
aleph mcp pending
aleph mcp approve <request-id> [--reason <text>]
aleph mcp reject <request-id> [--reason <text>]
aleph mcp cancel <request-id>
```

Maps to: `mcp.list_pending_approvals`, `mcp.respond_approval` (approved=true/false), `mcp.cancel_approval`

## Gateway Handler Wiring

### Stateless (register in HandlerRegistry::new())

- `services.start` → `services::handle_start`
- `services.stop` → `services::handle_stop`
- `services.list` → `services::handle_list`
- `services.status` → `services::handle_status`
- `vault.status` → `vault_config::handle_status`
- `vault.storeKey` → `vault_config::handle_store_key`
- `vault.deleteKey` → `vault_config::handle_delete_key`
- `vault.verify` → `vault_config::handle_verify`

### Stateful (register at startup)

- `identity.get` → `identity::handle_get` (needs `SharedIdentityResolver`)
- `identity.set` → `identity::handle_set` (needs `SharedIdentityResolver`)
- `identity.clear` → `identity::handle_clear` (needs `SharedIdentityResolver`)
- `identity.list` → `identity::handle_list` (needs `SharedIdentityResolver`)

## File Structure

### New Files

```
apps/cli/src/commands/
├── chat_cmd.rs        # chat send/abort/history/clear
├── poe_cmd.rs         # 8 POE subcommands
├── services_cmd.rs    # services list/status/start/stop
├── identity_cmd.rs    # identity get/set/clear/list
├── vault_cmd.rs       # vault status/store/delete/verify
├── mcp_cmd.rs         # mcp pending/approve/reject/cancel
```

### Modified Files

```
apps/cli/src/commands/mod.rs                           # add 6 pub mod
apps/cli/src/commands/session.rs                       # extend with usage + compact
apps/cli/src/main.rs                                   # add 6 enums + dispatch
core/src/gateway/handlers/mod.rs                       # register services.*, vault.*
core/src/bin/aleph/commands/start/builder/handlers.rs  # register_identity_handlers()
core/src/bin/aleph/commands/start/mod.rs               # call register_identity_handlers
```

## Tasks

1. Wire Gateway handlers (services, vault, identity)
2. Add chat command (send, abort, history, clear)
3. Extend session with usage + compact
4. Add poe command (8 subcommands)
5. Add services command (list, status, start, stop)
6. Add identity command (get, set, clear, list)
7. Add vault command (status, store, delete, verify)
8. Add mcp command (pending, approve, reject, cancel)
9. Update main.rs dispatch for all new commands
