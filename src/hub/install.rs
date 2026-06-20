//! Install routing over `InstallSpec`.
//!
//! `mcp_config_from_spec` (pure) builds the MCP server config, writing
//! `{{secret:NAME}}` references for secret-bearing env fields — never
//! plaintext; the reference resolves per-server at spawn. `run_install` routes
//! a resolved spec to the correct backend (MCP add / marketplace plugin copy /
//! OCI-unsupported).

use std::collections::HashMap;

use crate::extension::marketplace::MarketplaceManager;
use crate::extension::PluginScope;
use crate::mcp::manager::{McpManagerConfig, McpManagerHandle};
use crate::hub::secrets::secret_ref;
use crate::hub::types::{ExtensionEntry, InstallSpec};

/// Build an `McpManagerConfig` from an install spec.
///
/// `secret_refs` maps an env var name to its stored vault secret name (from
/// `crate::hub::secrets::field_key`); `plain_values` maps a non-secret env var
/// name to the user-submitted value. Per field, precedence is: secret reference
/// (`{{secret:NAME}}`) → submitted plain value → declared `default`. Plaintext
/// secrets never enter the config.
pub fn mcp_config_from_spec(
    id: &str,
    name: &str,
    spec: &InstallSpec,
    secret_refs: &HashMap<String, String>,
    plain_values: &HashMap<String, String>,
) -> Result<McpManagerConfig, String> {
    match spec {
        InstallSpec::McpStdio { command, args, env } => {
            let mut env_map = HashMap::new();
            for e in env {
                if let Some(secret_name) = secret_refs.get(&e.name) {
                    env_map.insert(e.name.clone(), secret_ref(secret_name));
                } else if let Some(v) = plain_values.get(&e.name) {
                    env_map.insert(e.name.clone(), v.clone());
                } else if let Some(def) = &e.default {
                    env_map.insert(e.name.clone(), def.clone());
                }
            }
            Ok(McpManagerConfig::stdio(id, name, command)
                .with_args(args.clone())
                .with_env(env_map)
                .with_auto_start(true))
        }
        InstallSpec::McpRemote { url, .. } => {
            // Header-secret injection for remote MCP is a follow-up; build the
            // base config so the server is reachable.
            Ok(McpManagerConfig::http(id, name, url).with_auto_start(true))
        }
        InstallSpec::OciImage { .. } => {
            Err("OCI/Docker MCP containers are not installable in this version".into())
        }
        InstallSpec::GitDir { .. } => Err("GitDir installs via the plugin path, not MCP".into()),
    }
}

/// Outcome of a successful install.
#[derive(Debug, Clone)]
pub enum InstallOutcome {
    Mcp { id: String },
    Plugin { path: String },
}

/// Inputs the install router needs from the handler layer.
pub struct InstallContext<'a> {
    pub entry: &'a ExtensionEntry,
    pub mcp: Option<&'a McpManagerHandle>,
    pub marketplace: Option<&'a MarketplaceManager>,
    /// env/header field name -> stored vault secret name.
    pub secret_refs: HashMap<String, String>,
    /// non-secret env field name -> user-submitted plain value.
    pub plain_values: HashMap<String, String>,
}

/// Deterministic MCP server id derived from the store entry id.
fn mcp_server_id(entry_id: &str) -> String {
    entry_id.replace([':', '/'], "_")
}

/// Route a resolved install spec to its backend and perform the install.
pub async fn run_install(
    spec: &InstallSpec,
    ctx: &InstallContext<'_>,
) -> Result<InstallOutcome, String> {
    match spec {
        InstallSpec::McpStdio { .. } | InstallSpec::McpRemote { .. } => {
            let mcp = ctx.mcp.ok_or("MCP manager unavailable")?;
            let id = mcp_server_id(&ctx.entry.id);
            let cfg = mcp_config_from_spec(
                &id,
                &ctx.entry.name,
                spec,
                &ctx.secret_refs,
                &ctx.plain_values,
            )?;
            mcp.add_server(cfg).await.map_err(|e| e.to_string())?;
            Ok(InstallOutcome::Mcp { id })
        }
        InstallSpec::OciImage { .. } => {
            Err("OCI/Docker MCP containers are not installable in this version".into())
        }
        InstallSpec::GitDir { .. } => {
            // Plugin install via the marketplace path (SHA-256 + atomic copy).
            let marketplace = ctx.marketplace.ok_or("marketplace unavailable")?;
            let marketplace_name =
                (ctx.entry.source_id != "local").then_some(ctx.entry.source_id.as_str());
            let path = marketplace.install_to_scope(
                &ctx.entry.name,
                marketplace_name,
                PluginScope::User,
                None,
            )?;
            Ok(InstallOutcome::Plugin {
                path: path.display().to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::types::EnvDecl;

    #[test]
    fn stdio_spec_builds_config_with_secret_refs() {
        let spec = InstallSpec::McpStdio {
            command: "npx".into(),
            args: vec!["-y".into(), "@x/y".into()],
            env: vec![
                EnvDecl {
                    name: "TOKEN".into(),
                    required: true,
                    secret: true,
                    ..Default::default()
                },
                EnvDecl {
                    name: "REGION".into(),
                    default: Some("us".into()),
                    ..Default::default()
                },
                // required, non-secret, NO default — must take the submitted value
                EnvDecl {
                    name: "ACCOUNT".into(),
                    required: true,
                    secret: false,
                    ..Default::default()
                },
            ],
        };
        let mut refs = HashMap::new();
        refs.insert("TOKEN".to_string(), "ext.mcp.x.TOKEN".to_string());
        let mut plain = HashMap::new();
        plain.insert("ACCOUNT".to_string(), "acct-123".to_string());
        let cfg = mcp_config_from_spec("x", "Y", &spec, &refs, &plain).unwrap();
        assert_eq!(cfg.command.as_deref(), Some("npx"));
        assert_eq!(cfg.args, vec!["-y".to_string(), "@x/y".to_string()]);
        assert_eq!(
            cfg.env.get("TOKEN").map(String::as_str),
            Some("{{secret:ext.mcp.x.TOKEN}}")
        );
        // non-secret field falls back to its declared default
        assert_eq!(cfg.env.get("REGION").map(String::as_str), Some("us"));
        // required non-secret field with no default takes the submitted value
        assert_eq!(cfg.env.get("ACCOUNT").map(String::as_str), Some("acct-123"));
        assert!(cfg.auto_start);
    }

    #[test]
    fn oci_spec_is_unsupported() {
        let spec = InstallSpec::OciImage {
            image: "mcp/y@sha256:abc".into(),
        };
        let err = mcp_config_from_spec("x", "Y", &spec, &Default::default(), &Default::default())
            .unwrap_err();
        assert!(err.contains("not installable"));
    }
}
