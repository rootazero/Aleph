//! Client-side screen state: applies server diffs, detects gaps, and buffers
//! frames that arrive while an attach is in flight.
//!
//! The gateway event bus is a bounded broadcast that drops frames for lagging
//! subscribers, so a gap is ordinary traffic rather than an exceptional case.
//! That is why `seq` exists and why a gap must resynchronise from a snapshot
//! instead of being papered over.
//!
//! A missed frame (`seq` skipped ahead) and a stale frame (`seq` at or below
//! what is already applied) are different situations with different correct
//! responses, so they are different [`ApplyOutcome`] variants rather than one
//! `Gap` reported for both: a stale frame is already represented in this
//! screen's state and must be dropped cheaply, not treated as a reason to pay
//! for a full `pty.attach` round trip. Collapsing the two would also risk a
//! resync loop if a stale frame kept repeating.
//!
//! `pty.screen` is one topic carrying every session's frames — this screen
//! must not let another session's `seq` be read against its own counter, so
//! [`ClientScreen::apply`] is constructed with the session it tracks and
//! filters on `session_id` itself rather than trusting the caller to
//! pre-filter (see that method's doc for why).

use aleph_protocol::pty::{PtyAttachResponse, PtyScreenFrame, PtyScreenPatch, PtyStyleRun};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied,
    /// A frame was missed (`seq` skipped ahead of what was expected). The
    /// caller must call `pty.attach` and feed the result to
    /// [`ClientScreen::finish_attach`].
    Gap {
        expected: u64,
        got: u64,
    },
    /// Held until the in-flight attach lands.
    Buffered,
    /// `frame.seq` is at or below the already-applied seq: a duplicate
    /// delivery, or a frame that predates the last attach's snapshot. Not an
    /// error and not a gap — there is nothing to resync, the frame's content
    /// is already reflected in this screen's state.
    Discarded {
        seq: u64,
    },
    /// `frame.session_id` does not match the session this screen tracks.
    /// Ignored, not counted against this screen's `seq`.
    WrongSession,
}

/// The result of [`ClientScreen::finish_attach`]: adopting a snapshot, then
/// replaying whatever had buffered while the attach was in flight.
///
/// This is a separate type from [`ApplyOutcome`] rather than reusing it:
/// `finish_attach` can never buffer (there is no attach nested inside an
/// attach) or discard a live frame (buffered frames are filtered against the
/// snapshot's `seq`, not reported one at a time) or see a foreign session
/// (that filter already ran when the frame was buffered, in `apply`), so
/// `Buffered`, `Discarded`, and `WrongSession` are not states this method can
/// be in. A type that can only ever be constructed as one of two variants
/// says that at the type level instead of by convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachOutcome {
    /// The snapshot was adopted, and every frame buffered while the attach
    /// was in flight — if any — replayed with `seq` advancing by exactly one
    /// each time. There is nothing for the caller to do.
    Resynced,
    /// Replay hit a hole: a buffered frame's `seq` was more than one past
    /// the last applied `seq`, meaning a frame between them was dropped by
    /// the broadcast bus while this attach was in flight. Replay stops at
    /// the hole rather than skipping over it — `seq` is left at the frame
    /// just before it, not advanced past it — so the screen never reports
    /// itself more current than it actually is. The rows only the missing
    /// frame would have touched are wrong until the caller re-attaches;
    /// the next live [`ClientScreen::apply`] call would also report this
    /// same gap on its own, but the caller may prefer to re-attach
    /// immediately rather than wait for one.
    Gap { expected: u64, got: u64 },
}

pub struct ClientScreen {
    session_id: String,
    rows: u16,
    cols: u16,
    seq: u64,
    grid: Vec<Vec<PtyStyleRun>>,
    cursor: (u16, u16),
    title: Option<String>,
    alt_screen: bool,
    /// `Some` while an attach is in flight; frames land here instead of on
    /// the grid, because the snapshot they must be ordered against has not
    /// arrived yet.
    pending: Option<Vec<PtyScreenFrame>>,
}

impl ClientScreen {
    /// `session_id` is the PTY session this screen tracks. [`apply`](Self::apply)
    /// enforces it against every incoming frame — `pty.screen` is one topic
    /// shared by every session, so a screen that trusted its caller to
    /// pre-filter would read another session's `seq` as a gap in its own.
    ///
    /// `rows`/`cols` are clamped to at least 1, same as [`resize`](Self::resize) —
    /// a screen with a zero dimension cannot address any row, and a caller
    /// that has not learned real geometry yet (e.g. a placeholder before the
    /// first `finish_attach`) should not be able to construct one by
    /// accident.
    #[must_use]
    pub fn new(rows: u16, cols: u16, seq: u64, session_id: impl Into<String>) -> Self {
        let rows = rows.max(1);
        let cols = cols.max(1);
        Self {
            session_id: session_id.into(),
            rows,
            cols,
            seq,
            grid: vec![Vec::new(); rows as usize],
            cursor: (0, 0),
            title: None,
            alt_screen: false,
            pending: None,
        }
    }

    #[must_use]
    pub const fn dims(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    #[must_use]
    pub const fn seq(&self) -> u64 {
        self.seq
    }

    #[must_use]
    pub const fn cursor(&self) -> (u16, u16) {
        self.cursor
    }

    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    #[must_use]
    pub const fn alt_screen(&self) -> bool {
        self.alt_screen
    }

    #[must_use]
    pub fn row_runs(&self, row: u16) -> &[PtyStyleRun] {
        self.grid.get(row as usize).map_or(&[], Vec::as_slice)
    }

    /// Row as plain text. Test- and selection-facing.
    #[must_use]
    pub fn row_text(&self, row: u16) -> String {
        let s: String = self.row_runs(row).iter().map(|r| r.text.as_str()).collect();
        s.trim_end().to_string()
    }

    pub fn begin_attach(&mut self) {
        self.pending.get_or_insert_with(Vec::new);
    }

    /// Adopt the snapshot, then replay every buffered frame newer than it —
    /// stopping at the first hole rather than skipping over it. See
    /// [`AttachOutcome`] for what the caller should do with the result.
    ///
    /// A buffered frame at or below `resp.seq` is already represented in the
    /// snapshot (it was captured on the server after that frame was
    /// produced); replaying it would double-apply, so it is dropped here —
    /// same rule as [`ApplyOutcome::Discarded`], applied on the way out of
    /// the buffer instead of on the way in.
    #[must_use]
    pub fn finish_attach(&mut self, resp: PtyAttachResponse) -> AttachOutcome {
        if resp.seq < self.seq {
            // The snapshot predates what this screen already has. This
            // should not happen in normal operation — `seq` is per-session
            // and monotonic on the server, and only one attach is meant to
            // be in flight at a time — but `resp` is wire input, not a local
            // invariant, so it is guarded rather than trusted. Adopting it
            // would move `seq` (and grid content) backward, which is exactly
            // what the monotonic counter exists to prevent. The attach is
            // still resolved — `pending` is cleared so buffered frames are
            // not held forever — but they are dropped rather than replayed:
            // once the snapshot they were meant to be ordered against is
            // rejected, there is no `expected` left to replay them against.
            self.pending = None;
            return AttachOutcome::Resynced;
        }
        self.resize(resp.rows, resp.cols);
        self.seq = resp.seq;
        self.write_patch(&resp.patch);
        let buffered = self.pending.take().unwrap_or_default();
        for frame in buffered {
            if frame.seq <= self.seq {
                // Already represented in the snapshot (or in an earlier
                // replayed frame); drop and keep scanning — a hole can still
                // exist later in the buffer.
                continue;
            }
            let expected = self.seq.saturating_add(1);
            if frame.seq != expected {
                // A hole: the broadcast bus dropped a frame while this
                // attach was in flight. Stop here rather than skipping over
                // it — `seq` stays at the last frame actually applied, so it
                // never claims to be more current than it is.
                return AttachOutcome::Gap {
                    expected,
                    got: frame.seq,
                };
            }
            self.seq = frame.seq;
            self.write_patch(&frame.patch);
        }
        AttachOutcome::Resynced
    }

    /// Apply one server frame.
    ///
    /// `pty.screen` is one topic carrying every session's frames, so this
    /// filters on `session_id` itself rather than trusting the caller to
    /// pre-filter: a caller that forgot to filter would otherwise let
    /// another session's `seq` be read against this screen's counter,
    /// producing a spurious [`ApplyOutcome::Gap`] (or, worse, painting that
    /// session's content here if the sequence numbers happened to line up).
    ///
    /// `frame.patch.bell` is not read here and `ClientScreen` has no field
    /// for it — a bell is an edge (the server takes it once and clears it,
    /// see `perform.rs`'s `take_bell`), not a level this screen holds, and a
    /// frame can be bell-only (no row/cursor/title change at all). A caller
    /// that wants to react to it must read `frame.patch.bell` itself before
    /// calling `apply`, which consumes `frame`; after this returns, the bit
    /// is gone.
    pub fn apply(&mut self, frame: PtyScreenFrame) -> ApplyOutcome {
        if frame.session_id != self.session_id {
            return ApplyOutcome::WrongSession;
        }
        if let Some(buf) = &mut self.pending {
            buf.push(frame);
            return ApplyOutcome::Buffered;
        }
        let expected = self.seq.saturating_add(1);
        if frame.seq < expected {
            return ApplyOutcome::Discarded { seq: frame.seq };
        }
        if frame.seq > expected {
            return ApplyOutcome::Gap {
                expected,
                got: frame.seq,
            };
        }
        self.seq = frame.seq;
        self.write_patch(&frame.patch);
        ApplyOutcome::Applied
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        if (rows, cols) == (self.rows, self.cols) {
            return;
        }
        self.rows = rows.max(1);
        self.cols = cols.max(1);
        self.grid.resize(self.rows as usize, Vec::new());
    }

    fn write_patch(&mut self, patch: &PtyScreenPatch) {
        for row in &patch.rows {
            if let Some(slot) = self.grid.get_mut(row.row as usize) {
                slot.clone_from(&row.runs);
            }
        }
        if let Some(c) = patch.cursor {
            self.cursor = c;
        }
        if let Some(alt) = patch.alt_screen {
            self.alt_screen = alt;
        }
        if let Some(t) = &patch.title {
            self.title = Some(t.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::pty::{
        PtyAttachResponse, PtyRowPatch, PtyScreenFrame, PtyScreenPatch, PtyStyleRun,
    };

    const SID: &str = "s";

    fn run(text: &str) -> PtyStyleRun {
        PtyStyleRun {
            text: text.into(),
            fg: Default::default(),
            bg: Default::default(),
            attrs: Default::default(),
        }
    }

    fn frame(seq: u64, row: u16, text: &str) -> PtyScreenFrame {
        frame_for(SID, seq, row, text)
    }

    fn frame_for(session_id: &str, seq: u64, row: u16, text: &str) -> PtyScreenFrame {
        PtyScreenFrame {
            session_id: session_id.into(),
            seq,
            patch: PtyScreenPatch {
                rows: vec![PtyRowPatch {
                    row,
                    runs: vec![run(text)],
                }],
                ..Default::default()
            },
        }
    }

    #[test]
    fn frames_in_order_apply() {
        let mut s = ClientScreen::new(4, 20, 0, SID);
        assert!(matches!(s.apply(frame(1, 0, "a")), ApplyOutcome::Applied));
        assert!(matches!(s.apply(frame(2, 1, "b")), ApplyOutcome::Applied));
        assert_eq!(s.row_text(0), "a");
        assert_eq!(s.row_text(1), "b");
    }

    /// The gateway event bus is a bounded broadcast that drops for lagging
    /// subscribers, so a gap is expected traffic, not an exceptional case.
    #[test]
    fn a_gap_is_reported_rather_than_silently_misapplied() {
        let mut s = ClientScreen::new(4, 20, 0, SID);
        let _ = s.apply(frame(1, 0, "a"));
        match s.apply(frame(3, 1, "c")) {
            ApplyOutcome::Gap { expected, got } => {
                assert_eq!((expected, got), (2, 3));
            }
            other => panic!("a missed frame must be reported, got {other:?}"),
        }
        assert_eq!(s.row_text(1), "", "a gapped frame must not be applied");
    }

    /// A frame at or below the current seq is already represented in this
    /// screen's state. Reporting it as a `Gap` would spend a full
    /// `pty.attach` round trip on a frame that needed nothing, and — because
    /// gaps resync from a snapshot rather than advancing past the offending
    /// frame — a repeating stale frame would resync forever without making
    /// progress.
    #[test]
    fn a_stale_frame_is_discarded_not_reported_as_a_gap() {
        let mut s = ClientScreen::new(4, 20, 0, SID);
        assert!(matches!(s.apply(frame(1, 0, "a")), ApplyOutcome::Applied));
        match s.apply(frame(1, 0, "a-again")) {
            ApplyOutcome::Discarded { seq } => assert_eq!(seq, 1),
            other => panic!("a stale frame must be discarded, not {other:?}"),
        }
        assert_eq!(s.row_text(0), "a", "a stale frame must not be reapplied");
        assert_eq!(s.seq(), 1, "a discarded frame must not move seq");
    }

    /// `pty.screen` carries every session's frames on one topic. A frame for
    /// a session this screen does not track must not be read against this
    /// screen's `seq` — that would read as a gap for a stream this screen
    /// never subscribed to and resync it needlessly (or, if the other
    /// session's `seq` happens to line up, silently paint its content here).
    #[test]
    fn a_frame_for_a_different_session_is_ignored() {
        let mut s = ClientScreen::new(4, 20, 0, SID);
        assert!(matches!(s.apply(frame(1, 0, "a")), ApplyOutcome::Applied));
        let outcome = s.apply(frame_for("other-session", 2, 1, "intruder"));
        assert!(
            matches!(outcome, ApplyOutcome::WrongSession),
            "got {outcome:?}"
        );
        assert_eq!(
            s.row_text(1),
            "",
            "another session's frame must not be painted"
        );
        assert_eq!(s.seq(), 1, "another session's frame must not advance seq");
    }

    /// The same filter must hold while an attach is in flight: a wrongly
    /// addressed frame must not be buffered for replay either.
    #[test]
    fn a_frame_for_a_different_session_is_ignored_even_mid_attach() {
        let mut s = ClientScreen::new(4, 20, 0, SID);
        s.begin_attach();
        let outcome = s.apply(frame_for("other-session", 1, 0, "intruder"));
        assert!(
            matches!(outcome, ApplyOutcome::WrongSession),
            "got {outcome:?}"
        );
        let attach_outcome = s.finish_attach(PtyAttachResponse {
            seq: 0,
            rows: 4,
            cols: 20,
            patch: PtyScreenPatch::default(),
            scrollback_len: 0,
        });
        assert!(
            matches!(attach_outcome, AttachOutcome::Resynced),
            "got {attach_outcome:?}"
        );
        assert_eq!(
            s.row_text(0),
            "",
            "a different session's frame must not be replayed"
        );
    }

    /// The snapshot is taken at seq N while frames N+1.. are already in
    /// flight. Without buffer-and-replay those frames are lost and the screen
    /// is silently wrong with no error anywhere.
    #[test]
    fn frames_arriving_during_attach_are_replayed_after_the_snapshot() {
        let mut s = ClientScreen::new(4, 20, 0, SID);
        s.begin_attach();
        assert!(matches!(
            s.apply(frame(6, 2, "late")),
            ApplyOutcome::Buffered
        ));
        let outcome = s.finish_attach(PtyAttachResponse {
            seq: 5,
            rows: 4,
            cols: 20,
            patch: PtyScreenPatch {
                rows: vec![PtyRowPatch {
                    row: 0,
                    runs: vec![run("snap")],
                }],
                ..Default::default()
            },
            scrollback_len: 0,
        });
        assert!(
            matches!(outcome, AttachOutcome::Resynced),
            "got {outcome:?}"
        );
        assert_eq!(s.row_text(0), "snap");
        assert_eq!(s.row_text(2), "late", "in-flight frames must be replayed");
        assert_eq!(s.seq(), 6);
    }

    /// A frame at or below the snapshot's seq is already represented in the
    /// snapshot; replaying it would double-apply.
    #[test]
    fn buffered_frames_at_or_below_the_snapshot_seq_are_dropped_on_replay() {
        let mut s = ClientScreen::new(4, 20, 0, SID);
        s.begin_attach();
        let _ = s.apply(frame(5, 3, "stale"));
        let outcome = s.finish_attach(PtyAttachResponse {
            seq: 5,
            rows: 4,
            cols: 20,
            patch: PtyScreenPatch::default(),
            scrollback_len: 0,
        });
        assert!(
            matches!(outcome, AttachOutcome::Resynced),
            "got {outcome:?}"
        );
        assert_eq!(
            s.row_text(3),
            "",
            "a frame already in the snapshot must be dropped"
        );
    }

    /// A hole *inside* the replay buffer — not between the snapshot and the
    /// first buffered frame, but between two buffered frames — must be
    /// caught too. Buffer `[101, 103]` with 102 dropped by the bounded
    /// broadcast: skipping straight to 103 would leave `seq` contiguous
    /// again with nothing to show a live frame ever gapped on, so whatever
    /// only frame 102 touched would be wrong forever with no error anywhere.
    #[test]
    fn a_hole_inside_the_replay_buffer_is_reported_and_does_not_advance_past_it() {
        let mut s = ClientScreen::new(4, 20, 0, SID);
        s.begin_attach();
        assert!(matches!(
            s.apply(frame(101, 1, "a")),
            ApplyOutcome::Buffered
        ));
        // seq 102 was dropped by the broadcast bus before it ever arrived.
        assert!(matches!(
            s.apply(frame(103, 2, "c")),
            ApplyOutcome::Buffered
        ));
        let outcome = s.finish_attach(PtyAttachResponse {
            seq: 100,
            rows: 4,
            cols: 20,
            patch: PtyScreenPatch::default(),
            scrollback_len: 0,
        });
        match outcome {
            AttachOutcome::Gap { expected, got } => assert_eq!((expected, got), (102, 103)),
            other => panic!("a hole in the replay buffer must be reported, got {other:?}"),
        }
        assert_eq!(
            s.row_text(1),
            "a",
            "the frame before the hole must still apply"
        );
        assert_eq!(
            s.row_text(2),
            "",
            "the frame after the hole must not be applied"
        );
        assert_eq!(s.seq(), 101, "seq must not advance past the hole");
    }

    #[test]
    fn a_resize_in_the_snapshot_is_adopted_and_grown_rows_are_addressable() {
        let mut s = ClientScreen::new(4, 20, 0, SID);
        s.begin_attach();
        let outcome = s.finish_attach(PtyAttachResponse {
            seq: 1,
            rows: 10,
            cols: 60,
            patch: PtyScreenPatch {
                rows: vec![PtyRowPatch {
                    row: 9,
                    runs: vec![run("bottom")],
                }],
                ..Default::default()
            },
            scrollback_len: 0,
        });
        assert!(
            matches!(outcome, AttachOutcome::Resynced),
            "got {outcome:?}"
        );
        assert_eq!(s.dims(), (10, 60));
        assert_eq!(
            s.row_text(9),
            "bottom",
            "a row beyond the old geometry must be addressable and carry the snapshot's content"
        );
    }

    /// A snapshot older than what this screen already has must not move
    /// `seq` (or grid content) backward. This should not happen in normal
    /// operation (only one attach in flight at a time, `seq` monotonic on
    /// the server) but is wire input, not a local invariant.
    #[test]
    fn a_stale_snapshot_does_not_move_seq_backward() {
        let mut s = ClientScreen::new(4, 20, 5, SID);
        let outcome = s.finish_attach(PtyAttachResponse {
            seq: 2,
            rows: 4,
            cols: 20,
            patch: PtyScreenPatch {
                rows: vec![PtyRowPatch {
                    row: 0,
                    runs: vec![run("old")],
                }],
                ..Default::default()
            },
            scrollback_len: 0,
        });
        assert!(
            matches!(outcome, AttachOutcome::Resynced),
            "got {outcome:?}"
        );
        assert_eq!(
            s.seq(),
            5,
            "seq must not regress to the stale snapshot's seq"
        );
        assert_eq!(
            s.row_text(0),
            "",
            "a stale snapshot's content must not overwrite current state"
        );
    }

    /// `expected` must saturate rather than overflow when `seq` is already
    /// at the top of its range — `seq` comes from the wire, not a value this
    /// screen controls, so `u64::MAX` is a value to guard against, not one
    /// to trust never arrives. A debug build panics on `u64::MAX + 1` and a
    /// release build silently wraps to 0 (after which every future frame
    /// would gap forever); the panic alone is enough to prove this test
    /// would have caught the bug, since it happens computing `expected`
    /// before `frame.seq` is even inspected.
    #[test]
    fn seq_at_the_maximum_does_not_overflow_on_the_next_apply() {
        let mut s = ClientScreen::new(4, 20, u64::MAX, SID);
        match s.apply(frame(u64::MAX - 1, 0, "x")) {
            ApplyOutcome::Discarded { seq } => assert_eq!(seq, u64::MAX - 1),
            other => panic!("must not panic or wrap, got {other:?}"),
        }
        assert_eq!(s.seq(), u64::MAX, "seq must not move for a discarded frame");
    }
}
