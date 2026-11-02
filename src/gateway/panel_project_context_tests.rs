use super::*;
use crate::extension::visibility::{visible_to, ScopeKey};
use crate::gateway::security::store::{DeviceUpsertData, UserRole, UserStatus};

fn device(store: &SecurityStore, id: &str, user: Option<&str>) {
    store
        .upsert_device(&DeviceUpsertData {
            device_id: id,
            device_name: "Panel",
            device_type: Some("panel"),
            public_key: &[1; 32],
            fingerprint: id,
            role: "member",
            scopes: &[],
            user_id: user,
        })
        .unwrap();
}

static TEST_GUARD: Mutex<()> = Mutex::new(());

fn fixture() -> (
    std::sync::MutexGuard<'static, ()>,
    PanelProjectSelector,
    PanelCallerContext,
    tempfile::TempDir,
) {
    let guard = TEST_GUARD.lock().unwrap_or_else(|error| error.into_inner());
    let root = tempfile::tempdir().unwrap();
    let projects = Arc::new(ProjectStore::new(
        rusqlite::Connection::open_in_memory().unwrap(),
    ));
    projects.create_schema().unwrap();
    let security = Arc::new(SecurityStore::in_memory().unwrap());
    security
        .create_user("panel-alice", "Alice", UserRole::Member)
        .unwrap();
    security
        .create_user("panel-bob", "Bob", UserRole::Member)
        .unwrap();
    device(&security, "panel-device", Some("panel-alice"));
    (
        guard,
        PanelProjectSelector::new(projects, security),
        PanelCallerContext::authenticated(Some("panel-alice".into()), Some("panel-device".into())),
        root,
    )
}

#[test]
fn absent_selection_is_global_without_cwd() {
    let (_guard, selector, caller, _) = fixture();
    assert!(selector
        .resolve(&caller)
        .unwrap()
        .visibility
        .project_root
        .is_none());
    assert!(selector
        .resolve(&PanelCallerContext::default())
        .unwrap()
        .visibility
        .project_root
        .is_none());
}

#[test]
fn selection_exposes_global_and_only_authorized_project() {
    let (_guard, selector, caller, root) = fixture();
    let a = selector
        .projects
        .create("A", Some("panel-alice"), Some(root.path()))
        .unwrap();
    selector.select(&caller, Some(a.id.clone())).unwrap();
    let resolved = selector.resolve(&caller).unwrap();
    assert_eq!(
        resolved.selection.project_id.as_deref(),
        Some(a.id.as_str())
    );
    assert!(visible_to(&ScopeKey::Global, &resolved.visibility));
    assert!(visible_to(
        &ScopeKey::project(root.path()),
        &resolved.visibility
    ));
    assert!(!visible_to(
        &ScopeKey::project(&root.path().join("B")),
        &resolved.visibility
    ));
}

#[test]
fn refused_claim_preserves_selection_and_clear_is_explicit() {
    let (_guard, selector, caller, root) = fixture();
    let a = selector
        .projects
        .create("A", Some("panel-alice"), Some(root.path()))
        .unwrap();
    let b = selector
        .projects
        .create("B", Some("panel-bob"), Some(root.path()))
        .unwrap();
    let blank = selector
        .projects
        .create("Blank", Some("panel-alice"), None)
        .unwrap();
    let archived = selector
        .projects
        .create(
            "Archived",
            Some("panel-alice"),
            Some(&root.path().join("archived")),
        )
        .unwrap();
    selector.projects.archive(&archived.id).unwrap();
    let selected = selector.select(&caller, Some(a.id.clone())).unwrap();
    for claim in [
        b.id,
        blank.id,
        archived.id,
        "missing".into(),
        "".into(),
        " bad ".into(),
    ] {
        assert_eq!(
            selector.select(&caller, Some(claim)),
            Err(PanelProjectContextError)
        );
        assert_eq!(selector.resolve(&caller).unwrap().selection, selected);
    }
    let cleared = selector.select(&caller, None).unwrap();
    assert!(cleared.project_id.is_none());
    assert!(cleared.generation > selected.generation);
    assert!(selector
        .resolve(&caller)
        .unwrap()
        .visibility
        .project_root
        .is_none());
}

#[test]
fn compatibility_and_unbound_devices_cannot_select() {
    let (_guard, selector, _caller, root) = fixture();
    let a = selector
        .projects
        .create("A", Some("panel-alice"), Some(root.path()))
        .unwrap();
    let compatibility = PanelCallerContext::authenticated(Some("owner".into()), None);
    assert_eq!(
        selector.select(&compatibility, Some(a.id.clone())),
        Err(PanelProjectContextError)
    );
    device(&selector.security, "unbound", None);
    let unbound =
        PanelCallerContext::authenticated(Some("panel-alice".into()), Some("unbound".into()));
    assert_eq!(
        selector.select(&unbound, Some(a.id)),
        Err(PanelProjectContextError)
    );
    assert_eq!(
        selector.select(&compatibility, None),
        Err(PanelProjectContextError)
    );
}

#[test]
fn current_membership_and_workspace_are_rechecked_for_every_use() {
    let (_guard, selector, caller, root) = fixture();
    let a = selector
        .projects
        .create("A", Some("panel-bob"), Some(root.path()))
        .unwrap();
    selector.projects.add_member(&a.id, "panel-alice").unwrap();
    selector.select(&caller, Some(a.id.clone())).unwrap();
    selector
        .projects
        .remove_member(&a.id, "panel-alice")
        .unwrap();
    assert!(selector.resolve(&caller).is_err());
    selector.projects.add_member(&a.id, "panel-alice").unwrap();
    selector.projects.bind_workspace(&a.id, None).unwrap();
    assert!(selector.resolve(&caller).is_err());
    selector
        .projects
        .bind_workspace(&a.id, Some(root.path()))
        .unwrap();
    assert!(selector.resolve(&caller).is_ok());
    selector.projects.archive(&a.id).unwrap();
    assert!(selector.resolve(&caller).is_err());
}

#[test]
fn device_user_rebinding_or_revocation_never_inherits_selection() {
    let (_guard, selector, caller, root) = fixture();
    let a = selector
        .projects
        .create("A", Some("panel-alice"), Some(root.path()))
        .unwrap();
    selector.projects.add_member(&a.id, "panel-bob").unwrap();
    selector.select(&caller, Some(a.id)).unwrap();
    device(&selector.security, "panel-device", Some("panel-bob"));
    assert!(selector.resolve(&caller).is_err());
    let bob =
        PanelCallerContext::authenticated(Some("panel-bob".into()), Some("panel-device".into()));
    assert!(selector.resolve(&bob).is_err());
    device(&selector.security, "panel-device", Some("panel-alice"));
    selector.security.revoke_device("panel-device").unwrap();
    assert!(selector.resolve(&caller).is_err());
}

#[test]
fn deactivated_user_or_missing_verified_identity_refuses_selected_context() {
    let (_guard, selector, caller, root) = fixture();
    let a = selector
        .projects
        .create("A", Some("panel-alice"), Some(root.path()))
        .unwrap();
    selector.select(&caller, Some(a.id)).unwrap();
    let unverified = PanelCallerContext::authenticated(None, Some("panel-device".into()));
    assert!(selector.resolve(&unverified).is_err());
    selector
        .security
        .update_user("panel-alice", None, None, Some(UserStatus::Deactivated))
        .unwrap();
    assert!(selector.resolve(&caller).is_err());
}
