//! Single source of MoA session-activation ("arm") logic, shared by every
//! entry point that arms/disarms MoA (the `moa` tool, the `select_model`
//! `moa:` pick, the `chat.send` `provider:"moa"` override, and the `/moa`
//! one-shot intercept). Resolves + validates the preset once, then mutates
//! the session handles with the canonical set-then-clear ordering.

use crate::providers::{session_model_handle, session_moa_handle};

/// Resolve a preset name against the live `[moa]` config, or an error string
/// naming the missing preset. Shared by the arm helpers below.
fn resolve_or_err(preset: Option<&str>) -> Result<String, String> {
    super::get_moa_config()
        .as_ref()
        .and_then(|cfg| cfg.resolve_preset(preset))
        .map(|(name, _)| name)
        .ok_or_else(|| {
            format!(
                "MoA preset '{}' not found — use the moa tool (action='list') to see presets.",
                preset.unwrap_or("<default>")
            )
        })
}

/// Arm sticky MoA: resolve+validate, write the sticky pref, and clear any
/// per-session model pick (selector-slot exclusivity). Returns the resolved
/// preset name (for user-facing messages) or an error string.
pub fn arm_sticky(session_key: &str, preset: Option<String>) -> Result<String, String> {
    let name = resolve_or_err(preset.as_deref())?;
    session_moa_handle::set_session_moa(session_key, preset, false);
    session_model_handle::clear_session_model(session_key);
    Ok(name)
}

/// Arm one-shot MoA (F2 stash semantics live in `set_session_moa_one_shot`).
/// Same resolution/exclusivity as [`arm_sticky`].
pub fn arm_one_shot(session_key: &str, preset: Option<String>) -> Result<String, String> {
    let name = resolve_or_err(preset.as_deref())?;
    session_moa_handle::set_session_moa_one_shot(session_key, preset);
    session_model_handle::clear_session_model(session_key);
    Ok(name)
}

/// Clear the session's MoA sticky. Idempotent. Called by the "normal model
/// pick" path (selector-slot exclusivity) before it sets the model handle.
pub fn disarm(session_key: &str) {
    session_moa_handle::clear_session_moa(session_key);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::moa::config_handle::moa_config_test_lock;

    fn deep_preset_config() -> crate::config::MoaToml {
        let mut cfg = crate::config::MoaToml::default();
        cfg.presets.insert(
            "deep".to_string(),
            crate::config::MoaPreset {
                enabled: true,
                advisors: vec![crate::config::MoaSlot {
                    provider: "openai".to_string(),
                    model: "gpt-5".to_string(),
                }],
                aggregator: crate::config::MoaSlot {
                    provider: "anthropic".to_string(),
                    model: "claude-opus-4-8".to_string(),
                },
                fanout: crate::config::MoaFanout::default(),
                advisor_timeout_secs: 120,
                advisor_max_tokens: None,
                advisor_temperature: None,
                aggregator_temperature: None,
            },
        );
        cfg
    }

    #[test]
    fn arm_sticky_ok_writes_sticky_and_clears_model() {
        let _guard = moa_config_test_lock();
        let key = "test:moa:activation:sticky-ok";
        crate::providers::moa::store_moa_config(Some(deep_preset_config()));
        session_model_handle::set_session_model(key, None, "gpt-5".to_string());

        let name = arm_sticky(key, Some("deep".to_string())).unwrap();
        assert_eq!(name, "deep");
        let pref = session_moa_handle::get_session_moa(key).unwrap();
        assert_eq!(pref.preset.as_deref(), Some("deep"));
        assert!(!pref.one_shot);
        assert!(session_model_handle::get_session_model(key).is_none());

        session_moa_handle::clear_session_moa(key);
        crate::providers::moa::store_moa_config(None);
    }

    #[test]
    fn arm_sticky_unknown_preset_errs_and_writes_nothing() {
        let _guard = moa_config_test_lock();
        let key = "test:moa:activation:sticky-ghost";
        crate::providers::moa::store_moa_config(Some(deep_preset_config()));

        let err = arm_sticky(key, Some("ghost".to_string())).unwrap_err();
        assert!(err.contains("'ghost' not found"), "got: {err}");
        assert!(session_moa_handle::get_session_moa(key).is_none());

        crate::providers::moa::store_moa_config(None);
    }

    #[test]
    fn arm_one_shot_ok_writes_one_shot() {
        let _guard = moa_config_test_lock();
        let key = "test:moa:activation:oneshot-ok";
        crate::providers::moa::store_moa_config(Some(deep_preset_config()));

        let name = arm_one_shot(key, Some("deep".to_string())).unwrap();
        assert_eq!(name, "deep");
        let pref = session_moa_handle::get_session_moa(key).unwrap();
        assert!(pref.one_shot);

        session_moa_handle::clear_session_moa(key);
        crate::providers::moa::store_moa_config(None);
    }

    #[test]
    fn disarm_clears_sticky() {
        let key = "test:moa:activation:disarm";
        session_moa_handle::set_session_moa(key, Some("deep".to_string()), false);
        disarm(key);
        assert!(session_moa_handle::get_session_moa(key).is_none());
    }
}
