//! External-consumer witnesses for the pure AI port, not a production bridge.
//! AI types, JSON and async plumbing intentionally come through the API root.
use aleph_slot_api::{
    async_trait, json, normalize_think_level, serde_json, AiProvider, AiProviderContract,
    AlephError, CacheControl, ContentBlock, DeltaSink, EphemeralTtl, MetadataToolDefinition,
    NativeToolCall, ProviderDelta, ProviderResponse, ReasoningReplay, RequestPayload, Result,
    StopReason, SystemPromptPart, ThinkLevel, ThinkingOrigin, TokenCost, TokenUsage, ToolCategory,
    ToolChoice, UnifiedMessage, Value,
};
use std::borrow::Cow;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};

#[derive(Default)]
struct WakeCount(AtomicUsize);
impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

// The tested mocks/replay do not need an executor. A surprise suspension is a
// failure rather than a spin loop or an unbounded timeout.
fn ready<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(WakeCount::default()));
    match std::pin::pin!(future).poll(&mut Context::from_waker(&waker)) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("expected an immediately ready mock/replay"),
    }
}

struct RecordingSink<'a>(&'a Mutex<Vec<ProviderDelta>>);
#[async_trait]
impl DeltaSink for RecordingSink<'_> {
    async fn on_delta(&self, delta: &ProviderDelta) {
        self.0.lock().unwrap().push(delta.clone());
    }
}

struct Echo;
impl AiProviderContract for Echo {
    fn process<'a>(
        &'a self,
        payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move {
            Ok(ProviderResponse::text_only(format!(
                "{}: {}",
                payload.system_prompt.unwrap_or("system"),
                payload.messages[0].text_content(),
            )))
        })
    }
    fn name(&self) -> &str {
        "external-echo"
    }
    fn color(&self) -> &str {
        "#123456"
    }
}

fn response_facts(response: &ProviderResponse) -> Value {
    json!({
        "text": response.text,
        "tool_calls": response.tool_calls,
        "thinking": response.thinking,
        "thinking_signature": response.thinking_signature,
        "stop_reason": format!("{:?}", response.stop_reason),
        "usage": response.usage.as_ref().map(usage_facts),
        "truncated_tool_call": response.truncated_tool_call,
        "provider_error": response.provider_error,
    })
}

fn usage_facts(usage: &TokenUsage) -> Value {
    json!({
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
        "cache_read_tokens": usage.cache_read_tokens,
        "cache_creation_tokens": usage.cache_creation_tokens,
        "thinking_tokens": usage.thinking_tokens,
        "cost": usage.cost.as_ref().map(|cost| json!({
            "input": cost.input_cost_per_million,
            "output": cost.output_cost_per_million,
        })),
    })
}

fn delta_facts(delta: &ProviderDelta) -> Value {
    match delta {
        ProviderDelta::TextDelta(text) => json!(["text", text]),
        ProviderDelta::ThinkingDelta(text) => json!(["thinking", text]),
        ProviderDelta::ThinkingSignatureDelta(sig) => json!(["signature", sig]),
        ProviderDelta::ToolCallStart {
            id,
            name,
            signature,
        } => json!(["start", id, name, signature]),
        ProviderDelta::ToolCallArgDelta { id, delta } => json!(["args", id, delta]),
        ProviderDelta::ToolCallArgsComplete { id, arguments } => json!(["complete", id, arguments]),
        ProviderDelta::ToolCallEnd { id } => json!(["end", id]),
        ProviderDelta::Usage(usage) => json!(["usage", usage_facts(usage)]),
        ProviderDelta::Done(reason) => json!(["done", format!("{reason:?}")]),
        ProviderDelta::Error(error) => json!(["error", error]),
    }
}

#[test]
fn borrowed_dyn_process_and_default_streaming_deliver_exact_text_only_response() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Box<dyn AiProviderContract>>();
    // The alias must be usable as the same trait object, not just importable.
    let alias: Box<dyn AiProvider> = Box::new(Echo);
    let provider: Box<dyn AiProviderContract> = alias;
    assert_eq!(provider.name(), "external-echo");
    assert_eq!(provider.color(), "#123456");
    assert!(!provider.supports_native_tools());
    assert!(!provider.supports_streaming());
    assert!(!provider.clears_tool_results_server_side());
    assert!(matches!(provider.protocol(), Cow::Borrowed("unknown")));
    assert_eq!(provider.model_behavior_override(), None);
    assert_eq!(provider.behavior_hint(), None);
    assert_eq!(provider.serving_model_hint(), None);
    assert_eq!(provider.serving_provider_hint(), None);
    assert_eq!(provider.reasoning_replay(None), ReasoningReplay::Drop);

    let prompt = String::from("local system");
    let messages = vec![UnifiedMessage::user(String::from("local question"))];
    let expected = json!({
        "text": "local system: local question", "tool_calls": [],
        "thinking": null, "thinking_signature": null, "stop_reason": "EndTurn",
        "usage": null, "truncated_tool_call": null, "provider_error": null,
    });
    let response =
        ready(provider.process(RequestPayload::new(&messages).with_system(Some(&prompt)))).unwrap();
    assert_eq!(response_facts(&response), expected);
    let events = Mutex::new(Vec::new());
    let sink = RecordingSink(&events);
    let immutable_sink: &dyn DeltaSink = &sink;
    let streamed = ready(provider.execute_streaming_dyn(
        RequestPayload::new(&messages).with_system(Some(&prompt)),
        immutable_sink,
    ))
    .unwrap();
    assert_eq!(response_facts(&streamed), expected);
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .map(delta_facts)
            .collect::<Vec<_>>(),
        vec![
            json!(["text", "local system: local question"]),
            json!(["done", "EndTurn"])
        ],
    );
}

#[test]
fn ten_field_payload_and_metadata_tool_borrows_reach_external_provider() {
    struct Inspector<'a> {
        messages: &'a [UnifiedMessage],
        system: &'a str,
        blocks: &'a [SystemPromptPart],
        tools: &'a [MetadataToolDefinition],
    }
    impl AiProviderContract for Inspector<'_> {
        fn process<'a>(
            &'a self,
            payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
            Box::pin(async move {
                // Pointer identity proves the caller's actual buffers arrived.
                assert!(std::ptr::eq(payload.messages, self.messages));
                assert!(std::ptr::eq(payload.system_prompt.unwrap(), self.system));
                assert!(std::ptr::eq(payload.system_blocks.unwrap(), self.blocks));
                assert!(std::ptr::eq(payload.tools.unwrap(), self.tools));
                assert_eq!(payload.system_blocks.unwrap()[0].content, "cacheable");
                assert!(payload.system_blocks.unwrap()[0].cache);
                let tool: &aleph_slot_api::metadata::ToolDefinition = &payload.tools.unwrap()[0];
                assert_eq!(tool.name, "lookup");
                assert_eq!(tool.description, "local metadata tool");
                assert_eq!(tool.parameters, json!({"type": "object"}));
                assert!(tool.strict && tool.requires_confirmation);
                assert_eq!(tool.category, ToolCategory::Builtin);
                assert_eq!(payload.think_level, Some(ThinkLevel::High));
                assert_eq!(payload.temperature, Some(0.25));
                assert_eq!(payload.max_tokens, Some(321));
                assert_eq!(
                    payload.tool_choice,
                    Some(ToolChoice::Specific("lookup".into()))
                );
                assert_eq!(payload.model.as_deref(), Some("local-model"));
                assert_eq!(
                    payload.metadata.unwrap(),
                    HashMap::from([("user_id".into(), "42".into())])
                );
                Ok(ProviderResponse::text_only(tool.name.clone()))
            })
        }
        fn name(&self) -> &str {
            "inspector"
        }
        fn color(&self) -> &str {
            "#abcdef"
        }
    }
    let defaults = RequestPayload::default();
    assert!(defaults.messages.is_empty());
    assert!(
        defaults.system_prompt.is_none()
            && defaults.system_blocks.is_none()
            && defaults.tools.is_none()
    );
    assert!(
        defaults.think_level.is_none()
            && defaults.temperature.is_none()
            && defaults.max_tokens.is_none()
    );
    assert!(
        defaults.tool_choice.is_none() && defaults.model.is_none() && defaults.metadata.is_none()
    );

    let messages = vec![UnifiedMessage::user("borrowed history")];
    let system = String::from("legacy fallback");
    let blocks = vec![SystemPromptPart {
        content: "cacheable".into(),
        cache: true,
    }];
    let tools = vec![MetadataToolDefinition::new(
        "lookup",
        "local metadata tool",
        json!({"type": "object"}),
        ToolCategory::Builtin,
    )
    .with_confirmation(true)
    .with_strict(true)];
    let provider: Box<dyn AiProviderContract + '_> = Box::new(Inspector {
        messages: &messages,
        system: &system,
        blocks: &blocks,
        tools: &tools,
    });
    let payload = RequestPayload::new(&messages)
        .with_system(Some(&system))
        .with_system_blocks(Some(&blocks))
        .with_tools(Some(&tools))
        .with_think_level(Some(ThinkLevel::High))
        .with_temperature(Some(0.25))
        .with_max_tokens(Some(321))
        .with_tool_choice(Some(ToolChoice::Specific("lookup".into())))
        .with_model(Some("local-model".into()))
        .with_metadata(Some(HashMap::from([("user_id".into(), "42".into())])));
    assert_eq!(
        ready(provider.process(payload)).unwrap().text.as_deref(),
        Some("lookup")
    );
}

struct Reply(ProviderResponse);
impl AiProviderContract for Reply {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move { Ok(self.0.clone()) })
    }
    fn name(&self) -> &str {
        "reply"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

#[test]
fn default_streaming_replays_ordered_full_response_without_losing_diagnostics() {
    let expected = ProviderResponse {
        text: Some("answer".into()),
        thinking: Some("reason".into()),
        thinking_signature: Some("signed".into()),
        tool_calls: vec![
            NativeToolCall {
                id: "c1".into(),
                name: "lookup".into(),
                arguments: json!({"q": "x"}),
                thought_signature: Some("tool-sig".into()),
            },
            NativeToolCall {
                id: "c2".into(),
                name: "noop".into(),
                arguments: Value::Null,
                thought_signature: None,
            },
        ],
        stop_reason: StopReason::ToolUse,
        usage: Some(TokenUsage {
            input_tokens: 10,
            output_tokens: 3,
            cache_read_tokens: Some(30),
            cache_creation_tokens: Some(2),
            thinking_tokens: Some(8),
            cost: Some(TokenCost {
                input_cost_per_million: 2.0,
                output_cost_per_million: 4.0,
            }),
        }),
        truncated_tool_call: Some("third: unexpected end of JSON".into()),
        provider_error: Some("upstream fault after partial response".into()),
    };
    let usage = expected.usage.as_ref().unwrap();
    assert_eq!(
        usage.cache_hit_ratio(),
        aleph_slot_api::protocol::cache_hit_ratio(10, Some(30))
    );
    assert_eq!(usage.prompt_tokens_total(), 42);
    assert_eq!(usage.context_occupancy_tokens(), 53);
    assert!((usage.cost.as_ref().unwrap().calculate(usage) - 0.000032).abs() < 1e-12);
    let expected_facts = response_facts(&expected);
    let provider: Box<dyn AiProviderContract> = Box::new(Reply(expected));
    let events = Mutex::new(Vec::new());
    let sink = RecordingSink(&events);
    let response = ready(provider.execute_streaming_dyn(RequestPayload::default(), &sink)).unwrap();
    assert_eq!(response_facts(&response), expected_facts);
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .map(delta_facts)
            .collect::<Vec<_>>(),
        vec![
            json!(["thinking", "reason"]),
            json!(["signature", "signed"]),
            json!(["text", "answer"]),
            json!(["start", "c1", "lookup", "tool-sig"]),
            json!(["args", "c1", "{\"q\":\"x\"}"]),
            json!(["end", "c1"]),
            json!(["start", "c2", "noop", null]),
            json!(["end", "c2"]),
            json!(["usage", expected_facts["usage"]]),
            json!(["done", "ToolUse"]),
        ]
    );
    // Existing behavior: the diagnostic fields remain on the response; replay
    // does NOT turn them into Error deltas. This is not a recovery guarantee.
}

#[test]
fn process_error_propagates_through_default_streaming_without_sink_effects() {
    struct Failing;
    impl AiProviderContract for Failing {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
            Box::pin(async {
                Err(AlephError::ProviderError {
                    message: "external fault".into(),
                    suggestion: Some("try later".into()),
                })
            })
        }
        fn name(&self) -> &str {
            "failing"
        }
        fn color(&self) -> &str {
            "#ff0000"
        }
    }
    let provider: Box<dyn AiProviderContract> = Box::new(Failing);
    let messages = vec![UnifiedMessage::user("local")];
    let events = Mutex::new(Vec::new());
    let sink = RecordingSink(&events);
    for result in [
        ready(provider.process(RequestPayload::new(&messages))),
        ready(provider.execute_streaming_dyn(RequestPayload::new(&messages), &sink)),
    ] {
        match result.unwrap_err() {
            AlephError::ProviderError {
                message,
                suggestion,
            } => {
                assert_eq!(message, "external fault");
                assert_eq!(suggestion.as_deref(), Some("try later"));
            }
            error => panic!("error changed: {error:?}"),
        }
    }
    assert!(events.lock().unwrap().is_empty());
}

#[test]
fn polled_pending_request_drops_once_and_late_wake_has_no_sink_effects() {
    #[derive(Default)]
    struct PendingProvider {
        polls: AtomicUsize,
        drops: AtomicUsize,
        release: AtomicBool,
        waker: Mutex<Option<Waker>>,
    }
    struct PendingRequest<'a> {
        owner: &'a PendingProvider,
        payload: RequestPayload<'a>,
    }
    impl Future for PendingRequest<'_> {
        type Output = Result<ProviderResponse>;
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            self.owner.polls.fetch_add(1, Ordering::SeqCst);
            if self.owner.release.load(Ordering::SeqCst) {
                Poll::Ready(Ok(ProviderResponse::text_only(
                    self.payload.messages[0].text_content(),
                )))
            } else {
                *self.owner.waker.lock().unwrap() = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
    impl Drop for PendingRequest<'_> {
        fn drop(&mut self) {
            self.owner.drops.fetch_add(1, Ordering::SeqCst);
        }
    }
    impl AiProviderContract for PendingProvider {
        fn process<'a>(
            &'a self,
            payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
            Box::pin(PendingRequest {
                owner: self,
                payload,
            })
        }
        fn name(&self) -> &str {
            "pending"
        }
        fn color(&self) -> &str {
            "#888888"
        }
    }
    let owner = PendingProvider::default();
    let provider: &dyn AiProviderContract = &owner;
    let messages = vec![UnifiedMessage::user(String::from("not static"))];
    let events = Mutex::new(Vec::new());
    let sink = RecordingSink(&events);
    let wakes = Arc::new(WakeCount::default());
    let waker = Waker::from(wakes.clone());
    let mut future = provider.execute_streaming_dyn(RequestPayload::new(&messages), &sink);
    assert!(future
        .as_mut()
        .poll(&mut Context::from_waker(&waker))
        .is_pending());
    assert_eq!(owner.polls.load(Ordering::SeqCst), 1);
    assert_eq!(owner.drops.load(Ordering::SeqCst), 0);
    assert!(events.lock().unwrap().is_empty());
    drop(future);
    assert_eq!(owner.drops.load(Ordering::SeqCst), 1);
    owner.release.store(true, Ordering::SeqCst);
    owner.waker.lock().unwrap().take().unwrap().wake();
    assert_eq!(wakes.0.load(Ordering::SeqCst), 1);
    assert_eq!(owner.polls.load(Ordering::SeqCst), 1);
    assert_eq!(owner.drops.load(Ordering::SeqCst), 1);
    assert!(events.lock().unwrap().is_empty());
    // Only local future destruction/replay effects are witnessed. No remote
    // operation was started, and cancellation cannot promise to reverse one.
}

#[test]
fn reasoning_default_uses_explicit_model_then_hint_then_empty_model() {
    struct Hinted<'a>(Option<&'a str>);
    impl AiProviderContract for Hinted<'_> {
        fn process<'a>(
            &'a self,
            payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
            Echo.process(payload)
        }
        fn name(&self) -> &str {
            "hinted"
        }
        fn color(&self) -> &str {
            "#111111"
        }
        fn protocol(&self) -> Cow<'_, str> {
            Cow::Borrowed("anthropic")
        }
        fn serving_model_hint(&self) -> Option<Cow<'_, str>> {
            self.0.map(Cow::Borrowed)
        }
    }
    let model = String::from("anthropic/claude-sonnet-4-5-20250929");
    let hinted: Box<dyn AiProviderContract + '_> = Box::new(Hinted(Some(&model)));
    let strip = ReasoningReplay::AnthropicSigned {
        strip_earlier_turns: true,
    };
    let preserve = ReasoningReplay::AnthropicSigned {
        strip_earlier_turns: false,
    };
    assert_eq!(hinted.reasoning_replay(None), strip);
    assert_eq!(hinted.reasoning_replay(Some("claude-sonnet-4-6")), preserve);
    assert_eq!(hinted.reasoning_replay(Some("")), preserve);
    let no_hint: &dyn AiProviderContract = &Hinted(None);
    assert_eq!(no_hint.reasoning_replay(None), preserve);
    assert_eq!(no_hint.reasoning_replay(Some(&model)), strip);
    assert_eq!(ReasoningReplay::default(), preserve);
}

#[test]
fn all_thinking_aliases_round_trip_and_unknown_values_are_rejected_externally() {
    let aliases = [
        (ThinkLevel::Off, "off none disable disabled 0 false no"),
        (ThinkLevel::Minimal, "minimal min think on enable enabled 1 true yes"),
        (ThinkLevel::Low, "low thinkhard think-hard think_hard 2"),
        (ThinkLevel::Medium, "medium med mid thinkharder think-harder think_harder harder 3"),
        (ThinkLevel::High, "high ultra ultrathink ultra-think ultra_think thinkhardest think-hardest highest max 4"),
        (ThinkLevel::XHigh, "xhigh x-high x_high extended ext 5 extreme"),
    ];
    assert_eq!(ThinkLevel::default(), ThinkLevel::Minimal);
    assert_eq!(aliases.map(|(level, _)| level), ThinkLevel::ALL);
    for (level, names) in aliases {
        for alias in names.split_whitespace() {
            let input = format!("  {}  ", alias.to_uppercase());
            assert_eq!(normalize_think_level(&input), Some(level));
            assert_eq!(input.parse::<ThinkLevel>().unwrap(), level);
        }
        assert_eq!(serde_json::to_value(level).unwrap(), json!(level.id()));
        assert_eq!(level.to_string(), level.id());
    }
    for input in ["", "turbo", "6", "high-ish", "off on"] {
        assert_eq!(normalize_think_level(input), None);
        assert_eq!(
            input.parse::<ThinkLevel>().unwrap_err(),
            format!("Unknown thinking level: '{input}'")
        );
        assert!(serde_json::from_value::<ThinkLevel>(json!(input)).is_err());
    }
    assert_eq!(ThinkLevel::XHigh.display_name(), "Extended");
}

#[test]
fn response_validation_emits_both_diagnostics_to_external_subscriber() {
    use tracing::{span, Event, Metadata, Subscriber};
    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<(String, String)>>>);
    #[derive(Default)]
    struct Fields {
        message: String,
        protocol: String,
    }
    impl tracing::field::Visit for Fields {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            match field.name() {
                "message" => self.message = value.into(),
                "protocol" => self.protocol = value.into(),
                _ => {}
            }
        }
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                self.message = format!("{value:?}");
            }
        }
    }
    impl Subscriber for Capture {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
            span::Id::from_u64(1)
        }
        fn record(&self, _: &span::Id, _: &span::Record<'_>) {}
        fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
        fn event(&self, event: &Event<'_>) {
            assert_eq!(*event.metadata().level(), tracing::Level::WARN);
            let mut fields = Fields::default();
            event.record(&mut fields);
            self.0
                .lock()
                .unwrap()
                .push((fields.message, fields.protocol));
        }
        fn enter(&self, _: &span::Id) {}
        fn exit(&self, _: &span::Id) {}
    }
    let events = Arc::new(Mutex::new(Vec::new()));
    tracing::subscriber::with_default(Capture(events.clone()), || {
        ProviderResponse {
            stop_reason: StopReason::Unknown,
            ..Default::default()
        }
        .validate("external-protocol");
        ProviderResponse {
            usage: Some(TokenUsage::default()),
            ..Default::default()
        }
        .validate("complete");
    });
    assert_eq!(
        *events.lock().unwrap(),
        vec![
            (
                "Provider response missing usage data".into(),
                "external-protocol".into()
            ),
            (
                "Provider response has Unknown stop_reason".into(),
                "external-protocol".into()
            ),
        ]
    );
    let mut response = ProviderResponse {
        tool_calls: vec![NativeToolCall {
            id: " ".into(),
            name: "lookup".into(),
            arguments: json!({}),
            thought_signature: None,
        }],
        ..Default::default()
    };
    assert_eq!(
        response.validate_tool_calls().unwrap_err(),
        "tool call 'lookup' has an empty call_id"
    );
    response.tool_calls[0].id = "c1".into();
    response.tool_calls.push(response.tool_calls[0].clone());
    assert_eq!(
        response.validate_tool_calls().unwrap_err(),
        "provider returned duplicate tool call id 'c1'"
    );
}

#[test]
fn external_projection_borrows_unchanged_messages_and_normalizes_tool_pairs() {
    let messages = vec![
        UnifiedMessage::user("question"),
        UnifiedMessage::Assistant {
            content: vec![
                ContentBlock::Thinking {
                    thinking: "keep encrypted".into(),
                    signature: Some("{\"id\":\"r1\",\"ec\":\"opaque\"}".into()),
                    earlier_turn: true,
                },
                ContentBlock::Thinking {
                    thinking: "drop signed".into(),
                    signature: Some("anthropic-sig".into()),
                    earlier_turn: false,
                },
                ContentBlock::ToolCall {
                    id: "c1".into(),
                    name: "lookup".into(),
                    arguments: json!({"q":"x"}),
                    thought_signature: Some("tool-sig".into()),
                },
            ],
        },
        UnifiedMessage::tool_result("orphan", "lookup", "stray", false),
    ];
    assert_eq!(ThinkingOrigin::of(None), ThinkingOrigin::Unsigned);
    let replay = ReasoningReplay::ResponsesEncrypted;
    let projected: Vec<_> = replay.projected(&messages).collect();
    assert!(
        matches!(&projected[0], Cow::Borrowed(message) if std::ptr::eq(*message, &messages[0]))
    );
    assert!(matches!(&projected[1], Cow::Owned(_)));
    assert_eq!(projected[1].content_blocks().len(), 2);
    assert!(
        matches!(&projected[1].content_blocks()[0], ContentBlock::Thinking { thinking, signature: Some(sig), earlier_turn: true } if thinking == "keep encrypted" && sig.contains("opaque"))
    );
    assert_eq!(
        messages[1].content_blocks().len(),
        3,
        "projection must not mutate source"
    );
    let mut wire = aleph_slot_api::transform_messages(&messages, &replay);
    assert_eq!(wire.len(), 3);
    assert_eq!(
        wire[2].tool_result_info(),
        Some((
            "lookup",
            "No result provided — tool call was interrupted".into()
        ))
    );
    assert!(
        matches!(&wire[2], UnifiedMessage::ToolResult { tool_name, is_error: true, .. } if tool_name == "lookup")
    );
    assert_eq!(wire[1].tool_calls()[0].2, &json!({"q":"x"}));
    let once = serde_json::to_value(&wire).unwrap();
    aleph_slot_api::normalize_tool_pairs(&mut wire);
    assert_eq!(serde_json::to_value(wire).unwrap(), once);
}

#[test]
fn think_dispatch_all_arms_are_lazy_mutate_context_and_return_nonstatic_borrows() {
    use aleph_slot_api::ai::{dispatch_think_level, select_think_level_copy};
    // A full const-evaluated table, not just runtime calls to a const fn.
    const fn budget(level: &ThinkLevel) -> Option<u32> {
        select_think_level_copy(
            level,
            None,
            Some(1024),
            Some(4096),
            Some(10000),
            Some(20000),
            Some(50000),
        )
    }
    const BUDGETS: [Option<u32>; 6] = [
        budget(&ThinkLevel::Off),
        budget(&ThinkLevel::Minimal),
        budget(&ThinkLevel::Low),
        budget(&ThinkLevel::Medium),
        budget(&ThinkLevel::High),
        budget(&ThinkLevel::XHigh),
    ];
    assert_eq!(
        BUDGETS,
        [
            None,
            Some(1024),
            Some(4096),
            Some(10000),
            Some(20000),
            Some(50000)
        ]
    );
    let labels: Vec<String> = ThinkLevel::ALL
        .iter()
        .map(|level| format!("local-{}", level.id()))
        .collect();
    fn arm<'a>(index: usize, labels: &'a [String]) -> impl FnOnce(&mut [usize; 6]) -> &'a str {
        let ticket = format!("owned callback {index}");
        move |hits| {
            drop(ticket);
            hits[index] += 1;
            labels[index].as_str()
        }
    }
    for (index, level) in ThinkLevel::ALL.iter().enumerate() {
        let borrowed = {
            let mut hits = [0; 6];
            let value = dispatch_think_level(
                level,
                &mut hits,
                arm(0, &labels),
                arm(1, &labels),
                arm(2, &labels),
                arm(3, &labels),
                arm(4, &labels),
                arm(5, &labels),
            );
            let mut expected = [0; 6];
            expected[index] = 1;
            assert_eq!(hits, expected);
            value
        };
        assert!(std::ptr::eq(borrowed, labels[index].as_str()));
    }
}

#[test]
fn message_dispatch_all_arms_forward_borrowed_fields_and_only_selected_effects() {
    use aleph_slot_api::ai_message::dispatch_unified_message;
    let messages = [
        UnifiedMessage::user("user"),
        UnifiedMessage::assistant("assistant"),
        UnifiedMessage::tool_result("c1", "lookup", "result", true),
    ];
    for (index, message) in messages.iter().enumerate() {
        let borrowed = {
            let mut hits = [0; 3];
            let content = dispatch_unified_message(
                message,
                &mut hits,
                |hits, content| {
                    hits[0] += 1;
                    content
                },
                |hits, content| {
                    hits[1] += 1;
                    content
                },
                |hits, id, name, content, error| {
                    hits[2] += 1;
                    assert_eq!((id, name, error), ("c1", "lookup", true));
                    if let UnifiedMessage::ToolResult {
                        tool_call_id,
                        tool_name,
                        ..
                    } = message
                    {
                        assert!(std::ptr::eq(id, tool_call_id.as_str()));
                        assert!(std::ptr::eq(name, tool_name.as_str()));
                    }
                    content
                },
            );
            let mut expected = [0; 3];
            expected[index] = 1;
            assert_eq!(hits, expected);
            content
        };
        assert!(std::ptr::eq(borrowed, message.content_blocks()));
    }
}

#[test]
fn content_dispatch_all_arms_preserve_fields_borrowed_output_and_selected_effects() {
    use aleph_slot_api::ai_message::dispatch_content_block;
    let cache = CacheControl::Ephemeral {
        ttl: Some(EphemeralTtl::OneHour),
    };
    let blocks = [
        ContentBlock::Text {
            text: "text".into(),
            cache_control: Some(cache),
        },
        ContentBlock::Json {
            value: json!({"key":"json"}),
        },
        ContentBlock::Thinking {
            thinking: "thinking".into(),
            signature: Some("thinking-sig".into()),
            earlier_turn: true,
        },
        ContentBlock::ToolCall {
            id: "c1".into(),
            name: "lookup".into(),
            arguments: json!({"q":"x"}),
            thought_signature: Some("tool-sig".into()),
        },
        ContentBlock::Image {
            data: "aGk=".into(),
            mime_type: "image/png".into(),
        },
    ];
    for (index, block) in blocks.iter().enumerate() {
        let borrowed = {
            let mut hits = [0; 5];
            let value = dispatch_content_block(
                block,
                &mut hits,
                |hits, text, cc| {
                    hits[0] += 1;
                    assert_eq!(cc, Some(cache));
                    text
                },
                |hits, value| {
                    hits[1] += 1;
                    assert_eq!(value, &json!({"key":"json"}));
                    value["key"].as_str().unwrap()
                },
                |hits, thinking, signature, earlier| {
                    hits[2] += 1;
                    assert_eq!((signature, earlier), (Some("thinking-sig"), true));
                    thinking
                },
                |hits, id, name, args, signature| {
                    hits[3] += 1;
                    assert_eq!((name, signature), ("lookup", Some("tool-sig")));
                    assert_eq!(args, &json!({"q":"x"}));
                    id
                },
                |hits, data, mime| {
                    hits[4] += 1;
                    assert_eq!(mime, "image/png");
                    data
                },
            );
            let mut expected = [0; 5];
            expected[index] = 1;
            assert_eq!(hits, expected);
            value
        };
        assert_eq!(borrowed, ["text", "json", "thinking", "c1", "aGk="][index]);
    }
}
