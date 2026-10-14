//! Composition root between the r4 viewer-grant flow and the browser's
//! live-target machinery.
//!
//! The gateway viewer route is pure I/O (R4): it validates credentials
//! against the grant store and moves bytes. Everything that touches CDP —
//! attaching the screencast, binding the control plane, subscribing frames,
//! dispatching human input — goes through this trait, whose production
//! implementation owns an [`super::LiveProfileOwner`]. The trait boundary is what
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
    BrowserLiveSnapshot, HumanInputEvent, PreviewResource, RedactionStatus, ResourceBinding,
    ViewerBinding, ViewerPermission, ViewportCssPx,
};

use super::control::{InterventionSink, TargetControl};
use super::grant::ExchangedGrant;
use super::owner_registry::{LiveOwnerRegistry, OwnerRouteLease};
use super::resource::ResourceRegistry;
use super::stream_hub::LatestFrameReceiver;
use super::AttachmentLease;
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
    pub input_route: ViewerInputRoute,
    pub resource_id: String,
    pub permissions: Vec<ViewerPermission>,
    pub expires_at_ms: i64,
    /// This viewer's reference on the shared target attachment. Dropping the
    /// prepared viewer (parked-ticket expiry, auth failure, WS end) gives it
    /// back; the last one out releases the screencast and the controls.
    /// `None` only for backends with no live engine (route test fakes).
    pub attachment: Option<AttachmentLease>,
}

/// Prepared internal input authority. Wire bindings cannot construct one.
#[derive(Clone)]
pub(crate) struct ViewerInputRoute {
    lease: Option<OwnerRouteLease>,
    #[cfg(test)]
    target_id: String,
}

impl ViewerInputRoute {
    fn exact(lease: OwnerRouteLease) -> Self {
        Self {
            #[cfg(test)]
            target_id: lease.binding().target_id,
            lease: Some(lease),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(target_id: &str) -> Self {
        Self {
            target_id: target_id.into(),
            lease: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn target_id(&self) -> &str {
        &self.target_id
    }
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
        route: &'a ViewerInputRoute,
        event: HumanInputEvent,
    ) -> BoxFuture<'a, Result<(), BrowserError>>;
}

/// Production backend over a [`super::LiveProfileOwner`].
///
/// Constructed by `gateway::server::browser_live_runtime` when an explicit,
/// valid `[gateway] browser_viewer_origin` installs the browser-live runtime.
pub(crate) struct LiveViewerBackend {
    owners: LiveOwnerRegistry,
    sink: InterventionSink,
    viewport: ViewportCssPx,
    navigation_guard: BrowserSsrfGuard,
    /// The single authority for `resource_id -> PreviewResource`. The viewer
    /// snapshot embeds the registry's record verbatim; this backend never
    /// synthesizes a resource of its own.
    resources: ResourceRegistry,
    /// The owner binds ONE control plane per target for its lifetime, so a
    /// second grant on the same target must reuse it (same resource and
    /// binding only) instead of re-binding — and must never tear the first
    /// viewer's control down on failure.
    controls: tokio::sync::Mutex<std::collections::HashMap<(ControlKey, u64), Arc<TargetControl>>>,
}

/// Exact `(browser_session_id, target_id)` identity of a control plane: the
/// same target id under a different browser session is a different target.
type ControlKey = (String, String);

fn control_key(binding: &ViewerBinding) -> ControlKey {
    (
        binding.target.browser_session_id.clone(),
        binding.target.target_id.clone(),
    )
}

impl LiveViewerBackend {
    pub(crate) fn new(
        owners: LiveOwnerRegistry,
        sink: InterventionSink,
        viewport: ViewportCssPx,
        navigation_guard: BrowserSsrfGuard,
        resources: ResourceRegistry,
    ) -> Self {
        Self {
            owners,
            sink,
            viewport,
            navigation_guard,
            resources,
            controls: Default::default(),
        }
    }

    /// The registry's active resource for this grant, or an error. Fail
    /// closed: unknown / closed / expired / foreign-principal ids all look
    /// the same (`get_active_for`), the resource must be bound to exactly the
    /// grant's viewer binding, and the grant may not outlive it.
    fn authoritative_resource(
        &self,
        grant: &ExchangedGrant,
    ) -> Result<PreviewResource, BrowserError> {
        let deny = |why: &str| BrowserError::ActionFailed(format!("viewer resource denied: {why}"));
        let resource = self
            .resources
            .get_active_for(&grant.resource_id, &grant.binding.principal)
            .ok_or_else(|| deny("resource is not active for this principal"))?;
        let expected = ResourceBinding::Browser {
            viewer: grant.binding.clone(),
        };
        if resource.binding != expected {
            return Err(deny("resource binding does not match the grant"));
        }
        if grant.expires_at_ms > resource.expires_at_ms {
            return Err(deny("grant outlives its resource"));
        }
        // Fail closed on redaction: Pending (undecided) and Refused alike.
        if resource.redaction != RedactionStatus::Safe {
            return Err(deny("resource is not viewable"));
        }
        Ok(resource)
    }

    /// The control plane for this grant's target: the one already bound for
    /// the identical resource/binding, or a fresh bind. Anything else is a
    /// conflict. Never closes the target (that would end other viewers).
    #[cfg(test)]
    async fn control_for(
        &self,
        resource_id: &str,
        binding: &ViewerBinding,
    ) -> Result<Arc<TargetControl>, BrowserError> {
        let route = self.owners.lookup_exact(&binding.target)?;
        self.control_for_route(&route, resource_id, binding).await
    }

    /// Post-attach half of `prepare`: subscribe, bind control, build the
    /// snapshot. Fallible; the caller owns unwinding the attach on `Err`.
    async fn bind_attached(
        &self,
        grant: &ExchangedGrant,
        route: &OwnerRouteLease,
        resource: PreviewResource,
    ) -> Result<PreparedViewer, BrowserError> {
        route.validate()?;
        // Subscribe before binding: dropping the receiver detaches only this
        // viewer.
        let frames = route
            .owner()
            .subscribe_latest(&grant.binding.target.target_id)?;
        // The approval this prepare started from must still be the live one.
        // `subscribe_latest` pinned the receiver to the target's content
        // generation, and an owner declares a document boundary by demoting
        // the approval FIRST and advancing the generation second: if this
        // re-check passes, any boundary that follows also advances past the
        // pinned generation and fences the receiver; if a boundary already
        // demoted it, this fails and no frame is ever handed out.
        let still = self.authoritative_resource(grant)?;
        if still.resource_id != resource.resource_id || still.revision != resource.revision {
            return Err(BrowserError::ActionFailed(
                "viewer resource denied: approval changed while the viewer was attaching".into(),
            ));
        }
        let control = self
            .control_for_route(route, &grant.resource_id, &grant.binding)
            .await?;
        route.validate()?;
        let live = control.snapshot();
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
            input_route: ViewerInputRoute::exact(route.clone()),
            resource_id: grant.resource_id.clone(),
            permissions: grant.permissions.clone(),
            expires_at_ms: grant.expires_at_ms,
            attachment: None,
        })
    }

    async fn control_for_route(
        &self,
        route: &OwnerRouteLease,
        resource_id: &str,
        binding: &ViewerBinding,
    ) -> Result<Arc<TargetControl>, BrowserError> {
        self.owners.accepts(route)?;
        if route.binding() != binding.target {
            return Err(BrowserError::ActionFailed(
                "control binding differs from exact owner lease".into(),
            ));
        }
        let key = (control_key(binding), route.epoch());
        let mut controls = self.controls.lock().await;
        route.validate()?;
        controls.retain(|(target, epoch), _| target != &key.0 || *epoch == key.1);
        if let Some(existing) = controls.get(&key) {
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
            controls.remove(&key);
        }
        let control = route
            .owner()
            .bind_control(
                resource_id,
                binding.clone(),
                self.viewport.clone(),
                self.sink.clone(),
            )
            .await?;
        controls.insert(key, control.clone());
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
            // The registry is the only source of the resource record; resolve
            // it (and its binding/expiry agreement with the grant) before any
            // attach or bind so a rejected grant has no engine side effects.
            let resource = self.authoritative_resource(grant)?;
            let route = self.owners.lookup_exact(&target)?;
            // Every prepare takes its own reference on the shared attachment,
            // so a failure or cancellation here (the lease drops with the
            // future) gives back exactly what THIS call took and can never
            // end another viewer's stream or control.
            let lease = route.owner().acquire_attachment(target).await?;
            match self.bind_attached(grant, &route, resource).await {
                Ok(mut prepared) => {
                    prepared.attachment = Some(lease);
                    Ok(prepared)
                }
                Err(error) => {
                    lease.release().await;
                    Err(error)
                }
            }
        })
    }

    fn dispatch_input<'a>(
        &'a self,
        route: &'a ViewerInputRoute,
        event: HumanInputEvent,
    ) -> BoxFuture<'a, Result<(), BrowserError>> {
        Box::pin(async move {
            let lease = route.lease.as_ref().ok_or_else(|| {
                BrowserError::ActionFailed("viewer input requires an exact owner lease".into())
            })?;
            self.owners.accepts(lease)?;
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
            self.owners.accepts(lease)?;
            lease
                .owner()
                .dispatch_routed_human_input(lease, event)
                .await
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use aleph_protocol::browser_live::{
        ClosedReason, ConversationBinding, DisplayPolicy, IssuerIdentity, PreviewKind,
        PreviewProvenance, PreviewSource, RedactionStatus, ViewerOrigin, ViewerSecret,
    };

    use super::super::resource::ResourceDraft;
    use super::super::tests::fixture;
    use super::super::LiveProfileOwner;
    use super::*;

    fn owner_registry(
        owner: &Arc<LiveProfileOwner>,
        handle: &Arc<crate::browser::engine::EngineHandle>,
    ) -> LiveOwnerRegistry {
        use crate::browser::cdp_backend::{BrowserOperationKind, BrowserOperationRecord};
        let registry = LiveOwnerRegistry::default();
        for target in ["T1", "T2", "T3"] {
            registry
                .register_operation(&BrowserOperationRecord::capture_for_test(
                    owner,
                    handle,
                    target.into(),
                    BrowserOperationKind::Open,
                ))
                .unwrap();
        }
        registry
    }

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
            owner_registry(&owner, &_handle),
            Arc::new(|_| Ok(())),
            ViewportCssPx {
                width: 800,
                height: 600,
            },
            BrowserSsrfGuard::default(),
            ResourceRegistry::default(),
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

    #[test]
    fn control_key_is_the_exact_session_and_target_pair() {
        let mut a = super::super::tests::binding("T1");
        a.browser_session_id = "session-a".into();
        let mut b = a.clone();
        b.browser_session_id = "session-b".into();
        let (ka, kb) = (
            control_key(&viewer_binding(a.clone())),
            control_key(&viewer_binding(b)),
        );
        assert_eq!(ka, ("session-a".to_string(), "T1".to_string()));
        assert_ne!(ka, kb, "same target id under another session is distinct");
        assert_eq!(ka, control_key(&viewer_binding(a)));
    }

    #[tokio::test]
    async fn foreign_session_same_target_id_never_reuses_or_evicts_the_owners_control() {
        let (_server, owner, _handle) = fixture().await;
        let backend = LiveViewerBackend::new(
            owner_registry(&owner, &_handle),
            Arc::new(|_| Ok(())),
            ViewportCssPx {
                width: 800,
                height: 600,
            },
            BrowserSsrfGuard::default(),
            ResourceRegistry::default(),
        );
        let mut target = super::super::tests::binding("T1");
        target.browser_session_id = owner.browser_session_id().into();
        let binding = viewer_binding(target.clone());
        owner.attach_target(target.clone()).await.unwrap();
        let first = backend.control_for("res", &binding).await.unwrap();

        let mut foreign_target = target;
        foreign_target.browser_session_id = "foreign-session".into();
        let foreign = viewer_binding(foreign_target);
        assert!(
            backend.control_for("res", &foreign).await.is_err(),
            "a foreign session must not get the owner's control for the same target id"
        );

        let again = backend.control_for("res", &binding).await.unwrap();
        assert!(Arc::ptr_eq(&first, &again), "owner's entry is untouched");
    }

    #[tokio::test]
    async fn no_exact_registration_or_token_means_no_attach_or_dispatch() {
        let (server, owner, _handle) = fixture().await;
        let resources = ResourceRegistry::default();
        let backend = LiveViewerBackend::new(
            LiveOwnerRegistry::default(),
            Arc::new(|_| Ok(())),
            ViewportCssPx {
                width: 800,
                height: 600,
            },
            BrowserSsrfGuard::default(),
            resources.clone(),
        );
        let mut target = super::super::tests::binding("T1");
        target.browser_session_id = owner.browser_session_id().into();
        let binding = viewer_binding(target);
        let resource = resources.publish(draft(binding.clone())).unwrap();
        assert!(backend
            .prepare(&grant_for(binding, &resource))
            .await
            .is_err());
        let route = ViewerInputRoute::for_test("T1");
        assert!(backend
            .dispatch_input(&route, HumanInputEvent::Text { text: "x".into() })
            .await
            .is_err());
        assert!(server.received_for("Page.startScreencast").is_empty());
        assert!(server.received_for("Input.insertText").is_empty());
    }

    #[tokio::test]
    async fn prepared_exact_routes_dispatch_only_to_their_owner_and_retire_fail_closed() {
        use aleph_cdp::testkit::Responder;
        use serde_json::json;
        let (sa, oa, ha) = fixture().await;
        let (sb, ob, hb) = fixture().await;
        sa.on("Input.insertText", Responder::Reply(json!({})));
        sb.on("Input.insertText", Responder::Reply(json!({})));
        let owners = owner_registry(&oa, &ha);
        for target in ["T1", "T2"] {
            owners
                .register_operation(
                    &crate::browser::cdp_backend::BrowserOperationRecord::capture_for_test(
                        &ob,
                        &hb,
                        target.into(),
                        crate::browser::cdp_backend::BrowserOperationKind::Open,
                    ),
                )
                .unwrap();
        }
        let resources = ResourceRegistry::default();
        let backend = LiveViewerBackend::new(
            owners.clone(),
            Arc::new(|_| Ok(())),
            ViewportCssPx {
                width: 800,
                height: 600,
            },
            BrowserSsrfGuard::default(),
            resources.clone(),
        );
        let mut routes = Vec::new();
        for owner in [&oa, &ob] {
            let mut target = super::super::tests::binding("T1");
            target.browser_session_id = owner.browser_session_id().into();
            let binding = viewer_binding(target);
            let resource = resources.publish(draft(binding.clone())).unwrap();
            routes.push(
                backend
                    .prepare(&grant_for(binding, &resource))
                    .await
                    .unwrap()
                    .input_route,
            );
        }
        backend
            .dispatch_input(&routes[0], HumanInputEvent::Text { text: "a".into() })
            .await
            .unwrap();
        assert_eq!(sa.received_for("Input.insertText").len(), 1);
        assert!(sb.received_for("Input.insertText").is_empty());
        assert!(owners.retire_owner(routes[0].lease.as_ref().unwrap()));
        assert!(backend
            .dispatch_input(
                &routes[0],
                HumanInputEvent::Text {
                    text: "stale".into()
                }
            )
            .await
            .is_err());
        backend
            .dispatch_input(&routes[1], HumanInputEvent::Text { text: "b".into() })
            .await
            .unwrap();
        assert_eq!(sa.received_for("Input.insertText").len(), 1);
        assert_eq!(sb.received_for("Input.insertText").len(), 1);
        assert_eq!(sa.connection_count(), 1);
        assert_eq!(sb.connection_count(), 1);
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

    fn grant_for(binding: ViewerBinding, resource: &PreviewResource) -> ExchangedGrant {
        ExchangedGrant {
            grant_id: "g".into(),
            viewer_session_id: "vs".into(),
            rotation_generation: 0,
            current_secret: ViewerSecret("s".into()),
            binding,
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

    #[tokio::test]
    async fn prepare_uses_the_registry_resource_and_rejects_bad_resources_before_attach() {
        let (_server, owner, _handle) = fixture().await;
        let registry = ResourceRegistry::default();
        let backend = LiveViewerBackend::new(
            owner_registry(&owner, &_handle),
            Arc::new(|_| Ok(())),
            ViewportCssPx {
                width: 800,
                height: 600,
            },
            BrowserSsrfGuard::default(),
            registry.clone(),
        );
        let mut target = super::super::tests::binding("T1");
        target.browser_session_id = owner.browser_session_id().into();
        let binding = viewer_binding(target);
        let published = registry.publish(draft(binding.clone())).unwrap();
        let good = grant_for(binding.clone(), &published);
        // Nothing below may leave the target attached (no engine side effects).
        let not_attached = || owner.subscribe_latest("T1").is_err();

        // Unknown resource id.
        let mut unknown = good.clone();
        unknown.resource_id = "res_unknown".into();
        assert!(backend.prepare(&unknown).await.is_err());
        assert!(not_attached(), "unknown resource must not attach");

        // Resource bound to a different binding than the grant.
        let mut other = binding.clone();
        other.task.run_id = "other-run".into();
        let mismatched = grant_for(other, &published);
        assert!(backend.prepare(&mismatched).await.is_err());
        assert!(not_attached(), "mismatched binding must not attach");

        // Same binding, different principal: indistinguishable from unknown.
        let mut stranger = binding.clone();
        stranger.principal = IssuerIdentity::User {
            user_id: "stranger".into(),
        };
        assert!(backend
            .prepare(&grant_for(stranger, &published))
            .await
            .is_err());
        assert!(not_attached(), "foreign principal must not attach");

        // Grant outliving its resource.
        let mut long = good.clone();
        long.expires_at_ms = published.expires_at_ms + 1;
        assert!(backend.prepare(&long).await.is_err());
        assert!(not_attached(), "grant past resource expiry must not attach");

        // Closed resource.
        let closed_binding = viewer_binding({
            let mut t = super::super::tests::binding("T2");
            t.browser_session_id = owner.browser_session_id().into();
            t
        });
        let to_close = registry.publish(draft(closed_binding.clone())).unwrap();
        registry
            .close(&to_close.resource_id, ClosedReason::TargetClosed)
            .unwrap();
        assert!(backend
            .prepare(&grant_for(closed_binding, &to_close))
            .await
            .is_err());
        assert!(
            owner.subscribe_latest("T2").is_err(),
            "closed must not attach"
        );

        // Pending redaction: not viewable, no engine side effects, and the
        // error does not echo the resource id.
        let pending_binding = viewer_binding({
            let mut t = super::super::tests::binding("T3");
            t.browser_session_id = owner.browser_session_id().into();
            t
        });
        let mut pending_draft = draft(pending_binding.clone());
        pending_draft.redaction = RedactionStatus::Pending;
        let pending = registry.publish(pending_draft).unwrap();
        let error = backend
            .prepare(&grant_for(pending_binding, &pending))
            .await
            .err()
            .expect("pending resource must be denied");
        assert!(!error.to_string().contains(&pending.resource_id));
        assert!(
            owner.subscribe_latest("T3").is_err(),
            "pending must not attach"
        );

        // The honest grant embeds the registry record verbatim.
        let prepared = backend.prepare(&good).await.expect("prepare");
        assert_eq!(prepared.snapshot.resource, published);
        assert_eq!(prepared.resource_id, published.resource_id);
    }

    #[tokio::test]
    async fn prepare_failure_after_attach_closes_only_the_screencast_it_started() {
        let (server, owner, handle) = fixture().await;
        let registry = ResourceRegistry::default();
        // Zero viewport: attach succeeds, then bind_control fails.
        let backend = LiveViewerBackend::new(
            owner_registry(&owner, &handle),
            Arc::new(|_| Ok(())),
            ViewportCssPx {
                width: 0,
                height: 0,
            },
            BrowserSsrfGuard::default(),
            registry.clone(),
        );
        let mk = |id: &str| {
            let mut t = super::super::tests::binding(id);
            t.browser_session_id = owner.browser_session_id().into();
            viewer_binding(t)
        };
        let b1 = mk("T1");
        let r1 = registry.publish(draft(b1.clone())).unwrap();
        assert!(backend.prepare(&grant_for(b1, &r1)).await.is_err());
        assert!(
            owner.subscribe_latest("T1").is_err(),
            "a failed prepare must not leave its screencast attached"
        );
        assert!(!server.received_for("Page.startScreencast").is_empty());

        // A target another viewer already attached survives our failure.
        let b2 = mk("T2");
        let r2 = registry.publish(draft(b2.clone())).unwrap();
        owner.attach_target(b2.target.clone()).await.unwrap();
        assert!(backend.prepare(&grant_for(b2, &r2)).await.is_err());
        assert!(
            owner.subscribe_latest("T2").is_ok(),
            "must not tear down another viewer's attach"
        );
    }
}
