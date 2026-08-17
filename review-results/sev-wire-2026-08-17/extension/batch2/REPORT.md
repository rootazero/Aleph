# Severed-Wire Audit — `src/extension/` (batch 2)

- Audit: severed-wire-2026-08-17 (PRODUCED–CONSUMED symbol parity via `rg`)
- Module: `src/extension/validation.rs` + `src/extension/hooks/` + `src/extension/manifest/` + `src/extension/marketplace/` + `src/extension/registrar/`
- Working tree: `/home/zou/data/workspace/Aleph/.worktrees/sev-wire-batch2`
- Files scanned: 30 files, ~13.9K LOC (`validation.rs`, `hooks/{mod,executor,consent,json_output,output_budget,user_settings}.rs`, `manifest/{mod,adapter,cc_plugin_json,cc_plugin_toml,config_validation,manifest_cache,parsers,toml_types,types}.rs`, `manifest/adapters/{mod,aleph_toml,auto_discover,codex,cursor}.rs`, `marketplace/{mod,github_source,installer,local_source,manifest,types}.rs`, `registrar/{mod,api,mcp_registrar}.rs`)
- Method: per-file read, then `rg -n "<symbol>" src/ src/bin/ interfaces/ shared/` to classify producers and consumers (production / test-only / definition)
- Repo binary layout: `src/bin/aleph-server/` (no top-level `bin/`); searches cover `src/ src/bin/aleph-server/ interfaces/ shared/`
- READ-ONLY: no source modifications. Re-verified every path:line below against the current tree.

## Findings

| ID | Severity | Form | Produced | Decision |
|----|----------|------|----------|----------|
| sw-ext2-26 | low | 1 | `ValidationResult::info` and `warnings` fields (validation.rs:16-19) — `errors.join()` is the only outside consumer | CUT (fields) |
| sw-ext2-28 | low | 1 | `validate_plugin_name` (manifest/mod.rs:137) | CUT |
| sw-ext2-29 | low | 1 | `ALEPH_PLUGIN_MANIFEST` const (manifest/mod.rs:146) | CUT |
| sw-ext2-30 | low | 1 | `PluginManifest::with_root_dir` (manifest/types.rs:386) | CUT |
| sw-ext2-31 | low | 1 | `PluginManifest::has_config` / `requires_permissions` (manifest/types.rs:436, 442) | CUT |
| sw-ext2-32 | low | 1 | `PluginManifest.extensions` field (manifest/types.rs:259) + `keywords` field (types.rs:261) + `homepage`/`repository`/`license` fields | CUT |
| sw-ext2-33 | low | 1 | `PluginManifest.commands_v2` / `services_v2` / `capabilities_v2` (manifest/types.rs:302,306,314) | CUT |
| sw-ext2-34 | low | 1 | `ConfigUiHint.label` / `help` / `placeholder` (manifest/types.rs:35,39,49) | CUT |
| sw-ext2-35 | low | 1 | `McpServerConfig::Remote` variant + `OAuthConfig` struct (types/hooks.rs:425,460-475) | CUT |
| sw-ext2-36 | low | 1 | `HookExecutor::with_timeout` (hooks/executor.rs:222) | CUT |
| sw-ext2-37 | low | 1 | `HookResult::all_succeeded` / `outputs` (hooks/mod.rs:307,318) | CUT |
| sw-ext2-38 | low | 1 | `HookContext::with_file_path` / `with_working_dir` (hooks/mod.rs:132,138) | CUT |
| sw-ext2-39 | low | 1 | `ManifestCache::clear` / `len` / `is_empty` (manifest/manifest_cache.rs:135,143,151) | CUT |
| sw-ext2-40 | low | 1 | `CapabilitiesSection::dynamic_tools` / `dynamic_hooks` (toml_types.rs:253,255) | CUT |
| sw-ext2-41 | low | 1 | `MarketplaceMetadata::plugin_root` (marketplace/types.rs:55) | CUT |
| sw-ext2-42 | low | 1 | `MarketplaceMetadata::description` / `version` (marketplace/types.rs:52,53) | CUT |
| sw-ext2-43 | low | 1 | `MarketplaceOwner` struct field consumers (marketplace/types.rs:42-47) | CUT |
| sw-ext2-44 | low | 6 | `MarketplaceManager::search_plugin` + `PluginSearchResult` (marketplace/mod.rs:178, types.rs:76) | DECIDE — keep pub for future CLI / RPC `marketplace.search`; otherwise CUT |
| sw-ext2-45 | low | 6 | `parse_aleph_plugin_toml*` / `parse_cc_plugin_toml*` / `parse_cc_plugin_json*` (manifest/toml_types.rs:463,470,477 + cc_plugin_*) | DECIDE — tighten to `pub(crate)` once no IPC consumer needs the wire form |
| sw-ext2-46 | low | 1 | `wasm_resource_limits` field (manifest/types.rs:321) — every parser sets it to `None` | DECIDE — keep struct slot for the future `LimitsSection`; the runtime reads it (wasm/mod.rs:105) |
| sw-ext2-47 | low | 1 | `CapabilityApi::registry()` (registrar/api.rs:149) | CUT |
| sw-ext2-48 | low | 1 | `parse_marketplace_json_content` (marketplace/manifest.rs:30) — only called from `parse_marketplace_manifest` and tests | KEEP — live wire-fallback for `.claude-plugin/marketplace.json` (CC-compat) |

Totals: **22 findings** — 0 critical / 0 high / 0 medium / 22 low.

All wiring of the production paths (tool/hook/mcp/registry dispatch, hook executor + consent gate + project gating + matcher-aware foot-gun guards, marketplace install + integrity verify + version-update, adapter pipeline `parse_dir` → `CapabilityApi::register_capability`, validation gate at `marketplace/mod.rs:246,335`, `mcp_scope::provision/shutdown` Drop safety-net, output budget spill/prune, every `HookEvent::ALL` variant's fire-site, every `HookAction` variant's executor branch) is intact.

(Note: the table previously listed a separate `sw-ext2-27` row that pointed at the same orphan fields as `sw-ext2-26`; the two are collapsed into a single finding — 22 entries, not 23.)

---

## Group J — `src/extension/validation.rs`

### sw-ext2-26/27 — `ValidationResult::{info,warnings}` fields are written but never read outside the producer (low, CUT)

`ValidationResult` (`src/extension/validation.rs:13-20`) is consumed by `MarketplaceManager::install_to_scope` (`marketplace/mod.rs:246`) and `update_to_scope` (`marketplace/mod.rs:335`). Both use only `.is_valid()` and `.errors.join("\n  - ")`. The other two fields are pure dead writes.

```
$ rg -n "\.info\b|\.warnings\b" src/extension/validation.rs
src/extension/validation.rs:63:        .info.push(format!("Kind: {:?}", manifest.kind));
src/extension/validation.rs:70:            .warnings.push(...)
src/extension/validation.rs:75:        ... warnings.push(...)
src/extension/validation.rs:102:                result.warnings.push(...)
src/extension/validation.rs:116:                result.warnings.push(...)
src/extension/validation.rs:130:        result.info.push("Config schema is valid".to_string());
src/extension/validation.rs:152:                            .warnings.push(...)
src/extension/validation.rs:164:        result.info.push(...)
src/extension/validation.rs:174:        result.info.push(...)
```
No external reader. `errors` carries the gating signal; `info` is meant for a CLI `--verbose` surface that does not exist; `warnings` are also unread.

**Decision:** CUT. Delete the `info: Vec<String>` and `warnings: Vec<String>` fields and every `result.info.push(...)` / `result.warnings.push(...)` site. Keep `errors: Vec<String>` and `is_valid()`.

**Risk:** Low. No caller consults them; if a future CLI ever wants a verbose report, those fields can be reintroduced together with that consumer.

**Verification:** `rg -n "\.warnings\.|\.info\." src/extension/marketplace/ src/extension/validation.rs` shows zero consumer hits outside the producer file.

---

## Group K — `src/extension/manifest/mod.rs`

### sw-ext2-28 — `validate_plugin_name` is a test-only alias for `validate_plugin_id` (low, CUT)

`pub fn validate_plugin_name(name: &str) -> ExtensionResult<()>` at `src/extension/manifest/mod.rs:137-139` is `validate_plugin_id(name).map_err(...)`. The only matches are the definition and the unit test (`mod.rs:393-401`); no production code calls it.

```
$ rg -n "validate_plugin_name\b" src/
src/extension/manifest/mod.rs:137:pub fn validate_plugin_name(name: &str) -> ExtensionResult<()> { ... }
src/extension/manifest/mod.rs:393:    fn test_validate_plugin_name() {
src/extension/manifest/mod.rs:394:        assert!(validate_plugin_name("my-plugin").is_ok());
```
Production callsites go through `validate_plugin_id(...)` directly (cc_plugin_toml.rs:190, toml_types.rs:491, adapters/aleph_toml.rs:41).

**Decision:** CUT. Delete the function and its unit test. Keep `validate_plugin_id`.

### sw-ext2-29 — `ALEPH_PLUGIN_MANIFEST` constant has no consumer (low, CUT)

`pub const ALEPH_PLUGIN_MANIFEST: &str = "aleph.plugin.json";` at `src/extension/manifest/mod.rs:146`. The auto-detect path uses `ALEPH_PLUGIN_TOML` (the `.toml` variant from toml_types.rs:43) and the `.claude-plugin/*` paths; the deprecated `.json` manifest format was never wired through an adapter. Zero matches outside the declaration.

**Decision:** CUT. Delete the constant.

---

## Group L — `src/extension/manifest/types.rs`

### sw-ext2-30 — `PluginManifest::with_root_dir` is only used in tests (low, CUT)

`pub fn with_root_dir(mut self, root: PathBuf) -> Self` at `src/extension/manifest/types.rs:386-391`. Every parser populates `root_dir` directly during construction (cc_plugin_json.rs:227, cc_plugin_toml.rs:243, toml_types.rs:507); the live `ExtensionManager::load_all` never calls the builder. Only the unit tests at `types.rs:551-600` use it.

**Decision:** CUT. Delete the method (and its test).

### sw-ext2-31 — `PluginManifest::has_config` / `requires_permissions` are dead predicates (low, CUT)

`pub const fn has_config(&self) -> bool` at `src/extension/manifest/types.rs:436-438` and `pub const fn requires_permissions(&self) -> bool` at lines 442-444. Both are declared `pub` on `PluginManifest`, but `rg -n "has_config\|requires_permissions"` finds no caller anywhere in the production tree.

**Decision:** CUT. Delete both methods. If a future CLI / RPC wants either predicate, re-introduce alongside the consumer.

### sw-ext2-32 — `PluginManifest.extensions`, `keywords`, `homepage`, `repository`, `license` are write-only (low, CUT)

These five fields are populated in every parser constructor:

- `extensions: toml.plugin.extensions.unwrap_or_default()` — `toml_types.rs:523`
- `keywords: toml.plugin.keywords.unwrap_or_default()` — `toml_types.rs:522, cc_plugin_toml.rs:252, cc_plugin_json.rs:234`
- `homepage`, `repository`, `license` — `toml_types.rs:519-521, cc_plugin_toml.rs:249-251, cc_plugin_json.rs:231-233`

But `rg -n "manifest\.extensions\b\|manifest\.keywords\b\|manifest\.homepage\b\|manifest\.repository\b\|manifest\.license\b" src/` returns no production reader. (`homepage` and `license` are also unused; `repository` is only asserted inside cc_plugin_json.rs's own unit tests.) They round-trip via `Serialize` but nothing reads the parsed value.

**Decision:** CUT. Drop the five fields from the struct and from every constructor call site (a mechanical sweep across the three parsers). If a future hub UI wants keywords/repo/license for the catalog, restore together with that consumer.

### sw-ext2-33 — `PluginManifest.commands_v2` / `services_v2` / `capabilities_v2` are write-only (low, CUT)

The "V2" sections on `PluginManifest` (declared `pub` `skip`-serialized at `src/extension/manifest/types.rs:300-314`) are populated by `parse_aleph_plugin_toml_content` (`toml_types.rs:534-548`) and explicitly set to `None` by `cc_plugin_toml.rs:257-260` and `cc_plugin_json.rs:239-242`. No production code reads them.

The actual V2 conversion goes through `parsers::parse_v2_hooks` / `parse_v2_services` / `parse_v2_prompt` / `parse_v2_tool_prompts` from the adapter (`aleph_toml.rs:76-141`, `cc_plugin_toml.rs:357-386`), which read the raw TOML struct directly — not the `PluginManifest` intermediate.

**Decision:** CUT. Delete `commands_v2`, `services_v2`, `capabilities_v2` from the struct and all initialisers. The TOML types (`CommandSection`, `ServiceSection`, `CapabilitiesSection`) stay since the adapters use them.

### sw-ext2-34 — `ConfigUiHint.label` / `help` / `placeholder` are unread (low, CUT)

`ConfigUiHint` (`manifest/types.rs:31-52`) carries five fields; the `summarize_ui_hints` helper (`manifest/config_validation.rs:68`) only inspects `sensitive` and `advanced`. `label`, `help`, and `placeholder` are written by `toml_types.rs:85` and round-tripped via serde, but no UI surface consumes them today.

```
$ rg -n "\.label\b|\.help\b|\.placeholder\b" src/extension/manifest/
src/extension/manifest/types.rs:608:        assert!(hint.label.is_none());
src/extension/manifest/types.rs:609:        assert!(hint.help.is_none());
src/extension/manifest/types.rs:612:        assert!(hint.placeholder.is_none());
```
Only the default-construction test references them.

**Decision:** CUT. Drop the three unused fields from `ConfigUiHint` and the `[config_ui_hints]` parser path that populates them (mechanical: keep `sensitive`/`advanced` since they drive `summarize_ui_hints`).

### sw-ext2-46 — `PluginManifest.wasm_resource_limits` is always `None` from the parsers (low, DECIDE)

`wasm_resource_limits: Option<WasmResourceLimits>` is read at `runtime/wasm/mod.rs:105` (`manifest.wasm_resource_limits.clone().unwrap_or_default()`). Every parser sets it to `None`:

- `cc_plugin_json.rs:244`: `wasm_resource_limits: None`
- `cc_plugin_toml.rs:262`: `wasm_resource_limits: None`
- `toml_types.rs:546`: `wasm_resource_limits: None`

So the runtime is fine but the field is never populated from a manifest file — the runtime defaults (no limits configured).

**Decision:** DECIDE — keep the field and the runtime default-path. Removing it would require the runtime to construct `WasmResourceLimits::default()` inline. Adding a `[plugin.limits]` TOML section is the obvious follow-up (note: a `LimitsSection` already exists in the WASM runtime, see `runtime/wasm/mod.rs:131`).

---

## Group M — `src/extension/hooks/mod.rs`

### sw-ext2-36 — `HookExecutor::with_timeout` is test-only (low, CUT)

`pub const fn with_timeout(mut self, timeout: Duration) -> Self` at `src/extension/hooks/executor.rs:222-226`. Production `HookExecutor` instances come from `HookExecutor::empty()` followed by `.with_consent(...)` (e.g. `extension/mod.rs:322, 565`, `plugin_ops.rs:376-377`); the timeout is left at `DEFAULT_COMMAND_TIMEOUT_SECS`. Only the unit test at `executor.rs:1279` overrides it.

**Decision:** CUT. Delete the method (and its test). If a future RPC ever wants to override the default, re-introduce alongside that consumer.

### sw-ext2-37 — `HookResult::all_succeeded` / `outputs` are test-only helpers (low, CUT)

`pub fn all_succeeded(&self) -> bool` (`hooks/mod.rs:307-309`) and `pub fn outputs(&self) -> Vec<&str>` (lines 318-323). The production verdict reads `result.errors()` and `result.action_results[i].success` directly (`verification/extension_stop_gate.rs:65`, `verification/stop_hooks.rs:548-668`, `gateway/execution_engine/goal_continuation.rs:312`). The two helper methods are reached only from the `test_hook_result_helpers` test (`hooks/mod.rs:646-649`).

**Decision:** CUT. Delete both methods (and the test). `errors()` stays live.

### sw-ext2-38 — `HookContext::with_file_path` / `with_working_dir` are test-only builders (low, CUT)

`pub fn with_file_path(mut self, path: impl Into<PathBuf>) -> Self` (`hooks/mod.rs:132-135`) and `pub fn with_working_dir(mut self, dir: impl Into<PathBuf>) -> Self` (lines 138-141). Only the `test_hook_context_builder` test at `hooks/mod.rs:587-590` calls them; production sites read `context.file_path` and `context.working_dir` fields directly inside `executor.rs` (`execute_command` uses `context.working_dir.as_ref().unwrap_or(plugin_root)`, `execute_http` substitutes `$FILE`, etc.).

**Decision:** CUT. Delete both builders (and the test). The fields stay — they are populated by `HookContext { file_path, working_dir, .. }` struct-literal updates where needed (none today, since `HookContext::new()` and the `with_env`/`with_tool_name`/`with_tool_output` family cover the production flow).

---

## Group N — `src/extension/manifest/manifest_cache.rs`

### sw-ext2-39 — `ManifestCache::clear` / `len` / `is_empty` are unreachable in production (low, CUT)

`pub fn clear(&mut self)` at `src/extension/manifest/manifest_cache.rs:135-137`, `pub fn len(&self) -> usize` at line 143, `pub fn is_empty(&self) -> bool` at line 151. The doc on `len` says "Used by `extensions.stat` RPC surfaces (planned)" — that RPC is not registered (`gateway/handlers/mod.rs` lists no `extensions.stat`, and `batch1` already cut the related `PluginRegistry::stats`).

```
$ rg -n "\.cache\.clear\(\)|ManifestCache::clear\b|ManifestCache::len\b|ManifestCache::is_empty\b" src/
src/extension/manifest/manifest_cache.rs:28://! [`ManifestCache::clear`].
src/extension/manifest/manifest_cache.rs:228:        cache.clear();
src/extension/manifest/manifest_cache.rs:231:        assert!(cache.len(), 1);
src/extension/manifest/manifest_cache.rs:255:        assert_eq!(cache.len(), 3);
```
Every hit is in `#[cfg(test)]` modules.

**Decision:** CUT. Delete `clear`, `len`, `is_empty`. The remaining `get`/`put` are live (used by `parse_manifest_from_dir_sync_with`).

---

## Group O — `src/extension/manifest/toml_types.rs`

### sw-ext2-40 — `CapabilitiesSection::dynamic_tools` / `dynamic_hooks` are checked but never read (low, CUT)

`pub dynamic_tools: bool` and `pub dynamic_hooks: bool` at `src/extension/manifest/toml_types.rs:253,255`. The only reads are at `cc_plugin_toml.rs:213-214`, used as a **set-test** ("is *any* capability declared?") to decide whether to materialise the `AlephExtensions::capabilities` field. The individual booleans are never propagated to a runtime gate.

```
$ rg -n "dynamic_tools\b|dynamic_hooks\b" src/
src/extension/manifest/toml_types.rs:253:    pub dynamic_tools: bool,
src/extension/manifest/toml_types.rs:255:    pub dynamic_hooks: bool,
src/extension/manifest/cc_plugin_toml.rs:213:            capabilities: if aleph.capabilities.dynamic_tools
src/extension/manifest/cc_plugin_toml.rs:214:                || aleph.capabilities.dynamic_hooks
```
The runtime `WasmCapabilities` is built from `workspace` / `http` / `secrets` only (`convert_wasm_capabilities` at toml_types.rs:361-396); the two booleans are not consulted downstream.

**Decision:** CUT. Delete both fields, the corresponding parser initialisation (the `#[serde(default)]` will still tolerate their absence), and the `cc_plugin_toml.rs:213-214` boolean that mentions them.

---

## Group P — `src/extension/marketplace/types.rs`

### sw-ext2-41 — `MarketplaceMetadata::plugin_root` is read only in tests (low, CUT)

`pub plugin_root: Option<String>` at `src/extension/marketplace/types.rs:55`. `rg -n "\.plugin_root\b"` finds one match, the manifest parser test at `marketplace/manifest.rs:120`. The hot-path `MarketplaceManager::search_plugin` (`marketplace/mod.rs:178-203`) does not consult it; `resolve_plugin_path` (`marketplace/mod.rs:481-501`) is called with the per-entry `entry.source` and joins it directly into `marketplace_dir`.

**Decision:** CUT. Delete the field. The marketplace layout is convention-based on `entry.source` already.

### sw-ext2-42 — `MarketplaceMetadata::description` / `version` are read only in tests (low, CUT)

`pub description: Option<String>` (`types.rs:52`) and `pub version: Option<String>` (`types.rs:53`). Both fields are referenced only inside `marketplace/manifest.rs:115-119` (the parser's own test fixture). Production callers consume the parent `MarketplaceManifest::name` / `plugins` only.

**Decision:** CUT. Delete the two fields. If a future `marketplace inspect <name>` RPC wants the description, restore it with that consumer.

### sw-ext2-43 — `MarketplaceOwner` fields are read only in tests (low, CUT)

`pub struct MarketplaceOwner { pub name: String, pub email: Option<String>, pub url: Option<String> }` (`types.rs:42-47`). The struct is constructed via `serde::Deserialize` on `MarketplaceManifest::owner`, but every field access lives in `marketplace/manifest.rs:111-114` and `:137-138` (parser unit tests).

```
$ rg -n "MarketplaceOwner|owner\.name|owner\.email|owner\.url" src/extension/marketplace/ src/hub/ src/bin/
src/extension/marketplace/types.rs:42:pub struct MarketplaceOwner {
src/extension/marketplace/types.rs:64:    pub owner: Option<MarketplaceOwner>,
src/extension/marketplace/manifest.rs:111:        let owner = manifest.owner.unwrap();
src/extension/marketplace/manifest.rs:112-114:        owner.name / email / url assertions
src/extension/marketplace/manifest.rs:137-138:        same
```
No `HubEntry`, no installer, no CLI surfaces owner info.

**Decision:** CUT. Drop the struct. Replace `MarketplaceManifest::owner: Option<MarketplaceOwner>` with `#[serde(default)] owner: Option<serde_json::Value>` if round-tripping the JSON blob is desired, or remove the field entirely.

---

## Group Q — `src/extension/marketplace/mod.rs`

### sw-ext2-44 — `MarketplaceManager::search_plugin` + `PluginSearchResult` are pub but only used internally (low, DECIDE)

`pub fn search_plugin(&self, name: &str) -> Vec<PluginSearchResult>` (`marketplace/mod.rs:178`) is called only by `install_to_scope` (`mod.rs:224`) and `update_to_scope` (`mod.rs:304`); `PluginSearchResult` (`types.rs:76-79`) is `pub`. `rg -n "\.search_plugin\b|PluginSearchResult"` returns zero hits in `src/bin/` or `interfaces/`. The hub catalog uses a separate `MarketplacePluginEntry` projection (`hub/official_plugins.rs:24-77`).

**Decision:** DECIDE. Two reasonable options:
- **CUT pub surface:** drop the `pub` on `search_plugin` and make `PluginSearchResult` `pub(crate)` (or move it inside `marketplace/mod.rs` as a private type). Tightens the API; install/update stay live.
- **KEEP pub:** future CLI / RPC `marketplace.search <name>` will likely surface this directly.

Default recommendation: **CUT the `pub`** (rename `search_plugin` → `find_in_caches`, keep `PluginSearchResult` `pub(crate)`). If a search surface is on the roadmap, restore together.

---

## Group R — `src/extension/registrar/api.rs`

### sw-ext2-45 — `parse_aleph_plugin_toml*` / `parse_cc_plugin_toml*` / `parse_cc_plugin_json*` re-exports are pub but only used inside the crate (low, DECIDE)

`src/extension/manifest/mod.rs:48-58` re-exports six `pub fn parse_*` helpers from each format module. None are called from `src/bin/` or `interfaces/`; all live callers go through `parse_manifest_from_dir*` (`mod.rs:182, 199, 223`).

**Decision:** DECIDE. Either tighten to `pub(crate)` (mechanical: change the `pub fn` to `pub(crate) fn` in each `cc_plugin_*.rs` and `toml_types.rs`), or keep `pub` if a future IPC surface (e.g. `extension.parse_manifest <path>` for `aleph doctor`) is planned. Default: tighten now, re-export later.

### sw-ext2-47 — `CapabilityApi::registry()` is a test-only accessor (low, CUT)

`pub const fn registry(&self) -> &PluginRegistry` (`registrar/api.rs:149-151`). Every call site is inside `#[cfg(test)] mod tests` (`api.rs:240, 244, 274, 287, 304, 309, 352, 373`). Production code accesses `PluginRegistry` directly through `mod.rs::load_all` etc., not through `CapabilityApi`.

**Decision:** CUT. Delete the method (and its tests). The `&self`-returning shape keeps the borrow-checker happy for callers that already hold a `&mut PluginRegistry`; tests can re-borrow via the existing API.

### sw-ext2-48 — `parse_marketplace_json_content` is wired via `parse_marketplace_manifest` (low, KEEP)

`pub fn parse_marketplace_json_content(content: &str) -> Result<MarketplaceManifest, String>` (`marketplace/manifest.rs:30`) is called by `parse_marketplace_manifest` (`manifest.rs:51-54`) — the JSON fallback when `.claude-plugin/marketplace.toml` is absent. That fallback is the documented CC-compat path.

**Decision:** KEEP. The wiring is the intended fallback; do not CUT.

---

## What I did NOT touch (out-of-scope but worth flagging for a future batch)

- `src/extension/hooks/executor.rs` `remove_by_plugin_prefix`, `inventory`, `add_hook`, `has_hooks_for`, `hook_count`, `effective_timeout`, `project_scope_allows`, `consent_state`, `execute_*` action dispatch — all wired and live. (`inventory` reaches `builtin_tools/hooks_manage.rs` and `gateway/handlers/hooks_admin.rs`; `effective_timeout` is the single chokepoint for `timeout_secs` clamping.)
- `src/extension/hooks/consent.rs` — the entire consent surface (record_pending, approve, revoke, revoke_all, fingerprint, script-content TOCTOU guard, file-stamp reload) is wired through `ShellHookConsent::shared()` at `plugin_ops.rs:377`, `mod.rs:322, 565`, plus `bin/aleph-server/commands/hooks.rs` and `diagnostics/checks/hooks_consent.rs`. Every `pub` method has a consumer.
- `src/extension/hooks/output_budget.rs` — `budget_hook_contexts` is the live consumer of every `additional_contexts` write from `run_loop/inner.rs:412`, `tools/scoped/dispatch.rs`, `memory/session_compactor/prepare_history.rs`. `join_messages` is reached from `run_loop/inner.rs`.
- `src/extension/hooks/user_settings.rs` — `load_user_hooks` is the production loader (`mod.rs:1089`); `default_kind_for_event` is `pub(crate)` and used from both `mod.rs:1168` and `manifest/parsers.rs:679`. The `parse_event` snake/Pascal tolerance is the CC-compat wire bridge.
- `src/extension/manifest/parsers.rs` — every `parse_*_dir`, `parse_hooks_file`, `parse_mcp_config_file`, `parse_v2_prompt`, `parse_v2_tool_prompts`, `parse_v2_hooks`, `parse_v2_services` has a live caller through the adapter pipeline. `is_path_inside`, `is_hidden`, `substitute_vars` are private helpers.
- `src/extension/marketplace/installer.rs` — `install_plugin_from_cache`, `update_plugin_from_cache`, `verify_plugin_integrity`, `directory_digest`, `copy_dir_recursive` are all wired (`hub/install.rs:179, 713`, `marketplace/mod.rs:238, 332`, internal recursive use). `pub(crate)` visibility is correct.
- `src/extension/marketplace/manifest.rs` — every `parse_marketplace_*` function is the load-time deserialiser for the cache; `MarketplaceManifest` is the wire DTO consumed by `MarketplaceManager::search_plugin` and `hub/official_plugins.rs:75`.
- `src/extension/marketplace/github_source.rs` — `clone_github_repo` / `pull_github_repo` / `sync_github_marketplace` are exercised from `MarketplaceManager::update` (mod.rs:142). The owner/repo slug validation (`is_valid_owner_repo`) is the input gate.
- `src/extension/marketplace/local_source.rs` — `resolve_local_marketplace` is the local-source path of `MarketplaceManager::update` (mod.rs:144) and `resolve_cache_dir` (mod.rs:390). `expand_tilde` is a private helper.
- `src/extension/registrar/mcp_registrar.rs` — `McpScope::provision` / `shutdown` / `tools` are the production surface (`agents/subagent_spawner/mod.rs:312, 608`); `InlineMcpHandle::mark_cleaned` is the explicit-cleanup path (mcp_registrar.rs:308); the Drop safety-net is the leak backstop.
- `src/extension/manifest/adapters/{aleph_toml, codex, cursor, auto_discover}.rs` — every adapter registers the four priority slots (100/90/85/80/70/-100) and exposes `detect` / `parse` / `format_name` / `priority`. The lifecycle is the `AdapterRegistry::with_defaults` constructor (`manifest/adapter.rs:91-103`).

## Verification summary

- `rg -n "validation\.warnings\|validation\.info"` → only `validation.rs`'s own writes.
- `rg -n "validate_plugin_name\b" src/` → only the definition and one unit test.
- `rg -n "ALEPH_PLUGIN_MANIFEST\b" src/` → only the definition.
- `rg -n "\.with_root_dir\b\|\.has_config\b\|\.requires_permissions\b" src/` → only tests.
- `rg -n "manifest\.extensions\b\|manifest\.keywords\b\|manifest\.homepage\b\|manifest\.repository\b\|manifest\.license\b" src/` → no production match.
- `rg -n "\.commands_v2\b\|\.services_v2\b\|\.capabilities_v2\b" src/` → only the three parser constructors that set them to `None`.
- `rg -n "\.label\b\|\.help\b\|\.placeholder\b" src/extension/manifest/` → only the default-construction test.
- `rg -n "McpServerConfig::Remote\|OAuthConfig" src/` → only the type definition itself (no `McpServerConfig::Remote { … }` literal anywhere).
- `rg -n "\.with_timeout\b" src/extension/hooks/executor.rs` → only the executor's own test.
- `rg -n "all_succeeded\|outputs\(\)" src/extension/hooks/` outside tests → zero hits.
- `rg -n "with_file_path\|with_working_dir" src/extension/hooks/` outside tests → zero hits.
- `rg -n "ManifestCache::clear\|ManifestCache::len\|ManifestCache::is_empty" src/` outside tests → zero hits.
- `rg -n "dynamic_tools\b\|dynamic_hooks\b" src/` → only the field definitions + `cc_plugin_toml.rs:213-214` boolean.
- `rg -n "metadata\.plugin_root\|metadata\.description\|metadata\.version" src/` outside tests → zero hits in production code.
- `rg -n "owner\.name\|owner\.email\|owner\.url" src/extension/marketplace/` outside tests → zero hits.
- `rg -n "\.search_plugin\b\|PluginSearchResult" src/bin/ interfaces/` → zero hits.
- `rg -n "api\.registry\(\)" src/` outside `registrar/api.rs` → zero hits.
- Every path:line above was re-verified after the audit ran.