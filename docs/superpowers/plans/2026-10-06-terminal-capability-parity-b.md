# Terminal Capability Parity B — Human Grants, Agent Writes & Hooks

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 提供真实可用但不可委托的 human shell 交互，随后开放严格受治理的 agent 控制、生命周期与第三方 hooks / Deliver guarded effects without a hidden raw-input bypass.

**Architecture:** 同一 callable registry/scoped gate；human grant 是 per-connection resource，agent effect 绑定 per-call proof 与 durable intent。输入串行化存在 session，不在 UI；第三方 hook 配置不是 Aleph extension hook / Separate provenance, shared canonical admission.

**Tech Stack:** 既有 Rust/Tokio/serde、approval、command policy、SessionEventStore、portable-pty；无新依赖。

**Spec:** `docs/superpowers/specs/2026-10-06-terminal-capability-parity-design.md` §2.2–§2.3。

## Global Constraints

继承总计划所有约束。A3–A4 通过后方可接线。旧 RPC `pty.input` 在 B1 必须失败关闭，直到 grant 存在；不能暂留“迁移期后门”。以下新增 signatures 是计划契约 / Planned interfaces.

## Review Focus

Wire 不可伪造 human context；send 一半 unknown 不自动 retry；approval 精确绑定 final input；close 不能把 kill request 写成 exit；hooks 不改无关 JSON。

---

### B1: Nondelegable direct-interaction grant / 人类授权

**Files:** Create `src/gateway/pty/interaction.rs`; Modify `src/gateway/server/connection/dispatch.rs`, `src/gateway/server/connection/cleanup.rs`, `src/gateway/handlers/pty.rs`, `src/gateway/handlers/tools_invoke.rs`, `src/gateway/handlers/mod.rs`, `src/gateway/pty/mod.rs`, `src/config/types/policies/terminal.rs`, `shared/protocol/src/terminal.rs`; tests in interaction module and gateway dispatcher。

**Interfaces:** `TerminalWritePolicy::{Off,Ask,On}` serde snake_case default Off；config field `TerminalConfig::terminal_write`；human grant not controlled by that agent switch. Server-only `DirectInteractionContext` constructed only from authenticated direct RPC boundary **and actual explicit operator approval**, contains actor+connection_id. `InteractionGrant` has private fields (session id/generation/actor/connection/grant id); cannot Deserialize. DTO `TerminalInteractionAuthorizeParams { session_id: String }`, `TerminalInteractionGrant { grant_id: String, session_id: String }` does not expose process credentials. Store methods:
```rust
pub(crate) async fn authorize_interaction(ctx: &DirectInteractionContext, session_id: &str) -> Result<TerminalInteractionGrant, ToolError>;
pub(crate) fn revoke_interaction(ctx: &DirectInteractionContext, grant_id: &str) -> Result<(), ToolError>;
pub(crate) fn revoke_connection(connection_id: &str);
```
Grant source is a trusted user approval event, not a client UI label. Network protocol cannot prove biological humanity; the guarantee is explicit user consent + nondelegable server provenance, documented as such.

- [ ] **Red:** `human_true_json_cannot_mint_grant`, `tools_invoke_cannot_authorize_interaction`, `grant_is_bound_to_connection_and_generation`, `disconnect_and_policy_disable_revoke_grants`, `agent_write_default_is_off`；even operator Tool caller cannot create DirectInteractionContext。
- [ ] **Verify red:** `CG test -p alephcore --lib terminal_interaction` and `CG test -p aleph-protocol terminal_interaction`；Expected absent grant/default-off field。
- [ ] **Implement:** use existing approval request/response bridge, random opaque grant id from existing UUID generator; approval card describes unrestricted subsequent shell bytes, actor/session/connection binding. Privacy-preserving audit of grant/revoke metadata; explicit grant authorization is the durable intent boundary for the interactive lease, not one SQLite ToolCallRequested barrier per key. Per-input records preserve generation/length/outcome without logging secret bytes; revocation checked on every frame. `tools.invoke` does not inherit direct context; reconnect requires new approval, not token transplant. Config disable closes admission immediately, revoke on terminal close/connection disposal。Old `pty.input` rejects absent grant before any byte。
- [ ] **Green:** tests above + existing config defaults/serialization tests; addressed cross-owner authorize always denied; reply path verified user consent not merely card emission。
- [ ] **Commit:** `terminal: add revocable direct-interaction grants`。

### B2: Serialized human input and TUI/Panel wiring / 真正交互

**Files:** Modify `src/gateway/pty/interaction.rs`, `src/gateway/pty/session.rs`, `src/gateway/handlers/pty.rs`, `src/builtin_tools/terminal/capabilities.rs`, `shared/client/src/terminal.rs`, `shared/protocol/src/terminal.rs`, `interfaces/tui/src/tui/terminal.rs`, `interfaces/tui/src/tui/event.rs`, `interfaces/tui/src/tui/keys.rs`, `interfaces/webchat/src/platform/wide/views/terminal/session.rs`。

**Interfaces:** `TerminalInteractionInputParams { session_id: String, grant_id: String, data: String }` data UTF-8/control representation matches legacy input wire; max `65_536` encoded bytes per call, error before write on overflow. `InputOutcome::{Written { bytes: usize }, Refused { reason: String }, Unknown { bytes_may_have_been_written: bool, reason: String }}` serialized with stable tagged outcome. `write_interaction(ctx: &DirectInteractionContext, params: &TerminalInteractionInputParams) -> Result<InputOutcome, ToolError>` uses per-session input mutex shared with B3; serializes only input (never holds grid mutex across await)。

- [ ] **Red:** other connection replays valid grant -> writer count 0；grant revoked while queued -> count 0；write error after partial -> Unknown not retry；Press/Release -> exactly one key；approval input not forwarded；large paste bounded chunks and starts/ends bracket once。
- [ ] **Verify red:** `CG test -p alephcore --lib terminal_interaction` and `CG test -p aleph-tui terminal_input` Expected renderer locked/no guarded write outcome。
- [ ] **Implement:** authentic ctx plus owner/generation/grant recheck inside serialized critical section. Reuse existing write sink; existing portable-pty write may fail without exact byte count, return Unknown. Register `terminal_interaction_input` privileged handler, exclude from LLM projection but retain authorization. UI explicit authorize/unlock/revoke, exact structured refusal display; client routes no direct subprocess. Preserve keymap/mouse/application cursor/bracketed-paste modes from server; unsupported kitty keyboard mode is visible, not corrupted sequences。
- [ ] **Green:** tests + disposable shell live QA: grant accepted then type marker, screen shows marker; revoke then marker absent (effect test, not called=true). Panel browser same contract; unsupported host marked UNRUN。
- [ ] **Commit:** `terminal: wire granted human input through thin clients`。

### B3: Guarded prompt/send_keys / Agent 主动控制

**Files:** Create `src/gateway/pty/control.rs`; Modify `src/gateway/pty/session.rs`, `src/gateway/pty/foreground.rs`, `src/gateway/runtime/mod.rs`, `src/builtin_tools/terminal/capabilities.rs`, `shared/protocol/src/terminal.rs`, `shared/client/src/terminal.rs`; Test control module and `src/builtin_tools/terminal/tests.rs`。

**Interfaces:** DTO `TerminalPromptParams { session_id: String, text: String }`; `TerminalKey::{Enter, Escape, Tab, Backspace, Delete, Up, Down, Left, Right, Home, End, PageUp, PageDown, CtrlC, CtrlD}` (no arbitrary ANSI input); `TerminalSendKeysParams { session_id: String, keys: Vec<TerminalKey> }`, `1..=32` keys. Private `WritePermit` created only after scoped proof+policy+owner+fresh foreground+runtime identity; snapshots session generation and process identity. `TerminalControl<'a>` borrows existing manager/runtime/config/required SessionEventStore; no owned second session map. `TerminalControl::new(pty: &'a PtyManager, agents: &'a RuntimeAgents, config: &'a TerminalConfig, store: &'a dyn SessionEventStore) -> Self`; config snapshot is revalidated against current policy before bytes. `prompt(&self, permit: WritePermit, params: &TerminalPromptParams) -> Result<InputOutcome, ToolError>`; `send_keys(&self, permit: WritePermit, params: &TerminalSendKeysParams) -> Result<InputOutcome, ToolError>`。`WritePermit` is not caller-editable and not Clone beyond same call.

- [ ] **Red:** table off/ask/on × owner/nonowner × agent/nonagent × Blocked × proof present/missing; every forbidden case records zero first bytes. empty/whitespace, ESC/NUL/C0 except TAB/LF, >65_536 UTF-8 bytes fail preflight. `foreground_changes_before_delayed_enter_do_not_submit`；ordered concurrent prompt/key cannot interleave; partial write/restart never auto-retries。
- [ ] **Verify red:** `CG test -p alephcore --lib terminal_control` Expected no write API or ungoverned writes caught。
- [ ] **Implement:** prompt normalizes CRLF to LF, rejects bare CR/control delimiters and requires actual bracketed-paste mode; text with embedded `ESC[201~` rejected by ESC gate. Write `ESC[200~`, text, `ESC[201~`; existing async timer delay `30ms`, then recheck owner/generation/foreground/approval revocation before Enter. Hold only input serialization lease over delay, no screen/store lock. Failure after text but before Enter returns Unknown/partial; do not clean up by blindly writing Enter. send_keys allowed Blocked, requires same fresh agent foreground; CtrlC/CtrlD request dedicated confirmation due destructive effect. Start auto commands are command-policy checked; prompts are not disguised shell exec. Mandatory intent commit before first bytes; journal is evidence, not sole barrier。
- [ ] **Green:** above tests + real fake-agent fixture consumes bracketed-paste and emits exact text + Enter observation; evidence confirms effect reached target. `terminal_write=on` still fails nonowner/missing proof; no human grant required for agent callable, no human grant usable to bypass it。
- [ ] **Commit:** `terminal: add admitted agent prompt and key effects`。

### B4: Governed start/spawn/resize/close / 生命周期效果

**Files:** Modify `src/gateway/pty/control.rs`, `src/gateway/pty/manager.rs`, `src/gateway/pty/session.rs`, `src/gateway/handlers/pty.rs`, `src/builtin_tools/terminal/capabilities.rs`, `src/builtin_tools/process_journal.rs`, `shared/protocol/src/terminal.rs`; tests manager/control。

**Interfaces:** `TerminalStartParams { agent_id: String, workspace_id: String, rows: u16, cols: u16 }` manifest-selected argv, no free script；`TerminalSpawnParams { workspace_id: String, shell_profile: String, rows: u16, cols: u16 }` configured profile only. `TerminalCloseParams { session_id: String }`; `TerminalCloseOutcome::{ExitObserved { exit_code: Option<i32> }, TerminationRequested, Unknown}`；`PtySession::request_kill(&self) -> Result<(), String>` replaces ignored-error kill while preserving exit watcher as authoritative. `PtyManager::close(&self, session_id: &str) -> Result<TerminalCloseOutcome, String>` keeps tombstone until observed exit, disallows new writes before request. resize `1..=1000` and owner/generation admission, separate from prompt。

- [ ] **Red:** rejected command policy -> no spawn; unknown profile/agent -> no effect; invalid dimension -> no spawn/resize；kill error -> not closed success；kill requested/no exit -> TerminationRequested；late exit records exact observed state, no duplicate settled event；write queued before close fails pre-effect。
- [ ] **Verify red:** `CG test -p alephcore --lib gateway::pty` Expected current manager remove-before-kill / ignored kill error assertions FAIL。
- [ ] **Implement:** reuse configured jailed workspace roots, existing command-policy evaluators and ExecTier approve effective argv/cwd; process initial argv directly through SpawnOptions, do not `pty.input` launch scripts. Durable effect intent mandatory; if store unavailable refuse start. Record child/spawn/settled journal alongside required intent/outcome; missing optional journal cannot be sole permission. Close gate precedes irreversible request, actual child exit sets closed; no lock held awaiting child. Resize is explicit capability, not agent-only text write restriction。
- [ ] **Green:** targeted manager/handler/journal tests plus bins compilation; approved start real process PID and runtime identification observed, close waits actual exit event. Audit identity same captured descriptor, no optimistic success。
- [ ] **Commit:** `terminal: govern launch and report observed lifecycle outcomes`。

### B5: External hook plan/consent/apply / 外部 hooks 配置

**Files:** Create `src/gateway/pty/agent_hooks.rs`; Modify `src/builtin_tools/terminal/capabilities.rs`, `shared/protocol/src/terminal.rs`, `crates/agent-detect/src/manifests/claude.toml`, `crates/agent-detect/src/manifest.rs`, `crates/agent-detect/src/manifest/tests.rs`; test hook fixtures `qa/terminal/fixtures/agent_hooks/`。

**Interfaces:** `TerminalHookMutation::{Install, Enable, Disable, Remove, Repair}`; `TerminalHookPlanParams { agent_id: String, workspace_id: String, mutation: TerminalHookMutation }`; `TerminalHookPlan { plan_id: String, before_revision: String, target: String, patch_summary: Vec<String> }`; `TerminalHookApplyParams { plan_id: String }` (no caller path/shell); `HookApplyOutcome::{AppliedNeedsReload, Conflict, Refused, Unknown}`。`plan_hooks(...) -> Result<TerminalHookPlan, ToolError>` read-only; `apply_hooks(...) -> Result<HookApplyOutcome, ToolError>` proof bound to plan id+before hash+canonical descriptor. First adapter Claude JSON settings only; existing manifest/schema is extended minimally with validated hook support declaration, not inferred from agent name.

- [ ] **Red:** preserve unrelated user keys/hooks byte semantics after parse/serialize; repeated apply doesn't duplicate Aleph-owned entries; consent absent -> file untouched；external modification since plan -> Conflict unchanged；symlink target outside root -> Refused；write/rename failure -> Unknown, not Applied。
- [ ] **Verify red:** `CG test -p alephcore --lib terminal_agent_hooks` Expected no hook planner/owned marker support。
- [ ] **Implement:** jailed canonical target from manifest+workspace; human sees diff summary and approves exact plan. Atomic temp-write+rename with directory durability where supported, journal required intent/outcome. Explicit Aleph-owned entry marker/version for install/disable/remove/repair; update/remove only owned entry, never replace complete hooks array. status reads actual filesystem and distinguishes installed vs agent reloaded. Unknown agent Unsupported. Aleph internal `hooks_manage` remains separate domain, no hidden config edits from prompt/start。
- [ ] **Green:** isolated temp dirs tests and real Claude settings fixture parse (no user config touched)；manifest loading/schema suite PASS；filesystem operation outcome exact。
- [ ] **Commit:** `terminal: add consented external agent hook patches`。

### B6: Runtime hook delivery and destructive verb projections / Hook 连线

**Files:** Modify `src/gateway/pty/agent_hooks.rs`, `src/gateway/runtime/mod.rs`, `src/gateway/handlers/mod.rs`, `src/gateway/event_scope.rs`, `shared/client/src/terminal.rs`, `interfaces/tui/src/tui/terminal.rs`, `interfaces/cli/src/commands/terminal.rs`, `qa/terminal/drive_terminal.mjs`; fixtures/test in hook/runtime modules。

**Interfaces:** `TerminalHookEvent { session_id: String, invocation_id: String, lifecycle: TerminalHookLifecycle }`; `TerminalHookLifecycle::{Started,Working,Blocked,Idle,Exited}`; authenticated local ingress validates session/process generation-bound nonce before feeding **existing** RuntimeAgents sampler evidence. No `agent.status=...` UI-authoritative shortcut. Client methods for prompt/send_keys/start/close/hooks serialize typed DTO only; foreground/policy remains Core。

- [ ] **Red:** event with wrong nonce/old generation ignored；foreign owner receives none；duplicate invocation id does not update state twice；old hook event cannot regress newer observed process exit；hooks apply returns NeedsReload and UI cannot label active hook without delivery evidence。
- [ ] **Verify red:** `CG test -p alephcore --lib terminal_hook_event` and `CG test -p aleph-tui terminal_control` Expected missing ingress/status distinctions。
- [ ] **Implement:** lifecycle hook transport uses existing authenticated gateway/CLI delivery, not second server/scheduler；nonce scoped to current session generation, injected only into the launched PTY child environment; hook config contains `aleph terminal hook emit` invoking the existing CLI transport and reads nonce/session from environment, never stores secret in JSON. User-owned existing shells without launch nonce show hook unavailable until explicit recreated/started session; no unsafe retroactive credential injection. Integrate provenance into RuntimeAgents with process/screen priority documented; hook facts cannot spoof foreground identity. TUI approval/output reflects typed outcomes, CLI write commands use scoped service. QA replaces direct ungranted pty.input with explicit human authorization or agent start fixture。
- [ ] **Green:** fixtures prove hook -> existing runtime changed topic -> TUI row actual lifecycle change；observed shell bytes remain human audited path；blocked/unknown cannot be green success. Non-Claude agent hook coverage remains listed OPEN until adapter fixture verified。
- [ ] **Commit:** `terminal: connect authenticated hook evidence and control clients`。
