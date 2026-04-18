# Managed-Agents Phase 0 — Terminology Reset & Glossary — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename `AcpHarness` and all associated types to `AcpAdapter` terminology, freeing "Harness" for its Anthropic meaning (Think→Act loop) in future phases; create a canonical `GLOSSARY.md`; update top-level architecture docs.

**Architecture:** Pure rename + docs. No behavior changes. Rust type names, module paths, and directory names change in lockstep; each atomic rename is one commit to keep intermediate states green. User-facing TOML keys under `~/.aleph/config.toml` preserved via `#[serde(alias = "harnesses")]` so existing user configs keep working.

**Tech Stack:** Rust (workspace), `cargo check -p alephcore`, `cargo test -p alephcore --lib`, `just test-all`, `git mv`, serde with aliases.

**Source spec:** `docs/superpowers/specs/2026-04-18-managed-agents-refactor-roadmap.md` §7 Phase 0.

---

## Baseline Facts (verified 2026-04-18)

**Types to rename:**
- `AcpHarness` trait — defined `src/acp/harness.rs:~60`
- `HarnessMode` enum — defined `src/acp/harness.rs`
- `HarnessConfig` struct — defined `src/acp/session.rs`
- `AcpHarnessEntry` struct — defined `src/config/types/acp.rs`
- `GenericAcpHarness` struct — defined `src/acp/harnesses/generic.rs:57`
- `CustomHarness` struct — defined `src/acp/harnesses/custom.rs:22`

**Files/directories to move:**
- `src/acp/harness.rs` → `src/acp/adapter.rs`
- `src/acp/harnesses/` → `src/acp/adapters/`

**Occurrence surface:** 260 hits across 18 src files; 9 files in `src/acp/` mention "harness"; 7 doc files reference it.

**Out of scope (per spec):** `SandboxManager` rename (deferred to Phase 3). Do NOT touch `src/exec/sandbox/**` in this phase.

---

## File Structure After Plan

**Created:**
- `docs/reference/GLOSSARY.md` — canonical Anthropic-aligned definitions

**Renamed (content preserved, identifiers replaced):**
- `src/acp/adapter.rs` (was `harness.rs`)
- `src/acp/adapters/mod.rs` (was `harnesses/mod.rs`)
- `src/acp/adapters/generic.rs` (was `harnesses/generic.rs`)
- `src/acp/adapters/custom.rs` (was `harnesses/custom.rs`)

**Edited (identifier changes + doc updates):**
- All 18 src files currently referencing `AcpHarness*` / `HarnessMode` / `HarnessConfig`
- `src/config/types/acp.rs` — rename struct + field, add `#[serde(alias = "harnesses")]`
- `docs/reference/ARCHITECTURE.md` — terminology section
- `docs/reference/AGENT_SYSTEM.md` — harness references

---

## Task 1: Baseline Snapshot

**Files:** none created; produces a pre-rename occurrence record for later verification.

- [ ] **Step 1.1: Confirm working tree is clean**

Run: `git status`
Expected: `working tree clean` (or only the plan file modified). If other work is pending, stop and commit/stash first.

- [ ] **Step 1.2: Record pre-rename occurrence counts**

Run:
```bash
echo "=== AcpHarness / harness symbol counts (pre-rename) ===" > /tmp/phase0-baseline.txt
grep -rc 'AcpHarness\|HarnessMode\|HarnessConfig\|AcpHarnessEntry\|GenericAcpHarness\|CustomHarness' src/ | grep -v ':0$' >> /tmp/phase0-baseline.txt
echo "=== Files under src/acp/harness* ===" >> /tmp/phase0-baseline.txt
ls src/acp/harness.rs src/acp/harnesses/ >> /tmp/phase0-baseline.txt
cat /tmp/phase0-baseline.txt
```
Expected: non-empty output listing 18ish files and current directory structure. Keep this file — Task 10 diffs against it.

- [ ] **Step 1.3: Ensure baseline build is green**

Run: `cargo check -p alephcore 2>&1 | tail -20`
Expected: `Finished ... dev [unoptimized + debuginfo] target(s)` with no errors. If broken, stop — do not start renaming on a broken baseline.

- [ ] **Step 1.4: Ensure baseline tests pass**

Run: `cargo test -p alephcore --lib 2>&1 | tail -30`
Expected: all tests `ok` or `ignored`; no `FAILED`. If failing, stop — baseline must be green before renaming.

---

## Task 2: Create `docs/reference/GLOSSARY.md`

**Files:**
- Create: `docs/reference/GLOSSARY.md`
- Modify: `docs/reference/ARCHITECTURE.md` (add cross-link only)

- [ ] **Step 2.1: Write the glossary file**

Create `docs/reference/GLOSSARY.md` with exactly this content:

```markdown
# Aleph Glossary — Managed-Agents Aligned

Terminology in Aleph is aligned with Anthropic's managed-agents paradigm ([blog](https://www.anthropic.com/engineering/managed-agents)). This file is the single source of truth. If any other doc conflicts with this, this wins.

## Core terms

### Harness
**Anthropic meaning:** The loop that calls the LLM and routes tool calls to relevant infrastructure. Stateless; recoverable via `wake(session_id)` after crashes.

**Aleph today:** The Think→Act loop lives in `src/agent_loop/loop_core.rs` (pre-refactor) or `src/harness/` (post-Phase-4). No external-CLI meaning.

### Sandbox
**Anthropic meaning:** Execution environment where the agent runs code and edits files. Provisioned on-demand via `execute(name, input) → string`.

**Aleph today:** The agent-level `Sandbox` trait (post-Phase-3, `src/sandbox/`) is the workspace + capability-ledger abstraction. Implementations include `WorkspaceSandbox` (cwd + macOS seatbelt + approval gate).

**Do not confuse with:** `SandboxManager` / `ExecSecurityGate` / `ApprovalGate` — these are lower-level OS-sandbox primitives that sit *beneath* the `Sandbox` trait. Their names may change in Phase 3 for clarity.

### Session
**Anthropic meaning:** Append-only log recording everything that happened during an agent's work. Persists independently outside the harness; accessed via `getEvents()` / `emitEvent()`.

**Aleph today:** `SessionService` trait (post-Phase-1, `src/session/`), backed by an in-process tokio actor with SQLite persistence. Trait shape permits cross-process backends later.

### Tools
**Anthropic meaning:** The "hands" — custom tools, MCP servers, execution environments — all reached through one `execute()` surface. The brain is agnostic to the backing.

**Aleph today:** `ToolService` façade (post-Phase-2, `src/tools/`) unifies builtin / MCP / extension dispatch behind one `execute(name, input) → ToolOutput` call.

### Orchestrator
**Anthropic meaning:** Infrastructure managing session state, sandbox provisioning, and routing between brains and hands.

**Aleph today:** `src/orchestrator/` module (post-Phase-5). Owns session lifecycle + Harness dispatch + Sandbox provisioning + `FlowSpec` composition.

## Adapter terms (not Anthropic)

### AcpAdapter
**Aleph-specific:** A Rust adapter that bridges an external CLI tool (claude-code, codex, gemini-cli, opencode, …) to the Agent Client Protocol. Formerly called `AcpHarness`; renamed in Phase 0 to free "Harness" for its Anthropic meaning.

Defined in `src/acp/adapter.rs` (trait) and `src/acp/adapters/` (implementations).

### Brain / Hands
**Anthropic shorthand, used informally:**
- **Brain:** LLM + Harness
- **Hands:** Sandbox + Tools

## Phase reference

This glossary's forward-looking terms align with the 6-phase refactor roadmap: `docs/superpowers/specs/2026-04-18-managed-agents-refactor-roadmap.md`.
```

- [ ] **Step 2.2: Cross-link from ARCHITECTURE.md**

In `docs/reference/ARCHITECTURE.md`, add a line near the top (after the title) that reads:

```markdown
> **Terminology:** See [GLOSSARY.md](./GLOSSARY.md) for canonical Anthropic-aligned definitions of Harness, Sandbox, Session, Tools, Orchestrator, and AcpAdapter.
```

Exact location: directly after the first `# ` heading line.

- [ ] **Step 2.3: Verify the link renders**

Run: `grep -n 'GLOSSARY.md' docs/reference/ARCHITECTURE.md`
Expected: at least one line with the relative link.

- [ ] **Step 2.4: Commit**

```bash
git add docs/reference/GLOSSARY.md docs/reference/ARCHITECTURE.md
git commit -m "docs: add Aleph glossary with Anthropic-aligned terminology"
```

---

## Task 3: Rename `AcpHarness` trait → `AcpAdapter`

**Files:** every src file currently containing `AcpHarness` (baseline: 18 files). Touch all in one atomic commit — Rust trait renames must be atomic.

- [ ] **Step 3.1: Global exact-string replace across src/**

Run:
```bash
# macOS sed requires empty ''; use a temp approach that is cross-platform
grep -rl 'AcpHarness' src/ | while read f; do
  sed -i '' 's/AcpHarness/AcpAdapter/g' "$f"
done
```

(If on Linux: drop the `''` after `-i`.)

- [ ] **Step 3.2: Verify zero remaining `AcpHarness` in src**

Run: `grep -rn 'AcpHarness' src/`
Expected: zero output.

- [ ] **Step 3.3: Build**

Run: `cargo check -p alephcore 2>&1 | tail -30`
Expected: `Finished dev` with no errors. If errors appear:
- `unresolved import`: a module path also needs renaming; proceed to Task 7 before committing — or revert this task and do Tasks 3+7 combined.
- Any other error: revert with `git checkout -- src/` and investigate before retrying.

- [ ] **Step 3.4: Run tests**

Run: `cargo test -p alephcore --lib 2>&1 | tail -30`
Expected: same green result as baseline (Step 1.4).

- [ ] **Step 3.5: Commit**

```bash
git add -u src/
git commit -m "acp: rename AcpHarness trait to AcpAdapter

Free \"Harness\" for its Anthropic meaning (Think→Act loop).
Phase 0 of managed-agents refactor. Pure rename; no behavior change."
```

---

## Task 4: Rename `HarnessMode` → `AdapterMode`

**Files:** all files containing `HarnessMode` (subset of Task 3's surface).

- [ ] **Step 4.1: Global exact-string replace**

Run:
```bash
grep -rl 'HarnessMode' src/ | while read f; do
  sed -i '' 's/HarnessMode/AdapterMode/g' "$f"
done
```

- [ ] **Step 4.2: Verify zero `HarnessMode` in src**

Run: `grep -rn 'HarnessMode' src/`
Expected: zero output.

- [ ] **Step 4.3: Build + test**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib 2>&1 | tail -15`
Expected: all green.

- [ ] **Step 4.4: Commit**

```bash
git add -u src/
git commit -m "acp: rename HarnessMode to AdapterMode"
```

---

## Task 5: Rename `HarnessConfig` → `AdapterConfig`

**Files:** all files containing `HarnessConfig`.

- [ ] **Step 5.1: Global replace**

Run:
```bash
grep -rl 'HarnessConfig' src/ | while read f; do
  sed -i '' 's/HarnessConfig/AdapterConfig/g' "$f"
done
```

- [ ] **Step 5.2: Verify zero `HarnessConfig`**

Run: `grep -rn 'HarnessConfig' src/`
Expected: zero output.

- [ ] **Step 5.3: Build + test**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib 2>&1 | tail -15`
Expected: all green.

- [ ] **Step 5.4: Commit**

```bash
git add -u src/
git commit -m "acp: rename HarnessConfig to AdapterConfig"
```

---

## Task 6: Rename config entry type `AcpHarnessEntry` → `AcpAdapterEntry`

**Files:** `src/config/types/acp.rs` + every call site.

- [ ] **Step 6.1: Global replace**

Run:
```bash
grep -rl 'AcpHarnessEntry' src/ | while read f; do
  sed -i '' 's/AcpHarnessEntry/AcpAdapterEntry/g' "$f"
done
```

- [ ] **Step 6.2: Verify**

Run: `grep -rn 'AcpHarnessEntry' src/`
Expected: zero output.

- [ ] **Step 6.3: Build + test**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib 2>&1 | tail -15`
Expected: green.

- [ ] **Step 6.4: Commit**

```bash
git add -u src/
git commit -m "config: rename AcpHarnessEntry to AcpAdapterEntry"
```

---

## Task 7: Rename concrete impls `GenericAcpHarness` → `GenericAcpAdapter`, `CustomHarness` → `CustomAcpAdapter`

**Files:** `src/acp/harnesses/generic.rs`, `src/acp/harnesses/custom.rs`, plus call sites (manager.rs, tests.rs, etc.).

- [ ] **Step 7.1: Global replace for `GenericAcpHarness`**

Run:
```bash
grep -rl 'GenericAcpHarness' src/ | while read f; do
  sed -i '' 's/GenericAcpHarness/GenericAcpAdapter/g' "$f"
done
```

- [ ] **Step 7.2: Global replace for `CustomHarness`**

Run:
```bash
grep -rl 'CustomHarness' src/ | while read f; do
  sed -i '' 's/CustomHarness/CustomAcpAdapter/g' "$f"
done
```

- [ ] **Step 7.3: Verify zero occurrences**

Run: `grep -rn 'GenericAcpHarness\|CustomHarness' src/`
Expected: zero output.

- [ ] **Step 7.4: Build + test**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib 2>&1 | tail -15`
Expected: green.

- [ ] **Step 7.5: Commit**

```bash
git add -u src/
git commit -m "acp: rename Generic/Custom adapter impl structs"
```

---

## Task 8: Move file `src/acp/harness.rs` → `src/acp/adapter.rs`

**Files:**
- Move: `src/acp/harness.rs` → `src/acp/adapter.rs`
- Modify: `src/acp/mod.rs` (update `pub mod` declaration)
- Modify: every `use crate::acp::harness::*` call site

- [ ] **Step 8.1: Git-move the file**

Run: `git mv src/acp/harness.rs src/acp/adapter.rs`

- [ ] **Step 8.2: Update the module declaration in `src/acp/mod.rs`**

In `src/acp/mod.rs`, replace every occurrence of `harness` with `adapter` **only in `pub mod`, `pub use`, or `use` lines that reference the `harness` module**. Use this command:

```bash
sed -i '' 's|pub mod harness;|pub mod adapter;|g; s|pub use harness::|pub use adapter::|g; s|use crate::acp::harness::|use crate::acp::adapter::|g; s|use super::harness::|use super::adapter::|g' src/acp/mod.rs
```

- [ ] **Step 8.3: Update all cross-module imports**

Run:
```bash
grep -rl 'crate::acp::harness::' src/ | while read f; do
  sed -i '' 's|crate::acp::harness::|crate::acp::adapter::|g' "$f"
done
grep -rl 'super::harness::' src/ | while read f; do
  sed -i '' 's|super::harness::|super::adapter::|g' "$f"
done
```

- [ ] **Step 8.4: Verify zero `::harness::` module references**

Run: `grep -rn 'acp::harness::\|super::harness::' src/`
Expected: zero output.

- [ ] **Step 8.5: Build + test**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib 2>&1 | tail -15`
Expected: green.

- [ ] **Step 8.6: Commit**

```bash
git add -A src/
git commit -m "acp: move harness.rs to adapter.rs"
```

---

## Task 9: Move directory `src/acp/harnesses/` → `src/acp/adapters/`

**Files:**
- Move: entire `src/acp/harnesses/` directory → `src/acp/adapters/`
- Modify: `src/acp/mod.rs` (update `pub mod` declaration)
- Modify: every `use crate::acp::harnesses::*` call site

- [ ] **Step 9.1: Git-move the directory**

Run: `git mv src/acp/harnesses src/acp/adapters`

- [ ] **Step 9.2: Update the module declaration in `src/acp/mod.rs`**

```bash
sed -i '' 's|pub mod harnesses;|pub mod adapters;|g; s|pub use harnesses::|pub use adapters::|g; s|use crate::acp::harnesses::|use crate::acp::adapters::|g; s|use super::harnesses::|use super::adapters::|g' src/acp/mod.rs
```

- [ ] **Step 9.3: Update all cross-module imports**

Run:
```bash
grep -rl 'crate::acp::harnesses::' src/ | while read f; do
  sed -i '' 's|crate::acp::harnesses::|crate::acp::adapters::|g' "$f"
done
```

- [ ] **Step 9.4: Verify zero `::harnesses::` module references**

Run: `grep -rn 'acp::harnesses::' src/`
Expected: zero output.

- [ ] **Step 9.5: Also ensure no stray `harnesses` doc comments reference wrong paths**

Run: `grep -rn '// .*harnesses\|//! .*harnesses\|/// .*harnesses' src/acp/`
Read each hit: if it's a code reference (`src/acp/harnesses/...`), update to `src/acp/adapters/...`. If it's prose about the generic concept, leave as-is for now (Task 11 handles prose).

- [ ] **Step 9.6: Build + test**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib 2>&1 | tail -15`
Expected: green.

- [ ] **Step 9.7: Commit**

```bash
git add -A src/
git commit -m "acp: move harnesses/ directory to adapters/"
```

---

## Task 10: Add serde aliases for `~/.aleph/config.toml` back-compat

**Files:** `src/config/types/acp.rs` — add `#[serde(alias = "...")]` on any field whose Rust name changed from `harness*` to `adapter*`.

- [ ] **Step 10.1: Identify changed field names**

Run: `git log -p --since='1 hour' src/config/types/acp.rs | grep '^[+-].*\b\(harness\|adapter\)'`
Expected: shows the serde/field name diffs applied in Tasks 3–7. Record each renamed field.

- [ ] **Step 10.2: Identify struct fields that back TOML keys**

Run: `grep -n 'pub .*: .*\bVec<AcpAdapterEntry>\|pub adapters:\|pub adapter_' src/config/types/acp.rs`
Expected: at least one field, probably named `adapters` (post-rename) on a config-root struct. Record the line numbers.

- [ ] **Step 10.3: Add `#[serde(alias = "...")]` on each renamed field**

For each field `pub <new_name>:` whose TOML key changed from `harness*` to `adapter*` (i.e. where the Rust field was renamed in Task 6), add a `#[serde(alias = "<old_toml_key>")]` attribute on the line immediately before the field. Example, if Task 6 renamed a field `adapters: Vec<AcpAdapterEntry>` that used to be `harnesses: Vec<AcpHarnessEntry>`:

```rust
// Before (post-Task-6):
pub adapters: Vec<AcpAdapterEntry>,

// After:
#[serde(alias = "harnesses", default)]
pub adapters: Vec<AcpAdapterEntry>,
```

Use the Edit tool with exact context so each attribute lands on the right line. Do NOT use `sed` here — mis-targeted `sed` on a struct file is easy to break.

- [ ] **Step 10.4: Write a round-trip test for the alias**

Append to `src/config/types/acp.rs` under `#[cfg(test)] mod tests { ... }`:

```rust
#[test]
fn old_harnesses_toml_key_still_loads_as_adapters() {
    // Use whatever root struct carries the renamed field — replace
    // `AcpConfig` below with the actual struct name present in this file.
    let old_toml = r#"
        [[harnesses]]
        id = "claude-code"
        executable = "claude"
    "#;
    let cfg: AcpConfig = toml::from_str(old_toml).expect("old-key TOML must still parse");
    assert_eq!(cfg.adapters.len(), 1);
    assert_eq!(cfg.adapters[0].id, "claude-code");
}
```

If the root struct is not named `AcpConfig`, search for it with: `grep -n 'pub adapters:' src/config/types/acp.rs` and substitute the enclosing struct's name.

- [ ] **Step 10.5: Run the new test to verify it passes**

Run: `cargo test -p alephcore --lib old_harnesses_toml_key_still_loads 2>&1 | tail -10`
Expected: `test result: ok. 1 passed`. If it fails with "unknown field `harnesses`", the alias is missing; revisit Step 10.3. If it fails compilation, the struct name assumption was wrong — adjust.

- [ ] **Step 10.6: Run the full test suite**

Run: `cargo test -p alephcore --lib 2>&1 | tail -15`
Expected: all green, including the new test.

- [ ] **Step 10.7: Commit**

```bash
git add src/config/types/acp.rs
git commit -m "config: preserve old [[harnesses]] TOML key via serde alias"
```

---

## Task 11: Update top-level architecture docs

**Files:**
- Modify: `docs/reference/ARCHITECTURE.md`
- Modify: `docs/reference/AGENT_SYSTEM.md`

- [ ] **Step 11.1: Find every "harness" prose reference in ARCHITECTURE.md**

Run: `grep -n -i 'harness' docs/reference/ARCHITECTURE.md`
Expected: a small list. For each hit, decide:
- If the text describes ACP external-CLI integration → change to "adapter" or "ACP adapter"
- If the text describes the Think→Act loop (Anthropic meaning) → leave as "Harness" with a capital H, and add a `[harness](./GLOSSARY.md#harness)` link on first occurrence
- If the text is a code reference (`AcpHarness`, `src/acp/harness.rs`, etc.) → update to the renamed form

- [ ] **Step 11.2: Apply the edits**

Use the Edit tool per-hit with enough context to disambiguate. Do not use `sed` on docs — prose is easy to mangle. Each edit should be a deliberate reading of the surrounding sentence.

- [ ] **Step 11.3: Repeat for AGENT_SYSTEM.md**

Run: `grep -n -i 'harness' docs/reference/AGENT_SYSTEM.md`
Apply the same decision framework. Add GLOSSARY cross-links on first occurrence of each Anthropic term.

- [ ] **Step 11.4: Spot-check rendered output**

Open `docs/reference/ARCHITECTURE.md` and `docs/reference/AGENT_SYSTEM.md` in a markdown viewer (or `glow docs/reference/ARCHITECTURE.md` if installed). Read the changed sections for coherence. The terminology must match GLOSSARY.md.

- [ ] **Step 11.5: Commit**

```bash
git add docs/reference/ARCHITECTURE.md docs/reference/AGENT_SYSTEM.md
git commit -m "docs: update ARCHITECTURE and AGENT_SYSTEM for adapter terminology"
```

---

## Task 12: Final verification

**Files:** none modified; this is a gate before declaring Phase 0 complete.

- [ ] **Step 12.1: Grep verification — zero hits for old src identifiers**

Run:
```bash
grep -rn 'AcpHarness\|HarnessMode\|HarnessConfig\|AcpHarnessEntry\|GenericAcpHarness\|CustomHarness' src/
```
Expected: zero output. If any hit remains, it is a leftover — re-run the relevant Task's replace and commit as a fix-up.

- [ ] **Step 12.2: Grep verification — zero hits for old module paths in src**

Run:
```bash
grep -rn 'acp::harness::\|acp::harnesses::\|super::harness::\|super::harnesses::' src/
```
Expected: zero output.

- [ ] **Step 12.3: Confirm files moved**

Run:
```bash
test ! -f src/acp/harness.rs && echo "harness.rs gone OK"
test ! -d src/acp/harnesses && echo "harnesses/ gone OK"
test -f src/acp/adapter.rs && echo "adapter.rs present OK"
test -d src/acp/adapters && echo "adapters/ present OK"
```
Expected: all four "OK" lines printed.

- [ ] **Step 12.4: Run the full project test battery**

Before running anything that starts `aleph-server`, kill stale processes (per `CLAUDE.md` process-management warning):
```bash
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
sleep 2
```

Then:
```bash
just test-all 2>&1 | tail -50
```
Expected: all tests green. If anything fails, fix-up commit before proceeding.

- [ ] **Step 12.5: Run clippy**

Run: `just clippy 2>&1 | tail -30`
Expected: zero warnings in the renamed modules. If clippy complains about the new names (e.g. doc-comment mismatches), apply a fix-up commit.

- [ ] **Step 12.6: Confirm user config back-compat by smoke test**

Create a temp TOML file and run the round-trip test one more time:
```bash
cat > /tmp/phase0-smoke.toml <<'EOF'
[[harnesses]]
id = "claude-code"
executable = "claude"
EOF
cargo test -p alephcore --lib old_harnesses_toml_key 2>&1 | tail -5
```
Expected: `test result: ok. 1 passed`.

- [ ] **Step 12.7: Write CHANGELOG entry**

Open `CHANGELOG.md`. Under the `## [Unreleased]` section (or create one at the top if absent), add under `### Changed`:

```markdown
- **ACP:** renamed `AcpHarness` trait and family to `AcpAdapter` to free "Harness" for its Anthropic managed-agents meaning. Old `[[harnesses]]` TOML key remains accepted for one release via serde alias.
```

- [ ] **Step 12.8: Commit CHANGELOG**

```bash
git add CHANGELOG.md
git commit -m "changelog: note AcpHarness→AcpAdapter rename"
```

- [ ] **Step 12.9: Release (explicit user decision)**

Phase 0's spec exit criterion mentions "One release shipped". Releasing is a user-facing action — do NOT auto-release. Present the option to the user:

> "Phase 0 implementation complete and all commits green. Ready to run `just release $(date +%Y.%m.%d)` to ship? (This triggers the 4-platform GitHub build workflow.) Say 'release' to proceed, or 'hold' to defer."

Only on explicit "release" confirmation, run:
```bash
just release $(date +%Y.%m.%d)
```

---

## Non-Goals (Explicitly Out of Scope)

- `SandboxManager` / `ExecSecurityGate` / `ApprovalGate` rename — deferred to Phase 3
- Any behavior changes in adapter lifecycle, ACP protocol, or session handling
- Changes to `src/agent_loop/` (owned by Phase 4)
- New features, tests, or refactoring beyond the rename

## Rollback

If any task's build/test gate fails and the cause isn't obvious within 10 minutes:
```bash
git reset --hard HEAD~<N>   # where <N> = number of Phase 0 commits to undo
```
Then re-read the spec, reconsider, possibly adjust the plan, and retry. Per user CLAUDE.md — `git reset --hard` is destructive; only invoke it with explicit user confirmation.

## Done-ness Signals

Phase 0 is **done** when:
1. All 12 tasks above are checked off
2. `git log --oneline main..HEAD` shows ~11 small commits under the Phase 0 umbrella
3. `grep -rn 'AcpHarness' src/` returns zero lines
4. `just test-all` is green
5. `GLOSSARY.md` exists and is cross-linked from `ARCHITECTURE.md`
6. CHANGELOG has the rename entry
7. (Optional) A release was cut

Proceed to **Phase 1 brainstorming** only after all signals are green.
