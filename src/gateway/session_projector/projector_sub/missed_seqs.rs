//! Missed-seq tracking for the session projector.
//!
//! Split out of `crate::gateway::session_projector`. The projector remembers
//! which seq ids are known to be absent from the projection and which sessions
//! have gained a new one since the last heal.
//!
//! `dirty` exists so a heal is triggered by NEW information only. Re-inserting
//! a seq that a pass could not resolve (a `RunMeta` whose run produced no
//! assistant row at all, say) must not re-arm the heal, or every subsequent
//! event on that session would pay for a full transcript read forever.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::session::events::EventSeq;
use crate::session::service::SessionId;

/// What one `heal_session` pass did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RepairReport {
    /// Transcript rows that were absent and are now written.
    pub holes_filled: usize,
    /// `AssistantRunMeta` stamps that landed on a row that had none.
    pub stamps_reapplied: usize,
    /// Stamps this pass wrote for a finished run whose `AssistantRunMeta`
    /// never reached the log. Several shapes of run never get one, and at
    /// boot the routine ones outnumber the crash: an errored or cancelled run
    /// (the meta is emitted only on the engine's `Ok` arm), a slash-command
    /// fast-path turn (no model call, no meta), a run the resume coordinator
    /// closed as `Abandoned` and then repainted, the CHILD side of a session
    /// split (the meta lands on the parent — `execute()` stamps
    /// `request.session_key` and never learns the adopted child — so the
    /// child's post-split span is the meta-less one), and the process dying
    /// between `RunFinished` and the meta. Carries the `run_id` alone (the
    /// gauge is unknown and is never written as zeros). Only a
    /// `WholeSession` pass writes these — see [`super::run_span::synthesize_missing_stamps`].
    pub stamps_synthesized: usize,
    /// Of the stamps above (re-applied or synthesized), how many also
    /// accumulated the run's spend. A stamp that found the row already
    /// carrying this run's id bills nothing — that is what makes a replay,
    /// and a second heal, non-double-billing.
    pub usage_rebilled: usize,
    /// Nothing was missing, nothing was stamped, and nothing was deferred.
    ///
    /// The last clause is the one that is easy to drop: a seq the pass could
    /// not resolve sets no counter at all, so a heal that wrote nothing
    /// *because it could not* would otherwise be indistinguishable from a
    /// session that needed nothing.
    pub up_to_date: bool,
    /// The transcript is non-empty and carries no projector seq ids (foreign
    /// or pre-SSOT content). Nothing was written: without seqs there is no way
    /// to tell a hole from a row this projector never wrote, and filling
    /// blindly would duplicate the conversation.
    pub legacy: bool,
    /// Something could not be read or written, so this report does NOT say the
    /// session is whole — it says the pass could not find out.
    pub errored: bool,
}

/// Seqs known to be absent from the projection, and which sessions have gained
/// one since the last heal.
#[derive(Default)]
pub(crate) struct MissedSeqs {
    pub(crate) seqs: HashMap<SessionId, BTreeSet<EventSeq>>,
    dirty: HashSet<SessionId>,
}

impl MissedSeqs {
    /// A newly-discovered gap: remember it AND arm the next heal.
    pub(crate) fn record(&mut self, id: &SessionId, seq: EventSeq) {
        self.seqs.entry(id.clone()).or_default().insert(seq);
        self.dirty.insert(id.clone());
    }

    /// A gap this pass could not close: remember it WITHOUT arming a heal.
    pub(crate) fn restore(&mut self, id: &SessionId, seqs: BTreeSet<EventSeq>) {
        if seqs.is_empty() {
            return;
        }
        self.seqs.entry(id.clone()).or_default().extend(seqs);
    }

    /// Take this session's gaps for a heal pass to work on.
    pub(crate) fn take(&mut self, id: &SessionId) -> BTreeSet<EventSeq> {
        self.dirty.remove(id);
        self.seqs.remove(id).unwrap_or_default()
    }

    /// Has this session gained a gap since the last heal?
    pub(crate) fn is_dirty(&self, id: &SessionId) -> bool {
        self.dirty.contains(id)
    }
}

/// `flush` gave up waiting. Not "the drain is empty" — the caller does not know
/// what the drain still holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the projector drain did not settle within the flush timeout")]
pub struct FlushTimeout;
