//! LLM provider selection logic for harness bridge.

use std::collections::HashMap;
use std::sync::Arc;

use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_spec::BrainRef;
use crate::providers::{AiProvider, DefaultProviderHandle};

/// Pick the `AiProvider` for a given [`BrainRef`]. `Strict` returns
/// `ProviderUnavailable` when the named provider is not registered; model
/// matching is deferred to Phase 6.
///
/// `default_provider` is a live handle (see Step 5 hot-reload): every call to
/// `.current()` reads through the registry's `RwLock`, so UI-driven
/// `set_default` swaps take effect on the very next turn.
pub(super) fn pick_llm(
    brain: &BrainRef,
    default_provider: &Arc<dyn DefaultProviderHandle>,
    named: &HashMap<String, Arc<dyn AiProvider>>,
) -> Result<Arc<dyn AiProvider>, FlowError> {
    match brain {
        BrainRef::Default => Ok(default_provider.current()),
        BrainRef::Preferred { provider } => {
            if let Some(llm) = named.get(provider) {
                Ok(llm.clone())
            } else {
                // Silent fallback is intentional — Preferred means "use this if
                // available, otherwise default." A debug log signals the mismatch
                // so operators can spot misconfigured preferred providers.
                tracing::debug!(
                    provider = %provider,
                    "preferred provider not registered, falling back to default"
                );
                Ok(default_provider.current())
            }
        }
        BrainRef::Strict { provider, .. } => named
            .get(provider)
            .cloned()
            .ok_or_else(|| FlowError::ProviderUnavailable(provider.clone())),
    }
}
