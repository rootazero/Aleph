# Monorepo Skills & Plugins Consolidation

**Date:** 2026-03-27
**Status:** Approved

## Summary

Migrate official skills (`rootazero/Aleph-skills`) and plugins (`rootazero/Aleph-plugins`) from separate GitHub repositories into the main Aleph monorepo. Skills and plugins are embedded in the binary at compile time via `include_dir`, extracted on first launch or version upgrade. User installation/update experience remains unchanged.

## Decisions

| Decision | Choice |
|----------|--------|
| Repo structure | Top-level `skills/` + `plugins/` |
| Skills install target | Unified `~/.aleph/skills/` (no separate `skills-official/`) |
| Source tracking | `~/.aleph/skills/manifest.json` with reconcile on startup |
| Plugins structure | Retain marketplace cache → installed two-level |
| Distribution | Compile-time embed via `include_dir!` |
| Update channel | Version-bound only (no hot update) |

## Repository Structure

```
Aleph/
├── skills/                    # Migrated from Aleph-skills
│   ├── web-search/
│   │   └── SKILL.md
│   ├── code-review/
│   │   └── SKILL.md
│   └── ...
├── plugins/                   # Migrated from Aleph-plugins
│   ├── marketplace.toml       # Marketplace index (existing format)
│   ├── plugin-a/
│   │   └── plugin.json
│   └── ...
├── core/
├── interfaces/
└── ...
```

## Compile-Time Embedding

New module `src/bundled.rs`:

```rust
use include_dir::{include_dir, Dir};

pub static BUNDLED_SKILLS: Dir = include_dir!("$CARGO_MANIFEST_DIR/../skills");
pub static BUNDLED_PLUGINS: Dir = include_dir!("$CARGO_MANIFEST_DIR/../plugins");
pub const BUNDLED_VERSION: &str = env!("ALEPH_VERSION");
```

- `include_dir` embeds entire directory trees as bytes in the binary
- `BUNDLED_VERSION` reuses existing VERSION file mechanism
- Estimated binary size increase: 200-500KB (text-heavy content)
- Build-time note: changes to `skills/` or `plugins/` files trigger recompilation of the embedding crate; this is acceptable given these files change infrequently

## Startup Extraction Logic

New module `src/bundled/extractor.rs`:

```
Startup flow:
1. Read ~/.aleph/skills/manifest.json → get bundled_version
2. If manifest missing → reconcile-before-extract (see below)
3. Compare bundled_version with BUNDLED_VERSION
4. If different → extract
5. Update manifest (bundled_version + skill entries) only after ALL extractions succeed
6. Run reconcile to sync manifest with directory state
```

### Reconcile-Before-Extract (First Upgrade)

On first upgrade from old version, manifest does not exist but `~/.aleph/skills/` may contain user-installed skills:

1. Scan `~/.aleph/skills/` directories
2. Populate manifest with `"source": "local"` for every existing skill
3. Then proceed to extraction (which skips `"local"` entries)

This protects pre-existing user content from being overwritten.

### Skills Extraction

- Iterate all skill directories in `BUNDLED_SKILLS`
- Per skill, check manifest source:
  - `"official"` or absent → overwrite to `~/.aleph/skills/<name>/`
  - `"local"` or `"github:*"` → **skip** (preserve user content)
- Update manifest entries with `"source": "official"`

### Error Handling

- Extraction failures (disk full, permission errors) are logged as warnings (non-fatal, consistent with current `updater.rs` behavior)
- `bundled_version` in manifest is updated **only after all official skills are successfully extracted**
- On partial failure, next startup will retry extraction since version still mismatches

### Plugins Extraction

- Overwrite entire `~/.aleph/plugins/cache/aleph-official/` directory
- This is marketplace cache only — does not affect installed plugins
- `marketplace.toml` updated alongside

### Manifest Structure

```json
{
  "bundled_version": "0.3.1",
  "skills": {
    "web-search": { "source": "official", "version": "0.3.1" },
    "my-tool": { "source": "github:user/repo", "installed_at": "2026-03-27" },
    "quick-hack": { "source": "local" }
  }
}
```

### Startup Reconcile

- Scan `~/.aleph/skills/` directories
- Directory exists but not in manifest → add as `"source": "local"`
- In manifest but directory missing → remove from manifest

## Code Changes

### Delete

- `src/skills/updater.rs` — git clone/pull logic for official skills (entire file)

### Create

- `src/bundled/mod.rs` — embedded static directories
- `src/bundled/extractor.rs` — extraction, version comparison, reconcile
- `src/skills/manifest.rs` — manifest.json read/write/reconcile

### Modify

- `src/bin/aleph-server/commands/start/mod.rs` — call extractor instead of updater
- `src/utils/paths.rs` — remove `skills-official` search directory (lines 278-282)
- `src/skill/mod.rs` — `guess_source()` should consult manifest instead of path heuristic (`path_str.contains("skills-official")`)
- `src/skills/registry.rs` — `SkillEcosystem::Official` classification should use manifest instead of `dir_str.contains("skills-official")`
- `src/builtin_tools/self_manage.rs` — remove hard-coded `aleph_home.join("skills-official")` path
- `src/extension/mod.rs` — remove `skills-official` path reference
- `src/extension/marketplace/types.rs` — `BUILTIN_MARKETPLACE_SOURCE` points to local bundled cache instead of GitHub repo
- `src/extension/marketplace/github_source.rs` — **keep entirely** (used by third-party marketplaces); only the builtin source constant in `types.rs` changes
- `src/skills/installer.rs` — write manifest entry on third-party install
- CLI `skill delete` command — sync manifest on deletion

### Unchanged

- `SKILL.md` format
- `plugin.json` / `marketplace.toml` format
- User CLI commands (`aleph plugin install/uninstall`, `aleph skill install/delete`)
- Third-party marketplace git mechanism

## Migration & Compatibility

### First Upgrade

- Users upgrading from old version will have `~/.aleph/skills-official/` present
- Extractor detects missing manifest → runs reconcile-before-extract to protect existing user skills, then extracts bundled content
- `skills-official/` directory: auto-removed after successful first extraction (all path references already removed from code)

### Discovery Priority

- Before: `skills-official/` → `~/.aleph/skills/` → project `.claude/skills/`
- After: `~/.aleph/skills/` (mixed official + user, manifest distinguishes) → project `.claude/skills/`
- Same-name skill: user's `"local"` overrides official (manifest tracks the override)

### Old Repositories

- `rootazero/Aleph-skills` and `rootazero/Aleph-plugins` → archived
- README updated to point to main repository
