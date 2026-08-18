# Code review — `src/bundled/` (2026-08-19 round r4)

## Scope
- Files reviewed (production `src/bundled/`, ~1,500 LoC + tests):
  - `extractor.rs` — embedded-content extraction, atomic writes, prune, symlink-safe staging
  - `manifest.rs` — `InstallRegistry` / `SkillEntry` / `SkillOrigin` schema + `save`/`load`/`reconcile`
  - `sync.rs` — git2 clone/fetch/pin/reset of the official repos
  - `mod.rs` — public re-exports + `BUNDLED_SKILLS` / `BUNDLED_PLUGINS` / `BUNDLED_VERSION` constants
- LoC total: ~1,500 (review-time counts: extractor ~770, manifest ~290, sync ~220, mod ~40)
- Cross-checked callers:
  - `bin/aleph-server/commands/start/helpers.rs:355` — `extract_bundled_content` via `spawn_blocking`
  - `gateway/handlers/bundled_sync.rs:5,21-25,49` — `bundled.sync` RPC → `sync_official_now`
  - `hub/install.rs:168,196,206-211,602-605` — `clone_or_update_at` + `copy_skill_leaf` + manifest writes for `GitDir` skill installs
  - `hub/official_skills.rs:8` and `hub/official_plugins.rs:9` — read-only `BUNDLED_*` / `OFFICIAL_*_REPO` projections
  - `skill/mod.rs:712-723` — `CACHED_MANIFEST` consumes `InstallRegistry::load`/`is_official`
- Method: read-first sweep of every file in scope, focused on (1) the file-system primitives (atomic rename, symlink metadata, tmp-file naming), (2) the manifest lifecycle (load → reconcile → save), (3) the clone/pin semantics in `sync.rs`, and (4) the public surface documented in `mod.rs`. Cross-checked the in-tree callers above to confirm the documented public API is used as advertised. Did **not** audit the bundled `skills/` or `plugins/` trees (build-time artifacts), nor the `include_dir!` macro internals.

## Findings

### BUNDLED-R4-01 — `manifest.rs::save` is vulnerable to a symlink attack via `manifest.tmp`
- **File**: `src/bundled/manifest.rs:78-99`
- **Severity**: High
- **Category**: security
- **Description**: `save()` opens `path.with_extension("tmp")` (i.e. `~/.aleph/skills/manifest.tmp`) with `OpenOptions::create_new(true)`. On Unix this maps to `O_CREAT | O_EXCL`, which **follows symlinks**: if an attacker (or a co-tenant LLM tool running as the same daemon user) plants a symlink at `manifest.tmp` pointing at an arbitrary destination that does not yet exist, `create_new(true)` succeeds and the serialized manifest — including any Github `url` field with attacker-controlled contents — is written to the symlink target. There is no `O_NOFOLLOW` flag, no `symlink_metadata` pre-check, and the tmp path is a fixed string so the attacker has unlimited time to plant the link before the next `save()`. The symlink-target write is also unprivileged (the daemon user can normally write anywhere it has access to), so this turns into arbitrary-file-overwrite within the daemon user's authority.
- **Evidence**:
  ```rust
  // manifest.rs:78-85
  let path = skills_dir.join("manifest.json");
  let content = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
  let tmp_path = path.with_extension("tmp");
  match std::fs::OpenOptions::new()
      .write(true)
      .create_new(true)
      .open(&tmp_path)
  ```
  No `custom_flags(libc::O_NOFOLLOW)`, no `symlink_metadata` pre-check on `tmp_path`. Compare with the rest of the file, which goes out of its way to use `symlink_metadata` (`reconcile()` at lines 158-172) and to peel symlinks (`prepare_plugin_temp_dir` in extractor.rs:395-419) precisely to avoid this class of bug.
- **Suggested fix**:
  1. Refuse to write through a symlink: `if std::fs::symlink_metadata(&tmp_path).map(|m| m.file_type().is_symlink()).unwrap_or(false) { return Err(io::Error::new(io::ErrorKind::AlreadyExists, "manifest.tmp is a symlink")) }` before opening. Use a unique-per-call tmp name (`manifest.{pid}.{counter}.{nanos}.tmp`) so the symlink race window is effectively zero — this also fixes BUNDLED-R4-07 below.
  2. Or, on Unix only, gate the open with `OpenOptionsExt::custom_flags(libc::O_NOFOLLOW | libc::O_EXCL)` so the kernel refuses to traverse a symlink at `tmp_path`.
  3. Add a regression test: in a tempdir, `symlink("/tmp/elsewhere", manifest.tmp)`, call `save()`, assert the manifest was written under `manifest.json` and not into `/tmp/elsewhere`.
- **Verification**: Confirmed by reading every call site of `manifest.rs::save` (`extractor.rs:226, 254` and `hub/install.rs:215`) — each writes into the user's `~/.aleph/skills/`, which is the only location this is exploitable. Compared with `prepare_plugin_temp_dir` (`extractor.rs:395-419`), which uses the same pattern but adds a `symlink_metadata` + `remove_file` peel — the manifest path needs the same defense. The `symlink_metadata` usage in `manifest.rs::reconcile()` (line 161) demonstrates the codebase already understands the threat model; the omission in `save()` is the gap.

### BUNDLED-R4-02 — `extract_dir_contents` rename-then-replace fails on directory collisions, creating a permanent stuck state
- **File**: `src/bundled/extractor.rs:498-531`
- **Severity**: High
- **Category**: correctness / error-handling
- **Description**: For every bundled file `f` whose basename already exists at `target/f`, the function writes `target/.<f>.tmp.<pid>.<counter>.<nanos>` and renames it onto `target/f`. If the rename fails with `AlreadyExists`, the recovery branch removes `dest` with `std::fs::remove_file(&dest)` and retries. But `dest` can be a directory — for example, a user has `~/.aleph/skills/<name>/subdir/` where `<name>/subdir` happens to match the basename of a bundled file. On that collision `std::fs::remove_file(&dest)` returns `Err`, the rename retry fails, and the function bubbles the `io::Error` up to `extract_skills`. `extract_skills` records the skill as `SkillOrigin::Official` with `version: None` and leaves the half-written tree. `bundled_version` is **not** bumped because `skills_ok && plugins_ok` is false, so every subsequent startup retries the same failing extraction. There is no retry budget, no escalation, and the failure is only visible as a `warn!` line that an operator rarely sees.
- **Evidence**:
  ```rust
  // extractor.rs:518-528
  if let Err(e) = std::fs::rename(&tmp, &dest) {
      if e.kind() == std::io::ErrorKind::AlreadyExists {
          let _ = std::fs::remove_file(&dest);
          if let Err(e) = std::fs::rename(&tmp, &dest) {
              let _ = std::fs::remove_file(&tmp);
              return Err(e);
          }
      } else {
          let _ = std::fs::remove_file(&tmp);
          return Err(e);
      }
  }
  ```
  No branch on the file-type of `dest`. Compare with `swap_dir_into_place` (`extractor.rs:436-456`) which does handle the dir-vs-file distinction via `remove_dir_all` for the same reason — that fix should apply here as well.
- **Suggested fix**: Replace the `AlreadyExists` branch with a stat-and-decide: if `dest.symlink_metadata()?.is_dir()`, `remove_dir_all(&dest)`; otherwise `remove_file(&dest)`. Then retry the rename. Better still, do the pre-check before writing the temp file and surface a clear error like "destination `…` is a directory, refusing to overwrite" so the caller can decide whether to skip or refuse. Add a `MAX_EXTRACT_FAILURES` counter on `SkillEntry` so a permanently-stuck skill is downgraded to `Local` after N tries instead of looping forever.
- **Verification**: Traced the failure flow from `extract_dir_contents` up through `extract_dir_recursive` → `extract_skills` (`extractor.rs:268-296`) → `extract_bundled_content` (`extractor.rs:228-261`). Confirmed `extract_skills` records `version: None` on failure (line 286-295) and confirmed `bundled_version` is only bumped on `skills_ok && plugins_ok` (line 252). No upper bound on retries. No test for the directory-collision case exists (searched the `#[cfg(test)]` block in `extractor.rs`).

### BUNDLED-R4-03 — `extract_dir_contents` does not prune `target/` for files that are not in the bundle
- **File**: `src/bundled/extractor.rs:498-531`
- **Severity**: Medium
- **Category**: correctness
- **Description**: Pruning only happens at the end of `extract_dir_recursive` (`extractor.rs:459-462`) and only after `extract_dir_contents` returns `Ok(())`. If the user has previously installed a skill under `~/.aleph/skills/<name>/` (manually or via a Github install) and the next bundled upgrade bundles a **different set** of children for `<name>` (e.g., a subdir the user added locally, or a file that used to be bundled and is now removed), the order of operations is: (a) `extract_dir_contents` writes the new files, (b) the rename-on-collision branch in `extract_dir_contents` only handles `AlreadyExists` for **files**, not for directories the user added. The user-added files are left in place. The follow-up `prune_stale_entries` does clean them up — **but only when extraction succeeds**. The moment `extract_dir_contents` errors out for one file (e.g., BUNDLED-R4-02), pruning is skipped entirely, so stale entries from the previous install remain on disk forever, even though the skill is marked Official with `version: None`. Combined with BUNDLED-R4-02, this means a directory-collision blocks both the upgrade **and** the prune.
- **Evidence**:
  ```rust
  // extractor.rs:459-462
  fn extract_dir_recursive(dir: &Dir, target: &Path) -> std::io::Result<()> {
      std::fs::create_dir_all(target)?;
      extract_dir_contents(dir, target)?;
      prune_stale_entries(dir, target)?;
      Ok(())
  }
  ```
  `prune_stale_entries` only runs if `extract_dir_contents` returns `Ok`. The bug is masked today because `extract_dir_contents` rarely fails in practice (disk-full is the main scenario); but combined with BUNDLED-R4-02 it becomes visible.
- **Suggested fix**: Always run `prune_stale_entries` after a successful `extract_dir_contents`, and on partial failure at least log which files could not be extracted and what state the target dir is left in. Better: split the function so a partial failure in one file does not abort the whole prune. Document the prune contract on the function (currently only on `extract_skill_tree_from_dir` / `copy_tree_with_prune`).
- **Verification**: Read the full `extract_dir_recursive` body, the `prune_stale_entries` signature, and the test coverage in `extractor.rs:783-863`. No test exercises a partial-failure scenario (e.g., simulating a write error mid-extraction) — the existing tests only cover the happy path and the empty-checkout / symlink-passthrough regressions.

### BUNDLED-R4-04 — `extract_bundled_content` returns `()`; the entire failure path is invisible to the daemon operator
- **File**: `src/bundled/extractor.rs:136-262`
- **Severity**: Medium
- **Category**: error-handling / observability
- **Description**: `extract_bundled_content` is the only entry point that runs at startup (`bin/aleph-server/commands/start/helpers.rs:355`). It returns `()`. Every error path inside (skills-dir creation failure, manifest corruption, clone failure, extraction failure, save failure, reconcile failure) is logged at `warn!` and silently swallowed. The caller wraps the call in `tokio::task::spawn_blocking(...).await` and discards the `Result<(), JoinError>` with `let _ =`. The result: a daemon that boots successfully even if **none** of the bundled skills or plugins made it onto disk. The `bundled.sync` RPC handler (`gateway/handlers/bundled_sync.rs:49-66`) does return errors to the caller, but startup extraction is invisible.
- **Evidence**:
  ```rust
  // bin/aleph-server/commands/start/helpers.rs:355
  let _ = tokio::task::spawn_blocking(move || {
      alephcore::bundled::extract_bundled_content(&home_for_extract)
  })
  .await;
  ```
  and `extract_bundled_content` has no `Result` return:
  ```rust
  // extractor.rs:136
  pub fn extract_bundled_content(aleph_home: &Path) {
  ```
- **Suggested fix**: Change the signature to `Result<(), BundledError>` (new enum with `Skills`, `Plugins`, `Manifest`, `DiskIo` variants). Bubble the error out via a tracing event AND a structured response. At minimum, return a `SyncReport`-style struct (`{skills_ok: bool, plugins_ok: bool, manifest_saved: bool, errors: Vec<String>}`) so the startup caller can decide whether to refuse to come up healthy when the local content is broken.
- **Verification**: The `bundled.sync` handler already shows the right shape — it returns JSON `{ok, skills, plugins}` plus an error. The startup entry point does not. The daemon-side `doctor` checks (`src/diagnostics/checks/duplicate_instance.rs`) demonstrate that startup-time invariants are usually surfaced there; nothing equivalent exists for bundled content.

### BUNDLED-R4-05 — `sync_official_with_urls` masks skill-extraction failure behind a partial-success `Ok`
- **File**: `src/bundled/extractor.rs:83-125`
- **Severity**: Medium
- **Category**: error-handling / correctness
- **Description**: If `clone_or_update(skills_url, &checkout)` succeeds and the checkout has content, the code calls `extract_skill_tree_from_dir(&checkout, &skills_dir, &mut manifest)`. If **that** returns `false` (any per-skill error during extract/copy/prune), `report.skills` is set to `false`. If the plugins branch then succeeds, the function returns `Ok(SyncReport { skills: false, plugins: true })` — caller-visible success. The caller (`extract_bundled_content::first_run` at lines 172-198) checks `if r.skills && r.plugins` to decide whether to fall through to the embedded snapshot; a partial-success falls through, which silently re-extracts skills from the embedded snapshot and **overwrites** whatever the network clone wrote (with the embedded contents). The user asked for an online sync and got an embedded fallback without being told.
- **Evidence**:
  ```rust
  // extractor.rs:100-105 (skills branch)
  Ok(()) => {
      let mut manifest = InstallRegistry::load(&skills_dir)
          .unwrap_or_else(|| InstallRegistry::new(""));
      if let Err(e) = manifest.reconcile(&skills_dir) { warn!(...); }
      report.skills = extract_skill_tree_from_dir(&checkout, &skills_dir, &mut manifest);
      ...
  }
  // extractor.rs:120-122 (final decision)
  if !report.skills && !report.plugins {
      return Err(last_err.unwrap_or_else(|| "nothing synced".into()));
  }
  Ok(report)
  ```
- **Suggested fix**: Surface per-branch errors in the report. Make `SyncReport` carry an `Option<String>` per branch so the `bundled.sync` RPC and `extract_bundled_content` first_run can both distinguish "skills extracted from network" from "skills fell back to embedded". The RPC can then return `{ok: false, skills_error: "..."}` instead of `Ok(report)` with hidden detail.
- **Verification**: Read `gateway/handlers/bundled_sync.rs:49-66` — the handler returns `{ok: true, skills, plugins}` without any error info. The test `sync_official_with_urls_extracts_skills_from_local_repo` (`extractor.rs:854-867`) covers the happy path only.

### BUNDLED-R4-06 — `CACHED_MANIFEST` in `skill/mod.rs` is set once and never invalidated, becoming stale after any `bundled.sync`
- **File**: `src/skill/mod.rs:712-728` (cross-caller)
- **Severity**: Medium
- **Category**: correctness
- **Description**: `skill/mod.rs:guess_source` caches `InstallRegistry::load(&global_skills)` in a `OnceLock<Option<InstallRegistry>>`. The first call from any thread loads the manifest and freezes it. After that, calls to `manifest.is_official(&name)` always answer against the snapshot — including after the `bundled.sync` RPC writes a fresh manifest to disk. A skill freshly installed as Official (or downgraded from Official to Local) keeps its old classification in the cache. Since `is_official` decides whether a skill is reported as `SkillSource::Bundled` (which affects prompt-index ranking and `skill_read` collision resolution — see the comment at `skill/mod.rs:697-705`), a stale cache can flip a user-installed skill between Global and Bundled across processes, even though the on-disk manifest is correct.
- **Evidence**:
  ```rust
  // skill/mod.rs:712-723
  static CACHED_MANIFEST: OnceLock<Option<crate::bundled::manifest::InstallRegistry>> =
      OnceLock::new();
  ...
  let manifest = CACHED_MANIFEST
      .get_or_init(|| crate::bundled::manifest::InstallRegistry::load(&global_skills));
  ```
- **Suggested fix**: Either (a) replace `OnceLock` with a `tokio::sync::RwLock<InstallRegistry>` reloaded on demand (the cache invalidation hook is the `bundled.sync` RPC, or `extract_bundled_content` can install the freshly-loaded manifest after each successful extract), or (b) document that the cache is only safe across the lifetime of the daemon and explicitly invalidate it in `bundled.sync`. The codebase already has a precedent: `skill/mod.rs` reloads on each `Skill::list()` call rather than caching; matching that pattern is simpler and correct.
- **Verification**: Confirmed `CACHED_MANIFEST` is the only reader of `InstallRegistry` outside `bundled/` itself (`grep -n 'InstallRegistry::load\|InstallRegistry::new' src/` returns skill/mod.rs, extractor.rs, manifest.rs, hub/install.rs). The `hub/install.rs` callers (`src/hub/install.rs:206,602`) re-load on every install, so they are correct; the cached reader in skill/mod.rs is the only stale path. The `bundled.sync` RPC handler (`gateway/handlers/bundled_sync.rs:49-66`) calls `sync_official_now` which writes the manifest but does not invalidate the cache.

### BUNDLED-R4-07 — Stale `manifest.tmp` from a crashed save blocks every subsequent `save()` call
- **File**: `src/bundled/manifest.rs:78-99`
- **Severity**: Medium
- **Category**: error-handling / resource
- **Description**: The tmp path is a fixed string: `path.with_extension("tmp")` → `manifest.tmp`. `OpenOptions::create_new(true)` fails with `AlreadyExists` if `manifest.tmp` already exists. The save function returns `Err` immediately in that case (the comment at lines 89-91 calls this intentional to refuse to clobber a concurrent writer). The problem is that a **crashed** save leaves `manifest.tmp` on disk forever, and the next save attempt — whether minutes later or on the next daemon restart — sees the leftover and errors out. The result is a manifest that can never be re-saved until the operator manually `rm ~/.aleph/skills/manifest.tmp`. The `extractor.rs::save` call sites (lines 226, 254) only log `warn!` and continue; the daemon happily runs with a manifest that can no longer be updated, and any new skill installation cannot update provenance.
- **Evidence**:
  ```rust
  // manifest.rs:80-91
  let tmp_path = path.with_extension("tmp");
  match std::fs::OpenOptions::new()
      .write(true)
      .create_new(true)
      .open(&tmp_path)
  {
      Ok(mut f) => { ... write ... }
      Err(e) => {
          // A concurrent writer is mid-save. Refuse rather than clobber
          // its tmp; the next save will pick a fresh name.
          return Err(e);
      }
  }
  ```
  The "next save will pick a fresh name" claim is false — `tmp_path` is a fixed name.
- **Suggested fix**: Use a unique-per-call tmp name (e.g. `manifest.{pid}.{counter}.{nanos}.tmp`). Then `create_new(true)` is purely a defense against accidental collisions, not against crashes. Alternatively, attempt to `remove_file` a stale tmp before opening (with a `File::open(...).metadata().created()` check to avoid clobbering a live concurrent writer — but uniqueness is simpler).
- **Verification**: Traced every `save` call (`extractor.rs:226, 254`; `hub/install.rs:215`). None clean up `manifest.tmp` on startup. The tests in `manifest.rs:226-289` do not exercise the crash-leftover scenario. `hub/install.rs:215` uses `let _ = manifest.save(...)` so the failure is also silent there.

### BUNDLED-R4-08 — `prepare_plugin_temp_dir` does not defend against symlinks that point to a directory (TOCTOU)
- **File**: `src/bundled/extractor.rs:395-419`
- **Severity**: Low
- **Category**: security / concurrency
- **Description**: The function calls `symlink_metadata(tmp_dir)` and peels the entry off if it's a symlink (`Ok(_) => remove_file`). Between the `symlink_metadata` and the subsequent `create_dir_all(tmp_dir)`, an attacker with write access to the cache parent can swap a non-symlink entry for a symlink (TOCTOU). The window is microseconds, but `create_dir_all` follows symlinks on Unix and would write through any symlink placed at `tmp_dir` in the gap. The comment claims the fix closes the gap, but only against a symlink present *before* this call — not against a symlink planted concurrently.
- **Evidence**:
  ```rust
  // extractor.rs:411-419
  Ok(_) => {
      // Symlink or other non-directory — remove the entry itself (a
      // symlink removal does not touch the target).
      if let Err(e) = std::fs::remove_file(tmp_dir) { ... }
  }
  ...
  if let Err(e) = std::fs::create_dir_all(tmp_dir) {
      warn!(error = %e, "Failed to create plugin cache temp directory");
      return false;
  }
  ```
- **Suggested fix**: Use `OpenOptions::create_new(true).write(true).custom_flags(libc::O_NOFOLLOW)` semantics by combining `create_dir_all` with a pre-check that races less — e.g., loop until `mkdir(tmp_dir)` returns `EEXIST` with the entry being a real directory, otherwise remove and retry; or use `O_DIRECTORY | O_NOFOLLOW | O_EXCL` on a placeholder file (the kernel-level "is this a directory and not a symlink" trick). Acceptable alternative: document the race as low-severity given the threat model requires co-tenant write access to `~/.aleph/`.
- **Verification**: The existing test `prepare_plugin_temp_dir_rejects_symlink_passthrough` (`extractor.rs:935-961`) covers the symlink-already-present case but not the concurrent-planting case. The `extractor.rs::extract_plugins_from_dir` (line 370) calls the same function; both inherit the same race.

### BUNDLED-R4-09 — `swap_dir_into_place` has a TOCTOU gap between `remove_dir_all(dest)` and `rename(staged, dest)`
- **File**: `src/bundled/extractor.rs:436-456`
- **Severity**: Low
- **Category**: concurrency / resource
- **Description**: On a non-empty dest (Unix upgrade case), the function removes `dest` and retries `rename(staged, dest)`. Between the `remove_dir_all` and the `rename`, a concurrent process (e.g., a second startup of the daemon racing with the first, or a plugin install in flight) can recreate `dest`. The retry rename fails, the function returns `false`, and the staged `tmp_dir` is left on disk. The next call retries — and either succeeds (the orphan is reused) or loops. There is no orphan-GC sweep, so a permanent failure mode accumulates stale `*.tmp` dirs in `~/.aleph/plugins/cache/` over time.
- **Evidence**:
  ```rust
  // extractor.rs:447-454
  if let Err(e) = std::fs::remove_dir_all(dest) { ... }
  if let Err(e) = std::fs::rename(staged, dest) {
      warn!(error = %e, "Failed to atomically swap plugin cache after removing old");
      return false;
  }
  ```
- **Suggested fix**: Use `renameat2` with `RENAME_EXCHANGE` on Linux (atomic swap) or fall back to a per-call unique staged name (`aleph-official.{pid}.{counter}.{nanos}.tmp`) and an opportunistic cleanup of any stale `*.tmp` dirs in `cache/` at the top of `extract_plugins`. The latter also fixes a separate concern: a permanent orphan `*.tmp` left behind by a crash blocks every subsequent `prepare_plugin_temp_dir` call until manual cleanup.
- **Verification**: The tests `swap_dir_replaces_nonempty_destination`, `swap_dir_into_empty_destination`, `swap_dir_preserves_nested_content` (extractor.rs:786-844) cover the serial happy paths but not a concurrent recreate. The `extract_plugins` callers (`extractor.rs:347` for startup, `extractor.rs:368` for explicit sync) both call `prepare_plugin_temp_dir` first, which has the same TOCTOU concern (BUNDLED-R4-08).

### BUNDLED-R4-10 — `InstallRegistry::save` does not `fsync` the tmp file or the parent directory before the rename
- **File**: `src/bundled/manifest.rs:88-93`
- **Severity**: Low
- **Category**: durability / correctness
- **Description**: The save writes `manifest.tmp`, then `rename`s it onto `manifest.json`. On POSIX, the rename is atomic, but the **durability** of the rename depends on whether the tmp file's contents and metadata have been flushed to disk. A power loss between the write and the rename results in either an empty `manifest.json` (rename never happened) or a torn file (rename happened but contents not flushed). The manifest is the authoritative provenance record for installed skills — losing it can downgrade every Official skill to "unknown" on next reconcile. The daemon's startup path always re-runs `extract_bundled_content`, which would silently re-classify skills as Local if the manifest is gone.
- **Evidence**:
  ```rust
  // manifest.rs:84-93
  Ok(mut f) => {
      if let Err(e) = f.write_all(content.as_bytes()) {
          let _ = std::fs::remove_file(&tmp_path);
          return Err(e);
      }
  }
  ...
  if let Err(e) = std::fs::rename(&tmp_path, &path) { ... }
  ```
  No `f.sync_all()` before the rename. No `File::open(parent_dir).sync_all()` after the rename.
- **Suggested fix**: Add `f.sync_all()?` after `write_all` and before the rename (covers the data flush), then open the parent dir and `sync_all()` (covers the directory-entry flush). On Windows, additionally call `FlushFileBuffers` via `OpenOptionsExt`. The fix costs ~1ms per save and is appropriate for a manifest that is only written at startup / on sync.
- **Verification**: Compared with the rest of the file — there is no other durability primitive. The `extractor.rs::extract_plugins` swap (`extractor.rs:436-456`) also relies on rename atomicity without fsync; same trade-off. This is a project-wide choice, but the manifest is the highest-value file to make durable.

### BUNDLED-R4-11 — `~/.aleph/cache/aleph-{skills,plugins}-checkout/` directories are never garbage-collected
- **File**: `src/bundled/extractor.rs:85-125`
- **Severity**: Low
- **Category**: resource
- **Description**: `sync_official_with_urls` clones into `cache/aleph-skills-checkout` and `cache/aleph-plugins-checkout`. On success, the directories are reused on the next call (fast path in `clone_or_update`). On failure (`Err(e)` from `clone_or_update`, or the "empty checkout" branch setting `last_err`), the function does **not** delete the checkout dir. Over time, with intermittent network failures, the cache accumulates `*.tmp` and stale `.git` directories. Compare with `hub/install.rs:170` which uses `let _ = std::fs::remove_dir_all(&checkout)` on every error path inside `install_git_skill`; `extractor.rs` lacks the equivalent cleanup. The `hub::cache::gc_git_checkouts` (referenced in the doc comment at `hub/install.rs:217-219`) handles `hub/install.rs` checkouts but not the ones in `cache/`.
- **Evidence**:
  ```rust
  // extractor.rs:85-110 (skills branch)
  match crate::bundled::clone_or_update(skills_url, &checkout) {
      Ok(()) if !checkout_has_content(&checkout) => {
          last_err = Some(format!("cloned skills checkout is empty: {}", checkout.display()));
          // no cleanup of `checkout`
      }
      Ok(()) => { ... }
      Err(e) => last_err = Some(e),  // no cleanup of `checkout`
  }
  ```
- **Suggested fix**: Add a `remove_dir_all(&checkout)` in both error branches. The next call to `clone_or_update` will re-clone from scratch, which is the right behavior for a corrupted checkout anyway. Optionally add a `gc` helper that prunes any directory under `cache/` older than N days.
- **Verification**: Searched the codebase for references to `aleph-skills-checkout` / `aleph-plugins-checkout` (`grep -rn 'aleph-skills-checkout\|aleph-plugins-checkout' src/`). The only consumers are in `extractor.rs`. The `hub::cache::gc_git_checkouts` cleanup (mentioned in the `hub/install.rs:217-219` comment) is for `.git-cache/<id>` under the skills dir, not for `cache/` under `aleph_home`.

### BUNDLED-R4-12 — `manifest.rs::SkillOrigin` has no `Marketplace` / `Plugin` variant; marketplace-installed skills are silently misclassified as `Github`
- **File**: `src/bundled/manifest.rs:39-49`
- **Severity**: Low
- **Category**: api-design / correctness
- **Description**: `SkillOrigin` only has `Official`, `Github`, `Local`. When `hub/install.rs:install_git_skill` (line 209) records an installed plugin-provided skill, it stamps it as `Github`, even though the install path went through the **marketplace** (`crate::extension::marketplace::installer::verify_plugin_integrity` is invoked before the save). The naming conflates "installed via Hub catalog" with "trust tier verified by the marketplace + sha256". The provenance is incomplete — `SkillEntry::url` is the git URL of the leaf, but the marketplace that vouched for it is not recorded. This is partly a documentation problem (the variant name says Github but the install path is marketplace-aware) and partly an observability problem (an operator looking at `manifest.json` cannot tell which installs were marketplace-vouched).
- **Evidence**:
  ```rust
  // hub/install.rs:206-215
  let mut manifest = crate::bundled::manifest::InstallRegistry::load(skills_dir)
      .unwrap_or_else(|| crate::bundled::manifest::InstallRegistry::new(""));
  manifest.skills.insert(
      safe_name.clone(),
      crate::bundled::manifest::SkillEntry {
          source: crate::bundled::manifest::SkillOrigin::Github,
          version: entry.version.clone(),
          url: Some(git_url.clone()),
          installed_at: None,
      },
  );
  ```
- **Suggested fix**: Add `SkillOrigin::Marketplace` (or split `Github` into `Marketplace` and `DirectGit` based on whether `verify_plugin_integrity` was involved). The marketplace is a distinct trust tier from a direct GitHub install; conflating them is a future-cleanup hazard rather than a current bug. If kept as `Github`, add a doc comment on `SkillOrigin::Github` clarifying the actual install paths.
- **Verification**: Read the install path (`hub/install.rs:130-219`) and the marketplace trust disclosure. Confirmed `verify_plugin_integrity` is the marketplace-vouch step. Confirmed the resulting manifest entry carries `source: Github`. The doc comment on `SkillOrigin` (manifest.rs:34-37) only mentions Hub catalog vs local; the marketplace is not acknowledged.

## Cross-cutting concerns

1. **`Save` is the most fragile public surface.** Three callers (`extractor.rs:226,254`, `hub/install.rs:215`), all writing through a fixed tmp path with no `O_NOFOLLOW`, no fsync, and no stale-tmp recovery. BUNDLED-R4-01, BUNDLED-R4-07, and BUNDLED-R4-10 all point at the same function. Refactor it into a single `atomic_write_json(path, body)` helper (with symlink defense, unique tmp name, and fsync) and route all three callers through it.

2. **`extract_bundled_content` and `sync_official_with_urls` are two sides of the same coin, but their error contracts are different.** `sync_official_with_urls` returns `Result<SyncReport, String>`; `extract_bundled_content` returns `()`. The startup path swallows every error (BUNDLED-R4-04); the RPC path surfaces most errors but masks partial successes (BUNDLED-R4-05). The two paths should agree on a single `SyncReport`-shaped error envelope.

3. **`SkillSource` classification is cached against a manifest that changes mid-process.** BUNDLED-R4-06 shows that the `OnceLock` cache in `skill/mod.rs` becomes stale after `bundled.sync`. The same staleness applies if any future code path mutates the manifest outside the daemon process (e.g., a CLI tool run by the operator). Either invalidate the cache on every `manifest.save()` or document the contract clearly: "the cache is valid for the lifetime of the daemon and is refreshed only on startup".

4. **Symlink defense is inconsistent across the file.** `prepare_plugin_temp_dir` (extractor.rs:395-419) and `manifest.rs::reconcile` (lines 158-172) use `symlink_metadata`. `extract_dir_contents` (extractor.rs:498-531) does not. `manifest.rs::save` does not. The codebase has clearly internalized the threat model in two places but missed two other places that need the same defense (BUNDLED-R4-01, BUNDLED-R4-02). A shared `safe_create_dir_all(path)` / `safe_write_tmp(path, body)` primitive would close the gap.

5. **No retry budget or escalation on persistent extraction failures.** BUNDLED-R4-02 and BUNDLED-R4-04 combine to create a silent permanent-failure loop: extract fails → mark `version: None` → don't bump bundled_version → retry on next startup → same failure → repeat. The `bundled_version` is a sticky "you have the latest" guarantee; there is no "give up after N tries" or "downgrade to Local so user can fix it manually" mechanism. The doctor (`src/diagnostics/checks/`) should ideally include a check for `SkillEntry { source: Official, version: None }` and warn the operator.

6. **The cached `InstallRegistry` reader in `skill/mod.rs` is the only consumer that bypasses the on-disk source of truth.** Every other consumer (`extractor.rs`, `hub/install.rs`) re-loads on each call. The `OnceLock` is a pure-locality optimization that has correctness implications; the optimization should be opt-in (e.g., behind a "I just ran extract and I'm sure the manifest is stable" comment) rather than baked in by default.

## Summary
- **Total: 12 findings** (0 Critical, 2 High, 5 Medium, 5 Low)
- **Top priority items (must-fix)**:
  1. **BUNDLED-R4-01** — `manifest.rs::save` writes through symlinks at the fixed `manifest.tmp` path. Symlink attack → arbitrary file write within the daemon user's authority. Add `O_NOFOLLOW` or a `symlink_metadata` pre-check.
  2. **BUNDLED-R4-02** — `extract_dir_contents` rename-replace fails on directory collisions, leaving a permanent stuck state with `version: None`. The skill is re-tried on every startup forever with no escalation.
  3. **BUNDLED-R4-06** — `CACHED_MANIFEST` in `skill/mod.rs` is never invalidated. After a `bundled.sync`, skill source classifications (Bundled vs Global) become wrong, affecting prompt-index ranking and `skill_read` collision resolution.

## What was NOT covered
- **Build-time embedding** — the `include_dir!("$CARGO_MANIFEST_DIR/skills")` and `include_dir!("$CARGO_MANIFEST_DIR/plugins")` macros and the `build.rs` that sets `ALEPH_VERSION`. These run at compile time and are out of scope for runtime review.
- **The bundled `skills/` and `plugins/` trees themselves** — these are build-time artifacts (git submodules). Their contents were not audited; only the runtime code that consumes them.
- **`include_dir::Dir::files()` / `dirs()` semantics** — assumed to match the published `include_dir` API. Did not verify the macro's path-normalization guarantees against malicious submodule names (this is upstream's contract).
- **The marketplace installer** (`crate::extension::marketplace::installer::verify_plugin_integrity`) — only its boundary is touched (`hub/install.rs:185-189`). The verifier itself is out of scope.
- **`hub::cache::gc_git_checkouts`** — referenced in `hub/install.rs:217-219` as the GC for `.git-cache/<id>`. Not read in this review; BUNDLED-R4-11 assumes it does **not** cover `cache/aleph-{skills,plugins}-checkout/` (which the code path confirms).
- **Wire-protocol conformance for the `bundled.sync` RPC** — only the error-envelope shape was checked. The JSON-RPC wrapping is delegated to `gateway::protocol`.
- **Performance benchmarks** — no `cargo bench` was run. The fixed `manifest.tmp` collision risk (BUNDLED-R4-07) and the `extract_dir_contents` per-file-write cost are flagged based on static analysis, not measured.
- **`hashify-out/GRAPH_REPORT.md` cross-reference** — not consulted; the review relied on direct reads of the four files in scope and their callers.