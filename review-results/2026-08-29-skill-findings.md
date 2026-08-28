# Logic Review Report
**Module**: `src/skill/`
**Scope**: 13 files, ~6,103 LOC (`compat.rs` 50, `shared.rs` 81, `installer.rs` 483, `manifest.rs` 758, `registry.rs` 184, `config.rs` 239, `preprocess.rs` 356, `usage.rs` 444, `guard.rs` 423, `prompt.rs` 743, `cooccurrence.rs` 266, `snapshot.rs` 398, `mod.rs` 1050, `eligibility.rs` 368, `status.rs` 260)
**Date**: 2026-08-29
**Branch**: `audit/2026-08-29-skill`
**Mode**: strict (security-critical)

## Summary
| Level | Count |
|-------|-------|
| Critical | 0 |
| Warning | 4 |
| Suggested Test | 3 |

## Findings

### [Warning] `parse_skill_file` size cap silently bypassed when `std::fs::metadata` fails
- **Location**: `src/skill/manifest.rs:208-219` (pre-fix)
- **Trigger condition**: A SKILL.md whose `metadata()` call fails (broken symlink, permission-denied stat, vanished inode) but whose `read()` succeeds — for example, a network mount that returns metadata EIO but lets the read go through. The previous `if let Ok(meta) = ...` pattern meant the cap was *only* enforced on the happy path.
- **Expected behavior**: A file whose size we cannot determine must be refused, not loaded — the same defensive posture the YAML/ReDoS budget already takes against unbounded input.
- **Actual behavior**: The previous `if let Ok(meta) = ...` silently fell through to `read()` when stat failed. A multi-GB payload at a path that was unreadable via stat but readable via read would have OOMed the parser.
- **Suggested fix**: Replaced `if let Ok(meta) = std::fs::metadata(...).map(|m| ...)` with `let meta = std::fs::metadata(...).map_err(SkillParseError::Io)?;` — fail-closed on stat failure, surfacing it as `SkillParseError::Io`. See `src/skill/manifest.rs:211-216` (post-fix).

### [Warning] `crate::config::Config::load()` inside async context causes eligibility flap + redundant disk I/O on every rebuild
- **Location**: `src/skill/mod.rs:198-207` and `src/skill/mod.rs:524-531` (pre-fix)
- **Trigger condition**: Every call to `full_status()` and `rebuild_snapshot()` re-invoked `Config::load()`, which is a synchronous disk read + TOML parse that *also* writes a defaults file when the config is missing. Two real problems flowed from this:
  1. **Eligibility flap**: a transient I/O error (file locked, transient EIO on a slow disk) would replace the loaded `serde_json::Value` with the empty object fallback. Skills with `required_config: ["foo.bar"]` would flip from "ready" to "needs setup" on a single rebuild and back on the next — visible to the user as flapping status without any actual config change.
  2. **Performance**: every rebuild (and every status RPC) re-reads + re-parses the entire main config from disk, even when nothing changed.
- **Expected behavior**: Transient failures should not change the eligibility verdict; an unchanged config should be served from cache.
- **Actual behavior**: Both rebuild and status hit disk on every call and flap on transient failure.
- **Suggested fix**: Added `Inner::cached_config_value: RwLock<Option<serde_json::Value>>` and a private `load_or_cached_config_value()` helper. On success the value is written into the cache; on failure the cached value (or the initial empty object) is returned with a `tracing::warn!`. `full_status()` and `rebuild_snapshot()` now route through the helper. See `src/skill/mod.rs:75-82, 188-230, 257, 575`.

### [Warning] Unknown scope / install kind silently dropped, hiding frontmatter typos from skill authors
- **Location**: `src/skill/manifest.rs:285-302` (scope) and `src/skill/manifest.rs:344-373` (install kind)
- **Trigger condition**: A skill author writes `scope: sytem` (typo) or `install.kind: brewx` (typo). Both branches previously swallowed the error — the skill's `scope` silently became `Disabled` (invisible from the prompt index) and the install spec was filtered out, with no log line to indicate what happened. The skill parses and installs but does not behave as expected.
- **Expected behavior**: The skill still defaults to a safe state (`Disabled`, `filter_map`-drop) but emits a `tracing::warn!` naming the offending field and value so the author can diagnose from the operator log.
- **Actual behavior**: Silent default. Operators debug by trial-and-error.
- **Suggested fix**: Added `tracing::warn!` in both branches (`src/skill/manifest.rs:303-307` and `src/skill/manifest.rs:375-379`). Defaults are preserved — only observability changed.

### [Warning] Sync `which::which` calls inside `rebuild_snapshot` block the async runtime
- **Location**: `src/skill/eligibility.rs:111-115` (called from `src/skill/snapshot.rs:78-118` via `SkillSnapshot::build`, called from `src/skill/mod.rs::rebuild_snapshot`)
- **Trigger condition**: A skill with `required_bins: ["docker", "kubectl", "terraform", "aws", "gcloud", "gh", ...]` causes `which::which` to be invoked once per bin per rebuild, synchronously, on the tokio runtime thread. For 100 skills × 6 bins, that's 600 spawn-shells-on-PATH lookups per rebuild. On slow PATHs (Windows, networked filesystems) this can starve other tasks.
- **Expected behavior**: Either amortise the lookups (cache per `(bin, path-modtime)`), batch in `tokio::task::spawn_blocking`, or expose a fast-path for already-resolved bins.
- **Actual behavior**: Every rebuild walks the full PATH for every required bin. Not catastrophic for a 50-skill library but noticeable on a 500+ skill host.
- **Suggested fix**: NOT applied in this pass — requires architectural decision (where to cache; what the invalidation key is; whether to allow operators to disable the check). Documented as a follow-up.

### [Suggested Test] Cache fallback under transient `Config::load` failure
- **Location**: `src/skill/mod.rs:188-230`
- **Why**: The new `load_or_cached_config_value` helper is invoked from two sites with identical contract (success caches, failure falls back). A regression that breaks the fallback (e.g. accidentally re-routing one site through `Config::load().unwrap_or(json!({}))`) silently reintroduces the flap and would not be caught by any current test.
- **Suggested shape**:
  ```rust
  #[tokio::test]
  async fn cached_config_value_survives_transient_failure() {
      // First call seeds the cache via a successful Config::load.
      let system = SkillSystem::new();
      let _ = system.full_status().await;
      // Subsequent calls under forced failure must return the seeded value.
      // Hard to simulate without a mock for Config::load — pin via a known
      // required_config key plus a forced Config::load failure instead.
      todo!("needs Config::load injection seam or a #[cfg(test)] shim");
  }
  ```
  Until the seam exists, the cache is exercised by the production code path
  (every successful `full_status` updates the cache; every `rebuild_snapshot`
  reads it). The warning above documents the real failure mode.

### [Suggested Test] Many small `required_bins` in async context
- **Location**: `src/skill/eligibility.rs:108-114`
- **Why**: The Performance Warning above will only surface on a host with many bins. A regression that switched the `which::which` calls to a sync loop on a 100-bin skill would not be caught without an explicit stress test.
- **Suggested shape**:
  ```rust
  #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
  async fn rebuild_with_many_required_bins_does_not_starve() {
      let m = manifest_with_bins(&["a","b","c", /* ... 50 fake bins ... */]);
      // Assert wall-clock budget is small and that other tasks can make
      // progress on the same runtime during the rebuild.
  }
  ```

### [Suggested Test] `SkillSystem::remove_skill` on disk-with-stale-registry resurrection
- **Location**: `src/skill/mod.rs:419-462`
- **Why**: `remove_skill` deletes the on-disk directory best-effort; if it fails, the registry is updated but the skill file remains, so the next `rescan_dirs` resurrects the skill. The current behaviour is documented as "warn and continue" but no test pins it.
- **Suggested shape**:
  ```rust
  #[tokio::test]
  async fn remove_skill_resurrects_when_disk_delete_fails() {
      // Make the skill dir read-only, remove, then rescan — assert the skill
      // comes back. This pins the documented "best-effort" semantics so a
      // future "fail closed" change is a deliberate decision.
  }
  ```

## Cross-Module Findings

### [Warning] Sync I/O from `Config::load()` and `SkillsConfig::save()` on async paths in multiple modules
- **Modules**: `src/skill/mod.rs`, `src/builtin_tools/skill_manage.rs`, `src/builtin_tools/skill_install.rs`, `src/gateway/handlers/skills.rs`
- **Risk**: Both `Config::load()` (in `rebuild_snapshot` / `full_status`) and `SkillsConfig::save()` (in `update_config`) do synchronous file I/O on the tokio runtime thread. Under a slow or contended filesystem, every status RPC and every skill-mutation blocks the executor.
- **Suggested fix**: Out of scope for this module — see `src/config/load.rs:320` (`Config::load` is `pub fn load()` not `async`) and `src/skill/config.rs:128-141` (save is sync). The skill module's new cache helps in steady state but does not move the synchronous load off the runtime thread on the first call. A cross-module refactor to wrap these in `tokio::task::spawn_blocking` (or to make them natively async) belongs in a dedicated audit pass.

### [Warning] `SkillId::new` does not validate path-unsafe characters; defence is at every consumer
- **Modules**: `src/domain/skill.rs:24-30` (`SkillId::new`), `src/skill/mod.rs:259-300` (`owning_dir`/`skill_dir_for_id` traversal guard), `src/builtin_tools/skill_reader/` (separate validation)
- **Risk**: `SkillId::new("../etc/passwd")` is accepted. The defence lives at the consumer side — `owning_dir` rejects `..`, `/`, `\` — but a new consumer that forgets the guard would gain a path-traversal primitive. The defence-by-convention pattern is fragile; centralising the validation in `SkillId::new` would be safer.
- **Suggested fix**: Add a `validate` step in `SkillId::new` (or a `try_new`) that rejects empty + `..` + `/` + `\` + NUL at the type boundary. Existing call sites that intentionally construct `SkillId` from a `parse_skill_content`-validated string continue to work; new RPC / IPC consumers cannot accidentally bypass the guard.

## Wiring Audit Summary

`SkillSystem` public API surface, verified via the call graph
(`graphify-out/2026-08-28/graph.json` + grep):

| Method | Caller(s) | Status |
|---|---|---|
| `SkillSystem::new` | `shared_skill_system`, builtin_tools tests | wired |
| `SkillSystem::init` | `src/extension/projection.rs:139`, tests | wired |
| `SkillSystem::reload_file` | `src/builtin_tools/skill_manage.rs:302,435` | wired |
| `SkillSystem::ensure_dir_registered` | `src/skill/shared.rs:58`, `src/builtin_tools/skill_manage.rs:433` | wired |
| `SkillSystem::current_snapshot` | `src/orchestrator/harness_bridge/prompt_build.rs:241`, tests | wired |
| `SkillSystem::get_skill` | `src/builtin_tools/skill_manage.rs`, `src/bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs:157` | wired |
| `SkillSystem::list_skills` | `src/bin/aleph-server/.../tool_catalog_init.rs:157` | wired |
| `SkillSystem::full_status` | `src/builtin_tools/skill_status.rs:93`, `src/gateway/handlers/skills.rs`, `src/hub/reconcile.rs:86` | wired |
| `SkillSystem::install_dependency` | `src/gateway/handlers/skills.rs:141` | wired |
| `SkillSystem::remove_skill` | `src/gateway/handlers/skills.rs:176`, `src/gateway/handlers/extensions/lifecycle.rs:114` | wired |
| `SkillSystem::update_config` | `src/gateway/handlers/skills.rs:67,93`, `src/gateway/handlers/extensions/lifecycle.rs:73` | wired |
| `SkillSystem::record_patch` | `src/builtin_tools/skill_manage.rs:305,349,575`, `src/builtin_tools/skill_install.rs:86` | wired |
| `SkillSystem::record_use` | `src/gateway/execution_engine/slash_command.rs:256` | wired |
| `SkillSystem::set_pinned` | `src/builtin_tools/skill_manage.rs:622,884,897` | wired |
| `SkillSystem::set_skill_state` | `src/builtin_tools/skill_manage.rs:653` | wired |
| `SkillSystem::usage_for` | `src/builtin_tools/skill_manage.rs:585` | wired |
| `SkillSystem::locate_skill_file` | `src/builtin_tools/skill_manage.rs:273` | wired |
| `SkillSystem::rescan_dirs` | `src/skill/shared.rs:60`, `src/skill/mod.rs::init` | wired (private fan-out) |
| `SkillSystem::rebuild_snapshot` | internal only | wired (private fan-out) |

- Total `pub fn` (or `pub async fn`) on `SkillSystem`: 17 (above) + `Default`, `Debug`, `Clone`.
- Verified callers: 17 of 17.
- Orphaned `pub fn`s: 0.

Free-function exports via `src/skill/mod.rs:14-38`:

| Export | Caller(s) | Status |
|---|---|---|
| `compat::SkillInfo` (and `From` impls) | `src/skill/compat.rs` tests | wired |
| `config::{InstallPreferences, SkillConfigUpdate, SkillEntryConfig, SkillsConfig}` | `src/builtin_tools/skill_manage.rs`, `src/gateway/handlers/skills.rs` | wired |
| `cooccurrence::{cluster_chains, CoOccurrenceLog, RecentUse}` | `src/skill/cooccurrence.rs` tests, `src/skill/usage.rs:413` | wired |
| `eligibility::{EligibilityResult, EligibilityService, IneligibilityReason}` | `src/skill/mod.rs`, `src/skill/snapshot.rs`, `src/skill/status.rs` | wired |
| `guard::{install_allowed, merge_verdicts, scan_content, scan_skill_directory, ScanVerdict, ThreatLevel, TrustLevel}` | `src/skill/manifest.rs`, `src/builtin_tools/skill_install.rs`, `src/gateway/handlers/markdown_skills.rs` | wired |
| `installer::{build_install_command, filter_install_specs_for_current_os, select_best_install, InstallExecutor, InstallResult}` | `src/skill/mod.rs`, `src/skill/status.rs`, `src/builtin_tools/skill_install.rs` | wired |
| `manifest::{automation_notice, parse_skill_content, parse_skill_file, SkillParseError}` | `src/skill/mod.rs`, `src/builtin_tools/skill_install.rs`, `src/builtin_tools/skill_manage.rs` | wired |
| `preprocess::{preprocess_skill_content, SkillPreprocessContext}` | `src/builtin_tools/skill_reader/` | wired |
| `prompt::{build_skills_prompt_xml, SkillPromptBudget}` | `src/thinker/layers/skill_instructions.rs`, `src/thinker/prompt_builder/` | wired |
| `registry::SkillRegistry` | `src/skill/snapshot.rs`, `src/skill/mod.rs` | wired |
| `shared::{ensure_shared_skill_system_initialized, shared_skill_system}` | `src/gateway/handlers/skills.rs`, `src/hub/reconcile.rs`, `src/extension/mod.rs`, builtin_tools | wired |
| `snapshot::SkillSnapshot` | `src/orchestrator/harness_bridge/prompt_build.rs` | wired |
| `status::{InstallOption, MissingRequirements, SkillStatusEntry, SkillStatusFilter}` | `src/builtin_tools/skill_status.rs` | wired |
| `usage::{record_use_in_dir, SkillState, UsageStats, UsageStore}` | `src/builtin_tools/skill_reader/`, `src/skill/cooccurrence.rs`, `src/skill/mod.rs` | wired |

No orphaned `pub` exports detected.

`SkillStatusFilter::All` is consumed only by `SkillStatusTool::Readiness` in `builtin_tools/skill_status.rs:38-41`; `Disabled` is the same enum; the variant ordering (All, Ready, NeedsSetup, Disabled) is total over the readiness matrix and there are no `_ => unreachable!()` arms.

## What was NOT reviewed

- **`src/builtin_tools/skill_*` (skill_manage, skill_install, skill_status, skill_reader)** — out of scope; these are the consumers of the `src/skill/` module and their wiring was only spot-checked.
- **`src/utils/atomic_io.rs::with_file_lock`** — used by `usage.rs` and `cooccurrence.rs`; if it deadlocks (no timeout, can block forever on a crashed peer), the skill module inherits the deadlock but the bug lives in `utils/`.
- **`src/config/load.rs::Config::load`** — sync I/O on async paths; the cross-module finding above flags it but the fix is in `config/`, not here.
- **`src/domain/skill.rs::SkillId`** — defensive validation belongs at the type boundary; addressed only as a cross-module finding above.
- **`src/bundled/manifest.rs::InstallRegistry`** — cached inside `guess_source` via `OnceLock`; out of scope.
- **`src/security/unicode_guard`** — referenced by `guard.rs::scan_content`; out of scope.

## Concerns about the changes applied in this pass

1. **Cache field on `Inner`** — adding `cached_config_value: RwLock<Option<serde_json::Value>>` adds a second lock to `Inner`. Existing tests for `SkillSystem::new` exercise the construction path; the new lock is initialised to `None`, matching the previous "empty fallback on first failure" semantics. No behavioural regression expected.

2. **`parse_skill_file` fail-closed on stat failure** — the existing tests all create real files via `std::fs::write`, so `metadata()` succeeds; no regression. The new test (`parse_skill_file_rejects_unstatable_file`) is `#[cfg(unix)]` and uses a broken symlink to exercise the failure path; it should pass on Linux but is skipped on Windows where symlink creation requires elevated rights.

3. **`tracing::warn!` in unknown scope / install kind** — log volume increases by one line per affected skill at parse time. For a 100-skill library where one typo slipped in, this is one extra warn — negligible. For a tool that intentionally parses thousands of skills in batch, the volume scales linearly; not a concern for the in-process paths.

4. **`rebuild_snapshot` still calls `Config::load()`** — the cache avoids the second call in the same rebuild but does not move the synchronous load off the tokio runtime thread. A burst of `Config::load` calls (first build, or after the cache is invalidated by a write elsewhere) still blocks the executor. This is left as a follow-up — the warning above documents it.