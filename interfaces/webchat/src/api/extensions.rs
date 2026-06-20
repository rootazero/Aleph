//! Typed client for the `extensions.*` JSON-RPC façade (P0–P2 backend).
//! Mirrors the exact wire shapes in `src/store/types.rs` / `src/store/trust.rs`.
use serde::Deserialize;
use serde_json::{json, Value};

use crate::context::DashboardState;

/// One catalog/installed entry. Wire shape: snake_case, optionals omitted-when-None.
/// `kind`/`category`/`trust_tier` are kept as `String` (forward-compatible with the open
/// category set — a new backend category must not break the panel deserializer).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ExtensionEntry {
    pub id: String,
    pub kind: String,
    pub category: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub source_id: String,
    pub trust_tier: String,
    #[serde(default)]
    pub repo_url: Option<String>,
    #[serde(default)]
    pub requires_config: bool,
    #[serde(default)]
    pub config_schema: Option<Value>,
    #[serde(default)]
    pub installed: bool,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub update_available: bool,
    /// Provenance: human label of the source/hub that surfaced this entry
    /// (e.g. "Aleph Hub"). Emitted by `extensions.catalog`; empty for installed.
    #[serde(default)]
    pub source_label: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SecretDisclosure {
    pub name: String,
    #[serde(default)]
    pub purpose: String,
    #[serde(default)]
    pub sensitive: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DisclosurePayload {
    pub tier: String,
    pub risk: String,
    pub one_line: String,
    #[serde(default)]
    pub command_display: Option<String>,
    #[serde(default)]
    pub secrets: Vec<SecretDisclosure>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub ack_required: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct InjectionFinding {
    pub kind: String,
    pub detail: String,
}

/// The three branch shapes of `extensions.install` (all are JSON-RPC successes).
#[derive(Debug, Clone, PartialEq)]
pub enum InstallResult {
    NeedsAck {
        disclosure: DisclosurePayload,
        injection_findings: Vec<InjectionFinding>,
    },
    Missing {
        missing: Vec<String>,
    },
    Done {
        outcome: Value,
        verify: Value,
        pin: Value,
        injection_findings: Vec<InjectionFinding>,
    },
}

/// Pure: classify an `extensions.install` result `Value` into its branch.
/// Order matters — `needs_ack` also carries `ok:false`, so test it first.
pub fn parse_install_result(v: &Value) -> Result<InstallResult, String> {
    if v.get("needs_ack").and_then(Value::as_bool) == Some(true) {
        let disclosure =
            serde_json::from_value(v.get("disclosure").cloned().unwrap_or(Value::Null))
                .map_err(|e| format!("bad disclosure: {e}"))?;
        let injection_findings =
            serde_json::from_value(v.get("injection_findings").cloned().unwrap_or(json!([])))
                .unwrap_or_default();
        return Ok(InstallResult::NeedsAck {
            disclosure,
            injection_findings,
        });
    }
    match v.get("ok").and_then(Value::as_bool) {
        Some(false) => {
            let missing = serde_json::from_value(v.get("missing").cloned().unwrap_or(json!([])))
                .unwrap_or_default();
            Ok(InstallResult::Missing { missing })
        }
        Some(true) => Ok(InstallResult::Done {
            outcome: v.get("outcome").cloned().unwrap_or(Value::Null),
            verify: v.get("verify").cloned().unwrap_or(Value::Null),
            pin: v.get("pin").cloned().unwrap_or(Value::Null),
            injection_findings: serde_json::from_value(
                v.get("injection_findings").cloned().unwrap_or(json!([])),
            )
            .unwrap_or_default(),
        }),
        None => Err("unrecognized install response".into()),
    }
}

pub struct ExtensionsApi;

impl ExtensionsApi {
    pub async fn catalog(
        state: &DashboardState,
        params: Value,
    ) -> Result<Vec<ExtensionEntry>, String> {
        let r = state.rpc_call("extensions.catalog", params).await?;
        let arr = r.get("extensions").cloned().unwrap_or(json!([]));
        serde_json::from_value(arr).map_err(|e| format!("parse catalog: {e}"))
    }

    pub async fn installed(state: &DashboardState) -> Result<Vec<ExtensionEntry>, String> {
        let r = state.rpc_call("extensions.installed", Value::Null).await?;
        let arr = r.get("extensions").cloned().unwrap_or(json!([]));
        serde_json::from_value(arr).map_err(|e| format!("parse installed: {e}"))
    }

    pub async fn disclosure(
        state: &DashboardState,
        id: String,
    ) -> Result<(DisclosurePayload, Vec<InjectionFinding>), String> {
        let r = state
            .rpc_call("extensions.disclosure", json!({ "id": id }))
            .await?;
        let disclosure =
            serde_json::from_value(r.get("disclosure").cloned().unwrap_or(Value::Null))
                .map_err(|e| format!("parse disclosure: {e}"))?;
        let findings =
            serde_json::from_value(r.get("injection_findings").cloned().unwrap_or(json!([])))
                .unwrap_or_default();
        Ok((disclosure, findings))
    }

    pub async fn install(
        state: &DashboardState,
        id: String,
        values: Value,
        acknowledge_risk: bool,
    ) -> Result<InstallResult, String> {
        let r = state
            .rpc_call(
                "extensions.install",
                json!({ "id": id, "values": values, "acknowledge_risk": acknowledge_risk }),
            )
            .await?;
        parse_install_result(&r)
    }

    pub async fn toggle(state: &DashboardState, id: String, enabled: bool) -> Result<(), String> {
        state
            .rpc_call("extensions.toggle", json!({ "id": id, "enabled": enabled }))
            .await
            .map(|_| ())
    }

    pub async fn uninstall(state: &DashboardState, id: String) -> Result<(), String> {
        state
            .rpc_call("extensions.uninstall", json!({ "id": id }))
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn entry_deserializes_minimal_wire_shape() {
        // optionals (author/icon/version/repo_url/config_schema) omitted, matching the backend
        let v = json!({
            "id": "mcp-official:io.github.acme/foo",
            "kind": "mcp",
            "category": "developer",
            "name": "Foo",
            "description": "Does foo.",
            "tags": ["mcp", "developer"],
            "source_id": "mcp-official",
            "trust_tier": "community",
            "requires_config": true,
            "installed": false,
            "enabled": false,
            "update_available": false
        });
        let e: ExtensionEntry = serde_json::from_value(v).unwrap();
        assert_eq!(e.id, "mcp-official:io.github.acme/foo");
        assert_eq!(e.kind, "mcp");
        assert_eq!(e.category, "developer");
        assert_eq!(e.author, None);
        assert!(e.requires_config);
        assert_eq!(e.tags, vec!["mcp".to_string(), "developer".to_string()]);
    }

    #[test]
    fn parse_install_needs_ack_branch() {
        let v = json!({
            "ok": false, "needs_ack": true,
            "disclosure": { "tier": "community", "risk": "runs_commands", "one_line": "Runs commands on your computer.",
                "command_display": "npx -y @x/y", "secrets": [{"name":"TOKEN","purpose":"auth","sensitive":true}], "ack_required": true },
            "injection_findings": []
        });
        match parse_install_result(&v).unwrap() {
            InstallResult::NeedsAck { disclosure, .. } => {
                assert_eq!(disclosure.risk, "runs_commands");
                assert!(disclosure.ack_required);
                assert_eq!(disclosure.secrets.len(), 1);
                assert!(disclosure.secrets[0].sensitive);
            }
            other => panic!("expected NeedsAck, got {other:?}"),
        }
    }

    #[test]
    fn parse_install_missing_branch() {
        let v = json!({ "ok": false, "missing": ["GITHUB_TOKEN", "ACCOUNT"] });
        match parse_install_result(&v).unwrap() {
            InstallResult::Missing { missing } => assert_eq!(
                missing,
                vec!["GITHUB_TOKEN".to_string(), "ACCOUNT".to_string()]
            ),
            other => panic!("expected Missing, got {other:?}"),
        }
    }

    #[test]
    fn parse_install_done_branch() {
        let v = json!({ "ok": true, "outcome": {"kind":"mcp","id":"foo"},
            "verify": {"ok": true, "tool_count": 7}, "pin": {"version":"1.0.0","sha256":null}, "injection_findings": [] });
        match parse_install_result(&v).unwrap() {
            InstallResult::Done { verify, .. } => {
                assert_eq!(verify.get("tool_count").and_then(|x| x.as_u64()), Some(7))
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn parse_install_unknown_is_error() {
        assert!(parse_install_result(&json!({"weird": 1})).is_err());
    }
}
