//! Handoff context builder.
//!
//! Assembles the deterministic context envelope injected into a member agent
//! when the dispatcher launches it for a task. Inspired by the "worker
//! context" pattern: instead of a background sensing loop, everything the
//! member needs is gathered once, at launch, from the task DAG and team state.

use crate::agents::swarm::tasks::acceptance::{
    read_acceptance_criteria, render_acceptance_section,
};
use crate::agents::swarm::tasks::{CoordTask, CoordTaskStatus, CoordTaskStore, TaskRunStatus};
use crate::sync_primitives::Arc;
use crate::teams::context::InboxContextProvider;
use crate::teams::store::TeamStore;

/// Max bytes kept per free-form section (task body, each dependency result).
const MAX_SECTION_BYTES: usize = 4096;

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
/// On a retry — after a zombie/orphan reclaim or a leader-driven
/// `Failed`/`Cancelled` → `Pending` reset — it surfaces the durable hand-off
/// that earlier attempts produced but which, until now, only the panel ever
/// read: the per-attempt run log (`coord_task_runs`) and the structured exit
/// journal (`coord_task_journals`, written by the `task_exit_journal` tool).
///
/// This closes the write→read loop on ClawTeam's context-recovery pattern: the
/// resuming agent reads what its previous self learned instead of cold-starting.
/// Pure context assembly — the dumb loop performs no recovery *decision* (that
/// stays the leader's, per R7/R10); it only hands the member what already exists.
async fn build_recovery_section(coord_store: &Arc<dyn CoordTaskStore>, task: &CoordTask) -> String {
    // `build_handoff_context` runs before `start_task_run`, so every row here
    // belongs to an *earlier* attempt — the current claim is not yet recorded.
    let runs = coord_store.list_task_runs(&task.id).await.unwrap_or_default();
    if runs.is_empty() {
        return String::new();
    }

    let attempt = runs.len() + 1;
    let mut out = format!(
        "\n## Recovery Context\nThis is attempt {attempt}. {} previous attempt(s) did not \
         complete — resume from where they left off; reuse work already done and do not \
         restart from scratch.\n",
        runs.len()
    );

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
                // — the process died mid-task and was reclaimed.
                TaskRunStatus::Running => "interrupted before a clean exit".to_string(),
                _ => "(no detail recorded)".to_string(),
            },
        };
        out.push_str(&format!("- {}: {detail}\n", run.status.as_str()));
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

    // --- Acceptance criteria (the task's definition of done) ---
    // Read straight from the metadata channel and rendered as a checklist so
    // the member knows the completion bar before starting. Empty for tasks that
    // declare none, leaving the envelope byte-identical to the legacy one.
    out.push_str(&render_acceptance_section(&read_acceptance_criteria(
        &task.metadata,
    )));

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
    let mut dep_section = String::new();
    for dep_id in &task.dependencies {
        match coord_store.get_task(dep_id).await {
            Ok(Some(dep)) => {
                if dep.status == CoordTaskStatus::Completed {
                    if let Some(result) = &dep.result {
                        dep_section.push_str(&format!(
                            "### {}\n{}\n",
                            dep.subject,
                            truncate_utf8(result, MAX_SECTION_BYTES)
                        ));
                    }
                }
            }
            Ok(None) => {
                tracing::warn!(task_id = %task.id, dep_id = %dep_id, "Handoff: dependency task not found");
                dep_section.push_str(&format!(
                    "### Dependency `{}`\n*(missing from store)*\n",
                    dep_id
                ));
            }
            Err(e) => {
                tracing::warn!(task_id = %task.id, dep_id = %dep_id, error = %e, "Handoff: failed to fetch dependency");
                dep_section.push_str(&format!(
                    "### Dependency `{}`\n*(fetch error: {})*\n",
                    dep_id, e
                ));
            }
        }
    }
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

    #[tokio::test]
    async fn no_recovery_or_notes_on_first_attempt() {
        let cs = coord_store().await;
        let ts = team_store().await;
        let task = cs.create_task(plain_task("Fresh task")).await.unwrap();

        let ctx = build_handoff_context(&cs, &ts, None, &task).await;
        // A first-pass task with no run history / comments is byte-identical to
        // the legacy envelope — neither additive section appears.
        assert!(!ctx.contains("## Recovery Context"));
        assert!(!ctx.contains("## Notes"));
    }

    #[tokio::test]
    async fn recovery_section_surfaces_prior_runs_and_journal() {
        let cs = coord_store().await;
        let ts = team_store().await;
        let task = cs.create_task(plain_task("Migrate the parser")).await.unwrap();

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

        let ctx = build_handoff_context(&cs, &ts, None, &task).await;
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
        let task = cs.create_task(plain_task("Build the report")).await.unwrap();

        cs.add_task_comment(&task.id, "lead", "Use the Q2 numbers, not Q1.")
            .await
            .unwrap();

        let ctx = build_handoff_context(&cs, &ts, None, &task).await;
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

        let ctx = build_handoff_context(&cs, &ts, None, &task).await;
        assert!(ctx.contains("## Task"));
        assert!(ctx.contains("Analyze data"));
        assert!(ctx.contains("Produce a summary"));
        assert!(ctx.contains("## Dependency Results"));
        assert!(ctx.contains("Gather data"));
        assert!(ctx.contains("found 42 records"));
    }

    #[tokio::test]
    async fn handoff_injects_acceptance_criteria_when_present() {
        use crate::agents::swarm::tasks::acceptance::with_acceptance_criteria;
        let cs = coord_store().await;
        let ts = team_store().await;

        // No criteria -> no Acceptance Criteria heading (byte-identical legacy).
        let plain = cs.create_task(plain_task("Bare task")).await.unwrap();
        let before = build_handoff_context(&cs, &ts, None, &plain).await;
        assert!(!before.contains("## Acceptance Criteria"));

        // A task carrying criteria in its metadata surfaces them as a checklist.
        let mut spec = plain_task("Ship login");
        spec.metadata = with_acceptance_criteria(
            spec.metadata,
            vec!["tests pass".into(), "no clippy warnings".into()],
        );
        let task = cs.create_task(spec).await.unwrap();
        let after = build_handoff_context(&cs, &ts, None, &task).await;
        assert!(after.contains("## Acceptance Criteria"));
        assert!(after.contains("- [ ] tests pass"));
        assert!(after.contains("- [ ] no clippy warnings"));
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

        let ctx = build_handoff_context(&cs, &ts, None, &task).await;
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
        let before = build_handoff_context(&cs, &ts, None, &task).await;
        assert!(!before.contains("## Team Protocol"));

        // After setting a protocol, it appears verbatim.
        ts.set_protocol(&team.id, Some("Always write tests first.".into()))
            .await
            .unwrap();
        let after = build_handoff_context(&cs, &ts, None, &task).await;
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
}
