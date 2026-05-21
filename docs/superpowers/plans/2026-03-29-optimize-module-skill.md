# Optimize-Module Skill Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create a Claude Code skill that autonomously optimizes Aleph Rust modules through a commit/verify/keep-or-reset loop, guided by a 6-dimension checklist and R1-R10 redlines.

**Architecture:** Pure Skill Markdown — no shell scripts, no external deps. SKILL.md contains full agent instructions (the "program.md"). A separate optimization-checklist.md provides the 6-dimension reference. Output goes to `optimize-results/` in the repo root.

**Tech Stack:** Claude Code Skill (Markdown), Bash (cargo, git, jq), TSV state files

**Spec:** `docs/specs/2026-03-29-optimize-module-skill-design.md`

---

## File Structure

```
/Users/zouguojun/Workspace/Skills/optimize-module/
├── SKILL.md                              # Main skill — full agent loop instructions
└── references/
    └── optimization-checklist.md         # 6-dimension checklist with examples
```

---

### Task 1: Create skill directory and SKILL.md

**Files:**
- Create: `/Users/zouguojun/Workspace/Skills/optimize-module/SKILL.md`

- [ ] **Step 1: Create skill directory**

```bash
mkdir -p /Users/zouguojun/Workspace/Skills/optimize-module/references
```

- [ ] **Step 2: Write SKILL.md with frontmatter and overview**

Create `/Users/zouguojun/Workspace/Skills/optimize-module/SKILL.md` with:

```markdown
---
name: optimize-module
description: Autonomous code optimization for Aleph Rust modules. Iterates commit/verify/keep-or-reset loop per module guided by 6-dimension checklist and R1-R10 redlines. Use when user says "optimize module", "优化模块", "代码优化", "自主优化", or wants systematic code improvement.
---

# Autonomous Module Optimizer

Systematic code optimization for Aleph's `src/` modules. Inspired by autoresearch: minimal system + strong instructions = maximum agent capability.

**Philosophy**: You are the agent. This skill is your program.md. Git is your state machine. Each optimization is a commit — good results advance, bad results reset. Deletions are preferred over additions (Occam's Razor).

## Parse Arguments

Determine from the user's request:
- **Module**: specific module name, or `--all` for full sweep
- **Resume**: `--resume` to continue interrupted run
- **Dimension**: `--dim N` to run only dimension N (1-6)
- **Dry-run**: `--dry-run` to analyze without modifying
- **List**: `--list` to show discovered modules only

## Initialization

Run these checks ONCE at the start of every invocation:

1. Verify repo root:
   ```bash
   test -d src/ || echo "ERROR: must run from Aleph repo root"
   ```

2. Ensure output directory exists and is gitignored:
   ```bash
   mkdir -p optimize-results
   grep -q 'optimize-results/' .gitignore 2>/dev/null || echo 'optimize-results/' >> .gitignore
   ```

3. Pre-flight compile check:
   ```bash
   cargo check -p alephcore
   ```
   If this fails → ABORT with "Pre-existing compile error. Fix before optimizing."

## Module Discovery

Auto-discover targets under `src/`:

```bash
# Module directories (exclude bin/)
find core/src -maxdepth 1 -mindepth 1 -type d ! -name bin | sort

# Standalone .rs files (exclude lib.rs, main.rs)
find core/src -maxdepth 1 -name '*.rs' ! -name 'lib.rs' ! -name 'main.rs' | sort

# Bin targets
find src/bin -maxdepth 1 -mindepth 1 -type d | sort
```

**Naming convention**:
- Directory: `gateway`, `memory`, `thinker`
- Standalone: `standalone:sync_primitives.rs`
- Bin: `bin:aleph-server`

**Path resolution**:
- `gateway` → `src/gateway/`
- `standalone:sync_primitives.rs` → `src/sync_primitives.rs`
- `bin:aleph-server` → `src/bin/aleph-server/`

**Sorting (--all mode)**: Sort by total .rs line count ascending. Small modules first for fast wins.

```bash
# Get line count for a module
find <module_path> -name '*.rs' -exec cat {} + | wc -l
```

If `--list` was requested, print the sorted list with total .rs line count per module, then STOP.

## Resume Logic

If `--resume` is specified:
1. Read `optimize-results/_progress.tsv`
2. For each module:
   - `done` → skip
   - `in_progress` → read `last_dim` column, resume from dimension `last_dim + 1`. The WHILE loop for each resumed dimension restarts fresh (re-scan for opportunities from the beginning of that dimension).
   - `pending` or not listed → start normally

## Core Optimization Loop

```
FOR each module (discovered or user-specified):

  # Per-module pre-flight
  Run: cargo check -p alephcore
  If FAIL → record "SKIPPED" in _progress.tsv, move to next module

  # Take baseline
  CLIPPY_BASELINE: run clippy JSON command (see Evaluation Pipeline)
  MODULE_LINES_START: count total .rs lines in module

  # Initialize module tracking
  kept_count = 0
  discarded_count = 0
  Record module as "in_progress" in _progress.tsv

  FOR dim = 1 to 6 (or single dim if --dim specified):
    consecutive_discards = 0

    WHILE true:
      # 1. Read module code, identify next opportunity for this dimension
      #    Refer to references/optimization-checklist.md for what to look for
      #    If dim 5 (Redline Audit): REPORT ONLY, do not edit code
      # 2. If no opportunity found → break
      # 3. If this specific opportunity was already tried and discarded → skip it

      # For dim 5 (Report Only):
      #   Record finding in _summary.md with file:line reference
      #   Do NOT edit, commit, or verify — just report and continue scanning

      # For all other dimensions:
      # 4. Apply change using Edit tool
      # 5. Stage changed files: git add <files>
      # 6. Record safe rollback point:
      BEFORE_SHA=$(git rev-parse HEAD)
      # 7. Commit:
      git commit -m "<module>: optimize <dim_name> - <short description>"
      # 8. Run Evaluation Pipeline (see below)
      # 9. If PASS:
      #      Update clippy baseline
      #      kept_count += 1
      #      consecutive_discards = 0
      #      Record "keep" in module TSV
      #    If FAIL:
      #      git reset --hard $BEFORE_SHA
      #      discarded_count += 1
      #      consecutive_discards += 1
      #      Record "discard" in module TSV
      # 10. If consecutive_discards >= 2 → break (dimension exhausted)

    Update last_dim in _progress.tsv

  # Module complete
  Record module as "done" in _progress.tsv with final stats
  Append module summary to _summary.md
```

**CRITICAL behaviors**:
- NEVER pause to ask "should I continue?" — run until checklist exhausted
- Each commit is atomic — one logical change only
- If cargo check crashes: diagnose quickly. Trivial fix → fix and retry. Fundamental → discard and move on.
- Each opportunity is tried at most once. Do NOT re-attempt a discarded change.

**Worktree recommendation**: For `--all` runs, suggest the user run in a git worktree and squash-merge back to main when done.

## Evaluation Pipeline

Run after EVERY commit. All steps must pass to keep the change.

**Prerequisite**: `BEFORE_SHA=$(git rev-parse HEAD)` must have been captured before `git commit` in the core loop.

### Step 1: Compile check
```bash
cargo check -p alephcore
```
FAIL → `git reset --hard $BEFORE_SHA`

### Step 2: Test
```bash
cargo test -p alephcore --lib
```
FAIL → `git reset --hard $BEFORE_SHA`

### Step 3: Clippy comparison

Baseline command (run once per module start, update after each keep):
```bash
cargo clippy -p alephcore --message-format=json 2>/dev/null \
  | jq -r 'select(.reason=="compiler-message") | select(.message.level=="warning") | .message.code.code // empty' \
  | sort | uniq -c | sort -rn > /tmp/clippy_baseline.txt
```

After change: same command → `/tmp/clippy_after.txt`

Compare:
```bash
comm -13 <(awk '{print $2}' /tmp/clippy_baseline.txt | sort) \
         <(awk '{print $2}' /tmp/clippy_after.txt | sort)
```
Any output = new clippy warning introduced → `git reset --hard $BEFORE_SHA`

### Step 4: Line count comparison (skip for dim 3 and dim 5)

```bash
# Changed files in this commit
git diff --name-only HEAD~1 HEAD | grep '\.rs$'

# Before (from parent commit)
git show HEAD~1:<file> | wc -l   # sum for all changed files

# After (working tree)
wc -l <file>                      # sum for all changed files
```
net_delta > 0 → `git reset --hard $BEFORE_SHA`

**Exception**: Dimension 3 (Large File Split) and Dimension 5 (Redline Audit) are exempt from the line count check — structural improvements.

## dry-run Mode

When `--dry-run` is active:
- Read code and identify opportunities per dimension normally
- Output findings to `optimize-results/<module>-dryrun.md` (one section per dimension)
- Do NOT edit files, commit, or run any git commands
- Format: list each opportunity with file:line and description

## State Files

### optimize-results/_progress.tsv
```
module	status	last_dim	kept	discarded	clippy_delta	lines_delta	started_at	finished_at
```

### optimize-results/<module>.tsv
```
commit	dimension	action	status	clippy_delta	lines_delta	description
```

### optimize-results/_summary.md
Append after each module completes:
```markdown
## <module> (<kept> kept / <discarded> discarded, <clippy_delta> clippy, <lines_delta> lines)
- Description of each kept change
- [Dim 5 findings if any: redline violations with file:line]
```
```

Verify the file was created correctly:

```bash
wc -l /Users/zouguojun/Workspace/Skills/optimize-module/SKILL.md
```

Expected: ~200 lines

- [ ] **Step 3: Verify SKILL.md renders correctly**

Read back the file to confirm no formatting issues.

---

### Task 2: Create optimization-checklist.md reference

**Files:**
- Create: `/Users/zouguojun/Workspace/Skills/optimize-module/references/optimization-checklist.md`

- [ ] **Step 1: Write the 6-dimension checklist**

Create `/Users/zouguojun/Workspace/Skills/optimize-module/references/optimization-checklist.md` with:

```markdown
# Optimization Checklist (6 Dimensions)

Execute in order. Low-risk first, high-risk last.

## Dimension 1: Dead Code Cleanup

**What to look for:**
- Unused `use` imports (compiler warnings)
- Unused functions/methods — grep for callers: `grep -r "function_name" src/`
- Unused structs/enum variants — grep for usage
- Commented-out code blocks (>3 lines of `//` commented code)
- `#[allow(dead_code)]` annotations on truly dead items

**Decision rule:**
- Compiler/clippy already flags it → delete directly
- Not flagged → grep-confirm zero callers across entire `src/`, then delete
- If uncertain (might be used via macro or reflection) → skip, don't risk it

**Example commit:** `gateway: optimize dead_code - remove 5 unused imports`

## Dimension 2: DRY Merge

**What to look for:**
- Code patterns repeated 3+ times within the same module
- Similar struct definitions that differ only in 1-2 fields
- Duplicate match arms doing the same thing
- Copy-pasted error handling blocks

**Decision rule:**
- Only merge within the same module (never cross-module — P1 Low Coupling)
- Extract to a private helper function with a clear name
- If merging adds more complexity than it removes → skip

**Example commit:** `providers: optimize dry - extract common auth header builder`

## Dimension 3: Large File Split

**What to look for:**
- Any `.rs` file with >500 lines (`find <module> -name '*.rs' -exec wc -l {} + | sort -rn`)
- Mixed concerns in single file (e.g., types + logic + tests in one file)
- Multiple impl blocks for different types in one file

**Decision rule:**
- Follow CODE_ORGANIZATION.md conventions
- Split by responsibility: types.rs, logic.rs, helpers.rs
- New mod.rs with `pub use` re-exports to maintain public API
- Line count WILL increase (new file headers, mod declarations) — this is expected and allowed

**Example commit:** `memory: optimize split - extract retrieval types to types.rs`

## Dimension 4: Visibility Narrowing

**What to look for:**
- `pub fn` / `pub struct` / `pub enum` that are only used within the crate
- `pub` fields on structs that should be private with accessors
- Check usage: `grep -r "TypeName" src/` — if all hits are in `src/`, use `pub(crate)`

**Decision rule:**
- `pub` → `pub(crate)` if only used within `src/`
- `pub` → `pub(super)` if only used within parent module
- After changing: `cargo check -p alephcore` MUST pass (catches external breakage)
- If used in `interfaces/` or other crates → leave as `pub`

**Example commit:** `thinker: optimize visibility - narrow 3 internal types to pub(crate)`

## Dimension 5: Redline Compliance Audit — REPORT ONLY

**⚠️ DO NOT AUTO-FIX. Report findings only.**

**What to scan for:**
- **R1**: Any `use appkit::`, `use core_graphics::`, `use windows::` in `src/`
- **R3**: Heavy dependencies (`Cargo.toml`) used for a single non-core feature
- **R4**: Business logic in `src/gateway/interfaces/` (should be pure I/O)
- **R8**: Regex-based intent detection, hardcoded routing rules, deterministic classifiers
  that should be LLM-driven
- **R9**: Configuration operations not exposed as callable tools

**Output format** (append to _summary.md):
```
### Redline Findings
- R1 VIOLATION: `src/vision/ocr.rs:42` — direct CoreGraphics API call
- R8 CONCERN: `src/dispatcher/filter.rs:88` — regex-based tool filtering
```

**Why report-only**: Redline fixes are architecture-level multi-file refactors. They require
human judgment and supervised execution, beyond safe atomic commit scope.

## Dimension 6: Idiomatic Rust Rewrite

**What to look for and fix:**

| Pattern | Fix | Example |
|---------|-----|---------|
| `.lock().unwrap()` | `.lock().unwrap_or_else(\|e\| e.into_inner())` | Prevents panic cascade on poisoned lock |
| `&s[..n]` | `s.get(..n).unwrap_or(s)` | UTF-8 safe, no panic on multi-byte |
| `if let Some(x) = ... { if let Some(y) = ...` | `let (Some(x), Some(y)) = (..., ...) else { return }` | Flatten nesting |
| Deep nesting (>3 levels) | Early return + `?` operator | Reduce indentation |
| `x.clone()` where borrow works | `&x` or lifetime annotation | Avoid unnecessary allocation |
| `static mut` | `std::sync::OnceLock` or `LazyLock` | Sound initialization |
| `.unwrap()` on user-facing path | `?` with context or `.unwrap_or_default()` | Graceful error handling |

**Decision rule:**
- Each fix is one commit (don't batch multiple patterns)
- If the idiomatic version is longer/more complex than the original → skip
- Prioritize safety fixes (lock poisoning, UTF-8) over style improvements
```

- [ ] **Step 2: Verify the references file**

```bash
wc -l /Users/zouguojun/Workspace/Skills/optimize-module/references/optimization-checklist.md
```

Expected: ~100 lines

---

### Task 3: Verify skill works end-to-end

**Files:**
- Read: `/Users/zouguojun/Workspace/Skills/optimize-module/SKILL.md`
- Read: `/Users/zouguojun/Workspace/Skills/optimize-module/references/optimization-checklist.md`

- [ ] **Step 1: Verify directory structure**

```bash
find /Users/zouguojun/Workspace/Skills/optimize-module/ -type f
```

Expected output:
```
/Users/zouguojun/Workspace/Skills/optimize-module/SKILL.md
/Users/zouguojun/Workspace/Skills/optimize-module/references/optimization-checklist.md
```

- [ ] **Step 2: Verify SKILL.md frontmatter is valid YAML**

Read the first 4 lines and confirm the `---` delimiters and `name`/`description` fields are present.

- [ ] **Step 3: Verify all spec requirements are covered**

Cross-check against `docs/specs/2026-03-29-optimize-module-skill-design.md`:

| Spec Requirement | SKILL.md Section |
|-----------------|-----------------|
| Module discovery (find commands) | "Module Discovery" section |
| Core loop with commit/verify/reset | "Core Optimization Loop" section |
| BEFORE_SHA rollback | "Evaluation Pipeline" section |
| Clippy JSON comparison | "Step 3: Clippy comparison" |
| Line count per-file | "Step 4: Line count comparison" |
| 6 dimensions ordered | References checklist.md |
| Dim 5 report-only | Explicit in both SKILL.md and checklist |
| _progress.tsv with last_dim | "State Files" section |
| Resume logic | "Resume Logic" section |
| dry-run mode | "dry-run Mode" section |
| 2-consecutive-discard exit | Core loop step 10 |
| Worktree recommendation | After core loop |

- [ ] **Step 4: Test `--list` mode on live repo**

Run the module discovery commands from the Aleph repo root to verify they work:

```bash
cd /Users/zouguojun/Workspace/Aleph && find core/src -maxdepth 1 -mindepth 1 -type d ! -name bin | sort
```

Confirm it returns the expected module directories (gateway, memory, thinker, etc.)

- [ ] **Step 5: Commit the new skill**

```bash
cd /Users/zouguojun/Workspace/Skills
git add optimize-module/
git commit -m "skill: add optimize-module — autonomous code optimization loop

Inspired by autoresearch: agent iterates code changes in a
commit/verify/keep-or-reset loop guided by a 6-dimension
optimization checklist and Aleph's R1-R10 redlines."
```
