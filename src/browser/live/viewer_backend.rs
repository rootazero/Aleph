//! Composition root between the r4 viewer-grant flow and the browser's
//! live-target machinery.
//!
//! The gateway viewer route is pure I/O (R4): it validates credentials
//! against the grant store and moves bytes. Everything that touches CDP —
//! attaching the screencast, binding the control plane, subscribing frames,
//! dispatching human input — goes through this trait, whose production
//! implementation owns an [`LiveProfileOwner`]. The trait boundary is what
//! lets the route's auth/state machine be tested without a browser.
//!
//! `engine_ack_id` never crosses this boundary: [`PreparedViewer`] carries
//! the typed frame receiver, and only the route's frame-encode step reads
//! `LiveFrame` fields (it reads `epoch`, `seq`, `data`, `timestamp` — the
//! ack id stays `pub(crate)` inside the browser module).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use aleph_protocol::browser_live::{
    BrowserLiveSnapshot, DisplayPolicy, HumanInputEvent, PreviewKind, PreviewProvenance,
    PreviewResource, PreviewSource, RedactionStatus, ResourceBinding, ViewerBinding,
    ViewerPermission, ViewportCssPx,
};

use super::control::{InterventionSink, TargetControl};
use super::grant::ExchangedGrant;
use super::stream_hub::LatestFrameReceiver;
use super::LiveProfileOwner;
use crate::browser::error::BrowserError;
use crate::browser::network_policy::BrowserSsrfGuard;

pub(crate) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Everything the WS loop needs once a ticket is exchanged: the initial
/// snapshot, the bound control plane, and the typed frame stream.
pub(crate) struct PreparedViewer {
    pub snapshot: BrowserLiveSnapshot,
    pub control: Arc<TargetControl>,
    pub frames: LatestFrameReceiver,
    pub binding: ViewerBinding,
    pub resource_id: String,
    pub permissions: Vec<ViewerPermission>,
    pub expires_at_ms: i64,
}

/// The browser-side half of the viewer route. Implemented by
/// [`LiveViewerBackend`] in production and by fakes in route tests.
pub(crate) trait ViewerBackend: Send + Sync {
    /// Attach the target's screencast, bind its control plane, and build the
    /// initial snapshot. Called once per ticket exchange; the result is
    /// parked until the AuthOk write commits.
    fn prepare<'a>(
        &'a self,
        grant: &'a ExchangedGrant,
    ) -> BoxFuture<'a, Result<PreparedViewer, BrowserError>>;

    /// Dispatch one admitted human input event. Lease and permission
    /// validation happen in `TargetControl::queue_input`/`dispatch` before
    /// this is called; this is the mechanical mapping to CDP only.
    fn dispatch_input<'a>(
        &'a self,
        target_id: &'a str,
        event: HumanInputEvent,
    ) -> BoxFuture<'a, Result<(), BrowserError>>;
}

/// Production backend over a [`LiveProfileOwner`].
///
/// Constructed by the boot wiring that owns the browser engine handle
/// (a later task); until then nothing in production builds one, so the
/// struct keeps its dead-code allowance.
#[allow(dead_code)]
pub(crate) struct LiveViewerBackend {
    owner: Arc<LiveProfileOwner>,
    sink: InterventionSink,
    viewport: ViewportCssPx,
    navigation_guard: BrowserSsrfGuard,
    /// The owner binds ONE control plane per target for its lifetime, so a
    /// second grant on the same target must reuse it (same resource and
    /// binding only) instead of re-binding — and must never tear the first
    /// viewer's control down on failure.
    controls: tokio::sync::Mutex<std::collections::HashMap<String, Arc<TargetControl>>>,
}

#[allow(dead_code)] // constructed by the Task 6 boot wiring
impl LiveViewerBackend {
    pub(crate) fn new(
        owner: Arc<LiveProfileOwner>,
        sink: InterventionSink,
        viewport: ViewportCssPx,
        navigation_guard: BrowserSsrfGuard,
    ) -> Self {
        Self {
            owner,
            sink,
            viewport,
            navigation_guard,
            controls: Default::default(),
        }
    }

    /// The control plane for this grant's target: the one already bound for
    /// the identical resource/binding, or a fresh bind. Anything else is a
    /// conflict. Never closes the target (that would end other viewers).
    async fn control_for(
        &self,
        resource_id: &str,
        binding: &ViewerBinding,
    ) -> Result<Arc<TargetControl>, BrowserError> {
        let target_id = binding.target.target_id.clone();
        let mut controls = self.controls.lock().await;
        if let Some(existing) = controls.get(&target_id) {
            if existing.is_live() {
                let live = existing.snapshot();
                if live.resource_id == resource_id && live.binding == *binding {
                    return Ok(existing.clone());
                }
                return Err(BrowserError::ActionFailed(
                    "target already has a control binding for a different viewer resource".into(),
                ));
            }
            // Dead tombstone (target closed / re-attached): drop it and bind
            // fresh. The owner's `bind_control` replaces dead tombstones and
            // still refuses a live duplicate.
            controls.remove(&target_id);
        }
        let control = self
            .owner
            .bind_control(
                resource_id,
                binding.clone(),
                self.viewport.clone(),
                self.sink.clone(),
            )
            .await?;
        controls.insert(target_id, control.clone());
        Ok(control)
    }
}

impl ViewerBackend for LiveViewerBackend {
    fn prepare<'a>(
        &'a self,
        grant: &'a ExchangedGrant,
    ) -> BoxFuture<'a, Result<PreparedViewer, BrowserError>> {
        Box::pin(async move {
            let target = grant.binding.target.clone();
            // Refuse a foreign binding BEFORE touching the engine, so a
            // rejected grant cannot leave a screencast attached.
            if target.browser_session_id != self.owner.browser_session_id() {
                return Err(BrowserError::ActionFailed(
                    "viewer binding does not name this browser session".into(),
                ));
            }
            let target_id = target.target_id.clone();
            self.owner.attach_target(target).await?;
            // Subscribe before binding: a failure here has nothing to unwind,
            // and dropping the receiver detaches only this viewer.
            let frames = self.owner.subscribe_latest(&target_id)?;
            let control = self.control_for(&grant.resource_id, &grant.binding).await?;
            let live = control.snapshot();
            // The Task 6 producer owns resource publishing; the viewer
            // snapshot constructs the minimal honest record for its own
            // resource id. Redaction is `Pending` because no redaction pass
            // has run over this stream — the route streams frames regardless,
            // and a UI that gates on `Safe` refuses, which is the safe
            // direction.
            let resource = PreviewResource {
                resource_id: grant.resource_id.clone(),
                revision: 0,
                binding: ResourceBinding::Browser {
                    viewer: grant.binding.clone(),
                },
                kind: PreviewKind::Page,
                display: DisplayPolicy::ExplicitOnly,
                source: PreviewSource::BrowserTool,
                provenance: PreviewProvenance::NotManaged,
                expires_at_ms: grant.expires_at_ms,
                redaction: RedactionStatus::Pending,
                companion_candidates: Vec::new(),
            };
            let snapshot = BrowserLiveSnapshot {
                resource,
                connection_epoch: live.connection_epoch,
                stream_epoch: live.stream_epoch,
                last_frame_seq: 0,
                viewport_css_px: live.viewport_css_px,
                controller: live.controller.clone(),
                control_epoch: live.control_epoch,
            };
            Ok(PreparedViewer {
                snapshot,
                control,
                frames,
                binding: grant.binding.clone(),
                resource_id: grant.resource_id.clone(),
                permissions: grant.permissions.clone(),
                expires_at_ms: grant.expires_at_ms,
            })
        })
    }

    fn dispatch_input<'a>(
        &'a self,
        target_id: &'a str,
        event: HumanInputEvent,
    ) -> BoxFuture<'a, Result<(), BrowserError>> {
        Box::pin(async move {
            // Human navigation goes through the SAME global SSRF baseline as
            // agent navigation (the context-grant axis of the network
            // amendment is gated on the Task 0 evidence and is NOT here).
            if let HumanInputEvent::Navigate { url } = &event {
                self.navigation_guard
                    .check_navigation(url)
                    .await
                    .map_err(|v| {
                        BrowserError::ActionFailed(format!("viewer navigation denied: {v}"))
                    })?;
            }
            self.owner.dispatch_human_input(target_id, event).await
        })
    }
}

#[cfg(test)]
mod tests {
    use aleph_protocol::browser_live::{ConversationBinding, IssuerIdentity};

    use super::super::tests::fixture;
    use super::*;

    fn viewer_binding(target: aleph_protocol::browser_live::BrowserTargetBinding) -> ViewerBinding {
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

    #[tokio::test]
    async fn control_for_rebinds_after_a_dead_map_entry_but_reuses_a_live_one() {
        let (_server, owner, _handle) = fixture().await;
        let backend = LiveViewerBackend::new(
            owner.clone(),
            Arc::new(|_| Ok(())),
            ViewportCssPx {
                width: 800,
                height: 600,
            },
            BrowserSsrfGuard::default(),
        );
        let mut target = super::super::tests::binding("T1");
        target.browser_session_id = owner.browser_session_id().into();
        let binding = viewer_binding(target.clone());

        owner.attach_target(target.clone()).await.unwrap();
        let first = backend.control_for("res", &binding).await.unwrap();
        let again = backend.control_for("res", &binding).await.unwrap();
        assert!(
            Arc::ptr_eq(&first, &again),
            "live identical grant is reused"
        );
        assert!(
            backend.control_for("other", &binding).await.is_err(),
            "a live control for a different resource is a conflict"
        );

        // close_target tombstones the control; a re-attach must get a fresh one.
        owner.close_target("T1").await.unwrap();
        assert!(!first.is_live());
        owner.attach_target(target).await.unwrap();
        let second = backend.control_for("res", &binding).await.unwrap();
        assert!(second.is_live());
        assert!(!Arc::ptr_eq(&first, &second), "dead tombstone is replaced");
    }
}
