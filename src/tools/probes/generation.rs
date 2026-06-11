//! `GenerationProbe` — gates a media-generation tool (`image_generate`,
//! `speech_generate`, `audio_generate`, `video_generate`) on whether the
//! shared [`GenerationProviderRegistry`] currently has a provider that
//! supports the tool's [`GenerationType`].
//!
//! The generation tools are gated **statically** at registration time
//! (`executor::builtin_registry::builder::optional_tools`) based on the
//! providers present at boot. But the registry lives behind an
//! `Arc<RwLock<…>>` and is hot-reloaded when the user edits provider config
//! from the panel (`generation_init`'s reload subscriber). A static gate
//! cannot reflect a provider that was removed mid-session — the LLM would keep
//! seeing a tool whose backend has gone away. This probe re-evaluates the live
//! registry each TTL window, so the tool disappears from the prompt the moment
//! its last supporting provider does.

use std::borrow::Cow;
use std::time::Duration;

use async_trait::async_trait;

use crate::generation::{GenerationProviderRegistry, GenerationType};
use crate::sync_primitives::{Arc, RwLock};
use crate::tool_metadata::{HealthReason, ProbeResult, ToolHealthProbe};

/// TTL for generation-capability probe results. A minute is a good balance:
/// long enough to avoid re-scanning the registry every turn, short enough that
/// a panel-driven provider change is reflected promptly.
const GENERATION_PROBE_TTL: Duration = Duration::from_secs(60);

/// Reports whether the live registry has at least one provider for `gen_type`.
pub struct GenerationProbe {
    registry: Arc<RwLock<GenerationProviderRegistry>>,
    gen_type: GenerationType,
}

impl GenerationProbe {
    pub const fn new(
        registry: Arc<RwLock<GenerationProviderRegistry>>,
        gen_type: GenerationType,
    ) -> Self {
        Self { registry, gen_type }
    }
}

#[async_trait]
impl ToolHealthProbe for GenerationProbe {
    async fn probe(&self) -> ProbeResult {
        // Poison-safe read: a panicking writer must not permanently disable
        // the capability gate (P7 lock safety).
        let reg = self.registry.read().unwrap_or_else(|e| e.into_inner());
        if reg.providers_for_type(self.gen_type).is_empty() {
            ProbeResult::Unhealthy {
                reason: HealthReason::DependencyDown(Cow::Owned(format!(
                    "no generation provider configured for {:?}",
                    self.gen_type
                ))),
                retry_after: None,
            }
        } else {
            ProbeResult::Healthy
        }
    }

    fn ttl(&self) -> Duration {
        GENERATION_PROBE_TTL
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::MockGenerationProvider;

    fn registry_with(
        provider: Option<MockGenerationProvider>,
    ) -> Arc<RwLock<GenerationProviderRegistry>> {
        let mut reg = GenerationProviderRegistry::new();
        if let Some(p) = provider {
            reg.register("mock".to_string(), Arc::new(p)).unwrap();
        }
        Arc::new(RwLock::new(reg))
    }

    #[tokio::test]
    async fn unhealthy_when_no_provider_for_type() {
        let reg = registry_with(None);
        let probe = GenerationProbe::new(reg, GenerationType::Image);
        match probe.probe().await {
            ProbeResult::Unhealthy { reason, .. } => {
                assert!(reason.short_label().contains("Image"));
            }
            ProbeResult::Healthy => panic!("expected Unhealthy with no providers"),
        }
    }

    #[tokio::test]
    async fn healthy_when_provider_supports_type() {
        let reg = registry_with(Some(MockGenerationProvider::image_only("dalle")));
        let probe = GenerationProbe::new(reg, GenerationType::Image);
        assert!(matches!(probe.probe().await, ProbeResult::Healthy));
    }

    #[tokio::test]
    async fn unhealthy_when_provider_lacks_requested_type() {
        // An image-only provider must not make `video_generate` look available.
        let reg = registry_with(Some(MockGenerationProvider::image_only("dalle")));
        let probe = GenerationProbe::new(reg, GenerationType::Video);
        assert!(matches!(probe.probe().await, ProbeResult::Unhealthy { .. }));
    }
}
