//! Opt-in startup switch of the public telecom private turn.
//!
//! When enabled (and the trusted operator control plane plus the memory
//! backend are available), public SMS runs are offered the single
//! `request_private_lookup` tool, and an operator may approve (A) a
//! read-only search of the owner's notes and (B) the exact reply drawn from
//! it. Disabled by default: public runs then have no tools and no private
//! path at all.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Startup-only `[telecom_private_read]` settings. Changes require a restart.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct TelecomPrivateReadConfig {
    /// Off by default. The only source it opens is the owner's memory,
    /// searched read-only, behind two operator approvals.
    pub enabled: bool,
}

impl TelecomPrivateReadConfig {
    /// Whether the private turn is requested by configuration.
    pub fn is_enabled(section: Option<&Self>) -> bool {
        section.is_some_and(|c| c.enabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn absent_section_is_disabled_and_not_serialized() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.telecom_private_read.is_none());
        assert!(!TelecomPrivateReadConfig::is_enabled(
            cfg.telecom_private_read.as_ref()
        ));
        assert!(!TelecomPrivateReadConfig::default().enabled);
        assert!(!toml::to_string(&cfg)
            .unwrap()
            .contains("[telecom_private_read]"));
    }

    #[test]
    fn section_round_trips_and_rejects_unknown_fields() {
        let cfg: Config = toml::from_str("[telecom_private_read]\nenabled = true\n").unwrap();
        assert!(TelecomPrivateReadConfig::is_enabled(
            cfg.telecom_private_read.as_ref()
        ));
        cfg.validate().unwrap();
        let again: Config = toml::from_str(&toml::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(again.telecom_private_read, cfg.telecom_private_read);
        let empty: Config = toml::from_str("[telecom_private_read]\n").unwrap();
        assert!(!TelecomPrivateReadConfig::is_enabled(
            empty.telecom_private_read.as_ref()
        ));
        // No other source can be opened through this section.
        assert!(toml::from_str::<Config>(
            "[telecom_private_read]\nenabled = true\nsources = ['owner_secrets']\n"
        )
        .is_err());
    }
}
