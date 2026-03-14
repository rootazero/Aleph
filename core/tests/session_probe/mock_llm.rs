//! Mock LLM provider for session probe tests.
//!
//! Implements `AiProvider` with configurable default response, queued responses,
//! call tracking, and a failing mode.

#![allow(dead_code)]

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use alephcore::Result;
use alephcore::providers::AiProvider;

/// Mock LLM provider with call tracking and queued responses.
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
    /// Create with a default response.
    pub fn new(default_response: impl Into<String>) -> Self {
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
        let p = Self::new("unreachable");
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

    /// Push multiple responses.
    pub fn enqueue_many(&self, responses: impl IntoIterator<Item = impl Into<String>>) {
        let mut q = self.response_queue.lock().unwrap_or_else(|e| e.into_inner());
        for r in responses {
            q.push(r.into());
        }
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
    fn process(
        &self,
        input: &str,
        _system_prompt: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        // Record call
        self.call_count.fetch_add(1, Ordering::SeqCst);
        {
            let mut last = self.last_input.lock().unwrap_or_else(|e| e.into_inner());
            *last = Some(input.to_string());
        }

        let should_fail = self.should_fail.load(Ordering::SeqCst);
        let response = if should_fail {
            None
        } else {
            let mut q = self.response_queue.lock().unwrap_or_else(|e| e.into_inner());
            if q.is_empty() {
                Some(self.default_response.clone())
            } else {
                Some(q.remove(0))
            }
        };

        Box::pin(async move {
            match response {
                Some(r) => Ok(r),
                None => Err(alephcore::AlephError::provider(
                    "MockLlmProvider: configured to fail",
                )),
            }
        })
    }

    fn name(&self) -> &str {
        "mock-llm"
    }

    fn color(&self) -> &str {
        "#888888"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn default_response() {
        let p = MockLlmProvider::new("hello");
        let r = p.process("hi", None).await.unwrap();
        assert_eq!(r, "hello");
        assert_eq!(p.call_count(), 1);
        assert_eq!(p.last_input().as_deref(), Some("hi"));
    }

    #[tokio::test]
    async fn queued_responses_drain_then_default() {
        let p = MockLlmProvider::new("default");
        p.enqueue("first");
        p.enqueue("second");

        assert_eq!(p.process("a", None).await.unwrap(), "first");
        assert_eq!(p.process("b", None).await.unwrap(), "second");
        assert_eq!(p.process("c", None).await.unwrap(), "default");
        assert_eq!(p.call_count(), 3);
    }

    #[tokio::test]
    async fn failing_mode() {
        let p = MockLlmProvider::failing();
        assert!(p.process("x", None).await.is_err());
    }
}
