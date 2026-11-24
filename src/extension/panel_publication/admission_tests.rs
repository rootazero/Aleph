use std::path::Path;

use aleph_protocol::panel::PANEL_UI_API_MAJOR;
use base64::Engine;
use sha2::{Digest, Sha256};

use super::{PanelAssetIdentity, PanelAssetPublicationStore, PreparedPanelPublication};
use crate::extension::manifest::PanelUiManifest;
use crate::extension::plugin_trust::OwnerTrustPolicy;
use crate::extension::visibility::VisibilityCtx;
use crate::extension::{PluginKind, PluginOrigin, PluginRecord};

fn record(root: &Path, id: &str, bytes: &[u8]) -> PluginRecord {
    let mut record = PluginRecord::new(
        id.into(),
        id.into(),
        PluginKind::Static,
        PluginOrigin::Workspace,
    );
    record.version = Some("1.0.0".into());
    record.root_dir = root.join(id);
    std::fs::create_dir_all(record.root_dir.join("ui")).unwrap();
    std::fs::write(record.root_dir.join("ui/panel.mjs"), bytes).unwrap();
    record.panel = Some(PanelUiManifest {
        tab_id: "overview".into(),
        label: "Overview".into(),
        icon: "layout".into(),
        order: 0,
        entry: "ui/panel.mjs".into(),
        api_major: PANEL_UI_API_MAJOR,
        permissions: vec![],
        asset_integrity: format!(
            "sha256-{}",
            base64::engine::general_purpose::STANDARD.encode(Sha256::digest(bytes))
        ),
    });
    record
}

async fn publish(store: &mut PanelAssetPublicationStore, records: Vec<PluginRecord>) {
    let policy = OwnerTrustPolicy::permissive();
    let prepared = PreparedPanelPublication::prepare(records, &policy).await;
    store.commit(prepared, &policy);
}

#[tokio::test]
async fn admits_explicit_exact_file_and_retains_immutable_bytes() {
    let root = tempfile::tempdir().unwrap();
    let record = record(root.path(), "explicit", b"export default 'panel'");
    std::fs::write(record.root_dir.join("generic.mjs"), b"wrong generic bytes").unwrap();
    let mut store = PanelAssetPublicationStore::default();
    publish(&mut store, vec![record.clone()]).await;
    let snapshot = store.snapshot(&VisibilityCtx::default());
    assert_eq!(snapshot.declarations.len(), 1);
    let declaration = &snapshot.declarations[0];
    let identity = PanelAssetIdentity::from_declaration(declaration);
    std::fs::write(
        record.root_dir.join("ui/panel.mjs"),
        b"changed after admission",
    )
    .unwrap();
    assert_eq!(
        store
            .asset(
                &identity,
                &declaration.descriptor.entry,
                &VisibilityCtx::default()
            )
            .unwrap()
            .bytes
            .as_ref(),
        b"export default 'panel'"
    );
    publish(&mut store, vec![record]).await;
    assert_eq!(store.snapshot(&VisibilityCtx::default()).revision, 2);
    assert!(store
        .snapshot(&VisibilityCtx::default())
        .declarations
        .is_empty());
    assert!(store
        .asset(
            &identity,
            &declaration.descriptor.entry,
            &VisibilityCtx::default()
        )
        .is_err());
}

#[tokio::test]
async fn missing_directory_integrity_and_parent_entries_are_omitted() {
    let root = tempfile::tempdir().unwrap();
    let missing = record(root.path(), "missing", b"module");
    std::fs::remove_file(missing.root_dir.join("ui/panel.mjs")).unwrap();
    let directory = record(root.path(), "directory", b"module");
    std::fs::remove_file(directory.root_dir.join("ui/panel.mjs")).unwrap();
    std::fs::create_dir(directory.root_dir.join("ui/panel.mjs")).unwrap();
    let mismatch = record(root.path(), "mismatch", b"module");
    std::fs::write(mismatch.root_dir.join("ui/panel.mjs"), b"different").unwrap();
    let mut parent = record(root.path(), "parent", b"module");
    parent.panel.as_mut().unwrap().entry = "../outside.mjs".into();
    let mut store = PanelAssetPublicationStore::default();
    publish(&mut store, vec![missing, directory, mismatch, parent]).await;
    assert!(store
        .snapshot(&VisibilityCtx::default())
        .declarations
        .is_empty());
}

#[tokio::test]
async fn entry_limit_and_generation_budget_are_exact_and_deterministic() {
    let root = tempfile::tempdir().unwrap();
    let bytes = vec![b'x'; 4 * 1024 * 1024];
    let records = (0..17)
        .rev()
        .map(|index| record(root.path(), &format!("budget-{index:02}"), &bytes))
        .collect();
    let mut store = PanelAssetPublicationStore::default();
    publish(&mut store, records).await;
    let snapshot = store.snapshot(&VisibilityCtx::default());
    assert_eq!(snapshot.declarations.len(), 16);
    assert_eq!(
        snapshot
            .declarations
            .last()
            .unwrap()
            .descriptor
            .key
            .plugin_id,
        "budget-15"
    );
    let oversized = record(root.path(), "oversized", &vec![b'x'; 4 * 1024 * 1024 + 1]);
    publish(&mut store, vec![oversized]).await;
    assert!(store
        .snapshot(&VisibilityCtx::default())
        .declarations
        .is_empty());
}

#[tokio::test]
async fn duplicate_keys_and_changed_final_policy_cannot_publish() {
    let root = tempfile::tempdir().unwrap();
    let first = record(root.path(), "duplicate", b"module");
    let mut duplicate = first.clone();
    duplicate.version = Some("2.0.0".into());
    let mut store = PanelAssetPublicationStore::default();
    publish(&mut store, vec![first.clone(), duplicate]).await;
    assert!(store
        .snapshot(&VisibilityCtx::default())
        .declarations
        .is_empty());
    let prepared =
        PreparedPanelPublication::prepare(vec![first], &OwnerTrustPolicy::permissive()).await;
    store.commit(
        prepared,
        &OwnerTrustPolicy::restrictive(Vec::<String>::new()),
    );
    assert!(store
        .snapshot(&VisibilityCtx::default())
        .declarations
        .is_empty());
}

#[tokio::test]
async fn exhausted_revision_withdraws_existing_records_without_wrapping() {
    let root = tempfile::tempdir().unwrap();
    let record = record(root.path(), "exhaustion", b"module");
    let mut store = PanelAssetPublicationStore::default();
    publish(&mut store, vec![record.clone()]).await;
    let previous = store
        .snapshot(&VisibilityCtx::default())
        .declarations
        .remove(0);
    store.revision = u64::MAX;
    publish(&mut store, vec![record]).await;
    assert_eq!(store.snapshot(&VisibilityCtx::default()).revision, u64::MAX);
    assert!(store
        .snapshot(&VisibilityCtx::default())
        .declarations
        .is_empty());
    assert!(store
        .asset(
            &PanelAssetIdentity::from_declaration(&previous),
            &previous.descriptor.entry,
            &VisibilityCtx::default()
        )
        .is_err());
}

#[tokio::test]
async fn escaping_symlink_is_not_a_publication_source() {
    let root = tempfile::tempdir().unwrap();
    let record = record(root.path(), "symlink", b"module");
    let outside = root.path().join("outside.mjs");
    std::fs::write(&outside, b"module").unwrap();
    let entry = record.root_dir.join("ui/panel.mjs");
    std::fs::remove_file(&entry).unwrap();
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&outside, &entry);
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&outside, &entry);
    if let Err(error) = linked {
        if error.kind() == std::io::ErrorKind::PermissionDenied
            || error.raw_os_error() == Some(1314)
        {
            eprintln!(
                "SKIP escaping_symlink_is_not_a_publication_source: symlink privilege unavailable"
            );
            return;
        }
        panic!("symlink fixture failed: {error}");
    }
    let mut store = PanelAssetPublicationStore::default();
    publish(&mut store, vec![record]).await;
    assert!(store
        .snapshot(&VisibilityCtx::default())
        .declarations
        .is_empty());
}
