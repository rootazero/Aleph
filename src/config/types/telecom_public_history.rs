//! Opt-in startup configuration for dedicated public telecom history.
//! Only a key-file path is configured here; key material never enters Config.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::gateway::telecom::public_history::{
    PublicHistoryConfig, PublicHistoryError, PUBLIC_HISTORY_MAX_SESSION_BYTES,
    PUBLIC_HISTORY_MAX_TOTAL_BYTES, PUBLIC_HISTORY_MAX_TTL, PUBLIC_HISTORY_MAX_TURNS,
};

/// Startup-only `[telecom_public_history]` settings. Changes require a restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PublicHistoryStartupConfig {
    /// Off by default. Missing/unusable keys never enable a fallback store.
    pub enabled: bool,
    /// Operator-provided dedicated key; absolute path, no default lookup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_file: Option<PathBuf>,
    /// Retention in seconds, within 0..=86400. Zero disables startup I/O.
    pub ttl_secs: u64,
    /// Retained completed turns per session, within 1..=32.
    pub max_turns: usize,
    /// Plaintext bytes per session, within 1..=65536.
    pub max_session_bytes: usize,
    /// Encrypted payload bytes across the store, within 1..=8388608.
    pub max_total_bytes: usize,
}

impl Default for PublicHistoryStartupConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            key_file: None,
            ttl_secs: PUBLIC_HISTORY_MAX_TTL.num_seconds() as u64,
            max_turns: PUBLIC_HISTORY_MAX_TURNS,
            max_session_bytes: PUBLIC_HISTORY_MAX_SESSION_BYTES,
            max_total_bytes: PUBLIC_HISTORY_MAX_TOTAL_BYTES,
        }
    }
}

impl PublicHistoryStartupConfig {
    /// Validate values without reading a key, creating directories, or opening SQLite.
    /// Missing keys are a startup degradation, not a config validation failure.
    pub fn validate(&self) -> Result<(), PublicHistoryError> {
        self.store_config().map(|_| ())
    }

    /// Derive all storage limits through the store's existing ceiling checks.
    pub(crate) fn store_config(&self) -> Result<PublicHistoryConfig, PublicHistoryError> {
        if self.key_file.as_ref().is_some_and(|p| !p.is_absolute()) {
            return Err(PublicHistoryError::InvalidConfig(
                "telecom_public_history.key_file must be absolute".into(),
            ));
        }
        let ttl = i64::try_from(self.ttl_secs)
            .ok()
            .and_then(chrono::Duration::try_seconds)
            .ok_or_else(|| {
                PublicHistoryError::InvalidConfig("ttl must be within 0..=24h".into())
            })?;
        PublicHistoryConfig::with_limits(
            ttl,
            self.max_turns,
            self.max_session_bytes,
            self.max_total_bytes,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn absent_history_section_preserves_old_config_default() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.telecom_public_history.is_none());
        assert!(Config::default().telecom_public_history.is_none());
        assert!(!toml::to_string(&cfg)
            .unwrap()
            .contains("[telecom_public_history]"));
    }

    #[test]
    fn history_section_round_trips_without_inline_key_material() {
        let cfg: Config = toml::from_str(
            "[telecom_public_history]\nenabled = true\nttl_secs = 300\nmax_turns = 2\n",
        )
        .unwrap();
        let history = cfg.telecom_public_history.as_ref().unwrap();
        assert!(history.enabled);
        assert!(history.key_file.is_none());
        assert_eq!(history.ttl_secs, 300);
        assert_eq!(history.max_turns, 2);
        assert_eq!(history.max_session_bytes, 65536);
        assert_eq!(history.max_total_bytes, 8388608);
        cfg.validate().unwrap();
        let serialized = toml::to_string(&cfg).unwrap();
        assert!(serialized.contains("[telecom_public_history]"));
        let again: Config = toml::from_str(&serialized).unwrap();
        assert_eq!(again.telecom_public_history, cfg.telecom_public_history);
        // Unknown secret-bearing fields must not appear to configure a key.
        assert!(toml::from_str::<Config>(
            "[telecom_public_history]\nkey = 'not-a-config-secret'\n"
        )
        .is_err());
    }

    #[test]
    fn config_validate_rejects_history_limits_and_relative_key_paths() {
        for settings in [
            "ttl_secs = 86401",
            "ttl_secs = 18446744073709551615",
            "max_turns = 0",
            "max_turns = 33",
            "max_session_bytes = 0",
            "max_session_bytes = 65537",
            "max_total_bytes = 0",
            "max_total_bytes = 8388609",
            "key_file = 'relative.key'",
            "key_file = ''",
        ] {
            let parsed =
                toml::from_str::<Config>(&format!("[telecom_public_history]\n{settings}\n"));
            match parsed {
                Ok(cfg) => assert!(cfg.validate().is_err(), "accepted {settings}"),
                Err(_) => {
                    // TOML's integer ceiling may reject a u64 before validation.
                    assert_eq!(settings, "ttl_secs = 18446744073709551615");
                }
            }
        }
        let mut history = PublicHistoryStartupConfig {
            ttl_secs: u64::MAX,
            ..Default::default()
        };
        assert!(history.validate().is_err());
        history.ttl_secs = 0;
        assert!(history.validate().is_ok());
    }

    #[test]
    fn config_validate_accepts_lowered_limits_and_missing_key() {
        let cfg = Config {
            telecom_public_history: Some(PublicHistoryStartupConfig {
                enabled: true,
                ttl_secs: 0,
                max_turns: 1,
                max_session_bytes: 1,
                max_total_bytes: 1,
                ..Default::default()
            }),
            ..Default::default()
        };
        cfg.validate().unwrap();
        assert_eq!(
            PublicHistoryStartupConfig::default()
                .store_config()
                .unwrap(),
            PublicHistoryConfig::default()
        );
        assert!(!PublicHistoryStartupConfig::default().enabled);
    }
}
