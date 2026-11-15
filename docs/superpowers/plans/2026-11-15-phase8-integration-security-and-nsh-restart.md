# Phase8 Integration & Security and NSH Restart Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Preserve the previously selected serial native implementation plus independent SPEC/QUALITY review; do not ask to choose an execution method again.

**Goal / 目标:** From local `main`, turn the accepted bounded Capability Phase7 into a reviewed Capability Phase8 Integration & Security specification and executable implementation plan, while reconciling the separate NSH 0.5 migration against its master design. / 从本地 main 接续已验收 Phase7，形成可批准的 Phase8 设计与实施细案，并单独核实 NSH 迁移状态。

**Architecture / 架构:** Keep canonical Capability ownership, generation, policy and event authorities in Rust Core. NSH remains an I/O client through `shared/client` and `shared/protocol`; its shell owns OS integration, not server business logic. Treat Capability Phase8 and NSH Phases0–6 as separate deliverables with explicit dependencies. / 不新增平行 registry、ready map、会话 store 或权限权威。

**Trade-off / 代价:** This preserves lifecycle and process boundaries but requires explicit IPC, native event-loop/Tokio ownership, independent security review and native parity evidence. Old 0.4 spike results cannot remove that work; full native migration cannot be inferred from Core or browser tests. / 版本适配、原生退出和双端 parity 的成本必须实际承担。

**Tech Stack:** Rust/tokio/serde/schemars; canonical Capability Core; existing Gateway and shared client/protocol; current Leptos/WASM client retained; NSH target v0.5.0 at release commit `f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1`.

**Spec / 事实与设计入口:**

1. **NSH master / 总纲（全文必读）:** [2026-11-05-nsh-desktop-refactor-design.md](../specs/2026-11-05-nsh-desktop-refactor-design.md). It is still a draft with 0.4 API examples; it is not a newly accepted 0.5 implementation spec.
2. [Phase7 acceptance and main handoff](../runs/2026-11-15-phase7-acceptance-and-main-handoff.md), including portable review and source manifest.
3. [Approved readiness/continuation plan](2026-11-12-session-ui-readiness-and-phase7-continuation.md), especially “Subsequent phases”.
4. [Capability Phase7 master](2026-11-05-phase7-advanced-capabilities.md), which names Phase8 **Integration & Security**, and [I3 plan](2026-11-07-phase7-i3-production-integration.md).
5. [NSH 0.5 frozen audit](../runs/2026-11-15-phase7-closeout/nsh05-audit.md).

**NSH historical execution documents / NSH 历史执行资料（按实际内容对账，不按文件编号推定阶段完成）:**

| Document / 文档 | Use / 用途 |
|---|---|
| [Phase0 spike](2026-11-05-nsh-phase0-spike.md) and [API verification](2026-11-05-nsh-api-verification.md) | Historical compatibility/API claims; revalidate the selected 0.5 API |
| [Input limitations](2026-11-05-nsh-input-limitations.md) | Recheck old input limitations against the pinned new API |
| [Phase1 protocol/transport](2026-11-05-nsh-desktop-phase1-protocol-transport.md) and [Phase2 connection](2026-11-05-nsh-desktop-phase2-connection.md) | Audit actual shared-client transport, event consumers and supervisor; old numbering is not proof of master Phase2 OS parity |
| [Phase3 core UI](2026-11-05-nsh-desktop-phase3-ui-core.md) and [Phase3 technical debt](2026-11-05-nsh-desktop-phase3-technical-debt.md) | State/UI implementation, real session/event/history flow and recorded debt |
| [Phase4 capability alignment](2026-11-05-nsh-desktop-phase4-capability-alignment.md) | Historical Tool-only/revision assumptions; reconcile against current accepted I3 |
| [Spike findings](../../../desktop/nsh-spike/FINDINGS.md) | Actual old spike observations; not native 0.5 execution proof |

**Plan scope / 本计划边界:** Tasks 1–4 are the next session's authorized preparation, design and review work. They produce the concrete Phase8/NSH specifications needed before production implementation. Task 5 is a gate: a successful Phase7 review or this handoff is not human approval of an unwritten Phase8 design. / 下一会话可直接执行核查、设计和审查准备；真实生产实施须按明确设计取得批准，不重复索要已批准的合同 A 或本次 main 合并授权。

## Global Constraints / 全局约束

- Start from local `main`; determine the actual Git root with `git rev-parse --show-toplevel`. Paths in documents stay repository-relative. This user-directed main workflow overrides generic origin/dev/worktree defaults. Do not reset main, pull unrelated remote commits, or delete existing branches/worktrees/evidence.
- Read root `AGENTS.md`, then `docs/reference/SUBSYSTEM_ROUTING.md`, applicable subsystem instructions and `REDLINES.md` before proposing changes. The NSH master discusses a native UI architecture while the current constitution still describes Leptos/WASM; surface the exact ADR/R2 transition for human decision rather than silently overriding either.
- Contract A remains approved: each addressed-key success from real `chat.send`/`agent.run` follows committed canonical metadata; valid no-run and later rejection/cancellation/timeout/disconnection may leave a row. One storage latency is accepted. This does not promise a run slot, renderer readiness or durable simulated transcript.
- Never read, transmit, stage, overwrite, delete or commit `.claude/settings.json`, root `config.lock` or root `config.toml`. Runtime sandbox configuration and private credentials are not evidence to publish.
- Every source investigator first uses Graphify. Include this verbatim in any source-agent brief: **MANDATORY: graphify-out/graph.json exists. You MUST run graphify before reading source files. Use: `graphify query "<question>"` (scoped subgraph), `graphify explain "<concept>"`, or `graphify path "<A>" "<B>"`. Only read raw files after graphify has oriented you, or to modify/debug specific lines. This rule applies to subagents too — include it in every subagent prompt involving code exploration.**
- One checkout/source writer and one Cargo owner at a time. Native implementation and independent SPEC/QUALITY reviews are sequential. No unrequested external CLI agents. DeepSeek may only be Flash.
- Before **each** Cargo invocation: `python3 qa/browser_live/check_memory.py --required-bytes 4294967296`; allow only exit 0. Set an explicit isolated `CARGO_TARGET_DIR`, use `--offline --locked --jobs 1`, bounded owned PID/process groups, and retain natural exit/signal/cleanup receipts. A memory failure or unavailable offline dependency is a blocker to report, not permission to install/update globally.
- Behavioral RED must compile and actually fail because of the severed behavior. A compiler failure, timeout, cancelled flag, old binary or mock DOM is not that proof. Exact restoration and GREEN are required for authorized mutation controls.
- Coverage remains production-file **line** coverage at unchanged 80%; unmapped executable files are uncovered. Do not reuse the defective old parser or archived 90.724% label. New source changes require a corrected, reviewed measurement method before quoting new production coverage.
- No global installs/updates/trust, threshold relaxation, paid external LLM, push, publication, Phase8 implementation, NSH dependency migration or Tauri removal is authorized merely by this planning handoff. Obtain only the concrete permissions not already granted, at the final reviewable boundary.

## Review Focus / 重点失败形状

1. Foreign owner/scope, stale lease/provider/connection and queued cancellation: the exact production ingress rejects before effect, with cleanup still reachable. Task 2 owns the threat/evidence cases.
2. Canonical metadata readiness versus streamed result persistence: preserve Contract A; explicitly scope the server-reachable simulated transcript P2. Tasks 1–2 own this separation.
3. Event delivery, renderer acknowledgement and PTY operation replies: display/ack/input is a different contract from PTY attach/input/resize/close. Task 2 must identify the actual consumer, reconnect and teardown evidence.
4. NSH 0.4-to-0.5 API replacement and native shutdown: version-matched CLI/crates/runtime/host/types and real owned host/Tokio teardown are required. Task 3 owns this matrix.
5. UI migration completeness and trust: source directory existence, Core compilation and an upstream feature are not native parity or ADR acceptance; Phase4 plus two-week dual-run precede removal. Tasks 3–4 own these gates.

---

### Task 1: Re-establish the accepted main baseline / 核实 main 基线

**Files:** Read the handoff run, portable evidence under `docs/superpowers/runs/2026-11-15-phase7-closeout/`, and local original evidence under `.superpowers/sdd/2026-11-12-session-ui-readiness-and-phase7-continuation/`. Create `docs/superpowers/runs/2026-11-15-phase8-entry-inventory.md`.

**Interfaces:** Consumes the committed main tree and the v26 relative-path SHA-256 manifest; produces an evidence table with exact current HEAD, path/hash changes, retained gate IDs, missing artifacts and source-backed P2 scope.

- [ ] Run `git rev-parse --show-toplevel`, `git branch --show-current`, `git rev-parse HEAD`, `git status --short`, `git worktree list --porcelain`; verify the closeout commit is an ancestor. Record current HEAD rather than copying the old pre-commit HEAD.
- [ ] Rehash the 178 entries in the portable v26 source manifest plus the two JS inputs recorded in the run. If source differs, inventory the actual diff and affected gates; do not rerun all Phase7 work solely because the session restarted.
- [ ] Confirm accepted gates and counts: strict/workspace/build/fresh check natural 0; 27,649 outer passed, 0 failed, 56 ignored; corrected 66-file 14,311/16,193 = 88.37769406533687%; I3 25/25 listed scenarios; 16 production encoded resources; real PTY; controls and benchmark.
- [ ] Graphify and verify the installed-service simulation P2 at `src/gateway/execution_engine/simple.rs` and actual no-provider construction/mount at `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs`. Record Contract A separately from transcript guarantees; do not claim this code is library-only.
- [ ] Save the inventory. Expected: accepted unchanged source is carried forward with explicit P2s and unexecuted scopes; differing or missing evidence is named, not fabricated.

### Task 2: Author the concrete Capability Phase8 security/integration specification / 编写 Phase8 规格

**Files:** Create `docs/superpowers/specs/2026-11-15-phase8-integration-security-design.md` and `docs/superpowers/runs/2026-11-15-phase8-security-evidence-matrix.md`. Read current `src/capability/`, Gateway ingress, `shared/client`, `shared/protocol`, matching QA and subsystem references only after Graphify.

**Interfaces:** Consumes Task 1's frozen baseline/P2 table and current canonical descriptor/lease/ownership/policy/event interfaces. Produces a bounded threat model, exact ingress/owner/teardown map, explicit out-of-scope list and proposed public-interface changes. Existing signatures must be copied from current source, not invented in this handoff.

- [ ] Enumerate callers, trust boundaries, capability kinds actually mounted, resolver/admission/effect fire sites and terminal consumers. For each row, identify the current module/symbol, authority and live consumer.
- [ ] For foreign principal/scope, stale owner/provider generation, queued cancellation, reconnect/withdrawal and cleanup-after-disable, map a positive/negative/malformed/cancellation/teardown case to its real ingress and observable effect. Reuse valid existing fixtures; identify precisely which Phase8 behaviors still need new code.
- [ ] Disposition each P2: simulated installed-service transcript failure; per-store/process metadata serialization; PTY error presentation; coverage parser. Any proposed promotion to a required Phase8 fix needs a concrete failure contract and tests, not a new global registry or speculative universal scheduler.
- [ ] Define the renderer ACK/PTY reply boundary, production identity/sidecar provenance and local deterministic provider strategy. Same-port candidate routing, native NSH and remote deployment require their own explicit evidence if added to this phase.
- [ ] Write the spec with exact acceptance cases, source owners, public API/ADR decisions and exclusions. Expected: an independent reviewer can reject or accept a specific design without guessing its scope. No implementation in this task.

### Task 3: Reconcile NSH master Phases0–6 at v0.5 / 核实 NSH 总纲与真实进度

**Files:** Create `docs/superpowers/runs/2026-11-15-nsh05-migration-inventory.md`; read the **entire** NSH master and its Phase0/API/Phase1/Phase2/Phase3/technical-debt/Phase4 plans, `desktop/nsh-spike/FINDINGS.md`, actual `desktop/nsh-shell/` and shared client/protocol call sites. The initial handoff found `desktop/nsh-shell` present and `interfaces/nsh-desktop` absent; recheck rather than assuming.

**Interfaces:** Consumes the unchanged historical master, pinned 0.5 upstream audit and actual committed shell/transport/state code. Produces a phase-by-phase DONE/PARTIAL/NOT_PROVEN/DEFERRED matrix with proof and concrete next NSH slice. The matrix does not itself upgrade the draft master or accept ADRs.

- [ ] Freeze matching v0.5.0 release/tag/commit and locate local upstream source dynamically. Do not assume the absolute clone path printed in the old 0.4 master still exists. Record CLI, crates, runtime, platform host and JS/TS type identities as a matched set.
- [ ] Audit Phase0 compatibility: old 0.4 spike is history; 0.5 replaces window APIs with `WindowRef`, `<Window>` and window kinds. Include host channel/optional Tokio runtime ownership, secondary windows/shutdown, media grants and activation policy. Upstream floors: Rust 1.97, macOS14+, Windows10 1809+, Linux GTK4.14+ with WebKitGTK6.0 (2.40+).
- [ ] Audit Phase1 hello/events/session list/supervisor and Phase3 UI use of canonical keys/RPC/events/history. Look for dropped events, generated local session IDs and fake replies; a real transport alone does not prove real chat. Do not recreate merged phases blindly.
- [ ] Audit Phase2 window/tray/hotkey/deeplink/update/daemon/notification/single-instance and Phase4 Canvas/Settings/Browser/Panel/PhoneTablet parity. Separate Swift screenshot/voice/camera and i18n; master excludes them from the cited phase acceptance. Record actual source and native QA, not old line-count estimates.
- [ ] Locate the real ADR index and verify status rather than assuming `docs/adr` exists (it did not at handoff). Check master ADR-003/004 before Phase2 and ADR-005/006/007/008/009 before PluginRuntime, particularly 006 WASM and 008 trust/signing. Surface the current R2/native-UI constitutional transition for review.
- [ ] Propose the smallest missing NSH slice in its own spec/plan. Phase5 PluginRuntime has its own ADR prerequisites. Phase6 requires completed Phase4 parity **and two weeks dual-run**; do not remove Tauri, WebChat or workspace members now. Do not restart the deferred GPUI alternative.
- [ ] Save the matrix and native validation plan. Missing SDK/dependencies or unavailable platform means NOT_PROVEN with the exact blocker; no automatic global install, broad platform success or silent dependency/lock change.

### Task 4: Produce reviewable execution plans / 形成可执行细案并独立审查

**Files:** Create `docs/superpowers/plans/2026-11-15-phase8-integration-security-implementation.md`; if Task 3 identifies an actionable NSH gap, create a separate dated NSH slice spec/plan, not a combined server/UI rewrite. Create SPEC and QUALITY review records under `docs/superpowers/runs/`.

**Interfaces:** Consumes Tasks 2–3 designs and evidence matrices; produces exact Create/Modify/Test paths, current signatures, causal RED/GREEN cases, command/memory budgets, producer-consumer ownership, validation and rollback boundaries per task.

- [ ] Use writing-plans to split implementation into independently rejectable slices. For each slice list the real interface signatures and tests; determine them from the spec/current source instead of filling placeholders.
- [ ] Pin observable tests for the five Review Focus classes. Installed-service failures require terminal/idle cleanup tests without poisoning a process-global singleton; teardown must prove owned resources settled; a counter saying cancellation was requested is insufficient.
- [ ] Define focused tests, strict lint, complete default-feature regression when source changes warrant it, corrected production coverage, actual Gateway/renderer/PTY/native probes and final verification. Record ignored/skipped/unreached separately and label renderer retained-mutant replay accurately.
- [ ] Obtain independent SPEC then QUALITY review sequentially, with the verbatim Graphify instruction and unchanged-source hashes. Do not write source concurrently with reviewers or invent a reviewer if tools are unavailable. Repair substantive findings and retain earlier reports.
- [ ] Present the concrete reviewed designs/plans, trade-offs, exact proposed public API/ADR/dependency changes and any required one-time permission. Ask only for the outstanding human design/implementation approval. Expected: a reviewable approval target, not permission for unspecified work.

### Task 5: Approval and implementation gate / 批准与实施门槛

**Files:** Update the new phase ledger and reviewed specs with the actual decision; modify production code only within the subsequently approved slices.

**Interfaces:** Consumes explicit human approval of the reviewed Phase8 or NSH slice; produces the corresponding scoped implementation, not approval of the other track.

- [ ] Once the design is approved, implement sequentially with the preserved native + independent SPEC/QUALITY method. Continue ordinary authorized steps without asking “继续?” again.
- [ ] For NSH version/path/lock/constitutional changes, execute only the reviewed approved patch and actual native verification; do not borrow approval from Capability Phase7 or Phase8.
- [ ] At completion report source/build/runtime evidence, unresolved P2s, ignored/skipped/not-run cases and remaining platform/phase gates. Future commit/merge/push authorization must be based on that session's instructions; the 2026-11-15 instruction authorized the Phase7 closeout merge, not every later release.

## Author self-review / 本人自检

- [x] Scope: preserves Phase7 accepted status, approved metadata contract, actual P2 reachability and distinct Phase8/NSH gates.
- [x] Master coverage: maps NSH Phases0–6, version replacement, platform floors, ADRs, exclusions and dual-run; does not treat the historical draft as accepted 0.5 code.
- [x] Interfaces: design-stage outputs are concrete file paths and evidence tables; later production signatures are deliberately established by the new spec, not fabricated here.
- [x] Review Focus: each of the five failure classes has an owning task and observable validation requirement.
- [x] Execution handoff: same serial execution/review method; only new concrete design decisions require approval. No Phase8/NSH production implementation is performed while writing this plan.
