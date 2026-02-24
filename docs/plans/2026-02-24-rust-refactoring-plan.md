# Rust Core Refactoring Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Eliminate 343 Clippy warnings, convert EventHandler trait to zero-allocation &str parameters, and replace 81 handler registrations with a DRY macro — all without changing external behavior.

**Architecture:** 4-wave risk progression. Each wave is independently compilable and testable. Wave 1-2 are mechanical Clippy fixes. Wave 3 changes the InternalEventHandler trait signature. Wave 4 introduces a `register_handler!` macro to eliminate boilerplate.

**Tech Stack:** Rust, cargo clippy, declarative macros

**Design Doc:** `docs/plans/2026-02-24-rust-refactoring-design.md`

---

## Task 1: Wave 1 — Automatic Clippy Fix

**Files:**
- Modify: ~60+ files across `core/src/` (auto-applied by cargo clippy --fix)

**Step 1: Run cargo clippy --fix**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core
cargo clippy --fix --allow-dirty --all-targets 2>&1 | tee /tmp/clippy-fix-output.txt
```

Expected: Auto-fixes for unused_imports (62+), redundant_closure (6), let_and_return (11), len_zero (10), bool_comparison (4), clone_on_copy (3), manual_range_contains (9), redundant_pattern_matching (2), identity_map (1).

**Step 2: Verify compilation**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core
cargo build 2>&1
```

Expected: Build succeeds with fewer warnings.

**Step 3: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core
cargo test --lib 2>&1
```

Expected: All tests pass. Zero behavior change from syntax-only transformations.

**Step 4: Check remaining Clippy warnings**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core
cargo clippy --all-targets 2>&1 | grep "warning\[" | sort | uniq -c | sort -rn | head -20
```

Expected: Remaining warnings are the ones needing manual fixes (Default::default, expect_fun_call, etc.).

**Step 5: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core
git add -A
git commit -m "refactor(core): auto-fix Clippy warnings (Wave 1)

Applied cargo clippy --fix for: unused_imports, redundant_closure,
let_and_return, len_zero, bool_comparison, clone_on_copy,
manual_range_contains, redundant_pattern_matching, identity_map.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 2: Wave 2A — Fix Default::default() Field Reassignment (75 locations)

**Files:**
- Modify: Files flagged by `clippy::field_reassign_with_default`

**Step 1: Find all locations**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core
cargo clippy --all-targets 2>&1 | grep "field_reassign_with_default" | grep -oP '(src/[^:]+:\d+)' | sort -u
```

Expected: ~75 file:line locations.

**Step 2: Fix each location**

Pattern: Convert sequential field assignments to struct literal with `..Default::default()`.

```rust
// BEFORE
let mut config = SomeStruct::default();
config.field_a = value_a;
config.field_b = value_b;

// AFTER
let config = SomeStruct {
    field_a: value_a,
    field_b: value_b,
    ..SomeStruct::default()
};
```

**Important:** If the struct is later mutated (not just initialized), keep `let mut` and only convert the initialization part. Check for subsequent mutations like `config.field_c = something_computed_later;` — if those exist, either include them in the literal or keep the original mutable pattern.

**Step 3: Verify compilation**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core
cargo build 2>&1
```

**Step 4: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core
cargo test --lib 2>&1
```

**Step 5: Commit**

```bash
git add -A
git commit -m "refactor(core): replace Default::default() field reassignment with struct literals (Wave 2A)

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 3: Wave 2B — Fix expect(format!()), useless_vec, derivable_impls, ptr_arg, type_complexity

**Files:**
- Modify: Files flagged by the following Clippy lints

**Step 1: Fix expect_fun_call (12 locations)**

Find locations:
```bash
cd /Volumes/TBU4/Workspace/Aleph/core
cargo clippy --all-targets 2>&1 | grep "expect_fun_call"
```

Pattern:
```rust
// BEFORE
.expect(&format!("missing key: {}", name))

// AFTER
.unwrap_or_else(|| panic!("missing key: {name}"))
```

**Step 2: Fix useless_vec (9 locations)**

Find locations:
```bash
cargo clippy --all-targets 2>&1 | grep "useless_vec"
```

Pattern:
```rust
// BEFORE
let args = vec!["build", "--release"];
command.args(&args);

// AFTER
command.args(&["build", "--release"]);
```

**Note:** Only convert when the vec is used once as a temporary. If the vec is passed to multiple consumers or mutated, keep it.

**Step 3: Fix derivable_impls (4 locations)**

Find locations:
```bash
cargo clippy --all-targets 2>&1 | grep "derivable_impls"
```

Pattern: Replace manual `impl Default for T { fn default() -> Self { Self { field: Default::default(), ... } } }` with `#[derive(Default)]` on the struct.

**Step 4: Fix ptr_arg — &PathBuf → &Path (3 locations)**

Find locations:
```bash
cargo clippy --all-targets 2>&1 | grep "ptr_arg"
```

Pattern:
```rust
// BEFORE
fn load(path: &PathBuf) -> Result<()>

// AFTER
use std::path::Path;
fn load(path: &Path) -> Result<()>
```

**Note:** Also update any callers that explicitly pass `&PathBuf` — they auto-deref so no caller changes needed.

**Step 5: Fix type_complexity (7 locations)**

Find locations:
```bash
cargo clippy --all-targets 2>&1 | grep "type_complexity"
```

Pattern: Extract a `type` alias.
```rust
// BEFORE
fn process() -> HashMap<String, Vec<(Arc<Mutex<Box<dyn Trait>>>, usize)>> { ... }

// AFTER
type ProcessResult = HashMap<String, Vec<(Arc<Mutex<Box<dyn Trait>>>, usize)>>;
fn process() -> ProcessResult { ... }
```

Place the type alias in the same module, with `pub(crate)` visibility if needed externally.

**Step 6: Fix module_inception (5 locations)**

Find locations:
```bash
cargo clippy --all-targets 2>&1 | grep "module_inception"
```

Suppress with `#[allow(clippy::module_inception)]` if renaming would break too many imports. Otherwise rename the inner module.

**Step 7: Verify and commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core
cargo build 2>&1
cargo test --lib 2>&1
git add -A
git commit -m "refactor(core): manual Clippy fixes — expect_fun_call, useless_vec, derivable_impls, ptr_arg, type_complexity, module_inception (Wave 2B)

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 4: Wave 3A — Convert InternalEventHandler Trait String → &str

**Files:**
- Modify: `core/src/event_handler.rs` (trait definition at line ~104, MockEventHandler impl at line ~538)

**Step 1: Change the trait definition**

File: `core/src/event_handler.rs`

Change every method that accepts `String` to accept `&str`. Change every `Option<String>` to `Option<&str>`. The following methods need changes:

```rust
// In pub trait InternalEventHandler: Send + Sync { ... }

// These change:
fn on_error(&self, message: &str, suggestion: Option<&str>);
fn on_response_chunk(&self, text: &str);
fn on_error_typed(&self, error_type: ErrorType, message: &str);
fn on_ai_processing_started(&self, provider_name: &str, provider_color: &str);
fn on_ai_response_received(&self, response_preview: &str);
fn on_provider_fallback(&self, from_provider: &str, to_provider: &str);
fn on_conversation_started(&self, session_id: &str);
fn on_conversation_ended(&self, session_id: &str, total_turns: u32);
fn on_confirmation_expired(&self, confirmation_id: &str);
fn on_agent_started(&self, plan_id: &str, total_steps: u32, description: &str);
fn on_agent_tool_started(&self, plan_id: &str, step_index: u32, tool_name: &str, tool_description: &str);
fn on_agent_tool_completed(&self, plan_id: &str, step_index: u32, tool_name: &str, success: bool, result_preview: &str);
fn on_agent_completed(&self, plan_id: &str, success: bool, total_duration_ms: u64, final_response: &str);

// These stay unchanged (struct parameters):
fn on_state_changed(&self, state: ProcessingState);
fn on_progress(&self, percent: f32);
fn on_config_changed(&self);
fn on_typewriter_progress(&self, percent: f32);
fn on_typewriter_cancelled(&self);
fn on_clarification_needed(&self, request: ClarificationRequest) -> ClarificationResult;
fn on_conversation_turn_completed(&self, turn: crate::conversation::ConversationTurn);
fn on_conversation_continuation_ready(&self);
fn on_confirmation_needed(&self, confirmation: PendingConfirmationInfo);
fn on_tools_changed(&self, tool_count: u32);
fn on_tools_refresh_needed(&self);
fn on_mcp_startup_complete(&self, report: McpStartupReport);
```

**Step 2: Update MockEventHandler impl**

In the same file, update the `impl InternalEventHandler for MockEventHandler` block. The mock stores values in `Arc<Mutex<Vec<String>>>`, so it needs to call `.to_owned()` on `&str` before pushing:

```rust
// BEFORE
fn on_error(&self, message: String, suggestion: Option<String>) {
    self.errors.lock().unwrap().push(message);
    // ...
}

// AFTER
fn on_error(&self, message: &str, suggestion: Option<&str>) {
    self.errors.lock().unwrap().push(message.to_owned());
    // ...
}
```

Apply this pattern to ALL 13 changed methods. For tuple pushes:
```rust
// BEFORE
fn on_ai_processing_started(&self, provider_name: String, provider_color: String) {
    self.ai_processing_started.lock().unwrap().push((provider_name, provider_color));
}

// AFTER
fn on_ai_processing_started(&self, provider_name: &str, provider_color: &str) {
    self.ai_processing_started.lock().unwrap().push((provider_name.to_owned(), provider_color.to_owned()));
}
```

**Step 3: Attempt compilation — let compiler find all callers**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core
cargo build 2>&1 | grep "error\[" | head -50
```

Expected: Compiler errors at every call site that passes `String` where `&str` is now expected. Most will auto-resolve because `String` derefs to `&str`. Errors will occur where callers build strings and pass ownership — they can now pass `&string_var` or `&format!(...)`.

**Step 4: Fix all compiler errors**

For each error, the fix is usually:
```rust
// If caller had:
handler.on_error(error_message, Some(suggestion));
// where error_message: String, suggestion: String

// Change to:
handler.on_error(&error_message, Some(&suggestion));
// or if it was a literal:
handler.on_error("something failed", Some("try again"));
```

**Step 5: Verify**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core
cargo build 2>&1
cargo test --lib event_handler 2>&1
cargo test --lib 2>&1
```

**Step 6: Commit**

```bash
git add -A
git commit -m "refactor(core): convert InternalEventHandler String params to &str (Wave 3A)

Eliminates heap allocation on every event emission. Only the test mock
(MockEventHandler) now calls .to_owned() for storage — production callers
pass borrowed references with zero allocation.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 5: Wave 3B — Fix &Vec Anti-Pattern

**Files:**
- Modify: `core/src/memory/store/lance/arrow_convert.rs` (5 locations)
- Modify: `core/src/resilience/recovery/shadow_replay.rs` (3 locations)

**Step 1: Fix arrow_convert.rs**

File: `core/src/memory/store/lance/arrow_convert.rs`

Change the function signature at line ~46:
```rust
// BEFORE
fn build_vector_column(embeddings: &[Option<&Vec<f32>>], dim: i32) -> Result<FixedSizeListArray, AlephError>

// AFTER
fn build_vector_column(embeddings: &[Option<&[f32]>], dim: i32) -> Result<FixedSizeListArray, AlephError>
```

Also update local variable types at lines ~221, ~231, ~241, ~622:
```rust
// BEFORE
let embeddings_384: Vec<Option<&Vec<f32>>> = ...

// AFTER
let embeddings_384: Vec<Option<&[f32]>> = ...
```

The `.as_slice()` or `&**v` patterns at call sites may need adjustment. Since `&Vec<f32>` auto-derefs to `&[f32]`, most callers won't need changes. Check with:

```bash
cargo build 2>&1 | grep "arrow_convert"
```

**Step 2: Fix shadow_replay.rs**

File: `core/src/resilience/recovery/shadow_replay.rs`

At lines ~116, ~164, ~275:
```rust
// BEFORE
let tool_calls: &Vec<ToolCall> = calls;
// or
|calls: &Vec<ToolCall>| { ... }

// AFTER
let tool_calls: &[ToolCall] = calls;
// or
|calls: &[ToolCall]| { ... }
```

**Step 3: Verify and commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core
cargo build 2>&1
cargo test --lib 2>&1
git add -A
git commit -m "refactor(core): replace &Vec<T> with &[T] in arrow_convert and shadow_replay (Wave 3B)

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 6: Wave 4 — Handler Registration Macro

**Files:**
- Modify: `core/src/bin/aleph_server/commands/start/builder/handlers.rs`

**Step 1: Add the register_handler! macro at the top of handlers.rs**

Insert after the imports, before the first function:

```rust
/// Register a JSON-RPC handler with shared context via Arc.
///
/// Eliminates the repeated clone-into-closure boilerplate.
/// Supports 0, 1, or 2 context arguments.
macro_rules! register_handler {
    // No context args (stateless handler)
    ($server:expr, $method:expr, $handler:path) => {{
        $server.handlers_mut().register($method, |req| async move {
            $handler(req).await
        });
    }};
    // 1 context arg
    ($server:expr, $method:expr, $handler:path, $ctx1:expr) => {{
        let ctx1 = ::std::sync::Arc::clone(&$ctx1);
        $server.handlers_mut().register($method, move |req| {
            let ctx1 = ::std::sync::Arc::clone(&ctx1);
            async move { $handler(req, ctx1).await }
        });
    }};
    // 2 context args
    ($server:expr, $method:expr, $handler:path, $ctx1:expr, $ctx2:expr) => {{
        let ctx1 = ::std::sync::Arc::clone(&$ctx1);
        let ctx2 = ::std::sync::Arc::clone(&$ctx2);
        $server.handlers_mut().register($method, move |req| {
            let ctx1 = ::std::sync::Arc::clone(&ctx1);
            let ctx2 = ::std::sync::Arc::clone(&ctx2);
            async move { $handler(req, ctx1, ctx2).await }
        });
    }};
}
```

**Step 2: Convert register_auth_handlers (6 handlers, all 1-arg)**

```rust
#[cfg(feature = "gateway")]
pub(in crate::commands::start) fn register_auth_handlers(
    server: &mut GatewayServer,
    auth_ctx: &Arc<auth_handlers::AuthContext>,
) {
    register_handler!(server, "connect",         auth_handlers::handle_connect,         auth_ctx);
    register_handler!(server, "pairing.approve", auth_handlers::handle_pairing_approve, auth_ctx);
    register_handler!(server, "pairing.reject",  auth_handlers::handle_pairing_reject,  auth_ctx);
    register_handler!(server, "pairing.list",    auth_handlers::handle_pairing_list,    auth_ctx);
    register_handler!(server, "devices.list",    auth_handlers::handle_devices_list,    auth_ctx);
    register_handler!(server, "devices.revoke",  auth_handlers::handle_devices_revoke,  auth_ctx);
}
```

**Step 3: Verify first function compiles**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core
cargo build 2>&1 | head -20
```

Expected: Clean build for register_auth_handlers.

**Step 4: Convert register_guest_handlers (6 handlers, mixed 1-arg and 2-arg)**

```rust
#[cfg(feature = "gateway")]
pub(in crate::commands::start) fn register_guest_handlers(
    server: &mut GatewayServer,
    invitation_manager: &Arc<alephcore::gateway::security::InvitationManager>,
    session_manager: &Arc<alephcore::gateway::security::GuestSessionManager>,
    event_bus: &Arc<alephcore::gateway::event_bus::GatewayEventBus>,
) {
    use alephcore::gateway::handlers::guests;

    register_handler!(server, "guests.createInvitation",  guests::handle_create_invitation,  invitation_manager, event_bus);
    register_handler!(server, "guests.listPending",       guests::handle_list_guests,         invitation_manager);
    register_handler!(server, "guests.revokeInvitation",  guests::handle_revoke_invitation,  invitation_manager, event_bus);
    register_handler!(server, "guests.listSessions",      guests::handle_list_sessions,       session_manager);
    register_handler!(server, "guests.terminateSession",  guests::handle_terminate_session,  session_manager, event_bus);
    register_handler!(server, "guests.getActivityLogs",   guests::handle_get_activity_logs,   session_manager);
}
```

**Step 5: Convert register_session_handlers (4 handlers, all 1-arg)**

```rust
#[cfg(feature = "gateway")]
pub(in crate::commands::start) fn register_session_handlers(
    server: &mut GatewayServer,
    session_manager: &Arc<SessionManager>,
    daemon: bool,
) {
    register_handler!(server, "sessions.list",    session_handlers::handle_list_db,    session_manager);
    register_handler!(server, "sessions.history", session_handlers::handle_history_db, session_manager);
    register_handler!(server, "sessions.reset",   session_handlers::handle_reset_db,   session_manager);
    register_handler!(server, "sessions.delete",  session_handlers::handle_delete_db,  session_manager);

    if !daemon {
        println!("Session methods:");
        println!("  - sessions.list   : List all sessions");
        println!("  - sessions.history: Get session message history");
        println!("  - sessions.reset  : Clear session messages");
        println!("  - sessions.delete : Delete a session");
        println!();
    }
}
```

**Step 6: Convert register_channel_handlers (6 gateway + 6 discord handlers)**

```rust
#[cfg(feature = "gateway")]
pub(in crate::commands::start) fn register_channel_handlers(
    server: &mut GatewayServer,
    channel_registry: &Arc<ChannelRegistry>,
) {
    register_handler!(server, "channels.list",       channel_handlers::handle_list,         channel_registry);
    register_handler!(server, "channels.status",     channel_handlers::handle_status,       channel_registry);
    register_handler!(server, "channel.start",       channel_handlers::handle_start,        channel_registry);
    register_handler!(server, "channel.stop",        channel_handlers::handle_stop,         channel_registry);
    register_handler!(server, "channel.pairing_data", channel_handlers::handle_pairing_data, channel_registry);
    register_handler!(server, "channel.send",        channel_handlers::handle_send,         channel_registry);

    #[cfg(feature = "discord")]
    {
        register_handler!(server, "discord.validate_token",    discord_panel_handlers::handle_validate_token);
        register_handler!(server, "discord.save_config",       discord_panel_handlers::handle_save_config);
        register_handler!(server, "discord.list_guilds",       discord_panel_handlers::handle_list_guilds,       channel_registry);
        register_handler!(server, "discord.list_channels",     discord_panel_handlers::handle_list_channels,     channel_registry);
        register_handler!(server, "discord.audit_permissions", discord_panel_handlers::handle_audit_permissions, channel_registry);
        register_handler!(server, "discord.update_allowlists", discord_panel_handlers::handle_update_allowlists, channel_registry);
    }
}
```

**Step 7: Convert all remaining register functions**

Apply the same pattern to:
- `setup_config_watcher` (config.reload, config.get, config.validate, config.path — all 1-arg)
- `register_config_handlers` and any other register_* functions in the file

For all remaining handlers, read each registration block and convert using the appropriate macro arity:
- 0 args: `register_handler!(server, "method", handler::func);`
- 1 arg: `register_handler!(server, "method", handler::func, ctx);`
- 2 args: `register_handler!(server, "method", handler::func, ctx1, ctx2);`

**Step 8: Remove unused `use std::sync::Arc;` if the macro uses fully qualified path**

Check if the `Arc` import is still needed for other code in the file. If only the macro uses Arc, the fully-qualified `::std::sync::Arc::clone` in the macro means the import can be removed.

**Step 9: Verify and commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core
cargo build 2>&1
cargo test --lib 2>&1
git add -A
git commit -m "refactor(core): introduce register_handler! macro, eliminate handler boilerplate (Wave 4)

Replaces 81 handler registrations (6-line pattern each) with 1-line macro
invocations. Reduces handlers.rs from ~900 lines to ~200 lines (-78%).
Macro expansion is identical to hand-written code — zero behavior change.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 7: Final Validation

**Step 1: Run full Clippy check**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core
cargo clippy --all-targets 2>&1 | grep -c "warning\["
```

Expected: Significantly fewer warnings than the original 343. Remaining should be only the out-of-scope items (Arc<non-Send+Sync>, etc.).

**Step 2: Run full test suite**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core
cargo test --lib 2>&1
```

Expected: All tests pass.

**Step 3: Verify git log**

```bash
git log --oneline -6
```

Expected: 6 clean commits (Wave 1, 2A, 2B, 3A, 3B, 4).

---

## Summary

| Task | Wave | Risk | Key Action |
|------|------|------|------------|
| 1 | 1 | Zero | `cargo clippy --fix` auto-fix ~100 warnings |
| 2 | 2A | Low | Default::default() → struct literal (75 locations) |
| 3 | 2B | Low | expect_fun_call, useless_vec, derivable_impls, ptr_arg, type_complexity |
| 4 | 3A | Medium | InternalEventHandler String → &str (13 methods, 1 impl) |
| 5 | 3B | Low | &Vec<T> → &[T] (8 locations in 2 files) |
| 6 | 4 | Medium | register_handler! macro (81 handlers in 1 file) |
| 7 | — | — | Final validation |
