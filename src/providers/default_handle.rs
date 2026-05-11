//! `DefaultProviderHandle` — live resolver for the configured default provider.
//!
//! Without this trait the harness caches `Arc<dyn AiProvider>` at boot and
//! never picks up `set_default_provider` UI changes; the user must restart the
//! server. The handle wraps either a static `Arc` (tests / non-registry boot
//! paths) or a `MultiProviderRegistry` (production), and resolves the current
//! default on every dispatch via `current()`. The registry already provides
//! interior mutability through `RwLock`, so no caching or event subscription
//! is needed here — `pick_llm` simply asks for the live default each turn.
//!
//! See: docs/superpowers/specs/2026-05-12-default-provider-hot-reload.md

use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

/// Resolves the current default provider on demand.
///
/// The orchestrator harness holds this instead of a frozen `Arc<dyn AiProvider>`
/// so that UI-driven default-provider swaps take effect on the very next turn,
/// without a restart.
pub trait DefaultProviderHandle: Send + Sync {
    /// Return the provider currently configured as default.
    fn current(&self) -> Arc<dyn AiProvider>;
}

/// Trivial handle that always returns the same provider. Used by tests and any
/// boot path that has no registry (env-only single-provider mode).
pub struct StaticDefault(Arc<dyn AiProvider>);

impl StaticDefault {
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self(provider)
    }
}

impl DefaultProviderHandle for StaticDefault {
    fn current(&self) -> Arc<dyn AiProvider> {
        self.0.clone()
    }
}

// `MultiProviderRegistry` impls `DefaultProviderHandle` directly (see
// `crate::thinker::mod`). The orchestrator boot path passes the registry as
// `Arc<dyn DefaultProviderHandle>` so each turn reads through the registry's
// `RwLock`.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result;
    use crate::providers::adapter::{ProviderResponse, RequestPayload};
    use std::future::Future;
    use std::pin::Pin;

    struct LabeledProvider(&'static str);
    impl AiProvider for LabeledProvider {
        fn process<'a>(
            &'a self,
            _req: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
            Box::pin(async { Ok(ProviderResponse::default()) })
        }
        fn name(&self) -> &str {
            self.0
        }
        fn color(&self) -> &str {
            "#000"
        }
    }

    #[test]
    fn static_default_always_returns_same_provider() {
        let p: Arc<dyn AiProvider> = Arc::new(LabeledProvider("alpha"));
        let handle = StaticDefault::new(p);
        assert_eq!(handle.current().name(), "alpha");
        assert_eq!(handle.current().name(), "alpha");
    }
}
