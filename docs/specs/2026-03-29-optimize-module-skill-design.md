# Optimize-Module Skill Design

> Autonomous code optimization skill inspired by [autoresearch](https://github.com/jxnl/autoresearch) — an LLM agent autonomously modifies, verifies, and keeps-or-reverts changes in a continuous loop, guided by a structured optimization checklist and Aleph's R1-R10 architectural redlines.

## Philosophy

From autoresearch: **minimal system + strong instructions = maximum agent capability**.

- The skill is the agent's `program.md` — humans iterate the checklist, agent iterates the code
- Git as state machine — each optimization is a commit, good results advance, bad results reset
- Single clear evaluation criterion — no ambiguous scoring
- Occam's Razor as first-class citizen — deletions preferred over additions

## Skill Structure

```
Skills/optimize-module/
├── SKILL.md                              # Main skill file (agent instructions)
└── references/
    └── optimization-checklist.md         # Ordered optimization dimensions (6)
```

## Invocation

**Skill name**: `optimize-module`

**Trigger keywords**: "optimize module", "优化模块", "代码优化", "自主优化"

```
/optimize-module <module>              # Single module
/optimize-module --all                 # All modules (small to large)
/optimize-module --all --resume        # Resume interrupted run
/optimize-module gateway --dim 3       # Only dimension 3 (large file split)
/optimize-module --all --dry-run       # Analyze only, no modifications
/optimize-module --list                # List discovered modules with line counts
```

### Parameters

| Param | Default | Description |
|-------|---------|-------------|
| `<module>` | — | Module name (gateway, memory, standalone:xxx, bin:xxx) |
| `--all` | false | Traverse all modules |
| `--resume` | false | Continue from last interruption |
| `--dim <N>` | all | Run only dimension N (1-6) |
| `--dry-run` | false | Analyze and report, no edits or commits |
| `--list` | false | Only list discovered modules and line counts |

## Module Discovery

Three categories, zero hardcoding (reuses review-modules pattern):

```bash
# Must run from repository root. Agent should verify: test -d src/

# 1. Module directories (exclude bin/)
find core/src -maxdepth 1 -mindepth 1 -type d ! -name bin | sort

# 2. Standalone .rs files (exclude main.rs, lib.rs)
find core/src -maxdepth 1 -name '*.rs' ! -name 'lib.rs' ! -name 'main.rs' | sort

# 3. Bin targets
find src/bin -maxdepth 1 -mindepth 1 -type d | sort
```

**Naming convention**:
- Directory modules: `gateway`, `memory`, `thinker`
- Standalone files: `standalone:sync_primitives.rs`
- Bin targets: `bin:aleph-server`

**Sorting (--all mode)**: By total line count ascending — small modules first for fast early wins.

## Core Loop

```
# Initialization (once per run):
#   1. Ensure CWD is repo root: test -d src/
#   2. Ensure optimize-results/ exists and is gitignored
#   3. Pre-flight: cargo check -p alephcore must pass before ANY optimization
#      If pre-flight fails → abort with "ABORTED: pre-existing compile error"

FOR each module (discovered or user-specified):
  Pre-flight: cargo check -p alephcore
    → FAIL = skip module, record "SKIPPED: pre-existing compile error" in _progress.tsv
  baseline = snapshot(clippy_count, line_count, test_status)

  FOR each dimension in optimization_checklist (1 through 6):
    WHILE opportunities exist for this dimension:
      1. Read module code, identify next opportunity
      2. If none found → break to next dimension
      3. Apply change (Edit tool)
      4. git add <changed files>
      5. BEFORE_SHA=$(git rev-parse HEAD)
      6. git commit -m "<module>: optimize <dimension> - <description>"
      7. Verify (see Evaluation Pipeline below)
      8. IF pass → update baseline, record "keep" in TSV
         ELSE → git reset --hard $BEFORE_SHA, record "discard" in TSV

  Record module as DONE in _progress.tsv
  Append summary to _summary.md
```

**Stop condition**: All 6 dimensions scanned for the current module, no more opportunities → module complete, move to next.

**Key behaviors (from autoresearch)**:
- NEVER pause to ask "should I continue?" — run until the checklist is exhausted
- If a change crashes cargo check, diagnose quickly: trivial fix → fix and retry; fundamental issue → discard and move on
- Each commit is atomic — one logical change per commit
- **WHILE loop exit**: If 2 consecutive attempts in the same dimension are discarded, the dimension is exhausted — move to the next unconditionally. Agent must NOT re-attempt the same opportunity that was already discarded (each opportunity is tried at most once).

**Worktree recommendation**: For `--all` runs (many commits on main), consider running in a git worktree and squash-merging back to main when done, to keep git history clean.

## Evaluation Pipeline

Every commit is verified by this pipeline. All steps must pass to keep the change:

```
# Before ANY changes, record HEAD sha for safe rollback:
BEFORE_SHA=$(git rev-parse HEAD)

Step 1: cargo check -p alephcore
        → FAIL = git reset --hard $BEFORE_SHA

Step 2: cargo test -p alephcore --lib
        → FAIL = git reset --hard $BEFORE_SHA

Step 3: Clippy comparison (using JSON for reliable counting, bypassing incremental cache)
        Baseline (taken once per module start, updated after each keep):
          cargo clippy -p alephcore --message-format=json 2>/dev/null \
            | jq -r 'select(.reason=="compiler-message") | select(.message.level=="warning") | .message.code.code // empty' \
            | sort | uniq -c | sort -rn > /tmp/clippy_baseline.txt
        After change:
          same command > /tmp/clippy_after.txt
        Compare new codes not in baseline:
          comm -13 <(awk '{print $2}' /tmp/clippy_baseline.txt | sort) \
                   <(awk '{print $2}' /tmp/clippy_after.txt | sort)
          → any output = new warning code introduced → rollback
        (Existing warnings may decrease but must not increase)

Step 4: Line count comparison (scoped to changed files only)
        Changed files: git diff --name-only HEAD~1 HEAD | grep '\.rs$'
        Before: git show HEAD~1:<file> | wc -l  (sum all changed files)
        After:  wc -l <file>  (sum all changed files)
        → net_delta > 0 = rollback
        EXCEPTION: dimension 3 (large file split) and dimension 5 (redline report)
                   are exempt from line count check — structural improvements
```

**Rollback safety**:
- Always record `BEFORE_SHA=$(git rev-parse HEAD)` before committing
- Rollback with `git reset --hard $BEFORE_SHA` (never `HEAD~1`, which can misfire if commit failed)
- This is safe even if `git commit` itself failed (reset to known-good state)

**Baseline management**:
- Initial baseline taken once per module at start
- Updated after each successful "keep" (cumulative improvement)
- Clippy baseline is crate-level (clippy doesn't support module granularity)
- Clippy uses `--message-format=json` to avoid ANSI color codes and incremental cache issues

## Optimization Checklist (6 Dimensions, Ordered)

Executed in order from low-risk to high-risk:

### Dimension 1: Dead Code Cleanup
- Unused `use` imports
- Unused functions/methods/structs/enum variants
- Commented-out code blocks
- Code suppressed by `#[allow(dead_code)]` that is truly dead
- **Rule**: Compiler/clippy-flagged items delete directly; unflagged items grep-confirm zero callers first

### Dimension 2: DRY Merge
- 3+ repeated code patterns → extract function
- Similar structs → unify with generics/enum
- Duplicate match arms → merge
- **Constraint**: Same-module only, no cross-module extraction (P1 low coupling)

### Dimension 3: Large File Split
- Files > 500 lines → split by responsibility into submodules
- Follow CODE_ORGANIZATION.md conventions
- **Exception**: Line count increase allowed (new mod.rs + pub use)

### Dimension 4: Visibility Narrowing
- `pub` → `pub(crate)` or `pub(super)` where usage is crate/module-internal
- Unnecessary `pub` fields → private + accessor
- **Verify**: cargo check confirms no external dependency breakage

### Dimension 5: Redline Compliance Audit (R1-R10) — REPORT ONLY
- R1: Platform API calls in core?
- R3: Heavy third-party libs for non-core features?
- R4: Business logic leaking into interface layer?
- R8: Deterministic code replacing LLM judgment?
- R9: Configurable operations not exposed as Tools?
- **Rule**: This dimension is REPORT ONLY — do not auto-fix. Record findings in `_summary.md` with file:line references. Redline violations are architecture-level issues requiring human-supervised multi-file refactors, beyond safe atomic commit scope.

### Dimension 6: Idiomatic Rust Rewrite
- `lock().unwrap()` → `lock().unwrap_or_else(|e| e.into_inner())`
- `&s[..n]` → `s.get(..n)` (UTF-8 safety)
- Deep nesting → early return + `?` operator
- Eliminable `clone()` → references/borrows
- `static mut` → `OnceLock`

## State Management & Resume

### Output Structure

Output directory: `optimize-results/` in repository root. Agent should ensure it is gitignored on first run (`grep -q optimize-results .gitignore || echo "optimize-results/" >> .gitignore`).

```
optimize-results/
├── _progress.tsv          # Global progress tracker
├── _summary.md            # Human-readable summary report
├── gateway.tsv            # Per-module operation log
├── memory.tsv
├── thinker.tsv
└── ...
```

### _progress.tsv Format

```
module	status	last_dim	kept	discarded	clippy_delta	lines_delta	started_at	finished_at
clipboard	done	6	2	0	-1	-12	2026-03-29T10:00	2026-03-29T10:03
utils	done	6	5	2	-3	-45	2026-03-29T10:03	2026-03-29T10:15
gateway	in_progress	2	3	1	-2	-28	2026-03-29T10:15	—
memory	pending	0	—	—	—	—	—	—
```

### Per-Module TSV Format

```
commit	dimension	action	status	clippy_delta	lines_delta	description
a1b2c3d	dead_code	remove unused imports	keep	-2	-15	removed 5 unused use statements
b2c3d4e	dry	extract common parser	discard	0	+8	test failed: parser_test
```

### Resume Logic

- `--resume` reads `_progress.tsv`
- `done` → skip
- `in_progress` → read `last_dim` from `_progress.tsv`, resume from dimension `last_dim + 1`; each dimension's WHILE loop restarts fresh (re-scan for opportunities)
- `pending` → start normally

### _summary.md (appended per module)

```markdown
## clipboard (2 kept / 0 discarded, -1 clippy, -12 lines)
- Removed 3 unused imports
- Replaced lock().unwrap() with safe pattern
```

## dry-run Mode

When `--dry-run` is passed:
- Agent reads code and identifies opportunities per dimension
- Outputs findings to `optimize-results/<module>-dryrun.md`
- No edits, no commits, no git operations
- Useful for previewing what the skill would do before committing

## Design Decisions

### Why pure Skill (no shell script)?
- Aligns with autoresearch philosophy: minimal system + strong instructions
- Verification logic is simple enough for agent to run as bash commands
- R8 compliance: don't over-engineer deterministic wrappers
- YAGNI: shell script can be added later if needed

### Why ordered checklist (not fully autonomous)?
- Predictable, reproducible runs
- Low-risk changes first (dead code) → high-risk last (architecture)
- User can stop anytime, completed dimensions are independently valuable
- The checklist IS the agent's program.md — humans iterate it, agent iterates code

### Why commit-then-verify (not verify-then-commit)?
- Simpler rollback: `git reset --hard $BEFORE_SHA`
- Git history shows attempted experiments (even if reverted)
- Matches autoresearch pattern exactly

### Why line count as metric?
- Embodies Occam's Razor: "A 0.001 improvement from deleting code? Definitely keep."
- Gives agent clear preference signal: simplification > complexity
- Exception for structural improvements (file splits, redline fixes) prevents metric gaming

## Relation to review-modules

| Aspect | review-modules | optimize-module |
|--------|---------------|-----------------|
| Purpose | Find bugs and issues | Fix and improve code |
| Output | Report with scored issues | Committed code changes |
| Action | Read-only analysis | Read-write optimization loop |
| Agents | Multi-agent parallel review | Single agent serial loop |
| State | review-results/ reports | optimize-results/ + git commits |

They are complementary: review-modules finds problems, optimize-module fixes them systematically.
