//! PromptLayer trait — the composable unit of prompt assembly

use crate::agent_loop::ToolInfo;
use crate::dispatcher::tool_index::HydrationResult;
use crate::poe::PoePromptContext;
use super::context::ResolvedContext;
use super::prompt_builder::PromptConfig;
use super::prompt_mode::PromptMode;
use super::soul::SoulManifest;

/// Which assembly path a layer participates in.
///
/// The pipeline filters layers by the active path so that only
/// relevant sections are injected into the final system prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssemblyPath {
    /// Minimal prompt — tools only, no hydration / soul / context.
    Basic,
    /// Hydration-based prompt — tools come from semantic retrieval.
    Hydration,
    /// Soul-enriched prompt — includes identity / personality.
    Soul,
    /// Context-aware prompt — includes environment / security context.
    Context,
    /// Pre-cached prompt — used when prompt caching is active.
    Cached,
}

/// Everything a layer might need to produce its output.
///
/// Each constructor pre-fills only the fields relevant to a given
/// assembly path; the rest stay `None`.
pub struct LayerInput<'a> {
    pub config: &'a PromptConfig,
    pub tools: Option<&'a [ToolInfo]>,
    pub hydration: Option<&'a HydrationResult>,
    pub soul: Option<&'a SoulManifest>,
    pub context: Option<&'a ResolvedContext>,
    /// POE context (success criteria, behavioral anchors, hints)
    pub poe: Option<&'a PoePromptContext>,
    /// Active workspace profile (system_prompt overlay, tool whitelist, etc.)
    pub profile: Option<&'a crate::config::ProfileConfig>,
}

impl<'a> LayerInput<'a> {
    /// Input for the `Basic` path — config + tool list.
    pub fn basic(config: &'a PromptConfig, tools: &'a [ToolInfo]) -> Self {
        Self { config, tools: Some(tools), hydration: None, soul: None, context: None, poe: None, profile: None }
    }

    /// Input for the `Hydration` path — config + hydration result.
    pub fn hydration(config: &'a PromptConfig, hydration: &'a HydrationResult) -> Self {
        Self { config, tools: None, hydration: Some(hydration), soul: None, context: None, poe: None, profile: None }
    }

    /// Input for the `Soul` path — config + tools + soul manifest.
    pub fn soul(config: &'a PromptConfig, tools: &'a [ToolInfo], soul: &'a SoulManifest) -> Self {
        Self { config, tools: Some(tools), hydration: None, soul: Some(soul), context: None, poe: None, profile: None }
    }

    /// Input for the `Context` path — config + resolved context.
    pub fn context(config: &'a PromptConfig, ctx: &'a ResolvedContext) -> Self {
        Self { config, tools: None, hydration: None, soul: None, context: Some(ctx), poe: None, profile: None }
    }

    /// Attach POE context to this input.
    pub fn with_poe(mut self, poe: &'a PoePromptContext) -> Self {
        self.poe = Some(poe);
        self
    }

    /// Attach workspace profile to this input.
    pub fn with_profile(mut self, profile: Option<&'a crate::config::ProfileConfig>) -> Self {
        self.profile = profile;
        self
    }

    /// Get POE manifest if present.
    pub fn poe_manifest(&self) -> Option<&crate::poe::types::SuccessManifest> {
        self.poe.and_then(|p| p.manifest.as_ref())
    }

    /// Get POE hint if present.
    pub fn poe_hint(&self) -> Option<&str> {
        self.poe.and_then(|p| p.current_hint.as_deref())
    }
}

/// A composable unit of prompt assembly.
///
/// Each layer appends its contribution to the system prompt string.
/// Layers declare which assembly paths they participate in and a
/// numeric priority that controls ordering (lower = earlier).
pub trait PromptLayer: Send + Sync {
    /// Human-readable name for debugging / logging.
    fn name(&self) -> &'static str;

    /// Sort key — layers are executed in ascending priority order.
    fn priority(&self) -> u32;

    /// Which assembly paths this layer participates in.
    fn paths(&self) -> &'static [AssemblyPath];

    /// Append this layer's content to `output`.
    fn inject(&self, output: &mut String, input: &LayerInput);

    /// Whether this layer participates in the given prompt mode.
    /// Default: true (all modes). Override to exclude from Compact/Minimal.
    fn supports_mode(&self, _mode: PromptMode) -> bool {
        true
    }
}
