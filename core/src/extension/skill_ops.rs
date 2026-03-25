//! Skill, command, and agent query/execution operations for ExtensionManager

use std::collections::HashMap;

use crate::extension::config;
use crate::extension::error::*;
use crate::extension::types::*;

use super::ExtensionManager;

impl ExtensionManager {
    // ── Skill / Command / Agent Queries ───────────────────────────────────────

    /// Get all skills (from enabled sources)
    pub async fn get_all_skills(&self) -> Vec<ExtensionSkill> {
        let reg = self.plugin_registry.read().await;
        reg.list_skills().into_iter().cloned().collect()
    }

    /// Get auto-invocable skills (for LLM prompt injection)
    pub async fn get_auto_invocable_skills(&self) -> Vec<ExtensionSkill> {
        let reg = self.plugin_registry.read().await;
        reg.list_skills()
            .into_iter()
            .filter(|s| s.is_auto_invocable())
            .cloned()
            .collect()
    }

    /// Get all commands
    pub async fn get_all_commands(&self) -> Vec<ExtensionCommand> {
        // Commands are now stored as skills with SkillType::Command in the registry
        let reg = self.plugin_registry.read().await;
        reg.list_skills()
            .into_iter()
            .filter(|s| s.skill_type == SkillType::Command)
            .cloned()
            .collect()
    }

    /// Get all agents
    pub async fn get_all_agents(&self) -> Vec<ExtensionAgent> {
        let reg = self.plugin_registry.read().await;
        reg.list_agents().into_iter().cloned().collect()
    }

    /// Get a specific skill by qualified name
    pub async fn get_skill(&self, qualified_name: &str) -> Option<ExtensionSkill> {
        let reg = self.plugin_registry.read().await;
        reg.get_skill(qualified_name).cloned()
    }

    /// Get a specific command by name
    pub async fn get_command(&self, name: &str) -> Option<ExtensionCommand> {
        // Try as skill with Command type
        let reg = self.plugin_registry.read().await;
        reg.get_skill(name)
            .filter(|s| s.skill_type == SkillType::Command)
            .cloned()
    }

    /// Get a specific agent by name
    pub async fn get_agent(&self, name: &str) -> Option<ExtensionAgent> {
        let reg = self.plugin_registry.read().await;
        reg.get_agent(name).cloned()
    }

    /// Get all primary agents (can be selected by user)
    pub async fn get_primary_agents(&self) -> Vec<ExtensionAgent> {
        let reg = self.plugin_registry.read().await;
        reg.list_agents()
            .into_iter()
            .filter(|a| a.is_primary() && !a.hidden)
            .cloned()
            .collect()
    }

    /// Get all sub-agents (can be delegated to)
    pub async fn get_sub_agents(&self) -> Vec<ExtensionAgent> {
        let reg = self.plugin_registry.read().await;
        reg.list_agents()
            .into_iter()
            .filter(|a| a.is_subagent())
            .cloned()
            .collect()
    }

    // ── Skill / Command Execution ─────────────────────────────────────────────

    /// Execute a skill with arguments
    pub async fn execute_skill(
        &self,
        qualified_name: &str,
        arguments: &str,
    ) -> ExtensionResult<String> {
        let skill = self.get_skill(qualified_name).await.ok_or_else(|| {
            ExtensionError::SkillNotFound(qualified_name.to_string())
        })?;
        Ok(skill.with_arguments(arguments))
    }

    /// Execute a command with arguments
    pub async fn execute_command(
        &self,
        name: &str,
        arguments: &str,
    ) -> ExtensionResult<String> {
        let cmd = self.get_command(name).await.ok_or_else(|| {
            ExtensionError::CommandNotFound(name.to_string())
        })?;
        Ok(cmd.with_arguments(arguments))
    }

    // ── Skill Tool (LLM-callable) ─────────────────────────────────────────────

    /// Invoke a skill as an LLM tool
    pub async fn invoke_skill_tool(
        &self,
        qualified_name: &str,
        arguments: &str,
        ctx: &SkillContext,
    ) -> ExtensionResult<SkillToolResult> {
        self.ensure_loaded().await?;
        let skill = self.get_skill(qualified_name).await.ok_or_else(|| {
            ExtensionError::SkillNotFound(qualified_name.to_string())
        })?;
        super::skill_tool::invoke_skill(&skill, arguments, ctx).await
    }

    // ── Hook Execution ────────────────────────────────────────────────────────

    /// Execute hooks for an event
    pub async fn execute_hooks(
        &self,
        event: HookEvent,
        context: &super::hooks::HookContext,
    ) -> ExtensionResult<super::hooks::HookResult> {
        self.hook_executor.read().await.execute(event, context).await
    }

    /// Get the number of registered hooks
    pub async fn hook_count(&self) -> usize {
        self.hook_executor.read().await.hook_count()
    }

    // ── Configuration Access ──────────────────────────────────────────────────

    /// Get all MCP server configurations from config
    pub async fn get_mcp_servers(&self) -> HashMap<String, McpServerConfig> {
        let mut servers = HashMap::new();

        if let Some(mcp) = self.config_manager.get_mcp_servers() {
            for (name, cfg) in mcp {
                match cfg {
                    config::McpConfig::Local { command, environment, .. } => {
                        let (cmd, args) = if command.is_empty() {
                            (String::new(), Vec::new())
                        } else {
                            (command[0].clone(), command[1..].to_vec())
                        };
                        servers.insert(name.clone(), McpServerConfig {
                            command: cmd,
                            args,
                            env: environment.clone(),
                        });
                    }
                    config::McpConfig::Remote { url, .. } => {
                        tracing::debug!("Remote MCP server {} at {} not yet supported", name, url);
                    }
                }
            }
        }

        servers
    }

    /// Get the merged configuration
    pub fn get_config(&self) -> &super::AlephConfig {
        self.config_manager.get_config()
    }

    /// Get the discovery manager
    pub fn discovery(&self) -> &crate::discovery::DiscoveryManager {
        &self.discovery
    }

    /// Get the Aleph home directory
    pub fn aleph_home(&self) -> ExtensionResult<std::path::PathBuf> {
        Ok(self.discovery.aleph_home()?)
    }

    /// Get the default model from configuration
    pub fn get_default_model(&self) -> Option<&str> {
        self.config_manager.get_config().model.as_deref()
    }

    /// Get the small/fast model from configuration
    pub fn get_small_model(&self) -> Option<&str> {
        self.config_manager.get_config().small_model.as_deref()
    }

    /// Get the default agent from configuration
    pub fn get_default_agent(&self) -> Option<&str> {
        self.config_manager.get_config().default_agent.as_deref()
    }

    /// Get all custom instructions
    pub fn get_instructions(&self) -> Vec<&str> {
        self.config_manager
            .get_config()
            .instructions
            .as_ref()
            .map(|v| v.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default()
    }

    /// Get the Skill System v2 instance.
    pub fn skill_system(&self) -> &crate::skill::SkillSystem {
        &self.skill_system
    }
}
