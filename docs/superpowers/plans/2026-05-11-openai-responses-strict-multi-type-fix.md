# OpenAI Responses Strict-Mode Multi-Type Schema Fix — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Aleph's `openai_responses` protocol strict-mode normalizer handle multi-type `"type": [...]` schemas. Rewrite `["null", X]` to `anyOf`; bail with per-tool downgrade for unrepresentable shapes. Fix the `tools[25] desktop` 400 from Step 2 e2e.

**Architecture:** Extend `normalize_strict_schema` to return a `StrictResult { Ok, Incompatible }` signal. `shared::build_tools` consumes the signal, sets `strict: None` per tool when needed, and emits a `tracing::warn!` audit log. Two-commit scope. No new modules.

**Tech Stack:** Rust 2024, `serde_json::Value` patching, `tracing::warn!`.

**Spec:** [`docs/superpowers/specs/2026-05-11-openai-responses-strict-multi-type-fix.md`](../specs/2026-05-11-openai-responses-strict-multi-type-fix.md) (commit `17ea16022`)

**Predecessors:** Step 2 fully shipped at `6edf18f73`; Step 3 spec at `17ea16022`. HEAD at plan write time: `17ea16022`.

**Verification Strategy:** Same as Step 1/2 — `cargo check -p alephcore` after each significant change (baseline 484 pre-existing test compile errors from openai protocol split, unrelated to Step 3). Manual e2e (Tasks 14-16) covers runtime correctness against T8Star.

---

## Commit 1 — Normalizer with Multi-Type Handling

Files touched (1):
- `src/providers/protocols/openai_common/openai_strict_schema.rs` (only file in Commit 1)

### Task 1: Add `StrictResult` enum

**Files:**
- Modify: `src/providers/protocols/openai_common/openai_strict_schema.rs:1-7` (module header)

- [ ] **Step 1.1: Read current header + first function**

Run: `sed -n '1,20p' /Volumes/TBU4/Workspace/Aleph/src/providers/protocols/openai_common/openai_strict_schema.rs`

Expected: Module docstring + `use serde_json::Value;` + `pub fn normalize_strict_schema(...)`.

- [ ] **Step 1.2: Insert `StrictResult` enum above `normalize_strict_schema`**

Add immediately after `use serde_json::Value;` (around line 8), before `/// Recursively normalize ...`:

```rust
/// Outcome of normalizing a JSON schema for OpenAI strict mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrictResult {
    /// Schema is fully normalized and strict-compatible.
    Ok,
    /// Schema contains a sub-tree that cannot be expressed in strict mode.
    /// Caller should downgrade the affected tool to `strict: None`.
    Incompatible {
        /// Diagnostic for `tracing::warn!`. Format:
        /// `"<json-pointer>: <description>"` e.g.
        /// `".properties.actions.items: multi-type schema is not strict-compatible (types: [boolean,object,array,number,string,integer,null])"`.
        reason: String,
    },
}
```

- [ ] **Step 1.3: Verify compile (lib only)**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore 2>&1 | tail -3`

Expected: 0 errors. `StrictResult` is dead-code-warned until Task 2 uses it — fine.

### Task 2: Change `normalize_strict_schema` signature to return `StrictResult`

**Files:**
- Modify: `src/providers/protocols/openai_common/openai_strict_schema.rs` (entry point + helper)

- [ ] **Step 2.1: Read current function bodies**

Run: `sed -n '15,57p' /Volumes/TBU4/Workspace/Aleph/src/providers/protocols/openai_common/openai_strict_schema.rs`

Expected: `pub fn normalize_strict_schema(schema: &mut Value, set_top_level_strict: bool)` calling `normalize_node(schema, set_top_level_strict, true)`. `normalize_node` returns `()`.

- [ ] **Step 2.2: Change `normalize_strict_schema` signature**

Replace the `pub fn normalize_strict_schema(...)` body with:

```rust
pub fn normalize_strict_schema(schema: &mut Value, set_top_level_strict: bool) -> StrictResult {
    normalize_node(schema, set_top_level_strict, true, "")
}
```

(Note: added a 4th argument `&str` for the JSON-pointer path prefix, used by later tasks. For now pass `""`.)

- [ ] **Step 2.3: Change `normalize_node` signature to match**

Replace the current `fn normalize_node(node: &mut Value, set_strict: bool, is_top_level: bool)` signature with:

```rust
fn normalize_node(
    node: &mut Value,
    set_strict: bool,
    is_top_level: bool,
    path: &str,
) -> StrictResult {
```

For now, leave the body unchanged but add `StrictResult::Ok` as the trailing expression (or insert it at every early exit). Specifically:

```rust
fn normalize_node(
    node: &mut Value,
    set_strict: bool,
    is_top_level: bool,
    path: &str,
) -> StrictResult {
    if let Value::Object(map) = node {
        if is_top_level && set_strict {
            map.insert("strict".to_string(), Value::Bool(true));
        }

        let is_object = map
            .get("type")
            .and_then(|t| t.as_str())
            == Some("object");

        if is_object {
            if !map.contains_key("properties") {
                map.insert("properties".to_string(), Value::Object(Default::default()));
            }

            map.insert("additionalProperties".to_string(), Value::Bool(false));

            if let Some(Value::Object(props)) = map.get_mut("properties") {
                for (k, prop_schema) in props.iter_mut() {
                    let child_path = format!("{path}.properties.{k}");
                    let result = normalize_node(prop_schema, false, false, &child_path);
                    if matches!(result, StrictResult::Incompatible { .. }) {
                        return result;
                    }
                }
            }
        }

        if let Some(items) = map.get_mut("items") {
            let child_path = format!("{path}.items");
            let result = normalize_node(items, false, false, &child_path);
            if matches!(result, StrictResult::Incompatible { .. }) {
                return result;
            }
        }

        for key in &["anyOf", "allOf", "oneOf"] {
            if let Some(Value::Array(variants)) = map.get_mut(*key) {
                for (idx, variant) in variants.iter_mut().enumerate() {
                    let child_path = format!("{path}.{key}[{idx}]");
                    let result = normalize_node(variant, false, false, &child_path);
                    if matches!(result, StrictResult::Incompatible { .. }) {
                        return result;
                    }
                }
            }
        }
    }
    StrictResult::Ok
}
```

This restructures the body to:
- Propagate `Incompatible` from any recursive call
- Track JSON-pointer path for the reason field in later tasks
- Return `Ok` at the end if all recursion paths complete successfully

- [ ] **Step 2.4: Update existing 8 test call sites in `mod tests`**

Run: `grep -n 'normalize_strict_schema(' /Volumes/TBU4/Workspace/Aleph/src/providers/protocols/openai_common/openai_strict_schema.rs`

Expected: 9 call sites total — 1 in production, 8 in mod tests.

For each of the 8 test call sites, wrap or assert the returned `StrictResult`:

```rust
// Before:
normalize_strict_schema(&mut schema, false);

// After (in each existing test):
assert_eq!(
    normalize_strict_schema(&mut schema, false),
    StrictResult::Ok,
);
```

(8 tests touched: `test_inject_additional_properties`, `test_ensure_properties_exists`, `test_nested_objects`, `test_top_level_strict_flag`, `test_array_items`, `test_any_of_composite`, `test_preserves_required`, `test_non_object_unchanged`.)

- [ ] **Step 2.5: Verify compile**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore --lib 2>&1 | tail -3`

Expected: 0 errors.

Also check production call site in `shared.rs`:

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore 2>&1 | grep -E 'normalize_strict_schema|shared\.rs' | head -5`

Expected: a compile error in `shared.rs:168` because `normalize_strict_schema` now returns `StrictResult` not `()`. Acceptable — fixed in Commit 2.

To unblock Commit 1, **temporarily** add `let _ =` in `shared.rs` to consume the return:

Run: `sed -i '' 's|crate::providers::protocols::openai_common::openai_strict_schema::normalize_strict_schema(\&mut params, true);|let _ = crate::providers::protocols::openai_common::openai_strict_schema::normalize_strict_schema(\&mut params, true);|' /Volumes/TBU4/Workspace/Aleph/src/providers/responses/shared.rs`

Then re-run: `cargo check -p alephcore 2>&1 | tail -3`. Expected: 0 errors.

This `let _ =` will be replaced by the proper match in Commit 2 Task 10.

### Task 3: Add `["null", X]` → `anyOf` transformation (TDD)

**Files:**
- Modify: `src/providers/protocols/openai_common/openai_strict_schema.rs`

- [ ] **Step 3.1: Write failing test (TDD red)**

Append inside `mod tests { ... }` (before the closing brace) — the existing test block ends around line 169:

```rust
    #[test]
    fn multi_type_null_pair_becomes_anyof() {
        let mut schema = serde_json::json!({
            "type": ["null", "string"]
        });
        let result = normalize_strict_schema(&mut schema, false);
        assert_eq!(result, StrictResult::Ok);
        let any_of = schema["anyOf"].as_array().expect("anyOf array");
        assert_eq!(any_of.len(), 2);
        // First branch: null
        assert_eq!(any_of[0]["type"], "null");
        // Second branch: string
        assert_eq!(any_of[1]["type"], "string");
        // Top-level type is gone
        assert!(schema.get("type").is_none());
    }
```

- [ ] **Step 3.2: Verify red**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore --lib --tests 2>&1 | grep -E 'multi_type_null_pair_becomes_anyof|FAILED' | head -3`

If it compiles, the test runs but fails at the `any_of` line (since current code doesn't transform multi-type).

- [ ] **Step 3.3: Implement the transformation**

In `normalize_node`, add detection before the `is_object` check:

```rust
fn normalize_node(
    node: &mut Value,
    set_strict: bool,
    is_top_level: bool,
    path: &str,
) -> StrictResult {
    if let Value::Object(map) = node {
        if is_top_level && set_strict {
            map.insert("strict".to_string(), Value::Bool(true));
        }

        // NEW: multi-type handling
        if let Some(Value::Array(types)) = map.get("type").cloned().as_ref() {
            // ... (handled in Task 5)
            // For Task 3: handle only the ["null", X] case here.
            if types.len() == 2 {
                let null_idx = types.iter().position(|t| t.as_str() == Some("null"));
                let other_idx = types.iter().position(|t| t.as_str().is_some_and(|s| s != "null"));
                if let (Some(_), Some(other)) = (null_idx, other_idx) {
                    let other_type = types[other].clone();
                    let map_clone: serde_json::Map<String, Value> = map.iter()
                        .filter(|(k, _)| k.as_str() != "type")
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    // Non-null branch carries sibling keywords + the single type
                    let mut non_null_branch = serde_json::Map::new();
                    non_null_branch.insert("type".to_string(), other_type);
                    for (k, v) in map_clone {
                        non_null_branch.insert(k, v);
                    }
                    // Replace map contents with {"anyOf": [...]} only
                    map.clear();
                    map.insert(
                        "anyOf".to_string(),
                        Value::Array(vec![
                            serde_json::json!({"type": "null"}),
                            Value::Object(non_null_branch),
                        ]),
                    );
                    // Recurse into the non-null branch
                    if let Some(Value::Array(arr)) = map.get_mut("anyOf") {
                        if let Some(non_null) = arr.get_mut(1) {
                            let result = normalize_node(non_null, false, false, path);
                            if matches!(result, StrictResult::Incompatible { .. }) {
                                return result;
                            }
                        }
                    }
                    return StrictResult::Ok;
                }
            }
            // Multi-type that's not exactly ["null", X] — Task 5 handles.
        }

        // ... existing is_object handling ...
```

- [ ] **Step 3.4: Verify green**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore --lib --tests 2>&1 | grep -E 'error\[|warning: unused' | head -5`

Expected: 0 errors. Then run mod test (if 484 baseline doesn't block):

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib openai_strict_schema::tests::multi_type_null_pair_becomes_anyof 2>&1 | tail -10`

Expected: 1 passed. If baseline 484 errors block, that's documented OOS — skip this run, trust the cargo check.

### Task 4: Sibling-keyword preservation test

**Files:**
- Modify: `src/providers/protocols/openai_common/openai_strict_schema.rs` mod tests

- [ ] **Step 4.1: Add test**

Append to `mod tests`:

```rust
    #[test]
    fn null_pair_preserves_sibling_keywords() {
        let mut schema = serde_json::json!({
            "type": ["null", "string"],
            "description": "an optional label",
            "minLength": 1
        });
        let result = normalize_strict_schema(&mut schema, false);
        assert_eq!(result, StrictResult::Ok);
        let non_null_branch = &schema["anyOf"][1];
        assert_eq!(non_null_branch["type"], "string");
        assert_eq!(non_null_branch["description"], "an optional label");
        assert_eq!(non_null_branch["minLength"], 1);
    }
```

- [ ] **Step 4.2: Verify it passes already**

The Task 3 implementation already copies sibling keys (`map_clone` filters out `"type"` and keeps the rest, inserts them onto the non-null branch). So this test should pass on the first run.

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib openai_strict_schema::tests::null_pair_preserves_sibling_keywords 2>&1 | tail -3` (or skip if baseline blocks).

Cargo check: `cargo check -p alephcore --lib --tests 2>&1 | tail -3` → 0 errors.

### Task 5: Multi-type bail (Incompatible)

**Files:**
- Modify: `src/providers/protocols/openai_common/openai_strict_schema.rs`

- [ ] **Step 5.1: Write failing tests (TDD red)**

Append to `mod tests`:

```rust
    #[test]
    fn multi_type_any_value_returns_incompatible() {
        let mut schema = serde_json::json!({
            "type": ["boolean", "object", "array", "number", "string", "integer", "null"]
        });
        let result = normalize_strict_schema(&mut schema, false);
        match result {
            StrictResult::Incompatible { reason } => {
                assert!(reason.contains("multi-type"), "reason: {reason}");
            }
            other => panic!("expected Incompatible, got {other:?}"),
        }
    }

    #[test]
    fn multi_type_non_null_pair_returns_incompatible() {
        let mut schema = serde_json::json!({
            "type": ["string", "number"]
        });
        let result = normalize_strict_schema(&mut schema, false);
        assert!(matches!(result, StrictResult::Incompatible { .. }));
    }
```

- [ ] **Step 5.2: Verify red**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore --lib --tests 2>&1 | tail -3`

Expected: 0 errors (Task 3's `["null", X]` branch doesn't match these cases, so they currently fall through to `Ok`, failing the assertions).

- [ ] **Step 5.3: Implement bail**

In `normalize_node`, extend the `Some(Value::Array(types))` arm. Replace the inner `if types.len() == 2 { ... }` block with:

```rust
        // NEW: multi-type handling
        if let Some(Value::Array(types)) = map.get("type").cloned().as_ref() {
            // Case 1: exactly ["null", X] with one non-null type → anyOf transform
            if types.len() == 2 {
                let null_idx = types.iter().position(|t| t.as_str() == Some("null"));
                let other_idx = types.iter().position(|t| t.as_str().is_some_and(|s| s != "null"));
                if let (Some(_), Some(other)) = (null_idx, other_idx) {
                    // ... (existing anyOf transformation code from Task 3) ...
                    return StrictResult::Ok;
                }
            }
            // Case 2: anything else (multi non-null, or length != 2) → bail
            let type_list = types
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<&str>>()
                .join(",");
            return StrictResult::Incompatible {
                reason: format!(
                    "{path}: multi-type schema is not strict-compatible (types: [{type_list}])"
                ),
            };
        }
```

- [ ] **Step 5.4: Verify green**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore --lib --tests 2>&1 | tail -3`

Expected: 0 errors.

If `cargo test` works (skip if 484 baseline blocks):
Run: `cargo test -p alephcore --lib openai_strict_schema 2>&1 | tail -10`
Expected: all 13 tests pass (8 existing + 5 new: null_pair_becomes_anyof, preserves_sibling, any_value_incompatible, non_null_pair_incompatible).

### Task 6: Short-circuit through nesting + reason path prefixing

**Files:**
- Modify: `src/providers/protocols/openai_common/openai_strict_schema.rs`

- [ ] **Step 6.1: Write test**

Append to `mod tests`:

```rust
    #[test]
    fn incompatible_short_circuits_through_nesting() {
        // Mimic desktop tool: actions field is array of any-value
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": {
                "actions": {
                    "type": "array",
                    "items": {
                        "type": ["boolean", "object", "array", "number", "string", "integer", "null"]
                    }
                }
            }
        });
        let result = normalize_strict_schema(&mut schema, false);
        match result {
            StrictResult::Incompatible { reason } => {
                assert!(
                    reason.contains(".properties.actions.items"),
                    "reason should carry JSON-pointer path: {reason}"
                );
                assert!(reason.contains("multi-type"), "reason: {reason}");
            }
            other => panic!("expected Incompatible, got {other:?}"),
        }
    }
```

- [ ] **Step 6.2: Verify**

The path prefixing logic was already added in Task 2 (each recursive call computes `child_path = format!("{path}.properties.{k}")` etc.), and the Task 5 reason string uses `path`. So the short-circuit + path prefix should already work.

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore --lib --tests 2>&1 | tail -3`

Expected: 0 errors.

If cargo test works:
Run: `cargo test -p alephcore --lib openai_strict_schema::tests::incompatible_short_circuits_through_nesting 2>&1 | tail -3`
Expected: 1 passed.

### Task 7: Clippy + sanity sweep

- [ ] **Step 7.1: Full clippy on touched file**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo clippy -p alephcore --lib --no-deps 2>&1 | grep -E 'openai_strict_schema\.rs' | head -10`

Expected: no new lints attributable to the changes (pre-existing warnings allowed).

- [ ] **Step 7.2: Full check**

Run: `cargo check -p alephcore 2>&1 | tail -3`

Expected: `Finished dev` 0 errors.

### Task 8: Commit 1

- [ ] **Step 8.1: Review diff**

Run: `cd /Volumes/TBU4/Workspace/Aleph && git diff --stat`

Expected:
- `src/providers/protocols/openai_common/openai_strict_schema.rs` ~120 insertions (enum + new branches + 5 new tests + 8 updated assertions)
- `src/providers/responses/shared.rs` 1 insertion (temporary `let _ =` from Task 2)

- [ ] **Step 8.2: Stage + commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git add src/providers/protocols/openai_common/openai_strict_schema.rs \
        src/providers/responses/shared.rs
git commit -m "$(cat <<'EOF'
providers/openai: handle multi-type schemas in strict normalizer

Step 3 commit 1 of 2. Extends OpenAI Responses strict-mode normalizer to
handle JSON Schema multi-type fields produced by schemars from
`serde_json::Value` and `Option<T>` shapes. The previous implementation
silently ignored `type: [...]` arrays, producing payloads that OpenAI
rejected with 400 `invalid_function_parameters`.

Changes:
- Add `StrictResult { Ok, Incompatible { reason } }` enum.
- `normalize_strict_schema` now returns `StrictResult` (was `()`).
- New private helper signature: `normalize_node(node, set_strict,
  is_top_level, path) -> StrictResult`, threading a JSON-pointer path
  through the recursion for reason diagnostics.
- Case A: `type: ["null", X]` (exactly 2 elements, one null) → rewrite
  to `{"anyOf": [{"type":"null"}, {<sibling keys>, "type": X}]}` (option
  α from spec §4). Recurse into the non-null branch.
- Case B: `type: [...]` of any other shape (multi non-null, or length
  != 2, includes the 7-type "any-value" from `serde_json::Value`) →
  return `Incompatible { reason }` with the offending JSON pointer
  prefixed.
- Recursive calls short-circuit on `Incompatible`.
- 5 new mod tests: null_pair → anyOf; null_pair preserves siblings;
  7-type any-value → Incompatible; non-null pair → Incompatible;
  nested any-value → Incompatible with correct path.
- 8 existing mod tests updated to `assert_eq!(..., StrictResult::Ok)`.

shared.rs gets a temporary `let _ =` over the new return value to keep
the build green; commit 2 replaces it with the proper match arms.

cargo check -p alephcore: 0 errors. No new clippy lints on touched file.
EOF
)"
git log -1 --format='%h %s'
```

Expected: New commit hash printed.

---

## Commit 2 — Per-Tool Strict Downgrade Wiring

Files touched (2):
- `src/providers/responses/shared.rs` (main change)
- `CHANGELOG.md`

### Task 9: Write `build_tools` strict-kept-for-well-typed-tools test (TDD red-green)

**Files:**
- Modify: `src/providers/responses/shared.rs` mod tests (or test file if separated)

- [ ] **Step 9.1: Locate existing tests in shared.rs**

Run: `grep -n '#\[cfg(test)\]\|fn build_tools\|fn ensure_properties' /Volumes/TBU4/Workspace/Aleph/src/providers/responses/shared.rs | head -10`

Expected: `pub(crate) fn build_tools(...)` around line 154; possibly an existing `#[cfg(test)] mod tests`. If no mod tests exists, this task adds one.

- [ ] **Step 9.2: Identify `ToolDefinition` and `FunctionToolDef` imports**

Run: `grep -n 'use crate.*ToolDefinition\|use.*FunctionToolDef\|FunctionToolDef\b' /Volumes/TBU4/Workspace/Aleph/src/providers/responses/shared.rs | head -10`

Note the imports for use in the test.

- [ ] **Step 9.3: Add or extend `mod tests` with the well-typed test**

If `mod tests` does not exist, add at end of file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool(name: &str, parameters: serde_json::Value) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: "test tool".to_string(),
            parameters,
            // Add other ToolDefinition fields if required by struct definition.
            // Inspect via `grep -n 'pub struct ToolDefinition' <path>` and fill defaults.
            ..Default::default()
        }
    }

    #[test]
    fn build_tools_keeps_strict_for_well_typed_tools() {
        let td = make_tool(
            "well_typed",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"}
                },
                "required": ["name"]
            }),
        );
        let out = build_tools(Some(&[td]), true).expect("Some tools");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].strict, Some(true));
    }
}
```

If `ToolDefinition` doesn't derive `Default`, replace the `..Default::default()` with explicit field values. Use `grep -n 'pub struct ToolDefinition' src/ -r` to find the struct definition and copy its required-field shape.

- [ ] **Step 9.4: Verify compile**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore --lib --tests 2>&1 | tail -3`

Expected: 0 errors (if `ToolDefinition` construction is correct).

If the test runs (484 baseline permitting), it should pass — current `build_tools` with strict-compatible schema returns `strict: Some(true)`.

### Task 10: Write build_tools downgrade test + implement (TDD red-green)

**Files:**
- Modify: `src/providers/responses/shared.rs`

- [ ] **Step 10.1: Write failing test (TDD red)**

Append to `mod tests`:

```rust
    #[test]
    fn build_tools_downgrades_tool_with_any_value_field() {
        // Mimics desktop tool's actions: Option<Vec<serde_json::Value>>
        let td = make_tool(
            "desktop_like",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "actions": {
                        "type": "array",
                        "items": {
                            "type": ["boolean", "object", "array", "number", "string", "integer", "null"]
                        }
                    }
                }
            }),
        );
        let out = build_tools(Some(&[td]), true).expect("Some tools");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].strict, None, "should downgrade to non-strict");
        // Parameters should still have properties present (ensure_properties_recursive applied)
        assert!(out[0].parameters["properties"].is_object());
    }
```

- [ ] **Step 10.2: Verify red**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore --lib --tests 2>&1 | tail -3`

Expected: 0 errors. If `cargo test` works, this test fails because current code still uses `let _ =` and sets `strict: Some(true)` unconditionally.

- [ ] **Step 10.3: Implement downgrade branch**

Replace the existing `if enable_strict { let _ = ...; } else { ensure_properties_recursive(...); }` block in `build_tools` (around line 167-171 after the Commit 1 patch) with:

```rust
                let strict = if enable_strict {
                    match crate::providers::protocols::openai_common::openai_strict_schema::normalize_strict_schema(
                        &mut params,
                        true,
                    ) {
                        crate::providers::protocols::openai_common::openai_strict_schema::StrictResult::Ok => Some(true),
                        crate::providers::protocols::openai_common::openai_strict_schema::StrictResult::Incompatible { reason } => {
                            tracing::warn!(
                                tool_name = %td.name,
                                reason = %reason,
                                "OpenAI strict mode incompatible — downgrading this tool to non-strict",
                            );
                            // Reset params from the original and apply non-strict normalization.
                            params = td.parameters.clone();
                            if let Some(obj) = params.as_object_mut() {
                                obj.remove("$schema");
                                obj.remove("title");
                            }
                            ensure_properties_recursive(&mut params);
                            None
                        }
                    }
                } else {
                    ensure_properties_recursive(&mut params);
                    None
                };
```

Then in the `FunctionToolDef` construction below, replace:

```rust
                    strict: if enable_strict { Some(true) } else { None },
```

with:

```rust
                    strict,
```

Add the `tracing` import at the top of `shared.rs` if not already there:

Run: `grep -n '^use tracing' /Volumes/TBU4/Workspace/Aleph/src/providers/responses/shared.rs`

If absent, add `use tracing;` near the other `use` statements (or just qualify as `tracing::warn!` which works without `use` when `tracing` is in the crate's dependencies).

- [ ] **Step 10.4: Verify green**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore 2>&1 | tail -3`

Expected: 0 errors.

If `cargo test` works:
Run: `cargo test -p alephcore --lib build_tools_downgrades_tool_with_any_value_field build_tools_keeps_strict_for_well_typed_tools 2>&1 | tail -10`
Expected: 2 passed.

### Task 11: CHANGELOG entry

**Files:**
- Modify: `CHANGELOG.md`

- [ ] **Step 11.1: Read current Unreleased section**

Run: `head -30 /Volumes/TBU4/Workspace/Aleph/CHANGELOG.md`

Locate `[Unreleased]` + its subsections.

- [ ] **Step 11.2: Append `### Fixed` entry**

Add to `[Unreleased]` → `### Fixed` (create the subsection if it doesn't exist; standard order is Added → Changed → Fixed):

```markdown
- OpenAI Responses strict-mode normalizer now handles multi-type JSON schemas. `["null", X]` (Option<T>-shaped) is rewritten to `anyOf` with sibling-keyword preservation; other multi-type shapes (e.g., the 7-type "any-value" emitted by schemars for `serde_json::Value`) trigger a per-tool strict downgrade via `tracing::warn!` audit log. Fixes 400 `invalid_function_parameters` errors on tools containing `serde_json::Value` fields (e.g., `desktop` tool's `actions: Option<Vec<serde_json::Value>>`).
```

### Task 12: Final clippy + verify

- [ ] **Step 12.1: Full check**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore 2>&1 | tail -3`

Expected: `Finished dev` 0 errors.

- [ ] **Step 12.2: Clippy on touched files**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo clippy -p alephcore --lib --no-deps 2>&1 | grep -E 'shared\.rs|openai_strict_schema\.rs' | head -20`

Expected: no NEW lints on touched lines (pre-existing import warnings allowed).

### Task 13: Commit 2

- [ ] **Step 13.1: Diff stat**

Run: `cd /Volumes/TBU4/Workspace/Aleph && git diff --stat`

Expected: 2 files modified (`shared.rs` + `CHANGELOG.md`), ~40 insertions, ~5 deletions.

- [ ] **Step 13.2: Stage + commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git add CHANGELOG.md src/providers/responses/shared.rs
git commit -m "$(cat <<'EOF'
providers/openai: per-tool strict downgrade for incompatible schemas

Step 3 commit 2 of 2. Wires the StrictResult signal from commit 1 into
shared::build_tools and adds an audit log for downgraded tools.

When a tool's parameter schema cannot be expressed in OpenAI strict
mode (e.g., contains `serde_json::Value` producing a 7-type "any-value"
schema), build_tools now:
- Resets params from the original ToolDefinition (since normalize_strict_schema
  may have partially mutated them before bailing).
- Applies the non-strict normalization path (ensure_properties_recursive)
  instead.
- Emits a tracing::warn! audit log with the tool name and the JSON-pointer
  reason.
- Sets strict: None on that specific tool. Other tools in the same request
  keep their strict: Some(true) flag.

Replaces the temporary `let _ =` from commit 1 with the proper match.

2 new mod tests: well-typed tool keeps strict; any-value-shaped tool
downgrades to non-strict.

cargo check -p alephcore: 0 errors. No new clippy lints on touched files.
EOF
)"
git log -1 --format='%h %s'
```

Expected: New commit hash printed.

---

## Manual e2e (no commit)

### Task 14: Restart server with `default_provider` routed to OpenAI Responses

- [ ] **Step 14.1: Set default to a Responses-protocol provider**

In `~/.aleph/config.toml`, change `[general]` to `default_provider = "T8Star"` (or another `openai-responses` provider visible in the running config).

```bash
sed -i '' 's|^default_provider = "kimi-for-coding"|default_provider = "T8Star"|' /Users/zouguojun/.aleph/config.toml
grep -n '^default_provider' /Users/zouguojun/.aleph/config.toml
```

Expected: `default_provider = "T8Star"`.

- [ ] **Step 14.2: Stop current server + restart in background with debug logging**

```bash
# Find and stop running aleph-server
PID=$(pgrep -f 'target/release/aleph-server start' | head -1)
if [ -n "$PID" ]; then kill "$PID"; sleep 2; fi

# Restart with verbose logging on the responses path
RUST_LOG=alephcore::providers::responses=debug,alephcore::providers::protocols::openai_common::openai_strict_schema=debug,alephcore::gateway=info,alephcore=info \
  /Volumes/TBU4/Workspace/Aleph/target/release/aleph-server start \
  > /tmp/aleph-server-step3.out 2>&1 &
echo "spawned pid=$!"
sleep 6
grep -E 'Providers:|listening|default_provider|T8Star' /tmp/aleph-server-step3.out | tail -10
```

Expected: `Providers: 5 registered (default: T8Star)`, server listening on 18790.

### Task 15: Send a webchat conversation

- [ ] **Step 15.1: Send via webchat panel**

Open the running panel at `http://127.0.0.1:18790/` and send any non-trivial message that will exercise the tools array (e.g., "请总结一下当前桌面上有什么内容" — model may invoke `desktop` tool).

- [ ] **Step 15.2: Observe behavior**

The expected flow:
- `build_tools` runs with `enable_strict = true`
- `desktop` tool hits the multi-type bail → strict downgrade
- `tracing::warn!` "OpenAI strict mode incompatible — downgrading this tool to non-strict" appears in `/tmp/aleph-server-step3.out` with `tool_name = "desktop"`
- Request goes out to T8Star with desktop tool having `strict: None` (or just omitted)
- T8Star accepts the request (no more 400 `invalid_function_parameters`)
- Conversation completes successfully

### Task 16: Verify e2e

- [ ] **Step 16.1: Grep for the audit log**

```bash
grep -E 'OpenAI strict mode incompatible|invalid_function_parameters|tools\[' /tmp/aleph-server-step3.out | tail -10
```

Expected:
- At least one `OpenAI strict mode incompatible — downgrading this tool to non-strict` with `tool_name=desktop`.
- ZERO `invalid_function_parameters` errors for `desktop`.

- [ ] **Step 16.2: Confirm successful dispatch**

```bash
grep -E 'Agent execution completed|Orchestrator dispatch (completed|failed)' /tmp/aleph-server-step3.out | tail -5
```

Expected: at least one `Agent execution completed, ..., success=true` for the run that exercised the `desktop` tool.

- [ ] **Step 16.3: Revert config**

```bash
sed -i '' 's|^default_provider = "T8Star"|default_provider = "kimi-for-coding"|' /Users/zouguojun/.aleph/config.toml
grep -n '^default_provider' /Users/zouguojun/.aleph/config.toml
```

(Do NOT commit the toml — it's a local override.)

### Task 17: Final clean-up

- [ ] **Step 17.1: Verify git status is clean**

Run: `cd /Volumes/TBU4/Workspace/Aleph && git status --short`

Expected: empty (or only this plan doc as untracked, which will be committed separately).

- [ ] **Step 17.2: Verify commit chain**

Run: `cd /Volumes/TBU4/Workspace/Aleph && git log --oneline -8`

Expected (most recent first):
- `<sha>` providers/openai: per-tool strict downgrade for incompatible schemas (Commit 2)
- `<sha>` providers/openai: handle multi-type schemas in strict normalizer (Commit 1)
- `17ea16022` docs: anthropic step 3 spec — openai responses strict multi-type schema fix
- `6edf18f73` providers/anthropic: hoist use super:: into top import block (Step 2 last)
- ... earlier

- [ ] **Step 17.3: Optional — commit this plan doc**

If the project convention is to commit plan docs (Step 1 + Step 2 did):

```bash
cd /Volumes/TBU4/Workspace/Aleph
git add docs/superpowers/plans/2026-05-11-openai-responses-strict-multi-type-fix.md
git commit -m "docs: anthropic step 3 implementation plan"
```

---

## Self-Review Checklist

(Performed after writing the full plan, before execution.)

- [x] **Spec coverage** — every spec § (1 problem, 2 goal, 3 architecture, 4 decision table, 5 API, 6 categorization, 7 recursion+short-circuit, 8 tests, 14 verification, 15 acceptance) has a Task. Risk register (§ 12) and OOS (§ 13) don't generate tasks — by design.
- [x] **Placeholder scan** — no "TBD", "etc.", "and similar". Task 9 references `..Default::default()` with a fallback instruction if `ToolDefinition` doesn't derive Default — documented, not placeholdered.
- [x] **Type consistency** — `StrictResult { Ok, Incompatible }`, `normalize_strict_schema`, `normalize_node`, `build_tools`, `ensure_properties_recursive`, `FunctionToolDef.strict` — every reference uses the same name across tasks.
- [x] **Commit message accuracy** — Commit 1 and Commit 2 messages reflect their respective scopes (signature + transformations vs. wiring + audit log).
- [x] **TDD red-green explicit** — Tasks 3, 5, 9, 10 each write tests first, verify red (or note "may pass already"), then implement.
- [x] **Adversarial scenarios covered** — `Option<T>`, `serde_json::Value` 7-type, non-null pair, nested any-value with path prefix.
- [x] **R7/R10 compliance** — every decision arm is a pure structural match; no LLM, no scoring, no policy.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-11-openai-responses-strict-multi-type-fix.md`. Two execution options:

**1. Subagent-Driven (recommended)** — Fresh subagent per task, review between tasks, fast iteration. Mirror of Step 1/2 execution mode that produced commits `c001f1d7c`, `e62032df9`, `123a83c5d`, `c58dea3c4`.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
