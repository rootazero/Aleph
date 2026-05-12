# OpenAI Protocol Cycle 2 — Output Format & Completion-Tokens Wiring Design

**Date:** 2026-05-12
**Status:** Approved
**Cycle:** 2 of the OpenAI protocol provider-side optimization series
**Predecessor:** [`2026-05-12-openai-protocol-token-and-events-wiring-design.md`](./2026-05-12-openai-protocol-token-and-events-wiring-design.md) (Cycle 1, shipped)

---

## 1. Scope & Goals

Wire three feature gaps in Aleph's OpenAI Chat and Responses protocol adapters. These are surgical additions / fixes — no new abstractions, no trait changes, no destructive refactors. The infrastructure already exists for all three; this cycle just connects the wires.

### Three-Bundle Manifest

| Bundle | Feature | Class | Files touched |
|---|---|---|---|
| **C2-A** | `response_format` (Text / JsonObject / JsonSchema) | New capability | `src/config/types/provider.rs`, `src/providers/protocols/openai_common/response_format.rs` (new), `src/providers/protocols/openai_chat/adapter.rs`, `src/providers/protocols/openai_responses/mod.rs`, `src/providers/protocols/openai_common/provider_policy.rs` |
| **C2-B** | `max_completion_tokens` Chat field-name swap for reasoning models | Bug fix | `src/providers/protocols/openai_common/max_tokens.rs` (new), `src/providers/protocols/openai_chat/adapter.rs` |
| **C2-C** | `parallel_tool_calls` unhardcode | R10 cleanup | `src/config/types/provider.rs`, `src/providers/protocols/openai_chat/adapter.rs`, `src/providers/protocols/openai_responses/mod.rs` |

### Non-Goals

- `seed`, `logprobs`, `top_logprobs` — deferred to Cycle 3.
- Anthropic protocol changes.
- Per-request payload override paths for the three new fields — config-only.
- New trait extensions or breaking signature changes.

### Architectural Alignment

- **R7 LLM Sovereignty**: config drives wire shape; no LLM-replacement logic in code paths.
- **R10 Thin Harness**: Cycle 2 removes a hardcoded `parallel_tool_calls: Some(true)` smell at `openai_responses/mod.rs:172` — decisions live in config, not in adapter literals.
- **P1/P2 Coupling/Cohesion**: shared wire-shape translators live in `openai_common/`; per-protocol adapters call them.
- **DRY**: Chat and Responses both consume the same `ResponseFormat` enum and the same `uses_max_completion_tokens(model)` helper.

---

## 2. Type Design

### 2.1 `ResponseFormat` Enum (New)

Added to `src/config/types/provider.rs` alongside `CacheRetention` — it is a config-level enum that describes a wire-shape decision.

```rust
/// Structured output format for the model response.
///
/// Maps to:
/// - Chat protocol → top-level `response_format` field
/// - Responses protocol → `text.format` field (already typed as `TextFormat`)
///
/// Capability-gated: silently dropped when endpoint doesn't support it
/// (see `ProviderCapabilities::supports_response_format`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    /// Free-form text (default; equivalent to no field). Included for explicitness.
    Text,
    /// Force valid JSON output (no schema).
    JsonObject,
    /// Force JSON matching schema (strict mode when endpoint supports).
    JsonSchema {
        name: String,
        schema: serde_json::Value,
    },
}
```

### 2.2 `ProviderConfig` New Fields

Two new optional fields, no breaking change to existing config files (both `#[serde(default)]`):

```rust
/// Structured output format (None = free-form text)
#[serde(default)]
pub response_format: Option<ResponseFormat>,

/// Whether the model accepts parallel tool calls (None = server default)
#[serde(default)]
pub parallel_tool_calls: Option<bool>,
```

`max_completion_tokens` does **not** get a new config field — it reuses the existing `max_tokens: Option<u32>` field, only the Chat wire name changes when the model is a reasoning family.

### 2.3 `ProviderCapabilities` Extension

`src/providers/protocols/openai_common/provider_policy.rs` `ProviderCapabilities` gains:

```rust
/// Supports the `response_format` (Chat) / `text.format` (Responses) field
pub supports_response_format: bool,
```

Per-`EndpointClass` mapping (13 variants total in `provider_policy.rs`):

| EndpointClass | supports_response_format |
|---|---|
| `OpenAiPublic` | **true** |
| `OpenAiCodex` | **true** |
| `AzureOpenAi`, `AnthropicPublic`, `DeepSeekNative`, `GroqNative`, `MistralPublic`, `MoonshotNative`, `CerebrasNative`, `XAiNative`, `OpenRouter`, `Local`, `Custom` (11) | false (conservative; opt-in later by per-class flip in Cycle 3) |

`build_payload_policy` `strip` list gains `"response_format"` when `!supports_response_format` — defense-in-depth tier so a forgotten adapter check still scrubs the field.

### 2.4 Shared Wire-Shape Translators (New)

**File 1**: `src/providers/protocols/openai_common/response_format.rs` (~60 lines)

```rust
use crate::config::ResponseFormat;
use crate::providers::responses::types::{TextConfig, TextFormat};
use serde_json::{json, Value};

/// Build the Chat protocol's `response_format` JSON value.
/// Returns None when `Text` (omit field).
pub fn to_chat_response_format(
    fmt: &ResponseFormat,
    supports_strict: bool,
) -> Option<Value> {
    match fmt {
        ResponseFormat::Text => None,
        ResponseFormat::JsonObject => Some(json!({"type": "json_object"})),
        ResponseFormat::JsonSchema { name, schema } => {
            let mut inner = json!({
                "name": name,
                "schema": schema,
            });
            if supports_strict {
                inner["strict"] = json!(true);
            }
            Some(json!({
                "type": "json_schema",
                "json_schema": inner,
            }))
        }
    }
}

/// Build the Responses protocol's `text.format` typed value.
/// Returns None when `Text` (omit field).
pub fn to_responses_text_format(fmt: &ResponseFormat) -> Option<TextFormat> {
    match fmt {
        ResponseFormat::Text => None,
        ResponseFormat::JsonObject => Some(TextFormat::JsonObject),
        ResponseFormat::JsonSchema { name, schema } => Some(TextFormat::JsonSchema {
            name: name.clone(),
            schema: schema.clone(),
        }),
    }
}

/// Merge an explicit response_format into the variant's existing TextConfig.
/// Preserves variant's `verbosity`, overrides `format`.
pub fn merge_text_format(
    base: Option<TextConfig>,
    fmt: Option<&ResponseFormat>,
) -> Option<TextConfig> {
    match (base, fmt) {
        (existing, None) => existing,
        (Some(mut t), Some(f)) => {
            if let Some(rf) = to_responses_text_format(f) {
                t.format = Some(rf);
            }
            Some(t)
        }
        (None, Some(f)) => to_responses_text_format(f).map(|rf| TextConfig {
            format: Some(rf),
            verbosity: None,
        }),
    }
}
```

**File 2**: `src/providers/protocols/openai_common/max_tokens.rs` (~30 lines)

```rust
/// Returns true if the model requires `max_completion_tokens` (Chat protocol)
/// instead of the legacy `max_tokens` field.
///
/// Reasoning model families that reject `max_tokens` with HTTP 400:
/// - o1- (o1-mini, o1-preview, ...)
/// - o3- (o3-mini, o3-pro, ...)
/// - o4- (o4-mini, ...)
/// - gpt-5 (gpt-5.x family)
///
/// All other models (gpt-4o, gpt-4-turbo, gpt-3.5-turbo, third-party compat
/// backends, …) continue to use `max_tokens`.
///
/// Responses protocol uses `max_output_tokens` and is unaffected.
pub fn uses_max_completion_tokens(model: &str) -> bool {
    let m = model.trim();
    m.starts_with("o1-")
        || m.starts_with("o3-")
        || m.starts_with("o4-")
        || m.starts_with("gpt-5")
}
```

Both files re-exported from `src/providers/protocols/openai_common/mod.rs` for downstream use.

---

## 3. Chat Adapter Wiring

`src/providers/protocols/openai_chat/adapter.rs::build_request`:

### 3.1 max_tokens Field-Name Swap (replace lines ~42–44)

```rust
if let Some(max_tokens) = payload.max_tokens.or(config.max_tokens) {
    let field = if uses_max_completion_tokens(model_name) {
        "max_completion_tokens"
    } else {
        "max_tokens"
    };
    body[field] = json!(max_tokens);
}
```

Where `model_name` is the resolved model used elsewhere in the function: `payload.model.as_deref().unwrap_or_else(|| config.default_model())`. Hoist into a local let-binding at the top of `build_request` so it can be reused (currently computed inline in `body` construction and in `tracing::debug!`).

### 3.2 response_format Injection (after policy construction, before tools block)

```rust
if let Some(ref fmt) = config.response_format {
    if policy.capabilities.supports_response_format {
        if let Some(v) = to_chat_response_format(
            fmt,
            policy.capabilities.supports_strict_schema,
        ) {
            body["response_format"] = v;
        }
    }
}
```

### 3.3 parallel_tool_calls Injection (adjacent to tool_choice handling, lines ~115–124)

```rust
if let Some(parallel) = config.parallel_tool_calls {
    body["parallel_tool_calls"] = json!(parallel);
}
```

`policy.apply(obj)` (line 111) already strips unsupported fields per the endpoint's strip list — this is the safety net.

---

## 4. Responses Adapter Wiring

`src/providers/protocols/openai_responses/mod.rs::build_responses_request`:

### 4.1 parallel_tool_calls — Unhardcode (line ~172)

```rust
// OLD: parallel_tool_calls: Some(true),
parallel_tool_calls: config.parallel_tool_calls,
```

### 4.2 text Field Fusion (replace `text: variant.text.clone()`)

```rust
text: merge_text_format(variant.text.clone(), config.response_format.as_ref()),
```

`merge_text_format` lives in `openai_common/response_format.rs` (see §2.4). It preserves the variant's `verbosity` slot while overriding the `format` slot only when config specifies a response_format.

### 4.3 max_completion_tokens — Not Applicable

Responses protocol uses `max_output_tokens`, which is already correctly wired (`max_output_tokens: payload.max_tokens` at the existing struct construction site). No change required.

---

## 5. Capability Policy Updates

`src/providers/protocols/openai_common/provider_policy.rs`:

### 5.1 Add Field to `ProviderCapabilities`

```rust
pub struct ProviderCapabilities {
    // ... existing fields ...
    /// Supports the `response_format` (Chat) / `text.format` (Responses) field
    pub supports_response_format: bool,
}
```

### 5.2 Initialize in All 13 `EndpointClass` Branches

In `resolve_capabilities`:

```rust
EndpointClass::OpenAiPublic => ProviderCapabilities {
    // ... existing fields ...
    supports_response_format: true,
},
EndpointClass::OpenAiCodex => ProviderCapabilities {
    // ... existing fields ...
    supports_response_format: true,
},
// All other 11 variants (AzureOpenAi, AnthropicPublic, DeepSeekNative,
// GroqNative, MistralPublic, MoonshotNative, CerebrasNative, XAiNative,
// OpenRouter, Local, Custom):
//   ProviderCapabilities { ..., supports_response_format: false, }
```

### 5.3 Strip-List Defense

In `build_payload_policy`:

```rust
if !capabilities.supports_response_format {
    payload.remove("response_format");
}
```

Placed alongside the existing `reasoning_effort` strip (current line 104).

---

## 6. Testing Strategy

All tests follow the Cycle 1 pattern: shared helper functions in `tests` modules; no new dev-dependencies (no `rstest`). All `use` imports at file top. Total: **18 new tests** (10 + 4 + 4).

### 6.1 C2-A response_format Tests (10 new)

**Unit (`openai_common/response_format.rs` tests submodule):**

1. `to_chat_response_format_text_returns_none` — variant `Text` → `None`
2. `to_chat_response_format_json_object_emits_type_field` — `JsonObject` → `{"type":"json_object"}`
3. `to_chat_response_format_json_schema_strict_includes_strict_true` — `JsonSchema{...}` with `supports_strict=true` → output has `"strict": true`
4. `to_chat_response_format_json_schema_no_strict_omits_strict` — same with `supports_strict=false` → no `strict` key
5. `to_responses_text_format_json_object` — `JsonObject` → `TextFormat::JsonObject`
6. `to_responses_text_format_json_schema_preserves_name_and_schema` — `JsonSchema{...}` → `TextFormat::JsonSchema{...}` with identical fields

**Chat adapter integration (`openai_chat/tests.rs`):**

7. `chat_response_format_when_capability_enabled` — `config.response_format=Some(JsonSchema{...})`, endpoint OpenAiPublic → body contains `response_format` with strict=true
8. `chat_response_format_stripped_when_capability_disabled` — same config, endpoint set to a third-party class → body has no `response_format` key

**Responses adapter integration (`openai_responses/tests.rs`):**

9. `responses_text_merges_format_into_existing_variant` — variant.text has `verbosity="medium"`, config sets `JsonObject` → result has both `verbosity="medium"` and `format=JsonObject`
10. `responses_text_passes_through_when_no_response_format` — config.response_format=None → text identical to variant.text

### 6.2 C2-B max_completion_tokens Tests (4 new)

**Unit (`openai_common/max_tokens.rs` tests submodule):**

1. `uses_max_completion_tokens_table` — helper-fn covers 8 cases: `gpt-4o` / `gpt-4-turbo` / `gpt-3.5-turbo` → false; `o1-mini` / `o3-pro` / `o4-mini` / `gpt-5.4` / `gpt-5` → true

**Chat adapter integration (`openai_chat/tests.rs`):**

2. `chat_uses_max_completion_tokens_for_o3` — `models=["o3-mini"]`, `max_tokens=Some(4096)` → body has `max_completion_tokens=4096`, no `max_tokens` key
3. `chat_uses_max_tokens_for_gpt4o` — `models=["gpt-4o"]`, `max_tokens=Some(4096)` → body has `max_tokens=4096`, no `max_completion_tokens` key
4. `chat_payload_model_overrides_config_for_field_swap` — `config.default_model="gpt-4o"`, `payload.model=Some("o3-mini")` → uses `max_completion_tokens` (payload wins)

### 6.3 C2-C parallel_tool_calls Tests (4 new)

**Chat adapter (`openai_chat/tests.rs`):**

1. `chat_parallel_tool_calls_some_true` — `config.parallel_tool_calls=Some(true)` → body has `parallel_tool_calls: true`
2. `chat_parallel_tool_calls_some_false` — same with `Some(false)` → body has `parallel_tool_calls: false`
3. `chat_parallel_tool_calls_none_omits_field` — `None` → body has no `parallel_tool_calls` key

**Responses adapter (`openai_responses/tests.rs`):**

4. `responses_parallel_tool_calls_respects_config` — `config.parallel_tool_calls=None` → built `ResponsesRequest.parallel_tool_calls` is `None` (verifies the hardcoded `Some(true)` is gone)

### 6.4 Regression Coverage

- All existing 42 Chat + 50 Responses tests pass.
- `cargo test -p alephcore --lib openai_chat openai_responses openai_common` is green.
- `cargo clippy -p alephcore -- -D warnings` produces zero new warnings vs Cycle 1 baseline.

---

## 7. Backward Compatibility

| Change | User-visible effect | Mitigation |
|---|---|---|
| `parallel_tool_calls` no longer hardcoded to `true` on Responses | OpenAI public endpoint server default is `true` → **identical wire behavior**. Third-party Responses-compat backends now receive `None` (server-side default applies). | Documented in CHANGELOG `[Changed]`. No config migration needed. |
| `max_completion_tokens` auto-swap for o1/o3/o4/gpt-5 | Previously: reasoning models returned HTTP 400 for any Aleph user with those models configured. After: works automatically. **Bug fix, non-breaking.** | CHANGELOG `[Fixed]`. |
| `response_format: Option<ResponseFormat>` new field | Defaults to `None` → existing users see **zero change**. New capability for users who want structured output. | CHANGELOG `[Added]`. Schema for UI panel auto-updates via schemars. |
| `parallel_tool_calls: Option<bool>` new field | Defaults to `None` → zero change. | CHANGELOG `[Added]`. |
| `ResponseFormat` enum in schemars JSON Schema | UI settings panel sees a new optional control. | Non-breaking; additive only. |

**Config file migration path**: All new fields are `#[serde(default)]`. Existing `~/.aleph/config.toml` files load unchanged. No data migration.

---

## 8. Acceptance Criteria

| # | Criterion | Verification |
|---|---|---|
| AC-1 | All existing tests pass (no regressions in 42 Chat + 50 Responses + others) | `cargo test -p alephcore --lib` green |
| AC-2 | All 18 new Cycle 2 tests pass | `cargo test -p alephcore --lib response_format max_tokens parallel_tool_calls` green |
| AC-3 | `cargo clippy -p alephcore -- -D warnings` produces zero new warnings vs Cycle 1 baseline | Diff clippy output against the Cycle 1 commit `ea0b2a3f3` baseline |
| AC-4 | Manual smoke: configure `models=["o3-mini"]` on an OpenAI public-endpoint provider with `max_tokens=1024`; request a Chat completion → succeeds (no HTTP 400) | Manual run; deferred from blocking gate |
| AC-5 | Manual smoke: configure `parallel_tool_calls=Some(false)` on a Responses provider; observe tool calls run serially in a multi-tool turn | Manual run; deferred from blocking gate |
| AC-6 | CHANGELOG.md `[Unreleased]` updated with 1 Added (response_format), 1 Fixed (max_completion_tokens), 1 Changed (parallel_tool_calls) | `git diff CHANGELOG.md` |

AC-4 and AC-5 are non-blocking for spec acceptance — code review and unit-test acceptance (AC-1, AC-2, AC-3, AC-6) gate the merge. Manual smokes can land in a follow-up note.

---

## 9. File-Touch Summary

**Created (3 files):**
- `src/providers/protocols/openai_common/response_format.rs` (~60 lines)
- `src/providers/protocols/openai_common/max_tokens.rs` (~30 lines)
- (No new test files — tests added to existing `openai_chat/tests.rs`, `openai_responses/tests.rs`, and new `#[cfg(test)] mod tests` in the two new files.)

**Modified (5 files):**
- `src/config/types/provider.rs` — add `ResponseFormat` enum + 2 fields in `ProviderConfig` + test_config initializers
- `src/providers/protocols/openai_common/mod.rs` — `pub mod response_format; pub mod max_tokens;` re-exports
- `src/providers/protocols/openai_common/provider_policy.rs` — add `supports_response_format`, set per-EndpointClass, add strip-list entry
- `src/providers/protocols/openai_chat/adapter.rs` — three wiring blocks
- `src/providers/protocols/openai_responses/mod.rs` — two wiring changes (parallel_tool_calls, text fusion)

**Documentation (1 file):**
- `CHANGELOG.md` — three `[Unreleased]` entries

**Expected diff size**: ~400–500 lines (incl. tests).

---

## 10. Out-of-Scope (Cycle 3 Candidates)

- **`seed`** — `Option<u64>` for determinism; trivial wiring, both protocols accept it.
- **`logprobs` / `top_logprobs`** — Chat accepts these at top-level; Responses surfaces logprobs via `include: ["logprobs"]`. Per-protocol shape difference makes this its own bundle.
- **Strict-schema normalization for response_format JSON Schema** — reuse `openai_strict_schema::normalize_strict_schema` for the `JsonSchema { schema, … }` payload if discovered compatibility issues surface.
- **Anthropic protocol parity** — separate cycle.
