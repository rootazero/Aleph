use crate::gateway::handlers::{context_breakdown, tool_output, trace_replay};
use crate::gateway::router::SessionKey;
use crate::gateway::session_store::{types::MessageRecord, SessionStore};
use crate::resilience::{AgentTask, RiskLevel, TaskStatus, TaskTrace};
use crate::session::events::{SessionEvent, ToolOutput};
use crate::session::store::{install_test_event_store, SessionEventStore};
use crate::sync_primitives::Arc;
use aleph_protocol::{
    AgentTraceEvent, AgentTraceToolCallEnd, AgentTraceToolCallStart, AgentTraceToolResult,
};
use serde_json::json;

use crate::gateway::GatewayServer;

pub(super) async fn mount_addressed_reads(
    gateway: &mut GatewayServer,
    sessions: Arc<dyn SessionStore>,
    key: &SessionKey,
    foreign: &SessionKey,
) {
    let store = install_test_event_store();
    store
        .append(
            key,
            1,
            &SessionEvent::RunStarted {
                run_id: "browser-run".into(),
                at: 1,
                project_root: None,
                envelope: None,
                telecom_provenance: Default::default(),
            },
            1,
        )
        .await
        .unwrap();
    store
        .append(
            key,
            2,
            &SessionEvent::ToolResult {
                turn_id: uuid::Uuid::new_v4(),
                call_id: "browser-result".into(),
                output: ToolOutput {
                    value: serde_json::json!("browser-owned output"),
                    metadata: Default::default(),
                },
                at: 1,
            },
            1,
        )
        .await
        .unwrap();
    crate::thinker::prompt_size_registry::install_test_prompt_size_registry().record_turn(
        &key.to_key_string(),
        None,
        vec![],
    );

    let db = Arc::new(crate::resilience::StateDatabase::in_memory().unwrap());
    for (session, run, call) in [
        (key, "browser-run", "browser-result"),
        (foreign, "foreign-run", "foreign-result"),
    ] {
        sessions
            .append_message(
                session,
                MessageRecord {
                    id: uuid::Uuid::new_v4().to_string(),
                    role: "assistant".into(),
                    content: "fixture".into(),
                    timestamp: 1,
                    metadata: Some(json!({"run_id": run})),
                    input_tokens: 0,
                    output_tokens: 0,
                    tool_call_id: None,
                    tool_name: None,
                },
            )
            .await
            .unwrap();
        db.insert_agent_task(&AgentTask::new(
            run,
            session.to_key_string(),
            "test",
            "fixture",
            RiskLevel::Low,
        ))
        .await
        .unwrap();
        db.update_task_status(run, TaskStatus::Completed)
            .await
            .unwrap();
        let input = json!({"path":"src/example.rs", "private":"raw input excluded"});
        // Insert in the opposite order to the expected transcript.
        db.insert_trace(&TaskTrace::new(
            run,
            2,
            AgentTraceEvent::ToolCallCompleted {
                iteration: 0,
                call: AgentTraceToolCallEnd {
                    tool_id: call.into(),
                    tool_name: "file_read".into(),
                    input: input.clone(),
                    duration_ms: 3,
                    presentation: None,
                },
                result: AgentTraceToolResult::Success {
                    output: json!("raw output excluded"),
                },
            },
        ))
        .await
        .unwrap();
        db.insert_trace(&TaskTrace::new(
            run,
            1,
            AgentTraceEvent::ToolCallStarted {
                iteration: 0,
                call: AgentTraceToolCallStart {
                    tool_id: call.into(),
                    tool_name: "file_read".into(),
                    input,
                },
            },
        ))
        .await
        .unwrap();
    }
    let replay_sessions = sessions.clone();
    gateway.handlers_mut().register(
        aleph_protocol::chat_transcript::CHAT_TRANSCRIPT_DETAILS_METHOD,
        move |request| {
            trace_replay::handle_transcript_details(request, db.clone(), replay_sessions.clone())
        },
    );
    let output_sessions = sessions.clone();
    gateway
        .handlers_mut()
        .register("trace.tool_output", move |request| {
            tool_output::handle_tool_output(request, output_sessions.clone())
        });
    gateway
        .handlers_mut()
        .register("context.breakdown", move |request| {
            context_breakdown::handle_context_breakdown(request, sessions.clone(), None)
        });
    // Register deliberately to distinguish denied mount from a missing handler.
    gateway
        .handlers_mut()
        .register("trace.by_runs", |req| async move {
            crate::gateway::protocol::JsonRpcResponse::success(req.id, serde_json::json!({}))
        });
}
