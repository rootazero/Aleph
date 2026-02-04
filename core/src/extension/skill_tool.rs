//! Skill Tool - LLM-callable skill invocation
//!
//! This module provides the core logic for invoking skills as LLM tools,
//! including permission checking, template rendering, and structured results.

use super::error::{ExtensionError, ExtensionResult};
use super::template::SkillTemplate;
use super::types::{
    ExtensionSkill, PermissionAction, PermissionRule, PromptScope, SkillContext, SkillMetadata,
    SkillToolResult,
};
use crate::event::{
    AetherEvent, EventFilter, EventType, GlobalBus, PermissionReply, PermissionRequest,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;
use tracing::{debug, info, warn};

/// Check if a skill invocation is permitted
///
/// Returns Ok(()) if allowed, Err(PermissionDenied) if denied.
/// Note: "Ask" permission is currently treated as "Allow" - proper ask flow
/// would require UI/event integration.
pub fn check_skill_permission(
    skill_name: &str,
    ctx: &SkillContext,
) -> ExtensionResult<PermissionAction> {
    let Some(permissions) = &ctx.agent_permissions else {
        // No permissions defined = allow
        return Ok(PermissionAction::Allow);
    };

    // Check for "skill" permission rules
    let skill_rule = permissions.get("skill");

    match skill_rule {
        None => {
            // No skill-specific rule = allow
            Ok(PermissionAction::Allow)
        }
        Some(rule) => {
            let action = evaluate_permission_rule(rule, skill_name);
            match action {
                PermissionAction::Deny => {
                    warn!("Permission denied for skill: {}", skill_name);
                    Err(ExtensionError::PermissionDenied(skill_name.to_string()))
                }
                PermissionAction::Ask => {
                    // Return Ask - caller should use request_skill_permission_async
                    debug!("Skill {} requires user permission", skill_name);
                    Ok(PermissionAction::Ask)
                }
                PermissionAction::Allow => Ok(PermissionAction::Allow),
            }
        }
    }
}

/// Evaluate a permission rule against a skill name
fn evaluate_permission_rule(rule: &PermissionRule, skill_name: &str) -> PermissionAction {
    match rule {
        PermissionRule::Simple(action) => *action,
        PermissionRule::Patterns(patterns) => {
            // Check for exact match first
            if let Some(action) = patterns.get(skill_name) {
                return *action;
            }

            // Check for prefix matches (e.g., "plugin:*") - more specific than wildcard
            for (pattern, action) in patterns {
                if pattern.ends_with('*') && pattern != "*" {
                    let prefix = &pattern[..pattern.len() - 1];
                    if skill_name.starts_with(prefix) {
                        return *action;
                    }
                }
            }

            // Check for wildcard match (least specific)
            if let Some(action) = patterns.get("*") {
                return *action;
            }

            // No match = default to Ask
            PermissionAction::Ask
        }
    }
}

/// Default timeout for permission requests (300 seconds)
const PERMISSION_TIMEOUT_SECS: u64 = 300;

/// Request skill permission asynchronously via EventBus
///
/// This function publishes a PermissionAsked event to the GlobalBus and waits
/// for a PermissionReplied event with the matching request_id.
///
/// # Arguments
///
/// * `skill_name` - The qualified skill name (e.g., "plugin:skill")
/// * `session_id` - The current session ID
/// * `agent_id` - The current agent ID
///
/// # Returns
///
/// * `Ok(true)` if permission was granted (Once or Always)
/// * `Ok(false)` if permission was denied (Reject)
/// * `Err(_)` if there was an error or timeout
pub async fn request_skill_permission_async(
    skill_name: &str,
    session_id: &str,
    agent_id: &str,
) -> ExtensionResult<bool> {
    let request_id = uuid::Uuid::new_v4().to_string();
    let bus = GlobalBus::global();

    // Create oneshot channel for response
    let (tx, rx) = oneshot::channel::<PermissionReply>();
    let tx = Arc::new(tokio::sync::Mutex::new(Some(tx)));

    // Subscribe to PermissionReplied events
    let request_id_clone = request_id.clone();
    let session_id_owned = session_id.to_string();
    let tx_clone = tx.clone();

    let filter = EventFilter::new(vec![EventType::PermissionReplied])
        .with_session(&session_id_owned);

    let sub_id = bus
        .subscribe_async(filter, move |global_event| {
            if let AetherEvent::PermissionReplied {
                request_id: ref rid,
                reply,
                ..
            } = global_event.event
            {
                if rid == &request_id_clone {
                    // Send reply through oneshot channel
                    if let Ok(mut guard) = tx_clone.try_lock() {
                        if let Some(sender) = guard.take() {
                            let _ = sender.send(reply);
                        }
                    }
                }
            }
        })
        .await;

    // Create and publish permission request
    let request = PermissionRequest::new(
        &request_id,
        session_id,
        "skill",
        vec![skill_name.to_string()],
    )
    .with_metadata("skill_name", serde_json::json!(skill_name));

    info!(
        skill_name,
        request_id, "Requesting skill permission via EventBus"
    );

    bus.broadcast(agent_id, session_id, AetherEvent::PermissionAsked(request))
        .await;

    // Wait for response with timeout
    let result = tokio::time::timeout(Duration::from_secs(PERMISSION_TIMEOUT_SECS), rx).await;

    // Clean up subscription
    bus.unsubscribe(&sub_id).await;

    match result {
        Ok(Ok(reply)) => {
            let allowed = reply.is_allowed();
            info!(
                skill_name,
                request_id,
                allowed,
                "Permission reply received"
            );
            Ok(allowed)
        }
        Ok(Err(_)) => {
            // Channel closed without response
            warn!(skill_name, request_id, "Permission request channel closed");
            Err(ExtensionError::PermissionDenied(format!(
                "Permission request cancelled for skill: {}",
                skill_name
            )))
        }
        Err(_) => {
            // Timeout
            warn!(
                skill_name,
                request_id, "Permission request timed out after {} seconds", PERMISSION_TIMEOUT_SECS
            );
            Err(ExtensionError::PermissionDenied(format!(
                "Permission request timed out for skill: {}",
                skill_name
            )))
        }
    }
}

/// Invoke a skill and return structured result
///
/// This is the core function called when LLM invokes the skill tool.
pub async fn invoke_skill(
    skill: &ExtensionSkill,
    arguments: &str,
    ctx: &SkillContext,
) -> ExtensionResult<SkillToolResult> {
    let qualified_name = skill.qualified_name();

    // Check permission
    check_skill_permission(&qualified_name, ctx)?;

    // Create template and render
    let template = SkillTemplate::new(&skill.content, &skill.source_path);
    let rendered_content = template.render(arguments).await?;

    // Build result
    let result = SkillToolResult {
        title: format!("Loaded skill: {}", skill.name),
        content: rendered_content,
        base_dir: template.base_dir().to_path_buf(),
        metadata: SkillMetadata {
            name: skill.name.clone(),
            qualified_name,
            source: skill.source,
        },
    };

    debug!(
        "Skill {} invoked successfully with base_dir: {:?}",
        result.metadata.qualified_name, result.base_dir
    );

    Ok(result)
}

/// Build a tool description for the skill tool
///
/// This generates the description that tells the LLM about available skills.
pub fn build_skill_tool_description(skills: &[ExtensionSkill]) -> String {
    if skills.is_empty() {
        return "Load a skill to get detailed instructions for a specific task. No skills are currently available.".to_string();
    }

    let mut parts = vec![
        "Load a skill to get detailed instructions for a specific task.".to_string(),
        "Skills provide specialized knowledge and step-by-step guidance.".to_string(),
        "Use this when a task matches an available skill's description.".to_string(),
        "Only the skills listed here are available:".to_string(),
        "<available_skills>".to_string(),
    ];

    for skill in skills {
        parts.push("  <skill>".to_string());
        parts.push(format!("    <name>{}</name>", skill.qualified_name()));
        parts.push(format!("    <description>{}</description>", skill.description));
        parts.push("  </skill>".to_string());
    }

    parts.push("</available_skills>".to_string());

    parts.join(" ")
}

/// Filter skills by scope for injection
///
/// Returns skills that should be auto-injected based on their scope and active tools:
/// - `System` scope: Always included
/// - `Tool` scope: Only included if the bound tool is in the active tools list
/// - `Standalone` scope: Never auto-injected (user must explicitly invoke)
/// - `Disabled` scope: Never included
pub fn filter_skills_by_scope<'a>(
    skills: &'a [ExtensionSkill],
    active_tools: Option<&[String]>,
) -> Vec<&'a ExtensionSkill> {
    skills
        .iter()
        .filter(|skill| match skill.scope {
            PromptScope::System => true,
            PromptScope::Tool => {
                // Only include if bound tool is active
                if let (Some(bound), Some(tools)) = (&skill.bound_tool, active_tools) {
                    tools.iter().any(|t| t == bound)
                } else {
                    false
                }
            }
            PromptScope::Standalone => false,
            PromptScope::Disabled => false,
        })
        .collect()
}

/// Build skill tool description with scope-aware filtering
///
/// This is an enhanced version of `build_skill_tool_description` that filters
/// skills based on their scope and the currently active tools.
///
/// # Arguments
///
/// * `skills` - All available skills
/// * `active_tools` - Optional list of currently active tool names. If provided,
///   skills with `Tool` scope will only be included if their bound tool is in this list.
///
/// # Returns
///
/// A formatted string describing the available skills, suitable for injection
/// into the system prompt. Returns an empty string if no skills pass the filter.
pub fn build_skill_tool_description_v2(
    skills: &[ExtensionSkill],
    active_tools: Option<&[String]>,
) -> String {
    let filtered = filter_skills_by_scope(skills, active_tools);

    if filtered.is_empty() {
        return String::new();
    }

    let mut output = String::from("<available_skills>\n");

    for skill in filtered {
        if skill.is_auto_invocable() {
            output.push_str(&format!(
                "  <skill>\n    <name>{}</name>\n    <description>{}</description>\n  </skill>\n",
                skill.qualified_name(),
                skill.description
            ));
        }
    }

    output.push_str("</available_skills>");
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::DiscoverySource;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn create_test_skill() -> ExtensionSkill {
        ExtensionSkill {
            name: "test-skill".to_string(),
            plugin_name: Some("my-plugin".to_string()),
            skill_type: crate::extension::SkillType::Skill,
            description: "A test skill".to_string(),
            content: "Hello $ARGUMENTS!".to_string(),
            disable_model_invocation: false,
            scope: crate::extension::PromptScope::System,
            bound_tool: None,
            source_path: PathBuf::from("/test/skill/SKILL.md"),
            source: DiscoverySource::AetherGlobal,
        }
    }

    #[test]
    fn test_permission_no_rules() {
        let ctx = SkillContext::default();
        let result = check_skill_permission("any-skill", &ctx);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PermissionAction::Allow);
    }

    #[test]
    fn test_permission_simple_allow() {
        let mut permissions = HashMap::new();
        permissions.insert("skill".to_string(), PermissionRule::Simple(PermissionAction::Allow));

        let ctx = SkillContext {
            session_id: "test".to_string(),
            agent_permissions: Some(permissions),
        };

        let result = check_skill_permission("any-skill", &ctx);
        assert!(result.is_ok());
    }

    #[test]
    fn test_permission_simple_deny() {
        let mut permissions = HashMap::new();
        permissions.insert("skill".to_string(), PermissionRule::Simple(PermissionAction::Deny));

        let ctx = SkillContext {
            session_id: "test".to_string(),
            agent_permissions: Some(permissions),
        };

        let result = check_skill_permission("any-skill", &ctx);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ExtensionError::PermissionDenied(_)
        ));
    }

    #[test]
    fn test_permission_pattern_match() {
        let mut patterns = HashMap::new();
        patterns.insert("my-plugin:*".to_string(), PermissionAction::Allow);
        patterns.insert("*".to_string(), PermissionAction::Deny);

        let mut permissions = HashMap::new();
        permissions.insert("skill".to_string(), PermissionRule::Patterns(patterns));

        let ctx = SkillContext {
            session_id: "test".to_string(),
            agent_permissions: Some(permissions),
        };

        // Should match my-plugin:* pattern
        let result = check_skill_permission("my-plugin:test", &ctx);
        assert!(result.is_ok());

        // Should match * pattern (deny)
        let result = check_skill_permission("other-plugin:test", &ctx);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_invoke_skill() {
        let skill = create_test_skill();
        let ctx = SkillContext::default();

        let result = invoke_skill(&skill, "World", &ctx).await.unwrap();

        assert_eq!(result.title, "Loaded skill: test-skill");
        assert_eq!(result.content, "Hello World!");
        assert_eq!(result.metadata.name, "test-skill");
        assert_eq!(result.metadata.qualified_name, "my-plugin:test-skill");
    }

    #[test]
    fn test_build_tool_description_empty() {
        let desc = build_skill_tool_description(&[]);
        assert!(desc.contains("No skills are currently available"));
    }

    #[test]
    fn test_build_tool_description() {
        let skills = vec![create_test_skill()];
        let desc = build_skill_tool_description(&skills);

        assert!(desc.contains("available_skills"));
        assert!(desc.contains("my-plugin:test-skill"));
        assert!(desc.contains("A test skill"));
    }

    // Note: Full EventBus integration tests for permission request are in
    // core/src/event/integration_test.rs. These unit tests focus on basic
    // permission logic without requiring the full async EventBus flow.

    // =============================================================================
    // Scope Filtering Tests
    // =============================================================================

    fn create_skill_with_scope(name: &str, scope: PromptScope, bound_tool: Option<&str>) -> ExtensionSkill {
        ExtensionSkill {
            name: name.to_string(),
            plugin_name: Some("test-plugin".to_string()),
            skill_type: crate::extension::SkillType::Skill,
            description: format!("{} description", name),
            content: "Content".to_string(),
            disable_model_invocation: false,
            scope,
            bound_tool: bound_tool.map(|s| s.to_string()),
            source_path: PathBuf::from("/test/skill/SKILL.md"),
            source: DiscoverySource::AetherGlobal,
        }
    }

    #[test]
    fn test_filter_skills_by_scope_system_always_included() {
        let skills = vec![
            create_skill_with_scope("system-skill", PromptScope::System, None),
        ];

        // Should be included with no active tools
        let filtered = filter_skills_by_scope(&skills, None);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "system-skill");

        // Should be included with any active tools
        let active_tools = vec!["some-tool".to_string()];
        let filtered = filter_skills_by_scope(&skills, Some(&active_tools));
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn test_filter_skills_by_scope_tool_requires_active_tool() {
        let skills = vec![
            create_skill_with_scope("tool-skill", PromptScope::Tool, Some("bash")),
        ];

        // Should NOT be included with no active tools
        let filtered = filter_skills_by_scope(&skills, None);
        assert_eq!(filtered.len(), 0);

        // Should NOT be included if bound tool is not in active tools
        let active_tools = vec!["read".to_string(), "write".to_string()];
        let filtered = filter_skills_by_scope(&skills, Some(&active_tools));
        assert_eq!(filtered.len(), 0);

        // Should be included if bound tool IS in active tools
        let active_tools = vec!["bash".to_string(), "read".to_string()];
        let filtered = filter_skills_by_scope(&skills, Some(&active_tools));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "tool-skill");
    }

    #[test]
    fn test_filter_skills_by_scope_tool_without_bound_tool_excluded() {
        // A Tool scope skill without a bound_tool should never be included
        let skills = vec![
            create_skill_with_scope("broken-tool-skill", PromptScope::Tool, None),
        ];

        let active_tools = vec!["bash".to_string()];
        let filtered = filter_skills_by_scope(&skills, Some(&active_tools));
        assert_eq!(filtered.len(), 0);
    }

    #[test]
    fn test_filter_skills_by_scope_standalone_never_included() {
        let skills = vec![
            create_skill_with_scope("standalone-skill", PromptScope::Standalone, None),
        ];

        // Should never be auto-injected
        let filtered = filter_skills_by_scope(&skills, None);
        assert_eq!(filtered.len(), 0);

        let active_tools = vec!["bash".to_string()];
        let filtered = filter_skills_by_scope(&skills, Some(&active_tools));
        assert_eq!(filtered.len(), 0);
    }

    #[test]
    fn test_filter_skills_by_scope_disabled_never_included() {
        let skills = vec![
            create_skill_with_scope("disabled-skill", PromptScope::Disabled, None),
        ];

        let filtered = filter_skills_by_scope(&skills, None);
        assert_eq!(filtered.len(), 0);
    }

    #[test]
    fn test_filter_skills_by_scope_mixed() {
        let skills = vec![
            create_skill_with_scope("system-1", PromptScope::System, None),
            create_skill_with_scope("system-2", PromptScope::System, None),
            create_skill_with_scope("tool-bash", PromptScope::Tool, Some("bash")),
            create_skill_with_scope("tool-read", PromptScope::Tool, Some("read")),
            create_skill_with_scope("standalone", PromptScope::Standalone, None),
            create_skill_with_scope("disabled", PromptScope::Disabled, None),
        ];

        // With no active tools: only System skills
        let filtered = filter_skills_by_scope(&skills, None);
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|s| s.scope == PromptScope::System));

        // With bash active: System + tool-bash
        let active_tools = vec!["bash".to_string()];
        let filtered = filter_skills_by_scope(&skills, Some(&active_tools));
        assert_eq!(filtered.len(), 3);

        let names: Vec<&str> = filtered.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"system-1"));
        assert!(names.contains(&"system-2"));
        assert!(names.contains(&"tool-bash"));
        assert!(!names.contains(&"tool-read"));
        assert!(!names.contains(&"standalone"));
        assert!(!names.contains(&"disabled"));
    }

    #[test]
    fn test_build_skill_tool_description_v2_empty() {
        let skills: Vec<ExtensionSkill> = vec![];
        let desc = build_skill_tool_description_v2(&skills, None);
        assert!(desc.is_empty());
    }

    #[test]
    fn test_build_skill_tool_description_v2_filters_by_scope() {
        let skills = vec![
            create_skill_with_scope("system-skill", PromptScope::System, None),
            create_skill_with_scope("tool-skill", PromptScope::Tool, Some("bash")),
            create_skill_with_scope("standalone-skill", PromptScope::Standalone, None),
        ];

        // Without active tools: only system-skill
        let desc = build_skill_tool_description_v2(&skills, None);
        assert!(desc.contains("system-skill"));
        assert!(!desc.contains("tool-skill"));
        assert!(!desc.contains("standalone-skill"));

        // With bash active: system-skill + tool-skill
        let active_tools = vec!["bash".to_string()];
        let desc = build_skill_tool_description_v2(&skills, Some(&active_tools));
        assert!(desc.contains("system-skill"));
        assert!(desc.contains("tool-skill"));
        assert!(!desc.contains("standalone-skill"));
    }

    #[test]
    fn test_build_skill_tool_description_v2_respects_auto_invocable() {
        let system_skill = create_skill_with_scope("invocable", PromptScope::System, None);
        let mut non_invocable = create_skill_with_scope("non-invocable", PromptScope::System, None);
        non_invocable.disable_model_invocation = true;

        let mut command_skill = create_skill_with_scope("command", PromptScope::System, None);
        command_skill.skill_type = crate::extension::SkillType::Command;

        let skills = vec![system_skill, non_invocable, command_skill];

        let desc = build_skill_tool_description_v2(&skills, None);

        // Only "invocable" should appear (auto-invocable)
        assert!(desc.contains("invocable"));
        // "non-invocable" has disable_model_invocation = true
        assert!(!desc.contains("non-invocable"));
        // "command" is SkillType::Command, not Skill
        assert!(!desc.contains(">command<")); // Using >< to avoid matching "command" in other contexts
    }

    #[test]
    fn test_build_skill_tool_description_v2_format() {
        let skills = vec![
            create_skill_with_scope("my-skill", PromptScope::System, None),
        ];

        let desc = build_skill_tool_description_v2(&skills, None);

        // Check XML structure
        assert!(desc.starts_with("<available_skills>"));
        assert!(desc.ends_with("</available_skills>"));
        assert!(desc.contains("<skill>"));
        assert!(desc.contains("</skill>"));
        assert!(desc.contains("<name>test-plugin:my-skill</name>"));
        assert!(desc.contains("<description>my-skill description</description>"));
    }
}
