# P5 — Subagent Orchestration YAGNI Retraction Design

**Status**: Approved 2026-04-25
**Phase**: P5 (harness-dissolution roadmap)
**Predecessor phases**: P0 ✅ · P1 ✅ · P2 ✅ · P3 ✅ · P4 ✅
**Outcome**: Pure documentation retraction. Zero source-code changes.

---

## 1. Goals & Anti-goals

### Goals
- Document on the harness-dissolution roadmap that the originally-planned **4-way merge** of `src/agents/` + `src/teams/` + `src/orchestrator/` + `src/group_chat/` into `src/subagents/` **is not happening**.
- Explain *why* the merge and the proposed `SubagentOrchestrator` trait + Fork/Handoff/Graph mode taxonomy are over-abstraction the codebase does not currently demand.
- Leave a clearly-numbered footnote (⁵) so a future reader who proposes "just merge them already" has a written counter-argument with audit evidence.
- Mark P5 ✅ Complete (retracted) so subsequent phases (P6, P7) are unblocked and the status table no longer carries `📋 Planned` against P5.

### Anti-goals
- **Do not** physically merge any of the 4 directories. They stay where they are.
- **Do not** introduce `SubagentOrchestrator`, `Section`, `OrchestrationMode`, `Fork`, `Handoff`, or `Graph` traits or enums. Zero new abstractions.
- **Do not** create `src/subagents/` (not even as an empty placeholder — that would repeat the `src/prompt_assembly/` mistake P0 made and P2 had to delete).
- **Do not** edit `src/lib.rs`, any `mod.rs`, or any `.rs` file inside the 4 target directories. Source code is frozen.
- **Do not** delete anything. The earlier audit's "5 zero-impl traits" claim was wrong (every trait has a `Sqlite*` implementation); the "9 unimplemented!() in swarm" are intentional `#[cfg(test)]` mock stubs.

---

## 2. Code Census Evidence

This census is the load-bearing evidence behind the retraction. It must remain in the spec so future readers can verify the retraction was data-driven, not aesthetic.

### 2.1 Shape

| Directory | Files | LOC | mod.rs purpose |
|-----------|-------|-----|----------------|
| `src/agents/` | 32 | 17,503 | AgentDef + AgentRegistry + SubAgent trait + Rig-core runtime + swarm intelligence + thinking levels |
| `src/teams/` | 18 | 9,203 | Team / Member types + SQLite store + lifecycle + messaging + sessions + task artifacts + plans |
| `src/orchestrator/` | 21 | 4,163 | FlowSpec parsing + HarnessRunner trait + FlowRegistry + sandboxing + harness bridging |
| `src/group_chat/` | 8 | 5,770 | Multi-agent group chat + Personas + session orchestration + channel protocol + Telegram |
| **Total** | **79** | **36,639** | |

### 2.2 External consumers (the merge cost)

| Directory | External consumer files | Live runtime? | Notes |
|-----------|------------------------|----------------|-------|
| `src/agents/` | 27 | YES | thinking/providers, SubagentTool, task management tools, A2A, swarm coordination, gateway runtime |
| `src/teams/` | 19 | YES | 14 dedicated `team-*` builtin tools + swarm coordinator + teammates + gateway handlers |
| `src/orchestrator/` | 4 | YES | gateway/execution_engine (`engine.rs:962+`), `flow_admin.rs`, event_drain, helpers |
| `src/group_chat/` | 4 | YES | gateway handlers, Telegram channel integration, inbound router |

A 4-way physical merge would force ~70 import-site rewrites across `src/`, plus rewrites in workspace consumers (`interfaces/webchat/`, `desktop/`). All for zero behavioural payoff — the consumers don't care which directory the symbol lives in.

### 2.3 Internal coherence (rebuts "merge will clarify responsibilities")

The 4 directories already have **orthogonal responsibilities**:

- `agents/` = **runtime** (what an agent IS — definition, registry, dispatch loop)
- `teams/` = **lifecycle / persistence** (Team and Member as data, with SQLite-backed storage and history)
- `orchestrator/` = **flow composition** (declarative FlowSpec → execution graph for harness runs)
- `group_chat/` = **channel** (multi-agent IRC-style sessions over Telegram and other rich channels)

Collapsing four orthogonal axes into one directory does not clarify responsibilities — it dilutes them. A future reader looking at `src/subagents/agents/`, `src/subagents/teams/`, `src/subagents/orchestrator/`, `src/subagents/group_chat/` would be no better off than today.

### 2.4 Rebuttal of two earlier dead-code claims

The original P5 audit identified two suspected dead-code patches. Both were re-investigated and confirmed **not dead**:

**(a) "5 zero-impl traits in `teams/`"** — wrong. Every trait has a SQLite-backed implementation:

| Trait | Defined in | Live impl |
|-------|------------|-----------|
| `TeamStore` | `teams/store.rs:52` | `SqliteTeamStore` (`teams/store.rs:192`) |
| `ArtifactStore` | `teams/artifacts.rs:124` | `SqliteArtifactStore` (`teams/artifacts.rs:195`) |
| `EventLogStore` | `teams/events.rs:119` | `SqliteEventLogStore` (`teams/events.rs:233`) |
| `MessageStore` | `teams/messages/store.rs:57` | `SqliteMessageStore` (`teams/messages/store.rs:345`) |
| `InboxContextProvider` | `teams/context.rs:78` | `TeamInboxContextProvider` (`teams/context.rs:112`) |
| `SessionStore` | `teams/sessions/store.rs:41` | 3 impls: `SqliteSessionStore`, `FileSessionStore` (`gateway/session_store/file_backend/mod.rs:257`), `SessionManager` (`gateway/session_store/sqlite_backend/mod.rs:73`) |
| `CoordTaskStore` | (`agents/swarm/tasks/store.rs`) | `SqliteCoordTaskStore` (`agents/swarm/tasks/store.rs:258`) |

This is textbook trait + impl dependency-inversion (P4 design pattern from CLAUDE.md). Not dead, not awaiting cleanup.

**(b) "9 `unimplemented!()` in `agents/swarm/context_injector.rs:671-719`"** — also wrong. They are inside `#[cfg(test)] mod tests`'s `MockTaskStore`. The test function `test_inject_task_context_returns_formatted_list` only exercises `list_tasks`, so the other 9 trait methods are intentionally narrow stubs. This is a normal narrow-mock pattern, not unfinished implementation.

### 2.5 The "SubagentOrchestrator trait" pre-mortem

The original roadmap proposed introducing a `SubagentOrchestrator` trait above the 4 directories with explicit `Fork` / `Handoff` / `Graph` modes. Concrete observations:

- **Zero current consumers** ask for a unifying abstraction. Each spawn path today (`builtin_tools/sessions/spawn_tool.rs`, `builtin_tools/team/delegate.rs`, `a2a/`, `orchestrator/`) calls a concrete API directly. None of them dispatch through a `dyn SubagentOrchestrator`.
- **Each domain already has its own well-bounded trait**: `agents` has `SubAgent`, `teams` has 7 store traits, `orchestrator` has `HarnessRunner`, `group_chat` has `GroupChatRenderer` and `GroupChatCommandParser`. A second-layer trait above these is parallel abstraction — what R3 (Core Minimalism) explicitly forbids.
- **"Fork / Handoff / Graph" is post-hoc taxonomy, not real concepts in the code.** Today's spawn paths use specific verbs (delegate, hand off via A2A, group_chat orchestration) backed by specific tool implementations. Renaming them into three formal modes adds vocabulary cost without removing duplication, and the *prompt* — not a trait taxonomy — is where the LLM picks the right verb (R10: Intelligence in Prompt).

---

## 3. Key Decisions

**D1: P5 is documentation-only.** One spec doc + one roadmap-update commit. No source-code changes. Same shape as P3's retraction (`90c45e661` + `d4d4d6ddc`).

**D2: Reject `SubagentOrchestrator` trait.** Each of the 4 directories already provides domain-specific dependency-inversion traits (see §2.4). A second-layer unifying trait has zero current consumers and would violate R3 (Core Minimalism) by adding an abstraction the codebase doesn't ask for.

**D3: Reject 4-way physical merge.** The 4 directories' responsibilities are orthogonal (§2.3); merging them into `src/subagents/` would dilute boundaries and force ~70 import rewrites with no business payoff.

**D4: Reject Fork / Handoff / Graph mode taxonomy.** The spawn verbs in the codebase today (delegate / handoff / group orchestration) are concrete tool calls whose dispatch happens at the LLM layer via prompts (R10). Encoding the verbs as Rust enum variants is post-hoc naming, not de-duplication.

**D5: Do not create `src/subagents/`.** Even an empty placeholder would repeat the P0 → P2 anti-pattern (P0 created `src/prompt_assembly/`, P2 had to delete it). Better to never create.

**D6: Retraction ≠ "P5 is unimportant".** The 4 directories may eventually outgrow their independence. If a future 5th subagent shape (e.g., a long-running plan-and-act actor or a federated A2A-over-network mode) genuinely needs a unified entry-point, a future phase can revisit consolidation. To avoid slot confusion, that future phase should be named **P8-subagent-merge** (or similar), **not** "P5 round 2". P5 is closed.

**D7: Footnote ⁵ format mirrors ⁴ / ³ / ² / ¹.** Each prior phase has a footnote at the bottom of §4.2 that records (a) the audit findings, (b) what was retracted, (c) which redlines (R3/R8/R9/R10) it cites, (d) the risk/estimate downgrade, (e) where to look for the design spec. Footnote ⁵ follows the same skeleton.

**D8: Single commit for the roadmap edit.** Spec doc gets its own commit first (this file), then the roadmap edit gets its own commit. Same 2-commit pattern as P2.

**D9: Five edit sites in the roadmap:**
1. **§3.3 row 11** (module 11 — Subagent Orchestration) — Final Directory column changes from `src/subagents/` to "`src/{agents,teams,orchestrator,group_chat}/` (kept in place)⁵"; Action column changes from "Collapse 4 dirs" to a retraction summary.
2. **§4.2 P5 row** — Risk 🔴 High → 🟢 Low⁵; Estimate 3 weeks → 1 day⁵; Exit Artifact column changes from physical-merge description to "no merge — see footnote ⁵".
3. **§6 open question 3** (P5: `src/supervisor/` PTY relocation) — already resolved by P0 (`supervisor/` was renamed to `src/process_supervisor/` in commit `4f96f2d66`-era P0 work). Mark with ✅ Resolved (2026-04-24, by P0).
4. **§7 status table P5 row** — 📋 Planned → ✅ Complete (retracted); both date columns set to 2026-04-25; Spec column points to this design doc; Plan column is "(no plan needed — see commit log)".
5. **Insert footnote ⁵** after footnote ⁴ and before `### 4.3 Dependency Graph`.

---

## 4. Commit Plan

### Commit 1: this design doc

```
docs(spec): P5 subagents YAGNI retraction design
```

Single file: `docs/superpowers/specs/2026-04-25-p5-subagents-design.md`. Created. Committed.

### Commit 2: roadmap close-out

```
docs(spec): mark P5 retracted; document subagent dirs healthy as-is
```

Single file: `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md`. Five edits per §3.D9.

Estimated stat: 1 file changed, ~10 insertions, ~5 deletions.

### Verification bar

No source-code changes → no `cargo check` or `cargo clippy` requirement (P0/P1/P2/P3/P4 verification bar is inherited unchanged: still 7 alephcore lib warnings + 8 P0 clippy exemptions, no new ones because no source changed).

Visual verification only: `git diff` review confirms each of the 5 edits lands at the right location and renders coherently in markdown.

### Constraints

- Worktree: `/Volumes/TBU4/Workspace/Aleph.harness-dissolution`. Branch: `harness-dissolution`. Do NOT push. Do NOT merge to main. User decides merge timing.
- Commit messages: single-line `-m "..."` only (multi-line bodies trip the `block-no-verify@1.1.2` hook false-positive).
- Do NOT use `--no-verify` even if hooks complain — investigate the hook's underlying complaint.

---

## 5. Risk

🟢 Low. No source-code changes; no test impact; no consumer rewrites; no behaviour changes; only doc edits on the harness-dissolution branch (still unmerged, still under user-controlled merge timing).

The one residual risk is **future-proofing**: a future reader may want to revisit the merge despite this retraction. Footnote ⁵ explicitly acknowledges that and prescribes the path (a new phase named P8-subagent-merge, not a P5 redo). That keeps the door open without re-shrinking it.

---

## 6. Open Questions

None remaining. All P5-related questions in §6 of the parent roadmap are resolved by this design (the §6 question 3 about `src/supervisor/` was already resolved at P0 time; this design only needs to record the resolution).

---

## 7. References

- Parent roadmap: `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md`
- Prior retraction precedents:
  - P1: `docs/superpowers/specs/2026-04-24-p1-context-management-design.md` (compressor deletion)
  - P2: `docs/superpowers/specs/2026-04-25-p2-prompt-assembly-design.md` (4-module deletion ~5,200 LOC)
  - P3: `docs/superpowers/specs/2026-04-25-p3-guardrails-design.md` (permission deletion)
  - P4: `docs/superpowers/specs/2026-04-24-p4-verification-design.md` (VerifyStopHook deletion)
- Architectural redlines (CLAUDE.md): R3 (Core Minimalism), R8 (LLM Sovereignty), R9 (Everything is a Tool), R10 (Intelligence in Prompt)
