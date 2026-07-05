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
use crate::config::patcher::{ConfigPatcher, PatchRequest};
use crate::config::{default_advisor_timeout_secs, Config, MoaFanout, MoaPreset, MoaSlot, MoaToml};
use crate::error::Result;
use crate::providers::moa::{get_moa_config, store_moa_config};
use crate::providers::session_moa_handle::{clear_session_moa, get_session_moa, set_session_moa};
use crate::sync_primitives::Arc;
use crate::tools::turn_context::current_turn_context;
use crate::tools::AlephTool;

use super::error::ToolError;

/// Guidance shown when `on`/`once` can't resolve a preset to activate (no
/// `[moa]` section, no presets, or the named/`default_preset` preset is
/// missing) — one message for all three cases, matching hermes' single
/// "nothing to activate" UX rather than three subtly different errors.
const NO_PRESET_GUIDANCE: &str = "no [moa] presets configured — use action='set_preset' to \
     create one; use list_models to discover available models";

// =============================================================================
// Args
// =============================================================================

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
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
        /// Advisor fan-out cadence. Omit for `per_iteration` (hermes default).
        #[serde(default)]
        fanout: Option<MoaFanout>,
        /// Per-advisor wall-clock budget in seconds. Omit for 120.
        #[serde(default)]
        advisor_timeout_secs: Option<u64>,
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
    config_patcher: Option<Arc<ConfigPatcher>>,
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
    pub fn with_patcher(mut self, patcher: Arc<ConfigPatcher>) -> Self {
        self.config_patcher = Some(patcher);
        self
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

        let moa_cfg = get_moa_config();
        let resolved = moa_cfg
            .as_ref()
            .and_then(|cfg| cfg.resolve_preset(preset.as_deref()));
        let Some((name, _preset)) = resolved else {
            return Ok(MoaManageOutput {
                success: false,
                message: NO_PRESET_GUIDANCE.to_string(),
                data: None,
            });
        };

        let key = ctx.session_key.to_key_string();
        set_session_moa(&key, preset, one_shot);
        // Selector-slot exclusivity (round-2 E3): arming MoA supersedes any
        // per-session model pick — one slot, no precedence confusion.
        crate::providers::session_model_handle::clear_session_model(&key);

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
                let preset = &moa_cfg.presets[name];
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

    #[allow(clippy::too_many_arguments)]
    async fn set_preset(
        &self,
        name: String,
        advisors: Vec<MoaSlot>,
        aggregator: MoaSlot,
        fanout: Option<MoaFanout>,
        advisor_timeout_secs: Option<u64>,
        advisor_max_tokens: Option<u32>,
        advisor_temperature: Option<f32>,
        aggregator_temperature: Option<f32>,
        set_default: Option<bool>,
    ) -> Result<MoaManageOutput> {
        let preset = MoaPreset {
            enabled: true,
            advisors,
            aggregator,
            fanout: fanout.unwrap_or_default(),
            advisor_timeout_secs: advisor_timeout_secs.unwrap_or_else(default_advisor_timeout_secs),
            advisor_max_tokens,
            advisor_temperature,
            aggregator_temperature,
        };

        // Layer-2 validation: run the preset through the SAME
        // `MoaToml::validation_errors()` pipeline a TOML-parsed config goes
        // through (recursive-slot guard, empty-advisor guard, ...) against a
        // scratch config containing only this preset.
        let mut scratch = MoaToml::default();
        scratch.presets.insert(name.clone(), preset.clone());
        let errors = scratch.validation_errors();
        if !errors.is_empty() {
            return Ok(MoaManageOutput {
                success: false,
                message: format!("Preset '{name}' rejected: {}", errors.join("; ")),
                data: Some(serde_json::json!({ "errors": errors })),
            });
        }

        let patcher = match &self.config_patcher {
            Some(p) => p,
            None => {
                return Ok(MoaManageOutput {
                    success: false,
                    message: "Config patcher not available".to_string(),
                    data: None,
                })
            }
        };

        // Every MoaPreset field is serialized explicitly (Option fields as
        // `null` when unset) so the deep-merge patch fully replaces an
        // existing same-named preset instead of leaving stale fields behind.
        let preset_json = serde_json::to_value(&preset)
            .map_err(|e| ToolError::Execution(format!("Failed to serialize preset: {e}")))?;
        let mut presets_patch = serde_json::Map::new();
        presets_patch.insert(name.clone(), preset_json);
        let mut patch = serde_json::json!({ "presets": presets_patch });
        if set_default.unwrap_or(false) {
            patch["default_preset"] = serde_json::json!(name);
        }

        let request = PatchRequest {
            path: "moa".to_string(),
            patch,
            health_check: false,
            dry_run: false,
        };

        match patcher.apply(request).await {
            Ok(result) => {
                if result.success {
                    // Hot-refresh the process-global handle so the change is
                    // visible to the very next run (mirrors self_config's
                    // route hot-apply).
                    if let Some(cfg) = &self.config {
                        store_moa_config(cfg.read().await.moa.clone());
                    }
                    Ok(MoaManageOutput {
                        success: true,
                        message: format!(
                            "Preset '{name}' saved ({} field change(s)).",
                            result.diff.len()
                        ),
                        data: Some(serde_json::to_value(&result).unwrap_or_default()),
                    })
                } else {
                    Ok(MoaManageOutput {
                        success: false,
                        message: format!("Preset '{name}' patch did not apply."),
                        data: None,
                    })
                }
            }
            Err(e) => Ok(MoaManageOutput {
                success: false,
                message: format!("Config patch failed: {e}"),
                data: None,
            }),
        }
    }

    async fn delete_preset(&self, name: String) -> Result<MoaManageOutput> {
        let moa_cfg = get_moa_config().unwrap_or_default();
        if !moa_cfg.presets.contains_key(&name) {
            return Ok(MoaManageOutput {
                success: false,
                message: format!("Preset '{name}' does not exist."),
                data: None,
            });
        }
        if moa_cfg.presets.len() == 1 {
            return Ok(MoaManageOutput {
                success: false,
                message: format!(
                    "Cannot delete '{name}': it is the only MoA preset. Create another preset \
                     first with set_preset."
                ),
                data: None,
            });
        }

        let patcher = match &self.config_patcher {
            Some(p) => p,
            None => {
                return Ok(MoaManageOutput {
                    success: false,
                    message: "Config patcher not available".to_string(),
                    data: None,
                })
            }
        };

        let mut presets_patch = serde_json::Map::new();
        presets_patch.insert(name.clone(), serde_json::Value::Null);
        let mut patch = serde_json::json!({ "presets": presets_patch });

        // The deleted preset was the default: reassign to any remaining
        // preset (alphabetically first, for determinism) so a subsequent
        // `resolve_preset(None)` doesn't dangle on a name that no longer
        // exists.
        if moa_cfg.default_preset.as_deref() == Some(name.as_str()) {
            let mut remaining: Vec<&String> =
                moa_cfg.presets.keys().filter(|k| *k != &name).collect();
            remaining.sort();
            if let Some(next) = remaining.first() {
                patch["default_preset"] = serde_json::json!(next);
            }
        }

        // A session pointing at the deleted preset by name is left as-is:
        // there is no reverse index from preset name -> session keys (the
        // map has no enumeration need elsewhere), and `status`/run
        // construction already fail soft when a preset no longer resolves.

        let request = PatchRequest {
            path: "moa".to_string(),
            patch,
            health_check: false,
            dry_run: false,
        };

        match patcher.apply(request).await {
            Ok(result) => {
                if result.success {
                    if let Some(cfg) = &self.config {
                        store_moa_config(cfg.read().await.moa.clone());
                    }
                    Ok(MoaManageOutput {
                        success: true,
                        message: format!("Preset '{name}' deleted."),
                        data: Some(serde_json::to_value(&result).unwrap_or_default()),
                    })
                } else {
                    Ok(MoaManageOutput {
                        success: false,
                        message: format!("Preset '{name}' delete did not apply."),
                        data: None,
                    })
                }
            }
            Err(e) => Ok(MoaManageOutput {
                success: false,
                message: format!("Config patch failed: {e}"),
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
        deactivates, 'status'/'list' inspect, 'set_preset'/'delete_preset' manage presets \
        conversationally. MoA multiplies per-turn cost by the advisor count — activate only when \
        the user asks for it.";

    type Args = MoaManageArgs;
    type Output = MoaManageOutput;

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
                fanout,
                advisor_timeout_secs,
                advisor_max_tokens,
                advisor_temperature,
                aggregator_temperature,
                set_default,
            } => {
                self.set_preset(
                    name,
                    advisors,
                    aggregator,
                    fanout,
                    advisor_timeout_secs,
                    advisor_max_tokens,
                    advisor_temperature,
                    aggregator_temperature,
                    set_default,
                )
                .await
            }
            MoaManageArgs::DeletePreset { name } => self.delete_preset(name).await,
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
    use crate::routing::session_key::SessionKey;
    use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};
    use crate::tools::AlephTool;

    // Tests that mutate the process-global `[moa]` config slot serialize on
    // the crate-wide lock in `config_handle` — shared with `select_model.rs`
    // tests, which touch the same slot.
    use crate::providers::moa::config_handle::moa_config_test_lock;

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
        crate::providers::session_model_handle::set_session_model(
            &key,
            None,
            "gpt-5".to_string(),
        );
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

    #[tokio::test]
    async fn set_preset_rejects_recursive_advisor_slot() {
        // No patcher wired — this only exercises the pre-patch validation
        // branch, which must reject before ever touching self.config_patcher.
        let out = MoaManageTool::default()
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
                fanout: None,
                advisor_timeout_secs: None,
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
}
