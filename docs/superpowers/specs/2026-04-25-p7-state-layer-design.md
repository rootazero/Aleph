# P7 — State Layer YAGNI Retraction + STATE_LAYER.md Doc

**Status**: Design (2026-04-25)
**Phase**: P7 of harness-dissolution roadmap
**Pattern**: doc-only investment (same shape as P6)

---

## 1. Goals & Anti-Goals

### Goals

1. **Close out P7** in the roadmap: §7 row → ✅ Complete; §4.2 row → updated
   risk/estimate/exit-artifact; §3.3 row 8 → ⁷ marker.
2. **Add a single new reference doc** `docs/reference/STATE_LAYER.md` (~120 lines,
   option β) capturing:
   - mod.rs comment is misleading — "resilience" middleware was deleted but
     the directory name was never updated
   - StateDatabase is a legitimate **single-connection multi-tenant SQLite
     Repository** (ApplicationRecord pattern), with 11 private submodules
     attaching methods via `impl StateDatabase { ... }`
   - 28 consumer files across 8 top-level domains
   - `types.rs` cross-domain types (`RiskLevel` 24 consumers, `Lane` 10, etc.)
3. **Add footnote ⁷** to roadmap (matching ¹/²/³/⁴/⁵/⁶ style) recording the
   YAGNI retraction rationale.

### Anti-Goals

1. ❌ No rename `src/resilience/` → `src/state/` (path A/C subsumed by D)
2. ❌ No splitting StateDatabase across domain directories (path B)
3. ❌ No code deletion — mod.rs's "Only … remain" wording is true, but
   "remain ⇒ deletable" is the roadmap assumption that audit refuted
4. ❌ No `src/state/` directory created (avoiding the P0→P2 placeholder
   anti-pattern, consistent with P5/P6)
5. ❌ No source-code edits / no import rewrites (0 LOC source change)
6. ❌ No future-failures section in STATE_LAYER.md (option β, same as P6
   BOOT_ASSEMBLY.md)

**Net effect**: 1 commit, 2 files (new STATE_LAYER.md + modified roadmap.md),
0 LOC source change, risk 🟢 Low, estimate 1 day. Same shape as P6.

---

## 2. Audit Evidence

### 2.1 — `src/resilience/` is NOT "gutted", it is 5,031 LOC of live code

| File | LOC | Content |
|---|---|---|
| `types.rs` | 682 | Cross-domain types (TaskStatus, RiskLevel, Lane, SessionStatus, AgentTask, TaskTrace, AgentEvent, SubagentSession) |
| `database/mod.rs` | 22 | 11 private submodules + `pub mod migration` + single pub re-export `StateDatabase` |
| `database/state_database/mod.rs` | 410 | StateDatabase struct + Connection management + sqlite-vec registration + 6 pub methods |
| `database/state_database/schema.rs` | 426 | DDL |
| `database/state_database/tests.rs` | 244 | Unit tests |
| `database/migration.rs` | 626 | Three add_* migrations (channel_offsets, paired_users, sticker_descriptions) |
| `database/memory_events.rs` | 635 | impl block — memory event store methods |
| `database/events.rs` | 245 | impl block — agent events |
| `database/sessions.rs` | 211 | impl block — sessions |
| `database/tasks.rs` | 233 | impl block — tasks |
| `database/group_chat.rs` | 177 | impl block — group_chat |
| `database/traces.rs` | 292 | impl block — traces |
| `database/paired_users.rs` | 60 | impl block — paired_users |
| `database/channel_offsets.rs` | 53 | impl block — Telegram offsets |
| `database/replay.rs` | 18 | impl block — replay helper |

**Total**: 5,031 LOC live code, 0 lines dead, all reached through the single
public `StateDatabase` type.

### 2.2 — mod.rs self-description is misleading

`src/resilience/mod.rs:1-5`:

```
//! Resilience Module — Database and Core Types
//!
//! Governance, collaboration, perception, and recovery middleware have been
//! removed as part of the agent loop migration. Only the database layer
//! (StateDatabase) and shared types remain.
```

Fact-check:

- "Governance / collaboration / perception / recovery middleware removed" ✅
  true (deleted by P0 agent loop dissolution)
- "Only … remain" implies leftover ⇒ deletable ✅ true that this is what
  remains, but **what remains is 5,031 LOC of single-connection multi-tenant
  store**, not "a few deletable lines"
- The name "resilience" is now misleading: the module contains **no retry /
  circuit-breaker / supervisor logic**, only SQLite persistence.

### 2.3 — StateDatabase is a legitimate God-Object pattern

Evidence:

- 1 × `Arc<Mutex<Connection>>` shared across 11 domains (events / memory_events /
  tasks / sessions / group_chat / traces / paired_users / channel_offsets /
  replay / sticker_descriptions / sqlite-vec)
- All 11 submodules are declared `mod xxx` (private), attaching methods to the
  same struct via `impl StateDatabase { ... }`
- Only public submodule: `pub mod migration` (called once from binary boot)
- 28 consumers all `use crate::resilience::StateDatabase`, with **zero files
  importing `crate::resilience::database::events` or any other submodule path**
  (grep result: 0)

→ This is the **ApplicationRecord pattern** (Rails / Django), expressed in
Rust. Strong encapsulation + single connection is the SQLite best practice in
concurrent applications (avoids multi-writer lock contention). Splitting it
would be an anti-pattern.

### 2.4 — Consumer distribution (28 files / 8 domains)

| Domain | File count | % |
|---|---|---|
| `gateway/` | 9 | 33% |
| `memory/` | 7 | 25% |
| `executor/` | 4 | 14% |
| `bin/` | 3 | 11% |
| `group_chat/` | 2 | 7% |
| `arena/` | 1 | 4% |
| `lib.rs` | 1 | 4% |
| `sync_primitives.rs` | 1 | 4% |

### 2.5 — types.rs cross-domain usage

| Type | Consumer files | Use |
|---|---|---|
| `RiskLevel` | 24 | Task risk level (most cross-domain) |
| `Lane` | 10 | Task lane |
| `AgentEvent` | 10 | Event persistence |
| `TaskStatus` | 9 | Task status enum |
| `SessionStatus` | 9 | Session status |
| `SubagentSession` | 3 | Long-lived subagent |
| `AgentTask` | 2 | Task struct |
| `TaskTrace` | 1 | Near-dead-code (single consumer) |

→ These are de facto crate-wide vocabulary. Physical migration cost vastly
exceeds benefit.

### Conclusion

Of the three exit-artifact items in the roadmap §4.2 P7 row:

1. "decide StateDatabase home" — assumes it needs a new home; audit shows the
   current home is functionally fine.
2. "delete gutted resilience/" — **factually wrong**: 5K LOC is fully alive,
   zero lines deletable.
3. "20+ consumers updated" — accurate count (28), but **the rename is mechanical
   churn with zero architectural value**.

→ Apply YAGNI retraction (path D, doc-only investment).

---

## 3. Key Decisions

### D1 — Retract "delete gutted `src/resilience/`"

5,031 LOC live + 28 consumers, zero deletable. Roadmap exit-artifact item (2)
based on a false premise.

### D2 — Retract "decide StateDatabase home"

Current home (`src/resilience/database/state_database/`) is functionally
complete. Both proposed candidates resolve no current pain point:

- `src/session/` already has its own `SessionEventStore` + `SessionActor::replay()`
  (confirmed by P6). Stuffing StateDatabase inside would force the session
  module to host 11 non-session domains' storage — breaks P2 high-cohesion.
- `src/state/` is a brand-new directory equivalent to path C (pure rename),
  zero architectural value.

### D3 — Retract "rename to `src/state/`"

28 consumers' import rewrites = mechanical churn. After audit, the original
roadmap goal of "deciding home" has lost meaning — home is already reasonable.
Do not create `src/state/` directory (avoiding P0→P2 placeholder anti-pattern,
consistent with P5/P6).

### D4 — Do not split StateDatabase across domains (path B)

11 private submodules sharing 1 × `Arc<Mutex<Connection>>` = standard
ApplicationRecord pattern. Splitting would:

- Introduce SQLite multi-connection lock contention on the same DB file
- Expand 28 consumers' import surface from `use ::StateDatabase` to
  `use ::EventsStore` + `use ::TasksStore` + … — strictly larger surface
- Convert strong encapsulation (all 11 submodules `mod xxx`, zero leak) into
  weak encapsulation

### D5 — Add `docs/reference/STATE_LAYER.md` (~120 lines, doc-only)

Single deliverable: codify the truth that "`src/resilience/` has a misleading
name + lives a healthy ApplicationRecord life" so the next engineer hitting
the misleading mod.rs comment finds the explanation in the doc, instead of
re-proposing rename / split / delete.

### D6 — STATE_LAYER.md scope = option β (same as P6 BOOT_ASSEMBLY.md)

- §1 "Misleading Name, Living Code" — module self-description vs reality
- §2 ApplicationRecord design rationale — 11 private submodules + single
  shared Connection
- §3 28 consumer distribution (gateway 9 / memory 7 / executor 4 / bin 3 /
  group_chat 2 / arena 1 / lib.rs 1 / sync_primitives.rs 1)
- §4 types.rs cross-domain types (RiskLevel 24 / Lane 10 / AgentEvent 10 /
  TaskStatus 9 / SessionStatus 9 / etc.)
- §5 References (roadmap §3.3 row 8 + footnote ⁷ + key file:line citations)
- ❌ **No** §6 future-failures / no chapter reserved for hypothetical futures

### D7 — Footnote ⁷ matches ¹–⁶ style

- Unicode-superscript U+2077 (⁷), not ASCII `7`
- Single long paragraph
- Opening: `⁷ **P7 YAGNI retraction + doc-only investment (2026-04-25)**:`
  (matching ⁶'s wording)
- Closing: `See P7 design §X for details.`

### D8 — Single-line commit message

`block-no-verify@1.1.2` hook has a false positive on multi-line bodies.
Locked template:

```
docs: P7 — retract resilience deletion + add STATE_LAYER.md doc
```

### D9 — Roadmap 4 surgical edits

1. **§3.3 row 8** (Module 8 — Error Handling): Action column appended with
   ⁷ marker + "(c) `src/resilience/` retained as-is — see note ⁷"
2. **§4.2 P7 row**: Risk 🟡 Medium → 🟢 Low⁷; Estimate 1.5 weeks → 1 day⁷;
   Exit Artifact rewritten
3. **§7 P7 row**: 📋 Planned → ✅ Complete + double 2026-04-25 dates + design
   link + "(no plan needed — see commit log)"
4. **Insert footnote ⁷** after footnote ⁶, before `### 4.3 Dependency Graph`

---

## 4. Commit Plan

Single commit, 2 files:

| File | Change | Stat |
|---|---|---|
| `docs/reference/STATE_LAYER.md` | NEW | ~120 insertions |
| `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md` | MODIFIED | ~5 insertions / ~3 deletions |

Commit message (single line):

```
docs: P7 — retract resilience deletion + add STATE_LAYER.md doc
```

**Verification bar** (inherits from P5/P6):

- 0 LOC source-code change → no `cargo check` / `cargo clippy` required
- Sole gate = visual `git diff` review + 4 roadmap edits one-by-one
- Working tree clean (`git status --short` empty after commit)
- Do not push, do not merge to main

---

## 5. Risk & Rollback

**Risk**: 🟢 Low — same level as P5/P6 (doc-only commit, no source-code
exposure).

**Rollback**:

```bash
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution revert <commit-sha>
```

This restores the roadmap §7 P7 row to 📋 Planned and deletes
`STATE_LAYER.md`. No source-code side effects.

---

## 6. References

- Parent roadmap: `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md`
- P6 design (same pattern): `docs/superpowers/specs/2026-04-25-p6-checkpoint-boot-design.md`
- Module under review: `src/resilience/` (5,031 LOC, 13 files)
- StateDatabase entry: `src/resilience/database/state_database/mod.rs:20`
- mod.rs self-description: `src/resilience/mod.rs:1-5`
- Migration entry: `src/resilience/database/migration.rs`
