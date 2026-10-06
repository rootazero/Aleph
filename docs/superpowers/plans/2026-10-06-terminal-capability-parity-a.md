# Terminal Capability Parity A — Callable Convergence & TUI Observation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将现有只读 terminal/PTY/runtime 串成 canonical callable → typed client → 可重连 TUI screen / Make observation usable end-to-end.

**Architecture:** 不搬迁现有状态 map；小型 `TerminalRuntime<'a>` 借用 `PtyManager`/`RuntimeAgents`。注册同代 descriptor/handler，旧 faces 仅适配；客户端缓存只应用已排序的 server screen / Borrow stores, capture generations, project ordered screens.

**Tech Stack:** 见总计划，Rust/Tokio/serde、现有 ratatui/vte；无新运行时依赖。

**Spec:** `docs/superpowers/specs/2026-10-06-terminal-capability-parity-design.md`。

## Global Constraints

继承 `docs/superpowers/plans/2026-10-06-terminal-capability-parity.md` 的全部硬约束、paths、CG、资源租约。所有下列接口为拟新增/修改契约，不是假称当前存在 / Interfaces below are planned contracts.

## Review Focus

同代 race、approval rewrite、actor-less caller 差异、seq/attach race、wide-cell 渲染；具体测试在 A2–A6。

---

### A0: Guarded baseline and executable census / 资源闸与基线

**Files:** Create `qa/terminal/memory_guard.py`, `qa/terminal/test_memory_guard.py`; Modify 仅必要的 census tests `src/gateway/method_census.rs`；record `docs/audits/2026-10-06-terminal-parity-census.md`。

**Interfaces:** Python `parse_meminfo(text: str) -> int | None`; `available_memory_bytes() -> int | None`; CLI `python qa/terminal/memory_guard.py -- <command> [args...]` waits below `4_294_967_296`, probe unavailable exits nonzero before child spawn. `CG` 是临时 Cargo shim，不提交绝对机器路径。

- [ ] **Red:** unittest `test_missing_memavailable_is_unknown`, `test_kib_to_bytes`, `test_probe_failure_never_spawns`。
  ```python
  assert parse_meminfo('MemAvailable: 4194304 kB\n') == 4_294_967_296
  assert parse_meminfo('MemFree: 99999999 kB\n') is None
  # Mock available_memory_bytes() -> None; assert child-run mock call_count == 0.
  ```
- [ ] **Verify red:** `python -m unittest discover -s qa/terminal -p test_memory_guard.py`，Expected missing module/API，不能归为基线故障。
- [ ] **Implement:** Linux 只读 MemAvailable；Windows ctypes GlobalMemoryStatusEx 只读 ullAvailPhys；失败无默认值。保留 Ctrl-C；低内存每 `5s` 重查。Bash 临时 `cargo` shim 或 Windows `cargo.cmd` 均调用 guard 后 exec original absolute Cargo；校验调用 just 的内部 cargo 也经过它。
- [ ] **Green + baseline:** unittest PASS；验证 worktree SHA/git dirs；CG 跑 core lib/bin 及 protocol/tui targeted baseline。任何 baseline FAIL 原样记录并停止实现，询问用户，不先“顺手修复”。census 记录实际 RPC 注册、operator ruling、event、tool projection 的路径和 consumer；检查旧图谱 HEAD，只用作定位不当权威。
- [ ] **Commit:** `qa: add fail-closed terminal build memory guard`，显式 add 两个 QA 文件与 census 文档；不提交测试产物。

### A1: Typed read contract / 只读契约

**Files:** Create `shared/protocol/src/terminal.rs`; Modify `shared/protocol/src/lib.rs`; Test 同 module。

**Interfaces:** 从 `src/builtin_tools/terminal.rs` 移动现有 wait/explain 输出结构，read 将现有 JSON keys 类型化；`&'static str` 改 owned wire String，不改序列化。复用 `RuntimeAgentState`, `PtyAttachResponse`, `PtyListResponse`。
```rust
pub struct TerminalReadResponse { pub session_id: String, pub text: String }
pub struct TerminalWaitResponse { pub session_id: String, pub outcome: TerminalWaitOutcome, pub agent: Option<RuntimeAgentEntry> }
pub enum TerminalWaitOutcome { Reached, Timeout, Gone } // serde snake_case
pub struct TerminalExplainRule { pub id: String, pub priority: i32, pub region: String, pub state: RuntimeAgentState }
pub struct TerminalExplainInputs { pub title: String, pub osc_progress: String, pub screen_tail: String }
pub struct TerminalExplainResponse {
    pub session_id: String, pub agent: Option<String>, pub state: RuntimeAgentState,
    pub matched_rule: Option<TerminalExplainRule>, pub source: Option<String>,
    pub manifest_version: Option<String>, pub reason: Option<String>, pub inputs: TerminalExplainInputs,
}
pub struct TerminalSessionParams { pub session_id: String }
pub struct TerminalWaitParams { pub session_id: String, pub until: Option<Vec<RuntimeAgentState>>, pub timeout_ms: Option<u64> }
```
`reason` retains skip_serializing_if Option::is_none；until omitted defaults `[Blocked, Idle]`, explicit empty rejected；wait timeout default `60_000ms`, clamp `150_000ms`（与现有 foreground budget test 同源）；explain screen tail `12` lines。Runtime may cancel with existing ToolError, not a new wire outcome。

- [ ] **Red:** `terminal_read_schema_preserves_legacy_keys` 对 server fixture 与 typed serialization 的 key-set equality；`wait_state_uses_runtime_schema` 断言 until schema 枚举正好现有四值；旧 JSON round-trip unchanged，内部 owner 不在响应。
- [ ] **Verify red:** `CG test -p aleph-protocol terminal::tests`，Expected 新类型未定义或 equality FAIL。
- [ ] **Implement:** 搬同一字段定义而非重写；`serde(default)` 仅在旧 reader 有已定义语义时加；保留 wait deadline clamping（现有 `wait_window` 测试决定数值），不猜新 timeout。
- [ ] **Green:** 上述 tests PASS，`CG test -p aleph-protocol`；此任务只改变 DTO，未扩 raw write。
- [ ] **Commit:** `protocol: centralize terminal observation contracts`。

### A2: Borrowed runtime and caller admission / 借用 runtime

**Files:** Create `src/gateway/pty/runtime.rs`; Modify `src/gateway/pty/mod.rs`, `src/builtin_tools/terminal.rs`, `src/builtin_tools/terminal/tests.rs`, `src/gateway/handlers/runtime.rs`; Test `src/gateway/pty/runtime.rs`。

**Interfaces:**
```rust
pub(crate) enum ObservationCaller { Gateway { actor: Option<String> }, Tool { actor: Option<String> } }
pub(crate) struct TerminalRuntime<'a> { /* borrows only */ }
impl<'a> TerminalRuntime<'a> {
    pub(crate) fn new(pty: &'a PtyManager, agents: &'a RuntimeAgents) -> Self;
    pub(crate) fn list(&self, caller: &ObservationCaller) -> PtyListResponse;
    pub(crate) fn status(&self, caller: &ObservationCaller) -> RuntimeAgentsListResponse;
    pub(crate) fn attach(&self, caller: &ObservationCaller, session_id: &str) -> Result<PtyAttachResponse, ToolError>;
    pub(crate) fn read(&self, caller: &ObservationCaller, session_id: &str) -> Result<TerminalReadResponse, ToolError>;
    pub(crate) async fn wait(&self, caller: &ObservationCaller, params: &TerminalWaitParams, cancel: CancellationToken) -> Result<TerminalWaitResponse, ToolError>;
    pub(crate) fn explain(&self, caller: &ObservationCaller, session_id: &str) -> Result<TerminalExplainResponse, ToolError>;
}
```
Types import existing `PtyManager`, `RuntimeAgents`, `ToolError`, tokio-util token and A1 DTO. No global second runtime singleton.

- [ ] **Red:** `tool_without_actor_does_not_inherit_gateway_admission` compares existing actor-less tool narrowing and gateway semantics；owner alice/bob on list/addressed/explain/wait；`quiet_does_not_become_idle`；cancel wait completes without busy poll。
- [ ] **Verify red:** `CG test -p alephcore --lib gateway::pty::runtime::tests` Expected new module/API missing。
- [ ] **Implement:** 将 `terminal_admits`/owned read/wait/explain 的共同逻辑抽到 runtime，保留 explicit caller provenance、lost restart output；wait 复用 watch，no lock across await；旧 terminal wrapper 序列化 A1 DTO。删除搬迁的原实现，只留 adapters。
- [ ] **Green:** 新 tests、`CG test -p alephcore --lib builtin_tools::terminal`、`CG test -p alephcore --lib gateway::runtime`；single clock/probe/cwd 已有 guards 不变。
- [ ] **Commit:** `terminal: converge observation over existing runtime stores`。

### A3: Same-generation dispatch and per-call proof / 同代与授权凭据

**Files:** Modify `src/tools/registry.rs`, `src/tools/scoped/dispatch.rs`, `src/tools/scoped/mod.rs`, `src/tools/turn_context.rs`, `src/tools/runtime.rs`, `src/tools/handlers/mod.rs`; Create `src/tools/dispatch_verdict.rs`; test registry/scoped modules。

**Interfaces:** `ToolHandlerRegistry::resolve_entry(&self, name: &str) -> Option<RegistryEntry>` returns one snapshot pair, no separately resolve handler+descriptor. `DispatchVerdict` fields private, constructor private to gate; `matches(&self, call_id: &str, name: &str, identity: &ToolCallIdentity, input: &serde_json::Value, actor: Option<&str>) -> bool`; `current_dispatch_verdict() -> Option<DispatchVerdict>` task-local read-only. Gate binds call_id as well; never serializes proof. Retain exact signature `pub(super) async fn execute_inner(&self, name: &str, input: Value, cancel: CancellationToken) -> (Result<ToolOutput, ToolError>, Option<Value>)`。

- [ ] **Red:** barriers in `replacement_between_gate_and_dispatch_uses_captured_entry`: capture descriptor rev1/handler1, replace rev2, allow in-flight handler1 and next call handler2；assert audited identity matches executed handler. `proof_for_original_input_cannot_authorize_rewrite`, `same_name_parallel_calls_do_not_share_verdict`, `child_task_does_not_inherit_human_or_operator_proof`。
- [ ] **Verify red:** `CG test -p alephcore --lib tools::registry` and `CG test -p alephcore --lib tools::scoped`，Expected capture/matching tests FAIL，no successful dispatch on mismatch。
- [ ] **Implement:** supported builtin/markdown/MCP route uses captured RegistryEntry at final invocation; catalog is not a handler store. Effective input after hooks rechecks operator and confirmation gates and captures effective identity. Do not change role or remove terminal inline gate; invoke scoped proof only around the captured handler. Direct calls without proof remain fail-closed for effects. Capture route state before any retry; terminal external effects use `ReplayPolicy::Unsafe` and must not enter existing one-shot retry helper. Add `unsafe_terminal_write_error_is_not_retried` asserting handler count1 after possible partial write. Preserve unrelated Plugin fallback and record boundary.
- [ ] **Green:** scoped hook-rewrite/approval concurrent tests PASS；existing Safe-replay/boundary-repair tests remain green；no harness changes, proof unavailable outside scope。
- [ ] **Commit:** `tools: bind admission to captured capability generation`。关键 reviewer 拒绝任何旧 map 或二次 resolve workaround。

### A4: Canonical read handlers and RPC compatibility / 注册与连线

**Files:** Create `src/builtin_tools/terminal/capabilities.rs`; Modify `src/builtin_tools/terminal.rs`, `src/executor/builtin_registry/definitions.rs`, `src/bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs`, `src/bin/aleph-server/commands/start/orchestrator_init.rs`, `src/bin/aleph-server/commands/start/builder/handlers/system.rs`, `src/gateway/handlers/pty.rs`, `src/gateway/handlers/runtime.rs`, `src/gateway/handlers/mod.rs`, `src/gateway/method_census.rs`, `src/gateway/method_admin.rs`, `src/gateway/event_scope.rs`, `src/tools/scoped/dispatch.rs`。

**Interfaces:** Existing registration `register(&self, descriptor: ToolCapabilityDescriptor, handler: Arc<dyn ToolHandler>) -> Result<RegistrationHandle, ToolError>`；new `register_observation_capabilities(registry: &ToolHandlerRegistry, scope: &mut ToolRegistrationScope) -> Result<(), ToolError>` registers real handlers backed by A2。RPC adapter `invoke_terminal_projection(request: JsonRpcRequest, canonical_name: &'static str, service: Arc<dyn ToolService>) -> JsonRpcResponse` receives the **scoped** service, not raw registry.

- [ ] **Red:** `rpc_and_tool_resolve_one_terminal_descriptor`, `legacy_terminal_action_uses_same_owner_result`, `alias_does_not_register_second_identity`；table row equal keys from real handler, nonowner denied every addressed face。Missing service returns unavailable, not success/empty table。
- [ ] **Verify red:** `CG test -p alephcore --lib terminal_capability`；`CG test -p alephcore --lib method_census`，Expected unknown projections or bypass detected。
- [ ] **Implement:** real descriptor per implemented verb；read-only legacy `terminal {action}` 是 ingress-only 参数适配，**进入 gate 前**变成 canonical name/input，原 terminal callable descriptor/catalog entry 被移除，不能 adapter 注册成第二 identity。新增纯适配函数 `normalize_terminal_compat_call(name: &str, input: Value) -> Result<(String, Value), ToolError>`；不持有 handler/schema/policy；不能递归调用旧 terminal handler。 literal RPC registration adapters injected at boot; keep event sources existing owner. Compatibility methods resolve same callable, ownership admission remains addressed; resize scoped independently in B4, don't silently migrate input/spawn to unimplemented handlers。
- [ ] **Green:** new tests plus registry/catalog projection-hole tests；`CG test -p alephcore --bins` verifies boot wiring；methods absent from admin census must fail。Record and remove obsolete duplicate read action bodies, not necessary compatibility API。
- [ ] **Commit:** `terminal: project canonical observation into tools and RPC`。

### A5: Typed client and ordered screen cache / 客户端排序

**Files:** Create `shared/client/src/terminal.rs`, `interfaces/tui/src/tui/terminal.rs`; Modify `shared/client/src/lib.rs`, `interfaces/tui/src/tui/mod.rs`; tests in new modules。

**Interfaces:** `AlephClient::terminal_list(&self) -> CliResult<PtyListResponse>`；`terminal_attach(&self, session_id: &str) -> CliResult<PtyAttachResponse>`；`terminal_status(&self) -> CliResult<RuntimeAgentsListResponse>`。Client methods retain gateway error/refusal classification. `TerminalScreenCache` purely wire state；`apply_frame(&mut self, frame: &PtyScreenFrame) -> FrameDisposition`；`FrameDisposition::{Applied, Duplicate, Reattach}`；`begin_attach(&mut self) -> u64` returns request epoch；`complete_attach(&mut self, epoch: u64, snapshot: PtyAttachResponse) -> bool` accepts only latest pending epoch and retains bounded frames observed during attach。

- [ ] **Red:** after snapshot seq10 frame11 Applied, frame11 Duplicate, frame13 Reattach；`old_attach_response_cannot_reset_new_connection`；event seq11 arriving before snapshot10 is replayed once；geometry change with contiguous frame accepted atomically；wrong session rejected。
- [ ] **Verify red:** `CG test -p aleph-tui terminal::tests`；Expected missing screen cache or ordering assertions FAIL。
- [ ] **Implement:** bounded per-session frame queue during attach；overflow demands fresh attach; neither ignoring early frames nor applying gap. On reconnect invalidate outstanding epoch, resubscribe before attach, include runtime refresh；async RPC completion uses event channel rather than blocking TUI 50ms tick。
- [ ] **Green:** cache tests plus `CG test -p aleph-client` and `CG test -p aleph-tui`；disconnect never shown as empty list。
- [ ] **Commit:** `tui: add typed terminal observation and attach repair`。

### A6: Terminal renderer, focus and scrolling / TUI 可达

**Files:** Create `interfaces/tui/src/tui/widgets/terminal.rs`; Modify `interfaces/tui/src/tui/app/mod.rs`, `interfaces/tui/src/tui/app/events.rs`, `interfaces/tui/src/tui/app/tests.rs`, `interfaces/tui/src/tui/event.rs`, `interfaces/tui/src/tui/render.rs`, `interfaces/tui/src/tui/widgets/mod.rs`, `interfaces/tui/src/tui/slash.rs`, `interfaces/tui/src/tui/keys.rs`, `interfaces/tui/src/tui/regions.rs`; Extend `shared/protocol/src/pty.rs`, `src/gateway/pty/screen/grid/mod.rs`, `src/gateway/pty/runtime.rs` only for paged history read。

**Interfaces:** `render_terminal(frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect, cache: &TerminalScreenCache)`；client `TerminalViewState` stores selected session/focus/scroll, not Core layout truth。`TerminalHistoryParams { session_id: String, before: Option<u64>, limit: u16 }`, `TerminalHistoryResponse { rows: Vec<PtyRowPatch>, next_before: Option<u64> }` immutable sequence-indexed history pagination；`limit` `1..=200`，invalid拒绝；no second scrollback store。

- [ ] **Red:** TestBackend buffer assertions for `中🙂`, wide cell clipped at last col, reverse/underline/cursor visibility, alt-screen scroll boundary；`terminal_key_release_does_not_double_input`；session selection invokes actual client attach, not only state set；history page retains row order and bounds。
- [ ] **Verify red:** `CG test -p aleph-tui terminal`；`CG test -p alephcore --lib terminal_history`，Expected no reachable terminal renderer/history API。
- [ ] **Implement:** explicit terminal switch/session picker; ratatui spans from server runs, no VT parse. Current read-only view does not pretend typing is enabled；show locked input until B1 integration. focus escape is local UI, approval keys never forwarded to PTY. Core history callable registered after real implementation, client page fetch async; no megapage allocation。
- [ ] **Green:** TestBackend and app routing PASS；guarded `just test-shared` verifies protocol/client/TUI/CLI siblings；document actual navigation binding in TUI help。
- [ ] **Commit:** `tui: render reachable terminal sessions with bounded history`。

### A7: Panel/CLI shared projection compatibility / 多端不漂移

**Files:** Create `interfaces/cli/src/commands/terminal.rs`; Modify `interfaces/webchat/src/platform/wide/views/terminal/session.rs`, `interfaces/webchat/src/platform/wide/views/terminal/mod.rs`, `interfaces/cli/src/commands/mod.rs`, `interfaces/cli/src/commands/cli_args.rs`, `interfaces/cli/src/lib.rs`; shared client only adapters。

**Interfaces:** CLI observation subcommands use A5 typed client; Panel snapshot/list/runtime retain current DTO exactness, not duplicate schemas. New CLI names `terminal list/read/status/wait/explain` with JSON option consistent with existing CLI serializer.

- [ ] **Red:** protocol fixtures from real server serialization consumed by both clients; `refusal_is_not_no_live_sessions`, `legacy_missing_cwd_does_not_spawn_duplicate`；CLI parser and output golden tests。
- [ ] **Verify red:** `CG test -p aleph-cli terminal`；`CG test -p aleph-panel --lib terminal`，Expected missing CLI routes or error-mapping failure。
- [ ] **Implement:** common client contract and refusal UI; don't expand Panel business logic; old input UI shows authorization requirement pending B1 rather than direct unguarded call. No new main-local code。
- [ ] **Green:** Panel lib actual tests + `just wasm` (guard internal Cargo)；`just test-shared`；terminal TUI live QA with disposable operator connection verifies runtime row and rendered screen came from live events。
- [ ] **Commit:** `interfaces: align terminal observation projections`。
