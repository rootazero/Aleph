# Aleph Context Plugin Fabric Implementation Plan

> **For agentic workers:** 本计划由主会话按双线并行方式派发执行。Steps 用 checkbox 追踪。执行者必须先阅读批准的设计文档 `docs/superpowers/specs/2026-10-19-context-plugin-fabric-design.md`，再按本计划逐任务实施。

**Goal:** 将 Aleph 的上下文 Admission、Preflight、Compaction、Recovery、Search、Status 和 Lifecycle 收敛为 Capability 化 Context Fabric，并让第三方插件按 phase 或原子 bundle 替换/组合内置能力。

**Architecture:** Core 保留现有 `PreflightStage`、`ContextCompactor`、fold/recovery、ContentIndex 和 tool-output pipeline 作为内置实现；新增窄 facade、descriptor、registry、宿主授权和 adapter boundary。Rust native stages 与外部 serde wire contract 分离，避免把现有静态 trait 当作插件 ABI。`session_events` 仍是唯一事实源，插件只能产生经过 Core 校验的 bounded result 或可重建派生缓存。

**Tech Stack:** Rust、tokio、serde/schemars、现有 Aleph extension registry、WASM/MCP/static runtime、SQLite/sqlite-vec、现有 `alephcore` tests。

**Spec:** `docs/superpowers/specs/2026-10-19-context-plugin-fabric-design.md`

## Global Constraints

- 所有代码操作只在 `D:/Workspace/Aleph/.worktrees/context-fabric-plugin`（分支 `codex/context-fabric-plugin`）进行，禁止直接修改 main checkout。
- `session_events` 是唯一持久化真源；插件 checkpoint、索引和缓存必须可从 Core 事件/既有索引重建，不能创建第二事件库。
- `Everything externally composable is a Capability.` 任何可被 Agent、Runtime 或其他系统 discover/invoke/replace/subscribe/manage 的上下文能力必须有 descriptor、稳定 contract、权限和生命周期。
- 内置实现是默认 owner；Replace 或 Bundle 失败只能在当次调用内使用 bounded built-in fallback，并让 degraded 状态可见。
- 宿主显式授予 namespace/scope；默认只允许当前 session，project scope 默认不可跨 session；跨 session 访问必须显式授权并审计。
- 历史 fold 通过 owner/provenance 路由；旧事件缺少 owner 时按 builtin 解释，不能因新插件接管而隐藏旧 fold。
- 恢复内容走普通工具结果或动态 tail，不改 system prompt 稳定前缀；普通隐藏 reasoning 默认不持久化。用户确认仅 Core 可保留目标协议强制要求的续跑推理内容或签名，全部推理从插件、摘要和通用检索投影中剔除。 / Core may retain only provider-required protocol artifacts for resume compatibility; plugin, summarization and general retrieval projections exclude all reasoning.
- 网络出口和项目目录写入默认拒绝；必须经过显式 capability 声明、宿主授权和审计。
- 不新增第二套插件系统、MCP server、CDP/terminal 实现；复用现有 `src/extension/` runtime 与 lifecycle。
- 不扩张 `src/harness/` 超过 R10 允许的 12 文件边界；每个 harness 修改都必须说明其对现有链路的必要性。
- Rust 生产代码遵循现有错误传播和命名风格；不得以 `unwrap` 掩盖插件、wire、预算或 session 失败。
- 跑 cargo 前检查可用内存；低于 4GiB 时暂停 cargo 并重新检查。当前 Windows 验证统一使用 `CARGO_BUILD_JOBS=4 CARGO_PROFILE_DEV_DEBUG=0`：默认 debug profile 曾在 test crate 编译时 LLVM OOM，低 debug profile 已通过 Task 1 focused tests；后续复用该 profile 避免重复构建。 / Check available memory before cargo; keep one verified low-debug profile and use four workers when at least 4GiB is available.
- 每个任务收尾至少运行相关域测试；最终运行 `cargo fmt --check`、`cargo check`、`cargo clippy -- -D warnings` 和必要的 `cargo test`。pre-commit rustfmt 漂移若仍存在，提交可用 `--no-verify`，但不得跳过手动验证。
- 文档和 reference 更新使用中英双语，代码注释使用英文；不复制代码中的第二份事实。

## Review Focus

1. **Legacy folds without owner**：旧 `FoldRecorded`/`CompactionPerformed` 必须继续列出和恢复，`owner = None` 要解释为 builtin；测试归入 Task 4。
2. **Owner switch with mixed history**：owner A 的 fold 在 owner B 接管后仍必须通过 Core 路由恢复，插件 B 不得意外获得 A 的 scope；测试归入 Task 4/5。
3. **Plugin timeout with oversized output**：插件超时或返回越界内容时，原始无限输出不得进入 prompt，必须在同一调用内进入 bounded builtin fallback；测试归入 Task 3/5。
4. **Native trait vs external wire**：动态插件 id 不能迫使 `PreflightStage::name(&self) -> &'static str` 变成不兼容的 ABI；测试归入 Task 1/3。
5. **Permission and side-effect escalation**：自声明 project/cross-session、网络出口、项目写入都必须被宿主授权拒绝或审计；测试归入 Task 2/6。

---

## Task 1: Define Context Capability wire contract

**Files:**
- Create: `src/context/fabric.rs`（或现有模块验证后拆为 `src/context/fabric/mod.rs`）
- Modify: `src/context/mod.rs`
- Modify: `src/extension/capability.rs`（仅增加 context capability kind/permission mapping；不复制 descriptor）
- Test: `src/context/fabric.rs` unit tests，必要时 `tests/` context contract tests

**Interfaces:**

- Produce `ContextPhase` with `Admission`, `Preflight`, `Compaction`, `Recovery`, `Search`, `Status`, `Lifecycle`。
- Produce `ContextCapabilityMode` with `Builtin`, `Replace`, `Compose`, `Observer`, `Interceptor`, `Bundle`。
- Produce serde/schemars `ContextCapabilityDescriptor` containing stable `id`, `api_version`, `implementation_version`, phases/mode, namespace scope, input/output budgets, persistence policy, side-effect permissions and lifecycle capabilities。
- Produce `ContextCapabilityRequest`, `ContextSnapshot`, `ContextBudget`, `ContextTransformResult`, `DegradedReason` and versioned operation names for `describe`, `admit`, `preflight`, `compact`, `restore`, `search`, `status`, `shutdown`。
- Keep `PreflightStage::name(&self) -> &'static str` unchanged. Dynamic plugin ids live in `ContextCapabilityDescriptor.id` and registry keys; the external adapter, not the native trait, bridges dynamic wire calls to native execution.

- [ ] **Step 1: Write failing tests** for serde round-trip, schemars generation, unknown optional fields, stable phase/mode tags, bounded budget serialization, and rejection of an undeclared phase.
- [ ] **Step 2: Run** `cargo test -p alephcore --lib context::fabric`; expected initial compile/test failure because the module and types do not exist.
- [ ] **Step 3: Implement** the smallest wire module and mount it from `src/context/mod.rs`; use existing serde/schemars tagging and namespaced key conventions from `src/extension/`.
- [ ] **Step 4: Run** the focused tests and confirm exact JSON shape is stable.
- [ ] **Step 5: Commit** `git commit --no-verify -m "context: define capability fabric wire contract"`.

## Task 2: Add host grants, registry ownership, and atomic bundles

**Files:**
- Modify: `src/extension/capability.rs`
- Modify: `src/extension/registry/types.rs`
- Modify: `src/extension/registry/plugin_registry/mod.rs`
- Modify: `src/extension/plugin_trust.rs` and/or `src/extension/hooks/consent_scope.rs` only after confirming the existing grant seam
- Test: existing `extension::registry` tests plus context-specific registry tests

**Interfaces:**

- Produce `ContextGrant { capability_id, plugin_id, namespace, scope, cross_session, egress, project_write, expires_at, audit_reason }` with host-issued semantics.
- Produce `ContextRegistration` keyed by the single namespaced component key; it carries descriptor, grant, lifecycle handle, owner mode and optional atomic bundle id.
- Produce registry operations equivalent to `register_context`, `unregister_context_bundle`, `resolve_context`, `list_context_descriptors`, `validate_context_grant`, and `context_stats`.
- Duplicate final owners for one phase or overlapping bundle must fail before invocation. Bundle registration/unregistration is all-or-nothing; disabling a plugin removes its context registrations and leaves builtin ownership valid.
- Keep existing `CapabilityDeclaration` static kind methods valid; dynamic phase data remains in `ContextCapabilityDescriptor`.

- [ ] **Step 1: Write failing tests** for host grant required, default session isolation, cross-session denial, side-effect denial, duplicate owner rejection before invocation, bundle atomic install/removal, disabled plugin cleanup, and registry stats.
- [ ] **Step 2: Run** `cargo test -p alephcore --lib extension::registry`; capture expected failures.
- [ ] **Step 3: Implement** registration and grant validation by reusing existing plugin trust/consent/lifecycle seams; do not create another registry.
- [ ] **Step 4: Run** focused registry tests and verify no ungranted snapshot reaches an adapter.
- [ ] **Step 5: Commit** `git commit --no-verify -m "extension: register context capabilities with host grants"`.

## Task 3: Assemble the builtin ContextFabric and preflight registry

**Files:**
- Modify: `src/context/budget/preflight.rs`
- Modify: `src/context/fabric.rs`
- Modify: `src/context/compact/compactor.rs` only where facade injection is required
- Modify: `src/tool_output/ingress.rs`, `src/tool_output/hygiene.rs`, `src/tool_output/structured/*`, `src/tools/result_store.rs` only for the bounded Admission adapter
- Test: `src/context/budget/preflight.rs`, tool-output domain tests, `tests/preflight_cheap_passes_e2e.rs`

**Interfaces:**

- Produce `ContextFabric` and `ContextFabricBuilder` that consume existing builtin `PreflightPipeline`, `ContextCompactor`, `ContentIndex`, session event reader, and registry handles.
- Produce `ContextPipelinePolicy` selecting builtin, one replace owner, ordered compose stages, observer, or validated bundle per phase.
- Produce a registered native stage wrapper that retains `PreflightStage::prepare(&mut Vec<UnifiedMessage>, &ContextPressure, fresh_tail_count) -> usize` and uses a separate descriptor id; do not widen the static trait to dynamic plugin strings.
- Replace `default_pipeline`'s hardcoded assembly with a builder-provided ordered registration while preserving both `llm.clears_tool_results_server_side()` stage-list shapes and existing `pub(crate)` cache-stability/min-pressure setters.
- Admission adapter must enforce existing UTF-8/JSON/byte/token bounds and return a bounded fallback result when an external adapter times out or violates output contract.

- [ ] **Step 1: Write failing tests** for builtin-default equivalence, registration ordering, both server-side tool-result branches, phase owner policy, plugin timeout fallback, over-budget output fallback, and no raw unbounded result entering the next prompt.
- [ ] **Step 2: Run** `cargo test -p alephcore --lib context::budget::preflight` and `cargo test -p alephcore --test preflight_cheap_passes_e2e`; confirm failures.
- [ ] **Step 3: Implement** facade and native stage registration first; then connect the existing Admission pipeline through the same facade. Keep plugin transport outside native stage logic.
- [ ] **Step 4: Run** focused context/tool-output tests and compare builtin output/prefix stability with the pre-change behavior.
- [ ] **Step 5: Commit** `git commit --no-verify -m "context: assemble builtin fabric and registered preflight"`.

## Task 4: Add fold owner provenance and Core-routed recovery

**Files:**
- Modify: `src/session/events.rs`
- Modify: `src/context/compact/folds.rs`
- Modify: `src/context/compact/manual.rs`
- Modify: `src/builtin_tools/decompress.rs`
- Modify: `src/session/store.rs` projection classification if needed
- Test: fold, event compatibility, and decompress tests

**Interfaces:**

- Extend `SessionEvent::FoldRecorded` with an optional serde-compatible owner/provenance field; `None` means builtin for all historical events.
- Extend `FoldRecord` and `list_folds(session_key, events)` to preserve owner, prefer recorded folds over derived legacy folds, and keep deterministic ids.
- Stamp the builtin owner when the builtin manual compaction writes the event batch; the `CompactionPerformed` + `FoldRecorded` pair remains atomic.
- Add Core recovery routing so `session_decompress` resolves old folds by owner/provenance. A new owner does not automatically gain access to foreign folds; unavailable owner adapters produce an explicit degraded result and bounded builtin recovery where valid.
- Preserve `Retire::Through` readable fold spans and report `Retire::From` hard-deleted ranges honestly.

- [ ] **Step 1: Write failing tests** for old event deserialization, `owner=None` builtin behavior, owner A/B mixed history, recorded-vs-derived deduplication, atomic pair emission, exact restore, and hard-retired range errors.
- [ ] **Step 2: Run** `cargo test -p alephcore --lib context::compact::folds` and `cargo test -p alephcore --lib builtin_tools::decompress`; confirm failures.
- [ ] **Step 3: Implement** the optional event field and owner-aware pure fold functions, then wire the manual emission and Core router.
- [ ] **Step 4: Run** event variant-name compatibility tests, fold/decompress tests, and the relevant session store projection tests.
- [ ] **Step 5: Commit** `git commit --no-verify -m "compact: route folds by capability owner"`.

## Task 5: Rewire harness injection and context lifecycle events

**Files:**
- Modify: `src/harness/deps.rs`
- Modify: `src/harness/agent/think.rs`
- Modify: `src/orchestrator/harness_bridge/runner_impl.rs`
- Modify: `src/agents/subagent_spawner/mod.rs`
- Modify: `src/extension/lifecycle.rs` and `src/extension/hooks/registry.rs` only for context lifecycle events and disposer integration
- Test: harness regression tests, lifecycle tests, prefix stability tests

**Interfaces:**

- Replace concrete-only context injection where needed with a narrow enum/trait boundary that can hold builtin or validated `ContextFabric` owner; preserve existing `Option<Arc<PreflightPipeline>>`, `Option<Arc<ContextCompactor>>`, and `Option<Arc<Mutex<ContextBudget>>>` behavior for builtin callers until all call sites are migrated.
- Both `runner_impl.rs:851` and `subagent_spawner/mod.rs:1476` must assemble the same fabric/policy; add a regression proving the subagent path cannot silently omit preflight.
- Add lifecycle events: `session_start`, `tool_result_admitted`, `before_preflight`, `after_preflight`, `before_compaction`, `after_compaction`, `fold_recorded`, `restore_requested`, `turn_end`, `session_shutdown`, using existing extension event/disposing mechanisms.
- Propagate turn deadline/cancellation to plugin invocation; on timeout invoke the Task 3 bounded builtin fallback in the same call and expose degraded status.

- [ ] **Step 1: Write failing tests** for both harness assembly paths, lifecycle event order, disposer idempotence, plugin timeout cancellation, and prefix stability after admission/recovery.
- [ ] **Step 2: Run** `cargo test -p alephcore --lib harness` plus lifecycle/prefix focused tests; confirm failures.
- [ ] **Step 3: Implement** the narrow seam and shared assembly helper; avoid expanding the harness file count without documenting the ratchet impact.
- [ ] **Step 4: Run** focused harness/lifecycle tests and `cargo fmt --check` on touched files.
- [ ] **Step 5: Commit** `git commit --no-verify -m "harness: wire context fabric lifecycle and fallback"`.

## Task 6: Add external adapter fixture and rebuildable plugin cache

**Files:**
- Modify: `src/extension/runtime/wasm/*`, `src/extension/registrar/mcp_registrar.rs`, `src/extension/loader.rs`, or the smallest existing adapter boundary discovered during implementation
- Modify: `src/context/compact/compaction_cache.rs`
- Modify: `src/context/retrieval/content_index.rs` only for namespace-aware derived state
- Test: adapter fixture tests and namespaced-cache rebuild tests

**Interfaces:**

- Produce one minimal external adapter fixture using an existing static/WASM/MCP runtime; do not add a second plugin transport. It must exercise `Observer`, `Admission`, `Replace`, and `Compose/Bundle` at protocol level.
- Adapter operations must use `ContextCapabilityDescriptor` and host `ContextGrant`; snapshots exclude hidden reasoning and ungranted sessions.
- Add namespace and owner/version to plugin cache keys; never route this through process-global `COMPACTION_CARRYOVER`. Cache loss must not lose fold/recovery truth; dropping the cache must leave status/search/recovery answerable from Core data.
- Register plugin lifecycle shutdown and enforce default-denied egress/project write permissions; fixture tests must prove denied side effects are never invoked.

- [ ] **Step 1: Write failing adapter/cache tests** for descriptor handshake, grant denial, observer no-op, admission bounded result, replace fallback, compose bundle, timeout, cache drop/rebuild, and side-effect denial.
- [ ] **Step 2: Run** the adapter and cache-focused tests; confirm failures.
- [ ] **Step 3: Implement** the smallest adapter around the existing runtime and namespaced derived cache.
- [ ] **Step 4: Run** focused tests and one cross-restart/rebuild test using existing session fixtures.
- [ ] **Step 5: Commit** `git commit --no-verify -m "extension: exercise external context adapter and rebuildable cache"`.

## Task 7: Synchronize reference docs and capability projections

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md`
- Modify: `docs/reference/AGENT_LOOP_CONTEXT_BUDGET.md`
- Modify: `docs/reference/EXTENSION_SYSTEM.md`
- Modify: `docs/reference/PLUGIN_SYSTEM.md`
- Modify: `AGENTS.md` and `CLAUDE.md` only for changed durable Capability facts
- Test/verify: `rg` consistency checks, `aleph-server prompt-size` if descriptors/tool descriptions change

**Interfaces:**

- Document the single source of truth for Context Fabric descriptors, host grants, bundle ownership, owner-routed fold recovery, bounded fallback, namespace cache, and lifecycle events.
- Add FEATURE_LOCATOR entries pointing to exact implementation symbols and verification tests; remove stale statements that claim context capabilities are deferred when they are implemented.
- Keep the new principle verbatim in the design/reference layer: `Everything externally composable is a Capability.`
- Do not copy implementation details into multiple docs; cross-reference the canonical reference sections.

- [ ] **Step 1: Search** all existing Context Fabric, preflight, fold, extension and plugin claims and record stale locations.
- [ ] **Step 2: Update** canonical references and the locator from final code symbols, not from the plan's guesses.
- [ ] **Step 3: Run** doc link/anchor checks, `rg` stale-claim checks, and `aleph-server prompt-size` when applicable.
- [ ] **Step 4: Commit** `git commit --no-verify -m "docs: document capability-based context plugins"`.

## Parallel execution and integration order

- **Line 1 — Core contract and lifecycle:** Task 1 first; then Task 2 and Task 4 may run in parallel because they touch separate contracts/files; Task 3 starts after Task 1+2; Task 5 starts after Task 3+4.
- **Line 2 — Admission and adapter:** Task 3's Admission portion may be developed alongside Task 4 only after Task 1's wire types are stable; Task 6 starts after Task 2+3+5. Shared files (`compactor.rs`, `preflight.rs`, `extension/registry`) must have one owner per task; no concurrent edits to the same file.
- **Integration checkpoint 1:** After Tasks 1–4, run focused context/session/tool-output tests and inspect the diff before changing harness files.
- **Integration checkpoint 2:** After Task 5, run both harness assembly paths and prefix stability tests before enabling any external adapter fixture.
- **Final integration:** Task 7 is last. Then run memory check and, sequentially, `cargo fmt --check`, `cargo check`, `cargo clippy -- -D warnings`, `CARGO_BUILD_JOBS=4 CARGO_PROFILE_DEV_DEBUG=0 cargo test -p alephcore --lib`, targeted integration tests, and required real-machine plugin QA.
- **Commit policy:** Each task is independently committed. Main session reviews actual diff and tests after every merge; no subagent report alone marks a task complete.

## Self-review against the approved spec

- Capability principle, descriptor, phase model, replace/compose/bundle, host grants, lifecycle and side-effect permissions: Tasks 1–3, 5–6.
- Builtin default and existing-module reuse: Task 3.
- Owner/provenance, single event source, mixed-history recovery: Task 4.
- Admission bounds, fallback, cache stability and hidden reasoning exclusion: Tasks 3, 5, 6.
- External adapter and billion-context compatibility fixture: Task 6.
- Diagnostics/status/registry observability and docs: Tasks 2, 6, 7.
- Verification, reference updates and no second plugin system: Global Constraints + Tasks 5–7.

Known implementation decisions intentionally left to workers after code inspection: exact facade file split, exact existing extension adapter selected for the fixture, whether `ContextGrant` reuses or extends existing consent scope storage, and the smallest enum/trait seam for concrete `HarnessDeps`; these do not change the approved product behavior or security boundaries.
