use super::registry::{RegistryError, TabRegistry};
use aleph_protocol::panel::{
    PanelTabDescriptor, PanelTabKey, PanelTabStatus, PanelUiTrust, PANEL_UI_API_MAJOR,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelRoute {
    Legacy,
    Modular(PanelTabKey),
    Unavailable,
}

/// Host-owned projection of the descriptors available to the modular Panel.
#[derive(Clone, Debug)]
pub struct PanelProjection {
    registry: TabRegistry,
}

impl PanelProjection {
    /// Build the built-in projection. Plugin projection is added separately.
    pub fn builtins(label: String) -> Result<Self, RegistryError> {
        Self::from_descriptors(vec![terminal_descriptor(label)])
    }

    /// Build a projection from host-owned descriptors after boundary validation.
    fn from_descriptors(descriptors: Vec<PanelTabDescriptor>) -> Result<Self, RegistryError> {
        Ok(Self {
            registry: TabRegistry::new(descriptors)?,
        })
    }

    pub fn registry(&self) -> TabRegistry {
        self.registry.clone()
    }

    /// Resolve a path against an optional projection without letting a malformed
    /// projected route fall through to a legacy mount.
    pub fn select_route(path: &str, projection: Option<&Self>) -> PanelRoute {
        let route = path.strip_suffix('/').unwrap_or(path);
        if route != "/terminal" {
            return PanelRoute::Legacy;
        }
        let Some(projection) = projection else {
            return PanelRoute::Unavailable;
        };
        projection
            .route_key(path)
            .map_or(PanelRoute::Unavailable, PanelRoute::Modular)
    }

    /// Resolve an application route to an enabled projected tab.
    pub fn route_key(&self, path: &str) -> Option<PanelTabKey> {
        let route = path.strip_suffix('/').unwrap_or(path);
        if route != "/terminal" {
            return None;
        }

        let key = terminal_key();
        self.registry
            .get(&key)
            .filter(|descriptor| descriptor.status == PanelTabStatus::Enabled)
            .map(|descriptor| descriptor.key.clone())
    }
}

fn terminal_key() -> PanelTabKey {
    PanelTabKey {
        plugin_id: "aleph".into(),
        #[cfg(feature = "panel-tab-spike")]
        tab_id: "terminal-pilot".into(),
        #[cfg(not(feature = "panel-tab-spike"))]
        tab_id: "terminal".into(),
    }
}

fn terminal_descriptor(label: String) -> PanelTabDescriptor {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_rejects_duplicate_invalid_entry_and_api_mismatch() {
        let descriptor = terminal_descriptor("Terminal".into());
        assert!(matches!(
            PanelProjection::from_descriptors(vec![descriptor.clone(), descriptor]),
            Err(RegistryError::DuplicateKey)
        ));

        let mut invalid_entry = terminal_descriptor("Terminal".into());
        invalid_entry.entry = "https://example.invalid/terminal.js".into();
        assert!(matches!(
            PanelProjection::from_descriptors(vec![invalid_entry]),
            Err(RegistryError::InvalidEntry)
        ));

        let mut invalid_path = terminal_descriptor("Terminal".into());
        invalid_path.entry = "../outside.js".into();
        assert!(matches!(
            PanelProjection::from_descriptors(vec![invalid_path]),
            Err(RegistryError::InvalidEntry)
        ));

        let mut invalid_api = terminal_descriptor("Terminal".into());
        invalid_api.api_major = PANEL_UI_API_MAJOR.saturating_add(1);
        assert!(matches!(
            PanelProjection::from_descriptors(vec![invalid_api]),
            Err(RegistryError::InvalidEntry)
        ));
    }

    #[test]
    fn projected_terminal_descriptor_has_one_complete_host_contract() {
        let projection = PanelProjection::builtins("Terminal".into()).unwrap();
        let descriptor = projection
            .registry()
            .get(&terminal_key())
            .cloned()
            .expect("projected terminal descriptor");

        assert_eq!(descriptor.label, "Terminal");
        assert_eq!(descriptor.icon, "terminal");
        assert_eq!(descriptor.order, 0);
        assert_eq!(descriptor.api_major, PANEL_UI_API_MAJOR);
        assert_eq!(descriptor.trust, PanelUiTrust::Verified);
        assert_eq!(descriptor.status, PanelTabStatus::Enabled);
        assert_eq!(descriptor.status_detail, None);
        assert!(!descriptor.entry.trim().is_empty());

        #[cfg(feature = "panel-tab-spike")]
        assert_eq!(descriptor.key.tab_id, "terminal-pilot");
        #[cfg(not(feature = "panel-tab-spike"))]
        assert_eq!(descriptor.key.tab_id, "terminal");

        #[cfg(feature = "panel-tab-spike")]
        assert_eq!(descriptor.permissions, vec!["sessions.list"]);
        #[cfg(not(feature = "panel-tab-spike"))]
        assert_eq!(
            descriptor.permissions,
            [
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
            .collect::<Vec<_>>()
        );
    }

    #[test]
    fn only_projected_terminal_routes_select_a_mountable_descriptor() {
        let projection = PanelProjection::builtins("Terminal".into()).unwrap();
        assert_eq!(projection.route_key("/terminal"), Some(terminal_key()));
        assert_eq!(projection.route_key("/terminal/"), Some(terminal_key()));
        assert_eq!(projection.route_key("/panel/terminal"), None);
        assert_eq!(projection.route_key("/dashboard"), None);
        assert_eq!(
            PanelProjection::select_route("/terminal", Some(&projection)),
            PanelRoute::Modular(terminal_key())
        );
        assert_eq!(
            PanelProjection::select_route("/terminal", None),
            PanelRoute::Unavailable
        );
        assert_eq!(
            PanelProjection::select_route("/dashboard", None),
            PanelRoute::Legacy
        );
    }

    #[test]
    fn route_selection_fails_closed_for_non_enabled_projection_status() {
        for status in [
            PanelTabStatus::Disabled,
            PanelTabStatus::Incompatible,
            PanelTabStatus::Failed,
        ] {
            let mut descriptor = terminal_descriptor("Terminal".into());
            descriptor.status = status;
            let projection = PanelProjection::from_descriptors(vec![descriptor]).unwrap();
            assert_eq!(projection.route_key("/terminal"), None);
        }
    }
}
