# Surgical DRY Refactoring Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Extract duplicated patterns into shared helpers across 7 already-modified files, fix a lock-during-I/O bug, reduce ~190 lines.

**Architecture:** Pure refactoring — no public API changes, no file splits, no new modules. All helpers are `pub(crate)` or private. Validation via `cargo build` + `cargo clippy`.

**Tech Stack:** Rust, async/await, serde, arrow-rs, LanceDB

---

## Task 1: Add shared `parse_params` to gateway handlers module

**Files:**
- Modify: `core/src/gateway/handlers/mod.rs` (add function after imports, before `HandlerRegistry`)

**Step 1: Add the shared helper function**

Add after line 103 (`use crate::config::Config;`) and before line 106 (`/// Type alias`):

```rust
use super::protocol::INVALID_PARAMS;

/// Parse and deserialize JSON-RPC request params into a typed struct.
///
/// Returns `Err(JsonRpcResponse)` with `INVALID_PARAMS` on missing or
/// malformed params — callers can early-return this directly.
// JsonRpcResponse is 152+ bytes but boxing it would complicate all handler call sites
#[allow(clippy::result_large_err)]
pub(crate) fn parse_params<T: serde::de::DeserializeOwned>(
    request: &JsonRpcRequest,
) -> Result<T, JsonRpcResponse> {
    match &request.params {
        Some(p) => serde_json::from_value(p.clone()).map_err(|e| {
            JsonRpcResponse::error(
                request.id.clone(),
                INVALID_PARAMS,
                format!("Invalid params: {}", e),
            )
        }),
        None => Err(JsonRpcResponse::error(
            request.id.clone(),
            INVALID_PARAMS,
            "Missing params",
        )),
    }
}
```

Note: `INVALID_PARAMS` is already imported on line 103 via `use super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, METHOD_NOT_FOUND};`. We need to add `INVALID_PARAMS` to that import.

**Step 2: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo build 2>&1 | head -20`
Expected: Compiles successfully (unused function warning is OK for now).

**Step 3: Commit**

```bash
git add core/src/gateway/handlers/mod.rs
git commit -m "gateway: add shared parse_params helper for RPC handlers"
```

---

## Task 2: Replace inline params parsing in `embedding_providers.rs`

**Files:**
- Modify: `core/src/gateway/handlers/embedding_providers.rs`

**Step 1: Replace 6 inline parsing blocks with `parse_params`**

Replace each occurrence of the 14-line inline parsing pattern with a 4-line match.

The 14-line pattern (appears at lines 65-79, 114-128, 179-193, 245-259, 322-336, 389-403):
```rust
let params: Params = match request.params {
    Some(p) => match serde_json::from_value(p) {
        Ok(params) => params,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid params: {}", e),
            )
        }
    },
    None => {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params")
    }
};
```

Replace with:
```rust
let params: Params = match super::parse_params(&request) {
    Ok(p) => p,
    Err(e) => return e,
};
```

Also remove unused imports that were only needed for inline parsing:
- Remove `INVALID_PARAMS` from imports on line 19 (keep `INTERNAL_ERROR`)

Update the import line from:
```rust
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
```
to:
```rust
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
```

**Step 2: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo build 2>&1 | head -20`
Expected: Compiles successfully.

**Step 3: Run existing tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test --lib gateway::handlers::embedding_providers 2>&1 | tail -10`
Expected: All 5 tests pass.

**Step 4: Commit**

```bash
git add core/src/gateway/handlers/embedding_providers.rs
git commit -m "gateway: use shared parse_params in embedding_providers handlers"
```

---

## Task 3: Replace inline params parsing in `generation_providers.rs`

**Files:**
- Modify: `core/src/gateway/handlers/generation_providers.rs`

**Step 1: Replace 6 inline parsing blocks**

Same transformation as Task 2: replace the inline `match request.params` blocks at lines 205, 268, 347, 425, 515, 609 with:
```rust
let params: Params = match super::parse_params(&request) {
    Ok(p) => p,
    Err(e) => return e,
};
```

Remove `INVALID_PARAMS` from imports if it becomes unused.

**Step 2: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo build 2>&1 | head -20`
Expected: Compiles successfully.

**Step 3: Commit**

```bash
git add core/src/gateway/handlers/generation_providers.rs
git commit -m "gateway: use shared parse_params in generation_providers handlers"
```

---

## Task 4: Remove private `parse_params` from `exec_approvals.rs`

**Files:**
- Modify: `core/src/gateway/handlers/exec_approvals.rs`

**Step 1: Delete the private `parse_params` function (lines 357-374)**

Delete lines 357-374 (the `#[allow(clippy::result_large_err)]` annotation and the function body).

**Step 2: Update all call sites**

Replace all `parse_params(&request)` calls with `super::parse_params(&request)`:

The pattern at call sites is already:
```rust
let params: T = match parse_params(&request) {
    Ok(p) => p,
    Err(e) => return e,
};
```

Change `parse_params` to `super::parse_params` at each occurrence.

**Step 3: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo build 2>&1 | head -20`
Expected: Compiles successfully.

**Step 4: Commit**

```bash
git add core/src/gateway/handlers/exec_approvals.rs
git commit -m "gateway: use shared parse_params in exec_approvals, remove private copy"
```

---

## Task 5: Remove private `parse_params` from `mcp.rs`

**Files:**
- Modify: `core/src/gateway/handlers/mcp.rs`

**Step 1: Delete the private `parse_params` function (lines 53-73)**

This version has an extra `required_field: &str` parameter. After deletion, the call sites need to drop this argument.

**Step 2: Update all call sites**

The current call pattern:
```rust
let params: T = match parse_params(&request, "field_name") {
    Ok(p) => p,
    Err(e) => return e,
};
```

Change to:
```rust
let params: T = match super::parse_params(&request) {
    Ok(p) => p,
    Err(e) => return e,
};
```

There are 10 call sites (lines 93, 117, 143, 163, 187, 202, 222, 242, 451, 477).

**Step 3: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo build 2>&1 | head -20`
Expected: Compiles successfully.

**Step 4: Commit**

```bash
git add core/src/gateway/handlers/mcp.rs
git commit -m "gateway: use shared parse_params in mcp handlers, remove private copy"
```

---

## Task 6: Add shared `lance_err` to lance module

**Files:**
- Modify: `core/src/memory/store/lance/mod.rs` (add function)
- Modify: `core/src/memory/store/lance/sessions.rs` (delete private `lance_err`, use `super::lance_err`)
- Modify: `core/src/memory/store/lance/facts.rs` (delete private `lance_err`, use `super::lance_err`)
- Modify: `core/src/memory/store/lance/graph.rs` (delete private `lance_err`, use `super::lance_err`)

**Step 1: Add shared `lance_err` to `lance/mod.rs`**

Add after the `use` statements (after line 12) and before line 14 (`pub mod arrow_convert;`):

```rust
/// Map a LanceDB error to an AlephError.
pub(crate) fn lance_err(msg: impl std::fmt::Display) -> AlephError {
    AlephError::config(format!("LanceDB error: {}", msg))
}
```

**Step 2: Remove private `lance_err` from `sessions.rs`**

Delete lines 27-29:
```rust
fn lance_err(msg: impl std::fmt::Display) -> AlephError {
    AlephError::config(format!("LanceDB error: {}", msg))
}
```

Replace all `lance_err` calls with `super::lance_err` in the file.

**Step 3: Remove private `lance_err` from `facts.rs`**

Same: delete lines 32-34 and replace `lance_err` with `super::lance_err`.

**Step 4: Remove private `lance_err` from `graph.rs`**

Same: delete lines 27-29 and replace `lance_err` with `super::lance_err`.

**Step 5: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo build 2>&1 | head -20`
Expected: Compiles successfully.

**Step 6: Run lance tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test --lib memory::store::lance 2>&1 | tail -20`
Expected: All tests pass.

**Step 7: Commit**

```bash
git add core/src/memory/store/lance/mod.rs core/src/memory/store/lance/sessions.rs core/src/memory/store/lance/facts.rs core/src/memory/store/lance/graph.rs
git commit -m "memory: deduplicate lance_err into shared module-level helper"
```

---

## Task 7: Merge `count_rows` and `count_rows_with_filter` in `sessions.rs`

**Files:**
- Modify: `core/src/memory/store/lance/sessions.rs`

**Step 1: Replace two functions with one**

Delete `count_rows` (lines 79-88) and `count_rows_with_filter` (lines 91-104). Replace with:

```rust
/// Count rows in a LanceDB table with an optional filter.
async fn count_rows(table: &lancedb::Table, filter: Option<&str>) -> Result<usize, AlephError> {
    let mut query = table.query().select(Select::columns(&["id"]));
    if let Some(f) = filter {
        query = query.only_if(f);
    }
    let stream = query.execute().await.map_err(super::lance_err)?;
    let batches = collect_batches(stream).await?;
    Ok(batches.iter().map(|b| b.num_rows()).sum())
}
```

**Step 2: Update call sites**

In `get_stats()` (~line 213):
- `count_rows(&self.facts_table).await?` → `count_rows(&self.facts_table, None).await?`
- `count_rows_with_filter(&self.facts_table, "is_valid = true").await?` → `count_rows(&self.facts_table, Some("is_valid = true")).await?`
- `count_rows(&self.memories_table).await?` → `count_rows(&self.memories_table, None).await?`
- `count_rows(&self.nodes_table).await?` → `count_rows(&self.nodes_table, None).await?`
- `count_rows(&self.edges_table).await?` → `count_rows(&self.edges_table, None).await?`

In `delete_older_than()` (~line 254):
- `count_rows_with_filter(&self.memories_table, &filter).await?` → `count_rows(&self.memories_table, Some(&filter)).await?`

In `clear_memories()` (~line 281):
- `count_rows_with_filter(&self.memories_table, &filter).await?` → `count_rows(&self.memories_table, Some(&filter)).await?`

**Step 3: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo build 2>&1 | head -20`
Expected: Compiles successfully.

**Step 4: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test --lib memory::store::lance::sessions 2>&1 | tail -10`
Expected: All tests pass.

**Step 5: Commit**

```bash
git add core/src/memory/store/lance/sessions.rs
git commit -m "memory: merge count_rows and count_rows_with_filter into single function"
```

---

## Task 8: Fix `test_provider()` lock-during-I/O bug

**Files:**
- Modify: `core/src/memory/embedding_manager.rs`

**Step 1: Fix the lock scope**

Replace `test_provider` (lines 95-105):

```rust
/// Test a specific provider's connectivity
pub async fn test_provider(&self, provider_id: &str) -> Result<(), AlephError> {
    let settings = self.settings.read().await;
    let config = settings
        .providers
        .iter()
        .find(|p| p.id == provider_id)
        .ok_or_else(|| AlephError::config(format!("Provider not found: {}", provider_id)))?;

    let provider = RemoteEmbeddingProvider::from_config(config)?;
    provider.test_connection().await
}
```

With:

```rust
/// Test a specific provider's connectivity.
///
/// Clones the config and releases the settings lock before performing
/// network I/O, matching the pattern used in `switch_provider()`.
pub async fn test_provider(&self, provider_id: &str) -> Result<(), AlephError> {
    let config = {
        let settings = self.settings.read().await;
        settings
            .providers
            .iter()
            .find(|p| p.id == provider_id)
            .ok_or_else(|| AlephError::config(format!("Provider not found: {}", provider_id)))?
            .clone()
    }; // settings lock released here
    let provider = RemoteEmbeddingProvider::from_config(&config)?;
    provider.test_connection().await
}
```

**Step 2: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo build 2>&1 | head -20`
Expected: Compiles successfully.

**Step 3: Commit**

```bash
git add core/src/memory/embedding_manager.rs
git commit -m "memory: fix test_provider() holding read lock during network I/O"
```

---

## Task 9: Extract `inject_is_active` helper in `embedding_providers.rs`

**Files:**
- Modify: `core/src/gateway/handlers/embedding_providers.rs`

**Step 1: Add helper function**

Add near the top of the file (after imports, before first handler):

```rust
/// Serialize a provider config to JSON and inject `is_active` based on the active provider id.
fn inject_is_active(provider: &EmbeddingProviderConfig, active_id: &str) -> serde_json::Value {
    let mut val = serde_json::to_value(provider).unwrap_or_default();
    if let Some(obj) = val.as_object_mut() {
        obj.insert(
            "is_active".into(),
            serde_json::json!(provider.id == active_id),
        );
    }
    val
}
```

**Step 2: Simplify `handle_list`**

Replace the `.map()` closure body in `handle_list`:

```rust
let providers: Vec<serde_json::Value> = settings
    .providers
    .iter()
    .map(|p| inject_is_active(p, &settings.active_provider_id))
    .collect();
```

**Step 3: Simplify `handle_get`**

Replace the `Some(provider)` arm:

```rust
Some(provider) => {
    JsonRpcResponse::success(request.id, inject_is_active(provider, &settings.active_provider_id))
}
```

**Step 4: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo build 2>&1 | head -20`
Expected: Compiles successfully.

**Step 5: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test --lib gateway::handlers::embedding_providers 2>&1 | tail -10`
Expected: All tests pass.

**Step 6: Commit**

```bash
git add core/src/gateway/handlers/embedding_providers.rs
git commit -m "gateway: extract inject_is_active helper in embedding_providers"
```

---

## Task 10: Add nullable column helpers in `arrow_convert.rs`

**Files:**
- Modify: `core/src/memory/store/lance/arrow_convert.rs`

**Step 1: Verify existing helpers**

The file already has `read_nullable_string` (lines 110-116) and `read_nullable_i64` (lines 119-125). These are standalone functions that take an array + index.

These helpers already exist — our goal is to confirm they are used consistently and replace any inline nullable checks that bypass them.

Search for inline `is_null` patterns that don't use these helpers. The `is_null` grep shows only 5 occurrences:
- Line 80: in `read_vector()` — specific to `FixedSizeListArray`, helper not applicable
- Line 90: in `read_string_list()` — specific to `ListArray`, helper not applicable
- Line 100: in `read_string_list()` inner loop — already handled
- Line 111: in `read_nullable_string()` — this IS the helper
- Line 120: in `read_nullable_i64()` — this IS the helper

**Conclusion**: The nullable helpers already exist and are properly factored. No further extraction needed.

**Step 2: Skip this task — already clean**

No changes needed for arrow_convert.rs nullable helpers.

---

## Task 11: Final validation

**Step 1: Run full build**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo build 2>&1 | tail -5`
Expected: `Finished` with no errors.

**Step 2: Run clippy**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo clippy 2>&1 | tail -20`
Expected: No new warnings from our modified files.

**Step 3: Run all tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test --lib 2>&1 | tail -20`
Expected: All tests pass.

---

## Summary

| Task | File(s) | Action | Lines Changed |
|------|---------|--------|---------------|
| 1 | `handlers/mod.rs` | Add shared `parse_params` | +20 |
| 2 | `embedding_providers.rs` | Use shared `parse_params` | -60 |
| 3 | `generation_providers.rs` | Use shared `parse_params` | -60 |
| 4 | `exec_approvals.rs` | Remove private copy, use shared | -17 |
| 5 | `mcp.rs` | Remove private copy, use shared | -21 |
| 6 | `lance/{mod,sessions,facts,graph}.rs` | Deduplicate `lance_err` | -6 |
| 7 | `sessions.rs` | Merge `count_rows` variants | -12 |
| 8 | `embedding_manager.rs` | Fix lock-during-I/O bug | ~0 |
| 9 | `embedding_providers.rs` | Extract `inject_is_active` | -8 |
| 10 | `arrow_convert.rs` | SKIP — already clean | 0 |
| 11 | — | Final validation | — |

**Total net reduction**: ~164 lines
**Risk**: Low — all changes are internal, no public API modifications
