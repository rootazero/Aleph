//! Skill / command query operations for `ExtensionManager`.
//!
//! Earlier revisions also hosted per-name lookup (`get_skill`, `get_command`,
//! `get_agent`), per-list `get_all_*`, and the `execute_*` / `invoke_skill_tool`
//! helpers — every one of those has been superseded. The live consumers of
//! plugin-side data today read through [`super::ExtensionManager`] itself
//! (`active_plugin_tools_snapshot`, `plugin_agent_to_def`, the registry's
//! iterators) and the per-request tool service, so the eleven dead methods
//! were cut from this module on 2026-09-04; the zero-caller `discovery()`
//! accessor followed on 2026-10-02.
//!
//! What remains: [`ExtensionManager::skill_system`] (handle accessor used by
//! tool catalog init) and [`ExtensionManager::hook_executor_snapshot`] (cheap handle clone consumed
//! by the hook executor when wiring plugin hooks). Plugin `commands/` entries
//! are registered per mount by `slash_effect.rs`, not listed from here.

use super::ExtensionManager;

impl ExtensionManager {
    /// Get a stable snapshot of the current hook executor.
    ///
    /// The snapshot is cheap to clone and lets callers execute hooks without
    /// holding the extension manager's internal lock for the full agent run.
    pub async fn hook_executor_snapshot(&self) -> super::hooks::HookExecutor {
        self.hook_executor.read().await.clone()
    }

    /// Get the Skill System v2 instance.
    pub const fn skill_system(&self) -> &crate::skill::SkillSystem {
        &self.skill_system
    }
}
