# Anthropic Protocol Hardening — Design Spec

**Date:** 2026-05-19
**Branch:** `feat/anthropic-protocol-hardening`
**Lineage:** continues `2026-05-11-anthropic-protocol-step1/step2`, `2026-05-12-anthropic-protocol-cycle4`.

## Background

Comparison baseline: `hermes-agent` (NousResearch Python agent) `agent/anthropic_adapter.py`,
`agent/transports/anthropic.py`, `agent/prompt_caching.py`. Aleph's Anthropic wire
implementation lives in `src/providers/protocols/anthropic/` (`adapter.rs`, `sse.rs`,
`proto_impl.rs`, `provider_policy.rs`) plus the shared `src/providers/delta.rs`.

Aleph's architecture is already sound — clean `ProtocolAdapter` trait, stream-first SSE,
capability policy gating, signed-thinking round-trip. This cycle fixes **verified
correctness/perf gaps**, not a rewrite. Each item is contained and unit-testable; no
behavioral change to the harness loop (R10 — dumb loop stays dumb).

## Verified Gaps (with hermes cross-check)

| ID | Severity | Location | Problem |
|----|----------|----------|---------|
| F1 | bug | `sse.rs` message_delta | Only `end_turn`/`tool_use`/`max_tokens` mapped. `stop_sequence`, `pause_turn`, `refusal`, `model_context_window_exceeded` all collapse to `StopReason::Unknown`. The `StopReason` enum already has `StopSequence`/`PauseTurn`/`Refusal` variants and `gateway/openai_api/stream.rs:224-226` already translates them — pure missing wiring. Causes wrong `finish_reason` for OpenAI-compatible API clients and spurious "Unknown stop_reason" warnings. |
| F2 | bug | `adapter.rs` build_request | `temperature`/`top_p`/`top_k` are sent unconditionally. Anthropic rejects sampling params when extended thinking is enabled (HTTP 400). A user who configures `temperature` *and* uses a thinking model gets every request 400'd. hermes forces `temperature=1` and strips `top_p`/`top_k` in this case. |
| F3 | perf | `adapter.rs` cache injection | `cache_control` is injected at only 2 breakpoints (system + last user message). Anthropic allows 4; hermes uses `system_and_3` (system + last 3 messages). More stable breakpoints across turns → higher multi-turn cache-hit rate → lower cost + latency. |
| F4 | bug | `delta.rs` `DeltaCollector::push` | `ProviderDelta::Usage` overwrites (`self.usage = Some(u)`). Anthropic streams usage twice: `message_start` carries `input_tokens` + cache tokens, `message_delta` carries final `output_tokens`. The second event overwrites the first → **`input_tokens` and cache counts are lost (reported as 0)**. Breaks Stage-J cost metering and makes F3's caching un-measurable. |

## Out of Scope (deliberate)

- **redacted_thinking round-trip.** Real but rare (fires only when Claude's CoT is
  safety-redacted *and* the turn has tool_use). A full fix needs a new
  `message::ContentBlock` variant, which is exhaustively matched in ~25 files — the
  ripple violates surgical-change discipline (P6 / YAGNI). Current behavior (drop the
  block) degrades gracefully and matches hermes' third-party-endpoint handling.
  Recorded as a known limitation for a future cycle.
- **`pause_turn` loop continuation.** Mapping the stop reason (F1) is protocol-correct.
  Making the harness *resume* a paused turn is a loop-behavior change — R10 territory,
  and `pause_turn` is near-unreachable for Aleph (no server-side tools). Left alone.

## Design

### F1 — stop reason mapping
`sse.rs`, `parse_anthropic_sse_event`, `message_delta` arm. Extend the match:
```
"end_turn"                       => EndTurn
"tool_use"                       => ToolUse
"max_tokens"                     => MaxTokens
"stop_sequence"                  => StopSequence
"pause_turn"                     => PauseTurn
"refusal"                        => Refusal
"model_context_window_exceeded"  => ContextWindowExceeded
_                                => Unknown
```
> **Superseded 2026-06-10:** originally mapped to `MaxTokens` (hermes maps it
> to "length"), but that folded a context overflow into the output-cap stop and
> sent the harness down the resume-nudge loop — which appends messages and
> re-hits the wall. `ContextWindowExceeded` is now a distinct `StopReason`
> variant; the harness routes it to the reactive-compaction rescue
> (`try_reactive_compact_and_retry`) and the OpenAI-compatible gateway still
> surfaces it as finish_reason "length".

No other consumer change — remaining variants already handled downstream.

### F2 — thinking ⊥ sampling params
`adapter.rs` `build_request`. After `thinking` is resolved, if `thinking.is_some()`,
force `temperature`, `top_p`, `top_k` to `None` before constructing `MessagesRequest`.
Defensive and orthogonal to the existing capability-policy stripping. When thinking is
off, behavior is unchanged.

### F3 — cache breakpoints: system + last N messages
Replace `inject_cache_control_into_last_user_message` with
`inject_cache_control_into_recent_messages(payload, cc, max_breakpoints)`:
- Budget = 4 total (Anthropic limit). System block consumes 1 when present; the rest
  (≥3) go to the most-recent messages.
- For each targeted message, tag the last **non-thinking / non-redacted_thinking**
  content block (string content is normalized to an array, as the old fn did). A
  message whose blocks are all thinking-type is skipped and does not consume budget.
- Old single-message function is deleted (no other caller).

### F4 — usage delta merge
`delta.rs` `DeltaCollector::push`, `ProviderDelta::Usage` arm. Replace overwrite with a
field-wise merge: for each field keep the incoming value when it is non-zero / `Some`,
otherwise retain the accumulated value. `input_tokens`/`output_tokens`: last non-zero
wins. `cache_read_tokens`/`cache_creation_tokens`/`thinking_tokens`/`cost`: last `Some`
wins. Safe for gemini/openai (single usage event → merge with default = identity).

## Testing

Unit tests colocated with each module (`#[cfg(test)] mod tests`):
- F1: each new reason string → expected `StopReason` (drive `parse_anthropic_sse_event`).
- F2: thinking on ⇒ request JSON has no `temperature`/`top_p`/`top_k`; thinking off ⇒
  params preserved.
- F3: system + 3 recent messages tagged; 4-breakpoint cap respected; all-thinking
  message skipped; string content normalized.
- F4: push `message_start`-shape then `message_delta`-shape usage → merged result keeps
  `input_tokens` and cache counts *and* final `output_tokens`.

Gate: `cargo fmt`, `cargo clippy -p alephcore -- -D warnings`, `cargo test -p alephcore --lib`.

## Cleanup

- Delete the superseded `inject_cache_control_into_last_user_message`.
- Fix the stale doc comment on `OutputConfig` (`types.rs`) — it claims "Not yet wired to
  RequestPayload" but `adapter.rs` wires `config.effort` into it.
