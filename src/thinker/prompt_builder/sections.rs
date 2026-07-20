//! Prompt section builders (append_* methods)
//!
//! Test-only section builders retained as `#[cfg(test)]` twins of the live
//! `PromptLayer`s (RuntimeCapabilities / GenerationModels / SkillInstructions /
//! CustomInstructions / Language). Production prompt assembly runs entirely
//! through the layer pipeline (`PromptPipeline::execute`), never these helpers;
//! they exist only to unit-test the shared `sanitize_for_prompt` path.

use super::PromptBuilder;
#[cfg(test)]
use crate::thinker::prompt_sanitizer::{sanitize_for_prompt, SanitizeLevel};

impl PromptBuilder {
    /// Append runtime capabilities section (test-only; pipeline uses RuntimeCapabilitiesLayer)
    #[cfg(test)]
    pub(crate) fn append_runtime_capabilities(&self, prompt: &mut String) {
        if let Some(ref runtimes) = self.config.runtime_capabilities {
            let runtimes = sanitize_for_prompt(runtimes, SanitizeLevel::Light);
            prompt.push_str("## Available Runtimes\n\n");
            prompt.push_str("You can execute code using these installed runtimes:\n\n");
            prompt.push_str(&runtimes);
            prompt.push_str(
                "\n**IMPORTANT**: Runtimes are NOT tools. They describe execution environments.\n",
            );
            prompt.push_str("- To execute Python code, use the `file_ops` tool to write a .py script, then use `bash` tool to run it\n");
            prompt.push_str("- To execute Node.js code, use the `file_ops` tool to write a .js script, then use `bash` tool to run it\n");
            prompt.push_str(
                "- Do NOT try to call runtime names (uv, fnm, ffmpeg, yt-dlp) as tools directly\n",
            );
            prompt.push_str("\n**CRITICAL - Use Aleph Runtimes**:\n");
            prompt.push_str("When executing Python/Node.js scripts, ALWAYS use the full executable path from the runtimes above:\n");
            prompt.push_str(
                "- ✅ CORRECT: Use the exact \"Executable\" path shown in the runtime info\n",
            );
            prompt.push_str("- ✅ Example: If runtime shows \"Executable: /path/to/python\", use \"/path/to/python script.py\"\n");
            prompt
                .push_str("- ❌ WRONG: `python3 script.py` (system default may be incompatible)\n");
            prompt.push_str("- ❌ WRONG: `python script.py` (may not exist)\n");
            prompt.push_str(
                "Aleph provides managed runtimes to ensure correct versions and dependencies.\n\n",
            );
        }
    }

    /// Append generation models section (test-only; pipeline uses GenerationModelsLayer)
    #[cfg(test)]
    pub(crate) fn append_generation_models(&self, prompt: &mut String) {
        if let Some(ref models) = self.config.generation_models {
            let models = sanitize_for_prompt(models, SanitizeLevel::Light);
            prompt.push_str("## Media Generation Models\n\n");
            prompt.push_str(&models);
            prompt.push('\n');
        }
    }

    /// Append skill instructions from SkillSystem v2 snapshot (test-only; pipeline uses SkillInstructionsLayer)
    #[cfg(test)]
    pub(crate) fn append_skill_instructions(&self, prompt: &mut String) {
        if let Some(ref instructions) = self.config.skill_instructions {
            if !instructions.is_empty() {
                let instructions = sanitize_for_prompt(instructions, SanitizeLevel::Moderate);
                let instructions = sanitize_for_prompt(&instructions, SanitizeLevel::Light);
                prompt.push_str("## Available Skills\n\n");
                prompt.push_str("You can invoke skills using the `skill` tool. ");
                prompt.push_str("Skills provide specialized instructions for specific tasks.\n\n");
                prompt.push_str(&instructions);
                prompt.push_str("\n\n");
            }
        }
    }

    /// Append custom instructions section (test-only; pipeline uses CustomInstructionsLayer)
    #[cfg(test)]
    pub(crate) fn append_custom_instructions(&self, prompt: &mut String) {
        if let Some(instructions) = &self.config.custom_instructions {
            let instructions = sanitize_for_prompt(instructions, SanitizeLevel::Moderate);
            let instructions = sanitize_for_prompt(&instructions, SanitizeLevel::Light);
            prompt.push_str("## Additional Instructions\n");
            prompt.push_str(&instructions);
            prompt.push_str("\n\n");
        }
    }

    /// Append language setting section (test-only; pipeline uses LanguageLayer)
    #[cfg(test)]
    pub(crate) fn append_language_setting(&self, prompt: &mut String) {
        if let Some(lang) = &self.config.language {
            let lang = sanitize_for_prompt(lang, SanitizeLevel::Strict);
            let language_name = match lang.as_str() {
                "zh-Hans" => "Chinese (Simplified)",
                "zh-Hant" => "Chinese (Traditional)",
                "en" => "English",
                "ja" => "Japanese",
                "ko" => "Korean",
                "de" => "German",
                "fr" => "French",
                "es" => "Spanish",
                "it" => "Italian",
                "pt" => "Portuguese",
                "ru" => "Russian",
                _ => lang.as_str(),
            };
            prompt.push_str("## Response Language\n");
            prompt.push_str(&format!(
                "Respond in {} by default. Exception: If the task explicitly requires a different language \
                (e.g., translation, writing in a specific language), use the requested language instead.\n\n",
                language_name
            ));
        }
    }
}
