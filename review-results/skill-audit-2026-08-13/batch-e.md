# Batch E — Co-occurrence & installation

Scope: `src/skill/cooccurrence.rs` (259 L) · `src/skill/installer.rs` (483 L)
Method: repo-wide `grep -rn --include="*.rs"` for every `pub` item, then producer/consumer dir + face comparison. No files were modified; no cargo commands were run.

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High     | 1 |
| Medium   | 3 |
| Low      | 2 |
| **Total**| **6**|

## Findings (highest severity first)

### [E-1]. `InstallResult` reaches the Panel on the wire and is discarded — a failed dependency install renders as success
- **File:line**: src/skill/installer.rs:97 (`pub struct InstallResult`), consumed at `interfaces/webchat/src/platform/wide/views/settings/skills.rs:704-712`
- **Category**: client-ghost / severed-wire (one of two faces)
- **Severity**: HIGH
- **Description**: `InstallResult { success, message, stdout, stderr }` is fully produced and serialized by `skills.install_dep` (`src/gateway/handlers/skills.rs:141-147`, which always returns `JsonRpcResponse::success` regardless of `result.success`). The Panel's only reader matches `Ok(_) => { installing_dep.set(None); on_refresh(); on_close(); }` — the `result` object is never destructured. So every failure mode of the executor (non-zero exit, 300 s timeout, `"Cannot build install command for …"` from the shell-arg allowlist) closes the dialog with no message. The consumer end already exists and is idle: `dep_error` + `settings_write_error(...)` is wired only to the *transport* `Err` arm. The tool face is fine — `SkillInstallTool` forwards `success`/`message`/`stderr` to the model (`src/builtin_tools/skill_install.rs:41-48`), so the two faces of the same verb tell different stories.
- **Verification grep**: `grep -rn "skills.install_dep" --include="*.rs" src/ interfaces/` → 3 hits: handler registration (`handlers/mod.rs:445`), handler (`handlers/skills.rs:112`), single Panel call site (`skills.rs:704`). `sed -n 690,740p interfaces/webchat/src/platform/wide/views/settings/skills.rs` shows the `Ok(_)` arm ignoring the payload. `grep -rn "install_dependency" --include="*.rs" .` → only the tool + this handler.
- **Triage**: CONNECT
- **Proposed fix**: In the Panel `Ok(v)` arm, read `v["result"]["success"]`; on `false`, set `dep_error` to `v["result"]["message"]` (plus a stderr tail) and keep the dialog open instead of `on_close()`.

### [E-2]. Rank tables are OS-blind: a spec with no `os:` restriction can select a package manager that cannot exist on the current platform
- **File:line**: src/skill/installer.rs:119-137 (`PREFER_BREW_RANKS` / `PREFER_UV_RANKS` / `install_kind_rank`)
- **Category**: logic-bug / criteria-drift
- **Severity**: MEDIUM
- **Description**: `select_best_install` runs `filter_install_specs_for_current_os` first, but that filter only removes specs whose author *declared* `os:`. Rank is then a pure function of `InstallKind` + `prefer_brew`. With the shipped default (`prefer_brew = cfg!(target_os = "macos")` → `false` off macOS), `PREFER_UV_RANKS` gives Brew = 2 vs Apt = 6 and Scoop = 3 / Winget = 4. Concretely: a manifest declaring `brew` (unrestricted, the common authoring habit) + `apt` picks `brew install …` on Linux; the same manifest with `brew` + `scoop` picks brew on **Windows**, where the in-file comment claims "OS filtering runs before ranking, so these only compete on Windows in practice" — brew competes there too and wins. The classifier for this already exists in the same crate and is unused here: `which::which` (`src/skill/eligibility.rs:111,118`).
- **Verification grep**: `grep -n "const PREFER" src/skill/installer.rs` → `125: [0,1,2,3,4,5,6,7]`, `126: [2,3,4,0,1,5,6,7]`; index order at `install_kind_index` is `[Brew,Scoop,Winget,Uv,Npm,Go,Apt,Download]`. `grep -n "prefer_brew" -A3 src/skill/config.rs` → `Default { prefer_brew: cfg!(target_os = "macos") }`. `grep -rln "install:" skills/ plugins/` → no bundled manifest declares install specs, so every spec comes from third-party/hub authors who may omit `os:`.
- **Triage**: CONNECT
- **Proposed fix**: Drop OS-exclusive kinds before ranking — filter out `Brew` off macOS/Linux-with-brew, `Apt` off Linux, `Scoop`/`Winget` off Windows — or, better, rank `which::which(<manager bin>)`-absent kinds last so an unrestricted `brew` spec loses to `apt` on a box with no brew.

### [E-3]. `.cooccur.json` is written into every registered skill dir but mined from only `default_skill_dirs()`
- **File:line**: src/skill/cooccurrence.rs:61 (`CoOccurrenceLog::new(skills_dir)`); writer `src/skill/usage.rs:281` / `src/builtin_tools/skill_reader/read.rs:411`; reader `src/memory/dreaming/stages/workflow_proposal.rs:94-106`
- **Category**: partition-mismatch (writer set ⊋ reader set)
- **Severity**: MEDIUM
- **Description**: The writer stamps the ring into `skill_dir.parent()` — whichever registered dir owns the skill. The registered set at boot is discovery dirs **plus every plugin's `<root>/skills`** (`src/extension/mod.rs:550-576`) and `SkillSystem::record_use` resolves through `owning_dir` over that same superset. The miner iterates only `default_skill_dirs()` = `~/.aleph/skills` + `~/.claude/skills` (`src/skill/mod.rs:644-661`). Uses of plugin-shipped and workspace-`.aleph/skills` skills therefore accumulate rings nobody opens: no workflow proposal can ever be drafted from them, and the 512-entry sidecars grow in dirs that are never read or pruned. Second half of the same shape: the stage clusters **each ring independently**, so a chain that mixes an `~/.aleph/skills` skill with a `~/.claude/skills` skill is never seen as co-occurring even though both dirs are read and both entries share one wall clock (`now_ms`).
- **Verification grep**: `grep -rn "record_use_in_dir\|default_skill_dirs" --include="*.rs" src/` → writers at `read.rs:411` / `mod.rs:366` (dir from `owning_dir`), readers at `workflow_proposal.rs:94` and `skill_lifecycle.rs:61` (both `default_skill_dirs()`); `grep -n "skill_dirs" -A12 src/extension/mod.rs` shows plugin skill dirs appended before `skill_system.init(skill_dirs)`.
- **Triage**: CONNECT
- **Proposed fix**: Give the enumeration one source — have `WorkflowProposalStage` read the *registered* dirs (the `SkillSystem`'s `skill_dirs`, or a `skill::all_registered_skill_dirs()` helper) instead of `default_skill_dirs()`, and merge all rings before `cluster_chains` rather than clustering per dir.

### [E-4]. `remove_skill` forgets the `.usage.json` row but leaves the deleted skill in `.cooccur.json`
- **File:line**: src/skill/cooccurrence.rs:59-123 (no `forget`/prune API on `CoOccurrenceLog`); asymmetric call site `src/skill/mod.rs:482-485`
- **Category**: dead-wire / stale-state
- **Severity**: MEDIUM
- **Description**: `usage::record_use_in_dir` exists precisely so the two sidecar writes cannot drift ("A use is **two** writes", `usage.rs:265-277`), but the removal path only performs one of the two deletes: `UsageStore::new(dir).forget(id)` with no co-occurrence counterpart, and `CoOccurrenceLog` exposes no removal method at all. A removed skill therefore survives in the ring for up to 512 entries, and `skeleton_from_chain` performs no existence check (`src/workflow/proposal.rs:86-131`) — so the dream pipeline can draft and persist a workflow proposal whose steps say "Apply the '<deleted-skill>' skill". `remove_skill`'s own doc claims the sidecar cleanup prevents "orphan telemetry over time"; that promise holds for one sidecar only.
- **Verification grep**: `grep -n "forget\|cooccur" src/skill/mod.rs` → `484: UsageStore::new(dir).forget(id.as_str());` and no cooccurrence reference outside the module re-export at line 24. `grep -n "pub fn" src/skill/cooccurrence.rs` → only `new`, `record`, `snapshot`, `cluster_chains`.
- **Triage**: CONNECT
- **Proposed fix**: Add `CoOccurrenceLog::forget(&self, skill)` (lock → load → `retain(|e| e.skill != skill)` → save) and call it in the same `for dir in &dirs` loop as `UsageStore::forget`; symmetrically, consider a `remove_use_in_dir` sibling to `record_use_in_dir` so the pairing is structural.

### [E-5]. `InstallExecutor::run` takes `&InstallPreferences` and never reads it
- **File:line**: src/skill/installer.rs:181
- **Category**: inert-parameter
- **Severity**: LOW
- **Description**: The parameter is bound as `_prefs` and unused — every caller (`src/skill/mod.rs:435`) clones and threads preferences that the executor discards. Selection already consumed `prefs` upstream in `select_best_install`, so the argument is pure ceremony; it reads as "the executor honours preferences" while it does not.
- **Verification grep**: `grep -n "_prefs" src/skill/installer.rs` → declaration only, no body use; `grep -rn "InstallExecutor" --include="*.rs" .` → one production call site (`src/skill/mod.rs:435`) plus the `lib.rs:204` re-export.
- **Triage**: CUT
- **Proposed fix**: Drop the parameter from `InstallExecutor::run` (and from the `lib.rs` public surface if it is only re-exported for this signature), or use it — e.g. to pick the shell / add `--yes`-style flags per preference.

### [E-6]. `build_install_command` is re-exported with zero external consumers
- **File:line**: src/skill/installer.rs:14, re-exported at src/skill/mod.rs:31
- **Category**: dead-code (visibility only)
- **Severity**: LOW
- **Description**: The function is genuinely used — but only by `InstallExecutor::run` in the same file (line 182) and by tests. The `pub use` in `src/skill/mod.rs:31` has no consumer anywhere in the workspace (it is not even carried into `src/lib.rs:204`, which re-exports only `InstallExecutor` / `InstallResult`). Same for `select_best_install`'s re-export: its only caller is `src/skill/mod.rs:420`, inside the module that re-exports it.
- **Verification grep**: `grep -rn "build_install_command" --include="*.rs" .` → `installer.rs` (def + in-file call + 9 test uses) and `mod.rs:31` (re-export) only; nothing under `interfaces/`, `shared/`, `src/gateway/`, `src/builtin_tools/`.
- **Triage**: CUT
- **Proposed fix**: Narrow to `pub(crate)` (or `pub(super)`) and drop the unconsumed names from the `src/skill/mod.rs:31` re-export list.

## Verified wired (no-op, do NOT re-flag)
- `cluster_chains` — `src/memory/dreaming/stages/workflow_proposal.rs:106`; the stage itself is registered in the dream pipeline at `src/memory/dreaming/mod.rs:334`. Live end-to-end (subject to E-3's dir gap).
- `CoOccurrenceLog::new` / `::record` — `src/skill/usage.rs:281` via `record_use_in_dir`, reached from both faces (`skill_reader/read.rs:411` and `SkillSystem::record_use`, `src/skill/mod.rs:366`).
- `CoOccurrenceLog::snapshot` — `workflow_proposal.rs:105`.
- `RecentUse.skill` / `.at_ms` — read by `cluster_chains` (sort key + gap + dedup) and by serde on both sides of the sidecar. Not write-only.
- `filter_install_specs_for_current_os` — `src/skill/status.rs:101` (builds the Panel's per-OS install-option list) plus `select_best_install`.
- `select_best_install` — `src/skill/mod.rs:420` (the `spec_id`-omitted branch of `install_dependency`).
- `InstallExecutor::run` — `src/skill/mod.rs:435`.
- `InstallResult` as a type, and fields `success` / `message` / `stdout` / `stderr` — reach the model through `SkillInstallOutput` (`src/builtin_tools/skill_install.rs:41-48`); the `skill_install` tool is registered in `definitions.rs:920`, `groups.rs:115`, and dispatched at `tool_registry_impl.rs:1335`. (`InstallResult` in `interfaces/webchat/src/api/extensions.rs:86` is an unrelated Panel-side enum for hub extensions — name collision, not drift.)
- `InstallKind::Scoop` / `Winget` branches in `build_install_command` — reachable: the markdown manifest parser maps both strings (`src/skill/manifest.rs:241-242`) and serde accepts the lowercase names. Not dead (their *ranking* is the E-2 issue).
- `Os` filtering has no unreachable arm: `current_os()` (`src/skill/eligibility.rs:160-178`) covers all three variants with a Linux fallback.

## Notes / non-findings
- `InstallResult.exit_code` has no reader today (`SkillInstallOutput` drops it; the Panel ignores the whole payload). It is folded into E-1 rather than filed separately — fixing E-1 by rendering the failure naturally gives it a consumer; if E-1 is fixed without it, `exit_code` becomes a CUT candidate.
- `MAX_ENTRIES = 512` FIFO with no age-based expiry is intentional per the module doc; the miner's `window_secs` / `min_observations` bound the interpretation. Not flagged.
- `build_install_command`'s injection allowlist and `..`-segment rejection for `Download` look correct for the unquoted `-o <path>` interpolation; URL is single-quoted with `'`/control chars rejected. No finding.
