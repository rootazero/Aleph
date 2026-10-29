use super::registry::{validate_descriptor, RegistryError, TabRegistry, BUILTIN_NAMESPACE};
#[cfg(test)]
use super::source::terminal_descriptor;
use super::source::{
    terminal_key, BuiltInTerminalSource, PanelUiCapabilityDeclaration, PanelUiCapabilityOrigin,
    PanelUiCapabilitySnapshot, PanelUiCapabilitySource,
};
#[cfg(test)]
use aleph_protocol::panel::PANEL_UI_API_MAJOR;
use aleph_protocol::panel::{
    supported_community_permission, valid_asset_integrity, valid_identity_segment,
};
use aleph_protocol::panel::{PanelTabDescriptor, PanelTabKey, PanelTabStatus, PanelUiTrust};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelRoute {
    Legacy,
    Modular(PanelTabKey),
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PanelProjectionError {
    InvalidIdentity,
    InvalidOrigin,
    UnacceptableTrust,
    UnsupportedPermission,
    MissingAssetIntegrity,
    InvalidAssetIntegrity,
    InvalidEntry,
    InvalidPublicBase,
    UnversionedEntry,
    RevisionMismatch,
    DuplicateKey,
    BuiltInShadowed,
    StaleSnapshot,
}

impl PanelProjectionError {
    pub const fn reason_code(self) -> &'static str {
        match self {
            Self::InvalidIdentity => "invalid_identity",
            Self::InvalidOrigin => "invalid_origin",
            Self::UnacceptableTrust => "unacceptable_trust",
            Self::UnsupportedPermission => "unsupported_permission",
            Self::MissingAssetIntegrity => "missing_asset_integrity",
            Self::InvalidAssetIntegrity => "invalid_asset_integrity",
            Self::InvalidEntry => "invalid_entry",
            Self::InvalidPublicBase => "invalid_public_base",
            Self::UnversionedEntry => "unversioned_entry",
            Self::RevisionMismatch => "revision_mismatch",
            Self::DuplicateKey => "duplicate_key",
            Self::BuiltInShadowed => "built_in_shadowed",
            Self::StaleSnapshot => "stale_snapshot",
        }
    }
}

/// Host-owned projection of the descriptors available to the modular Panel.
#[derive(Clone, Debug)]
pub struct PanelProjection {
    registry: TabRegistry,
}

impl PanelProjection {
    /// Build the built-in projection. Plugin projection is added separately.
    pub fn builtins(label: String) -> Result<Self, RegistryError> {
        let snapshot = BuiltInTerminalSource::new(label).snapshot();
        Self::from_descriptors(
            snapshot
                .declarations
                .into_iter()
                .map(|declaration| declaration.descriptor)
                .collect(),
        )
    }

    pub fn from_snapshot(
        snapshot: PanelUiCapabilitySnapshot,
    ) -> Result<Self, PanelProjectionError> {
        Ok(Self {
            registry: Self::registry_from_snapshot(snapshot)?,
        })
    }

    /// Atomically replace the projection only with a newer, fully valid snapshot.
    pub fn replace_snapshot(
        &mut self,
        snapshot: PanelUiCapabilitySnapshot,
    ) -> Result<(), PanelProjectionError> {
        if snapshot.revision <= self.registry.source_revision() {
            return Err(PanelProjectionError::StaleSnapshot);
        }
        let registry = Self::registry_from_snapshot(snapshot)?;
        self.registry = registry;
        Ok(())
    }

    fn registry_from_snapshot(
        snapshot: PanelUiCapabilitySnapshot,
    ) -> Result<TabRegistry, PanelProjectionError> {
        if snapshot
            .declarations
            .iter()
            .any(|declaration| declaration.source_revision != snapshot.revision)
        {
            return Err(PanelProjectionError::RevisionMismatch);
        }

        let snapshot = PanelUiCapabilitySnapshot::new(snapshot.revision, snapshot.declarations);
        let mut registry = TabRegistry::default();
        for declaration in snapshot.declarations {
            validate_declaration(&declaration)?;
            let key = declaration.descriptor.key.clone();
            let integrity = declaration.asset_integrity.clone();
            if registry.get(&key).is_some() {
                return Err(PanelProjectionError::DuplicateKey);
            }
            match declaration.origin {
                PanelUiCapabilityOrigin::BuiltIn => registry
                    .insert_builtin(declaration.descriptor)
                    .map_err(map_registry_error)?,
                PanelUiCapabilityOrigin::Community => registry
                    .insert_or_replace(declaration.descriptor)
                    .map_err(map_registry_error)?,
            }
            registry.set_asset_integrity(&key, integrity);
        }
        registry.set_source_revision(snapshot.revision);
        Ok(registry)
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

fn map_registry_error(error: RegistryError) -> PanelProjectionError {
    match error {
        RegistryError::BuiltInShadowed => PanelProjectionError::BuiltInShadowed,
        RegistryError::DuplicateKey => PanelProjectionError::DuplicateKey,
        RegistryError::InvalidEntry => PanelProjectionError::InvalidEntry,
    }
}

fn validate_declaration(
    declaration: &PanelUiCapabilityDeclaration,
) -> Result<(), PanelProjectionError> {
    let descriptor = &declaration.descriptor;
    if !valid_identity_segment(&descriptor.key.plugin_id)
        || !valid_identity_segment(&descriptor.key.tab_id)
    {
        return Err(PanelProjectionError::InvalidIdentity);
    }
    match declaration.origin {
        PanelUiCapabilityOrigin::BuiltIn if descriptor.key.plugin_id != BUILTIN_NAMESPACE => {
            return Err(PanelProjectionError::InvalidOrigin);
        }
        PanelUiCapabilityOrigin::Community if descriptor.key.plugin_id == BUILTIN_NAMESPACE => {
            return Err(PanelProjectionError::BuiltInShadowed);
        }
        _ => {}
    }
    validate_descriptor(descriptor).map_err(|_| PanelProjectionError::InvalidEntry)?;

    let expected_trust = match declaration.origin {
        PanelUiCapabilityOrigin::BuiltIn => PanelUiTrust::Verified,
        PanelUiCapabilityOrigin::Community => PanelUiTrust::Trusted,
    };
    if descriptor.trust != expected_trust {
        return Err(PanelProjectionError::UnacceptableTrust);
    }
    if declaration.origin == PanelUiCapabilityOrigin::Community
        && descriptor
            .permissions
            .iter()
            .any(|permission| !supported_community_permission(permission))
    {
        return Err(PanelProjectionError::UnsupportedPermission);
    }
    match declaration.origin {
        PanelUiCapabilityOrigin::BuiltIn => {}
        PanelUiCapabilityOrigin::Community if declaration.asset_integrity.is_none() => {
            return Err(PanelProjectionError::MissingAssetIntegrity);
        }
        PanelUiCapabilityOrigin::Community => {}
    }
    if declaration
        .asset_integrity
        .as_deref()
        .is_some_and(|integrity| !valid_asset_integrity(integrity))
    {
        return Err(PanelProjectionError::InvalidAssetIntegrity);
    }
    if !valid_public_base(declaration) {
        return Err(PanelProjectionError::InvalidPublicBase);
    }
    if declaration.origin == PanelUiCapabilityOrigin::Community {
        if !descriptor
            .entry
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/'))
        {
            return Err(PanelProjectionError::InvalidEntry);
        }
        if !valid_identity_segment(&declaration.plugin_version)
            || matches!(declaration.plugin_version.as_str(), "." | "..")
        {
            return Err(PanelProjectionError::UnversionedEntry);
        }
        let prefix = format!(
            "./tabs/{}/{}/",
            descriptor.key.plugin_id, declaration.plugin_version
        );
        if !descriptor.entry.starts_with(&prefix) || descriptor.entry.len() == prefix.len() {
            return Err(PanelProjectionError::UnversionedEntry);
        }
    }
    Ok(())
}

fn valid_public_base(declaration: &PanelUiCapabilityDeclaration) -> bool {
    let base = declaration.public_base.as_str();
    let supported = match declaration.origin {
        PanelUiCapabilityOrigin::BuiltIn => {
            base.ends_with('/')
                && !base.starts_with('/')
                && base.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/')
                })
                && !base.split('/').any(|segment| segment == "..")
        }
        PanelUiCapabilityOrigin::Community => base == "./tabs/",
    };
    supported && declaration.descriptor.entry.starts_with(base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panel::source::{
        BuiltInTerminalSource, FixturePanelUiCapabilitySource, PanelUiCapabilityDeclaration,
        PanelUiCapabilityOrigin, PanelUiCapabilitySnapshot, PanelUiCapabilitySource,
    };

    fn community_declaration(revision: u64) -> PanelUiCapabilityDeclaration {
        PanelUiCapabilityDeclaration {
            descriptor: PanelTabDescriptor {
                key: PanelTabKey {
                    plugin_id: "community.example".into(),
                    tab_id: "overview".into(),
                },
                label: "Overview".into(),
                icon: "layout".into(),
                order: 10,
                entry: "./tabs/community.example/1.2.3/overview.js".into(),
                api_major: PANEL_UI_API_MAJOR,
                trust: PanelUiTrust::Trusted,
                permissions: vec!["sessions.list".into()],
                status: PanelTabStatus::Enabled,
                status_detail: None,
            },
            origin: PanelUiCapabilityOrigin::Community,
            plugin_version: "1.2.3".into(),
            asset_integrity: Some("sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".into()),
            public_base: "./tabs/".into(),
            source_revision: revision,
        }
    }

    fn community_snapshot(
        revision: u64,
        community: Vec<PanelUiCapabilityDeclaration>,
    ) -> PanelUiCapabilitySnapshot {
        let mut declarations = BuiltInTerminalSource::new("Terminal".into())
            .snapshot()
            .declarations;
        declarations.extend(community);
        for declaration in &mut declarations {
            declaration.source_revision = revision;
        }
        PanelUiCapabilitySnapshot::new(revision, declarations)
    }

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
        assert_eq!(projection.registry().asset_integrity(&terminal_key()), None);

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
    fn capability_sources_expose_stable_descriptor_snapshots() {
        let baseline = PanelProjection::builtins("Terminal".into()).unwrap();
        let builtin = BuiltInTerminalSource::new("Terminal".into()).snapshot();
        assert_eq!(builtin.revision, 0);
        assert_eq!(builtin.declarations.len(), 1);
        assert_eq!(
            builtin.declarations[0].descriptor,
            baseline.registry().get(&terminal_key()).cloned().unwrap()
        );
        assert_eq!(builtin.declarations[0].source_revision, builtin.revision);

        let fixture = FixturePanelUiCapabilitySource::new(builtin.clone());
        assert_eq!(fixture.snapshot(), builtin);
        assert_eq!(
            PanelProjection::from_snapshot(builtin)
                .unwrap()
                .registry()
                .get(&terminal_key())
                .cloned()
                .unwrap(),
            baseline.registry().get(&terminal_key()).cloned().unwrap()
        );
    }

    #[test]
    fn valid_community_declaration_projects_in_deterministic_source_order() {
        let mut other = community_declaration(7);
        other.descriptor.key.plugin_id = "community.alpha".into();
        other.descriptor.key.tab_id = "alpha".into();
        other.descriptor.entry = "./tabs/community.alpha/1.2.3/alpha.js".into();
        let snapshot = community_snapshot(7, vec![community_declaration(7), other]);
        assert_eq!(
            snapshot
                .declarations
                .iter()
                .map(|declaration| declaration.descriptor.key.qualified())
                .collect::<Vec<_>>(),
            vec![
                terminal_key().qualified(),
                "community.alpha:alpha".into(),
                "community.example:overview".into()
            ]
        );

        let projection = PanelProjection::from_snapshot(snapshot).unwrap();
        let community = projection
            .registry()
            .get(&community_declaration(7).descriptor.key)
            .cloned()
            .unwrap();
        assert_eq!(community, community_declaration(7).descriptor);
        assert_eq!(
            projection
                .registry()
                .asset_integrity(&community_declaration(7).descriptor.key),
            Some("sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
        );
        assert_eq!(projection.registry().source_revision(), 7);
        assert_eq!(projection.route_key("/terminal"), Some(terminal_key()));
    }

    #[test]
    fn community_validation_rejects_unsupported_facts_with_stable_reason_codes() {
        let cases = [
            (
                PanelUiCapabilityDeclaration {
                    descriptor: PanelTabDescriptor {
                        key: PanelTabKey {
                            plugin_id: String::new(),
                            ..community_declaration(1).descriptor.key
                        },
                        ..community_declaration(1).descriptor
                    },
                    ..community_declaration(1)
                },
                PanelProjectionError::InvalidIdentity,
                "invalid_identity",
            ),
            (
                PanelUiCapabilityDeclaration {
                    origin: PanelUiCapabilityOrigin::BuiltIn,
                    ..community_declaration(1)
                },
                PanelProjectionError::InvalidOrigin,
                "invalid_origin",
            ),
            (
                PanelUiCapabilityDeclaration {
                    descriptor: PanelTabDescriptor {
                        trust: PanelUiTrust::Verified,
                        ..community_declaration(1).descriptor
                    },
                    ..community_declaration(1)
                },
                PanelProjectionError::UnacceptableTrust,
                "unacceptable_trust",
            ),
            (
                PanelUiCapabilityDeclaration {
                    descriptor: PanelTabDescriptor {
                        permissions: vec!["gateway.admin".into()],
                        ..community_declaration(1).descriptor
                    },
                    ..community_declaration(1)
                },
                PanelProjectionError::UnsupportedPermission,
                "unsupported_permission",
            ),
            (
                PanelUiCapabilityDeclaration {
                    asset_integrity: None,
                    ..community_declaration(1)
                },
                PanelProjectionError::MissingAssetIntegrity,
                "missing_asset_integrity",
            ),
            (
                PanelUiCapabilityDeclaration {
                    asset_integrity: Some("sha256-not-a-digest".into()),
                    ..community_declaration(1)
                },
                PanelProjectionError::InvalidAssetIntegrity,
                "invalid_asset_integrity",
            ),
            (
                PanelUiCapabilityDeclaration {
                    public_base: "/tabs/".into(),
                    ..community_declaration(1)
                },
                PanelProjectionError::InvalidPublicBase,
                "invalid_public_base",
            ),
            (
                PanelUiCapabilityDeclaration {
                    descriptor: PanelTabDescriptor {
                        entry: "https://example.invalid/overview.js".into(),
                        ..community_declaration(1).descriptor
                    },
                    ..community_declaration(1)
                },
                PanelProjectionError::InvalidEntry,
                "invalid_entry",
            ),
            (
                PanelUiCapabilityDeclaration {
                    descriptor: PanelTabDescriptor {
                        entry: "./tabs/community.example/../outside.js".into(),
                        ..community_declaration(1).descriptor
                    },
                    ..community_declaration(1)
                },
                PanelProjectionError::InvalidEntry,
                "invalid_entry",
            ),
            (
                PanelUiCapabilityDeclaration {
                    descriptor: PanelTabDescriptor {
                        entry: "./tabs/community.example/1.2.3/%2e%2e/outside.js".into(),
                        ..community_declaration(1).descriptor
                    },
                    ..community_declaration(1)
                },
                PanelProjectionError::InvalidEntry,
                "invalid_entry",
            ),
            (
                PanelUiCapabilityDeclaration {
                    descriptor: PanelTabDescriptor {
                        entry: "./tabs/community.example/1.2.3/overview.js?raw=1".into(),
                        ..community_declaration(1).descriptor
                    },
                    ..community_declaration(1)
                },
                PanelProjectionError::InvalidEntry,
                "invalid_entry",
            ),
            (
                PanelUiCapabilityDeclaration {
                    descriptor: PanelTabDescriptor {
                        entry: "./tabs/community.example/1.2.3/overview.js#module".into(),
                        ..community_declaration(1).descriptor
                    },
                    ..community_declaration(1)
                },
                PanelProjectionError::InvalidEntry,
                "invalid_entry",
            ),
            (
                PanelUiCapabilityDeclaration {
                    descriptor: PanelTabDescriptor {
                        entry: "./tabs/community.example/2.0.0/overview.js".into(),
                        ..community_declaration(1).descriptor
                    },
                    ..community_declaration(1)
                },
                PanelProjectionError::UnversionedEntry,
                "unversioned_entry",
            ),
        ];

        for (declaration, expected, reason_code) in cases {
            let error = PanelProjection::from_snapshot(community_snapshot(1, vec![declaration]))
                .unwrap_err();
            assert_eq!(error, expected);
            assert_eq!(error.reason_code(), reason_code);
        }
    }

    #[test]
    fn snapshot_validation_rejects_duplicates_collisions_and_revision_mismatch() {
        assert_eq!(
            PanelProjection::from_snapshot(community_snapshot(
                2,
                vec![community_declaration(2), community_declaration(2)],
            ))
            .unwrap_err(),
            PanelProjectionError::DuplicateKey
        );

        let collision = PanelUiCapabilityDeclaration {
            descriptor: terminal_descriptor("Imposter".into()),
            origin: PanelUiCapabilityOrigin::Community,
            plugin_version: "1.2.3".into(),
            asset_integrity: community_declaration(2).asset_integrity,
            public_base: "./tabs/".into(),
            source_revision: 2,
        };
        assert_eq!(
            PanelProjection::from_snapshot(community_snapshot(2, vec![collision])).unwrap_err(),
            PanelProjectionError::BuiltInShadowed
        );

        let mut mismatched = community_snapshot(3, vec![community_declaration(3)]);
        mismatched.declarations[1].source_revision = 2;
        assert_eq!(
            PanelProjection::from_snapshot(mismatched).unwrap_err(),
            PanelProjectionError::RevisionMismatch
        );
    }

    #[test]
    fn snapshot_replacement_fences_stale_and_rejected_late_results() {
        let mut projection =
            PanelProjection::from_snapshot(community_snapshot(2, vec![community_declaration(2)]))
                .unwrap();
        projection
            .replace_snapshot(community_snapshot(3, vec![]))
            .unwrap();
        assert_eq!(projection.registry().source_revision(), 3);
        assert!(projection
            .registry()
            .get(&community_declaration(2).descriptor.key)
            .is_none());

        let baseline = projection.registry();
        assert_eq!(
            baseline.asset_integrity(&community_declaration(2).descriptor.key),
            None
        );
        for late in [
            community_snapshot(2, vec![community_declaration(2)]),
            community_snapshot(3, vec![community_declaration(3)]),
        ] {
            assert_eq!(
                projection.replace_snapshot(late).unwrap_err(),
                PanelProjectionError::StaleSnapshot
            );
            assert_eq!(projection.registry().rows(), baseline.rows());
            assert_eq!(projection.registry().source_revision(), 3);
        }

        let mut replacement = community_declaration(4);
        replacement.descriptor.key = community_declaration(2).descriptor.key;
        replacement.asset_integrity =
            Some("sha256-LXEWQrcmsEQBYnyp+6wy9chTD7GQPMTbAiWHF5IaSIE=".into());
        projection
            .replace_snapshot(community_snapshot(4, vec![replacement.clone()]))
            .unwrap();
        assert_eq!(
            projection
                .registry()
                .asset_integrity(&replacement.descriptor.key),
            replacement.asset_integrity.as_deref()
        );

        let rejected = PanelUiCapabilityDeclaration {
            asset_integrity: None,
            ..community_declaration(5)
        };
        assert_eq!(
            projection
                .replace_snapshot(community_snapshot(5, vec![rejected]))
                .unwrap_err(),
            PanelProjectionError::MissingAssetIntegrity
        );
        assert_eq!(projection.registry().source_revision(), 4);
        assert_eq!(
            projection
                .registry()
                .asset_integrity(&replacement.descriptor.key),
            replacement.asset_integrity.as_deref()
        );
    }

    #[test]
    fn unknown_wire_trust_fails_closed_before_projection() {
        let mut value =
            serde_json::to_value(community_snapshot(1, vec![community_declaration(1)])).unwrap();
        value["declarations"][1]["descriptor"]["trust"] = serde_json::json!("unknown");
        assert!(serde_json::from_value::<PanelUiCapabilitySnapshot>(value).is_err());
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
