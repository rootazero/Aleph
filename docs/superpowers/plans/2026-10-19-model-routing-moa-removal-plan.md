# Aleph 模型路由与 MoA 删除实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 删除 Aleph 的 MoA 多模型聚合和策略性模型路由，把任务拆分与模型分配交给主 Agent，同时保留显式模型绑定、provider failover、subagent 生命周期和历史数据兼容。

**Architecture:** 主 Agent/Skill 决定任务拆分、模型绑定和协作策略；通用 Subagent Runtime 只提供模型无关、任务无关的 spawn、join、cancel、collect、隔离、权限、并发和失败可见性。MoA、Debate、Review、Swarm 和 pipeline 是主 Agent/Skill 可动态组合的策略，不是 Runtime 的固定执行原语。Runtime 不内置投票、聚合、最佳答案选择、自动模型重选或策略性路由；`verify` 由调用方提供证据和验证规则。先以契约测试判断现有 tracker/spawner/recovery 是否已经足够，只有证明缺少稳定 handle、join 或 collect 语义时才做最小 generic facade 收敛，不复制状态机。运行时最终仍只保留显式 `select_model`/subagent model directive、一个 `effective_model_directive` 汇合点和一个 `Orchestrator -> Harness` 执行路径；失败恢复仍由 provider chain 负责。共享协议保留 deprecated MoA trace typed variants，只用于历史读取。

**Tech Stack:** Rust workspace (`alephcore`, `aleph-server`, `aleph-panel`)、Leptos/WASM Panel、TUI、SQLite/sqlite-vec、serde/TOML/JSON-RPC、Python stdlib archive helper、现有 Cargo/just 验证命令。

**Spec:** `docs/superpowers/specs/2026-10-19-model-routing-moa-removal-design.md`

**Current status:** Tasks 1–5（archive、MoA core/provider/UI core deletion、proposer aggregator deletion、historical trace compatibility、learned routing/AgentDef hints deletion）已完成并分别提交；本计划只继续收敛 Task 6–9，不回滚这些删除。

**Builder check:** 已完成；没有重要未决问题。已知限制是完整 `cargo check --tests` 曾因资源超时并收到 SIGKILL，后续必须单独验证并如实记录。

## Global Constraints

- 不删除 `src/routing/` 的 channel/peer -> agent/session 确定性路由；只删除 model routing recall/experience/observer/attribution。
- 删除 MoA runtime、builtin、RPC、slash command、Panel/TUI 入口和 proposal/aggregator 行为；普通主 Agent 显式 subagent 并行保留。
- 保留 `list_models`、普通 `select_model`、显式 subagent provider/model override、provider catalog、`ModelOverrideProvider`、retry/failover/cooldown、cancel/resume/child-first cleanup。
- 显式 `(provider, model)` 继续约束 primary；只在既有可恢复失败语义下进入 provider 配置的备用链，不新增认知重选；trace/cost/resume 保留原始 directive 与实际 serving model 的既有记录语义。
- 旧 `[moa]` 配置只忽略并诊断，不自动改写；旧 Agent frontmatter `model_hint`/`provider_hint` 只忽略并诊断，不阻止加载。
- 新数据库不创建 routing relational/vector objects；旧对象全部忽略，不补建、不校验、不 DROP。
- 共享协议保留 MoA trace typed variants 作为 deprecated historical compatibility；新运行不得生产这些事件。
- 归档必须在任何删除前生成，包含完整删除文件和保留文件的 pre-removal snapshots，并有 manifest、SHA-256 和动态引用审计。
- 不新增第三方依赖；Python archive helper 只使用标准库。
- 完整 `cargo check --tests` 曾因资源超时并收到 SIGKILL 未通过；后续必须单独验证，不能把该次失败当作代码回归或成功结果。
- 通用 facade 若现有 tracker/spawner/recovery API 已满足契约测试，不强行新增；只有契约测试证明缺口时才引入最小稳定 handle/join/collect 收敛。
- 每个任务完成后独立运行其测试并提交一个可审查 commit；不得把失败测试改成无意义的 no-op。

## Review Focus

1. **历史 MoA trace decode/replay**：旧事件仍能读取和展示，新运行不产生事件。测试：共享协议序列化/反序列化、trace presentation/replay fixture 和 SQLite trace reader。
2. **显式 model 与 failover 边界**：显式 directive 仍命中 primary，只有既有失败分类可进入备用链，resume 不把 fallback 变成 session pin。测试：`effective_model_directive`、`ModelOverrideProvider`、failover decision/provider、resume marker。
3. **旧 Agent frontmatter 与配置兼容**：含旧 hints 或 `[moa]` 的用户配置能启动，字段被忽略并给出诊断。测试：loader fixture、config loader/diagnostic fixture。
4. **routing SQLite 半成品状态**：relational/mapping/vector 对象全存在、部分存在、扩展不可用或全不存在时都能启动，且不补建。测试：schema fixtures 和普通 memory initialization。
5. **删除入口不成功 no-op**：旧 builtin、slash、RPC、`moa:*` request、Panel/TUI 操作不能误报成功；普通 model picker 仍走同一显式 directive。测试：gateway method census、builtin registry、Panel chat/settings、TUI picker。

---

### Task 1: 生成删除前归档与引用闭包

**Files:**
- Create: `scripts/archive_model_routing_moa.py`
- Create: `archive/removed-model-routing-moa/aleph-model-routing-moa-pre-removal-2026-10-19.tar.gz`
- Create: `archive/removed-model-routing-moa/MANIFEST.json`
- Create: `archive/removed-model-routing-moa/SHA256SUMS`
- Create: `archive/removed-model-routing-moa/README.md`
- Reference: `docs/superpowers/specs/2026-10-19-model-routing-moa-removal-design.md`

**Interfaces:**
- Produces `MANIFEST.json` entries with original relative path, archive path, category, SHA-256, snapshot kind, deletion reason, and production/test classification.
- Produces a fixed-point source set from deleted module/type/function/tool/event symbols, Rust references, serde tags, string registries, macro/feature-gated references, shared wire references, MCP/ACP census hits, and unresolved dynamic references.
- The archive helper must refuse to run with a dirty worktree and must exit non-zero if any unresolved dynamic reference lacks a manifest decision.

- [ ] **Step 1: Add failing archive-helper tests** for dirty worktree refusal, deterministic manifest ordering, preservation of original paths, SHA mismatch detection, and unresolved dynamic-reference reporting. Put tests next to the helper using Python `unittest`; do not invoke Cargo.
- [ ] **Step 2: Run `python3 -m unittest scripts/archive_model_routing_moa.py`** and verify the new tests fail because the helper does not exist.
- [ ] **Step 3: Implement the stdlib-only helper** using `pathlib`, `subprocess`, `tarfile`, `hashlib`, and JSON. Capture `git rev-parse HEAD`, branch, repository root, Cargo.lock hash, and tool version. Recursively search the fixed symbol seed set and write the dynamic-reference review list before creating the archive.
- [ ] **Step 4: Run the helper against the clean pre-removal tree** and verify the tarball, manifest, checksum file, and README are created under `archive/removed-model-routing-moa/`; extract to a temporary directory and compare every manifest hash.
- [ ] **Step 5: Run `python3 -m unittest scripts/archive_model_routing_moa.py`** and `git diff --check`; expected: all helper tests pass and no whitespace errors.
- [ ] **Step 6: Commit archive and helper** with `git add scripts/archive_model_routing_moa.py archive/removed-model-routing-moa` and message `chore: archive model routing and moa before removal`.

### Task 2: Remove MoA provider, configuration, builtin, and gateway execution

**Files:**
- Delete: `src/providers/moa/*.rs`
- Delete: `src/providers/session_moa_handle.rs`
- Delete: `src/config/types/moa.rs`
- Delete: `src/builtin_tools/moa_manage.rs`
- Delete: `src/gateway/handlers/moa.rs`
- Delete: `interfaces/webchat/src/api/moa.rs`
- Delete: `interfaces/webchat/src/platform/wide/views/settings/moa/mod.rs`
- Delete: `interfaces/webchat/src/platform/wide/views/settings/moa/options.rs`
- Delete: `interfaces/webchat/src/platform/wide/views/settings/moa/preset_editor.rs`
- Modify: `src/providers/mod.rs`, `src/config/structs.rs`, `src/config/types/mod.rs`, `src/config/types/tools.rs`, `src/config/patcher.rs`
- Modify: `src/builtin_tools/mod.rs`, `src/executor/builtin_registry/{definitions.rs,groups.rs}`
- Modify: `src/executor/builtin_registry/builder/{core_tools.rs,constructor/mod.rs}`
- Modify: `src/executor/builtin_registry/registry/{inherent.rs,struct_def.rs,tool_registry_impl.rs}`
- Modify: `src/bin/aleph-server/commands/start/{mod.rs,builder/handlers/settings.rs}`
- Modify: `src/gateway/{lane.rs,method_authz.rs,method_census.rs,rate_limiter.rs}`
- Modify: `src/gateway/execution_engine/{execute.rs,slash_command.rs,steering.rs}`
- Modify: `src/orchestrator/harness_bridge/runner_impl.rs`

**Interfaces:**
- Removes the `moa` builtin, `moa.*` JSON-RPC methods, `/moa` intercept, `[moa]` typed configuration, session MoA activation, and `MoaProvider` construction.
- Leaves ordinary `select_model`, `list_models`, provider catalog, and the existing default/failover provider chain available.

- [ ] **Step 1: Add/adjust negative tests** asserting builtin registry no longer contains `moa`, gateway method census no longer registers `moa.*`, slash dispatch treats `/moa` as removed/unknown according to existing conventions, and `select_model` rejects `moa`/`moa:<preset>` without changing ordinary model behavior.
- [ ] **Step 2: Run targeted core tests** separately, with one filter per invocation:
  - `cargo test -p alephcore --lib builtin_tools::select_model`
  - `cargo test -p alephcore --lib gateway::method_census`
  Record the expected failures from the existing MoA assertions.
- [ ] **Step 3: Remove module declarations and constructor/registry wiring**, then delete MoA config fields and patcher branches. Keep config deserialization tolerant of an unknown `[moa]` section; add a raw-config diagnostic hook without importing `MoaToml`.
- [ ] **Step 4: Remove gateway execution branches and session activation** from `execute.rs`, `slash_command.rs`, `steering.rs`, and `runner_impl.rs`; ensure a normal message containing `/moa` follows the existing unknown/LLM pass-through convention and cannot arm hidden state.
- [ ] **Step 5: Remove the MoA-specific tests and add the negative/ordinary-model tests from Step 1.** Do not delete shared historical trace variants; those belong to Task 4.
- [ ] **Step 6: Run `cargo fmt --check`, `cargo test -p alephcore --lib builtin_tools::select_model`, and `cargo check -p alephcore`.** Expected: no MoA execution symbols in the core build and ordinary model selection tests pass.
- [ ] **Step 7: Commit** with `core: remove moa execution and configuration`.

### Task 3: Remove MoA proposal aggregation from subagent tools

**Files:**
- Modify: `src/agents/subagent_tool/loop_tool.rs`
- Modify: `src/agents/subagent_tool/tests.rs`
- Modify: `src/agents/background_tracker.rs` only where `moa-proposal` is a dedicated aggregation identity
- Modify: tool schemas/metadata generated or registered by the above files

**Interfaces:**
- Removes `proposer_models`, `synthesize`, `moa_completed`, `moa_synthesis_failed`, `moa_no_proposals`, and `moa-proposal` aggregation semantics.
- Keeps ordinary `batch_tasks`, explicit parallel children, chains, background work, and result collection under main Agent control.

- [ ] **Step 1: Update tests** so ordinary batch/parallel task behavior remains covered and all proposal/aggregator actions are asserted as unavailable.
- [ ] **Step 2: Run the subagent tool tests** with `cargo test -p alephcore --lib agents::subagent_tool`; verify only the MoA-specific expectations fail.
- [ ] **Step 3: Remove the proposal parsing, aggregator invocation, statuses, synthetic server identity, schema entries, and related test fixtures.** Do not replace them with a deterministic “choose one proposal” fallback; the main Agent owns that decision.
- [ ] **Step 4: Run `cargo test -p alephcore --lib agents::subagent_tool` and `cargo fmt --check`.** Expected: ordinary delegation tests pass and no `proposer_models`/`moa_synthesis` production symbol remains.
- [ ] **Step 5: Commit** with `agents: remove multi-model proposal aggregation`.

### Task 4: Preserve historical trace wire compatibility while removing MoA producers

**Files:**
- Modify: `src/harness/trace.rs`
- Modify: `src/gateway/trace_protocol.rs`
- Modify: `shared/protocol/src/events.rs`
- Modify: `shared/protocol/src/trace_presentation.rs`
- Modify: `shared/protocol/src/trace_replay.rs`
- Modify: `shared/ui_logic/src/transcript/{step.rs,reducer/mod.rs}`
- Modify: `src/gateway/execution_engine/{agent_trace_emit_sink.rs,unattended_redacting_sink.rs}`
- Modify: `src/resilience/database/traces.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/chat/events.rs`
- Modify: `interfaces/webchat/src/locales/{en.json,zh.json}` only for removed live MoA labels
- Test: existing shared protocol and trace tests in the files above

**Interfaces:**
- Historical typed variants remain deserializable and presentable: `MoaAdvisor`, `MoaAggregating`, `MoaAdvisorSpend`, `MoaTurnTrace`.
- No producer, live renderer action, usage aggregation, redaction special case, or trace emission path may create new MoA events.

- [ ] **Step 1: Add regression fixtures** that deserialize each historical event, pass it through trace presentation/replay and transcript reduction, and assert the historical fields survive; add a producer-side test that a normal run emits none of them.
- [ ] **Step 2: Run shared trace/protocol tests** using `cargo test -p aleph-protocol` and `cargo test -p alephcore --lib resilience::database::traces`; verify the producer-side test initially fails while historical decode remains green.
- [ ] **Step 3: Remove live MoA event construction and MoA-only redaction/usage paths** from Core and Gateway, while leaving the shared protocol variants and read-only presentation branches marked deprecated.
- [ ] **Step 4: Remove MoA-only usage aggregation queries and tests** from `src/resilience/database/traces.rs`; keep generic trace querying and historical event decoding.
- [ ] **Step 5: Remove MoA locale strings used only by live controls/status, retaining no live UI references to them.** Historical presentation may use stable generic/deprecated labels if required by the existing shared renderer.
- [ ] **Step 6: Run `cargo test -p aleph-protocol`, `cargo test -p shared-ui-logic`, and the targeted core trace tests.** Expected: old fixtures decode, new runs emit no MoA events, and generic trace queries pass.
- [ ] **Step 7: Commit** with `trace: retain historical moa decoding without live producers`.

### Task 5: Remove routing experience and role-based model hints

**Files:**
- Delete: `src/routing/recall.rs`, `src/routing/experience_store.rs`, `src/routing/observer.rs`
- Delete: `src/memory/store/sqlite/routing_experience.rs`
- Modify: `src/routing/mod.rs` to retain channel/session routing only
- Modify: `src/memory/store/sqlite/mod.rs`
- Modify: `src/memory/store/sqlite/schema/{ddl.rs,mod.rs}`
- Modify: `src/agents/{runtime.rs,loader.rs,types.rs}`
- Modify: `src/agents/subagent_spawner/{mod.rs,tests.rs}`
- Modify: `src/agents/subagent_tool/mod.rs`
- Modify: `src/orchestrator/{dispatch.rs,harness_bridge/mod.rs}`
- Modify: `src/orchestrator/deps_builder/provider_chain.rs`
- Modify: `src/context/compact/cheap_poison.rs`
- Modify: `src/extension/{declared_model.rs,mod.rs}`
- Modify: `src/builtin_tools/agent_manage/info.rs`
- Modify: `src/gateway/execution_engine/run_loop/inner.rs`

**Interfaces:**
- `AgentDef` no longer exposes or serializes Agent model/provider hints as routing policy.
- `subagent` explicit request model/provider remains the only child model override; absent override inherits the current/default provider path.
- `src/routing::{config,identity_links,overlay,resolve,session_key}` remain intact.
- Schema stops creating `routing_experiences`, `routing_exp_vec_map`, and `routing_exp_vec_*`; existing objects are never read, validated, rebuilt, or dropped.

- [ ] **Step 1: Add loader compatibility tests** with legacy frontmatter `model_hint` and `provider_hint`; assert the AgentDef loads without those fields and emits the deprecation diagnostic. Add spawn tests for explicit request model, no override, and no hidden AgentDef fallback.
- [ ] **Step 2: Add SQLite fixtures** for all routing objects present, partial objects, vector extension unavailable, and no objects; assert schema initialization and ordinary memory startup continue without creating, repairing, or querying them.
- [ ] **Step 3: Run targeted tests** separately, with one filter per invocation:
  - `cargo test -p alephcore --lib agents::loader`
  - `cargo test -p alephcore --lib agents::types`
  - `cargo test -p alephcore --lib agents::subagent_spawner`
  - `cargo test -p alephcore --lib routing`
  Verify failures identify the old hint/routing expectations.
- [ ] **Step 4: Remove routing store/recall/observer injection** from runtime, spawner, harness bridge, context compact, and subagent tool; remove only model-routing exports from `src/routing/mod.rs`.
- [ ] **Step 5: Remove `model_hint`/`provider_hint` from AgentDef and loader schema**, preserving tolerant frontmatter parsing and the explicit request model path. Remove per-provider hint override registry and update attribution to actual serving model.
- [ ] **Step 6: Remove routing experience backend and DDL creation statements**. Ensure existing partial objects are ignored by using no access path, not by a new repair migration.
- [ ] **Step 7: Run targeted tests, `cargo fmt --check`, and `cargo check -p alephcore`.** Expected: channel/session routing remains available, no model routing experience is written/read, and explicit child model selection passes.
- [ ] **Step 8: Commit** with `core: remove learned and role-based model routing`.

### Task 6: Simplify explicit model binding and preserve failover semantics

**Files:**
- Modify: `src/orchestrator/harness_bridge/runner_impl.rs`
- Modify: `src/orchestrator/harness_bridge/mod.rs`
- Modify: `src/providers/model_override_provider.rs` only if tests expose an unnecessary policy branch
- Modify: `src/providers/failover/{provider.rs,decision.rs,tests.rs}` only to remove stale `model_hint` comments/tests, not to change failure policy
- Modify: `src/builtin_tools/{select_model.rs,list_models.rs}`
- Modify: `src/session/events.rs`
- Modify: `src/gateway/resume_coordinator.rs`
- Modify: `src/orchestrator/harness_bridge/public_run.rs`

**Interfaces:**
- Explicit model binding is independent from generic lifecycle: the same spawn/join/cancel/collect path must work with the provider default, an explicit model, and provider-configured failover.
- BrainRef and its preset remain available as explicit flow/session/provider-model bindings or defaults; they do not infer from task semantics and do not select a lifecycle strategy.
- `effective_model_directive` resolves only per-turn explicit model, session `select_model` pin, then provider default.
- Explicit `(provider, model)` remains the primary pin; existing `FailureKind`/`Decision` behavior remains authoritative for retry, next model, rate limit, next provider, and stop.
- Resume preserves current `RunStarted` freeze/replay semantics and does not write a fallback as a new session pin.
- Current `verify` acceptance uses existing state, result, and caller-side external checks; it does not add a caller-supplied evidence API. If contract tests prove an expression gap, add only the smallest generic facade and reuse the existing tracker/spawner/recovery state.

- [ ] **Step 1: Pin the precedence, lifecycle, and failover baseline** with tests for per-turn > session pin > default, one generic lifecycle working with default/explicit/failover model selection, explicit provider/model primary, transient retry, model-specific next-model, provider failover, permanent stop, actual serving-model attribution, cancellation, join/collect, and resume after fallback. Verify `verify` through existing state/result observations and caller-side checks; do not add a new evidence API.
- [ ] **Step 2: Run the pre-change baseline separately, with one filter per invocation:
  - `cargo test -p alephcore --lib orchestrator::harness_bridge::runner_impl`
  - `cargo test -p alephcore --lib providers::failover`
  - `cargo test -p alephcore --lib builtin_tools::select_model`
  - `cargo test -p alephcore --lib gateway::resume_coordinator`
  Then run the smallest applicable lifecycle contract target, using one filter per invocation. Record the pre-change baseline and any resource limitation separately.
- [ ] **Step 3: Remove AgentDef hint/MoA/routing branches from `effective_model_directive` and runner construction**, leaving BrainRef/preset explicit bindings or defaults intact, the explicit directive chain, and one `ModelOverrideProvider` wrapper. Keep lifecycle ownership in the existing tracker/spawner/recovery path.
- [ ] **Step 4: Remove `moa` pseudo-model handling from `select_model` and `list_models`;** ordinary model discovery and session pin behavior remain unchanged. Verify that model binding does not select or reshape lifecycle strategy.
- [ ] **Step 5: If the lifecycle contract tests expose a real missing handle/join/collect boundary, make the minimum generic facade change.** Otherwise document that existing APIs satisfy the contract. Keep `verify` on existing state/result/caller checks and do not add a caller-supplied evidence API. Update stale comments and test fixtures without changing `FailoverProvider` decision semantics. Use actual serving provider/model for attribution and preserve the original directive separately where the current event contract supports it.
- [ ] **Step 6: Run the baseline commands from Step 2 again, one command per filter, then run `cargo test -p alephcore --lib orchestrator::harness_bridge::public_run` and the applicable lifecycle contract tests with one filter per invocation.** Expected: all baseline behavior passes, generic lifecycle is independent of model strategy, `verify` remains expressible through existing observations and caller checks, and no strategy-level model selection remains.
- [ ] **Step 7: Commit** with `orchestrator: simplify explicit model binding`.

### Task 7: Remove model strategy UI/API and update Panel/chat/TUI

**Files:**
- Delete: `interfaces/webchat/src/api/moa.rs`
- Delete: `interfaces/webchat/src/platform/wide/views/settings/moa/{mod.rs,options.rs,preset_editor.rs}`
- Modify: `interfaces/webchat/src/app.rs`
- Modify: `interfaces/webchat/src/api.rs`
- Modify: `interfaces/webchat/src/components/{settings_sidebar.rs,model_picker.rs}`
- Modify: `interfaces/webchat/src/platform/wide/views/settings/{mod.rs,routing_rules.rs,providers/{list.rs,picker.rs,model_ladder.rs}}`
- Modify: `interfaces/webchat/src/platform/wide/views/chat/state/mod.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/agents/{mod.rs,overview.rs,detail.rs}` and agent info views as found by the census
- Modify: `interfaces/webchat/locales/{en.json,zh.json}`
- Modify: `interfaces/tui/src/tui/{app/mod.rs,commands.rs,slash.rs,widgets/provider_picker.rs,widgets/status_bar.rs}` and affected tests
- Modify: `src/gateway/handlers/{route_config.rs,providers/handlers.rs}` only for removed strategy endpoints
- Modify: `interfaces/webchat/src/api/routing.rs` and routing handler only if the census confirms they are exclusively provider/model rules
- Preserve: provider catalog, ordinary model picker, `select_model`, channel/session route UI if any separate surface exists

**Interfaces:**
- Panel/chat/TUI expose only a normal configured provider/model picker and the existing explicit `select_model` path.
- No MoA, debate, review, swarm, pipeline, model ladder, strategy, or routing-rule collaboration entry is added.
- Ordinary model selection remains independent from generic subagent lifecycle and uses the same `select_model` -> `effective_model_directive` path.
- Deleted RPC/CLI/tool surfaces return existing unknown/removed errors; they do not return successful no-op responses.

- [ ] **Step 1: Add UI tests** for configured provider/model-only picker output, no `moa` pseudo-provider, ordinary picker selection invoking `select_model`, no MoA/routing/strategy tabs, and no AgentDef hint fields. Add API tests for removed methods using the existing unknown-method convention.
- [ ] **Step 2: Run the focused Panel and TUI tests** before implementation and record existing failures caused by current MoA/routing expectations.
- [ ] **Step 3: Remove Panel MoA page/API/sidebar/app route, `save_traces` control, MoA locales and live trace labels.** Keep generic provider/model catalog UI and do not add a collaboration-strategy picker.
- [ ] **Step 4: Remove the provider/model `routing_rules` settings page and API wiring;** do not remove the distinct channel/session routing implementation under `src/routing/`.
- [ ] **Step 5: Simplify chat selector/state and TUI picker/status** to show configured models, explicit pin, and actual serving model only. Remove role defaults, MoA presets, model-ladder strategy labels, and any new strategy entry point.
- [ ] **Step 6: Run `cargo test -p aleph-panel --lib`, the focused TUI test target, and `cargo fmt --check`.** Expected: Panel and TUI compile without MoA/routing settings and ordinary model selection remains functional.
- [ ] **Step 7: Commit** with `ui: remove model routing and moa settings`.

### Task 8: Add compatibility diagnostics and update documentation/census

**Files:**
- Modify: config raw-load/diagnostic module identified by the existing config loader
- Modify: `src/agents/loader.rs` diagnostic path
- Modify: `docs/reference/AGENT_SYSTEM.md`
- Modify: `docs/reference/MODEL_CATALOG.md`
- Modify: `docs/reference/AGENT_DESIGN_PHILOSOPHY.md`
- Modify: relevant provider/agent guides and stale model-router references found by census
- Modify: `docs/superpowers/specs/2026-07-05-moa-continuous-advisory-port-design.md` and superseded routing docs only to mark them historical, not to rewrite history
- Test: config/loader diagnostics and documentation census tests

**Interfaces:**
- Old `[moa]` is detected from raw config and produces one clear deprecated-feature diagnostic without constructing MoA types or writing config.
- Old Agent frontmatter hints are ignored and produce a clear deprecated-field diagnostic without blocking load.
- Documentation states that model choice, task decomposition, and collaboration strategy belong to the LLM/主 Agent or Skill; the Runtime retains only generic lifecycle/reliability capabilities and explicit provider/model binding.
- MoA, debate, review, and pipeline are documented as dynamic caller composition, not as dedicated Runtime APIs or automatic policy routes.

- [ ] **Step 1: Add config and loader diagnostic fixtures** for one old `[moa]` section, one old Agent frontmatter, both together, and clean config; assert no config rewrite and no MoA type construction.
- [ ] **Step 2: Run the focused diagnostic tests** and verify old fixtures fail before the compatibility hooks exist.
- [ ] **Step 3: Implement one-shot diagnostics** using the existing config/loader diagnostic channel; do not add an auto-migration that edits user files.
- [ ] **Step 4: Update reference docs** to remove the obsolete model router description, document the explicit model binding/failover boundary, and state the generic Runtime/LLM/Skill boundary. Mark historical MoA design docs superseded.
- [ ] **Step 5: Run documentation/i18n census and focused tests**; expected: no stale live product references, while historical docs remain clearly marked.
- [ ] **Step 6: Commit** with `docs: document explicit model selection after routing removal`.

### Task 9: Full verification, archive proof, and final review

**Files:**
- Modify: no production files unless a verification failure identifies a concrete missing reference
- Verify: `archive/removed-model-routing-moa/*`, all workspace crates and affected test targets

**Interfaces:**
- Final proof must demonstrate the archive can restore every deleted source file and that the production tree has no live MoA/strategy routing path.
- Generic lifecycle contract proof must cover spawn, join, collect, and cancel; success, failure, timeout, cancellation, and recovery; request ownership; and tool/MCP/worktree isolation boundaries.
- BrainRef and its preset remain in the retained surface as explicit flow/session/provider-model bindings or defaults. The final census must distinguish these explicit bindings/defaults from deleted AgentDef hints, learned routing, and semantic strategy routing.
- `verify` is accepted through existing task state, result, and caller-side external checks. It may use caller-provided evidence and validation rules at that boundary, but Runtime adds no evidence API, aggregation, voting, best-answer selection, or automatic model re-selection. A facade is conditional on contract tests proving an expression gap.

- [ ] **Step 1: Run archive verification**: checksum files, extract archive to a temporary directory, compare every manifest entry, and verify the dynamic-reference decision list is complete.
- [ ] **Step 2: Run production census** with `rg` for `MoaProvider`, `MoaManageTool`, `session_moa_handle`, `RoutingRecall`, `RoutingExperienceStore`, `OutcomeObserver`, `routing_experience`, `model_hint`, `provider_hint`, `proposer_models`, `moa_synthesis`, `moa.*`, and `protocol == "moa"`; manually classify historical compatibility, explicit BrainRef/preset bindings or defaults, and unrelated provider-config matches.
- [ ] **Step 3: Run lifecycle and boundary contract checks** for spawn/join/collect/cancel, success/failure/timeout/cancellation/recovery, request ownership, tool/MCP/worktree isolation, permissions, concurrency, and failure visibility. Verify through existing task state, result, and caller-side external checks; do not add an evidence API or facade unless these tests prove the existing contract cannot express the check. There must be no Runtime aggregation, voting, or model re-selection.
- [ ] **Step 4: Run the minimum trusted validation set from `docs/reference/DEVELOPMENT.md`** as separate executable commands or explicit single-target filters: Core lib/bins compile and tests, the `test-helpers` integration target, Panel lib/no-run, `just test-shared`, desktop checks required by the change, and workspace all-target clippy. Run `cargo check --tests` as its own separate check because the earlier attempt ended in resource timeout/SIGKILL; report its actual result independently.
- [ ] **Step 5: Run focused runtime smoke tests** for default model, explicit session model, explicit subagent model, failover, cancel, join/collect, resume, old `[moa]` config, old Agent frontmatter, old trace replay, and partial routing tables.
- [ ] **Step 6: Run `git diff --check`, `cargo fmt --check`, and final `git status --short`;** expected: all pass, archive is tracked, no generated build output or user data is included.
- [ ] **Step 7: Have an independent reviewer inspect the complete diff and archive manifest** for omitted dynamic registrations, accidental deletion of channel/session routing, changed failover semantics, UI no-op paths, and accidental reintroduction of fixed collaboration strategy APIs.
- [ ] **Step 8: Commit any concrete verification fixes separately** with a message naming the defect; do not squash away archive or migration evidence.

## Execution Order and Handoff

Tasks 1–6 are sequential because the archive must precede deletion and the binding contract must be simplified before UI selectors are changed. Task 7 depends on Tasks 2, 5, and 6. Task 8 depends on the final compatibility surface from Tasks 2 and 5. Task 9 runs only after Tasks 1–8 are complete.

Builder check is complete and has no important unresolved issues. The plan records the known `cargo check --tests` resource timeout/SIGKILL limitation; that check remains an independent validation result. The plan is intentionally implementation-ready but does not authorize pushing, releasing, or deploying. The archive is a tracked historical asset; it is not added to the workspace and is never compiled.
