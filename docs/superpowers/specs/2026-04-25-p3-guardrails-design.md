# P3 — Guardrails Facade YAGNI Retraction & Orphan Deletion

**Status**: Design approved 2026-04-25
**Phase**: P3 of harness-dissolution roadmap
**Branch**: `harness-dissolution` (worktree at `/Volumes/TBU4/Workspace/Aleph.harness-dissolution`)
**Parent roadmap**: `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md`
**Predecessors**: P0 (slim harness, merged), P1 (context consolidation), P4 (verification minimization)

---

## 1. Goal

Retract the roadmap's original P3 plan ("create `src/guardrails/` facade aggregating security + sandbox + permission + approval + pii, with InputGuard / OutputGuard / ToolCallGuard contracts") and replace it with two concrete actions:

1. **Delete the orphan `src/permission/` module** (zero external consumers since introduction).
2. **Document the retraction** in the parent roadmap (footnote ³ alongside P1's ¹ and P4's ²).

This makes P3 the third in a row of YAGNI-retraction phases, following the precedent established by P1 (deleted `src/compressor/`) and P4 (deleted `src/verification/verify_stop_hook.rs`).

### Anti-goals

- **No** `src/guardrails/` directory creation. The four live modules (`security`, `sandbox`, `approval`, `pii`) stay at the crate root.
- **No** `InputGuard` / `OutputGuard` / `ToolCallGuard` traits. No present consumer needs a unified guardrail interface, and inventing one violates R3 (Core Minimalism) and the YAGNI discipline.
- **No** physical move of `security` / `sandbox` / `approval` / `pii`. The four modules have distinct call sites and command-different consumer footprints; a parent directory adds 40+ import churn without solving any concrete pain.
- **No** consolidation of the parallel exec-approval implementations or the six `ApprovalDecision` types (see §7). That is a separate layered problem belonging to a future phase.

---

## 2. Code Census

Brainstorm-time audit of the five modules the roadmap originally listed for the guardrails facade.

### 2.1 `src/permission/` — ORPHAN

| Aspect | Finding |
|---|---|
| Size | 4 files, ~40K (`config.rs` 7K, `error.rs` 4K, `manager.rs` 14K, `rule.rs` 13K, `mod.rs` 1.6K) |
| Origin | Commit `1f7b33931` "feat(permission): add OpenCode-compatible permission and question system" (Apr 2026) |
| External consumers | **Zero**. `grep -rn 'crate::permission' src/` excluding `src/permission/` returns no matches. |
| Name-collision check | `PermissionRule` / `PermissionManager` / `PermissionAction` exist elsewhere as **independent types**: `crate::extension::types::agents::PermissionRule` (live), `RscPermissionManager` in `gateway/interfaces/msteams/rsc.rs` (live). Deleting `crate::permission::*` does not touch these. |
| Re-export risk | `mod.rs:51` re-exports `crate::event::permission::{PermissionAction, PermissionReply, PermissionRequest}`. The bodies in `crate::event::permission::*` stay intact; the indirection through `crate::permission::*` is what disappears, and zero consumers use that indirection. |

**Verdict**: Delete entirely, per P1 (`compressor`) and P4 (`VerifyStopHook`) precedent.

### 2.2 The four live modules — keep in place

| Module | External consumers | Surface |
|---|---|---|
| `src/security/` | 13 (gateway, mcp, tools, thinker, browser, executor) | HTTP security headers, SSRF protection, content sanitizer, persistent audit, `RuntimeSecurityGuard`, `ContextIdHasher` |
| `src/sandbox/` | 30+ (harness, gateway, agents, tools, executor, session, approval, orchestrator) | `Sandbox` trait, `WorkspaceSandbox`, OS drivers (macOS seatbelt), `exec_approval/` submodule, capabilities, policies |
| `src/approval/` | 3 (`builtin_tools/desktop`, `builtin_tools/pim`) | `ActionRequest`/`ActionType`/`ApprovalDecision`/`ApprovalPolicy` for desktop & browser action authorization (allow/deny/ask glob) |
| `src/pii/` | 2 (`providers/http_provider`, `security/runtime_guard`) | Gateway-level PII regex engine (precision-tuned, distinct from log-scrubber `utils::pii`) |

These four serve genuinely distinct domains. They share the conceptual label "guardrails" but have no shared interface and no overlapping call sites. A parent `src/guardrails/` directory would only add hierarchy without changing how anything is consumed.

### 2.3 Bonus fragmentation — out of P3 scope

The audit surfaced two pre-existing fragmentation issues that are **not** part of P3:

**Three parallel exec-approval implementations** (each at a different layer):
- `src/exec/approval/` — IPC/socket-based exec command approval pipeline (allowlist / full / deny / ask)
- `src/sandbox/exec_approval/` — in-process sandbox `ApprovalGate` with retry + parsing
- `src/tools/middleware/permission/` — tool-level `LayeredPermissionResolver` + `AgentPermissionFilter`

These are layered, not duplicated: they sit at IPC time, sandbox-entry time, and tool-call time respectively. Merging them is a non-trivial layered refactor that should be its own phase.

**Six `ApprovalDecision` types** (each in a distinct domain):
- `src/exec/decision.rs` — exec command approval enum
- `src/exec/socket.rs` — IPC `ApprovalDecisionType` variant
- `src/mcp/protocol.rs` — MCP wire protocol enum
- `src/sandbox/exec_approval/types.rs` — sandbox gate struct
- `src/gateway/handlers/secret_approvals.rs` — secret/vault approval enum
- `src/approval/types.rs` — desktop/browser action approval enum

These serve genuinely different shapes (IPC wire format vs in-process struct vs domain-specific enum) and are not accidental duplicates.

**Decision**: Defer both to a future phase (working name: P3b, or fold into P5 subagent work). P3's mission is the YAGNI retraction, not unrelated cleanup.

---

## 3. Key Decisions

| ID | Decision | Rationale |
|---|---|---|
| D1 | Delete `src/permission/` entirely (4 files, ~40K) | Zero external consumers; never wired in since introduction; P1/P4 dead-code precedent |
| D2 | Keep `src/security/`, `src/sandbox/`, `src/approval/`, `src/pii/` at crate root unchanged | Four distinct domains with distinct consumers; umbrella adds no value |
| D3 | No new traits (`InputGuard` / `OutputGuard` / `ToolCallGuard` retracted) | No present consumer; violates R3 + YAGNI |
| D4 | Defer 3-way exec-approval cleanup and 6-way `ApprovalDecision` cleanup to a future phase | Layered/domain-distinct, not name-conflict; out of P3 scope |

---

## 4. Commit Plan

Two atomic commits on `harness-dissolution`. Same verification bar as P0/P1/P4: `cargo check` + `cargo clippy -- -D warnings` after each commit, inheriting pre-existing P0 clippy exemptions.

### Commit 1 — `permission: delete orphan crate::permission module (0 consumers)`

**Files changed**:
- Delete: `src/permission/` (entire directory: `mod.rs`, `config.rs`, `error.rs`, `manager.rs`, `rule.rs`)
- Modify: `src/lib.rs` line 72 — remove `pub mod permission;`

**Verification**:
- `cargo check -p alephcore` → green (zero consumers, so no breakage expected)
- `cargo clippy -- -D warnings` → green (sustaining P0 exemptions only; deletion should not introduce new warnings)
- `grep -rn 'crate::permission' src/` → zero matches (confirms cleanness)

### Commit 2 — `docs(spec): mark P3 complete; record YAGNI retraction and orphan deletion in roadmap`

**Files changed**:
- Modify: `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md`
  - §3.3 row 9 (Guardrails) — add `³` after "Aggregate security/sandbox/permission/approval/pii", trim or delete the InputGuard/OutputGuard/ToolCallGuard claim
  - §4.2 P3 row — Risk 🟡 → 🟢³, Estimate `1.5 weeks` → `1–2 hours³`, Exit Artifact trimmed to "orphan `src/permission/` deleted; facade plan retracted"
  - §7 status table P3 row — `📋 Planned` → `✅ Complete | 2026-04-25 | 2026-04-25 | [design](../specs/2026-04-25-p3-guardrails-design.md) | (no plan needed — see commit log)`
  - Add new footnote ³ alongside ¹ (P1) and ² (P4)

**Verification**:
- `cargo check` not affected (docs-only commit)
- Visual diff review of roadmap

### Footnote ³ draft

> ³ **P3 YAGNI retraction + orphan deletion (2026-04-25)**: P3 brainstorm audited the five modules originally proposed for the guardrails facade. Findings: (a) `src/permission/` was orphan code — zero external consumers since its April 2026 introduction in commit `1f7b33931` — and was deleted per the P1 (`compressor`) / P4 (`VerifyStopHook`) precedent (dead code with zero consumers gets removed, not relocated); (b) the four live modules (`security`, `sandbox`, `approval`, `pii`) serve genuinely distinct domains with distinct consumer footprints, so a parent `src/guardrails/` directory was rejected as adding hierarchy without solving any pain; (c) the planned `InputGuard` / `OutputGuard` / `ToolCallGuard` traits had no present consumer and were retracted (R3 + YAGNI). A separate fragmentation finding — three parallel exec-approval implementations (`src/exec/approval/`, `src/sandbox/exec_approval/`, `src/tools/middleware/permission/`) and six distinct `ApprovalDecision` types across the codebase — is layered/domain-distinct rather than a name collision, and is deferred to a future phase. Risk downgraded 🟡 Medium → 🟢 Low; estimate shortened 1.5 weeks → 1–2 hours. See P3 design §2–§3 for details.

---

## 5. Verification

Same bar as P0/P1/P4:
- `cargo check -p alephcore` green after each commit
- `cargo clippy -- -D warnings` green after each commit (P0 exemptions inherited; no new warnings)
- `grep -rn 'crate::permission' src/` returns zero matches after Commit 1
- No `git push`. No `merge` to main. Commits stay on `harness-dissolution`.

---

## 6. Risks & Rollback

**Risk level**: 🟢 Low (matches P1, P4 retraction-class phases).

**Identified risks**:

1. **Hidden re-export consumer** — already mitigated by full-tree grep for `crate::permission` (zero matches). If a runtime path-dependent consumer surfaces post-deletion, `git revert` of Commit 1 fully restores the module.

2. **Name-collision false positive** — `PermissionRule`/`PermissionManager`/`PermissionAction` exist in other modules under those exact names. Verified: those are independent type definitions (`crate::extension::types::agents::PermissionRule`, `RscPermissionManager` in msteams). Their compilation is unaffected by deleting `crate::permission::*`.

3. **Test-only consumer** — also covered by the grep: zero matches across the entire `src/` tree, including the `tests/` subdirectories within modules.

**Rollback**: `git revert <commit-1-sha>` restores `src/permission/` exactly. The directory is preserved in git history at the commit-1 parent.

---

## 7. Future Work — Deferred Fragmentation

Out of P3 scope, but documented here so the next phase has a starting point.

**Three parallel exec-approval systems**:
- `src/exec/approval/` (IPC layer)
- `src/sandbox/exec_approval/` (in-process gate layer)
- `src/tools/middleware/permission/` (tool-call middleware layer)

These currently work together through layered composition. Whether they should remain three layers or collapse into fewer is a design question requiring its own brainstorm. Tentative phase name: **P3b — exec-approval consolidation** (or fold into P5 subagent work since subagent dispatch consumes all three layers).

**Six `ApprovalDecision` types**:
- `src/exec/decision.rs`
- `src/exec/socket.rs` (`ApprovalDecisionType`)
- `src/mcp/protocol.rs`
- `src/sandbox/exec_approval/types.rs`
- `src/gateway/handlers/secret_approvals.rs`
- `src/approval/types.rs`

Each occupies a different domain (IPC wire format, sandbox in-process, MCP protocol, secret/vault, desktop action). Unification (or explicit non-unification) is part of the same future phase.

---

## 8. Sequence

1. ✅ Brainstorm + design (this document)
2. → Plan written to `docs/superpowers/plans/2026-04-25-p3-guardrails.md` (next step — covers the 2 commits)
3. → Implementation: 2 commits via subagent-driven-development on `harness-dissolution`
4. → User decides merge timing for `harness-dissolution` after P3 lands
