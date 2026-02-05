//! Prompt builder for Agent Loop
//!
//! This module builds prompts for the LLM thinking step,
//! including system prompts and message history.

use crate::agent_loop::{LoopState, Observation, StepSummary, ToolInfo};
use crate::core::MediaAttachment;
use crate::dispatcher::tool_index::HydrationResult;

use super::context::{DisableReason, DisabledTool, EnvironmentContract, ResolvedContext};
use super::interaction::Capability;
use super::soul::SoulManifest;

/// System prompt part with optional cache flag
///
/// When using Anthropic's prompt caching, static content can be cached
/// for improved performance. This struct allows splitting the system
/// prompt into cacheable and non-cacheable parts.
#[derive(Debug, Clone)]
pub struct SystemPromptPart {
    /// The content of this part
    pub content: String,
    /// Whether this part should be cached (for Anthropic)
    pub cache: bool,
}

/// Configuration for prompt building
#[derive(Debug, Clone)]
pub struct PromptConfig {
    /// Assistant persona/name
    pub persona: Option<String>,
    /// Response language
    pub language: Option<String>,
    /// Custom instructions to append
    pub custom_instructions: Option<String>,
    /// Maximum tokens for tool descriptions
    pub max_tool_description_tokens: usize,
    /// Runtime capabilities (pre-formatted prompt text)
    /// Describes available runtimes (Python, Node.js, FFmpeg, etc.)
    pub runtime_capabilities: Option<String>,
    /// Generation models (pre-formatted prompt text)
    /// Describes available image/video/audio generation models and aliases
    pub generation_models: Option<String>,
    /// Tool index for smart tool discovery (pre-formatted markdown)
    /// When set, enables two-stage tool discovery mode:
    /// - Tools passed to `build_system_prompt` get full schema
    /// - Additional tools are listed in this index (name + summary only)
    /// - LLM can call `get_tool_schema` to get full schema for indexed tools
    pub tool_index: Option<String>,
    /// Skill execution mode - when true, enforces strict workflow completion
    /// The agent MUST complete all steps specified in the skill instructions
    /// and generate all required output files before calling `complete`
    pub skill_mode: bool,
    /// Enable thinking transparency guidance
    /// When true, adds guidance for structured reasoning output
    /// (Observation -> Analysis -> Planning -> Decision)
    pub thinking_transparency: bool,
}

impl Default for PromptConfig {
    fn default() -> Self {
        Self {
            persona: None,
            language: None,
            custom_instructions: None,
            max_tool_description_tokens: 2000,
            runtime_capabilities: None,
            generation_models: None,
            tool_index: None,
            skill_mode: false,
            thinking_transparency: false,
        }
    }
}

/// Prompt builder for Agent Loop thinking
pub struct PromptBuilder {
    config: PromptConfig,
}

impl PromptBuilder {
    /// Create a new prompt builder
    pub fn new(config: PromptConfig) -> Self {
        Self { config }
    }

    /// Build the system prompt
    pub fn build_system_prompt(&self, tools: &[ToolInfo]) -> String {
        let mut prompt = String::new();

        // Role definition
        prompt.push_str("You are an AI assistant executing tasks step by step.\n\n");

        // Core instructions
        prompt.push_str("## Your Role\n");
        prompt.push_str("- Observe the current state and history\n");
        prompt.push_str("- Decide the SINGLE next action to take\n");
        prompt.push_str("- Execute until the task is complete or you need user input\n\n");

        // Build dynamic content using shared helpers
        self.append_runtime_capabilities(&mut prompt);
        self.append_tools(&mut prompt, tools);
        self.append_generation_models(&mut prompt);
        self.append_special_actions(&mut prompt);
        self.append_response_format(&mut prompt);
        self.append_guidelines(&mut prompt);
        self.append_thinking_guidance(&mut prompt);
        self.append_skill_mode(&mut prompt);
        self.append_custom_instructions(&mut prompt);
        self.append_language_setting(&mut prompt);

        prompt
    }

    // ========== Shared prompt section builders ==========

    /// Append runtime capabilities section
    fn append_runtime_capabilities(&self, prompt: &mut String) {
        if let Some(ref runtimes) = self.config.runtime_capabilities {
            prompt.push_str("## Available Runtimes\n\n");
            prompt.push_str("You can execute code using these installed runtimes:\n\n");
            prompt.push_str(runtimes);
            prompt.push_str("\n**IMPORTANT**: Runtimes are NOT tools. They describe execution environments.\n");
            prompt.push_str("- To execute Python code, use the `file_ops` tool to write a .py script, then use `bash` tool to run it\n");
            prompt.push_str("- To execute Node.js code, use the `file_ops` tool to write a .js script, then use `bash` tool to run it\n");
            prompt.push_str("- Do NOT try to call runtime names (uv, fnm, ffmpeg, yt-dlp) as tools directly\n");
            prompt.push_str("\n**CRITICAL - Use Aleph Runtimes**:\n");
            prompt.push_str("When executing Python/Node.js scripts, ALWAYS use the full executable path from the runtimes above:\n");
            prompt.push_str("- ✅ CORRECT: Use the exact \"Executable\" path shown in the runtime info\n");
            prompt.push_str("- ✅ Example: If runtime shows \"Executable: /path/to/python\", use \"/path/to/python script.py\"\n");
            prompt.push_str("- ❌ WRONG: `python3 script.py` (system default may be incompatible)\n");
            prompt.push_str("- ❌ WRONG: `python script.py` (may not exist)\n");
            prompt.push_str("Aleph provides managed runtimes to ensure correct versions and dependencies.\n\n");
        }
    }

    /// Append available tools section
    fn append_tools(&self, prompt: &mut String, tools: &[ToolInfo]) {
        prompt.push_str("## Available Tools\n");
        if tools.is_empty() && self.config.tool_index.is_none() {
            prompt.push_str("No tools available. You can only use special actions.\n\n");
        } else {
            if !tools.is_empty() {
                prompt.push_str("### Tools (with full parameters)\n");
                for tool in tools {
                    prompt.push_str(&format!("#### {}\n", tool.name));
                    prompt.push_str(&format!("{}\n", tool.description));
                    if !tool.parameters_schema.is_empty() {
                        prompt.push_str(&format!("Parameters: {}\n", tool.parameters_schema));
                    }
                    prompt.push('\n');
                }
            }

            if let Some(ref index) = self.config.tool_index {
                prompt.push_str("### Additional Tools (use `get_tool_schema` to get parameters)\n");
                prompt.push_str("The following tools are available but not shown with full parameters.\n");
                prompt.push_str(
                    "Call `get_tool_schema(tool_name)` to get the complete parameter schema before using.\n\n",
                );
                prompt.push_str(index);
                prompt.push('\n');
            }
        }
    }

    /// Append generation models section
    fn append_generation_models(&self, prompt: &mut String) {
        if let Some(ref models) = self.config.generation_models {
            prompt.push_str("## Media Generation Models\n\n");
            prompt.push_str(models);
            prompt.push('\n');
        }
    }

    /// Append hydrated tools from semantic retrieval
    ///
    /// Formats tools by hydration level:
    /// - Full schema tools: name + description + JSON parameters
    /// - Summary tools: name + description (use get_tool_schema for params)
    /// - Indexed tools: names list only
    ///
    /// This enables progressive disclosure of tool information based on
    /// semantic relevance to the user's query.
    pub fn append_hydrated_tools(&self, prompt: &mut String, result: &HydrationResult) {
        if result.is_empty() {
            prompt.push_str("## Available Tools\n");
            prompt.push_str("No semantically relevant tools found. Use `get_tool_schema` to discover tools.\n\n");
            return;
        }

        prompt.push_str("## Available Tools\n\n");

        // Full schema tools - highest relevance, include complete parameter info
        if !result.full_schema_tools.is_empty() {
            prompt.push_str("### Tools (full parameters)\n\n");
            for tool in &result.full_schema_tools {
                prompt.push_str(&format!("#### {}\n", tool.name));
                prompt.push_str(&format!("{}\n", tool.description));
                if let Some(schema) = tool.schema_json() {
                    prompt.push_str(&format!("Parameters: {}\n", schema));
                }
                prompt.push('\n');
            }
        }

        // Summary tools - medium relevance, description only
        if !result.summary_tools.is_empty() {
            prompt.push_str("### Tools (summary - call `get_tool_schema` for parameters)\n\n");
            for tool in &result.summary_tools {
                prompt.push_str(&format!("- **{}**: {}\n", tool.name, tool.description));
            }
            prompt.push('\n');
        }

        // Indexed tools - low relevance, just names
        if !result.indexed_tool_names.is_empty() {
            prompt.push_str("### Additional Tools (call `get_tool_schema` to use)\n\n");
            prompt.push_str(&result.indexed_tool_names.join(", "));
            prompt.push_str("\n\n");
        }
    }

    /// Build system prompt with hydrated tools from semantic retrieval
    ///
    /// This method builds a complete system prompt using HydrationResult
    /// instead of the traditional ToolInfo array, enabling semantic tool
    /// selection based on query relevance.
    pub fn build_system_prompt_with_hydration(&self, hydration: &HydrationResult) -> String {
        let mut prompt = String::new();

        // Role definition
        prompt.push_str("You are an AI assistant executing tasks step by step.\n\n");

        // Core instructions
        prompt.push_str("## Your Role\n");
        prompt.push_str("- Observe the current state and history\n");
        prompt.push_str("- Decide the SINGLE next action to take\n");
        prompt.push_str("- Execute until the task is complete or you need user input\n\n");

        // Runtime capabilities
        self.append_runtime_capabilities(&mut prompt);

        // Hydrated tools (semantic retrieval)
        self.append_hydrated_tools(&mut prompt, hydration);

        // Generation models
        self.append_generation_models(&mut prompt);

        // Special actions
        self.append_special_actions(&mut prompt);

        // Response format
        self.append_response_format(&mut prompt);

        // Guidelines
        self.append_guidelines(&mut prompt);

        // Thinking guidance
        self.append_thinking_guidance(&mut prompt);

        // Skill mode
        self.append_skill_mode(&mut prompt);

        // Custom instructions
        self.append_custom_instructions(&mut prompt);

        // Language setting
        self.append_language_setting(&mut prompt);

        prompt
    }

    /// Append special actions section
    fn append_special_actions(&self, prompt: &mut String) {
        prompt.push_str("## Special Actions\n");
        prompt.push_str("- `complete`: Call when the task is fully done. The `summary` field MUST be a comprehensive report that includes:\n");
        prompt.push_str("  1. A brief overview of what was accomplished\n");
        prompt.push_str("  2. Key results and findings (data, insights, metrics)\n");
        prompt.push_str("  3. List of all generated files with their purposes\n");
        prompt.push_str("  4. Any important notes or recommendations\n");
        prompt.push_str(
            "  **DO NOT** just say 'Task completed'. Write a detailed summary the user can immediately understand.\n",
        );
        prompt.push_str("- `ask_user`: Call when you need clarification or user decision\n");
        prompt.push_str("- `fail`: Call when the task cannot be completed\n\n");
    }

    /// Append response format section
    fn append_response_format(&self, prompt: &mut String) {
        prompt.push_str("## Response Format\n");
        prompt.push_str("You must respond with a JSON object:\n");
        prompt.push_str("```json\n");
        prompt.push_str("{\n");
        prompt.push_str("  \"reasoning\": \"Brief explanation of your thinking\",\n");
        prompt.push_str("  \"action\": {\n");
        prompt.push_str("    \"type\": \"tool|ask_user|complete|fail\",\n");
        prompt.push_str("    \"tool_name\": \"...\",      // if type=tool\n");
        prompt.push_str("    \"arguments\": {...},       // if type=tool\n");
        prompt.push_str("    \"question\": \"...\",        // if type=ask_user\n");
        prompt.push_str("    \"options\": [...],         // if type=ask_user (optional)\n");
        prompt.push_str("    \"summary\": \"...\",         // if type=complete (MUST be detailed report)\n");
        prompt.push_str("    \"reason\": \"...\"           // if type=fail\n");
        prompt.push_str("  }\n");
        prompt.push_str("}\n");
        prompt.push_str("```\n\n");
        prompt.push_str("### ask_user Format Details\n");
        prompt.push_str("When using `ask_user`, you have TWO modes:\n\n");

        prompt.push_str("**Mode 1: Single Question** (simple selection or text input)\n");
        prompt.push_str("- Use `options` field as an array of SEPARATE choices:\n");
        prompt.push_str("  - ✅ CORRECT: [\"Option 1\", \"Option 2\", \"Option 3\"]\n");
        prompt.push_str("  - ❌ WRONG: [\"Option 1 / Option 2 / Option 3\"] (single merged string)\n");
        prompt.push_str("- Each option should be a standalone, selectable choice\n");
        prompt.push_str("- If no options (free-form text input), omit the field or use empty array\n\n");

        prompt.push_str("**Mode 2: Multi-Group Questions** (multiple related questions)\n");
        prompt.push_str("Use this when you need answers to MULTIPLE independent questions simultaneously.\n");
        prompt.push_str("Instead of asking one by one, group them together for better UX.\n\n");

        prompt.push_str("```json\n");
        prompt.push_str("{\n");
        prompt.push_str("  \"reasoning\": \"Need multiple image generation parameters\",\n");
        prompt.push_str("  \"action\": {\n");
        prompt.push_str("    \"type\": \"ask_user_multigroup\",\n");
        prompt.push_str("    \"question\": \"Please configure the image generation settings\",  // Overall prompt\n");
        prompt.push_str("    \"groups\": [\n");
        prompt.push_str("      {\n");
        prompt.push_str("        \"id\": \"format\",  // Unique group ID (alphanumeric)\n");
        prompt.push_str("        \"prompt\": \"Output format\",\n");
        prompt.push_str("        \"options\": [\"PNG\", \"JPEG\", \"WebP\"]\n");
        prompt.push_str("      },\n");
        prompt.push_str("      {\n");
        prompt.push_str("        \"id\": \"quality\",\n");
        prompt.push_str("        \"prompt\": \"Quality level\",\n");
        prompt.push_str("        \"options\": [\"Low\", \"Medium\", \"High\", \"Best\"]\n");
        prompt.push_str("      },\n");
        prompt.push_str("      {\n");
        prompt.push_str("        \"id\": \"size\",\n");
        prompt.push_str("        \"prompt\": \"Image size\",\n");
        prompt.push_str("        \"options\": [\"512x512\", \"1024x1024\", \"2048x2048\"]\n");
        prompt.push_str("      }\n");
        prompt.push_str("    ]\n");
        prompt.push_str("  }\n");
        prompt.push_str("}\n");
        prompt.push_str("```\n\n");

        prompt.push_str("**When to use Multi-Group**:\n");
        prompt.push_str("- Multiple configuration options needed (3+ choices)\n");
        prompt.push_str("- Questions are independent but related\n");
        prompt.push_str("- Better UX than asking one-by-one\n");
        prompt.push_str("- Example: \"Choose format (PNG/JPEG), quality (Low/Medium/High), size (Small/Large)\"\n\n");

        prompt.push_str("**When NOT to use Multi-Group**:\n");
        prompt.push_str("- Single question with multiple options → Use simple `ask_user`\n");
        prompt.push_str("- Questions depend on previous answers → Ask sequentially\n");
        prompt.push_str("- Free-form text input → Use `ask_user` with no options\n\n");

        prompt.push_str("**Simple ask_user Example**:\n");
        prompt.push_str("```json\n");
        prompt.push_str("{\n");
        prompt.push_str("  \"reasoning\": \"Need user to select image format\",\n");
        prompt.push_str("  \"action\": {\n");
        prompt.push_str("    \"type\": \"ask_user\",\n");
        prompt.push_str("    \"question\": \"Which output format do you prefer?\",\n");
        prompt.push_str("    \"options\": [\"PNG\", \"JPEG\", \"WebP\"]\n");
        prompt.push_str("  }\n");
        prompt.push_str("}\n");
        prompt.push_str("```\n\n");
        prompt.push_str("### Completion Summary Format\n");
        prompt.push_str("When `type=complete`, the `summary` should be a well-formatted report:\n");
        prompt.push_str("```\n");
        prompt.push_str("## Task Completed\n");
        prompt.push_str("[Brief description of what was accomplished]\n\n");
        prompt.push_str("### Results\n");
        prompt.push_str("[Key findings, data, or outcomes]\n\n");
        prompt.push_str("### Generated Files\n");
        prompt.push_str("- file1.json: [description]\n");
        prompt.push_str("- file2.png: [description]\n\n");
        prompt.push_str("### Notes\n");
        prompt.push_str("[Any recommendations or important observations]\n");
        prompt.push_str("```\n\n");
    }

    /// Append guidelines section
    fn append_guidelines(&self, prompt: &mut String) {
        prompt.push_str("## Guidelines\n");
        prompt.push_str("1. Take ONE action at a time, observe the result, then decide next\n");
        prompt.push_str("2. Use tool results to inform subsequent decisions\n");
        prompt.push_str(
            "3. Ask user when: multiple valid approaches, unclear requirements, need confirmation\n",
        );
        prompt.push_str(
            "4. Complete when: task is done, or you've provided the requested information\n",
        );
        prompt.push_str("5. Fail when: impossible to proceed, missing critical resources\n\n");
    }

    /// Append thinking transparency guidance section
    ///
    /// Guides the AI on structured reasoning output:
    /// Observation -> Analysis -> Planning -> Decision
    fn append_thinking_guidance(&self, prompt: &mut String) {
        if !self.config.thinking_transparency {
            return;
        }

        prompt.push_str("## Thinking Transparency\n\n");
        prompt.push_str("Structure your reasoning to be transparent and understandable:\n\n");

        prompt.push_str("### Reasoning Flow\n");
        prompt.push_str("Follow this progression in your `reasoning` field:\n\n");
        prompt.push_str("1. **Observation** (👁️): Start by observing the current state\n");
        prompt.push_str("   - \"Looking at the request, I see...\"\n");
        prompt.push_str("   - \"The user wants to...\"\n");
        prompt.push_str("   - \"Based on the previous result...\"\n\n");
        prompt.push_str("2. **Analysis** (🔍): Analyze options and trade-offs\n");
        prompt.push_str("   - \"Considering the options: A vs B vs C...\"\n");
        prompt.push_str("   - \"The trade-off here is...\"\n");
        prompt.push_str("   - \"Comparing approaches...\"\n\n");
        prompt.push_str("3. **Planning** (📝): Outline your approach\n");
        prompt.push_str("   - \"I'll start by...\"\n");
        prompt.push_str("   - \"First, ... then...\"\n");
        prompt.push_str("   - \"My strategy is to...\"\n\n");
        prompt.push_str("4. **Decision** (✅): State your conclusion\n");
        prompt.push_str("   - \"Therefore, I will...\"\n");
        prompt.push_str("   - \"The best approach is...\"\n");
        prompt.push_str("   - \"So I've decided to...\"\n\n");

        prompt.push_str("### Expressing Uncertainty\n");
        prompt.push_str("When uncertain, be explicit rather than hiding it:\n\n");
        prompt.push_str("- **High confidence**: \"I'm confident that...\" or \"Clearly,...\"\n");
        prompt.push_str("- **Medium confidence**: \"I think...\" or \"This should work...\"\n");
        prompt.push_str("- **Low confidence**: \"I'm not sure, but...\" or \"This might...\"\n");
        prompt.push_str("- **Exploratory**: \"Let's try...\" or \"Worth experimenting with...\"\n\n");

        prompt.push_str("### Acknowledging Alternatives\n");
        prompt.push_str("When relevant, mention alternatives you considered:\n");
        prompt.push_str("- \"Alternatively, we could...\"\n");
        prompt.push_str("- \"Another option would be...\"\n");
        prompt.push_str("- \"I chose X over Y because...\"\n\n");

        prompt.push_str("This structured thinking helps users understand your reasoning process.\n\n");
    }

    /// Append skill mode section
    fn append_skill_mode(&self, prompt: &mut String) {
        if self.config.skill_mode {
            prompt.push_str("## ⚠️ Skill Execution Mode - CRITICAL RULES\n\n");
            prompt.push_str("You are executing a SKILL workflow. You MUST follow these rules EXACTLY:\n\n");
            prompt.push_str("### 🔴 RESPONSE FORMAT (MANDATORY)\n");
            prompt.push_str("**EVERY response MUST be a valid JSON action object. NEVER output raw content directly!**\n\n");
            prompt.push_str("❌ WRONG: Outputting processed text, data, or results directly\n");
            prompt.push_str("✅ CORRECT: Always return {\"reasoning\": \"...\", \"action\": {...}}\n\n");
            prompt.push_str("If you need to process data and save it, use the `file_ops` tool:\n");
            prompt.push_str("```json\n");
            prompt.push_str("{\"reasoning\": \"Writing processed data to file\", \"action\": {\"type\": \"tool\", \"tool_name\": \"file_ops\", \"arguments\": {\"operation\": \"write\", \"path\": \"output.json\", \"content\": \"...\"}}}\n");
            prompt.push_str("```\n\n");
            prompt.push_str("### Workflow Requirements\n");
            prompt.push_str("1. Complete ALL steps in the skill workflow - NO exceptions\n");
            prompt.push_str("2. Generate ALL output files specified (JSON, .mmd, .txt, images, etc.)\n");
            prompt.push_str("3. Use `file_ops` with `operation: \"write\"` to save each file\n");
            prompt.push_str("4. DO NOT skip any step, even if you think it's redundant\n");
            prompt.push_str("5. Before calling `complete`, verify ALL required outputs exist\n\n");
            prompt.push_str("### Common skill outputs to generate\n");
            prompt.push_str("- Data files: `triples.json`, `*.json`\n");
            prompt.push_str("- Visualization code: `graph.mmd`, `*.mmd`\n");
            prompt.push_str("- Prompts: `image-prompt.txt`, `*.txt`\n");
            prompt.push_str("- Images: via `generate_image` tool\n");
            prompt.push_str("- Merged outputs: `merged-*.json`, `full-*.mmd`\n\n");
            prompt.push_str("**If you output raw content instead of JSON action, you have FAILED.**\n\n");
        }
    }

    /// Append custom instructions section
    fn append_custom_instructions(&self, prompt: &mut String) {
        if let Some(instructions) = &self.config.custom_instructions {
            prompt.push_str("## Additional Instructions\n");
            prompt.push_str(instructions);
            prompt.push_str("\n\n");
        }
    }

    /// Append language setting section
    fn append_language_setting(&self, prompt: &mut String) {
        if let Some(lang) = &self.config.language {
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

    // ========== Soul/Identity Section Builders ==========

    /// Append soul/identity section at the very top of the prompt
    ///
    /// This section has the highest priority and defines core personality.
    pub fn append_soul_section(&self, prompt: &mut String, soul: &SoulManifest) {
        if soul.is_empty() {
            return;
        }

        prompt.push_str("# Identity\n\n");

        // Core identity statement
        if !soul.identity.is_empty() {
            prompt.push_str(&soul.identity);
            prompt.push_str("\n\n");
        }

        // Communication style
        if !soul.voice.tone.is_empty() {
            prompt.push_str("## Communication Style\n\n");
            prompt.push_str(&format!("- **Tone**: {}\n", soul.voice.tone));
            prompt.push_str(&format!("- **Verbosity**: {:?}\n", soul.voice.verbosity));
            prompt.push_str(&format!(
                "- **Formatting**: {:?}\n",
                soul.voice.formatting_style
            ));
            if let Some(ref notes) = soul.voice.language_notes {
                prompt.push_str(&format!("- **Language Notes**: {}\n", notes));
            }
            prompt.push('\n');
        }

        // Relationship mode
        prompt.push_str("## Relationship with User\n\n");
        prompt.push_str(soul.relationship.description());
        prompt.push_str("\n\n");

        // Expertise domains
        if !soul.expertise.is_empty() {
            prompt.push_str("## Areas of Expertise\n\n");
            for domain in &soul.expertise {
                prompt.push_str(&format!("- {}\n", domain));
            }
            prompt.push('\n');
        }

        // Behavioral directives
        if !soul.directives.is_empty() {
            prompt.push_str("## Behavioral Directives\n\n");
            for directive in &soul.directives {
                prompt.push_str(&format!("- {}\n", directive));
            }
            prompt.push('\n');
        }

        // Anti-patterns
        if !soul.anti_patterns.is_empty() {
            prompt.push_str("## What I Never Do\n\n");
            for anti in &soul.anti_patterns {
                prompt.push_str(&format!("- {}\n", anti));
            }
            prompt.push('\n');
        }

        // Custom addendum
        if let Some(ref addendum) = soul.addendum {
            prompt.push_str("## Additional Context\n\n");
            prompt.push_str(addendum);
            prompt.push_str("\n\n");
        }

        prompt.push_str("---\n\n");
    }

    /// Build system prompt with soul section at the top
    ///
    /// This is the primary entry point when using the Embodiment Engine.
    /// Soul content appears at the very top of the prompt for highest priority.
    pub fn build_system_prompt_with_soul(&self, tools: &[ToolInfo], soul: &SoulManifest) -> String {
        let mut prompt = String::with_capacity(16384);

        // Soul section at the very top (highest priority)
        self.append_soul_section(&mut prompt, soul);

        // Role definition
        prompt.push_str("You are an AI assistant executing tasks step by step.\n\n");

        // Core instructions
        prompt.push_str("## Your Role\n");
        prompt.push_str("- Observe the current state and history\n");
        prompt.push_str("- Decide the SINGLE next action to take\n");
        prompt.push_str("- Execute until the task is complete or you need user input\n\n");

        // Add runtime capabilities if configured
        self.append_runtime_capabilities(&mut prompt);

        // Add tools section
        self.append_tools(&mut prompt, tools);

        // Add generation models if configured
        self.append_generation_models(&mut prompt);

        // Add special actions
        self.append_special_actions(&mut prompt);

        // Add response format
        self.append_response_format(&mut prompt);

        // Add guidelines
        self.append_guidelines(&mut prompt);

        // Add thinking transparency guidance if enabled
        self.append_thinking_guidance(&mut prompt);

        // Add skill mode instructions if enabled
        self.append_skill_mode(&mut prompt);

        // Add custom instructions if configured
        self.append_custom_instructions(&mut prompt);

        // Add language setting
        self.append_language_setting(&mut prompt);

        prompt
    }

    // ========== Environment Contract & Security Section Builders ==========

    /// Append environment contract section describing the current channel capabilities
    ///
    /// This section informs the AI about:
    /// - The current interaction paradigm (CLI, WebRich, Messaging, etc.)
    /// - Active capabilities available in this environment
    /// - Interaction constraints (output limits, streaming support)
    pub fn append_environment_contract(&self, prompt: &mut String, contract: &EnvironmentContract) {
        prompt.push_str("## Environment Contract\n\n");

        // Paradigm description
        prompt.push_str(&format!(
            "**Paradigm**: {}\n\n",
            contract.paradigm.description()
        ));

        // Active capabilities
        if !contract.active_capabilities.is_empty() {
            prompt.push_str("**Active Capabilities**:\n");
            for cap in &contract.active_capabilities {
                let (name, hint) = cap.prompt_hint();
                prompt.push_str(&format!("- `{}`: {}\n", name, hint));
            }
            prompt.push('\n');
        }

        // Constraints
        let mut constraint_notes = Vec::new();
        if let Some(max_chars) = contract.constraints.max_output_chars {
            constraint_notes.push(format!("Max output: {} characters", max_chars));
        }
        if contract.constraints.prefer_compact {
            constraint_notes.push("Prefer concise responses".to_string());
        }
        if contract.constraints.supports_streaming {
            constraint_notes.push("Streaming enabled".to_string());
        }

        if !constraint_notes.is_empty() {
            prompt.push_str("**Constraints**:\n");
            for note in constraint_notes {
                prompt.push_str(&format!("- {}\n", note));
            }
            prompt.push('\n');
        }
    }

    /// Append security constraints section
    ///
    /// This section informs the AI about:
    /// - General security notes (sandbox level, filesystem scope, etc.)
    /// - Tools blocked by policy (should not be attempted)
    /// - Tools requiring user approval (can be used but need confirmation)
    pub fn append_security_constraints(
        &self,
        prompt: &mut String,
        disabled_tools: &[DisabledTool],
        security_notes: &[String],
    ) {
        // Only add section if there's something to report
        if security_notes.is_empty() && disabled_tools.is_empty() {
            return;
        }

        prompt.push_str("## Security & Constraints\n\n");

        // Security notes
        for note in security_notes {
            prompt.push_str(&format!("- {}\n", note));
        }
        if !security_notes.is_empty() {
            prompt.push('\n');
        }

        // Collect policy-blocked tools
        let blocked_by_policy: Vec<&DisabledTool> = disabled_tools
            .iter()
            .filter(|d| matches!(d.reason, DisableReason::BlockedByPolicy { .. }))
            .collect();

        if !blocked_by_policy.is_empty() {
            prompt.push_str("**Disabled by Policy**:\n");
            for tool in blocked_by_policy {
                if let DisableReason::BlockedByPolicy { ref reason } = tool.reason {
                    prompt.push_str(&format!("- `{}` — {}\n", tool.name, reason));
                }
            }
            prompt.push('\n');
        }

        // Collect approval-required tools
        let requires_approval: Vec<&DisabledTool> = disabled_tools
            .iter()
            .filter(|d| matches!(d.reason, DisableReason::RequiresApproval { .. }))
            .collect();

        if !requires_approval.is_empty() {
            prompt.push_str("**Requires User Approval**:\n");
            for tool in requires_approval {
                if let DisableReason::RequiresApproval { prompt: ref approval_prompt } = tool.reason
                {
                    prompt.push_str(&format!(
                        "- `{}` — available, but each invocation requires user confirmation ({})\n",
                        tool.name, approval_prompt
                    ));
                }
            }
            prompt.push('\n');
        }
    }

    /// Append silent behavior section for background/silent channels
    ///
    /// This section is only added when the environment supports silent replies
    /// (e.g., background processing channels). It instructs the AI on proper
    /// behavior for silent/heartbeat operations.
    pub fn append_silent_behavior(&self, prompt: &mut String, contract: &EnvironmentContract) {
        // Only add if SilentReply capability is active
        if !contract.active_capabilities.contains(&Capability::SilentReply) {
            return;
        }

        prompt.push_str("## Silent Behavior\n\n");
        prompt.push_str("You are running in a **background/silent context** where user notifications should be minimized.\n\n");
        prompt.push_str("**Guidelines**:\n");
        prompt.push_str("- Use `heartbeat_ok` for successful silent operations that need no user notification\n");
        prompt.push_str("- Use `silent_complete` when a background task finishes successfully\n");
        prompt.push_str("- Only use `ask_user` for critical decisions that cannot be automated\n");
        prompt.push_str("- Prefer logging results to files rather than generating verbose output\n");
        prompt.push_str("- Keep reasoning concise as it may not be visible to the user\n\n");
    }

    /// Build system prompt using ResolvedContext
    ///
    /// This is the new entry point that uses the two-phase filtered context
    /// from the ContextAggregator. It builds the complete system prompt with:
    /// 1. Role definition
    /// 2. Core instructions
    /// 3. Environment contract (paradigm, capabilities, constraints)
    /// 4. Runtime capabilities
    /// 5. Tools (using ctx.available_tools)
    /// 6. Security constraints (blocked tools, approval-required tools)
    /// 7. Silent behavior (if applicable)
    /// 8. Generation models
    /// 9. Special actions
    /// 10. Response format
    /// 11. Guidelines
    /// 12. Skill mode
    /// 13. Custom instructions
    /// 14. Language setting
    pub fn build_system_prompt_with_context(&self, ctx: &ResolvedContext) -> String {
        let mut prompt = String::new();

        // 1. Role definition
        prompt.push_str("You are an AI assistant executing tasks step by step.\n\n");

        // 2. Core instructions
        prompt.push_str("## Your Role\n");
        prompt.push_str("- Observe the current state and history\n");
        prompt.push_str("- Decide the SINGLE next action to take\n");
        prompt.push_str("- Execute until the task is complete or you need user input\n\n");

        // 3. Environment contract (NEW)
        self.append_environment_contract(&mut prompt, &ctx.environment_contract);

        // 4. Runtime capabilities
        self.append_runtime_capabilities(&mut prompt);

        // 5. Tools (using available_tools from context)
        self.append_tools(&mut prompt, &ctx.available_tools);

        // 6. Security constraints (NEW)
        self.append_security_constraints(
            &mut prompt,
            &ctx.disabled_tools,
            &ctx.environment_contract.security_notes,
        );

        // 7. Silent behavior (NEW - if applicable)
        self.append_silent_behavior(&mut prompt, &ctx.environment_contract);

        // 8. Generation models
        self.append_generation_models(&mut prompt);

        // 9. Special actions
        self.append_special_actions(&mut prompt);

        // 10. Response format
        self.append_response_format(&mut prompt);

        // 11. Guidelines
        self.append_guidelines(&mut prompt);

        // 12. Skill mode
        self.append_skill_mode(&mut prompt);

        // 13. Custom instructions
        self.append_custom_instructions(&mut prompt);

        // 14. Language setting
        self.append_language_setting(&mut prompt);

        prompt
    }

    /// Build two-part system prompt for Anthropic cache optimization
    ///
    /// Returns a vector of SystemPromptParts where:
    /// - Part 1: Static header (cacheable) - role definition, core instructions
    /// - Part 2: Dynamic content (not cacheable) - tools, runtimes, custom instructions
    ///
    /// This maximizes Anthropic's prompt cache hit rate by keeping
    /// the frequently-repeated header separate from dynamic content.
    pub fn build_system_prompt_cached(&self, tools: &[ToolInfo]) -> Vec<SystemPromptPart> {
        // Part 1: Static header (cacheable)
        let header = self.build_static_header();

        // Part 2: Dynamic content (not cacheable)
        let dynamic = self.build_dynamic_content(tools);

        vec![
            SystemPromptPart {
                content: header,
                cache: true,
            },
            SystemPromptPart {
                content: dynamic,
                cache: false,
            },
        ]
    }

    /// Build the static header portion of the system prompt
    ///
    /// This content is stable across invocations and can be cached.
    fn build_static_header(&self) -> String {
        let mut prompt = String::new();

        // Role definition
        prompt.push_str("You are an AI assistant executing tasks step by step.\n\n");

        // Core instructions
        prompt.push_str("## Your Role\n");
        prompt.push_str("- Observe the current state and history\n");
        prompt.push_str("- Decide the SINGLE next action to take\n");
        prompt.push_str("- Execute until the task is complete or you need user input\n\n");

        // Decision framework
        prompt.push_str("## Decision Framework\n");
        prompt.push_str("For each step, consider:\n");
        prompt.push_str("1. What is the current state?\n");
        prompt.push_str("2. What is the next logical step?\n");
        prompt.push_str("3. Which tool is most appropriate?\n\n");

        prompt
    }

    /// Build the dynamic content portion of the system prompt
    ///
    /// This content varies based on available tools, runtimes, and configuration.
    fn build_dynamic_content(&self, tools: &[ToolInfo]) -> String {
        let mut prompt = String::new();

        // Use shared helper methods to avoid duplication
        self.append_runtime_capabilities(&mut prompt);
        self.append_tools(&mut prompt, tools);
        self.append_generation_models(&mut prompt);
        self.append_special_actions(&mut prompt);
        self.append_response_format(&mut prompt);
        self.append_guidelines(&mut prompt);
        self.append_skill_mode(&mut prompt);
        self.append_custom_instructions(&mut prompt);
        self.append_language_setting(&mut prompt);

        prompt
    }

    /// Build messages for the thinking step
    pub fn build_messages(
        &self,
        original_request: &str,
        observation: &Observation,
    ) -> Vec<Message> {
        let mut messages = Vec::new();

        // 1. User's original request with context
        let mut user_msg = format!("Task: {}\n", original_request);

        // Add attachments info
        if !observation.attachments.is_empty() {
            user_msg.push_str("\nAttachments:\n");
            for (i, attachment) in observation.attachments.iter().enumerate() {
                user_msg.push_str(&format!("{}. {}\n", i + 1, format_attachment(attachment)));
            }
        }

        messages.push(Message::user(user_msg));

        // 2. Compressed history summary (if any)
        if !observation.history_summary.is_empty() {
            messages.push(Message::assistant(format!(
                "[Previous steps summary]\n{}",
                observation.history_summary
            )));
        }

        // 3. Recent steps with full details
        for step in &observation.recent_steps {
            // Assistant's thinking and action
            messages.push(Message::assistant(format!(
                "Reasoning: {}\nAction: {} {}",
                step.reasoning, step.action_type, step.action_args
            )));

            // CRITICAL FIX: User responses must use User role, not Tool role
            // This ensures the LLM understands the user has answered the question
            // and doesn't ask the same question again
            if step.action_type == "ask_user" {
                // User's response to a question - use User role
                messages.push(Message::user(step.result_output.clone()));
            } else {
                // Tool result - use full output to ensure LLM sees complete data
                // (e.g., full file paths, complete JSON output)
                messages.push(Message::tool_result(&step.action_type, &step.result_output));
            }
        }

        // 4. Current context and request for next action
        // IMPORTANT: Use clear system-level language to avoid confusing agent
        // with user instructions (e.g., "Current step: X" was misinterpreted
        // as user requesting to restart at step X, causing infinite loops)
        let context_msg = format!(
            "[System] Loop iteration: {} | Tokens: {} | Continue with your next action.",
            observation.current_step, observation.total_tokens
        );
        messages.push(Message::user(context_msg));

        messages
    }

    /// Build observation from state
    pub fn build_observation(
        &self,
        state: &LoopState,
        tools: &[ToolInfo],
        window_size: usize,
    ) -> Observation {
        let recent_steps: Vec<StepSummary> = state
            .recent_steps(window_size)
            .iter()
            .map(StepSummary::from)
            .collect();

        Observation {
            history_summary: state.history_summary.clone(),
            recent_steps,
            available_tools: tools.to_vec(),
            attachments: state.context.attachments.clone(),
            current_step: state.step_count,
            total_tokens: state.total_tokens,
        }
    }
}

/// Message type for LLM conversation
#[derive(Debug, Clone)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
}

/// Message role
#[derive(Debug, Clone, PartialEq)]
pub enum MessageRole {
    User,
    Assistant,
    Tool,
}

impl Message {
    /// Create a user message
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
        }
    }

    /// Create an assistant message
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
        }
    }

    /// Create a tool result message
    pub fn tool_result(tool_name: &str, result: &str) -> Self {
        Self {
            role: MessageRole::Tool,
            content: format!("[{}]\n{}", tool_name, result),
        }
    }
}

/// Safely truncate a string at character boundaries (UTF-8 safe)
fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let end_byte = s
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    format!("{}...", &s[..end_byte])
}

/// Format attachment for display
fn format_attachment(attachment: &MediaAttachment) -> String {
    let preview = truncate_str(&attachment.data, 50);

    match attachment.media_type.as_str() {
        "image" => {
            format!(
                "Image ({}, {} bytes)",
                attachment.mime_type,
                attachment.size_bytes
            )
        }
        "document" => {
            format!(
                "Document: {} ({}, {} bytes)",
                attachment.filename.as_deref().unwrap_or("unnamed"),
                attachment.mime_type,
                attachment.size_bytes
            )
        }
        "file" => {
            format!(
                "File: {} ({}, {} bytes)",
                attachment.filename.as_deref().unwrap_or("unnamed"),
                attachment.mime_type,
                attachment.size_bytes
            )
        }
        _ => {
            format!(
                "{}: {} ({} bytes)",
                attachment.media_type,
                attachment.filename.as_deref().unwrap_or(&preview),
                attachment.size_bytes
            )
        }
    }
}

// Tests migrated to BDD format in core/tests/features/thinker/prompt_builder.feature

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::soul::{SoulVoice, Verbosity};

    #[test]
    fn test_append_soul_section_empty() {
        let builder = PromptBuilder::new(PromptConfig::default());
        let mut prompt = String::new();
        let soul = SoulManifest::default();

        builder.append_soul_section(&mut prompt, &soul);

        // Empty soul should produce no output
        assert!(prompt.is_empty());
    }

    #[test]
    fn test_append_soul_section_basic() {
        let builder = PromptBuilder::new(PromptConfig::default());
        let mut prompt = String::new();

        let soul = SoulManifest {
            identity: "I am a test assistant.".to_string(),
            voice: SoulVoice {
                tone: "friendly".to_string(),
                verbosity: Verbosity::Balanced,
                ..Default::default()
            },
            directives: vec!["Be helpful".to_string()],
            anti_patterns: vec!["Don't be rude".to_string()],
            ..Default::default()
        };

        builder.append_soul_section(&mut prompt, &soul);

        assert!(prompt.contains("# Identity"));
        assert!(prompt.contains("I am a test assistant"));
        assert!(prompt.contains("Communication Style"));
        assert!(prompt.contains("friendly"));
        assert!(prompt.contains("Behavioral Directives"));
        assert!(prompt.contains("Be helpful"));
        assert!(prompt.contains("What I Never Do"));
        assert!(prompt.contains("Don't be rude"));
    }

    #[test]
    fn test_build_system_prompt_with_soul() {
        let builder = PromptBuilder::new(PromptConfig::default());

        let soul = SoulManifest {
            identity: "I am Aleph.".to_string(),
            directives: vec!["Help users".to_string()],
            ..Default::default()
        };

        let prompt = builder.build_system_prompt_with_soul(&[], &soul);

        // Soul should appear first
        let identity_pos = prompt.find("# Identity").unwrap();
        let role_pos = prompt.find("Your Role").unwrap();
        assert!(
            identity_pos < role_pos,
            "Identity should appear before Role"
        );

        // Standard sections should still be present
        assert!(prompt.contains("Response Format"));
        assert!(prompt.contains("JSON"));
    }

    #[test]
    fn test_soul_section_with_expertise() {
        let builder = PromptBuilder::new(PromptConfig::default());
        let mut prompt = String::new();

        let soul = SoulManifest {
            identity: "Expert assistant.".to_string(),
            expertise: vec!["Rust".to_string(), "Python".to_string()],
            ..Default::default()
        };

        builder.append_soul_section(&mut prompt, &soul);

        assert!(prompt.contains("Areas of Expertise"));
        assert!(prompt.contains("- Rust"));
        assert!(prompt.contains("- Python"));
    }

    #[test]
    fn test_thinking_guidance_disabled_by_default() {
        let builder = PromptBuilder::new(PromptConfig::default());
        let prompt = builder.build_system_prompt(&[]);

        // Default is off, so no thinking transparency section
        assert!(!prompt.contains("Thinking Transparency"));
        assert!(!prompt.contains("Reasoning Flow"));
    }

    #[test]
    fn test_thinking_guidance_enabled() {
        let config = PromptConfig {
            thinking_transparency: true,
            ..Default::default()
        };
        let builder = PromptBuilder::new(config);
        let prompt = builder.build_system_prompt(&[]);

        // Should contain thinking transparency section
        assert!(prompt.contains("## Thinking Transparency"));
        assert!(prompt.contains("### Reasoning Flow"));

        // Should contain the four phases
        assert!(prompt.contains("**Observation**"));
        assert!(prompt.contains("**Analysis**"));
        assert!(prompt.contains("**Planning**"));
        assert!(prompt.contains("**Decision**"));

        // Should contain uncertainty guidance
        assert!(prompt.contains("Expressing Uncertainty"));
        assert!(prompt.contains("High confidence"));
        assert!(prompt.contains("Low confidence"));

        // Should contain alternatives guidance
        assert!(prompt.contains("Acknowledging Alternatives"));
    }

    #[test]
    fn test_thinking_guidance_with_soul() {
        let config = PromptConfig {
            thinking_transparency: true,
            ..Default::default()
        };
        let builder = PromptBuilder::new(config);

        let soul = SoulManifest {
            identity: "Test assistant.".to_string(),
            ..Default::default()
        };

        let prompt = builder.build_system_prompt_with_soul(&[], &soul);

        // Both soul and thinking guidance should be present
        assert!(prompt.contains("# Identity"));
        assert!(prompt.contains("## Thinking Transparency"));
    }
}
