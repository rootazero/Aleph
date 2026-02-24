# Rust Core Refactoring Design

> Date: 2026-02-24
> Status: Approved
> Scope: Clippy + Idiomatic Rust + Performance Hotspots
> Strategy: Wave-based risk progression (4 waves)

---

## Context

The Aleph Rust core (`core/src/`) consists of 1,215 files totaling 378,627 lines. A comprehensive audit identified:

- **343 Clippy warnings** across multiple categories
- **3,726 `.clone()` calls** with 221 concentrated in `handlers.rs`
- **EventHandler trait** accepting owned `String` parameters (50+ methods) causing unnecessary heap allocations on every event emission
- **8 `&Vec<T>` anti-patterns** where `&[T]` suffices
- **9,302 `.to_string()` calls** spread across the codebase

### Out of Scope

- Large file splitting / module restructuring (reserved for future refactoring)
- `resilience/` vs `resilient/` module consolidation
- `skills/` vs `skill/` module unification
- 22 `Arc<non-Send+Sync>` warnings (require architectural review)
- Memory module reorganization

---

## Strategy: Wave-Based Risk Progression

Each wave completes independently with `cargo test` validation before proceeding.

### Wave 1: Automatic Clippy Fix (~100+ warnings)

**Risk: Zero** — Pure automated syntax transformations.

| Fix Type | Count | Method |
|----------|-------|--------|
| `unused_imports` | 62+ | Delete |
| `redundant_closure` | 6 | `\|x\| foo(x)` → `foo` |
| `let_and_return` | 11 | Remove intermediate binding |
| `len_zero` / `len_without_is_empty` | 10 | `.len() == 0` → `.is_empty()` |
| `bool_comparison` | 4 | `== false` → `!` |
| `clone_on_copy` | 3 | `.clone()` → value copy |
| `manual_range_contains` | 9 | `x >= lo && x <= hi` → `(lo..=hi).contains(&x)` |
| `redundant_pattern_matching` | 2 | Use `.is_ok()` |
| `identity_map` | 1 | Remove `.map(\|x\| x)` |

**Execution:**
```bash
cargo clippy --fix --allow-dirty --all-targets
cargo test
```

### Wave 2: Manual Clippy + Idiomatic Fixes (~200 locations)

**Risk: Low** — Equivalent transformations recommended by Clippy.

#### 2.1 Default::default() Field Reassignment (75 locations)

```rust
// Before
let mut config = Config::default();
config.timeout = Duration::from_secs(30);

// After
let config = Config {
    timeout: Duration::from_secs(30),
    ..Config::default()
};
```

#### 2.2 expect(format!()) → unwrap_or_else (12 locations)

```rust
// Before — allocates String even on success
map.get("key").expect(&format!("missing key: {}", name));

// After — allocates only on panic
map.get("key").unwrap_or_else(|| panic!("missing key: {name}"));
```

#### 2.3 useless_vec → slice (9 locations)

```rust
// Before
let items = vec!["a", "b", "c"];
process(&items);

// After
process(&["a", "b", "c"]);
```

#### 2.4 derivable_impls → #[derive(Default)] (4 locations)

Manual `impl Default` replaced with derive when behavior is identical.

#### 2.5 &PathBuf → &Path (3 locations)

Function parameters typed as `&PathBuf` changed to `&Path`.

#### 2.6 module_inception (5 locations)

Rename inner modules that share names with parent directories.

#### 2.7 type_complexity (7 locations)

Extract `type` aliases for complex type signatures.

### Wave 3: EventHandler Trait + &Vec Anti-Pattern

**Risk: Medium** — Breaking trait change requiring all impl blocks to update.

#### 3.1 InternalEventHandler: String → &str

All `String` parameters in the `InternalEventHandler` trait converted to `&str`:

```rust
// Before
pub trait InternalEventHandler: Send + Sync {
    fn on_error(&self, message: String, suggestion: Option<String>);
    fn on_response_chunk(&self, text: String);
    fn on_ai_processing_started(&self, provider_name: String, provider_color: String);
    fn on_conversation_started(&self, session_id: String);
    fn on_agent_started(&self, plan_id: String, total_steps: u32, description: String);
    // ... ~15 methods total
}

// After
pub trait InternalEventHandler: Send + Sync {
    fn on_error(&self, message: &str, suggestion: Option<&str>);
    fn on_response_chunk(&self, text: &str);
    fn on_ai_processing_started(&self, provider_name: &str, provider_color: &str);
    fn on_conversation_started(&self, session_id: &str);
    fn on_agent_started(&self, plan_id: &str, total_steps: u32, description: &str);
    // ...
}
```

**Preserved methods** (struct parameters, not bare strings):
- `on_clarification_needed(request: ClarificationRequest)`
- `on_mcp_startup_complete(report: McpStartupReport)`
- `on_confirmation_needed(confirmation: PendingConfirmationInfo)`
- `on_conversation_turn_completed(turn: ConversationTurn)`

#### 3.2 &Vec<f32> → &[f32] (arrow_convert.rs, 5 locations)

```rust
// Before
fn build_vector_column(embeddings: &[Option<&Vec<f32>>], dim: i32) -> Result<...>

// After
fn build_vector_column(embeddings: &[Option<&[f32]>], dim: i32) -> Result<...>
```

#### 3.3 &Vec<ToolCall> → &[ToolCall] (shadow_replay.rs, 3 locations)

Closure parameters and let bindings using `&Vec<T>` changed to `&[T]`.

### Wave 4: Handler Registration Macro

**Risk: Medium-High** — Redesigns the registration pattern.

#### Problem

Current pattern in `handlers.rs` (40+ handlers, ~370 lines of boilerplate):

```rust
let auth_ctx_connect = auth_ctx.clone();          // clone 1
server.handlers_mut().register("connect", move |req| {
    let ctx = auth_ctx_connect.clone();            // clone 2
    async move { auth_handlers::handle_connect(req, ctx).await }
});
```

#### Solution: register_handler! macro

```rust
macro_rules! register_handler {
    // 1 context arg
    ($server:expr, $method:expr, $handler:path, $ctx1:expr) => {{
        let ctx1 = Arc::clone(&$ctx1);
        $server.handlers_mut().register($method, move |req| {
            let ctx1 = Arc::clone(&ctx1);
            async move { $handler(req, ctx1).await }
        });
    }};
    // 2 context args
    ($server:expr, $method:expr, $handler:path, $ctx1:expr, $ctx2:expr) => {{
        let ctx1 = Arc::clone(&$ctx1);
        let ctx2 = Arc::clone(&$ctx2);
        $server.handlers_mut().register($method, move |req| {
            let ctx1 = Arc::clone(&ctx1);
            let ctx2 = Arc::clone(&ctx2);
            async move { $handler(req, ctx1, ctx2).await }
        });
    }};
    // 3 context args
    ($server:expr, $method:expr, $handler:path, $ctx1:expr, $ctx2:expr, $ctx3:expr) => {{
        let ctx1 = Arc::clone(&$ctx1);
        let ctx2 = Arc::clone(&$ctx2);
        let ctx3 = Arc::clone(&$ctx3);
        $server.handlers_mut().register($method, move |req| {
            let ctx1 = Arc::clone(&ctx1);
            let ctx2 = Arc::clone(&ctx2);
            let ctx3 = Arc::clone(&ctx3);
            async move { $handler(req, ctx1, ctx2, ctx3).await }
        });
    }};
    // No context args (stateless handler)
    ($server:expr, $method:expr, $handler:path) => {{
        $server.handlers_mut().register($method, |req| async move {
            $handler(req).await
        });
    }};
}
```

#### Result

```rust
// After: 1 line per handler
register_handler!(server, "connect", auth_handlers::handle_connect, auth_ctx);
register_handler!(server, "pairing.approve", auth_handlers::handle_pairing_approve, auth_ctx);
register_handler!(server, "guests.createInvitation", guests::handle_create_invitation, invitation_manager, event_bus);
```

#### Impact

| Function | Before | After | Reduction |
|----------|--------|-------|-----------|
| `register_auth_handlers` | 48 lines | 8 lines | -83% |
| `register_guest_handlers` | 58 lines | 10 lines | -83% |
| `register_session_handlers` | 44 lines | 8 lines | -82% |
| `register_channel_handlers` | 82 lines | 15 lines | -82% |
| `setup_config_watcher` (handlers) | 40 lines | 8 lines | -80% |
| Other register functions | ~100 lines | ~20 lines | -80% |
| **Total** | **~372 lines** | **~69 lines** | **-81%** |

---

## Semantic Guarantees

| Guarantee | Verification |
|-----------|-------------|
| Public API equivalence | Compiler enforces trait bounds. No public API changes except InternalEventHandler signature. |
| Error type preservation | No error variants added/removed. Only parameter types change (String → &str). |
| Send/Sync behavior | &str is Send+Sync. Macro produces identical Arc clone pattern. |
| No runtime regression | &str eliminates allocations. Macro is zero-cost (same expansion). |
| Concurrency safety | Lock ordering unchanged. No new mutexes or channels introduced. |

---

## Excluded from This Refactoring

| Item | Reason |
|------|--------|
| 22 `Arc<non-Send+Sync>` warnings | Requires architectural analysis of threading model |
| `resilience/` vs `resilient/` consolidation | Module restructuring out of scope |
| `skills/` vs `skill/` unification | Module restructuring out of scope |
| Memory module sprawl (89 files) | Module restructuring out of scope |
| Large file splitting (50+ files > 300 lines) | Module restructuring out of scope |
| `.to_string()` broad optimization (9,302 calls) | Too diffuse, requires per-call-site analysis |
| 54 `#[allow(dead_code)]` annotations | Most are intentional (serde, architecture reserves) |
