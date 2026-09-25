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
/// and the lookup key cannot drift apart. Fields from the command's
/// frontmatter are added HERE, never at a second construction site.
pub(crate) fn plugin_command_skill_info(cmd: &SkillRegistration) -> SkillInfo {
    SkillInfo {
        id: cmd.qualified_name(),
        name: cmd.name.clone(),
        description: cmd.description.clone(),
        // Plugin commands have no SkillManifest behind them; System is the
        // manifest default and matches every other slash command.
        scope: crate::domain::skill::PromptScope::System,
        version: None,
        // The command's own `allowed-tools`, already Aleph names
        // (`manifest/parsers.rs`, restrict mode). `None` keeps the full surface.
        allowed_tools: cmd.allowed_tools.clone(),
        argument_hint: cmd.argument_hint.clone(),
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
///
/// `declared_mcp_servers` are the server ids this plugin declares
/// (`mcp_config::plugin_server_id`). A command may name their tools although
/// no catalog row exists yet: a server's tools reach the catalog only when it
/// starts, and nothing orders that before this effect. Ownership is decided by
/// the MCP handler's own key rule (`tools::handlers::mcp::is_tool_key_of_server`),
/// so a server this plugin does not declare stays unknown.
///
/// A command the catalog refuses (an `allowed-tools:` name the registry does
/// not know at registration time — e.g. a CC tool in neither of
/// `extension::hooks`' alias tables, which is forwarded under its own name,
/// or a tool of an MCP server this plugin does not declare, the known false
/// negative of `resolve_skill_tool_scope`) is simply not in the id list the
/// disposer removes, and `register_skills` has already warned by name.
pub(crate) async fn register_slash_commands_effect(
    catalog: Arc<ToolCatalog>,
    infos: Vec<SkillInfo>,
    declared_mcp_servers: Vec<String>,
) -> Disposer {
    let admit = move |name: &str| {
        declared_mcp_servers
            .iter()
            .any(|server| crate::tools::handlers::mcp::is_tool_key_of_server(server, name))
    };
    let rejected = catalog.register_skills_admitting(&infos, &admit).await;
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
    fn plugin_command_skill_info_projects_the_qualified_name_and_the_frontmatter() {
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
        // The frontmatter half (P4.7b): declared `allowed-tools` (already
        // Aleph names) and `argument-hint` ride onto the SkillInfo.
        let mut with_fm = cmd("qa-plug", "review");
        with_fm.allowed_tools = Some(vec!["grep".into(), "bash".into()]);
        with_fm.argument_hint = Some("[pr-number]".into());
        let info = plugin_command_skill_info(&with_fm);
        assert_eq!(
            info.allowed_tools.as_deref(),
            Some(&["grep".to_string(), "bash".to_string()][..])
        );
        assert_eq!(info.argument_hint.as_deref(), Some("[pr-number]"));
        // …and a command that declares nothing keeps the full surface.
        let bare = plugin_command_skill_info(&cmd("qa-plug", "hello"));
        assert!(bare.allowed_tools.is_none() && bare.argument_hint.is_none());
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
        let disposer =
            register_slash_commands_effect(Arc::clone(&catalog), infos, Vec::new()).await;
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
