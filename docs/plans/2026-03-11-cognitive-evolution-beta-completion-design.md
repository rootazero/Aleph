# Cognitive Evolution Beta Completion: ProviderBackend + Dreaming Integration

**Date**: 2026-03-11
**Scope**: Close the last two gaps in the Beta pipeline

---

## 1. Design Decisions Summary

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Provider injection | Constructor injection `Arc<dyn AiProvider>` | Matches `llm_matcher.rs` pattern, YAGNI on hot-swap |
| IdleDetector injection | Direct `Arc<IdleDetector>` into constructor | Single implementation in Beta, no trait needed |
| LLM call strategy | `synthesize` = LLM, `confidence` = heuristic | Token-efficient; confidence inputs are structural |
| Dreaming integration depth | Dual-queue (entropy + value) | Faithful to design doc Section 6; prevents priority dilution |

---

## 2. ProviderBackend (Real LLM Implementation)

### Goal
Implement `PatternSynthesisBackend` trait using `Arc<dyn AiProvider>`, connecting pattern extraction to actual LLM inference.

### Files
- **New**: `core/src/poe/crystallization/provider_backend.rs`
- **Modify**: `core/src/poe/crystallization/mod.rs` (add `pub mod provider_backend`)

### Structure

```rust
pub struct ProviderBackend {
    provider: Arc<dyn AiProvider>,
}

impl ProviderBackend {
    pub fn new(provider: Arc<dyn AiProvider>) -> Self { ... }
}
```

### `synthesize_pattern()` — LLM Call

1. Build system prompt instructing LLM to:
   - Analyze tool-sequence traces
   - Extract a reusable pattern with conditional/loop/subpattern support
   - Output JSON matching `PatternSuggestion` schema
2. Serialize `PatternSynthesisRequest` fields into user prompt:
   - Objective, tool sequences (JSON), env context, existing pattern hashes
3. Call `provider.process(user_prompt, Some(system_prompt))`
4. Parse response JSON into `PatternSuggestion`
5. On parse failure: return `anyhow::bail!` with context

### `evaluate_confidence()` — Heuristic (No LLM)

Formula:
```
base = 0.5
occurrence_bonus = min(count * 0.05, 0.3)
success_bonus = avg_satisfaction * 0.2
recency_bonus = if any execution within 7 days { 0.05 } else { 0.0 }
confidence = min(base + occurrence_bonus + success_bonus + recency_bonus, 1.0)
```

Rationale: Structural inputs (count, satisfaction, timestamps) are sufficient for numeric confidence without LLM inference.

---

## 3. CortexDreamingService Integration

### Goal
Replace placeholder `get_idle_seconds()` with real `IdleDetector`, add entropy-driven dual-queue from design doc Section 6.

### Files
- **Modify**: `core/src/poe/crystallization/dreaming.rs`

### Constructor Change

```rust
pub fn new(
    db: MemoryBackend,
    distillation_service: Arc<RwLock<DistillationService>>,
    value_estimator: Arc<CortexValueEstimator>,
    idle_detector: Arc<IdleDetector>,  // NEW
    config: CortexDreamingConfig,
) -> Self
```

`idle_detector` stored as field, passed to `worker_loop`.

### `worker_loop` Redesign

Replace the idle check section (lines 220-235):

```rust
// OLD: let idle_secs = Self::get_idle_seconds();
// NEW:
if idle_detector.is_idle() {
    debug!("System idle, starting dual-queue batch processing");

    // Queue 1: Entropy-driven (up to 3 high-entropy patterns)
    if let Err(e) = Self::process_entropy_batch(...).await {
        error!("Entropy batch failed: {}", e);
    }

    // Queue 2: Value-driven (existing logic)
    if let Err(e) = Self::process_value_batch(...).await {
        error!("Value batch failed: {}", e);
    }
}
```

### `process_entropy_batch()` — New Method

1. Collect recent execution data per pattern (from experience store or EvolutionTracker)
2. Call `CognitiveEntropyTracker::analyze(executions, entropy_threshold)`
3. Take up to 3 reports where `entropy_trend == Increasing`
4. Enqueue to `DistillationService` with `DistillationPriority::High`

### `process_value_batch()` — Renamed from `process_batch()`

Existing logic unchanged: query candidates → CortexValueEstimator scoring → sort → enqueue with Normal/Low priority.

### Delete

- `get_idle_seconds()` method (lines 327-336) — fully replaced by `IdleDetector`

### Test Updates

- `test_service_lifecycle`: Add `Arc::new(IdleDetector::new(IdleConfig::default()))` to constructor
- `test_metrics`: Same constructor update
- New test: `test_idle_detector_integration` — verify `worker_loop` respects idle state

---

## 4. Constraints & Invariants

1. **ProviderBackend never panics on LLM failure** — returns `Err` with context
2. **Confidence always in [0.0, 1.0]** — clamped by `min(..., 1.0)`
3. **Entropy batch capped at 3** — prevents entropy queue from starving value queue
4. **Rate limit preserved** — `max_distillations_per_minute` applies across both queues combined
5. **IdleDetector is Clone** — safe to share via `Arc` across threads
6. **No breaking public API** — only `CortexDreamingService::new()` signature changes (internal type)
