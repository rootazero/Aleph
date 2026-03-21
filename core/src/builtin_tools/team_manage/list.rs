//! TeamListTool — list all coordination teams with status summaries.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::agents::swarm::tasks::{
    CoordTaskFilter, CoordTaskStatus, CoordTaskStore, Team, TeamFilter, TeamStatus,
};
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for listing teams (currently no filters).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TeamListArgs {}

/// A single team entry in the list output.
#[derive(Debug, Clone, Serialize)]
pub struct TeamListEntry {
    pub id: String,
    pub name: String,
    pub status: String,
    pub leader: String,
    pub member_count: usize,
    /// Total task count (only for active teams)
    pub task_count: Option<usize>,
    /// Completed task count (only for active teams)
    pub completed_count: Option<usize>,
}

/// Output from team listing.
#[derive(Debug, Clone, Serialize)]
pub struct TeamListOutput {
    pub teams: Vec<TeamListEntry>,
    pub total: usize,
    pub summary: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that lists all coordination teams.
#[derive(Clone)]
pub struct TeamListTool {
    store: Arc<dyn CoordTaskStore>,
}

impl TeamListTool {
    pub fn new(store: Arc<dyn CoordTaskStore>) -> Self {
        Self { store }
    }
}

fn format_team_board(teams: &[Team], task_stats: &[(usize, usize)]) -> String {
    if teams.is_empty() {
        return "No teams found.".to_string();
    }

    let mut lines = Vec::new();
    lines.push(format!("Teams ({}):", teams.len()));
    lines.push("\u{2500}".repeat(60));

    for (team, stats) in teams.iter().zip(task_stats.iter()) {
        let status_icon = match team.status {
            TeamStatus::Active => "\u{25c9}",    // ◉
            TeamStatus::Completed => "\u{25cf}",  // ●
            TeamStatus::Disbanded => "\u{2298}",  // ⊘
        };

        let task_info = if team.status == TeamStatus::Active {
            format!(" [{}/{} tasks done]", stats.1, stats.0)
        } else {
            String::new()
        };

        lines.push(format!(
            "{} {} [{}] — {} (leader: {}, {} members){}",
            status_icon,
            team.id,
            team.status.as_str(),
            team.name,
            team.leader,
            team.members.len(),
            task_info,
        ));
    }

    lines.join("\n")
}

#[async_trait]
impl AlephTool for TeamListTool {
    const NAME: &'static str = "team_list";
    const DESCRIPTION: &'static str =
        "List all coordination teams with their status, leader, member count, \
         and task progress for active teams.";

    type Args = TeamListArgs;
    type Output = TeamListOutput;

    fn examples(&self) -> Option<Vec<String>> {
        None
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output> {
        let teams = self.store.list_teams(TeamFilter {}).await?;

        // Gather task stats for each team
        let mut task_stats: Vec<(usize, usize)> = Vec::new();
        for team in &teams {
            if team.status == TeamStatus::Active {
                let tasks = self
                    .store
                    .list_tasks(CoordTaskFilter {
                        team_id: Some(team.id.clone()),
                        ..Default::default()
                    })
                    .await
                    .unwrap_or_default();

                let total = tasks.len();
                let completed = tasks
                    .iter()
                    .filter(|t| t.status == CoordTaskStatus::Completed)
                    .count();
                task_stats.push((total, completed));
            } else {
                task_stats.push((0, 0));
            }
        }

        let summary = format_team_board(&teams, &task_stats);
        let total = teams.len();

        let entries = teams
            .iter()
            .zip(task_stats.iter())
            .map(|(t, stats)| {
                let (task_count, completed_count) = if t.status == TeamStatus::Active {
                    (Some(stats.0), Some(stats.1))
                } else {
                    (None, None)
                };

                TeamListEntry {
                    id: t.id.clone(),
                    name: t.name.clone(),
                    status: t.status.as_str().to_string(),
                    leader: t.leader.clone(),
                    member_count: t.members.len(),
                    task_count,
                    completed_count,
                }
            })
            .collect();

        Ok(TeamListOutput {
            teams: entries,
            total,
            summary,
        })
    }
}
