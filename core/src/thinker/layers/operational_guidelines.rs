//! OperationalGuidelinesLayer — system operational awareness (priority 800)

use crate::thinker::interaction::InteractionParadigm;
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct OperationalGuidelinesLayer;

impl PromptLayer for OperationalGuidelinesLayer {
    fn name(&self) -> &'static str { "operational_guidelines" }
    fn priority(&self) -> u32 { 800 }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Context]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        let ctx = match input.context {
            Some(c) => c,
            None => return,
        };

        let paradigm = ctx.environment_contract.paradigm;
        match paradigm {
            InteractionParadigm::Background
            | InteractionParadigm::CLI => {}
            _ => return, // Skip for Messaging, WebRich, Embedded
        }

        output.push_str("## System Operational Awareness\n\n");
        output.push_str(
            "You are aware of your own runtime environment and can monitor it proactively.\n\n",
        );

        output.push_str("### Diagnostic Capabilities (read-only, always allowed)\n");
        output.push_str("- Check disk space: `df -h`\n");
        output.push_str("- Check memory usage: `vm_stat` / `free -h`\n");
        output.push_str("- Check running Aleph processes: `ps aux | grep aleph`\n");
        output.push_str(
            "- Check configuration validity: read config files and validate structure\n",
        );
        output.push_str("- Check Desktop Bridge status: query UDS socket availability\n");
        output.push_str("- Check LanceDB health: verify database file accessibility\n\n");

        output.push_str("### When You Detect Issues\n");
        output.push_str(
            "If you notice configuration conflicts, database issues, disconnected bridges,\n",
        );
        output.push_str("abnormal resource usage, or runtime capability degradation:\n\n");
        output.push_str("**Action**: Report to the user with:\n");
        output.push_str("1. What you observed (specific evidence)\n");
        output.push_str("2. Potential impact\n");
        output.push_str("3. Suggested remediation steps\n");
        output.push_str("4. Do NOT execute remediation without explicit user approval\n\n");

        output.push_str("### What You Must NEVER Do Autonomously\n");
        output.push_str("- Restart Aleph services\n");
        output.push_str("- Modify configuration files\n");
        output.push_str("- Delete or compact databases\n");
        output.push_str("- Kill processes\n");
        output.push_str("- Change system settings\n\n");
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
        assert_eq!(paths.len(), 1);
        assert!(paths.contains(&AssemblyPath::Context));
    }
}
