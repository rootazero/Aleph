# Gemini Protocol Hardening — Design Spec

- **Date**: 2026-05-19
- **Branch**: `gemini-protocol-opt` (worktree `/Volumes/TBU4/Workspace/Aleph-gemini-wt`)
- **Status**: Approved for planning
- **Scope decision**: Full — all 7 gap classes, including the cross-cutting `thoughtSignature` work (G2b).

## 1. Context

Aleph's Gemini wire protocol lives in two directories:

- `src/providers/gemini/` — type definitions (`types.rs`) + JSON-Schema sanitization (`schema.rs`).
- `src/providers/protocols/gemini/` — `ProtocolAdapter` impl (`adapter.rs`), request helpers (`proto_impl.rs`), SSE parsing (`sse.rs`), tests (`tests.rs`).

The infrastructure is mature: stream-first SSE, native function calling with `tool_choice`, thinking budget (Gemini 2.5) / thinking level (Gemini 3+), schema sanitization, synthetic tool-call IDs. A comparison against the reference implementation in `hermes-agent` (Python) surfaced **7 gap classes** — bugs, missing wiring, and dropped features. None require a rewrite; all are additive fixes that reuse existing Aleph types and patterns.

This spec covers fixing all 7. It explicitly does **not** introduce: `stopSequences` (Aleph's `RequestPayload` carries no stop field — a cross-protocol change out of scope), request-side `safetySettings`, explicit `cachedContent` caching, `responseSchema` structured output, grounding / Google Search, or the Cloud Code Assist OAuth path. Those are either global features or things the reference itself does not implement.

## 2. Goals / Non-goals

**Goals**

- Fix correctness bugs: dropped image input, asymmetric tool-call IDs, silent empty turns, missing Gemini-3 thought signatures.
- Parse Gemini's error envelope and mid-stream error frames into structured, actionable errors.
- Wire up already-defined-but-unused infrastructure (`Part::InlineData`, `ProviderConfig::top_k`, `TokenUsage::cache_read_tokens`, `StopReason::{Refusal,Sensitive}`, `ProviderDelta::Error`).
- Remove code that the changes orphan; do not leave dead scaffolding.

**Non-goals**

- No destructive refactor. No change to the `ProtocolAdapter` trait shape, the stream-first architecture, or the `ProviderDelta` event model (beyond one additive field).
- No new request-level features beyond the gap list.
- No touching unrelated pre-existing dead code except where this spec explicitly calls it out (§9).

## 3. Gap inventory

| # | Severity | Defect | Evidence |
|---|----------|--------|----------|
| G1 | HIGH | Image input silently dropped. `convert_messages` filters with `as_text()`, so `ContentBlock::Image` is discarded; `Part::InlineData` is defined but never constructed. Anthropic / OpenAI-chat / Responses all handle `ContentBlock::Image`. | `proto_impl.rs:48-52`, `:78`; cf. `protocols/anthropic/proto_impl.rs:62` |
| G2a | HIGH | Tool-call ID replay asymmetry. The Assistant turn's `functionCall` is always serialized with `id: None`, while the matching `functionResponse` carries `id` — Gemini-3 call/response pairing breaks. | `proto_impl.rs:74` vs `:116` |
| G2b | HIGH | Gemini-3 `thoughtSignature` is never captured or replayed. Gemini 3 requires the signature on a replayed `functionCall` part; without it multi-turn tool calling errors or degrades. Zero occurrences in `src/`. | grep: no matches |
| G3 | MEDIUM | Error responses not parsed. HTTP non-2xx bodies are returned raw; an in-stream `{"error": ...}` frame is ignored. `ProviderDelta::Error` exists but is never emitted by this protocol. | `adapter.rs:144-168`, `sse.rs:30` |
| G4 | MEDIUM | `promptFeedback` block → silent empty turn. When the prompt is blocked there are no `candidates`; the parser ignores `promptFeedback` entirely, so the agent loop sees an empty response with no error. | `sse.rs:30` |
| G5 | LOW | `finishReason` `SAFETY` / `RECITATION` map to `StopReason::Unknown`, even though `StopReason::Refusal` and `StopReason::Sensitive` exist. | `sse.rs:98-115` |
| G6 | LOW | `usageMetadata.cachedContentTokenCount` dropped (`cache_read_tokens` always `None`); `top_k` hard-coded `None` despite `ProviderConfig::top_k` existing. | `sse.rs:142-143`, `adapter.rs:51` |
| G7 | LOW | Structured tool output double-encoded. A `ContentBlock::Json` tool result is `to_string`'d and wrapped as `{"result":"<json-string>"}`, so the model sees a string instead of an object. | `proto_impl.rs:103-115` |

## 4. Design — per gap

### G1 — Multimodal image input

`GeminiProtocol::convert_messages`, `User` arm (`proto_impl.rs:47-57`): replace the text-join with a per-block walk that preserves order:

- `ContentBlock::Text { text, .. }` → `Part::Text { text }`
- `ContentBlock::Image { data, mime_type }` → `Part::InlineData { inline_data: InlineData { mime_type, data } }`

`ContentBlock::Image.data` is already raw base64 (no data-URI prefix) — direct passthrough, matching `protocols/anthropic/proto_impl.rs:62`. Adjacent text blocks are no longer collapsed into one part; emit one `Part` per block. If a `User` message yields no parts, keep the existing empty-text fallback.

Scope: `User` messages (the vision-input path). `Assistant`-role images and tool-result images stay out of scope — the model does not take image input on those roles in this flow, and Gemini's `functionResponse` cannot carry inline images cleanly.

### G2a — Tool-call ID passthrough

`convert_messages`, `Assistant` arm, `ToolCall` block (`proto_impl.rs:65-77`): set `GeminiFunctionCall.id` to `Some(id.clone())` instead of `None`. The `id` is the same value used for the matching `functionResponse` (§G7 leaves that path intact), so call and response become symmetric. Synthetic IDs (`gemini_fc_N`) round-trip on both sides for older models; native IDs round-trip for Gemini 3.

### G2b — Gemini-3 `thoughtSignature` capture & replay

`thoughtSignature` is an opaque base64 string that Gemini 3 attaches to a `Part` **as a sibling of `functionCall`** (not nested inside it). The signature on the `functionCall` part must be echoed back on replay. Signatures on plain thought parts are out of scope — Aleph does not replay raw thinking parts to Gemini, so only the `functionCall` signature is reachable and relevant.

Data path (capture → replay):

1. **Wire type** — `gemini/types.rs`, `Part::FunctionCall` variant: add a sibling field
   `thought_signature: Option<String>` with `#[serde(rename = "thoughtSignature", default, skip_serializing_if = "Option::is_none")]`.
   Serialized request shape: `{"functionCall": {...}, "thoughtSignature": "..."}`. Requests are serialize-only for `Part`; responses are parsed via raw `serde_json::Value` in `sse.rs`, so the untagged-enum deserialization path is unaffected.
2. **Capture (SSE)** — `sse.rs`: when a part contains `functionCall`, also read `part.get("thoughtSignature")` as a string and carry it on the emitted `ToolCallStart`.
3. **`ProviderDelta`** — `delta.rs`: extend `ProviderDelta::ToolCallStart` with `thought_signature: Option<String>`. This is the one additive change to the event model.
4. **`DeltaCollector`** — `delta.rs`: the internal `tool_calls` accumulator becomes a named struct (`CollectedToolCall { id, name, args, thought_signature }`) instead of a 3-tuple, for clarity. `finish()` copies `thought_signature` into `NativeToolCall`.
5. **`NativeToolCall`** — `adapter.rs`: add `thought_signature: Option<String>` (`#[serde(default, skip_serializing_if = "Option::is_none")]`).
6. **`ContentBlock::ToolCall`** — `message.rs`: add `thought_signature: Option<String>` (same serde attrs). `UnifiedMessage::from_provider_response` copies it from `NativeToolCall`.
7. **Replay** — `convert_messages` `Assistant` arm: read `thought_signature` off `ContentBlock::ToolCall` and place it on `Part::FunctionCall`.

Non-Gemini protocols (Anthropic, OpenAI-chat, Responses) supply `thought_signature: None` everywhere they construct `NativeToolCall` / `ContentBlock::ToolCall` / `ProviderDelta::ToolCallStart`. All construction sites get `thought_signature: None`; all destructuring sites that do not need it get `..`. The cross-cutting edit is mechanical and additive — no behavior change for non-Gemini paths.

### G3 — Error response parsing

**HTTP-level** (`adapter.rs::stream_deltas`, non-2xx branch): parse the body before wrapping. Gemini returns either `{"error": {code, message, status}}` or, on the streaming endpoint, an array `[{"error": {...}}]`. Reuse the existing `GeminiError` type (`types.rs:183`). Helper: try object form, then array-first-element form; on parse failure fall back to the raw text. Keep the 429 → `AlephError::RateLimitError` branch. For HTTP 500/503 produce an error variant the retry layer treats as retryable (verify against `llm_retry.rs` during implementation); 400/401/403/404 → `AlephError::provider` with the clean `message`/`status`.

**Mid-stream** (`sse.rs`): if a parsed data chunk has a top-level `error` object, push `Err(AlephError::provider("Gemini stream error: <message> (<status>)"))`. This matches the existing SSE-parse-error handling in the same function (`sse.rs:21`) and signals a broken stream per the `delta.rs` error semantics — a mid-stream Gemini error is fatal, not a continuable `ProviderDelta::Error`.

### G4 — `promptFeedback` block detection

`sse.rs`: after the `candidates` block, inspect `json.get("promptFeedback")`. If `blockReason` is present and non-empty, push `Err(AlephError::provider("Gemini blocked the prompt (blockReason=<reason>)"))`. A blocked prompt is non-retryable (the same prompt re-blocks); surfacing it as a `provider` error gives the user a clear message without a retry storm. `promptFeedback` without `blockReason` (ratings only) is ignored — it also appears on successful responses.

### G5 — `finishReason` mapping

`sse.rs` stop-reason match — extend:

- `SAFETY`, `BLOCKLIST`, `PROHIBITED_CONTENT`, `SPII` → `StopReason::Refusal`
- `RECITATION` → `StopReason::Sensitive`
- `MALFORMED_FUNCTION_CALL`, `OTHER` → `StopReason::Unknown`
- existing: `STOP`→`EndTurn`, `MAX_TOKENS`→`MaxTokens`, `FUNCTION_CALL`→`ToolUse`

Keep the existing fallback: an unrecognized non-empty reason with tool calls in the same chunk → `ToolUse`.

### G6 — Usage cache tokens + `top_k`

- `sse.rs` usage block: `cache_read_tokens = usageMetadata.cachedContentTokenCount` (u64 → u32, like the sibling fields).
- `adapter.rs::build_request`: `top_k: config.top_k` instead of `None`. `ProviderConfig::top_k` already exists and is validated in `config/validate.rs:113`.

### G7 — Structured tool-result passthrough

`convert_messages`, `ToolResult` arm (`proto_impl.rs:96-119`): `functionResponse.response` must be a JSON object. Rule:

- Content is exactly one `ContentBlock::Json { value }` and `value` is an object → use `value` directly as `response`.
- Otherwise (text, mixed, or a non-object `Json`) → keep the current `{"result": <joined-text-or-value>}` wrapping.

This preserves structured tool output when available without changing the common text path.

## 5. Cross-cutting type changes (summary)

Three shared types gain one optional, serde-defaulted field each — all additive, none destructive:

| Type | File | Added field |
|------|------|-------------|
| `ProviderDelta::ToolCallStart` | `providers/delta.rs` | `thought_signature: Option<String>` |
| `NativeToolCall` | `providers/adapter.rs` | `thought_signature: Option<String>` |
| `ContentBlock::ToolCall` | `providers/message.rs` | `thought_signature: Option<String>` |

`DeltaCollector`'s private `tool_calls` field changes from `Vec<(String,String,String)>` to `Vec<CollectedToolCall>` (a local named struct). Implementation must grep every construction and destructuring site of these three types across `src/` (notably `protocols/anthropic/`, `protocols/openai_chat/`, `protocols/openai_responses/`, `responses/`, the run loop, subagent code, tests) and add `thought_signature: None` to constructions / `..` to destructurings. Verified via `cargo check`/test for all protocols.

## 6. Error semantics

| Condition | Surfaced as | Retryable |
|-----------|-------------|-----------|
| HTTP 429 | `AlephError::RateLimitError` (existing) | yes (existing path) |
| HTTP 500/503 | retryable error variant (per `llm_retry.rs`) | yes |
| HTTP 400/401/403/404 | `AlephError::provider` with parsed `message`/`status` | no |
| Mid-stream `{"error":...}` frame | `Err(AlephError::provider(...))` — breaks stream | no |
| `promptFeedback.blockReason` | `Err(AlephError::provider(...))` | no |
| `finishReason: SAFETY/RECITATION/...` | `StopReason::Refusal`/`Sensitive` on a normal response | n/a |

## 7. Testing

Unit tests extend `src/providers/protocols/gemini/tests.rs` and `src/providers/gemini/types.rs`:

- **G1**: `convert_messages` with an image block → `Part::InlineData`; mixed text+image preserves order.
- **G2a**: Assistant tool call → serialized `functionCall.id` is populated.
- **G2b**: SSE `functionCall` + `thoughtSignature` → `ToolCallStart.thought_signature` set; collector → `NativeToolCall.thought_signature`; `convert_messages` replays it; serialized request emits a sibling `thoughtSignature` key; absent signature → field omitted.
- **G3**: HTTP error envelope parsed in object and array form; mid-stream `error` frame → `Err`.
- **G4**: `promptFeedback.blockReason` chunk → `Err` with the reason.
- **G5**: `SAFETY`→`Refusal`, `RECITATION`→`Sensitive`.
- **G6**: `cachedContentTokenCount` → `cache_read_tokens`; `top_k` from config appears in the request body.
- **G7**: single `Json`-object tool result → bare `response` object; text result → unchanged `{"result":...}`.

Regression gate: `cargo test -p alephcore --lib` green for all four protocols after the cross-cutting change; `cargo check` clean.

## 8. Build sequence

Isolated fixes first, cross-cutting last, each step compiling and tested before the next:

1. G6 — usage cache tokens + `top_k` (`sse.rs`, `adapter.rs`).
2. G5 — `finishReason` mapping (`sse.rs`).
3. G1 — image input (`proto_impl.rs`).
4. G2a — tool-call ID passthrough (`proto_impl.rs`, one line).
5. G7 — structured tool-result passthrough (`proto_impl.rs`).
6. G3 — error parsing (`adapter.rs` + `sse.rs`).
7. G4 — `promptFeedback` detection (`sse.rs`).
8. G2b — `thoughtSignature` end-to-end, including all cross-protocol match-site fixes.
9. Cleanup (§9) + full `cargo test -p alephcore --lib` + `cargo clippy`.

## 9. Cleanup

- Wiring up `Part::InlineData` (G1) and `ProviderDelta::Error`-adjacent paths removes "defined but unused" scaffolding rather than adding it.
- During implementation, verify whether `GenerateContentResponse`, `Candidate`, `CandidateContent`, and `ResponsePart` in `gemini/types.rs` have any non-test consumers. The streaming path parses responses via raw `serde_json::Value`, so these may be dead. If grep confirms zero production consumers, remove them and their unit tests as part of this cycle's cleanup (the goal explicitly calls for removing stale code). If any are used, leave them untouched. `GeminiError` stays — G3 wires it into the error path.
- Remove any imports/helpers that the above changes orphan. Do not delete unrelated pre-existing dead code.

## 10. Risks

- **Cross-cutting match-site churn (G2b)**: wide but mechanical. Mitigated by doing it last as one focused step, with `cargo check` driving completeness.
- **G7 behavior shift**: tool results that previously arrived as `{"result":"<string>"}` now arrive as structured objects for `Json` tool outputs. This is strictly more faithful; tool results are not cross-session cached, so blast radius is one turn. Acceptable.
- **G3 retry classification**: mapping 5xx to a retryable variant depends on `llm_retry.rs` semantics — confirm during implementation; if uncertain, default to `AlephError::provider` (non-retryable) to avoid retry storms.
