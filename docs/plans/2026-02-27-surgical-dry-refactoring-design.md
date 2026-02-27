# Surgical DRY Refactoring Design

**Date**: 2026-02-27
**Scope**: 7 already-modified files (embedding providers + lance store)
**Approach**: Extract repeated patterns into shared helpers, fix bugs, zero structural changes
**Validation**: `cargo build` + `cargo clippy`

---

## Problem Statement

The 7 files currently modified in git contain significant code duplication and one concurrency bug:

| File | Lines | Core Issue |
|------|-------|------------|
| `gateway/handlers/embedding_providers.rs` | 571 | Params deserialization template repeated 6× |
| `memory/embedding_manager.rs` | 122 | Read lock held during network I/O in `test_provider()` |
| `memory/store/lance/arrow_convert.rs` | 1,180 | Nullable column read pattern repeated throughout |
| `memory/store/lance/schema.rs` | 280 | Clean — no changes needed |
| `memory/store/lance/sessions.rs` | 509 | `count_rows` / `count_rows_with_filter` near-identical; `lance_err` defined 3× across lance module |
| `ui/control_plane/src/api.rs` | 1,275 | Huge file, but structural split deferred |
| `ui/control_plane/src/views/settings/embedding_providers.rs` | 905 | UI component — minimal changes |

---

## Refactoring Actions

### Action 1: Promote `parse_params` to shared gateway helper

**Context**: `parse_params<T>()` is already defined privately in `mcp.rs` (line 55) and `exec_approvals.rs` (line 359), plus inlined 6× in `embedding_providers.rs` and 6× in `generation_providers.rs`.

**Target location**: `core/src/gateway/handlers/mod.rs`

```rust
/// Parse and deserialize RPC request params, returning a typed error response on failure.
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

**Consumers** (replace inline / private definitions):
- `embedding_providers.rs` — 6 handlers → `let params: Params = parse_params(&request)?;`
- `generation_providers.rs` — 6 handlers → same
- `mcp.rs` — delete private `parse_params`, use shared (note: drops unused `required_field` parameter)
- `exec_approvals.rs` — delete private `parse_params`, use shared

**Line reduction**: ~120 lines across 4 files

**Semantic equivalence**: Return type, error codes, and error messages are identical. The `mcp.rs` version has an extra `required_field` parameter that is only used in the error message — dropping it simplifies without losing meaningful information since the JSON deserialization error already describes what's missing.

---

### Action 2: Promote `lance_err` to lance module level

**Context**: Identical `fn lance_err(msg) -> AlephError` defined in:
- `sessions.rs:27-29`
- `facts.rs:32-34`
- `graph.rs:27-29`

**Target location**: `core/src/memory/store/lance/mod.rs`

```rust
/// Map a LanceDB error to an AlephError.
pub(crate) fn lance_err(msg: impl std::fmt::Display) -> AlephError {
    AlephError::config(format!("LanceDB error: {}", msg))
}
```

**Consumers**: All 3 files import `super::lance_err` instead of defining their own.

**Line reduction**: ~9 lines

---

### Action 3: Merge `count_rows` and `count_rows_with_filter`

**Context**: In `sessions.rs`, two functions differ only by an `only_if(filter)` call.

**Merged version**:

```rust
async fn count_rows(table: &lancedb::Table, filter: Option<&str>) -> Result<usize, AlephError> {
    let mut query = table.query().select(Select::columns(&["id"]));
    if let Some(f) = filter {
        query = query.only_if(f);
    }
    let stream = query.execute().await.map_err(lance_err)?;
    let batches = collect_batches(stream).await?;
    Ok(batches.iter().map(|b| b.num_rows()).sum())
}
```

**Call site updates**:
- `count_rows(&self.facts_table)` → `count_rows(&self.facts_table, None)`
- `count_rows_with_filter(&self.facts_table, "is_valid = true")` → `count_rows(&self.facts_table, Some("is_valid = true"))`

**Line reduction**: ~12 lines

---

### Action 4: Fix `test_provider()` lock-during-I/O bug

**File**: `memory/embedding_manager.rs:95-105`

**Bug**: `self.settings.read().await` lock is held while `provider.test_connection().await` performs network I/O. This blocks all other reads of `self.settings` during the entire network round-trip.

**Fix**: Clone the config, release the lock, then create provider and test.

```rust
pub async fn test_provider(&self, provider_id: &str) -> Result<(), AlephError> {
    let config = {
        let settings = self.settings.read().await;
        settings
            .providers
            .iter()
            .find(|p| p.id == provider_id)
            .ok_or_else(|| AlephError::config(format!("Provider not found: {}", provider_id)))?
            .clone()
    }; // lock released here
    let provider = RemoteEmbeddingProvider::from_config(&config)?;
    provider.test_connection().await
}
```

**Semantic note**: This matches the pattern already used correctly in `switch_provider()` (line 68-79) in the same file.

---

### Action 5: Extract `inject_is_active` helper in gateway handler

**Context**: In `embedding_providers.rs`, the pattern of injecting `is_active` into serialized JSON is repeated in `handle_list` (lines 40-47) and `handle_get` (lines 86-92).

```rust
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

**Line reduction**: ~10 lines

---

### Action 6: Extract nullable column helpers in arrow_convert.rs

**Context**: The pattern of checking `is_null(row)` before reading a value is repeated for every nullable column type in every `record_batch_to_*` function.

**New helpers** (add near existing `col()` helper):

```rust
fn col_str_opt(batch: &RecordBatch, name: &str, row: usize) -> Result<Option<String>, AlephError> {
    let arr: &StringArray = col(batch, name)?;
    Ok(if arr.is_null(row) { None } else { Some(arr.value(row).to_string()) })
}

fn col_i64_opt(batch: &RecordBatch, name: &str, row: usize) -> Result<Option<i64>, AlephError> {
    let arr: &Int64Array = col(batch, name)?;
    Ok(if arr.is_null(row) { None } else { Some(arr.value(row)) })
}
```

**Impact**: Reduces multi-line nullable reads to single-line calls throughout `record_batch_to_facts()`, `record_batch_to_graph_nodes()`, etc.

**Estimated line reduction**: ~40-60 lines in arrow_convert.rs

---

## Out of Scope

- **api.rs file split**: Structural change deferred. The file is large (1,275 lines) but each section is self-contained.
- **UI component refactoring**: `embedding_providers.rs` (view) is typical Leptos component code; duplication is inherent to the reactive pattern.
- **schema.rs changes**: File is clean and well-tested.

---

## Validation Plan

1. After each action, run `cargo build` to verify compilation
2. After all actions, run `cargo clippy` to verify zero warnings
3. Verify no public API changes (all helpers are `pub(crate)` or private)
4. Existing tests must continue to pass

---

## Summary

| Action | Target | Impact | Risk |
|--------|--------|--------|------|
| 1. Shared `parse_params` | gateway/handlers | -120 lines, DRY | Low |
| 2. Shared `lance_err` | lance/ | -9 lines, DRY | Minimal |
| 3. Merge `count_rows` | sessions.rs | -12 lines, DRY | Minimal |
| 4. Fix lock-during-I/O | embedding_manager.rs | Bug fix | Low |
| 5. Extract `inject_is_active` | embedding_providers.rs | -10 lines, DRY | Minimal |
| 6. Nullable column helpers | arrow_convert.rs | -40-60 lines, DRY | Low |

**Total estimated reduction**: ~190-210 lines
**Risk level**: Low (all helpers are internal, no public API changes)
