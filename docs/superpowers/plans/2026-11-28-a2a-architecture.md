# A2A lifecycle and interoperability implementation plan / A2A 生命周期与互操作实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal** 修复 A2A 任务隔离、取消和观察，再接通标准 JSON-RPC/SSE 与远程 Agent Capability。 Deliver isolated and observable A2A execution, standard protocol interoperability, and remote Agent Capability invocation.

**Architecture** `TaskStore` 固定任务归属和运行身份，`AgentLoopBridge` 使用 Core `ExecutionAdapter`。协议适配器共用操作入口。`CardRegistry` 是远程 Agent 的来源，Capability 绑定从它派生，实际发送前判定准入。 `TaskStore` binds ownership and run identity. The bridge executes via Core, while protocol adapters use the same operations. The remote agent registry supplies Capability bindings and dispatch admission.

**Tech stack** Rust, Tokio, Axum, serde, reqwest, existing Capability and Core execution APIs, official Python SDKs for interoperability.

**Spec** [A2A architecture / A2A 架构设计](../specs/2026-11-28-a2a-architecture-design.md) at `4774e5f64`.

## Global constraints / 全局约束

- Worktree `a2a-architecture` only. Default `[a2a] enabled = false` and credential-free loopback remains opt-in. Preserve unattended execution, current tool policy, configured SSRF policy, and memory capture. 仅在隔离分支工作，复用现有授权和内存链路。
- Cancel acknowledgement waits at most five seconds. Pending `-32603` has `data.reason = cancellation_pending`. A second run in the same context fails before task creation with `-32603` and `data.reason = context_busy`. 两类超时或忙都有可机器读取的原因。
- The standard JSON-RPC/SSE endpoint is `/a2a/rpc`. Missing `A2A-Version` means `0.3`. Keep `/a2a` and `/a2a/stream` and compatibility routes under `/a2a/legacy`. Interoperability covers text. Standard push, REST, gRPC, restart recovery, multi-tenant identity service, and file execution are separate scopes.
- Tests must observe effects at the actual route or dispatch boundary. Reference code is comparative evidence. Apply the published protocol and the approved design. 门禁验证实际发生的效果，协议以发布规范为准。

## Review focus / 审阅重点

| Input or failure / 输入或故障 | Test owner / 验收位置 |
|---|---|
| Same legacy alias from two token owners, including after reuse and retained push | Task 1 `recycled_alias_has_a_new_instance_and_no_inherited_push` |
| Core cancellation requested before admission or during completion | Task 2 `cancel_before_admission_never_enters_core` and `cancel_waits_for_core_settlement` |
| Terminal state or lagged receiver has no open-ended stream | Task 2 `resubscribe_terminal_or_lagged_task_ends` |
| Card advertises a non-default RPC path or a forged different origin | Task 4 `delegates_once_to_selected_rpc_and_refuses_unconfigured_origin` |
| Registry changes while a caller waits for the client pool | Task 5 `withdrawn_agent_never_sends_after_client_lookup` |

## File responsibilities / 文件分工

| Area / 层 | Files / 文件 |
|---|---|
| Inbound identity and task state / 入站身份与任务状态 | `src/a2a/adapter/auth/{token_store,tiered}.rs`, `src/a2a/port/{authenticator,task_manager}.rs`, `src/a2a/adapter/server/task_store.rs`, `src/a2a/service/notification.rs` |
| Run, observer, and HTTP composition / 运行、观察与 HTTP 接线 | `src/a2a/adapter/server/{bridge,mod,request_processor,routes,stream_hub}.rs`, `src/a2a/port/{message_handler,streaming}.rs`, `src/gateway/execution_adapter.rs`, `src/bin/aleph-server/commands/start/{mod.rs,builder/startup_resources.rs}` |
| Standard wire and discovery / 标准协议与发现 | Create `src/a2a/adapter/wire/{mod,v03,v10}.rs`. Modify `src/a2a/adapter/mod.rs`, `src/a2a/adapter/server/{request_processor,routes}.rs`, `src/a2a/service/card_builder.rs`, `src/a2a/config.rs`, `src/a2a/domain/error.rs`. Share DTO conversion with outbound code. |
| Outbound card, pool, and SSE / 出站卡片、连接池与 SSE | `src/a2a/adapter/client/{http_client,pool,sse_stream}.rs`, `src/a2a/service/{card_registry,card_refresh,smart_router}.rs`, `src/a2a/port/agent_resolver.rs`, `src/a2a/sub_agent.rs`, `src/builtin_tools/a2a.rs` |
| Capability and verification / Capability 与验收 | `src/capability/{zahir_facade,projection_host}.rs`, A2A registry and startup above, `src/a2a/tests.rs`, `docs/reference/{FEATURE_LOCATOR,SUBSYSTEM_ROUTING,SECURITY}.md`, and new `docs/reference/A2A.md` |

---

## Stage 1 / 第一阶段

### Task 1 — Scope legacy task operations and push by task instance / 兼容任务与推送归属

**Interfaces** `TokenStore::principal_for(&self, token: &str) -> Option<A2APrincipalId>`. `A2AAuthPrincipal` carries `principal_id: A2APrincipalId`. `TaskStore::resolve_owned(&self, owner: &A2APrincipalId, task_id: &str) -> A2AResult<TaskInstanceId>` resolves a public ID to its private instance. Task claim and list operate within `owner`. Stream and push operations use `TaskInstanceId` internally, preserving the public ID in events.

- [ ] Add route tests `distinct_credentials_scope_every_task_face` and `recycled_alias_has_a_new_instance_and_no_inherited_push`. Assert that two verified credentials and loopback are independent. For each foreign or absent operation, compare the JSON-RPC `error.code` and shape. Assert that listing has zero foreign rows, denied cancellation causes zero adapter calls, and rejected push leaves no callback. Under bounded eviction, recreate the alias and assert unequal instance IDs and zero old callbacks for new events.
- [ ] Run `cargo test --locked -p alephcore --lib -- distinct_credentials_scope_every_task_face` and `cargo test --locked -p alephcore --lib -- recycled_alias_has_a_new_instance_and_no_inherited_push`. Confirm the tests expose the current missing identity and coupled alias state.
- [ ] Assign opaque identities at `TokenStore` construction. Reuse its entire-table secret comparison and blank-token rejection. Validate provided credentials before the loopback branch. Index the existing task store by owner and public ID where alias resolution is required, and carry the instance ID through authorization, session keys, subscription, and notification. Apply task/context scope before side effects. Retain the current bounded terminal eviction and release stale push configurations.
- [ ] Run both focused tests and `cargo test --locked -p alephcore --lib -- a2a::`. Commit `fix(a2a): scope task access to authenticated principal`.

### Task 2 — Settle each task from its Core run / 按 Core 结果结算任务

**Interfaces** One `AgentLoopBridge` send operation accepts typed owner, public task/context selectors, and message, and returns an admitted task plus its observation handle. `TaskStore` supplies `finish_run(instance: TaskInstanceId, run_id: &str, outcome: RunOutcome)` and an owner-scoped observation operation returning a committed snapshot and live receiver at one observation point. Use `ExecutionAdapter::chat_run_cancellation` and `execute_chat_run` on the production adapter. Use the existing `ChatRunCancellation` request operation for pre-admission and active cancellation. Keep `ExecutionAdapter::cancel` as the existing Core route where needed by the actual adapter contract. Test doubles must consume the real cancellation contract.

- [ ] Add `cancel_before_admission_never_enters_core`, `cancel_waits_for_core_settlement`, `resubscribe_terminal_or_lagged_task_ends`, and `second_send_to_same_context_is_busy`. Assert that cancel-before-start has zero execution effects, completion wins without being overwritten, delayed settlement reports `cancellation_pending`, final lookup agrees with the execution outcome, and a resumed `InputRequired` run has a new run ID. Assert that the initial snapshot and subsequent event sequence have no gap, that terminal or lagged subscriptions end, and that a busy second send causes zero new task or Core effects. In `awaiter_disconnect_leaves_task_queryable`, drop the HTTP waiter after admission and verify that the owned run still settles and is readable.
- [ ] Run `cargo test --locked -p alephcore --lib -- a2a::adapter::server`. Confirm the behavior regressions before changing the orchestration.
- [ ] Consolidate blocking, streaming, and immediate send around one claim and server-owned execution. Bind the context slot and cancellation handle atomically at claim. Stamp Core busy mode to avoid steering if a Core admission race occurs. Settle from the Core result, commit snapshots before publication, emit artifacts before terminal status, and clean up the live hub once. Update `routes.rs` and `request_processor.rs` to use the same authorized operation for subscribe and cancel. Remove synchronous-only settlement, synthetic cancel, and empty late subscription paths. Keep policy and output capture at the existing Core boundary.
- [ ] Give refresh and health workers a cancellation/join handle in `StartupResources`. Use `shutdown_joins_a2a_workers` to assert the work stops and a late result does not restore a removed or replaced peer. Keep worker ownership with production startup and cleanup.
- [ ] Run focused server and execution adapter tests plus `cargo check --locked -p alephcore --bin aleph-server`. Commit `fix(a2a): settle task lifecycle through core execution`.

## Stage 2 / 第二阶段

### Task 3 — Serve supported standard operations over JSON-RPC/SSE / 标准 JSON-RPC/SSE 服务

**Interfaces** `A2aWireVersion::{V03,V10}` selected from `A2A-Version`. `wire::decode_request(version, method, params) -> A2AResult<Operation>` and `wire::encode_result(version, response) -> A2AResult<serde_json::Value>` share operation types with Tasks 1–2. Preserve legacy DTO conversion only on legacy bindings. Use server-issued task and context IDs on standard routes.

- [ ] Add protocol fixtures in `src/a2a/tests.rs`. Exercise version absence, `0.3`, `1.0`, and `1.2` such that unsupported `1.2` produces `-32009`. In 1.0, verify `SendMessage`, `SendStreamingMessage`, `GetTask`, `ListTasks`, `CancelTask`, and `SubscribeToTask`. Assert `returnImmediately: true` produces an admitted task before releasing the execution barrier, the stream begins with a task, and resubscribe receives an initial snapshot. In 0.3 verify the slash-delimited methods, `blocking`, standard part tags, and error mapping. Compare same-owner paginated IDs, exercise token-independent loopback, and assert invisible or invalid context references return equivalent not-found responses. Send a nonempty unsupported `tenant` and assert a version-appropriate rejection before creating a task. Decode file and structured parts, then verify that the text-only execution adapter rejects them before starting Core. Exercise legacy URLs and the standard default card.
- [ ] Run `cargo test --locked -p alephcore --lib -- a2a::tests`. Confirm the versioned routes and discovery assertions fail against legacy-only behavior.
- [ ] Implement bounded typed 0.3 and 1.0 conversion, explicit method dispatch, 1.0 scoped pagination, the JSON-RPC/SSE route, and a shared card builder for default and legacy discovery. Accept supported text, reject unsupported input with version-specific errors, and preserve the existing parser and request size limits. Derive advertised addresses from optional `[a2a.server] advertised_base_url` or Gateway startup. Verify transport, capabilities, authentication tags, field differences, and terminal bools against the two pinned specs.
- [ ] Run the protocol and legacy fixtures. Commit `feat(a2a): serve standard JSON-RPC and SSE bindings`.

### Task 4 — Delegate once through the selected card interface / 按卡片地址只发送一次

**Interfaces** `wire::decode_agent_card(value: serde_json::Value) -> A2AResult<DiscoveredCard>` returns a canonical card and a validated candidate interface list. `CardRegistry` stores the selected `ResolvedEndpoint` with the peer record. `A2AClientPool::get_or_create` takes that immutable record. Select call mode and prepare the request before awaiting the first remote execution response.

- [ ] Add `delegates_once_to_selected_rpc_and_refuses_unconfigured_origin`, `ambiguous_stream_open_never_resends_unary`, and `sse_preserves_utf8_across_byte_chunks`. Assert the advertised endpoint receives exactly one POST, a different allowed-by-DNS but unconfigured credential destination receives zero, and a stream request with unknown delivery does not trigger a second execution. Verify the SSE reducer returns complete multi-byte text and reports the actual terminal state after split frames and keep-alives. Add a refresh race test checking that stale resolution does not overwrite a replacement.
- [ ] Run `cargo test --locked -p alephcore --lib -- a2a::adapter::client` and `cargo test --locked -p alephcore --lib -- a2a::sub_agent` to pin current behavior.
- [ ] Reuse the card factory and reqwest client. Resolve the version/endpoint and mode before sending. Apply the same outbound SSRF policy and same-origin credential test to the selected address and keep redirects disabled. Respect operator-pinned destinations and existing legacy peers without retrying a submitted call. Key the pool by dispatch-relevant configuration. Use the same selected endpoint and generation checks in refresh and health workers. Reuse or correct the existing bytes-first SSE frame parser.
- [ ] Run client, refresh, and subagent suites. Commit `fix(a2a): dispatch once using the advertised agent interface`.

### Task 5 — Mount Agent Capability through the live registry / 挂载远程 Agent Capability

**Interfaces** Derive agent IDs in the `a2a` capability namespace from the `CardRegistry` peer IDs. Expose a snapshot plus live registry revision through `ZahirFacade`/`ProjectionHost`. Store the invocation generation in the existing ownership binding and derive callability from the authoritative source. Construct the outbound call using the peer selected by that binding. Once preparation is complete, `CardRegistry::admit_dispatch` checks the captured generation within the same mutation boundary as withdrawal and returns an immutable admitted call.

- [ ] Add `mount_lists_and_resolves_remote_agent_without_duplicate_registry`, `withdrawn_agent_never_sends_after_client_lookup`, and `health_update_preserves_generation`. Assert `CapabilityKind::Agent` is observable from production-shaped composition, invalid visibility is denied, source replacement invalidates the old lease, retired binding receives zero POST after a paused pool lookup, and admitted-before-withdrawal finishes at most once. Check the default-disabled and common shutdown paths.
- [ ] Run `cargo test --locked -p alephcore --lib -- a2a::` and `cargo test --locked -p alephcore --lib -- capability::zahir_facade` to isolate the new mounting tests and existing Tool/Memory paths.
- [ ] Wire the registry before production readiness by reusing source-publication, scope, and ownership primitives. Ensure remove, replacement, and shutdown invalidate call leases at their mutation point. Route `a2a_delegate` through the real Capability resolution and final admission, following existing tool permissions and reusing `A2ASubAgent` and memory capture. Avoid adding a second endpoint or secret registry. Preserve observation of health without rotating a dispatch generation for health-only updates.
- [ ] Rerun the focused suites and server build. Commit `feat(a2a): mount remote agents as capabilities`.

### Task 6 — Exercise mounted routes and publish bilingual reference / 验证接线并校对双语文档

- [ ] Run the pinned `a2a-python` v0.3.26 and v1.0.3 clients against default discovery and real Gateway HTTP routes using an isolated test instance. Verify send, SSE, status, and cancellation with a controlled Core run. Verify outbound dispatch to a pinned SDK or a spec-conforming peer at a custom endpoint. Record exact versions, commands, and confirmed results. Try `ra2a` against the shared RPC endpoint and record observed interoperability and any limitations separately from SDK evidence.
- [ ] From the isolated branch, run the `a2a::`, execution adapter, and Capability focused gates, `cargo test --locked -p alephcore --lib --no-run`, `cargo test --locked -p alephcore --bins`, `cargo check --locked -p alephcore --bin aleph-server`, and `git diff --check`. Verify repo formatting or check touched files separately and compare unrelated diagnostics with the base. Confirm superseded call sites and dead production paths are absent.
- [ ] Add bilingual `docs/reference/A2A.md` and connect it from `FEATURE_LOCATOR.md` and `SUBSYSTEM_ROUTING.md`. Update the A2A boundary in `SECURITY.md`. Document wire/feature support, endpoint migration, setup, cancellation and task retention, and the verification performed, with code anchors. Validate navigation and actual config names. Commit `docs: describe and verify mounted A2A support`.

## Execution / 执行方式

The server state and route tests share one writer through Tasks 1–3. A separate writer may prepare outbound SSE framing and reference fixtures in its own worktree while the server changes are in flight, with the client/wire merge after Task 3 contracts are available. Use independent read-only reviewers for authorization/settlement and final integration. Model assignments follow the operator's cost tiers and delegation rules. 任务 1–3 的服务端入口保持单写者。独立出站工作和互操作验收可并行，交叉依赖按协议契约合并。
