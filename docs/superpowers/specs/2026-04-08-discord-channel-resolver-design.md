# Discord Channel Resolver Design

> Date: 2026-04-08
> Status: Approved

## Overview

Add a Discord channel resolver to Aleph that parses user input (channel names, IDs, guild prefixes) and resolves them to concrete Discord channels. Follows OpenClaw's resolver patterns but implemented in Rust with type safety.

## Motivation

Currently, Aleph's Discord integration lacks sophisticated channel resolution. Users must specify exact channel IDs, which is poor UX. OpenClaw demonstrates a better approach: parse natural input like `general` or `guild-name/general` and resolve to the correct channel.

## Design Decisions

### Resolution Strategy: Priority-Ordered

Search strategies tried in order:
1. **Exact ID** — `channel:123` or raw `123`
2. **Name Match** — case-insensitive exact name match
3. **Slug Match** — `general-channel` matches `General Channel`
4. **Fuzzy Match** — Levenshtein distance for typos

First match wins. If multiple strategies match, the higher-priority strategy wins.

### Guild Scope: All Accessible Guilds

Search across all guilds the bot has permission to access. This is the simplest and most flexible approach.

### Conflict Resolution: Fail on Ambiguity

If the same channel name exists in multiple guilds:
- Return `Ambiguous(Vec<Candidate>)` error
- LLM handles disambiguation by asking the user
- Does NOT auto-select "most active" — avoids silent wrong choices

### Input Formats Supported

| Input | Parsed As |
|-------|-----------|
| `123` | `ParsedInput::Id("123")` |
| `channel:123` | `ParsedInput::Id("123")` |
| `guild-name/general` | `ParsedInput::GuildPrefix { guild: "guild-name", channel: "general" }` |
| `general` | `ParsedInput::ChannelName("general")` |

### Channel Filtering

- **Exclude archived channels** by default
- Return `NotFound` if no match found (no fuzzy fallback suggestions)

### Scope: Discord-Only MVP

Initial implementation is Discord-specific. Generic `ChannelResolver` trait to be added in a future phase.

---

## Architecture

### File Structure

```
gateway/interfaces/discord/
├── mod.rs
├── api.rs              # REST wrapper (existing)
├── config.rs           # Configuration (existing)
├── permissions.rs      # Permission audit (existing)
├── resolver/
│   ├── mod.rs         # Public API
│   ├── input.rs        # Input format parsing
│   ├── strategy.rs     # Search strategies
│   └── error.rs        # ChannelResolutionError
```

### Module Boundaries

| Module | Responsibility |
|--------|----------------|
| `resolver/mod.rs` | Public API: `DiscordResolver::resolve()` |
| `resolver/input.rs` | Parse input strings into `ParsedInput` enum |
| `resolver/strategy.rs` | Implement search strategies |
| `resolver/error.rs` | Error types: `NotFound`, `Ambiguous`, `NoPermission` |

---

## API Design

### Public API

```rust
// resolver/mod.rs

pub struct DiscordResolver {
    http_client: Arc<DiscordHttpClient>,
}

impl DiscordResolver {
    /// Resolve user input to a Discord channel.
    pub async fn resolve(&self, input: &str) -> Result<Channel, ChannelResolutionError>;

    /// List all accessible channels for UI selection.
    pub async fn list_channels(&self) -> Result<Vec<Candidate>, ChannelResolutionError>;
}
```

### Return Types

```rust
// resolver/mod.rs

pub struct Channel {
    pub channel_id: String,
    pub guild_id: String,
    pub name: String,
}

pub struct Candidate {
    pub channel_id: String,
    pub channel_name: String,
    pub guild_id: String,
    pub guild_name: String,
}
```

### Error Types

```rust
// resolver/error.rs

pub enum ChannelResolutionError {
    NotFound(String),                    // No channel matched the input
    Ambiguous(Vec<Candidate>),         // Multiple matches found
    NoPermission(String),               // Bot lacks access to this channel
}
```

---

## Data Flow

```
User: "send to general"
  ↓
DiscordResolver::resolve("general")
  ↓
input.rs: parse("general") → ParsedInput::ChannelName("general")
  ↓
strategy.rs: try Exact → fail
  ↓
strategy.rs: try Name("general") → found [guild-A/general, guild-B/general]
  ↓
return Err(Ambiguous([Candidate { guild-A, ... }, Candidate { guild-B, ... }]))
  ↓
LLM: "I found #general in two servers: Guild A and Guild B. Which one?"
  ↓
User: "guild-a/general"
  ↓
resolve("guild-a/general") → ParsedInput::GuildPrefix { "guild-a", "general" }
  ↓
strategy.rs: try Name → found [guild-a/general] (unique)
  ↓
return Ok(Channel { id, guild_id, name })
```

---

## Implementation Details

### Input Parsing (resolver/input.rs)

```rust
pub enum ParsedInput {
    Id(String),
    GuildPrefix { guild: String, channel: String },
    ChannelName(String),
}

pub fn parse(input: &str) -> ParsedInput {
    // "123" → Id
    // "channel:123" → Id
    // "guild-name/general" → GuildPrefix
    // "general" → ChannelName
}
```

### Search Strategies (resolver/strategy.rs)

```rust
pub enum SearchStrategy {
    Exact,     // ID match
    Name,      // case-insensitive name
    Slug,      // "general-channel" ↔ "General Channel"
    Fuzzy,     // Levenshtein distance
}

impl DiscordResolver {
    async fn search(
        &self,
        input: &ParsedInput,
        guilds: &[Guild],
    ) -> Result<Channel, ChannelResolutionError> {
        // Try strategies in order, return first match
    }
}
```

### HTTP Client Reuse

The resolver reuses the existing `DiscordHttpClient` from `api.rs` but only calls read-only endpoints:
- `GET /users/@me/guilds` — list accessible guilds
- `GET /guilds/{guild_id}/channels` — list channels per guild

No write operations.

---

## Open Questions (Resolved)

| Question | Decision |
|----------|----------|
| Guild scope | All accessible guilds |
| Conflict handling | Fail ambiguous, let LLM ask user |
| Input formats | ID, guild/name, bare name |
| Archived channels | Excluded by default |
| Scope | Discord-only MVP |

---

## Testing Strategy

1. **Unit tests** for `input.rs` parsing (all formats)
2. **Unit tests** for `strategy.rs` (each strategy)
3. **Integration tests** with mocked HTTP responses

---

## References

- OpenClaw resolver: `extensions/discord/src/resolve-channels.ts`
- Aleph Channel trait: `gateway/channel.rs`
- Aleph Discord API: `gateway/interfaces/discord/api.rs`
