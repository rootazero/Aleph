# Terminal Capability Parity C — Logical Workspaces, Recovery & Closure

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将多 workspace/tab/BSP/worktree 和恢复做成 Core 真相，并用真实效果 QA 闭合 parity / Core-owned organization and honest recovery.

**Architecture:** 引用现有 AgentEnv workspace identity 与 sandbox/worktree owner；逻辑树存到现有 SessionEventStore，运行态 projection 不另开数据库。reconcile 是事实分类；recreate 是重新授权的新效果 / Reuse identity and durability, never silently replay.

**Tech Stack:** Rust/Tokio/serde、SessionEventStore/SQLite、existing sandbox/worktree；TUI ratatui 绘制；no new dependencies。

**Spec:** `docs/superpowers/specs/2026-10-06-terminal-capability-parity-design.md` §3.3–§3.5、§7–§11。

## Global Constraints

继承总计划全部约束。全量目标不允许用一个空的 workspace/layout/handoff RPC 宣称完成。待验证的跨重启 handoff/额外 agent hooks 必须保留 OPEN / Unverified capabilities remain open.

## Review Focus

多 workspace ownership、layout optimistic revision、worktree Drop 清理、store commit failure、PID 重用与 restart unknown。C4 handoff probe 是单独受审阅的 feasibility spike，不擅自扩张至 broker。

---

### C1: Workspace identity and logical BSP reducer / 布局真相

**Files:** Create `shared/protocol/src/terminal_layout.rs`, `src/gateway/pty/layout.rs`; Modify `shared/protocol/src/lib.rs`, `src/gateway/pty/mod.rs`, `src/gateway/agent_env/ops.rs`, `src/gateway/agent_env/mod.rs`, `src/builtin_tools/terminal/capabilities.rs`; tests in layout/protocol。

**Interfaces:**
```rust
pub enum TerminalSplitAxis { Horizontal, Vertical }
pub enum TerminalPaneNode {
    Leaf { pane_id: String, session_id: Option<String> },
    Split { axis: TerminalSplitAxis, ratio_milli: u16, first: Box<TerminalPaneNode>, second: Box<TerminalPaneNode> },
}
pub struct TerminalTab { pub tab_id: String, pub label: String, pub root: TerminalPaneNode, pub active_pane_id: String }
pub struct TerminalLayout { pub workspace_id: String, pub revision: u64, pub tabs: Vec<TerminalTab>, pub active_tab_id: Option<String> }
pub struct TerminalLayoutApplyParams { pub workspace_id: String, pub expected_revision: u64, pub mutation: TerminalLayoutMutation }
pub enum TerminalLayoutOutcome { Applied { layout: TerminalLayout }, Conflict { current_revision: u64 } }
```
Mutation enum fields exact per action: `CreateTab { label: String }`, `SelectTab { tab_id: String }`, `CloseTab { tab_id: String }`, `Split { pane_id: String, axis: TerminalSplitAxis, ratio_milli: u16 }`, `Focus { pane_id: String }`, `ResizeSplit { pane_id: String, ratio_milli: u16 }`, `ClosePane { pane_id: String }`, `BindSession { pane_id: String, session_id: String }`, `UnbindSession { pane_id: String }`。Layout mutations never implicitly kill child or launch worktree. `apply_layout(caller: &ObservationCaller, params: TerminalLayoutApplyParams) -> Result<TerminalLayoutOutcome, ToolError>` references actual workspace visibility through existing `agent_env::ops`, uses existing session owner for binding。

- [ ] **Red:** multi-workspace alice/bob visible set exact, archived workspace mutation refused；duplicate ids/invalid active pane/foreign session binding rejected；ratio input `100..=900` or Refused (not silently clamp arbitrary); depth >16, tabs >32, leaves >64 refused before allocation amplification；closing leaf collapses sibling tree, no PTY close; simultaneous expected revision1 mutations -> exactly one Applied, one Conflict。
- [ ] **Verify red:** `CG test -p aleph-protocol terminal_layout` and `CG test -p alephcore --lib terminal_layout` Expected absent tree/reducer。
- [ ] **Implement:** reducer validates full invariants before swap; two independent valid workspace trees from first version；stable opaque ids from existing UUID mechanism. New descriptors `terminal_layout_get/apply`; mutation policy/approval per verb data, no hidden subprocess. Core event `terminal.layout.changed` carries workspace+revision, filtered by same owner; resource emission after committed mutation only。
- [ ] **Green:** unit/property-style exhaustive small tree tests without new property-test dep；wire key/schema equality；existing workspace tool/RPC owner/archived contracts PASS unchanged。
- [ ] **Commit:** `terminal: add Core-owned logical workspace layouts`。

### C2: Explicit worktree binding / Worktree 复用

**Files:** Modify `src/sandbox/worktree.rs`, `src/gateway/pty/layout.rs`, `src/builtin_tools/terminal/capabilities.rs`, `shared/protocol/src/terminal_layout.rs`; tests temporary Git repos。

**Interfaces:** `TerminalWorktreeOpenParams { workspace_id: String, branch: String }` existing workspace-approved repo root only；`TerminalWorktreeCloseParams { workspace_id: String, worktree_id: String }`；`TerminalWorktreeBinding { worktree_id: String, workspace_id: String, branch: String, path: String }` owned resource. Store holds actual `WorktreeHandle`, not clone of path; retain through terminal sessions. Existing `WorktreeHandle::cleanup(self) -> Result<(), WorktreeError>` remains explicit; close outcome distinguishes refusal/cleanup requested/observed directory removed。Current `create(repo_root: &Path, label: &str, trace_sink: Option<Arc<dyn TraceSink>>) -> Result<WorktreeHandle, WorktreeError>` always `--detach HEAD` in temp and cleans on Drop, so it cannot silently satisfy durable named-branch binding. Extend the same owner with `create_managed(repo_root: &Path, branch: &str, workspace_root: &Path, trace_sink: Option<Arc<dyn TraceSink>>) -> Result<WorktreeHandle, WorktreeError>` and `WorktreeCleanup::{OnDrop,Explicit}`; existing create preserves OnDrop, managed creation uses Explicit. Shared private git effect implementation, no second lifecycle.

- [ ] **Red:** owner/command policy denied -> git invocation count0；root/symlink/branch path injection rejected；Drop after binding doesn't remove active tree；close bound live PTY -> refusal until explicit lifecycle close observed；cleanup error not success；non-git workspace -> typed refusal no orphan dir。
- [ ] **Verify red:** `CG test -p alephcore --lib terminal_worktree` Expected no binding/handle retention behavior。
- [ ] **Implement:** extend existing `sandbox::worktree` owner as above and hold returned handle in workspace-owned resource; no raw git wrapper in terminal UI. Managed path derives only from configured root and opaque worktree id; branch passes `git check-ref-format --branch` under command policy. Create **new** requested branch; existing branch yields explicit conflict, no implicit checkout/switch. Persist binding before releasing owned handle; Explicit cleanup survives server Drop/restart, no mem::forget workaround. Explicit callable open/close, command policy on effective git args/cwd; durable intent before create, reconcile existing root on unknown outcome not duplicate worktree. Do not alter unrelated subagent sandbox isolation; terminal/session reference count determines allowed cleanup, not harness policy。
- [ ] **Green:** real temporary Git repository assertions worktree exists while bound and disappears only after approved cleanup; existing sandbox/worktree tests remain green. No user repo branch deletion or destructive worktree cleanup。
- [ ] **Commit:** `terminal: bind existing worktree lifecycle to workspaces`。

### C3: TUI BSP, tabs and complete navigation / 交互布局

**Files:** Modify `shared/client/src/terminal.rs`, `interfaces/tui/src/tui/terminal.rs`, `interfaces/tui/src/tui/widgets/terminal.rs`, `interfaces/tui/src/tui/render.rs`, `interfaces/tui/src/tui/event.rs`, `interfaces/tui/src/tui/regions.rs`, `interfaces/tui/src/tui/app/events.rs`, `interfaces/tui/src/tui/app/tests.rs`, `interfaces/tui/src/tui/keys.rs`, `interfaces/webchat/src/platform/wide/views/terminal/tabs.rs`, `interfaces/cli/src/commands/terminal.rs`。

**Interfaces:** client `terminal_layout_get(&self, workspace_id: &str) -> CliResult<TerminalLayout>`；`terminal_layout_apply(&self, params: &TerminalLayoutApplyParams) -> CliResult<TerminalLayoutOutcome>`。Pure render `pane_rectangles(layout: &TerminalLayout, area: Rect) -> Vec<(String, Rect)>` uses BSP only for geometry; drag commits milli ratio, server is authoritative. Local focus for typing selection reconciles server active pane, cannot silently become persisted truth。

- [ ] **Red:** TestBackend nested H/V split produces no overlap and no negative/overflow on tiny viewport；ratio drag sends expected revision, conflict refreshes no overwrite；click different pane routes one input only to its granted session；closing tab never sends PTY kill; resize measured rectangle updates only corresponding session once per changed dimensions; workspace switch preserves each server tree。
- [ ] **Verify red:** `CG test -p aleph-tui terminal_layout` Expected single-view implementation lacks projection。
- [ ] **Implement:** keyboard split/focus/tab/workspace actions and divider drag; derive dimensions/ratios in one geometry function. Resize async coalesces UI events without another flush clock; reject zero-size PTY resize locally and send valid min one. Render active pane/session labels/agent state/locked grant; Panel uses same logical tree, CLI layouts JSON same DTO. Terminal mouse forwarding only when explicit input grant and VT mouse mode demand it; separator drag is UI not PTY bytes。
- [ ] **Green:** TUI/app tests, Panel lib and wasm, guarded shared suite; live TUI multiple fake agents + splitting/drag proves server layout revision changed and PTY sizes match each leaf, not only local panes drawn。
- [ ] **Commit:** `tui: project multi-workspace tabs and BSP interaction`。

### C4: Durability, reconcile and honest process handoff / 持久化恢复

**Files:** Create `src/gateway/pty/recovery.rs`; Modify `src/session/events.rs`, `src/session/store.rs`, `src/session/projection.rs`, `src/session/boundary_repair.rs`, `src/gateway/resume_coordinator.rs`, `src/gateway/pty/layout.rs`, `src/gateway/pty/manager.rs`, `src/builtin_tools/process_journal.rs`, `src/builtin_tools/terminal/capabilities.rs`, `shared/protocol/src/terminal.rs`, `shared/protocol/src/terminal_layout.rs`; QA add `qa/terminal/probe_handoff.py` (throwaway probe, no product broker)。

**Interfaces:** add serializable SessionEvent payloads for `TerminalLayoutCommitted { workspace_id: String, revision: u64, layout: TerminalLayout }`, `TerminalSessionIntent { session_id: String, workspace_id: String, actor: Option<String>, call_id: String, descriptor: ToolCallIdentity, profile: String, created_at_ms: i64 }`, and `TerminalSessionOutcome { session_id: String, call_id: String, state: TerminalRecoveryState, pid: Option<u32>, process_start_id: Option<String>, observed_at_ms: i64 }` using same existing durable store and schema version discipline; mandatory effect intent uses existing ToolCallRequested/effective-input/approved events when dispatcher already committed them, never duplicate same fact. `TerminalRecoveryState::{Live,LostWithRestart,Unknown,DescriptorMismatch,ReplayRefused}`；`TerminalRecoveryRow { session_id: String, workspace_id: String, state: TerminalRecoveryState, next_actions: Vec<TerminalRecoveryAction> }`；actions `Inspect, Recreate, Abandon` only, never auto-repeat input. `reconcile_terminal_state(store: &dyn SessionEventStore) -> Result<Vec<TerminalRecoveryRow>, ToolError>` read/validate only；`recreate_terminal(params: &TerminalSessionParams) -> Result<TerminalRecoveryRow, ToolError>` is new admitted spawn with new session id, preserving old tombstone.

- [ ] **Red:** storage unavailable or commit failure -> no first effect bytes and no layout revision publication；legacy event JSON decode remains unchanged；restoring live-map absence yields LostWithRestart not completed；PID reused by different process -> Unknown；descriptor revision mismatch -> DescriptorMismatch；two consumers cannot dispatch the same recorded approval/call twice; distinct fresh approved calls are explicitly different effects, not an exactly-once claim; explicit abandon doesn't kill unidentified PID。
- [ ] **Verify red:** `CG test -p alephcore --lib terminal_recovery` plus session store/boundary tests Expected missing terminal projection/classification。
- [ ] **Implement:** fold durable layout+intents into existing projection, integrate terminal inspect rows into ResumeCoordinator without selecting recovery strategy. Atomic layout revision commit before in-memory publish; optional process journal corroborates PID/start identity, never authorizes effect alone. Owner survives restart, raw bytes/grants/fds never durable. Unknown outcome requires explicit fresh call; no unsafe scoped retry. Extend strict legacy/unknown event decode tests and durability classification exhaustively; no event variant silently ignored.
- [ ] **Handoff probe:** separately reproduce portable-pty Unix master lifetime and Windows ConPTY child/screen access across actual server exit+new server process, not mere WebSocket reconnect; record PID **and process start identity**, stdout continuity, input reachability, owner invariants. Probe question: can current platform bridge preserve original process without permanent second broker? Any probe code remains QA-only. Failure -> mark full live-handoff BLOCKED/OPEN and request separate design approval before new daemon/service/platform API; do not relabel recreate as handoff.
- [ ] **Green:** durable classification tests + disposable home restart QA reconstructs layout and shows honest tombstones; approved recreate creates new PID/session and operator-visible lineage. If live handoff probe succeeds, add product adapter only after bounded/architectural design review at proper gate and retain this task OPEN until verified; no assumed platform success.
- [ ] **Commit:** `terminal: persist layouts and classify restart recovery`。No full-parity completion claim from this commit alone。

### C5: Reference acceptance and end-to-end effects / 验收闭环

**Files:** Modify `qa/terminal/drive_terminal.mjs`, `qa/terminal/drive_tui.py`, `qa/terminal/run.sh`; Create `qa/terminal/README.md`, `qa/terminal/drive_capabilities.mjs`, `qa/terminal/fixtures/` vetted fixtures, `docs/audits/2026-10-06-terminal-parity-acceptance.md`。

**Interfaces:** acceptance matrix rows `reference_feature / source_anchor / canonical_descriptor / wire_face / consumer / effect_assertion / result / evidence` with `PASS|FAIL|UNRUN|BLOCKED` status. QA always disposable home/token/approval sink, owns only its child PIDs; guard each cargo build, one compiler lease。

- [ ] **Red:** fixtures fail against pre-change baseline for TUI render/input grant/revoke, split/revision, agent prompt/keys/start, hooks reload, restart tombstone. No grep-only success: each driver asserts screen/runtime/store/worktree/config actual effect. Confirm test filters list at least one matching test before treating exit0 as green.
- [ ] **Verify red:** selected driver against isolated baseline binary, record expected missing capability/refusal, not tests passed without requests. Baseline unsupported host → UNRUN rather than fake red。
- [ ] **Implement:** exact mapping for herdr agent/session/workspace/pane/layout/history/input/hooks/worktree/persistence/handoff feature families; Orca admission-control/stream/input/resource bound invariants; Paseo snapshot/restore/reconnect/CLI lifecycle invariants. Verify negative direction as well: hidden/inaccessible resource no leak, rejected action no effect. Store user-secret-free metadata only. Check selection/scrollback/mouse, UTF-8, fullscreen app, ESC/Press/Release, bracketed paste, reconnect and lag; document unsupported keyboard protocol distinctly。
- [ ] **Green:** `identify`, `wait`, `quiet`, `cwd`, `real`, `tui` fixtures independently + new capability/hook/recovery drivers; Windows ConPTY actual shell case where Unix fixtures can't run. Seven minimum validation surfaces from total plan; failure cannot be hidden by passing related crate. Package `aleph-panel` wasm build, TUI no-core dep, cross-client exact DTO keys. Real external agents absent are UNRUN, fake fixture not falsely substituted for real QA。
- [ ] **Commit:** `qa: verify terminal capability effects and parity gaps`。

### C6: Entropy reduction, locator/reference docs and final integration / 清理与文档

**Files:** Modify `docs/reference/FEATURE_LOCATOR.md` §6.11/§6.12/§5.13, `docs/reference/TERMINAL_RUNTIME.md`, `docs/reference/TOOL_SYSTEM.md`, `docs/reference/ARCHITECTURE.md`, `docs/reference/SUBSYSTEM_ROUTING.md`, `qa/README.md`; delete proven obsolete bodies/DTOs/RPC direct handlers in files owned by A/B tasks; refresh `graphify-out/` only if approved project workflow supports regeneration on this host。

**Interfaces:** documentation maps one callable descriptor to all real consumers; references only actual module/function/test names. Dead-code evidence: old fact/action body replaced by canonical owner, no remaining callers, tests exercise new path. Compatibility adapters are not dead just because UI migrated.

- [ ] **Red:** automated source census identifies duplicated read/action schemas, old ungranted pty.input sinks, `interfaces/tui` dependency on alephcore if introduced, dangling doc code paths. Doc-only red evidence is links/path check, not fabricated unit-test failure。
- [ ] **Verify red:** `git -c core.autocrlf=true diff --check`; script resolves all changed FL paths and asserts listed capability names registered once; method census ensures both admit/refuse ruling recorded. Expectations explicit; no “search found none therefore feature correct”。
- [ ] **Implement:** remove only evidenced duplicate stores/inference/registration/handler bodies; retain transitional ingress compatibility with canonical gate. Bilingual docs show exact default-off/independent human grant/security exception, error outcomes, replay Unsafe, real-vs-recreate recovery, limits, QA commands and unsupported platforms. New FL entries point to tested consumer rendering lines, not DTO existence. Update graph when possible; record HEAD/date and stale graph status if unavailable rather than manually fabricate nodes。
- [ ] **Green:** full targeted tests/minimum validation set + clippy and diff-check; fresh reviewer verifies captures/authorization/lifecycle/negative effects and no harness creep. All acceptance rows reconciled; unresolved handoff or other adapters remain OPEN and user explicitly decides further design/implementation, never unilateral scope cut。
- [ ] **Commit:** `docs: locate terminal capabilities and remove obsolete paths`；feature branch only, no main merge/push/worktree cleanup。Final Chinese report: delivered features, commit IDs, PASS/FAIL/UNRUN/BLOCKED, known gaps and explicit “没有做什么”。
