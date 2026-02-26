//! RuntimeCapabilitiesLayer — available runtime environments (priority 400)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_sanitizer::{sanitize_for_prompt, SanitizeLevel};

pub struct RuntimeCapabilitiesLayer;

impl PromptLayer for RuntimeCapabilitiesLayer {
    fn name(&self) -> &'static str { "runtime_capabilities" }
    fn priority(&self) -> u32 { 400 }
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
            output.push_str("\n**IMPORTANT**: Runtimes are NOT tools. They describe execution environments.\n");
            output.push_str("- To execute Python code, use the `file_ops` tool to write a .py script, then use `bash` tool to run it\n");
            output.push_str("- To execute Node.js code, use the `file_ops` tool to write a .js script, then use `bash` tool to run it\n");
            output.push_str("- Do NOT try to call runtime names (uv, fnm, ffmpeg, yt-dlp) as tools directly\n");
            output.push_str("\n**CRITICAL - Use Aleph Runtimes**:\n");
            output.push_str("When executing Python/Node.js scripts, ALWAYS use the full executable path from the runtimes above:\n");
            output.push_str("- ✅ CORRECT: Use the exact \"Executable\" path shown in the runtime info\n");
            output.push_str("- ✅ Example: If runtime shows \"Executable: /path/to/python\", use \"/path/to/python script.py\"\n");
            output.push_str("- ❌ WRONG: `python3 script.py` (system default may be incompatible)\n");
            output.push_str("- ❌ WRONG: `python script.py` (may not exist)\n");
            output.push_str("Aleph provides managed runtimes to ensure correct versions and dependencies.\n\n");
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
