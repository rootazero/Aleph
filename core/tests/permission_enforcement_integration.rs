//! Integration tests for permission enforcement
//!
//! Tests the full flow from guest invitation creation to tool execution
//! with permission checks.

use aleph_protocol::{CreateInvitationRequest, GuestScope, IdentityContext, Role};
use alephcore::agent_loop::{Action, ActionExecutor, ActionResult};
use alephcore::executor::SingleStepExecutor;
use alephcore::gateway::security::invitation_manager::InvitationManager;
use alephcore::gateway::security::policy_engine::PolicyEngine;
use alephcore::{AlephError, UnifiedTool};
use serde_json::json;
use std::sync::Arc;

/// Mock tool registry for testing
struct MockToolRegistry {
    tools: Vec<UnifiedTool>,
}

impl MockToolRegistry {
    fn new() -> Self {
        Self { tools: vec![] }
    }

    fn add_tool(&mut self, tool: UnifiedTool) {
        self.tools.push(tool);
    }
}

impl alephcore::executor::ToolRegistry for MockToolRegistry {
    fn get_tool(&self, name: &str) -> Option<&UnifiedTool> {
        self.tools.iter().find(|t| t.name == name)
    }

    fn execute_tool(
        &self,
        tool_name: &str,
        _arguments: serde_json::Value,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, AlephError>> + Send + '_>,
    > {
        let result = if self.get_tool(tool_name).is_some() {
            Ok(json!({"result": "success", "tool": tool_name}))
        } else {
            Err(AlephError::other(format!(
                "Tool not found: {}",
                tool_name
            )))
        };
        Box::pin(async move { result })
    }
}

fn create_test_tool(name: &str) -> UnifiedTool {
    UnifiedTool::new(
        format!("test:{}", name),
        name,
        format!("Test tool: {}", name),
        alephcore::dispatcher::ToolSource::Builtin,
    )
}

#[tokio::test]
async fn test_guest_invitation_and_permission_flow() {
    // Setup: Create InvitationManager
    let manager = InvitationManager::new();

    // Step 1: Create guest invitation with limited scope
    let scope = GuestScope {
        allowed_tools: vec!["translate".to_string(), "search".to_string()],
        expires_at: Some(chrono::Utc::now().timestamp() + 3600), // 1 hour from now
        display_name: Some("Test Guest".to_string()),
    };

    let request = CreateInvitationRequest {
        guest_name: "test-guest".to_string(),
        scope: scope.clone(),
    };

    let invitation = manager.create_invitation(request).unwrap();

    assert!(!invitation.guest_id.is_empty());
    assert!(!invitation.token.is_empty());

    // Step 2: Activate invitation to create IdentityContext
    let activated = manager.activate_invitation(&invitation.token).unwrap();
    assert_eq!(activated.guest_id, invitation.guest_id);

    let guest_identity = IdentityContext {
        request_id: "test-request".to_string(),
        session_key: "test-session".to_string(),
        role: Role::Guest,
        identity_id: activated.guest_id.clone(),
        scope: Some(activated.scope.clone()),
        created_at: chrono::Utc::now().timestamp(),
        source_channel: "test".to_string(),
    };

    // Step 3: Setup executor with test tools
    let mut registry = MockToolRegistry::new();
    registry.add_tool(create_test_tool("translate"));
    registry.add_tool(create_test_tool("search"));
    registry.add_tool(create_test_tool("shell_exec"));

    let executor = SingleStepExecutor::new(Arc::new(registry));

    // Step 4: Test allowed tool (translate) - should succeed
    let allowed_action = Action::ToolCall {
        tool_name: "translate".to_string(),
        arguments: json!({"text": "Hello"}),
    };

    let result = executor.execute(&allowed_action, &guest_identity).await;
    assert!(
        matches!(result, ActionResult::ToolSuccess { .. }),
        "Expected ToolSuccess for allowed tool, got: {:?}",
        result
    );

    // Step 5: Test denied tool (shell_exec) - should fail with permission error
    let denied_action = Action::ToolCall {
        tool_name: "shell_exec".to_string(),
        arguments: json!({"command": "ls"}),
    };

    let result = executor.execute(&denied_action, &guest_identity).await;
    assert!(
        matches!(result, ActionResult::ToolError { retryable: false, .. }),
        "Expected ToolError for denied tool, got: {:?}",
        result
    );

    if let ActionResult::ToolError { error, .. } = result {
        assert!(
            error.contains("not in guest") || error.contains("scope"),
            "Error message should indicate permission denial, got: {}",
            error
        );
    }
}

#[tokio::test]
async fn test_owner_bypasses_permission_checks() {
    // Setup: Create Owner identity
    let owner_identity = IdentityContext::owner(
        "owner-session".to_string(),
        "test".to_string(),
    );

    // Setup executor with test tools
    let mut registry = MockToolRegistry::new();
    registry.add_tool(create_test_tool("shell_exec"));
    registry.add_tool(create_test_tool("translate"));

    let executor = SingleStepExecutor::new(Arc::new(registry));

    // Test: Owner can execute any tool (including shell_exec)
    let action = Action::ToolCall {
        tool_name: "shell_exec".to_string(),
        arguments: json!({"command": "ls"}),
    };

    let result = executor.execute(&action, &owner_identity).await;
    assert!(
        matches!(result, ActionResult::ToolSuccess { .. }),
        "Owner should be able to execute any tool, got: {:?}",
        result
    );
}

#[tokio::test]
async fn test_guest_with_wildcard_permission() {
    // Setup: Create guest with wildcard permission
    let scope = GuestScope {
        allowed_tools: vec!["*".to_string()],
        expires_at: None,
        display_name: Some("Admin Guest".to_string()),
    };

    let guest_identity = IdentityContext {
        request_id: "test-request".to_string(),
        session_key: "test-session".to_string(),
        role: Role::Guest,
        identity_id: "admin-guest".to_string(),
        scope: Some(scope),
        created_at: chrono::Utc::now().timestamp(),
        source_channel: "test".to_string(),
    };

    // Setup executor
    let mut registry = MockToolRegistry::new();
    registry.add_tool(create_test_tool("translate"));
    registry.add_tool(create_test_tool("shell_exec"));

    let executor = SingleStepExecutor::new(Arc::new(registry));

    // Test: Guest with wildcard can execute any tool
    let action1 = Action::ToolCall {
        tool_name: "translate".to_string(),
        arguments: json!({"text": "Hello"}),
    };
    let result1 = executor.execute(&action1, &guest_identity).await;
    assert!(matches!(result1, ActionResult::ToolSuccess { .. }));

    let action2 = Action::ToolCall {
        tool_name: "shell_exec".to_string(),
        arguments: json!({"command": "ls"}),
    };
    let result2 = executor.execute(&action2, &guest_identity).await;
    assert!(matches!(result2, ActionResult::ToolSuccess { .. }));
}

#[tokio::test]
async fn test_guest_with_category_permission() {
    // Setup: Create guest with category permission (shell)
    let scope = GuestScope {
        allowed_tools: vec!["shell".to_string()],
        expires_at: None,
        display_name: Some("Shell Guest".to_string()),
    };

    let guest_identity = IdentityContext {
        request_id: "test-request".to_string(),
        session_key: "test-session".to_string(),
        role: Role::Guest,
        identity_id: "shell-guest".to_string(),
        scope: Some(scope),
        created_at: chrono::Utc::now().timestamp(),
        source_channel: "test".to_string(),
    };

    // Setup executor
    let mut registry = MockToolRegistry::new();
    // Register tools with base names (without operation suffix)
    // because execute_tool_call normalizes "shell:exec" to "shell"
    registry.add_tool(create_test_tool("shell"));
    registry.add_tool(create_test_tool("translate"));

    let executor = SingleStepExecutor::new(Arc::new(registry));

    // Test: Guest can execute shell:exec (category match)
    // The tool is registered as "shell" but we call it as "shell:exec"
    // execute_tool_call normalizes "shell:exec" to "shell" for lookup
    let action1 = Action::ToolCall {
        tool_name: "shell:exec".to_string(),
        arguments: json!({"command": "ls"}),
    };
    let result1 = executor.execute(&action1, &guest_identity).await;
    if !matches!(result1, ActionResult::ToolSuccess { .. }) {
        eprintln!("Expected ToolSuccess but got: {:?}", result1);
    }
    assert!(
        matches!(result1, ActionResult::ToolSuccess { .. }),
        "Category permission should allow shell:exec, got: {:?}",
        result1
    );

    // Test: Guest can execute shell:read (category match)
    let action2 = Action::ToolCall {
        tool_name: "shell:read".to_string(),
        arguments: json!({"file": "test.txt"}),
    };
    let result2 = executor.execute(&action2, &guest_identity).await;
    assert!(
        matches!(result2, ActionResult::ToolSuccess { .. }),
        "Category permission should allow shell:read"
    );

    // Test: Guest cannot execute translate (not in category)
    let action3 = Action::ToolCall {
        tool_name: "translate".to_string(),
        arguments: json!({"text": "Hello"}),
    };
    let result3 = executor.execute(&action3, &guest_identity).await;
    assert!(
        matches!(result3, ActionResult::ToolError { retryable: false, .. }),
        "Should deny tool outside category"
    );
}

#[tokio::test]
async fn test_expired_guest_token_denied() {
    // Setup: Create guest with expired token
    let scope = GuestScope {
        allowed_tools: vec!["*".to_string()],
        expires_at: Some(chrono::Utc::now().timestamp() - 3600), // 1 hour ago
        display_name: Some("Expired Guest".to_string()),
    };

    let guest_identity = IdentityContext {
        request_id: "test-request".to_string(),
        session_key: "test-session".to_string(),
        role: Role::Guest,
        identity_id: "expired-guest".to_string(),
        scope: Some(scope),
        created_at: chrono::Utc::now().timestamp(),
        source_channel: "test".to_string(),
    };

    // Test: PolicyEngine should deny expired token
    let result = PolicyEngine::check_tool_permission(&guest_identity, "translate");
    assert!(
        !result.is_allowed(),
        "Expired token should be denied"
    );

    if let alephcore::gateway::security::policy_engine::PermissionResult::Denied { reason } = result {
        assert!(
            reason.contains("expired"),
            "Error should mention expiration, got: {}",
            reason
        );
    }
}

#[tokio::test]
async fn test_anonymous_role_denied() {
    // Setup: Create anonymous identity
    let anon_identity = IdentityContext {
        request_id: "test-request".to_string(),
        session_key: "test-session".to_string(),
        role: Role::Anonymous,
        identity_id: "anonymous".to_string(),
        scope: None,
        created_at: chrono::Utc::now().timestamp(),
        source_channel: "test".to_string(),
    };

    // Test: PolicyEngine should deny anonymous
    let result = PolicyEngine::check_tool_permission(&anon_identity, "translate");
    assert!(
        !result.is_allowed(),
        "Anonymous should be denied"
    );

    if let alephcore::gateway::security::policy_engine::PermissionResult::Denied { reason } = result {
        assert!(
            reason.contains("Authentication required"),
            "Error should mention authentication, got: {}",
            reason
        );
    }
}
