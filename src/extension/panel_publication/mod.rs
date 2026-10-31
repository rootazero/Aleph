//! Core-owned, immutable Panel generations. HTTP authentication belongs to the route.

use std::collections::BTreeMap;
use std::sync::Arc;

use aleph_protocol::panel::{
    valid_asset_integrity, valid_identity_segment, valid_relative_entry, valid_version_segment,
    PanelTabStatus, PanelUiCapabilityDeclaration, PanelUiCapabilityOrigin,
    PanelUiCapabilitySnapshot, PanelUiTrust,
};
use base64::Engine;
use sha2::{Digest, Sha256};

use super::visibility::{visible_to, ScopeKey, VisibilityCtx};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct PanelAssetIdentity {
    pub(crate) plugin_id: String,
    pub(crate) plugin_version: String,
    pub(crate) tab_id: String,
    pub(crate) source_revision: u64,
}

impl PanelAssetIdentity {
    pub(crate) fn from_declaration(declaration: &PanelUiCapabilityDeclaration) -> Self {
        Self {
            plugin_id: declaration.descriptor.key.plugin_id.clone(),
            plugin_version: declaration.plugin_version.clone(),
            tab_id: declaration.descriptor.key.tab_id.clone(),
            source_revision: declaration.source_revision,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PanelAssetPublicationError {
    InvalidDeclaration,
    IntegrityMismatch,
    Missing,
    EntryMismatch,
    Stale,
}

#[derive(Clone)]
pub(crate) struct PublishedPanelAsset {
    pub(crate) declaration: PanelUiCapabilityDeclaration,
    pub(crate) bytes: Arc<[u8]>,
}

struct PanelAssetPublication {
    asset: PublishedPanelAsset,
    scope: ScopeKey,
}

#[derive(Default)]
pub(crate) struct PanelAssetPublicationStore {
    revision: u64,
    records: BTreeMap<PanelAssetIdentity, PanelAssetPublication>,
}

impl PanelAssetPublicationStore {
    // Only the Core publication builder may replace a generation. Consumers are read-only.
    fn replace(
        &mut self,
        revision: u64,
        publications: Vec<(PanelUiCapabilityDeclaration, Arc<[u8]>, ScopeKey)>,
    ) -> Result<(), PanelAssetPublicationError> {
        if revision <= self.revision {
            return Err(PanelAssetPublicationError::Stale);
        }
        let mut records = BTreeMap::new();
        for (declaration, bytes, scope) in publications {
            if declaration.source_revision != revision || !valid_declaration(&declaration) {
                return Err(PanelAssetPublicationError::InvalidDeclaration);
            }
            if !matches_integrity(&bytes, declaration.asset_integrity.as_deref().unwrap()) {
                return Err(PanelAssetPublicationError::IntegrityMismatch);
            }
            if records
                .insert(
                    PanelAssetIdentity::from_declaration(&declaration),
                    PanelAssetPublication {
                        asset: PublishedPanelAsset { declaration, bytes },
                        scope,
                    },
                )
                .is_some()
            {
                return Err(PanelAssetPublicationError::InvalidDeclaration);
            }
        }
        self.records = records;
        self.revision = revision;
        Ok(())
    }

    pub(crate) fn snapshot(&self, context: &VisibilityCtx) -> PanelUiCapabilitySnapshot {
        PanelUiCapabilitySnapshot {
            revision: self.revision,
            declarations: self
                .records
                .values()
                .filter(|record| visible_to(&record.scope, context))
                .map(|record| record.asset.declaration.clone())
                .collect(),
        }
    }

    pub(crate) fn asset(
        &self,
        identity: &PanelAssetIdentity,
        entry: &str,
        context: &VisibilityCtx,
    ) -> Result<PublishedPanelAsset, PanelAssetPublicationError> {
        let publication = self
            .records
            .get(identity)
            .filter(|record| visible_to(&record.scope, context))
            .ok_or(PanelAssetPublicationError::Missing)?;
        if publication.asset.declaration.descriptor.entry != entry {
            return Err(PanelAssetPublicationError::EntryMismatch);
        }
        Ok(publication.asset.clone())
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
        .is_some_and(valid_relative_entry)
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
mod tests;
