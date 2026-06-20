//! Docker MCP catalog `SourceProvider`. Signed, sha-pinned images →
//! `TrustTier::Official`; install spec is a single `OciImage`.

use crate::store::provider::{SourceError, SourceProvider, SyncCtx};
use crate::store::types::{ExtensionEntry, ExtensionKind, InstallSpec, TrustTier};
use serde::Deserialize;
use std::collections::BTreeMap;

pub const DEFAULT_CATALOG_URL: &str = "https://desktop.docker.com/mcp/catalog/v2/catalog.yaml";

#[derive(Debug, Deserialize)]
pub struct DockerCatalog {
    #[serde(default)]
    pub registry: BTreeMap<String, DockerServer>,
}

#[derive(Debug, Deserialize)]
pub struct DockerServer {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
}

pub fn docker_install_spec(s: &DockerServer) -> InstallSpec {
    InstallSpec::OciImage {
        image: s.image.clone().unwrap_or_default(),
    }
}

pub fn docker_server_to_extension(name: &str, s: &DockerServer) -> ExtensionEntry {
    let description = s.description.clone();
    let tags = vec!["mcp".into(), "container".into()];
    let category = s.category
        .as_deref()
        .and_then(crate::store::categorize::category_from_hint)
        .unwrap_or_else(|| crate::store::categorize::categorize(name, &description, &tags, None));
    ExtensionEntry {
        id: format!("docker-mcp:{name}"),
        kind: ExtensionKind::Mcp,
        category,
        name: name.to_string(),
        description,
        author: Some("docker".into()),
        icon: None,
        tags,
        version: None,
        source_id: "docker-mcp".into(),
        repo_url: None,
        trust_tier: TrustTier::Official, // signed, sha-pinned images
        requires_config: false,
        config_schema: None,
        installed: false,
        enabled: false,
        update_available: false,
    }
}

pub struct DockerMcpProvider {
    pub url: String,
    pub http: reqwest::Client,
}

impl DockerMcpProvider {
    pub fn new() -> Self {
        Self {
            url: DEFAULT_CATALOG_URL.into(),
            http: reqwest::Client::new(),
        }
    }
}

impl Default for DockerMcpProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl SourceProvider for DockerMcpProvider {
    fn id(&self) -> &str {
        "docker-mcp"
    }
    fn kinds(&self) -> &[ExtensionKind] {
        &[ExtensionKind::Mcp]
    }
    fn trust_tier(&self) -> TrustTier {
        TrustTier::Official
    }

    async fn sync(&self, _ctx: &SyncCtx) -> Result<Vec<ExtensionEntry>, SourceError> {
        let text = self
            .http
            .get(&self.url)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| SourceError::Network(e.to_string()))?
            .text()
            .await
            .map_err(|e| SourceError::Network(e.to_string()))?;
        let cat: DockerCatalog =
            serde_yaml::from_str(&text).map_err(|e| SourceError::Parse(e.to_string()))?;
        Ok(cat
            .registry
            .iter()
            .map(|(n, s)| docker_server_to_extension(n, s))
            .collect())
    }

    async fn resolve_install_spec(
        &self,
        entry: &ExtensionEntry,
    ) -> Result<InstallSpec, SourceError> {
        // The catalog row fully determines the install spec; re-fetch and find by id.
        let text = self
            .http
            .get(&self.url)
            .send()
            .await
            .map_err(|e| SourceError::Network(e.to_string()))?
            .text()
            .await
            .map_err(|e| SourceError::Network(e.to_string()))?;
        let cat: DockerCatalog =
            serde_yaml::from_str(&text).map_err(|e| SourceError::Parse(e.to_string()))?;
        let name = entry.id.strip_prefix("docker-mcp:").unwrap_or(&entry.id);
        cat.registry
            .get(name)
            .map(docker_install_spec)
            .ok_or_else(|| SourceError::Other("server not in catalog".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
registry:
  github:
    description: "GitHub access"
    image: "mcp/github@sha256:abc123"
    category: "developer"
  postgres:
    description: "Query Postgres"
    image: "mcp/postgres@sha256:def456"
"#;

    #[test]
    fn parses_and_maps_docker_catalog() {
        let cat: DockerCatalog = serde_yaml::from_str(FIXTURE).unwrap();
        let srv = cat.registry.get("github").expect("github in catalog");
        let e = docker_server_to_extension("github", srv);
        assert_eq!(e.kind, ExtensionKind::Mcp);
        assert_eq!(e.id, "docker-mcp:github");
        assert_eq!(e.trust_tier, TrustTier::Official); // signed images
        match docker_install_spec(srv) {
            InstallSpec::OciImage { image } => assert_eq!(image, "mcp/github@sha256:abc123"),
            _ => panic!("expected OciImage"),
        }
    }
}
