use crate::config::types::{FetchBackendConfig, FetchConfigInternal};
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
        // Self-gate on the top-level `enabled` flag. Without this, any caller
        // of `from_config` (now or future) would silently get a populated
        // registry when the operator set `[fetch].enabled = false` — the
        // external gate in
        // `executor/builtin_registry/builder/constructor/mod.rs` masked this
        // today, but the registry API contract is broken if the registry
        // itself does not respect its own config knob.
        if !cfg.enabled {
            return Self {
                providers: HashMap::new(),
                order: Vec::new(),
            };
        }
        let factories = FetchProviderFactoryRegistry::with_defaults();
        let mut providers: HashMap<String, Arc<dyn FetchProvider>> = HashMap::new();
        for (name, backend) in &cfg.backends {
            if let Some(factory) = factories.get(&backend.provider_type) {
                match factory.build(backend, ctx) {
                    Ok(Some(p)) => {
                        providers.insert(name.clone(), p);
                    }
                    Ok(None) => log::warn!("fetch backend '{name}' skipped (unconfigured)"),
                    Err(e) => log::warn!("fetch backend '{name}' build failed: {e}"),
                }
            }
        }

        // Strategy V: Firecrawl shares the [search] config (Decision A) and needs
        // no [fetch] backend entry. Derive it from search when not already built.
        //
        // KNOWN LIMITATION: there is no per-fetch-backend disable gate on the
        // search side (`SearchBackendConfig` has no `enabled` field). The
        // synthetic entry below is therefore built with `enabled: true` to
        // honour the operator's `[fetch].enabled = true` top-level knob — an
        // operator who wants Firecrawl for search but NOT for fetch currently
        // cannot express this without disabling fetch entirely. Once a
        // per-fetch override lands on `FetchConfigInternal`, replace this with
        // the dedicated opt-in.
        if !providers.contains_key("firecrawl") {
            if let Some(factory) = factories.get("firecrawl") {
                let synthetic = FetchBackendConfig {
                    provider_type: "firecrawl".to_string(),
                    api_key: None,
                    base_url: None,
                    timeout_seconds: None,
                    verified: false,
                    enabled: cfg.enabled,
                };
                if let Ok(Some(p)) = factory.build(&synthetic, ctx) {
                    providers.insert("firecrawl".to_string(), p);
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
            for n in fb {
                push(n, &mut order);
            }
        }
        // Auto-fallback tail: every other built provider, in stable (sorted) order.
        let mut rest: Vec<String> = providers.keys().cloned().collect();
        rest.sort();
        for n in &rest {
            push(n, &mut order);
        }
        Self { providers, order }
    }

    /// Providers to try, in order. Empty when nothing is configured/available.
    pub fn select(&self) -> Vec<Arc<dyn FetchProvider>> {
        self.order
            .iter()
            .filter_map(|n| self.providers.get(n).cloned())
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{FetchBackendConfig, FetchConfigInternal, SearchConfigInternal};
    use std::collections::HashMap;

    fn ctx_no_search() -> FetchBuildCtx<'static> {
        FetchBuildCtx {
            search: None,
            resolve_secret: &|_| None,
        }
    }

    #[test]
    fn builds_crawl4ai_and_orders_default_first() {
        let mut backends = HashMap::new();
        backends.insert(
            "crawl4ai".into(),
            FetchBackendConfig {
                provider_type: "crawl4ai".into(),
                api_key: None,
                base_url: Some("http://x:11235".into()),
                timeout_seconds: Some(60),
                verified: false,
                enabled: true,
            },
        );
        let cfg = FetchConfigInternal {
            enabled: true,
            default_provider: "crawl4ai".into(),
            fallback_providers: None,
            backends,
        };
        let reg = FetchRegistry::from_config(&cfg, &ctx_no_search());
        let sel = reg.select();
        assert_eq!(sel.len(), 1);
        assert_eq!(sel[0].name(), "crawl4ai");
    }

    #[test]
    fn firecrawl_unavailable_without_search_config() {
        let mut backends = HashMap::new();
        backends.insert(
            "firecrawl".into(),
            FetchBackendConfig {
                provider_type: "firecrawl".into(),
                api_key: None,
                base_url: None,
                timeout_seconds: None,
                verified: false,
                enabled: true,
            },
        );
        let cfg = FetchConfigInternal {
            enabled: true,
            default_provider: "firecrawl".into(),
            fallback_providers: None,
            backends,
        };
        let reg = FetchRegistry::from_config(&cfg, &ctx_no_search());
        assert!(
            reg.select().is_empty(),
            "no search firecrawl config → no provider"
        );
    }

    fn search_with_firecrawl() -> SearchConfigInternal {
        serde_json::from_value(serde_json::json!({
            "enabled": true,
            "default_provider": "firecrawl",
            "backends": {
                "firecrawl": { "provider_type": "firecrawl", "base_url": "https://api.firecrawl.dev" }
            }
        }))
        .unwrap()
    }

    #[test]
    fn firecrawl_built_from_search_without_fetch_backend() {
        let search = search_with_firecrawl();
        let resolve = |k: &str| -> Option<String> {
            (k == "search:firecrawl").then(|| "fc-token".to_string())
        };
        let ctx = FetchBuildCtx {
            search: Some(&search),
            resolve_secret: &resolve,
        };
        let cfg = FetchConfigInternal {
            enabled: true,
            default_provider: "firecrawl".into(),
            fallback_providers: None,
            backends: HashMap::new(), // no [fetch] backend entry for firecrawl
        };
        let reg = FetchRegistry::from_config(&cfg, &ctx);
        let sel = reg.select();
        assert_eq!(sel.len(), 1);
        assert_eq!(sel[0].name(), "firecrawl");
    }

    #[test]
    fn default_firecrawl_orders_first_then_crawl4ai_fallback() {
        let search = search_with_firecrawl();
        let resolve = |k: &str| -> Option<String> {
            (k == "search:firecrawl").then(|| "fc-token".to_string())
        };
        let ctx = FetchBuildCtx {
            search: Some(&search),
            resolve_secret: &resolve,
        };
        let mut backends = HashMap::new();
        backends.insert(
            "crawl4ai".into(),
            FetchBackendConfig {
                provider_type: "crawl4ai".into(),
                api_key: None,
                base_url: Some("http://x:11235".into()),
                timeout_seconds: Some(60),
                verified: false,
                enabled: true,
            },
        );
        let cfg = FetchConfigInternal {
            enabled: true,
            default_provider: "firecrawl".into(),
            fallback_providers: None,
            backends,
        };
        let sel = FetchRegistry::from_config(&cfg, &ctx).select();
        let names: Vec<&str> = sel.iter().map(|p| p.name()).collect();
        assert_eq!(names, vec!["firecrawl", "crawl4ai"]);
    }

    #[test]
    fn auto_fallback_appends_other_built_after_default() {
        let search = search_with_firecrawl();
        let resolve = |k: &str| -> Option<String> {
            (k == "search:firecrawl").then(|| "fc-token".to_string())
        };
        let ctx = FetchBuildCtx {
            search: Some(&search),
            resolve_secret: &resolve,
        };
        let mut backends = HashMap::new();
        backends.insert(
            "crawl4ai".into(),
            FetchBackendConfig {
                provider_type: "crawl4ai".into(),
                api_key: None,
                base_url: Some("http://x:11235".into()),
                timeout_seconds: Some(60),
                verified: false,
                enabled: true,
            },
        );
        let cfg = FetchConfigInternal {
            enabled: true,
            default_provider: "crawl4ai".into(),
            fallback_providers: None,
            backends,
        };
        let sel = FetchRegistry::from_config(&cfg, &ctx).select();
        let names: Vec<&str> = sel.iter().map(|p| p.name()).collect();
        assert_eq!(names, vec!["crawl4ai", "firecrawl"]);
    }

    /// The top-level `[fetch].enabled = false` knob must short-circuit
    /// `from_config` itself, not depend on every caller to gate the call.
    /// The executor-side constructor still gates today, but the registry's
    /// own contract requires it.
    #[test]
    fn disabled_fetch_returns_empty_registry() {
        let search = search_with_firecrawl();
        let resolve = |k: &str| -> Option<String> {
            (k == "search:firecrawl").then(|| "fc-token".to_string())
        };
        let ctx = FetchBuildCtx {
            search: Some(&search),
            resolve_secret: &resolve,
        };
        let mut backends = HashMap::new();
        backends.insert(
            "crawl4ai".into(),
            FetchBackendConfig {
                provider_type: "crawl4ai".into(),
                api_key: None,
                base_url: Some("http://x:11235".into()),
                timeout_seconds: Some(60),
                verified: false,
                enabled: true,
            },
        );
        let cfg = FetchConfigInternal {
            enabled: false,
            default_provider: "crawl4ai".into(),
            fallback_providers: None,
            backends,
        };
        let reg = FetchRegistry::from_config(&cfg, &ctx);
        assert!(
            reg.select().is_empty(),
            "from_config must respect the top-level enabled gate"
        );
        assert!(reg.is_empty());
    }
}
