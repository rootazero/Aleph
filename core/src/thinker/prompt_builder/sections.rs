//! Prompt section builders (append_* methods)
//!
//! All `append_*` methods that build individual prompt sections live here.

use crate::agent_loop::ToolInfo;
use crate::dispatcher::tool_index::HydrationResult;

use super::PromptBuilder;
use crate::thinker::context::{DisableReason, DisabledTool, EnvironmentContract};
use crate::thinker::interaction::Capability;
use crate::thinker::prompt_sanitizer::{sanitize_for_prompt, SanitizeLevel};
use crate::thinker::soul::SoulManifest;

impl PromptBuilder {
    // ========== Shared prompt section builders ==========

    /// Append runtime capabilities section
    pub(crate) fn append_runtime_capabilities(&self, prompt: &mut String) {
        if let Some(ref runtimes) = self.config.runtime_capabilities {
            let runtimes = sanitize_for_prompt(runtimes, SanitizeLevel::Light);
            prompt.push_str("## Available Runtimes\n\n");
            prompt.push_str("You can execute code using these installed runtimes:\n\n");
            prompt.push_str(&runtimes);
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

    /// Append runtime context section (micro-environmental awareness)
    pub fn append_runtime_context_section(
        &self,
        prompt: &mut String,
        runtime_ctx: &crate::thinker::runtime_context::RuntimeContext,
    ) {
        prompt.push_str(&runtime_ctx.to_prompt_section());
        prompt.push_str("\n\n");
    }

    /// Append available tools section
    pub(crate) fn append_tools(&self, prompt: &mut String, tools: &[ToolInfo]) {
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
    pub(crate) fn append_generation_models(&self, prompt: &mut String) {
        if let Some(ref models) = self.config.generation_models {
            let models = sanitize_for_prompt(models, SanitizeLevel::Light);
            prompt.push_str("## Media Generation Models\n\n");
            prompt.push_str(&models);
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

    /// Append special actions section
    pub(crate) fn append_special_actions(&self, prompt: &mut String) {
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
    pub(crate) fn append_response_format(&self, prompt: &mut String) {
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
    pub(crate) fn append_guidelines(&self, prompt: &mut String) {
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

    /// Append constitutional AI safety guardrails
    ///
    /// Adds foundational safety principles covering:
    /// - Autonomy boundaries (no independent goals)
    /// - Oversight priority (human supervision)
    /// - Transparency (explain before acting)
    /// - Data handling (protect credentials and sensitive data)
    pub fn append_safety_constitution(&self, prompt: &mut String) {
        prompt.push_str("## Safety Principles\n\n");

        prompt.push_str("### Autonomy Boundaries\n");
        prompt.push_str("- You have NO independent goals. Your purpose is to help the user.\n");
        prompt.push_str("- You NEVER take actions that could affect systems beyond the user's workspace without explicit approval.\n");
        prompt.push_str("- When uncertain about safety implications, ASK rather than ACT.\n\n");

        prompt.push_str("### Oversight Priority\n");
        prompt.push_str("- Human oversight is paramount. Never bypass, disable, or work around safety mechanisms.\n");
        prompt.push_str("- If a tool requires approval, explain why you need it and wait.\n");
        prompt.push_str("- Never attempt to elevate your own permissions or access.\n\n");

        prompt.push_str("### Transparency\n");
        prompt.push_str("- Always explain what you're about to do before doing it (for impactful actions).\n");
        prompt.push_str("- If you make a mistake, acknowledge it immediately.\n");
        prompt.push_str("- Never hide errors or pretend actions succeeded when they didn't.\n\n");

        prompt.push_str("### Data Handling\n");
        prompt.push_str("- Never expose, transmit, or store credentials, API keys, or sensitive data unless explicitly directed by the user.\n");
        prompt.push_str("- In group contexts, respect that private user information should not be shared.\n\n");
    }

    /// Append memory-first guidance to the system prompt
    ///
    /// Instructs the AI to proactively search persistent memory before
    /// answering context-dependent questions, and to store new facts
    /// discovered during conversations.
    pub fn append_memory_guidance(&self, prompt: &mut String) {
        prompt.push_str("## Memory Protocol\n\n");
        prompt.push_str("You have persistent memory across sessions. Use it.\n\n");

        prompt.push_str("### Before Answering\n");
        prompt.push_str("When the user asks about past work, preferences, or context:\n");
        prompt.push_str("1. FIRST use `memory_search` to recall relevant facts\n");
        prompt.push_str("2. THEN answer with recalled context\n");
        prompt.push_str("3. ALWAYS cite sources: [Source: <path>#<id>]\n\n");

        prompt.push_str("### After Learning\n");
        prompt.push_str("When you discover new facts worth remembering:\n");
        prompt.push_str("- User preferences → use `memory_store` with category \"user_preference\"\n");
        prompt.push_str("- Project decisions → use `memory_store` with category \"project_decision\"\n");
        prompt.push_str("- Task outcomes → use `memory_store` with category \"task_outcome\"\n\n");

        prompt.push_str("### Memory Hygiene\n");
        prompt.push_str("- Don't store trivial or temporary information\n");
        prompt.push_str("- Don't store information the user explicitly asks you to forget\n");
        prompt.push_str("- Update existing facts rather than creating duplicates\n\n");
    }

    /// Append soul continuity guidance to the system prompt
    ///
    /// Instructs the AI to incrementally evolve its soul manifest
    /// based on interactions, rather than rewriting identity wholesale.
    pub fn append_soul_continuity(&self, prompt: &mut String) {
        prompt.push_str(
            "## Soul Continuity\n\n\
             Your identity files are your persistent memory of who you are.\n\
             - After meaningful interactions that reveal new preferences, update your soul\n\
             - After corrections from the user (\"don't do that\"), add anti-patterns\n\
             - After discovering new expertise areas, extend your expertise list\n\
             - Rule: Changes are gradual. Never rewrite your entire identity at once.\n\n"
        );
    }

    /// Append thinking transparency guidance section
    ///
    /// Guides the AI on structured reasoning output:
    /// Observation -> Analysis -> Planning -> Decision
    pub(crate) fn append_thinking_guidance(&self, prompt: &mut String) {
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
    pub(crate) fn append_skill_mode(&self, prompt: &mut String) {
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

    /// Append skill instructions from SkillSystem v2 snapshot
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

    /// Append custom instructions section
    pub(crate) fn append_custom_instructions(&self, prompt: &mut String) {
        if let Some(instructions) = &self.config.custom_instructions {
            let instructions = sanitize_for_prompt(instructions, SanitizeLevel::Moderate);
            let instructions = sanitize_for_prompt(&instructions, SanitizeLevel::Light);
            prompt.push_str("## Additional Instructions\n");
            prompt.push_str(&instructions);
            prompt.push_str("\n\n");
        }
    }

    /// Append language setting section
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

    // ========== Soul/Identity Section Builders ==========

    /// Append soul/identity section at the very top of the prompt
    ///
    /// This section has the highest priority and defines core personality.
    /// All soul fields are sanitized at Moderate level since they come from
    /// user-editable files.
    pub fn append_soul_section(&self, prompt: &mut String, soul: &SoulManifest) {
        if soul.is_empty() {
            return;
        }

        prompt.push_str("# Identity\n\n");

        // Core identity statement
        if !soul.identity.is_empty() {
            let identity = sanitize_for_prompt(&soul.identity, SanitizeLevel::Moderate);
            let identity = sanitize_for_prompt(&identity, SanitizeLevel::Light);
            prompt.push_str(&identity);
            prompt.push_str("\n\n");
        }

        // Communication style
        if !soul.voice.tone.is_empty() {
            let tone = sanitize_for_prompt(&soul.voice.tone, SanitizeLevel::Moderate);
            let tone = sanitize_for_prompt(&tone, SanitizeLevel::Light);
            prompt.push_str("## Communication Style\n\n");
            prompt.push_str(&format!("- **Tone**: {}\n", tone));
            prompt.push_str(&format!("- **Verbosity**: {:?}\n", soul.voice.verbosity));
            prompt.push_str(&format!(
                "- **Formatting**: {:?}\n",
                soul.voice.formatting_style
            ));
            if let Some(ref notes) = soul.voice.language_notes {
                let notes = sanitize_for_prompt(notes, SanitizeLevel::Moderate);
                let notes = sanitize_for_prompt(&notes, SanitizeLevel::Light);
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
                let domain = sanitize_for_prompt(domain, SanitizeLevel::Moderate);
                let domain = sanitize_for_prompt(&domain, SanitizeLevel::Light);
                prompt.push_str(&format!("- {}\n", domain));
            }
            prompt.push('\n');
        }

        // Behavioral directives
        if !soul.directives.is_empty() {
            prompt.push_str("## Behavioral Directives\n\n");
            for directive in &soul.directives {
                let directive = sanitize_for_prompt(directive, SanitizeLevel::Moderate);
                let directive = sanitize_for_prompt(&directive, SanitizeLevel::Light);
                prompt.push_str(&format!("- {}\n", directive));
            }
            prompt.push('\n');
        }

        // Anti-patterns
        if !soul.anti_patterns.is_empty() {
            prompt.push_str("## What I Never Do\n\n");
            for anti in &soul.anti_patterns {
                let anti = sanitize_for_prompt(anti, SanitizeLevel::Moderate);
                let anti = sanitize_for_prompt(&anti, SanitizeLevel::Light);
                prompt.push_str(&format!("- {}\n", anti));
            }
            prompt.push('\n');
        }

        // Custom addendum
        if let Some(ref addendum) = soul.addendum {
            let addendum = sanitize_for_prompt(addendum, SanitizeLevel::Moderate);
            let addendum = sanitize_for_prompt(&addendum, SanitizeLevel::Light);
            prompt.push_str("## Additional Context\n\n");
            prompt.push_str(&addendum);
            prompt.push_str("\n\n");
        }

        prompt.push_str("---\n\n");
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
            let note = sanitize_for_prompt(note, SanitizeLevel::Light);
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

    /// Append protocol tokens section (replaces append_silent_behavior for protocol-aware mode)
    ///
    /// When SilentReply capability is active, injects structured protocol tokens
    /// that the LLM can use as minimal-cost responses in background mode.
    pub fn append_protocol_tokens(
        &self,
        prompt: &mut String,
        contract: &EnvironmentContract,
    ) {
        if !contract.active_capabilities.contains(&Capability::SilentReply) {
            return;
        }
        prompt.push_str(&crate::thinker::protocol_tokens::ProtocolToken::to_prompt_section());
    }

    /// Append system operational awareness guidelines.
    ///
    /// Only injected for Background and CLI paradigms where the LLM
    /// may need to detect and report system issues proactively.
    pub fn append_operational_guidelines(
        &self,
        prompt: &mut String,
        paradigm: crate::thinker::interaction::InteractionParadigm,
    ) {
        match paradigm {
            crate::thinker::interaction::InteractionParadigm::Background
            | crate::thinker::interaction::InteractionParadigm::CLI => {}
            _ => return, // Skip for Messaging, WebRich, Embedded
        }

        prompt.push_str("## System Operational Awareness\n\n");
        prompt.push_str(
            "You are aware of your own runtime environment and can monitor it proactively.\n\n",
        );

        prompt.push_str("### Diagnostic Capabilities (read-only, always allowed)\n");
        prompt.push_str("- Check disk space: `df -h`\n");
        prompt.push_str("- Check memory usage: `vm_stat` / `free -h`\n");
        prompt.push_str("- Check running Aleph processes: `ps aux | grep aleph`\n");
        prompt.push_str(
            "- Check configuration validity: read config files and validate structure\n",
        );
        prompt.push_str("- Check Desktop Bridge status: query UDS socket availability\n");
        prompt.push_str("- Check LanceDB health: verify database file accessibility\n\n");

        prompt.push_str("### When You Detect Issues\n");
        prompt.push_str(
            "If you notice configuration conflicts, database issues, disconnected bridges,\n",
        );
        prompt.push_str("abnormal resource usage, or runtime capability degradation:\n\n");
        prompt.push_str("**Action**: Report to the user with:\n");
        prompt.push_str("1. What you observed (specific evidence)\n");
        prompt.push_str("2. Potential impact\n");
        prompt.push_str("3. Suggested remediation steps\n");
        prompt.push_str("4. Do NOT execute remediation without explicit user approval\n\n");

        prompt.push_str("### What You Must NEVER Do Autonomously\n");
        prompt.push_str("- Restart Aleph services\n");
        prompt.push_str("- Modify configuration files\n");
        prompt.push_str("- Delete or compact databases\n");
        prompt.push_str("- Kill processes\n");
        prompt.push_str("- Change system settings\n\n");
    }

    /// Append memory citation standards.
    ///
    /// Always injected — citation standards are valuable in all interaction modes.
    pub fn append_citation_standards(&self, prompt: &mut String) {
        prompt.push_str("## Citation Standards\n\n");
        prompt.push_str("When referencing information from memory or knowledge base:\n");
        prompt.push_str("- Include source reference in format: `[Source: <path>#<id>]` or `[Source: <path>#L<line>]`\n");
        prompt.push_str("- Sources are provided in the context metadata — do not fabricate source paths\n");
        prompt.push_str("- If multiple sources support a claim, cite the most specific one\n");
        prompt.push_str("- For real-time observations (current tool output, live data), no citation needed\n");
        prompt.push_str("- For recalled facts, prior decisions, or historical context, citation is mandatory\n\n");
    }

    /// Append channel-specific behavioral guidance.
    pub fn append_channel_behavior(
        &self,
        prompt: &mut String,
        guide: &crate::thinker::channel_behavior::ChannelBehaviorGuide,
    ) {
        let section = sanitize_for_prompt(&guide.to_prompt_section(), SanitizeLevel::Light);
        prompt.push_str(&section);
    }

    /// Append user profile section to the prompt.
    pub fn append_user_profile(
        &self,
        prompt: &mut String,
        profile: &crate::thinker::user_profile::UserProfile,
    ) {
        // User profile is loaded from user-editable files → Moderate + Light
        let section = sanitize_for_prompt(&profile.to_prompt_section(), SanitizeLevel::Moderate);
        let section = sanitize_for_prompt(&section, SanitizeLevel::Light);
        prompt.push_str(&section);
    }
}
