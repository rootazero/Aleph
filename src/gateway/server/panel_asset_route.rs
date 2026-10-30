use super::*;
use aleph_protocol::panel::{
    PANEL_UI_API_MAJOR, PanelTabDescriptor, PanelTabKey, PanelTabStatus,
    PanelUiCapabilityDeclaration, PanelUiCapabilityOrigin, PanelUiTrust,
};
use base64::Engine;
use sha2::{Digest, Sha256};

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
