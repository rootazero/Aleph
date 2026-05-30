//! Cached prompt building for Anthropic prompt caching optimization
//!
//! Leverages the [`LayerStability`] classification to partition the
//! system prompt into a stable prefix (cacheable) and a dynamic suffix
//! that changes per request.

use crate::tools::info::ToolInfo;

use super::{PromptBuilder, SystemPromptPart};
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput};
use crate::thinker::prompt_mode::PromptMode;

impl PromptBuilder {
    /// Build two-part system prompt for Anthropic cache optimization.
    ///
    /// Returns a vector of [`SystemPromptPart`]s where:
    /// - Part 1: Stable layers (cacheable) — persona, tools, security, skills, etc.
    /// - Part 2: Dynamic layers (not cacheable) — inbound context, runtime, memory, etc.
    ///
    /// The stable/dynamic boundary is determined by each layer's
    /// [`stability()`](crate::thinker::prompt_layer::PromptLayer::stability)
    /// declaration, so adding new layers automatically classifies them.
    ///
    /// Equivalent to [`build_system_prompt_cached_with_mode`] with
    /// [`PromptMode::Full`] — kept as the back-compatible default entry point.
    ///
    /// [`build_system_prompt_cached_with_mode`]: Self::build_system_prompt_cached_with_mode
    pub fn build_system_prompt_cached(&self, tools: &[ToolInfo]) -> Vec<SystemPromptPart> {
        self.build_system_prompt_cached_with_mode(tools, PromptMode::Full)
    }

    /// Mode-aware variant of [`build_system_prompt_cached`].
    ///
    /// Threads `mode` through the layer pipeline so token-constrained
    /// deployments can opt into a leaner system prompt — `Compact` / `Minimal`
    /// shed the heavy guidance layers that declare `supports_mode(mode) ==
    /// false`, while `Full` reproduces the legacy assembly byte-for-byte. The
    /// stable/dynamic split (and thus the prompt-cache breakpoint) is
    /// preserved across all modes.
    ///
    /// [`build_system_prompt_cached`]: Self::build_system_prompt_cached
    pub fn build_system_prompt_cached_with_mode(
        &self,
        tools: &[ToolInfo],
        mode: PromptMode,
    ) -> Vec<SystemPromptPart> {
        let input = LayerInput::basic(&self.config, tools).with_mode(mode);
        let stable = self
            .pipeline
            .execute_stable_with_mode(AssemblyPath::Cached, &input, mode);
        let dynamic = self
            .pipeline
            .execute_dynamic_with_mode(AssemblyPath::Cached, &input, mode);

        vec![
            SystemPromptPart {
                content: stable,
                cache: true,
            },
            SystemPromptPart {
                content: dynamic,
                cache: false,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};
    use crate::thinker::prompt_mode::PromptMode;

    fn total_len(parts: &[SystemPromptPart]) -> usize {
        parts.iter().map(|p| p.content.len()).sum()
    }

    #[test]
    fn full_is_the_back_compat_default() {
        let builder = PromptBuilder::new(PromptConfig::default());
        let legacy = builder.build_system_prompt_cached(&[]);
        let full = builder.build_system_prompt_cached_with_mode(&[], PromptMode::Full);
        // The no-mode entry point must reproduce Full byte-for-byte.
        assert_eq!(legacy.len(), full.len());
        for (a, b) in legacy.iter().zip(full.iter()) {
            assert_eq!(a.content, b.content);
            assert_eq!(a.cache, b.cache);
        }
    }

    #[test]
    fn minimal_sheds_heavy_layers_on_cached_path() {
        let builder = PromptBuilder::new(PromptConfig::default());
        let full = builder.build_system_prompt_cached_with_mode(&[], PromptMode::Full);
        let minimal = builder.build_system_prompt_cached_with_mode(&[], PromptMode::Minimal);
        // `MemoryProtocolLayer` participates in the Cached path and declares
        // `supports_mode(Minimal) == false`, so Minimal must be strictly
        // leaner than Full even on the default config.
        assert!(
            total_len(&full) > total_len(&minimal),
            "expected Minimal prompt ({}) to be leaner than Full ({})",
            total_len(&minimal),
            total_len(&full),
        );
        // The stable/dynamic split (prompt-cache breakpoint) is preserved.
        assert_eq!(minimal.len(), 2);
        assert!(minimal[0].cache && !minimal[1].cache);
    }
}
