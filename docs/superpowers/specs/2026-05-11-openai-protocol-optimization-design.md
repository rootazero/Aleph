# OpenAI Protocol Provider Client Optimization Design

> Date: 2026-05-11
> Scope: Provider client protocol layer (OpenAI Chat + Responses APIs)
> Strategy: Phase A incremental enhancement, paving the way for Phase B unified abstraction

## 1. Goal

Reference OpenClaw's mature OpenAI Protocol implementation to identify and fix bugs, performance issues, and missing features in Aleph's provider client protocol layer — without destructive refactoring.

Key constraints:
- **Non-breaking**: All changes must be backward compatible
- **Rust-first**: Leverage Rust's type safety and zero-cost abstractions
- **Incremental**: Phase A builds independent modules; Phase B unifies them into traits
- **Cleanup**: Remove old code after new modules are integrated

## 2. Problem Analysis

### 2.1 High-Priority Issues

| Area | Aleph Current State | OpenClaw Advantage | Impact |
|------|---------------------|-------------------|--------|
| **SSE Parsing** | Single `\n` delimiter; silently drops malformed JSON; no keepalive filtering | `findSseEventBoundary` supports `\r\n\r\n`/`\n\n`/`\r\r`; `sanitizeOpenAISdkSseResponse` filters empty events | Stream events may be lost or parsed incorrectly |
| **Tool Schema Strict Mode** | Only basic `sanitize_tool_name` + `ensure_properties_recursive`; no strict validation | `normalizeStrictOpenAIJsonSchema` recursively normalizes; validates `anyOf`/`oneOf`/`allOf` prohibition | `strict: true` may be rejected by provider |
| **Provider Payload Policy** | Hardcoded `is_openai_official()` binary check | `OpenAIResponsesPayloadPolicy` detects 15+ endpoint classes with per-provider store/reasoning/cache/compaction policies | Non-official providers may receive unsupported fields |
| **Retry Header Handling** | Parses "retry after N" from error message string only | `parseRetryAfterSeconds` supports `Retry-After-Ms`, `Retry-After` headers, and HTTP-date format | 429 retry delays inaccurate; may block indefinitely |

### 2.2 Medium-Priority Missing Features

- No `ModelCompatConfig` layer for per-provider capability declaration
- No `supportsOpenAIReasoningEffort` detection
- No `findOpenAIStrictToolSchemaDiagnostics` pre-flight validation
- `content_filter` finish reason mapped to `MaxTokens` (should be separate)

### 2.3 Aleph Strengths to Preserve

- `ProtocolAdapter` trait abstraction is clean and correct
- `ProviderDelta` + `DeltaCollector` streaming architecture is excellent
- `IndexIdTracker` tool call correlation design is sound
- `RetryVerdict` error classification framework is well-designed
- YAML-configurable protocol (`ConfigurableProtocol`) is extensible

## 3. Design — Four Enhancement Modules

### 3.1 Module 1: SSE Parsing Layer Enhancement

**New File**: `src/providers/protocols/openai_common/sse.rs`

#### 3.1.1 API Design

```rust
//! Robust SSE parsing utilities for OpenAI-compatible protocols.

use crate::error::Result;

/// SSE event boundary detection (supports \r\n\r\n, \n\n, \r\r).
pub(crate) fn find_sse_event_boundary(buffer: &[u8]) -> Option<(usize, usize)>;

/// Check if an SSE block contains readable data (not just empty keepalive).
pub(crate) fn has_readable_sse_data(block: &str) -> bool;

/// Parse a single SSE data line, propagating parse errors.
pub(crate) fn parse_sse_data_line(line: &str) -> Result<Option<&str>>;

/// Generic SSE stream builder that eliminates duplication between Chat/Responses adapters.
pub(crate) struct SseStreamBuilder<S, F> { ... }

impl<S, F, D> SseStreamBuilder<S, F>
where
    S: Stream<Item = Result<axum::body::Bytes>> + Unpin,
    F: FnMut(&str, &mut VecDeque<Result<D>>) + Send,
    D: Send + 'static,
{
    pub fn new(byte_stream: S, parser: F) -> Self;
    pub fn into_stream(self) -> BoxStream<'static, Result<D>>;
}
```

#### 3.1.2 Key Behaviors

1. **Multi-delimiter boundary detection**: Scan for `\r\n\r\n`, `\n\n`, `\r\r` simultaneously; choose earliest match
2. **Malformed event handling**: JSON parse failures propagate as `Result::Err` instead of silent `return`
3. **Keepalive filtering**: Drop SSE blocks where all `data:` lines are empty or `[DONE]`
4. **UTF-8 safety**: Buffer raw bytes until complete line found; decode per-line to avoid multi-byte split issues

#### 3.1.3 Integration Points

- `openai_chat/adapter.rs`: Replace inline `unfold` state machine with `SseStreamBuilder`
- `openai_responses/mod.rs`: Replace inline `unfold` state machine with `SseStreamBuilder`
- `sse.rs`: Change `parse_chat_sse_event` return type to `Result<()>`

#### 3.1.4 Code to Remove

- `openai_chat/adapter.rs`: Inline `State` struct and `unfold` logic (~80 lines)
- `openai_responses/mod.rs`: Inline `State` struct and `unfold` logic (~80 lines)
- `sse.rs`: Silent error handling (`return` on JSON parse fail)

---

### 3.2 Module 2: Tool Schema Strict Normalization

**New File**: `src/providers/protocols/openai_common/openai_strict_schema.rs`

#### 3.2.1 API Design

```rust
//! Strict JSON Schema normalization and validation for OpenAI tool calling.

use serde_json::Value;

/// Recursively normalize a JSON schema for OpenAI strict mode compatibility.
pub fn normalize_strict_schema(schema: &mut Value);

/// A schema violation diagnostic.
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaDiagnostic {
    pub path: String,
    pub violation: String,
}

/// Find all strict schema violations without modifying the schema.
pub fn find_strict_schema_diagnostics(schema: &Value) -> Vec<SchemaDiagnostic>;

/// Check if a schema is compatible with OpenAI strict mode.
pub fn is_strict_schema_compatible(schema: &Value) -> bool;
```

#### 3.2.2 Normalization Rules

1. **Root object**: Set `additionalProperties: false` if absent (depth == 0)
2. **Object schemas**: Ensure `properties` exists (empty object if none)
3. **Object schemas**: Ensure `required` exists (empty array if none)
4. **Strip unsupported**: Remove `anyOf`, `oneOf`, `allOf` keywords
5. **Type arrays**: Collapse homogeneous arrays to single type; heterogeneous flagged as violation
6. **Recursion**: Descend into `properties`, `items`, `prefixItems`, `contains`, etc.

#### 3.2.3 Integration Points

- `openai_chat/adapter.rs`: Call `normalize_strict_schema` when `td.strict == true`; call `find_strict_schema_diagnostics` and warn if violations found
- `openai_common/tools.rs`: `ensure_properties_recursive` functionality subsumed by `normalize_strict_schema`

#### 3.2.4 Code to Remove

- `openai_common/tools.rs`: `ensure_properties_recursive` function (~30 lines)
- `adapter.rs`: Inline `properties`/`type` object injection logic (replaced by `normalize_strict_schema`)

---

### 3.3 Module 3: Retry Header Handling Enhancement

**New File**: `src/providers/retry_policy.rs`

#### 3.3.1 API Design

```rust
//! Retry policy with HTTP-aware delay resolution.

use std::time::Duration;

/// Parsed retry delay from provider response.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RetryDelay {
    Fixed(Duration),
    Exponential { base: Duration, attempt: u32 },
    NoRetry,
}

/// Extract retry delay from HTTP response headers and/or error context.
pub fn resolve_retry_delay(
    status: Option<u16>,
    headers: Option<&reqwest::header::HeaderMap>,
    error_message: Option<&str>,
) -> RetryDelay;

/// Apply maximum wait cap and compute final delay.
pub fn apply_delay_cap(delay: Duration, max_wait: Option<Duration>) -> Duration;
```

#### 3.3.2 Resolution Priority

1. `Retry-After-Ms` header (Azure/OpenAI specific)
2. `Retry-After` header (seconds or HTTP-date format)
3. Error message text parsing ("retry after N seconds")
4. Status-code defaults:
   - 429 → Exponential backoff (base 1s)
   - 529 → Fixed 2s
   - 5xx → Exponential backoff (base 300ms)
   - Other → NoRetry

#### 3.3.3 Integration Points

- `llm_retry.rs`: Replace `extract_retry_after` with `resolve_retry_delay`; add HTTP context to `classify_error`
- `http_provider.rs`: Pass response status/headers to error context so retry policy can use them
- `openai_chat/adapter.rs` + `openai_responses/mod.rs`: Include HTTP headers in error construction

#### 3.3.4 Code to Remove

- `llm_retry.rs`: `extract_retry_after` function (~15 lines)
- Adapter error handlers: String-only error construction (enhanced to include HTTP context)

---

### 3.4 Module 4: Provider Payload Policy

**New File**: `src/providers/protocols/openai_common/provider_policy.rs`

#### 3.4.1 API Design

```rust
//! Provider-specific payload policy for OpenAI-compatible protocols.

/// Detected provider endpoint class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EndpointClass {
    OpenAiPublic, OpenAiCodex, AzureOpenAi,
    AnthropicPublic, DeepSeekNative, GroqNative,
    MistralPublic, MoonshotNative, CerebrasNative,
    XAiNative, OpenRouter, Local, Custom,
}

/// Per-provider capability flags.
#[derive(Debug, Clone, Default)]
pub struct ProviderCapabilities {
    pub supports_responses_store: bool,
    pub supports_reasoning_effort: bool,
    pub supports_prompt_cache: bool,
    pub supports_service_tier: bool,
    pub supports_strict_schema: bool,
    pub supports_server_compaction: bool,
    pub requires_object_properties: bool,
    pub context_window: Option<usize>,
}

/// Resolved payload policy.
#[derive(Debug, Clone)]
pub struct PayloadPolicy {
    pub endpoint_class: EndpointClass,
    pub capabilities: ProviderCapabilities,
    pub explicit_store: Option<bool>,
    pub strip_store: bool,
    pub strip_reasoning: bool,
    pub strip_prompt_cache: bool,
    pub compaction_threshold: Option<usize>,
}

impl PayloadPolicy {
    pub fn apply(&self, payload: &mut serde_json::Map<String, serde_json::Value>);
    pub fn apply_to_schema(&self, schema: &mut serde_json::Value);
}

/// Detect endpoint class from base URL.
pub fn detect_endpoint_class(base_url: Option<&str>) -> EndpointClass;

/// Resolve capabilities for a given endpoint class.
pub fn resolve_capabilities(class: EndpointClass) -> ProviderCapabilities;

/// Build policy from provider configuration.
pub fn build_payload_policy(
    base_url: Option<&str>,
    api_type: &str,
    variant_store: Option<bool>,
) -> PayloadPolicy;
```

#### 3.4.2 Endpoint Detection Rules

| Host Pattern | EndpointClass |
|-------------|---------------|
| `api.openai.com` | `OpenAiPublic` |
| `chatgpt.com` | `OpenAiCodex` |
| `*.openai.azure.com` | `AzureOpenAi` |
| `api.anthropic.com` | `AnthropicPublic` |
| `api.deepseek.com` | `DeepSeekNative` |
| `api.groq.com` | `GroqNative` |
| `api.mistral.ai` | `MistralPublic` |
| `api.moonshot.ai` / `api.moonshot.cn` | `MoonshotNative` |
| `api.cerebras.ai` | `CerebrasNative` |
| `api.x.ai` / `api.grok.x.ai` | `XAiNative` |
| `*openrouter.ai` | `OpenRouter` |
| `localhost` / `127.0.0.1` / `*.local` | `Local` |

#### 3.4.3 Integration Points

- `openai_responses/mod.rs`: Replace `is_openai_official()` with `build_payload_policy`; use policy to determine store/context_management/reasoning
- `openai_chat/adapter.rs`: Use policy to filter reasoning_effort, service_tier, and other provider-specific fields
- `openai_chat/proto_impl.rs`: Use policy to determine if `requires_object_properties` should trigger extra schema injection

#### 3.4.4 Code to Remove

- `openai_responses/mod.rs`: `is_openai_official()` function (~10 lines)
- `openai_responses/mod.rs`: Hardcoded store/compaction logic in `build_responses_request`
- `openai_chat/adapter.rs`: Hardcoded reasoning field injection

---

## 4. Phase A → Phase B Migration Path

```
Phase A (Current — Incremental)              Phase B (Future — Unified)
─────────────────────────────────────────────────────────────────────────────

sse.rs (standalone functions)         ──►    trait SseTransport
                                              ├─ fn parse_event_boundary()
                                              ├─ fn filter_keepalive()
                                              └─ fn into_stream()

openai_strict_schema.rs               ──►    trait SchemaPolicy
(normalize/diagnose/check)                    ├─ fn normalize()
                                              ├─ fn diagnose()
                                              └─ fn is_compatible()
                                              
                                              impl OpenAiStrictSchemaPolicy
                                              impl PermissiveSchemaPolicy

retry_policy.rs                       ──►    trait RetryPolicy
(resolve_delay/apply_cap)                     ├─ fn classify()
                                              ├─ fn compute_delay()
                                              └─ fn max_wait()
                                              
                                              impl OpenAiRetryPolicy
                                              impl ExponentialRetryPolicy

provider_policy.rs                    ──►    trait PayloadPolicy
(build/apply/capabilities)                    ├─ fn apply_to_request()
                                              ├─ fn apply_to_schema()
                                              └─ fn supports_feature()
                                              
                                              impl OpenAiPayloadPolicy
                                              impl AnthropicPayloadPolicy

ProtocolAdapter::build_request()      ──►    trait RequestBuilder
                                              ├─ fn build_url()
                                              ├─ fn build_headers()
                                              ├─ fn build_body()
                                              └─ fn apply_policy()

ProtocolAdapter::stream_deltas()      ──►    trait ResponseParser
                                              ├─ fn parse_sse()
                                              ├─ fn map_to_deltas()
                                              └─ fn handle_error()
```

**Migration Principle**: All Phase A modules are pure functions or standalone structs with no trait dependencies. In Phase B, they become default trait implementations. Existing code continues to work through blanket impls or adapter structs.

---

## 5. Cleanup Plan

### 5.1 Files to Modify

| File | Changes |
|------|---------|
| `src/providers/protocols/openai_common/sse.rs` | **NEW** — SSE parsing utilities |
| `src/providers/protocols/openai_common/openai_strict_schema.rs` | **NEW** — Schema normalization |
| `src/providers/retry_policy.rs` | **NEW** — Retry header handling |
| `src/providers/protocols/openai_common/provider_policy.rs` | **NEW** — Provider payload policy |
| `src/providers/protocols/openai_chat/adapter.rs` | Use `SseStreamBuilder`, apply `PayloadPolicy`, use `normalize_strict_schema` |
| `src/providers/protocols/openai_chat/sse.rs` | Return `Result`, propagate errors |
| `src/providers/protocols/openai_chat/proto_impl.rs` | Use `PayloadPolicy` for schema requirements |
| `src/providers/protocols/openai_responses/mod.rs` | Use `SseStreamBuilder`, replace `is_openai_official` with `PayloadPolicy` |
| `src/providers/llm_retry.rs` | Use `resolve_retry_delay`, pass HTTP context |
| `src/providers/http_provider.rs` | Pass HTTP context to retry logic |
| `src/providers/protocols/openai_common/tools.rs` | Remove `ensure_properties_recursive` |

### 5.2 Code to Delete

- `is_openai_official()` in `openai_responses/mod.rs`
- `ensure_properties_recursive()` in `openai_common/tools.rs`
- `extract_retry_after()` in `llm_retry.rs`
- Inline `unfold` state machines in both adapters (replaced by `SseStreamBuilder`)

---

## 6. Testing Strategy

### 6.1 Unit Tests (per module)

| Module | Test Coverage |
|--------|--------------|
| `sse.rs` | Multi-delimiter boundary detection; UTF-8 chunk splitting; malformed JSON propagation; empty keepalive filtering; `[DONE]` sentinel handling |
| `openai_strict_schema.rs` | Normalization: root `additionalProperties`, nested `properties`/`required`, `anyOf` stripping; Diagnostics: correct paths, all violation types; Compatibility: pass/fail cases |
| `retry_policy.rs` | `Retry-After-Ms` parsing; `Retry-After` seconds; `Retry-After` HTTP-date; message text parsing; status code defaults; delay cap application |
| `provider_policy.rs` | Endpoint detection: all 15+ classes; Capability resolution: per-class flags; Policy application: field injection/stripping; Compaction threshold calculation |

### 6.2 Integration Tests

- `openai_chat/adapter.rs`: `build_request` with strict tools → normalized schema in body
- `openai_chat/adapter.rs`: `build_request` for DeepSeek → no `reasoning_effort` field
- `openai_responses/mod.rs`: `build_responses_request` for OpenAI official → store=true, compaction present
- `openai_responses/mod.rs`: `build_responses_request` for Moonshot → store stripped

### 6.3 Regression Tests

- Existing tests in `openai_chat/tests.rs` must continue to pass
- Existing tests in `openai_responses/tests.rs` must continue to pass
- `cargo test -p alephcore --lib` must pass

---

## 7. Implementation Order

1. **Module 4 first** (Provider Policy) — needed by Modules 1-3 for configuration
2. **Module 2 second** (Schema) — independent, high impact
3. **Module 3 third** (Retry) — independent, medium impact
4. **Module 4 last** (SSE) — most invasive, depends on understanding stream patterns
5. **Cleanup** — remove old code after all modules integrated
6. **Final verification** — run full test suite

---

## 8. Acceptance Criteria

- [ ] All 4 modules implemented with comprehensive unit tests
- [ ] `cargo test -p alephcore --lib` passes
- [ ] `cargo clippy -p alephcore -- -D warnings` passes
- [ ] Old code removed: `is_openai_official`, `ensure_properties_recursive`, `extract_retry_after`, inline unfold state machines
- [ ] No new compiler warnings introduced
- [ ] Design document updated if deviations occur during implementation

---

*Next step: Load `writing-plans` skill to create detailed implementation plan.*
