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

use aleph_protocol::pty::{
    PtyAttachResponse, PtyListResponse, PtyScreenFrame, PtyScreenPatch, PtySessionInfo, PtyStyleRun,
};

/// A readable authorized list and a failed read are distinct facts.
/// Neither result authorizes creating a shell; creation requires a gesture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListOutcome {
    /// The response was READ. The rows are exactly what the server said, open
    /// and closed alike; an empty vector here is a CONFIRMED absence.
    Read(Vec<PtySessionInfo>),
    /// No usable answer: the call failed, or it returned something this
    /// client cannot read. Show it; do not act on it. Notably NOT a claim
    /// that the server has no sessions — it may have several.
    Fail(String),
}

/// Classify a list without treating transport or decode failure as emptiness.
#[must_use]
pub fn resolve_session_list(list_result: Result<serde_json::Value, String>) -> ListOutcome {
    let value = match list_result {
        Ok(value) => value,
        Err(e) => return ListOutcome::Fail(format!("pty.list failed: {e}")),
    };
    match serde_json::from_value::<PtyListResponse>(value) {
        Ok(list) => ListOutcome::Read(list.sessions),
        Err(e) => ListOutcome::Fail(format!("pty.list decode failed: {e}")),
    }
}

/// The end marker of a bracketed paste lives with the encoder that writes it:
/// [`crate::input::encode_paste`] is the single place that knows both how to
/// close a paste and how to remove a hostile one from a payload.

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

/// A whole contiguous capture, or a request for another snapshot. Failure
/// preserves every part of the previous confirmed paint and sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachOutcome {
    /// The snapshot was adopted, and every frame buffered while the attach
    /// was in flight — if any — replayed with `seq` advancing by exactly one
    /// each time. There is nothing for the caller to do.
    Resynced,
    /// No part of an incomplete replay is published.
    Gap { expected: u64, got: u64 },
    /// Staging overflowed or the snapshot predates the confirmed screen.
    RecoveryRequired,
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
    /// `CSI ?25 h/l`. Starts TRUE: a terminal that has said nothing about its
    /// cursor has one, and defaulting to hidden would paint a frozen-looking
    /// screen on every fresh attach.
    cursor_visible: bool,
    /// `CSI ?2004 h/l`, or `None` while the server has not said.
    ///
    /// Deliberately `Option<bool>` and not `bool`: the patch field is `Some`
    /// only when the mode CHANGED, so "we have not been told" is a real state
    /// here and it is not the same as "off".
    /// [`crate::input::encode_paste`] treats the two identically today, but
    /// it must be able to tell them apart to keep being able to (判据 §8).
    bracketed_paste: Option<bool>,
    /// `Some` while an attach is in flight; frames land here instead of on
    /// the grid, because the snapshot they must be ordered against has not
    /// arrived yet.
    pending: Option<Vec<PtyScreenFrame>>,
    pending_bytes: usize,
    pending_lost: bool,
}

impl ClientScreen {
    /// `session_id` is the PTY session this screen tracks. [`apply`](Self::apply)
    /// enforces it against every incoming frame — `pty.screen` is one topic
    /// shared by every session, so a screen that trusted its caller to
    /// pre-filter would read another session's `seq` as a gap in its own.
    ///
    /// `rows`/`cols` are clamped to at least 1, same as `resize` —
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
            cursor_visible: true,
            bracketed_paste: None,
            pending: None,
            pending_bytes: 0,
            pending_lost: false,
        }
    }

    #[must_use]
    pub const fn dims(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    #[must_use]
    #[allow(dead_code)] // no reader since the terminal herdr removal (5dd337df1)
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
    #[allow(dead_code)] // no reader since the terminal herdr removal (5dd337df1)
    pub const fn alt_screen(&self) -> bool {
        self.alt_screen
    }

    /// Whether the program wants its cursor drawn (`CSI ?25 h/l`).
    #[must_use]
    pub const fn cursor_visible(&self) -> bool {
        self.cursor_visible
    }

    /// Whether the program has asked for bracketed paste, or `None` if it has
    /// not said either way. Feed it to [`crate::input::encode_paste`]; do not
    /// collapse the `None` here.
    #[must_use]
    pub const fn bracketed_paste(&self) -> Option<bool> {
        self.bracketed_paste
    }

    #[must_use]
    pub fn row_runs(&self, row: u16) -> &[PtyStyleRun] {
        self.grid.get(row as usize).map_or(&[], Vec::as_slice)
    }

    /// Row as plain text. Test- and selection-facing.
    #[must_use]
    #[allow(dead_code)] // used by tests only — no reader since the terminal herdr
                        // removal (5dd337df1)
    pub fn row_text(&self, row: u16) -> String {
        let s: String = self.row_runs(row).iter().map(|r| r.text.as_str()).collect();
        s.trim_end().to_string()
    }

    pub fn begin_attach(&mut self) {
        self.pending = Some(Vec::new());
        self.pending_bytes = 0;
        self.pending_lost = false;
    }

    pub fn abort_attach(&mut self) {
        self.pending = None;
        self.pending_bytes = 0;
        self.pending_lost = false;
    }

    /// Validate the entire replay before publishing any of it. The old grid
    /// remains authoritative paint until a contiguous capture is available.
    #[must_use]
    pub fn finish_attach(&mut self, resp: PtyAttachResponse) -> AttachOutcome {
        let lost = self.pending_lost;
        let buffered = self.pending.take().unwrap_or_default();
        self.abort_attach();
        if lost || resp.seq < self.seq {
            return AttachOutcome::RecoveryRequired;
        }
        let mut seq = resp.seq;
        for frame in &buffered {
            if frame.seq <= seq {
                continue;
            }
            let expected = seq.saturating_add(1);
            if frame.seq != expected {
                return AttachOutcome::Gap {
                    expected,
                    got: frame.seq,
                };
            }
            seq = frame.seq;
        }
        self.seq = resp.seq;
        self.settle(resp.rows, resp.cols, &resp.patch);
        for frame in buffered {
            if frame.seq > self.seq {
                self.seq = frame.seq;
                self.settle(frame.rows, frame.cols, &frame.patch);
            }
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
            let bytes = serde_json::to_vec(&frame).map_or(usize::MAX, |v| v.len());
            if self.pending_lost
                || buf.len() >= 256
                || bytes > (8 * 1024 * 1024_usize).saturating_sub(self.pending_bytes)
            {
                buf.clear();
                self.pending_bytes = 0;
                self.pending_lost = true;
                return ApplyOutcome::Gap {
                    expected: self.seq.saturating_add(1),
                    got: frame.seq,
                };
            }
            self.pending_bytes += bytes;
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
        self.settle(frame.rows, frame.cols, &frame.patch);
        ApplyOutcome::Applied
    }

    /// Adopt this frame's geometry, then write its rows.
    ///
    /// The two steps are one method rather than two calls because the order
    /// between them is the whole correctness argument: a naive `write` half's
    /// `grid.get_mut(row)` silently returns `None` for a row past the current
    /// bottom, so writing before resizing drops rows with no error anywhere
    /// and no gap to trigger a re-attach. There were two call sites for the
    /// write half (`apply` and `finish_attach`'s replay loop) and a rule that
    /// only one of them followed; there is now no way to write a patch
    /// without handing over the geometry it belongs to.
    fn settle(&mut self, rows: u16, cols: u16, patch: &PtyScreenPatch) {
        if (rows, cols) != (self.rows, self.cols) {
            self.rows = rows.max(1);
            self.cols = cols.max(1);
            self.grid.resize(self.rows as usize, Vec::new());
        }
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
        // Both of these are "Some only when it changed", like `alt_screen`
        // above: a `None` leaves the stored value alone. It must NOT be read
        // as `false` — for the cursor that would blink it out of existence on
        // every frame that did not mention it, and for bracketed paste it
        // would turn "the program asked for it" back into "unknown".
        if let Some(visible) = patch.cursor_visible {
            self.cursor_visible = visible;
        }
        if let Some(bracketed) = patch.bracketed_paste {
            self.bracketed_paste = Some(bracketed);
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

    /// A frame carrying nothing but `patch`, at this screen's geometry.
    fn patch_frame(seq: u64, patch: PtyScreenPatch) -> PtyScreenFrame {
        PtyScreenFrame {
            session_id: SID.to_string(),
            seq,
            rows: 4,
            cols: 10,
            patch,
        }
    }

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

    /// The dimensions every existing test's screen already has, so a frame
    /// built by `frame_for` resizes nothing and the suite is unchanged by
    /// this field's arrival. Any other value silently resizes on frame one.
    const FIXTURE_DIMS: (u16, u16) = (4, 20);

    fn frame_for(session_id: &str, seq: u64, row: u16, text: &str) -> PtyScreenFrame {
        frame_sized(session_id, seq, FIXTURE_DIMS.0, FIXTURE_DIMS.1, row, text)
    }

    fn frame_sized(
        session_id: &str,
        seq: u64,
        rows: u16,
        cols: u16,
        row: u16,
        text: &str,
    ) -> PtyScreenFrame {
        PtyScreenFrame {
            session_id: session_id.into(),
            seq,
            rows,
            cols,
            patch: PtyScreenPatch {
                rows: vec![PtyRowPatch {
                    row,
                    runs: vec![run(text)],
                }],
                ..Default::default()
            },
        }
    }

    /// A resize the client did not ask for still has to land. Sizing is
    /// smallest-wins across clients, so a client can be shrunk by someone
    /// else joining without ever calling `pty.resize` itself -- and a grow
    /// leaves `seq` contiguous, so nothing ever gaps and nothing self-heals.
    #[test]
    fn a_frame_carrying_new_geometry_grows_the_screen_before_its_rows_land() {
        let mut s = ClientScreen::new(24, 80, 5, SID);

        assert_eq!(
            s.apply(frame_sized(SID, 6, 40, 100, 39, "bottom")),
            ApplyOutcome::Applied
        );

        assert_eq!(s.dims(), (40, 100));
        // The ordering assertion: adopting AFTER write_patch leaves the grid
        // 24 rows long, `get_mut(39)` returns None, and the row is dropped
        // with no error anywhere.
        assert_eq!(
            s.row_text(39),
            "bottom",
            "the new geometry must be adopted before its rows are written"
        );
    }

    /// Shrink is the other half and it is not symmetric: rows past the new
    /// bottom must go away, or the renderer keeps painting content the
    /// server no longer has.
    #[test]
    fn a_shrinking_frame_drops_the_rows_below_the_new_bottom() {
        let mut s = ClientScreen::new(40, 100, 5, SID);
        assert_eq!(
            s.apply(frame_sized(SID, 6, 40, 100, 39, "gone")),
            ApplyOutcome::Applied
        );
        assert_eq!(s.row_text(39), "gone");

        // A frame with no row patches at all, carrying only the new size.
        let shrink = PtyScreenFrame {
            session_id: SID.into(),
            seq: 7,
            rows: 24,
            cols: 80,
            patch: PtyScreenPatch::default(),
        };
        assert_eq!(s.apply(shrink), ApplyOutcome::Applied);

        assert_eq!(s.dims(), (24, 80));
        assert_eq!(
            s.row_text(39),
            "",
            "a row past the new bottom must not survive"
        );
    }

    /// A frame we are throwing away must not move the geometry either. Its
    /// dimensions are as old as its content.
    #[test]
    fn a_discarded_frame_does_not_move_the_geometry() {
        let mut s = ClientScreen::new(24, 80, 5, SID);
        let stale = frame_sized(SID, 3, 40, 100, 0, "old");
        assert!(matches!(s.apply(stale), ApplyOutcome::Discarded { .. }));
        assert_eq!(
            s.dims(),
            (24, 80),
            "a discarded frame carries stale dimensions too"
        );
    }

    /// The replay path, which is the one a fix written only into `apply`
    /// misses entirely. Frames that arrive while an attach is in flight are
    /// buffered and replayed by `finish_attach` -- and a resize is exactly
    /// what can happen in that window, since sizing is smallest-wins and
    /// another client leaving grows this one without it calling anything.
    ///
    /// Without `settle`, replay writes rows straight into a grid still sized
    /// to the attach snapshot, `get_mut` returns None past the old bottom,
    /// the rows vanish, and `seq` advances anyway -- so nothing gaps and
    /// nothing ever heals. That is the same defect this task exists to fix,
    /// reproduced inside the code that was written to fix it.
    #[test]
    fn a_buffered_frame_that_grows_the_screen_lands_its_rows_on_replay() {
        let mut s = ClientScreen::new(24, 80, 5, SID);
        s.begin_attach();

        // Arrives mid-attach, carrying a geometry the snapshot will not have.
        assert_eq!(
            s.apply(frame_sized(SID, 7, 40, 100, 39, "late")),
            ApplyOutcome::Buffered
        );

        // The snapshot is older and smaller: 24x80 at seq 6.
        let resp = PtyAttachResponse {
            seq: 6,
            rows: 24,
            cols: 80,
            patch: PtyScreenPatch::default(),
            scrollback_len: 0,
        };
        assert_eq!(s.finish_attach(resp), AttachOutcome::Resynced);

        assert_eq!(
            s.dims(),
            (40, 100),
            "the replayed frame's geometry must be adopted too"
        );
        assert_eq!(
            s.row_text(39),
            "late",
            "a replayed row past the snapshot's bottom must not be silently dropped"
        );
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

    #[test]
    fn aborting_an_attach_allows_later_frames_to_apply() {
        let mut s = ClientScreen::new(4, 20, 0, SID);
        s.begin_attach();
        assert!(matches!(
            s.apply(frame(1, 0, "buffered")),
            ApplyOutcome::Buffered
        ));

        s.abort_attach();

        assert!(matches!(
            s.apply(frame(1, 0, "applied")),
            ApplyOutcome::Applied
        ));
        assert_eq!(s.row_text(0), "applied");
    }

    /// The snapshot is taken at seq N while frames N+1.. are already in
    /// flight. Without buffer-and-replay those frames are lost and the screen
    /// is silently wrong with no error anywhere.
    #[test]
    fn attach_staging_count_and_byte_overflow_are_bounded_and_sticky() {
        for bytes in [false, true] {
            let mut s = ClientScreen::new(4, 20, 10, SID);
            let _ = s.apply(frame(11, 0, "old"));
            s.begin_attach();
            if bytes {
                assert!(matches!(
                    s.apply(frame(12, 0, &"x".repeat(8 * 1024 * 1024))),
                    ApplyOutcome::Gap { .. }
                ));
            } else {
                for seq in 12..268 {
                    assert_eq!(s.apply(frame(seq, 0, "x")), ApplyOutcome::Buffered);
                }
                assert!(matches!(
                    s.apply(frame(268, 0, "overflow")),
                    ApplyOutcome::Gap { .. }
                ));
            }
            assert_eq!(s.pending.as_ref().unwrap().len(), 0);
            assert_eq!(s.pending_bytes, 0);
            let _ = s.apply(frame(269, 0, "not restored"));
            assert_eq!(s.pending.as_ref().unwrap().len(), 0);
            assert_eq!(
                s.finish_attach(PtyAttachResponse {
                    seq: 269,
                    rows: 4,
                    cols: 20,
                    patch: PtyScreenPatch::default(),
                    scrollback_len: 0,
                }),
                AttachOutcome::RecoveryRequired
            );
            assert_eq!(s.row_text(0), "old");
        }
    }

    #[test]
    fn quiet_replay_gap_preserves_old_paint() {
        let mut s = ClientScreen::new(4, 20, 10, SID);
        let _ = s.apply(frame(11, 0, "old"));
        s.begin_attach();
        let _ = s.apply(frame(12, 0, "partial"));
        let _ = s.apply(frame(14, 0, "gap"));
        assert!(matches!(
            s.finish_attach(PtyAttachResponse {
                seq: 11,
                rows: 4,
                cols: 20,
                patch: PtyScreenPatch::default(),
                scrollback_len: 0,
            }),
            AttachOutcome::Gap { .. }
        ));
        assert_eq!(s.row_text(0), "old");
        assert_eq!(s.seq(), 11);
    }

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
    fn a_hole_inside_replay_preserves_entire_confirmed_capture() {
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
            "",
            "an incomplete capture must not publish even its contiguous prefix"
        );
        assert_eq!(
            s.row_text(2),
            "",
            "the frame after the hole must not be applied"
        );
        assert_eq!(s.seq(), 0, "an incomplete capture preserves the old seq");
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
            matches!(outcome, AttachOutcome::RecoveryRequired),
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

    /// D11 / spec §5. `Some(true)` is the ONLY value that wraps.
    ///
    /// `None` is the case that matters: `PtyScreenPatch::bracketed_paste` is
    /// `Some` only when the mode CHANGED, so a paste that happens before the
    /// first such frame has genuinely not been told anything. Reading that as
    /// "on" leaves `ESC[200~` sitting in the user's command line — a visible
    /// corruption of what they pasted — while reading it as "off" only gives
    /// up a hint. Unknown takes the weaker assumption (判据 §8).
    ///
    /// Reddens if `None` or `Some(false)` starts wrapping, if `Some(true)`
    /// stops, or if the end marker survives inside a wrapped payload. That
    /// coverage now lives with the encoder, in
    /// [`crate::input::encode_paste`].

    /// C9. `?25 l` hides the cursor; `?25 h` shows it again; a patch that says
    /// NOTHING leaves it alone.
    ///
    /// The third case is the one that bites: `cursor_visible` is `Some` only
    /// when the mode changed, so reading a `None` as `false` would blink the
    /// cursor out on every frame that did not mention it. `ClientScreen`
    /// starts `true` for the same reason — a fresh attach has been told
    /// nothing, and a shell prompt with no cursor reads as a frozen terminal.
    ///
    /// "Render skips it" is asserted through `render::cursor_rect`, which is
    /// where `paint` gets the answer: a `CanvasRenderingContext2d` cannot be
    /// constructed off the browser, so there is nowhere else an assertion
    /// about the painted result could live.
    #[test]
    fn cursor_visible_false_is_stored_and_render_skips_the_cursor() {
        use crate::render::{cursor_rect, CellMetrics};

        let metrics = CellMetrics {
            width: 8.0,
            height: 16.0,
            font_px: 14.0,
            font_family: "monospace".to_string(),
        };

        let mut s = ClientScreen::new(4, 10, 0, SID);
        assert!(
            s.cursor_visible(),
            "a screen that has been told nothing has a cursor"
        );
        assert!(cursor_rect(&s, &metrics).is_some());

        // ?25 l
        assert_eq!(
            s.apply(patch_frame(
                1,
                PtyScreenPatch {
                    cursor_visible: Some(false),
                    ..Default::default()
                }
            )),
            ApplyOutcome::Applied
        );
        assert!(!s.cursor_visible());
        assert_eq!(
            cursor_rect(&s, &metrics),
            None,
            "a hidden cursor has no rectangle to paint"
        );

        // A frame that says nothing about the cursor must NOT re-show it.
        assert_eq!(
            s.apply(patch_frame(2, PtyScreenPatch::default())),
            ApplyOutcome::Applied
        );
        assert!(
            !s.cursor_visible(),
            "`None` means unchanged, not `true` — otherwise the cursor \
             reappears on the next frame that happens to be silent about it"
        );

        // ?25 h
        assert_eq!(
            s.apply(patch_frame(
                3,
                PtyScreenPatch {
                    cursor_visible: Some(true),
                    ..Default::default()
                }
            )),
            ApplyOutcome::Applied
        );
        assert!(s.cursor_visible());
        assert!(cursor_rect(&s, &metrics).is_some());
    }

    /// The paste mode is a THREE-state fact on this screen and stays one:
    /// unknown, off, on. Collapsing `None` into `false` here is what would
    /// make `encode_paste`'s `None` arm unreachable and its test vacuous.
    #[test]
    fn bracketed_paste_starts_unknown_and_only_a_patch_moves_it() {
        let mut s = ClientScreen::new(4, 10, 0, SID);
        assert_eq!(s.bracketed_paste(), None);

        assert_eq!(
            s.apply(patch_frame(
                1,
                PtyScreenPatch {
                    bracketed_paste: Some(true),
                    ..Default::default()
                }
            )),
            ApplyOutcome::Applied
        );
        assert_eq!(s.bracketed_paste(), Some(true));

        assert_eq!(
            s.apply(patch_frame(2, PtyScreenPatch::default())),
            ApplyOutcome::Applied
        );
        assert_eq!(
            s.bracketed_paste(),
            Some(true),
            "a silent frame leaves the mode alone"
        );

        assert_eq!(
            s.apply(patch_frame(
                3,
                PtyScreenPatch {
                    bracketed_paste: Some(false),
                    ..Default::default()
                }
            )),
            ApplyOutcome::Applied
        );
        assert_eq!(s.bracketed_paste(), Some(false));
    }

    /// The finding this function exists for. A payload the client cannot
    /// decode is the server saying something unreadable — it is NOT the
    /// server saying "you have no sessions". Reading it as the latter makes
    /// the view spawn a SECOND shell beside the live one, with the first
    /// one's screen still on the server and nothing on the page pointing at
    /// it (判据 §8: an `Err` may only say "I do not know").
    #[test]
    fn a_malformed_list_payload_is_a_failure_not_an_empty_list() {
        let outcome = resolve_session_list(Ok(serde_json::json!({ "sessions": "not an array" })));
        match outcome {
            ListOutcome::Fail(msg) => assert!(
                !msg.is_empty(),
                "the message is what names the remedy; an empty one is a blank error card"
            ),
            other => panic!("a payload we cannot read must not become a spawn, got {other:?}"),
        }
    }

    /// A row missing a key the client requires is the same situation as a
    /// wholly malformed body, and must not degrade into a duplicate shell
    /// either. Kept distinct from the case above because this is the shape a
    /// genuine version skew produces, and it is the one that looks most like
    /// a legitimately empty list.
    #[test]
    fn a_row_the_client_cannot_decode_is_a_failure_not_an_empty_list() {
        let outcome = resolve_session_list(Ok(serde_json::json!({
            "sessions": [{ "shell": "zsh", "closed": false }]
        })));
        assert!(
            matches!(outcome, ListOutcome::Fail(_)),
            "a row without session_id is unreadable, not an absence"
        );
    }

    /// A list with something live in it is read, kept whole (the closed row
    /// included — `TabModel::reconcile` is what drops those), and does NOT
    /// license a spawn.
    #[test]
    fn a_list_holding_a_live_session_attaches_to_it() {
        let outcome = resolve_session_list(Ok(serde_json::json!({
            "sessions": [
                { "session_id": "dead", "shell": "zsh", "cwd": "", "created_at": 1, "closed": true },
                { "session_id": "live", "shell": "zsh", "cwd": "", "created_at": 2, "closed": false },
            ]
        })));
        let ListOutcome::Read(sessions) = outcome else {
            panic!("a readable list must be Read, got {outcome:?}");
        };
        assert_eq!(
            sessions
                .iter()
                .map(|s| s.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["dead", "live"]
        );
    }

    #[test]
    fn a_transport_error_is_a_failure_not_a_spawn() {
        for message in ["connection reset", "Request timed out", "Not connected"] {
            let outcome = resolve_session_list(Err(message.to_string()));
            match outcome {
                ListOutcome::Fail(msg) => assert!(
                    msg.contains(message),
                    "the error must name what went wrong; got {msg:?}"
                ),
                other => panic!("{message:?} must not become {other:?}"),
            }
        }
    }
}
