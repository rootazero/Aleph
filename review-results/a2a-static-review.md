# A2A Static Review (`src/a2a/`)

**Worktree**: `.worktrees/review-a2a` (branch `review/a2a`)
**Scope**: 38 files, ~11,092 lines under `src/a2a/`
**External callers**: `bin/aleph-server/commands/start/mod.rs`, `gateway/server/mod.rs`, `builtin_tools/a2a_tools.rs`, `agents/subagent_spawner/mod.rs`, `config/structs.rs` — every type renamed or removed below must remain reachable through `crate::a2a::*`.

---

## Headline counts

| Module                        | P0 | P1 | P2 |
|-------------------------------|----|----|----|
| adapter/auth/                 | 0  | 0  | 0  |
| adapter/client/               | 1  | 0  | 1  |
| adapter/server/               | 0  | 4  | 2  |
| domain/                       | 0  | 0  | 3  |
| port/                         | 0  | 0  | 1  |
| service/                      | 0  | 1  | 2  |
| sub_agent.rs / config.rs      | 0  | 0  | 1  |
| **total**                     | **1** | **5** | **10** |

---

## Findings

### P0 — security / silent control failure

#### [P0] silent-reqwest-fallback-defeats-redirect-policy
- File: `src/a2a/adapter/client/http_client.rs:67`
- Rule: `error-swallowing` (Error Handling / Security)
- Context: `A2AClient::new()`, public client constructor used by every outbound RPC; sets `Policy::limited(0)` redirects explicitly to harden against SSRF pivots via the operator-configured `base_url`.
- Before:
  ```rust
  let http = reqwest::Client::builder()
      .redirect(reqwest::redirect::Policy::limited(0))
      .build()
      .unwrap_or_else(|_| reqwest::Client::new());
  ```
- After:
  ```rust
  // Builder config is fully static (a redirect policy toggle) — the only way
  // build() can fail is a reqwest/TLS-backend bug. Falling back to a default
  // client silently DEFEATS the no-redirect policy this whole block exists
  // to enforce. Panic loudly instead: misconfiguration is a build-time
  // invariant, not a runtime decision.
  let http = reqwest::Client::builder()
      .redirect(reqwest::redirect::Policy::limited(0))
      .build()
      .expect("reqwest ClientBuilder configuration is static and must succeed");
  ```
- Why: The entire purpose of the explicit `.redirect(Policy::limited(0))` is to stop an attacker-controlled `base_url` from pivoting through `Location:` headers to Aleph's private network. Silently falling back to a default client on builder error yields a client that follows redirects — and the only signal that the security policy is off is `tracing`'s absence. Builder failure here would indicate a reqwest/TLS backend bug, which is a process-startup concern, not a per-request decision.

---

### P1 — reliability / observability

#### [P1] swallowed-broadcast-status-streaming-completion
- File: `src/a2a/adapter/server/bridge.rs:362`
- Rule: `error-swallowing` (Error Handling)
- Context: inside the spawned `TURN_CONTEXT.scope` arm for streaming task completion; `broadcast_status` is the terminal-state signal SSE consumers need.
- Before:
  ```rust
  let _ = streaming
      .broadcast_status(&task_id_owned, completed_event)
      .await;
  info!(task_id = %task_id_owned, "A2A bridge: streaming task completed");
  ```
- After:
  ```rust
  if let Err(e) = streaming
      .broadcast_status(&task_id_owned, completed_event)
      .await
  {
      tracing::error!(
          task_id = %task_id_owned,
          error = %e,
          "A2A bridge: failed to broadcast completion event"
      );
  }
  ```
- Why: `broadcast_status` returning `Err` means the SSE terminal event was dropped — `fold_stream` in the consumer will mark the task as not-finished and the caller sees a phantom success. The error is recoverable only with logs.

#### [P1] swallowed-broadcast-status-streaming-failure
- File: `src/a2a/adapter/server/bridge.rs:395`
- Rule: `error-swallowing` (Error Handling)
- Context: companion arm of the above, for streaming task failure.
- Before:
  ```rust
  let _ = streaming
      .broadcast_status(&task_id_owned, failed_event)
      .await;
  error!(task_id = %task_id_owned, error = %e, "A2A bridge: streaming task failed");
  ```
- After:
  ```rust
  if let Err(bcast_err) = streaming
      .broadcast_status(&task_id_owned, failed_event)
      .await
  {
      tracing::error!(
          task_id = %task_id_owned,
          error = %bcast_err,
          "A2A bridge: failed to broadcast failed-state event"
      );
  }
  ```
- Why: Same reason as above; the `error!` already logs the execution error but not the broadcast failure, hiding a second-order breakage.

#### [P1] swallowed-cleanup-task-spawn-tail
- File: `src/a2a/adapter/server/bridge.rs:413`
- Rule: `error-swallowing` (Error Handling)
- Context: trailing cleanup in the spawned streaming task; runs in both success and panic branches.
- Before:
  ```rust
  let _ = streaming.cleanup_task(&task_id_owned).await;
  ```
- After:
  ```rust
  if let Err(e) = streaming.cleanup_task(&task_id_owned).await {
      tracing::warn!(
          task_id = %task_id_owned,
          error = %e,
          "A2A bridge: cleanup_task failed; broadcast channel may leak"
      );
  }
  ```
- Why: `cleanup_task` removes the broadcast channel entry; failure here leaks the entry in `StreamHub.channels` (an unbounded `HashMap<String, broadcast::Sender<UpdateEvent>>`). A leaked entry keeps a channel alive for a terminal task and can confuse downstream subscribers.

#### [P1] swallowed-update-status-no-default-agent-sync
- File: `src/a2a/adapter/server/bridge.rs:183`
- Rule: `error-swallowing` (Error Handling)
- Context: `handle_message` (sync path); the only `task_manager.update_status` call in the no-default-agent branch.
- Before:
  ```rust
  let _ = self
      .task_manager
      .update_status(
          task_id,
          TaskState::Failed,
          Some(A2AMessage::text(
              A2ARole::Agent,
              "No default agent registered",
          )),
      )
      .await;
  ```
- After:
  ```rust
  if let Err(e) = self
      .task_manager
      .update_status(
          task_id,
          TaskState::Failed,
          Some(A2AMessage::text(
              A2ARole::Agent,
              "No default agent registered",
          )),
      )
      .await
  {
      tracing::error!(
          task_id,
          error = %e,
          "A2A bridge: failed to mark task Failed after missing-default-agent"
      );
  }
  ```
- Why: The task was atomically claimed (`claim_task` already promoted it to `Working`) so a swallowed failure leaves the task stuck in `Working` forever. The error path is recoverable only with logs.

#### [P1] swallowed-update-status-no-default-agent-streaming
- File: `src/a2a/adapter/server/bridge.rs:263`
- Rule: `error-swallowing` (Error Handling)
- Context: streaming counterpart of the above; same hazard in the `handle_message_stream` path.
- Before / After: identical to the sync arm — replace `let _ = ...` with the `if let Err(e) = tracing::error!` pattern.
- Why: Same as above; both arms must be patched in lock-step since the streaming path additionally has a half-subscribed SSE stream attached.

---

### P2 — listed but not fixed

#### [P2] unbounded-push-config-hashmap
- File: `src/a2a/service/notification.rs:60`
- Rule: `unbounded-collection` (Resource Safety)
- Context: `NotificationService.configs: AsyncRwLock<HashMap<String, PushNotificationConfig>>`. `set_config` upserts by `task_id` but nothing evicts after a task ends; long-running daemons accumulate one entry per task. There is a capacity ceiling elsewhere (TaskStore caps at 10_000) but no ceiling here.
- Why deferred: Touching this requires a task-completion hook that does not currently exist. A single-file eviction heuristic could orphan active configs. Filed for a follow-up.

#### [P2] public-enums-missing-non-exhaustive
- Files: `domain/task.rs:TaskState`, `domain/agent_card.rs:TransportProtocol`, `domain/security.rs:SecurityScheme`, `domain/security.rs:ApiKeyLocation`, `port/authenticator.rs:A2AAction`, `domain/events.rs:UpdateEvent`, `service/smart_router.rs:RoutingMethod`
- Rule: `public-enum-non-exhaustive` (API Design)
- Why deferred: Matches the aggregate-review conclusion on `shared::58-enum non_exhaustive gap`. Adding `#[non_exhaustive]` is a one-line attribute but breaks every exhaustive `match` site across the crate's binary and tool layers. Dedicated change with a migration plan.

#### [P2] public-field-on-domain-types
- Files: `domain/task.rs:A2ATask`, `domain/message.rs:A2AMessage`, `domain/agent_card.rs:AgentCard`, `domain/events.rs:TaskStatusUpdateEvent`, `service/smart_router.rs:RoutingDecision`
- Rule: `public-field-locks-layout` (API Design)
- Why deferred: These types cross serde + JSON-RPC + HTTP boundaries and the field layout is part of the wire format. Switching to accessors would force every reader site to change in lock-step.

#### [P2] public-enum-missing-derive
- Files: `domain/message.rs:A2AMessage` (no `PartialEq`), `domain/events.rs:TaskStatusUpdateEvent` (no `PartialEq`), `port/streaming.rs:A2AStreamingHandler::subscribe_all` return type
- Rule: `missing-debug-clone-partialeq` (API Design)
- Why deferred: Public types are public; adding `PartialEq` is mechanical but expands the API surface (semver). A single-pass sweep across all public a2a types belongs in a dedicated change.

#### [P2] inline-push-config-param-struct
- Files: `adapter/server/routes.rs:120` and `adapter/server/request_processor.rs:174`
- Rule: `duplicate-struct` (Maintainability)
- Context: Both files declare an identical `InlinePushConfig` struct for the `pushNotificationConfig` parameter of `message/send`. Lift to a single shared type in `service/notification.rs`.
- Why deferred: Pure DRY — non-functional. Trivial follow-up.

#### [P2] auth-context-headers-cloned-each-request
- File: `port/authenticator.rs:A2AAuthContext`
- Rule: `clone-in-hot-path` (Performance)
- Context: Every authenticated request clones the header `HashMap` once via `headers_to_map`. Small absolute cost but the headers map size is unbounded by request — a peer can send a few hundred custom headers.
- Why deferred: Not a measured hot path; one-time auth cost. Worth tracking if profiling later flags it.

#### [P2] sse-frame-serialization-silently-defaults-to-null
- File: `adapter/server/routes.rs:393`
- Rule: `silent-fallback-hides-errors` (Error Handling)
- Context: `serde_json::to_value(&event).unwrap_or_default()` — serialization failure becomes a `Null` SSE event delivered to consumers. Should log on failure path.
- Why deferred: `UpdateEvent` is locally controlled and `serde_json::to_value` on it should not fail; the `unwrap_or_default` is defensive. The downstream `JsonRpcResponse::serialize_ok` already handles this correctly with a -32603 fallback, so the SSE path is inconsistent with the synchronous path. Worth a small follow-up but not load-bearing.

---

## Findings fixed in this PR

- **[P0]** `adapter/client/http_client.rs:67` — `unwrap_or_else(|_| ...)` → `.expect("...invariant...")` so a builder failure cannot silently drop the no-redirect policy.
- **[P1]** `adapter/server/bridge.rs:183` — swallowed `update_status` after `No default agent` → log via `tracing::error!` and continue.
- **[P1]** `adapter/server/bridge.rs:263` — same as above, streaming counterpart.
- **[P1]** `adapter/server/bridge.rs:362` — swallowed `broadcast_status` for `Completed` event → `tracing::error!` on `Err`.
- **[P1]** `adapter/server/bridge.rs:395` — same as above, `Failed` event.
- **[P1]** `adapter/server/bridge.rs:413` — swallowed `cleanup_task` (broadcast-channel-leak risk) → `tracing::warn!` on `Err`.

## Findings deferred / cross-module

- **[P2]** `service/notification.rs:60` — unbounded push-config `HashMap` (needs task-completion hook across adapter).
- **[P2]** 7 public enums missing `#[non_exhaustive]` — aggregate-review conclusion: dedicated change with `try_match!` migration plan.
- **[P2]** Public-field structs (semver impact).
- **[P2]** Public types missing `PartialEq` derives (semver impact).
- **[P2]** Duplicate `InlinePushConfig` literal in routes.rs / request_processor.rs (DRY only).
- **[P2]** `routes.rs:393` silent `unwrap_or_default` on event serialization (defensive; inconsistent with sync path).

## State of negative

- **Not** running `cargo check` / `cargo build` (per user instruction; unified cargo check runs after all modules finish).
- **Not** running `cargo clippy` (would OOM at 11GB available).
- **Not** running `cargo test` (per AGENTS.md "无需cargo check" plan; delegated to post-merge verification).
- **Not** adding `#[non_exhaustive]` to public enums (cross-module semver impact; deferred per aggregate-review convention).
- **Not** capping `NotificationService.configs` (cross-module: requires a task-end hook that does not exist).
- **Not** switching public domain structs to accessors (semver; deferred).
- **Not** fixing `sse_from_update_stream` serialization fallback (purely defensive; sync path's `serialize_ok` already covers the same pattern).
- **Not** testing the build (no compile checks by design).