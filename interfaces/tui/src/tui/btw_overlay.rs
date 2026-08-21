// Side-question overlay controller (`/btw`).
//
// A `/btw` question runs as its own turn on a *derived* session the gateway
// keeps out of the main conversation entirely. That is the whole point of the
// feature, and it is also why this overlay exists: the frames that answer a
// side question are, by construction, frames for a session that is not the one
// on screen — so `AppState::frame_belongs_here` correctly drops every one of
// them. Dropped is right for the transcript and useless for the user, who
// asked a question and would otherwise watch nothing happen.
//
// This controller is what those frames are routed to instead. It follows the
// approval overlay's shape (one struct on `AppState`, a `Focus` variant, keys
// in `keys.rs`, a widget in `widgets/`) rather than inventing a second overlay
// mechanism.
//
// # Identity is the run id, not the session key
//
// The obvious design — remember the side session's key and match frames
// against it — cannot work from a thin client. `side_key_for` hashes the main
// key *including its epoch*, the epoch lives server-side, and this crate may
// not depend on `alephcore` at all (`interfaces/tui/Cargo.toml` says so). A
// client-side re-derivation would be byte-identical at epoch 0 and wrong from
// the first `/new` onward — the failure shape that never reproduces locally.
//
// The run id has none of those problems and the TUI already holds it: the
// `agent.run` reply carries `{run_id, session_key}` for the very call that
// asked the question. `claim_run` records it; `accepts_frame` keys on it.
//
// # Why claims outlive the exchange
//
// `claimed` is a bounded FIFO of run ids, not just the in-flight one. A run's
// last frames can arrive after `RunComplete` has already settled the exchange
// (`agent_trace` is a deliberately lossy mirror and can reorder against the
// authoritative stream), and a frame from a run this overlay asked for must
// never reach the transcript. Eviction is safe in the same direction as
// `AppState::run_sessions`: an evicted id stops being intercepted here and
// falls back to `frame_belongs_here`, which drops it because the side session
// is not the screen's session. Losing a claim can only cost a late frame that
// nobody was going to render; it can never leak one into the transcript.

use std::collections::VecDeque;

/// How many side-question run ids the overlay remembers. Side questions are
/// asked one at a time by a human, so this is generous; see the module doc for
/// why eviction is safe.
const CLAIM_CAP: usize = 32;

/// One finished side question.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BtwExchange {
    /// What was asked, verbatim.
    pub question: String,
    /// The answer as raw markdown — what `c` copies.
    pub answer: String,
    /// The user pressed Esc while it was still answering.
    pub aborted: bool,
    /// The run failed. Distinct from an empty answer: "it broke" and "it said
    /// nothing" are different things to be told.
    pub error: Option<String>,
}

impl BtwExchange {
    /// A question that got an answer.
    ///
    /// Test-only: the three production paths (`finish_active` / `fail_active`
    /// / `abort_active`) each build the whole struct, because each has to say
    /// something different about `aborted` and `error`. A production shorthand
    /// that fixed both to their happy values would be an abstraction with no
    /// consumer, and the first caller to reach for it would be the one who
    /// wanted one of the other two endings.
    #[cfg(test)]
    #[must_use]
    pub fn answered(question: &str, answer: &str) -> Self {
        Self {
            question: question.to_string(),
            answer: answer.to_string(),
            aborted: false,
            error: None,
        }
    }

    /// The one-word status shown next to the question.
    #[must_use]
    pub fn status(&self) -> &'static str {
        if self.error.is_some() {
            "failed"
        } else if self.aborted {
            "aborted"
        } else {
            "answered"
        }
    }
}

/// The side question currently being answered.
#[derive(Debug, Clone, Default)]
pub struct BtwActive {
    /// The run answering it, once `agent.run` has replied. `None` for the
    /// window between opening the overlay and that reply — during which
    /// nothing can be intercepted, because nothing has been claimed.
    pub run_id: Option<String>,
    pub question: String,
    /// Accumulated answer text.
    pub answer: String,
    /// Bytes of `answer` already delivered as `ResponseChunk` deltas.
    ///
    /// The same turn's text arrives twice on the wire — as deltas and again in
    /// full as `AgentTrace{TextEmitted{Final}}` — so the final frame appends
    /// only its un-streamed suffix. Same counter, same reason, as
    /// `AppState::turn_streamed_len`; the overlay needs its own because the
    /// side run and the main run stream concurrently.
    streamed_len: usize,
    /// The tool the side question is running right now, for a status line.
    pub tool_name: Option<String>,
}

/// The `/btw` overlay: a history of side questions plus the one in flight.
#[derive(Debug, Default)]
pub struct BtwOverlay {
    /// Finished exchanges, oldest first.
    pub exchanges: Vec<BtwExchange>,
    /// Which finished exchange is on screen. Always a valid index into
    /// `exchanges` when that is non-empty.
    pub view_index: usize,
    /// The question being answered, if any.
    pub active: Option<BtwActive>,
    /// Run ids whose frames belong to this overlay (see module doc).
    claimed: VecDeque<String>,
    /// Is the overlay on screen and holding focus?
    pub open: bool,
    /// Vertical scroll into the displayed answer, in wrapped lines.
    pub scroll: u16,
    /// The follow-up being typed.
    pub composer: String,
    /// Keys go to [`Self::composer`] rather than to the browse shortcuts.
    ///
    /// Structural, and it only ever flips *towards* composing on its own (any
    /// printable key that is not a shortcut). It is never derived from
    /// `composer.is_empty()`: clearing the buffer would silently drop the user
    /// back into a mode where the next letter they type means something else.
    /// Tab is the explicit toggle in both directions.
    pub composing: bool,
}

impl BtwOverlay {
    /// Start a new side question and show the overlay.
    ///
    /// The run id is not known yet — `agent.run` has not been called. Claiming
    /// it is [`Self::claim_run`]'s job, and there is no window in between: the
    /// TUI processes gateway frames only at the top of the main loop, never
    /// while an action's RPC is being awaited, so no frame can arrive between
    /// the reply and the claim.
    pub fn begin(&mut self, question: String) {
        self.active = Some(BtwActive {
            question,
            ..BtwActive::default()
        });
        self.open = true;
        self.scroll = 0;
        self.composer.clear();
        self.composing = false;
    }

    /// Record that `run_id` is answering the active side question.
    pub fn claim_run(&mut self, run_id: String) {
        if let Some(active) = &mut self.active {
            active.run_id = Some(run_id.clone());
        }
        if self.claimed.iter().any(|id| *id == run_id) {
            return;
        }
        if self.claimed.len() >= CLAIM_CAP {
            self.claimed.pop_front();
        }
        self.claimed.push_back(run_id);
    }

    /// Claim `run_id` **only** for a side question that is still waiting for
    /// one.
    ///
    /// Called from the shared `agent.run` reply path, which every send goes
    /// through — including sends that have nothing to do with `/btw`. The
    /// precondition is what keeps those out: an active question with no run id
    /// exists only between [`Self::begin`] and this call, and nothing else
    /// runs in between. An ordinary message therefore finds no pending
    /// question and claims nothing.
    pub fn claim_pending_run(&mut self, run_id: String) {
        if self
            .active
            .as_ref()
            .is_none_or(|active| active.run_id.is_some())
        {
            return;
        }
        self.claim_run(run_id);
    }

    /// Does a frame naming `run_id` belong to this overlay?
    ///
    /// `None` — a frame with no run id at all — is **not** ours. Those frames
    /// (`AskUser`, and anything the protocol keys by session) are exempted
    /// from the screen's cross-session guard on purpose, and swallowing them
    /// here would hide a parked question behind an overlay that cannot answer
    /// it.
    #[must_use]
    pub fn accepts_frame(&self, run_id: Option<&str>) -> bool {
        run_id.is_some_and(|id| self.claimed.iter().any(|claimed| claimed == id))
    }

    /// The run id of the side question being answered right now.
    #[must_use]
    pub fn active_run_id(&self) -> Option<&str> {
        self.active.as_ref()?.run_id.as_deref()
    }

    /// Append a streamed delta to the active answer.
    pub fn push_delta(&mut self, delta: &str) {
        if let Some(active) = &mut self.active {
            active.streamed_len += delta.len();
            active.answer.push_str(delta);
        }
    }

    /// Apply the turn's full final text, appending only what streaming has not
    /// already delivered.
    ///
    /// `.get()` rather than a slice: an out-of-range or non-boundary index
    /// yields `None` and appends nothing, instead of panicking mid-render if
    /// the counter is ever out of step with the bytes.
    pub fn push_final(&mut self, text: &str) {
        if let Some(active) = &mut self.active {
            let fresh = text.get(active.streamed_len..).unwrap_or("").to_string();
            active.streamed_len = text.len();
            active.answer.push_str(&fresh);
        }
    }

    /// Note which tool the side question is running, for the status line.
    pub fn note_tool(&mut self, name: Option<String>) {
        if let Some(active) = &mut self.active {
            active.tool_name = name;
        }
    }

    /// Settle the active question as answered.
    ///
    /// `fallback` is the authoritative final text from the run summary, used
    /// only when nothing streamed — a turn that produced no deltas and no
    /// trace mirror still has an answer, and an empty bubble would report the
    /// opposite.
    pub fn finish_active(&mut self, fallback: Option<&str>) {
        let Some(active) = self.active.take() else {
            return;
        };
        let answer = if active.answer.trim().is_empty() {
            fallback.unwrap_or_default().to_string()
        } else {
            active.answer
        };
        self.finish_exchange(BtwExchange {
            question: active.question,
            answer,
            aborted: false,
            error: None,
        });
    }

    /// Settle the active question as failed, keeping whatever text arrived
    /// before the failure — a partial answer is still worth reading.
    pub fn fail_active(&mut self, error: String) {
        let Some(active) = self.active.take() else {
            return;
        };
        self.finish_exchange(BtwExchange {
            question: active.question,
            answer: active.answer,
            aborted: false,
            error: Some(error),
        });
    }

    /// Settle the active question as aborted by the user.
    pub fn abort_active(&mut self) {
        let Some(active) = self.active.take() else {
            return;
        };
        self.finish_exchange(BtwExchange {
            question: active.question,
            answer: active.answer,
            aborted: true,
            error: None,
        });
    }

    /// File a finished exchange and put it on screen.
    ///
    /// A new answer is what the user is waiting for, so it becomes the shown
    /// page even if they had paged back through history.
    pub fn finish_exchange(&mut self, exchange: BtwExchange) {
        self.exchanges.push(exchange);
        self.view_index = self.exchanges.len().saturating_sub(1);
        self.scroll = 0;
    }

    /// Page one exchange towards the start. Clamps; never wraps — wrapping
    /// would make "I am at the oldest one" indistinguishable from "I have
    /// looped round to the newest".
    pub const fn page_left(&mut self) {
        self.view_index = self.view_index.saturating_sub(1);
        self.scroll = 0;
    }

    /// Page one exchange towards the end. Clamps; never wraps.
    pub const fn page_right(&mut self) {
        let last = self.exchanges.len().saturating_sub(1);
        if self.view_index < last {
            self.view_index += 1;
        }
        self.scroll = 0;
    }

    /// The finished exchange currently on screen, if any.
    #[must_use]
    pub fn current(&self) -> Option<&BtwExchange> {
        self.exchanges.get(self.view_index)
    }

    /// The raw markdown `c` copies: the live answer while one is streaming,
    /// otherwise the exchange on screen.
    #[must_use]
    pub fn copyable(&self) -> Option<&str> {
        match &self.active {
            Some(active) if !active.answer.is_empty() => Some(&active.answer),
            _ => self
                .current()
                .map(|e| e.answer.as_str())
                .filter(|a| !a.is_empty()),
        }
    }

    /// Hide the overlay. History is kept: the next `/btw` reopens onto it.
    pub fn close(&mut self) {
        self.open = false;
        self.composing = false;
    }

    pub const fn scroll_down(&mut self, n: u16) {
        self.scroll = self.scroll.saturating_add(n);
    }

    pub const fn scroll_up(&mut self, n: u16) {
        self.scroll = self.scroll.saturating_sub(n);
    }
}

/// The OSC 52 sequence that asks the terminal to put `text` on the system
/// clipboard.
///
/// # Why an escape sequence and not a clipboard library
///
/// A clipboard crate would be a new third-party dependency for one keystroke
/// (R3), and it would only ever reach the clipboard of the machine the process
/// runs on — which for a TUI over SSH is the wrong machine. OSC 52 is answered
/// by the terminal emulator the human is actually looking at, so it works
/// locally and remotely by the same mechanism.
///
/// The cost is honest and worth stating where the caller can see it: not every
/// terminal implements it (Apple's Terminal.app does not; iTerm2, kitty,
/// WezTerm, Alacritty, and tmux with `set-clipboard on` do), and there is no
/// reply to read — the sequence is fire-and-forget, so this can never report
/// whether it worked. The caller therefore says "sent to the terminal", not
/// "copied".
///
/// Base64 is inlined below rather than pulled in as a crate for the same
/// reason: a dozen lines against a dependency.
#[must_use]
pub fn osc52_clipboard_sequence(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64_encode(text.as_bytes()))
}

/// Standard base64 (RFC 4648, with padding). The only encoder this crate
/// needs, and it exists solely for [`osc52_clipboard_sequence`].
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).copied().map_or(0, u32::from);
        let b2 = chunk.get(2).copied().map_or(0, u32::from);
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(triple >> 18) as usize & 0x3f] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(triple >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[triple as usize & 0x3f] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_overlay_pages_through_history_without_running_off_either_end() {
        let mut o = BtwOverlay::default();
        o.finish_exchange(BtwExchange::answered("q1", "a1"));
        o.finish_exchange(BtwExchange::answered("q2", "a2"));
        assert_eq!(o.view_index, 1, "a new answer is the one on screen");

        o.page_left();
        assert_eq!(o.view_index, 0);
        o.page_left();
        assert_eq!(
            o.view_index, 0,
            "paging past the start must clamp, not wrap"
        );

        o.page_right();
        o.page_right();
        assert_eq!(o.view_index, 1, "paging past the end must clamp, not wrap");
    }

    /// Paging an empty history must not index out of bounds, and must not
    /// leave `view_index` pointing past the end once one arrives.
    #[test]
    fn paging_an_empty_history_stays_at_zero() {
        let mut o = BtwOverlay::default();
        o.page_right();
        o.page_right();
        assert_eq!(o.view_index, 0);
        assert!(o.current().is_none());
        o.page_left();
        assert_eq!(o.view_index, 0);
    }

    #[test]
    fn the_overlay_only_shows_frames_from_the_side_run() {
        let mut o = BtwOverlay::default();
        o.claim_run("run-side".to_string());
        assert!(o.accepts_frame(Some("run-side")));
        assert!(!o.accepts_frame(Some("run-main")));
        assert!(
            !o.accepts_frame(None),
            "a frame with no run id is not addressed to this overlay"
        );
    }

    /// An overlay that has claimed nothing must claim nothing — otherwise the
    /// first `/btw` of a session would swallow the main run's frames.
    #[test]
    fn a_fresh_overlay_claims_no_frames() {
        let o = BtwOverlay::default();
        assert!(!o.accepts_frame(Some("run-main")));
        assert!(!o.accepts_frame(None));
    }

    /// A claim outlives its exchange: `RunComplete` settles the answer, and a
    /// late frame from the same run must still be swallowed rather than fall
    /// through to the transcript.
    #[test]
    fn a_settled_runs_late_frames_are_still_claimed() {
        let mut o = BtwOverlay::default();
        o.begin("why?".into());
        o.claim_run("r1".into());
        o.push_delta("because");
        o.finish_active(None);
        assert!(o.active.is_none());
        assert!(o.accepts_frame(Some("r1")));
    }

    /// Claims are bounded, and eviction is FIFO — the oldest claim is the one
    /// least likely to still be producing frames.
    #[test]
    fn claims_are_bounded_and_evict_oldest_first() {
        let mut o = BtwOverlay::default();
        for i in 0..(CLAIM_CAP + 2) {
            o.claim_run(format!("r{i}"));
        }
        assert_eq!(o.claimed.len(), CLAIM_CAP);
        assert!(!o.accepts_frame(Some("r0")));
        assert!(o.accepts_frame(Some(&format!("r{}", CLAIM_CAP + 1))));

        // Re-claiming an id already held must not burn a slot.
        let before = o.claimed.len();
        o.claim_run(format!("r{}", CLAIM_CAP + 1));
        assert_eq!(o.claimed.len(), before);
    }

    /// The turn's text arrives twice — as deltas and again in full — so the
    /// final frame must append only its un-streamed suffix.
    #[test]
    fn the_final_text_appends_only_what_streaming_missed() {
        let mut o = BtwOverlay::default();
        o.begin("q".into());
        o.push_delta("hello ");
        o.push_final("hello world");
        o.finish_active(None);
        assert_eq!(o.current().expect("exchange").answer, "hello world");

        // A turn that never streamed lands in full.
        let mut o = BtwOverlay::default();
        o.begin("q".into());
        o.push_final("whole thing");
        o.finish_active(None);
        assert_eq!(o.current().expect("exchange").answer, "whole thing");
    }

    /// A turn with no deltas and no trace mirror still has an answer — the run
    /// summary's. An empty bubble would report the opposite of what happened.
    #[test]
    fn an_unstreamed_answer_falls_back_to_the_run_summary() {
        let mut o = BtwOverlay::default();
        o.begin("q".into());
        o.finish_active(Some("from the summary"));
        assert_eq!(o.current().expect("exchange").answer, "from the summary");
    }

    /// "It broke", "you stopped it" and "it said nothing" are three different
    /// things to be told, so they are three different states — and a failure
    /// keeps whatever text had arrived, because a partial answer still reads.
    #[test]
    fn the_three_endings_stay_distinguishable() {
        let mut o = BtwOverlay::default();
        o.begin("q1".into());
        o.push_delta("partial");
        o.fail_active("provider unreachable".into());
        let failed = o.current().expect("exchange");
        assert_eq!(failed.status(), "failed");
        assert_eq!(failed.answer, "partial");

        o.begin("q2".into());
        o.abort_active();
        assert_eq!(o.current().expect("exchange").status(), "aborted");

        o.begin("q3".into());
        o.finish_active(None);
        let silent = o.current().expect("exchange");
        assert_eq!(silent.status(), "answered");
        assert!(silent.answer.is_empty());
    }

    /// `c` copies the live answer while one is streaming and the page on
    /// screen otherwise — never an empty string, which would silently replace
    /// whatever the user already had on their clipboard.
    #[test]
    fn copy_prefers_the_live_answer_then_the_page_on_screen() {
        let mut o = BtwOverlay::default();
        assert_eq!(o.copyable(), None);

        o.finish_exchange(BtwExchange::answered("q1", "old answer"));
        assert_eq!(o.copyable(), Some("old answer"));

        o.begin("q2".into());
        assert_eq!(
            o.copyable(),
            Some("old answer"),
            "a question that has not answered yet must not shadow the page on screen"
        );
        o.push_delta("new answer");
        assert_eq!(o.copyable(), Some("new answer"));
    }

    /// Base64 against the RFC 4648 test vectors, both padding cases included.
    /// A hand-rolled encoder tested only against its own output would prove
    /// nothing.
    #[test]
    fn base64_matches_the_rfc_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        // A side answer is markdown and can hold anything; non-ASCII must go
        // through as its UTF-8 bytes, not be mangled per-char.
        assert_eq!(base64_encode("héllo".as_bytes()), "aMOpbGxv");
    }

    #[test]
    fn the_clipboard_sequence_is_a_well_formed_osc_52() {
        assert_eq!(osc52_clipboard_sequence("foobar"), "\x1b]52;c;Zm9vYmFy\x07");
    }

    /// Closing hides the overlay; it does not discard the history, so the next
    /// `/btw` reopens onto what was already asked.
    #[test]
    fn closing_keeps_the_history() {
        let mut o = BtwOverlay::default();
        o.begin("q1".into());
        o.push_delta("a1");
        o.finish_active(None);
        o.close();
        assert!(!o.open);
        assert_eq!(o.exchanges.len(), 1);

        o.begin("q2".into());
        assert!(o.open);
        assert_eq!(o.exchanges.len(), 1);
    }
}
