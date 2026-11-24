# Session/UI Readiness and Phase7 Continuation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. The execution method is already selected; do not ask Native versus SDD again.

**Goal:** Under explicitly approved Option A, make every successful real `chat.send`/`agent.run` response carrying an addressed-session key publish its metadata—including inline/slash paths with no queued run—then independently revalidate real I3 and finish the remaining Phase7 gates before advancing the canonical roadmaps.

**Architecture:** Return the real SessionStore Result through one request-scoped preparation function, reuse it before both real RPC background spawns, and preserve the engine's execution-key preparation. Keep canonical attribution, post-create incumbent visibility and existing attachment/Policy/renderer fences; no second readiness authority. NSH 0.5 migration and later Capability phases have separate specification gates rather than being smuggled into the readiness repair.

**Tech Stack:** Rust 1.99.1, Tokio/local futures, existing SessionStore backends, Gateway JSON-RPC, existing Core provider/tool traits, Leptos/WASM Panel, Python original QA, Node/Bun Panel tests. No new dependency or global tool upgrade is authorized by this document.

**Spec:** `docs/superpowers/specs/2026-11-12-session-ui-readiness-design.md` (PROPOSED Option A; read before this plan).

## Global Constraints

- **Activation gate:** obtain explicit human acceptance of Option A's metadata-before-every-addressed-success semantics, including legitimate rows for no-run inline replies and before later run rejection, and this plan. Errors, no-addressed-key successes and other RPC handlers are outside the promise. This document and a successful builder check are not that approval. If B is selected, stop this implementation sequence and author its event/readiness contract first.
- Start from actual local `main` containing `fa996e6407c1ba7adbdf871b9b4185a0257dafd8` and these handoff docs. Create an implementation branch before product commits. Resolve paths against the current Git root, not a remembered machine path or session name.
- One agent owns an actual source tree at a time, including readers; Main coordinates. Every source brief includes the mandatory graphify rule verbatim; scoped graph orientation precedes raw reads. No nested agents or self-resume after handback.
- Preserve `.claude/settings.json`, root `config.lock`/`config.toml`, old branches/worktrees/stash/evidence/targets. No `git add -A`, forced ignored files, reset/clean, automatic cleanup, push, release or unrelated merge.
- Preserve authorization, real owner/scope, connection/mount/provider generations, shared Policy, source-owned effect/operation settlement and current-source visibility. Never turn missing metadata into allow, restamp an incumbent, add global ready state or blanket `Send`/`Sync` to make a fixture compile.
- No sleep/retry or `session.create` substitution in the original immediate-attach probe. No paid/external LLM or user data/resources. Test-owned temporary backends, local deterministic provider, isolated loopback identities and renderer only.
- Every Cargo command has an explicit independent `CARGO_TARGET_DIR`. Read and satisfy the repository memory gate before Cargo. Long jobs have bounded owned process groups, native terminal consumption and zero hidden retries. No task/file/log polling; a missing completion event is not evidence of running or success. A genuine deadline recovery is one bounded result consumption, not a loop.
- Trust/permission/checker/threshold/harness/CI are not changed to make verification pass. `/verify` consent failure remains a block, not approval to trust commands. Never use dangerously-skip-permissions or hardcode secrets. Validate unknown data at its owning system boundary, not again across trusted internal types.
- Comments English; explanatory docs Chinese/English. Report exact reached/failed/ignored/timed-out/not-run evidence. A build or focused test is not whole Phase7/NSH acceptance.

## Review Focus

1. A concurrent foreign first speaker wins the same key: actual returned incumbent must deny the loser without restamping or model/queue effect (R2).
2. A row commits but the response future disappears: no false rollback, unauthorized takeover or premature source-owned effect release (R2).
3. A queue timeout/rejection or spend denial follows preparation: a legitimate row may remain, but no model/tool effect is inferred (R2).
4. `/btw` or an inline command has different execution/admission behavior: the response addressed key is ready without lying about side-key readiness or RunAccepted (R2).
5. Source/mount/connection changes between response and render: attach/lease/ACK must still use live authority, not a remembered metadata-ready bit (R3).

## File and interface map

| Owner | Files | Responsibility |
|---|---|---|
| R1 | `tests/session_ui_readiness.rs`; `tests/session_ui_readiness/support.rs`; `Cargo.toml` only if target discovery actually requires it | Compiling, discoverable real-handler fixture; no product scaffolding-only commit |
| R2 | `src/gateway/agent_instance.rs`; `src/gateway/execution_engine/run_loop/mod.rs`; `src/gateway/execution_engine/mod.rs`; `src/bin/aleph-server/server_init.rs`; `src/gateway/execution_engine/execute.rs`; `src/gateway/execution_engine/simple.rs` | Fallible canonical preparation, real-handler ordering, typed engine failure/settlement |
| R2 | `src/gateway/execution_engine/btw_promote.rs`; `src/gateway/openai_api/completions/agent.rs`; existing ensure tests and `src/gateway/execution_engine/session_run_registry.rs` source pin | All Result callers and side-key/source-order expectations; not a new routing design |
| R3 | same integration target; existing `qa/capability_i3/probe.py`; `interfaces/webchat/tests/panel-capability-surface.test.mjs` | Adversarial boundary evidence and unchanged actual QA. Do not edit these consumers merely to hide publication failure |
| A/B/G | new test-owned output and ignored coordination workspace; acceptance notes under `docs/superpowers/runs/` | Fresh full-range review, real integration, independently accounted global gates |

Existing signatures:

```rust
pub async fn handle_chat_send_with_engine<P, R>(
    request: JsonRpcRequest,
    engine: Arc<ExecutionEngine<P, R>>,
    event_bus: Arc<GatewayEventBus>,
    router: Arc<AgentRouter>,
    agent_registry: Arc<AgentRegistry>,
    app_config: Arc<tokio::sync::RwLock<alephcore::Config>>,
    workspace_manager: Option<Arc<AgentEnvStore>>,
    provider_registry: Arc<P>,
    session_manager: Arc<dyn SessionStore>,
    telecom_lifecycle: Option<Arc<TelecomLifecycleService>>,
) -> JsonRpcResponse
where P: ProviderRegistry + 'static, R: ToolRegistry + 'static;
```

Use the actual fully qualified imports from `server_init.rs`; parameter names above omit current underscore prefixes only for readability. `handle_run_with_engine` is the sibling real handler, not an alias invented here. Existing lightweight engine construction is `ExecutionEngine::new(config, Arc::new(SingleProviderRegistry::new(create_mock_provider())), Arc::new(test_registry), Vec::new())`. The existing `EmptyToolRegistry`/test factory is `pub(super)`, so an integration crate must define its own tiny trait implementation, not import that private helper.

Proposed production signatures, owned by R2:

```rust
AgentInstance::ensure_session(&self, key: &SessionKey)
    -> Result<SessionMetadata, SessionStoreError> // async

pub async fn prepare_session_for_run(
    agent: &AgentInstance,
    request: &RunRequest,
) -> Result<SessionMetadata, SessionStoreError>;
```

The second function is defined beside the existing scoped helper and re-exported from `gateway::execution_engine`. If the internal helper retains its old name, its Result and one attribution derivation must still be shared. Do not introduce a prepared bool, alternative session store or new execution-error enum; existing `ExecutionError::Failed` plus safe fixed presentation is sufficient.

## R0 — Baseline and one contract decision / 基线与唯一合同选择

- [ ] Read the spec and restart prompt, actual Git root/worktrees/branch/status and constitution. Prove the checkpoint ancestor and preserve unrelated local state. Recheck scoped source anchors after graphify; old line numbers are locators, not API truth.
- [ ] Ask only whether A's defined addressed-success scope is accepted: committed metadata also for no-run inline replies and before a later run denial. Record the answer and current HEAD in a new SDD ledger. If not accepted, do not dispatch R1–R3; design B separately. No repeated general interview or execution-method menu.
- [ ] Declare one new test-owned artifacts root, target, ALEPH_HOME, provider/renderer/ports and cleanup ownership. Obtain actual command/resource consent if required; do not manufacture trust. Each implementer gets its exact files, signatures, expected evidence and handback condition.

Produces: approved contract/baseline/verification-resource scope or an honest blocked gate. No product change.

## R1 — A discoverable real-handler fixture / 可执行真实入口 fixture

**Consumes:** current public real handler, actual provider/tool/store traits, canonical startup alias chain in spec §2.3.

**Produces:** executable `session_ui_readiness` integration target plus test-only `ReadinessRig`, `PreparationBarrierStore` and local deterministic provider. Proposed fixture APIs are owned here, not existing library symbols:

```rust
impl ReadinessRig {
    async fn new() -> anyhow::Result<Self>;
    async fn chat_send(&self, user: &str, key: &str) -> JsonRpcResponse;
    async fn agent_run(&self, user: &str, key: &str) -> JsonRpcResponse;
    async fn metadata(&self, key: &str) -> Option<SessionMetadata>;
    async fn wait_prepare_entered(&self);
    async fn wait_prepare_entries(&self, count: usize);
    async fn wait_metadata_committed(&self);
    fn release_prepare(&self);
    fn release_committed_prepare(&self);
    fn queue_or_provider_effects(&self) -> usize;
    async fn close(self) -> anyhow::Result<()>;
}
```

- [ ] **RED:** add a selected-target smoke test `real_handler_fixture_preserves_existing_session_owner`. Use a real temporary backend, pre-existing key `agent:main:readiness-a`, trusted caller `readiness-user-a`, correct request attribution, real AgentRegistry/AgentInstance, local provider and tiny ToolRegistry. Open both persistent fixture gates with `release_prepare()` and `release_committed_prepare()` before calling the handler; this smoke must not wait on a barrier intended for R2. Assert response success, exact persisted owner unchanged, no external provider request and clean fixture closure. The test must directly call the imported real `server_init` handler, not a copy.
- [ ] Import the real binary module via `#[path = "../src/bin/aleph-server/server_init.rs"] mod server_init;`. Inspect actual target discovery: `aleph-server` is `test=false`, so no binary-only cfg(test) claim. Do not add new production public APIs just to make a test fixture compile. If actual private auth/startup seams prevent a genuine fixture, stop and report the concrete compiler/API issue before changing this plan.
- [ ] Run the new target with `--list`; before adding it the old tree must genuinely fail to locate the target, not a hand-waved RED. After scaffolding, ensure the named smoke test appears and compile with `--no-run`. A target-discovery/compiler RED is fixture evidence only, not the publication behavioral RED required by R2.
- [ ] Implement the barrier store as a delegating adapter over the **same real temporary backend**, gate only its target `get_or_create`, and forward all other actual trait methods. It records real entry, actual returned owner and effect counters; no alternate memory truth. Its two initially closed gates are before delegation and after a successful actual backend commit/before returning the result. `release_*` opens each gate persistently for all later calls, including the engine's second ensure; entry/commit notifications must retain their observed condition, so no notify-before-listener loss. `wait_prepare_entries(2)` is the two-principal pre-commit coordination seam. An actual backend error does not emit a successful-commit event. Use channels/watch or condition-backed Notify, not delays. `close` opens both gates before cancelling/draining owned work. Each test must capture its body's result/panic and await this finalizer before propagating it; a Drop guard may release/cancel but is not an async join receipt. A current-thread LocalSet/pinned future is allowed; no blanket Send/Sync changes.
- [ ] Run the smoke test; assert clean owned-task cancellation/drain on fixture exit. Preserve actual output and selected test name/count. Commit fixture plus its meaningful smoke test after the required verification/review gate, not an empty scaffold commit.

Commands (repeat the explicit target setting on **every** Cargo invocation):

```bash
CARGO_TARGET_DIR="$QA_ARTIFACTS/target" cargo test -p alephcore --test session_ui_readiness -- --list
CARGO_TARGET_DIR="$QA_ARTIFACTS/target" cargo test -p alephcore --test session_ui_readiness --no-run
CARGO_TARGET_DIR="$QA_ARTIFACTS/target" cargo test -p alephcore --test session_ui_readiness real_handler_fixture_preserves_existing_session_owner -- --exact --nocapture
```

Expected: discovered exact test and actual nonzero executed count, exit 0 after fixture implementation. The named target/helper code is proposed and has **not** been compiled in this authoring session. R1 is a genuine feasibility gate, not an assertion that it already works.

## R2 — Publish before successful real responses / 成功响应前发布

**Consumes:** R1 fixture and test APIs; R0-approved A. **Produces:** both proposed fallible production signatures, real RPC preparation ordering, safe errors and settled failure paths. All future consumers use these exact names/types.

- [ ] **Behavioral RED:** add `chat_response_waits_for_addressed_metadata_commit` and sibling `agent_run_response_waits_for_addressed_metadata_commit`. Pin the actual RPC future and select real preparation entry against premature response. Before releasing the pre-commit barrier, success must not be observed and queue/provider effects must be zero. The old product must fail by returning an actual successful RPC too early—not auth denial, missing fixture, compiler error or timeout.

```rust
// Pseudocode using the R1-owned rig; keep a real handler future, not a mocked RPC.
let rpc = rig.chat_send("readiness-user-a", "agent:main:readiness-a");
tokio::pin!(rpc);
tokio::select! {
    biased;
    response = &mut rpc => panic!("RPC settled before preparation: {response:?}"),
    _ = rig.wait_prepare_entered() => {}
}
assert!(futures::poll!(&mut rpc).is_pending());
assert_eq!(rig.queue_or_provider_effects(), 0);
rig.release_prepare();
// Keep driving the real RPC: awaiting only the observer would strand a pinned future.
tokio::select! {
    biased;
    response = &mut rpc => panic!("RPC settled before committed gate: {response:?}"),
    _ = rig.wait_metadata_committed() => {}
}
let metadata = rig.metadata("agent:main:readiness-a").await.unwrap();
assert_eq!(metadata.owner_user_id.as_deref(), Some("readiness-user-a"));
// Also compare scope_id with the actual trusted scope used by the R1 fixture.
rig.release_committed_prepare();
let response = rpc.await;
assert!(response.error.is_none());
```

- [ ] Return the actual store Result from `AgentInstance::ensure_session`; expose `prepare_session_for_run` through the existing request-scoped derivation. Do not call warn-and-unit as a readiness guarantee or compute attribution from caller task-local current_scope.
- [ ] Inventory all success returns in both real handlers and mark which carry an addressed-session key, including no-run inline/slash branches. After applicable routing/auth/key selection, reuse the canonical request-attribution construction and await that addressed key's preparation: before background spawn on queued paths and before the actual return on inline paths. Check the returned incumbent with existing trusted-caller visibility policy. Keep the earlier existence guard; the returned-row check closes a different TOCTOU fact. Storage/visibility failure returns a fixed safe error/no spawn/no success. Do not fabricate a queued run or RunAccepted for an inline reply. Preserve the existing wire shape; no-addressed-key successes and other handlers are outside A.
- [ ] Propagate typed failure in real/simple engines and all callers (`btw_promote`, OpenAI completion, existing tests). Recheck run-claim/session guards/emitter finalization on every new early error; use existing safe error presentation, never raw store errors. Update the existing source-order pin to assert the **checked** operation, not delete it or keep an ignored Result. Engine actual execution-key preparation remains, including non-RPC producers and queued work.
- [ ] Add and run exact named tests with these assertions:
  - `store_failure_does_not_spawn_or_reply_success`: get/create error; fixed non-secret error, zero queue/provider/UI effects, claimed resources settle.
  - `concurrent_first_speaker_preserves_incumbent`: pin and drive both real caller futures until `wait_prepare_entries(2)` proves both passed the initial unknown-key visibility check, then open the pre-commit gate. Observe the actual atomic-create winner, release the committed-result gate and require exactly that owner to succeed; the loser cannot restamp/read/attach/start a model under the winner's key. Compare stored owner/scope, not a guessed winning task schedule.
  - `preparation_cancel_and_committed_row_response_loss_are_distinct`: one case cancels while the pre-commit gate is closed and actual delegation has not begun; another drives the RPC to `wait_metadata_committed`, reads the real row while the committed-result gate is still closed, then drops the caller and opens the gate for owned cleanup. Do not claim rollback, grant another principal or infer physical I/O cancellation from future drop; explicitly settle the backend operation.
  - `later_queue_or_spend_rejection_keeps_legitimate_row_without_model_effect`: separately cover reject/timeout/purge/spend outcomes, real denied terminal results and zero actual model/tool effect. Do not infer effect absence from a vanished response.
  - `btw_prepares_response_key_without_replacing_execution_key`: main key ready at response; side key follows engine's own real path, with preserved attribution and admission.
  - `inline_response_publishes_metadata_without_fabricating_run_accepted`: exercise actual no-run inline returns carrying the addressed key, not a fixture response. Hold/release preparation and require commit before success, exact owner/scope, zero queue/provider effect and no RunAccepted. Store/visibility failure must not return inline success. Add the sibling real handler case wherever its actual success-return inventory contains such a branch; do not invent a branch that does not exist. The corresponding real same-caller attachment is an explicit R3 case, not an unimplemented R1 rig method.
- [ ] Run fresh discovery and exact integration target, plus actual existing related library filters. Require nonzero counts; a single test with a vector table is one test, not many. Preserve RED/GREEN receipts and current source identity. Independent fresh SPEC and QUALITY review with exclusive tree ownership, then verification and a focused commit. No release assertion.

```bash
CARGO_TARGET_DIR="$QA_ARTIFACTS/target" cargo test -p alephcore --test session_ui_readiness
CARGO_TARGET_DIR="$QA_ARTIFACTS/target" cargo test -p alephcore --lib gateway::busy_queue
CARGO_TARGET_DIR="$QA_ARTIFACTS/target" cargo test -p alephcore --lib gateway::execution_engine
CARGO_TARGET_DIR="$QA_ARTIFACTS/target" cargo test -p alephcore --lib gateway::capability_ui
CARGO_TARGET_DIR="$QA_ARTIFACTS/target" cargo test -p alephcore --lib capability::ui_surface
CARGO_TARGET_DIR="$QA_ARTIFACTS/target" cargo test -p aleph-protocol
```

## R3 — Real attachment and lifecycle revalidation / 真实附着与生命周期

**Consumes:** R2 checked publication; unchanged real attach/lease/Policy/renderer and original QA. **Produces:** actual new-build receipts, not a second response contract.

- [ ] Extend real boundary cases for foreign principal/connection, stale mount/provider replacement, revoked owner, detach/disconnect, late renderer ACK and caller-response loss. Include `real_inline_addressed_success_is_attachable_without_run_accepted`: for each applicable actual no-run success identified in R2, drive the real Gateway handler and immediate same-caller attach under unchanged live gates, inspect committed owner/scope, and prove zero queue/provider effect and no RunAccepted. Extend the R3-owned real-runtime fixture in the same integration target as needed; no fake R1 attach method or mock Panel receipt counts. An inventory with no applicable branch is a source-backed non-applicability finding, never an ignored/zero-match PASS. Assert effect/accounting at the actual owner and settlement boundary; metadata-ready never bypasses current permission/generation.
- [ ] Build a fresh binary and Panel production outputs in a new declared environment, record exact source/build/accepted SHA and PID/PG. Use the original QA immediate sequence with actual Gateway authenticated caller and local deterministic provider/renderer, proving Real mode—not Simulated fallback. Do not backport E2's temporary diagnostics indiscriminately.
- [ ] Run current Panel DOM fixture separately; its mock attachment is not real Gateway proof. For actual served Panel record gzip/br bytes, decoded hash versus this build's source artifact and real render ACK. Unknown or unavailable browser/native fixture is BLOCKED, not a mocked production PASS.

```bash
bun interfaces/webchat/scripts/build-panel-tabs.mjs --output="$PANEL_OUTPUT_ROOT"
PANEL_OUTPUT_ROOT="$PANEL_OUTPUT_ROOT" node --test interfaces/webchat/tests/panel-capability-surface.test.mjs
python3 -I qa/capability_i3/probe.py --binary "$SERVER_BIN" --sha256 "$SERVER_SHA256" --panel-output "$PANEL_OUTPUT_ROOT" --artifacts "$QA_ARTIFACTS"
```

- [ ] Demand actual completion through display/ACK, Memory, real disposable PTY, audience/isolation and normal 45 s shutdown. A failing earlier step marks later steps NOT_REACHED. Failure cleanup SIGKILL/-9 is not graceful PASS; timer join/drop is not an actual physical-effect completion fence. Retain EOF/truncation/redaction/resource limitations and cleanup receipts.
- [ ] Independently review the final repair range and save the scoped result. Any behavior-severing mutation needs an explicit fresh scope/authorization; compile failure is not a successful behavioral RED. Do not install/upgrade tools or change scripts/permissions to manufacture a gate.

## I3-A — Fresh whole changed-tree review / 全集成复审

- [ ] Recompute actual main-baseline-to-current product changes and all I3 integration paths, not only R2. Read approved `2026-11-07-phase7-i3-production-integration` spec/plan and original `2026-11-05-phase7-advanced-capabilities` plan. Keep prior review history; no recycled verdict or source hash from the old checkpoint.
- [ ] One fresh independent reader at a time checks all ingresses, common Policy/live facts, canonical Tool/Memory/special capture and schema, source/owner/generation, cancellation/physical settlement, real UI/PTY and event/audit/ACL/close/drain/join. Resolve findings with owned tests and focused re-review; do not silently broaden a previous fixed-wave allowance.
- [ ] Produce explicit current SPEC/QUALITY results and unresolved findings. This is a full-range source gate, not the runtime or whole-phase acceptance.

## I3-B and P7-G — Production acceptance and independent global gates

- [ ] Run actual current isolated startup/Gateway/Panel/PTY/Memory/event/audit/browser/served-byte probes against the reviewed bytes; rerun after source changes. D-T=A means real PTY input/attach/resize/close, **not** strict separate stdin/stdout/stderr streaming APIs.
- [ ] Reconcile each retained full-core failure ID, ignored test and incomplete run. Old StageB had 50 full-core failures; do not assume all inherited or all fixed. Establish actual controlled baseline runs where needed; source equality is not a baseline runtime proof. Fix new regressions; unrelated failures need an explicit disposition, not silent green.
- [ ] Verify strict lint separately from default lint. Measure actual coverage against >=80% only with already-authorized compatible tools; do not install on an assumed approval or quote focused counts as coverage. Preserve raw mapping/aggregation limits.
- [ ] For authorized sever-wire controls prove actual policy/renderer/emitter behavior RED, restore exact bytes and GREEN. Keep global verification/tooling incident limitations, trust consent blocks, current served gzip/br evidence and real shutdown/owned-resource state in the acceptance report.
- [ ] Reconcile all four original Phase7 deliverables: activation/constraints; special capabilities; event/audit; performance/optimization. Re-evaluate source-coupled cache/reservations/metrics and a current, correctly labeled benchmark; prior dev warm/cold numbers are not release desktop or controlled old-binary A/B evidence.
- [ ] Report separately: repair implemented/verified, I3 production verified, whole Phase7 accepted or not, D-T semantics, NSH native support and each remaining gate. No automatic merge/push/ship/Phase8-ready on focused GREEN. User decisions/permissions are gates; ordinary scoped task review continuation does not need repeated “continue?” questions.

## Subsequent phases / 后续阶段（不是虚构实施细案）

This table preserves the actual canonical masters and next decision gates. It is not authorization to implement every subsystem in one task or reuse another phase's approval.

| Track / 轨道 | Actual source / 名称 | Next deliverable and gate / 真实后续 |
|---|---|---|
| Capability Phase7 | `docs/superpowers/plans/2026-11-05-phase7-advanced-capabilities.md`; seven-task I3 plan `docs/superpowers/plans/2026-11-07-phase7-i3-production-integration.md` | Finish R/I3/global gates above; evidence per original four tasks. Current NOT_ACCEPTED. |
| Capability Phase8 | Original Phase7 master names **Integration & Security** | Detailed Phase8 plan was not found. Only after Phase7 disposition, author a real security/integration spec and concrete plan from remaining threat/owner/ingress evidence; independent review and human approval. Do not invent Phase9 name/schedule or claim implementation-ready. |
| NSH Phase0 | `docs/superpowers/specs/2026-11-05-nsh-desktop-refactor-design.md:212` spike/API/cost | Audit existing Phase0 work, don't restart it blindly. Freeze new 0.5.0 compatibility scope and prove CLI/crates/runtime/host/types plus platform minima. Existing E2 excluded NSH compilation. |
| NSH Phase1 | Same master `:228`, shared protocol/client + hello/events/session list/transport/supervisor | Reconcile actual integrated branch commits and wire event consumption; prevent a real transport plus discarded events from being called end-to-end chat. |
| NSH Phase2 | Same master `:243`, tray/hotkey/deeplink/updater/notification/window state/daemon/single instance | Port actual 0.5 WindowRef/window kinds/lifecycle. No Tauri fallback; Swift screenshot/voice/camera explicitly outside this layer. Real host shutdown is not a Core task-drop proof. |
| NSH Phase3 | Same master `:270`, core chat and session UI | Replace generated local session IDs/fake replies with canonical keys/RPC/events/history, state-first tests and real isolation. Readiness semantics must match the approved Core contract. |
| NSH Phase4 | Same master `:298`, full Canvas/Settings/Browser/Panel/PhoneTablet parity | Inventory actual current parity per surface, two-desktop QA; i18n separate. Old `2026-11-05-nsh-desktop-phase4-capability-alignment.md` Tool-only/old-rev claims are history, not current I3 truth. |
| NSH Phase5 | Same master `:318`, independent PluginRuntime timeline | Restricted QuickJS/TSX/register_component with owner/scope/quota/cleanup/UI bridge. Verify current ADR006 WASM and ADR008 signing Accepted prerequisites before a new spec; their current status is not established here. |
| NSH Phase6 | Same master `:332`, Tauri removal | Only after Phase4 complete and two-week dual-run. Then remove actual members/implementation and update docs. No early deletion or GPUI reopening. |

NSH preferred target: official v0.5.0, observed release commit `f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1`. Official release and master evidence are distinct: https://github.com/longbridge/nsh/releases/tag/v0.5.0 ; https://nsh.dev/docs/compatibility/ ; https://nsh.dev/docs/native-components/ . The public releases index body lagged its 0.5 navbar; cause unknown. Do not quote old 0.4 restrictions without rechecking the selected 0.5 API; do not assume new upstream features are integrated in Aleph. No global `nsh update` or dependency install is authorized here.

## Author self-review and handoff / 本人自检与交接

- [x] Spec coverage: contract/owner/error/addressed-key are R0/R2; compiling fixture R1; real attachment/lifetime/shutdown R3; whole-review/production/global gates I3-A/B/P7-G; distinct subsequent tracks explicitly gated.
- [x] Step scan: each feature task has observable tests, named owner files/interfaces, actual command and independent review. Proposed fixture compile is a first gate, not a fake existing helper. Roadmap unknowns are explicit planning gates, not placeholder code APIs.
- [x] Type consistency: one Result<SessionMetadata, SessionStoreError>, one prepare_session_for_run name, current real handler generic bounds, existing Failed error; all new rig methods belong to R1. No internal pub(super) helper imported externally.
- [x] Five Review Focus rows map to concrete R2/R3 cases. Pinned-future/barrier approach avoids response polling, sleeps and imposed Send/Sync. Exact counts/names must come from new discovery, not this authoring checklist.
- [x] Proportion: method bodies are left to implementers except the small causal RED sketch. Later subsystems get new specs rather than speculative bulk implementation. This authored plan's self-review is not delegated and not test execution.

**Authoring status:** no product implementation, new integration target, build, test, fixture execution, mutation, native NSH migration or release acceptance was performed while writing this plan. Source checkpoint is in main; final handoff docs must also be in main before restart. Review this conditional plan and accept A explicitly in the new session before execution.
