# P0 Slim Harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Physically slim `src/harness/` from 16 files (~3712 lines) to 9 files (~1500 lines) by relocating non-core concerns to their target domain directories. Rename `src/supervisor/` → `src/process_supervisor/` and `src/resilient/` → `src/task_resilience/`. Zero semantic changes.

**Architecture:** Pure `git mv`-driven physical relocation. Skeleton directories (`src/prompt_assembly/`, `src/verification/`) are created empty except for `mod.rs` re-exports — they host no new traits (that's P2/P4's job). Six atomic commits, each with `cargo check -p alephcore` green before commit. Final smoke test: HTTP endpoint hit on running `aleph-server`.

**Tech Stack:** Rust, cargo, git, just.

**Parent Spec:** `docs/superpowers/specs/2026-04-24-p0-slim-harness-design.md`

---

## Task 0: Pre-P0 Baseline

**Files:**
- Working tree: 4 pre-existing uncommitted modifications (`interfaces/webchat/src/views/home.rs`, `src/gateway/inbound_router/command_handler.rs`, `src/harness/agent.rs`, `src/tasks/cron/executor.rs`)
- Untracked: 2 desktop-capability docs

**Purpose:** P0 demands a clean working tree so that each of the 6 commits shows only its own physical moves in `git diff`. Mixing unrelated edits invalidates the "pure rename" verification.

- [ ] **Step 1: Inspect current working tree state**

Run:
```bash
git -C /Volumes/TBU4/Workspace/Aleph status --short
git -C /Volumes/TBU4/Workspace/Aleph diff --stat
```
Expected: 4 modified files + 2 untracked docs.

- [ ] **Step 2: Stash uncommitted changes (default-safe disposition)**

The 4 modified files are pre-existing work unrelated to P0. Default action: stash them so P0 starts with a clean slate, and the user can restore them after P0 completes.

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
git stash push -m "pre-P0 uncommitted work (restore after P0)" -- \
  interfaces/webchat/src/views/home.rs \
  src/gateway/inbound_router/command_handler.rs \
  src/harness/agent.rs \
  src/tasks/cron/executor.rs
```

The 2 untracked docs (`docs/plans/2026-04-24-001-feat-linux-desktop-capability-enhancement-plan.md`, `docs/superpowers/specs/2026-04-24-linux-desktop-capability-enhancement-design.md`) are a parallel desktop-capability workstream — leave them untracked (they don't affect `cargo check`).

**Override**: if the user explicitly wants a different disposition (commit / revert), follow their instruction instead and skip the stash command above.

- [ ] **Step 3: Clean working tree**

After disposition decision, run:
```bash
git -C /Volumes/TBU4/Workspace/Aleph status --short
```
Expected: empty output (or only untracked files explicitly kept untracked).

- [ ] **Step 4: Verify baseline green**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
cargo check -p alephcore
```
Expected: no errors. **If this fails, STOP — fix the baseline before starting P0.**

- [ ] **Step 5: Record baseline hash**

Run:
```bash
git -C /Volumes/TBU4/Workspace/Aleph rev-parse HEAD
```
Write the commit SHA into a scratch note — this is the revert target if all of P0 needs to be rolled back.

---

## Task 1: Commit 1 — Relocate context_* → src/context/

**Files:**
- Move: `src/harness/context_budget/` (8 files: `autocompact.rs`, `context_collapse.rs`, `diagnostics.rs`, `microcompact.rs`, `mod.rs`, `pipeline.rs`, `preflight.rs`, `pressure.rs`) → `src/context/budget/`
- Move: `src/harness/context_compactor.rs` → `src/context/compact/compactor.rs`
- Modify: `src/context/mod.rs` (add two new module declarations)
- Create: `src/context/compact/mod.rs` (single-file new subdir needs its mod entry)
- Modify: `src/harness/mod.rs` (remove `pub mod context_budget; pub mod context_compactor;`)
- Modify: all consumers that import from the old paths (see Step 4 grep)

- [ ] **Step 1: Identify all consumers (fresh grep)**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
grep -rln "harness::context_budget\|harness::context_compactor" src/
```
Record the file list. Expected at least: `memory/compaction/orchestrator.rs`, `memory/compaction/session_summary_source.rs`, `memory/compaction/types.rs`, `harness/agent.rs`, `harness/deps.rs`, `harness/tests/task10_wiring.rs`.

- [ ] **Step 2: Create target skeleton subdirs**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
mkdir -p src/context/compact
```
(`src/context/budget/` is created implicitly by the `git mv` in Step 3.)

- [ ] **Step 3: Move the files with git mv**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
git mv src/harness/context_budget src/context/budget
git mv src/harness/context_compactor.rs src/context/compact/compactor.rs
```

- [ ] **Step 4: Create src/context/compact/mod.rs**

File: `src/context/compact/mod.rs`
```rust
//! Cross-turn context compaction (relocated from src/harness/ in P0).

pub mod compactor;
```

- [ ] **Step 5: Update src/context/mod.rs**

Add to the existing module declarations in `src/context/mod.rs`:
```rust
pub mod budget;
pub mod compact;
```
Place these after the existing `pub mod environment; pub mod memory_context; pub mod session_info;` lines.

- [ ] **Step 6: Update src/harness/mod.rs**

Remove these two lines from `src/harness/mod.rs`:
```rust
pub mod context_budget;
pub mod context_compactor;
```
Also remove any `pub use context_budget::...` or `pub use context_compactor::...` re-exports if present.

- [ ] **Step 7: Update all consumer imports**

For each file found in Step 1, replace:
- `crate::harness::context_budget` → `crate::context::budget`
- `crate::harness::context_compactor` → `crate::context::compact::compactor`
- `use crate::harness::context_budget::*` → `use crate::context::budget::*`
- `use crate::harness::context_compactor::*` → `use crate::context::compact::compactor::*`

Use `sed` carefully or edit manually. Example for one file:
```bash
sed -i '' 's|crate::harness::context_budget|crate::context::budget|g' src/memory/compaction/orchestrator.rs
sed -i '' 's|crate::harness::context_compactor|crate::context::compact::compactor|g' src/memory/compaction/orchestrator.rs
```
Apply to every file from Step 1.

- [ ] **Step 8: Verify with cargo check**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
cargo check -p alephcore
```
Expected: no errors. **If errors, fix import paths and re-check. Do not commit until green.**

- [ ] **Step 9: Verify diff is rename-dominated**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
git status --short
git diff --stat
```
Expected: `context_budget/*` and `context_compactor.rs` appear as `R` (renamed) entries; consumer files show small import-path-only edits.

- [ ] **Step 10: Commit**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
git add -A
git commit -m "harness: relocate context_budget/ + context_compactor → src/context/

Pure physical move as part of P0 harness dissolution. No semantic
changes. src/context/ now hosts budget/ and compact/ submodules
(skeleton; trait design deferred to P1).

Spec: docs/superpowers/specs/2026-04-24-p0-slim-harness-design.md"
```

---

## Task 2: Commit 2 — Carve Out prompt_assembly/ + verification/ Skeletons

**Files:**
- Move: `src/harness/sections/` (7 entries: `mod.rs`, 5 `.md` files, `guidance/` subdir) → `src/prompt_assembly/sections/`
- Move: `src/harness/stop_hooks.rs` → `src/verification/stop_hooks.rs`
- Move: `src/harness/verify_stop_hook.rs` → `src/verification/verify_stop_hook.rs`
- Create: `src/prompt_assembly/mod.rs`
- Create: `src/verification/mod.rs`
- Modify: `src/lib.rs` (register two new top-level modules)
- Modify: `src/harness/mod.rs` (remove three `pub mod` entries)
- Modify: all consumers (see Step 4 grep)

- [ ] **Step 1: Identify all consumers**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
grep -rln "harness::sections\|harness::stop_hooks\|harness::verify_stop_hook" src/
```
Record the file list.

- [ ] **Step 2: Create skeleton parent dirs**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
mkdir -p src/prompt_assembly
mkdir -p src/verification
```

- [ ] **Step 3: Move the files with git mv**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
git mv src/harness/sections src/prompt_assembly/sections
git mv src/harness/stop_hooks.rs src/verification/stop_hooks.rs
git mv src/harness/verify_stop_hook.rs src/verification/verify_stop_hook.rs
```

- [ ] **Step 4: Create src/prompt_assembly/mod.rs**

File: `src/prompt_assembly/mod.rs`
```rust
//! Prompt Assembly — relocated scaffolding (skeleton created in P0).
//!
//! Full trait design and consolidation with src/thinker/ and src/prompt/
//! is deferred to P2 (P2-prompt-assembly). This directory currently
//! hosts only the section content moved out of src/harness/.

pub mod sections;
```

- [ ] **Step 5: Create src/verification/mod.rs**

File: `src/verification/mod.rs`
```rust
//! Verification & Feedback — relocated scaffolding (skeleton created in P0).
//!
//! Full verification loop design (rule-based, visual, LLM-judge) is
//! deferred to P4 (P4-verification). This directory currently hosts
//! only the stop hooks moved out of src/harness/.

pub mod stop_hooks;
pub mod verify_stop_hook;
```

- [ ] **Step 6: Update src/lib.rs**

Add these two lines to `src/lib.rs` near the other top-level `pub mod` declarations (alphabetical position):
```rust
pub mod prompt_assembly;
pub mod verification;
```

- [ ] **Step 7: Update src/harness/mod.rs**

Remove these three lines from `src/harness/mod.rs`:
```rust
pub mod sections;
pub mod stop_hooks;
pub mod verify_stop_hook;
```
Also remove any `pub use` re-exports of these modules.

- [ ] **Step 8: Verify include_str! paths in sections/mod.rs still resolve**

Open `src/prompt_assembly/sections/mod.rs`. The `include_str!` macros use paths relative to the source file, so they should still work after the move (since all `.md` files moved with `mod.rs`). Confirm by inspection:
```bash
grep -n "include_str!" src/prompt_assembly/sections/mod.rs
```
All referenced paths (`task_philosophy.md`, `risk_actions.md`, `tool_grammar.md`, `output_style.md`, `persistence.md`, `guidance/*.md`) must exist at their new locations.

- [ ] **Step 9: Update consumer imports**

For each file found in Step 1, apply these replacements:
- `crate::harness::sections` → `crate::prompt_assembly::sections`
- `crate::harness::stop_hooks` → `crate::verification::stop_hooks`
- `crate::harness::verify_stop_hook` → `crate::verification::verify_stop_hook`

Example:
```bash
sed -i '' 's|crate::harness::sections|crate::prompt_assembly::sections|g' src/harness/agent.rs
sed -i '' 's|crate::harness::stop_hooks|crate::verification::stop_hooks|g' src/harness/agent.rs
sed -i '' 's|crate::harness::verify_stop_hook|crate::verification::verify_stop_hook|g' src/harness/agent.rs
```
Apply to every file from Step 1.

- [ ] **Step 10: Verify with cargo check**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
cargo check -p alephcore
```
Expected: no errors.

- [ ] **Step 11: Commit**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
git add -A
git commit -m "harness: carve out prompt_assembly/ + verification/ skeletons

Relocate harness/sections/ → prompt_assembly/sections/, and harness
stop hooks → verification/. Two new top-level skeleton modules are
created with minimal mod.rs (re-exports only, no new traits). P2
and P4 will fill in the consolidation and trait design later.

Pure physical move; no semantic changes.

Spec: docs/superpowers/specs/2026-04-24-p0-slim-harness-design.md"
```

---

## Task 3: Commit 3 — Relocate Remaining to Existing Homes

**Files:**
- Move: `src/harness/skill_prefetch.rs` → `src/skill/prefetch.rs`
- Move: `src/harness/provider_bridge.rs` → `src/providers/bridge.rs`
- Move: `src/harness/tool_execution_context.rs` → `src/tools/execution_context.rs`
- Move: `src/harness/tool_summary.rs` → `src/tool_output/summary.rs`
- Move: `src/harness/adapters/` (6 files: `builtin_adapter.rs`, `daemon_adapter.rs`, `mcp_adapter.rs`, `memory_adapter.rs`, `mod.rs`, `registry_adapter.rs`) → `src/tools/adapters/`
- Modify: target-domain `mod.rs` files (4 of them)
- Modify: `src/harness/mod.rs` (remove 5 `pub mod` entries)
- Modify: all consumers

- [ ] **Step 1: Identify all consumers**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
grep -rln "harness::skill_prefetch\|harness::provider_bridge\|harness::tool_execution_context\|harness::tool_summary\|harness::adapters" src/
```
Record the file list.

- [ ] **Step 2: Move the files with git mv**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
git mv src/harness/skill_prefetch.rs src/skill/prefetch.rs
git mv src/harness/provider_bridge.rs src/providers/bridge.rs
git mv src/harness/tool_execution_context.rs src/tools/execution_context.rs
git mv src/harness/tool_summary.rs src/tool_output/summary.rs
git mv src/harness/adapters src/tools/adapters
```

- [ ] **Step 3: Update src/skill/mod.rs**

Add to `src/skill/mod.rs` (near the existing module declarations):
```rust
pub mod prefetch;
```

- [ ] **Step 4: Update src/providers/mod.rs**

Add to `src/providers/mod.rs`:
```rust
pub mod bridge;
```

- [ ] **Step 5: Update src/tools/mod.rs**

Add to `src/tools/mod.rs`:
```rust
pub mod adapters;
pub mod execution_context;
```

- [ ] **Step 6: Update src/tool_output/mod.rs**

Add to `src/tool_output/mod.rs`:
```rust
pub mod summary;
```

- [ ] **Step 7: Update src/harness/mod.rs**

Remove these five lines from `src/harness/mod.rs`:
```rust
pub mod adapters;
pub mod provider_bridge;
pub mod skill_prefetch;
pub mod tool_execution_context;
pub mod tool_summary;
```
Also remove any `pub use` re-exports of these modules.

- [ ] **Step 8: Update consumer imports**

For each file found in Step 1, apply these replacements:
- `crate::harness::skill_prefetch` → `crate::skill::prefetch`
- `crate::harness::provider_bridge` → `crate::providers::bridge`
- `crate::harness::tool_execution_context` → `crate::tools::execution_context`
- `crate::harness::tool_summary` → `crate::tool_output::summary`
- `crate::harness::adapters` → `crate::tools::adapters`

Example for one consumer:
```bash
sed -i '' 's|crate::harness::skill_prefetch|crate::skill::prefetch|g' src/harness/agent.rs
sed -i '' 's|crate::harness::provider_bridge|crate::providers::bridge|g' src/harness/agent.rs
sed -i '' 's|crate::harness::tool_execution_context|crate::tools::execution_context|g' src/harness/agent.rs
sed -i '' 's|crate::harness::tool_summary|crate::tool_output::summary|g' src/harness/agent.rs
sed -i '' 's|crate::harness::adapters|crate::tools::adapters|g' src/harness/agent.rs
```
Apply to every file from Step 1.

- [ ] **Step 9: Verify with cargo check**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
cargo check -p alephcore
```
Expected: no errors.

- [ ] **Step 10: Verify P0 goal reached — 9 files in src/harness/**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
ls src/harness/*.rs
```
Expected output (exactly 9 files):
```
src/harness/agent.rs
src/harness/callback.rs
src/harness/chain_context.rs
src/harness/deps.rs
src/harness/loop_callback.rs
src/harness/mod.rs
src/harness/trace.rs
src/harness/trace_sink.rs
src/harness/trait_def.rs
```

- [ ] **Step 11: Commit**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
git add -A
git commit -m "harness: relocate remaining to existing homes — P0 target reached

Relocate skill_prefetch → src/skill/, provider_bridge → src/providers/,
tool_execution_context → src/tools/, tool_summary → src/tool_output/,
adapters/ → src/tools/adapters/.

src/harness/ now contains exactly 9 files: the thin orchestration core
(agent, deps, trait_def) plus shared editing/observability plumbing
(callback, loop_callback, trace, trace_sink, chain_context, mod).

Adapters target was corrected to src/tools/adapters/ during brainstorm —
they bridge tool sources (Builtin/MCP/Daemon/Memory), not prompt segments.

Pure physical move; no semantic changes.

Spec: docs/superpowers/specs/2026-04-24-p0-slim-harness-design.md"
```

---

## Task 4: Commit 4 — Rename src/supervisor/ → src/process_supervisor/ (W7)

**Files:**
- Rename: `src/supervisor/` → `src/process_supervisor/`
- Modify: `src/lib.rs`

- [ ] **Step 1: Confirm zero external consumers**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
grep -rln "crate::supervisor\|use crate::supervisor" src/ | grep -v "^src/supervisor/"
```
Expected: empty output (only the supervisor module's own files reference it).

- [ ] **Step 2: Rename the directory**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
git mv src/supervisor src/process_supervisor
```

- [ ] **Step 3: Update internal references inside the renamed module**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
grep -rln "crate::supervisor" src/process_supervisor/
```
For each match, replace `crate::supervisor` → `crate::process_supervisor` using sed or manual edit.

- [ ] **Step 4: Update src/lib.rs**

In `src/lib.rs`, replace:
```rust
pub mod supervisor;
```
with:
```rust
pub mod process_supervisor;
```

- [ ] **Step 5: Verify with cargo check**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
cargo check -p alephcore
```
Expected: no errors.

- [ ] **Step 6: Commit**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
git add -A
git commit -m "process_supervisor: rename src/supervisor/ → src/process_supervisor/

Rename for clarity: the module supervises OS-level PTY subprocesses,
not subagents. Aligns with P0 physical-slimming goal of removing
naming collisions ahead of P5's subagent orchestration consolidation.

Pure directory rename; zero external consumers; no semantic changes.

Spec: docs/superpowers/specs/2026-04-24-p0-slim-harness-design.md"
```

---

## Task 5: Commit 5 — Rename src/resilient/ → src/task_resilience/ (W8-partial)

**Files:**
- Rename: `src/resilient/` → `src/task_resilience/`
- Modify: `src/lib.rs`
- Modify: all consumers

- [ ] **Step 1: Identify all consumers**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
grep -rln "crate::resilient\|use crate::resilient" src/ | grep -v "^src/resilient/"
```
Record the file list (may be small — resilient is a task-level retry framework).

- [ ] **Step 2: Rename the directory**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
git mv src/resilient src/task_resilience
```

- [ ] **Step 3: Update references inside the renamed module**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
grep -rln "crate::resilient" src/task_resilience/
```
Replace `crate::resilient` → `crate::task_resilience` for every match.

- [ ] **Step 4: Update src/lib.rs**

In `src/lib.rs`, replace:
```rust
pub mod resilient;
```
with:
```rust
pub mod task_resilience;
```

- [ ] **Step 5: Update all consumer imports**

For each file found in Step 1:
```bash
sed -i '' 's|crate::resilient|crate::task_resilience|g' <file>
```

- [ ] **Step 6: Verify with cargo check**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
cargo check -p alephcore
```
Expected: no errors.

- [ ] **Step 7: Commit**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
git add -A
git commit -m "task_resilience: rename src/resilient/ → src/task_resilience/

Rename for clarity: the module provides task-level retry/fallback
(ResilientTask trait for cron-integrated work). The name collision
with src/resilience/ (storage) was confusing.

src/resilience/ (StateDatabase) cleanup deferred to new phase P7 —
it has 20+ consumers and is a real architectural decision.

Pure directory rename; no semantic changes.

Spec: docs/superpowers/specs/2026-04-24-p0-slim-harness-design.md"
```

---

## Task 6: Commit 6 — Docs Update + Final Smoke Test

**Files:**
- Modify: `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md` (mark P0 ✅ Complete in status table)

- [ ] **Step 1: Update roadmap status table**

Edit `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md` §7. Change the P0 row from:
```markdown
| P0 | 📋 Planned | — | — | [2026-04-24-p0-slim-harness-design.md](./2026-04-24-p0-slim-harness-design.md) | — |
```
to (fill in today's date and plan link):
```markdown
| P0 | ✅ Complete | 2026-04-24 | 2026-04-24 | [2026-04-24-p0-slim-harness-design.md](./2026-04-24-p0-slim-harness-design.md) | [2026-04-24-p0-slim-harness.md](../plans/2026-04-24-p0-slim-harness.md) |
```

- [ ] **Step 2: Run full test suite**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
just test-all
```
Expected: all tests pass. **If any test fails, diagnose — failures at this point usually mean an import path was missed or a `pub(crate)` visibility broke.**

- [ ] **Step 3: Run clippy with strict warnings**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
cargo clippy -p alephcore -- -D warnings
```
Expected: no warnings.

- [ ] **Step 4: Kill any running aleph-server processes**

Per project `CLAUDE.md` warning (vault data loss from concurrent instances):
```bash
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
sleep 2
ps aux | grep "[a]leph-server" | grep -v zsh | grep -v cp | grep -v tail
```
Expected: no aleph-server processes listed.

- [ ] **Step 5: Build and start aleph-server (smoke test)**

Run in one terminal (or as background process):
```bash
cd /Volumes/TBU4/Workspace/Aleph
cargo build -p aleph-server --release
./target/release/aleph-server start &
SERVER_PID=$!
sleep 10
```

- [ ] **Step 6: Hit a simple HTTP endpoint**

First, discover the gateway's listen port and an unauthenticated GET route:
```bash
cd /Volumes/TBU4/Workspace/Aleph
grep -rn "bind\|listen\|SocketAddr::" src/gateway/ src/bin/ | grep -iE "8080|3000|port" | head -5
grep -rn "\.route(" src/gateway/ | grep -iE "get\(|/health|/status|/version" | head -10
```

Then curl the discovered endpoint:
```bash
curl -sv -m 5 "http://127.0.0.1:<PORT><PATH>"
```
Expected: any HTTP 2xx response. The purpose is to confirm the server booted past module initialization — the response body is irrelevant.

**Success criterion**: connection accepted and at least one Think→Act cycle or route handler reachable. **Failure** here usually means a `lazy_static` / `OnceCell` init-order issue introduced by the moves (rare; pure physical relocation shouldn't cause it). If it fails, check the server's stderr for the panic location.

- [ ] **Step 7: Stop aleph-server**

Run:
```bash
kill $SERVER_PID 2>/dev/null
sleep 2
ps aux | grep "[a]leph-server" | grep -v zsh | grep -v cp | grep -v tail
```
Expected: no aleph-server processes.

- [ ] **Step 8: Commit the roadmap update**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
git add docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md
git commit -m "docs(spec): mark P0 slim-harness complete in roadmap

Roadmap §7 P0 status: Planned → Complete. Phase 0 delivered:
  - src/harness/ reduced from 16 files to 9 files
  - src/supervisor/ → src/process_supervisor/
  - src/resilient/ → src/task_resilience/
  - skeleton dirs for prompt_assembly/ + verification/ ready for P2/P4
  - full test suite + cargo clippy + HTTP smoke test all green

src/resilience/ StateDatabase relocation deferred to P7.

Plan: docs/superpowers/plans/2026-04-24-p0-slim-harness.md
Spec: docs/superpowers/specs/2026-04-24-p0-slim-harness-design.md"
```

- [ ] **Step 9: Final verification — count commits**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph
git log --oneline e33d22f73..HEAD
```
Expected: 6 commits with the prefixes `harness:` (×3), `process_supervisor:`, `task_resilience:`, `docs(spec):`.

- [ ] **Step 10: P0 complete — report to user**

Report success with:
- Line count before/after for `src/harness/` (`wc -l src/harness/*.rs`)
- Commit count and SHAs
- Confirmation that `cargo check`, `just test-all`, `cargo clippy`, and smoke HTTP all passed

---

## Rollback Procedure

If the phase needs to be abandoned mid-flight:

1. Identify the last green commit SHA (from Task 0 Step 5 or any intermediate green).
2. Reset: `git -C /Volumes/TBU4/Workspace/Aleph reset --hard <SHA>` (destructive — confirm no uncommitted work will be lost).
3. Or, for surgical rollback of one commit: `git -C /Volumes/TBU4/Workspace/Aleph revert <SHA>` (preserves history).

Commits 1-3 (harness relocations) have order dependencies — revert in reverse order (3 → 2 → 1). Commits 4, 5, 6 are independent and revertible individually.

## Notes for the Implementing Engineer

- **`git mv` is mandatory for file moves**, not `cp + rm`. It preserves rename detection in `git diff --stat`, which is how we verify "pure physical move".
- **Do not edit file contents during a move** except for import paths. If a file gains any other change, stop and commit it separately.
- **`cargo check -p alephcore` after every step**, not just before commit. If it breaks between steps, you know exactly which step introduced the issue.
- **The `sed -i ''` syntax is macOS-specific.** On Linux, use `sed -i ''` → `sed -i`. The dev machine here is macOS per `uname -s`.
- **Consumer grep is the ground truth.** The lists in Task 1/2/3 Step 1 are what you actually use; any earlier references in this plan are hints only. If the grep finds a file I didn't mention, update that file too.
- **If `cargo check` fails**, the error is almost always an import path you missed. Search the error message for `use crate::harness::`; that's your next edit.
