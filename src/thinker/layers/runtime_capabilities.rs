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
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        if let Some(ref runtimes) = input.config.runtime_capabilities {
            let runtimes = sanitize_for_prompt(runtimes, SanitizeLevel::Light);
            output.push_str("## Available Runtimes\n\n");
            output.push_str("You can execute code using these installed runtimes:\n\n");
            output.push_str(&runtimes);
            output.push_str(
                "\n**IMPORTANT**: Runtimes are NOT tools — they're execution environments. \
                 To run Python/Node code, write a script with `file_ops`, then run it with \
                 `bash`. Don't call runtime names (uv, fnm, ffmpeg, yt-dlp) as tools.\n",
            );
            output.push_str(
                "\n**CRITICAL - Use Aleph Runtimes**: always invoke the full \"Executable\" path \
                 shown above (e.g. `/path/to/python script.py`), never bare `python3` / `python` \
                 — the system default may be missing or incompatible. Aleph's managed runtimes \
                 guarantee the right versions and dependencies.\n\n",
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
        assert!(out.contains("CRITICAL - Use Aleph Runtimes"));
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
