# Module: skill

**Date**: 2026-07-19
**Reviewers**: 4 parallel agents (security, logic, architecture, quality)

## Summary
- Path: `src/skill/` (15 files, ~6k LOC)
- Raw issues found: 21
- After filtering (high-confidence only): 3

## High-Confidence Issues (will fix)

### 1. Download-kind path traversal via `spec.package` — MEDIUM (security)
- **File**: `src/skill/installer.rs:18-53`
- **Description**: `is_safe_shell_arg` permits `.` and `/` but doesn't reject `..`. For `InstallKind::Download`, `spec.package` is interpolated unquoted as the `-o` output path in `curl -fsSL -o {} '{}'`. A manifest with `package: ../../../tmp/payload` writes outside the install dir.
- **Fix**: Add `is_safe_path_arg` helper that also rejects any `..` segment; call it from the Download branch.

### 2. Memory DoS via unbounded file reads in install-time guard scan — MEDIUM (security)
- **File**: `src/skill/guard.rs:245`
- **Description**: `scan_skill_directory_inner` calls `std::fs::read(&path)` with no per-file size cap, then `scan_content` materializes the full file as `String` for regex matching. A multi-GB file in a malicious bundle OOMs the scanner before any verdict is rendered.
- **Fix**: Add `MAX_SCAN_BYTES = 8 MiB` cap; check file size before read; emit an `oversized_file` Caution finding if exceeded.

### 3. Skill-id construction leaks Unicode whitespace — MEDIUM (logic)
- **File**: `src/skill/manifest.rs:160-168`
- **Description**: `raw.name.replace(' ', "-")` only handles ASCII space. Tabs, NBSP, ideographic space, multiple-consecutive spaces (via split-filter-join) all slip through and produce ids that break downstream path / filesystem operations.
- **Fix**: Use `split_whitespace()` (handles all Unicode whitespace, collapses runs automatically) then join with `-`.

## Skipped Issues (low signal / design choices / high risk)

- **R9 violation**: persisted `install_preferences` and `prompt_budget` have no tool-based update path — design decision; the skill module is content-only and these are config-time decisions.
- **Bidirectional coupling with thinker layer** (prompt.rs imports thinker::xml_util, thinker imports skill::prompt) — architectural refactor, requires product owner sign-off.
- **`SkillSystem` constructs concrete services instead of receiving traits** (P4 violation) — would require redesigning the boot sequence.
- **`mod.rs` 1157 lines** — refactor risk; touches every test.
- **`prompt.rs` 743 lines, `manifest.rs` 649 lines** — same.
- **`preprocess.rs` owns shell execution** (R3 territory) — intentional; this is the "skill as code" contract.
- **`NodeManager` dead config field** — needs wider audit; might be consumed by future install paths.
- **Duplicated YAML frontmatter parsing** (manifest.rs + preprocess.rs) — mechanical refactor with risk of subtle drift.
- **Duplicated file-lock + mkdir + load + save pattern** in usage.rs/cooccurrence.rs — same.
- **Duplicated `Os` string-list parsing** in manifest.rs — same.
- **Duplicated `ThreatLevel` max computation** in guard.rs — same.
- **Inconsistent JSON sidecar serialization** (compact vs pretty) — cosmetic; both are diffed but no functional difference.
- **`parse_skill_content` 81 lines, `SkillStatusEntry::build` 74 lines, `rebuild_snapshot` 61 lines** — coordination functions, defensible size.
- **`pub` visibility overkill on preprocess helpers** — cosmetic.
- **Test helper `make_manifest` duplicated** — test code, low impact.
- **`snapshot.rs::build` sets prompt_budget to default then it's overwritten** — cosmetic.
- **`skill_status` redundant with `full_status`** — needs wider call-site audit before removal.

## Status
- 3 high-confidence issues fixed.
- Committed without per-module `cargo check` per user instruction.
- Full project `cargo check` deferred to end of sweep.