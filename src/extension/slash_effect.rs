//! The `slash_command` effect: a plugin's `commands/*.md` as `/name` entries
//! in the `ToolCatalog`, registered at mount and removed at unmount.
//!
//! Until this file existed the entries were written once at boot
//! (`bin/aleph-server/…/tool_catalog_init.rs`) and never removed, so a plugin
//! enabled after boot had no slash commands and a disabled one kept its
//! entries for the life of the process (scan-aleph-plugins.md §7 #5).

use crate::extension::effects::{async_disposer, Disposer};
use crate::extension::registry::SkillRegistration;
use crate::extension::types::SkillType;
use crate::skill::SkillInfo;
use crate::sync_primitives::Arc;
use crate::tool_metadata::ToolCatalog;

/// THE builder of a plugin command's `SkillInfo` — the one place this shape
/// is written for plugin commands (it replaces the literal that lived in
/// `bin/aleph-server/…/tool_catalog_init.rs`). The id is `qualified_name()`
/// (`<plugin>:<name>`), the registry's own key derivation, so the dispatch id
/// and the lookup key cannot drift apart. Later rounds add fields HERE
/// (`argument_hint` / `allowed_tools` / `model` from the command's
/// frontmatter), never at a second construction site.
pub(crate) fn plugin_command_skill_info(cmd: &SkillRegistration) -> SkillInfo {
    SkillInfo {
        id: cmd.qualified_name(),
        name: cmd.name.clone(),
        description: cmd.description.clone(),
        // Plugin commands have no SkillManifest behind them; System is the
        // manifest default and matches every other slash command.
        scope: crate::domain::skill::PromptScope::System,
        version: None,
        // No `allowed-tools:` to project: `None` keeps the agent's full tool
        // surface, which is what plugin commands have always done.
        allowed_tools: None,
        // The owner, for `extension::visibility` face ④ (the slash list and
        // the fast path). Unmount does not read it — the `slash_command`
        // disposer removes the exact ids it registered.
        plugin_id: Some(cmd.plugin_id.clone()),
    }
}

/// The plugin-command subset of a capability list, projected through
/// [`plugin_command_skill_info`].
pub(crate) fn plugin_command_skill_infos(commands: &[SkillRegistration]) -> Vec<SkillInfo> {
    commands
        .iter()
        .filter(|c| c.skill_type == SkillType::Command && !c.plugin_id.is_empty())
        .map(plugin_command_skill_info)
        .collect()
}

/// Register the entries and return the disposer that removes exactly them.
/// A skill the catalog refuses (unknown `allowed-tools:` name — impossible
/// here, `allowed_tools` is always `None`) would simply not be in the id
/// list the disposer removes.
pub(crate) async fn register_slash_commands_effect(
    catalog: Arc<ToolCatalog>,
    infos: Vec<SkillInfo>,
) -> Disposer {
    let rejected = catalog.register_skills(&infos).await;
    let ids: Vec<String> = infos
        .into_iter()
        .map(|i| i.id)
        .filter(|id| !rejected.contains(id))
        .collect();
    async_disposer(move || async move {
        let removed = catalog.unregister_skills(&ids).await;
        if removed == ids.len() {
            Ok(())
        } else {
            Err(format!(
                "expected to remove {} slash entries, removed {removed}",
                ids.len()
            ))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::registry::SkillRegistration;
    use crate::extension::types::SkillType;
    use crate::sync_primitives::Arc;
    use crate::tool_metadata::ToolCatalog;

    fn cmd(plugin: &str, name: &str) -> SkillRegistration {
        SkillRegistration {
            name: name.to_string(),
            plugin_id: plugin.to_string(),
            skill_type: SkillType::Command,
            description: format!("{name} description"),
            ..Default::default()
        }
    }

    #[test]
    fn plugin_command_skill_info_projects_the_qualified_name_and_nothing_else() {
        let info = plugin_command_skill_info(&cmd("qa-plug", "hello"));
        assert_eq!(info.id, "qa-plug:hello", "registry key = slash id");
        assert_eq!(info.name, "hello");
        assert_eq!(info.description, "hello description");
        assert_eq!(
            info.plugin_id.as_deref(),
            Some("qa-plug"),
            "the owner rides onto the catalog row for face ④"
        );
        assert_eq!(info.scope, crate::domain::skill::PromptScope::System);
        assert!(
            info.version.is_none(),
            "plugin commands carry no manifest version"
        );
        assert!(
            info.allowed_tools.is_none(),
            "no `allowed-tools:` → full tool surface"
        );
    }

    #[test]
    fn plugin_command_skill_infos_keeps_only_plugin_commands() {
        let infos = plugin_command_skill_infos(&[cmd("qa-plug", "hello")]);
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].id, "qa-plug:hello");
        // A registration that is not a Command is not a slash entry…
        let mut skill = cmd("qa-plug", "not-a-command");
        skill.skill_type = SkillType::Skill;
        assert!(plugin_command_skill_infos(&[skill]).is_empty());
        // …and neither is a command with no owning plugin.
        let orphan = cmd("", "loose");
        assert!(plugin_command_skill_infos(&[orphan]).is_empty());
    }

    #[tokio::test]
    async fn register_slash_commands_effect_round_trips_the_catalog() {
        let catalog = Arc::new(ToolCatalog::new());
        let baseline: Vec<String> = catalog
            .list_all()
            .await
            .into_iter()
            .map(|t| t.name)
            .collect();
        let infos = plugin_command_skill_infos(&[cmd("qa-plug", "hello"), cmd("qa-plug", "bye")]);
        let disposer = register_slash_commands_effect(Arc::clone(&catalog), infos).await;
        let during: Vec<String> = catalog
            .list_all()
            .await
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert!(during.contains(&"qa-plug:hello".to_string()));
        assert!(during.contains(&"qa-plug:bye".to_string()));
        disposer().await.unwrap();
        let after: Vec<String> = catalog
            .list_all()
            .await
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(
            after, baseline,
            "unmount leaves the catalog exactly as it was"
        );
    }
}
