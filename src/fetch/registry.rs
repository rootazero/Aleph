use crate::config::types::FetchConfigInternal;
use crate::fetch::factory::{FetchBuildCtx, FetchProviderFactoryRegistry};
use crate::fetch::FetchProvider;
use std::collections::HashMap;
use std::sync::Arc;

/// Active fetch providers built from `[fetch]`, with a stable selection order.
pub struct FetchRegistry {
    providers: HashMap<String, Arc<dyn FetchProvider>>,
    order: Vec<String>, // default first, then fallbacks (only built ones)
}

impl FetchRegistry {
    pub fn from_config(cfg: &FetchConfigInternal, ctx: &FetchBuildCtx) -> Self {
        let factories = FetchProviderFactoryRegistry::with_defaults();
        let mut providers: HashMap<String, Arc<dyn FetchProvider>> = HashMap::new();
        for (name, backend) in &cfg.backends {
            if let Some(factory) = factories.get(&backend.provider_type) {
                match factory.build(backend, ctx) {
                    Ok(Some(p)) => { providers.insert(name.clone(), p); }
                    Ok(None) => log::warn!("fetch backend '{name}' skipped (unconfigured)"),
                    Err(e) => log::warn!("fetch backend '{name}' build failed: {e}"),
                }
            }
        }
        let mut order = Vec::new();
        let push = |n: &str, order: &mut Vec<String>| {
            if providers.contains_key(n) && !order.iter().any(|x| x == n) {
                order.push(n.to_string());
            }
        };
        push(&cfg.default_provider, &mut order);
        if let Some(fb) = &cfg.fallback_providers {
            for n in fb { push(n, &mut order); }
        }
        Self { providers, order }
    }

    /// Providers to try, in order. Empty when nothing is configured/available.
    pub fn select(&self) -> Vec<Arc<dyn FetchProvider>> {
        self.order.iter().filter_map(|n| self.providers.get(n).cloned()).collect()
    }

    pub fn is_empty(&self) -> bool { self.order.is_empty() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{FetchBackendConfig, FetchConfigInternal};
    use std::collections::HashMap;

    fn ctx_no_search() -> FetchBuildCtx<'static> {
        FetchBuildCtx { search: None, resolve_secret: &|_| None }
    }

    #[test]
    fn builds_crawl4ai_and_orders_default_first() {
        let mut backends = HashMap::new();
        backends.insert("crawl4ai".into(), FetchBackendConfig {
            provider_type: "crawl4ai".into(), api_key: None,
            base_url: Some("http://x:11235".into()), timeout_seconds: Some(60), verified: false,
        });
        let cfg = FetchConfigInternal {
            enabled: true, default_provider: "crawl4ai".into(),
            fallback_providers: None, backends,
        };
        let reg = FetchRegistry::from_config(&cfg, &ctx_no_search());
        let sel = reg.select();
        assert_eq!(sel.len(), 1);
        assert_eq!(sel[0].name(), "crawl4ai");
    }

    #[test]
    fn firecrawl_unavailable_without_search_config() {
        let mut backends = HashMap::new();
        backends.insert("firecrawl".into(), FetchBackendConfig {
            provider_type: "firecrawl".into(), api_key: None,
            base_url: None, timeout_seconds: None, verified: false,
        });
        let cfg = FetchConfigInternal {
            enabled: true, default_provider: "firecrawl".into(),
            fallback_providers: None, backends,
        };
        let reg = FetchRegistry::from_config(&cfg, &ctx_no_search());
        assert!(reg.select().is_empty(), "no search firecrawl config → no provider");
    }
}
