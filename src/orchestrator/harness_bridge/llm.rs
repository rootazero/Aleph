//! LLM provider selection logic for harness bridge.

use std::collections::HashMap;
use std::sync::Arc;

use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_spec::BrainRef;
use crate::providers::AiProvider;

/// Pick the `AiProvider` for a given [`BrainRef`]. `Strict` returns
/// `ProviderUnavailable` when the named provider is not registered; model
/// matching is deferred to Phase 6.
pub(super) fn pick_llm(
    brain: &BrainRef,
    default_provider: &Arc<dyn AiProvider>,
    named: &HashMap<String, Arc<dyn AiProvider>>,
) -> Result<Arc<dyn AiProvider>, FlowError> {
    match brain {
        BrainRef::Default => Ok(default_provider.clone()),
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
                Ok(default_provider.clone())
            }
        }
        BrainRef::Strict { provider, .. } => named
            .get(provider)
            .cloned()
            .ok_or_else(|| FlowError::ProviderUnavailable(provider.clone())),
    }
}
