//! Skill Sub-Agent
//!
//! A specialized sub-agent for discovering and understanding Skills (DAG-based workflows).
//! This agent is delegated to when the main agent needs to find and understand
//! complex multi-step workflows defined as Skills.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::RwLock;
use tracing::{debug, info};

use super::traits::{SubAgent, SubAgentCapability, SubAgentRequest, SubAgentResult, ToolCallRecord};
use crate::dispatcher::{ToolRegistry, ToolSource, UnifiedTool};
use crate::error::Result;

/// Skill Sub-Agent for discovering and understanding Skills (DAG workflows)
///
/// This sub-agent specializes in:
/// - Listing available skills
/// - Providing skill schemas and workflow info
/// - Recommending appropriate skills for tasks
pub struct SkillSubAgent {
    /// Unique ID for this sub-agent
    id: String,
    /// Tool registry for accessing skill tools
    tool_registry: Arc<RwLock<ToolRegistry>>,
}

impl SkillSubAgent {
    /// Create a new Skill sub-agent
    pub fn new(tool_registry: Arc<RwLock<ToolRegistry>>) -> Self {
        Self {
            id: "skill_agent".to_string(),
            tool_registry,
        }
    }

    /// Get all skill tools from the tool registry
    async fn get_skill_tools(&self) -> Vec<UnifiedTool> {
        let registry = self.tool_registry.read().await;
        registry
            .list_all()
            .await
            .into_iter()
            .filter(|tool| matches!(tool.source, ToolSource::Skill { .. }))
            .collect()
    }

    /// Find tools matching a query
    fn find_matching_skills<'a>(&self, prompt: &str, skills: &'a [UnifiedTool]) -> Vec<&'a UnifiedTool> {
        let prompt_lower = prompt.to_lowercase();
        let keywords: Vec<&str> = prompt_lower.split_whitespace().collect();

        skills
            .iter()
            .filter(|skill| {
                let name_lower = skill.name.to_lowercase();
                let desc_lower = skill.description.to_lowercase();

                // Check for keyword matches
                keywords.iter().any(|kw| {
                    kw.len() > 2 && (name_lower.contains(kw) || desc_lower.contains(kw))
                })
            })
            .collect()
    }

    /// Format skill info for response
    fn format_skill_info(&self, skill: &UnifiedTool) -> String {
        let skill_id = if let ToolSource::Skill { id } = &skill.source {
            id.clone()
        } else {
            skill.name.clone()
        };

        let params = skill
            .parameters_schema
            .as_ref()
            .map(|s| serde_json::to_string_pretty(s).unwrap_or_default())
            .unwrap_or_else(|| "{}".to_string());

        format!(
            "**{}** (id: {})\n{}\nParameters:\n```json\n{}\n```",
            skill.name, skill_id, skill.description, params
        )
    }
}

#[async_trait]
impl SubAgent for SkillSubAgent {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        "Skill Agent"
    }

    fn description(&self) -> &str {
        "Specialized agent for discovering and understanding Skills (DAG-based workflows)"
    }

    fn capabilities(&self) -> Vec<SubAgentCapability> {
        vec![SubAgentCapability::SkillExecution]
    }

    fn can_handle(&self, request: &SubAgentRequest) -> bool {
        // Can handle if:
        // 1. Target is specified and is a skill ID
        // 2. Prompt mentions skill-related keywords
        if request.target.is_some() {
            return true;
        }

        let prompt_lower = request.prompt.to_lowercase();
        prompt_lower.contains("skill")
            || prompt_lower.contains("workflow")
            || prompt_lower.contains("run skill")
            || prompt_lower.contains("execute skill")
    }

    async fn execute(&self, request: SubAgentRequest) -> Result<SubAgentResult> {
        info!("Skill Agent executing request: {}", request.id);

        // Get available skills
        let available_skills = self.get_skill_tools().await;

        if available_skills.is_empty() {
            return Ok(SubAgentResult::success(
                &request.id,
                "No skills are currently available. Skills can be added through the plugin system or skill registry.".to_string(),
            )
            .with_output(json!({
                "skill_count": 0,
            })));
        }

        debug!("Skill Agent has {} skills available", available_skills.len());

        // Find matching skills based on the prompt or target
        let matching_skills = if let Some(ref target) = request.target {
            // Direct match by target name
            available_skills
                .iter()
                .filter(|s| {
                    s.name.to_lowercase() == target.to_lowercase()
                        || if let ToolSource::Skill { id } = &s.source {
                            id.to_lowercase() == target.to_lowercase()
                        } else {
                            false
                        }
                })
                .collect::<Vec<_>>()
        } else {
            self.find_matching_skills(&request.prompt, &available_skills)
        };

        if !matching_skills.is_empty() {
            // Found matching skills - provide detailed info
            let skill_infos: Vec<String> = matching_skills
                .iter()
                .take(5)
                .map(|s| self.format_skill_info(s))
                .collect();

            let summary = format!(
                "Found {} relevant skill(s) for your request:\n\n{}",
                matching_skills.len(),
                skill_infos.join("\n\n")
            );

            let skill_names: Vec<_> = matching_skills.iter().map(|s| s.name.clone()).collect();

            Ok(SubAgentResult::success(&request.id, summary)
                .with_output(json!({
                    "matching_skills": skill_names,
                    "skill_count": matching_skills.len(),
                    "recommendation": if matching_skills.len() == 1 {
                        format!("Use the '{}' skill to complete this task.", matching_skills[0].name)
                    } else {
                        "Review the skills above and select the most appropriate one.".to_string()
                    }
                }))
                .with_tools_called(vec![ToolCallRecord {
                    name: "list_skills".to_string(),
                    arguments: json!({"query": request.prompt}),
                    success: true,
                    result_summary: format!("Found {} matching skills", matching_skills.len()),
                }]))
        } else {
            // No matching skills - list all available
            let skill_list: Vec<_> = available_skills
                .iter()
                .map(|s| {
                    let skill_id = if let ToolSource::Skill { id } = &s.source {
                        id.clone()
                    } else {
                        s.name.clone()
                    };
                    format!("- **{}** [{}]: {}", s.name, skill_id, s.description)
                })
                .collect();

            Ok(SubAgentResult::success(
                &request.id,
                format!(
                    "Available skills ({} total):\n\n{}",
                    available_skills.len(),
                    skill_list.join("\n")
                ),
            )
            .with_output(json!({
                "available_skills": available_skills.iter().map(|s| &s.name).collect::<Vec<_>>(),
                "skill_count": available_skills.len(),
            })))
        }
    }

    fn available_actions(&self) -> Vec<String> {
        vec![
            "list_skills".to_string(),
            "get_skill_info".to_string(),
            "find_skills".to_string(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::ToolRegistry;

    #[tokio::test]
    async fn test_skill_agent_creation() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let agent = SkillSubAgent::new(registry);

        assert_eq!(agent.id(), "skill_agent");
        assert_eq!(agent.name(), "Skill Agent");
        assert!(agent.capabilities().contains(&SubAgentCapability::SkillExecution));
    }

    #[tokio::test]
    async fn test_skill_agent_can_handle() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let agent = SkillSubAgent::new(registry);

        // With target
        let request = SubAgentRequest::new("Execute").with_target("my_skill");
        assert!(agent.can_handle(&request));

        // With skill keyword
        let request = SubAgentRequest::new("Run the workflow skill");
        assert!(agent.can_handle(&request));

        // Without relevant keywords
        let request = SubAgentRequest::new("Search the web");
        assert!(!agent.can_handle(&request));
    }

    #[tokio::test]
    async fn test_skill_agent_no_skills() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let agent = SkillSubAgent::new(registry);

        let request = SubAgentRequest::new("Run skill").with_target("my_skill");
        let result = agent.execute(request).await.unwrap();

        // Should succeed with info about no skills
        assert!(result.success);
        assert!(result.summary.contains("No skills"));
    }
}
