# D1 – cold `ensure_loaded` extension chain: off-loop design

Scope: `ExtensionManager::ensure_loaded` (src/extension/mod.rs:510) -> `load_all_locked` (src/extension/lifecycle.rs:185) -> `discover_and_mount` (lifecycle.rs:204) + `after_transition` (lifecycle.rs:1040). Excludes the skill scan / CatalogCache SQL (already being fixed elsewhere).

## 1. Synchronous steps on the cold async path (inventory)

| # | Site | Blocking work |
|---|------|---------------|
| A1 | `collect_plugin_dirs` mod.rs:~720 | `ProjectStore::shared()` (first call opens `projects.db`) + `.list()` sync SQLite under std `Mutex` |
| A2 | `DirectoryScanner::discover_plugins_with_extra` scanner.rs:286 / `scan_plugin_parent` | `is_dir`, `read_dir` x2 levels, `symlink_metadata`, `has_plugin_manifest` stats, per parent |
| A3 | `claude_cache::discover_claude_cache` claude_cache.rs:167 | `read_to_string(installed_plugins.json)` + JSON parse (only when Claude root on) |
| A4 | `collect_plugin_dirs` loop | `d.path.canonicalize()` per plugin |
| A5 | `identify` lifecycle.rs:419 -> `AdapterRegistry::parse_dir` adapter.rs:108 | adapter `detect` stats, manifest read/parse, skills/commands/agents/hooks/.mcp.json walk+read; `PluginVars::ensure_data_dir_if_referenced` (`create_dir_all`, plugin_vars.rs:96). Called for EVERY dir incl. shadowed ones |
| A6 | `migrate_legacy_disabled_marker` lifecycle.rs:361 | `legacy_marker.exists()` (rest already tokio::fs) |
| A7 | `build_record` lifecycle.rs:389 | `parse_manifest_from_dir_cached_global` (stat for cache key; read+parse on miss) |
| A8 | `mount_parsed` lifecycle.rs:582 | second `parse_manifest_from_dir_cached_global` (stats again) |
| A9 | `mount_parsed` step 3 | `mcp_config::provision_data_dir` (`create_dir_all`, mcp_config/mod.rs:312), MCP kind only |
| A10 | `publish_plugin_settings` -> `VaultSecretResolver::resolve` vault_resolver.rs:24 | sync `SharedTokenManager::get_secret` (vault read + decrypt) inside async fn; only when settings contain `{{secret:..}}` |
| A11 | `after_transition` -> `Views::user_hook_configs` mod.rs:~797 | `current_dir`, `ProjectStore::list` (SQLite), `load_user_hooks` reads/canonicalizes hooks.json files |
| A12 | `republish_plugin_projections` projection.rs:208 | `discovery.discover_skill_dirs()` (`read_dir` component scan) |
| A13 | `derive_plugin_projection` projection.rs:162 | `record.root_dir.join("skills").is_dir()` per active plugin, UNDER `plugin_registry.read()` |
| A14 | `mount_parsed` step 2 -> `load_wasm_effect` loader.rs:307 | extism module compile in `loader.write().await.load_plugin` — third-party runtime (see section 5) |

Already off-loop: service start (service_ops.rs:89), MCP spawns (actor), skill init (other implementer).

## 2. Design: two-phase discover_and_mount

Keep order and lock semantics: `unmount_all` -> `publish_plugin_settings` -> **scan (blocking pool)** -> registry clear/publish -> per-dir admit/mount (async, unchanged order: reverse priority, `winners`, `failed_plugin_ids`).

### 2.1 Owned scan input / output (lifecycle.rs, new private items)

```rust
struct ScannedDir {
    found: DiscoveredExtensionDir,      // path, origin, scope_key (all owned)
    identified: Identified,             // Parsed(AdapterOutput) | Unparsed{id,error}
    manifest: Option<PluginManifest>,   // from parse_manifest_from_dir_cached_global, Ok only
    legacy_marker: bool,                // origin != ClaudeCache && dir/.disabled exists
}
struct ScanPlan { dirs: Vec<ScannedDir>, unreadable: Vec<PathBuf> }

fn scan_plugins_blocking(
    discovery: Arc<DiscoveryManager>,
    adapters: Arc<AdapterRegistry>,
    #[cfg(test)] extra: Vec<ProjectPluginParent>,
) -> ExtensionResult<ScanPlan>   // A1-A7 inside; pure sync, no manager borrow
```

- Field change: `adapter_registry: AdapterRegistry` -> `Arc<AdapterRegistry>` (mod.rs:195/388). Stateless, built once; `ManifestAdapter: Send + Sync` already (adapter.rs:37). Do NOT construct a second registry in the closure (two copies of one fact, shape 1).
- `collect_plugin_dirs(&self)` becomes a free fn `collect_plugin_dirs(&DiscoveryManager, extra)` called only inside the closure; `discovered_plugin_ids` (offline CLI, sync) keeps calling it directly.
- `identify` becomes `fn identify(adapters: &AdapterRegistry, found) -> Identified` (same single derivation shared with `discovered_plugin_ids`).

### 2.2 Async side

```rust
let plan = crate::skill::run_blocking_fs("extension.discover", move || scan_plugins_blocking(d, a, extra))
    .await
    .ok_or_else(|| ExtensionError::Runtime("extension discovery cancelled".into()))??;
```
Reuse `run_blocking_fs` (src/skill/mod.rs:113): panic re-raised on caller (same as today's inline panic); join-cancel (runtime shutdown) -> `Err`, which takes the exact existing `collect_plugin_dirs()?` error path (returns before registry clear, `loaded` stays false, no `after_transition`).

Then the existing loop iterates `plan.dirs.iter().rev()` using `scanned.identified`; `migrate_legacy_disabled_marker(dir, id, scanned.legacy_marker)` returns early on `false` (no `.exists()`); `build_record(&output, …, scanned.manifest.as_ref())` takes `record.kind` from the pre-parsed manifest; `mount_parsed(output, record, manifest: Option<PluginManifest>)` drops its own re-parse (A8). All registry writes, `admit`, trust/activation gates, shadow diagnostics, mount order are unchanged and stay on the async side under `load_guard`.

`mount_inner` (single-plugin RPC mount, not cold path but same face, shape 9): replace its inline `parse_dir` + manifest with `run_blocking_fs` of `(identify, parse_manifest)` on the owned `root_dir`, same `MountError::Parse` mapping; cancel -> `MountError::Parse{reason:"cancelled"}`.

A9: wrap `provision_data_dir` in `run_blocking_fs` with cloned `(id, root_dir, declared_servers.clone())`; cancel = skip (same as today's warn-and-continue on mkdir failure).

### 2.3 after_transition (A11-A13)

```rust
let disc = Arc::clone(&self.discovery);
let Some((user_hooks, skill_dirs)) = run_blocking_fs("extension.views", move || {
    (Self::user_hook_configs(), disc.discover_skill_dirs().unwrap_or_default())
}).await else { return };   // shutdown: keep previous executor+views
```
Never install an executor without the user layer on a cancel (fail-open, shape 8). `republish_plugin_projections(skill_dirs)` takes the pre-scanned dirs (sole caller; G3 census unaffected). `derive_plugin_projection`: snapshot candidates `(root/skills, id, scope_key, root)` under the read lock, drop lock, filter `is_dir` in the same `run_blocking_fs` batch (or a second one), preserving order + dedup; agents part stays under lock. `sync_user_hooks` (watcher path) gets the same offload.

### 2.4 A10 vault
`VaultSecretResolver::resolve`: `let inner = Arc::clone(&self.inner); let name = name.to_owned(); spawn_blocking(move || inner.get_secret(&name))`, join error -> `SecretError::Io`. Local, no vault test needed for D1 (fixtures have no `{{secret}}`).

## 3. Cancellation implications
- `spawn_blocking` cannot be aborted: if the `ensure_loaded` future is dropped mid-scan, the closure finishes detached; its effects are idempotent (manifest cache fill, `create_dir_all`, SQLite read). `load_guard` is released; next caller rescans.
- Post-drop state (mounts disposed, registry not yet cleared, `loaded=false`) is identical to dropping at today's existing awaits (`unmount_all`, `publish_plugin_settings`) — no new state class.
- Runtime shutdown waits for in-flight blocking scans (bounded by disk I/O).

## 4. Deterministic test (lifecycle.rs tests, temp fixture only)
Seams (cfg(test), mirroring existing `claude_home_override`/`extra_plugin_parents`):
- `DiscoveryConfig.aleph_home_override: Option<PathBuf>` honored in `DirectoryScanner::new` (scanner.rs:39) so `~/.aleph/plugins` resolves into the TempDir.
- `ExtensionManager` test field `scan_probe: Option<Arc<dyn Fn() + Send + Sync>>`, invoked at the top of `scan_plugins_blocking`.

```rust
#[tokio::test(flavor = "current_thread")]
async fn cold_ensure_loaded_scans_off_the_runtime_thread() {
    let tmp = TempDir::new()?; write_static_plugin(tmp.path(), "alpha");
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let rx = Arc::new(Mutex::new(rx)); let rt_thread = thread::current().id();
    let saw = Arc::new(AtomicBool::new(false));
    // probe: assert thread != rt_thread; rx.recv_timeout(5s).is_ok() -> saw=true
    let mgr = isolated_manager_with(tmp.path(), probe) /* scan_claude_dirs:false, scan_project_dirs:false, aleph_home_override: tmp */;
    let hb = tokio::spawn(async move { tokio::task::yield_now().await; let _ = tx.send(()); });
    mgr.ensure_loaded().await.unwrap();
    hb.await.unwrap();
    assert!(saw.load(SeqCst)); assert!(registry has "alpha");
}
```
Inline scan on a current-thread runtime makes the heartbeat task unrunnable -> `recv_timeout` fails -> deterministic red, no deadlock. Same pattern can assert the `extension.views` closure. Residual real-home reads: `user_hook_configs` uses `get_config_dir()/hooks.json` (read-only); to make it zero, pass `discovery.aleph_home()` into `load_user_hooks` (same value in production). Skill roots depend on the concurrent scan_roots fix. `ProjectStore::shared()` is in-memory under cfg(test). No env/HOME mutation.

## 5. Not safely fixable here (separate scope)
- A14 WASM: `load_plugin` (extism compile) under `plugin_loader.write()`. `write_owned()+spawn_blocking` is mechanically available (pattern at plugin_ops.rs:26), but a dropped mount future would leave a loaded module with no disposer in the `EffectScope`; `load_plugin` then "already loaded, skipping" on remount -> stale module. Needs a cancellation-safe effect handoff (detached completion registering the disposer, or a spawned non-cancellable mount task). Also holds loader lock across compile, blocking tool calls.
- Guest code in service `start_handler` / tool calls: already blocking-pool but serialized behind owned loader/service locks; extism timeout is the only bound.
- `register_memory_extension_effect` / `register_transient_servers`: in-memory/actor; not blocking.

## Not done
No code, no Cargo, no tests run. `AdapterOutput`/`PluginManifest` `Send` assumed (plain data; compiler will confirm). ExtensionManager construction (`plugins.toml` load) not on the `ensure_loaded` path and not audited.
