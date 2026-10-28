//! Immutable Panel UI capability snapshots; no plugin lifecycle or asset loading.

use aleph_protocol::panel::{
    PanelTabDescriptor, PanelTabKey, PanelTabStatus, PanelUiTrust, PANEL_UI_API_MAJOR,
};
use serde::{Deserialize, Serialize};

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

pub trait PanelUiCapabilitySource {
    fn snapshot(&self) -> PanelUiCapabilitySnapshot;
}

pub struct BuiltInTerminalSource {
    label: String,
}

impl BuiltInTerminalSource {
    pub fn new(label: String) -> Self {
        Self { label }
    }
}

impl PanelUiCapabilitySource for BuiltInTerminalSource {
    fn snapshot(&self) -> PanelUiCapabilitySnapshot {
        let descriptor = terminal_descriptor(self.label.clone());
        #[cfg(feature = "panel-tab-spike")]
        let public_base = descriptor
            .entry
            .strip_suffix("terminal-pilot/terminal-pilot.js")
            .unwrap_or("tabs/")
            .into();
        #[cfg(not(feature = "panel-tab-spike"))]
        let public_base = "./tabs/".into();
        PanelUiCapabilitySnapshot {
            revision: 0,
            declarations: vec![PanelUiCapabilityDeclaration {
                descriptor,
                origin: PanelUiCapabilityOrigin::BuiltIn,
                plugin_version: env!("CARGO_PKG_VERSION").into(),
                asset_integrity: None,
                public_base,
                source_revision: 0,
            }],
        }
    }
}

pub struct FixturePanelUiCapabilitySource {
    snapshot: PanelUiCapabilitySnapshot,
}

impl FixturePanelUiCapabilitySource {
    pub fn new(snapshot: PanelUiCapabilitySnapshot) -> Self {
        Self {
            snapshot: PanelUiCapabilitySnapshot::new(snapshot.revision, snapshot.declarations),
        }
    }
}

impl PanelUiCapabilitySource for FixturePanelUiCapabilitySource {
    fn snapshot(&self) -> PanelUiCapabilitySnapshot {
        self.snapshot.clone()
    }
}

pub(super) fn terminal_key() -> PanelTabKey {
    PanelTabKey {
        plugin_id: "aleph".into(),
        #[cfg(feature = "panel-tab-spike")]
        tab_id: "terminal-pilot".into(),
        #[cfg(not(feature = "panel-tab-spike"))]
        tab_id: "terminal".into(),
    }
}

pub(super) fn terminal_descriptor(label: String) -> PanelTabDescriptor {
    #[cfg(feature = "panel-tab-spike")]
    return PanelTabDescriptor {
        key: terminal_key(),
        label,
        icon: "terminal".into(),
        order: 0,
        // The build can project a different asset root without changing the loader.
        entry: option_env!("ALEPH_PANEL_PILOT_ENTRY")
            .unwrap_or("tabs/terminal-pilot/terminal-pilot.js")
            .into(),
        api_major: PANEL_UI_API_MAJOR,
        trust: PanelUiTrust::Verified,
        permissions: vec!["sessions.list".into()],
        status: PanelTabStatus::Enabled,
        status_detail: None,
    };

    #[cfg(not(feature = "panel-tab-spike"))]
    PanelTabDescriptor {
        key: terminal_key(),
        label,
        icon: "terminal".into(),
        order: 0,
        entry: option_env!("PANEL_TERMINAL_ENTRY")
            .unwrap_or("./tabs/terminal/dev/terminal.js")
            .into(),
        api_major: PANEL_UI_API_MAJOR,
        trust: PanelUiTrust::Verified,
        permissions: [
            "pty.list",
            "pty.spawn",
            "pty.attach",
            "pty.input",
            "pty.resize",
            "pty.close",
            "config.get",
            "events.subscribe:pty.screen",
            "events.subscribe:pty.exit",
        ]
        .into_iter()
        .map(String::from)
        .collect(),
        status: PanelTabStatus::Enabled,
        status_detail: None,
    }
}
