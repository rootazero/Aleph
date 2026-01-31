//! Hot reload planning based on config changes.
//!
//! This module analyzes configuration changes and generates a reload plan
//! that specifies which components need to be restarted or reconfigured.
//! This enables efficient hot-reloading where only affected components
//! are restarted rather than the entire application.

use std::collections::HashSet;

/// Plan for handling configuration changes.
///
/// This struct describes what actions need to be taken when configuration
/// changes are detected. Components can use this to determine whether they
/// need to restart or can hot-update their settings.
#[derive(Debug, Clone, Default)]
pub struct ReloadPlan {
    /// Requires full Gateway restart.
    ///
    /// Set to true when changes affect core Gateway settings like port,
    /// bind address, or protocol settings that cannot be hot-reloaded.
    pub restart_gateway: bool,

    /// Channels that need restart.
    ///
    /// Contains channel identifiers (e.g., "telegram", "discord") that
    /// need to be restarted due to configuration changes.
    pub restart_channels: HashSet<String>,

    /// Whether to reload hooks.
    ///
    /// Set to true when hook configurations have changed and the
    /// hook executor needs to reload its registered hooks.
    pub reload_hooks: bool,

    /// Whether to restart cron.
    ///
    /// Set to true when cron job configurations have changed and
    /// the scheduler needs to be restarted.
    pub restart_cron: bool,

    /// Paths that can be hot-updated without restart.
    ///
    /// These are configuration paths that can be applied dynamically
    /// without requiring any component restarts.
    pub hot_paths: Vec<String>,
}

impl ReloadPlan {
    /// Check if any restart is required.
    ///
    /// Returns true if the Gateway needs to restart or if any channels
    /// need to be restarted.
    pub fn requires_restart(&self) -> bool {
        self.restart_gateway || !self.restart_channels.is_empty()
    }

    /// Check if the plan is empty (no changes).
    ///
    /// Returns true if no actions need to be taken.
    pub fn is_empty(&self) -> bool {
        !self.restart_gateway
            && self.restart_channels.is_empty()
            && !self.reload_hooks
            && !self.restart_cron
            && self.hot_paths.is_empty()
    }

    /// Get a human-readable summary of the plan.
    pub fn summary(&self) -> String {
        if self.is_empty() {
            return "No changes detected".to_string();
        }

        let mut parts = Vec::new();

        if self.restart_gateway {
            parts.push("Gateway restart required".to_string());
        }

        if !self.restart_channels.is_empty() {
            let channels: Vec<_> = self.restart_channels.iter().map(|s| s.as_str()).collect();
            parts.push(format!("Restart channels: {}", channels.join(", ")));
        }

        if self.reload_hooks {
            parts.push("Reload hooks".to_string());
        }

        if self.restart_cron {
            parts.push("Restart cron".to_string());
        }

        if !self.hot_paths.is_empty() {
            parts.push(format!("Hot-update {} paths", self.hot_paths.len()));
        }

        parts.join("; ")
    }
}

/// Build a reload plan from changed configuration paths.
///
/// This function analyzes the list of changed configuration paths and
/// determines what actions need to be taken to apply those changes.
///
/// # Arguments
///
/// * `changed_paths` - List of dot-separated paths that have changed
///
/// # Examples
///
/// ```rust,ignore
/// use aethecore::config::{build_reload_plan, diff_config};
///
/// let changes = diff_config(&prev_config, &next_config);
/// let plan = build_reload_plan(&changes);
///
/// if plan.requires_restart() {
///     println!("Restart required: {}", plan.summary());
/// } else {
///     println!("Hot-reloading: {}", plan.summary());
/// }
/// ```
pub fn build_reload_plan(changed_paths: &[String]) -> ReloadPlan {
    let mut plan = ReloadPlan::default();

    for path in changed_paths {
        classify_change(path, &mut plan);
    }

    plan
}

/// Classify a single configuration change and update the reload plan.
fn classify_change(path: &str, plan: &mut ReloadPlan) {
    // Gateway core settings require full restart
    if path.starts_with("gateway.") {
        plan.restart_gateway = true;
        return;
    }

    // Plugin and MCP changes require full restart
    if path.starts_with("plugins") || path.starts_with("mcp.") {
        plan.restart_gateway = true;
        return;
    }

    // Security settings require full restart
    if path.starts_with("security.") {
        plan.restart_gateway = true;
        return;
    }

    // Channel changes require that specific channel to restart
    if path.starts_with("channels.") {
        if let Some(channel) = path.strip_prefix("channels.") {
            let channel_name = channel.split('.').next().unwrap_or(channel);
            plan.restart_channels.insert(channel_name.to_string());
        }
        return;
    }

    // Hook changes require hook reload
    if path.starts_with("hooks") {
        plan.reload_hooks = true;
        return;
    }

    // Cron changes require cron restart
    if path.starts_with("cron") {
        plan.restart_cron = true;
        return;
    }

    // Everything else can be hot-updated
    plan.hot_paths.push(path.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_changes() {
        let plan = build_reload_plan(&[]);
        assert!(plan.is_empty());
        assert!(!plan.requires_restart());
    }

    #[test]
    fn test_gateway_restart() {
        let plan = build_reload_plan(&["gateway.port".to_string()]);
        assert!(plan.restart_gateway);
        assert!(plan.requires_restart());
    }

    #[test]
    fn test_channel_restart() {
        let plan = build_reload_plan(&["channels.telegram.token".to_string()]);
        assert!(plan.restart_channels.contains("telegram"));
        assert!(!plan.restart_gateway);
        assert!(plan.requires_restart());
    }

    #[test]
    fn test_multiple_channels() {
        let plan = build_reload_plan(&[
            "channels.telegram.token".to_string(),
            "channels.discord.bot_id".to_string(),
        ]);
        assert!(plan.restart_channels.contains("telegram"));
        assert!(plan.restart_channels.contains("discord"));
        assert_eq!(plan.restart_channels.len(), 2);
    }

    #[test]
    fn test_hot_update() {
        let plan = build_reload_plan(&["providers.openai.model".to_string()]);
        assert!(!plan.restart_gateway);
        assert!(!plan.requires_restart());
        assert!(plan.hot_paths.contains(&"providers.openai.model".to_string()));
    }

    #[test]
    fn test_hooks_reload() {
        let plan = build_reload_plan(&["hooks.email.enabled".to_string()]);
        assert!(plan.reload_hooks);
        assert!(!plan.restart_gateway);
    }

    #[test]
    fn test_cron_restart() {
        let plan = build_reload_plan(&["cron.daily.schedule".to_string()]);
        assert!(plan.restart_cron);
        assert!(!plan.restart_gateway);
    }

    #[test]
    fn test_mcp_restart() {
        let plan = build_reload_plan(&["mcp.filesystem.command".to_string()]);
        assert!(plan.restart_gateway);
    }

    #[test]
    fn test_plugins_restart() {
        let plan = build_reload_plan(&["plugins.my-plugin.enabled".to_string()]);
        assert!(plan.restart_gateway);
    }

    #[test]
    fn test_security_restart() {
        let plan = build_reload_plan(&["security.require_auth".to_string()]);
        assert!(plan.restart_gateway);
    }

    #[test]
    fn test_mixed_changes() {
        let plan = build_reload_plan(&[
            "gateway.port".to_string(),
            "channels.telegram.token".to_string(),
            "providers.openai.model".to_string(),
        ]);

        assert!(plan.restart_gateway);
        assert!(plan.restart_channels.contains("telegram"));
        assert!(plan.hot_paths.contains(&"providers.openai.model".to_string()));
    }

    #[test]
    fn test_summary() {
        let plan = build_reload_plan(&[]);
        assert_eq!(plan.summary(), "No changes detected");

        let plan = build_reload_plan(&["gateway.port".to_string()]);
        assert!(plan.summary().contains("Gateway restart required"));

        let plan = build_reload_plan(&["channels.telegram.token".to_string()]);
        assert!(plan.summary().contains("telegram"));
    }
}
