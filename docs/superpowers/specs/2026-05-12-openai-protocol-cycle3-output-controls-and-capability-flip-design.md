# OpenAI Protocol Cycle 3 — Output Controls & Capability Flip Design

**Date:** 2026-05-12
**Status:** Approved
**Cycle:** 3 of the OpenAI protocol provider-side optimization series
**Predecessor:** [`2026-05-12-openai-protocol-cycle2-output-format-and-completion-tokens-design.md`](./2026-05-12-openai-protocol-cycle2-output-format-and-completion-tokens-design.md) (Cycle 2, shipped)
**Successor:** Cycle 4 will own Anthropic protocol parity (separate spec)

---

## 1. Scope & Goals

Four surgical additions to Aleph's OpenAI Chat and Responses adapters. No new abstractions, no trait changes, no destructive refactors. All wires already exist; this cycle just connects them and flips conservative capability defaults that Cycle 2 left behind.

### Four-Bundle Manifest

| Bundle | Feature | Class | Files touched |
|---|---|---|---|
| **C3-A** | `seed: Option<u64>` (deterministic sampling) | New capability | `src/config/types/provider.rs`, `src/providers/protocols/openai_chat/adapter.rs`, `src/providers/protocols/openai_responses/mod.rs`, `src/providers/protocols/openai_common/provider_policy.rs` |
| **C3-B** | `logprobs: Option<bool>` + `top_logprobs: Option<u8>` (request-only) | New capability | Same as C3-A |
| **C3-C** | Flip `supports_response_format` to `true` on 8 endpoint classes; add `JsonSchema → JsonObject` degrade path for non-strict endpoints | Capability flip + degrade | `src/providers/protocols/openai_common/provider_policy.rs`, `src/providers/protocols/openai_common/response_format.rs` |
| **C3-D** | Route `ResponseFormat::JsonSchema { schema }` through `openai_strict_schema::normalize_for_strict` when `supports_strict_schema = true` | Normalization | `src/providers/protocols/openai_common/response_format.rs` |

### Non-Goals (Out-of-Scope This Cycle)

- Anthropic protocol changes (deferred to Cycle 4)
- `logprobs` response-side parsing (SSE deltas, `choices[*].logprobs` → MessageDelta surfacing)
- Per-model capability gating (e.g., `logprobs=true` on o1/o3 still 400s — Cycle 4 candidate)
- Per-request payload override paths for any of the new fields
- User-level `ProviderConfig` capability overrides (force `supports_seed=true` on Local, etc.)
- Trait extension or breaking signature change

### Architectural Alignment

- **R7 LLM Sovereignty** — All four fields are config-driven wire shape; zero rule-engine, zero intent-detection logic.
- **R10 Thin Harness** — Pure surgical wiring; no new abstractions, no trait churn. Caps shipped Cycle 2's defense-in-depth pattern.
- **P1/P2 Coupling/Cohesion** — Shared translation lives in `openai_common/`; Chat and Responses adapters both consume the same helpers.
- **DRY** — `normalize_for_strict` is the same helper tool definitions go through. Response_format JsonSchema and tool schemas share a single normalization path.

---

## 2. Type Design

### 2.1 `ProviderConfig` New Fields

Added alongside Cycle 2's `response_format` / `parallel_tool_calls`. All `#[serde(default)]` to preserve backward compatibility with existing TOML configs.

```rust
/// Deterministic sampling seed. None = server default.
#[serde(default)]
pub seed: Option<u64>,

/// Whether to return per-token logprobs. None = no field emitted.
#[serde(default)]
pub logprobs: Option<bool>,

/// Number of top alternative tokens per position (Chat range: 0..=20).
/// Only meaningful when logprobs == Some(true). None = no field emitted.
#[serde(default)]
pub top_logprobs: Option<u8>,
```

### 2.2 `ProviderCapabilities` New Bits

Added in `openai_common/provider_policy.rs` adjacent to the Cycle 2 `supports_response_format` bit.

```rust
/// Endpoint accepts the `seed` field on Chat/Responses requests.
pub supports_seed: bool,

/// Endpoint accepts `logprobs` / `top_logprobs` on Chat/Responses requests.
pub supports_logprobs: bool,
```

### 2.3 No New Enums or Structs

- `seed` is bare `u64`.
- `logprobs` is bare `bool`; `top_logprobs` is bare `u8`.
- No `LogprobsConfig` wrapper struct (YAGNI — two fields cover the API surface).
- `ResponseFormat` enum is unchanged from Cycle 2.

---

## 3. Capability Matrix Updates

### 3.1 `supports_response_format` Flip (10 of 13 → 8 newly flipped)

```text
EndpointClass            Cycle 2     Cycle 3     Change
─────────────────────────────────────────────────────────
OpenAiPublic             true        true        —
OpenAiCodex              true        true        —
AzureOpenAi              false       true        FLIP
OpenRouter               false       true        FLIP
DeepSeekNative           false       true        FLIP
GroqNative               false       true        FLIP
MistralPublic            false       true        FLIP
MoonshotNative           false       true        FLIP
CerebrasNative           false       true        FLIP
XAiNative                false       true        FLIP
AnthropicPublic          false       false       — (different wire)
Local                    false       false       — (conservative)
Custom                   false       false       — (conservative)
```

### 3.2 `supports_seed` / `supports_logprobs` Initial Matrix

```text
EndpointClass            supports_seed   supports_logprobs   Reason
──────────────────────────────────────────────────────────────────────────────
OpenAiPublic             true            true                Official API
OpenAiCodex              true            true                ChatGPT backend
AzureOpenAi              true            true                Azure passthrough of OpenAI
OpenRouter               true            true                Passes through to underlying model
DeepSeekNative           true            false               seed documented; logprobs unclear
GroqNative               true            true                Official OpenAI-compat
MistralPublic            true            false               seed documented; logprobs unclear
MoonshotNative           true            false               seed documented; logprobs unclear
CerebrasNative           true            true                Official OpenAI-compat
XAiNative                true            true                xAI claims OpenAI compat
AnthropicPublic          false           false               Different protocol
Local                    false           false               vLLM/llama.cpp variable
Custom                   false           false               Unknown — conservative
```

Conservative principle: a third-party endpoint flips `supports_logprobs=true` only if the vendor docs explicitly enumerate the parameter. A follow-up PR can flip without blocking Cycle 3.

### 3.3 `supports_strict_schema` Unchanged

Cycle 3 **does not** redistribute `supports_strict_schema`. That bit is co-owned by the tool-definition normalization path; any reassignment would have a much larger blast radius. Cycle 3 only **consumes** the existing bit inside the `response_format` JsonSchema branch (see §4).

---

## 4. Wire Translation Details

### 4.1 `openai_common/response_format.rs` — Degrade + Normalize

```rust
pub fn to_chat_response_format(
    fmt: &ResponseFormat,
    supports_strict: bool,
) -> Option<Value> {
    match fmt {
        ResponseFormat::Text => None,
        ResponseFormat::JsonObject => Some(json!({ "type": "json_object" })),
        ResponseFormat::JsonSchema { name, schema } => {
            if !supports_strict {
                // C3-C: degrade JsonSchema → JsonObject on non-strict endpoints
                return Some(json!({ "type": "json_object" }));
            }
            // C3-D: normalize user schema in line with tool definitions
            let normalized = openai_strict_schema::normalize_for_strict(schema);
            Some(json!({
                "type": "json_schema",
                "json_schema": {
                    "name": name,
                    "strict": true,
                    "schema": normalized,
                }
            }))
        }
    }
}
```

`merge_text_format(base, fmt)` (Responses-side helper) follows the identical branching: JsonSchema on non-strict → `TextFormat::JsonObject`; strict → `TextFormat::JsonSchema { name, strict: true, schema: normalized }`.

### 4.2 `normalize_for_strict` Semantics

Reuse of the existing `openai_common/openai_strict_schema.rs` helper that tool definitions already pass through:

1. Recursively inject `"additionalProperties": false` into every object node.
2. Copy each object's `properties` keys into `required`.
3. Preserve user-supplied `description` / `enum` / `format` / `pattern` constraints.

This is **reuse, not duplication** — no new normalization code is written.

### 4.3 Chat Adapter — `openai_chat/adapter.rs`

Inject after Cycle 2's `response_format` / `parallel_tool_calls` block, before `PayloadPolicy::apply`:

```rust
// C3-A: seed
if let Some(seed) = config.seed {
    if policy.capabilities.supports_seed {
        body["seed"] = json!(seed);
    }
}

// C3-B: logprobs + top_logprobs
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

### 4.4 Responses Adapter — `openai_responses/mod.rs`

Responses protocol uses a single `top_logprobs: u32` field at the top level (no separate `logprobs` boolean). Setting `top_logprobs > 0` opts in; omitting it opts out.

```rust
// C3-A: seed
if let Some(seed) = config.seed {
    if policy.capabilities.supports_seed {
        req.seed = Some(seed);
    }
}

// C3-B: top_logprobs (Responses has no `logprobs` boolean)
if let Some(want_logprobs) = config.logprobs {
    if policy.capabilities.supports_logprobs && want_logprobs {
        req.top_logprobs = config.top_logprobs.map(|n| n as u32).or(Some(0));
    }
}
```

`ResponsesRequest` struct grows two new optional fields:

```rust
#[serde(skip_serializing_if = "Option::is_none")]
pub seed: Option<u64>,

#[serde(skip_serializing_if = "Option::is_none")]
pub top_logprobs: Option<u32>,
```

### 4.5 `PayloadPolicy::apply` Strip-List (Defense-in-Depth)

Three new stripper branches in `provider_policy.rs`, parallel to Cycle 2's `response_format` strip:

```rust
if !self.capabilities.supports_seed {
    payload.remove("seed");
}
if !self.capabilities.supports_logprobs {
    payload.remove("logprobs");
    payload.remove("top_logprobs");
}
```

### 4.6 Injection Order (Cycle 1/2/3 Consolidated)

```text
build_body flow:
  1. base body (model / messages / max_tokens / temperature / ...)
  2. Cycle 1: stop_sequences, finish_reason mapping
  3. Cycle 2: response_format, parallel_tool_calls, max_completion_tokens swap
  4. Cycle 3: seed, logprobs, top_logprobs
  5. PayloadPolicy::apply — strip unsupported fields (final defense)
  6. serialize → HTTP
```

---

## 5. Testing Strategy

### 5.1 Test Groups by Bundle

| Bundle | Test groups | Location | Count |
|---|---|---|---|
| C3-A `seed` | Chat wire emit, Responses wire emit, capability gate (true/false), strip-list | `openai_chat/tests.rs`, `openai_responses/tests.rs`, `provider_policy.rs#tests` | 8 |
| C3-B `logprobs` / `top_logprobs` | Chat `logprobs=true` with/without `top_logprobs`; Chat `logprobs=false` omits `top_logprobs`; Responses `top_logprobs` injection; capability gate; strip-list | Same | 10 |
| C3-C Endpoint flip + degrade | Assert `supports_response_format == true` on 8 flipped endpoints; assert `== false` on AnthropicPublic / Local / Custom; assert JsonSchema → JsonObject degrade on non-strict; assert JsonSchema preserved on strict | `response_format.rs#tests`, `provider_policy.rs#tests` | 8 |
| C3-D Strict normalization | normalize injects `additionalProperties:false`; injects `required = all properties`; preserves user descriptions/enums; recurses into nested objects; matches tool-definition path output | `response_format.rs#tests` | 6 |

**Target: ≥ 32 new tests** (realistic estimate 35-40).

### 5.2 No New Dev-Dependencies

- Reuse Cycle 2's `extract_chat_body` / `standard_test_variant` helpers.
- Do not introduce `rstest` (project policy).
- One `#[test]` per case, descriptive names (`seed_emitted_when_supported`, `logprobs_stripped_when_endpoint_lacks_support`, `json_schema_degrades_to_json_object_on_non_strict_endpoint`, ...).

### 5.3 Regression Baseline

- **Chat suite**: 57 → at least 71 (+14), all green
- **Responses suite**: 53 → at least 63 (+10), all green
- **provider_policy suite**: current → +8, all green
- **response_format suite**: current → +12, all green
- **`test_apply_policy_strips_fields`**: known pre-existing baseline failure — non-blocking, same posture as Cycle 2.

### 5.4 Clippy

- Target: zero new warnings (sustain Cycle 2 baseline of 9 pre-existing).
- Command: `cargo clippy -p alephcore --lib -- -D warnings`.

### 5.5 Execution Pattern (For Subagent-Driven Implementation)

Each implementation task is RED test → GREEN code → REFACTOR → COMMIT. Use narrow filters (`cargo test -p alephcore --lib <module>`) to keep harness latency low, matching the stable pattern from Cycle 2.

---

## 6. Backward Compatibility & Risk

### 6.1 Compatibility Matrix

| Change | Old config behavior | Old wire payload | Compat |
|---|---|---|---|
| `seed` / `logprobs` / `top_logprobs` new fields | `#[serde(default)]` → `None` | Field absent | ✅ Zero regression |
| `supports_seed` / `supports_logprobs` new bits | Auto-initialized | No effect when config field is `None` | ✅ Zero regression |
| `supports_response_format` flipped on 8 endpoints | Old configs have no `response_format` field → nothing emitted | Byte-identical | ✅ Zero regression |
| `JsonSchema → JsonObject` degrade | Cycle 2 already returned `None` on non-strict endpoints (capability=false) → no user could have shipped this combo | N/A | ✅ Zero regression |
| Strict normalization injects `additionalProperties:false` | Idempotent if user already set it; injected otherwise | Wire becomes stricter | ⚠️ See 6.2 |

### 6.2 Single Risk Annotation: Strict Normalization

**Behavior change on strict endpoints (OpenAi / OpenAiCodex)**: Cycle 2 forwarded user-provided JSON schemas verbatim. Cycle 3 routes them through `normalize_for_strict` first.

- **Potential impact**: A user schema that allowed implicit "extra fields" (no explicit `additionalProperties: false`) will now reject them — model output may drop fields the user previously got.
- **Surface area**: minimal. OpenAI's strict mode itself requires `additionalProperties: false`; non-normalized schemas would already 400 upstream. Cycle 3 turns "400 errors" into "valid requests," not the other way around.
- **Mitigation**: CHANGELOG explicitly notes "strict-mode response_format schemas now normalized in line with tool definitions."

### 6.3 Other Risks

| Risk | Assessment | Mitigation |
|---|---|---|
| A flipped endpoint actually rejects `{type: "json_object"}` | Third-party docs are uneven; some "OpenAI-compat" backends 400 on `response_format` | Strip-list defense covers; per-endpoint flip can be reverted in a follow-up if needed |
| `logprobs=true` 400s on reasoning models routed through OpenAiPublic | Per-endpoint gating doesn't catch per-model variance | Known limitation; documented in CHANGELOG; per-model gate is Cycle 4 candidate |
| Custom / Local user wants `response_format` | Default false; user can't toggle | Cycle 3 doesn't expose user override; Cycle 4 candidate (per-config capability override) |

### 6.4 User-Level Capability Override — Out of Scope

Cycle 3 keeps endpoint-class inference as the single source of capability truth. Users cannot force `supports_seed=true` on a `Local` endpoint via config. If demand emerges, a separate cycle adds a `ProviderConfig::capability_overrides: HashMap<String, bool>` field.

---

## 7. Acceptance Criteria

| AC | Description | Verification | Gate |
|---|---|---|---|
| **AC-1** | Zero new regressions in existing suites | `cargo test -p alephcore --lib openai_chat openai_responses openai_common` green; `test_apply_policy_strips_fields` remains the only pre-existing baseline failure | Blocking |
| **AC-2** | ≥ 32 new tests, all green | Filtered test runs per §5.1 | Blocking |
| **AC-3** | Zero new clippy warnings | `cargo clippy -p alephcore --lib -- -D warnings` matches Cycle 2 baseline (9 pre-existing) | Blocking |
| **AC-4** | 8 endpoints flipped to `supports_response_format = true` | Unit test enumerates AzureOpenAi / OpenRouter / DeepSeek / Groq / Mistral / Moonshot / Cerebras / XAi | Blocking |
| **AC-5** | JsonSchema degrades to JsonObject on non-strict endpoints | `to_chat_response_format(JsonSchema{..}, supports_strict=false)` returns `{type: "json_object"}` | Blocking |
| **AC-6** | Strict-endpoint schemas pass through `normalize_for_strict` | Unit test: nested objects gain `additionalProperties:false` and `required`-all-keys; output matches tool-definition path | Blocking |
| **AC-7** | `seed` emitted only on supporting endpoints | Chat + Responses each ship 2 tests (OpenAiPublic emits / Local strips) | Blocking |
| **AC-8** | `logprobs` + `top_logprobs` gated; `top_logprobs` omitted when `logprobs=false` | Chat + Responses each ship 2-3 tests | Blocking |
| **AC-9** | CHANGELOG.md `[Unreleased]` lists 3 English entries (Added / Fixed / Changed as relevant) | grep | Blocking |
| **AC-10** | Manual smoke: configure Groq endpoint with `response_format=JsonObject` → valid JSON returned | Live curl | Deferred (non-blocking) |
| **AC-11** | Manual smoke: gpt-4o with `response_format=JsonSchema{..}` + strict → output conforms to schema | Live curl | Deferred (non-blocking) |
| **AC-12** | Manual smoke: same prompt with `seed=42` twice on OpenAi public → identical (or near-identical) output | Live curl | Deferred (non-blocking) |

**Blocking vs non-blocking** follows Cycle 1/2 convention: automated suites block, live-endpoint smokes defer.

---

## 8. Implementation Order (Indicative for Plan Phase)

Plan phase will decompose into TDD tasks; rough order:

1. C3-A T1: `supports_seed` capability bit + 13 endpoint values + `seed` strip-list + tests
2. C3-A T2: `seed` field on `ProviderConfig` + adapter literal updates
3. C3-A T3: Chat adapter wires `seed` + wire tests
4. C3-A T4: Responses adapter + `ResponsesRequest.seed` field + wire tests
5. C3-B T5: `supports_logprobs` capability bit + 13 endpoint values + strip-list + tests
6. C3-B T6: `logprobs` + `top_logprobs` on `ProviderConfig` + adapter literal updates
7. C3-B T7: Chat adapter wires logprobs pair + tests (incl. `top_logprobs` omitted when `logprobs=false`)
8. C3-B T8: Responses adapter wires `top_logprobs` + `ResponsesRequest.top_logprobs` + tests
9. C3-C T9: Flip `supports_response_format` to true on 8 endpoints + assertion tests
10. C3-C T10: Add `JsonSchema → JsonObject` degrade in `to_chat_response_format` + `merge_text_format` + tests
11. C3-D T11: Route JsonSchema strict path through `normalize_for_strict` (Chat + Responses) + parity tests vs tool-definition path
12. T12: CHANGELOG `[Unreleased]` 3 entries

Twelve tasks is the working estimate; plan phase can split or merge.

---

## 9. Open Questions / Defer to Cycle 4

- Anthropic protocol parity (separate spec; Cycle 4)
- Per-model capability gating (e.g., reasoning models reject `logprobs`)
- User-level `ProviderConfig` capability overrides
- `logprobs` response-side parsing (SSE delta surfacing)
- Strict-schema validation (vs normalization) — reject ill-formed schemas at build time
