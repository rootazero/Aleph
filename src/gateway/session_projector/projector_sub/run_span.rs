//! Per-run span book-keeping for the session projector.
//!
//! Split out of `crate::gateway::session_projector`. A heal pass walks the log
//! and rebuilds the shape of every run it sees — where it opened, where (if
//! anywhere) it closed, how many assistant messages it produced, and whether
//! an `AssistantRunMeta` landed on it — then synthesizes stamps for any run
//! whose meta never reached the log.

use std::sync::Arc;

use crate::gateway::session_store::{SessionStore, StampOutcome};
use crate::session::events::{EventSeq, SessionEvent, SessionEventRecord};
use crate::session::service::SessionId;
use crate::session::store::{RetiredAnchorKind, RetiredRunAnchor, SessionEventStore};

use super::missed_seqs::RepairReport;

/// One run's extent in the log, as a heal pass sees it: where it opened,
/// where (if anywhere) it closed, how many assistant messages it produced,
/// and whether an `AssistantRunMeta` landed on it.
pub(crate) struct RunSpan {
    /// The id its `RunStarted` carries — the harness-minted MARKER id
    /// (`runner_impl.rs`), which is NOT the engine's `RunRequest.run_id` the
    /// meta carries — equal since 2026-09-24 (F1), different in older logs;
    /// the join is positional, so either reads the same. It is what a
    /// synthesized stamp writes under the row's `run_id` key, and it names
    /// this span in log lines; it is never compared to a meta's id.
    run_id: String,
    /// Seq of the `RunStarted` that opened it.
    start: EventSeq,
    /// Seq of the `RunFinished` that closed it; `None` while it is open.
    end: Option<EventSeq>,
    /// `AssistantMessage` events between `start` and `end` — after the walk in
    /// [`super::super::heal_session`], each is a transcript row or a deferred seq.
    assistant_messages: usize,
    /// A meta was appended after this span opened and before the next one
    /// did — see [`collect_run_spans`] for why that is the join.
    meta: bool,
}

impl RunSpan {
    /// The seq to stamp up to when this run needs a synthesized stamp — it
    /// finished, produced at least one assistant message, and no meta for it
    /// is in the log. `None` for every other shape, each for its own reason:
    /// an open run's meta may still come; a run with no assistant message has
    /// no row to stamp; and a run WITH a meta keeps its own stamp — landed or
    /// deferred, that meta carries the gauge and the price, which a
    /// synthesized stamp cannot.
    pub(crate) fn synthesis_end(&self) -> Option<EventSeq> {
        if self.assistant_messages == 0 || self.meta {
            return None;
        }
        self.end
    }
}

/// Fold the log into run spans. Pure, so the shape a stamp is synthesized
/// for is unit-testable without a store.
///
/// Every `RunStarted` opens a new span and a `RunFinished` closes the NEWEST
/// one (the same "last marker wins" reading as `reduction::reduce_run`), so
/// no span holds a second `RunStarted` between its `start` and its `end`.
/// That is what lets the stamp range and the usage fold agree by
/// construction: the fold anchors on the last `RunStarted` in the slice it
/// is given, and over `[start, end)` that is `start`. A split child's log
/// carries the parent's opener copied inside the tail and the child's own
/// opener last (`session_split`); the copied one opens a span that is never
/// closed, and so is never synthesized. The split's meta lands on the
/// PARENT: `execute()` stamps `request.session_key` and never learns the
/// adopted child (`final_session_id()` is read only by the harness bridge,
/// for the `RunFinished`), so the parent's `RunStarted(p) … RunFinished {
/// R } … AssistantRunMeta` is a span WITH a meta — billed live by
/// the meta arm for its pre-split tokens plus the whole run's `cost_usd` and
/// model. The CHILD's own span, `RunStarted { R } … RunFinished` (split
/// reuses the parent run's id), is the meta-less one: synthesized and billed
/// (tokens only) at the next whole-session heal, so its tokens reach its
/// session row no sooner than that (FOLLOW-UP F26). Assistant messages count
/// into the newest span only while it is open.
///
/// A meta marks the NEWEST span — the join is POSITIONAL, not by id. The
/// older logs' markers carry a harness-minted id that is never the meta's;
/// newer logs carry the engine id on both (F1). The join below is positional
/// and reads both, so an id join never matched: every finished run read as
/// meta-less at boot, a stamp was synthesized and billed on EVERY boot, and
/// the two
/// stamps then overwrote each other's `run_id` so the next boot re-applied
/// both — session totals doubled per restart (44 → 88 → 176 on the real
/// machine). Position is the same derivation the projector's meta arm uses
/// (`ctx.run_start` is the last `RunStarted` it walked past) and the usage
/// fold anchors on (the last `RunStarted` in its slice), so the three cannot
/// name three different runs. Two consequences: on a log written after
/// `execute()` began holding the run slot through the meta append, the meta
/// always precedes the next opener, so "newest span" is this run; on a
/// historical log where a meta landed AFTER the next run's opener, it marks
/// the next span — the same reading under which the projector anchored it
/// on that opener and finalised it `NoRowInRange` — and the older span reads
/// meta-less and is synthesized, billing that run's tokens once from its own
/// messages. That is one bill for the older run, not two: its misanchored
/// meta billed nothing (`NoRowInRange` never reaches the bill). What the
/// historical shape still costs is on the projector's side, not this fold's
/// — that meta can stamp and bill the NEXT run's first row under the older
/// run's id if the next run had already produced one — and stays a T24
/// known limit; the heal does not second-guess a landed meta.
///
/// `retired` — the session's retired openers and metas
/// ([`SessionEventStore::load_retired_run_anchors`]), merged into the walk by
/// `seq` — is the same positional join applied to the rows a rewind took
/// out of the live log. A `chat.rewind` / `session.truncate` / `/undo` whose
/// cut falls inside a finished, billed run retires that run's meta (and
/// maybe its stamped row) and appends a `Cancelled` closer; over the live
/// rows alone the run is then finished-without-meta and would be synthesized
/// and billed again for whatever survived (final review I1). A retired meta
/// marks the span the newest opener before it names, exactly as a live one
/// does; a retired opener MOVES that anchor (so a retired later run's meta
/// cannot be read as the surviving earlier run's) and opens no span of its
/// own — its rows are gone from the fold and from `messages`. A live closer
/// closes the newest LIVE span: the balancing closer is appended for the run
/// the surviving prefix leaves open, after every retired row. Retired
/// assistant messages and closers are not consulted: neither is billed, and
/// a span's `end` must be a live seq.
pub(crate) fn collect_run_spans(
    events: &[SessionEventRecord],
    retired: &[RetiredRunAnchor],
) -> Vec<RunSpan> {
    let mut spans: Vec<RunSpan> = Vec::new();
    // Whether the newest opener walked so far — live or retired — is the one
    // that opened `spans.last()`. False after a retired opener: the newest
    // run is one the fold cannot see, so nothing that follows positionally
    // (a message, a meta) belongs to a span in `spans`.
    let mut newest_opener_is_live = false;
    let mut retired = retired.iter().peekable();
    let note_retired =
        |anchor: &RetiredRunAnchor, spans: &mut Vec<RunSpan>, live: &mut bool| match anchor.kind {
            RetiredAnchorKind::RunStarted => *live = false,
            RetiredAnchorKind::RunMeta => {
                if *live {
                    if let Some(span) = spans.last_mut() {
                        span.meta = true;
                    }
                }
            }
        };
    for rec in events {
        while let Some(anchor) = retired.next_if(|a| a.seq < rec.seq) {
            note_retired(anchor, &mut spans, &mut newest_opener_is_live);
        }
        match &rec.event {
            SessionEvent::RunStarted { run_id, .. } => {
                spans.push(RunSpan {
                    run_id: run_id.clone(),
                    start: rec.seq,
                    end: None,
                    assistant_messages: 0,
                    meta: false,
                });
                newest_opener_is_live = true;
            }
            // The two positional readers fall through to `_` after a retired
            // opener: what they would mark belongs to a run the fold cannot see.
            SessionEvent::AssistantMessage { .. } if newest_opener_is_live => {
                if let Some(span) = spans.last_mut().filter(|s| s.end.is_none()) {
                    span.assistant_messages += 1;
                }
            }
            SessionEvent::RunFinished { .. } => {
                if let Some(span) = spans.last_mut() {
                    span.end.get_or_insert(rec.seq);
                }
            }
            SessionEvent::AssistantRunMeta { .. } if newest_opener_is_live => {
                if let Some(span) = spans.last_mut() {
                    span.meta = true;
                }
            }
            _ => {}
        }
    }
    for anchor in retired {
        note_retired(anchor, &mut spans, &mut newest_opener_is_live);
    }
    spans
}

/// Stamp and bill every finished run whose `AssistantRunMeta` never reached
/// the log, so the run's rows never got their `run_id` join and its spend
/// never reached the session row. #11 names the crash between `RunFinished`
/// and the meta; the routine shapes that leave the same hole are listed on
/// [`RepairReport::stamps_synthesized`], and this pass treats them alike.
///
/// The stamp carries the `run_id` alone — `build_message_metadata` with no
/// occupancy — because the gauge is unknown here and a zero would read as a
/// measurement on the Panel. It goes through the same
/// `stamp_assistant_metadata_in_range` as a real meta, over the same shape of
/// range (`(start, end]`: the run's own `RunStarted` to its `RunFinished`, the
/// rows strictly between), so a later heal reads `AlreadyStamped` and bills
/// nothing: the stamp is the idempotence guard, exactly as on the live path.
/// The bill is [`bill_run_from_fold`] with `run_start = start` and the fold
/// read up to `end`; the cost and model are `None` because there is no meta
/// to take them from, so a synthesized bill adds tokens and never dollars.
/// A run whose provider reported no usage
/// is stamped (the join is still owed) and not billed — nothing to add, and
/// `bill_run_from_fold` says nothing about it.
///
/// `NoRowInRange` here means the run's row is a hole this pass could not
/// fill — it is in `retry`, `up_to_date` is already false, and the pass that
/// fills it synthesizes. A refused stamp sets `errored`: unlike a deferred
/// meta there is no seq to retry, and the next whole-session pass finds the
/// stamp still missing.
///
/// The boot reconciler runs before any run is live, so it cannot race a meta
/// that is about to be appended. A `request_repair` on a live session (the
/// doctor's repair) can, in the window between a run's `RunFinished` and its
/// meta: the synthesized stamp lands first, under the run's own id since the
/// markers carry the engine id (F1), so the meta then reads `AlreadyStamped`
/// and bills nothing. The run's tokens are billed once; the cost and model
/// the meta carried are not (ruling U5). Logs written before F1 still carry
/// a marker id there, and for them the window still double-bills — U1.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn synthesize_missing_stamps(
    store: &Arc<dyn SessionStore>,
    id: &SessionId,
    event_store: &Arc<dyn SessionEventStore>,
    present: &(dyn Fn(EventSeq) -> bool + Send + Sync),
    spans: &[RunSpan],
    report: &mut RepairReport,
) {
    for span in spans {
        let Some(end) = span.synthesis_end() else {
            continue;
        };
        let Some(meta) =
            crate::gateway::agent_instance::build_message_metadata(Some(&span.run_id), None)
        else {
            continue;
        };
        match store
            .stamp_assistant_metadata_in_range(id, span.start, end, &meta)
            .await
        {
            Ok(StampOutcome::Stamped) => {
                report.stamps_synthesized += 1;
                let ctx = ProjectionCtx {
                    store,
                    events: Some(event_store),
                    present,
                    run_start: span.start,
                    bus: None,
                };
                if bill_run_from_fold(id, end, &ctx, &span.run_id, None, None, None).await {
                    report.usage_rebilled += 1;
                }
            }
            Ok(StampOutcome::AlreadyStamped | StampOutcome::NoRowInRange) => {}
            Err(e) => {
                tracing::warn!(
                    session = ?id,
                    run_id = %span.run_id,
                    error = %e,
                    "heal: synthesized stamp failed"
                );
                report.errored = true;
            }
        }
    }
}

// Pulled in for the `synthesize_missing_stamps` body. Defined in the parent
// module's body (after the split these stay in `session_projector.rs`).
use super::super::{bill_run_from_fold, ProjectionCtx};
