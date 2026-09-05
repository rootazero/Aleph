# src/skill review (raw agent output)

## Summary
- Files scanned: src/skill/{mod.rs, compat.rs, config.rs, cooccurrence.rs, eligibility.rs, guard.rs, installer.rs, manifest.rs, preprocess.rs, prompt.rs, registry.rs, shared.rs, snapshot.rs, status.rs, usage.rs}
- Critical: 2, Important: 4, Minor: 4
- Health: orange

## Critical findings

### C-1 `remove_skill` leaves the deleted skill's entries in `.cooccur.json` (orphan-signal leak + incorrect workflow proposals)
- File: src/skill/mod.rs:542-577 (paired with cooccurrence.rs:53-134)
- Problem: `remove_skill` calls `UsageStore::new(dir).forget(id.as_str())` for every registered dir (mod.rs:573-575), but never calls any counterpart on `CoOccurrenceLog`. `CoOccurrenceLog` exposes no `forget` / `prune` API at all — `grep "pub fn" src/skill/cooccurrence.rs` returns only `new`, `record`, `snapshot`, `cluster_chains`. The skill module's own design comment (`usage.rs:267-275`) says "a use is **two** writes — `.usage.json` and `.cooccur.json` — and they have to happen together or the second signal silently describes only part of the activity"; the deletion path violates that invariant.
- Why it matters: `cluster_chains` (workflow_proposal.rs:106) treats every entry as a real recent use, so a deleted skill keeps reappearing in chains for up to `MAX_ENTRIES = 512` records. The dream pipeline then proposes workflows that include a skill the user already deleted.
- Suggested fix: Add `CoOccurrenceLog::forget(&self, skill)` (lock → load → `retain(|e| e.skill != skill)` → save) and call it inside the same `for dir in &dirs` loop in `remove_skill` next to `UsageStore::forget`.

### C-2 `serde_yaml` 0.9.34+deprecated is the deserializer for untrusted skill frontmatter (yaml-rust CVE class)
- File: src/skill/manifest.rs:282 (`let raw: RawFrontmatter = serde_yaml::from_str(&yaml_str)?`), src/skill/preprocess.rs:123
- Problem: `Cargo.lock` shows `serde_yaml 0.9.34+deprecated` — the crate's own `+deprecated` suffix flags an unmaintained `yaml-rust` backend with multiple known issues.
- Why it matters: Skill frontmatter is read from `~/.aleph/skills`, `~/.claude/skills`, and per-plugin `skills/` directories — every Community-trust path is attacker-controllable once the directory exists.
- Suggested fix: Either (a) switch to `serde_yml` (the maintained `serde_yaml` successor that's API-compatible), or (b) gate `serde_yaml::from_str` behind an explicit `serde_yaml::Deserializer::from_str` with a manual recursion-limit.

## Important findings

### I-1 Companion files in a skill directory are never scanned unless installed via the RPC path
- File: src/skill/mod.rs:719-768 (`scan_directory`), src/skill/manifest.rs:212-260 (`parse_skill_file`)
- Problem: `scan_directory` reads only files whose basename matches `SKILL.md`. Companion files (`scripts/setup.sh`, `references/example.py`) are never scanned by the rescan/rescan_dirs path used by `extension/projection.rs:139`.
- Suggested fix: In `parse_skill_file`, after the SKILL.md scan, walk the parent directory's non-hidden files and call `scan_content` on each.

### I-2 `with_file_lock` has no timeout — a crashed peer holding the lock blocks `.usage.json` and `.cooccur.json` updates forever
- File: src/utils/atomic_io.rs:53-72 (used by usage.rs:225, 312 and cooccurrence.rs:84)
- Problem: `with_file_lock` does `file.lock_exclusive()` with no `try_lock` fallback. If a peer crashes mid-closure, the lock can survive on Linux until the kernel reclaims the file handle.
- Suggested fix: Wrap `lock_exclusive` in `tokio::time::timeout(Duration::from_secs(5), ...)` and degrade to a warn + best-effort on timeout.

### I-3 `preprocess.rs` runs user-controlled shell snippets with `sh -c <cmd>` when `allow-inline-shell: true`
- File: src/skill/preprocess.rs:202-249 (`run_snippet`), 176-200 (`expand_inline_shell`)
- Problem: When a skill's YAML frontmatter contains `allow-inline-shell: true`, the skill body is executed via `Command::new("sh").arg("-c").arg(cmd)`. The snippet's stdout is spliced into the skill body AFTER the scan, bypassing the guard.
- Suggested fix: Drop inline-shell entirely or run snippets under a stricter sandbox AND require the guard verdict on the EXPANDED body (re-scan after splicing).

### I-4 `compat::SkillInfo` is a strict lossy projection — downstream callers cannot reconstruct eligibility / scope / version
- File: src/skill/compat.rs:10-30
- Problem: `SkillInfo { id, name, description }` strips `scope`, `eligibility`, `install_specs`, `bound_tool`, `primary_env`, `when_to_use`, and the new `<version>` tag.
- Suggested fix: Add a fuller `SkillInfo { id, name, description, scope, version }` field set, or gate the legacy view behind a feature flag.

## Minor findings
### M-1 `String::from_utf8_lossy` in `scan_content` can mask YAML/control-char payloads
- File: src/skill/guard.rs:142-148
- Note: Lossy conversion replaces invalid UTF-8 bytes with U+FFFD.

### M-2 `scan_skill_directory` reads symlink targets but `parse_skill_file` does not check `is_symlink`
- File: src/skill/guard.rs:226-265, src/skill/manifest.rs:212

### M-3 `mod.rs::scan_directory` recurses into subdirs, silently multiplying registration under large subtrees
- File: src/skill/mod.rs:751-755

### M-4 `cluster_chains` `at_ms` overflow when `as_millis()` saturates
- File: src/skill/cooccurrence.rs:185-189
