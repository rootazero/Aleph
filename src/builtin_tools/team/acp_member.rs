//! `TeamAcpMemberTool` — manage external coding CLI sessions (Claude Code,
//! Codex, ...) as team members.
//!
//! Implements R8 (Everything is a Tool) for ACP-backed members: the leader
//! agent can attach a worker via natural language ("add a Claude Code
//! worker to my team in /work/repo") and the autonomous dispatcher will
//! route subsequent tasks through the ACP adapter pool.
//!
//! Single tool with an `action` discriminator covering add/remove/list —
//! mirrors the `team_snapshot` shape so the LLM only learns one verb per
//! domain.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::teams::{NewTeamMember, TeamMember, TeamMemberKind, TeamStore};
use crate::tools::AlephTool;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum TeamAcpMemberArgs {
    /// Attach an ACP harness session as a team member. Idempotent.
    Add {
        team_id: String,
        /// Adapter id registered in `[acp.adapters]` — e.g. `claude-code`,
        /// `codex`, `gemini_cli`.
        harness_id: String,
        /// Working directory the external CLI process should run in. Use an
        /// absolute path. The team dispatcher passes this through to
        /// `AcpAdapterManager::prompt_named`.
        cwd: String,
        /// Optional pool key — supply when keeping multiple parallel
        /// sessions in the same repo (e.g. `backend`, `frontend`).
        #[serde(default)]
        session_name: Option<String>,
        /// Free-text role label (e.g. `code-reviewer`, `bug-hunter`). Used
        /// for prompt building and digests.
        #[serde(default = "default_role")]
        role: String,
    },
    /// Detach a previously-added ACP member. Does NOT kill the live session
    /// in the adapter pool — use `acp.sessions.shutdown` for that.
    Remove { team_id: String, agent_id: String },
    /// Return only ACP-backed members of `team_id`. Pure read.
    List { team_id: String },
}

fn default_role() -> String {
    "acp-worker".to_string()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum TeamAcpMemberOutput {
    Add {
        member: TeamMember,
    },
    Remove {
        team_id: String,
        agent_id: String,
    },
    List {
        team_id: String,
        members: Vec<TeamMember>,
    },
}

#[derive(Clone)]
pub struct TeamAcpMemberTool {
    store: Arc<dyn TeamStore>,
    current_agent_id: String,
}

impl TeamAcpMemberTool {
    pub fn new(store: Arc<dyn TeamStore>, current_agent_id: String) -> Self {
        Self {
            store,
            current_agent_id,
        }
    }

    /// Verify the caller is the team's leader — the same gate `team_member_add`
    /// / `team_member_remove` enforce. Without it any agent could attach or
    /// detach ACP members, an authorization inconsistency with the sibling
    /// membership tools.
    async fn ensure_leader(&self, team_id: &str) -> Result<()> {
        let team = self
            .store
            .get_team(team_id)
            .await?
            .ok_or_else(|| AlephError::tool(format!("team not found: {team_id}")))?;
        if team.leader_id != self.current_agent_id {
            return Err(AlephError::tool(format!(
                "only the team leader ('{}') can manage ACP members",
                team.leader_id
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl AlephTool for TeamAcpMemberTool {
    const NAME: &'static str = "team_acp_member";
    const DESCRIPTION: &'static str =
        "Manage external coding CLI sessions (Claude Code, Codex, Gemini CLI, \
         ...) as team members. Use `add` to attach a harness session in a \
         given cwd; the dispatcher will then route tasks with `owner = \
         <agent_id>` through the ACP adapter pool. Use `remove` to detach \
         and `list` to inspect ACP-only members.";

    type Args = TeamAcpMemberArgs;
    type Output = TeamAcpMemberOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match args {
            TeamAcpMemberArgs::Add {
                team_id,
                harness_id,
                cwd,
                session_name,
                role,
            } => {
                debug!(
                    team_id = %team_id,
                    harness_id = %harness_id,
                    cwd = %cwd,
                    "team_acp_member: add"
                );
                self.ensure_leader(&team_id).await?;
                let member = self
                    .store
                    .add_member(NewTeamMember::for_acp_session(
                        team_id,
                        harness_id,
                        cwd,
                        session_name,
                        role,
                    ))
                    .await?;
                Ok(TeamAcpMemberOutput::Add { member })
            }
            TeamAcpMemberArgs::Remove { team_id, agent_id } => {
                debug!(team_id = %team_id, agent_id = %agent_id, "team_acp_member: remove");
                self.ensure_leader(&team_id).await?;
                // Refuse to remove non-ACP rows via this tool so the LLM
                // doesn't accidentally drop an in-process agent.
                let members = self.store.get_members(&team_id).await?;
                match members.iter().find(|m| m.agent_id == agent_id) {
                    Some(m) if m.kind == TeamMemberKind::AcpSession => {}
                    Some(_) => {
                        return Err(AlephError::other(format!(
                            "Member '{agent_id}' is not ACP-backed; \
                             use team_member_remove instead"
                        )))
                    }
                    None => {
                        return Err(AlephError::other(format!(
                            "Member '{agent_id}' not found in team '{team_id}'"
                        )))
                    }
                }
                self.store.remove_member(&team_id, &agent_id).await?;
                Ok(TeamAcpMemberOutput::Remove { team_id, agent_id })
            }
            TeamAcpMemberArgs::List { team_id } => {
                debug!(team_id = %team_id, "team_acp_member: list");
                let members = self.store.get_members(&team_id).await?;
                let acp_only: Vec<TeamMember> = members
                    .into_iter()
                    .filter(|m| m.kind == TeamMemberKind::AcpSession)
                    .collect();
                Ok(TeamAcpMemberOutput::List {
                    team_id,
                    members: acp_only,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::{NewTeam, SqliteTeamStore};

    async fn setup() -> (Arc<dyn TeamStore>, String) {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let store = SqliteTeamStore::new(conn);
        store.migrate().await.unwrap();
        let store: Arc<dyn TeamStore> = Arc::new(store);
        let team = store
            .create_team(NewTeam {
                name: "AcpTeam".into(),
                description: String::new(),
                leader_id: "leader".into(),
            })
            .await
            .unwrap();
        (store, team.id)
    }

    #[tokio::test]
    async fn add_then_list_returns_only_acp_members() {
        let (store, team_id) = setup().await;

        // Add an in-process agent first — should NOT show up in list.
        store
            .add_member(NewTeamMember::for_agent(
                team_id.clone(),
                "leader",
                "leader",
            ))
            .await
            .unwrap();

        let tool = TeamAcpMemberTool::new(Arc::clone(&store), "leader".to_string());
        tool.call(TeamAcpMemberArgs::Add {
            team_id: team_id.clone(),
            harness_id: "claude-code".into(),
            cwd: "/work/repo".into(),
            session_name: None,
            role: "reviewer".into(),
        })
        .await
        .unwrap();

        let out = tool
            .call(TeamAcpMemberArgs::List {
                team_id: team_id.clone(),
            })
            .await
            .unwrap();

        let members = match out {
            TeamAcpMemberOutput::List { members, .. } => members,
            _ => panic!("wrong variant"),
        };
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].kind, TeamMemberKind::AcpSession);
        assert_eq!(members[0].acp_harness_id.as_deref(), Some("claude-code"));
    }

    #[tokio::test]
    async fn remove_refuses_non_acp_rows() {
        let (store, team_id) = setup().await;
        store
            .add_member(NewTeamMember::for_agent(
                team_id.clone(),
                "worker",
                "worker",
            ))
            .await
            .unwrap();

        let tool = TeamAcpMemberTool::new(store, "leader".to_string());
        let err = tool
            .call(TeamAcpMemberArgs::Remove {
                team_id,
                agent_id: "worker".into(),
            })
            .await
            .expect_err("must refuse non-acp member");
        assert!(format!("{err:?}").contains("not ACP-backed"));
    }
}
