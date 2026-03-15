//! V2 prompt loading methods

use super::*;

impl ContentLoader {
    /// Load V2 global prompt from manifest's `[prompt]` section
    ///
    /// This method reads the prompt file specified in the TOML manifest and
    /// creates an ExtensionSkill with the appropriate scope settings.
    ///
    /// # Arguments
    /// * `manifest` - The parsed V2 plugin manifest
    /// * `plugin_dir` - The plugin directory containing the prompt file
    ///
    /// # Returns
    /// * `Ok(Some(skill))` - If a prompt is configured and loaded successfully
    /// * `Ok(None)` - If no prompt is configured or it's disabled
    /// * `Err(ExtensionError)` - If the prompt file cannot be read or parsed
    pub async fn load_v2_prompt(
        &self,
        manifest: &PluginManifest,
        plugin_dir: &Path,
    ) -> ExtensionResult<Option<ExtensionSkill>> {
        let prompt_config = match &manifest.prompt_v2 {
            Some(p) => p,
            None => return Ok(None),
        };

        // Check if disabled
        if prompt_config.scope == "disabled" {
            return Ok(None);
        }

        // Read prompt file
        let prompt_path = plugin_dir.join(&prompt_config.file);
        let content = tokio::fs::read_to_string(&prompt_path).await.map_err(|e| {
            ExtensionError::invalid_manifest(
                &prompt_path,
                format!("Failed to read prompt file: {}", e),
            )
        })?;

        // Parse frontmatter if present
        let (frontmatter, body) = if content.starts_with("---") {
            parse_frontmatter::<SkillFrontmatter>(&content, &prompt_path)?
        } else {
            (SkillFrontmatter::default(), content)
        };

        let skill = ExtensionSkill {
            name: frontmatter.name.unwrap_or_else(|| manifest.id.clone()),
            plugin_name: Some(manifest.id.clone()),
            skill_type: SkillType::Skill,
            description: frontmatter.description.unwrap_or_default(),
            content: body,
            disable_model_invocation: frontmatter.disable_model_invocation,
            source_path: prompt_path,
            source: DiscoverySource::Plugin,
            scope: PromptScope::from_str_or_default(&prompt_config.scope),
            bound_tool: None,
        };

        debug!("Loaded V2 prompt for plugin {}: scope={:?}", manifest.id, skill.scope);
        Ok(Some(skill))
    }

    /// Load V2 tool-bound prompts (instruction files) from manifest's `[[tools]]` sections
    ///
    /// This method loads instruction files specified in tool definitions and creates
    /// ExtensionSkills bound to specific tools. These are automatically injected
    /// when the associated tool is available.
    ///
    /// # Arguments
    /// * `manifest` - The parsed V2 plugin manifest
    /// * `plugin_dir` - The plugin directory containing the instruction files
    ///
    /// # Returns
    /// * `Ok(Vec<ExtensionSkill>)` - Tool-bound skills loaded from instruction files
    /// * `Err(ExtensionError)` - If an instruction file cannot be read
    pub async fn load_v2_tool_prompts(
        &self,
        manifest: &PluginManifest,
        plugin_dir: &Path,
    ) -> ExtensionResult<Vec<ExtensionSkill>> {
        let tools = match &manifest.tools_v2 {
            Some(t) => t,
            None => return Ok(vec![]),
        };

        let mut skills = Vec::new();

        for tool in tools {
            if let Some(ref instruction_file) = tool.instruction_file {
                let path = plugin_dir.join(instruction_file);
                if path.exists() {
                    let content = tokio::fs::read_to_string(&path).await.map_err(|e| {
                        ExtensionError::invalid_manifest(
                            &path,
                            format!("Failed to read instruction file: {}", e),
                        )
                    })?;

                    let skill = ExtensionSkill {
                        name: format!("{}_instructions", tool.name),
                        plugin_name: Some(manifest.id.clone()),
                        skill_type: SkillType::Skill,
                        description: format!("Instructions for {} tool", tool.name),
                        content,
                        disable_model_invocation: true, // Tool-bound, not direct invoke
                        source_path: path.clone(),
                        source: DiscoverySource::Plugin,
                        scope: PromptScope::Tool,
                        bound_tool: Some(tool.name.clone()),
                    };

                    debug!(
                        "Loaded V2 tool prompt for {}/{}: {}",
                        manifest.id, tool.name, path.display()
                    );
                    skills.push(skill);
                } else {
                    trace!(
                        "Tool instruction file not found for {}/{}: {}",
                        manifest.id, tool.name, path.display()
                    );
                }
            }
        }

        Ok(skills)
    }
}
