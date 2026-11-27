use super::*;

fn job_and_connection() -> (Job, ConnectionState) {
    let mut state = ConnectionState::new("127.0.0.1".parse().unwrap(), true);
    state.first_message = false;
    state.caller_user = Some("alice".into());
    state.verified_user = Some("alice".into());
    let job = Job {
        request: ThoughtTranslationRequest {
            block: ThoughtBlockRef {
                session_key: "agent:main:main".into(),
                run_id: "run-a".into(),
                block_id: ThoughtBlockId("block-a".into()),
            },
            engine: TranslationEngine::Ai,
            target: TranslationTarget::Zh,
            expected_destination: "local/model".into(),
            request_id: "request-a".into(),
        },
        identity: Identity {
            role: Some(state.caller_role.clone()),
            user: state.caller_user.clone(),
            verified: state.verified_user.clone(),
            loopback: true,
        },
        generation: "generation-a".into(),
        cancel: CancellationToken::new(),
        running: true,
    };
    (job, state)
}

#[test]
fn thought_translation_updates_require_current_connection_identity_and_generation() {
    let (job, mut state) = job_and_connection();
    let event = json!({
        "connection_id": "connection-a",
        "request_id": "request-a",
        "generation": "generation-a",
        "update": { "type": "delta", "text": "译文" }
    });

    assert!(job.admits_event(&event, "connection-a", &state));
    assert!(!job.admits_event(&event, "connection-b", &state));
    for key in ["generation", "request_id"] {
        let mut stale = event.clone();
        stale[key] = json!("previous");
        assert!(!job.admits_event(&stale, "connection-a", &state));
    }
    state.verified_user = Some("bob".into());
    assert!(!job.admits_event(&event, "connection-a", &state));
}

#[test]
fn thought_translation_decodes_the_real_mcp_content_envelope() {
    let mut result = json!({
        "content": [{"type": "text", "text": "{\"text\":\"译文\"}"}],
        "isError": false,
    });
    assert_eq!(machine_translation_text(&result).as_deref(), Some("译文"));
    result["content"][0]["text"] = json!("a free-form tool response");
    assert_eq!(machine_translation_text(&result), None);
}

#[test]
fn thought_translation_cancel_drops_queued_text_and_keeps_terminal_failure() {
    let (job, state) = job_and_connection();
    let mut event = json!({
        "connection_id": "connection-a",
        "request_id": "request-a",
        "generation": "generation-a",
        "update": { "type": "completed", "text": "译文" }
    });

    assert!(job.admits_event(&event, "connection-a", &state));
    job.cancel.cancel();
    assert!(!job.admits_event(&event, "connection-a", &state));
    event["update"] = json!({"type": "failed", "code": "cancelled"});
    assert!(job.admits_event(&event, "connection-a", &state));
}
