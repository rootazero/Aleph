# CLI/TUI Phase 3: TUI Refactor

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate 4,262 lines of duplicate TUI code, make CLI's `chat` command use the standalone TUI, and integrate TUI with Gateway's command system via RPC.

**Architecture:** Convert `interfaces/tui/` from binary-only to lib+bin crate. CLI's `chat` command calls `aleph_tui::run()` as a library function. TUI's hardcoded `SlashCommand` enum replaced by Gateway's `commands.list` RPC for command discovery and `command.execute` RPC for execution.

**Tech Stack:** Rust, ratatui, crossterm, aleph-client, aleph-protocol

**Spec:** `docs/reference/2026-03-20-cli-tui-separation-design.md`

---

## Phase 3a: Deduplicate TUI Code

### Task 1: Convert `interfaces/tui/` to lib+bin Crate

The standalone TUI is binary-only. We need to add a lib target so CLI can call it.

**Files:**
- Modify: `interfaces/tui/Cargo.toml` (add `[lib]` section)
- Create: `interfaces/tui/src/lib.rs` (public API)
- Modify: `interfaces/tui/src/main.rs` (use lib API)

- [ ] **Step 1: Add `[lib]` section to Cargo.toml**

Add before `[[bin]]`:
```toml
[lib]
name = "aleph_tui"
path = "src/lib.rs"
```

- [ ] **Step 2: Create `src/lib.rs`**

Extract the public API from `main.rs`. The TUI's entry point is `tui::run()`. Create a thin lib.rs:

```rust
//! Aleph TUI Library
//!
//! Interactive terminal interface for Aleph Gateway.
//! Can be used as a library (from CLI's `chat` command) or standalone binary.

pub mod tui;

pub use aleph_client::{AlephClient, CliConfig, CliResult};

/// Launch the interactive TUI.
///
/// Connects to Gateway, authenticates, and enters the ratatui event loop.
/// Returns when the user quits.
pub async fn run(
    server_url: &str,
    agent: Option<&str>,
    session: Option<&str>,
    config: &CliConfig,
    verbose: bool,
) -> CliResult<()> {
    // Connect to gateway
    let (client, events) = AlephClient::connect(server_url).await?;

    // Authenticate
    client.authenticate(config).await?;

    // Determine session key
    let session_key = session
        .map(String::from)
        .or_else(|| config.default_session.clone())
        .unwrap_or_else(|| {
            format!(
                "chat-{}",
                uuid::Uuid::new_v4()
                    .to_string()
                    .split('-')
                    .next()
                    .unwrap_or("0000")
            )
        });

    // Launch TUI
    tui::run(client, events, config, session_key).await
}
```

- [ ] **Step 3: Simplify `main.rs` to use lib**

Replace the current `main.rs` (90 lines) with:
```rust
use aleph_tui::CliResult;
use clap::Parser;

#[derive(Parser)]
#[command(name = "aleph-tui")]
struct Args {
    #[arg(short, long, default_value = "ws://127.0.0.1:18789")]
    server: String,
    #[arg(short = 'k', long)]
    session: Option<String>,
    #[arg(long)]
    agent: Option<String>,
    #[arg(short, long)]
    verbose: bool,
    #[arg(short, long)]
    config: Option<String>,
}

#[tokio::main]
async fn main() -> CliResult<()> {
    let args = Args::parse();
    let config = aleph_tui::CliConfig::load(args.config.as_deref())?;
    aleph_tui::run(&args.server, args.agent.as_deref(), args.session.as_deref(), &config, args.verbose).await
}
```

- [ ] **Step 4: Make `tui` module public**

In `interfaces/tui/src/tui/mod.rs`, ensure the `run` function is `pub`:
```rust
pub async fn run(...) -> CliResult<()>
```

Check all types used in the `run` signature are also `pub`.

- [ ] **Step 5: Verify TUI compiles as both lib and bin**

Run: `cargo check -p aleph-tui --lib && cargo check -p aleph-tui --bin aleph-tui`

- [ ] **Step 6: Commit**

```bash
git add interfaces/tui/
git commit -m "tui: convert to lib+bin crate with public run() API"
```

---

### Task 2: Rewire CLI `chat` Command to Use TUI Library

**Files:**
- Modify: `interfaces/cli/Cargo.toml` (add aleph-tui dependency)
- Modify: `interfaces/cli/src/commands/chat.rs` (call aleph_tui::run)
- Modify: `interfaces/cli/src/main.rs` (remove `mod tui;`, add --agent flag to chat)

- [ ] **Step 1: Add aleph-tui dependency**

In `interfaces/cli/Cargo.toml`:
```toml
aleph-tui = { path = "../tui" }
```

- [ ] **Step 2: Rewrite chat.rs**

Replace the current implementation that calls `crate::tui::run()` with a call to `aleph_tui::run()`:

```rust
use aleph_client::CliResult;

pub async fn run(
    server_url: &str,
    agent: Option<&str>,
    session: Option<&str>,
    config: &aleph_client::CliConfig,
    verbose: bool,
) -> CliResult<()> {
    aleph_tui::run(server_url, agent, session, config, verbose).await
}
```

- [ ] **Step 3: Update main.rs**

Add `--agent` flag to the Chat command variant if not already present.
Update the chat match arm to pass agent.

- [ ] **Step 4: Verify CLI compiles**

Run: `cargo check -p aleph-cli`

- [ ] **Step 5: Commit**

```bash
git add interfaces/cli/
git commit -m "cli: rewire chat command to use aleph-tui library"
```

---

### Task 3: Delete Embedded TUI from CLI

**Files:**
- Delete: `interfaces/cli/src/tui/` (entire directory — 4,262 lines)
- Modify: `interfaces/cli/src/main.rs` (remove `mod tui;`)
- Modify: `interfaces/cli/Cargo.toml` (remove TUI-only deps if unused elsewhere)

- [ ] **Step 1: Remove `mod tui;` from main.rs**

- [ ] **Step 2: Verify compilation still works**

Run: `cargo check -p aleph-cli`

The embedded TUI should now be unreachable code since chat.rs calls aleph_tui instead.

- [ ] **Step 3: Delete the directory**

```bash
rm -rf interfaces/cli/src/tui/
```

- [ ] **Step 4: Remove unused dependencies from CLI Cargo.toml**

Check if these are still used elsewhere in CLI, remove if not:
- `crossterm` — only used by embedded TUI
- `ratatui` — only used by embedded TUI
- `tui-textarea` — only used by embedded TUI
- `unicode-width` — might be used by output formatting
- `textwrap` — might be used by output formatting

Be conservative — only remove if `cargo check` confirms they're unused.

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p aleph-cli`

- [ ] **Step 6: Commit**

```bash
git add -A interfaces/cli/
git commit -m "cli: delete embedded TUI code (4,262 lines), use aleph-tui library"
```

---

## Phase 3b: Gateway Command Integration

### Task 4: Add `commands.list` RPC Method

**Files:**
- Create: `src/gateway/handlers/command_handlers.rs`
- Modify: `src/gateway/handlers/mod.rs` (register module)
- Modify: `src/bin/aleph-server/commands/start/builder/handlers.rs` (register handler)

- [ ] **Step 1: Create command handler module**

Create `src/gateway/handlers/command_handlers.rs`:

The `commands.list` handler should:
1. Accept `{ "interface": "tui" }` parameter (optional)
2. Query `ToolRegistry` for all registered tools/commands
3. Map each tool to: `{ name, hint, source_type, arguments_schema }`
4. Return as JSON array

Read `src/dispatcher/mod.rs` (ToolRegistry) to understand how to list tools.
Look at existing handler patterns (e.g., `session_handlers.rs`) for the RPC handler signature.

- [ ] **Step 2: Register the handler**

In `src/bin/aleph-server/commands/start/builder/handlers.rs`, add:
```rust
register_handler!(server, "commands.list", command_handlers::list, tool_registry_clone);
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore --bin aleph-server`

- [ ] **Step 4: Commit**

```bash
git add core/
git commit -m "gateway: add commands.list RPC method"
```

---

### Task 5: Add `command.execute` RPC Method

**Files:**
- Modify: `src/gateway/handlers/command_handlers.rs`
- Modify: `src/bin/aleph-server/commands/start/builder/handlers.rs`

- [ ] **Step 1: Add execute handler**

The `command.execute` handler should:
1. Accept `{ "input": "session new my-topic", "session_id": "..." }`
2. Parse input via existing `CommandParser.parse_async()`
3. Execute via existing fast-path (direct_tool / skill / mcp routing)
4. Return execution result

Read `src/gateway/inbound_router/mod.rs` to understand how slash commands are currently detected and routed in the message flow. The `command.execute` handler should reuse that logic.

- [ ] **Step 2: Register the handler**

```rust
register_handler!(server, "command.execute", command_handlers::execute, ...needed_contexts...);
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore --bin aleph-server`

- [ ] **Step 4: Commit**

```bash
git add core/
git commit -m "gateway: add command.execute RPC method"
```

---

### Task 6: TUI Uses Gateway Commands Instead of Hardcoded Enum

**Files:**
- Modify: `interfaces/tui/src/tui/slash.rs` (replace hardcoded enum with RPC)
- Modify: `interfaces/tui/src/tui/mod.rs` (startup command fetch)
- Modify: `interfaces/tui/src/tui/app.rs` (store command list)
- Modify: `interfaces/tui/src/tui/widgets/command_palette.rs` (use dynamic list)

- [ ] **Step 1: Add command fetch at TUI startup**

In `tui/mod.rs` or `app.rs`, after connecting and authenticating, call:
```rust
let commands = client.call::<Vec<CommandEntry>>("commands.list", json!({"interface": "tui"})).await
    .unwrap_or_default(); // graceful degradation if Gateway is old
```

Store in `AppState`.

- [ ] **Step 2: Replace hardcoded SlashCommand enum**

In `slash.rs`, keep only local commands (`/clear`, `/quit`, `/verbose`). All other commands go through `command.execute` RPC.

The slash parser becomes:
1. Check local commands first
2. If not local, send to Gateway via `command.execute`

- [ ] **Step 3: Update command palette widget**

`command_palette.rs` should use the dynamic command list from `AppState` for autocompletion instead of the hardcoded enum variants.

- [ ] **Step 4: Verify TUI compiles**

Run: `cargo check -p aleph-tui`

- [ ] **Step 5: Commit**

```bash
git add interfaces/tui/
git commit -m "tui: replace hardcoded slash commands with Gateway RPC integration"
```

---

### Task 7: Full Verification

- [ ] **Step 1: Build all crates**

```bash
cargo check -p aleph-client && cargo check -p aleph-cli && cargo check -p aleph-tui && cargo check -p alephcore --bin aleph-server
```

- [ ] **Step 2: Run core tests**

```bash
cargo test -p alephcore --lib 2>&1 | tail -5
```

- [ ] **Step 3: Verify no embedded TUI remains**

```bash
ls interfaces/cli/src/tui/ 2>&1  # should fail: No such directory
grep -rn "mod tui" interfaces/cli/src/main.rs  # should find nothing
```

- [ ] **Step 4: Verify CLI chat still compiles**

```bash
grep "aleph_tui::run" interfaces/cli/src/commands/chat.rs  # should find the call
```

---

## Phase 3 Complete Checklist

- [ ] `interfaces/tui/` is lib+bin crate with public `run()` API
- [ ] `interfaces/cli/src/tui/` deleted (4,262 lines removed)
- [ ] CLI `chat` command calls `aleph_tui::run()`
- [ ] Gateway has `commands.list` RPC method
- [ ] Gateway has `command.execute` RPC method
- [ ] TUI uses Gateway commands instead of hardcoded SlashCommand enum
- [ ] All crates compile, core tests pass
