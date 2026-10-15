# Panel Browser Workspace Implementation Plan

> **Execution method:** REQUIRED SUB-SKILL: use `superpowers:subagent-driven-development`. Delegate concrete tasks to fresh implementation subagents, dispatch Lane 2 independently once Task 1 is frozen, and use a fresh reviewer after each task before integration. Stop at every gate; never weaken a failed safety or identity check.

**Goal / 目标：** Gateway 执行并唯一拥有 browser；Panel Web/Desktop 在当前任务右栏展示 live viewer，TUI 只在本机默认浏览器或显式 Chrome 打开同一 Gateway target 的独立精简 viewer。

**Architecture / 架构：** 一 profile 一条 CDP connection，由一个 lifecycle owner 管理多个 target 各自的 screencast、共享 recording observer、统一 Fetch policy worker 与 control lease。结构化 `PreviewResource` 是 UI 唯一输入；Panel 负责复杂 Leptos UI，interface/Bridge 仅薄 I/O，不复制 Core 业务。独立 viewer 与 Panel 复用组件，但使用专属受限凭据和 frame/input 通道。

**Tech Stack：** Rust、tokio、serde/schemars、Axum/WebSocket、Leptos/WASM、CDP Fetch/Page、现有 browser backend 与 process registry。

**Spec：** `docs/superpowers/specs/2026-10-07-panel-browser-workspace-design.md`；控制、screencast 与 bounded fan-out 补充遵循 `docs/superpowers/specs/2026-09-05-browser-live-view-design.md`。

**English summary:** Add a Gateway-owned, task-bound browser projection shared by Panel and a capability-limited standalone viewer; no local execution bridge, URL inference, or second CDP owner.

## Global Constraints

- 只在 feature worktree 实施；不得修改或合入 `main`。Lane 2 的结果先合入整合 feature 分支，后续任务才能依赖。
- 两条执行线文件不得重叠。跨 crate wire protocol 先冻结；冲突时两线都停止。每 2–3 个独立任务做一次整合编译，task 级 RED/GREEN 可定点运行。
- Panel 承担复杂 UI（R2）；Gateway/Core 持有业务与安全状态；TUI、CLI、desktop Bridge 只做 I/O。不得在 `src/harness/` 新增智能路由。
- 禁止从 chat URL regex、prompt、profile 当前 active tab 或 `system open` 推断 target。绑定只能由后端当前 turn/run 与实际 BrowserSession/Target 生成。
- browser CDP connection、每 target screencast、recording observer、Fetch policy 与 frame ACK 必须同一 lifecycle owner；不得第二次 connect、第二个 Fetch consumer 或第二处 ACK。
- 静态 artifact 不得创建 browser；`artifact_publish` 只可作为显式 metadata adapter，不能被宣称为稳定 preview contract，也不得破坏既有消费者。
- precise network grant 默认关闭。Task 0 未获单独批准、未验证或覆盖不完整时，停止 Task 4 的 grant 产品代码；不得 fallback 到 obscura v0.2.2 或 `--allow-private-network`。
- 每次执行任意 `cargo` 前单次检查可用物理内存：Linux=`MemAvailable_kB*1024`；Windows=`Win32_OperatingSystem.FreePhysicalMemory*1024`；macOS=`(Pages free+Pages inactive+Pages speculative)*hw.pagesize`（`vm_stat` 页数去掉尾点）。若 `<4294967296` bytes，不启动 cargo，等待资源或用户决定；下一次尝试重新检查，禁止无止境轮询或偷偷 build。
- 每个 RED/GREEN 命令必须显示 `running 1 test`（或脚本的唯一 case）；`0 tests`、filter 不匹配、跳过均不是证据。提交频繁但不得绕 hooks。
- 实施前先排查已知 hook `include_dir` 的 `STATUS_DLL_INIT_FAILED`。未排除时不得进入 cargo 验证，不得宣称项目绿色。

## Review Focus

1. **Contract / 契约（Tasks 1, 4, 6）：** 绑定字段、epoch/seq/expiry、producer 与 viewer capability 是否端到端同源。
2. **Correctness / 正确性（Tasks 2, 3, 5）：** 一 connection、多 target、一次 ACK、控制互斥与重连恢复是否成立。
3. **Security / 安全（Tasks 0, 4, 7）：** redirect/subresource/iframe/fetch/HMR WS、凭据泄漏、跨任务订阅与通用 RPC 提权是否 fail closed。
4. **Resilience / 韧性（Tasks 2, 5, 6）：** slow viewer、隐藏 viewer、recording、服务退出、engine exit 与 stale input 是否独立且有界。
5. **Migration & QA / 迁移验证（Tasks 8, 9）：** 旧消费者先 census 后清理、文档与真机 selector 是否能证明效果到达。

---

## File responsibility map / 文件责任表

| Lane / Owner | Exact files | Responsibility |
|---|---|---|
| Shared contract（Task 1，串行） | Create `shared/protocol/src/browser_live.rs`; modify `shared/protocol/src/lib.rs` | 唯一 wire types、事件/RPC payload、schema/serde tests |
| Lane 1 lifecycle（Task 2） | Create `src/browser/live/{mod.rs,session.rs,stream_hub.rs}`; modify `src/browser/{manager.rs,error.rs}`、`src/browser/cdp_backend/{routes.rs,recording.rs,mod.rs}` | 单 CDP owner、多 target cast、统一 Fetch/recording/ACK |
| Lane 1 control（Task 3） | Create `src/browser/live/control.rs`; modify `src/browser/manager.rs`、`src/builtin_tools/browser_tools/{open.rs,navigate.rs,click.rs,type_text.rs,tabs.rs,profile_tool.rs,exec.rs,mod.rs}` | 原子 mutation permit、Take/Cede、stale input 与所有入口一致 gate |
| Lane 1 viewer security/service（Task 4） | Create `src/browser/live/grant.rs`、`src/gateway/browser_viewer_route.rs`; modify `src/gateway/server/{mod.rs,connection/auth.rs}`、`src/gateway/{event_scope.rs,method_admin.rs,event_visibility.rs}` | grant、fragment exchange、`/viewer/ws` upgrade preauth、受限 channel、precise policy |
| Lane 1 Leptos UI（Task 5） | Create `interfaces/webchat/src/platform/wide/views/browser/{mod.rs,model.rs,viewer.rs,standalone.rs}`; modify `interfaces/webchat/src/{app.rs,components/mode_sidebar.rs,components/workspace_panel.rs,state/layout.rs}`、`interfaces/webchat/src/platform/wide/views/chat/events.rs` | 共享 viewer、右栏、独立 route、live-only reveal |
| Lane 1 producer（Task 6） | Create `src/builtin_tools/browser_tools/present.rs`; modify `src/builtin_tools/{browser_tools/mod.rs,mod.rs,artifact_publish.rs,process_registry.rs}`、`src/executor/builtin_registry/{definitions.rs,registry/struct_def.rs,registry/tool_registry_impl.rs,builder/constructor/mod.rs}`、browser `open.rs`/`navigate.rs` | managed-tool producer、注册/发现/调用、普通 browser resource 事件、服务生命周期 |
| Lane 2 TUI launcher（Task 7） | Create `interfaces/tui/src/viewer_launcher.rs`; modify existing `interfaces/tui/src/lib.rs`、`interfaces/tui/src/tui/mod.rs`、`interfaces/tui/src/tui/app/mod.rs`、`interfaces/tui/src/tui/app/events.rs`、`interfaces/tui/src/tui/commands.rs` | 由 `lib.rs`/`tui/mod.rs` 接入；仅请求受限 URL 并启动本机 viewer |
| Integration/docs（Tasks 8–9） | Modify `docs/reference/{FEATURE_LOCATOR.md,ARCHITECTURE.md,TOOL_SYSTEM.md,SESSION_KNOBS.md,SECURITY.md,DEVELOPMENT.md}`、`qa/README.md`; create `qa/browser_live/{run.sh,check_memory.py}` | census、迁移、文档、资源 gate、真机验收 |

## Frozen shared interfaces / 冻结共享接口

**Normative source:** [`../../../.superpowers/sdd/2026-10-07-panel-browser-workspace/task-1-contract-proposal.md`](../../../.superpowers/sdd/2026-10-07-panel-browser-workspace/task-1-contract-proposal.md) r4, with freeze acceptance and consumption rules recorded in [`../../../.superpowers/sdd/2026-10-07-panel-browser-workspace/task-1-contract-checkpoint.md`](../../../.superpowers/sdd/2026-10-07-panel-browser-workspace/task-1-contract-checkpoint.md). The proposal is **FROZEN FOR TASK 1** and is the sole source of concrete wire fields, serde/schema shapes, validation invariants, and constants. This plan intentionally keeps no competing field sketch. Timestamps are Unix milliseconds and wire IDs remain `String`.

- **Binding and authority:** consume the exact tagged `IssuerIdentity`, `ResourceBinding`, `ViewerBinding`, and `ArtifactBinding` contracts from r4 §§1/4. Effective authority is the exact server-derived issuer/principal/task/resource/target tuple, with permissions `server_policy(...) ∩ request`; artifact issue/consume/exchange/auth fails closed.
- **Same-target companion:** consume r4's `CompanionCandidate`, `CompanionLayout`, and `CompanionStateHint`. A candidate is revision-scoped state/layout of its containing resource, inherits that resource's already-bound target, and has no `target_id`, `browser_session_id`, or independent `resource_id`; stale/unknown selection creates no target, grant, or WS.
- **Grant and first-frame auth:** consume the exact r4 request/response and secret-wrapper types. Fragment nonce consumption mints a one-time exchange ticket; ticket exchange creates S0. The WS TCP/HTTP upgrade is unauthenticated and grants no authority. Its sole allowed first client message is text `AuthHello`; the validated `AuthHello` CAS is the authentication/rotation linearization point and enforces rotating secrets plus one live winner. `AuthOk` carries `next_secret`; `AuthError` closes pre-auth sockets.
- **Publication and registries:** before `AuthOk` is successfully queued/written, publish no frame/state/control and do not mark the socket live. Queue/write failure closes and removes it immediately. Viewer sockets never enter generic connection/subscription registries.
- **Dedicated transport:** frame/state/control use only the dedicated viewer WS. Frames use r4's exact two-`u32 BE` binary envelope and limits. The generic `BusEvent` has exactly three events: `browser.live.resource`, `browser.live.closed`, and `browser.live.intervention`; frame/state are not bus events or generic RPC.
- **Dispatch barrier:** the per-target serializer performs the r4 §3 dispatch-start recheck over WS/connection/stream/control generations, expiry/seq, permission/controller, binding liveness, and revocation. Whole-grant revoke closes WS and clears rotation; human-only control loss preserves Frames.
- **Evidence boundary:** r4's Origin contract is frozen. Runtime/deployment proof remains gated exactly as listed in r4 §8 and the checkpoint, including supported proxies, logs/WS frame-body redaction, CSPRNG/hash, CSP, and pre-auth DoS limits.

All downstream tasks must consume these shared types directly, not local JSON clones. Pre-r4 sketches—including `CompanionCandidate.target_id`, bus `frame|state`, static `ViewerGrant.nonce`, nonce-only exchange, and pre-upgrade/`on_upgrade` authentication—are prohibited. UI local state remains `BrowserViewerModel`, distinct from the frozen wire `BrowserLiveState`.

### Task 0: Disposable safety Spike gate（需再次获得用户明确批准）

**Files:** throwaway isolated profile and evidence under `.analysis/browser-viewer-safety-spike/`; no product source and no commit.

- [ ] Before any probe, obtain separate explicit approval; otherwise record `Gate=Blocked(approval missing)` and stop.
- [ ] Build self-owned loopback HTTP/WS/isolated-DNS receivers and disposable Chromium profile. Never access real metadata/LAN, daily profile, or install/update a browser.
- [ ] Probe baseline and throwaway unified-Fetch grant prototype. Allowed top-level/redirect/subresource/iframe/fetch/HMR WS count **>0**; denied origin/port/protocol/rebinding count **=0**, before and after mock rule add/remove.
- [ ] Assert with `assert!(allowed_count > 0); assert_eq!(denied_count, 0);`; the isolated DNS maps only self-owned loopback names and sends no real private-network traffic.
- [ ] Do not assume CDP Fetch observes WS. If WS cannot be rejected before connection, return to spec review and block precise grant; do not mark unsafe feature complete.
- [ ] Emit only `Gate=Validated(engine=<name>, cases=<list>)` or `Gate=Blocked(reason=<exact reason>)`. Missing engine is unvalidated, not a forced install; baseline failure blocks this gate pending remedy/re-run, not permanent rejection of all dev preview work.

### Task 1: Freeze shared protocol and schema

**Interfaces:** implements every exact type and invariant in the frozen r4 proposal §§1–7, including issue/consume/exchange/auth/attach/input/control request-response shapes, the binary frame codec, and the three-event `BusEvent`; consumes only serde/schemars/chrono already in `aleph-protocol`. No plan-local field sketch or JSON clone is authoritative.

- [ ] Add test `browser_live::tests::viewer_binding_round_trip_preserves_every_axis`: round-trip tagged issuer/principal, session/run, browser-session/target and `assert_eq!(decoded.binding, original.binding)`; reject every missing/unknown axis and User/Connection collisions.
- [ ] RED: `cargo test -p aleph-protocol browser_live::tests::viewer_binding_round_trip_preserves_every_axis -- --exact`; expect compile/missing-module failure and exactly one selected test after scaffold.
- [ ] Implement the exact frozen r4 structs/enums and serde/schema forms; secret wrappers and containing values use redacted hand-written `Debug`, and duplicate permissions/unknown fields/tags/actions fail closed.
- [ ] Add schema/model tests for all epochs/sequences/expiry fields; first-frame `AuthHello/AuthOk/AuthError` size and malformed vectors; tagged browser/artifact binding; same-target candidate with no cross-target IDs; exactly three bus events; and deterministic frame length/bounds. Assert secrets and grant state never appear inside `PreviewResource`.
- [ ] GREEN: rerun the exact test plus `cargo test -p aleph-protocol browser_live::tests::schema_has_epoch_and_expiry -- --exact`; each must run one test and pass. Complete the remaining r4 §8 Task 1 evidence before marking this task done.
- [ ] Commit: `feat(protocol): define task-bound browser live wire`.

### Task 2: Build the single lifecycle owner and bounded stream hub

**Interfaces:** consumes Task 1 bindings; produces `pub async fn LiveProfileOwner::attach_target(&self, binding: BrowserTargetBinding) -> Result<(), BrowserError>`, `pub async fn close_target(&self, target_id: &str) -> Result<(), BrowserError>`, `pub fn subscribe_latest(&self, target_id: &str) -> Result<LatestFrameReceiver, BrowserError>`.

- [ ] Test `one_profile_has_one_connection_and_per_target_streams`: attach two targets, assert one CDP connect, two stream epochs, and exactly one ACK per received frame.
- [ ] Test `slow_or_hidden_viewer_does_not_stop_recording`: latest-frame capacity is `1`; hide/detach drops only that subscription, slow receiver skips old seq, recording remains active and never ACKs.
- [ ] RED separately: `cargo test -p alephcore browser::live::tests::one_profile_has_one_connection_and_per_target_streams -- --exact`; then `cargo test -p alephcore browser::live::tests::slow_or_hidden_viewer_does_not_stop_recording -- --exact`.
- [ ] Implement one profile connection owner with target map; Page screencast start/stop and ACK occur only there. Recording is an observer of the same frame lifecycle.
- [ ] Move mock interception behind the owner's one Fetch consumer; preserve mock add/remove behavior while preparing a second policy source for Task 4. Never spawn an independent policy worker on the same session.
- [ ] GREEN separately: rerun both exact commands; then run legal separate filters `cargo test -p alephcore --lib cdp_backend::routes` and `cargo test -p alephcore --lib cdp_backend::recording`.
- [ ] Commit: `feat(browser): unify live target lifecycle`.

### Task 3: Make Agent/Human control atomic across every mutation entry

**Interfaces:** consumes the exact frozen `ViewerBinding`, `IssuerIdentity`, `HumanInputRequest`, `ControlRequest`, result/reject types, `BrowserLiveState`, and `InterventionSummary`. One per-target serializer owns control mutation and dispatch; internal Rust APIs must preserve r4 §3 semantics and must not introduce a second wire shape. An agent mutation permit remains held until its backend future completes.

- [ ] Add table test over open/navigate/click/type/close-tab/engine-switch/batch exec. While human owns the exact matching binding, every entry returns `BrowserError::HumanHasControl`; another issuer/run/resource/target never satisfies the guard.
- [ ] Add race test: take control after mutation permit acquisition cannot interleave before permit drop; no check-then-act gap.
- [ ] Add dispatcher-barrier interleavings for Input vs Take/Cede/supersede/revoke/expiry/target-close/connection deactivation/restamp/issuer death. Immediately before CDP dispatch starts, recheck the complete queued tuple, permission/controller, binding liveness, and revocation; assert post-bump starts are zero and old queued tuples are purged.
- [ ] Distinguish whole-grant revoke from human-only control loss exactly as r4 §3: whole-grant revoke bumps `control_epoch` and `ws_generation`, closes WS, and clears rotation; human-only loss bumps/purges without closing WS and Frames survive.
- [ ] RED: `cargo test -p alephcore browser::live::control::tests::all_agent_mutations_hold_bound_permit -- --exact`; assert one test fails before guards exist.
- [ ] Cede emits exactly one frozen `InterventionSummary` to its `ConversationBinding`. Connection deactivation/restamp and issuer death execute whole-grant revoke; no disconnect path silently returns Agent control.
- [ ] GREEN: rerun exact control tests and `browser::live::control::tests::stale_input_is_never_replayed`; each runs one test and passes.
- [ ] Commit: `feat(browser): enforce bound control lease atomically`.

### Task 4: Add precise grants and the restricted viewer service

**Interfaces:** consumes and implements the frozen r4 grant, nonce-consume, ticket-exchange, first-frame auth, dedicated viewer message, frame, state, closed, and intervention contracts. Internal route/service APIs may adapt those shared types but must not add authority fields or parallel JSON shapes.

- [ ] Hard gate: require Task 0 `Validated` for the exact engine/cases before product grant code. `network_policy.rs` remains the global baseline; precise grants live with `LiveProfileOwner` and expire/revoke with issuer/run/session/target.
- [ ] Add one Fetch policy decision path combining existing mock rules and precise grants for request/redirect/subresource/iframe/fetch; WS is enabled only if Task 0 proved its pre-connect boundary. DNS/IP is re-resolved at decision time where supported; otherwise block and return to spec review.
- [ ] Register dedicated fragment-nonce consume, exchange-ticket, and viewer WS handlers before SPA fallback in `src/gateway/server/mod.rs`. **WS TCP upgrade 后未认证；首帧 `AuthHello` 通过后才 live/auth.** Upgrade performs only the exact Origin pre-filter. Before successful `AuthOk` queue/write, publish no frame/state/control and add no live entry; failure closes/removes immediately. Viewer sockets never enter generic registries or accept generic JSON-RPC.
- [ ] Fragment flow: launch `/viewer/browser#grant=<nonce>`; Leptos immediately reads and clears the fragment with `history.replaceState`, consumes it once for an `ExchangeTicket`, exchanges that ticket once for S0, then opens WS and sends `AuthHello` as the sole first frame. Secrets never enter query/cookie/header/`Sec-WebSocket-Protocol`, logs, analytics, or error text. Add `Referrer-Policy: no-referrer`.
- [ ] Test `viewer_credential_cannot_cross_scope`: nonce/ticket/`AuthHello` each have one winner; replay, target B attach/input, artifact auth, permission widening, generic `/ws` RPC, and generic subscription registration have zero effect. Assert unauthenticated sockets are never live and receive no frame/state/control; do not use `wall_admits(...) == false` as isolation proof.
- [ ] Add the r4 §8 auth-race, cohort cleanup, lost-`AuthOk`, revoke/expiry, dispatcher, frame-boundary, Origin, and authority tests. Keep supported-proxy Origin, proxy/server/DevTools first-frame-body and access/error-log redaction, CSP/no-third-party/no-cookie/no-store/no-referrer, real CSPRNG/hash, pre-auth socket/fragment limits, and runtime input/IME as explicit deployment/runtime evidence gates; schema/model success cannot satisfy them.
- [ ] RED: `cargo test -p alephcore gateway::browser_viewer_route::tests::viewer_credential_cannot_cross_scope -- --exact`; expect missing route/test, never zero tests.
- [ ] GREEN: rerun exact route/auth test and `browser::live::grant::tests::revocation_closes_frame_and_input -- --exact`; each runs one test and passes only after required runtime evidence is recorded.
- [ ] Commit: `feat(gateway): serve capability-limited browser viewer`.

### Task 5: Share the Leptos viewer between right rail and standalone route

**Interfaces:** consumes `PreviewResource`/`BrowserLiveSnapshot`; produces `#[component] pub fn BrowserViewer(model: BrowserViewerModel, standalone: bool) -> impl IntoView` and `#[component] pub fn BrowserViewerPage() -> impl IntoView`.

- [ ] Add `WorkspaceBody::Browser`; update `available`, `coerce_body`, width and render exhaustiveness. Auto-reveal only the current foreground run's structured resource; replay never reveals; user collapse mutes the same run.
- [ ] Add app pathname handling for `/viewer/browser` in `interfaces/webchat/src/app.rs`; static bytes continue through the Gateway control-plane SPA fallback, while Task 4 registers credential exchange/WS ahead of fallback.
- [ ] Standalone page mounts only the shared viewer component and grant exchange. Panel/Desktop right rail adds candidate comparison, horizontal/multi-screen and animation controls from exact metadata; it does not infer layouts or create duplicate targets.
- [ ] Unit test `resource_binding_survives_refresh_and_reconnect`: restore from backend snapshot, not active tab; assert conversation switch removes old resource and target binding remains exact.
- [ ] RED: `cargo test -p aleph-panel --lib browser_viewer::tests::resource_binding_survives_refresh_and_reconnect -- --exact`; expect missing module/test.
- [ ] GREEN: rerun it plus `browser_viewer::tests::replay_does_not_auto_reveal`; each runs one test and passes.
- [ ] QA: `bash qa/browser_live/run.sh --case standalone-same-target`; assert same target/stream epoch and fragment removed before network/log capture.
- [ ] Commit: `feat(panel): add shared browser workspace viewer`.

### Task 6: Add an explicit managed preview producer

**Interfaces:** produces the exact frozen r4 `PreviewResource` with tagged `ResourceBinding`, `PreviewProvenance`, and revision-scoped `CompanionCandidate`/layout/state metadata; emits only the frozen `BusEvent::Resource`/`Closed` forms. Tool-internal arguments/results must not become a competing wire contract.

- [ ] Register `browser_present` in definition/constructor/struct/dispatch maps. Build `ConversationBinding` from `tools::turn_context::current()` and tagged `IssuerIdentity` from `current_caller_user()` or `current_caller_conn_id()` without collapsing User/Connection to an untyped string; missing identity/session/run fails closed.
- [ ] `CurrentTarget` resolves the actual tool-operation `BrowserTargetBinding`, never a profile active-tab guess. Successful ordinary `browser_open`/`browser_navigate` emit a `Page` resource for the current run, so early preview does not depend on finished artifacts.
- [ ] Managed preview must resolve an exact spawn-time launcher-registry record and derive `PreviewProvenance::Managed` server-side from its launcher/process/timestamp. Rediscovery never retro-attests provenance. Startup/HMR reuse the same already-bound target; process exit emits frozen `browser.live.closed` and disables input.
- [ ] Companion candidates are states/layouts inside one resource revision and always inherit that resource's target. Selection never accepts or creates another target, grant, or WS.
- [ ] Artifact is an optional explicit metadata adapter using `ResourceBinding::Artifact`: it is `NotManaged`, cannot issue/consume/exchange/auth a browser grant, and never creates/closes a browser. Existing artifact consumers remain unchanged.
- [ ] Known unmanaged entry points may emit a warning only when the entry itself declares unmanaged (for example an explicit shell open). Do not auto-detect arbitrary shell URLs or promise lifecycle/control.
- [ ] Test `present_producer_preserves_turn_binding_and_service_exit`: assert tagged issuer/session/run/resource/target axes, one resource event, candidate/HMR same target, stale revision rejection, and exit closure; static artifact asserts zero browser opens and zero viewer authority.
- [ ] RED/GREEN: `cargo test -p alephcore builtin_tools::browser_tools::present::tests::present_producer_preserves_turn_binding_and_service_exit -- --exact`; first missing/failing, then exactly one passing test.
- [ ] Commit: `feat(browser): publish managed preview resources`.

### Task 7: Launch the restricted viewer from TUI（Lane 2）

**Files:** Create `interfaces/tui/src/viewer_launcher.rs`; modify existing `interfaces/tui/src/lib.rs`、`interfaces/tui/src/tui/mod.rs`、`interfaces/tui/src/tui/app/mod.rs`、`interfaces/tui/src/tui/app/events.rs`、`interfaces/tui/src/tui/commands.rs`. `lib.rs`/`tui/mod.rs` expose the module; app events and commands invoke it.

**Interfaces:** consumes `IssueViewerGrantResponse`; produces `pub enum ViewerBrowser { Default, Chrome }` and `pub fn launch_viewer(browser: ViewerBrowser, launch_url: &str) -> Result<ViewerProcess, ViewerLaunchError>`.

- [ ] TUI requests a grant for the selected structured resource, then opens its Gateway `/viewer/browser#grant=...` URL. It does not access a dev server, create a local task browser, use Panel fallback, or add a local execution Bridge.
- [ ] Default browser and explicit Chrome are separate argument builders; command/process errors remain visible with safe manual URL entry. Logs redact fragment.
- [ ] Test `launch_failure_is_not_reported_as_success`: fake launcher exits nonzero; assert typed error and redacted manual URL. Test arguments preserve URL but no extra remote-debug/dev-server flags.
- [ ] RED/GREEN: after memory check, run `cargo test -p aleph-tui viewer_launcher::tests::launch_failure_is_not_reported_as_success -- --exact`; first missing/failing, then one passing test.
- [ ] Integrate Lane 2 result into the feature integration branch (never `main`) before any dependent task.
- [ ] Commit: `feat(tui): launch restricted browser viewer`.

### Task 8: Census migration paths and update reference docs

**Files/verification loci:** `src/builtin_tools/artifact_publish.rs`, `interfaces/cli/src/commands/open_cmd.rs`, `interfaces/webchat/src/components/workspace_panel.rs`, `src/browser/cdp_backend/routes.rs`, `src/browser/engine/obscura.rs`; reference files listed in responsibility map.

- [ ] Census bare URL/iframe/system-open/duplicate state/false-success consumers. For each confirmed consumer choose migrate/adapter/remove and name replacement; absence means no deletion.
- [ ] Remove only verified duplicate truth after its consumer uses `PreviewResource`. Preserve unrelated artifact notification and explicit unmanaged open behavior; no permanent dual contract.
- [ ] Document browser execution/viewer/control in `ARCHITECTURE.md`, producer discovery in `TOOL_SYSTEM.md`, binding/reconnect in `SESSION_KNOBS.md`, grants in `SECURITY.md`, memory/gates in `DEVELOPMENT.md`, routing in `FEATURE_LOCATOR.md`, and QA meaning in `qa/README.md`.
- [ ] RED/GREEN doc check: run a bounded Python assertion that all seven files contain their assigned terms and every linked exact path exists; before edits it must fail at least one assertion, after edits pass. No unverified “other docs”.
- [ ] Commit: `docs(browser): locate workspace contracts and safety gates`.

### Task 9: Integration and acceptance gates

**Interfaces:** `qa/browser_live/run.sh --case <selector>` with selectors `stream`, `control-all-entries`, `standalone-same-target`, `binding-switch`, `managed-companion`, `grant-boundary`, `reap`; `qa/browser_live/check_memory.py --self-test`.

- [ ] Before cargo, diagnose hook/include_dir `STATUS_DLL_INIT_FAILED`; unresolved means stop as environment-blocked, never “green”.
- [ ] Add memory self-test with exact boundary assertions: `assert allowed(4294967296)` and `assert not allowed(4294967295)` for Linux/macOS/Windows fixture parsers.
- [ ] RED/GREEN: `python qa/browser_live/check_memory.py --self-test`; expect missing/failing first, then pass without invoking cargo.
- [ ] Run task-level exact tests with per-command memory checks. After each 2–3 independent tasks, run the smallest affected package compile; before every later cargo invocation repeat the memory check.
- [ ] Run each QA selector explicitly. `control-all-entries` mutates every Agent entry; `binding-switch` proves conversation/refresh/reconnect isolation; `reap` proves close/engine-exit/revoke/expiry and no stale replay; `managed-companion` proves startup/HMR/exit.
- [ ] Run `grant-boundary` only for Task 0 validated engine/cases and assert allowed receiver `>0`, denied receiver `==0`; unsupported WS remains a spec-review blocker.
- [ ] Final targeted commands: `cargo test -p aleph-protocol --lib browser_live`; `cargo test -p alephcore --lib browser::live`; `cargo test -p aleph-panel --lib browser_viewer`; `cargo test -p aleph-tui viewer_launcher`. Never combine two filters.
- [ ] Review dead code/adapters, verify `git -c core.autocrlf=true diff --check`, preserve hooks, and commit: `test(browser): cover workspace integration gates`.

## Dependency and handoff / 依赖与交接

`Task 0 approval+evidence` gates Task 4 network grant；`Task 1` gates Tasks 2–7；Tasks 2+3 gate 4；Task 4 gates standalone security in 5+7；Tasks 5+6 may proceed independently after 2–4；Task 7 remains file-disjoint；Tasks 8–9 follow integration. Task 0 未批准时可审查协议/生命周期计划，但不得实现 precise grant。

Plan complete for implementation review. Do not begin execution until this revised plan is accepted.