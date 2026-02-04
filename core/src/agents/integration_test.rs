//! Integration tests for the sub-agent system.

use std::sync::Arc;

use crate::agents::{AgentRegistry, TaskTool};
use crate::components::SubAgentHandler;
use crate::event::{
    AlephEvent, EventBus, EventContext, EventHandler, SubAgentRequest, SubAgentResult,
};

fn create_test_setup() -> (Arc<AgentRegistry>, Arc<EventBus>, SubAgentHandler, TaskTool) {
    let registry = Arc::new(AgentRegistry::with_builtins());
    let bus = Arc::new(EventBus::new());
    let handler = SubAgentHandler::new(Arc::clone(&registry));
    let tool = TaskTool::new(Arc::clone(&registry), Arc::clone(&bus));
    (registry, bus, handler, tool)
}

fn create_context(bus: &EventBus) -> EventContext {
    EventContext::new(bus.clone())
}

#[tokio::test]
async fn test_full_subagent_lifecycle() {
    let (_registry, bus, handler, tool) = create_test_setup();
    let mut subscriber = bus.subscribe();
    let ctx = create_context(&bus);

    // 1. Call TaskTool to start a sub-agent
    let args = serde_json::json!({
        "agent": "explore",
        "prompt": "Find all Rust files"
    });
    let result = tool.execute(args, "parent-session").await.unwrap();
    assert!(result.success);

    // 2. Receive the SubAgentStarted event
    let event = subscriber.recv().await.unwrap();
    let request = match &event.event {
        AlephEvent::SubAgentStarted(req) => req.clone(),
        _ => panic!("Expected SubAgentStarted"),
    };

    // 3. Handler processes the event
    handler.handle(&event.event, &ctx).await.unwrap();
    assert!(handler.is_session_active(&request.child_session_id).await);

    // 4. Simulate sub-agent iterations
    for i in 0..5 {
        let count = handler.increment_iteration(&request.child_session_id).await;
        assert_eq!(count, Some(i + 1));
    }

    // 5. Sub-agent completes
    let completion = SubAgentResult {
        agent_id: request.agent_id.clone(),
        child_session_id: request.child_session_id.clone(),
        summary: "Found 10 Rust files".into(),
        success: true,
        error: None,
        request_id: None,
        tools_called: vec![],
        execution_duration_ms: None,
    };

    bus.publish(AlephEvent::SubAgentCompleted(completion.clone()))
        .await;

    // 6. Handler processes completion
    let complete_event = subscriber.recv().await.unwrap();
    handler.handle(&complete_event.event, &ctx).await.unwrap();

    // 7. Session is no longer active
    assert!(!handler.is_session_active(&request.child_session_id).await);
}

#[tokio::test]
async fn test_tool_filter_by_agent() {
    let (registry, _, _, _) = create_test_setup();

    // Test explore agent tool filtering
    let explore = registry.get("explore").unwrap();
    assert!(explore.is_tool_allowed("glob"));
    assert!(explore.is_tool_allowed("grep"));
    assert!(explore.is_tool_allowed("read_file"));
    assert!(!explore.is_tool_allowed("write_file"));
    assert!(!explore.is_tool_allowed("bash"));

    // Test coder agent tool filtering
    let coder = registry.get("coder").unwrap();
    assert!(coder.is_tool_allowed("write_file"));
    assert!(coder.is_tool_allowed("edit_file"));

    // Test researcher agent tool filtering
    let researcher = registry.get("researcher").unwrap();
    assert!(researcher.is_tool_allowed("search"));
    assert!(researcher.is_tool_allowed("web_fetch"));
    assert!(!researcher.is_tool_allowed("bash"));
}

#[tokio::test]
async fn test_max_iterations_enforcement() {
    let (_registry, bus, handler, _) = create_test_setup();
    let ctx = create_context(&bus);

    // Start explore agent (max_iterations = 20)
    let request = SubAgentRequest {
        agent_id: "explore".into(),
        prompt: "test".into(),
        parent_session_id: "parent".into(),
        child_session_id: "test-child".into(),
    };
    handler
        .handle(&AlephEvent::SubAgentStarted(request.clone()), &ctx)
        .await
        .unwrap();

    // Iterate up to max
    for _ in 0..19 {
        assert!(!handler.has_exceeded_max_iterations("test-child").await);
        handler.increment_iteration("test-child").await;
    }

    // At iteration 19, not yet exceeded
    assert!(!handler.has_exceeded_max_iterations("test-child").await);

    // At iteration 20, exceeded
    handler.increment_iteration("test-child").await;
    assert!(handler.has_exceeded_max_iterations("test-child").await);
}

#[tokio::test]
async fn test_nested_subagent_tracking() {
    let (_, bus, handler, _) = create_test_setup();
    let ctx = create_context(&bus);

    // Start first sub-agent
    let request1 = SubAgentRequest {
        agent_id: "explore".into(),
        prompt: "Find files".into(),
        parent_session_id: "main-session".into(),
        child_session_id: "child-1".into(),
    };
    handler
        .handle(&AlephEvent::SubAgentStarted(request1), &ctx)
        .await
        .unwrap();

    // Start second sub-agent from the first
    let request2 = SubAgentRequest {
        agent_id: "researcher".into(),
        prompt: "Research topic".into(),
        parent_session_id: "child-1".into(),
        child_session_id: "child-2".into(),
    };
    handler
        .handle(&AlephEvent::SubAgentStarted(request2), &ctx)
        .await
        .unwrap();

    // Both sessions active
    assert!(handler.is_session_active("child-1").await);
    assert!(handler.is_session_active("child-2").await);

    // Parent tracking
    assert_eq!(
        handler.get_parent_session("child-1").await,
        Some("main-session".into())
    );
    assert_eq!(
        handler.get_parent_session("child-2").await,
        Some("child-1".into())
    );

    // Complete inner first
    let result2 = SubAgentResult {
        agent_id: "researcher".into(),
        child_session_id: "child-2".into(),
        summary: "Research complete".into(),
        success: true,
        error: None,
        request_id: None,
        tools_called: vec![],
        execution_duration_ms: None,
    };
    handler
        .handle(&AlephEvent::SubAgentCompleted(result2), &ctx)
        .await
        .unwrap();

    assert!(!handler.is_session_active("child-2").await);
    assert!(handler.is_session_active("child-1").await);

    // Complete outer
    let result1 = SubAgentResult {
        agent_id: "explore".into(),
        child_session_id: "child-1".into(),
        summary: "Exploration complete".into(),
        success: true,
        error: None,
        request_id: None,
        tools_called: vec![],
        execution_duration_ms: None,
    };
    handler
        .handle(&AlephEvent::SubAgentCompleted(result1), &ctx)
        .await
        .unwrap();

    assert!(!handler.is_session_active("child-1").await);
}

#[tokio::test]
async fn test_subagent_failure_tracking() {
    let (_, bus, handler, _) = create_test_setup();
    let ctx = create_context(&bus);

    let request = SubAgentRequest {
        agent_id: "coder".into(),
        prompt: "Write code".into(),
        parent_session_id: "parent".into(),
        child_session_id: "child".into(),
    };
    handler
        .handle(&AlephEvent::SubAgentStarted(request), &ctx)
        .await
        .unwrap();

    // Sub-agent fails
    let result = SubAgentResult {
        agent_id: "coder".into(),
        child_session_id: "child".into(),
        summary: "".into(),
        success: false,
        error: Some("Tool execution failed".into()),
        request_id: None,
        tools_called: vec![],
        execution_duration_ms: None,
    };
    handler
        .handle(&AlephEvent::SubAgentCompleted(result), &ctx)
        .await
        .unwrap();

    // Session still cleaned up on failure
    assert!(!handler.is_session_active("child").await);
}

#[tokio::test]
async fn test_builtin_agent_prompts_loaded() {
    let (registry, _, _, _) = create_test_setup();

    let main = registry.get("main").unwrap();
    assert!(!main.system_prompt.is_empty());
    assert!(main.system_prompt.contains("main assistant"));

    let explore = registry.get("explore").unwrap();
    assert!(!explore.system_prompt.is_empty());
    assert!(explore.system_prompt.contains("exploration"));

    let coder = registry.get("coder").unwrap();
    assert!(!coder.system_prompt.is_empty());
    assert!(coder.system_prompt.contains("coding"));

    let researcher = registry.get("researcher").unwrap();
    assert!(!researcher.system_prompt.is_empty());
    assert!(researcher.system_prompt.contains("research"));
}

#[tokio::test]
async fn test_task_tool_schema() {
    let (_, _, _, tool) = create_test_setup();

    let schema = tool.parameters_schema();

    // Verify schema structure
    assert_eq!(schema["type"], "object");
    assert!(schema["properties"]["agent"].is_object());
    assert!(schema["properties"]["prompt"].is_object());

    // Verify available agents in enum
    let enum_values = schema["properties"]["agent"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect::<Vec<_>>();

    assert!(enum_values.contains(&"explore".to_string()));
    assert!(enum_values.contains(&"coder".to_string()));
    assert!(enum_values.contains(&"researcher".to_string()));
    // main should not be in the list (it's Primary)
    assert!(!enum_values.contains(&"main".to_string()));
}

#[tokio::test]
async fn test_concurrent_subagent_sessions() {
    let (_, bus, handler, _) = create_test_setup();
    let ctx = create_context(&bus);

    // Start multiple sub-agents concurrently
    let requests: Vec<SubAgentRequest> = (0..5)
        .map(|i| SubAgentRequest {
            agent_id: "explore".into(),
            prompt: format!("Task {}", i),
            parent_session_id: "main".into(),
            child_session_id: format!("child-{}", i),
        })
        .collect();

    for request in &requests {
        handler
            .handle(&AlephEvent::SubAgentStarted(request.clone()), &ctx)
            .await
            .unwrap();
    }

    // All sessions should be active
    for i in 0..5 {
        assert!(handler.is_session_active(&format!("child-{}", i)).await);
    }

    // Complete them in reverse order
    for i in (0..5).rev() {
        let result = SubAgentResult {
            agent_id: "explore".into(),
            child_session_id: format!("child-{}", i),
            summary: format!("Completed {}", i),
            success: true,
            error: None,
            request_id: None,
            tools_called: vec![],
            execution_duration_ms: None,
        };
        handler
            .handle(&AlephEvent::SubAgentCompleted(result), &ctx)
            .await
            .unwrap();
    }

    // All sessions should be inactive
    for i in 0..5 {
        assert!(!handler.is_session_active(&format!("child-{}", i)).await);
    }
}
