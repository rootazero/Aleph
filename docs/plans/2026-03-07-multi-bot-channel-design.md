# Multi-Bot Channel Support Design

> Date: 2026-03-07
> Status: Approved

## Overview

Allow each social platform (Telegram, Discord, WhatsApp, etc.) to configure multiple bot instances. Each instance is independent with its own identity, session isolation, and agent routing.

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Config format | `[channels.<id>]` with `type` field | Fits existing `HashMap<String, Value>`, minimal migration |
| Backward compat | Auto-infer `type` from key if missing | Old configs work without modification |
| Agent binding | Default to main, switchable at runtime | Flexible; `default_agent` deferred to future |
| `aleph.toml` | Keep as single-instance fallback | Minimize change surface |
| Panel UI | Platform card → instance list inside | Clean hierarchy, avoids card sprawl |
| Implementation | Minimal — config parsing + startup logic only | ChannelRegistry already supports multi-instance |

## Configuration Format

### New format (multi-instance)

```toml
[channels.telegram-main]
type = "telegram"
bot_token = "123:ABC..."
allowed_users = [111, 222]

[channels.telegram-work]
type = "telegram"
bot_token = "456:DEF..."
allowed_users = [333]

[channels.discord-gaming]
type = "discord"
bot_token = "xyz..."
```

### Old format (backward compatible)

```toml
# key is a known platform name, no type field → type auto-inferred from key
[channels.telegram]
bot_token = "123:ABC..."
```

### Type inference rules

1. If `type` field present → use it
2. If no `type` field and key is a known platform name → infer `type = key`
3. Neither → warn and skip

Known platform names: `telegram`, `discord`, `whatsapp`, `slack`, `imessage`, `email`, `matrix`, `signal`, `mattermost`, `irc`, `webhook`, `xmpp`, `nostr`

## Data Structures

### New: `ChannelInstanceConfig`

```rust
/// Resolved channel instance from config
pub struct ChannelInstanceConfig {
    pub id: String,              // HashMap key (instance id)
    pub channel_type: String,    // explicit type or inferred from key
    pub config: serde_json::Value, // remaining config (type field stripped)
}
```

### Unchanged: `Config.channels`

Storage remains `HashMap<String, serde_json::Value>`. New method added:

```rust
impl Config {
    /// Parse channels config into resolved instances with type inference
    pub fn resolved_channels(&self) -> Vec<ChannelInstanceConfig> { ... }
}
```

## Startup Logic

### Current (hardcoded)

Each platform has a dedicated code block in `initialize_channels` creating exactly one instance with a fixed id.

### New (dynamic)

```rust
let instances = app_config.resolved_channels();

// Merge aleph.toml fallback: if no instance of a type exists in config.toml
// but aleph.toml has config for that type, create a fallback instance

for inst in &instances {
    let channel = create_channel_from_config(&inst.id, &inst.channel_type, &inst.config);
    if let Some(mut ch) = channel {
        if inst.channel_type == "telegram" {
            // inject slash commands (shared across all telegram instances)
        }
        channel_registry.register(ch).await;
    }
}
```

### `create_channel_from_config`

Already exists in `channel.rs` handler for `channel.start` RPC recreation. Extract as public function and reuse at startup.

### iMessage handling

- If config has `type = "imessage"` instance → create on macOS
- If no imessage instance and platform is macOS → auto-create default (preserves current behavior)

## RPC Changes

### Existing (no changes needed)

- `channel.start { id }` — already re-reads config and recreates channel
- `channel.stop { id }` — works with any channel id
- `channels.list` — returns all registered channels
- `channels.status { id }` — works with any channel id

### New RPCs

#### `channel.create`

```json
{
    "method": "channel.create",
    "params": {
        "id": "telegram-work",
        "type": "telegram",
        "config": { "bot_token": "456:DEF...", "allowed_users": [333] }
    }
}
```

Writes to `Config.channels`, creates channel instance, registers, and auto-starts.

#### `channel.delete`

```json
{
    "method": "channel.delete",
    "params": { "id": "telegram-work" }
}
```

Stops channel, removes from registry, removes from `Config.channels`.

## Session Routing

**No changes needed.** `SessionKey.channel` is already a `String` populated from `Channel::id()`. Multi-instance automatically produces isolated session keys:

```
agent:main:telegram-main:dm:user123
agent:main:telegram-work:dm:user123
```

## Panel UI

### Overview page (grid)

Unchanged layout. Each platform card gains an instance count badge (e.g., "Telegram (2)").

### Platform detail page

Changes from single-instance form to instance list:

- Instance list with status indicators (running/stopped)
- Per-instance actions: Edit, Start/Stop, Delete
- "New Instance" button → form with instance id + platform config fields
- Edit uses existing `config_template.rs` generic form renderer

Single-instance platforms look the same — list contains one item.

## Files to Modify

| File | Change |
|------|--------|
| `core/src/config/structs.rs` | Add `ChannelInstanceConfig`, `resolved_channels()` method |
| `core/src/bin/aleph/commands/start/mod.rs` | Rewrite `initialize_channels` to iterate `resolved_channels()` |
| `core/src/gateway/handlers/channel.rs` | Extract `create_channel_from_config` as public; add `channel.create`/`channel.delete` handlers |
| `apps/panel/src/views/settings/channels/overview.rs` | Instance count badge on cards |
| `apps/panel/src/views/settings/channels/mod.rs` | Platform detail page → instance list UI |

## Files NOT Modified

| File | Reason |
|------|--------|
| `core/src/gateway/channel.rs` | Channel trait / ChannelRegistry already supports multi-instance |
| `core/src/gateway/channel_registry.rs` | `HashMap<ChannelId, Handle>` — no changes needed |
| `core/src/routing/session_key.rs` | `channel` field is String, auto-adapts |
| `core/src/gateway/config.rs` | `aleph.toml` stays as single-instance fallback |
| Platform Channel implementations | Not changed, just instantiated multiple times |

## Out of Scope (YAGNI)

- `aleph.toml` multi-instance format
- `default_agent` binding per channel instance
- `ChannelFactory` registration pattern
- Per-instance slash command customization
