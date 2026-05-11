# OpenAI Responses Strict-Mode Multi-Type Schema Fix

**Status:** Design approved 2026-05-11. Plan + execution pending.
**Predecessors:** `ff38ff758` (OpenAI protocol split that introduced the strict normalizer), `6edf18f73` (Step 2 prompt-cache HEAD).
**Severity:** HIGH — blocks all chat flows that route through `openai_responses` protocol when any active tool's parameter schema contains a multi-type field (commonly `serde_json::Value` or `Option<T>` depending on schemars output).

---

## 1. Problem Statement

When `provider_factory` routes a request through the `openai_responses` protocol with `enable_strict = true`, `shared::build_tools` calls `normalize_strict_schema` on each `ToolDefinition.parameters`. The current normalizer (`src/providers/protocols/openai_common/openai_strict_schema.rs`) handles single-type schemas only — it never inspects nor transforms `type: [...]` multi-type arrays.

Observed failure (Step 2 e2e on 2026-05-11, run_id `b596268b-527c-4433-859c-164fc935676b`):

```
Provider error: OpenAI Responses API error (400 Bad Request):
{"error":{"message":"Invalid schema for function 'desktop':
  In context=('properties', 'actions', 'type', '0'),
  array schema items is not an object.",
  "type":"invalid_request_error",
  "param":"tools[25].parameters",
  "code":"invalid_function_parameters"}}
```

Root cause: `src/builtin_tools/desktop/types.rs:138`

```rust
pub actions: Option<Vec<serde_json::Value>>,
```

`serde_json::Value`'s schemars output is `{"type": ["boolean","object","array","number","string","integer","null"]}` — a multi-type "any-value" schema. OpenAI strict mode rejects it.

---

## 2. Goal

Make the strict normalizer aware of multi-type schemas and emit a per-tool downgrade signal when the schema cannot be expressed in strict mode. Tools with `Option<T>`-style nullable fields continue to enjoy strict guarantees via an `anyOf` rewrite; tools with truly untypable fields gracefully degrade to non-strict instead of producing 400-rejecting payloads.

---

## 3. Architecture

Single-point fix in two files:

1. `src/providers/protocols/openai_common/openai_strict_schema.rs` — extend `normalize_strict_schema` with multi-type handling and a `StrictResult` return type.
2. `src/providers/responses/shared.rs:154` (`build_tools`) — consume `StrictResult` to set `strict: None` for downgraded tools and emit a `tracing::warn!` audit log.

No new crates. No new modules. No public-API surface changes outside the two files above.

---

## 4. Decision Table

The normalizer dispatches on the shape of `type` at every traversed node:

| Shape of `type` | Action |
|---|---|
| Single string (`"object"`, `"string"`, …) | Existing behavior — inject `additionalProperties: false`, ensure `properties`, recurse |
| Array of length 2 containing exactly one `"null"` and one other type, e.g. `["null", "string"]` | **NEW** — rewrite node to `{"anyOf": [{"type":"null"}, {<sibling keys>, "type": <other>}]}` and recurse into the non-null branch |
| Array of any other shape (length ≠ 2, or 2 non-null types, or 0 elements) | **NEW** — return `StrictResult::Incompatible { reason }` immediately; short-circuit ancestors |
| `type` keyword absent | Existing behavior — no normalization, recurse into known sub-schemas |

The `anyOf` transformation preserves sibling keywords on the non-null branch (per Q3b option α). This keeps `description`, `minLength`, `format`, etc. validated against the non-null payload while still permitting null.

---

## 5. API Changes

### 5.1 New enum `StrictResult`

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrictResult {
    /// Schema is fully normalized for OpenAI strict mode.
    Ok,
    /// Schema contains a sub-tree that cannot be expressed in strict mode.
    /// Caller should downgrade the affected tool to `strict: None`.
    Incompatible {
        /// Human-readable diagnostic; goes into `tracing::warn!` for auditability.
        /// Format: `"<json-pointer>: <description>"` (e.g.,
        /// `".properties.actions.items: multi-type schema [boolean,object,...]"`).
        reason: String,
    },
}
```

### 5.2 Updated `normalize_strict_schema` signature

```rust
pub fn normalize_strict_schema(
    schema: &mut Value,
    set_top_level_strict: bool,
) -> StrictResult;
```

Mutation semantics on `Incompatible`: the schema **may be partially mutated** before incompatibility is detected (e.g., outer object had `additionalProperties: false` injected before the recursion hit a multi-type leaf). Callers MUST NOT assume the schema is unchanged on `Incompatible`. The recommended pattern is to clone the original parameters and call again with non-strict treatment (see § 5.3).

### 5.3 Updated `build_tools` consumer

```rust
let strict = if enable_strict {
    match normalize_strict_schema(&mut params, true) {
        StrictResult::Ok => Some(true),
        StrictResult::Incompatible { reason } => {
            tracing::warn!(
                tool_name = %td.name,
                reason = %reason,
                "OpenAI strict mode incompatible — downgrading this tool to non-strict",
            );
            // Reset and apply non-strict normalization instead.
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

The reset path repeats the `$schema`/`title` removal and `ensure_properties_recursive` from the existing non-strict branch — these are idempotent so cost is negligible.

---

## 6. Multi-Type Categorization (`["null", X]` vs. else)

Why exactly 2 elements with 1 null? Three reasons:

1. **Common case**: `Option<T>` is the dominant nullable pattern in Rust + schemars. Some schemars configurations emit `"type": ["null", X]` rather than dropping `null` and relying on the parent `required` array. Handling this preserves strict for the majority of tools.
2. **JSON Schema 2020-12 strict ambiguity**: rewriting `[Y, Z]` (no null) to `anyOf` would technically be valid JSON Schema, but the resulting strict tool can fail at runtime if OpenAI's validator doesn't accept anyOf branches without `additionalProperties` consistency. Bailing is safer.
3. **Catch-all `serde_json::Value` case**: 7-type "any-value" schemas are fundamentally incompatible with strict mode (the "object" branch would need known properties, "array" branch would need items, etc.). Bail is correct.

The rule "exactly length 2 with one null" is precise and reduces the risk surface compared to "any array containing null."

---

## 7. Recursion + Short-Circuit

`normalize_strict_schema` descends into:

- `properties[*]` (when `type == "object"`)
- `items` (when array-typed)
- `anyOf[*]` / `allOf[*]` / `oneOf[*]`
- Within the `anyOf` produced by our own multi-type transformation, the non-null branch

If any recursive call returns `Incompatible`, the outer call propagates it immediately. The first incompatibility wins (we don't accumulate multiple). The `reason` field gets prefixed with the JSON pointer as we unwind.

Example: `desktop.parameters → .properties.actions.items` has the 7-type schema. The reason returned to `build_tools` will be roughly:

```
.properties.actions.items: multi-type schema is not strict-compatible (types: [boolean,object,array,number,string,integer,null])
```

---

## 8. Tests

### 8.1 `openai_strict_schema.rs` mod tests (5 new)

| # | Test | Asserts |
|---|---|---|
| 1 | `multi_type_null_pair_becomes_anyof` | `{"type":["null","string"]}` after normalize → `{"anyOf":[{"type":"null"},{"type":"string"}]}` and returns `StrictResult::Ok` |
| 2 | `null_pair_preserves_sibling_keywords` | `{"type":["null","string"],"description":"x","minLength":1}` → non-null branch contains `description` + `minLength` |
| 3 | `multi_type_any_value_returns_incompatible` | `{"type":["boolean","object","array","number","string","integer","null"]}` returns `Incompatible` with reason mentioning multi-type |
| 4 | `multi_type_non_null_pair_returns_incompatible` | `{"type":["string","number"]}` returns `Incompatible` |
| 5 | `incompatible_short_circuits_through_nesting` | Schema with nested `properties.actions.items` carrying the 7-type any-value returns `Incompatible` and the reason carries `.properties.actions.items` in its path prefix |

### 8.2 `responses/shared.rs` mod tests (2 new)

| # | Test | Asserts |
|---|---|---|
| 6 | `build_tools_downgrades_tool_with_any_value_field` | Given a `ToolDefinition` mimicking the desktop schema (with `actions: serde_json::Value` shape), `build_tools(_, true)` produces `FunctionToolDef { strict: None, ... }` and the `parameters` still validates (no `additionalProperties: false` leftover) |
| 7 | `build_tools_keeps_strict_for_well_typed_tools` | Given a fully-typed `ToolDefinition` (no multi-type fields), `build_tools(_, true)` produces `strict: Some(true)` |

### 8.3 Existing tests preserved

All 8 existing tests in `openai_strict_schema.rs` continue to pass. They use single-type schemas which the new code path leaves untouched. The signature change (`-> ()` to `-> StrictResult`) requires updating each test's call site to either ignore the return or assert `StrictResult::Ok`.

---

## 9. Files Touched

| File | Change |
|---|---|
| `src/providers/protocols/openai_common/openai_strict_schema.rs` | Add `StrictResult` enum; change signature; add multi-type branches; add 5 new tests; update 8 existing tests' call sites |
| `src/providers/responses/shared.rs` | Add `StrictResult` consumer + downgrade branch in `build_tools`; add 2 new tests |
| `CHANGELOG.md` | `[Unreleased]` `### Fixed` entry describing the strict-mode multi-type bug + downgrade behavior |

No other crate files touched. No `Cargo.toml` / `Cargo.lock` change.

---

## 10. Commits

**Commit 1 — `providers/openai: handle multi-type schemas in strict normalizer`**
- Add `StrictResult` enum
- Change `normalize_strict_schema` signature
- Implement `["null", X]` → `anyOf` rewrite (α form)
- Implement multi-type bail with reason string
- Implement short-circuit through nested calls
- Update 8 existing test call sites to assert `StrictResult::Ok`
- Add 5 new mod tests covering null-pair / multi-primitive / non-null-pair / short-circuit

**Commit 2 — `providers/openai: per-tool strict downgrade for incompatible schemas`**
- Update `shared::build_tools` to consume `StrictResult`
- Add downgrade branch with `params.clone()` reset + `ensure_properties_recursive`
- Add `tracing::warn!` audit log
- Add 2 new mod tests
- CHANGELOG `### Fixed` entry

Commits are small (~ 50 / 30 insertions respectively), strictly ordered, and each compiles + passes its own tests independently.

---

## 11. R7 / R10 Compliance

R7 (LLM Sovereignty) and R10 (Thin Harness / Dumb Loop) require zero reasoning logic in protocol code.

- The normalizer is **pure mechanical schema transformation**: dispatch on `type` shape via `match`, no scoring, no LLM, no policy DSL.
- The `build_tools` consumer is a **flat match** on `StrictResult` (2 arms) — no branching beyond Ok/Incompatible.
- `tracing::warn!` is an **audit log only**, not a decision gate.
- Schema mutations are positional and structural (e.g., "if type is `["null", X]`, rewrite to anyOf"), never content-aware.

R7 / R10 PASS by construction.

---

## 12. Risk Register

| # | Risk | Mitigation |
|---|---|---|
| R1 | OpenAI's strict-mode parser rejects `anyOf: [{"type":"null"}, ...]` patterns | Test #1 + #2 are unit-level; e2e (Task 11 in plan) hits T8Star/OpenAI Responses to confirm at runtime |
| R2 | Sibling-keywords preservation breaks for `format` / `pattern` keywords | Test #2 covers `description` + `minLength`; pattern is unlikely on nullable fields in practice |
| R3 | Caller forgets to clone params before retry → strict partial mutation persists in non-strict path | Implementation in §5.3 always clones — this is enforced by code, not convention |
| R4 | Tools with `serde_json::Value` lose strict guarantees (LLM may emit malformed JSON) | Acceptable trade-off — the alternative (failed 400) is worse. `tracing::warn!` tells the user which tools degraded so they can fix sources over time |
| R5 | Future schemars version emits a different multi-type form (e.g., `oneOf` instead of `type` array) | Out of scope; current schemars output is the target. New emission patterns become a follow-up if observed |
| R6 | Downgraded tool's `parameters` still has stale strict transformations from the first pass | Mitigated by `params = td.parameters.clone()` reset in the downgrade branch (§5.3) |

---

## 13. Out of Scope (deferred)

- **D1 source fix**: replacing `actions: Option<Vec<serde_json::Value>>` with a typed `Vec<DesktopBatchAction>` — separate cleanup, not part of Step 3.
- **D3 + D4 (Step 4)**: token-usage observability + migration noise.
- **D5 (Step 6)**: 484 baseline test compile errors.
- **D2 (Step 5)**: hot-reload default-provider routing.
- Per-tool strict mode toggle in `aleph.toml` (e.g., force-disable strict for specific tools). Today's auto-downgrade is enough; manual override is YAGNI.

---

## 14. Verification Strategy

1. `cargo check -p alephcore`: 0 errors after each commit.
2. `cargo clippy -p alephcore --lib --no-deps` on touched files: no new lints.
3. New + existing mod tests pass (assumed; 484 baseline integration errors block `cargo test` but mod tests compile independently).
4. Manual e2e: route a webchat conversation through `T8Star` (OpenAI Responses protocol, `default_provider = T8Star` or via UI), confirm:
   - Server log contains `tracing::warn!` "OpenAI strict mode incompatible — downgrading this tool to non-strict" for `desktop`
   - No `400 Bad Request` schema errors
   - Tool calls (if model invokes `desktop`) succeed end-to-end

---

## 15. Acceptance Criteria

- [ ] `normalize_strict_schema` returns `StrictResult` per § 5.2
- [ ] `["null", X]` → `anyOf` α form with sibling-keyword preservation
- [ ] Multi-type other shapes → `Incompatible` with JSON-pointer-prefixed reason
- [ ] `build_tools` downgrades `strict` to `None` per-tool on `Incompatible` and emits `tracing::warn!`
- [ ] 7 new mod tests pass (5 normalizer + 2 build_tools)
- [ ] 8 existing normalizer tests continue to pass after call-site update
- [ ] `cargo check -p alephcore` clean, 0 errors, no new clippy lints on touched files
- [ ] CHANGELOG `### Fixed` entry references this spec and the desktop reproducer
- [ ] Manual e2e: webchat through `openai_responses` provider does not 400 on `desktop` tool schema; `tracing::warn!` audit log fires once per request listing degraded tools

---

## 16. Predecessor + Step Sequence Context

Step 3 is the first follow-up after Step 2 e2e (`6edf18f73`). Issue catalog and Step ordering recorded in `~/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/project_step3_plus_followups.md`. Recommended sequence: **Step 3 (this) → Step 4 (observability + migration noise) → Step 6 (baseline test repair) → Step 5 (hot-reload routing)**.
