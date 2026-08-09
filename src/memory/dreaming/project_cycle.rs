//! Per-corpus dream sub-cycle.
//!
//! Every note corpus that is not the base agent's — a non-default agent, a
//! personal scope (`{base}__u-*`), a room scope (`{base}__p-*`), a
//! project-directory namespace (`{base}__proj-*`) — gets the note-maintenance
//! subset of the pipeline ([`retain_project_stages`]) so its notes are linted,
//! consolidated and synthesised like the base agent's.
//!
//! Note that only `proj-*` is gated by `memory.project_scoped`: personal and
//! room partitions are produced by session scope, which does not consult that
//! flag, and a non-default agent is just a directory. Enumerating corpora is
//! therefore [`list_note_corpora`]'s job, never a config read.
//!
//! [`list_note_corpora`]: crate::memory::project_scope::list_note_corpora
//!
//! # Why a namespace governs itself
//!
//! The maintenance subset is exactly the part that *produces* churn signals:
//! `note_consolidate` writes `merged_pairs` and `note_synthesis` writes
//! `synthesis_rewrites` — the two inputs [`MutationGate`] folds into its
//! merge-cycle and synthesis-churn detectors. Before this module those reports
//! were `info!`-logged and dropped, so a project corpus could merge A→B and
//! B→A every night forever with nothing able to see it, let alone stop it.
//!
//! Folding them into the **base** agent's event log instead would have been
//! worse than dropping them. A note `path` is relative *within* an agent
//! (`"reference/rust-ownership"` — see `notes::store::NoteIndexEntry`), so
//! `proj-a`'s `skill/foo` and the base agent's `skill/foo` are the same string.
//! Merging the histories would hand the base gate phantom merge cycles for
//! notes it does not own, and phantom churn is worse than none: it conserves
//! the corpus that was behaving.
//!
//! So each namespace is its own dream subject — its own `dream_events.jsonl`,
//! its own gate window, its own personality, its own best-health checkpoint
//! (`dream_best_health__*` is already keyed by agent id). All of it is folded
//! out of that one log at the start of every sub-cycle, so the state is
//! reconstructible from a single persistent source instead of living in the
//! daemon process (constitution A3) — the same shape as the base cycle.
//!
//! [`retain_project_stages`]: super::DreamPipeline::retain_project_stages

use std::collections::HashMap;
use std::path::Path;

use tracing::warn;

use crate::config::types::memory::MemoryDecayPolicy;
use crate::config::DreamingConfig as ConfigDreamingConfig;
use crate::error::AlephError;
use crate::memory::embedding_provider::EmbeddingProvider;
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::NoteIndexer;
use crate::memory::store::MemoryBackend;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

use super::evolution::{
    evaluate_gate, memory_health_score, EditBudget, EvolutionOutcome, GateOutcome,
    HEALTH_GATE_EPSILON,
};
use super::validation::{self, DreamValidationReport};
use super::{
    compute_raw_metrics, l1_over_corpus, now_timestamp, CycleDecision, DreamContext,
    DreamCycleOutcome, DreamEvent, DreamPipeline, DreamReport, DreamReportStatus, DreamRunStatus,
    EventLog, MutationGate, NoteEntry, SignalSnapshot, StrategySelector, DREAM_HISTORY_WINDOW,
};

/// Everything a project sub-cycle borrows from the daemon.
///
/// Passed as one struct rather than eight positional arguments: the base cycle
/// already threads the identical set through `DreamContext`, and a positional
/// list of four `Arc`s is the shape that silently swaps two of them.
pub(super) struct ProjectCycleDeps<'a> {
    pub memory_dir: &'a Path,
    pub database: &'a MemoryBackend,
    pub provider: &'a Arc<dyn AiProvider>,
    pub embedder: &'a Arc<dyn EmbeddingProvider>,
    pub config: &'a ConfigDreamingConfig,
    pub decay_policy: &'a MemoryDecayPolicy,
    pub orientation: &'a Option<Arc<dyn crate::memory::notes::orientation::NoteOrientation>>,
    pub activity_checker: &'a Arc<dyn Fn() -> bool + Send + Sync>,
}

/// Outcome of asking a corpus to dream.
///
/// The fan-out runs over **every** corpus on disk, so most nights most corpora
/// have nothing to maintain. Telling "ran and produced nothing" apart from
/// "never started, spent no tokens" is what lets the caller's nightly budget
/// count only the cycles that actually cost something.
pub(super) enum NamespaceCycle {
    /// No note in this corpus has changed since its last cycle (or it holds no
    /// notes at all). Not one stage started, so not one LLM call was made — the
    /// night's per-corpus budget is untouched.
    Idle,
    /// The corpus ran the maintenance subset. Carries the same
    /// [`DreamCycleOutcome`] the base cycle produces, decision included.
    Ran(Box<DreamCycleOutcome>),
}

/// Whether a corpus has anything for the maintenance subset to chew on.
///
/// Three independent inputs, because the subset has three independent sources
/// of work and gating on any subset of them strands the rest:
///
/// * **note content.** `note_lint` / `note_consolidate` / `note_synthesis` /
///   `skill_distill` are pure functions of the notes, so a corpus whose notes
///   have not moved since its own last cycle would re-derive byte-identical
///   verdicts at full LLM price. `since` is the *start* of that corpus's last
///   dream cycle, deliberately not its end: notes the cycle itself wrote carry
///   an `updated_at` from the middle of the run, so measuring from the end
///   would exclude exactly the writes that mean a capped stage still has
///   backlog. Measuring from the start admits the corpus for one more night
///   after any write, and then — if that night changes nothing — stops. A
///   corpus that has never dreamed passes `0` and is always admitted.
/// * **the review queue.** `notes_review_queue` rows are enqueued by the ingest
///   governance gate for candidates that were deliberately *not* written as
///   notes. They therefore move no `updated_at`, and a content-only gate would
///   leave them pending forever — the queue's only consumer is `note_review`,
///   which only runs inside this cycle.
/// * **undistilled corrections.** `flag_user_correction` writes raw-memory rows
///   keyed on the *turn's* agent, and `feedback_distill` is the only reader.
///   Those rows touch no note and enqueue no review, so the two gates above are
///   both blind to them — a corpus whose only new work is a batch of
///   corrections would sit at `Idle` forever while the tool told the user the
///   nightly cycle would distill them. The answer comes from
///   [`has_undistilled_corrections`], i.e. the stage's own watermark read, so
///   the gate cannot disagree with the stage it is gating.
///
/// * **undistilled tool failures.** `RawMemoryToolSink` writes one row per tool
///   call keyed on the *turn's* agent, and `tool_failure_distill` is their only
///   distiller. Exactly like corrections, those rows move no `updated_at` and
///   enqueue no review, so a corpus whose only new work is "the same tool kept
///   failing" would sit at `Idle` forever. The answer again comes from the
///   stage's own predicate ([`has_undistilled_tool_failures`]) — same
///   watermark, same window, same quorum — so gate and stage cannot disagree.
///
/// Deliberately **not** a config knob: this is not a policy about how eager
/// maintenance should be, it is the observation that re-judging unchanged bytes
/// cannot produce a different answer.
///
/// [`has_undistilled_corrections`]: crate::memory::dreaming::stages::feedback_distill::has_undistilled_corrections
/// [`has_undistilled_tool_failures`]: crate::memory::dreaming::stages::tool_failure_distill::has_undistilled_tool_failures
#[must_use]
pub(super) fn corpus_needs_maintenance(
    index: &[crate::memory::notes::store::NoteIndexEntry],
    since: i64,
    pending_reviews: usize,
    has_corrections: bool,
    has_tool_failures: bool,
) -> bool {
    has_corrections
        || has_tool_failures
        || pending_reviews > 0
        || index.iter().any(|n| n.updated_at >= since)
}

/// Run one corpus's maintenance cycle, governed by that corpus's
/// own history.
///
/// Mirrors the base cycle's phases (rehydrate → gate → select → run → validate →
/// evolution gate → solidify) keyed on `agent_id` instead of the base agent. The
/// caller treats a failure as non-fatal: one bad namespace must never abort the
/// night.
///
/// Returns [`NamespaceCycle::Ran`] with the same [`DreamCycleOutcome`] the base
/// cycle produces, decision included. The decision used to be computed here and
/// dropped on the floor, which is why a project corpus's history was legible to
/// the model (it reads the namespace's own event log) and to nobody else: the
/// audit row the Panel reads is written by the caller, and the caller had
/// nothing to write.
pub(super) async fn run_namespace_cycle(
    deps: &ProjectCycleDeps<'_>,
    agent_id: &str,
) -> Result<NamespaceCycle, AlephError> {
    let started_at = now_timestamp();
    let log = EventLog::new(deps.memory_dir.join(agent_id));

    // --- Rehydrate this namespace's cross-cycle state ---
    // One read serves three consumers, exactly as in the base cycle: the prior
    // report's rot counts, the gate's churn windows, and the selector's
    // personality window. A read failure degrades to an empty history (disarmed
    // detectors, neutral personality) rather than skipping the namespace.
    let history = log
        .read_last(DREAM_HISTORY_WINDOW)
        .await
        .unwrap_or_else(|e| {
            warn!(agent = %agent_id, error = %e, "failed to read project dream event log; cross-cycle state starts empty");
            Vec::new()
        });
    let prior_report = history.last().map(|ev| ev.report.clone());

    let index = deps.database.list_notes(agent_id).await.unwrap_or_else(|e| {
        warn!(agent = %agent_id, error = %e, "failed to list notes for project namespace, proceeding with empty index");
        Vec::new()
    });
    // --- Budget gate: an unchanged corpus is not worth an LLM call ---
    // Placed after the two reads it needs and before anything that can dispatch
    // a prompt, so a quiet corpus costs exactly one log read and one index read.
    // The fan-out covers every corpus on disk, so without this the nightly bill
    // would scale with the number of partitions rather than with activity.
    let last_cycle_started_at = history.last().map_or(0, |ev| ev.report.started_at);
    // `i64::MAX` rather than the review stage's dwell bound on purpose: a
    // not-yet-ripe row still means work is coming, and admitting a corpus one
    // night early is cheap next to stranding its queue. A read failure admits
    // too — this gate may only ever *save* a call, never withhold one on a
    // guess.
    let pending_reviews = deps
        .database
        .list_pending_review(agent_id, i64::MAX)
        .await
        .map_or(usize::MAX, |rows| rows.len());
    // Third leg: corrections filed against THIS agent that `feedback_distill`
    // has not consumed. Read through the stage's own predicate so gate and
    // stage share one watermark.
    let has_corrections =
        crate::memory::dreaming::stages::feedback_distill::has_undistilled_corrections(
            deps.database,
            agent_id,
            deps.config.feedback_lookback,
        )
        .await;
    // Fourth leg: tool failures recorded against THIS agent that
    // `tool_failure_distill` has not consumed. Same shape as corrections —
    // read through the stage's own predicate so gate and stage share one
    // watermark.
    let has_tool_failures =
        crate::memory::dreaming::stages::tool_failure_distill::has_undistilled_tool_failures(
            deps.database,
            agent_id,
        )
        .await;
    if !corpus_needs_maintenance(
        &index,
        last_cycle_started_at,
        pending_reviews,
        has_corrections,
        has_tool_failures,
    ) {
        tracing::debug!(
            agent = %agent_id,
            notes = index.len(),
            last_cycle_started_at,
            "corpus unchanged since its last dream cycle, its review queue is \
             empty and it has neither undistilled corrections nor undistilled \
             tool failures; skipping"
        );
        return Ok(NamespaceCycle::Idle);
    }

    let raw_metrics = compute_raw_metrics(
        &index,
        deps.database.as_ref(),
        agent_id,
        prior_report.as_ref(),
    )
    .await;
    let signal_snapshot = SignalSnapshot::from_metrics(&raw_metrics);
    let baseline_health = memory_health_score(&signal_snapshot);

    // --- Gate + strategy, from this namespace's own history ---
    let gate_decision = MutationGate::from_reports(history.iter().map(|ev| &ev.report)).evaluate();
    let selection =
        StrategySelector::from_outcomes(history.iter().map(|ev| ev.validation.overall_ok()))
            .select(&signal_snapshot, &gate_decision);
    let strategy = selection.strategy;

    // --- Run the note-maintenance subset ---
    let notes: Vec<NoteEntry> = index.iter().map(NoteEntry::from_index_entry).collect();
    // rust-doctor-disable-next-line excessive-clone
    let indexer = NoteIndexer::new(deps.memory_dir.to_path_buf(), deps.database.clone())
        // rust-doctor-disable-next-line excessive-clone
        .with_embedder(deps.embedder.clone());
    let ctx = DreamContext {
        notes,
        note_contents: HashMap::new(),
        agent_id: agent_id.to_string(),
        // rust-doctor-disable-next-line excessive-clone
        database: deps.database.clone(),
        indexer,
        // rust-doctor-disable-next-line excessive-clone
        provider: deps.provider.clone(),
        // rust-doctor-disable-next-line excessive-clone
        embedder: deps.embedder.clone(),
        report: DreamReport {
            pipeline_type: strategy.to_string(),
            started_at,
            ..Default::default()
        },
        pipeline_type: strategy.to_string(),
        strategy,
        // rust-doctor-disable-next-line excessive-clone
        activity_checker: deps.activity_checker.clone(),
        orientation: deps.orientation.clone(),
        evolution_budget: EditBudget::default(),
    };
    let mut report = DreamPipeline::from_strategy(strategy, deps.config, deps.decay_policy)
        .retain_project_stages()
        .run(ctx)
        .await?;
    report.finished_at = now_timestamp();
    report.duration_ms = ((report.finished_at - started_at).max(0) as u64) * 1000;

    // --- Validation over the post-cycle corpus ---
    // Real L1/L2 here is load-bearing, not audit decoration: `overall_ok()` is
    // what the *next* cycle's `StrategySelector` folds into this namespace's
    // personality. A vacuous pass would rubber-stamp every cycle and freeze the
    // synthesize threshold — the failure mode the base cycle already fixed.
    let post_index = deps.database.list_notes(agent_id).await.unwrap_or_else(|e| {
        warn!(agent = %agent_id, error = %e, "failed to list notes for project post-cycle health check, proceeding with empty index");
        Vec::new()
    });
    let l2_pairs: Vec<(String, String)> = post_index
        .iter()
        // rust-doctor-disable-next-line excessive-clone
        .map(|n| (n.path.clone(), n.content_hash.clone()))
        .collect();
    let validation_report = DreamValidationReport {
        l1_format: l1_over_corpus(deps.memory_dir, agent_id, &post_index).await,
        l2_consistency: validation::run_l2_validation(&l2_pairs),
        l3_semantic: None,
        l4_retrospective: None,
    };

    // --- Evolution gate (memory health before/after) ---
    let post_metrics = compute_raw_metrics(
        &post_index,
        deps.database.as_ref(),
        agent_id,
        prior_report.as_ref(),
    )
    .await;
    report.distill_produced = post_metrics.mature_skill_total;
    report.distill_recalled = post_metrics.mature_skill_recalled;
    let candidate_health = memory_health_score(&SignalSnapshot::from_metrics(&post_metrics));
    // Unlike the base agent, a namespace keeps no in-process checkpoint: the
    // best-ever score is read from and written back to `dream_best_health__*`
    // every cycle. Namespaces come and go with the projects that spawned them,
    // so holding a per-namespace `Mutex<f64>` in the daemon would be state that
    // outlives its subject — and the read is one keyed lookup a night.
    let best_before = deps
        .database
        .get_best_health(agent_id)
        .unwrap_or(None)
        .unwrap_or(0.0);
    let gate_outcome = evaluate_gate(
        candidate_health,
        baseline_health,
        best_before,
        HEALTH_GATE_EPSILON,
    );
    let new_best = if gate_outcome == GateOutcome::AcceptNewBest {
        candidate_health
    } else {
        best_before
    };
    if gate_outcome == GateOutcome::AcceptNewBest {
        if let Err(e) = deps.database.set_best_health(agent_id, new_best) {
            warn!(agent = %agent_id, error = %e, "failed to persist project best_health checkpoint");
        }
    }
    report.evolution = Some(EvolutionOutcome {
        baseline: baseline_health,
        candidate: candidate_health,
        best: new_best,
        outcome: gate_outcome,
        merges_rejected: report.merges_rejected,
    });

    // --- Solidify (event log + the caller's audit row) ---
    let decision = CycleDecision {
        strategy,
        // rust-doctor-disable-next-line excessive-clone
        rationale: selection.rationale.clone(),
        personality_adjustment: selection.personality_adjustment,
        // rust-doctor-disable-next-line excessive-clone
        gate: gate_decision.clone(),
        // rust-doctor-disable-next-line excessive-clone
        stages: report.stages_executed.clone(),
        validation_passed: validation_report.overall_ok(),
    };
    let status = if report.status == DreamReportStatus::Interrupted {
        DreamRunStatus::Cancelled
    } else {
        DreamRunStatus::Success
    };

    // A cycle that yielded before running a single stage produced nothing to
    // account for — see `DreamReport::is_vacuous_interruption` for why neither
    // durable record wants it. The caller reads the same predicate before
    // writing its audit row.
    if report.is_vacuous_interruption() {
        return Ok(NamespaceCycle::Ran(Box::new(DreamCycleOutcome {
            status,
            report,
            decision,
        })));
    }

    let cycle = log.next_cycle().await.unwrap_or(1);
    let event = DreamEvent {
        id: format!("dream_{started_at}_{cycle}"),
        cycle,
        strategy,
        selection,
        gate_decision,
        // rust-doctor-disable-next-line excessive-clone
        report: report.clone(),
        validation: validation_report,
        duration_ms: report.duration_ms,
        created_at: now_timestamp(),
    };
    if let Err(e) = log.append(&event).await {
        // Same cost as the base cycle's append: this is not merely a lost audit
        // line — the next cycle's churn gate and personality window will not see
        // this one at all.
        tracing::error!(
            agent = %agent_id,
            error = %e,
            "failed to write project dream event log — the next cycle's churn gate \
             and personality window will not see this cycle"
        );
    }

    Ok(NamespaceCycle::Ran(Box::new(DreamCycleOutcome {
        status,
        report,
        decision,
    })))
}
