# I-6 Root Cause: Kimi Structured-Action JSON in Text

**Date:** 2026-05-04
**Source issue:** `docs/reports/2026-05-04-note-layer-e2e-verification.md` §7.4
**Status:** Diagnosis only. No code changes in this report.

## Symptom

When the agent uses `kimi-for-coding`, the model frequently emits tool intentions
inside a JSON envelope inside `text` content rather than as a native Anthropic
`tool_use` block:

```json
{"reasoning": "...", "action": {"type": "tool", "tool_name": "note_manage", "arguments": {...}}}
```

Aleph's harness occasionally extracts these (May 4 first run produced 3
`note_manage(create)` calls); most turns the JSON is emitted but no tool runs.

## 1. Where model output is parsed into tool calls today

| Layer | File | Behavior |
|---|---|---|
| Provider SSE parser | `src/providers/protocols/anthropic.rs:682-830` (`parse_anthropic_sse_event`) | Emits `ProviderDelta::ToolCallStart` **only** when `block_type == "tool_use"` (line 710-723). Text content goes through `TextDelta` (line 737-740). |
| Delta collector | `src/providers/delta.rs:113-160` (`DeltaCollector::finish`) | Only `ToolCallStart`/`ToolCallArgDelta`/`ToolCallEnd` populate `tool_calls`. Text and tool_calls are isolated. |
| Harness loop | `src/harness/agent.rs:186-212` | Receives the assembled `ProviderResponse`. Line 212: `if response.tool_calls.is_empty()` ends the turn. **Text is never re-scanned for tool calls.** |

## 2. Existing recognizers for "structured-action JSON in text" — none

Greps for `parse_structured_action`, `extract_action`, `from_text_action`,
`kimi`, `moonshot`, `parse_json_action`: **0 hits** in
`src/{harness,orchestrator,providers,executor}/`.

The closest existing post-processor is `src/utils/json_extract.rs:48-93`
(`extract_json_robust`), which is used by memory extraction / note parsing /
distillation responses but is **not wired into the harness or protocol layer**.

## 3. What the system prompt teaches the model

- Tools are passed via the native API tool list at `src/harness/agent.rs:178`
  (`RequestPayload::new(&messages).with_tools(tools_ref)`), **not** through a
  prompt template that demonstrates the JSON envelope.
- `src/providers/model_behaviors/anthropic.md` is intentionally minimal —
  comment: "Claude's RLHF alignment already favors proactive execution".
- Conclusion: Aleph does **not** instruct the model to use the JSON envelope.
  Kimi's behavior comes from **its own training**, where it emits Cline /
  Claude-Code-style `{reasoning, action}` JSON inside text content.

## 4. Provider routing for `kimi-for-coding`

`src/providers/presets.rs:82-90`:

```rust
"kimi-for-coding" => ProviderPreset {
    base_url: "https://api.kimi.com/coding/v1",
    protocol: "anthropic",
    color: "#6366f1",
    default_model: "Kimi-K2.6",
}
```

`protocol: "anthropic"` → factory loads `AnthropicProtocol`
(`src/providers/protocols/anthropic.rs`) → SSE parser (above) — which only
recognizes native Anthropic `tool_use` blocks. Kimi's JSON envelope is parsed as
plain text and thrown into `response.text`, never `response.tool_calls`.

## 5. Root cause (single sentence)

**Kimi emits tool intentions as JSON inside text because that's what its
training teaches; Aleph's Anthropic-protocol SSE parser only materializes tool
calls from native `tool_use` blocks; the harness never re-reads the text for
tool intent. Result: ~80%+ of Kimi's tool-call attempts are silently dropped.**

## 6. Recommended adapter location

**Option (b): orchestrator / harness post-processor**, between
`DeltaCollector::finish()` and the `tool_calls.is_empty()` check at
`src/harness/agent.rs:212`.

Rejected alternatives:

- **(a) Protocol layer** — pollutes Anthropic protocol with model-specific
  behavior. The Anthropic SSE spec doesn't have a "JSON-in-text tool" concept.
- **(c) Model-specific adapter keyed off provider name** — duplicates work.
  Future Cline-trained / Claude-Code-trained models (and certain GPT-mini
  fine-tunes) emit the same envelope; one post-processor handles all of them.

Sketch:

```rust
// src/harness/agent.rs (after DeltaCollector::finish)
if response.tool_calls.is_empty() && !response.text.is_empty() {
    if let Some(synthesized) = extract_structured_actions(&response.text) {
        response.tool_calls.extend(synthesized);
    }
}
```

`extract_structured_actions` reuses `utils::json_extract::extract_json_robust`,
matches the `{"action": {"type": "tool", ...}}` shape, and constructs synthetic
`NativeToolCall` entries.

## 7. Integration points for the eventual fix

- New helper in `src/utils/structured_action.rs` (or `src/harness/structured_action.rs`).
- Call site in `src/harness/agent.rs:~210` (just before the `is_empty` check).
- A second call site in `src/orchestrator/harness_bridge.rs:139-204`
  for parity if any orchestrator path bypasses the harness loop.
- Unit tests on the helper covering: clean envelope, envelope inside markdown
  fences, malformed/no-action JSON, multiple actions, mixed text+envelope.
- Integration test driving a stub provider that replies with the envelope text
  and asserts the synthetic tool call is dispatched.

## 8. Why this is independent of I-1 (LLM tool-call evasion)

- I-1: model **describes** what it would do without emitting any tool intent —
  prompt-discipline issue.
- I-6: model emits **structured tool intent**, but in a format Aleph silently
  drops — adapter gap.

A proper fix needs both: prompt-side nudges to invoke tools (I-1) and
parser-side acceptance of the JSON envelope (I-6).
