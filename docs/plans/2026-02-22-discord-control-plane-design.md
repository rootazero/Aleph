# Discord Control Plane Panel Design

**Date**: 2026-02-22
**Status**: Approved
**Phase**: 8.3 — Discord Control Plane Integration

---

## Overview

Extend the Discord channel (Phase 8.2) with a full Control Plane management panel: Token validation, Bot identity display, Guild/Channel management, and permission audit dashboard. Architecture follows "lightweight frontend, heavyweight backend" — all Discord API interactions happen in Rust, exposed via RPC to the Leptos WASM panel.

---

## Architecture: Lightweight Frontend, Heavyweight Backend

### Design Principles

- **Server-side execution**: All Discord API calls (validate token, list guilds, audit permissions) run in Rust backend
- **RPC bridge**: 6 new JSON-RPC methods expose Discord management capabilities to the Control Plane
- **Reuse serenity**: Leverage existing `serenity::Http` client for REST API calls, no new HTTP dependencies
- **Token security**: Token never leaves backend; frontend displays masked value only

### Why This Approach

| Alternative | Rejected Because |
|-------------|-----------------|
| Minimal enhancement (method B) | Cannot dynamically configure Guild/Channel listening from panel |
| Full WebSocket push (method C) | Over-engineered — permission/Guild changes are infrequent; manual refresh suffices |

---

## Backend: RPC Interface Design

### New RPC Methods

| Method | Params | Returns | Purpose |
|--------|--------|---------|---------|
| `discord.validate_token` | `{ token: String }` | `{ valid, bot_id, bot_name, bot_avatar, discriminator }` | Live Token validation, returns Bot identity |
| `discord.save_config` | `{ token, application_id?, ... }` | `{ success }` | Save Discord config to config.toml with hot-reload |
| `discord.list_guilds` | `{ channel_id }` | `[{ guild_id, name, icon, member_count, bot_permissions }]` | Fetch all Guilds the Bot has joined |
| `discord.list_channels` | `{ channel_id, guild_id }` | `[{ channel_id, name, type, position, permissions }]` | Fetch channel tree for a given Guild |
| `discord.audit_permissions` | `{ channel_id, guild_id }` | `{ permissions: [{ name, has, required, status }] }` | Check Bot permissions in a Guild with traffic lights |
| `discord.update_allowlists` | `{ channel_id, guilds: [], channels: [] }` | `{ success }` | Update monitored Guild/Channel allowlists |

### Permission Audit Model

```rust
pub struct PermissionAudit {
    pub guild_id: u64,
    pub guild_name: String,
    pub permissions: Vec<PermissionCheck>,
    pub overall_status: HealthStatus,  // Green / Yellow / Red
}

pub struct PermissionCheck {
    pub name: String,        // e.g. "Send Messages"
    pub discord_flag: u64,   // Discord permission bitfield
    pub has: bool,           // Whether Bot has this permission
    pub required: bool,      // Whether Aleph requires it
    pub status: TrafficLight, // Green / Yellow / Red
}

pub enum TrafficLight { Green, Yellow, Red }
```

### Permission Checklist

| Permission | Level | Status When Missing |
|------------|-------|-------------------|
| Send Messages | Required | Red |
| Read Messages | Required | Red |
| View Channels | Required | Red |
| Embed Links | Recommended | Yellow |
| Attach Files | Recommended | Yellow |
| Read Message History | Recommended | Yellow |
| Manage Messages | Optional | Green |
| Add Reactions | Optional | Green |
| Use Slash Commands | Optional | Green |

---

## Frontend: Control Plane Panel

### Panel Layout

```
┌─────────────────────────────────────────────────────────────┐
│  Discord Channel                                             │
├─────────────────────────────────────────────────────────────┤
│  Bot Identity                                    [Online]    │
│  Avatar  BotName#1234                                        │
│  ID: 123456789  Ping: 42ms                                   │
├─────────────────────────────────────────────────────────────┤
│  Token Configuration                                         │
│  [••••••••••••••••••••] [Validate] [Reset]                   │
│  Status: Valid                                                │
├─────────────────────────────────────────────────────────────┤
│  Guild Management                              [Refresh]     │
│  ┌──────────────────┬──────────────────────────────────────┐ │
│  │ Guilds           │ Channels                              │ │
│  │ [x] My Server  G │ [x] #general     G                   │ │
│  │ [ ] Test Guild Y │ [x] #ai-chat     G                   │ │
│  │                  │ [ ] #random      G                   │ │
│  └──────────────────┴──────────────────────────────────────┘ │
├─────────────────────────────────────────────────────────────┤
│  Permission Audit                              [Re-scan]     │
│  My Server:                                                  │
│   G Send Messages        Has                                 │
│   G Read Messages        Has                                 │
│   G View Channels        Has                                 │
│   Y Embed Links          Missing                             │
│   G Attach Files         Has                                 │
│   G Read History         Has                                 │
│                                                              │
│   Overall: Y Functional (1 recommendation)                   │
│   [Fix: Enable "Embed Links" in Server Settings]             │
└─────────────────────────────────────────────────────────────┘
```

(G = Green, Y = Yellow, R = Red)

### Panel Sections

| Section | Function | Data Source |
|---------|----------|-------------|
| **Bot Identity** | Bot name, avatar, ID, online status, latency | `discord.validate_token` + `channel.status` |
| **Token Configuration** | Masked input, validate button, reset button | `discord.validate_token` + `discord.save_config` |
| **Guild Management** | Dual-column: Guilds list (checkbox) + Channel tree (checkbox) | `discord.list_guilds` + `discord.list_channels` + `discord.update_allowlists` |
| **Permission Audit** | Per-Guild permission checks with traffic lights + fix guidance | `discord.audit_permissions` |

### Interaction Flow

```
User opens Discord panel
  |
  +-- Bot not configured --> Show "Enter Token" guidance
  |   +-- Enter Token --> call discord.validate_token
  |   +-- Validation success --> Show Bot identity --> Auto-fetch Guilds
  |   +-- Validation failure --> Show error + fix hint
  |
  +-- Bot configured --> Show full panel
      +-- Fetch Guild list --> Render dual-column selector
      +-- User toggles Guild/Channel --> call discord.update_allowlists
      +-- Click "Re-scan" --> call discord.audit_permissions --> Update traffic lights
```

---

## File Structure

### Backend (new/modified)

```
core/src/gateway/
├── handlers/
│   └── discord_panel.rs         # NEW: 6 Discord panel RPC handlers
├── channels/discord/
│   ├── mod.rs                   # MODIFY: add guild/permission query methods
│   ├── config.rs                # MODIFY: add validate/save/hot-reload support
│   ├── permissions.rs           # NEW: permission audit logic
│   └── api.rs                   # NEW: Discord REST API wrapper (guilds, channels, permissions)
```

### Frontend (new/modified)

```
core/ui/control_plane/src/
├── views/
│   └── channels/
│       ├── mod.rs               # NEW: channel management routes
│       └── discord.rs           # NEW: Discord panel components
├── components/
│   ├── traffic_light.rs         # NEW: traffic light indicator component
│   └── dual_list.rs             # NEW: dual-column list selector component
├── app.rs                       # MODIFY: add Discord route
└── api.rs                       # MODIFY: add discord.* RPC calls
```

---

## Key Implementation Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Discord API calls | Reuse serenity `Http` client | Already have Token and type definitions |
| Config persistence | Hot-reload to config.toml | Consistent with existing config system |
| Permission check frequency | Manual trigger (Re-scan button) | Permission changes are infrequent |
| Guild/Channel cache | In-memory + manual refresh | Simple and reliable, Refresh button covers stale data |
| Token storage | config.toml (Keychain upgrade later) | Phase 1 simplicity |

---

## Security

| Measure | Implementation |
|---------|---------------|
| **Transport** | Token over WebSocket (localhost only) |
| **Storage** | config.toml with 600 file permissions |
| **Display** | Frontend shows `••••` + last 4 chars; full Token never returned |
| **Validation** | Only on explicit user action, no auto-polling |

---

## Error Handling

| Scenario | Handling |
|----------|----------|
| Invalid Token | Clear error message: "Invalid token. Check your bot token in Discord Developer Portal." |
| Token revoked | Bot disconnects; panel shows Error + "Token may have been revoked" |
| Discord API rate limit | Return 429 + Retry-After; frontend shows countdown |
| Guild query failure | Degrade to showing configured Guild IDs from config |
| Bot lacks Guild access | Skip that Guild; panel shows "insufficient permissions" |
| Network failure | 3 retries + exponential backoff; panel shows "Connecting..." |

---

## Testing Strategy

| Level | Coverage |
|-------|----------|
| **Unit tests** | Permission audit logic, config validation, traffic light computation |
| **Integration tests** | RPC handler request/response format (mock serenity Http) |
| **Manual tests** | End-to-end with real Discord Bot |

---

## Out of Scope (Future Iterations)

These features are explicitly excluded from this iteration and recorded as candidates for future work:

- Slash Commands visual configuration and registration
- Rich Embed editor and preview
- Interaction buttons/menus mapping to Aleph Skills
- OAuth2 invite link generator
- Debug console (raw Gateway JSON stream)
- Multi-Bot load balancing
- Voice channel integration
- WebSocket real-time event push to Control Plane
- Thread mode toggle per channel

---

## Success Criteria

- [ ] `discord.validate_token` RPC validates token and returns Bot identity
- [ ] `discord.list_guilds` returns all Guilds with Bot permissions
- [ ] `discord.list_channels` returns channel tree for a Guild
- [ ] `discord.audit_permissions` returns traffic-light permission status
- [ ] `discord.update_allowlists` persists Guild/Channel selection to config
- [ ] Control Plane shows Bot identity section with avatar and status
- [ ] Control Plane allows Token input with masked display
- [ ] Control Plane renders dual-column Guild/Channel selector
- [ ] Control Plane displays permission audit with traffic lights
- [ ] Unit tests cover permission audit logic
- [ ] Integration tests cover all 6 RPC handlers
