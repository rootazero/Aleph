//! Minimal AiProvider stub that records the last system prompt it saw.
//! Test-only helper for verifying source-aware prompt dispatch.

use crate::error::Result;
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::AiProvider;
use crate::sync_primitives::{Arc, Mutex};
use std::future::Future;
use std::pin::Pin;

pub struct RecordingMockProvider {
    canned: String,
    last_system: Arc<Mutex<Option<String>>>,
}

impl RecordingMockProvider {
    pub fn new(canned: String) -> Self {
        Self {
            canned,
            last_system: Arc::new(Mutex::new(None)),
        }
    }

    pub fn recorded_system_prompt(&self) -> Arc<Mutex<Option<String>>> {
        self.last_system.clone()
    }
}

impl AiProvider for RecordingMockProvider {
    fn process<'a>(
        &'a self,
        req: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        let recorded = self.last_system.clone();
        let system = req.system_prompt.map(|s| s.to_string());
        let canned = self.canned.clone();
        Box::pin(async move {
            if let Some(sys) = system {
                *recorded.lock().unwrap() = Some(sys);
            }
            Ok(ProviderResponse::text_only(canned))
        })
    }

    fn name(&self) -> &str {
        "recording-mock"
    }

    fn color(&self) -> &str {
        "#000000"
    }
}
