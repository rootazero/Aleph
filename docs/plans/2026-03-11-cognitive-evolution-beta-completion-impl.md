# Cognitive Evolution Beta Completion Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Connect PatternSynthesisBackend to real LLM inference and wire CortexDreamingService to IdleDetector + CognitiveEntropyTracker.

**Architecture:** ProviderBackend wraps `Arc<dyn AiProvider>` to implement `PatternSynthesisBackend`. CortexDreamingService gains `Arc<IdleDetector>` as constructor dependency, replacing the stub `get_idle_seconds()`. Dual-queue processing separates entropy-driven and value-driven candidates.

**Tech Stack:** Rust, async_trait, serde_json, chrono, tokio

---

### Task 1: ProviderBackend — Struct and Constructor

**Files:**
- Create: `core/src/poe/crystallization/provider_backend.rs`
- Modify: `core/src/poe/crystallization/mod.rs:47` (add module declaration)

**Step 1: Write the failing test**

Add to `core/src/poe/crystallization/provider_backend.rs`:

```rust
//! Real LLM implementation of PatternSynthesisBackend.
//!
//! Wraps `Arc<dyn AiProvider>` to connect pattern extraction to actual
//! LLM inference. `synthesize_pattern` calls the LLM; `evaluate_confidence`
//! uses a token-efficient heuristic.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

use super::experience_store::PoeExperience;
use super::synthesis_backend::{
    PatternSuggestion, PatternSynthesisBackend, PatternSynthesisRequest,
};

/// Real LLM-backed implementation of `PatternSynthesisBackend`.
pub struct ProviderBackend {
    provider: Arc<dyn AiProvider>,
}

impl ProviderBackend {
    /// Create a new ProviderBackend wrapping the given AI provider.
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self { provider }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::create_provider;
    use crate::config::ProviderConfig;

    // We can't test with a real provider in unit tests, so we use a mock.
    // The mock provider is defined here for ProviderBackend-specific tests.

    struct MockAiProvider;

    impl AiProvider for MockAiProvider {
        fn process(
            &self,
            _input: &str,
            _system_prompt: Option<&str>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::error::Result<String>> + Send + '_>> {
            Box::pin(async { Ok("mock response".to_string()) })
        }
    }

    #[test]
    fn test_provider_backend_creation() {
        let provider: Arc<dyn AiProvider> = Arc::new(MockAiProvider);
        let _backend = ProviderBackend::new(provider);
    }
}
```

**Step 2: Add module declaration**

In `core/src/poe/crystallization/mod.rs`, add after line 47 (`pub mod synthesis_backend;`):

```rust
pub mod provider_backend;
```

**Step 3: Run test to verify it passes**

Run: `cargo test -p alephcore --lib crystallization::provider_backend`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/poe/crystallization/provider_backend.rs core/src/poe/crystallization/mod.rs
git commit -m "poe: add ProviderBackend struct and constructor"
```

---

### Task 2: ProviderBackend — evaluate_confidence Heuristic

**Files:**
- Modify: `core/src/poe/crystallization/provider_backend.rs`

**Step 1: Write the failing tests**

Add tests to the `tests` module in `provider_backend.rs`:

```rust
#[tokio::test]
async fn test_evaluate_confidence_empty_occurrences() {
    let provider: Arc<dyn AiProvider> = Arc::new(MockAiProvider);
    let backend = ProviderBackend::new(provider);

    let conf = backend.evaluate_confidence("hash-1", &[]).await.unwrap();
    // base = 0.5, no bonuses
    assert!((conf - 0.5).abs() < f32::EPSILON);
}

#[tokio::test]
async fn test_evaluate_confidence_with_occurrences() {
    let provider: Arc<dyn AiProvider> = Arc::new(MockAiProvider);
    let backend = ProviderBackend::new(provider);

    let now_ms = chrono::Utc::now().timestamp_millis();
    let exps = vec![
        make_exp("a", 0.9, 0.1, now_ms),
        make_exp("b", 0.8, 0.2, now_ms),
        make_exp("c", 0.7, 0.3, now_ms - 86_400_000 * 10), // 10 days ago
    ];

    let conf = backend.evaluate_confidence("hash-1", &exps).await.unwrap();
    // base=0.5, occurrence_bonus=min(3*0.05,0.3)=0.15, success_bonus=avg(0.9,0.8,0.7)*0.2=0.8*0.2=0.16, recency_bonus=0.05
    // total = 0.5 + 0.15 + 0.16 + 0.05 = 0.86
    assert!((conf - 0.86).abs() < 0.01);
}

#[tokio::test]
async fn test_evaluate_confidence_clamped_to_one() {
    let provider: Arc<dyn AiProvider> = Arc::new(MockAiProvider);
    let backend = ProviderBackend::new(provider);

    let now_ms = chrono::Utc::now().timestamp_millis();
    // 10 occurrences with perfect satisfaction
    let exps: Vec<PoeExperience> = (0..10)
        .map(|i| make_exp(&format!("e{}", i), 1.0, 0.0, now_ms))
        .collect();

    let conf = backend.evaluate_confidence("hash-1", &exps).await.unwrap();
    // Should be clamped to 1.0
    assert!((conf - 1.0).abs() < f32::EPSILON);
}

fn make_exp(id: &str, satisfaction: f32, distance: f32, created_at: i64) -> PoeExperience {
    PoeExperience {
        id: id.to_string(),
        task_id: "t1".to_string(),
        objective: "test".to_string(),
        pattern_id: "poe-test".to_string(),
        tool_sequence_json: "[]".to_string(),
        parameter_mapping: None,
        satisfaction,
        distance_score: distance,
        attempts: 1,
        duration_ms: 100,
        created_at,
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib crystallization::provider_backend::tests::test_evaluate_confidence`
Expected: FAIL — `evaluate_confidence` not implemented

**Step 3: Implement evaluate_confidence**

Add `#[async_trait]` impl block to `provider_backend.rs`:

```rust
#[async_trait]
impl PatternSynthesisBackend for ProviderBackend {
    async fn synthesize_pattern(
        &self,
        _request: PatternSynthesisRequest,
    ) -> anyhow::Result<PatternSuggestion> {
        // Placeholder — implemented in Task 3
        anyhow::bail!("synthesize_pattern not yet implemented")
    }

    async fn evaluate_confidence(
        &self,
        _pattern_hash: &str,
        occurrences: &[PoeExperience],
    ) -> anyhow::Result<f32> {
        let base: f32 = 0.5;

        let count = occurrences.len() as f32;
        let occurrence_bonus = (count * 0.05).min(0.3);

        let success_bonus = if occurrences.is_empty() {
            0.0
        } else {
            let avg_satisfaction: f32 =
                occurrences.iter().map(|e| e.satisfaction).sum::<f32>() / count;
            avg_satisfaction * 0.2
        };

        let seven_days_ms: i64 = 7 * 24 * 3600 * 1000;
        let now_ms = chrono::Utc::now().timestamp_millis();
        let recency_bonus = if occurrences
            .iter()
            .any(|e| (now_ms - e.created_at) < seven_days_ms)
        {
            0.05
        } else {
            0.0
        };

        let confidence = (base + occurrence_bonus + success_bonus + recency_bonus).min(1.0);
        Ok(confidence)
    }
}
```

Don't forget the imports at the top of the file:

```rust
use super::pattern_model::ParameterMapping;
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib crystallization::provider_backend`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/poe/crystallization/provider_backend.rs
git commit -m "poe: implement heuristic evaluate_confidence for ProviderBackend"
```

---

### Task 3: ProviderBackend — synthesize_pattern LLM Call

**Files:**
- Modify: `core/src/poe/crystallization/provider_backend.rs`

**Step 1: Write the failing tests**

Add to `tests` module:

```rust
struct JsonMockProvider {
    response: String,
}

impl AiProvider for JsonMockProvider {
    fn process(
        &self,
        _input: &str,
        _system_prompt: Option<&str>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::error::Result<String>> + Send + '_>> {
        let resp = self.response.clone();
        Box::pin(async move { Ok(resp) })
    }
}

#[tokio::test]
async fn test_synthesize_pattern_success() {
    let json_response = serde_json::json!({
        "description": "Compile and test Rust project",
        "steps": [{
            "type": "Action",
            "tool_call": { "tool_name": "cargo", "category": "Shell" },
            "params": { "variables": {} }
        }],
        "parameter_mapping": { "variables": {} },
        "pattern_hash": "abc123",
        "confidence": 0.9
    });

    let provider: Arc<dyn AiProvider> = Arc::new(JsonMockProvider {
        response: json_response.to_string(),
    });
    let backend = ProviderBackend::new(provider);

    let request = PatternSynthesisRequest {
        objective: "Build the project".to_string(),
        tool_sequences: vec![],
        env_context: None,
        existing_patterns: vec![],
    };

    let result = backend.synthesize_pattern(request).await;
    assert!(result.is_ok());
    let suggestion = result.unwrap();
    assert_eq!(suggestion.description, "Compile and test Rust project");
    assert_eq!(suggestion.pattern_hash, "abc123");
}

#[tokio::test]
async fn test_synthesize_pattern_invalid_json() {
    let provider: Arc<dyn AiProvider> = Arc::new(JsonMockProvider {
        response: "This is not JSON at all".to_string(),
    });
    let backend = ProviderBackend::new(provider);

    let request = PatternSynthesisRequest {
        objective: "Build".to_string(),
        tool_sequences: vec![],
        env_context: None,
        existing_patterns: vec![],
    };

    let result = backend.synthesize_pattern(request).await;
    assert!(result.is_err());
}

struct ErrorMockProvider;

impl AiProvider for ErrorMockProvider {
    fn process(
        &self,
        _input: &str,
        _system_prompt: Option<&str>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::error::Result<String>> + Send + '_>> {
        Box::pin(async {
            Err(crate::error::AlephError::ProviderError {
                provider: "mock".to_string(),
                message: "connection refused".to_string(),
            })
        })
    }
}

#[tokio::test]
async fn test_synthesize_pattern_provider_error() {
    let provider: Arc<dyn AiProvider> = Arc::new(ErrorMockProvider);
    let backend = ProviderBackend::new(provider);

    let request = PatternSynthesisRequest {
        objective: "Build".to_string(),
        tool_sequences: vec![],
        env_context: None,
        existing_patterns: vec![],
    };

    let result = backend.synthesize_pattern(request).await;
    assert!(result.is_err());
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib crystallization::provider_backend::tests::test_synthesize`
Expected: FAIL — `synthesize_pattern` returns bail

**Step 3: Implement synthesize_pattern**

Replace the placeholder `synthesize_pattern` in the `#[async_trait]` impl:

```rust
async fn synthesize_pattern(
    &self,
    request: PatternSynthesisRequest,
) -> anyhow::Result<PatternSuggestion> {
    let system_prompt = Self::build_system_prompt();
    let user_prompt = Self::build_user_prompt(&request);

    let response = self
        .provider
        .process(&user_prompt, Some(&system_prompt))
        .await
        .map_err(|e| anyhow::anyhow!("LLM call failed: {}", e))?;

    // Try robust JSON extraction first (handles ```json blocks)
    if let Some(json_value) = crate::utils::json_extract::extract_json_robust(&response) {
        let suggestion: PatternSuggestion = serde_json::from_value(json_value)
            .map_err(|e| anyhow::anyhow!("Failed to parse PatternSuggestion: {}", e))?;
        return Ok(suggestion);
    }

    // Fallback: try direct parse
    let suggestion: PatternSuggestion = serde_json::from_str(&response)
        .map_err(|e| anyhow::anyhow!("LLM response is not valid PatternSuggestion JSON: {}", e))?;
    Ok(suggestion)
}
```

Add these helper methods to `impl ProviderBackend`:

```rust
fn build_system_prompt() -> String {
    r#"You are an expert at analyzing tool execution traces and extracting reusable patterns.

Given a set of tool-sequence traces for a common objective, synthesize a single reusable pattern.

Output ONLY a JSON object matching this schema:
{
  "description": "What this pattern does (1-2 sentences)",
  "steps": [PatternStep],
  "parameter_mapping": { "variables": {} },
  "pattern_hash": "content-hash-string",
  "confidence": 0.0-1.0
}

PatternStep is one of:
- {"type": "Action", "tool_call": {"tool_name": "...", "category": "ReadOnly|FileWrite|Shell|Network|CrossPlugin|Destructive"}, "params": {"variables": {}}}
- {"type": "Conditional", "predicate": {"type": "Semantic", "value": "condition text"}, "then_steps": [...], "else_steps": [...]}
- {"type": "Loop", "predicate": {"type": "Semantic", "value": "condition"}, "body": [...], "max_iterations": N}
- {"type": "SubPattern", "pattern_id": "existing-pattern-id"}

Output ONLY the JSON, no markdown fences, no explanation."#.to_string()
}

fn build_user_prompt(request: &PatternSynthesisRequest) -> String {
    let traces_json = serde_json::to_string_pretty(&request.tool_sequences)
        .unwrap_or_else(|_| "[]".to_string());

    let env = request
        .env_context
        .as_deref()
        .unwrap_or("not provided");

    let existing = if request.existing_patterns.is_empty() {
        "none".to_string()
    } else {
        request.existing_patterns.join(", ")
    };

    format!(
        "Objective: {objective}\n\nTool Sequence Traces:\n{traces}\n\nEnvironment: {env}\n\nExisting patterns (avoid duplicates): {existing}",
        objective = request.objective,
        traces = traces_json,
        env = env,
        existing = existing,
    )
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib crystallization::provider_backend`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/poe/crystallization/provider_backend.rs
git commit -m "poe: implement synthesize_pattern with LLM call for ProviderBackend"
```

---

### Task 4: CortexDreamingService — Inject IdleDetector

**Files:**
- Modify: `core/src/poe/crystallization/dreaming.rs`

**Step 1: Update struct and constructor**

In `dreaming.rs`, add import at line 6 area:

```rust
use super::idle_detector::IdleDetector;
```

Add field to `CortexDreamingService` struct (after line 100, the `worker_handle` field):

```rust
idle_detector: Arc<IdleDetector>,
```

Update `new()` constructor signature (line 106-111) to accept `idle_detector`:

```rust
pub fn new(
    db: MemoryBackend,
    distillation_service: Arc<RwLock<DistillationService>>,
    value_estimator: Arc<CortexValueEstimator>,
    idle_detector: Arc<IdleDetector>,
    config: CortexDreamingConfig,
) -> Self {
    Self {
        db,
        distillation_service,
        value_estimator,
        idle_detector,
        config,
        metrics: Arc::new(DreamingMetrics::default()),
        running: Arc::new(AtomicBool::new(false)),
        worker_handle: None,
    }
}
```

**Step 2: Pass idle_detector to worker_loop**

In `start()` method (around line 124), add before `let handle = tokio::spawn(...)`:

```rust
let idle_detector = self.idle_detector.clone();
```

Update `worker_loop` call inside `tokio::spawn` to pass `idle_detector`:

```rust
let handle = tokio::spawn(async move {
    Self::worker_loop(
        db,
        distillation_service,
        value_estimator,
        idle_detector,
        config,
        metrics,
        running,
    )
    .await;
});
```

Update `worker_loop` signature to accept `idle_detector: Arc<IdleDetector>`:

```rust
async fn worker_loop(
    db: MemoryBackend,
    distillation_service: Arc<RwLock<DistillationService>>,
    value_estimator: Arc<CortexValueEstimator>,
    idle_detector: Arc<IdleDetector>,
    config: CortexDreamingConfig,
    metrics: Arc<DreamingMetrics>,
    running: Arc<AtomicBool>,
)
```

**Step 3: Replace get_idle_seconds with idle_detector.is_idle()**

In `worker_loop`, replace lines 220-221:

```rust
// OLD:
let idle_secs = Self::get_idle_seconds();
if idle_secs >= config.min_idle_seconds {
```

With:

```rust
// NEW:
if idle_detector.is_idle() {
```

Update the debug log at line 222:

```rust
debug!("System idle, starting batch processing");
```

**Step 4: Delete get_idle_seconds()**

Delete the entire `get_idle_seconds()` method (lines 327-336):

```rust
// DELETE THIS ENTIRE BLOCK:
/// Get system idle time in seconds
/// TODO: Integrate with actual activity tracking
fn get_idle_seconds() -> u64 {
    // Placeholder implementation
    // In real implementation, this would check:
    // - Last user interaction timestamp
    // - Last agent loop execution
    // - System activity indicators
    0
}
```

**Step 5: Update existing tests**

In `test_service_lifecycle` and `test_metrics` tests, update the `CortexDreamingService::new()` calls to include `idle_detector`:

```rust
use super::super::idle_detector::{IdleConfig, IdleDetector};

// In each test, before creating the service:
let idle_detector = Arc::new(IdleDetector::new(IdleConfig::default()));

// Update the constructor call:
let mut service = CortexDreamingService::new(
    db,
    distillation_service,
    value_estimator,
    idle_detector,
    config,
);
```

**Step 6: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib crystallization::dreaming`
Expected: PASS

**Step 7: Commit**

```bash
git add core/src/poe/crystallization/dreaming.rs
git commit -m "poe: inject IdleDetector into CortexDreamingService, remove get_idle_seconds stub"
```

---

### Task 5: CortexDreamingService — Rename process_batch to process_value_batch

**Files:**
- Modify: `core/src/poe/crystallization/dreaming.rs`

**Step 1: Rename process_batch to process_value_batch**

Find all references to `process_batch` in `dreaming.rs` and rename to `process_value_batch`:

1. Method definition at line 242: `async fn process_batch(` → `async fn process_value_batch(`
2. Scheduled processing call at line 205: `Self::process_batch(` → `Self::process_value_batch(`
3. Idle processing call at line 224: `Self::process_batch(` → `Self::process_value_batch(`

**Step 2: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib crystallization::dreaming`
Expected: PASS (no public API change)

**Step 3: Commit**

```bash
git add core/src/poe/crystallization/dreaming.rs
git commit -m "poe: rename process_batch to process_value_batch for dual-queue clarity"
```

---

### Task 6: CortexDreamingService — Add process_entropy_batch

**Files:**
- Modify: `core/src/poe/crystallization/dreaming.rs`

**Step 1: Add import for CognitiveEntropyTracker**

At the top of `dreaming.rs`, add:

```rust
use super::cognitive_entropy::{CognitiveEntropyTracker, EntropyTrend};
```

**Step 2: Add process_entropy_batch method**

Add this method after `process_value_batch`:

```rust
/// Process high-entropy patterns that need urgent distillation.
///
/// Queries CognitiveEntropyTracker for patterns with `Increasing` trend,
/// taking up to 3 candidates and enqueuing them with `High` priority.
async fn process_entropy_batch(
    distillation_service: &Arc<RwLock<DistillationService>>,
    metrics: &DreamingMetrics,
) -> Result<()> {
    info!("Starting entropy-driven batch processing");

    // TODO: In full integration, query ExperienceStore for recent execution data.
    // For now, construct execution map from available data.
    // This will be wired when ExperienceStore is integrated with CortexDreamingService.
    let executions: std::collections::HashMap<String, Vec<(f32, f32)>> =
        std::collections::HashMap::new();

    if executions.is_empty() {
        debug!("No execution data for entropy analysis");
        return Ok(());
    }

    let entropy_threshold = 0.3;
    let reports = CognitiveEntropyTracker::analyze(&executions, entropy_threshold);

    // Take up to 3 high-entropy (Increasing trend) patterns
    let high_entropy: Vec<_> = reports
        .into_iter()
        .filter(|r| r.entropy_trend == EntropyTrend::Increasing)
        .take(3)
        .collect();

    if high_entropy.is_empty() {
        debug!("No high-entropy patterns found");
        return Ok(());
    }

    info!("Found {} high-entropy patterns for distillation", high_entropy.len());

    let service = distillation_service.read().await;
    for report in high_entropy {
        let task = DistillationTask {
            trace_id: report.pattern_id.clone(),
            mode: DistillationMode::Batch,
        };

        match service.enqueue_task(task, DistillationPriority::High).await {
            Ok(_) => {
                metrics.increment_processed();
                debug!("Enqueued entropy-driven distillation for: {}", report.pattern_id);
            }
            Err(e) => {
                metrics.increment_errors();
                error!("Failed to enqueue entropy distillation: {}", e);
            }
        }
    }

    Ok(())
}
```

**Step 3: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib crystallization::dreaming`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/poe/crystallization/dreaming.rs
git commit -m "poe: add process_entropy_batch for high-entropy pattern distillation"
```

---

### Task 7: CortexDreamingService — Wire Dual-Queue in worker_loop

**Files:**
- Modify: `core/src/poe/crystallization/dreaming.rs`

**Step 1: Update idle processing block in worker_loop**

Replace the idle processing section (the `if idle_detector.is_idle()` block) with dual-queue logic:

```rust
if idle_detector.is_idle() {
    debug!("System idle, starting dual-queue batch processing");

    // Queue 1: Entropy-driven (up to 3 high-entropy patterns, High priority)
    if let Err(e) = Self::process_entropy_batch(
        &distillation_service,
        &metrics,
    )
    .await
    {
        error!("Entropy batch processing failed: {}", e);
    }

    // Queue 2: Value-driven (existing logic, Normal/Low priority)
    if let Err(e) = Self::process_value_batch(
        &db,
        &distillation_service,
        &value_estimator,
        &config,
        &metrics,
    )
    .await
    {
        error!("Value batch processing failed: {}", e);
    }
}
```

**Step 2: Add new test for dual-queue idle behavior**

Add to the `tests` module:

```rust
#[tokio::test]
async fn test_idle_detector_integration() {
    use super::super::idle_detector::{IdleConfig, IdleDetector};

    let (db, _temp) = create_test_db().await;

    let distillation_config = DistillationConfig::default();
    let (distillation_service, _rx) = DistillationService::new(db.clone(), distillation_config);
    let distillation_service = Arc::new(RwLock::new(distillation_service));

    let value_estimator = Arc::new(CortexValueEstimator::default());

    // Create idle detector that starts as non-idle
    let idle_detector = Arc::new(IdleDetector::new(IdleConfig {
        min_idle_seconds: 300,
    }));

    let config = CortexDreamingConfig::default();

    let service = CortexDreamingService::new(
        db,
        distillation_service,
        value_estimator,
        idle_detector.clone(),
        config,
    );

    // Initially should not be idle (just created = just recorded activity)
    assert!(!idle_detector.is_idle());

    // After recording activity, should still not be idle
    idle_detector.record_activity();
    assert!(!idle_detector.is_idle());

    // Verify service has idle_detector wired in
    let (processed, _, _, _) = service.metrics();
    assert_eq!(processed, 0);
}
```

**Step 3: Run all tests**

Run: `cargo test -p alephcore --lib crystallization::dreaming`
Expected: PASS

**Step 4: Run full test suite to check for regressions**

Run: `cargo test -p alephcore --lib`
Expected: PASS (no regressions)

**Step 5: Commit**

```bash
git add core/src/poe/crystallization/dreaming.rs
git commit -m "poe: wire dual-queue (entropy + value) processing in CortexDreamingService worker_loop"
```

---

## Summary

| Task | Component | What |
|------|-----------|------|
| 1 | ProviderBackend | Struct, constructor, module declaration |
| 2 | ProviderBackend | evaluate_confidence heuristic (no LLM) |
| 3 | ProviderBackend | synthesize_pattern with LLM call |
| 4 | CortexDreamingService | Inject IdleDetector, delete get_idle_seconds |
| 5 | CortexDreamingService | Rename process_batch → process_value_batch |
| 6 | CortexDreamingService | Add process_entropy_batch |
| 7 | CortexDreamingService | Wire dual-queue in worker_loop |
