//! Realtime reconciliation for `canvas.updated` frames — the pure decision
//! table. The frame handler in `mod.rs` executes whatever this module
//! decides; nothing here touches a signal or the wire.
//!
//! # The protocol
//!
//! Every committed `canvas.apply` broadcasts one frame carrying the batch's
//! ops and the revision they produced. A client holding revision `R`
//! therefore expects the next frame to say `R + 1`: apply its ops
//! incrementally and adopt that revision. Every other frame shape is one of:
//!
//! - **Our own echo.** The server rebroadcasts a batch to its author too
//!   (the subscription is per-connection and knows nothing about origins).
//!   The author already applied those ops optimistically — applying them a
//!   second time would clobber any newer local preview of the same shapes.
//!   The echo is recognized by revision *and* content: it says
//!   `inflight.base_revision + 1` (acceptance means the server was exactly
//!   at our base) and carries the ops we sent, compared verbatim — exact,
//!   because serde_json round-trips finite `f64` losslessly and every other
//!   field is discrete. Revision alone would misread a *foreign* batch that
//!   won the race to that same revision, which must be applied, not
//!   dropped. The frame's `actor` is deliberately not the discriminator:
//!   two tabs of the same user share an actor, and tab B must apply what
//!   tab A drew.
//! - **Stale.** `revision <= local`: the apply ack landed first (the pump
//!   already bumped the doc), or the frame is a replay. Nothing to do.
//! - **A gap.** `revision > local + 1`: at least one frame was missed.
//!   Applying incrementally would silently skip the missing ops — the only
//!   honest answer is refetching the whole document (`CanvasApi::get`).

use aleph_protocol::canvas::CanvasUpdated;

use crate::state::canvas::InflightBatch;

/// What to do with one `canvas.updated` frame aimed at the open document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reconcile {
    /// The next revision in sequence, not ours: apply the frame's ops to
    /// the doc signal and adopt the frame's revision.
    ApplyOps,
    /// A revision gap — at least one frame was missed: refetch the whole
    /// document.
    Refetch,
    /// Our own optimistic batch echoed back, or a revision already held:
    /// drop the frame.
    DropEcho,
}

/// Classify one frame against the locally held revision and the batch (if
/// any) this client currently has on the wire.
///
/// The echo check runs before the next-revision check on purpose: while a
/// batch is in flight, `base_revision == local` (the base is read from the
/// doc at send time and acks are what advance it), so our echo *also*
/// satisfies `revision == local + 1` — order is the correctness here.
pub(super) fn reconcile(
    local_rev: u64,
    frame: &CanvasUpdated,
    inflight: Option<&InflightBatch>,
) -> Reconcile {
    if frame.revision <= local_rev {
        return Reconcile::DropEcho;
    }
    if let Some(inflight) = inflight {
        if frame.revision == inflight.base_revision + 1 && frame.ops == inflight.ops {
            return Reconcile::DropEcho;
        }
    }
    if frame.revision == local_rev + 1 {
        return Reconcile::ApplyOps;
    }
    Reconcile::Refetch
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::canvas::{CanvasOp, FracIndex, Shape, ShapeCommon, ShapeStyle};

    fn upsert_note(id: &str, x: f64) -> CanvasOp {
        CanvasOp::UpsertShape {
            shape: Shape::Note {
                common: ShapeCommon {
                    id: id.to_string(),
                    x,
                    y: 0.0,
                    w: 100.0,
                    h: 100.0,
                    z: FracIndex::first(),
                    parent_id: None,
                },
                style: ShapeStyle::default(),
                text: String::new(),
            },
        }
    }

    fn frame(revision: u64, ops: Vec<CanvasOp>) -> CanvasUpdated {
        CanvasUpdated {
            canvas_id: "cv-1".to_string(),
            revision,
            ops,
            actor: None,
        }
    }

    fn inflight(base_revision: u64, ops: Vec<CanvasOp>) -> InflightBatch {
        InflightBatch { base_revision, ops }
    }

    /// Arm 1: the next revision in sequence, nothing of ours on the wire —
    /// the ops apply incrementally.
    #[test]
    fn the_next_revision_applies_its_ops_incrementally() {
        let f = frame(6, vec![upsert_note("a", 1.0)]);
        assert_eq!(reconcile(5, &f, None), Reconcile::ApplyOps);
    }

    /// Arm 2: a revision gap means missed ops — incremental application
    /// would silently skip them, so the whole document is refetched.
    #[test]
    fn a_revision_gap_refetches_the_whole_document() {
        let f = frame(7, vec![upsert_note("a", 1.0)]);
        assert_eq!(reconcile(5, &f, None), Reconcile::Refetch, "local + 2");
        let f = frame(42, vec![upsert_note("a", 1.0)]);
        assert_eq!(reconcile(5, &f, None), Reconcile::Refetch, "a wide gap");
    }

    /// Arm 3: our own inflight batch echoed back — matched by base
    /// revision + ops — is dropped, even though it also reads as the next
    /// revision (base == local while the ack is pending).
    #[test]
    fn our_own_inflight_echo_is_dropped_by_revision_and_ops_match() {
        let ops = vec![upsert_note("a", 1.0), upsert_note("b", 2.0)];
        let f = frame(6, ops.clone());
        assert_eq!(
            reconcile(5, &f, Some(&inflight(5, ops))),
            Reconcile::DropEcho
        );
    }

    /// A foreign batch that won the race to the revision our echo would
    /// have used carries different ops — it must be applied, not dropped.
    #[test]
    fn a_foreign_batch_at_the_echo_revision_is_not_mistaken_for_the_echo() {
        let ours = vec![upsert_note("a", 1.0)];
        let theirs = vec![upsert_note("z", 9.0)];
        let f = frame(6, theirs);
        assert_eq!(
            reconcile(5, &f, Some(&inflight(5, ours))),
            Reconcile::ApplyOps
        );
    }

    /// A revision already held is dropped — the post-ack echo (the ack
    /// bumped the doc first) and any replayed frame both land here.
    #[test]
    fn an_already_held_revision_is_dropped() {
        let f = frame(6, vec![upsert_note("a", 1.0)]);
        assert_eq!(reconcile(6, &f, None), Reconcile::DropEcho, "exactly held");
        assert_eq!(reconcile(9, &f, None), Reconcile::DropEcho, "long past");
    }
}
