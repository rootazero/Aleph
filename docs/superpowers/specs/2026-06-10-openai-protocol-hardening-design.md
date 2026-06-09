# OpenAI Chat/Responses Protocol Hardening — Design

Date: 2026-06-10
References: codex (`/Volumes/TBU4/Github/codex`), openclaw, hermes-agent.

## Verdict from gap analysis

Aleph's OpenAI protocol layer (`src/providers/protocols/openai_chat|openai_responses|openai_common`,
`src/providers/http_provider.rs`) is mature and at/above reference parity on: dual-protocol
capability gating, reasoning-effort clamping, byte-level UTF-8-safe SSE, deferred-Done usage
preservation, truncated-tool-call diagnostics, TTFB + stream-idle watchdogs, encrypted-reasoning
NDJSON replay, `response.failed` / top-level `error` frame handling.

No deep refactor is warranted. Three genuine gaps remain, all fix/wire work:

## GAP A — stale encrypted-reasoning blob permanently poisons a session (port openclaw)

`src/providers/responses/shared.rs:154-157` (author comment): *"A future hardening stage could
strip reasoning items and retry; for now the blob is only ever emitted on the same endpoint that
captured it."* In reality the blob lives in session history and IS replayed across model switches
and provider failover. When the server rejects it (`invalid_encrypted_content`, HTTP 400), the
error is classified Fatal by `llm_retry` (correct — same payload can't succeed), so the turn
fails, and every subsequent turn replays the same blob: the session is bricked.

Reference: openclaw `openai-transport-stream.ts:956-981` — catch the error, retry once without
encrypted reasoning.

Fix (in `HttpProvider::execute`, the only layer that can rewrite the payload below the retry
classifier):
- Extract the request/stream/collect body of `execute` into `execute_once(&self, messages, payload, sink)`.
- After a failed attempt, if the error message contains `encrypted_content` AND any message carries
  a `ContentBlock::Thinking { signature: Some(_) }`, rebuild the message list with signatures
  dropped (immutable rebuild) and retry exactly once.
- Update the stale comment in `shared.rs` to describe the now-existing recovery.

Known cost (accepted, mirrors openclaw): the poisoned blob stays in session history, so an
affected session pays one failed request + one retry per turn until compaction rewrites history.
Scrubbing the session store from the provider layer would violate layer boundaries (R1/P1).

`stream_raw` (gateway passthrough) is intentionally not covered: deltas are already forwarded to
the client mid-stream, and passthrough clients own their history.

## GAP B — mid-stream drop is reported as a complete turn (port codex/hermes semantics)

If the HTTP stream closes before any terminal signal (chat: no `finish_reason`, no `[DONE]`;
responses: no `response.completed`/`failed`/`error`), both unfolds end the stream silently.
`DeltaCollector::finish()` then defaults `stop_reason` to `EndTurn` — truncated output is
presented as a finished turn. codex errors out ("stream closed before response.completed");
hermes distinguishes stream-drop from length-cap.

Fix (scoped to the two OpenAI protocol unfolds; other protocols out of scope for this pass):
- Track whether a terminal signal was seen ( `[DONE]` sentinel, a released `Done`, or an `Error`
  delta).
- In the HTTP-stream-end branch, when no terminal signal exists, push a final
  `Err(AlephError::Timeout { .. })` — the same typed transient error the existing
  `truncated_tool_call` path uses, so failover/retry classify it correctly.
- A drop after `finish_reason` (deferred Done released at stream end) is still a complete turn —
  unchanged behaviour.

## GAP C — usage-limit detection missing on the Responses HTTP error path (wiring/parity)

`is_usage_limit_body` (xAI 403 spending-limit, `insufficient_quota`, …, #86614) exists only in
`openai_chat/adapter.rs` and is checked only on the Chat non-2xx path. The Responses adapter's
non-2xx path (`openai_responses/mod.rs` `stream_deltas`) only special-cases 429, so the same
quota bodies surface as generic retryable provider errors.

Fix: move `is_usage_limit_body` (and its tests) to `openai_common`, re-wire the Chat call site,
add the identical check to the Responses non-2xx path. One definition, two consumers (entropy
reduction).

## Explicitly not done (honest deferrals)

- `previous_response_id` stays `None`: server-side threading requires `store=true`, which
  contradicts Aleph's deliberate stateless replay design. The field documents the wire format.
- Responses-over-WebSocket transport, prewarm, incremental requests (codex): heavy new
  infrastructure with no current consumer — R3/R10/YAGNI.
- Inbound stateful `/v1/responses` response store (hermes): new persistence subsystem — YAGNI.
- Extending `health.rs` string classification: GAP C already yields typed `RateLimitError`,
  which `health.rs:168` classifies; no further wiring needed.

## Verification

Per the operating constraint for this task, no `cargo check`/`cargo test` is run; correctness is
guarded by added unit tests (compile-ready, reviewed), close-reading of call sites, and an
independent code review pass before merge.
