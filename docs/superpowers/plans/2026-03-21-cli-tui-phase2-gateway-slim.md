# CLI/TUI Phase 2: Gateway Slimming + CLI Command Migration

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename the gateway binary to `aleph-server`, extract `src/cli/` client code, and migrate RPC-based commands + daemon management to `apps/cli/`.

**Architecture:** The gateway binary keeps commands that access internal state directly (pairing, devices, secret, plugins, audit, start). Only RPC-thin-wrapper commands (config, cron, channels, gateway) and daemon management (stop, status) migrate to `apps/cli/`. The `src/cli/` hidden client layer (GatewayClient, OutputFormat, output helpers) gets merged into `apps/client/` (aleph-client).

**Tech Stack:** Rust, clap, tokio, aleph-client, aleph-protocol

**Spec:** `docs/reference/2026-03-20-cli-tui-separation-design.md`

> **Scope note:** The spec envisioned moving ALL commands out of the server binary. In practice, 5 commands (pairing, devices, secret, plugins, audit) directly access internal state (SecurityStore, DeviceStore, ExtensionManager, ToolRegistry) with no existing RPC methods. Creating those RPC methods is out of scope for this phase. They remain as admin subcommands of `aleph-server` until Phase 2b adds the necessary RPC endpoints.

---

## File Map

| Action | File | Purpose |
|--------|------|---------|
| **Rename** | `src/bin/aleph/` → `src/bin/aleph-server/` | Gateway binary rename |
| **Modify** | `Cargo.toml` | Update `[[bin]]` name and path |
| **Extract** | `src/cli/client.rs` → `apps/client/src/gateway_client.rs` | Simple stateless RPC client |
| **Extract** | `src/cli/output.rs` → `apps/client/src/output.rs` | Print helpers (print_table, print_json, etc.) |
| **Extract** | `src/cli/error.rs` → merge into `apps/client/src/error.rs` | Error type unification |
| **Extract** | `src/cli/config.rs` → `apps/cli/src/commands/config_cmd.rs` | Config command handlers |
| **Extract** | `src/cli/cron.rs` → `apps/cli/src/commands/cron_cmd.rs` | Cron command handlers |
| **Extract** | `src/cli/channels.rs` → `apps/cli/src/commands/channels_cmd.rs` | Channels command handlers |
| **Delete** | `src/cli/` | Remove hidden client layer from core |
| **Modify** | `src/lib.rs` | Remove `pub mod cli;` |
| **Create** | `apps/cli/src/commands/daemon.rs` (rewrite) | Daemon start/stop/status using aleph-server binary |
| **Modify** | `apps/cli/src/main.rs` | Add daemon, config, cron, channels subcommands |
| **Modify** | Server binary commands | Update imports from `alephcore::cli::` to `aleph_client::` |

---

### Task 1: Extract `src/cli/` Client Code to `aleph-client`

The `src/cli/` module contains a simpler `GatewayClient` (stateless, one-shot connections) and output formatting helpers. These need to be available to both the server binary and `apps/cli/`.

**Files:**
- Create: `apps/client/src/gateway_client.rs` (from `src/cli/client.rs`)
- Modify: `apps/client/src/error.rs` (merge core's error variants)
- Create: `apps/client/src/output.rs` (from `src/cli/output.rs`)
- Modify: `apps/client/src/lib.rs` (re-export new modules)
- Modify: `apps/client/Cargo.toml` (if needed)

- [ ] **Step 1: Read source files**

Read `src/cli/client.rs` (142 lines), `src/cli/error.rs` (24 lines), `src/cli/output.rs` (110 lines) to understand exact contents.

- [ ] **Step 2: Copy `client.rs` → `apps/client/src/gateway_client.rs`**

Copy `src/cli/client.rs` to `apps/client/src/gateway_client.rs`. Update internal imports:
- Replace `use super::error::CliError;` (or however it references CliError) with `use crate::error::CliError;`

The `GatewayClient` struct is a simple stateless WebSocket client — different from the persistent `AlephClient` in `connection.rs`. Both are useful:
- `AlephClient` = persistent connection with event streaming (for TUI/CLI interactive mode)
- `GatewayClient` = one-shot RPC calls (for CLI management commands)

- [ ] **Step 3: Merge error variants**

The `src/cli/error.rs` has different error variants than `apps/client/src/error.rs`:
- Core: `ConnectionFailed(String)`, `RpcError(String)`, `Timeout(u64)`, `InvalidResponse(String)`
- Apps: `Connection(String)`, `WebSocket(String)`, `Rpc { code, message }`, `Timeout`, `Disconnected`, `Config(String)`, `Other(String)`

The apps version is a superset. Add any missing variant from core's version into the apps version. `GatewayClient` methods should return the existing `CliError` variants.

Update `GatewayClient` to use the apps error type. Map:
- `CliError::ConnectionFailed(s)` → `CliError::Connection(s)`
- `CliError::RpcError(s)` → `CliError::Other(s)` or `CliError::Rpc { code: -1, message: s }`
- `CliError::Timeout(ms)` → `CliError::Timeout`
- `CliError::InvalidResponse(s)` → `CliError::Other(s)`

- [ ] **Step 4: Copy `output.rs` → `apps/client/src/output.rs`**

Copy `src/cli/output.rs` to `apps/client/src/output.rs`. This contains:
- `OutputFormat` enum (Table, Json)
- `print_json()`, `print_table()`, `print_list_table()`, `print_success()`, `print_error()`

No internal dependencies — just `serde::Serialize` and `std::io`.

- [ ] **Step 5: Update `apps/client/src/lib.rs`**

Add new modules and re-exports:
```rust
mod gateway_client;
mod output;

pub use gateway_client::GatewayClient;
pub use output::{OutputFormat, print_json, print_table, print_list_table, print_success, print_error};
```

- [ ] **Step 6: Verify compilation**

Run: `cargo check -p aleph-client`

Expected: Clean compilation.

- [ ] **Step 7: Commit**

```bash
git add apps/client/
git commit -m "client: extract GatewayClient and output helpers from src/cli"
```

---

### Task 2: Update Server Binary to Use `aleph-client` Instead of `alephcore::cli`

**Files:**
- Modify: `Cargo.toml` (add aleph-client dependency)
- Modify: `src/bin/aleph/commands/config.rs`
- Modify: `src/bin/aleph/commands/cron.rs`
- Modify: `src/bin/aleph/commands/channels.rs`
- Modify: `src/bin/aleph/commands/gateway.rs`

- [ ] **Step 1: Add `aleph-client` as a dependency in `Cargo.toml`**

```toml
aleph-client = { path = "../apps/client" }
```

Note: This is only for the binary target (`src/bin/aleph/`), not for the core library itself. The dependency direction is acceptable because binary targets can depend on anything.

- [ ] **Step 2: Update command imports**

In each of these files, replace `use alephcore::cli::` imports with `use aleph_client::`:
- `commands/config.rs`: `use alephcore::cli::{GatewayClient, OutputFormat, config}` → `use aleph_client::{GatewayClient, OutputFormat}`
- `commands/cron.rs`: similar
- `commands/channels.rs`: similar
- `commands/gateway.rs`: similar

Note: The config/cron/channels command HANDLERS (the functions like `config::handle_get()`) are still in `src/cli/`. We'll move them in Task 4. For now, just update the client/output imports.

Actually — wait. The commands in `src/bin/aleph/commands/` delegate to `alephcore::cli::config::handle_get()` etc. If we delete `src/cli/` before moving those handlers, the server binary breaks. So the order must be:

1. Extract client/output to aleph-client (Task 1 — done)
2. Move command handlers to apps/cli (Task 4)
3. Delete src/cli/ (Task 5)
4. Update server binary to no longer have these commands

Actually, let's restructure. The server binary's thin wrapper commands (config, cron, channels, gateway) will be REMOVED from the server binary entirely and only exist in apps/cli. The server binary keeps only: start, stop, status, pairing, devices, secret, plugins, audit.

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`

- [ ] **Step 4: Commit**

```bash
git add core/
git commit -m "core: add aleph-client dependency for server binary"
```

---

### Task 3: Rename Gateway Binary to `aleph-server`

**Files:**
- Rename: `src/bin/aleph/` → `src/bin/aleph-server/`
- Modify: `Cargo.toml` (update `[[bin]]` section)

- [ ] **Step 1: Rename directory**

```bash
mv src/bin/aleph src/bin/aleph-server
```

- [ ] **Step 2: Update `Cargo.toml`**

Change:
```toml
[[bin]]
name = "aleph"
path = "src/bin/aleph/main.rs"
```

To:
```toml
[[bin]]
name = "aleph-server"
path = "src/bin/aleph-server/main.rs"
```

- [ ] **Step 3: Update any references to the binary name**

Search for `"aleph"` binary name references in:
- `CLAUDE.md` — update `cargo run --bin aleph` to `cargo run --bin aleph-server`
- `Justfile` / `justfile` — update build/run commands
- Any scripts or docs that reference the binary name
- `apps/cli/src/commands/daemon.rs` — if it spawns the binary by name

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore --bin aleph-server`

Expected: Clean compilation.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "core: rename gateway binary from aleph to aleph-server"
```

---

### Task 4: Add RPC-Based Commands to `apps/cli/`

Move the config/cron/channels command handlers from `src/cli/` to `apps/cli/src/commands/`. These are the actual command implementations that call Gateway RPC methods.

**Files:**
- Create/Modify: `apps/cli/src/commands/config_cmd.rs` (merge with existing or replace)
- Create: `apps/cli/src/commands/cron_cmd.rs`
- Create: `apps/cli/src/commands/channels_cmd.rs`
- Modify: `apps/cli/src/main.rs` (add subcommands if not already present)
- Modify: `apps/cli/src/commands/mod.rs`

- [ ] **Step 1: Check what apps/cli already has**

Read `apps/cli/src/commands/config_cmd.rs` to see if it already implements config commands. If so, compare with `src/cli/config.rs` and merge the functionality. Same for cron and channels.

- [ ] **Step 2: Migrate config command handler**

Copy the RPC-calling logic from `src/cli/config.rs` to `apps/cli/src/commands/config_cmd.rs`. Adapt to use `aleph_client::{GatewayClient, OutputFormat, print_json, print_table, ...}` instead of `alephcore::cli::*`.

Key functions to migrate:
- `handle_get(client, path, format)` → calls `config.get` RPC
- `handle_set(client, path, value)` → calls `config.patch` RPC
- `handle_validate(client)` → calls `config.validate` RPC
- `handle_reload(client)` → calls `config.reload` RPC
- `handle_schema(client, output)` → calls `config.schema` RPC
- `handle_edit()` → spawns `$EDITOR` (local)
- `build_patch_from_path()` helper

- [ ] **Step 3: Migrate cron command handler**

Copy `src/cli/cron.rs` logic to `apps/cli/src/commands/cron_cmd.rs`:
- `handle_list(client, format)` → calls `cron.list` RPC
- `handle_status(client, format)` → calls `cron.status` RPC
- `handle_run(client, job_id)` → calls `cron.run` RPC

- [ ] **Step 4: Migrate channels command handler**

Copy `src/cli/channels.rs` logic to `apps/cli/src/commands/channels_cmd.rs`:
- `handle_list(client, format)` → calls `channels.list` RPC
- `handle_status(client, name, format)` → calls `channels.status` RPC

- [ ] **Step 5: Register new subcommands in main.rs**

If not already present, add `config`, `cron`, `channels` subcommands to the clap `Commands` enum in `apps/cli/src/main.rs`. Make sure the existing command structure is preserved.

- [ ] **Step 6: Verify compilation**

Run: `cargo check -p aleph-cli`

- [ ] **Step 7: Commit**

```bash
git add apps/cli/
git commit -m "cli: migrate config/cron/channels command handlers from core"
```

---

### Task 5: Remove `src/cli/` and Update Server Binary

Now that the client code is in `aleph-client` and the command handlers are in `apps/cli/`, remove the hidden client layer from core.

**Files:**
- Delete: `src/cli/` (entire directory)
- Modify: `src/lib.rs` (remove `pub mod cli;`)
- Modify: `src/bin/aleph-server/commands/config.rs` (remove or update)
- Modify: `src/bin/aleph-server/commands/cron.rs` (remove or update)
- Modify: `src/bin/aleph-server/commands/channels.rs` (remove or update)
- Modify: `src/bin/aleph-server/commands/gateway.rs` (update to use aleph-client)
- Modify: `src/bin/aleph-server/commands/mod.rs` (remove deleted command re-exports)

- [ ] **Step 1: Remove thin wrapper commands from server binary**

Delete or gut these files in `src/bin/aleph-server/commands/`:
- `config.rs` — config commands now live in apps/cli
- `cron.rs` — cron commands now live in apps/cli
- `channels.rs` — channels commands now live in apps/cli

Update `commands/mod.rs` to remove their re-exports.

- [ ] **Step 2: Update gateway.rs to use aleph-client**

`commands/gateway.rs` uses `GatewayClient` and `print_json`. Update imports:
```rust
use aleph_client::{GatewayClient, print_json};
```

- [ ] **Step 3: Update cli.rs to remove migrated subcommands**

In `src/bin/aleph-server/cli.rs`, remove the `ConfigAction`, `ChannelsAction`, `CronAction` enums and their corresponding `Command` variants. Keep: Start, Stop, Status, Pairing, Devices, Plugins, Plugin, Gateway, Audit, Secret.

- [ ] **Step 4: Update main.rs to remove migrated command handling**

Remove the match arms for Config, Channels, Cron in the command dispatcher.

- [ ] **Step 5: Delete `src/cli/`**

```bash
rm -rf src/cli/
```

- [ ] **Step 6: Remove `pub mod cli;` from `src/lib.rs`**

- [ ] **Step 7: Verify compilation**

Run: `cargo check -p alephcore --bin aleph-server && cargo check -p aleph-cli`

Both must compile clean.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "core: remove cli module, migrate thin wrapper commands to apps/cli"
```

---

### Task 6: Update Daemon Management in `apps/cli/`

The existing `apps/cli/src/commands/daemon.rs` currently just calls the gateway RPC methods for daemon management. Rewrite it to manage the `aleph-server` process directly (spawn, stop via PID, check status).

**Files:**
- Modify: `apps/cli/src/commands/daemon.rs`
- Reference: `src/bin/aleph-server/daemon.rs` (for PID file and process management patterns)

- [ ] **Step 1: Read existing daemon.rs in both locations**

Read `apps/cli/src/commands/daemon.rs` and `src/bin/aleph-server/daemon.rs` to understand current implementations.

- [ ] **Step 2: Implement `daemon start`**

`aleph daemon start [--port N] [--daemon]` should:
1. Check if aleph-server is already running (read PID file)
2. Locate the `aleph-server` binary (same directory as `aleph`, or PATH)
3. Spawn `aleph-server` with appropriate flags
4. If `--daemon`, detach the process
5. Print status message

- [ ] **Step 3: Implement `daemon stop`**

Copy the stop logic from `src/bin/aleph-server/daemon.rs`:
1. Read PID from `~/.aleph/aleph.pid`
2. Send SIGTERM
3. Wait up to 5 seconds
4. If still running, send SIGKILL
5. Remove PID file

- [ ] **Step 4: Implement `daemon status`**

Copy the status logic:
1. Read PID from file
2. Check if process is running
3. Print status (running/stopped)

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p aleph-cli`

- [ ] **Step 6: Commit**

```bash
git add apps/cli/src/commands/daemon.rs
git commit -m "cli: rewrite daemon management to spawn aleph-server binary"
```

---

### Task 7: Update Documentation and Build Commands

**Files:**
- Modify: `CLAUDE.md`
- Modify: `justfile` (if exists)
- Modify: Any scripts referencing `aleph` binary

- [ ] **Step 1: Update CLAUDE.md**

Update build commands table:
- `cargo run --bin aleph` → `cargo run --bin aleph-server`
- Add note about `aleph` now being the CLI binary from `apps/cli/`

Update process management section:
- `pkill -f "target/release/aleph"` → `pkill -f "target/release/aleph-server"`
- `pkill -f "target/debug/aleph"` → `pkill -f "target/debug/aleph-server"`

- [ ] **Step 2: Update justfile**

If `just dev` or `just build` reference the binary name, update them.

- [ ] **Step 3: Verify**

Run: `cargo build -p alephcore --bin aleph-server 2>&1 | tail -3`

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md justfile
git commit -m "docs: update binary name references for aleph-server rename"
```

---

### Task 8: Full Build Verification

- [ ] **Step 1: Build all three crates**

```bash
cargo check -p aleph-client && cargo check -p aleph-cli && cargo check -p alephcore --bin aleph-server
```

- [ ] **Step 2: Run core tests**

```bash
cargo test -p alephcore --lib 2>&1 | tail -10
```

Expected: Tests pass (the `src/cli/` removal should not break lib tests since cli was only used by the binary).

- [ ] **Step 3: Verify no stale `alephcore::cli` imports**

```bash
grep -rn "alephcore::cli" src/bin/ apps/
```

Expected: No matches.

- [ ] **Step 4: Verify binary builds**

```bash
cargo build -p alephcore --bin aleph-server 2>&1 | tail -3
cargo build -p aleph-cli --bin aleph 2>&1 | tail -3
```

---

## Phase 2 Complete Checklist

- [ ] Gateway binary renamed to `aleph-server`
- [ ] `src/cli/` deleted — no hidden client layer in server crate
- [ ] `GatewayClient` and `OutputFormat` available from `aleph-client`
- [ ] Config/cron/channels commands available in `apps/cli/`
- [ ] `apps/cli/` daemon commands manage `aleph-server` process
- [ ] `src/lib.rs` no longer exports `pub mod cli;`
- [ ] All three crates compile clean
- [ ] Core tests pass
