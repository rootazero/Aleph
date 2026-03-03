# Runtime Channel Control: Feature Flag Simplification

> **Date**: 2026-03-03
> **Status**: Approved
> **Scope**: Build architecture, feature flags, channel initialization

## Problem

Aleph has 20+ Cargo feature flags controlling compilation of core components (Panel UI, Desktop, Telegram, Discord, etc.). This causes:

1. **Fragmentation** — developers must remember which features to enable for each build
2. **Silent failures** — forgetting a feature (e.g., `telegram`) means the channel silently doesn't exist at runtime
3. **Misaligned semantics** — Panel, Desktop, and channels are core Aleph capabilities, not optional add-ons
4. **~172 `#[cfg]` blocks** scattered across ~50 files, increasing maintenance burden

Real-world bug: `just server` didn't include `telegram` feature, causing "Channel not found" error at runtime.

## Decision

**Remove all production feature flags.** All Aleph capabilities compile unconditionally. Channel activation moves from compile-time (`#[cfg]`) to runtime configuration (`aleph.toml`).

### Features After (2 remaining)

```toml
[features]
default = []
loom = ["dep:loom"]       # Concurrency testing only
test-helpers = []          # Integration test utilities
```

### Features Removed (18+)

`gateway`, `control-plane`, `telegram`, `discord`, `whatsapp`, `slack`, `email`, `matrix`, `signal`, `mattermost`, `irc`, `webhook`, `xmpp`, `nostr`, `all-channels`, `cron`, `plugin-wasm`, `plugin-nodejs`, `plugin-all`, `cli`, `desktop-native`, `desktop`

## Design

### 1. Cargo.toml Changes

All `optional = true` dependencies become required:

```toml
# Before:
teloxide = { version = "0.13", optional = true }
serenity = { version = "0.12", optional = true }
lettre = { version = "0.11", optional = true }
extism = { version = "1.7", optional = true }
aleph-desktop = { path = "../crates/desktop", optional = true }
rust-embed = { version = "8", optional = true }

# After:
teloxide = { version = "0.13" }
serenity = { version = "0.12" }
lettre = { version = "0.11" }
extism = { version = "1.7" }
aleph-desktop = { path = "../crates/desktop" }
rust-embed = { version = "8" }
```

### 2. Channel Initialization (Runtime Config)

```rust
// Before (compile-time gating):
#[cfg(feature = "telegram")]
{
    let channel = TelegramChannel::new("telegram", TelegramConfig::default());
    channel_registry.register(Box::new(channel)).await;
}

// After (runtime config):
if let Some(telegram_config) = &gateway_config.channels.telegram {
    if telegram_config.enabled.unwrap_or(true) {
        let channel = TelegramChannel::new("telegram", telegram_config.into());
        channel_registry.register(Box::new(channel)).await;
    }
}
```

**Principle**: Channel present in config → register. `enabled = false` → skip. Not in config → don't register.

### 3. Struct Simplification

```rust
// Before:
pub struct TelegramChannel {
    #[cfg(feature = "telegram")]
    bot: Option<teloxide::Bot>,
    #[cfg(not(feature = "telegram"))]
    _phantom: PhantomData<()>,
}

// After:
pub struct TelegramChannel {
    bot: Option<teloxide::Bot>,
}
```

### 4. Module Export Cleanup

```rust
// Before (lib.rs):
#[cfg(feature = "gateway")]
pub mod gateway;
#[cfg(feature = "cron")]
pub mod cron;

// After:
pub mod gateway;
pub mod cron;
```

### 5. Justfile Simplification

```just
# Before:
server: wasm
    cargo build --bin {{server_bin}} --features control-plane,desktop,telegram --release

dev:
    cargo run --bin {{server_bin}} --features control-plane

# After:
server: wasm
    cargo build --bin {{server_bin}} --release

dev:
    cargo run --bin {{server_bin}}
```

### 6. aleph.toml Channel Config

```toml
[channels.telegram]
enabled = true
token = "bot-token-here"
allowed_users = [123456789]

[channels.discord]
enabled = false    # Configured but not active

# [channels.slack]  — not configured, won't register
```

## Impact

| Area | Files | Work |
|------|-------|------|
| `core/Cargo.toml` | 1 | Remove features + optional deps |
| `gateway/interfaces/*/` | ~20 | Remove `#[cfg]` blocks |
| `gateway/interfaces/mod.rs` | 1 | Remove conditional exports |
| `bin/aleph_server/.../mod.rs` | 1 | Rewrite `initialize_channels()` |
| `lib.rs` | 1 | Remove conditional module exports |
| `extension/` (WASM) | 3 | Remove `plugin-wasm` `#[cfg]` |
| `executor/builtin_registry/` | 2 | Remove gateway/desktop `#[cfg]` |
| `cron/` | 5 | Remove cron `#[cfg]` |
| `desktop/` + `builtin_tools/` | 2 | Remove desktop-native `#[cfg]` |
| `justfile` | 1 | Remove all `--features` flags |
| **Total** | **~37** | **~172 `#[cfg]` blocks removed** |

## Implementation Phases

1. **Phase 1**: Modify `Cargo.toml` — remove features, make optional deps required
2. **Phase 2**: Remove all `#[cfg(feature = "...")]` blocks (keep loom/test-helpers)
3. **Phase 3**: Rewrite `initialize_channels()` to runtime config-driven
4. **Phase 4**: Simplify justfile (remove all `--features`)
5. **Phase 5**: `cargo check` + `cargo test` verification

## Trade-offs

| Pro | Con |
|-----|-----|
| No more "forgot feature" bugs | Slightly larger binary (~5-10 MB) |
| Simpler mental model | Longer full compile (~30s more) |
| Clean justfile | All channel deps always pulled |
| Runtime channel flexibility | — |
| ~172 fewer `#[cfg]` blocks | — |

The cons are negligible for a desktop/server product. Aleph is not an embedded system — binary size and compile time are not critical constraints.
