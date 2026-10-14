//! Pure state model for the browser viewer shell.
//!
//! The Panel never *decides* what browser it is looking at. A
//! [`BrowserViewerModel`] is built only from data the gateway already
//! authorized — a [`PreviewResource`] (bus event) or a [`BrowserLiveSnapshot`]
//! (viewer exchange) — and every later update must match the binding that data
//! carried **exactly**. There is deliberately no constructor that takes a
//! profile, a URL, an "active tab", or a target id: nothing here can be
//! reached from ambient UI state.
//!
//! Scope of this slice: model + render shell. It does not consume fragment
//! nonces, exchange tickets, open sockets, or touch `location.hash`; the
//! issuer/producer wiring that feeds these constructors lands with the
//! events/producer task.
//!
//! # Epoch rule
//!
//! `connection_epoch`, `stream_epoch` and `control_epoch` are each required to
//! be non-decreasing on their own. That is stricter than a lexicographic
//! ordering (which would let `stream_epoch` fall on a newer connection); the
//! wire contract does not promise such a reset, so a regression on any axis is
//! rejected rather than guessed at.

use aleph_protocol::browser_live::{
    BrowserLiveClosed, BrowserLiveSnapshot, BrowserLiveState, BrowserLiveValidationError,
    ClosedReason, CompanionCandidate, Controller, ConversationBinding, DisplayPolicy,
    PreviewResource, RedactionStatus, ResourceBinding, ViewerBinding, ViewportCssPx,
};
use leptos::prelude::*;

/// Why a model refused an input. Every variant leaves the model untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    /// `PreviewResource::validate` failed.
    InvalidResource(BrowserLiveValidationError),
    /// The resource is bound to an artifact, not a browser target.
    NotBrowserResource,
    /// The model is unbound, so there is nothing for the input to match.
    Unbound,
    /// `resource_id` differs from the bound resource.
    ResourceMismatch,
    /// The viewer binding differs on any axis (principal, task, target).
    BindingMismatch,
    /// A resource revision older than the bound one.
    StaleRevision,
    /// An epoch went backwards on some axis.
    EpochRegression,
    /// Live state arrived before any authorized snapshot.
    NoSnapshot,
    /// The viewer is already closed; only a new resource can reopen it.
    AlreadyClosed,
    /// The same resource revision was already refused; it stays refused.
    AlreadyRefused,
    /// The candidate id is not in the bound resource's candidate list.
    UnknownCandidate,
    /// The resource was refused by redaction and can never be bound. Callers
    /// at the model level turn this into `Unbound(Refused)`.
    Refused,
}

/// Tombstone of a viewer that ended. It remembers *which* resource revision
/// closed (never a target), so a replayed or stale resource with the same id
/// cannot bring it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosedResource {
    pub resource_id: String,
    pub revision: u64,
    pub reason: ClosedReason,
}

/// Tombstone of a resource refused by redaction. Like [`ClosedResource`] it
/// remembers only *which* resource revision was refused (never a target), so
/// an older or replayed same-id resource cannot resurrect it (fail closed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedResource {
    pub resource_id: String,
    pub revision: u64,
}

/// Why there is no browser to show. Carries no target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnboundReason {
    /// Nothing has been published for this session yet.
    NoResource,
    /// The last browser viewer ended; the tombstone blocks same-id replays.
    Closed(ClosedResource),
    /// The resource was refused by redaction; the tombstone blocks same-id
    /// resources up to (and including) the refused revision.
    Refused(RefusedResource),
}

/// Where a bound viewer is in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerPhase {
    /// A resource is known but no authorized snapshot has arrived.
    AwaitingAuthorization,
    /// An authorized snapshot is held; epochs are tracked.
    Live,
    /// The gateway reported the viewer closed.
    Closed(ClosedReason),
}

/// Epoch / control facts from the last accepted snapshot or state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveFacts {
    pub connection_epoch: u64,
    pub stream_epoch: u64,
    pub control_epoch: u64,
    pub viewport_css_px: ViewportCssPx,
    pub controller: Controller,
}

impl LiveFacts {
    fn of_snapshot(snapshot: &BrowserLiveSnapshot) -> Self {
        Self {
            connection_epoch: snapshot.connection_epoch,
            stream_epoch: snapshot.stream_epoch,
            control_epoch: snapshot.control_epoch,
            viewport_css_px: snapshot.viewport_css_px.clone(),
            controller: snapshot.controller.clone(),
        }
    }

    fn of_state(state: &BrowserLiveState) -> Self {
        Self {
            connection_epoch: state.connection_epoch,
            stream_epoch: state.stream_epoch,
            control_epoch: state.control_epoch,
            viewport_css_px: state.viewport_css_px.clone(),
            controller: state.controller.clone(),
        }
    }
}

/// Validate a resource and return its browser binding. Shared by every entry
/// point so no path can accept what another would reject.
fn vet_resource(resource: &PreviewResource) -> Result<&ViewerBinding, ModelError> {
    resource.validate().map_err(ModelError::InvalidResource)?;
    resource
        .viewer_binding()
        .ok_or(ModelError::NotBrowserResource)
}

/// A browser resource the gateway authorized this Panel to display.
///
/// Never holds a refused resource: construction and updates fail with
/// [`ModelError::Refused`] instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundBrowser {
    resource: PreviewResource,
    viewer: ViewerBinding,
    live: Option<LiveFacts>,
    phase: ViewerPhase,
    selected_candidate: Option<String>,
    muted: bool,
}

impl BoundBrowser {
    /// Bind to a validated browser-kind resource. No snapshot yet, so the
    /// phase is [`ViewerPhase::AwaitingAuthorization`]. A refused resource is
    /// [`ModelError::Refused`].
    pub fn from_resource(resource: PreviewResource) -> Result<Self, ModelError> {
        let viewer = vet_resource(&resource)?.clone();
        if resource.redaction == RedactionStatus::Refused {
            return Err(ModelError::Refused);
        }
        Ok(Self {
            resource,
            viewer,
            live: None,
            phase: ViewerPhase::AwaitingAuthorization,
            selected_candidate: None,
            muted: false,
        })
    }

    /// Bind from an authorized exchange snapshot.
    pub fn from_snapshot(snapshot: BrowserLiveSnapshot) -> Result<Self, ModelError> {
        let live = LiveFacts::of_snapshot(&snapshot);
        let mut bound = Self::from_resource(snapshot.resource)?;
        bound.live = Some(live);
        bound.phase = ViewerPhase::Live;
        Ok(bound)
    }

    #[must_use]
    pub fn resource(&self) -> &PreviewResource {
        &self.resource
    }

    #[must_use]
    pub fn viewer(&self) -> &ViewerBinding {
        &self.viewer
    }

    #[must_use]
    pub fn live(&self) -> Option<&LiveFacts> {
        self.live.as_ref()
    }

    #[must_use]
    pub fn phase(&self) -> ViewerPhase {
        self.phase
    }

    #[must_use]
    pub fn selected_candidate(&self) -> Option<&CompanionCandidate> {
        let id = self.selected_candidate.as_deref()?;
        self.resource
            .companion_candidates
            .iter()
            .find(|candidate| candidate.id == id)
    }

    #[must_use]
    pub fn candidates(&self) -> &[CompanionCandidate] {
        &self.resource.companion_candidates
    }

    #[must_use]
    pub fn is_muted(&self) -> bool {
        self.muted
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    fn matches_exact(&self, resource_id: &str, binding: &ViewerBinding) -> Result<(), ModelError> {
        if self.resource.resource_id != resource_id {
            return Err(ModelError::ResourceMismatch);
        }
        if &self.viewer != binding {
            return Err(ModelError::BindingMismatch);
        }
        Ok(())
    }

    /// The one admission rule for new live facts, used by snapshots and
    /// states alike: no epoch axis may go backwards, and within one
    /// `control_epoch` the controller may not change.
    fn check_epochs(&self, next: &LiveFacts) -> Result<(), ModelError> {
        if let Some(cur) = &self.live {
            if next.connection_epoch < cur.connection_epoch
                || next.stream_epoch < cur.stream_epoch
                || next.control_epoch < cur.control_epoch
                || (next.control_epoch == cur.control_epoch && next.controller != cur.controller)
            {
                return Err(ModelError::EpochRegression);
            }
        }
        Ok(())
    }

    /// Swap in newer resource metadata. A selection survives only if the new
    /// candidate list still offers it; phase, live facts and mute are kept.
    fn refresh_resource(&mut self, resource: PreviewResource) {
        if let Some(id) = &self.selected_candidate {
            if !resource.companion_candidates.iter().any(|c| &c.id == id) {
                self.selected_candidate = None;
            }
        }
        self.resource = resource;
    }

    /// Accept a later snapshot of the *same* resource and binding. A refused
    /// snapshot is [`ModelError::Refused`] and leaves this value untouched;
    /// [`BrowserViewerModel`] turns that into `Unbound(Refused)`.
    pub fn apply_snapshot(&mut self, snapshot: BrowserLiveSnapshot) -> Result<(), ModelError> {
        if matches!(self.phase, ViewerPhase::Closed(_)) {
            return Err(ModelError::AlreadyClosed);
        }
        let next = LiveFacts::of_snapshot(&snapshot);
        let resource = snapshot.resource;
        let viewer = vet_resource(&resource)?;
        self.matches_exact(&resource.resource_id, viewer)?;
        if resource.revision < self.resource.revision {
            return Err(ModelError::StaleRevision);
        }
        if resource.redaction == RedactionStatus::Refused {
            return Err(ModelError::Refused);
        }
        self.check_epochs(&next)?;
        self.refresh_resource(resource);
        self.live = Some(next);
        self.phase = ViewerPhase::Live;
        Ok(())
    }

    /// Accept a live state update for the bound resource.
    pub fn apply_state(&mut self, state: &BrowserLiveState) -> Result<(), ModelError> {
        if matches!(self.phase, ViewerPhase::Closed(_)) {
            return Err(ModelError::AlreadyClosed);
        }
        self.matches_exact(&state.resource_id, &state.binding)?;
        if self.live.is_none() {
            return Err(ModelError::NoSnapshot);
        }
        let next = LiveFacts::of_state(state);
        self.check_epochs(&next)?;
        self.live = Some(next);
        Ok(())
    }

    /// Accept a close notice only when it names exactly this resource and
    /// binding.
    pub fn apply_closed(&mut self, closed: &BrowserLiveClosed) -> Result<(), ModelError> {
        if self.resource.resource_id != closed.resource_id {
            return Err(ModelError::ResourceMismatch);
        }
        match &closed.binding {
            ResourceBinding::Browser { viewer } if viewer == &self.viewer => {}
            _ => return Err(ModelError::BindingMismatch),
        }
        self.phase = ViewerPhase::Closed(closed.reason);
        Ok(())
    }

    /// Choose one of this resource's own candidates. Selection only changes
    /// what is *displayed*: it never produces a target, resource or grant.
    pub fn select_candidate(&mut self, candidate_id: &str) -> Result<(), ModelError> {
        if !self
            .resource
            .companion_candidates
            .iter()
            .any(|candidate| candidate.id == candidate_id)
        {
            return Err(ModelError::UnknownCandidate);
        }
        self.selected_candidate = Some(candidate_id.to_string());
        Ok(())
    }

    /// True when this viewer belongs to the given chat session and so may stay
    /// in memory across a session switch. Any other session drops it.
    #[must_use]
    pub fn retain_for_session(&self, session_key: &str) -> bool {
        self.viewer.task.session_key == session_key
    }

    /// May the workspace open itself for this resource right now?
    ///
    /// Requires the *foreground* task to equal the resource's task on both
    /// `session_key` and `run_id`, an `AutoRevealCurrentRun` display policy, a
    /// live (non-replayed) event, an unmuted viewer, an unexpired resource, a
    /// displayable redaction state, and a viewer that has not closed.
    #[must_use]
    pub fn auto_reveal(&self, foreground: &ConversationBinding, now_ms: i64, replay: bool) -> bool {
        !replay
            && !self.muted
            && self.resource.display == DisplayPolicy::AutoRevealCurrentRun
            && &self.viewer.task == foreground
            && now_ms < self.resource.expires_at_ms
            && matches!(
                self.resource.redaction,
                RedactionStatus::Safe | RedactionStatus::Redacted
            )
            && !matches!(self.phase, ViewerPhase::Closed(_))
    }
}

/// What the browser tab shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserViewerModel {
    Unbound(UnboundReason),
    Bound(Box<BoundBrowser>),
}

impl Default for BrowserViewerModel {
    fn default() -> Self {
        Self::Unbound(UnboundReason::NoResource)
    }
}

impl BrowserViewerModel {
    /// Build from a published resource. A refused resource becomes
    /// `Unbound(Refused)` instead of an error: it is a valid wire value that
    /// must simply never be displayed.
    pub fn from_resource(resource: PreviewResource) -> Result<Self, ModelError> {
        let tomb = RefusedResource {
            resource_id: resource.resource_id.clone(),
            revision: resource.revision,
        };
        match BoundBrowser::from_resource(resource) {
            Ok(bound) => Ok(Self::Bound(Box::new(bound))),
            Err(ModelError::Refused) => Ok(Self::Unbound(UnboundReason::Refused(tomb))),
            Err(other) => Err(other),
        }
    }

    /// Build from an authorized viewer snapshot.
    pub fn from_snapshot(snapshot: BrowserLiveSnapshot) -> Result<Self, ModelError> {
        let tomb = RefusedResource {
            resource_id: snapshot.resource.resource_id.clone(),
            revision: snapshot.resource.revision,
        };
        match BoundBrowser::from_snapshot(snapshot) {
            Ok(bound) => Ok(Self::Bound(Box::new(bound))),
            Err(ModelError::Refused) => Ok(Self::Unbound(UnboundReason::Refused(tomb))),
            Err(other) => Err(other),
        }
    }

    #[must_use]
    pub fn bound(&self) -> Option<&BoundBrowser> {
        match self {
            Self::Bound(bound) => Some(bound),
            Self::Unbound(_) => None,
        }
    }

    fn bound_mut(&mut self) -> Result<&mut BoundBrowser, ModelError> {
        match self {
            Self::Bound(bound) => Ok(bound),
            Self::Unbound(_) => Err(ModelError::Unbound),
        }
    }

    /// A matching, non-stale snapshot that is now refused fails closed: the
    /// model becomes `Unbound(Refused)`. Any mismatch or invalid snapshot is
    /// an error and changes nothing.
    pub fn apply_snapshot(&mut self, snapshot: BrowserLiveSnapshot) -> Result<(), ModelError> {
        let tomb = RefusedResource {
            resource_id: snapshot.resource.resource_id.clone(),
            revision: snapshot.resource.revision,
        };
        match self.bound_mut()?.apply_snapshot(snapshot) {
            Err(ModelError::Refused) => {
                *self = Self::Unbound(UnboundReason::Refused(tomb));
                Ok(())
            }
            other => other,
        }
    }

    pub fn apply_state(&mut self, state: &BrowserLiveState) -> Result<(), ModelError> {
        self.bound_mut()?.apply_state(state)
    }

    /// A matching close notice ends the viewer and unbinds the model; a
    /// non-matching one is rejected and changes nothing.
    pub fn apply_closed(&mut self, closed: &BrowserLiveClosed) -> Result<(), ModelError> {
        let bound = self.bound_mut()?;
        bound.apply_closed(closed)?;
        *self = Self::Unbound(UnboundReason::Closed(ClosedResource {
            resource_id: bound.resource.resource_id.clone(),
            revision: bound.resource.revision,
            reason: closed.reason,
        }));
        Ok(())
    }

    pub fn select_candidate(&mut self, candidate_id: &str) -> Result<(), ModelError> {
        self.bound_mut()?.select_candidate(candidate_id)
    }

    /// Bind a published resource.
    ///
    /// For the *same* `resource_id` this is monotonic: the viewer binding must
    /// be identical, an older revision is [`ModelError::StaleRevision`], the
    /// same revision is an idempotent no-op, and a newer one refreshes only
    /// the resource metadata (live facts, phase, mute and a still-offered
    /// selection are kept). A closed resource stays closed for every revision
    /// up to the one that closed (`AlreadyClosed` / `StaleRevision`). A
    /// different `resource_id` replaces the model.
    pub fn bind_resource(&mut self, resource: PreviewResource) -> Result<(), ModelError> {
        enum Plan {
            Fresh,
            Refresh,
            Unbind,
        }
        let incoming = vet_resource(&resource)?;
        let plan = match &*self {
            Self::Bound(cur) if cur.resource.resource_id == resource.resource_id => {
                if &cur.viewer != incoming {
                    return Err(ModelError::BindingMismatch);
                }
                let closed = matches!(cur.phase, ViewerPhase::Closed(_));
                if resource.revision < cur.resource.revision {
                    return Err(ModelError::StaleRevision);
                }
                if resource.revision == cur.resource.revision {
                    return if closed {
                        Err(ModelError::AlreadyClosed)
                    } else if resource.redaction == RedactionStatus::Refused {
                        // Same revision now refused: never an idempotent no-op.
                        // Fail closed and remember the refusal.
                        *self = Self::from_resource(resource)?;
                        Ok(())
                    } else {
                        Ok(())
                    };
                }
                if closed {
                    Plan::Fresh
                } else if resource.redaction == RedactionStatus::Refused {
                    Plan::Unbind
                } else {
                    Plan::Refresh
                }
            }
            Self::Unbound(UnboundReason::Closed(tomb))
                if tomb.resource_id == resource.resource_id =>
            {
                if resource.revision < tomb.revision {
                    return Err(ModelError::StaleRevision);
                }
                if resource.revision == tomb.revision {
                    return Err(ModelError::AlreadyClosed);
                }
                Plan::Fresh
            }
            Self::Unbound(UnboundReason::Refused(tomb))
                if tomb.resource_id == resource.resource_id =>
            {
                if resource.revision < tomb.revision {
                    return Err(ModelError::StaleRevision);
                }
                if resource.revision == tomb.revision {
                    return Err(ModelError::AlreadyRefused);
                }
                // Strictly newer: a Safe/Redacted one restarts, a Refused one
                // replaces the tombstone (both via `from_resource`).
                Plan::Fresh
            }
            _ => Plan::Fresh,
        };
        match plan {
            Plan::Fresh => *self = Self::from_resource(resource)?,
            Plan::Unbind => {
                *self = Self::Unbound(UnboundReason::Refused(RefusedResource {
                    resource_id: resource.resource_id.clone(),
                    revision: resource.revision,
                }));
            }
            Plan::Refresh => {
                if let Self::Bound(cur) = self {
                    cur.refresh_resource(resource);
                }
            }
        }
        Ok(())
    }

    /// Keep the model only for the session it belongs to.
    pub fn retain_for_session(&mut self, session_key: &str) {
        if let Self::Bound(bound) = self {
            if !bound.retain_for_session(session_key) {
                *self = Self::default();
            }
        }
    }

    #[must_use]
    pub fn auto_reveal(&self, foreground: &ConversationBinding, now_ms: i64, replay: bool) -> bool {
        self.bound()
            .is_some_and(|bound| bound.auto_reveal(foreground, now_ms, replay))
    }
}

/// Context handle for the workspace's Browser tab.
///
/// Provided by whoever owns the producer wiring (not wired at the Panel root
/// yet); `WorkspacePanel` offers the Browser body only when this context
/// exists **and** holds a bound model.
#[derive(Clone, Copy)]
pub struct BrowserWorkspace {
    pub model: RwSignal<BrowserViewerModel>,
    /// Count of decoded `browser.live.*` bus frames seen by the bridge. Not
    /// reactive: it only fences a Gateway list snapshot against frames that
    /// landed while the list request was in flight (see `bridge::seed_workspace`).
    frames: StoredValue<u64>,
}

impl BrowserWorkspace {
    #[must_use]
    pub fn new() -> Self {
        Self {
            model: RwSignal::new(BrowserViewerModel::default()),
            frames: StoredValue::new(0),
        }
    }

    /// Record one decoded browser-live bus frame (disposed handle: no-op).
    pub fn note_frame(&self) {
        let _ = self.frames.try_update_value(|n| *n = n.wrapping_add(1));
    }

    /// Decoded browser-live frames seen so far (`0` once disposed).
    #[must_use]
    pub fn frame_seq(&self) -> u64 {
        self.frames.try_get_value().unwrap_or(0)
    }

    /// Tracked: does the tab have something to show?
    #[must_use]
    pub fn has_browser(&self) -> bool {
        self.model.with(|model| model.bound().is_some())
    }
}

impl Default for BrowserWorkspace {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::browser_live::{
        AnimationPreview, ArtifactBinding, BrowserTargetBinding, CompanionArrangement,
        CompanionLayout, CompanionScreen, CompanionStateHint, IssuerIdentity, PreviewKind,
        PreviewProvenance, PreviewSource,
    };

    fn task() -> ConversationBinding {
        ConversationBinding {
            session_key: "agent:operator:browser".to_string(),
            run_id: "run-7".to_string(),
        }
    }

    fn viewer(target_id: &str) -> ViewerBinding {
        ViewerBinding {
            principal: IssuerIdentity::User {
                user_id: "user-42".to_string(),
            },
            task: task(),
            target: BrowserTargetBinding {
                profile: "default".to_string(),
                browser_session_id: "browser-session-3".to_string(),
                target_id: target_id.to_string(),
            },
        }
    }

    fn candidate(id: &str) -> CompanionCandidate {
        CompanionCandidate {
            id: id.to_string(),
            label: id.to_string(),
            layout: CompanionLayout {
                arrangement: CompanionArrangement::Single,
                screens: vec![CompanionScreen {
                    id: "screen-main".to_string(),
                    label: "Main".to_string(),
                    viewport_css_px: ViewportCssPx {
                        width: 390,
                        height: 844,
                    },
                }],
            },
            state: CompanionStateHint {
                active_screen_id: Some("screen-main".to_string()),
                animation: Some(AnimationPreview {
                    name: "pulse".to_string(),
                    duration_ms: 250,
                    looped: true,
                }),
                prefers_reduced_motion: false,
            },
        }
    }

    fn resource() -> PreviewResource {
        PreviewResource {
            resource_id: "resource-5".to_string(),
            revision: 2,
            binding: ResourceBinding::Browser {
                viewer: viewer("target-9"),
            },
            kind: PreviewKind::Page,
            display: DisplayPolicy::AutoRevealCurrentRun,
            source: PreviewSource::BrowserTool,
            provenance: PreviewProvenance::NotManaged,
            expires_at_ms: 2_000,
            redaction: RedactionStatus::Safe,
            companion_candidates: vec![candidate("a"), candidate("b")],
        }
    }

    fn snapshot() -> BrowserLiveSnapshot {
        BrowserLiveSnapshot {
            resource: resource(),
            connection_epoch: 11,
            stream_epoch: 13,
            last_frame_seq: 17,
            viewport_css_px: ViewportCssPx {
                width: 1280,
                height: 720,
            },
            controller: Controller::Agent,
            control_epoch: 19,
        }
    }

    fn state() -> BrowserLiveState {
        BrowserLiveState {
            resource_id: "resource-5".to_string(),
            binding: viewer("target-9"),
            connection_epoch: 11,
            stream_epoch: 13,
            viewport_css_px: ViewportCssPx {
                width: 1280,
                height: 720,
            },
            controller: Controller::Agent,
            control_epoch: 19,
        }
    }

    fn live_model() -> BrowserViewerModel {
        BrowserViewerModel::from_snapshot(snapshot()).unwrap()
    }

    fn refused_model(revision: u64) -> BrowserViewerModel {
        BrowserViewerModel::Unbound(UnboundReason::Refused(RefusedResource {
            resource_id: "resource-5".to_string(),
            revision,
        }))
    }

    #[test]
    fn default_is_unbound_and_has_no_target() {
        let model = BrowserViewerModel::default();
        assert_eq!(
            model,
            BrowserViewerModel::Unbound(UnboundReason::NoResource)
        );
        assert!(model.bound().is_none());
        assert!(!model.auto_reveal(&task(), 0, false));
    }

    #[test]
    fn resource_must_validate_and_be_browser_bound() {
        let mut artifact = resource();
        artifact.kind = PreviewKind::Artifact;
        artifact.binding = ResourceBinding::Artifact {
            artifact: ArtifactBinding {
                principal: IssuerIdentity::User {
                    user_id: "user-42".to_string(),
                },
                task: task(),
                artifact_id: "artifact-1".to_string(),
            },
        };
        assert_eq!(
            BrowserViewerModel::from_resource(artifact),
            Err(ModelError::NotBrowserResource)
        );

        let mut mismatch = resource();
        mismatch.kind = PreviewKind::Artifact;
        assert!(matches!(
            BrowserViewerModel::from_resource(mismatch),
            Err(ModelError::InvalidResource(_))
        ));

        let model = BrowserViewerModel::from_resource(resource()).unwrap();
        let bound = model.bound().unwrap();
        assert_eq!(bound.phase(), ViewerPhase::AwaitingAuthorization);
        assert!(bound.live().is_none());
    }

    #[test]
    fn refused_resource_is_never_bound() {
        let mut refused = resource();
        refused.redaction = RedactionStatus::Refused;
        assert_eq!(
            BrowserViewerModel::from_resource(refused.clone()).unwrap(),
            refused_model(2)
        );
        let mut snap = snapshot();
        snap.resource = refused;
        assert_eq!(
            BrowserViewerModel::from_snapshot(snap).unwrap(),
            refused_model(2)
        );
    }

    #[test]
    fn snapshot_binds_live_with_exact_binding() {
        let model = live_model();
        let bound = model.bound().unwrap();
        assert_eq!(bound.phase(), ViewerPhase::Live);
        assert_eq!(bound.viewer(), &viewer("target-9"));
        assert_eq!(bound.live().unwrap().control_epoch, 19);
    }

    #[test]
    fn snapshot_for_other_target_or_resource_is_rejected() {
        let mut model = live_model();
        let mut other_target = snapshot();
        other_target.resource.binding = ResourceBinding::Browser {
            viewer: viewer("target-other"),
        };
        assert_eq!(
            model.apply_snapshot(other_target),
            Err(ModelError::BindingMismatch)
        );
        let mut other_resource = snapshot();
        other_resource.resource.resource_id = "resource-other".to_string();
        assert_eq!(
            model.apply_snapshot(other_resource),
            Err(ModelError::ResourceMismatch)
        );
        assert_eq!(model, live_model());
    }

    #[test]
    fn snapshot_rejects_revision_and_epoch_regression() {
        let mut model = live_model();
        let mut stale = snapshot();
        stale.resource.revision = 1;
        assert_eq!(model.apply_snapshot(stale), Err(ModelError::StaleRevision));
        let mut back = snapshot();
        back.stream_epoch = 12;
        assert_eq!(model.apply_snapshot(back), Err(ModelError::EpochRegression));
        let mut forward = snapshot();
        forward.resource.revision = 3;
        forward.stream_epoch = 14;
        model.apply_snapshot(forward).unwrap();
        assert_eq!(model.bound().unwrap().resource().revision, 3);
    }

    #[test]
    fn state_requires_snapshot_and_exact_scope() {
        let mut awaiting = BrowserViewerModel::from_resource(resource()).unwrap();
        assert_eq!(awaiting.apply_state(&state()), Err(ModelError::NoSnapshot));

        let mut model = live_model();
        let mut wrong_resource = state();
        wrong_resource.resource_id = "other".to_string();
        assert_eq!(
            model.apply_state(&wrong_resource),
            Err(ModelError::ResourceMismatch)
        );
        let mut wrong_binding = state();
        wrong_binding.binding = viewer("target-other");
        assert_eq!(
            model.apply_state(&wrong_binding),
            Err(ModelError::BindingMismatch)
        );
        assert_eq!(
            BrowserViewerModel::default().apply_state(&state()),
            Err(ModelError::Unbound)
        );
    }

    #[test]
    fn state_rejects_epoch_regression_on_every_axis() {
        let mut model = live_model();
        for mutate in [
            (|s: &mut BrowserLiveState| s.connection_epoch -= 1) as fn(&mut BrowserLiveState),
            |s| s.stream_epoch -= 1,
            |s| s.control_epoch -= 1,
        ] {
            let mut regressed = state();
            mutate(&mut regressed);
            assert_eq!(
                model.apply_state(&regressed),
                Err(ModelError::EpochRegression)
            );
        }
        let mut forward = state();
        forward.control_epoch += 1;
        forward.controller = Controller::Human {
            principal: IssuerIdentity::User {
                user_id: "user-42".to_string(),
            },
            viewer_session_id: "vs-1".to_string(),
        };
        model.apply_state(&forward).unwrap();
        assert_eq!(model.bound().unwrap().live().unwrap().control_epoch, 20);
    }

    #[test]
    fn controller_cannot_change_within_one_control_epoch() {
        let mut model = live_model();
        let mut same_epoch = state();
        same_epoch.controller = Controller::Human {
            principal: IssuerIdentity::User {
                user_id: "user-42".to_string(),
            },
            viewer_session_id: "vs-1".to_string(),
        };
        assert_eq!(
            model.apply_state(&same_epoch),
            Err(ModelError::EpochRegression)
        );
    }

    #[test]
    fn closed_only_applies_to_exact_binding() {
        let closed = |binding: ResourceBinding, id: &str| BrowserLiveClosed {
            resource_id: id.to_string(),
            binding,
            reason: ClosedReason::TargetClosed,
            at_ms: 5,
        };
        let mut model = live_model();
        assert_eq!(
            model.apply_closed(&closed(
                ResourceBinding::Browser {
                    viewer: viewer("target-other"),
                },
                "resource-5"
            )),
            Err(ModelError::BindingMismatch)
        );
        assert_eq!(
            model.apply_closed(&closed(
                ResourceBinding::Browser {
                    viewer: viewer("target-9"),
                },
                "resource-other"
            )),
            Err(ModelError::ResourceMismatch)
        );
        assert!(model.bound().is_some());
        model
            .apply_closed(&closed(
                ResourceBinding::Browser {
                    viewer: viewer("target-9"),
                },
                "resource-5",
            ))
            .unwrap();
        assert_eq!(
            model,
            BrowserViewerModel::Unbound(UnboundReason::Closed(ClosedResource {
                resource_id: "resource-5".to_string(),
                revision: 2,
                reason: ClosedReason::TargetClosed,
            }))
        );
    }

    #[test]
    fn select_candidate_stays_within_the_resource() {
        let mut model = live_model();
        assert_eq!(
            model.select_candidate("nope"),
            Err(ModelError::UnknownCandidate)
        );
        model.select_candidate("b").unwrap();
        let bound = model.bound().unwrap();
        assert_eq!(bound.selected_candidate().unwrap().id, "b");
        // Selection did not touch the binding or resource.
        assert_eq!(bound.viewer(), &viewer("target-9"));
        assert_eq!(bound.resource().resource_id, "resource-5");

        // A newer revision that drops "b" clears the selection.
        let mut next = snapshot();
        next.resource.revision = 3;
        next.resource.companion_candidates = vec![candidate("a")];
        model.apply_snapshot(next).unwrap();
        assert!(model.bound().unwrap().selected_candidate().is_none());
    }

    #[test]
    fn bind_resource_refuses_revision_rollback_for_same_resource() {
        let mut model = BrowserViewerModel::from_resource(resource()).unwrap();
        let mut stale = resource();
        stale.revision = 1;
        assert_eq!(model.bind_resource(stale), Err(ModelError::StaleRevision));
        let mut other = resource();
        other.resource_id = "resource-6".to_string();
        other.revision = 1;
        model.bind_resource(other).unwrap();
        assert_eq!(model.bound().unwrap().resource().resource_id, "resource-6");
    }

    fn refused_snapshot() -> BrowserLiveSnapshot {
        let mut snap = snapshot();
        snap.resource.redaction = RedactionStatus::Refused;
        snap
    }

    fn closed_notice(reason: ClosedReason) -> BrowserLiveClosed {
        BrowserLiveClosed {
            resource_id: "resource-5".to_string(),
            binding: ResourceBinding::Browser {
                viewer: viewer("target-9"),
            },
            reason,
            at_ms: 5,
        }
    }

    #[test]
    fn same_revision_bind_is_idempotent_and_keeps_live_state() {
        let mut model = live_model();
        model.select_candidate("b").unwrap();
        if let BrowserViewerModel::Bound(bound) = &mut model {
            bound.set_muted(true);
        }
        let before = model.clone();
        model.bind_resource(resource()).unwrap();
        assert_eq!(model, before);
        assert_eq!(model.bound().unwrap().phase(), ViewerPhase::Live);
    }

    #[test]
    fn newer_same_resource_bind_refreshes_metadata_but_keeps_live_state() {
        let mut model = live_model();
        model.select_candidate("b").unwrap();
        if let BrowserViewerModel::Bound(bound) = &mut model {
            bound.set_muted(true);
        }
        let mut newer = resource();
        newer.revision = 3;
        newer.expires_at_ms = 9_000;
        model.bind_resource(newer).unwrap();
        let bound = model.bound().unwrap();
        assert_eq!(bound.resource().revision, 3);
        assert_eq!(bound.resource().expires_at_ms, 9_000);
        assert_eq!(bound.phase(), ViewerPhase::Live);
        assert_eq!(bound.live().unwrap().control_epoch, 19);
        assert!(bound.is_muted());
        assert_eq!(bound.selected_candidate().unwrap().id, "b");

        // A selection the newer revision no longer offers is cleared.
        let mut dropped = resource();
        dropped.revision = 4;
        dropped.companion_candidates = vec![candidate("a")];
        model.bind_resource(dropped).unwrap();
        let bound = model.bound().unwrap();
        assert!(bound.selected_candidate().is_none());
        assert_eq!(bound.phase(), ViewerPhase::Live);
    }

    #[test]
    fn same_resource_bind_requires_identical_binding_and_validity() {
        let mut model = live_model();
        let before = model.clone();
        let mut other_target = resource();
        other_target.revision = 3;
        other_target.binding = ResourceBinding::Browser {
            viewer: viewer("target-other"),
        };
        assert_eq!(
            model.bind_resource(other_target),
            Err(ModelError::BindingMismatch)
        );
        let mut invalid = resource();
        invalid.revision = 3;
        invalid.kind = PreviewKind::Artifact;
        assert!(matches!(
            model.bind_resource(invalid),
            Err(ModelError::InvalidResource(_))
        ));
        assert_eq!(model, before);
    }

    #[test]
    fn newer_refused_bind_fails_closed() {
        let mut model = live_model();
        let mut refused = resource();
        refused.revision = 3;
        refused.redaction = RedactionStatus::Refused;
        model.bind_resource(refused).unwrap();
        assert_eq!(model, refused_model(3));
    }

    #[test]
    fn same_revision_refused_bind_over_bound_fails_closed() {
        let mut model = live_model();
        let mut refused = resource();
        refused.redaction = RedactionStatus::Refused;
        model.bind_resource(refused).unwrap();
        assert_eq!(model, refused_model(2));
    }

    fn refused_resource(revision: u64) -> PreviewResource {
        let mut r = resource();
        r.revision = revision;
        r.redaction = RedactionStatus::Refused;
        r
    }

    #[test]
    fn refused_tombstone_blocks_older_and_same_revision_safe_rebind() {
        for mut model in [
            BrowserViewerModel::from_resource(refused_resource(3)).unwrap(),
            {
                let mut m = live_model();
                let mut snap = refused_snapshot();
                snap.resource.revision = 3;
                m.apply_snapshot(snap).unwrap();
                m
            },
            {
                let mut m = live_model();
                m.bind_resource(refused_resource(3)).unwrap();
                m
            },
        ] {
            assert_eq!(model, refused_model(3));
            let mut older = resource();
            older.revision = 2;
            assert_eq!(model.bind_resource(older), Err(ModelError::StaleRevision));
            assert_eq!(model, refused_model(3));
            let mut same = resource();
            same.revision = 3;
            assert_eq!(model.bind_resource(same), Err(ModelError::AlreadyRefused));
            assert_eq!(model, refused_model(3));
            assert_eq!(
                model.bind_resource(refused_resource(3)),
                Err(ModelError::AlreadyRefused)
            );
            assert_eq!(
                model.bind_resource(refused_resource(1)),
                Err(ModelError::StaleRevision)
            );
            assert_eq!(model, refused_model(3));
        }
    }

    #[test]
    fn refused_tombstone_allows_newer_rebind_and_other_ids() {
        let mut model = BrowserViewerModel::from_resource(refused_resource(3)).unwrap();
        // Newer refused replaces the tombstone.
        model.bind_resource(refused_resource(5)).unwrap();
        assert_eq!(model, refused_model(5));
        // A newer Safe resource restarts the viewer.
        let mut newer = resource();
        newer.revision = 6;
        model.bind_resource(newer).unwrap();
        assert_eq!(model.bound().unwrap().resource().revision, 6);
        // A different id binds normally even over a tombstone.
        let mut model = BrowserViewerModel::from_resource(refused_resource(3)).unwrap();
        let mut other = resource();
        other.resource_id = "resource-6".to_string();
        other.revision = 1;
        model.bind_resource(other).unwrap();
        assert_eq!(model.bound().unwrap().resource().resource_id, "resource-6");
    }

    #[test]
    fn refused_snapshot_unbinds_but_mismatch_keeps_model() {
        let mut model = live_model();
        model.apply_snapshot(refused_snapshot()).unwrap();
        assert_eq!(model, refused_model(2));

        let mut model = live_model();
        let mut other_target = refused_snapshot();
        other_target.resource.binding = ResourceBinding::Browser {
            viewer: viewer("target-other"),
        };
        assert_eq!(
            model.apply_snapshot(other_target),
            Err(ModelError::BindingMismatch)
        );
        let mut other_resource = refused_snapshot();
        other_resource.resource.resource_id = "resource-other".to_string();
        assert_eq!(
            model.apply_snapshot(other_resource),
            Err(ModelError::ResourceMismatch)
        );
        let mut stale = refused_snapshot();
        stale.resource.revision = 1;
        assert_eq!(model.apply_snapshot(stale), Err(ModelError::StaleRevision));
        let mut invalid = refused_snapshot();
        invalid.resource.kind = PreviewKind::Artifact;
        assert!(matches!(
            model.apply_snapshot(invalid),
            Err(ModelError::InvalidResource(_))
        ));
        assert_eq!(model, live_model());
    }

    #[test]
    fn refused_resource_cannot_be_bound_directly() {
        let mut refused = resource();
        refused.redaction = RedactionStatus::Refused;
        assert_eq!(
            BoundBrowser::from_resource(refused.clone()),
            Err(ModelError::Refused)
        );
        let mut snap = snapshot();
        snap.resource = refused;
        assert_eq!(BoundBrowser::from_snapshot(snap), Err(ModelError::Refused));
        let mut bound = BoundBrowser::from_snapshot(snapshot()).unwrap();
        let before = bound.clone();
        assert_eq!(
            bound.apply_snapshot(refused_snapshot()),
            Err(ModelError::Refused)
        );
        assert_eq!(bound, before);
    }

    #[test]
    fn closed_tombstone_blocks_replay_of_same_or_older_revision() {
        let mut model = live_model();
        model
            .apply_closed(&closed_notice(ClosedReason::RunEnded))
            .unwrap();
        let closed = model.clone();

        // Same revision and older revision of the closed resource: no revival.
        assert_eq!(
            model.bind_resource(resource()),
            Err(ModelError::AlreadyClosed)
        );
        let mut older = resource();
        older.revision = 1;
        assert_eq!(model.bind_resource(older), Err(ModelError::StaleRevision));
        assert_eq!(model, closed);

        // A mismatched close is not a tombstone.
        let mut open = live_model();
        let mut wrong = closed_notice(ClosedReason::RunEnded);
        wrong.resource_id = "resource-other".to_string();
        assert!(open.apply_closed(&wrong).is_err());
        assert!(open.bound().is_some());

        // A different resource binds; so does a newer revision of the same id.
        let mut other = closed.clone();
        let mut different = resource();
        different.resource_id = "resource-6".to_string();
        different.revision = 1;
        other.bind_resource(different).unwrap();
        assert_eq!(other.bound().unwrap().resource().resource_id, "resource-6");

        let mut newer = closed;
        let mut next = resource();
        next.revision = 3;
        newer.bind_resource(next).unwrap();
        assert_eq!(
            newer.bound().unwrap().phase(),
            ViewerPhase::AwaitingAuthorization
        );
    }

    #[test]
    fn closed_phase_bound_value_also_blocks_same_revision() {
        let mut bound = BoundBrowser::from_snapshot(snapshot()).unwrap();
        bound
            .apply_closed(&closed_notice(ClosedReason::Expired))
            .unwrap();
        let mut model = BrowserViewerModel::Bound(Box::new(bound));
        assert_eq!(
            model.bind_resource(resource()),
            Err(ModelError::AlreadyClosed)
        );
        assert_eq!(
            model.apply_snapshot(snapshot()),
            Err(ModelError::AlreadyClosed)
        );
    }

    #[test]
    fn selection_persists_through_newer_same_resource_bind_and_snapshot() {
        let mut model = live_model();
        model.select_candidate("b").unwrap();

        let mut newer_resource = resource();
        newer_resource.revision = 3;
        model.bind_resource(newer_resource).unwrap();
        assert_eq!(model.bound().unwrap().selected_candidate().unwrap().id, "b");

        let mut newer_snapshot = snapshot();
        newer_snapshot.resource.revision = 4;
        model.apply_snapshot(newer_snapshot).unwrap();
        assert_eq!(model.bound().unwrap().selected_candidate().unwrap().id, "b");
        assert_eq!(model.bound().unwrap().resource().revision, 4);
    }

    #[test]
    fn snapshot_and_state_share_the_controller_epoch_rule() {
        let human = Controller::Human {
            principal: IssuerIdentity::User {
                user_id: "user-42".to_string(),
            },
            viewer_session_id: "vs-1".to_string(),
        };
        let mut model = live_model();
        let mut snap = snapshot();
        snap.controller = human.clone();
        assert_eq!(model.apply_snapshot(snap), Err(ModelError::EpochRegression));
        let mut st = state();
        st.controller = human.clone();
        assert_eq!(model.apply_state(&st), Err(ModelError::EpochRegression));

        let mut snap = snapshot();
        snap.controller = human;
        snap.control_epoch += 1;
        model.apply_snapshot(snap).unwrap();
        assert_eq!(model.bound().unwrap().live().unwrap().control_epoch, 20);
    }

    #[test]
    fn retain_for_session_drops_foreign_bindings() {
        let mut model = live_model();
        model.retain_for_session("agent:operator:browser");
        assert!(model.bound().is_some());
        model.retain_for_session("agent:other:chat");
        assert_eq!(model, BrowserViewerModel::default());
    }

    #[test]
    fn auto_reveal_requires_every_condition() {
        let model = live_model();
        assert!(model.auto_reveal(&task(), 1_000, false));

        // Replay, expiry boundary, foreground session / run.
        assert!(!model.auto_reveal(&task(), 1_000, true));
        assert!(!model.auto_reveal(&task(), 2_000, false));
        let other_run = ConversationBinding {
            run_id: "run-8".to_string(),
            ..task()
        };
        assert!(!model.auto_reveal(&other_run, 1_000, false));
        let other_session = ConversationBinding {
            session_key: "agent:other:chat".to_string(),
            ..task()
        };
        assert!(!model.auto_reveal(&other_session, 1_000, false));

        // Muted.
        let mut muted = live_model();
        if let BrowserViewerModel::Bound(bound) = &mut muted {
            bound.set_muted(true);
        }
        assert!(!muted.auto_reveal(&task(), 1_000, false));

        // Explicit-only display policy.
        let mut snap = snapshot();
        snap.resource.display = DisplayPolicy::ExplicitOnly;
        let explicit = BrowserViewerModel::from_snapshot(snap).unwrap();
        assert!(!explicit.auto_reveal(&task(), 1_000, false));

        // Pending redaction is not shown automatically.
        let mut pending = snapshot();
        pending.resource.redaction = RedactionStatus::Pending;
        let pending = BrowserViewerModel::from_snapshot(pending).unwrap();
        assert!(!pending.auto_reveal(&task(), 1_000, false));
    }

    #[test]
    fn workspace_has_browser_only_when_bound() {
        let ws = BrowserWorkspace::new();
        assert!(!ws.has_browser_untracked());
        ws.model.set(live_model());
        assert!(ws.has_browser_untracked());
    }

    impl BrowserWorkspace {
        fn has_browser_untracked(&self) -> bool {
            self.model.read_untracked().bound().is_some()
        }
    }
}
