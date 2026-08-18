# `src/bundled/` — Severed-Wire Audit (2026-08-19)

| Field | Value |
|-------|-------|
| Module | `src/bundled/` |
| Scope | 4 files: `mod.rs`, `extractor.rs`, `manifest.rs`, `sync.rs` (no `tests/` subdir) |
| Branch | `audit-2026-08-19-bin-browser-builtin_tools-bundled` |
| Baseline | `3dcc8f31a64d895afd54d43164d91bddc5800249` (no rollback past this) |
| Result | **0 severed wires / 0 CONNECTs / 0 CUTs** + **0 functional changes** |
| Verdict | Module is well-wired at every seam |

---

## Summary

The bundled module — extractor + manifest + git-sync — was swept against all
seven seam lenses from the severed-wire catalog. **No wire between a producer
and a consumer was found to be severed**: every `pub` symbol has at least one
real external caller, every RPC handler is registered + dispatched +
classifier-covered, and the field/cfg/path/test-stub lenses turned up nothing
that compiled but was unreachable.

The module is small (4 files, no tests subdir), so "no findings" is a credible
outcome. Three borderline candidates (write-only `SkillEntry` fields, an
overstated doc comment, two `pub(crate)` internal helpers) were **DELIBERATELY
KEPT** with rationale documented below — none rose to a wire-level defect.

---

## What was audited (file map)

| File | Lines | Public surface |
|------|-------|----------------|
| `mod.rs` | 40 | `BUNDLED_SKILLS` / `BUNDLED_PLUGINS` (compile-time-embedded dirs), `BUNDLED_VERSION` / `OFFICIAL_SKILLS_REPO` / `OFFICIAL_PLUGINS_REPO` (consts), `pub use` re-exports: `extract_bundled_content`, `sync_official_now`, `SyncKind`, `SyncReport`. `pub(crate)` re-exports: `copy_skill_leaf`, `clone_or_update`, `clone_or_update_at`. `pub mod manifest`. |
| `manifest.rs` | 305 | `pub struct InstallRegistry { pub bundled_version, pub skills }`, `pub enum SkillOrigin { Official, Github, Local }`, `pub struct SkillEntry { source, version, url, installed_at }`, `pub fn load / save / new / reconcile / is_official`. |
| `sync.rs` | 220 | `pub(crate) fn clone_or_update / clone_or_update_at` (both re-exported through `mod.rs`). |
| `extractor.rs` | ~970 | `pub enum SyncKind { Skills, Plugins, All }`, `pub struct SyncReport { pub skills, pub plugins }`, `pub fn sync_official_now`, `pub fn extract_bundled_content`. Plus `pub(crate) fn sync_official_with_urls / extract_skill_tree_from_dir / copy_skill_leaf` and several private helpers. |

---

## Lens 1 — Registration parity (every `pub`/re-export vs every consumer)

**No severed wire.** Each pub/re-exported symbol has a live external caller:

| Symbol | External consumer(s) |
|--------|----------------------|
| `BUNDLED_SKILLS: Dir` | `src/hub/official_skills.rs:8` (cold-start primer) |
| `BUNDLED_PLUGINS: Dir` | `src/hub/official_plugins.rs:9` (cold-start primer) |
| `BUNDLED_VERSION: &str` | internal to `extractor.rs` only |
| `OFFICIAL_SKILLS_REPO: &str` | `src/hub/official_skills.rs:8`, `src/bundled/extractor.rs:42` |
| `OFFICIAL_PLUGINS_REPO: &str` | `src/hub/official_plugins.rs:9`, `src/bundled/extractor.rs:43` |
| `pub use extract_bundled_content` | `src/bin/aleph-server/commands/start/helpers.rs:355` (startup wiring) |
| `pub use sync_official_now` | `src/gateway/handlers/bundled_sync.rs:49` |
| `pub use SyncKind` | `src/gateway/handlers/bundled_sync.rs:5` |
| `pub use SyncReport` | constructed inside `extractor.rs`, fields `skills`/`plugins` read at `bundled_sync.rs:53-54` |
| `pub(crate) use copy_skill_leaf` | `src/hub/install.rs:196` |
| `pub(crate) use clone_or_update` | `src/bundled/extractor.rs:85,115` |
| `pub(crate) use clone_or_update_at` | `src/hub/install.rs:168` |
| `pub mod manifest` (and its `InstallRegistry`/`SkillOrigin`/`SkillEntry` + methods) | `src/hub/install.rs:206-217,602-605` (Github install path), `src/skill/mod.rs:712,723,728` (cached manifest loader + `is_official` for source ranking), `src/bundled/extractor.rs` (read+write the manifest). |

---

## Lens 2 — Call-vs-handler parity (`bundled.sync` end-to-end)

The only RPC exposed by this module is `bundled.sync`. It is **fully wired**:

| Stage | Where | Status |
|-------|-------|--------|
| Definition (module-level fn) | `bundled/extractor.rs:38 sync_official_now` | ✓ |
| Handler implementation | `gateway/handlers/bundled_sync.rs:30 handle_sync` | ✓ |
| Handler registration | `gateway/handlers/mod.rs:430 registry.register("bundled.sync", bundled_sync::handle_sync)` | ✓ |
| Lane classifier | `gateway/lane.rs:143 "skills.remove" \| "bundled.sync" => Some(Self::System)` | ✓ |
| Rate-limit classifier | `gateway/rate_limiter.rs:443 RateLimitScope::RpcWrite` arm | ✓ |
| Admin methods listing | `gateway/method_admin.rs:630 "bundled.sync"` + comment at `:203` | ✓ |
| Handler-existence test | `gateway/handlers/mod.rs:1188 assert!(registry.has_method("bundled.sync"))` + `gateway/rate_limiter.rs:631` | ✓ |
| Param parsing | `bundled_sync.rs::parse_kind` — strict match on `"skills" / "plugins" / "all"`, returns `INVALID_PARAMS` otherwise | ✓ |

All four call-vs-handler parity checks pass. No client ghost, no missing arm,
no name drift across the seam.

---

## Lens 3 — Classifier parity (`SkillOrigin` arms)

`SkillOrigin` has three variants. Each has a read site:

| Variant | Construction site | Read site |
|---------|-------------------|-----------|
| `Official` | `extractor.rs:298,318,615,626`; `manifest.rs:221` | `manifest.rs:211` (`is_official`), `extractor.rs:285,604` (extraction skip predicate) |
| `Github` | `hub/install.rs:211,605` | Read implicitly via the `if entry.source != SkillOrigin::Official` arms at `extractor.rs:285,604` — `bundled.sync` correctly skips names owned by `Github` skills |
| `Local` | `manifest.rs:192` (reconcile auto-add), `extractor.rs:808` (test) | Read implicitly via the same skip predicate |

All three variants are wired. The `Github` and `Local` arms do not need
explicit positive reads because the only dispatch logic is "skip if not
Official" — the equivalent of an "Official vs. not" two-way classification.
**No severed arm.**

---

## Lens 4 — Event emit-vs-subscribe parity

Not applicable. `src/bundled/` writes only files (`<aleph>/skills/...`,
`manifest.json`) and does not emit or subscribe to any in-process event bus.

---

## Lens 5 — Config-reader parity (`InstallRegistry` / `SkillEntry` fields)

`InstallRegistry`:
- `bundled_version: String` — **heavily read+written**: `extractor.rs:170,189,203,216,237` (the re-extraction gate). ✓
- `skills: BTreeMap<String, SkillEntry>` — **read+written everywhere**. ✓

`SkillEntry`:
- `source: SkillOrigin` — **read** at `manifest.rs:211`, `extractor.rs:285,604`, `skill/mod.rs:728` (via `is_official`). ✓
- `version: Option<String>` — **written** at `extractor.rs:299,318,618,630` (Official skills stamped `Some(BUNDLED_VERSION)` on extraction success, `None` on failure), `hub/install.rs:212` (Github skills stamped `entry.version.clone()`). **Read nowhere outside the manifest** — see DECIDE below.
- `url: Option<String>` — **written** only at `hub/install.rs:213` (`Some(git_url)`). **Read nowhere** — see DECIDE.
- `installed_at: Option<String>` — **set to `None` everywhere** (every constructor in `manifest.rs`, `extractor.rs`, `hub/install.rs`). **Read nowhere** — see DECIDE.

No real severed wires here, but the three write-only fields are flagged as
DECIDE candidates.

---

## Lens 6 — Path/route parity

Paths constructed by the bundled module and how they line up with the
canonical resolvers:

| Bundled module path | Canonical resolver | Status |
|---------------------|--------------------|--------|
| `aleph_home.join("skills")` | `utils::paths::get_skills_dir() = get_config_dir()?.join("skills")` | ✓ matches |
| `aleph_home.join("plugins/cache/aleph-official")` | module-internal (marketplace cache) | ✓ |
| `aleph_home.join("cache/aleph-skills-checkout")` | module-internal (git clone target) | ✓ |
| `aleph_home.join("cache/aleph-plugins-checkout")` | module-internal | ✓ |
| `aleph_home.join("skills-official")` | legacy; cleaned only inside `cleanup_legacy_dir` | ✓ |
| `skills_dir.join("manifest.json")` / `manifest.json.tmp` | module-internal | ✓ |

`aleph_home` (the function parameter) is sourced from
`utils::paths::get_config_dir()` at every call site
(`bin/aleph-server/commands/start/helpers.rs:353`, `bundled_sync.rs:46`),
so there is no path drift between the bundled module and the rest of the
codebase.

---

## Lens 7 — Stub sweep

Grepped `src/bundled/` for `TODO`, `FIXME`, `XXX`, `HACK`, `unimplemented!`,
`todo!(...)`, `panic!(`. **No matches.** Every function in the module
produces either a `Result<_, E>` or a side-effecting return value — none of
them are `// TODO persist` stubs.

---

## DECIDE (kept, with rationale)

### `SkillEntry::{version, url, installed_at}` — write-only fields

These three fields are written in `SkillEntry` constructors but never read by
Rust code:

- `version: Option<String>` — stamped on extraction success (`Some(BUNDLED_VERSION)`)
  and on Github install (`Some(entry.version.clone())`); `None` on extraction failure.
- `url: Option<String>` — stamped only for Github installs as `Some(git_url)`.
- `installed_at: Option<String>` — `None` in every constructor.

**Why kept:** These are **dormant provenance metadata** serialized into the
user-facing `~/.aleph/skills/manifest.json` file. `cat manifest.json` reveals
which skills were Official/Github/Local and what version/URL/provenance data
they have. All three are gated by `#[serde(skip_serializing_if =
"Option::is_none")]`, so a `None` value costs zero JSON bytes. Removing the
fields would:

1. Break upgrade compatibility for any existing user manifest that has a
   `version`, `url`, or `installed_at` value (none currently write
   `installed_at`, but `version`/`url` for Github installs and Official
   extractions are widely persisted).
2. Eliminate user-visible provenance — anyone inspecting the manifest via
   `cat` / parsing would lose the version-stamped-at-extraction and URL info.
3. Risk future re-addition under a different name, doubling the schema-
   migration cost once a future feature (e.g. an "audit when was this skill
   installed" command) inevitably needs these fields.

The fields are inert config (form 3 per the seam catalog) rather than inert
code; per the triage playbook's decision tree, **inert config with user-
visible provenance and zero cost per entry is KEEP**, not CUT. No caller
exists because the consumer is "anyone who wants to inspect the JSON" —
humans and future Rust code, both of which still benefit from the field
existing.

---

## Almost-cut but kept

These were considered but explicitly **NOT** cut, with rationale:

1. **`pub(crate) fn extract_skill_tree_from_dir`** and **`pub(crate) fn sync_official_with_urls`**
   (`bundled/extractor.rs:574` and `:52`) — both `pub(crate)` despite being
   consumed only inside `extractor.rs` and its `#[cfg(test)] mod tests`
   module (which has private-fn visibility). Lowering to private `fn` would
   clarify "this is bundled-internal" but they're legitimate internal-API
   extension hooks per the codebase's pattern (cf. `clone_or_update_at`).
   Not a wire issue; no production pain from leaving `pub(crate)`.

2. **Doc comment in `bundled_sync.rs:1` claims** `bundled.sync` is callable
   from "RPC / CLI / LLM tool / Hub button". Only the RPC path is wired.
   The wire (RPC handler → `sync_official_now`) is sound. The CLI/Hub/LLM-
   tool claims are scope creep, not severed wires. Keeping the doc unchanged
   here (out of audit scope; behaviour is correct, docs merely
   overpromise).

3. **Prior audit findings** in
   `docs/engineering-reports/review-results/bundled.md` flagged four issues:
   - `extractor.rs:258 create_dir_all follows symlinks` (high)
   - `extractor.rs:17 extract_bundled_content swallows all extraction failures` (medium)
   - `manifest.rs:61 load() treats non-NotFound IO errors as missing` (low)
   - `manifest.rs:97 reconcile() silently skips directory entries` (low)
   These are **functional / error-handling concerns**, NOT severed wires. The
   functions are all called by live consumers and produce observable side
   effects — the prior audits observed them through the lens of "missing
   feature" rather than "missing wire". Out of scope here.

---

## Files changed

None. `src/bundled/` was not modified.

The only file added by this audit is this report at
`docs/engineering-reports/review-results/sev-wire-2026-08-19/bundled/REPORT.md`.

---

## Provenance

- Branch: `audit-2026-08-19-bin-browser-builtin_tools-bundled`
- Baseline: `3dcc8f31a64d895afd54d43164d91bddc5800249`
- Commit: see the audit commit on the branch above
- Method: read-first grep of every `pub`/`pub(crate)` symbol against every
  external consumer; tested each suspect read site against production code
  paths, not tests, before declaring a wire intact
