//! The internal document-boundary gate.
//!
//! A Safe approval is an approval of *one page's content*. The frozen r4 wire
//! carries no content generation, so the generation lives entirely inside the
//! owner: a [`ContentGate`] per `(browser_session_id, target_id)` that the
//! owner advances **synchronously on its dispatcher task** the moment it sees a
//! document boundary (top-level navigation, same-document navigation, reload,
//! an agent/human navigation about to be issued, or an event gap it cannot
//! attribute). Every frame is stamped with the generation it was published
//! under and every viewer receiver is pinned to the generation it was
//! approved under, so a frame of another document can never reach an old
//! approval, regardless of what asynchronous revoke work is still in flight.
//!
//! Nothing here looks at a URL, a hash, a loader id, a "current tab" or a
//! profile name: the key is always the owner's own browser session plus the
//! exact CDP target id.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use aleph_protocol::browser_live::ClosedReason;

/// The monotonic content generation of one target. Cloning shares the counter.
#[derive(Debug, Clone, Default)]
pub(crate) struct ContentGate(Arc<AtomicU64>);

impl ContentGate {
    pub(crate) fn current(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }

    /// Start a new generation; returns the new value.
    pub(crate) fn advance(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }
}

/// Which targets of one browser session a boundary covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BoundaryScope {
    /// Exactly this CDP target.
    Target(String),
    /// Every target of the session: used when the event cannot be attributed to
    /// one target (malformed event, unknown CDP session, event gap). Fail
    /// closed: over-approximating only costs a re-approval, never a leak.
    AllTargets,
}

/// Why a boundary was declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoundaryCause {
    /// `Page.frameNavigated` for a main frame.
    TopLevelNavigation,
    /// `Page.navigatedWithinDocument` (hash / `pushState`) for the main frame.
    SameDocumentNavigation,
    /// An agent verb is about to navigate / reload / traverse history.
    AgentNavigation,
    /// A human `Navigate` input is about to be issued.
    HumanNavigation,
    /// The event stream may have dropped events (lag).
    EventGap,
    /// A navigation event that could not be attributed to one target.
    Unattributed,
}

/// One declared boundary, addressed by the owner's exact identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DocumentBoundary {
    pub(crate) profile: String,
    pub(crate) browser_session_id: String,
    pub(crate) scope: BoundaryScope,
    pub(crate) cause: BoundaryCause,
}

/// Receiver of boundaries. Implemented by the lifecycle, which demotes every
/// matching approval (revision bump, back to Pending) and revokes its grants.
///
/// Called **synchronously**, on the owner's dispatcher task, *before* the
/// owner advances the content gate and with no owner lock held. The
/// implementation must therefore be non-async and must not call back into the
/// owner.
pub(crate) trait DocumentBoundarySink: Send + Sync {
    fn document_boundary(&self, boundary: &DocumentBoundary);
}

/// Receiver of an owner's terminal events: the exact CDP target was destroyed,
/// or the owner itself ended (engine gone, dispatcher lag fail-closed, retired).
/// Implemented by the lifecycle, which closes the published resources bound to
/// exactly that `(profile, browser_session_id[, target_id])` and revokes their
/// grants.
///
/// Called **synchronously** on the owner's dispatcher task (or on the caller of
/// `retire`), after the owner has released its state lock and with no owner
/// lock held. Implementations must be non-async, must not call back into the
/// owner and must not touch the owner registry (route retirement is a separate
/// concern): they only close resources. `owner_ended` is delivered at most once
/// per owner.
pub(crate) trait OwnerTerminalSink: Send + Sync {
    /// The exact CDP target `target_id` was destroyed.
    fn target_destroyed(&self, profile: &str, browser_session_id: &str, target_id: &str);
    /// Every target of the owner's browser session is gone.
    fn owner_ended(&self, profile: &str, browser_session_id: &str, reason: ClosedReason);
}
