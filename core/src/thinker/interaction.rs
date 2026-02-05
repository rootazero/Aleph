//! Interaction types for Channel Capability Awareness
//!
//! This module defines types that describe what a channel can technically do,
//! allowing the AI to adapt its behavior based on the current interaction
//! environment (CLI, Web, Telegram, etc.).
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                  InteractionManifest                        │
//! │  ┌─────────────────┐  ┌──────────────┐  ┌───────────────┐  │
//! │  │ InteractionPara │  │ Capabilities │  │  Constraints  │  │
//! │  │     digm        │  │  (HashSet)   │  │               │  │
//! │  │                 │  │              │  │ max_output    │  │
//! │  │ • CLI           │  │ • RichText   │  │ streaming     │  │
//! │  │ • WebRich       │  │ • Streaming  │  │ prefer_compact│  │
//! │  │ • Messaging     │  │ • Canvas     │  │               │  │
//! │  │ • Background    │  │ • ...        │  │               │  │
//! │  │ • Embedded      │  │              │  │               │  │
//! │  └─────────────────┘  └──────────────┘  └───────────────┘  │
//! └─────────────────────────────────────────────────────────────┘
//! ```

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// Interaction paradigm representing the type of channel environment
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionParadigm {
    /// Command-line interface with terminal output
    CLI,
    /// Web-based rich interface with full capabilities
    WebRich,
    /// Messaging platforms (Telegram, Discord, iMessage)
    Messaging,
    /// Background processing with no direct user interaction
    Background,
    /// Embedded contexts with minimal UI capabilities
    Embedded,
}

impl InteractionParadigm {
    /// Returns a human-readable description for use in prompts
    pub fn description(&self) -> &'static str {
        match self {
            Self::CLI => "Command-line interface with terminal output supporting ANSI formatting",
            Self::WebRich => "Web-based rich interface with full interactive capabilities",
            Self::Messaging => "Messaging platform with limited formatting and inline media",
            Self::Background => "Background processing with no direct user interaction",
            Self::Embedded => "Embedded context with minimal UI capabilities",
        }
    }

    /// Returns the default capabilities for this paradigm
    pub fn default_capabilities(&self) -> HashSet<Capability> {
        match self {
            Self::CLI => {
                [Capability::RichText, Capability::CodeHighlight, Capability::Streaming]
                    .into_iter()
                    .collect()
            }
            Self::WebRich => [
                Capability::RichText,
                Capability::CodeHighlight,
                Capability::MultiGroupUI,
                Capability::Streaming,
                Capability::MermaidCharts,
                Capability::ImageInline,
                Capability::Canvas,
            ]
            .into_iter()
            .collect(),
            Self::Messaging => {
                [Capability::RichText, Capability::ImageInline].into_iter().collect()
            }
            Self::Background => [Capability::SilentReply].into_iter().collect(),
            Self::Embedded => HashSet::new(),
        }
    }
}

/// Individual capabilities that a channel may support
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Rich text formatting (markdown, bold, italic, etc.)
    RichText,
    /// Interactive inline buttons
    InlineButtons,
    /// Multi-group UI layouts (tabs, accordions, etc.)
    MultiGroupUI,
    /// Streaming responses in real-time
    Streaming,
    /// Inline image display
    ImageInline,
    /// Mermaid diagram rendering
    MermaidCharts,
    /// Syntax-highlighted code blocks
    CodeHighlight,
    /// File upload capabilities
    FileUpload,
    /// Canvas for interactive drawing/editing
    Canvas,
    /// Silent replies (background processing, no user notification)
    SilentReply,
}

impl Capability {
    /// Returns a (name, hint) tuple for prompt generation
    pub fn prompt_hint(&self) -> (&'static str, &'static str) {
        match self {
            Self::RichText => ("rich_text", "You can use markdown formatting for emphasis"),
            Self::InlineButtons => {
                ("inline_buttons", "You can provide interactive button options")
            }
            Self::MultiGroupUI => (
                "multi_group_ui",
                "You can organize content into tabs or collapsible sections",
            ),
            Self::Streaming => ("streaming", "Responses will stream in real-time"),
            Self::ImageInline => ("image_inline", "Images can be displayed inline"),
            Self::MermaidCharts => ("mermaid_charts", "You can render Mermaid diagrams"),
            Self::CodeHighlight => ("code_highlight", "Code blocks will have syntax highlighting"),
            Self::FileUpload => ("file_upload", "Files can be uploaded and attached"),
            Self::Canvas => ("canvas", "Interactive canvas is available for drawing and editing"),
            Self::SilentReply => {
                ("silent_reply", "Responses will be processed silently in background")
            }
        }
    }
}

/// Constraints on interaction behavior
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InteractionConstraints {
    /// Maximum output characters (None means unlimited)
    pub max_output_chars: Option<usize>,
    /// Whether the channel supports streaming
    pub supports_streaming: bool,
    /// Whether compact output is preferred
    pub prefer_compact: bool,
}

impl InteractionConstraints {
    /// Create new constraints with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Set maximum output characters
    pub fn max_output_chars(mut self, limit: usize) -> Self {
        self.max_output_chars = Some(limit);
        self
    }

    /// Set streaming support
    pub fn supports_streaming(mut self, supports: bool) -> Self {
        self.supports_streaming = supports;
        self
    }

    /// Set compact preference
    pub fn prefer_compact(mut self, prefer: bool) -> Self {
        self.prefer_compact = prefer;
        self
    }
}

/// Complete manifest describing a channel's interaction capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionManifest {
    /// The interaction paradigm
    pub paradigm: InteractionParadigm,
    /// Set of supported capabilities
    pub capabilities: HashSet<Capability>,
    /// Interaction constraints
    pub constraints: InteractionConstraints,
}

impl InteractionManifest {
    /// Create a new manifest with paradigm defaults
    pub fn new(paradigm: InteractionParadigm) -> Self {
        Self {
            capabilities: paradigm.default_capabilities(),
            paradigm,
            constraints: InteractionConstraints::default(),
        }
    }

    /// Override capabilities with an explicit set
    pub fn with_capabilities(mut self, capabilities: HashSet<Capability>) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Add a single capability
    pub fn add_capability(&mut self, capability: Capability) {
        self.capabilities.insert(capability);
    }

    /// Remove a single capability
    pub fn remove_capability(&mut self, capability: &Capability) {
        self.capabilities.remove(capability);
    }

    /// Check if a capability is supported
    pub fn has_capability(&self, capability: &Capability) -> bool {
        self.capabilities.contains(capability)
    }

    /// Set constraints
    pub fn with_constraints(mut self, constraints: InteractionConstraints) -> Self {
        self.constraints = constraints;
        self
    }

    /// Check if a tool is supported based on capabilities
    ///
    /// Some tools require specific capabilities:
    /// - Canvas tool requires Canvas capability
    pub fn supports_tool(&self, tool_name: &str) -> bool {
        match tool_name {
            "canvas" => self.has_capability(&Capability::Canvas),
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_paradigm_defaults() {
        let manifest = InteractionManifest::new(InteractionParadigm::CLI);

        assert!(manifest.has_capability(&Capability::Streaming));
        assert!(manifest.has_capability(&Capability::RichText));
        assert!(!manifest.has_capability(&Capability::MultiGroupUI));
    }

    #[test]
    fn test_web_paradigm_defaults() {
        let manifest = InteractionManifest::new(InteractionParadigm::WebRich);

        assert!(manifest.has_capability(&Capability::MultiGroupUI));
        assert!(manifest.has_capability(&Capability::MermaidCharts));
        assert!(manifest.has_capability(&Capability::Canvas));
    }

    #[test]
    fn test_capability_override() {
        let mut manifest = InteractionManifest::new(InteractionParadigm::Messaging);

        // Messaging doesn't have InlineButtons by default
        assert!(!manifest.has_capability(&Capability::InlineButtons));

        // Add InlineButtons capability
        manifest.add_capability(Capability::InlineButtons);

        assert!(manifest.has_capability(&Capability::InlineButtons));
    }

    #[test]
    fn test_constraints() {
        let constraints = InteractionConstraints::new()
            .max_output_chars(4096)
            .supports_streaming(true)
            .prefer_compact(false);

        let manifest =
            InteractionManifest::new(InteractionParadigm::Messaging).with_constraints(constraints);

        assert_eq!(manifest.constraints.max_output_chars, Some(4096));
        assert!(manifest.constraints.supports_streaming);
        assert!(!manifest.constraints.prefer_compact);
    }

    #[test]
    fn test_supports_tool_canvas() {
        let web_manifest = InteractionManifest::new(InteractionParadigm::WebRich);
        let cli_manifest = InteractionManifest::new(InteractionParadigm::CLI);

        // WebRich has Canvas capability
        assert!(web_manifest.supports_tool("canvas"));

        // CLI doesn't have Canvas capability
        assert!(!cli_manifest.supports_tool("canvas"));

        // Other tools are always supported
        assert!(cli_manifest.supports_tool("web_search"));
        assert!(web_manifest.supports_tool("read_file"));
    }

    #[test]
    fn test_paradigm_description() {
        assert!(InteractionParadigm::CLI.description().contains("Command-line"));
        assert!(InteractionParadigm::WebRich.description().contains("Web"));
        assert!(InteractionParadigm::Background.description().contains("Background"));
    }

    #[test]
    fn test_capability_prompt_hint() {
        let (name, hint) = Capability::RichText.prompt_hint();
        assert_eq!(name, "rich_text");
        assert!(hint.contains("markdown"));

        let (name, hint) = Capability::Canvas.prompt_hint();
        assert_eq!(name, "canvas");
        assert!(hint.contains("canvas"));
    }

    #[test]
    fn test_remove_capability() {
        let mut manifest = InteractionManifest::new(InteractionParadigm::WebRich);

        assert!(manifest.has_capability(&Capability::Canvas));

        manifest.remove_capability(&Capability::Canvas);

        assert!(!manifest.has_capability(&Capability::Canvas));
    }

    #[test]
    fn test_with_capabilities_override() {
        let custom_caps: HashSet<Capability> =
            [Capability::RichText, Capability::FileUpload].into_iter().collect();

        let manifest =
            InteractionManifest::new(InteractionParadigm::CLI).with_capabilities(custom_caps);

        // Should only have the overridden capabilities
        assert!(manifest.has_capability(&Capability::RichText));
        assert!(manifest.has_capability(&Capability::FileUpload));
        assert!(!manifest.has_capability(&Capability::Streaming)); // Not in override set
    }

    #[test]
    fn test_serde_roundtrip() {
        let manifest = InteractionManifest::new(InteractionParadigm::WebRich);

        let json = serde_json::to_string(&manifest).expect("serialize");
        let deserialized: InteractionManifest =
            serde_json::from_str(&json).expect("deserialize");

        assert_eq!(deserialized.paradigm, InteractionParadigm::WebRich);
        assert!(deserialized.has_capability(&Capability::Canvas));
    }
}
