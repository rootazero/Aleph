# Extension Consolidation — Dead-Capability Withdrawal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Withdraw 5 dead plugin-capability areas (Channel, Provider, GatewayMethod, Cli, HttpRoute+HttpHandler) from `src/extension/`, removing ~1,500–2,000 lines of zero-consumer code per CLAUDE.md R10.

**Architecture:** Pure deletion. Each of the 5 capabilities is removed atomically across every file that references it (capability enum, registrar dispatch, registry storage, registration types, manifest sections, runtime types, plugin record, manager files). The Rust compiler is the completeness oracle: delete the root declarations, then `cargo check` and remove every dangling reference it reports (all removals). No behavior change to the 6 working capabilities (Tool/Hook/Skill/Command/Agent/Service) or McpServer.

**Tech Stack:** Rust, `cargo check -p alephcore`, `cargo test -p alephcore --lib`, `rg` (ripgrep).

**Spec:** `docs/superpowers/specs/2026-05-19-extension-consolidation-design.md`

**Worktree:** `.worktrees/extension-consolidation` (branch `extension-consolidation`). All commands run from the worktree root: `/Volumes/TBU4/Workspace/Aleph/.worktrees/extension-consolidation`.

**Baseline note:** `main` has 8 lib + 4 integration pre-existing test failures (project memory). Verification asserts **no new failures**, not zero failures.

**Deletion-loop convention (applies to every Task 2–6):** After deleting the named root symbols, run `cargo check -p alephcore`. Every error will be a dangling reference to a just-deleted item. Remove each such reference (imports, match arms, test bodies, doc-comment `[`links`]`). Repeat until `cargo check` is clean. This is the "green" step — there is no implementation code to add.

---

## Task 1: External-consumer recheck + baseline gate

**Files:** none modified — this is a verification gate.

- [ ] **Step 1: Confirm worktree + branch**

Run: `git -C /Volumes/TBU4/Workspace/Aleph/.worktrees/extension-consolidation branch --show-current`
Expected: `extension-consolidation`

- [ ] **Step 2: Recheck for consumers OUTSIDE `src/extension/`**

Run each command from the worktree root. Each MUST return no hits outside `src/extension/` and `docs/`:

```bash
rg -n 'ChannelRegistration|ProviderRegistration|GatewayMethodRegistration|HttpRouteRegistration|HttpHandlerRegistration|CliRegistration' src --glob '!src/extension/**'
rg -n 'ChannelManager|ChannelHandle|PluginProviderAdapter|PluginHttpHandler|match_path' src --glob '!src/extension/**'
rg -n 'register_channel|register_provider|register_gateway_method|register_http_route|register_http_handler|register_cli_command|list_channels\(|list_providers\(|list_gateway_methods|list_http_routes|list_http_handlers|list_cli_commands|get_gateway_method|get_cli_command' src --glob '!src/extension/**'
rg -n 'ChannelSection|HttpRouteSection|ProviderSection' src --glob '!src/extension/**'
```

Note: `src/gateway/` has its OWN unrelated `ChannelInfo`/`ChannelManager`/`Channel` types — those are name collisions, not consumers of `src/extension/` types. Only flag hits that `use crate::extension::...` or `use alephcore::extension::...` the symbols above.

Expected: no extension-symbol consumers outside `src/extension/`.

**If any external consumer is found:** STOP. Do not proceed. Report the exact file:line to the user — the deletion scope must be re-confirmed.

- [ ] **Step 3: Capture baseline test state**

Run: `cargo test -p alephcore --lib 2>&1 | tail -30`
Record the failing-test count and names. This is the baseline for "no new failures" in Task 8.

- [ ] **Step 4: No commit** (verification only).

---

## Task 2: Withdraw the Channel capability

**Files:**
- Delete: `src/extension/channel_manager.rs`
- Modify: `src/extension/mod.rs`
- Modify: `src/extension/capability.rs`
- Modify: `src/extension/registrar/api.rs`
- Modify: `src/extension/registry/types.rs`
- Modify: `src/extension/registry/plugin_registry/mod.rs`
- Modify: `src/extension/types/runtime.rs`
- Modify: `src/extension/types/plugins.rs`
- Modify: `src/extension/error.rs`
- Modify: `src/extension/manifest/toml_types.rs`, `src/extension/manifest/types.rs`, `src/extension/manifest/mod.rs`, `src/extension/manifest/cc_plugin_toml.rs`, `src/extension/manifest/cc_plugin_json.rs`

- [ ] **Step 1: Delete the manager file and its module wiring**

Delete file `src/extension/channel_manager.rs`.
In `src/extension/mod.rs` remove the line `mod channel_manager;` and the line `pub use channel_manager::{ChannelHandle, ChannelManager};`.

- [ ] **Step 2: Remove the `Channel` capability variant**

In `src/extension/capability.rs`:
- Remove `ChannelRegistration` from the `use crate::extension::registry::{...}` import list.
- Remove the type alias `pub type ChannelDeclaration = ChannelRegistration;` (and its doc line).
- Remove the `Channel(ChannelDeclaration)` variant from `enum CapabilityDeclaration`.
- In `tier()`, remove `Self::Channel(_)` from the `Tier::Pluggable` arm.
- In `kind_name()`, remove the `Self::Channel(_) => "channel",` arm.
- In `required_permission()`, remove the `Self::Channel(_) => Some(PluginPermission::Network),` arm.
- In the test `test_all_12_variants_have_kind_names`, remove the `CapabilityDeclaration::Channel(...)` element and the `"channel"` string from the expected `kind_names` vec. (The final variant count after Tasks 2–6 is handled in Task 7 — for now just keep the test compiling by also updating the `assert_eq!(capabilities.len(), 12)` to the current count.)

- [ ] **Step 3: Remove `Channel` from the registrar**

In `src/extension/registrar/api.rs`:
- Remove the `CapabilityDeclaration::Channel(channel) => { self.registry.register_channel(channel); }` arm from `dispatch()`.
- Remove `ChannelRegistration` from the test `use` import.
- Remove the `make_channel()` test helper.
- Remove tests `test_p2_channel_fails_without_network_permission` and `test_p2_channel_succeeds_with_network_permission`.
- In `test_dispatch_routes_correctly`, remove the `api.register_capability(make_channel()).unwrap();` line and the `assert!(reg.get_channel("test-channel").is_some());` line.

- [ ] **Step 4: Remove `ChannelRegistration` type**

In `src/extension/registry/types.rs`:
- Remove the `ChannelRegistration` struct.
- Remove the `test_channel_registration` test.

- [ ] **Step 5: Remove channel storage from the registry**

In `src/extension/registry/plugin_registry/mod.rs`:
- Remove `ChannelRegistration` from the `use super::types::{...}` import.
- Remove the `channels: HashMap<String, ChannelRegistration>,` field.
- Remove `self.channels.clear();` from `clear()`.
- Remove the entire "Channel Registration" section: `register_channel`, `get_channel`, `list_channels`.
- Remove `self.channels.retain(...)` from `unregister_plugin()`.
- Remove `channels: self.channels.len(),` from `stats()`.
- Remove `pub channels: usize,` from `struct RegistryStats`.

- [ ] **Step 6: Remove channel runtime types**

In `src/extension/types/runtime.rs` remove these structs/enums: `ChannelMessage`, `ChannelSendRequest`, `ChannelState`, `ChannelInfo`. Keep all `Service*` types. Update the module doc comment to drop the "messaging channels" line.

- [ ] **Step 7: Remove channel tracking from `PluginRecord`**

In `src/extension/types/plugins.rs`:
- Remove the `pub channel_ids: Vec<String>,` field from `PluginRecord`.
- Remove `channel_ids: Vec::new(),` from `PluginRecord::new()`.
- In `from_adapter_output()`: remove the `let mut channel_ids = Vec::new();` line, the `CapabilityDeclaration::Channel(c) => channel_ids.push(c.id.clone()),` match arm, and `channel_ids,` from the returned struct.

- [ ] **Step 8: Remove `ChannelNotFound` error**

In `src/extension/error.rs` remove the `ChannelNotFound(String)` variant and its `#[error("Channel not found: {0}")]` attribute.

- [ ] **Step 9: Remove channel manifest sections**

Apply, then let `cargo check` guide removal of anything missed:
- `src/extension/manifest/toml_types.rs`: remove `pub struct ChannelSection`, the `pub channels: Vec<ChannelSection>,` field, and the `channels_v2: if toml.channels.is_empty() {...}` block (set the consuming field accordingly / remove it).
- `src/extension/manifest/types.rs`: remove `ChannelSection` from the import list, remove `pub channels_v2: Option<Vec<ChannelSection>>,`, remove `pub channels: Vec<ChannelSection>,` (line ~468), and any defaulting of those fields.
- `src/extension/manifest/cc_plugin_toml.rs`: remove `ChannelSection` from import, the `pub channels: Vec<ChannelSection>,` field, `channels: aleph.channels,`, and the `test_parse_cc_toml_with_channels` test.
- `src/extension/manifest/cc_plugin_json.rs`: remove the `"channels"` block in the test JSON and the `ext.channels` assertions.
- `src/extension/manifest/mod.rs`: remove `ChannelSection` from the re-export list.
- Wherever the manifest→capability conversion produces `CapabilityDeclaration::Channel` (compiler will flag it once the variant is gone): remove that conversion code.

- [ ] **Step 10: Compile-clean loop**

Run: `cargo check -p alephcore`
Apply the deletion-loop convention until clean. Expected final: `Finished`.

- [ ] **Step 11: Verify no residue**

Run: `rg -n 'Channel(Registration|Manager|Handle|Message|SendRequest|State|Info|Section|Declaration)|register_channel|channel_ids|ChannelNotFound' src/extension`
Expected: no hits (the gateway's own `Channel*` types live outside `src/extension/` and are unaffected).

- [ ] **Step 12: Run extension tests**

Run: `cargo test -p alephcore --lib extension 2>&1 | tail -20`
Expected: passes; no new failures vs Task 1 baseline.

- [ ] **Step 13: Commit**

```bash
git add -A
git commit -m "extension: withdraw dead Channel capability

Zero consumers, zero plugins declare it; ChannelManager used an
in-process mpsc model incompatible with WASM/MCP plugins. Per R10."
```

---

## Task 3: Withdraw the Provider capability

**Files:**
- Delete: `src/extension/provider_adapter.rs`
- Modify: `src/extension/mod.rs`, `capability.rs`, `registrar/api.rs`, `registry/types.rs`, `registry/plugin_registry/mod.rs`, `types/runtime.rs`, `types/plugins.rs`
- Modify: `src/extension/manifest/cc_plugin_toml.rs` (and any other manifest file referencing `ProviderSection`)

- [ ] **Step 1: Delete the adapter file and module wiring**

Delete file `src/extension/provider_adapter.rs`.
In `src/extension/mod.rs` remove `mod provider_adapter;` and `pub use provider_adapter::PluginProviderAdapter;`.

- [ ] **Step 2: Remove the `Provider` capability variant**

In `src/extension/capability.rs`:
- Remove `ProviderRegistration` from the `use crate::extension::registry::{...}` import.
- Remove the `pub type ProviderDeclaration = ProviderRegistration;` alias + doc line.
- Remove the `Provider(ProviderDeclaration)` variant from `CapabilityDeclaration`.
- In `tier()`, remove `Self::Provider(_)` from the `Tier::Pluggable` arm.
- In `kind_name()`, remove `Self::Provider(_) => "provider",`.
- In `required_permission()`, remove `Self::Provider(_) => None,`.
- In `test_all_12_variants_have_kind_names`, remove the `Provider` element, the `"provider"` string, and decrement the length assertion.

- [ ] **Step 3: Remove `Provider` from the registrar**

In `src/extension/registrar/api.rs`:
- Remove the `CapabilityDeclaration::Provider(provider) => {...}` arm from `dispatch()`.
- Remove `ProviderRegistration` from test imports.
- Remove the `make_provider()` helper and `test_p1_registration_no_permissions_required` (it registers a provider — if it tests something else too, just remove the provider line; this test's body only registers `make_provider()`, so remove the whole test).
- In `test_dispatch_routes_correctly`, remove the `api.register_capability(CapabilityDeclaration::Provider(...))` block and `assert!(reg.get_provider("test-provider").is_some());`.

- [ ] **Step 4: Remove `ProviderRegistration` type**

In `src/extension/registry/types.rs`: remove the `ProviderRegistration` struct and `test_provider_registration`.

- [ ] **Step 5: Remove provider storage from the registry**

In `src/extension/registry/plugin_registry/mod.rs`:
- Remove `ProviderRegistration` from imports.
- Remove the `providers:` field, `self.providers.clear();`, the "Provider Registration" section (`register_provider`/`get_provider`/`list_providers`), the `providers.retain(...)` line in `unregister_plugin()`, `providers: self.providers.len(),` in `stats()`, and `pub providers: usize,` in `RegistryStats`.

- [ ] **Step 6: Remove provider runtime types**

In `src/extension/types/runtime.rs` remove: `ProviderChatRequest`, `ProviderMessage`, `ProviderChatResponse`, `ProviderUsage`, `ProviderStreamChunk`, `ProviderModelInfo`. Update the module doc comment to drop the "AI providers" line.

- [ ] **Step 7: Remove provider tracking from `PluginRecord`**

In `src/extension/types/plugins.rs`:
- Remove `pub provider_ids: Vec<String>,` and `provider_ids: Vec::new(),` from `new()`.
- In `from_adapter_output()`: remove `let mut provider_ids = Vec::new();`, the `CapabilityDeclaration::Provider(p) => provider_ids.push(p.id.clone()),` arm, and `provider_ids,` from the returned struct.

- [ ] **Step 8: Remove provider manifest sections**

`rg -n 'ProviderSection|providers_v2|provider' src/extension/manifest` to locate. Remove the `ProviderSection` struct/import, any `providers`/`providers_v2` manifest fields, and the manifest→`CapabilityDeclaration::Provider` conversion code. Use `cargo check` to find every site.

- [ ] **Step 9: Compile-clean loop**

Run: `cargo check -p alephcore` — apply deletion-loop until `Finished`.

- [ ] **Step 10: Verify no residue**

Run: `rg -n 'ProviderRegistration|ProviderDeclaration|PluginProviderAdapter|register_provider|provider_ids|ProviderChat|ProviderStreamChunk|ProviderModelInfo|ProviderUsage|ProviderSection' src/extension`
Expected: no hits. (Unrelated `provider` words in McpServer/other contexts are fine — only the listed identifiers must be gone.)

- [ ] **Step 11: Run extension tests**

Run: `cargo test -p alephcore --lib extension 2>&1 | tail -20` — no new failures vs baseline.

- [ ] **Step 12: Commit**

```bash
git add -A
git commit -m "extension: withdraw dead Provider capability

PluginProviderAdapter had zero consumers; plugin IPC providers are
strictly weaker than Aleph's native provider system. Per R10."
```

---

## Task 4: Withdraw the GatewayMethod capability

**Files:** `capability.rs`, `registrar/api.rs`, `registry/types.rs`, `registry/plugin_registry/mod.rs`, `types/plugins.rs`

- [ ] **Step 1: Remove the `GatewayMethod` capability variant**

In `src/extension/capability.rs`:
- Remove `GatewayMethodRegistration` from the `use crate::extension::registry::{...}` import.
- Remove the `pub type GatewayMethodDeclaration = GatewayMethodRegistration;` alias + doc line.
- Remove the `GatewayMethod(GatewayMethodDeclaration)` variant.
- In `tier()`, remove `Self::GatewayMethod(_)` from the `Tier::GatewayExtension` arm.
- In `kind_name()`, remove `Self::GatewayMethod(_) => "gateway_method",`.
- In `required_permission()`, remove `Self::GatewayMethod(_) => Some(PluginPermission::GatewayRpc),`.
- In `test_all_12_variants_have_kind_names`, remove the `GatewayMethod` element + `"gateway_method"` string + decrement length assertion.

- [ ] **Step 2: Remove from the registrar**

In `src/extension/registrar/api.rs`:
- Remove the `CapabilityDeclaration::GatewayMethod(method) => {...}` arm from `dispatch()`.
- Remove `GatewayMethodRegistration` from test imports.
- In `test_dispatch_routes_correctly`, remove the `api.register_capability(CapabilityDeclaration::GatewayMethod(...))` block and `assert!(reg.get_gateway_method("test.method").is_some());`.

- [ ] **Step 3: Remove `GatewayMethodRegistration` type**

In `src/extension/registry/types.rs`: remove the struct and `test_gateway_method_registration`.

- [ ] **Step 4: Remove gateway-method storage**

In `src/extension/registry/plugin_registry/mod.rs`: remove `GatewayMethodRegistration` import, the `gateway_methods:` field, `self.gateway_methods.clear();`, the "Gateway Method Registration" section (`register_gateway_method`/`get_gateway_method`/`list_gateway_methods`), the `gateway_methods.retain(...)` line, `gateway_methods: self.gateway_methods.len(),` in `stats()`, and `pub gateway_methods: usize,` in `RegistryStats`.

- [ ] **Step 5: Remove from `PluginRecord`**

In `src/extension/types/plugins.rs`: remove `pub gateway_methods: Vec<String>,`, `gateway_methods: Vec::new(),` in `new()`, `gateway_methods: Vec::new(),` in `from_adapter_output()`'s returned struct, and the `CapabilityDeclaration::GatewayMethod(_)` entry from the catch-all `{}` match arm.

- [ ] **Step 6: Compile-clean loop**

Run: `cargo check -p alephcore` — apply deletion-loop until `Finished`.

- [ ] **Step 7: Verify no residue**

Run: `rg -n 'GatewayMethod|gateway_method' src/extension`
Expected: no hits.

- [ ] **Step 8: Run extension tests**

Run: `cargo test -p alephcore --lib extension 2>&1 | tail -20` — no new failures.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "extension: withdraw dead GatewayMethod capability

Registered into PluginRegistry but never dispatched by the gateway.
Zero consumers, zero plugins. Per R10."
```

---

## Task 5: Withdraw the Cli capability

**Files:** `capability.rs`, `registrar/api.rs`, `registry/types.rs`, `registry/plugin_registry/mod.rs`, `types/plugins.rs`

- [ ] **Step 1: Remove the `Cli` capability variant**

In `src/extension/capability.rs`:
- Remove `CliRegistration` from the `use crate::extension::registry::{...}` import.
- Remove the `pub type CliDeclaration = CliRegistration;` alias + doc line.
- Remove the `Cli(CliDeclaration)` variant.
- In `tier()`, remove `Self::Cli(_)` from the `Tier::GatewayExtension` arm.
- In `kind_name()`, remove `Self::Cli(_) => "cli",`.
- In `required_permission()`, remove `Self::Cli(_) => Some(PluginPermission::Shell),`.
- In `test_all_12_variants_have_kind_names`, remove the `Cli` element + `"cli"` string + decrement length assertion.

- [ ] **Step 2: Remove from the registrar**

In `src/extension/registrar/api.rs`:
- Remove the `CapabilityDeclaration::Cli(cli) => {...}` arm from `dispatch()`.
- Remove `CliRegistration` from test imports.
- In `test_dispatch_routes_correctly`, remove the `api.register_capability(CapabilityDeclaration::Cli(...))` block and `assert!(reg.get_cli_command("test-cmd").is_some());`.

- [ ] **Step 3: Remove `CliRegistration` type**

In `src/extension/registry/types.rs`: remove the struct and `test_cli_registration`.

- [ ] **Step 4: Remove CLI-command storage**

In `src/extension/registry/plugin_registry/mod.rs`: remove `CliRegistration` import, the `cli_commands:` field, `self.cli_commands.clear();`, the "CLI Command Registration" section (`register_cli_command`/`get_cli_command`/`list_cli_commands`), the `cli_commands.retain(...)` line, `cli_commands: self.cli_commands.len(),` in `stats()`, and `pub cli_commands: usize,` in `RegistryStats`.

- [ ] **Step 5: Remove from `from_adapter_output`**

In `src/extension/types/plugins.rs`: remove `CapabilityDeclaration::Cli(_)` from the catch-all `{}` match arm.

- [ ] **Step 6: Compile-clean loop**

Run: `cargo check -p alephcore` — apply deletion-loop until `Finished`.

- [ ] **Step 7: Verify no residue**

Run: `rg -n 'CliRegistration|CliDeclaration|cli_command' src/extension`
Expected: no hits.

- [ ] **Step 8: Run extension tests**

Run: `cargo test -p alephcore --lib extension 2>&1 | tail -20` — no new failures.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "extension: withdraw dead Cli capability

Registered into PluginRegistry but never consumed by the CLI layer.
Plugin functionality already reaches users via Tool/Command. Per R10."
```

---

## Task 6: Withdraw the HttpRoute + HttpHandler capability

**Files:**
- Delete: `src/extension/http_handler.rs`
- Modify: `src/extension/mod.rs`, `capability.rs`, `registrar/api.rs`, `registry/types.rs`, `registry/plugin_registry/mod.rs`, `types/runtime.rs`, `types/plugins.rs`
- Modify: manifest files referencing `HttpRouteSection` / `http_routes`

- [ ] **Step 1: Delete the handler file and module wiring**

Delete file `src/extension/http_handler.rs`.
In `src/extension/mod.rs` remove `mod http_handler;` and `pub use http_handler::{match_path, PluginHttpHandler};`.

- [ ] **Step 2: Remove the `HttpRoute` capability variant**

In `src/extension/capability.rs`:
- Remove `HttpRouteRegistration` from the `use crate::extension::registry::{...}` import.
- Remove the `pub type HttpRouteDeclaration = HttpRouteRegistration;` alias + doc line.
- Remove the `HttpRoute(HttpRouteDeclaration)` variant.
- In `tier()`, remove `Self::HttpRoute(_)` from the `Tier::GatewayExtension` arm.
- In `kind_name()`, remove `Self::HttpRoute(_) => "http_route",`.
- In `required_permission()`, remove `Self::HttpRoute(_) => Some(PluginPermission::HttpRoutes),`.
- In `test_all_12_variants_have_kind_names`, remove the `HttpRoute` element + `"http_route"` string + decrement length assertion.

- [ ] **Step 3: Remove from the registrar**

In `src/extension/registrar/api.rs`:
- Remove the `CapabilityDeclaration::HttpRoute(route) => {...}` arm from `dispatch()`.
- Remove `HttpRouteRegistration` from test imports.
- Remove the `make_http_route()` helper.
- Remove tests `test_p2_http_route_fails_without_http_routes_permission` and `test_p2_http_route_succeeds_with_permission`.
- In `test_dispatch_routes_correctly`, remove the `api.register_capability(make_http_route()).unwrap();` line, `assert_eq!(reg.list_http_routes().len(), 1);`, and `PluginPermission::HttpRoutes` from the permissions vec passed to `CapabilityApi::new`.

- [ ] **Step 4: Remove the registration types**

In `src/extension/registry/types.rs`: remove `HttpRouteRegistration` and `HttpHandlerRegistration` structs, plus tests `test_http_route_registration` and `test_http_handler_registration`.

- [ ] **Step 5: Remove HTTP storage from the registry**

In `src/extension/registry/plugin_registry/mod.rs`: remove `HttpHandlerRegistration` and `HttpRouteRegistration` from imports; remove the `http_routes:` and `http_handlers:` fields; remove `self.http_routes.clear();` and `self.http_handlers.clear();`; remove the "HTTP Route Registration" section (`register_http_route`/`list_http_routes`/`find_http_routes`) and the "HTTP Handler Registration" section (`register_http_handler`/`list_http_handlers`); remove the `http_routes.retain(...)` and `http_handlers.retain(...)` lines; remove `http_routes:`/`http_handlers:` from `stats()`; remove `pub http_routes: usize,` and `pub http_handlers: usize,` from `RegistryStats`.

- [ ] **Step 6: Remove HTTP runtime types**

In `src/extension/types/runtime.rs` remove `HttpRequest` and `HttpResponse` (including the `impl HttpResponse` block). Update the module doc comment to drop the "HTTP routes" line.

- [ ] **Step 7: Remove from `from_adapter_output`**

In `src/extension/types/plugins.rs`: remove `CapabilityDeclaration::HttpRoute(_)` from the catch-all `{}` match arm.

- [ ] **Step 8: Remove HTTP-route manifest sections**

- `src/extension/manifest/toml_types.rs`: remove `pub struct HttpRouteSection`, the `pub http_routes: Vec<HttpRouteSection>,` field, and the `http_routes_v2: if toml.http_routes.is_empty() {...}` block.
- `src/extension/manifest/types.rs`: remove `HttpRouteSection` from imports, `pub http_routes_v2: Option<Vec<HttpRouteSection>>,`, and `http_routes_v2: None,` defaulting.
- `src/extension/manifest/cc_plugin_toml.rs` and `cc_plugin_json.rs`: remove `http_routes_v2: None,` lines.
- `src/extension/manifest/mod.rs`: remove `HttpRouteSection` from the re-export list.
- Remove the manifest→`CapabilityDeclaration::HttpRoute` conversion code (compiler will flag it).

- [ ] **Step 9: Compile-clean loop**

Run: `cargo check -p alephcore` — apply deletion-loop until `Finished`.

- [ ] **Step 10: Verify no residue**

Run: `rg -n 'HttpRoute|HttpHandler|http_route|http_handler|PluginHttpHandler|match_path|HttpRequest|HttpResponse' src/extension`
Expected: no hits.

- [ ] **Step 11: Run extension tests**

Run: `cargo test -p alephcore --lib extension 2>&1 | tail -20` — no new failures.

- [ ] **Step 12: Commit**

```bash
git add -A
git commit -m "extension: withdraw dead HttpRoute/HttpHandler capability

PluginHttpHandler was never wired into the HTTP server; routes and
handlers registered but never dispatched. Per R10."
```

---

## Task 7: Cross-cutting cleanup — empty Tier + orphan permissions

**Files:** `src/extension/capability.rs`, `src/extension/registrar/api.rs`, `src/extension/manifest/types.rs`, `src/extension/registry/mod.rs`, `src/extension/registry/types.rs`

- [ ] **Step 1: Confirm `Tier::GatewayExtension` is now empty**

Run: `rg -n 'Tier::GatewayExtension|GatewayExtension' src/extension`
Expected: hits only in the `Tier` enum definition, `register_capability()`, and doc comments — no `tier()` arm returns it anymore (GatewayMethod/HttpRoute/Cli are gone).

- [ ] **Step 2: Remove the `GatewayExtension` tier**

In `src/extension/capability.rs`: remove the `GatewayExtension` variant from `enum Tier` and its doc line.
In `src/extension/registrar/api.rs` `register_capability()`: remove the `Tier::GatewayExtension => {...}` match arm entirely (the warning-log arm).

- [ ] **Step 3: Fix the `Tier` doc comments**

In `src/extension/capability.rs`, update the per-variant doc comments on `enum Tier` to match the post-deletion reality:
- `Core` — "Tool, Hook, Skill"
- `Important` — "Command, Agent"
- `Pluggable` — "Service, McpServer"

- [ ] **Step 4: Confirm orphan permissions**

Run: `rg -n 'GatewayRpc|HttpRoutes' src/extension`
Expected: hits only in the `PluginPermission` enum definition, its `Display` impl, and its string-parse function — no capability references them.

- [ ] **Step 5: Remove orphan `PluginPermission` variants**

In `src/extension/manifest/types.rs`: remove `PluginPermission::HttpRoutes` and `PluginPermission::GatewayRpc` — the enum variants, their `Display` match arms (`"http-routes"`, `"gateway-rpc"`), and their string-parse arms. Leave `Network`, `Background`, `Shell`, `Filesystem`, `Env` untouched.

- [ ] **Step 6: Confirm graceful unknown-permission handling**

Inspect the `PluginPermission` string-parse function. Unknown permission strings (now including `"http-routes"`/`"gateway-rpc"` in any stale manifest) must NOT panic — they should be ignored or produce a recoverable error. If the parser currently has no catch-all, add one that skips unknown strings with a `tracing::warn!`.

- [ ] **Step 7: Add a regression test for unknown-permission tolerance**

In the `#[cfg(test)] mod tests` of `src/extension/manifest/types.rs`, add:

```rust
#[test]
fn unknown_permission_string_is_ignored_not_panicked() {
    // A stale manifest may still carry a withdrawn permission name.
    // Parsing must degrade gracefully rather than panic.
    let perms = parse_permissions_from_strings(&[
        "network".to_string(),
        "http-routes".to_string(), // withdrawn — must be tolerated
        "gateway-rpc".to_string(), // withdrawn — must be tolerated
    ]);
    assert!(perms.contains(&PluginPermission::Network));
    assert_eq!(perms.len(), 1);
}
```

Adjust the function name `parse_permissions_from_strings` / argument shape to match the actual parser in this file (read it first; the call must compile against the real signature).

- [ ] **Step 8: Fix stale doc comments**

- `src/extension/registry/mod.rs`: the module doc lists "9 types" and a P0/P1/P2/P3 breakdown including Channel/Provider/GatewayMethod/HttpRoute/HttpHandler/Cli. Rewrite it to list only the surviving registration types: `ToolRegistration`, `HookRegistration`, `ServiceRegistration`, `CommandRegistration`, `SkillRegistration`, `AgentRegistration`, plus `PluginDiagnostic`/`DiagnosticLevel`.
- `src/extension/registry/types.rs`: the module doc has the same stale P0–P3 list — rewrite to match.
- `src/extension/registry/plugin_registry/mod.rs`: the module doc enumerates "channels, providers, gateway methods, HTTP routes/handlers, CLI commands" — rewrite to "tools, hooks, services, in-chat commands, skills, agents". Also fix the `unregister_plugin()` doc-comment bullet list.

- [ ] **Step 9: Finalize the capability-count test**

In `src/extension/capability.rs`, rename `test_all_12_variants_have_kind_names` to `test_all_7_variants_have_kind_names`; it should build exactly the 7 surviving variants (Tool, Hook, Service, Command, Skill, Agent, McpServer) and assert `capabilities.len() == 7` with the matching `kind_names` vec `["tool","hook","service","command","skill","agent","mcp_server"]`.

- [ ] **Step 10: Compile-clean loop**

Run: `cargo check -p alephcore` — apply deletion-loop until `Finished`.

- [ ] **Step 11: Run the new test + extension tests**

Run: `cargo test -p alephcore --lib extension 2>&1 | tail -20`
Expected: `unknown_permission_string_is_ignored_not_panicked` and `test_all_7_variants_have_kind_names` pass; no new failures vs baseline.

- [ ] **Step 12: Commit**

```bash
git add -A
git commit -m "extension: drop empty GatewayExtension tier + orphan permissions

After withdrawing 5 dead capabilities, Tier::GatewayExtension has no
members and PluginPermission::{HttpRoutes,GatewayRpc} are unreferenced.
Remove them; keep unknown-permission parsing graceful. Per R10."
```

---

## Task 8: Final verification

**Files:** none modified (formatting only).

- [ ] **Step 1: Full residue sweep**

Run from the worktree root:

```bash
rg -n 'ChannelRegistration|ProviderRegistration|GatewayMethodRegistration|HttpRouteRegistration|HttpHandlerRegistration|CliRegistration|ChannelManager|PluginProviderAdapter|PluginHttpHandler|GatewayExtension' src
```

Expected: no hits anywhere in `src/` (gateway's own unrelated `ChannelManager` — confirm it is a distinct type in `src/gateway/`, which is acceptable; the extension types must be fully gone).

- [ ] **Step 2: Workspace build**

Run: `cargo build -p alephcore 2>&1 | tail -15`
Expected: `Finished`.

- [ ] **Step 3: Full lib test suite**

Run: `cargo test -p alephcore --lib 2>&1 | tail -30`
Expected: failing-test set ⊆ Task 1 baseline. **No new failures.** If any new failure appears, investigate and fix before proceeding.

- [ ] **Step 4: Format + lint changed files only**

```bash
git diff --name-only main...HEAD -- '*.rs' | xargs -r cargo fmt --
git diff --name-only main...HEAD -- '*.rs'   # confirm fmt produced no surprises
cargo clippy -p alephcore 2>&1 | rg -n 'warning|error' | tail -20
```

Expected: `cargo fmt` touches only changed files; no NEW clippy warnings introduced by this branch (the project has pre-existing clippy noise — compare against baseline if unsure).

- [ ] **Step 5: Commit any formatting**

```bash
git add -A
git commit -m "extension: cargo fmt on consolidation changes" --allow-empty
```

- [ ] **Step 6: Report**

Summarize: lines removed (`git diff --shortstat main...HEAD`), capabilities withdrawn (5), final capability count (7), test delta vs baseline (must be zero new failures).

---

## Self-Review

**Spec coverage:** Spec §4.1 Channel→Task 2, Provider→Task 3, GatewayMethod→Task 4, Cli→Task 5, HttpRoute+HttpHandler→Task 6. Spec §4.2 cross-cutting (Tier, PluginPermission, doc comments, the variant-count test)→Task 7. Spec §4.3 external recheck→Task 1. Spec §8 verification→Task 8. Spec §5 deferred work (McpServer, plugin channels) is explicitly out of scope — no task, by design. ✔

**Placeholder scan:** No TBD/TODO. Task 7 Step 7's test names a parser function `parse_permissions_from_strings` with an explicit instruction to match the real signature after reading the file — this is a deliberate adapt-to-actual instruction, acceptable. ✔

**Type consistency:** Symbol names (`ChannelRegistration`, `register_channel`, `channel_ids`, `gateway_methods`, `RegistryStats`, `Tier::GatewayExtension`, `PluginPermission::{HttpRoutes,GatewayRpc}`, `CapabilityDeclaration` variants) verified against the current source in `capability.rs`, `registry/types.rs`, `registry/plugin_registry/mod.rs`, `types/plugins.rs`, `types/runtime.rs`, `error.rs`. ✔

**Note on Task 2 Step 2:** the capability-count assertion is decremented incrementally across Tasks 2–6 and finalized to 7 in Task 7 Step 9 — intentional, so each task leaves `cargo check`/tests green.
