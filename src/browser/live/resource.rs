//! Server-side registry of browser-live [`PreviewResource`]s.
//!
//! This is the single authority for `resource_id -> current PreviewResource`
//! plus its live / closed state. Everything else (grant issuance, snapshots,
//! gateway events) must ask here instead of keeping a second copy.
//!
//! Invariants:
//!
//! * A resource is always bound to an exact, caller-supplied
//!   [`ViewerBinding`] (principal + task + browser target). The registry never
//!   infers a target, profile, or "current tab"; empty axes, artifact kinds /
//!   sources, and `Refused` redaction are rejected (fail closed).
//! * Resource ids are opaque 256-bit CSPRNG values (`res_` + base64url). They
//!   embed no URL, title, target, or secret, and the stored
//!   [`PreviewResource`] has no URL / secret field to leak.
//! * One live resource per binding. Publishing the same binding again is a new
//!   *revision* of the same resource id; revisions only ever increase. Source
//!   and provenance are immutable across revisions (rediscovery never
//!   retro-attests provenance).
//! * Expiry is an absolute `expires_at_ms` taken from a single clock. An
//!   expired resource is never returned as active, even before [`sweep`]
//!   transitions it to a tombstone.
//! * Closing is a one-way live -> closed transition that yields the exact
//!   [`BrowserLiveClosed`] event once; closed resources stay as bounded,
//!   time-limited tombstones and are never resurrected under the same id.
//!
//! The registry does NOT own grants. Whoever closes or sweeps a resource must
//! also call `ViewerGrantStore::revoke_resource` for each returned
//! [`BrowserLiveClosed`]; keeping the two stores decoupled avoids a hidden
//! lock order between them. Production callers go through
//! `lifecycle::BrowserLiveLifecycle`, which composes registry transition,
//! grant revocation and the typed event in one place.
//!
//! [`sweep`]: ResourceRegistry::sweep

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use aleph_protocol::browser_live::{
    BrowserLiveClosed, BrowserLiveValidationError, BrowserTargetBinding, BusEvent, ClosedReason,
    CompanionArrangement, CompanionCandidate, CompanionLayout, DisplayPolicy, IssuerIdentity,
    PreviewKind, PreviewProvenance, PreviewResource, PreviewSource, RedactionStatus,
    ResourceBinding, ResourceDecision, ViewerBinding,
};

use super::grant::mint_secret;
use super::navigation_gate::BoundaryScope;

/// Hard bound on stored records (live + tombstones).
const MAX_RECORDS: usize = 1024;
/// Hard bound on live (not yet closed) resources. Overflow fails closed; a
/// live resource is never evicted silently because its close event and grant
/// revocation would be lost.
const MAX_LIVE_RESOURCES: usize = 256;
/// How long a closed resource stays recognizable.
const TOMBSTONE_RETENTION_MS: i64 = 10 * 60 * 1000;
/// Upper bound for a resource's lifetime from (re)publication.
const MAX_RESOURCE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// Bounds on producer-supplied companion metadata.
const MAX_CANDIDATES: usize = 16;
const MAX_SCREENS_PER_CANDIDATE: usize = 8;
const MAX_TEXT_BYTES: usize = 256;
/// Largest plausible CSS viewport edge of a companion screen.
const MAX_VIEWPORT_CSS_PX: u32 = 16_384;
/// Longest companion animation hint.
const MAX_ANIMATION_MS: u64 = 60_000;

fn tombstone_alive(closed: &BrowserLiveClosed, now: i64) -> bool {
    closed.at_ms.saturating_add(TOMBSTONE_RETENTION_MS) > now
}

fn wall_clock_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Single time source of a registry. Production uses wall-clock millis; tests
/// inject a controllable one.
type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Why a registry operation was refused. Server-internal; the only payload is
/// the protocol validation enum, which carries no producer-supplied strings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ResourceError {
    /// A binding axis (principal / session / run / profile / browser session /
    /// target) is empty or oversized.
    InvalidBinding,
    /// Artifact kind or source: artifacts are `NotManaged` metadata and never
    /// enter the browser-viewer registry.
    ArtifactNotPublishable,
    /// `RedactionStatus::Refused` is never published. If the draft targeted a
    /// resource that was live, that resource was closed in the same critical
    /// section (the older, safe revision must not stay active under a refusal)
    /// and `closed` is the exact event the caller must emit and revoke grants
    /// for ([`ResourceError::closed_event`]).
    Refused {
        // Boxed: the close event is large and rare; this keeps every
        // `Result<_, ResourceError>` small (clippy::result_large_err).
        closed: Option<Box<BrowserLiveClosed>>,
    },
    /// TTL or companion metadata out of bounds / inconsistent.
    InvalidDraft,
    /// The wire-level `PreviewResource::validate` rejected the result.
    Validation(BrowserLiveValidationError),
    UnknownResource,
    ResourceClosed,
    /// The live resource's `expires_at_ms` passed but it has not been swept
    /// yet. Call [`ResourceRegistry::sweep`] (emit + revoke) and retry.
    Expired,
    StaleRevision,
    /// The draft's binding differs from the resource's bound binding.
    BindingMismatch,
    /// Source or provenance differs from the live resource's.
    IdentityChanged,
    CapacityExceeded,
    RevisionExhausted,
    /// A user decision targets only a `Pending` revision; `Safe`/`Redacted`
    /// need no confirmation and `Refused` is never stored.
    NotPending,
}

impl fmt::Display for ResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidBinding => "invalid resource binding",
            Self::ArtifactNotPublishable => "artifact resources are not browser-viewer resources",
            Self::Refused { .. } => "refused resources are not published",
            Self::InvalidDraft => "invalid resource draft",
            Self::Validation(_) => "resource failed wire validation",
            Self::UnknownResource => "unknown resource",
            Self::ResourceClosed => "resource is closed",
            Self::Expired => "resource expired; sweep required",
            Self::StaleRevision => "stale resource revision",
            Self::BindingMismatch => "resource binding mismatch",
            Self::IdentityChanged => "resource source/provenance cannot change",
            Self::CapacityExceeded => "resource registry full",
            Self::RevisionExhausted => "resource revision exhausted",
            Self::NotPending => "resource revision is not pending",
        })
    }
}

impl ResourceError {
    /// The close event a refusal produced, if it closed a live resource. Must
    /// be handled like any [`ResourceRegistry::close`] result.
    pub(crate) fn closed_event(&self) -> Option<&BrowserLiveClosed> {
        match self {
            Self::Refused { closed } => closed.as_deref(),
            _ => None,
        }
    }
}

impl std::error::Error for ResourceError {}

/// Producer input. Everything the registry derives itself (resource id,
/// revision, absolute expiry, `ResourceBinding::Browser` wrapping) is absent.
#[derive(Clone, Debug)]
pub(crate) struct ResourceDraft {
    /// Exact binding of the real tool operation. Browser-only by type.
    pub binding: ViewerBinding,
    pub kind: PreviewKind,
    pub display: DisplayPolicy,
    pub source: PreviewSource,
    pub provenance: PreviewProvenance,
    pub redaction: RedactionStatus,
    /// Lifetime from (re)publication; becomes an absolute `expires_at_ms`.
    pub ttl: Duration,
    pub companion_candidates: Vec<CompanionCandidate>,
}

enum Slot {
    Live(PreviewResource),
    Closed(BrowserLiveClosed),
}

#[derive(Default)]
struct Inner {
    slots: HashMap<String, Slot>,
}

impl Inner {
    fn live_count(&self) -> usize {
        self.slots
            .values()
            .filter(|s| matches!(s, Slot::Live(_)))
            .count()
    }

    fn live_id_for(&self, binding: &ViewerBinding) -> Option<String> {
        self.slots.iter().find_map(|(id, slot)| match slot {
            Slot::Live(r) if r.viewer_binding() == Some(binding) => Some(id.clone()),
            _ => None,
        })
    }

    fn prune_tombstones(&mut self, now: i64) {
        self.slots.retain(|_, slot| match slot {
            Slot::Closed(c) => tombstone_alive(c, now),
            Slot::Live(_) => true,
        });
    }

    /// Make room for one more record by dropping the oldest tombstone.
    fn make_room(&mut self) -> Result<(), ResourceError> {
        if self.slots.len() < MAX_RECORDS {
            return Ok(());
        }
        let victim = self
            .slots
            .iter()
            .filter_map(|(id, slot)| match slot {
                Slot::Closed(c) => Some((c.at_ms, id.clone())),
                Slot::Live(_) => None,
            })
            .min()
            .map(|(_, id)| id)
            .ok_or(ResourceError::CapacityExceeded)?;
        self.slots.remove(&victim);
        Ok(())
    }

    /// Fail-closed transition for a refused redaction verdict: the live
    /// resource stops being active in the same critical section as the refusal.
    fn close_refused(&mut self, resource_id: &str, now: i64) -> Option<BrowserLiveClosed> {
        let reason = match self.slots.get(resource_id) {
            Some(Slot::Live(r)) if r.expires_at_ms <= now => ClosedReason::Expired,
            _ => ClosedReason::Revoked,
        };
        self.close(resource_id, reason, now)
    }

    fn close(
        &mut self,
        resource_id: &str,
        reason: ClosedReason,
        now: i64,
    ) -> Option<BrowserLiveClosed> {
        let slot = self.slots.get_mut(resource_id)?;
        let Slot::Live(resource) = slot else {
            return None;
        };
        let closed = BrowserLiveClosed {
            resource_id: resource_id.to_string(),
            binding: resource.binding.clone(),
            reason,
            at_ms: now,
        };
        *slot = Slot::Closed(closed.clone());
        Some(closed)
    }
}

/// What a document boundary did to the registry.
#[derive(Debug, Default)]
pub(crate) struct NavigationDemotion {
    /// Resources now `Pending` at a new revision.
    pub(crate) demoted: Vec<PreviewResource>,
    /// Resources that could not be demoted and were closed instead.
    pub(crate) closed: Vec<BrowserLiveClosed>,
}

/// The registry. Clone is cheap (one shared table).
#[derive(Clone)]
pub(crate) struct ResourceRegistry {
    inner: Arc<Mutex<Inner>>,
    clock: Clock,
}

impl Default for ResourceRegistry {
    fn default() -> Self {
        Self {
            inner: Arc::default(),
            clock: Arc::new(wall_clock_ms),
        }
    }
}

impl ResourceRegistry {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[cfg(test)]
    pub(crate) fn with_clock(clock: Clock) -> Self {
        Self {
            inner: Arc::default(),
            clock,
        }
    }

    /// Publish `draft` for its exact binding.
    ///
    /// A `Refused` draft is an error AND closes the binding's live resource
    /// (see [`ResourceError::Refused`]); it is never left active.
    ///
    /// No live resource for the binding: mints a new opaque id at revision 1.
    /// A live resource for the same binding: bumps that resource's revision
    /// (same id). Returns the stored resource; the caller emits it as
    /// `BusEvent::Resource`.
    pub(crate) fn publish(&self, draft: ResourceDraft) -> Result<PreviewResource, ResourceError> {
        validate_scope(&draft)?;
        let now = (self.clock)();
        let mut inner = self.lock();
        if draft.redaction == RedactionStatus::Refused {
            let closed = inner
                .live_id_for(&draft.binding)
                .and_then(|id| inner.close_refused(&id, now));
            return Err(ResourceError::Refused {
                closed: closed.map(Box::new),
            });
        }
        validate_content(&draft)?;
        if let Some(id) = inner.live_id_for(&draft.binding) {
            return revise(&mut inner, now, &id, None, draft);
        }
        inner.prune_tombstones(now);
        if inner.live_count() >= MAX_LIVE_RESOURCES {
            return Err(ResourceError::CapacityExceeded);
        }
        inner.make_room()?;
        let id = loop {
            let candidate = format!("res_{}", mint_secret());
            if !inner.slots.contains_key(&candidate) {
                break candidate;
            }
        };
        let resource = build(id.clone(), 1, draft, now)?;
        inner.slots.insert(id, Slot::Live(resource.clone()));
        Ok(resource)
    }

    /// Compare-and-set revision of an existing live resource. A caller that
    /// built its draft from revision `expected_revision` loses
    /// ([`ResourceError::StaleRevision`]) if another revision landed first.
    ///
    /// A `Refused` draft fails closed: see [`ResourceError::Refused`].
    pub(crate) fn republish(
        &self,
        resource_id: &str,
        expected_revision: u64,
        draft: ResourceDraft,
    ) -> Result<PreviewResource, ResourceError> {
        validate_scope(&draft)?;
        let now = (self.clock)();
        let mut inner = self.lock();
        if draft.redaction == RedactionStatus::Refused {
            // Only the resource's own binding may refuse it.
            let owned = matches!(
                inner.slots.get(resource_id),
                Some(Slot::Live(r)) if r.viewer_binding() == Some(&draft.binding)
            );
            let closed = owned
                .then(|| inner.close_refused(resource_id, now))
                .flatten();
            return Err(ResourceError::Refused {
                closed: closed.map(Box::new),
            });
        }
        validate_content(&draft)?;
        revise(&mut inner, now, resource_id, Some(expected_revision), draft)
    }

    /// The current resource iff it is live and unexpired.
    pub(crate) fn get_active(&self, resource_id: &str) -> Option<PreviewResource> {
        let now = (self.clock)();
        match self.lock().slots.get(resource_id)? {
            Slot::Live(r) if r.expires_at_ms > now => Some(r.clone()),
            _ => None,
        }
    }

    /// [`get_active`](Self::get_active) restricted to the resource's bound
    /// principal. Any other caller gets the same `None` as for an unknown id.
    pub(crate) fn get_active_for(
        &self,
        resource_id: &str,
        owner: &IssuerIdentity,
    ) -> Option<PreviewResource> {
        self.get_active(resource_id)
            .filter(|r| r.viewer_binding().is_some_and(|b| &b.principal == owner))
    }

    /// Every live, unexpired resource bound to `owner`, ordered by
    /// `resource_id`. Same liveness and ownership predicate as
    /// [`get_active_for`](Self::get_active_for), applied to the whole registry
    /// under one lock, so the snapshot is internally consistent. Closed,
    /// expired, unbound and foreign resources are absent.
    pub(crate) fn list_active_for(&self, owner: &IssuerIdentity) -> Vec<PreviewResource> {
        let now = (self.clock)();
        let mut out: Vec<PreviewResource> = self
            .lock()
            .slots
            .values()
            .filter_map(|slot| match slot {
                Slot::Live(r)
                    if r.expires_at_ms > now
                        && r.viewer_binding().is_some_and(|b| &b.principal == owner) =>
                {
                    Some(r.clone())
                }
                _ => None,
            })
            .collect();
        out.sort_by(|a, b| a.resource_id.cmp(&b.resource_id));
        out
    }

    /// The close tombstone of a closed resource still within retention.
    pub(crate) fn closed(&self, resource_id: &str) -> Option<BrowserLiveClosed> {
        let now = (self.clock)();
        match self.lock().slots.get(resource_id)? {
            Slot::Closed(c) if tombstone_alive(c, now) => Some(c.clone()),
            _ => None,
        }
    }

    /// Close a live resource. `Some` exactly once (the transition) with the
    /// exact event to emit; `None` for unknown or already-closed ids.
    pub(crate) fn close(
        &self,
        resource_id: &str,
        reason: ClosedReason,
    ) -> Option<BrowserLiveClosed> {
        let now = (self.clock)();
        self.lock().close(resource_id, reason, now)
    }

    /// Close the live resource bound to exactly `binding` (e.g. its target
    /// closed or its engine exited, events that know the binding, not the id).
    pub(crate) fn close_binding(
        &self,
        binding: &ViewerBinding,
        reason: ClosedReason,
    ) -> Option<BrowserLiveClosed> {
        let now = (self.clock)();
        let mut inner = self.lock();
        let id = inner.live_id_for(binding)?;
        inner.close(&id, reason, now)
    }

    /// Close every live Browser resource bound to exactly this
    /// `(profile, browser_session_id, target_id)` (the engine target was
    /// destroyed). One lock scan; Artifact resources never match. Returns the
    /// exact close events, in no particular order; the caller (the lifecycle)
    /// must revoke each resource's grants and announce it.
    pub(crate) fn close_target(
        &self,
        profile: &str,
        browser_session_id: &str,
        target_id: &str,
        reason: ClosedReason,
    ) -> Vec<BrowserLiveClosed> {
        self.close_matching(reason, |target| {
            target.profile == profile
                && target.browser_session_id == browser_session_id
                && target.target_id == target_id
        })
    }

    /// As [`Self::close_target`], for every target of the exact
    /// `(profile, browser_session_id)` (the owner ended).
    pub(crate) fn close_browser_session(
        &self,
        profile: &str,
        browser_session_id: &str,
        reason: ClosedReason,
    ) -> Vec<BrowserLiveClosed> {
        self.close_matching(reason, |target| {
            target.profile == profile && target.browser_session_id == browser_session_id
        })
    }

    fn close_matching(
        &self,
        reason: ClosedReason,
        matches: impl Fn(&BrowserTargetBinding) -> bool,
    ) -> Vec<BrowserLiveClosed> {
        let now = (self.clock)();
        let mut inner = self.lock();
        let ids: Vec<String> = inner
            .slots
            .iter()
            .filter_map(|(id, slot)| match slot {
                Slot::Live(resource) => resource
                    .viewer_binding()
                    .filter(|viewer| matches(&viewer.target))
                    .map(|_| id.clone()),
                Slot::Closed(_) => None,
            })
            .collect();
        ids.iter()
            .filter_map(|id| inner.close(id, reason, now))
            .collect()
    }

    /// A document boundary passed under `(profile, browser_session_id)`:
    /// every live Browser resource bound to exactly that session (and, for
    /// [`BoundaryScope::Target`], exactly that target id) loses whatever
    /// approval it had. Revision + 1 and `redaction = Pending`, even when it
    /// already was `Pending`: an owner `decide(Safe, R)` that was composed
    /// against the previous page's revision must fail as `StaleRevision`.
    /// Binding, expiry, kind, source and provenance are untouched, so the
    /// producer's next `republish` still addresses the same resource id.
    ///
    /// Nothing is inferred: no URL, hash, current tab or profile-wide guess
    /// takes part in the match. A resource whose revision counter is exhausted
    /// is closed (`Revoked`) instead; there is no way to fence it otherwise.
    ///
    /// The caller (the lifecycle) must revoke each returned resource's grants
    /// and announce the result; the registry never calls the grant store.
    pub(crate) fn demote_for_navigation(
        &self,
        profile: &str,
        browser_session_id: &str,
        scope: &BoundaryScope,
    ) -> NavigationDemotion {
        let now = (self.clock)();
        let mut inner = self.lock();
        let matching: Vec<String> = inner
            .slots
            .iter()
            .filter_map(|(id, slot)| {
                let Slot::Live(resource) = slot else {
                    return None;
                };
                let target = &resource.viewer_binding()?.target;
                let in_scope = target.profile == profile
                    && target.browser_session_id == browser_session_id
                    && match scope {
                        BoundaryScope::Target(target_id) => &target.target_id == target_id,
                        BoundaryScope::AllTargets => true,
                    };
                in_scope.then(|| id.clone())
            })
            .collect();
        let mut out = NavigationDemotion::default();
        for id in matching {
            let Some(Slot::Live(current)) = inner.slots.get(&id) else {
                continue;
            };
            let mut next = current.clone();
            next.redaction = RedactionStatus::Pending;
            let valid = next.revision.checked_add(1).is_some_and(|revision| {
                next.revision = revision;
                next.validate().is_ok()
            });
            if valid {
                inner.slots.insert(id, Slot::Live(next.clone()));
                out.demoted.push(next);
            } else if let Some(closed) = inner.close(&id, ClosedReason::Revoked, now) {
                out.closed.push(closed);
            }
        }
        out
    }

    /// Atomic owner-scoped user decision on the exact current revision of a
    /// `Pending` browser resource. Everything is checked and applied under one
    /// registry lock, in this order, so no rejected call changes any state:
    /// live + owner (`User` principal match on a Browser binding; Artifact,
    /// Connection and foreign callers are indistinguishable from an unknown id)
    /// -> not expired -> exact revision -> `Pending`.
    ///
    /// * `Safe`: revision + 1 and `redaction = Safe`, nothing else. The absolute
    ///   `expires_at_ms`, kind, source, provenance and binding are untouched:
    ///   the user's confirmation never extends the resource's lifetime.
    /// * `Refused`: closes the resource with the same `ClosedReason::Revoked`
    ///   the refused-publish path uses (the user's verdict is the request
    ///   decision, not a new wire reason) and leaves a tombstone.
    ///
    /// Returns the canonical bus event to announce. The caller must revoke the
    /// resource's grants (`lifecycle::BrowserLiveLifecycle::decide` does).
    /// `ResourceError::Expired` means the resource is still live in the table:
    /// sweep it (emit + revoke) like any other expired refusal.
    pub(crate) fn decide(
        &self,
        resource_id: &str,
        owner: &IssuerIdentity,
        expected_revision: u64,
        decision: ResourceDecision,
    ) -> Result<BusEvent, ResourceError> {
        let now = (self.clock)();
        let mut inner = self.lock();
        let Some(Slot::Live(current)) = inner.slots.get(resource_id) else {
            return Err(ResourceError::UnknownResource);
        };
        // Ownership first: a non-owner learns nothing about expiry/revision.
        if !current
            .viewer_binding()
            .is_some_and(|binding| &binding.principal == owner)
        {
            return Err(ResourceError::UnknownResource);
        }
        if current.expires_at_ms <= now {
            return Err(ResourceError::Expired);
        }
        if current.revision != expected_revision {
            return Err(ResourceError::StaleRevision);
        }
        if current.redaction != RedactionStatus::Pending {
            return Err(ResourceError::NotPending);
        }
        match decision {
            ResourceDecision::Safe => {
                let mut next = current.clone();
                next.revision = next
                    .revision
                    .checked_add(1)
                    .ok_or(ResourceError::RevisionExhausted)?;
                next.redaction = RedactionStatus::Safe;
                // Validate before storing so a failure leaves the slot as-is.
                next.validate().map_err(ResourceError::Validation)?;
                inner
                    .slots
                    .insert(resource_id.to_string(), Slot::Live(next.clone()));
                Ok(BusEvent::Resource { resource: next })
            }
            ResourceDecision::Refused => inner
                .close(resource_id, ClosedReason::Revoked, now)
                .map(|closed| BusEvent::Closed { closed })
                .ok_or(ResourceError::UnknownResource),
        }
    }

    /// Transition every expired live resource to a tombstone
    /// (`ClosedReason::Expired`) and drop aged tombstones. Returns the events
    /// to emit; the caller must also revoke each resource's grants.
    pub(crate) fn sweep(&self) -> Vec<BrowserLiveClosed> {
        let now = (self.clock)();
        let mut inner = self.lock();
        let expired: Vec<String> = inner
            .slots
            .iter()
            .filter_map(|(id, slot)| match slot {
                Slot::Live(r) if r.expires_at_ms <= now => Some(id.clone()),
                _ => None,
            })
            .collect();
        let closed = expired
            .iter()
            .filter_map(|id| inner.close(id, ClosedReason::Expired, now))
            .collect();
        inner.prune_tombstones(now);
        closed
    }
}

/// Apply `draft` as the next revision of live resource `id`.
fn revise(
    inner: &mut Inner,
    now: i64,
    id: &str,
    expected_revision: Option<u64>,
    draft: ResourceDraft,
) -> Result<PreviewResource, ResourceError> {
    let slot = inner
        .slots
        .get_mut(id)
        .ok_or(ResourceError::UnknownResource)?;
    let Slot::Live(current) = slot else {
        return Err(match slot {
            Slot::Closed(c) if !tombstone_alive(c, now) => ResourceError::UnknownResource,
            _ => ResourceError::ResourceClosed,
        });
    };
    if current.expires_at_ms <= now {
        return Err(ResourceError::Expired);
    }
    if current.viewer_binding() != Some(&draft.binding) {
        return Err(ResourceError::BindingMismatch);
    }
    if expected_revision.is_some_and(|expected| expected != current.revision) {
        return Err(ResourceError::StaleRevision);
    }
    if current.source != draft.source || current.provenance != draft.provenance {
        return Err(ResourceError::IdentityChanged);
    }
    let revision = current
        .revision
        .checked_add(1)
        .ok_or(ResourceError::RevisionExhausted)?;
    let next = build(id.to_string(), revision, draft, now)?;
    *current = next.clone();
    Ok(next)
}

fn build(
    resource_id: String,
    revision: u64,
    draft: ResourceDraft,
    now: i64,
) -> Result<PreviewResource, ResourceError> {
    let ttl_ms = i64::try_from(draft.ttl.as_millis()).map_err(|_| ResourceError::InvalidDraft)?;
    let resource = PreviewResource {
        resource_id,
        revision,
        binding: ResourceBinding::Browser {
            viewer: draft.binding,
        },
        kind: draft.kind,
        display: draft.display,
        source: draft.source,
        provenance: draft.provenance,
        expires_at_ms: now.saturating_add(ttl_ms),
        redaction: draft.redaction,
        companion_candidates: draft.companion_candidates,
    };
    resource.validate().map_err(ResourceError::Validation)?;
    Ok(resource)
}

fn text_ok(s: &str) -> bool {
    !s.trim().is_empty() && s.len() <= MAX_TEXT_BYTES
}

fn identity_ok(identity: &IssuerIdentity) -> bool {
    match identity {
        IssuerIdentity::User { user_id } => text_ok(user_id),
        IssuerIdentity::Connection { connection_id } => text_ok(connection_id),
    }
}

/// Binding and kind/source checks that must hold before the draft may even
/// refer to (or close) an existing resource.
fn validate_scope(draft: &ResourceDraft) -> Result<(), ResourceError> {
    let b = &draft.binding;
    let axes = [
        &b.task.session_key,
        &b.task.run_id,
        &b.target.profile,
        &b.target.browser_session_id,
        &b.target.target_id,
    ];
    if !identity_ok(&b.principal) || !axes.iter().all(|a| text_ok(a)) {
        return Err(ResourceError::InvalidBinding);
    }
    if draft.kind == PreviewKind::Artifact || draft.source == PreviewSource::ArtifactAdapter {
        return Err(ResourceError::ArtifactNotPublishable);
    }
    Ok(())
}

/// Content checks for a draft that is going to be stored.
fn validate_content(draft: &ResourceDraft) -> Result<(), ResourceError> {
    if draft.ttl.is_zero() || draft.ttl > MAX_RESOURCE_TTL {
        return Err(ResourceError::InvalidDraft);
    }
    // Managed provenance <-> managed source, both directions: a browser-tool
    // resource cannot claim a launched process, and a managed one must say
    // which process it is.
    match (&draft.source, &draft.provenance) {
        (PreviewSource::BrowserTool, PreviewProvenance::NotManaged) => {}
        (
            PreviewSource::ManagedTool,
            PreviewProvenance::Managed {
                launcher,
                process_id,
                launched_at_ms,
            },
        ) if text_ok(launcher) && *process_id > 0 && *launched_at_ms > 0 => {}
        _ => return Err(ResourceError::InvalidDraft),
    }
    validate_candidates(draft.kind, &draft.companion_candidates)
}

fn validate_candidates(
    kind: PreviewKind,
    candidates: &[CompanionCandidate],
) -> Result<(), ResourceError> {
    if candidates.is_empty() {
        return Ok(());
    }
    // Candidates are design-companion state/layout hints; no other kind
    // carries them. (The protocol's round-trip fixture puts one on a Page, but
    // that only exercises serde, not publishing rules.)
    if kind != PreviewKind::DesignCompanion || candidates.len() > MAX_CANDIDATES {
        return Err(ResourceError::InvalidDraft);
    }
    let mut seen = std::collections::HashSet::new();
    for c in candidates {
        if !text_ok(&c.id) || !text_ok(&c.label) || !seen.insert(c.id.as_str()) {
            return Err(ResourceError::InvalidDraft);
        }
        validate_layout(&c.layout)?;
        if let Some(active) = &c.state.active_screen_id {
            if !c.layout.screens.iter().any(|s| &s.id == active) {
                return Err(ResourceError::InvalidDraft);
            }
        }
        if let Some(a) = &c.state.animation {
            if !text_ok(&a.name) || a.duration_ms > MAX_ANIMATION_MS {
                return Err(ResourceError::InvalidDraft);
            }
        }
    }
    Ok(())
}

fn validate_layout(layout: &CompanionLayout) -> Result<(), ResourceError> {
    let n = layout.screens.len();
    let count_ok = match layout.arrangement {
        CompanionArrangement::Single => n == 1,
        CompanionArrangement::SideBySide => n == 2,
        CompanionArrangement::HorizontalScreens => (2..=MAX_SCREENS_PER_CANDIDATE).contains(&n),
    };
    let mut ids = std::collections::HashSet::new();
    let screens_ok = layout.screens.iter().all(|s| {
        let v = &s.viewport_css_px;
        text_ok(&s.id)
            && text_ok(&s.label)
            && ids.insert(s.id.as_str())
            && (1..=MAX_VIEWPORT_CSS_PX).contains(&v.width)
            && (1..=MAX_VIEWPORT_CSS_PX).contains(&v.height)
    });
    if count_ok && screens_ok {
        Ok(())
    } else {
        Err(ResourceError::InvalidDraft)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicI64, Ordering};

    use aleph_protocol::browser_live::{
        AnimationPreview, BrowserTargetBinding, CompanionArrangement, CompanionLayout,
        CompanionScreen, CompanionStateHint, ConversationBinding, ViewportCssPx,
    };
    use serde_json::Value;

    use super::*;

    const T0: i64 = 1_000_000;

    fn registry() -> (ResourceRegistry, Arc<AtomicI64>) {
        let now = Arc::new(AtomicI64::new(T0));
        let clock_now = now.clone();
        (
            ResourceRegistry::with_clock(Arc::new(move || clock_now.load(Ordering::SeqCst))),
            now,
        )
    }

    fn owner() -> IssuerIdentity {
        IssuerIdentity::User {
            user_id: "user-1".into(),
        }
    }

    fn binding(target_id: &str) -> ViewerBinding {
        ViewerBinding {
            principal: owner(),
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

    fn draft(target_id: &str) -> ResourceDraft {
        ResourceDraft {
            binding: binding(target_id),
            kind: PreviewKind::Page,
            display: DisplayPolicy::AutoRevealCurrentRun,
            source: PreviewSource::BrowserTool,
            provenance: PreviewProvenance::NotManaged,
            redaction: RedactionStatus::Safe,
            ttl: Duration::from_secs(600),
            companion_candidates: Vec::new(),
        }
    }

    fn managed_draft(target_id: &str) -> ResourceDraft {
        let mut d = draft(target_id);
        d.kind = PreviewKind::DevPreview;
        d.source = PreviewSource::ManagedTool;
        d.provenance = PreviewProvenance::Managed {
            launcher: "present".into(),
            process_id: 4242,
            launched_at_ms: T0,
        };
        d
    }

    fn companion_draft(target_id: &str, candidates: Vec<CompanionCandidate>) -> ResourceDraft {
        let mut d = draft(target_id);
        d.kind = PreviewKind::DesignCompanion;
        d.companion_candidates = candidates;
        d
    }

    fn candidate(id: &str) -> CompanionCandidate {
        CompanionCandidate {
            id: id.into(),
            label: "Mobile".into(),
            layout: CompanionLayout {
                arrangement: CompanionArrangement::Single,
                screens: vec![CompanionScreen {
                    id: "s1".into(),
                    label: "Phone".into(),
                    viewport_css_px: ViewportCssPx {
                        width: 390,
                        height: 844,
                    },
                }],
            },
            state: CompanionStateHint {
                active_screen_id: Some("s1".into()),
                animation: Some(AnimationPreview {
                    name: "fade".into(),
                    duration_ms: 200,
                    looped: false,
                }),
                prefers_reduced_motion: false,
            },
        }
    }

    #[test]
    fn revisions_are_monotonic_per_resource_and_stale_cas_loses() {
        let (reg, _) = registry();
        let r1 = reg.publish(draft("t1")).expect("first");
        assert_eq!((r1.revision, r1.expires_at_ms), (1, T0 + 600_000));
        assert!(r1.resource_id.starts_with("res_"));

        // Same binding => same id, next revision.
        let r2 = reg.publish(draft("t1")).expect("second");
        assert_eq!(
            (r2.resource_id.as_str(), r2.revision),
            (r1.resource_id.as_str(), 2)
        );
        // CAS: building on revision 2 wins, building on revision 1 is stale.
        let r3 = reg.republish(&r1.resource_id, 2, draft("t1")).expect("cas");
        assert_eq!(r3.revision, 3);
        assert_eq!(
            reg.republish(&r1.resource_id, 1, draft("t1")).unwrap_err(),
            ResourceError::StaleRevision
        );
        assert_eq!(reg.get_active(&r1.resource_id).unwrap().revision, 3);

        // A different target is a different resource with its own revision.
        let other = reg.publish(draft("t2")).expect("other");
        assert_ne!(other.resource_id, r1.resource_id);
        assert_eq!(other.revision, 1);
    }

    #[test]
    fn binding_source_and_provenance_cannot_change_across_revisions() {
        let (reg, _) = registry();
        let r1 = reg.publish(draft("t1")).expect("first");
        // Republishing a resource id with another target's binding.
        assert_eq!(
            reg.republish(&r1.resource_id, 1, draft("t2")).unwrap_err(),
            ResourceError::BindingMismatch
        );
        // Rediscovery must not retro-attest provenance: a BrowserTool draft
        // claiming a launched process is an inconsistent pairing, not a
        // revision.
        let mut attest = draft("t1");
        attest.provenance = managed_draft("t1").provenance;
        assert_eq!(
            reg.publish(attest).unwrap_err(),
            ResourceError::InvalidDraft
        );
        // A managed resource cannot be re-published as an unmanaged one, nor
        // swap the process it is attributed to.
        let m1 = reg.publish(managed_draft("t3")).expect("managed");
        assert_eq!(
            reg.publish(draft("t3")).unwrap_err(),
            ResourceError::IdentityChanged
        );
        let mut other_process = managed_draft("t3");
        other_process.provenance = PreviewProvenance::Managed {
            launcher: "present".into(),
            process_id: 4243,
            launched_at_ms: T0,
        };
        assert_eq!(
            reg.republish(&m1.resource_id, 1, other_process)
                .unwrap_err(),
            ResourceError::IdentityChanged
        );
        assert_eq!(
            reg.republish(&m1.resource_id, 1, managed_draft("t3"))
                .expect("same identity revises")
                .revision,
            2
        );
        assert_eq!(
            reg.republish("res_unknown", 1, draft("t1")).unwrap_err(),
            ResourceError::UnknownResource
        );
        // Nothing above advanced the revision.
        assert_eq!(reg.get_active(&r1.resource_id).unwrap().revision, 1);
    }

    #[test]
    fn invalid_refused_artifact_and_empty_drafts_fail_closed() {
        let (reg, _) = registry();
        let mut artifact_kind = draft("t1");
        artifact_kind.kind = PreviewKind::Artifact;
        assert_eq!(
            reg.publish(artifact_kind).unwrap_err(),
            ResourceError::ArtifactNotPublishable
        );
        let mut artifact_source = draft("t1");
        artifact_source.source = PreviewSource::ArtifactAdapter;
        assert_eq!(
            reg.publish(artifact_source).unwrap_err(),
            ResourceError::ArtifactNotPublishable
        );
        let mut refused = draft("t1");
        refused.redaction = RedactionStatus::Refused;
        assert_eq!(
            reg.publish(refused).unwrap_err(),
            ResourceError::Refused { closed: None }
        );

        // Every binding axis, including the principal, must be non-empty.
        for tweak in 0..6 {
            let mut d = draft("t1");
            match tweak {
                0 => d.binding.task.session_key = " ".into(),
                1 => d.binding.task.run_id.clear(),
                2 => d.binding.target.profile.clear(),
                3 => d.binding.target.browser_session_id.clear(),
                4 => d.binding.target.target_id.clear(),
                _ => {
                    d.binding.principal = IssuerIdentity::Connection {
                        connection_id: String::new(),
                    }
                }
            }
            assert_eq!(reg.publish(d).unwrap_err(), ResourceError::InvalidBinding);
        }

        let mut zero_ttl = draft("t1");
        zero_ttl.ttl = Duration::ZERO;
        assert_eq!(
            reg.publish(zero_ttl).unwrap_err(),
            ResourceError::InvalidDraft
        );
        let mut long_ttl = draft("t1");
        long_ttl.ttl = MAX_RESOURCE_TTL + Duration::from_secs(1);
        assert_eq!(
            reg.publish(long_ttl).unwrap_err(),
            ResourceError::InvalidDraft
        );

        let dup = companion_draft("t1", vec![candidate("c1"), candidate("c1")]);
        assert_eq!(reg.publish(dup).unwrap_err(), ResourceError::InvalidDraft);
        let mut c = candidate("c1");
        c.state.active_screen_id = Some("nope".into());
        assert_eq!(
            reg.publish(companion_draft("t1", vec![c])).unwrap_err(),
            ResourceError::InvalidDraft
        );
        let too_many = companion_draft(
            "t1",
            (0..=MAX_CANDIDATES)
                .map(|i| candidate(&format!("c{i}")))
                .collect(),
        );
        assert_eq!(
            reg.publish(too_many).unwrap_err(),
            ResourceError::InvalidDraft
        );

        // Nothing was stored by any refusal.
        assert!(reg.sweep().is_empty());
        let ok = reg.publish(draft("t1")).expect("clean publish still works");
        assert_eq!(ok.revision, 1);
    }

    #[test]
    fn close_emits_exact_event_once_and_leaves_a_tombstone() {
        let (reg, now) = registry();
        let r = reg.publish(draft("t1")).expect("publish");
        now.store(T0 + 5, Ordering::SeqCst);

        let closed = reg
            .close(&r.resource_id, ClosedReason::TargetClosed)
            .expect("transition");
        assert_eq!(
            closed,
            BrowserLiveClosed {
                resource_id: r.resource_id.clone(),
                binding: r.binding.clone(),
                reason: ClosedReason::TargetClosed,
                at_ms: T0 + 5,
            }
        );
        // Not active, no second event, tombstone retained.
        assert!(reg.get_active(&r.resource_id).is_none());
        assert!(reg.close(&r.resource_id, ClosedReason::Revoked).is_none());
        assert_eq!(reg.closed(&r.resource_id), Some(closed));
        assert_eq!(
            reg.republish(&r.resource_id, 1, draft("t1")).unwrap_err(),
            ResourceError::ResourceClosed
        );
        // Re-publishing the binding mints a NEW id; the old one stays closed.
        let again = reg.publish(draft("t1")).expect("new resource");
        assert_ne!(again.resource_id, r.resource_id);
        assert_eq!(again.revision, 1);
        assert!(reg.closed(&r.resource_id).is_some());

        // close_binding closes exactly the bound resource.
        let by_binding = reg
            .close_binding(&binding("t1"), ClosedReason::EngineExited)
            .expect("by binding");
        assert_eq!(by_binding.resource_id, again.resource_id);
        assert!(reg
            .close_binding(&binding("t1"), ClosedReason::EngineExited)
            .is_none());
        assert!(reg.close("res_unknown", ClosedReason::Revoked).is_none());

        // Tombstones age out after retention, measured from the close event's
        // `at_ms`: readable up to the last millisecond, gone at the boundary
        // even before any sweep prunes them, and then unknown (not "closed").
        now.store(T0 + 5 + TOMBSTONE_RETENTION_MS - 1, Ordering::SeqCst);
        assert_eq!(reg.closed(&r.resource_id).map(|c| c.at_ms), Some(T0 + 5));
        now.store(T0 + 5 + TOMBSTONE_RETENTION_MS, Ordering::SeqCst);
        assert!(reg.closed(&r.resource_id).is_none());
        assert_eq!(
            reg.republish(&r.resource_id, 1, draft("t1")).unwrap_err(),
            ResourceError::UnknownResource
        );
        assert!(reg.sweep().is_empty());
        assert!(reg.closed(&r.resource_id).is_none());
    }

    #[test]
    fn close_target_and_session_are_exact_about_profile_session_and_target() {
        let (reg, _now) = registry();
        let mk = |profile: &str, session: &str, target: &str| {
            let mut d = draft(target);
            d.binding.target.profile = profile.into();
            d.binding.target.browser_session_id = session.into();
            reg.publish(d).unwrap().resource_id
        };
        let a1 = mk("default", "bs-1", "t1");
        let a2 = mk("default", "bs-1", "t2");
        let other_session = mk("default", "bs-2", "t1");
        let other_profile = mk("other", "bs-1", "t1");

        let closed = reg.close_target("default", "bs-1", "t1", ClosedReason::TargetClosed);
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].resource_id, a1);
        assert_eq!(closed[0].reason, ClosedReason::TargetClosed);
        assert_eq!(reg.closed(&a1), Some(closed[0].clone()));
        // Exactly once.
        assert!(reg
            .close_target("default", "bs-1", "t1", ClosedReason::TargetClosed)
            .is_empty());
        for id in [&a2, &other_session, &other_profile] {
            assert!(reg.get_active(id).is_some());
        }

        assert!(reg
            .close_browser_session("nope", "bs-1", ClosedReason::EngineExited)
            .is_empty());
        let closed = reg.close_browser_session("default", "bs-1", ClosedReason::EngineExited);
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].resource_id, a2);
        assert_eq!(closed[0].reason, ClosedReason::EngineExited);
        assert!(reg.get_active(&other_session).is_some());
        assert!(reg.get_active(&other_profile).is_some());
        assert!(reg
            .close_browser_session("default", "bs-1", ClosedReason::EngineExited)
            .is_empty());
    }

    #[test]
    fn expired_resources_are_never_active_and_sweep_emits_expired_once() {
        let (reg, now) = registry();
        let a = reg.publish(draft("t1")).expect("a");
        let mut short = draft("t2");
        short.ttl = Duration::from_secs(1);
        let b = reg.publish(short).expect("b");

        now.store(b.expires_at_ms - 1, Ordering::SeqCst);
        assert!(reg.get_active(&b.resource_id).is_some());
        now.store(b.expires_at_ms, Ordering::SeqCst);
        // Expired before the sweep: inactive, uneditable, but not yet closed.
        assert!(reg.get_active(&b.resource_id).is_none());
        assert!(reg.get_active(&a.resource_id).is_some());
        assert!(reg.closed(&b.resource_id).is_none());
        assert_eq!(
            reg.republish(&b.resource_id, 1, draft("t2")).unwrap_err(),
            ResourceError::Expired
        );
        assert_eq!(
            reg.publish({
                let mut d = draft("t2");
                d.ttl = Duration::from_secs(5);
                d
            })
            .unwrap_err(),
            ResourceError::Expired
        );

        // The sweep runs late: the event's `at_ms` is the sweep time (when the
        // transition happened), not the nominal expiry, and the tombstone
        // retention counts from it.
        let sweep_at = b.expires_at_ms + 7;
        now.store(sweep_at, Ordering::SeqCst);
        let swept = reg.sweep();
        assert_eq!(swept.len(), 1);
        assert_eq!(swept[0].resource_id, b.resource_id);
        assert_eq!(swept[0].reason, ClosedReason::Expired);
        assert_eq!(swept[0].at_ms, sweep_at);
        assert!(reg.sweep().is_empty(), "expiry is emitted exactly once");
        assert!(reg.closed(&b.resource_id).is_some());
        now.store(sweep_at + TOMBSTONE_RETENTION_MS - 1, Ordering::SeqCst);
        assert!(reg.closed(&b.resource_id).is_some());
        now.store(sweep_at + TOMBSTONE_RETENTION_MS, Ordering::SeqCst);
        assert!(reg.closed(&b.resource_id).is_none());
        now.store(sweep_at, Ordering::SeqCst);
        // After the sweep the binding can be published afresh.
        assert_eq!(reg.publish(draft("t2")).expect("fresh").revision, 1);

        // Revising extends the absolute expiry from the revision time.
        now.store(T0 + 400_000, Ordering::SeqCst);
        let revised = reg
            .republish(&a.resource_id, 1, draft("t1"))
            .expect("revise");
        assert_eq!(revised.expires_at_ms, T0 + 400_000 + 600_000);
    }

    #[test]
    fn refused_draft_closes_the_live_resource_in_the_same_step() {
        let (reg, now) = registry();
        let r = reg.publish(draft("t1")).expect("safe");
        let mut refused = draft("t1");
        refused.redaction = RedactionStatus::Refused;
        now.store(T0 + 9, Ordering::SeqCst);

        // publish with the same binding: error AND the old safe revision dies.
        let err = reg.publish(refused.clone()).unwrap_err();
        let expected = BrowserLiveClosed {
            resource_id: r.resource_id.clone(),
            binding: r.binding.clone(),
            reason: ClosedReason::Revoked,
            at_ms: T0 + 9,
        };
        assert_eq!(
            err,
            ResourceError::Refused {
                closed: Some(Box::new(expected.clone()))
            }
        );
        assert_eq!(err.closed_event(), Some(&expected));
        assert!(reg.get_active(&r.resource_id).is_none());
        assert_eq!(reg.closed(&r.resource_id), Some(expected));
        // The close event is produced exactly once.
        assert!(reg.close(&r.resource_id, ClosedReason::Revoked).is_none());
        assert_eq!(
            reg.publish(refused.clone()).unwrap_err(),
            ResourceError::Refused { closed: None }
        );
        assert!(reg.sweep().is_empty());

        // republish path: same fail-closed behaviour, and a refusal that does
        // not carry the resource's own binding cannot close it.
        let r2 = reg.publish(draft("t2")).expect("second");
        let mut foreign = draft("t3");
        foreign.redaction = RedactionStatus::Refused;
        assert_eq!(
            reg.republish(&r2.resource_id, 1, foreign).unwrap_err(),
            ResourceError::Refused { closed: None }
        );
        assert!(reg.get_active(&r2.resource_id).is_some());
        let mut refused2 = draft("t2");
        refused2.redaction = RedactionStatus::Refused;
        let err = reg
            .republish(&r2.resource_id, 1, refused2.clone())
            .unwrap_err();
        assert_eq!(
            err.closed_event().map(|c| (&c.resource_id, c.reason)),
            Some((&r2.resource_id, ClosedReason::Revoked))
        );
        assert!(reg.get_active(&r2.resource_id).is_none());
        assert_eq!(
            reg.republish(&r2.resource_id, 1, refused2).unwrap_err(),
            ResourceError::Refused { closed: None }
        );

        // A refusal must not depend on the rest of the draft being sane, and
        // an already-expired live resource closes as Expired.
        let r3 = reg.publish(draft("t4")).expect("third");
        now.store(r3.expires_at_ms, Ordering::SeqCst);
        let mut sloppy = draft("t4");
        sloppy.redaction = RedactionStatus::Refused;
        sloppy.provenance = managed_draft("t4").provenance;
        sloppy.ttl = Duration::ZERO;
        let err = reg.publish(sloppy).unwrap_err();
        assert_eq!(
            err.closed_event().map(|c| c.reason),
            Some(ClosedReason::Expired)
        );
    }

    #[test]
    fn refusal_event_drives_grant_revocation() {
        use super::super::grant::ViewerGrantStore;

        let (reg, _) = registry();
        let grants = ViewerGrantStore::default();
        let r = reg.publish(draft("t1")).expect("safe");
        let mut refused = draft("t1");
        refused.redaction = RedactionStatus::Refused;
        let err = reg.publish(refused).unwrap_err();
        let closed = err
            .closed_event()
            .expect("refusal closes the live resource");
        // The caller wires exactly this: close event -> revoke by resource id.
        assert_eq!(closed.resource_id, r.resource_id);
        assert_eq!(grants.revoke_resource(&closed.resource_id), 0);
    }

    #[test]
    fn managed_provenance_must_pair_with_managed_source() {
        let (reg, _) = registry();
        let mut bad = Vec::new();
        // BrowserTool claiming a managed process.
        let mut d = managed_draft("t1");
        d.source = PreviewSource::BrowserTool;
        bad.push(d);
        // ManagedTool with no attested process.
        let mut d = managed_draft("t1");
        d.provenance = PreviewProvenance::NotManaged;
        bad.push(d);
        for (launcher, process_id, launched_at_ms) in [
            (String::new(), 7, T0),
            ("  ".to_string(), 7, T0),
            ("x".repeat(MAX_TEXT_BYTES + 1), 7, T0),
            ("present".to_string(), 0, T0),
            ("present".to_string(), 7, 0),
            ("present".to_string(), 7, -1),
        ] {
            let mut d = managed_draft("t1");
            d.provenance = PreviewProvenance::Managed {
                launcher,
                process_id,
                launched_at_ms,
            };
            bad.push(d);
        }
        for d in bad {
            assert_eq!(reg.publish(d).unwrap_err(), ResourceError::InvalidDraft);
        }
        assert!(reg.publish(managed_draft("t1")).is_ok());
    }

    #[test]
    fn companion_candidates_are_design_companion_only_and_bounded() {
        let (reg, _) = registry();
        let screen = |id: &str, w: u32, h: u32| CompanionScreen {
            id: id.into(),
            label: "S".into(),
            viewport_css_px: ViewportCssPx {
                width: w,
                height: h,
            },
        };
        let with = |f: &dyn Fn(&mut CompanionCandidate)| {
            let mut c = candidate("c1");
            f(&mut c);
            companion_draft("t1", vec![c])
        };
        let invalid =
            |d: ResourceDraft| assert_eq!(reg.publish(d).unwrap_err(), ResourceError::InvalidDraft);

        // Only DesignCompanion carries candidates.
        for kind in [PreviewKind::Page, PreviewKind::DevPreview] {
            let mut d = companion_draft("t1", vec![candidate("c1")]);
            d.kind = kind;
            invalid(d);
        }
        // Screens: unique ids, non-zero bounded viewport.
        invalid(with(&|c| {
            c.layout.arrangement = CompanionArrangement::SideBySide;
            c.layout.screens = vec![screen("s1", 390, 844), screen("s1", 390, 844)];
        }));
        invalid(with(&|c| c.layout.screens = vec![screen("s1", 0, 844)]));
        invalid(with(&|c| c.layout.screens = vec![screen("s1", 390, 0)]));
        invalid(with(&|c| {
            c.layout.screens = vec![screen("s1", MAX_VIEWPORT_CSS_PX + 1, 844)]
        }));
        // Arrangement vs. screen count.
        invalid(with(&|c| c.layout.screens.clear()));
        invalid(with(&|c| {
            c.layout.screens = vec![screen("s1", 390, 844), screen("s2", 390, 844)]
        }));
        invalid(with(&|c| {
            c.layout.arrangement = CompanionArrangement::SideBySide
        }));
        invalid(with(&|c| {
            c.layout.arrangement = CompanionArrangement::HorizontalScreens
        }));
        invalid(with(&|c| {
            c.layout.arrangement = CompanionArrangement::HorizontalScreens;
            c.layout.screens = (0..=MAX_SCREENS_PER_CANDIDATE)
                .map(|i| screen(&format!("s{i}"), 390, 844))
                .collect();
        }));
        // Animation bounds and labels.
        invalid(with(&|c| {
            c.state.animation.as_mut().unwrap().duration_ms = MAX_ANIMATION_MS + 1
        }));
        invalid(with(&|c| c.state.animation.as_mut().unwrap().name.clear()));
        invalid(with(&|c| c.label = "x".repeat(MAX_TEXT_BYTES + 1)));
        invalid(with(&|c| c.id.clear()));

        // Valid shapes for every arrangement.
        let mut ok = candidate("a");
        ok.layout.arrangement = CompanionArrangement::SideBySide;
        ok.layout.screens = vec![screen("s1", 390, 844), screen("s2", 1280, 720)];
        let mut many = candidate("b");
        many.layout.arrangement = CompanionArrangement::HorizontalScreens;
        many.layout.screens = (0..MAX_SCREENS_PER_CANDIDATE)
            .map(|i| screen(&format!("s{i}"), MAX_VIEWPORT_CSS_PX, 1))
            .collect();
        many.state.active_screen_id = None;
        many.state.animation = None;
        let r = reg
            .publish(companion_draft("t1", vec![candidate("c"), ok, many]))
            .expect("valid candidates");
        assert_eq!(r.companion_candidates.len(), 3);
    }

    #[test]
    fn owner_scoped_lookup_is_generic_for_other_principals() {
        let (reg, _) = registry();
        let r = reg.publish(draft("t1")).expect("publish");
        assert_eq!(
            reg.get_active_for(&r.resource_id, &owner()),
            Some(r.clone())
        );
        let other_user = IssuerIdentity::User {
            user_id: "user-2".into(),
        };
        // Same string, other identity kind, must not match either.
        let connection = IssuerIdentity::Connection {
            connection_id: "user-1".into(),
        };
        assert_eq!(reg.get_active_for(&r.resource_id, &other_user), None);
        assert_eq!(reg.get_active_for(&r.resource_id, &connection), None);
        assert_eq!(reg.get_active_for("res_unknown", &owner()), None);
    }

    #[test]
    fn caps_fail_closed_and_tombstones_make_room() {
        let (reg, _) = registry();
        let mut ids = Vec::new();
        for i in 0..MAX_LIVE_RESOURCES {
            ids.push(
                reg.publish(draft(&format!("t{i}")))
                    .expect("under cap")
                    .resource_id,
            );
        }
        assert_eq!(
            reg.publish(draft("overflow")).unwrap_err(),
            ResourceError::CapacityExceeded
        );
        // Existing resources are untouched and still revisable at the cap.
        assert_eq!(reg.publish(draft("t0")).expect("revise at cap").revision, 2);

        // Closing frees a live slot; its tombstone is bounded too.
        reg.close(&ids[1], ClosedReason::TargetClosed)
            .expect("close");
        reg.publish(draft("overflow")).expect("slot freed");

        // The total record cap evicts the oldest tombstone, never a live one.
        let (reg, now) = registry();
        for round in 0..(MAX_RECORDS + 8) {
            now.store(T0 + round as i64, Ordering::SeqCst);
            let r = reg.publish(draft("churn")).expect("churn publish");
            reg.close(&r.resource_id, ClosedReason::TargetClosed)
                .expect("churn close");
        }
        assert!(reg.lock().slots.len() <= MAX_RECORDS);
        let live = reg.publish(draft("churn")).expect("still publishable");
        assert!(reg.get_active(&live.resource_id).is_some());
    }

    #[test]
    fn resource_ids_are_opaque_and_serialization_has_no_url_or_secret() {
        let (reg, _) = registry();
        let mut d = managed_draft("t-secretish-1");
        d.kind = PreviewKind::DesignCompanion;
        d.companion_candidates = vec![candidate("c1")];
        let r = reg.publish(d).expect("publish");
        let closed = reg
            .close(&r.resource_id, ClosedReason::ServiceExited)
            .expect("close");

        // Opaque id: fixed shape, 32 random bytes, nothing derived from input.
        let suffix = r.resource_id.strip_prefix("res_").expect("prefix");
        assert_eq!(suffix.len(), 43);
        assert!(suffix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
        assert!(!r.resource_id.contains("t-secretish") && !r.resource_id.contains("user-1"));
        let mut distinct = std::collections::HashSet::new();
        for i in 0..64 {
            assert!(distinct.insert(reg.publish(draft(&format!("u{i}"))).unwrap().resource_id));
        }

        // Neither wire object has any URL / title / secret-ish key or value.
        for value in [
            serde_json::to_value(&r).unwrap(),
            serde_json::to_value(&closed).unwrap(),
        ] {
            assert_no_url_or_secret(&value);
        }
    }

    fn assert_no_url_or_secret(value: &Value) {
        match value {
            Value::Object(map) => {
                for (key, v) in map {
                    let key = key.to_ascii_lowercase();
                    for banned in [
                        "url", "uri", "title", "secret", "token", "nonce", "ticket", "cookie",
                        "password",
                    ] {
                        assert!(!key.contains(banned), "forbidden key {key}");
                    }
                    assert_no_url_or_secret(v);
                }
            }
            Value::Array(items) => items.iter().for_each(assert_no_url_or_secret),
            Value::String(s) => assert!(!s.contains("://"), "URL-shaped value {s}"),
            _ => {}
        }
    }
}
