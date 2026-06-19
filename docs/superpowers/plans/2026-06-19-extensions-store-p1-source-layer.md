# Extensions Store — P1 Source Layer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the pluggable `SourceProvider` aggregation layer and three v1 providers (plugin marketplace, official MCP registry, Docker MCP catalog) that sync normalized `ExtensionEntry`s into the P0 catalog cache, so `extensions.catalog` returns real, browsable, offline-capable data.

**Architecture:** A `SourceProvider` trait sits above the existing `MarketplaceManager` and HTTP catalogs. A `ProviderRegistry` fans `sync()` out concurrently and writes each provider's slice into the rusqlite cache via `replace_source`. Providers normalize heterogeneous source metadata into the P0 `ExtensionEntry` + `InstallSpec`.

**Tech Stack:** Rust (tokio, reqwest 0.12, serde, serde_json, serde_yaml, async-trait, rusqlite).

## Global Constraints

See INDEX → "Global Constraints". P1-specific:
- Providers are the ONLY network callers; `sync()` runs in the background, never per-keystroke. `resolve_install_spec()` may fetch a detail endpoint and is cached.
- v1 providers: `cc-marketplace` (plugin), `mcp-official` (mcp), `docker-mcp` (mcp). No GitHub crawler, no ClawHub.
- Validate/parse defensively; one failing provider must not block others or wipe its last-good cache (only `replace_source` on a successful, non-empty fetch).
- Category assignment is the Store Agent's job (P4) — providers set `category: ExtensionCategory::Other` and a provider-default `trust_tier`.
- Test builds narrow: `cargo test -p alephcore store::provider`. Provider parsing is tested against **captured fixtures** (no network in unit tests).

**Reference signatures (verified, file:line):**
- `MarketplaceManager::new(HashMap<String, MarketplaceConfig>, Option<PathBuf>)` `src/extension/marketplace/mod.rs:37`; `update(&self, name: &str) -> Result<PathBuf, String>` `:124`; `list(&self) -> HashMap<String, MarketplaceConfig>` `:103`; `install_to_scope(...)` `:217`.
- `parse_marketplace_manifest(dir: &Path) -> Result<MarketplaceManifest, String>` `src/extension/marketplace/manifest.rs:39`. `MarketplaceManifest { name, owner, metadata, plugins: Vec<MarketplacePluginEntry> }` `types.rs:61`. `MarketplacePluginEntry { name, source, description: Option, version: Option, sha256: Option }` `types.rs:29`.
- `verify_plugin_integrity(&Path, Option<&str>) -> Result<(), String>` `installer.rs:186`.
- reqwest 0.12 (`Cargo.toml:148`); pattern `client.get(url).timeout(d).send().await?.json::<T>().await?` (`src/a2a/adapter/client/http_client.rs`).
- P0 types: `crate::store::types::{ExtensionEntry, ExtensionKind, ExtensionCategory, TrustTier, InstallSpec, EnvDecl, McpTransport}`; cache: `crate::store::cache::CatalogCache::{replace_source, query}`.

---

### Task 1: `SourceProvider` trait + `ProviderRegistry`

**Files:**
- Create: `src/store/provider/mod.rs`
- Modify: `src/store/mod.rs` (`pub mod provider;`)
- Modify: `Cargo.toml` (ensure `async-trait` + `serde_yaml` deps)
- Test: inline `#[cfg(test)]`

**Interfaces:**
- Produces:
```rust
pub struct SyncCtx;                       // reserved (cache dir, http client) — empty in v1
pub struct Query { pub text: String }
#[derive(Debug)] pub enum SourceError { Network(String), Parse(String), Other(String) }

#[async_trait::async_trait]
pub trait SourceProvider: Send + Sync {
    fn id(&self) -> &str;
    fn kinds(&self) -> &[ExtensionKind];
    fn trust_tier(&self) -> TrustTier;
    async fn sync(&self, ctx: &SyncCtx) -> Result<Vec<ExtensionEntry>, SourceError>;
    async fn search(&self, _q: &Query) -> Option<Result<Vec<ExtensionEntry>, SourceError>> { None }
    async fn resolve_install_spec(&self, entry: &ExtensionEntry) -> Result<InstallSpec, SourceError>;
}

pub struct ProviderRegistry { providers: Vec<Box<dyn SourceProvider>> }
// new(), register(Box<dyn SourceProvider>), sync_all_into(&CatalogCache) -> SyncReport
pub struct SyncReport { pub synced: Vec<(String, usize)>, pub failed: Vec<(String, String)> }
```

- [ ] **Step 1: Ensure deps** — in `Cargo.toml` confirm/add:
```toml
async-trait = "0.1"
serde_yaml = "0.9"
```
Run: `cargo tree -p alephcore -i async-trait` to check presence; add if missing.

- [ ] **Step 2: Write the failing test** in `src/store/provider/mod.rs`

```rust
use crate::store::cache::{CatalogCache, CatalogFilter};
use crate::store::types::{ExtensionCategory, ExtensionEntry, ExtensionKind, InstallSpec, TrustTier};

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeProvider { id: String, entries: Vec<ExtensionEntry> }
    #[async_trait::async_trait]
    impl SourceProvider for FakeProvider {
        fn id(&self) -> &str { &self.id }
        fn kinds(&self) -> &[ExtensionKind] { &[ExtensionKind::Mcp] }
        fn trust_tier(&self) -> TrustTier { TrustTier::Community }
        async fn sync(&self, _c: &SyncCtx) -> Result<Vec<ExtensionEntry>, SourceError> { Ok(self.entries.clone()) }
        async fn resolve_install_spec(&self, _e: &ExtensionEntry) -> Result<InstallSpec, SourceError> {
            Ok(InstallSpec::OciImage { image: "x".into() })
        }
    }

    fn entry(id: &str, src: &str) -> ExtensionEntry {
        ExtensionEntry {
            id: id.into(), kind: ExtensionKind::Mcp, category: ExtensionCategory::Other,
            name: id.into(), description: String::new(), author: None, icon: None, tags: vec![],
            version: None, source_id: src.into(), repo_url: None, trust_tier: TrustTier::Community,
            requires_config: false, config_schema: None, installed: false, enabled: false, update_available: false,
        }
    }

    #[tokio::test]
    async fn sync_all_writes_each_provider_slice() {
        let cache = CatalogCache::open_in_memory().unwrap();
        let mut reg = ProviderRegistry::new();
        reg.register(Box::new(FakeProvider { id: "p1".into(), entries: vec![entry("p1:a", "p1")] }));
        reg.register(Box::new(FakeProvider { id: "p2".into(), entries: vec![entry("p2:a", "p2"), entry("p2:b", "p2")] }));

        let report = reg.sync_all_into(&cache).await;
        assert_eq!(report.failed.len(), 0);
        let all = cache.query(&CatalogFilter::default()).await.unwrap();
        assert_eq!(all.len(), 3);
    }
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p alephcore store::provider::tests::sync_all_writes_each_provider_slice`
Expected: FAIL — trait/registry not defined.

- [ ] **Step 4: Implement trait + registry** (above the test module)

```rust
pub struct SyncCtx;
pub struct Query { pub text: String }

#[derive(Debug)]
pub enum SourceError { Network(String), Parse(String), Other(String) }
impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network(s) => write!(f, "network: {s}"),
            Self::Parse(s) => write!(f, "parse: {s}"),
            Self::Other(s) => write!(f, "{s}"),
        }
    }
}

#[async_trait::async_trait]
pub trait SourceProvider: Send + Sync {
    fn id(&self) -> &str;
    fn kinds(&self) -> &[ExtensionKind];
    fn trust_tier(&self) -> TrustTier;
    async fn sync(&self, ctx: &SyncCtx) -> Result<Vec<ExtensionEntry>, SourceError>;
    async fn search(&self, _q: &Query) -> Option<Result<Vec<ExtensionEntry>, SourceError>> { None }
    async fn resolve_install_spec(&self, entry: &ExtensionEntry) -> Result<InstallSpec, SourceError>;
}

pub struct SyncReport { pub synced: Vec<(String, usize)>, pub failed: Vec<(String, String)> }

pub struct ProviderRegistry { providers: Vec<Box<dyn SourceProvider>> }

impl ProviderRegistry {
    pub fn new() -> Self { Self { providers: Vec::new() } }
    pub fn register(&mut self, p: Box<dyn SourceProvider>) { self.providers.push(p); }
    pub fn get(&self, id: &str) -> Option<&dyn SourceProvider> {
        self.providers.iter().find(|p| p.id() == id).map(|b| b.as_ref())
    }

    /// Sync every provider concurrently; each writes its own slice via
    /// `replace_source` only on a successful, non-empty fetch (keeps last-good on failure).
    pub async fn sync_all_into(&self, cache: &CatalogCache) -> SyncReport {
        let ctx = SyncCtx;
        let futures = self.providers.iter().map(|p| async {
            (p.id().to_string(), p.sync(&ctx).await)
        });
        let results = futures::future::join_all(futures).await;
        let mut report = SyncReport { synced: vec![], failed: vec![] };
        for (id, res) in results {
            match res {
                Ok(entries) if !entries.is_empty() => {
                    if let Err(e) = cache.replace_source(&id, &entries).await {
                        report.failed.push((id, e.to_string()));
                    } else {
                        report.synced.push((id, entries.len()));
                    }
                }
                Ok(_) => report.failed.push((id, "empty result; kept last-good cache".into())),
                Err(e) => report.failed.push((id, e.to_string())),
            }
        }
        report
    }
}

impl Default for ProviderRegistry { fn default() -> Self { Self::new() } }
```
Add `pub mod provider;` to `src/store/mod.rs`. (`futures` crate is already a workspace dep; if not, use `tokio::join!`/a loop.)

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p alephcore store::provider::tests`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/store/provider/mod.rs src/store/mod.rs Cargo.toml
git commit -m "feat(store): SourceProvider trait + ProviderRegistry with concurrent sync"
```

---

### Task 2: Plugin-marketplace provider

**Files:**
- Create: `src/store/provider/marketplace.rs`
- Modify: `src/store/provider/mod.rs` (`pub mod marketplace;`)
- Test: inline `#[cfg(test)]`

**Interfaces:**
- Consumes: `MarketplaceManager`, `parse_marketplace_manifest`, `MarketplacePluginEntry`.
- Produces: `MarketplaceProvider { manager: MarketplaceManager }`; pure fn `plugin_entry_to_extension(provider_id, &MarketplacePluginEntry) -> ExtensionEntry`.

- [ ] **Step 1: Write the failing test**

```rust
use crate::extension::marketplace::types::MarketplacePluginEntry;
use crate::store::types::{ExtensionKind, TrustTier};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maps_plugin_entry() {
        let pe = MarketplacePluginEntry {
            name: "hello".into(),
            source: "acme/hello".into(),
            description: Some("Says hi".into()),
            version: Some("0.2.0".into()),
            sha256: Some("abc123".into()),
        };
        let e = plugin_entry_to_extension("cc-marketplace", &pe);
        assert_eq!(e.kind, ExtensionKind::Plugin);
        assert_eq!(e.id, "cc-marketplace:hello");
        assert_eq!(e.source_id, "cc-marketplace");
        assert_eq!(e.trust_tier, TrustTier::Verified);
        assert_eq!(e.version.as_deref(), Some("0.2.0"));
        assert!(e.tags.contains(&"plugin".to_string()));
    }
}
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p alephcore store::provider::marketplace::tests::maps_plugin_entry` → FAIL.

- [ ] **Step 3: Implement** (above test)

```rust
use crate::extension::marketplace::manifest::parse_marketplace_manifest;
use crate::extension::marketplace::types::MarketplacePluginEntry;
use crate::extension::marketplace::MarketplaceManager;
use crate::store::provider::{SourceError, SourceProvider, SyncCtx};
use crate::store::types::{ExtensionCategory, ExtensionEntry, ExtensionKind, InstallSpec, TrustTier};

pub fn plugin_entry_to_extension(provider_id: &str, pe: &MarketplacePluginEntry) -> ExtensionEntry {
    ExtensionEntry {
        id: format!("{provider_id}:{}", pe.name),
        kind: ExtensionKind::Plugin,
        category: ExtensionCategory::Other,
        name: pe.name.clone(),
        description: pe.description.clone().unwrap_or_default(),
        author: None,
        icon: None,
        tags: vec!["plugin".into()],
        version: pe.version.clone(),
        source_id: provider_id.to_string(),
        repo_url: Some(pe.source.clone()),
        trust_tier: TrustTier::Verified, // Anthropic-screened marketplaces
        requires_config: false,
        config_schema: None,
        installed: false,
        enabled: false,
        update_available: false,
    }
}

pub struct MarketplaceProvider {
    pub manager: MarketplaceManager,
    pub provider_id: String,
}

#[async_trait::async_trait]
impl SourceProvider for MarketplaceProvider {
    fn id(&self) -> &str { &self.provider_id }
    fn kinds(&self) -> &[ExtensionKind] { &[ExtensionKind::Plugin, ExtensionKind::Skill] }
    fn trust_tier(&self) -> TrustTier { TrustTier::Verified }

    async fn sync(&self, _ctx: &SyncCtx) -> Result<Vec<ExtensionEntry>, SourceError> {
        // MarketplaceManager is sync/blocking (git + fs). Run on a blocking thread.
        let manager_names: Vec<String> = self.manager.list().keys().cloned().collect();
        let mut out = Vec::new();
        for name in manager_names {
            let cache_dir = self.manager.update(&name).map_err(SourceError::Network)?;
            let manifest = parse_marketplace_manifest(&cache_dir).map_err(SourceError::Parse)?;
            out.extend(manifest.plugins.iter().map(|pe| plugin_entry_to_extension(&self.provider_id, pe)));
        }
        Ok(out)
    }

    async fn resolve_install_spec(&self, entry: &ExtensionEntry) -> Result<InstallSpec, SourceError> {
        // Plugins install via the marketplace path; the InstallSpec carries the git source.
        let repo = entry.repo_url.clone().ok_or_else(|| SourceError::Other("missing repo_url".into()))?;
        Ok(InstallSpec::GitDir { git_url: repo, subdir: None, git_ref: None, sha256: None })
    }
}
```
> Note: `self.manager.update()` does git/fs I/O synchronously. For v1 this runs inside `sync()` on a background task, which is acceptable; if it blocks the runtime noticeably, wrap the per-marketplace body in `tokio::task::spawn_blocking`. The actual plugin install in P2 still goes through `install_to_scope` (which also does the SHA256 verify), so the `GitDir` spec here is the routing hint, not the installer.

Add `pub mod marketplace;` to `src/store/provider/mod.rs`.

- [ ] **Step 4: Run to verify it passes** — `cargo test -p alephcore store::provider::marketplace::tests` → PASS.

- [ ] **Step 5: Commit**
```bash
git add src/store/provider/marketplace.rs src/store/provider/mod.rs
git commit -m "feat(store): plugin-marketplace SourceProvider"
```

---

### Task 3: Official MCP registry provider

**Files:**
- Create: `src/store/provider/mcp_registry.rs`
- Modify: `src/store/provider/mod.rs` (`pub mod mcp_registry;`)
- Test: inline `#[cfg(test)]` with a captured JSON fixture

**Interfaces:**
- Produces: serde structs for the registry response subset; pure fns `server_to_extension(&RegistryServer) -> ExtensionEntry` and `server_to_install_spec(&RegistryServer) -> Option<InstallSpec>` (+ `synthesize_config_schema(&[RegistryEnvVar]) -> Option<Value>`); `McpRegistryProvider { base_url: String, http: reqwest::Client }`.

- [ ] **Step 1: Write the failing test** (fixture mirrors `GET /v0/servers` server.json shape)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::types::{ExtensionKind, InstallSpec};

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
        assert_eq!(e.name, "github");                 // tail of reverse-DNS name
        assert!(e.requires_config);                    // has a required env var
        assert!(e.config_schema.is_some());
        assert_eq!(e.repo_url.as_deref(), Some("https://github.com/acme/github-mcp"));
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
```

- [ ] **Step 2: Run to verify it fails** — FAIL (types/fns undefined).

- [ ] **Step 3: Implement** (above test)

```rust
use crate::store::provider::{Query, SourceError, SourceProvider, SyncCtx};
use crate::store::types::{
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
pub struct RegistryMeta { #[serde(default, rename = "nextCursor")] pub next_cursor: Option<String> }

#[derive(Debug, Deserialize)]
pub struct RegistryServer { pub server: ServerDetail }
#[derive(Debug, Deserialize)]
pub struct ServerDetail {
    pub name: String,
    #[serde(default)] pub description: String,
    #[serde(default)] pub version: Option<String>,
    #[serde(default)] pub repository: Option<Repository>,
    #[serde(default)] pub packages: Vec<Package>,
    #[serde(default)] pub remotes: Vec<Remote>,
}
#[derive(Debug, Deserialize)]
pub struct Repository { pub url: String }
#[derive(Debug, Deserialize)]
pub struct Package {
    #[serde(rename = "runtimeHint")] pub runtime_hint: Option<String>,
    pub identifier: String,
    #[serde(default, rename = "runtimeArguments")] pub runtime_arguments: Vec<Argument>,
    #[serde(default, rename = "packageArguments")] pub package_arguments: Vec<Argument>,
    #[serde(default, rename = "environmentVariables")] pub environment_variables: Vec<RegistryEnvVar>,
    #[serde(default)] pub transport: Option<Transport>,
}
#[derive(Debug, Deserialize)]
pub struct Argument { #[serde(default)] pub value: Option<String> }
#[derive(Debug, Deserialize)]
pub struct Transport { #[serde(rename = "type")] pub kind: String }
#[derive(Debug, Deserialize)]
pub struct RegistryEnvVar {
    pub name: String,
    #[serde(default)] pub description: Option<String>,
    #[serde(default, rename = "isRequired")] pub is_required: bool,
    #[serde(default, rename = "isSecret")] pub is_secret: bool,
    #[serde(default)] pub default: Option<String>,
}
#[derive(Debug, Deserialize)]
pub struct Remote { #[serde(rename = "type")] pub kind: String, pub url: String }

fn name_tail(reverse_dns: &str) -> &str { reverse_dns.rsplit('/').next().unwrap_or(reverse_dns) }

pub fn synthesize_config_schema(envs: &[RegistryEnvVar]) -> Option<Value> {
    if envs.is_empty() { return None; }
    let mut props = serde_json::Map::new();
    let mut required = Vec::new();
    for e in envs {
        let mut field = json!({ "type": "string" });
        if let Some(d) = &e.description { field["description"] = json!(d); }
        if e.is_secret { field["x-sensitive"] = json!(true); }
        if let Some(def) = &e.default { field["default"] = json!(def); }
        props.insert(e.name.clone(), field);
        if e.is_required { required.push(e.name.clone()); }
    }
    Some(json!({ "type": "object", "properties": props, "required": required }))
}

pub fn server_to_extension(s: &RegistryServer) -> ExtensionEntry {
    let d = &s.server;
    let envs: Vec<&RegistryEnvVar> = d.packages.iter().flat_map(|p| p.environment_variables.iter()).collect();
    let any_required = envs.iter().any(|e| e.is_required);
    let owned_envs: Vec<RegistryEnvVar> = d.packages.iter().flat_map(|p| p.environment_variables.clone()).collect();
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
    }
}

pub fn server_to_install_spec(s: &RegistryServer) -> Option<InstallSpec> {
    let d = &s.server;
    if let Some(pkg) = d.packages.first() {
        let command = pkg.runtime_hint.clone().unwrap_or_else(|| "npx".into());
        let mut args: Vec<String> = pkg.runtime_arguments.iter().filter_map(|a| a.value.clone()).collect();
        args.push(pkg.identifier.clone());
        args.extend(pkg.package_arguments.iter().filter_map(|a| a.value.clone()));
        let env = pkg.environment_variables.iter().map(|e| EnvDecl {
            name: e.name.clone(), description: e.description.clone(),
            required: e.is_required, secret: e.is_secret, default: e.default.clone(), placeholder: None,
        }).collect();
        return Some(InstallSpec::McpStdio { command, args, env });
    }
    if let Some(rem) = d.remotes.first() {
        let transport = match rem.kind.as_str() {
            "sse" => McpTransport::Sse, _ => McpTransport::StreamableHttp,
        };
        return Some(InstallSpec::McpRemote { url: rem.url.clone(), transport, headers: vec![] });
    }
    None
}

pub struct McpRegistryProvider { pub base_url: String, pub http: reqwest::Client }

impl McpRegistryProvider {
    pub fn new() -> Self { Self { base_url: DEFAULT_BASE_URL.into(), http: reqwest::Client::new() } }
}

#[async_trait::async_trait]
impl SourceProvider for McpRegistryProvider {
    fn id(&self) -> &str { "mcp-official" }
    fn kinds(&self) -> &[ExtensionKind] { &[ExtensionKind::Mcp] }
    fn trust_tier(&self) -> TrustTier { TrustTier::Community }

    async fn sync(&self, _ctx: &SyncCtx) -> Result<Vec<ExtensionEntry>, SourceError> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut url = format!("{}/v0/servers?limit=100", self.base_url);
            if let Some(c) = &cursor { url.push_str(&format!("&cursor={c}")); }
            let resp = self.http.get(&url).timeout(std::time::Duration::from_secs(30)).send().await
                .map_err(|e| SourceError::Network(e.to_string()))?;
            if !resp.status().is_success() {
                return Err(SourceError::Network(format!("HTTP {}", resp.status())));
            }
            let body: RegistryResponse = resp.json().await.map_err(|e| SourceError::Parse(e.to_string()))?;
            out.extend(body.servers.iter().map(server_to_extension));
            match body.metadata.next_cursor { Some(c) if !c.is_empty() => cursor = Some(c), _ => break }
            if out.len() > 10_000 { break; } // safety bound
        }
        Ok(out)
    }

    async fn resolve_install_spec(&self, entry: &ExtensionEntry) -> Result<InstallSpec, SourceError> {
        let native = entry.id.strip_prefix("mcp-official:").unwrap_or(&entry.id);
        let url = format!("{}/v0/servers/{}/versions/latest", self.base_url, urlencoding::encode(native));
        let resp = self.http.get(&url).timeout(std::time::Duration::from_secs(30)).send().await
            .map_err(|e| SourceError::Network(e.to_string()))?;
        let server: RegistryServer = resp.json().await.map_err(|e| SourceError::Parse(e.to_string()))?;
        server_to_install_spec(&server).ok_or_else(|| SourceError::Other("no installable package/remote".into()))
    }
}
```
> Notes: (a) `urlencoding` crate — if not a dep, encode `/` → `%2F` manually. (b) The detail endpoint path (`/v0/servers/{name}/versions/latest`) per the registry API; if the deployed API uses `/v0.1/`, make `base_url` include the version segment. (c) `synthesize_config_schema` uses `x-sensitive` to drive the P2 config wizard's masked fields.

Add `pub mod mcp_registry;` to `src/store/provider/mod.rs`.

- [ ] **Step 4: Run to verify it passes** — `cargo test -p alephcore store::provider::mcp_registry::tests` → PASS (2 tests).

- [ ] **Step 5: Commit**
```bash
git add src/store/provider/mcp_registry.rs src/store/provider/mod.rs
git commit -m "feat(store): official MCP registry SourceProvider"
```

---

### Task 4: Docker MCP catalog provider

**Files:**
- Create: `src/store/provider/docker_mcp.rs`
- Modify: `src/store/provider/mod.rs` (`pub mod docker_mcp;`)
- Test: inline `#[cfg(test)]` with a captured YAML fixture

**Interfaces:**
- Produces: serde structs for the Docker catalog YAML subset; `docker_server_to_extension(name, &DockerServer) -> ExtensionEntry`; `DockerMcpProvider`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::types::{ExtensionKind, InstallSpec, TrustTier};

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
        let (name, srv) = cat.registry.iter().find(|(n, _)| *n == "github").unwrap();
        let e = docker_server_to_extension(name, srv);
        assert_eq!(e.kind, ExtensionKind::Mcp);
        assert_eq!(e.id, "docker-mcp:github");
        assert_eq!(e.trust_tier, TrustTier::Official);  // signed images
        match docker_install_spec(srv) {
            InstallSpec::OciImage { image } => assert_eq!(image, "mcp/github@sha256:abc123"),
            _ => panic!("expected OciImage"),
        }
    }
}
```

- [ ] **Step 2: Run to verify it fails** → FAIL.

- [ ] **Step 3: Implement** (above test)

```rust
use crate::store::provider::{SourceError, SourceProvider, SyncCtx};
use crate::store::types::{ExtensionCategory, ExtensionEntry, ExtensionKind, InstallSpec, TrustTier};
use serde::Deserialize;
use std::collections::BTreeMap;

pub const DEFAULT_CATALOG_URL: &str =
    "https://desktop.docker.com/mcp/catalog/v2/catalog.yaml";

#[derive(Debug, Deserialize)]
pub struct DockerCatalog { #[serde(default)] pub registry: BTreeMap<String, DockerServer> }
#[derive(Debug, Deserialize)]
pub struct DockerServer {
    #[serde(default)] pub description: String,
    #[serde(default)] pub image: Option<String>,
    #[serde(default)] pub category: Option<String>,
}

pub fn docker_install_spec(s: &DockerServer) -> InstallSpec {
    InstallSpec::OciImage { image: s.image.clone().unwrap_or_default() }
}

pub fn docker_server_to_extension(name: &str, s: &DockerServer) -> ExtensionEntry {
    ExtensionEntry {
        id: format!("docker-mcp:{name}"),
        kind: ExtensionKind::Mcp,
        category: ExtensionCategory::Other,
        name: name.to_string(),
        description: s.description.clone(),
        author: Some("docker".into()),
        icon: None,
        tags: vec!["mcp".into(), "container".into()],
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

pub struct DockerMcpProvider { pub url: String, pub http: reqwest::Client }
impl DockerMcpProvider {
    pub fn new() -> Self { Self { url: DEFAULT_CATALOG_URL.into(), http: reqwest::Client::new() } }
}

#[async_trait::async_trait]
impl SourceProvider for DockerMcpProvider {
    fn id(&self) -> &str { "docker-mcp" }
    fn kinds(&self) -> &[ExtensionKind] { &[ExtensionKind::Mcp] }
    fn trust_tier(&self) -> TrustTier { TrustTier::Official }

    async fn sync(&self, _ctx: &SyncCtx) -> Result<Vec<ExtensionEntry>, SourceError> {
        let text = self.http.get(&self.url).timeout(std::time::Duration::from_secs(30)).send().await
            .map_err(|e| SourceError::Network(e.to_string()))?
            .text().await.map_err(|e| SourceError::Network(e.to_string()))?;
        let cat: DockerCatalog = serde_yaml::from_str(&text).map_err(|e| SourceError::Parse(e.to_string()))?;
        Ok(cat.registry.iter().map(|(n, s)| docker_server_to_extension(n, s)).collect())
    }

    async fn resolve_install_spec(&self, entry: &ExtensionEntry) -> Result<InstallSpec, SourceError> {
        // Docker entries' install spec is fully determined by the catalog row;
        // a fresh sync would carry it. For v1, re-fetch and find by id.
        let text = self.http.get(&self.url).send().await
            .map_err(|e| SourceError::Network(e.to_string()))?
            .text().await.map_err(|e| SourceError::Network(e.to_string()))?;
        let cat: DockerCatalog = serde_yaml::from_str(&text).map_err(|e| SourceError::Parse(e.to_string()))?;
        let name = entry.id.strip_prefix("docker-mcp:").unwrap_or(&entry.id);
        cat.registry.get(name).map(docker_install_spec)
            .ok_or_else(|| SourceError::Other("server not in catalog".into()))
    }
}
```
> Note: confirm the Docker catalog top-level key (`registry:`) against a live fetch; the schema is YAML `registry: { name: {image, description, category, ...} }`. If the field differs, adjust the `DockerCatalog` struct only.

Add `pub mod docker_mcp;` to `src/store/provider/mod.rs`.

- [ ] **Step 4: Run to verify it passes** — `cargo test -p alephcore store::provider::docker_mcp::tests` → PASS.

- [ ] **Step 5: Commit**
```bash
git add src/store/provider/docker_mcp.rs src/store/provider/mod.rs
git commit -m "feat(store): Docker MCP catalog SourceProvider"
```

---

### Task 5: Wire providers + background sync + `extensions.sources.*` + catalog smoke

**Files:**
- Create: `src/store/provider/registry_builder.rs` (assemble the v1 `ProviderRegistry`)
- Create: `src/gateway/handlers/extensions/sources.rs`
- Modify: `src/gateway/handlers/extensions/mod.rs`, the startup builder (spawn initial sync), registration file from P0 Task 7
- Test: inline `#[cfg(test)]` for the builder

**Interfaces:**
- Produces: `build_default_registry(marketplaces: HashMap<String, MarketplaceConfig>) -> ProviderRegistry`; handlers `extensions.sources.list`, `extensions.sources.refresh` (triggers `sync_all_into`).

- [ ] **Step 1: Write the failing test** in `registry_builder.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    #[test]
    fn default_registry_has_three_providers() {
        let reg = build_default_registry(HashMap::new());
        assert!(reg.get("mcp-official").is_some());
        assert!(reg.get("docker-mcp").is_some());
        assert!(reg.get("cc-marketplace").is_some());
    }
}
```

- [ ] **Step 2: Run to verify it fails** → FAIL.

- [ ] **Step 3: Implement the builder**

```rust
use std::collections::HashMap;
use crate::extension::marketplace::types::MarketplaceConfig;
use crate::extension::marketplace::MarketplaceManager;
use crate::store::provider::docker_mcp::DockerMcpProvider;
use crate::store::provider::marketplace::MarketplaceProvider;
use crate::store::provider::mcp_registry::McpRegistryProvider;
use crate::store::provider::ProviderRegistry;

pub fn build_default_registry(marketplaces: HashMap<String, MarketplaceConfig>) -> ProviderRegistry {
    let mut reg = ProviderRegistry::new();
    reg.register(Box::new(McpRegistryProvider::new()));
    reg.register(Box::new(DockerMcpProvider::new()));
    reg.register(Box::new(MarketplaceProvider {
        manager: MarketplaceManager::new(marketplaces, None),
        provider_id: "cc-marketplace".into(),
    }));
    reg
}
```
Add `pub mod registry_builder;` to `src/store/provider/mod.rs`.

- [ ] **Step 4: Run to verify it passes** — `cargo test -p alephcore store::provider::registry_builder::tests` → PASS.

- [ ] **Step 5: Implement `extensions.sources.*`** in `sources.rs` (list returns provider ids+tiers; refresh runs sync)

```rust
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use crate::store::cache::CatalogCache;
use crate::store::provider::ProviderRegistry;
use serde_json::json;
use std::sync::Arc;

pub async fn handle_refresh(req: JsonRpcRequest, reg: Arc<ProviderRegistry>, cache: Arc<CatalogCache>) -> JsonRpcResponse {
    let report = reg.sync_all_into(&cache).await;
    JsonRpcResponse::success(req.id, json!({
        "synced": report.synced.iter().map(|(id, n)| json!({"source": id, "count": n})).collect::<Vec<_>>(),
        "failed": report.failed.iter().map(|(id, e)| json!({"source": id, "error": e})).collect::<Vec<_>>(),
    }))
}
```
Wire `pub mod sources;` in `extensions/mod.rs`; register `extensions.sources.refresh` capturing `Arc<ProviderRegistry>` + `Arc<CatalogCache>` (mirror P0 Task 7).

- [ ] **Step 6: Spawn an initial background sync at startup**

In the builder (after constructing the registry + cache):
```rust
let registry = std::sync::Arc::new(
    alephcore::store::provider::registry_builder::build_default_registry(marketplace_configs.clone())
);
{
    let registry = registry.clone();
    let cache = catalog_cache.clone();
    tokio::spawn(async move {
        let report = registry.sync_all_into(&cache).await;
        tracing::info!(synced = ?report.synced, failed = ?report.failed, "initial extensions catalog sync");
    });
}
```
> `marketplace_configs` is the same `[plugin_marketplaces]` map the existing `build_marketplace_manager()` uses (see `src/gateway/handlers/plugins/handlers.rs`).

- [ ] **Step 7: Build + smoke**

Run: `cargo build -p alephcore && cargo build -p aleph-server`
Then start the server, wait for the initial sync log, and call:
```json
{ "jsonrpc": "2.0", "id": 1, "method": "extensions.catalog", "params": { "kind": "mcp" } }
```
Expected: `result.extensions` contains real MCP servers from the official registry + Docker catalog (names, descriptions, `requires_config` set where the server declares required env). Re-running offline still returns them (served from the rusqlite cache).

Then:
```json
{ "jsonrpc": "2.0", "id": 2, "method": "extensions.sources.refresh", "params": {} }
```
Expected: `result.synced` lists `mcp-official`, `docker-mcp`, `cc-marketplace` with counts.

- [ ] **Step 8: Commit**
```bash
git add src/store/provider/registry_builder.rs src/gateway/handlers/extensions/ src/bin/aleph-server/commands/start/builder/
git commit -m "feat(store): wire v1 providers + background sync + extensions.sources.*"
```

---

## Self-review (P1)

**Spec coverage (P1 scope):** `SourceProvider`/`ProviderRegistry` §6 → Task 1 ✓; marketplace provider → Task 2 ✓; official MCP registry (deterministic install spec + synthesized config_schema) → Task 3 ✓; Docker catalog (signed/official tier) → Task 4 ✓; concurrent sync into cache, keep-last-good on failure → Task 1 §`sync_all_into` ✓; background sync + sources RPC → Task 5 ✓. ClawHub/GitHub crawler correctly absent (deferred). Category=Other (P4 assigns) ✓.

**Placeholder scan:** every step has complete code. Notes flag live-API confirmations (registry version path, Docker top-level key, `urlencoding` dep) with concrete fallbacks — not placeholders.

**Type consistency:** all providers produce `ExtensionEntry` (P0) with `source_id` matching `id()` (`mcp-official`/`docker-mcp`/`cc-marketplace`), so `replace_source` keys align. `InstallSpec` variants (`McpStdio`/`McpRemote`/`OciImage`/`GitDir`) match P0 definitions. `ProviderRegistry::get/register/sync_all_into` signatures consistent across Tasks 1 & 5. `synthesize_config_schema` emits `x-sensitive`, consumed by P2's config wizard.

**Consumed-by-P2 handoff:** P2 install routing consumes `SourceProvider::resolve_install_spec` (per-provider) + `MarketplaceProvider`/`MarketplaceManager::install_to_scope` + `verify_plugin_integrity`; P2 must add the trust-gate before calling them.
