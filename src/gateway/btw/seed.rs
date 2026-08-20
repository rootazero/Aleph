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
//! # The cursor has one writer
//!
//! [`ensure_seeded`] writes it, in the same step that performs the copy. A
//! second writer would be a second answer to "how much have we carried", and
//! the two would disagree on the first interleaved question.
//!
//! # Where the cursor lives, and why it is not derived
//!
//! On the **side session's own row**, in the `identity_meta.custom` bag that
//! already holds the exec tier, session mode, thinking depth, memory mode and
//! model pin. Three properties decided it:
//!
//! * **It survives a restart.** An in-process cursor would read as "never
//!   seeded" after every daemon restart and re-carry the whole prefix — the
//!   exact doubling this module exists to prevent.
//! * **It is written under a lock.** Both backends make their read-modify-write
//!   of that bag one critical section (the file backend through its
//!   `MetaGuard`, SQLite by holding the connection mutex across `SELECT` and
//!   `UPDATE`), so this writer cannot lose or be lost to a concurrent one.
//! * **It is merged key-by-key**, so writing this one key cannot clobber a dial
//!   somebody set on the same row.
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
//! # The one window this leaves open
//!
//! The copy and the cursor write are two steps. A process that dies between
//! them re-carries that delta once on the next question. That direction is
//! chosen rather than merely tolerated: writing the cursor *first* would make a
//! crash lose those events permanently and silently, and a duplicated turn is
//! visible in the transcript while a hole is not. `/new` clears the side thread
//! outright, so the duplicate is also recoverable by the one action a user
//! would already reach for.

use crate::agents::subagent_spawner::fork::{self, ForkBudget};
use crate::gateway::session_store::types::SessionPatch;
use crate::gateway::session_store::SessionStore;
use crate::session::events::EventSeq;
use crate::session::service::{SessionId, SessionService};

/// Metadata key on the SIDE session holding the last-carried main event seq.
pub(crate) const CURSOR_KEY: &str = "btw_seed_cursor";

/// What one [`ensure_seeded`] call actually carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SeedOutcome {
    /// Events copied from the main session this call. `0` means the side
    /// session was already current — the normal case for a follow-up asked
    /// seconds after the previous one.
    pub(crate) events_added: usize,
    /// How far into the main log the side session has now been carried.
    /// `None` only when nothing has ever been carried.
    pub(crate) cursor: Option<EventSeq>,
}

/// Bring `side` up to date with `main`.
///
/// `budget` bounds the **cold** seed only, and both of its halves matter: the
/// turn count keeps the fork to recent context, and the char ceiling keeps a
/// cold seed below the child's own compaction threshold, so the side agent does
/// not pay an LLM to summarise the history we just paid to copy. Once a cursor
/// exists the delta is whatever closed since, which is naturally small;
/// clamping the delta too would silently drop the middle of a long-running main
/// session and leave the side agent reading a transcript with a hole in it that
/// nothing announces.
///
/// Not `pub`: [`ForkBudget`] is crate-private, and every surface that reaches a
/// side question is inside this crate.
pub(crate) async fn ensure_seeded(
    session: &dyn SessionService,
    store: &dyn SessionStore,
    main: &SessionId,
    side: &SessionId,
    budget: &ForkBudget,
) -> Result<SeedOutcome, String> {
    // The cursor lives on the side session's row, so the row has to exist
    // before the copy — `patch_session` reports `Ok(false)` for an absent row,
    // and a cursor that is silently never written is the doubling this module
    // prevents. Idempotent, and it stamps attribution from the ambient scope,
    // so callers must reach here with the run's scope already re-established
    // (task-locals do not cross `tokio::spawn`).
    store
        .get_or_create(side)
        .await
        .map_err(|e| format!("btw: open side session: {e}"))?;

    let cursor = read_cursor(store, side).await?;

    let carried = match cursor {
        // Cold: plan against the whole main log, so the char ceiling is
        // honoured and the fork is provenance-marked exactly once.
        None => {
            fork::seed(
                session,
                main,
                side,
                &fork::snapshot(session, main).await?,
                budget,
            )
            .await?
        }
        // Warm: carry whatever closed since. `get_events` is `seq >= from`, so
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
            let plan = fork::plan(&delta, &delta_budget());
            if plan.is_empty() {
                None
            } else {
                // Not `fork::seed`: re-emitting `SessionForked` on every top-up
                // would claim N forks where one happened, and the provenance
                // classification reads that marker.
                fork::seed_events(session, side, &plan.events).await?;
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

    let advanced = plan.read_through.or(cursor);
    if advanced != cursor {
        if let Some(seq) = advanced {
            write_cursor(store, side, seq).await?;
        }
    }
    Ok(SeedOutcome {
        events_added: plan.events.len(),
        cursor: advanced,
    })
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
