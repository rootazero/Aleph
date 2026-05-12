# Anthropic Protocol Cycle 4 — Parity & Capability Matrix Design

**Date:** 2026-05-12
**Status:** Approved (pending implementation)
**Cycle:** 4 of the provider-side protocol optimization series
**Predecessor:** [`2026-05-12-openai-protocol-cycle3-output-controls-and-capability-flip-design.md`](./2026-05-12-openai-protocol-cycle3-output-controls-and-capability-flip-design.md) (Cycle 3, shipped 2026-05-12)
**Successor:** Cycle 5+ may add Bedrock / Vertex / Kimi named variants if real divergence emerges

---

## 1. Scope & Goals

Anthropic-protocol parity with the OpenAI cycles. Wire the `ProviderConfig` fields that Anthropic's Messages API supports but Aleph's adapter ignores, plus a parallel capability matrix so feature gating doesn't depend on hostname checks scattered across `adapter.rs`.

### Four-Bundle Manifest

| Bundle | Feature | Class | Files touched |
|---|---|---|---|
| **C4-A** | Wire `top_p` / `top_k` / `stop_sequences` / `service_tier` from `ProviderConfig` into `MessagesRequest` | Field parity | `src/providers/anthropic/types.rs`, `src/providers/protocols/anthropic/adapter.rs` |
| **C4-B** | New `AnthropicEndpointClass` + `AnthropicCapabilities` + `AnthropicPolicy` sibling module | New capability layer | `src/providers/protocols/anthropic/provider_policy.rs` (new) |
| **C4-C** | New `effort: Option<String>` on `ProviderConfig`, wired to `MessagesRequest.output_config.effort` | New Anthropic-only feature | `src/config/types/provider.rs`, `src/providers/protocols/anthropic/adapter.rs` |
| **C4-D** | New `metadata_user_id: Option<String>` on `ProviderConfig`, wired to `MessagesRequest.metadata.user_id` | New Anthropic-only feature | `src/config/types/provider.rs`, `src/providers/anthropic/types.rs`, `src/providers/protocols/anthropic/adapter.rs` |

### Non-Goals (Out-of-Scope This Cycle)

- `test_apply_policy_strips_fields` fix — pre-existing OpenAI-protocol bug (DeepSeekNative chat path doesn't strip `store`). Confirmed unrelated to Anthropic. Belongs in a standalone OpenAI patch.
- Additional `AnthropicEndpointClass` variants (Bedrock / Vertex / Kimi / OpenRouter). Initial enum is two-variant (Official + Custom); expand when real feature-profile divergence emerges.
- Per-tool `cache_control` breakpoints (C3 from the brainstorm — deferred to Cycle 5+).
- Configurable beta-header array (C4 from the brainstorm — deferred to Cycle 5+).
- User-level capability override fields on `ProviderConfig` — explicitly forbidden per [`feedback_no_user_capability_override.md`](../../../../.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/feedback_no_user_capability_override.md). Capability bits flow one way: `base_url → AnthropicEndpointClass → AnthropicCapabilities`.
- Anthropic OAuth-token detection / `service_tier` auto-downgrade (openclaw pattern). Aleph doesn't use OAuth-style Anthropic tokens today; YAGNI.
- `reasoning_effort` symmetric work on the OpenAI side. Cycle 4 is Anthropic-scoped.
- Consolidation of OpenAI vs Anthropic `stop_sequences` parsers. Each protocol keeps its own CSV splitter; refactor is a separate concern.
- `MessagesRequest` field renames / removals. Cycle 4 only adds.

### Architectural Alignment

- **R3 Core Minimalism** — New module is ~250 lines, no third-party crates added.
- **R4 I/O-Only Interfaces** — Capability gating happens at protocol adapter boundary; channels and gateways untouched.
- **R7 LLM Sovereignty** — All new fields are config-driven wire shape; zero rule-engine, zero intent-detection logic. Capability matrix is hostname classification, not reasoning.
- **R10 Thin Harness** — Pure surgical wiring; one new sibling module, no trait churn.
- **P1/P2 Coupling/Cohesion** — Anthropic capability bits live in their own struct, not mixed with OpenAI's `ProviderCapabilities`. High cohesion within each protocol family.
- **P4 Dependency Inversion** — `AnthropicPolicy::apply(&mut serde_json::Value)` is the single mutation point; `build_request` depends on the policy abstraction, not on hostname strings.

### Single Source of Truth (Capability Bits)

Capability resolution follows the OpenAI Cycle 3 discipline verbatim:

```
config.base_url
   ↓ detect_anthropic_endpoint_class(...)
AnthropicEndpointClass { Official | Custom }
   ↓ resolve_anthropic_capabilities(class)
AnthropicCapabilities { 7-bit feature profile }
   ↓ build_anthropic_policy(config.base_url)
AnthropicPolicy { class, capabilities }
   ↓ policy.apply(&mut body)
final wire body
```

No `Option<bool>` override field on `ProviderConfig`. If a third-party Anthropic-compatible endpoint turns out to support an Official-only feature, the right fix is a new `AnthropicEndpointClass` variant (or a more granular inference rule), not a config override.

---

## 2. Module Layout

```
src/providers/protocols/anthropic/
├── mod.rs              (existing; re-export AnthropicPolicy types)
├── adapter.rs          (existing; build_request consumes policy)
├── proto_impl.rs       (existing; no changes)
├── sse.rs              (existing; no changes)
└── provider_policy.rs  (NEW, ~250 lines including tests)
```

**Sibling-module rationale**: Anthropic and OpenAI are different wire protocols. Mixing `supports_logprobs` next to `supports_anthropic_metadata_user_id` in one struct would violate P2 (high cohesion). The `openai_common/` directory exists for the same reason — protocol-family colocation.

**Does not touch**: existing OpenAI `EndpointClass::AnthropicPublic` variant. That variant remains for the (unlikely) case of an OpenAI-protocol provider pointing at `api.anthropic.com`. Cycle 4 introduces an Anthropic-protocol-specific classifier independent of that variant.

---

## 3. New Types

### 3.1 `AnthropicEndpointClass`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnthropicEndpointClass {
    /// Official Anthropic API: host == api.anthropic.com
    Official,
    /// Everything else: kimi-for-coding, MiniMax-anthropic-mode, Bedrock,
    /// Vertex, OpenRouter-anthropic, Anyscale, etc.
    Custom,
}
```

### 3.2 `AnthropicCapabilities`

```rust
#[derive(Debug, Clone, Default)]
pub struct AnthropicCapabilities {
    /// `cache_control` breakpoints in `system` / `messages`.
    /// Gates `effective_cache_retention` injection.
    pub supports_cache_control: bool,
    /// `service_tier` field ("auto" / "standard_only").
    pub supports_service_tier: bool,
    /// `metadata.user_id` field.
    pub supports_metadata_user_id: bool,
    /// `output_config.effort` field ("low"/"medium"/"high"/"max").
    pub supports_output_config_effort: bool,
    /// `top_k` sampling field.
    pub supports_top_k: bool,
    /// `top_p` sampling field.
    pub supports_top_p: bool,
    /// `stop_sequences` field.
    pub supports_stop_sequences: bool,
}
```

### 3.3 Capability Matrix

| Bit | `Official` | `Custom` | Rationale |
|---|---|---|---|
| `supports_cache_control` | ✅ | ❌ | Third-party gateways routinely reject `cache_control`. Mirrors today's host-gated default in `effective_cache_retention`. |
| `supports_service_tier` | ✅ | ❌ | Anthropic-org billing field; rejected by every known third-party. |
| `supports_metadata_user_id` | ✅ | ❌ | Anthropic-org abuse-detection field; rejected by third-parties. |
| `supports_output_config_effort` | ✅ | ❌ | Anthropic-org feature (analog of OpenAI reasoning_effort). |
| `supports_top_k` | ✅ | ✅ | Core Anthropic-protocol sampling field; universally supported. |
| `supports_top_p` | ✅ | ✅ | Core sampling field. |
| `supports_stop_sequences` | ✅ | ✅ | Core protocol field. |

### 3.4 `AnthropicPolicy`

```rust
#[derive(Debug, Clone)]
pub struct AnthropicPolicy {
    pub class: AnthropicEndpointClass,
    pub capabilities: AnthropicCapabilities,
}

impl AnthropicPolicy {
    /// Mutate the serialized request body to strip fields whose
    /// capability bit is `false`. Called once at the end of `build_request`,
    /// after `serde_json::to_value(&request_body)`.
    pub fn apply(&self, body: &mut serde_json::Value) {
        let Some(obj) = body.as_object_mut() else { return };
        let caps = &self.capabilities;
        if !caps.supports_service_tier         { obj.remove("service_tier"); }
        if !caps.supports_metadata_user_id     { strip_metadata_user_id(obj); }
        if !caps.supports_output_config_effort { strip_output_config_effort(obj); }
        if !caps.supports_top_k                { obj.remove("top_k"); }
        if !caps.supports_top_p                { obj.remove("top_p"); }
        if !caps.supports_stop_sequences       { obj.remove("stop_sequences"); }
        // `cache_control` is gated at injection time, not stripped post-hoc.
    }
}
```

`strip_metadata_user_id` / `strip_output_config_effort` are private helpers that walk one level into `metadata` / `output_config` to remove the nested key. If the resulting nested object is empty, the whole nested object is removed.

### 3.5 Endpoint Detection

```rust
pub fn detect_anthropic_endpoint_class(base_url: Option<&str>) -> AnthropicEndpointClass {
    let url = match base_url {
        None | Some("") => return AnthropicEndpointClass::Official,
        Some(u) => u,
    };
    let host = extract_hostname(url).unwrap_or_default().to_lowercase();
    if host == "api.anthropic.com" {
        AnthropicEndpointClass::Official
    } else {
        AnthropicEndpointClass::Custom
    }
}
```

`extract_hostname` is a small local helper (or shared with `openai_common` via a `pub(crate)` re-export — choice deferred to implementation).

### 3.6 Policy Builder

```rust
pub fn build_anthropic_policy(base_url: Option<&str>) -> AnthropicPolicy {
    let class = detect_anthropic_endpoint_class(base_url);
    let capabilities = resolve_anthropic_capabilities(class);
    AnthropicPolicy { class, capabilities }
}
```

---

## 4. `ProviderConfig` New Fields

Two new fields on `pub struct ProviderConfig` in `src/config/types/provider.rs`:

```rust
/// Anthropic `metadata.user_id`. Opaque string passed to Anthropic's
/// abuse-detection / rate-limit-bucketing system.
/// Capability-gated; silently dropped on non-Official Anthropic endpoints.
#[serde(default)]
pub metadata_user_id: Option<String>,

/// Anthropic `output_config.effort`. Maps to `output_config.effort` field
/// on MessagesRequest. Accepted values: "low", "medium", "high", "max".
/// Capability-gated; silently dropped on non-Official Anthropic endpoints.
#[serde(default)]
pub effort: Option<String>,
```

Placement: after the Cycle 3 OpenAI block in the struct definition, grouped under a `// Anthropic-specific parameters` comment marker.

### 4.1 Existing Fields Cycle 4 Newly Wires

| `ProviderConfig` field | Currently | After Cycle 4 |
|---|---|---|
| `top_p: Option<f32>` | Unused on Anthropic adapter | Wired to `MessagesRequest.top_p` (capability-gated) |
| `top_k: Option<u32>` | Unused on Anthropic adapter | Wired to `MessagesRequest.top_k` (capability-gated) |
| `stop_sequences: Option<String>` | Unused on Anthropic adapter | Parsed (CSV) + wired to `MessagesRequest.stop_sequences` (capability-gated) |
| `service_tier: Option<String>` | Hardcoded `None` in `adapter.rs:238` | Wired to `MessagesRequest.service_tier` (capability-gated) |

### 4.2 `test_config()` + Struct-Literal Updates

`provider.rs:261` `test_config()` adds:
```rust
metadata_user_id: None,
effort: None,
```

Four other struct-literal `ProviderConfig` instances in the test module (`provider.rs:314-343`, `349-378`) get the same two `None` entries.

### 4.3 TOML Deserialization Test

```rust
#[test]
fn cycle4_anthropic_fields_deserialize_from_toml() {
    let toml_str = r#"
        protocol = "anthropic"
        models = ["claude-3-5-sonnet"]
        metadata_user_id = "u_42"
        effort = "high"
    "#;
    let cfg: ProviderConfig = toml::from_str(toml_str).expect("valid TOML");
    assert_eq!(cfg.metadata_user_id, Some("u_42".to_string()));
    assert_eq!(cfg.effort, Some("high".to_string()));
}

#[test]
fn cycle4_anthropic_fields_default_when_toml_omits_them() {
    let toml_str = r#"
        protocol = "anthropic"
        models = ["claude-3-5-sonnet"]
    "#;
    let cfg: ProviderConfig = toml::from_str(toml_str).expect("valid TOML");
    assert!(cfg.metadata_user_id.is_none());
    assert!(cfg.effort.is_none());
}
```

---

## 5. `MessagesRequest` Wire Struct Evolution

`src/providers/anthropic/types.rs` grows by 4 fields + 1 new struct. All `Option<...>` with `#[serde(skip_serializing_if = "Option::is_none")]`.

```rust
#[derive(Debug, Serialize)]
pub struct MessagesRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<Vec<SystemBlock>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,                  // NEW
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,                  // NEW
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>, // NEW
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<AnthropicTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,          // NEW
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<OutputConfig>,
}

/// Anthropic abuse-detection / rate-limit metadata.
/// Spec: https://docs.anthropic.com/en/api/messages#body-metadata
#[derive(Debug, Serialize)]
pub struct Metadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}
```

`OutputConfig` already exists at `types.rs:86-90` with `effort: Option<String>`. No change to that struct.

**Field order rationale**: matches Anthropic's Messages API reference top-to-bottom — sampling fields cluster after `temperature`, metadata fields cluster near `service_tier`, `output_config` stays at the bottom.

**Wire shape verification test** (T4): serialize a fully-populated `MessagesRequest` (all Option fields = `Some(...)`) and `assert_eq!` against an expected JSON value with the exact nesting Anthropic expects.

---

## 6. `build_request` Wiring

`src/providers/protocols/anthropic/adapter.rs:147` `build_request` gets the following changes:

```rust
// At top of function (after stream_idle_timeout_secs.store):
let policy = build_anthropic_policy(config.base_url.as_deref());

// Field construction (NEW lines, added before MessagesRequest literal):
let top_p          = config.top_p;
let top_k          = config.top_k;
let stop_sequences = config.stop_sequences.as_deref()
                          .map(parse_stop_sequences)
                          .filter(|v| !v.is_empty());
let service_tier   = config.service_tier.clone();   // was: hardcoded None
let metadata       = config.metadata_user_id.as_ref()
                          .map(|uid| Metadata { user_id: Some(uid.clone()) });
let output_config  = config.effort.as_ref()
                          .map(|e| OutputConfig { effort: Some(e.clone()) });

// MessagesRequest literal updated:
let request_body = MessagesRequest {
    model: actual_model.to_string(),
    messages,
    max_tokens,
    system,
    temperature,
    top_p, top_k, stop_sequences,    // NEW
    stream: Some(true),
    thinking,
    tools,
    service_tier,                    // NEW (un-hardcoded)
    metadata,                        // NEW
    output_config,                   // NEW
};

// ... existing serde_json::to_value + tool_choice handling unchanged ...

// REFACTORED: cache_control injection gated by policy
let extended_cache_ttl = if policy.capabilities.supports_cache_control {
    let retention = effective_cache_retention(config, &endpoint);
    let ext = matches!(retention, CacheRetention::Long);
    if retention != CacheRetention::Off {
        let cc = CacheControl::Ephemeral {
            ttl: if ext { Some(EphemeralTtl::OneHour) } else { None },
        };
        inject_cache_control_into_system_array(&mut body, cc);
        inject_cache_control_into_last_user_message(&mut body, cc);
    }
    ext
} else {
    false
};

// NEW: single capability gate at the end
policy.apply(&mut body);

Ok(self.client.post(&endpoint)
    .header("x-api-key", api_key)
    .header("anthropic-version", ANTHROPIC_VERSION)
    .header("anthropic-beta",
        Self::build_beta_headers(actual_model, Some(api_key), extended_cache_ttl))
    .header("Content-Type", "application/json")
    .json(&body))
```

### 6.1 `parse_stop_sequences` Helper

```rust
fn parse_stop_sequences(csv: &str) -> Vec<String> {
    csv.split(',')
       .map(str::trim)
       .filter(|s| !s.is_empty())
       .map(String::from)
       .collect()
}
```

Lives in `adapter.rs`. The empty `Vec::new()` case is filtered out by the caller (`.filter(|v| !v.is_empty())`), so the field never appears as `[]` on the wire.

### 6.2 `effective_cache_retention` Simplification

Before (`adapter.rs:32`):
```rust
fn effective_cache_retention(config: &ProviderConfig, base_url: &str) -> CacheRetention {
    match config.cache_retention {
        Some(r) => r,
        None => {
            let is_official = base_url.contains("api.anthropic.com");
            if is_official { CacheRetention::Short } else { CacheRetention::Off }
        }
    }
}
```

After:
```rust
fn effective_cache_retention(config: &ProviderConfig, _endpoint: &str) -> CacheRetention {
    match config.cache_retention {
        Some(r) => r,
        None => CacheRetention::Short,   // host gate now lives in policy
    }
}
```

The `_endpoint` parameter stays in the signature to avoid touching the 4 existing call sites and tests. Host-level gating moves to `policy.capabilities.supports_cache_control` (which wraps the whole injection block in `build_request`). The explicit `cache_retention = Long` warning on non-official hosts (`adapter.rs:43`) is preserved.

---

## 7. Testing Strategy

Continues the Cycle 1-3 **subagent-driven TDD** pattern: RED → GREEN per task, two-stage review (spec → quality), one commit per task.

### 7.1 Unit Tests (in `anthropic/provider_policy.rs`)

**`detect_anthropic_endpoint_class`** (5 tests):
- `Official` ← `Some("https://api.anthropic.com/v1/messages")`
- `Official` ← `None` (default)
- `Official` ← `Some("")` (empty)
- `Custom` ← `Some("https://kimi-for-coding.example.com/v1/messages")`
- `Custom` ← URL that fails parsing

**`resolve_anthropic_capabilities`** (2 tests):
- Official: 7 bits all `true`
- Custom: 4 Anthropic-only bits `false`, 3 protocol-standard bits `true`

**`AnthropicPolicy::apply`** (8 matrix tests):
- Custom + body has `service_tier="auto"` → field removed
- Custom + body has `metadata.user_id="u"` → nested key removed; empty `metadata` object also removed
- Custom + body has `output_config.effort="high"` → nested key removed; empty `output_config` also removed
- Custom + body has `top_k=40` → field NOT removed (Custom supports `top_k`)
- Custom + body has `top_p=0.9` → field NOT removed
- Custom + body has `stop_sequences=["END"]` → field NOT removed
- Official + body has every Anthropic-only field → none removed
- Body has unrelated fields (`model`, `messages`, `temperature`, ...) → never touched

### 7.2 Integration Tests (in `anthropic/mod.rs` tests module)

Following the existing `test_build_request_*` style (`anthropic.rs:170-219`):

- `test_build_request_wires_top_p_top_k_on_official`
- `test_build_request_wires_top_p_top_k_on_custom` (both still wired — Custom supports them)
- `test_build_request_wires_stop_sequences_csv` — `"END,STOP,\\n\\n"` → `["END","STOP","\\n\\n"]`
- `test_build_request_drops_empty_stop_sequences` — `""` / `","` → field absent
- `test_build_request_wires_service_tier_on_official` — `"auto"` → body has it
- `test_build_request_strips_service_tier_on_custom` — body lacks it
- `test_build_request_wires_metadata_user_id_on_official` — body has `metadata.user_id`
- `test_build_request_strips_metadata_on_custom` — body lacks `metadata` entirely
- `test_build_request_wires_effort_on_official` — body has `output_config.effort`
- `test_build_request_strips_output_config_on_custom` — body lacks `output_config`
- `test_build_request_cache_control_only_on_official` — extends existing `test_build_request_system_block_cached`; Custom path: system block has no `cache_control`

### 7.3 Wire Shape Test (T4)

In `anthropic/types.rs` tests module: build a fully-populated `MessagesRequest`, serialize, `assert_eq!` against a hand-written `serde_json::json!(...)` literal with exact field names and nesting.

### 7.4 Existing Test Updates (T9)

`anthropic/adapter.rs:515-547` four `effective_cache_retention` tests:
- `effective_retention_official_unset_defaults_short` — keep
- `effective_retention_third_party_unset_defaults_off` → rename to `effective_retention_unset_always_defaults_short`; predict `Short` regardless of host; pair with the new `test_build_request_cache_control_only_on_official` to lock the host-gate behavior end-to-end
- `effective_retention_explicit_long_on_third_party_respected` — keep (semantics unchanged: explicit value respected)
- `effective_retention_explicit_off_always_off` — keep

### 7.5 Regression Sweep

Every 3-4 tasks, run full `cargo test -p alephcore --lib`. Final task runs `cargo clippy -p alephcore -- -D warnings`. Pre-existing `provider_policy.rs:test_apply_policy_strips_fields` failure is documented as out-of-scope (OpenAI-protocol bug, separate patch).

---

## 8. TDD Task Decomposition

10 tasks, each one fresh-subagent RED→GREEN, ~50-100 LOC implementation + 30-80 LOC tests, one commit per task.

| # | Task | RED tests | Files | Est. LOC |
|---|---|---|---|---|
| **T1** | `AnthropicEndpointClass` + `detect_anthropic_endpoint_class` | 5 | `anthropic/provider_policy.rs` (new), `anthropic/mod.rs` | ~80 |
| **T2** | `AnthropicCapabilities` + `resolve_anthropic_capabilities` + Official/Custom matrix | 2 | `anthropic/provider_policy.rs` | ~60 |
| **T3** | `AnthropicPolicy` + `apply()` strip logic + `build_anthropic_policy` | 8 | `anthropic/provider_policy.rs` | ~100 |
| **T4** | `MessagesRequest` adds `top_p / top_k / stop_sequences / metadata`; new `Metadata` struct; wire-shape test | 1 | `anthropic/types.rs` | ~40 |
| **T5** | `ProviderConfig` adds `metadata_user_id / effort`; `test_config()` + 4 struct-literal sites; TOML deser tests | 2 | `config/types/provider.rs` | ~50 |
| **T6** | `build_request` wires `top_p / top_k / stop_sequences` (incl. CSV parser) | 4 | `anthropic/adapter.rs`, `anthropic.rs` (tests) | ~60 |
| **T7** | `build_request` wires `service_tier` (un-hardcode `None`) | 2 | `anthropic/adapter.rs`, `anthropic.rs` (tests) | ~30 |
| **T8** | `build_request` wires `metadata_user_id` + `effort` | 4 | `anthropic/adapter.rs`, `anthropic.rs` (tests) | ~50 |
| **T9** | Refactor `effective_cache_retention` + gate `cache_control` injection on `policy.capabilities.supports_cache_control`; update existing 4 tests; add end-to-end Official-vs-Custom assertion | 1+4 | `anthropic/adapter.rs` | ~30 |
| **T10** | CHANGELOG.md "Cycle 4 — Anthropic protocol parity" retrospective entry | — | `CHANGELOG.md` | docs |

Estimated total: ~500 LOC implementation + ~300 LOC tests across 10 commits.

---

## 9. Error Handling

- **`detect_anthropic_endpoint_class`** — URL parse failure → `Custom` (conservative default). No panic.
- **`AnthropicPolicy::apply`** — `body` not an object → early return. Missing nested keys → `remove` is no-op.
- **`parse_stop_sequences`** — empty / whitespace-only / all-commas input → `Vec::new()`. Filter at call site converts to `None` (field absent on wire).
- **`config.effort`** value validation — no client-side check. `"weird"` is sent verbatim; Anthropic returns 400; existing `stream_deltas` error path surfaces it. Consistent with Cycle 3's posture on `seed` / `logprobs`.
- **`metadata_user_id`** length / character validation — no client-side check. Anthropic documents a 256-char limit and rejects overflow server-side.

---

## 10. Backward Compatibility

| Dimension | Impact | Mitigation |
|---|---|---|
| `ProviderConfig` serialization | 2 new `Option<String>` fields with `#[serde(default)]` | Old `config.toml` deserializes unchanged; missing fields → `None` |
| `MessagesRequest` serialization | 4 new `Option<...>` fields, all `skip_if_none` | Absent fields = old wire shape; no breaking change |
| `AnthropicProtocol` public API | No additions or removals | Callers unaffected |
| `EndpointClass::AnthropicPublic` (OpenAI side) | Untouched | OpenAI capability inference unchanged |
| `effective_cache_retention` signature | Unchanged (`_endpoint` retained) | Existing call sites compile as-is |
| `effective_cache_retention` semantics | Custom-host default goes from `Off` → `Short`, BUT capability gate in `build_request` prevents physical wire change | 4 existing tests updated; 1 new end-to-end test locks wire-level behavior |
| Existing user `config.toml` with `cache_retention = "long"` on Custom host | Same warn! log preserved; physically still no `cache_control` injected | Behavior identical to today |

---

## 11. Risk Assessment

| Risk | Probability | Severity | Mitigation |
|---|---|---|---|
| Cycle 4 mis-strips a legitimate Official field | Low | Medium | 8 matrix tests + manual Official-endpoint regression |
| Custom endpoint receives field it doesn't support → 400 | Low | Medium | Custom profile defaults the 4 Anthropic-only bits `false` + manual kimi regression |
| Third-party Anthropic gateway (MiniMax/OpenRouter) rejects `top_k` | Very Low | Low | `top_k` is core Anthropic-protocol; every known third-party supports it. If a counter-example appears, Cycle 5 adds a named variant |
| `effective_cache_retention` refactor breaks existing tests | Medium | Low | T9 dedicated task; explicit predictions for all 4 existing tests |
| Task granularity → noisy commit history | Medium | Low | ~10 commits, comparable to Cycle 3's 12. Acceptable |
| `MessagesRequest` field-order change confuses reviewer | Low | Low | Spec §5 documents the order |

---

## 12. Rollback Strategy

Every task is an independent commit; any single task can be reverted with `git revert <sha>`.

**Surgical rollback** (e.g., found Custom mis-strips one field): edit `resolve_anthropic_capabilities`, add a test, new commit.

**Full-cycle rollback** (e.g., real-machine regression reveals Anthropic changed wire shape): `git revert` T1..T10. The new module file disappears; the two `ProviderConfig` fields remain as unused fields (kept for forward-compat of any `config.toml` already updated). Next cycle re-wires.

**No transition layer**: Unlike Cycle 3's `ResponseFormatLayer` (with its 2026-05-17 deletion window), Cycle 4 introduces no compat shim. New module is production-quality from day one.

---

## 13. Acceptance Criteria

Cycle 4 ships when:

1. All new unit + integration tests pass.
2. `cargo clippy -p alephcore -- -D warnings` reports zero new warnings.
3. **Manual Official-endpoint regression** (1 request): `service_tier="auto"` + `metadata_user_id="cycle4-test"` + `effort="medium"` → Anthropic accepts, response streams normally.
4. **Manual Custom-endpoint regression** (1 request, kimi-for-coding): same config → wire body lacks `service_tier` / `metadata` / `output_config`, request accepted, response streams normally.
5. CHANGELOG.md gains an English "Cycle 4 — Anthropic protocol parity" entry under Unreleased, with Added / Fixed sections.
6. Pre-existing `test_apply_policy_strips_fields` failure remains as-is (out-of-scope, OpenAI-protocol bug — separate patch).

---

## 14. References

- Cycle 3 spec: [`2026-05-12-openai-protocol-cycle3-output-controls-and-capability-flip-design.md`](./2026-05-12-openai-protocol-cycle3-output-controls-and-capability-flip-design.md)
- Cycle 4 scope decision: [`project_cycle4_anthropic_scope.md`](../../../../.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/project_cycle4_anthropic_scope.md)
- Capability-override redline: [`feedback_no_user_capability_override.md`](../../../../.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/feedback_no_user_capability_override.md)
- Prompt-cache prior art: [`2026-05-11-anthropic-protocol-step2-prompt-cache.md`](./2026-05-11-anthropic-protocol-step2-prompt-cache.md)
- openclaw Anthropic extension: `/Volumes/TBU4/Github/openclaw/extensions/anthropic/` (reference only, not vendored)
- Anthropic Messages API: https://docs.anthropic.com/en/api/messages
