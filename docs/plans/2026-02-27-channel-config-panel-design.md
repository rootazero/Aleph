# Channel Configuration Panel Design

## Overview

Add visual configuration UI in the Aleph Control Panel (Leptos/WASM) for all 13 social bot channels. Replace the current 4 isolated channel settings pages with a unified, template-driven architecture: a **Channels Overview** page (card grid) + a **generic ChannelConfigTemplate** component that auto-renders config forms from static channel definitions.

## Decision Record

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Feature depth | Basic config + connection test | Matches "Telegram-style" simplicity; sufficient for all channels |
| RPC strategy | Unified `config.patch` | DRY; no per-channel RPC handlers needed |
| Navigation | Single "Channels" sidebar entry → Overview page | Scales to 13+ channels without sidebar bloat |
| UI architecture | Template-driven (Option A) | 13 channels share same form pattern; DRY; easy to extend |
| Discord exception | Keep existing complex view | Already built with guild/channel selection, permissions audit |

## Architecture

```
┌─ Sidebar ──────────────┐     ┌─ Main Content ──────────────────────────┐
│                        │     │                                          │
│ ▸ Channels ────────────│────▶│  Channels Overview (card grid)           │
│   (single entry)       │     │  ┌──────┐ ┌──────┐ ┌──────┐            │
│                        │     │  │ TG ✓ │ │ DC ✓ │ │ SL ○ │  ...       │
│                        │     │  └──┬───┘ └──────┘ └──────┘            │
│                        │     │     │                                    │
│                        │     │     ▼  click card                       │
│                        │     │  ChannelConfigTemplate                   │
│                        │     │  ┌────────────────────────────────┐     │
│                        │     │  │ Header (icon + name + desc)    │     │
│                        │     │  │ Connection Status + controls   │     │
│                        │     │  │ Config Fields (from definition)│     │
│                        │     │  │ Allowlists (TagList fields)    │     │
│                        │     │  │ [Validate] [Save] [Docs ↗]    │     │
│                        │     │  └────────────────────────────────┘     │
└────────────────────────┘     └──────────────────────────────────────────┘
```

### Data Flow

1. **Overview page** → `channels.list` RPC → card grid with real-time status
2. **Config page load** → `config.get { section: "channels.{id}" }` → populate form fields
3. **Save** → `config.patch { path: "channels.{id}.field", value: "..." }` → TOML write + hot-reload
4. **Connect/Disconnect** → `channel.start` / `channel.stop` RPC
5. **Status updates** → Event bus subscription `channels.status.*` → reactive UI updates

### Routes

| Path | Component | Notes |
|------|-----------|-------|
| `/settings/channels` | `ChannelsOverview` | Card grid, real-time status |
| `/settings/channels/discord` | `DiscordChannelView` | Preserved existing complex view |
| `/settings/channels/:id` | `ChannelConfigPage` | Template-driven, resolves ChannelDefinition by id |

## ChannelDefinition Data Model

```rust
/// Field input type for form rendering
pub enum FieldKind {
    Text,                       // Plain text input
    Secret,                     // Masked input with show/hide toggle
    Url,                        // URL input with format hint
    Number,                     // Numeric input
    Toggle,                     // Boolean switch
    TagList,                    // Tag list editor (add/remove chips)
    Select(&'static [&'static str]),  // Dropdown select
}

/// Single configuration field descriptor
pub struct FieldDef {
    pub key: &'static str,           // TOML key (e.g., "bot_token")
    pub label: &'static str,         // Display label (e.g., "Bot Token")
    pub kind: FieldKind,
    pub placeholder: &'static str,
    pub help: &'static str,          // Help text below field
    pub required: bool,
    pub default_value: &'static str,
}

/// Complete channel definition
pub struct ChannelDefinition {
    pub id: &'static str,            // "telegram", "slack", etc.
    pub name: &'static str,          // "Telegram", "Slack", etc.
    pub description: &'static str,   // Short description
    pub icon_svg: &'static str,      // SVG path data for icon
    pub brand_color: &'static str,   // Hex color (e.g., "#26A5E4")
    pub config_section: &'static str,// TOML section (e.g., "channels.telegram")
    pub fields: &'static [FieldDef],
    pub docs_url: &'static str,      // External documentation link
}
```

### Channel Definitions (13 total)

| Channel | Config Section | Key Fields |
|---------|---------------|------------|
| Telegram | `channels.telegram` | bot_token (Secret), allowed_users (TagList), allowed_groups (TagList) |
| Discord | `channels.discord` | *Uses existing complex view — not in template system* |
| WhatsApp | `channels.whatsapp` | phone_number (Text), session_data (Text), bridge_binary (Text) |
| iMessage | `channels.imessage` | *macOS only*; db_path (Text), target (Text) |
| Slack | `channels.slack` | app_token (Secret), bot_token (Secret), allowed_channels (TagList) |
| Email | `channels.email` | imap_server (Text), imap_port (Number), imap_username (Text), imap_password (Secret), smtp_server (Text), smtp_port (Number), smtp_username (Text), smtp_password (Secret), from_address (Text), use_tls (Toggle), check_interval_secs (Number), allowed_senders (TagList) |
| Matrix | `channels.matrix` | homeserver_url (Url), access_token (Secret), allowed_rooms (TagList), display_name (Text) |
| Signal | `channels.signal` | api_url (Url), phone_number (Text), allowed_users (TagList), trust_mode (Select: always/tofu/manual) |
| Mattermost | `channels.mattermost` | server_url (Url), bot_token (Secret), allowed_channels (TagList), team_name (Text) |
| IRC | `channels.irc` | server (Text), port (Number), nick (Text), password (Secret), channels (TagList), use_tls (Toggle), nickserv_password (Secret) |
| Webhook | `channels.webhook` | secret (Secret), callback_url (Url), path (Text), allowed_senders (TagList) |
| XMPP | `channels.xmpp` | jid (Text), password (Secret), server (Text), port (Number), muc_rooms (TagList), use_tls (Toggle) |
| Nostr | `channels.nostr` | private_key (Secret), relays (TagList), allowed_pubkeys (TagList) |

## UI Components

### New Components

| Component | File | Responsibility |
|-----------|------|---------------|
| `ChannelsOverview` | `views/settings/channels/overview.rs` | Card grid page, fetches status via RPC |
| `ChannelConfigTemplate` | `views/settings/channels/config_template.rs` | Generic config page, renders fields from definition |
| `ChannelConfigPage` | `views/settings/channels/config_page.rs` | Route handler that resolves `:id` → definition → template |
| `ChannelDefinitions` | `views/settings/channels/definitions.rs` | Static `ALL_CHANNELS` array of 13 definitions |
| `SecretInput` | `components/ui/secret_input.rs` | Password input with show/hide eye toggle |
| `TagListInput` | `components/ui/tag_list_input.rs` | Chip-based tag list (add/remove) |
| `StatusBadge` | `components/ui/status_badge.rs` | 5-state connection indicator (color dot + label) |
| `ChannelCard` | `components/ui/channel_card.rs` | Overview card (icon, name, status, action button) |
| `FormField` | `components/ui/form_field.rs` | Field renderer dispatching by FieldKind |

### Modified Files

| File | Change |
|------|--------|
| `views/settings/channels/mod.rs` | Export new modules |
| `components/settings_sidebar.rs` | Replace 4 channel tabs → single `SettingsTab::Channels` entry |
| `app.rs` | Add new routes, remove old per-channel routes (except Discord) |
| `components/ui/mod.rs` | Export new UI components |

### Preserved Files

| File | Reason |
|------|--------|
| `views/settings/channels/discord.rs` | Complex existing implementation with guild/channel selection |
| `views/settings/channels/telegram.rs` | Kept for reference but Overview redirects to template view |
| `views/settings/channels/whatsapp.rs` | Kept for reference but Overview redirects to template view |
| `views/settings/channels/imessage.rs` | Kept for reference but Overview redirects to template view |

## Styling

Follows existing design system tokens:
- `bg-surface`, `bg-surface-raised`, `bg-surface-sunken` — backgrounds
- `text-text-primary`, `text-text-secondary`, `text-text-tertiary` — text colors
- `border-border` — borders
- `bg-primary`, `hover:bg-primary-hover` — action buttons
- `rounded-xl` — card corners
- Brand colors per channel for icons and accents

## Estimated Scope

| Category | Lines |
|----------|-------|
| Template components (config_template, form_field) | ~400 |
| Shared UI components (secret, tag_list, status, card) | ~400 |
| Channel definitions (13 channels) | ~400 |
| Overview page | ~200 |
| Route/sidebar modifications | ~50 |
| **Total** | **~1500** |

## Out of Scope

- Per-channel dedicated RPC handlers (using unified `config.patch`)
- Server-side data fetching (e.g., listing Slack channels, Matrix rooms)
- Draft/preview config mode
- Real-time validation feedback from server
- Refactoring existing Discord view to use template
