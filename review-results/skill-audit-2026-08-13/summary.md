# Static Audit Report — src/skill/

**Date:** 2026-08-13
**Scope:** `src/skill/*.rs` (14 files, ~6,000 LOC)
**Method:** Severed-wire audit. For each candidate, grep the whole repo for a live
consumer; if zero non-test readers, it's a CUT candidate; if found, it's wired.
**Worktree:** `/home/zou/data/workspace/Aleph/.worktrees/skill-audit` (branch `audit/skill-system`)

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High     | 1 |
| Medium   | 2 |
| Low      | 0 |
| **Total**| **3** |

The skill module is unusually clean — most of the ~50 public methods and structs
have live consumers; the three findings below are the only dead abstractions found.

---

## Findings (highest severity first)

### [A-1]. `SkillSystemError` enum is fully unused outside its own definition

- **File:line**: src/skill/mod.rs:57-80
- **Category**: dead-code
- **Severity**: HIGH
- **Description**:
  `pub enum SkillSystemError { Parse(SkillParseError) }` is declared with `Display`,
  `Error`, and `From<SkillParseError>` impls, and is the return type of `init()`
  and `reload_file()`. But **no module outside `src/skill/mod.rs` ever imports,
  names, or matches on `SkillSystemError`** — verified by
  `grep -rn "SkillSystemError" src/ --include="*.rs"` returning only the
  definition file plus its own impls. The enum exists only as a single-variant
  pass-through wrapper over `SkillParseError`, which itself has 4 non-skill
  consumers. The `init()` function body never produces an error (it returns
  `Ok(())` unconditionally after `rescan_dirs`), so its `Result<(), SkillSystemError>`
  signature can never yield `Err`.
- **Verification grep**:
  ```
  $ grep -rn "SkillSystemError" src --include="*.rs" | grep -v "src/skill/mod.rs"
  (no output)
  ```
- **Triage**: CUT (delete `SkillSystemError` enum + its 3 trait impls, change `init()`
  to return `()` since it never fails, change `reload_file()` to return
  `Result<(), SkillParseError>` since that's the only error it can produce).
- **Proposed fix**:
  - Delete lines 53-82 of mod.rs (the enum, Display, Error, From impls).
  - Change `init()` to `pub async fn init(&self, dirs: Vec<PathBuf>)` (no `Result`).
  - Change `reload_file()` to `Result<(), SkillParseError>`.
  - Update 2 callers of `init()` (`src/extension/mod.rs:603`,
    `src/extension/plugin_ops.rs:370`, `src/skill/shared.rs:41`) to drop the
    `if let Err` / `let _ =` wrappers — they only `tracing::warn!` the error
    anyway.

### [D-1]. `SkillSnapshot.ineligible` field has zero non-test consumers

- **File:line**: src/skill/snapshot.rs:25
- **Category**: dead-field
- **Severity**: MEDIUM
- **Description**:
  `pub ineligible: HashMap<SkillId, Vec<IneligibilityReason>>` is computed during
  `SkillSnapshot::build()` (snapshot.rs:91, 132) but **never read** by any code
  outside `src/skill/snapshot.rs`'s own tests (verified via
  `grep -rn "\.ineligible\b" src --include="*.rs"`). The downstream
  `SkillStatusEntry::build` (status.rs:73-95) takes `eligibility: &EligibilityResult`
  directly — not from `snapshot.ineligible` — and pattern-matches the reasons
  itself to populate `MissingRequirements`. The HashMap therefore stores
  information that's already been consumed at a different layer, with no reader
  ever seeing the stored copy.
- **Verification grep**:
  ```
  $ grep -rn "\.ineligible\b" src --include="*.rs" | grep -v "src/skill/snapshot.rs"
  (no output)
  ```
- **Triage**: CUT (delete the `ineligible` field, simplify `build()` to populate
  only `eligible` and `eligible_manifests`).
- **Proposed fix**:
  - Remove `pub ineligible: HashMap<SkillId, Vec<IneligibilityReason>>` field
    (line 25).
  - Remove the `ineligible` HashMap population in `build()` (lines 91, 104, 132, 140).
  - Remove the `ineligible` initialization in `empty()` (line 52).
  - Remove the `ineligible.is_empty()` / `.len()` / `.contains_key(...)` assertions
    in the tests (lines 181, 212, 214).

### [D-2]. `SkillSnapshot.built_at` field has zero consumers

- **File:line**: src/skill/snapshot.rs:29
- **Category**: dead-field
- **Severity**: MEDIUM
- **Description**:
  `pub built_at: DateTime<Utc>` is set to `Utc::now()` in both `empty()`
  (snapshot.rs:54) and `build()` (snapshot.rs:142), but **no code anywhere in
  the repo reads it back** (verified via
  `grep -rn "\.built_at\b" src --include="*.rs"` returning empty). The field
  exists only to be written; the DateTime itself is otherwise unused.
- **Verification grep**:
  ```
  $ grep -rn "\.built_at\b" src --include="*.rs"
  (no output)
  ```
- **Triage**: CUT (delete the field; remove both initializations).
- **Proposed fix**:
  - Remove `pub built_at: DateTime<Utc>` field (line 29).
  - Remove the `Utc` import from snapshot.rs if no longer needed.
  - Remove the `built_at: Utc::now()` initialization in `empty()` (line 54) and
    `build()` (line 142).

---

## Verified wired (no-op, not re-flagged)

The following pub items were spot-checked and confirmed to have live consumers
across the repo (or are explicit re-exports — meant to be reachable):

- `SkillSystem::{new, init, reload_file, ensure_dir_registered, current_snapshot,
  get_skill, list_skills, full_status, locate_skill_file, usage_for, set_pinned,
  set_skill_state, record_use, record_patch, update_config, install_dependency,
  remove_skill}` — all have non-test callers in `src/builtin_tools/skill_manage.rs`,
  `src/gateway/handlers/skills.rs`, `src/extension/{mod,skill_ops,plugin_ops}.rs`,
  `src/memory/dreaming/stages/skill_lifecycle.rs`, `src/orchestrator/dispatch.rs`,
  `src/builtin_tools/skill_install.rs`, etc.
- `default_skill_dirs()` — 5 callers (mod.rs, shared.rs, builtin_tools,
  extension/mod.rs, hub/reconcile.rs).
- `shared_skill_system()` — 15 callers across `src/gateway/handlers/skills.rs`,
  `src/extension/mod.rs`, `src/builtin_tools/*`, `src/executor/builtin_registry/*`,
  `src/bin/aleph-server/commands/start/orchestrator_init.rs`, `src/hub/reconcile.rs`.
- `ensure_shared_skill_system_initialized()` — 3 callers.
- `SkillInfo` and its 3 fields — wired through `tool_metadata/registry/{mod,registration,tests}.rs`
  and `src/bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs`.
- `scan_directory`, `is_skill_file`, `guess_source`, `plugin_id_from_path` — used
  internally by `SkillSystem::rescan_dirs` and tests; `is_skill_file` is also
  reimplemented by `src/tools/markdown_skill/watcher.rs` (separate file, separate
  concerns).
- `SkillRegistry::{new, register, register_all, replace, get, list_all, len,
  is_empty, remove, clear, iter}` — all wired (the `iter` method is consumed by
  `src/skill/snapshot.rs:95`).
- `EligibilityService::{new, evaluate, evaluate_spec}` — used by
  `src/skill/{mod,snapshot}.rs`. `OsNotSupported` constructed at
  `src/skill/eligibility.rs:105`. All 6 `IneligibilityReason` variants are
  constructed and matched.
- `guard::{scan_content, merge_verdicts, scan_skill_directory, install_allowed,
  ScanVerdict, ThreatLevel, TrustLevel, Finding}` — all wired. `merge_verdicts`
  has only 1 external call (`src/skill/guard.rs:222` inside `scan_skill_directory`)
  but is correctly re-exported through mod.rs for callers who want to merge
  manually.
- `manifest::{parse_skill_file, parse_skill_content, automation_notice,
  split_frontmatter, SkillParseError}` — all wired (13, 12, 2, 14, 4 callers
  respectively).
- `preprocess::{preprocess_skill_content, SkillPreprocessContext,
  expand_template_vars, frontmatter_allows_inline_shell}` — wired
  (`preprocess_skill_content` consumed by `src/builtin_tools/skill_reader/read.rs:367`).
- `prompt::{build_skills_prompt_xml, build_skills_prompt_xml_with_budget,
  SkillPromptBudget, DEFERRED_LOADING_GUIDANCE, DEFAULT_MAX_SKILLS_IN_PROMPT,
  DEFAULT_MAX_SKILLS_PROMPT_CHARS}` — wired through `src/thinker/layers/skill_instructions.rs`.
- `config::{InstallPreferences, SkillEntryConfig, SkillsConfig, SkillConfigUpdate}`
  — wired through `src/config/{structs,types/skills,types/mod}.rs`,
  `src/gateway/handlers/{skills,extensions/lifecycle}.rs`,
  `src/builtin_tools/skill_manage.rs`.
- `SkillSnapshot::{empty, build}` and fields `version`, `eligible`,
  `eligible_manifests`, `prompt_budget` — wired through
  `src/skill/{mod,snapshot}.rs`, `src/builtin_tools/skill_manage.rs`,
  `src/orchestrator/harness_bridge/prompt_build.rs`.
- `status::{InstallOption, MissingRequirements, SkillStatusEntry,
  SkillStatusFilter}` — wired. `SkillStatusEntry` is serialized to JSON and
  consumed by `src/gateway/handlers/skills.rs` and `src/tools/usage/report.rs`;
  the `install_options` field is intentionally part of the JSON wire format
  even though no Rust code reads it after construction.
- `usage::{UsageStats, UsageStore, SkillState, record_use_in_dir,
  latest_activity_at}` — wired. All UsageStats fields read at least once
  outside `src/skill/usage.rs` (e.g. `use_count` in
  `src/tools/usage/report.rs:484`, `last_used_at` in `src/builtin_tools/remember.rs`,
  `archived_at` in `src/memory/dreaming/stages/skill_lifecycle.rs`). All
  UsageStore methods (`new`, `get`, `snapshot`, `record_view`, `record_use`,
  `record_patch`, `set_state`, `set_pinned`, `forget`) have external consumers.
- `cooccurrence::{cluster_chains, CoOccurrenceLog, RecentUse}` — wired through
  `src/memory/dreaming/stages/workflow_proposal.rs`. All `CoOccurrenceLog`
  methods (`new`, `record`, `snapshot`) are used.
- `installer::{build_install_command, filter_install_specs_for_current_os,
  select_best_install, InstallExecutor, InstallResult}` — all wired. Every
  `InstallKind` variant has a `build_install_command` arm and a test fixture.
- `Os` enum (defined in `src/domain/skill.rs:141`) — used in
  `IneligibilityReason::OsNotSupported(Os)` and `eligibility::current_os()`.

## Noise / non-findings

- No `TODO` / `FIXME` / `unimplemented!` / `todo!()` in production skill code.
- No `.unwrap()` / `.expect()` calls outside `#[cfg(test)]` modules.
- No `#[allow(dead_code)]` workarounds needed.

---

## Build verification

Per task instructions, **no `cargo check` was run during the audit**. A unified
`cargo check -p alephcore --lib` will be run after all fix commits land on
`audit/skill-system` and the branch is merged into `main`.