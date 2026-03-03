# Runtime Channel Control Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Eliminate 20+ Cargo feature flags, making all Aleph production capabilities compile unconditionally. Channel activation moves from compile-time `#[cfg]` to runtime config.

**Architecture:** Remove all feature definitions except `loom` and `test-helpers` from `core/Cargo.toml`. Convert all `optional = true` deps to required. Strip ~450 `#[cfg(feature = "...")]` blocks across ~40 files. Rewrite `initialize_channels()` to read runtime config from `aleph.toml`.

**Tech Stack:** Rust, Cargo features, serde, TOML config

---

## Task 1: Strip Feature Definitions from `core/Cargo.toml`

**Files:**
- Modify: `core/Cargo.toml` (features section at lines 11-61, binary definition at line 241-244)

**Step 1: Replace the entire `[features]` section**

Find the current features block (lines 11-61) and replace with:

```toml
[features]
default = []
# Loom concurrency testing (replaces std sync primitives)
loom = ["dep:loom"]
# Test helpers for integration tests
test-helpers = []
```

**Step 2: Convert all optional dependencies to required**

For each dependency currently marked `optional = true`, remove the `optional = true` flag. The affected deps are:

| Dependency | Feature it was gated by |
|---|---|
| `tokio-tungstenite` | gateway |
| `futures-util` | gateway |
| `clap` | gateway |
| `tower-http` | gateway |
| `axum` | gateway |
| `tower` | gateway |
| `mime_guess` | gateway |
| `mdns-sd` | gateway |
| `sysinfo` | gateway, cron |
| `rust-embed` | control-plane |
| `teloxide` | telegram |
| `serenity` | discord |
| `lettre` | email |
| `mail-parser` | email |
| `async-imap` | email |
| `async-native-tls` | email |
| `cron` (dep) | cron (feature) |
| `chrono-tz` | cron |
| `extism` | plugin-wasm |
| `inquire` | cli |
| `aleph-desktop` | desktop-native |

For each, remove `optional = true`. Example:
```toml
# Before:
teloxide = { version = "0.13", features = ["macros"], optional = true }
# After:
teloxide = { version = "0.13", features = ["macros"] }
```

**Step 3: Remove `required-features` from the binary definition**

Find (around line 241-244):
```toml
[[bin]]
name = "aleph-server"
path = "src/bin/aleph_server/main.rs"
required-features = ["gateway"]
```

Replace with:
```toml
[[bin]]
name = "aleph-server"
path = "src/bin/aleph_server/main.rs"
```

**Step 4: Commit**

```bash
git add core/Cargo.toml
git commit -m "build: remove production feature flags from Cargo.toml

Convert 20+ optional features to always-compiled. Only loom and
test-helpers remain as feature flags. All optional deps become required."
```

> **Note:** The project will NOT compile after this task. That's expected — the `#[cfg]` blocks still reference removed features. Tasks 2-6 fix that.

---

## Task 2: Strip `#[cfg]` from `lib.rs` and Top-Level Module Exports

**Files:**
- Modify: `core/src/lib.rs:107-111`

**Step 1: Remove feature gates on module declarations**

Find (lines 106-111):
```rust
// Feature-gated modules
#[cfg(feature = "gateway")]
pub mod gateway;

#[cfg(feature = "cron")]
pub mod cron;
```

Replace with:
```rust
pub mod gateway;
pub mod cron;
```

**Step 2: Commit**

```bash
git add core/src/lib.rs
git commit -m "build: remove cfg gates from lib.rs module exports"
```

---

## Task 3: Strip `#[cfg]` from `gateway/interfaces/mod.rs`

**Files:**
- Modify: `core/src/gateway/interfaces/mod.rs` (entire file, 104 lines)

**Step 1: Remove all feature gates from module declarations and re-exports**

Replace the entire file content with:

```rust
//! Interface Implementations
//!
//! This module contains concrete interface implementations for various messaging platforms.
//! Each interface represents a connection endpoint (Telegram, Discord, iMessage, CLI, etc.)
//! through which users interact with the Aleph Server.

pub mod cli;

#[cfg(target_os = "macos")]
pub mod imessage;

pub mod telegram;
pub mod discord;
pub mod whatsapp;
pub mod slack;
pub mod email;
pub mod matrix;
pub mod signal;
pub mod mattermost;
pub mod irc;
pub mod webhook;
pub mod xmpp;
pub mod nostr;

pub use cli::{CliChannel, CliChannelConfig, CliChannelFactory};

#[cfg(target_os = "macos")]
pub use imessage::{IMessageChannel, IMessageChannelFactory, IMessageConfig, IMessageTarget, MessageSender, MessagesDb};

pub use telegram::{TelegramChannel, TelegramChannelFactory, TelegramConfig};
pub use discord::{DiscordChannel, DiscordChannelFactory, DiscordConfig};
pub use whatsapp::{WhatsAppChannel, WhatsAppChannelFactory, WhatsAppConfig};
pub use slack::{SlackChannel, SlackChannelFactory, SlackConfig};
pub use email::{EmailChannel, EmailChannelFactory, EmailConfig};
pub use matrix::{MatrixChannel, MatrixChannelFactory, MatrixConfig};
pub use signal::{SignalChannel, SignalChannelFactory, SignalConfig};
pub use mattermost::{MattermostChannel, MattermostChannelFactory, MattermostConfig};
pub use irc::{IrcChannel, IrcChannelFactory, IrcConfig};
pub use webhook::{WebhookChannel, WebhookChannelFactory, WebhookChannelConfig};
pub use xmpp::{XmppChannel, XmppChannelFactory, XmppConfig};
pub use nostr::{NostrChannel, NostrChannelFactory, NostrConfig};
```

Note: `#[cfg(target_os = "macos")]` on `imessage` is a **platform gate**, not a feature gate — keep it.

**Step 2: Commit**

```bash
git add core/src/gateway/interfaces/mod.rs
git commit -m "build: remove feature gates from channel interface exports"
```

---

## Task 4: Strip `#[cfg]` from Channel Implementations (12 channels)

**Files:**
- Modify: `core/src/gateway/interfaces/telegram/mod.rs`
- Modify: `core/src/gateway/interfaces/telegram/message_ops.rs`
- Modify: `core/src/gateway/interfaces/discord/mod.rs`
- Modify: `core/src/gateway/interfaces/discord/message_ops.rs`
- Modify: `core/src/gateway/interfaces/discord/api.rs`
- Modify: `core/src/gateway/interfaces/slack/mod.rs`
- Modify: `core/src/gateway/interfaces/slack/message_ops.rs`
- Modify: `core/src/gateway/interfaces/email/mod.rs`
- Modify: `core/src/gateway/interfaces/email/message_ops.rs`
- Modify: `core/src/gateway/interfaces/matrix/mod.rs`
- Modify: `core/src/gateway/interfaces/matrix/message_ops.rs`
- Modify: `core/src/gateway/interfaces/signal/mod.rs`
- Modify: `core/src/gateway/interfaces/signal/message_ops.rs`
- Modify: `core/src/gateway/interfaces/mattermost/mod.rs`
- Modify: `core/src/gateway/interfaces/mattermost/message_ops.rs`
- Modify: `core/src/gateway/interfaces/irc/mod.rs`
- Modify: `core/src/gateway/interfaces/irc/message_ops.rs`
- Modify: `core/src/gateway/interfaces/webhook/mod.rs`
- Modify: `core/src/gateway/interfaces/webhook/message_ops.rs`
- Modify: `core/src/gateway/interfaces/xmpp/mod.rs`
- Modify: `core/src/gateway/interfaces/xmpp/message_ops.rs`
- Modify: `core/src/gateway/interfaces/nostr/mod.rs`
- Modify: `core/src/gateway/interfaces/nostr/message_ops.rs`

**Strategy:** For each channel, apply these transformations:

### A. Remove `#[cfg(feature = "channel_name")]` lines

Every line matching `#[cfg(feature = "telegram")]` (or discord, slack, etc.) should be deleted. The code block it guarded stays.

### B. Remove `#[cfg(not(feature = "channel_name"))]` blocks entirely

These are stub/fallback blocks that return errors or no-ops. Delete the entire `#[cfg(not(...))]` block AND the code it wraps.

### C. Remove PhantomData stubs

If a struct has:
```rust
#[cfg(feature = "telegram")]
bot: Option<Bot>,
#[cfg(not(feature = "telegram"))]
_phantom: PhantomData<()>,
```

Replace with just:
```rust
bot: Option<Bot>,
```

### D. Clean up `use` imports

Remove `#[cfg(feature = "...")]` from import lines. If `PhantomData` was only used in stubs, remove its import too.

**Step 1: Process Telegram (highest density — 42 cfg blocks)**

Read `telegram/mod.rs` and `telegram/message_ops.rs`. Apply transformations A-D.

Key patterns to strip in `telegram/mod.rs`:
- Line 42-49: `#[cfg(feature = "telegram")] use teloxide::{...}` → remove cfg line, keep import
- Line 70-71: `#[cfg(feature = "telegram")] bot: Option<Bot>` → remove cfg, keep field
- Line 97-98: `#[cfg(feature = "telegram")] bot: None` → remove cfg, keep init
- Lines 122-290: `#[cfg(feature = "telegram")]` on methods → remove cfg, keep methods
- Lines 316-439: `#[cfg(feature = "telegram")] { ... }` blocks → remove cfg, unwrap block (remove braces)
- Lines 441-446: `#[cfg(not(feature = "telegram"))] { ... }` → delete entirely
- Lines 755-766: `#[cfg(not(feature = "telegram"))]` stub edit_message → delete entirely

**Step 2: Process Discord (60 cfg blocks)**

Read `discord/mod.rs`, `discord/message_ops.rs`, `discord/api.rs`. Apply same transformations.

Key extras:
- `struct Handler` with `#[cfg(feature = "discord")]` → remove cfg
- `impl EventHandler for Handler` with cfg → remove cfg
- `discord/api.rs` validation functions with cfg → remove cfg

**Step 3: Process remaining 10 channels**

For each of slack, email, matrix, signal, mattermost, irc, webhook, xmpp, nostr, whatsapp — read both `mod.rs` and `message_ops.rs`, apply transformations A-D.

Most of these are lighter (2-15 cfg blocks each). Some (whatsapp, webhook) may have very few.

**Step 4: Run quick check**

```bash
# This won't pass yet (other files still have cfg), but checks channel code specifically
cargo check -p alephcore 2>&1 | head -50
```

**Step 5: Commit**

```bash
git add core/src/gateway/interfaces/
git commit -m "build: remove feature gates from all 12 channel implementations"
```

---

## Task 5: Strip `#[cfg]` from Gateway Core (`gateway/mod.rs`, `gateway/handlers/`, `gateway/control_plane/`)

**Files:**
- Modify: `core/src/gateway/mod.rs` (~28 cfg blocks)
- Modify: `core/src/gateway/handlers/mod.rs` (1 discord cfg)
- Modify: `core/src/gateway/handlers/discord_panel.rs` (6 discord cfg blocks)
- Modify: `core/src/gateway/control_plane/mod.rs` (3 control-plane cfg blocks)

**Step 1: `gateway/mod.rs`**

Remove all `#[cfg(feature = "gateway")]` lines from module declarations, struct definitions, and re-exports. This file has ~28 cfg blocks — all gating on `gateway` which is now always-on.

**Step 2: `gateway/handlers/mod.rs`**

Find (around line 92):
```rust
#[cfg(feature = "discord")]
pub mod discord_panel;
```
Remove the cfg line, keep `pub mod discord_panel;`.

**Step 3: `gateway/handlers/discord_panel.rs`**

Remove all 6 `#[cfg(feature = "discord")]` lines. Keep the code they guard.

**Step 4: `gateway/control_plane/mod.rs`**

Remove all 3 `#[cfg(feature = "control-plane")]` lines. Keep the module declarations they guard.

**Step 5: Commit**

```bash
git add core/src/gateway/
git commit -m "build: remove feature gates from gateway core, handlers, control plane"
```

---

## Task 6: Strip `#[cfg]` from Server Binary (`bin/aleph_server/`)

**Files:**
- Modify: `core/src/bin/aleph_server/commands/start/mod.rs` (~48 gateway + channel cfg blocks)
- Modify: `core/src/bin/aleph_server/commands/start/builder/handlers.rs` (~11 gateway + 2 discord + 1 control-plane cfg blocks)
- Modify: `core/src/bin/aleph_server/server_init.rs` (~7 cfg blocks)
- Modify: `core/src/bin/aleph_server/main.rs` (~8 cfg blocks)

**Step 1: `start/mod.rs` — imports (lines 1-80)**

Remove ALL `#[cfg(feature = "gateway")]` and `#[cfg(feature = "telegram")]` etc. from import lines.

Before (lines 11-55):
```rust
#[cfg(feature = "gateway")]
use std::sync::Arc;
#[cfg(feature = "gateway")]
use alephcore::gateway::GatewayServer;
...
#[cfg(feature = "telegram")]
use alephcore::gateway::interfaces::{TelegramChannel, TelegramConfig};
#[cfg(feature = "discord")]
use alephcore::gateway::interfaces::{DiscordChannel, DiscordConfig};
#[cfg(feature = "whatsapp")]
use alephcore::gateway::interfaces::{WhatsAppChannel, WhatsAppConfig};
```

After:
```rust
use std::sync::Arc;
use alephcore::gateway::GatewayServer;
...
use alephcore::gateway::interfaces::{TelegramChannel, TelegramConfig};
use alephcore::gateway::interfaces::{DiscordChannel, DiscordConfig};
use alephcore::gateway::interfaces::{WhatsAppChannel, WhatsAppConfig};
```

Note: Keep `#[cfg(target_os = "macos")]` on the iMessage import (line 46-47) — that's a platform gate.

Also remove `#[cfg(feature = "gateway")]` from `mod builder;` and `use builder::*;` (lines 77-80).

**Step 2: `start/mod.rs` — function definitions**

Remove `#[cfg(feature = "gateway")]` from every function signature. Affected functions:
- `validate_bind_address` (line 87)
- `print_startup_banner` (line ~100)
- `initialize_tracing` (line ~130)
- `load_gateway_config` (line ~160)
- `initialize_session_manager` (line ~200)
- `initialize_extension_manager` (line ~250)
- `create_provider_registry_from_config` (line ~300)
- `register_agent_handlers` (line ~350)
- `register_poe_handlers` (line ~500)
- `initialize_auth` (line ~600)
- `load_app_config` (line ~700)
- `setup_graceful_shutdown` (line ~750)
- `initialize_channels` (line 784)
- `initialize_inbound_router` (line 876)
- `start_server` (line ~920)

**Step 3: `start/mod.rs` — channel blocks in `initialize_channels()`**

Remove `#[cfg(feature = "telegram")]`, `#[cfg(feature = "discord")]`, `#[cfg(feature = "whatsapp")]` from the channel registration blocks (lines 805-841). Keep `#[cfg(target_os = "macos")]` on the iMessage block.

**Step 4: `builder/handlers.rs`**

Remove all `#[cfg(feature = "gateway")]` (11 blocks) and `#[cfg(feature = "discord")]` (2 blocks) lines.

Find combined gate:
```rust
#[cfg(all(feature = "gateway", feature = "control-plane"))]
```
Replace with nothing (remove line, keep code).

Find:
```rust
#[cfg(all(feature = "gateway", feature = "discord"))]
```
Replace with nothing.

**Step 5: `server_init.rs` and `main.rs`**

Remove all `#[cfg(feature = "gateway")]` lines from both files.

**Step 6: Commit**

```bash
git add core/src/bin/aleph_server/
git commit -m "build: remove feature gates from aleph-server binary"
```

---

## Task 7: Strip `#[cfg]` from Extension System (plugin-wasm)

**Files:**
- Modify: `core/src/extension/plugin_loader.rs` (~19 cfg blocks)
- Modify: `core/src/extension/runtime/wasm/mod.rs` (~18 cfg blocks)
- Modify: `core/src/extension/runtime/wasm/host_functions.rs` (~8 cfg blocks)

**Step 1: `plugin_loader.rs`**

Remove all `#[cfg(feature = "plugin-wasm")]` and `#[cfg(not(feature = "plugin-wasm"))]` blocks. Patterns:
- Conditional struct fields → keep the real field, remove PhantomData stubs
- Conditional method implementations → keep the real impl, delete the stub
- `is_wasm_runtime_active()` stub returning false → delete, keep real version

**Step 2: `runtime/wasm/mod.rs`**

Remove all cfg blocks. Patterns:
- Conditional imports → keep all imports
- Conditional struct fields → keep real fields, remove PhantomData
- Dual method implementations → keep real impl, delete no-op stub

**Step 3: `runtime/wasm/host_functions.rs`**

Remove all cfg blocks from host function definitions.

**Step 4: Commit**

```bash
git add core/src/extension/
git commit -m "build: remove plugin-wasm feature gates from extension system"
```

---

## Task 8: Strip `#[cfg]` from Cron, Desktop, Executor, Agent Loop

**Files:**
- Modify: `core/src/cron/mod.rs` (~8 cfg blocks)
- Modify: `core/src/cron/scheduler.rs` (~3 cfg blocks)
- Modify: `core/src/cron/config.rs` (~2 cfg blocks)
- Modify: `core/src/cron/resource.rs` (~2 cfg blocks)
- Modify: `core/src/desktop/mod.rs` (~1 cfg block)
- Modify: `core/src/builtin_tools/desktop.rs` (~5 cfg blocks)
- Modify: `core/src/executor/builtin_registry/registry.rs` (~8 cfg blocks for gateway + 2 for desktop)
- Modify: `core/src/executor/builtin_registry/definitions.rs` (~7 cfg blocks)
- Modify: `core/src/executor/builtin_registry/config.rs` (~2 cfg blocks)
- Modify: `core/src/executor/builtin_registry/mod.rs` (~1 cfg block + test cfg)
- Modify: `core/src/builtin_tools/sessions/mod.rs` (~6 cfg blocks)
- Modify: `core/src/agent_loop/mod.rs` (~2 cli cfg blocks)
- Modify: `core/src/config/types/agent/subagents.rs` (~2 cfg blocks)

**Step 1: Cron module**

Remove all `#[cfg(feature = "cron")]` from `cron/mod.rs`, `scheduler.rs`, `config.rs`, `resource.rs`.

**Step 2: Desktop module**

Remove `#[cfg(feature = "desktop-native")]` from `desktop/mod.rs` and `builtin_tools/desktop.rs`.

In `builtin_tools/desktop.rs`, there will be `#[cfg(not(feature = "desktop-native"))]` stubs — delete them entirely, keep only the real implementation.

**Step 3: Executor builtin registry**

Remove all `#[cfg(feature = "gateway")]` and `#[cfg(feature = "desktop-native")]` from:
- `registry.rs` — tool registration
- `definitions.rs` — tool definitions (sessions tools)
- `config.rs` — config handling
- `mod.rs` — module exports and tests

**Step 4: Sessions and Agent Loop**

Remove `#[cfg(feature = "gateway")]` from `builtin_tools/sessions/mod.rs`.
Remove `#[cfg(feature = "cli")]` from `agent_loop/mod.rs`.

**Step 5: Subagents config**

Remove `#[cfg(feature = "gateway")]` from `config/types/agent/subagents.rs`.

**Step 6: Commit**

```bash
git add core/src/cron/ core/src/desktop/ core/src/builtin_tools/ core/src/executor/ core/src/agent_loop/ core/src/config/
git commit -m "build: remove feature gates from cron, desktop, executor, agent_loop"
```

---

## Task 9: Simplify Justfile

**Files:**
- Modify: `justfile`

**Step 1: Remove all `--features` flags from build/dev recipes**

Replace the development recipes:
```just
# Run server with Panel UI (debug)
dev:
    cargo run --bin {{server_bin}}

# Run server with native desktop (debug)
dev-desktop:
    cargo run --bin {{server_bin}}

# Run server headless (debug)
dev-headless:
    cargo run --bin {{server_bin}}
```

Since all features are always compiled, `dev`, `dev-desktop`, and `dev-headless` are now identical. Consolidate to just `dev`:

```just
# Run server (debug)
dev:
    cargo run --bin {{server_bin}}
```

Replace build recipes:
```just
# Build server (release)
server: wasm
    cargo build --bin {{server_bin}} --release
    @echo "✓ Server: {{release_dir}}/{{server_bin}}"

# Build server (debug, faster compile)
server-debug: wasm
    cargo build --bin {{server_bin}}
    @echo "✓ Server (debug): {{debug_dir}}/{{server_bin}}"
```

Remove `server-light` recipe (no longer meaningful — all builds are "full").

Replace check/clippy recipes:
```just
# Quick check: core compiles
check:
    cargo check -p alephcore

# Quick check: core + desktop compiles
check-desktop:
    cargo check -p aleph-desktop

# Clippy on core
clippy:
    cargo clippy -p alephcore -- -D warnings

# Clippy on desktop crate
clippy-desktop:
    cargo clippy -p aleph-desktop -- -D warnings
```

Remove `--features` from test recipes:
```just
# Run core tests
test:
    cargo test -p alephcore --lib

# Run desktop crate tests
test-desktop:
    cargo test -p aleph-desktop --lib

# Run desktop integration tests
test-desktop-integration:
    cargo test -p alephcore --lib builtin_tools::desktop
```

Keep loom test recipe (it still uses a feature):
```just
# Run loom concurrency tests
test-loom:
    LOOM_MAX_PREEMPTIONS=3 cargo test -p alephcore --features loom --lib loom
```

**Step 2: Commit**

```bash
git add justfile
git commit -m "build: simplify justfile — remove all --features flags"
```

---

## Task 10: Verify Build and Tests

**Step 1: Run cargo check**

```bash
cargo check -p alephcore
```

Expected: SUCCESS (0 errors). If there are errors, they're likely:
- Unused imports (from deleted stubs) → fix by removing
- Missing `PhantomData` usage after removal → fix by removing import
- Duplicate definitions (if both cfg/not-cfg versions remain) → fix by keeping only the real version

**Step 2: Run cargo check for the binary**

```bash
cargo check --bin aleph-server
```

**Step 3: Run tests**

```bash
cargo test -p alephcore --lib
```

Expected: All tests pass (except the 2 known pre-existing failures in `tools::markdown_skill::loader::tests`).

**Step 4: Run clippy**

```bash
cargo clippy -p alephcore -- -D warnings
```

Fix any warnings (likely dead code from removed stubs, unused variables).

**Step 5: Build WASM + server (full pipeline)**

```bash
just server
```

Expected: Clean build without `--features` flags.

**Step 6: Final commit (if any fixes were needed)**

```bash
git add -A
git commit -m "build: fix compilation issues from feature flag removal"
```

---

## Task 11: Update Documentation

**Files:**
- Modify: `CLAUDE.md` (Feature Flags section)
- Modify: `docs/reference/SERVER_DEVELOPMENT.md` (if it references features)

**Step 1: Update CLAUDE.md Feature Flags section**

Find the Feature Flags section and replace with:

```markdown
### Feature Flags

All production capabilities compile unconditionally. Only testing features remain:

```toml
[features]
default = []
loom = ["dep:loom"]       # Concurrency testing only
test-helpers = []          # Integration test utilities
```

Channel activation is controlled at runtime via `aleph.toml`:
```toml
[channels.telegram]
enabled = true
token = "your-bot-token"
```
```

**Step 2: Update build command examples in CLAUDE.md**

Replace all `cargo run --bin aleph-server --features control-plane` etc. with just `cargo run --bin aleph-server`.

**Step 3: Commit**

```bash
git add CLAUDE.md docs/
git commit -m "docs: update feature flags documentation for runtime channel control"
```

---

## Summary

| Task | Scope | ~Cfg Blocks Removed |
|------|-------|---------------------|
| 1 | Cargo.toml features + deps | 22 feature defs |
| 2 | lib.rs module exports | 2 |
| 3 | interfaces/mod.rs | 24 |
| 4 | 12 channel implementations | ~250 |
| 5 | gateway core + handlers | ~36 |
| 6 | aleph-server binary | ~70 |
| 7 | extension system (WASM) | ~45 |
| 8 | cron, desktop, executor, agent_loop | ~40 |
| 9 | justfile | N/A |
| 10 | verification | N/A |
| 11 | documentation | N/A |
| **Total** | | **~450+** |
