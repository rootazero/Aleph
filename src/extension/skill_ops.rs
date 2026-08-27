//! Skill, command, and agent query/execution operations for `ExtensionManager`

use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::types::{
    ExtensionAgent, ExtensionCommand, ExtensionSkill, SkillToolResult, SkillType,
};

use super::ExtensionManager;

impl ExtensionManager {
    // ── Skill / Command / Agent Queries ───────────────────────────────────────

    /// Get all skills (from enabled sources)
    pub async fn get_all_skills(&self) -> Vec<ExtensionSkill> {
        let reg = self.plugin_registry.read().await;
        reg.list_skills()
            .into_iter()
            .filter(|skill| {
                reg.get_plugin(&skill.plugin_id)
                    .is_some_and(|plugin| plugin.status.is_active())
            })
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
            .filter(|skill| {
                reg.get_plugin(&skill.plugin_id)
                    .is_some_and(|plugin| plugin.status.is_active())
            })
            .cloned()
            .collect()
    }

    /// Get all agents
    pub async fn get_all_agents(&self) -> Vec<ExtensionAgent> {
        let reg = self.plugin_registry.read().await;
        reg.list_agents()
            .into_iter()
            .filter(|agent| {
                reg.get_plugin(&agent.plugin_id)
                    .is_some_and(|plugin| plugin.status.is_active())
            })
            .cloned()
            .collect()
    }

    /// Get a specific skill by qualified name
    pub async fn get_skill(&self, qualified_name: &str) -> Option<ExtensionSkill> {
        let reg = self.plugin_registry.read().await;
        let skill = reg.get_skill(qualified_name)?;
        reg.get_plugin(&skill.plugin_id)
            .filter(|plugin| plugin.status.is_active())
            .map(|_| skill.clone())
    }

    /// Get a specific command by name
    pub async fn get_command(&self, name: &str) -> Option<ExtensionCommand> {
        // Try as skill with Command type
        let reg = self.plugin_registry.read().await;
        let skill = reg
            .get_skill(name)
            .filter(|s| s.skill_type == SkillType::Command)?;
        reg.get_plugin(&skill.plugin_id)
            .filter(|plugin| plugin.status.is_active())
            .map(|_| skill.clone())
    }

    /// Get a specific agent by name
    pub async fn get_agent(&self, name: &str) -> Option<ExtensionAgent> {
        let reg = self.plugin_registry.read().await;
        let agent = reg.get_agent(name)?;
        reg.get_plugin(&agent.plugin_id)
            .filter(|plugin| plugin.status.is_active())
            .map(|_| agent.clone())
    }

    /// Get all primary agents (can be selected by user)
    pub async fn get_primary_agents(&self) -> Vec<ExtensionAgent> {
        let reg = self.plugin_registry.read().await;
        reg.list_agents()
            .into_iter()
            .filter(|a| a.is_primary() && !a.hidden)
            .filter(|agent| {
                reg.get_plugin(&agent.plugin_id)
                    .is_some_and(|plugin| plugin.status.is_active())
            })
            .cloned()
            .collect()
    }

    /// Get all sub-agents (can be delegated to)
    pub async fn get_sub_agents(&self) -> Vec<ExtensionAgent> {
        let reg = self.plugin_registry.read().await;
        reg.list_agents()
            .into_iter()
            .filter(|a| a.is_subagent())
            .filter(|agent| {
                reg.get_plugin(&agent.plugin_id)
                    .is_some_and(|plugin| plugin.status.is_active())
            })
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
        let skill = self
            .get_skill(qualified_name)
            .await
            .ok_or_else(|| ExtensionError::SkillNotFound(qualified_name.to_string()))?;
        Ok(skill.with_arguments(arguments))
    }

    /// Execute a command with arguments
    pub async fn execute_command(&self, name: &str, arguments: &str) -> ExtensionResult<String> {
        let cmd = self
            .get_command(name)
            .await
            .ok_or_else(|| ExtensionError::CommandNotFound(name.to_string()))?;
        Ok(cmd.with_arguments(arguments))
    }

    // ── Skill Tool (LLM-callable) ─────────────────────────────────────────────

    /// Invoke a skill as an LLM tool
    pub async fn invoke_skill_tool(
        &self,
        qualified_name: &str,
        arguments: &str,
    ) -> ExtensionResult<SkillToolResult> {
        self.ensure_loaded().await?;
        let skill = self
            .get_skill(qualified_name)
            .await
            .ok_or_else(|| ExtensionError::SkillNotFound(qualified_name.to_string()))?;
        super::skill_tool::invoke_skill(&skill, arguments).await
    }

    // ── Hook Execution ────────────────────────────────────────────────────────

    /// Get a stable snapshot of the current hook executor.
    ///
    /// The snapshot is cheap to clone and lets callers execute hooks without
    /// holding the extension manager's internal lock for the full agent run.
    pub async fn hook_executor_snapshot(&self) -> super::hooks::HookExecutor {
        self.hook_executor.read().await.clone()
    }

    /// Get the discovery manager
    pub const fn discovery(&self) -> &crate::discovery::DiscoveryManager {
        &self.discovery
    }

    /// Get the Aleph home directory
    pub fn aleph_home(&self) -> ExtensionResult<std::path::PathBuf> {
        Ok(self.discovery.aleph_home())
    }

    /// Get the Skill System v2 instance.
    pub const fn skill_system(&self) -> &crate::skill::SkillSystem {
        &self.skill_system
    }
}
