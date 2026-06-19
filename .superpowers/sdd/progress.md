# SDD Progress — Extensions Store P0 Foundations

Plan: docs/superpowers/plans/2026-06-19-extensions-store-p0-foundations.md
Branch: feat/unified-extensions-store
Base before Task 1: 767f5f63d
Execution mode: controller-driven + batched builds (user-approved 2026-06-19). Gate = `cargo test -p alephcore --lib store::` (`--lib` skips the pre-existing-broken `tests/cancellation_chain.rs`). Per-group + final reviewer dispatch.

- [x] Task 1: complete (commit 767f5f6..38e528f, review clean — types verified vs plan; 3 tests pass)
- [x] Task 2: complete (commit 5c6327424, batched w/ Task 3 — 3 new types tests pass)
- [x] Task 3: complete (commit 5c6327424 — ExtensionEntry roundtrip test passes)
- [x] Task 4: complete (commit f99fdbd37, batched w/ Task 5 — 4 cache tests pass)
- [x] Task 5: complete (commit f99fdbd37 — 2 reconcile tests pass; FIXED plan's private-module imports to crate::extension::{PluginRecord,PluginStatus} + crate::mcp::manager::{HealthStatus,McpServerInfo,McpTransportType}, and s.id.as_str())
- [x] Task 6: complete (commit fab9dd6ad — façade; lib compiles, 3 parse_local_id tests pass)
- [x] Task 7: complete (commit c0ac645cf — registration; `cargo build --bin aleph-server` clean, no warnings)

## Plan deviations (for final review)
Interface verification (2 Explore passes) found the plan's Task 5–7 code assumed
APIs that DO NOT exist. Corrections applied:
- `SkillSystem::current()` → reuse `crate::gateway::handlers::skills::shared_system()` (made `pub(crate)`); SkillId/SkillConfigUpdate via `crate::domain::skill` / `crate::skill`.
- `mgr.plugin_registry().list_plugins()` → added thin `ExtensionManager::list_plugin_records()` (mirrors existing `get_plugin_record`); reconcile keeps `PluginRecord`.
- Dropped the plan's `src/store/lifecycle_glue.rs` entirely — handlers reuse real lib fns (`set_plugin_enabled`, `unload_runtime_plugin`, `default_plugins_dir`, `reload`). YAGNI.
- Handlers take `Option<McpManagerHandle>` so plugins/skills/catalog work even if MCP didn't spawn.
- reconcile imports fixed to public re-exports: `crate::extension::{PluginRecord,PluginStatus}`, `crate::mcp::manager::{HealthStatus,McpServerInfo,McpTransportType}`.
- `aleph-server` is a BIN TARGET of the `alephcore` package (build via `cargo build --bin aleph-server`, not `-p aleph-server`).
- Smoke test (plan Task 7 Step 4, live daemon RPC) NOT run — needs a running server + ws client; left for manual/UI verification. Compile-level wiring verified.
- Pre-existing broken integration test `tests/cancellation_chain.rs` (missing `SpawnRequest.strategy`) blocks full `cargo test -p alephcore`; gated on `--lib` instead. Unrelated to this work.

## Final whole-branch review (opus, range 767f5f63d..c0ac645cf)
Verdict: CHANGES REQUIRED → resolved. 1 CRITICAL fixed (commit 4d366e489), no Important.
- [x] C1 CRITICAL: path traversal in extensions.uninstall plugin route — FIXED + regression test (4d366e489).
- Minors recorded, DEFERRED to their phases (not merge-blocking; branch is mid-feature):
  - M1: `cache::replace_source` doc says "atomic" but isn't transactional — fix when P1 adds its first caller (+ test). Wrap in `conn.transaction()`.
  - M2: `cache.rs` upsert/query use `serde_json::to_value(..).unwrap().as_str().unwrap()` — replace with the existing `ExtensionKind/Category::as_str()` (safe today, pure cleanup).
  - M5: `handle_toggle` discards `set_plugin_enabled`'s bool (unknown plugin id → ok:true). Optional; consider erroring on false.
  - M3 (empty catalog in P0) and M4 (mcp enabled = !Stopped/Dead) are by-design, not issues.

## P0 STATUS: COMPLETE (code + unit tests + review). Commits 38e528f03..4d366e489.

---
# P1 Source Layer — plan: docs/superpowers/plans/2026-06-19-extensions-store-p1-source-layer.md
Base before P1: 4d366e489. Same execution mode (controller-driven + batched builds).
Interface verification (Explore): marketplace APIs all MATCH the plan; deps async-trait/serde_yaml/futures/reqwest all present (no Cargo.toml change). `urlencoding` NOT a dep → encode manually. Config `plugin_marketplaces: HashMap<String,PluginMarketplaceEntry>` ≠ `MarketplaceConfig` → convert in Task 5 wiring. `parse_marketplace_manifest` reachable only via full path (not re-exported).
- [x] Task 1: complete (commit c56e9282a — trait+registry, 1 test)
- [x] Task 2-4: complete (commit a62000014 — 3 providers, 5 tests). Simplified docker test to registry.get() (plan's .iter().find() wouldn't compile); manual %2F encoding (no urlencoding dep); added Default impls to providers.
- [x] Task 5: complete (commit f6311ea54 — registry_builder test green, `cargo build --bin aleph-server` clean).

## Final whole-branch review (opus, range 4d366e489..f6311ea54)
Verdict: APPROVE WITH MINOR. Zero Critical, zero Important. Verified: no panic vectors (all non-test unwraps total), keep-last-good honored (replace_source only on Ok-non-empty), source_id/id() aligned, pagination bounded (cursor + 10k cap), arg ordering correct, startup spawn Send+'static with config guard dropped pre-await. Blocking-in-async MarketplaceProvider::sync = acceptable for background v1 (spawn_blocking = P2 follow-up).
Minors (optional, forward-looking — NO change made): `Query`/`SourceProvider::search` unused scaffolding (reserved for P2/P3 on-demand search); `SyncCtx` empty placeholder (reserved for a shared reqwest::Client in P2 — avoids per-provider connection pools).

## P1 STATUS: COMPLETE (code + unit tests + review). Commits c56e9282a..f6311ea54.
Runtime smoke (live sync → extensions.catalog) still un-run (same as P0).

---
# P2 Trust Rails + Secure Install — plan: docs/superpowers/plans/2026-06-19-extensions-store-p2-trust-install.md (commit 8849ba746)
Base before P2: 8849ba746. Same execution mode (controller-driven + batched builds). SECURITY-CRITICAL phase.
Locked design (user-decided 2026-06-19): secrets → encrypted SecretVault via SharedTokenManager (NO OS keychain); MCP secrets injected per-server at spawn from vault (${vault:KEY} refs, never plaintext, never shared child env) — "安全注入"; OCI install deferred (no runtime); plugin install reuses install_to_scope (SHA256).
- [x] T1: complete (commit e94388159, batched w/ T2 — disclosure payload, 3 trust tests pass)
- [x] T2: complete (commit e94388159 — injection scan; zero_width/bidi/phrase detection)
- [x] T3: complete (commit 1229533e1 — REVISED: reuses existing `{{secret:NAME}}` pipeline, see deviation below; field_key/secret_ref helpers + round-trip-through-canonical-parser test, 2 tests pass)
- [x] T4: complete (commit cbed04e7a — per-server secret injection at spawn; 2 secret_resolver tests pass, `cargo build --bin aleph-server` clean)
- [x] T5: complete (commit f609f3158 — install routing; `{{secret:}}` refs via secret_ref/field_key, plugin via install_to_scope, OCI Err; 2 tests pass. FIXED plan's `crate::extension::types::PluginScope` → public re-export `crate::extension::PluginScope`.)
- [x] T6+T7: complete (commit 474d9d0c5 — COMBINED: both live in extensions/install.rs + one registration, so built/committed as one unit to avoid wiring install.rs twice. handle_disclosure/configure/install (trust gate: OCI reject → ack gate → missing-required → store_secret per secret field → run_install → verify → pin); split_fields/missing_required pure (3 tests). Added `CatalogFilter.id` + WHERE clause (+ query_by_id test); FIXED catalog.rs CatalogFilter literal to `..Default::default()`. Registered extensions.{disclosure,configure,install} in builder + start/mod.rs, building MarketplaceManager from `marketplace_configs.clone()` (registry gets the clone) + shared_token_mgr vault. Verify is tolerant of add_server auto-start.)
- [x] T8: build gate met (`cargo build --bin aleph-server` clean, dev profile, exit 0). Pin record implemented in handle_install (`pin: {version, sha256}`). Live ws smoke (install a stdio MCP → disclosure→ack→install→start→tools; secret-bearing MCP → vault stores plaintext, config holds `{{secret:}}`, child gets resolved value; marketplace plugin → SHA256; OCI → unsupported err) DEFERRED to manual/UI verification — needs a running daemon + ws client (same deferral as P0/P1).

## Final whole-branch security review (opus, range 8849ba746..474d9d0c5)
Verdict: APPROVE WITH MINOR. Zero Critical, zero High. All 6 security invariants verified PASS against changed + unchanged code:
no plaintext secret leak (field_key sanitizes → round-trips through extract_secret_refs; config holds only {{secret:}}); fail-closed spawn boundary
(resolve_secret_env at the single ExternalServerConfig site, covers all 6 start paths, unresolved dropped, never in daemon std::env); ack gate
ordered before any store_secret/install side effect; OCI rejected pre-mutation; path/id safe (install_plugin_from_cache rejects ../\; server id is map-key only); parameterized SQL for CatalogFilter.id; no panics/secret-logging on attacker data.
- [x] I-1 (IMPORTANT) FIXED (commit <pending>): required non-secret env field with no default was collected into split_fields' plain list then DROPPED — passed missing_required but never reached the child env (server starts misconfigured). Fix: added `plain_values` to InstallContext + `mcp_config_from_spec` param; precedence secret-ref → submitted plain → default. Test extended (required non-secret ACCOUNT with no default flows through).
Minors recorded (follow-ups, NOT merge-blocking):
  - M-1: `mcp_server_id` doesn't strip `..`/`\` (inert today — id is map-key/process-name only, never a path; latent footgun).
  - M-2: McpRemote secret headers are stored in vault but not yet referenced (header injection deferred) → orphaned encrypted entry; skip-store or TODO when header injection lands.
  - M-3: `missing_required` validates only McpStdio (McpRemote required headers unvalidated — land with header injection).
  - M-4: injection scan is advisory (non-gating) + covers name+description only — by design per spec.

## P2 STATUS: COMPLETE (code + unit tests + security review; 1 Important fixed). Commits e94388159..<I-1 fix>. Live ws smoke deferred (manual/UI).

## MAJOR DEVIATION (T3/T4/T5+) — reuse existing secret pipeline instead of inventing `${vault:KEY}`
Interface research (reading `src/secrets/`) found Aleph ALREADY has the canonical secret-injection pipeline the spec's Global Constraints mandate reusing ("Reuse, do not fork ... credential injection"):
- `crate::secrets::AsyncSecretResolver` trait (`async resolve(name) -> Result<DecryptedSecret, SecretError>`).
- `crate::secrets::render_with_secrets(text, &resolver)` resolves `{{secret:NAME}}` placeholders AND records each injection for leak detection.
- `crate::secrets::VaultSecretResolver::new(Arc<SharedTokenManager>)` — production resolver over the SAME vault used by `store_secret`/`get_secret`.
- `extract_secret_refs` placeholder parser: names limited to `[A-Za-z0-9_.-]` (NO colons).
My committed P2 plan invented a parallel `${vault:KEY}` scheme + new `store::secrets::SecretResolver` trait + `resolve_vault_env`, authored without knowledge of this module. CORRECTED to reuse the existing pipeline (DRY, free leak-detection, one consistent placeholder format across HTTP headers/WASM/MCP, avoids near-collision with the existing `${VAR}` process-env expansion). Changes vs plan:
- T3: `store::secrets` shrank to `field_key(kind,id,field)` (namespaced, placeholder-SAFE → `.` separators, sanitize non-`[A-Za-z0-9_.-]` to `_`) + `secret_ref(name)` → `{{secret:NAME}}`. NO trait, NO VaultResolver (use `crate::secrets::VaultSecretResolver`). Format: `ext.{kind}.{sanitized_id}.{field}`.
- T4: helper `src/mcp/manager/secret_resolver.rs::resolve_secret_env(env, Option<&dyn AsyncSecretResolver>)` calls `render_with_secrets` per value containing `{{secret:`; unresolved → DROP key (fail-closed, warn). Threaded `Option<Arc<dyn AsyncSecretResolver>>` onto `McpManagerActor` (field + `with_secret_resolver` builder); applied at the SINGLE `ExternalServerConfig` materialization site (`actor.rs` start_server_internal — funnel for all 5 start paths: auto-start, add, start, restart). Startup: DEFERRED `actor.run()` spawn until AFTER `initialize_vault` (mod.rs ~414) so persisted vault-backed servers resolve secrets on boot auto-start (no reboot race); injects `VaultSecretResolver` via `with_secret_resolver`. Secrets never enter the daemon's own `std::env`.
- T5+: `mcp_config_from_spec` writes `secret_ref(field_key(...))` (`{{secret:NAME}}`), NOT `vault_ref`. Plugin install reuses `MarketplaceManager::install_to_scope` (SHA256). T7 stores secrets via `SharedTokenManager::store_secret(field_key, value)`.
  Deviations: used `app_config.read().await` (loaded_app_config is MOVED at start/mod.rs:571 — Explore was wrong); inlined the PluginMarketplaceEntry→MarketplaceConfig conversion (mirrors plugins.rs load_marketplace_configs); added `extensions.sources.list` (+ ProviderRegistry::list_sources) alongside the plan's refresh.
  KNOWN TRADEOFF (for review): MarketplaceProvider::sync does blocking git/fs inside async (plan-accepted v1; wrap in spawn_blocking if it stalls a worker).
Runtime smoke test (plan Task 7 Step 4: live daemon `extensions.installed`/`toggle`) NOT yet run — needs a running server; recommend manual/UI verification or a scripted ws smoke before P1.
