# Extension cold path D1 — implementation report

Worktree: `D:/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256`
Basis: `.analysis/browser-live-runtime/extension-cold-design.md`
Status: implemented, **not compiled** (no Cargo in this run, per task). `rustfmt --edition 2021 --check src/extension/lifecycle.rs src/extension/mod.rs` -> rc=0.

## Changed paths

- `src/extension/mod.rs`
- `src/extension/lifecycle.rs`
- (this report)

Not touched: `src/skill/**`, `src/hub/cache.rs`, `src/hub/reconcile.rs` (not needed), any test file.

## What changed

### mod.rs
- `adapter_registry: AdapterRegistry` -> `adapter_registry: Arc<AdapterRegistry>` (constructor wraps `AdapterRegistry::with_defaults()` in `Arc::new`). Private field; method calls auto-deref, no public API change.
- `collect_plugin_dirs(&self)` now delegates to a new associated fn
  `collect_plugin_dirs_with(discovery: &DiscoveryManager, extra_parents: Vec<ProjectPluginParent>) -> ExtensionResult<CollectedPluginDirs>` (no `&self`, same body/priority/shadow order).
- New `extra_plugin_scan_parents(&self) -> Vec<ProjectPluginParent>`: `cfg(test)` clone of `extra_plugin_parents`, otherwise empty (same effective input as before).

### lifecycle.rs
- New owned scan types: `ScannedDir { found, identified, manifest: Option<PluginManifest>, legacy_marker: bool }`, `ScanPlan { dirs, unreadable }`.
- New `fn scan_plugins_blocking(discovery, adapters, extra_parents) -> ExtensionResult<ScanPlan>`: discovery walk, adapter identify (all dirs, incl. shadowed — as before), `parse_manifest_from_dir_cached_global` for `Parsed` dirs, `.disabled` stat when `origin != ClaudeCache`.
- New `fn identify_with(adapters: &AdapterRegistry, found)`; `identify(&self, ..)` wraps it (still used by sync `discovered_plugin_ids`).
- `discover_and_mount`: runs the scan via `crate::skill::run_blocking_fs("ExtensionManager::scan_plugins_blocking", ..)` over `Arc` clones + owned parents. Join cancellation -> `ExtensionError::Runtime("extension discovery was cancelled (runtime shutting down)")`, returned **before** `registry.clear()` — i.e. the same early-return shape a discovery error already had (`loaded` stays false, no `after_transition`). Then the async walk iterates `plan.dirs.into_iter().rev()` exactly as before: shadow/winner map, unparsed rows, marker migration, `build_record`, `admit` trust gate, `mount_parsed` — order, locks and registry publication unchanged.
- `migrate_legacy_disabled_marker(dir, id, present: bool)`: takes the scanned answer; no `exists()` on the runtime. Migration write/remove itself unchanged (still async path, see residuals).
- `build_record(.., manifest: Option<&PluginManifest>)`: no re-parse; kind from the scanned manifest.
- `mount_parsed(output, record, manifest: Option<PluginManifest>)`: duplicate manifest parse (A8) removed; it consumes the same manifest `build_record` read.
- `mount_inner` (single-plugin mount/reload path): adapter `parse_dir` + manifest parse offloaded via `run_blocking_fs("ExtensionManager::mount_inner.parse", ..)`. Cancellation -> `MountError::Parse { reason: "manifest parse was cancelled (runtime shutting down)" }`, returned before any row write.

### Semantics preserved / changed
- Discovery priority, shadowing, unreadable-source publication (same registry guard), trust gates, `load_guard` serialisation, projection: unchanged.
- Detached completion after a dropped awaiting future is harmless: the blocking scan writes only what the async walk already wrote (adapter `ensure_data_dir_if_referenced`, global manifest LRU fill).
- Minor behaviour delta: shadowed dirs now also get a (cached) manifest parse in the scan, because shadowing is decided in the async walk. Pure extra read, no observable output change. Could be removed by computing winners inside the scan.
- Manifest kind and mount steps now read one parse result instead of two — previously a file edited between the two reads could yield a kind/steps mismatch; now consistent.

## Residual synchronous sites on the runtime (untouched, by design or scope)
1. A14 WASM `loader::load_wasm_effect` / Extism `load_plugin` — not offloaded: cancellation/disposer safety of a detached plugin instance unresolved.
2. A9 `provision_data_dir` (create_dir in mount_parsed).
3. A10 vault/secret resolve during mount.
4. A12/A13 `projection.rs` `discover_skill_dirs` / `is_dir` under the projection lock (`republish_plugin_projections`).
5. `after_transition` -> `Self::user_hook_configs()` (hooks.json reads + `ProjectStore::shared().list()`); not offloaded because ProjectStore thread-affinity and the "one atomic executor swap" contract under cancellation need their own decision.
6. Legacy `.disabled` migration write/remove (rare, one-shot).
7. Watcher `sync_user_hooks` path and sync CLI `discovered_plugin_ids` (uses `collect_plugin_dirs` + `identify` synchronously; not a runtime path for catalog).

## Compile-risk notes
- **Hard dependency**: `crate::skill::run_blocking_fs` (src/skill/mod.rs:113, `pub(crate) async fn run_blocking_fs<T: Send+'static, F: FnOnce()->T+Send+'static>(ctx: &'static str, f: F) -> Option<T>`) exists only in the other agent's dirty working tree, not HEAD. If that change is dropped, these two call sites fail to resolve.
- `Send + 'static` bounds: closure captures `Arc<DiscoveryManager>`, `Arc<AdapterRegistry>`, `Vec<ProjectPluginParent>`, `PathBuf`; returns `ExtensionResult<ScanPlan>` / `anyhow::Result<(AdapterOutput, Option<PluginManifest>)>`. Requires `DiscoveryManager`, `AdapterRegistry` (`Box<dyn ManifestAdapter>`, trait is `Send + Sync`), `AdapterOutput`, `PluginManifest`, `ExtensionError` to be `Send`. All are plain data per reading; not compiler-verified.
- `Self::scan_plugins_blocking` / `Self::collect_plugin_dirs_with` inside a `'static` closure: `ExtensionManager` has no lifetime/generic params, so OK.
- Double `??` on `Option<ExtensionResult<_>>` after `.ok_or_else(..)`: outer is `Result<ExtensionResult<ScanPlan>, ExtensionError>`; both `?` convert to the fn's `ExtensionResult` error type (assumed `discover_and_mount` returns `ExtensionResult<..>` — it did use `?` on `collect_plugin_dirs()` before).
- Visibility: `collect_plugin_dirs_with` / `extra_plugin_scan_parents` are private inherent fns in `extension/mod.rs`, called from child module `extension::lifecycle` — allowed.
- Unused-import risk: none expected (`AdapterRegistry` now imported in lifecycle and used in signatures).

## Tests
None added. A deterministic thread-boundary test of `discover_and_mount` needs an injectable discovery root (aleph_home override seam in `src/discovery`, outside scope) to avoid real home. A source-route guard in the style of `src/skill/mod.rs` tests is possible later. Existing test at `src/extension/mod.rs` (~1501) still calls `collect_plugin_dirs()` and remains valid.

## Required next verification (not done)
`CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 cargo check -p alephcore` then `cargo test -p alephcore extension::` with the usual home-isolation audit.
