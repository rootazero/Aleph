//! `MoaManageTool` — LLM-facing management surface for MoA (Mixture of
//! Agents) advisory mode (R8 "everything is a tool").
//!
//! Lets the main-loop LLM turn per-session MoA activation on/off, inspect it,
//! and manage `[moa]` presets conversationally — no config.toml editing by
//! hand required. Activation is recorded in
//! [`session_moa_handle`](crate::providers::session_moa_handle) and consumed
//! at the next run's construction (`harness_bridge`), same wiring as
//! `select_model`'s per-session preference. Preset CRUD goes through the same
//! `ConfigPatcher` pipeline `self_config` uses, then hot-refreshes the
//! process-global `[moa]` handle
//! ([`config_handle`](crate::providers::moa::config_handle)) so the change is
//! visible to the next run without a daemon restart.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use super::{notify_tool_result, notify_tool_start};
use crate::config::patcher::ConfigPatcher;
use crate::config::{default_advisor_timeout_secs, Config, MoaFanout, MoaPreset, MoaSlot};
use crate::error::Result;
use crate::providers::moa::get_moa_config;
use crate::providers::session_moa_handle::{clear_session_moa, get_session_moa};
use crate::sync_primitives::Arc;
use crate::tools::turn_context::current_turn_context;
use crate::tools::AlephTool;

/// Guidance shown when `on`/`once` can't resolve a preset to activate (no
/// `[moa]` section, no presets, or the named/`default_preset` preset is
/// missing) — one message for all three cases, matching hermes' single
/// "nothing to activate" UX rather than three subtly different errors.
const NO_PRESET_GUIDANCE: &str = "no [moa] presets configured — use action='set_preset' to \
     create one; use list_models to discover available models";

// =============================================================================
// Args
// =============================================================================

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum MoaManageArgs {
    /// Activate a MoA preset for this session, sticky until turned off.
    On {
        /// Preset name; omit to use `[moa].default_preset` (or the sole
        /// preset when exactly one is configured).
        #[serde(default)]
        preset: Option<String>,
    },
    /// Deactivate MoA for this session.
    Off,
    /// Activate a MoA preset for exactly the next turn, then auto-deactivate.
    Once {
        /// Preset name; omit to use `[moa].default_preset` (or the sole
        /// preset when exactly one is configured).
        #[serde(default)]
        preset: Option<String>,
    },
    /// Show this session's MoA activation and the resolved preset's shape.
    Status,
    /// List all configured MoA presets.
    List,
    /// Create or fully overwrite a named MoA preset.
    SetPreset {
        /// Preset name to create or overwrite.
        name: String,
        /// Advisor slots consulted in parallel on each consultation.
        advisors: Vec<MoaSlot>,
        /// The acting model: receives the full payload plus advisor guidance.
        aggregator: MoaSlot,
        /// `false` = advisors are skipped and the aggregator acts alone.
        /// Omit for `true`.
        #[serde(default)]
        enabled: Option<bool>,
        /// Advisor fan-out cadence. Omit for `per_iteration` (hermes default).
        #[serde(default)]
        fanout: Option<MoaFanout>,
        /// Per-advisor wall-clock budget in seconds. Omit for 120. Accepts the
        /// legacy `advisor_timeout_secs` spelling.
        #[serde(default, alias = "advisor_timeout_secs")]
        advisor_timeout_seconds: Option<u64>,
        /// Caps only advisor output. Omit for no cap.
        #[serde(default)]
        advisor_max_tokens: Option<u32>,
        /// Omit to let the provider default apply.
        #[serde(default)]
        advisor_temperature: Option<f32>,
        /// Omit to let the provider default apply.
        #[serde(default)]
        aggregator_temperature: Option<f32>,
        /// Also set this preset as `[moa].default_preset`.
        #[serde(default)]
        set_default: Option<bool>,
    },
    /// Delete a named MoA preset. Refuses to delete the only remaining preset.
    DeletePreset {
        /// Preset name to delete.
        name: String,
    },
    /// Make an existing preset the `[moa].default_preset`.
    SetDefault {
        /// Preset name to promote to default.
        name: String,
    },
    /// Toggle capture of the heavy `MoaTurnTrace` (full advisor I/O) events.
    SetSaveTraces {
        /// `true` = record full advisor traces; `false` = off.
        on: bool,
    },
}

/// Hand-written FLAT schema instead of `#[derive(JsonSchema)]`.
///
/// The derived schema for an internally-tagged enum is a root `oneOf` with
/// `$defs`/`$ref`s. Round-2 runtime QA proved that shape unusable end to end:
/// the Anthropic protocol adapter deliberately strips root union keywords
/// (`strip_anthropic_tool_schema_unions` — the real Anthropic API rejects
/// them with HTTP 400), leaving `properties: {}`, so every model faithfully
/// emits empty `{}` arguments and the tool can never be called. A flat
/// object schema (`action` enum + per-action fields documented in
/// descriptions — the same shape battle-tested tools like `note_manage`
/// use) survives every adapter untouched. Serde parsing is unchanged: the
/// internally-tagged enum accepts exactly these flat shapes and reports
/// precise per-action missing-field errors.
///
/// This impl is the single source of truth for every declaration path
/// (builtin registry `schema_for!`, `AlephTool::definition()`, the
/// progressive-disclosure `get_tool_schema` snapshot).
impl JsonSchema for MoaManageArgs {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "MoaManageArgs".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "object",
            "title": "MoaManageArgs",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["on", "off", "once", "status", "list", "set_preset", "delete_preset", "set_default", "set_save_traces"],
                    "description": "Operation: on (activate preset for this session, sticky) / off (deactivate) / once (next turn only) / status / list / set_preset (create or overwrite a preset) / delete_preset / set_default (promote a preset to [moa].default_preset) / set_save_traces (toggle full advisor trace capture)."
                },
                "preset": {
                    "type": "string",
                    "description": "on/once only: preset name; omit to use the default preset."
                },
                "name": {
                    "type": "string",
                    "description": "set_preset/delete_preset/set_default: preset name. REQUIRED for those actions."
                },
                "enabled": {
                    "type": "boolean",
                    "description": "set_preset: false = create the preset disabled (aggregator acts alone, advisors skipped). Default true."
                },
                "on": {
                    "type": "boolean",
                    "description": "set_save_traces: true records full advisor I/O traces, false disables. REQUIRED for that action."
                },
                "advisors": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "provider": { "type": "string", "description": "A [providers.<key>] entry; \"moa\" is rejected (recursion guard)." },
                            "model": { "type": "string" }
                        },
                        "required": ["provider", "model"]
                    },
                    "description": "set_preset: advisor (provider, model) slots consulted in parallel. REQUIRED for set_preset."
                },
                "aggregator": {
                    "type": "object",
                    "properties": {
                        "provider": { "type": "string", "description": "A [providers.<key>] entry; \"moa\" is rejected (recursion guard)." },
                        "model": { "type": "string" }
                    },
                    "required": ["provider", "model"],
                    "description": "set_preset: the acting model slot (receives the payload plus advisor guidance). REQUIRED for set_preset."
                },
                "fanout": {
                    "type": "string",
                    "enum": ["per_iteration", "user_turn"],
                    "description": "set_preset: advisor fan-out cadence. Default per_iteration."
                },
                "advisor_timeout_seconds": { "type": "integer", "description": "set_preset: per-advisor wall-clock budget in seconds. Default 120." },
                "advisor_max_tokens": { "type": "integer", "description": "set_preset: cap advisor output tokens. Omit for no cap." },
                "advisor_temperature": { "type": "number", "description": "set_preset: advisor sampling temperature. Omit for provider default." },
                "aggregator_temperature": { "type": "number", "description": "set_preset: aggregator sampling temperature. Omit for provider default." },
                "set_default": { "type": "boolean", "description": "set_preset: also make this preset [moa].default_preset." }
            },
            "required": ["action"]
        })
    }
}

// =============================================================================
// Output
// =============================================================================

#[derive(Debug, Serialize)]
pub struct MoaManageOutput {
    pub success: bool,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

// =============================================================================
// Tool Struct
// =============================================================================

#[derive(Clone, Default)]
pub struct MoaManageTool {
    config: Option<Arc<RwLock<Config>>>,
    // `OnceLock` (not `Option<T>`) so the registry can late-bind the patcher
    // through `Arc<BuiltinToolRegistry>` (no `&mut self` available post-wrap).
    // Hot-path read is a single pointer-load (`OnceLock::get`).
    config_patcher: Arc<std::sync::OnceLock<Arc<ConfigPatcher>>>,
}

impl MoaManageTool {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_config(mut self, config: Arc<RwLock<Config>>) -> Self {
        self.config = Some(config);
        self
    }

    #[must_use]
    pub fn with_patcher(self, patcher: Arc<ConfigPatcher>) -> Self {
        let _ = self.config_patcher.set(patcher);
        self
    }

    /// Late-bind a `ConfigPatcher` after construction (see
    /// `BuiltinToolRegistry::set_config_patcher`). Idempotent: a second set
    /// silently no-ops (`OnceLock::set` returns Err when the cell is full).
    pub fn set_patcher(&self, patcher: Arc<ConfigPatcher>) {
        let _ = self.config_patcher.set(patcher);
    }
}

fn no_turn_context_output() -> MoaManageOutput {
    MoaManageOutput {
        success: false,
        message: "No active session: moa must run inside a conversation turn.".to_string(),
        data: None,
    }
}

// =============================================================================
// Operation Implementations
// =============================================================================

impl MoaManageTool {
    /// Shared `on`/`once` implementation: resolve the requested (or default)
    /// preset against the live `[moa]` config, and — only if it resolves —
    /// arm the session handle the harness reads at the next run.
    async fn activate(&self, preset: Option<String>, one_shot: bool) -> Result<MoaManageOutput> {
        let Some(ctx) = current_turn_context() else {
            return Ok(no_turn_context_output());
        };
        let key = ctx.session_key.to_key_string();

        let armed = if one_shot {
            crate::providers::moa::activation::arm_one_shot(&key, preset)
        } else {
            crate::providers::moa::activation::arm_sticky(&key, preset)
        };

        match armed {
            Ok(name) => {
                let message = if one_shot {
                    format!("MoA '{name}' active for this session for the next turn only")
                } else {
                    format!("MoA '{name}' active for this session from the NEXT turn")
                };
                Ok(MoaManageOutput {
                    success: true,
                    message,
                    data: Some(serde_json::json!({ "preset": name, "one_shot": one_shot })),
                })
            }
            Err(msg) => Ok(MoaManageOutput {
                success: false,
                // Audit AW2: a RESOLVED-but-invalid preset (hand-edited: empty
                // slot, recursive, duplicate slots, bad timeout/temperature) now
                // returns a SPECIFIC arm-time validation error from
                // `activation::resolve_or_err`. Surface it so the user fixes the
                // broken preset instead of being wrongly told to create a new
                // one — the four other arm sites (select_model / slash / chat.send)
                // already pass `msg` through. The generic guidance is kept only
                // for the genuinely-missing cases (no `[moa]` section / preset
                // not found), which `on_with_no_presets_configured_gives_guidance`
                // locks in (that error reads "no [moa] section configured", not
                // "invalid").
                message: if msg.contains("is invalid and cannot be armed") {
                    msg
                } else {
                    NO_PRESET_GUIDANCE.to_string()
                },
                data: None,
            }),
        }
    }

    async fn deactivate(&self) -> Result<MoaManageOutput> {
        let Some(ctx) = current_turn_context() else {
            return Ok(no_turn_context_output());
        };
        clear_session_moa(&ctx.session_key.to_key_string());
        Ok(MoaManageOutput {
            success: true,
            message: "MoA deactivated for this session.".to_string(),
            data: None,
        })
    }

    async fn status(&self) -> Result<MoaManageOutput> {
        let Some(ctx) = current_turn_context() else {
            return Ok(no_turn_context_output());
        };
        let key = ctx.session_key.to_key_string();

        let Some(pref) = get_session_moa(&key) else {
            return Ok(MoaManageOutput {
                success: true,
                message: "MoA is not active for this session.".to_string(),
                data: Some(serde_json::json!({ "active": false })),
            });
        };

        let moa_cfg = get_moa_config();
        let resolved = moa_cfg
            .as_ref()
            .and_then(|cfg| cfg.resolve_preset(pref.preset.as_deref()));

        match resolved {
            Some((name, preset)) => {
                let suffix = if pref.one_shot {
                    " (one-shot, next turn only)"
                } else {
                    ""
                };
                Ok(MoaManageOutput {
                    success: true,
                    message: format!("MoA '{name}' active for this session{suffix}."),
                    data: Some(serde_json::json!({
                        "active": true,
                        "preset": name,
                        "one_shot": pref.one_shot,
                        "aggregator": preset.aggregator,
                        "advisors": preset.advisors,
                        "fanout": preset.fanout,
                    })),
                })
            }
            None => Ok(MoaManageOutput {
                success: true,
                message: format!(
                    "MoA is armed for this session but preset '{}' no longer resolves — it may \
                     have been deleted or renamed.",
                    pref.preset.as_deref().unwrap_or("<default>")
                ),
                data: Some(serde_json::json!({ "active": true, "preset_resolved": false })),
            }),
        }
    }

    async fn list(&self) -> Result<MoaManageOutput> {
        let moa_cfg = get_moa_config().unwrap_or_default();
        if moa_cfg.presets.is_empty() {
            return Ok(MoaManageOutput {
                success: true,
                message: "No [moa] presets configured — use action='set_preset' to create one."
                    .to_string(),
                data: Some(serde_json::json!({ "presets": [] })),
            });
        }

        let mut names: Vec<String> = moa_cfg.presets.keys().cloned().collect();
        names.sort();
        let presets: Vec<serde_json::Value> = names
            .iter()
            .map(|name| {
                let preset = moa_cfg
                    .presets
                    .get(name)
                    .expect("invariant: name came from presets keys");
                let is_default = moa_cfg.default_preset.as_deref() == Some(name.as_str());
                serde_json::json!({
                    "name": name,
                    "default": is_default,
                    "enabled": preset.enabled,
                    "advisors": preset.advisors,
                    "aggregator": preset.aggregator,
                    "fanout": preset.fanout,
                })
            })
            .collect();

        Ok(MoaManageOutput {
            success: true,
            message: format!(
                "{} MoA preset(s) configured{}.",
                presets.len(),
                moa_cfg
                    .default_preset
                    .as_deref()
                    .map(|d| format!(" ('{d}' is default)"))
                    .unwrap_or_default()
            ),
            data: Some(serde_json::json!({ "presets": presets })),
        })
    }

    /// Construct the shared preset write-store, or return the "not available"
    /// output when config/patcher weren't injected (e.g. a bare tool instance).
    /// The single seam every preset-mutating action goes through.
    fn resolve_store(
        &self,
    ) -> std::result::Result<crate::providers::moa::MoaPresetStore, MoaManageOutput> {
        match (&self.config, self.config_patcher.get()) {
            (Some(c), Some(p)) => Ok(crate::providers::moa::MoaPresetStore::new(
                Arc::clone(c),
                Arc::clone(p),
            )),
            _ => Err(MoaManageOutput {
                success: false,
                message: "MoA config or patcher not available".to_string(),
                data: None,
            }),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn set_preset(
        &self,
        name: String,
        advisors: Vec<MoaSlot>,
        aggregator: MoaSlot,
        enabled: Option<bool>,
        fanout: Option<MoaFanout>,
        advisor_timeout_seconds: Option<u64>,
        advisor_max_tokens: Option<u32>,
        advisor_temperature: Option<f32>,
        aggregator_temperature: Option<f32>,
        set_default: Option<bool>,
    ) -> Result<MoaManageOutput> {
        let preset = MoaPreset {
            enabled: enabled.unwrap_or(true),
            advisors,
            aggregator,
            fanout: fanout.unwrap_or_default(),
            advisor_timeout_secs: advisor_timeout_seconds.unwrap_or_else(default_advisor_timeout_secs),
            advisor_max_tokens,
            advisor_temperature,
            aggregator_temperature,
        };

        let store = match self.resolve_store() {
            Ok(s) => s,
            Err(out) => return Ok(out),
        };
        match store
            .save_preset(&name, preset, set_default.unwrap_or(false))
            .await
        {
            Ok(result) => Ok(MoaManageOutput {
                success: true,
                message: format!(
                    "Preset '{name}' saved ({} field change(s)).",
                    result.diff.len()
                ),
                data: Some(serde_json::to_value(&result).unwrap_or_default()),
            }),
            Err(crate::providers::moa::MoaStoreError::Validation(errors)) => Ok(MoaManageOutput {
                success: false,
                message: format!("Preset '{name}' rejected: {}", errors.join("; ")),
                data: Some(serde_json::json!({ "errors": errors })),
            }),
            Err(e) => Ok(MoaManageOutput {
                success: false,
                message: e.to_string(),
                data: None,
            }),
        }
    }

    async fn delete_preset(&self, name: String) -> Result<MoaManageOutput> {
        let store = match self.resolve_store() {
            Ok(s) => s,
            Err(out) => return Ok(out),
        };
        match store.delete_preset(&name).await {
            Ok(result) => Ok(MoaManageOutput {
                success: true,
                message: format!("Preset '{name}' deleted."),
                data: Some(serde_json::to_value(&result).unwrap_or_default()),
            }),
            Err(e) => Ok(MoaManageOutput {
                success: false,
                message: e.to_string(),
                data: None,
            }),
        }
    }

    async fn set_default(&self, name: String) -> Result<MoaManageOutput> {
        let store = match self.resolve_store() {
            Ok(s) => s,
            Err(out) => return Ok(out),
        };
        match store.set_default(&name).await {
            Ok(result) => Ok(MoaManageOutput {
                success: true,
                message: format!("Preset '{name}' is now the default."),
                data: Some(serde_json::to_value(&result).unwrap_or_default()),
            }),
            Err(e) => Ok(MoaManageOutput {
                success: false,
                message: e.to_string(),
                data: None,
            }),
        }
    }

    async fn set_save_traces(&self, on: bool) -> Result<MoaManageOutput> {
        let store = match self.resolve_store() {
            Ok(s) => s,
            Err(out) => return Ok(out),
        };
        match store.set_save_traces(on).await {
            Ok(result) => Ok(MoaManageOutput {
                success: true,
                message: format!(
                    "MoA advisor trace capture {}.",
                    if on { "enabled" } else { "disabled" }
                ),
                data: Some(serde_json::to_value(&result).unwrap_or_default()),
            }),
            Err(e) => Ok(MoaManageOutput {
                success: false,
                message: e.to_string(),
                data: None,
            }),
        }
    }
}

// =============================================================================
// AlephTool Implementation
// =============================================================================

#[async_trait]
impl AlephTool for MoaManageTool {
    const NAME: &'static str = "moa";
    const DESCRIPTION: &'static str = "Manage Mixture-of-Agents (MoA) advisory mode for this \
        session. MoA consults several advisor models in parallel on the live conversation before \
        each step and hands their private guidance to the acting aggregator model. action='on' \
        activates a preset for this session (sticky), 'once' for the next turn only, 'off' \
        deactivates, 'status'/'list' inspect, 'set_preset'/'delete_preset'/'set_default' manage \
        presets and 'set_save_traces' toggles full advisor trace capture — all conversationally. \
        MoA multiplies per-turn cost by the advisor count — activate only when the user asks for it.";

    type Args = MoaManageArgs;
    type Output = MoaManageOutput;

    /// The flat schema's non-`action` fields are all optional (per-action
    /// requirements live in serde parsing) — strictification would force
    /// every field required. Same opt-out as `self_config`.
    fn strict_schema(&self) -> bool {
        false
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match &args {
            MoaManageArgs::On { preset } => notify_tool_start(
                Self::NAME,
                &format!("on:{}", preset.as_deref().unwrap_or("<default>")),
            ),
            MoaManageArgs::Off => notify_tool_start(Self::NAME, "off"),
            MoaManageArgs::Once { preset } => notify_tool_start(
                Self::NAME,
                &format!("once:{}", preset.as_deref().unwrap_or("<default>")),
            ),
            MoaManageArgs::Status => notify_tool_start(Self::NAME, "status"),
            MoaManageArgs::List => notify_tool_start(Self::NAME, "list"),
            MoaManageArgs::SetPreset { name, .. } => {
                notify_tool_start(Self::NAME, &format!("set_preset:{name}"))
            }
            MoaManageArgs::DeletePreset { name } => {
                notify_tool_start(Self::NAME, &format!("delete_preset:{name}"))
            }
            MoaManageArgs::SetDefault { name } => {
                notify_tool_start(Self::NAME, &format!("set_default:{name}"))
            }
            MoaManageArgs::SetSaveTraces { on } => {
                notify_tool_start(Self::NAME, &format!("set_save_traces:{on}"))
            }
        }

        let result = match args {
            MoaManageArgs::On { preset } => self.activate(preset, false).await,
            MoaManageArgs::Off => self.deactivate().await,
            MoaManageArgs::Once { preset } => self.activate(preset, true).await,
            MoaManageArgs::Status => self.status().await,
            MoaManageArgs::List => self.list().await,
            MoaManageArgs::SetPreset {
                name,
                advisors,
                aggregator,
                enabled,
                fanout,
                advisor_timeout_seconds,
                advisor_max_tokens,
                advisor_temperature,
                aggregator_temperature,
                set_default,
            } => {
                self.set_preset(
                    name,
                    advisors,
                    aggregator,
                    enabled,
                    fanout,
                    advisor_timeout_seconds,
                    advisor_max_tokens,
                    advisor_temperature,
                    aggregator_temperature,
                    set_default,
                )
                .await
            }
            MoaManageArgs::DeletePreset { name } => self.delete_preset(name).await,
            MoaManageArgs::SetDefault { name } => self.set_default(name).await,
            MoaManageArgs::SetSaveTraces { on } => self.set_save_traces(on).await,
        };

        match &result {
            Ok(output) => notify_tool_result(Self::NAME, &output.message, output.success),
            Err(e) => notify_tool_result(Self::NAME, &e.to_string(), false),
        }

        result
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MoaToml;
    use crate::providers::session_moa_handle::set_session_moa;
    use crate::routing::session_key::SessionKey;
    use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};
    use crate::tools::AlephTool;

    // Tests that mutate the process-global `[moa]` config slot serialize on
    // the crate-wide lock in `config_handle` — shared with `select_model.rs`
    // tests, which touch the same slot.
    use crate::providers::moa::config_handle::moa_config_test_lock;
    use crate::providers::moa::store_moa_config;

    fn test_ctx(ephemeral_id: &str) -> TurnContext {
        TurnContext {
            session_key: SessionKey::Ephemeral {
                agent_id: "main".to_string(),
                ephemeral_id: ephemeral_id.to_string(),
            },
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: None,
            channel_tool_permissions: None,
            unattended: false,
        }
    }

    fn solo_preset() -> MoaPreset {
        MoaPreset {
            enabled: true,
            advisors: vec![MoaSlot {
                provider: "openai".to_string(),
                model: "gpt-5".to_string(),
            }],
            aggregator: MoaSlot {
                provider: "anthropic".to_string(),
                model: "claude-opus-4".to_string(),
            },
            fanout: MoaFanout::default(),
            advisor_timeout_secs: 120,
            advisor_max_tokens: None,
            advisor_temperature: None,
            aggregator_temperature: None,
        }
    }

    #[test]
    fn moa_set_preset_advisor_timeout_accepts_canonical_and_legacy_alias() {
        // `advisors`/`aggregator` are required fields of the `SetPreset`
        // variant (no `#[serde(default)]`) — included here so the JSON
        // actually deserializes; the brief's minimal JSON omitted them.
        let a: MoaManageArgs = serde_json::from_value(serde_json::json!({
            "action": "set_preset", "name": "p", "advisors": [],
            "aggregator": {"provider": "anthropic", "model": "opus"},
            "advisor_timeout_seconds": 120
        }))
        .unwrap();
        match a {
            MoaManageArgs::SetPreset {
                advisor_timeout_seconds,
                ..
            } => assert_eq!(advisor_timeout_seconds, Some(120)),
            _ => panic!("wrong variant"),
        }
        let b: MoaManageArgs = serde_json::from_value(serde_json::json!({
            "action": "set_preset", "name": "p", "advisors": [],
            "aggregator": {"provider": "anthropic", "model": "opus"},
            "advisor_timeout_secs": 120
        }))
        .unwrap();
        match b {
            MoaManageArgs::SetPreset {
                advisor_timeout_seconds,
                ..
            } => assert_eq!(advisor_timeout_seconds, Some(120)),
            _ => panic!("wrong variant"),
        }
    }

    #[tokio::test]
    async fn on_with_no_presets_configured_gives_guidance() {
        let _guard = moa_config_test_lock();
        store_moa_config(None);

        let ctx = test_ctx("moa-test-no-presets");
        let key = ctx.session_key.to_key_string();
        let out = TURN_CONTEXT
            .scope(ctx, async {
                MoaManageTool::default()
                    .call(MoaManageArgs::On { preset: None })
                    .await
            })
            .await
            .unwrap();

        assert!(!out.success);
        assert!(
            out.message.contains("no [moa] presets configured"),
            "{}",
            out.message
        );
        clear_session_moa(&key);
    }

    #[tokio::test]
    async fn on_with_invalid_preset_surfaces_specific_error() {
        // Audit AW2: arming an EXISTING-but-invalid preset must report the
        // SPECIFIC validation failure (so the user fixes it), NOT the generic
        // "no presets configured — create one" guidance meant for missing ones.
        let _guard = moa_config_test_lock();
        let mut moa = MoaToml::default();
        let mut broken = solo_preset();
        broken.advisor_timeout_secs = 0; // invalid: must be >= 1
        moa.presets.insert("broken".to_string(), broken);
        store_moa_config(Some(moa));

        let ctx = test_ctx("moa-test-invalid-preset");
        let key = ctx.session_key.to_key_string();
        let out = TURN_CONTEXT
            .scope(ctx, async {
                MoaManageTool::default()
                    .call(MoaManageArgs::On {
                        preset: Some("broken".to_string()),
                    })
                    .await
            })
            .await
            .unwrap();

        assert!(!out.success);
        assert!(
            out.message.contains("is invalid and cannot be armed"),
            "{}",
            out.message
        );
        assert!(
            out.message.contains("advisor_timeout_secs must be >= 1"),
            "{}",
            out.message
        );
        // Must NOT mask the real reason behind the generic guidance.
        assert!(
            !out.message.contains("no [moa] presets configured"),
            "{}",
            out.message
        );
        clear_session_moa(&key);
        store_moa_config(None);
    }

    #[tokio::test]
    async fn on_with_resolvable_preset_writes_sticky_session_handle() {
        let _guard = moa_config_test_lock();
        let mut moa = MoaToml::default();
        moa.presets.insert("solo".to_string(), solo_preset());
        store_moa_config(Some(moa));

        let ctx = test_ctx("moa-test-on-sticky");
        let key = ctx.session_key.to_key_string();
        // Prime a model pick — activating MoA must clear it (round-2 E3
        // selector-slot exclusivity, symmetric with select_model's "moa:"
        // branch clearing any sticky MoA preset).
        crate::providers::session_model_handle::set_session_model(&key, None, "gpt-5".to_string());
        let out = TURN_CONTEXT
            .scope(ctx, async {
                MoaManageTool::default()
                    .call(MoaManageArgs::On { preset: None })
                    .await
            })
            .await
            .unwrap();

        assert!(out.success, "{}", out.message);
        let pref = get_session_moa(&key).unwrap();
        assert_eq!(pref.preset, None);
        assert!(!pref.one_shot);
        assert!(crate::providers::session_model_handle::get_session_model(&key).is_none());

        clear_session_moa(&key);
        store_moa_config(None);
    }

    #[tokio::test]
    async fn once_writes_one_shot_session_handle() {
        let _guard = moa_config_test_lock();
        let mut moa = MoaToml::default();
        moa.presets.insert("solo".to_string(), solo_preset());
        store_moa_config(Some(moa));

        let ctx = test_ctx("moa-test-once");
        let key = ctx.session_key.to_key_string();
        let out = TURN_CONTEXT
            .scope(ctx, async {
                MoaManageTool::default()
                    .call(MoaManageArgs::Once { preset: None })
                    .await
            })
            .await
            .unwrap();

        assert!(out.success, "{}", out.message);
        let pref = get_session_moa(&key).unwrap();
        assert!(pref.one_shot);

        clear_session_moa(&key);
        store_moa_config(None);
    }

    #[tokio::test]
    async fn off_clears_session_handle() {
        let ctx = test_ctx("moa-test-off");
        let key = ctx.session_key.to_key_string();
        set_session_moa(&key, Some("solo".to_string()), false);
        assert!(get_session_moa(&key).is_some());

        let out = TURN_CONTEXT
            .scope(ctx, async {
                MoaManageTool::default().call(MoaManageArgs::Off).await
            })
            .await
            .unwrap();

        assert!(out.success);
        assert!(get_session_moa(&key).is_none());
    }

    #[tokio::test]
    async fn no_turn_context_is_graceful() {
        // Outside a turn scope there is no session to bind to — degrade, not panic.
        let out = MoaManageTool::default()
            .call(MoaManageArgs::On { preset: None })
            .await
            .unwrap();
        assert!(!out.success);
    }

    #[test]
    fn definition_declares_flat_non_strict_schema() {
        // Regression lock (round-2 runtime QA): grammar-constrained provider
        // endpoints cannot compile a root-oneOf/$ref schema and emit tool
        // calls with EMPTY arguments. The declared schema must stay a flat
        // object (action enum + optional fields) and opt out of strict mode.
        let def = MoaManageTool::default().definition();
        assert!(!def.strict, "moa tool must opt out of strict schema mode");
        assert!(
            def.parameters.get("oneOf").is_none() && def.parameters.get("$defs").is_none(),
            "declared schema must be flat — no root oneOf/$defs"
        );
        let actions: Vec<&str> = def.parameters["properties"]["action"]["enum"]
            .as_array()
            .expect("action enum present")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        for a in [
            "on",
            "off",
            "once",
            "status",
            "list",
            "set_preset",
            "delete_preset",
            "set_default",
            "set_save_traces",
        ] {
            assert!(actions.contains(&a), "action enum missing {a}");
        }
        assert_eq!(
            def.parameters["required"],
            serde_json::json!(["action"]),
            "only `action` is unconditionally required"
        );
    }

    #[test]
    fn moa_schema_emits_canonical_advisor_timeout_seconds() {
        // Task 5 (2e86b318e) renamed the model-facing arg
        // `advisor_timeout_secs` -> `advisor_timeout_seconds` and hand-edited
        // the hand-written `impl JsonSchema for MoaManageArgs` (this schema
        // is NOT `#[derive(JsonSchema)]`-generated, so nothing re-checks it
        // against the struct field/alias). Guard against a future edit
        // silently reverting the emitted property key back to the legacy
        // spelling, which would re-diverge the model-facing schema from the
        // serde-accepted name with zero other test failing.
        let def = MoaManageTool::default().definition();
        let props = def.parameters["properties"]
            .as_object()
            .expect("schema has a properties object");

        assert!(
            props.contains_key("advisor_timeout_seconds"),
            "schema must declare the canonical `advisor_timeout_seconds` property; got keys: {:?}",
            props.keys().collect::<Vec<_>>()
        );
        assert!(
            !props.contains_key("advisor_timeout_secs"),
            "schema must NOT declare the legacy `advisor_timeout_secs` property key; got keys: {:?}",
            props.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn flat_shaped_json_parses_into_tagged_args() {
        // The flat declared schema and the serde tagged-enum parse layer must
        // accept the same shapes — lock the two most complex actions.
        let set: MoaManageArgs = serde_json::from_value(serde_json::json!({
            "action": "set_preset",
            "name": "deep",
            "advisors": [
                { "provider": "p", "model": "m" },
                { "provider": "p", "model": "m" }
            ],
            "aggregator": { "provider": "p", "model": "m" },
            "fanout": "per_iteration",
            "set_default": true
        }))
        .expect("flat set_preset JSON parses");
        match set {
            MoaManageArgs::SetPreset {
                name,
                advisors,
                fanout,
                set_default,
                ..
            } => {
                assert_eq!(name, "deep");
                assert_eq!(advisors.len(), 2);
                assert_eq!(fanout, Some(MoaFanout::PerIteration));
                assert_eq!(set_default, Some(true));
            }
            other => panic!("expected SetPreset, got {other:?}"),
        }
        let on: MoaManageArgs =
            serde_json::from_value(serde_json::json!({ "action": "on" })).expect("bare on parses");
        assert!(matches!(on, MoaManageArgs::On { preset: None }));
    }

    #[tokio::test]
    async fn set_preset_rejects_recursive_advisor_slot() {
        // `set_preset` now delegates to `MoaPresetStore`, which runs
        // `MoaToml::validation_errors()` (the recursive-slot guard) before it
        // ever calls `ConfigPatcher::apply` — a real patcher is wired here
        // (roundtrip-test pattern) purely so the store can be constructed;
        // the assertions below confirm rejection happens pre-write, not that
        // no patcher exists.
        use crate::config::backup::ConfigBackup;

        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "").unwrap();
        let config = Arc::new(RwLock::new(Config::default()));
        let backup = ConfigBackup::new(dir.path().join("backups"), 10);
        let patcher = Arc::new(ConfigPatcher::new(config.clone(), config_path, backup));
        let tool = MoaManageTool::new()
            .with_config(config)
            .with_patcher(patcher);

        let out = tool
            .call(MoaManageArgs::SetPreset {
                name: "evil".to_string(),
                advisors: vec![MoaSlot {
                    provider: "moa".to_string(),
                    model: "m".to_string(),
                }],
                aggregator: MoaSlot {
                    provider: "anthropic".to_string(),
                    model: "n".to_string(),
                },
                enabled: None,
                fanout: None,
                advisor_timeout_seconds: None,
                advisor_max_tokens: None,
                advisor_temperature: None,
                aggregator_temperature: None,
                set_default: None,
            })
            .await
            .unwrap();

        assert!(!out.success);
        assert!(out.message.contains("recursive"), "{}", out.message);
    }

    #[tokio::test]
    async fn set_preset_then_delete_roundtrip_via_real_patcher() {
        use crate::config::backup::ConfigBackup;

        let _guard = moa_config_test_lock();

        // Real patcher over a temp config.toml (sibling pattern: see
        // config::patcher::tests::setup_patcher /
        // gateway::handlers::config::tests::create_test_patcher).
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "").unwrap();
        let config = Arc::new(RwLock::new(Config::default()));
        let backup = ConfigBackup::new(dir.path().join("backups"), 10);
        let patcher = Arc::new(ConfigPatcher::new(
            config.clone(),
            config_path.clone(),
            backup,
        ));
        let tool = MoaManageTool::new()
            .with_config(config.clone())
            .with_patcher(patcher);

        // set_preset: writes config + hot-reloads the process-global handle.
        let out = tool
            .call(MoaManageArgs::SetPreset {
                name: "roundtrip".to_string(),
                advisors: vec![MoaSlot {
                    provider: "openai".into(),
                    model: "gpt-5".into(),
                }],
                aggregator: MoaSlot {
                    provider: "anthropic".into(),
                    model: "opus".into(),
                },
                enabled: None,
                fanout: None,
                advisor_timeout_seconds: None,
                advisor_max_tokens: None,
                advisor_temperature: None,
                aggregator_temperature: None,
                set_default: Some(true),
            })
            .await
            .unwrap();
        assert!(out.success, "{}", out.message);
        let live = get_moa_config().expect("hot-reloaded");
        assert!(live.presets.contains_key("roundtrip"));
        assert_eq!(live.default_preset.as_deref(), Some("roundtrip"));

        // Second preset so delete has a survivor; then delete the default —
        // default_preset must reassign to the survivor.
        let out = tool
            .call(MoaManageArgs::SetPreset {
                name: "survivor".to_string(),
                advisors: vec![MoaSlot {
                    provider: "openai".into(),
                    model: "gpt-5".into(),
                }],
                aggregator: MoaSlot {
                    provider: "anthropic".into(),
                    model: "opus".into(),
                },
                enabled: None,
                fanout: None,
                advisor_timeout_seconds: None,
                advisor_max_tokens: None,
                advisor_temperature: None,
                aggregator_temperature: None,
                set_default: None,
            })
            .await
            .unwrap();
        assert!(out.success);
        let out = tool
            .call(MoaManageArgs::DeletePreset {
                name: "roundtrip".to_string(),
            })
            .await
            .unwrap();
        assert!(out.success, "{}", out.message);
        let live = get_moa_config().expect("hot-reloaded after delete");
        assert!(!live.presets.contains_key("roundtrip"));
        assert_eq!(live.default_preset.as_deref(), Some("survivor"));

        store_moa_config(None);
    }

    #[tokio::test]
    async fn set_default_save_traces_and_disabled_preset_via_tool() {
        // Round-4 deepen E4: the tool reaches feature-parity with the moa.*
        // RPCs — create a disabled preset, promote a default standalone, and
        // toggle save_traces, all conversationally.
        use crate::config::backup::ConfigBackup;
        let _guard = moa_config_test_lock();

        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "").unwrap();
        let config = Arc::new(RwLock::new(Config::default()));
        let backup = ConfigBackup::new(dir.path().join("backups"), 10);
        let patcher = Arc::new(ConfigPatcher::new(config.clone(), config_path, backup));
        let tool = MoaManageTool::new()
            .with_config(config)
            .with_patcher(patcher);

        let disabled = |name: &str| MoaManageArgs::SetPreset {
            name: name.to_string(),
            advisors: vec![MoaSlot {
                provider: "openai".into(),
                model: "gpt-5".into(),
            }],
            aggregator: MoaSlot {
                provider: "anthropic".into(),
                model: "opus".into(),
            },
            enabled: Some(false),
            fanout: None,
            advisor_timeout_seconds: None,
            advisor_max_tokens: None,
            advisor_temperature: None,
            aggregator_temperature: None,
            set_default: None,
        };

        assert!(tool.call(disabled("solo-agg")).await.unwrap().success);
        assert!(tool.call(disabled("full")).await.unwrap().success);
        assert!(
            !get_moa_config().unwrap().presets["solo-agg"].enabled,
            "enabled=false must persist"
        );

        // set_default action promotes an existing preset with no full re-save.
        assert!(
            tool.call(MoaManageArgs::SetDefault {
                name: "full".into()
            })
            .await
            .unwrap()
            .success
        );
        assert_eq!(
            get_moa_config().unwrap().default_preset.as_deref(),
            Some("full")
        );

        // set_save_traces action toggles the trace gate.
        assert!(
            tool.call(MoaManageArgs::SetSaveTraces { on: true })
                .await
                .unwrap()
                .success
        );
        assert!(get_moa_config().unwrap().save_traces);

        store_moa_config(None);
    }
}
