//! Forking a sub-agent off the parent's own transcript.
//!
//! # The two ways a sub-agent can start
//!
//! A delegated child either starts **isolated** — nothing but its task, so its
//! judgement is uncontaminated by how the parent framed the work — or it starts
//! **forked**, seeing the parent's real conversation. Those are the two halves
//! of the same axis and they trade against each other exactly:
//!
//! * isolated buys independence, and pays for it in context the child must
//!   rediscover (or that the parent must hand-compress into `context_summary`,
//!   which is lossy, costs parent tokens, and is *the parent's account of its
//!   own work* — the one input an adversarial reviewer must not be given);
//! * forked buys fidelity and prefix warmth, and pays for it in inherited
//!   framing.
//!
//! Until this module existed Aleph could only express the first, and only as a
//! property of the *agent definition* (`AgentDef.context_mode`) rather than of
//! the call — so the choice belonged to whoever wrote the agent file, not to the
//! model that knows what this particular delegation is for.
//!
//! # What "prefix warmth" does and does not mean here
//!
//! Stated precisely, because the imprecise version is the kind of claim that
//! reads true and bills false.
//!
//! A forked child does **not** reuse the *parent's* cache entry. It cannot: the
//! parent assembles its system prompt through [`AssemblyPath::Cached`] (a split
//! stable/dynamic pair) while every sub-agent assembles through
//! [`AssemblyPath::Basic`] (one unsplit string). Different layer sets, different
//! bytes, and a prefix cache that diverges in the system block has already
//! missed everything after it.
//!
//! What a fork warms is **its own lineage**: `(role, forked range)`.
//!
//! * **Within one fan-out** — K forked reviewers spawned from one turn share a
//!   byte-identical `system + messages` prefix. The first writes it, the other
//!   K−1 read it. This is the large win, and it grows with K.
//! * **Across turns** — turn N+1's fork of the same role sees turn N's history
//!   *plus* what the parent has since added. The old part is still a prefix, so
//!   it still hits; only the delta is written.
//!
//! Both properties depend on the copied events being byte-identical replays,
//! which is why this module copies [`SessionEvent`]s verbatim rather than
//! re-rendering them, and why it slices on **whole turns** rather than on an
//! event count that could land mid-turn and shift every later boundary.
//!
//! [`AssemblyPath::Cached`]: crate::thinker::prompt_layer::AssemblyPath::Cached
//! [`AssemblyPath::Basic`]: crate::thinker::prompt_layer::AssemblyPath::Basic

use std::collections::HashSet;

use crate::session::events::{EventSeq, SessionEvent, SessionEventRecord, TurnId};

/// How much of the parent's transcript one fork may carry.
///
/// Both bounds are *derived*, never guessed — see [`ForkBudget::for_child`].
#[derive(Debug, Clone)]
pub(crate) struct ForkBudget {
    /// Complete parent turns to carry, newest-first. `None` = as many as the
    /// char budget allows.
    pub(crate) max_turns: Option<usize>,
    /// Ceiling on the copied transcript, in characters of serialized event.
    pub(crate) max_chars: usize,
}

/// Floor for [`ForkBudget::max_chars`].
///
/// A budget that rounds to zero would make every fork silently degenerate into
/// an isolated spawn — the failure mode where a feature reports success and
/// does nothing. Below this a fork is not worth attempting and the caller is
/// told so instead.
pub(crate) const MIN_FORK_CHARS: usize = 2_000;

impl ForkBudget {
    /// The budget a fork into *this* child may spend.
    ///
    /// Derived rather than picked: the number that matters is "how much can the
    /// child carry before its own context manager starts paying an LLM to
    /// summarise it", and the repo already knows that number —
    /// [`ContextBudgetConfig::warning_threshold`] is the fill ratio at which
    /// compaction triggers, `token_budget` is the window, and
    /// `token_estimate_ratio` converts tokens to characters. Seeding a child
    /// *above* its own warning line means its very first Think compacts away
    /// the history we just paid to copy — cost with none of the fidelity.
    ///
    /// `system_prompt_chars` is subtracted because the child's system block
    /// occupies the same window and is known exactly by the time a fork is
    /// planned (the spawner builds it first for this reason).
    ///
    /// `None` when there is no `[context_budget]` config — with no window to
    /// reason about, any ceiling would be the invented constant this function
    /// exists to avoid, and the honest answer is "I cannot size this", which
    /// the caller surfaces as a refusal rather than an unbounded copy.
    pub(crate) fn for_child(
        cfg: Option<&crate::context::budget::ContextBudgetConfig>,
        system_prompt_chars: usize,
        max_turns: Option<usize>,
    ) -> Option<Self> {
        let cfg = cfg?;
        let window_chars = (cfg.token_budget as f64)
            * cfg.warning_threshold.clamp(0.0, 1.0)
            * cfg.token_estimate_ratio.max(1.0);
        // `as usize` after the clamp: the product is a positive finite f64 for
        // any config that passed validation, and saturating at 0 is the same
        // answer the `< MIN_FORK_CHARS` check below gives.
        let usable = (window_chars as usize).saturating_sub(system_prompt_chars);
        (usable >= MIN_FORK_CHARS).then_some(Self {
            max_turns,
            max_chars: usable,
        })
    }
}

/// One parent turn's worth of prompt-bearing events, kept together.
///
/// Turns are the slicing unit because a tool call and its result carry the same
/// `turn_id`, and [`crate::harness::agent::prompt`] resolves that pairing
/// *within* a turn on purpose (`result_call_id_of_turn`). Slicing anywhere else
/// can orphan one half of a pair; slicing here cannot.
struct TurnGroup {
    turn: Option<TurnId>,
    events: Vec<SessionEvent>,
    /// `call_id`s requested in this turn with no result yet.
    open_calls: HashSet<String>,
    /// Source seq of the newest event in this group. Groups drop the record
    /// wrapper, so this is the only place the source ordinal survives.
    last_seq: EventSeq,
}

/// What a fork will actually copy, plus what it had to leave behind.
#[derive(Debug, Default)]
pub(crate) struct ForkPlan {
    /// Verbatim parent events, oldest-first, ready to emit into the child.
    pub(crate) events: Vec<SessionEvent>,
    /// Complete turns carried.
    pub(crate) turns_copied: usize,
    /// Complete turns that were available to carry.
    pub(crate) turns_available: usize,
    /// Serialized size of [`Self::events`].
    pub(crate) chars: usize,
    /// Source seq of the newest event this plan consumed, or `None` when it
    /// carried nothing.
    ///
    /// This is **how far into the source the plan read**, which is not the same
    /// question as what it kept, and the difference cuts both ways:
    ///
    /// * turns dropped off the *front* for budget are older than this, so a
    ///   caller resuming from here will not re-read them — which is what makes
    ///   an incremental caller append-only rather than doubling;
    /// * the trailing *open* turn is deliberately left behind and sits above
    ///   this, so it is re-read once it closes and its answer is not lost.
    ///
    /// Only a caller that resumes from the same log needs it; the sub-agent
    /// spawner forks once and ignores it.
    pub(crate) read_through: Option<EventSeq>,
}

impl ForkPlan {
    pub(crate) fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// The one line a forked child is told about its own fork.
    ///
    /// Always emitted, not only on truncation. Two reasons, and the second is
    /// the one that decided it:
    ///
    /// 1. A child that believes it can see the whole conversation, while the
    ///    opening turns — the ones that say what the work is *for* — were
    ///    dropped off the front, will answer the wrong question with complete
    ///    confidence. Truncation must never be silent.
    /// 2. "Did my fork actually land?" is otherwise unanswerable from either
    ///    end. The parent gets a result with no indication of what the child
    ///    started from, and a fork that quietly degraded (budget too small,
    ///    parent barely started) is indistinguishable from one that worked. A
    ///    receipt costs ~100 bytes against a fork that carried tens of
    ///    thousands, and it is a runtime fact neither party can derive.
    ///
    /// `None` only when nothing was carried, where the caller has already
    /// decided what to do about it.
    pub(crate) fn receipt(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let dropped = self.turns_available.saturating_sub(self.turns_copied);
        let head = format!(
            "[fork] The conversation above is a verbatim copy of the parent's most recent \
             {copied} of {available} completed turns ({chars} chars).",
            copied = self.turns_copied,
            available = self.turns_available,
            chars = self.chars,
        );
        Some(if dropped == 0 {
            head
        } else {
            format!(
                "{head} The {dropped} oldest turns were NOT carried over — if the original \
                 objective is not visible above, ask for it rather than inferring it."
            )
        })
    }
}

/// Does this event become part of the LLM prompt?
///
/// Mirrors the arms [`crate::harness::agent::prompt::build_prompt_with_transient_tail`]
/// matches on. Deliberately **exhaustive with no wildcard**: a new
/// [`SessionEvent`] variant must be classified here or the crate does not
/// compile, which is the only form of "remember to update the other list" that
/// does not eventually get forgotten.
///
/// The compiler covers *added* variants; `fork_kinds_track_the_prompt_builder`
/// covers the other direction (an existing variant becoming prompt-bearing over
/// in `prompt.rs` without anyone thinking about forks).
pub(crate) fn is_prompt_bearing(event: &SessionEvent) -> bool {
    match event {
        SessionEvent::UserMessage { .. }
        | SessionEvent::AssistantMessage { .. }
        | SessionEvent::SystemMessage { .. }
        | SessionEvent::ToolCallRequested { .. }
        | SessionEvent::ToolResult { .. }
        | SessionEvent::ToolError { .. } => true,
        // Only the `Cancelled` outcome renders (as the interruption note);
        // `Completed` is the ordinary seam between turns and `Errored` already
        // left its trail in the visible tool/assistant events.
        SessionEvent::RunFinished { outcome, .. } => {
            matches!(outcome, crate::session::events::RunOutcome::Cancelled)
        }
        // Bookkeeping, lifecycle, telemetry and provenance — none of it reaches
        // the model, and a fork that replayed it would be seeding the child
        // with the parent's *plumbing* rather than the parent's conversation.
        SessionEvent::SessionWoken { .. }
        | SessionEvent::RunStarted { .. }
        | SessionEvent::TurnStarted { .. }
        | SessionEvent::AssistantRunMeta { .. }
        | SessionEvent::ToolCallApproved { .. }
        | SessionEvent::ToolCallDenied { .. }
        | SessionEvent::SubagentSpawned { .. }
        | SessionEvent::SubagentReturned { .. }
        | SessionEvent::CompactionPerformed { .. }
        | SessionEvent::SessionForked { .. }
        | SessionEvent::Error { .. } => false,
    }
}

/// Plan a fork from the parent's event log.
///
/// Pure — it neither reads nor writes a session — so the whole selection policy
/// is unit-testable without a store, which is what makes the boundary rules
/// below assertable rather than aspirational.
pub(crate) fn plan(records: &[SessionEventRecord], budget: &ForkBudget) -> ForkPlan {
    let groups = group_into_turns(records);

    // Drop trailing *open* turns. The parent is mid-turn right now — it is
    // inside the very `subagent` call being served, so its own
    // `ToolCallRequested` for this spawn is sitting in the log with no result.
    // Carrying it would seed the child with "I asked someone to do this" as the
    // last thing it sees, and `build_prompt` would then drop that half-turn as
    // an orphan anyway. Cutting at the last *closed* turn is both the honest
    // boundary and the stable one: it does not move when the parent's in-flight
    // turn finally lands, so consecutive forks share a prefix.
    let closed_len = groups
        .iter()
        .rposition(|g| g.open_calls.is_empty())
        .map_or(0, |i| i + 1);
    let closed = &groups[..closed_len];
    let turns_available = closed.len();

    // Walk newest-first, taking whole turns while both bounds hold.
    let mut taken: Vec<&TurnGroup> = Vec::new();
    let mut chars = 0usize;
    for group in closed.iter().rev() {
        if budget.max_turns.is_some_and(|max| taken.len() >= max) {
            break;
        }
        let size: usize = group
            .events
            .iter()
            .map(|e| serde_json::to_string(e).map_or(0, |s| s.len()))
            .sum();
        // Always take at least one turn if it fits at all; an over-budget
        // *first* turn is the one case where taking nothing and taking
        // something are equally wrong, and "nothing" is the one that looks
        // like the feature is broken.
        if !taken.is_empty() && chars + size > budget.max_chars {
            break;
        }
        if taken.is_empty() && size > budget.max_chars {
            break;
        }
        chars += size;
        taken.push(group);
    }
    taken.reverse();

    let mut events: Vec<SessionEvent> = taken
        .iter()
        .flat_map(|g| g.events.iter().cloned())
        .collect();

    // Head snap, mirroring `session_split`'s: never *begin* a copied window
    // with a `ToolResult` / `ToolError` whose `tool_use` was left behind.
    // Whole-turn slicing should already prevent it (a pair shares a turn id),
    // but a parent log written across a schema change, or a synthetic closure
    // stamped with a foreign turn id, would land exactly here — and the
    // failure is an HTTP 400 on the child's first call, not a warning.
    let head = events
        .iter()
        .position(|e| {
            !matches!(
                e,
                SessionEvent::ToolResult { .. } | SessionEvent::ToolError { .. }
            )
        })
        .unwrap_or(events.len());
    events.drain(..head);

    // Taken newest-first then reversed, so the last group is the newest turn
    // consumed. Gated on `events`: after the head snap a non-empty `taken` can
    // still carry nothing, and a plan that carried nothing has not read
    // anything either.
    let read_through = (!events.is_empty())
        .then(|| taken.last().map(|g| g.last_seq))
        .flatten();

    ForkPlan {
        turns_copied: taken.len(),
        turns_available,
        chars,
        events,
        read_through,
    }
}

/// The parent transcript a fork is taken from, captured **once** per tool call.
///
/// Shared rather than re-read per child, and that is load-bearing rather than a
/// micro-optimisation. Two things break if each child reads the log itself:
///
/// 1. **Background children read late.** A background spawn detaches and runs
///    after the tool call has returned, so by the time it read the log the
///    parent would have moved on — the child would be forked from a *future*
///    of the conversation that did not exist when the delegation was made.
///    "Fork" means fork here, not fork whenever the task happens to start.
/// 2. **A fan-out would stop sharing a prefix.** K children each reading at
///    their own instant get K different transcripts, so K different prompt
///    prefixes, so K full-price cache writes instead of one write and K−1
///    reads — deleting the exact saving the mode exists for, silently, on the
///    bill only.
///
/// Planning still happens per child (each has its own system-prompt size and so
/// its own ceiling); it is the *source* that is pinned.
pub(crate) type ForkSource = std::sync::Arc<Vec<SessionEventRecord>>;

/// Capture the parent's log for [`ForkSource`].
pub(crate) async fn snapshot(
    session: &dyn crate::session::service::SessionService,
    parent: &crate::session::service::SessionId,
) -> Result<ForkSource, String> {
    session
        .get_events(parent, None, None)
        .await
        .map(std::sync::Arc::new)
        .map_err(|e| format!("sub-agent failed: fork: read parent log: {e}"))
}

/// Plan a fork from `source` and seed the child session with it.
///
/// Returns the plan that was actually applied. `Ok(None)` means "no fork
/// happened" for a reason that is not an error — nothing closed to carry yet
/// (the parent's very first turn), or a budget too small for even one turn. A
/// fork that cannot be *sized at all* is an error raised by the caller, not an
/// empty fork: silently degrading to an isolated spawn would hand back a
/// confident answer produced under conditions the caller did not ask for and
/// cannot see.
///
/// The `SessionForked` provenance marker mirrors `context::compact::
/// session_split`, which already establishes both the marker and the
/// verbatim-`emit_event` copy as the way a child session inherits a parent's
/// history here. Reusing it rather than inventing a second shape means the
/// existing readers of that marker (`store.rs` classification, anything that
/// asks "where did this session come from") light up for free.
pub(crate) async fn seed(
    session: &dyn crate::session::service::SessionService,
    parent: &crate::session::service::SessionId,
    child: &crate::session::service::SessionId,
    source: &[SessionEventRecord],
    budget: &ForkBudget,
) -> Result<Option<ForkPlan>, String> {
    let plan = plan(source, budget);
    if plan.is_empty() {
        return Ok(None);
    }

    session
        .emit_event(
            child,
            SessionEvent::SessionForked {
                parent_session_id: parent.to_key_string(),
                at: crate::session::events::now_ms(),
            },
        )
        .await
        .map_err(|e| format!("sub-agent failed: fork: emit SessionForked: {e}"))?;

    seed_events(session, child, &plan.events).await?;

    Ok(Some(plan))
}

/// Copy planned events into `child` verbatim, without provenance marking.
///
/// The copy half of [`seed`], split out for callers that top a child up
/// *incrementally* — a second fork of the same pair is one fork that grew, not
/// two forks, so `SessionForked` must not be re-emitted for it. Marking stays
/// in [`seed`], which is the only place a fork *begins*.
///
/// Verbatim `emit_event` rather than re-rendering is the whole point: prefix
/// warmth requires the copied bytes to be byte-identical replays, so anything
/// that reshapes an event here would silently delete the saving the fork modes
/// exist for.
pub(crate) async fn seed_events(
    session: &dyn crate::session::service::SessionService,
    child: &crate::session::service::SessionId,
    events: &[SessionEvent],
) -> Result<(), String> {
    for event in events {
        session
            .emit_event(child, event.clone())
            .await
            .map_err(|e| format!("sub-agent failed: fork: copy event: {e}"))?;
    }
    Ok(())
}

/// Bucket prompt-bearing events into consecutive same-`turn_id` runs.
fn group_into_turns(records: &[SessionEventRecord]) -> Vec<TurnGroup> {
    let mut groups: Vec<TurnGroup> = Vec::new();
    for record in records {
        if !is_prompt_bearing(&record.event) {
            continue;
        }
        let turn = turn_of(&record.event);
        // A turn-less event (the `Cancelled` interruption marker) belongs with
        // whatever it interrupted, not to a bucket of its own.
        let start_new = match (&turn, groups.last()) {
            (Some(t), Some(last)) => last.turn != Some(*t),
            (Some(_), None) | (None, None) => true,
            (None, Some(_)) => false,
        };
        if start_new {
            groups.push(TurnGroup {
                turn,
                events: Vec::new(),
                open_calls: HashSet::new(),
                last_seq: record.seq,
            });
        }
        let group = groups
            .last_mut()
            .expect("a group exists: pushed above when absent");
        match &record.event {
            SessionEvent::ToolCallRequested { call_id, .. } => {
                group.open_calls.insert(call_id.clone());
            }
            SessionEvent::ToolResult { call_id, .. } | SessionEvent::ToolError { call_id, .. } => {
                group.open_calls.remove(call_id);
            }
            _ => {}
        }
        group.last_seq = record.seq;
        group.events.push(record.event.clone());
    }
    groups
}

fn turn_of(event: &SessionEvent) -> Option<TurnId> {
    match event {
        SessionEvent::UserMessage { turn_id, .. }
        | SessionEvent::AssistantMessage { turn_id, .. }
        | SessionEvent::SystemMessage { turn_id, .. }
        | SessionEvent::ToolCallRequested { turn_id, .. }
        | SessionEvent::ToolResult { turn_id, .. }
        | SessionEvent::ToolError { turn_id, .. } => Some(*turn_id),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
