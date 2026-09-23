//! Agent loop execution and streaming callback.
//!
//! Contains `run_agent_loop` (the think-act two-step loop).

mod author_census;
mod flow_scope_census;
mod inner;
mod project_context;
#[cfg(test)]
mod tests;

// Re-export the project-context helpers at the historical `run_loop::` path so
// any internal consumer keeps resolving the same items.
pub(crate) use project_context::lifecycle_hook_context;

use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::sync_primitives::Arc;

use super::{ExecutionError, RunRequest};
use crate::extension::HookEvent;
use crate::gateway::agent_instance::AgentInstance;
use crate::gateway::event_emitter::EventEmitter;
use crate::projects::binding::ClaimSource;
use crate::session::events::{now_ms, ErrorKind, MessageContent, RunOutcome, SessionEvent, TurnId};

use crate::executor::ToolRegistry;
use crate::thinker::ProviderRegistry as ThinkerProviderRegistry;

use super::engine::ExecutionEngine;

// ============================================================================
// Agent loop execution
// ============================================================================

/// This run's scope attribution: what the producer stamped, corrected by what
/// the gateway itself knows about the session key.
///
/// [`crate::scope::scope_from_metadata`] reads a map some producer wrote.
/// `projects.current_session_key` is written by exactly one function
/// ([`crate::projects::ProjectStore::claim_session_key`]), so a key it names is
/// a room **by declaration** rather than by inference — and when the two
/// disagree, the declaration is the one that knows.
///
/// The disagreement is not hypothetical and does not heal. A room is opened
/// (`projects.room_session` claims the key) before anyone speaks, and whoever
/// speaks first creates the row. A producer that never heard of rooms stamps
/// that row `personal:<first speaker>`; `stamp_attribution` is create-only and
/// `attribution_backfill`'s predicate is `owner_user_id IS NULL AND scope_id IS
/// NULL`, so the wrong stamp is permanent and the room goes invisible to every
/// other member — including its owner — while `projects.list` keeps listing it.
///
/// `handlers::agent::resolve_attribution` already asks this question for ONE
/// producer, the Panel's `agent.run` / `chat.send`, and keeps asking it there
/// because it can also *refuse*: for an arm-1 room a non-member gets
/// `ProjectNotFound`, the same refusal a named foreign project gets. This
/// function cannot refuse — it runs after admission, on a request that is
/// already going to execute — so it only corrects the filing. The six
/// producers that never pass through that handler (the channel inbound router,
/// cron, heartbeat, the teams dispatcher, `session_send`, A2A) get the
/// correction here.
///
/// Only the scope is replaced. `owner_user_id` still names whoever spoke: for a
/// project-scoped row visibility is decided by the roster
/// ([`crate::gateway::visibility::owner_and_scope_visible_to`]), so overwriting
/// the owner would buy nothing and lose the attribution.
///
/// A catalogue failure reads as "not a room" — a degraded SQLite must not turn
/// into a mis-scoped turn *or* a refused one. That ruling is not made here: it
/// is made once, in [`crate::projects::ProjectStore::room_claiming`], because
/// this path and the admission path had always made it identically and a
/// ruling two callers share must not be written twice.
///
/// The upgrade — replacing a producer's own stamp with the room's — passes a
/// roster gate first, but **only for a room discovered through arm 2** (a
/// bound channel conversation). Arm 1 (an explicit `projects.room_session`
/// claim) is a declaration, not an inference — a room is opened by an
/// operator or the Panel deliberately naming this session key as the room's
/// conversation — so it outranks the producer's stamp unconditionally, same
/// as before this gate existed. That is load-bearing for this very path: the
/// six producers here include cron/A2A re-opening a room's session, whose
/// stamped `owner_user_id` is legitimately the legacy owner and is never on
/// any roster, and the channel inbound router continuing a claimed
/// conversation, whose stamped owner may be a member nobody re-adds on every
/// turn. Gating arm 1 would silently demote those runs to personal.
///
/// Arm 2 has no equivalent declaration: a channel conversation is bound by an
/// operator, but *being in that conversation* is not — anyone in the
/// Telegram group could otherwise ride the binding into the room's scope.
/// The stamp's own `owner_user_id` is used as the actor because there is no
/// ambient caller here, unlike the admission path. A caller the roster does
/// not admit keeps its producer's own stamp rather than being silently
/// dropped into the room: being in the channel conversation must not be
/// equivalent to being on the roster, and this is the only place downstream
/// of the channel inbound router that ever asks.
///
/// The admission twin reaches the same verdict for arm 2 by a different route
/// — it falls through to its personal arm — and that agreement is deliberate,
/// because the two are reachable by the *same* principal in the *same*
/// conversation through two different doors: through the channel, which lands
/// here, or with the same channel-shaped session key on `agent.run` /
/// `chat.send`, which lands there. `handlers::agent::resolve_attribution`'s
/// `None` arm carries the other half of this argument, and
/// `the_two_room_claim_twins_agree_on_which_project_governs` pins the pair.
pub(super) fn request_scope(request: &RunRequest) -> Option<crate::scope::ScopeAttribution> {
    scope_for_session(&request.metadata, &request.session_key)
}

/// [`request_scope`] with the session named explicitly.
///
/// The room lookup keys on a session, and for one caller that session is **not**
/// `request.session_key`: `/btw promote` runs with the key already redirected
/// onto the side thread, then creates the row for `main`. Correcting with the
/// request's own key there would ask whether the *side thread* is a room — it
/// never is — and file `main`'s row under the raw producer stamp, permanently
/// (`stamp_attribution` is create-only and `attribution_backfill` only fills
/// NULLs). Separating the parameter is what lets that caller ask the question
/// about the session it is actually creating.
///
/// Takes the metadata map rather than an already-parsed attribution so that no
/// caller has to touch `scope_from_metadata` itself — a raw read is exactly what
/// `no_reader_under_execution_engine_takes_the_uncorrected_scope_stamp` forbids,
/// and an entry point that requires one would have to exempt every caller from
/// its own guard.
pub(super) fn scope_for_session(
    metadata: &std::collections::HashMap<String, String>,
    session_key: &crate::routing::session_key::SessionKey,
) -> Option<crate::scope::ScopeAttribution> {
    let stamped = crate::scope::scope_from_metadata(metadata);
    let Some((pid, source)) = crate::projects::ProjectStore::shared().room_claiming(session_key)
    else {
        return stamped;
    };
    let mut attr = stamped?;
    let target = crate::scope::ScopeId::Project(pid.clone());
    if attr.scope == target {
        return Some(attr);
    }
    if source == ClaimSource::BoundConversation
        && !crate::gateway::visibility::project_visible_to(&pid, Some(&attr.owner_user_id))
    {
        return Some(attr);
    }
    attr.scope = target;
    Some(attr)
}

/// Whether this turn is happening inside a project room.
///
/// The shared-room busy-lane rule ([`super::BusyInputMode::for_shared_room`])
/// needs this and used to answer it by parsing `SCOPE_META_KEY` out of the
/// incoming metadata — i.e. from the producer's raw stamp, which for the six
/// producers that need [`request_scope`]'s correction says `personal:<speaker>`
/// about a session a room has already claimed. The rule then read "not a room"
/// and let one member's message steer another member's in-flight run: the exact
/// thing it exists to forbid, silently, and only on the channel/cron/A2A paths.
///
/// The room half is asked of [`crate::projects::ProjectStore::room_claiming`]
/// directly rather than read off [`request_scope`]'s verdict, because
/// `request_scope` applies arm 2's roster gate and that gate answers a
/// *different* question: whether to hand an off-roster speaker the room's DATA
/// scope. Here an off-roster speaker steering a member's in-flight turn is
/// WORSE, not better, so the predicate is "is this session claimed by a room",
/// full stop. `request_scope` is still ORed in for the other direction — a
/// project-scoped session no room has claimed keeps the protection it has
/// today — so this can only add cases, never remove one.
pub(super) fn request_is_in_a_room(request: &RunRequest) -> bool {
    if crate::projects::ProjectStore::shared()
        .room_claiming(&request.session_key)
        .is_some()
    {
        return true;
    }
    matches!(
        request_scope(request).map(|a| a.scope),
        Some(crate::scope::ScopeId::Project(_))
    )
}

/// The two strings [`crate::orchestrator::FlowRequest`] carries for this run's
/// scope attribution — derived from [`request_scope`], never read back out of
/// `request.metadata`.
///
/// This is the FOURTH reader of `request_scope` (`src/gateway/CLAUDE.md` 地雷 Q
/// names the other three: the session row, the loop's task-local, the sidebar
/// recency touch), and it is the boundary where the room upgrade used to be
/// lost. The raw keys hold whatever the PRODUCER stamped — for a channel turn
/// that is `personal:<speaker>` — and `request_scope` is the only thing that
/// turns that into the room's scope when the conversation is bound. Reading
/// the keys directly here handed the un-upgraded pair to
/// `orchestrator::dispatch`, which re-seeds the scope task-local inside its
/// `tokio::spawn`, so the session row was filed under the room while
/// everything downstream of the spawn — the memory partition, the
/// `<room_context>` roster (`harness_bridge::prompt_build` reads this very
/// task-local and its comment claims it equals what `request_scope`
/// resolved), and the transcript's speaker attribution — ran personal.
///
/// `FlowRequest` carries strings rather than a `ScopeAttribution`, because it
/// has no metadata map. That is a reason to CONVERT here, not a reason to read
/// a different source: `ScopeId::render` is the same call
/// [`crate::scope::stamp_metadata`] makes, so `dispatch`'s rebuild — a map of
/// these two keys fed back through [`crate::scope::scope_from_metadata`] —
/// parses back exactly the attribution this returned, including its
/// fail-closed `None`.
///
/// The two strings ride inside [`crate::scope::FlowScope`], whose fields are
/// private and whose only non-empty constructor takes a `ScopeAttribution`.
/// That makes ONE spelling of the raw read a compile error at the
/// `FlowRequest` site: a pair lifted out of `request.metadata` is
/// `(Option<String>, Option<String>)` and does not fit the field. It does not
/// make every spelling one. `ScopeAttribution::from_persisted` takes exactly
/// that pair and yields the type this constructor accepts, so metadata still
/// reaches the site in one public call — measured, compiling, with every
/// lexical layer green. See that type's doc; the bound is stated in full in
/// `flow_scope_census`'s module doc.
///
/// A named function rather than two inline expressions: the property that must
/// NOT change is that an off-roster speaker in a bound conversation is
/// projected to the same pair the raw read produced, and a test that
/// re-derived the projection to check that would be measuring its own copy.
/// `flow_scope_census` keeps the site honest lexically;
/// `tests::the_flow_request_projection_carries_the_room_upgrade` and
/// `tests::the_projection_round_trips_through_the_dispatch_rebuild` keep this
/// function honest behaviourally: they are the only two that go red when a
/// re-resolution here LOSES the room upgrade, whatever spelling it uses. A
/// re-resolution that KEEPS it is neither theirs nor layer 3's counts: those
/// count OCCURRENCES, not answers, and a fork that agrees adds none of them.
/// One thing objects — `flow_scope_census`'s layer 5, the requirement that
/// this body CALL `request_scope` — and a fork that leaves a dead call
/// standing beside its own resolution still passes even that. Both halves are
/// measured in
/// `flow_scope_census::tests::the_projection_body_must_call_request_scope`.
fn request_scope_strings(request: &RunRequest) -> crate::scope::FlowScope {
    crate::scope::FlowScope::resolved(request_scope(request).as_ref())
}

/// Establishes this run's scope attribution (owner/scope) and this turn's
/// speaker as task-locals for `fut`'s duration, both derived from
/// `request.metadata` — see [`crate::scope::stamp_metadata`] and
/// [`super::AUTHOR_USER_KEY`]. `author_census::ORIGIN_SITES` (this module's
/// `author_census` submodule) names the origin writers of that key the census
/// pins by name. It is not every writer: later origins are pinned by unit
/// tests at their own sites (the census's module doc says which), and the
/// continuation paths forward or rehydrate the key rather than originate it.
/// The list lives there rather than here so this doc cannot drift out of
/// sync with it the way it once did — a prior revision named
/// only two producers (`build_run_request`, the channel inbound router's
/// `execute_for_context_inner`) while two more (the team broadcast and
/// dispatcher child-run builders) had already existed for over a week.
///
/// The scope and author travel together on purpose. The scope names the ROOM, the author
/// names whoever is typing, and in a project room those genuinely differ; the
/// main path's user-message writer (`harness_bridge::session_seed`) reaches
/// neither the request nor `CALLER_USER`, so seeding both here is what keeps
/// the transcript label and the memory partition talking about the same turn.
///
/// Extracted from [`ExecutionEngine::run_agent_loop`]'s wrapping nest for
/// testability: unlike the other layers in that nest (agent id, project
/// root, fs scope), this one depends on nothing but the metadata map, so it
/// can be driven directly with a minimal `RunRequest` and a probe future.
pub(super) async fn with_request_scope<F, T>(request: &RunRequest, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    let author = request.metadata.get(super::AUTHOR_USER_KEY).cloned();
    crate::scope::with_scope(
        request_scope(request),
        crate::scope::with_room_author(author, fut),
    )
    .await
}

/// The run-admission spend arm: deny before this run claims any resource if
/// its principal ([`crate::spend::principal_from_metadata`], resolved off
/// `request.metadata` the same way [`with_request_scope`] resolves scope —
/// see that resolver's doc for why it is unconditionally equivalent to the
/// floor arm's `ambient_principal`) is over its ceiling for the period.
///
/// Both engines call this — `ExecutionEngine::execute` (`execute.rs`, ahead
/// of `admit_run`) and `SimpleExecutionEngine::execute` (`simple.rs`, which
/// has no `admit_run` to gate alongside and so calls this as its own first
/// act) — as the very first thing they do, before either claims a session
/// run slot, a concurrency permit, or (`SimpleExecutionEngine`) transitions
/// the agent to `Running`. A principal already over the line should never be
/// handed a resource it is about to be denied anyway, and a shared call site
/// is what keeps `SimpleExecutionEngine`, which has no `admit_run` to
/// piggyback on, from silently skipping a floor the full engine enforces —
/// see this module's doc and the plan this task belongs to for why "a floor
/// only one engine honours is not a floor".
///
/// One helper rather than each engine open-coding the
/// `spend::principal_from_metadata` / `spend::check` pairing itself: a
/// second, hand-written copy of that pairing is exactly the kind of drift
/// [`crate::spend::check`]'s own doc warns against.
pub(super) fn deny_if_over_spend(request: &RunRequest) -> Result<(), ExecutionError> {
    let principal = crate::spend::principal_from_metadata(&request.metadata);
    let now_ms = chrono::Utc::now().timestamp_millis();
    admission_result_for(crate::spend::check(&principal, now_ms))
}

/// [`deny_if_over_spend`], plus the one thing a bare `?` on it cannot do: put
/// a `RunError` on the wire when it denies.
///
/// This fires *before* `RunAccepted` — the run has not yet claimed a slot,
/// so nothing downstream will ever emit a terminal frame for it. Every other
/// `Err` an engine's `execute()` can produce is caught by the think/act
/// loop's own error arm, which renders `ExecutionError::user_receipt` onto
/// the wire before returning — see `execute.rs`'s `Err(e) => { .. }` tail.
/// The admission arm runs ahead of that whole apparatus, so it is on its own
/// for upholding the same contract, the one
/// `busy_queue::spawn_queued_run`/`deliver_with_ticket` already assume every
/// `execute()` error keeps: "the engine already emits `RunError` for
/// anything that fails inside `execute`". Skipping this and returning the
/// bare `Err` — which is what both engines did before this existed — breaks
/// that contract silently: `chat.send`/`agent.run` still returns a `run_id`,
/// but the run never reaches `RunAccepted` OR `RunError`, so every observer
/// (Panel spinner, CLI, channel reply) waits on a run that will never answer
/// and only `spend_ledger` and a `tracing::error!` line know why (see
/// task-12's real-machine fixture, assertion 4).
///
/// `session_key` is stamped explicitly on the frame — the same reason
/// `spawn_queued_run`'s own never-ran producer does, on the same never-ran
/// case: with no `RunAccepted` to have seeded it, `EventVisibilityIndex`'s
/// run→session index has nothing to resolve `ByRunId` against, so the frame
/// must carry its own addressing or the delivery filter drops it before any
/// client sees it.
pub(super) async fn deny_if_over_spend_and_report<E: EventEmitter + Send + Sync>(
    request: &RunRequest,
    emitter: &E,
) -> Result<(), ExecutionError> {
    report_admission_denial(deny_if_over_spend(request), request, emitter).await
}

/// The reporting half of [`deny_if_over_spend_and_report`], with the
/// admission result taken as a plain parameter instead of computed here —
/// the same hazard-free split [`admission_result_for`] exists for: a test
/// can drive this with a hand-built `Err(ExecutionError::SpendExhausted {
/// .. })` without installing a low ceiling into the process-wide
/// policy/ledger `OnceLock`s the rest of this crate's tests already share
/// and race.
async fn report_admission_denial<E: EventEmitter + Send + Sync>(
    result: Result<(), ExecutionError>,
    request: &RunRequest,
    emitter: &E,
) -> Result<(), ExecutionError> {
    if let Err(e) = result {
        let (error_code, error_message) = e.user_receipt(
            crate::gateway::i18n::Locale::from_run_metadata(&request.metadata),
        );
        let seq = emitter.next_seq();
        if let Err(emit_err) = emitter
            .emit(crate::gateway::event_emitter::StreamEvent::RunError {
                run_id: request.run_id.clone(),
                seq,
                error: error_message,
                error_code: Some(error_code.to_string()),
                session_key: Some(request.session_key.to_key_string()),
            })
            .await
        {
            tracing::warn!(
                run_id = %request.run_id,
                error = %emit_err,
                "failed to emit RunError stream event for a spend-denied admission",
            );
        }
        return Err(e);
    }
    Ok(())
}

/// The translation [`deny_if_over_spend`] applies to whatever
/// [`crate::spend::check`] returns — split out so it is testable without
/// touching the process-global ledger/policy `check` reads. `cargo test
/// --lib` runs every test in this crate in one binary, and
/// `providers::metering`'s tests already install a real (if generously
/// high) process-wide policy for their own wiring tests; a second test here
/// racing `spend::check`'s global read would either see that policy or the
/// pre-install default depending on execution order. Taking the `Verdict`
/// as a plain parameter sidesteps the hazard entirely — same reasoning as
/// `spend::check_with`'s own doc.
fn admission_result_for(verdict: crate::spend::Verdict) -> Result<(), ExecutionError> {
    match verdict {
        crate::spend::Verdict::Allowed(_) => Ok(()),
        crate::spend::Verdict::Denied { limit, spent } => {
            // `spent.period_end_ms` is `None` only out of a raw
            // `SpendLedger` read (see `Spent::period_end_ms`'s doc) — this
            // `spent` came from `spend::check`/`check_with`, which always
            // fills it in before returning `Denied` (the only early-return
            // that skips filling it is `Verdict::Allowed`, taken while the
            // policy is disabled). A `None` here means an earlier layer
            // broke that guarantee; recomputing a plausible-looking instant
            // would hide exactly the drift this field exists to prevent, so
            // this is `expect`, not a fallback.
            let reset_ms = spent
                .period_end_ms
                .expect("spend::check always fills period_end_ms before returning Verdict::Denied");
            Err(ExecutionError::SpendExhausted { limit, reset_ms })
        }
    }
}

/// Create this run's session row **under the run's own attribution**.
///
/// `SessionMetadata::stamp_attribution` reads `scope::current_scope()` on the
/// CREATE branch of `SessionStore::get_or_create`, and that task-local does not
/// survive `tokio::spawn`. Every producer of a run — the Panel handler
/// (`handlers::agent`), the channel inbound router, cron, heartbeat, the teams
/// dispatcher, `sessions_send`, A2A — hands the request to a *spawned* task, so
/// by the time the engine creates the row the ambient scope is `None`.
///
/// ⚠️ **"the attribution is sitting right there in `request.metadata`" is a
/// claim about each producer, not a property of this helper.** This helper can
/// only read what a producer wrote, and until 2026-08-09 the teams fan-out
/// wrote neither key — `member_run_metadata` inserted `team_id` / `chain_depth`
/// / `platform` / run-mode and stopped — so for every member run this helper
/// was a no-op while a reader of this sentence would believe it was covered.
/// The census that matters is `scope_stamping_producers_are_all_accounted_for`
/// in this module's tests, not this paragraph. The row
/// then persists with `owner_user_id`/`scope_id` NULL and is adopted as
/// owner-owned, which for a member means their own session is invisible to them
/// (`sessions.list` empty, `sessions.set_topic` "not found",
/// `chat.context_estimate` null) and their transcript is attributed to the
/// operator — including to `handlers::trace`'s cross-user read audit, which
/// compares against `effective_owner` and therefore never fires for it.
///
/// Reading the metadata rather than capturing the caller's task-local is
/// deliberate and load-bearing: `current_scope()` is **also** `None` in the
/// gateway dispatch loop (which scopes `CALLER_USER`/`CALLER_ROLE`, not the
/// attribution), so the resolved attribution exists ONLY in the metadata map.
/// Using the same accessor [`with_request_scope`] uses is what keeps the row
/// and the loop from disagreeing about whose turn this is.
///
/// One helper rather than the same three lines at both engines' call sites:
/// `ExecutionEngine::execute` and `SimpleExecutionEngine::execute` each create
/// the row, and a second copy is a second answer waiting to drift.
pub(super) async fn ensure_session_under_request_scope(
    agent: &AgentInstance,
    request: &RunRequest,
) {
    crate::scope::with_scope(
        request_scope(request),
        agent.ensure_session(&request.session_key),
    )
    .await;
}

/// §5.4: what a pre-seed hook stop leaves on the log — a **closed** run with
/// a receipt, so a reload shows "this turn was stopped by a hook" and the
/// reducer reads `Clean` rather than `Unanswered` (which would retrigger a
/// run the hook already refused). Its writers are the pre-seed hook seams —
/// the ones that return before the harness is handed the request; the set is
/// derived by `hook_stop_tests::every_pre_seed_hook_exit_journals_the_stop`,
/// not listed here.
///
/// The seed pair is written here because the bridge's `seed_history`
/// (`runner_impl.rs`) never runs on this path — **text only**: the receipt
/// writes `blocks: Vec::new()`, so a stopped attachment turn reloads without
/// its images (both seams fire before media is resolved, and the seed the
/// bridge would have written is the one carrying `media_blocks`). A resume
/// (`is_resume`) already holds its user message in the log and gets only the
/// run bracket. With no
/// turn opened on that arm the receipt names `turn_id: None` — a turn id that
/// points at no `TurnStarted` would be a specific lie. `RunStarted.envelope`
/// is `None` for the same reason: this writer resolved no knobs, and the run
/// is closed in the same batch so nothing ever replays it. `Error{HookStop}`
/// reuses the guardrail receipt's projection row
/// (`session::projection::project_row`) and is NOT prompt-bearing.
///
/// One `now_ms()` for the whole batch: it is one moment, and per-row calls
/// can straddle a millisecond. The batch's durability is derived from its
/// members by `events::durability_of` (U3) — `RunStarted` makes it a barrier
/// on both arms — never chosen here.
pub(super) fn hook_stop_receipt(
    request: &RunRequest,
    outcome: RunOutcome,
    text: &str,
) -> Vec<SessionEvent> {
    let at = now_ms();
    let run_id = format!("hookstop-{}", uuid::Uuid::new_v4());
    // `Some` exactly when this batch opens the turn it files the receipt under.
    let seeded_turn = (!request.is_resume()).then(TurnId::new_v4);
    let mut events = Vec::with_capacity(5);
    if let Some(turn_id) = seeded_turn {
        events.extend(SessionEvent::user_turn(
            turn_id,
            MessageContent {
                text: request.input.clone(),
                blocks: Vec::new(),
                thinking: None,
                thinking_signature: None,
            },
            crate::scope::room_author_from_metadata(&request.metadata),
            at,
        ));
    }
    events.push(SessionEvent::RunStarted {
        run_id: run_id.clone(),
        at,
        project_root: request
            .workspace_override
            .as_ref()
            .map(|p| p.display().to_string()),
        envelope: None,
    });
    events.push(SessionEvent::Error {
        turn_id: seeded_turn,
        kind: ErrorKind::HookStop,
        message: text.to_string(),
        // The model cannot retry its way past a hook; the hook (or the
        // input it judged) has to change.
        recoverable: false,
        at,
    });
    events.push(SessionEvent::RunFinished {
        run_id,
        outcome,
        at,
    });
    events
}

/// One batch, best-effort: a stop that could not be journaled still stops
/// the run; the warn is the trace. Mirrors
/// `harness_bridge::callback::record_input_block`, the guardrail twin.
///
/// What "best-effort" costs is NOT the same on the two arms that call this:
/// - **deny** (`return Err`): the user sees a `RunError` from the `Err`
///   regardless (`execute.rs`'s failure arm); a lost receipt loses the
///   reload, not the report.
/// - **prevent_continuation** (`return Ok(stop_msg)`): `execute.rs` binds
///   the loop's `Ok` string as `_response` and drops it — no
///   `ResponseChunk`, no `RunComplete` — so the receipt's projected row
///   ("Stopped by hook: …") IS the report. A refused or absent journal there
///   is a silent stop: no error, no text, the turn gone (the pre-receipt
///   behaviour). Logged, not escalated — escalating would change the arm's
///   contract, which is not this writer's call.
///
/// Resolves the process-global service and delegates to
/// [`journal_hook_stop_with`], which is the testable half.
pub(super) async fn journal_hook_stop(request: &RunRequest, outcome: RunOutcome, text: &str) {
    let svc = crate::session::service::global_session_service();
    journal_hook_stop_with(svc.as_deref(), request, outcome, text).await;
}

/// [`journal_hook_stop`] with the service handed in. Returns `()` whatever
/// the store says, so no caller can make its own `return` depend on the
/// receipt — that is the "best-effort" contract, as a type.
pub(super) async fn journal_hook_stop_with(
    svc: Option<&dyn crate::session::service::SessionService>,
    request: &RunRequest,
    outcome: RunOutcome,
    text: &str,
) {
    let Some(svc) = svc else {
        warn!(
            session_key = %request.session_key.to_key_string(),
            "session/service capability absent; hook stop not journaled — see `aleph doctor`"
        );
        return;
    };
    if let Err(e) = svc
        .emit_batch(
            &request.session_key,
            hook_stop_receipt(request, outcome, text),
            None,
        )
        .await
    {
        warn!(
            session_key = %request.session_key.to_key_string(),
            error = %e,
            "hook stop receipt append failed"
        );
    }
}

impl<P: ThinkerProviderRegistry + 'static, R: ToolRegistry + 'static> ExecutionEngine<P, R> {
    /// Run the agent loop (think->act two-step, Claude Code-inspired).
    ///
    /// Uses the flat `LoopToolRegistry`; tool permissions are enforced by
    /// `ScopedToolService` (merged global → agent → channel policy).
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_agent_loop<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        request: &RunRequest,
        agent: Arc<AgentInstance>,
        emitter: Arc<E>,
        deadline: Arc<tokio::sync::Mutex<tokio::time::Instant>>,
        trace_task_id: Option<String>,
        cancel_token: CancellationToken,
        occupancy_out: Arc<std::sync::Mutex<Option<super::helpers::RunContextOccupancy>>>,
    ) -> Result<String, ExecutionError> {
        // Resolve the extension manager + snapshot its HookExecutor once for
        // the whole run. Both flow into `run_agent_loop_inner` so tool
        // dispatch and history compaction can fire hooks without
        // re-snapshotting per turn.
        let extension_manager: Option<Arc<crate::extension::ExtensionManager>> =
            crate::gateway::handlers::plugins::get_extension_manager()
                .ok()
                .map(Arc::clone);
        if let Some(ext_manager) = extension_manager.as_ref() {
            if let Err(e) = ext_manager.ensure_loaded().await {
                warn!("Failed to ensure extension manager is loaded: {}", e);
            }
        }
        let hook_executor = if let Some(ext_manager) = extension_manager.as_ref() {
            let snapshot = ext_manager.hook_executor_snapshot().await;
            (snapshot.hook_count() > 0).then(|| Arc::new(snapshot))
        } else {
            None
        };
        let hook_session_id = request.session_key.to_key_string();

        // BeforeAgentStart — interceptor-kind hooks may abort the run before
        // any provider call; observer-kind hooks just witness the start.
        if let Some(executor) = hook_executor.as_ref() {
            let ctx = lifecycle_hook_context(&hook_session_id, run_id, &agent);
            match executor
                .execute_interceptors(HookEvent::BeforeAgentStart, ctx)
                .await
            {
                Ok((_ctx, hr)) if hr.denied || hr.blocked => {
                    let reason = hr
                        .deny_reason
                        .or(hr.block_reason)
                        .unwrap_or_else(|| "agent start blocked by hook".to_string());
                    warn!(
                        run_id = run_id,
                        reason = %reason,
                        "BeforeAgentStart hook aborted the run"
                    );
                    // §5.4: the receipt goes down BEFORE the stop is reported
                    // — and on this arm it IS reported: the `Err` becomes a
                    // `RunError` in `execute.rs`, so the user sees the reason
                    // whether or not the batch landed; the receipt only adds
                    // the reload. The closer says `Errored` — the plan's word
                    // (the spec said `Cancelled`; both reduce `Clean`, and
                    // `Errored` is what the caller's `RunState::Failed`
                    // already says about this exit).
                    journal_hook_stop(request, RunOutcome::Errored, &reason).await;
                    return Err(ExecutionError::Failed(format!(
                        "BeforeAgentStart hook aborted the run: {reason}"
                    )));
                }
                // Graceful stop (Claude-Code `continue: false`): the hook
                // decided the agent should not start, but this is NOT an error
                // — the run did exactly what the hook asked. Surface the hook's
                // message as the run output instead of failing.
                Ok((_ctx, hr)) if hr.prevent_continuation => {
                    // `stop_message` handles the fallback chain (plain stdout
                    // `messages` → Claude-Code JSON `stopReason` in
                    // `additional_contexts` → default) shared with the
                    // UserPromptSubmit seam and the extension stop gate.
                    let stop_msg = hr.stop_message(
                        "Run halted by BeforeAgentStart hook (prevent_continuation).",
                    );
                    warn!(
                        run_id = run_id,
                        "BeforeAgentStart hook requested prevent_continuation; stopping run"
                    );
                    // §5.4: same receipt as the deny arm; `Cancelled` because
                    // the run did what the hook asked — this is not an error.
                    // Unlike the deny arm, the receipt is the ONLY report
                    // here: `execute.rs` drops this `Ok` string
                    // (`Ok(_response)`), so a refused batch is a silent stop
                    // — see `journal_hook_stop`'s doc.
                    journal_hook_stop(request, RunOutcome::Cancelled, &stop_msg).await;
                    return Ok(stop_msg);
                }
                Ok(_) => {}
                Err(e) => warn!(run_id = run_id, error = %e, "BeforeAgentStart hook failed"),
            }
        }

        // Publish the project root as a task-local for the duration of the
        // think→act loop so child runs spawned mid-loop (session.send, team
        // dispatcher worker tasks, etc.) inherit the project context.
        // `None` is also published explicitly so a nested run cannot leak
        // an outer scope's project into a non-project agent.
        //
        // Alongside it, publish the per-run `FsScope` carrying this run's
        // workspace artifact dir (`<workspace>/output/documents`, the same
        // value `ToolContext::from_workspace` derives). File tools prefer the
        // task-local over the shared `ToolContextHandle`, so a concurrent run
        // rewriting the handle mid-run no longer redirects THIS run's
        // relative-path writes into the other run's workspace. Mirrors the
        // `effective_workspace` fallback inside `run_agent_loop_inner`
        // (override > agent workspace); validation of the override stays in
        // the inner fn — a vanished dir still fails the run there.
        let scope_workspace = request
            .workspace_override
            .clone()
            .unwrap_or_else(|| agent.workspace().to_path_buf());
        // Team-worktree runs (dispatcher members) carry the parent repo root
        // in metadata: build a rebasing worktree scope so the member's file
        // tools anchor at the worktree root AND parent-repo absolute paths
        // are redirected into the checkout — the same semantics the subagent
        // spawner publishes for `IsolationMode::Worktree`. Everything else
        // gets the plain workspace artifact scope.
        let fs_scope = match request.metadata.get("team_worktree_repo_root") {
            Some(repo_root) if request.workspace_override.is_some() => {
                let wt = scope_workspace
                    .canonicalize()
                    .unwrap_or_else(|_| scope_workspace.clone());
                let repo = std::path::PathBuf::from(repo_root);
                let repo = repo.canonicalize().unwrap_or(repo);
                crate::tools::fs_scope::FsScope::worktree(wt, repo)
            }
            _ => {
                crate::tools::fs_scope::FsScope::workspace(scope_workspace.join("output/documents"))
            }
        };
        // Publish the active agent id as a task-local for the whole run so
        // agent-scoped tools (skill_list / skill_read) can resolve this agent's
        // `~/.aleph/agents/<id>/skills` directory. Mirrors the project-root
        // scope below; `None` outside this scope keeps non-agent paths intact.
        // Originating channel user id (raw sender) for the approval-originator
        // gate. Read before `request` is moved into the loop below; published as
        // a run-tree-wide task-local next to `FsScope`/agent-id so the channel
        // approval bridge can stamp it onto a pending record. `None` for
        // non-channel runs — the gate then degrades to the prior behaviour.
        let originator = request.metadata.get(super::ORIGINATOR_USER_KEY).cloned();
        // This run's channel-delivery buffer, published run-tree-wide for the
        // same reason as `originator`: the tool chokepoint that harvests a
        // tool's `_media` sits many frames below here and must not have the
        // buffer threaded through `build_request_tool_service` to reach it.
        // Without this, only the slash fast path (which holds the buffer
        // directly) could ever deliver media to a channel — a model-initiated
        // `media_send` / `image_generate` reached the artifact pane and stopped
        // there. Clone rather than move: `request` goes into the loop below.
        let delivery_media = request.pending_media.clone();
        let mut result = crate::agents::with_agent_id(
            Some(agent.id().to_string()),
            crate::projects::with_project_root(
                request.workspace_override.clone(),
                // The exec-side twin of `fs_scope`, published from the SAME
                // `override > agent workspace` value so the two layers cannot
                // drift on "where does this run work". This is the ONLY channel
                // by which the sandbox learns the authorised root: routing it
                // through the tool's `working_dir` argument (as the tool
                // adapters used to) launders a gateway-owned path through a
                // model-writable field, and the jail — which exists to judge
                // model-supplied paths — then refused it.
                crate::sandbox::context::with_exec_workspace(
                    Some(scope_workspace.clone()),
                    crate::tools::fs_scope::with_fs_scope(
                        Some(fs_scope),
                        // P1 data isolation: publish this run's owner/scope
                        // attribution as a task-local, sibling of `originator`
                        // below — both are derived from `request.metadata` and
                        // must be re-seeded at every spawn boundary that carries
                        // this request's metadata forward (see
                        // `scope::with_scope`'s doc and `carry_policy_metadata`).
                        with_request_scope(
                            request,
                            crate::tools::turn_context::with_originator(
                                originator,
                                crate::gateway::media::with_pending_media(
                                    Some(delivery_media),
                                    self.run_agent_loop_inner(
                                        run_id,
                                        request,
                                        agent.clone(),
                                        emitter,
                                        deadline,
                                        trace_task_id,
                                        cancel_token,
                                        extension_manager,
                                        hook_executor.clone(),
                                        hook_session_id.clone(),
                                        occupancy_out,
                                    ),
                                ),
                            ),
                        ),
                    ),
                ),
            ),
        )
        .await;

        // AgentEnd — observers witness the end; Interceptor-kind hooks may
        // rewrite the final assistant text via `update_output:` (hermes
        // `transform_llm_output` parity). This reuses the exact `updated_output`
        // seam already honored on the AfterToolCall path — no new protocol.
        // block / deny are meaningless post-hoc (the run is over) and ignored.
        if let Some(executor) = hook_executor.as_ref() {
            let mut ctx = lifecycle_hook_context(&hook_session_id, run_id, &agent);
            ctx = ctx.with_env("AGENT_OUTCOME", if result.is_ok() { "ok" } else { "error" });
            match &result {
                Ok(text) => ctx = ctx.with_tool_output(text.clone()),
                Err(e) => ctx = ctx.with_env("AGENT_ERROR", e.to_string()),
            }
            executor.execute_observers(HookEvent::AgentEnd, &ctx).await;
            // Only the success path carries a final text to transform.
            if let Ok(ref mut text) = result {
                if let Ok((_ctx, hr)) = executor
                    .execute_interceptors(HookEvent::AgentEnd, ctx)
                    .await
                {
                    if let Some(new_text) = hr.updated_output {
                        *text = new_text;
                    }
                }
            }
        }
        result
    }
}

/// The receipt a `BeforeAgentStart` stop leaves (§5.4) is a pure builder, so
/// it is tested next to itself; `tests.rs` holds the integration-shaped ones.
#[cfg(test)]
mod hook_stop_tests {
    use super::*;
    use crate::session::events::{
        batch_durability, Durability, ErrorKind, RunOutcome, SessionEvent, SessionEventRecord,
    };
    use crate::session::reduction::{reduce_run, RunDisposition};
    use crate::session::store::event_type_tag;

    fn request(input: &str, resume: bool) -> RunRequest {
        let mut metadata = std::collections::HashMap::new();
        if resume {
            metadata.insert("resume".to_string(), "true".to_string());
        }
        RunRequest {
            run_id: "test-run".to_string(),
            input: input.to_string(),
            session_key: crate::routing::session_key::SessionKey::main("hook-stop"),
            timeout_secs: None,
            metadata,
            attachments: Vec::new(),
            pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            sandbox_override: None,
            workspace_override: None,
            max_iterations_override: None,
            model_override: None,
        }
    }

    fn seq_log(events: Vec<SessionEvent>) -> Vec<SessionEventRecord> {
        events
            .into_iter()
            .enumerate()
            .map(|(i, e)| SessionEventRecord {
                seq: i as u64 + 1,
                event: e,
                created_at_ms: (i as i64 + 1) * 10,
            })
            .collect()
    }

    fn kinds(events: &[SessionEvent]) -> Vec<&'static str> {
        events.iter().map(event_type_tag).collect()
    }

    /// Every payload `at` in the batch is the same instant: it is one moment,
    /// and per-row `now_ms()` calls can straddle a millisecond.
    fn assert_one_instant(events: &[SessionEvent]) {
        let ats: Vec<i64> = events
            .iter()
            .map(|e| match e {
                SessionEvent::TurnStarted { at, .. }
                | SessionEvent::UserMessage { at, .. }
                | SessionEvent::RunStarted { at, .. }
                | SessionEvent::Error { at, .. }
                | SessionEvent::RunFinished { at, .. } => *at,
                other => panic!("unexpected receipt member {other:?}"),
            })
            .collect();
        assert!(
            ats.windows(2).all(|w| w[0] == w[1]),
            "one batch, one instant; got {ats:?}"
        );
    }

    /// §5.4: the log says "a run happened and the hook stopped it" — five
    /// events, reducing Clean, so §5.2 does not read it as an unanswered
    /// message and retrigger it three times.
    #[test]
    fn a_stopping_hook_leaves_five_events_that_reduce_clean() {
        let evs = hook_stop_receipt(
            &request("hi", false),
            RunOutcome::Cancelled,
            "halted by policy",
        );
        assert_eq!(
            kinds(&evs),
            [
                "turn_started",
                "user_message",
                "run_started",
                "error",
                "run_finished"
            ]
        );
        assert!(matches!(
            &evs[3],
            SessionEvent::Error { kind: ErrorKind::HookStop, message, recoverable: false, .. }
                if message == "halted by policy"
        ));
        let r = reduce_run(&seq_log(evs)).expect("legal");
        assert_eq!(r.disposition, RunDisposition::Clean);
        assert!(r.dangling.is_empty() && r.contradictions.is_empty());
    }

    /// The seed pair carries the user's text, the run bracket shares one
    /// `run_id`, the receipt files under the turn the batch opened, and the
    /// closer carries the outcome the arm chose.
    #[test]
    fn the_receipt_is_one_turn_one_run_one_instant() {
        let evs = hook_stop_receipt(&request("hi", false), RunOutcome::Cancelled, "halted");
        assert_one_instant(&evs);
        let (
            SessionEvent::TurnStarted { turn_id: t0, .. },
            SessionEvent::UserMessage {
                turn_id: t1,
                content,
                synthetic,
                ..
            },
        ) = (&evs[0], &evs[1])
        else {
            panic!("seed pair first, got {:?}", kinds(&evs));
        };
        assert_eq!(t0, t1, "the seed pair shares a turn");
        assert_eq!(content.text, "hi");
        assert!(!synthetic, "the user's own words are not harness-authored");
        let (
            SessionEvent::RunStarted {
                run_id: r0,
                envelope,
                ..
            },
            SessionEvent::Error { turn_id, .. },
            SessionEvent::RunFinished {
                run_id: r1,
                outcome,
                ..
            },
        ) = (&evs[2], &evs[3], &evs[4])
        else {
            panic!("run bracket around the receipt, got {:?}", kinds(&evs));
        };
        assert_eq!(r0, r1, "the bracket names one run");
        assert!(
            r0.starts_with("hookstop-"),
            "a locally-minted marker id, got {r0}"
        );
        assert!(envelope.is_none(), "this writer resolved no knobs");
        assert_eq!(
            *turn_id,
            Some(*t0),
            "the receipt files under the turn it opened"
        );
        assert_eq!(*outcome, RunOutcome::Cancelled);
    }

    /// A resumed run already holds its user message; the receipt must not
    /// seed a second, empty one — and with no turn opened here, the receipt
    /// names none rather than a turn the log never saw.
    #[test]
    fn a_hook_stopped_resume_writes_no_seed_pair() {
        let evs = hook_stop_receipt(&request("", true), RunOutcome::Errored, "denied");
        assert_eq!(kinds(&evs), ["run_started", "error", "run_finished"]);
        assert_one_instant(&evs);
        assert!(
            matches!(
                &evs[1],
                SessionEvent::Error {
                    turn_id: None,
                    kind: ErrorKind::HookStop,
                    ..
                }
            ),
            "no turn was opened, so none is named; got {:?}",
            evs[1]
        );
        assert!(matches!(
            &evs[2],
            SessionEvent::RunFinished {
                outcome: RunOutcome::Errored,
                ..
            }
        ));
        let r = reduce_run(&seq_log(evs)).expect("legal");
        assert_eq!(r.disposition, RunDisposition::Clean);
    }

    /// U3: the batch's durability is DERIVED from its members by the store's
    /// policy table, never chosen here. Both shapes carry `RunStarted`, so
    /// both are a barrier — asserted through the real derivation, so a
    /// receipt that lost its marker would fail here, not in production.
    #[test]
    fn both_receipt_shapes_derive_a_barrier_from_their_members() {
        for (resume, label) in [(false, "seeded"), (true, "resume")] {
            let evs = hook_stop_receipt(&request("hi", resume), RunOutcome::Cancelled, "x");
            assert_eq!(
                batch_durability(evs.iter()),
                Durability::Barrier,
                "{label}: a receipt without a barrier member could be lost on crash"
            );
        }
    }

    /// The effect, not the call: after `journal_hook_stop` the session log
    /// holds the receipt, in order, and the real reducer reads it `Clean` —
    /// so the census below is guarding a wire that actually reaches the
    /// store. `SessionKey::ephemeral` mints a fresh id: the shared test
    /// service is one store, and tests keep to their own keys.
    #[tokio::test]
    async fn journal_hook_stop_appends_the_receipt_to_the_session_log() {
        use crate::session::service::SessionService;
        let svc = crate::session::in_process::install_test_session_service();
        let mut req = request("hi", false);
        req.session_key = crate::routing::session_key::SessionKey::ephemeral("hook-stop");

        journal_hook_stop(&req, RunOutcome::Cancelled, "halted by policy").await;

        let records = svc
            .get_events(&req.session_key, None, None)
            .await
            .expect("the log reads back");
        let logged: Vec<&'static str> = records.iter().map(|r| event_type_tag(&r.event)).collect();
        assert_eq!(
            logged,
            [
                "turn_started",
                "user_message",
                "run_started",
                "error",
                "run_finished"
            ],
            "the receipt reached the store as one batch, in order"
        );
        let r = reduce_run(&records).expect("legal");
        assert_eq!(r.disposition, RunDisposition::Clean);
    }

    /// The best-effort contract as behaviour: a refused batch and an absent
    /// service both come back normally, so the arm's `return` that follows
    /// the call runs unconditionally (`journal_hook_stop_with` returns `()`;
    /// there is nothing an arm could branch on). What each arm LOSES when
    /// this happens differs and is stated in `journal_hook_stop`'s doc. This
    /// goes red the day someone `expect`s the batch result inside.
    #[tokio::test]
    async fn a_refused_or_absent_journal_returns_normally() {
        let refusing = super::super::tests::RefusingSessionService;
        let req = request("hi", false);
        journal_hook_stop_with(Some(&refusing), &req, RunOutcome::Cancelled, "halted").await;
        journal_hook_stop_with(None, &req, RunOutcome::Errored, "denied").await;
    }

    /// §5.2 "a hook-stopped resume does not retrigger", on the two REAL resume
    /// shapes rather than the receipt in isolation. The coordinator stamps
    /// `ResumeAttempted{target}` naming the open `RunStarted` (interrupted)
    /// or the unanswered `UserMessage`, writes no `RunFinished` before
    /// retriggering, and the retriggered run is then stopped by a hook,
    /// leaving the resume-arm receipt. Without the receipt each log reads
    /// `Interrupted{1}` / `Unanswered{1}` — the shape the coordinator would
    /// stamp and retrigger again; with it, `Clean` and no contradiction.
    #[test]
    fn a_hook_stopped_resume_reduces_clean_in_both_real_shapes() {
        let turn = TurnId::new_v4();
        let seed = SessionEvent::user_turn(
            turn,
            MessageContent {
                text: "hi".into(),
                blocks: Vec::new(),
                thinking: None,
                thinking_signature: None,
            },
            None,
            1,
        );
        let receipt = || hook_stop_receipt(&request("", true), RunOutcome::Cancelled, "halted");

        // Interrupted: the crash landed after `RunStarted` (seq 3).
        let mut interrupted: Vec<SessionEvent> = seed.to_vec();
        interrupted.push(SessionEvent::RunStarted {
            run_id: "crashed".into(),
            at: 1,
            project_root: None,
            envelope: None,
        });
        interrupted.push(SessionEvent::ResumeAttempted {
            target: 3,
            attempt: 1,
        });
        let before = reduce_run(&seq_log(interrupted.clone())).expect("legal");
        assert_eq!(
            before.disposition,
            RunDisposition::Interrupted { attempts: 1 },
            "control: without the receipt this is what the coordinator would retrigger"
        );
        interrupted.extend(receipt());
        let after = reduce_run(&seq_log(interrupted)).expect("legal");
        assert_eq!(after.disposition, RunDisposition::Clean);
        assert!(
            after.contradictions.is_empty(),
            "{:?}",
            after.contradictions
        );

        // Unanswered: the crash landed before `RunStarted` (the message is seq 2).
        let mut unanswered: Vec<SessionEvent> = seed.to_vec();
        unanswered.push(SessionEvent::ResumeAttempted {
            target: 2,
            attempt: 1,
        });
        let before = reduce_run(&seq_log(unanswered.clone())).expect("legal");
        assert_eq!(
            before.disposition,
            RunDisposition::Unanswered {
                user_seq: 2,
                attempts: 1
            },
            "control: without the receipt this is what the coordinator would retrigger"
        );
        unanswered.extend(receipt());
        let after = reduce_run(&seq_log(unanswered)).expect("legal");
        assert_eq!(after.disposition, RunDisposition::Clean);
        assert!(
            after.contradictions.is_empty(),
            "{:?}",
            after.contradictions
        );
    }

    /// The corpus the two censuses below walk: each file of the loop that
    /// dispatches lifecycle hooks, as comment- and literal-stripped production
    /// code, with the **hand-off** after which a hook stop is no longer
    /// "before the seed" — the seed pair is written by the bridge's
    /// `seed_history`, which runs inside the stage each hand-off starts.
    /// Everything before the hand-off is pre-seed by construction; nothing
    /// here names which hooks live there.
    struct PreSeedFile {
        name: &'static str,
        code: String,
        hand_off: &'static str,
    }

    fn pre_seed_files() -> Vec<PreSeedFile> {
        use crate::utils::source_scan::{code_text, production_prefix};
        vec![
            PreSeedFile {
                name: "run_loop/mod.rs",
                code: code_text(&production_prefix(include_str!("mod.rs"))),
                hand_off: ".run_agent_loop_inner(",
            },
            PreSeedFile {
                name: "run_loop/inner.rs",
                code: code_text(&production_prefix(include_str!("inner.rs"))),
                hand_off: "run_dispatch_and_drain_classified(",
            },
        ]
    }

    /// The dispatch line is the anchor: a hook-event name inside a comment or
    /// a `warn!` string cannot satisfy it (`code_text` strips both).
    const DISPATCH: &str = "execute_interceptors(HookEvent::";

    /// Where a file's pre-seed region ends — asserted unique so a moved or
    /// renamed hand-off cannot silently widen or empty the region.
    fn hand_off_at(file: &PreSeedFile) -> usize {
        assert_eq!(
            file.code.matches(file.hand_off).count(),
            1,
            "{}: the hand-off marker `{}` must occur exactly once in the production half",
            file.name,
            file.hand_off
        );
        file.code.find(file.hand_off).expect("counted once above")
    }

    /// The block a hook dispatch is matched on: from the first `{` after the
    /// anchor to its matching `}`. Brace-balanced on stripped code, so a
    /// brace inside a literal cannot desynchronise it, and indifferent to
    /// how the arms are spelled (`Ok(_) => {}`, `Err(e) =>`, an `if let`).
    ///
    /// **It recognises exactly ONE shape of stop exit**: a `return ` (with
    /// the trailing space) lexically inside that first brace block. It
    /// cannot see a hoisted result (`let hr = match … { … }; if hr.denied {
    /// return … }` — the `return` is in the NEXT statement), a `?`, or a
    /// bare `return;`. Both live seams are the recognised shape, and
    /// `the_twin_hook_seams_fire_before_anything_seeds_the_turn` pins them
    /// by name; a seam written in one of the unseen shapes would be counted
    /// as zero exits and pass — write it in this shape, or extend this.
    fn dispatch_body(after_anchor: &str) -> &str {
        let open = after_anchor
            .find('{')
            .expect("a hook dispatch is followed by a block");
        let mut depth = 0usize;
        for (i, c) in after_anchor.char_indices() {
            if i < open {
                continue;
            }
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return after_anchor.get(open..=i).expect("char boundaries");
                    }
                }
                _ => {}
            }
        }
        panic!("unbalanced braces after a hook dispatch:\n{after_anchor}");
    }

    /// Every hook exit that stops a run before the seed owes the receipt, and
    /// owes it BEFORE it returns. Derived, not listed: the corpus is the two
    /// loop files, the sites are every interceptor dispatch before each
    /// file's hand-off, the exits are the `return`s inside each dispatch's
    /// block, and the equality is `journal calls == exits` — so a third
    /// pre-seed seam with a bare `return` is red on arrival, and a journal
    /// call moved after its `return` (or into the caller) leaves that exit's
    /// segment empty. The post-seed dispatch (`AgentEnd`) sits after the
    /// hand-off and is deliberately outside the rule.
    #[test]
    fn every_pre_seed_hook_exit_journals_the_stop() {
        let mut exits_total = 0usize;
        let mut journaled_total = 0usize;
        let mut sites: Vec<(&str, String, usize)> = Vec::new();
        for file in pre_seed_files() {
            let cut = hand_off_at(&file);
            for (offset, _) in file.code.match_indices(DISPATCH) {
                if offset >= cut {
                    continue;
                }
                // The body is taken from the whole file so a block that
                // straddles the cut is not truncated; only the site's
                // position decides whether it is governed.
                let after = file
                    .code
                    .get(offset + DISPATCH.len()..)
                    .expect("char boundary");
                let hook = after.split(',').next().unwrap_or("?").to_string();
                let body = dispatch_body(after);
                let segments: Vec<&str> = body.split("return ").collect();
                let exits = segments.len() - 1;
                for (i, before) in segments[..exits].iter().enumerate() {
                    let n = before.matches("journal_hook_stop(").count();
                    assert_eq!(
                        n, 1,
                        "{}: HookEvent::{hook} exit {i} must journal exactly once before it \
                         returns; segment:\n{before}",
                        file.name
                    );
                    journaled_total += n;
                }
                assert_eq!(
                    segments[exits].matches("journal_hook_stop(").count(),
                    0,
                    "{}: HookEvent::{hook} — a journal call after the last return is unreachable",
                    file.name
                );
                exits_total += exits;
                sites.push((file.name, hook, exits));
            }
        }
        assert_eq!(
            journaled_total, exits_total,
            "every pre-seed stop exit journals; derived sites (file, hook, exits): {sites:?}"
        );
        assert!(
            exits_total > 0,
            "no pre-seed hook exit found at all — the dispatch anchor or a hand-off rotted; \
             sites: {sites:?}"
        );
    }

    /// The twins are PRE-SEED, which is what licenses the receipt to write the
    /// seed pair (and, on a resume, to skip it): each dispatch sits before its
    /// file's hand-off, and no seed writer is spelled on the path from the
    /// enclosing function's signature to the dispatch. `prepare_history` on
    /// that path may compact, but the seed-pair constructors it could reach
    /// are pinned by `reduction::tests::user_message_producers_are_the_known_set`
    /// — none is in the compactor. Names are written here on purpose: this
    /// pins two facts, it does not count anything.
    #[test]
    fn the_twin_hook_seams_fire_before_anything_seeds_the_turn() {
        const SEED_WRITERS: [&str; 4] = [
            "user_turn(",
            "seed_history(",
            "seed_session(",
            "UserMessage {",
        ];
        let files = pre_seed_files();
        for (name, enclosing_fn, hook) in [
            ("run_loop/mod.rs", "fn run_agent_loop<", "BeforeAgentStart"),
            (
                "run_loop/inner.rs",
                "fn run_agent_loop_inner<",
                "UserPromptSubmit",
            ),
        ] {
            let file = files
                .iter()
                .find(|f| f.name == name)
                .expect("the corpus names this file");
            let anchor = format!("{DISPATCH}{hook},");
            assert_eq!(
                file.code.matches(&anchor).count(),
                1,
                "{name}: exactly one HookEvent::{hook} dispatch"
            );
            let at = file.code.find(&anchor).expect("counted once above");
            let start = file
                .code
                .find(enclosing_fn)
                .expect("the enclosing function is where it was");
            assert!(
                start < at && at < hand_off_at(file),
                "{name}: HookEvent::{hook} must fire inside {enclosing_fn}.. and before the \
                 hand-off `{}` — otherwise it is no longer pre-seed and the receipt would \
                 double-seed",
                file.hand_off
            );
            let path = file.code.get(start..at).expect("char boundaries");
            for writer in SEED_WRITERS {
                assert_eq!(
                    path.matches(writer).count(),
                    0,
                    "{name}: `{writer}` on the path to HookEvent::{hook} — the seam is no \
                     longer pre-seed; `hook_stop_receipt` would seed a second user message"
                );
            }
        }
    }
}
