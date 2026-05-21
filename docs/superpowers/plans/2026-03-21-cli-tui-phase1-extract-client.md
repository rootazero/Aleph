# CLI/TUI Phase 1: Extract `apps/client/` Crate

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract the duplicated WebSocket client, config, and error code from `apps/cli/` and `apps/tui/` into a shared `apps/client/` crate, eliminating 520 lines of identical code.

**Architecture:** Create `aleph-client` as a pure protocol client library (no core dependency). Both `apps/cli/` and `apps/tui/` will depend on it instead of maintaining their own copies. `src/cli/` is left untouched in this phase — it has a different, simpler `GatewayClient` used only by the server binary. Extracting it here would create a circular dependency (core binary → apps/client). It gets migrated in Phase 2 when the server binary commands move to `apps/cli/`.

> **Note:** The spec's Phase 1 originally included `src/cli/` merger. This plan intentionally defers it because `src/cli/GatewayClient` is architecturally distinct from `apps/cli/AlephClient` (stateless vs persistent WebSocket), and extracting it requires first migrating the server binary commands (Phase 2 scope).

**Tech Stack:** Rust, tokio, tokio-tungstenite, aleph-protocol, serde, thiserror

**Spec:** `docs/reference/2026-03-20-cli-tui-separation-design.md`

---

## File Map

| Action | File | Purpose |
|--------|------|---------|
| Create | `apps/client/Cargo.toml` | Crate manifest for aleph-client |
| Create | `apps/client/src/lib.rs` | Re-export AlephClient, CliConfig, CliError, CliResult |
| Move | `apps/cli/src/client.rs` → `apps/client/src/connection.rs` | WebSocket JSON-RPC 2.0 client |
| Move | `apps/cli/src/config.rs` → `apps/client/src/config.rs` | CliConfig + ManifestConfig |
| Move | `apps/cli/src/error.rs` → `apps/client/src/error.rs` | CliError + CliResult |
| Delete | `apps/tui/src/client.rs` | Identical duplicate |
| Delete | `apps/tui/src/config.rs` | Identical duplicate |
| Delete | `apps/tui/src/error.rs` | Identical duplicate |
| Modify | `apps/cli/Cargo.toml` | Add aleph-client dep, remove extracted deps |
| Modify | `apps/tui/Cargo.toml` | Add aleph-client dep, remove extracted deps |
| Modify | `apps/cli/src/main.rs` | Replace `mod client/config/error` with `use aleph_client::*` |
| Modify | `apps/tui/src/main.rs` | Replace `mod client/config/error` with `use aleph_client::*` |
| Modify | `Cargo.toml` (root) | Add `apps/client` to workspace members |
| Modify | All `apps/cli/src/commands/*.rs` | Update `use crate::` → `use aleph_client::` |
| Modify | `apps/cli/src/tui/mod.rs` | Update `use crate::` → `use aleph_client::` |
| Modify | `apps/tui/src/tui/mod.rs` | Update `use crate::` → `use aleph_client::` |

---

### Task 1: Create `apps/client/` Crate Scaffold

**Files:**
- Create: `apps/client/Cargo.toml`
- Create: `apps/client/src/lib.rs`
- Modify: `Cargo.toml` (root workspace)

- [ ] **Step 1: Create Cargo.toml**

```toml
[package]
name = "aleph-client"
version = "0.1.0"
edition = "2021"
description = "Aleph Gateway client library — WebSocket JSON-RPC 2.0"

# IMPORTANT: This crate MUST NOT depend on alephcore.
# It is a pure protocol client used by CLI, TUI, and any future client.

[dependencies]
aleph-protocol = { path = "../../shared/protocol" }
tokio = { version = "1.35", features = ["rt-multi-thread", "sync", "time", "macros"] }
tokio-tungstenite = { version = "0.24", features = ["native-tls"] }
futures-util = "0.3"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
toml = "0.8"
thiserror = "2.0"
anyhow = "1.0"
tracing = "0.1"
uuid = { version = "1.7", features = ["v4"] }
dirs = "6.0"
```

- [ ] **Step 2: Create lib.rs with placeholder modules**

```rust
//! Aleph Client Library
//!
//! Shared WebSocket JSON-RPC 2.0 client for communicating with Aleph Gateway.
//! Used by CLI, TUI, and any future client application.

mod connection;
mod config;
mod error;

pub use connection::AlephClient;
pub use config::{CliConfig, ManifestConfig};
pub use error::{CliError, CliResult};
```

- [ ] **Step 3: Add to workspace members**

In root `Cargo.toml`, add `"apps/client"` to the `[workspace] members` list, after `"shared/ui_logic"` and before `"apps/cli"`.

- [ ] **Step 4: Verify scaffold compiles**

Run: `cargo check -p aleph-client 2>&1 | head -20`

Expected: Errors about missing modules (connection, config, error) — that's correct, we haven't moved them yet.

- [ ] **Step 5: Commit**

```bash
git add apps/client/Cargo.toml apps/client/src/lib.rs Cargo.toml
git commit -m "cli-tui: scaffold apps/client crate (aleph-client)"
```

---

### Task 2: Move Error Types to `apps/client/`

**Files:**
- Create: `apps/client/src/error.rs` (move from `apps/cli/src/error.rs`)
- Delete: `apps/tui/src/error.rs`

- [ ] **Step 1: Copy error.rs to apps/client/**

Copy `apps/cli/src/error.rs` → `apps/client/src/error.rs`. No content changes needed — the file is self-contained (depends only on `thiserror` and `anyhow`).

The file defines:
- `CliError` enum with variants: `Connection`, `WebSocket`, `Json`, `Io`, `Rpc`, `Timeout`, `Disconnected`, `Config`, `Other`
- `CliResult<T>` type alias
- `From<anyhow::Error>` and `From<tungstenite::Error>` impls

- [ ] **Step 2: Add tokio-tungstenite to error.rs imports**

The `From<tungstenite::Error>` impl needs `tokio_tungstenite` in scope. Verify the import path works from the new crate location. The import should be:

```rust
use tokio_tungstenite::tungstenite;
```

This is already in the original file — just verify it compiles from the new location.

- [ ] **Step 3: Verify error module compiles**

Run: `cargo check -p aleph-client 2>&1 | head -20`

Expected: Errors about missing `connection` and `config` modules only. Error module should compile clean.

- [ ] **Step 4: Commit**

```bash
git add apps/client/src/error.rs
git commit -m "cli-tui: move error types to aleph-client"
```

---

### Task 3: Move Config to `apps/client/`

**Files:**
- Create: `apps/client/src/config.rs` (move from `apps/cli/src/config.rs`)
- Delete: `apps/tui/src/config.rs`

- [ ] **Step 1: Copy config.rs to apps/client/**

Copy `apps/cli/src/config.rs` → `apps/client/src/config.rs`.

- [ ] **Step 2: Update internal import**

Change `use crate::error::{CliError, CliResult};` — this should already work since error.rs is now in the same crate.

- [ ] **Step 3: Verify config module compiles**

Run: `cargo check -p aleph-client 2>&1 | head -20`

Expected: Only `connection` module missing.

- [ ] **Step 4: Commit**

```bash
git add apps/client/src/config.rs
git commit -m "cli-tui: move config types to aleph-client"
```

---

### Task 4: Move WebSocket Client to `apps/client/`

**Files:**
- Create: `apps/client/src/connection.rs` (move from `apps/cli/src/client.rs`)
- Delete: `apps/tui/src/client.rs`

- [ ] **Step 1: Copy client.rs to apps/client/src/connection.rs**

Copy `apps/cli/src/client.rs` → `apps/client/src/connection.rs`.

- [ ] **Step 2: Update internal imports**

Replace:
```rust
use crate::config::CliConfig;
use crate::error::{CliError, CliResult};
```
These should already work since config and error are in the same crate now.

- [ ] **Step 3: Verify aleph-client compiles clean**

Run: `cargo check -p aleph-client`

Expected: Clean compilation, no errors.

- [ ] **Step 4: Commit**

```bash
git add apps/client/src/connection.rs
git commit -m "cli-tui: move WebSocket client to aleph-client"
```

---

### Task 5: Rewire `apps/cli/` to Depend on `aleph-client`

**Files:**
- Modify: `apps/cli/Cargo.toml`
- Modify: `apps/cli/src/main.rs`
- Delete: `apps/cli/src/client.rs`
- Delete: `apps/cli/src/config.rs`
- Delete: `apps/cli/src/error.rs`
- Modify: All files in `apps/cli/src/commands/` that import from `crate::client`, `crate::config`, or `crate::error`
- Modify: `apps/cli/src/tui/mod.rs`

- [ ] **Step 1: Add aleph-client dependency to Cargo.toml**

In `apps/cli/Cargo.toml`, add:
```toml
aleph-client = { path = "../client" }
```

Do NOT remove any existing dependencies in this step — dependency cleanup can be done in a follow-up.

- [ ] **Step 2: Update main.rs module declarations and imports**

In `apps/cli/src/main.rs`, replace:
```rust
mod client;
mod commands;
mod config;
mod error;
pub(crate) mod output;
mod tui;
```

With:
```rust
mod commands;
pub(crate) mod output;
mod tui;

use aleph_client::{AlephClient, CliConfig, CliError, CliResult};
```

Remove:
```rust
use crate::config::CliConfig;
use crate::error::CliResult;
```

**Also update inline references in main.rs body** — search for `crate::client::` and `crate::error::` in the match arms (around lines 926, 977, 993):
- `crate::client::AlephClient::connect(...)` → `AlephClient::connect(...)`
- `error::CliError::Other(...)` → `CliError::Other(...)`

- [ ] **Step 3: Update all command file imports**

In every file under `apps/cli/src/commands/`, replace:
- `use crate::client::AlephClient;` → `use aleph_client::AlephClient;`
- `use crate::config::CliConfig;` → `use aleph_client::CliConfig;`
- `use crate::error::{CliError, CliResult};` → `use aleph_client::{CliError, CliResult};`
- `use crate::error::CliResult;` → `use aleph_client::CliResult;`

Files to update (all in `apps/cli/src/commands/`):
- `ask.rs`, `chat.rs`, `chat_cmd.rs`, `config_cmd.rs`, `connect.rs`
- `daemon.rs`, `gateway_cmd.rs`, `guests.rs`, `health.rs`
- `identity_cmd.rs`, `info.rs`, `logs_cmd.rs`, `mcp_cmd.rs`
- `memory_cmd.rs`, `models_cmd.rs`, `plugins_cmd.rs`, `plugin_cmd.rs`
- `poe_cmd.rs`, `providers_cmd.rs`, `services_cmd.rs`, `session.rs`
- `skills_cmd.rs`, `tools.rs`, `vault_cmd.rs`, `workspace_cmd.rs`

- [ ] **Step 4: Update tui/mod.rs imports**

In `apps/cli/src/tui/mod.rs`, replace:
```rust
use crate::client::AlephClient;
use crate::config::CliConfig;
use crate::error::CliResult;
```
With:
```rust
use aleph_client::{AlephClient, CliConfig, CliResult};
```

Note: files under `apps/cli/src/tui/widgets/` do NOT reference `crate::client/config/error` — they use `crate::tui::` and need no changes.

- [ ] **Step 5: Verify compilation with old files still present**

Run: `cargo check -p aleph-cli`

Expected: Clean compilation. Old files still exist but `mod client/config/error` declarations are removed so they're ignored.

- [ ] **Step 6: Delete old files**

Delete:
- `apps/cli/src/client.rs`
- `apps/cli/src/config.rs`
- `apps/cli/src/error.rs`
- `apps/cli/src/client.rs.backup` (if exists)

- [ ] **Step 7: Verify compilation after deletion**

Run: `cargo check -p aleph-cli`

Expected: Still compiles clean.

- [ ] **Step 8: Commit**

```bash
git add -A apps/cli/
git commit -m "cli-tui: rewire apps/cli to use aleph-client"
```

---

### Task 6: Rewire `apps/tui/` to Depend on `aleph-client`

**Files:**
- Modify: `apps/tui/Cargo.toml`
- Modify: `apps/tui/src/main.rs`
- Delete: `apps/tui/src/client.rs`
- Delete: `apps/tui/src/config.rs`
- Delete: `apps/tui/src/error.rs`
- Modify: `apps/tui/src/tui/mod.rs`

- [ ] **Step 1: Add aleph-client dependency to Cargo.toml**

In `apps/tui/Cargo.toml`, add:
```toml
aleph-client = { path = "../client" }
```

Do NOT remove any existing dependencies in this step.

- [ ] **Step 2: Update main.rs**

In `apps/tui/src/main.rs`, replace:
```rust
mod client;
mod config;
mod error;
mod tui;
```

With:
```rust
mod tui;
```

Replace:
```rust
use crate::client::AlephClient;
use crate::config::CliConfig;
use crate::error::CliResult;
```

With:
```rust
use aleph_client::{AlephClient, CliConfig, CliResult};
```

- [ ] **Step 3: Update tui/mod.rs imports**

In `apps/tui/src/tui/mod.rs`, replace:
```rust
use crate::client::AlephClient;
use crate::config::CliConfig;
use crate::error::CliResult;
```

With:
```rust
use aleph_client::{AlephClient, CliConfig, CliResult};
```

Note: files under `apps/tui/src/tui/widgets/` use `crate::tui::` only — no changes needed.

- [ ] **Step 4: Verify compilation with old files still present**

Run: `cargo check -p aleph-tui`

Expected: Clean compilation.

- [ ] **Step 5: Delete old files**

Delete:
- `apps/tui/src/client.rs`
- `apps/tui/src/config.rs`
- `apps/tui/src/error.rs`

- [ ] **Step 6: Verify compilation after deletion**

Run: `cargo check -p aleph-tui`

Expected: Still compiles clean.

- [ ] **Step 7: Commit**

```bash
git add -A apps/tui/
git commit -m "cli-tui: rewire apps/tui to use aleph-client"
```

---

### Task 7: Full Build Verification

**Files:** None (verification only)

- [ ] **Step 1: Check entire workspace compiles**

Run: `cargo check -p aleph-client && cargo check -p aleph-cli && cargo check -p aleph-tui`

Expected: All three compile clean.

- [ ] **Step 2: Run CLI tests if any exist**

Run: `cargo test -p aleph-cli --lib 2>&1 | tail -5`

Expected: Tests pass (or no tests found — both are OK).

- [ ] **Step 3: Run TUI tests if any exist**

Run: `cargo test -p aleph-tui --lib 2>&1 | tail -5`

Expected: Tests pass (or no tests found — both are OK).

- [ ] **Step 4: Verify no remaining references to deleted files**

Run: `grep -r "mod client;" apps/cli/src/main.rs apps/tui/src/main.rs`

Expected: No matches (the old `mod client;` declarations should be gone).

Run: `grep -rn "use crate::client\|use crate::config\|use crate::error" apps/cli/src/ apps/tui/src/`

Expected: No matches in command files or tui files. The only remaining `use crate::` should be for `output` and `tui` and `commands` modules.

- [ ] **Step 5: Commit verification pass**

No changes expected. If any fixes were needed, commit them:
```bash
git add -A && git commit -m "cli-tui: fix remaining import issues after client extraction"
```

---

## Phase 1 Complete Checklist

After all tasks:
- [ ] `apps/client/` exists as standalone crate with `connection.rs`, `config.rs`, `error.rs`
- [ ] `apps/cli/` depends on `aleph-client`, no local `client.rs/config.rs/error.rs`
- [ ] `apps/tui/` depends on `aleph-client`, no local `client.rs/config.rs/error.rs`
- [ ] All three crates compile: `cargo check -p aleph-client -p aleph-cli -p aleph-tui`
- [ ] `src/cli/` is untouched (handled in Phase 2)
- [ ] 520 lines of code duplication eliminated
