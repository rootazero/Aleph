# Phase10: NSH macOS Shell Integration — Implementation Plan
# Phase10：NSH macOS 桌面壳集成实施计划

> **For agentic workers:** Use `superpowers:subagent-driven-development` only when delegation is authorized, or `superpowers:executing-plans` for direct execution. Track the checkboxes below. Read the restart prompt before implementation.

**Status / 状态:** Proposed next implementation scope, prepared at the user's request after Phase9 acceptance. This document does not claim Phase10 has been implemented or independently accepted. / Phase9 完整验收后的下一阶段提案；本文不是 Phase10 完成证明。

**Goal / 目标:** Make the existing NSH 0.5 macOS Panel host usable as a resident desktop shell: real window close/reopen/quit, tray and single-instance delivery, summon hotkey, deep-link delivery, notifications and window-state restoration, with explicit resource ownership and native evidence. / 在现有 Panel 宿主上补齐可验证的常驻桌面壳能力。

**Architecture / 架构:** Keep Leptos/WASM Panel as the sole business UI, Chat/session state and Gateway transport owner. NSH owns only OS integration and the lifetime of its window/WebView, callbacks and registrations. Use documented pinned NSH APIs and existing Aleph contracts; keep Core platform independent. / 业务仍在 Panel；NSH 只拥有 OS I/O 与自身资源生命周期。

**Tech Stack / 技术栈:** Rust, pinned NSH 0.5, public macOS APIs exposed through the approved host/Bridge boundary, existing Leptos/WASM Panel, deterministic isolated Gateway QA, Python native evidence tools. No new heavy runtime dependency by default.

**Spec / 规格依据:** [NSH master](../specs/2026-11-05-nsh-desktop-refactor-design.md), [R2 choice A](../runs/2026-11-16-phase9-nsh-r2-boundary.md), [Phase9 accepted contract](../specs/2026-11-15-phase9-runtime-proof-and-dependency-closure.md), [Panel parity contract](../specs/2026-11-15-nsh-panel-parity-slice.md), [Phase10 entry brief](../runs/2026-11-18-phase10-entry-brief.md). Task 1 produces the concrete Phase10 spec before production edits.

**Restart prompt / 重启提示词:** [2026-11-18-phase10-nsh-shell-restart.md](../prompts/2026-11-18-phase10-nsh-shell-restart.md).

## 1. Authority, numbering and design trade-offs / 权威、编号与取舍

1. The NSH master is a historical **DRAFT for v0.4**, with Phases **0–6**, not a Phase10 specification. Read all 393 lines, especially §§2–3, 5, 6 and 8. Phase10 here names the continuation after accepted Phase9; it is not master Phase5 Plugin Runtime, nor a new Capability Core roadmap claim. / 两套阶段编号不能互相充当完成证明。
2. The explicit R2 decision is already made: **choice A, NSH 0.5 WKWebView hosts the existing Panel**. Do not reopen it or silently replace it with native Chat. The master's native business UI and proposed `interfaces/nsh-desktop` are not this phase's implementation instructions.
3. Phase9 final acceptance and final integrity supersede old `D OPEN/BLOCKED` checkpoints. Preserve those checkpoints and the rejected blocks as history; do not rewrite their verdicts.
4. The proposed Phase10 scope below fills concrete gaps in the master's **Phase2 OS integration**. It does not declare the master's entire Phase2/3/4 done. Phase5 requires its ADR prerequisites; Phase6 requires Phase4 plus the stated two-week dual run. Neither is authorized here.
5. Thin native integration preserves R2 but requires an explicit Panel navigation/intent boundary and real OS tests. Resident behavior makes close different from quit and requires owned registration teardown. Those costs must be visible in the spec, not hidden inside a generic supervisor or second transport.
6. This plan is a proposal requested for the next session. Using the companion prompt starts work on this bounded proposal. A public API gap, constitution change, new daemon ownership model or release trust change still needs a concrete decision before dependent implementation. Routine review fixes inside the agreed scope do not need repeated permission.

## 2. Accepted baseline / 已验收基线

The accepted Phase9 tree is committed as `42764c43d4be5bbe26094a84d1ce9495f08a6dca` (`feat(nsh): close phase9 runtime proof and dependency validation`). The user requested local merge to `main`; on restart verify this commit is an ancestor of the actual main HEAD. The original measurement base/HEAD was `3892804c112fd6e574166a4c03a7c40a77d0b239` on `runtime-proof`; it is a **historical measurement identity**, not the HEAD to reset to. Preserve the integrated Phase9 tree and its original receipt identities.

| Gate | Accepted evidence / 已有证据 | Limit / 限界 |
| --- | --- | --- |
| A | Retained graph: 54 terminal PASS invocations; full Core `22,402 passed / 0 failed / 20 existing ignored / 656.84s` | Do not add nested subprocess summaries to the total |
| B | Adopted real resolver graph: 1,572 entries; separate full Core `22,402 / 0 / 20 / 661.14s` | Not the same run as A |
| C | Existing production Panel, canonical session/agent projection, history race regressions, real queued wire → production dispatch → ChatState | Panel 1,568; Tauri 190; NSH 5 tests at the accepted build |
| D | Entire fresh block `isolated-matrix-40-20261118T185504`: 40 Tauri then 40 NSH, each 10 warmups + 30 measured; all 80 runs PASS | Approved macOS debug slice only |
| E | Final SPEC `1b81b8e5` → distinct-model QUALITY `8377f29f`; final handoff verification PASS | Reviewer notes and limitations remain recorded |

Important identities / 关键身份：

- `Cargo.lock` SHA-256: `12300967333b45f8a0fa1f401cf13a559b5b8a881ee95550b1bbf1a8d42e21f5`.
- NSH Git revision: `f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1`.
- Final acceptance JSON SHA-256: `8714d91fb95a07a95f0cf77f921c1917ca4451de5f31d79db7c9f72bd4e4f181`.
- Canonical default WASM SHA-256: `9f708c647e481cb1e1a53947ec8a6c0694895d6f10f9409ba46732e8db367cb8`.
- Immutable Phase9 auditor SHA-256: `1d358b50a1417c9ccad34e3fceaa7a042d44485355c8b7bd5b6c313865dfdc94`.
- 52 accepted source files, nine restored default assets, 30 archived review reports; 6,400 raw correlated events and 7,002 real resource samples in the accepted block.
- 17 complete historical matrix directories: **15 INVALID**, one `174308 VALID / NOT_ACCEPTED`, one `185504 PASS`; all default restorations succeeded. No selected-round splicing.

| Median metric / 中位数 | Tauri | NSH |
| --- | ---: | ---: |
| Startup to first-frame receipt, ms | 5357.951 | 4933.0425 |
| Chat route, ms | 33 | 33 |
| Input to addressed `chat.send` receipt, ms | 198.5 | 198.5 |
| Reconnect repair, ms | 1041.5 | 1022.5 |
| Application quit settlement, ms | 47.679 | 48.921 |
| Peak RSS, MB | 678.1015625 | 640.84375 |
| Maximum live threads | 76 | 68 |

Close p95 was `77.444 → 61.365ms`. All original thresholds passed. The application-quit test uses public `quit_app()` and independent kernel/all-observed-member checks; it **does not prove user WindowClose callbacks**. Phase10 must supply that evidence itself.

## 3. Required reading and source owners / 必读与源码权威

Read in this order; historical plans are a gap checklist, not authority to restore native business UI. / 按顺序阅读，旧计划只用于差距审计。

1. Root `AGENTS.md`; `docs/reference/SUBSYSTEM_ROUTING.md`, `REDLINES.md`, `DEVELOPMENT.md`, `WORKSPACE.md`, `STYLE.md`, `PROCESS_MANAGEMENT.md`, `SECURITY.md`, `TOOL_SYSTEM.md`; relevant subsystem instructions. Read `HARNESS_PHILOSOPHY.md` only if touching that boundary; this phase should not modify the harness.
2. **Entire NSH master:** `docs/superpowers/specs/2026-11-05-nsh-desktop-refactor-design.md`.
3. `docs/superpowers/plans/2026-11-05-nsh-phase0-spike.md`, `2026-11-05-nsh-api-verification.md`, `2026-11-05-nsh-input-limitations.md`; `desktop/nsh-spike/FINDINGS.md`.
4. `docs/superpowers/plans/2026-11-05-nsh-desktop-phase1-protocol-transport.md`, `2026-11-05-nsh-desktop-phase2-connection.md`, `2026-11-05-nsh-desktop-phase3-ui-core.md`, `2026-11-05-nsh-desktop-phase3-technical-debt.md`, `2026-11-05-nsh-desktop-phase4-capability-alignment.md`.
5. `docs/superpowers/plans/2026-11-15-phase8-integration-security-and-nsh-restart.md`; `docs/superpowers/specs/2026-11-15-phase8-integration-security-design.md`; final Phase8 handoff `docs/superpowers/runs/2026-11-16-phase8-main-handoff.md`.
6. Current R2 and accepted Phase9 files in the Spec header; `docs/superpowers/runs/2026-11-16-phase9-closeout.md`, `2026-11-16-phase9-execution-receipt.md`, `2026-11-16-phase9-final-integrity.json`, `2026-11-18-phase9-acceptance.json`, `2026-11-18-phase9-source-manifest.json`, `2026-11-18-phase9-default-manifest.json`, `2026-11-18-phase9-parity-receipt.json`.
7. `qa/phase9_runtime.md`, `qa/nsh_panel_launcher.py`, `qa/nsh_panel_matrix.py`, `qa/nsh_panel_gateway.py`, `qa/nsh_panel_driver.js`, `qa/nsh_panel_parity.py`, `qa/test_phase9_native_meter.py`; use the measurement repair report when a historical failure mechanism matters.

Current source facts to recheck / 当前落点（重启后复核）：

| Path | Actual responsibility / 真实职责 |
| --- | --- |
| `desktop/nsh-shell/src/main.rs` | Only `shell_host` plus debug QA clock is mounted; one existing Panel WebView, 1180×800, `com.aleph.nsh-shell`; QA nonce/run-bound quit |
| `desktop/nsh-shell/src/shell_host.rs` | `HostSettings::from_inputs(url, nonce)`; default `http://127.0.0.1:18790/`; confined debug loopback+nonce validation |
| `desktop/nsh-shell/tests/nsh_v05_window_compat.rs` | Pinned public window API compile coverage; not proof of user-close semantics |
| `desktop/nsh-shell/src/{connection,session,runtime,supervisor}.rs` | Old unmounted paths; do not mount duplicate Gateway/session state or detached `ensure_ready` by convenience |
| `desktop/nsh-shell/src/components/` | Old unmounted native business UI; keep unmounted |
| `desktop/shell/src/{main,tray,hotkey,deeplink,notify}.rs` | Tauri behavior reference; its `Supervisor` is in `main.rs`, not a separate `supervisor.rs`; platform wrappers are not Core policy |
| `interfaces/webchat/src/{app,context}.rs` | Existing Panel state/transport and route owner; maintain its attempt/epoch guards |
| `interfaces/webchat/src/platform/wide/views/chat/events.rs` | Production event projection, including queue lifecycle; lazy i18n boundary is intentional |
| `interfaces/webchat/src/components/chat_sidebar.rs` | History hydration race protection and QA history witness |
| `shared/ui_logic/src/connection/wasm.rs` | Actual Panel WASM transport; a new native `shared/client` loop would duplicate it |

`desktop/nsh-shell/src/tray.rs`, `hotkey.rs`, `deeplink.rs`, `window_state.rs` and `notifications.rs` are **proposed new files, not existing implementations**. Keep the final module split small and based on proven ownership boundaries.

## 4. Global constraints / 全局约束

- Work from the user's **local main**, not `origin/dev`. Resolve the current Git root; documents use relative paths. Do not pull/reset main, delete branches/worktrees/evidence, or overwrite unknown edits. Use short branch names if a later user requests a branch.
- Never read, modify or stage `.claude/settings.json`, root `config.lock` or root `config.toml`. Do not change `AGENTS.md`/`CLAUDE.md` or architectural redlines as an implementation shortcut. `Cargo.lock` is allowed, but keep the accepted pin unless an actual resolver change is justified and reviewed.
- **Before source exploration, run Graphify**, then targeted reads/searches. Preserve this instruction verbatim in any authorized child-agent brief. Inspect current files before editing; the prior session had concurrent/shared edits.
- One Cargo owner, no parallel Cargo. Before **every** Cargo invocation, run `python3 qa/browser_live/check_memory.py --required-bytes 4294967296`; continue only on ALLOW. Use `--offline --locked --jobs 1`, with `CARGO_TARGET_DIR` pointing at a new `.phase10-target`. Do not overwrite `.phase9-target` binaries that bind old evidence.
- Read relevant testing/debugging/review skills before changes. Prefer actual native/component state transitions over global mocks or implementation-mirroring tests. Record actual RED → GREEN for substantive regressions; missing helper tests alone do not prove a causal bug.
- Independent **SPEC → QUALITY**, in that order, for the implemented scope; use a different Pi model for the second opinion. Delegate only with authorization. No Codex/Claude/Gemini CLI fallback. DeepSeek is **Flash only**.
- New externally composable behavior follows the existing Capability/tool contracts. Inspect the current canonical owners before proposing a new callable surface. Do not add a second registry, lease map, channel authority, tool metadata authority or business reducer.
- Keep disabled features resource-free; own callbacks, registrations and workers. No detached threads or implicit global Tokio runtime for optional OS features. R1/R4/R6 remain binding.
- Do not change the Phase9 reducer or thresholds, relax ownership, use whole-machine deltas, run as root, use private macOS APIs or change OS security policy. Do not ship QA ingress/callbacks in default WASM.
- `just stage-shell-placeholders` can replace real sidecars with zero-byte placeholders. Snapshot and restore any owned binary it touches; never use placeholders as full-package proof. Debug RustEmbed reads the **current** `interfaces/webchat/dist`, so a copied Gateway binary does not freeze Panel assets.
- Preserve raw evidence and review originals byte-for-byte, including historical whitespace. Source/new-document whitespace checks are separate from diagnostics in retained logs; do not rewrite hashed receipts to silence `git diff --check`.
- Communicate in Chinese; code comments in English; new documents bilingual. Report changes, trade-offs, actual checks and skipped validations explicitly.

## 5. Scope and acceptance contract / 范围与验收合同

The spec created in Task 1 must resolve these observable cases without adding a native business UI. / Task 1 的具体规格须逐项落实这些行为。

| Gate | Required behavior / 必须实现的行为 | Required evidence / 必须提供的证据 |
| --- | --- | --- |
| G1 Window | User close enters the agreed resident-hidden state; reopen/focus reuses the existing Panel; explicit Quit settles shell-owned resources; close/quit causes remain distinguishable | Actual public native window callbacks, close → reopen → quit, repeated lifecycle, no duplicate receiver/window |
| G2 Tray & instance | Tray Show/Quit use the same lifecycle owner; a second launch forwards only a validated launch intent and exits, without a second resident host | Two real same-user launches; one resident window owner; primary/secondary shutdown and registration cleanup |
| G3 Hotkey | Summon hides/shows/focuses the existing window; conflict/registration errors are visible; disabled mode has no registration/worker | Native hotkey delivery, conflict case, teardown/re-registration |
| G4 Deep link | Existing supported URI grammar is validated, then delivered once to the existing Panel navigation owner; business state stays in Panel | Cold/warm delivery, second-instance delivery, invalid/unsupported input rejection and unchanged projection |
| G5 Notification | Existing notification intent is shown through the host boundary; click focuses the current Panel and routes through its existing owner | Real native notification + callback where OS permission permits; denied permission is reported, never bypassed |
| G6 Window state | Restore supported normal-frame/visibility state; reject invalid/off-screen state safely; presentation state does not become session authority | Relaunch/restore and changed-display checks using actual public host events |
| G7 Lifecycle & regression | Quit joins/releases every new shell-owned worker/receiver/registration; external managed Gateway is untouched; existing Chat/Artifacts/reconnect guards survive | Native exit + observed-member settlement, registrations gone, controlled managed Gateway still healthy, scoped tests/default asset check and fresh comparable runtime proof |

If G1's public API feasibility fails, produce a concrete supported alternative and obtain the needed decision; do not advertise a tray app with unproved close semantics. Missing OS permission is a scoped validation blocker, not evidence that G5 passed.

Out of scope / 不在本期：release installer/notarization/auto-update installation, switching the default desktop, full embedded-core bundle ownership, detached daemon takeover, Core execution redesign, installed-service `SimulatedEngine` transcript repair, new native Chat/Artifacts/Settings business UI, Plugin Runtime, Swift camera/voice/screenshot features, i18n migration, Windows/Linux runtime acceptance, Tauri/WebChat/workspace removal and two-week rollout completion. No remote push/release is implied.

## 6. Implementation tasks / 实施任务

### Task 1 — Reconcile main, prove APIs, freeze the Phase10 spec / 复核 main、验证 API 并形成规格

**Files:** Read all required references; create `docs/superpowers/specs/2026-11-18-phase10-nsh-shell-integration.md`, `docs/superpowers/runs/2026-11-18-phase10-baseline.md`, `docs/superpowers/runs/2026-11-18-phase10-nsh-api-inventory.md`. Extend the date only if the next session intentionally chooses its actual execution date.

**Interfaces:** Integrated main tree + accepted manifests → explicit G1–G7 cases, real source owners, public pinned API list and a master Phase0–6 DONE/PARTIAL/NOT_PROVEN/DEFERRED table.

- [ ] Record root/branch/HEAD/status/worktrees and the merged Phase9 commit as an ancestor. Hash-check accepted source/default files; classify later changes rather than resetting to historical HEAD.
- [ ] Read the entire NSH master and current R2 boundary. Map every master Phase2 item to current mounted code. Record updater as deferred, not silently complete.
- [ ] Graphify, then inspect pinned NSH 0.5 public window, menu/tray, instance, hotkey, URL, notification and state APIs. Validate only APIs actually needed; build small executable spikes where compile coverage is insufficient. Do not infer behavior from the v0.4 spike.
- [ ] Define window CloseRequested/Closed/Hidden/Reopened/Quit and ownership in the spec, plus the managed-server boundary and intent validation. Decide the minimal host module shape from actual APIs.
- [ ] State what the chosen design makes harder: cross-platform host reuse, callback cleanup, intent handoff and OS QA. Complete SPEC review of this concrete spec before production edits; significant new architecture outside this proposal needs a user decision.
- [ ] Self-review: no master Phase10 invented, no R2 override, no dormant native business path mounted, no missing feature marked DONE.

### Task 2 — Implement real window lifecycle / 实现真实窗口生命周期

**Files:** Modify `desktop/nsh-shell/src/main.rs`, `desktop/nsh-shell/src/shell_host.rs`; add `desktop/nsh-shell/src/lifecycle.rs` only if it names a coherent reusable lifecycle owner; extend `desktop/nsh-shell/tests/nsh_v05_window_compat.rs` and add focused lifecycle/native QA fixtures as needed.

**Interfaces:** Public NSH window events and explicit Quit → one owned host lifecycle. `HostSettings` continues to validate launch input; no session/Gateway transport moves into it.

- [ ] First demonstrate the missing user-close/reopen behavior using the real pinned host. Keep Phase9 application-quit proof separate.
- [ ] Implement resident close/reopen/focus and explicit quit through one owner. Preserve QA nonce/run checks and the real shutdown path; avoid a fake QA-only close state.
- [ ] Cover repeated close/reopen, quit while hidden, init failure and callback teardown. Check window/WebView and owned workers, not only `run_app` return.
- [ ] Self-review: user close evidence is actual, application quit still reaches documented terminal state, external server remains alive, no duplicate Panel state.

### Task 3 — Tray and single-instance intent delivery / 托盘与单实例意图投递

**Files:** Modify `desktop/nsh-shell/src/main.rs`; proposed `desktop/nsh-shell/src/tray.rs` and `desktop/nsh-shell/src/instance.rs`; add focused actual-process tests/native QA.

**Interfaces:** Tray Show/Quit and validated secondary launch intent → Task 2 lifecycle owner and existing Panel route owner. An OS instance endpoint is transport only, not a second business registry.

- [ ] Connect tray Show/Quit to the real Task 2 behavior. Own menu callbacks and release them on quit.
- [ ] Start two real instances; verify one primary, secondary intent delivery, secondary terminal exit and no second resident WebView. Recheck process identity before any cleanup signal.
- [ ] Test hidden primary, primary quit/relaunch and delivery failure. Keep OS routing errors visible; do not spawn a detached fallback daemon.
- [ ] Self-review: no stale socket/lock/registration after owner exit, no broad process-name cleanup, no duplicate intent effects.

### Task 4 — Hotkey and deep-link host boundary / 热键与 deeplink 边界

**Files:** Proposed `desktop/nsh-shell/src/hotkey.rs`, `desktop/nsh-shell/src/deeplink.rs`; read `desktop/shell/src/hotkey.rs`, `desktop/shell/src/deeplink.rs`, `shared/protocol/src/paths.rs` and existing Panel route handling before selecting exact modified paths. Add Panel intent handling only if a canonical boundary is missing.

**Interfaces:** Public OS callback → validated host action or existing Panel navigation. Reuse canonical URI grammar; do not create an independent native session/tool dispatcher.

- [ ] Assert summon/focus behavior with real registration and actual native events; cover conflict and disabled mode.
- [ ] Cover warm, cold and secondary-instance URL delivery; reject malformed, unsupported or foreign-origin inputs before navigation.
- [ ] If a new configurable action is required, reconcile it with the existing canonical Capability/tool owner first. Do not invent RPC names or a registry in this plan's name.
- [ ] Self-review: callbacks are generation/lifetime bound where applicable; repeated delivery cannot revive disposed state; no hidden native Gateway client.

### Task 5 — Notifications and window state / 通知与窗口状态

**Files:** Proposed `desktop/nsh-shell/src/notifications.rs`, `desktop/nsh-shell/src/window_state.rs`; existing Tauri notification/window-state behavior and public pinned APIs; scoped tests and native evidence.

**Interfaces:** Existing notification intent and OS presentation state → owned native adapter and current Panel route owner. Persist only host presentation settings in the canonical allowed settings location.

- [ ] Implement notification display/click and explicit permission-denied outcome; do not request OS policy changes or silently fall back to another channel.
- [ ] Restore supported window state through real native lifecycle events. Check invalid geometry, changed display bounds and hidden/visible semantics.
- [ ] Verify disable/quit removes handlers and consumes no optional worker/connection/cache. Keep localized business notification content in its existing owner.
- [ ] Self-review: no native business state store, no config writes to the protected root files, no claim of permission-granted QA when delivery did not occur.

### Task 6 — Owned resource and managed-server closure / 资源与外部服务边界闭合

**Files:** The new host modules; `qa/phase10_nsh_shell.py` and `qa/test_phase10_nsh_shell.py` are proposed QA entrypoints; reuse proven lower-level `qa/nsh_panel_launcher.py` capture facilities where applicable without changing Phase9 semantics.

**Interfaces:** Host registrations/workers/window + externally managed loopback Gateway → independently observable terminal facts.

- [ ] Enumerate resource ownership and stop order. Exercise startup failure, second-instance failure, user close/reopen and quit while hidden/busy. Native callbacks and registrations must settle even if a helper has already exited.
- [ ] Verify actual native exit status and all observed host/WebKit member settlement. Use public per-instance ownership; preserve raw source and monotonic timings. Do not claim coverage of unobserved subinterval processes.
- [ ] Prove the pre-existing managed Gateway remains healthy after shell quit. Keep the isolated QA Gateway's own fixture cleanup separate; no production daemon ownership is inferred.
- [ ] Record raw command/argv, source/build/asset/lock identities, process identity and terminal checks. Add meaningful failure regressions, not assertions that a cleanup function was called.
- [ ] Self-review: no late signal to a changed PID identity, no newly discovered unvalidated member becomes signal authority, no forced cleanup is reported as natural success.

### Task 7 — Regression, performance and release-safe default / 回归、性能与默认资产

**Files:** `desktop/nsh-shell/`, affected Panel/shared modules, new Phase10 QA, `qa/phase9_runtime.md` as a contract reference; create `docs/superpowers/runs/2026-11-18-phase10-test-matrix.json` and source/default manifests.

**Interfaces:** Current implementation → scoped checks and fresh native acceptance. Historical Phase9 PASS is a baseline, not a new build result.

- [ ] Run the appropriate memory-gated offline/locked/jobs1 checks: NSH package tests/build; Panel full lib tests if Panel changes; shared tests if shared code changes; Tauri QA tests/build if its baseline or QA integration changes. Read `DEVELOPMENT.md` for the seven broader verification classes; record unrun classes honestly.
- [ ] If WASM changes, build both QA and canonical default variants serially; retain actual HTTP-served asset identities. Verify default assets exclude QA ingress, reconnect and history witnesses.
- [ ] Run actual G1–G7 native scenarios at normal and changed window sizes. A browser-only story or compile-only API probe cannot replace native callbacks.
- [ ] Run a fresh complete comparable Phase9-style debug block for the changed shell: exact Tauri 40 then NSH 40, fresh Gateway/provider/session every round, same 10 warmups + 30 measured, 100ms real owned-resource sampling and original unchanged reducer. First ensure G1 resident behavior and explicit application-quit metric are distinct in the QA contract. OS-feature scenarios have additional evidence; do not insert extra events into the old schema ad hoc.
- [ ] Preserve every invalid/rejected block; retry entire affected blocks, never splice. Keep latency median ≤1.10×, p95 ≤1.20×, RSS median ≤1.10×, threads median delta ≤2 and p95 rank29. Do not carry old 185504 numbers forward as new measurements.
- [ ] Self-review: no competing Cargo/native tests during timed runs, all fixture roots cleaned, defaults restored exactly, process ownership and metrics reconstruct from retained evidence.

### Task 8 — Independent review and Phase10 handback / 独立审查与交付

**Files:** Create `docs/superpowers/runs/2026-11-18-phase10-closeout.md`, execution/acceptance/integrity receipts and final master gap matrix; update the locator/entry docs only where scope and evidence justify it.

**Interfaces:** G1–G7 evidence and accepted manifests → explicit per-gate verdict, ordered review reports and a reviewable diff.

- [ ] Perform independent SPEC then distinct-model QUALITY review of the current implementation and concrete evidence. Address real findings with actual validation; preserve retractions/corrections without rewriting original reports.
- [ ] Verify all linked files, identity hashes, source/default manifests, teardown evidence, original reducer result and final scope. Read actual test footers; do not sum nested logs or inflate test counts.
- [ ] Report each master phase separately. Phase10 complete means its approved G1–G7 gates passed, not full native UI parity, Plugin Runtime, release readiness or Tauri-removal readiness.
- [ ] Include changes, reasons, trade-offs, tested commands, remaining limitations and skipped checks. Keep all evidence and existing branches/worktrees. New remote push/release/default-switch requires its own authorization.
- [ ] Self-review: no PASS backed only by a reviewer assertion; no unresolved required gate; no historical QA verdict silently promoted to this tree's acceptance.

## 7. Evidence continuity and pitfalls / 证据续接与已知陷阱

Private local corpus (preserve, do not bulk-add binaries to Git) / 本机私有证据目录：

```text
.superpowers/sdd/2026-11-15-phase9-runtime-proof-and-dependency-closure/full-closure/
  isolated-matrix-40-20261118T185504/
    matrix.json
    measurement-source.json
    source-integrity.json
    restoration.json
    independent-integrity.json
    audit-program-execution.json
  final-audit-program/verify_current_matrix.py
  verify_final_handoff.py
  review-archive/
  final-review-note-reconciliation.md
  queue-projection-evidence.json
  queued-wire-20261118T145900/
  meter-fixes-20261118T155601/receipt.json
  quit-app-build-20261118T181620/receipt.json
```

The corpus is ignored/private and remains in the current checkout across a branch switch. It is **not automatically present in a fresh clone or another machine**. Committed acceptance/manifests summarize and hash it; a missing raw corpus is an evidence-availability issue, not permission to manufacture replacement receipts. Use new Phase10 outputs and target paths; do not mutate the immutable Phase9 auditor or old blocks.

Past mechanisms to retain / 需要记住的机制：

- Native capture on the external repository volume blocked `OpenOptions::open` before Tauri `builder.build`. Use owned local temporary capture, archive after exit. It was not a proved WebView construction hang.
- The shared native/collector monotonic clock and nonce/PID gate cover the launch boundary. Browser route/input/reconnect metrics use browser event timestamps, not IPC receive time; document time origin is captured once because WebKit getters can round differently.
- Input ends at the addressed `chat.send` response. New Chat may send a null session key; canonical response/run identity supplies the binding. Reconnect starts at the production reconnect-state commit and ends after replacement history application plus rendered frames.
- Bare LaunchServices coalition ID can represent one member; fork/join controlled proof exists. Host disappearance, tail-only members and stale known-absent WebKit records require the current strict validator; no broad name-only ownership or EPERM-as-absence shortcut.
- Public `proc_pidinfo` may return a full task record with zero RSS during exec. The accepted sampler preserves that actual value; PID identity admission and signal authority remain separate. Error/partial records still fail closed.
- All observed members must settle; `owned_drained` after `run_app` alone is insufficient. Unknown target WebKit in the final catalog cannot be dropped to obtain a PASS.
- Queue wire capture needs `stream.**`; production default busy input steers. The dedicated queue fixture disables existing `mid_turn_steering` only in its isolated root. The performance matrix retains the production default.
- The installed-service simulated execution transcript issue remains deferred. Inspect the known owners `src/gateway/execution_engine/simple.rs` and `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs` only if that separate scope is authorized.

**Not done by this planning document / 本文未做:** Phase10 implementation or API feasibility proof, OS permission changes, release/cross-platform/full-bundle acceptance, default client replacement, ADR/constitution changes, Plugin Runtime, Tauri removal or remote publication.
