//! Panel API for global tool permission management (config.* RPC calls).
//!
//! The single decoder for `config.get_tool_permissions` — Settings → Policies
//! edits the advanced axes, the composer's `ExecTierPicker` reads the tier and
//! its presets. One wire shape, one DTO: two hand-written decoders would drift.

use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;

// -- Types --

/// One selectable position of a session dial.
///
/// Core ships the id set and its order (`builtin_tiers` / `builtin_modes` /
/// `builtin_think_levels` / `builtin_memory_modes`) — that is the part every
/// surface must agree on. The copy is this surface's own, resolved per locale
/// by `components::*_labels` (R4/R6).
///
/// One type for all four dials: `{ id }` is the whole contract, and four
/// identical structs would be four places for it to drift.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DialPreset {
    pub id: String,
}

/// One selectable execution-permission tier.
pub type TierPreset = DialPreset;

/// One selectable session usage mode.
pub type ModePreset = DialPreset;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPermissionsResponse {
    /// Active tier id (`ask` / `auto` / `full`).
    pub exec_tier: String,
    /// Selectable INSTALL tiers, ordered least → most permissive. What
    /// Settings → Policies offers as the machine-wide default.
    #[serde(default)]
    pub tiers: Vec<TierPreset>,
    /// Selectable tiers for a single CONVERSATION — `tiers` plus `plan`, the
    /// read-only planning posture that ends when a human approves a plan. What
    /// the composer's tier pill offers.
    ///
    /// Empty against a core that predates the split; [`Self::session_tier_presets`]
    /// falls back to `tiers` there, so an older core keeps offering three
    /// choices rather than none.
    #[serde(default)]
    pub session_tiers: Vec<TierPreset>,
    /// Global default usage mode id (`chat` / `work` / `code`). Defaulted so
    /// the decoder tolerates an older core that predates the mode dial.
    #[serde(default)]
    pub mode: String,
    /// Selectable modes, in display order.
    #[serde(default)]
    pub modes: Vec<ModePreset>,
    /// The reasoning-depth ladder, shallow → deep.
    ///
    /// There is deliberately **no** `think_level` beside it: core resolves
    /// depth as request > session > *no directive at all*, so there is no
    /// global position to report. A pill must render its clear-the-override row
    /// as "provider default", never as "follow global" — the latter names a
    /// setting that does not exist. Empty against a core that predates the
    /// dial, which hides the pill.
    #[serde(default)]
    pub think_levels: Vec<DialPreset>,
    /// Global memory-injection position (`on` / `off`), i.e. where
    /// `[memory] enabled` sits. Empty against an older core.
    #[serde(default)]
    pub memory: String,
    /// Selectable memory modes, in display order.
    #[serde(default)]
    pub memory_modes: Vec<DialPreset>,
    /// Server-global default tool permission — one of the two advanced axes
    /// Settings → Policies edits.
    ///
    /// **Defaulted because the server deliberately omits it for a member.**
    /// `config::member_visible_permissions_value` narrows the response BY
    /// REMOVAL (`obj.remove("default"); obj.remove("overrides")`) and pins that
    /// removal with its own test. Without `#[serde(default)]` this decoder —
    /// the only one, by design — failed the whole payload with
    /// "missing field `default`" for every member, so the carve-out that was
    /// supposed to hand members the tier and mode ids shipped 100% inert: the
    /// mode pill vanished and the tier popover degraded to a single blank row.
    ///
    /// Worse, the error is not refusal-shaped, so `is_admin_refusal` was false
    /// too and the surface could not even say what had happened. Two halves of
    /// the same round cancelled each other, and both test suites stayed green
    /// because each side only ever read its own literal.
    #[serde(default)]
    pub default: String,
    #[serde(default)]
    pub overrides: HashMap<String, String>,
}

impl ToolPermissionsResponse {
    /// The tiers a composer pill may offer for THIS conversation.
    ///
    /// Falls back to the install list against a core that predates the split.
    /// A bare `session_tiers` read would have degraded that case to an empty
    /// popover — the exact symptom `a_member_still_receives_both_dials…`
    /// exists to prevent, arriving through a version skew instead of a
    /// permission narrowing.
    #[must_use]
    pub fn session_tier_presets(&self) -> &[TierPreset] {
        if self.session_tiers.is_empty() {
            &self.tiers
        } else {
            &self.session_tiers
        }
    }
}

// -- API --

pub struct ToolPermissionsApi;

impl ToolPermissionsApi {
    // Global (Policies) API

    pub async fn get_global(state: &DashboardState) -> Result<ToolPermissionsResponse, String> {
        let result = state
            .rpc_call("config.get_tool_permissions", Value::Null)
            .await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    /// Set the execution tier. Partial update — the advanced per-tool overrides
    /// are left untouched. Returns the whole permission surface so the caller
    /// re-renders straight from Core.
    pub async fn set_exec_tier(
        state: &DashboardState,
        tier_id: &str,
    ) -> Result<ToolPermissionsResponse, String> {
        let result = state
            .rpc_call(
                "config.update_tool_permissions",
                json!({ "exec_tier": tier_id }),
            )
            .await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    /// Set the global default usage mode. Partial update, mirror of
    /// [`Self::set_exec_tier`] for the mode twin.
    pub async fn set_mode(
        state: &DashboardState,
        mode_id: &str,
    ) -> Result<ToolPermissionsResponse, String> {
        let result = state
            .rpc_call("config.update_tool_permissions", json!({ "mode": mode_id }))
            .await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn update_global(
        state: &DashboardState,
        default: &str,
        overrides: &HashMap<String, String>,
    ) -> Result<(), String> {
        let params = json!({
            "default": default,
            "overrides": overrides,
        });
        state
            .rpc_call("config.update_tool_permissions", params)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_deserializes_from_the_rpc_shape() {
        let v = json!({
            "exec_tier": "auto",
            "tiers": [{ "id": "ask" }],
            "default": "allow",
            "overrides": { "bash": "ask" },
        });
        let cfg: ToolPermissionsResponse = serde_json::from_value(v).unwrap();
        assert_eq!(cfg.exec_tier, "auto");
        // An older core without the mode dial must still decode.
        assert_eq!(cfg.mode, "");
        assert!(cfg.modes.is_empty());
        // …and likewise for the two dials added after it. Each pill hides
        // itself on an empty enumeration rather than rendering a blank label.
        assert!(cfg.think_levels.is_empty());
        assert_eq!(cfg.memory, "");
        assert!(cfg.memory_modes.is_empty());
        assert_eq!(cfg.tiers.len(), 1);
        assert_eq!(cfg.tiers[0].id, "ask");
        assert_eq!(cfg.default, "allow");
        assert_eq!(cfg.overrides.get("bash").map(String::as_str), Some("ask"));
    }

    /// The composer pill reads the SESSION list; Settings → Policies reads the
    /// install list. A core that predates the split ships only the latter, and
    /// the pill must degrade to three choices rather than to an empty popover —
    /// the same symptom `the_member_shape_from_the_shared_contract_decodes`
    /// exists to prevent, arriving through a version skew instead.
    #[test]
    fn the_session_tier_list_falls_back_to_the_install_list() {
        let old_core = json!({
            "exec_tier": "auto",
            "tiers": [{ "id": "ask" }, { "id": "auto" }, { "id": "full" }],
        });
        let cfg: ToolPermissionsResponse = serde_json::from_value(old_core).unwrap();
        assert!(cfg.session_tiers.is_empty());
        assert_eq!(cfg.session_tier_presets().len(), 3);

        let new_core = json!({
            "exec_tier": "auto",
            "tiers": [{ "id": "ask" }, { "id": "auto" }, { "id": "full" }],
            "session_tiers": [
                { "id": "plan" }, { "id": "ask" }, { "id": "auto" }, { "id": "full" }
            ],
        });
        let cfg: ToolPermissionsResponse = serde_json::from_value(new_core).unwrap();
        assert_eq!(cfg.session_tier_presets().len(), 4);
        assert_eq!(cfg.session_tier_presets()[0].id, "plan");
    }

    #[test]
    fn response_decodes_the_mode_dial() {
        let v = json!({
            "exec_tier": "auto",
            "tiers": [{ "id": "ask" }],
            "mode": "work",
            "modes": [{ "id": "chat" }, { "id": "work" }, { "id": "code" }],
            "default": "allow",
            "overrides": {},
        });
        let cfg: ToolPermissionsResponse = serde_json::from_value(v).unwrap();
        assert_eq!(cfg.mode, "work");
        assert_eq!(cfg.modes.len(), 3);
        assert_eq!(cfg.modes[2].id, "code");
    }

    /// The member shape, built from the shared contract rather than from a
    /// literal typed here.
    ///
    /// A hand-written literal is what let this break: the server narrowed by
    /// removing `default` and `overrides`, this DTO required `default`, and
    /// every member's fetch failed the whole decode with "missing field
    /// `default`". Both test suites were green — each read only its own copy of
    /// the shape. Building the object from
    /// `aleph_protocol::tool_permissions::MEMBER_VISIBLE_KEYS` means the server
    /// dropping a key from that list, or a new required field appearing here,
    /// fails this test by name.
    ///
    /// Note what a decode failure costs: it is not "one field is missing", it
    /// is the whole payload — the tier popover degrades to a single blank row
    /// and the mode pill hides itself, with no console trace, because a serde
    /// message is not refusal-shaped either.
    #[test]
    fn the_member_shape_from_the_shared_contract_decodes() {
        use aleph_protocol::tool_permissions::{MEMBER_VISIBLE_KEYS, OPERATOR_ONLY_KEYS};

        let mut obj = serde_json::Map::new();
        for key in MEMBER_VISIBLE_KEYS {
            // Shape per key: the two dials are ids, the two enumerations are
            // arrays of `{id}`.
            let value = match *key {
                // Enumerations: arrays of `{ id }`.
                "tiers" | "session_tiers" | "modes" | "think_levels" | "memory_modes" => {
                    json!([{ "id": "ask" }])
                }
                // Dial positions: a bare id.
                "exec_tier" | "mode" | "memory" => json!("auto"),
                // No catch-all on purpose. A new member-visible key has to be
                // given its shape here, because the wrong shape is what this
                // whole test exists to catch — and a silent `json!("auto")`
                // fallback would hand an array-typed field a string and then
                // pass, which is the failure one level up.
                other => panic!(
                    "`{other}` joined MEMBER_VISIBLE_KEYS without a shape here — say whether                      it is a dial position or an enumeration"
                ),
            };
            obj.insert((*key).to_string(), value);
        }
        for key in OPERATOR_ONLY_KEYS {
            assert!(
                !obj.contains_key(*key),
                "{key} must not be in the member shape — the contract says it is withheld"
            );
        }

        let cfg: ToolPermissionsResponse = serde_json::from_value(Value::Object(obj)).expect(
            "the member shape must decode — a missing withheld key fails the WHOLE payload",
        );
        assert_eq!(cfg.exec_tier, "auto");
        assert_eq!(cfg.tiers.len(), 1);
        assert_eq!(cfg.modes.len(), 1);
        assert_eq!(cfg.think_levels.len(), 1);
        assert_eq!(cfg.memory_modes.len(), 1);
        // The withheld axes land on their defaults rather than killing the decode.
        assert_eq!(cfg.default, "");
        assert!(cfg.overrides.is_empty());
    }
}
