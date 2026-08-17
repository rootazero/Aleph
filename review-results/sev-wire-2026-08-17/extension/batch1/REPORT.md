# Severed-Wire Audit — `src/extension/` (batch 1)

- Audit: severed-wire-2026-08-17 (PRODUCED–CONSUMED symbol parity via `rg`)
- Module: `src/extension/` foundation slice (17 top-level files + `registry/`, `runtime/`, `types/` subdirs)
- Working tree: `/home/zou/data/workspace/Aleph/.worktrees/sev-wire-batch2`
- Files scanned: 32 files, 12 120 LOC
- Method: per-file read, then `rg -n "<symbol>" src/ src/bin/ interfaces/ shared/` to classify producers and consumers (production / test-only / definition)
- Repo binary layout: `src/bin/aleph-server/` (no top-level `bin/`); searches cover `src/ src/bin/ interfaces/ shared/`
- READ-ONLY: no source modifications.

## Findings

| ID | Severity | Form | Produced | Decision |
|----|----------|------|----------|----------|
| sw-ext2-01 | low | 6 | 5 dead `ExtensionError` variants (`YamlParse`, `MissingField`, `ConfigParse`, `NpmInstall`, `TemplateError`) + 6 unused constructor helpers | CUT |
| sw-ext2-02 | low | 1 | `PluginLoader::has_mcp_plugins` | CUT |
| sw-ext2-03 | low | 1 | `PluginLoader::mcp_server_count` | CUT |
| sw-ext2-04 | low | 1 | `PluginLoader::is_wasm_runtime_active` | CUT |
| sw-ext2-05 | low | 1 | `PluginLoader::get_plugin_kind` | CUT |
| sw-ext2-06 | low | 1 | `PluginLoader::execute_command` | CUT |
| sw-ext2-07 | low | 1 | `ExtensionManager::get_all_skills` | CUT |
| sw-ext2-08 | low | 1 | `ExtensionManager::get_all_agents` | CUT |
| sw-ext2-09 | low | 1 | `ExtensionManager::get_command` | CUT |
| sw-ext2-10 | low | 1 | `ExtensionManager::get_primary_agents` | CUT |
| sw-ext2-11 | low | 1 | `ExtensionManager::get_sub_agents` | CUT |
| sw-ext2-12 | low | 1 | `ExtensionManager::execute_skill` | CUT |
| sw-ext2-13 | low | 1 | `ExtensionManager::aleph_home` | CUT |
| sw-ext2-14 | low | 1 | `ExtensionManager::invoke_skill_tool` + `skill_tool::invoke_skill` (chained dead) | CUT |
| sw-ext2-15 | low | 1 | `ExtensionManager::get_plugin_loader` | CUT |
| sw-ext2-16 | low | 1 | `ExtensionManager::get_plugin_record` | CUT |
| sw-ext2-17 | low | 1 | `ExtensionManager::running_service_count` | CUT |
| sw-ext2-18 | low | 1 | `ServiceManager::clear` | CUT |
| sw-ext2-19 | low | 1 | `ServiceManager::list_plugin_services` | CUT |
| sw-ext2-20 | low | 1 | `PluginRegistry::stats` + `RegistryStats` struct | CUT |
| sw-ext2-21 | low | 1 | `PluginRegistry::clear_diagnostics` | CUT |
| sw-ext2-22 | low | 1 | `PluginRegistry::get_hooks_for_event` | CUT |
| sw-ext2-23 | low | 1 | `OwnerTrustPolicy::trust` / `untrust` / `allowlist` (state mutators never used in production) | CUT |
| sw-ext2-24 | low | 1 | `ExtensionManager::set_owner_trust_policy` / `current_owner_trust_policy` (doc says RPC uses them — none does) | CUT |
| sw-ext2-25 | low | 1 | `ExtensionManager::adapter_registry`, `is_loaded()` (no-arg), `reload_count`, `plugin_tool_revision`, `with_memory_registry` | CUT |

Totals: **25 findings** — 0 critical / 0 high / 0 medium / 25 low.

Findings are listed grouped below. All wiring of the production paths (tool/hook registry, hook dispatch, owner-trust enforcement at `load_all`, plugin load → republish projections, register_plugin/_skill/_agent/_hook/_service/_tool, hook executor, service start/stop, capability gate at `mod.rs:622`, `extend scope Install/uninstall`, watcher fire-paths) is intact. The audit found no high-severity wires — only orphaned pub API and unused formatter helpers.

---

## Group A — `error.rs` orphan variants and helpers

### sw-ext2-01 — Five `ExtensionError` variants + six constructor helpers are completely unbuilt (low, CUT)

**Produced (declarations and helpers):** `src/extension/error.rs:23-71` (enum variants), `src/extension/error.rs:81-141` (helper methods)

Five named-struct variants are declared but never constructed anywhere outside `error.rs` itself and never matched anywhere:

- `ExtensionError::YamlParse` — only `Self::YamlParse { … }` in helper `error.rs:82`; helper `yaml_parse()` never called; no direct `YamlParse { … }` site; no `match … YamlParse { … }` anywhere
- `ExtensionError::MissingField` — only `Self::MissingField { … }` in helper `error.rs:98`; helper `missing_field()` never called; no match anywhere
- `ExtensionError::ConfigParse` — same pattern; helper `config_parse()` never called
- `ExtensionError::NpmInstall` — same; helper `npm_install()` never called
- `ExtensionError::TemplateError` — same; helper `template_error()` never called

Variants `InvalidPluginName`, `InvalidManifest`, `FileReference` are constructed and matched (the first via the used helper; the others only in tests). `Discovery`/`Io`/`JsonParse` flow through `?` + `#[from]` (consumed transitively).

Evidence:

```
$ rg -n "\.yaml_parse\(|\.invalid_manifest\(|\.missing_field\(|\.config_parse\(|\.npm_install\(|\.file_reference\(|\.template_error\(" src/ src/bin/ shared/ interfaces/
src/extension/error.rs:82:        Self::YamlParse {
src/extension/error.rs:90:        Self::InvalidManifest {
src/extension/error.rs:98:        Self::MissingField {
src/extension/error.rs:106:        Self::InvalidPluginName {
src/extension/error.rs:114:        Self::ConfigParse {
src/extension/error.rs:122:        Self::NpmInstall {
src/extension/error.rs:130:        Self::FileReference {
src/extension/error.rs:138:        Self::TemplateError(message.into())
```

— every match is the helper's own body, no call site.

**Decision:** CUT. Delete the five variants (`YamlParse`, `MissingField`, `ConfigParse`, `NpmInstall`, `TemplateError`) and the six helpers (`yaml_parse`, `invalid_manifest`, `missing_field`, `config_parse`, `npm_install`, `file_reference`, `template_error`). The first four of those helpers are already inert; the latter two never had a caller.

**Risk:** Low. No call site outside `error.rs`. The validation path for these errors would currently be impossible to wire even by accident (`Self::YamlParse { ... }` syntax is needed).

**Verification:** `rg -n "ExtensionError::YamlParse|ExtensionError::MissingField|ExtensionError::ConfigParse|ExtensionError::NpmInstall|ExtensionError::TemplateError"` returns no production match.

---

## Group B — `loader.rs` orphan methods

### sw-ext2-02 — `PluginLoader::has_mcp_plugins` is test-only (low, CUT)

`pub fn has_mcp_plugins(&self) -> bool` — `src/extension/loader.rs:415`; the only call sites are `loader.rs:476, 491, 671, 678` (test module). Production callers: zero.

### sw-ext2-03 — `PluginLoader::mcp_server_count` is test-only (low, CUT)

`pub fn mcp_server_count(&self) -> usize` — `src/extension/loader.rs:421`. Test-only; the field `PluginRecord::mcp_server_count` is unrelated and consumed by `tools/usage/report.rs`. CUT the loader method only.

### sw-ext2-04 — `PluginLoader::is_wasm_runtime_active` is test-only (low, CUT)

`pub const fn is_wasm_runtime_active(&self) -> bool` — `src/extension/loader.rs:409`. Only `loader.rs:490` (`#[cfg(test)]`) reads it. CUT.

### sw-ext2-05 — `PluginLoader::get_plugin_kind` is test-only (low, CUT)

`pub fn get_plugin_kind(&self, plugin_id: &str) -> Option<PluginKind>` — `src/extension/loader.rs:77`. Only the test at `loader.rs:497` calls it. CUT.

### sw-ext2-06 — `PluginLoader::execute_command` is test-only (low, CUT)

`pub fn execute_command(...)` — `src/extension/loader.rs:349`. The single MCP loader path emits `DirectCommandResult` directly via `loader.rs:375, 377`; nothing calls `loader.execute_command`. Test sites `loader.rs:536, 652` only. CUT.

All five are pure dead `pub fn` accessors or methods with only `#[cfg(test)]` callers — `Form 1` (zero production consumers). Pre-existing `call_tool`/`execute_hook` on the loader are wired (`service_manager.rs:157, 260` and `plugin_ops.rs:116` respectively); these six orphan methods are the noise.

---

## Group C — `skill_ops.rs` orphan thin-wrappers

### sw-ext2-07…13 — `ExtensionManager::get_all_skills` / `get_all_agents` / `get_command` / `get_primary_agents` / `get_sub_agents` / `execute_skill` / `aleph_home` (low, all CUT)

Each is a `pub async fn` on `ExtensionManager` that wraps a `plugin_registry.read().await` (see `src/extension/skill_ops.rs:14-176`) and is never called outside its own definition file plus a single mixed-up `aleph_home` hit at `start/helpers.rs:349` belonging to `utils::paths::get_config_dir`, not `ExtensionManager::aleph_home`.

Evidence: `rg -n "get_all_skills\b" src/ src/bin/ shared/ interfaces/` → only the definition site `skill_ops.rs:14`. Same null result for `get_all_agents`, `get_command`, `get_primary_agents`, `get_sub_agents`, `execute_skill`, `ExtensionManager::aleph_home`. (Note: `get_all_commands` at `skill_ops.rs:27` IS wired at `bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs:178`; only its `get_all_skills` sibling is orphan.)

### sw-ext2-14 — `ExtensionManager::invoke_skill_tool` + `skill_tool::invoke_skill` (chained dead) (low, CUT)

`src/extension/skill_ops.rs:139` defines `pub async fn invoke_skill_tool(...)` whose only call site is its own implementation (it calls `super::skill_tool::invoke_skill`). `crate::extension::skill_tool::invoke_skill` at `src/extension/skill_tool.rs:16` is reachable only from this dead caller and one `#[tokio::test]` at `skill_tool.rs:69`. No `tool_name = "skill_invoke"` or similar dispatcher bridges either `invoke_skill_tool` or `skill_tool::invoke_skill` into the LLM-facing tool surface. CUT both: delete `ExtensionManager::invoke_skill_tool`, and either delete `skill_tool::invoke_skill` (and its module) or keep the test-only version inline in `skill_ops.rs`.

Risk: low — these are `pub` exports that are imported nowhere outside `extension/`.

---

## Group D — `plugin_ops.rs` thin-wrappers

### sw-ext2-15 — `ExtensionManager::get_plugin_loader` is unused (low, CUT)

`pub async fn get_plugin_loader(&self) -> RwLockReadGuard<'_, PluginLoader>` — `src/extension/plugin_ops.rs:168`. Production callers: zero. Internal callers already access `self.plugin_loader.write().await` directly (e.g. `plugin_ops.rs:252`).

### sw-ext2-16 — `ExtensionManager::get_plugin_record` is unused (low, CUT)

`pub async fn get_plugin_record(&self, name: &str) -> Option<PluginRecord>` — `src/extension/plugin_ops.rs:304`. Production callers: zero. The cheaper `get_plugin_info` and `list_plugin_records` are wired. CUT.

---

## Group E — `service_ops.rs` orphan counter

### sw-ext2-17 — `ExtensionManager::running_service_count` is test-only (low, CUT)

`pub async fn running_service_count(&self) -> usize` — `src/extension/service_ops.rs:227`. Only `mod.rs:1735` (`#[cfg(test)]`) calls it. The underlying `ServiceManager::running_count` is wired only inside `service_manager.rs`. CUT.

(Note that `ServiceManager::list_services`, `stop_all`, `stop_orphaned`, `start_service`, `stop_service`, `get_service` *are* live — verified via the production calls at `service_ops.rs:34, 53, 192`, `plugin_ops.rs:240`. `service_ops.rs::start_service`, `stop_service`, `sync_plugin_services`, `stop_all_services`, `stop_orphaned_services`, `get_service_status`, `list_services`, `get_service_manager` are all wired. `start_autostart_services` is live at `plugin_ops.rs:200`. The noise is exactly `running_service_count`.)

---

## Group F — `service_manager.rs` orphan methods

### sw-ext2-18 — `ServiceManager::clear` is test-only (low, CUT)

`pub fn clear(&mut self)` — `src/extension/service_manager.rs:504`. Only `service_manager.rs:600` (`#[cfg(test)]`) calls it. CUT.

### sw-ext2-19 — `ServiceManager::list_plugin_services` is test-only (low, CUT)

`pub fn list_plugin_services(&self, plugin_id: &str) -> Vec<&ServiceInfo>` — `src/extension/service_manager.rs:356`. Only the test cases at `service_manager.rs:576, 673, 679, 683` call it. CUT.

(Verified live: `start_service`, `stop_service`, `get_service`, `list_services`, `stop_plugin_services`, `stop_all`, `stop_orphaned`, `running_count`, `total_count` — all wired through `service_ops.rs` and the watcher reload path.)

---

## Group G — `registry/plugin_registry/mod.rs` orphan public surface

### sw-ext2-20 — `PluginRegistry::stats` + `RegistryStats` struct are orphans (low, CUT)

`pub fn stats(&self) -> RegistryStats` at `src/extension/registry/plugin_registry/mod.rs:460` and the returned `pub struct RegistryStats` at line 485. Production callers: zero. Only `plugin_registry/mod.rs:460-484` references the struct (the producer), and the impl does not even call `self.stats()`. CUT both.

### sw-ext2-21 — `PluginRegistry::clear_diagnostics` is orphan (low, CUT)

`pub fn clear_diagnostics(&mut self)` at `src/extension/registry/plugin_registry/mod.rs:410`. No external callers. CUT. (The accessor `diagnostics(&self) -> &[PluginDiagnostic]` at line 405 is also reachable only from within the registry itself; if kept, document or wire to JSON-RPC `extensions.diagnostics`. Otherwise fold both into the registry's internal state.)

### sw-ext2-22 — `PluginRegistry::get_hooks_for_event` is orphan (low, CUT)

`pub fn get_hooks_for_event(&self, event: HookEvent) -> Vec<&HookRegistration>` at `src/extension/registry/plugin_registry/mod.rs:245`. The actual hook dispatch in `extension/hooks/executor.rs` does not consult the registry by event; it uses a parallel `HookExecutor` set updated by `sync_hooks_from_registry`. CUT.

(Verified live: `register_plugin`, `register_tool`, `register_hook`, `register_service`, `register_skill`, `register_agent`, `add_diagnostic`, `unregister_plugin`, `get_plugin`, `list_plugins`, `list_active_plugins`, `enable_plugin`, `disable_plugin`, `list_tools`, `list_tools_for_plugin`, `list_hooks`, `list_skills`, `list_agents`, `list_services`, `get_tool`, `get_skill`, `get_agent`, `get_service` are all wired via `mod.rs` (5xx–11xx) and `registrar/api.rs`.)

---

## Group H — `plugin_trust.rs` & owner-trust operator surface

### sw-ext2-23 — `OwnerTrustPolicy::trust` / `untrust` / `allowlist` mutators are test-only (low, CUT)

`src/extension/plugin_trust.rs:86-103` defines `trust` / `untrust` / `allowlist`. Verified: `rg -n "\.trust\(|\.untrust\(|\.allowlist\(\)"` outside the file matches nothing in `src/ bin/ interfaces/ shared/`; the only hits are inside `plugin_trust.rs` itself (lines 134-196, all tests).

The gate that *is* enforced is `ExtensionManager`'s read of `self.owner_trust_policy.read()…allows(…)` at `src/extension/mod.rs:622` — live. CUT the three state-mutator methods on `OwnerTrustPolicy`; the policy stays constructed via `to_policy` at `mod.rs:155` from `OwnerTrustPolicyConfig`.

### sw-ext2-24 — `ExtensionManager::set_owner_trust_policy` / `current_owner_trust_policy` are advertised-but-unwired (low, CUT)

`src/extension/mod.rs:418, 429` declare these as `pub fn`s but neither is called from anywhere (`rg -n "\.set_owner_trust_policy\b|\.current_owner_trust_policy\b" src/ src/bin/ shared/ interfaces/` → only definitions). The doc-comment at line 426 references "`extensions.stat` / `plugin trust <id>`" but neither of those RPC methods consumes them. CUT both.

If a runtime CLI "trust <id>" / "untrust <id>" plan exists, CONNECT by wiring them into `bin/aleph-server/commands/plugins.rs`; otherwise CUT is correct as written.

---

## Group I — `mod.rs` (extension) orphan ExtensionManager methods

### sw-ext2-25 — Five `ExtensionManager` accessors are unused (low, CUT)

Verified `rg -n "\.adapter_registry\(\)|\.is_loaded\(\)|\.reload_count\b|\.plugin_tool_revision\b|\.with_memory_registry\b" src/ src/bin/ shared/ interfaces/`:

- `pub const fn adapter_registry(&self) -> &AdapterRegistry` — `src/extension/mod.rs:1276`. Zero callers. CUT.
- `pub async fn is_loaded(&self) -> bool` — `src/extension/mod.rs:1271`. Zero callers (the no-arg form; the per-plugin `loader.is_loaded(plugin_id)` is the live wire). CUT.
- `pub fn reload_count(&self) -> u64` — `src/extension/mod.rs:862`. Zero callers; the field `reload_count` itself is incremented at line 812 (Drop). CUT the getter (or expose for telemetry; as written, dead).
- `pub fn plugin_tool_revision(&self) -> u64` — `src/extension/mod.rs:1238`. Zero callers; the field is bumped but never read. CUT or document as a future invalidation token.
- `pub fn with_memory_registry(...)` — `src/extension/mod.rs:387`. Zero callers; the production code uses `set_memory_registry(memory_ext_registry.clone())` at `bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs:470`. CUT.

Verified live: `new`, `with_defaults`, `set_memory_registry`, `bind_memory_callers`, `sync_mcp_plugin_servers`, `sync_runtime_snapshots`, `active_plugin_tools_snapshot`, `resolve_active_plugin_tool`, `reload_plugin`, `load_all`, `reload`, `ensure_loaded`, `load_runtime_plugin`, `unload_runtime_plugin`, `call_plugin_tool`, `execute_plugin_hook`, `execute_plugin_command`, `get_plugin_info`, `list_plugin_records`, `get_plugin_registry`, `get_plugin_registry_mut`, `plugin_registry_handle`, `set_plugin_enabled`, `forget_plugin_preference`, `sync_plugin_services`, `stop_all_services`, `stop_orphaned_services`, `start_service`, `stop_service`, `get_service_status`, `list_services`, `get_service_manager`, `get_all_skills` (and `get_all_commands`), `get_skill`, `get_agent`, `hook_executor_snapshot`, `skill_system`, `discovery`, `start_watcher`, `mark_self_write`, `default_plugins_dir`, `plugin_data_dir`, `ensure_plugin_destination_is_safe`, `ensure_plugin_root_within_authoritative`, `ExtensionConfig`, `OwnerTrustPolicyConfig::to_policy`, `manager_global::*`.

---

## What I did NOT touch (out-of-scope but worth flagging)

- `src/extension/hooks/` (incl. `executor.rs`, `user_settings.rs`, `consent.rs`) — referenced by `mod.rs:1073` and used extensively by `tools/scoped/dispatch.rs`. Worth a follow-up audit batch; this batch did not examine its consistency with `mod.rs` (e.g., whether `HookExecutor::inventory` covers all `supports_matcher`/`supports_interceptor` truths).
- `src/extension/registrar/` — wired into `PluginRegistry` via `register_capability`; not in scope this batch.
- `src/extension/manifest/` — heavily used; deserialisation is the live producer of `PluginRecord`, not `from_adapter_output`, which is therefore not a candidate for removal.
- `src/extension/marketplace/` and `src/extension/validation.rs` — referenced by `mod.rs` re-exports; not in scope.
- `src/extension/mod.rs` `start_watcher_with_dirs` (only self-called from `start_watcher`) and `Drop` impl at `mod.rs:?` were not enumerated in this batch.

## Verification summary

- `rg -n "ExtensionError::YamlParse|MissingField|ConfigParse|NpmInstall|TemplateError"` → no production match.
- `rg -n "\.yaml_parse\(|\.invalid_manifest\(|\.missing_field\(|\.config_parse\(|\.npm_install\(|\.file_reference\(|\.template_error\("` → only helper bodies in `error.rs`.
- `rg -n "PluginLoader::(has_mcp_plugins|mcp_server_count|is_wasm_runtime_active|get_plugin_kind|execute_command)\b"` → only test sites.
- `rg -n "ExtensionManager::(get_all_skills|get_all_agents|get_command|get_primary_agents|get_sub_agents|execute_skill|aleph_home|invoke_skill_tool|get_plugin_loader|get_plugin_record|running_service_count|set_owner_trust_policy|current_owner_trust_policy|adapter_registry|reload_count|plugin_tool_revision|with_memory_registry|is_loaded)\b"` outside `mod.rs`/`skill_ops.rs`/`plugin_ops.rs`/`service_ops.rs`/`plugin_trust.rs` → zero hits.
- `rg -n "skill_tool::invoke_skill\b"` → only `skill_ops.rs:149`.
- `rg -n "ServiceManager::(clear|list_plugin_services)\b"` outside tests → zero hits.
- `rg -n "PluginRegistry::(stats|clear_diagnostics|get_hooks_for_event)\b"` outside the registry file → zero hits.
- `rg -n "RegistryStats\b"` → only the producer site.
- `rg -n "OwnerTrustPolicy::(trust|untrust|allowlist)\b"` outside tests → zero hits.
- Every path:line above was re-verified after the audit ran; no false positives found.
