# OpenAI Protocol — Token & Event Wiring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire three independent gaps in Aleph's OpenAI Provider client side — canonical `TokenUsage` fields, Responses reasoning_summary event coverage, and `stop_sequences` config — without introducing new abstractions.

**Architecture:** Pure additive wiring. Extend two existing deserialize structs (`OpenAiUsage`, `UsageInfo`) with optional `*_tokens_details` sub-structs; replace three hardcoded `None` lines per call site; expand a `match` arm with explicit handling; add one `body["stop"]` field in Chat adapter and a `stop` field in `ResponsesRequest` struct.

**Tech Stack:** Rust, serde (with `#[serde(default)]` for forward compat), tracing for observability, rstest for table-driven tests, `cargo test -p alephcore`.

**Source spec:** `docs/superpowers/specs/2026-05-12-openai-protocol-token-and-events-wiring-design.md`

**Sibling specs/plans (do NOT duplicate):**
- `2026-05-11-openai-responses-strict-multi-type-fix.md` (B1 desktop schema — separate)
- `2026-05-11-openai-protocol-optimization.md` (M3 retry_policy — deferred)
- `2026-05-11-cache-token-observability.md` (Anthropic `MeteringProvider` tracing — downstream consumer of B2)

---

## File Structure

| File | Responsibility | Touched by |
|---|---|---|
| `src/providers/openai/types.rs` | OpenAI Chat wire types (`OpenAiUsage`, `ChatCompletionRequest`) | Task 1 (extend `OpenAiUsage`) |
| `src/providers/protocols/openai_chat/sse.rs` | Chat SSE event parser → canonical `ProviderDelta` | Task 1 (populate Usage), Task 4 (finish_reason) |
| `src/providers/responses/types.rs` | Responses wire types (`UsageInfo`, `ResponsesRequest`, `StreamEvent`) | Task 2 (extend `UsageInfo`), Task 5 (add `stop` field to `ResponsesRequest`) |
| `src/providers/protocols/openai_responses/mod.rs` | Responses SSE parser + request builder | Task 2 (populate Usage), Task 3 (reasoning_summary arms), Task 5 (request stop) |
| `src/providers/protocols/openai_chat/adapter.rs` | Chat request builder (uses `json!()` macro inline) | Task 5 (`body["stop"]`) |
| `src/providers/protocols/openai_chat/tests.rs` | Chat unit tests | Tasks 1, 4, 5 |
| `src/providers/protocols/openai_responses/tests.rs` | Responses unit tests | Tasks 2, 3, 5 |
| `tests/fixtures/openai_sse/` | Plaintext SSE fixtures (NEW directory) | Tasks 1, 2, 3 |

Each task corresponds to one git commit. Tasks are independently revertable; execution order 1→2→3→4→5 is recommended but technically commutative.

---

## Pre-flight (run once before starting Task 1)

- [ ] **Step P1: Confirm baseline builds clean**

```bash
cargo check -p alephcore 2>&1 | tail -20
```
Expected: `Finished ... [unoptimized + debuginfo] target(s) in Xs` with no errors. If errors exist that aren't introduced by this plan, stop and investigate.

- [ ] **Step P2: Note current commit**

```bash
git log -1 --oneline
```
Record this hash. Each task should commit on top of this base in order.

- [ ] **Step P3: Create fixture directory**

```bash
mkdir -p tests/fixtures/openai_sse
```
Empty for now; Tasks 1/2/3 each drop one fixture file in.

---

## Task 1: B2-Chat — Populate `cache_read_tokens` and `thinking_tokens` from OpenAI Chat usage payload

**Files:**
- Modify: `src/providers/openai/types.rs:181-188` — extend `OpenAiUsage` with `prompt_tokens_details` and `completion_tokens_details`
- Modify: `src/providers/protocols/openai_chat/sse.rs:88-108` — replace three hardcoded `None` lines with parsed values
- Test: `src/providers/protocols/openai_chat/tests.rs` (append)
- Create: `tests/fixtures/openai_sse/chat_completion_with_cache.txt`

- [ ] **Step 1.1: Create the fixture**

Write `tests/fixtures/openai_sse/chat_completion_with_cache.txt` containing a single SSE chunk that includes both new sub-fields. This file is checked in as the authoritative example wire shape.

```
data: {"id":"chatcmpl-test","object":"chat.completion.chunk","created":1700000000,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":50,"total_tokens":150,"prompt_tokens_details":{"cached_tokens":80},"completion_tokens_details":{"reasoning_tokens":30}}}

data: [DONE]

```

(Two trailing newlines after `[DONE]` are intentional — matches OpenAI SSE format.)

- [ ] **Step 1.2: Write the failing test (red)**

Append to `src/providers/protocols/openai_chat/tests.rs`. Skim the file first to follow its existing test style (top of file imports + helper functions); add this test in the appropriate section near other usage-parsing tests.

```rust
#[test]
fn openai_chat_usage_deserializes_cache_and_reasoning_tokens() {
    // Fixture intentionally contains both prompt_tokens_details.cached_tokens
    // and completion_tokens_details.reasoning_tokens.
    let fixture = include_str!("../../../../tests/fixtures/openai_sse/chat_completion_with_cache.txt");

    // The chunk-line we care about (single SSE event).
    let json_line = fixture
        .lines()
        .find(|l| l.starts_with("data: {"))
        .expect("fixture must contain a data: JSON line")
        .strip_prefix("data: ")
        .unwrap();

    let value: serde_json::Value = serde_json::from_str(json_line).unwrap();

    // Call the public SSE-event parser used by the streaming adapter.
    // (If the function lives at a different path, adapt the import — see sse.rs
    //  top of file for the actual fn name. The relevant fn is the one that
    //  emits ProviderDelta::Usage when the chunk carries `usage`.)
    let mut collected: std::collections::VecDeque<
        crate::providers::Result<crate::providers::ProviderDelta>,
    > = Default::default();
    super::sse::parse_chat_sse_event(&value, &mut collected, &mut Default::default());

    // Find the Usage delta.
    let usage_delta = collected
        .iter()
        .find_map(|res| match res {
            Ok(crate::providers::ProviderDelta::Usage(u)) => Some(u),
            _ => None,
        })
        .expect("expected a ProviderDelta::Usage emission");

    assert_eq!(usage_delta.input_tokens, 100);
    assert_eq!(usage_delta.output_tokens, 50);
    assert_eq!(usage_delta.cache_read_tokens, Some(80));
    assert_eq!(usage_delta.thinking_tokens, Some(30));
    assert_eq!(usage_delta.cache_creation_tokens, None); // Chat API never surfaces cache-write
    assert_eq!(usage_delta.cost, None);
}
```

If the actual parser signature differs (e.g., it takes raw bytes or a different intermediate type), inspect `sse.rs` and adapt the call so the test compiles, but keep the assertions identical.

- [ ] **Step 1.3: Run test to verify it fails**

```bash
cargo test -p alephcore --lib openai_chat_usage_deserializes_cache_and_reasoning_tokens 2>&1 | tail -30
```
Expected: FAIL — either the new sub-struct fields are missing from `OpenAiUsage` (compile error), or the canonical `TokenUsage` carries `None` for `cache_read_tokens` and `thinking_tokens` (assertion failure).

- [ ] **Step 1.4: Extend `OpenAiUsage` struct**

Edit `src/providers/openai/types.rs`. Locate the existing struct (currently around line 181-188):

```rust
/// Token usage statistics from OpenAI API
#[derive(Debug, Deserialize)]
pub struct OpenAiUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    #[allow(dead_code)] // Deserialized from API response
    pub total_tokens: Option<u32>,
}
```

Replace with:

```rust
/// Token usage statistics from OpenAI API
#[derive(Debug, Deserialize)]
pub struct OpenAiUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    #[allow(dead_code)] // Deserialized from API response
    pub total_tokens: Option<u32>,
    /// Breakdown of prompt tokens (cache_read).
    /// OpenAI returns this on `gpt-4o*` and later when prompt caching applies.
    #[serde(default)]
    pub prompt_tokens_details: Option<OpenAiPromptTokensDetails>,
    /// Breakdown of completion tokens (reasoning for o1/o3).
    #[serde(default)]
    pub completion_tokens_details: Option<OpenAiCompletionTokensDetails>,
}

/// Sub-payload: prompt token breakdown.
#[derive(Debug, Default, Deserialize)]
pub struct OpenAiPromptTokensDetails {
    #[serde(default)]
    pub cached_tokens: Option<u32>,
}

/// Sub-payload: completion token breakdown.
#[derive(Debug, Default, Deserialize)]
pub struct OpenAiCompletionTokensDetails {
    #[serde(default)]
    pub reasoning_tokens: Option<u32>,
}
```

- [ ] **Step 1.5: Wire the parsed values into `ProviderDelta::Usage` in Chat SSE**

Edit `src/providers/protocols/openai_chat/sse.rs:103-105`. The current `Usage` construction is:

```rust
ProviderDelta::Usage(TokenUsage {
    input_tokens: usage.prompt_tokens,
    output_tokens: usage.completion_tokens,
    cache_read_tokens: None,
    cache_creation_tokens: None,
    thinking_tokens: None,
    cost: None,
})
```

Lift cache_read and reasoning before the construction (so the field-init expressions stay readable), then plug them in:

```rust
let cache_read_tokens = usage
    .prompt_tokens_details
    .as_ref()
    .and_then(|d| d.cached_tokens);
let thinking_tokens = usage
    .completion_tokens_details
    .as_ref()
    .and_then(|d| d.reasoning_tokens);

ProviderDelta::Usage(TokenUsage {
    input_tokens: usage.prompt_tokens,
    output_tokens: usage.completion_tokens,
    cache_read_tokens,
    cache_creation_tokens: None, // OpenAI Chat does not surface cache-write
    thinking_tokens,
    cost: None,
})
```

Keep the trailing comment — it's the institutional knowledge that Chat (unlike Anthropic) does not expose a cache-creation metric.

- [ ] **Step 1.6: Run the cache test to confirm it passes (green)**

```bash
cargo test -p alephcore --lib openai_chat_usage_deserializes_cache_and_reasoning_tokens 2>&1 | tail -15
```
Expected: PASS.

- [ ] **Step 1.7: Add the "missing details" regression test**

Append to the same tests file:

```rust
#[test]
fn openai_chat_usage_handles_missing_details() {
    // Usage payload with only the legacy three fields — no *_tokens_details.
    let json_line = r#"{"id":"chatcmpl-x","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
    let value: serde_json::Value = serde_json::from_str(json_line).unwrap();

    let mut collected: std::collections::VecDeque<
        crate::providers::Result<crate::providers::ProviderDelta>,
    > = Default::default();
    super::sse::parse_chat_sse_event(&value, &mut collected, &mut Default::default());

    let usage_delta = collected
        .iter()
        .find_map(|res| match res {
            Ok(crate::providers::ProviderDelta::Usage(u)) => Some(u),
            _ => None,
        })
        .expect("usage delta must still be emitted with legacy-shaped payload");

    assert_eq!(usage_delta.input_tokens, 10);
    assert_eq!(usage_delta.output_tokens, 5);
    assert_eq!(usage_delta.cache_read_tokens, None);
    assert_eq!(usage_delta.thinking_tokens, None);
}
```

- [ ] **Step 1.8: Run both new tests + full Chat suite**

```bash
cargo test -p alephcore --lib openai_chat_usage 2>&1 | tail -10
cargo test -p alephcore --lib openai_chat 2>&1 | tail -10
```
Expected: both new tests PASS; existing Chat tests still PASS.

- [ ] **Step 1.9: Commit**

```bash
git add src/providers/openai/types.rs \
        src/providers/protocols/openai_chat/sse.rs \
        src/providers/protocols/openai_chat/tests.rs \
        tests/fixtures/openai_sse/chat_completion_with_cache.txt
git commit -m "providers/openai: populate cache_read and reasoning tokens on Chat path"
```

---

## Task 2: B2-Responses — Populate `cache_read_tokens` and `thinking_tokens` from Responses usage payload

**Files:**
- Modify: `src/providers/responses/types.rs:220-225` — extend `UsageInfo` with details sub-structs
- Modify: `src/providers/protocols/openai_responses/mod.rs:469-477` — replace three hardcoded `None` lines
- Test: `src/providers/protocols/openai_responses/tests.rs` (append)
- Create: `tests/fixtures/openai_sse/responses_with_cache_and_reasoning.txt`

- [ ] **Step 2.1: Create the fixture**

Write `tests/fixtures/openai_sse/responses_with_cache_and_reasoning.txt`:

```
event: response.completed
data: {"type":"response.completed","response":{"id":"resp_test","status":"completed","model":"gpt-4o","output":[{"type":"message","content":[]}],"usage":{"input_tokens":120,"output_tokens":40,"total_tokens":160,"input_tokens_details":{"cached_tokens":90},"output_tokens_details":{"reasoning_tokens":25}}}}

```

- [ ] **Step 2.2: Write the failing test (red)**

Append to `src/providers/protocols/openai_responses/tests.rs`:

```rust
#[test]
fn openai_responses_usage_deserializes_cache_and_reasoning_tokens() {
    let fixture = include_str!(
        "../../../../tests/fixtures/openai_sse/responses_with_cache_and_reasoning.txt"
    );

    // Extract the JSON payload from the `data:` line.
    let json_line = fixture
        .lines()
        .find(|l| l.starts_with("data: {"))
        .expect("fixture must contain a data: JSON line")
        .strip_prefix("data: ")
        .unwrap();

    let event: crate::providers::responses::types::StreamEvent =
        serde_json::from_str(json_line).unwrap();

    let mut out: std::collections::VecDeque<
        crate::providers::Result<crate::providers::ProviderDelta>,
    > = Default::default();
    let mut tracker = Default::default();
    super::dispatch_stream_event(event, &mut out, &mut tracker);

    let usage_delta = out
        .iter()
        .find_map(|res| match res {
            Ok(crate::providers::ProviderDelta::Usage(u)) => Some(u),
            _ => None,
        })
        .expect("Responses Completed should emit Usage delta");

    assert_eq!(usage_delta.input_tokens, 120);
    assert_eq!(usage_delta.output_tokens, 40);
    assert_eq!(usage_delta.cache_read_tokens, Some(90));
    assert_eq!(usage_delta.thinking_tokens, Some(25));
    assert_eq!(usage_delta.cache_creation_tokens, None); // Responses API does not surface cache-write
}
```

If `dispatch_stream_event` isn't the exact fn name, look at `openai_responses/mod.rs:444` for the helper that takes a `StreamEvent::Completed` and pushes into `out`. Adapt the call to match.

- [ ] **Step 2.3: Run test to confirm failure**

```bash
cargo test -p alephcore --lib openai_responses_usage_deserializes 2>&1 | tail -20
```
Expected: FAIL (`UsageInfo` doesn't have `input_tokens_details` field yet → compile error, OR canonical `cache_read_tokens` still `None`).

- [ ] **Step 2.4: Extend `UsageInfo` struct**

Edit `src/providers/responses/types.rs:220-226`. Current shape:

```rust
/// Token usage information
#[derive(Debug, Deserialize)]
pub struct UsageInfo {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
}
```

Replace with:

```rust
/// Token usage information
#[derive(Debug, Deserialize)]
pub struct UsageInfo {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
    /// Breakdown of input tokens (cache_read).
    #[serde(default)]
    pub input_tokens_details: Option<ResponsesInputTokensDetails>,
    /// Breakdown of output tokens (reasoning for o1/o3 reasoning models).
    #[serde(default)]
    pub output_tokens_details: Option<ResponsesOutputTokensDetails>,
}

/// Sub-payload: Responses input token breakdown.
#[derive(Debug, Default, Deserialize)]
pub struct ResponsesInputTokensDetails {
    #[serde(default)]
    pub cached_tokens: Option<u32>,
}

/// Sub-payload: Responses output token breakdown.
#[derive(Debug, Default, Deserialize)]
pub struct ResponsesOutputTokensDetails {
    #[serde(default)]
    pub reasoning_tokens: Option<u32>,
}
```

- [ ] **Step 2.5: Wire parsed values into `ProviderDelta::Usage` in Responses SSE handler**

Edit `src/providers/protocols/openai_responses/mod.rs` around line 469 — the `StreamEvent::Completed` arm. Locate the `if let Some(u) = response.usage { out.push_back(Ok(ProviderDelta::Usage(TokenUsage { ... })))` block and replace the inner `None` fields:

```rust
if let Some(u) = response.usage {
    let cache_read_tokens = u
        .input_tokens_details
        .as_ref()
        .and_then(|d| d.cached_tokens);
    let thinking_tokens = u
        .output_tokens_details
        .as_ref()
        .and_then(|d| d.reasoning_tokens);

    out.push_back(Ok(ProviderDelta::Usage(TokenUsage {
        input_tokens: u.input_tokens,
        output_tokens: u.output_tokens,
        cache_read_tokens,
        cache_creation_tokens: None, // Responses API does not surface cache-write
        thinking_tokens,
        cost: None,
    })));
}
```

- [ ] **Step 2.6: Run test (green)**

```bash
cargo test -p alephcore --lib openai_responses_usage_deserializes 2>&1 | tail -10
```
Expected: PASS.

- [ ] **Step 2.7: Add regression test for missing-details payload**

Append:

```rust
#[test]
fn openai_responses_usage_handles_missing_details() {
    let json_line = r#"{"type":"response.completed","response":{"id":"r","status":"completed","model":"gpt-4o","output":[],"usage":{"input_tokens":12,"output_tokens":6,"total_tokens":18}}}"#;
    let event: crate::providers::responses::types::StreamEvent =
        serde_json::from_str(json_line).unwrap();

    let mut out: std::collections::VecDeque<
        crate::providers::Result<crate::providers::ProviderDelta>,
    > = Default::default();
    let mut tracker = Default::default();
    super::dispatch_stream_event(event, &mut out, &mut tracker);

    let usage = out
        .iter()
        .find_map(|res| match res {
            Ok(crate::providers::ProviderDelta::Usage(u)) => Some(u),
            _ => None,
        })
        .expect("Usage delta should still be present");

    assert_eq!(usage.input_tokens, 12);
    assert_eq!(usage.output_tokens, 6);
    assert_eq!(usage.cache_read_tokens, None);
    assert_eq!(usage.thinking_tokens, None);
}
```

- [ ] **Step 2.8: Run both new tests + full Responses suite**

```bash
cargo test -p alephcore --lib openai_responses_usage 2>&1 | tail -10
cargo test -p alephcore --lib openai_responses 2>&1 | tail -10
```
Expected: PASS for both new, regression PASS for existing.

- [ ] **Step 2.9: Commit**

```bash
git add src/providers/responses/types.rs \
        src/providers/protocols/openai_responses/mod.rs \
        src/providers/protocols/openai_responses/tests.rs \
        tests/fixtures/openai_sse/responses_with_cache_and_reasoning.txt
git commit -m "providers/openai: populate cache_read and reasoning tokens on Responses path"
```

---

## Task 3: B3a — Explicit reasoning_summary_part_* event handling in Responses

**Files:**
- Modify: `src/providers/protocols/openai_responses/mod.rs:490-494` — expand catch-all into 4 explicit arms
- Test: `src/providers/protocols/openai_responses/tests.rs`
- Create: `tests/fixtures/openai_sse/responses_with_reasoning_summary_parts.txt`

- [ ] **Step 3.1: Create the fixture**

Write `tests/fixtures/openai_sse/responses_with_reasoning_summary_parts.txt`:

```
event: response.reasoning_summary_part.added
data: {"type":"response.reasoning_summary_part.added","item_id":"item_1","output_index":0,"summary_index":0,"part":{"type":"summary_text","text":""}}

event: response.reasoning_summary_text.delta
data: {"type":"response.reasoning_summary_text.delta","item_id":"item_1","output_index":0,"summary_index":0,"delta":"thinking..."}

event: response.reasoning_summary_text.done
data: {"type":"response.reasoning_summary_text.done","item_id":"item_1","output_index":0,"summary_index":0,"text":"thinking..."}

event: response.reasoning_summary_part.done
data: {"type":"response.reasoning_summary_part.done","item_id":"item_1","output_index":0,"summary_index":0,"part":{"type":"summary_text","text":"thinking..."}}

```

The four events arrive in this order in real Responses streams.

- [ ] **Step 3.2: Write the failing tests (red)**

Append to `openai_responses/tests.rs`:

```rust
#[test]
fn responses_reasoning_summary_part_added_emits_no_delta() {
    let json = r#"{"type":"response.reasoning_summary_part.added","item_id":"x","output_index":0,"summary_index":0,"part":{"type":"summary_text","text":""}}"#;
    let event: crate::providers::responses::types::StreamEvent =
        serde_json::from_str(json).unwrap();
    let mut out = std::collections::VecDeque::new();
    let mut tracker = Default::default();
    super::dispatch_stream_event(event, &mut out, &mut tracker);
    assert_eq!(out.len(), 0, "part.added should not emit any delta");
}

#[test]
fn responses_reasoning_summary_text_delta_emits_thinking() {
    let json = r#"{"type":"response.reasoning_summary_text.delta","item_id":"x","output_index":0,"summary_index":0,"delta":"abc"}"#;
    let event: crate::providers::responses::types::StreamEvent =
        serde_json::from_str(json).unwrap();
    let mut out = std::collections::VecDeque::new();
    let mut tracker = Default::default();
    super::dispatch_stream_event(event, &mut out, &mut tracker);
    let delta = out.front().expect("expected one delta");
    match delta {
        Ok(crate::providers::ProviderDelta::ThinkingDelta(s)) => assert_eq!(s, "abc"),
        other => panic!("expected ThinkingDelta, got {:?}", other),
    }
}

#[test]
fn responses_reasoning_summary_text_done_emits_no_delta() {
    let json = r#"{"type":"response.reasoning_summary_text.done","item_id":"x","output_index":0,"summary_index":0,"text":"abc"}"#;
    let event: crate::providers::responses::types::StreamEvent =
        serde_json::from_str(json).unwrap();
    let mut out = std::collections::VecDeque::new();
    let mut tracker = Default::default();
    super::dispatch_stream_event(event, &mut out, &mut tracker);
    assert_eq!(out.len(), 0, "text.done should not emit (already accumulated)");
}

#[test]
fn responses_reasoning_summary_part_done_emits_no_delta() {
    let json = r#"{"type":"response.reasoning_summary_part.done","item_id":"x","output_index":0,"summary_index":0,"part":{"type":"summary_text","text":"abc"}}"#;
    let event: crate::providers::responses::types::StreamEvent =
        serde_json::from_str(json).unwrap();
    let mut out = std::collections::VecDeque::new();
    let mut tracker = Default::default();
    super::dispatch_stream_event(event, &mut out, &mut tracker);
    assert_eq!(out.len(), 0, "part.done should not emit");
}
```

- [ ] **Step 3.3: Run tests; the first three already pass (no-emit is the current behavior because of `_ => {}`), but verify by running**

```bash
cargo test -p alephcore --lib responses_reasoning_summary 2>&1 | tail -15
```
Expected: all 4 PASS even before the change. The point of this task is **regression-locking** the no-emit behavior so future refactors can't silently start emitting unintended deltas, AND making the silent drop visible via debug logs.

- [ ] **Step 3.4: Replace the `_ => {}` catch-all with explicit arms**

Edit `src/providers/protocols/openai_responses/mod.rs:490-494`. Current shape:

```rust
StreamEvent::ReasoningSummaryTextDelta { delta, .. } => {
    out.push_back(Ok(ProviderDelta::ThinkingDelta(delta)));
}

_ => {}
```

Replace with:

```rust
StreamEvent::ReasoningSummaryPartAdded { .. } => {
    tracing::debug!(
        target: "aleph::openai_responses_sse",
        "reasoning_summary_part.added — boundary marker, no canonical delta emitted"
    );
}
StreamEvent::ReasoningSummaryTextDelta { delta, .. } => {
    out.push_back(Ok(ProviderDelta::ThinkingDelta(delta)));
}
StreamEvent::ReasoningSummaryTextDone { .. } => {
    tracing::debug!(
        target: "aleph::openai_responses_sse",
        "reasoning_summary_text.done — content already accumulated via delta events"
    );
}
StreamEvent::ReasoningSummaryPartDone { .. } => {
    tracing::debug!(
        target: "aleph::openai_responses_sse",
        "reasoning_summary_part.done — boundary marker, no canonical delta emitted"
    );
}
_ => {}
```

The trailing `_ => {}` stays — it catches any genuinely-unhandled future StreamEvent variants.

- [ ] **Step 3.5: Re-run the 4 tests + full Responses suite (green check)**

```bash
cargo test -p alephcore --lib responses_reasoning_summary 2>&1 | tail -10
cargo test -p alephcore --lib openai_responses 2>&1 | tail -10
```
Expected: all 4 still PASS; existing Responses tests still PASS.

- [ ] **Step 3.6: Commit**

```bash
git add src/providers/protocols/openai_responses/mod.rs \
        src/providers/protocols/openai_responses/tests.rs \
        tests/fixtures/openai_sse/responses_with_reasoning_summary_parts.txt
git commit -m "providers/openai: explicit reasoning_summary_part event handling in Responses"
```

---

## Task 4: B3b — Expand Chat finish_reason mapping and warn on unknown

**Files:**
- Modify: `src/providers/protocols/openai_chat/sse.rs:111-119`
- Test: `src/providers/protocols/openai_chat/tests.rs` (append with rstest)

- [ ] **Step 4.1: Write the failing tests (red)**

Append to `openai_chat/tests.rs`. Use `rstest` (already a Cargo dev-dependency per `Cargo.toml`):

```rust
#[rstest::rstest]
#[case::stop("stop", crate::providers::StopReason::EndTurn)]
#[case::tool_calls("tool_calls", crate::providers::StopReason::ToolUse)]
#[case::function_call("function_call", crate::providers::StopReason::ToolUse)]
#[case::length("length", crate::providers::StopReason::MaxTokens)]
#[case::content_filter("content_filter", crate::providers::StopReason::MaxTokens)]
#[case::content_policy_violation("content_policy_violation", crate::providers::StopReason::MaxTokens)]
#[case::incomplete("incomplete", crate::providers::StopReason::MaxTokens)]
fn chat_finish_reason_maps_correctly(
    #[case] input: &str,
    #[case] expected: crate::providers::StopReason,
) {
    let json_line = format!(
        r#"{{"id":"x","choices":[{{"index":0,"delta":{{}},"finish_reason":"{}"}}],"usage":null}}"#,
        input
    );
    let value: serde_json::Value = serde_json::from_str(&json_line).unwrap();
    let mut out = std::collections::VecDeque::new();
    let mut tracker = Default::default();
    super::sse::parse_chat_sse_event(&value, &mut out, &mut tracker);

    let done = out
        .iter()
        .find_map(|r| match r {
            Ok(crate::providers::ProviderDelta::Done(reason)) => Some(reason),
            _ => None,
        })
        .copied();
    assert_eq!(done, Some(expected), "finish_reason `{}` mapping wrong", input);
}

#[test]
fn chat_finish_reason_unknown_falls_back_to_endturn() {
    let json_line = r#"{"id":"x","choices":[{"index":0,"delta":{},"finish_reason":"some_future_reason"}],"usage":null}"#;
    let value: serde_json::Value = serde_json::from_str(json_line).unwrap();
    let mut out = std::collections::VecDeque::new();
    let mut tracker = Default::default();
    super::sse::parse_chat_sse_event(&value, &mut out, &mut tracker);

    let done = out
        .iter()
        .find_map(|r| match r {
            Ok(crate::providers::ProviderDelta::Done(reason)) => Some(reason),
            _ => None,
        })
        .copied();
    assert_eq!(
        done,
        Some(crate::providers::StopReason::EndTurn),
        "unknown finish_reason must fall back to EndTurn (not None — that hangs the loop)"
    );
}
```

- [ ] **Step 4.2: Run to confirm failures**

```bash
cargo test -p alephcore --lib chat_finish_reason 2>&1 | tail -20
```
Expected: FAIL — `function_call`, `content_policy_violation`, `incomplete`, and unknown all currently map to `None`.

- [ ] **Step 4.3: Replace the finish_reason match**

Edit `src/providers/protocols/openai_chat/sse.rs:111-119`. Current shape:

```rust
let finish_reason = choice.get("finish_reason").and_then(|r| r.as_str());

if let Some(reason) = finish_reason {
    let stop = match reason {
        "stop" => Some(StopReason::EndTurn),
        "tool_calls" => Some(StopReason::ToolUse),
        "length" | "content_filter" => Some(StopReason::MaxTokens),
        _ => None,
    };
    // ... existing emission logic
}
```

Replace the `match` body with:

```rust
let stop = match reason {
    "stop" => Some(StopReason::EndTurn),
    "tool_calls" | "function_call" => Some(StopReason::ToolUse),
    "length" => Some(StopReason::MaxTokens),
    "content_filter" | "content_policy_violation" => Some(StopReason::MaxTokens),
    "incomplete" => Some(StopReason::MaxTokens),
    other => {
        tracing::warn!(
            target: "aleph::openai_chat_sse",
            finish_reason = other,
            "unknown finish_reason from OpenAI Chat; defaulting to EndTurn"
        );
        Some(StopReason::EndTurn)
    }
};
```

If `tracing::warn!` isn't already in scope at the top of the file, add `use tracing::warn;` to the use-block. (Many Aleph files prefer the macro path `tracing::warn!` inline — either is fine; match the file's existing convention.)

- [ ] **Step 4.4: Run tests (green)**

```bash
cargo test -p alephcore --lib chat_finish_reason 2>&1 | tail -15
cargo test -p alephcore --lib openai_chat 2>&1 | tail -10
```
Expected: all 8 cases PASS; existing Chat tests still PASS.

- [ ] **Step 4.5: Commit**

```bash
git add src/providers/protocols/openai_chat/sse.rs \
        src/providers/protocols/openai_chat/tests.rs
git commit -m "providers/openai: expand Chat finish_reason mapping and warn on unknown"
```

---

## Task 5: B4 — Wire `stop_sequences` into Chat (body json) and Responses (struct field)

**Files:**
- Modify: `src/providers/responses/types.rs` — add `stop: Option<Vec<String>>` to `ResponsesRequest`
- Modify: `src/providers/protocols/openai_chat/adapter.rs` — inject `body["stop"] = json!(vec)` in `build_request`
- Modify: `src/providers/protocols/openai_responses/mod.rs` — populate `ResponsesRequest.stop` in `build_responses_request`
- Test: `src/providers/protocols/openai_chat/tests.rs`, `src/providers/protocols/openai_responses/tests.rs`

- [ ] **Step 5.1: Write the failing Chat test (red)**

Append to `openai_chat/tests.rs`. The test inspects the request body that `OpenAiProtocol::build_request` produces. Helper for that (look for existing tests with the pattern `let req = OpenAiProtocol::new(...).build_request(...)` and check how they extract the body — typically via `req.build()?.body().unwrap().as_bytes()`).

```rust
#[test]
fn chat_stop_sequences_serializes_into_request() {
    let cfg = crate::config::ProviderConfig {
        stop_sequences: Some("END,STOP".into()),
        ..Default::default()
    };
    let proto = super::OpenAiProtocol::new(reqwest::Client::new());
    let payload = crate::providers::adapter::RequestPayload {
        model: Some("gpt-4o".into()),
        messages: vec![crate::providers::message::UnifiedMessage::user("hi")],
        ..Default::default()
    };
    let req_builder = proto.build_request(&payload, &cfg).unwrap();
    let req = req_builder.build().unwrap();
    let body_bytes = req.body().unwrap().as_bytes().unwrap();
    let body: serde_json::Value = serde_json::from_slice(body_bytes).unwrap();

    assert_eq!(body["stop"], serde_json::json!(["END", "STOP"]));
}

#[rstest::rstest]
#[case::none(None)]
#[case::empty(Some("".to_string()))]
#[case::only_commas(Some(",,".to_string()))]
fn chat_stop_sequences_omits_field_when_empty(#[case] stop_sequences: Option<String>) {
    let cfg = crate::config::ProviderConfig {
        stop_sequences,
        ..Default::default()
    };
    let proto = super::OpenAiProtocol::new(reqwest::Client::new());
    let payload = crate::providers::adapter::RequestPayload {
        model: Some("gpt-4o".into()),
        messages: vec![crate::providers::message::UnifiedMessage::user("hi")],
        ..Default::default()
    };
    let req = proto.build_request(&payload, &cfg).unwrap().build().unwrap();
    let body: serde_json::Value = serde_json::from_slice(req.body().unwrap().as_bytes().unwrap()).unwrap();

    assert!(body.get("stop").is_none(), "stop field must be absent");
}

#[test]
fn chat_stop_sequences_trims_whitespace() {
    let cfg = crate::config::ProviderConfig {
        stop_sequences: Some(" END , STOP ".into()),
        ..Default::default()
    };
    let proto = super::OpenAiProtocol::new(reqwest::Client::new());
    let payload = crate::providers::adapter::RequestPayload {
        model: Some("gpt-4o".into()),
        messages: vec![crate::providers::message::UnifiedMessage::user("hi")],
        ..Default::default()
    };
    let req = proto.build_request(&payload, &cfg).unwrap().build().unwrap();
    let body: serde_json::Value = serde_json::from_slice(req.body().unwrap().as_bytes().unwrap()).unwrap();

    assert_eq!(body["stop"], serde_json::json!(["END", "STOP"]));
}
```

If `UnifiedMessage::user` or `RequestPayload::default()` shape differs, look at neighboring tests in the same file for the canonical helper and adapt — but keep the assertion semantics identical (body has correct `stop` field).

- [ ] **Step 5.2: Run to confirm failure**

```bash
cargo test -p alephcore --lib chat_stop_sequences 2>&1 | tail -15
```
Expected: FAIL (body has no `stop` key in the populated case).

- [ ] **Step 5.3: Inject `body["stop"]` in Chat adapter**

Edit `src/providers/protocols/openai_chat/adapter.rs`. Locate the block in `build_request` that conditionally injects per-config fields (between the temperature block at ~line 45 and the reasoning_effort block at ~line 60). Insert before the tool-handling section:

```rust
// stop sequences: parse comma-separated config value, trim, drop empties
if let Some(raw) = config.stop_sequences.as_ref() {
    let sequences: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if !sequences.is_empty() {
        body["stop"] = json!(sequences);
    }
}
```

- [ ] **Step 5.4: Run Chat tests (green)**

```bash
cargo test -p alephcore --lib chat_stop_sequences 2>&1 | tail -15
cargo test -p alephcore --lib openai_chat 2>&1 | tail -10
```
Expected: 5 new tests PASS; existing PASS.

- [ ] **Step 5.5: Add `stop` field to `ResponsesRequest`**

Edit `src/providers/responses/types.rs`. Find `ResponsesRequest` (the struct holding `model`, `input`, `instructions`, etc.). Add:

```rust
#[serde(skip_serializing_if = "Option::is_none")]
pub stop: Option<Vec<String>>,
```

Place it near `max_output_tokens` to keep semantically-related fields together.

- [ ] **Step 5.6: Populate `stop` in `build_responses_request`**

Edit `src/providers/protocols/openai_responses/mod.rs` around line 156 (the `ResponsesRequest { ... }` literal at the bottom of `build_responses_request`). Before constructing the literal, lift the parsed sequences:

```rust
let stop = config.stop_sequences.as_ref().and_then(|raw| {
    let v: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if v.is_empty() { None } else { Some(v) }
});
```

Then add `stop,` as a field-init shortcut in the `ResponsesRequest { ... }` literal.

- [ ] **Step 5.7: Write the Responses test (red→green in one shot, post-impl)**

Append to `openai_responses/tests.rs`:

```rust
#[test]
fn responses_stop_sequences_serializes_into_request() {
    let cfg = crate::config::ProviderConfig {
        stop_sequences: Some("END,STOP".into()),
        ..Default::default()
    };
    let payload = crate::providers::adapter::RequestPayload {
        model: Some("gpt-4o".into()),
        messages: vec![crate::providers::message::UnifiedMessage::user("hi")],
        ..Default::default()
    };
    let variant = crate::providers::protocols::openai_responses::ResponsesVariant::default();
    let req = super::OpenAiResponsesProtocol::build_responses_request(
        &payload, "gpt-4o", &variant, &cfg
    );
    let body = serde_json::to_value(&req).unwrap();
    assert_eq!(body["stop"], serde_json::json!(["END", "STOP"]));
}

#[test]
fn responses_stop_sequences_none_omits_field() {
    let cfg = crate::config::ProviderConfig {
        stop_sequences: None,
        ..Default::default()
    };
    let payload = crate::providers::adapter::RequestPayload {
        model: Some("gpt-4o".into()),
        messages: vec![crate::providers::message::UnifiedMessage::user("hi")],
        ..Default::default()
    };
    let variant = crate::providers::protocols::openai_responses::ResponsesVariant::default();
    let req = super::OpenAiResponsesProtocol::build_responses_request(
        &payload, "gpt-4o", &variant, &cfg
    );
    let body = serde_json::to_value(&req).unwrap();
    assert!(body.get("stop").is_none(), "stop field must be absent");
}
```

If `ResponsesVariant::default()` doesn't exist, look at neighboring tests for the canonical way to obtain a `ResponsesVariant` instance and adapt. The two assertions are the contract.

Note this also requires `ResponsesRequest` to derive `Serialize`. If it doesn't, add `Serialize` to its derive macro list. (Likely already there — it's serialized to a request body.)

- [ ] **Step 5.8: Run Responses tests**

```bash
cargo test -p alephcore --lib responses_stop_sequences 2>&1 | tail -10
cargo test -p alephcore --lib openai_responses 2>&1 | tail -10
```
Expected: PASS for both new; existing PASS.

- [ ] **Step 5.9: Full Cycle 1 regression check**

```bash
cargo test -p alephcore --lib 2>&1 | tail -20
```
Expected: full lib suite green (whatever was green before, still green).

- [ ] **Step 5.10: Lint check**

```bash
cargo clippy -p alephcore --lib --no-deps 2>&1 | tail -20
```
Expected: no new lints on the 6 touched source files. If clippy complains about any new code, fix inline before committing.

- [ ] **Step 5.11: Commit**

```bash
git add src/providers/responses/types.rs \
        src/providers/protocols/openai_chat/adapter.rs \
        src/providers/protocols/openai_responses/mod.rs \
        src/providers/protocols/openai_chat/tests.rs \
        src/providers/protocols/openai_responses/tests.rs
git commit -m "providers/openai: wire ProviderConfig.stop_sequences into Chat and Responses requests"
```

---

## Post-Cycle 1 verification

- [ ] **Step Q1: Five commits land cleanly**

```bash
git log --oneline -6
```
Expected: 5 new commits on top of the pre-flight base hash, each titled per the commit-plan in §9 of the spec.

- [ ] **Step Q2: Append CHANGELOG entries**

Edit `CHANGELOG.md` `[Unreleased]` section. Under `### Fixed`:

```
- providers/openai: cache_read_tokens and reasoning_tokens are now extracted from OpenAI Chat and Responses usage payloads (were previously hardcoded to None). MeteringProvider tracing logs now show real cache hit / reasoning token counts.
- providers/openai: Chat finish_reason mapping now covers function_call, content_policy_violation, and incomplete, and unknown reasons fall back to EndTurn with a warning (was silently None which could hang the stream loop).
- providers/openai: reasoning_summary_part.added, .text.done, and .part.done events are now explicitly logged at debug level instead of being silently dropped.
```

Under `### Added`:

```
- providers/openai: ProviderConfig.stop_sequences (comma-separated) is now forwarded to both OpenAI Chat and Responses requests as the `stop` field. Empty / whitespace-only entries are filtered.
- tests/fixtures/openai_sse/: new directory of plaintext SSE fixtures captured for regression testing.
```

Commit:

```bash
git add CHANGELOG.md
git commit -m "docs: changelog for OpenAI provider token & events wiring (Cycle 1)"
```

- [ ] **Step Q3: Manual e2e verification**

Spin up the dev server:

```bash
cargo run --bin aleph-server 2>&1 &
```

Through webchat (or `aleph` CLI), send two consecutive turns to an OpenAI-Chat or OpenAI-Responses provider that supports prompt caching (current good candidates: `T8Star` for Responses, or any kimi/deepseek-via-OpenAI-protocol provider for Chat with caching enabled).

Watch the server log for:

```
INFO aleph::provider_usage agent_id=... provider=...
     input_tokens=... output_tokens=...
     cache_read_tokens=Some(N>0)        ← key check
     cache_creation_tokens=None
     thinking_tokens=...
     "LLM call completed"
```

Second turn's `cache_read_tokens` should be `Some(N)` with `N > 0`. This validates B2 end-to-end through the `MeteringProvider` tracing layer (the Anthropic-side spec `2026-05-11-cache-token-observability.md` already shipped the tracing emission; our changes are what populate the canonical fields).

If the e2e doesn't show the expected fields, troubleshoot:
- Is the provider actually one that returns cache hits in the wire response? (Try a longer system prompt to maximize cache reuse.)
- Is the path going through the OpenAI Chat protocol or Anthropic protocol? Memory note `[Provider → Wire Protocol Mapping]` records which providers use which protocol — make sure the test provider goes through OpenAI.

---

## Self-Review

**Spec coverage:**

| Spec section | Task(s) | Status |
|---|---|---|
| §4.1 B2 Chat OpenAiUsage extension + sse.rs wiring | Task 1 | ✅ Covered |
| §4.2 B2 Responses UsageInfo extension + mod.rs wiring | Task 2 | ✅ Covered |
| §4.3 B2 4 deserialize tests | Tasks 1.2/1.7, 2.2/2.7 | ✅ Covered |
| §5.1 B3a 4 reasoning_summary explicit arms | Task 3 | ✅ Covered |
| §5.2 B3b finish_reason expansion + warn-on-unknown | Task 4 | ✅ Covered |
| §5.3 B3 8 tests | Tasks 3.2 (4 cases), 4.1 (rstest 7 + 1) | ✅ Covered |
| §6.1 B4 ChatCompletionRequest stop field | Task 5 (adapter `body["stop"]` instead; spec deviation noted) | ✅ Covered with intentional pattern alignment |
| §6.2 B4 adapter wiring (Chat + Responses) | Task 5.3, 5.6 | ✅ Covered |
| §6.3 B4 7 tests | Task 5.1 (5 cases) + 5.7 (2 cases) | ✅ Covered |
| §7.1 fixture directory | Step P3 + Tasks 1.1, 2.1, 3.1 | ✅ 3 of 4 fixtures created (the 4th `chat_completion_with_reasoning.txt` is YAGNI — Task 1's fixture already exercises reasoning_tokens) |
| §9 5-commit plan | Tasks 1-5 commits | ✅ Covered |
| §11 acceptance criteria (19 tests) | Tasks 1-5 | ✅ Total: Task 1 (2) + Task 2 (2) + Task 3 (4) + Task 4 (8) + Task 5 (5+2) = 23 tests; exceeds the 19-test floor |

**Spec deviation note:** Spec §6.1 proposed adding a `stop: Option<Vec<String>>` field to the typed `ChatCompletionRequest` struct in `openai/types.rs`. The actual Chat protocol adapter at `openai_chat/adapter.rs` does NOT serialize that struct; it builds the request via the `json!()` macro inline. Following the existing pattern is more conservative than introducing an unused struct field. The plan adds `body["stop"] = json!(...)` directly. The typed struct change is skipped (and so is the parallel addition for `request.rs` builder functions which appear to be a separate / partially-unused code path).

**Placeholder scan:** No TBD / TODO / "implement later" / "add appropriate X". Each step has exact code or exact command. ✅

**Type consistency check:**
- `OpenAiPromptTokensDetails::cached_tokens` matches usage in `sse.rs` `.prompt_tokens_details.as_ref().and_then(|d| d.cached_tokens)`. ✅
- `OpenAiCompletionTokensDetails::reasoning_tokens` matches `.completion_tokens_details.as_ref().and_then(|d| d.reasoning_tokens)`. ✅
- `ResponsesInputTokensDetails::cached_tokens` + `ResponsesOutputTokensDetails::reasoning_tokens` paired correctly. ✅
- `TokenUsage` includes `cost: None` (canonical struct has this field; both Task 1 and Task 2 set it). ✅
- `StopReason` enum: `EndTurn`, `ToolUse`, `MaxTokens` used consistently across Task 4. ✅
- `UsageInfo` is the actual struct name (not `ResponsesUsage` as the spec's placeholder said). Plan uses `UsageInfo`. ✅

No drift between tasks.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-12-openai-protocol-token-and-events-wiring.md`. Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration. Each Task 1-5 becomes a self-contained subagent invocation, and after each commit I verify the commit before dispatching the next.

**2. Inline Execution** — Execute tasks in this session using `executing-plans`, batch execution with checkpoints for review.

Which approach?
