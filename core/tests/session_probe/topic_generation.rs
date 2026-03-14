//! P3: Topic generation tests.
//!
//! Covers LLM-driven topic summarisation, fallback on LLM failure,
//! and topic persistence in session metadata.

#[allow(unused_imports)]
use super::harness::SessionProbeHarness;
#[allow(unused_imports)]
use super::mock_llm::MockLlmProvider;
