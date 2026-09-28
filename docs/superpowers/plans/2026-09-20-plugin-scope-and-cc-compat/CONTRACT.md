# Shared interface contract for all plan writers (2026-09-20 plugin-scope round)

Every plan writer MUST use these exact names/signatures when a task consumes or produces them.
If the real codebase forces a deviation (e.g. `PluginId` is actually `String`, or `ExtensionManager`
methods take `&mut self`), KEEP the contract name and record the deviation in a final
`## Contract deltas` section of your output so the lead can reconcile across phases.

Repo root (worktree): `/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/plugin-scope-round`
Spec: `docs/superpowers/specs/2026-09-20-plugin-scope-and-cc-compat-design.md`
Evidence: `docs/superpowers/specs/2026-09-20-plugin-scope-and-cc-compat-evidence/scan-*.md`
Base commit: `3ddc1f2e7` (+ `35e5f8bca` spec commit). All `file:line` cites in the spec are against `3ddc1f2e7`.

## Temporal composability (src/extension/effects/)

```rust
// src/extension/effects/disposer.rs
use futures::future::BoxFuture;
/// One reversible side effect a plugin made on the running process.
pub type Disposer = Box<dyn FnOnce() -> BoxFuture<'static, ()> + Send>;
/// Wrap a synchronous cleanup as a Disposer.
pub fn sync_disposer(f: impl FnOnce() + Send + 'static) -> Disposer;
/// Wrap an async cleanup as a Disposer.
pub fn async_disposer<F, Fut>(f: F) -> Disposer
where F: FnOnce() -> Fut + Send + 'static, Fut: std::future::Future<Output = ()> + Send + 'static;

// src/extension/effects/scope.rs
pub struct EffectScope { /* plugin_id, disposers: Vec<(&'static str, Disposer)> */ }
impl EffectScope {
    pub fn new(plugin_id: PluginId) -> Self;
    pub fn plugin_id(&self) -> &PluginId;
    /// Record one effect. `step` is a stable label such as "registry_row", "wasm_module",
    /// "mcp_server", "service", "memory_extension", "slash_command".
    pub fn effect(&mut self, step: &'static str, d: Disposer);
    pub fn len(&self) -> usize;
    /// Run every disposer in REVERSE registration order. A failing/panicking disposer is
    /// recorded in the report and does NOT stop the rest. Consumes self.
    pub async fn dispose(self) -> DisposeReport;
}
#[derive(Debug)]
pub struct DisposeReport { pub plugin_id: PluginId, pub steps: Vec<(&'static str, Result<(), String>)> }
impl DisposeReport { pub fn all_ok(&self) -> bool; }
```

`PluginId` = whatever `src/extension/registry/types.rs` uses today for the plugin identifier
(verify; if it is a bare `String`, use `String` and note it in Contract deltas).

Effect step labels (exactly these six, in this registration order inside `mount`):
1. `"registry_row"`   — `PluginRegistry::register_plugin` ↔ `PluginRegistry::unregister_plugin(id)` (new)
2. `"wasm_module"`    — `PluginLoader` load ↔ unload
3. `"mcp_server"`     — `McpManagerHandle::add_transient_server` ↔ `remove_transient_server`
4. `"service"`        — `service_manager` start ↔ stop
5. `"memory_extension"` — `MemoryExtensionRegistry::register*` ↔ `MemoryExtensionRegistry::unregister(plugin_id)` (new)
6. `"slash_command"`  — ToolCatalog `register_skills` ↔ `unregister_skills_for_plugin(plugin_id)` (new)

Every registrar function that creates one of these effects returns `#[must_use] Disposer`
(or `Result<Disposer, E>`). The CALLER (lifecycle) pushes it into the plugin's `EffectScope`.

## Lifecycle (src/extension/lifecycle.rs)

```rust
impl ExtensionManager {
    /// Parse → trust/enabled gate → new EffectScope → register effects in the fixed order above.
    /// Any failure disposes the partial scope (all-or-none) and returns Err(MountError { step, reason }).
    pub async fn mount(&self, id: &PluginId) -> Result<PluginStatus, MountError>;
    /// Take the scope out of `self.scopes`, dispose it, write terminal status.
    pub async fn unmount(&self, id: &PluginId) -> Result<DisposeReport, UnmountError>;
    /// unmount + mount. Replaces the old narrow `reload_plugin` (mod.rs:1303).
    pub async fn reload_plugin(&self, id: &PluginId) -> Result<PluginStatus, MountError>;
    /// unmount + mount for every discovered plugin. Replaces the body of the old `reload()` (mod.rs:802).
    pub async fn reload(&self) -> ReloadReport;
}
/// THE ONLY place that recomputes views after a transition:
/// republish_plugin_projections() + sync_hooks_from_registry() + McpFace::notify_tools_list_changed().
async fn after_transition(&self);

#[derive(Debug, thiserror::Error)] pub enum MountError { /* Step(&'static str, String), NotFound, Blocked, ... */ }
#[derive(Debug, thiserror::Error)] pub enum UnmountError { NotMounted, /* ... */ }
pub struct ReloadReport { pub mounted: Vec<PluginId>, pub failed: Vec<(PluginId, MountError)> }
```

`ExtensionManager` gains a field `scopes: <lock>HashMap<PluginId, EffectScope>` (use the same lock
type the manager already uses for its registry; lock unwrap via `unwrap_or_else(|e| e.into_inner())`).
`set_plugin_enabled(id, true)` → `mount`; `(id, false)` → `unmount`. Watcher and `plugin.reload` /
`hooks.reload` RPC call only these primitives.

## Status (src/extension/types/plugins.rs)

```rust
pub enum PluginStatus {
    Active, Disabled, Blocked, Failed { step: String, reason: String },
    /// Mounted but one effect has not reached its terminal state yet (e.g. MCP `initialize` pending).
    Pending { waiting_on: Vec<String> },
    // `Overridden` is REMOVED in this round (zero producers).
}
```

## Spatial composability (src/extension/visibility.rs — renamed from scope.rs)

```rust
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ScopeKey { Global, Project(std::path::PathBuf /* canonicalized root */) }
#[derive(Clone, Debug, Default)]
pub struct VisibilityCtx { pub project_root: Option<std::path::PathBuf> }
/// Global → always visible. Project(p) → visible iff ctx.project_root == Some(p).
/// A session with no project sees Global only (fail-closed; behaviour change vs today).
pub fn visible_to(key: &ScopeKey, ctx: &VisibilityCtx) -> bool;
```

Every registry row carries a `scope_key: ScopeKey` derived at discovery time
(`Project(root)` only for `<project>/.claude/` and `<project>/.aleph/plugins{,.local}`; all other
origins incl. the new `PluginOrigin::ClaudeCache` are `Global`).
`VisibilityCtx.project_root` is derived by the SAME code hooks use today upstream of
`executor.rs:918 project_scope_allows` — find it, extract it into visibility.rs, do not write a second one.

## Activation gate (src/extension/activation_gate.rs)

```rust
pub struct ActivationReport { pub non_terminal: Vec<(PluginId, Vec<String> /* waiting_on */)> }
/// Called once after `load_all` at boot; terminal-status set is derived from the enum (census G6).
pub fn assess(registry: &PluginRegistry) -> ActivationReport;
```
Doctor check id: `extension/plugins-activated` (in `src/diagnostics/checks/`).

## MCP face (src/gateway/mcp_face/)

```rust
// src/gateway/mcp_face/config.rs
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct McpServerConfig { pub enabled: bool, pub expose: Vec<String> /* tool names */ }
// config key: [mcp_server] in the main config TOML.

// src/gateway/mcp_face/mod.rs
pub struct McpFace { /* ... */ }
impl McpFace {
    /// Broadcast `notifications/tools/list_changed` to every live MCP session. No-op when disabled.
    pub fn notify_tools_list_changed(&self);
}
/// Process-level handle so lifecycle.rs can notify without threading a reference through 7 constructors
/// (same pattern as spend `install_ledger`). Returns None when the face is not installed.
pub fn try_mcp_face() -> Option<&'static McpFace>;
```

HTTP route: `POST /mcp` (JSON-RPC), `GET /mcp` (SSE stream, optional), header `Mcp-Session-Id`.
Protocol versions supported: at least `2025-03-26` and the newest version implemented by
`src/mcp/protocol.rs` (verify the literal there). Capabilities advertised: `tools: { listChanged: true }` only.
Principal for calls: `McpClient { client_name: String }` (verify how principals are modelled in
`src/identity/` / `src/gateway/` and record deltas).

## Hooks (src/extension/hooks/)

- Decision derivation lives in ONE function (new or existing) that takes
  `(exit_code: Option<i32>, stdout: &str, stderr: &str, kind: HookKind)` and returns the existing
  hook decision type. Exit 2 on an Interceptor → blocked with stderr as reason.
- `JsonHookOutput` gains `hook_specific_output.updated_input: Option<serde_json::Value>` routed
  through the same path as the `update_input:` prefix.
- CC→Aleph tool-name alias table for matchers: one `const CC_TOOL_ALIASES: &[(&str, &str)]` in the
  matcher module, used only there.

## Commands (src/extension/template.rs → connected)

`SkillTemplate::render(&self, args: &str, ctx: &TemplateCtx) -> Result<String, TemplateError>` where
`TemplateCtx { cwd, file_reader: Box<dyn Fn(&Path) -> Result<String>>, shell: Option<Box<dyn Fn(&str) -> Result<String>>> }`
(verify the existing `template.rs` API first; keep its names if they already exist and note deltas).

## Discovery

`PluginOrigin::ClaudeCache` (new variant). Source dir: `~/.claude/plugins/` — read
`installed_plugins.json`, resolve each entry to `cache/<marketplace>/<plugin>/<version>/`.
Verify the JSON shape against `scan-cc-plugin-format.md` §1 AND the real file on this machine
(`~/.claude/plugins/installed_plugins.json`, read-only). Newly discovered ClaudeCache plugins default
to `Disabled` in `plugins.toml` semantics (i.e. absent ⇒ disabled for this origin only).

## Test/QA conventions

- Unit tests live in `#[cfg(test)] mod tests` next to the code (existing convention); census-style
  source-level guards live where the existing `census` tests live (look at `src/capability/census.rs`
  and `src/extension/projection.rs:162` for the shape).
- Integration tests: `tests/*.rs` with `--features test-helpers`.
- Real-machine QA: `qa/<name>/run.sh <stage>` bash scripts; read `qa/README.md` and an existing
  `qa/plugins/run.sh` for the harness conventions (fake API key, port allocation, log grep).
- Verification commands (copy verbatim into tasks as appropriate):
  ```
  cargo test -p alephcore --lib <filter>
  cargo test -p alephcore --lib --no-run
  cargo test -p alephcore --bins
  cargo test -p alephcore --features test-helpers --test '*' --no-run
  cargo test -p aleph-panel --lib --no-run
  cargo clippy --workspace --all-targets   # after `just _stage-shell-placeholders`
  ```
- Commits: `<scope>: <description>` in English, plus the trailer line
  `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`. Do NOT bypass git hooks.
- `src/harness/` must have a 0-line diff in every task.
