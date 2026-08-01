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
use crate::hub::cache::{CatalogCache, CatalogFilter};
use crate::hub::install::{run_install, InstallContext, InstallOutcome};
use crate::hub::secrets::field_key;
use crate::hub::trust::{build_disclosure, scan_for_injection};
use crate::hub::types::{ExtensionEntry, InstallSpec};
use crate::mcp::manager::McpManagerHandle;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct DisclosureParams {
    pub id: String,
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

/// Config fields the spec requires that are absent or blank in `values`.
///
/// Covers both spec shapes that declare config, matching
/// `InstallSpec::requires_config`: stdio `env` entries flagged `required`, and
/// remote `headers` flagged `secret` (a secret header is auth material — the
/// endpoint is unusable without it, which is exactly why `requires_config`
/// counts it).
pub fn missing_required(spec: &InstallSpec, values: &Map<String, Value>) -> Vec<String> {
    let filled = |name: &str| {
        values
            .get(name)
            .and_then(value_to_string)
            .is_some_and(|s| !s.trim().is_empty())
    };
    match spec {
        InstallSpec::McpStdio { env, .. } => env
            .iter()
            .filter(|e| e.required && !filled(&e.name))
            .map(|e| e.name.clone())
            .collect(),
        InstallSpec::McpRemote { headers, .. } => headers
            .iter()
            .filter(|h| h.secret && !filled(&h.name))
            .map(|h| h.name.clone())
            .collect(),
        InstallSpec::OciImage { .. } | InstallSpec::GitDir { .. } => Vec::new(),
    }
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

fn resolve_spec(entry: &ExtensionEntry) -> Result<InstallSpec, String> {
    entry
        .install_spec
        .clone()
        .ok_or_else(|| format!("no install_spec for entry '{}'", entry.id))
}

fn scan_text(entry: &ExtensionEntry) -> Vec<crate::hub::trust::InjectionFinding> {
    scan_for_injection(&format!("{} {}", entry.name, entry.description))
}

// --------------------------------------------------------------------------
// Handlers
// --------------------------------------------------------------------------

/// extensions.disclosure — resolve the spec and return the pre-install
/// disclosure payload + injection findings, without installing.
pub async fn handle_disclosure(req: JsonRpcRequest, cache: Arc<CatalogCache>) -> JsonRpcResponse {
    let p: DisclosureParams = match parse_params(&req) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some(entry) = lookup_entry(&cache, &p.id).await else {
        return JsonRpcResponse::error(req.id, INVALID_PARAMS, "unknown extension id");
    };
    let spec = match resolve_spec(&entry) {
        Ok(s) => s,
        Err(e) => return JsonRpcResponse::error(req.id, INTERNAL_ERROR, e),
    };
    let disclosure = build_disclosure(&entry, &spec);
    let post_install = crate::hub::official_mcp::post_install_for(&entry.id);
    JsonRpcResponse::success(
        req.id,
        json!({
            "disclosure": disclosure,
            "injection_findings": scan_text(&entry),
            "post_install": post_install,
        }),
    )
}

/// extensions.install — the full trust-gated install pipeline.
pub async fn handle_install(
    req: JsonRpcRequest,
    mcp: Option<McpManagerHandle>,
    cache: Arc<CatalogCache>,
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
    let spec = match resolve_spec(&entry) {
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
    let (secret_fields, plain_fields) = split_fields(&spec, &p.values);
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
    let plain_values: HashMap<String, String> = plain_fields.into_iter().collect();
    // (6) route the install.
    let ctx = InstallContext {
        entry: &entry,
        mcp: mcp.as_ref(),
        marketplace: Some(marketplace.as_ref()),
        secret_refs,
        plain_values,
    };
    let outcome = match run_install(&spec, &ctx).await {
        Ok(o) => o,
        Err(e) => return JsonRpcResponse::error(req.id, INTERNAL_ERROR, e),
    };
    // (7) record provenance: which catalog entry, at what version and spec, this
    // install came from.
    crate::hub::origin::record_install(&cache, &entry, &spec, &outcome).await;
    // (8) post-install verify + (9) respond with the approved pin record.
    let verify = verify_after_install(&outcome, mcp.as_ref()).await;
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
        InstallOutcome::Skill { path } => json!({ "kind": "skill", "path": path }),
    }
}

/// Confirm the install took effect, then report the verdict from the single
/// verifier (`hub::verify::verify_install`, shared with `hub_install_verify`).
///
/// Only the *nudges* live here — they are side effects the pure verifier must
/// not perform: start the MCP server (tolerating "already running", since
/// `add_server` auto-starts) and reload the extension manager so a freshly
/// copied plugin/skill is on disk *and* loaded before we look.
async fn verify_after_install(outcome: &InstallOutcome, mcp: Option<&McpManagerHandle>) -> Value {
    match outcome {
        InstallOutcome::Mcp { id } => {
            if let Some(mcp) = mcp {
                let _ = mcp.start_server(id).await;
            }
        }
        InstallOutcome::Plugin { .. } | InstallOutcome::Skill { .. } => {
            if let Some(mgr) = crate::extension::try_extension_manager() {
                let _ = mgr.reload().await;
            }
        }
    }
    let report = crate::hub::verify::verify_install(outcome, mcp).await;
    serde_json::to_value(&report).unwrap_or_else(|e| json!({ "ok": false, "error": e.to_string() }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::types::{EnvDecl, HeaderDecl, McpTransport};

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

    /// Regression: `missing_required` only looked at stdio `env`, so a remote
    /// entry whose auth header was left blank installed "successfully" and then
    /// 401'd. A secret header is auth material — `requires_config` already counts
    /// it, so the gate must too.
    #[test]
    fn missing_required_covers_remote_secret_headers() {
        let spec = InstallSpec::McpRemote {
            url: "https://x".into(),
            transport: McpTransport::StreamableHttp,
            headers: vec![
                HeaderDecl {
                    name: "Authorization".into(),
                    secret: true,
                },
                HeaderDecl {
                    name: "X-Region".into(),
                    secret: false,
                },
            ],
        };
        // Nothing supplied → the secret header is missing; the plain one is not.
        assert_eq!(
            missing_required(&spec, &Map::new()),
            vec!["Authorization".to_string()]
        );
        // Blank counts as missing.
        let mut blank = Map::new();
        blank.insert("Authorization".into(), Value::String("  ".into()));
        assert_eq!(
            missing_required(&spec, &blank),
            vec!["Authorization".to_string()]
        );
        // Supplied → satisfied.
        let mut filled = Map::new();
        filled.insert("Authorization".into(), Value::String("Bearer z".into()));
        assert!(missing_required(&spec, &filled).is_empty());
    }

    #[test]
    fn remote_headers_split_by_secret_flag() {
        let spec = InstallSpec::McpRemote {
            url: "https://x".into(),
            transport: McpTransport::StreamableHttp,
            headers: vec![
                HeaderDecl {
                    name: "Authorization".into(),
                    secret: true,
                },
                HeaderDecl {
                    name: "X-Region".into(),
                    secret: false,
                },
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
