//! Handoff context builder.
//!
//! Assembles the deterministic context envelope injected into a member agent
//! when the dispatcher launches it for a task. Inspired by the "worker
//! context" pattern: instead of a background sensing loop, everything the
//! member needs is gathered once, at launch, from the task DAG and team state.

use crate::agents::swarm::tasks::{CoordTask, CoordTaskStatus, CoordTaskStore};
use crate::sync_primitives::Arc;
use crate::teams::context::InboxContextProvider;
use crate::teams::store::TeamStore;

/// Max bytes kept per free-form section (task body, each dependency result).
const MAX_SECTION_BYTES: usize = 4096;

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

/// Build the handoff context block for `task`.
///
/// Sections (each individually byte-capped): the task instruction, results of
/// any completed dependencies (the DAG fan-in channel), the team roster, and
/// an unread-inbox summary. The returned string is the complete `input` handed
/// to the member agent.
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

    // --- Dependency results (fan-in from completed upstream tasks) ---
    let mut dep_section = String::new();
    for dep_id in &task.dependencies {
        if let Ok(Some(dep)) = coord_store.get_task(dep_id).await {
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
    use crate::agents::swarm::tasks::{CoordTaskUpdate, NewCoordTask, Priority};
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
