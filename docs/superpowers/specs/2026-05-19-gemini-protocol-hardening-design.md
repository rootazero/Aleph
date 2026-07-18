# Gemini Protocol Hardening — Design Spec

- **Date**: 2026-05-19
- **Branch**: `gemini-protocol-opt` (worktree `/Volumes/TBU4/Workspace/Aleph-gemini-wt`)
- **Status**: Approved for planning
- **Scope decision**: This cycle ships **G1, G2a, G3, G4, G5, G6, G7** — every change confined to
  `src/providers/gemini/**` and `src/providers/protocols/gemini/**`, with zero overlap with the
  four active sibling worktrees. **G2b (`thoughtSignature`) is deferred** to a dedicated follow-up
  cycle (rationale and preserved design in §11) because a faithful implementation requires
  ~55-65 cross-cutting edits to shared types that collide with in-flight sibling branches.

## 1. Context

Aleph's Gemini wire protocol lives in two directories:

- `src/providers/gemini/` — type definitions (`types.rs`) + JSON-Schema sanitization (`schema.rs`).
- `src/providers/protocols/gemini/` — `ProtocolAdapter` impl (`adapter.rs`), request helpers (`proto_impl.rs`), SSE parsing (`sse.rs`), tests (`tests.rs`).

The infrastructure is mature: stream-first SSE, native function calling with `tool_choice`, thinking budget (Gemini 2.5) / thinking level (Gemini 3+), schema sanitization, synthetic tool-call IDs. A comparison against the reference implementation in `hermes-agent` (Python) surfaced **7 gap classes** — bugs, missing wiring, and dropped features. None require a rewrite; all are additive fixes that reuse existing Aleph types and patterns.

This cycle fixes 6 of the 7 (G1, G2a, G3, G4, G5, G6, G7). It explicitly does **not** introduce: `stopSequences` (Aleph's `RequestPayload` carries no stop field — a cross-protocol change out of scope), request-side `safetySettings`, explicit `cachedContent` caching, `responseSchema` structured output, grounding / Google Search, or the Cloud Code Assist OAuth path. Those are either global features or things the reference itself does not implement.

## 2. Goals / Non-goals

**Goals**

- Fix correctness bugs: dropped image input, asymmetric tool-call IDs, silent empty turns.
- Parse Gemini's error envelope and mid-stream error frames into clear, actionable errors.
- Wire up already-defined-but-unused infrastructure (`Part::InlineData`, `ProviderConfig::top_k`, `TokenUsage::cache_read_tokens`, `StopReason::{Refusal,Sensitive}`, `GeminiError`).
- Keep every edit inside the two Gemini directories — no shared-type changes, no sibling-branch conflict surface.

**Non-goals**

- No destructive refactor. No change to the `ProtocolAdapter` trait, the stream-first architecture, the `ProviderDelta` event model, or any shared type (`ContentBlock`, `NativeToolCall`, `ProviderResponse`).
- No new request-level features beyond the gap list.
- No touching unrelated pre-existing dead code (§9 records observations only).
- `thoughtSignature` (G2b) is out of scope this cycle — see §11.

## 3. Gap inventory

| # | Severity | Defect | Evidence | This cycle |
|---|----------|--------|----------|-----------|
| G1 | HIGH | Image input silently dropped. `convert_messages` filters with `as_text()`, so `ContentBlock::Image` is discarded; `Part::InlineData` is defined but never constructed. Anthropic / OpenAI-chat / Responses all handle `ContentBlock::Image`. | `proto_impl.rs:48-52,78`; cf. `protocols/anthropic/proto_impl.rs:62` | ✅ fix |
| G2a | HIGH | Tool-call ID replay asymmetry. The Assistant turn's `functionCall` is always serialized with `id: None`, while the matching `functionResponse` carries `id` — Gemini-3 call/response pairing breaks. | `proto_impl.rs:74` vs `:116` | ✅ fix |
| G2b | HIGH | Gemini-3 `thoughtSignature` never captured or replayed. | grep: no matches | ⏸ deferred (§11) |
| G3 | MEDIUM | Error responses not parsed. HTTP non-2xx bodies are returned raw; an in-stream `{"error": ...}` frame is ignored. | `adapter.rs:144-168`, `sse.rs:30` | ✅ fix |
| G4 | MEDIUM | `promptFeedback` block → silent empty turn. When the prompt is blocked there are no `candidates`; the parser ignores `promptFeedback`, so the agent loop sees an empty response with no error. | `sse.rs:30` | ✅ fix |
| G5 | LOW | `finishReason` `SAFETY` / `RECITATION` map to `StopReason::Unknown`, even though `StopReason::Refusal` and `StopReason::Sensitive` exist. | `sse.rs:98-115` | ✅ fix |
| G6 | LOW | `usageMetadata.cachedContentTokenCount` dropped (`cache_read_tokens` always `None`); `top_k` hard-coded `None` despite `ProviderConfig::top_k` existing. | `sse.rs:142-143`, `adapter.rs:51` | ✅ fix |
| G7 | LOW | Structured tool output double-encoded. A `ContentBlock::Json` tool result is `to_string`'d and wrapped as `{"result":"<json-string>"}`, so the model sees a string instead of an object. | `proto_impl.rs:103-115` | ✅ fix |

## 4. Design — per gap

### G1 — Multimodal image input

`GeminiProtocol::convert_messages`, `User` arm (`proto_impl.rs:47-57`): replace the text-join with a per-block walk that preserves order:

- `ContentBlock::Text { text, .. }` → `Part::Text { text }`
- `ContentBlock::Image { data, mime_type }` → `Part::InlineData { inline_data: InlineData { mime_type, data } }`

`ContentBlock::Image.data` is already raw base64 (no data-URI prefix) — direct passthrough, matching `protocols/anthropic/proto_impl.rs:62`. Adjacent text blocks are no longer collapsed into one part; emit one `Part` per block. If a `User` message yields no parts, keep an empty-text fallback so the request stays valid. `InlineData` is already re-exported from `providers::gemini` (`pub use types::*`).

The `Part::InlineData` variant in `gemini/types.rs` is also latently broken: its `inline_data` field has no `#[serde(rename)]`, so it would serialize as `inline_data` instead of Gemini's required `inlineData` (the `FunctionCall`/`FunctionResponse` variants have explicit renames; `InlineData` was missed). G1 fixes this by adding `#[serde(rename = "inlineData")]` to the field — a one-line change inside the Gemini types directory.

Scope: `User` messages (the vision-input path). `Assistant`-role images and tool-result images stay out of scope — the model does not take image input on those roles, and Gemini's `functionResponse` cannot carry inline images cleanly.

### G2a — Tool-call ID passthrough

`convert_messages`, `Assistant` arm, `ToolCall` block (`proto_impl.rs:65-77`): destructure `id` and set `GeminiFunctionCall.id` to `Some(id.clone())` instead of `None`. The `id` is the same value used for the matching `functionResponse`, so call and response become symmetric. Synthetic IDs (`gemini_fc_N`) round-trip on both sides for older models; native IDs round-trip for Gemini 3. This is independent of G2b and stands on its own.

### G3 — Error response parsing

A shared helper `parse_gemini_error_body(body: &str) -> Option<GeminiError>` is added to `sse.rs` (`pub(crate)`, testable like `parse_gemini_sse_chunk`). It reuses the existing `GeminiError` type (`types.rs:183`) and handles both the object form `{"error": {...}}` and the streaming array form `[{"error": {...}}]`; on parse failure it returns `None`.

**HTTP-level** (`adapter.rs::stream_deltas`, non-2xx branch): call the helper on the body. Build the error message from `GeminiError.message` + `status` when parsed, else the raw text. Keep the existing 429 → `AlephError::RateLimitError` branch. All other non-2xx → `AlephError::provider` with the clean message. (No 5xx retry reclassification — the core G3 win is a clear message; reclassifying retry behavior is out of scope and risks retry storms.)

**Mid-stream** (`sse.rs::parse_gemini_sse_chunk`): if a parsed data chunk has a top-level `error` object, push `Err(AlephError::provider("Gemini stream error: <message> (<status>)"))` and return. This matches the existing SSE-parse-error handling in the same function (`sse.rs:21`) and signals a broken stream per the `delta.rs` error semantics — a mid-stream Gemini error is fatal, not a continuable `ProviderDelta::Error`.

### G4 — `promptFeedback` block detection

`sse.rs::parse_gemini_sse_chunk`: after the mid-stream `error` check and before candidate extraction, inspect `json.promptFeedback.blockReason`. If present and non-empty, push `Err(AlephError::provider("Gemini blocked the prompt (blockReason=<reason>)"))` and return. A blocked prompt is non-retryable (the same prompt re-blocks); surfacing it as a `provider` error gives the user a clear message without a retry storm. `promptFeedback` without `blockReason` (ratings only) is ignored — it also appears on successful responses.

### G5 — `finishReason` mapping

`sse.rs` stop-reason match — extend:

- `SAFETY`, `BLOCKLIST`, `PROHIBITED_CONTENT`, `SPII` → `StopReason::Refusal`
- `RECITATION` → `StopReason::Sensitive`
- existing: `STOP`→`EndTurn`, `MAX_TOKENS`→`MaxTokens`, `FUNCTION_CALL`→`ToolUse`
- `MALFORMED_FUNCTION_CALL`, `OTHER`, and any other non-empty reason fall through the existing fallback arm (→ `ToolUse` if tool calls were emitted this chunk, else `Unknown`).

### G6 — Usage cache tokens + `top_k`

- `sse.rs` usage block: `cache_read_tokens = usageMetadata.cachedContentTokenCount` (u64 → u32, like the sibling fields).
- `adapter.rs::build_request`: `top_k: config.top_k` instead of `None`. `ProviderConfig::top_k` already exists and is validated in `config/validate.rs:113`.

### G7 — Structured tool-result passthrough

`convert_messages`, `ToolResult` arm (`proto_impl.rs:96-119`): `functionResponse.response` must be a JSON object. Rule:

- Content is exactly one `ContentBlock::Json { value }` and `value` is an object → use `value` directly as `response`.
- Otherwise (text, mixed, or a non-object `Json`) → keep the current `{"result": <joined-text-or-value>}` wrapping.

This preserves structured tool output when available without changing the common text path.

## 5. Blast radius

**Zero shared-type changes.** Every edit lands in `src/providers/gemini/**` (`types.rs` — one-line `serde(rename)` fix for `Part::InlineData`; `GeminiError` reused as-is) and `src/providers/protocols/gemini/**` (`adapter.rs`, `proto_impl.rs`, `sse.rs`, `tests.rs`). No file touched by the `feat/anthropic-protocol-hardening`, `subagent-hardening`, `l1-stalltracker-lock`, or `openai-protocol-opt` branches is modified. Merge-conflict surface against those branches: none.

## 6. Error semantics

| Condition | Surfaced as | Retryable |
|-----------|-------------|-----------|
| HTTP 429 | `AlephError::RateLimitError` (existing path, message now from the parsed envelope) | yes (existing path) |
| HTTP 4xx/5xx (non-429) | `AlephError::provider` with parsed `message` + `status` | per existing classification (unchanged) |
| Mid-stream `{"error":...}` frame | `Err(AlephError::provider(...))` — breaks the stream | no |
| `promptFeedback.blockReason` | `Err(AlephError::provider(...))` | no |
| `finishReason: SAFETY/RECITATION/...` | `StopReason::Refusal`/`Sensitive` on an otherwise-normal response | n/a |

## 7. Testing

Unit tests extend `src/providers/protocols/gemini/tests.rs`:

- **G1**: `convert_messages` with an image block → `Part::InlineData`; mixed text+image preserves order.
- **G2a**: Assistant tool call → serialized `functionCall.id` is populated.
- **G3**: `parse_gemini_error_body` parses object form and array form; mid-stream `error` frame → `Err`.
- **G4**: `promptFeedback.blockReason` chunk → `Err` with the reason.
- **G5**: `SAFETY`→`Refusal`, `RECITATION`→`Sensitive`.
- **G6**: `cachedContentTokenCount` → `cache_read_tokens`; `top_k` from config appears in the serialized request body.
- **G7**: single `Json`-object tool result → bare `response` object; text result → unchanged `{"result":...}`.

Regression gate: `cargo test -p alephcore --lib` green; `cargo clippy` clean on touched files. Existing Gemini tests (`test_convert_s1`–`s8`, SSE tests) must stay green — G1's per-block walk keeps single-text-block messages at one part, so no existing assertion changes.

## 8. Build sequence

Each gap is one task, each task TDD (failing test → implement → green → commit). Isolated, low-risk fixes first:

1. G6 — usage cache tokens + `top_k` (`sse.rs`, `adapter.rs`).
2. G5 — `finishReason` mapping (`sse.rs`).
3. G1 — image input (`proto_impl.rs`).
4. G2a — tool-call ID passthrough (`proto_impl.rs`).
5. G7 — structured tool-result passthrough (`proto_impl.rs`).
6. G3 — error parsing (`sse.rs` helper + `adapter.rs`).
7. G4 — `promptFeedback` detection (`sse.rs`).
8. Final verification: full `cargo test -p alephcore --lib` + `cargo clippy`.

## 9. Cleanup

- Wiring up `Part::InlineData` (G1), `ProviderConfig::top_k` (G6), `TokenUsage::cache_read_tokens` (G6), `StopReason::{Refusal,Sensitive}` (G5), and `GeminiError` (G3) removes "defined but unused" scaffolding rather than adding it.
- No change in this cycle orphans any import, type, or function — all edits are additive within the Gemini modules. Nothing to delete.
- **Observation only (no action this cycle):** `tests.rs` contains a vestigial `extract_provider_response` helper (a test-only re-implementation of a `parse_response` path that no longer exists) and its dependent `test_extract_response_*` / `test_parse_response_*` tests. They exercise the fake helper, not production code. The `GenerateContentResponse` / `Candidate` / `CandidateContent` / `ResponsePart` types in `types.rs` exist only to support those tests (the streaming path parses via raw `serde_json::Value`). This is genuine dead weight, but removing it is orthogonal to the 7 gaps and is left as a recommendation for a separate cleanup pass.

## 10. Risks

- **G7 behavior shift**: tool results that previously arrived as `{"result":"<string>"}` now arrive as structured objects for `Json` tool outputs. This is strictly more faithful; tool results are not cross-session cached, so blast radius is one turn. Acceptable.
- **G1 part-count change**: a multi-text-block `User` message now yields multiple `Part::Text` entries instead of one joined part. Semantically equivalent for Gemini; no current call site builds such messages (`UnifiedMessage::user()` always produces one block), so no observable change.

## 11. Deferred Work — G2b: Gemini-3 `thoughtSignature`

**Problem.** Gemini 3 attaches an opaque base64 `thoughtSignature` to a `Part` as a sibling of `functionCall`. On replay the signature must be echoed back on the corresponding `functionCall` part, or multi-turn function calling errors / degrades. Aleph captures and replays nothing — zero occurrences of `thoughtSignature` in `src/`.

**Why deferred.** A faithful implementation must thread the signature through shared types: `ProviderDelta` (capture → collector), `NativeToolCall` / `ProviderResponse` (collector → response), and `ContentBlock::ToolCall` (response → `convert_messages` replay). A grep of construction and destructuring sites found ~55-65 mandatory edits across `harness/`, `gateway/`, `agents/`, `memory/`, `context/`, and every protocol's test suite. Many land in files actively modified by the `feat/anthropic-protocol-hardening`, `subagent-hardening`, `l1-stalltracker-lock`, and `openai-protocol-opt` worktrees (`agents/subagent_spawner/tests.rs` is currently in a `UU` conflict state). Doing G2b now guarantees large merge conflicts.

**Recommended approach for the follow-up cycle** (run after the sibling branches land on `main`, on a clean tree):

- New additive `ProviderDelta::ToolCallSignature { id, signature }` variant (a new variant breaks only exhaustive `match ProviderDelta` arms — far cheaper than adding a field to `ToolCallStart`).
- New `Option<String>` field on `ContentBlock::ToolCall`, serde-defaulted (`#[serde(default, skip_serializing_if = "Option::is_none")]`).
- A signature-carrying field on `ProviderResponse` (derives `Default`, so most construction sites are unaffected) populated by `DeltaCollector`; `UnifiedMessage::from_provider_response` copies it onto the `ContentBlock::ToolCall`.
- Gemini side: `sse.rs` reads `part.thoughtSignature` and emits `ToolCallSignature`; `gemini/types.rs` adds a `thoughtSignature` sibling field on the `Part::FunctionCall` variant; `convert_messages` replays it.
- Scope is the `functionCall`-part signature only — Aleph does not replay raw thought parts to Gemini, so other per-part signatures are unreachable and irrelevant.

G2a (this cycle) already fixes Gemini-3 call/response ID pairing; G2b adds the remaining signature replay.
