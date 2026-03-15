//! Skill and agent loading logic

use super::*;

impl ContentLoader {
    /// Load a skill from a directory or file
    pub async fn load_skill(&self, path: &Path) -> ExtensionResult<ExtensionSkill> {
        self.load_skill_internal(path, None, SkillType::Skill).await
    }

    /// Load a command from a directory or file
    pub async fn load_command(&self, path: &Path) -> ExtensionResult<ExtensionCommand> {
        self.load_skill_internal(path, None, SkillType::Command)
            .await
    }

    /// Load a skill/command with optional plugin name
    pub(super) async fn load_skill_internal(
        &self,
        path: &Path,
        plugin_name: Option<String>,
        skill_type: SkillType,
    ) -> ExtensionResult<ExtensionSkill> {
        // Determine the markdown file path
        let (md_path, name) = if path.is_dir() {
            // Directory format: look for SKILL.md
            let skill_md = path.join(SKILL_FILE);
            if !skill_md.exists() {
                return Err(ExtensionError::missing_field(path, "SKILL.md"));
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            (skill_md, name)
        } else if path.extension().map(|e| e == "md").unwrap_or(false) {
            // Direct .md file
            let name = path
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            (path.to_path_buf(), name)
        } else {
            return Err(ExtensionError::invalid_manifest(
                path,
                "Expected directory with SKILL.md or .md file",
            ));
        };

        debug!("Loading skill from: {:?}", md_path);

        // Read and parse markdown
        let content = tokio::fs::read_to_string(&md_path).await?;
        let (frontmatter, body) = parse_frontmatter::<SkillFrontmatter>(&content, &md_path)?;

        // Build skill
        let skill = ExtensionSkill {
            name: frontmatter.name.unwrap_or(name),
            plugin_name,
            skill_type,
            description: frontmatter.description.unwrap_or_default(),
            content: body,
            disable_model_invocation: frontmatter.disable_model_invocation,
            scope: frontmatter.scope.unwrap_or_default(),
            bound_tool: frontmatter.bound_tool,
            source_path: path.to_path_buf(),
            source: determine_source(path),
        };

        trace!("Loaded skill: {:?}", skill.qualified_name());
        Ok(skill)
    }

    /// Load an agent from a directory or file
    pub async fn load_agent(&self, path: &Path) -> ExtensionResult<ExtensionAgent> {
        self.load_agent_internal(path, None).await
    }

    /// Load an agent with optional plugin name
    pub(super) async fn load_agent_internal(
        &self,
        path: &Path,
        plugin_name: Option<String>,
    ) -> ExtensionResult<ExtensionAgent> {
        // Determine the markdown file path
        let (md_path, name) = if path.is_dir() {
            // Directory format: look for agent.md
            let agent_md = path.join(AGENT_FILE);
            if !agent_md.exists() {
                return Err(ExtensionError::missing_field(path, "agent.md"));
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            (agent_md, name)
        } else if path.extension().map(|e| e == "md").unwrap_or(false) {
            // Direct .md file
            let name = path
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            (path.to_path_buf(), name)
        } else {
            return Err(ExtensionError::invalid_manifest(
                path,
                "Expected directory with agent.md or .md file",
            ));
        };

        debug!("Loading agent from: {:?}", md_path);

        // Read and parse markdown
        let content = tokio::fs::read_to_string(&md_path).await?;
        let (frontmatter, body) = parse_frontmatter::<AgentFrontmatter>(&content, &md_path)?;

        // Build agent
        let agent = ExtensionAgent {
            name,
            plugin_name,
            mode: frontmatter.mode.unwrap_or_default(),
            description: frontmatter.description,
            hidden: frontmatter.hidden.unwrap_or(false),
            color: frontmatter.color,
            model: frontmatter.model,
            temperature: frontmatter.temperature,
            top_p: frontmatter.top_p,
            steps: frontmatter.steps,
            tools: frontmatter.tools,
            permission: frontmatter.permission,
            options: frontmatter.options.unwrap_or_default(),
            system_prompt: body,
            source_path: path.to_path_buf(),
            source: determine_source(path),
        };

        trace!("Loaded agent: {:?}", agent.qualified_name());
        Ok(agent)
    }

    /// Load all skills from a directory
    pub(super) async fn load_skills_from_dir(
        &self,
        dir: &Path,
        plugin_name: Option<String>,
    ) -> ExtensionResult<Vec<ExtensionSkill>> {
        let mut skills = Vec::new();

        let mut entries = tokio::fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();

            // Skip hidden entries
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }

            // Load skill
            match self
                .load_skill_internal(&path, plugin_name.clone(), SkillType::Skill)
                .await
            {
                Ok(skill) => skills.push(skill),
                Err(e) => {
                    tracing::warn!("Failed to load skill from {:?}: {}", path, e);
                }
            }
        }

        Ok(skills)
    }

    /// Load all commands from a directory
    pub(super) async fn load_commands_from_dir(
        &self,
        dir: &Path,
        plugin_name: Option<String>,
    ) -> ExtensionResult<Vec<ExtensionCommand>> {
        let mut commands = Vec::new();

        let mut entries = tokio::fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();

            // Skip hidden entries
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }

            // Load command
            match self
                .load_skill_internal(&path, plugin_name.clone(), SkillType::Command)
                .await
            {
                Ok(cmd) => commands.push(cmd),
                Err(e) => {
                    tracing::warn!("Failed to load command from {:?}: {}", path, e);
                }
            }
        }

        Ok(commands)
    }

    /// Load all agents from a directory
    pub(super) async fn load_agents_from_dir(
        &self,
        dir: &Path,
        plugin_name: Option<String>,
    ) -> ExtensionResult<Vec<ExtensionAgent>> {
        let mut agents = Vec::new();

        let mut entries = tokio::fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();

            // Skip hidden entries
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }

            // Load agent
            match self.load_agent_internal(&path, plugin_name.clone()).await {
                Ok(agent) => agents.push(agent),
                Err(e) => {
                    tracing::warn!("Failed to load agent from {:?}: {}", path, e);
                }
            }
        }

        Ok(agents)
    }
}
