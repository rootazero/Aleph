//! Skill Tool - LLM-callable skill invocation
//!
//! This module provides the core logic for invoking skills as LLM tools,
//! including permission checking, template rendering, and structured results.

use super::error::{ExtensionError, ExtensionResult};
use super::template::SkillTemplate;
use super::types::{
    ExtensionSkill, PermissionAction, PermissionRule, SkillContext, SkillMetadata, SkillToolResult,
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
        parts.push(format!("  <skill>"));
        parts.push(format!("    <name>{}</name>", skill.qualified_name()));
        parts.push(format!("    <description>{}</description>", skill.description));
        parts.push(format!("  </skill>"));
    }

    parts.push("</available_skills>".to_string());

    parts.join(" ")
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
}
