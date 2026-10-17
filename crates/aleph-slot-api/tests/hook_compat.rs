use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use aleph_slot_api::{
    async_trait, bytes, hooks::*, http, json, protocol, serde_json, sync, validate_hook_request,
    Hook, HookContext, HookDescriptor, HookError, SlotId, Value,
};

// Private test-only call boundary: the same guard -> execute -> protocol
// `validate_response` sequence that the previous public `execute_validated`
// helper exposed, but kept inside the integration-test binary so it never
// becomes a production dispatcher contract. A new `Hook` impl applied with
// the root-reexported `#[async_trait]` attribute above is what it drives.
async fn execute_validated<H: Hook + ?Sized>(
    hook: &H,
    ctx: &HookContext,
) -> Result<HookResponse, HookError> {
    validate_hook_request(hook.descriptor(), &ctx.envelope)?;
    let response = hook.execute(ctx).await?;
    validate_response(&ctx.envelope.event, &response).map_err(|err| HookError {
        message: err.to_string(),
    })?;
    Ok(response)
}

#[test]
fn legacy_hook_golden_unchanged() {
    let expected = [
        (
            HookEvent::BeforeAgentStart,
            "before_agent_start",
            vec!["BeforeAgentStart"],
            MatchSubject::Ignored,
            true,
        ),
        (
            HookEvent::AgentEnd,
            "agent_end",
            vec!["AgentEnd"],
            MatchSubject::Ignored,
            true,
        ),
        (
            HookEvent::BeforeToolCall,
            "before_tool_call",
            vec!["PreToolUse", "BeforeToolCall"],
            MatchSubject::ToolName,
            true,
        ),
        (
            HookEvent::AfterToolCall,
            "after_tool_call",
            vec!["PostToolUse", "AfterToolCall"],
            MatchSubject::ToolName,
            true,
        ),
        (
            HookEvent::AfterToolCallFailure,
            "after_tool_call_failure",
            vec!["PostToolUseFailure", "AfterToolCallFailure"],
            MatchSubject::ToolName,
            true,
        ),
        (
            HookEvent::ToolResultPersist,
            "tool_result_persist",
            vec!["ToolResultPersist"],
            MatchSubject::ToolName,
            false,
        ),
        (
            HookEvent::MessageReceived,
            "message_received",
            vec!["MessageReceived"],
            MatchSubject::Ignored,
            false,
        ),
        (
            HookEvent::MessageSending,
            "message_sending",
            vec!["MessageSending"],
            MatchSubject::Ignored,
            false,
        ),
        (
            HookEvent::MessageSent,
            "message_sent",
            vec!["MessageSent"],
            MatchSubject::Ignored,
            false,
        ),
        (
            HookEvent::SessionStart,
            "session_start",
            vec!["SessionStart"],
            MatchSubject::SessionSource,
            true,
        ),
        (
            HookEvent::SessionEnd,
            "session_end",
            vec!["SessionEnd"],
            MatchSubject::Ignored,
            false,
        ),
        (
            HookEvent::BeforeCompaction,
            "before_compaction",
            vec!["PreCompact", "BeforeCompaction"],
            MatchSubject::Ignored,
            true,
        ),
        (
            HookEvent::AfterCompaction,
            "after_compaction",
            vec!["AfterCompaction", "PostCompact"],
            MatchSubject::Ignored,
            false,
        ),
        (
            HookEvent::PreApiRequest,
            "pre_api_request",
            vec!["PreApiRequest"],
            MatchSubject::Ignored,
            false,
        ),
        (
            HookEvent::PostApiRequest,
            "post_api_request",
            vec!["PostApiRequest"],
            MatchSubject::Ignored,
            false,
        ),
        (
            HookEvent::GatewayStart,
            "gateway_start",
            vec!["GatewayStart"],
            MatchSubject::Ignored,
            false,
        ),
        (
            HookEvent::GatewayStop,
            "gateway_stop",
            vec!["GatewayStop"],
            MatchSubject::Ignored,
            false,
        ),
        (
            HookEvent::Notification,
            "notification",
            vec!["Notification"],
            MatchSubject::ToolName,
            false,
        ),
        (
            HookEvent::PermissionRequest,
            "permission_request",
            vec!["PermissionRequest"],
            MatchSubject::ToolName,
            false,
        ),
        (
            HookEvent::PermissionDenied,
            "permission_denied",
            vec!["PermissionDenied"],
            MatchSubject::ToolName,
            false,
        ),
        (
            HookEvent::UserPromptSubmit,
            "user_prompt_submit",
            vec!["UserPromptSubmit"],
            MatchSubject::Ignored,
            true,
        ),
        (
            HookEvent::SubagentStart,
            "subagent_start",
            vec!["SubagentStart"],
            MatchSubject::Ignored,
            false,
        ),
        (
            HookEvent::SubagentStop,
            "subagent_stop",
            vec!["SubagentStop"],
            MatchSubject::Ignored,
            false,
        ),
        (
            HookEvent::Stop,
            "stop",
            vec!["Stop"],
            MatchSubject::Ignored,
            true,
        ),
    ];
    assert_eq!(HookEvent::ALL.len(), 24);
    assert_eq!(HookEvent::ALL, expected.each_ref().map(|e| e.0));
    for (event, canonical, aliases, subject, interceptor) in expected {
        assert_eq!(event.canonical_name(), canonical);
        assert_eq!(serde_json::to_value(event).unwrap(), json!(canonical));
        assert_eq!(
            serde_json::from_value::<HookEvent>(json!(canonical)).unwrap(),
            event
        );
        for alias in aliases {
            assert_eq!(
                serde_json::from_value::<HookEvent>(json!(alias)).unwrap(),
                event
            );
            assert_eq!(
                serde_json::from_str::<HookEvent>(&json!(alias).to_string()).unwrap(),
                event
            );
        }
        assert_eq!(event.match_subject(), subject);
        assert_eq!(event.supports_interceptor(), interceptor);
    }
    assert_eq!(MatchSubject::ToolName.as_str(), "tool_name");
    assert_eq!(MatchSubject::SessionSource.as_str(), "session_source");
    assert_eq!(MatchSubject::Ignored.as_str(), "ignored");
    assert_eq!(SESSION_SOURCE_STARTUP, "startup");
    assert_eq!(SESSION_SOURCES_FIRED, ["startup"]);
}

#[test]
fn legacy_kind_priority_defaults_and_parsing() {
    assert_eq!(HookKind::default(), HookKind::Observer);
    for (name, kind) in [
        ("observer", HookKind::Observer),
        ("interceptor", HookKind::Interceptor),
    ] {
        assert_eq!(serde_json::to_value(kind).unwrap(), json!(name));
        assert_eq!(
            serde_json::from_value::<HookKind>(json!(name)).unwrap(),
            kind
        );
        assert_eq!(name.to_uppercase().parse::<HookKind>().unwrap(), kind);
        assert_eq!(HookKind::from_str_or_default(name), kind);
    }
    for unknown in ["resolver", "future", ""] {
        assert_eq!(HookKind::from_str_or_default(unknown), HookKind::Observer);
        assert_eq!(
            unknown.parse::<HookKind>().unwrap_err(),
            format!("Unknown hook kind: {unknown}")
        );
        assert!(serde_json::from_value::<HookKind>(json!(unknown)).is_err());
    }
    let priorities = [
        HookPriority::System,
        HookPriority::High,
        HookPriority::Normal,
        HookPriority::Low,
    ];
    assert_eq!(priorities.map(|p| p.as_i32()), [-1000, -100, 0, 100]);
    assert_eq!(priorities.map(|p| p as i32), [-1000, -100, 0, 100]);
    assert_eq!(HookPriority::default(), HookPriority::Normal);
    for (priority, name) in priorities
        .into_iter()
        .zip(["system", "high", "normal", "low"])
    {
        assert_eq!(serde_json::to_value(priority).unwrap(), json!(name));
        assert_eq!(
            serde_json::from_value::<HookPriority>(json!(name)).unwrap(),
            priority
        );
        assert_eq!(
            name.to_uppercase().parse::<HookPriority>().unwrap(),
            priority
        );
        assert_eq!(HookPriority::from_str_or_default(name), priority);
    }
    assert!(priorities.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(
        HookPriority::from_str_or_default("future"),
        HookPriority::Normal
    );
    assert_eq!(
        "future".parse::<HookPriority>().unwrap_err(),
        "Unknown hook priority: future"
    );
}

#[test]
fn legacy_actions_all_shapes_unchanged() {
    let fixtures = [
        json!({"type":"command","command":"printf '%s' \"$TOOL_INPUT\""}),
        json!({"type":"prompt","prompt":"context template"}),
        json!({"type":"agent","agent":"reviewer"}),
        json!({"type":"http","url":"https://example.invalid/hook"}),
        json!({"type":"http","url":"https://example.invalid/hook","headers":{"x-context":"${CONTEXT}"}}),
        json!({"type":"plugin","plugin_id":"plugin-a","handler":"on_tool"}),
    ];
    for fixture in fixtures {
        let action: HookAction = serde_json::from_value(fixture.clone()).unwrap();
        assert_eq!(serde_json::to_value(&action).unwrap(), fixture);
        match action {
            HookAction::Command { command } => assert_eq!(command, fixture["command"]),
            HookAction::Prompt { prompt } => assert_eq!(prompt, fixture["prompt"]),
            HookAction::Agent { agent } => assert_eq!(agent, fixture["agent"]),
            HookAction::Http { url, headers } => {
                assert_eq!(url, fixture["url"]);
                assert_eq!(
                    serde_json::to_value(headers).unwrap(),
                    fixture.get("headers").cloned().unwrap_or(json!({}))
                );
            }
            HookAction::Plugin { plugin_id, handler } => {
                assert_eq!(plugin_id, fixture["plugin_id"]);
                assert_eq!(handler, fixture["handler"]);
            }
        }
    }
    let empty_headers: HookAction =
        serde_json::from_value(json!({"type":"http","url":"u","headers":{}})).unwrap();
    assert_eq!(
        serde_json::to_value(empty_headers).unwrap(),
        json!({"type":"http","url":"u"})
    );
    // Legacy actions intentionally retain their existing permissive unknown-field behavior.
    let legacy: HookAction =
        serde_json::from_value(json!({"type":"command","command":"c","extra":true})).unwrap();
    assert_eq!(
        serde_json::to_value(legacy).unwrap(),
        json!({"type":"command","command":"c"})
    );
}

#[test]
fn hook_protocol_reexports_have_one_rust_identity() {
    fn roundtrip(value: HookEnvelope) -> protocol::hook::HookEnvelope {
        value
    }
    fn reverse(value: protocol::hook::HookEnvelope) -> HookEnvelope {
        value
    }
    assert_eq!(
        std::any::TypeId::of::<HookEnvelope>(),
        std::any::TypeId::of::<protocol::hook::HookEnvelope>()
    );
    assert_eq!(
        std::any::TypeId::of::<HookInput>(),
        std::any::TypeId::of::<protocol::hook::HookInput>()
    );
    assert_eq!(
        std::any::TypeId::of::<HookResponse>(),
        std::any::TypeId::of::<protocol::hook::HookResponse>()
    );
    let value = HookEnvelope {
        version: HOOK_CONTRACT_VERSION,
        session_id: "s".into(),
        turn_id: Some("t".into()),
        slot: "slot".into(),
        event: HookInput::ToolPost {
            call_id: "c".into(),
            tool: "tool".into(),
            result: Value::Null,
        },
    };
    assert_eq!(reverse(roundtrip(value.clone())), value);
}

// =============================================================================
// Task 6 — typed Hook@1 contract: Slot identity, typed context, guard, and
// the test-only call boundary. The 7 event tags each get a real
// envelope/response pair; the guard rejects wrong descriptor version, wrong
// event tag, direct envelope version mismatch, and invalid envelope facts
// before the hook is even called. The test-only call boundary validates
// the response shape after execution and only lets a consumer counter
// advance on `Ok`.
// =============================================================================

fn make_observer_turn_envelope(session_id: &str, turn_id: &str) -> HookEnvelope {
    HookEnvelope {
        version: HOOK_CONTRACT_VERSION,
        session_id: session_id.into(),
        turn_id: Some(turn_id.into()),
        slot: "slot-a".into(),
        event: HookInput::Observer {
            lifecycle: ObserverLifecycle::TurnStart {
                context: TurnLifecycleContext {
                    session_id: session_id.into(),
                    turn_id: turn_id.into(),
                },
            },
        },
    }
}

fn make_tool_pre_envelope() -> HookEnvelope {
    HookEnvelope {
        version: HOOK_CONTRACT_VERSION,
        session_id: "s1".into(),
        turn_id: Some("t1".into()),
        slot: "slot-a".into(),
        event: HookInput::ToolPre {
            call_id: "call-1".into(),
            tool: "tool-1".into(),
            input: json!({}),
            original_input: json!({}),
        },
    }
}

fn make_tool_pre_descriptor() -> HookDescriptor {
    HookDescriptor {
        id: "h-tool-pre".into(),
        slot: SlotId("slot-a".into()),
        event: HookEventTag::ToolPre,
        kind: HookKind::Interceptor,
        priority: HookPriority::Normal,
        contract_version: ContractVersion(HOOK_CONTRACT_VERSION),
    }
}

struct CountedHook {
    descriptor: HookDescriptor,
    response: HookResponse,
    counter: Arc<AtomicUsize>,
}

#[async_trait]
impl Hook for CountedHook {
    fn descriptor(&self) -> &HookDescriptor {
        &self.descriptor
    }
    async fn execute(&self, _ctx: &HookContext) -> Result<HookResponse, HookError> {
        self.counter.fetch_add(1, Ordering::SeqCst);
        Ok(self.response.clone())
    }
}

fn counted(descriptor: HookDescriptor, response: HookResponse) -> (CountedHook, Arc<AtomicUsize>) {
    let counter = Arc::new(AtomicUsize::new(0));
    let hook = CountedHook {
        descriptor,
        response,
        counter: counter.clone(),
    };
    (hook, counter)
}

#[test]
fn typed_hook_context_matches_wire() {
    let envelope = make_tool_pre_envelope();
    let descriptor = make_tool_pre_descriptor();
    let ctx = HookContext {
        envelope: envelope.clone(),
    };
    assert_eq!(ctx.envelope, envelope);
    assert_eq!(descriptor.event, envelope.event.event_tag());
    assert_eq!(descriptor.event, HookEventTag::ToolPre);
    assert_eq!(descriptor.contract_version.0, HOOK_CONTRACT_VERSION);
    assert_eq!(
        ContractVersion(HOOK_CONTRACT_VERSION).0,
        HOOK_CONTRACT_VERSION
    );
    validate_hook_request(&descriptor, &ctx.envelope)
        .expect("valid descriptor+envelope must pass the guard");
}

#[test]
fn slot_id_and_contract_version_newtype_serde() {
    let slot = SlotId("slot-a".into());
    let cv = ContractVersion(HOOK_CONTRACT_VERSION);
    assert_eq!(serde_json::to_value(&slot).unwrap(), json!("slot-a"));
    assert_eq!(
        serde_json::from_value::<SlotId>(json!("slot-b")).unwrap(),
        SlotId("slot-b".into())
    );
    assert_eq!(
        serde_json::to_value(cv).unwrap(),
        json!(HOOK_CONTRACT_VERSION)
    );
    assert_eq!(
        serde_json::from_value::<ContractVersion>(json!(HOOK_CONTRACT_VERSION)).unwrap(),
        ContractVersion(HOOK_CONTRACT_VERSION)
    );
    assert_eq!(slot.to_string(), "slot-a");
    assert_eq!(cv.to_string(), HOOK_CONTRACT_VERSION.to_string());
}

#[test]
fn hook_descriptor_serde_is_deny_unknown_fields() {
    let descriptor = make_tool_pre_descriptor();
    let value = serde_json::to_value(&descriptor).unwrap();
    assert_eq!(value["id"], "h-tool-pre");
    assert_eq!(value["slot"], "slot-a");
    assert_eq!(value["event"], "tool.pre");
    assert_eq!(value["kind"], "interceptor");
    assert_eq!(value["priority"], "normal");
    assert_eq!(value["contract_version"], HOOK_CONTRACT_VERSION);
    let bogus = json!({
        "id": "h",
        "slot": "s",
        "event": "tool.pre",
        "kind": "interceptor",
        "priority": "normal",
        "contract_version": 1,
        "extra_field": true,
    });
    let err = serde_json::from_value::<HookDescriptor>(bogus).unwrap_err();
    assert!(
        err.to_string().contains("extra_field"),
        "deny_unknown_fields should reject the extra field, got: {err}"
    );
}

#[test]
fn seventag_correspondences_match_between_input_and_response() {
    struct Case {
        tag: HookEventTag,
        envelope: HookEnvelope,
        response: HookResponse,
    }
    let cases: Vec<Case> = vec![
        Case {
            tag: HookEventTag::ToolPre,
            envelope: HookEnvelope {
                version: HOOK_CONTRACT_VERSION,
                session_id: "s".into(),
                turn_id: Some("t".into()),
                slot: "slot".into(),
                event: HookInput::ToolPre {
                    call_id: "c".into(),
                    tool: "tool".into(),
                    input: json!({}),
                    original_input: json!({}),
                },
            },
            response: HookResponse::ToolPre {
                action: ToolPreAction::Continue {
                    updated_input: None,
                },
            },
        },
        Case {
            tag: HookEventTag::ToolPost,
            envelope: HookEnvelope {
                version: HOOK_CONTRACT_VERSION,
                session_id: "s".into(),
                turn_id: Some("t".into()),
                slot: "slot".into(),
                event: HookInput::ToolPost {
                    call_id: "c".into(),
                    tool: "tool".into(),
                    result: Value::Null,
                },
            },
            response: HookResponse::ToolPost { replacement: None },
        },
        Case {
            tag: HookEventTag::Prompt,
            envelope: HookEnvelope {
                version: HOOK_CONTRACT_VERSION,
                session_id: "s".into(),
                turn_id: Some("t".into()),
                slot: "slot".into(),
                event: HookInput::Prompt {
                    layers: vec![PromptLayerView {
                        id: "layer-replaceable".into(),
                        text: "orig".into(),
                        replaceability: Replaceability::Replaceable,
                    }],
                },
            },
            response: HookResponse::Prompt {
                append: vec!["appended".into()],
                replace: vec![ReplaceLayerRequest {
                    replaceable_layer_id: "layer-replaceable".into(),
                    text: "new".into(),
                }],
            },
        },
        Case {
            tag: HookEventTag::Compaction,
            envelope: HookEnvelope {
                version: HOOK_CONTRACT_VERSION,
                session_id: "s".into(),
                turn_id: Some("t".into()),
                slot: "slot".into(),
                event: HookInput::Compaction {
                    messages: vec![CompactionMessage {
                        id: "m1".into(),
                        role: "user".into(),
                        text: "hello".into(),
                        tool_call_id: None,
                    }],
                    required_pairs: vec![],
                    required_session_markers: vec![],
                },
            },
            response: HookResponse::Compaction {
                summary: CompactionSummary {
                    text: "summary".into(),
                    preserved_pairs: vec![],
                    session_markers: vec![],
                },
            },
        },
        Case {
            tag: HookEventTag::Continue,
            envelope: HookEnvelope {
                version: HOOK_CONTRACT_VERSION,
                session_id: "s".into(),
                turn_id: Some("t".into()),
                slot: "slot".into(),
                event: HookInput::Continue {
                    iteration: 0,
                    limit: 5,
                },
            },
            response: HookResponse::Continue {
                request: false,
                reason: None,
            },
        },
        Case {
            tag: HookEventTag::IngestFilter,
            envelope: HookEnvelope {
                version: HOOK_CONTRACT_VERSION,
                session_id: "s".into(),
                turn_id: Some("t".into()),
                slot: "slot".into(),
                event: HookInput::IngestFilter {
                    records: vec![IngestRecord {
                        id: "rec-1".into(),
                        content: json!({}),
                    }],
                },
            },
            response: HookResponse::IngestFilter {
                keep_ids: vec!["rec-1".into()],
            },
        },
        Case {
            tag: HookEventTag::Observer,
            envelope: make_observer_turn_envelope("s", "t"),
            response: HookResponse::Observer {
                ack: ObserverAck {
                    records: vec!["observed".into()],
                },
            },
        },
    ];
    assert_eq!(cases.len(), 7, "all 7 event tags must be covered");
    for case in &cases {
        assert_eq!(case.envelope.event.event_tag(), case.tag);
        validate_response(&case.envelope.event, &case.response)
            .unwrap_or_else(|e| panic!("validate_response for {:?} should pass: {e}", case.tag));
    }
}

#[test]
fn box_dyn_hook_is_send_sync() {
    fn assert_send_sync<T: Send + Sync + ?Sized>() {}
    assert_send_sync::<Box<dyn Hook>>();
    assert_send_sync::<dyn Hook>();
}

#[test]
fn root_reexports_are_visible_to_consumers() {
    // The protocol reexport at the root is the same Rust type as the
    // underlying module path: this pins `aleph_slot_api::protocol` and
    // `aleph_slot_api::protocol::hook::HookEnvelope` to one `TypeId`.
    assert_eq!(
        std::any::TypeId::of::<HookEnvelope>(),
        std::any::TypeId::of::<protocol::hook::HookEnvelope>()
    );
    // `bytes` / `http` / `sync` reexported at the root compile at
    // consumer scope; an actual value is constructed so the types are
    // exercised, not merely named.
    let _b: bytes::Bytes = bytes::Bytes::from_static(b"root-reexport");
    let _h: http::HeaderName = http::HeaderName::from_static("x-root-reexport");
    let _l: sync::RwLock<()> = sync::RwLock::new(());
}

#[test]
fn validate_hook_request_rejects_wrong_descriptor_version() {
    let envelope = make_tool_pre_envelope();
    let mut descriptor = make_tool_pre_descriptor();
    descriptor.contract_version = ContractVersion(999);
    let err = validate_hook_request(&descriptor, &envelope).unwrap_err();
    let display = err.to_string();
    assert!(
        display.contains("999"),
        "Display must surface the bad version: {display}"
    );
    assert!(
        display.contains("1"),
        "Display must surface HOOK_CONTRACT_VERSION=1: {display}"
    );
}

#[test]
fn validate_hook_request_rejects_wrong_event_tag() {
    let envelope = make_tool_pre_envelope();
    let mut descriptor = make_tool_pre_descriptor();
    descriptor.event = HookEventTag::ToolPost;
    let err = validate_hook_request(&descriptor, &envelope).unwrap_err();
    let display = err.to_string();
    assert!(
        display.contains("ToolPost"),
        "Display must show descriptor event: {display}"
    );
    assert!(
        display.contains("ToolPre"),
        "Display must show envelope event tag: {display}"
    );
}

#[test]
fn validate_hook_request_rejects_direct_unsupported_envelope_version() {
    let mut envelope = make_tool_pre_envelope();
    envelope.version = 999;
    let descriptor = make_tool_pre_descriptor();
    let err = validate_hook_request(&descriptor, &envelope).unwrap_err();
    let display = err.to_string();
    assert!(
        display.contains("999"),
        "Display must surface the bad version: {display}"
    );
}

#[test]
fn validate_hook_request_rejects_invalid_envelope_facts() {
    let mut envelope = make_tool_pre_envelope();
    envelope.session_id = String::new();
    let descriptor = make_tool_pre_descriptor();
    let err = validate_hook_request(&descriptor, &envelope).unwrap_err();
    let display = err.to_string();
    assert!(
        display.contains("session_id"),
        "Display must surface the invalid fact: {display}"
    );
}

#[tokio::test]
async fn pre_execute_helper_blocks_invocation_before_execute() {
    struct Case {
        label: &'static str,
        envelope: HookEnvelope,
        descriptor: HookDescriptor,
    }
    let good = make_tool_pre_envelope();
    let good_desc = make_tool_pre_descriptor();
    let mut bad_version_envelope = good.clone();
    bad_version_envelope.version = 999;
    let mut bad_version_descriptor = good_desc.clone();
    bad_version_descriptor.contract_version = ContractVersion(999);
    let mut bad_tag_descriptor = good_desc.clone();
    bad_tag_descriptor.event = HookEventTag::ToolPost;
    let mut bad_envelope = good.clone();
    bad_envelope.session_id = String::new();

    let cases = vec![
        Case {
            label: "wrong descriptor version",
            envelope: good.clone(),
            descriptor: bad_version_descriptor,
        },
        Case {
            label: "wrong event tag",
            envelope: good.clone(),
            descriptor: bad_tag_descriptor,
        },
        Case {
            label: "direct envelope version mismatch",
            envelope: bad_version_envelope,
            descriptor: good_desc.clone(),
        },
        Case {
            label: "invalid envelope fact",
            envelope: bad_envelope,
            descriptor: good_desc,
        },
    ];
    for case in cases {
        let response = HookResponse::ToolPre {
            action: ToolPreAction::Continue {
                updated_input: None,
            },
        };
        let (hook, counter) = counted(case.descriptor, response);
        let ctx = HookContext {
            envelope: case.envelope,
        };
        let consumer = Arc::new(AtomicUsize::new(0));
        let result = execute_validated(&hook, &ctx).await;
        if result.is_ok() {
            consumer.fetch_add(1, Ordering::SeqCst);
        }
        assert!(
            result.is_err(),
            "[{}] helper must return Err on guard failure, got Ok",
            case.label
        );
        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "[{}] execute() must not have been called",
            case.label
        );
        assert_eq!(
            consumer.load(Ordering::SeqCst),
            0,
            "[{}] consumer counter must not advance on Err",
            case.label
        );
    }
}

#[tokio::test]
async fn post_execute_validator_blocks_cross_event_response() {
    let envelope = make_tool_pre_envelope();
    let descriptor = make_tool_pre_descriptor();
    let response = HookResponse::Observer {
        ack: ObserverAck {
            records: vec!["x".into()],
        },
    };
    let (hook, counter) = counted(descriptor, response);
    let ctx = HookContext { envelope };
    let consumer = Arc::new(AtomicUsize::new(0));
    let result = execute_validated(&hook, &ctx).await;
    if result.is_ok() {
        consumer.fetch_add(1, Ordering::SeqCst);
    }
    assert!(result.is_err(), "cross-event response must be rejected");
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "execute() was called once; only the response validator rejected"
    );
    assert_eq!(
        consumer.load(Ordering::SeqCst),
        0,
        "consumer counter must not advance when response is rejected"
    );
}

#[tokio::test]
async fn post_execute_validator_blocks_illegal_pinned_replacement() {
    let envelope = HookEnvelope {
        version: HOOK_CONTRACT_VERSION,
        session_id: "s".into(),
        turn_id: Some("t".into()),
        slot: "slot".into(),
        event: HookInput::Prompt {
            layers: vec![
                PromptLayerView {
                    id: "layer-pinned".into(),
                    text: "orig".into(),
                    replaceability: Replaceability::Pinned,
                },
                PromptLayerView {
                    id: "layer-replaceable".into(),
                    text: "orig".into(),
                    replaceability: Replaceability::Replaceable,
                },
            ],
        },
    };
    let descriptor = HookDescriptor {
        id: "h-prompt".into(),
        slot: SlotId("slot".into()),
        event: HookEventTag::Prompt,
        kind: HookKind::Interceptor,
        priority: HookPriority::Normal,
        contract_version: ContractVersion(HOOK_CONTRACT_VERSION),
    };
    let response = HookResponse::Prompt {
        append: vec![],
        replace: vec![ReplaceLayerRequest {
            replaceable_layer_id: "layer-pinned".into(),
            text: "new".into(),
        }],
    };
    let (hook, counter) = counted(descriptor, response);
    let ctx = HookContext { envelope };
    let consumer = Arc::new(AtomicUsize::new(0));
    let result = execute_validated(&hook, &ctx).await;
    if result.is_ok() {
        consumer.fetch_add(1, Ordering::SeqCst);
    }
    assert!(
        result.is_err(),
        "replacement of a Pinned layer must be rejected by validate_response"
    );
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    assert_eq!(
        consumer.load(Ordering::SeqCst),
        0,
        "consumer counter must not advance on illegal pinned replacement"
    );
}

#[tokio::test]
async fn post_execute_validator_accepts_valid_prompt_replacement() {
    let envelope = HookEnvelope {
        version: HOOK_CONTRACT_VERSION,
        session_id: "s".into(),
        turn_id: Some("t".into()),
        slot: "slot".into(),
        event: HookInput::Prompt {
            layers: vec![PromptLayerView {
                id: "layer-replaceable".into(),
                text: "orig".into(),
                replaceability: Replaceability::Replaceable,
            }],
        },
    };
    let descriptor = HookDescriptor {
        id: "h-prompt".into(),
        slot: SlotId("slot".into()),
        event: HookEventTag::Prompt,
        kind: HookKind::Interceptor,
        priority: HookPriority::Normal,
        contract_version: ContractVersion(HOOK_CONTRACT_VERSION),
    };
    let response = HookResponse::Prompt {
        append: vec!["appended".into()],
        replace: vec![ReplaceLayerRequest {
            replaceable_layer_id: "layer-replaceable".into(),
            text: "new".into(),
        }],
    };
    let (hook, counter) = counted(descriptor, response);
    let ctx = HookContext { envelope };
    let consumer = Arc::new(AtomicUsize::new(0));
    let result = execute_validated(&hook, &ctx).await;
    if result.is_ok() {
        consumer.fetch_add(1, Ordering::SeqCst);
    }
    assert!(
        result.is_ok(),
        "valid replacement must be accepted, got: {result:?}"
    );
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    assert_eq!(
        consumer.load(Ordering::SeqCst),
        1,
        "consumer counter advances only on Ok"
    );
}

#[tokio::test]
async fn observer_cannot_control_execution_via_non_observer_response() {
    let envelope = make_observer_turn_envelope("s", "t");
    let descriptor = HookDescriptor {
        id: "h-observer".into(),
        slot: SlotId("slot-a".into()),
        event: HookEventTag::Observer,
        kind: HookKind::Observer,
        priority: HookPriority::Low,
        contract_version: ContractVersion(HOOK_CONTRACT_VERSION),
    };
    let response = HookResponse::ToolPre {
        action: ToolPreAction::Deny {
            reason: "hostile".into(),
        },
    };
    let (hook, counter) = counted(descriptor, response);
    let ctx = HookContext { envelope };
    let consumer = Arc::new(AtomicUsize::new(0));
    let result = execute_validated(&hook, &ctx).await;
    if result.is_ok() {
        consumer.fetch_add(1, Ordering::SeqCst);
    }
    assert!(
        result.is_err(),
        "Observer must not produce a non-Observer response"
    );
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    assert_eq!(
        consumer.load(Ordering::SeqCst),
        0,
        "consumer counter must not advance on rejected response"
    );
}

#[tokio::test]
async fn observer_only_ack_is_the_only_accepted_response_shape() {
    let envelope = make_observer_turn_envelope("s", "t");
    let descriptor = HookDescriptor {
        id: "h-observer".into(),
        slot: SlotId("slot-a".into()),
        event: HookEventTag::Observer,
        kind: HookKind::Observer,
        priority: HookPriority::Low,
        contract_version: ContractVersion(HOOK_CONTRACT_VERSION),
    };
    let response = HookResponse::Observer {
        ack: ObserverAck {
            records: vec!["observed-1".into(), "observed-2".into()],
        },
    };
    let (hook, counter) = counted(descriptor, response);
    let ctx = HookContext { envelope };
    let consumer = Arc::new(AtomicUsize::new(0));
    let result = execute_validated(&hook, &ctx).await;
    if result.is_ok() {
        consumer.fetch_add(1, Ordering::SeqCst);
    }
    assert!(
        result.is_ok(),
        "Observer response with non-empty Ack records must be accepted, got: {result:?}"
    );
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    assert_eq!(consumer.load(Ordering::SeqCst), 1);
}
