# Sandbox Execution Boundary Implementation Plan / 沙箱执行边界实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route Team and subagent worktree execution through the existing OS sandbox and report the actual configuration and failure state. / 让两条 worktree 入口共享 OS 沙箱，并准确报告配置与失败。

**Architecture:** One mounted `Sandbox` remains the command authority. One trusted execution-workspace value supplies the root and ownership kind across dispatch, tool calls, and prompt projection; Git creation and cleanup stay separate from process launch. Driver preflight reports known hard dependency failures but does not certify launch.

**Tech Stack:** Rust, Tokio task-local scopes, existing Git worktrees, Seatbelt, bwrap and Windows token/AppContainer drivers. No new dependencies.

**Spec:** [`docs/superpowers/specs/2026-11-25-sandbox-execution-boundary-design.md`](../specs/2026-11-25-sandbox-execution-boundary-design.md).

## Global Constraints / 全局约束

- Work exclusively in `sandbox-capability-refactor`. No edits, merge, or push to `main`.
- Keep `build_sandbox`, `prepare_command`, hooks, approvals, cwd revalidation, rate limits, output scrubbing, and managed child/proxy ownership. No second production override or registry.
- For owned worktrees, OS policy is rooted at the trusted checkout; validated `cwd` is only where the process starts. Set `CARGO_TARGET_DIR=<worktree>/target`. Preserve ordinary-run `cwd` policy, the default deny for writes outside the worktree, and existing explicit grants. Never infer authority from paths supplied to a tool or metadata strings.
- Do not add an unsandboxed retry, broaden permissions, replace platform policy, or claim native process exit from a Rust join. `ALEPH_TEAM_MEMBER_WORKTREE=0` continues to disable automatic Team worktrees. ACP and ordinary host services keep their existing behavior.
- Externally visible operations remain Tool Capabilities via the existing registry and scoped admission. A prompt or debug summary is a declaration, not kernel enforcement evidence.
- Maintain one writer per worktree. The lead integrates Tasks 1–3 in order because they share dispatch contracts; delegate independent test/document work on separate worktrees after its interface settles and obtain fresh final review. Use GPT Sol/Kimi for cross-module integration and review; prefer Flash/Luna for focused mechanical work. No redundant agents for the same changes. / 主线串行改共享契约，独立验证与文档才并行。

> Implementation decision / 实施决定：独立审阅发现，只传路径不能保证取消时的 checkout 寿命。最终 `OwnedWorktree` 持有 `Arc<WorktreeHandle>`；实际执行任务保留租约，活动状态快照和持久数据不持有租约。下方保留原计划及预期受测边界，不是执行状态；实际结果、合并的实施提交和未验证项见[验证记录](../runs/2026-11-25-sandbox-execution-boundary-evidence.md)。

## Review Focus / 审阅重点

| Case / 情况 | Required evidence / 证据 |
|---|---|
| Run from a worktree child directory with a sibling `target` | Task 1 records both inputs and checks `CARGO_TARGET_DIR`; Task 5 checks a real backend. |
| A worktree was removed after scope capture | Task 1 proves no directory recreation, approvals, or driver launch. |
| Team project lookup fails versus no project binding | Task 2 checks different results and no daemon-cwd fallback. |
| Explicit nested worktree has a different HEAD than the project root | Task 2 checks committed contents and that uncommitted changes are not copied. |
| Backend is configured but unavailable; a later native launch can also fail | Task 4 verifies separate refusal categories, with no preflight side effects or unsandboxed retry. |

---

### Task 1: Carry and consume a trusted workspace / 工作区承载与消费

**Files:** `src/sandbox/context.rs`, `src/scope/carried.rs`, `src/sandbox/workspace/mod.rs`, `src/gateway/execution_engine/{mod.rs,run_loop/mod.rs,run_loop/inner.rs}`, `src/orchestrator/dispatch.rs`, `src/builtin_tools/bash_exec.rs`. Update actual carried-workspace call sites identified by `rg -n 'EXEC_WORKSPACE|with_exec_workspace|workspace_override' src`.

**Interfaces:** Define `ExecWorkspace::{Directory(PathBuf), OwnedWorktree(PathBuf)}` in `src/sandbox/context.rs`, with `fn root(&self) -> &Path`, `fn is_owned_worktree(&self) -> bool`, and `fn current_exec_workspace_binding() -> Option<ExecWorkspace>`. Keep `fn current_exec_workspace() -> Option<PathBuf>` as the path-only projection. Add `async fn with_exec_workspace_binding<F: Future>(workspace: Option<ExecWorkspace>, fut: F) -> F::Output`; keep ordinary `with_exec_workspace` as an adapter to `Directory`. Make the trusted run override carry `ExecWorkspace` wherever the run can publish it; project `.root()` for the existing FsScope and path-only consumers.

- [ ] Add `owned_worktree_separates_launch_cwd_from_policy_root`: adapt the recording driver in `src/sandbox/workspace/mod.rs` to observe `profile_for` and `run`; assert `policy_root == root`, `actual_cwd == root.join("src")`, and `env.get("CARGO_TARGET_DIR") == Some(&root.join("target").to_string_lossy().into_owned())`. Include a `Directory` control that still passes child `cwd` to `profile_for` and does not inject the target override.
- [ ] Add `missing_owned_worktree_never_recreated_or_launched`: enter an owned binding then remove its directory, execute with default cwd, and assert failure, `!root.exists()`, no hook/approval effects and no driver launch. Add `carried_attribution_preserves_owned_worktree` capturing and re-entering scope across `tokio::spawn`, checking exact binding equality and restoring the outer scope afterward.
- [ ] Add `owned_worktree_explicit_write_grant_uses_existing_approval`: capture effective policy with and without a request for a sibling temporary directory. Assert absent from baseline and denied with no launch when the existing chain refuses; an accepted explicit request follows the same grant path as `Directory` and does not change the workspace root. Test native refusal separately in Task 5.
- [ ] Run `cargo test -p alephcore --lib owned_worktree` and confirm the new regression tests fail before the contract is wired. Record the red stage (if compilation initially lacks the proposed type, do not count that alone as a behavior regression; rerun with the minimal type seam).
- [ ] Scope the binding where the gateway and orchestrator currently publish workspace paths, including carry/replay paths that previously re-established `EXEC_WORKSPACE`. Preserve absence and ordinary `Directory` distinctly. Do not serialize a worktree ownership tag through user-authored metadata or a user-facing wire field.
- [ ] In `WorkspaceSandbox::prepare_command(&self, SandboxCommand) -> Result<PreparedCommand, SandboxError>`, validate the owned root before any fallback directory creation, retain late cwd validation, pass the owned root rather than launch cwd to `profile_for`, and derive the authoritative target environment value after normal environment assembly. Do not add a writable-path grant to achieve this.
- [ ] Run `cargo test -p alephcore --lib owned_worktree` plus `cargo test -p alephcore --lib sandbox::workspace` and targeted dispatch scope tests; require green. Commit `refactor(sandbox): carry trusted worktree execution scope`.

### Task 2: Wire the two production entrances / 接通生产入口

**Files:** `src/teams/dispatcher/{runner.rs,schedule/authority.rs,schedule/mod.rs}`, `src/agents/subagent_spawner/{mod.rs,tests.rs}`, `src/sandbox/worktree.rs`, `src/gateway/execution_engine/run_loop/{mod.rs,inner.rs}`, `src/orchestrator/{dispatch.rs,harness_bridge/prompt_build.rs}`. Use `src/projects/store.rs::ProjectStore::get` and the existing authorized project workspace resolution; install production dependency plumbing alongside the existing dispatcher constructor if the scheduler does not yet receive it.

**Interfaces:** Consume `ExecWorkspace` from Task 1 and existing `WorktreeHandle` from `worktree::create(&Path, &str, …)`. Team's worktree source resolver consumes the authorized project scope and `&ProjectStore`, produces `Result<Option<PathBuf>, String>` (`None` = confirmed no binding; a stale or unreadable binding is `Err`). `ExecWorkspace::OwnedWorktree` is only published from an actual successful owned handle or inherited from a trusted parent.

- [ ] Add `team_worktree_uses_authorized_project_not_daemon_cwd` and `team_worktree_resolution_does_not_swallow_lookup_failure`: assert selected path is the authorized checkout, no binding or confirmed non-repository executes ordinary sandbox without an owned binding, and invalid project/lookup/Git-create error never runs the member in a fallback root. Use real temporary Git roots when testing Git outcomes. Exercise the disabled/ACP branches without mutating process-global state concurrently.
- [ ] Add `nested_subagent_worktree_uses_trusted_parent_head`: give the parent worktree a distinct committed sentinel and an uncommitted sentinel; assert the child checkout sees only the parent's committed content. Assert explicit isolation without a trusted checkout errors and no-isolation inherits a parent binding. Add `worktree_member_exec_reaches_mounted_driver` and `worktree_subagent_exec_reaches_mounted_driver`: record the selected policy root and launch cwd at the OS driver reached by the real Code Exec route, and compare with file scope and prompt. Prove the same session with two authorized roots does not reuse the first root.
- [ ] Run `cargo test -p alephcore --lib team_worktree`, `cargo test -p alephcore --lib nested_subagent_worktree`, and `cargo test -p alephcore --lib reaches_mounted_driver`; confirm red on the existing producers with the recording test seam.
- [ ] Resolve Team checkout from already-authorized durable scope using `task_fire_facts`/existing fire-time authorization and `ProjectStore`; do not rederive scope from task metadata. Check that the binding is a live directory before treating no `.git` as not-a-repository. Move Git worktree creation to the chosen checkout. Preserve operator-disabled, ACP and true no-binding/no-repository behavior; report genuine lookup/creation failures.
- [ ] In subagent spawning, use parent owned checkout first, then trusted project/execution scope; remove ambient cwd fallback for explicit isolation. Publish the same new/inherited binding alongside existing FsScope. For both entries stop selecting a different executor; derive prompt summary from the mounted sandbox under that same scope.
- [ ] Rerun the three red filters and `cargo test -p alephcore --lib worktree`; require green. Commit `fix(sandbox): wire team and subagent worktrees to the OS boundary`.

### Task 3: Close old paths and bind process lifetime / 清理旧路径与生命周期

**Files:** `src/sandbox/{context.rs,mod.rs,worktree.rs,summary.rs}`, `src/builtin_tools/{bash_exec.rs,code_exec.rs,code_check.rs,browser_tools/companion/tool.rs}`, `src/teams/dispatcher/runner.rs`, and gateway/orchestrator request types and constructors identified by `rg -n 'sandbox_override|WorktreeSandbox|isolated_worktree' src`.

**Interfaces:** All production foreground tools use their mounted `Arc<dyn Sandbox>`; background admission consumes `current_exec_workspace_binding()` before side effects. Team timeout awaits `&mut tokio::task::JoinHandle<_>` under the existing `WORKTREE_CLEANUP_GRACE_MS` budget after abort.

- [ ] Add `owned_worktree_rejects_detached_bash_before_job_creation` and `owned_worktree_rejects_browser_companion_start_before_manager`: assert explicit refusal, zero registered jobs/manager starts and no driver launch. Check ordinary detached Bash still carries session, exec root and call identity, and that a read/status action for an existing companion is not accidentally rejected. Add `team_timeout_waits_for_rust_task_cancellation` using a controlled future whose drop marks cancellation; assert the acknowledged and deadline branches differ without claiming native reap.
- [ ] Run `cargo test -p alephcore --lib owned_worktree_rejects` and `cargo test -p alephcore --lib team_timeout_waits`; confirm red.
- [ ] Gate at Bash detached dispatch and `BrowserCompanionTool::start` before registration, prepare/preflight or spawn. Await Team cancellation within the existing budget, record unconfirmed Rust completion distinctly, then preserve the existing cleanup outcome and partial reply semantics.
- [ ] Delete `WorktreeSandbox`, its direct host spawn and duplicated gating/drain/scrub path, `SANDBOX_OVERRIDE` helpers, the obsolete summary constructor, and request fields/`None` plumbing that only supported that executor. Retain WorktreeHandle, shared helpers with live consumers, and ordinary workspace overrides. Confirm no old production references; inspect each deleted alias by reference count rather than deleting common helpers by filename.
- [ ] Rerun the two red filters, `cargo test -p alephcore --lib worktree`, `cargo test -p alephcore --lib --no-run`, and `cargo check -p alephcore --all-targets`; require green. Commit `fix(sandbox): close legacy host launch and guard worktree lifetime`.

### Task 4: Report typed refusals and truthful posture / 错误与状态如实呈现

**Files:** `src/sandbox/{command.rs,driver.rs,factory.rs,summary.rs,workspace/mod.rs}`, targeted checks in `src/sandbox/platforms/{macos/seatbelt.rs,linux/bwrap.rs,windows/driver.rs}`, `src/builtin_tools/{code_exec.rs,code_check.rs}`, `src/bin/aleph-server/commands/sandbox_debug.rs`, and the existing prompt consumers under `src/{orchestrator/harness_bridge,thinker}`.

**Interfaces:** Extend `SandboxError` with `Disabled` and `BackendUnavailable { platform: &'static str, reason: String }`; retain `UnsupportedPolicy`, `CapabilityDenied`, `ExecutionFailed` and `Timeout`. Add `OsSandboxDriverTrait::execution_preflight(&self) -> Result<(), SandboxError>` for known hard requirements only; keep `managed_spawn_preflight` for the stronger child ownership contract. Add `SandboxPosture::{Configured, Disabled}` to the internal summary; `Disabled` declares neither access tier nor writable roots. Keep backend readiness a distinct typed check, not a probe on every prompt.

- [ ] Add `unavailable_backend_refuses_before_prepare_side_effects`, `disabled_sandbox_rejects_without_claiming_access`, and `summary_root_matches_execution_baseline`: assert typed error and zero approval, hook, proxy and driver-run effects; disabled summary does not present full access or open network; check scoped root, no-run session fallback root, and unknown root with no session. With an available recording driver, separately assert unsupported policy, hook denial, and actual launch error retain distinct errors and reach the tool/debug result.
- [ ] Run `cargo test -p alephcore --lib unavailable_backend`, `cargo test -p alephcore --lib disabled_sandbox`, `cargo test -p alephcore --lib summary_root_matches`; confirm red before behavior changes.
- [ ] Add cheap preflight before `prepare_command` to common execute; use only platform checks already known to decisively indicate unavailability (no launch, proxy, network lookup or new subprocess probe). Keep actual driver enforcement and independent managed preflight. Map explicit policy/hook refusal to `CapabilityDenied`, not generic success or unavailable.
- [ ] Use the same session-root resolver and policy baseline for configured summary and execution. Render unknown root honestly; use typed disabled posture across prompt/debug consumers without probing from summary. Preserve scoped Tool registry behavior and verify the foreground Bash delegation surfaces errors.
- [ ] Rerun red filters, `cargo test -p alephcore --lib sandbox`, and `cargo test -p alephcore --lib operating_envelope`; require green. Commit `fix(sandbox): distinguish execution refusals and configured posture`.

### Task 5: Verify and align current references / 验证、校对、提交

**Files:** `docs/reference/FEATURE_LOCATOR.md` (§3.15; keep §3.8 content-policy distinction), `docs/reference/SANDBOX.md`, and `docs/reference/SUBSYSTEM_ROUTING.md` only if its QA routing changes. Record verification in `docs/superpowers/runs/2026-11-25-sandbox-execution-boundary-evidence.md`.

**Interfaces:** Existing CLI `aleph-server sandbox-debug [--show-summary] [--network none] [-- COMMAND …]`, the real worktree entrypoint tests, and host-specific backend/smoke results. No migration of the reference graph or prompt harness.

- [ ] Run `cargo fmt --all -- --check`, `cargo check -p alephcore --all-targets`, the affected sandbox/worktree/Team/subagent/dispatch/tool tests, and the `docs/reference/DEVELOPMENT.md` Core gates (`cargo test -p alephcore --lib --no-run`, `cargo test -p alephcore --bins`, `cargo test -p alephcore --features test-helpers --test '*' --no-run`). Stage shell placeholders with `just _stage-shell-placeholders` before applicable `cargo clippy --workspace --all-targets`. Keep per-command outcomes, counts and blocked prerequisites; keep unrelated files out of commits.
- [ ] On the current supported backend, use isolated temporary checkouts and harmless commands to verify workspace read/write and local Cargo output, plus refusal of a write to a sibling temporary checkout without approval. Assert actual file effects, no outside sentinel and no fallback launch. Run `sandbox-debug --show-summary` and `sandbox-debug --network none -- /usr/bin/true` on macOS as separate ordinary-backend diagnostics (not proof of the Team/subagent wiring). Collect real Linux/Windows evidence only when their environment is available; label compilation, mocks and unrun tests separately.
- [ ] Update current references against actual production mounts and error behavior; remove macOS-only and git-only executor descriptions and outdated pipeline/root claims. Before documenting any guarantee, identify the enforcing branch and its test. Record no global native process-reap guarantee, no untested platform parity, and no benchmark-derived performance claim.
- [ ] Request fresh review of branch diff with GPT Sol/Kimi (independent reader; use artifacts for test/document checks). Inspect effective transitions and physical effects, not only method calls; resolve blockers and rerun implicated checks. Run `git diff --check` and confirm no old production symbols, only approved branch edits, `main` untouched.
- [ ] Commit references and evidence as `docs(sandbox): align execution boundary and verification`; report short SHAs, test evidence, skipped validation and non-goals. Do not merge or push.
