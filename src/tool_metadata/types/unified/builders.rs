//! Builder methods for `UnifiedTool`
//!
//! Fluent builder API for constructing `UnifiedTool` instances with optional fields.

use super::{ChannelType, UnifiedTool};
use crate::tool_metadata::types::safety::ToolSafetyLevel;
use serde_json::Value;

impl UnifiedTool {
    // =========================================================================
    // Basic Builder Methods
    // =========================================================================

    /// Builder method: set display name
    pub fn with_display_name(mut self, display_name: impl Into<String>) -> Self {
        self.display_name = display_name.into();
        self
    }

    /// Builder method: set alternative invocation names (aliases).
    ///
    /// Replaces any existing aliases. Empty/whitespace-only entries are dropped
    /// and duplicates of the canonical name are ignored so resolution stays
    /// unambiguous.
    pub fn with_aliases<I, S>(mut self, aliases: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let canonical = self.name.to_lowercase();
        self.aliases = aliases
            .into_iter()
            .map(|a| a.into())
            .filter(|a| {
                let t = a.trim();
                !t.is_empty() && t.to_lowercase() != canonical
            })
            .collect();
        self
    }

    /// Builder method: set parameters schema
    #[must_use]
    pub fn with_parameters_schema(mut self, schema: Value) -> Self {
        self.parameters_schema = Some(schema);
        self
    }

    /// Builder method: set requires confirmation
    #[must_use]
    pub const fn with_requires_confirmation(mut self, requires: bool) -> Self {
        self.requires_confirmation = requires;
        self
    }

    /// Builder method: set safety level
    #[must_use]
    pub const fn with_safety_level(mut self, level: ToolSafetyLevel) -> Self {
        self.safety_level = level;
        self
    }

    // =========================================================================
    // UI Metadata Builder Methods
    // =========================================================================

    /// Builder method: set icon (SF Symbol name)
    pub fn with_icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Builder method: set usage example
    pub fn with_usage(mut self, usage: impl Into<String>) -> Self {
        self.usage = Some(usage.into());
        self
    }

    /// Builder method: set parameter hint for UI display
    #[must_use]
    pub fn with_param_hint(mut self, hint: &str) -> Self {
        self.param_hint = Some(hint.to_string());
        self
    }

    /// Builder method: set localization key
    pub fn with_localization_key(mut self, key: impl Into<String>) -> Self {
        self.localization_key = Some(key.into());
        self
    }

    /// Builder method: set sort order
    #[must_use]
    pub const fn with_sort_order(mut self, order: i32) -> Self {
        self.sort_order = order;
        self
    }

    // =========================================================================
    // Routing Config Builder Methods (for builtin commands)
    // =========================================================================

    /// Builder method: set routing regex pattern
    pub fn with_routing_regex(mut self, regex: impl Into<String>) -> Self {
        self.routing_regex = Some(regex.into());
        self
    }

    /// Builder method: set routing system prompt
    pub fn with_routing_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.routing_system_prompt = Some(prompt.into());
        self
    }

    /// Builder method: set routing capabilities
    #[must_use]
    pub fn with_routing_capabilities(mut self, caps: Vec<String>) -> Self {
        self.routing_capabilities = caps;
        self
    }

    /// Builder method: set routing intent type
    pub fn with_routing_intent_type(mut self, intent: impl Into<String>) -> Self {
        self.routing_intent_type = Some(intent.into());
        self
    }

    /// Builder method: set routing strip prefix
    #[must_use]
    pub const fn with_routing_strip_prefix(mut self, strip: bool) -> Self {
        self.routing_strip_prefix = strip;
        self
    }

    // =========================================================================
    // Visibility Builder Methods
    // =========================================================================

    /// Builder method: set visible channels
    #[must_use]
    pub fn with_visible_channels(mut self, channels: Vec<ChannelType>) -> Self {
        self.visible_channels = channels;
        self
    }
}
