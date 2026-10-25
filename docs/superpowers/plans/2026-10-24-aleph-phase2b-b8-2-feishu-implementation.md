# Aleph Phase 2B B8.2 Feishu Capability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在不新增事实源或绕过决策门的前提下闭合 Feishu provider lifecycle，并 source-backed 重审 B8.2-A/B/C/D；任一门无法证明即保留 provider slice 并停止为 NO-GO/deferred。

**Architecture:** ChannelRegistry 是 Gateway lifecycle authority；ProviderHandle 只负责 provider state/generation/admission；api_handle 是 derived cache；Capability descriptor/lease/visibility/owner generation 复用现有 facade/backend/OwnershipTree 契约；SessionEventStore 是 durable session authority。Feishu → Zahir production mount 只有在 A-D 全部 GO 后才允许。

**Tech Stack:** Rust workspace；tokio；serde/schemars；现有 ChannelRegistry/ProviderHandle/FeishuApi/ZahirFacade/ToolBackendAdapter/OwnershipTree/BackendLease/SessionEventStore；cargo tests；qa/channels/run.sh。

**Spec:** docs/superpowers/specs/2026-10-24-aleph-phase2b-b8-2-feishu-capability-design.md

## Global Constraints
- 不新增 Capability kind、第二 callable registry、第二 enabled/actual-state authority、第二 owner/generation/disposer map、第二 durable store。
- 不实现 EffectClaim producer、Safe Replay、universal scheduler、MCP lazy、external exactly-once。
- 不修改 src/harness/；不把静态 in-process provider 描述成 hot unload。
- catalog/describe/list/resolve/project/doctor/observer 不得启动或认证 Feishu。
- 不用 api_handle、broadcast 或测试 fixture 充当 authority 或 durable recovery。
- 不自动 commit、push、pull、reset、rebase、stash。
- 每条 cargo 命令前分别运行 `python qa/browser_live/check_memory.py --self-test` 和 `python qa/browser_live/check_memory.py`；两者都必须 exit 0 且 `available_bytes >= 4294967296`，记录 available bytes、exit 和 warnings。
- `qa/channels/run.sh all` 缺少 websockets 或其他依赖只能报告 BLOCKED/UNRUN，不改仓库伪造通过。
- 只有 A-D 全部 GO 才实现 production Feishu → Zahir mount；不能证明即停止。

## Review Focus
- disabled boot 是否产生 token/API/listener/WS/emitter/cache/publication：由 Task 1 效果测试固定。
- replacement 后旧 instance/generation 的 publish/withdraw 是否能覆盖新实例：由 Task 2 固定。
- stop 的 accepted handler、顶层 accept task、已准入 API call 是否被错误宣称为完整 join：由 Task 2 residual 测试固定。
- 既有 CapabilityKind 是否真实表达 Feishu，是否错误复用 Tool：由 Task 3 契约映射和 gate 固定。
- observer lag 是否被误当逐条 replay 或 durable recovery：由 Task 5 测试固定。

---

### Task 0: Freeze baseline and repair audit inputs

**Files:**
- Read: `docs/research/aleph_modularization_phase2b_b8_2_implementation_plan.md`
- Read: `docs/research/aleph_modularization_phase2b_b8_2_plan.md`
- Read: `docs/research/aleph_modularization_phase2b_b8_2_prompt.md`
- Read: `docs/research/aleph_modularization_phase2b_b8_2_implementation_prompt.md`
- Read: `docs/research/aleph_modularization_phase2b_b8_audit.md` (expected path; if absent record absence)
- Read: `docs/research/aleph_modularization_phase2b_b8_plan.md`
- Read: `docs/reference/FEATURE_LOCATOR.md`, `docs/reference/ARCHITECTURE.md`, `docs/reference/TOOL_SYSTEM.md`
- Create or modify only the appropriate B8.2 audit/closeout document after confirming the actual path.

**Interfaces:** Produces a baseline record with actual `HEAD`, branch, status, worktrees, B5/B6/B7/B8 history, source-backed current claims, and a failure matrix skeleton. Does not claim clean status if untracked files exist.

- [ ] Step 1: Use read-only commands to record `pwd`, git root, HEAD, status, log, worktree list and B milestone history; do not reset or hide user files.
- [ ] Step 2: Locate the actual B8.2 audit/closeout filename; if the requested audit path is absent, record that fact and use an explicitly named replacement rather than inventing continuity.
- [ ] Step 3: Write/update only the audit/closeout record with actual evidence and preserve prior audit claims as historical.
- [ ] Step 4: Review the diff; expected result is documentation-only and no production code changes.

### Task 1: Close ChannelRegistry-owned Feishu lifecycle

**Files:**
- Modify only after source inspection: `src/gateway/channel_registry.rs`, `src/gateway/interfaces/feishu/**`, relevant Gateway lifecycle/boot files.
- Test: existing Feishu contract/fixture/protocol tests and the smallest existing module test file owning the lifecycle.

**Interfaces:** Preserve `ChannelRegistry` as enabled/desired/actual lifecycle authority. Preserve `ProviderHandle` as provider state/generation/admission fence. Preserve `FeishuApi` and its real admission/drain semantics. Any new test helper must not become authority.

- [ ] Step 1: Identify the current disabled boot path and write failing effect tests asserting no token request, listener bind, WS/emitter runtime, cache entry, or publication.
- [ ] Step 2: Run only the focused test target after the required memory gates; expected FAIL for each currently missing effect.
- [ ] Step 3: Implement the smallest lifecycle change so disabled boot is decided by ChannelRegistry and no Feishu runtime is constructed.
- [ ] Step 4: Add/adjust boot, explicit enable, stop and replacement tests using the existing owner/registry path; assert provider failure remains Failed/residual.
- [ ] Step 5: Run focused Feishu lifecycle tests; expected PASS with no duplicated enabled state.

### Task 2: Close replacement, publication fence, admission and drain boundaries

**Files:**
- Modify: the source-backed Feishu provider/api_handle/webhook/WS lifecycle files found in Task 1.
- Test: existing Feishu contract/fixture/protocol tests plus lifecycle module tests.

**Interfaces:** Preserve instance/generation fence for `api_handle::publish` and `withdraw`; old generation cannot mutate new publication. Preserve actual distinction among top-level accept task, accepted handler and admitted API call. If complete handler join cannot be proven, expose structured residual and test it rather than claiming full join.

- [ ] Step 1: Write failing replacement tests: old port released, old admission closed, old generation publish/withdraw cannot affect new instance.
- [ ] Step 2: Write failing stop/revoke tests: new calls fail closed; admitted work completes only according to actual drain/join semantics; incomplete join leaves residual.
- [ ] Step 3: Implement fences and stop ordering in existing lifecycle owner; do not add a second owner/generation/disposer map.
- [ ] Step 4: Run focused tests; expected PASS and explicit residual on unsupported join.

### Task 3: Prove or stop at B8.2-A/B

**Files:**
- Read/modify only contract mapping and tests initially: `src/capability/descriptor.rs`, `src/capability/facade.rs`, `src/capability/backend.rs`, `src/capability/ownership.rs`, `src/capability/provider.rs`, `src/capability/zahir_facade.rs`, `src/capability/projection_host.rs`, relevant Feishu files.
- Test: `src/capability/**` tests and a focused Feishu contract test if an existing test location is source-backed.
- Docs: appropriate B8.2 audit/closeout and reference files only after decision.

**Interfaces:** Existing `CapabilityKind` values are `Tool`, `Skill`, `Agent`, `Task`, `Resource`, `EventSource`, `Subscription`, `Plugin`, `Hook`; current `ToolBackendAdapter` maps to Tool. Existing `BackendLease` has `descriptor`, `owner_generation`, `provider_generation`, `canonical_entry`. `OwnershipTree::generation(&CapabilityId, &VisibilityScope) -> Option<OwnerGeneration>` is sole owner-generation source. `ZahirFacade::resolve`, `validate_lease`, `subscribe`, and `project` remain existing contracts.

- [ ] Step 1: Write the contract table before code: semantic kind, descriptor source, route metadata, visibility/reference, owner, revision, fingerprint, provider generation, owner generation, revoke/replacement behavior.
- [ ] Step 2: Validate that descriptor projection is read-side and does not construct/authenticate Feishu.
- [ ] Step 3: Add only tests needed to prove identity, visibility, provider/owner generation and stale lease fail-closed, reusing existing facade/backend/ownership contracts.
- [ ] Step 4: If no existing kind expresses Feishu semantics without Tool masquerade or a copied authority, record B8.2-A/B `NO-GO/deferred`, update docs, and stop. Do not implement adapter or mount.
- [ ] Step 5: If both gates are GO, record exact lease route and proceed to Task 4.

### Task 4: Prove callable/effect boundary only after A/B GO

**Files:**
- Modify only source-backed existing callable route files and Feishu admission files.
- Test: focused callable/Feishu contract tests.

**Interfaces:** Required route is `Capability resolve -> lease validation -> ChannelRegistry actual instance -> ProviderHandle generation admission -> FeishuApi admission -> outbound Feishu call`. Read-side operations remain pure. Webhook remains Gateway-owned external effect unless existing contract proves otherwise.

- [ ] Step 1: Write failing tests proving catalog/describe/list/resolve/project/doctor do not token-auth, bind listener, start WS or call Feishu API.
- [ ] Step 2: Write failing outbound tests proving each route stage and stale/revoked/replaced calls fail closed.
- [ ] Step 3: Implement only missing route composition using existing authorities; no callable registry.
- [ ] Step 4: If inbound effect needs forbidden EffectClaim/Safe Replay/scheduler/exactly-once, record C `NO-GO/deferred`, preserve Gateway-owned event path, and stop.
- [ ] Step 5: Otherwise run focused tests and record C GO.

### Task 5: Prove observer/recovery boundary only after prior gates GO

**Files:**
- Modify only existing capability observer/projection and ChannelRegistry lifecycle notification files.
- Test: existing facade stream tests plus focused lifecycle observer tests.

**Interfaces:** `subscribe` happens before snapshot; broadcast is live notification only; lag emits `Invalidated`, then snapshot is re-read from authority. No durable recovery from api_handle/broadcast. SessionEventStore remains the only durable session authority.

- [ ] Step 1: Write failing subscribe-before-snapshot race, registration/replacement/stop lag, subscriber restart and live-vs-durable separation tests.
- [ ] Step 2: Implement revision/epoch or equivalent invalidation through existing authority, without second durable store.
- [ ] Step 3: Run focused observer tests; expected PASS with no per-event replay claim.
- [ ] Step 4: If provider lifecycle cannot be represented by existing durable event contract, record D `NO-GO/deferred` and stop.

### Task 6: Update reference and FEATURE_LOCATOR truthfully

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md`
- Modify: `docs/reference/ARCHITECTURE.md`
- Modify: `docs/reference/TOOL_SYSTEM.md`
- Modify: actual B8.2 audit/closeout document identified in Task 0

**Interfaces:** Documentation must match source and tests. It must state whether production mount exists, exact A-D outcomes, failure matrix, residuals, blocked/unrun checks and next smallest prerequisite.

- [ ] Step 1: Write source-backed status and call graph with full paths and line numbers.
- [ ] Step 2: Write unique authority mapping and failure matrix.
- [ ] Step 3: Record verification evidence exactly; never turn BLOCKED/UNRUN into PASS.
- [ ] Step 4: Review all stale “Feishu connected to Zahir” claims and correct them.

### Task 7: Independent review and verification

**Files:** No production changes unless review finds a defect; review the diff and docs.

- [ ] Step 1: Fresh reviewer checks architecture redlines, duplicate authorities, stale claims, and gate conclusions.
- [ ] Step 2: Before each cargo command run both memory gates and record exit/available/warnings.
- [ ] Step 3: Serially run `cargo fmt --all --check`; focused Feishu tests; capability tests; Feishu lib tests; full alephcore lib; all-target check; clippy; `qa/channels/run.sh all` as applicable to actual changes.
- [ ] Step 4: If any check fails, create a blocker task and keep the owning task in progress; do not claim completion.
- [ ] Step 5: Final report includes actual HEAD/branch/status/worktree, modified/unmodified scope, source-backed call graph, authority map, A-D results, failure matrix, every memory gate, North Star conformance, residuals, BLOCKED/UNRUN and production mount condition.

## Stop Conditions
Stop implementation at the first unproven decision gate. A lifecycle fix that does not establish the Capability contract is still a valid provider slice but cannot be called production mount. Never invent `docs/research/aleph_modularization_phase2b_b8_2_audit.md` if it is absent; name the actual closeout file.

## Handoff
Implement with subagent-driven development, one writer per worktree, one task in progress at a time. Keep the approved design and this plan in scope. Before production changes, review exact current source signatures; this plan intentionally does not invent signatures that were not source-backed.
