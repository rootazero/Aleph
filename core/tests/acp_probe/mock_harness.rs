use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;

use alephcore::acp::harness::{AcpHarness, HarnessMode};
use alephcore::acp::session::HarnessConfig;
use alephcore::{AlephError, Result};

pub struct MockAcpHarness {
    id: String,
    name: String,
    mode: HarnessMode,
    available: AtomicBool,
    failing: AtomicBool,
    responses: Mutex<VecDeque<String>>,
    default_response: String,
    pub call_count: AtomicU64,
    pub last_prompt: Mutex<Option<String>>,
}

impl MockAcpHarness {
    pub fn new(id: &str, name: &str, mode: HarnessMode) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            mode,
            available: AtomicBool::new(true),
            failing: AtomicBool::new(false),
            responses: Mutex::new(VecDeque::new()),
            default_response: format!("mock response from {}", id),
            call_count: AtomicU64::new(0),
            last_prompt: Mutex::new(None),
        }
    }

    pub fn oneshot(id: &str, name: &str) -> Self {
        Self::new(id, name, HarnessMode::Oneshot)
    }

    pub fn native_acp(id: &str, name: &str) -> Self {
        Self::new(id, name, HarnessMode::NativeAcp)
    }

    pub fn set_available(&self, available: bool) {
        self.available.store(available, Ordering::SeqCst);
    }

    pub fn set_failing(&self) {
        self.failing.store(true, Ordering::SeqCst);
    }

    pub fn enqueue_response(&self, response: &str) {
        self.responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(response.to_string());
    }

    pub fn calls(&self) -> u64 {
        self.call_count.load(Ordering::SeqCst)
    }

    pub fn was_called(&self) -> bool {
        self.calls() > 0
    }

    pub fn last_prompt_text(&self) -> Option<String> {
        self.last_prompt
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[async_trait]
impl AcpHarness for MockAcpHarness {
    fn id(&self) -> &str {
        &self.id
    }
    fn display_name(&self) -> &str {
        &self.name
    }
    fn mode(&self) -> HarnessMode {
        self.mode
    }

    fn build_config(&self, cwd: Option<&str>) -> HarnessConfig {
        HarnessConfig {
            executable: format!("mock-{}", self.id),
            args: vec![],
            cwd: cwd.map(String::from),
            ..Default::default()
        }
    }

    async fn is_available(&self) -> bool {
        self.available.load(Ordering::SeqCst)
    }

    async fn execute_oneshot(&self, prompt: &str, _cwd: &str) -> Result<String> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        *self
            .last_prompt
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(prompt.to_string());

        if self.failing.load(Ordering::SeqCst) {
            return Err(AlephError::tool("Mock harness failing"));
        }

        let response = self
            .responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
            .unwrap_or_else(|| self.default_response.clone());
        Ok(response)
    }
}

/// Resolve path to a mock script in the test fixtures directory.
pub fn mock_script_path(name: &str) -> String {
    let manifest =
        std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    format!("{}/tests/acp_probe/mock_scripts/{}", manifest, name)
}
