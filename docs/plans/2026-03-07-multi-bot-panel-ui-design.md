# Multi-Bot Panel UI Design

> Date: 2026-03-07
> Status: Approved
> Depends on: [Multi-Bot Channel Support](2026-03-07-multi-bot-channel-design.md) (backend, completed)

## Overview

Adapt the Panel UI to support multiple bot instances per social platform. Two changes: overview page gains instance count badges, detail pages become master-detail layouts with instance list sidebar.

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Detail layout | Left sidebar (instances) + right panel (config) | User preference, natural master-detail pattern |
| Instance naming | User inputs id on creation | Id appears in session keys and logs, meaningful names help |
| Instance treatment | All equal, no "default" label | Simplicity, no special cases |
| Custom pages | Discord keeps custom UI, others use template | Telegram/WhatsApp/iMessage custom pages are unused legacy |
| Legacy cleanup | Delete telegram.rs, whatsapp.rs, imessage.rs | Not referenced by routes |

## Overview Page Changes

Existing 13-platform card grid stays. Each card gains an instance count badge.

Data flow: `channels.list` RPC returns all instances. Group by `channel_type`, count per platform, display `(N)` next to card name. Hide badge if count is 0.

## Detail Page: Master-Detail Layout

Clicking a platform card navigates to `/settings/channels/{platform_type}`.

```
+-----------------------------------------------------------+
|  <- Channels    Telegram                                   |
+---------------+-------------------------------------------+
|               |                                           |
| telegram      |  Connection Status: * Connected           |
| * running     |  [Disconnect]                             |
|               |                                           |
| tg-work       |  Configuration                           |
| o stopped     |  Bot Token: [**********]                  |
|               |  Bot Username: [@my_bot]                  |
| [+ New]       |  Polling Interval: [1000]                 |
|               |  ...                                      |
|               |                                           |
|               |  [Save]  [Delete Instance]                |
+---------------+-------------------------------------------+
```

- **Left sidebar** (~200px): instance list with status dots, "+ New" button at bottom
- **Right panel**: selected instance config form (template-driven or Discord custom UI)
- Default selection: first instance in list
- Empty state: if no instances, show "+ Create your first bot" prompt

## Data Flow & RPC

### Overview page

1. Call `channels.list` -> group by `channel_type` -> count per platform -> badge

### Detail page

1. **Load instances**: `channels.list`, filter by `channel_type == platform_type`
2. **Load config**: `config.get { section: "channels.{instance_id}" }`
3. **Switch instance**: click sidebar item -> re-fetch config for new id
4. **Save config**: `config.patch { path: "channels.{instance_id}", patch: {...} }`
5. **Create instance**: input id -> `channel.create { id, type, config }` -> refresh list
6. **Delete instance**: confirm dialog -> `channel.delete { id }` -> refresh list, select first
7. **Start/Stop**: `channel.start` / `channel.stop` with instance id

### Signals

```rust
let platform_type: String;                          // from URL path
let instances: RwSignal<Vec<ChannelInstanceInfo>>;  // [{id, status}]
let selected_id: RwSignal<Option<String>>;          // currently selected
let field_values: RwSignal<serde_json::Map>;        // selected instance config
```

## Components

### New

| Component | File | Responsibility |
|-----------|------|----------------|
| `ChannelPlatformPage` | `platform_page.rs` | Top-level detail page, left-right split layout |
| `InstanceSidebar` | `platform_page.rs` | Left panel: instance list + status + new button |
| `NewInstanceDialog` | `platform_page.rs` | Inline form for creating instance (id input) |

### Modified

| Component | Change |
|-----------|--------|
| `ChannelConfigTemplate` | New `instance_id: String` prop; config path becomes `channels.{instance_id}`; add Delete button |
| `DiscordChannelView` | New `instance_id: String` prop for multi-instance |
| `ChannelsOverview` | Group `channels.list` by type, pass count to cards |
| `ChannelCard` | New optional `count: Option<usize>` prop for badge |

### Routing changes (app.rs)

Replace 13 per-platform wrapper components with single dynamic route:

```rust
// Before: 13 separate routes
"/settings/channels/telegram" => view! { <TelegramConfigPage /> },
"/settings/channels/discord" => view! { <DiscordChannelView /> },
// ...

// After: single parameterized route
"/settings/channels/:platform_type" => view! { <ChannelPlatformPage /> },
```

`ChannelPlatformPage` reads `platform_type` from URL, looks up `ChannelDefinition` from `ALL_CHANNELS`, renders `InstanceSidebar` + right panel (Discord custom or template).

## Files

### New

| File | Description |
|------|-------------|
| `apps/panel/src/views/settings/channels/platform_page.rs` | ChannelPlatformPage + InstanceSidebar + NewInstanceDialog |

### Modified

| File | Change |
|------|--------|
| `apps/panel/src/views/settings/channels/config_template.rs` | Add `instance_id` prop, dynamic config path, Delete button |
| `apps/panel/src/views/settings/channels/overview.rs` | Instance count badge on cards |
| `apps/panel/src/views/settings/channels/discord.rs` | Add `instance_id` prop |
| `apps/panel/src/views/settings/channels/mod.rs` | Add `platform_page` module, remove old exports |
| `apps/panel/src/app.rs` | Unified route, delete 13 wrapper components |

### Deleted

| File | Reason |
|------|--------|
| `apps/panel/src/views/settings/channels/telegram.rs` | Legacy, not referenced by routes |
| `apps/panel/src/views/settings/channels/whatsapp.rs` | Legacy, not referenced by routes |
| `apps/panel/src/views/settings/channels/imessage.rs` | Legacy, not referenced by routes |

## Out of Scope (YAGNI)

- Instance drag-to-reorder
- Instance rename (id is immutable after creation)
- Batch start/stop
- Config copy between instances
