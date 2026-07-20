# OpenAI Protocol Cycle 3 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire `seed`, `logprobs`/`top_logprobs`, flip `supports_response_format` on 8 endpoints, and route strict-mode JSON Schema through the existing `normalize_strict_schema` helper.

**Architecture:** Surgical extensions to Cycle 1/2's pattern: new `ProviderConfig` fields + capability bits + `PayloadPolicy::apply` strip-list defense + inline capability-gated wire injection in Chat and Responses adapters. Shared translation in `openai_common/`. No new abstractions, no trait changes.

**Tech Stack:** Rust 2021, `serde`, `serde_json`, `schemars`, existing crate `alephcore`.

**Spec:** [`docs/superpowers/specs/2026-05-12-openai-protocol-cycle3-output-controls-and-capability-flip-design.md`](../specs/2026-05-12-openai-protocol-cycle3-output-controls-and-capability-flip-design.md)

**Predecessor commits:**
- Cycle 1 ended `ea0b2a3f3`
- Cycle 2 ended `6facb6bde`
- Cycle 3 spec committed `493d8b641`

---

## File Map

| File | Role | T1 | T2 | T3 | T4 | T5 | T6 | T7 | T8 | T9 | T10 | T11 | T12 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `src/providers/protocols/openai_common/provider_policy.rs` | Capability bits + matrix + strip-list | ✓ | ✓ | ✓ |  |  |  |  |  |  |  |  |  |
| `src/config/types/provider.rs` | `ProviderConfig` new fields + 3 literal sites in tests/helper |  |  |  | ✓ |  |  |  |  |  |  |  |  |
| `src/providers/auth_profile_registry.rs` | `ProviderConfig` literal |  |  |  | ✓ |  |  |  |  |  |  |  |  |
| `src/gateway/provider_factory.rs` | 2 `ProviderConfig` literals |  |  |  | ✓ |  |  |  |  |  |  |  |  |
| `src/gateway/handlers/oauth.rs` | `ProviderConfig` literal |  |  |  | ✓ |  |  |  |  |  |  |  |  |
| `src/gateway/handlers/providers/handlers.rs` | `ProviderConfig` literal |  |  |  | ✓ |  |  |  |  |  |  |  |  |
| `src/gateway/handlers/providers/helpers.rs` | `ProviderConfig` literal |  |  |  | ✓ |  |  |  |  |  |  |  |  |
| `src/generation/providers/openai_tts/tests.rs` | `ProviderConfig` literal |  |  |  | ✓ |  |  |  |  |  |  |  |  |
| `src/providers/protocols/openai_common/response_format.rs` | JsonSchema degrade + strict normalize |  |  |  |  | ✓ | ✓ |  |  |  |  |  |  |
| `src/providers/protocols/openai_chat/adapter.rs` | Chat wire injection |  |  |  |  |  |  | ✓ | ✓ |  |  |  |  |
| `src/providers/protocols/openai_chat/tests.rs` | Chat wire tests |  |  |  |  |  |  | ✓ | ✓ |  |  |  |  |
| `src/providers/responses/types.rs` | `ResponsesRequest` new fields |  |  |  |  |  |  |  |  | ✓ |  |  |  |
| `src/providers/protocols/openai_responses/mod.rs` | Responses wire injection |  |  |  |  |  |  |  |  |  | ✓ | ✓ |  |
| `src/providers/protocols/openai_responses/tests.rs` | Responses wire tests |  |  |  |  |  |  |  |  |  | ✓ | ✓ |  |
| `CHANGELOG.md` | Unreleased entries |  |  |  |  |  |  |  |  |  |  |  | ✓ |

---

## Notes for Subagents

**Test harness:** Always run targeted tests, never `cargo test -p alephcore --lib` whole-suite — Cycle 2 confirmed it times out the agent harness. Use the narrow filter shown in each task.

**`normalize_strict_schema` real signature** (the spec called this `normalize_for_strict`; the actual function name in the codebase is `normalize_strict_schema`):
```rust
// src/providers/protocols/openai_common/openai_strict_schema.rs:227
pub fn normalize_strict_schema(schema: &mut Value, set_top_level_strict: bool) -> StrictResult
```
- It mutates `schema` in place (recursively injects `"additionalProperties": false` and copies properties keys into `required`).
- For `response_format` we pass `set_top_level_strict: false` because `strict: true` lives inside the `json_schema` block, not at the schema root.
- Returns `StrictResult` — for Cycle 3 we ignore the return value (best-effort normalization, same posture as tool definitions).

**`ProviderConfig` literal sites** (10 total need 3 new fields each): see file map above. Use `grep -n "response_format: None,"` to verify after each task.

**Pre-existing baseline failure** `test_apply_policy_strips_fields` is the known Cycle 2 baseline; not introduced or fixed by this cycle.

---

## Task 1: `supports_seed` capability bit

**Files:**
- Modify: `src/providers/protocols/openai_common/provider_policy.rs:49-68` (struct), `216-362` (resolve_capabilities), `93-137` (apply), `414-639` (tests)

- [ ] **Step 1: Write the failing tests**

Add at the bottom of the `#[cfg(test)] mod tests { ... }` block in `provider_policy.rs`:

```rust
#[test]
fn supports_seed_matrix_matches_cycle3_spec() {
    let truthy = [
        EndpointClass::OpenAiPublic,
        EndpointClass::OpenAiCodex,
        EndpointClass::AzureOpenAi,
        EndpointClass::OpenRouter,
        EndpointClass::DeepSeekNative,
        EndpointClass::GroqNative,
        EndpointClass::MistralPublic,
        EndpointClass::MoonshotNative,
        EndpointClass::CerebrasNative,
        EndpointClass::XAiNative,
    ];
    for class in truthy {
        assert!(
            resolve_capabilities(class).supports_seed,
            "{:?} should support seed",
            class
        );
    }
    let falsy = [
        EndpointClass::AnthropicPublic,
        EndpointClass::Local,
        EndpointClass::Custom,
    ];
    for class in falsy {
        assert!(
            !resolve_capabilities(class).supports_seed,
            "{:?} should NOT support seed",
            class
        );
    }
}

#[test]
fn apply_strips_seed_when_unsupported() {
    let policy = build_payload_policy(
        Some("http://localhost:8080"),
        "openai-chat",
        None,
    );
    let mut payload = serde_json::Map::new();
    payload.insert("seed".into(), serde_json::json!(42));
    payload.insert("model".into(), serde_json::Value::String("m".into()));

    policy.apply(&mut payload);

    assert!(payload.get("seed").is_none());
    assert!(payload.get("model").is_some());
}

#[test]
fn apply_keeps_seed_when_supported() {
    let policy = build_payload_policy(
        Some("https://api.openai.com"),
        "openai-chat",
        None,
    );
    let mut payload = serde_json::Map::new();
    payload.insert("seed".into(), serde_json::json!(42));

    policy.apply(&mut payload);

    assert_eq!(payload.get("seed"), Some(&serde_json::json!(42)));
}
```

- [ ] **Step 2: Run to verify fail**

```bash
cargo test -p alephcore --lib provider_policy::tests::supports_seed_matrix_matches_cycle3_spec provider_policy::tests::apply_strips_seed_when_unsupported provider_policy::tests::apply_keeps_seed_when_supported 2>&1 | tail -20
```
Expected: compilation error `no field 'supports_seed' on ProviderCapabilities`.

- [ ] **Step 3: Add the capability bit**

Modify `ProviderCapabilities` struct (currently at lines 47-68), inserting after `supports_response_format` (line 61):

```rust
/// Endpoint accepts the `seed` field on Chat/Responses requests
pub supports_seed: bool,
```

- [ ] **Step 4: Add field value to all 13 resolve_capabilities branches**

In `resolve_capabilities` (line 216 onward), add one line to every branch. The matrix:

| EndpointClass | supports_seed |
|---|---|
| OpenAiPublic | `true` |
| OpenAiCodex | `true` |
| AzureOpenAi | `true` |
| AnthropicPublic | `false` |
| DeepSeekNative | `true` |
| GroqNative | `true` |
| MistralPublic | `true` |
| MoonshotNative | `true` |
| CerebrasNative | `true` |
| XAiNative | `true` |
| OpenRouter | `true` |
| Local | `false` |
| Custom | `false` |

For each `ProviderCapabilities { ... }` literal, insert the new line right after `supports_response_format`. Example for the first branch:

```rust
EndpointClass::OpenAiPublic => ProviderCapabilities {
    supports_responses_store: true,
    supports_reasoning_effort: true,
    supports_prompt_cache: true,
    supports_service_tier: true,
    supports_strict_schema: true,
    supports_response_format: true,
    supports_seed: true,                  // NEW
    supports_server_compaction: true,
    requires_object_properties: false,
    context_window: Some(128_000),
},
```

- [ ] **Step 5: Add strip-list branch**

In `PayloadPolicy::apply` (around line 95-137), insert after the existing `response_format` strip (line 110-112):

```rust
if !self.capabilities.supports_seed {
    payload.remove("seed");
}
```

- [ ] **Step 6: Run targeted tests to verify pass**

```bash
cargo test -p alephcore --lib provider_policy::tests 2>&1 | tail -25
```
Expected: all `supports_seed_*` and `apply_*_seed_*` tests pass. Pre-existing `test_apply_policy_strips_fields` failure stays (baseline).

- [ ] **Step 7: cargo check the whole crate**

```bash
cargo check -p alephcore 2>&1 | tail -10
```
Expected: compiles. Note: `ProviderCapabilities` derives `Default` which auto-defaults new bool to `false`, so no callers break.

- [ ] **Step 8: Commit**

```bash
git add src/providers/protocols/openai_common/provider_policy.rs
git commit -m "providers: add supports_seed capability bit + strip-list

Adds the supports_seed bit to ProviderCapabilities, sets per-EndpointClass
values per Cycle 3 spec matrix (10 true, 3 false), and strips 'seed' from
the payload when the capability is absent. Follows the same defense-in-depth
pattern as Cycle 2's supports_response_format.

Refs spec §3.2."
```

---

## Task 2: `supports_logprobs` capability bit

**Files:**
- Modify: `src/providers/protocols/openai_common/provider_policy.rs` (struct, matrix, apply, tests)

- [ ] **Step 1: Write the failing tests**

Append to `#[cfg(test)] mod tests` in `provider_policy.rs`:

```rust
#[test]
fn supports_logprobs_matrix_matches_cycle3_spec() {
    let truthy = [
        EndpointClass::OpenAiPublic,
        EndpointClass::OpenAiCodex,
        EndpointClass::AzureOpenAi,
        EndpointClass::OpenRouter,
        EndpointClass::GroqNative,
        EndpointClass::CerebrasNative,
        EndpointClass::XAiNative,
    ];
    for class in truthy {
        assert!(
            resolve_capabilities(class).supports_logprobs,
            "{:?} should support logprobs",
            class
        );
    }
    let falsy = [
        EndpointClass::AnthropicPublic,
        EndpointClass::DeepSeekNative,
        EndpointClass::MistralPublic,
        EndpointClass::MoonshotNative,
        EndpointClass::Local,
        EndpointClass::Custom,
    ];
    for class in falsy {
        assert!(
            !resolve_capabilities(class).supports_logprobs,
            "{:?} should NOT support logprobs",
            class
        );
    }
}

#[test]
fn apply_strips_logprobs_when_unsupported() {
    let policy = build_payload_policy(
        Some("https://api.deepseek.com"),
        "openai-chat",
        None,
    );
    let mut payload = serde_json::Map::new();
    payload.insert("logprobs".into(), serde_json::json!(true));
    payload.insert("top_logprobs".into(), serde_json::json!(5));
    payload.insert("model".into(), serde_json::Value::String("dsk".into()));

    policy.apply(&mut payload);

    assert!(payload.get("logprobs").is_none());
    assert!(payload.get("top_logprobs").is_none());
    assert!(payload.get("model").is_some());
}

#[test]
fn apply_keeps_logprobs_when_supported() {
    let policy = build_payload_policy(
        Some("https://api.groq.com"),
        "openai-chat",
        None,
    );
    let mut payload = serde_json::Map::new();
    payload.insert("logprobs".into(), serde_json::json!(true));
    payload.insert("top_logprobs".into(), serde_json::json!(3));

    policy.apply(&mut payload);

    assert_eq!(payload.get("logprobs"), Some(&serde_json::json!(true)));
    assert_eq!(payload.get("top_logprobs"), Some(&serde_json::json!(3)));
}
```

- [ ] **Step 2: Run to verify fail**

```bash
cargo test -p alephcore --lib provider_policy::tests::supports_logprobs_matrix_matches_cycle3_spec 2>&1 | tail -10
```
Expected: compile error `no field 'supports_logprobs' on ProviderCapabilities`.

- [ ] **Step 3: Add the capability bit**

In `ProviderCapabilities`, insert after `supports_seed`:

```rust
/// Endpoint accepts `logprobs` / `top_logprobs` on Chat/Responses requests
pub supports_logprobs: bool,
```

- [ ] **Step 4: Add field value to all 13 resolve_capabilities branches**

Matrix:

| EndpointClass | supports_logprobs |
|---|---|
| OpenAiPublic | `true` |
| OpenAiCodex | `true` |
| AzureOpenAi | `true` |
| AnthropicPublic | `false` |
| DeepSeekNative | `false` |
| GroqNative | `true` |
| MistralPublic | `false` |
| MoonshotNative | `false` |
| CerebrasNative | `true` |
| XAiNative | `true` |
| OpenRouter | `true` |
| Local | `false` |
| Custom | `false` |

Insert each line after the new `supports_seed:` line.

- [ ] **Step 5: Add strip-list branch**

In `PayloadPolicy::apply`, insert after the Task-1 `seed` strip:

```rust
if !self.capabilities.supports_logprobs {
    payload.remove("logprobs");
    payload.remove("top_logprobs");
}
```

- [ ] **Step 6: Run targeted tests**

```bash
cargo test -p alephcore --lib provider_policy::tests 2>&1 | tail -25
```
Expected: all new `supports_logprobs_*` and `apply_*_logprobs_*` tests green; nothing else regresses.

- [ ] **Step 7: cargo check**

```bash
cargo check -p alephcore 2>&1 | tail -10
```
Expected: compiles.

- [ ] **Step 8: Commit**

```bash
git add src/providers/protocols/openai_common/provider_policy.rs
git commit -m "providers: add supports_logprobs capability bit + strip-list

Adds the supports_logprobs bit to ProviderCapabilities and strips both
'logprobs' and 'top_logprobs' when absent. Per-EndpointClass matrix per
Cycle 3 spec (7 true, 6 false — DeepSeek/Mistral/Moonshot left false
until vendor docs confirm).

Refs spec §3.2."
```

---

## Task 3: Flip `supports_response_format` on 8 endpoints

**Files:**
- Modify: `src/providers/protocols/openai_common/provider_policy.rs` (resolve_capabilities, tests)

- [ ] **Step 1: Write the failing tests**

Replace the existing `third_party_endpoints_do_not_support_response_format` test (currently at ~line 578-600) with a positive list, and add a negative list:

```rust
#[test]
fn cycle3_flipped_endpoints_support_response_format() {
    for class in [
        EndpointClass::AzureOpenAi,
        EndpointClass::OpenRouter,
        EndpointClass::DeepSeekNative,
        EndpointClass::GroqNative,
        EndpointClass::MistralPublic,
        EndpointClass::MoonshotNative,
        EndpointClass::CerebrasNative,
        EndpointClass::XAiNative,
    ] {
        assert!(
            resolve_capabilities(class).supports_response_format,
            "{:?} should support response_format after Cycle 3 flip",
            class
        );
    }
}

#[test]
fn anthropic_local_custom_still_skip_response_format() {
    for class in [
        EndpointClass::AnthropicPublic,
        EndpointClass::Local,
        EndpointClass::Custom,
    ] {
        assert!(
            !resolve_capabilities(class).supports_response_format,
            "{:?} should NOT support response_format",
            class
        );
    }
}
```

- [ ] **Step 2: Run to verify fail**

```bash
cargo test -p alephcore --lib provider_policy::tests::cycle3_flipped_endpoints_support_response_format provider_policy::tests::anthropic_local_custom_still_skip_response_format 2>&1 | tail -20
```
Expected: `cycle3_flipped_endpoints_support_response_format` fails (`AzureOpenAi should support response_format after Cycle 3 flip`).

- [ ] **Step 3: Flip the 8 endpoints**

In `resolve_capabilities`, change `supports_response_format: false` to `true` for each of:
- `EndpointClass::AzureOpenAi` (~line 246)
- `EndpointClass::DeepSeekNative` (~line 268)
- `EndpointClass::GroqNative` (~line 279)
- `EndpointClass::MistralPublic` (~line 290)
- `EndpointClass::MoonshotNative` (~line 301)
- `EndpointClass::CerebrasNative` (~line 312)
- `EndpointClass::XAiNative` (~line 323)
- `EndpointClass::OpenRouter` (~line 334)

Leave `AnthropicPublic`, `Local`, `Custom` at `false`.

- [ ] **Step 4: Remove the now-stale Cycle 2 test**

Delete the old `third_party_endpoints_do_not_support_response_format` test block — it asserts the pre-flip state and will now fail.

- [ ] **Step 5: Run targeted tests**

```bash
cargo test -p alephcore --lib provider_policy::tests 2>&1 | tail -25
```
Expected: both new tests green; `third_party_endpoints_do_not_support_response_format` no longer exists; other tests stable.

- [ ] **Step 6: Commit**

```bash
git add src/providers/protocols/openai_common/provider_policy.rs
git commit -m "providers: flip supports_response_format=true on 8 endpoints

Per Cycle 3 spec, eight OpenAI-compatible backends are now considered
response_format-capable: Azure, OpenRouter, DeepSeek, Groq, Mistral,
Moonshot, Cerebras, xAI. AnthropicPublic / Local / Custom remain false
(different protocol; conservative defaults).

JsonSchema variant will degrade to JsonObject on endpoints that don't
support strict schemas (handled in T5).

Refs spec §3.1."
```

---

## Task 4: `ProviderConfig` new fields + 10 literal sites

**Files:**
- Modify: `src/config/types/provider.rs:74-199` (struct), `246-274` (test_config helper), `296-356` (test literals)
- Modify: `src/providers/auth_profile_registry.rs:166-200`
- Modify: `src/gateway/provider_factory.rs:65-95`, `131-160`
- Modify: `src/gateway/handlers/oauth.rs:112-150`
- Modify: `src/gateway/handlers/providers/handlers.rs:420-435`
- Modify: `src/gateway/handlers/providers/helpers.rs:85-100`
- Modify: `src/generation/providers/openai_tts/tests.rs:360-375`

- [ ] **Step 1: Write the failing test**

In `src/config/types/provider.rs`, append to `#[cfg(test)] mod tests`:

```rust
#[test]
fn new_cycle3_fields_default_to_none() {
    let config = ProviderConfig::test_config("gpt-4o");
    assert!(config.seed.is_none());
    assert!(config.logprobs.is_none());
    assert!(config.top_logprobs.is_none());
}

#[test]
fn cycle3_fields_deserialize_from_toml() {
    let toml_str = r#"
        protocol = "openai"
        models = ["gpt-4o"]
        seed = 42
        logprobs = true
        top_logprobs = 5
    "#;
    let cfg: ProviderConfig = toml::from_str(toml_str).expect("valid TOML");
    assert_eq!(cfg.seed, Some(42));
    assert_eq!(cfg.logprobs, Some(true));
    assert_eq!(cfg.top_logprobs, Some(5));
}

#[test]
fn cycle3_fields_default_when_toml_omits_them() {
    let toml_str = r#"
        protocol = "openai"
        models = ["gpt-4o"]
    "#;
    let cfg: ProviderConfig = toml::from_str(toml_str).expect("valid TOML");
    assert!(cfg.seed.is_none());
    assert!(cfg.logprobs.is_none());
    assert!(cfg.top_logprobs.is_none());
}
```

- [ ] **Step 2: Run to verify fail**

```bash
cargo test -p alephcore --lib config::types::provider::tests::new_cycle3_fields_default_to_none 2>&1 | tail -10
```
Expected: compile error `no field 'seed' on ProviderConfig`.

- [ ] **Step 3: Add fields to `ProviderConfig` struct**

In `provider.rs` at line ~199 (after `parallel_tool_calls`), append:

```rust
    /// Deterministic sampling seed. None = server default.
    /// Capability-gated; silently dropped when endpoint doesn't support it.
    #[serde(default)]
    pub seed: Option<u64>,

    /// Whether to return per-token logprobs. None = no field emitted.
    /// Capability-gated; silently dropped when endpoint doesn't support it.
    #[serde(default)]
    pub logprobs: Option<bool>,

    /// Number of top alternative tokens per position (Chat range: 0..=20).
    /// Only emitted when `logprobs = Some(true)` and capability supports it.
    #[serde(default)]
    pub top_logprobs: Option<u8>,
```

- [ ] **Step 4: Update `test_config` helper**

In `ProviderConfig::test_config` (line 246), append three lines before the closing `}`:

```rust
            response_format: None,
            parallel_tool_calls: None,
            seed: None,             // NEW
            logprobs: None,         // NEW
            top_logprobs: None,     // NEW
```

(The two existing lines are already there; show context.)

- [ ] **Step 5: Update the 2 test literals in `provider.rs`**

Two literal `ProviderConfig { ... }` constructions live at lines 296-322 and 328-354. To each, before the closing `}`, append:

```rust
            seed: None,
            logprobs: None,
            top_logprobs: None,
```

- [ ] **Step 6: Update each of the 7 external literal sites**

Find each via:

```bash
grep -n "response_format: None," src/ -rln 2>/dev/null
```

You should find these files (already enumerated in the file map):
- `src/providers/auth_profile_registry.rs` (1 site near line 190)
- `src/gateway/provider_factory.rs` (2 sites near lines 89 and 155)
- `src/gateway/handlers/oauth.rs` (1 site near line 142)
- `src/gateway/handlers/providers/handlers.rs` (1 site near line 425)
- `src/gateway/handlers/providers/helpers.rs` (1 site near line 92)
- `src/generation/providers/openai_tts/tests.rs` (1 site near line 366)

For each, locate the `response_format: None,` line and insert these three lines directly after it:

```rust
        seed: None,
        logprobs: None,
        top_logprobs: None,
```

(Indentation should match local context — 8 spaces in most of these files.)

- [ ] **Step 7: cargo check**

```bash
cargo check -p alephcore 2>&1 | tail -10
```
Expected: compiles. If any error like `missing field 'seed' in initializer of ProviderConfig`, find the file and add the 3 None lines.

- [ ] **Step 8: Run the three new tests + provider.rs tests**

```bash
cargo test -p alephcore --lib config::types::provider::tests 2>&1 | tail -20
```
Expected: all 7 tests in module green (4 existing + 3 new).

- [ ] **Step 9: Commit**

```bash
git add src/config/types/provider.rs src/providers/auth_profile_registry.rs src/gateway/ src/generation/providers/openai_tts/tests.rs
git commit -m "config: add seed, logprobs, top_logprobs to ProviderConfig

Three new optional fields, all #[serde(default)]:
- seed: Option<u64>
- logprobs: Option<bool>
- top_logprobs: Option<u8>

Backward compatible: existing TOML configs (no fields set) deserialize
to None across the board. Updates 10 literal ProviderConfig construction
sites across config + gateway + auth profile + TTS test helpers.

Refs spec §2.1."
```

---

## Task 5: `response_format.rs` — JsonSchema → JsonObject degrade

**Files:**
- Modify: `src/providers/protocols/openai_common/response_format.rs:16-37` (`to_chat_response_format`), `41-50` (`to_responses_text_format`), `54-71` (`merge_text_format`), tests block

- [ ] **Step 1: Write the failing tests**

Append to `#[cfg(test)] mod tests` in `response_format.rs`:

```rust
// ─── Cycle 3: JsonSchema degrade on non-strict ───────────────────

#[test]
fn chat_json_schema_degrades_to_json_object_when_not_strict() {
    let v = to_chat_response_format(
        &ResponseFormat::JsonSchema {
            name: "thing".into(),
            schema: json!({"type": "object", "properties": {"x": {"type": "string"}}}),
        },
        false,
    )
    .unwrap();
    assert_eq!(v, json!({"type": "json_object"}));
}

#[test]
fn responses_json_schema_degrades_to_json_object_when_not_strict() {
    let fmt = ResponseFormat::JsonSchema {
        name: "thing".into(),
        schema: json!({"type": "object"}),
    };
    let result = to_responses_text_format(&fmt, false).unwrap();
    assert!(matches!(result, TextFormat::JsonObject));
}

#[test]
fn responses_json_schema_preserved_when_strict() {
    let schema = json!({"type": "object"});
    let fmt = ResponseFormat::JsonSchema {
        name: "n".into(),
        schema: schema.clone(),
    };
    let result = to_responses_text_format(&fmt, true).unwrap();
    match result {
        TextFormat::JsonSchema { name, .. } => assert_eq!(name, "n"),
        other => panic!("expected JsonSchema, got {:?}", other),
    }
}

#[test]
fn merge_text_format_degrades_json_schema_when_not_strict() {
    let result = merge_text_format(
        None,
        Some(&ResponseFormat::JsonSchema {
            name: "n".into(),
            schema: json!({"type": "object"}),
        }),
        false,
    )
    .unwrap();
    assert!(matches!(result.format, Some(TextFormat::JsonObject)));
}
```

- [ ] **Step 2: Run to verify fail**

```bash
cargo test -p alephcore --lib openai_common::response_format::tests::chat_json_schema_degrades_to_json_object_when_not_strict 2>&1 | tail -10
```
Expected: assertion failure (currently returns full json_schema even when `supports_strict=false`).

- [ ] **Step 3: Modify `to_chat_response_format`**

Replace the existing body (lines 16-37):

```rust
pub fn to_chat_response_format(
    fmt: &ResponseFormat,
    supports_strict: bool,
) -> Option<Value> {
    match fmt {
        ResponseFormat::Text => None,
        ResponseFormat::JsonObject => Some(json!({"type": "json_object"})),
        ResponseFormat::JsonSchema { name, schema } => {
            if !supports_strict {
                // Cycle 3: degrade to json_object on endpoints that don't
                // support strict schemas (most third-party OpenAI-compat backends)
                return Some(json!({"type": "json_object"}));
            }
            let mut inner = json!({
                "name": name,
                "schema": schema,
                "strict": true,
            });
            Some(json!({
                "type": "json_schema",
                "json_schema": inner,
            }))
        }
    }
}
```

(Note: `let mut inner` is now non-mutable conceptually — we'll need it mutable in Task 6 when normalization is added, so leave the binding `mut`.)

- [ ] **Step 4: Modify `to_responses_text_format` to take `supports_strict`**

Replace lines 41-50:

```rust
/// Build the Responses protocol's `text.format` typed value.
/// Returns `None` when `Text` (omit format slot inside TextConfig).
/// Degrades `JsonSchema` to `JsonObject` when `supports_strict` is false.
pub fn to_responses_text_format(
    fmt: &ResponseFormat,
    supports_strict: bool,
) -> Option<TextFormat> {
    match fmt {
        ResponseFormat::Text => None,
        ResponseFormat::JsonObject => Some(TextFormat::JsonObject),
        ResponseFormat::JsonSchema { name, schema } => {
            if !supports_strict {
                return Some(TextFormat::JsonObject);
            }
            Some(TextFormat::JsonSchema {
                name: name.clone(),
                schema: schema.clone(),
            })
        }
    }
}
```

- [ ] **Step 5: Modify `merge_text_format` signature**

Replace lines 54-71:

```rust
/// Merge an explicit `ResponseFormat` config into the variant's `TextConfig`.
/// Preserves variant's `verbosity` slot; overrides `format` slot only.
/// Honors capability gate: `supports_strict` controls the JsonSchema branch.
pub fn merge_text_format(
    base: Option<TextConfig>,
    fmt: Option<&ResponseFormat>,
    supports_strict: bool,
) -> Option<TextConfig> {
    match (base, fmt) {
        (existing, None) => existing,
        (Some(mut t), Some(f)) => {
            if let Some(rf) = to_responses_text_format(f, supports_strict) {
                t.format = Some(rf);
            }
            Some(t)
        }
        (None, Some(f)) => to_responses_text_format(f, supports_strict).map(|rf| TextConfig {
            format: Some(rf),
            verbosity: None,
        }),
    }
}
```

- [ ] **Step 6: Update existing tests that call the 2-arg signatures**

Three existing tests use `to_responses_text_format(&fmt)` or `merge_text_format(base, Some(&fmt))` — they need a trailing `true` argument:

```rust
// Existing test: responses_text_returns_none
// CHANGE FROM:
assert!(to_responses_text_format(&ResponseFormat::Text).is_none());
// CHANGE TO:
assert!(to_responses_text_format(&ResponseFormat::Text, true).is_none());

// Existing test: responses_json_object_returns_typed
// CHANGE TO:
assert!(matches!(
    to_responses_text_format(&ResponseFormat::JsonObject, true),
    Some(TextFormat::JsonObject)
));

// Existing test: responses_json_schema_preserves_name_and_schema
// CHANGE TO:
let result = to_responses_text_format(&ResponseFormat::JsonSchema {
    name: "config".into(),
    schema: schema.clone(),
}, true)
.unwrap();

// Existing merge_text_format tests — add `, true` before the closing `)`:
let result = merge_text_format(base.clone(), None, true);
let result = merge_text_format(base, Some(&ResponseFormat::JsonObject), true).unwrap();
let result = merge_text_format(None, Some(&ResponseFormat::JsonObject), true).unwrap();
assert!(merge_text_format(None, None, true).is_none());
```

- [ ] **Step 7: Update the one call site in `openai_responses/mod.rs`**

At line 174:

```rust
// FROM:
text: merge_text_format(variant.text.clone(), config.response_format.as_ref()),
// TO:
text: merge_text_format(
    variant.text.clone(),
    config.response_format.as_ref(),
    policy.capabilities.supports_strict_schema,
),
```

- [ ] **Step 8: cargo check**

```bash
cargo check -p alephcore 2>&1 | tail -15
```
Expected: compiles.

- [ ] **Step 9: Run targeted tests**

```bash
cargo test -p alephcore --lib openai_common::response_format 2>&1 | tail -25
```
Expected: all tests green including new degrade tests.

```bash
cargo test -p alephcore --lib openai_responses 2>&1 | tail -10
```
Expected: existing Responses tests still green (signature change propagated).

- [ ] **Step 10: Commit**

```bash
git add src/providers/protocols/openai_common/response_format.rs src/providers/protocols/openai_responses/mod.rs
git commit -m "response_format: degrade JsonSchema to JsonObject on non-strict endpoints

When supports_strict=false, both to_chat_response_format and
to_responses_text_format now return the JsonObject form for the
JsonSchema variant instead of forwarding an unsupported strict schema.

Plumbs supports_strict through to_responses_text_format and
merge_text_format (was previously absent on the Responses side).

Refs spec §4.1."
```

---

## Task 6: Strict-schema normalization for `response_format`

**Files:**
- Modify: `src/providers/protocols/openai_common/response_format.rs` (use the existing normalizer)

- [ ] **Step 1: Write the failing tests**

Append to `#[cfg(test)] mod tests` in `response_format.rs`:

```rust
// ─── Cycle 3: strict-schema normalization ────────────────────────

#[test]
fn chat_json_schema_strict_injects_additional_properties_false() {
    let v = to_chat_response_format(
        &ResponseFormat::JsonSchema {
            name: "n".into(),
            schema: json!({
                "type": "object",
                "properties": {"x": {"type": "string"}},
            }),
        },
        true,
    )
    .unwrap();
    let inner_schema = &v["json_schema"]["schema"];
    assert_eq!(inner_schema["additionalProperties"], json!(false));
}

#[test]
fn chat_json_schema_strict_injects_required_all_properties() {
    let v = to_chat_response_format(
        &ResponseFormat::JsonSchema {
            name: "n".into(),
            schema: json!({
                "type": "object",
                "properties": {"a": {"type": "string"}, "b": {"type": "number"}},
            }),
        },
        true,
    )
    .unwrap();
    let required = v["json_schema"]["schema"]["required"]
        .as_array()
        .expect("required should be an array");
    let names: std::collections::HashSet<_> = required
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(names.contains("a"));
    assert!(names.contains("b"));
}

#[test]
fn chat_json_schema_strict_recurses_into_nested_objects() {
    let v = to_chat_response_format(
        &ResponseFormat::JsonSchema {
            name: "n".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "outer": {
                        "type": "object",
                        "properties": {"inner": {"type": "string"}},
                    }
                },
            }),
        },
        true,
    )
    .unwrap();
    let outer = &v["json_schema"]["schema"]["properties"]["outer"];
    assert_eq!(outer["additionalProperties"], json!(false));
    assert!(
        outer["required"]
            .as_array()
            .map(|a| a.iter().any(|v| v.as_str() == Some("inner")))
            .unwrap_or(false),
        "nested object should have required[inner]"
    );
}

#[test]
fn chat_json_schema_strict_preserves_user_descriptions() {
    let v = to_chat_response_format(
        &ResponseFormat::JsonSchema {
            name: "n".into(),
            schema: json!({
                "type": "object",
                "properties": {"x": {"type": "string", "description": "user note"}},
            }),
        },
        true,
    )
    .unwrap();
    assert_eq!(
        v["json_schema"]["schema"]["properties"]["x"]["description"],
        json!("user note")
    );
}

#[test]
fn chat_json_schema_strict_does_not_set_top_level_strict_on_schema() {
    // strict: true belongs in json_schema block, not at schema root
    let v = to_chat_response_format(
        &ResponseFormat::JsonSchema {
            name: "n".into(),
            schema: json!({"type": "object", "properties": {"a": {"type": "string"}}}),
        },
        true,
    )
    .unwrap();
    assert_eq!(v["json_schema"]["strict"], json!(true));
    assert!(
        v["json_schema"]["schema"].get("strict").is_none(),
        "the inner schema should not carry a top-level 'strict' key"
    );
}
```

- [ ] **Step 2: Run to verify fail**

```bash
cargo test -p alephcore --lib openai_common::response_format::tests::chat_json_schema_strict_injects_additional_properties_false 2>&1 | tail -10
```
Expected: fails — the user schema is currently forwarded verbatim, so `additionalProperties` is missing.

- [ ] **Step 3: Add the use import + normalize at the top of `response_format.rs`**

At the existing import block (lines 6-8), add:

```rust
use crate::providers::protocols::openai_common::openai_strict_schema::normalize_strict_schema;
```

- [ ] **Step 4: Modify `to_chat_response_format` to normalize on strict**

Replace the strict branch from Task 5:

```rust
ResponseFormat::JsonSchema { name, schema } => {
    if !supports_strict {
        return Some(json!({"type": "json_object"}));
    }
    let mut normalized = schema.clone();
    // Cycle 3: run user schema through the same normalizer tool
    // definitions use (additionalProperties: false + required-all-properties).
    // set_top_level_strict=false because `strict: true` lives in the
    // json_schema envelope, not on the schema root.
    let _ = normalize_strict_schema(&mut normalized, false);
    Some(json!({
        "type": "json_schema",
        "json_schema": {
            "name": name,
            "schema": normalized,
            "strict": true,
        }
    }))
}
```

- [ ] **Step 5: Apply the same normalization to `to_responses_text_format`**

Replace the strict branch:

```rust
ResponseFormat::JsonSchema { name, schema } => {
    if !supports_strict {
        return Some(TextFormat::JsonObject);
    }
    let mut normalized = schema.clone();
    let _ = normalize_strict_schema(&mut normalized, false);
    Some(TextFormat::JsonSchema {
        name: name.clone(),
        schema: normalized,
    })
}
```

- [ ] **Step 6: Run targeted tests**

```bash
cargo test -p alephcore --lib openai_common::response_format 2>&1 | tail -30
```
Expected: all Cycle 3 normalize tests green; existing Cycle 2 tests still green.

- [ ] **Step 7: cargo check**

```bash
cargo check -p alephcore 2>&1 | tail -10
```
Expected: compiles.

- [ ] **Step 8: Commit**

```bash
git add src/providers/protocols/openai_common/response_format.rs
git commit -m "response_format: normalize JsonSchema via openai_strict_schema on strict endpoints

When supports_strict=true, response_format JsonSchema now runs through
normalize_strict_schema (same helper tool definitions use) before being
emitted. Injects additionalProperties:false and copies properties keys
into required, recursively. Preserves user descriptions/enums.

Set_top_level_strict=false because the 'strict: true' marker lives in
the json_schema envelope, not on the schema root.

Refs spec §4.2."
```

---

## Task 7: Chat adapter — `seed`

**Files:**
- Modify: `src/providers/protocols/openai_chat/adapter.rs:88-103` (insertion site after response_format, before tools)
- Modify: `src/providers/protocols/openai_chat/tests.rs` (append tests)

- [ ] **Step 1: Write the failing tests**

Append to `src/providers/protocols/openai_chat/tests.rs`:

```rust
// ─── Cycle 3: seed wiring ────────────────────────────────────────

#[test]
fn chat_seed_emitted_for_openai_public() {
    let protocol = super::OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.seed = Some(42);
    // base_url None → OpenAiPublic which supports_seed=true

    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert_eq!(body.get("seed"), Some(&serde_json::json!(42)));
}

#[test]
fn chat_seed_stripped_for_local_endpoint() {
    let protocol = super::OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("local-model");
    config.base_url = Some("http://localhost:8080".to_string());
    config.seed = Some(42);

    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert!(
        body.get("seed").is_none(),
        "seed must be absent on Local endpoint (supports_seed=false)"
    );
}

#[test]
fn chat_seed_omitted_when_config_none() {
    let protocol = super::OpenAiProtocol::new(reqwest::Client::new());
    let config = ProviderConfig::test_config("gpt-4o");
    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert!(body.get("seed").is_none());
}
```

- [ ] **Step 2: Run to verify fail**

```bash
cargo test -p alephcore --lib openai_chat::tests::chat_seed_emitted_for_openai_public 2>&1 | tail -10
```
Expected: assertion failure (seed not present in body).

- [ ] **Step 3: Inject seed in Chat adapter**

In `src/providers/protocols/openai_chat/adapter.rs`, after the response_format block (line 96-102), before the tools block (line 104):

```rust
        // seed: emit only when capability-enabled
        if let Some(seed) = config.seed {
            if policy.capabilities.supports_seed {
                body["seed"] = json!(seed);
            }
        }
```

- [ ] **Step 4: Run targeted tests**

```bash
cargo test -p alephcore --lib openai_chat::tests::chat_seed 2>&1 | tail -15
```
Expected: all three new `chat_seed_*` tests green.

```bash
cargo test -p alephcore --lib openai_chat 2>&1 | tail -10
```
Expected: full openai_chat module test count rises by 3; no regressions.

- [ ] **Step 5: Commit**

```bash
git add src/providers/protocols/openai_chat/adapter.rs src/providers/protocols/openai_chat/tests.rs
git commit -m "openai-chat: wire seed from ProviderConfig

Emits 'seed' in the Chat request body when both config.seed is Some
and the resolved endpoint capability supports it. Defense-in-depth via
PayloadPolicy::apply (strip when unsupported) was added in T1.

Refs spec §4.3."
```

---

## Task 8: Chat adapter — `logprobs` + `top_logprobs`

**Files:**
- Modify: `src/providers/protocols/openai_chat/adapter.rs` (insert after seed block from Task 7)
- Modify: `src/providers/protocols/openai_chat/tests.rs` (append tests)

- [ ] **Step 1: Write the failing tests**

Append to `src/providers/protocols/openai_chat/tests.rs`:

```rust
// ─── Cycle 3: logprobs / top_logprobs wiring ─────────────────────

#[test]
fn chat_logprobs_true_with_top_logprobs_emitted_for_openai_public() {
    let protocol = super::OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.logprobs = Some(true);
    config.top_logprobs = Some(5);

    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert_eq!(body.get("logprobs"), Some(&serde_json::json!(true)));
    assert_eq!(body.get("top_logprobs"), Some(&serde_json::json!(5)));
}

#[test]
fn chat_logprobs_false_omits_top_logprobs() {
    let protocol = super::OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.logprobs = Some(false);
    config.top_logprobs = Some(5); // should be ignored

    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert_eq!(body.get("logprobs"), Some(&serde_json::json!(false)));
    assert!(
        body.get("top_logprobs").is_none(),
        "top_logprobs must not be sent when logprobs=false"
    );
}

#[test]
fn chat_logprobs_stripped_for_deepseek() {
    let protocol = super::OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("deepseek-chat");
    config.base_url = Some("https://api.deepseek.com".to_string());
    config.logprobs = Some(true);
    config.top_logprobs = Some(3);

    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert!(body.get("logprobs").is_none());
    assert!(body.get("top_logprobs").is_none());
}

#[test]
fn chat_logprobs_omitted_when_config_none() {
    let protocol = super::OpenAiProtocol::new(reqwest::Client::new());
    let config = ProviderConfig::test_config("gpt-4o");
    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert!(body.get("logprobs").is_none());
    assert!(body.get("top_logprobs").is_none());
}
```

- [ ] **Step 2: Run to verify fail**

```bash
cargo test -p alephcore --lib openai_chat::tests::chat_logprobs_true_with_top_logprobs_emitted_for_openai_public 2>&1 | tail -10
```
Expected: assertion failure.

- [ ] **Step 3: Inject logprobs in Chat adapter**

In `src/providers/protocols/openai_chat/adapter.rs`, after the seed block from Task 7:

```rust
        // logprobs + top_logprobs: emit only when capability-enabled
        if let Some(want_logprobs) = config.logprobs {
            if policy.capabilities.supports_logprobs {
                body["logprobs"] = json!(want_logprobs);
                if want_logprobs {
                    if let Some(top_n) = config.top_logprobs {
                        body["top_logprobs"] = json!(top_n);
                    }
                }
            }
        }
```

- [ ] **Step 4: Run targeted tests**

```bash
cargo test -p alephcore --lib openai_chat::tests::chat_logprobs 2>&1 | tail -20
```
Expected: all four new `chat_logprobs_*` tests green.

```bash
cargo test -p alephcore --lib openai_chat 2>&1 | tail -10
```
Expected: count rises by 4; no regressions.

- [ ] **Step 5: Commit**

```bash
git add src/providers/protocols/openai_chat/adapter.rs src/providers/protocols/openai_chat/tests.rs
git commit -m "openai-chat: wire logprobs and top_logprobs from ProviderConfig

Emits 'logprobs' in the Chat request body when config.logprobs is Some
and the endpoint supports it; emits 'top_logprobs' only when
logprobs=true and top_logprobs is also Some. Strip-list in
PayloadPolicy::apply (from T2) provides defense-in-depth.

Refs spec §4.3."
```

---

## Task 9: `ResponsesRequest` struct fields

**Files:**
- Modify: `src/providers/responses/types.rs:13-49` (struct definition)

- [ ] **Step 1: Write the failing test**

Append to whatever test module exists for `responses::types`; if none, add at the bottom of the file:

```rust
#[cfg(test)]
mod cycle3_struct_tests {
    use super::*;

    #[test]
    fn responses_request_omits_seed_when_none() {
        let req = ResponsesRequest {
            model: "gpt-4o".into(),
            input: vec![],
            instructions: None,
            stream: true,
            store: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            text: None,
            max_output_tokens: None,
            include: None,
            previous_response_id: None,
            stop: None,
            context_management: None,
            seed: None,
            top_logprobs: None,
        };
        let v = serde_json::to_value(&req).unwrap();
        assert!(v.get("seed").is_none());
        assert!(v.get("top_logprobs").is_none());
    }

    #[test]
    fn responses_request_emits_seed_and_top_logprobs_when_some() {
        let req = ResponsesRequest {
            model: "gpt-4o".into(),
            input: vec![],
            instructions: None,
            stream: true,
            store: None,
            reasoning: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            text: None,
            max_output_tokens: None,
            include: None,
            previous_response_id: None,
            stop: None,
            context_management: None,
            seed: Some(42),
            top_logprobs: Some(3),
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v.get("seed"), Some(&serde_json::json!(42)));
        assert_eq!(v.get("top_logprobs"), Some(&serde_json::json!(3)));
    }
}
```

- [ ] **Step 2: Run to verify fail**

```bash
cargo test -p alephcore --lib providers::responses::types::cycle3_struct_tests 2>&1 | tail -10
```
Expected: compile error — missing fields `seed` and `top_logprobs`.

- [ ] **Step 3: Add the two fields**

In `src/providers/responses/types.rs`, inside `ResponsesRequest` (lines 13-49), append before the closing `}` of the struct:

```rust
    /// Deterministic sampling seed (Cycle 3, capability-gated)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// Number of top alternative tokens per position (Cycle 3, capability-gated)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_logprobs: Option<u32>,
```

- [ ] **Step 4: cargo check**

```bash
cargo check -p alephcore 2>&1 | tail -15
```
Expected: compile error in `openai_responses/mod.rs` at the `ResponsesRequest { ... }` literal — missing fields. That's expected; we patch it in Task 10. Skip this temporarily by allowing only the unit-test compile to pass:

Actually since cargo check will fail, this commit will fail too. To avoid a broken commit, do Task 10's struct-literal update inside this commit's scope:

Update `src/providers/protocols/openai_responses/mod.rs:164-189` `ResponsesRequest { ... }` literal — append before closing `}`:

```rust
            seed: None,         // T10 will wire from config
            top_logprobs: None, // T11 will wire from config
```

- [ ] **Step 5: Re-run cargo check**

```bash
cargo check -p alephcore 2>&1 | tail -10
```
Expected: compiles.

- [ ] **Step 6: Run targeted tests**

```bash
cargo test -p alephcore --lib providers::responses::types::cycle3_struct_tests 2>&1 | tail -10
```
Expected: both new tests green.

- [ ] **Step 7: Commit**

```bash
git add src/providers/responses/types.rs src/providers/protocols/openai_responses/mod.rs
git commit -m "responses: add seed and top_logprobs fields to ResponsesRequest

Two new optional fields with #[serde(skip_serializing_if = ...)] —
omitted from the wire when None. Adapter currently passes None for both;
T10 and T11 wire them from ProviderConfig.

Refs spec §4.4."
```

---

## Task 10: Responses adapter — `seed`

**Files:**
- Modify: `src/providers/protocols/openai_responses/mod.rs:164-189` (struct literal)
- Modify: `src/providers/protocols/openai_responses/tests.rs` (append tests)

- [ ] **Step 1: Write the failing tests**

Append to `src/providers/protocols/openai_responses/tests.rs`:

```rust
// ─── Cycle 3: seed wiring ────────────────────────────────────────

#[test]
fn responses_seed_emitted_for_openai_public() {
    let protocol = super::OpenAiResponsesProtocol::new(
        reqwest::Client::new(),
        standard_test_variant(),
    );
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.seed = Some(42);

    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);

    let req = super::OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-4o",
        &standard_test_variant(),
        &config,
    );
    assert_eq!(req.seed, Some(42));
}

#[test]
fn responses_seed_stripped_for_local() {
    let mut config = ProviderConfig::test_config("local");
    config.base_url = Some("http://localhost:11434".to_string());
    config.seed = Some(42);

    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);

    let req = super::OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "local",
        &standard_test_variant(),
        &config,
    );
    assert!(
        req.seed.is_none(),
        "seed must be None when endpoint capability does not support it"
    );
}

#[test]
fn responses_seed_none_when_config_unset() {
    let config = ProviderConfig::test_config("gpt-4o");
    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);

    let req = super::OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-4o",
        &standard_test_variant(),
        &config,
    );
    assert!(req.seed.is_none());
}
```

- [ ] **Step 2: Run to verify fail**

```bash
cargo test -p alephcore --lib openai_responses::tests::responses_seed_emitted_for_openai_public 2>&1 | tail -10
```
Expected: assertion failure (req.seed still None per Task 9 placeholder).

- [ ] **Step 3: Wire seed in `build_responses_request`**

In `src/providers/protocols/openai_responses/mod.rs`, locate the struct literal (lines 164-189) and replace the `seed: None,` placeholder added in T9:

```rust
            seed: config.seed.filter(|_| policy.capabilities.supports_seed),
```

(`Option::filter` is idiomatic: keeps Some(v) only when the capability predicate is true; collapses to None otherwise.)

- [ ] **Step 4: Run targeted tests**

```bash
cargo test -p alephcore --lib openai_responses::tests::responses_seed 2>&1 | tail -15
```
Expected: all three new tests green.

- [ ] **Step 5: Commit**

```bash
git add src/providers/protocols/openai_responses/mod.rs src/providers/protocols/openai_responses/tests.rs
git commit -m "openai-responses: wire seed from ProviderConfig

Emits ResponsesRequest.seed when both config.seed is Some and
the resolved endpoint capability supports it. Defense-in-depth via
PayloadPolicy::apply remains in place (T1).

Refs spec §4.4."
```

---

## Task 11: Responses adapter — `top_logprobs`

**Files:**
- Modify: `src/providers/protocols/openai_responses/mod.rs` (struct literal)
- Modify: `src/providers/protocols/openai_responses/tests.rs` (append tests)

**Wire semantics:** The Responses API has no `logprobs: bool` field — only `top_logprobs: u32`. We treat `ProviderConfig.logprobs == Some(true)` as the opt-in. When opted in, emit `top_logprobs` using `config.top_logprobs` (default 0 if unset). When `logprobs` is `None` or `Some(false)`, emit nothing.

- [ ] **Step 1: Write the failing tests**

Append to `src/providers/protocols/openai_responses/tests.rs`:

```rust
// ─── Cycle 3: top_logprobs wiring ────────────────────────────────

#[test]
fn responses_top_logprobs_emitted_when_logprobs_true() {
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.logprobs = Some(true);
    config.top_logprobs = Some(5);

    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);

    let req = super::OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-4o",
        &standard_test_variant(),
        &config,
    );
    assert_eq!(req.top_logprobs, Some(5));
}

#[test]
fn responses_top_logprobs_default_zero_when_logprobs_true_count_unset() {
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.logprobs = Some(true);
    // config.top_logprobs unset

    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);

    let req = super::OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-4o",
        &standard_test_variant(),
        &config,
    );
    assert_eq!(
        req.top_logprobs,
        Some(0),
        "opt-in with no count should emit 0 (Responses has no `logprobs: bool`)"
    );
}

#[test]
fn responses_top_logprobs_none_when_logprobs_false() {
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.logprobs = Some(false);
    config.top_logprobs = Some(5); // should be ignored

    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);

    let req = super::OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-4o",
        &standard_test_variant(),
        &config,
    );
    assert!(req.top_logprobs.is_none());
}

#[test]
fn responses_top_logprobs_stripped_for_deepseek() {
    let mut config = ProviderConfig::test_config("deepseek-reasoner");
    config.base_url = Some("https://api.deepseek.com".to_string());
    config.logprobs = Some(true);
    config.top_logprobs = Some(5);

    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);

    let req = super::OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "deepseek-reasoner",
        &standard_test_variant(),
        &config,
    );
    assert!(
        req.top_logprobs.is_none(),
        "DeepSeek has supports_logprobs=false; field must be stripped"
    );
}
```

- [ ] **Step 2: Run to verify fail**

```bash
cargo test -p alephcore --lib openai_responses::tests::responses_top_logprobs_emitted_when_logprobs_true 2>&1 | tail -10
```
Expected: assertion failure (req.top_logprobs still None).

- [ ] **Step 3: Wire top_logprobs in `build_responses_request`**

In the `ResponsesRequest { ... }` literal, replace the `top_logprobs: None,` placeholder from T9:

```rust
            top_logprobs: if config.logprobs == Some(true)
                && policy.capabilities.supports_logprobs
            {
                Some(config.top_logprobs.map(|n| n as u32).unwrap_or(0))
            } else {
                None
            },
```

- [ ] **Step 4: Run targeted tests**

```bash
cargo test -p alephcore --lib openai_responses::tests::responses_top_logprobs 2>&1 | tail -20
```
Expected: all four new tests green.

```bash
cargo test -p alephcore --lib openai_responses 2>&1 | tail -10
```
Expected: full Responses suite stable; new count visible.

- [ ] **Step 5: Commit**

```bash
git add src/providers/protocols/openai_responses/mod.rs src/providers/protocols/openai_responses/tests.rs
git commit -m "openai-responses: wire top_logprobs from ProviderConfig.logprobs

Responses API has no 'logprobs: bool' — only 'top_logprobs: u32' which
acts as the opt-in. When config.logprobs=Some(true) and the endpoint
supports logprobs, emit top_logprobs (default 0 if config.top_logprobs
unset). Otherwise omit the field entirely.

Refs spec §4.4."
```

---

## Task 12: CHANGELOG

**Files:**
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Read the current `[Unreleased]` block**

```bash
sed -n '1,40p' CHANGELOG.md
```

Note: per saved memory `feedback_changelog_english.md`, all CHANGELOG entries must be in English.

- [ ] **Step 2: Add Cycle 3 entries**

Locate the `## [Unreleased]` section and append (or merge into existing) three subsections — `### Added`, `### Fixed` (if applicable), `### Changed`:

```markdown
### Added
- **OpenAI provider** — `seed` (Option<u64>) is now wired into both
  Chat and Responses request bodies, capability-gated per endpoint
  (OpenAI/Codex/Azure/OpenRouter and 6 OpenAI-compat backends emit it;
  Local/Custom/AnthropicPublic strip it).
- **OpenAI provider** — `logprobs` (Option<bool>) and `top_logprobs`
  (Option<u8>) are now wired into the Chat request body and surfaced
  through `top_logprobs` on Responses. Capability-gated; emitted only on
  endpoints that document support. Response-side parsing remains future
  work (Cycle 4 candidate).
- **OpenAI provider** — 8 endpoints flipped to `supports_response_format
  = true`: Azure, OpenRouter, DeepSeek, Groq, Mistral, Moonshot, Cerebras,
  xAI. JsonSchema variants degrade to `{type: "json_object"}` on
  endpoints that do not support strict schemas.

### Changed
- **OpenAI provider** — `response_format` JSON Schemas now run through
  the same `normalize_strict_schema` helper that tool definitions use,
  injecting `additionalProperties: false` and copying property keys into
  `required` recursively. This brings response_format strict-mode parity
  with tool-definition strict mode.
```

- [ ] **Step 3: Confirm with grep**

```bash
grep -c "Cycle 3\|seed\|logprobs\|response_format" CHANGELOG.md | head
```
Expected: at least 4 matching lines under `[Unreleased]`.

- [ ] **Step 4: Commit**

```bash
git add CHANGELOG.md
git commit -m "changelog: document Cycle 3 — seed, logprobs, capability flip, strict normalization"
```

---

## Final Verification

After all 12 tasks land:

- [ ] **Targeted regression sweep**

```bash
cargo test -p alephcore --lib openai_chat 2>&1 | tail -5
cargo test -p alephcore --lib openai_responses 2>&1 | tail -5
cargo test -p alephcore --lib openai_common 2>&1 | tail -5
cargo test -p alephcore --lib config::types::provider 2>&1 | tail -5
cargo test -p alephcore --lib providers::responses::types 2>&1 | tail -5
```
Expected:
- Chat: 57 → 71+ passing
- Responses: 53 → 63+ passing
- openai_common: existing + 8 new
- config::types::provider: existing + 3 new
- providers::responses::types: existing + 2 new
- Pre-existing baseline failure `test_apply_policy_strips_fields` unchanged.

- [ ] **Clippy gate**

```bash
cargo clippy -p alephcore --lib -- -D warnings 2>&1 | tail -30
```
Expected: number of warnings matches Cycle 2 baseline (9 pre-existing). Zero new.

- [ ] **CHANGELOG sanity**

```bash
grep -A20 "^## \[Unreleased\]" CHANGELOG.md | head -30
```
Expected: shows the Cycle 3 entries; entries are in English.

- [ ] **Shipping manifest**

```bash
git log --oneline 493d8b641..HEAD
```
Expected: 12 commits, plus the spec commit `493d8b641` and this plan commit (if separately committed).

---

## Out-of-Scope Reminders

This cycle does NOT include:
- Anthropic protocol parity (Cycle 4)
- `logprobs` response-side parsing (SSE deltas, choices[*].logprobs surfacing)
- Per-model gating (e.g., o1/o3 reject logprobs even on OpenAi endpoint)
- User-level `ProviderConfig` capability overrides
- Strict-schema validation that errors out on bad schemas (only normalization is in scope)

Per spec §9, any of these can become Cycle 4 candidates after this cycle ships.
