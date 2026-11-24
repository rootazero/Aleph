//! Pure derivation of Panel declarations from admitted extension state.
//!
//! This module owns no browser, asset, or lifecycle effects. It converts the
//! host-owned plugin record plus its explicit manifest section into the one
//! shared protocol declaration shape. Lifecycle code supplies the revision and
//! publishes the resulting snapshot through its existing projection chokepoint.

use std::collections::{HashMap, HashSet};

use aleph_protocol::panel::{
    valid_identity_segment, valid_relative_entry, valid_version_segment, PanelTabDescriptor,
    PanelTabKey, PanelTabStatus, PanelUiCapabilityDeclaration, PanelUiCapabilityOrigin,
    PanelUiCapabilitySnapshot, PanelUiTrust,
};

use super::plugin_trust::OwnerTrustPolicy;
use super::types::PluginRecord;

const COMMUNITY_PUBLIC_BASE: &str = "./tabs/";
const BUILTIN_NAMESPACE: &str = "aleph";

/// Stable reasons why an explicit Panel declaration cannot be projected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PanelDeclarationError {
    OwnerTrustRejected,
    InvalidPluginId,
    ReservedNamespace,
    MissingVersion,
    InvalidManifest(&'static str),
}

impl PanelDeclarationError {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::OwnerTrustRejected => "panel.owner_trust_rejected",
            Self::InvalidPluginId => "panel.invalid_plugin_id",
            Self::ReservedNamespace => "panel.reserved_namespace",
            Self::MissingVersion => "panel.missing_version",
            Self::InvalidManifest(reason) => reason,
        }
    }
}

/// One omitted declaration and its stable host diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PanelProjectionDiagnostic {
    pub(crate) plugin_id: String,
    pub(crate) code: &'static str,
}

/// The canonical snapshot plus diagnostics for declarations omitted during
/// fail-closed discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PanelSnapshotProjection {
    pub(crate) snapshot: PanelUiCapabilitySnapshot,
    pub(crate) diagnostics: Vec<PanelProjectionDiagnostic>,
}

/// Derive one Panel declaration from a host-admitted plugin record.
///
/// A generic plugin record without an explicit `panel` section is intentionally
/// a no-op. The manifest never selects trust, status, origin, or the asset
/// base: those facts come from the record and host policy.
pub(crate) fn derive_panel_declaration(
    record: &PluginRecord,
    trust_policy: &OwnerTrustPolicy,
    source_revision: u64,
) -> Result<Option<PanelUiCapabilityDeclaration>, PanelDeclarationError> {
    if !record.status.is_active() {
        return Ok(None);
    }
    let Some(panel) = record.panel.as_ref() else {
        return Ok(None);
    };
    if !trust_policy.allows(&record.id, record.origin) {
        return Err(PanelDeclarationError::OwnerTrustRejected);
    }
    if !valid_identity_segment(&record.id) {
        return Err(PanelDeclarationError::InvalidPluginId);
    }
    if record.id == BUILTIN_NAMESPACE {
        return Err(PanelDeclarationError::ReservedNamespace);
    }
    let Some(version) = record
        .version
        .as_deref()
        .filter(|version| !version.trim().is_empty())
    else {
        return Err(PanelDeclarationError::MissingVersion);
    };
    if !valid_version_segment(version) {
        return Err(PanelDeclarationError::InvalidManifest(
            "panel.invalid_version",
        ));
    }
    panel
        .validate()
        .map_err(PanelDeclarationError::InvalidManifest)?;

    let entry = format!(
        "{COMMUNITY_PUBLIC_BASE}{}/{}/{}",
        record.id, version, panel.entry
    );
    if !valid_relative_entry(&entry) {
        return Err(PanelDeclarationError::InvalidManifest(
            "panel.invalid_entry",
        ));
    }

    Ok(Some(PanelUiCapabilityDeclaration {
        descriptor: PanelTabDescriptor {
            key: PanelTabKey {
                plugin_id: record.id.clone(),
                tab_id: panel.tab_id.clone(),
            },
            label: panel.label.clone(),
            icon: panel.icon.clone(),
            order: panel.order,
            entry,
            api_major: panel.api_major,
            trust: PanelUiTrust::Trusted,
            permissions: panel.permissions.clone(),
            status: PanelTabStatus::Enabled,
            status_detail: None,
        },
        origin: PanelUiCapabilityOrigin::Community,
        plugin_version: version.to_owned(),
        asset_integrity: Some(panel.asset_integrity.clone()),
        public_base: COMMUNITY_PUBLIC_BASE.to_owned(),
        source_revision,
    }))
}

/// Derive a complete snapshot while omitting invalid or colliding rows.
///
/// Duplicate qualified keys are removed as a group, so registry iteration
/// order cannot decide which plugin declaration wins. The caller can surface
/// `diagnostics` without exposing an unsafe partial row to the browser.
#[must_use]
pub(crate) fn derive_panel_snapshot(
    records: &[PluginRecord],
    trust_policy: &OwnerTrustPolicy,
    revision: u64,
) -> PanelSnapshotProjection {
    let mut declarations = Vec::new();
    let mut diagnostics = Vec::new();
    for record in records {
        match derive_panel_declaration(record, trust_policy, revision) {
            Ok(Some(declaration)) => declarations.push(declaration),
            Ok(None) => {}
            Err(error) => diagnostics.push(PanelProjectionDiagnostic {
                plugin_id: record.id.clone(),
                code: error.code(),
            }),
        }
    }

    let mut key_counts = HashMap::new();
    for declaration in &declarations {
        *key_counts
            .entry(declaration.descriptor.key.qualified())
            .or_insert(0usize) += 1;
    }
    let duplicate_keys: HashSet<_> = key_counts
        .into_iter()
        .filter_map(|(key, count)| (count > 1).then_some(key))
        .collect();
    for key in &duplicate_keys {
        let plugin_id = key.split(':').next().unwrap_or_default().to_owned();
        diagnostics.push(PanelProjectionDiagnostic {
            plugin_id,
            code: "panel.duplicate_key",
        });
    }
    declarations
        .retain(|declaration| !duplicate_keys.contains(&declaration.descriptor.key.qualified()));

    PanelSnapshotProjection {
        snapshot: PanelUiCapabilitySnapshot::new(revision, declarations),
        diagnostics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::manifest::PanelUiManifest;
    use crate::extension::types::{PluginOrigin, PluginStatus};

    fn panel() -> PanelUiManifest {
        PanelUiManifest {
            tab_id: "overview".into(),
            label: "Overview".into(),
            icon: "layout".into(),
            order: 10,
            entry: "overview.js".into(),
            api_major: aleph_protocol::panel::PANEL_UI_API_MAJOR,
            permissions: vec!["sessions.list".into()],
            asset_integrity: "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".into(),
        }
    }

    fn record(id: &str, version: Option<&str>) -> PluginRecord {
        let mut record = PluginRecord::new(
            id.into(),
            id.into(),
            crate::extension::types::PluginKind::Wasm,
            PluginOrigin::Workspace,
        );
        record.version = version.map(str::to_owned);
        record.panel = Some(panel());
        record
    }

    #[test]
    fn derives_host_owned_facts_and_never_reads_generic_entry() {
        let record = record("community.example", Some("1.2.3"));
        let declaration = derive_panel_declaration(&record, &OwnerTrustPolicy::permissive(), 7)
            .unwrap()
            .unwrap();

        assert_eq!(declaration.origin, PanelUiCapabilityOrigin::Community);
        assert_eq!(declaration.descriptor.trust, PanelUiTrust::Trusted);
        assert_eq!(declaration.descriptor.status, PanelTabStatus::Enabled);
        assert_eq!(declaration.plugin_version, "1.2.3");
        assert_eq!(declaration.source_revision, 7);
        assert_eq!(
            declaration.descriptor.entry,
            "./tabs/community.example/1.2.3/overview.js"
        );
        assert_eq!(declaration.public_base, "./tabs/");
    }

    #[test]
    fn accepts_semver_build_metadata_in_the_canonical_entry() {
        let declaration = derive_panel_declaration(
            &record("community.example", Some("1.2.3+build.7")),
            &OwnerTrustPolicy::permissive(),
            1,
        )
        .unwrap()
        .unwrap();
        assert_eq!(declaration.plugin_version, "1.2.3+build.7");
        assert_eq!(
            declaration.descriptor.entry,
            "./tabs/community.example/1.2.3+build.7/overview.js"
        );
    }

    #[test]
    fn generic_or_inactive_plugins_do_not_produce_panel_rows() {
        let mut generic = record("generic", Some("1.0.0"));
        generic.panel = None;
        assert_eq!(
            derive_panel_declaration(&generic, &OwnerTrustPolicy::permissive(), 1).unwrap(),
            None
        );

        let mut disabled = record("disabled", Some("1.0.0"));
        disabled.status = PluginStatus::Disabled;
        assert_eq!(
            derive_panel_declaration(&disabled, &OwnerTrustPolicy::permissive(), 1).unwrap(),
            None
        );
    }

    #[test]
    fn missing_version_and_untrusted_origin_fail_closed_with_stable_codes() {
        let missing = record("missing-version", None);
        assert_eq!(
            derive_panel_declaration(&missing, &OwnerTrustPolicy::permissive(), 1)
                .unwrap_err()
                .code(),
            "panel.missing_version"
        );

        let untrusted = record("untrusted", Some("1.0.0"));
        assert_eq!(
            derive_panel_declaration(
                &untrusted,
                &OwnerTrustPolicy::restrictive(Vec::<String>::new()),
                1,
            )
            .unwrap_err()
            .code(),
            "panel.owner_trust_rejected"
        );
    }

    #[test]
    fn snapshot_omits_invalid_rows_and_duplicate_keys_deterministically() {
        let first = record("same", Some("1.0.0"));
        let second = record("same", Some("2.0.0"));
        let valid = record("valid", Some("1.0.0"));
        let projection =
            derive_panel_snapshot(&[second, valid, first], &OwnerTrustPolicy::permissive(), 4);

        assert_eq!(projection.snapshot.revision, 4);
        assert_eq!(projection.snapshot.declarations.len(), 1);
        assert_eq!(
            projection.snapshot.declarations[0].descriptor.key.plugin_id,
            "valid"
        );
        assert!(projection
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "panel.duplicate_key"));
    }
}
