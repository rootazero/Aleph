//! Compile-time embedded prompt templates for team roles.

use super::types::AgentRole;

const LEADER_PROMPT: &str = include_str!("../../agents/prompts/team_leader.md");
const EXPLORER_PROMPT: &str = include_str!("../../agents/prompts/team_explorer.md");
const CRITIC_PROMPT: &str = include_str!("../../agents/prompts/team_critic.md");
const WORKER_PROMPT: &str = include_str!("../../agents/prompts/team_worker.md");

/// Returns the built-in prompt template for a known role, or `None` for custom roles.
pub fn role_prompt_template(role: &AgentRole) -> Option<&'static str> {
    match role {
        AgentRole::Leader => Some(LEADER_PROMPT),
        AgentRole::Explorer => Some(EXPLORER_PROMPT),
        AgentRole::Critic => Some(CRITIC_PROMPT),
        AgentRole::Worker => Some(WORKER_PROMPT),
        AgentRole::Custom(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_roles_return_non_empty_templates() {
        for role in [
            AgentRole::Leader,
            AgentRole::Explorer,
            AgentRole::Critic,
            AgentRole::Worker,
        ] {
            let tpl = role_prompt_template(&role);
            assert!(tpl.is_some(), "missing template for {:?}", role);
            assert!(!tpl.unwrap().is_empty(), "empty template for {:?}", role);
        }
    }

    #[test]
    fn custom_role_returns_none() {
        assert!(role_prompt_template(&AgentRole::Custom("analyst".into())).is_none());
    }
}
