use super::tests::{req, seed_session, session_store};
use super::thought_source::thought_block_id;
use super::*;
use crate::gateway::caller_identity::CALLER_USER;
use crate::resilience::ThoughtProvenance;
use aleph_protocol::thought::{ThoughtBlockId, ThoughtBlockRef};
use tempfile::TempDir;

async fn fixture() -> (
    TempDir,
    Arc<StateDatabase>,
    Arc<dyn SessionStore>,
    SessionKey,
) {
    let temp = TempDir::new().unwrap();
    let db = Arc::new(StateDatabase::new(temp.path().join("state.db")).unwrap());
    let sessions = session_store(&temp);
    let key = SessionKey::main("thought-source");
    seed_session(&sessions, &key, "alice", &["run-a"]).await;
    db.insert_agent_task(&crate::resilience::AgentTask::new(
        "run-a",
        key.to_key_string(),
        "coder",
        "x",
        crate::resilience::RiskLevel::Low,
    ))
    .await
    .unwrap();
    (temp, db, sessions, key)
}

async fn thought(db: &StateDatabase, text: &str, step: u32, provenance: Option<ThoughtProvenance>) {
    let mut trace = crate::resilience::TaskTrace::new(
        "run-a",
        step,
        aleph_protocol::AgentTraceEvent::ReasoningEmitted {
            iteration: 1,
            text: text.into(),
        },
    );
    trace.thought_provenance = provenance;
    db.insert_trace(&trace).await.unwrap();
}

async fn replay(
    db: Arc<StateDatabase>,
    sessions: Arc<dyn SessionStore>,
    key: &SessionKey,
) -> Value {
    CALLER_USER
        .scope(Some("alice".into()), async {
            handle_by_runs(
                req(json!({"session_key": key.to_key_string(), "run_ids": ["run-a"]})),
                db,
                sessions,
            )
            .await
            .result
            .unwrap()["runs"]["run-a"]
                .clone()
        })
        .await
}

async fn source(
    reference: &ThoughtBlockRef,
    db: &StateDatabase,
    sessions: &dyn SessionStore,
) -> Option<String> {
    CALLER_USER
        .scope(
            Some("alice".into()),
            resolve_thought_source(reference, db, sessions),
        )
        .await
}

fn reference(row: &Value) -> ThoughtBlockRef {
    serde_json::from_value(row["thought_block_ref"].clone())
        .expect("eligible replay must carry a reference")
}

struct FixtureMachine;

#[async_trait::async_trait]
impl crate::tools::handlers::ToolHandler for FixtureMachine {
    fn definition(&self) -> crate::tools::service::ToolDefinition {
        crate::tools::service::ToolDefinition {
            name: "fixture_translate".into(),
            description: "Fixture translation".into(),
            input_schema: json!({"type": "object"}),
            source: crate::tools::service::ToolSource::Mcp {
                server_id: "fixture".into(),
            },
            metadata: Default::default(),
        }
    }

    async fn invoke(
        &self,
        input: Value,
    ) -> Result<crate::tools::service::ToolOutput, crate::tools::service::ToolError> {
        assert_eq!(
            input,
            json!({"text": "owned source", "target_language": "zh"})
        );
        Ok(crate::tools::service::ToolOutput {
            value: json!({"content": [{"type": "text", "text": "{\"text\":\"机器译文\"}"}]}),
            metadata: Default::default(),
        })
    }
}

#[tokio::test]
async fn thought_source_translates_only_owned_blocks_through_registered_engines() {
    use crate::capability::{ownership::OwnershipTree, zahir_facade::ZahirFacade};
    use crate::gateway::{
        caller_identity::with_verified_caller_identity,
        event_bus::TopicEvent,
        handlers::thought_translation,
        server::{ConnectionState, GatewayServer},
    };
    use crate::tools::{handlers::ToolHandler, ToolHandlerRegistry};
    use crate::{config::Config, providers::MockProvider, thinker::MultiProviderRegistry};
    use aleph_protocol::thought::{
        ThoughtTranslationRequest, TranslationEngine, TranslationTarget, THOUGHT_TRANSLATION_TOPIC,
    };

    let (_temp, db, sessions, key) = fixture().await;
    thought(
        &db,
        "owned source",
        0,
        Some(ThoughtProvenance::ProviderResponse),
    )
    .await;
    let block = reference(&replay(db.clone(), sessions.clone(), &key).await[0]);
    let mut gateway = GatewayServer::new("127.0.0.1:0".parse().unwrap());
    let tools = Arc::new(ToolHandlerRegistry::new());
    let machine = Arc::new(FixtureMachine);
    let _machine_handle = tools
        .register(
            aleph_slot_api::descriptor::ToolCapabilityDescriptor::from_definition(
                &machine.definition(),
                0,
            ),
            machine,
        )
        .unwrap();
    let ownership = Arc::new(OwnershipTree::new());
    let facade = ZahirFacade::new((*tools).clone(), ownership.clone());
    let providers = Arc::new(MultiProviderRegistry::new(
        "fixture".into(),
        Arc::new(MockProvider::new("模型译文")),
    ));
    let mut config = Config::default();
    config.mcp.enabled = true;
    config.providers.insert(
        "fixture".into(),
        serde_json::from_value(json!({
            "model": "translate", "enabled": true,
        }))
        .unwrap(),
    );
    let config = Arc::new(tokio::sync::RwLock::new(config));
    thought_translation::install(
        &mut gateway,
        config.clone(),
        providers.clone(),
        tools,
        db,
        sessions,
        facade,
    )
    .unwrap();
    let mut updates = gateway.event_bus().subscribe();
    let handlers = gateway.handlers().clone();
    let connection = || {
        let mut state = ConnectionState::new("127.0.0.1".parse().unwrap(), true);
        state.first_message = false;
        state.caller_user = Some("alice".into());
        state.verified_user = state.caller_user.clone();
        state
    };
    let state = connection();
    gateway
        .connections
        .write()
        .await
        .insert("fixture".into(), connection());
    let call = |request: JsonRpcRequest| {
        let handlers = handlers.clone();
        let role = state.caller_role.clone();
        async move {
            with_verified_caller_identity(
                Some(role),
                Some("alice".into()),
                true,
                Some("fixture".into()),
                Some("alice".into()),
                handlers.handle(&request),
            )
            .await
        }
    };
    let engines = || JsonRpcRequest::new("thought.translation.engines", None, Some(json!(1)));
    assert_eq!(
        call(engines()).await.result,
        Some(json!({"machine": null, "ai": null}))
    );
    {
        let mut config = config.write().await;
        config.thought_translation.machine_tool = Some("fixture_translate".into());
        config.thought_translation.ai_model = Some("fixture/translate".into());
    }
    let destinations: aleph_protocol::thought::ThoughtTranslationEngines =
        serde_json::from_value(call(engines()).await.result.unwrap()).unwrap();
    let ai = destinations.ai.unwrap();
    let machine = destinations.machine.unwrap();
    assert_eq!(ai.label, "fixture/translate");
    assert_eq!(machine.label, "fixture_translate");

    let start = |request: &ThoughtTranslationRequest| {
        JsonRpcRequest::new(
            "thought.translation.start",
            Some(json!(request)),
            Some(json!(2)),
        )
    };
    for (engine, destination, expected) in [
        (TranslationEngine::Ai, ai, "模型译文"),
        (TranslationEngine::Machine, machine, "机器译文"),
    ] {
        let mut request = ThoughtTranslationRequest {
            block: block.clone(),
            engine,
            target: TranslationTarget::Zh,
            request_id: destination.label.clone(),
            expected_destination: destination,
        };
        request.block.run_id = "foreign-run".into();
        assert!(call(start(&request)).await.error.is_some());
        request.block = block.clone();
        let response = call(start(&request)).await;
        assert!(response.error.is_none(), "{response:?}");
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let event: TopicEvent =
                    serde_json::from_str(&updates.recv().await.unwrap()).unwrap();
                if event.topic == THOUGHT_TRANSLATION_TOPIC
                    && event.data["request_id"] == request.request_id.as_str()
                    && event.data["update"]["type"] != "delta"
                {
                    break event;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(event.data["request_id"], request.request_id);
        assert_eq!(
            event.data["update"],
            json!({"type": "completed", "text": expected})
        );
        let service = gateway.thought_translation.as_ref().unwrap();
        assert!(service.event_admits(Some(&event.data), "fixture", &state));
        assert!(!service.event_admits(Some(&event.data), "other-connection", &state));
        match engine {
            TranslationEngine::Ai => {
                let endpoint = wiremock::MockServer::start().await;
                {
                    let mut config = config.write().await;
                    let provider = config.providers.get_mut("fixture").unwrap();
                    provider.base_url = Some(endpoint.uri());
                    let mut transport_config = provider.clone();
                    transport_config.api_key = Some("test-key".into());
                    let transport =
                        crate::providers::create_provider("fixture", transport_config).unwrap();
                    providers.register("fixture".into(), transport);
                }
                assert!(!service.event_admits(Some(&event.data), "fixture", &state));
                request.request_id = "stale-disclosure".into();
                assert!(call(start(&request)).await.error.is_some());
                assert!(endpoint.received_requests().await.unwrap().is_empty());
                let current: aleph_protocol::thought::ThoughtTranslationEngines =
                    serde_json::from_value(call(engines()).await.result.unwrap()).unwrap();
                let current = current.ai.unwrap();
                assert_eq!(current.label, request.expected_destination.label);
                assert_ne!(current.revision, request.expected_destination.revision);
            }
            TranslationEngine::Machine => {
                assert!(
                    ownership.revoke(&crate::capability::descriptor::CapabilityId {
                        namespace: crate::capability::backend::TOOL_NAMESPACE.into(),
                        name: "fixture_translate".into(),
                    })
                );
                assert!(!service.event_admits(Some(&event.data), "fixture", &state));
            }
        }
    }
}

#[tokio::test]
async fn thought_source_rejects_foreign_run() {
    let (_temp, db, sessions, key) = fixture().await;
    thought(
        &db,
        "own source",
        0,
        Some(ThoughtProvenance::ProviderResponse),
    )
    .await;
    let rows = replay(db.clone(), sessions.clone(), &key).await;
    let owned = reference(&rows[0]);
    assert_eq!(
        source(&owned, &db, sessions.as_ref()).await.as_deref(),
        Some("own source")
    );
    let mut foreign = owned.clone();
    foreign.run_id = "foreign-run".into();
    assert!(source(&foreign, &db, sessions.as_ref()).await.is_none());
    let bob = SessionKey::main("bob-thought");
    seed_session(&sessions, &bob, "bob", &["foreign-run"]).await;
    db.insert_agent_task(&crate::resilience::AgentTask::new(
        "foreign-run",
        bob.to_key_string(),
        "coder",
        "x",
        crate::resilience::RiskLevel::Low,
    ))
    .await
    .unwrap();
    let mut trace = crate::resilience::TaskTrace::new(
        "foreign-run",
        0,
        aleph_protocol::AgentTraceEvent::ReasoningEmitted {
            iteration: 1,
            text: "foreign secret".into(),
        },
    );
    trace.thought_provenance = Some(ThoughtProvenance::ProviderResponse);
    db.insert_trace(&trace).await.unwrap();
    let bob_rows = CALLER_USER
        .scope(
            Some("bob".into()),
            handle_by_runs(
                req(json!({"session_key":bob.to_key_string(), "run_ids":["foreign-run"]})),
                db.clone(),
                sessions.clone(),
            ),
        )
        .await
        .result
        .unwrap();
    foreign = reference(&bob_rows["runs"]["foreign-run"][0]);
    assert!(source(&foreign, &db, sessions.as_ref()).await.is_none());
    foreign.session_key = key.to_key_string();
    assert!(source(&foreign, &db, sessions.as_ref()).await.is_none());
    foreign = owned;
    foreign.block_id = ThoughtBlockId("unknown".into());
    assert!(source(&foreign, &db, sessions.as_ref()).await.is_none());
}

#[tokio::test]
async fn thought_source_rejects_unfinished_block() {
    let (_temp, db, sessions, key) = fixture().await;
    use crate::gateway::event_emitter::{CollectingEventEmitter, EventEmitter};
    let emitter = CollectingEventEmitter::new();
    // The same stream-only shape is emitted by slash commands and synthetic
    // execution progress. is_complete never attests a provider response.
    for (seq, content, complete) in [
        (1, "unrecorded thought", false),
        (2, "command reasoning", true),
        (3, "Analyzing the request...", true),
    ] {
        emitter
            .emit(crate::gateway::event_emitter::StreamEvent::Reasoning {
                run_id: "run-a".into(),
                seq,
                content: content.into(),
                is_complete: complete,
            })
            .await
            .unwrap();
    }
    assert_eq!(emitter.events().await.len(), 3);
    assert_eq!(replay(db.clone(), sessions.clone(), &key).await, json!([]));
    let guessed = ThoughtBlockRef {
        session_key: key.to_key_string(),
        run_id: "run-a".into(),
        block_id: ThoughtBlockId("stream-only".into()),
    };
    assert!(source(&guessed, &db, sessions.as_ref()).await.is_none());
}

#[tokio::test]
async fn thought_source_rejects_mixed_or_legacy_rows() {
    let (_temp, db, sessions, key) = fixture().await;
    thought(&db, "legacy reasoning", 0, None).await;
    thought(
        &db,
        "unknown provenance",
        1,
        Some(ThoughtProvenance::Unknown),
    )
    .await;
    let mut mixed = crate::resilience::TaskTrace::new(
        "run-a",
        2,
        aleph_protocol::AgentTraceEvent::ToolSummary {
            iteration: 1,
            summary: "tool summary".into(),
        },
    );
    mixed.thought_provenance = Some(ThoughtProvenance::ProviderResponse);
    db.insert_trace(&mixed).await.unwrap();
    thought(&db, "  ", 3, Some(ThoughtProvenance::ProviderResponse)).await;
    let rows = replay(db.clone(), sessions.clone(), &key).await;
    assert_eq!(rows.as_array().unwrap().len(), 4);
    for row in rows.as_array().unwrap() {
        assert!(row.get("thought_block_ref").is_none());
    }
    for trace in db.get_traces_by_task("run-a").await.unwrap() {
        let guessed = ThoughtBlockRef {
            session_key: key.to_key_string(),
            run_id: "run-a".into(),
            block_id: thought_block_id(&trace),
        };
        assert!(source(&guessed, &db, sessions.as_ref()).await.is_none());
    }
}

#[tokio::test]
async fn two_blocks_in_one_iteration_have_distinct_ids() {
    let (_temp, db, sessions, key) = fixture().await;
    thought(&db, "first", 0, Some(ThoughtProvenance::ProviderResponse)).await;
    thought(&db, "second", 1, Some(ThoughtProvenance::ProviderResponse)).await;
    let rows = replay(db.clone(), sessions.clone(), &key).await;
    let first = reference(&rows[0]);
    let second = reference(&rows[1]);
    assert_ne!(first.block_id, second.block_id);
    assert_eq!(
        source(&first, &db, sessions.as_ref()).await.as_deref(),
        Some("first")
    );
    assert_eq!(
        source(&second, &db, sessions.as_ref()).await.as_deref(),
        Some("second")
    );
    let reopened = Arc::new(StateDatabase::new(_temp.path().join("state.db")).unwrap());
    assert_eq!(rows, replay(reopened, sessions.clone(), &key).await);
}

#[tokio::test]
async fn thought_source_equals_masked_replay_text() {
    let (_temp, db, sessions, key) = fixture().await;
    let secret = "sk-abcdefghijklmnopqrstuvwxyz0123456789";
    let text = crate::exec::SecretMasker::new().mask(&format!("think {secret} carefully"));
    assert!(!text.contains(secret));
    thought(&db, &text, 0, Some(ThoughtProvenance::ProviderResponse)).await;
    let rows = replay(db.clone(), sessions.clone(), &key).await;
    let selected_input = source(&reference(&rows[0]), &db, sessions.as_ref())
        .await
        .unwrap();
    assert_eq!(selected_input, rows[0]["text"].as_str().unwrap());
    assert_eq!(selected_input, text);
}

#[tokio::test]
async fn thought_source_keeps_old_and_unknown_provenance_readable() {
    let (_temp, db, sessions, key) = fixture().await;
    thought(&db, "old visible thought", 0, None).await;
    // Simulate a future provenance spelling in the existing stored JSON format.
    db.with_conn(|conn| {
        conn.execute("UPDATE task_traces SET event_json = json_set(event_json, '$.thought_provenance', 'future_provider')", [])
            .map_err(|e| crate::error::AlephError::config(e.to_string()))?;
        Ok(())
    }).await.unwrap();
    let rows = replay(db.clone(), sessions.clone(), &key).await;
    assert_eq!(rows[0]["text"], "old visible thought");
    assert!(rows[0].get("thought_block_ref").is_none());
    let old = serde_json::json!({ "id":1, "task_id":"run-a", "step_index":0,
        "event": {"kind":"reasoning_emitted", "iteration":1, "text":"legacy"}, "timestamp":0 });
    let trace: crate::resilience::TaskTrace = serde_json::from_value(old).unwrap();
    assert!(trace.thought_provenance.is_none());
}
