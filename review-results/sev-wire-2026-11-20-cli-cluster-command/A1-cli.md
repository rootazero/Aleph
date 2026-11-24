# A1 CLI Severed-Wire Audit

Scope: `src/cli/mod.rs`, `src/cli/endpoint.rs`, `src/cli/ipc_client.rs`, `src/cli/policy.rs` read in full. Read-only audit; no source changes, commits, or cargo commands.

## Part A — Prior findings re-verification

| Finding | Verdict | Evidence summary |
|---|---|---|
| #17 Non-loopback endpoint receives admin bearer token | FIXED | `src/cli/ipc_client.rs:72-83` rejects non-loopback base hosts before `read_token` at line 147; joined URL is checked again at lines 121-141. Production endpoint published by `src/bin/aleph-server/commands/start/mod.rs:3967-4010`. |
| #23 Full response body read then truncated | FIXED | `src/cli/ipc_client.rs:184-199` streams into capped `CappedBuffer`; `resp.copy_to` errors on overflow, and `finalize` consumes this helper at line 391. |
| #24 Absolute route replaces endpoint base path | FIXED | `src/cli/ipc_client.rs:85-96` refuses non-root base prefixes before joining at lines 113-115; server publishes bare origin via `src/cli/endpoint.rs:28-40` and start path at `src/bin/aleph-server/commands/start/mod.rs:3967-4010`. |
| #25 LockOrIpc discards holder PID after IPC failure | FIXED | `src/cli/policy.rs:197-209` retains `LockHeldError` and adds it as context when the retry still sees a held lock. |
| #26 Missing/empty/unsupported endpoint reported as missing | FIXED | `src/cli/endpoint.rs:156-157,184-199` returns `None` for missing, empty, unsupported version; `src/cli/ipc_client.rs:38-42` accurately reports all three. |
| #29 Static LockOrIpc route blocks dynamic per-key admin routes | OPEN (previously deferred) | `src/cli/policy.rs:95` stores `route: &'static str`; `src/bin/aleph-server/commands/secret.rs:252-269` dispatches delete/verify as LockOnly, while server registers `/{key}` at `src/gateway/admin_api/secrets.rs:18-19`. This is the already-known deferred limitation, not a new finding. |

### #17 — Admin bearer token could reach a non-loopback host named by endpoint file
- Form: 4
- Producer: `src/bin/aleph-server/commands/start/mod.rs:3967-4010` — publishes endpoint URL then writes `.ipc-endpoint.json`; user-edited/tampered endpoint is validated by the forwarding consumer.
- Consumer: `src/cli/ipc_client.rs:72-83,147` — validates loopback host before loading the bearer token; joined authority is revalidated at lines 121-141.
- Live caller search: `rg -n 'forward_to_server\\(' src --glob '*.rs'` found production caller `src/bin/aleph-server/commands/resume.rs:29`; `rg -n 'CommandPolicy::LockOrIpc|route:.*LockOrIpc|route:.*v1/admin' src/bin/aleph-server/commands src/cli --glob '*.rs'` found secret callers at `src/bin/aleph-server/commands/secret.rs:189,211,227`. These are production callers. The outbound token is gated before `read_token`; no non-loopback request path remains.
- Symptom: A non-loopback endpoint is refused before reading or attaching the token; no reported exposure remains in this path.
- Hypothesis: CONNECT — resolved in code; no additional wire change indicated.
- Confidence: high

### #23 — Response body is read fully before truncation
- Form: 2
- Producer: `src/gateway/admin_api/secrets.rs:29-34,45-54` — admin handlers produce JSON responses; other admin results also flow through this response reader.
- Consumer: `src/cli/ipc_client.rs:184-199,391` — reads at most `MAX_RESPONSE_BYTES` (1 MiB) via `CappedBuffer`; overflow terminates copy with an error instead of buffering the rest.
- Live caller search: `rg -n 'forward_to_server\\(' src --glob '*.rs'` found production resume caller at `src/bin/aleph-server/commands/resume.rs:29`; `rg -n 'CommandPolicy::LockOrIpc|route:.*v1/admin' src/bin/aleph-server/commands src/cli --glob '*.rs'` found live secret IPC callers at `src/bin/aleph-server/commands/secret.rs:189,211,227`. The production `finalize` path calls `read_body_bytes`; test-only coverage additionally exercises the capped sink.
- Symptom: Oversized response is rejected after at most 1 MiB buffered, rather than allocating the full response.
- Hypothesis: CONNECT — resolved in code; no additional wire change indicated.
- Confidence: high

### #24 — Url::join with an absolute route replaces endpoint base path
- Form: 5
- Producer: `src/cli/endpoint.rs:28-40` — `build_endpoint_url` publishes a bare scheme/host/port URL; production write occurs at `src/bin/aleph-server/commands/start/mod.rs:3967-4010`.
- Consumer: `src/cli/ipc_client.rs:85-96,113-115` — refuses a non-root path prefix, then joins the absolute route. Live route producers are `src/bin/aleph-server/commands/secret.rs:189,211,227` and `src/bin/aleph-server/commands/resume.rs:29`.
- Live caller search: `rg -n 'forward_to_server\\(' src --glob '*.rs'` found resume at line 29; `rg -n 'CommandPolicy::LockOrIpc|route:.*v1/admin' src/bin/aleph-server/commands src/cli --glob '*.rs'` found three production secret routes. Thus the route/join path is live; prefixed endpoints are rejected before join.
- Symptom: A prefixed endpoint fails with a clear refusal instead of silently dropping its prefix and addressing the wrong route.
- Hypothesis: CONNECT — resolved in code; no additional wire change indicated.
- Confidence: high

### #25 — LockOrIpc drops LockHeldError naming the running server PID
- Form: 2
- Producer: `src/cli/policy.rs:108-133` — `acquire_or_held` constructs the typed holder error; `LockHeldError` formats live PID at lines 34-69.
- Consumer: `src/cli/policy.rs:197-209` — `lock_or_ipc_dispatch` preserves the first held error and chains it with the forwarding error when the retry remains held.
- Live caller search: `rg -n 'CommandPolicy::LockOrIpc|route:.*v1/admin' src/bin/aleph-server/commands src/cli --glob '*.rs'` found production secret callers at lines 189, 211, 227; `rg -n 'forward_to_server\\(' src --glob '*.rs'` found production resume at line 29. The dispatch is live. `src/cli/policy.rs:459-486` also has a unit test asserting the retained holder diagnostic and transport cause.
- Symptom: If IPC fails while the singleton lock remains held, the returned error includes lock-holder information as well as the transport failure.
- Hypothesis: CONNECT — resolved in code; no additional wire change indicated.
- Confidence: high

### #26 — read_endpoint collapses missing/empty/unsupported into None
- Form: 5
- Producer: `src/cli/endpoint.rs:150-157,181-199` — read helper maps missing, empty, and unsupported-version files to `Ok(None)` intentionally.
- Consumer: `src/cli/ipc_client.rs:38-42` — caller describes the combined states as “missing, empty, or unsupported version”; `src/bin/aleph-server/daemon.rs:439-441` separately uses the result best-effort for status.
- Live caller search: `rg -n 'read_endpoint\\(' src --glob '*.rs'` found production consumers `src/cli/ipc_client.rs:38` and `src/bin/aleph-server/daemon.rs:441`; endpoint module references otherwise are tests. `rg -n 'missing, empty, or unsupported|no usable \.ipc-endpoint' src tests` found the production message and test assertion. Caller is live and no longer claims only that the file is absent.
- Symptom: CLI diagnostics accurately describe the unusable endpoint states; status remains best-effort.
- Hypothesis: CONNECT — resolved in code; no additional wire change indicated.
- Confidence: high

### #29 — Static LockOrIpc route blocks per-key admin routes
- Form: 5
- Producer: `src/gateway/admin_api/mod.rs:43` and `src/gateway/admin_api/secrets.rs:18-19` — server mounts secret routes including `GET/DELETE /{key}`.
- Consumer: `src/cli/policy.rs:93-96,156-161,180-198` — `CommandPolicy::LockOrIpc.route` and dispatch helper require a static string. `src/bin/aleph-server/commands/secret.rs:252-269` consequently uses LockOnly for delete/verify, which refuses when server lock is held.
- Live caller search: `rg -n 'LockOrIpc|LockOnly' src/bin/aleph-server/commands/secret.rs tests/spec_c_cli_refuse.rs` found production delete/verify LockOnly call sites at lines 252 and 265 and the test contract in `tests/spec_c_cli_refuse.rs:1-13`. `rg -n '\.route\("/\\{key\\}"|\.nest\("/secrets"' src/gateway/admin_api` found the production server handler/mount. The CLI delete/verify are live commands; the current server-up refusal is intentional and tested, so this is a real feature limitation rather than a ghost client route.
- Symptom: `secret delete` and `secret verify` refuse with a held server lock even though the server exposes per-key secret handlers; operator must stop the server first.
- Hypothesis: DECIDE — remains the previously deferred product/API decision whether to support owned/dynamic route segments in the shared policy and promote these commands to IPC.
- Confidence: high

## Part B — New severed wires

No additional severed-wire candidates found in the four in-scope files. The exported endpoint and policy functions have production call sites; HTTP methods and routes inspected here agree with server registration except for the already-recorded #29 limitation. No config section/field in these files is parsed without a production read, and the stub/cfg-feature sweep found no `todo!`, `unimplemented!`, empty production handlers, or undeclared feature-gated implementation blocks. Tests were treated separately from production callers.

## Seams not covered

- Did not audit implementations beyond their CLI/API boundary except targeted production call sites and registered admin routes needed to verify the six prior findings.
- Did not exhaustively enumerate unrelated gateway handlers, all CLI commands outside the four-file scope, or the whole-repository config/event/classifier graph.
- No cargo/build/test commands were run, per task constraint.
