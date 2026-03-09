//! Cached prompt building for Anthropic prompt caching optimization
//!
//! Leverages the [`LayerStability`] classification to partition the
//! system prompt into a stable prefix (cacheable) and a dynamic suffix
//! that changes per request.

use crate::agent_loop::ToolInfo;

use super::{PromptBuilder, SystemPromptPart};
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput};

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
    pub fn build_system_prompt_cached(&self, tools: &[ToolInfo]) -> Vec<SystemPromptPart> {
        let input = LayerInput::basic(&self.config, tools);
        let stable = self.pipeline.execute_stable_only(AssemblyPath::Cached, &input);
        let dynamic = self.pipeline.execute_dynamic_only(AssemblyPath::Cached, &input);

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
