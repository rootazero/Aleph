//! Skill / command query operations for `ExtensionManager`.
//!
//! Earlier revisions also hosted per-name lookup (`get_skill`, `get_command`,
//! `get_agent`), per-list `get_all_*`, and the `execute_*` / `invoke_skill_tool`
//! helpers — every one of those has been superseded. The live consumers of
//! plugin-side data today read through [`super::ExtensionManager`] itself
//! (`active_plugin_tools_snapshot`, `plugin_agent_to_def`, the registry's
//! iterators) and the per-request tool service, so the eleven dead methods
//! were cut from this module on 2026-09-04.
//!
//! What remains: [`ExtensionManager::get_all_commands`] (the only list query
//! still used at boot), [`ExtensionManager::skill_system`] (handle accessor
//! used by tool catalog init), [`ExtensionManager::discovery`] (read-only
//! handle), and [`ExtensionManager::hook_executor_snapshot`] (cheap handle
//! clone consumed by the hook executor when wiring plugin hooks).

use crate::extension::types::{ExtensionCommand, SkillType};

use super::ExtensionManager;

impl ExtensionManager {
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

    /// Get the Skill System v2 instance.
    pub const fn skill_system(&self) -> &crate::skill::SkillSystem {
        &self.skill_system
    }
}
