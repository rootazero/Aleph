//! Viewer attachment ownership: N viewers share ONE attachment per target and
//! only the last one out releases it. No sleeps: every interleaving is pinned
//! with locks and cooperative yields on the current-thread test runtime.

use std::time::Duration;

use aleph_cdp::testkit::{FakeCdpServer, Responder};
use aleph_cdp::SessionId;
use aleph_protocol::browser_live::{
    BrowserTargetBinding, ConversationBinding, DisplayPolicy, IssuerIdentity, PreviewKind,
    PreviewProvenance, PreviewResource, PreviewSource, RedactionStatus, ViewerBinding,
    ViewerOrigin, ViewerPermission, ViewerSecret, ViewportCssPx,
};

use super::grant::ExchangedGrant;
use super::owner_registry::LiveOwnerRegistry;
use super::resource::{ResourceDraft, ResourceRegistry};
use super::tests::{binding, fixture, fixture_with, frame_event, within};
use super::viewer_backend::{LiveViewerBackend, ViewerBackend};
use super::LiveProfileOwner;
use crate::browser::cdp_backend::{BrowserOperationKind, BrowserOperationRecord};
use crate::browser::engine::EngineHandle;
use crate::browser::network_policy::BrowserSsrfGuard;
use std::sync::Arc;

/// Cooperative yields: on the current-thread runtime every ready task runs to
/// its next real suspension point (lock / network) in this many rounds.
async fn settle() {
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
}

async fn until(mut cond: impl FnMut() -> bool, what: &str) {
    within(
        async {
            while !cond() {
                tokio::task::yield_now().await;
            }
        },
        what,
    )
    .await
}

struct Rig {
    server: FakeCdpServer,
    owner: Arc<LiveProfileOwner>,
    backend: Arc<LiveViewerBackend>,
    viewer: ViewerBinding,
    resource: PreviewResource,
}

fn viewer_binding(target: BrowserTargetBinding) -> ViewerBinding {
    ViewerBinding {
        principal: IssuerIdentity::User {
            user_id: "user".into(),
        },
        task: ConversationBinding {
            session_key: "session".into(),
            run_id: "run".into(),
        },
        target,
    }
}

fn draft(binding: ViewerBinding) -> ResourceDraft {
    ResourceDraft {
        binding,
        kind: PreviewKind::Page,
        display: DisplayPolicy::AutoRevealCurrentRun,
        source: PreviewSource::BrowserTool,
        provenance: PreviewProvenance::NotManaged,
        redaction: RedactionStatus::Safe,
        ttl: Duration::from_secs(600),
        companion_candidates: Vec::new(),
    }
}

fn grant(viewer: &ViewerBinding, resource: &PreviewResource, id: &str) -> ExchangedGrant {
    ExchangedGrant {
        grant_id: id.into(),
        viewer_session_id: format!("vs-{id}"),
        rotation_generation: 0,
        current_secret: ViewerSecret("s".into()),
        binding: viewer.clone(),
        resource_id: resource.resource_id.clone(),
        permissions: vec![ViewerPermission::Frames],
        origin: ViewerOrigin {
            scheme: "http".into(),
            host: "localhost".into(),
            port: 1,
        },
        expires_at_ms: resource.expires_at_ms,
    }
}

async fn rig() -> Rig {
    let (server, owner, handle) = fixture().await;
    rig_over(server, owner, &handle)
}

fn rig_over(
    server: FakeCdpServer,
    owner: Arc<LiveProfileOwner>,
    handle: &Arc<EngineHandle>,
) -> Rig {
    let owners = LiveOwnerRegistry::default();
    owners
        .register_operation(&BrowserOperationRecord::capture_for_test(
            &owner,
            handle,
            "T1".into(),
            BrowserOperationKind::Open,
        ))
        .unwrap();
    let resources = ResourceRegistry::default();
    let backend = Arc::new(LiveViewerBackend::new(
        owners,
        Arc::new(|_| Ok(())),
        ViewportCssPx {
            width: 800,
            height: 600,
        },
        BrowserSsrfGuard::default(),
        resources.clone(),
    ));
    let mut target = binding("T1");
    target.browser_session_id = owner.browser_session_id().into();
    let viewer = viewer_binding(target);
    let resource = resources.publish(draft(viewer.clone())).unwrap();
    Rig {
        server,
        owner,
        backend,
        viewer,
        resource,
    }
}

impl Rig {
    fn grant(&self, id: &str) -> ExchangedGrant {
        grant(&self.viewer, &self.resource, id)
    }

    fn attached(&self) -> bool {
        self.owner.subscribe_latest("T1").is_ok()
    }

    fn stops(&self) -> usize {
        self.server.received_for("Page.stopScreencast").len()
    }

    fn starts(&self) -> usize {
        self.server.received_for("Page.startScreencast").len()
    }
}

/// RED against the boolean `AttachCleanup`: A started the attachment, B
/// shares it, A is cancelled mid-bind. A's cleanup used to `close_target`,
/// tearing B's stream and control down.
#[tokio::test]
async fn cancelled_first_prepare_does_not_close_the_shared_attachment() {
    let rig = rig().await;
    // Hold the owner mutation gate: `bind_control` (inside A's prepare, after
    // it attached) parks here, deterministically between attach and bind.
    let gate = rig.owner.mutations.lock().await;
    let a = {
        let (backend, grant) = (rig.backend.clone(), rig.grant("a"));
        tokio::spawn(async move { backend.prepare(&grant).await.map(|_| ()) })
    };
    until(|| rig.attached(), "A attached").await;
    let b = {
        let (backend, grant) = (rig.backend.clone(), rig.grant("b"));
        tokio::spawn(async move { backend.prepare(&grant).await })
    };
    // B shares the attachment (no I/O needed), then queues behind A on the
    // control cache / mutation gate.
    settle().await;
    assert_eq!(rig.starts(), 1, "B reuses A's screencast");

    a.abort();
    assert!(a.await.unwrap_err().is_cancelled());
    settle().await; // any cancellation cleanup A scheduled has now run
    drop(gate);

    let mut prepared = within(b, "B prepare")
        .await
        .unwrap()
        .expect("B must survive A's cancellation: A did not own the attachment B is holding");
    settle().await;
    assert!(rig.attached(), "B's stream is still attached");
    assert!(prepared.control.is_live(), "B's control is live");
    rig.server.push_event(frame_event("S1", 7, "after-cancel"));
    let frame = within(prepared.frames.next(), "frame for B").await;
    assert!(frame.is_some(), "B keeps receiving frames");
    assert_eq!(rig.stops(), 0, "nothing stopped the shared screencast");
}

/// RED against the old code: a successful `PreparedViewer` was disarmed and
/// nothing ever released its attachment.
#[tokio::test]
async fn dropping_the_last_prepared_viewer_releases_attachment_and_control() {
    let rig = rig().await;
    let prepared = rig.backend.prepare(&rig.grant("a")).await.unwrap();
    let control = prepared.control.clone();
    assert!(rig.attached() && control.is_live());
    drop(prepared);
    until(|| !rig.attached(), "attachment released").await;
    until(|| !control.is_live(), "control ended").await;
    until(|| rig.stops() == 1, "screencast stopped").await;
    // The released target can be viewed again from scratch.
    let again = rig.backend.prepare(&rig.grant("c")).await.unwrap();
    assert!(again.control.is_live() && !Arc::ptr_eq(&again.control, &control));
}

// ---- owner-level lease tests -------------------------------------------

fn target_of(owner: &LiveProfileOwner) -> BrowserTargetBinding {
    let mut target = binding("T1");
    target.browser_session_id = owner.browser_session_id().into();
    target
}

fn held(owner: &LiveProfileOwner) -> usize {
    owner.state().attachments.get("T1").map_or(0, |e| e.refs)
}

fn stops(server: &FakeCdpServer) -> usize {
    server.received_for("Page.stopScreencast").len()
}

fn starts(server: &FakeCdpServer) -> usize {
    server.received_for("Page.startScreencast").len()
}

async fn bind(owner: &LiveProfileOwner) -> Arc<super::control::TargetControl> {
    owner
        .bind_control(
            "resource",
            viewer_binding(target_of(owner)),
            ViewportCssPx {
                width: 800,
                height: 600,
            },
            Arc::new(|_| Ok(())),
        )
        .await
        .expect("bind control")
}

#[tokio::test]
async fn n_prepared_viewers_share_one_attachment_and_the_last_out_closes_it() {
    let rig = rig().await;
    let a = rig.backend.prepare(&rig.grant("a")).await.unwrap();
    let b = rig.backend.prepare(&rig.grant("b")).await.unwrap();
    let c = rig.backend.prepare(&rig.grant("c")).await.unwrap();
    assert_eq!(held(&rig.owner), 3, "one reference per prepared viewer");
    assert_eq!(rig.starts(), 1, "one engine screencast for all three");
    let control = c.control.clone();

    drop(a);
    drop(b);
    settle().await;
    assert_eq!(held(&rig.owner), 1);
    assert!(rig.attached() && control.is_live(), "one viewer keeps it");
    assert_eq!(rig.stops(), 0);

    drop(c);
    until(|| !rig.attached(), "last viewer releases the attachment").await;
    until(|| !control.is_live(), "control ended with the last viewer").await;
    until(|| rig.stops() == 1, "screencast stopped once").await;
    assert_eq!(held(&rig.owner), 0);
    assert!(rig.owner.state().streams.is_empty());
}

/// The release of the last reference re-checks refcount/epoch under the
/// lifecycle lock: a viewer that re-acquires while the release is queued
/// keeps the attachment.
#[tokio::test]
async fn release_recheck_under_lifecycle_lock_spares_a_concurrent_reacquire() {
    let (server, owner, _handle) = fixture().await;
    let first = owner.acquire_attachment(target_of(&owner)).await.unwrap();
    let life = owner.lifecycle.lock().await;
    // The re-acquire queues on the lifecycle lock first (FIFO) ...
    let again = {
        let owner = owner.clone();
        tokio::spawn(async move { owner.acquire_attachment(target_of(&owner)).await })
    };
    settle().await;
    // ... then the old lease returns its (so far last) reference.
    drop(first);
    settle().await;
    assert_eq!(held(&owner), 0, "the reference was returned synchronously");
    drop(life);

    let second = within(again, "re-acquire")
        .await
        .unwrap()
        .unwrap_or_else(|_| panic!("re-acquire must succeed"));
    settle().await;
    assert_eq!(held(&owner), 1);
    assert!(owner.subscribe_latest("T1").is_ok(), "still attached");
    assert_eq!(stops(&server), 0, "the queued release did not close it");
    assert_eq!(starts(&server), 1);

    second.release().await;
    assert!(owner.subscribe_latest("T1").is_err());
    assert_eq!(stops(&server), 1);
}

#[tokio::test]
async fn stale_lease_after_close_and_reacquire_releases_nothing() {
    let (server, owner, _handle) = fixture().await;
    let old = owner.acquire_attachment(target_of(&owner)).await.unwrap();
    owner.close_target("T1").await.unwrap();
    assert_eq!(stops(&server), 1);

    let new = owner.acquire_attachment(target_of(&owner)).await.unwrap();
    assert_ne!(old.epoch, new.epoch, "close + reacquire starts a new epoch");
    assert_eq!(starts(&server), 2);

    old.release().await;
    settle().await;
    assert_eq!(
        held(&owner),
        1,
        "the stale lease took nothing from the new epoch"
    );
    assert!(owner.subscribe_latest("T1").is_ok());
    assert_eq!(
        stops(&server),
        1,
        "no second stop: the new epoch is untouched"
    );

    new.release().await;
    assert!(owner.subscribe_latest("T1").is_err());
    assert_eq!(stops(&server), 2);
}

#[tokio::test]
async fn last_lease_release_leaves_the_recording_running() {
    let (server, owner, _handle) = fixture().await;
    let mut tap = owner
        .observe_recording(
            "T1",
            &SessionId("S1".to_string()),
            aleph_cdp::methods::screencast::ScreencastFormat::Jpeg { quality: 80 },
        )
        .await
        .expect("observe");
    let lease = owner.acquire_attachment(target_of(&owner)).await.unwrap();
    let control = bind(&owner).await;
    assert_eq!(
        starts(&server),
        1,
        "the viewer shares the recording's screencast"
    );

    lease.release().await;
    assert!(owner.subscribe_latest("T1").is_err(), "viewer detached");
    assert!(!control.is_live(), "viewer control ended");
    assert_eq!(
        stops(&server),
        0,
        "the recording still holds the screencast"
    );

    server.push_event(frame_event("S1", 5, "recorded"));
    let frame = within(tap.frames.recv(), "recording frame")
        .await
        .expect("the recording keeps receiving frames");
    assert_eq!(&*frame.data, b"recorded");

    owner.release_recording("T1", &tap.captured).await;
    assert_eq!(stops(&server), 1, "recording was the last holder");
}

#[tokio::test]
async fn foreign_session_or_profile_is_rejected_without_side_effects() {
    let (server, owner, _handle) = fixture().await;
    let mut other_profile = target_of(&owner);
    other_profile.profile = "other".into();
    let mut other_session = target_of(&owner);
    other_session.browser_session_id = "someone-else".into();
    assert!(owner.acquire_attachment(other_profile).await.is_err());
    assert!(owner.acquire_attachment(other_session).await.is_err());
    // The default test binding names session "bs", which is not this owner's.
    assert!(owner.acquire_attachment(binding("T1")).await.is_err());
    assert_eq!(starts(&server), 0, "no screencast for a foreign identity");
    assert!(owner.state().streams.is_empty());
    assert!(owner.state().attachments.is_empty());
}

#[tokio::test]
async fn cancelled_acquire_leaks_no_reference_or_stream() {
    let (server, owner, _handle) = fixture_with(Responder::Delay(
        // Never answers within the test: the start stays in flight until
        // the acquiring task is aborted. (`Hang` would also silence the
        // rollback's stopScreencast.)
        Duration::from_secs(60),
        Box::new(Responder::Reply(serde_json::json!({}))),
    ))
    .await;
    let pending = {
        let owner = owner.clone();
        tokio::spawn(async move {
            owner
                .acquire_attachment(target_of(&owner))
                .await
                .map(|_| ())
        })
    };
    until(|| starts(&server) == 1, "startScreencast in flight").await;
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    until(|| stops(&server) == 1, "cancelled start rolled back").await;
    settle().await;
    assert!(owner.state().streams.is_empty(), "no stream survives");
    assert!(
        owner.state().attachments.is_empty(),
        "no reference survives"
    );
    assert_eq!(held(&owner), 0);
}

#[tokio::test]
async fn lease_release_does_not_tear_down_a_legacy_attachment() {
    let (server, owner, _handle) = fixture().await;
    assert!(owner.attach_target(target_of(&owner)).await.unwrap());
    let lease = owner.acquire_attachment(target_of(&owner)).await.unwrap();
    assert_eq!(held(&owner), 1);
    lease.release().await;
    settle().await;
    assert!(
        owner.subscribe_latest("T1").is_ok(),
        "legacy attach survives"
    );
    assert_eq!(stops(&server), 0);

    let dropped = owner.acquire_attachment(target_of(&owner)).await.unwrap();
    drop(dropped);
    settle().await;
    assert!(owner.subscribe_latest("T1").is_ok());
    assert_eq!(stops(&server), 0);

    owner.close_target("T1").await.unwrap();
    assert_eq!(
        stops(&server),
        1,
        "only close_target releases a legacy attach"
    );
}

/// A legacy `attach_target` that lands between the last lease's return and
/// its queued release takes the attachment over: the release stands down.
#[tokio::test]
async fn legacy_attach_adopts_an_attachment_whose_release_is_pending() {
    let (server, owner, _handle) = fixture().await;
    let lease = owner.acquire_attachment(target_of(&owner)).await.unwrap();
    let life = owner.lifecycle.lock().await;
    // The legacy attach queues first ...
    let attach = {
        let owner = owner.clone();
        tokio::spawn(async move { owner.attach_target(target_of(&owner)).await })
    };
    settle().await;
    // ... then the lease returns its last reference and queues its release.
    drop(lease);
    settle().await;
    drop(life);

    let owned = within(attach, "legacy attach").await.unwrap().unwrap();
    assert!(owned, "the legacy caller owns the adopted attachment");
    settle().await;
    assert!(
        owner.subscribe_latest("T1").is_ok(),
        "the release stood down"
    );
    assert_eq!(stops(&server), 0);

    owner.close_target("T1").await.unwrap();
    assert_eq!(stops(&server), 1);
}
