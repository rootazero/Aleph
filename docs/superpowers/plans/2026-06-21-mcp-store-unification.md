# MCP Store Unification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Settings → MCP page read/write the live `McpManagerActor` store (`~/.aleph/mcp_config.json`) instead of the dead `config.unified_tools.mcp`, so MCP servers sync with the Hub and actually run.

**Architecture:** Repoint the `mcp_config.*` gateway handler bodies onto `McpManagerHandle` (+ vault for secrets); add one actor query (`list_server_configs`) so the Settings list can show editable command/args/env in a single call; migrate any legacy `config.unified_tools.mcp` entries into the actor store once at boot, then clear the source. Panel keys servers by `id` (Hub servers have derived ids). The existing `mcp.*` surface is untouched.

**Tech Stack:** Rust (alephcore lib + aleph-server bin), tokio actor, serde JSON-RPC, Leptos/WASM panel. Reuses `src/secrets/` + `src/hub/secrets.rs` vault pipeline.

**Spec:** `docs/superpowers/specs/2026-06-21-mcp-store-unification-design.md`

## Global Constraints

- **Single source of truth:** all MCP CRUD goes through `McpManagerHandle`; do not read or write `config.unified_tools.mcp` except in the one-time migration.
- **Secrets:** reuse the vault only — `SharedTokenManager::store_secret(name, value)` + `crate::hub::secrets::{field_key, secret_ref}` + `ExtensionKind::Mcp`. Never write plaintext secrets into `mcp_config.json`. Never introduce a parallel secret scheme.
- **No new crates. No second async runtime. No platform-API crate.** (Tech-stack guardrails.)
- **R4 / R7:** handlers stay pure I/O (DTO map + handle/vault calls); no business logic, no LLM, no regex intent parsing.
- **Gateway auth (`src/gateway/CLAUDE.md`):** `mcp_config.*` stay post-connect config RPCs; method names and reachability are unchanged, so the trust boundary is unchanged. Do not alter auth/authz/origin code. If you touch any auth code, update its tests.
- **MSRV = 1.95** (pinned toolchain `1.96.0`); no version bumps.
- **Build is memory-heavy (rustc OOMs on parallel builds).** Be frugal: run **scoped** tests per task (`cargo test -p alephcore --lib <module>`), and at most one `cargo check -p alephcore --bin aleph-server` plus one `cargo check -p aleph-panel --target wasm32-unknown-unknown` for the whole plan. Do NOT run the full test suite (`tests/cancellation_chain.rs` is pre-existing-broken).
- **CRLF:** repo files are CRLF on Windows; `git` will warn on LF→CRLF — ignore.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `src/mcp/manager/types.rs` | actor command enum | Add `McpCommand::ListServerConfigs` + Debug arm |
| `src/mcp/manager/actor.rs` | actor command dispatch | Handle `ListServerConfigs` (read `self.config.servers`) |
| `src/mcp/manager/handle.rs` | public actor API | Add `list_server_configs()` |
| `src/gateway/handlers/mcp_config.rs` | Settings MCP RPC | Rewrite bodies onto `McpManagerHandle` + vault; add `id` to DTO, drop `cwd`; add pure helpers + `migrate_unified_to_actor` |
| `src/bin/aleph-server/commands/start/builder/handlers/mcp.rs` | actor-backed handler registration | Add `register_mcp_config_handlers` |
| `src/bin/aleph-server/commands/start/builder/handlers/settings.rs` | config RPC registration | Remove the `mcp_config.*` macro block |
| `src/bin/aleph-server/commands/start/builder/mod.rs` | builder re-exports | Export `register_mcp_config_handlers` (next to `register_mcp_handlers`) |
| `src/bin/aleph-server/commands/start/mod.rs` | boot wiring | Call migration (after `:420`) + `register_mcp_config_handlers` (in the `:1311` block) |
| `interfaces/webchat/src/api/mcp.rs` | panel RPC client + DTO | Add `id` to `McpServerInfo`, drop `cwd`; `get/update/delete` key by `id` |
| `interfaces/webchat/src/views/settings/mcp.rs` | panel MCP page | Key cards/dialog by `id`; display `name` |

---

## Task 1: Actor `list_server_configs` query

**Files:**
- Modify: `src/mcp/manager/types.rs` (enum `McpCommand` + its `Debug` impl)
- Modify: `src/mcp/manager/actor.rs:305-379` (the `handle_command` match)
- Modify: `src/mcp/manager/handle.rs` (add method)
- Test: `src/mcp/manager/actor.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `McpManagerHandle::list_server_configs(&self) -> crate::error::Result<Vec<McpManagerConfig>>` — full persisted configs (id, name, transport, command, args, url, env, requires_runtime, auto_start, timeout_seconds, tool_filter), excluding transient plugin-owned servers. Consumed by Task 2 (`handle_list`/`handle_get`/`handle_update`) and Task 4 (migration dup-check).

- [ ] **Step 1: Add the command variant.** In `src/mcp/manager/types.rs`, inside `pub enum McpCommand`, after the `ListServers { .. }` variant (around line 503):

```rust
    /// List all **persisted** server configurations (full config, not the
    /// lightweight `McpServerInfo`). Excludes transient plugin-owned servers,
    /// which live only in `clients`. Used by the Settings MCP page to render
    /// editable command/args/env without per-server status round-trips.
    ListServerConfigs {
        /// Response channel
        respond_to: oneshot::Sender<Vec<McpManagerConfig>>,
    },
```

- [ ] **Step 2: Add the Debug arm.** In the `impl std::fmt::Debug for McpCommand` match (around line 603, next to `Self::ListServers { .. } => ...`):

```rust
            Self::ListServerConfigs { .. } => f.debug_struct("ListServerConfigs").finish(),
```

- [ ] **Step 3: Dispatch it in the actor.** In `src/mcp/manager/actor.rs`, in `handle_command`, after the `McpCommand::ListServers { respond_to } => { ... }` arm (around line 360):

```rust
            McpCommand::ListServerConfigs { respond_to } => {
                let configs = self.config.servers.values().cloned().collect::<Vec<_>>();
                let _ = respond_to.send(configs);
            }
```

- [ ] **Step 4: Add the handle method.** In `src/mcp/manager/handle.rs`, after `list_servers` (around line 230):

```rust
    /// List all persisted server configurations (full config).
    ///
    /// Unlike [`Self::list_servers`] (lightweight `McpServerInfo`), this returns
    /// the complete `McpManagerConfig` for each persisted server — enough to
    /// render and edit command/args/env. Transient plugin-owned servers are not
    /// included.
    pub async fn list_server_configs(&self) -> Result<Vec<McpManagerConfig>> {
        let (respond_to, rx) = oneshot::channel();

        self.tx
            .send(McpCommand::ListServerConfigs { respond_to })
            .await
            .map_err(|_| AlephError::channel_closed("McpManager command channel closed"))?;

        rx.await
            .map_err(|_| AlephError::channel_closed("McpManager response channel closed"))
    }
```

- [ ] **Step 5: Write the integration test.** In `src/mcp/manager/actor.rs` `mod tests`, add (uses `auto_start=false` so no child process spawns; `std::process::id()` keeps the temp path unique across concurrent test binaries):

```rust
    #[tokio::test]
    async fn list_server_configs_returns_persisted_configs() {
        let path = std::env::temp_dir().join(format!("aleph_mcp_cfgs_{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let (actor, handle) = McpManagerActor::new(Some(path.clone()))
            .await
            .expect("actor builds");
        tokio::spawn(actor.run());

        handle
            .add_server(
                McpManagerConfig::stdio("srv-a", "Server A", "/bin/true").with_auto_start(false),
            )
            .await
            .expect("add_server");

        let configs = handle.list_server_configs().await.expect("list configs");
        assert!(configs.iter().any(|c| c.id == "srv-a" && c.name == "Server A"));

        let _ = std::fs::remove_file(&path);
    }
```

(If `McpManagerActor`/`McpManagerConfig` aren't already in the test module's scope, add `use super::*;` is already present at the top of `mod tests` — confirm and reuse it.)

- [ ] **Step 6: Run the scoped test.**

Run: `cargo test -p alephcore --lib mcp::manager::actor::tests::list_server_configs_returns_persisted_configs`
Expected: PASS (1 passed).

- [ ] **Step 7: Commit.**

```bash
git add src/mcp/manager/types.rs src/mcp/manager/actor.rs src/mcp/manager/handle.rs
git commit -m "mcp: add list_server_configs actor query"
```

---

## Task 2: Repoint `mcp_config.*` handlers onto the actor store

**Files:**
- Modify (rewrite): `src/gateway/handlers/mcp_config.rs`
- Test: same file (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `McpManagerHandle::{list_server_configs, add_server, remove_server}` (Task 1); `SharedTokenManager::store_secret`; `crate::hub::secrets::{field_key, secret_ref}`; `crate::hub::types::ExtensionKind`.
- Produces (new handler signatures, consumed by Task 3's registration):
  - `handle_list(req, mcp: McpManagerHandle) -> JsonRpcResponse`
  - `handle_get(req, mcp: McpManagerHandle) -> JsonRpcResponse`
  - `handle_create(req, mcp: McpManagerHandle, vault: Arc<SharedTokenManager>, event_bus: Arc<GatewayEventBus>) -> JsonRpcResponse`
  - `handle_update(req, mcp: McpManagerHandle, vault: Arc<SharedTokenManager>, event_bus: Arc<GatewayEventBus>) -> JsonRpcResponse`
  - `handle_delete(req, mcp: McpManagerHandle, event_bus: Arc<GatewayEventBus>) -> JsonRpcResponse`
  - `pub(crate) fn derive_server_id(name: &str) -> String`
  - `pub(crate) fn plan_secret_env(id, incoming, existing) -> (HashMap<String,String>, Vec<(String,String)>)` (used by Task 4)

- [ ] **Step 1: Replace the file header + imports.** Replace lines 1-16 of `src/gateway/handlers/mcp_config.rs` with:

```rust
//! MCP Configuration RPC Handlers (`mcp_config.*`)
//!
//! The Settings → MCP page's CRUD surface. These handlers operate on the live
//! `McpManagerActor` store (`~/.aleph/mcp_config.json`) — the same store the Hub
//! and the runtime use — so servers added here actually run and stay in sync
//! with the Hub. Secret-looking env vars are stored in the vault as
//! `{{secret:NAME}}` references (never plaintext on disk), mirroring the Hub
//! install path.

use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use tracing::{error, info};

use super::super::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::parse_params;
use crate::gateway::security::SharedTokenManager;
use crate::hub::secrets::{field_key, secret_ref};
use crate::hub::types::ExtensionKind;
use crate::mcp::manager::{McpManagerConfig, McpManagerHandle};
```

- [ ] **Step 2: Update the read DTO** — add `id`, drop `cwd`. Replace the `McpServerInfo` struct (old lines 23-36):

```rust
/// MCP server info for JSON serialization (panel read DTO).
#[derive(Debug, Clone, Serialize)]
pub struct McpServerInfo {
    /// Stable server id (actor key). Hub-installed servers carry derived ids
    /// like `aleph-hub_github`; Settings-created servers derive theirs from the
    /// name. The panel keys/edits/deletes by this.
    pub id: String,
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_runtime: Option<String>,
}
```

Leave `McpServerConfigJson` (old lines 39-56) unchanged — it deserializes the panel write DTO (`command`/`args`/`env`); its `cwd`/`triggers` fields are simply unused now.

- [ ] **Step 3: Keep `is_secret_env_key` + `redact_secret_env`; replace `merge_secret_env` with `plan_secret_env` and add `derive_server_id` + `info_from_config`.** Keep old lines 65-85 (`is_secret_env_key`, `redact_secret_env`) as-is. Delete `merge_secret_env` (old lines 91-106) and insert:

```rust
/// Derive a deterministic, placeholder-safe server id from a user-given name.
/// Mirrors the charset enforced by `crate::secrets::extract_secret_refs`.
pub(crate) fn derive_server_id(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Plan how an incoming env (from the Panel) is persisted.
///
/// Returns `(env_to_store, vault_writes)`:
/// - secret-looking key + non-blank value → store the value in the vault and
///   write a `{{secret:NAME}}` reference into the env; the (name, value) pair is
///   returned in `vault_writes` for the caller to persist.
/// - secret-looking key + blank value → keep the existing stored ref (stable
///   echo: blank means "unchanged"); dropped if there was none.
/// - non-secret key → plaintext, unchanged.
pub(crate) fn plan_secret_env(
    id: &str,
    incoming: HashMap<String, String>,
    existing: &HashMap<String, String>,
) -> (HashMap<String, String>, Vec<(String, String)>) {
    let mut env = HashMap::new();
    let mut writes = Vec::new();
    for (k, v) in incoming {
        if is_secret_env_key(&k) {
            if v.is_empty() {
                if let Some(prev) = existing.get(&k) {
                    env.insert(k, prev.clone());
                }
            } else {
                let name = field_key(ExtensionKind::Mcp, id, &k);
                env.insert(k, secret_ref(&name));
                writes.push((name, v));
            }
        } else {
            env.insert(k, v);
        }
    }
    (env, writes)
}

/// Build the panel read DTO from an actor config. Secret env values are blanked
/// for display (the stored `{{secret:..}}` ref never leaves the host); the keys
/// stay so the panel shows the var is configured.
fn info_from_config(cfg: &McpManagerConfig) -> McpServerInfo {
    McpServerInfo {
        id: cfg.id.clone(),
        name: cfg.name.clone(),
        command: cfg.command.clone().unwrap_or_default(),
        args: cfg.args.clone(),
        env: redact_secret_env(&cfg.env),
        enabled: cfg.auto_start,
        requires_runtime: cfg.requires_runtime.clone(),
    }
}
```

- [ ] **Step 4: Rewrite `handle_list` + `handle_get`.** Replace old `handle_list` (113-150) and `handle_get` (156-211):

```rust
// ============================================================================
// List
// ============================================================================

/// List all MCP servers (persisted actor configs).
pub async fn handle_list(request: JsonRpcRequest, mcp: McpManagerHandle) -> JsonRpcResponse {
    match mcp.list_server_configs().await {
        Ok(configs) => {
            let servers: Vec<McpServerInfo> = configs.iter().map(info_from_config).collect();
            JsonRpcResponse::success(request.id, json!({ "servers": servers }))
        }
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

// ============================================================================
// Get
// ============================================================================

/// Parameters for `mcp_config.get`
#[derive(Debug, Deserialize)]
pub struct GetParams {
    pub id: String,
}

/// Get a single MCP server by id.
pub async fn handle_get(request: JsonRpcRequest, mcp: McpManagerHandle) -> JsonRpcResponse {
    let params: GetParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let configs = match mcp.list_server_configs().await {
        Ok(c) => c,
        Err(e) => return JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    };
    match configs.iter().find(|c| c.id == params.id) {
        Some(cfg) => {
            JsonRpcResponse::success(request.id, json!({ "server": info_from_config(cfg) }))
        }
        None => JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("MCP server not found: {}", params.id),
        ),
    }
}
```

- [ ] **Step 5: Rewrite `handle_create`.** Replace old `CreateParams` + `handle_create` (217-299):

```rust
// ============================================================================
// Create
// ============================================================================

/// Parameters for `mcp_config.create`
#[derive(Debug, Deserialize)]
pub struct CreateParams {
    pub name: String,
    pub config: McpServerConfigJson,
}

/// Broadcast a `ConfigChanged(section="mcp")` event (best-effort, keeps panel
/// live-refresh subscribers working).
fn publish_mcp_change(event_bus: &GatewayEventBus, action: &str, server: &str) {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("mcp".to_string()),
        value: json!({ "action": action, "server": server }),
        timestamp,
    });
    if let Err(e) = event_bus.publish_json(&event) {
        error!(error = %e, "Failed to broadcast MCP config event");
    }
}

/// Create a new MCP server in the actor store.
pub async fn handle_create(
    request: JsonRpcRequest,
    mcp: McpManagerHandle,
    vault: Arc<SharedTokenManager>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: CreateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let id = derive_server_id(&params.name);

    // Duplicate check against the live store.
    match mcp.list_server_configs().await {
        Ok(configs) if configs.iter().any(|c| c.id == id) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("MCP server already exists: {}", params.name),
            );
        }
        Ok(_) => {}
        Err(e) => return JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }

    // Route secret env vars into the vault; build the env with `{{secret:..}}`.
    let (env, writes) = plan_secret_env(&id, params.config.env, &HashMap::new());
    for (name, value) in &writes {
        if let Err(e) = vault.store_secret(name, value) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to store secret {name}: {e}"),
            );
        }
    }

    let mut cfg = McpManagerConfig::stdio(&id, &params.name, params.config.command)
        .with_args(params.config.args)
        .with_env(env)
        .with_auto_start(params.config.enabled.unwrap_or(true));
    if let Some(rt) = params.config.requires_runtime {
        cfg = cfg.with_runtime(rt);
    }
    if let Some(t) = params.config.timeout_seconds {
        cfg = cfg.with_timeout(t);
    }

    if let Err(e) = mcp.add_server(cfg).await {
        return JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string());
    }

    publish_mcp_change(&event_bus, "created", &params.name);
    info!(id = %id, name = %params.name, "MCP server created");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}
```

- [ ] **Step 6: Rewrite `handle_update`.** Replace old `UpdateParams` + `handle_update` (305-395):

```rust
// ============================================================================
// Update
// ============================================================================

/// Parameters for `mcp_config.update`
#[derive(Debug, Deserialize)]
pub struct UpdateParams {
    pub id: String,
    pub config: McpServerConfigJson,
}

/// Update an MCP server in the actor store (restart with new config).
pub async fn handle_update(
    request: JsonRpcRequest,
    mcp: McpManagerHandle,
    vault: Arc<SharedTokenManager>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: UpdateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let configs = match mcp.list_server_configs().await {
        Ok(c) => c,
        Err(e) => return JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    };
    let Some(existing) = configs.into_iter().find(|c| c.id == params.id) else {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("MCP server not found: {}", params.id),
        );
    };

    // Blank secrets keep the stored ref; new values rotate into the vault.
    let (env, writes) = plan_secret_env(&params.id, params.config.env, &existing.env);
    for (name, value) in &writes {
        if let Err(e) = vault.store_secret(name, value) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to store secret {name}: {e}"),
            );
        }
    }

    // Preserve transport/url/auto_start/timeout/tool_filter; for stdio servers
    // also update command/args/requires_runtime from the panel. A remote
    // (url-bearing) server is env-only-editable here so its url is never lost.
    let is_remote = existing.url.is_some();
    let mut new_cfg = existing.clone();
    new_cfg.env = env;
    if !is_remote {
        new_cfg.command = Some(params.config.command);
        new_cfg.args = params.config.args;
        new_cfg.requires_runtime = params.config.requires_runtime;
    }

    // Restart cleanly so the running client picks up the new config.
    let _ = mcp.remove_server(&params.id).await;
    if let Err(e) = mcp.add_server(new_cfg).await {
        return JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string());
    }

    publish_mcp_change(&event_bus, "updated", &existing.name);
    info!(id = %params.id, "MCP server updated");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}
```

- [ ] **Step 7: Rewrite `handle_delete`.** Replace old `DeleteParams` + `handle_delete` (401-471):

```rust
// ============================================================================
// Delete
// ============================================================================

/// Parameters for `mcp_config.delete`
#[derive(Debug, Deserialize)]
pub struct DeleteParams {
    pub id: String,
}

/// Delete an MCP server from the actor store.
pub async fn handle_delete(
    request: JsonRpcRequest,
    mcp: McpManagerHandle,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: DeleteParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    if let Err(e) = mcp.remove_server(&params.id).await {
        return JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string());
    }

    publish_mcp_change(&event_bus, "deleted", &params.id);
    info!(id = %params.id, "MCP server deleted");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}
```

- [ ] **Step 8: Update the unit tests.** In `mod tests` (old 473-536): keep `secret_keys_detected_case_insensitively` and `redact_blanks_secrets_keeps_keys_and_nonsecrets`. Delete the three `merge_*` tests (they referenced the removed `merge_secret_env`). Add:

```rust
    #[test]
    fn derive_server_id_sanitizes_to_placeholder_safe() {
        assert_eq!(super::derive_server_id("My Server"), "My_Server");
        assert_eq!(super::derive_server_id("a:b/c"), "a_b_c");
        assert_eq!(super::derive_server_id("github.mcp-1"), "github.mcp-1");
    }

    #[test]
    fn plan_secret_env_routes_secret_to_vault_ref() {
        let incoming = env(&[("GITHUB_TOKEN", "ghp_real"), ("REGION", "us")]);
        let (stored, writes) = super::plan_secret_env("srv", incoming, &HashMap::new());
        // secret value never stored inline; a {{secret:..}} ref is written instead
        assert_eq!(
            stored.get("GITHUB_TOKEN"),
            Some(&"{{secret:ext.mcp.srv.GITHUB_TOKEN}}".to_string())
        );
        assert_eq!(stored.get("REGION"), Some(&"us".to_string()));
        assert_eq!(writes, vec![("ext.mcp.srv.GITHUB_TOKEN".to_string(), "ghp_real".to_string())]);
    }

    #[test]
    fn plan_secret_env_blank_secret_keeps_existing_ref() {
        let existing = env(&[("API_KEY", "{{secret:ext.mcp.srv.API_KEY}}")]);
        let incoming = env(&[("API_KEY", "")]); // panel echoes blank for an unchanged secret
        let (stored, writes) = super::plan_secret_env("srv", incoming, &existing);
        assert_eq!(
            stored.get("API_KEY"),
            Some(&"{{secret:ext.mcp.srv.API_KEY}}".to_string())
        );
        assert!(writes.is_empty(), "blank secret must not write to the vault");
    }
```

- [ ] **Step 9: Run the scoped tests.**

Run: `cargo test -p alephcore --lib gateway::handlers::mcp_config`
Expected: PASS (all tests in the module pass; ensure no leftover references to `merge_secret_env`, `Config`, or `cwd`).

- [ ] **Step 10: Commit.**

```bash
git add src/gateway/handlers/mcp_config.rs
git commit -m "gateway: repoint mcp_config.* onto the live MCP actor store with vault secrets"
```

---

## Task 3: Re-wire registration (actor-backed) + drop the dead config-backed registration

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/mcp.rs` (add `register_mcp_config_handlers`)
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/settings.rs:384-407` (remove `mcp_config.*` block)
- Modify: `src/bin/aleph-server/commands/start/builder/mod.rs` (re-export)
- Modify: `src/bin/aleph-server/commands/start/mod.rs:1310-1314` (call new registration)

**Interfaces:**
- Consumes: Task 2 handler signatures; `McpManagerHandle`; `SharedTokenManager`; `GatewayEventBus`.
- Produces: `register_mcp_config_handlers(server, handle, vault, event_bus)`, called from the `if let Some(ref h) = mcp_handle` block.

- [ ] **Step 1: Add `register_mcp_config_handlers`.** Append to `src/bin/aleph-server/commands/start/builder/handlers/mcp.rs`:

```rust
/// Register the Settings-page MCP CRUD handlers (`mcp_config.*`) against the
/// live [`McpManagerHandle`] + vault. Like [`register_mcp_handlers`], these use
/// manual closures (the handle is not `Arc`-wrapped). Registered only when the
/// MCP actor spawned; if it did not, the Settings MCP page returns
/// method-not-found — consistent with MCP being unavailable that run.
pub(in crate::commands::start) fn register_mcp_config_handlers(
    server: &mut GatewayServer,
    handle: &McpManagerHandle,
    vault: alephcore::sync_primitives::Arc<alephcore::gateway::security::SharedTokenManager>,
    event_bus: alephcore::sync_primitives::Arc<alephcore::gateway::event_bus::GatewayEventBus>,
) {
    use alephcore::gateway::handlers::mcp_config;

    {
        let handle = handle.clone();
        server.handlers_mut().register("mcp_config.list", move |req| {
            let handle = handle.clone();
            async move { mcp_config::handle_list(req, handle).await }
        });
    }
    {
        let handle = handle.clone();
        server.handlers_mut().register("mcp_config.get", move |req| {
            let handle = handle.clone();
            async move { mcp_config::handle_get(req, handle).await }
        });
    }
    {
        let handle = handle.clone();
        let vault = vault.clone();
        let event_bus = event_bus.clone();
        server.handlers_mut().register("mcp_config.create", move |req| {
            let handle = handle.clone();
            let vault = vault.clone();
            let event_bus = event_bus.clone();
            async move { mcp_config::handle_create(req, handle, vault, event_bus).await }
        });
    }
    {
        let handle = handle.clone();
        let vault = vault.clone();
        let event_bus = event_bus.clone();
        server.handlers_mut().register("mcp_config.update", move |req| {
            let handle = handle.clone();
            let vault = vault.clone();
            let event_bus = event_bus.clone();
            async move { mcp_config::handle_update(req, handle, vault, event_bus).await }
        });
    }
    {
        let handle = handle.clone();
        let event_bus = event_bus.clone();
        server.handlers_mut().register("mcp_config.delete", move |req| {
            let handle = handle.clone();
            let event_bus = event_bus.clone();
            async move { mcp_config::handle_delete(req, handle, event_bus).await }
        });
    }
}
```

> Note: confirm `McpManagerHandle` is already imported at the top of this file (it is: `use alephcore::mcp::McpManagerHandle;`). The `Arc` path here is fully-qualified to avoid relying on a local alias.

- [ ] **Step 2: Re-export it.** In `src/bin/aleph-server/commands/start/builder/mod.rs`, find the `pub use` line that exports `register_mcp_handlers` and add `register_mcp_config_handlers` next to it. (Search for `register_mcp_handlers` in that file; add the new name to the same `pub use ...::{...}` group.)

- [ ] **Step 3: Remove the dead config-backed registration.** In `src/bin/aleph-server/commands/start/builder/handlers/settings.rs`, delete the entire `// MCP config` block (old lines 384-407 — the five `register_handler!(... mcp_config::...)` calls). Leave the surrounding `// Routing rules` and `// Memory config` blocks intact.

- [ ] **Step 4: Call the new registration at boot.** In `src/bin/aleph-server/commands/start/mod.rs`, in the `if let Some(ref h) = mcp_handle {` block (around line 1310), after `register_mcp_handlers(&mut server, h);`:

```rust
        register_mcp_config_handlers(
            &mut server,
            h,
            auth_bundle.auth_ctx.shared_token_mgr.clone(),
            event_bus.clone(),
        );
```

Then add `register_mcp_config_handlers` to the `use builder::{ ... }` import list at the top of `mod.rs` (next to `register_mcp_handlers`).

> Verify `event_bus` is the in-scope `Arc<GatewayEventBus>` already passed to `register_config_handlers` earlier in this function (it is — same variable). If the local name differs, use that name.

- [ ] **Step 5: Compile-check the binary (single check for Tasks 1-4).** Defer running this until after Task 4 so one check covers both. (Placeholder reminder: do not run yet.)

- [ ] **Step 6: Commit.**

```bash
git add src/bin/aleph-server/commands/start/builder/handlers/mcp.rs \
        src/bin/aleph-server/commands/start/builder/handlers/settings.rs \
        src/bin/aleph-server/commands/start/builder/mod.rs \
        src/bin/aleph-server/commands/start/mod.rs
git commit -m "server: register mcp_config.* against the MCP actor; drop config-backed registration"
```

---

## Task 4: One-time migration of `config.unified_tools.mcp` → actor store

**Files:**
- Modify: `src/gateway/handlers/mcp_config.rs` (add `migrate_unified_to_actor`)
- Modify: `src/bin/aleph-server/commands/start/mod.rs` (call after `:420`)
- Test: `src/gateway/handlers/mcp_config.rs` (`mod tests`) — pure conversion only

**Interfaces:**
- Consumes: `plan_secret_env`, `derive_server_id` (Task 2); `McpManagerHandle::{list_server_configs, add_server}`; `Config::save_incremental`.
- Produces: `pub async fn migrate_unified_to_actor(config: &Arc<tokio::sync::RwLock<Config>>, mcp: &McpManagerHandle, vault: &Arc<SharedTokenManager>)`.

- [ ] **Step 1: Add a pure conversion helper + the migration fn.** Append to `src/gateway/handlers/mcp_config.rs` (before `#[cfg(test)] mod tests`):

```rust
// ============================================================================
// One-time migration: config.unified_tools.mcp -> actor store
// ============================================================================

/// Build an actor `McpManagerConfig` from a legacy `unified_tools.mcp` entry,
/// routing secret env vars into the vault. Returns the config plus the vault
/// writes the caller must persist. Pure (no I/O) so it is unit-testable.
pub(crate) fn unified_entry_to_manager_config(
    name: &str,
    sc: &crate::config::McpServerConfig,
) -> (McpManagerConfig, Vec<(String, String)>) {
    let id = derive_server_id(name);
    let (env, writes) = plan_secret_env(&id, sc.env.clone(), &HashMap::new());
    let mut cfg = McpManagerConfig::stdio(&id, name, sc.command.clone())
        .with_args(sc.args.clone())
        .with_env(env)
        .with_auto_start(sc.enabled)
        .with_timeout(sc.timeout_seconds);
    if let Some(rt) = sc.requires_runtime.clone() {
        cfg = cfg.with_runtime(rt);
    }
    (cfg, writes)
}

/// Migrate any legacy `config.unified_tools.mcp` servers into the live actor
/// store, then clear the migrated entries from `config.toml`. Idempotent: an
/// entry whose derived id already exists in the actor store is treated as
/// migrated (cleared, not re-added). Best-effort: failures are warn-logged and
/// leave the source entry in place; boot continues regardless.
pub async fn migrate_unified_to_actor(
    config: &Arc<tokio::sync::RwLock<crate::config::Config>>,
    mcp: &McpManagerHandle,
    vault: &Arc<SharedTokenManager>,
) {
    // Snapshot the legacy entries under a read lock.
    let entries: Vec<(String, crate::config::McpServerConfig)> = {
        let cfg = config.read().await;
        match &cfg.unified_tools {
            Some(u) if !u.mcp.is_empty() => {
                u.mcp.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
            }
            _ => return,
        }
    };

    let existing_ids: std::collections::HashSet<String> = mcp
        .list_server_configs()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|c| c.id)
        .collect();

    let mut migrated: Vec<String> = Vec::new();
    for (name, sc) in entries {
        let id = derive_server_id(&name);
        if existing_ids.contains(&id) {
            // Actor store already has it (e.g. Hub-installed) — clear the dup.
            migrated.push(name);
            continue;
        }
        let (cfg, writes) = unified_entry_to_manager_config(&name, &sc);
        let mut ok = true;
        for (vn, vv) in &writes {
            if let Err(e) = vault.store_secret(vn, vv) {
                tracing::warn!(server = %name, error = %e, "mcp migration: vault store failed; leaving source entry");
                ok = false;
                break;
            }
        }
        if !ok {
            continue;
        }
        match mcp.add_server(cfg).await {
            Ok(()) => {
                info!(id = %id, name = %name, "mcp migration: imported into actor store");
                migrated.push(name);
            }
            Err(e) => {
                tracing::warn!(server = %name, error = %e, "mcp migration: add_server failed; leaving source entry");
            }
        }
    }

    // Clear migrated entries from config.toml (prevents resurrection on delete).
    if !migrated.is_empty() {
        let mut cfg = config.write().await;
        if let Some(u) = cfg.unified_tools.as_mut() {
            for name in &migrated {
                u.mcp.remove(name);
            }
        }
        if let Err(e) = cfg.save_incremental(&["unified_tools"]) {
            tracing::warn!(error = %e, "mcp migration: failed to persist cleared unified_tools.mcp");
        }
    }
}
```

- [ ] **Step 2: Call the migration at boot.** In `src/bin/aleph-server/commands/start/mod.rs`, immediately after the actor run-loop spawn block (the `if let Some(actor) = mcp_actor_pending { ... tokio::spawn(actor.with_secret_resolver(resolver).run()); }` ending around line 421):

```rust
    // One-time migration of legacy Settings-page MCP servers
    // (config.unified_tools.mcp) into the live actor store. Runs after the actor
    // is running with its secret resolver, so imported {{secret:..}} servers can
    // resolve at auto-start. Warn-only; never aborts boot.
    if let Some(ref h) = mcp_handle {
        alephcore::gateway::handlers::mcp_config::migrate_unified_to_actor(
            &app_config,
            h,
            &auth_bundle.auth_ctx.shared_token_mgr,
        )
        .await;
    }
```

> `app_config` (`Arc<tokio::sync::RwLock<Config>>`) and `auth_bundle` are both in scope here (vault initialized at ~line 399). Confirm `app_config` is the live config Arc used by `register_config_handlers` — it is.

- [ ] **Step 3: Add a pure conversion test.** In `mod tests` of `src/gateway/handlers/mcp_config.rs`:

```rust
    #[test]
    fn unified_entry_converts_with_vault_secret() {
        let sc = crate::config::McpServerConfig {
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "@x/y".to_string()],
            env: env(&[("API_TOKEN", "t-real"), ("REGION", "us")]),
            cwd: None,
            requires_runtime: Some("node".to_string()),
            timeout_seconds: 30,
            enabled: false,
            triggers: None,
        };
        let (cfg, writes) = super::unified_entry_to_manager_config("My Srv", &sc);
        assert_eq!(cfg.id, "My_Srv");
        assert_eq!(cfg.name, "My Srv");
        assert_eq!(cfg.command.as_deref(), Some("npx"));
        assert!(!cfg.auto_start); // enabled=false -> auto_start=false
        assert_eq!(cfg.requires_runtime.as_deref(), Some("node"));
        // secret -> vault ref + a write; non-secret stays inline
        assert_eq!(
            cfg.env.get("API_TOKEN"),
            Some(&"{{secret:ext.mcp.My_Srv.API_TOKEN}}".to_string())
        );
        assert_eq!(cfg.env.get("REGION"), Some(&"us".to_string()));
        assert_eq!(writes, vec![("ext.mcp.My_Srv.API_TOKEN".to_string(), "t-real".to_string())]);
    }
```

- [ ] **Step 4: Run the scoped test.**

Run: `cargo test -p alephcore --lib gateway::handlers::mcp_config::tests::unified_entry_converts_with_vault_secret`
Expected: PASS.

> Note: the end-to-end migration (vault store + actor add + source clear) is verified in Task 6's runtime e2e — it needs a live actor + initialized vault, which a `--lib` unit test cannot set up cleanly.

- [ ] **Step 5: Single compile-check of the binary (covers Tasks 1-4).**

Run: `cargo check -p alephcore --bin aleph-server`
Expected: Finishes with no errors. (Memory-heavy; run once.)

- [ ] **Step 6: Commit.**

```bash
git add src/gateway/handlers/mcp_config.rs src/bin/aleph-server/commands/start/mod.rs
git commit -m "mcp: one-time migrate unified_tools.mcp into the actor store at boot"
```

---

## Task 5: Panel — key by `id`, drop `cwd`

**Files:**
- Modify: `interfaces/webchat/src/api/mcp.rs`
- Modify: `interfaces/webchat/src/views/settings/mcp.rs`

**Interfaces:**
- Consumes: server now returns `McpServerInfo { id, name, command, args, env, enabled, requires_runtime }` (Task 2) and accepts `id` (not `name`) for `mcp_config.get/update/delete`.

- [ ] **Step 1: Update the API DTO + methods.** In `interfaces/webchat/src/api/mcp.rs`, replace the `McpServerInfo` struct (lines 4-18) with (adds `id`, drops `cwd`):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerInfo {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Option<std::collections::HashMap<String, String>>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub requires_runtime: Option<String>,
}
```

Then change `get`/`update`/`delete` to key by `id` (rename the param the server expects from `name` to `id`; keep the Rust arg name or rename to `id` for clarity):

```rust
    /// Get a specific MCP server by id
    pub async fn get(state: &DashboardState, id: String) -> Result<McpServerInfo, String> {
        let params = serde_json::json!({ "id": id });
        let result = state.rpc_call("mcp_config.get", params).await?;
        let server = result.get("server").cloned().unwrap_or(result);
        serde_json::from_value(server).map_err(|e| format!("Failed to parse MCP server: {e}"))
    }

    /// Update an existing MCP server by id
    pub async fn update(
        state: &DashboardState,
        id: String,
        config: McpServerConfig,
    ) -> Result<(), String> {
        let params = serde_json::json!({ "id": id, "config": config });
        state.rpc_call("mcp_config.update", params).await?;
        Ok(())
    }

    /// Delete an MCP server by id
    pub async fn delete(state: &DashboardState, id: String) -> Result<(), String> {
        let params = serde_json::json!({ "id": id });
        state.rpc_call("mcp_config.delete", params).await?;
        Ok(())
    }
```

Leave `create` unchanged (it sends `name` + `config`; the server derives the id). `McpServerConfig` (write DTO) is unchanged.

- [ ] **Step 2: Key the list by id.** In `interfaces/webchat/src/views/settings/mcp.rs`, change the `<For>` key in `McpView` (line 149) from:

```rust
                                        key=|server| server.name.clone()
```
to:
```rust
                                        key=|server| server.id.clone()
```

- [ ] **Step 3: Card edits/deletes by id, displays name.** In `McpServerCard` (lines 195-302): replace the identity binding (line 206) and its uses. Change:

```rust
    let server_name = StoredValue::new(server.name.clone());
```
to:
```rust
    let server_id = StoredValue::new(server.id.clone());
```

In the Edit button `on:click` (line 261), change `editing_server.set(Some(server_name.get_value()));` to `editing_server.set(Some(server_id.get_value()));`.

In the Delete button `on:click` (lines 279-281), change `let name = server_name.get_value();` to `let id = server_id.get_value();` and `McpConfigApi::delete(&state, name)` to `McpConfigApi::delete(&state, id)`.

Leave the displayed `{server.name}` (line 230) as-is — the card still shows the human name.

- [ ] **Step 4: Dialog loads by id, updates by id.** In `EditMcpServerDialog` (lines 304-575): `editing_server` now holds the **id**.
  - The load block (line 324) `if let Some(server_name) = editing_server.get()` → rename the binding to `server_id` and call `McpConfigApi::get(&state_clone, server_id)`. The loaded `server.name` still populates the `name` signal for display (line 329) — unchanged.
  - In `handle_save` (lines 400-405), the update branch must send the id, not the typed name. Capture the id before the async move and use it:

```rust
        let editing_id = editing_server.get(); // Some(id) when editing, None when new
        spawn_local(async move {
            let result = if is_new {
                McpConfigApi::create(&state, server_name, config).await
            } else {
                let id = editing_id.unwrap_or_default();
                McpConfigApi::update(&state, id, config).await
            };
            // ... unchanged match on result ...
        });
```

  (The `name` field stays read-only on edit — lines 438 `disabled=move || !is_new` — so the display name and id remain consistent.)

- [ ] **Step 5: Compile-check the panel (WASM).**

Run: `cargo check -p aleph-panel --target wasm32-unknown-unknown`
Expected: Finishes with no errors. (Run once.)

- [ ] **Step 6: Commit.**

```bash
git add interfaces/webchat/src/api/mcp.rs interfaces/webchat/src/views/settings/mcp.rs
git commit -m "panel: key Settings MCP servers by id; drop cwd"
```

---

## Task 6: Runtime end-to-end verification

**Files:** none (verification only).

**Goal:** prove the unified store: Settings create → runs + visible to Hub; Hub install → visible in Settings; Settings delete → drops from both; migration imports a legacy entry.

- [ ] **Step 1: Build the dev panel + a fresh server binary.**

```bash
just wasm
cargo build -p alephcore --bin aleph-server
```
Expected: both succeed. (Debug `rust-embed` reads `interfaces/webchat/dist/` from disk at runtime, so `just wasm` output is picked up live; the binary rebuild is needed because handler wiring changed.)

- [ ] **Step 2: Seed a legacy entry + start an isolated server.** Use a temp `ALEPH_HOME` for lock/config isolation and a spare port. NOTE: `~/.aleph/mcp_config.json` is `dirs::home_dir()`-based and is NOT isolated by `ALEPH_HOME` — back it up first and restore at the end.

```bash
# back up the real actor store (may be absent)
[ -f ~/.aleph/mcp_config.json ] && cp ~/.aleph/mcp_config.json /tmp/mcp_config.bak || echo "no real mcp_config.json"
rm -f ~/.aleph/mcp_config.json

# isolated config home with one legacy unified_tools.mcp server
export ALEPH_HOME=$(mktemp -d)
mkdir -p "$ALEPH_HOME/data"
cat > "$ALEPH_HOME/config.toml" <<'TOML'
[unified_tools.mcp.legacy-echo]
command = "/bin/true"
args = []
enabled = false
TOML

target/debug/aleph-server --port 18796 --bind 127.0.0.1 start &
sleep 3
```
Expected: server boots; logs show `mcp migration: imported into actor store` for `legacy-echo`.

- [ ] **Step 2 (Windows variant):** if running on Windows, use `pwsh`-equivalent paths: back up `$env:USERPROFILE\.aleph\mcp_config.json`, set `$env:ALEPH_HOME` to a temp dir, write `config.toml` with the same `[unified_tools.mcp.legacy-echo]` block (use `command = "cmd"`, `args = ["/c","exit"]`), then `target\debug\aleph-server.exe --port 18796 --bind 127.0.0.1 start`. The migration assertion is the same.

- [ ] **Step 3: Drive the gateway over WebSocket.** Write `/tmp/e2e-mcp.mjs` (Node global `WebSocket`; loopback ⇒ auto operator):

```js
const ws = new WebSocket("ws://127.0.0.1:18796/ws");
let id = 0;
const call = (method, params) => new Promise((res) => {
  const rid = String(++id);
  const onMsg = (e) => {
    const m = JSON.parse(e.data);
    if (m.id === rid) { ws.removeEventListener("message", onMsg); res(m); }
  };
  ws.addEventListener("message", onMsg);
  ws.send(JSON.stringify({ jsonrpc: "2.0", id: rid, method, params }));
});
ws.addEventListener("open", async () => {
  await call("connect", { device_id: "e2e", device_name: "e2e" });
  // 1) migration: legacy-echo should now be in the actor store via mcp_config.list
  console.log("LIST_AFTER_MIGRATION", JSON.stringify(await call("mcp_config.list", null)));
  // 2) Settings create
  await call("mcp_config.create", { name: "settings-srv", config: { command: "/bin/true", args: [], env: {} } });
  console.log("INSTALLED", JSON.stringify(await call("extensions.installed", {})));
  // 3) delete by id and confirm it drops
  await call("mcp_config.delete", { id: "settings-srv" });
  console.log("LIST_AFTER_DELETE", JSON.stringify(await call("mcp_config.list", null)));
  ws.close();
});
```
Run: `node /tmp/e2e-mcp.mjs`

Expected:
- `LIST_AFTER_MIGRATION` includes a server with `id: "legacy-echo"` (migration worked).
- `INSTALLED` (Hub view) includes `local:mcp:settings-srv` (Settings create is visible to the Hub).
- `LIST_AFTER_DELETE` does NOT include `settings-srv` (delete drops it).

- [ ] **Step 4: Confirm migration cleared the source.**

```bash
grep -c "legacy-echo" "$ALEPH_HOME/config.toml" || true
```
Expected: `0` (the migrated entry was removed from `config.toml`).

- [ ] **Step 5: Tear down + restore.**

```bash
kill %1 2>/dev/null || true
rm -rf "$ALEPH_HOME" /tmp/e2e-mcp.mjs
[ -f /tmp/mcp_config.bak ] && mv /tmp/mcp_config.bak ~/.aleph/mcp_config.json || rm -f ~/.aleph/mcp_config.json
unset ALEPH_HOME
```
Expected: isolated server stopped, real `~/.aleph/mcp_config.json` restored to its prior state, temp files gone. (Do NOT touch the user's real daemon.)

- [ ] **Step 6: Commit (nothing to commit — verification only).** If `interfaces/webchat/dist/*` changed from `just wasm`, leave it as a build artifact (do not commit unless asked).

---

## Self-Review

**1. Spec coverage:**
- "Repoint Settings onto actor store" → Tasks 2, 3.
- "A1 (keep mcp.* untouched, repoint mcp_config.*)" → Task 3 leaves `register_mcp_handlers` intact; only `mcp_config.*` moves.
- "Vault secrets" → Task 2 `plan_secret_env` + `store_secret`; covered for create/update/migration.
- "Migrate once + clear source" → Task 4.
- "Add `id` to panel DTO; enabled↔auto_start; drop cwd" → Tasks 2 (DTO/`info_from_config`/`auto_start`) + 5 (panel).
- "Preserve http/sse url on round-trip" → Task 2 `handle_update` `is_remote` guard.
- "Non-goals" (no `mcp.*`/`mcp_config.*` consolidation, no enabled-semantics unification, no toggle button, no Skills/Plugins change, no remote-header secrets) → not implemented, by design.
- "Auth unchanged" → Task 3 keeps method names/reachability; Global Constraints flags it.
- "Testing: lib unit + runtime e2e, scoped builds" → Tasks 1,2,4 lib tests; Task 6 e2e; frugal build commands.

**2. Placeholder scan:** No `TBD`/`TODO`/"handle edge cases"/"similar to". Every code step has complete code. (The only "TODO"-like text is inside quoted existing-code comments, not plan instructions.)

**3. Type consistency:**
- `McpManagerConfig` fields used (id, name, transport, command: `Option<String>`, args, url: `Option<String>`, env, requires_runtime, auto_start, timeout_seconds: `Option<u64>`, tool_filter) match `src/mcp/manager/types.rs`. Builder methods used (`stdio`, `with_args`, `with_env`, `with_auto_start`, `with_runtime`, `with_timeout`) all exist.
- `McpManagerHandle` methods used (`list_server_configs` [Task 1], `add_server`, `remove_server`) match `handle.rs`.
- Config types: `Config.unified_tools: Option<UnifiedToolsConfig>`, `UnifiedToolsConfig.mcp: HashMap<String, McpServerConfig>`, config `McpServerConfig { command: String, args, env: HashMap, cwd, requires_runtime, timeout_seconds: u64, enabled: bool, triggers }` match `src/config/types/tools.rs`. `Config::save_incremental(&["unified_tools"])` matches `src/config/save.rs`.
- `SharedTokenManager::store_secret(&self, name, value) -> Result<(), SharedTokenError>` — synchronous, matches `shared_token.rs:202`.
- `field_key(ExtensionKind::Mcp, id, key)` / `secret_ref(name)` produce `ext.mcp.{id}.{key}` / `{{secret:..}}` — test expectations match `src/hub/secrets.rs`.
- Panel `McpServerInfo` gains `id`, drops `cwd`; `McpConfigApi::{get,update,delete}` switch to `id`; `create` unchanged — consistent across api + view.
