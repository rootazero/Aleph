# Adjudication — GX6 — 88apiclaude/claude-sonnet-5-5

| ID | Verdict | Reason | Corrected severity |
|---|---|---|---|
| GX6-01 | UPHELD | L215 `touch(id)` returns session to any bearer; no owner check exists | P0 |
| GX6-02 | UPHELD | L83 derived Debug on struct with `api_key: String` field | P0 |
| GX6-03 | UPHELD | L81 end spans to next TurnStarted; warm seed appends without turn boundaries | P1 |
| GX6-04 | UPHELD | L90-91 reuses Chat converter that expects nested `function` key | P1 |
| GX6-05 | UPHELD | L253-255 forces role-bearing ResponsesMessage; output lacks call_id | P1 |
| GX6-06 | UPHELD | L71-80 emits response at root not wrapped in lifecycle event schema | P1 |
| GX6-07 | UPHELD | L231-234 Done(_reason) discarded; no incomplete_details emitted | P1 |
| GX6-08 | UPHELD | L129 filters non-user/assistant/tool; L139 takes only first system | P1 |
| GX6-09 | UPHELD | L166-177 payload constructed without reading accepted `stop` field | P1 |
| GX6-10 | UPHELD | L267-269 usage maps internal input without adding cache_read tokens | P1 |
| GX6-11 | DOWNGRADE | L429 spawn without cancel tie; actual risk needs confirmation in exec | P2 |
| GX6-12 | UPHELD | L345 ignores emit_event Err; append failure doesn't block run | P1 |
| GX6-13 | UPHELD | L131-138 unconditional usage in finish chunk; hardcoded zeros contradict total | P1 |
| GX6-14 | UPHELD | L210-213 fallback assigns ogg for empty/unknown format strings | P1 |
| GX6-15 | UPHELD | L169-170 skipped sanitization returns None same as provider failure | P1 |
| GX6-16 | UPHELD | L86-89 processes bold/italic before code; markup inside backticks parsed | P1 |
| GX6-17 | UPHELD | L88-94 notification returns Accepted; no cancel token lookup exists | P2 |
