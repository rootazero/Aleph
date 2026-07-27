use std::sync::Arc;

use crate::harness::trace::LoopTraceEvent;
use crate::harness::TraceSink;
use crate::orchestrator::dispatch::{TerminateReason, TokenBreakdown, ToolInvocation};

use super::experience_store::{RoutingExperienceStore, RoutingOutcome};
use super::RoutingAttribution;

/// Stringify a `TerminateReason` verbatim (discriminant + embedded fields) via
/// its own serde tagging — no collapse to success/failure (R7).
fn terminate_reason_to_raw(tr: &Option<TerminateReason>) -> String {
    match tr {
        Some(r) => serde_json::to_string(r).unwrap_or_else(|_| "unknown".to_string()),
        None => "unknown".to_string(),
    }
}

/// Derive a `RoutingOutcome` from the verbatim `SessionCompleted` fields. Pure:
/// counts and discriminants only, zero interpretation; never reads judgment
/// signals from `LoopTraceTurnMetrics` (not present on `SessionCompleted`).
#[must_use]
pub fn outcome_from_session_completed(
    iterations: usize,
    tool_calls_made: usize,
    terminate_reason: &Option<TerminateReason>,
    token_breakdown: &Option<TokenBreakdown>,
    duration_ms: &Option<u64>,
    tool_timeline: &[ToolInvocation],
) -> RoutingOutcome {
    RoutingOutcome {
        iterations: iterations.min(u32::MAX as usize) as u32,
        tool_calls_made: tool_calls_made.min(u32::MAX as usize) as u32,
        terminate_reason: terminate_reason_to_raw(terminate_reason),
        // rust-doctor-disable-next-line excessive-clone
        token_breakdown: token_breakdown.clone().unwrap_or_default(),
        estimated_cost: None,
        duration_ms: duration_ms.unwrap_or(0),
        tool_error_count: tool_timeline.iter().filter(|t| !t.success).count() as u32,
        tool_call_total: tool_timeline.len() as u32,
    }
}

pub struct OutcomeObserver {
    inner: Arc<dyn TraceSink>,
    store: Arc<RoutingExperienceStore>,
    attribution: Arc<RoutingAttribution>,
    model_id: String,
    provider_id: String,
    agent_id: String,
}

impl OutcomeObserver {
    #[must_use]
    pub fn new(
        inner: Arc<dyn TraceSink>,
        store: Arc<RoutingExperienceStore>,
        attribution: Arc<RoutingAttribution>,
        model_id: String,
        provider_id: String,
        agent_id: String,
    ) -> Self {
        Self {
            inner,
            store,
            attribution,
            model_id,
            provider_id,
            agent_id,
        }
    }

    /// Fire-and-forget body, a free async fn so `on_trace` can `tokio::spawn`
    /// it with owned clones (the 'static bound forbids borrowing `self`).
    async fn record_to_store(
        store: Arc<RoutingExperienceStore>,
        agent_id: String,
        model_id: String,
        provider_id: String,
        task_emb: Vec<f32>,
        outcome: RoutingOutcome,
    ) {
        if let Err(e) = store
            .record(&agent_id, &model_id, &provider_id, &task_emb, &outcome)
            .await
        {
            tracing::warn!(error = %e, "routing experience record failed");
        }
    }
}

impl TraceSink for OutcomeObserver {
    fn on_trace(&self, event: &LoopTraceEvent) {
        if let LoopTraceEvent::SessionCompleted {
            iterations,
            tool_calls_made,
            terminate_reason,
            token_breakdown,
            duration_ms,
            tool_timeline,
            ..
        } = event
        {
            let mut outcome = outcome_from_session_completed(
                *iterations,
                *tool_calls_made,
                terminate_reason,
                token_breakdown,
                duration_ms,
                tool_timeline,
            );
            // v1.1 (c): enrich with USD from the static price table, keyed on the
            // REAL injected provider+model (never the FailoverProvider wrapper
            // name). Best-effort — unknown provider/model degrades to None, never
            // errors. Same enrichment for top-level and subagent observers.
            let est = crate::pricing::estimate(
                &self.provider_id,
                &self.model_id,
                &outcome.token_breakdown,
            );
            outcome.estimated_cost = match est.status {
                crate::pricing::CostStatus::Complete
                | crate::pricing::CostStatus::PartialMissingPrice => Some(est.usd),
                crate::pricing::CostStatus::Unknown => None,
            };
            if let Some(task_emb) = self.attribution.task_emb.get().cloned() {
                tracing::debug!(
                    session_id = %self.attribution.session_id,
                    model = %self.model_id,
                    "recording routing experience"
                );
                tokio::spawn(Self::record_to_store(
                    // rust-doctor-disable-next-line excessive-clone
                    self.store.clone(),
                    // rust-doctor-disable-next-line excessive-clone
                    self.agent_id.clone(),
                    // rust-doctor-disable-next-line excessive-clone
                    self.model_id.clone(),
                    // rust-doctor-disable-next-line excessive-clone
                    self.provider_id.clone(),
                    task_emb,
                    outcome,
                ));
            }
        }
        self.inner.on_trace(event); // MUST forward unchanged + non-blocking (trace_sink.rs:12-25)
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AlephError;
    use crate::routing::RoutingExperienceStore;

    fn emb(seed: f32) -> Vec<f32> {
        let mut v = vec![0.0f32; 768];
        v[0] = seed;
        v
    }
    fn temp_backend() -> crate::memory::store::sqlite::SqliteMemoryBackend {
        let dir = std::env::temp_dir().join(format!("aleph-routing-obs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::memory::store::sqlite::SqliteMemoryBackend::new(&dir.join("mem.db")).unwrap()
    }
    struct StubEmbedder;
    #[async_trait::async_trait]
    impl crate::memory::EmbeddingProvider for StubEmbedder {
        async fn embed(&self, _t: &str) -> Result<Vec<f32>, AlephError> {
            Ok(emb(1.0))
        }
        async fn embed_batch(&self, t: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
            Ok(t.iter().map(|_| emb(1.0)).collect())
        }
        fn dimensions(&self) -> usize {
            768
        }
        fn model_name(&self) -> &str {
            "stub"
        }
        fn provider_id(&self) -> &str {
            "stub"
        }
    }

    #[test]
    fn outcome_maps_raw_without_verdict() {
        let timeline = vec![
            ToolInvocation {
                id: "1".into(),
                name: "bash".into(),
                duration_ms: 5,
                success: true,
                error: None,
            },
            ToolInvocation {
                id: "2".into(),
                name: "web".into(),
                duration_ms: 5,
                success: false,
                error: Some("boom".into()),
            },
            ToolInvocation {
                id: "3".into(),
                name: "web".into(),
                duration_ms: 5,
                success: false,
                error: Some("boom".into()),
            },
        ];
        let tr = Some(TerminateReason::VerifierVeto { vetos: 3 });
        let tb = Some(TokenBreakdown {
            input: 10,
            output: 20,
            cache_read: 0,
            cache_creation: 0,
            reasoning: 5,
        });
        let dur = Some(1234u64);
        let outcome = outcome_from_session_completed(7, 3, &tr, &tb, &dur, &timeline);
        assert_eq!(outcome.iterations, 7);
        assert_eq!(outcome.tool_calls_made, 3);
        assert_eq!(outcome.tool_error_count, 2);
        assert_eq!(outcome.tool_call_total, 3);
        assert_eq!(outcome.duration_ms, 1234);
        assert_eq!(
            outcome.terminate_reason,
            "{\"kind\":\"verifier_veto\",\"vetos\":3}"
        );
        assert_eq!(outcome.token_breakdown.reasoning, 5);
    }

    #[test]
    fn mapper_never_fabricates_or_reads_judgment_signals() {
        let src = include_str!("observer.rs");
        // Split the sentinel strings so this guard code itself does not trigger the check
        // (include_str! embeds the full file including these test lines).
        let re_steer = ["user_re", "_steer"].concat();
        let consec_err = ["consecutive", "_errors"].concat();
        assert!(
            !src.contains(&re_steer),
            "must not reference user-re-steer signal (U2)"
        );
        assert!(
            !src.contains(&consec_err),
            "must not reference consecutive-errors signal (U2)"
        );
    }

    #[tokio::test]
    async fn observer_records_injected_model_not_provider_usage() {
        let backend = Arc::new(temp_backend());
        let embedder: Arc<dyn crate::memory::EmbeddingProvider> = Arc::new(StubEmbedder);
        let store = Arc::new(RoutingExperienceStore::new(backend, embedder));
        let outcome = RoutingOutcome {
            iterations: 0,
            tool_calls_made: 0,
            terminate_reason: "{\"kind\":\"completed\"}".into(),
            token_breakdown: TokenBreakdown::default(),
            estimated_cost: None,
            duration_ms: 0,
            tool_error_count: 0,
            tool_call_total: 0,
        };
        OutcomeObserver::record_to_store(
            store.clone(),
            "a".into(),
            "MODEL_X".into(),
            "PROV_Y".into(),
            emb(1.0),
            outcome,
        )
        .await;
        let got = store.recall("a", &emb(0.0), 5).await.unwrap();
        assert_eq!(got[0].model_id, "MODEL_X"); // injected at construction, not from ProviderUsage
        assert_eq!(got[0].provider_id, "PROV_Y");
    }
}
