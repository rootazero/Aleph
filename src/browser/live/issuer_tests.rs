use super::*;
use crate::browser::live::resource::{ResourceDraft, ResourceError};
use aleph_protocol::browser_live::{
    BrowserTargetBinding, ClosedReason, ConversationBinding, DisplayPolicy, FragmentNonce,
    PreviewKind, PreviewProvenance, PreviewSource, ViewerBinding,
};

pub(crate) const USER: &str = "issuer-user";
pub(crate) const ORIGIN: &str = "https://viewer.example.test";

pub(crate) struct Fixture {
    pub state: Arc<BrowserViewerIssuerState>,
    pub resources: ResourceRegistry,
    pub grants: ViewerGrantStore,
    pub draft: ResourceDraft,
    pub resource: PreviewResource,
}

pub(crate) fn fixture(ttl: Duration) -> Fixture {
    let grants = ViewerGrantStore::default();
    let resources = ResourceRegistry::default();
    let state = BrowserViewerIssuerState::new(
        grants.clone(),
        resources.clone(),
        ORIGIN,
        Duration::from_secs(300),
    )
    .unwrap();
    let draft = ResourceDraft {
        binding: ViewerBinding {
            principal: IssuerIdentity::User {
                user_id: USER.into(),
            },
            task: ConversationBinding {
                session_key: "session-one".into(),
                run_id: "run-one".into(),
            },
            target: BrowserTargetBinding {
                profile: "default".into(),
                browser_session_id: "browser-one".into(),
                target_id: "target-one".into(),
            },
        },
        kind: PreviewKind::Page,
        display: DisplayPolicy::AutoRevealCurrentRun,
        source: PreviewSource::BrowserTool,
        provenance: PreviewProvenance::NotManaged,
        redaction: RedactionStatus::Safe,
        ttl,
        companion_candidates: Vec::new(),
    };
    let resource = resources.publish(draft.clone()).unwrap();
    Fixture {
        state,
        resources,
        grants,
        draft,
        resource,
    }
}

pub(crate) fn request(
    resource: &PreviewResource,
    permissions: Vec<ViewerPermission>,
) -> IssueViewerGrantRequest {
    IssueViewerGrantRequest {
        resource_id: resource.resource_id.clone(),
        permissions,
    }
}

pub(crate) fn nonce(response: &IssueViewerGrantResponse) -> FragmentNonce {
    let url = url::Url::parse(&response.launch_url).unwrap();
    let fields: std::collections::HashMap<_, _> =
        url::form_urlencoded::parse(url.fragment().unwrap().as_bytes())
            .into_owned()
            .collect();
    assert_eq!(fields.get("grant_id"), Some(&response.grant_id));
    FragmentNonce(fields.get("nonce").unwrap().clone())
}

#[test]
fn owned_issue_uses_exact_registry_binding_and_fragment_only() {
    let f = fixture(Duration::from_secs(600));
    let response = f
        .state
        .issue(USER, request(&f.resource, vec![ViewerPermission::Frames]))
        .unwrap();
    let url = url::Url::parse(&response.launch_url).unwrap();
    assert_eq!(url.origin().ascii_serialization(), ORIGIN);
    assert_eq!(url.path(), "/viewer/browser");
    assert!(url.query().is_none());
    assert!(url.username().is_empty());
    assert!(url.password().is_none());
    let secret = nonce(&response);
    assert!(!response
        .launch_url
        .split('#')
        .next()
        .unwrap()
        .contains(&secret.0));
    let info = f.grants.grant_info(&response.grant_id).unwrap();
    assert_eq!(info.issuer, f.draft.binding.principal);
    assert_eq!(info.binding, f.draft.binding);
    assert_eq!(info.resource_id, f.resource.resource_id);
    assert_eq!(info.permissions, vec![ViewerPermission::Frames]);
    assert_eq!(info.origin, f.state.origin);
    f.grants
        .consume_nonce(&response.grant_id, &secret, Some(&f.state.origin))
        .unwrap();
    assert!(f
        .grants
        .consume_nonce(&response.grant_id, &secret, Some(&f.state.origin))
        .is_err());
}

#[test]
fn foreign_unknown_closed_expired_connection_artifact_and_refused_deny() {
    let f = fixture(Duration::from_secs(600));
    let req = || request(&f.resource, vec![ViewerPermission::Frames]);
    assert!(f.state.issue("foreign", req()).is_err());
    assert!(f.state.issue(" ", req()).is_err());
    let mut unknown = req();
    unknown.resource_id = "res-unknown".into();
    assert!(f.state.issue(USER, unknown).is_err());
    f.resources
        .close(&f.resource.resource_id, ClosedReason::TargetClosed)
        .unwrap();
    assert!(f.state.issue(USER, req()).is_err());

    let expired = fixture(Duration::from_millis(1));
    std::thread::sleep(Duration::from_millis(10));
    assert!(expired
        .state
        .issue(
            USER,
            request(&expired.resource, vec![ViewerPermission::Frames])
        )
        .is_err());

    let f = fixture(Duration::from_secs(600));
    let mut connection = f.draft.clone();
    connection.binding.principal = IssuerIdentity::Connection {
        connection_id: USER.into(),
    };
    let resource = f.resources.publish(connection).unwrap();
    // Same opaque string never collapses User and Connection identities.
    assert!(f
        .state
        .issue(USER, request(&resource, vec![ViewerPermission::Frames]))
        .is_err());

    let mut artifact = f.draft.clone();
    artifact.kind = PreviewKind::Artifact;
    artifact.source = PreviewSource::ArtifactAdapter;
    assert_eq!(
        f.resources.publish(artifact).unwrap_err(),
        ResourceError::ArtifactNotPublishable
    );
    assert!(f
        .state
        .issue(
            USER,
            IssueViewerGrantRequest {
                resource_id: "artifact-one".into(),
                permissions: vec![ViewerPermission::Frames]
            }
        )
        .is_err());

    let mut refused = f.draft.clone();
    refused.redaction = RedactionStatus::Refused;
    assert!(matches!(
        f.resources.publish(refused),
        Err(ResourceError::Refused { closed: Some(_) })
    ));
    assert!(f
        .state
        .issue(USER, request(&f.resource, vec![ViewerPermission::Frames]))
        .is_err());
}

#[test]
fn pending_redaction_resource_cannot_be_issued() {
    let f = fixture(Duration::from_secs(600));
    let mut pending = f.draft.clone();
    pending.redaction = RedactionStatus::Pending;
    // Same binding: revises the live Safe resource down to Pending.
    let resource = f.resources.publish(pending).unwrap();
    assert_eq!(resource.redaction, RedactionStatus::Pending);
    assert!(f
        .state
        .issue(USER, request(&resource, vec![ViewerPermission::Frames]))
        .is_err());
}

#[test]
fn permissions_narrow_without_implicitly_adding_frames_and_ttl_clamps() {
    let f = fixture(Duration::from_secs(600));
    for permissions in [
        vec![],
        vec![ViewerPermission::HumanInput, ViewerPermission::Navigate],
    ] {
        assert!(f
            .state
            .issue(USER, request(&f.resource, permissions))
            .is_err());
    }
    let before = chrono::Utc::now().timestamp_millis();
    let response = f
        .state
        .issue(
            USER,
            request(
                &f.resource,
                vec![
                    ViewerPermission::Frames,
                    ViewerPermission::HumanInput,
                    ViewerPermission::Navigate,
                ],
            ),
        )
        .unwrap();
    let after = chrono::Utc::now().timestamp_millis();
    assert!(response.expires_at_ms >= before + 300_000);
    assert!(response.expires_at_ms <= after + 300_000);
    assert!(response.expires_at_ms <= f.resource.expires_at_ms);
    assert_eq!(
        f.grants.grant_info(&response.grant_id).unwrap().permissions,
        vec![ViewerPermission::Frames]
    );
    let short = fixture(Duration::from_secs(60));
    let response = short
        .state
        .issue(
            USER,
            request(&short.resource, vec![ViewerPermission::Frames]),
        )
        .unwrap();
    assert_eq!(response.expires_at_ms, short.resource.expires_at_ms);
}

#[test]
fn issue_recheck_revokes_minted_grant_on_close_or_revision_change() {
    for revise in [false, true] {
        let f = fixture(Duration::from_secs(600));
        let (resource, grant) = f
            .state
            .mint(USER, request(&f.resource, vec![ViewerPermission::Frames]))
            .unwrap();
        let id = grant.grant_id.clone();
        let nonce = grant.nonce.clone();
        if revise {
            f.resources.publish(f.draft.clone()).unwrap();
        } else {
            f.resources
                .close(&resource.resource_id, ClosedReason::TargetClosed)
                .unwrap();
        }
        assert!(f.state.finish_issue(USER, &resource, grant).is_err());
        assert!(f.grants.grant_info(&id).is_none());
        assert!(f
            .grants
            .consume_nonce(&id, &nonce, Some(&f.state.origin))
            .is_err());
    }
}

#[test]
fn revoke_only_original_issuer_and_never_reports_unknown_or_expired() {
    let f = fixture(Duration::from_secs(600));
    let issue = || {
        f.state
            .issue(USER, request(&f.resource, vec![ViewerPermission::Frames]))
            .unwrap()
    };
    let response = issue();
    assert!(f.state.revoke("foreign", &response.grant_id).is_err());
    assert!(f.grants.grant_info(&response.grant_id).is_some());
    assert!(f.state.revoke(USER, "unknown").is_err());
    assert!(f.state.revoke(USER, &response.grant_id).unwrap().revoked);
    assert!(f.state.revoke(USER, &response.grant_id).is_err());
    assert!(f
        .grants
        .consume_nonce(&response.grant_id, &nonce(&response), Some(&f.state.origin))
        .is_err());
    let response = issue();
    f.grants.expire_grant_for_test(&response.grant_id);
    assert!(f.state.revoke(USER, &response.grant_id).is_err());
    assert!(!f
        .grants
        .revoke_for_issuer(&response.grant_id, &f.draft.binding.principal));
}

#[test]
fn trusted_origin_is_explicit_and_rejects_authority_or_url_extras() {
    for origin in [
        "",
        "null",
        "ftp://example.test",
        "https://u:p@example.test",
        "https://example.test/path",
        "https://example.test/../",
        "https://example.test?nonce=secret",
        "https://example.test#secret",
        "https://example.test:99999",
    ] {
        assert!(
            BrowserViewerIssuerState::new(
                ViewerGrantStore::default(),
                ResourceRegistry::default(),
                origin,
                Duration::from_secs(60)
            )
            .is_err(),
            "{origin}"
        );
    }
    assert!(BrowserViewerIssuerState::new(
        ViewerGrantStore::default(),
        ResourceRegistry::default(),
        ORIGIN,
        Duration::ZERO
    )
    .is_err());
    let state = BrowserViewerIssuerState::new(
        ViewerGrantStore::default(),
        ResourceRegistry::default(),
        "https://[::1]:443",
        Duration::from_secs(60),
    )
    .unwrap();
    assert_eq!(state.origin.host, "[::1]");
    assert_eq!(state.origin.port, 443);
    assert_eq!(state.launch_base, "https://[::1]/viewer/browser");
}

#[test]
fn absolute_store_expiry_never_extends_and_invalid_ttls_deny() {
    let f = fixture(Duration::from_secs(600));
    let expiry = chrono::Utc::now().timestamp_millis() + 1000;
    let grant = f
        .grants
        .issue_until(
            f.draft.binding.principal.clone(),
            f.draft.binding.clone(),
            &f.resource.resource_id,
            &[ViewerPermission::Frames],
            f.state.origin.clone(),
            expiry,
        )
        .unwrap();
    assert_eq!(grant.expires_at_ms, expiry);
    assert_eq!(
        f.grants.grant_info(&grant.grant_id).unwrap().expires_at_ms,
        expiry
    );
    assert!(f
        .grants
        .issue_until(
            f.draft.binding.principal.clone(),
            f.draft.binding.clone(),
            &f.resource.resource_id,
            &[ViewerPermission::Frames],
            f.state.origin.clone(),
            1
        )
        .is_err());
    assert!(f
        .grants
        .issue(
            f.draft.binding.principal.clone(),
            f.draft.binding.clone(),
            &f.resource.resource_id,
            &[ViewerPermission::Frames],
            f.state.origin.clone(),
            Duration::from_secs(u64::MAX)
        )
        .is_err());
}

/// `browser.viewer.resource.list` returns exactly the caller's live, unexpired
/// resources as the registry holds them (Safe and Pending alike), ordered by
/// id; closed, expired, foreign and connection-principal resources are absent,
/// and a blank caller is denied rather than listed as nobody.
#[test]
fn list_returns_only_the_callers_live_resources_exactly() {
    let f = fixture(Duration::from_secs(600));
    let mut pending = f.draft.clone();
    pending.binding.target.target_id = "target-two".into();
    pending.redaction = RedactionStatus::Pending;
    let pending = f.resources.publish(pending).unwrap();
    let mut foreign = f.draft.clone();
    foreign.binding.target.target_id = "target-three".into();
    foreign.binding.principal = IssuerIdentity::User {
        user_id: "foreign".into(),
    };
    f.resources.publish(foreign).unwrap();
    let mut connection = f.draft.clone();
    connection.binding.target.target_id = "target-four".into();
    connection.binding.principal = IssuerIdentity::Connection {
        connection_id: USER.into(),
    };
    f.resources.publish(connection).unwrap();

    let mut expected = vec![f.resource.clone(), pending.clone()];
    expected.sort_by(|a, b| a.resource_id.cmp(&b.resource_id));
    assert_eq!(f.state.list(USER).unwrap().resources, expected);
    assert_eq!(f.state.list("foreign").unwrap().resources.len(), 1);
    assert!(f.state.list("nobody").unwrap().resources.is_empty());
    assert!(f.state.list(" ").is_err());

    // A revision replaces the listed entry in place (same id, newer revision).
    let mut revised = f.draft.clone();
    revised.redaction = RedactionStatus::Pending;
    let revised = f.resources.publish(revised).unwrap();
    assert_eq!(revised.resource_id, f.resource.resource_id);
    assert!(f.state.list(USER).unwrap().resources.contains(&revised));

    f.resources
        .close(&f.resource.resource_id, ClosedReason::TargetClosed)
        .unwrap();
    assert_eq!(f.state.list(USER).unwrap().resources, vec![pending]);

    let expired = fixture(Duration::from_millis(1));
    std::thread::sleep(Duration::from_millis(10));
    assert!(expired.state.list(USER).unwrap().resources.is_empty());
}
