//! `MeteringProvider` — decorator that emits `LoopTraceEvent::ProviderUsage`
//! after each `process()` call (Stage J-pre cache observability pipeline).
//!
//! Decorator-only: no harness diff. Composes with any `AiProvider` (anthropic,
//! mock, failover, etc.). Non-Anthropic providers will populate `cache_*` as
//! `None` until their protocols extend.
//!
//! See: docs/superpowers/plans/2026-05-09-subagent-uplift-stage-j-pre-plan.md

use crate::error::Result;
use crate::harness::trace::LoopTraceEvent;
use crate::harness::TraceSink;
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;

pub struct MeteringProvider {
    inner: Arc<dyn AiProvider>,
    sink: Option<Arc<dyn TraceSink>>,
    agent_id: String,
}

impl MeteringProvider {
    pub fn new(
        inner: Arc<dyn AiProvider>,
        sink: Option<Arc<dyn TraceSink>>,
        agent_id: impl Into<String>,
    ) -> Self {
        Self {
            inner,
            sink,
            agent_id: agent_id.into(),
        }
    }
}

impl MeteringProvider {
    /// Emit the `ProviderUsage` trace event + cache-monitor feed + usage log for
    /// one response. Shared by the streaming and non-streaming paths so metering
    /// is byte-identical on both — the streaming path previously bypassed this
    /// decorator entirely, so streamed turns produced no `ProviderUsage` at all.
    /// The cache-watchdog key for a request: `(agent, session)`, because the
    /// provider's prompt-cache prefix is per conversation. `session_id` rides
    /// the payload metadata that `harness::agent::think::build_request_payload`
    /// stamps on every call, and it is the serialized `SessionKey` — the same
    /// string the compaction side resets under.
    fn cache_scope_of(payload: &RequestPayload<'_>, agent_id: &str) -> String {
        crate::thinker::prompt_builder::cache_monitor::cache_scope(
            agent_id,
            payload
                .metadata
                .as_ref()
                .and_then(|m| m.get("session_id"))
                .map(String::as_str),
        )
    }

    fn record_usage(
        resp: &ProviderResponse,
        sink: &Option<Arc<dyn TraceSink>>,
        agent_id: &str,
        provider_name: &str,
        cache_scope: &str,
        prefix_hash: Option<u64>,
    ) {
        let Some(usage) = resp.usage.as_ref() else {
            return;
        };
        tracing::info!(
            target: "aleph::provider_usage",
            agent_id = %agent_id,
            provider = %provider_name,
            input_tokens = usage.input_tokens,
            output_tokens = usage.output_tokens,
            cache_read_tokens = ?usage.cache_read_tokens,
            cache_creation_tokens = ?usage.cache_creation_tokens,
            cache_hit_ratio = ?usage.cache_hit_ratio(),
            thinking_tokens = ?usage.thinking_tokens,
            "LLM call completed"
        );
        // Cache-first observability: feed cache token counts into the
        // process-wide `CacheMonitor`, keyed per prompt-cache prefix. Three
        // consecutive misses (counted only once that prefix has seen real cache
        // activity) with more than three total calls triggers a warn — surfaces
        // accidental stable-prefix changes that would otherwise only show up on
        // the bill. On that rising edge the monitor hands back a report, which
        // becomes a `LoopTraceEvent::CacheHealthDegraded` on the same trace
        // stream as `ProviderUsage` — lifting the domain's only alarm out of
        // the log and onto the TUI / Panel / doctor surfaces.
        let degradation = crate::thinker::prompt_builder::cache_monitor::global_cache_monitor()
            .record_cache_usage(
                cache_scope,
                usage.cache_read_tokens,
                usage.cache_creation_tokens,
                prefix_hash,
            );
        if let Some(sink) = sink {
            sink.on_trace(&LoopTraceEvent::ProviderUsage {
                agent_id: agent_id.to_string(),
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_read_tokens: usage.cache_read_tokens,
                cache_creation_tokens: usage.cache_creation_tokens,
                thinking_tokens: usage.thinking_tokens,
            });
            if let Some(report) = degradation {
                sink.on_trace(&LoopTraceEvent::CacheHealthDegraded {
                    scope: report.scope,
                    streak: report.streak,
                    reads: report.reads,
                    writes: report.writes,
                    prefix_changed: report.prefix_changed,
                });
            }
        }
    }
}

impl AiProvider for MeteringProvider {
    fn process<'a>(
        &'a self,
        req: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        let scope = Self::cache_scope_of(&req, &self.agent_id);
        let prefix_hash = crate::thinker::prompt_builder::cache_monitor::stable_prefix_hash(&req);
        let fut = self.inner.process(req);
        let sink = self.sink.clone();
        let agent_id = self.agent_id.clone();
        let provider_name = self.inner.name().to_string();
        Box::pin(async move {
            let resp = fut.await?;
            Self::record_usage(&resp, &sink, &agent_id, &provider_name, &scope, prefix_hash);
            Ok(resp)
        })
    }

    fn execute_streaming_dyn<'a>(
        &'a self,
        payload: RequestPayload<'a>,
        stream_sink: &'a dyn crate::providers::DeltaSink,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        // Delegate the actual streaming to the inner provider, then meter its
        // assembled response identically to `process`. This is the fix for the
        // streaming metering gap: the same `ProviderUsage` pipeline now fires on
        // streamed turns instead of being skipped by the harness downcast.
        let scope = Self::cache_scope_of(&payload, &self.agent_id);
        let prefix_hash =
            crate::thinker::prompt_builder::cache_monitor::stable_prefix_hash(&payload);
        let fut = self.inner.execute_streaming_dyn(payload, stream_sink);
        let sink = self.sink.clone();
        let agent_id = self.agent_id.clone();
        let provider_name = self.inner.name().to_string();
        Box::pin(async move {
            let resp = fut.await?;
            Self::record_usage(&resp, &sink, &agent_id, &provider_name, &scope, prefix_hash);
            Ok(resp)
        })
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn color(&self) -> &str {
        self.inner.color()
    }

    fn supports_native_tools(&self) -> bool {
        self.inner.supports_native_tools()
    }

    fn protocol(&self) -> Cow<'_, str> {
        self.inner.protocol()
    }

    fn model_behavior_override(&self) -> Option<Cow<'_, str>> {
        self.inner.model_behavior_override()
    }

    fn behavior_hint(&self) -> Option<Cow<'_, str>> {
        self.inner.behavior_hint()
    }

    fn serving_model_hint(&self) -> Option<Cow<'_, str>> {
        self.inner.serving_model_hint()
    }

    fn serving_provider_hint(&self) -> Option<Cow<'_, str>> {
        self.inner.serving_provider_hint()
    }

    fn as_http_provider(&self) -> Option<&crate::providers::http_provider::HttpProvider> {
        self.inner.as_http_provider()
    }

    fn supports_streaming(&self) -> bool {
        self.inner.supports_streaming()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::adapter::TokenUsage;
    use crate::sync_primitives::Mutex;

    struct FakeProvider {
        usage: TokenUsage,
    }
    impl AiProvider for FakeProvider {
        fn process<'a>(
            &'a self,
            _req: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
            let usage = self.usage.clone();
            Box::pin(async move {
                Ok(ProviderResponse {
                    usage: Some(usage),
                    ..Default::default()
                })
            })
        }
        fn name(&self) -> &str {
            "fake"
        }
        fn color(&self) -> &str {
            "#000"
        }
    }

    struct CapturingSink(Mutex<Vec<LoopTraceEvent>>);
    impl TraceSink for CapturingSink {
        fn on_trace(&self, event: &LoopTraceEvent) {
            self.0.lock().unwrap().push(event.clone());
        }
        fn flush(&self) {}
    }

    #[tokio::test]
    async fn emits_provider_usage_with_agent_id_and_full_token_split() {
        let inner = Arc::new(FakeProvider {
            usage: TokenUsage {
                input_tokens: 200,
                output_tokens: 50,
                cache_read_tokens: Some(150),
                cache_creation_tokens: Some(20),
                thinking_tokens: None,
                cost: None,
            },
        });
        let sink = Arc::new(CapturingSink(Mutex::new(Vec::new())));
        let metering = MeteringProvider::new(
            inner,
            Some(sink.clone() as Arc<dyn TraceSink>),
            "subagent-test",
        );

        let msgs = [crate::providers::message::UnifiedMessage::user("hi")];
        let req = RequestPayload::new(&msgs);
        let _ = metering.process(req).await.expect("process");

        let events = sink.0.lock().unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            LoopTraceEvent::ProviderUsage {
                agent_id,
                input_tokens,
                cache_read_tokens,
                cache_creation_tokens,
                ..
            } => {
                assert_eq!(agent_id, "subagent-test");
                assert_eq!(*input_tokens, 200);
                assert_eq!(*cache_read_tokens, Some(150));
                assert_eq!(*cache_creation_tokens, Some(20));
            }
            other => panic!("expected ProviderUsage, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn degraded_streak_emits_cache_health_event_into_sink() {
        // The alarm-effect assertion: when the watchdog's streak crosses the
        // threshold, a `CacheHealthDegraded` event must actually reach the
        // trace sink (→ task_traces, wire, doctor) — not just a log line.
        // Rising edge only: 4 re-creating calls = 4 ProviderUsage + exactly 1
        // degraded event. Unique agent id: the monitor is a process-wide
        // singleton shared with every other test in this binary.
        let inner = Arc::new(FakeProvider {
            usage: TokenUsage {
                input_tokens: 100,
                output_tokens: 10,
                cache_read_tokens: Some(0),
                cache_creation_tokens: Some(1000),
                thinking_tokens: None,
                cost: None,
            },
        });
        let sink = Arc::new(CapturingSink(Mutex::new(Vec::new())));
        let metering = MeteringProvider::new(
            inner,
            Some(sink.clone() as Arc<dyn TraceSink>),
            "cache-degraded-sink-test",
        );
        let msgs = [crate::providers::message::UnifiedMessage::user("hi")];
        for _ in 0..4 {
            let _ = metering
                .process(RequestPayload::new(&msgs))
                .await
                .expect("process");
        }

        let events = sink.0.lock().unwrap();
        let degraded: Vec<&LoopTraceEvent> = events
            .iter()
            .filter(|e| matches!(e, LoopTraceEvent::CacheHealthDegraded { .. }))
            .collect();
        assert_eq!(
            degraded.len(),
            1,
            "rising edge only — total events: {}",
            events.len()
        );
        match degraded[0] {
            LoopTraceEvent::CacheHealthDegraded {
                scope,
                streak,
                writes,
                ..
            } => {
                assert_eq!(scope, "cache-degraded-sink-test");
                assert_eq!(*streak, 4);
                assert_eq!(*writes, 1000);
            }
            other => panic!("expected CacheHealthDegraded, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn no_event_when_response_lacks_usage() {
        struct EmptyProvider;
        impl AiProvider for EmptyProvider {
            fn process<'a>(
                &'a self,
                _req: RequestPayload<'a>,
            ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
                Box::pin(async { Ok(ProviderResponse::default()) })
            }
            fn name(&self) -> &str {
                "empty"
            }
            fn color(&self) -> &str {
                "#000"
            }
        }
        let sink = Arc::new(CapturingSink(Mutex::new(Vec::new())));
        let metering = MeteringProvider::new(
            Arc::new(EmptyProvider),
            Some(sink.clone() as Arc<dyn TraceSink>),
            "x",
        );
        let msgs = [crate::providers::message::UnifiedMessage::user("hi")];
        let _ = metering.process(RequestPayload::new(&msgs)).await.unwrap();
        assert!(sink.0.lock().unwrap().is_empty());
    }
}
