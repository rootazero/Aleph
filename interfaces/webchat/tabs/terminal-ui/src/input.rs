//! Ordered terminal text input.
//!
//! Three rules live here, and they are one rule seen from three sides: what
//! the user typed has to reach the PTY in the order they typed it, exactly
//! once, and nothing else may reach it.
//!
//! - [`encode_paste`] — the wire payload of one paste, with the bracketed
//!   framing and the byte limit the server enforces.
//! - [`InputPipeline`] — the per-selection FIFO that serializes writes, the
//!   IME commit rule, and the selection the user copied from.
//! - [`selection_text`] — the text a cell range covers, as the terminal
//!   painted it.
//!
//! The FIFO is deliberately conservative: one write in flight, a bounded
//! number waiting behind it, and no replay of anything that was already
//! dispatched. A terminal that resends a half-typed command after a
//! reconnect is worse than a terminal that drops it.

use crate::recovery::RequestFence;
use crate::render::run_char_columns;
use crate::session::ClientScreen;
use std::collections::VecDeque;

/// The wire limit for one `pty.input` payload, in UTF-8 bytes.
///
/// The server rejects a longer payload before it reaches the PTY
/// (`shared/protocol/src/capability.rs`), so the client rejects it too — and
/// it rejects it BEFORE the write is queued, because half a paste is a
/// corrupted paste.
pub const MAX_INPUT_BYTES: usize = 65_536;

/// The bracketed-paste framing: `ESC[200~` and `ESC[201~`, six bytes each.
pub const PASTE_FRAMING_BYTES: usize = 12;

/// How many writes may wait behind the one in flight.
pub const MAX_QUEUE_ITEMS: usize = 32;

/// How many bytes may wait behind the one in flight.
pub const MAX_QUEUE_BYTES: usize = 256 * 1024;

const PASTE_START: &str = "\x1b[200~";
const PASTE_END: &str = "\x1b[201~";

/// Why one input was not sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputError {
    /// The payload does not fit the wire limit.
    TooLarge { bytes: usize, limit: usize },
    /// The FIFO is full: too many waiting writes, or too many waiting bytes.
    QueueFull,
    /// A dispatched write failed. Its bytes are gone and are not replayed.
    SendFailed,
}

/// Encode one paste into the payload the wire carries.
///
/// Bracketed paste wraps the text in `ESC[200~`/`ESC[201~` when the program
/// asked for it. An inner end marker is dropped, because a marker inside the
/// text would end the paste early.
///
/// The byte limit is ATOMIC: a paste that does not fit is refused whole. It
/// is never split — a split bracketed paste leaves the program waiting for
/// the rest of a paste that will never arrive.
pub fn encode_paste(text: &str, bracketed: Option<bool>) -> Result<String, InputError> {
    let payload = if bracketed == Some(true) {
        format!("{PASTE_START}{}{PASTE_END}", text.replace(PASTE_END, ""))
    } else {
        text.to_string()
    };
    if payload.len() > MAX_INPUT_BYTES {
        return Err(InputError::TooLarge {
            bytes: payload.len(),
            limit: MAX_INPUT_BYTES,
        });
    }
    Ok(payload)
}

/// Is this the platform's COPY chord?
///
/// `Cmd-C` and `Ctrl-Shift-C`. `Ctrl-C` is NOT: in a terminal it is the
/// interrupt, and it stays the interrupt unless there is a selection to copy.
pub fn is_copy_chord(key: &str, ctrl: bool, shift: bool, meta: bool) -> bool {
    if !key.eq_ignore_ascii_case("c") {
        return false;
    }
    meta || (ctrl && shift)
}

/// One write waiting for — or holding — the PTY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedInput {
    /// The exact bytes to put on the wire, as UTF-8.
    pub payload: String,
    /// The selection and connection this write belongs to.
    pub fence: RequestFence,
}

/// A cell range in the painted grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub start_row: u16,
    pub start_col: u16,
    pub end_row: u16,
    pub end_col: u16,
}

impl Selection {
    /// A selection between two cells, normalized so the anchor comes first.
    pub fn new(start_row: u16, start_col: u16, end_row: u16, end_col: u16) -> Self {
        if (end_row, end_col) < (start_row, start_col) {
            Self {
                start_row: end_row,
                start_col: end_col,
                end_row: start_row,
                end_col: start_col,
            }
        } else {
            Self {
                start_row,
                start_col,
                end_row,
                end_col,
            }
        }
    }

    /// A selection that covers nothing — a click without a drag.
    pub fn is_empty(&self) -> bool {
        self.start_row == self.end_row && self.start_col == self.end_col
    }
}

/// The text a selection covers, as the terminal painted it.
///
/// A wide glyph occupies two cells but one character: the continuation cell
/// belongs to the glyph, so copying it must not produce a second character.
pub fn selection_text(screen: &ClientScreen, selection: &Selection) -> String {
    let (rows, _cols) = screen.dims();
    let mut out = String::new();
    let last_row = selection.end_row.min(rows.saturating_sub(1));
    for row in selection.start_row..=last_row {
        if row > selection.start_row {
            out.push('\n');
        }
        let from = if row == selection.start_row {
            selection.start_col as usize
        } else {
            0
        };
        let to = if row == selection.end_row {
            selection.end_col as usize
        } else {
            usize::MAX
        };
        let mut column = 0usize;
        for run in screen.row_runs(row) {
            let run_width = unicode_width::UnicodeWidthStr::width(run.text.as_str());
            let run_end = column + run_width;
            if run_end > from && column < to {
                for (start, end, ch) in run_char_columns(&run.text) {
                    let cell = column + start;
                    let cell_end = column + end;
                    if cell_end > from && cell < to {
                        out.push_str(&ch);
                    }
                }
            }
            column = run_end;
        }
    }
    out
}

/// The per-selection input pipeline.
///
/// One instance per terminal view. It owns the write FIFO, the IME
/// composition state, and the current selection — all of which are
/// invalidated together when the selection or the connection changes.
#[derive(Debug, Default)]
pub struct InputPipeline {
    queue: VecDeque<QueuedInput>,
    queued_bytes: usize,
    in_flight: Option<QueuedInput>,
    composition_fence: Option<RequestFence>,
    suppressed_kind: Option<String>,
}

impl InputPipeline {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue one write.
    ///
    /// Fails closed when the caps are reached: the edit is dropped whole,
    /// never split, and nothing is sent.
    pub fn enqueue(&mut self, payload: String, fence: RequestFence) -> Result<(), InputError> {
        let bytes = payload.len();
        if bytes > MAX_INPUT_BYTES {
            return Err(InputError::TooLarge {
                bytes,
                limit: MAX_INPUT_BYTES,
            });
        }
        // The in-flight write is RETAINED input: it is still on its way to
        // the terminal, and it counts against both caps. Counting only the
        // waiting queue would make the byte cap mean "256 KiB queued plus
        // one more write in flight".
        let retained = self.queued_bytes + self.in_flight_len();
        let waiting = self.queue.len() + usize::from(self.in_flight.is_some());
        if waiting >= MAX_QUEUE_ITEMS || retained + bytes > MAX_QUEUE_BYTES {
            return Err(InputError::QueueFull);
        }
        self.queued_bytes += bytes;
        self.queue.push_back(QueuedInput { payload, fence });
        Ok(())
    }

    fn in_flight_len(&self) -> usize {
        self.in_flight
            .as_ref()
            .map(|item| item.payload.len())
            .unwrap_or(0)
    }

    /// The next write to dispatch, if the head still belongs to the current
    /// selection and connection.
    pub fn take_next(&mut self, current: RequestFence) -> Option<QueuedInput> {
        if self.in_flight.is_some() {
            return None;
        }
        let head = self.queue.front()?;
        if head.fence != current {
            // The selection or the connection moved on: everything waiting
            // belongs to a terminal that is no longer on screen.
            self.discard_waiting();
            return None;
        }
        let item = self.queue.pop_front()?;
        self.queued_bytes -= item.payload.len();
        self.in_flight = Some(item.clone());
        Some(item)
    }

    /// The dispatched write settled.
    ///
    /// A write that belonged to ANOTHER selection says nothing about this
    /// one: its outcome must not drop the bytes the current terminal is
    /// waiting for, and it must not publish an error about a terminal the
    /// user is no longer looking at. The old write is gone either way --
    /// an already-dispatched call is not cancelled by moving on.
    ///
    /// A write that belonged to THIS selection and failed is never replayed,
    /// and the suffix behind it is dropped rather than sent on its own — a
    /// command fragment is worse than no command.
    pub fn settled(&mut self, ok: bool, current: RequestFence) -> Option<InputError> {
        let item = self.in_flight.take()?;
        if item.fence != current {
            // Stale: the bytes waiting for the CURRENT terminal stay.
            return None;
        }
        if ok {
            None
        } else {
            self.discard_waiting();
            Some(InputError::SendFailed)
        }
    }

    /// Drop everything that has not been dispatched.
    pub fn discard_waiting(&mut self) {
        self.queued_bytes = 0;
        self.queue.clear();
    }

    /// The selection or the connection changed: the waiting writes and any
    /// transient composition belong to a terminal that is gone.
    pub fn on_selection_changed(&mut self) {
        self.discard_waiting();
        self.composition_fence = None;
        self.suppressed_kind = None;
    }

    /// The IME started composing. Nothing is sent yet.
    pub fn composition_start(&mut self, fence: RequestFence) {
        self.composition_fence = Some(fence);
        self.suppressed_kind = None;
    }

    /// A preedit update. A preedit is not a commit: nothing is sent.
    pub fn composition_update(&mut self, _text: &str) {}

    /// The IME committed. The committed text is returned exactly once.
    ///
    /// `None` when the composition was canceled, or when it outlived the
    /// selection it started in — a composition that belongs to a terminal
    /// that is no longer on screen is not sent at all.
    pub fn composition_end(&mut self, current: RequestFence, text: &str) -> Option<String> {
        let fence = self.composition_fence.take()?;
        if fence != current || text.is_empty() {
            return None;
        }
        Some(text.to_string())
    }

    /// The browser is about to repeat something it already delivered: a
    /// `paste` is followed by an `input` event with the same text, and a
    /// `compositionend` by an `input` event with the committed text.
    ///
    /// The repeat is dropped, so the bytes are sent once. The suppression
    /// holds the RAW text the browser repeats, not the payload that was
    /// written — a bracketed paste is framed on the wire and bare in the
    /// The browser is about to deliver an event this view already handled.
    ///
    /// The KIND is recorded, not the text: two different edits can carry the
    /// same text, and a suppression that matches on text swallows the
    /// second one -- the user's next keystroke disappears.
    ///
    /// A bracketed paste is framed on the wire and bare in the clipboard, so
    /// the event the browser repeats is the BARE one; the kind is what
    /// identifies it.
    pub fn suppress_input_kind(&mut self, kind: &str) {
        self.suppressed_kind = Some(kind.to_string());
    }

    /// The composition was canceled. Nothing is sent.
    pub fn composition_cancel(&mut self) {
        self.composition_fence = None;
        self.suppressed_kind = None;
    }

    /// An ordinary text insertion, as the browser reports it.
    ///
    /// `kind` is the event's `inputType`, and the KIND decides:
    ///
    /// * `insertCompositionText` is the IME's own delivery -- the preedit
    ///   while it is typing, and the commit in the engines that report it
    ///   that way. It is not an insertion: the IME owns the text until
    ///   `compositionend`, and the commit is sent from there.
    /// * The kind armed by [`InputPipeline::suppress_input_kind`] is the
    ///   event this view already handled, and it is dropped.
    ///
    /// `None` also when the text belongs to a composition that outlived the
    /// selection it started in: a composition opened in one terminal does
    /// not get to type into the next one.
    pub fn ordinary_input(
        &mut self,
        text: &str,
        current: RequestFence,
        kind: &str,
    ) -> Option<String> {
        if kind == "insertCompositionText" {
            return None;
        }
        if let Some(suppressed) = self.suppressed_kind.take() {
            if suppressed == kind {
                // The event this view already handled.
                return None;
            }
            // The armed kind was something else: the suppression is stale,
            // and THIS event is a real edit.
        }
        if text.is_empty() {
            return None;
        }
        if let Some(fence) = self.composition_fence {
            if fence != current {
                self.composition_fence = None;
                self.suppressed_kind = None;
                return None;
            }
        }
        Some(text.to_string())
    }

    /// How many writes are waiting behind the one in flight.
    pub fn waiting(&self) -> usize {
        self.queue.len()
    }

    /// Whether a write is in flight.
    pub fn in_flight(&self) -> bool {
        self.in_flight.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::RequestFence;
    use aleph_protocol::capability::PtyOperation;
    use aleph_protocol::pty::{
        PtyAttrs, PtyColor, PtyRowPatch, PtyScreenFrame, PtyScreenPatch, PtyStyleRun,
    };

    fn fence(epoch: u64, revision: u64) -> RequestFence {
        RequestFence {
            epoch,
            selection_revision: revision,
        }
    }

    fn screen_with_run(text: &str) -> ClientScreen {
        let mut screen = ClientScreen::new(2, 8, 1, "s1");
        let frame = PtyScreenFrame {
            session_id: "s1".to_string(),
            seq: 2,
            rows: 2,
            cols: 8,
            patch: PtyScreenPatch {
                rows: vec![PtyRowPatch {
                    row: 0,
                    runs: vec![PtyStyleRun {
                        text: text.to_string(),
                        fg: PtyColor::default(),
                        bg: PtyColor::default(),
                        attrs: PtyAttrs::default(),
                    }],
                }],
                ..Default::default()
            },
        };
        let _ = screen.apply(frame);
        screen
    }

    /// The wire limit counts BYTES, framing included, and the refusal has to
    /// happen before anything is queued: a bracketed paste that is split is
    /// a paste the program never finishes reading.
    #[test]
    fn paste_limit_accounts_for_utf8_and_framing() {
        let at_limit = "a".repeat(MAX_INPUT_BYTES - PASTE_FRAMING_BYTES);
        let payload = encode_paste(&at_limit, Some(true)).expect("a paste that fits");
        assert_eq!(payload.len(), MAX_INPUT_BYTES);
        assert!(payload.starts_with(PASTE_START));
        assert!(payload.ends_with(PASTE_END));

        // One ASCII byte over the limit is refused whole.
        let over = "a".repeat(MAX_INPUT_BYTES - PASTE_FRAMING_BYTES + 1);
        assert!(matches!(
            encode_paste(&over, Some(true)),
            Err(InputError::TooLarge { .. })
        ));

        // A three-byte character over the limit is refused the same way: the
        // limit counts bytes, not characters.
        let cjk = "中".repeat((MAX_INPUT_BYTES - PASTE_FRAMING_BYTES) / 3 + 1);
        assert!(matches!(
            encode_paste(&cjk, Some(true)),
            Err(InputError::TooLarge { .. })
        ));

        // A refused paste is one discarded item and no write at all.
        let mut pipeline = InputPipeline::new();
        let current = fence(1, 1);
        assert!(pipeline
            .enqueue(String::new(), current)
            .is_ok());
        assert_eq!(pipeline.waiting(), 1);

        // ASCII C0 and CJK bytes survive the Gateway contract: the payload is
        // plain UTF-8, and the wire type accepts it as it stands.
        let raw = "\x03中文";
        let payload = encode_paste(raw, None).expect("plain utf-8");
        assert_eq!(payload.as_bytes(), raw.as_bytes());
        assert!(PtyOperation::Input {
            data: payload,
            base64: false
        }
        .validate()
        .is_ok());

        // A payload carrying the end marker would close the bracket early and
        // hand the rest to the shell as ordinary typed input -- which, with
        // bracketed paste on, is exactly the input it may run. The marker is
        // removed so "a paste is inert until the user presses Enter" holds for
        // arbitrary clipboard content and not just for content that happens
        // not to contain it.
        let hostile = "safe\x1b[201~rm -rf /";
        let wrapped = encode_paste(hostile, Some(true)).expect("a bracketed paste");
        assert_eq!(wrapped, "\x1b[200~saferm -rf /\x1b[201~");
        assert_eq!(wrapped.matches(PASTE_END).count(), 1);

        // Nothing is stripped when there is no bracket to break out of:
        // silently editing bytes the user asked to send would be its own lie.
        assert_eq!(
            encode_paste(hostile, None).expect("an unwrapped paste"),
            hostile,
            "an unwrapped paste is passed through byte for byte"
        );
    }

    /// The IME commits once. The preedit is not text yet, the commit is
    /// sent once, and the browser's trailing `input` event repeats that same
    /// commit — sending it twice would type the word twice.
    #[test]
    fn terminal_composition_commits_once_in_current_selection() {
        let current = fence(7, 3);
        let mut pipeline = InputPipeline::new();

        pipeline.composition_start(current);
        pipeline.composition_update("zhong");
        assert_eq!(pipeline.waiting(), 0, "a preedit is not a commit");

        let committed = pipeline
            .composition_end(current, "中文")
            .expect("the committed text");
        assert_eq!(committed, "中文");
        // The commit is sent ONCE: the composition is over, and a second
        // `compositionend` has nothing left to commit.
        assert!(pipeline.composition_end(current, "中文").is_none());
        // The IME's own delivery of the same text is not an insertion: it
        // belongs to the composition, which already committed.
        assert!(
            pipeline
                .ordinary_input("中文", current, "insertCompositionText")
                .is_none()
        );
        pipeline
            .enqueue(committed, current)
            .expect("the commit is queued");
        assert_eq!(pipeline.waiting(), 1);

        // A canceled composition sends nothing.
        let mut canceled = InputPipeline::new();
        canceled.composition_start(current);
        canceled.composition_cancel();
        assert!(canceled.composition_end(current, "中").is_none());
        assert_eq!(canceled.waiting(), 0);

        // A stale session sends nothing: the selection moved on while the
        // composition was open.
        let mut stale = InputPipeline::new();
        stale.composition_start(current);
        assert!(stale.composition_end(fence(7, 4), "中文").is_none());
        assert_eq!(stale.waiting(), 0);
    }

    /// The edit AFTER a composition is the user's own.
    ///
    /// The browser's own composition delivery is `insertCompositionText`;
    /// the next ordinary edit is `insertText`, and it is a real edit even
    /// when it carries the same text as the commit. A suppression that
    /// matched on TEXT would swallow it, and the user would lose a
    /// keystroke.
    #[test]
    fn terminal_input_keeps_the_next_real_edit_after_a_composition() {
        let current = fence(7, 3);
        let mut pipeline = InputPipeline::new();
        pipeline.composition_start(current);
        // The preedit, as the browser reports it.
        assert!(pipeline.ordinary_input("zhong", current, "insertCompositionText").is_none());
        let committed = pipeline
            .composition_end(current, "中文")
            .expect("the committed text");
        assert_eq!(committed, "中文");
        // The NEXT intentional edit carries the same text and must survive.
        assert_eq!(
            pipeline.ordinary_input("中文", current, "insertText"),
            Some("中文".to_string())
        );
    }

    /// A paste this view already sent is not sent again, and the edit after
    /// it is not collateral damage.
    #[test]
    fn terminal_input_drops_the_paste_repeat_but_not_the_next_edit() {
        let current = fence(7, 3);
        let mut pipeline = InputPipeline::new();
        pipeline.suppress_input_kind("insertFromPaste");
        assert!(
            pipeline
                .ordinary_input("pasted", current, "insertFromPaste")
                .is_none(),
            "the paste this view already sent is not sent twice"
        );
        assert_eq!(
            pipeline.ordinary_input("x", current, "insertText"),
            Some("x".to_string()),
            "the next real edit survives"
        );
    }

    /// One write is in flight; the rest wait behind it. When the selection
    /// changes, the waiting suffix is discarded and the late completion of
    /// the dispatched write changes nothing.
    #[test]
    fn terminal_input_fifo_fences_pending_writes() {
        let current = fence(1, 1);
        let mut pipeline = InputPipeline::new();
        pipeline.enqueue("first".to_string(), current).unwrap();
        pipeline.enqueue("second".to_string(), current).unwrap();

        let first = pipeline.take_next(current).expect("the first write");
        assert_eq!(first.payload, "first");
        assert!(pipeline.take_next(current).is_none(), "one write in flight");

        // The selection moves on before the first write settles.
        let next = fence(2, 2);
        assert!(
            pipeline.settled(true, next).is_none(),
            "a stale completion is not an error"
        );
        assert!(
            pipeline.take_next(next).is_none(),
            "the waiting suffix was discarded"
        );
        assert_eq!(pipeline.waiting(), 0);
    }

    /// A failed write is not replayed, and the suffix behind it is dropped
    /// rather than sent as a command fragment.
    #[test]
    fn terminal_input_fifo_drops_the_suffix_after_a_failure() {
        let current = fence(1, 1);
        let mut pipeline = InputPipeline::new();
        pipeline.enqueue("first".to_string(), current).unwrap();
        pipeline.enqueue("second".to_string(), current).unwrap();
        pipeline.take_next(current).expect("the first write");
        assert_eq!(
            pipeline.settled(false, current),
            Some(InputError::SendFailed),
            "the failure is surfaced"
        );
        assert_eq!(pipeline.waiting(), 0, "no command suffix is sent");
        assert!(pipeline.take_next(current).is_none());
    }

    /// An old write that settles late -- after the selection moved on --
    /// says nothing about the terminal that is on screen NOW.
    ///
    /// It must not drop the current terminal's bytes, and it must not
    /// publish an error about a terminal the user is not looking at. The
    /// bytes stay queued behind the old write and go out once it is done.
    #[test]
    fn terminal_input_fifo_keeps_the_current_bytes_when_an_old_write_settles() {
        let old = fence(1, 1);
        let current = fence(2, 2);
        let mut pipeline = InputPipeline::new();
        pipeline.enqueue("old".to_string(), old).unwrap();
        pipeline.take_next(old).expect("the old write dispatches");

        // The selection moved: the waiting suffix of the old terminal is
        // dropped, and the new terminal's bytes queue behind the write that
        // is still in flight.
        pipeline.on_selection_changed();
        pipeline.enqueue("new".to_string(), current).unwrap();
        assert_eq!(pipeline.waiting(), 1);

        // The old write settles late, and it FAILED.
        assert!(
            pipeline.settled(false, current).is_none(),
            "a stale failure is not an error about the current terminal"
        );
        assert_eq!(pipeline.waiting(), 1, "the current bytes are kept");
        let next = pipeline.take_next(current).expect("the current bytes dispatch");
        assert_eq!(next.payload, "new");
    }

    /// Copying the painted text must not duplicate a wide glyph's
    /// continuation cell, and `Ctrl-C` without a selection stays the
    /// terminal's interrupt.
    #[test]
    fn terminal_copy_preserves_wide_cells_and_interrupt() {
        let screen = screen_with_run("甲B");
        let selection = Selection::new(0, 0, 0, 3);
        assert_eq!(selection_text(&screen, &selection), "甲B");

        // `Ctrl-C` is the interrupt, not a copy.
        assert_eq!(
            crate::keymap::encode_key("c", true, false, false, false),
            crate::keymap::KeyAction::Bytes(vec![0x03])
        );
        assert!(!is_copy_chord("c", true, false, false));
        // The platform's copy chords are.
        assert!(is_copy_chord("c", false, false, true));
        assert!(is_copy_chord("c", true, true, false));
    }

    /// Rapid geometry changes collapse into the latest measured size, an
    /// attach resubmits the size even when it did not change, and a view
    /// that is gone schedules nothing more.
    #[test]
    fn terminal_resize_coalesces_and_releases_resources() {
        let mut coalescer = crate::render::ResizeCoalescer::new();
        coalescer.observe(24, 80);
        coalescer.observe(30, 100);
        assert_eq!(
            coalescer.take(),
            Some((30, 100)),
            "only the latest measured size is reported"
        );
        assert_eq!(coalescer.take(), None);

        // A new connection needs the size again, even an unchanged one.
        coalescer.force();
        assert_eq!(coalescer.take(), Some((30, 100)));

        // A hidden or unmounted view schedules nothing.
        coalescer.observe(40, 120);
        coalescer.release();
        assert!(!coalescer.is_live());
        assert_eq!(coalescer.take(), None);
    }
}
