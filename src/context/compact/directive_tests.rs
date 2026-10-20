use super::*;
use crate::context::budget::preflight::PreflightStage;
use crate::context::budget::{ContextBudgetConfig, ContextPressure};
use crate::context::fabric::{
    ContextBudget as WireBudget, ContextCapabilityDescriptor, ContextCapabilityMode,
    ContextCapabilityRequest, ContextHandler, ContextNamespace, ContextPersistencePolicy,
    ContextPhase, ContextQuality, ContextScope, ContextSideEffectPermissions,
    ContextTransformResult, CONTEXT_CAPABILITY_API_VERSION,
};
use crate::extension::capability::{ContextGrant, ContextGrantApproval};
use crate::extension::registry::{ContextRegistration, PluginRegistry};
use crate::extension::{PluginKind, PluginOrigin, PluginRecord};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::RwLock;

fn config() -> ContextBudgetConfig {
    ContextBudgetConfig {
        token_budget: 1_000,
        warning_threshold: 0.70,
        critical_threshold: 0.85,
        token_estimate_ratio: 3.5,
        fresh_tail_count: 1,
        summarizer_input_budget: 48_000,
        circuit_breaker_max: 2,
        max_splits: 3,
        fold_nudge_growth_tokens: 50_000,
    }
}

struct PressureProbe {
    budget: Arc<Mutex<ContextBudget>>,
    expected_ratio: f64,
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl PreflightStage for PressureProbe {
    fn name(&self) -> &'static str {
        "task5b-pressure-probe"
    }

    async fn prepare(
        &self,
        _messages: &mut Vec<UnifiedMessage>,
        pressure: &ContextPressure,
        fresh_tail: usize,
    ) -> usize {
        assert!(
            self.budget.try_lock().is_ok(),
            "budget guard crossed an await"
        );
        assert_eq!(pressure.ratio, self.expected_ratio);
        assert_eq!(
            fresh_tail, 3,
            "persisted fresh tail plus two transient messages"
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
        0
    }
}

#[tokio::test]
async fn preflight_forwards_real_pressure_and_transient_tail_without_budget_lock() {
    let budget = Arc::new(Mutex::new(ContextBudget::new(&config())));
    let calls = Arc::new(AtomicUsize::new(0));
    let mut messages = vec![UnifiedMessage::user("history"); 10];
    let expected_ratio = budget
        .lock()
        .await
        .peek_pressure(&messages, "system", 17)
        .ratio;
    let pipeline =
        crate::context::budget::preflight::PreflightPipeline::new(vec![Box::new(PressureProbe {
            budget: budget.clone(),
            expected_ratio,
            calls: calls.clone(),
        })])
        .with_cache_stability(1, 0.0, 1.0);
    let fabric = ContextFabric::builder(Arc::new(RwLock::new(PluginRegistry::new())))
        .native_pipeline(pipeline)
        .build();
    preflight(
        Some(&fabric),
        Some(&budget),
        &mut messages,
        "system",
        17,
        2,
        &SessionId::main("pressure"),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancelled_request_never_becomes_preflight_or_fit_success() {
    let cancel = CancellationToken::new();
    cancel.cancel();
    let session = SessionId::main("cancel");
    let mut messages = vec![UnifiedMessage::user("keep")];
    let before = serde_json::to_value(&messages).unwrap();
    assert!(matches!(
        preflight(None, None, &mut messages, "", 0, 0, &session, &cancel).await,
        Err(ContextFabricError::Cancelled)
    ));
    assert!(matches!(
        compact_to_fit_and_note(
            None,
            None,
            &session,
            &mut messages,
            "",
            0,
            0,
            false,
            &cancel
        )
        .await,
        Err(ContextFabricError::Cancelled)
    ));
    assert_eq!(serde_json::to_value(&messages).unwrap(), before);
}

struct SessionProbe {
    budget: Arc<Mutex<ContextBudget>>,
    session_key: String,
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl ContextHandler for SessionProbe {
    async fn invoke(
        &self,
        request: ContextCapabilityRequest,
    ) -> Result<ContextTransformResult, String> {
        assert!(
            self.budget.try_lock().is_ok(),
            "plugin awaited with a budget guard"
        );
        assert_eq!(request.phase, ContextPhase::Compaction);
        assert_eq!(
            request.snapshot.as_ref().unwrap().session_id,
            self.session_key
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
        let output = serde_json::to_value(vec![UnifiedMessage::user("plugin summary")]).unwrap();
        let bytes = serde_json::to_vec(&output).unwrap();
        let tokens = crate::context::budget::pressure::estimate_tokens_smart(
            std::str::from_utf8(&bytes).unwrap(),
        ) as u64;
        Ok(ContextTransformResult {
            input_fingerprint: request.input_fingerprint.unwrap(),
            output,
            output_bytes: bytes.len() as u64,
            output_tokens: tokens,
            strategy: "task5b".into(),
            continuation: None,
            quality: ContextQuality::Exact,
            degraded: None,
        })
    }
}

#[tokio::test]
async fn compact_to_fit_reaches_plugin_with_host_identity_and_keeps_transient_tail() {
    let session = SessionId::main("host-session");
    let caller = AuthenticatedCaller::from_host_boundary(session.to_key_string()).unwrap();
    let descriptor = ContextCapabilityDescriptor {
        id: "compact".into(),
        api_version: CONTEXT_CAPABILITY_API_VERSION.into(),
        implementation_version: "1.0.0".into(),
        phases: vec![ContextPhase::Compaction],
        mode: ContextCapabilityMode::Replace,
        scope: ContextScope {
            namespace: ContextNamespace::Session,
            handles: vec![],
        },
        input_budget: WireBudget::new(100_000, 1_000_000),
        output_budget: WireBudget::new(100_000, 1_000_000),
        persistence: ContextPersistencePolicy::default(),
        permissions: ContextSideEffectPermissions::default(),
        lifecycle: vec![],
        bundle_id: None,
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let grant = ContextGrant::host_issue(
        &descriptor,
        "probe",
        &caller,
        ContextGrantApproval {
            expires_at: now + 300,
            audit_reason: "task5b fixture".into(),
            ..Default::default()
        },
        now,
    )
    .unwrap();
    let mut registry = PluginRegistry::new();
    registry.register_plugin(PluginRecord::new(
        "probe".into(),
        "probe".into(),
        PluginKind::Wasm,
        PluginOrigin::Config,
    ));
    registry
        .register_context(
            ContextRegistration::new(descriptor, grant, Some("probe".into())),
            now,
        )
        .unwrap();
    let budget = Arc::new(Mutex::new(ContextBudget::new(&config())));
    let calls = Arc::new(AtomicUsize::new(0));
    let fabric = ContextFabric::builder(Arc::new(RwLock::new(registry)))
        .compactor(Arc::new(ContextCompactor::new(
            Arc::new(crate::providers::mock::MockProvider::new("native fallback")),
            crate::context::compact::compactor::CompactorConfig {
                fresh_tail: 1,
                ..Default::default()
            },
        )))
        .handler(
            "probe:compact",
            Arc::new(SessionProbe {
                budget: budget.clone(),
                session_key: session.to_key_string(),
                calls: calls.clone(),
            }),
        )
        .build();
    let mut messages = vec![UnifiedMessage::user("old ".repeat(500)); 8];
    messages.push(UnifiedMessage::user("persisted tail"));
    messages.push(UnifiedMessage::user("transient"));
    // Production takes its initial gauge before deciding to compact.
    let _ = budget.lock().await.before_turn(&messages, "", 0);
    compact_to_fit_and_note(
        Some(&budget),
        Some(&fabric),
        &session,
        &mut messages,
        "",
        0,
        1,
        true,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(messages.len(), 3);
    assert_eq!(
        serde_json::to_value(messages[0].clone()).unwrap(),
        serde_json::to_value(UnifiedMessage::user("plugin summary")).unwrap()
    );
    assert_eq!(
        serde_json::to_value(messages[2].clone()).unwrap(),
        serde_json::to_value(UnifiedMessage::user("transient")).unwrap()
    );
    assert!(budget.lock().await.last_pressure().is_some());

    // A reactive-cap fallback must apply only the floor, never a second plugin call.
    compact_to_fit_and_note(
        Some(&budget),
        Some(&fabric),
        &session,
        &mut messages,
        "",
        0,
        1,
        false,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
