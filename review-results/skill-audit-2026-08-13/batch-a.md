# Batch A — Facade, re-exports, shared state, compat

Scope: `src/skill/mod.rs` (1079 L), `src/skill/shared.rs` (62 L), `src/skill/compat.rs` (50 L).
Method: read-only static audit; every candidate grepped for a live consumer across the whole repo
(`src/`, `tests/`, `src/bin/`, `interfaces/`). No cargo invoked.

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High     | 1 |
| Medium   | 3 |
| Low      | 4 |
| **Total**| **8**|

## Findings (highest severity first)

### [A-1]. `ensure_shared_skill_system_initialized()` re-`init()`s the singleton with a *narrower* dir set, silently wiping plugin + workspace skills
- **File:line**: `src/skill/shared.rs:36-44` (latch) ↔ `src/skill/mod.rs:140-147` (`init` = replace, not merge)
- **Category**: severed-wire / logic-bug (last-writer-wins on a shared singleton)
- **Severity**: HIGH
- **Description**: `SkillSystem::init()` **replaces** `inner.skill_dirs` wholesale and then rescans, throwing away the previous registry. There are **two independent producers** of that dir set: `ExtensionManager::load_all` (`src/extension/mod.rs:603`) passes `discover_skill_dirs()` **+ every plugin's `<root>/skills`**, while `ensure_shared_skill_system_initialized` passes `default_skill_dirs()` — at most **two** dirs (`~/.aleph/skills`, `~/.claude/skills`). `INIT_CELL` only latches the *second* producer, so it cannot observe that the first one already initialized. Whichever runs last wins.
  The realistic ordering is the destructive one: boot runs `ext_manager.ensure_loaded()` (`src/bin/aleph-server/.../tool_catalog_init.rs:152`) → full dir set. The first Panel `skills.status` / `skills.set_enabled` / `skills.remove` / `extensions.*` RPC then calls `ensure_shared_system_initialized()` → latch is still unset → `init(default_skill_dirs())` → **all plugin skill dirs and any project-local `.aleph/skills` are dropped from the registry, the snapshot, and the injected `<available_skills>` prompt index.** Zero errors, zero log lines. It stays wiped until a plugin enable/disable (`src/extension/plugin_ops.rs:370`) or a full `reload()` happens to re-init.
  `src/hub/reconcile.rs` is the smoking gun — it does both, back to back, in one function: line 78 `mgr.ensure_loaded()` (full set), line 84 `ensure_shared_skill_system_initialized()` (narrow set), line 87 `full_status()` reads the now-shrunken result and stamps hub `installed` flags from it.
  This is exactly what `shared.rs`'s own module doc says the module exists to prevent ("They never agreed"): collapsing onto one `Arc` fixed *identity*, but the dir set still has two disagreeing writers.
- **Verification grep**:
  - `grep -rn "skill_system.init(" --include='*.rs' .` → `src/extension/mod.rs:603`, `src/extension/plugin_ops.rs:370` (both full sets, incl. `plugin_skill_dirs`)
  - `grep -rn "ensure_shared_skill_system_initialized" --include='*.rs' .` → `src/gateway/handlers/skills.rs:20` (fanned out to 4 RPC handlers + 2 in `extensions/lifecycle.rs`), `src/hub/reconcile.rs:84`
  - `grep -rn "INIT_CELL" src/skill/shared.rs` → set **only** inside `ensure_shared_skill_system_initialized`; `SkillSystem::init` never touches it
  - `sed -n '68,92p' src/hub/reconcile.rs` → `ensure_loaded()` at :78 immediately followed by `ensure_shared_skill_system_initialized()` at :84
- **Triage**: CONNECT
- **Proposed fix**: Make the latch describe the *system*, not the caller. Either (a) have `SkillSystem::init` itself set a `initialized: AtomicBool` on `Inner` and make `ensure_shared_skill_system_initialized` a no-op when it is already set, or (b) drop the narrow path entirely and use the merge-semantics helper that already exists — `ensure_dir_registered(dir)` per default dir + one `rescan_dirs()` — so a second initializer can only *widen* the dir set, never shrink it. (b) also removes the boot-order dependence.

---

### [A-2]. `guess_source()` fallback classes every `~/.claude/skills` skill as `Bundled`, making user-installed compat skills permanently read-only and undeletable
- **File:line**: `src/skill/mod.rs:767-772` (fallback arm) ↔ `src/skill/mod.rs:485-491` (`remove_skill` bundled guard)
- **Category**: logic-bug / fail-closed-with-no-door
- **Severity**: MEDIUM
- **Description**: `default_skill_dirs()` (mod.rs:684-689) deliberately scans `~/.claude/skills`. `guess_source()` has no branch for that path, so it falls through to `SkillSource::Bundled` — the doc comment even says so ("Otherwise → Bundled (e.g. Claude Code compatibility paths)"). But `Bundled` is not a neutral label, it is the **protection** label: `remove_skill` returns `PermissionDenied "Cannot remove bundled skills"` and `skill_manage::mutable_skill_file` (`src/builtin_tools/skill_manage.rs:256`) returns `"Bundled skills are read-only; cannot {action}"`. Result: a skill the user installed themselves into `~/.claude/skills` can never be edited or deleted through any Aleph surface, and the error message tells them it's a bundled Aleph skill — which it isn't. `deletable_on_disk` (mod.rs:492) is also false for it, so even if the registry guard were lifted the directory would survive and be resurrected on the next rescan.
  This is the CLAUDE.md "一道没有门把手的门不是闸，是墙" shape: the fallback grants a protection semantic nobody chose.
- **Verification grep**:
  - `grep -rn "SkillSource::Bundled" --include='*.rs' . | grep -v test` → consumers are `src/skill/mod.rs:486` (remove guard) and `src/builtin_tools/skill_manage.rs:256` (edit guard). No surface distinguishes "Aleph official" from "unknown fallback".
  - `sed -n '674,692p' src/skill/mod.rs` → `default_skill_dirs()` pushes `home/.claude/skills`
- **Triage**: DECIDE (behavioural choice: is a compat skill user-owned or read-only?)
- **Proposed fix**: Give the fallback its own variant (e.g. `SkillSource::External`) or add an explicit `~/.claude/skills` → `Global` arm, so `Bundled` means only "Aleph shipped this and `manifest.json::is_official` says so". Keep the read-only guard keyed on the real `Bundled`.

---

### [A-3]. `guess_source()`'s `.aleph/skills` check is a forward-slash substring test — Windows project-local skills are misclassified `Bundled`
- **File:line**: `src/skill/mod.rs:767` — `if path_str.contains(".aleph/skills")`
- **Category**: logic-bug (platform) / silent misclassification
- **Severity**: MEDIUM
- **Description**: The `Workspace` arm is decided by a literal substring on `path.to_string_lossy()`. On Windows a project-local skill path renders as `C:\proj\.aleph\skills\git\SKILL.md`, which does **not** contain `.aleph/skills`, so it falls through to the `Bundled` fallback. Combined with [A-2] that means every project-local skill on Windows is read-only + undeletable + mis-ranked in the prompt index (`Workspace` and `Bundled` have different collision precedence, and `deletable_on_disk` at mod.rs:492 only accepts `Global | Workspace`). The `~/.aleph/skills` branch above it is unaffected because it uses `PathBuf::starts_with`, which *is* separator-aware — so the bug only bites the one arm that was written with string matching, and only on the platform the repo actively ships (`aleph-desktop-windows`, `WINDOWS_RUNTIME.md`). No test covers it: `guess_source_workspace` (mod.rs:923) hardcodes a POSIX path.
- **Verification grep**:
  - `grep -n 'contains(".aleph/skills")' src/skill/mod.rs` → `767`
  - `grep -n "guess_source_workspace" -A4 src/skill/mod.rs` → asserts on `/some/project/.aleph/skills/git/SKILL.md` only
  - `grep -rn "starts_with(&resolved_global)" src/skill/mod.rs` → `750`, the separator-safe sibling in the same function
- **Triage**: CONNECT
- **Proposed fix**: Replace the substring test with a component walk — `path.components().collect::<Vec<_>>().windows(2).any(|w| w == [".aleph", "skills"])` — and add a `#[cfg(windows)]` case (or a component-built `PathBuf`) to `guess_source_workspace`.

---

### [A-4]. `CACHED_MANIFEST` is a process-lifetime `OnceLock` keyed on nothing — a `None` or stale read is latched forever
- **File:line**: `src/skill/mod.rs:741-742`, consumed at `751-752`
- **Category**: stale-cache / inert-after-first-call
- **Severity**: MEDIUM
- **Description**: `guess_source` caches `InstallRegistry::load(&global_skills)` in a `static OnceLock<Option<..>>`. Two problems:
  1. **`None` is cached as authoritative.** `InstallRegistry::load` returns `None` for *missing* and for *corrupt* `manifest.json` (`src/bundled/manifest.rs:50-63`) — the CLAUDE.md "缺失 vs 损坏" distinction is already collapsed upstream, and this cache then freezes the answer. If the first `guess_source` call in the process precedes bundled extraction (any binary/CLI path that reaches `load_all` without `extract_bundled_content` — e.g. `src/bin/aleph-server/commands/plugins.rs:22`), **every** official skill is classed `Global` for the process lifetime, which makes it deletable on disk (`remove_dir_all` at mod.rs:503) — the exact protection [A-2] over-applies elsewhere.
  2. **Runtime installs never invalidate it.** `src/hub/install.rs:206-217` loads, mutates and `save`s `manifest.json` at runtime. A skill hub-installed after boot is invisible to the cached registry → classed `Global` instead of `Bundled`, so it mis-ranks against same-id collisions in the prompt index.
  3. The cache is also not keyed on `global_skills`, so it survives an `ALEPH_HOME` change — harmless in production, but it makes `guess_source` order-dependent across tests sharing a process (`IsolatedAlephHome` / `HomeEnvGuards` are used by 3 tests in this very file).
- **Verification grep**:
  - `grep -n "CACHED_MANIFEST" src/skill/mod.rs` → declared `741`, `get_or_init` at `751`; never reset
  - `grep -rn "manifest.save\|InstallRegistry::load" src/hub/install.rs` → `206` load, `217` save (runtime writer)
  - `grep -rn "extract_bundled_content" --include='*.rs' .` → only `src/bin/aleph-server/commands/start/helpers.rs:355`; not on the `commands/plugins.rs` path
- **Triage**: CONNECT
- **Proposed fix**: Replace the `OnceLock` with a short-TTL / mtime-checked cache, or (cheapest) only cache `Some(..)` and re-attempt on `None`, plus invalidate from `hub::install`'s `manifest.save` site. Do not key the cache on nothing — key it on the resolved `global_skills` path.

---

### [A-5]. `SkillSystem::init()` returns a `Result` that can never be `Err`; one caller drops it without `let _`
- **File:line**: `src/skill/mod.rs:140-147`; `SkillSystemError` at `mod.rs:56-60`
- **Category**:恒真谓词 / dead error arm
- **Severity**: LOW
- **Description**: `init` calls `rescan_dirs()`, which swallows every parse failure into `tracing::warn!` inside `scan_directory` (mod.rs:642-645) and returns `()`. `init` then unconditionally `Ok(())`. So `SkillSystemError` is only ever constructible from `reload_file` (mod.rs:153, via `From<SkillParseError>`) — the single-variant enum is fine, but `init`'s signature promises a failure mode that does not exist, and every caller correctly ignores it. One of them ignores it *without* `let _`: `src/extension/mod.rs:603` `self.skill_system.init(skill_dirs).await;` — an unused `#[must_use] Result`, i.e. a live `unused_must_use` warning (the CLAUDE.md §10 "只出现在 `^warning` 里" family). Notably this is also the call site in [A-1] whose failure would matter most.
- **Verification grep**:
  - `grep -rn "\.init(" --include='*.rs' . | grep -i skill` → 4 production call sites; **none** branches on the `Result` (`src/extension/mod.rs:603` bare, `src/extension/plugin_ops.rs:370` bare, `src/builtin_tools/skill_status.rs:119` `let _ =` (test), `tests/skill_status_test.rs:20` `.unwrap()`)
  - `grep -rn "unused_must_use" src/lib.rs src/extension/mod.rs` → no allow attribute
- **Triage**: DECIDE
- **Proposed fix**: Change `init` to `pub async fn init(&self, dirs: Vec<PathBuf>)` (no `Result`) and drop the `?`-less error path, or surface a real error by having `rescan_dirs` return the count of unparseable files. Either way fix the bare call at `extension/mod.rs:603`.

---

### [A-6]. Re-export parity: 13 of the 41 names in `mod.rs`'s `pub use` block have zero importers anywhere; 4 more are only reached via the original submodule path
- **File:line**: `src/skill/mod.rs:22-41`
- **Category**: dead-code (facade drift)
- **Severity**: LOW
- **Description**: Only 12 names in the facade block are actually imported through `crate::skill::X` from outside `src/skill/`. The rest split three ways:
  - **Zero external references via *any* path** (grep-verified, whole repo, excluding `src/skill/`): `InstallPreferences`, `SkillEntryConfig`, `RecentUse`, `EligibilityService`, `IneligibilityReason`, `merge_verdicts`, `build_install_command`, `filter_install_specs_for_current_os`, `select_best_install`, `SkillParseError`, `SkillRegistry`, `parse_skill_file`.
    (`SkillSnapshot`, `InstallOption`, `MissingRequirements` also have zero *imports*, but are reachable structurally — `SkillSnapshot` is `current_snapshot()`'s return type, the other two are serialized fields of `SkillStatusEntry`. Not dead, just never named.)
  - **Only reached via the submodule path**, so the facade alias is a no-op: `cluster_chains` / `CoOccurrenceLog` (`crate::skill::cooccurrence::{…}` in `src/memory/dreaming/stages/workflow_proposal.rs:31`), `build_skills_prompt_xml` (`alephcore::skill::prompt::…` in `tests/prompt_injection_test.rs:4`), `SkillPromptBudget` (`crate::skill::prompt::…` in `src/thinker/prompt_builder/mod.rs:59`).
  - **Second-layer dead facade**: `src/lib.rs:201-206` re-exports 8 of these again at the crate root. `grep -rn "alephcore::SkillsConfig\|alephcore::InstallExecutor\|alephcore::SkillStatusFilter\|alephcore::SkillSystem"` returns **zero** hits — external binaries reach them via `alephcore::skill::X` instead.
- **Verification grep**: per-name `grep -rnw "<Name>" --include='*.rs' . | grep -v "^\./src/skill/"`; e.g. `merge_verdicts` → only `src/skill/guard.rs:181,222` + the `pub use` line; `parse_skill_file` → the only outside hits are a **different** function of the same name in `src/tools/markdown_skill/parser.rs:9`.
- **Triage**: CUT
- **Proposed fix**: Shrink the `pub use` block to the 12 names with facade importers plus the 3 structurally-reachable types; delete the rest (they stay `pub` in their own modules for the intra-crate users). Consider also trimming the `src/lib.rs:203-206` block, which has no consumers at all.

---

### [A-7]. `SkillsConfig` names two unrelated types in one crate, and the facade re-export is the one that shadows at the crate root
- **File:line**: `src/skill/mod.rs:23` (`pub use config::{… SkillsConfig}`) → re-exported at `src/lib.rs:205`
- **Category**: name-drift
- **Severity**: LOW
- **Description**: `src/skill/config.rs:37` defines `SkillsConfig { install_preferences, entries, prompt_budget }`, persisted to `~/.aleph/data/skills.toml`. `src/config/types/skills.rs:15` defines a *different* `SkillsConfig { enabled, skills_dir }`, which is the type of `Config.skills` (`src/config/structs.rs:78`) and lives in `aleph.toml`. Because `mod.rs:23` re-exports the first one and `lib.rs:205` lifts it to the crate root, `alephcore::SkillsConfig` and `config.skills`'s type are different structs with the same name — a reader following `alephcore::SkillsConfig` lands on the wrong file. `SkillSystem::new()` (mod.rs:120) hardcodes `<config_dir>/data/skills.toml` and never consults `config.skills.skills_dir`, so the two never meet.
  Supporting context (definition is outside this batch's scope — flagging for whoever owns `src/config/`): `grep -rn "\.skills\.\(enabled\|skills_dir\)" --include='*.rs' .` returns **zero** — both fields of the `aleph.toml` `[skills]` section appear to be inert, which is why the collision has gone unnoticed.
- **Verification grep**: `grep -rnw "SkillsConfig" --include='*.rs' . | grep -v "^\./src/skill/"` → `src/lib.rs:205`, `src/config/structs.rs:10,78,365`, `src/config/types/skills.rs:4,10,15` — two disjoint definition sites, no shared importer.
- **Triage**: DECIDE
- **Proposed fix**: Rename `skill::config::SkillsConfig` → `SkillUserConfig` (it is per-skill user overrides, not the `[skills]` section), and drop it from the `src/lib.rs` crate-root re-export.

---

### [A-8]. `scan_directory()` re-fetches `file_type()` for a symlink check the first branch already `continue`d on
- **File:line**: `src/skill/mod.rs:633-637` vs `649-654`
- **Category**: dead-code (unreachable predicate)
- **Severity**: LOW
- **Description**: Lines 633-637 `continue` on **any** symlink entry. Lines 649-654 then re-call `entry.file_type()` and test `is_dir() && !file_type.is_symlink()` — the `!is_symlink()` conjunct can never be false at that point, and `read_dir`'s `file_type()` is lstat-like so `is_dir()` is already false for a symlink-to-dir regardless. A second syscall-backed call plus a predicate that cannot fire. (Behaviourally correct — no recursion cycle is possible — so this is cosmetic only.)
- **Verification grep**: `sed -n '630,655p' src/skill/mod.rs` — two `if let Ok(file_type) = entry.file_type()` blocks in the same loop body, the first unconditionally `continue`s on symlinks.
- **Triage**: CUT
- **Proposed fix**: Hoist one `let Ok(file_type) = entry.file_type() else { continue };` above both uses and drop the redundant `!file_type.is_symlink()` conjunct.

---

## Verified wired (no-op, do NOT re-flag)

**`SkillSystem` methods — every `pub fn` has a live non-test caller outside `src/skill/`:**
- `init` → `src/extension/mod.rs:603`, `src/extension/plugin_ops.rs:370`
- `reload_file` → `src/builtin_tools/skill_manage.rs:299,432`
- `ensure_dir_registered` → `src/builtin_tools/skill_manage.rs:430`
- `current_snapshot` → 6 sites (thinker prompt layer, handlers)
- `get_skill` → 15 sites incl. `skill_manage.rs:251`
- `list_skills` → 11 sites incl. `.../agent_init/tool_catalog_init.rs:157`
- `full_status` → `gateway/handlers/skills.rs:30,97,136`, `tools/usage/report.rs:473`, `builtin_tools/skill_status.rs:93`, `hub/reconcile.rs:87`
- `locate_skill_file` → `skill_manage.rs:270`
- `usage_for` → `skill_manage.rs:582`
- `set_pinned` → `skill_manage.rs:619`
- `set_skill_state` → `skill_manage.rs:650`
- `record_use` → `gateway/execution_engine/slash_command.rs:192`
- `record_patch` → `skill_manage.rs:302,346,572`, `skill_install.rs:75`
- `update_config` → `gateway/handlers/skills.rs:59,85`, `handlers/extensions/lifecycle.rs:73`, `skill_manage.rs:322,338`
- `install_dependency` → `gateway/handlers/skills.rs:133`, `skill_install.rs:68`
- `remove_skill` → `gateway/handlers/skills.rs:168`, `handlers/extensions/lifecycle.rs:114`, `skill_manage.rs:592`
- `Default`, `Debug`, `Clone` impls — all used.

**Free functions:**
- `default_skill_dirs()` → `src/skill/shared.rs:38`, `src/memory/dreaming/stages/skill_lifecycle.rs:61`, `src/memory/dreaming/stages/workflow_proposal.rs:94` (+ test at `skill_status.rs:119`). Wired.
- `guess_source()` — private (`fn`, not `pub`); 3 internal callers (`reload_file:152`, `skill_dir_for_id:336`, `rescan_dirs:532`). Not a facade item; findings A-2/A-3/A-4 are about its logic, not its wiring.
- `scan_directory()` — private; 1 caller (`rescan_dirs:533`) + self-recursion. Correctly private.
- `plugin_id_from_path()` — private; 1 caller (`guess_source:736`). Correctly private.
- `is_skill_file()` — private; 1 caller (`scan_directory:639`).

**`shared.rs`:** `shared_skill_system()` has 7 production call sites (`gateway/handlers/skills.rs:13`, `extension/mod.rs:262`, `bin/.../orchestrator_init.rs:339`, `executor/builtin_registry/definitions.rs:1202,1207,1212`, `executor/.../collab_session_tools.rs:362`, `hub/reconcile.rs:86`). `ensure_shared_skill_system_initialized()` has 2 (`gateway/handlers/skills.rs:20` → fanned out to 6 handler entry points, `hub/reconcile.rs:84`). Both wired — see A-1 for the semantics bug, not a wiring gap.

**`compat.rs`:** `SkillInfo` and **all three** fields are read. `id` and `description` and `name` are all consumed at `src/tool_metadata/registry/registration.rs:227-239` (`format!("skill:{}", skill.id)`, `&skill.description`, `.with_display_name(&skill.name)`). Constructed at `src/bin/aleph-server/.../tool_catalog_init.rs:161` and `:188`. Both `From` impls exist; `From<SkillManifest>` is used at `:161`-style call sites, `From<&SkillManifest>` — no external caller found, but it is a one-line delegation, below the noise floor.

**`SkillSystemError`:** the single `Parse` variant *is* constructed — `reload_file` (mod.rs:153) uses `?` on `parse_skill_file`, going through `From<SkillParseError>` (mod.rs:78-82). `Display` / `Error::source` both implemented. Not dead. (See A-5 for the separate issue that `init` can never produce it.)

**Test-name confirmations (audit checklist items 9, 10, 13):** `init_with_temp_dir` (mod.rs:848), `list_skills` (mod.rs:873), `full_status_returns_entries` (mod.rs:1015), `remove_skill_from_registry` (mod.rs:1034), `remove_skill_rejects_bundled` (mod.rs:1057) are all `#[tokio::test]` functions inside `#[cfg(test)] mod tests` — **not** production API. `SkillSystem::list_skills` (the method, mod.rs:187) is separate and is wired (11 callers). `insert_manifests_for_test` (mod.rs:223) is correctly `#[cfg(test)] pub(crate)`.

**Panic scan (checklist item 11):** zero `.unwrap()`, `.expect(`, `todo!`, `unimplemented!`, `panic!`, `TODO`, or `FIXME` in the production prefix (pre-`#[cfg(test)]`) of all three files. The only `unwrap`-family calls are `unwrap_or_else` (mod.rs:114, 242, 556, 747, 749), `unwrap_or_default` and `unwrap_or(None)` (mod.rs:306, 346) — all infallible-by-construction defaults on `spawn_blocking` joins and config loads. Clean.
