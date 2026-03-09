//! InboundContextLayer — per-request dynamic context injection (priority 1700)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct InboundContextLayer;

impl PromptLayer for InboundContextLayer {
    fn name(&self) -> &'static str { "inbound_context" }
    fn priority(&self) -> u32 { 1700 }
    fn stability(&self) -> LayerStability { LayerStability::Dynamic }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Soul, AssemblyPath::Context, AssemblyPath::Cached]
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full | PromptMode::Compact)
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        let inbound = match input.inbound {
            Some(ctx) => ctx,
            None => return,
        };
        output.push_str("## Inbound Context\n");
        output.push_str(&inbound.format_for_prompt());
        output.push_str("\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::inbound_context::{InboundContext, SenderInfo, ChannelContext, SessionContext};
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn metadata() {
        let layer = InboundContextLayer;
        assert_eq!(layer.name(), "inbound_context");
        assert_eq!(layer.priority(), 1700);
    }

    #[test]
    fn paths_include_soul_context_cached() {
        let paths = InboundContextLayer.paths();
        assert!(paths.contains(&AssemblyPath::Soul));
        assert!(paths.contains(&AssemblyPath::Context));
        assert!(paths.contains(&AssemblyPath::Cached));
        assert!(!paths.contains(&AssemblyPath::Basic));
        assert!(!paths.contains(&AssemblyPath::Hydration));
    }

    #[test]
    fn supports_full_and_compact_only() {
        let layer = InboundContextLayer;
        assert!(layer.supports_mode(PromptMode::Full));
        assert!(layer.supports_mode(PromptMode::Compact));
        assert!(!layer.supports_mode(PromptMode::Minimal));
    }

    #[test]
    fn injects_when_inbound_present() {
        let layer = InboundContextLayer;
        let config = PromptConfig::default();
        let inbound = InboundContext {
            sender: SenderInfo {
                id: "u42".to_string(),
                display_name: Some("Alice".to_string()),
                is_owner: true,
            },
            channel: ChannelContext {
                kind: "telegram".to_string(),
                capabilities: vec![],
                is_group_chat: false,
                is_mentioned: false,
            },
            session: SessionContext {
                session_key: "tg:dm:42".to_string(),
                active_agent: None,
            },
            ..Default::default()
        };

        let input = LayerInput::basic(&config, &[]).with_inbound(&inbound);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.starts_with("## Inbound Context\n"));
        assert!(out.contains("Sender: Alice (owner)"));
        assert!(out.contains("Channel: telegram"));
        assert!(out.ends_with("\n\n"));
    }

    #[test]
    fn skips_when_inbound_absent() {
        let layer = InboundContextLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }
}
