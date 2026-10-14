//! Browser-live resource lifecycle coordinator.
//!
//! [`ResourceRegistry`] deliberately does not own grants (no hidden lock order
//! between the two stores), so *something* has to compose "registry transition
//! -> grant revocation -> typed event". That something is this coordinator; every
//! close/refusal/expiry/non-Safe-revision path goes through it so a future
//! caller (target closed, engine exited, owner shutdown) cannot forget to
//! revoke.
//!
//! Ordering per transition (all under one publication mutex, shared by clones):
//!
//! 1. the registry completes its state change (it never calls the grant store
//!    while holding its own lock);
//! 2. every affected resource's grants are revoked, for the whole batch, before
//!    any event is emitted;
//! 3. the typed `resource` / `closed` frame is emitted.
//!
//! Event failure never rolls back 1 or 2: authority is revoked even when the
//! bus is down or absent. Only *publishing* requires a bus (a new resource must
//! not be created without a way to announce it, so `publish` returns
//! `NotConfigured` before touching the registry). Revocation never depends on
//! the bus: `close` / `close_binding` / `sweep_expired` and the refusal /
//! expiry paths of `republish` always complete the registry transition and
//! grant revocation first; the closed event is then best-effort and an absent
//! bus is reported as `NotConfigured` *after* authority is already gone. A
//! `republish` that would succeed without a bus cannot be announced, so it is
//! closed (`Revoked`) and revoked like any other announcement failure.
//!
//! Grants are also revoked when a revision is not affirmatively `Safe` or when
//! it *shortens* the resource's expiry: an already-attached viewer does not
//! re-query the registry, so its grant must not outlive the new deadline.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use aleph_protocol::browser_live::{
    BrowserLiveClosed, BusEvent, ClosedReason, IssuerIdentity, PreviewResource, RedactionStatus,
    ResourceDecision, ViewerBinding,
};

use super::grant::ViewerGrantStore;
use super::navigation_gate::{DocumentBoundary, DocumentBoundarySink, OwnerTerminalSink};
use super::resource::{ResourceDraft, ResourceError, ResourceRegistry};
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::events::GatewayEventFrame;

#[derive(Debug, thiserror::Error)]
pub(crate) enum LifecycleError {
    #[error("browser-live lifecycle event bus not configured")]
    NotConfigured,
    #[error(transparent)]
    Registry(#[from] ResourceError),
    #[error(transparent)]
    Event(#[from] serde_json::Error),
}

/// The ONE owner of the registry/grant composition. Clones share the same
/// stores, bus and publication mutex; other components (issuer, viewer
/// backend) may read the stores through [`resources`](Self::resources) /
/// [`grants`](Self::grants) but must never construct their own.
#[derive(Clone)]
pub(crate) struct BrowserLiveLifecycle {
    resources: ResourceRegistry,
    grants: ViewerGrantStore,
    event_bus: Option<Arc<GatewayEventBus>>,
    // Serialize registry transition + revocation + event delivery across clones.
    publication: Arc<Mutex<()>>,
}

impl BrowserLiveLifecycle {
    pub(crate) fn new(resources: ResourceRegistry, grants: ViewerGrantStore) -> Self {
        Self {
            resources,
            grants,
            event_bus: None,
            publication: Arc::default(),
        }
    }

    /// Same stores and mutex, with event delivery enabled.
    pub(crate) fn with_event_bus(&self, event_bus: Arc<GatewayEventBus>) -> Self {
        Self {
            event_bus: Some(event_bus),
            ..self.clone()
        }
    }

    pub(crate) fn resources(&self) -> &ResourceRegistry {
        &self.resources
    }

    pub(crate) fn grants(&self) -> &ViewerGrantStore {
        &self.grants
    }

    pub(crate) fn has_event_bus(&self) -> bool {
        self.event_bus.is_some()
    }

    /// The document-boundary sink for owners (see
    /// [`DocumentBoundarySink`]): install ONE of these on the owner registry
    /// at composition time (`LiveOwnerRegistry::set_boundary_sink`).
    pub(crate) fn boundary_sink(&self) -> Arc<dyn DocumentBoundarySink> {
        Arc::new(self.clone())
    }

    /// The owner terminal sink (see [`OwnerTerminalSink`]): installed next to
    /// the boundary sink (`LiveOwnerRegistry::set_terminal_sink`).
    pub(crate) fn terminal_sink(&self) -> Arc<dyn OwnerTerminalSink> {
        Arc::new(self.clone())
    }

    /// Publish a new resource (or a new revision of the binding's live one).
    pub(crate) fn publish(&self, draft: ResourceDraft) -> Result<PreviewResource, LifecycleError> {
        self.bus()?;
        let _guard = self.serialize();
        let result = self.resources.publish(draft);
        // Prior expiry is not observable through the registry's public API for
        // a same-binding revision; only `republish` can compare deadlines.
        self.finish_resource(result, None)
    }

    /// Compare-and-set revision of an existing live resource.
    pub(crate) fn republish(
        &self,
        resource_id: &str,
        expected_revision: u64,
        draft: ResourceDraft,
    ) -> Result<PreviewResource, LifecycleError> {
        let _guard = self.serialize();
        // Read under the publication lock so no other lifecycle transition can
        // interleave between the read and the compare-and-set.
        let previous_expiry = self
            .resources
            .get_active(resource_id)
            .map(|previous| previous.expires_at_ms);
        let result = self
            .resources
            .republish(resource_id, expected_revision, draft);
        self.finish_resource(result, previous_expiry)
    }

    /// Apply the authenticated owner's decision on the exact current revision
    /// of a `Pending` resource (`browser.viewer.resource.decide`).
    ///
    /// Same composition as every other transition: publication mutex -> one
    /// atomic registry transition -> grant revocation -> typed event. Without
    /// an event bus the decision is denied BEFORE any mutation (a verdict that
    /// cannot be announced must not change state). `Safe` revokes the
    /// resource's grants too: a new revision is a permission boundary, so no
    /// grant minted against an earlier revision survives it. If the announce
    /// fails after the transition, the resource is closed and revoked like any
    /// other announcement failure (see `finish_resource`).
    ///
    /// `Refused` returns the canonical `Closed` event (`ClosedReason::Revoked`).
    pub(crate) fn decide(
        &self,
        user: &str,
        resource_id: &str,
        expected_revision: u64,
        decision: ResourceDecision,
    ) -> Result<BusEvent, LifecycleError> {
        self.bus()?;
        let owner = IssuerIdentity::User {
            user_id: user.to_string(),
        };
        let _guard = self.serialize();
        match self
            .resources
            .decide(resource_id, &owner, expected_revision, decision)
        {
            Ok(BusEvent::Resource { resource }) => {
                self.grants.revoke_resource(&resource.resource_id);
                let announced = self.bus().and_then(|bus| {
                    bus.publish_frame(&GatewayEventFrame::BrowserLiveResource {
                        resource: resource.clone(),
                    })
                    .map_err(LifecycleError::from)
                });
                if let Err(error) = announced {
                    if let Some(closed) = self
                        .resources
                        .close(&resource.resource_id, ClosedReason::Revoked)
                    {
                        let _ = self.revoke_and_emit(&closed);
                    }
                    return Err(error);
                }
                Ok(BusEvent::Resource { resource })
            }
            Ok(BusEvent::Closed { closed }) => {
                self.revoke_and_emit(&closed)?;
                Ok(BusEvent::Closed { closed })
            }
            // The registry only ever returns Resource/Closed.
            Ok(BusEvent::Intervention { .. }) => Err(ResourceError::UnknownResource.into()),
            Err(error) => {
                if error == ResourceError::Expired {
                    // Owner-expired: finish its lifecycle now, same rule as a
                    // refused expired republish. Never reached for non-owners.
                    self.sweep_locked()?;
                }
                Err(error.into())
            }
        }
    }

    /// Close a live resource. `Ok(None)` when it was unknown or already closed:
    /// nothing is revoked or emitted a second time.
    pub(crate) fn close(
        &self,
        resource_id: &str,
        reason: ClosedReason,
    ) -> Result<Option<BrowserLiveClosed>, LifecycleError> {
        let _guard = self.serialize();
        let Some(closed) = self.resources.close(resource_id, reason) else {
            return Ok(None);
        };
        self.revoke_and_emit(&closed)?;
        Ok(Some(closed))
    }

    /// Close the live resource bound to exactly `binding` (target closed,
    /// engine exited: events that know the binding, not the resource id).
    pub(crate) fn close_binding(
        &self,
        binding: &ViewerBinding,
        reason: ClosedReason,
    ) -> Result<Option<BrowserLiveClosed>, LifecycleError> {
        let _guard = self.serialize();
        let Some(closed) = self.resources.close_binding(binding, reason) else {
            return Ok(None);
        };
        self.revoke_and_emit(&closed)?;
        Ok(Some(closed))
    }

    /// Close every live Browser resource bound to exactly
    /// `(profile, browser_session_id, target_id)` (the engine destroyed that
    /// target). One registry scan; the whole batch is revoked before any event
    /// is emitted. Authority is gone even without an event bus (reported as
    /// the first error, `NotConfigured`, after the fact).
    pub(crate) fn close_target(
        &self,
        profile: &str,
        browser_session_id: &str,
        target_id: &str,
        reason: ClosedReason,
    ) -> Result<Vec<BrowserLiveClosed>, LifecycleError> {
        let _guard = self.serialize();
        let closed = self
            .resources
            .close_target(profile, browser_session_id, target_id, reason);
        self.revoke_and_emit_batch(closed)
    }

    /// As [`Self::close_target`], for every target of the exact
    /// `(profile, browser_session_id)` (the owner ended).
    pub(crate) fn close_session(
        &self,
        profile: &str,
        browser_session_id: &str,
        reason: ClosedReason,
    ) -> Result<Vec<BrowserLiveClosed>, LifecycleError> {
        let _guard = self.serialize();
        let closed = self
            .resources
            .close_browser_session(profile, browser_session_id, reason);
        self.revoke_and_emit_batch(closed)
    }

    /// Owner lifecycle maintenance: transition every expired resource, revoke
    /// the whole batch, then emit each canonical close event.
    pub(crate) fn sweep_expired(&self) -> Result<usize, LifecycleError> {
        let _guard = self.serialize();
        self.sweep_locked()
    }

    fn serialize(&self) -> MutexGuard<'_, ()> {
        self.publication
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn bus(&self) -> Result<&GatewayEventBus, LifecycleError> {
        self.event_bus
            .as_deref()
            .ok_or(LifecycleError::NotConfigured)
    }

    /// Caller holds the publication mutex.
    fn sweep_locked(&self) -> Result<usize, LifecycleError> {
        let closed = self.resources.sweep();
        // Revoke the whole batch before emission, even if emission fails.
        for event in &closed {
            self.grants.revoke_resource(&event.resource_id);
        }
        let mut first_error = None;
        for event in &closed {
            if let Err(error) = self.emit_closed(event) {
                first_error.get_or_insert(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(closed.len()),
        }
    }

    /// Caller holds the publication mutex.
    fn finish_resource(
        &self,
        result: Result<PreviewResource, ResourceError>,
        previous_expiry_ms: Option<i64>,
    ) -> Result<PreviewResource, LifecycleError> {
        match result {
            Ok(resource) => {
                // A revision that is not affirmatively Safe (e.g. Safe -> Pending)
                // must not leave earlier grants live; viewer prepare/issue also
                // fail closed on the registry's redaction verdict. Likewise a
                // revision that shortens the expiry: attached viewers do not
                // re-query the registry before the next sweep.
                let shortened =
                    previous_expiry_ms.is_some_and(|previous| resource.expires_at_ms < previous);
                if resource.redaction != RedactionStatus::Safe || shortened {
                    self.grants.revoke_resource(&resource.resource_id);
                }
                // Only the payload returned by the authoritative registry leaves.
                let announced = self.bus().and_then(|bus| {
                    bus.publish_frame(&GatewayEventFrame::BrowserLiveResource {
                        resource: resource.clone(),
                    })
                    .map_err(LifecycleError::from)
                });
                if let Err(error) = announced {
                    // An announced-nowhere resource must not stay viewable.
                    if let Some(closed) = self
                        .resources
                        .close(&resource.resource_id, ClosedReason::Revoked)
                    {
                        // Best effort: the bus is already failing; keep the
                        // original error, authority is revoked regardless.
                        let _ = self.revoke_and_emit(&closed);
                    }
                    return Err(error);
                }
                Ok(resource)
            }
            Err(error) => {
                // Refusal closes the live resource inside the registry step.
                if let Some(closed) = error.closed_event() {
                    self.revoke_and_emit(closed)?;
                }
                if error == ResourceError::Expired {
                    // A refused expired revision: finish its lifecycle now rather
                    // than leaving associated grants live.
                    self.sweep_locked()?;
                }
                Err(error.into())
            }
        }
    }

    fn emit_closed(&self, closed: &BrowserLiveClosed) -> Result<(), LifecycleError> {
        self.bus()?
            .publish_frame(&GatewayEventFrame::BrowserLiveClosed {
                closed: closed.clone(),
            })?;
        Ok(())
    }

    /// Caller holds the publication mutex and has completed the registry
    /// transition. Revokes the whole batch, then emits each event best-effort
    /// (every event is attempted); returns the batch, or the first emit error.
    fn revoke_and_emit_batch(
        &self,
        closed: Vec<BrowserLiveClosed>,
    ) -> Result<Vec<BrowserLiveClosed>, LifecycleError> {
        for event in &closed {
            self.grants.revoke_resource(&event.resource_id);
        }
        let mut first_error = None;
        for event in &closed {
            if let Err(error) = self.emit_closed(event) {
                first_error.get_or_insert(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(closed),
        }
    }

    fn revoke_and_emit(&self, closed: &BrowserLiveClosed) -> Result<(), LifecycleError> {
        self.grants.revoke_resource(&closed.resource_id);
        self.emit_closed(closed)
    }
}

impl DocumentBoundarySink for BrowserLiveLifecycle {
    /// A live owner declared a document boundary. Same composition as every
    /// other transition: publication mutex -> one atomic registry transition
    /// (Pending, revision + 1) -> grant revocation for the whole batch ->
    /// events. Authority is gone before this returns, whether or not an event
    /// bus is configured; the owner calls this synchronously, before it lets
    /// any frame of the new document out.
    fn document_boundary(&self, boundary: &DocumentBoundary) {
        let _guard = self.serialize();
        let outcome = self.resources.demote_for_navigation(
            &boundary.profile,
            &boundary.browser_session_id,
            &boundary.scope,
        );
        for resource in &outcome.demoted {
            self.grants.revoke_resource(&resource.resource_id);
        }
        for closed in &outcome.closed {
            self.grants.revoke_resource(&closed.resource_id);
        }
        // Announcements are best effort: revocation above is the authority.
        if self.event_bus.is_some() {
            for resource in outcome.demoted {
                let _ = self.bus().and_then(|bus| {
                    bus.publish_frame(&GatewayEventFrame::BrowserLiveResource { resource })
                        .map_err(LifecycleError::from)
                });
            }
            for closed in &outcome.closed {
                let _ = self.emit_closed(closed);
            }
        }
    }
}

impl OwnerTerminalSink for BrowserLiveLifecycle {
    /// The exact target was destroyed: close its resources (`TargetClosed`).
    /// Authority is revoked before this returns regardless of the bus; an
    /// announcement failure is only logged (the owner cannot act on it).
    fn target_destroyed(&self, profile: &str, browser_session_id: &str, target_id: &str) {
        let reason = ClosedReason::TargetClosed;
        if let Err(error) = self.close_target(profile, browser_session_id, target_id, reason) {
            tracing::warn!(%error, profile, target_id, "browser-live target close not announced");
        }
    }

    /// The owner ended: close every resource of its browser session.
    fn owner_ended(&self, profile: &str, browser_session_id: &str, reason: ClosedReason) {
        if let Err(error) = self.close_session(profile, browser_session_id, reason) {
            tracing::warn!(%error, profile, "browser-live session close not announced");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::time::Duration;

    use aleph_protocol::browser_live::{
        BrowserTargetBinding, ConversationBinding, DisplayPolicy, IssuerIdentity, PreviewKind,
        PreviewProvenance, PreviewSource, ViewerOrigin, ViewerPermission,
    };
    use tokio::sync::broadcast::{error::TryRecvError, Receiver};

    use super::*;

    const T0: i64 = 1_000_000;

    struct Fixture {
        lifecycle: BrowserLiveLifecycle,
        grants: ViewerGrantStore,
        resources: ResourceRegistry,
        rx: Receiver<GatewayEventFrame>,
        now: Arc<AtomicI64>,
    }

    fn fixture() -> Fixture {
        let now = Arc::new(AtomicI64::new(T0));
        let clock = now.clone();
        let resources =
            ResourceRegistry::with_clock(Arc::new(move || clock.load(Ordering::SeqCst)));
        let grants = ViewerGrantStore::default();
        let bus = Arc::new(GatewayEventBus::default());
        let rx = bus.subscribe_typed();
        Fixture {
            lifecycle: BrowserLiveLifecycle::new(resources.clone(), grants.clone())
                .with_event_bus(bus),
            grants,
            resources,
            rx,
            now,
        }
    }

    fn binding(target_id: &str) -> ViewerBinding {
        ViewerBinding {
            principal: IssuerIdentity::User {
                user_id: "user-1".into(),
            },
            task: ConversationBinding {
                session_key: "agent:main:demo".into(),
                run_id: "run-1".into(),
            },
            target: BrowserTargetBinding {
                profile: "default".into(),
                browser_session_id: "bs-1".into(),
                target_id: target_id.into(),
            },
        }
    }

    fn draft(target_id: &str, redaction: RedactionStatus) -> ResourceDraft {
        ResourceDraft {
            binding: binding(target_id),
            kind: PreviewKind::Page,
            display: DisplayPolicy::AutoRevealCurrentRun,
            source: PreviewSource::BrowserTool,
            provenance: PreviewProvenance::NotManaged,
            redaction,
            ttl: Duration::from_secs(60),
            companion_candidates: Vec::new(),
        }
    }

    fn mint(f: &Fixture, resource: &PreviewResource) -> String {
        let binding = resource.viewer_binding().unwrap();
        f.grants
            .issue(
                binding.principal.clone(),
                binding.clone(),
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
            .grant_id
    }

    fn read_resource(f: &mut Fixture) -> PreviewResource {
        match f.rx.try_recv().unwrap() {
            GatewayEventFrame::BrowserLiveResource { resource } => resource,
            other => panic!("expected resource, got {other:?}"),
        }
    }

    fn read_closed(f: &mut Fixture) -> BrowserLiveClosed {
        match f.rx.try_recv().unwrap() {
            GatewayEventFrame::BrowserLiveClosed { closed } => closed,
            other => panic!("expected closed, got {other:?}"),
        }
    }

    fn assert_empty(f: &mut Fixture) {
        assert!(matches!(f.rx.try_recv(), Err(TryRecvError::Empty)));
    }

    fn no_bus(f: &Fixture) -> BrowserLiveLifecycle {
        BrowserLiveLifecycle::new(f.resources.clone(), f.grants.clone())
    }

    #[test]
    fn missing_bus_publish_creates_nothing() {
        let f = fixture();
        assert!(matches!(
            no_bus(&f).publish(draft("t1", RedactionStatus::Safe)),
            Err(LifecycleError::NotConfigured)
        ));
        assert!(f.resources.get_active("anything").is_none());
        // Nothing was created for the binding either.
        assert!(f
            .resources
            .close_binding(&binding("t1"), ClosedReason::Revoked)
            .is_none());
    }

    #[test]
    fn missing_bus_close_still_closes_and_revokes() {
        let mut f = fixture();
        let live = f
            .lifecycle
            .publish(draft("t1", RedactionStatus::Safe))
            .unwrap();
        read_resource(&mut f);
        let grant = mint(&f, &live);
        assert!(matches!(
            no_bus(&f).close(&live.resource_id, ClosedReason::Revoked),
            Err(LifecycleError::NotConfigured)
        ));
        assert!(f.resources.get_active(&live.resource_id).is_none());
        assert!(f.resources.closed(&live.resource_id).is_some());
        assert!(f.grants.grant_info(&grant).is_none());
        assert_empty(&mut f);
    }

    #[test]
    fn missing_bus_close_binding_still_closes_and_revokes() {
        let mut f = fixture();
        let live = f
            .lifecycle
            .publish(draft("t1", RedactionStatus::Safe))
            .unwrap();
        let other = f
            .lifecycle
            .publish(draft("t2", RedactionStatus::Safe))
            .unwrap();
        read_resource(&mut f);
        read_resource(&mut f);
        let grant = mint(&f, &live);
        let other_grant = mint(&f, &other);
        assert!(matches!(
            no_bus(&f).close_binding(&binding("t1"), ClosedReason::TargetClosed),
            Err(LifecycleError::NotConfigured)
        ));
        assert!(f.resources.get_active(&live.resource_id).is_none());
        assert!(f.grants.grant_info(&grant).is_none());
        assert!(f.resources.get_active(&other.resource_id).is_some());
        assert!(f.grants.grant_info(&other_grant).is_some());
        assert_empty(&mut f);
    }

    #[test]
    fn missing_bus_sweep_still_expires_and_revokes_whole_batch() {
        let mut f = fixture();
        let mut short = draft("t1", RedactionStatus::Safe);
        short.ttl = Duration::from_millis(10);
        let a = f.lifecycle.publish(short.clone()).unwrap();
        short.binding = binding("t2");
        let b = f.lifecycle.publish(short).unwrap();
        let long = f
            .lifecycle
            .publish(draft("t3", RedactionStatus::Safe))
            .unwrap();
        for _ in 0..3 {
            read_resource(&mut f);
        }
        let (ga, gb, gl) = (mint(&f, &a), mint(&f, &b), mint(&f, &long));
        f.now.fetch_add(30, Ordering::SeqCst);
        assert!(matches!(
            no_bus(&f).sweep_expired(),
            Err(LifecycleError::NotConfigured)
        ));
        for (res, grant) in [(&a, &ga), (&b, &gb)] {
            assert!(f.resources.get_active(&res.resource_id).is_none());
            assert!(f.resources.closed(&res.resource_id).is_some());
            assert!(f.grants.grant_info(grant).is_none());
        }
        assert!(f.resources.get_active(&long.resource_id).is_some());
        assert!(f.grants.grant_info(&gl).is_some());
        assert_empty(&mut f);
    }

    #[test]
    fn missing_bus_republish_refusal_expiry_and_success_fail_closed() {
        // Refusal.
        let mut f = fixture();
        let live = f
            .lifecycle
            .publish(draft("t1", RedactionStatus::Safe))
            .unwrap();
        read_resource(&mut f);
        let grant = mint(&f, &live);
        assert!(matches!(
            no_bus(&f).republish(
                &live.resource_id,
                live.revision,
                draft("t1", RedactionStatus::Refused)
            ),
            Err(LifecycleError::NotConfigured)
        ));
        assert!(f.resources.get_active(&live.resource_id).is_none());
        assert!(f.grants.grant_info(&grant).is_none());

        // Expired revision.
        let f = fixture();
        let mut short = draft("t1", RedactionStatus::Safe);
        short.ttl = Duration::from_millis(10);
        let live = f.lifecycle.publish(short).unwrap();
        let grant = mint(&f, &live);
        f.now.fetch_add(30, Ordering::SeqCst);
        let _ = no_bus(&f).republish(
            &live.resource_id,
            live.revision,
            draft("t1", RedactionStatus::Safe),
        );
        assert!(f.resources.get_active(&live.resource_id).is_none());
        assert!(f.grants.grant_info(&grant).is_none());

        // A revision that would succeed cannot be announced: fail closed.
        let f = fixture();
        let live = f
            .lifecycle
            .publish(draft("t1", RedactionStatus::Safe))
            .unwrap();
        let grant = mint(&f, &live);
        assert!(matches!(
            no_bus(&f).republish(
                &live.resource_id,
                live.revision,
                draft("t1", RedactionStatus::Safe)
            ),
            Err(LifecycleError::NotConfigured)
        ));
        assert!(f.resources.get_active(&live.resource_id).is_none());
        assert!(f.grants.grant_info(&grant).is_none());
    }

    #[test]
    fn safe_republish_shortening_expiry_revokes_but_extending_keeps() {
        let mut f = fixture();
        let first = f
            .lifecycle
            .publish(draft("t1", RedactionStatus::Safe))
            .unwrap();
        read_resource(&mut f);
        let grant = mint(&f, &first);

        // Longer (or equal) TTL keeps the grant.
        let mut longer = draft("t1", RedactionStatus::Safe);
        longer.ttl = Duration::from_secs(120);
        let second = f
            .lifecycle
            .republish(&first.resource_id, first.revision, longer)
            .unwrap();
        assert!(second.expires_at_ms > first.expires_at_ms);
        assert!(f.grants.grant_info(&grant).is_some());

        // Shorter TTL: same Safe redaction, but the old grant must go.
        let mut shorter = draft("t1", RedactionStatus::Safe);
        shorter.ttl = Duration::from_secs(10);
        let third = f
            .lifecycle
            .republish(&second.resource_id, second.revision, shorter)
            .unwrap();
        assert!(third.expires_at_ms < second.expires_at_ms);
        assert_eq!(third.redaction, RedactionStatus::Safe);
        assert!(f.grants.grant_info(&grant).is_none());
        assert!(f.resources.get_active(&third.resource_id).is_some());
        read_resource(&mut f);
        assert_eq!(read_resource(&mut f), third);
        assert_empty(&mut f);
    }

    #[test]
    fn publish_and_republish_to_pending_revoke_existing_grant() {
        for republish in [false, true] {
            let mut f = fixture();
            let first = f
                .lifecycle
                .publish(draft("t1", RedactionStatus::Safe))
                .unwrap();
            assert_eq!(read_resource(&mut f), first);
            let grant = mint(&f, &first);
            let pending = draft("t1", RedactionStatus::Pending);
            let second = if republish {
                f.lifecycle
                    .republish(&first.resource_id, first.revision, pending)
            } else {
                f.lifecycle.publish(pending)
            }
            .unwrap();
            assert_eq!(second.resource_id, first.resource_id);
            assert_eq!(second.redaction, RedactionStatus::Pending);
            assert!(f.grants.grant_info(&grant).is_none());
            assert_eq!(read_resource(&mut f), second);
            assert_empty(&mut f);
        }
    }

    #[test]
    fn safe_revision_keeps_existing_grant() {
        let mut f = fixture();
        let first = f
            .lifecycle
            .publish(draft("t1", RedactionStatus::Safe))
            .unwrap();
        let grant = mint(&f, &first);
        f.lifecycle
            .republish(
                &first.resource_id,
                first.revision,
                draft("t1", RedactionStatus::Safe),
            )
            .unwrap();
        assert!(f.grants.grant_info(&grant).is_some());
        read_resource(&mut f);
        read_resource(&mut f);
        assert_empty(&mut f);
    }

    #[test]
    fn refusal_closes_revokes_and_emits_exactly_once() {
        for republish in [false, true] {
            let mut f = fixture();
            let live = f
                .lifecycle
                .publish(draft("t1", RedactionStatus::Safe))
                .unwrap();
            read_resource(&mut f);
            let grant = mint(&f, &live);
            let refused = draft("t1", RedactionStatus::Refused);
            let error = if republish {
                f.lifecycle
                    .republish(&live.resource_id, live.revision, refused)
            } else {
                f.lifecycle.publish(refused)
            }
            .unwrap_err();
            assert!(matches!(
                error,
                LifecycleError::Registry(ResourceError::Refused { closed: Some(_) })
            ));
            let closed = read_closed(&mut f);
            assert_eq!(closed.resource_id, live.resource_id);
            assert_eq!(closed.reason, ClosedReason::Revoked);
            assert_eq!(f.resources.closed(&live.resource_id), Some(closed));
            assert!(f.grants.grant_info(&grant).is_none());
            assert_empty(&mut f);

            // A second refusal has nothing live to close: no duplicate event.
            let again = f
                .lifecycle
                .publish(draft("t1", RedactionStatus::Refused))
                .unwrap_err();
            assert!(matches!(
                again,
                LifecycleError::Registry(ResourceError::Refused { closed: None })
            ));
            assert_empty(&mut f);
        }
    }

    #[test]
    fn close_twice_revokes_and_emits_once() {
        let mut f = fixture();
        let live = f
            .lifecycle
            .publish(draft("t1", RedactionStatus::Safe))
            .unwrap();
        read_resource(&mut f);
        let grant = mint(&f, &live);

        let closed = f
            .lifecycle
            .close(&live.resource_id, ClosedReason::ServiceExited)
            .unwrap()
            .expect("transition");
        assert_eq!(closed.reason, ClosedReason::ServiceExited);
        assert!(f.grants.grant_info(&grant).is_none());
        assert_eq!(read_closed(&mut f), closed);

        // A grant minted after the close is NOT touched by a no-op second close
        // (it would not have been revoked-and-emitted again).
        assert!(f
            .lifecycle
            .close(&live.resource_id, ClosedReason::Revoked)
            .unwrap()
            .is_none());
        assert!(f
            .lifecycle
            .close("res_unknown", ClosedReason::Revoked)
            .unwrap()
            .is_none());
        assert_empty(&mut f);
        assert_eq!(f.resources.closed(&live.resource_id), Some(closed));
    }

    #[test]
    fn close_binding_revokes_and_emits_target_closed_once() {
        let mut f = fixture();
        let live = f
            .lifecycle
            .publish(draft("t1", RedactionStatus::Safe))
            .unwrap();
        let other = f
            .lifecycle
            .publish(draft("t2", RedactionStatus::Safe))
            .unwrap();
        read_resource(&mut f);
        read_resource(&mut f);
        let grant = mint(&f, &live);
        let other_grant = mint(&f, &other);

        let closed = f
            .lifecycle
            .close_binding(&binding("t1"), ClosedReason::TargetClosed)
            .unwrap()
            .expect("transition");
        assert_eq!(closed.resource_id, live.resource_id);
        assert!(f.grants.grant_info(&grant).is_none());
        assert!(f.grants.grant_info(&other_grant).is_some());
        assert_eq!(read_closed(&mut f), closed);
        assert!(f
            .lifecycle
            .close_binding(&binding("t1"), ClosedReason::TargetClosed)
            .unwrap()
            .is_none());
        assert_empty(&mut f);
    }

    fn draft_in(session: &str, target_id: &str) -> ResourceDraft {
        let mut d = draft(target_id, RedactionStatus::Safe);
        d.binding.target.browser_session_id = session.into();
        d
    }

    #[test]
    fn close_target_closes_only_that_exact_target_revokes_and_emits_once() {
        let mut f = fixture();
        let live = f.lifecycle.publish(draft_in("bs-1", "t1")).unwrap();
        let sibling = f.lifecycle.publish(draft_in("bs-1", "t2")).unwrap();
        let other_session = f.lifecycle.publish(draft_in("bs-2", "t1")).unwrap();
        for _ in 0..3 {
            read_resource(&mut f);
        }
        let grant = mint(&f, &live);
        let sibling_grant = mint(&f, &sibling);
        let other_grant = mint(&f, &other_session);

        let closed = f
            .lifecycle
            .close_target("default", "bs-1", "t1", ClosedReason::TargetClosed)
            .unwrap();
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].resource_id, live.resource_id);
        assert_eq!(closed[0].reason, ClosedReason::TargetClosed);
        assert!(f.grants.grant_info(&grant).is_none());
        assert!(f.grants.grant_info(&sibling_grant).is_some());
        assert!(f.grants.grant_info(&other_grant).is_some());
        assert_eq!(read_closed(&mut f), closed[0]);
        assert_empty(&mut f);
        // Second call: nothing live, nothing emitted.
        assert!(f
            .lifecycle
            .close_target("default", "bs-1", "t1", ClosedReason::TargetClosed)
            .unwrap()
            .is_empty());
        assert_empty(&mut f);
    }

    #[test]
    fn close_session_closes_whole_batch_revokes_and_isolates_sessions() {
        let mut f = fixture();
        let a = f.lifecycle.publish(draft_in("bs-1", "t1")).unwrap();
        let b = f.lifecycle.publish(draft_in("bs-1", "t2")).unwrap();
        let other = f.lifecycle.publish(draft_in("bs-2", "t1")).unwrap();
        for _ in 0..3 {
            read_resource(&mut f);
        }
        let (ga, gb, go) = (mint(&f, &a), mint(&f, &b), mint(&f, &other));

        let closed = f
            .lifecycle
            .close_session("default", "bs-1", ClosedReason::EngineExited)
            .unwrap();
        assert_eq!(closed.len(), 2);
        assert!(closed
            .iter()
            .all(|c| c.reason == ClosedReason::EngineExited));
        assert!(f.grants.grant_info(&ga).is_none());
        assert!(f.grants.grant_info(&gb).is_none());
        assert!(f.grants.grant_info(&go).is_some());
        let mut emitted = vec![read_closed(&mut f), read_closed(&mut f)];
        emitted.sort_by(|x, y| x.resource_id.cmp(&y.resource_id));
        let mut expected = closed.clone();
        expected.sort_by(|x, y| x.resource_id.cmp(&y.resource_id));
        assert_eq!(emitted, expected);
        assert_empty(&mut f);
        assert!(f.resources.get_active(&other.resource_id).is_some());
    }

    #[test]
    fn missing_bus_close_target_and_session_still_close_and_revoke() {
        let mut f = fixture();
        let a = f.lifecycle.publish(draft_in("bs-1", "t1")).unwrap();
        let b = f.lifecycle.publish(draft_in("bs-1", "t2")).unwrap();
        read_resource(&mut f);
        read_resource(&mut f);
        let (ga, gb) = (mint(&f, &a), mint(&f, &b));
        assert!(matches!(
            no_bus(&f).close_target("default", "bs-1", "t1", ClosedReason::TargetClosed),
            Err(LifecycleError::NotConfigured)
        ));
        assert!(f.resources.get_active(&a.resource_id).is_none());
        assert!(f.grants.grant_info(&ga).is_none());
        assert!(f.grants.grant_info(&gb).is_some());
        assert!(matches!(
            no_bus(&f).close_session("default", "bs-1", ClosedReason::EngineExited),
            Err(LifecycleError::NotConfigured)
        ));
        assert!(f.resources.get_active(&b.resource_id).is_none());
        assert!(f.grants.grant_info(&gb).is_none());
        assert_empty(&mut f);
    }

    #[test]
    fn terminal_sink_maps_target_destroyed_and_owner_ended_to_closes() {
        let mut f = fixture();
        let a = f.lifecycle.publish(draft_in("bs-1", "t1")).unwrap();
        let b = f.lifecycle.publish(draft_in("bs-1", "t2")).unwrap();
        read_resource(&mut f);
        read_resource(&mut f);
        let (ga, gb) = (mint(&f, &a), mint(&f, &b));
        let sink = f.lifecycle.terminal_sink();

        sink.target_destroyed("default", "bs-1", "t1");
        let closed = read_closed(&mut f);
        assert_eq!(closed.resource_id, a.resource_id);
        assert_eq!(closed.reason, ClosedReason::TargetClosed);
        assert!(f.grants.grant_info(&ga).is_none());
        assert!(f.grants.grant_info(&gb).is_some());

        sink.owner_ended("default", "bs-1", ClosedReason::EngineExited);
        let closed = read_closed(&mut f);
        assert_eq!(closed.resource_id, b.resource_id);
        assert_eq!(closed.reason, ClosedReason::EngineExited);
        assert!(f.grants.grant_info(&gb).is_none());
        assert_empty(&mut f);
    }

    #[test]
    fn expiry_sweep_revokes_and_emits_once_and_expired_revision_sweeps() {
        for republish in [false, true] {
            let mut f = fixture();
            let mut short = draft("t1", RedactionStatus::Safe);
            short.ttl = Duration::from_millis(10);
            let expiring = f.lifecycle.publish(short).unwrap();
            let long = f
                .lifecycle
                .publish(draft("t2", RedactionStatus::Safe))
                .unwrap();
            read_resource(&mut f);
            read_resource(&mut f);
            // Directly minted so the grant's own TTL cannot be what ends it.
            let grant = mint(&f, &expiring);
            let long_grant = mint(&f, &long);
            f.now.fetch_add(30, Ordering::SeqCst);

            if republish {
                let error = f
                    .lifecycle
                    .republish(
                        &expiring.resource_id,
                        expiring.revision,
                        draft("t1", RedactionStatus::Safe),
                    )
                    .unwrap_err();
                assert!(matches!(
                    error,
                    LifecycleError::Registry(ResourceError::Expired)
                ));
            } else {
                assert_eq!(f.lifecycle.sweep_expired().unwrap(), 1);
            }
            let closed = read_closed(&mut f);
            assert_eq!(closed.resource_id, expiring.resource_id);
            assert_eq!(closed.reason, ClosedReason::Expired);
            assert!(f.grants.grant_info(&grant).is_none());
            assert!(f.grants.grant_info(&long_grant).is_some());
            assert_eq!(f.lifecycle.sweep_expired().unwrap(), 0);
            assert_empty(&mut f);
        }
    }
}
