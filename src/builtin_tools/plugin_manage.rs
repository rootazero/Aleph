//! Plugin management tool — inspect, toggle and configure plugins by
//! conversation.
//!
//! R8 (Everything is a Tool): every configurable Aleph operation should be
//! reachable through natural language. Plugins were the one extension kind
//! that was not. Skills have `skill_manage` / `skill_install` / `skill_list`,
//! hooks have `hooks_manage`, the Hub has its six `hub_*` tools — and plugin
//! enable / disable / reload / configure existed only as `plugin.*` and
//! `plugins.*` JSON-RPC, spoken by the CLI and the Panel.
//!
//! "Unreachable from conversation" would overstate it: `self_manage` returns a
//! manual telling the model to shell out to `aleph plugin …`. The accurate
//! statement is that plugin management had no **tool face**, so it was
//! governed by the sandbox command policy and the `bash` exec tier rather than
//! by tool-declared approval metadata, and it was invisible to `tool_search`
//! and progressive disclosure. The model could install a plugin from the Hub
//! and then had no way to enable, inspect or configure it.
//!
//! # What this tool deliberately cannot do
//!
//! **Install or uninstall.** Installing runs third-party code on the operator's
//! machine and uninstalling deletes a directory; both stay with the human and
//! the Hub's consent-gated `hub_install_run`. This tool operates on plugins
//! that are already on disk. The precedent is `hooks_manage`, which reports
//! consent state and structurally cannot grant it.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{AlephError, Result};
use crate::tools::AlephTool;

/// Action to perform on the plugin system.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PluginAction {
    /// List installed plugins with runtime kind, status and component counts.
    List,
    /// Show one plugin in detail, including its configuration schema.
    Show,
    /// Turn a plugin on.
    Enable,
    /// Turn a plugin off.
    Disable,
    /// Re-read a plugin from disk (picks up new configuration).
    Reload,
    /// Read a plugin's stored configuration.
    ConfigGet,
    /// Replace a plugin's stored configuration.
    ConfigSet,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PluginManageArgs {
    /// What to do.
    pub action: PluginAction,

    /// Plugin id. Required for everything except `list`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// For `config_set`: the complete configuration object. This REPLACES the
    /// stored configuration — read it with `config_get` first and send the
    /// merged result, or fields the operator set will disappear.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PluginManageOutput {
    /// Human-readable summary of what happened / what was found.
    pub summary: String,
    /// Structured payload; shape depends on the action.
    pub data: serde_json::Value,
}

/// Plugin management tool.
#[derive(Default, Clone)]
pub struct PluginManageTool;

impl PluginManageTool {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    fn require_name(args: &PluginManageArgs) -> Result<&str> {
        args.name
            .as_deref()
            .filter(|n| !n.is_empty())
            .ok_or_else(|| {
                AlephError::config("'name' is required — call action='list' to see plugin ids")
            })
    }
}

#[async_trait]
impl AlephTool for PluginManageTool {
    const NAME: &'static str = "plugin_manage";
    const DESCRIPTION: &'static str =
        "Inspect, enable/disable, reload and configure installed plugins. \
         Use action='list' to see every installed plugin with its runtime kind (wasm/mcp/static), \
         status, and how many skills/commands/agents/hooks/tools it contributes — a plugin whose \
         status is not 'loaded' carries the reason in status_detail, which is the answer to \
         'why isn't my plugin working?'. \
         action='config_get' returns the plugin's stored configuration together with the JSON \
         Schema its manifest declares, so you can see which fields exist and what they accept \
         before setting anything; action='config_set' REPLACES the whole configuration object, \
         so read it first and send the merged result. Configuration changes take effect on the \
         next reload — call action='reload' afterwards and say so. \
         This tool cannot install or uninstall plugins: installing runs third-party code, so it \
         stays with the operator (or the consent-gated hub_install_run). Do not claim you \
         installed or removed a plugin.";

    type Args = PluginManageArgs;
    type Output = PluginManageOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let manager = crate::extension::try_extension_manager().ok_or_else(|| {
            AlephError::config("The extension manager is not running in this process")
        })?;
        if let Err(e) = manager.ensure_loaded().await {
            tracing::warn!(error = %e, "plugin_manage: failed to load extensions");
        }

        match args.action {
            PluginAction::List => {
                let plugins = manager.get_plugin_info().await;
                let summary = format!("{} plugin(s) installed", plugins.len());
                Ok(PluginManageOutput {
                    summary,
                    data: serde_json::to_value(plugins)?,
                })
            }
            PluginAction::Show => {
                let name = Self::require_name(&args)?;
                let info = manager
                    .get_plugin_info()
                    .await
                    .into_iter()
                    .find(|p| p.name == name)
                    .ok_or_else(|| {
                        AlephError::config(format!(
                            "No plugin '{name}'. Call action='list' to see what is installed."
                        ))
                    })?;
                let config = manager.plugin_settings(name).await;
                Ok(PluginManageOutput {
                    summary: format!("{name}: {} ({})", info.status, info.kind),
                    data: serde_json::json!({ "plugin": info, "config": config }),
                })
            }
            PluginAction::Enable | PluginAction::Disable => {
                let name = Self::require_name(&args)?;
                let enable = matches!(args.action, PluginAction::Enable);
                let changed = manager.set_plugin_enabled(name, enable).await;
                let verb = if enable { "enabled" } else { "disabled" };
                Ok(PluginManageOutput {
                    summary: if changed {
                        format!("Plugin '{name}' {verb}")
                    } else {
                        format!("Plugin '{name}' was already {verb}")
                    },
                    data: serde_json::json!({ "name": name, "enabled": enable, "changed": changed }),
                })
            }
            PluginAction::Reload => {
                let name = Self::require_name(&args)?;
                manager
                    .reload_plugin(name)
                    .await
                    .map_err(|e| AlephError::config(format!("Reload failed: {e}")))?;
                Ok(PluginManageOutput {
                    summary: format!("Plugin '{name}' reloaded"),
                    data: serde_json::json!({ "name": name }),
                })
            }
            PluginAction::ConfigGet => {
                let name = Self::require_name(&args)?;
                let config = manager.plugin_settings(name).await;
                let schema = {
                    let registry = manager.get_plugin_registry().await;
                    registry.get_plugin(name).and_then(|record| {
                        crate::extension::manifest::parse_manifest_from_dir_cached_global(
                            &record.root_dir,
                        )
                        .ok()
                        .and_then(|m| m.config_schema.clone())
                    })
                };
                let summary = match &schema {
                    Some(_) => format!("Configuration for '{name}' (schema available)"),
                    None => format!(
                        "Configuration for '{name}'. This plugin declares no config_schema, \
                         so any field is accepted and none is required."
                    ),
                };
                Ok(PluginManageOutput {
                    summary,
                    data: serde_json::json!({ "name": name, "config": config, "schema": schema }),
                })
            }
            PluginAction::ConfigSet => {
                let name = Self::require_name(&args)?;
                let config = args.config.clone().ok_or_else(|| {
                    AlephError::config(
                        "'config' is required for config_set. It REPLACES the stored \
                         configuration — read it with config_get first and send the merged result.",
                    )
                })?;
                match manager.set_plugin_settings(name, config).await {
                    Ok(changed) => Ok(PluginManageOutput {
                        summary: if changed {
                            format!(
                                "Configuration for '{name}' saved. It takes effect on the next \
                                 reload — call action='reload' to apply it now."
                            )
                        } else {
                            format!("Configuration for '{name}' is unchanged")
                        },
                        data: serde_json::json!({
                            "name": name,
                            "changed": changed,
                            "reload_required": changed,
                        }),
                    }),
                    Err(errors) => Err(AlephError::config(format!(
                        "Configuration rejected by the plugin's schema:\n  - {}",
                        errors.join("\n  - ")
                    ))),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every action except `list` addresses one plugin, and a missing id must
    /// be a named refusal rather than an operation on an empty string.
    #[tokio::test]
    async fn actions_that_need_a_plugin_id_say_so() {
        for action in [
            PluginAction::Show,
            PluginAction::Enable,
            PluginAction::Disable,
            PluginAction::Reload,
            PluginAction::ConfigGet,
            PluginAction::ConfigSet,
        ] {
            let args = PluginManageArgs {
                action,
                name: None,
                config: None,
            };
            assert!(
                PluginManageTool::require_name(&args).is_err(),
                "{action:?} must refuse a missing name"
            );
        }
    }

    /// An empty string is not a plugin id; treating it as one would address
    /// whatever a lookup does with `""`.
    #[test]
    fn an_empty_name_is_not_a_name() {
        let args = PluginManageArgs {
            action: PluginAction::Show,
            name: Some(String::new()),
            config: None,
        };
        assert!(PluginManageTool::require_name(&args).is_err());
    }

    /// The description tells the model this tool cannot install — the claim it
    /// would otherwise be most tempted to make, since `hub_install_run` exists
    /// next to it.
    #[test]
    fn the_description_disclaims_install() {
        let d = PluginManageTool::DESCRIPTION;
        assert!(d.contains("cannot install"));
        assert!(d.contains("hub_install_run"));
    }
}
