use std::sync::Arc;

use aleph_protocol::panel::{
    PanelTabDescriptor, PanelTabKey, PanelTabStatus, PanelUiCapabilityDeclaration,
    PanelUiCapabilityOrigin, PanelUiTrust, PANEL_UI_API_MAJOR,
};
use base64::Engine;
use sha2::{Digest, Sha256};

use crate::extension::panel_publication::{
    PanelAssetIdentity, PanelAssetPublicationError, PanelAssetPublicationStore,
};
use crate::extension::visibility::{ScopeKey, VisibilityCtx};

const ENTRY: &str = "./tabs/community.example/1.2.3/overview.js";
const BYTES: &[u8] = b"export default 1";

fn declaration(revision: u64, bytes: &[u8]) -> PanelUiCapabilityDeclaration {
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
            base64::engine::general_purpose::STANDARD.encode(Sha256::digest(bytes))
        )),
        public_base: "./tabs/".into(),
        source_revision: revision,
    }
}

#[test]
fn scoped_snapshot_and_bytes_agree() {
    let root = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let context = VisibilityCtx::from_project_root(Some(root.path().to_path_buf()));
    let other_context = VisibilityCtx::from_project_root(Some(other.path().to_path_buf()));
    let mut store = PanelAssetPublicationStore::default();
    let declaration = declaration(7, BYTES);
    let identity = PanelAssetIdentity::from_declaration(&declaration);
    store
        .replace(
            7,
            vec![(
                declaration.clone(),
                Arc::from(BYTES),
                ScopeKey::project(root.path()),
            )],
        )
        .unwrap();
    for denied in [VisibilityCtx::default(), other_context] {
        assert!(store.snapshot(&denied).declarations.is_empty());
        assert!(store.asset(&identity, ENTRY, &denied).is_err());
    }
    assert_eq!(store.snapshot(&context).declarations, vec![declaration]);
    let asset = store.asset(&identity, ENTRY, &context).unwrap();
    assert_eq!(asset.bytes.as_ref(), b"export default 1");
    assert_eq!(asset.declaration.source_revision, 7);
}

#[test]
fn replacement_withdraws_prior_generation() {
    let mut store = PanelAssetPublicationStore::default();
    let first = declaration(7, BYTES);
    store
        .replace(7, vec![(first.clone(), Arc::from(BYTES), ScopeKey::Global)])
        .unwrap();
    let mut next = declaration(8, b"export default 2");
    next.plugin_version = "2.0.0".into();
    next.descriptor.entry = "./tabs/community.example/2.0.0/main.js".into();
    store
        .replace(
            8,
            vec![(
                next.clone(),
                Arc::from(&b"export default 2"[..]),
                ScopeKey::Global,
            )],
        )
        .unwrap();
    assert!(store
        .asset(
            &PanelAssetIdentity::from_declaration(&first),
            ENTRY,
            &VisibilityCtx::default()
        )
        .is_err());
    assert_eq!(
        store.snapshot(&VisibilityCtx::default()).declarations,
        vec![next]
    );
    store.replace(9, vec![]).unwrap();
    assert!(store
        .snapshot(&VisibilityCtx::default())
        .declarations
        .is_empty());
    assert!(matches!(
        store.replace(8, vec![(first, Arc::from(BYTES), ScopeKey::Global)]),
        Err(PanelAssetPublicationError::Stale)
    ));
}

#[test]
fn stale_attempt_with_valid_integrity_is_refused() {
    let mut store = PanelAssetPublicationStore::default();
    store
        .replace(
            7,
            vec![(declaration(7, BYTES), Arc::from(BYTES), ScopeKey::Global)],
        )
        .unwrap();
    for revision in [6, 7] {
        assert!(matches!(
            store.replace(
                revision,
                vec![(
                    declaration(revision, BYTES),
                    Arc::from(BYTES),
                    ScopeKey::Global
                )]
            ),
            Err(PanelAssetPublicationError::Stale)
        ));
    }
    assert_eq!(
        store
            .asset(
                &PanelAssetIdentity::from_declaration(&declaration(7, BYTES)),
                ENTRY,
                &VisibilityCtx::default()
            )
            .unwrap()
            .bytes
            .as_ref(),
        BYTES
    );
}

#[test]
fn integrity_entry_and_identity_cannot_select_other_bytes() {
    let mut store = PanelAssetPublicationStore::default();
    assert!(matches!(
        store.replace(
            7,
            vec![(
                declaration(7, BYTES),
                Arc::from(&b"wrong"[..]),
                ScopeKey::Global
            )]
        ),
        Err(PanelAssetPublicationError::IntegrityMismatch)
    ));
    let mut invalid = declaration(7, BYTES);
    invalid.descriptor.entry = "../outside.js".into();
    assert!(matches!(
        store.replace(7, vec![(invalid, Arc::from(BYTES), ScopeKey::Global)]),
        Err(PanelAssetPublicationError::InvalidDeclaration)
    ));
    let canonical = declaration(7, BYTES);
    store
        .replace(
            7,
            vec![(canonical.clone(), Arc::from(BYTES), ScopeKey::Global)],
        )
        .unwrap();
    let identity = PanelAssetIdentity::from_declaration(&canonical);
    for entry in [
        "https://example.invalid/module.js",
        "./tabs/community.example/1.2.3/../outside.js",
        "C:/private/overview.js",
        "generic.js",
    ] {
        assert!(store
            .asset(&identity, entry, &VisibilityCtx::default())
            .is_err());
    }
    let mut missing = identity;
    missing.plugin_id = "missing.example".into();
    assert!(store
        .asset(&missing, ENTRY, &VisibilityCtx::default())
        .is_err());
}
