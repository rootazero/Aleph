//! Optimistic ops pipeline for the whiteboard editor — all of it pure and
//! unit-tested on the native target. The DOM wiring in `editor.rs` is thin.
//!
//! # The pipeline
//!
//! Every local edit becomes a `Vec<CanvasOp>` that is (1) applied to the doc
//! signal immediately ([`apply_local`]), (2) recorded on the [`UndoStack`],
//! and (3) handed to the [`SendQueue`], which keeps **one batch in flight at
//! a time** — ops produced while a batch is on the wire are queued and go out
//! as the next batch after the ack. `base_revision` is *not* stored here: the
//! doc signal's `revision` field is server truth (set by fetch and by ack,
//! never bumped optimistically), and the sender reads it at send time — a
//! second copy in this queue would be the second source the §0 criteria warn
//! about.
//!
//! # `apply_local` mirrors the server, verbatim
//!
//! The server's `validate::apply_ops` is the semantic single source for what
//! an op does. This module cannot call it (the Panel does not depend on
//! `alephcore`), so it carries a token-identical copy of the mutation loop,
//! and `apply_local_matches_the_server_apply_ops_loop_verbatim` below pins
//! the two against each other at the source level: change either side and the
//! test names the drift. The server's post-state `MAX_SHAPES` check is
//! deliberately outside the pin — the server enforces caps; the Panel only
//! replays ops the server has already accepted (or is about to judge).
//!
//! # Conflict recovery (the protocol's whole point)
//!
//! A stale `base_revision` comes back as the `REVISION_CONFLICT` JSON-RPC
//! code, but that code never reaches this crate — `rpc_call` surfaces
//! `error.message` alone (see `api/canvas.rs`'s module doc for the
//! investigation). What survives is the message, whose conflict phrase is
//! minted by `CanvasError::Conflict`'s `#[error]` attribute in
//! `src/canvas/store.rs` and pinned here at the source level. On conflict the
//! editor: refetches the doc, replays [`SendQueue::recover`]'s pending ops
//! (refused batch first, queued ops after — original order) on the fresh doc,
//! and resends them as one batch against the fresh revision.

use aleph_protocol::canvas::{CanvasDoc, CanvasOp};

/// Undo history depth. Oldest entries fall off first.
pub(super) const UNDO_CAP: usize = 200;

/// Apply an ops batch to a document, with the server's exact semantics.
///
/// The `for` loop below is a token-for-token copy of the one inside
/// `src/canvas/validate.rs::apply_ops` — kept identical on purpose, and
/// pinned by the source-comparison test at the bottom of this file. Do not
/// "improve" one side without the other.
pub(super) fn apply_local(doc: &mut CanvasDoc, ops: &[CanvasOp]) {
    for op in ops {
        match op {
            CanvasOp::UpsertShape { shape } => {
                match doc.shapes.iter_mut().find(|s| s.id() == shape.id()) {
                    Some(slot) => *slot = shape.clone(),
                    None => doc.shapes.push(shape.clone()),
                }
            }
            CanvasOp::DeleteShape { id } => doc.shapes.retain(|s| s.id() != id),
            CanvasOp::SetDocMeta { title } => doc.title.clone_from(title),
            CanvasOp::UpsertDeck { deck } => match doc.decks.iter_mut().find(|d| d.id == deck.id) {
                Some(slot) => *slot = deck.clone(),
                None => doc.decks.push(deck.clone()),
            },
            CanvasOp::DeleteDeck { id } => doc.decks.retain(|d| &d.id != id),
        }
    }
}

/// Inverse of an ops batch against the document it was applied to.
///
/// Walked front-to-back on a scratch copy (a later op in the same batch may
/// touch state an earlier one changed), inverses collected in reverse order:
/// `apply(doc, ops); apply(doc, invert(doc_before, ops))` restores `doc` up
/// to storage order — an upsert-after-delete re-appends at the end of the
/// vec, which is invisible to rendering (z-order plus id tie-break decides
/// paint order, not vec position) and matches what the server itself would
/// do with the same inverse ops. Deleting a shape that was never there
/// inverts to nothing, exactly as its application is a no-op.
pub(super) fn invert(doc_before: &CanvasDoc, ops: &[CanvasOp]) -> Vec<CanvasOp> {
    let mut doc = doc_before.clone();
    let mut inverses: Vec<CanvasOp> = Vec::new();
    for op in ops {
        let inverse = match op {
            CanvasOp::UpsertShape { shape } => {
                Some(match doc.shapes.iter().find(|s| s.id() == shape.id()) {
                    Some(prev) => CanvasOp::UpsertShape {
                        shape: prev.clone(),
                    },
                    None => CanvasOp::DeleteShape {
                        id: shape.id().to_string(),
                    },
                })
            }
            CanvasOp::DeleteShape { id } => {
                doc.shapes
                    .iter()
                    .find(|s| s.id() == id)
                    .map(|prev| CanvasOp::UpsertShape {
                        shape: prev.clone(),
                    })
            }
            CanvasOp::SetDocMeta { .. } => Some(CanvasOp::SetDocMeta {
                title: doc.title.clone(),
            }),
            CanvasOp::UpsertDeck { deck } => {
                Some(match doc.decks.iter().find(|d| d.id == deck.id) {
                    Some(prev) => CanvasOp::UpsertDeck { deck: prev.clone() },
                    None => CanvasOp::DeleteDeck {
                        id: deck.id.clone(),
                    },
                })
            }
            CanvasOp::DeleteDeck { id } => doc
                .decks
                .iter()
                .find(|d| &d.id == id)
                .map(|prev| CanvasOp::UpsertDeck { deck: prev.clone() }),
        };
        inverses.extend(inverse);
        apply_local(&mut doc, std::slice::from_ref(op));
    }
    inverses.reverse();
    inverses
}

/// Is this `CanvasApi::apply` error a revision conflict?
///
/// There is no error code to branch on (`rpc_call` drops it — `api/canvas.rs`
/// module doc), so the branch is on the message phrase minted by
/// `CanvasError::Conflict` in `src/canvas/store.rs`. The source-level test
/// below pins that phrase where it is minted, so a server-side rewording
/// fails a test instead of silently turning every conflict into the
/// clear-and-refetch error path.
pub(super) fn is_revision_conflict(message: &str) -> bool {
    message.contains("revision conflict")
}

/// One undoable edit: the ops that made it and the ops that unmake it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct UndoEntry {
    pub redo_ops: Vec<CanvasOp>,
    pub undo_ops: Vec<CanvasOp>,
}

/// Per-open-canvas, per-client undo history (never persisted, never synced).
///
/// Undone edits move to the redo side; a fresh edit clears the redo side —
/// the universal branch-discard model. Capped at [`UNDO_CAP`] entries,
/// oldest first out.
#[derive(Debug, Default)]
pub(super) struct UndoStack {
    undo: Vec<UndoEntry>,
    redo: Vec<UndoEntry>,
}

impl UndoStack {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Record a fresh edit. Clears the redo branch.
    pub(super) fn record(&mut self, redo_ops: Vec<CanvasOp>, undo_ops: Vec<CanvasOp>) {
        self.redo.clear();
        self.undo.push(UndoEntry { redo_ops, undo_ops });
        if self.undo.len() > UNDO_CAP {
            self.undo.remove(0);
        }
    }

    /// Step back: returns the ops that unmake the newest edit.
    pub(super) fn undo(&mut self) -> Option<Vec<CanvasOp>> {
        let entry = self.undo.pop()?;
        let ops = entry.undo_ops.clone();
        self.redo.push(entry);
        Some(ops)
    }

    /// Step forward again: returns the ops that remake the newest undone edit.
    pub(super) fn redo(&mut self) -> Option<Vec<CanvasOp>> {
        let entry = self.redo.pop()?;
        let ops = entry.redo_ops.clone();
        self.undo.push(entry);
        Some(ops)
    }
}

/// Single-flight batching for `canvas.apply`.
///
/// At most one batch is on the wire; ops committed meanwhile accumulate and
/// leave as one batch on the next ack. The queue holds ops only — the base
/// revision is read from the doc signal at send time (module doc).
#[derive(Debug, Default)]
pub(super) struct SendQueue {
    inflight: Option<Vec<CanvasOp>>,
    queued: Vec<CanvasOp>,
}

impl SendQueue {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Hand ops to the queue. `Some(batch)` means "send this now"; `None`
    /// means a batch is already in flight and these ops will ride the next
    /// one.
    pub(super) fn push(&mut self, mut ops: Vec<CanvasOp>) -> Option<Vec<CanvasOp>> {
        if self.inflight.is_some() {
            self.queued.append(&mut ops);
            None
        } else {
            self.inflight = Some(ops.clone());
            Some(ops)
        }
    }

    /// The in-flight batch landed. `Some(batch)` is the next one to send.
    pub(super) fn on_ack(&mut self) -> Option<Vec<CanvasOp>> {
        self.inflight = None;
        if self.queued.is_empty() {
            return None;
        }
        let batch = std::mem::take(&mut self.queued);
        self.inflight = Some(batch.clone());
        Some(batch)
    }

    /// Conflict path: collapse everything pending — the refused in-flight
    /// batch first, then the queued ops, original order — into one new
    /// in-flight batch, returned for replay + resend.
    pub(super) fn recover(&mut self) -> Option<Vec<CanvasOp>> {
        let mut all = self.inflight.take().unwrap_or_default();
        all.append(&mut self.queued);
        if all.is_empty() {
            return None;
        }
        self.inflight = Some(all.clone());
        Some(all)
    }

    /// Drop everything — the giving-up path (non-conflict refusal, canvas
    /// closed mid-flight). The caller refetches server truth right after.
    pub(super) fn clear(&mut self) -> Option<Vec<CanvasOp>> {
        self.inflight = None;
        self.queued.clear();
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::canvas::{Deck, FracIndex, Shape, ShapeCommon, ShapeStyle};
    use std::path::Path;

    fn note(id: &str, x: f64) -> Shape {
        Shape::Note {
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
        }
    }

    fn doc_with(shapes: Vec<Shape>, decks: Vec<Deck>) -> CanvasDoc {
        CanvasDoc {
            id: "cv-1".to_string(),
            title: "t".to_string(),
            owner_user_id: None,
            project_id: None,
            revision: 1,
            shapes,
            decks,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    fn upsert(shape: Shape) -> CanvasOp {
        CanvasOp::UpsertShape { shape }
    }

    /// Storage order is not semantic (rendering sorts by z + id), so
    /// round-trip equality is judged with shapes and decks sorted by id.
    fn canonical(mut d: CanvasDoc) -> CanvasDoc {
        d.shapes.sort_by(|a, b| a.id().cmp(b.id()));
        d.decks.sort_by(|a, b| a.id.cmp(&b.id));
        d
    }

    // ---- apply_local ↔ server semantics -----------------------------------

    /// Extract the `for op in ops { … }` mutation loop that follows
    /// `fn <name>`, whitespace-normalized so formatting differences are
    /// invisible and token differences are not.
    fn mutation_loop(src: &str, fn_name: &str) -> String {
        let src = src.replace('\r', "");
        let fn_pos = src
            .find(&format!("fn {fn_name}"))
            .unwrap_or_else(|| panic!("fn {fn_name} not found"));
        let tail = &src[fn_pos..];
        let for_pos = tail
            .find("for op in ops")
            .unwrap_or_else(|| panic!("no `for op in ops` loop inside fn {fn_name}"));
        let body = &tail[for_pos..];
        let open = body.find('{').expect("loop must open a block");
        let mut depth = 0i32;
        let mut end = None;
        for (i, c) in body[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + i + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        let end = end.expect("unbalanced braces in mutation loop");
        body[..end].split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// The Panel's `apply_local` loop and the server's `validate::apply_ops`
    /// loop are token-identical. This is the drift pin the module doc
    /// promises: the Panel cannot call the server's function (no `alephcore`
    /// dependency), so the semantics are mirrored — and a mirror without a
    /// comparison test is two truths waiting to diverge (§0).
    #[test]
    fn apply_local_matches_the_server_apply_ops_loop_verbatim() {
        let server_path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src/canvas/validate.rs");
        let server = std::fs::read_to_string(&server_path).unwrap_or_else(|e| {
            panic!(
                "cannot read {} — if the server moved its ops semantics, \
                 update this pin, not just the path: {e}",
                server_path.display()
            )
        });
        let server_loop = mutation_loop(&server, "apply_ops");
        let panel_loop = mutation_loop(include_str!("ops.rs"), "apply_local");
        assert_eq!(
            server_loop, panel_loop,
            "apply_local has drifted from src/canvas/validate.rs::apply_ops — \
             the two mutation loops must stay token-identical"
        );
    }

    /// The conflict phrase this module branches on is pinned where it is
    /// minted: `CanvasError::Conflict`'s `#[error]` attribute. Reworded
    /// server-side, this test names the drift instead of every conflict
    /// silently taking the clear-and-refetch path.
    #[test]
    fn the_conflict_detector_matches_the_phrase_the_server_mints() {
        let store_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src/canvas/store.rs");
        let store = std::fs::read_to_string(&store_path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", store_path.display()));
        let minted = store
            .replace('\r', "")
            .lines()
            .find(|l| l.contains("#[error(") && l.contains("revision conflict"))
            .map(str::to_string);
        assert!(
            minted.is_some(),
            "src/canvas/store.rs no longer mints a 'revision conflict' error \
             message — update is_revision_conflict to whatever the server \
             sends now"
        );
        // The full wire shape (context prefix + Display) matches…
        assert!(is_revision_conflict(
            "Failed to apply canvas ops: revision conflict: canvas is at revision 7"
        ));
        // …and the not-found / invalid shapes do not.
        assert!(!is_revision_conflict(
            "Failed to apply canvas ops: canvas cv-x not found"
        ));
        assert!(!is_revision_conflict(
            "Failed to apply canvas ops: 501 ops in one apply exceeds the 500-op cap"
        ));
    }

    // ---- invert ------------------------------------------------------------

    #[test]
    fn invert_of_a_fresh_upsert_is_a_delete() {
        let doc = doc_with(vec![], vec![]);
        let ops = vec![upsert(note("a", 0.0))];
        let inv = invert(&doc, &ops);
        assert_eq!(inv, vec![CanvasOp::DeleteShape { id: "a".into() }]);
    }

    #[test]
    fn invert_of_an_overwrite_restores_the_previous_shape() {
        let doc = doc_with(vec![note("a", 1.0)], vec![]);
        let ops = vec![upsert(note("a", 99.0))];
        let inv = invert(&doc, &ops);
        assert_eq!(inv, vec![upsert(note("a", 1.0))]);
    }

    #[test]
    fn invert_of_a_delete_restores_and_of_a_missing_delete_is_nothing() {
        let doc = doc_with(vec![note("a", 1.0)], vec![]);
        let inv = invert(
            &doc,
            &[
                CanvasOp::DeleteShape { id: "a".into() },
                CanvasOp::DeleteShape { id: "ghost".into() },
            ],
        );
        // The missing delete contributes no inverse; the real one restores.
        assert_eq!(inv, vec![upsert(note("a", 1.0))]);
    }

    #[test]
    fn invert_walks_the_batch_in_order_so_later_ops_see_earlier_effects() {
        // Upsert a fresh shape, then overwrite it in the same batch: the
        // second op's inverse must restore the *first* op's value, and the
        // reversed order must delete last.
        let doc = doc_with(vec![], vec![]);
        let ops = vec![upsert(note("a", 1.0)), upsert(note("a", 2.0))];
        let inv = invert(&doc, &ops);
        assert_eq!(
            inv,
            vec![
                upsert(note("a", 1.0)),
                CanvasOp::DeleteShape { id: "a".into() }
            ]
        );
    }

    #[test]
    fn invert_restores_title_and_decks() {
        let deck = Deck {
            id: "d1".to_string(),
            title: "old deck".to_string(),
            frame_ids: vec!["f1".to_string()],
        };
        let doc = doc_with(vec![], vec![deck.clone()]);
        let ops = vec![
            CanvasOp::SetDocMeta {
                title: "new".into(),
            },
            CanvasOp::UpsertDeck {
                deck: Deck {
                    id: "d1".to_string(),
                    title: "renamed".to_string(),
                    frame_ids: vec![],
                },
            },
            CanvasOp::DeleteDeck { id: "d1".into() },
        ];
        let mut after = doc.clone();
        apply_local(&mut after, &ops);
        apply_local(&mut after, &invert(&doc, &ops));
        assert_eq!(canonical(after), canonical(doc));
    }

    /// The plan's property test: apply(invert) round-trips random op
    /// sequences. Deterministic LCG (the viewport.rs idiom — no rand dep).
    #[test]
    fn apply_then_apply_inverse_round_trips_random_op_sequences() {
        fn lcg(seed: &mut u64) -> u64 {
            *seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *seed >> 33
        }
        let ids = ["a", "b", "c", "d", "e"];
        let deck_ids = ["d1", "d2"];
        let mut seed = 0x1234_5678_9abc_def0u64;
        for _case in 0..300 {
            // Random initial doc.
            let mut shapes = Vec::new();
            for id in ids {
                if lcg(&mut seed) % 2 == 0 {
                    shapes.push(note(id, (lcg(&mut seed) % 1000) as f64));
                }
            }
            let mut decks = Vec::new();
            for id in deck_ids {
                if lcg(&mut seed) % 2 == 0 {
                    decks.push(Deck {
                        id: id.to_string(),
                        title: format!("t{}", lcg(&mut seed) % 10),
                        frame_ids: vec![],
                    });
                }
            }
            let before = doc_with(shapes, decks);

            // Random op sequence.
            let len = 1 + (lcg(&mut seed) % 15) as usize;
            let mut ops = Vec::with_capacity(len);
            for _ in 0..len {
                let op = match lcg(&mut seed) % 5 {
                    0 => upsert(note(
                        ids[(lcg(&mut seed) as usize) % ids.len()],
                        (lcg(&mut seed) % 1000) as f64,
                    )),
                    1 => CanvasOp::DeleteShape {
                        id: ids[(lcg(&mut seed) as usize) % ids.len()].to_string(),
                    },
                    2 => CanvasOp::SetDocMeta {
                        title: format!("title{}", lcg(&mut seed) % 7),
                    },
                    3 => CanvasOp::UpsertDeck {
                        deck: Deck {
                            id: deck_ids[(lcg(&mut seed) as usize) % deck_ids.len()].to_string(),
                            title: format!("deck{}", lcg(&mut seed) % 7),
                            frame_ids: vec![format!("f{}", lcg(&mut seed) % 3)],
                        },
                    },
                    _ => CanvasOp::DeleteDeck {
                        id: deck_ids[(lcg(&mut seed) as usize) % deck_ids.len()].to_string(),
                    },
                };
                ops.push(op);
            }

            let inv = invert(&before, &ops);
            let mut doc = before.clone();
            apply_local(&mut doc, &ops);
            apply_local(&mut doc, &inv);
            assert_eq!(
                canonical(doc),
                canonical(before),
                "round-trip failed for ops: {ops:?}"
            );
        }
    }

    // ---- UndoStack ---------------------------------------------------------

    #[test]
    fn undo_then_redo_round_trips_and_a_fresh_edit_clears_the_redo_branch() {
        let mut stack = UndoStack::new();
        let redo_a = vec![upsert(note("a", 1.0))];
        let undo_a = vec![CanvasOp::DeleteShape { id: "a".into() }];
        stack.record(redo_a.clone(), undo_a.clone());

        assert_eq!(stack.undo(), Some(undo_a.clone()));
        assert_eq!(stack.redo(), Some(redo_a.clone()));
        assert_eq!(stack.redo(), None, "nothing further to redo");

        // Undo again, then record a fresh edit: the redo branch is discarded.
        assert_eq!(stack.undo(), Some(undo_a));
        stack.record(vec![upsert(note("b", 2.0))], vec![]);
        assert_eq!(stack.redo(), None, "a fresh edit discards the redo branch");
    }

    #[test]
    fn the_undo_stack_caps_at_200_dropping_the_oldest() {
        let mut stack = UndoStack::new();
        for i in 0..(UNDO_CAP + 5) {
            stack.record(
                vec![],
                vec![CanvasOp::DeleteShape {
                    id: format!("s{i}"),
                }],
            );
        }
        let mut popped = 0;
        let mut last = None;
        while let Some(ops) = stack.undo() {
            popped += 1;
            last = Some(ops);
        }
        assert_eq!(popped, UNDO_CAP, "history must cap at {UNDO_CAP}");
        // The oldest surviving entry is #5 — entries 0..5 fell off the front.
        assert_eq!(last, Some(vec![CanvasOp::DeleteShape { id: "s5".into() }]));
    }

    // ---- SendQueue ---------------------------------------------------------

    #[test]
    fn the_queue_is_single_flight_and_acks_promote_queued_ops_in_order() {
        let mut q = SendQueue::new();
        let first = vec![upsert(note("a", 1.0))];
        assert_eq!(
            q.push(first.clone()),
            Some(first),
            "an idle queue sends immediately"
        );

        // While in flight, further ops queue instead of sending.
        assert_eq!(q.push(vec![upsert(note("b", 2.0))]), None);
        assert_eq!(q.push(vec![CanvasOp::DeleteShape { id: "a".into() }]), None);

        // The ack promotes everything queued, in commit order, as one batch.
        let next = q.on_ack().expect("queued ops must go out on ack");
        assert_eq!(
            next,
            vec![
                upsert(note("b", 2.0)),
                CanvasOp::DeleteShape { id: "a".into() }
            ]
        );
        assert_eq!(q.on_ack(), None, "nothing further after the last ack");
        // And the queue is idle again.
        let again = vec![upsert(note("c", 3.0))];
        assert_eq!(q.push(again.clone()), Some(again));
    }

    #[test]
    fn recover_collapses_the_refused_batch_and_the_queue_in_order() {
        let mut q = SendQueue::new();
        let refused = vec![upsert(note("a", 1.0))];
        assert!(q.push(refused).is_some());
        assert!(q.push(vec![upsert(note("b", 2.0))]).is_none());

        let replay = q.recover().expect("pending ops must be recoverable");
        assert_eq!(
            replay,
            vec![upsert(note("a", 1.0)), upsert(note("b", 2.0))],
            "refused batch first, queued ops after — the original order"
        );
        // The collapsed batch is the new in-flight batch: its ack drains it.
        assert_eq!(q.on_ack(), None);
        assert_eq!(q.recover(), None, "nothing pending after the ack");
    }

    #[test]
    fn clear_drops_everything_pending() {
        let mut q = SendQueue::new();
        assert!(q.push(vec![upsert(note("a", 1.0))]).is_some());
        assert!(q.push(vec![upsert(note("b", 2.0))]).is_none());
        assert_eq!(q.clear(), None);
        assert_eq!(q.recover(), None, "cleared means gone");
    }
}
