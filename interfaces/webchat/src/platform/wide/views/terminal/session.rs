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
    Gap { expected: u64, got: u64 },
    /// Held until the in-flight attach lands.
    Buffered,
    /// `frame.seq` is at or below the already-applied seq: a duplicate
    /// delivery, or a frame that predates the last attach's snapshot. Not an
    /// error and not a gap — there is nothing to resync, the frame's content
    /// is already reflected in this screen's state.
    Discarded { seq: u64 },
    /// `frame.session_id` does not match the session this screen tracks.
    /// Ignored, not counted against this screen's `seq`.
    WrongSession,
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
    #[must_use]
    pub fn new(rows: u16, cols: u16, seq: u64, session_id: impl Into<String>) -> Self {
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

    /// Adopt the snapshot, then replay every buffered frame newer than it.
    ///
    /// A buffered frame at or below `resp.seq` is already represented in the
    /// snapshot (it was captured on the server after that frame was
    /// produced); replaying it would double-apply, so it is dropped here —
    /// same rule as [`ApplyOutcome::Discarded`], applied on the way out of
    /// the buffer instead of on the way in.
    pub fn finish_attach(&mut self, resp: PtyAttachResponse) {
        self.resize(resp.rows, resp.cols);
        self.seq = resp.seq;
        self.write_patch(&resp.patch);
        let buffered = self.pending.take().unwrap_or_default();
        for frame in buffered {
            if frame.seq > self.seq {
                self.seq = frame.seq;
                self.write_patch(&frame.patch);
            }
        }
    }

    /// Apply one server frame.
    ///
    /// `pty.screen` is one topic carrying every session's frames, so this
    /// filters on `session_id` itself rather than trusting the caller to
    /// pre-filter: a caller that forgot to filter would otherwise let
    /// another session's `seq` be read against this screen's counter,
    /// producing a spurious [`ApplyOutcome::Gap`] (or, worse, painting that
    /// session's content here if the sequence numbers happened to line up).
    pub fn apply(&mut self, frame: PtyScreenFrame) -> ApplyOutcome {
        if frame.session_id != self.session_id {
            return ApplyOutcome::WrongSession;
        }
        if let Some(buf) = &mut self.pending {
            buf.push(frame);
            return ApplyOutcome::Buffered;
        }
        let expected = self.seq + 1;
        if frame.seq < expected {
            return ApplyOutcome::Discarded { seq: frame.seq };
        }
        if frame.seq > expected {
            return ApplyOutcome::Gap { expected, got: frame.seq };
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
    use aleph_protocol::pty::{PtyAttachResponse, PtyRowPatch, PtyScreenFrame, PtyScreenPatch, PtyStyleRun};

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
                rows: vec![PtyRowPatch { row, runs: vec![run(text)] }],
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
        assert!(matches!(outcome, ApplyOutcome::WrongSession), "got {outcome:?}");
        assert_eq!(s.row_text(1), "", "another session's frame must not be painted");
        assert_eq!(s.seq(), 1, "another session's frame must not advance seq");
    }

    /// The same filter must hold while an attach is in flight: a wrongly
    /// addressed frame must not be buffered for replay either.
    #[test]
    fn a_frame_for_a_different_session_is_ignored_even_mid_attach() {
        let mut s = ClientScreen::new(4, 20, 0, SID);
        s.begin_attach();
        let outcome = s.apply(frame_for("other-session", 1, 0, "intruder"));
        assert!(matches!(outcome, ApplyOutcome::WrongSession), "got {outcome:?}");
        s.finish_attach(PtyAttachResponse {
            seq: 0,
            rows: 4,
            cols: 20,
            patch: PtyScreenPatch::default(),
            scrollback_len: 0,
        });
        assert_eq!(s.row_text(0), "", "a different session's frame must not be replayed");
    }

    /// The snapshot is taken at seq N while frames N+1.. are already in
    /// flight. Without buffer-and-replay those frames are lost and the screen
    /// is silently wrong with no error anywhere.
    #[test]
    fn frames_arriving_during_attach_are_replayed_after_the_snapshot() {
        let mut s = ClientScreen::new(4, 20, 0, SID);
        s.begin_attach();
        assert!(matches!(s.apply(frame(6, 2, "late")), ApplyOutcome::Buffered));
        s.finish_attach(PtyAttachResponse {
            seq: 5,
            rows: 4,
            cols: 20,
            patch: PtyScreenPatch {
                rows: vec![PtyRowPatch { row: 0, runs: vec![run("snap")] }],
                ..Default::default()
            },
            scrollback_len: 0,
        });
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
        s.finish_attach(PtyAttachResponse {
            seq: 5,
            rows: 4,
            cols: 20,
            patch: PtyScreenPatch::default(),
            scrollback_len: 0,
        });
        assert_eq!(s.row_text(3), "", "a frame already in the snapshot must be dropped");
    }

    #[test]
    fn a_resize_in_the_snapshot_is_adopted() {
        let mut s = ClientScreen::new(4, 20, 0, SID);
        s.begin_attach();
        s.finish_attach(PtyAttachResponse {
            seq: 1,
            rows: 10,
            cols: 60,
            patch: PtyScreenPatch::default(),
            scrollback_len: 0,
        });
        assert_eq!(s.dims(), (10, 60));
    }
}
