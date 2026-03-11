# Cognitive Evolution Beta Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Close the open-loop skill evolution pipeline with tiered validation, shadow deployment, enhanced pattern model, and entropy-driven dreaming.

**Architecture:** Six components build bottom-up: (1) Pattern model types, (2) Synthesis backend trait + provider impl, (3) Tiered validation gate, (4) Shadow deployer + lifecycle, (5) Dreaming triggers + clustering storage, (6) Full pipeline integration. Each component has its own test suite. TDD throughout.

**Tech Stack:** Rust (async_trait, tokio, serde, rusqlite), existing ProviderManager for LLM calls, existing ExperienceStore/EvolutionTracker for persistence.

**Design Doc:** `docs/plans/2026-03-11-cognitive-evolution-beta-design.md`

---

## Task 1: Enhanced Sequence Pattern Model Types

**Files:**
- Create: `core/src/poe/crystallization/pattern_model.rs`
- Modify: `core/src/poe/crystallization/mod.rs` (add `pub mod pattern_model;`)
- Test: inline `#[cfg(test)] mod tests` in `pattern_model.rs`

**Step 1: Write failing tests for PatternStep serialization**

```rust
// core/src/poe/crystallization/pattern_model.rs — tests at bottom

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_step_roundtrip() {
        let step = PatternStep::Action {
            tool_call: ToolCallTemplate {
                tool_name: "search_codebase".to_string(),
                category: ToolCategory::ReadOnly,
            },
            params: ParameterMapping {
                variables: [("query".to_string(), "test".to_string())].into_iter().collect(),
            },
        };
        let json = serde_json::to_string(&step).unwrap();
        let back: PatternStep = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, PatternStep::Action { .. }));
    }

    #[test]
    fn test_conditional_step_roundtrip() {
        let step = PatternStep::Conditional {
            predicate: Predicate::MetricThreshold {
                metric: CognitiveMetric::Entropy,
                op: CompareOp::Gt,
                threshold: 0.5,
            },
            then_steps: vec![PatternStep::Action {
                tool_call: ToolCallTemplate {
                    tool_name: "search_more".to_string(),
                    category: ToolCategory::ReadOnly,
                },
                params: ParameterMapping { variables: Default::default() },
            }],
            else_steps: vec![],
        };
        let json = serde_json::to_string(&step).unwrap();
        let back: PatternStep = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, PatternStep::Conditional { .. }));
    }

    #[test]
    fn test_loop_step_max_iterations_constraint() {
        let step = PatternStep::Loop {
            predicate: Predicate::Semantic("result_incomplete".to_string()),
            body: vec![],
            max_iterations: 15,
        };
        // max_iterations > 10 should be clamped by validate()
        let seq = PatternSequence {
            description: "test".to_string(),
            steps: vec![step],
            expected_outputs: vec![],
        };
        let errors = seq.validate();
        assert!(!errors.is_empty());
        assert!(errors[0].contains("max_iterations"));
    }

    #[test]
    fn test_subpattern_nesting_depth() {
        // Build 4-level deep nesting (should fail at depth > 3)
        let deep = PatternStep::SubPattern { pattern_id: "level4".to_string() };
        let level3 = PatternStep::Conditional {
            predicate: Predicate::Semantic("x".to_string()),
            then_steps: vec![deep],
            else_steps: vec![],
        };
        let level2 = PatternStep::Conditional {
            predicate: Predicate::Semantic("x".to_string()),
            then_steps: vec![level3],
            else_steps: vec![],
        };
        let level1 = PatternStep::Conditional {
            predicate: Predicate::Semantic("x".to_string()),
            then_steps: vec![level2],
            else_steps: vec![],
        };
        let seq = PatternSequence {
            description: "too deep".to_string(),
            steps: vec![level1],
            expected_outputs: vec![],
        };
        let errors = seq.validate();
        assert!(errors.iter().any(|e| e.contains("nesting depth")));
    }

    #[test]
    fn test_semantic_predicate_length_limit() {
        let long_predicate = "x".repeat(201);
        let step = PatternStep::Loop {
            predicate: Predicate::Semantic(long_predicate),
            body: vec![],
            max_iterations: 3,
        };
        let seq = PatternSequence {
            description: "test".to_string(),
            steps: vec![step],
            expected_outputs: vec![],
        };
        let errors = seq.validate();
        assert!(errors.iter().any(|e| e.contains("200")));
    }

    #[test]
    fn test_pattern_step_cost_estimation() {
        let action = PatternStep::Action {
            tool_call: ToolCallTemplate {
                tool_name: "read".to_string(),
                category: ToolCategory::ReadOnly,
            },
            params: ParameterMapping { variables: Default::default() },
        };
        assert!((action.estimated_cost() - 1.0).abs() < f32::EPSILON);

        let looping = PatternStep::Loop {
            predicate: Predicate::Semantic("check".to_string()),
            body: vec![action.clone()],
            max_iterations: 4,
        };
        // body_cost(1.0) * (max/2=2.0) + 0.5 = 2.5
        assert!((looping.estimated_cost() - 2.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_iter_all_steps_flattens() {
        let seq = PatternSequence {
            description: "test".to_string(),
            steps: vec![
                PatternStep::Action {
                    tool_call: ToolCallTemplate {
                        tool_name: "a".to_string(),
                        category: ToolCategory::ReadOnly,
                    },
                    params: ParameterMapping { variables: Default::default() },
                },
                PatternStep::Conditional {
                    predicate: Predicate::Semantic("cond".to_string()),
                    then_steps: vec![PatternStep::Action {
                        tool_call: ToolCallTemplate {
                            tool_name: "b".to_string(),
                            category: ToolCategory::ReadOnly,
                        },
                        params: ParameterMapping { variables: Default::default() },
                    }],
                    else_steps: vec![PatternStep::Action {
                        tool_call: ToolCallTemplate {
                            tool_name: "c".to_string(),
                            category: ToolCategory::ReadOnly,
                        },
                        params: ParameterMapping { variables: Default::default() },
                    }],
                },
            ],
            expected_outputs: vec![],
        };
        let all: Vec<_> = seq.iter_all_steps().collect();
        assert_eq!(all.len(), 3); // a, b, c (not the Conditional itself)
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib pattern_model -- --nocapture 2>&1 | head -20`
Expected: compilation errors — module and types don't exist yet.

**Step 3: Implement pattern_model.rs**

```rust
// core/src/poe/crystallization/pattern_model.rs

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// --- Predicates ---

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum Predicate {
    Semantic(String),
    MetricThreshold {
        metric: CognitiveMetric,
        op: CompareOp,
        threshold: f32,
    },
    And(Vec<Predicate>),
    Or(Vec<Predicate>),
    Not(Box<Predicate>),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum CognitiveMetric {
    Entropy,
    TrustScore,
    RemainingBudgetRatio,
    AttemptCount,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum CompareOp {
    Gt,
    Gte,
    Lt,
    Lte,
    Eq,
}

// --- Tool Classification ---

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ToolCategory {
    ReadOnly,
    FileWrite,
    CrossPlugin,
    Shell,
    Network,
    Destructive,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolCallTemplate {
    pub tool_name: String,
    pub category: ToolCategory,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct ParameterMapping {
    pub variables: HashMap<String, String>,
}

// --- Pattern Steps ---

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "step_type")]
pub enum PatternStep {
    Action {
        tool_call: ToolCallTemplate,
        params: ParameterMapping,
    },
    Conditional {
        predicate: Predicate,
        then_steps: Vec<PatternStep>,
        else_steps: Vec<PatternStep>,
    },
    Loop {
        predicate: Predicate,
        body: Vec<PatternStep>,
        max_iterations: u32,
    },
    SubPattern {
        pattern_id: String,
    },
}

impl PatternStep {
    /// Estimate execution cost of this step
    pub fn estimated_cost(&self) -> f32 {
        match self {
            PatternStep::Action { .. } => 1.0,
            PatternStep::Conditional {
                then_steps,
                else_steps,
                ..
            } => {
                let then_cost: f32 = then_steps.iter().map(|s| s.estimated_cost()).sum();
                let else_cost: f32 = else_steps.iter().map(|s| s.estimated_cost()).sum();
                0.5 * (then_cost + else_cost) + 0.5
            }
            PatternStep::Loop {
                body,
                max_iterations,
                ..
            } => {
                let body_cost: f32 = body.iter().map(|s| s.estimated_cost()).sum();
                body_cost * (*max_iterations as f32 / 2.0) + 0.5
            }
            PatternStep::SubPattern { .. } => 3.0,
        }
    }

    /// Iterate over all leaf Action steps, recursively flattening
    fn collect_actions<'a>(&'a self, out: &mut Vec<&'a PatternStep>) {
        match self {
            PatternStep::Action { .. } => out.push(self),
            PatternStep::Conditional {
                then_steps,
                else_steps,
                ..
            } => {
                for s in then_steps {
                    s.collect_actions(out);
                }
                for s in else_steps {
                    s.collect_actions(out);
                }
            }
            PatternStep::Loop { body, .. } => {
                for s in body {
                    s.collect_actions(out);
                }
            }
            PatternStep::SubPattern { .. } => out.push(self),
        }
    }

    /// Check nesting depth of SubPattern references
    fn max_nesting_depth(&self, current: u32) -> u32 {
        match self {
            PatternStep::Action { .. } | PatternStep::SubPattern { .. } => current,
            PatternStep::Conditional {
                then_steps,
                else_steps,
                ..
            } => {
                let then_max = then_steps
                    .iter()
                    .map(|s| s.max_nesting_depth(current + 1))
                    .max()
                    .unwrap_or(current);
                let else_max = else_steps
                    .iter()
                    .map(|s| s.max_nesting_depth(current + 1))
                    .max()
                    .unwrap_or(current);
                then_max.max(else_max)
            }
            PatternStep::Loop { body, .. } => body
                .iter()
                .map(|s| s.max_nesting_depth(current + 1))
                .max()
                .unwrap_or(current),
        }
    }

    /// Validate constraints on this step, collecting errors
    fn validate_step(&self, errors: &mut Vec<String>, depth: u32) {
        match self {
            PatternStep::Loop {
                predicate,
                body,
                max_iterations,
            } => {
                if *max_iterations == 0 || *max_iterations > 10 {
                    errors.push(format!(
                        "max_iterations must be 1..=10, got {}",
                        max_iterations
                    ));
                }
                if let Predicate::Semantic(s) = predicate {
                    if s.len() > 200 {
                        errors.push(format!(
                            "Semantic predicate exceeds 200 chars (got {})",
                            s.len()
                        ));
                    }
                }
                for s in body {
                    s.validate_step(errors, depth + 1);
                }
            }
            PatternStep::Conditional {
                predicate,
                then_steps,
                else_steps,
            } => {
                if let Predicate::Semantic(s) = predicate {
                    if s.len() > 200 {
                        errors.push(format!(
                            "Semantic predicate exceeds 200 chars (got {})",
                            s.len()
                        ));
                    }
                }
                if depth > 3 {
                    errors.push(format!("nesting depth {} exceeds max 3", depth));
                }
                for s in then_steps {
                    s.validate_step(errors, depth + 1);
                }
                for s in else_steps {
                    s.validate_step(errors, depth + 1);
                }
            }
            PatternStep::SubPattern { .. } => {
                if depth > 3 {
                    errors.push(format!("nesting depth {} exceeds max 3", depth));
                }
            }
            PatternStep::Action { .. } => {}
        }
    }
}

// --- Pattern Sequence ---

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PatternSequence {
    pub description: String,
    pub steps: Vec<PatternStep>,
    pub expected_outputs: Vec<String>,
}

impl PatternSequence {
    /// Validate all constraints. Returns empty vec if valid.
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        for step in &self.steps {
            step.validate_step(&mut errors, 0);
        }
        errors
    }

    /// Iterate over all leaf steps (Actions and SubPatterns), flattened
    pub fn iter_all_steps(&self) -> Vec<&PatternStep> {
        let mut out = Vec::new();
        for step in &self.steps {
            step.collect_actions(&mut out);
        }
        out
    }

    /// Total estimated cost of executing this pattern
    pub fn estimated_total_cost(&self) -> f32 {
        self.steps.iter().map(|s| s.estimated_cost()).sum()
    }
}
```

**Step 4: Register module in mod.rs**

Add `pub mod pattern_model;` to `core/src/poe/crystallization/mod.rs` after line 43 (after existing module declarations).

**Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib pattern_model -- --nocapture`
Expected: all 7 tests PASS.

**Step 6: Commit**

```bash
git add core/src/poe/crystallization/pattern_model.rs core/src/poe/crystallization/mod.rs
git commit -m "poe: add enhanced sequence pattern model with predicates and validation"
```

---

## Task 2: PatternSynthesisBackend Trait

**Files:**
- Create: `core/src/poe/crystallization/synthesis_backend.rs`
- Modify: `core/src/poe/crystallization/mod.rs` (add `pub mod synthesis_backend;`)
- Test: inline `#[cfg(test)] mod tests` in `synthesis_backend.rs`

**Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::crystallization::pattern_model::*;

    struct MockBackend {
        suggestion: PatternSuggestion,
    }

    #[async_trait::async_trait]
    impl PatternSynthesisBackend for MockBackend {
        async fn synthesize_pattern(
            &self,
            _request: PatternSynthesisRequest,
        ) -> anyhow::Result<PatternSuggestion> {
            Ok(self.suggestion.clone())
        }

        async fn evaluate_confidence(
            &self,
            _pattern_hash: &str,
            _occurrences: &[crate::poe::crystallization::experience_store::PoeExperience],
        ) -> anyhow::Result<f32> {
            Ok(0.85)
        }
    }

    #[tokio::test]
    async fn test_mock_backend_synthesize() {
        let backend = MockBackend {
            suggestion: PatternSuggestion {
                description: "search and fix".to_string(),
                steps: vec![PatternStep::Action {
                    tool_call: ToolCallTemplate {
                        tool_name: "search".to_string(),
                        category: ToolCategory::ReadOnly,
                    },
                    params: ParameterMapping::default(),
                }],
                parameter_mapping: ParameterMapping::default(),
                pattern_hash: "abc123".to_string(),
                confidence: 0.9,
            },
        };

        let request = PatternSynthesisRequest {
            objective: "fix bug".to_string(),
            tool_sequences: vec![],
            env_context: None,
            existing_patterns: vec![],
        };

        let result = backend.synthesize_pattern(request).await.unwrap();
        assert_eq!(result.confidence, 0.9);
        assert_eq!(result.steps.len(), 1);
    }

    #[tokio::test]
    async fn test_mock_backend_evaluate_confidence() {
        let backend = MockBackend {
            suggestion: PatternSuggestion {
                description: String::new(),
                steps: vec![],
                parameter_mapping: ParameterMapping::default(),
                pattern_hash: String::new(),
                confidence: 0.0,
            },
        };
        let conf = backend.evaluate_confidence("pattern1", &[]).await.unwrap();
        assert!((conf - 0.85).abs() < f32::EPSILON);
    }
}
```

**Step 2: Run tests — expect compilation failure**

Run: `cargo test -p alephcore --lib synthesis_backend 2>&1 | head -10`

**Step 3: Implement synthesis_backend.rs**

```rust
// core/src/poe/crystallization/synthesis_backend.rs

use super::experience_store::PoeExperience;
use super::pattern_model::{ParameterMapping, PatternStep};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Trace of a tool sequence from a historical execution
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolSequenceTrace {
    pub tool_sequence_json: String,
    pub satisfaction: f32,
    pub duration_ms: u64,
    pub attempts: u8,
}

/// Request for pattern synthesis from execution traces
#[derive(Clone, Debug)]
pub struct PatternSynthesisRequest {
    pub objective: String,
    pub tool_sequences: Vec<ToolSequenceTrace>,
    pub env_context: Option<String>,
    pub existing_patterns: Vec<String>,
}

/// Result of pattern synthesis
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PatternSuggestion {
    pub description: String,
    pub steps: Vec<PatternStep>,
    pub parameter_mapping: ParameterMapping,
    pub pattern_hash: String,
    pub confidence: f32,
}

/// Backend trait for LLM-powered pattern synthesis.
/// Implementations can use ProviderManager, local models, or mocks.
#[async_trait]
pub trait PatternSynthesisBackend: Send + Sync {
    /// Synthesize abstract pattern from execution traces
    async fn synthesize_pattern(
        &self,
        request: PatternSynthesisRequest,
    ) -> anyhow::Result<PatternSuggestion>;

    /// Evaluate confidence of existing pattern against new occurrences
    async fn evaluate_confidence(
        &self,
        pattern_hash: &str,
        occurrences: &[PoeExperience],
    ) -> anyhow::Result<f32>;
}
```

**Step 4: Register module**

Add `pub mod synthesis_backend;` to `core/src/poe/crystallization/mod.rs`.

**Step 5: Run tests — expect PASS**

Run: `cargo test -p alephcore --lib synthesis_backend`

**Step 6: Commit**

```bash
git add core/src/poe/crystallization/synthesis_backend.rs core/src/poe/crystallization/mod.rs
git commit -m "poe: add PatternSynthesisBackend trait for dependency-inverted LLM access"
```

---

## Task 3: Refactor PatternExtractor to Use Backend

**Files:**
- Modify: `core/src/poe/crystallization/pattern_extractor.rs` (inject backend)
- Test: existing tests + new integration test

**Step 1: Write failing test for backend injection**

Add to `pattern_extractor.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::crystallization::synthesis_backend::*;
    use crate::poe::crystallization::pattern_model::*;

    struct StubBackend;

    #[async_trait::async_trait]
    impl PatternSynthesisBackend for StubBackend {
        async fn synthesize_pattern(
            &self,
            request: PatternSynthesisRequest,
        ) -> anyhow::Result<PatternSuggestion> {
            Ok(PatternSuggestion {
                description: format!("Pattern for: {}", request.objective),
                steps: vec![PatternStep::Action {
                    tool_call: ToolCallTemplate {
                        tool_name: "generated_tool".to_string(),
                        category: ToolCategory::ReadOnly,
                    },
                    params: ParameterMapping::default(),
                }],
                parameter_mapping: ParameterMapping::default(),
                pattern_hash: "stub_hash".to_string(),
                confidence: 0.95,
            })
        }

        async fn evaluate_confidence(
            &self,
            _pattern_hash: &str,
            _occurrences: &[crate::poe::crystallization::experience_store::PoeExperience],
        ) -> anyhow::Result<f32> {
            Ok(0.9)
        }
    }

    #[tokio::test]
    async fn test_extract_with_backend() {
        let backend = Arc::new(StubBackend);
        let extractor = PatternExtractor::with_backend(
            PatternExtractorConfig::default(),
            backend,
        );
        // Construct a minimal Experience for extraction
        let experience = Experience::builder("test_intent", "test_hash")
            .success_score(0.9)
            .build();
        let result = extractor.extract_pattern(&experience, false).await;
        assert!(result.is_ok());
        let pattern = result.unwrap();
        assert!(pattern.description.contains("test_intent"));
    }
}
```

**Step 2: Run test — expect failure (no `with_backend` constructor)**

**Step 3: Modify PatternExtractor**

Add `backend` field and `with_backend` constructor to `PatternExtractor` struct at line 48:

```rust
pub struct PatternExtractor {
    config: PatternExtractorConfig,
    backend: Option<Arc<dyn PatternSynthesisBackend>>,
}

impl PatternExtractor {
    pub fn new(config: PatternExtractorConfig) -> Self {
        Self { config, backend: None }
    }

    pub fn with_backend(
        config: PatternExtractorConfig,
        backend: Arc<dyn PatternSynthesisBackend>,
    ) -> Self {
        Self { config, backend: Some(backend) }
    }
}
```

Modify `extract_pattern` method (line 59) to use backend when available:

```rust
pub async fn extract_pattern(
    &self,
    experience: &Experience,
    use_realtime_model: bool,
) -> Result<ExtractedPattern> {
    if let Some(backend) = &self.backend {
        let request = self.build_synthesis_request(experience);
        let suggestion = backend.synthesize_pattern(request).await?;
        return Ok(ExtractedPattern {
            description: suggestion.description,
            parameter_mapping: self.convert_mapping(&suggestion.parameter_mapping),
            pattern_hash: suggestion.pattern_hash,
        });
    }

    // Existing stub/placeholder logic below...
    // (keep existing code as fallback)
}
```

Add helper method `build_synthesis_request`:

```rust
fn build_synthesis_request(&self, experience: &Experience) -> PatternSynthesisRequest {
    PatternSynthesisRequest {
        objective: experience.user_intent.clone(),
        tool_sequences: vec![ToolSequenceTrace {
            tool_sequence_json: experience.tool_sequence_json.clone(),
            satisfaction: experience.success_score,
            duration_ms: experience.latency_ms,
            attempts: 1,
        }],
        env_context: None,
        existing_patterns: vec![],
    }
}
```

**Step 4: Run tests — expect PASS**

Run: `cargo test -p alephcore --lib pattern_extractor`

**Step 5: Run full check**

Run: `cargo check -p alephcore`

**Step 6: Commit**

```bash
git add core/src/poe/crystallization/pattern_extractor.rs
git commit -m "poe: inject PatternSynthesisBackend into PatternExtractor"
```

---

## Task 4: ExperienceStore API Extensions

**Files:**
- Modify: `core/src/poe/crystallization/experience_store.rs` (add `delete`, `get_by_ids`)
- Test: extend existing tests in same file

**Step 1: Write failing tests**

```rust
// Add to existing tests in experience_store.rs

#[tokio::test]
async fn test_delete_experience() {
    let store = InMemoryExperienceStore::new();
    let exp = make_test_experience("exp1");
    let embedding = vec![1.0, 0.0, 0.0];
    store.insert(exp, &embedding).await.unwrap();

    assert_eq!(store.count().await.unwrap(), 1);
    let deleted = store.delete("exp1").await.unwrap();
    assert!(deleted);
    assert_eq!(store.count().await.unwrap(), 0);

    // Delete non-existent
    let deleted = store.delete("nonexistent").await.unwrap();
    assert!(!deleted);
}

#[tokio::test]
async fn test_get_by_ids() {
    let store = InMemoryExperienceStore::new();
    for i in 0..5 {
        let exp = make_test_experience(&format!("exp{}", i));
        store.insert(exp, &vec![i as f32, 0.0, 0.0]).await.unwrap();
    }

    let results = store
        .get_by_ids(&["exp1".to_string(), "exp3".to_string(), "exp99".to_string()])
        .await
        .unwrap();
    assert_eq!(results.len(), 2);
    assert!(results.iter().any(|e| e.id == "exp1"));
    assert!(results.iter().any(|e| e.id == "exp3"));
}
```

**Step 2: Run tests — expect compilation failure**

**Step 3: Add methods to ExperienceStore trait**

Add to trait definition at line 60:

```rust
#[async_trait]
pub trait ExperienceStore: Send + Sync {
    // ... existing methods ...

    /// Delete an experience by ID. Returns true if found and deleted.
    async fn delete(&self, experience_id: &str) -> Result<bool>;

    /// Retrieve multiple experiences by ID. Missing IDs are silently skipped.
    async fn get_by_ids(&self, ids: &[String]) -> Result<Vec<PoeExperience>>;
}
```

Implement for `InMemoryExperienceStore`:

```rust
async fn delete(&self, experience_id: &str) -> Result<bool> {
    let mut entries = self.entries.write().unwrap_or_else(|e| e.into_inner());
    let before = entries.len();
    entries.retain(|e| e.experience.id != experience_id);
    Ok(entries.len() < before)
}

async fn get_by_ids(&self, ids: &[String]) -> Result<Vec<PoeExperience>> {
    let entries = self.entries.read().unwrap_or_else(|e| e.into_inner());
    Ok(entries
        .iter()
        .filter(|e| ids.contains(&e.experience.id))
        .map(|e| e.experience.clone())
        .collect())
}
```

**Step 4: Run tests — expect PASS**

Run: `cargo test -p alephcore --lib experience_store`

**Step 5: Check compilation of downstream code**

Run: `cargo check -p alephcore`

Note: Any other implementations of ExperienceStore (e.g., LanceDB-backed) will need the new methods too. If there's a compile error from another impl, add stub implementations returning `Ok(false)` / `Ok(vec![])` with a TODO comment.

**Step 6: Commit**

```bash
git add core/src/poe/crystallization/experience_store.rs
git commit -m "poe: extend ExperienceStore trait with delete and get_by_ids"
```

---

## Task 5: Skill Risk Profiler

**Files:**
- Create: `core/src/skill_evolution/validation/mod.rs`
- Create: `core/src/skill_evolution/validation/risk_profiler.rs`
- Modify: `core/src/skill_evolution/mod.rs` (add `pub mod validation;`)
- Test: inline in `risk_profiler.rs`

**Step 1: Write failing tests**

```rust
// core/src/skill_evolution/validation/risk_profiler.rs — tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::crystallization::pattern_model::*;

    fn action(name: &str, category: ToolCategory) -> PatternStep {
        PatternStep::Action {
            tool_call: ToolCallTemplate {
                tool_name: name.to_string(),
                category,
            },
            params: ParameterMapping::default(),
        }
    }

    #[test]
    fn test_readonly_is_low_risk() {
        let seq = PatternSequence {
            description: "read-only".to_string(),
            steps: vec![action("search", ToolCategory::ReadOnly)],
            expected_outputs: vec![],
        };
        let profile = SkillRiskProfiler::profile(&seq);
        assert_eq!(profile.level, SkillRiskLevel::Low);
    }

    #[test]
    fn test_file_write_is_medium_risk() {
        let seq = PatternSequence {
            description: "writes files".to_string(),
            steps: vec![
                action("search", ToolCategory::ReadOnly),
                action("write_file", ToolCategory::FileWrite),
            ],
            expected_outputs: vec![],
        };
        let profile = SkillRiskProfiler::profile(&seq);
        assert_eq!(profile.level, SkillRiskLevel::Medium);
    }

    #[test]
    fn test_shell_is_high_risk() {
        let seq = PatternSequence {
            description: "runs shell".to_string(),
            steps: vec![action("exec_cmd", ToolCategory::Shell)],
            expected_outputs: vec![],
        };
        let profile = SkillRiskProfiler::profile(&seq);
        assert_eq!(profile.level, SkillRiskLevel::High);
    }

    #[test]
    fn test_high_iteration_loop_escalates() {
        let seq = PatternSequence {
            description: "loop heavy".to_string(),
            steps: vec![PatternStep::Loop {
                predicate: Predicate::Semantic("check".to_string()),
                body: vec![action("read", ToolCategory::ReadOnly)],
                max_iterations: 8,
            }],
            expected_outputs: vec![],
        };
        let profile = SkillRiskProfiler::profile(&seq);
        assert_eq!(profile.level, SkillRiskLevel::Medium);
    }

    #[test]
    fn test_risk_level_ordering() {
        assert!(SkillRiskLevel::Low < SkillRiskLevel::Medium);
        assert!(SkillRiskLevel::Medium < SkillRiskLevel::High);
    }
}
```

**Step 2: Run tests — expect failure**

**Step 3: Create validation module**

```rust
// core/src/skill_evolution/validation/mod.rs
pub mod risk_profiler;

pub use risk_profiler::{SkillRiskLevel, SkillRiskProfile, SkillRiskProfiler};
```

```rust
// core/src/skill_evolution/validation/risk_profiler.rs

use crate::poe::crystallization::pattern_model::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SkillRiskLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRiskProfile {
    pub level: SkillRiskLevel,
    pub reasoning: String,
}

pub struct SkillRiskProfiler;

impl SkillRiskProfiler {
    pub fn profile(pattern: &PatternSequence) -> SkillRiskProfile {
        let mut level = SkillRiskLevel::Low;
        let mut reasons = Vec::new();

        for step in &pattern.steps {
            Self::profile_step(step, &mut level, &mut reasons);
        }

        SkillRiskProfile {
            reasoning: if reasons.is_empty() {
                "read-only operations only".to_string()
            } else {
                reasons.join("; ")
            },
            level,
        }
    }

    fn profile_step(step: &PatternStep, level: &mut SkillRiskLevel, reasons: &mut Vec<String>) {
        match step {
            PatternStep::Action { tool_call, .. } => {
                let tool_level = Self::classify_tool(&tool_call.category);
                if tool_level > *level {
                    reasons.push(format!(
                        "tool '{}' is {:?}",
                        tool_call.tool_name, tool_call.category
                    ));
                    *level = (*level).clone().max(tool_level);
                }
            }
            PatternStep::Loop {
                body,
                max_iterations,
                ..
            } => {
                if *max_iterations > 5 && *level < SkillRiskLevel::Medium {
                    reasons.push(format!("loop with {} iterations", max_iterations));
                    *level = SkillRiskLevel::Medium;
                }
                for s in body {
                    Self::profile_step(s, level, reasons);
                }
            }
            PatternStep::Conditional {
                then_steps,
                else_steps,
                ..
            } => {
                for s in then_steps {
                    Self::profile_step(s, level, reasons);
                }
                for s in else_steps {
                    Self::profile_step(s, level, reasons);
                }
            }
            PatternStep::SubPattern { .. } => {
                if *level < SkillRiskLevel::Medium {
                    reasons.push("delegates to sub-pattern".to_string());
                    *level = SkillRiskLevel::Medium;
                }
            }
        }
    }

    fn classify_tool(category: &ToolCategory) -> SkillRiskLevel {
        match category {
            ToolCategory::ReadOnly => SkillRiskLevel::Low,
            ToolCategory::FileWrite | ToolCategory::CrossPlugin => SkillRiskLevel::Medium,
            ToolCategory::Shell | ToolCategory::Network | ToolCategory::Destructive => {
                SkillRiskLevel::High
            }
        }
    }
}
```

Add `pub mod validation;` to `core/src/skill_evolution/mod.rs`.

**Step 4: Run tests — expect PASS**

Run: `cargo test -p alephcore --lib risk_profiler`

**Step 5: Commit**

```bash
git add core/src/skill_evolution/validation/ core/src/skill_evolution/mod.rs
git commit -m "evolution: add skill risk profiler with tiered classification"
```

---

## Task 6: Test Set Generator (Cluster + Boundary Sampling)

**Files:**
- Create: `core/src/skill_evolution/validation/test_set_generator.rs`
- Modify: `core/src/skill_evolution/validation/mod.rs`
- Test: inline

**Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::crystallization::experience_store::{InMemoryExperienceStore, PoeExperience};

    fn make_experience(id: &str, satisfaction: f32, duration_ms: u64) -> PoeExperience {
        PoeExperience {
            id: id.to_string(),
            task_id: format!("task_{}", id),
            objective: "test objective".to_string(),
            pattern_id: "pattern_a".to_string(),
            tool_sequence_json: "[]".to_string(),
            parameter_mapping: None,
            satisfaction,
            distance_score: 1.0 - satisfaction,
            attempts: 1,
            duration_ms,
            created_at: 0,
        }
    }

    #[tokio::test]
    async fn test_generate_from_successful_experiences() {
        let store = InMemoryExperienceStore::new();

        // Insert 6 successful experiences with same pattern
        for i in 0..6 {
            let exp = make_experience(&format!("e{}", i), 0.9, 100 + i * 10);
            // Use slightly different embeddings so clustering produces 2+ clusters
            let embedding = vec![1.0, i as f32 * 0.1, 0.0];
            store.insert(exp, &embedding).await.unwrap();
        }

        let gen = TestSetGenerator::new(8);
        let test_set = gen.generate("pattern_a", &store).await.unwrap();

        // Should have samples but no more than max
        assert!(!test_set.samples.is_empty());
        assert!(test_set.samples.len() <= 8);
    }

    #[tokio::test]
    async fn test_boundary_cases_included() {
        let store = InMemoryExperienceStore::new();

        // One normal, one boundary (very long duration)
        store
            .insert(make_experience("normal", 0.9, 100), &vec![1.0, 0.0, 0.0])
            .await
            .unwrap();
        store
            .insert(make_experience("slow", 0.85, 10000), &vec![1.0, 0.1, 0.0])
            .await
            .unwrap();

        let gen = TestSetGenerator::new(8);
        let test_set = gen.generate("pattern_a", &store).await.unwrap();

        let has_boundary = test_set
            .samples
            .iter()
            .any(|s| matches!(s.source, SampleSource::BoundaryCase { .. }));
        // With only 2 experiences, boundary detection depends on variance
        // At minimum we should get both samples
        assert!(test_set.samples.len() >= 2 || has_boundary);
    }

    #[tokio::test]
    async fn test_filters_low_satisfaction() {
        let store = InMemoryExperienceStore::new();

        store
            .insert(make_experience("good", 0.9, 100), &vec![1.0, 0.0, 0.0])
            .await
            .unwrap();
        store
            .insert(make_experience("bad", 0.3, 100), &vec![0.0, 1.0, 0.0])
            .await
            .unwrap();

        let gen = TestSetGenerator::new(8);
        let test_set = gen.generate("pattern_a", &store).await.unwrap();

        // Should only include the good one
        assert_eq!(test_set.samples.len(), 1);
        assert_eq!(test_set.samples[0].experience.id, "good");
    }
}
```

**Step 2: Run tests — expect failure**

**Step 3: Implement test_set_generator.rs**

```rust
// core/src/skill_evolution/validation/test_set_generator.rs

use crate::poe::crystallization::experience_store::{ExperienceStore, PoeExperience};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationTestSet {
    pub samples: Vec<TestSample>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestSample {
    pub experience: PoeExperience,
    pub source: SampleSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SampleSource {
    ClusterRepresentative,
    BoundaryCase { dimension: String },
}

pub struct TestSetGenerator {
    max_samples: usize,
    min_satisfaction: f32,
}

impl TestSetGenerator {
    pub fn new(max_samples: usize) -> Self {
        Self {
            max_samples,
            min_satisfaction: 0.8,
        }
    }

    pub async fn generate(
        &self,
        pattern_id: &str,
        store: &dyn ExperienceStore,
    ) -> anyhow::Result<ValidationTestSet> {
        let experiences = store.get_by_pattern_id(pattern_id).await?;
        let successful: Vec<_> = experiences
            .into_iter()
            .filter(|e| e.satisfaction >= self.min_satisfaction)
            .collect();

        if successful.is_empty() {
            return Ok(ValidationTestSet {
                samples: vec![],
            });
        }

        let mut samples = Vec::new();

        // Step 1: Take representatives — for now, deduplicate by taking
        // unique objective strings and pick highest satisfaction per group.
        // Full clustering integration will come when ClusteringService is wired.
        let mut groups: std::collections::HashMap<String, Vec<&PoeExperience>> =
            std::collections::HashMap::new();
        for exp in &successful {
            groups
                .entry(exp.objective.clone())
                .or_default()
                .push(exp);
        }

        for (_key, group) in &groups {
            if let Some(best) = group.iter().max_by(|a, b| {
                a.satisfaction
                    .partial_cmp(&b.satisfaction)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }) {
                samples.push(TestSample {
                    experience: (*best).clone(),
                    source: SampleSource::ClusterRepresentative,
                });
            }
        }

        // Step 2: Add boundary cases (max 2)
        let boundary_cases = self.find_boundary_cases(&successful);
        for (dim, exp) in boundary_cases {
            if samples.len() >= self.max_samples {
                break;
            }
            if !samples.iter().any(|s| s.experience.id == exp.id) {
                samples.push(TestSample {
                    experience: exp.clone(),
                    source: SampleSource::BoundaryCase { dimension: dim },
                });
            }
        }

        // Cap at max
        samples.truncate(self.max_samples);

        Ok(ValidationTestSet { samples })
    }

    fn find_boundary_cases<'a>(
        &self,
        experiences: &'a [PoeExperience],
    ) -> Vec<(String, &'a PoeExperience)> {
        let mut cases = Vec::new();

        // Longest duration
        if let Some(max_dur) = experiences.iter().max_by_key(|e| e.duration_ms) {
            // Only include if it's significantly longer than average
            let avg_dur: u64 =
                experiences.iter().map(|e| e.duration_ms).sum::<u64>() / experiences.len() as u64;
            if max_dur.duration_ms > avg_dur * 2 {
                cases.push(("max_duration".to_string(), max_dur));
            }
        }

        // Most attempts
        if let Some(max_attempts) = experiences.iter().max_by_key(|e| e.attempts) {
            if max_attempts.attempts > 1 {
                cases.push(("max_attempts".to_string(), max_attempts));
            }
        }

        cases
    }
}
```

Update `core/src/skill_evolution/validation/mod.rs`:

```rust
pub mod risk_profiler;
pub mod test_set_generator;

pub use risk_profiler::{SkillRiskLevel, SkillRiskProfile, SkillRiskProfiler};
pub use test_set_generator::{SampleSource, TestSample, TestSetGenerator, ValidationTestSet};
```

**Step 4: Run tests — expect PASS**

Run: `cargo test -p alephcore --lib test_set_generator`

**Step 5: Commit**

```bash
git add core/src/skill_evolution/validation/
git commit -m "evolution: add test set generator with cluster and boundary sampling"
```

---

## Task 7: Structural Linter (L1 Validation)

**Files:**
- Create: `core/src/skill_evolution/validation/structural_linter.rs`
- Modify: `core/src/skill_evolution/validation/mod.rs`
- Test: inline

**Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::crystallization::pattern_model::*;
    use crate::skill_evolution::validation::test_set_generator::*;
    use crate::poe::crystallization::experience_store::PoeExperience;

    fn make_sample(id: &str, tool_seq: &str) -> TestSample {
        TestSample {
            experience: PoeExperience {
                id: id.to_string(),
                task_id: format!("task_{}", id),
                objective: "test".to_string(),
                pattern_id: "p1".to_string(),
                tool_sequence_json: tool_seq.to_string(),
                parameter_mapping: None,
                satisfaction: 0.9,
                distance_score: 0.1,
                attempts: 1,
                duration_ms: 100,
                created_at: 0,
            },
            source: SampleSource::ClusterRepresentative,
        }
    }

    #[test]
    fn test_valid_pattern_passes_lint() {
        let pattern = PatternSequence {
            description: "search and fix".to_string(),
            steps: vec![PatternStep::Action {
                tool_call: ToolCallTemplate {
                    tool_name: "search".to_string(),
                    category: ToolCategory::ReadOnly,
                },
                params: ParameterMapping::default(),
            }],
            expected_outputs: vec![],
        };
        let test_set = ValidationTestSet {
            samples: vec![make_sample("s1", r#"[{"tool":"search"}]"#)],
        };
        let linter = StructuralLinter;
        let result = linter.validate(&pattern, &test_set);
        assert!(result.passed);
    }

    #[test]
    fn test_empty_steps_fails_lint() {
        let pattern = PatternSequence {
            description: "empty".to_string(),
            steps: vec![],
            expected_outputs: vec![],
        };
        let test_set = ValidationTestSet {
            samples: vec![make_sample("s1", "[]")],
        };
        let linter = StructuralLinter;
        let result = linter.validate(&pattern, &test_set);
        assert!(!result.passed);
    }

    #[test]
    fn test_invalid_constraints_fails_lint() {
        // Pattern with max_iterations > 10 should fail
        let pattern = PatternSequence {
            description: "bad loop".to_string(),
            steps: vec![PatternStep::Loop {
                predicate: Predicate::Semantic("x".to_string()),
                body: vec![],
                max_iterations: 15,
            }],
            expected_outputs: vec![],
        };
        let test_set = ValidationTestSet { samples: vec![] };
        let linter = StructuralLinter;
        let result = linter.validate(&pattern, &test_set);
        assert!(!result.passed);
    }
}
```

**Step 2: Run tests — expect failure**

**Step 3: Implement structural_linter.rs**

```rust
// core/src/skill_evolution/validation/structural_linter.rs

use crate::poe::crystallization::pattern_model::PatternSequence;
use super::test_set_generator::ValidationTestSet;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LintResult {
    pub passed: bool,
    pub errors: Vec<String>,
}

pub struct StructuralLinter;

impl StructuralLinter {
    /// L1 validation: structural checks only, must complete in < 100ms
    pub fn validate(&self, pattern: &PatternSequence, _test_set: &ValidationTestSet) -> LintResult {
        let mut errors = Vec::new();

        // Check 1: Pattern must have at least one step
        if pattern.steps.is_empty() {
            errors.push("pattern has no steps".to_string());
        }

        // Check 2: Pattern constraint validation (max_iterations, nesting depth, etc.)
        let constraint_errors = pattern.validate();
        errors.extend(constraint_errors);

        // Check 3: Description must be non-empty
        if pattern.description.trim().is_empty() {
            errors.push("pattern description is empty".to_string());
        }

        LintResult {
            passed: errors.is_empty(),
            errors,
        }
    }
}
```

Update `validation/mod.rs` to add `pub mod structural_linter;` and re-export.

**Step 4: Run tests — expect PASS**

Run: `cargo test -p alephcore --lib structural_linter`

**Step 5: Commit**

```bash
git add core/src/skill_evolution/validation/
git commit -m "evolution: add L1 structural linter for pattern validation"
```

---

## Task 8: Semantic Replayer (L2 Validation)

**Files:**
- Create: `core/src/skill_evolution/validation/semantic_replayer.rs`
- Modify: `core/src/skill_evolution/validation/mod.rs`
- Test: inline

**Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::crystallization::synthesis_backend::*;
    use crate::poe::crystallization::pattern_model::*;
    use crate::skill_evolution::validation::test_set_generator::*;
    use crate::poe::crystallization::experience_store::PoeExperience;

    struct HighConfidenceBackend;

    #[async_trait::async_trait]
    impl PatternSynthesisBackend for HighConfidenceBackend {
        async fn synthesize_pattern(&self, _: PatternSynthesisRequest) -> anyhow::Result<PatternSuggestion> {
            unreachable!()
        }
        async fn evaluate_confidence(&self, _: &str, _: &[PoeExperience]) -> anyhow::Result<f32> {
            Ok(0.95)  // high confidence = similar
        }
    }

    struct LowConfidenceBackend;

    #[async_trait::async_trait]
    impl PatternSynthesisBackend for LowConfidenceBackend {
        async fn synthesize_pattern(&self, _: PatternSynthesisRequest) -> anyhow::Result<PatternSuggestion> {
            unreachable!()
        }
        async fn evaluate_confidence(&self, _: &str, _: &[PoeExperience]) -> anyhow::Result<f32> {
            Ok(0.3)  // low confidence = divergent
        }
    }

    fn make_sample(id: &str) -> TestSample {
        TestSample {
            experience: PoeExperience {
                id: id.to_string(),
                task_id: "t1".to_string(),
                objective: "test".to_string(),
                pattern_id: "p1".to_string(),
                tool_sequence_json: "[]".to_string(),
                parameter_mapping: None,
                satisfaction: 0.9,
                distance_score: 0.1,
                attempts: 1,
                duration_ms: 100,
                created_at: 0,
            },
            source: SampleSource::ClusterRepresentative,
        }
    }

    #[tokio::test]
    async fn test_high_confidence_passes() {
        let replayer = SemanticReplayer::new(Arc::new(HighConfidenceBackend), 0.8);
        let pattern = PatternSequence {
            description: "test".to_string(),
            steps: vec![],
            expected_outputs: vec![],
        };
        let test_set = ValidationTestSet {
            samples: vec![make_sample("s1"), make_sample("s2")],
        };
        let result = replayer.replay(&pattern, &test_set).await.unwrap();
        assert!(result.passed);
    }

    #[tokio::test]
    async fn test_low_confidence_fails() {
        let replayer = SemanticReplayer::new(Arc::new(LowConfidenceBackend), 0.8);
        let pattern = PatternSequence {
            description: "test".to_string(),
            steps: vec![],
            expected_outputs: vec![],
        };
        let test_set = ValidationTestSet {
            samples: vec![make_sample("s1"), make_sample("s2")],
        };
        let result = replayer.replay(&pattern, &test_set).await.unwrap();
        assert!(!result.passed);
    }
}
```

**Step 2: Run tests — expect failure**

**Step 3: Implement semantic_replayer.rs**

```rust
// core/src/skill_evolution/validation/semantic_replayer.rs

use crate::poe::crystallization::pattern_model::PatternSequence;
use crate::poe::crystallization::synthesis_backend::PatternSynthesisBackend;
use super::test_set_generator::ValidationTestSet;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayResult {
    pub passed: bool,
    pub avg_similarity: f64,
    pub passed_count: usize,
    pub total_count: usize,
    pub details: String,
}

pub struct SemanticReplayer {
    backend: Arc<dyn PatternSynthesisBackend>,
    similarity_threshold: f64,
    pass_rate_threshold: f32,
}

impl SemanticReplayer {
    pub fn new(backend: Arc<dyn PatternSynthesisBackend>, similarity_threshold: f64) -> Self {
        Self {
            backend,
            similarity_threshold,
            pass_rate_threshold: 0.8,
        }
    }

    /// L2 validation: semantic replay against test set samples
    pub async fn replay(
        &self,
        pattern: &PatternSequence,
        test_set: &ValidationTestSet,
    ) -> anyhow::Result<ReplayResult> {
        if test_set.samples.is_empty() {
            return Ok(ReplayResult {
                passed: true,
                avg_similarity: 1.0,
                passed_count: 0,
                total_count: 0,
                details: "no samples to replay".to_string(),
            });
        }

        let mut similarities = Vec::new();
        let mut passed_count = 0;

        for sample in &test_set.samples {
            let confidence = self
                .backend
                .evaluate_confidence(
                    &pattern.description,
                    &[sample.experience.clone()],
                )
                .await?;

            let sim = confidence as f64;
            if sim >= self.similarity_threshold {
                passed_count += 1;
            }
            similarities.push(sim);
        }

        let avg_similarity =
            similarities.iter().sum::<f64>() / similarities.len().max(1) as f64;
        let pass_rate = passed_count as f32 / test_set.samples.len() as f32;
        let passed = pass_rate >= self.pass_rate_threshold;

        Ok(ReplayResult {
            passed,
            avg_similarity,
            passed_count,
            total_count: test_set.samples.len(),
            details: format!(
                "{}/{} samples passed (avg similarity: {:.2}, threshold: {:.2})",
                passed_count,
                test_set.samples.len(),
                avg_similarity,
                self.similarity_threshold,
            ),
        })
    }
}
```

Update `validation/mod.rs`.

**Step 4: Run tests — expect PASS**

Run: `cargo test -p alephcore --lib semantic_replayer`

**Step 5: Commit**

```bash
git add core/src/skill_evolution/validation/
git commit -m "evolution: add L2 semantic replayer for pattern validation"
```

---

## Task 9: Differential Testing Engine

**Files:**
- Create: `core/src/skill_evolution/differential.rs`
- Modify: `core/src/skill_evolution/mod.rs`
- Test: inline

**Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::crystallization::pattern_model::*;

    #[test]
    fn test_more_efficient_skill_passes() {
        let diff = EfficiencyDiff::compute(
            2.0,   // skill steps
            100.0, // skill tokens
            5.0,   // baseline steps
            500.0, // baseline tokens
            0.1,   // tolerance
        );
        assert!(diff.is_more_efficient);
    }

    #[test]
    fn test_less_efficient_skill_fails() {
        let diff = EfficiencyDiff::compute(
            10.0,  // skill steps (worse)
            1000.0, // skill tokens (worse)
            5.0,   // baseline steps
            500.0, // baseline tokens
            0.1,
        );
        assert!(!diff.is_more_efficient);
    }

    #[test]
    fn test_within_tolerance_passes() {
        // 10% more steps but within tolerance
        let diff = EfficiencyDiff::compute(
            5.5,   // 10% more than baseline
            500.0,
            5.0,
            500.0,
            0.1,
        );
        assert!(diff.is_more_efficient);
    }

    #[test]
    fn test_estimate_pattern_cost() {
        let seq = PatternSequence {
            description: "test".to_string(),
            steps: vec![
                PatternStep::Action {
                    tool_call: ToolCallTemplate {
                        tool_name: "a".to_string(),
                        category: ToolCategory::ReadOnly,
                    },
                    params: ParameterMapping::default(),
                },
                PatternStep::Action {
                    tool_call: ToolCallTemplate {
                        tool_name: "b".to_string(),
                        category: ToolCategory::ReadOnly,
                    },
                    params: ParameterMapping::default(),
                },
            ],
            expected_outputs: vec![],
        };
        assert!((seq.estimated_total_cost() - 2.0).abs() < f32::EPSILON);
    }
}
```

**Step 2: Run tests — expect failure**

**Step 3: Implement differential.rs**

```rust
// core/src/skill_evolution/differential.rs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EfficiencyDiff {
    pub skill_avg_steps: f32,
    pub baseline_avg_steps: f32,
    pub skill_avg_tokens: f32,
    pub baseline_avg_tokens: f32,
    pub is_more_efficient: bool,
}

impl EfficiencyDiff {
    /// Compare skill efficiency against baseline with tolerance
    pub fn compute(
        skill_steps: f32,
        skill_tokens: f32,
        baseline_steps: f32,
        baseline_tokens: f32,
        tolerance: f32,
    ) -> Self {
        let step_ok = skill_steps <= baseline_steps * (1.0 + tolerance);
        let token_ok = skill_tokens <= baseline_tokens * (1.0 + tolerance);

        EfficiencyDiff {
            skill_avg_steps: skill_steps,
            baseline_avg_steps: baseline_steps,
            skill_avg_tokens: skill_tokens,
            baseline_avg_tokens: baseline_tokens,
            is_more_efficient: step_ok && token_ok,
        }
    }
}
```

Add `pub mod differential;` to `core/src/skill_evolution/mod.rs`.

**Step 4: Run tests — expect PASS**

Run: `cargo test -p alephcore --lib differential`

**Step 5: Commit**

```bash
git add core/src/skill_evolution/differential.rs core/src/skill_evolution/mod.rs
git commit -m "evolution: add differential testing engine for efficiency comparison"
```

---

## Task 10: Skill Lifecycle State Machine

**Files:**
- Create: `core/src/skill_evolution/lifecycle.rs`
- Modify: `core/src/skill_evolution/mod.rs`
- Test: inline

**Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_promotion_eligible() {
        let thresholds = PromotionThresholds::default();
        let state = ShadowState {
            deployed_at: 0,
            invocation_count: 10,
            success_count: 9,
        };
        let days_in_shadow = 3;
        assert!(thresholds.is_eligible(&state, days_in_shadow));
    }

    #[test]
    fn test_promotion_not_enough_invocations() {
        let thresholds = PromotionThresholds::default();
        let state = ShadowState {
            deployed_at: 0,
            invocation_count: 2,
            success_count: 2,
        };
        assert!(!thresholds.is_eligible(&state, 3));
    }

    #[test]
    fn test_promotion_too_low_success_rate() {
        let thresholds = PromotionThresholds::default();
        let state = ShadowState {
            deployed_at: 0,
            invocation_count: 10,
            success_count: 5,
        };
        assert!(!thresholds.is_eligible(&state, 3));
    }

    #[test]
    fn test_demotion_consecutive_failures() {
        let triggers = DemotionTriggers::default();
        assert!(triggers.should_demote(3, 0.8));  // 3 consecutive failures
    }

    #[test]
    fn test_demotion_low_success_rate() {
        let triggers = DemotionTriggers::default();
        assert!(triggers.should_demote(0, 0.4));  // below 0.5 floor
    }

    #[test]
    fn test_lifecycle_state_transitions() {
        let state = SkillLifecycleState::Draft;
        assert!(matches!(state, SkillLifecycleState::Draft));

        let shadow = SkillLifecycleState::Shadow(ShadowState {
            deployed_at: 1000,
            invocation_count: 0,
            success_count: 0,
        });
        assert!(matches!(shadow, SkillLifecycleState::Shadow(_)));
    }
}
```

**Step 2: Run tests — expect failure**

**Step 3: Implement lifecycle.rs**

```rust
// core/src/skill_evolution/lifecycle.rs

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum SkillLifecycleState {
    Draft,
    Shadow(ShadowState),
    Promoted { promoted_at: i64, shadow_duration_days: u32 },
    Demoted { reason: String, demoted_at: i64 },
    Retired { reason: String, retired_at: i64 },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ShadowState {
    pub deployed_at: i64,
    pub invocation_count: u32,
    pub success_count: u32,
}

impl ShadowState {
    pub fn success_rate(&self) -> f32 {
        if self.invocation_count == 0 {
            return 0.0;
        }
        self.success_count as f32 / self.invocation_count as f32
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PromotionThresholds {
    pub min_invocations: u32,
    pub min_success_rate: f32,
    pub min_shadow_days: u32,
    pub must_beat_baseline: bool,
}

impl Default for PromotionThresholds {
    fn default() -> Self {
        Self {
            min_invocations: 5,
            min_success_rate: 0.85,
            min_shadow_days: 2,
            must_beat_baseline: true,
        }
    }
}

impl PromotionThresholds {
    pub fn is_eligible(&self, state: &ShadowState, days_in_shadow: u32) -> bool {
        state.invocation_count >= self.min_invocations
            && state.success_rate() >= self.min_success_rate
            && days_in_shadow >= self.min_shadow_days
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DemotionTriggers {
    pub consecutive_failures: u32,
    pub success_rate_floor: f32,
    pub max_shadow_days: u32,
}

impl Default for DemotionTriggers {
    fn default() -> Self {
        Self {
            consecutive_failures: 3,
            success_rate_floor: 0.5,
            max_shadow_days: 30,
        }
    }
}

impl DemotionTriggers {
    pub fn should_demote(&self, consecutive_fails: u32, success_rate: f32) -> bool {
        consecutive_fails >= self.consecutive_failures
            || success_rate < self.success_rate_floor
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetadata {
    pub skill_id: String,
    pub lifecycle: SkillLifecycleState,
    pub origin: SkillOrigin,
    pub risk_level: String,
    pub validation_history: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillOrigin {
    pub pattern_id: String,
    pub source_experiences: Vec<String>,
    pub generator_version: String,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub enum LifecycleTransition {
    Unchanged,
    Promoted,
    Demoted { reason: String },
    Retired { reason: String },
}
```

Add `pub mod lifecycle;` to `core/src/skill_evolution/mod.rs`.

**Step 4: Run tests — expect PASS**

Run: `cargo test -p alephcore --lib lifecycle`

**Step 5: Commit**

```bash
git add core/src/skill_evolution/lifecycle.rs core/src/skill_evolution/mod.rs
git commit -m "evolution: add skill lifecycle state machine with promotion/demotion"
```

---

## Task 11: Shadow Deployer

**Files:**
- Create: `core/src/skill_evolution/shadow_deployer.rs`
- Modify: `core/src/skill_evolution/mod.rs`
- Test: inline (using tempdir)

**Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dirs() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let evolved = dir.path().join("evolved_skills");
        let official = dir.path().join("skills");
        std::fs::create_dir_all(&evolved).unwrap();
        std::fs::create_dir_all(&official).unwrap();
        (dir, evolved, official)
    }

    #[tokio::test]
    async fn test_deploy_creates_files() {
        let (_dir, evolved, official) = temp_dirs();
        let deployer = ShadowDeployer::new(evolved.clone(), official);

        let result = deployer
            .deploy("skill_search", "# Search Skill\n...", "pattern_a")
            .await
            .unwrap();

        assert!(result.skill_path.exists());
        assert!(result.meta_path.exists());

        let content = std::fs::read_to_string(&result.skill_path).unwrap();
        assert!(content.contains("Search Skill"));

        let meta: SkillMetadata =
            serde_json::from_str(&std::fs::read_to_string(&result.meta_path).unwrap()).unwrap();
        assert!(matches!(meta.lifecycle, SkillLifecycleState::Shadow(_)));
    }

    #[tokio::test]
    async fn test_promote_moves_to_official() {
        let (_dir, evolved, official) = temp_dirs();
        let deployer = ShadowDeployer::new(evolved.clone(), official.clone());

        deployer
            .deploy("skill_x", "# Skill X", "p1")
            .await
            .unwrap();

        let transition = deployer.promote("skill_x").await.unwrap();
        assert!(matches!(transition, LifecycleTransition::Promoted));

        // Should exist in official, not in evolved
        assert!(official.join("skill_x").join("SKILL.md").exists());
        assert!(!evolved.join("skill_x").exists());
    }

    #[tokio::test]
    async fn test_demote_removes_from_evolved() {
        let (_dir, evolved, official) = temp_dirs();
        let deployer = ShadowDeployer::new(evolved.clone(), official);

        deployer
            .deploy("skill_bad", "# Bad Skill", "p2")
            .await
            .unwrap();

        let transition = deployer.demote("skill_bad", "test failure").await.unwrap();
        assert!(matches!(transition, LifecycleTransition::Demoted { .. }));
        assert!(!evolved.join("skill_bad").exists());
    }
}
```

**Step 2: Run tests — expect failure**

**Step 3: Implement shadow_deployer.rs**

```rust
// core/src/skill_evolution/shadow_deployer.rs

use super::lifecycle::*;
use std::path::{Path, PathBuf};
use anyhow::Result;

pub struct ShadowDeployment {
    pub skill_path: PathBuf,
    pub meta_path: PathBuf,
    pub skill_id: String,
}

pub struct ShadowDeployer {
    evolved_dir: PathBuf,
    official_dir: PathBuf,
}

impl ShadowDeployer {
    pub fn new(evolved_dir: PathBuf, official_dir: PathBuf) -> Self {
        Self {
            evolved_dir,
            official_dir,
        }
    }

    pub async fn deploy(
        &self,
        skill_id: &str,
        skill_content: &str,
        pattern_id: &str,
    ) -> Result<ShadowDeployment> {
        let skill_dir = self.evolved_dir.join(skill_id);
        tokio::fs::create_dir_all(&skill_dir).await?;

        let skill_path = skill_dir.join("SKILL.md");
        tokio::fs::write(&skill_path, skill_content).await?;

        let meta = SkillMetadata {
            skill_id: skill_id.to_string(),
            lifecycle: SkillLifecycleState::Shadow(ShadowState {
                deployed_at: now_ms(),
                invocation_count: 0,
                success_count: 0,
            }),
            origin: SkillOrigin {
                pattern_id: pattern_id.to_string(),
                source_experiences: vec![],
                generator_version: env!("CARGO_PKG_VERSION").to_string(),
                created_at: now_ms(),
            },
            risk_level: "unknown".to_string(),
            validation_history: vec![],
        };

        let meta_path = skill_dir.join("metadata.json");
        let meta_json = serde_json::to_string_pretty(&meta)?;
        tokio::fs::write(&meta_path, meta_json).await?;

        Ok(ShadowDeployment {
            skill_path,
            meta_path,
            skill_id: skill_id.to_string(),
        })
    }

    pub async fn promote(&self, skill_id: &str) -> Result<LifecycleTransition> {
        let src = self.evolved_dir.join(skill_id);
        let dst = self.official_dir.join(skill_id);

        if !src.exists() {
            anyhow::bail!("shadow skill '{}' not found", skill_id);
        }

        tokio::fs::rename(&src, &dst).await?;

        // Update metadata
        let meta_path = dst.join("metadata.json");
        if meta_path.exists() {
            if let Ok(content) = tokio::fs::read_to_string(&meta_path).await {
                if let Ok(mut meta) = serde_json::from_str::<SkillMetadata>(&content) {
                    meta.lifecycle = SkillLifecycleState::Promoted {
                        promoted_at: now_ms(),
                        shadow_duration_days: 0, // could compute from deployed_at
                    };
                    let _ = tokio::fs::write(
                        &meta_path,
                        serde_json::to_string_pretty(&meta).unwrap_or_default(),
                    )
                    .await;
                }
            }
        }

        Ok(LifecycleTransition::Promoted)
    }

    pub async fn demote(&self, skill_id: &str, reason: &str) -> Result<LifecycleTransition> {
        let path = self.evolved_dir.join(skill_id);

        if !path.exists() {
            anyhow::bail!("shadow skill '{}' not found", skill_id);
        }

        // Archive metadata before deletion
        let meta_path = path.join("metadata.json");
        if meta_path.exists() {
            let content = tokio::fs::read_to_string(&meta_path).await.unwrap_or_default();
            tracing::info!(
                skill_id = skill_id,
                reason = reason,
                metadata = content.as_str(),
                "archiving demoted skill metadata"
            );
        }

        tokio::fs::remove_dir_all(&path).await?;

        Ok(LifecycleTransition::Demoted {
            reason: reason.to_string(),
        })
    }

    pub fn evolved_dir(&self) -> &Path {
        &self.evolved_dir
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
```

Add `pub mod shadow_deployer;` to `core/src/skill_evolution/mod.rs`.

**Step 4: Run tests — expect PASS**

Run: `cargo test -p alephcore --lib shadow_deployer`

**Step 5: Commit**

```bash
git add core/src/skill_evolution/shadow_deployer.rs core/src/skill_evolution/mod.rs
git commit -m "evolution: add shadow deployer with promote/demote lifecycle"
```

---

## Task 12: Idle Detector

**Files:**
- Create: `core/src/poe/crystallization/idle_detector.rs`
- Modify: `core/src/poe/crystallization/mod.rs`
- Test: inline

**Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initially_idle() {
        let detector = IdleDetector::new(IdleConfig {
            min_idle_seconds: 300,
        });
        // Just created with timestamp 0 — should be idle
        detector.last_activity.store(0, Ordering::Relaxed);
        assert!(detector.is_idle());
    }

    #[test]
    fn test_activity_resets_idle() {
        let detector = IdleDetector::new(IdleConfig {
            min_idle_seconds: 300,
        });
        detector.record_activity();
        assert!(!detector.is_idle());
    }

    #[test]
    fn test_idle_after_timeout() {
        let detector = IdleDetector::new(IdleConfig {
            min_idle_seconds: 1, // 1 second for test
        });
        // Set last activity to 2 seconds ago
        let two_sec_ago = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            - 2000;
        detector.last_activity.store(two_sec_ago, Ordering::Relaxed);
        assert!(detector.is_idle());
    }
}
```

**Step 2: Run tests — expect failure**

**Step 3: Implement idle_detector.rs**

```rust
// core/src/poe/crystallization/idle_detector.rs

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct IdleConfig {
    pub min_idle_seconds: u64,
}

impl Default for IdleConfig {
    fn default() -> Self {
        Self {
            min_idle_seconds: 300,
        }
    }
}

#[derive(Clone)]
pub struct IdleDetector {
    pub(crate) last_activity: Arc<AtomicU64>,
    config: IdleConfig,
}

impl IdleDetector {
    pub fn new(config: IdleConfig) -> Self {
        Self {
            last_activity: Arc::new(AtomicU64::new(Self::now_ms())),
            config,
        }
    }

    pub fn record_activity(&self) {
        self.last_activity.store(Self::now_ms(), Ordering::Relaxed);
    }

    pub fn is_idle(&self) -> bool {
        let last = self.last_activity.load(Ordering::Relaxed);
        let elapsed_ms = Self::now_ms().saturating_sub(last);
        elapsed_ms > self.config.min_idle_seconds * 1000
    }

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}
```

Add `pub mod idle_detector;` to `core/src/poe/crystallization/mod.rs`.

**Step 4: Run tests — expect PASS**

Run: `cargo test -p alephcore --lib idle_detector`

**Step 5: Commit**

```bash
git add core/src/poe/crystallization/idle_detector.rs core/src/poe/crystallization/mod.rs
git commit -m "poe: add idle detector for dreaming trigger"
```

---

## Task 13: Cognitive Entropy Tracker

**Files:**
- Create: `core/src/poe/crystallization/cognitive_entropy.rs`
- Modify: `core/src/poe/crystallization/mod.rs`
- Test: inline

**Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_trend_increasing() {
        // First half low, second half high → Increasing
        let values = vec![0.2, 0.3, 0.25, 0.7, 0.8, 0.75];
        let trend = CognitiveEntropyTracker::compute_trend(&values);
        assert_eq!(trend, EntropyTrend::Increasing);
    }

    #[test]
    fn test_compute_trend_decreasing() {
        let values = vec![0.8, 0.7, 0.75, 0.2, 0.3, 0.25];
        let trend = CognitiveEntropyTracker::compute_trend(&values);
        assert_eq!(trend, EntropyTrend::Decreasing);
    }

    #[test]
    fn test_compute_trend_stable() {
        let values = vec![0.5, 0.52, 0.48, 0.51, 0.49, 0.50];
        let trend = CognitiveEntropyTracker::compute_trend(&values);
        assert_eq!(trend, EntropyTrend::Stable);
    }

    #[test]
    fn test_compute_trend_volatile_too_few() {
        let values = vec![0.5, 0.6];
        let trend = CognitiveEntropyTracker::compute_trend(&values);
        assert_eq!(trend, EntropyTrend::Volatile);
    }

    #[test]
    fn test_entropy_trend_priority_ordering() {
        assert!(EntropyTrend::Increasing.priority() < EntropyTrend::Volatile.priority());
        assert!(EntropyTrend::Volatile.priority() < EntropyTrend::Stable.priority());
        assert!(EntropyTrend::Stable.priority() < EntropyTrend::Decreasing.priority());
    }
}
```

**Step 2: Run tests — expect failure**

**Step 3: Implement cognitive_entropy.rs**

```rust
// core/src/poe/crystallization/cognitive_entropy.rs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EntropyTrend {
    Increasing,  // degrading — highest priority
    Volatile,    // unstable — needs data
    Stable,      // converged — no action
    Decreasing,  // improving — skip
}

impl EntropyTrend {
    /// Lower value = higher priority for distillation
    pub fn priority(&self) -> u8 {
        match self {
            EntropyTrend::Increasing => 0,
            EntropyTrend::Volatile => 1,
            EntropyTrend::Stable => 2,
            EntropyTrend::Decreasing => 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntropyReport {
    pub pattern_id: String,
    pub recent_entropy: f32,
    pub entropy_trend: EntropyTrend,
    pub execution_count: u32,
}

pub struct CognitiveEntropyTracker;

impl CognitiveEntropyTracker {
    /// Compute trend from a series of distance scores (entropy values)
    pub fn compute_trend(values: &[f32]) -> EntropyTrend {
        if values.len() < 3 {
            return EntropyTrend::Volatile;
        }

        let mid = values.len() / 2;
        let first_half_avg: f32 = values[..mid].iter().sum::<f32>() / mid as f32;
        let second_half_avg: f32 =
            values[mid..].iter().sum::<f32>() / (values.len() - mid) as f32;
        let diff = second_half_avg - first_half_avg;

        if diff > 0.1 {
            EntropyTrend::Increasing
        } else if diff < -0.1 {
            EntropyTrend::Decreasing
        } else {
            EntropyTrend::Stable
        }
    }

    /// Build entropy reports from execution data.
    /// `executions` is a map of pattern_id → Vec<(satisfaction, distance_score)>
    pub fn analyze(
        executions: &std::collections::HashMap<String, Vec<(f32, f32)>>,
        entropy_threshold: f32,
    ) -> Vec<EntropyReport> {
        let mut reports = Vec::new();

        for (pattern_id, data) in executions {
            if data.len() < 3 {
                continue;
            }

            let distances: Vec<f32> = data.iter().map(|(_, d)| *d).collect();
            let avg_entropy = distances.iter().sum::<f32>() / distances.len() as f32;
            let trend = Self::compute_trend(&distances);

            if avg_entropy >= entropy_threshold || trend == EntropyTrend::Increasing {
                reports.push(EntropyReport {
                    pattern_id: pattern_id.clone(),
                    recent_entropy: avg_entropy,
                    entropy_trend: trend,
                    execution_count: data.len() as u32,
                });
            }
        }

        // Sort by priority (Increasing first), then by entropy descending
        reports.sort_by(|a, b| {
            a.entropy_trend
                .priority()
                .cmp(&b.entropy_trend.priority())
                .then(
                    b.recent_entropy
                        .partial_cmp(&a.recent_entropy)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
        });

        reports
    }
}
```

Add `pub mod cognitive_entropy;` to `core/src/poe/crystallization/mod.rs`.

**Step 4: Run tests — expect PASS**

Run: `cargo test -p alephcore --lib cognitive_entropy`

**Step 5: Commit**

```bash
git add core/src/poe/crystallization/cognitive_entropy.rs core/src/poe/crystallization/mod.rs
git commit -m "poe: add cognitive entropy tracker for priority-driven dreaming"
```

---

## Task 14: Clustering Merge Implementation

**Files:**
- Modify: `core/src/poe/crystallization/clustering.rs` (add `merge_clusters` method)
- Test: extend existing tests

**Step 1: Write failing tests**

```rust
// Add to clustering.rs tests

#[tokio::test]
async fn test_merge_clusters_deletes_redundant() {
    let store = InMemoryExperienceStore::new();

    // Insert 3 experiences
    for i in 0..3 {
        let exp = PoeExperience {
            id: format!("e{}", i),
            task_id: format!("t{}", i),
            objective: "same task".to_string(),
            pattern_id: "p1".to_string(),
            tool_sequence_json: "[]".to_string(),
            parameter_mapping: None,
            satisfaction: 0.8 + i as f32 * 0.05, // e2 has highest
            distance_score: 0.2 - i as f32 * 0.05,
            attempts: 1,
            duration_ms: 100,
            created_at: 0,
        };
        store.insert(exp, &vec![1.0, 0.0, 0.0]).await.unwrap();
    }

    let cluster = Cluster {
        cluster_id: "c1".to_string(),
        members: vec!["e0".to_string(), "e1".to_string(), "e2".to_string()],
        representative_id: "e2".to_string(), // highest satisfaction
    };

    let report = merge_clusters(&[cluster], &store).await.unwrap();
    assert_eq!(report.entries_deleted, 2);
    assert_eq!(report.entries_kept, 1);
    assert_eq!(store.count().await.unwrap(), 1);
}
```

**Step 2: Run test — expect failure**

**Step 3: Add merge function to clustering.rs**

```rust
/// Report of merge operation
#[derive(Debug)]
pub struct MergeReport {
    pub clusters_merged: usize,
    pub entries_deleted: usize,
    pub entries_kept: usize,
}

/// Merge cluster members into representatives, deleting redundant entries
pub async fn merge_clusters(
    clusters: &[Cluster],
    store: &dyn ExperienceStore,
) -> anyhow::Result<MergeReport> {
    let mut deleted = 0;
    let mut kept = 0;

    for cluster in clusters {
        if cluster.members.len() <= 1 {
            kept += 1;
            continue;
        }

        for member_id in &cluster.members {
            if member_id != &cluster.representative_id {
                store.delete(member_id).await?;
                deleted += 1;
            }
        }
        kept += 1;
    }

    Ok(MergeReport {
        clusters_merged: clusters.len(),
        entries_deleted: deleted,
        entries_kept: kept,
    })
}
```

**Step 4: Run tests — expect PASS**

Run: `cargo test -p alephcore --lib clustering`

**Step 5: Commit**

```bash
git add core/src/poe/crystallization/clustering.rs
git commit -m "poe: implement cluster merge with redundant entry deletion"
```

---

## Task 15: Tiered Validator Orchestrator

**Files:**
- Create: `core/src/skill_evolution/validation/tiered_validator.rs`
- Modify: `core/src/skill_evolution/validation/mod.rs`
- Test: inline

**Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::crystallization::pattern_model::*;
    use crate::poe::crystallization::synthesis_backend::*;
    use crate::poe::crystallization::experience_store::{InMemoryExperienceStore, PoeExperience};
    use crate::skill_evolution::validation::risk_profiler::*;

    struct AlwaysAgreeBackend;

    #[async_trait::async_trait]
    impl PatternSynthesisBackend for AlwaysAgreeBackend {
        async fn synthesize_pattern(&self, _: PatternSynthesisRequest) -> anyhow::Result<PatternSuggestion> {
            unreachable!()
        }
        async fn evaluate_confidence(&self, _: &str, _: &[PoeExperience]) -> anyhow::Result<f32> {
            Ok(0.95)
        }
    }

    fn simple_pattern(category: ToolCategory) -> PatternSequence {
        PatternSequence {
            description: "test pattern".to_string(),
            steps: vec![PatternStep::Action {
                tool_call: ToolCallTemplate {
                    tool_name: "tool".to_string(),
                    category,
                },
                params: ParameterMapping::default(),
            }],
            expected_outputs: vec![],
        }
    }

    #[tokio::test]
    async fn test_low_risk_only_needs_l1() {
        let validator = TieredValidator::new(Arc::new(AlwaysAgreeBackend));
        let pattern = simple_pattern(ToolCategory::ReadOnly);
        let risk = SkillRiskProfiler::profile(&pattern);
        let store = InMemoryExperienceStore::new();

        let verdict = validator.validate(&pattern, "p1", &risk, &store).await.unwrap();
        assert!(verdict.passed);
        assert_eq!(verdict.level_reached, ValidationLevel::L1Structural);
    }

    #[tokio::test]
    async fn test_medium_risk_needs_l2() {
        let validator = TieredValidator::new(Arc::new(AlwaysAgreeBackend));
        let pattern = simple_pattern(ToolCategory::FileWrite);
        let risk = SkillRiskProfiler::profile(&pattern);
        let store = InMemoryExperienceStore::new();

        // Insert a sample so L2 has something to replay
        let exp = PoeExperience {
            id: "e1".to_string(),
            task_id: "t1".to_string(),
            objective: "test".to_string(),
            pattern_id: "p1".to_string(),
            tool_sequence_json: "[]".to_string(),
            parameter_mapping: None,
            satisfaction: 0.9,
            distance_score: 0.1,
            attempts: 1,
            duration_ms: 100,
            created_at: 0,
        };
        store.insert(exp, &vec![1.0, 0.0, 0.0]).await.unwrap();

        let verdict = validator.validate(&pattern, "p1", &risk, &store).await.unwrap();
        assert!(verdict.passed);
        assert_eq!(verdict.level_reached, ValidationLevel::L2Semantic);
    }

    #[tokio::test]
    async fn test_high_risk_flags_human_review() {
        let validator = TieredValidator::new(Arc::new(AlwaysAgreeBackend));
        let pattern = simple_pattern(ToolCategory::Shell);
        let risk = SkillRiskProfiler::profile(&pattern);
        let store = InMemoryExperienceStore::new();

        let verdict = validator.validate(&pattern, "p1", &risk, &store).await.unwrap();
        assert!(verdict.passed);
        assert!(verdict.requires_human_review);
    }

    #[tokio::test]
    async fn test_empty_pattern_fails_l1() {
        let validator = TieredValidator::new(Arc::new(AlwaysAgreeBackend));
        let pattern = PatternSequence {
            description: "empty".to_string(),
            steps: vec![],
            expected_outputs: vec![],
        };
        let risk = SkillRiskProfile {
            level: SkillRiskLevel::Low,
            reasoning: String::new(),
        };
        let store = InMemoryExperienceStore::new();

        let verdict = validator.validate(&pattern, "p1", &risk, &store).await.unwrap();
        assert!(!verdict.passed);
    }
}
```

**Step 2: Run tests — expect failure**

**Step 3: Implement tiered_validator.rs**

```rust
// core/src/skill_evolution/validation/tiered_validator.rs

use crate::poe::crystallization::experience_store::ExperienceStore;
use crate::poe::crystallization::pattern_model::PatternSequence;
use crate::poe::crystallization::synthesis_backend::PatternSynthesisBackend;
use super::risk_profiler::{SkillRiskLevel, SkillRiskProfile};
use super::structural_linter::StructuralLinter;
use super::semantic_replayer::SemanticReplayer;
use super::test_set_generator::TestSetGenerator;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValidationLevel {
    L1Structural,
    L2Semantic,
    L3Sandbox,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationVerdict {
    pub passed: bool,
    pub level_reached: ValidationLevel,
    pub l1_errors: Vec<String>,
    pub l2_details: Option<String>,
    pub requires_human_review: bool,
}

pub struct TieredValidator {
    linter: StructuralLinter,
    replayer: SemanticReplayer,
    test_set_gen: TestSetGenerator,
}

impl TieredValidator {
    pub fn new(backend: Arc<dyn PatternSynthesisBackend>) -> Self {
        Self {
            linter: StructuralLinter,
            replayer: SemanticReplayer::new(backend, 0.8),
            test_set_gen: TestSetGenerator::new(8),
        }
    }

    pub async fn validate(
        &self,
        pattern: &PatternSequence,
        pattern_id: &str,
        risk: &SkillRiskProfile,
        store: &dyn ExperienceStore,
    ) -> anyhow::Result<ValidationVerdict> {
        let test_set = self.test_set_gen.generate(pattern_id, store).await?;

        // L1: Structural Linter (all skills must pass)
        let l1 = self.linter.validate(pattern, &test_set);
        if !l1.passed {
            return Ok(ValidationVerdict {
                passed: false,
                level_reached: ValidationLevel::L1Structural,
                l1_errors: l1.errors,
                l2_details: None,
                requires_human_review: false,
            });
        }

        // Low risk: L1 sufficient
        if risk.level == SkillRiskLevel::Low {
            return Ok(ValidationVerdict {
                passed: true,
                level_reached: ValidationLevel::L1Structural,
                l1_errors: vec![],
                l2_details: None,
                requires_human_review: false,
            });
        }

        // L2: Semantic Replay (medium + high risk)
        let l2 = self.replayer.replay(pattern, &test_set).await?;
        if !l2.passed {
            return Ok(ValidationVerdict {
                passed: false,
                level_reached: ValidationLevel::L2Semantic,
                l1_errors: vec![],
                l2_details: Some(l2.details),
                requires_human_review: false,
            });
        }

        // Medium risk: L1 + L2 sufficient
        if risk.level == SkillRiskLevel::Medium {
            return Ok(ValidationVerdict {
                passed: true,
                level_reached: ValidationLevel::L2Semantic,
                l1_errors: vec![],
                l2_details: Some(l2.details),
                requires_human_review: false,
            });
        }

        // High risk: L1 + L2 pass, but no sandbox → flag human review
        Ok(ValidationVerdict {
            passed: true,
            level_reached: ValidationLevel::L2Semantic,
            l1_errors: vec![],
            l2_details: Some(l2.details),
            requires_human_review: true,
        })
    }
}
```

Update `validation/mod.rs` to add module and re-exports.

**Step 4: Run tests — expect PASS**

Run: `cargo test -p alephcore --lib tiered_validator`

**Step 5: Commit**

```bash
git add core/src/skill_evolution/validation/
git commit -m "evolution: add tiered validator orchestrating L1+L2+risk-gated L3"
```

---

## Task 16: Full Pipeline Integration

**Files:**
- Modify: `core/src/skill_evolution/pipeline.rs` (add `EvolutionPipelineBeta`)
- Test: inline integration test

**Step 1: Write failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Integration test: full pipeline with mock backend
    #[tokio::test]
    async fn test_full_beta_pipeline_rejects_empty_pattern() {
        let backend = Arc::new(StubSynthesisBackend);
        let pipeline = EvolutionPipelineBeta::new(backend);

        let result = pipeline
            .evaluate_candidate(
                "pattern_test",
                &PatternSequence {
                    description: "".to_string(),  // empty description
                    steps: vec![],                  // empty steps
                    expected_outputs: vec![],
                },
                5.0,  // baseline steps
                500.0, // baseline tokens
            )
            .await;

        assert!(matches!(result, BetaEvalResult::ValidationFailed { .. }));
    }

    #[tokio::test]
    async fn test_full_beta_pipeline_rejects_inefficient() {
        let backend = Arc::new(StubSynthesisBackend);
        let pipeline = EvolutionPipelineBeta::new(backend);

        // Pattern with 10 steps but baseline is only 2
        let pattern = PatternSequence {
            description: "expensive pattern".to_string(),
            steps: (0..10)
                .map(|i| PatternStep::Action {
                    tool_call: ToolCallTemplate {
                        tool_name: format!("tool_{}", i),
                        category: ToolCategory::ReadOnly,
                    },
                    params: ParameterMapping::default(),
                })
                .collect(),
            expected_outputs: vec![],
        };

        let result = pipeline
            .evaluate_candidate("p1", &pattern, 2.0, 100.0)
            .await;

        assert!(matches!(result, BetaEvalResult::Rejected { .. }));
    }
}
```

**Step 2: Run tests — expect failure**

**Step 3: Implement EvolutionPipelineBeta in pipeline.rs**

```rust
// Add to pipeline.rs (or create a new file if pipeline.rs is too large)

use super::validation::tiered_validator::{TieredValidator, ValidationVerdict};
use super::validation::risk_profiler::SkillRiskProfiler;
use super::differential::EfficiencyDiff;
use crate::poe::crystallization::pattern_model::PatternSequence;
use crate::poe::crystallization::synthesis_backend::PatternSynthesisBackend;
use crate::poe::crystallization::experience_store::InMemoryExperienceStore;

pub enum BetaEvalResult {
    Passed {
        verdict: ValidationVerdict,
        diff: EfficiencyDiff,
    },
    Rejected {
        reason: String,
    },
    ValidationFailed {
        verdict: ValidationVerdict,
    },
}

pub struct EvolutionPipelineBeta {
    validator: TieredValidator,
}

impl EvolutionPipelineBeta {
    pub fn new(backend: Arc<dyn PatternSynthesisBackend>) -> Self {
        Self {
            validator: TieredValidator::new(backend),
        }
    }

    /// Evaluate a candidate pattern through differential testing + tiered validation
    pub async fn evaluate_candidate(
        &self,
        pattern_id: &str,
        pattern: &PatternSequence,
        baseline_steps: f32,
        baseline_tokens: f32,
    ) -> BetaEvalResult {
        // Phase 1: Differential Testing
        let skill_steps = pattern.estimated_total_cost();
        let skill_tokens = skill_steps * 100.0; // rough estimate
        let diff = EfficiencyDiff::compute(
            skill_steps,
            skill_tokens,
            baseline_steps,
            baseline_tokens,
            0.1,
        );

        if !diff.is_more_efficient {
            return BetaEvalResult::Rejected {
                reason: format!(
                    "skill ({:.1} steps) not more efficient than baseline ({:.1} steps)",
                    diff.skill_avg_steps, diff.baseline_avg_steps,
                ),
            };
        }

        // Phase 2: Risk Profiling + Tiered Validation
        let risk = SkillRiskProfiler::profile(pattern);
        let store = InMemoryExperienceStore::new(); // placeholder — real pipeline wires actual store

        match self.validator.validate(pattern, pattern_id, &risk, &store).await {
            Ok(verdict) if verdict.passed => BetaEvalResult::Passed { verdict, diff },
            Ok(verdict) => BetaEvalResult::ValidationFailed { verdict },
            Err(e) => BetaEvalResult::Rejected {
                reason: format!("validation error: {}", e),
            },
        }
    }
}
```

**Step 4: Run tests — expect PASS**

Run: `cargo test -p alephcore --lib pipeline`

**Step 5: Run full compile check**

Run: `cargo check -p alephcore`

**Step 6: Commit**

```bash
git add core/src/skill_evolution/pipeline.rs
git commit -m "evolution: integrate beta pipeline with differential testing and tiered validation"
```

---

## Task 17: Final Integration — Compile + Test All

**Step 1: Run full compile**

Run: `cargo check -p alephcore`
Expected: No errors.

**Step 2: Run all new tests**

Run: `cargo test -p alephcore --lib -- pattern_model synthesis_backend risk_profiler test_set_generator structural_linter semantic_replayer differential lifecycle shadow_deployer idle_detector cognitive_entropy clustering tiered_validator pipeline --nocapture`
Expected: All tests PASS.

**Step 3: Run existing tests to verify no regressions**

Run: `cargo test -p alephcore --lib`
Expected: No new failures (pre-existing `markdown_skill::loader` failures are known and acceptable per MEMORY.md).

**Step 4: Run clippy**

Run: `just clippy` or `cargo clippy -p alephcore -- -D warnings`
Expected: No new warnings.

**Step 5: Final commit if any fixups needed**

```bash
git add -A
git commit -m "evolution: fix clippy warnings and finalize beta integration"
```

---

## Summary of New Files

| File | Component | Lines (est.) |
|------|-----------|-------------|
| `core/src/poe/crystallization/pattern_model.rs` | Enhanced Sequence Model | ~250 |
| `core/src/poe/crystallization/synthesis_backend.rs` | Backend Trait | ~60 |
| `core/src/poe/crystallization/idle_detector.rs` | Idle Detection | ~50 |
| `core/src/poe/crystallization/cognitive_entropy.rs` | Entropy Tracking | ~100 |
| `core/src/skill_evolution/validation/mod.rs` | Validation Module | ~15 |
| `core/src/skill_evolution/validation/risk_profiler.rs` | Risk Classification | ~100 |
| `core/src/skill_evolution/validation/test_set_generator.rs` | Test Set Sampling | ~120 |
| `core/src/skill_evolution/validation/structural_linter.rs` | L1 Linter | ~40 |
| `core/src/skill_evolution/validation/semantic_replayer.rs` | L2 Replayer | ~80 |
| `core/src/skill_evolution/validation/tiered_validator.rs` | Orchestrator | ~100 |
| `core/src/skill_evolution/differential.rs` | Efficiency Comparison | ~50 |
| `core/src/skill_evolution/lifecycle.rs` | State Machine | ~120 |
| `core/src/skill_evolution/shadow_deployer.rs` | Shadow Deployment | ~120 |

## Modified Files

| File | Change |
|------|--------|
| `core/src/poe/crystallization/mod.rs` | Add 4 module declarations |
| `core/src/poe/crystallization/pattern_extractor.rs` | Inject backend, add `with_backend` |
| `core/src/poe/crystallization/experience_store.rs` | Add `delete` + `get_by_ids` to trait |
| `core/src/poe/crystallization/clustering.rs` | Add `merge_clusters` function |
| `core/src/skill_evolution/mod.rs` | Add 3 module declarations |
| `core/src/skill_evolution/pipeline.rs` | Add `EvolutionPipelineBeta` |
