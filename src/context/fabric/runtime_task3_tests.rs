use super::*;
use crate::context::budget::preflight::PreflightStage;
use crate::context::budget::pressure::estimate_tokens_smart;
use crate::context::budget::ContextPressure;
use crate::extension::capability::{ContextGrant, ContextGrantApproval};
use crate::extension::registry::{ContextRegistration, PluginRegistry};
use crate::extension::types::{PluginKind, PluginOrigin, PluginRecord};
use crate::providers::message::{ContentBlock, UnifiedMessage};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const SESSION: &str = "session-a";

fn caller() -> AuthenticatedCaller {
    AuthenticatedCaller::from_host_boundary(SESSION).unwrap()
}

fn descriptor(
    id: &str,
    phase: ContextPhase,
    mode: ContextCapabilityMode,
) -> ContextCapabilityDescriptor {
    ContextCapabilityDescriptor {
        id: id.into(),
        api_version: CONTEXT_CAPABILITY_API_VERSION.into(),
        implementation_version: "1.0.0".into(),
        phases: vec![phase],
        mode,
        scope: ContextScope {
            namespace: ContextNamespace::Session,
            handles: vec![],
        },
        input_budget: ContextBudget::new(10_000, 100_000),
        output_budget: ContextBudget::new(10_000, 100_000),
        persistence: ContextPersistencePolicy::default(),
        permissions: ContextSideEffectPermissions::default(),
        lifecycle: vec![],
        bundle_id: None,
    }
}

async fn registry_with(
    registrations: Vec<(&str, ContextCapabilityDescriptor)>,
) -> Arc<RwLock<PluginRegistry>> {
    let mut registry = PluginRegistry::new();
    for (plugin, _) in &registrations {
        registry.register_plugin(PluginRecord::new(
            (*plugin).into(),
            (*plugin).into(),
            PluginKind::Wasm,
            PluginOrigin::Config,
        ));
    }
    let now = unix_seconds();
    for (plugin, descriptor) in registrations {
        let grant = ContextGrant::host_issue(
            &descriptor,
            plugin,
            &caller(),
            ContextGrantApproval {
                expires_at: now + 300,
                audit_reason: "runtime test approval".into(),
                ..Default::default()
            },
            now,
        )
        .unwrap();
        registry
            .register_context(ContextRegistration::new(descriptor, grant, None), now)
            .unwrap();
    }
    Arc::new(RwLock::new(registry))
}

fn pressure() -> ContextPressure {
    ContextPressure {
        used_tokens: 900,
        budget_tokens: 1_000,
        ratio: 0.9,
        overhead_tokens: 0,
        available_for_messages: 1_000,
    }
}

fn output(value: Value) -> ContextTransformResult {
    let bytes = serde_json::to_vec(&value).unwrap();
    let tokens = estimate_tokens_smart(std::str::from_utf8(&bytes).unwrap()) as u64;
    ContextTransformResult {
        input_fingerprint: String::new(),
        output: value,
        output_bytes: bytes.len() as u64,
        output_tokens: tokens,
        strategy: "test".into(),
        continuation: None,
        quality: ContextQuality::Exact,
    }
}

fn messages_output(messages: Vec<UnifiedMessage>) -> ContextTransformResult {
    output(serde_json::to_value(messages).unwrap())
}

#[derive(Clone)]
struct Handler {
    transform: Arc<
        dyn Fn(ContextCapabilityRequest) -> Result<ContextTransformResult, String> + Send + Sync,
    >,
}

#[async_trait]
impl ContextHandler for Handler {
    async fn invoke(
        &self,
        request: ContextCapabilityRequest,
    ) -> Result<ContextTransformResult, String> {
        let mut result = (self.transform)(request.clone())?;
        result.input_fingerprint = request.input_fingerprint.unwrap_or_default();
        Ok(result)
    }
}

fn handler(
    transform: impl Fn(ContextCapabilityRequest) -> Result<ContextTransformResult, String>
        + Send
        + Sync
        + 'static,
) -> Arc<dyn ContextHandler> {
    Arc::new(Handler {
        transform: Arc::new(transform),
    })
}

struct NativeStage {
    transform: Arc<dyn Fn(&mut Vec<UnifiedMessage>, usize) + Send + Sync>,
}

#[async_trait]
impl PreflightStage for NativeStage {
    fn name(&self) -> &'static str {
        "task3-native"
    }

    async fn prepare(
        &self,
        messages: &mut Vec<UnifiedMessage>,
        _pressure: &ContextPressure,
        fresh_tail_count: usize,
    ) -> usize {
        (self.transform)(messages, fresh_tail_count);
        1
    }
}

fn native_stage(
    transform: impl Fn(&mut Vec<UnifiedMessage>, usize) + Send + Sync + 'static,
) -> NativeStage {
    NativeStage {
        transform: Arc::new(transform),
    }
}

fn text(message: &UnifiedMessage) -> String {
    message.text_content()
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[tokio::test]
async fn runtime_uses_descriptor_id_and_live_second_based_grant() {
    let descriptor = descriptor(
        "compact",
        ContextPhase::Preflight,
        ContextCapabilityMode::Replace,
    );
    let seen_id = Arc::new(Mutex::new(None));
    let seen_id_for_handler = Arc::clone(&seen_id);
    let registry = registry_with(vec![("plugin-a", descriptor)]).await;
    let fabric = ContextFabric::builder(Arc::clone(&registry))
        .handler(
            "plugin-a:compact",
            handler(move |request| {
                *seen_id_for_handler.lock().unwrap() = Some(request.capability_id.clone());
                Ok(messages_output(
                    request
                        .snapshot
                        .unwrap()
                        .payload
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| serde_json::from_value(v.clone()).unwrap())
                        .collect(),
                ))
            }),
        )
        .build();

    let mut messages = vec![UnifiedMessage::user("keep me")];
    let outcome = fabric
        .preflight(&mut messages, &pressure(), 0, &caller(), None)
        .await
        .unwrap();

    assert_eq!(
        outcome.owner,
        ContextOwner::Plugin("plugin-a:compact".into())
    );
    assert_eq!(seen_id.lock().unwrap().as_deref(), Some("compact"));
}

#[tokio::test]
async fn ingress_drops_thinking_and_tool_signatures_before_plugin_receives_snapshot() {
    let descriptor = descriptor(
        "preflight",
        ContextPhase::Preflight,
        ContextCapabilityMode::Replace,
    );
    let payload_seen = Arc::new(Mutex::new(None));
    let payload_for_handler = Arc::clone(&payload_seen);
    let registry = registry_with(vec![("plugin-a", descriptor)]).await;
    let fabric = ContextFabric::builder(registry)
        .handler(
            "plugin-a:preflight",
            handler(move |request| {
                *payload_for_handler.lock().unwrap() = Some(request.snapshot.unwrap().payload);
                Ok(messages_output(vec![UnifiedMessage::user("ok")]))
            }),
        )
        .build();

    let mut messages = vec![
        UnifiedMessage::Assistant {
            content: vec![
                ContentBlock::Thinking {
                    thinking: "private-thinking".into(),
                    signature: Some("thinking-signature".into()),
                    earlier_turn: false,
                },
                ContentBlock::ToolCall {
                    id: "call-1".into(),
                    name: "tool".into(),
                    arguments: serde_json::json!({}),
                    thought_signature: Some("tool-signature".into()),
                },
            ],
        },
        UnifiedMessage::tool_result_json("call-1", "tool", serde_json::json!("result"), false),
    ];
    fabric
        .preflight(&mut messages, &pressure(), 0, &caller(), None)
        .await
        .unwrap();

    let encoded = serde_json::to_string(&payload_seen.lock().unwrap().clone().unwrap()).unwrap();
    assert!(!encoded.contains("private-thinking"));
    assert!(!encoded.contains("thinking-signature"));
    assert!(!encoded.contains("tool-signature"));
}

#[tokio::test]
async fn successful_replace_is_followed_by_compose_and_skips_native() {
    let native_calls = Arc::new(Mutex::new(0usize));
    let native_calls_for_stage = Arc::clone(&native_calls);
    let replace = descriptor(
        "replace",
        ContextPhase::Preflight,
        ContextCapabilityMode::Replace,
    );
    let compose = descriptor(
        "compose",
        ContextPhase::Preflight,
        ContextCapabilityMode::Compose,
    );
    let registry = registry_with(vec![("plugin-a", replace), ("plugin-b", compose)]).await;
    let fabric = ContextFabric::builder(registry)
        .native_preflight_stage(native_stage(move |messages, _| {
            *native_calls_for_stage.lock().unwrap() += 1;
            *messages = vec![UnifiedMessage::user("native")];
        }))
        .handler(
            "plugin-a:replace",
            handler(|_| Ok(messages_output(vec![UnifiedMessage::user("replace")]))),
        )
        .handler(
            "plugin-b:compose",
            handler(|request| {
                let mut messages: Vec<UnifiedMessage> =
                    serde_json::from_value(request.snapshot.unwrap().payload).unwrap();
                messages.push(UnifiedMessage::user("compose"));
                Ok(messages_output(messages))
            }),
        )
        .build();

    let mut messages = vec![UnifiedMessage::user("original")];
    let outcome = fabric
        .preflight(&mut messages, &pressure(), 0, &caller(), None)
        .await
        .unwrap();

    assert_eq!(*native_calls.lock().unwrap(), 0);
    assert_eq!(
        outcome.owner,
        ContextOwner::Plugin("plugin-b:compose".into())
    );
    assert_eq!(
        messages.iter().map(text).collect::<Vec<_>>(),
        ["replace", "compose"]
    );
}

#[tokio::test]
async fn replace_failure_preserves_native_fallback_and_tail() {
    let native_calls = Arc::new(Mutex::new(0usize));
    let native_calls_for_stage = Arc::clone(&native_calls);
    let replace = descriptor(
        "replace",
        ContextPhase::Preflight,
        ContextCapabilityMode::Replace,
    );
    let registry = registry_with(vec![("plugin-a", replace)]).await;
    let fabric = ContextFabric::builder(registry)
        .native_preflight_stage(native_stage(move |messages, fresh_tail| {
            *native_calls_for_stage.lock().unwrap() += 1;
            let cut = messages.len().saturating_sub(fresh_tail);
            for message in messages.iter_mut().take(cut) {
                *message = UnifiedMessage::user("native");
            }
        }))
        .handler("plugin-a:replace", handler(|_| Err("plugin failed".into())))
        .build();

    let mut messages = vec![UnifiedMessage::user("old"), UnifiedMessage::user("fresh")];
    let outcome = fabric
        .preflight(&mut messages, &pressure(), 1, &caller(), None)
        .await
        .unwrap();

    assert_eq!(*native_calls.lock().unwrap(), 1);
    assert_eq!(
        outcome.owner,
        ContextOwner::BuiltinFallback {
            plugin: "plugin-a:replace".into(),
            reason: DegradedReason::Unavailable
        }
    );
    assert_eq!(
        messages.iter().map(text).collect::<Vec<_>>(),
        ["native", "fresh"]
    );
}

#[tokio::test]
async fn cancellation_leaves_messages_unmodified() {
    let replace = descriptor(
        "replace",
        ContextPhase::Preflight,
        ContextCapabilityMode::Replace,
    );
    let registry = registry_with(vec![("plugin-a", replace)]).await;
    let fabric = ContextFabric::builder(registry)
        .timeout(Duration::from_secs(5))
        .handler(
            "plugin-a:replace",
            Arc::new(BlockingHandler {
                entered: Arc::new(tokio::sync::Notify::new()),
            }),
        )
        .build();
    let mut messages = vec![UnifiedMessage::user("original")];
    let before = messages.clone();
    let cancel = CancellationToken::new();
    cancel.cancel();

    let result = fabric
        .preflight(&mut messages, &pressure(), 0, &caller(), Some(&cancel))
        .await;

    assert!(matches!(result, Err(ContextFabricError::Cancelled)));
    assert_eq!(
        serde_json::to_value(&messages).unwrap(),
        serde_json::to_value(&before).unwrap()
    );
}

struct BlockingHandler {
    entered: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl ContextHandler for BlockingHandler {
    async fn invoke(
        &self,
        _request: ContextCapabilityRequest,
    ) -> Result<ContextTransformResult, String> {
        self.entered.notify_one();
        std::future::pending::<()>().await;
        unreachable!()
    }
}

struct CountedSummaryProvider {
    calls: Arc<std::sync::atomic::AtomicUsize>,
    delay: Duration,
}

impl crate::providers::AiProvider for CountedSummaryProvider {
    fn name(&self) -> &str {
        "counted-summary"
    }
    fn color(&self) -> &str {
        "#000000"
    }
    fn process(
        &self,
        _payload: crate::providers::adapter::RequestPayload<'_>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = crate::error::Result<crate::providers::adapter::ProviderResponse>,
                > + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            Ok(crate::providers::adapter::ProviderResponse::text_only(
                "Summary of the older exchanges.",
            ))
        })
    }
}

fn summary_compactor(
    calls: Arc<std::sync::atomic::AtomicUsize>,
    delay: Duration,
) -> Arc<crate::context::compact::compactor::ContextCompactor> {
    Arc::new(crate::context::compact::compactor::ContextCompactor::new(
        Arc::new(CountedSummaryProvider { calls, delay }),
        crate::context::compact::compactor::CompactorConfig {
            fresh_tail: 2,
            ..Default::default()
        },
    ))
}

fn history() -> Vec<UnifiedMessage> {
    (0..12)
        .map(|i| {
            let body = format!("exchange {i}: {}", "information ".repeat(60));
            if i % 2 == 0 {
                UnifiedMessage::user(body)
            } else {
                UnifiedMessage::assistant(body)
            }
        })
        .collect()
}

#[tokio::test]
async fn compaction_replace_then_compose_skips_native_and_preserves_configured_and_transient_tail()
{
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let registry = registry_with(vec![
        (
            "plugin-a",
            descriptor(
                "replace",
                ContextPhase::Compaction,
                ContextCapabilityMode::Replace,
            ),
        ),
        (
            "plugin-b",
            descriptor(
                "compose",
                ContextPhase::Compaction,
                ContextCapabilityMode::Compose,
            ),
        ),
    ])
    .await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let replacement_seen = Arc::clone(&seen);
    let compose_seen = Arc::clone(&seen);
    let fabric = ContextFabric::builder(registry)
        .compactor(summary_compactor(Arc::clone(&calls), Duration::ZERO))
        .handler(
            "plugin-a:replace",
            handler(move |_| {
                replacement_seen.lock().unwrap().push("replace");
                Ok(messages_output(vec![UnifiedMessage::user(
                    "replacement summary",
                )]))
            }),
        )
        .handler(
            "plugin-b:compose",
            handler(move |_| {
                compose_seen.lock().unwrap().push("compose");
                Ok(messages_output(vec![UnifiedMessage::user(
                    "composed summary",
                )]))
            }),
        )
        .build();
    let mut messages = history();
    let protected = serde_json::to_value(&messages[8..]).unwrap();
    let outcome = fabric
        .compact(&mut messages, 0, 2, Some(SESSION), &caller(), None)
        .await
        .unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(*seen.lock().unwrap(), ["replace", "compose"]);
    assert_eq!(
        serde_json::to_value(&messages[messages.len() - 4..]).unwrap(),
        protected
    );
    assert_eq!(
        outcome.owner,
        ContextOwner::Plugin("plugin-b:compose".into())
    );
    assert!(matches!(
        outcome.result.strategy_used,
        crate::context::compact::compactor::CompactStrategy::Plugin { .. }
    ));
}

#[tokio::test]
async fn a_plugin_timeout_does_not_time_out_the_native_summary_fallback() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let registry = registry_with(vec![(
        "plugin-a",
        descriptor(
            "replace",
            ContextPhase::Compaction,
            ContextCapabilityMode::Replace,
        ),
    )])
    .await;
    let fabric = ContextFabric::builder(registry)
        .compactor(summary_compactor(
            Arc::clone(&calls),
            Duration::from_millis(20),
        ))
        .timeout(Duration::from_millis(1))
        .handler(
            "plugin-a:replace",
            Arc::new(BlockingHandler {
                entered: Arc::new(tokio::sync::Notify::new()),
            }),
        )
        .build();
    let mut messages = history();
    let protected = serde_json::to_value(&messages[10..]).unwrap();
    let outcome = fabric
        .compact(&mut messages, 0, 0, Some(SESSION), &caller(), None)
        .await
        .unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(
        serde_json::to_value(&messages[messages.len() - 2..]).unwrap(),
        protected
    );
    assert!(matches!(
        outcome.owner,
        ContextOwner::BuiltinFallback { .. }
    ));
    assert!(!matches!(
        outcome.result.strategy_used,
        crate::context::compact::compactor::CompactStrategy::Plugin { .. }
    ));
}

#[tokio::test]
async fn midflight_cancellation_rolls_back_the_plugin_and_does_not_call_native() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let entered = Arc::new(tokio::sync::Notify::new());
    let registry = registry_with(vec![(
        "plugin-a",
        descriptor(
            "replace",
            ContextPhase::Compaction,
            ContextCapabilityMode::Replace,
        ),
    )])
    .await;
    let fabric = ContextFabric::builder(registry)
        .compactor(summary_compactor(Arc::clone(&calls), Duration::ZERO))
        .handler(
            "plugin-a:replace",
            Arc::new(BlockingHandler {
                entered: Arc::clone(&entered),
            }),
        )
        .build();
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    let mut messages = history();
    let before = serde_json::to_value(&messages).unwrap();
    let stopper = async move {
        entered.notified().await;
        trigger.cancel();
    };
    let authenticated = caller();
    let (_, result) = tokio::join!(
        stopper,
        fabric.compact(
            &mut messages,
            0,
            0,
            Some(SESSION),
            &authenticated,
            Some(&cancel)
        )
    );
    assert!(matches!(result, Err(ContextFabricError::Cancelled)));
    assert_eq!(serde_json::to_value(&messages).unwrap(), before);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
}
