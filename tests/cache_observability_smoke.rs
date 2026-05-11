//! Stage J-pre — cache observability smoke.
//!
//! Asserts that:
//! 1. A `MeteringProvider` wrapping a provider that returns Some(TokenUsage)
//!    causes a `LoopTraceEvent::ProviderUsage` to land on the trace sink.
//! 2. The event carries the configured agent_id and the full token split.
//!
//! This is the intentional cheap smoke — real-LLM cache_read/creation
//! verification is in the manual checklist on the PR description.

use alephcore::Result;
use alephcore::harness::trace::LoopTraceEvent;
use alephcore::harness::TraceSink;
use alephcore::providers::adapter::{ProviderResponse, RequestPayload, TokenUsage};
use alephcore::providers::message::UnifiedMessage;
use alephcore::providers::{AiProvider, MeteringProvider};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

struct CannedUsageProvider {
    usage: TokenUsage,
}

impl AiProvider for CannedUsageProvider {
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
    fn name(&self) -> &str { "canned-usage" }
    fn color(&self) -> &str { "#000" }
}

struct CapturingSink(Mutex<Vec<LoopTraceEvent>>);
impl TraceSink for CapturingSink {
    fn on_trace(&self, event: &LoopTraceEvent) {
        self.0.lock().unwrap().push(event.clone());
    }
    fn flush(&self) {}
}

#[tokio::test]
async fn root_label_emits_provider_usage_event() {
    let inner = Arc::new(CannedUsageProvider {
        usage: TokenUsage {
            input_tokens: 1000,
            output_tokens: 200,
            cache_read_tokens: Some(800),
            cache_creation_tokens: Some(50),
            thinking_tokens: None,
            cost: None,
        },
    });
    let sink = Arc::new(CapturingSink(Mutex::new(Vec::new())));
    let metering = MeteringProvider::new(
        inner,
        Some(sink.clone() as Arc<dyn TraceSink>),
        "root",
    );

    let msgs = [UnifiedMessage::user("hi")];
    let _ = metering.process(RequestPayload::new(&msgs)).await.expect("process");

    let events = sink.0.lock().unwrap();
    assert_eq!(events.len(), 1);
    match &events[0] {
        LoopTraceEvent::ProviderUsage { agent_id, cache_read_tokens, cache_creation_tokens, .. } => {
            assert_eq!(agent_id, "root");
            assert_eq!(*cache_read_tokens, Some(800));
            assert_eq!(*cache_creation_tokens, Some(50));
        }
        other => panic!("expected ProviderUsage, got {other:?}"),
    }
}

#[tokio::test]
async fn subagent_label_distinguishes_from_root() {
    let inner = Arc::new(CannedUsageProvider {
        usage: TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            thinking_tokens: None,
            cost: None,
        },
    });
    let sink = Arc::new(CapturingSink(Mutex::new(Vec::new())));
    let metering = MeteringProvider::new(
        inner,
        Some(sink.clone() as Arc<dyn TraceSink>),
        "subagent-research",
    );
    let msgs = [UnifiedMessage::user("hi")];
    let _ = metering.process(RequestPayload::new(&msgs)).await.unwrap();

    let events = sink.0.lock().unwrap();
    let agent_ids: Vec<_> = events.iter().filter_map(|e| match e {
        LoopTraceEvent::ProviderUsage { agent_id, .. } => Some(agent_id.clone()),
        _ => None,
    }).collect();
    assert_eq!(agent_ids, vec!["subagent-research".to_string()]);
}

#[tokio::test]
async fn no_sink_means_no_panic_no_event() {
    let inner = Arc::new(CannedUsageProvider {
        usage: TokenUsage {
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            thinking_tokens: None,
            cost: None,
        },
    });
    let metering = MeteringProvider::new(inner, None, "root");
    let msgs = [UnifiedMessage::user("hi")];
    let resp = metering.process(RequestPayload::new(&msgs)).await.expect("process");
    assert!(resp.usage.is_some());
}
