//! Mock LLM provider for session compactor probe tests.
//!
//! Adapted from `session_probe/mock_llm.rs` with a default response
//! suitable for summarization scenarios.

#![allow(dead_code)]

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use alephcore::providers::{adapter, AiProvider, ProviderResponse};
use alephcore::Result;

/// Default summary text returned when no queued responses remain.
const DEFAULT_SUMMARY: &str = "Key decisions: implemented feature X. Files modified: src/main.rs. \
    Status: completed. Expand for details: specific code changes, test output";

/// Mock LLM provider with call tracking and queued responses.
///
/// Designed for session compactor tests — the default response mimics
/// a condensed summary that is shorter than typical multi-message input.
pub struct MockLlmProvider {
    /// Default response when queue is empty
    default_response: String,
    /// FIFO queue of responses (front = next to return)
    response_queue: Mutex<Vec<String>>,
    /// Total number of `process()` calls
    call_count: AtomicUsize,
    /// Last input seen by `process()`
    last_input: Mutex<Option<String>>,
    /// When true, `process()` returns an error
    should_fail: AtomicBool,
}

impl MockLlmProvider {
    /// Create with the default summarization response.
    pub fn new() -> Self {
        Self::with_response(DEFAULT_SUMMARY)
    }

    /// Create with a custom default response.
    pub fn with_response(default_response: impl Into<String>) -> Self {
        Self {
            default_response: default_response.into(),
            response_queue: Mutex::new(Vec::new()),
            call_count: AtomicUsize::new(0),
            last_input: Mutex::new(None),
            should_fail: AtomicBool::new(false),
        }
    }

    /// Create a provider that always fails.
    pub fn failing() -> Self {
        let p = Self::new();
        p.should_fail.store(true, Ordering::SeqCst);
        p
    }

    /// Push a response to the back of the queue.
    pub fn enqueue(&self, response: impl Into<String>) {
        self.response_queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(response.into());
    }

    /// How many times `process()` was called.
    pub fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }

    /// Return the last input passed to `process()`.
    pub fn last_input(&self) -> Option<String> {
        self.last_input
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Set whether future calls should fail.
    pub fn set_failing(&self, fail: bool) {
        self.should_fail.store(fail, Ordering::SeqCst);
    }
}

impl AiProvider for MockLlmProvider {
    fn process<'a>(
        &'a self,
        payload: adapter::RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        // Extract text from messages for tracking
        use alephcore::providers::message::UnifiedMessage;
        let input_text = UnifiedMessage::extract_all_text(payload.messages);

        // Record call
        self.call_count.fetch_add(1, Ordering::SeqCst);
        {
            let mut last = self.last_input.lock().unwrap_or_else(|e| e.into_inner());
            *last = Some(input_text);
        }

        let should_fail = self.should_fail.load(Ordering::SeqCst);
        let response = if should_fail {
            None
        } else {
            let mut q = self
                .response_queue
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if q.is_empty() {
                Some(self.default_response.clone())
            } else {
                Some(q.remove(0))
            }
        };

        Box::pin(async move {
            match response {
                Some(r) => Ok(ProviderResponse::text_only(r)),
                None => Err(alephcore::AlephError::provider(
                    "MockLlmProvider: configured to fail",
                )),
            }
        })
    }

    fn name(&self) -> &str {
        "mock-compactor-llm"
    }

    fn color(&self) -> &str {
        "#888888"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alephcore::providers::message::UnifiedMessage;

    fn make_payload(msgs: &[UnifiedMessage]) -> adapter::RequestPayload<'_> {
        adapter::RequestPayload::new(msgs)
    }

    #[tokio::test]
    async fn default_summary_response() {
        let p = MockLlmProvider::new();
        let msgs = [UnifiedMessage::user("summarize this")];
        let r = p.process(make_payload(&msgs)).await.unwrap();
        assert!(r.text_content().contains("Key decisions"));
        assert_eq!(p.call_count(), 1);
    }

    #[tokio::test]
    async fn queued_responses_drain_then_default() {
        let p = MockLlmProvider::new();
        p.enqueue("first summary");
        p.enqueue("second summary");

        let msgs_a = [UnifiedMessage::user("a")];
        let msgs_b = [UnifiedMessage::user("b")];
        let msgs_c = [UnifiedMessage::user("c")];
        assert_eq!(
            p.process(make_payload(&msgs_a))
                .await
                .unwrap()
                .text_content(),
            "first summary"
        );
        assert_eq!(
            p.process(make_payload(&msgs_b))
                .await
                .unwrap()
                .text_content(),
            "second summary"
        );
        assert!(p
            .process(make_payload(&msgs_c))
            .await
            .unwrap()
            .text_content()
            .contains("Key decisions"));
        assert_eq!(p.call_count(), 3);
    }

    #[tokio::test]
    async fn failing_mode() {
        let p = MockLlmProvider::failing();
        let msgs = [UnifiedMessage::user("x")];
        assert!(p.process(make_payload(&msgs)).await.is_err());
    }

    #[tokio::test]
    async fn toggle_failing() {
        let p = MockLlmProvider::new();
        let msgs = [UnifiedMessage::user("y")];
        assert!(p.process(make_payload(&msgs)).await.is_ok());
        p.set_failing(true);
        assert!(p.process(make_payload(&msgs)).await.is_err());
        p.set_failing(false);
        assert!(p.process(make_payload(&msgs)).await.is_ok());
    }
}
