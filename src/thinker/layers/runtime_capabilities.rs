//! `RuntimeCapabilitiesLayer` — available runtime environments (priority 400)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;
use crate::thinker::prompt_sanitizer::{sanitize_for_prompt, SanitizeLevel};

pub struct RuntimeCapabilitiesLayer;

impl PromptLayer for RuntimeCapabilitiesLayer {
    fn name(&self) -> &'static str {
        "runtime_capabilities"
    }
    fn priority(&self) -> u32 {
        400
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Cached]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        if let Some(ref runtimes) = input.config.runtime_capabilities {
            let runtimes = sanitize_for_prompt(runtimes, SanitizeLevel::Light);
            output.push_str("## Available Runtimes\n\n");
            output.push_str("You can run code with these installed, pre-verified runtimes:\n\n");
            output.push_str(&runtimes);
            // The LIST is the runtime fact. Keep only the non-obvious, Aleph-
            // specific usage note (invoke the managed Executable path, not a
            // bare `python3`); the "runtimes aren't tools" / no-probing lecture
            // was cut as inferable how-to (§1.1 prune-the-prompt).
            output.push_str(
                "\nInvoke the full \"Executable\" path shown above (not a bare `python3` / \
                 `node`) — these are Aleph's managed runtimes; the system default may be missing \
                 or incompatible, and a runtime absent from this list isn't installed.\n\n",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_runtime_capabilities_present() {
        let layer = RuntimeCapabilitiesLayer;
        let config = PromptConfig {
            runtime_capabilities: Some("- Python 3.11\n- Node.js 20".to_string()),
            ..Default::default()
        };
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## Available Runtimes"));
        assert!(out.contains("Python 3.11"));
        // The runtime LIST + the non-obvious full-path usage note survive; the
        // "runtimes aren't tools" / no-probing lecture is gone.
        assert!(out.contains("Executable"));
        assert!(!out.contains("Runtimes are NOT tools"));
    }

    #[test]
    fn test_runtime_capabilities_absent() {
        let layer = RuntimeCapabilitiesLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }
}
