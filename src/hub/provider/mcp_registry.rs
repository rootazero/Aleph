//! Official MCP registry `SourceProvider` (https://registry.modelcontextprotocol.io).
//! Parses the `/v0/servers` response into `ExtensionEntry`s and builds a
//! deterministic `InstallSpec` + synthesized config schema for each server.

use crate::hub::provider::{SourceError, SourceProvider, SyncCtx};
use crate::hub::types::{
    EnvDecl, ExtensionCategory, ExtensionEntry, ExtensionKind, InstallSpec, McpTransport, TrustTier,
};
use serde::Deserialize;
use serde_json::{json, Value};

pub const DEFAULT_BASE_URL: &str = "https://registry.modelcontextprotocol.io";

#[derive(Debug, Deserialize)]
pub struct RegistryResponse {
    #[serde(default)]
    pub servers: Vec<RegistryServer>,
    #[serde(default)]
    pub metadata: RegistryMeta,
}

#[derive(Debug, Default, Deserialize)]
pub struct RegistryMeta {
    #[serde(default, rename = "nextCursor")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RegistryServer {
    pub server: ServerDetail,
}

#[derive(Debug, Deserialize)]
pub struct ServerDetail {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub repository: Option<Repository>,
    #[serde(default)]
    pub packages: Vec<Package>,
    #[serde(default)]
    pub remotes: Vec<Remote>,
}

#[derive(Debug, Deserialize)]
pub struct Repository {
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct Package {
    #[serde(rename = "runtimeHint")]
    pub runtime_hint: Option<String>,
    pub identifier: String,
    #[serde(default, rename = "runtimeArguments")]
    pub runtime_arguments: Vec<Argument>,
    #[serde(default, rename = "packageArguments")]
    pub package_arguments: Vec<Argument>,
    #[serde(default, rename = "environmentVariables")]
    pub environment_variables: Vec<RegistryEnvVar>,
    #[serde(default)]
    pub transport: Option<Transport>,
}

#[derive(Debug, Deserialize)]
pub struct Argument {
    #[serde(default)]
    pub value: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Transport {
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegistryEnvVar {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, rename = "isRequired")]
    pub is_required: bool,
    #[serde(default, rename = "isSecret")]
    pub is_secret: bool,
    #[serde(default)]
    pub default: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Remote {
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
}

fn name_tail(reverse_dns: &str) -> &str {
    reverse_dns.rsplit('/').next().unwrap_or(reverse_dns)
}

pub fn synthesize_config_schema(envs: &[RegistryEnvVar]) -> Option<Value> {
    if envs.is_empty() {
        return None;
    }
    let mut props = serde_json::Map::new();
    let mut required = Vec::new();
    for e in envs {
        let mut field = json!({ "type": "string" });
        if let Some(d) = &e.description {
            field["description"] = json!(d);
        }
        if e.is_secret {
            field["x-sensitive"] = json!(true);
        }
        if let Some(def) = &e.default {
            field["default"] = json!(def);
        }
        props.insert(e.name.clone(), field);
        if e.is_required {
            required.push(e.name.clone());
        }
    }
    Some(json!({ "type": "object", "properties": props, "required": required }))
}

pub fn server_to_extension(s: &RegistryServer) -> ExtensionEntry {
    let d = &s.server;
    let any_required = d
        .packages
        .iter()
        .flat_map(|p| p.environment_variables.iter())
        .any(|e| e.is_required);
    let owned_envs: Vec<RegistryEnvVar> = d
        .packages
        .iter()
        .flat_map(|p| p.environment_variables.clone())
        .collect();
    ExtensionEntry {
        id: format!("mcp-official:{}", d.name),
        kind: ExtensionKind::Mcp,
        category: ExtensionCategory::Other,
        name: name_tail(&d.name).to_string(),
        description: d.description.clone(),
        author: d.name.split('/').next().map(|s| s.to_string()),
        icon: None,
        tags: vec!["mcp".into()],
        version: d.version.clone(),
        source_id: "mcp-official".into(),
        repo_url: d.repository.as_ref().map(|r| r.url.clone()),
        trust_tier: TrustTier::Community, // registry verifies namespace only
        requires_config: any_required,
        config_schema: synthesize_config_schema(&owned_envs),
        installed: false,
        enabled: false,
        update_available: false,
        via: None,
        install_spec: None,
    }
}

pub fn server_to_install_spec(s: &RegistryServer) -> Option<InstallSpec> {
    let d = &s.server;
    if let Some(pkg) = d.packages.first() {
        let command = pkg.runtime_hint.clone().unwrap_or_else(|| "npx".into());
        let mut args: Vec<String> = pkg
            .runtime_arguments
            .iter()
            .filter_map(|a| a.value.clone())
            .collect();
        args.push(pkg.identifier.clone());
        args.extend(pkg.package_arguments.iter().filter_map(|a| a.value.clone()));
        let env = pkg
            .environment_variables
            .iter()
            .map(|e| EnvDecl {
                name: e.name.clone(),
                description: e.description.clone(),
                required: e.is_required,
                secret: e.is_secret,
                default: e.default.clone(),
                placeholder: None,
            })
            .collect();
        return Some(InstallSpec::McpStdio { command, args, env });
    }
    if let Some(rem) = d.remotes.first() {
        let transport = match rem.kind.as_str() {
            "sse" => McpTransport::Sse,
            _ => McpTransport::StreamableHttp,
        };
        return Some(InstallSpec::McpRemote {
            url: rem.url.clone(),
            transport,
            headers: vec![],
        });
    }
    None
}

pub struct McpRegistryProvider {
    pub base_url: String,
    pub http: reqwest::Client,
}

impl McpRegistryProvider {
    pub fn new() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.into(),
            http: reqwest::Client::new(),
        }
    }
}

impl Default for McpRegistryProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl SourceProvider for McpRegistryProvider {
    fn id(&self) -> &str {
        "mcp-official"
    }
    fn display_name(&self) -> &str {
        "MCP Registry"
    }
    fn kinds(&self) -> &[ExtensionKind] {
        &[ExtensionKind::Mcp]
    }
    fn trust_tier(&self) -> TrustTier {
        TrustTier::Community
    }

    async fn sync(&self, _ctx: &SyncCtx) -> Result<Vec<ExtensionEntry>, SourceError> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut url = format!("{}/v0/servers?limit=100", self.base_url);
            if let Some(c) = &cursor {
                url.push_str(&format!("&cursor={c}"));
            }
            let resp = self
                .http
                .get(&url)
                .timeout(std::time::Duration::from_secs(30))
                .send()
                .await
                .map_err(|e| SourceError::Network(e.to_string()))?;
            if !resp.status().is_success() {
                return Err(SourceError::Network(format!("HTTP {}", resp.status())));
            }
            let body: RegistryResponse = resp
                .json()
                .await
                .map_err(|e| SourceError::Parse(e.to_string()))?;
            out.extend(body.servers.iter().map(server_to_extension));
            match body.metadata.next_cursor {
                Some(c) if !c.is_empty() => cursor = Some(c),
                _ => break,
            }
            if out.len() > 10_000 {
                break; // safety bound
            }
        }
        Ok(out)
    }

    async fn resolve_install_spec(
        &self,
        entry: &ExtensionEntry,
    ) -> Result<InstallSpec, SourceError> {
        let native = entry.id.strip_prefix("mcp-official:").unwrap_or(&entry.id);
        // urlencoding is not a dep; the reverse-DNS name only needs '/' escaped.
        let encoded = native.replace('/', "%2F");
        let url = format!("{}/v0/servers/{}/versions/latest", self.base_url, encoded);
        let resp = self
            .http
            .get(&url)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| SourceError::Network(e.to_string()))?;
        let server: RegistryServer = resp
            .json()
            .await
            .map_err(|e| SourceError::Parse(e.to_string()))?;
        server_to_install_spec(&server)
            .ok_or_else(|| SourceError::Other("no installable package/remote".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "servers": [{
        "server": {
          "name": "io.github.acme/github",
          "description": "GitHub access for agents.",
          "version": "1.4.0",
          "repository": { "url": "https://github.com/acme/github-mcp", "source": "github" },
          "packages": [{
            "registryType": "npm",
            "identifier": "@modelcontextprotocol/server-github",
            "version": "1.4.0",
            "runtimeHint": "npx",
            "transport": { "type": "stdio" },
            "runtimeArguments": [{ "type": "named", "value": "-y" }],
            "packageArguments": [],
            "environmentVariables": [
              { "name": "GITHUB_TOKEN", "description": "PAT", "isRequired": true, "isSecret": true }
            ]
          }]
        }
      }],
      "metadata": { "count": 1 }
    }"#;

    #[test]
    fn parses_and_maps_server() {
        let resp: RegistryResponse = serde_json::from_str(FIXTURE).unwrap();
        let server = &resp.servers[0];
        let e = server_to_extension(server);
        assert_eq!(e.kind, ExtensionKind::Mcp);
        assert_eq!(e.id, "mcp-official:io.github.acme/github");
        assert_eq!(e.name, "github"); // tail of reverse-DNS name
        assert!(e.requires_config); // has a required env var
        assert!(e.config_schema.is_some());
        assert_eq!(
            e.repo_url.as_deref(),
            Some("https://github.com/acme/github-mcp")
        );
    }

    #[test]
    fn builds_stdio_install_spec() {
        let resp: RegistryResponse = serde_json::from_str(FIXTURE).unwrap();
        let spec = server_to_install_spec(&resp.servers[0]).unwrap();
        match spec {
            InstallSpec::McpStdio { command, args, env } => {
                assert_eq!(command, "npx");
                assert_eq!(args, vec!["-y", "@modelcontextprotocol/server-github"]);
                assert_eq!(env.len(), 1);
                assert!(env[0].required && env[0].secret);
            }
            _ => panic!("expected McpStdio"),
        }
    }
}
