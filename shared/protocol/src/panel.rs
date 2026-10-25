//! Shared Panel Tab descriptors for host and client projections.

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
