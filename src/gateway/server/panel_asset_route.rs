use std::collections::BTreeMap;

use aleph_protocol::panel::{
    PanelTabStatus, PanelUiCapabilityDeclaration, PanelUiCapabilityOrigin, PanelUiTrust,
    valid_asset_integrity, valid_identity_segment, valid_relative_entry, valid_version_segment,
};
use base64::Engine;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct PanelAssetIdentity {
    pub(crate) plugin_id: String,
    pub(crate) plugin_version: String,
    pub(crate) tab_id: String,
    pub(crate) source_revision: u64,
}

impl PanelAssetIdentity {
    pub(crate) fn from_declaration(declaration: &PanelUiCapabilityDeclaration) -> Self {
        Self::new(
            declaration.descriptor.key.plugin_id.clone(),
            declaration.plugin_version.clone(),
            declaration.descriptor.key.tab_id.clone(),
            declaration.source_revision,
        )
    }

    pub(crate) fn new(
        plugin_id: String,
        plugin_version: String,
        tab_id: String,
        source_revision: u64,
    ) -> Self {
        Self {
            plugin_id,
            plugin_version,
            tab_id,
            source_revision,
        }
    }

    fn same_slot(&self, other: &Self) -> bool {
        self.plugin_id == other.plugin_id
            && self.plugin_version == other.plugin_version
            && self.tab_id == other.tab_id
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PanelAssetRequest {
    pub(crate) identity: PanelAssetIdentity,
    pub(crate) entry: String,
    pub(crate) authorized: bool,
}

impl PanelAssetRequest {
    pub(crate) fn from_declaration(declaration: &PanelUiCapabilityDeclaration) -> Self {
        Self::new(
            PanelAssetIdentity::from_declaration(declaration),
            declaration.descriptor.entry.clone(),
            true,
        )
    }

    pub(crate) fn new(identity: PanelAssetIdentity, entry: String, authorized: bool) -> Self {
        Self {
            identity,
            entry,
            authorized,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PanelAssetPublicationError {
    InvalidDeclaration,
    IntegrityMismatch,
    Unauthorized,
    Missing,
    EntryMismatch,
    Retracted,
}

#[derive(Clone)]
struct PanelAssetPublication {
    declaration: PanelUiCapabilityDeclaration,
    bytes: Vec<u8>,
}

#[derive(Default)]
pub(crate) struct PanelAssetPublicationStore {
    records: BTreeMap<PanelAssetIdentity, PanelAssetPublication>,
}

impl PanelAssetPublicationStore {
    pub(crate) fn publish(
        &mut self,
        declaration: PanelUiCapabilityDeclaration,
        bytes: Vec<u8>,
    ) -> Result<PanelAssetIdentity, PanelAssetPublicationError> {
        if !valid_declaration(&declaration) {
            return Err(PanelAssetPublicationError::InvalidDeclaration);
        }
        if !matches_integrity(&bytes, declaration.asset_integrity.as_deref().unwrap()) {
            return Err(PanelAssetPublicationError::IntegrityMismatch);
        }

        let identity = PanelAssetIdentity::from_declaration(&declaration);
        self.records
            .retain(|existing, _| !existing.same_slot(&identity));
        self.records.insert(
            identity.clone(),
            PanelAssetPublication { declaration, bytes },
        );
        Ok(identity)
    }

    pub(crate) fn retract(&mut self, identity: &PanelAssetIdentity) -> bool {
        self.records.remove(identity).is_some()
    }

    pub(crate) fn serve(
        &self,
        request: &PanelAssetRequest,
    ) -> Result<Vec<u8>, PanelAssetPublicationError> {
        if !request.authorized {
            return Err(PanelAssetPublicationError::Unauthorized);
        }
        let Some(publication) = self.records.get(&request.identity) else {
            return Err(PanelAssetPublicationError::Missing);
        };
        if publication.declaration.descriptor.entry != request.entry {
            return Err(PanelAssetPublicationError::EntryMismatch);
        }
        if !valid_relative_entry(&request.entry) {
            return Err(PanelAssetPublicationError::EntryMismatch);
        }
        Ok(publication.bytes.clone())
    }
}

fn valid_declaration(declaration: &PanelUiCapabilityDeclaration) -> bool {
    declaration.origin == PanelUiCapabilityOrigin::Community
        && declaration.public_base == "./tabs/"
        && declaration.descriptor.status == PanelTabStatus::Enabled
        && declaration.descriptor.trust == PanelUiTrust::Trusted
        && valid_identity_segment(&declaration.descriptor.key.plugin_id)
        && valid_identity_segment(&declaration.descriptor.key.tab_id)
        && valid_version_segment(&declaration.plugin_version)
        && valid_relative_entry(&declaration.descriptor.entry)
        && valid_asset_integrity(declaration.asset_integrity.as_deref().unwrap_or_default())
        && canonical_entry(declaration)
}

fn canonical_entry(declaration: &PanelUiCapabilityDeclaration) -> bool {
    let prefix = format!(
        "./tabs/{}/{}/",
        declaration.descriptor.key.plugin_id, declaration.plugin_version
    );
    declaration
        .descriptor
        .entry
        .strip_prefix(&prefix)
        .is_some_and(|entry| valid_relative_entry(entry))
}

fn matches_integrity(bytes: &[u8], integrity: &str) -> bool {
    let Some(encoded) = integrity.strip_prefix("sha256-") else {
        return false;
    };
    let Ok(expected) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
        return false;
    };
    expected.as_slice() == Sha256::digest(bytes).as_slice()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::panel::{
        PANEL_UI_API_MAJOR, PanelTabDescriptor, PanelTabKey, PanelUiCapabilityOrigin,
    };

    const ENTRY: &str = "./tabs/community.example/1.2.3/overview.js";

    fn declaration(revision: u64, bytes: &[u8]) -> PanelUiCapabilityDeclaration {
        let digest = Sha256::digest(bytes);
        PanelUiCapabilityDeclaration {
            descriptor: PanelTabDescriptor {
                key: PanelTabKey {
                    plugin_id: "community.example".into(),
                    tab_id: "overview".into(),
                },
                label: "Overview".into(),
                icon: "layout".into(),
                order: 10,
                entry: ENTRY.into(),
                api_major: PANEL_UI_API_MAJOR,
                trust: PanelUiTrust::Trusted,
                permissions: vec!["sessions.list".into()],
                status: PanelTabStatus::Enabled,
                status_detail: None,
            },
            origin: PanelUiCapabilityOrigin::Community,
            plugin_version: "1.2.3".into(),
            asset_integrity: Some(format!(
                "sha256-{}",
                base64::engine::general_purpose::STANDARD.encode(digest)
            )),
            public_base: "./tabs/".into(),
            source_revision: revision,
        }
    }

    #[test]
    fn admitted_declaration_resolves_exact_bytes_by_canonical_identity() {
        let bytes = b"export default {}".to_vec();
        let declaration = declaration(7, &bytes);
        let mut store = PanelAssetPublicationStore::default();
        store.publish(declaration.clone(), bytes.clone()).unwrap();

        let request = PanelAssetRequest::from_declaration(&declaration);
        assert_eq!(store.serve(&request).unwrap(), bytes);
        assert_eq!(
            request.identity,
            PanelAssetIdentity::from_declaration(&declaration)
        );
    }

    #[test]
    fn publication_fails_closed_for_integrity_path_auth_and_missing_records() {
        let bytes = b"export default {}".to_vec();
        let declaration = declaration(7, &bytes);
        let mut store = PanelAssetPublicationStore::default();
        assert!(
            store
                .publish(declaration.clone(), b"wrong bytes".to_vec())
                .is_err()
        );
        store.publish(declaration.clone(), bytes).unwrap();

        let mut traversal = PanelAssetRequest::from_declaration(&declaration);
        traversal.entry = "./tabs/community.example/1.2.3/../outside.js".into();
        assert!(store.serve(&traversal).is_err());

        let mut arbitrary_url = PanelAssetRequest::from_declaration(&declaration);
        arbitrary_url.entry = "https://example.invalid/module.js".into();
        assert!(store.serve(&arbitrary_url).is_err());

        let mut unauthorized = PanelAssetRequest::from_declaration(&declaration);
        unauthorized.authorized = false;
        assert!(store.serve(&unauthorized).is_err());

        let missing = PanelAssetRequest::new(
            PanelAssetIdentity::new(
                "missing.example".into(),
                "1.2.3".into(),
                "overview".into(),
                7,
            ),
            ENTRY.into(),
            true,
        );
        assert!(store.serve(&missing).is_err());
    }

    #[test]
    fn replacement_retracts_stale_revision_and_explicit_generic_fields_do_not_publish() {
        let first_bytes = b"export default 1".to_vec();
        let replacement_bytes = b"export default 2".to_vec();
        let first = declaration(7, &first_bytes);
        let mut replacement = declaration(8, &replacement_bytes);
        replacement.descriptor.label = "Changed label".into();
        replacement.descriptor.icon = "different".into();
        replacement.descriptor.order = 99;
        let mut store = PanelAssetPublicationStore::default();

        store.publish(first.clone(), first_bytes).unwrap();
        store
            .publish(replacement.clone(), replacement_bytes.clone())
            .unwrap();
        assert!(
            store
                .serve(&PanelAssetRequest::from_declaration(&first))
                .is_err()
        );
        assert_eq!(
            store
                .serve(&PanelAssetRequest::from_declaration(&replacement))
                .unwrap(),
            replacement_bytes
        );

        let identity = PanelAssetIdentity::from_declaration(&replacement);
        assert!(store.retract(&identity));
        assert!(
            store
                .serve(&PanelAssetRequest::from_declaration(&replacement))
                .is_err()
        );
    }
}
