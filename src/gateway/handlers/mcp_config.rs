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

// ============================================================================
// Types
// ============================================================================

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
    /// Invocation record for this server: call count, last-used date, idle
    /// days. `None` when the usage report could not be built at all — which is
    /// a different statement from a row whose `calls` is `Some(0)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<aleph_protocol::extension_usage::UsageSummary>,
}

/// MCP server config from JSON
#[derive(Debug, Clone, Deserialize)]
pub struct McpServerConfigJson {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub requires_runtime: Option<String>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

// ============================================================================
// Secret env redaction (stable-echo at the trust boundary)
// ============================================================================

/// Env var names matching these substrings (case-insensitive) are treated as
/// secrets: their values are redacted on read and preserved on blank update.
/// Mirrors the Panel's `is_secret_env_key` so masking is consistent both ways.
fn is_secret_env_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    ["KEY", "SECRET", "TOKEN", "PASSWORD", "PASS", "CREDENTIAL"]
        .iter()
        .any(|needle| upper.contains(needle))
}

/// Redact secret env values before sending to the Panel: secret-looking keys
/// keep their name but their value is blanked, so the stored secret never leaves
/// the host. Non-secret values (e.g. `DOMAIN`, `SERVICE_ID`) pass through.
fn redact_secret_env(env: &HashMap<String, String>) -> HashMap<String, String> {
    env.iter()
        .map(|(k, v)| {
            if is_secret_env_key(k) {
                (k.clone(), String::new())
            } else {
                (k.clone(), v.clone())
            }
        })
        .collect()
}

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
        usage: None,
    }
}

/// Fill in each row's `usage` from the shared report.
///
/// Joined by `id`, which is what the sidecar keys on — the same registry ids
/// this list is built from, so the two cannot drift the way a name-based join
/// would. A row keeps `usage: None` when the report has no entry for it, which
/// happens only if the inventory itself could not be read; that is deliberately
/// distinct from `Some(UsageSummary { calls: Some(0), .. })` — "unknown" versus
/// "known to be never used".
async fn attach_usage(servers: &mut [McpServerInfo], mcp: &McpManagerHandle) {
    // Aliased: this module already imports a DIFFERENT `ExtensionKind` at the
    // top (`hub::types`, the install-target discriminant). A bare `use` here
    // would shadow it inside this function only, which compiles and reads as
    // though the two were the same type.
    use crate::tools::usage::report::ExtensionKind as UsageKind;
    use aleph_protocol::extension_usage::UsageSummary;

    if servers.is_empty() {
        return;
    }
    // The handle is passed through (costing one extra actor round-trip on top
    // of the list we already have) rather than joining the sidecar here by
    // hand: re-deriving the join would be a second definition of what a row
    // means, and the `—`-vs-`0` distinction is exactly the thing that must have
    // only one.
    let report = crate::tools::usage::report::build_report_now(Some(mcp)).await;
    for row in servers.iter_mut() {
        if let Some(entry) = report
            .entries
            .iter()
            .find(|e| e.kind == UsageKind::Mcp && e.id == row.id)
        {
            row.usage = Some(UsageSummary::from(entry));
        }
    }
}

// ============================================================================
// List
// ============================================================================

/// List all MCP servers (persisted actor configs).
pub async fn handle_list(request: JsonRpcRequest, mcp: McpManagerHandle) -> JsonRpcResponse {
    match mcp.list_server_configs().await {
        Ok(configs) => {
            let mut servers: Vec<McpServerInfo> = configs.iter().map(info_from_config).collect();
            attach_usage(&mut servers, &mcp).await;
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
    if let Err(e) = event_bus.publish_gateway_event(&event) {
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
    // Ignore removal error — the server may already be stopped/absent; add_server below is the authoritative step.
    let _ = mcp.remove_server(&params.id).await;
    if let Err(e) = mcp.add_server(new_cfg).await {
        return JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string());
    }

    publish_mcp_change(&event_bus, "updated", &existing.name);
    info!(id = %params.id, "MCP server updated");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

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
    config: &crate::sync_primitives::Arc<tokio::sync::RwLock<crate::config::Config>>,
    mcp: &McpManagerHandle,
    vault: &crate::sync_primitives::Arc<crate::gateway::security::SharedTokenManager>,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    /// The Panel keeps its own `McpServerInfo` DTO (it cannot depend on
    /// `alephcore`), so the *value* type is shared via `aleph_protocol` — a
    /// rename inside `UsageSummary` is a compile error on both sides. What is
    /// still hand-kept is the container key, so pin the wire name here and in
    /// the Panel's `api::mcp` test: if one side renames it the other decodes
    /// `None` forever and the column silently goes blank, which is
    /// indistinguishable from "no server has ever been called".
    #[test]
    fn a_row_carries_usage_under_its_wire_name() {
        use aleph_protocol::extension_usage::UsageSummary;
        let row = McpServerInfo {
            id: "s".into(),
            name: "s".into(),
            command: "c".into(),
            args: vec![],
            env: HashMap::new(),
            enabled: true,
            requires_runtime: None,
            usage: Some(UsageSummary {
                calls: Some(3),
                errors: 1,
                ..Default::default()
            }),
        };
        let v = serde_json::to_value(&row).unwrap();
        let usage = v.get("usage").expect("wire key `usage` must be present");
        assert_eq!(
            usage.get("calls").and_then(serde_json::Value::as_u64),
            Some(3)
        );
    }

    /// A server with no report to attach must emit no `usage` key at all —
    /// "unknown", not `calls: 0`. The two mean opposite things to a reader
    /// deciding what to uninstall.
    #[test]
    fn an_absent_report_omits_the_key_rather_than_sending_zero() {
        let cfg = McpManagerConfig::stdio("s", "s", "c");
        let row = info_from_config(&cfg);
        let v = serde_json::to_value(&row).unwrap();
        assert!(
            v.get("usage").is_none(),
            "absent usage must be omitted, never serialized as a zero count"
        );
    }

    #[test]
    fn secret_keys_detected_case_insensitively() {
        assert!(is_secret_env_key("VOLCENGINE_ACCESS_KEY"));
        assert!(is_secret_env_key("VOLCENGINE_SECRET_KEY"));
        assert!(is_secret_env_key("api_token"));
        assert!(is_secret_env_key("DB_PASSWORD"));
        assert!(!is_secret_env_key("SERVICE_ID"));
        assert!(!is_secret_env_key("DOMAIN"));
    }

    #[test]
    fn redact_blanks_secrets_keeps_keys_and_nonsecrets() {
        let input = env(&[
            ("VOLCENGINE_ACCESS_KEY", "ak-real"),
            ("SECRET_KEY", "sk-real"),
            ("SERVICE_ID", "svc-123"),
            ("DOMAIN", "img.example.com"),
        ]);
        let out = redact_secret_env(&input);
        assert_eq!(out.get("VOLCENGINE_ACCESS_KEY"), Some(&String::new()));
        assert_eq!(out.get("SECRET_KEY"), Some(&String::new()));
        assert_eq!(out.get("SERVICE_ID"), Some(&"svc-123".to_string()));
        assert_eq!(out.get("DOMAIN"), Some(&"img.example.com".to_string()));
        // Keys are preserved so the Panel still shows the var is configured.
        assert_eq!(out.len(), 4);
    }

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
        assert_eq!(
            writes,
            vec![(
                "ext.mcp.srv.GITHUB_TOKEN".to_string(),
                "ghp_real".to_string()
            )]
        );
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
        assert!(
            writes.is_empty(),
            "blank secret must not write to the vault"
        );
    }

    #[test]
    fn plan_secret_env_blank_secret_without_existing_is_dropped() {
        let incoming = env(&[("NEW_TOKEN", "")]);
        let (stored, writes) = super::plan_secret_env("srv", incoming, &HashMap::new());
        assert!(
            !stored.contains_key("NEW_TOKEN"),
            "blank secret with no existing entry must be dropped"
        );
        assert!(writes.is_empty());
    }

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
        assert_eq!(
            writes,
            vec![("ext.mcp.My_Srv.API_TOKEN".to_string(), "t-real".to_string())]
        );
    }
}
