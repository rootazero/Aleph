# `src/hub/` static review — hub-statics cycle

This file mirrors the format of `clawhub.md` (one ISSUE|file|sev|title|body
line per finding, severity first). It is the static-review artefact for the
`src/hub/` (Unified Extensions Hub) module — the successor of the deleted
`src/clawhub/` directory the project decommissioned on commit b4d395fca.

Five subagent batches were run in parallel against the worktree:

| Batch | Scope | Findings (C/W/I) |
|-------|-------|------------------|
| 1     | `types.rs`, `mod.rs`                         | 0 / 1 / 8 |
| 2     | `cache.rs`, `catalog_client.rs`, `hub_catalog.rs` | 3 / 5 / 3 |
| 3     | `install.rs`, `origin.rs`, `primer.rs`      | 0 / 4 / 6 |
| 4     | `reconcile.rs`, `secrets.rs`, `verify.rs`   | 0 / 4 / 5 |
| 5     | `trust.rs`, `official_mcp.rs`, `official_plugins.rs`, `official_skills.rs` | 0 / 4 / 7 |
| **Total** |                                      | **3 / 18 / 29** |

The 3 CRITICAL + 5 high-value WARNING fixes were committed; the remaining
13 WARNINGS and 29 INFOs are catalogued below for triage in a future
review pass.

## Fixes shipped in this cycle (commits)

| SHA | Subject | Review tie |
|-----|---------|------------|
| 582dc6eb1 | `hub: cache: enable WAL + transactionally wrap replace_source/upsert_many` | A1 + A2 |
| 0c9089b23 | `hub: catalog: cap body size, switch to safe_fetch, sanitize generated_at, tighten schema version` | A3 + C3 + C4 + I2 |
| 452098691 | `hub: cache: use ExtensionKind::as_str() / ExtensionCategory::as_str() instead of JSON roundtrip` | C5 |
| 689efe29f | `hub: types: derive requires_config from install_spec in HubCatalogEntry::into_entry` | batch-1 W1 |
| 9e47a4bf1 | `hub: trust: extend injection-scan to NBSP + add Chinese phrase set` | batch-5 W4 + I2 |
| ee9d96f69 | `hub: install: scheme-restrict, atomic stage+rename, .git-cache cleanup` | batch-3 W2+W3+W4 |
| 16aea16df | `hub: official_mcp: require non-templated command for stdio projections` | batch-5 W1 |
| 4e2c24053 | `hub: verify: Degraded is "running", not "not running"` | batch-4 W1+W2 |
| 938e71f32 | `hub: install: keep .git-cache after install for re-pin; test bypass for local-path fixture` | batch-3 follow-up |
| eecf98747 | `hub: cache/catalog_client/trust: tighten implementation; let mut + NBSP-replaced pass` | follow-up after cargo check |

All 10 commits fast-forwarded onto `main` (no PR).

## Not fixed (deliberately left for a future cycle / DECIDE)

The following findings were not fixed in this cycle, with rationale:

  - **Single Arc<CatalogCache> for both opens** (`bin/aleph-server start/mod.rs`)
    — Larger refactor across `register_agent_handlers` and ~30 call sites.
    WAL is the surgical fix; the single-Arc ideal is net-better but not
    urgent.
  - **Inconsistent `requires_config` between stdio and remote**
    (`trust.rs::secrets_of`) — Functional, by-design. Two spec fragments
    disagree; needs product / spec decision.
  - **Asymmetric consent gate** (`extensions.install` vs `hub_install_run`)
    — Both paths gate on distinct key-sets by design; aligning them is a
    UX spec call, not a bug fix.
  - **`OriginSha256 == None` for official projections** (`trust.rs`)
    — Official sources are immutable in deployment, so a per-publish
    digest makes less sense than for community entries; leaving the field
    empty is fine. Worth a doc comment, not a behavior change.
  - **`hub.cache.installed` column written but never read** — Requires
    a DB migration; better as part of a cache schema-version bump.
  - **`ExtensionCategory` 9 wire-only variants, dead `icon`/`config_schema`
    / `placeholder` fields** — Spec-level (the upstream publisher uses the
    13-category taxonomy); need human sign-off before any cut.
  - **`as_str()` vs `serde` rename_all drift** — Hand-written `match` arms
    differ from auto-rename rule; safer to leave where a future rename
    compiler-errors than to silently auto-derive.
  - **`verify_install` does not check `secret resolvable-ness`** — A real
    gap, but probing the secrets vault at install-verify time needs
    cross-module plumbing the spec hasn't asked for; surfaced as a
    follow-up.

## Failure surfaces discovered but not patched (batch-3+ INFO)

  - `install_git_skill` subdir flattening can collide across different repos
    that use a common subdir name like `src/`. Lower priority than the
    path-traversal hardening above; same call-site though.
  - `mcp_server_id` does not sanitize beyond `:/`. Same call site as the
    target-naming concern; better fixed together.

See `/tmp/hub-review/summary/batch-{1..5}-REPORT.md` for the unabridged
subagent reports.
