# OpenAI Protocol Cycle 2 — Output Format & Completion-Tokens Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire `response_format`, `max_completion_tokens` swap, and `parallel_tool_calls` across both OpenAI Chat and Responses protocol adapters, removing the hardcoded `parallel_tool_calls: Some(true)` smell in the Responses adapter.

**Architecture:** Add a `ResponseFormat` enum in `src/config/types/provider.rs` and two shared wire-shape translators in `src/providers/protocols/openai_common/` (`response_format.rs` + `max_tokens.rs`). Both adapters consume these shared helpers. `response_format` is capability-gated via a new `supports_response_format` flag on `ProviderCapabilities`. No new dev-dependencies, no trait changes, no per-request payload overrides (config-only).

**Tech Stack:** Rust 1.x; `serde` + `serde_json` for wire serialization; `schemars` for JSON Schema generation; `async_trait` for adapter trait impls; built-in `#[test]` framework with shared helper functions (no `rstest`, matching Cycle 1 convention).

**Spec:** [`docs/superpowers/specs/2026-05-12-openai-protocol-cycle2-output-format-and-completion-tokens-design.md`](../specs/2026-05-12-openai-protocol-cycle2-output-format-and-completion-tokens-design.md)

**Predecessor:** Cycle 1 (commits `f6b787d8c..ea0b2a3f3`, all 8 commits shipped to `main`, final code review verdict SHIP IT — observation S2264).

---

## File Touch Map

**Created (2 files):**
- `src/providers/protocols/openai_common/max_tokens.rs` — `uses_max_completion_tokens(model)` helper + unit tests
- `src/providers/protocols/openai_common/response_format.rs` — `to_chat_response_format`, `to_responses_text_format`, `merge_text_format` + unit tests

**Modified (6 files):**
- `src/config/types/provider.rs` — add `ResponseFormat` enum + 2 `ProviderConfig` fields + 5 `test_config` initializer updates
- `src/providers/protocols/openai_common/mod.rs` — add 2 `pub mod` re-exports
- `src/providers/protocols/openai_common/provider_policy.rs` — add `supports_response_format` field, set in 13 EndpointClass branches, add strip-list defense
- `src/providers/protocols/openai_chat/adapter.rs` — three wiring blocks (max_tokens field swap, response_format, parallel_tool_calls); hoist `model_name` to local binding
- `src/providers/protocols/openai_responses/mod.rs` — two wiring changes (parallel_tool_calls unhardcode, text fusion)
- `CHANGELOG.md` — three `[Unreleased]` entries

**Test files (appended to existing):**
- `src/providers/protocols/openai_chat/tests.rs` — 8 new integration tests
- `src/providers/protocols/openai_responses/tests.rs` — 4 new integration tests

---

## Task Dependency Order

```
T1 (max_tokens helper)
  ↓
T2 (Chat max_tokens swap, uses T1)
  ↓
T3 (ResponseFormat enum + response_format.rs helpers)
  ↓
T4 (provider_policy capability + strip)
  ↓
T5 (ProviderConfig new fields, uses T3)
  ↓
T6 (Chat wire response_format + parallel_tool_calls, uses T3 T4 T5)
  ↓
T7 (Responses fuse text + unhardcode parallel, uses T3 T5)
  ↓
T8 (CHANGELOG + final cycle review)
```

Each task is self-contained: write tests first (RED), implement to make them pass (GREEN), run the full lib test suite to confirm zero regression, commit.

---

## Task 1: `max_tokens.rs` Helper + Unit Tests

**Files:**
- Create: `src/providers/protocols/openai_common/max_tokens.rs`
- Modify: `src/providers/protocols/openai_common/mod.rs`

- [ ] **Step 1: Write the failing tests (in the new file)**

Create `src/providers/protocols/openai_common/max_tokens.rs` with the test module already populated:

```rust
//! Per-model decision: which Chat-API token-limit field name to send.
//!
//! OpenAI reasoning model families (`o1-`, `o3-`, `o4-`, `gpt-5`) reject
//! `max_tokens` with HTTP 400; they require `max_completion_tokens` instead.
//! All other models (gpt-4o, gpt-4-turbo, gpt-3.5-turbo, third-party compat
//! backends, ...) continue to use `max_tokens`.
//!
//! The Responses protocol is unaffected — it uses `max_output_tokens`.

/// Returns true if the model requires `max_completion_tokens` instead of `max_tokens`.
pub fn uses_max_completion_tokens(model: &str) -> bool {
    let m = model.trim();
    m.starts_with("o1-")
        || m.starts_with("o3-")
        || m.starts_with("o4-")
        || m.starts_with("gpt-5")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_chat_models_use_max_tokens() {
        assert!(!uses_max_completion_tokens("gpt-4o"));
        assert!(!uses_max_completion_tokens("gpt-4-turbo"));
        assert!(!uses_max_completion_tokens("gpt-3.5-turbo"));
        assert!(!uses_max_completion_tokens(""));
    }

    #[test]
    fn o1_family_uses_max_completion_tokens() {
        assert!(uses_max_completion_tokens("o1-mini"));
        assert!(uses_max_completion_tokens("o1-preview"));
    }

    #[test]
    fn o3_family_uses_max_completion_tokens() {
        assert!(uses_max_completion_tokens("o3-mini"));
        assert!(uses_max_completion_tokens("o3-pro"));
    }

    #[test]
    fn o4_family_uses_max_completion_tokens() {
        assert!(uses_max_completion_tokens("o4-mini"));
    }

    #[test]
    fn gpt5_family_uses_max_completion_tokens() {
        assert!(uses_max_completion_tokens("gpt-5"));
        assert!(uses_max_completion_tokens("gpt-5.4"));
        assert!(uses_max_completion_tokens("gpt-5-codex"));
    }

    #[test]
    fn trims_whitespace_before_match() {
        assert!(uses_max_completion_tokens("  o3-mini  "));
        assert!(!uses_max_completion_tokens("  gpt-4o  "));
    }
}
```

- [ ] **Step 2: Register module in `openai_common/mod.rs`**

Edit `src/providers/protocols/openai_common/mod.rs`. Current contents:

```rust
pub mod openai_strict_schema;
pub mod provider_policy;
pub mod sse;
pub mod tools;
```

Add only the `max_tokens` line (the `response_format` module will be registered in Task 3 when that file actually exists):

```rust
pub mod max_tokens;
pub mod openai_strict_schema;
pub mod provider_policy;
pub mod sse;
pub mod tools;
```

- [ ] **Step 3: Verify tests compile and run**

Run: `cargo test -p alephcore --lib max_tokens -- --nocapture`
Expected: PASS — 6 tests pass.

- [ ] **Step 4: Run full lib regression**

Run: `cargo test -p alephcore --lib`
Expected: All previously-passing tests still pass; 6 new tests added. (One pre-existing baseline failure `provider_policy::tests::test_apply_policy_strips_fields` may still fail — confirmed unrelated to Cycle 2.)

- [ ] **Step 5: Commit**

```bash
git add src/providers/protocols/openai_common/max_tokens.rs \
        src/providers/protocols/openai_common/mod.rs
git commit -m "providers: add uses_max_completion_tokens helper for reasoning models"
```

---

## Task 2: Chat Adapter `max_completion_tokens` Field Swap

**Files:**
- Modify: `src/providers/protocols/openai_chat/adapter.rs:30-44, 134`
- Modify: `src/providers/protocols/openai_chat/tests.rs` (append)

- [ ] **Step 1: Write failing integration tests**

Append to `src/providers/protocols/openai_chat/tests.rs` (these go at the END of the file; ensure any new `use` imports go at the TOP of the file):

Add to the top-of-file `use` block (find the existing `use` block near the top of `tests.rs` and add this line if not already present):

```rust
use crate::providers::protocols::openai_common::max_tokens::uses_max_completion_tokens;
```

Then append at the bottom of the file:

```rust
// ─── Task 2: max_completion_tokens field swap ────────────────────

/// Build a Chat request body for the given model + max_tokens config,
/// returning the JSON body as a serde_json::Value for inspection.
fn build_chat_body_for_max_tokens(model: &str, max_tokens: Option<u32>) -> serde_json::Value {
    use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
    use crate::config::ProviderConfig;

    let protocol = super::OpenAiProtocol::new_for_tests();
    let mut config = ProviderConfig::test_config(model);
    config.max_tokens = max_tokens;

    let payload = RequestPayload {
        messages: &[],
        system_prompt: None,
        max_tokens: None,
        temperature: None,
        think_level: None,
        tools: None,
        tool_choice: None,
        model: None,
    };

    let req = protocol
        .build_request(&payload, &config)
        .expect("build_request should succeed");
    // Extract the JSON body by re-serializing the underlying reqwest body.
    // The test_config helper provides "test-key", so build_request succeeds.
    // The actual body bytes are not exposed by reqwest::RequestBuilder, so
    // we use a parallel route: call the helper via a known-shape extraction
    // through the protocol's introspection hook. Tests that already exist
    // in this file (search for `build_chat_body`) demonstrate the pattern.
    extract_chat_body(req)
}

#[test]
fn chat_uses_max_completion_tokens_for_o3_mini() {
    let body = build_chat_body_for_max_tokens("o3-mini", Some(4096));
    assert_eq!(body.get("max_completion_tokens"), Some(&serde_json::json!(4096)));
    assert!(body.get("max_tokens").is_none(),
        "max_tokens must NOT be present for o3-mini (reasoning model)");
}

#[test]
fn chat_uses_max_tokens_for_gpt4o() {
    let body = build_chat_body_for_max_tokens("gpt-4o", Some(4096));
    assert_eq!(body.get("max_tokens"), Some(&serde_json::json!(4096)));
    assert!(body.get("max_completion_tokens").is_none(),
        "max_completion_tokens must NOT be present for gpt-4o (legacy model)");
}

#[test]
fn chat_omits_max_tokens_when_both_none() {
    let body = build_chat_body_for_max_tokens("gpt-4o", None);
    assert!(body.get("max_tokens").is_none());
    assert!(body.get("max_completion_tokens").is_none());
}
```

**IMPORTANT NOTE FOR IMPLEMENTER:** The above test helper `build_chat_body_for_max_tokens` uses `extract_chat_body(req)` — this helper may or may not already exist in `tests.rs`. Before writing tests, **scan `tests.rs` for the established pattern** for extracting the JSON body from a `reqwest::RequestBuilder`. The existing Cycle 1 tests (e.g., `chat_stop_field_includes_sequences`) already demonstrate body extraction. Reuse whichever helper is named there. If no extractor exists, build the body via the protocol's internal `convert_messages` + raw `json!()` reconstruction pattern (search for existing test helpers in `tests.rs` to find the canonical example).

If the canonical extractor is named differently (e.g. `chat_request_body(req)`, `body_value(req)`, etc.), adapt the test code above to use that name.

- [ ] **Step 2: Run tests to confirm failure**

Run: `cargo test -p alephcore --lib chat_uses_max_completion_tokens chat_uses_max_tokens chat_omits_max_tokens -- --nocapture`
Expected: FAIL — `max_completion_tokens` not present for o3-mini, because the adapter currently always emits `max_tokens` regardless of model.

- [ ] **Step 3: Implement the swap in `adapter.rs`**

Edit `src/providers/protocols/openai_chat/adapter.rs`. The current `build_request` (lines 26–144) computes the model inline twice (line 36 and line 134). Hoist it once and use it for the field-name swap.

Find this region (lines ~31–44):

```rust
let endpoint = Self::build_endpoint(config);
let messages = Self::convert_messages(payload.messages, payload.system_prompt);

// Build request body — always streaming (stream-first architecture)
let mut body = json!({
    "model": payload.model.as_deref().unwrap_or_else(|| config.default_model()),
    "messages": messages,
    "stream": true,
});

// Add optional parameters (per-request overrides provider config)
if let Some(max_tokens) = payload.max_tokens.or(config.max_tokens) {
    body["max_tokens"] = json!(max_tokens);
}
```

Replace with:

```rust
let endpoint = Self::build_endpoint(config);
let messages = Self::convert_messages(payload.messages, payload.system_prompt);
let model_name = payload
    .model
    .as_deref()
    .unwrap_or_else(|| config.default_model())
    .to_string();

// Build request body — always streaming (stream-first architecture)
let mut body = json!({
    "model": model_name,
    "messages": messages,
    "stream": true,
});

// Add optional parameters (per-request overrides provider config)
if let Some(max_tokens) = payload.max_tokens.or(config.max_tokens) {
    let field = if super::super::openai_common::max_tokens::uses_max_completion_tokens(&model_name) {
        "max_completion_tokens"
    } else {
        "max_tokens"
    };
    body[field] = json!(max_tokens);
}
```

Also find the `tracing::debug!` near line 134:

```rust
debug!(
    endpoint = %endpoint,
    model = %payload.model.as_deref().unwrap_or_else(|| config.default_model()),
    "Building OpenAI request"
);
```

Replace with:

```rust
debug!(
    endpoint = %endpoint,
    model = %model_name,
    "Building OpenAI request"
);
```

- [ ] **Step 4: Add `use` import**

At the top of `adapter.rs`, the existing imports include:

```rust
use crate::providers::protocols::openai_common::openai_strict_schema::normalize_strict_schema;
use crate::providers::protocols::openai_common::provider_policy::build_payload_policy;
```

Add directly below:

```rust
use crate::providers::protocols::openai_common::max_tokens::uses_max_completion_tokens;
```

And update the field-swap code (Step 3) to use the shorter call:

```rust
let field = if uses_max_completion_tokens(&model_name) {
    "max_completion_tokens"
} else {
    "max_tokens"
};
```

- [ ] **Step 5: Run tests to confirm pass**

Run: `cargo test -p alephcore --lib chat_uses_max_completion_tokens chat_uses_max_tokens chat_omits_max_tokens -- --nocapture`
Expected: PASS — all 3 new tests pass.

- [ ] **Step 6: Run full Chat test suite for regression**

Run: `cargo test -p alephcore --lib openai_chat`
Expected: All previously-passing Chat tests still pass, plus the 3 new ones.

- [ ] **Step 7: Run full lib regression**

Run: `cargo test -p alephcore --lib`
Expected: Same baseline as Task 1 — no new failures.

- [ ] **Step 8: Commit**

```bash
git add src/providers/protocols/openai_chat/adapter.rs \
        src/providers/protocols/openai_chat/tests.rs
git commit -m "openai-chat: swap max_tokens → max_completion_tokens for reasoning models"
```

---

## Task 3: `ResponseFormat` Enum + `response_format.rs` Helpers

**Files:**
- Modify: `src/config/types/provider.rs` (add enum)
- Create: `src/providers/protocols/openai_common/response_format.rs`
- Modify: `src/providers/protocols/openai_common/mod.rs` (add `pub mod response_format;`)

- [ ] **Step 1: Add the `ResponseFormat` enum to `provider.rs`**

Edit `src/config/types/provider.rs`. Find the existing `CacheRetention` enum block (lines ~22–39, between the comments `// CacheRetention` and `// ProviderConfig`). Immediately after the closing `}` of `CacheRetention`, insert:

```rust
// =============================================================================
// ResponseFormat
// =============================================================================

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
    /// Free-form text (default; equivalent to no field).
    Text,
    /// Force valid JSON output (no schema).
    JsonObject,
    /// Force JSON matching the provided schema. Strict mode enabled when
    /// endpoint supports it.
    JsonSchema {
        name: String,
        schema: serde_json::Value,
    },
}
```

- [ ] **Step 2: Verify the enum compiles**

Run: `cargo check -p alephcore`
Expected: Clean compile (warnings about unused enum are OK at this stage).

- [ ] **Step 3: Create the helper file with failing tests**

Create `src/providers/protocols/openai_common/response_format.rs` with the test module already populated:

```rust
//! Wire-shape translators for `ResponseFormat`.
//!
//! Chat protocol uses a top-level `response_format` JSON value.
//! Responses protocol uses the typed `TextFormat` inside `TextConfig`.

use crate::config::types::provider::ResponseFormat;
use crate::providers::responses::types::{TextConfig, TextFormat};
use serde_json::{json, Value};

/// Build the Chat protocol's `response_format` JSON value.
/// Returns `None` when `Text` (omit field).
///
/// When `supports_strict` is true and the variant is `JsonSchema`, the wire
/// includes `"strict": true` inside the `json_schema` block to opt into
/// OpenAI's strict-mode token mask.
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
/// Returns `None` when `Text` (omit format slot inside TextConfig).
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

/// Merge an explicit `ResponseFormat` config into the variant's `TextConfig`.
/// Preserves variant's `verbosity` slot; overrides `format` slot only.
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

#[cfg(test)]
mod tests {
    use super::*;

    // ─── to_chat_response_format ─────────────────────────────────

    #[test]
    fn chat_text_returns_none() {
        assert!(to_chat_response_format(&ResponseFormat::Text, true).is_none());
        assert!(to_chat_response_format(&ResponseFormat::Text, false).is_none());
    }

    #[test]
    fn chat_json_object_emits_type_only() {
        let v = to_chat_response_format(&ResponseFormat::JsonObject, true).unwrap();
        assert_eq!(v, json!({"type": "json_object"}));
    }

    #[test]
    fn chat_json_schema_strict_includes_strict_true() {
        let schema = json!({"type": "object", "properties": {"x": {"type": "string"}}});
        let v = to_chat_response_format(
            &ResponseFormat::JsonSchema {
                name: "thing".into(),
                schema: schema.clone(),
            },
            true,
        )
        .unwrap();
        assert_eq!(v["type"], json!("json_schema"));
        assert_eq!(v["json_schema"]["name"], json!("thing"));
        assert_eq!(v["json_schema"]["schema"], schema);
        assert_eq!(v["json_schema"]["strict"], json!(true));
    }

    #[test]
    fn chat_json_schema_without_strict_omits_strict_key() {
        let v = to_chat_response_format(
            &ResponseFormat::JsonSchema {
                name: "t".into(),
                schema: json!({"type": "object"}),
            },
            false,
        )
        .unwrap();
        assert_eq!(v["json_schema"].get("strict"), None);
    }

    // ─── to_responses_text_format ────────────────────────────────

    #[test]
    fn responses_text_returns_none() {
        assert!(to_responses_text_format(&ResponseFormat::Text).is_none());
    }

    #[test]
    fn responses_json_object_returns_typed() {
        assert!(matches!(
            to_responses_text_format(&ResponseFormat::JsonObject),
            Some(TextFormat::JsonObject)
        ));
    }

    #[test]
    fn responses_json_schema_preserves_name_and_schema() {
        let schema = json!({"type": "object", "properties": {"y": {"type": "number"}}});
        let result = to_responses_text_format(&ResponseFormat::JsonSchema {
            name: "config".into(),
            schema: schema.clone(),
        })
        .unwrap();
        match result {
            TextFormat::JsonSchema { name, schema: s } => {
                assert_eq!(name, "config");
                assert_eq!(s, schema);
            }
            other => panic!("expected JsonSchema, got {:?}", other),
        }
    }

    // ─── merge_text_format ───────────────────────────────────────

    #[test]
    fn merge_passes_through_base_when_format_is_none() {
        let base = Some(TextConfig {
            format: None,
            verbosity: Some("medium".to_string()),
        });
        let result = merge_text_format(base.clone(), None);
        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r.verbosity, Some("medium".into()));
        assert!(r.format.is_none());
    }

    #[test]
    fn merge_overrides_format_preserves_verbosity() {
        let base = Some(TextConfig {
            format: None,
            verbosity: Some("medium".to_string()),
        });
        let result = merge_text_format(base, Some(&ResponseFormat::JsonObject)).unwrap();
        assert_eq!(result.verbosity, Some("medium".into()));
        assert!(matches!(result.format, Some(TextFormat::JsonObject)));
    }

    #[test]
    fn merge_creates_textconfig_when_base_none() {
        let result = merge_text_format(None, Some(&ResponseFormat::JsonObject)).unwrap();
        assert!(result.verbosity.is_none());
        assert!(matches!(result.format, Some(TextFormat::JsonObject)));
    }

    #[test]
    fn merge_returns_none_when_both_none() {
        assert!(merge_text_format(None, None).is_none());
    }
}
```

- [ ] **Step 4: Register module in `openai_common/mod.rs`**

Edit `src/providers/protocols/openai_common/mod.rs`. Current contents (after Task 1):

```rust
pub mod max_tokens;
pub mod openai_strict_schema;
pub mod provider_policy;
pub mod sse;
pub mod tools;
```

Add `response_format` line:

```rust
pub mod max_tokens;
pub mod openai_strict_schema;
pub mod provider_policy;
pub mod response_format;
pub mod sse;
pub mod tools;
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib response_format -- --nocapture`
Expected: PASS — 11 tests pass (4 for `to_chat_response_format` + 3 for `to_responses_text_format` + 4 for `merge_text_format`).

- [ ] **Step 6: Run full lib regression**

Run: `cargo test -p alephcore --lib`
Expected: No new failures; baseline failure (if any) unchanged.

- [ ] **Step 7: Commit**

```bash
git add src/config/types/provider.rs \
        src/providers/protocols/openai_common/response_format.rs \
        src/providers/protocols/openai_common/mod.rs
git commit -m "providers: add ResponseFormat enum + Chat/Responses wire translators"
```

---

## Task 4: `ProviderCapabilities::supports_response_format` + Strip-List Defense

**Files:**
- Modify: `src/providers/protocols/openai_common/provider_policy.rs`

- [ ] **Step 1: Add field to `ProviderCapabilities` struct**

Edit `src/providers/protocols/openai_common/provider_policy.rs`. Find the `ProviderCapabilities` struct (lines ~47–66):

```rust
#[derive(Debug, Clone, Default)]
pub struct ProviderCapabilities {
    /// Supports `store` field in Responses API
    pub supports_responses_store: bool,
    /// Supports `reasoning_effort` field
    pub supports_reasoning_effort: bool,
    /// Supports `prompt_cache_key` / `prompt_cache_retention`
    pub supports_prompt_cache: bool,
    /// Supports `service_tier` field
    pub supports_service_tier: bool,
    /// Supports strict JSON schema mode
    pub supports_strict_schema: bool,
    /// Supports server-side context compaction
    pub supports_server_compaction: bool,
    /// Known to reject object schemas without `properties`
    pub requires_object_properties: bool,
    /// Maximum context window (for compaction threshold calculation)
    pub context_window: Option<usize>,
}
```

Add `supports_response_format` after `supports_strict_schema`:

```rust
#[derive(Debug, Clone, Default)]
pub struct ProviderCapabilities {
    /// Supports `store` field in Responses API
    pub supports_responses_store: bool,
    /// Supports `reasoning_effort` field
    pub supports_reasoning_effort: bool,
    /// Supports `prompt_cache_key` / `prompt_cache_retention`
    pub supports_prompt_cache: bool,
    /// Supports `service_tier` field
    pub supports_service_tier: bool,
    /// Supports strict JSON schema mode
    pub supports_strict_schema: bool,
    /// Supports `response_format` (Chat) / `text.format` (Responses) field
    pub supports_response_format: bool,
    /// Supports server-side context compaction
    pub supports_server_compaction: bool,
    /// Known to reject object schemas without `properties`
    pub requires_object_properties: bool,
    /// Maximum context window (for compaction threshold calculation)
    pub context_window: Option<usize>,
}
```

- [ ] **Step 2: Initialize the field in all 13 EndpointClass branches**

In the same file, `resolve_capabilities` (lines ~209–342) has 13 match arms. Add `supports_response_format` to each. Set:

- `OpenAiPublic` → `true`
- `OpenAiCodex` → `true`
- All 11 others → `false`

For each branch, insert the new field on a fresh line between `supports_strict_schema` and `supports_server_compaction`. Example for `OpenAiPublic`:

```rust
EndpointClass::OpenAiPublic => ProviderCapabilities {
    supports_responses_store: true,
    supports_reasoning_effort: true,
    supports_prompt_cache: true,
    supports_service_tier: true,
    supports_strict_schema: true,
    supports_response_format: true,
    supports_server_compaction: true,
    requires_object_properties: false,
    context_window: Some(128_000),
},
```

For `OpenAiCodex`:

```rust
EndpointClass::OpenAiCodex => ProviderCapabilities {
    supports_responses_store: false,
    supports_reasoning_effort: true,
    supports_prompt_cache: false,
    supports_service_tier: false,
    supports_strict_schema: true,
    supports_response_format: true,
    supports_server_compaction: false,
    requires_object_properties: true,
    context_window: Some(128_000),
},
```

For the other 11 (`AzureOpenAi`, `AnthropicPublic`, `DeepSeekNative`, `GroqNative`, `MistralPublic`, `MoonshotNative`, `CerebrasNative`, `XAiNative`, `OpenRouter`, `Local`, `Custom`), set `supports_response_format: false`. Example for `DeepSeekNative`:

```rust
EndpointClass::DeepSeekNative => ProviderCapabilities {
    supports_responses_store: false,
    supports_reasoning_effort: false,
    supports_prompt_cache: false,
    supports_service_tier: false,
    supports_strict_schema: false,
    supports_response_format: false,
    supports_server_compaction: false,
    requires_object_properties: true,
    context_window: Some(64_000),
},
```

Apply this pattern to every branch. The field always sits between `supports_strict_schema` and `supports_server_compaction`.

- [ ] **Step 3: Add strip-list defense in `PayloadPolicy::apply`**

Find `PayloadPolicy::apply` (lines ~92–130). Find the existing reasoning strip block (lines ~101–105):

```rust
// Reasoning field
if self.strip_reasoning {
    payload.remove("reasoning");
    payload.remove("reasoning_effort");
}
```

Add directly below:

```rust
// Response format (when capability disabled)
if !self.capabilities.supports_response_format {
    payload.remove("response_format");
}
```

- [ ] **Step 4: Add a test that asserts the capability table is correct**

Append to the `#[cfg(test)] mod tests` block at the bottom of `provider_policy.rs`:

```rust
#[test]
fn openai_public_supports_response_format() {
    let caps = resolve_capabilities(EndpointClass::OpenAiPublic);
    assert!(caps.supports_response_format);
}

#[test]
fn openai_codex_supports_response_format() {
    let caps = resolve_capabilities(EndpointClass::OpenAiCodex);
    assert!(caps.supports_response_format);
}

#[test]
fn third_party_endpoints_do_not_support_response_format() {
    for class in [
        EndpointClass::AzureOpenAi,
        EndpointClass::AnthropicPublic,
        EndpointClass::DeepSeekNative,
        EndpointClass::GroqNative,
        EndpointClass::MistralPublic,
        EndpointClass::MoonshotNative,
        EndpointClass::CerebrasNative,
        EndpointClass::XAiNative,
        EndpointClass::OpenRouter,
        EndpointClass::Local,
        EndpointClass::Custom,
    ] {
        let caps = resolve_capabilities(class);
        assert!(
            !caps.supports_response_format,
            "{:?} unexpectedly supports response_format",
            class
        );
    }
}

#[test]
fn apply_strips_response_format_when_unsupported() {
    let policy = build_payload_policy(
        Some("https://api.deepseek.com"),
        "openai-chat",
        None,
    );
    let mut payload = serde_json::Map::new();
    payload.insert(
        "response_format".into(),
        serde_json::json!({"type": "json_object"}),
    );
    payload.insert("model".into(), serde_json::Value::String("dsk".into()));

    policy.apply(&mut payload);

    assert!(payload.get("response_format").is_none());
    assert!(payload.get("model").is_some());
}

#[test]
fn apply_keeps_response_format_when_supported() {
    let policy = build_payload_policy(
        Some("https://api.openai.com"),
        "openai-chat",
        None,
    );
    let mut payload = serde_json::Map::new();
    payload.insert(
        "response_format".into(),
        serde_json::json!({"type": "json_object"}),
    );

    policy.apply(&mut payload);

    assert!(payload.get("response_format").is_some());
}
```

- [ ] **Step 5: Run the new policy tests**

Run: `cargo test -p alephcore --lib provider_policy::tests::openai_public_supports_response_format provider_policy::tests::openai_codex_supports_response_format provider_policy::tests::third_party_endpoints_do_not_support_response_format provider_policy::tests::apply_strips_response_format_when_unsupported provider_policy::tests::apply_keeps_response_format_when_supported -- --nocapture`
Expected: PASS — all 5 new tests pass.

- [ ] **Step 6: Run full lib regression**

Run: `cargo test -p alephcore --lib`
Expected: All previously-passing tests still pass. (Baseline failure `test_apply_policy_strips_fields` remains as-is; not introduced by this task.)

- [ ] **Step 7: Commit**

```bash
git add src/providers/protocols/openai_common/provider_policy.rs
git commit -m "providers: capability-gate response_format per EndpointClass"
```

---

## Task 5: `ProviderConfig` New Fields + `test_config` Initializer

**Files:**
- Modify: `src/config/types/provider.rs:99-161, 204-234, 254-280, 284-310`

- [ ] **Step 1: Add two new fields to `ProviderConfig`**

Edit `src/config/types/provider.rs`. Find the existing `service_tier` field at the end of the `ProviderConfig` struct (around line ~158–160):

```rust
    /// Service tier for Anthropic API ("auto" or "default")
    #[serde(default)]
    pub service_tier: Option<String>,
}
```

Insert the two new fields directly before the closing `}`:

```rust
    /// Service tier for Anthropic API ("auto" or "default")
    #[serde(default)]
    pub service_tier: Option<String>,

    // OpenAI Cycle 2 fields
    /// Structured output format (None = free-form text).
    /// Capability-gated; silently dropped when endpoint doesn't support it.
    #[serde(default)]
    pub response_format: Option<ResponseFormat>,

    /// Whether the model accepts parallel tool calls (None = server default).
    /// When None, no `parallel_tool_calls` field is sent.
    #[serde(default)]
    pub parallel_tool_calls: Option<bool>,
}
```

- [ ] **Step 2: Update the `test_config` helper**

Find the `pub fn test_config(model: impl Into<String>) -> Self` function (around lines ~208–234). Find the closing brace of the `Self { ... }` block (around line ~232) just before `verified: false`. After `service_tier: None,` add the two new field initializers:

Locate this region:

```rust
            system_prompt_mode: None,
            model_behavior: None,
            verified: false,
            service_tier: None,
            stream_idle_timeout_secs: None,
            cache_retention: None,
        }
    }
}
```

Insert two new lines so the function becomes:

```rust
            system_prompt_mode: None,
            model_behavior: None,
            verified: false,
            service_tier: None,
            stream_idle_timeout_secs: None,
            cache_retention: None,
            response_format: None,
            parallel_tool_calls: None,
        }
    }
}
```

- [ ] **Step 3: Update the two `ProviderConfig { ... }` literal constructions in tests**

Still in `src/config/types/provider.rs`, the `#[cfg(test)] mod tests` block contains two test fns that build full `ProviderConfig { ... }` literals: `test_protocol_without_provider_type` (around lines ~254–282) and `test_protocol_defaults_to_openai` (around lines ~284–312). Both end with `cache_retention: None,`. Add `response_format: None,` and `parallel_tool_calls: None,` to both.

Find this pattern (appears twice):

```rust
            stream_idle_timeout_secs: None,
            cache_retention: None,
        };
```

Replace each occurrence with:

```rust
            stream_idle_timeout_secs: None,
            cache_retention: None,
            response_format: None,
            parallel_tool_calls: None,
        };
```

(Use `Edit` tool with `replace_all = true` since both occurrences are identical — but verify there are only the 2 expected matches in the file before running.)

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p alephcore`
Expected: Clean compile. If you see "missing field" errors, search the codebase for other `ProviderConfig { ... }` literal constructions:

```bash
grep -rn "ProviderConfig {" src/ tests/ --include="*.rs" | grep -v "ProviderConfig::"
```

Any literal construction (one that lists all fields explicitly rather than using `test_config()`) needs the two new fields added. Add `response_format: None,` and `parallel_tool_calls: None,` to each. If there are none beyond the two in `provider.rs`, you're done.

- [ ] **Step 5: Run lib tests for regression**

Run: `cargo test -p alephcore --lib provider::tests`
Expected: All 4 `provider.rs` unit tests pass.

Run: `cargo test -p alephcore --lib`
Expected: Same baseline as previous tasks — no new failures.

- [ ] **Step 6: Commit**

```bash
git add src/config/types/provider.rs
# Plus any other files touched in Step 4 if grep found literal constructions
git commit -m "config: add response_format + parallel_tool_calls to ProviderConfig"
```

---

## Task 6: Chat Adapter — `response_format` + `parallel_tool_calls` Wiring

**Files:**
- Modify: `src/providers/protocols/openai_chat/adapter.rs:76, 110-124`
- Modify: `src/providers/protocols/openai_chat/tests.rs` (append)

- [ ] **Step 1: Write failing integration tests**

Append to `src/providers/protocols/openai_chat/tests.rs`. Reuse the existing body-extraction helper (the same one used in Task 2's `build_chat_body_for_max_tokens`). At the top of the file, the `use` block should already include items from prior tasks. Add this import if not already present:

```rust
use crate::config::types::provider::ResponseFormat;
```

Append at the bottom:

```rust
// ─── Task 6: response_format wiring ───────────────────────────────

#[test]
fn chat_response_format_json_schema_emits_strict_for_openai() {
    use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
    use crate::config::ProviderConfig;
    let protocol = super::OpenAiProtocol::new_for_tests();
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.response_format = Some(ResponseFormat::JsonSchema {
        name: "answer".into(),
        schema: serde_json::json!({"type":"object","properties":{"x":{"type":"string"}}}),
    });
    // base_url None defaults to OpenAiPublic, which supports response_format.

    let payload = RequestPayload {
        messages: &[],
        system_prompt: None,
        max_tokens: None,
        temperature: None,
        think_level: None,
        tools: None,
        tool_choice: None,
        model: None,
    };
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert_eq!(body["response_format"]["type"], serde_json::json!("json_schema"));
    assert_eq!(body["response_format"]["json_schema"]["name"], serde_json::json!("answer"));
    assert_eq!(body["response_format"]["json_schema"]["strict"], serde_json::json!(true));
}

#[test]
fn chat_response_format_json_object() {
    use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
    use crate::config::ProviderConfig;
    let protocol = super::OpenAiProtocol::new_for_tests();
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.response_format = Some(ResponseFormat::JsonObject);

    let payload = RequestPayload {
        messages: &[],
        system_prompt: None,
        max_tokens: None,
        temperature: None,
        think_level: None,
        tools: None,
        tool_choice: None,
        model: None,
    };
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert_eq!(body["response_format"], serde_json::json!({"type":"json_object"}));
}

#[test]
fn chat_response_format_stripped_for_third_party_endpoint() {
    use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
    use crate::config::ProviderConfig;
    let protocol = super::OpenAiProtocol::new_for_tests();
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some("https://api.deepseek.com".into());
    config.response_format = Some(ResponseFormat::JsonObject);

    let payload = RequestPayload {
        messages: &[],
        system_prompt: None,
        max_tokens: None,
        temperature: None,
        think_level: None,
        tools: None,
        tool_choice: None,
        model: None,
    };
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert!(
        body.get("response_format").is_none(),
        "response_format must be absent for DeepSeek (capability disabled)"
    );
}

#[test]
fn chat_response_format_none_omits_field() {
    use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
    use crate::config::ProviderConfig;
    let protocol = super::OpenAiProtocol::new_for_tests();
    let config = ProviderConfig::test_config("gpt-4o");
    // response_format: None by default

    let payload = RequestPayload {
        messages: &[],
        system_prompt: None,
        max_tokens: None,
        temperature: None,
        think_level: None,
        tools: None,
        tool_choice: None,
        model: None,
    };
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert!(body.get("response_format").is_none());
}

// ─── Task 6: parallel_tool_calls wiring ───────────────────────────

#[test]
fn chat_parallel_tool_calls_some_true() {
    use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
    use crate::config::ProviderConfig;
    let protocol = super::OpenAiProtocol::new_for_tests();
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.parallel_tool_calls = Some(true);

    let payload = RequestPayload {
        messages: &[],
        system_prompt: None,
        max_tokens: None,
        temperature: None,
        think_level: None,
        tools: None,
        tool_choice: None,
        model: None,
    };
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert_eq!(body["parallel_tool_calls"], serde_json::json!(true));
}

#[test]
fn chat_parallel_tool_calls_some_false() {
    use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
    use crate::config::ProviderConfig;
    let protocol = super::OpenAiProtocol::new_for_tests();
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.parallel_tool_calls = Some(false);

    let payload = RequestPayload {
        messages: &[],
        system_prompt: None,
        max_tokens: None,
        temperature: None,
        think_level: None,
        tools: None,
        tool_choice: None,
        model: None,
    };
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert_eq!(body["parallel_tool_calls"], serde_json::json!(false));
}

#[test]
fn chat_parallel_tool_calls_none_omits_field() {
    use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
    use crate::config::ProviderConfig;
    let protocol = super::OpenAiProtocol::new_for_tests();
    let config = ProviderConfig::test_config("gpt-4o");
    // parallel_tool_calls: None by default

    let payload = RequestPayload {
        messages: &[],
        system_prompt: None,
        max_tokens: None,
        temperature: None,
        think_level: None,
        tools: None,
        tool_choice: None,
        model: None,
    };
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert!(body.get("parallel_tool_calls").is_none());
}
```

**IMPLEMENTER NOTE:** the body-extraction helper `extract_chat_body(req: RequestBuilder) -> serde_json::Value` should already be a shared test helper from Task 2. If it's not — and you find that Cycle 1 tests used a differently-named helper — use the canonical name and adapt the assertions above accordingly. The 7 tests above must extract a `Value` from the `RequestBuilder` to assert on JSON keys.

- [ ] **Step 2: Run tests to confirm failure**

Run: `cargo test -p alephcore --lib chat_response_format chat_parallel_tool_calls -- --nocapture`
Expected: FAIL — `response_format` not present (adapter doesn't write it yet); `parallel_tool_calls` not present (adapter doesn't write it yet).

- [ ] **Step 3: Implement the wiring**

Edit `src/providers/protocols/openai_chat/adapter.rs`. Add a new `use` near the existing imports at the top:

```rust
use crate::providers::protocols::openai_common::response_format::to_chat_response_format;
```

Find the policy construction (around lines 77–81):

```rust
let policy = build_payload_policy(
    config.base_url.as_deref(),
    "openai-chat",
    None,
);
```

Add **after** `let policy = ...` (and before the `if let Some(tool_defs) = payload.tools { ... }` block at line ~83):

```rust
// response_format: emit only when capability-enabled
if let Some(ref fmt) = config.response_format {
    if policy.capabilities.supports_response_format {
        if let Some(v) = to_chat_response_format(fmt, policy.capabilities.supports_strict_schema) {
            body["response_format"] = v;
        }
    }
}
```

Find the tool_choice block (around lines 115–124):

```rust
// Add tool_choice if specified
if let Some(ref choice) = payload.tool_choice {
    body["tool_choice"] = match choice {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::Required => json!("required"),
        ToolChoice::Specific(name) => {
            json!({"type": "function", "function": {"name": name}})
        }
        ToolChoice::None => json!("none"),
    };
}
```

Add directly **after** the closing `}` of this block:

```rust
// parallel_tool_calls: emit only when config explicitly sets it
if let Some(parallel) = config.parallel_tool_calls {
    body["parallel_tool_calls"] = json!(parallel);
}
```

- [ ] **Step 4: Run new tests to confirm pass**

Run: `cargo test -p alephcore --lib chat_response_format chat_parallel_tool_calls -- --nocapture`
Expected: PASS — all 7 new tests pass.

- [ ] **Step 5: Run full Chat suite for regression**

Run: `cargo test -p alephcore --lib openai_chat`
Expected: All previously-passing Chat tests still pass.

- [ ] **Step 6: Run full lib regression**

Run: `cargo test -p alephcore --lib`
Expected: Same baseline — no new failures.

- [ ] **Step 7: Commit**

```bash
git add src/providers/protocols/openai_chat/adapter.rs \
        src/providers/protocols/openai_chat/tests.rs
git commit -m "openai-chat: wire response_format + parallel_tool_calls from ProviderConfig"
```

---

## Task 7: Responses Adapter — Text Fusion + `parallel_tool_calls` Unhardcode

**Files:**
- Modify: `src/providers/protocols/openai_responses/mod.rs:172-173`
- Modify: `src/providers/protocols/openai_responses/tests.rs` (append)

- [ ] **Step 1: Write failing integration tests**

Append to `src/providers/protocols/openai_responses/tests.rs`. The Cycle 1 tests already include a `build_responses_request_for(...)` helper or similar pattern — locate it and use it. If no helper exists, the canonical pattern (from existing tests around line ~767, `let config = ProviderConfig::test_config("o3-mini")`) is:

```rust
let req = OpenAiResponsesProtocol::build_responses_request(&payload, "model", &variant, &config);
```

At the top of `tests.rs`, ensure the `use` block includes:

```rust
use crate::config::types::provider::ResponseFormat;
use crate::providers::responses::types::TextFormat;
```

Append at the bottom of the file:

```rust
// ─── Task 7: text fusion + parallel_tool_calls ────────────────────

#[test]
fn responses_text_merges_format_into_variant_verbosity() {
    use crate::providers::adapter::RequestPayload;
    use crate::config::ProviderConfig;

    let mut config = ProviderConfig::test_config("gpt-4o");
    config.response_format = Some(ResponseFormat::JsonObject);

    let payload = RequestPayload {
        messages: &[],
        system_prompt: None,
        max_tokens: None,
        temperature: None,
        think_level: None,
        tools: None,
        tool_choice: None,
        model: None,
    };

    // Build a variant that already has verbosity set (e.g., standard OpenAI).
    // The variant builder is module-private; the standard variant from
    // `OpenAiResponsesProtocol::new_standard()` is what gets used in
    // production. Inspect the test infrastructure for a `variant_with_verbosity`
    // helper or build a local variant inline. The key assertion: result.text
    // must have BOTH format=Some(JsonObject) AND the variant's verbosity preserved.

    let variant = standard_variant_with_verbosity("medium");
    let req = super::OpenAiResponsesProtocol::build_responses_request(
        &payload, "gpt-4o", &variant, &config,
    );

    let text = req.text.expect("text should be populated");
    assert!(matches!(text.format, Some(TextFormat::JsonObject)));
    assert_eq!(text.verbosity, Some("medium".to_string()));
}

#[test]
fn responses_text_passes_through_when_no_response_format() {
    use crate::providers::adapter::RequestPayload;
    use crate::config::ProviderConfig;

    let config = ProviderConfig::test_config("gpt-4o");
    // response_format: None (default)

    let payload = RequestPayload {
        messages: &[],
        system_prompt: None,
        max_tokens: None,
        temperature: None,
        think_level: None,
        tools: None,
        tool_choice: None,
        model: None,
    };

    let variant = standard_variant_with_verbosity("low");
    let req = super::OpenAiResponsesProtocol::build_responses_request(
        &payload, "gpt-4o", &variant, &config,
    );

    let text = req.text.expect("text should be the variant's original");
    assert!(text.format.is_none());
    assert_eq!(text.verbosity, Some("low".to_string()));
}

#[test]
fn responses_parallel_tool_calls_respects_config_some_false() {
    use crate::providers::adapter::RequestPayload;
    use crate::config::ProviderConfig;

    let mut config = ProviderConfig::test_config("gpt-4o");
    config.parallel_tool_calls = Some(false);

    let payload = RequestPayload {
        messages: &[],
        system_prompt: None,
        max_tokens: None,
        temperature: None,
        think_level: None,
        tools: None,
        tool_choice: None,
        model: None,
    };

    let variant = standard_test_variant();
    let req = super::OpenAiResponsesProtocol::build_responses_request(
        &payload, "gpt-4o", &variant, &config,
    );

    assert_eq!(req.parallel_tool_calls, Some(false));
}

#[test]
fn responses_parallel_tool_calls_none_omits_field() {
    use crate::providers::adapter::RequestPayload;
    use crate::config::ProviderConfig;

    let config = ProviderConfig::test_config("gpt-4o");
    // parallel_tool_calls: None (default)

    let payload = RequestPayload {
        messages: &[],
        system_prompt: None,
        max_tokens: None,
        temperature: None,
        think_level: None,
        tools: None,
        tool_choice: None,
        model: None,
    };

    let variant = standard_test_variant();
    let req = super::OpenAiResponsesProtocol::build_responses_request(
        &payload, "gpt-4o", &variant, &config,
    );

    // Verifies the hardcoded `Some(true)` is gone — when config is None,
    // the wire field must also be None.
    assert!(req.parallel_tool_calls.is_none());
}

// ─── Helpers (place near top of tests.rs, NOT inside #[test] fns) ─────

/// Build a standard ResponsesVariant for tests, with a chosen verbosity.
fn standard_variant_with_verbosity(verbosity: &str) -> super::ResponsesVariant {
    use crate::providers::responses::types::TextConfig;
    let mut v = standard_test_variant();
    v.text = Some(TextConfig {
        format: None,
        verbosity: Some(verbosity.to_string()),
    });
    v
}

/// Build a bare-bones standard ResponsesVariant for tests.
fn standard_test_variant() -> super::ResponsesVariant {
    // Inspect tests.rs for an existing helper; if one exists named e.g.
    // `default_test_variant()` or `make_variant()`, use that instead and
    // delete this duplicate. The variant's `text` field defaults to None.
    super::ResponsesVariant::default()
}
```

**IMPLEMENTER NOTES:**
1. Before writing this, **scan `tests.rs` for an existing variant builder**. The existing Cycle 1 tests around line ~764+ use `OpenAiResponsesProtocol::build_responses_request(&payload, "o3-mini", &variant, &config)` — find how `variant` is constructed there. Reuse that helper. If it's `let variant = ResponsesVariant::default()`, the helpers above are fine. If it's a richer setup, adapt.
2. The helpers `standard_variant_with_verbosity` and `standard_test_variant` MUST be placed near the top of the file (after `use` imports and before `#[test]` functions), not inside any test function. Helpers placed inside `#[test]` fns become local fns scoped to that single test.
3. `ResponsesVariant::default()` requires the struct to implement `Default`. If it doesn't, find the canonical constructor (e.g., `ResponsesVariant::standard()` or similar) and use that.

- [ ] **Step 2: Run tests to confirm failure**

Run: `cargo test -p alephcore --lib responses_text_merges responses_text_passes_through responses_parallel_tool_calls -- --nocapture`
Expected: FAIL — `parallel_tool_calls_none_omits_field` fails because line 172 hardcodes `Some(true)`; `text_merges` fails because the adapter doesn't yet call `merge_text_format`.

- [ ] **Step 3: Implement the two wiring changes in `mod.rs`**

Edit `src/providers/protocols/openai_responses/mod.rs`.

Find this line near the top of the file (with the other imports):

```rust
use super::openai_common::tools::extract_codex_account_id;
```

Add a new `use` for the merge helper. The cleanest location is at the file top with other top-level imports — find existing `use` lines for `openai_common::` and add:

```rust
use super::openai_common::response_format::merge_text_format;
```

(If the existing imports use a different path prefix like `crate::providers::protocols::openai_common::...`, match that style.)

Find lines 172–173 inside `build_responses_request`:

```rust
            parallel_tool_calls: Some(true),
            text: variant.text.clone(),
```

Replace with:

```rust
            parallel_tool_calls: config.parallel_tool_calls,
            text: merge_text_format(variant.text.clone(), config.response_format.as_ref()),
```

- [ ] **Step 4: Run new tests to confirm pass**

Run: `cargo test -p alephcore --lib responses_text_merges responses_text_passes_through responses_parallel_tool_calls -- --nocapture`
Expected: PASS — all 4 new tests pass.

- [ ] **Step 5: Run full Responses suite for regression**

Run: `cargo test -p alephcore --lib openai_responses`
Expected: All previously-passing Responses tests still pass. **If a Cycle 1 test exists that asserts `parallel_tool_calls == Some(true)` against the hardcoded value**, update that test to set `config.parallel_tool_calls = Some(true)` explicitly. Search:

```bash
grep -n "parallel_tool_calls" src/providers/protocols/openai_responses/tests.rs
```

If any assertion like `assert_eq!(req.parallel_tool_calls, Some(true))` exists without a corresponding `config.parallel_tool_calls = Some(true)` setup, that test was passing accidentally via the hardcoded literal and must be patched to set the config explicitly.

- [ ] **Step 6: Run full lib regression**

Run: `cargo test -p alephcore --lib`
Expected: Same baseline — no new failures.

- [ ] **Step 7: Commit**

```bash
git add src/providers/protocols/openai_responses/mod.rs \
        src/providers/protocols/openai_responses/tests.rs
git commit -m "openai-responses: fuse response_format into text; unhardcode parallel_tool_calls"
```

---

## Task 8: CHANGELOG + Final Cycle Verification

**Files:**
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Add three `[Unreleased]` entries**

Edit `CHANGELOG.md`. Find the `## [Unreleased]` section header and locate the `### Added`, `### Fixed`, and `### Changed` subsections inside it (these exist in the Cycle 1 retrospective entries — observation S5602/S5603 from this session's memory).

Under `### Added`:

```markdown
- **OpenAI protocol — response_format wiring**: `ProviderConfig` now exposes
  `response_format: Option<ResponseFormat>` (variants `Text` / `JsonObject` /
  `JsonSchema { name, schema }`). Both Chat and Responses adapters honor it.
  Capability-gated by `ProviderCapabilities::supports_response_format` —
  enabled for OpenAI public and ChatGPT Codex endpoints, conservative `false`
  for all third-party OpenAI-compatible backends (opt-in flip in Cycle 3).
  Responses adapter's `text.format` slot fuses with config; variant verbosity
  preserved. Strict mode emitted automatically when endpoint supports it.
- **OpenAI protocol — parallel_tool_calls config knob**: `ProviderConfig` now
  exposes `parallel_tool_calls: Option<bool>`. When `None`, no `parallel_tool_calls`
  field is sent on the wire (server default applies).
```

Under `### Fixed`:

```markdown
- **OpenAI Chat — max_completion_tokens for reasoning models**: Chat adapter
  now sends `max_completion_tokens` instead of `max_tokens` for `o1-` / `o3-` /
  `o4-` / `gpt-5` model families. Previously, any Aleph user configuring these
  models on a Chat endpoint received HTTP 400 from OpenAI; this is now
  resolved automatically based on model name. Responses adapter unaffected
  (already correctly uses `max_output_tokens`).
```

Under `### Changed`:

```markdown
- **OpenAI Responses — parallel_tool_calls no longer hardcoded**: The
  Responses adapter previously hardcoded `parallel_tool_calls: Some(true)`
  in `build_responses_request`. Now driven by `ProviderConfig.parallel_tool_calls`
  (default `None` → omit field). OpenAI public endpoint server default
  remains `true`, so observable behavior on OpenAI is unchanged. Compat
  backends will now receive `None` instead of forced `true`.
```

- [ ] **Step 2: Run final regression**

Run: `cargo test -p alephcore --lib`
Expected: All previously-passing tests still pass; 22 new tests from Cycle 2 all pass:
- Task 1: 6 max_tokens tests
- Task 3: 11 response_format tests
- Task 4: 5 provider_policy tests
- Task 2: 3 Chat integration tests for max_tokens
- Task 6: 7 Chat integration tests (4 response_format + 3 parallel_tool_calls)
- Task 7: 4 Responses integration tests (2 text fusion + 2 parallel_tool_calls)

Total: 36 new tests across all tasks (more than the spec's 18 — the per-helper unit tests added detail). Cumulative test count: 22 unit-level (in new files + provider_policy) + 14 integration-level (Chat + Responses appended).

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | grep -E "warning|error" | head -30`
Expected: Zero new warnings vs the Cycle 1 baseline (commit `ea0b2a3f3`). The 9 pre-existing clippy warnings from observation S5609 may still appear; document them as unchanged.

- [ ] **Step 4: Commit the CHANGELOG**

```bash
git add CHANGELOG.md
git commit -m "changelog: document Cycle 2 — response_format, max_completion_tokens, parallel_tool_calls"
```

- [ ] **Step 5: Manual smoke verification (non-blocking)**

These are AC-4 and AC-5 from the spec — they're non-blocking for spec acceptance and run against live OpenAI endpoints. Tag the manual smoke output (if performed) onto the cycle retrospective:

1. Configure an OpenAI public-endpoint provider with `models = ["o3-mini"]` and `max_tokens = 1024`. Send a Chat request. **Expect**: success (no HTTP 400; previously failed).
2. Configure a Responses provider with `parallel_tool_calls = false`. Trigger a multi-tool turn. **Expect**: tools execute serially.

Both can be confirmed via Aleph's CLI by inspecting the `aleph-server` logs at debug level (`request_body` shows the wire JSON).

- [ ] **Step 6: Cycle retrospective**

Use the `superpowers:subagent-driven-development` skill's "final cycle review" subagent dispatch pattern. Brief the reviewer with: list of all commits (T1–T8), the spec link, the file-touch summary, and AC-1 through AC-6. Reviewer's job: confirm SHIP IT or list specific blockers.

---

## Out-of-Scope — Cycle 3 Candidates

Carried forward from spec §10:

- **`seed`** — `Option<u64>` for determinism; trivial wiring, both protocols accept it at top level.
- **`logprobs` / `top_logprobs`** — Chat accepts these at top-level; Responses surfaces logprobs via `include: ["logprobs"]`. Per-protocol shape difference makes this its own bundle.
- **Per-class flip of `supports_response_format`** — promote AzureOpenAi or OpenRouter to `true` if user demand surfaces.
- **Strict-schema normalization for response_format JSON Schema** — reuse `openai_strict_schema::normalize_strict_schema` if compatibility issues surface in practice.
- **Anthropic protocol parity** — separate cycle, not in scope.
- **Pre-existing baseline failure `provider_policy::tests::test_apply_policy_strips_fields`** — confirmed not introduced by Cycle 2; investigation deferred.

---

## Notes on Pre-existing Baseline State

Per Cycle 1 retrospective and current session memory:

- The pre-existing failing test `provider_policy::tests::test_apply_policy_strips_fields` is NOT a Cycle-2 regression. It was failing on `main` at HEAD `fe4c1295e` before any Cycle 1 changes. **Do not** modify that test as part of Cycle 2 unless you discover Cycle 2 changes break it further — in which case revert your touch of `provider_policy.rs` and investigate.
- The 9 pre-existing clippy warnings (S5609) are unchanged by Cycle 1 work and should remain unchanged by Cycle 2 work. Document any new clippy output diffs in the final retrospective.
- Cycle 1 final commit (`ea0b2a3f3`) is the comparison baseline for "no new warnings" and "no new failures".
