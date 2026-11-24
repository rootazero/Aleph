use super::*;
use base64::Engine;
use sha2::{Digest, Sha256};

use crate::extension::panel_publication::PanelAssetIdentity;
use crate::extension::visibility::VisibilityCtx;

const FIRST: &[u8] = b"export const panelVersion = 1;";
const SECOND: &[u8] = b"export const panelVersion = 2;";

fn write_panel(parent: &Path, id: &str, version: &str, bytes: &[u8]) -> PathBuf {
    let dir = parent.join("plugins").join(id);
    std::fs::create_dir_all(dir.join("ui")).unwrap();
    std::fs::create_dir_all(dir.join("commands")).unwrap();
    std::fs::write(dir.join("ui/panel.mjs"), bytes).unwrap();
    std::fs::write(dir.join("generic.mjs"), b"export const generic = true;").unwrap();
    std::fs::write(
        dir.join("commands/hello.md"),
        "---\ndescription: hello\n---\nHello\n",
    )
    .unwrap();
    std::fs::write(dir.join("aleph.plugin.toml"), format!(
        "[plugin]\nid = \"{id}\"\nname = \"{id}\"\nversion = \"{version}\"\nkind = \"static\"\nentry = \"generic.mjs\"\n\n[panel]\ntab_id = \"overview\"\nlabel = \"Overview\"\nicon = \"layout\"\norder = 10\nentry = \"ui/panel.mjs\"\napi_major = 1\npermissions = []\nasset_integrity = \"sha256-{}\"\n",
        base64::engine::general_purpose::STANDARD.encode(Sha256::digest(bytes))
    )).unwrap();
    dir
}

async fn manager(root: &Path) -> ExtensionManager {
    let mut manager = ExtensionManager::new(ExtensionConfig {
        discovery: DiscoveryConfig {
            working_dir: root.to_path_buf(),
            scan_claude_dirs: false,
            scan_project_dirs: false,
            max_upward_depth: 0,
            claude_home_override: None,
        },
        plugins_config_path: Some(root.join("plugins.toml")),
        extra_plugin_parents: ["project-a", "project-b"]
            .map(|name| {
                let project_root = root.join(name);
                std::fs::create_dir_all(&project_root).unwrap();
                crate::discovery::ProjectPluginParent {
                    dir: project_root.join("plugins"),
                    project_root,
                }
            })
            .to_vec(),
    })
    .await
    .unwrap();
    manager.skill_system = crate::skill::SkillSystem::new();
    manager
}

fn context(root: &Path, project: &str) -> VisibilityCtx {
    VisibilityCtx::from_project_root(Some(root.join(project)))
}

#[tokio::test]
async fn core_panel_lifecycle_scopes_discovery_and_exact_bytes() {
    let root = tempfile::tempdir().unwrap();
    let _home = isolated_home(root.path());
    write_panel(
        &root.path().join("aleph-state"),
        "global-fixture",
        "1.0.0",
        FIRST,
    );
    write_panel(&root.path().join("project-a"), "a-fixture", "1.0.0", FIRST);
    write_panel(&root.path().join("project-b"), "b-fixture", "1.0.0", SECOND);
    let manager = manager(root.path()).await;
    assert_eq!(manager.load_all().await.unwrap().plugins_loaded, 3);
    let global = manager.panel_ui_capability_snapshot();
    assert_eq!(global.declarations.len(), 1);
    assert_eq!(
        global.declarations[0].descriptor.key.plugin_id,
        "global-fixture"
    );
    for (project, own, denied) in [
        ("project-a", "a-fixture", "b-fixture"),
        ("project-b", "b-fixture", "a-fixture"),
    ] {
        let ctx = context(root.path(), project);
        let snapshot = manager.panel_ui_capability_snapshot_for(&ctx);
        assert_eq!(snapshot.revision, manager.panel_ui_capability_revision());
        assert_eq!(snapshot.declarations.len(), 2);
        assert!(snapshot.declarations.iter().any(|declaration| declaration
            .descriptor
            .key
            .plugin_id
            == own));
        let other = manager.panel_ui_capability_snapshot_for(&context(
            root.path(),
            if project == "project-a" {
                "project-b"
            } else {
                "project-a"
            },
        ));
        let denied = other
            .declarations
            .iter()
            .find(|declaration| declaration.descriptor.key.plugin_id == denied)
            .unwrap();
        assert!(manager
            .panel_ui_asset(
                &PanelAssetIdentity::from_declaration(denied),
                &denied.descriptor.entry,
                &ctx
            )
            .is_err());
        for declaration in snapshot.declarations {
            let asset = manager
                .panel_ui_asset(
                    &PanelAssetIdentity::from_declaration(&declaration),
                    &declaration.descriptor.entry,
                    &ctx,
                )
                .unwrap();
            assert_eq!(asset.declaration, declaration);
            assert_eq!(
                asset.bytes.as_ref(),
                if declaration.descriptor.key.plugin_id == "b-fixture" {
                    SECOND
                } else {
                    FIRST
                }
            );
            assert!(!serde_json::to_string(&asset.declaration)
                .unwrap()
                .contains(&root.path().to_string_lossy().to_string()));
        }
    }
    assert_eq!(
        manager.panel_ui_capability_snapshot_for(&VisibilityCtx::default()),
        global
    );
    assert_eq!(
        manager.panel_ui_capability_snapshot_for(&context(root.path(), "unknown-project")),
        global
    );
}

#[tokio::test]
async fn core_panel_lifecycle_replacement_failure_disable_and_unmount_withdraw_bytes() {
    let root = tempfile::tempdir().unwrap();
    let _home = isolated_home(root.path());
    let parent = root.path().join("project-a");
    let dir = write_panel(&parent, "community-fixture", "1.0.0", FIRST);
    let manager = manager(root.path()).await;
    manager.load_all().await.unwrap();
    let ctx = context(root.path(), "project-a");
    let first = manager
        .panel_ui_capability_snapshot_for(&ctx)
        .declarations
        .remove(0);
    let first_id = PanelAssetIdentity::from_declaration(&first);
    assert!(manager
        .panel_ui_asset(&first_id, &first.descriptor.entry, &ctx)
        .is_ok());
    assert!(manager
        .panel_ui_asset(
            &first_id,
            "./tabs/community-fixture/1.0.0/generic.mjs",
            &ctx
        )
        .is_err());
    std::fs::write(dir.join("ui/panel.mjs"), SECOND).unwrap();
    assert_eq!(
        manager
            .panel_ui_asset(&first_id, &first.descriptor.entry, &ctx)
            .unwrap()
            .bytes
            .as_ref(),
        FIRST
    );
    manager.reload_plugin("community-fixture").await.unwrap();
    assert!(manager
        .get_plugin_record("community-fixture")
        .await
        .unwrap()
        .status
        .is_active());
    assert!(manager
        .panel_ui_capability_snapshot_for(&ctx)
        .declarations
        .is_empty());
    assert!(manager
        .panel_ui_asset(&first_id, &first.descriptor.entry, &ctx)
        .is_err());

    write_panel(&parent, "community-fixture", "2.0.0", SECOND);
    manager.reload_plugin("community-fixture").await.unwrap();
    let second = manager
        .panel_ui_capability_snapshot_for(&ctx)
        .declarations
        .remove(0);
    assert_eq!(second.plugin_version, "2.0.0");
    assert!(second.source_revision > first.source_revision);
    assert_eq!(
        manager
            .panel_ui_asset(
                &PanelAssetIdentity::from_declaration(&second),
                &second.descriptor.entry,
                &ctx
            )
            .unwrap()
            .bytes
            .as_ref(),
        SECOND
    );
    assert!(manager
        .panel_ui_asset(&first_id, &first.descriptor.entry, &ctx)
        .is_err());
    assert!(
        manager
            .set_plugin_enabled("community-fixture", false)
            .await
            .unwrap()
            .persisted
    );
    assert!(manager
        .panel_ui_capability_snapshot_for(&ctx)
        .declarations
        .is_empty());
    assert!(manager
        .panel_ui_asset(
            &PanelAssetIdentity::from_declaration(&second),
            &second.descriptor.entry,
            &ctx
        )
        .is_err());
    assert!(
        manager
            .set_plugin_enabled("community-fixture", true)
            .await
            .unwrap()
            .persisted
    );
    assert_eq!(
        manager
            .panel_ui_capability_snapshot_for(&ctx)
            .declarations
            .len(),
        1
    );
    manager.unmount("community-fixture").await.unwrap();
    assert!(manager
        .panel_ui_capability_snapshot_for(&ctx)
        .declarations
        .is_empty());
}

#[tokio::test]
async fn core_panel_lifecycle_failed_reload_never_serves_old_publication() {
    let root = tempfile::tempdir().unwrap();
    let _home = isolated_home(root.path());
    let dir = write_panel(
        &root.path().join("project-a"),
        "community-fixture",
        "1.0.0",
        FIRST,
    );
    let manager = manager(root.path()).await;
    manager.load_all().await.unwrap();
    let ctx = context(root.path(), "project-a");
    let first = manager
        .panel_ui_capability_snapshot_for(&ctx)
        .declarations
        .remove(0);
    std::fs::write(dir.join("aleph.plugin.toml"), "[invalid TOML").unwrap();
    assert!(manager.reload_plugin("community-fixture").await.is_err());
    assert!(manager
        .panel_ui_capability_snapshot_for(&ctx)
        .declarations
        .is_empty());
    assert!(manager
        .panel_ui_asset(
            &PanelAssetIdentity::from_declaration(&first),
            &first.descriptor.entry,
            &ctx
        )
        .is_err());
}

#[tokio::test]
async fn core_panel_lifecycle_root_and_scope_replacement_denies_old_context() {
    let root = tempfile::tempdir().unwrap();
    let _home = isolated_home(root.path());
    let old = write_panel(
        &root.path().join("project-a"),
        "community-fixture",
        "1.0.0",
        FIRST,
    );
    let manager = manager(root.path()).await;
    manager.load_all().await.unwrap();
    let a = context(root.path(), "project-a");
    let first = manager
        .panel_ui_capability_snapshot_for(&a)
        .declarations
        .remove(0);
    std::fs::remove_dir_all(old).unwrap();
    write_panel(
        &root.path().join("project-b"),
        "community-fixture",
        "2.0.0",
        SECOND,
    );
    manager.reload().await.unwrap();
    let b = context(root.path(), "project-b");
    let second = manager
        .panel_ui_capability_snapshot_for(&b)
        .declarations
        .remove(0);
    assert_eq!(second.plugin_version, "2.0.0");
    assert!(manager
        .panel_ui_capability_snapshot_for(&a)
        .declarations
        .is_empty());
    assert!(manager
        .panel_ui_asset(
            &PanelAssetIdentity::from_declaration(&first),
            &first.descriptor.entry,
            &a
        )
        .is_err());
    assert!(manager
        .panel_ui_asset(
            &PanelAssetIdentity::from_declaration(&second),
            &second.descriptor.entry,
            &a
        )
        .is_err());
    assert_eq!(
        manager
            .panel_ui_asset(
                &PanelAssetIdentity::from_declaration(&second),
                &second.descriptor.entry,
                &b
            )
            .unwrap()
            .bytes
            .as_ref(),
        SECOND
    );
}

#[tokio::test]
async fn core_panel_lifecycle_keeps_trust_policy_as_next_admission_gate() {
    let root = tempfile::tempdir().unwrap();
    let _home = isolated_home(root.path());
    write_panel(
        &root.path().join("project-a"),
        "community-fixture",
        "1.0.0",
        FIRST,
    );
    let manager = manager(root.path()).await;
    manager.load_all().await.unwrap();
    let ctx = context(root.path(), "project-a");
    let first = manager
        .panel_ui_capability_snapshot_for(&ctx)
        .declarations
        .remove(0);
    manager.set_owner_trust_policy(
        crate::extension::plugin_trust::OwnerTrustPolicy::restrictive(Vec::<String>::new()),
    );
    assert!(manager
        .panel_ui_asset(
            &PanelAssetIdentity::from_declaration(&first),
            &first.descriptor.entry,
            &ctx
        )
        .is_ok());
    let _ = manager.reload_plugin("community-fixture").await;
    assert!(manager
        .panel_ui_capability_snapshot_for(&ctx)
        .declarations
        .is_empty());
    assert!(manager
        .panel_ui_asset(
            &PanelAssetIdentity::from_declaration(&first),
            &first.descriptor.entry,
            &ctx
        )
        .is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn core_panel_lifecycle_concurrent_reads_keep_generation_bytes_aligned() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let root = tempfile::tempdir().unwrap();
    let _home = isolated_home(root.path());
    let parent = root.path().join("project-a");
    write_panel(&parent, "community-fixture", "1.0.0", FIRST);
    let manager = Arc::new(manager(root.path()).await);
    manager.load_all().await.unwrap();
    let ctx = context(root.path(), "project-a");
    let done = Arc::new(AtomicBool::new(false));
    let (seen, mut versions) = tokio::sync::watch::channel(String::new());
    let reader = tokio::spawn({
        let manager = manager.clone();
        let done = done.clone();
        let ctx = ctx.clone();
        async move {
            let mut observed = 0;
            while !done.load(Ordering::Acquire) {
                for declaration in manager.panel_ui_capability_snapshot_for(&ctx).declarations {
                    if let Ok(asset) = manager.panel_ui_asset(
                        &PanelAssetIdentity::from_declaration(&declaration),
                        &declaration.descriptor.entry,
                        &ctx,
                    ) {
                        assert_eq!(asset.declaration, declaration);
                        assert_eq!(
                            asset.bytes.as_ref(),
                            if declaration.plugin_version == "1.0.0" {
                                FIRST
                            } else {
                                SECOND
                            }
                        );
                        observed += 1;
                        seen.send_replace(declaration.plugin_version);
                    }
                }
                tokio::task::yield_now().await;
            }
            observed
        }
    });
    for version in ["1.0.0", "2.0.0", "3.0.0"] {
        if version != "1.0.0" {
            write_panel(&parent, "community-fixture", version, SECOND);
            manager.reload_plugin("community-fixture").await.unwrap();
        }
        assert_eq!(
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                versions.wait_for(|observed| observed == version),
            )
            .await
            .expect("reader must observe each committed version")
            .expect("reader must remain alive")
            .as_str(),
            version
        );
    }
    done.store(true, Ordering::Release);
    assert!(reader.await.unwrap() >= 3);
    let final_snapshot = manager.panel_ui_capability_snapshot_for(&ctx);
    assert_eq!(final_snapshot.declarations[0].plugin_version, "3.0.0");
    assert_eq!(
        final_snapshot.revision,
        manager.panel_ui_capability_revision()
    );
}
