# Final verdicts — GX6

| ID | Verdict | Severity | Reason |
|---|---|---|---|
| GX6-01 | UPHELD | P0 | L215 `touch(id)` returns session to any bearer; no owner check exists / `session_from_header` only calls `sessions().touch(id)` at http.rs:215; no principal ownership is checked. |
| GX6-02 | UPHELD | P0 | L83 derived Debug on struct with `api_key: String` field / `StreamingTarget` derives `Debug` at mod.rs:83 and exposes public `api_key` at mod.rs:88, enabling credential disclosure through formatting. |
| GX6-03 | UPHELD | P1 | L81 end spans to next TurnStarted; warm seed appends without turn boundaries / promote.rs:81 extends a span to the next `TurnStarted` or records end; copied later assistant content can therefore be paired with an earlier side question. |
| GX6-04 | UPHELD | P1 | L90-91 reuses Chat converter that expects nested `function` key / responses/mod.rs:90 reuses Chat tool conversion, whose nested-only parser silently removes flattened Responses tool definitions and choices. |
| GX6-05 | UPHELD | P1 | L253-255 forces role-bearing ResponsesMessage; output lacks call_id / types.rs:255 restricts input to `ResponsesMessage`; function-call output items lack the required `role`, so standard tool-result input cannot deserialize. |
| GX6-06 | UPHELD | P1 | L71-80 emits response at root not wrapped in lifecycle event schema / sse.rs:73 emits a bare response object under `response.created`; the event lacks the required `type`, `sequence_number`, and `response` wrapper. |
| GX6-07 | UPHELD | P1 | L231-234 Done(_reason) discarded; no incomplete_details emitted / sse.rs:231 discards `Done(_reason)` and always emits `completed_frame`, so token-limit truncation is reported as completion. |
| GX6-08 | UPHELD | P1 | L129 filters non-user/assistant/tool; L139 takes only first system / passthrough.rs:129 drops every unhandled role, while lines 139-140 retain only the first system message, losing developer and later instructions. |
| GX6-09 | UPHELD | P1 | L166-177 payload constructed without reading accepted `stop` field / The request accepts `stop`, but passthrough.rs:166-177 constructs the payload without forwarding it, silently changing generation limits. |
| GX6-10 | UPHELD | P1 | L267-269 usage maps internal input without adding cache_read tokens / stream.rs:267-269 maps only `u.input_tokens` to prompt tokens and totals, omitting the separate cache-read bucket. |
| GX6-12 | UPHELD | P1 | L345 ignores emit_event Err; append failure doesn't block run |
| GX6-13 | UPHELD | P1 | L131-138 unconditional usage in finish chunk; hardcoded zeros contradict total |
| GX6-14 | UPHELD | P1 | L210-213 fallback assigns ogg for empty/unknown format strings |
| GX6-15 | UPHELD | P1 | L169-170 skipped sanitization returns None same as provider failure |
| GX6-16 | UPHELD | P1 | L86-89 processes bold/italic before code; markup inside backticks parsed |
| GX6-11 | DOWNGRADE | P2 | L429 spawn without cancel tie; actual risk needs confirmation in exec / agent.rs:429 spawns execution independently of the response stream; no disconnect path shown connects receiver closure to execution cancellation. |
| GX6-17 | UPHELD | P2 | L88-94 notification returns Accepted; no cancel token lookup exists |
