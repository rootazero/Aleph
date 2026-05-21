# Gemini Protocol Upgrade + Provider Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade Gemini protocol to Gemini 3 features, polish Anthropic/OpenAI Responses adapters with missing thinking and reasoning capabilities.

**Architecture:** Protocol-centric changes — each protocol adapter gets independent modifications. A new `gemini/schema.rs` module handles Gemini-specific JSON Schema sanitization. Cross-protocol `TokenUsage` gets a `thinking_tokens` field. No trait or `RequestPayload` changes.

**Tech Stack:** Rust, serde_json, async_trait, futures streams

**Spec:** `docs/superpowers/specs/2026-03-25-gemini-upgrade-provider-polish-design.md`

---

### Task 1: Cross-Protocol Foundation — `TokenUsage` Extension

**Files:**
- Modify: `src/providers/adapter.rs:244-250`

- [ ] **Step 1: Add `thinking_tokens` field to `TokenUsage`**

In `src/providers/adapter.rs`, change the `TokenUsage` struct:

```rust
/// Token usage statistics
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: Option<u32>,
    /// Thinking/reasoning tokens consumed (Gemini `thoughtsTokenCount`)
    pub thinking_tokens: Option<u32>,
}
```

- [ ] **Step 2: Fix all `TokenUsage` construction sites**

Add `thinking_tokens: None` to every existing `TokenUsage { ... }` literal in:
- `src/providers/protocols/gemini.rs:523` (will be updated in Task 4)
- `src/providers/protocols/anthropic.rs:620`
- `src/providers/protocols/openai_responses.rs:435`
- `src/providers/protocols/openai_chat.rs:572`
- `src/providers/delta.rs:383` and `delta.rs:452`

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: compiles with no errors

- [ ] **Step 4: Commit**

```bash
git add src/providers/adapter.rs src/providers/protocols/ src/providers/delta.rs
git commit -m "providers: add thinking_tokens to TokenUsage"
```

---

### Task 2: Gemini Schema Sanitization — New Module

**Files:**
- Create: `src/providers/gemini/schema.rs`
- Modify: `src/providers/gemini/mod.rs`

- [ ] **Step 1: Add module declaration**

In `src/providers/gemini/mod.rs`, add:

```rust
pub mod schema;
pub mod types;

pub use types::*;
```

(Replace the existing content which has `pub mod types;` and `pub use types::*;`)

- [ ] **Step 2: Write failing tests for schema sanitization**

Create `src/providers/gemini/schema.rs` with tests first:

```rust
//! Gemini JSON Schema sanitization
//!
//! Transforms standard JSON Schema into the OpenAPI Schema subset that
//! Google Gemini accepts. Gemini rejects ~21 keywords that are valid in
//! JSON Schema draft-07 / 2020-12.

use serde_json::Value;

/// Keywords that Gemini API rejects in tool parameter schemas.
const UNSUPPORTED_KEYWORDS: &[&str] = &[
    "patternProperties",
    "additionalProperties",
    "$schema",
    "$id",
    "$ref",
    "$defs",
    "definitions",
    "examples",
    "minLength",
    "maxLength",
    "minimum",
    "maximum",
    "multipleOf",
    "pattern",
    "format",
    "minItems",
    "maxItems",
    "uniqueItems",
    "minProperties",
    "maxProperties",
    "title",
];

/// Clean a JSON Schema in-place for Gemini compatibility.
///
/// 1. Resolves `$ref` pointers against local `$defs`/`definitions`
/// 2. Strips unsupported keywords recursively
/// 3. Flattens `anyOf`/`oneOf` unions where possible
/// 4. Ensures top-level `type: "object"` if missing
pub fn clean_schema_for_gemini(schema: &mut Value) {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_strips_unsupported_keywords() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 100,
                    "pattern": "^[a-z]+$",
                    "format": "email",
                    "title": "User Name"
                }
            },
            "$schema": "http://json-schema.org/draft-07/schema#",
            "additionalProperties": false
        });

        clean_schema_for_gemini(&mut schema);

        // Top-level unsupported keywords removed
        assert!(schema.get("$schema").is_none());
        assert!(schema.get("additionalProperties").is_none());

        // Nested unsupported keywords removed
        let name = &schema["properties"]["name"];
        assert!(name.get("minLength").is_none());
        assert!(name.get("maxLength").is_none());
        assert!(name.get("pattern").is_none());
        assert!(name.get("format").is_none());
        assert!(name.get("title").is_none());

        // Supported keywords preserved
        assert_eq!(name["type"], "string");
    }

    #[test]
    fn test_flattens_nullable_anyof() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "value": {
                    "anyOf": [
                        { "type": "string" },
                        { "type": "null" }
                    ]
                }
            }
        });

        clean_schema_for_gemini(&mut schema);

        let value = &schema["properties"]["value"];
        assert_eq!(value["type"], "string");
        assert!(value.get("anyOf").is_none());
    }

    #[test]
    fn test_flattens_enum_anyof() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "color": {
                    "anyOf": [
                        { "const": "red", "type": "string" },
                        { "const": "blue", "type": "string" },
                        { "const": "green", "type": "string" }
                    ]
                }
            }
        });

        clean_schema_for_gemini(&mut schema);

        let color = &schema["properties"]["color"];
        assert_eq!(color["type"], "string");
        assert!(color.get("anyOf").is_none());
        let enums = color["enum"].as_array().unwrap();
        assert_eq!(enums, &[json!("red"), json!("blue"), json!("green")]);
    }

    #[test]
    fn test_inlines_ref() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "address": { "$ref": "#/$defs/Address" }
            },
            "$defs": {
                "Address": {
                    "type": "object",
                    "properties": {
                        "city": { "type": "string" }
                    }
                }
            }
        });

        clean_schema_for_gemini(&mut schema);

        // $ref replaced with inlined content
        let addr = &schema["properties"]["address"];
        assert_eq!(addr["type"], "object");
        assert_eq!(addr["properties"]["city"]["type"], "string");
        // $defs removed
        assert!(schema.get("$defs").is_none());
    }

    #[test]
    fn test_ensures_top_level_object_type() {
        let mut schema = json!({
            "properties": {
                "x": { "type": "string" }
            }
        });

        clean_schema_for_gemini(&mut schema);

        assert_eq!(schema["type"], "object");
    }

    #[test]
    fn test_handles_empty_schema() {
        let mut schema = json!({});
        clean_schema_for_gemini(&mut schema);
        assert_eq!(schema["type"], "object");
    }

    #[test]
    fn test_deeply_nested_stripping() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "outer": {
                    "type": "object",
                    "properties": {
                        "inner": {
                            "type": "array",
                            "items": {
                                "type": "string",
                                "minLength": 1,
                                "format": "uri"
                            },
                            "minItems": 1,
                            "maxItems": 10
                        }
                    },
                    "additionalProperties": true
                }
            }
        });

        clean_schema_for_gemini(&mut schema);

        let items = &schema["properties"]["outer"]["properties"]["inner"]["items"];
        assert!(items.get("minLength").is_none());
        assert!(items.get("format").is_none());

        let inner = &schema["properties"]["outer"]["properties"]["inner"];
        assert!(inner.get("minItems").is_none());
        assert!(inner.get("maxItems").is_none());

        let outer = &schema["properties"]["outer"];
        assert!(outer.get("additionalProperties").is_none());
    }

    #[test]
    fn test_unflattenable_anyof_takes_first() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "data": {
                    "anyOf": [
                        { "type": "string" },
                        { "type": "integer" }
                    ]
                }
            }
        });

        clean_schema_for_gemini(&mut schema);

        // Can't flatten two different types — take first
        let data = &schema["properties"]["data"];
        assert_eq!(data["type"], "string");
        assert!(data.get("anyOf").is_none());
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib gemini::schema -- --nocapture`
Expected: FAIL with "not yet implemented"

- [ ] **Step 4: Implement `clean_schema_for_gemini`**

Replace the `todo!()` in `clean_schema_for_gemini` with:

```rust
pub fn clean_schema_for_gemini(schema: &mut Value) {
    // Step 1: Resolve $ref before anything else (needs $defs intact)
    resolve_refs(schema);

    // Step 2: Recursive clean (strips keywords, flattens unions)
    clean_recursive(schema);

    // Step 3: Ensure top-level type
    if let Some(obj) = schema.as_object_mut() {
        obj.entry("type")
            .or_insert_with(|| Value::String("object".into()));
    }
}

/// Resolve all `$ref` pointers in-place using definitions from `$defs` or `definitions`.
fn resolve_refs(schema: &mut Value) {
    // Extract definitions first
    let defs = schema
        .get("$defs")
        .or_else(|| schema.get("definitions"))
        .cloned()
        .unwrap_or(Value::Null);

    if !defs.is_null() {
        resolve_refs_recursive(schema, &defs);
    }
}

fn resolve_refs_recursive(node: &mut Value, defs: &Value) {
    match node {
        Value::Object(map) => {
            // Check if this node IS a $ref
            if let Some(ref_val) = map.get("$ref").and_then(|v| v.as_str()).map(|s| s.to_string()) {
                // Parse "#/$defs/Name" or "#/definitions/Name"
                let name = ref_val
                    .strip_prefix("#/$defs/")
                    .or_else(|| ref_val.strip_prefix("#/definitions/"));

                if let Some(name) = name {
                    if let Some(resolved) = defs.get(name) {
                        let mut resolved = resolved.clone();
                        // Recursively resolve nested refs
                        resolve_refs_recursive(&mut resolved, defs);
                        *node = resolved;
                        return;
                    }
                }
                // Can't resolve — remove the $ref (will be stripped later anyway)
            }

            // Recurse into all values
            let keys: Vec<String> = map.keys().cloned().collect();
            for key in keys {
                if let Some(child) = map.get_mut(&key) {
                    resolve_refs_recursive(child, defs);
                }
            }
        }
        Value::Array(arr) => {
            for item in arr.iter_mut() {
                resolve_refs_recursive(item, defs);
            }
        }
        _ => {}
    }
}

/// Recursively strip unsupported keywords and flatten unions.
fn clean_recursive(node: &mut Value) {
    let obj = match node.as_object_mut() {
        Some(o) => o,
        None => return,
    };

    // Strip unsupported keywords at this level
    for kw in UNSUPPORTED_KEYWORDS {
        obj.remove(*kw);
    }

    // Flatten anyOf/oneOf
    for key in &["anyOf", "oneOf"] {
        if let Some(variants) = obj.remove(*key) {
            if let Some(arr) = variants.as_array() {
                flatten_union(obj, arr);
            }
        }
    }

    // Recurse into properties
    if let Some(props) = obj.get_mut("properties") {
        if let Some(props_map) = props.as_object_mut() {
            for (_key, prop_val) in props_map.iter_mut() {
                clean_recursive(prop_val);
            }
        }
    }

    // Recurse into items (array schemas)
    if let Some(items) = obj.get_mut("items") {
        clean_recursive(items);
    }
}

/// Flatten a union (anyOf/oneOf variants) into the parent object.
///
/// Strategy:
/// 1. If 2 items and one is `{type: "null"}` → take the non-null item (nullable)
/// 2. If all items are `{const: X, type: T}` or `{enum: [X], type: T}` with same T
///    → merge into `{type: T, enum: [values]}`
/// 3. Otherwise → take first item's type
fn flatten_union(parent: &mut serde_json::Map<String, Value>, variants: &[Value]) {
    if variants.is_empty() {
        return;
    }

    // Strategy 1: nullable (2 items, one null)
    if variants.len() == 2 {
        let null_idx = variants.iter().position(|v| {
            v.get("type").and_then(|t| t.as_str()) == Some("null")
        });
        if let Some(idx) = null_idx {
            let non_null = &variants[1 - idx];
            if let Some(obj) = non_null.as_object() {
                for (k, v) in obj {
                    if !UNSUPPORTED_KEYWORDS.contains(&k.as_str()) {
                        parent.insert(k.clone(), v.clone());
                    }
                }
            }
            return;
        }
    }

    // Strategy 2: all literal/single-enum with same type → merge enum
    if let Some(merged) = try_merge_enum(variants) {
        parent.insert("type".into(), Value::String(merged.0));
        parent.insert("enum".into(), Value::Array(merged.1));
        return;
    }

    // Strategy 3: take first variant's type
    if let Some(first) = variants.first() {
        if let Some(obj) = first.as_object() {
            for (k, v) in obj {
                if !UNSUPPORTED_KEYWORDS.contains(&k.as_str()) {
                    parent.insert(k.clone(), v.clone());
                }
            }
        }
    }
}

/// Try to merge all variants into a single enum.
/// Returns (type_string, enum_values) if successful.
fn try_merge_enum(variants: &[Value]) -> Option<(String, Vec<Value>)> {
    let mut common_type: Option<&str> = None;
    let mut values = Vec::new();

    for v in variants {
        let obj = v.as_object()?;
        let ty = obj.get("type")?.as_str()?;

        // Must have const or single-element enum
        let val = if let Some(c) = obj.get("const") {
            c.clone()
        } else if let Some(e) = obj.get("enum").and_then(|e| e.as_array()) {
            if e.len() == 1 { e[0].clone() } else { return None; }
        } else {
            return None;
        };

        match common_type {
            None => common_type = Some(ty),
            Some(ct) if ct != ty => return None,
            _ => {}
        }

        values.push(val);
    }

    Some((common_type?.to_string(), values))
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib gemini::schema -- --nocapture`
Expected: all 7 tests PASS

- [ ] **Step 6: Commit**

```bash
git add src/providers/gemini/schema.rs src/providers/gemini/mod.rs
git commit -m "gemini: add JSON Schema sanitization for Gemini API compatibility"
```

---

### Task 3: Gemini Types Upgrade

**Files:**
- Modify: `src/providers/gemini/types.rs`

- [ ] **Step 1: Update `ThinkingConfig` to dual-mode**

Change from:
```rust
pub struct ThinkingConfig {
    /// Budget for thinking tokens
    pub thinking_budget: Option<u32>,
}
```

To:
```rust
/// Thinking configuration for Gemini.
///
/// - Gemini 2.5 models use `thinking_budget` (integer token count, -1=dynamic)
/// - Gemini 3+ models use `thinking_level` (enum: MINIMAL/LOW/MEDIUM/HIGH)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingConfig {
    /// Token budget for thinking (Gemini 2.5). -1 = dynamic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<i32>,
    /// Thinking level enum (Gemini 3+): "MINIMAL", "LOW", "MEDIUM", "HIGH"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
    /// Whether to include thought content in response
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_thoughts: Option<bool>,
}
```

- [ ] **Step 2: Add `id` field to `GeminiFunctionCall`**

Change from:
```rust
pub struct GeminiFunctionCall {
    pub name: String,
    pub args: Value,
}
```

To:
```rust
pub struct GeminiFunctionCall {
    pub name: String,
    pub args: Value,
    /// Native tool call ID (Gemini 3+ models). Absent on older models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}
```

- [ ] **Step 3: Add `id` field to `GeminiFunctionResponse`**

Change from:
```rust
pub struct GeminiFunctionResponse {
    pub name: String,
    pub response: serde_json::Value,
}
```

To:
```rust
pub struct GeminiFunctionResponse {
    pub name: String,
    pub response: serde_json::Value,
    /// Pass back the tool call ID when available
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}
```

- [ ] **Step 4: Update `ResponsePart` to include `thought` marker**

Change from:
```rust
pub struct ResponsePart {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function_call: Option<GeminiFunctionCall>,
}
```

To:
```rust
pub struct ResponsePart {
    /// Text content (present for text parts)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Whether this text part is a thinking/reasoning trace
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought: Option<bool>,
    /// Function call (present for tool-use parts)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function_call: Option<GeminiFunctionCall>,
}
```

- [ ] **Step 5: Update existing tests that construct ThinkingConfig**

In `types.rs` test `test_thinking_config` (line ~237), update:
```rust
thinking_config: Some(ThinkingConfig {
    thinking_budget: Some(1000),
    thinking_level: None,
    include_thoughts: None,
}),
```

- [ ] **Step 6: Add test for dual-mode ThinkingConfig serialization**

Add to `types.rs` tests:
```rust
#[test]
fn test_thinking_config_level_mode() {
    let config = GenerationConfig {
        max_output_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        thinking_config: Some(ThinkingConfig {
            thinking_budget: None,
            thinking_level: Some("HIGH".to_string()),
            include_thoughts: Some(true),
        }),
    };
    let json = serde_json::to_string(&config).unwrap();
    assert!(json.contains("thinkingLevel"));
    assert!(json.contains("HIGH"));
    assert!(json.contains("includeThoughts"));
    assert!(!json.contains("thinkingBudget"));
}

#[test]
fn test_deserialize_response_with_thought_marker() {
    let json = r#"{
        "candidates": [{
            "content": {
                "parts": [
                    {"text": "Let me think...", "thought": true},
                    {"text": "The answer is 42"}
                ]
            },
            "finishReason": "STOP"
        }]
    }"#;
    let response: GenerateContentResponse = serde_json::from_str(json).unwrap();
    let parts = &response.candidates.unwrap()[0].content.parts;
    assert_eq!(parts[0].thought, Some(true));
    assert_eq!(parts[1].thought, None);
}

#[test]
fn test_deserialize_function_call_with_id() {
    let json = r#"{
        "candidates": [{
            "content": {
                "parts": [{
                    "functionCall": {
                        "name": "search",
                        "id": "abc123",
                        "args": {"query": "rust"}
                    }
                }]
            },
            "finishReason": "FUNCTION_CALL"
        }]
    }"#;
    let response: GenerateContentResponse = serde_json::from_str(json).unwrap();
    let fc = response.candidates.unwrap()[0].content.parts[0]
        .function_call.as_ref().unwrap();
    assert_eq!(fc.id.as_deref(), Some("abc123"));
}
```

- [ ] **Step 7: Verify tests pass**

Run: `cargo test -p alephcore --lib gemini::types -- --nocapture`
Expected: all PASS

- [ ] **Step 8: Commit**

```bash
git add src/providers/gemini/types.rs
git commit -m "gemini: upgrade types for Gemini 3 (dual thinking, native tool IDs, thought markers)"
```

---

### Task 4: Gemini Protocol Adapter Upgrade

**Files:**
- Modify: `src/providers/protocols/gemini.rs`

- [ ] **Step 1: Update imports to include schema module**

Add to the imports at top of `gemini.rs`:
```rust
use crate::providers::gemini::schema::clean_schema_for_gemini;
```

- [ ] **Step 2: Update `map_think_level` signature and logic**

Replace the existing `map_think_level` (lines 149-159) with:

```rust
/// Map ThinkLevel to Gemini ThinkingConfig.
///
/// - Gemini 2.5 models → `thinkingBudget` (integer)
/// - All others (Gemini 3+) → `thinkingLevel` (enum)
fn map_think_level(level: &ThinkLevel, model: &str) -> Option<ThinkingConfig> {
    if *level == ThinkLevel::Off {
        return None;
    }
    // Gemini 2.5 models use thinkingBudget; all others use thinkingLevel
    let use_budget = model.contains("gemini-2.5");
    if use_budget {
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
    } else {
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
    }
}
```

- [ ] **Step 3: Update `build_request` to pass model to `map_think_level` and call schema cleaning**

In `build_request`, update the thinking config construction (around line 175):

Change:
```rust
let thinking_config = payload
    .think_level
    .as_ref()
    .and_then(Self::map_think_level)
    .map(|budget| ThinkingConfig {
        thinking_budget: Some(budget),
    });
```

To:
```rust
let thinking_config = payload
    .think_level
    .as_ref()
    .and_then(|level| Self::map_think_level(level, config.default_model()));
```

Update the tool building section (around lines 193-214) to call schema cleaning:

Change:
```rust
let tools = payload.tools.map(|tool_defs| {
    let declarations: Vec<GeminiFunctionDeclaration> = tool_defs
        .iter()
        .map(|td| {
            let mut params = td.parameters.clone();
            if let Some(obj) = params.as_object_mut() {
                obj.entry("type")
                    .or_insert_with(|| serde_json::json!("object"));
            }
            GeminiFunctionDeclaration {
                name: td.name.clone(),
                description: td.description.clone(),
                parameters: params,
            }
        })
        .collect();
```

To:
```rust
let tools = payload.tools.map(|tool_defs| {
    let declarations: Vec<GeminiFunctionDeclaration> = tool_defs
        .iter()
        .map(|td| {
            let mut params = td.parameters.clone();
            // Sanitize schema for Gemini's restricted OpenAPI subset
            clean_schema_for_gemini(&mut params);
            GeminiFunctionDeclaration {
                name: td.name.clone(),
                description: td.description.clone(),
                parameters: params,
            }
        })
        .collect();
```

(The `clean_schema_for_gemini` function already ensures `type: "object"` at top level, so the manual `entry("type")` insertion is no longer needed.)

- [ ] **Step 4: Update `convert_messages` to pass through tool call IDs**

In the `ToolResult` arm of `convert_messages` (around line 108), change:

```rust
UnifiedMessage::ToolResult {
    tool_name,
    content,
    ..
} => {
```

To:
```rust
UnifiedMessage::ToolResult {
    tool_call_id,
    tool_name,
    content,
    ..
} => {
```

And update the `GeminiFunctionResponse` construction:

```rust
function_response: crate::providers::gemini::GeminiFunctionResponse {
    name: tool_name.clone(),
    response: serde_json::json!({ "result": output }),
    id: Some(tool_call_id.clone()),
},
```

- [ ] **Step 5: Update `parse_gemini_sse_chunk` — native ID, thought markers, thinking tokens**

Replace the text delta section (around line 442-446):
```rust
if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
    if !text.is_empty() {
        out.push_back(Ok(ProviderDelta::TextDelta(text.to_string())));
    }
}
```

With:
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

Replace the function call ID generation (around lines 458-460):
```rust
// Generate synthetic ID (Gemini provides no call IDs)
let id = format!("gemini_fc_{}", *fc_counter);
*fc_counter += 1;
```

With:
```rust
// Prefer native ID (Gemini 3+), fallback to synthetic
let id = fc.get("id")
    .and_then(|v| v.as_str())
    .map(|s| s.to_string())
    .unwrap_or_else(|| {
        let synthetic = format!("gemini_fc_{}", *fc_counter);
        *fc_counter += 1;
        synthetic
    });
```

Update the usage section (around line 523) to include thinking tokens:
```rust
let usage_event = Ok(ProviderDelta::Usage(TokenUsage {
    input_tokens: input,
    output_tokens: output,
    cache_read_tokens: None,
    thinking_tokens: usage
        .get("thoughtsTokenCount")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32),
}));
```

Update the doc comment on `parse_gemini_sse_chunk` (around line 411) to reflect the new behavior:
```rust
/// Parse one Gemini SSE data JSON chunk and push [`ProviderDelta`] events into `out`.
///
/// - Text parts with `thought: true` emit `ThinkingDelta` instead of `TextDelta`
/// - Function calls prefer native `id` field (Gemini 3+), fallback to synthetic `gemini_fc_{n}`
/// - Usage includes `thoughtsTokenCount` when available
```

- [ ] **Step 6: Update existing tests and add new ones**

Update `test_map_think_level` (it now needs a model parameter):

```rust
#[test]
fn test_map_think_level_budget_mode() {
    // Gemini 2.5 → thinkingBudget
    let result = GeminiProtocol::map_think_level(&ThinkLevel::Medium, "gemini-2.5-flash");
    let config = result.unwrap();
    assert_eq!(config.thinking_budget, Some(2000));
    assert!(config.thinking_level.is_none());
    assert_eq!(config.include_thoughts, Some(true));
}

#[test]
fn test_map_think_level_level_mode() {
    // Gemini 3 → thinkingLevel
    let result = GeminiProtocol::map_think_level(&ThinkLevel::High, "gemini-3-pro");
    let config = result.unwrap();
    assert!(config.thinking_budget.is_none());
    assert_eq!(config.thinking_level.as_deref(), Some("HIGH"));
}

#[test]
fn test_map_think_level_off() {
    assert!(GeminiProtocol::map_think_level(&ThinkLevel::Off, "gemini-3-pro").is_none());
}

#[test]
fn test_map_think_level_xhigh_caps_to_high() {
    let result = GeminiProtocol::map_think_level(&ThinkLevel::XHigh, "gemini-3-pro");
    assert_eq!(result.unwrap().thinking_level.as_deref(), Some("HIGH"));
}
```

Add SSE parsing tests:

```rust
#[test]
fn test_parse_sse_thought_marker() {
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"candidates":[{"content":{"parts":[{"text":"thinking...","thought":true},{"text":"answer"}]},"finishReason":"STOP"}]}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);

    // First: ThinkingDelta
    assert!(matches!(out.pop_front().unwrap(), Ok(ProviderDelta::ThinkingDelta(t)) if t == "thinking..."));
    // Second: TextDelta
    assert!(matches!(out.pop_front().unwrap(), Ok(ProviderDelta::TextDelta(t)) if t == "answer"));
}

#[test]
fn test_parse_sse_native_tool_id() {
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"search","id":"native_123","args":{"q":"rust"}}}]},"finishReason":"FUNCTION_CALL"}]}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);

    match out.pop_front().unwrap() {
        Ok(ProviderDelta::ToolCallStart { id, name }) => {
            assert_eq!(id, "native_123");
            assert_eq!(name, "search");
        }
        other => panic!("Expected ToolCallStart, got {:?}", other),
    }
    // Counter should NOT have incremented (native ID used)
    assert_eq!(fc, 0);
}

#[test]
fn test_parse_sse_synthetic_tool_id_fallback() {
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"search","args":{"q":"rust"}}}]},"finishReason":"FUNCTION_CALL"}]}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);

    match out.pop_front().unwrap() {
        Ok(ProviderDelta::ToolCallStart { id, .. }) => {
            assert_eq!(id, "gemini_fc_0");
        }
        other => panic!("Expected ToolCallStart, got {:?}", other),
    }
    assert_eq!(fc, 1);
}

#[test]
fn test_parse_sse_thinking_tokens_in_usage() {
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"candidates":[{"content":{"parts":[{"text":"done"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5,"thoughtsTokenCount":100}}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);

    // Find Usage event
    let usage = out.iter().find_map(|d| match d {
        Ok(ProviderDelta::Usage(u)) => Some(u.clone()),
        _ => None,
    }).expect("Usage event not found");
    assert_eq!(usage.thinking_tokens, Some(100));
}
```

- [ ] **Step 7: Delete the old `test_map_think_level` test**

Remove the old test that calls `GeminiProtocol::map_think_level(&ThinkLevel::Off)` with one arg (it's replaced by the new tests above).

- [ ] **Step 8: Verify all tests pass**

Run: `cargo test -p alephcore --lib protocols::gemini -- --nocapture`
Expected: all PASS

- [ ] **Step 9: Commit**

```bash
git add src/providers/protocols/gemini.rs
git commit -m "gemini: upgrade protocol adapter — schema cleaning, native IDs, thought markers, dual thinking"
```

---

### Task 5: Anthropic Protocol Polish

**Files:**
- Modify: `src/providers/anthropic/types.rs`
- Modify: `src/providers/protocols/anthropic.rs`

- [ ] **Step 1: Update `ThinkingBlock` type**

In `src/providers/anthropic/types.rs`, replace:

```rust
/// Extended thinking configuration
#[derive(Debug, Serialize)]
pub struct ThinkingBlock {
    #[serde(rename = "type")]
    pub thinking_type: String,
    pub budget_tokens: u32,
}
```

With:

```rust
/// Extended thinking configuration.
///
/// - `thinking_type`: "enabled" | "disabled" | "adaptive"
/// - `budget_tokens`: Required for "enabled", optional for "adaptive"
/// - `display`: "summarized" (default) | "omitted"
#[derive(Debug, Serialize)]
pub struct ThinkingBlock {
    #[serde(rename = "type")]
    pub thinking_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
}
```

- [ ] **Step 2: Add `OutputConfig` type**

Add to `anthropic/types.rs` after `ThinkingBlock`:

```rust
/// Output configuration for controlling response quality and format.
///
/// Not yet wired to RequestPayload — forward-looking type definition.
#[derive(Debug, Serialize)]
pub struct OutputConfig {
    /// Output effort level: "low", "medium", "high", "max"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}
```

- [ ] **Step 3: Update `MessagesRequest` to include `output_config`**

Add field to `MessagesRequest`:

```rust
/// Output configuration (effort level, structured output format)
#[serde(skip_serializing_if = "Option::is_none")]
pub output_config: Option<OutputConfig>,
```

- [ ] **Step 4: Update the construction site in `anthropic.rs`**

In `src/providers/protocols/anthropic.rs`, update the `ThinkingBlock` construction (around line 273):

Change:
```rust
.map(|budget| ThinkingBlock {
    thinking_type: "enabled".to_string(),
    budget_tokens: budget,
});
```

To:
```rust
.map(|budget| ThinkingBlock {
    thinking_type: "enabled".to_string(),
    budget_tokens: Some(budget),
    display: None,
});
```

And update the `MessagesRequest` construction (around line 305) to add:
```rust
output_config: None,
```

- [ ] **Step 5: Update `TokenUsage` construction in anthropic.rs**

At line ~620, add `thinking_tokens: None` to the `TokenUsage` literal (if not already done in Task 1).

- [ ] **Step 6: Add serialization tests**

Add to the tests in `anthropic.rs`:

```rust
#[test]
fn test_thinking_block_enabled_serialization() {
    let block = ThinkingBlock {
        thinking_type: "enabled".to_string(),
        budget_tokens: Some(10000),
        display: None,
    };
    let json = serde_json::to_value(&block).unwrap();
    assert_eq!(json["type"], "enabled");
    assert_eq!(json["budget_tokens"], 10000);
    assert!(json.get("display").is_none());
}

#[test]
fn test_thinking_block_adaptive_serialization() {
    let block = ThinkingBlock {
        thinking_type: "adaptive".to_string(),
        budget_tokens: None,
        display: Some("summarized".to_string()),
    };
    let json = serde_json::to_value(&block).unwrap();
    assert_eq!(json["type"], "adaptive");
    assert!(json.get("budget_tokens").is_none());
    assert_eq!(json["display"], "summarized");
}
```

- [ ] **Step 7: Verify tests pass**

Run: `cargo test -p alephcore --lib protocols::anthropic -- --nocapture`
Expected: all PASS

- [ ] **Step 8: Commit**

```bash
git add src/providers/anthropic/types.rs src/providers/protocols/anthropic.rs
git commit -m "anthropic: extend ThinkingBlock (adaptive/display), add OutputConfig type"
```

---

### Task 6: OpenAI Responses API Polish

**Files:**
- Modify: `src/providers/responses/types.rs`
- Modify: `src/providers/responses/shared.rs`
- Modify: `src/providers/protocols/openai_responses.rs`

- [ ] **Step 1: Add reasoning `StreamEvent` variants**

In `src/providers/responses/types.rs`, add before the `Completed` variant (around line 305):

```rust
/// Reasoning summary part added (o-series models)
#[serde(rename = "response.reasoning_summary_part.added")]
ReasoningSummaryPartAdded {
    item_id: String,
    output_index: usize,
},

/// Reasoning summary text delta (streaming thinking content)
#[serde(rename = "response.reasoning_summary_text.delta")]
ReasoningSummaryTextDelta {
    delta: String,
    item_id: String,
    output_index: usize,
},

/// Reasoning summary text complete
#[serde(rename = "response.reasoning_summary_text.done")]
ReasoningSummaryTextDone {
    text: String,
    item_id: String,
    output_index: usize,
},

/// Reasoning summary part complete
#[serde(rename = "response.reasoning_summary_part.done")]
ReasoningSummaryPartDone {
    item_id: String,
    output_index: usize,
},
```

- [ ] **Step 2: Handle reasoning delta in stream parsing**

In `src/providers/protocols/openai_responses.rs`, in the `match event` block (around line 372), add a new arm before the `_ => {}` catch-all:

```rust
StreamEvent::ReasoningSummaryTextDelta { delta, .. } => {
    out.push_back(Ok(ProviderDelta::ThinkingDelta(delta)));
}
```

- [ ] **Step 3: Fix `build_reasoning` ThinkLevel completeness**

In `src/providers/responses/shared.rs`, update `build_reasoning` (around line 128):

Change:
```rust
pub fn build_reasoning(think_level: Option<ThinkLevel>) -> Option<ReasoningConfig> {
    match think_level {
        Some(ThinkLevel::Low) => Some(ReasoningConfig {
            effort: Some("low".to_string()),
            summary: Some("auto".to_string()),
        }),
        Some(ThinkLevel::Medium) => Some(ReasoningConfig {
            effort: Some("medium".to_string()),
            summary: Some("auto".to_string()),
        }),
        Some(ThinkLevel::High) => Some(ReasoningConfig {
            effort: Some("high".to_string()),
            summary: Some("auto".to_string()),
        }),
        _ => None,
    }
}
```

To:
```rust
pub fn build_reasoning(think_level: Option<ThinkLevel>) -> Option<ReasoningConfig> {
    match think_level {
        Some(ThinkLevel::Low) => Some(ReasoningConfig {
            effort: Some("low".to_string()),
            summary: Some("auto".to_string()),
        }),
        Some(ThinkLevel::Medium) => Some(ReasoningConfig {
            effort: Some("medium".to_string()),
            summary: Some("auto".to_string()),
        }),
        Some(ThinkLevel::High) | Some(ThinkLevel::XHigh) => Some(ReasoningConfig {
            effort: Some("high".to_string()),
            summary: Some("auto".to_string()),
        }),
        _ => None, // Off, Minimal → no reasoning config
    }
}
```

- [ ] **Step 4: Add `include` default for official endpoints**

In `src/providers/protocols/openai_responses.rs`, in `build_responses_request` (around line 162), change:

```rust
include: variant.include.clone(),
```

To:
```rust
include: variant.include.clone().or_else(|| {
    if official {
        Some(vec!["reasoning.encrypted_content".into()])
    } else {
        None
    }
}),
```

- [ ] **Step 5: Update `TokenUsage` construction**

In `openai_responses.rs` around line 435, add `thinking_tokens: None` (if not already done in Task 1).

- [ ] **Step 6: Add tests**

Add to `shared.rs` tests:

```rust
#[test]
fn test_build_reasoning_xhigh_maps_to_high() {
    let result = build_reasoning(Some(ThinkLevel::XHigh));
    assert_eq!(result.as_ref().unwrap().effort.as_deref(), Some("high"));
}

#[test]
fn test_build_reasoning_minimal_maps_to_none() {
    assert!(build_reasoning(Some(ThinkLevel::Minimal)).is_none());
}

#[test]
fn test_parse_sse_reasoning_delta() {
    let data = r#"{"type":"response.reasoning_summary_text.delta","delta":"thinking step","item_id":"rs_1","output_index":0}"#;
    let event = parse_sse_data(data);
    assert!(matches!(event, Some(StreamEvent::ReasoningSummaryTextDelta { delta, .. }) if delta == "thinking step"));
}
```

Add to `openai_responses.rs` tests:

```rust
#[test]
fn test_include_default_for_official_endpoint() {
    let msgs = [UnifiedMessage::user("hello")];
    let payload = RequestPayload::new(&msgs);
    let variant = ResponsesVariant::default();
    let config = ProviderConfig::test_config("o3-mini"); // no base_url = official

    let request = OpenAiResponsesProtocol::build_responses_request(
        &payload, "o3-mini", &variant, &config,
    );
    assert!(request.include.is_some());
    assert!(request.include.unwrap().contains(&"reasoning.encrypted_content".to_string()));
}

#[test]
fn test_include_none_for_third_party() {
    let msgs = [UnifiedMessage::user("hello")];
    let payload = RequestPayload::new(&msgs);
    let variant = ResponsesVariant::default();
    let mut config = ProviderConfig::test_config("o3-mini");
    config.base_url = Some("https://openrouter.ai/api/v1".to_string());

    let request = OpenAiResponsesProtocol::build_responses_request(
        &payload, "o3-mini", &variant, &config,
    );
    assert!(request.include.is_none());
}
```

- [ ] **Step 7: Verify tests pass**

Run: `cargo test -p alephcore --lib responses -- --nocapture && cargo test -p alephcore --lib protocols::openai_responses -- --nocapture`
Expected: all PASS

- [ ] **Step 8: Commit**

```bash
git add src/providers/responses/ src/providers/protocols/openai_responses.rs
git commit -m "responses: add reasoning stream events, fix ThinkLevel mapping, gate include on official endpoints"
```

---

### Task 7: Final Integration Verification

**Files:** None (verification only)

- [ ] **Step 1: Full compile check**

Run: `cargo check -p alephcore`
Expected: no errors

- [ ] **Step 2: Run all provider tests**

Run: `cargo test -p alephcore --lib providers -- --nocapture`
Expected: all PASS

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: no warnings

- [ ] **Step 4: Commit any clippy fixes if needed**

```bash
git add -A && git commit -m "providers: fix clippy warnings"
```
