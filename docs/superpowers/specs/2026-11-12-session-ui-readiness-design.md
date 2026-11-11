# Session publication and UI readiness / 会话发布与 UI 就绪设计

**Status / 状态:** PROPOSED; no readiness implementation or contract approval in this session.

**Baseline / 基线:** `fa996e6407c1ba7adbdf871b9b4185a0257dafd8` on local `main` before this documentation branch. The checkpoint is integrated, not accepted for release. Resolve all paths against the current repository root; do not assume a particular machine or checkout location.

**Companion / 配套:** `docs/superpowers/plans/2026-11-12-session-ui-readiness-and-phase7-continuation.md` and `docs/superpowers/prompts/2026-11-12-phase7-main-restart.md`.

## 1. Decision and scope / 决策与范围

Recommend **A: publish addressed-session metadata before every successful response from the two real-engine `chat.send` / `agent.run` handlers that carries an addressed-session key**. This includes inline/slash success paths that enqueue no run and emit no RunAccepted; it is not limited to the branch containing `spawn_queued_run`. Errors, successes without an addressed-session key and other RPC handlers are outside this promise. This is a proposed external contract change, not a finding that the old RPC already promised readiness. Implementation requires explicit human acceptance of the cost below. The user requested a design and restart handoff, not a repair in this session.

推荐 A：真实 `chat.send` / `agent.run` 在成功返回前，完成响应所指会话的元数据 preparation。成功只承诺该行当时已提交并属于合法调用者；不承诺队列准入、run slot、模型开始、surface 可用、渲染成功或永不撤销。

**Load-bearing trade-off / 必须明确接受的代价:** an authorized, well-formed request can create a legitimate session row even for a no-run inline success, or before a later queue/spend/run-admission denial, timeout, purge or cancellation. A committed row is not rolled back merely because the caller disconnects. There is one storage-operation latency before success. No speculative empty-row deletion is added.

If the requirement is instead “no row until actual run admission,” select **B**, not an implicit compromise: retain lazy creation and design server-owned readiness notification plus caller waiting. That requires explicit rejection/cancellation/inline/side-session/disconnection semantics and a revised acceptance scenario. It is a separate design gate, not a sleep/retry patch to make the original QA green.

## 2. What is known / 已知事实

### 2.1 Real response path, not fallback / 真实入口

- `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1128` registers the real handler.
- `src/bin/aleph-server/server_init.rs:306` defines `handle_chat_send_with_engine`; visibility is checked at `:388`, request construction at `:445`, slash stamping at `:464`, background queue spawn at `:470`, response success at `:489`.
- `src/gateway/busy_queue/spawn.rs:75` executes background work. `src/gateway/execution_engine/execute.rs:516` calls the later preparation helper.
- `src/gateway/agent_instance.rs:425` currently returns `()` and only warns on `get_or_create` failure. `src/gateway/execution_engine/run_loop/mod.rs:446` preserves this unit result. Awaiting either current helper does **not** establish successful publication.

The source has no enforced publication-before-success happens-before edge. Background scheduling could still win in another run; do not claim the response always precedes creation.

### 2.2 E2 evidence / 实际复现

The unchanged original QA returned from `chat.send`, then immediately attempted `capabilities.ui.attach`. A fresh, qualified Core binary built successfully with warnings; the original runtime exited 1 with `-32099`, `session is unavailable`. Same-process diagnostic output observed `UI_GET_METADATA_ENTER → NONE → EXECUTE_ENTER → PRE_ENSURE → AGENT_GET_OR_CREATE_ENTER → OK`; `OK` appeared during failure cleanup.

这是一次受观察器扰动的实测顺序，不是旧二进制根因证明，也不是当前 wire 合同已被违反的自动证明。它确定了原验收场景确实失败，以及必须明确的就绪边界。原始 QA 不改成先 `session.create`、sleep 或重试来冒充修复。

E2 stage exits: copy 0; full offline metadata 0; Core build 0; original runtime 1. Native driver 952.091741292 s, under 4200 s; zero retries and no timeout. Its later completion notification is not evidence that the command ran for the notification-delay duration.

### 2.3 Canonical store construction / 权威 store 线索

The reconnaissance stopped before resolving the registry alias. Main subsequently checked the narrow construction links, not a second whole-source review:

- `src/bin/aleph-server/commands/start/mod.rs:510` selects the final `Arc<dyn SessionStore>` for the configured backend; `:1529` passes its clone into `register_agent_handlers`.
- That function takes the store at `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:122`; real configured agents receive its clones at `:1032` and `:1052`; the handler captures its clone at `:1126` and passes it at `:1151`.
- `src/gateway/agent_instance.rs:674` stores the lazy registration's supplied Arc; `:725` moves it into `AgentInstance::new`; `:263` keeps it in the instance.

This statically closes the canonical startup handler-to-registered-agent handle chain. It does not prove every independently constructed/replaced agent shares that handle, nor replace a fresh startup/store-effect assertion for the UI consumer. Recheck the same chain on the actual new-session baseline, especially live agent registration and backend selection.

## 3. Three different readiness facts / 三种就绪事实

1. **Metadata committed:** the canonical store has the addressed row with correct owner/scope.
2. **Attachment live:** the correct connection/principal/surface/mount/provider incarnation is installed.
3. **Render acknowledged:** the real renderer acknowledged the correct pending operation.

前者不授权后者。SessionMetadata owner/scope 不是 OwnershipTree generation，也不是 Provider incarnation。成功 RPC 不等于 actual model/tool effect、durable run trace 或 source-owned operation settlement。

`src/gateway/capability_ui.rs:83` requires authenticated connection, trusted principal, a parsed key, metadata existence, live visibility, and real ProjectionHost. `Ok(None)` at `:113–118` must remain denied. Do not create an owner for an unknown session in attach or change None to allow.

`src/capability/zahir_facade.rs:735–805` and `:958–1045` validate mount/connection/principal/lease/current capture/generation/Policy. `src/capability/ui_surface.rs:111–178` owns pending display work; `:276–310` closes and joins it. Caller cancellation is not physical settlement, and response loss must not prematurely release operation accounting.

## 4. Proposed A implementation / 方案 A 的最小实现

### 4.1 One fallible owning preparation / 单一可失败 preparation

Change the existing owning operation, not a second ready authority:

```rust
pub async fn ensure_session(
    &self,
    key: &SessionKey,
) -> Result<SessionMetadata, SessionStoreError>
```

It returns the actual `get_or_create` result. Preserve backend stamping/no-restamping semantics. Add a narrow library-visible reuse seam in the existing execution-engine module:

```rust
pub async fn prepare_session_for_run(
    agent: &AgentInstance,
    request: &RunRequest,
) -> Result<SessionMetadata, SessionStoreError>
```

This is a **proposed new API**, not a current symbol. It uses the existing `request_scope(request)` derivation and scoped ensure operation. Export through `src/gateway/execution_engine/mod.rs`; do not expose a new metadata-ready registry, bool, latch, epoch or session manager. Existing internal helper may retain its name while forwarding the Result. Do not reconstruct attribution from Gateway `current_scope()`; the spawned/request-attribution context already has a canonical derivation.

### 4.2 Position in both real handlers / 两个真实 handler 的位置

After the applicable routing, existing visibility, agent authorization and response-key/slash selection, await the shared preparation for the **response addressed key**, then check the actual returned incumbent metadata against the trusted caller using the existing visibility policy. The queued branch does this **before** `spawn_queued_run`; every inline/slash success branch carrying that key does it **before its own successful return**, even though there is no spawn. Inventory all such early returns in both real handlers rather than treating one pre-spawn insertion as complete coverage. Reuse the same canonical request-attribution construction/derivation for a no-run branch; constructing its trusted preparation context does not start or report a run. This post-create check closes the first-speaker race; it is not another input-schema validator.

- A first create uses trusted request attribution only. An existing owner is never restamped.
- A concurrent foreign incumbent must be refused with the existing non-disclosing not-found shape.
- Storage failure returns a fixed non-secret unavailable/internal response; no successful response, queue spawn or UI publication. Do not expose raw backend errors or introduce a client retry hint accidentally.
- Preserve `chat.send`/`agent.run` parity. A no-run inline success carrying the addressed key must return only after metadata commit and same-caller visibility, with zero queue/provider effect and no fabricated RunAccepted. Do not make an inline command execute a model or allocate an actual queued run merely to satisfy readiness. No new success fields such as `ready`, no changed run ticket semantics.
- `/btw`: response addressed key and side execution key differ. Prepare the former before RPC success; retain engine preparation for the actual side execution key. Do not replace one with the other.
- Do not skip engine preparation via `session_prepared: bool`; non-RPC producers still need the owning operation, and queued work must reobserve actual state.

### 4.3 Error propagation and cleanup / 错误传播与结算

Compiler-driven update of **all** callers is required, including `execute.rs`, `simple.rs`, `btw_promote.rs`, OpenAI completions and tests/source pins. Use existing `ExecutionError::Failed` with a fixed safe message and the existing receipt presentation; do not add a new enum/state framework solely for this failure. Inspect its real call position and ensure claimed run/session slots, attribution guards, emitter completion and in-flight state settle on an early preparation error. For other surfaces retain their existing typed error envelope, not a copied raw string. No `let _ =` or warn-and-success.

The plan's engine/provider fixture is not compiled in this authoring session. A discoverable, compiling real-handler fixture is an explicit first implementation gate. No fake shared-trait API or internal `pub(super)` test helper is promised available to an integration crate.

## 5. Option B / 备选 B

Retain lazy publication and have the consumer wait for server-owned readiness before attach. `RunAccepted` cannot simply be reused until ensure errors propagate and its row/publication semantics are established. Specify rejected/timed-out/purged/cancelled outcomes, inline commands with no accepted event, subscription-before-send and disconnect termination, and `/btw`'s two keys. Do not listen globally, poll or weaken attach visibility. B changes the client/probe contract and needs its own approved plan.

## 6. Verification contract / 验证合同

- Use barrier/channel synchronization with a delegating real temporary backend. Hold `get_or_create` before commit; drive the real handler; before release observe entered preparation and zero queued/provider effects, with no success response. Release, await response, attach with the same authenticated caller, inspect actual persisted attribution and live source.
- Cover store errors, foreign/pre-existing owner, same-key concurrent principals, revocation/connection changes, before/during preparation cancellation, committed-row caller loss, queue denial/timeout/purge, `/btw`, renderer late ACK, detach/replacement and operation lifetime. For each real no-run inline success carrying an addressed key, require committed metadata with exact attribution, successful same-caller attach under the unchanged live gates, zero queue/provider effect and no RunAccepted; an inline storage/visibility failure must not return success. A row's persistence is distinct from model/tool effect absence.
- `Cargo.toml:397–406` makes `aleph-server` `test=false`. Tests buried in the binary's `#[cfg(test)]` module are not proof. Proposed `tests/session_ui_readiness.rs` must be selected by a real integration target and verified with `--list` before any pass count. The actual `server_init.rs` uses library imports and is a candidate for direct module inclusion; fixture compilation remains to be demonstrated.
- Existing engine tests construct `ExecutionEngine<SingleProviderRegistry, EmptyToolRegistry>` via `ExecutionEngine::new`, `SingleProviderRegistry::new(create_mock_provider())`; that `EmptyToolRegistry` and constructor helper are `pub(super)` test-only, **not** public integration fixtures. Reimplement the tiny test-only registry against the actual trait, with a deterministic local provider and no external LLM.
- Fresh binary, current Panel production/compressed bytes, real Gateway/Panel/PTY/Memory/audience/shutdown probes and independent changed-tree review remain mandatory after focused GREEN. Mock Panel DOM tests are not real Gateway publication proof.

## 7. Actual consumers and later scope / 实际消费者与后续边界

`qa/capability_i3/probe.py:417–421` is the immediate same-caller sequence. Panel `interfaces/webchat/src/panel/capability_surface.rs:133–191` manually attaches a session key and fences late replies/unmount/disconnect. Its fixture `interfaces/webchat/tests/panel-capability-surface.test.mjs:73–79` fabricates an attachment for DOM testing.

NSH `desktop/nsh-shell/src/session.rs:70` generates local IDs and `:101–169` simulates replies; `desktop/nsh-shell/src/connection.rs:139–168` has real hello/subscribe/list but not a finished real chat/event projection. A Core readiness fix does not integrate that UI. NSH 0.5.0 is the user's preferred new compatibility target, not this checkpoint's verified native dependency/runtime version. Keep its migration as a separately frozen work item.

## 8. Historical gates and authoring limits / 历史 gate 与未做项

Phase7 and I3 remain **NOT_ACCEPTED**. Full-core prior run had 50 failures; strict lint, actual >=80% coverage, authorized sever-wire behavior mutations, current served gzip/br bytes, actual graceful 45s shutdown and later Memory/PTY portions are not cleared by this design. Old global verification/tooling incident and ignored test gaps remain disclosed.

Read-only reconnaissance covered 26 Rust/TS/QA files and 8 roadmap documents with approximate manual view/line accounting, not a full audit. Its agent could not write a report under its own profile; the complete evidence arrived as five messages. Main authored this synthesis and separately checked the constructor/test/error interfaces. No readiness code, test target, manifest, dependency or runtime fixture was changed or executed here. Branch-switch/post-commit hooks may launch their existing graph rebuild; no explicit graph/package update was requested.
