# Module: shared

- Path: `shared/` (client, config, logging, protocol, ui_logic)
- Files scanned: 51
- Total LOC: 8045

## Summary
| Severity | Count |
|----------|------:|
| critical | 0 |
| high     | 3 |
| medium   | 5 |
| low      | 17 |
| **Total**| **25** |

## High-Confidence Issues

### Perspective 1 — Security & Robustness
```
ISSUE|shared/logging/src/pii_filter.rs:9|high|PiiScrubbingLayer is a public no-op type exported under a name implying it scrubs PII — false sense of security for downstream consumers
ISSUE|shared/logging/src/lib.rs:31|high|PiiScrubbingLayer re-exported at crate root (aleph_logging::PiiScrubbingLayer) alongside real scrubber PiiScrubbingFormat — no-op trivially reachable via documented public API
ISSUE|shared/client/src/config.rs:112|medium|set_permissions error discarded with `let _ = …` even though comment promises owner-only — on FAT32/9P/Windows shares chmod silently no-ops and file stays 0644
ISSUE|shared/client/src/connection.rs:146|medium|JSON-RPC responses with id=null are silently dropped instead of matched against pending by id-null (per JSON-RPC 2.0 §4.3)
ISSUE|shared/ui_logic/src/protocol/rpc.rs:80|medium|handle_response only matches JSON-string ids (as_str()); numeric ids silently ignored — oneshot open until timeout
ISSUE|shared/client/src/connection.rs:281|low|call_with_timeout inserts pending entry before serialization — on serialization failure entry never removed, oneshot sender leaks
ISSUE|shared/logging/src/pii.rs:38|low|generic_secret regex misses `api key=secret` (literal space), and stops at first whitespace leaking `Basic <b64>` after the literal "Basic"
```

### Perspective 2 — Logic & Correctness
```
ISSUE|shared/client/src/gateway_client.rs:91|medium|GatewayClient::call_raw hardcodes device_name="aleph-cli" in connect frame while AlephClient::handshake takes it from CliConfig — duplicated handshake logic drifts
ISSUE|shared/protocol/src/trace_presentation.rs:571|low|summarize_tool_result with limit < 7 produces literally "ERROR: " with no error text — failure indistinguishable from empty success
ISSUE|shared/protocol/src/voice_text.rs:114,179|low|is_token_loop unreachable defensive check + asymmetric re-normalization API
ISSUE|shared/client/src/connection.rs:104|low|read_loop silently swallows Binary/Pong frames (`_ => {}`) without setting connected=false — caller can't distinguish idle from healthy
ISSUE|shared/ui_logic/src/protocol/rpc.rs:69|low|RpcClient::call inserts into pending then awaits send — if connection dies after send without Err, pending entry leaks until client drop
```

### Perspective 3 — Architecture Compliance
```
ISSUE|shared/logging/src/pii_filter.rs:13|high|Conflict with R9 (tools over switches): empty PiiScrubbingLayer is publicly callable, giving single-call "scrubbing layer" switch that does nothing
ISSUE|shared/protocol/src/jsonrpc.rs:302|medium|R3: protocol crate depends on `uuid` crate (with `v4` feature → pulls `rand`) purely to generate wire-format IDs — AtomicU64 would avoid dependency tree
ISSUE|shared/ui_logic/src/api/mod.rs:1|low|R3 / dead code: api module declared and re-exported but contains zero functionality
ISSUE|shared/ui_logic/src/observability/mod.rs:1|low|R3 / dead code: observability module declared and exported but zero functionality
ISSUE|shared/ui_logic/src/protocol/events.rs:1|low|R3 / dead code: protocol::events declared in mod.rs but zero functionality
ISSUE|shared/ui_logic/src/protocol/streaming.rs:1|low|R3 / dead code: protocol::streaming declared in mod.rs but zero functionality
```

### Perspective 4 — Code Quality
```
ISSUE|shared/protocol/src/events.rs:1|low|File length 980 lines — single enum file doubling as central type registry
ISSUE|shared/protocol/src/trace_presentation.rs:1|low|File length 933 lines — combines preset options, labels, API, presentation, helpers, tests
ISSUE|shared/client/src/connection.rs:238|low|call_with_timeout ~70 lines: mixes serialization, lock acquisition, send, timeout, response decoding
ISSUE|shared/ui_logic/src/safety/prompt_injection.rs:171|low|check_prompt_injection ~80 lines single-expression body
ISSUE|shared/client/src/connection.rs:111|low|`let _ = data;` after capturing ping payload is dead
ISSUE|shared/logging/src/retention.rs:89|low|`duration_since(modified).unwrap_or_default()` loses "modified in the future" signal (returns 0d)
ISSUE|shared/client/src/output.rs:38|low|max_key_len uses char count, not terminal column width — CJK keys produce misaligned columns
ISSUE|shared/client/src/connection.rs:268|low|JsonRpcRequest rebuilt by hand with three String::to_string calls — constructor JsonRpcRequest::with_id exists and dedups
ISSUE|shared/ui_logic/src/connection/wasm.rs:114|low|receive() takes &mut self and Option::take()s receiver — calling receive() twice silently returns empty stream
```
