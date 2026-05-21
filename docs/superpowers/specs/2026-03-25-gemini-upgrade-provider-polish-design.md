# Gemini Protocol Upgrade + Provider Polish

**Date**: 2026-03-25
**Status**: Approved
**Scope**: Gemini protocol full upgrade, Anthropic thinking extensions, OpenAI Responses reasoning events

## Summary

Upgrade the Gemini protocol adapter to support Gemini 3 features (native tool call IDs, `thinkingLevel` dual-mode, `thought` marker parsing, JSON Schema sanitization). Additionally, polish Anthropic and OpenAI Responses adapters with missing features: adaptive thinking, output config types, reasoning streaming events, and ThinkLevel mapping completeness.

## Context

### Current State
- **Gemini**: Functional but outdated — synthetic tool IDs only, no schema sanitization, single `thinkingBudget` mode, no `thought` marker parsing
- **Anthropic**: Thinking hardcoded to `"enabled"` type, no `"adaptive"` or `"disabled"`, no `display` field, no `OutputConfig` type
- **OpenAI Responses**: No `reasoning_summary_text.delta` event handling, `ThinkLevel::Minimal`/`XHigh` map to `None`, standard variant missing `include` for reasoning

### Reference
- **OpenClaw** (`~/Github/openclaw`): Production provider adapters with Gemini schema cleaning (`clean-for-gemini.ts`), composable stream wrappers, `ModelCompatConfig` flags
- **Gemini API**: https://ai.google.dev/gemini-api/docs — Gemini 3 adds native `id` field on `functionCall`, `thinkingLevel` enum, `thought: true` on text parts
- **Anthropic API**: https://platform.claude.com/docs — `thinking.type: "adaptive"`, `thinking.display`, `output_config.effort`
- **OpenAI Responses API**: https://developers.openai.com/api/reference — `response.reasoning_summary_text.delta/done` events

## Design

### A. Gemini Protocol Upgrade (Primary Focus)

#### A1. Schema Sanitization — New file `providers/gemini/schema.rs`

**Purpose**: Transform standard JSON Schema into the OpenAPI Schema subset Gemini accepts.

**Core function**: `pub fn clean_schema_for_gemini(schema: &mut Value)`

**Processing steps** (recursive, depth-first):
1. **Strip unsupported keywords** — Remove ~21 keywords Gemini rejects:
   `patternProperties`, `additionalProperties`, `$schema`, `$id`, `$ref`, `$defs`, `definitions`, `examples`, `minLength`, `maxLength`, `minimum`, `maximum`, `multipleOf`, `pattern`, `format`, `minItems`, `maxItems`, `uniqueItems`, `minProperties`, `maxProperties`, `title`
2. **Flatten `anyOf`/`oneOf`** — If two items where one is `{"type": "null"}`, take the non-null item. If all items are string enums, merge into single enum. Otherwise take first item's type.
3. **Inline `$ref`** — Resolve `$ref` pointers against `$defs`/`definitions` in the same schema, replace with inlined content. (schemars generates `$ref` for nested types.)
4. **Ensure top-level `type: "object"`** — Existing logic preserved.

**Called from**: `gemini.rs` `build_request`, when constructing `GeminiFunctionDeclaration` parameters.

#### A2. Native Tool Call ID with Fallback

**Change in `parse_gemini_sse_chunk`**:
```rust
// Before: always synthetic
let id = format!("gemini_fc_{}", *fc_counter);

// After: prefer native, fallback to synthetic
let id = fc.get("id")
    .and_then(|v| v.as_str())
    .map(|s| s.to_string())
    .unwrap_or_else(|| {
        let synthetic = format!("gemini_fc_{}", *fc_counter);
        *fc_counter += 1;
        synthetic
    });
```

**Type changes**:
- `GeminiFunctionCall`: add `#[serde(default)] pub id: Option<String>`
- `GeminiFunctionResponse`: add `#[serde(default, skip_serializing_if = "Option::is_none")] pub id: Option<String>`
- `convert_messages`: when converting `ToolResult`, pass through the tool call ID to `FunctionResponse.id`

**Note**: Counter only increments on fallback, so native IDs don't consume synthetic slots.

#### A3. Thinking Dual-Mode — `thinkingLevel` (Gemini 3) + `thinkingBudget` (Gemini 2.5)

**Type change in `gemini/types.rs`**:
```rust
pub struct ThinkingConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<i32>,       // Gemini 2.5: 0-32768, -1=dynamic
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,     // Gemini 3: "MINIMAL"/"LOW"/"MEDIUM"/"HIGH"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_thoughts: Option<bool>,     // Whether to return thought content
}
```

**Note**: `thinking_budget` changes from `Option<u32>` to `Option<i32>` — deliberate type widening to support Gemini 2.5's `-1` (dynamic budget) value. Existing positive values are unaffected.

**Model detection in `map_think_level`**:

Uses inverted logic: default to `thinkingLevel` (newer API), fallback to `thinkingBudget` for known Gemini 2.5 models. This is more future-proof as new models will likely support `thinkingLevel`.

```rust
fn map_think_level(level: &ThinkLevel, model: &str) -> Option<ThinkingConfig> {
    if level == &ThinkLevel::Off { return None; }
    // Gemini 2.5 models use thinkingBudget; all others (including Gemini 3+) use thinkingLevel
    let use_budget = model.contains("gemini-2.5");
    if !use_budget {
        let level_str = match level {
            ThinkLevel::Minimal => "MINIMAL",
            ThinkLevel::Low => "LOW",
            ThinkLevel::Medium => "MEDIUM",
            ThinkLevel::High | ThinkLevel::XHigh => "HIGH",
            ThinkLevel::Off => unreachable!(),
        };
        Some(ThinkingConfig {
            thinking_budget: None,
            thinking_level: Some(level_str.into()),
            include_thoughts: Some(true),
        })
    } else {
        // Gemini 2.5 style
        let budget = match level {
            ThinkLevel::Minimal => 500,
            ThinkLevel::Low => 1000,
            ThinkLevel::Medium => 2000,
            ThinkLevel::High => 4000,
            ThinkLevel::XHigh => 8000,
            ThinkLevel::Off => unreachable!(),
        };
        Some(ThinkingConfig {
            thinking_budget: Some(budget),
            thinking_level: None,
            include_thoughts: Some(true),
        })
    }
}
```

**Signature change**: `map_think_level` now takes `model: &str` parameter. Caller in `build_request` passes `config.default_model()`.

#### A4. `thought: true` Marker Parsing

**Change in `parse_gemini_sse_chunk`**, text part handling:
```rust
if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
    if !text.is_empty() {
        let is_thought = part.get("thought")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if is_thought {
            out.push_back(Ok(ProviderDelta::ThinkingDelta(text.to_string())));
        } else {
            out.push_back(Ok(ProviderDelta::TextDelta(text.to_string())));
        }
    }
}
```

#### A5. Usage Enhancement — `thoughtsTokenCount`

In `parse_gemini_sse_chunk`, extract thinking tokens from usage metadata:
```rust
let thinking = usage
    .get("thoughtsTokenCount")
    .and_then(|v| v.as_u64())
    .map(|v| v as u32);
```

Requires `TokenUsage` extension (see Section D).

---

### B. Anthropic Protocol Polish

#### B1. `ThinkingBlock` Extension

**Before**:
```rust
pub struct ThinkingBlock {
    pub thinking_type: String,
    pub budget_tokens: u32,
}
```

**After**:
```rust
pub struct ThinkingBlock {
    #[serde(rename = "type")]
    pub thinking_type: String,              // "enabled" | "disabled" | "adaptive"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u32>,         // Required for "enabled", optional for "adaptive"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,            // "summarized" | "omitted"
}
```

**`map_think_level` adjustment**: All existing levels continue to produce `thinking_type: "enabled"` with `budget_tokens: Some(n)`. No behavioral change for current callers.

**Construction site update** in `anthropic.rs` (line ~273): Must change from `budget_tokens: budget` to `budget_tokens: Some(budget)` and add `display: None`.

#### B2. `OutputConfig` Type (Predefined, Not Wired)

New types in `anthropic/types.rs`:
```rust
pub struct OutputConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,           // "low" | "medium" | "high" | "max"
}
```

Added to `MessagesRequest` as `Option<OutputConfig>`, defaulting to `None`. Not wired to `RequestPayload` — this is a forward-looking type definition for when the feature is needed.

---

### C. OpenAI Responses API Polish

#### C1. Reasoning Stream Events

**New `StreamEvent` variants in `responses/types.rs`**:
```rust
#[serde(rename = "response.reasoning_summary_part.added")]
ReasoningSummaryPartAdded { item_id: String, output_index: usize },

#[serde(rename = "response.reasoning_summary_text.delta")]
ReasoningSummaryTextDelta { delta: String, item_id: String, output_index: usize },

#[serde(rename = "response.reasoning_summary_text.done")]
ReasoningSummaryTextDone { text: String, item_id: String, output_index: usize },

#[serde(rename = "response.reasoning_summary_part.done")]
ReasoningSummaryPartDone { item_id: String, output_index: usize },
```

**Stream parsing**: `ReasoningSummaryTextDelta` → `ProviderDelta::ThinkingDelta(delta)`

#### C2. `include` Default for Standard Variant

In `build_responses_request`, when `variant.include` is `None` **and** the endpoint is official OpenAI, set default:
```rust
include: variant.include.clone().or_else(|| {
    if official {
        Some(vec!["reasoning.encrypted_content".into()])
    } else {
        None
    }
}),
```

This ensures o-series models' reasoning content is requested on official endpoints, while avoiding potential errors on third-party proxies (OpenRouter, etc.) that may not support the `include` parameter.

#### C3. `build_reasoning` ThinkLevel Completeness

**Before**: `Minimal` → None, `XHigh` → None
**After**:
- `ThinkLevel::Minimal` → `None` (preserved — sending reasoning config on default-level requests to non-o-series models could cause errors on third-party endpoints)
- `ThinkLevel::XHigh` → `effort: "high"` (OpenAI max is "high"; previously this was silently dropped)
- `ThinkLevel::Off` → `None` (no change)

---

### D. Cross-Protocol: `TokenUsage` Extension

**In `providers/adapter.rs`**:
```rust
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: Option<u32>,
    pub thinking_tokens: Option<u32>,     // NEW
}
```

All existing constructors updated to set `thinking_tokens: None` by default. Only Gemini populates this from `thoughtsTokenCount`.

---

## File Change Summary

| File | Action | Description |
|------|--------|-------------|
| `providers/gemini/schema.rs` | **CREATE** | Gemini JSON Schema sanitization (~120 lines) |
| `providers/gemini/mod.rs` | MODIFY | Add `pub mod schema;` |
| `providers/gemini/types.rs` | MODIFY | ThinkingConfig dual-mode, FunctionCall/Response + id field |
| `providers/protocols/gemini.rs` | MODIFY | Schema cleaning call, native ID fallback, thought parsing, thinkingLevel, usage thinking tokens |
| `providers/anthropic/types.rs` | MODIFY | ThinkingBlock extension, OutputConfig type |
| `providers/protocols/anthropic.rs` | MODIFY | ThinkingBlock construction adaptation |
| `providers/responses/types.rs` | MODIFY | StreamEvent + reasoning variants |
| `providers/responses/shared.rs` | MODIFY | build_reasoning completeness, include default |
| `providers/adapter.rs` | MODIFY | TokenUsage + thinking_tokens |
| `providers/delta.rs` | MODIFY | TokenUsage construction sites |

**No changes to**: `RequestPayload`, `ProviderDelta` enum (ThinkingDelta already exists), `ProtocolAdapter` trait, `HttpProvider`, retry logic.

## Testing Strategy

Each change gets unit tests in the same file:
- **schema.rs**: Test keyword stripping, anyOf flattening, $ref inlining, edge cases (empty schema, deeply nested)
- **gemini.rs**: Test native ID extraction, fallback to synthetic, thought marker → ThinkingDelta, thinkingLevel vs thinkingBudget per model, thoughtsTokenCount in usage
- **anthropic.rs**: Test ThinkingBlock serialization with display/adaptive
- **responses**: Test reasoning event parsing → ThinkingDelta, include default, build_reasoning for all ThinkLevel variants

## Non-Goals

- No changes to `ProviderConfig` or `RequestPayload` for service_tier/effort — these can be added when callers need them
- No unified ThinkLevel mapping layer — each protocol's mapping is semantically different
- No OpenAI Chat Completions changes (separate protocol, already adequate)
- No Ollama changes
