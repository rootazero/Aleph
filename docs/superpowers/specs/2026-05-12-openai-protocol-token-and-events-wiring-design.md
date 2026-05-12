# OpenAI Protocol — Token & Event Wiring Design

> **Date:** 2026-05-12
> **Status:** Design draft, pending user review.
> **Scope:** Provider client (consumer) side only. OpenAI Chat Completions + OpenAI Responses protocols.
> **Predecessors:** `a035e76a7` (Spec A curated hot memory ship), `6edf18f73` (Step 2 prompt-cache HEAD).
> **Sibling specs:**
> - `2026-05-11-openai-protocol-optimization-design.md` (the 4-module overhaul; M1/M2/M4 already shipped, M3 retry_policy.rs deferred)
> - `2026-05-11-openai-responses-strict-multi-type-fix.md` (B1 desktop schema fix; approved, awaiting implementation)
> - `2026-05-11-cache-token-observability.md` (Anthropic-side `MeteringProvider` tracing emit; OpenAI side downstream needs to populate canonical fields first — this spec)

---

## 1. Problem Statement

Three independent gaps remain in the OpenAI Provider client side after the 2026-05-11 module overhaul shipped. All three are pure "infrastructure exists, wires missing" — no new abstractions are introduced.

### B2 — Canonical TokenUsage fields hardcoded to `None`

`src/providers/protocols/openai_chat/sse.rs:103-105`:

```rust
cache_read_tokens: None,
cache_creation_tokens: None,
thinking_tokens: None,
```

`src/providers/protocols/openai_responses/mod.rs:472-474`: same three lines.

The canonical `TokenUsage` struct already has these fields. The Anthropic adapter populates them. OpenAI's `OpenAiUsage` deserialize struct (`src/providers/openai/types.rs:183-188`) only carries `prompt_tokens / completion_tokens / total_tokens` — the API actually returns `prompt_tokens_details.cached_tokens` (Chat) / `input_tokens_details.cached_tokens` (Responses) and `completion_tokens_details.reasoning_tokens` / `output_tokens_details.reasoning_tokens`, but neither is deserialized.

Downstream `MeteringProvider::process` (per `2026-05-11-cache-token-observability.md`) is set up to `tracing::info!` the canonical TokenUsage fields, but for OpenAI providers all three fields will stream out as `None` because the canonical struct is never populated.

### B3 — SSE event coverage gaps

**B3a — Responses `reasoning_summary_part_*` events silently dropped.**
`src/providers/responses/types.rs:309-336` defines four reasoning_summary StreamEvent variants. `src/providers/protocols/openai_responses/mod.rs:480-494` matches `ReasoningSummaryTextDelta` only; the other three (`ReasoningSummaryPartAdded`, `ReasoningSummaryTextDone`, `ReasoningSummaryPartDone`) fall into a default `_ => {}` arm and disappear without trace.

**B3b — Chat finish_reason mapping incomplete.**
`src/providers/protocols/openai_chat/sse.rs:114-119`:

```rust
"stop" => Some(StopReason::EndTurn),
"tool_calls" => Some(StopReason::ToolUse),
"length" | "content_filter" => Some(StopReason::MaxTokens),
_ => None,
```

Two consequences:
- Legacy `function_call` finish_reason → `None` (caller may treat as "stream not done")
- New `content_policy_violation` / `incomplete` finish reasons (returned by current OpenAI Chat for moderation/length cutoffs) → `None`
- Unknown values → silently `None`, no warning

The Responses-side variant at `mod.rs:445` already handles `status == "incomplete"` correctly. Chat side is misaligned.

### B4 — `stop_sequences` config never reaches OpenAI requests

`src/config/types/provider.rs:126` defines `pub stop_sequences: Option<String>` (comma-separated). `src/providers/protocols/template.rs:67` consumes it for the template-protocol path. Both OpenAI protocols ignore it. The Anthropic protocol consumes it via its own request builder.

Result: users who configure `stop_sequences = "END,STOP"` for an OpenAI provider see the field silently ignored.

---

## 2. Goals / Non-Goals

### Goals
- Populate canonical `TokenUsage.cache_read_tokens / cache_creation_tokens / thinking_tokens` on the OpenAI Chat + Responses streaming paths.
- Cover all four Responses reasoning_summary StreamEvent variants explicitly (no silent drops).
- Expand Chat finish_reason mapping to include legacy `function_call`, new `content_policy_violation`, `incomplete`, and warn-then-fallback for unknown values.
- Wire `ProviderConfig.stop_sequences` into both OpenAI Chat and Responses request builders.

### Non-Goals
- No changes to canonical `TokenUsage` shape or `ProtocolAdapter` trait.
- No re-implementation of `MeteringProvider` tracing (handled by `2026-05-11-cache-token-observability.md`).
- No new common modules or trait abstractions.
- No retry_policy implementation (deferred — M3 of the 4-module overhaul).
- No B1 desktop schema fix (separate approved spec).
- No Anthropic-side changes.

---

## 3. Architecture Compliance

| Redline | Status |
|---|---|
| **R3 Core Minimalism** | ✅ No new modules; expand existing struct fields and match arms only. |
| **R7 LLM Sovereignty** | ✅ All changes are deterministic deserialization, mechanical mapping, and field wiring. No reasoning logic added. |
| **R10 Thin Harness / Dumb Loop** | ✅ Touches protocol layer only; harness untouched. Observability ≠ reasoning. |
| **P1 Low coupling** | ✅ Each bundle modifies a single file group; no cross-cutting changes. |
| **P2 High cohesion** | ✅ Bundles align with `openai_chat/` / `openai_responses/` / `openai/types.rs` / `config/types/provider.rs` natural boundaries. |
| **P6 Simplicity (KISS / YAGNI)** | ✅ Defensive parses; no speculative future-proofing. |

---

## 4. Bundle B2 — Canonical TokenUsage Population

### 4.1 OpenAI Chat side

**File:** `src/providers/openai/types.rs`

Extend `OpenAiUsage`:

```rust
#[derive(Debug, Deserialize)]
pub struct OpenAiUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    #[allow(dead_code)]
    pub total_tokens: Option<u32>,
    // NEW
    #[serde(default)]
    pub prompt_tokens_details: Option<OpenAiPromptTokensDetails>,
    #[serde(default)]
    pub completion_tokens_details: Option<OpenAiCompletionTokensDetails>,
}

#[derive(Debug, Deserialize, Default)]
pub struct OpenAiPromptTokensDetails {
    #[serde(default)]
    pub cached_tokens: Option<u32>,
}

#[derive(Debug, Deserialize, Default)]
pub struct OpenAiCompletionTokensDetails {
    #[serde(default)]
    pub reasoning_tokens: Option<u32>,
}
```

No `#[serde(deny_unknown_fields)]` — forward compat preserved.

**File:** `src/providers/protocols/openai_chat/sse.rs:88-108`

Replace the three hardcoded `None` lines:

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
    cache_creation_tokens: None, // OpenAI Chat does not surface cache creation
    thinking_tokens,
})
```

**Why `cache_creation_tokens` stays `None`:** OpenAI Chat does not surface a cache-write metric (only cache-read is exposed via `prompt_tokens_details.cached_tokens`). Anthropic returns both. Leaving the canonical field `None` is correct semantics, not a stub.

### 4.2 OpenAI Responses side

**File:** `src/providers/responses/types.rs`

Extend the existing usage deserialize struct (the one consumed in `openai_responses/mod.rs:444-478` when the `response.completed` event arrives) with details sub-structs. If the struct is currently inline / anonymous in `mod.rs`, lift it to `responses/types.rs` as `ResponsesUsage` first so both this spec and the existing `MeteringProvider` pipeline can share it.

```rust
#[derive(Debug, Deserialize)]
pub struct ResponsesUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    // NEW
    #[serde(default)]
    pub input_tokens_details: Option<ResponsesInputTokensDetails>,
    #[serde(default)]
    pub output_tokens_details: Option<ResponsesOutputTokensDetails>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ResponsesInputTokensDetails {
    #[serde(default)]
    pub cached_tokens: Option<u32>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ResponsesOutputTokensDetails {
    #[serde(default)]
    pub reasoning_tokens: Option<u32>,
}
```

**File:** `src/providers/protocols/openai_responses/mod.rs:472-474`

Replace the three `None` lines with the parsed `Option<u32>` values lifted from `ResponsesUsage.input_tokens_details.cached_tokens` and `ResponsesUsage.output_tokens_details.reasoning_tokens`. Mirror of B2.1 above; `cache_creation_tokens` remains `None` (Responses API does not expose cache-write either).

### 4.3 Test plan

| Test | Location | Asserts |
|---|---|---|
| `openai_chat_usage_deserializes_cache_and_reasoning_tokens` | `openai_chat/tests.rs` | A `usage` JSON with both `prompt_tokens_details.cached_tokens` and `completion_tokens_details.reasoning_tokens` deserializes; canonical `TokenUsage` carries both as `Some(_)`; `cache_creation_tokens == None` |
| `openai_chat_usage_handles_missing_details` | `openai_chat/tests.rs` | A `usage` JSON without `*_details` fields still deserializes; canonical fields all `None` except `input_tokens` / `output_tokens` |
| `openai_responses_usage_deserializes_cache_and_reasoning_tokens` | `openai_responses/tests.rs` | Analogous to first test, but with `input_tokens_details.cached_tokens` and `output_tokens_details.reasoning_tokens` |
| `openai_responses_usage_handles_missing_details` | `openai_responses/tests.rs` | Analogous to second test |

**Fixture seed:** A real OpenAI Chat usage payload that the team captured in the wild (or synthesizes) goes into `tests/fixtures/openai_sse/chat_with_cache.txt` and `responses_with_reasoning.txt` so future protocol regressions can replay byte-exact streams.

---

## 5. Bundle B3 — SSE Event Coverage

### 5.1 B3a — Reasoning summary parts explicit handling

**File:** `src/providers/protocols/openai_responses/mod.rs:480-494`

Current shape (the four-event match has only one arm filled):

```rust
StreamEvent::ReasoningSummaryTextDelta { delta, .. } => {
    out.push_back(Ok(ProviderDelta::ThinkingDelta { delta }));
}
_ => {}
```

Replace with explicit four-arm match:

```rust
StreamEvent::ReasoningSummaryPartAdded { .. } => {
    // Reasoning summary part boundary — Aleph's canonical Delta
    // has no part-level concept; downstream UI streams text only.
    // Logged at debug for future debuggability.
    tracing::debug!(target: "aleph::openai_responses_sse",
        "reasoning_summary_part.added (boundary marker, ignored)");
}
StreamEvent::ReasoningSummaryTextDelta { delta, .. } => {
    out.push_back(Ok(ProviderDelta::ThinkingDelta { delta }));
}
StreamEvent::ReasoningSummaryTextDone { .. } => {
    // Already accumulated via delta events; .done payload is redundant.
    tracing::debug!(target: "aleph::openai_responses_sse",
        "reasoning_summary_text.done (already accumulated, ignored)");
}
StreamEvent::ReasoningSummaryPartDone { .. } => {
    tracing::debug!(target: "aleph::openai_responses_sse",
        "reasoning_summary_part.done (boundary marker, ignored)");
}
```

**Why log + drop (not map to canonical)?** Per R10 YAGNI: zero downstream consumer of part boundaries today. Mapping to a new canonical variant would introduce a phantom abstraction. Explicit drop with debug log preserves discoverability when future work needs the boundaries.

### 5.2 B3b — Chat finish_reason expansion

**File:** `src/providers/protocols/openai_chat/sse.rs:111-119`

Replace the four-arm match:

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

**Why unknown → `EndTurn` not `None`:** `None` may be interpreted as "stream not yet finished" by the loop driver, risking hangs. `EndTurn` is the safest stream-terminating fallback and the warning ensures we notice new reasons in production.

**Why `content_policy_violation` and `content_filter` both → MaxTokens:** Aleph's canonical `StopReason` has no `ContentPolicy` variant; introducing one is YAGNI (no UI/state machine consumer). MaxTokens semantics ("output truncated") is the closest available match. The `tracing::warn!` arm gives us visibility if this becomes painful.

### 5.3 Test plan

| Test | Location | Asserts |
|---|---|---|
| `responses_reasoning_summary_part_added_does_not_emit` | `openai_responses/tests.rs` | Feeding a `response.reasoning_summary_part.added` SSE event yields zero `ProviderDelta` items |
| `responses_reasoning_summary_text_done_does_not_emit` | `openai_responses/tests.rs` | Feeding a `response.reasoning_summary_text.done` SSE event yields zero `ProviderDelta` items |
| `responses_reasoning_summary_part_done_does_not_emit` | `openai_responses/tests.rs` | Feeding a `response.reasoning_summary_part.done` SSE event yields zero `ProviderDelta` items |
| `responses_reasoning_summary_text_delta_still_emits` | `openai_responses/tests.rs` | Regression: existing behavior of `text.delta → ThinkingDelta` preserved |
| `chat_finish_reason_function_call_maps_to_tool_use` | `openai_chat/tests.rs` (rstest) | `function_call` → `ToolUse` |
| `chat_finish_reason_content_policy_violation_maps_to_max_tokens` | `openai_chat/tests.rs` (rstest) | `content_policy_violation` → `MaxTokens` |
| `chat_finish_reason_incomplete_maps_to_max_tokens` | `openai_chat/tests.rs` (rstest) | `incomplete` → `MaxTokens` |
| `chat_finish_reason_unknown_warns_and_returns_endturn` | `openai_chat/tests.rs` | Unknown string → `Some(EndTurn)`; (optional) tracing-subscriber captures warn event |

---

## 6. Bundle B4 — `stop_sequences` Wiring

### 6.1 Request struct additions

**File:** `src/providers/openai/types.rs`

```rust
#[derive(Debug, Serialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    // NEW
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
}
```

**File:** `src/providers/responses/types.rs`

Add a `stop: Option<Vec<String>>` field with matching `skip_serializing_if`. (Field name confirmed via OpenAI Responses API docs; identical to Chat.)

### 6.2 Adapter wiring

**File:** `src/providers/protocols/openai_chat/adapter.rs`

In the request-building path, add:

```rust
stop: cfg.stop_sequences.as_ref()
    .map(|s| s.split(',').map(|x| x.trim().to_string())
         .filter(|x| !x.is_empty()).collect::<Vec<_>>())
    .filter(|v| !v.is_empty()),
```

**File:** `src/providers/protocols/openai_responses/mod.rs`

Same expression in the Responses request builder.

**Why this parsing shape:** `ProviderConfig.stop_sequences: Option<String>` is comma-separated (matches `template.rs:67`). The chain trims whitespace, drops empty fragments, and drops the field entirely if the resulting vector is empty (so a malformed `","` config doesn't send `[""]` to the API).

**Why 4-element limit not enforced here:** OpenAI Chat accepts at most 4 stop sequences and rejects more. We don't truncate or warn at config-parse time; the API will return a clear 400 if violated. Adding a check here is YAGNI until a user actually hits it.

### 6.3 Test plan

| Test | Location | Asserts |
|---|---|---|
| `chat_stop_sequences_serializes_into_request` | `openai_chat/tests.rs` | `ProviderConfig { stop_sequences: Some("END,STOP".into()), .. }` → request JSON contains `"stop": ["END","STOP"]` |
| `chat_stop_sequences_none_omits_field` | `openai_chat/tests.rs` | `stop_sequences: None` → request JSON has no `stop` key |
| `chat_stop_sequences_empty_string_omits_field` | `openai_chat/tests.rs` | `stop_sequences: Some("".into())` → request JSON has no `stop` key |
| `chat_stop_sequences_only_commas_omits_field` | `openai_chat/tests.rs` | `stop_sequences: Some(",".into())` → request JSON has no `stop` key |
| `chat_stop_sequences_trims_whitespace` | `openai_chat/tests.rs` | `stop_sequences: Some(" END , STOP ".into())` → request JSON contains `"stop": ["END","STOP"]` |
| `responses_stop_sequences_serializes_into_request` | `openai_responses/tests.rs` | Same as first test on Responses side |
| `responses_stop_sequences_none_omits_field` | `openai_responses/tests.rs` | Same as second test on Responses side |

---

## 7. Cross-cutting: Testing, Error Handling, Backward Compatibility

### 7.1 Fixture directory

New `tests/fixtures/openai_sse/` directory. Cycle 1 seeds:

- `chat_completion_with_cache.txt` — Chat SSE stream with `prompt_tokens_details.cached_tokens > 0`
- `chat_completion_with_reasoning.txt` — Chat SSE stream from an o1/o3 model with `completion_tokens_details.reasoning_tokens > 0`
- `responses_with_reasoning_summary_parts.txt` — Responses SSE stream containing all four reasoning_summary event types
- `responses_with_cache_and_reasoning.txt` — Responses SSE stream with both `input_tokens_details.cached_tokens` and `output_tokens_details.reasoning_tokens`

Fixtures are checked-in plaintext; tests `include_str!()` them.

### 7.2 Error handling

- **Forward compat:** All new deserialize structs use `#[serde(default)]` on inner Option fields. OpenAI adding new sub-fields under `*_tokens_details` won't break us.
- **Unknown SSE events:** Existing fall-through `_ => {}` arm in `openai_responses/mod.rs` preserved for genuinely-unknown events. We only made the four reasoning_summary variants explicit.
- **Unknown finish_reason:** Now `tracing::warn!` + `EndTurn` fallback (was silent `None`).

### 7.3 Backward compatibility

- `TokenUsage` field shape unchanged → no consumer rewrite.
- New `stop` field has `skip_serializing_if = "Option::is_none"` → callers without `stop_sequences` see byte-identical request wire format.
- `OpenAiUsage` only adds optional fields → existing JSON parses unchanged.
- Existing 8 finish_reason mappings preserved; new mappings are pure additions.

### 7.4 Performance impact

- ~2 extra Option dereferences per `Usage` event.
- ~1 extra serde struct allocation per usage payload.
- 3 added `tracing::debug!` per reasoning_summary-rich Responses stream (debug-level, filtered out at default).

Negligible across all three bundles.

---

## 8. Files Touched

| File | Bundles | Net change |
|---|---|---|
| `src/providers/openai/types.rs` | B2, B4 | +30 lines (two new structs + one new field) |
| `src/providers/responses/types.rs` | B2, B4 | +30 lines (two new structs + one new field) |
| `src/providers/protocols/openai_chat/sse.rs` | B2, B3b | ~+15, ~-3 (token wiring + finish_reason expansion) |
| `src/providers/protocols/openai_responses/mod.rs` | B2, B3a | ~+15, ~-3 (token wiring + reasoning_summary variants) |
| `src/providers/protocols/openai_chat/adapter.rs` | B4 | +8 lines (stop_sequences wiring) |
| `src/providers/protocols/openai_chat/tests.rs` | all | +5-6 tests, ~+90 lines |
| `src/providers/protocols/openai_responses/tests.rs` | all | +5-6 tests, ~+90 lines |
| `tests/fixtures/openai_sse/*.txt` | all | NEW directory, 4 fixtures, ~ 1KB each |
| `CHANGELOG.md` | all | `[Unreleased] ### Fixed` + `### Added` entries per bundle |

Total: ~250 lines net add, ~6 lines delete, 4 new fixture files. Zero file deletions. Zero Cargo.toml changes.

---

## 9. Commit Plan

| # | Title | Bundles | Files | LOC est. |
|---|---|---|---|---|
| 1 | `providers/openai: populate cache_read and reasoning tokens on Chat path` | B2 (Chat) | `openai/types.rs`, `openai_chat/sse.rs`, `openai_chat/tests.rs`, fixture | ~70 |
| 2 | `providers/openai: populate cache_read and reasoning tokens on Responses path` | B2 (Responses) | `responses/types.rs`, `openai_responses/mod.rs`, `openai_responses/tests.rs`, fixture | ~70 |
| 3 | `providers/openai: explicit reasoning_summary_part event handling in Responses` | B3a | `openai_responses/mod.rs`, `openai_responses/tests.rs`, fixture | ~50 |
| 4 | `providers/openai: expand Chat finish_reason mapping and warn on unknown` | B3b | `openai_chat/sse.rs`, `openai_chat/tests.rs` | ~40 |
| 5 | `providers/openai: wire ProviderConfig.stop_sequences into Chat and Responses requests` | B4 | `openai/types.rs`, `responses/types.rs`, `openai_chat/adapter.rs`, `openai_responses/mod.rs`, both `tests.rs` | ~80 |

All five commits are independently compilable and individually revertable. Order is technical-dependency-free; suggested execution order is 1 → 2 → 4 → 3 → 5 (most-observable-value first; B4 last because least user-visible).

---

## 10. Out of Scope (deferred to future cycles)

- **B1 (desktop tool schema fix)** — covered by `2026-05-11-openai-responses-strict-multi-type-fix.md` (approved, awaiting plan/implementation).
- **M3 retry_policy** — 4-module overhaul's last unshipped module (`2026-05-11-openai-protocol-optimization-design.md` §3.3). Separate brainstorm cycle.
- **Feature parity additions:** `response_format` JSON Schema mode, `seed`, `logprobs`, `max_completion_tokens`, `parallel_tool_calls` explicit toggle. Cycle 2 candidate.
- **Delta/adapter normalization:** porting OpenClaw's `NormalizedUsage`-with-aliases-and-dedup pattern, canonical message ↔ OpenAI/Anthropic round-trip audit. Cycle 3 candidate.
- **`previous_response_id` session continuity + incremental input mode + encrypted reasoning replay.** Cycle 4 candidate.
- **Re-validation that B1 strict-multi-type fix is actually implemented in the current `openai_strict_schema.rs:227 normalize_strict_schema` + `responses/shared.rs build_tools`** — if not, that work proceeds via its own spec, not this one.

---

## 11. Acceptance Criteria

- [ ] `OpenAiUsage` and `ResponsesUsage` deserialize structs include `prompt_tokens_details` / `completion_tokens_details` (Chat) and `input_tokens_details` / `output_tokens_details` (Responses) with optional `cached_tokens` / `reasoning_tokens` inner fields.
- [ ] Canonical `TokenUsage.cache_read_tokens` populated from API for both Chat and Responses; `thinking_tokens` populated for both; `cache_creation_tokens` remains `None` (OpenAI does not surface).
- [ ] All four `ReasoningSummary*` variants in `openai_responses/mod.rs` have explicit match arms (no implicit fall-through for the four).
- [ ] Chat finish_reason mapping covers `stop / tool_calls / function_call / length / content_filter / content_policy_violation / incomplete`; unknown values `tracing::warn!` + `Some(EndTurn)`.
- [ ] `ChatCompletionRequest` and Responses Request structs both have `stop: Option<Vec<String>>` with `skip_serializing_if`.
- [ ] Both OpenAI adapters parse `ProviderConfig.stop_sequences` (comma-separated, trimmed, empty-filtered, drop-if-vector-empty) into the request `stop` field.
- [ ] 19 new tests pass (4 B2 deserialize, 8 B3a/b SSE, 7 B4 wiring).
- [ ] `tests/fixtures/openai_sse/` directory created with 4 plaintext fixtures.
- [ ] `cargo check -p alephcore` clean.
- [ ] `cargo clippy -p alephcore --lib --no-deps` no new lints on touched files.
- [ ] `cargo test -p alephcore --test sqlite_migration_legacy_null` (and any other working integration tests) still green.
- [ ] CHANGELOG entries describe each bundle's user-visible effect.
- [ ] Manual e2e: route a webchat through an OpenAI-Chat or OpenAI-Responses provider that returns cached tokens; confirm `MeteringProvider` tracing log carries `cache_read_tokens=Some(N)` with `N > 0` for the second turn.

---

## 12. R7 / R10 Compliance Self-Check

- ✅ All decisions are deterministic: deserialize field names, mechanical match arms, `Option::map` chains.
- ✅ No LLM call added; no scoring; no policy DSL; no decision gate.
- ✅ Harness untouched. Only protocol adapters and their canonical-translation surface.
- ✅ Observability additions (`tracing::warn!` / `tracing::debug!`) are audit logs, not decision points.
- ✅ Unknown-finish-reason fallback is **fail-safe** (terminate stream conservatively), not **fail-clever** (try to infer semantics).

R7 / R10 PASS by construction.

---

## 13. Predecessor + Sibling Context

This spec is the **Cycle 1** of a 4-cycle OpenAI Provider client roadmap announced during the 2026-05-12 brainstorm session. Cycle 1 covers wire-level bug/coverage gaps that are orthogonal to the already-shipped 4-module overhaul. Cycles 2-4 are sketched in §10 (Out of Scope) and will be brainstormed independently after Cycle 1 lands.

Sequence relative to other ongoing OpenAI work:

```
Approved & ready to implement:
  - 2026-05-11-openai-responses-strict-multi-type-fix.md (B1)
  - 2026-05-11-cache-token-observability.md (Anthropic + MeteringProvider)

This spec (2026-05-12, draft):
  - B2 + B3 + B4 (OpenAI client wiring)

Future cycles:
  - retry_policy (M3 of 4-module overhaul)
  - response_format / seed / logprobs / max_completion_tokens (Cycle 2)
  - delta/adapter normalization (Cycle 3)
  - previous_response_id + incremental input (Cycle 4)
```

---

*Next step after approval: spec self-review → user review of written spec → `writing-plans` skill creates implementation plan.*
