# Severed-Wire Audit — `src/bundled/` (2026-08-17)

## Scope

| File | LoC (review) | Production surface |
|------|--------------|--------------------|
| `src/bundled/mod.rs` | ~40 | Re-exports + 5 module-level constants |
| `src/bundled/manifest.rs` | ~290 | `InstallRegistry`, `SkillEntry`, `SkillOrigin` schema |
| `src/bundled/extractor.rs` | ~770 | Extraction, atomic-rename, symlink-safe staging, `extract_bundled_content`, `sync_official_now` |
| `src/bundled/sync.rs` | ~220 | git2 clone/fetch/pin/reset |

## Method

PRODUCED − CONSUMED symbol parity via `rg` across `src/`, `src/bin/`, `interfaces/`, `shared/`.

Read before write — every "no consumer" claim was re-checked with `rg` against the candidate consumer side (e.g., for `SkillEntry::version` I confirmed no `.version` reader anywhere in `src/`). Read before CUT — every symbol reported as CUT was re-grepped across the whole repo for any reference.

## Cross-checked call sites (live wires — sanity sweep)

| Producer | Consumer | Status |
|----------|----------|--------|
| `extract_bundled_content` | `src/bin/aleph-server/commands/start/helpers.rs:384` | live |
| `sync_official_now` → `bundled.sync` RPC | `src/gateway/handlers/bundled_sync.rs:49` | live |
| `SyncKind` (all 3 variants) | `src/gateway/handlers/bundled_sync.rs:21-25,68-70` | live |
| `SyncReport` (used via return value) | `src/gateway/handlers/bundled_sync.rs:53-56` | live |
| `BUNDLED_SKILLS` | `src/hub/official_skills.rs:8,52` + `src/gateway/execution_engine/btw_wire_tests.rs:2063,2225,2265` (test) | live |
| `BUNDLED_PLUGINS` | `src/hub/official_plugins.rs:9,70` + test | live |
| `BUNDLED_VERSION` | `src/bundled/extractor.rs:8,105,189,203,204,217,237,239,299,654` | live |
| `OFFICIAL_SKILLS_REPO` | `src/hub/official_skills.rs:8,20,36,109` + `src/bundled/extractor.rs:42` | live |
| `OFFICIAL_PLUGINS_REPO` | `src/hub/official_plugins.rs:9,38,54,127` + `src/bundled/extractor.rs:43` | live |
| `InstallRegistry::load` | `src/hub/install.rs:248,674` + `src/skill/mod.rs:817` + `src/bundled/extractor.rs:97,155,185` | live |
| `InstallRegistry::save` | `src/bundled/extractor.rs:106,191,209,250` + `src/hub/install.rs:263` (silent) | live |
| `InstallRegistry::new` | `src/hub/install.rs:249` + `src/bundled/extractor.rs:97,160` | live |
| `InstallRegistry::reconcile` | `src/bundled/extractor.rs:98,102,190,206,225,247` | live |
| `InstallRegistry::is_official` | `src/skill/mod.rs:822` | live |
| `InstallRegistry::skills` field | `src/bundled/extractor.rs:170,284,295,315,641,650,661` + `src/hub/install.rs:250,676` + `src/skill/mod.rs:816` | live |
| `InstallRegistry::bundled_version` field | `src/bundled/extractor.rs:105,170,189,203,216,237` | live |
| `SkillOrigin::Official` (compared / matched) | `src/bundled/extractor.rs:285,298,318,642,653,664` + `src/bundled/manifest.rs:245` | live |
| `SkillEntry::source` field | `src/bundled/extractor.rs:285,642` + `src/skill/mod.rs:822` (transitively) + `src/hub/install.rs:676` | live |
| `clone_or_update` (pub(crate)) | `src/bundled/extractor.rs:85,115` | live |
| `clone_or_update_at` (pub(crate)) | `src/hub/install.rs:191` + `src/bundled/extractor.rs` (tests) | live |
| `copy_skill_leaf` (pub(crate)) | `src/hub/install.rs:219,225,230` | live |

Every pub / pub(crate) item in `src/bundled/` is consumed in production (not just tests). The remaining findings are about **fields and variants** that round-trip through serde but have no behavioral consumer.

## Findings (4 total)

| # | ID | File | Severity | Form | Decision |
|---|----|------|----------|------|----------|
| 1 | sw-bundled-01 | `manifest.rs:54` | low | form 3 (inert config) + form 6 (orphan pub API) | **DECIDE** |
| 2 | sw-bundled-02 | `manifest.rs:48` | low | form 3 + form 6 | **DECIDE** |
| 3 | sw-bundled-03 | `manifest.rs:39-49` (variant) | low | form 5 (name-drift / dead variant) + form 6 | **DECIDE** |
| 4 | sw-bundled-04 | `mod.rs:40` (re-export) | low | form 5 (name collision) | **DECIDE** |

All findings are low-severity because every one of these symbols survives a serde round-trip and removing them would alter the on-disk `manifest.json` shape for any operator who has a Github-installed skill stamped in their manifest. The wire is severed at the *behavioral* layer (no Rust code reads the field) but alive at the *wire-format* layer (the field is serialized and deserialized). Pragmatic call: keep the fields for now, but flag that they are inert so the next manifest-surgery pass can decide whether to wire them up or remove them.

---

## Finding 1 — `SkillEntry.installed_at` is never set to `Some` and never read

- **Severity:** low
- **Form:** 3 (inert config) + 6 (orphan pub API surface)
- **File:** `src/bundled/manifest.rs:53-54`
- **Decision:** **DECIDE** — see options below
- **Existing review ref:** none — round-r4 audit (`review-results/sev-wire-2026-08-19-r4/bundled/REPORT.md`) did not flag this field

### Evidence

Field declaration:
```rust
// src/bundled/manifest.rs:53-54
/// ISO date when installed (for non-official skills).
#[serde(skip_serializing_if = "Option::is_none")]
pub installed_at: Option<String>,
```

`installed_at` is set to `None` at every write site:

```
$ rg -n "installed_at:" src/bundled/manifest.rs src/bundled/extractor.rs src/hub/install.rs
src/bundled/manifest.rs:229:                        installed_at: None,
src/bundled/manifest.rs:258:            installed_at: None,
src/bundled/extractor.rs:301:                        installed_at: None,
src/bundled/extractor.rs:321:                        installed_at: None,
src/bundled/extractor.rs:656:                        installed_at: None,
src/bundled/extractor.rs:667:                        installed_at: None,
src/bundled/extractor.rs:854:                installed_at: None,
src/hub/install.rs:256:            installed_at: None,
```

No `installed_at:` ever takes the `Some(...)` form. Combined with `#[serde(skip_serializing_if = "Option::is_none")]`, this means the field is **never serialized to `manifest.json` and never read from it either**.

Consumer-side read search:

```
$ rg -n "\.installed_at" src/
src/hub/origin.rs:156:            local_ref=excluded.installed_at",
src/hub/origin.rs:165:            o.installed_at,
```
Both hits are on a **different struct** (`hub::origin::OriginEntry`), not on `bundled::manifest::SkillEntry`.

### Proposed change

**Option A — CUT (recommended):** Remove the field entirely from `SkillEntry`, drop the doc comment, drop the `installed_at: None` initializers at all 8 sites. The on-disk format is unchanged (the field never reached disk today).

**Option B — CONNECT:** If the intent was to record "when was this skill installed", wire it up in `hub/install.rs` (stamp `Utc::now().to_rfc3339()` into `Some(...)` when a Github install happens, plus a reconcile-time backfill). Then add a doctor check that surfaces skills installed >N days ago.

**Option C — DECIDE / leave as-is:** If the intent was "preserve for a future feature", leave the field and add a `// FIXME: wire up or remove — see sw-bundled-01` comment. This is what the current state amounts to.

### Risk

Low. The field is inert — removing it cannot change runtime behavior because it is `None` everywhere. The only observable difference is that operators reading `manifest.json` no longer see the field (which they cannot, because `skip_serializing_if` already excludes it). Manual edits to `manifest.json` that set `"installed_at": "..."` would silently drop on the next `save()`; this is unlikely to be relied on.

### Verification

After CUT:
- `rg -n "installed_at" src/bundled/` returns zero hits.
- `rg -n "SkillEntry \{|installed_at:" src/` returns zero hits.
- `cargo check -p alephcore` compiles.
- An existing `manifest.json` with `"installed_at"` parses fine (serde ignores unknown fields by default) and a subsequent save drops it.

---

## Finding 2 — `SkillEntry.version` is set on producer side but never read by behavioral code

- **Severity:** low
- **Form:** 6 (orphan pub API surface — round-trips through serde but has no reader)
- **File:** `src/bundled/manifest.rs:47-48`
- **Decision:** **DECIDE** — see options below
- **Existing review ref:** none

### Evidence

Field declaration:
```rust
// src/bundled/manifest.rs:47-48
/// Version when installed (for official skills, matches `bundled_version`).
#[serde(skip_serializing_if = "Option::is_none")]
pub version: Option<String>,
```

Producer-side writes:
```
$ rg -n "version: Some|version: None" src/bundled/manifest.rs src/bundled/extractor.rs src/hub/install.rs
src/bundled/manifest.rs:227:                        version: None,
src/bundled/manifest.rs:256:            version: Some(version.to_string()),
src/bundled/extractor.rs:299:                        version: Some(BUNDLED_VERSION.to_string()),
src/bundled/extractor.rs:319:                        version: None,
src/bundled/extractor.rs:654:                        version: Some(BUNDLED_VERSION.to_string()),
src/bundled/extractor.rs:665:                        version: None,
src/bundled/extractor.rs:852:                version: None,
src/hub/install.rs:254:            version: entry.version.clone(),  // Some(...) or None depending on catalog entry
```

So `version` IS set to `Some(BUNDLED_VERSION)` for Official skills and `Some(entry.version)` for Github-installed skills, and `None` on failed extractions. The field DOES reach `manifest.json`.

Consumer-side read search (anywhere outside `bundled/` itself):
```
$ rg -n "entry\.version|e\.version|skill\.version" src/
src/hub/trust.rs:114:        version: entry.version.clone(),
src/hub/install.rs:254:            version: entry.version.clone(),
src/hub/official_plugins.rs:52:        version: entry.version.clone(),
src/hub/origin.rs:121:    if let (Some(installed), Some(offered)) = (&origin.version, &entry.version) {
src/hub/origin.rs:74:            version: o.version.clone(),
```
Every hit is a `MarketplacePluginEntry.version` (catalog entry from the Hub), **not** a `SkillEntry.version` from the install registry. The `bundled::manifest::SkillEntry.version` field is never read by any behavioral code outside the bundled module itself.

### Proposed change

**Option A — DECIDE / leave:** The doc comment claims intent ("for official skills, matches `bundled_version`"), so the field is the foundation for a not-yet-implemented feature (e.g., "did this Official skill change since the last startup?"). The codebase already compares `manifest.bundled_version == BUNDLED_VERSION` (`extractor.rs:203`) and bails out of extraction — this is exactly what the per-skill `version` field was likely intended to enable. But that code path doesn't exist. Leaving the field is correct if the feature is on the roadmap; remove it if not.

**Option B — CONNECT (cheap):** Use `entry.version == Some(BUNDLED_VERSION)` in `extract_skills` as a quick-exit ("already extracted this exact bundled version, skip the copy"). This avoids the per-file copy + prune on every startup after the first. ~20-line change.

**Option C — CUT:** Remove the field if the upgrade-detection feature is deemed not worth the complexity. ~8 write sites to drop.

### Risk

Low. The field is alive in the wire format but inert at the behavioral layer. Removing it does not break runtime behavior; it only changes the on-disk shape of `manifest.json` (operators with existing manifests will see the `"version"` key silently dropped on the next save, which is the same drop as the `skip_serializing_if` already does for `None`).

### Verification

After CUT:
- `rg -n "\.version" src/bundled/` returns only the producer-side construction calls.
- `rg -n "skill.version|SkillEntry.version" src/` returns zero hits.

After CONNECT:
- `extract_skills` gains a `if entry.version == Some(BUNDLED_VERSION) { skip }` early-exit (file-existence check still required, since the dir might have been manually wiped).
- Add a test that exercises the early-exit path.

---

## Finding 3 — `SkillOrigin::Github` is set but never matched as a specific case

- **Severity:** low
- **Form:** 5 (name-drift: variant has a name but no behavioral distinction) + 6 (orphan variant — only used as a serde round-trip marker)
- **File:** `src/bundled/manifest.rs:39-49` (enum definition), `src/hub/install.rs:253,677` (write sites)
- **Decision:** **DECIDE**
- **Existing review ref:** round-r4 BUNDLED-R4-12 noted the same conflation but framed it as "api-design / correctness" — this audit confirms the runtime layer truly never distinguishes `Github` from `Local`.

### Evidence

`SkillOrigin` has three variants:
```rust
// src/bundled/manifest.rs:39-49
pub enum SkillOrigin {
    /// Bundled with the binary, extracted on startup.
    Official,
    /// Installed from a GitHub URL.
    Github,
    /// Manually placed in the skills directory.
    Local,
}
```

Write sites:
```
$ rg -n "SkillOrigin::Github" src/
src/hub/install.rs:253:            source: crate::bundled::manifest::SkillOrigin::Github,
src/hub/install.rs:677:            crate::bundled::manifest::SkillOrigin::Github
```

Read / match sites (anywhere outside the bundled module's own internal helpers):
```
$ rg -n "SkillOrigin::Official|SkillOrigin::Local" src/
src/bundled/extractor.rs:285:            if entry.source != SkillOrigin::Official {
src/bundled/extractor.rs:298:                        source: SkillOrigin::Official,
src/bundled/extractor.rs:318:                        source: SkillOrigin::Official,
src/bundled/extractor.rs:642:            if e.source != SkillOrigin::Official {
src/bundled/extractor.rs:653:                        source: SkillOrigin::Official,
src/bundled/extractor.rs:664:                        source: SkillOrigin::Official,
src/bundled/manifest.rs:226:                        source: SkillOrigin::Local,
src/bundled/manifest.rs:245:            .is_some_and(|e| e.source == SkillOrigin::Official)
src/bundled/manifest.rs:255:            source: SkillOrigin::Official,
src/bundled/extractor.rs:851:                source: SkillOrigin::Local,
src/bundled/extractor.rs:867:            SkillOrigin::Official
src/bundled/extractor.rs:871:            SkillOrigin::Local
src/hub/install.rs:676:            crate::bundled::manifest::SkillOrigin::Github
```
The `hub/install.rs:676` hit is inside the test `install_git_skill_clones_subdir_and_stamps_source` which asserts `.source == Github` — i.e., it tests the round-trip, not a behavioral branch on `Github`.

**Behavioral matches on `Github`: zero.** Every comparison on `SkillOrigin` either matches `Official` or constructs `Local` / `Official`. The runtime layer treats `Github` and `Local` as identical ("not Official, so user-owned, so don't overwrite on official sync").

This means `Github` survives only as a serde round-trip label. An operator reading `manifest.json` sees `"source": "github"` vs `"source": "local"` and can distinguish a Hub-installed skill from a manually-placed one — but no Rust code branches on this distinction.

### Proposed change

**Option A — DECIDE / leave:** The on-disk distinction has documentary value. An operator triaging an install can see at a glance "this came from a Hub catalog install" vs "someone dropped this in manually". Keeping the variant costs nothing at runtime.

**Option B — CUT:** Remove the variant. Existing manifests with `"source": "github"` will fail to deserialize (serde will error on the unknown variant). To avoid breaking upgrades, either bump the manifest schema version and add a migration, OR rename `Github` → `Local` in old manifests on load (`serde(alias = "github")` on `Local` works). The variant's behavioral absence means the rename is invisible to the rest of the system.

**Option C — CONNECT:** Give the variant a behavioral meaning. For example, `skill_manage` could refuse to "remove" a `Github`-sourced skill without an explicit "I know this was installed via Hub" confirmation, or `bundled.sync` could refuse to overwrite `Github`-sourced skills with a clearer error than the current "user skill wins" message.

### Risk

Low for option A, low for option B (with migration), medium for option C (changes user-facing behavior).

### Verification

After CUT (with alias):
- `rg -n "SkillOrigin::Github" src/` returns zero hits.
- A roundtrip test that loads a `"source": "github"` manifest still deserializes to `SkillOrigin::Local` and the next save writes `"source": "local"`.

---

## Finding 4 — `SyncReport` name collision between `bundled` and `hub::catalog_client`

- **Severity:** low
- **Form:** 5 (name-drift: two types share a name across module boundaries, conceptually different)
- **File:** `src/bundled/mod.rs:40` (re-export) and `src/hub/catalog_client.rs:152` (independent type)
- **Decision:** **DECIDE** — note the collision but do not propose a fix in this batch
- **Existing review ref:** none

### Evidence

`bundled::SyncReport`:
```rust
// src/bundled/extractor.rs:32-35
pub struct SyncReport {
    pub skills: bool,
    pub plugins: bool,
}
```

`hub::catalog_client::SyncReport` (independent type):
```rust
// src/hub/catalog_client.rs:152
pub struct SyncReport {
    ...
}
```

Used by:
- `bundled::SyncReport` → returned by `sync_official_now` → consumed by `gateway/handlers/bundled_sync.rs:53-56` (JSON `{ok, skills, plugins}`) and by the `bundled.sync` RPC classification in `gateway/rate_limiter.rs:631` and `gateway/lane.rs:182`.
- `hub::catalog_client::SyncReport` → returned by `CatalogClient::sync_into` → used by Hub catalog ingestion, unrelated to `bundled`.

```
$ rg -n "SyncReport" src/bundled/ src/hub/catalog_client.rs
src/bundled/mod.rs:40:pub use extractor::{extract_bundled_content, sync_official_now, SyncKind, SyncReport};
src/bundled/extractor.rs:32:pub struct SyncReport {
src/bundled/extractor.rs:38:pub fn sync_official_now(aleph_home: &Path, kind: SyncKind) -> Result<SyncReport, String> {
src/bundled/extractor.rs:57:) -> Result<SyncReport, String> {
src/bundled/extractor.rs:77:    let mut report = SyncReport {
src/hub/catalog_client.rs:152:pub struct SyncReport {
src/hub/catalog_client.rs:293:    pub async fn sync_into(&self, cache: &CatalogCache) -> SyncReport {
src/hub/catalog_client.rs:298:                    Ok(()) => SyncReport {
src/hub/catalog_client.rs:303:                    Err(e) => SyncReport {
src/hub/catalog_client.rs:310:            Ok(ing) => SyncReport {
src/hub/catalog_client.rs:315:            Err(e) => SyncReport {
```

The two types are in disjoint modules and never reference each other. Every call site uses a qualified path (`crate::bundled::SyncReport` or `crate::hub::catalog_client::SyncReport`), so the name collision does not produce a compile error. But a reader grepping for `SyncReport` will see both, and a future refactor that merges or imports one of them will likely hit a name-shadowing surprise.

### Proposed change

**Option A — DECIDE / leave:** Note the collision in a code comment on each type. Zero behavioral change.

**Option B — Rename:** `bundled::SyncReport` → `BundledSyncReport`, `hub::catalog_client::SyncReport` → `CatalogSyncReport`. Two-file rename; low churn.

### Risk

Low. The collision is cosmetic; either type can coexist with the other. If the next audit touches `hub::catalog_client`, schedule the rename then.

### Verification

After rename:
- `rg -n "\\bSyncReport\\b" src/` shows exactly one definition per module.
- `cargo check -p alephcore` compiles.

---

## Cross-cutting notes

1. **`manifest.json` on-disk shape.** Three findings touch the same wire format. If a future change removes `installed_at` / `version` / renames `Github`, the on-disk schema bumps and existing operator manifests need either `serde(alias = "old_name")` for forward-compat or a one-time migration step. The current shape is small enough (3 variants × 4 fields) that a migration is straightforward.

2. **The `bundled` module is otherwise tightly wired.** Every pub / pub(crate) function, struct, enum, and constant in `src/bundled/` has at least one production consumer (verified in the cross-checked call sites table). No dead modules, no orphaned functions, no name-drift between producers and consumers. The four findings above are the only severed wires in this module.

3. **No stubs / TODO handlers.** A `// TODO` / `unimplemented!` sweep across `src/bundled/` returns zero hits in production code (only `// TODO`-style comments inside `extractor.rs` test bodies, which are out of scope). No form-2 (stub far-end) issues.

4. **No `#[cfg(feature = "...")]` gates.** All four files compile unconditionally. No form-6 (never-compiled far-end) issues.

5. **No `dead_code` lints hit.** The Rust compiler does not flag any of these fields/variants because each has at least one structural use (a construction site for the field, a `match` arm for the variant). The severed-wire layer is the behavioral layer, not the syntactic one.

---

## What was NOT covered

- **Build-time embedding** (`include_dir!("$CARGO_MANIFEST_DIR/skills")` / `plugins/`, `build.rs` setting `ALEPH_VERSION`). Compile-time, out of scope for a runtime review.
- **The bundled `skills/` and `plugins/` trees themselves** — these are build-time git-submodule artifacts; their content was not audited.
- **`include_dir::Dir::files()` / `dirs()` semantics** — assumed to match the published `include_dir` API contract.
- **`git2` internals** — assumed to match libgit2's documented behavior; the sync.rs pin/fetch logic was reviewed for shape, not for upstream API correctness.
- **The marketplace installer** (`crate::extension::marketplace::installer::verify_plugin_integrity`) — only its boundary call at `hub/install.rs:185-189` was noted; the verifier itself is out of scope.
- **`hub::cache::gc_git_checkouts`** — mentioned in the `hub/install.rs:217-219` comment; only referenced, not read.
- **Cross-batch interaction** — the canvas / harness / etc. batches were not consulted. If a finding here collides with a finding in another batch, the merger will need to reconcile.

---

## Summary

- **Total: 4 findings** (0 Critical, 0 High, 0 Medium, 4 Low)
- **All 4 are DECIDE / low** — the wire is severed at the *behavioral* layer (no Rust code reads the symbol) but alive at the *wire-format* layer (serde preserves it on disk). Removing them changes the on-disk shape of `manifest.json` but cannot change runtime behavior.
- **No CUTs proposed** — the affected fields/variants have non-trivial disk-format implications (Github-stamped entries, operator tooling that reads `manifest.json`), and a CUT here would need a serde alias + migration step that is outside the scope of a static review.
- **No action is required for any of these in this batch.** They are flagged so the next time someone touches `src/bundled/manifest.rs` or the manifest schema, the inertness is on record and they can decide CUT vs CONNECT vs leave-with-comment.
- The `bundled` module is otherwise tightly wired and contains no other severed wires, dead scaffolding, name-drift, or orphaned re-exports at the function/struct/const level.
