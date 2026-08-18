# Code review — `src/a2a/` (2026-08-19 round r3)

## Scope
- Files reviewed (production `src/a2a/`, ~5,700 LoC + tests):
  - `port/`: `task_manager.rs`, `authenticator.rs`, `agent_resolver.rs`, `message_handler.rs`, `streaming.rs`, `mod.rs`
  - `domain/`: `task.rs`, `message.rs`, `events.rs`, `agent_card.rs`, `security.rs`, `error.rs`, `mod.rs`
  - `adapter/server/`: `task_store.rs`, `request_processor.rs`, `stream_hub.rs`, `routes.rs`, `bridge.rs`, `mod.rs`
  - `adapter/client/`: `http_client.rs`, `pool.rs`, `sse_stream.rs`, `mod.rs`
  - `adapter/auth/`: `tiered.rs`, `token_store.rs`, `mod.rs`
  - `service/`: `card_builder.rs`, `card_registry.rs`, `card_refresh.rs`, `smart_router.rs`, `llm_matcher.rs`, `notification.rs`, `mod.rs`
  - `sub_agent.rs`, `config.rs`, `mod.rs`
  - Cross-checked callers in `bin/aleph-server/commands/start/mod.rs` (A2A wiring) and `executor/builtin_registry/...` (`a2a_delegate` tool)
- Method: read-first sweep across all files, focused on correctness, error handling, concurrency, resource management, and security. Cross-referenced `docs/engineering-reports/review-results/sev-wire-2026-08-19-r2/a2a/REPORT.md` to skip already-fixed issues (none to skip — that round found 0 findings and explicitly deferred body-level work to "the correctness audit lane").

## Findings

### A2A-R3-01 — Synchronous `message/send` returns a hard-coded "Task completed successfully" string, never the agent's actual output
- **File**: `src/a2a/adapter/server/bridge.rs:114-143`
- **Severity**: High
- **Category**: correctness
- **Description**: The synchronous `handle_message` path builds a fixed `A2AMessage::text("Task completed successfully")` and stamps it on the `Completed` task. It never reads the agent's actual assistant output from `RunRequest` / `ExecutionAdapter` / the agent's workspace. Every A2A peer calling `message/send` on this Aleph instance receives the same canned string, regardless of what the agent did. The streaming path has the same issue (lines 254-261 / 285-292).
- **Evidence**:
  ```rust
  // bridge.rs:117-119
  Ok(()) => {
      let response_msg = A2AMessage::text(A2ARole::Agent, "Task completed successfully");
      let task = self.task_manager
          .update_status(task_id, TaskState::Completed, Some(response_msg))
          .await?;
  ```
- **Suggested fix**: Have `ExecutionAdapter` return the run's final assistant text (or accept a `ResultSink` that the adapter writes to) and use it as the `Completed` status message. The streaming path can do the same — emit the final text as a `TaskArtifactUpdateEvent` followed by `TaskStatusUpdateEvent{is_final:true}`.
- **Verification**: Read the outbound analog in `sub_agent.rs:dispatch_sync` (lines 145-200) which correctly extracts the last `A2ARole::Agent` message from `task.history` — confirming the inbound bridge should do the same. The execution-adapter contract is the only thing missing.

### A2A-R3-02 — `bridge.rs:handle_message_stream` leaks the broadcast channel if `update_status(Working)` fails after `subscribe_all`
- **File**: `src/a2a/adapter/server/bridge.rs:177-247`
- **Severity**: High
- **Category**: resource / concurrency
- **Description**: The function subscribes BEFORE transitioning to `Working`, but the `tokio::spawn` that owns the `cleanup_task` call is created AFTER `update_status(Working)` succeeds. If `update_status` fails (e.g. concurrent cancellation, the task moved to a terminal state, store-level invariant violation), the function returns `Err(e)` and the channel created by `subscribe_all` is never released. A concurrent `tasks/cancel` or `tasks/resubscribe` would compound the leak. The `cleanup_task` early-return branches inside the spawned task catch the post-spawn failure path, but the pre-spawn window is unprotected.
- **Evidence**:
  ```rust
  // bridge.rs:181-194
  let stream = self.streaming.subscribe_all(task_id).await?;       // channel created
  self.task_manager
      .update_status(task_id, TaskState::Working, None)
      .await?;                                                    // if this errors, leak
  let working_event = TaskStatusUpdateEvent { ... };
  let _ = self.streaming.broadcast_status(task_id, working_event).await;
  // ... build request, then spawn cleanup owner ...
  tokio::spawn(async move { /* cleanup_task lives here */ });
  Ok(stream)
  ```
- **Suggested fix**: Wrap the `task_id` in a `Drop` guard that calls `cleanup_task` on early return, or move the `subscribe_all` call down so it happens AFTER `update_status(Working)`. The current ordering was a deliberate race-avoidance choice (subscribe before broadcast) — keep order but add a RAII guard.
- **Verification**: Re-read the `cleanup_task` call sites in `bridge.rs`. The only three call sites are inside the spawned task. There is no path that returns `Err` after `subscribe_all` and before `tokio::spawn` that cleans up.

### A2A-R3-03 — `TaskStore` cap is silently bypassed when all tasks are active (no terminal tasks to evict)
- **File**: `src/a2a/adapter/server/task_store.rs:13-15, 25-58`
- **Severity**: High
- **Category**: resource / correctness
- **Description**: `evict_terminal_tasks` checks `if tasks.len() < MAX_TASKS { return; }` and only evicts terminal tasks. The function preserves active (non-terminal) tasks. If all 10,000 tasks are in `Working`/`InputRequired`/`AuthRequired`, no eviction occurs and the map grows unboundedly. The cap is a "memory leak in a slow-burn DDoS scenario" — a peer that forces many long-running tasks pins unlimited memory. The doc comment correctly identifies the goal but the implementation only enforces the cap when terminal tasks exist.
- **Evidence**:
  ```rust
  // task_store.rs:13
  const MAX_TASKS: usize = 10_000;
  // task_store.rs:33-36
  let mut terminal: Vec<...> = tasks.iter()
      .filter(|(_, t)| t.status.state.is_terminal())
      ...
  // (no eviction branch when terminal.is_empty())
  ```
- **Suggested fix**: Either reject `create_task` with `InvalidRequest` when `len >= MAX_TASKS` and no terminal tasks can be evicted, or fall back to evicting the oldest active task with a `tracing::warn!`. Add a per-task size cap (history + artifacts bytes) as a second defense.
- **Verification**: Read the `create_task` body (lines 77-91). The `evict_terminal_tasks(&mut tasks)` call is unconditional, but the function returns early when there are no terminal tasks. There is no other cap.

### A2A-R3-04 — `bridge.rs:handle_message_*` accepts a re-entrant `message/send` for a task already in `Working`, double-executing the agent
- **File**: `src/a2a/adapter/server/bridge.rs:96-144, 164-310`
- **Severity**: High
- **Category**: correctness
- **Description**: Both `handle_message` and `handle_message_stream` catch `A2AError::InvalidRequest` from `create_task` ("Task already exists") and continue as if the new request were a continuation. They then call `update_status(Working)` and execute the run. If the task is already in `Working` (a concurrent delegation, or a retry after a silent failure), `update_status(Working)` is a no-op (same-state transition is allowed by `can_transition_to`) and `execution_adapter.execute(...)` is invoked a second time. Two concurrent runs on the same `task_id` produce interleaved history/artifact writes and double-billed execution.
- **Evidence**:
  ```rust
  // bridge.rs:96-103
  match self.task_manager.create_task(task_id, context_id).await {
      Ok(_) => {}
      Err(A2AError::InvalidRequest(_)) => {
          // Task already exists — continue with existing task
      }
      Err(e) => return Err(e),
  }
  // ... nothing validates the existing task is in a state to accept work ...
  // bridge.rs:107-113 (sync path) and 185-194 (stream path) then re-execute
  ```
  And `task_store.rs:178-180` allows `Working → Working`:
  ```rust
  // task.rs:can_transition_to
  if self == target { return true; }
  ```
- **Suggested fix**: After catching the `Task already exists` arm, call `get_task` and reject with `A2AError::InvalidRequest` if the task is in `Working` (or any non-`InputRequired`/`Submitted` state). Alternatively, introduce a new error variant `TaskBusy` and map it to a JSON-RPC error code.
- **Verification**: Cross-checked `try_exact_name` semantics in `smart_router.rs` and the `can_transition_to` rules in `task.rs:46-87`. The `self == target` short-circuit is deliberate (idempotent self-update) but leaves the door open for the bug above.

### A2A-R3-05 — `NotificationService` configs grow without bound; no TTL or task-expiry cleanup
- **File**: `src/a2a/service/notification.rs:90-183`
- **Severity**: Medium
- **Category**: resource / security
- **Description**: `set_config` accepts any task ID and stores the config in an `AsyncRwLock<HashMap>`. There is no max-entries cap, no TTL, no eviction on task completion. A peer can call `tasks/pushNotificationConfig/set` with 100k distinct task IDs; each config holds a URL and a `token` (sensitive) in memory forever. The `TokenStore` cap-naught applies here too — the `token` is preserved indefinitely.
- **Evidence**:
  ```rust
  // notification.rs:113-128
  let mut configs = self.configs.write().await;
  configs.insert(config.task_id.clone(), config.clone());
  Ok(config)
  ```
  No `if configs.len() >= CAP` check. No `task_id` lifecycle hook.
- **Suggested fix**: Add a `MAX_CONFIGS` constant and reject `set_config` when exceeded, or hook `add_artifact` / `update_status` in `TaskStore` to call `delete_config(task_id)` when the task transitions to terminal. At minimum, add a TODO comment so the leak is documented.
- **Verification**: Searched for `delete_config` call sites — the only caller is the `tasks/pushNotificationConfig/delete` RPC. No background reaper. The same module already enforces scheme + SSRF policies on `set_config`, so adding a size cap is the same style of fix.

### A2A-R3-06 — `health_check_pass` is sequential and `spawn_health_monitor` allows overlapping passes if a pass takes longer than the interval
- **File**: `src/a2a/service/card_refresh.rs:90-141`
- **Severity**: Medium
- **Category**: concurrency / resource
- **Description**: `spawn_health_monitor` uses `tokio::time::interval(interval)` with default `MissedTickBehavior::Burst`. If a single pass (N agents × up-to-120s `A2AClient::timeout`) takes longer than the interval, the next tick fires immediately, spawning a concurrent pass. Concurrent passes take the same per-agent write lock in `CardRegistry::upsert`, so they serialize on the registry and net out to thundering-herd probing. With 100 agents and a 30s interval, 3-4 passes can run concurrently. CPU, network, and `reqwest::Client` pool pressure grow linearly with the overlap.
- **Evidence**:
  ```rust
  // card_refresh.rs:126-141
  pub fn spawn_health_monitor(...) {
      tokio::spawn(async move {
          let mut ticker = tokio::time::interval(interval);
          ticker.tick().await; // skip immediate
          loop {
              ticker.tick().await;
              let n = health_check_pass(&registry, &pool).await;  // sequential N agents
              ...
          }
      });
  }
  ```
- **Suggested fix**: Hold the `JoinHandle` of the previous pass and `await` it before launching the next, OR use `ticker.set_missed_tick_behavior(MissedTickBehavior::Delay)` and serialize via a `Semaphore`. Concurrency cap with a `tokio::sync::Semaphore::acquire_many_owned(N)` is also viable.
- **Verification**: `tokio::time::interval` defaults to `Burst` (see `tokio::time::Interval::new`). The `health_check_pass` body iterates `agents`-by-`agents` (lines 105-122) and calls `pool.health_check(...)` which hits the network. No semaphore. No `JoinHandle` checked.

### A2A-R3-07 — `fold_stream` accepts `is_final: true` without verifying `state.is_terminal()`, can report a fake success
- **File**: `src/a2a/adapter/client/sse_stream.rs:243-291`
- **Severity**: Medium
- **Category**: correctness
- **Description**: `fold_stream` sets `final_state = Some(ev.status.state)` whenever `ev.is_final || ev.status.state.is_terminal()`. The `is_final` flag is supplied by the remote peer (or by the server's `TaskStatusUpdateEvent::new` constructor, which derives `is_final` from `state.is_terminal()`). A malicious / buggy remote can send `is_final=true, state=Working`, causing `success = true` to be reported to the parent agent even though the task is still in progress. The result is an early caller-side decision that the task is done.
- **Evidence**:
  ```rust
  // sse_stream.rs:265-268
  if ev.is_final || ev.status.state.is_terminal() {
      final_state = Some(ev.status.state);
  }
  // ...
  // sse_stream.rs:284
  let success = stream_error.is_none() && !failed && got_terminal;
  ```
- **Suggested fix**: Drop the `ev.is_final` arm — only trust `state.is_terminal()`. Or, conversely, require both conditions: `ev.is_final && ev.status.state.is_terminal()`. The downstream caller only needs the truth, not the hint.
- **Verification**: Read the server-side `TaskStatusUpdateEvent::new` constructor (`events.rs:54-69`) which sets `is_final = status.state.is_terminal()` — so an honest server always agrees. The vulnerability is purely against a malicious / non-conformant remote.

### A2A-R3-08 — `extract_credentials` byte-slices ASCII-only, mismatches RFC 7235 whitespace separators
- **File**: `src/a2a/adapter/server/routes.rs:345-365`
- **Severity**: Low
- **Category**: api-design / correctness
- **Description**: `auth.get(..7)` and `auth.get(7..)` require `7` to lie on a UTF-8 boundary. (`str::get` returns `None` rather than panicking, so this is safe.) The check `prefix.eq_ignore_ascii_case("bearer ")` also requires exactly 7 bytes spelling `"Bearer "` (case-insensitive). Real-world auth headers may use `Bearer\tabc` (tab) or a non-breaking space as the scheme separator, both RFC 7235-compatible. The current code silently classifies these as `Credentials::None`, falling through to X-API-Key and ultimately to `Credentials::None` — every tab-separated bearer token is rejected.
- **Evidence**:
  ```rust
  // routes.rs:347-358
  if let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
      if let Some(prefix) = auth.get(..7) {
          if prefix.eq_ignore_ascii_case("bearer ") {
              if let Some(token) = auth.get(7..) {
                  return Credentials::BearerToken(token.to_string());
              }
          }
      }
  }
  ```
- **Suggested fix**: Iterate over `(scheme, rest) = auth.split_once(is_whitespace)` with a `char::is_ascii_whitespace` predicate (or `splitn(2, char::is_whitespace)`). Strip OWS from the token. Or use the `headers` crate's typed `Authorization<Bearer>` parser.
- **Verification**: The companion tests at `routes.rs:419-460` only cover the no-extra-whitespace cases. No test for tab/non-breaking-space input. The `token_store` does already reject empty tokens (`token_store.rs:36-39`), so this is conservative, not a security bypass.

### A2A-R3-09 — `pushNotificationConfig` registration is orphaned when `message_handler.handle_message` fails
- **File**: `src/a2a/adapter/server/request_processor.rs:149-244`, `src/a2a/adapter/server/routes.rs:122-218`
- **Severity**: Medium
- **Category**: resource / error-handling
- **Description**: Both the sync `handle_message_send` and the streaming `stream_message_send` register the `PushNotificationConfig` BEFORE calling `message_handler.handle_message`. If the handler fails (e.g. `bridge.rs:InternalError("No default agent registered")`), the config is left in `NotificationService.configs` with no associated task. The push webhook will never fire (no `notify_status_update` for a non-existent task), but the config (with the secret token) sits in memory until `tasks/pushNotificationConfig/delete` is called.
- **Evidence**:
  ```rust
  // request_processor.rs:191-219 (sync path)
  if let Some(push_params) = request.params.get("pushNotificationConfig").cloned() {
      ...
      if let Err(e) = self.state.notification.set_config(push_config).await {
          return JsonRpcResponse::from_a2a_error(request.id, &e);
      }
  }
  // ... then handle_message is called; if it fails, the config above is orphaned
  match self.state.message_handler.handle_message(...).await { ... }
  ```
- **Suggested fix**: Either register the config in the `bridge.rs` after the task is created, or add a compensating `delete_config` on the handler-error path. The simplest fix is to register on the success path only.
- **Verification**: The `delete_config` call sites (searched in `notification.rs`) are only the RPC handler and the test. No compensating cleanup in `request_processor.rs` or `routes.rs`.

### A2A-R3-10 — `bridge.rs:handle_message_stream` spawned task has no JoinHandle and no panic recovery
- **File**: `src/a2a/adapter/server/bridge.rs:228-301`
- **Severity**: Medium
- **Category**: error-handling / resource
- **Description**: The `tokio::spawn` that owns the post-execution status update + cleanup is detached. If the closure panics (e.g. `serde_json::to_string` on an odd payload, `attempt::unwrap` in a future code change), the panic is silently swallowed by the Tokio runtime. The cleanup at the end of the spawn body is skipped, so the broadcast channel leaks. Worse, the failure mode is invisible — no log line, no metric, no SSE error event for the subscriber.
- **Evidence**:
  ```rust
  // bridge.rs:228-301
  tokio::spawn(async move {
      match execution_adapter.execute(request, agent, emitter).await {
          Ok(()) => { ... }
          Err(e) => { ... }
      }
      let _ = streaming.cleanup_task(&task_id_owned).await;
  });
  ```
  No `AbortHandle`, no `catch_unwind`, no `tracing::error!` if the closure panics.
- **Suggested fix**: Wrap the body in `tokio::task::spawn` with a `JoinHandle` stored on the bridge, and use `tokio::select!` or `tokio::time::timeout` to surface hangs. Or wrap the body in `std::panic::AssertUnwindSafe` + `futures::FutureExt::catch_unwind` and log the panic. Add a `tracing::error!` if the spawn body exits abnormally.
- **Verification**: No `JoinHandle` is captured (`grep -n "JoinHandle" src/a2a/adapter/server/bridge.rs` returns 0 lines). The only error logging is inside the `Ok` / `Err` arms.

### A2A-R3-11 — `is_private_ip` does not classify IPv4-mapped IPv6 addresses
- **File**: `src/a2a/domain/security.rs:230-247`
- **Severity**: Low
- **Category**: security / correctness
- **Description**: `infer_from_addr` for an IPv4-mapped IPv6 peer (`::ffff:10.0.0.1`) hits the IPv6 arm of `is_private_ip`. The first octet is `0x00`, so neither `fe80::/10` nor `fc00::/7` matches. The peer is classified as `Public`, requiring credentials. This is safer than the alternative (LAN bypass) but inconsistent with `infer_from_url` which also misses the mapped form. The mismatch is a sharp edge for dual-stack listeners.
- **Evidence**:
  ```rust
  // security.rs:230-240
  const fn is_private_ip(ip: &std::net::IpAddr) -> bool {
      match ip {
          std::net::IpAddr::V4(v4) => v4.is_private() || v4.is_link_local(),
          std::net::IpAddr::V6(v6) => {
              let octets = v6.octets();
              (octets[0] == 0xfe && (octets[1] & 0xc0) == 0x80)
                  || (octets[0] & 0xfe) == 0xfc
          }
      }
  }
  ```
- **Suggested fix**: In the IPv6 arm, check `v6.is_loopback()` / `v4-mapped` first and convert to IPv4 when applicable. Or use `std::net::IpAddr::to_ipv4_mapped()` for the conversion path.
- **Verification**: Read the tests at `security.rs:165-205`. All tests use `Ipv4Addr` or `Ipv6Addr` directly — no IPv4-mapped IPv6 test. The function is on a hot path for `TieredAuthenticator::authenticate` which calls `context.remote_addr.ip().is_loopback()` directly, so the IPv6 path is actually used for IPv4-mapped peers indirectly through `is_loopback` — but `is_loopback` is false for `::ffff:10.0.0.1`, so the `is_private_ip` is asked.

### A2A-R3-12 — `TaskStore::list_tasks` returns a fresh clone of every task every call, O(N) with no pagination
- **File**: `src/a2a/adapter/server/task_store.rs:149-176`
- **Severity**: Medium
- **Category**: perf
- **Description**: `list_tasks` clones every task (and every task's history + artifacts) on every call, then sorts the full vector, then truncates. With 10,000 tasks and average history/artifacts of 100 items each, that's ~1M allocations per call. The `ListTasksResult::next_cursor` is hard-coded to `None`, so pagination is impossible. The `cursor` field in `ListTasksParams` is parsed but never used.
- **Evidence**:
  ```rust
  // task_store.rs:155-178
  let mut result: Vec<A2ATask> = tasks
      .values()
      .filter(|t| { ... })
      .cloned()
      .collect();
  result.sort_by(|a, b| b.status.timestamp.cmp(&a.status.timestamp).then_with(|| a.id.cmp(&b.id)));
  let limit = params.limit.unwrap_or(100);
  result.truncate(limit);
  Ok(ListTasksResult { tasks: result, next_cursor: None })
  ```
- **Suggested fix**: After applying filters, sort and apply cursor-based offset (e.g. `(timestamp, id)` tuple cursors), then only clone the page slice. The `cursor` field is already in `ListTasksParams` — wire it up.
- **Verification**: `ListTasksParams::cursor` is parsed but never read in the body. `next_cursor` is always `None`. The `request_processor.rs:handle_tasks_list` passes the params through unchanged.

### A2A-R3-13 — `parse_event` clones the full `serde_json::Value` to attempt the first deserialization
- **File**: `src/a2a/adapter/client/sse_stream.rs:215-240`
- **Severity**: Low
- **Category**: perf
- **Description**: Every SSE event pays for one `Value` clone (the entire JSON tree) before falling through to the `<event_type>`-keyed fallback. For large artifact updates (multi-KB code blocks, embedded JSON metadata), this is a non-trivial allocation per event. The clone is necessary because `serde_json::from_value` consumes the value.
- **Evidence**:
  ```rust
  // sse_stream.rs:222-227
  if let Ok(event) = serde_json::from_value::<UpdateEvent>(payload.clone()) {
      return Some(event);
  }
  ```
- **Suggested fix**: Use `serde_json::Value::as_str()` and route on the `kind` field directly, or use `serde_path_to_error` with a borrowed `&Value`. Acceptable for now; flag for a future pass.
- **Verification**: The clone is the only allocation in the hot path. The fallback path is `serde_json::from_value` on the original `payload`, so the clone is discarded after the first attempt.

### A2A-R3-14 — `extract_credentials` returns `Credentials::BearerToken("")` for `Authorization: Bearer ` (just the scheme)
- **File**: `src/a2a/adapter/server/routes.rs:345-365`
- **Severity**: Low
- **Category**: api-design
- **Description**: When the header is exactly `"Bearer "` (7 bytes), `get(7..)` returns `Some("")`. The code forwards `Credentials::BearerToken("")` to the authenticator. The `TokenStore` correctly rejects empty tokens (`token_store.rs:36-39`), so this is not a security bypass — but the logic is misleading. The intent ("presence of bearer scheme" → authenticate) is conflated with the actual credential value. A reader would expect an empty bearer to map to `Credentials::None`.
- **Evidence**:
  ```rust
  // routes.rs:354-358
  if let Some(token) = auth.get(7..) {
      return Credentials::BearerToken(token.to_string());
  }
  ```
- **Suggested fix**: `if let Some(token) = auth.get(7..) { if !token.is_empty() { return BearerToken(token.to_string()); } }`. Or trim and check `!trimmed.is_empty()`.
- **Verification**: The `token_store.rs:is_valid` empty-string guard is the actual defense. The route code is relying on a downstream check that should be a precondition.

### A2A-R3-15 — `A2AClient::fetch_agent_card` returns `A2AError::AgentUnreachable` for HTTP 401/403 instead of `Unauthorized` / `Forbidden`
- **File**: `src/a2a/adapter/client/http_client.rs:73-110`
- **Severity**: Low
- **Category**: api-design / error-handling
- **Description**: Symmetric to `rpc_call` (which correctly maps 401/403 to `A2AError::Unauthorized` / `A2AError::Forbidden`), but `fetch_agent_card` does not. A 401 on `agent-card.json` (e.g. configured credentials rejected) is reported as `AgentUnreachable`, which the smart router treats as "agent is down" and may auto-exclude. The caller cannot distinguish auth from connectivity.
- **Evidence**:
  ```rust
  // http_client.rs:96-101
  if !status.is_success() {
      let body = response.text().await.unwrap_or_default();
      let snippet: String = body.chars().take(256).collect();
      return Err(A2AError::AgentUnreachable(format!(
          "agent-card endpoint returned HTTP {status} — {snippet}"
      )));
  }
  ```
- **Suggested fix**: Mirror the `rpc_call` pattern — match on 401/403 and map to the right error variant.
- **Verification**: Compared to `rpc_call` at lines 138-153 which does the right mapping. Inconsistency is the issue.

### A2A-R3-16 — `A2AClient::rpc_call` includes the first 256 bytes of the response body in the error message, may leak server details
- **File**: `src/a2a/adapter/client/http_client.rs:96-101, 138-153`
- **Severity**: Low
- **Category**: security
- **Description**: On non-2xx, the code reads the response body and embeds up to 256 bytes in the error message. The error is propagated to the caller (`dispatch` / `dispatch_sync` in `sub_agent.rs`) which surfaces it as the `SubAgentResult` summary or error. The body could include stack traces, internal hostnames, or partial API tokens. The function does not redact.
- **Evidence**:
  ```rust
  // http_client.rs:99-101
  let body = response.text().await.unwrap_or_default();
  let snippet: String = body.chars().take(256).collect();
  return Err(A2AError::AgentUnreachable(format!(
      "agent-card endpoint returned HTTP {status} — {snippet}"
  )));
  ```
- **Suggested fix**: Truncate more aggressively (e.g. 64 bytes) and pass the full body through a separate channel (e.g. attach to the error variant) for the operator-facing log only. The summary visible to the parent's LLM should be minimal.
- **Verification**: The error is fed to `SubAgentResult::failure(message)` and rendered verbatim to the user. No redaction step.

### A2A-R3-17 — `build_run_request` allocates a fresh `Arc<Mutex<Vec<…>>>` for `pending_media` on every call
- **File**: `src/a2a/adapter/server/bridge.rs:80-101`
- **Severity**: Low
- **Category**: perf
- **Description**: `Arc::new(tokio::sync::Mutex::new(Vec::new()))` is constructed per request. For a high-throughput A2A delegation load, this is one allocation per call. The `Arc::new` is unnecessary if the consumer holds the only reference — could be a `Box::new(Mutex::new(Vec::new()))`. Minor.
- **Evidence**:
  ```rust
  // bridge.rs:90-101
  pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
  ```
- **Suggested fix**: Use `Box::new` if the consumer doesn't need shared ownership. Or use `Mutex::new(Vec::new()).into()` to keep the type drop-in compatible.
- **Verification**: Searched for `pending_media` usage in `gateway/execution_engine` — the field is shared with the run lifecycle, so `Arc` may in fact be required. Mark as low-confidence; flag for the executor owner to confirm.

### A2A-R3-18 — `card_builder::simple_hostname` falls back to `std::env::var("HOSTNAME")` / `COMPUTERNAME` / `"unknown"`, may leak into the agent card id
- **File**: `src/a2a/service/card_builder.rs:60-66`
- **Severity**: Low
- **Category**: security
- **Description**: The agent card id is `format!("aleph-{}", simple_hostname())`. The hostname is whatever `$HOSTNAME` is set to in the daemon's environment. If the daemon is run in a container with a unique hostname (e.g. Kubernetes pod name), the agent card id leaks the pod identity to every remote agent that pulls `/.well-known/agent-card.json`. The card is served unauthenticated.
- **Evidence**:
  ```rust
  // card_builder.rs:60-66
  fn simple_hostname() -> String {
      std::env::var("HOSTNAME")
          .or_else(|_| std::env::var("COMPUTERNAME"))
          .unwrap_or_else(|_| "unknown".to_string())
  }
  ```
- **Suggested fix**: Make the card id configurable, default to a stable value like `aleph-local` or read from `[a2a].server.card_id`. Or use the configured `card_name` as the id.
- **Verification**: The card is served by `routes.rs:agent_card_handler` with no auth check. The id is consumed by `card_registry.rs:load_from_config` for slug collision checks. The K8s-pod-id leak is a real concern in shared cluster deployments.

### A2A-R3-19 — `TaskStore::create_task` does not validate `task_id` / `context_id` length or content
- **File**: `src/a2a/adapter/server/task_store.rs:78-91`
- **Severity**: Low
- **Category**: security
- **Description**: `task_id` and `context_id` are accepted as `&str` and stored as `String` keys. There is no length cap, no character whitelist, no normalization. A peer can submit `task_id` of 1 MB or control characters. This abuses the `HashMap` key and the `Vec<history>` index. The `MAX_TASKS` cap (10k) bounds the count, but the per-task size is unbounded — see A2A-R3-20.
- **Evidence**:
  ```rust
  // task_store.rs:78-91
  async fn create_task(&self, task_id: &str, context_id: &str) -> A2AResult<A2ATask> {
      let task = A2ATask::new(task_id, context_id);
      let mut tasks = self.tasks.write().await;
      if tasks.contains_key(task_id) {
          return Err(A2AError::InvalidRequest(format!(
              "Task already exists: {task_id}"
          )));
      }
      evict_terminal_tasks(&mut tasks);
      tasks.insert(task_id.to_string(), task.clone());
      Ok(task)
  }
  ```
- **Suggested fix**: Add `const MAX_ID_LEN: usize = 256;` and reject overly long ids with `InvalidParams`. Also reject control characters (use `char::is_control`).
- **Verification**: The `request_processor.rs:handle_message_send` generates a UUID by default (good), but the peer can pass a custom `taskId` (line 174-177). No id validation downstream.

### A2A-R3-20 — `TaskStore::update_status` and `add_artifact` have no per-task size cap
- **File**: `src/a2a/adapter/server/task_store.rs:107-145, 180-189`
- **Severity**: Medium
- **Category**: resource
- **Description**: A single task's `history` and `artifacts` can grow unbounded. Each `update_status` pushes one message into `history`. Each `add_artifact` pushes one artifact. A peer can issue millions of either, pinning RAM on a single task. The `MAX_TASKS` cap (10k) limits the *number* of tasks but not the size of any one task.
- **Evidence**:
  ```rust
  // task_store.rs:124 (update_status)
  if let Some(msg) = message {
      task.history.push(msg);
  }
  // task_store.rs:184 (add_artifact)
  task.artifacts.push(artifact);
  ```
  No `if task.history.len() >= MAX_HISTORY` check. No byte-size cap.
- **Suggested fix**: Add `MAX_HISTORY_PER_TASK` and `MAX_ARTIFACTS_PER_TASK` constants. Reject with `A2AError::InvalidRequest` when exceeded. Consider a per-task TTL via a background reaper.
- **Verification**: The `get_task(..., history_length)` truncation at the read side (lines 89-99) reads `task.history` which is already fully populated. The unbounded write is the issue.

### A2A-R3-21 — `RegisteredAgent.auth_token` is a `pub` `Option<String>` without serde redaction
- **File**: `src/a2a/port/agent_resolver.rs:23-40`
- **Severity**: Medium
- **Category**: security
- **Description**: `RegisteredAgent` derives `Serialize, Deserialize` and exposes `auth_token: Option<String>` with `#[serde(skip_serializing_if = "Option::is_none")]` (only on the optional wrap, not redacting the value when present). If anyone serializes a `RegisteredAgent` (e.g. a debug log, a future RPC, a metric exporter), the token is included. The `Health` enum and `RegisteredAgent` are also `pub` — others can read.
- **Evidence**:
  ```rust
  // agent_resolver.rs:25-39
  pub struct RegisteredAgent {
      pub card: AgentCard,
      pub trust_level: TrustLevel,
      pub base_url: String,
      pub last_seen: DateTime<Utc>,
      pub health: AgentHealth,
      #[serde(skip_serializing_if = "Option::is_none")]
      pub auth_token: Option<String>,
  }
  ```
- **Suggested fix**: Write a custom `Serialize` that omits the token, or replace the field with a `Secret<String>` newtype that implements `Debug`/`Serialize` with redaction. Alternatively, store the token in a separate `HashMap<card_id, Secret<String>>` outside `RegisteredAgent`.
- **Verification**: Searched for `serde_json::to_string(&registered_agent)` calls — none today, but the door is open. The `card_registry.rs::list_agents` returns the full struct, so any consumer that logs or serializes the list leaks the token.

### A2A-R3-22 — `bridge.rs:handle_message_stream` swallows `broadcast_status` errors with `let _ =`
- **File**: `src/a2a/adapter/server/bridge.rs:204-206, 266-270, 294-298`
- **Severity**: Low
- **Category**: error-handling
- **Description**: The three `let _ = self.streaming.broadcast_status(...).await;` calls silently discard any error. `broadcast_status` returns `A2AResult<()>` — the only currently-implementable error is `InternalError` (the broadcast sender is internally infallible). The `let _ =` is fine for the current adapter, but it loses any future error (e.g. webhook serialization failure, lock contention timeout). The `stream_hub.rs:140` `let _ = sender.send(...)` has the same shape.
- **Evidence**:
  ```rust
  // bridge.rs:204-206
  let _ = self.streaming.broadcast_status(task_id, working_event).await;
  ```
- **Suggested fix**: At minimum, log the error: `if let Err(e) = ... { tracing::warn!(...); }`. The `let _ =` annotation is a code smell if the function returns a real `Result`.
- **Verification**: Read the `A2AStreamingHandler` trait — `broadcast_status` returns `A2AResult<()>`. The error is intentional to outlive the no-subscribers case (which the hub handles internally), but the trait is over-broad.

### A2A-R3-23 — `sse_stream.rs` multibyte split test is not gated by the CJK-locale assumption
- **File**: `src/a2a/adapter/client/sse_stream.rs:444-518`
- **Severity**: Low
- **Category**: test-coverage
- **Description**: The regression test for multibyte UTF-8 split (`parse_sse_byte_stream_multibyte_char_split_across_chunks`) uses CJK characters ("任务-1", "分析完成：黄金走势看涨"). The test is correct, but the protected code path (the `valid_up_to` + `carry` logic) was originally buggy — the inline comment "CJK is the primary language" suggests the fix targeted only CJK. Splits at a 3-byte UTF-8 boundary (e.g. emoji) are not exercised. Add a test for the 4-byte `😀` boundary.
- **Evidence**:
  ```rust
  // sse_stream.rs:456-462
  let event = TaskStatusUpdateEvent {
      task_id: "任务-1".to_string(),
      context_id: "上下文-1".to_string(),
      status: TaskStatus { ..., message: Some(A2AMessage::text(A2ARole::Agent, "分析完成：黄金走势看涨")), ... },
  ```
- **Suggested fix**: Add a parameterised test that runs the split at every non-boundary byte position and asserts the reassembled message equals the original. Include a 4-byte char (emoji) case.
- **Verification**: The existing test only covers a 3-byte CJK char. The `String::from_utf8` + `valid_up_to` logic is symmetric across byte widths, but a regression in handling 4-byte chars would not be caught.

### A2A-R3-24 — `A2AClient::new` constructs a fresh `reqwest::Client` per agent card fetch path
- **File**: `src/a2a/adapter/client/http_client.rs:43-69`
- **Severity**: Low
- **Category**: perf
- **Description**: `A2AClient::new` builds a new `reqwest::Client` (which itself pools connections, holds a TLS config, etc.) on every instantiation. `A2AClientPool::get_or_create` caches them per agent, so the steady-state is fine. But `card_refresh.rs:refresh_all_cards` builds a fresh `A2AClient` per agent (lines 47-50) instead of using the pool, doubling the client count and bypassing connection reuse for the refresh pass.
- **Evidence**:
  ```rust
  // card_refresh.rs:47-50
  let client = match &agent.auth_token {
      Some(token) => A2AClient::with_auth(&agent.base_url, token),
      None => A2AClient::new(&agent.base_url),
  };
  match client.fetch_agent_card().await { ... }
  ```
- **Suggested fix**: Use the pool's `get_or_create` (it already handles token rotation). Move the pool reference into `refresh_all_cards`.
- **Verification**: `card_refresh.rs:health_check_pass` correctly uses the pool. `refresh_all_cards` does not. The same `fetch_agent_card` is therefore done with a disposable client.

### A2A-R3-25 — `bridge.rs:handle_message` does not handle `TaskState::InputRequired` continuation semantics
- **File**: `src/a2a/adapter/server/bridge.rs:96-143`
- **Severity**: Low
- **Category**: correctness
- **Description**: When a task is in `InputRequired` state (the agent paused to ask the user a question), the A2A spec says `message/send` should resume with the user's reply. The current code catches "Task already exists" and unconditionally executes a new run via `execution_adapter.execute(...)`. The agent's resume hook is not invoked — the new input is treated as a fresh request, not a continuation. The A2A spec's `InputRequired` round-trip is effectively broken.
- **Evidence**:
  ```rust
  // bridge.rs:96-113
  match self.task_manager.create_task(task_id, context_id).await {
      Ok(_) => {}
      Err(A2AError::InvalidRequest(_)) => {
          // Task already exists — continue with existing task
      }
      ...
  }
  // ... always calls execute(request, ...), not a "resume" path ...
  ```
- **Suggested fix**: After "Task already exists", call `get_task` and route on the existing state. If `InputRequired`, dispatch to a resume entry point on the agent. If `Working`, reject (see A2A-R3-04). If terminal, error.
- **Verification**: `TaskState::InputRequired` is defined in `domain/task.rs:25`. The transition matrix allows `InputRequired → Working`. The bridge does not branch on the pre-existing state.

## Cross-cutting concerns

1. **Bridge↔StreamHub cleanup contract is fragile.** A2A-R3-02 and A2A-R3-10 both stem from the same structural issue: the streaming path is "subscribe → mutate → spawn cleanup owner → return stream", and there is no failure mode between subscribe and spawn that protects the channel. The `Drop`-guard pattern would fix both simultaneously.

2. **Synchronous vs streaming message handling is duplicated three times.** `bridge.rs:handle_message` vs `bridge.rs:handle_message_stream` is one pair; `request_processor.rs:handle_message_send` vs `routes.rs:stream_message_send` is another. Both pairs share the same `pushNotificationConfig` registration logic and the same `FileContent::validate` loop. Extract a shared `decode_message_params` helper to deduplicate and ensure drift-free evolution.

3. **Error propagation in `execute_delegation`'s `dispatch_sync`** is inconsistent with streaming. Streaming (`fold_stream`) maps `Failed`/`Rejected`/`Canceled` to `success = false`. Sync (`sub_agent.rs:165-200`) does the same now but only after the comment "A transport-level success can still carry a task that ended in a failed terminal state — mirror fold_stream". The two paths are written to the same contract but separately. A single `TerminalState::is_unsuccessful()` helper would be safer.

4. **The `auth_token` field is treated as a public secret.** A2A-R3-21 plus the fact that `RegisteredAgent::Clone` is used pervasively (e.g. in `upsert`, `rebuilt_agent`) means a single leaked log can dump the token. A `Secret<String>` newtype is the right fix; the codebase already has `security::secret_equal::secret_equal_bytes` for safe comparison.

5. **No consistent panic recovery.** A2A-R3-10 noted the spawned task has no `JoinHandle`. The same pattern is repeated in `spawn_card_refresh` and `spawn_health_monitor` (both detach with `tokio::spawn`, no error capture). A panic in any of these background tasks is silently swallowed. A daemon-level `JoinSet` or `tracing` instrumentation would help.

6. **Tests are dense but uneven.** Many tests use `#[tokio::test]` and construct a full `AgentLoopBridge` with `tempfile::TempDir` + `SessionManager` for what could be a single trait mock. The mock scaffolding in `request_processor.rs:dispatch` is excellent and should be reused. The `bridge.rs` tests could be ~40% smaller with a shared `make_bridge` helper.

## Summary
- **Total: 25 findings** (0 Critical, 5 High, 12 Medium, 8 Low)
- **Top priority items (must-fix)**:
  1. **A2A-R3-01** — `message/send` returns a hard-coded string, not the agent's output. Breaks the A2A protocol contract for every sync caller.
  2. **A2A-R3-02** — `handle_message_stream` leaks the broadcast channel on `update_status(Working)` failure. Subscriber hangs forever.
  3. **A2A-R3-03** — `TaskStore` cap is silently bypassed when all tasks are active. Memory leak in slow-burn DDoS.
  4. **A2A-R3-04** — Re-entrant `message/send` on a `Working` task double-executes the agent. History corruption + double-billed execution.
  5. **A2A-R3-21** — `RegisteredAgent.auth_token` is a public field, no redaction on (de)serialize. Token leak risk.

## What was NOT covered
- **Loom concurrency**: I did not run or model the loom tests. The `sync_primitives.rs` module comment notes that async locks are not loom-instrumented by design; the `AsyncRwLock` is `tokio::sync::RwLock` which has its own internal concurrency model.
- **Wire-protocol conformance**: I did not exhaustively check that every emitted SSE event matches the A2A spec's wire format. The `update_event_frame` function does the framing, and the tests cover the JSON-RPC wrapping, but a spec-level diff against the reference SDKs is out of scope.
- **Performance benchmarks**: No `cargo bench` was run. The `list_tasks` O(N) issue (A2A-R3-12) and the SSE `Value::clone` (A2A-R3-13) are flagged based on static analysis, not measured.
- **Cross-module contracts**: I did not verify the `ExecutionAdapter::execute` contract enforces the "unattended" flag surfaced in `bridge.rs:build_run_request`. That belongs to the gateway/execution_engine module.
- **`sub_agent.rs:emit_delegation_primitives`**: The fire-and-forget `tokio::spawn` for raw-memory writes is a separate concern from the A2A protocol body. I noted it implicitly in A2A-R3-10 but did not fully audit the memory-write path.
- **`builtin_tools/a2a_tools.rs`**: The `a2a_delegate` tool wrapper layer (in `src/builtin_tools/a2a_tools.rs`) is outside `src/a2a/` and was not reviewed.
