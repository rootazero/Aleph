//! `extensions.disclosure` / `extensions.configure` / `extensions.install`.
//!
//! Trust rails are enforced here, never by the calling agent: the pre-install
//! disclosure is built from the resolved spec; an install whose disclosure sets
//! `ack_required` does not proceed without `acknowledge_risk: true`. Secrets are
//! stored in the encrypted vault and referenced in the MCP config as
//! `{{secret:NAME}}` (resolved per-server at spawn) — never written in plaintext.

use crate::extension::marketplace::MarketplaceManager;
use crate::gateway::handlers::parse_params;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::security::SharedTokenManager;
use crate::mcp::manager::McpManagerHandle;
use crate::store::cache::{CatalogCache, CatalogFilter};
use crate::store::install::{run_install, InstallContext, InstallOutcome};
use crate::store::provider::ProviderRegistry;
use crate::store::secrets::field_key;
use crate::store::trust::{build_disclosure, scan_for_injection};
use crate::store::types::{ExtensionEntry, InstallSpec};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct DisclosureParams {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct ConfigureParams {
    pub id: String,
    #[serde(default)]
    pub values: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
pub struct InstallParams {
    pub id: String,
    #[serde(default)]
    pub values: Map<String, Value>,
    #[serde(default)]
    pub acknowledge_risk: bool,
}

// --------------------------------------------------------------------------
// Pure config-validation helpers
// --------------------------------------------------------------------------

fn value_to_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Split submitted values into (secret, plain) field lists per the spec's
/// declared `secret` flag. Only fields present in `values` are returned.
pub fn split_fields(
    spec: &InstallSpec,
    values: &Map<String, Value>,
) -> (Vec<(String, String)>, Vec<(String, String)>) {
    let mut secret = Vec::new();
    let mut plain = Vec::new();
    let mut consider = |name: &str, is_secret: bool| {
        if let Some(v) = values.get(name).and_then(value_to_string) {
            if is_secret {
                secret.push((name.to_string(), v));
            } else {
                plain.push((name.to_string(), v));
            }
        }
    };
    match spec {
        InstallSpec::McpStdio { env, .. } => {
            for e in env {
                consider(&e.name, e.secret);
            }
        }
        InstallSpec::McpRemote { headers, .. } => {
            for h in headers {
                consider(&h.name, h.secret);
            }
        }
        _ => {}
    }
    (secret, plain)
}

/// Required env fields (McpStdio) that are absent or blank in `values`.
pub fn missing_required(spec: &InstallSpec, values: &Map<String, Value>) -> Vec<String> {
    let mut missing = Vec::new();
    if let InstallSpec::McpStdio { env, .. } = spec {
        for e in env {
            if e.required {
                let filled = values
                    .get(&e.name)
                    .and_then(value_to_string)
                    .is_some_and(|s| !s.trim().is_empty());
                if !filled {
                    missing.push(e.name.clone());
                }
            }
        }
    }
    missing
}

// --------------------------------------------------------------------------
// Spec resolution
// --------------------------------------------------------------------------

async fn lookup_entry(cache: &CatalogCache, id: &str) -> Option<ExtensionEntry> {
    let filter = CatalogFilter {
        id: Some(id.to_string()),
        ..Default::default()
    };
    cache.query(&filter).await.ok()?.into_iter().next()
}

async fn resolve_spec(
    entry: &ExtensionEntry,
    registry: &ProviderRegistry,
) -> Result<InstallSpec, String> {
    let provider = registry
        .get(&entry.source_id)
        .ok_or_else(|| format!("no provider for source '{}'", entry.source_id))?;
    provider
        .resolve_install_spec(entry)
        .await
        .map_err(|e| e.to_string())
}

fn scan_text(entry: &ExtensionEntry) -> Vec<crate::store::trust::InjectionFinding> {
    scan_for_injection(&format!("{} {}", entry.name, entry.description))
}

// --------------------------------------------------------------------------
// Handlers
// --------------------------------------------------------------------------

/// extensions.disclosure — resolve the spec and return the pre-install
/// disclosure payload + injection findings, without installing.
pub async fn handle_disclosure(
    req: JsonRpcRequest,
    cache: Arc<CatalogCache>,
    registry: Arc<ProviderRegistry>,
) -> JsonRpcResponse {
    let p: DisclosureParams = match parse_params(&req) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some(entry) = lookup_entry(&cache, &p.id).await else {
        return JsonRpcResponse::error(req.id, INVALID_PARAMS, "unknown extension id");
    };
    let spec = match resolve_spec(&entry, &registry).await {
        Ok(s) => s,
        Err(e) => return JsonRpcResponse::error(req.id, INTERNAL_ERROR, e),
    };
    let disclosure = build_disclosure(&entry, &spec);
    JsonRpcResponse::success(
        req.id,
        json!({ "disclosure": disclosure, "injection_findings": scan_text(&entry) }),
    )
}

/// extensions.configure — validate a submitted config against the spec.
pub async fn handle_configure(
    req: JsonRpcRequest,
    cache: Arc<CatalogCache>,
    registry: Arc<ProviderRegistry>,
) -> JsonRpcResponse {
    let p: ConfigureParams = match parse_params(&req) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some(entry) = lookup_entry(&cache, &p.id).await else {
        return JsonRpcResponse::error(req.id, INVALID_PARAMS, "unknown extension id");
    };
    let spec = match resolve_spec(&entry, &registry).await {
        Ok(s) => s,
        Err(e) => return JsonRpcResponse::error(req.id, INTERNAL_ERROR, e),
    };
    let missing = missing_required(&spec, &p.values);
    JsonRpcResponse::success(
        req.id,
        json!({ "ok": missing.is_empty(), "missing": missing }),
    )
}

/// extensions.install — the full trust-gated install pipeline.
#[allow(clippy::too_many_arguments)]
pub async fn handle_install(
    req: JsonRpcRequest,
    mcp: Option<McpManagerHandle>,
    cache: Arc<CatalogCache>,
    registry: Arc<ProviderRegistry>,
    vault: Arc<SharedTokenManager>,
    marketplace: Arc<MarketplaceManager>,
) -> JsonRpcResponse {
    let p: InstallParams = match parse_params(&req) {
        Ok(p) => p,
        Err(e) => return e,
    };
    // (1) look up the catalog entry.
    let Some(entry) = lookup_entry(&cache, &p.id).await else {
        return JsonRpcResponse::error(req.id, INVALID_PARAMS, "unknown extension id");
    };
    // (2) resolve the install spec; OCI is unsupported (no container runtime).
    let spec = match resolve_spec(&entry, &registry).await {
        Ok(s) => s,
        Err(e) => return JsonRpcResponse::error(req.id, INTERNAL_ERROR, e),
    };
    if matches!(spec, InstallSpec::OciImage { .. }) {
        return JsonRpcResponse::error(
            req.id,
            INVALID_PARAMS,
            "OCI/Docker MCP containers are not installable in this version",
        );
    }
    // (3) trust gate: build disclosure; require ack when mandated.
    let disclosure = build_disclosure(&entry, &spec);
    let findings = scan_text(&entry);
    if disclosure.ack_required && !p.acknowledge_risk {
        return JsonRpcResponse::success(
            req.id,
            json!({
                "ok": false,
                "needs_ack": true,
                "disclosure": disclosure,
                "injection_findings": findings,
            }),
        );
    }
    // (4) validate required fields are present.
    let missing = missing_required(&spec, &p.values);
    if !missing.is_empty() {
        return JsonRpcResponse::success(req.id, json!({ "ok": false, "missing": missing }));
    }
    // (5) store secret fields in the vault; map field name -> vault secret name.
    let (secret_fields, _plain) = split_fields(&spec, &p.values);
    let mut secret_refs: HashMap<String, String> = HashMap::new();
    for (name, val) in &secret_fields {
        let key = field_key(entry.kind, &entry.id, name);
        if let Err(e) = vault.store_secret(&key, val) {
            return JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                format!("failed to store secret '{name}': {e}"),
            );
        }
        secret_refs.insert(name.clone(), key);
    }
    // (6) route the install.
    let ctx = InstallContext {
        entry: &entry,
        mcp: mcp.as_ref(),
        marketplace: Some(marketplace.as_ref()),
        secret_refs,
    };
    let outcome = match run_install(&spec, &ctx).await {
        Ok(o) => o,
        Err(e) => return JsonRpcResponse::error(req.id, INTERNAL_ERROR, e),
    };
    // (7) post-install verify + (8) respond with the approved pin record.
    let verify = verify_install(&outcome, mcp.as_ref()).await;
    JsonRpcResponse::success(
        req.id,
        json!({
            "ok": true,
            "outcome": outcome_json(&outcome),
            "verify": verify,
            "pin": { "version": entry.version, "sha256": disclosure.sha256 },
            "injection_findings": findings,
        }),
    )
}

fn outcome_json(o: &InstallOutcome) -> Value {
    match o {
        InstallOutcome::Mcp { id } => json!({ "kind": "mcp", "id": id }),
        InstallOutcome::Plugin { path } => json!({ "kind": "plugin", "path": path }),
    }
}

/// Confirm the install took effect. Tolerant of `add_server` having already
/// auto-started the MCP server: success is "server is listed", regardless of
/// whether the explicit `start_server` returned an already-running error.
async fn verify_install(outcome: &InstallOutcome, mcp: Option<&McpManagerHandle>) -> Value {
    match outcome {
        InstallOutcome::Mcp { id } => {
            let Some(mcp) = mcp else {
                return json!({ "ok": false, "error": "mcp manager unavailable" });
            };
            let start_err = mcp.start_server(id).await.err().map(|e| e.to_string());
            match mcp.list_servers().await {
                Ok(servers) => match servers.iter().find(|s| &s.id == id) {
                    Some(info) => json!({ "ok": true, "tool_count": info.tool_count }),
                    None => json!({
                        "ok": false,
                        "error": start_err.unwrap_or_else(|| "server not listed after start".into()),
                    }),
                },
                Err(e) => json!({ "ok": false, "error": e.to_string() }),
            }
        }
        InstallOutcome::Plugin { .. } => match crate::extension::try_extension_manager() {
            Some(mgr) => {
                let _ = mgr.reload().await;
                json!({ "ok": true })
            }
            None => json!({ "ok": false, "error": "extension manager unavailable" }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::types::{EnvDecl, HeaderDecl, McpTransport};

    fn stdio_spec() -> InstallSpec {
        InstallSpec::McpStdio {
            command: "npx".into(),
            args: vec![],
            env: vec![
                EnvDecl {
                    name: "TOKEN".into(),
                    required: true,
                    secret: true,
                    ..Default::default()
                },
                EnvDecl {
                    name: "REGION".into(),
                    required: false,
                    secret: false,
                    ..Default::default()
                },
            ],
        }
    }

    #[test]
    fn split_separates_secret_from_plain() {
        let mut values = Map::new();
        values.insert("TOKEN".into(), Value::String("sek".into()));
        values.insert("REGION".into(), Value::String("us".into()));
        let (secret, plain) = split_fields(&stdio_spec(), &values);
        assert_eq!(secret, vec![("TOKEN".to_string(), "sek".to_string())]);
        assert_eq!(plain, vec![("REGION".to_string(), "us".to_string())]);
    }

    #[test]
    fn missing_required_flags_blank_and_absent() {
        let mut values = Map::new();
        values.insert("TOKEN".into(), Value::String("   ".into())); // blank
        let missing = missing_required(&stdio_spec(), &values);
        assert_eq!(missing, vec!["TOKEN".to_string()]);

        let mut ok = Map::new();
        ok.insert("TOKEN".into(), Value::String("real".into()));
        assert!(missing_required(&stdio_spec(), &ok).is_empty());
    }

    #[test]
    fn remote_headers_split_by_secret_flag() {
        let spec = InstallSpec::McpRemote {
            url: "https://x".into(),
            transport: McpTransport::StreamableHttp,
            headers: vec![
                HeaderDecl { name: "Authorization".into(), secret: true },
                HeaderDecl { name: "X-Region".into(), secret: false },
            ],
        };
        let mut values = Map::new();
        values.insert("Authorization".into(), Value::String("Bearer z".into()));
        values.insert("X-Region".into(), Value::String("us".into()));
        let (secret, plain) = split_fields(&spec, &values);
        assert_eq!(secret.len(), 1);
        assert_eq!(secret[0].0, "Authorization");
        assert_eq!(plain.len(), 1);
        assert_eq!(plain[0].0, "X-Region");
    }
}
