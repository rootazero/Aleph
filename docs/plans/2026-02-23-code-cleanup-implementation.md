# Code Cleanup Implementation Plan (Occam's Razor Pass)

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Eliminate 188 clippy warnings across core/ and shared/protocol/ while preserving identical functional behavior.

**Architecture:** 3-pass risk-layered approach. Pass 1 handles mechanical auto-fixable issues, Pass 2 handles local refactoring, Pass 3 handles structural changes. Each pass is verified with `cargo build && cargo test` before committing.

**Tech Stack:** Rust, cargo clippy, cargo test

**Design doc:** `docs/plans/2026-02-23-code-cleanup-design.md`

---

## Guardrails (Read Before Every Task)

1. **Zero functional modification** — input/output must be identical
2. **Preserve defensive programming** — keep all error handling, null checks, guards
3. **No over-engineering** — no new abstractions beyond what's specified

---

## Pass 1: Mechanical Cleanup

### Task 1: Run clippy --fix for auto-fixable warnings

**Files:** All files in `core/src/` and `shared/protocol/src/`

**Step 1: Capture baseline warning count**

Run: `cargo clippy -p alephcore -p aleph-protocol 2>&1 | grep "^warning:" | wc -l`
Expected: ~188 warnings

**Step 2: Run cargo clippy --fix for auto-applicable suggestions**

Run: `cargo clippy --fix --allow-dirty --allow-staged -p alephcore -p aleph-protocol`

This will auto-fix:
- Redundant closures (~32): `.map_err(|e| lance_err(e))` → `.map_err(lance_err)`
- Some unused imports
- Boolean simplifications
- Identity operations

**Step 3: Run build and test to verify**

Run: `cargo build -p alephcore -p aleph-protocol && cargo test -p alephcore -p aleph-protocol`
Expected: PASS with zero errors

**Step 4: Check remaining warnings**

Run: `cargo clippy -p alephcore -p aleph-protocol 2>&1 | grep "^warning:" | wc -l`
Expected: Significantly fewer warnings

**Step 5: Commit**

```bash
git add -A
git commit -m "cleanup: apply cargo clippy --fix auto-corrections"
```

---

### Task 2: Fix remaining unused imports manually

**Files (exact locations from clippy):**
- `core/build.rs:5-6` — unused `std::process::Command`, `std::path::Path`
- `core/src/config/mod.rs:33,42,45` — unused `generate_config_schema`, `diff_config`, `build_reload_plan`
- `core/src/config/types/generation/mod.rs:17` — unused `GenerationDefaults`
- `core/src/agents/swarm/bus.rs:6,9,12` — unused `HashMap`, `warn`, `AlephError`
- `core/src/memory/store/lance/facts.rs` — unused `MemoryStore`, `StoreStats`
- `core/src/memory/cortex/clustering.rs` — unused `EvolutionStatus`, `Arc`
- `core/src/perception/state_bus/ax_observer.rs` — unused `serde_json::Value`, `Arc`
- `core/src/perception/connectors/mod.rs` — unused `EnvironmentCapability`, `ProcessCapability`
- `core/src/exec/sandbox/platforms/macos.rs` — unused imports
- `core/src/exec/sandbox/profile.rs` — unused `parameter_binding::RequiredCapabilities`
- `core/src/skill_evolution/constraint_validator.rs` — unused `ConstraintMismatch`, `ValidationReport`
- `core/src/skill_evolution/collaborative_pipeline.rs` — unused `StateCache`
- `core/src/memory/cortex/dreaming.rs` — unused imports
- `core/src/daemon/resource_governor.rs` — unused `Value`
- `core/src/gateway/handlers/state_bus.rs` — unused imports
- `core/src/dispatcher/experience_replay_layer.rs` — unused imports

**Step 1: For each file, remove the unused import line**

For each file listed above: open it, delete or trim the unused import. If a `use` line has multiple items and only some are unused, remove only the unused ones.

Example for `core/build.rs`:
```rust
// DELETE these two lines:
use std::process::Command;
use std::path::Path;
```

Example for `core/src/agents/swarm/bus.rs`:
```rust
// Before:
use std::collections::HashMap;
use tracing::{debug, info, warn};
use crate::error::{AlephError, Result};

// After (remove unused, keep used):
use tracing::{debug, info};
use crate::error::Result;
```

**Step 2: Build and test**

Run: `cargo build -p alephcore -p aleph-protocol && cargo test -p alephcore -p aleph-protocol`
Expected: PASS

**Step 3: Commit**

```bash
git add -A
git commit -m "cleanup: remove unused imports across core and protocol crates"
```

---

### Task 3: Replace derivable impls with #[derive]

**Files:**
- `shared/protocol/src/auth.rs:18-22` — `impl Default for Role`
- `core/src/config/types/privacy.rs` — derivable Default
- `core/src/exec/sandbox/executor.rs` — derivable Default
- `core/src/exec/sandbox/parameter_binding.rs` — derivable Default (2 instances)
- `core/src/memory/workspace.rs` — derivable Default

**Step 1: Fix shared/protocol/src/auth.rs**

```rust
// Before:
pub enum Role {
    Admin,
    Owner,
    Guest,
    Anonymous,
}

impl Default for Role {
    fn default() -> Self {
        Self::Anonymous
    }
}

// After:
#[derive(Default)]
pub enum Role {
    Admin,
    Owner,
    Guest,
    #[default]
    Anonymous,
}
// DELETE the impl Default block entirely
```

**Step 2: Apply same pattern to each file listed above**

For each: add `#[derive(Default)]` to the type, add `#[default]` to the default variant/field, delete the manual `impl Default` block.

**Step 3: Fix MacOSSandbox — add Default implementation**

File: `core/src/exec/sandbox/platforms/macos.rs`
If clippy suggests adding a Default impl, add `#[derive(Default)]` or a simple `impl Default`.

**Step 4: Build and test**

Run: `cargo build -p alephcore -p aleph-protocol && cargo test -p alephcore -p aleph-protocol`
Expected: PASS

**Step 5: Commit**

```bash
git add -A
git commit -m "cleanup: replace manual Default impls with #[derive(Default)]"
```

---

### Task 4: Fix boolean simplifications and identity operations

**Files:**
- `shared/protocol/src/auth.rs:67` — `map_or(false, ...)` → `is_some_and(...)`
- `shared/protocol/src/jsonrpc.rs:304` — `(random >> 16) as u16 & 0xFFFF` → `(random >> 16) as u16`
- `shared/protocol/src/manifest.rs:110` — doc indentation fix

**Step 1: Fix auth.rs**

```rust
// Before:
self.expires_at.map_or(false, |exp| current_time >= exp)

// After:
self.expires_at.is_some_and(|exp| current_time >= exp)
```

**Step 2: Fix jsonrpc.rs**

```rust
// Before:
(random >> 16) as u16 & 0xFFFF,

// After:
(random >> 16) as u16,
```

**Step 3: Fix manifest.rs doc comment**

```rust
// Before:
/// AND tool is NOT in `excluded_tools`

// After:
///   AND tool is NOT in `excluded_tools`
```

**Step 4: Build and test**

Run: `cargo build -p alephcore -p aleph-protocol && cargo test -p alephcore -p aleph-protocol`
Expected: PASS

**Step 5: Commit**

```bash
git add -A
git commit -m "cleanup: simplify boolean expressions and remove identity operations"
```

---

### Task 5: Fix &PathBuf → &Path in function signatures

**Files (6 instances):**

Search for all `&PathBuf` in function signatures:

Run: `grep -rn "&PathBuf" core/src/ --include="*.rs" | grep -v "test" | grep -v "target"`

For each occurrence, change the parameter type from `&PathBuf` to `&Path`. Since `PathBuf` auto-derefs to `Path`, all call sites remain valid.

```rust
// Before:
fn process_file(path: &PathBuf) -> Result<()> {

// After:
fn process_file(path: &Path) -> Result<()> {
```

If `std::path::Path` is not yet imported in the file, add it. If `std::path::PathBuf` import becomes unused after the change, remove it.

**Step 1: Find and fix all instances**
**Step 2: Build and test**

Run: `cargo build -p alephcore && cargo test -p alephcore`
Expected: PASS

**Step 3: Commit**

```bash
git add -A
git commit -m "cleanup: use &Path instead of &PathBuf in function signatures"
```

---

### Task 6: Verify Pass 1 results

**Step 1: Run full clippy check**

Run: `cargo clippy -p alephcore -p aleph-protocol 2>&1 | grep "^warning:" | wc -l`

**Step 2: Compare with baseline (188)**

Document the number of warnings eliminated.

**Step 3: Run full test suite**

Run: `cargo test -p alephcore -p aleph-protocol`
Expected: All tests PASS

---

## Pass 2: Local Refactoring

### Task 7: Fix clone-to-slice-ref inefficiencies

**Files (6 instances, all in memory/store/lance/):**
- `facts.rs:129` — `&[fact.clone()]`
- `graph.rs:122,143,269,308` — `&[node.clone()]`, `&[edge.clone()]`
- `sessions.rs:131` — `&[memory.clone()]`

**Step 1: Replace each instance**

```rust
// Before:
&[fact.clone()]

// After:
std::slice::from_ref(fact)
```

Or if the item is not behind a reference:
```rust
// Before:
&[node.clone()]

// After:
std::slice::from_ref(&node)
```

**Step 2: Build and test**

Run: `cargo build -p alephcore && cargo test -p alephcore`
Expected: PASS

**Step 3: Commit**

```bash
git add -A
git commit -m "cleanup: replace clone-to-slice with std::slice::from_ref"
```

---

### Task 8: Fix manual string prefix stripping

**Files (7+ instances):**
- `core/src/thinker/soul.rs:260,265,345,347` — manual `[n..]` after `starts_with()`
- `core/src/gateway/config.rs:523` — manual `[2..]` after prefix check
- Other files flagged by clippy

**Step 1: Fix thinker/soul.rs**

```rust
// Before:
if line.starts_with("## ") {
    current_heading = Some(line[3..].trim().to_string());
}

// After:
if let Some(rest) = line.strip_prefix("## ") {
    current_heading = Some(rest.trim().to_string());
}
```

```rust
// Before (identical blocks for "- " and "* "):
if trimmed.starts_with("- ") {
    Some(trimmed[2..].trim().to_string())
} else if trimmed.starts_with("* ") {
    Some(trimmed[2..].trim().to_string())
}

// After (consolidated):
if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
    Some(rest.trim().to_string())
}
```

**Step 2: Fix gateway/config.rs:523**

```rust
// Before:
if path.starts_with("~/") {
    return home.join(&path[2..]);
}

// After:
if let Some(rest) = path.strip_prefix("~/") {
    return home.join(rest);
}
```

**Step 3: Fix suffix stripping (1 instance)**

Search for and fix any `strip_suffix` manual implementation.

**Step 4: Build and test**

Run: `cargo build -p alephcore && cargo test -p alephcore`
Expected: PASS

**Step 5: Commit**

```bash
git add -A
git commit -m "cleanup: use strip_prefix/strip_suffix instead of manual slicing"
```

---

### Task 9: Fix unused variables and dead code

**Files:**
- Unused variables: `ctx` (4 instances), `bundle_id` (2), `tx`, `request`, `intent_vector`, `full_id`, `fallback`, `db` (1 each)
- Dead fields: `db` (3 instances), `credit_card_regex`, `base_url`, `event_tx`, `db_path`, `patches`+`base_iframe_ts`, `bundle_id`+`last_capture`+`last_state_hash`+`consecutive_no_change`
- Dead methods: `clear()` in aggregator.rs, `unsupported()` in pal/health.rs, `deserialize_embedding()` in state_database.rs, `compute_state_hash`+`adjust_poll_interval` in ax_observer.rs

**Step 1: Prefix unused variables with `_`**

For variables that are intentionally unused (e.g., destructuring patterns):
```rust
// Before:
let ctx = ...;  // unused

// After:
let _ctx = ...;
```

**Step 2: Delete dead methods (verify zero references first)**

For each dead method, run:
```bash
grep -rn "method_name" core/src/ --include="*.rs" | grep -v "fn method_name"
```

If zero references, delete the method.

**Step 3: For dead fields — add `#[allow(dead_code)]` with a TODO comment**

Dead fields may be part of a struct used for serialization/deserialization. Verify before deleting. If the field is part of a serde struct (has `#[derive(Deserialize)]`), keep it with `#[allow(dead_code)]`. Otherwise delete.

**Step 4: Build and test**

Run: `cargo build -p alephcore && cargo test -p alephcore`
Expected: PASS

**Step 5: Commit**

```bash
git add -A
git commit -m "cleanup: remove dead code and prefix unused variables"
```

---

### Task 10: Fix miscellaneous clippy warnings

**Step 1: Fix match → if let (audit.rs:49)**

```rust
// Before:
match fs_cap {
    FileSystemCapability::ReadWrite { .. } => score += 20,
    _ => {}
}

// After:
if let FileSystemCapability::ReadWrite { .. } = fs_cap {
    score += 20;
}
```

**Step 2: Fix loop variable indexing (clustering.rs)**

```rust
// Before:
for i in 0..matrix.len() {
    for j in 0..matrix[i].len() {
        // uses matrix[i][j]
    }
}

// After:
for (i, row) in matrix.iter().enumerate() {
    for (j, val) in row.iter().enumerate() {
        // uses val directly where possible
    }
}
```

**Step 3: Fix map keys iteration**

```rust
// Before:
for (key, _value) in map.iter() { ... }

// After:
for key in map.keys() { ... }
```

**Step 4: Fix RangeInclusive::contains**

```rust
// Before:
x >= min && x <= max

// After:
(min..=max).contains(&x)
```

**Step 5: Fix is_multiple_of**

```rust
// Before:
x % 2 == 0

// After:
x.is_multiple_of(2)  // or keep as-is if is_multiple_of not available
```

**Step 6: Fix clamp pattern**

```rust
// Before:
if x < min { min } else if x > max { max } else { x }

// After:
x.clamp(min, max)
```

**Step 7: Fix transmute annotation, deprecated method, push_str, unnecessary cast, etc.**

Apply remaining clippy suggestions one by one.

**Step 8: Build and test**

Run: `cargo build -p alephcore && cargo test -p alephcore`
Expected: PASS

**Step 9: Commit**

```bash
git add -A
git commit -m "cleanup: fix miscellaneous clippy warnings (match, range, clamp, etc.)"
```

---

### Task 11: Fix module naming conflicts

**Files:**
- `core/src/agent_loop/mod.rs:69` — `mod agent_loop;` inside `agent_loop` module

**Step 1: Investigate the naming conflict**

Read the file and understand why the submodule shares the parent's name. Possible fixes:
- Rename the inner module to something more specific
- This may be intentional (the `mod.rs` re-exports from a file named `agent_loop.rs`)

**Step 2: If it's just a re-export pattern, suppress with `#[allow(clippy::module_inception)]`**

Only suppress if renaming would break public API.

**Step 3: Build and test**

Run: `cargo build -p alephcore && cargo test -p alephcore`
Expected: PASS

**Step 4: Commit**

```bash
git add -A
git commit -m "cleanup: resolve module naming conflicts"
```

---

### Task 12: Verify Pass 2 results

**Step 1: Run full clippy check**

Run: `cargo clippy -p alephcore -p aleph-protocol 2>&1 | grep "^warning:" | wc -l`

**Step 2: Run full test suite**

Run: `cargo test -p alephcore -p aleph-protocol`
Expected: All tests PASS

---

## Pass 3: Structural Refactoring

### Task 13: Extract type aliases for complex types

**Files (9 instances):**
- `core/src/gateway/channel_registry.rs:40` — `RwLock<HashMap<ChannelId, Arc<RwLock<Box<dyn Channel>>>>>`
- `core/src/gateway/handlers/exec_approvals.rs:108-109` — handler factory return type
- `core/src/gateway/handlers/wizard.rs:384,386` — same handler factory pattern
- `core/src/resilient/executor.rs:19` — callback type
- `core/src/resilient/task.rs:129` — async task closure type
- `core/src/secrets/crypto.rs:47` — encrypt return tuple
- `core/src/skill_evolution/tracker.rs:180` — query result tuple

**Step 1: Fix channel_registry.rs**

```rust
// Add at top of file or in a types module:
type ChannelHandle = Arc<RwLock<Box<dyn Channel>>>;

// Replace usage:
// Before:
channels: RwLock<HashMap<ChannelId, Arc<RwLock<Box<dyn Channel>>>>>,
// After:
channels: RwLock<HashMap<ChannelId, ChannelHandle>>,
```

**Step 2: Fix handler factory pattern (exec_approvals.rs + wizard.rs)**

```rust
// Add shared type alias:
type RpcHandler = Box<dyn Fn(JsonRpcRequest) -> Pin<Box<dyn Future<Output = JsonRpcResponse> + Send>> + Send + Sync>;
type HandlerFactory = Box<dyn Fn(&str) -> Option<RpcHandler>>;
```

**Step 3: Fix resilient/executor.rs**

```rust
// Before:
notify_callback: Option<Box<dyn Fn(&str, &str) + Send + Sync>>,

// After:
type NotifyCallback = Box<dyn Fn(&str, &str) + Send + Sync>;
// ...
notify_callback: Option<NotifyCallback>,
```

**Step 4: Fix resilient/task.rs**

```rust
// Extract the complex closure type into a type alias
type AsyncTaskFn<O> = Box<dyn Fn(&TaskContext) -> Pin<Box<dyn Future<Output = Result<O>> + Send + '_>> + Send + Sync>;
```

**Step 5: Fix secrets/crypto.rs**

```rust
// Before:
pub fn encrypt(&self, plaintext: &str) -> Result<(Vec<u8>, [u8; 12], [u8; 32]), SecretError>

// After:
/// Encrypted data: (ciphertext, nonce, salt)
pub struct EncryptedData {
    pub ciphertext: Vec<u8>,
    pub nonce: [u8; 12],
    pub salt: [u8; 32],
}

pub fn encrypt(&self, plaintext: &str) -> Result<EncryptedData, SecretError>
```

Update all call sites that destructure the tuple.

**Step 6: Fix skill_evolution/tracker.rs**

```rust
// Before:
let result: Option<(i64, i64, f64, Option<f64>, f64, i64, i64, String)> = ...

// After: Define a struct for the query result
struct TrackerQueryResult {
    // fields with meaningful names
}
```

**Step 7: Build and test**

Run: `cargo build -p alephcore && cargo test -p alephcore`
Expected: PASS

**Step 8: Commit**

```bash
git add -A
git commit -m "cleanup: extract type aliases for complex types"
```

---

### Task 14: Extract parameter structs for functions with too many arguments

**Files (7 functions):**
- `core/src/gateway/security/store.rs:96` — `upsert_device` (8 params)
- `core/src/gateway/security/store.rs:309` — `insert_pairing_request` (11 params)
- `core/src/gateway/server.rs:381` — `handle_connection` (8 params)
- `core/src/gateway/handlers/auth.rs:85` — `AuthHandler::new` (8 params)
- `core/src/memory/store/lance/facts.rs:570` — `manual_hybrid_search` (8 params)
- `core/src/memory/store/mod.rs:151` — trait method (8 params)
- `core/src/poe/services/run_service.rs:295` — `execute_poe_task` (9 params)

**Step 1: Fix gateway/security/store.rs:309 (worst offender: 11 params)**

Read the function signature, group related parameters into a struct:

```rust
pub struct PairingRequestData {
    pub request_id: String,
    pub code: String,
    // ... remaining fields
}

pub fn insert_pairing_request(&self, data: &PairingRequestData) -> Result<()> {
    // use data.request_id, data.code, etc.
}
```

Update all call sites.

**Step 2: Fix gateway/security/store.rs:96 (8 params)**

Similar pattern — extract `DeviceUpsertData` struct.

**Step 3: Fix gateway/server.rs:381 (8 params)**

Extract `ConnectionContext` struct for `handle_connection`.

**Step 4: Fix gateway/handlers/auth.rs:85 (8 params)**

Extract deps struct for `AuthHandler::new`.

**Step 5: Fix memory/store/lance/facts.rs:570 (8 params)**

Extract `HybridSearchParams` struct.

**Step 6: Fix memory/store/mod.rs:151 (trait method)**

This is a trait definition. Add a `HybridSearchParams` struct and change the trait method signature. Update all implementations.

**Step 7: Fix poe/services/run_service.rs:295 (9 params)**

Extract `PoeExecutionContext` struct.

**Step 8: Build and test**

Run: `cargo build -p alephcore && cargo test -p alephcore`
Expected: PASS

**Step 9: Commit**

```bash
git add -A
git commit -m "cleanup: extract parameter structs for functions with 8+ arguments"
```

---

### Task 15: Fix Arc misuse and large error variants

**Arc misuse (3 instances):**
- `core/src/agent_loop/meta_cognition_integration.rs:84` — `Arc<ReactiveReflector>` (not Send+Sync)
- `core/src/agent_loop/meta_cognition_integration.rs:91` — `Arc<Locked<AnchorRetriever>>` (not Send+Sync)
- `core/src/engine/persistence.rs:77` — `Arc<RwLock<Connection>>` (RwLock not Sync)

**Step 1: Investigate each Arc usage**

For each, determine whether the value is actually shared across threads:
- If yes → make the inner type Send+Sync (e.g., use `tokio::sync::RwLock` instead of `std::sync::RwLock`)
- If no → replace `Arc` with `Rc` (if single-threaded) or remove wrapping entirely

**Step 2: For persistence.rs, check if std::sync::RwLock should be tokio::sync::RwLock**

The `RwLock<Connection>` issue: `std::sync::RwLock` is not `Sync` when wrapped in `Arc` in certain contexts. If this is used in async context, switch to `tokio::sync::RwLock`.

**Step 3: For meta_cognition_integration.rs, investigate ReactiveReflector**

If `ReactiveReflector` needs to be shared across tasks, add `Send + Sync` bounds or wrap inner state with `Mutex`. If not shared, remove `Arc`.

**Large error variants (4 instances):**
- `Result<T, JsonRpcResponse>` — `JsonRpcResponse` is 152+ bytes

**Step 4: Box the large error variant**

```rust
// Before:
fn handler() -> Result<T, JsonRpcResponse>

// After:
fn handler() -> Result<T, Box<JsonRpcResponse>>
```

Update all match/map_err sites. This is a judgment call — if `JsonRpcResponse` is frequently created in error paths, boxing adds overhead. If errors are rare (which they should be), boxing is net positive.

**Step 5: Build and test**

Run: `cargo build -p alephcore && cargo test -p alephcore`
Expected: PASS

**Step 6: Commit**

```bash
git add -A
git commit -m "cleanup: fix Arc misuse and box large error variants"
```

---

### Task 16: Final verification

**Step 1: Run full clippy**

Run: `cargo clippy -p alephcore -p aleph-protocol 2>&1 | grep "^warning:" | wc -l`
Expected: ≤ 38 warnings (≥80% reduction from 188)

**Step 2: Run full test suite**

Run: `cargo test -p alephcore -p aleph-protocol`
Expected: All tests PASS

**Step 3: Document results**

Update the design doc success criteria with actual numbers:
- Baseline warnings: 188
- Final warnings: [actual count]
- Reduction: [percentage]
- Tests: All passing

**Step 4: Commit final documentation update**

```bash
git add -A
git commit -m "cleanup: complete Occam's Razor pass — N warnings eliminated"
```
