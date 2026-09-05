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

/// How a side question ended.
///
/// One enum rather than an `aborted` flag beside an `Option<String>` error.
/// The endings are mutually exclusive and each owes the user a different word,
/// and the flags-plus-option shape had already forced two of them to share one:
/// `settle_superseded` had nowhere to put "the user moved on" except the error
/// field, so a question nobody failed rendered as `failed` — the same word a
/// provider outage gets, over a run that may still be running and may still
/// succeed. A third boolean would have made eight states of which five are
/// unrepresentable nonsense; this makes each ending sayable and the rest
/// impossible.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BtwOutcome {
    /// It finished. The answer may still be empty — "it said nothing" is a
    /// real outcome and not the same as any of the others.
    #[default]
    Answered,
    /// The user pressed Esc while it was still answering.
    Aborted,
    /// A newer side question replaced it before it finished.
    ///
    /// Deliberately neither `Answered` (nothing said it finished) nor
    /// `Failed` (nothing broke). The run is very likely still going
    /// server-side — see `commands::btw_abort_or_close` for what that costs.
    Superseded,
    /// The run failed, carrying the reason. Distinct from an empty answer:
    /// "it broke" and "it said nothing" are different things to be told.
    Failed(String),
    /// The socket died while it was being answered, and on reconnect the
    /// gateway no longer reported the run in flight. Carries what it did say.
    ///
    /// The fifth ending exists because the other four are each a lie about
    /// this one. The frames emitted while this client was away are gone — the
    /// text on file may be a **prefix** of the real answer with no way to tell,
    /// so `Answered` would present a truncation as the whole thing. And on the
    /// commonest of the states the server reports here the run did not break,
    /// it finished with nobody listening, so `Failed` names the wrong event.
    ///
    /// This is `Superseded`'s sibling: same "the text is whatever arrived",
    /// different cause, and the cause is the part the user needs to read.
    Disconnected(String),
}

/// One finished side question.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BtwExchange {
    /// What was asked, verbatim.
    pub question: String,
    /// The answer as raw markdown — what `c` copies.
    pub answer: String,
    /// How it ended.
    pub outcome: BtwOutcome,
}

impl BtwExchange {
    /// A question that got an answer.
    ///
    /// Test-only: the production paths each build the whole struct, because
    /// each has a different outcome to name. A production shorthand fixing the
    /// outcome to its happy value would be an abstraction with no consumer,
    /// and the first caller to reach for it would be one wanting a different
    /// ending.
    #[cfg(test)]
    #[must_use]
    pub fn answered(question: &str, answer: &str) -> Self {
        Self {
            question: question.to_string(),
            answer: answer.to_string(),
            outcome: BtwOutcome::Answered,
        }
    }

    /// The one-word status shown next to the question.
    ///
    /// Every word here has to be true of the thing it names — this line is
    /// the part that survives a glance, and the body underneath it is the
    /// part that does not.
    #[must_use]
    pub fn status(&self) -> &'static str {
        match self.outcome {
            BtwOutcome::Answered => "answered",
            BtwOutcome::Aborted => "aborted",
            BtwOutcome::Superseded => "superseded",
            BtwOutcome::Failed(_) => "failed",
            BtwOutcome::Disconnected(_) => "disconnected",
        }
    }

    /// The line that goes above the answer, when the ending owes one.
    ///
    /// **The label belongs to the outcome, not to the widget.** This used to be
    /// an `error()` accessor with the widget writing `"Error: "` in front of
    /// whatever came back — which was right while `Failed` was the only ending
    /// with anything to say, and became wrong the moment
    /// [`BtwOutcome::Disconnected`] arrived: a question that finished on the
    /// server while this client was away is not an error, and captioning it as
    /// one is the same overload the enum's own doc exists to prevent.
    ///
    /// It goes in the BODY, not in the status line. That line is one row tall,
    /// so a multi-line provider error rendered there showed its first line and
    /// dropped the rest with nowhere else to read it — and the interesting part
    /// of an error is rarely its first line. The body region wraps and scrolls.
    #[must_use]
    pub fn note(&self) -> Option<String> {
        match &self.outcome {
            BtwOutcome::Failed(reason) => Some(format!("Error: {reason}")),
            BtwOutcome::Disconnected(note) => Some(note.clone()),
            _ => None,
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
        // A side question can be asked while the previous one is still
        // answering — `Enter` in the overlay does not wait, and neither does a
        // second `/btw` typed in the main composer. Overwriting `active` used
        // to discard that question's run id AND its partial answer, after
        // which the old run's remaining frames (still claimed) appended
        // themselves to the NEW question's answer. File it instead: whatever
        // text arrived is worth reading, and once its run id is no longer the
        // active one, `for_active_run` drops its tail rather than
        // misattributing it.
        self.settle_superseded();
        self.active = Some(BtwActive {
            question,
            ..BtwActive::default()
        });
        self.open = true;
        self.scroll = 0;
        // The composer is deliberately NOT cleared here. The send path already
        // clears it at the moment it takes the text (`handle_btw_key`), so the
        // only drafts this would reach are ones nobody sent — a `/btw` typed
        // in the main composer while a half-written follow-up sits in the
        // overlay. Clearing that is silent data loss with no undo.
        self.composing = false;
    }

    /// File a still-answering question because a newer one has replaced it.
    ///
    /// Recorded as an error rather than as an answer, because it is neither:
    /// nothing said the run finished, and the text on file is whatever had
    /// arrived by the time the user moved on. Saying "answered" would present
    /// a truncated answer as a complete one.
    fn settle_superseded(&mut self) {
        let Some(active) = self.active.take() else {
            return;
        };
        self.finish_exchange(BtwExchange {
            question: active.question,
            answer: active.answer,
            outcome: BtwOutcome::Superseded,
        });
    }

    /// The active question, but only when `run_id` is the run answering it.
    ///
    /// **Every frame application goes through this.** `claimed` deliberately
    /// outlives an exchange (a settled run can still emit), so "this frame is
    /// ours" and "this frame belongs to what is on screen right now" are two
    /// different questions — and answering the second with the first is how a
    /// finished question's tail ends up appended to the next one's answer.
    fn for_active_run(&mut self, run_id: &str) -> Option<&mut BtwActive> {
        self.active
            .as_mut()
            .filter(|active| active.run_id.as_deref() == Some(run_id))
    }

    /// Record that `run_id` is answering the active side question.
    pub fn claim_run(&mut self, run_id: String) {
        if run_id.is_empty() {
            return;
        }
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
        self.active
            .as_ref()?
            .run_id
            .as_deref()
            .filter(|id| !id.is_empty())
    }

    /// Append a streamed delta to the active answer.
    pub fn push_delta(&mut self, run_id: &str, delta: &str) {
        if let Some(active) = self.for_active_run(run_id) {
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
    pub fn push_final(&mut self, run_id: &str, text: &str) {
        if let Some(active) = self.for_active_run(run_id) {
            let fresh = text.get(active.streamed_len..).unwrap_or("").to_string();
            active.streamed_len = text.len();
            active.answer.push_str(&fresh);
        }
    }

    /// Note which tool the side question is running, for the status line.
    pub fn note_tool(&mut self, run_id: &str, name: Option<String>) {
        if let Some(active) = self.for_active_run(run_id) {
            active.tool_name = name;
        }
    }

    /// Take the active question, but only when `run_id` is the run answering
    /// it.
    ///
    /// The one place a settlement asks "is this mine, and may I have it" — the
    /// three settlers below differ only in the ending they then name, and they
    /// used to repeat this check-then-take in full. A settler that forgot the
    /// check would file a *different* question's text under this run's ending,
    /// which is the misattribution [`Self::for_active_run`] exists to prevent.
    fn take_active_run(&mut self, run_id: &str) -> Option<BtwActive> {
        self.for_active_run(run_id)?;
        self.active.take()
    }

    /// Settle the active question as answered.
    ///
    /// `fallback` is the authoritative final text from the run summary, used
    /// only when nothing streamed — a turn that produced no deltas and no
    /// trace mirror still has an answer, and an empty bubble would report the
    /// opposite.
    pub fn finish_active(&mut self, run_id: &str, fallback: Option<&str>) {
        let Some(active) = self.take_active_run(run_id) else {
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
            outcome: BtwOutcome::Answered,
        });
    }

    /// Settle the active question as failed, keeping whatever text arrived
    /// before the failure — a partial answer is still worth reading.
    pub fn fail_active(&mut self, run_id: &str, error: String) {
        let Some(active) = self.take_active_run(run_id) else {
            return;
        };
        self.finish_exchange(BtwExchange {
            question: active.question,
            answer: active.answer,
            outcome: BtwOutcome::Failed(error),
        });
    }

    /// Stop claiming a side question is being answered, because the connection
    /// that was carrying its frames died and the gateway no longer reports the
    /// run in flight.
    ///
    /// Deliberately **not** a verdict. `note` is what the server said became of
    /// the run, verbatim from the caller — this overlay is not in a position to
    /// decide whether the work succeeded, only to stop showing a spinner over
    /// something nobody is going to stream to it. See
    /// [`BtwOutcome::Disconnected`] for why the four older endings all say
    /// something untrue here.
    ///
    /// Keyed on `run_id` like every other settler: by the time this is called
    /// the user may already have asked something else, and that question's text
    /// must not be filed under this run's ending.
    pub fn settle_disconnected(&mut self, run_id: &str, note: String) {
        let Some(active) = self.take_active_run(run_id) else {
            return;
        };
        self.finish_exchange(BtwExchange {
            question: active.question,
            answer: active.answer,
            outcome: BtwOutcome::Disconnected(note),
        });
    }

    /// Settle a question whose `agent.run` never came back with a run id.
    ///
    /// Separate from [`Self::fail_active`] because it is the one failure that
    /// cannot name a run: there is no run. Folding the two would mean giving
    /// `fail_active` a "match nothing" mode, and that mode would then be
    /// reachable by every caller that has a run id and gets it wrong.
    pub fn fail_unclaimed(&mut self, error: String) {
        if self
            .active
            .as_ref()
            .is_none_or(|active| active.run_id.is_some())
        {
            return;
        }
        let Some(active) = self.active.take() else {
            return;
        };
        self.finish_exchange(BtwExchange {
            question: active.question,
            answer: active.answer,
            outcome: BtwOutcome::Failed(error),
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
            outcome: BtwOutcome::Aborted,
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

    /// Drop everything that belonged to the conversation being left — and
    /// nothing that did not.
    ///
    /// # Why this is not `*self = Self::default()`
    ///
    /// Two kinds of state live here and only one of them is
    /// per-conversation. The exchanges, the page on screen, the draft and the
    /// active question are that conversation's and must go, for the reason
    /// `switch_session` wipes `messages` and `total_tokens`: this is a
    /// singleton, so state that outlives the switch reports the previous
    /// conversation under the new one's name.
    ///
    /// **`claimed` is not per-conversation — it is per-run, and clearing it
    /// re-opens the leak this overlay exists to close.** A side run whose
    /// `RunAccepted` has not landed yet is unknown to
    /// `AppState::run_sessions`, and an unknown run id is deliberately KEPT by
    /// `frame_belongs_here` ("I cannot tell" must not become "not mine"). So a
    /// forgotten claim means that run's answer renders into the transcript of
    /// a conversation it has nothing to do with — the exact defect, arriving
    /// through the fix for a different one. Keeping the claims costs nothing:
    /// the frames are still intercepted, `for_active_run` matches no active
    /// question, and they land nowhere. The FIFO bound still caps the memory.
    pub fn clear_for_session_switch(&mut self) {
        self.exchanges.clear();
        self.view_index = 0;
        self.active = None;
        self.open = false;
        self.scroll = 0;
        self.composer.clear();
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

    /// Open a question and claim the run answering it — the state every frame
    /// application now requires, because applications name their run.
    fn asking(o: &mut BtwOverlay, question: &str, run_id: &str) {
        o.begin(question.to_string());
        o.claim_pending_run(run_id.to_string());
    }

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
        asking(&mut o, "why?", "r1");
        o.push_delta("r1", "because");
        o.finish_active("r1", None);
        assert!(o.active.is_none());
        assert!(o.accepts_frame(Some("r1")));
    }

    /// A second side question asked while the first is still answering must
    /// not inherit the first's frames.
    ///
    /// `claimed` outlives an exchange on purpose, so `accepts_frame` says yes
    /// to BOTH runs — which is right for "is this the overlay's", and useless
    /// for "does this belong to what is on screen". Before frame application
    /// named its run, the first question's remaining deltas appended
    /// themselves to the second question's answer, and the first question's
    /// partial answer was discarded outright.
    #[test]
    fn a_second_question_does_not_inherit_the_first_ones_frames() {
        let mut o = BtwOverlay::default();
        asking(&mut o, "first?", "r1");
        o.push_delta("r1", "first answer");

        asking(&mut o, "second?", "r2");

        // The first question was filed with what it had, and said so.
        let filed = o.exchanges.last().expect("the first question was filed");
        assert_eq!(filed.question, "first?");
        assert_eq!(filed.answer, "first answer");
        // Its own word: nothing failed here — the user moved on, and that run
        // may well still be running and still succeed. `failed` is the word a
        // provider outage gets, and the status line is the part that survives
        // a glance.
        assert_eq!(filed.status(), "superseded");
        assert_eq!(filed.outcome, BtwOutcome::Superseded);
        assert_eq!(
            filed.note(),
            None,
            "a superseded question has no failure to report"
        );

        // Both runs are still claimed — so the routing, not the claim, is what
        // keeps them apart.
        assert!(o.accepts_frame(Some("r1")));
        assert!(o.accepts_frame(Some("r2")));

        // r1's tail must not land on q2.
        o.push_delta("r1", " ...tail");
        o.push_final("r1", "a whole different answer");
        o.note_tool("r1", Some("bash".into()));
        let active = o.active.as_ref().expect("q2 is answering");
        assert_eq!(active.question, "second?");
        assert_eq!(active.answer, "", "r1's tail leaked onto q2");
        assert_eq!(active.tool_name, None, "r1's tool leaked onto q2");

        // Nor may r1 settle q2.
        o.finish_active("r1", Some("r1 summary"));
        assert!(
            o.active.is_some(),
            "r1 settled the question it does not own"
        );
        o.fail_active("r1", "r1 blew up".into());
        assert!(o.active.is_some(), "r1 failed the question it does not own");

        // r2 does own it.
        o.push_delta("r2", "second answer");
        o.finish_active("r2", None);
        let filed = o.exchanges.last().expect("q2 filed");
        assert_eq!(filed.question, "second?");
        assert_eq!(filed.answer, "second answer");
        assert_eq!(filed.status(), "answered");
    }

    /// Two sends in flight get their OWN run ids: a claim only lands on a
    /// question that has none yet, and `begin` files the previous one, so the
    /// second send cannot resolve to the first run.
    #[test]
    fn each_send_claims_its_own_run_even_when_the_text_is_identical() {
        let mut o = BtwOverlay::default();
        asking(&mut o, "same question", "r1");
        asking(&mut o, "same question", "r2");
        assert_eq!(o.active_run_id(), Some("r2"));

        // A late claim cannot overwrite a run id already in hand.
        o.claim_pending_run("r3".to_string());
        assert_eq!(o.active_run_id(), Some("r2"));
    }

    /// A send that never came back with a run id has to be settleable, and it
    /// is the one failure that cannot name a run.
    #[test]
    fn a_send_that_never_got_a_run_id_still_settles() {
        let mut o = BtwOverlay::default();
        o.begin("why?".into());
        o.fail_unclaimed("the side question was not accepted".to_string());
        assert!(o.active.is_none());
        assert_eq!(o.current().expect("filed").status(), "failed");

        // ...and it must not settle a question that DOES have a run: that one
        // has a real answer coming.
        asking(&mut o, "live", "r1");
        o.fail_unclaimed("nope".to_string());
        assert!(o.active.is_some(), "a claimed question is not unclaimed");
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
        asking(&mut o, "q", "r1");
        o.push_delta("r1", "hello ");
        o.push_final("r1", "hello world");
        o.finish_active("r1", None);
        assert_eq!(o.current().expect("exchange").answer, "hello world");

        // A turn that never streamed lands in full.
        let mut o = BtwOverlay::default();
        asking(&mut o, "q", "r1");
        o.push_final("r1", "whole thing");
        o.finish_active("r1", None);
        assert_eq!(o.current().expect("exchange").answer, "whole thing");
    }

    /// A turn with no deltas and no trace mirror still has an answer — the run
    /// summary's. An empty bubble would report the opposite of what happened.
    #[test]
    fn an_unstreamed_answer_falls_back_to_the_run_summary() {
        let mut o = BtwOverlay::default();
        asking(&mut o, "q", "r1");
        o.finish_active("r1", Some("from the summary"));
        assert_eq!(o.current().expect("exchange").answer, "from the summary");
    }

    /// "It broke", "you stopped it" and "it said nothing" are three different
    /// things to be told, so they are three different states — and a failure
    /// keeps whatever text had arrived, because a partial answer still reads.
    #[test]
    fn the_three_endings_stay_distinguishable() {
        let mut o = BtwOverlay::default();
        asking(&mut o, "q1", "r1");
        o.push_delta("r1", "partial");
        o.fail_active("r1", "provider unreachable".into());
        let failed = o.current().expect("exchange");
        assert_eq!(failed.status(), "failed");
        assert_eq!(failed.answer, "partial");

        asking(&mut o, "q2", "r2");
        o.abort_active();
        assert_eq!(o.current().expect("exchange").status(), "aborted");

        asking(&mut o, "q3", "r3");
        o.finish_active("r3", None);
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

        asking(&mut o, "q2", "r2");
        assert_eq!(
            o.copyable(),
            Some("old answer"),
            "a question that has not answered yet must not shadow the page on screen"
        );
        o.push_delta("r2", "new answer");
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
        asking(&mut o, "q1", "r1");
        o.push_delta("r1", "a1");
        o.finish_active("r1", None);
        o.close();
        assert!(!o.open);
        assert_eq!(o.exchanges.len(), 1);

        asking(&mut o, "q2", "r2");
        assert!(o.open);
        assert_eq!(o.exchanges.len(), 1);
    }

    /// The reconnect repair's landing: the spinner stops, whatever streamed is
    /// kept, and the word is neither `answered` nor `failed`.
    ///
    /// `answered` would present a possibly-truncated prefix as the whole
    /// answer; `failed` would report a break over a run that, on the commonest
    /// path here, finished cleanly with nobody listening.
    #[test]
    fn a_question_settled_after_a_reconnect_keeps_its_text_and_says_why() {
        let mut o = BtwOverlay::default();
        asking(&mut o, "what is a monoid?", "r-side");
        o.push_delta("r-side", "a monoid is");

        o.settle_disconnected("r-side", "the gateway reports the run finished".to_string());

        assert!(o.active.is_none(), "the spinner must stop");
        let filed = o.current().expect("the question was filed");
        assert_eq!(filed.answer, "a monoid is", "the partial answer is kept");
        assert_eq!(filed.status(), "disconnected");
        assert_ne!(filed.status(), "answered");
        assert_eq!(
            filed.note(),
            Some("the gateway reports the run finished".to_string()),
            "what the server said is the part the user needs"
        );
    }

    /// The note is NOT captioned as an error. `note()` owns the wording per
    /// outcome precisely so a finished-while-away question is not labelled the
    /// way a provider outage is.
    #[test]
    fn only_a_failure_is_captioned_as_one() {
        let mut o = BtwOverlay::default();
        asking(&mut o, "q", "r1");
        o.settle_disconnected("r1", "the gateway has no record of the run".to_string());
        let note = o.current().and_then(BtwExchange::note).expect("a note");
        assert!(!note.starts_with("Error:"), "not an error: {note}");

        asking(&mut o, "q2", "r2");
        o.fail_active("r2", "provider 429".to_string());
        assert_eq!(
            o.current().and_then(BtwExchange::note),
            Some("Error: provider 429".to_string()),
            "a real failure still says so"
        );
    }

    /// Keyed on the run id like every other settler. By the time the repair
    /// runs the user may already have asked something else — filing that
    /// question's text under the old run's ending is the misattribution
    /// `for_active_run` exists to prevent.
    #[test]
    fn the_repair_cannot_settle_a_question_it_is_not_about() {
        let mut o = BtwOverlay::default();
        asking(&mut o, "second question", "r-new");
        o.push_delta("r-new", "still going");

        o.settle_disconnected("r-old", "the gateway has no record of the run".to_string());

        let active = o.active.as_ref().expect("the live question is untouched");
        assert_eq!(active.question, "second question");
        assert_eq!(active.answer, "still going");
        assert!(o.exchanges.is_empty(), "nothing was filed");
    }
}
