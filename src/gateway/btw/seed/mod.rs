//! Incremental fork seeding for the side session.
//!
//! # Why incremental
//!
//! The side session key is deterministic (see [`super::side_key_for`]), so the
//! side session persists across questions — that is what gives the side thread
//! its memory. Re-seeding the whole main transcript on each question would
//! therefore append the same prefix again and again (`seed₁ + Q1A1 +
//! seed₁seed₂ + …`), and each re-seed would re-key the provider prefix cache
//! for a conversation that is meant to be cheap.
//!
//! Seeding only what closed since a cursor keeps the side transcript
//! append-only, which is exactly what prefix caching rewards.
//!
//! # Three separate things have to be true, and they have three separate
//! # mechanisms
//!
//! It is easy to read any one of these as covering the others. They do not
//! overlap, and each names its own enforcement:
//!
//! 1. **No other writer clobbers the cursor key.** Owned by the session
//!    store: both backends make the read-modify-write of the
//!    `identity_meta.custom` bag one critical section (the file backend
//!    through its `MetaGuard`, SQLite by holding the connection mutex across
//!    `SELECT` and `UPDATE`) and merge it key-by-key, so writing this key
//!    cannot disturb the exec tier or the model pin that share the bag. This
//!    is about the metadata **document** and nothing else.
//! 2. **No two seedings of one side session interleave.** Owned by
//!    [`gate`] — `patch_session`'s critical section does *not* span
//!    `ensure_seeded`'s read → copy → write, and without a lock two concurrent
//!    side questions copy the same delta twice. See that module.
//! 3. **The cursor has exactly one writer.** [`write_cursor`], called from the
//!    one place that performs the copy. A second writer would be a second
//!    answer to "how much have we carried", and the two would disagree on the
//!    first interleaved question.
//!
//! # Where the cursor lives, and why it is not derived
//!
//! On the **side session's own row**, in the `identity_meta.custom` bag that
//! already holds the exec tier, session mode, thinking depth, memory mode and
//! model pin. It survives a restart, which is the property that matters: an
//! in-process cursor would read as "never seeded" after every daemon restart
//! and re-carry the whole prefix.
//!
//! It is deliberately **not derived** from the side session's own log — for
//! instance by collecting the turn ids already present there. That derivation
//! reads as free, and it is wrong in one specific case that matters: a cold
//! seed over budget drops the *oldest* turns, so those turn ids are absent from
//! the side log while being permanently behind us. A derived cursor would
//! re-copy them on the next question and append them **after** the side thread's
//! own questions, producing a transcript that reads Q3, Q4, [side Q&A], Q1, Q2.
//! The cursor records how far we *read*, which is not the same question as what
//! the plan chose to *keep*.
//!
//! A new [`crate::session::events::SessionEvent`] variant was the other
//! candidate and was refused: a protocol change plus every exhaustive match arm
//! in the repo, for a private bookkeeping value.
//!
//! # The window this leaves open, at its real width
//!
//! The copy and the cursor write are two steps. A process that dies between
//! them — or a `write_cursor` that fails — leaves the cursor describing less
//! than the side transcript holds, and the next question re-carries. **How
//! much** it re-carries depends on which arm was running, and the expensive
//! case is the first one:
//!
//! * **Warm arm:** one delta arrives twice. Bounded by whatever closed since
//!   the last question.
//! * **Cold arm:** the cursor is still `None`, so the next question re-enters
//!   the cold arm and re-copies the **entire** prefix up to `max_chars`. The
//!   *marker* half of that is closed — the cold arm emits `SessionForked` only
//!   when the side session does not already carry one, so "exactly one marker
//!   per side session" holds by construction rather than only where every
//!   cursor write succeeded. The duplicated prefix is **not** closed.
//!
//! The direction is chosen rather than merely tolerated: writing the cursor
//! *first* would make a crash lose those events permanently and silently, and a
//! duplicated turn is visible in the transcript while a hole is not. `/new`
//! bumps the epoch and so re-derives the side key, which clears the side thread
//! outright — the one action a user would already reach for. Closing the window
//! properly means making the marker and the cursor land as one durable fact,
//! which is a larger change than this layer warrants; it is **not** covered
//! here and the marker test pins the top-up path, not this one.

pub(crate) mod gate;

use std::collections::HashMap;

use crate::agents::subagent_spawner::fork::{self, ForkBudget};
use crate::gateway::session_store::types::SessionPatch;
use crate::gateway::session_store::SessionStore;
use crate::session::events::{EventSeq, SessionEvent, SessionEventRecord};
use crate::session::service::{SessionId, SessionService};

/// Metadata key on the SIDE session holding the last-carried main event seq.
pub(crate) const CURSOR_KEY: &str = "btw_seed_cursor";

/// What one [`ensure_seeded`] call actually carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SeedOutcome {
    /// Events copied from the main session this call. `0` means the side
    /// session was already current — the normal case for a follow-up asked
    /// seconds after the previous one, and also the case where main is
    /// mid-turn and there is nothing settled to carry yet.
    pub(crate) events_added: usize,
    /// How far into the main log the side session has now been carried.
    /// `None` only when nothing has ever been carried.
    pub(crate) cursor: Option<EventSeq>,
}

/// Bring `side` up to date with `main`.
///
/// # The side session row is created under the run's attribution
///
/// This calls `SessionStore::get_or_create(side)`, whose CREATE branch stamps
/// `owner_user_id`/`scope_id` from the ambient scope — **create-only**, so a
/// row created without one is wrong permanently, and on a multi-user install
/// that means a member's side thread is attributed to the machine owner and
/// `handlers::trace`'s cross-user read audit never fires for it.
///
/// The attribution therefore is not left to the caller's task-local, and
/// "call this from inside the run's scope" would not be a satisfiable
/// instruction: `crate::scope::current_scope()` is `None` in the gateway
/// dispatch loop too, which scopes `CALLER_USER`/`CALLER_ROLE` rather than the
/// attribution. The resolved attribution exists **only in the run's metadata
/// map**, so this takes `run_metadata` and re-establishes the scope itself,
/// through the same `scope_from_metadata` accessor
/// `run_loop::ensure_session_under_request_scope` uses for the identical
/// problem one layer up. Pass the run request's own `metadata`; do not rebuild
/// one.
///
/// # What `budget` bounds
///
/// The **cold** seed only, and both halves matter: the turn count keeps the
/// fork to recent context, and the char ceiling keeps a cold seed below the
/// side agent's own compaction threshold, so it does not pay an LLM to
/// summarise the history we just paid to copy. Once a cursor exists the delta
/// is whatever settled since, which is naturally small; clamping the delta too
/// would silently drop the middle of a long-running main session and leave the
/// side agent reading a transcript with a hole in it that nothing announces.
///
/// # Concurrency
///
/// Serialised per side session by [`gate`]; concurrent callers queue rather
/// than interleave.
///
/// Not `pub`: [`ForkBudget`] is crate-private, and every surface that reaches a
/// side question is inside this crate.
pub(crate) async fn ensure_seeded(
    session: &dyn SessionService,
    store: &dyn SessionStore,
    main: &SessionId,
    side: &SessionId,
    run_metadata: &HashMap<String, String>,
    budget: &ForkBudget,
) -> Result<SeedOutcome, String> {
    // Held for the whole read → copy → write. Dropped on every return path,
    // including the `?`s below.
    let _seeding = gate::acquire(side).await;

    // The cursor lives on the side session's row, so the row has to exist
    // before the copy — `patch_session` reports `Ok(false)` for an absent row,
    // and a cursor that is silently never written is the doubling this module
    // prevents.
    crate::scope::with_scope(
        crate::scope::scope_from_metadata(run_metadata),
        store.get_or_create(side),
    )
    .await
    .map_err(|e| format!("btw: open side session: {e}"))?;

    let cursor = read_cursor(store, side).await?;

    let carried = match cursor {
        // Cold: plan against the whole settled main log, so the char ceiling
        // is honoured.
        //
        // Not `fork::seed`, which would mark unconditionally. Reaching the
        // cold arm does not prove the fork is new: a lost cursor write lands
        // here a second time (see the module doc), and a second marker would
        // claim two forks where one happened. So the marker is emitted only
        // when the side session does not already carry one — the invariant
        // then holds by construction rather than only on the path where every
        // cursor write succeeded. The prefix still re-copies in that window;
        // that half is stated in the module doc, not fixed here.
        None => {
            let source = fork::snapshot(session, main).await?;
            let plan = fork::plan(settled_prefix(&source), budget);
            if plan.is_empty() {
                None
            } else {
                if !already_forked(session, side).await? {
                    fork::mark_forked(session, main, side)
                        .await
                        .map_err(|e| format!("btw: {e}"))?;
                }
                fork::seed_events(session, side, &plan.events)
                    .await
                    .map_err(|e| format!("btw: seed side session: {e}"))?;
                Some(plan)
            }
        }
        // Warm: carry whatever settled since. `get_events` is `seq >= from`, so
        // the delta starts one past what we already hold. Reading a range
        // rather than filtering a full snapshot also degrades correctly when
        // the event the cursor names has been retired by compaction: the range
        // still yields whatever lives at or above it, where a scan looking for
        // that exact event would find nothing and carry nothing, forever.
        Some(c) => {
            let delta = session
                .get_events(main, Some(c.saturating_add(1)), None)
                .await
                .map_err(|e| format!("btw: read main log: {e}"))?;
            let plan = fork::plan(settled_prefix(&delta), &delta_budget());
            if plan.is_empty() {
                None
            } else {
                // Not `fork::seed`: re-emitting `SessionForked` on every top-up
                // would claim N forks where one happened, and the provenance
                // classification reads that marker.
                fork::seed_events(session, side, &plan.events)
                    .await
                    .map_err(|e| format!("btw: seed side session: {e}"))?;
                Some(plan)
            }
        }
    };

    let Some(plan) = carried else {
        return Ok(SeedOutcome {
            events_added: 0,
            cursor,
        });
    };

    // Deliberately not guarded by `advanced != cursor` or an `if let`: both
    // were structurally always true, and a tautological guard reads as
    // idempotence protection that does not exist. `read_through` is `Some`
    // whenever the plan is non-empty, and this arm is only reached with a
    // non-empty plan — so if it is ever `None`, an edit upstream has broken
    // that invariant and the honest response is to say so. Falling back to the
    // old cursor here would re-carry this same delta on every later question,
    // forever, silently.
    let Some(seq) = plan.read_through else {
        return Err(format!(
            "btw: carried {} events but the plan reported no source seq — \
             refusing to leave the cursor behind",
            plan.events.len()
        ));
    };
    write_cursor(store, side, seq).await?;
    Ok(SeedOutcome {
        events_added: plan.events.len(),
        cursor: Some(seq),
    })
}

/// Has this side session already been stamped as a fork?
///
/// Asked once per cold seed — which is once per side session on the path where
/// nothing was lost — so the cost of the scan is not on the hot path.
async fn already_forked(session: &dyn SessionService, side: &SessionId) -> Result<bool, String> {
    Ok(session
        .get_events(side, None, None)
        .await
        .map_err(|e| format!("btw: read side log: {e}"))?
        .iter()
        .any(|r| matches!(r.event, SessionEvent::SessionForked { .. })))
}

/// The prefix of `records` that main's own log proves is behind us.
///
/// [`fork::plan`]'s notion of a closed turn is "no outstanding tool call". That
/// is exactly right for the one-shot fork it was written for, where the parent
/// is provably *inside* the `subagent` call being served. Sampled repeatedly
/// against a main session that is still running it is a snapshot predicate: a
/// multi-step assistant turn is momentarily free of open calls in the gap
/// between an `AssistantMessage` and the next `ToolCallRequested`, and a delta
/// cut there carries half of that turn, lets the side thread append its own
/// Q&A, and carries the other half next time — one main turn split across two
/// places with foreign content wedged between them. That is not a rare
/// interleaving here: a side question asked while the main run works is the
/// entire premise of the feature.
///
/// So the cut uses the log's own turn-boundary evidence rather than a sampled
/// predicate. `SessionEvent::TurnStarted`'s doc states the rule this relies on:
/// *a turn ends when the next one opens or when the run does*. Hence
///
/// * a `RunFinished` proves everything through it is over — cut after it;
/// * a `TurnStarted` proves everything before it is over — cut before it.
///
/// This is not a second answer to `plan`'s question — it answers a different
/// one, about where to cut, and `plan` then decides what inside that cut is
/// worth carrying. `TurnStarted` is never prompt-bearing, so `plan` drops it.
/// `RunFinished` is prompt-bearing for exactly one outcome, `Cancelled` (it
/// renders as the interruption note — see [`fork::is_prompt_bearing`]), and
/// that is fine here rather than an oversight: the cut runs *after* such a
/// marker, so it lands inside the carried slice, and `group_into_turns` has an
/// explicit turn-less arm that files it with the turn it interrupted instead of
/// opening a bucket of its own.
///
/// Returning an empty slice is the correct answer, not a failure: it means main
/// is mid-turn and nothing has settled since the last question. The next call
/// carries it once a marker lands.
///
/// **A marker always lands in production, but because of who calls what, not
/// because either emit promises it.** Neither is a callee guarantee:
///
/// * `harness_bridge::runner_impl` emits `RunFinished` on the completed,
///   cancelled and errored paths alike — but the emit is fail-soft (a store
///   error is warned and swallowed), and a run that `?`s out *before*
///   `harness.run` never reaches it at all.
/// * `harness_bridge::session_seed` opens a `TurnStarted` for `History` and
///   `Multimodal` inputs only; `Prompt` and `Messages` seed user messages
///   without one.
///
/// Every gateway turn is one of those two shapes and does reach `harness.run`,
/// which is why the empty slice is a "not yet" rather than a "never" — but that
/// is a fact about this caller, and a producer that stopped being either shape
/// would strand a side session on a stale cursor with nothing saying so.
///
/// [`fork::is_prompt_bearing`]: crate::agents::subagent_spawner::fork::is_prompt_bearing
fn settled_prefix(records: &[SessionEventRecord]) -> &[SessionEventRecord] {
    let mut cut = 0usize;
    for (i, record) in records.iter().enumerate() {
        match record.event {
            SessionEvent::RunFinished { .. } => cut = i + 1,
            SessionEvent::TurnStarted { .. } => cut = i,
            _ => {}
        }
    }
    &records[..cut]
}

/// The delta's own budget: unbounded.
///
/// See [`ensure_seeded`] — a clamp here drops the middle of the conversation
/// rather than its head, which nothing announces and no receipt describes. If a
/// delta is genuinely large, the side agent's own context manager is the layer
/// that is allowed to lose things, because it says so when it does.
fn delta_budget() -> ForkBudget {
    ForkBudget {
        max_turns: None,
        max_chars: usize::MAX,
    }
}

async fn read_cursor(
    store: &dyn SessionStore,
    side: &SessionId,
) -> Result<Option<EventSeq>, String> {
    let meta = store
        .get_metadata(side)
        .await
        .map_err(|e| format!("btw: read seed cursor: {e}"))?;
    interpret_cursor(
        meta.as_ref()
            .and_then(|m| m.identity_meta.as_ref())
            .and_then(|im| im.custom.get(CURSOR_KEY)),
    )
}

/// Read a stored cursor value.
///
/// Absent and `null` both mean "nothing carried yet" — both stores merge this
/// bag key-by-key with nulls included, so a null is how a key gets cleared.
/// Anything else is an **error**, never an absence: only `Ok` may assert
/// something about what is stored, and reading a value this code cannot
/// interpret as "no cursor" would re-carry the whole prefix.
pub(crate) fn interpret_cursor(
    raw: Option<&serde_json::Value>,
) -> Result<Option<EventSeq>, String> {
    match raw {
        None => Ok(None),
        Some(v) if v.is_null() => Ok(None),
        Some(v) => v.as_u64().map(Some).ok_or_else(|| {
            format!(
                "btw: seed cursor is not an event seq: {v} — refusing to re-carry the transcript"
            )
        }),
    }
}

async fn write_cursor(
    store: &dyn SessionStore,
    side: &SessionId,
    seq: EventSeq,
) -> Result<(), String> {
    let mut bag = serde_json::Map::new();
    bag.insert(CURSOR_KEY.to_string(), serde_json::json!(seq));
    let patch = SessionPatch {
        metadata: Some(serde_json::Value::Object(bag)),
        ..Default::default()
    };
    match store.patch_session(side, &patch).await {
        Ok(true) => Ok(()),
        // The row was opened at the top of this call, so its absence now is a
        // real fault. Swallowing it would leave the copy done and the cursor
        // unmoved — every later question would re-carry the same prefix.
        Ok(false) => Err(format!(
            "btw: seed cursor not written: side session {} has no row",
            side.to_key_string()
        )),
        Err(e) => Err(format!("btw: write seed cursor: {e}")),
    }
}
