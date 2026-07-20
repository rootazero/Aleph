//! `OperationalGuidelinesLayer` — system operational awareness (priority 800)

use crate::thinker::interaction::InteractionParadigm;
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct OperationalGuidelinesLayer;

impl PromptLayer for OperationalGuidelinesLayer {
    fn name(&self) -> &'static str {
        "operational_guidelines"
    }
    fn priority(&self) -> u32 {
        800
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        // Ride every non-minimal path; `ResolvedContext` is threaded on the
        // Basic / Cached production routes and the inject() guard keeps output
        // empty when it is absent (capture / snapshot).
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Cached,
        ]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        let ctx = match input.context {
            Some(c) => c,
            None => return,
        };

        let paradigm = ctx.environment_contract.paradigm;
        match paradigm {
            InteractionParadigm::Background | InteractionParadigm::CLI => {}
            _ => return, // Skip for Messaging, WebRich, Embedded
        }

        output.push_str("## System Operational Awareness\n\n");
        output.push_str(
            "You may monitor your own runtime (disk, memory, processes, config, database, \
             desktop-capability availability) with read-only diagnostics and report anomalies \
             to the user. Never autonomously restart services, delete/compact databases, kill \
             processes, or change system settings.\n\n",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_operational_guidelines_no_context() {
        let layer = OperationalGuidelinesLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn test_operational_guidelines_paths() {
        let paths = OperationalGuidelinesLayer.paths();
        assert!(paths.contains(&AssemblyPath::Basic));
        assert!(paths.contains(&AssemblyPath::Soul));
        assert!(paths.contains(&AssemblyPath::Hydration));
        assert!(paths.contains(&AssemblyPath::Cached));
    }

    #[test]
    fn graceful_noop_on_basic_path_without_context() {
        let layer = OperationalGuidelinesLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }
}
