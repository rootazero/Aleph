//! Plugin handler parameter types

use serde::Deserialize;

use aleph_protocol::plugins::{MarketplacePluginRow, PluginRow, PluginRuntimeStatus};

// The wire shapes below are **not** redefined here. They live in
// `aleph_protocol::plugins` because `aleph-cli` — which cannot depend on
// `alephcore` — is the other half of each contract. Aliases keep the handler
// call sites reading naturally while a rename stays a compile error on both
// sides; hand-copying these five shapes is what made `aleph plugin call` fail
// on every invocation it ever had.
pub use aleph_protocol::plugins::PluginCallToolParams as CallToolParams;
pub use aleph_protocol::plugins::PluginInstallParams as InstallParams;
pub use aleph_protocol::plugins::PluginNameParams as ToggleParams;
pub use aleph_protocol::plugins::PluginNameParams as UninstallParams;
pub use aleph_protocol::plugins::PluginReloadParams as ReloadPluginParams;

use crate::extension::PluginInfo;

// ============================================================================
// Plugin Info JSON
// ============================================================================

/// Build the wire row for one plugin.
///
/// The row type is [`aleph_protocol::plugins::PluginRow`], shared with every
/// client. Constructing it here (rather than serialising an ad-hoc struct and
/// letting a test parse it back) is what makes over-sending impossible: serde
/// ignores unknown keys on the way in, so a parse-only reconciliation test is
/// structurally blind to extra fields on the wire. This function is the only
/// place a `PluginInfo` becomes a row.
#[must_use]
pub fn plugin_row(info: PluginInfo) -> PluginRow {
    PluginRow {
        name: info.name,
        version: info.version.unwrap_or_default(),
        description: info.description.unwrap_or_default(),
        enabled: info.enabled,
        path: info.path,
        kind: info.kind,
        status: parse_status(&info.status),
        status_detail: info.error,
        skills_count: info.skills_count as u32,
        commands_count: info.commands_count as u32,
        agents_count: info.agents_count as u32,
        hooks_count: info.hooks_count as u32,
        mcp_servers_count: info.mcp_servers_count as u32,
        tools_count: info.tools_count as u32,
        // Filled by the list handler, which is where the usage report lives.
        usage: None,
    }
}

/// Map the core's status label onto the wire vocabulary.
///
/// An unrecognised label falls back to `Error` with the raw label preserved in
/// `status_detail` by the caller — **not** to `Loaded`. A status we cannot read
/// must never be reported as healthy; that is the same rule the doctor checks
/// follow ("unknown may not be read as healthy").
fn parse_status(label: &str) -> PluginRuntimeStatus {
    match label {
        "loaded" => PluginRuntimeStatus::Loaded,
        "disabled" => PluginRuntimeStatus::Disabled,
        "overridden" => PluginRuntimeStatus::Overridden,
        "blocked" => PluginRuntimeStatus::Blocked,
        _ => PluginRuntimeStatus::Error,
    }
}

// ============================================================================
// Install Parameters
// ============================================================================

/// Parameters for plugins.installFromZip
#[derive(Debug, Deserialize)]
pub struct InstallFromZipParams {
    /// Base64-encoded zip data
    pub data: String,
}

// ============================================================================
// Uninstall Parameters
// ============================================================================

// ============================================================================
// Enable/Disable Parameters
// ============================================================================

// ============================================================================
// Call Tool Parameters
// ============================================================================

// ============================================================================
// Load/Unload Parameters
// ============================================================================

/// Parameters for plugins.load
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadPluginParams {
    /// Path to the plugin directory (containing aleph.plugin.json or package.json with aleph field)
    pub path: String,
}

/// Parameters for plugins.unload
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnloadPluginParams {
    /// ID of the plugin to unload
    pub plugin_id: String,
}

// ============================================================================
// Marketplace Parameters — shapes live in `aleph_protocol::plugins`
// ============================================================================

/// Build the wire row for one marketplace entry.
///
/// `installable` comes from [`PluginSearchResult::installable_path`] — the same
/// call `install_to_scope` makes — rather than from re-reading the source enum
/// here. Two readings drift, and the direction they drift in is a catalogue
/// that offers an Install button the install call then refuses.
#[must_use]
pub fn marketplace_row(
    entry: &crate::extension::marketplace::PluginSearchResult,
) -> MarketplacePluginRow {
    let (installable, unavailable_reason) = match entry.installable_path() {
        Ok(_) => (true, None),
        Err(reason) => (false, Some(reason)),
    };
    MarketplacePluginRow {
        name: entry.plugin.name.clone(),
        marketplace: entry.marketplace_name.clone(),
        description: entry.plugin.description.clone().unwrap_or_default(),
        version: entry.plugin.version.clone().unwrap_or_default(),
        installable,
        unavailable_reason,
    }
}

/// Build the wire row for one marketplace *registration*.
///
/// The sibling above describes a plugin inside a marketplace; this one
/// describes the marketplace itself. `removable` comes from
/// [`MarketplaceManager::removal_refusal`] — the same call `remove` makes —
/// rather than from comparing `name` against `"aleph-official"` here. Two
/// readings drift, and the direction they drift in is a list that offers a
/// Remove button the remove call then refuses; the built-in marketplace is
/// always listed and is the only row a fresh install has.
#[must_use]
pub fn marketplace_registration_row(
    name: &str,
    config: &crate::extension::marketplace::types::MarketplaceConfig,
) -> aleph_protocol::plugins::MarketplaceRow {
    use crate::extension::marketplace::types::MarketplaceSourceType;

    let source_type = match config.source_type {
        MarketplaceSourceType::Local => "local",
        MarketplaceSourceType::Github => "github",
    };
    let refusal = crate::extension::marketplace::MarketplaceManager::removal_refusal(name);
    aleph_protocol::plugins::MarketplaceRow {
        name: name.to_string(),
        source: config.source.clone(),
        source_type: source_type.to_string(),
        removable: refusal.is_none(),
        unremovable_reason: refusal,
    }
}

pub use aleph_protocol::plugins::MarketplaceAddParams;
pub use aleph_protocol::plugins::MarketplaceBrowseParams;
pub use aleph_protocol::plugins::MarketplaceInstallParams;
pub use aleph_protocol::plugins::MarketplaceRemoveParams;
pub use aleph_protocol::plugins::MarketplaceUpdateParams;
pub use aleph_protocol::plugins::PluginUpdateParams as UpdatePluginParams;

// ============================================================================
// Execute Command Parameters
// ============================================================================

/// Parameters for plugins.executeCommand
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteCommandParams {
    /// ID of the plugin providing the command
    pub plugin_id: String,
    /// Name of the command to execute
    pub command_name: String,
    /// Arguments to pass to the command handler
    #[serde(default)]
    pub args: serde_json::Value,
}
