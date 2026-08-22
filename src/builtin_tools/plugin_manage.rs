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
//!
//! # Marketplaces are a different verb
//!
//! Registering a marketplace is not installing a plugin: it records where a
//! catalogue lives and, on sync, `git clone`s a directory of manifests.
//! Nothing from it executes until a human installs something. That is why the
//! five `marketplace_*` actions live here while `install` does not, and why
//! the description says both things rather than leaving the model to infer
//! which side of the boundary it is on.
//!
//! Before these existed, `plugin.marketplace.{list,add,remove}` had exactly
//! two clients: the Panel, and `interfaces/cli` — a binary the release
//! workflow does not build. A conversation could ask which plugins were
//! available and get nothing back.

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
    /// Show the owner trust policy: whether it is enforced, and which plugins
    /// are vouched for.
    TrustStatus,
    /// Vouch for a plugin, so it may load from an untrusted origin.
    Trust,
    /// Withdraw vouching for a plugin.
    Untrust,
    /// Turn owner-trust enforcement on or off (`enforce`).
    TrustEnforce,
    /// List registered marketplaces, with the type and source of each and
    /// whether it can be removed.
    MarketplaceList,
    /// List the plugins a marketplace offers. Reads the local cache; it does
    /// not fetch.
    MarketplaceBrowse,
    /// Register a marketplace from `source`, then fetch its contents.
    MarketplaceAdd,
    /// Drop a marketplace registration and its cache.
    MarketplaceRemove,
    /// Re-fetch one marketplace (`name`) or all of them.
    MarketplaceUpdate,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PluginManageArgs {
    /// What to do.
    pub action: PluginAction,

    /// Plugin id — or, for the `marketplace_*` actions, the marketplace name.
    /// Required for everything except `list`, `marketplace_list`,
    /// `marketplace_browse`, `marketplace_add` and `marketplace_update`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// For `marketplace_add`: an `owner/repo` slug, a GitHub URL, or a local
    /// directory path. The name and type are derived from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,

    /// For `marketplace_browse`: substring filter over plugin ids and
    /// descriptions. Omit to list everything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,

    /// For `config_set`: the complete configuration object. This REPLACES the
    /// stored configuration — read it with `config_get` first and send the
    /// merged result, or fields the operator set will disappear.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Value>,

    /// For `trust_enforce`: whether to enforce the trust allowlist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enforce: Option<bool>,
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
         action='trust_status' reports the owner trust policy; when it is enforced, plugins from \
         workspace/global directories load only if action='trust' vouched for them, and the rest \
         are listed with status 'blocked'. trust/untrust and trust_enforce are LOAD gates: they \
         do not stop a plugin that is already running — action='disable' does. \
         This tool cannot install or uninstall plugins: installing runs third-party code, so it \
         stays with the operator (or the consent-gated hub_install_run). Do not claim you \
         installed or removed a plugin. \
         The marketplace_* actions manage plugin CATALOGUES, which is a different thing from \
         installing: marketplace_list shows what is registered and whether each can be removed, \
         marketplace_browse reports what a marketplace offers so you can tell the operator (a \
         local cache read; run marketplace_update first if it says the cache is missing, and note \
         that installing from it is still the operator's to do -- the rows say whether an \
         entry is one Aleph can install at all), marketplace_add \
         takes an owner/repo slug, a GitHub URL or a local directory in `source` and both \
         registers and fetches it, and marketplace_remove drops a registration and its cache. \
         Registering a catalogue never runs anything from it.";

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
            PluginAction::TrustStatus => {
                let (enforced, trusted) = manager.trust_status().await;
                let summary = if enforced {
                    format!(
                        "Owner trust is ENFORCED. {} plugin(s) vouched for; any other plugin \
                         from a workspace or global directory is refused at load.",
                        trusted.len()
                    )
                } else {
                    format!(
                        "Owner trust is NOT enforced — every installed plugin loads. \
                         {} plugin(s) are already marked trusted and would keep loading if \
                         you turn enforcement on.",
                        trusted.len()
                    )
                };
                Ok(PluginManageOutput {
                    summary,
                    data: serde_json::json!({ "enforced": enforced, "trusted": trusted }),
                })
            }
            PluginAction::Trust | PluginAction::Untrust => {
                let name = Self::require_name(&args)?;
                let trust = matches!(args.action, PluginAction::Trust);
                let changed = manager.set_plugin_trusted(name, trust).await;
                let (enforced, _) = manager.trust_status().await;
                // Two things a caller must not infer: that untrusting stops a
                // running plugin (it is a load gate — `disable` is the verb
                // that stops one now), and that trusting matters at all while
                // enforcement is off.
                let summary = if trust {
                    let tail = if enforced {
                        " It may now load from a workspace or global directory."
                    } else {
                        " Note that owner trust is not currently enforced, so this changes \
                          nothing until you enable it with action='trust_enforce'."
                    };
                    format!("Plugin '{name}' is now trusted.{tail}")
                } else {
                    format!(
                        "Plugin '{name}' is no longer trusted. It will be refused at the next \
                         load; if it is running now, that does not stop it — use \
                         action='disable' for that."
                    )
                };
                Ok(PluginManageOutput {
                    summary,
                    data: serde_json::json!({
                        "name": name,
                        "trusted": trust,
                        "changed": changed,
                        "enforced": enforced,
                    }),
                })
            }
            PluginAction::TrustEnforce => {
                let enforce = args.enforce.ok_or_else(|| {
                    AlephError::config(
                        "'enforce' (true/false) is required for trust_enforce. \
                         Call action='trust_status' first to see the current posture and \
                         which plugins are already vouched for.",
                    )
                })?;
                let changed = manager.set_trust_enforced(enforce).await;
                let (_, trusted) = manager.trust_status().await;
                let summary = if enforce {
                    format!(
                        "Owner trust enforcement is ON. {} plugin(s) are vouched for; \
                         everything else in a workspace or global plugin directory will be \
                         refused at the next load and listed with status 'blocked'. \
                         Plugins running now are not stopped.",
                        trusted.len()
                    )
                } else {
                    "Owner trust enforcement is OFF — every installed plugin loads.".to_string()
                };
                Ok(PluginManageOutput {
                    summary,
                    data: serde_json::json!({
                        "enforced": enforce,
                        "changed": changed,
                        "trusted": trusted,
                    }),
                })
            }
            PluginAction::MarketplaceList
            | PluginAction::MarketplaceBrowse
            | PluginAction::MarketplaceAdd
            | PluginAction::MarketplaceRemove
            | PluginAction::MarketplaceUpdate => {
                // `git clone`, config file reads and directory deletion are all
                // blocking, and this runs on the agent loop's executor.
                tokio::task::spawn_blocking(move || Self::marketplace(args))
                    .await
                    .map_err(|e| {
                        AlephError::config(format!("Marketplace task failed to run: {e}"))
                    })?
            }
        }
    }
}

impl PluginManageTool {
    /// The marketplace half. Split out because it touches no plugin registry:
    /// it reads and writes `[plugin_marketplaces]` and the marketplace cache,
    /// and folding it into the match above would put a second subsystem
    /// inside one 200-line function.
    fn marketplace(args: PluginManageArgs) -> Result<PluginManageOutput> {
        use crate::extension::marketplace::{classify, MarketplaceConfig, MarketplaceManager};
        // The two row builders the Panel and the CLI already render, so
        // "what a marketplace row looks like" has one answer across all three.
        use crate::gateway::handlers::plugins::types::{
            marketplace_registration_row, marketplace_row,
        };

        let mut manager =
            MarketplaceManager::from_config().map_err(|e| AlephError::config(e.to_string()))?;

        match args.action {
            PluginAction::MarketplaceList => {
                // Same row builder as `plugin.marketplace.list`, so the
                // `removable` bit the model reads is the server's own refusal
                // and not a second reading of it.
                let mut rows: Vec<aleph_protocol::plugins::MarketplaceRow> = manager
                    .list()
                    .iter()
                    .map(|(name, config)| marketplace_registration_row(name, config))
                    .collect();
                // `list()` hands back a HashMap; an unordered answer would
                // reshuffle on every call for no reason.
                rows.sort_by(|a, b| a.name.cmp(&b.name));
                Ok(PluginManageOutput {
                    summary: format!("{} marketplace(s) registered", rows.len()),
                    data: serde_json::json!({ "marketplaces": rows }),
                })
            }

            PluginAction::MarketplaceBrowse => {
                let listing = manager.browse(args.name.as_deref(), args.query.as_deref());
                // Projected rather than passed straight through. The wire row
                // carries `installable`, whose subject is the Panel's Install
                // button — and this tool has no install verb, so a bit by that
                // name on this surface reads as an invitation it cannot honour
                // ("a list that comes with an action invitation must have every
                // row be actionable by it"). Renaming it for its real subject
                // keeps the one derivation (`marketplace_row`) and drops the
                // false invitation; the reason a row is not fetchable stays,
                // because that IS what the operator needs to hear.
                let plugins: Vec<serde_json::Value> = listing
                    .entries
                    .iter()
                    .map(|entry| {
                        let row = marketplace_row(entry);
                        serde_json::json!({
                            "name": row.name,
                            "marketplace": row.marketplace,
                            "description": row.description,
                            "version": row.version,
                            "operator_can_install": row.installable,
                            "unavailable_reason": row.unavailable_reason,
                        })
                    })
                    .collect();
                let problems: Vec<serde_json::Value> = listing
                    .problems
                    .iter()
                    .map(|p| serde_json::json!({ "marketplace": p.marketplace, "reason": p.reason }))
                    .collect();
                Ok(PluginManageOutput {
                    summary: format!(
                        "{} plugin(s) offered; {} marketplace(s) unreadable",
                        plugins.len(),
                        problems.len()
                    ),
                    data: serde_json::json!({
                        "plugins": plugins,
                        "problems": problems,
                    }),
                })
            }

            PluginAction::MarketplaceAdd => {
                let source = args.source.as_deref().filter(|s| !s.trim().is_empty()).ok_or_else(|| {
                    AlephError::config(
                        "'source' is required for marketplace_add — an owner/repo slug, a GitHub \
                         URL, or a local directory path",
                    )
                })?;
                // One classifier for every face; see `marketplace::source_spec`
                // for the two heuristics this replaced.
                let spec = classify(source, args.name.as_deref())
                    .map_err(|e| AlephError::config(e.to_string()))?;

                let entry = MarketplaceConfig {
                    source: spec.source.clone(),
                    source_type: spec.source_type,
                };
                manager.add(spec.name.clone(), entry.clone());

                let mut config = crate::config::Config::load()?;
                config
                    .plugin_marketplaces
                    .insert(spec.name.clone(), (&entry).into());
                config.save_incremental(&["plugin_marketplaces"])?;

                // Registering does not fetch. Composed here for the same
                // reason the Panel and the shipped subcommand compose it: a
                // catalogue that is registered but empty looks broken, and
                // nothing on this surface hints that a second call fills it.
                let fetch_error = manager.update(&spec.name).err();
                Ok(PluginManageOutput {
                    summary: match &fetch_error {
                        None => format!("Registered '{}' and fetched its contents", spec.name),
                        Some(e) => format!(
                            "Registered '{}', but fetching its contents failed: {e}",
                            spec.name
                        ),
                    },
                    data: serde_json::json!({
                        "name": spec.name,
                        "source": spec.source,
                        "type": spec.source_type.as_config_str(),
                        "fetched": fetch_error.is_none(),
                        "fetch_error": fetch_error,
                    }),
                })
            }

            PluginAction::MarketplaceRemove => {
                let name = Self::require_name(&args)?.to_string();
                manager
                    .remove(&name)
                    .map_err(|e| AlephError::config(e.to_string()))?;

                let mut config = crate::config::Config::load()?;
                config.plugin_marketplaces.remove(&name);
                config.save_incremental(&["plugin_marketplaces"])?;

                Ok(PluginManageOutput {
                    summary: format!("Marketplace '{name}' removed"),
                    data: serde_json::json!({ "name": name }),
                })
            }

            PluginAction::MarketplaceUpdate => {
                let result = match args.name.as_deref().filter(|n| !n.is_empty()) {
                    Some(name) => manager.update(name).map(|_| ()),
                    None => manager.update_all(),
                };
                result.map_err(|e| AlephError::config(e.to_string()))?;
                Ok(PluginManageOutput {
                    summary: match args.name.as_deref() {
                        Some(n) => format!("Marketplace '{n}' updated"),
                        None => "All marketplaces updated".to_string(),
                    },
                    data: serde_json::json!({ "name": args.name }),
                })
            }

            // The caller above only routes the five arms handled here.
            other => Err(AlephError::config(format!(
                "{other:?} is not a marketplace action"
            ))),
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
            PluginAction::Trust,
            PluginAction::Untrust,
        ] {
            let args = PluginManageArgs {
                action,
                name: None,
                config: None,
                enforce: None,
                source: None,
                query: None,
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
            enforce: None,
            source: None,
            query: None,
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

    /// `marketplace_remove` addresses one registration by name, so a missing
    /// one must refuse rather than call `remove("")` — which `removal_refusal`
    /// would reject anyway, but with a message about path separators rather
    /// than about the argument the caller left out.
    #[test]
    fn marketplace_remove_needs_a_name() {
        let args = PluginManageArgs {
            action: PluginAction::MarketplaceRemove,
            name: None,
            config: None,
            enforce: None,
            source: None,
            query: None,
        };
        assert!(PluginManageTool::require_name(&args).is_err());
    }

    /// `marketplace_add` is the one action keyed on `source`, and omitting it
    /// has to say which argument is missing.
    #[test]
    fn marketplace_add_without_a_source_says_which_argument_is_missing() {
        let err = PluginManageTool::marketplace(PluginManageArgs {
            action: PluginAction::MarketplaceAdd,
            name: None,
            config: None,
            enforce: None,
            source: None,
            query: None,
        })
        .expect_err("a source-less add must refuse");
        assert!(
            err.to_string().contains("'source' is required"),
            "got {err}"
        );
    }

    /// `marketplace_browse` was recorded as deliberately-not-done on
    /// 2026-08-19 (FEATURE_LOCATOR §5.24), on the grounds that handing the
    /// model a catalogue it is forbidden to act on inverts "a list that comes
    /// with an action invitation must have every row be actionable by it".
    /// The action shipped anyway, by explicit operator decision — so the
    /// objection has to be discharged rather than outvoted, and this is where
    /// it is: no field on a browse row may name an action *this* tool offers.
    /// The wire row's `installable` describes the Panel's Install button, and
    /// under that name on this surface it is exactly the false invitation the
    /// record warned about.
    #[test]
    fn a_browse_row_names_the_actor_who_can_install() {
        let src = include_str!("plugin_manage.rs");
        let body = src
            .replace('\r', "")
            .split("#[cfg(test)]")
            .next()
            .unwrap()
            .to_string();
        assert!(
            body.contains("\"operator_can_install\""),
            "the browse projection must say whose install it is talking about"
        );
        assert!(
            !body.contains("\"installable\""),
            "a bare `installable` on this surface reads as something this tool can do"
        );
    }

    /// Adding a catalogue and installing from it are different acts, and the
    /// model reads only the description. If it ever collapses them, the
    /// disclaimer above becomes a contradiction rather than a boundary.
    #[test]
    fn the_description_separates_registering_a_catalogue_from_installing() {
        let d = PluginManageTool::DESCRIPTION;
        assert!(
            d.contains("marketplace_add") && d.contains("marketplace_browse"),
            "the marketplace actions must be reachable from the description"
        );
        assert!(
            d.contains("never runs anything from it"),
            "the description must say that registering does not execute anything, or the \
             `cannot install` line reads as a contradiction"
        );
    }

    /// Every action this enum declares must be routed. A new arm that falls
    /// through to the marketplace helper's catch-all would answer "not a
    /// marketplace action" to a question about plugins.
    #[test]
    fn every_marketplace_action_routes_into_the_marketplace_half() {
        for action in [
            PluginAction::MarketplaceList,
            PluginAction::MarketplaceBrowse,
            PluginAction::MarketplaceAdd,
            PluginAction::MarketplaceRemove,
            PluginAction::MarketplaceUpdate,
        ] {
            let name = serde_json::to_value(action).unwrap();
            assert!(
                name.as_str().is_some_and(|n| n.starts_with("marketplace_")),
                "{action:?} serialises as {name} — the wire name must say which half it is in"
            );
        }
    }
}
