//! Shared Panel Tab descriptors for host and client projections.

use base64::Engine;
use serde::{Deserialize, Serialize};

/// Supported major version of the Panel UI API.
pub const PANEL_UI_API_MAJOR: u16 = 1;

/// A tab identity scoped to its owning plugin.
#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct PanelTabKey {
    pub plugin_id: String,
    pub tab_id: String,
}

impl PanelTabKey {
    /// Return the plugin-qualified tab identity.
    pub fn qualified(&self) -> String {
        format!("{}:{}", self.plugin_id, self.tab_id)
    }
}

/// Trust levels supported by same-origin Panel UI modules.
#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PanelUiTrust {
    Verified,
    Trusted,
}

/// Projected availability of a Panel tab.
#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PanelTabStatus {
    Enabled,
    Disabled,
    Incompatible,
    Failed,
}

/// Single-source wire contract for a plugin-provided Panel tab.
#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct PanelTabDescriptor {
    pub key: PanelTabKey,
    pub label: String,
    pub icon: String,
    pub order: i32,
    pub entry: String,
    pub api_major: u16,
    pub trust: PanelUiTrust,
    pub permissions: Vec<String>,
    pub status: PanelTabStatus,
    pub status_detail: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PanelUiCapabilityOrigin {
    BuiltIn,
    Community,
}

/// Audit facts around the sole browser projection shape.
#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct PanelUiCapabilityDeclaration {
    pub descriptor: PanelTabDescriptor,
    pub origin: PanelUiCapabilityOrigin,
    pub plugin_version: String,
    pub asset_integrity: Option<String>,
    pub public_base: String,
    pub source_revision: u64,
}

/// The only output exposed by a Panel UI capability source.
#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct PanelUiCapabilitySnapshot {
    pub revision: u64,
    pub declarations: Vec<PanelUiCapabilityDeclaration>,
}

impl PanelUiCapabilitySnapshot {
    pub fn new(revision: u64, mut declarations: Vec<PanelUiCapabilityDeclaration>) -> Self {
        declarations.sort_by(|left, right| {
            let rank = |declaration: &PanelUiCapabilityDeclaration| match declaration.origin {
                PanelUiCapabilityOrigin::BuiltIn => 0,
                PanelUiCapabilityOrigin::Community => 1,
            };
            rank(left).cmp(&rank(right)).then_with(|| {
                left.descriptor
                    .key
                    .qualified()
                    .cmp(&right.descriptor.key.qualified())
            })
        });
        Self {
            revision,
            declarations,
        }
    }
}

/// Safe identifier alphabet shared by manifest admission and browser validation.
pub fn valid_identity_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Safe version path segment shared by manifest admission and browser validation.
pub fn valid_version_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+'))
}

/// Validate a relative single-file asset path shared by manifest and browser projection.
pub fn valid_relative_entry(value: &str) -> bool {
    if value.is_empty() || value.starts_with('/') || value.contains('\\') || value.contains(':') {
        return false;
    }
    let path = value.strip_prefix("./").unwrap_or(value);
    !path.is_empty() && path.split('/').all(valid_identity_segment)
}

/// Existing Panel SDK permission vocabulary; generic plugin permissions do not apply.
pub fn supported_community_permission(permission: &str) -> bool {
    matches!(
        permission,
        "pty.list"
            | "pty.spawn"
            | "pty.attach"
            | "pty.input"
            | "pty.resize"
            | "pty.close"
            | "config.get"
            | "events.subscribe:pty.screen"
            | "events.subscribe:pty.exit"
            | "sessions.list"
    )
}

/// Validate the declared SHA-256 SRI format; exact bytes are checked by the loader.
pub fn valid_asset_integrity(integrity: &str) -> bool {
    integrity.strip_prefix("sha256-").is_some_and(|digest| {
        base64::engine::general_purpose::STANDARD
            .decode(digest)
            .is_ok_and(|decoded| decoded.len() == 32)
    })
}

#[cfg(test)]
mod tests {
    use crate::{
        PanelTabDescriptor, PanelTabKey, PanelTabStatus, PanelUiTrust, PANEL_UI_API_MAJOR,
    };
    use serde_json::json;

    fn descriptor() -> PanelTabDescriptor {
        PanelTabDescriptor {
            key: PanelTabKey {
                plugin_id: "example.plugin".into(),
                tab_id: "overview".into(),
            },
            label: "Overview".into(),
            icon: "layout".into(),
            order: -10,
            entry: "tabs/overview.js".into(),
            api_major: PANEL_UI_API_MAJOR,
            trust: PanelUiTrust::Verified,
            permissions: vec!["session.read".into(), "tools.list".into()],
            status: PanelTabStatus::Enabled,
            status_detail: Some("Ready".into()),
        }
    }

    #[test]
    fn version_segments_allow_semver_build_metadata_but_not_path_syntax() {
        assert!(super::valid_version_segment("1.2.3+build.7"));
        assert!(!super::valid_version_segment("1.2.3/build"));
        assert!(!super::valid_version_segment(".."));
    }
    #[test]
    fn panel_descriptor_has_stable_wire_fields_and_enum_spellings() {
        assert_eq!(
            serde_json::to_value(descriptor()).unwrap(),
            json!({
                "key": {"plugin_id": "example.plugin", "tab_id": "overview"},
                "label": "Overview",
                "icon": "layout",
                "order": -10,
                "entry": "tabs/overview.js",
                "api_major": 1,
                "trust": "verified",
                "permissions": ["session.read", "tools.list"],
                "status": "enabled",
                "status_detail": "Ready"
            })
        );
        for (trust, wire) in [
            (PanelUiTrust::Verified, "verified"),
            (PanelUiTrust::Trusted, "trusted"),
        ] {
            assert_eq!(serde_json::to_value(trust).unwrap(), json!(wire));
        }
        for (status, wire) in [
            (PanelTabStatus::Enabled, "enabled"),
            (PanelTabStatus::Disabled, "disabled"),
            (PanelTabStatus::Incompatible, "incompatible"),
            (PanelTabStatus::Failed, "failed"),
        ] {
            assert_eq!(serde_json::to_value(status).unwrap(), json!(wire));
        }
    }

    #[test]
    fn panel_descriptor_round_trips_all_trust_status_and_diagnostic_values() {
        for trust in [PanelUiTrust::Verified, PanelUiTrust::Trusted] {
            for status in [
                PanelTabStatus::Enabled,
                PanelTabStatus::Disabled,
                PanelTabStatus::Incompatible,
                PanelTabStatus::Failed,
            ] {
                for status_detail in [None, Some("Could not load: unavailable 模块".into())] {
                    let original = PanelTabDescriptor {
                        trust: trust.clone(),
                        status: status.clone(),
                        status_detail,
                        ..descriptor()
                    };
                    let decoded: PanelTabDescriptor =
                        serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();
                    assert_eq!(decoded, original);
                }
            }
        }
    }

    #[test]
    fn panel_qualified_key_preserves_plugin_namespaces() {
        let first = PanelTabKey {
            plugin_id: "plugin-a".into(),
            tab_id: "overview".into(),
        };
        let second = PanelTabKey {
            plugin_id: "plugin-b".into(),
            tab_id: "overview".into(),
        };
        assert_eq!(first.qualified(), "plugin-a:overview");
        assert_eq!(second.qualified(), "plugin-b:overview");
        assert_ne!(first.qualified(), second.qualified());
    }

    #[test]
    fn panel_snapshot_has_one_shared_wire_contract() {
        use super::{
            PanelUiCapabilityDeclaration, PanelUiCapabilityOrigin, PanelUiCapabilitySnapshot,
        };
        let snapshot = PanelUiCapabilitySnapshot::new(
            7,
            vec![PanelUiCapabilityDeclaration {
                descriptor: descriptor(),
                origin: PanelUiCapabilityOrigin::Community,
                plugin_version: "1.2.3".into(),
                asset_integrity: Some("sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".into()),
                public_base: "./tabs/".into(),
                source_revision: 7,
            }],
        );
        let wire = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(
            wire,
            json!({
                "revision": 7,
                "declarations": [{
                    "descriptor": serde_json::to_value(descriptor()).unwrap(),
                    "origin": "community",
                    "plugin_version": "1.2.3",
                    "asset_integrity": "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
                    "public_base": "./tabs/",
                    "source_revision": 7
                }]
            })
        );
        assert_eq!(
            serde_json::from_value::<PanelUiCapabilitySnapshot>(wire).unwrap(),
            snapshot
        );
        assert_eq!(
            serde_json::to_value(PanelUiCapabilityOrigin::BuiltIn).unwrap(),
            json!("builtin")
        );
    }

    #[test]
    fn panel_webchat_source_does_not_define_another_wire_shape() {
        let source = include_str!("../../../interfaces/webchat/src/panel/source.rs");
        for name in [
            "PanelUiCapabilityDeclaration",
            "PanelUiCapabilitySnapshot",
            "PanelUiCapabilityOrigin",
        ] {
            assert!(!source.contains(&format!("struct {name}")));
            assert!(!source.contains(&format!("enum {name}")));
        }
    }

    #[test]
    fn panel_missing_status_detail_deserializes_as_none() {
        let decoded: PanelTabDescriptor = serde_json::from_value(json!({
            "key": {"plugin_id": "example.plugin", "tab_id": "overview"},
            "label": "Overview",
            "icon": "layout",
            "order": 0,
            "entry": "tabs/overview.js",
            "api_major": 1,
            "trust": "trusted",
            "permissions": [],
            "status": "disabled"
        }))
        .unwrap();
        assert_eq!(decoded.status_detail, None);
    }
}
