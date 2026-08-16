//! Plugin RPC contract — the `plugin.*` / `plugins.*` families.
//!
//! # Why these types live here
//!
//! This is the **fourth** recurrence in this repo of one failure shape: a wire
//! contract whose two halves live in two crates, written twice by hand, and
//! disagreeing. `workspace.rs` documents the first (`workspace create|archive`,
//! `INVALID_PARAMS` on every call ever made), `session_thread.rs` the second
//! (the TUI sending `message` where `agent.run` wanted `input`), `providers/`
//! the third (`providers add|test` flat where the handler wanted an envelope).
//!
//! The plugin family had all three sub-species at once:
//!
//! | call | CLI sent / read | handler wanted / sent | effect |
//! |---|---|---|---|
//! | `plugins.callTool` | `{plugin, tool, params}` | `{pluginId, handler, args}` | **every `aleph plugin call` was `INVALID_PARAMS`** |
//! | `plugins.list` (info) | `result.as_array()` | `{"plugins": [...]}` | **`aleph plugin info` always "not found"** |
//! | `plugins.list` (list) | row key `type` | no such key | Type column always `-` |
//! | `plugins.list` (info) | row keys `tools`, `hooks` | `tools_count`, `hooks_count` | both always `0` |
//!
//! None of it went red, because the CLI's only two tests compared string
//! literals with themselves (`assert!("my-plugin.zip".ends_with(".zip"))` tests
//! the standard library, not the wire).
//!
//! # The rule this module encodes
//!
//! Sharing one type turns a rename into a **compile** error on both sides.
//! `aleph-cli` deliberately cannot depend on `alephcore` — it doubles as the
//! reference protocol implementation — so this crate, which both depend on, is
//! the only place a shared shape can live.
//!
//! Responses are **constructed** from these types server-side rather than
//! merely parsed into them by a test. Parsing only ever proves a superset:
//! serde ignores unknown keys, so a reconciliation test that deserializes a real
//! response is structurally blind to whatever *else* is on the wire. Building
//! the response from the contract type makes over-sending impossible instead of
//! merely untested.

use serde::{Deserialize, Serialize};

// =============================================================================
// plugins.list
// =============================================================================

/// One row of `plugins.list`.
///
/// Field names here are the wire names. Renaming one breaks every renderer at
/// compile time, which is the entire point.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PluginRow {
    /// Registry id. Also the key `uninstall` / `enable` / `disable` take, so a
    /// list that shows something else shows an id the caller cannot act on.
    pub name: String,

    /// Declared version, empty when the manifest omits it.
    #[serde(default)]
    pub version: String,

    #[serde(default)]
    pub description: String,

    /// `true` only for [`PluginRuntimeStatus::Loaded`]. Kept alongside
    /// `status` because a toggle needs a boolean and a human needs the reason.
    #[serde(default)]
    pub enabled: bool,

    /// Install directory.
    #[serde(default)]
    pub path: String,

    /// Runtime kind — `"static"`, `"mcp"` or `"wasm"`.
    ///
    /// The CLI rendered a `Type` column from a key by this name for as long as
    /// the column existed; the server never sent one. The column was not
    /// wrong to want it, so the field is added here rather than the column
    /// removed.
    #[serde(default)]
    pub kind: String,

    /// Why this plugin is or is not active — see [`PluginRuntimeStatus`].
    #[serde(default)]
    pub status: PluginRuntimeStatus,

    /// Human-readable detail for a non-`Loaded` status: the parse error, the
    /// path that shadowed this one, the policy that refused it.
    ///
    /// A status without a detail tells the operator that something is wrong
    /// and nothing about what to do, which is the half that matters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_detail: Option<String>,

    #[serde(default)]
    pub skills_count: u32,
    #[serde(default)]
    pub commands_count: u32,
    #[serde(default)]
    pub agents_count: u32,
    #[serde(default)]
    pub hooks_count: u32,
    #[serde(default)]
    pub mcp_servers_count: u32,

    /// Tools this plugin contributes. Zero means usage is **not measurable**
    /// for it (its `usage.calls` will be `None`), not that it is unused — a
    /// renderer needs both numbers to say the right thing.
    #[serde(default)]
    pub tools_count: u32,

    /// Invocation record. `None` when talking to a server that predates the
    /// field — an empty cell, not a zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<crate::extension_usage::UsageSummary>,
}

/// Why a discovered plugin is, or is not, active.
///
/// Before this contract the server could only ever say `loaded` or `disabled`:
/// `Overridden` and `Error` existed as enum variants with **zero producers**,
/// and a plugin that failed to parse, lost a shadow contest, or was refused by
/// the owner-trust policy was simply dropped from the registry. "Installed but
/// broken" and "never installed" were byte-for-byte the same answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginRuntimeStatus {
    /// Active: capabilities registered and visible to the model.
    #[default]
    Loaded,
    /// The operator turned it off (`<data_dir>/plugins.toml`). Registered and
    /// listable so it can be turned back on, but invisible to the model.
    Disabled,
    /// A same-id plugin from a higher-priority scope won. `status_detail`
    /// carries the winning path.
    Overridden,
    /// The manifest could not be parsed. `status_detail` carries the error.
    Error,
    /// The owner-trust policy refused this origin. Distinct from `Disabled`:
    /// the remedy is an allowlist entry, not a toggle.
    Blocked,
}

impl PluginRuntimeStatus {
    /// Stable lowercase label, matching the serde representation.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Loaded => "loaded",
            Self::Disabled => "disabled",
            Self::Overridden => "overridden",
            Self::Error => "error",
            Self::Blocked => "blocked",
        }
    }

    /// Whether the plugin's capabilities are live.
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Loaded)
    }
}

/// Response envelope of `plugins.list` / `plugin.list`.
///
/// The envelope is part of the wire contract, and historically it was the last
/// part left hand-copied: two CLI functions in the same file disagreed about
/// whether the response was `{"plugins": [...]}` or a bare array, and the one
/// that guessed "bare array" reported every plugin as missing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PluginListResult {
    pub plugins: Vec<PluginRow>,
}

// =============================================================================
// plugins.callTool
// =============================================================================

/// Parameters for `plugins.callTool`.
///
/// `handler` is the exported function name, not the model-facing tool name —
/// the CLI used to send a `tool` key holding the latter, which is both the
/// wrong key and the wrong value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCallToolParams {
    /// Registry id of the plugin owning the handler.
    pub plugin_id: String,
    /// Exported handler function to invoke.
    pub handler: String,
    /// Arguments passed through to the handler.
    #[serde(default)]
    pub args: serde_json::Value,
}

// =============================================================================
// Simple by-name parameter families
// =============================================================================

/// Parameters for every `plugins.{uninstall,enable,disable}` and
/// `plugin.update` call: a single plugin id under the key `name`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginNameParams {
    pub name: String,
}

/// Parameters for `plugin.reload`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginReloadParams {
    pub plugin_id: String,
}

/// Parameters for `plugins.install` (a git URL).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginInstallParams {
    pub url: String,
}

/// Parameters for `plugin.install` — the unified entry point that classifies
/// `source` as a marketplace name or a git URL server-side (R4: the shell does
/// not decide).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginInstallUnifiedParams {
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

// =============================================================================
// plugin.marketplace.*
// =============================================================================

/// Parameters for `plugin.marketplace.add`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplaceAddParams {
    /// `"owner/repo"` for GitHub, a filesystem path for local.
    pub source: String,
    /// Explicit name; derived from `source` when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Parameters for `plugin.marketplace.remove`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplaceRemoveParams {
    pub name: String,
}

/// Parameters for `plugin.marketplace.update`. `None` updates every cache.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplaceUpdateParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Parameters for `plugin.marketplace.install`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplaceInstallParams {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marketplace: Option<String>,
    /// `"user"` (default), `"project"` or `"local"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// Parameters for `plugin.update` — upgrade an installed plugin in place.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginUpdateParams {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marketplace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Re-install even when the version is unchanged.
    #[serde(default)]
    pub force: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact payload `aleph plugin call` must produce. Written as a
    /// round-trip through the contract type rather than a JSON literal, so it
    /// cannot drift into asserting itself.
    #[test]
    fn call_tool_params_use_camel_case_on_the_wire() {
        let params = PluginCallToolParams {
            plugin_id: "diagnostics".into(),
            handler: "system_health".into(),
            args: serde_json::json!({"verbose": true}),
        };
        let wire = serde_json::to_value(&params).unwrap();
        // Key *set*, not order: serde_json's map is sorted, and the wire
        // contract is about which names exist, not their sequence.
        let mut keys: Vec<&str> = wire
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["args", "handler", "pluginId"],
            "the CLI used to send plugin/tool/params — three wrong names"
        );
    }

    #[test]
    fn the_list_envelope_is_an_object_not_a_bare_array() {
        let wire = serde_json::to_value(PluginListResult::default()).unwrap();
        assert!(
            wire.get("plugins").is_some(),
            "`aleph plugin info` read this as a bare array and found nothing, ever"
        );
    }

    #[test]
    fn status_labels_match_their_serde_representation() {
        for status in [
            PluginRuntimeStatus::Loaded,
            PluginRuntimeStatus::Disabled,
            PluginRuntimeStatus::Overridden,
            PluginRuntimeStatus::Error,
            PluginRuntimeStatus::Blocked,
        ] {
            let wire = serde_json::to_value(status).unwrap();
            assert_eq!(
                wire.as_str().unwrap(),
                status.label(),
                "a client rendering `label()` and a client reading the JSON \
                 must see the same word"
            );
        }
    }

    #[test]
    fn only_loaded_is_active() {
        assert!(PluginRuntimeStatus::Loaded.is_active());
        for inactive in [
            PluginRuntimeStatus::Disabled,
            PluginRuntimeStatus::Overridden,
            PluginRuntimeStatus::Error,
            PluginRuntimeStatus::Blocked,
        ] {
            assert!(!inactive.is_active(), "{inactive:?} must not be active");
        }
    }

    /// An older server sends none of the newer keys; the row must still parse.
    #[test]
    fn a_minimal_row_from_an_older_server_still_parses() {
        let row: PluginRow = serde_json::from_value(serde_json::json!({
            "name": "legacy",
        }))
        .unwrap();
        assert_eq!(row.name, "legacy");
        assert_eq!(row.status, PluginRuntimeStatus::Loaded);
        assert!(row.usage.is_none());
        assert!(row.status_detail.is_none());
    }
}
