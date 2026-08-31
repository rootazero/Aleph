use crate::config::types::{FetchBackendConfig, SearchConfigInternal};
use crate::error::Result;
use crate::fetch::providers::{Crawl4aiFetchProvider, FirecrawlFetchProvider};
use crate::fetch::FetchProvider;
use std::collections::HashMap;
use std::sync::Arc;

/// Context a factory may consult: the `[search]` config (for shared providers
/// like Firecrawl) and a vault secret resolver.
pub struct FetchBuildCtx<'a> {
    pub search: Option<&'a SearchConfigInternal>,
    pub resolve_secret: &'a dyn Fn(&str) -> Option<String>,
}

pub trait FetchProviderFactory: Send + Sync {
    fn provider_type(&self) -> &'static str;
    fn build(
        &self,
        backend: &FetchBackendConfig,
        ctx: &FetchBuildCtx,
    ) -> Result<Option<Arc<dyn FetchProvider>>>;
}

pub struct Crawl4aiFetchFactory;
impl FetchProviderFactory for Crawl4aiFetchFactory {
    fn provider_type(&self) -> &'static str {
        "crawl4ai"
    }
    fn build(
        &self,
        backend: &FetchBackendConfig,
        ctx: &FetchBuildCtx,
    ) -> Result<Option<Arc<dyn FetchProvider>>> {
        // Token precedence:
        // 1. `backend.api_key` — only set programmatically by `handle_test`
        //    in `gateway/handlers/fetch_config.rs`. The field is
        //    `#[serde(default, skip_serializing)]` on `FetchBackendConfig`,
        //    so operators CANNOT populate it in `config.toml` — the vault
        //    lookup below is the only production source.
        // 2. Vault key `fetch:crawl4ai`.
        // 3. Vault key `web_fetch:crawl4ai` (back-compat alias).
        let token = backend
            .api_key
            .clone()
            .or_else(|| (ctx.resolve_secret)("fetch:crawl4ai"))
            .or_else(|| (ctx.resolve_secret)("web_fetch:crawl4ai"));
        let mut b = backend.clone();
        b.api_key = token;
        Ok(Crawl4aiFetchProvider::from_backend(&b).map(|p| Arc::new(p) as Arc<dyn FetchProvider>))
    }
}

pub struct FirecrawlFetchFactory;
impl FetchProviderFactory for FirecrawlFetchFactory {
    fn provider_type(&self) -> &'static str {
        "firecrawl"
    }
    fn build(
        &self,
        _backend: &FetchBackendConfig,
        ctx: &FetchBuildCtx,
    ) -> Result<Option<Arc<dyn FetchProvider>>> {
        // Decision A (intentional coupling, see
        // docs/superpowers/specs/2026-06-28-fetch-provider-category-design.md):
        // the fetch Firecrawl backend ALWAYS reuses the `[search]` Firecrawl
        // backend (base_url from `search.backends["firecrawl"]`) and the
        // vault secret `search:firecrawl`. The `_backend` argument is
        // ignored on purpose: there is no separate fetch Firecrawl deployment.
        //
        // Operators with multi-tenant setups that want fetch and search to
        // hit different Firecrawl endpoints should run a separate Firecrawl
        // instance reachable from both `[search]` and `[fetch]` configs
        // (the current Firecrawl factory reads from `[search]` regardless).
        // Future change: honor `backend.base_url` / `backend.api_key` when
        // they are `Some`, falling back to search only when absent.
        let Some(search) = ctx.search else {
            return Ok(None);
        };
        let Some(fc) = search.backends.get("firecrawl") else {
            return Ok(None);
        };
        let Some(base_url) = fc.base_url.clone().filter(|s| !s.is_empty()) else {
            return Ok(None);
        };
        let Some(token) = (ctx.resolve_secret)("search:firecrawl") else {
            return Ok(None);
        };
        Ok(Some(Arc::new(FirecrawlFetchProvider::new(
            base_url, token,
        )?)))
    }
}

pub struct FetchProviderFactoryRegistry {
    factories: HashMap<&'static str, Box<dyn FetchProviderFactory>>,
}
impl FetchProviderFactoryRegistry {
    pub fn with_defaults() -> Self {
        let mut factories: HashMap<&'static str, Box<dyn FetchProviderFactory>> = HashMap::new();
        for f in [
            Box::new(Crawl4aiFetchFactory) as Box<dyn FetchProviderFactory>,
            Box::new(FirecrawlFetchFactory),
        ] {
            factories.insert(f.provider_type(), f);
        }
        Self { factories }
    }
    pub fn get(&self, provider_type: &str) -> Option<&dyn FetchProviderFactory> {
        self.factories.get(provider_type).map(|b| b.as_ref())
    }
}
