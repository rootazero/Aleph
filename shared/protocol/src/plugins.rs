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
/// The runtimes a plugin manifest may declare, in wire spelling.
///
/// This vocabulary had three holders and no owner: `PluginKind`'s serde
/// representation in the server, prose in this file's doc comment, and the
/// CLI's `aleph plugin init --type` template list. They disagreed —
/// `aleph plugin init --type nodejs` scaffolded `kind = "nodejs"`, which
/// `PluginKind` rejects with `unknown variant`, so a plugin created by Aleph's
/// own scaffolder could never be loaded by Aleph. `aleph plugin validate` said
/// it was fine and `aleph plugin pack` shipped it, because the CLI had its own
/// weaker schema.
///
/// Both sides derive from this constant now, and each holds a test that its
/// own list equals it.
pub const PLUGIN_RUNTIMES: [&str; 3] = ["wasm", "mcp", "static"];

/// Whether `runtime` is a runtime the host can actually load.
#[must_use]
pub fn is_known_plugin_runtime(runtime: &str) -> bool {
    PLUGIN_RUNTIMES.contains(&runtime)
}

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

    /// Runtime kind — one of [`PLUGIN_RUNTIMES`].
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

/// Parameters for `plugin.marketplace.browse`.
///
/// Both fields optional: no `marketplace` reads every registered one, no
/// `query` lists everything. This is the call that answers "what is in there",
/// which `plugin.marketplace.list` does not — that one lists the *registrations*
/// (name, source, type), and a caller who reads it looking for plugin names
/// finds none and concludes the marketplace is empty.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplaceBrowseParams {
    /// Restrict to one marketplace by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marketplace: Option<String>,
    /// Case-insensitive substring, matched against name and description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}

/// One row of `plugin.marketplace.browse`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplacePluginRow {
    /// The plugin id. Also the key `plugin.marketplace.install` takes.
    pub name: String,

    /// Which marketplace this row came from. Sending it back with the install
    /// call is what keeps a name that exists in two marketplaces from being
    /// refused as ambiguous.
    pub marketplace: String,

    #[serde(default)]
    pub description: String,

    #[serde(default)]
    pub version: String,

    /// Whether `plugin.marketplace.install` can actually act on this row.
    ///
    /// Derived server-side from the predicate install itself runs, not from a
    /// client's reading of the source field. A row rendered with an Install
    /// button that the install call then refuses is the failure this exists to
    /// prevent.
    pub installable: bool,

    /// Why not, when `installable` is false. Present exactly when it is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

/// A marketplace that could not be read during a browse, and why.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplaceProblemRow {
    pub marketplace: String,
    pub reason: String,
}

/// Result of `plugin.marketplace.browse`.
///
/// `problems` is not decoration. An empty `plugins` with an empty `problems`
/// means the query matched nothing; an empty `plugins` with a problem means
/// something upstream needs doing, and collapsing the two into one empty list
/// is how "run `marketplace update` first" becomes invisible.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplaceBrowseResult {
    pub plugins: Vec<MarketplacePluginRow>,
    #[serde(default)]
    pub problems: Vec<MarketplaceProblemRow>,
}

/// One row of `plugin.marketplace.list` — a *registration*, not its contents.
///
/// The sibling [`MarketplacePluginRow`] describes a plugin inside a
/// marketplace; this one describes the marketplace itself. Reading one looking
/// for the other is the confusion `MarketplaceBrowseParams` already documents.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplaceRow {
    /// Registration key. Also what `plugin.marketplace.remove` and
    /// `plugin.marketplace.update` take.
    pub name: String,

    /// `"owner/repo"` for GitHub, a filesystem path for local, and the
    /// sentinel `"bundled"` for the built-in one.
    pub source: String,

    /// `"local"` or `"github"` today.
    ///
    /// A `String` rather than an enum on purpose: serde does not degrade field
    /// by field, so a source kind added server-side would make the *whole*
    /// response unparseable for an older client rather than costing it one
    /// column. The renderers treat an unrecognised value as an opaque label.
    #[serde(rename = "type")]
    pub source_type: String,

    /// Whether `plugin.marketplace.remove` can actually act on this row.
    ///
    /// Derived server-side from the predicate `remove` itself runs, not from a
    /// client comparing `name` against a hard-coded `"aleph-official"`. The
    /// built-in marketplace is always listed and can never be removed, and on
    /// a fresh install it is the *only* row — so a Remove button on every row
    /// is an invitation that fails for the only thing on screen.
    #[serde(default = "default_true")]
    pub removable: bool,

    /// Why not, when `removable` is false. Present exactly when it is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unremovable_reason: Option<String>,
}

/// A client older than the `removable` bit must not read its absence as
/// "nothing here can be removed" — that would hide the button on every row.
const fn default_true() -> bool {
    true
}

/// Result of `plugin.marketplace.list`.
///
/// The last member of this family that had no contract type: the handler built
/// a `json!` literal and the CLI pretty-printed whatever came back, so neither
/// end could go red on a renamed key. Rows arrive sorted by name — the server
/// holds them in a `HashMap`, and an unsorted response reshuffles the Panel
/// list on every load.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplaceListResult {
    #[serde(default)]
    pub marketplaces: Vec<MarketplaceRow>,
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

    /// Every column the CLI table and the Panel row render, present on the
    /// wire under exactly these names.
    ///
    /// The failure this pins is the one `providers list` shipped for a whole
    /// release: the renderer read `type` and `default` while the server sent
    /// `provider_type` and `is_default`, so every row printed a dash. A
    /// missing key renders identically to a missing value, which is why
    /// nothing went red.
    #[test]
    fn a_browse_row_carries_every_column_its_renderers_read() {
        let wire = serde_json::to_value(MarketplacePluginRow {
            name: "alpha".into(),
            marketplace: "fixture".into(),
            description: "A calendar helper".into(),
            version: "1.0.0".into(),
            installable: true,
            unavailable_reason: None,
        })
        .unwrap();
        let mut keys: Vec<&str> = wire.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["description", "installable", "marketplace", "name", "version"],
            "renaming a column here must break both renderers at compile time, \
             not print a row of dashes"
        );
    }

    /// `unavailable_reason` is present exactly when `installable` is false.
    ///
    /// Both renderers key off this: a reason with no refusal is a warning
    /// beside a working button, and a refusal with no reason is a disabled
    /// button with nothing to say.
    #[test]
    fn a_browse_row_carries_a_reason_exactly_when_it_is_not_installable() {
        let ok = serde_json::to_value(MarketplacePluginRow {
            name: "alpha".into(),
            marketplace: "fixture".into(),
            installable: true,
            unavailable_reason: None,
            ..Default::default()
        })
        .unwrap();
        assert!(ok.get("unavailable_reason").is_none());

        let refused = serde_json::to_value(MarketplacePluginRow {
            name: "gamma".into(),
            marketplace: "fixture".into(),
            installable: false,
            unavailable_reason: Some("declares an 'npm' source".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            refused.get("unavailable_reason").and_then(|v| v.as_str()),
            Some("declares an 'npm' source")
        );
    }

    /// An omitted `params` object must be a valid "browse everything", not a
    /// deserialisation error — the CLI's bare `marketplace browse` sends it.
    #[test]
    fn browse_params_are_all_optional() {
        let empty: MarketplaceBrowseParams = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(empty, MarketplaceBrowseParams::default());
        assert_eq!(
            serde_json::to_value(MarketplaceBrowseParams::default()).unwrap(),
            serde_json::json!({}),
            "an all-None browse must not put explicit nulls on the wire"
        );
    }

    /// Empty plugins with no problems and empty plugins with a problem are
    /// two different answers, and the second one must survive the round trip.
    #[test]
    fn a_browse_result_keeps_problems_separate_from_an_empty_catalogue() {
        let quiet = serde_json::to_value(MarketplaceBrowseResult::default()).unwrap();
        assert_eq!(quiet.get("problems").and_then(|v| v.as_array()).map(Vec::len), Some(0));

        let noisy = MarketplaceBrowseResult {
            plugins: vec![],
            problems: vec![MarketplaceProblemRow {
                marketplace: "aleph-official".into(),
                reason: "not synced yet".into(),
            }],
        };
        let back: MarketplaceBrowseResult =
            serde_json::from_value(serde_json::to_value(&noisy).unwrap()).unwrap();
        assert_eq!(back, noisy);
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
    /// The same reconciliation for the *registration* row, whose columns the
    /// CLI table and the Panel's Marketplaces section both read.
    ///
    /// Key-set **equality**, not containment: parsing a real response only ever
    /// proves a superset, because serde ignores unknown keys on the way in.
    /// The expectation is spelled out because that is the wire contract; the
    /// value under each key is what the round-trip below checks.
    #[test]
    fn a_marketplace_row_carries_every_column_its_renderers_read() {
        let wire = serde_json::to_value(MarketplaceRow {
            name: "aleph-official".into(),
            source: "bundled".into(),
            source_type: "local".into(),
            removable: false,
            unremovable_reason: Some("Cannot remove built-in marketplace".into()),
        })
        .unwrap();
        let mut keys: Vec<&str> = wire
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "name",
                "removable",
                "source",
                "type",
                "unremovable_reason"
            ],
            "the handler used to build this row as a json! literal, so a \
             renamed key could not go red on either side"
        );
        // `type` is a Rust keyword, so the field is `source_type` and only the
        // rename carries it onto the wire. Dropping that attribute is a silent
        // column of dashes, which is exactly the shape this family keeps
        // repeating.
        assert_eq!(wire["type"], "local");
    }

    /// A client built before `removable` existed must not read its absence as
    /// "nothing is removable" — that hides the button on every row, including
    /// the ones that would have worked.
    #[test]
    fn a_row_without_the_removable_bit_defaults_to_removable() {
        let row: MarketplaceRow = serde_json::from_value(serde_json::json!({
            "name": "third-party",
            "source": "owner/repo",
            "type": "github",
        }))
        .expect("a pre-`removable` row still parses");
        assert!(row.removable, "an absent bit is not a refusal");
        assert!(row.unremovable_reason.is_none());
    }

    /// The envelope is a wire key too, and it is usually the last part left
    /// hand-copied. Round-tripped rather than asserted against a literal so it
    /// cannot drift into testing itself.
    #[test]
    fn the_list_envelope_round_trips_through_its_contract_type() {
        let result = MarketplaceListResult {
            marketplaces: vec![MarketplaceRow {
                name: "third-party".into(),
                source: "owner/repo".into(),
                source_type: "github".into(),
                removable: true,
                unremovable_reason: None,
            }],
        };
        let wire = serde_json::to_value(&result).unwrap();
        assert_eq!(
            wire.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["marketplaces"]
        );
        let back: MarketplaceListResult = serde_json::from_value(wire).unwrap();
        assert_eq!(back, result);
    }
}
