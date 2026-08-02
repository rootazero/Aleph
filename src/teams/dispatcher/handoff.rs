//! Handoff context builder.
//!
//! Assembles the deterministic context envelope injected into a member agent
//! when the dispatcher launches it for a task. Inspired by the "worker
//! context" pattern: instead of a background sensing loop, everything the
//! member needs is gathered once, at launch, from the task DAG and team state.

use crate::agents::swarm::tasks::acceptance::{
    lead_review_required, read_acceptance_criteria, render_acceptance_section,
};
use crate::agents::swarm::tasks::{CoordTask, CoordTaskStatus, CoordTaskStore, TaskRunStatus};
use crate::sync_primitives::Arc;
use crate::teams::artifacts::ArtifactStore;
use crate::teams::context::InboxContextProvider;
use crate::teams::store::TeamStore;

/// Max bytes kept per free-form section (task body, each dependency result).
const MAX_SECTION_BYTES: usize = 4096;

/// Budget for the deliverable half of the WHOLE fan-in block, split evenly
/// across the dependencies.
///
/// The per-dependency cap alone is not a bound: a 40-way fan-in (per-file
/// analysis feeding one synthesis node — the natural map/reduce shape this
/// engine exists to run) could contribute 40 × [`MAX_SECTION_BYTES`] before the
/// first model call. Dividing a total keeps every dependency represented
/// (narrower slices) instead of dropping the tail. Chosen so that a fan-in of
/// six or fewer keeps the full [`MAX_SECTION_BYTES`] slice each — i.e. a
/// dependency with no exit journal renders byte-identically to before.
///
/// A dependency's exit journal rides on top of its slice, itself capped at
/// `slice / 4` (and [`MAX_DEP_JOURNAL_BYTES`]), so the whole block stays under
/// `MAX_DEP_SECTION_TOTAL_BYTES × 1.25` at any width.
const MAX_DEP_SECTION_TOTAL_BYTES: usize = MAX_SECTION_BYTES * 6;

/// Floor for one dependency's slice. Below this a section cannot carry enough
/// to be worth reading, so a very wide fan-in exceeds the total rather than
/// degenerating into 40 unreadable stubs.
const MIN_DEP_BYTES: usize = 512;

/// Max items surfaced per exit-journal list on a dependency (artifacts / next
/// steps). The full journal stays available to the owner via `task_*` tools.
const MAX_DEP_JOURNAL_ITEMS: usize = 5;

/// Bytes per exit-journal list item carried across an edge.
const MAX_DEP_JOURNAL_ITEM_BYTES: usize = 256;

/// Ceiling for one dependency's exit-journal block, whatever the fan-in width.
const MAX_DEP_JOURNAL_BYTES: usize = 1024;

/// Per-dependency byte slice for a fan-in of `dep_count`.
fn dep_budget(dep_count: usize) -> usize {
    if dep_count <= 1 {
        return MAX_SECTION_BYTES;
    }
    (MAX_DEP_SECTION_TOTAL_BYTES / dep_count).clamp(MIN_DEP_BYTES, MAX_SECTION_BYTES)
}

/// Max bytes kept for the injected team protocol. Larger than a normal section
/// because the operating agreement may enumerate roles, hand-off rules, and
/// quality gates, yet still bounded so a runaway protocol cannot dominate the
/// member's context window.
const MAX_PROTOCOL_BYTES: usize = 8192;

/// How many of the most-recent prior attempts to surface in the recovery
/// section. Bounded so a task that has been retried many times cannot flood
/// the resuming member's context: the exit journal carries the durable
/// hand-off, the run log only needs to show the latest failure reasons.
const MAX_RECOVERY_RUNS: usize = 3;

/// Truncate `s` to at most `max` bytes on a UTF-8 char boundary.
fn truncate_utf8(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… (truncated)", &s[..end])
}

/// Build the **recovery** section for a re-dispatched task.
///
/// Returns an empty string for a task's first attempt (no prior run records).
/// On a retry — bounded automatic re-dispatch on `Failed`/`Timeout`
/// ([`fail_or_retry`](super::schedule), the common case), an orphan reclaim, or a
/// leader-driven `Failed`/`Cancelled` → `Pending` reset — it surfaces the durable
/// hand-off that earlier attempts produced but which, until now, only the panel
/// ever read: the per-attempt run log (`coord_task_runs`) and the structured exit
/// journal (`coord_task_journals`, written by the `task_exit_journal` tool).
///
/// This closes the write→read loop on `ClawTeam`'s context-recovery pattern: the
/// resuming agent reads what its previous self learned instead of cold-starting.
/// Pure context assembly — the dumb loop performs no recovery *decision* beyond a
/// mechanical retry ceiling (R7/R10); *how* to recover stays the model's call,
/// expressed through this injected context.
async fn build_recovery_section(coord_store: &Arc<dyn CoordTaskStore>, task: &CoordTask) -> String {
    // `build_handoff_context` runs before `start_task_run`, so every row here
    // belongs to an *earlier* attempt — the current claim is not yet recorded.
    let runs = coord_store
        .list_task_runs(&task.id)
        .await
        .unwrap_or_default();
    if runs.is_empty() {
        return String::new();
    }

    let attempt = runs.len() + 1;
    // A rejected run DID complete — a reviewer sent it back. Saying "did not
    // complete" to a member whose work was turned down is not a wording
    // quibble: it tells it to resume and reuse, when the actual instruction is
    // to change what the reviewer objected to. The verdict is written by five
    // producers (`record_run_review` from both review tools and both RPCs) and,
    // until now, read by none.
    let rejected = runs
        .iter()
        .any(|r| r.review_verdict == Some(crate::agents::swarm::tasks::ReviewVerdict::Rejected));
    let mut out = if rejected {
        format!(
            "\n## Recovery Context\nThis is attempt {attempt}. A previous attempt was \
             REVIEWED AND REJECTED — read the reviewer's note below and in Notes, and change \
             what they objected to. Do not simply resubmit the same work.\n"
        )
    } else {
        format!(
            "\n## Recovery Context\nThis is attempt {attempt}. {} previous attempt(s) did not \
             complete — resume from where they left off; reuse work already done and do not \
             restart from scratch.\n",
            runs.len()
        )
    };

    // Most-recent prior attempts first, capped. Each line names the terminal
    // status and the failure reason / final note so the member sees *why* it
    // is being re-run.
    out.push_str("\n### Previous attempts\n");
    for run in runs.iter().rev().take(MAX_RECOVERY_RUNS) {
        let detail = match (&run.error, &run.summary) {
            (Some(err), _) if !err.is_empty() => truncate_utf8(err, MAX_SECTION_BYTES),
            (_, Some(sum)) if !sum.is_empty() => truncate_utf8(sum, MAX_SECTION_BYTES),
            _ => match run.status {
                // A run still marked Running here never reached `finish_task_run`
                // — the process died mid-task and was reclaimed. Abandoned is
                // the same fate after the run-row janitor closed it (it
                // normally carries an error string; this arm is defense).
                TaskRunStatus::Running | TaskRunStatus::Abandoned => {
                    "interrupted before a clean exit".to_string()
                }
                _ => "(no detail recorded)".to_string(),
            },
        };
        // Name the verdict when there was one: "failed" and "rejected by the
        // reviewer" call for different next moves.
        match run.review_verdict {
            Some(crate::agents::swarm::tasks::ReviewVerdict::Rejected) => out.push_str(&format!(
                "- rejected by {}: {detail}\n",
                run.reviewer_id.as_deref().unwrap_or("the reviewer")
            )),
            _ => out.push_str(&format!("- {}: {detail}\n", run.status.as_str())),
        }
    }

    // Structured exit journal (the prior self's deliberate hand-off). One per
    // task, last-write-wins, so this is the freshest post-mortem available.
    if let Ok(Some(j)) = coord_store.get_task_journal(&task.id).await {
        out.push_str(&format!(
            "\n### Exit journal from a previous attempt (by `{}`)\n",
            j.agent_id
        ));
        out.push_str(&truncate_utf8(j.summary.trim(), MAX_SECTION_BYTES));
        out.push('\n');
        if !j.decisions.is_empty() {
            out.push_str("\nDecisions made:\n");
            for d in &j.decisions {
                out.push_str(&format!("- {}\n", truncate_utf8(d, MAX_SECTION_BYTES)));
            }
        }
        if !j.artifacts_ref.is_empty() {
            out.push_str("\nArtifacts / anchors to reuse:\n");
            for a in &j.artifacts_ref {
                out.push_str(&format!("- {}\n", truncate_utf8(a, MAX_SECTION_BYTES)));
            }
        }
        if !j.next_steps.is_empty() {
            out.push_str("\nRecommended next steps:\n");
            for n in &j.next_steps {
                out.push_str(&format!("- {}\n", truncate_utf8(n, MAX_SECTION_BYTES)));
            }
        }
        if let Some(c) = j.confidence {
            out.push_str(&format!("\nPrevious self-rated confidence: {c}/100\n"));
        }
    }

    out
}

/// Build the **notes** section from the task's comment thread.
///
/// `coord_task_comments` (written by the `task_comment` tool) carries leader /
/// teammate annotations on a task — clarifications, scope changes, review
/// feedback. Until now only the panel rendered them; the agent that actually
/// executes the task never saw them. Returns an empty string when the task has
/// no comments, so first-pass tasks are unaffected.
async fn build_notes_section(coord_store: &Arc<dyn CoordTaskStore>, task: &CoordTask) -> String {
    let comments = coord_store
        .list_task_comments(&task.id)
        .await
        .unwrap_or_default();
    if comments.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n## Notes\nLeader / teammate notes on this task:\n");
    for c in &comments {
        out.push_str(&format!(
            "- **{}**: {}\n",
            c.author,
            truncate_utf8(c.body.trim(), MAX_SECTION_BYTES)
        ));
    }
    out
}

/// The deliverable a completed dependency put on the edge.
///
/// `coord_tasks.result` is the primary channel (the dispatcher writes the
/// member's reply there). It is legitimately absent for a task that submitted
/// its work through `task_submit`: that tool flips the row to `WaitingReview`
/// mid-run, so the dispatcher's finalize fence classifies its own completion
/// write as a foreign transition and skips it — the deliverable then exists
/// ONLY as an artifact row. Falling back to the newest artifact is what makes
/// the documented "submit your deliverable, then the lead reviews it" contract
/// actually reach the next node instead of handing it an empty section.
async fn dependency_deliverable(
    artifact_store: Option<&Arc<dyn ArtifactStore>>,
    dep_id: &str,
    result: Option<&str>,
) -> Option<String> {
    if let Some(text) = result.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(text.to_string());
    }
    let store = artifact_store?;
    let artifacts = store.get_artifacts_for_task(dep_id).await.ok()?;
    // Newest wins: `get_artifacts_for_task` is creation-ordered, and a member
    // that submitted twice meant the later one.
    artifacts
        .into_iter()
        .rev()
        .find(|a| !a.content.trim().is_empty())
        .map(|a| format!("{}\n{}", a.title.trim(), a.content.trim()))
}

/// The upstream's structured exit journal, compacted for the edge.
///
/// The acceptance envelope tells every node to write one, but until now only a
/// *retrying* node read it (`build_recovery_section`) — a downstream node never
/// learned the artifact paths or caveats its upstream deliberately recorded.
/// Returns an empty string when there is no journal (envelope byte-identical).
/// `budget` is the journal's share of this dependency's slice, so the block
/// cannot escape the fan-in total (a 40-way fan-in where every member did what
/// the acceptance envelope asks would otherwise add ~100 KB of journals on top
/// of a capped 20 KB of deliverables).
async fn dependency_journal_block(
    coord_store: &Arc<dyn CoordTaskStore>,
    dep_id: &str,
    budget: usize,
) -> String {
    let Ok(Some(j)) = coord_store.get_task_journal(dep_id).await else {
        return String::new();
    };
    let mut out = String::new();
    let mut list = |heading: &str, items: &[String]| {
        if items.is_empty() || out.len() >= budget {
            return;
        }
        out.push_str(heading);
        for item in items.iter().take(MAX_DEP_JOURNAL_ITEMS) {
            if out.len() >= budget {
                out.push_str("- … (more omitted)\n");
                return;
            }
            out.push_str(&format!(
                "- {}\n",
                truncate_utf8(item, MAX_DEP_JOURNAL_ITEM_BYTES.min(budget))
            ));
        }
    };
    list("Artifacts / anchors it left:\n", &j.artifacts_ref);
    list("It flagged for whoever continues:\n", &j.next_steps);
    out
}

/// Render one dependency's fan-in section (heading + body), byte-capped to
/// `budget`. Pure formatting over already-fetched state — no judgment (R7).
async fn render_dependency(
    coord_store: &Arc<dyn CoordTaskStore>,
    artifact_store: Option<&Arc<dyn ArtifactStore>>,
    dep: &CoordTask,
    budget: usize,
) -> String {
    // The deliverable keeps the whole slice (so a dependency with no exit
    // journal renders exactly as it always did); the journal is an annotation
    // and gets a quarter of the slice on top, itself capped — so it scales
    // DOWN with the fan-in width instead of escaping the bound entirely.
    let journal_budget = (budget / 4).min(MAX_DEP_JOURNAL_BYTES);
    match dep.status {
        CoordTaskStatus::Completed => {
            match dependency_deliverable(artifact_store, &dep.id, dep.result.as_deref()).await {
                Some(body) => format!(
                    "### {}\n{}\n{}",
                    dep.subject,
                    truncate_utf8(&body, budget),
                    dependency_journal_block(coord_store, &dep.id, journal_budget).await
                ),
                // Completed with nothing on the edge used to render as an
                // empty heading — indistinguishable from "no section at all",
                // which is the exact ambiguity the Skipped marker exists to
                // kill. Name it instead of leaving silence.
                None => format!(
                    "### {}\n*(completed but recorded no output — treat this input as missing)*\n",
                    dep.subject
                ),
            }
        }
        // A skipped upstream also satisfies the dependency but has no output —
        // say so explicitly, or the member reads the silence as a missing input
        // and wastes a run hunting for it. A skip that WAIVED a failure is a
        // different fact and its error text is the evidence: announcing it as
        // "deliberately not run" tells the next node the opposite of the truth.
        CoordTaskStatus::Skipped => match dep
            .result
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(err) => format!(
                "### {}\n*(skipped after a failed attempt — its last recorded error follows; \
                 treat this input as unavailable, not as done)*\n{}\n",
                dep.subject,
                truncate_utf8(err, budget)
            ),
            None => format!(
                "### {}\n*(skipped — deliberately not run; no output to consume)*\n",
                dep.subject
            ),
        },
        // Any other status shouldn't occur for a task that unblocked us;
        // contribute nothing (matches the legacy envelope).
        _ => String::new(),
    }
}

/// Build the handoff context block for `task`.
///
/// Sections (each individually byte-capped): the task instruction, the team's
/// operating protocol (if the leader authored one), a recovery block when this
/// is a retry (prior run log + exit journal), leader/teammate notes, results of
/// any completed dependencies (the DAG fan-in channel), the team roster, and an
/// unread-inbox summary. The returned string is the complete `input` handed to
/// the member agent.
pub async fn build_handoff_context(
    coord_store: &Arc<dyn CoordTaskStore>,
    team_store: &Arc<dyn TeamStore>,
    artifact_store: Option<&Arc<dyn ArtifactStore>>,
    inbox_provider: Option<&dyn InboxContextProvider>,
    task: &CoordTask,
) -> String {
    let mut out = String::new();

    // --- Task instruction ---
    out.push_str("## Task\n");
    out.push_str(&truncate_utf8(&task.subject, MAX_SECTION_BYTES));
    if !task.description.is_empty() {
        out.push('\n');
        out.push_str(&truncate_utf8(&task.description, MAX_SECTION_BYTES));
    }
    out.push('\n');

    // --- Global Strategy (workflow run-global frame) ---
    // Stamped by `workflow::materialize` under `WORKFLOW_STRATEGY_KEY`: the
    // run-global objective + cross-cutting guardrails (no phases — the DAG is
    // the phase structure). Placed AFTER the task so the per-node task
    // description stays the authoritative local instruction; this is context.
    // Absent for plain team tasks and pre-strategy runs (byte-identical).
    if let Some(frame) = task
        .metadata
        .get(crate::workflow::WORKFLOW_STRATEGY_KEY)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        out.push_str("\n## Global Strategy (context — your specific task is below)\n");
        out.push_str(&truncate_utf8(frame, MAX_SECTION_BYTES));
        out.push('\n');
    }

    // --- Acceptance criteria (the task's definition of done) ---
    // Read straight from the metadata channel and rendered as a checklist so
    // the member knows the completion bar before starting. Empty for tasks that
    // declare none, leaving the envelope byte-identical to the legacy one.
    out.push_str(&render_acceptance_section(&read_acceptance_criteria(
        &task.metadata,
    )));

    // --- Review notice (review-gated steps only) ---
    // The member should know its output goes to a reviewer, not straight to
    // done: self-reports are judged, so claims need verifiable evidence.
    // Empty for non-gated tasks (envelope byte-identical to before).
    if lead_review_required(&task.metadata) {
        out.push_str(
            "\n## Review Gate\n\
             Your result will be reviewed by the team lead before this step \
             counts as complete. Make the reviewer's job easy: state exactly \
             what you did, and back every externally-visible claim (files \
             written, requests sent, things published) with a verifiable \
             handle — a path, URL, id, or status code — rather than an \
             unsupported assertion.\n",
        );
    }

    // --- Team protocol (operating agreement injected verbatim) ---
    // Placed right after the task so the member reads the team's rules before
    // diving into dependency outputs. Fetched only when the task belongs to a
    // team; a missing/blank protocol contributes nothing (no empty heading).
    if let Some(team_id) = &task.team_id {
        if let Ok(Some(team)) = team_store.get_team(team_id).await {
            if let Some(proto) = team.protocol.as_deref() {
                let proto = proto.trim();
                if !proto.is_empty() {
                    out.push_str("\n## Team Protocol\n");
                    out.push_str(&truncate_utf8(proto, MAX_PROTOCOL_BYTES));
                    out.push('\n');
                }
            }
        }
    }

    // --- Recovery context (retry-only: prior run log + exit journal) ---
    // Placed before dependency results so a resuming member first learns that
    // it is resuming, and why, before re-reading upstream outputs.
    out.push_str(&build_recovery_section(coord_store, task).await);

    // --- Leader / teammate notes (task comment thread) ---
    out.push_str(&build_notes_section(coord_store, task).await);

    // --- Dependency results (fan-in from completed upstream tasks) ---
    // `task.dependencies` now arrives in the order the template declared
    // (both store readers order by the dependency row's rowid), so render it
    // as given: a three-way fan-in reaches the synthesis node in the author's
    // order, and re-running the same template does not reshuffle it.
    let mut fetched: Vec<CoordTask> = Vec::with_capacity(task.dependencies.len());
    let mut unreadable = String::new();
    for dep_id in &task.dependencies {
        match coord_store.get_task(dep_id).await {
            Ok(Some(dep)) => fetched.push(dep),
            Ok(None) => {
                tracing::warn!(task_id = %task.id, dep_id = %dep_id, "Handoff: dependency task not found");
                unreadable.push_str(&format!(
                    "### Dependency `{dep_id}`\n*(missing from store)*\n"
                ));
            }
            Err(e) => {
                tracing::warn!(task_id = %task.id, dep_id = %dep_id, error = %e, "Handoff: failed to fetch dependency");
                unreadable.push_str(&format!(
                    "### Dependency `{dep_id}`\n*(fetch error: {e})*\n"
                ));
            }
        }
    }
    let budget = dep_budget(task.dependencies.len());
    let mut dep_section = String::new();
    for dep in &fetched {
        dep_section.push_str(&render_dependency(coord_store, artifact_store, dep, budget).await);
    }
    dep_section.push_str(&unreadable);
    if !dep_section.is_empty() {
        out.push_str("\n## Dependency Results\n");
        out.push_str(&dep_section);
    }

    // --- Team roster ---
    if let Some(team_id) = &task.team_id {
        if let Ok(members) = team_store.get_members(team_id).await {
            if !members.is_empty() {
                out.push_str("\n## Team\n");
                if let Some(owner) = &task.owner {
                    out.push_str(&format!("You are agent `{owner}` on team `{team_id}`.\n"));
                }
                out.push_str("Members:\n");
                for m in &members {
                    if m.role.is_empty() {
                        out.push_str(&format!("- {}\n", m.agent_id));
                    } else {
                        out.push_str(&format!("- {} ({})\n", m.agent_id, m.role));
                    }
                }
            }
        }
    }

    // --- Unread inbox summary ---
    if let (Some(provider), Some(owner)) = (inbox_provider, &task.owner) {
        let ctx = provider.get_inbox_context(owner).await;
        if let Some(text) = ctx.to_injection_text() {
            out.push_str("\n## Inbox\n");
            out.push_str(&text);
            out.push('\n');
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::swarm::tasks::store::SqliteCoordTaskStore;
    use crate::agents::swarm::tasks::{
        CoordTaskUpdate, NewCoordTask, NewTaskExitJournal, Priority,
    };
    use crate::teams::store::SqliteTeamStore;
    use crate::teams::types::{NewTeam, NewTeamMember};
    use rusqlite::Connection;

    async fn coord_store() -> Arc<dyn CoordTaskStore> {
        let store = SqliteCoordTaskStore::new(Connection::open_in_memory().unwrap());
        store.migrate().await.unwrap();
        Arc::new(store)
    }

    async fn team_store() -> Arc<dyn TeamStore> {
        let store = SqliteTeamStore::new(Connection::open_in_memory().unwrap());
        store.migrate().await.unwrap();
        Arc::new(store)
    }

    fn plain_task(subject: &str) -> NewCoordTask {
        NewCoordTask {
            team_id: None,
            subject: subject.into(),
            description: String::new(),
            owner: Some("worker".into()),
            priority: Priority::Normal,
            blocked_by: vec![],
            metadata: serde_json::json!({}),
        }
    }

    async fn artifact_store() -> Arc<dyn ArtifactStore> {
        let store = crate::teams::artifacts::SqliteArtifactStore::new(
            Connection::open_in_memory().unwrap(),
        );
        store.migrate().await.unwrap();
        Arc::new(store)
    }

    /// The `task_submit` shape end to end: the member puts its deliverable in
    /// the artifact store, the tool flips the row to WaitingReview mid-run (so
    /// the dispatcher's finalize fence never writes `result`), the lead
    /// approves, and the DOWNSTREAM node must still receive the work.
    ///
    /// Delete the artifact-fallback branch and this test fails — which is the
    /// point: the previous suite passed `artifact_store = None` everywhere, so
    /// the whole branch was unreachable from any test.
    #[tokio::test]
    async fn submitted_artifact_crosses_the_edge_when_result_is_empty() {
        let cs = coord_store().await;
        let ts = team_store().await;
        let arts = artifact_store().await;

        let upstream = cs.create_task(plain_task("analyse")).await.unwrap();
        // What `task_submit` does: artifact row, then WaitingReview.
        arts.create_artifact(crate::teams::artifacts::NewArtifact {
            task_id: upstream.id.clone(),
            agent_id: "worker".into(),
            artifact_type: crate::teams::artifacts::ArtifactType::Report,
            title: "Q2 analysis".into(),
            content: "revenue is up 12% on EU rows".into(),
            metadata: serde_json::Value::Null,
            status: crate::teams::artifacts::TaskStatus::Pending,
            blocked_by: vec![],
            assignee: None,
            priority: 0,
        })
        .await
        .unwrap();
        // ...and what the lead's approve does: Completed, `result` untouched.
        cs.update_task(
            &upstream.id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::Completed),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let mut down = plain_task("write report");
        down.blocked_by = vec![upstream.id.clone()];
        let down = cs.create_task(down).await.unwrap();

        // Without the artifact store the edge is empty and says so...
        let blind = build_handoff_context(&cs, &ts, None, None, &down).await;
        assert!(
            blind.contains("completed but recorded no output"),
            "an empty edge must be named, not silent: {blind}"
        );
        // ...with it, the deliverable actually reaches the next node.
        let ctx = build_handoff_context(&cs, &ts, Some(&arts), None, &down).await;
        assert!(
            ctx.contains("revenue is up 12% on EU rows"),
            "the submitted artifact must cross the edge: {ctx}"
        );
    }

    /// A three-way fan-in must present every upstream, in the order the
    /// template declared, each under its own heading.
    #[tokio::test]
    async fn three_way_fan_in_keeps_every_dependency_in_declared_order() {
        let cs = coord_store().await;
        let ts = team_store().await;
        let mut ids = Vec::new();
        for subject in ["gather_market", "gather_legal", "gather_finance"] {
            let t = cs.create_task(plain_task(subject)).await.unwrap();
            cs.update_task(
                &t.id,
                CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Completed),
                    result: Some(format!("{subject} findings")),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            ids.push(t.id);
        }
        let mut synth = plain_task("synthesise");
        // Declared order is deliberately NOT the creation order of the ids as
        // sorted text — it is the order written here.
        synth.blocked_by = vec![ids[2].clone(), ids[0].clone(), ids[1].clone()];
        let synth = cs.create_task(synth).await.unwrap();
        let reread = cs.get_task(&synth.id).await.unwrap().unwrap();

        let ctx = build_handoff_context(&cs, &ts, None, None, &reread).await;
        let pos = |needle: &str| {
            ctx.find(needle)
                .unwrap_or_else(|| panic!("missing {needle}: {ctx}"))
        };
        assert!(pos("gather_finance findings") < pos("gather_market findings"));
        assert!(pos("gather_market findings") < pos("gather_legal findings"));
    }

    /// A skip that WAIVED a failure is a different fact from a skip that was
    /// planned — announcing the first as "deliberately not run" tells the next
    /// node the opposite of the truth and throws away the error evidence.
    #[tokio::test]
    async fn skipped_after_failure_carries_the_error_not_a_planned_skip_notice() {
        let cs = coord_store().await;
        let ts = team_store().await;
        let up = cs.create_task(plain_task("fetch_pricing")).await.unwrap();
        cs.update_task(
            &up.id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::Failed),
                result: Some("Timed out after 600 seconds".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        cs.update_task(
            &up.id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::Skipped),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let mut down = plain_task("write_report");
        down.blocked_by = vec![up.id.clone()];
        let down = cs.create_task(down).await.unwrap();

        let ctx = build_handoff_context(&cs, &ts, None, None, &down).await;
        assert!(ctx.contains("Timed out after 600 seconds"), "{ctx}");
        assert!(
            !ctx.contains("deliberately not run"),
            "a waived failure is not a planned skip: {ctx}"
        );
    }

    /// The exit journal is the structured hand-off the acceptance envelope asks
    /// every node to write; until now only a RETRY of the same node read it.
    #[tokio::test]
    async fn dependency_exit_journal_crosses_the_edge() {
        let cs = coord_store().await;
        let ts = team_store().await;
        let up = cs.create_task(plain_task("research")).await.unwrap();
        cs.update_task(
            &up.id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::Completed),
                result: Some("two sentences of summary".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        cs.upsert_task_journal(NewTaskExitJournal {
            task_id: up.id.clone(),
            agent_id: "worker".into(),
            summary: "done".into(),
            decisions: vec![],
            artifacts_ref: vec!["reports/q2-pricing.csv".into()],
            next_steps: vec!["the EU rows are estimates, re-check before publishing".into()],
            confidence: Some(70),
        })
        .await
        .unwrap();

        let mut down = plain_task("write_report");
        down.blocked_by = vec![up.id.clone()];
        let down = cs.create_task(down).await.unwrap();

        let ctx = build_handoff_context(&cs, &ts, None, None, &down).await;
        assert!(ctx.contains("reports/q2-pricing.csv"), "{ctx}");
        assert!(ctx.contains("re-check before publishing"), "{ctx}");
    }

    #[test]
    fn dep_budget_keeps_narrow_fan_in_byte_identical_and_bounds_wide_ones() {
        assert_eq!(dep_budget(1), MAX_SECTION_BYTES);
        assert_eq!(dep_budget(6), MAX_SECTION_BYTES, "<=6 unchanged");
        let wide = dep_budget(40);
        assert!(wide >= MIN_DEP_BYTES);
        assert!(
            wide * 40 <= MAX_DEP_SECTION_TOTAL_BYTES + MIN_DEP_BYTES * 40,
            "a 40-way fan-in must not scale linearly at the per-dep ceiling"
        );
    }

    #[tokio::test]
    async fn no_recovery_or_notes_on_first_attempt() {
        let cs = coord_store().await;
        let ts = team_store().await;
        let task = cs.create_task(plain_task("Fresh task")).await.unwrap();

        let ctx = build_handoff_context(&cs, &ts, None, None, &task).await;
        // A first-pass task with no run history / comments is byte-identical to
        // the legacy envelope — neither additive section appears.
        assert!(!ctx.contains("## Recovery Context"));
        assert!(!ctx.contains("## Notes"));
    }

    #[tokio::test]
    async fn recovery_section_surfaces_prior_runs_and_journal() {
        let cs = coord_store().await;
        let ts = team_store().await;
        let task = cs
            .create_task(plain_task("Migrate the parser"))
            .await
            .unwrap();

        // Simulate a failed first attempt: a finished run + an exit journal the
        // prior self wrote via the task_exit_journal tool.
        let run_id = cs.start_task_run(&task.id, "worker").await.unwrap();
        cs.finish_task_run(
            &run_id,
            TaskRunStatus::Failed,
            None,
            Some("panic: index out of bounds in lexer".into()),
        )
        .await
        .unwrap();
        cs.upsert_task_journal(NewTaskExitJournal {
            task_id: task.id.clone(),
            agent_id: "worker".into(),
            summary: "Rewrote the tokenizer; the AST visitor still needs porting.".into(),
            decisions: vec!["Kept the old token enum for compatibility".into()],
            artifacts_ref: vec!["src/parser/lexer.rs".into()],
            next_steps: vec!["Port visit_expr in ast/visitor.rs".into()],
            confidence: Some(60),
        })
        .await
        .unwrap();

        let ctx = build_handoff_context(&cs, &ts, None, None, &task).await;
        assert!(ctx.contains("## Recovery Context"));
        assert!(ctx.contains("This is attempt 2"));
        assert!(ctx.contains("panic: index out of bounds"));
        assert!(ctx.contains("Rewrote the tokenizer"));
        assert!(ctx.contains("src/parser/lexer.rs"));
        assert!(ctx.contains("Port visit_expr"));
        assert!(ctx.contains("60/100"));
    }

    #[tokio::test]
    async fn notes_section_injects_task_comments() {
        let cs = coord_store().await;
        let ts = team_store().await;
        let task = cs
            .create_task(plain_task("Build the report"))
            .await
            .unwrap();

        cs.add_task_comment(&task.id, "lead", "Use the Q2 numbers, not Q1.")
            .await
            .unwrap();

        let ctx = build_handoff_context(&cs, &ts, None, None, &task).await;
        assert!(ctx.contains("## Notes"));
        assert!(ctx.contains("**lead**"));
        assert!(ctx.contains("Use the Q2 numbers, not Q1."));
    }

    #[tokio::test]
    async fn handoff_includes_task_and_dependency_results() {
        let cs = coord_store().await;
        let ts = team_store().await;

        // Dependency task, completed with a result.
        let dep = cs
            .create_task(NewCoordTask {
                team_id: None,
                subject: "Gather data".into(),
                description: String::new(),
                owner: None,
                priority: Priority::Normal,
                blocked_by: vec![],
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();
        cs.update_task(
            &dep.id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::Completed),
                result: Some("found 42 records".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        // Downstream task depending on it.
        let task = cs
            .create_task(NewCoordTask {
                team_id: None,
                subject: "Analyze data".into(),
                description: "Produce a summary".into(),
                owner: Some("analyst".into()),
                priority: Priority::Normal,
                blocked_by: vec![dep.id.clone()],
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();

        let ctx = build_handoff_context(&cs, &ts, None, None, &task).await;
        assert!(ctx.contains("## Task"));
        assert!(ctx.contains("Analyze data"));
        assert!(ctx.contains("Produce a summary"));
        assert!(ctx.contains("## Dependency Results"));
        assert!(ctx.contains("Gather data"));
        assert!(ctx.contains("found 42 records"));
    }

    #[tokio::test]
    async fn handoff_notes_skipped_dependency_explicitly() {
        let cs = coord_store().await;
        let ts = team_store().await;

        let dep = cs
            .create_task(plain_task("Optional research"))
            .await
            .unwrap();
        cs.update_task(
            &dep.id,
            CoordTaskUpdate {
                status: Some(CoordTaskStatus::Skipped),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let task = cs
            .create_task(NewCoordTask {
                team_id: None,
                subject: "Write summary".into(),
                description: String::new(),
                owner: Some("writer".into()),
                priority: Priority::Normal,
                blocked_by: vec![dep.id.clone()],
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();

        let ctx = build_handoff_context(&cs, &ts, None, None, &task).await;
        // The skipped upstream is named with an explicit no-output note, so
        // the member doesn't read the silence as a missing input.
        assert!(ctx.contains("## Dependency Results"));
        assert!(ctx.contains("Optional research"));
        assert!(ctx.contains("skipped — deliberately not run"));
    }

    #[tokio::test]
    async fn handoff_injects_acceptance_criteria_when_present() {
        use crate::agents::swarm::tasks::acceptance::with_acceptance_criteria;
        let cs = coord_store().await;
        let ts = team_store().await;

        // No criteria -> no Acceptance Criteria heading (byte-identical legacy).
        let plain = cs.create_task(plain_task("Bare task")).await.unwrap();
        let before = build_handoff_context(&cs, &ts, None, None, &plain).await;
        assert!(!before.contains("## Acceptance Criteria"));

        // A task carrying criteria in its metadata surfaces them as a checklist.
        let mut spec = plain_task("Ship login");
        spec.metadata = with_acceptance_criteria(
            spec.metadata,
            vec!["tests pass".into(), "no clippy warnings".into()],
        );
        let task = cs.create_task(spec).await.unwrap();
        let after = build_handoff_context(&cs, &ts, None, None, &task).await;
        assert!(after.contains("## Acceptance Criteria"));
        assert!(after.contains("- [ ] tests pass"));
        assert!(after.contains("- [ ] no clippy warnings"));
    }

    #[tokio::test]
    async fn handoff_injects_review_notice_only_when_gated() {
        use crate::agents::swarm::tasks::acceptance::with_lead_review_required;
        let cs = coord_store().await;
        let ts = team_store().await;

        // No flag -> no Review Gate heading (byte-identical legacy envelope).
        let plain = cs.create_task(plain_task("Bare task")).await.unwrap();
        let before = build_handoff_context(&cs, &ts, None, None, &plain).await;
        assert!(!before.contains("## Review Gate"));

        // A review-gated task tells the member its output will be judged.
        let mut spec = plain_task("Ship login");
        spec.metadata = with_lead_review_required(spec.metadata, true);
        let task = cs.create_task(spec).await.unwrap();
        let after = build_handoff_context(&cs, &ts, None, None, &task).await;
        assert!(after.contains("## Review Gate"));
        assert!(after.contains("verifiable handle"));
    }

    #[tokio::test]
    async fn handoff_includes_team_roster() {
        let cs = coord_store().await;
        let ts = team_store().await;

        let team = ts
            .create_team(NewTeam {
                name: "Research".into(),
                description: String::new(),
                leader_id: "lead".into(),
            })
            .await
            .unwrap();
        ts.add_member(NewTeamMember {
            team_id: team.id.clone(),
            agent_id: "analyst".into(),
            role: "data analyst".into(),
            ..Default::default()
        })
        .await
        .unwrap();

        let task = cs
            .create_task(NewCoordTask {
                team_id: Some(team.id.clone()),
                subject: "Do work".into(),
                description: String::new(),
                owner: Some("analyst".into()),
                priority: Priority::Normal,
                blocked_by: vec![],
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();

        let ctx = build_handoff_context(&cs, &ts, None, None, &task).await;
        assert!(ctx.contains("## Team"));
        assert!(ctx.contains("You are agent `analyst`"));
        assert!(ctx.contains("data analyst"));
    }

    #[tokio::test]
    async fn handoff_injects_team_protocol_when_set() {
        let cs = coord_store().await;
        let ts = team_store().await;

        let team = ts
            .create_team(NewTeam {
                name: "Squad".into(),
                description: String::new(),
                leader_id: "lead".into(),
            })
            .await
            .unwrap();

        let task = cs
            .create_task(NewCoordTask {
                team_id: Some(team.id.clone()),
                subject: "Ship it".into(),
                description: String::new(),
                owner: Some("worker".into()),
                priority: Priority::Normal,
                blocked_by: vec![],
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();

        // No protocol set -> no protocol heading.
        let before = build_handoff_context(&cs, &ts, None, None, &task).await;
        assert!(!before.contains("## Team Protocol"));

        // After setting a protocol, it appears verbatim.
        ts.set_protocol(&team.id, Some("Always write tests first.".into()))
            .await
            .unwrap();
        let after = build_handoff_context(&cs, &ts, None, None, &task).await;
        assert!(after.contains("## Team Protocol"));
        assert!(after.contains("Always write tests first."));
    }

    #[test]
    fn truncate_respects_char_boundary() {
        let s = "héllo wörld";
        let t = truncate_utf8(s, 3);
        assert!(t.starts_with('h'));
        assert!(t.ends_with("(truncated)"));
        // Must not panic on a multi-byte boundary.
        let _ = truncate_utf8(s, 2);
    }

    #[tokio::test]
    async fn global_strategy_section_renders_after_task_when_stamped() {
        let cs = coord_store().await;
        let ts = team_store().await;
        let mut nt = plain_task("Implement the parser");
        nt.description = "Write the recursive-descent parser for the grammar.".into();
        nt.metadata = serde_json::json!({
            crate::workflow::WORKFLOW_STRATEGY_KEY:
                "Objective: ship a correct parser.\nGuardrails:\n- no panics on malformed input",
        });
        let task = cs.create_task(nt).await.unwrap();

        let ctx = build_handoff_context(&cs, &ts, None, None, &task).await;

        // Labeled global-frame block is present...
        assert!(ctx.contains("## Global Strategy (context — your specific task is below)"));
        assert!(ctx.contains("ship a correct parser."));
        // ...and it comes AFTER the ## Task block (task stays authoritative).
        let task_pos = ctx.find("## Task").unwrap();
        let strat_pos = ctx.find("## Global Strategy").unwrap();
        assert!(task_pos < strat_pos, "strategy must follow the task block");
        // The per-node description is present and precedes the strategy.
        let desc_pos = ctx.find("recursive-descent parser").unwrap();
        assert!(desc_pos < strat_pos);
    }

    #[tokio::test]
    async fn no_global_strategy_section_when_metadata_absent() {
        let cs = coord_store().await;
        let ts = team_store().await;
        let task = cs.create_task(plain_task("Plain task")).await.unwrap();

        let ctx = build_handoff_context(&cs, &ts, None, None, &task).await;
        // Byte-identical to the legacy envelope: no global-strategy heading.
        assert!(!ctx.contains("## Global Strategy"));
    }
}
