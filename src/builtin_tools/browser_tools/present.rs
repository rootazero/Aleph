//! Explicit browser-live publication, not URL discovery or a viewer transport.
//!
//! Only an owner holding a real operation/launcher record may construct
//! `BrowserPresentOperation`; model arguments must never construct one.
//! `CdpBackend::{open_tab,navigate,history}_with_record` return such a record
//! (exact live owner + session + target, captured inside the mutation scope);
//! `operation_publish` turns it into a best-effort publication for
//! `browser_open` / `browser_navigate`. Boot installs a producer only when the
//! gateway runtime is fully composed (explicit `browser_viewer_origin`); otherwise
//! `browser_live_producer` stays unset and tools publish nothing.

#![allow(dead_code)] // Awaiting an operation-bound backend/launcher record provider.

use std::sync::Arc;
use std::time::Duration;

use aleph_protocol::browser_live::{
    BrowserLiveClosed, BrowserTargetBinding, ClosedReason, CompanionCandidate, ConversationBinding,
    DisplayPolicy, IssuerIdentity, PreviewKind, PreviewProvenance, PreviewResource, PreviewSource,
    RedactionStatus, ViewerBinding,
};

use crate::browser::live::grant::ViewerGrantStore;
use crate::browser::live::lifecycle::{BrowserLiveLifecycle, LifecycleError};
use crate::browser::live::owner_registry::{LiveOwnerRegistry, OwnerRouteLease};
use crate::browser::live::resource::{ResourceDraft, ResourceError, ResourceRegistry};
use crate::browser::live::LiveProfileOwner;
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::visibility::{owner_or_legacy, run_principal_sans_caller, RunPrincipal};
use crate::tools::turn_context::current_turn_context;

/// Internal, non-deserializable attestation from an actual operation/launch.
/// There is deliberately no URL, active-tab lookup, or caller/task argument.
#[derive(Clone, Debug)]
pub(crate) struct BrowserPresentOperation {
    target: BrowserTargetBinding,
    // Internal authority only: never deserialized from a tool/RPC request.
    operation_record: Option<crate::browser::cdp_backend::BrowserOperationRecord>,
    source: PreviewSource,
    provenance: PreviewProvenance,
}

impl BrowserPresentOperation {
    /// The backend owner must supply the target returned by the real operation.
    pub(crate) fn from_browser_operation(target: BrowserTargetBinding) -> Self {
        Self {
            target,
            operation_record: None,
            source: PreviewSource::BrowserTool,
            provenance: PreviewProvenance::NotManaged,
        }
    }

    /// Attestation from a successful CDP mutation: the target is the record's
    /// own owner/session/target, never re-resolved after the operation.
    pub(crate) fn from_operation_record(
        record: &crate::browser::cdp_backend::BrowserOperationRecord,
    ) -> Self {
        let mut operation = Self::from_browser_operation(record.target_binding());
        operation.operation_record = Some(record.clone());
        operation
    }

    pub(crate) fn operation_record(
        &self,
    ) -> Option<&crate::browser::cdp_backend::BrowserOperationRecord> {
        self.operation_record.as_ref()
    }

    /// Production managed attestation: the target is ONLY the real operation
    /// record's own binding (exact owner/session/target from `open_tab_with_record`),
    /// the provenance ONLY the internal [`ManagedLaunchRecord`] (driver-reported
    /// service PID, Gateway clock, verified endpoint proof). Nothing here can be
    /// built from tool arguments, a URL, a PID guess or the current tab.
    pub(crate) fn from_managed_operation(
        record: &crate::browser::cdp_backend::BrowserOperationRecord,
        launch: &super::companion::ManagedLaunchRecord,
    ) -> Result<Self, PresentError> {
        if launch.service_pid() == 0 || launch.launched_at_ms() <= 0 {
            return Err(PresentError::MissingTarget);
        }
        Ok(Self {
            target: record.target_binding(),
            operation_record: Some(record.clone()),
            source: PreviewSource::ManagedTool,
            provenance: PreviewProvenance::Managed {
                launcher: super::companion::LAUNCHER.to_owned(),
                process_id: launch.service_pid(),
                launched_at_ms: launch.launched_at_ms(),
            },
        })
    }

    /// Test fixture only: a launcher record WITHOUT an operation record. The
    /// production path is [`Self::from_managed_operation`].
    #[cfg(test)]
    pub(crate) fn from_launcher_record(
        target: BrowserTargetBinding,
        launcher: String,
        process_id: u32,
        launched_at_ms: i64,
    ) -> Self {
        Self {
            target,
            operation_record: None,
            source: PreviewSource::ManagedTool,
            provenance: PreviewProvenance::Managed {
                launcher,
                process_id,
                launched_at_ms,
            },
        }
    }
}

/// Presentation metadata only; authority and registry-derived fields are absent.
#[derive(Clone, Debug)]
pub(crate) struct BrowserPresentation {
    pub kind: PreviewKind,
    pub display: DisplayPolicy,
    pub redaction: RedactionStatus,
    pub ttl: Duration,
    pub companion_candidates: Vec<CompanionCandidate>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PresentError {
    #[error("browser-live producer/event bus not configured")]
    NotConfigured,
    #[error("browser presentation requires turn session and run")]
    MissingTurn,
    #[error("browser presentation requires caller identity")]
    MissingIdentity,
    #[error("backend/launcher did not provide an operation-bound browser target")]
    MissingTarget,
    #[error("presentation is bound to a different caller or turn")]
    BindingMismatch,
    #[error("browser presentation is already closed")]
    AlreadyClosed,
    #[error("browser target no longer available: its owner route ended during publication")]
    OwnerGone,
    #[error(transparent)]
    Registry(#[from] ResourceError),
    #[error(transparent)]
    Event(#[from] serde_json::Error),
}

impl From<LifecycleError> for PresentError {
    fn from(error: LifecycleError) -> Self {
        match error {
            LifecycleError::NotConfigured => Self::NotConfigured,
            LifecycleError::Registry(e) => Self::Registry(e),
            LifecycleError::Event(e) => Self::Event(e),
        }
    }
}

/// Thin presentation front-end over the shared `BrowserLiveLifecycle`, which
/// is the only owner of registry transition + grant revocation + event emission
/// and shares the SAME stores as the viewer issuer/backend. The config seam
/// supplies its existing event bus; absent bus denies publication.
#[derive(Clone)]
pub struct BrowserLiveProducer {
    lifecycle: BrowserLiveLifecycle,
    /// The exact-owner registry shared with the viewer backend. `None` (test /
    /// probe producers) registers no owner, so no viewer route can resolve.
    owners: Option<LiveOwnerRegistry>,
}

impl BrowserLiveProducer {
    pub(crate) fn new(resources: ResourceRegistry, grants: ViewerGrantStore) -> Self {
        Self {
            lifecycle: BrowserLiveLifecycle::new(resources, grants),
            owners: None,
        }
    }

    /// Runtime constructor: the SAME lifecycle (stores) and owner registry the
    /// issuer and `LiveViewerBackend` hold; never builds its own stores.
    pub(crate) fn new_with_lifecycle(
        lifecycle: BrowserLiveLifecycle,
        owners: LiveOwnerRegistry,
    ) -> Self {
        Self {
            lifecycle,
            owners: Some(owners),
        }
    }

    /// Binds `event_bus` only to a producer that has none. A producer already
    /// bound (the runtime one, sharing the server bus with the issuer/backend)
    /// is returned unchanged so a second bus cannot split its events.
    pub(crate) fn with_event_bus(&self, event_bus: Arc<GatewayEventBus>) -> Self {
        if self.lifecycle.has_event_bus() {
            return self.clone();
        }
        Self {
            lifecycle: self.lifecycle.with_event_bus(event_bus),
            owners: self.owners.clone(),
        }
    }

    /// The coordinator every close/refusal/expiry path must go through.
    pub(crate) fn lifecycle(&self) -> &BrowserLiveLifecycle {
        &self.lifecycle
    }

    /// Explicit `browser_present` producer entry. Missing authority/target/bus
    /// produces neither a registry entry nor a resource event.
    pub(crate) fn browser_present(
        &self,
        operation: Option<BrowserPresentOperation>,
        presentation: BrowserPresentation,
    ) -> Result<PublishedPreview, PresentError> {
        let operation = operation.ok_or(PresentError::MissingTarget)?;
        let binding = current_binding(operation.target)?;
        let draft = ResourceDraft {
            binding,
            kind: presentation.kind,
            display: presentation.display,
            source: operation.source,
            provenance: operation.provenance,
            redaction: producer_redaction(presentation.redaction),
            ttl: presentation.ttl,
            companion_candidates: presentation.companion_candidates,
        };
        // Register the exact operation owner FIRST (record's own owner/session/
        // target only; no URL/current-tab/profile inference), then publish.
        // A registration failure here means the runtime owner route is
        // unavailable or collides with another owner: `OwnerGone`, not
        // `NotConfigured` (reserved for a producer with no bus/owners).
        let registered = match (&self.owners, operation.operation_record.as_ref()) {
            (Some(owners), Some(record)) => Some((
                owners,
                owners
                    .register_operation_fresh(record)
                    .map_err(|_| PresentError::OwnerGone)?,
            )),
            _ => None,
        };
        let resource = match self.lifecycle.publish(draft) {
            Ok(resource) => resource,
            Err(error) => {
                // Compensation: retire only a registration this call created;
                // a pre-existing one may already back a live resource.
                if let Some((owners, (lease, true))) = &registered {
                    owners.retire_target(lease);
                }
                return Err(error.into());
            }
        };
        self.complete_publication(
            registered.as_ref().map(|(o, (l, fresh))| (*o, l, *fresh)),
            resource,
        )
    }

    /// Post-publish route recheck. The owner may have ended (or the target been
    /// destroyed) between registration and publish: a terminal notification in
    /// that window found no resource to close. If the route no longer
    /// resolves, the resource just published is unreachable, so close it
    /// through the lifecycle (revoke + event). Authority is never restored if
    /// that close fails. A registration THIS call created (`fresh`) is
    /// compare-retired so the active map does not strongly hold the ended
    /// owner; a pre-existing registration is never retired.
    fn complete_publication(
        &self,
        route: Option<(&LiveOwnerRegistry, &OwnerRouteLease, bool)>,
        resource: PreviewResource,
    ) -> Result<PublishedPreview, PresentError> {
        if let Some((owners, lease, fresh)) = route {
            if owners.accepts(lease).is_err() {
                self.close_unroutable(&resource, lease.owner());
                if fresh {
                    owners.retire_target(lease);
                }
                return Err(PresentError::OwnerGone);
            }
        }
        Ok(PublishedPreview {
            producer: self.clone(),
            resource,
            route: route
                .and_then(|(owners, lease, fresh)| fresh.then(|| (owners.clone(), lease.clone()))),
        })
    }

    /// Close a just-published resource whose owner route is gone. Best effort:
    /// a failed close/announcement is logged and the caller still reports
    /// [`PresentError::OwnerGone`]. The close reason is derived from the owner
    /// (ended/closed connection -> `EngineExited`, as the terminal sinks report;
    /// otherwise `TargetClosed`).
    fn close_unroutable(&self, resource: &PreviewResource, owner: &LiveProfileOwner) {
        let Some(binding) = resource.viewer_binding() else {
            return;
        };
        let reason = owner.route_close_reason(&binding.target.target_id);
        if let Err(error) = self.lifecycle.close_binding(binding, reason) {
            tracing::warn!(%error, "browser-live unroutable resource close not announced");
        }
    }

    /// HMR/candidate metadata revises the containing resource, never its target.
    /// Caller and conversation are rechecked against the captured binding.
    pub(crate) fn revise(
        &self,
        previous: &PreviewResource,
        presentation: BrowserPresentation,
    ) -> Result<PreviewResource, PresentError> {
        let binding = previous
            .viewer_binding()
            .ok_or(PresentError::BindingMismatch)?;
        if current_binding(binding.target.clone())? != *binding {
            return Err(PresentError::BindingMismatch);
        }
        let draft = ResourceDraft {
            binding: binding.clone(),
            kind: presentation.kind,
            display: presentation.display,
            source: previous.source,
            provenance: previous.provenance.clone(),
            redaction: producer_redaction(presentation.redaction),
            ttl: presentation.ttl,
            companion_candidates: presentation.companion_candidates,
        };
        Ok(self
            .lifecycle
            .republish(&previous.resource_id, previous.revision, draft)?)
    }

    /// Owner lifecycle maintenance, not a timer hidden in the tool loop.
    /// Each expired resource yields its canonical close event and grant revoke.
    pub(crate) fn sweep_expired(&self) -> Result<usize, PresentError> {
        Ok(self.lifecycle.sweep_expired()?)
    }
}

/// Private construction binds a lifecycle handle to the exact published resource.
/// A launcher retains this handle and calls `service_exited` from its real exit
/// observation; no task-local authority is needed after the turn has finished.
/// Dropping the handle is NOT evidence of a service exit.
pub(crate) struct PublishedPreview {
    producer: BrowserLiveProducer,
    resource: PreviewResource,
    /// The owner route THIS publication registered (never a pre-existing one).
    /// Retired by `service_exited` after the resource is closed.
    route: Option<(LiveOwnerRegistry, OwnerRouteLease)>,
}

impl PublishedPreview {
    pub(crate) fn resource(&self) -> &PreviewResource {
        &self.resource
    }

    pub(crate) fn revise(&mut self, presentation: BrowserPresentation) -> Result<(), PresentError> {
        self.resource = self.producer.revise(&self.resource, presentation)?;
        Ok(())
    }

    /// Close the resource (revokes its viewer grants and emits the canonical
    /// close event, so live viewer sockets are fenced) and then retire the
    /// owner route this publication registered. Idempotent: a second call is
    /// `AlreadyClosed` and changes nothing.
    pub(crate) fn service_exited(&self) -> Result<BrowserLiveClosed, PresentError> {
        let closed = self
            .producer
            .lifecycle
            .close(&self.resource.resource_id, ClosedReason::ServiceExited);
        // Retire the route even if the close announcement failed: the grants
        // are already revoked and the route must not outlive the service.
        if let Some((owners, lease)) = &self.route {
            owners.retire_target(lease);
        }
        closed?.ok_or(PresentError::AlreadyClosed)
    }
}

/// Producer-side redaction clamp, shared by `browser_present` and `revise` so
/// the two publication paths cannot drift. `Safe` is the user's explicit
/// confirmation and is set ONLY by the `browser.viewer.resource.decide`
/// revision CAS; a producer (tool/LLM) may publish only `Pending` or `Refused`.
/// A producer-supplied `Safe` is therefore demoted to `Pending`; `Pending`
/// stays; `Refused` stays so the registry still fails closed (close + revoke).
fn producer_redaction(requested: RedactionStatus) -> RedactionStatus {
    match requested {
        RedactionStatus::Safe => RedactionStatus::Pending,
        other => other,
    }
}

fn current_binding(target: BrowserTargetBinding) -> Result<ViewerBinding, PresentError> {
    let turn = current_turn_context().ok_or(PresentError::MissingTurn)?;
    let task = ConversationBinding {
        session_key: turn.session_key.to_key_string(),
        run_id: turn.run_id,
    };
    // SessionKey is typed, but malformed raw variants can still serialize to
    // nonempty strings (e.g. `agent::`). Reject those rather than normalizing
    // them into a default conversation. The bound value still comes only from
    // TURN_CONTEXT, never a caller argument or alternate task-local.
    if crate::routing::session_key::SessionKey::parse(&task.session_key).as_ref()
        != Some(&turn.session_key)
        || task.run_id.trim().is_empty()
    {
        return Err(PresentError::MissingTurn);
    }
    // The principal is the run-scope verdict only (same derivation as every
    // per-person gate); the caller connection/user task-locals are never read,
    // so a foreign RPC caller cannot re-attribute the run's browser.
    let principal = match run_principal_sans_caller() {
        RunPrincipal::Person(user_id) if !user_id.trim().is_empty() => {
            IssuerIdentity::User { user_id }
        }
        RunPrincipal::Person(_) => return Err(PresentError::MissingIdentity),
        RunPrincipal::Legacy => IssuerIdentity::User {
            user_id: owner_or_legacy(None).to_string(),
        },
        RunPrincipal::Unattached => return Err(PresentError::MissingIdentity),
    };
    if target.profile.trim().is_empty()
        || target.browser_session_id.trim().is_empty()
        || target.target_id.trim().is_empty()
    {
        return Err(PresentError::MissingTarget);
    }
    Ok(ViewerBinding {
        principal,
        task,
        target,
    })
}

#[cfg(test)]
#[path = "present_tests.rs"]
mod tests;
