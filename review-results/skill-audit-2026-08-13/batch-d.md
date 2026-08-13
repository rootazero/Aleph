# Batch D — Persistence & status

Scope: `src/skill/config.rs`, `src/skill/snapshot.rs`, `src/skill/status.rs`, `src/skill/usage.rs`.
Method: for every `pub fn` / `pub` field / enum variant, grep the **whole repo** (`src/`, `interfaces/`,
`shared/`) for a non-test, non-same-file consumer. Read-only audit; no build run.

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High     | 0 |
| Medium   | 4 |
| Low      | 2 |
| **Total**| **6**|

## Findings (highest severity first)

### [D-1]. `SkillSnapshot.version` is a cache-invalidation key that invalidates no cache
- **File:line**: src/skill/snapshot.rs:19 (field), src/skill/mod.rs:69,516-518 (the counter that feeds it)
- **Category**: dead-code / inert-field
- **Severity**: MEDIUM
- **Description**: The field is documented as *"Monotonically increasing version counter for cache
  invalidation. version increments indicate cache invalidation"*, and `SkillSystem` maintains a whole
  `RwLock<u64>` (`version_counter`) plus an increment-under-write-lock in `rebuild_snapshot` to produce it.
  Repo-wide there is **no non-test reader**: every consumer of `current_snapshot()`
  (`harness_bridge/prompt_build.rs:238`, `builtin_tools/skill_manage.rs:915/930`) reads only
  `eligible_manifests` / `prompt_budget`. `SkillSnapshot` does not derive `Serialize`, so the value
  cannot reach a client either. Nothing keys a cache off it; the prompt layer re-renders unconditionally.
- **Verification grep**:
  ```
  grep -rn "snap\w*\.version\|snapshot\.version" --include="*.rs" src/ interfaces/ shared/
  ```
  returns 7 hits — `src/skill/mod.rs:833`, `src/skill/shared.rs:60`, and
  `src/skill/snapshot.rs:170,201,223,224,225` — **all inside `#[cfg(test)] mod tests`**. Zero production hits.
- **Triage**: DECIDE — CUT the field + `version_counter` + the lock/increment in `rebuild_snapshot`,
  **or** CONNECT it to a real consumer (e.g. let `prompt_build.rs` skip re-deriving `active_tool_names`
  when the version is unchanged).
- **Proposed fix**: default to CUT (R10 "零消费者的通道优先 CUT"): delete `SkillSnapshot.version`,
  `Inner::version_counter`, and the `build(..., version, ...)` parameter; drop the three test assertions
  that only observe it.

### [D-2]. `SkillSnapshot.eligible: Vec<SkillId>` has zero non-test readers but is rebuilt on every snapshot
- **File:line**: src/skill/snapshot.rs:21 (field), src/skill/snapshot.rs:83,102,130 (built)
- **Category**: dead-code
- **Severity**: MEDIUM
- **Description**: `build()` pushes a cloned `SkillId` for every eligible skill into this vector on every
  `rebuild_snapshot()` (which runs on init, every rescan, every `update_config`, every install, every
  `set_skill_state`, every remove). Nothing outside the file's own tests ever reads it. This is the exact
  shape the file's own doc comment (snapshot.rs:31-35) records for the already-deleted `prompt_xml`
  field — "read by nothing outside its own tests" — the deletion just stopped one field short.
- **Verification grep**:
  ```
  grep -rn "\.eligible\b" --include="*.rs" src/ interfaces/ shared/
  ```
  Snapshot-typed hits: `src/skill/mod.rs:834` and `src/skill/snapshot.rs:171,202,203,207,263,295,327,363,390`
  — all `#[cfg(test)]`. The `interfaces/webchat/.../settings/skills.rs:75,627` and `src/skill/status.rs:142,143`
  hits are `SkillStatusEntry.eligible`, a different type/field (that one is wired).
- **Triage**: CUT
- **Proposed fix**: delete the `eligible` field and the `eligible.push(id.clone())` in `build()`; rewrite
  the tests that assert on it to assert on `eligible_manifests` (they already do so alongside), or on
  `SkillSystem::full_status()` which is the surface that actually reports eligibility to Panel/CLI/LLM.

### [D-3]. `UsageStats::activity_count()` has no caller; the one place that sums activity hand-rolls a different sum
- **File:line**: src/skill/usage.rs:100-107
- **Category**: dead-code
- **Severity**: MEDIUM
- **Description**: `activity_count()` returns `use + view + patch`. Repo-wide the only call site is its own
  unit test. The one production consumer that needs an activity total —
  `src/tools/usage/report.rs:485` — deliberately computes `u.use_count.saturating_add(u.view_count)`
  by hand and documents at line 482 why `patch_count` is **excluded** ("install / enable / scope change"
  is not user activity). So the helper is not merely uncalled, it encodes a definition the one real
  consumer rejects; leaving it is an invitation for the next caller to pick the wrong one.
  (Sibling `latest_activity_at()` at usage.rs:89 **is** wired — `memory/dreaming/stages/skill_lifecycle.rs:83`
  — so this is not a blanket "the impl block is dead" claim.)
- **Verification grep**:
  ```
  grep -rn "activity_count()" --include="*.rs" .
  ```
  → single hit `src/skill/usage.rs:415` (inside `#[cfg(test)] mod tests`). The other
  `activity_count` matches in the repo are `migrate_dream_reports_add_activity_counters`, an unrelated
  SQLite migration.
- **Triage**: CUT
- **Proposed fix**: delete `UsageStats::activity_count` and its test; if a shared total is ever wanted,
  add it next to `latest_activity_at` with the semantics `report.rs` already chose (use + view) so the
  two faces cannot drift.

### [D-4]. `SkillsConfig::save` hand-rolls a non-fsync'd write, bypassing the module's own atomic writer — and `load` silently converts the resulting corruption into "all user overrides erased"
- **File:line**: src/skill/config.rs:74-83 (`save`), src/skill/config.rs:57-72 (`load`)
- **Category**: unsafe-correctness / silent-data-loss
- **Severity**: MEDIUM
- **Description**: `save` does `fs::write(<path>.toml.tmp)` + `fs::rename`, with **no `fsync`** and a
  **fixed** temp filename. The repo has a single-source atomic writer — `utils::atomic_io::write_atomic`
  (random temp name + `sync_all()` + `persist`) — and `src/skill/usage.rs:37,139` in the *same module*
  already uses it. The failure mode is asymmetric because of the read side: `load` treats a TOML parse
  error as `Self::default()` behind a bare `tracing::warn!`, so a truncated/zero-length `skills.toml`
  after a crash presents as "every skill is back to default enabled/default scope", with no user-visible
  signal. Worse, the loss is then made **permanent**: the next `update_config` call writes those defaults
  back over the file (`config.apply_update` → `config.save`, mod.rs:386-388), so the surviving overrides
  for every *other* skill are destroyed by an unrelated toggle.
  Scope note, stated honestly: the fixed temp name is not a live race — `skills.toml` has exactly one
  writer (`SkillSystem::update_config`, holding `config.write().await` across the save) and the OS-level
  singleton `flock` keeps a second server out. The reachable path is crash / power loss between
  `rename` and the (never-issued) `fsync`.
- **Verification grep**:
  ```
  grep -rn "skills.toml" --include="*.rs" src/ interfaces/     # 6 hits, one writer: src/skill/mod.rs:87
  grep -rn "pub fn write_atomic" --include="*.rs" src/utils/   # src/utils/atomic_io.rs:21
  grep -rn "write_atomic\|with_file_lock" src/skill/usage.rs   # lines 37, 139, 163, 252
  ```
- **Triage**: CONNECT
- **Proposed fix**: replace the body of `save` with
  `crate::utils::atomic_io::write_atomic(path, content.as_bytes())` after `create_dir_all(parent)`;
  and make `load` distinguish "file absent" (→ defaults, correct) from "file present but unparseable"
  (→ refuse to overwrite: either return an error or set a poison flag that makes `save` a no-op),
  so a parse failure cannot be laundered into a permanent default-write.

### [D-5]. `InstallPreferences.prefer_brew` and `SkillsConfig.prompt_budget` are readable but have no writer surface
- **File:line**: src/skill/config.rs:15 (`prefer_brew`), src/skill/config.rs:42-46 (`prompt_budget`)
- **Category**: inert-config
- **Severity**: LOW
- **Description**: Both fields are genuinely **read** in production — `prefer_brew` at
  `src/skill/installer.rs:144` (install-kind ranking) via `mod.rs:413`, and `prompt_budget` at
  `src/skill/mod.rs:534,561` → `harness_bridge/prompt_build.rs:516` → `SkillInstructionsLayer`.
  What is missing is the other half: **nothing in the repo ever writes them**. `SkillConfigUpdate` has
  only `SetEnabled` / `SetScope`, so `apply_update` cannot reach either field; there is no tool, no
  `skills.*` RPC param, and no CLI flag. The only way a user can change them is hand-editing
  `~/.aleph/data/skills.toml`, a path documented nowhere outside these source comments. Under R8
  ("工具即一切") a configurable with no conversational face is a knob nobody knows exists — the same
  shape as the "no reader" knobs in CLAUDE.md §5.23, mirrored.
- **Verification grep**:
  ```
  grep -rn "prefer_brew" --include="*.rs" src/ interfaces/
  # writes: only src/skill/config.rs:23 (Default) and :121 / installer.rs:398,426 (tests)
  grep -rn "prompt_budget" --include="*.rs" src/ interfaces/ | grep -v thinker::prompt_budget
  # writes: only src/skill/config.rs:161 (test) and snapshot.rs:47,134 (::default())
  ```
- **Triage**: DECIDE — either CONNECT (add `SetPreferBrew` / `SetPromptBudget` arms to
  `SkillConfigUpdate` and expose them via `self_config` or `skills.update`), or accept them as
  hand-edit-only and say so in a doc.
- **Proposed fix**: if CONNECT, extend `SkillConfigUpdate` and route through the existing
  `skills.update` handler (`src/gateway/handlers/skills.rs:46`) — the persistence path already exists;
  only the update variants are missing.

### [D-6]. Two distinct public types named `SkillsConfig`
- **File:line**: src/skill/config.rs:37 vs src/config/types/skills.rs:15
- **Category**: name-drift
- **Severity**: LOW
- **Description**: `skill::config::SkillsConfig` (per-skill overrides + install prefs + prompt budget,
  persisted to `~/.aleph/data/skills.toml`) and `config::types::skills::SkillsConfig` (`enabled` +
  `skills_dir`, persisted under `[skills]` in the main Aleph config) are unrelated types sharing a name.
  The confusion is live rather than theoretical: `rebuild_snapshot` (mod.rs:527-531) serializes
  `crate::config::Config` — which contains the *other* `SkillsConfig` under key `skills` — and feeds it
  to `EligibilityService::evaluate` for `required_config` lookups, right next to a read of the
  *first* one. No functional bug found; flagged so a future `required_config: "skills.prompt_budget"`
  author does not assume the wrong table.
- **Verification grep**:
  ```
  grep -rn "SkillsConfig" --include="*.rs" src/ | grep "struct SkillsConfig"
  # src/skill/config.rs:37, src/config/types/skills.rs:15
  ```
  Only `skill::config::SkillsConfig` is re-exported at the crate root (`src/lib.rs:205`), so there is
  no import ambiguity today.
- **Triage**: DECIDE
- **Proposed fix**: rename `skill::config::SkillsConfig` → `SkillOverridesConfig` (or
  `SkillPrefsConfig`), updating the 3 non-test references (`mod.rs:71,88`, `lib.rs:205`).

## Verified wired (no-op, do NOT re-flag)

**config.rs**
- `InstallPreferences` / `prefer_brew` — read at `installer.rs:141,144` via `mod.rs:413` (`select_best_install`).
- `SkillEntryConfig.enabled` / `.scope_override` — read at `snapshot.rs:95,110` and `status.rs:70,73`.
- `SkillsConfig.entries` — read at `mod.rs:534`, `config.rs:87,91`.
- `SkillsConfig.prompt_budget` (read side) — `mod.rs:534,561` → `prompt_build.rs:516,563` → `PromptConfig.skill_prompt_budget`.
- `SkillsConfig::load` — `mod.rs:88`. `::save` — `mod.rs:388`. `::get_entry` — `mod.rs:221`. `::apply_update` — `mod.rs:387`.
- `SkillConfigUpdate::SetEnabled` — `gateway/handlers/skills.rs:59`, `gateway/handlers/extensions/lifecycle.rs:75`, `builtin_tools/skill_manage.rs:322`.
- `SkillConfigUpdate::SetScope` — `gateway/handlers/skills.rs:85`, `builtin_tools/skill_manage.rs:338`.

**snapshot.rs**
- `SkillSnapshot::empty()` — `mod.rs:93`. `::build()` — `mod.rs:550`.
- `SkillSnapshot.eligible_manifests` — `prompt_build.rs:381,518`, `skill_manage.rs:918,932`.
- `SkillSnapshot.prompt_budget` — `prompt_build.rs:516`.
- `build()`'s `entries` and `archived` params — both fed live from `mod.rs:532-548`.

**status.rs**
- `SkillStatusEntry::build` — `mod.rs:224`. `::matches_filter` — `builtin_tools/skill_status.rs:49`.
- All 4 `SkillStatusFilter` variants — matched at `builtin_tools/skill_status.rs:38-41`.
- All 3 `MissingRequirements` fields — populated at `status.rs:82-87` from
  `IneligibilityReason::{MissingBinary, MissingAnyBinary, MissingEnv, MissingConfig}`, each of which is
  constructed at `eligibility.rs:112,120,127,134`. Read by Panel at `settings/skills.rs:76-78,660,673`.
- `InstallOption.{id,label,bins}` — read by Panel at `settings/skills.rs:679,690,691`. `.kind` is folded
  into `label` at `status.rs:106` and is additionally serialized into the model-facing
  `skill_status` output, so it has a consumer.
- `SkillStatusEntry.{id,name,description,emoji,source,source_label,homepage,eligible,disabled,missing,
  install_options,primary_env,api_key_set,scope,user_invocable}` — all read by
  `interfaces/webchat/src/platform/wide/views/settings/skills.rs` and/or `interfaces/cli/src/commands/skills_cmd.rs`
  and/or `src/hub/reconcile.rs:56`. **No name drift**: the Panel's hand-copied DTO (`settings/skills.rs:14-54`)
  matches the core struct field-for-field; `scope: PromptScope` and `kind: InstallKind` are both
  `#[serde(rename_all="lowercase")]` unit enums (wire = string) so the Panel's `String` decodes cleanly,
  and `SkillId` is a newtype (wire = string) so `id: String` decodes cleanly. Panel omits `usage`,
  which serde tolerates; `usage` is consumed on the model face instead (`skill_status.rs:50-53`).

**usage.rs**
- `record_use_in_dir` — `mod.rs:366` (slash-command path) and `builtin_tools/skill_reader/read.rs:411` (tool path).
- `UsageStore::new` — 8 production sites (`mod.rs:239,268,330,339,350,376,484`, `skill_lifecycle.rs:74`).
- `::get` — `mod.rs:268,330`. `::snapshot` — `mod.rs:239`. `::record_view` — `skill_reader/read.rs:413`.
- `::record_use` — via `record_use_in_dir`. `::record_patch` — `mod.rs:376` ← `skill_manage.rs:302,346,572`, `skill_install.rs:75`.
- `::set_state` — `mod.rs:350` ← `skill_manage.rs:685,686`; also `skill_lifecycle.rs:92`.
- `::set_pinned` — `mod.rs:339` ← `skill_manage.rs:683,684`. `::forget` — `mod.rs:484` (`remove_skill`).
- All 3 `SkillState` variants matched: `Active` (`skill_manage.rs:686`, `skill_lifecycle.rs:143`),
  `Stale` (`skill_status.rs:53`, `skill_lifecycle.rs:92,144`), `Archived` (`mod.rs:545`, `skill_manage.rs:638,654,685`).
- `UsageStats.{use_count,view_count,last_used_at,last_viewed_at}` — `tools/usage/report.rs:485,487`.
  `.patch_count`/`.last_patched_at` — serialized to the model via `skill_status`, referenced in its
  `DESCRIPTION`. `.created_at` — `skill_lifecycle.rs:83` fallback anchor. `.state` — `mod.rs:545`,
  `skill_status.rs:53`, `skill_lifecycle.rs:143`. `.pinned` — `skill_lifecycle.rs:76`,
  `skill_manage.rs:583`, `tools/usage/report.rs:316,493`. `.archived_at` — no Rust reader, but it is a
  `Serialize` field on the model-facing `skill_status` payload, which per the repo's own criterion
  ("`#[derive(Serialize)]` 的 struct 字段没有 Rust 消费者是正常的") counts as wired.
- `UsageStats::latest_activity_at` — `skill_lifecycle.rs:83,280`.

## Not verified
- No `cargo check` / `cargo test` was run (per instructions). All findings are static.
- Runtime reachability of the crash window in D-4 was reasoned about from call sites, not reproduced.
