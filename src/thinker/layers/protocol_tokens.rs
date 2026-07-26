//! `ProtocolTokensLayer` — structured LLM-to-system protocol tokens (priority 700)

use crate::thinker::interaction::Capability;
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct ProtocolTokensLayer;

impl PromptLayer for ProtocolTokensLayer {
    fn name(&self) -> &'static str {
        "protocol_tokens"
    }
    fn priority(&self) -> u32 {
        700
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        // Ride every non-minimal path; `ResolvedContext` is threaded on the
        // Basic / Cached production routes and the inject() guard keeps output
        // empty when it (or the `SilentReply` capability) is absent.
        &[AssemblyPath::Basic, AssemblyPath::Cached]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        let ctx = match input.context {
            Some(c) => c,
            None => return,
        };

        if !ctx
            .environment_contract
            .active_capabilities
            .contains(&Capability::SilentReply)
        {
            return;
        }

        output.push_str(&crate::thinker::protocol_tokens::ProtocolToken::to_prompt_section());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_protocol_tokens_no_context() {
        let layer = ProtocolTokensLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn test_protocol_tokens_paths() {
        let paths = ProtocolTokensLayer.paths();
        assert!(paths.contains(&AssemblyPath::Basic));
        assert!(paths.contains(&AssemblyPath::Cached));
    }

    #[test]
    fn graceful_noop_on_basic_path_without_context() {
        let layer = ProtocolTokensLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }
}
