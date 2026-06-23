# Skills → Aleph Hub Convergence — Design

> **Status:** Approved (brainstorming). Follow-on to the MCP → Hub convergence
> (`2026-06-23-mcp-hub-convergence-design.md`). Plugins convergence is a separate
> later spec.

> **本文摘要 (zh):** 让 Aleph Hub 成为**官方 Skill** 的单一权威来源——每个内置官方
> skill 以 `aleph-hub:<skill_id>` / `Official` 出现在 Hub 目录中，正确显示「已安装」
> 状态，并可在卸载后经 Hub 引擎重装。**纯增量**：复刻 MCP 的 primer 模式，但**无需
> 退役任何引擎、无需 D9 迁移、不动 reconcile、不动 Panel**。

---

## 1. Goal

Make the Aleph Hub the single authority for **official skills**, matching what the
MCP convergence did for official MCP servers:

- Every bundled official skill appears in the Hub catalog as `aleph-hub:<skill_id>`,
  `trust_tier: Official`, browsable offline before the remote catalog is fetched.
- Each shows accurate **installed** status in the Hub browse view.
- An official skill that the user uninstalled remains in the catalog as available and
  is reinstallable through the existing Hub install engine (`run_install` → `GitDir`
  → `install_git_skill`).

Non-goal: changing how skills are authored, loaded, enabled/disabled, or how their
runtime dependencies are installed. Those stay on the existing `skills.*` surface.

## 2. Background — why this is additive, not a rewrite

The MCP convergence had to retire a **duplicate install engine** (`plan_install` vs
`run_install`). Skills have **no such duplication**:

- Skills already install through the Hub: `run_install` routes `InstallSpec::GitDir`
  to `install_git_skill()` (`src/hub/install.rs`), which clones a repo, copies the
  `<subdir>` leaf into `~/.aleph/skills/<name>`, and stamps the manifest.
- `ExtensionKind::Skill` and the polymorphic `ExtensionEntry` schema already exist.
- The `skills.*` gateway RPCs (`status` / `update` / `install_dep` / `remove`) are
  **management** operations (eligibility, official-repo sync, runtime-dep install,
  removal) with **no Hub analog** — nothing to retire.

The **only** gaps relative to "Hub is the authority for official skills":

1. **No cold-start primer.** Nothing projects the bundled official skills into the
   `aleph-hub` catalog slot, so official skills are absent from the Hub browse view
   until/unless the remote catalog supplies them.
2. **(Closed by #1.)** Installed-status and Official identity in the browse view both
   come *for free* from the primer entry + the existing `mark_installed`, so no
   reconcile change is needed (see §5).

## 3. Key facts that constrain the design

- **`skills/` is a git submodule, embedded via `include_dir!` at *compile* time**
  (`src/bundled/mod.rs`: `BUNDLED_SKILLS`). A release build (submodules initialized)
  bakes the tree into the binary, so the primer works in production. **But the
  submodule may be empty in dev/CI checkouts** → tests must NOT depend on real
  bundle contents.
- **Two distinct "source" enums exist:**
  - `domain::skill::SkillSource { Bundled, Global, Workspace, Plugin }` — *where the
    skill loads from at runtime*. Official skills extract to `~/.aleph/skills/` and
    load as `Global`/`Bundled` — **indistinguishable from a Github/Local skill by
    this field**.
  - `bundled::manifest::SkillOrigin { Official, Github, Local }` — *how installed*.
  Neither is needed by this design (see §5), but they are why a naïve
  "source-based" approach would fail.
- **The canonical slug must be the `SkillId`** = `parse_skill_content(SKILL.md).id`
  (frontmatter `name`, lowercased, spaces→hyphens, collapsed) — the same value the
  rest of the skill system uses. The bundle **directory name** may differ from the
  `SkillId`, so the directory name is carried separately as the `GitDir` `subdir`.
- **`mark_installed` matches Skill/Plugin entries by `(kind, case-insensitive name)`**
  (`src/gateway/handlers/extensions/catalog.rs:63-92`), unlike MCP which matches by a
  derived id. This is the mechanism that collapses the primer entry against the live
  installed skill — and it requires **no reconcile change**.

## 4. Decisions

- **D1 — Cold-start primer for official skills.** Add a projection of the bundled
  official skills into the `aleph-hub` catalog slot, mirroring `official_mcp.rs`.
- **D2 — Source = the bundled tree (all of it).** Scan `BUNDLED_SKILLS` at runtime
  and parse each `SKILL.md` with the existing `parse_skill_content`. Project *every*
  bundled skill — the embedded tree IS the official set; no second catalog file, zero
  drift. (Rejected: a curated `official_skills.json`; an in-code id allowlist.)
- **D3 — Canonical id = `aleph-hub:<skill_id>`.** `skill_id` from frontmatter `name`.
  The `GitDir` `subdir` carries the bundle directory name separately.
- **D4 — Unified primer.** Generalize the cold-start gate so MCP + skills fill the
  shared `aleph-hub` slot in **one** `replace_source(ALEPH_HUB_ID, …)`. A second
  per-kind primer on the same slot would clobber the first (`replace_source` is
  wholesale). The remote fetch still overwrites the whole slot later — unchanged.
- **D5 — No reconcile change.** `reconcile.rs` / `skill_to_entry` stays byte-identical
  (mirrors MCP's "reconcile 字节不动"). Installed-marking and Official identity in the
  browse view come from the primer entry + the existing name-based `mark_installed`.
- **D6 — No D9 migration.** Pre-Hub official skills need no delete-and-re-fetch: there
  is nothing that "can't be handled" (no vault keys bound to ids, unlike MCP). The
  browse view simply gains an `aleph-hub:<skill_id>` entry marked installed; the live
  `extensions.installed` view continues to show `local:skill:<id>` exactly as today.
- **D7 — No RPC retirement, no Panel changes.** The `skills.*` RPCs are
  non-overlapping management ops. The Panel's Extensions Hub view already renders the
  `skill` kind (chips/badges/browse); `Settings▸Skills` is a management page with no
  discover/recommend block (no D6-of-MCP analog to remove).
- **D8 — Reinstall via Hub engine.** Primer entries carry
  `install_spec = GitDir { git_url: OFFICIAL_SKILLS_REPO, subdir: Some(<dir_name>) }`
  so an uninstalled official skill reinstalls through `run_install` →
  `install_git_skill` (which re-stamps the manifest `Github`, so official-sync won't
  fight an explicit reinstall — acceptable).

## 5. Architecture & data flow

### 5.1 New file: `src/hub/official_skills.rs`

Mirrors `official_mcp.rs`. Three items, structured so tests never touch the real
submodule:

```rust
/// Pure projection of one bundled skill into a Hub catalog entry. Testable on a
/// synthetic SkillManifest — independent of whether the submodule is present.
fn project_skill(dir_name: &str, manifest: &SkillManifest) -> ExtensionEntry;

/// Scan BUNDLED_SKILLS, parse each SKILL.md, project each. Returns [] (logged)
/// when the submodule is absent. Skips entries whose SKILL.md fails to parse.
pub fn primer_entries() -> Vec<ExtensionEntry>;
```

`project_skill` produces:

| field | value |
|-------|-------|
| `id` | `format!("{ALEPH_HUB_ID}:{}", manifest.id())` |
| `kind` | `ExtensionKind::Skill` |
| `category` | `ExtensionCategory::Other` |
| `name` | `manifest.name()` |
| `description` | `manifest.description()` |
| `trust_tier` | `TrustTier::Official` |
| `source_id` / `via` | `ALEPH_HUB_ID` |
| `requires_config` | `spec.requires_config()` (GitDir ⇒ `false`) |
| `installed` / `enabled` / `update_available` | `false` (flipped by `mark_installed`) |
| `install_spec` | `Some(GitDir { git_url: OFFICIAL_SKILLS_REPO.into(), subdir: Some(dir_name.into()), git_ref: None, sha256: None })` |

`primer_entries` iterates `BUNDLED_SKILLS.dirs()`; for each dir with a `SKILL.md`,
reads the file's contents (via `include_dir` `get_file`) and calls
`parse_skill_content(contents, SkillSource::Bundled)`, then `project_skill(dir_name,
&manifest)`. Parse failures are skipped with a `warn!` (mirrors MCP's
`is_projectable` filter).

### 5.2 Unified primer — new `src/hub/primer.rs`

The cold-start gate is kind-agnostic, so it moves OUT of the MCP-specific
`official_mcp.rs` into a small focused module that composes both projections:

```rust
// src/hub/primer.rs  (new)
pub async fn prime_official_catalog_if_empty(cache: &CatalogCache) {
    if matches!(cache.count_source(ALEPH_HUB_ID).await, Ok(0)) {
        let mut entries = crate::hub::official_mcp::primer_entries();      // MCP
        entries.extend(crate::hub::official_skills::primer_entries());     // Skills
        match cache.replace_source(ALEPH_HUB_ID, &entries).await { .. }
    }
}
```

Concretely:

- **Move** the cold-start gate `prime_official_mcp_if_empty` and its test
  `primes_when_empty_then_is_noop_when_populated` out of `official_mcp.rs` into
  `primer.rs`, renamed `prime_official_catalog_if_empty`.
- **Keep** in `official_mcp.rs`: `primer_entries()` (MCP projection) + the legacy
  migration fns + their tests — `official_mcp.rs` stays the MCP-projection module.
- Update the single boot call site `src/bin/aleph-server/commands/start/mod.rs:822`
  from `official_mcp::prime_official_mcp_if_empty(&cache)` to
  `primer::prime_official_catalog_if_empty(&cache)`. Exactly **one** boot call remains.

### 5.3 Data flow

**Cold start (boot, slot empty):** `prime_official_catalog_if_empty` writes MCP +
skill entries into the `aleph-hub` slot in one shot. Browse view (`extensions.catalog`)
returns them offline.

**Remote fetch (later):** overwrites the whole `aleph-hub` slot with remote entries —
unchanged behavior; primer is purely a cold-start seed.

**Installed-marking (every `extensions.catalog` call):** `collect_installed()` gathers
live skills as `local:skill:<id>` (name = frontmatter name) via the **unchanged**
`skill_to_entry`. `mark_installed` matches each catalog entry of `kind=Skill` against
the live set by `(kind, lowercased name)` → flips `installed`/`enabled` on the
`aleph-hub:<skill_id>` primer entry. Result: browse shows Official + installed.

**Reinstall after uninstall:** the `aleph-hub:<skill_id>` entry stays in the catalog
(primer/remote) as available. Install → `extensions.install` → `run_install` →
`GitDir` → `install_git_skill` clones `OFFICIAL_SKILLS_REPO`, copies `subdir` leaf →
the skill reappears on disk → next catalog call marks it installed again.

## 6. Cross-repo id/name contract (§5 analog — user-owned, outside this repo)

When the remote `hub.heyaleph.com` catalog begins shipping official skills, each entry
must carry:

- `id = aleph-hub:<skill_id>` (frontmatter-name slug) — for stable browse identity and
  to coincide with the primer's id; **and**
- `name` equal to the bundled skill's frontmatter `name` — because skill
  installed-marking is **name-based** (`mark_installed`), the name (not the id) is what
  flips the installed flag. A mismatched name splits installed-status.

This is subtly stricter than the MCP contract (which is purely id-based).

## 7. Risks & edge cases

- **Submodule empty at build** → `primer_entries()` returns `[]`; official skills
  appear only in `extensions.installed` as `local:skill:<id>` (today's behavior).
  Graceful degradation; no failure.
- **id/name collision** — a user-authored skill whose frontmatter `name` equals an
  official skill's name would, via name-based `mark_installed`, flip the official
  primer entry to installed. Same risk class as MCP's slug collision and as the
  existing plugin name-matching; skill names are unique within the loaded set
  (shared `~/.aleph/skills/` dir namespace). Accepted; documented.
- **`SKILL.md` parse failure** in the bundle → that skill is skipped (logged); the
  rest still project.
- **`subdir` ≠ repo layout** — the primer assumes the bundle dir name equals the
  `Aleph-skills` repo subdir (true because the bundle is the repo via submodule). If
  the repo is ever restructured, reinstall (not cold-start browse) would fail with
  `subdir '<x>' not found` — surfaced by `install_git_skill`'s existing check.

## 8. Testing strategy (submodule-independent)

All unit tests use synthetic input, never the real bundle:

1. **`project_skill`** on a `SkillManifest` parsed from a sample `SKILL.md` string:
   asserts `id == "aleph-hub:<skill_id>"`, `kind == Skill`, `trust_tier == Official`,
   `source_id == aleph-hub`, and `install_spec == GitDir { git_url: OFFICIAL_SKILLS_REPO,
   subdir: Some(<dir_name>), .. }`. Include a case where `dir_name != skill_id` to lock
   the decoupling.
2. **`primer_entries` tolerates an empty bundle** — returns `[]` without panicking
   (the realistic CI state). No assertion on specific skills.
3. **Unified primer** (`prime_official_catalog_if_empty`) on an in-memory
   `CatalogCache`, asserted through `count_source`/`query` (the seam the existing MCP
   test already uses): after the first call the slot is non-empty and the second call
   is a no-op (count unchanged). Crucially, **`query(kind=Mcp)` still returns the full
   MCP primer set** — proving the skills extension never clobbers MCP. This holds even
   when the skills submodule is absent (MCP's `catalog.json` is always present, so the
   MCP count is a stable, non-zero anchor) — the test needs no real skill bundle.
4. **`mark_installed` for skills by name** — already covered by the existing
   `plugin_entry_marked_installed_by_name_case_insensitive` and
   `name_match_does_not_cross_kinds` tests; add a `Skill`-kind variant asserting an
   `aleph-hub:<skill_id>` catalog entry is marked installed by a live
   `local:skill:<id>` of the same name, and is NOT marked when names differ.

## 9. File summary

| Action | File | Purpose |
|--------|------|---------|
| Create | `src/hub/official_skills.rs` | `project_skill` + `primer_entries` |
| Create | `src/hub/primer.rs` | unified `prime_official_catalog_if_empty` (gate moved here) |
| Modify | `src/hub/official_mcp.rs` | remove the cold-start gate + its test (moved to `primer.rs`); keep MCP projection + legacy migration |
| Modify | `src/hub/mod.rs` | register `official_skills` + `primer` modules |
| Modify | `src/bin/aleph-server/commands/start/mod.rs:822` | call `primer::prime_official_catalog_if_empty` |
| Modify | `src/gateway/handlers/extensions/catalog.rs` (tests only) | add Skill-kind `mark_installed` test |

Unchanged: `src/hub/reconcile.rs`, `src/hub/install.rs`, all `skills.*` handlers, the
Panel.

## 10. Out of scope / follow-ons

- **Plugins → Hub convergence** — same primer pattern, but plugins have no
  `manifest.json` and use a marketplace-scoped cache; separate spec.
- Surfacing `trust_tier: Official` in the live `extensions.installed` view (would
  require a reconcile change and diverge from mcp/plugins) — intentionally not done.
