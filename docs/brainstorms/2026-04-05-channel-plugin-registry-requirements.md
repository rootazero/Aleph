---
date: 2026-04-05
topic: channel-plugin-registry
---

# Channel Plugin Registry

## Problem Frame

Aleph's channel implementations (Telegram, Discord, Slack, etc.) are **hardcoded module imports** in `gateway/interfaces/mod.rs`. Adding a new channel requires:
1. Adding `pub mod new_channel;` to `interfaces/mod.rs`
2. Adding `pub use new_channel::{NewChannel, NewChannelFactory, NewConfig};`
3. Importing the factory in the channel registration code

OpenClaw solves this with a **runtime-discoverable plugin system**: drop a file in `channels/plugins/` and it's automatically registered. Aleph can do better — Rust's type system can make this **compile-time safe** with zero runtime overhead.

**Affected users**: Channel developers extending Aleph with new messaging platforms.

---

## Requirements

### Plugin Discovery

- **R1**: Each channel implements a `ChannelPlugin` trait that provides metadata and a factory
- **R2**: `#[derive(ChannelPlugin)]` macro auto-generates:
  - `ChannelMetadata` from struct name and doc comments
  - Automatic registration in a static registry via `inventory::submit!`
  - `ChannelFactory` implementation delegating to the struct's constructor

- **R3**: `ChannelRegistry` reads from the static registry at startup, no manual registration needed

### Channel Metadata

- **R4**: `ChannelMetadata` includes:
  - `channel_type: &'static str` (e.g., "telegram", "discord")
  - `display_name: &'static str` (e.g., "Telegram", "Discord")
  - `description: &'static str` (from doc comments)
  - `config_schema: Schema` (for config validation UI)

- **R5**: Metadata is derived from the channel struct's doc comments and name

### Plugin Trait

- **R6**: `ChannelPlugin` sealed trait:
  ```rust
  pub trait ChannelPlugin: Send + Sync + 'static {
      const CHANNEL_TYPE: &'static str;
      fn metadata() -> ChannelMetadata;
      fn create_factory(config: ChannelConfig) -> Arc<dyn ChannelFactory>
      where Self: Sized;
  }
  ```

- **R7**: Only one impl per `CHANNEL_TYPE` allowed (compile-time enforcement via const)

### Backward Compatibility

- **R8**: Existing `ChannelFactory` trait remains unchanged
- **R9**: Existing channels work without modification (they can opt-in to `#[derive(ChannelPlugin)]` later)
- **R10**: `interfaces/mod.rs` no longer needs manual channel imports after migration

### Third-Party Channels

- **R11**: Third-party channels can be added as separate crates that depend on `aleph-core`
- **R12**: Plugin registry is global (process-wide), not crate-local

---

## Success Criteria

- **SC1**: Adding a new built-in channel requires only adding a file with `#[derive(ChannelPlugin)]`
- **SC2**: No manual registration calls needed — auto-discovery via `inventory`
- **SC3**: Third-party channels work without modifying Aleph core
- **SC4**: Compile-time safety: duplicate `CHANNEL_TYPE` produces a clear error

---

## Scope Boundaries

**In Scope:**
- `gateway/interfaces/plugin.rs` — new `ChannelPlugin` trait
- `gateway/interfaces/plugin_derive.rs` — `#[derive(ChannelPlugin)]` macro
- `gateway/channel_registry.rs` — update to use plugin registry
- `gateway/interfaces/mod.rs` — remove hardcoded imports (cleanup)

**Out of Scope:**
- WASM plugin support (separate effort)
- Dynamic config reload after startup
- Per-channel feature flags

---

## Key Decisions

- **inventory vs lazy_static**: Use `inventory` crate (`inventory::submit!`) for compile-time plugin collection — works with `#[derive]` and produces static references
- **Derive vs manual impl**: `#[derive(ChannelPlugin)]` preferred for ergonomics, but manual impl possible for complex channels
- **Sealed trait**: `ChannelPlugin` is sealed to prevent external implementors

---

## Dependencies / Assumptions

- **D1**: `inventory` crate is available (check Cargo.toml)
- **D2**: Existing `ChannelFactory` + `Channel` traits are stable (verified)
- **D3**: All existing channels can have `#[derive(ChannelPlugin)]` added without breaking changes

---

## Implementation Sketch

```rust
// gateway/interfaces/plugin.rs

use serde::Serialize;
use std::sync::Arc;

inventory::collect!(&'static dyn ChannelPlugin);

pub struct ChannelMetadata {
    pub channel_type: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
}

pub trait ChannelPlugin: Send + Sync + 'static {
    fn metadata(&self) -> ChannelMetadata;
    fn factory(&self) -> Arc<dyn ChannelFactory>
    where Self: Sized;
}

// Example derive usage:
#[derive(ChannelPlugin)]
#[channel(type = "telegram", name = "Telegram")]
pub struct TelegramChannel { ... }
```

---

## Next Steps

→ `/ce:plan` for Channel Plugin Registry implementation planning
