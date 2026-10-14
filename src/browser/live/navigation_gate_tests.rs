//! Document-boundary gate: a Safe approval is an approval of ONE page's
//! content. Every real top-level navigation, same-document navigation, reload,
//! agent/human navigation and event gap must, synchronously, send the exact
//! `(browser_session_id, target_id)` resource back to Pending at a new revision,
//! revoke its grants, and fence every viewer receiver approved before it.
//!
//! Event-driven cases push CDP events through the fake engine and wait on an
//! observable effect (never a sleep-and-hope); "ignored" cases prove the event
//! was consumed by following it with a screencast frame that must still be
//! delivered (the dispatcher is one ordered task).

use std::sync::Arc;
use std::time::Duration;

use aleph_cdp::SessionId;
use aleph_protocol::browser_live::{
    ConversationBinding, DisplayPolicy, IssuerIdentity, PreviewKind, PreviewProvenance,
    PreviewResource, PreviewSource, RedactionStatus, ResourceDecision, ViewerBinding, ViewerOrigin,
    ViewerPermission,
};
use serde_json::json;

use super::lifecycle::BrowserLiveLifecycle;
use super::navigation_gate::{BoundaryScope, ContentGate};
use super::owner_registry::LiveOwnerRegistry;
use super::resource::{ResourceDraft, ResourceError, ResourceRegistry};
use super::stream_hub::LatestFrameHub;
use super::tests::{fixture, frame_event, within};
use super::LiveProfileOwner;
use crate::browser::cdp_backend::{BrowserOperationKind, BrowserOperationRecord};
use crate::gateway::event_bus::GatewayEventBus;

fn user() -> IssuerIdentity {
    IssuerIdentity::User {
        user_id: "user-1".into(),
    }
}

fn viewer_binding(owner: &LiveProfileOwner, target: &str) -> ViewerBinding {
    let mut target_binding = super::tests::binding(target);
    target_binding.browser_session_id = owner.browser_session_id().into();
    ViewerBinding {
        principal: user(),
        task: ConversationBinding {
            session_key: "agent:main:demo".into(),
            run_id: "run-1".into(),
        },
        target: target_binding,
    }
}

fn draft(binding: ViewerBinding, redaction: RedactionStatus) -> ResourceDraft {
    ResourceDraft {
        binding,
        kind: PreviewKind::Page,
        display: DisplayPolicy::AutoRevealCurrentRun,
        source: PreviewSource::BrowserTool,
        provenance: PreviewProvenance::NotManaged,
        redaction,
        ttl: Duration::from_secs(600),
        companion_candidates: Vec::new(),
    }
}

struct Rig {
    server: aleph_cdp::testkit::FakeCdpServer,
    owner: Arc<LiveProfileOwner>,
    lifecycle: BrowserLiveLifecycle,
    /// Strong ref: the owner's engine resolver is a `Weak`, so dropping the
    /// handle makes approved attaches fail with `TabNotFound`.
    _handle: Arc<crate::browser::engine::EngineHandle>,
}

/// A real owner over the fake engine whose boundaries feed a real lifecycle.
async fn rig() -> Rig {
    let (server, owner, handle) = fixture().await;
    let lifecycle = BrowserLiveLifecycle::new(
        ResourceRegistry::default(),
        super::grant::ViewerGrantStore::default(),
    )
    .with_event_bus(Arc::new(GatewayEventBus::default()));
    owner.add_boundary_sink(lifecycle.boundary_sink());
    Rig {
        server,
        owner,
        lifecycle,
        _handle: handle,
    }
}

impl Rig {
    /// Attach `target` and publish a Safe resource bound to it.
    async fn approved(&self, target: &str) -> (PreviewResource, String) {
        let binding = viewer_binding(&self.owner, target);
        self.owner
            .attach_target(binding.target.clone())
            .await
            .unwrap();
        let resource = self
            .lifecycle
            .publish(draft(binding.clone(), RedactionStatus::Safe))
            .unwrap();
        let grant = self
            .lifecycle
            .grants()
            .issue(
                binding.principal.clone(),
                binding,
                &resource.resource_id,
                &[ViewerPermission::Frames],
                ViewerOrigin {
                    scheme: "https".into(),
                    host: "viewer.test".into(),
                    port: 443,
                },
                Duration::from_secs(600),
            )
            .unwrap()
            .grant_id;
        (resource, grant)
    }

    fn current(&self, resource: &PreviewResource) -> PreviewResource {
        self.lifecycle
            .resources()
            .get_active(&resource.resource_id)
            .expect("the resource stays live (demoted, not closed)")
    }

    fn demoted(&self, resource: &PreviewResource) -> bool {
        let now = self.current(resource);
        now.redaction == RedactionStatus::Pending && now.revision > resource.revision
    }

    async fn until_demoted(&self, resource: &PreviewResource) {
        within(
            async {
                while !self.demoted(resource) {
                    tokio::task::yield_now().await;
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            },
            "approval demoted by the document boundary",
        )
        .await;
    }

    /// Push `ev`, then a screencast frame, and wait for that frame to reach
    /// `rx`: proves the dispatcher consumed `ev` (and left everything alone).
    async fn push_and_flush(
        &self,
        ev: serde_json::Value,
        session: &str,
        rx: &mut super::stream_hub::LatestFrameReceiver,
        ack_id: u64,
    ) {
        self.server.push_event(ev);
        self.server
            .push_event(frame_event(session, ack_id, "marker"));
        let frame = within(rx.next(), "marker frame behind an ignored event")
            .await
            .expect("an ignored event must not fence the viewer");
        assert_eq!(&*frame.data, b"marker");
    }
}

fn frame_navigated(session: &str, frame: serde_json::Value) -> serde_json::Value {
    json!({
        "method": "Page.frameNavigated",
        "sessionId": session,
        "params": { "frame": frame }
    })
}

#[tokio::test]
async fn top_level_frame_navigated_demotes_safe_revokes_grants_and_fences_viewer() {
    let rig = rig().await;
    let (resource, grant) = rig.approved("T1").await;
    let mut viewer = rig.owner.subscribe_latest("T1").unwrap();
    // A frame of the OLD page is already sitting in the hub slot.
    rig.server.push_event(frame_event("S1", 1, "old-page"));
    rig.server.push_event(frame_navigated(
        "S1",
        json!({ "id": "MAIN", "loaderId": "L2", "url": "https://other.test/" }),
    ));
    rig.until_demoted(&resource).await;

    let now = rig.current(&resource);
    assert_eq!(now.redaction, RedactionStatus::Pending);
    assert_eq!(now.revision, resource.revision + 1);
    assert!(
        rig.lifecycle.grants().grant_info(&grant).is_none(),
        "the old approval's grant is revoked, not left to age out"
    );
    // The stale frame in the slot is never delivered; the viewer is fenced.
    assert!(within(viewer.next(), "fenced viewer").await.is_none());
    assert!(viewer.is_fenced(), "fenced, not merely closed");
    // A decide composed against the previous page's revision is stale.
    let stale = rig.lifecycle.resources().decide(
        &resource.resource_id,
        &user(),
        resource.revision,
        ResourceDecision::Safe,
    );
    assert!(matches!(stale, Err(ResourceError::StaleRevision)));
}

#[tokio::test]
async fn obscura_stale_loader_navigation_still_fences() {
    // Obscura reports a click-driven navigation as ONE frameNavigated carrying
    // the OLD loader id. Loader-id comparison cannot see it; the gate must.
    let rig = rig().await;
    let (resource, _grant) = rig.approved("T1").await;
    let first = json!({ "id": "MAIN", "loaderId": "SAME", "url": "https://a.test/" });
    rig.server.push_event(frame_navigated("S1", first.clone()));
    rig.until_demoted(&resource).await;

    // Approve again (as the owner would), then navigate with the SAME loader id.
    let again = rig
        .lifecycle
        .republish(
            &resource.resource_id,
            rig.current(&resource).revision,
            draft(viewer_binding(&rig.owner, "T1"), RedactionStatus::Safe),
        )
        .unwrap();
    assert_eq!(again.redaction, RedactionStatus::Safe);
    rig.server.push_event(frame_navigated("S1", first));
    rig.until_demoted(&again).await;
}

#[tokio::test]
async fn sub_frame_navigation_is_not_a_page_boundary() {
    let rig = rig().await;
    let (resource, _grant) = rig.approved("T1").await;
    let mut viewer = rig.owner.subscribe_latest("T1").unwrap();
    rig.push_and_flush(
        frame_navigated(
            "S1",
            json!({ "id": "CHILD", "parentId": "MAIN", "loaderId": "LC", "url": "https://ad.test/" }),
        ),
        "S1",
        &mut viewer,
        1,
    )
    .await;
    let now = rig.current(&resource);
    assert_eq!(
        (now.redaction, now.revision),
        (RedactionStatus::Safe, resource.revision)
    );
}

#[tokio::test]
async fn same_document_navigation_demotes_and_fences() {
    let rig = rig().await;
    let (resource, grant) = rig.approved("T1").await;
    let mut viewer = rig.owner.subscribe_latest("T1").unwrap();
    rig.server.push_event(json!({
        "method": "Page.navigatedWithinDocument",
        "sessionId": "S1",
        "params": { "frameId": "MAIN", "url": "https://a.test/#other" }
    }));
    rig.until_demoted(&resource).await;
    assert!(rig.lifecycle.grants().grant_info(&grant).is_none());
    assert!(within(viewer.next(), "fenced viewer").await.is_none());
    assert!(viewer.is_fenced());
}

#[tokio::test]
async fn same_document_navigation_of_a_known_other_frame_is_ignored() {
    let rig = rig().await;
    let (resource, _grant) = rig.approved("T1").await;
    // Teach the owner the main frame id (this is itself a boundary) ...
    rig.server.push_event(frame_navigated(
        "S1",
        json!({ "id": "MAIN", "loaderId": "L1" }),
    ));
    rig.until_demoted(&resource).await;
    // ... approve the new page ...
    let approved = rig
        .lifecycle
        .republish(
            &resource.resource_id,
            rig.current(&resource).revision,
            draft(viewer_binding(&rig.owner, "T1"), RedactionStatus::Safe),
        )
        .unwrap();
    let mut viewer = rig.owner.subscribe_latest("T1").unwrap();
    // ... and an iframe's pushState must leave it alone.
    rig.push_and_flush(
        json!({
            "method": "Page.navigatedWithinDocument",
            "sessionId": "S1",
            "params": { "frameId": "CHILD", "url": "https://ad.test/#x" }
        }),
        "S1",
        &mut viewer,
        2,
    )
    .await;
    let now = rig.current(&approved);
    assert_eq!(
        (now.redaction, now.revision),
        (RedactionStatus::Safe, approved.revision)
    );
}

#[tokio::test]
async fn unattributable_navigation_events_fail_closed_for_every_target_of_the_session() {
    let rig = rig().await;
    let (r1, _g1) = rig.approved("T1").await;
    let (r2, _g2) = rig.approved("T2").await;
    // Unknown CDP session: not guessed to be T1 or T2.
    rig.server.push_event(frame_navigated(
        "S9",
        json!({ "id": "MAIN", "loaderId": "L" }),
    ));
    rig.until_demoted(&r1).await;
    rig.until_demoted(&r2).await;
}

#[tokio::test]
async fn malformed_navigation_event_fails_closed() {
    let rig = rig().await;
    let (resource, _grant) = rig.approved("T1").await;
    rig.server.push_event(json!({
        "method": "Page.frameNavigated",
        "sessionId": "S1",
        "params": {}
    }));
    rig.until_demoted(&resource).await;
}

#[tokio::test]
async fn navigation_of_one_target_leaves_the_other_target_approved() {
    let rig = rig().await;
    let (r1, _g1) = rig.approved("T1").await;
    let (r2, _g2) = rig.approved("T2").await;
    let mut other = rig.owner.subscribe_latest("T2").unwrap();
    rig.server.push_event(frame_navigated(
        "S1",
        json!({ "id": "MAIN", "loaderId": "L" }),
    ));
    rig.until_demoted(&r1).await;
    rig.push_and_flush(
        json!({ "method": "Target.targetInfoChanged", "params": {} }),
        "S2",
        &mut other,
        1,
    )
    .await;
    let still = rig.current(&r2);
    assert_eq!(
        (still.redaction, still.revision),
        (RedactionStatus::Safe, r2.revision)
    );
}

#[tokio::test]
async fn agent_navigation_declares_the_boundary_before_returning() {
    // Reload / history / navigate call this BEFORE the engine is told, so the
    // old approval is gone with no event round trip at all.
    let rig = rig().await;
    let (resource, grant) = rig.approved("T1").await;
    let mut viewer = rig.owner.subscribe_latest("T1").unwrap();
    rig.owner
        .agent_navigation("T1", &SessionId("S1".into()))
        .await;
    assert!(rig.demoted(&resource), "synchronous, no await needed");
    assert!(rig.lifecycle.grants().grant_info(&grant).is_none());
    assert!(within(viewer.next(), "fenced viewer").await.is_none());
}

#[tokio::test]
async fn event_gap_fails_closed() {
    let rig = rig().await;
    let (resource, grant) = rig.approved("T1").await;
    rig.owner.note_lag(1);
    assert!(rig.demoted(&resource));
    assert!(rig.lifecycle.grants().grant_info(&grant).is_none());
}

#[tokio::test]
async fn receiver_taken_after_a_boundary_never_sees_the_previous_documents_frame() {
    let rig = rig().await;
    let (resource, _grant) = rig.approved("T1").await;
    rig.server.push_event(frame_event("S1", 1, "old-page"));
    rig.server.push_event(frame_navigated(
        "S1",
        json!({ "id": "MAIN", "loaderId": "L2" }),
    ));
    rig.until_demoted(&resource).await;
    let mut late = rig.owner.subscribe_latest("T1").unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(150), late.next())
            .await
            .is_err(),
        "the old page's last frame was cleared with the boundary"
    );
    rig.server.push_event(frame_event("S1", 2, "new-page"));
    let frame = within(late.next(), "first frame of the new document")
        .await
        .unwrap();
    assert_eq!(&*frame.data, b"new-page");
    assert!(late.admits(&frame));
}

#[tokio::test]
async fn human_navigate_input_declares_the_boundary_first() {
    use aleph_protocol::browser_live::HumanInputEvent;
    let rig = rig().await;
    let (resource, _grant) = rig.approved("T1").await;
    // The fake engine may never answer `Page.navigate`; the boundary is
    // declared BEFORE it is sent, so a short timeout is enough to observe it.
    let _ = tokio::time::timeout(
        Duration::from_millis(500),
        rig.owner.dispatch_human_input_gated(
            "T1",
            HumanInputEvent::Navigate {
                url: "https://example.test/".into(),
            },
            None,
        ),
    )
    .await;
    assert!(rig.demoted(&resource));
}

#[tokio::test]
async fn registering_an_operation_installs_the_registry_sink_on_its_owner() {
    let (_server, owner, handle) = fixture().await;
    let lifecycle = BrowserLiveLifecycle::new(
        ResourceRegistry::default(),
        super::grant::ViewerGrantStore::default(),
    )
    .with_event_bus(Arc::new(GatewayEventBus::default()));
    let owners = LiveOwnerRegistry::default();
    owners.set_boundary_sink(lifecycle.boundary_sink());
    let resource = lifecycle
        .publish(draft(viewer_binding(&owner, "T1"), RedactionStatus::Safe))
        .unwrap();
    let record = BrowserOperationRecord::capture_for_test(
        &owner,
        &handle,
        "T1".into(),
        BrowserOperationKind::Open,
    );
    owners.register_operation(&record).unwrap();
    // Registering again (a join) must not duplicate the sink.
    owners.register_operation(&record).unwrap();
    assert_eq!(owner.boundary_sink_count(), 1);
    owner.agent_navigation("T1", &SessionId("S1".into())).await;
    let now = lifecycle
        .resources()
        .get_active(&resource.resource_id)
        .unwrap();
    assert_eq!(
        (now.redaction, now.revision),
        (RedactionStatus::Pending, resource.revision + 1)
    );
}

#[test]
fn registry_demotion_is_exact_about_session_and_target() {
    let registry = ResourceRegistry::default();
    let mk = |session: &str, target: &str| {
        let mut b = super::tests::binding(target);
        b.browser_session_id = session.into();
        ViewerBinding {
            principal: user(),
            task: ConversationBinding {
                session_key: "agent:main:demo".into(),
                run_id: "run-1".into(),
            },
            target: b,
        }
    };
    let mut ids = Vec::new();
    for (s, t) in [("bs", "T1"), ("bs", "T2"), ("other", "T1")] {
        ids.push(
            registry
                .publish(draft(mk(s, t), RedactionStatus::Safe))
                .unwrap()
                .resource_id,
        );
    }
    let out = registry.demote_for_navigation("p", "bs", &BoundaryScope::Target("T1".into()));
    assert_eq!(out.demoted.len(), 1);
    assert_eq!(out.demoted[0].resource_id, ids[0]);
    let state = |i: usize| {
        let r = registry.get_active(&ids[i]).unwrap();
        (r.redaction, r.revision)
    };
    assert_eq!(state(0), (RedactionStatus::Pending, 2));
    assert_eq!(state(1), (RedactionStatus::Safe, 1));
    assert_eq!(state(2), (RedactionStatus::Safe, 1));
    // AllTargets stays inside the browser session; a wrong profile matches nothing.
    assert_eq!(
        registry
            .demote_for_navigation("p", "bs", &BoundaryScope::AllTargets)
            .demoted
            .len(),
        2
    );
    assert!(registry
        .demote_for_navigation("nope", "bs", &BoundaryScope::AllTargets)
        .demoted
        .is_empty());
    assert_eq!(state(2), (RedactionStatus::Safe, 1));
}

#[tokio::test]
async fn hub_receiver_is_pinned_to_the_generation_it_was_approved_under() {
    let gate = ContentGate::default();
    let mut hub = LatestFrameHub::with_gate(gate.clone());
    let mut rx = hub.subscribe();
    hub.publish(1, vec![1], None, 1);
    let f = within(rx.next(), "frame of the approved document")
        .await
        .unwrap();
    assert!(rx.admits(&f));
    // A boundary passes after the frame was taken but before it is sent.
    gate.advance();
    assert!(!rx.admits(&f), "last-moment check refuses a stale frame");
    hub.publish(1, vec![2], None, 2);
    assert!(within(rx.next(), "fenced receiver").await.is_none());
    assert!(rx.is_fenced());
    // Fresh receivers start on the new generation.
    let mut fresh = hub.subscribe();
    let f2 = within(fresh.next(), "new document frame").await.unwrap();
    assert_eq!(&*f2.data, &[2u8][..]);
    assert!(fresh.admits(&f2));
}
