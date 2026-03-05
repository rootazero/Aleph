# Task Routing Decision Layer — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Insert a TaskRouter between Gateway and execution that routes tasks to Agent Loop, Dispatcher DAG, POE Full, or Swarm based on complexity.

**Architecture:** Two-phase classification (rules + LLM fallback) at entry, dynamic escalation guard inside Agent Loop. CompositeRouter implements TaskRouter trait. ExecutionEngine dispatches based on TaskRoute.

**Tech Stack:** Rust, `tracing`, `serde`, `schemars`, `async-trait`, `regex`

**Design doc:** `docs/plans/2026-03-05-task-routing-decision-layer-design.md`

---

### Task 1: Core Types — TaskRoute, Contexts, Snapshot

**Files:**
- Create: `core/src/routing/task_router.rs`
- Modify: `core/src/routing/mod.rs`

**Step 1: Create task_router.rs with core types**

```rust
//! Task routing decision layer — core types and trait.
//!
//! Routes incoming tasks to the appropriate execution path:
//! Agent Loop (simple), Dispatcher DAG (multi-step), POE Full (critical),
//! or Swarm (collaborative).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Routing decision for a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskRoute {
    /// Simple task: direct Agent Loop OTAF
    Simple,

    /// Multi-step task: Dispatcher decomposes into DAG
    MultiStep { reason: String },

    /// Critical task: Dispatcher DAG wrapped by POE Full Manager
    Critical {
        reason: String,
        manifest_hints: ManifestHints,
    },

    /// Collaborative task: Swarm multi-agent execution
    Collaborative {
        reason: String,
        strategy: CollabStrategy,
    },
}

impl TaskRoute {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Simple => "simple",
            Self::MultiStep { .. } => "multi_step",
            Self::Critical { .. } => "critical",
            Self::Collaborative { .. } => "collaborative",
        }
    }
}

/// Hints for constructing a SuccessManifest in the Critical path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestHints {
    pub hard_constraints: Vec<String>,
    pub quality_threshold: f64,
}

impl Default for ManifestHints {
    fn default() -> Self {
        Self {
            hard_constraints: Vec::new(),
            quality_threshold: 0.7,
        }
    }
}

/// Strategy for collaborative (Swarm) execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CollabStrategy {
    /// Multi-domain parallel execution
    Parallel,
    /// Adversarial verification (generator + reviewer)
    Adversarial,
    /// User-requested multi-persona group chat
    GroupChat,
}

/// Context available at pre-classification time.
pub struct RouterContext {
    pub session_history_len: usize,
    pub available_tools: Vec<String>,
    pub user_preferences: Option<String>,
}

/// Context available when checking dynamic escalation inside Agent Loop.
pub struct EscalationContext {
    pub step_count: usize,
    pub tools_invoked: Vec<String>,
    pub has_failures: bool,
    pub original_message: String,
}

/// Snapshot of Agent Loop state when escalating.
#[derive(Debug, Clone)]
pub struct EscalationSnapshot {
    pub original_message: String,
    pub completed_steps: usize,
    pub tools_invoked: Vec<String>,
    pub partial_result: Option<String>,
}

/// The core routing trait. Implementations decide which execution path a task takes.
#[async_trait]
pub trait TaskRouter: Send + Sync {
    /// Pre-classification: categorize message before Agent Loop starts.
    async fn classify(&self, message: &str, context: &RouterContext) -> TaskRoute;

    /// Dynamic escalation: check if running Agent Loop should upgrade.
    /// Returns None to continue, Some(route) to escalate.
    async fn should_escalate(&self, state: &EscalationContext) -> Option<TaskRoute>;
}
```

**Step 2: Register module in routing/mod.rs**

In `core/src/routing/mod.rs`, add after the existing module declarations (around line 5):

```rust
pub mod task_router;
```

And add to exports (after line 12):

```rust
pub use task_router::{
    CollabStrategy, EscalationContext, EscalationSnapshot, ManifestHints, RouterContext,
    TaskRoute, TaskRouter,
};
```

**Step 3: Build and verify**

Run: `cargo check -p alephcore`
Expected: Compiles with no new errors

**Step 4: Commit**

```bash
git add core/src/routing/task_router.rs core/src/routing/mod.rs
git commit -m "routing: add TaskRoute types and TaskRouter trait"
```

---

### Task 2: Rule-Based Classifier

**Files:**
- Create: `core/src/routing/rules.rs`
- Modify: `core/src/routing/mod.rs`

**Step 1: Create rules.rs**

```rust
//! Rule-based task classifier (zero-latency path).
//!
//! Pattern-matches incoming messages against configurable regex rules.
//! Returns Some(TaskRoute) on confident match, None to fall through to LLM.

use regex::Regex;
use super::task_router::{CollabStrategy, ManifestHints, TaskRoute};

/// A compiled set of routing rules.
pub struct RoutingRules {
    collaborative_patterns: Vec<Regex>,
    critical_patterns: Vec<Regex>,
    multi_step_patterns: Vec<Regex>,
    simple_patterns: Vec<Regex>,
}

impl RoutingRules {
    /// Compile rules from string patterns. Invalid patterns are logged and skipped.
    pub fn compile(config: &RoutingPatternsConfig) -> Self {
        Self {
            collaborative_patterns: compile_patterns(&config.collaborative),
            critical_patterns: compile_patterns(&config.critical),
            multi_step_patterns: compile_patterns(&config.multi_step),
            simple_patterns: compile_patterns(&config.simple),
        }
    }

    /// Classify a message using rules. Returns None if no rule matches.
    pub fn classify(&self, message: &str) -> Option<TaskRoute> {
        // Priority order: collaborative > critical > multi_step > simple
        if let Some(pattern) = first_match(&self.collaborative_patterns, message) {
            return Some(TaskRoute::Collaborative {
                reason: format!("Rule match: {}", pattern),
                strategy: infer_collab_strategy(message),
            });
        }

        if let Some(pattern) = first_match(&self.critical_patterns, message) {
            return Some(TaskRoute::Critical {
                reason: format!("Rule match: {}", pattern),
                manifest_hints: ManifestHints::default(),
            });
        }

        if let Some(pattern) = first_match(&self.multi_step_patterns, message) {
            return Some(TaskRoute::MultiStep {
                reason: format!("Rule match: {}", pattern),
            });
        }

        if first_match(&self.simple_patterns, message).is_some() {
            return Some(TaskRoute::Simple);
        }

        None
    }
}

/// Configuration for routing patterns (deserialized from config.toml).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RoutingPatternsConfig {
    #[serde(default = "default_critical_patterns")]
    pub critical: Vec<String>,
    #[serde(default = "default_multi_step_patterns")]
    pub multi_step: Vec<String>,
    #[serde(default = "default_simple_patterns")]
    pub simple: Vec<String>,
    #[serde(default = "default_collaborative_patterns")]
    pub collaborative: Vec<String>,
}

fn default_critical_patterns() -> Vec<String> {
    vec![
        r"生成.*报告".into(),
        r"分析.*并.*生成".into(),
        r"审查.*并修复".into(),
    ]
}

fn default_multi_step_patterns() -> Vec<String> {
    vec![
        r"先.*然后.*最后".into(),
        r"分步".into(),
        r"依次完成".into(),
    ]
}

fn default_simple_patterns() -> Vec<String> {
    vec![
        r"^你好".into(),
        r"^什么是".into(),
        r"^帮我翻译".into(),
    ]
}

fn default_collaborative_patterns() -> Vec<String> {
    vec![
        r"/group".into(),
        r"@专家".into(),
    ]
}

fn compile_patterns(patterns: &[String]) -> Vec<Regex> {
    patterns
        .iter()
        .filter_map(|p| match Regex::new(p) {
            Ok(r) => Some(r),
            Err(e) => {
                tracing::warn!(pattern = %p, error = %e, "Invalid routing pattern, skipping");
                None
            }
        })
        .collect()
}

fn first_match<'a>(patterns: &'a [Regex], text: &str) -> Option<String> {
    patterns.iter().find(|p| p.is_match(text)).map(|p| p.to_string())
}

fn infer_collab_strategy(message: &str) -> CollabStrategy {
    if message.contains("/group") {
        CollabStrategy::GroupChat
    } else if message.contains("审查") || message.contains("review") {
        CollabStrategy::Adversarial
    } else {
        CollabStrategy::Parallel
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> RoutingPatternsConfig {
        RoutingPatternsConfig::default()
    }

    #[test]
    fn test_simple_greeting() {
        let rules = RoutingRules::compile(&default_config());
        let result = rules.classify("你好，请问今天天气如何？");
        assert!(matches!(result, Some(TaskRoute::Simple)));
    }

    #[test]
    fn test_critical_report() {
        let rules = RoutingRules::compile(&default_config());
        let result = rules.classify("分析这周的数据并生成一份报告");
        assert!(matches!(result, Some(TaskRoute::Critical { .. })));
    }

    #[test]
    fn test_multi_step() {
        let rules = RoutingRules::compile(&default_config());
        let result = rules.classify("先搜索文件，然后修改代码，最后运行测试");
        assert!(matches!(result, Some(TaskRoute::MultiStep { .. })));
    }

    #[test]
    fn test_collaborative_group() {
        let rules = RoutingRules::compile(&default_config());
        let result = rules.classify("/group 讨论一下这个方案");
        assert!(matches!(result, Some(TaskRoute::Collaborative { .. })));
    }

    #[test]
    fn test_no_match() {
        let rules = RoutingRules::compile(&default_config());
        let result = rules.classify("请帮我查一下最近的聊天记录");
        assert!(result.is_none());
    }

    #[test]
    fn test_invalid_pattern_skipped() {
        let config = RoutingPatternsConfig {
            critical: vec!["[invalid".into(), "有效模式".into()],
            ..Default::default()
        };
        let rules = RoutingRules::compile(&config);
        // Should not panic, invalid pattern is skipped
        let result = rules.classify("有效模式测试");
        assert!(matches!(result, Some(TaskRoute::Critical { .. })));
    }
}
```

**Step 2: Register in mod.rs**

Add `pub mod rules;` and `pub use rules::{RoutingRules, RoutingPatternsConfig};`

**Step 3: Build and test**

Run: `cargo test -p alephcore --lib routing::rules`
Expected: All 6 tests pass

**Step 4: Commit**

```bash
git add core/src/routing/rules.rs core/src/routing/mod.rs
git commit -m "routing: add rule-based task classifier with tests"
```

---

### Task 3: LLM Fallback Classifier

**Files:**
- Create: `core/src/routing/llm_classifier.rs`
- Modify: `core/src/routing/mod.rs`

**Step 1: Create llm_classifier.rs**

```rust
//! LLM-based task classifier (fallback when rules don't match).
//!
//! Uses a fast model to classify task intent into a TaskRoute.

use serde::Deserialize;
use super::task_router::{CollabStrategy, ManifestHints, TaskRoute};

/// LLM classification response.
#[derive(Debug, Deserialize)]
struct ClassifyResponse {
    route: String,
    reason: String,
    #[serde(default)]
    hard_constraints: Vec<String>,
    #[serde(default)]
    strategy: Option<String>,
}

/// Build the classification prompt for LLM.
pub fn build_classify_prompt(message: &str) -> String {
    format!(
        r#"Classify this user task into exactly ONE category.

Categories:
- "simple": Single-turn Q&A, greetings, translations, factual lookups
- "multi_step": Task requires 3+ sequential steps across different tools
- "critical": Task requires generating artifacts with quality verification (reports, code review + fix, analysis + output)
- "collaborative": Task explicitly requests multiple roles, group discussion, or adversarial review

Task: "{message}"

Respond with ONLY valid JSON (no markdown):
{{"route": "<category>", "reason": "<one sentence>", "hard_constraints": ["<optional>"], "strategy": "<parallel|adversarial|group_chat, only for collaborative>"}}"#
    )
}

/// Parse LLM response text into a TaskRoute.
pub fn parse_classify_response(response: &str) -> TaskRoute {
    // Try to extract JSON from response (LLM may wrap in markdown)
    let json_str = extract_json(response);

    match serde_json::from_str::<ClassifyResponse>(&json_str) {
        Ok(resp) => match resp.route.as_str() {
            "simple" => TaskRoute::Simple,
            "multi_step" => TaskRoute::MultiStep {
                reason: resp.reason,
            },
            "critical" => TaskRoute::Critical {
                reason: resp.reason,
                manifest_hints: ManifestHints {
                    hard_constraints: resp.hard_constraints,
                    quality_threshold: 0.7,
                },
            },
            "collaborative" => TaskRoute::Collaborative {
                reason: resp.reason,
                strategy: match resp.strategy.as_deref() {
                    Some("adversarial") => CollabStrategy::Adversarial,
                    Some("group_chat") => CollabStrategy::GroupChat,
                    _ => CollabStrategy::Parallel,
                },
            },
            _ => {
                tracing::warn!(route = %resp.route, "Unknown LLM route, defaulting to Simple");
                TaskRoute::Simple
            }
        },
        Err(e) => {
            tracing::warn!(error = %e, response = %response, "Failed to parse LLM classify response, defaulting to Simple");
            TaskRoute::Simple
        }
    }
}

/// Extract JSON object from potentially markdown-wrapped response.
fn extract_json(text: &str) -> String {
    // Try direct parse first
    if text.trim().starts_with('{') {
        return text.trim().to_string();
    }
    // Try to find JSON block in markdown
    if let Some(start) = text.find('{') {
        if let Some(end) = text.rfind('}') {
            return text[start..=end].to_string();
        }
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple() {
        let resp = r#"{"route": "simple", "reason": "greeting"}"#;
        assert!(matches!(parse_classify_response(resp), TaskRoute::Simple));
    }

    #[test]
    fn test_parse_critical_with_constraints() {
        let resp = r#"{"route": "critical", "reason": "needs report", "hard_constraints": ["must generate PDF"]}"#;
        match parse_classify_response(resp) {
            TaskRoute::Critical { manifest_hints, .. } => {
                assert_eq!(manifest_hints.hard_constraints.len(), 1);
            }
            _ => panic!("Expected Critical"),
        }
    }

    #[test]
    fn test_parse_collaborative() {
        let resp = r#"{"route": "collaborative", "reason": "needs review", "strategy": "adversarial"}"#;
        match parse_classify_response(resp) {
            TaskRoute::Collaborative { strategy, .. } => {
                assert!(matches!(strategy, CollabStrategy::Adversarial));
            }
            _ => panic!("Expected Collaborative"),
        }
    }

    #[test]
    fn test_parse_markdown_wrapped() {
        let resp = "```json\n{\"route\": \"multi_step\", \"reason\": \"sequential\"}\n```";
        assert!(matches!(parse_classify_response(resp), TaskRoute::MultiStep { .. }));
    }

    #[test]
    fn test_parse_invalid_defaults_simple() {
        let resp = "I cannot parse this";
        assert!(matches!(parse_classify_response(resp), TaskRoute::Simple));
    }

    #[test]
    fn test_build_prompt_contains_message() {
        let prompt = build_classify_prompt("analyze my data");
        assert!(prompt.contains("analyze my data"));
        assert!(prompt.contains("simple"));
        assert!(prompt.contains("critical"));
    }
}
```

**Step 2: Register in mod.rs**

Add `pub mod llm_classifier;`

**Step 3: Build and test**

Run: `cargo test -p alephcore --lib routing::llm_classifier`
Expected: All 6 tests pass

**Step 4: Commit**

```bash
git add core/src/routing/llm_classifier.rs core/src/routing/mod.rs
git commit -m "routing: add LLM fallback classifier with prompt and parser"
```

---

### Task 4: CompositeRouter Implementation

**Files:**
- Create: `core/src/routing/composite_router.rs`
- Modify: `core/src/routing/mod.rs`

**Step 1: Create composite_router.rs**

```rust
//! CompositeRouter — rules-first, LLM-fallback task router.
//!
//! Combines rule-based classification (zero latency) with LLM fallback
//! for ambiguous cases. Also handles dynamic escalation checks.

use async_trait::async_trait;
use super::rules::RoutingRules;
use super::llm_classifier;
use super::task_router::{EscalationContext, RouterContext, TaskRoute, TaskRouter};

/// Composite router combining rules + LLM fallback.
pub struct CompositeRouter {
    rules: RoutingRules,
    /// Whether to use LLM when rules don't match
    llm_fallback_enabled: bool,
    /// Step threshold for dynamic escalation
    escalation_threshold: usize,
    /// Optional: LLM classify function (injected for testability)
    /// In production, this calls Thinker. In tests, it can be mocked.
    llm_classify_fn: Option<Box<dyn Fn(&str) -> TaskRoute + Send + Sync>>,
}

impl CompositeRouter {
    pub fn new(rules: RoutingRules, llm_fallback_enabled: bool, escalation_threshold: usize) -> Self {
        Self {
            rules,
            llm_fallback_enabled,
            escalation_threshold,
            llm_classify_fn: None,
        }
    }

    /// Set a custom LLM classify function (useful for testing).
    pub fn with_llm_classify_fn(
        mut self,
        f: impl Fn(&str) -> TaskRoute + Send + Sync + 'static,
    ) -> Self {
        self.llm_classify_fn = Some(Box::new(f));
        self
    }
}

#[async_trait]
impl TaskRouter for CompositeRouter {
    async fn classify(&self, message: &str, _context: &RouterContext) -> TaskRoute {
        // Phase 1: Try rules (zero latency)
        if let Some(route) = self.rules.classify(message) {
            tracing::info!(
                subsystem = "task_router",
                event = "classified",
                route = route.label(),
                method = "rules",
                "task classified via rules"
            );
            return route;
        }

        // Phase 2: LLM fallback
        if self.llm_fallback_enabled {
            let route = if let Some(ref classify_fn) = self.llm_classify_fn {
                classify_fn(message)
            } else {
                // In production, this would call Thinker with build_classify_prompt.
                // For now, default to Simple when no LLM function is configured.
                tracing::debug!("No LLM classify function configured, defaulting to Simple");
                TaskRoute::Simple
            };

            tracing::info!(
                subsystem = "task_router",
                event = "classified",
                route = route.label(),
                method = "llm_fallback",
                "task classified via LLM fallback"
            );
            return route;
        }

        // No match, no fallback — default to Simple
        tracing::info!(
            subsystem = "task_router",
            event = "classified",
            route = "simple",
            method = "default",
            "task classified as simple (no match, no fallback)"
        );
        TaskRoute::Simple
    }

    async fn should_escalate(&self, state: &EscalationContext) -> Option<TaskRoute> {
        // Check step threshold
        if state.step_count < self.escalation_threshold {
            return None;
        }

        // Heuristic: if tools span multiple unrelated domains, suggest parallel
        let unique_tool_prefixes: std::collections::HashSet<&str> = state
            .tools_invoked
            .iter()
            .filter_map(|t| t.split('_').next())
            .collect();

        if unique_tool_prefixes.len() >= 3 {
            return Some(TaskRoute::Collaborative {
                reason: format!(
                    "Agent used {} different tool domains in {} steps",
                    unique_tool_prefixes.len(),
                    state.step_count
                ),
                strategy: super::task_router::CollabStrategy::Parallel,
            });
        }

        // Heuristic: if there are failures, suggest POE protection
        if state.has_failures {
            return Some(TaskRoute::Critical {
                reason: format!(
                    "Agent encountered failures after {} steps",
                    state.step_count
                ),
                manifest_hints: super::task_router::ManifestHints::default(),
            });
        }

        // Step count exceeded but no other signal — suggest DAG decomposition
        Some(TaskRoute::MultiStep {
            reason: format!(
                "Agent exceeded {} steps without completion",
                self.escalation_threshold
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::rules::RoutingPatternsConfig;

    fn test_router() -> CompositeRouter {
        let rules = RoutingRules::compile(&RoutingPatternsConfig::default());
        CompositeRouter::new(rules, false, 3)
    }

    fn test_router_with_llm() -> CompositeRouter {
        let rules = RoutingRules::compile(&RoutingPatternsConfig::default());
        CompositeRouter::new(rules, true, 3)
            .with_llm_classify_fn(|_msg| TaskRoute::MultiStep {
                reason: "LLM decided".into(),
            })
    }

    fn test_context() -> RouterContext {
        RouterContext {
            session_history_len: 0,
            available_tools: vec![],
            user_preferences: None,
        }
    }

    #[tokio::test]
    async fn test_rules_take_priority() {
        let router = test_router_with_llm();
        let ctx = test_context();
        // "你好" matches simple rule
        let route = router.classify("你好", &ctx).await;
        assert!(matches!(route, TaskRoute::Simple));
    }

    #[tokio::test]
    async fn test_llm_fallback_when_no_rule() {
        let router = test_router_with_llm();
        let ctx = test_context();
        // No rule matches this
        let route = router.classify("请帮我优化这段代码", &ctx).await;
        assert!(matches!(route, TaskRoute::MultiStep { .. }));
    }

    #[tokio::test]
    async fn test_default_simple_when_no_fallback() {
        let router = test_router(); // LLM disabled
        let ctx = test_context();
        let route = router.classify("请帮我优化这段代码", &ctx).await;
        assert!(matches!(route, TaskRoute::Simple));
    }

    #[tokio::test]
    async fn test_escalation_below_threshold() {
        let router = test_router();
        let ctx = EscalationContext {
            step_count: 2,
            tools_invoked: vec!["search".into()],
            has_failures: false,
            original_message: "test".into(),
        };
        assert!(router.should_escalate(&ctx).await.is_none());
    }

    #[tokio::test]
    async fn test_escalation_on_failures() {
        let router = test_router();
        let ctx = EscalationContext {
            step_count: 3,
            tools_invoked: vec!["bash".into()],
            has_failures: true,
            original_message: "test".into(),
        };
        let route = router.should_escalate(&ctx).await;
        assert!(matches!(route, Some(TaskRoute::Critical { .. })));
    }

    #[tokio::test]
    async fn test_escalation_multi_domain() {
        let router = test_router();
        let ctx = EscalationContext {
            step_count: 4,
            tools_invoked: vec![
                "file_read".into(),
                "web_fetch".into(),
                "memory_search".into(),
            ],
            has_failures: false,
            original_message: "test".into(),
        };
        let route = router.should_escalate(&ctx).await;
        assert!(matches!(route, Some(TaskRoute::Collaborative { .. })));
    }
}
```

**Step 2: Register in mod.rs**

Add `pub mod composite_router;` and `pub use composite_router::CompositeRouter;`

**Step 3: Build and test**

Run: `cargo test -p alephcore --lib routing::composite_router`
Expected: All 6 tests pass

**Step 4: Commit**

```bash
git add core/src/routing/composite_router.rs core/src/routing/mod.rs
git commit -m "routing: add CompositeRouter with rules + LLM fallback + escalation"
```

---

### Task 5: LoopResult::Escalated Variant

**Files:**
- Modify: `core/src/agent_loop/loop_result.rs`

**Step 1: Read current file to confirm exact structure**

Read `core/src/agent_loop/loop_result.rs` and verify the enum definition matches exploration (variants at lines 7-33).

**Step 2: Add Escalated variant**

After the `PoeAborted` variant (around line 32), add:

```rust
    /// Task escalated to higher-level execution path via routing decision layer.
    Escalated {
        route: crate::routing::TaskRoute,
        context: crate::routing::EscalationSnapshot,
    },
```

**Step 3: Update helper methods**

In the `steps()` method, add match arm:
```rust
Self::Escalated { context, .. } => context.completed_steps,
```

In any `is_success()` or similar method, add:
```rust
Self::Escalated { .. } => false,
```

**Step 4: Build and verify**

Run: `cargo check -p alephcore`
Expected: Compiles (may need to update match arms elsewhere — follow compiler errors)

**Step 5: Fix any exhaustive match errors**

The compiler will flag any `match` on `LoopResult` that doesn't handle `Escalated`. Follow each error and add appropriate handling:
- In `agent_loop.rs` callback methods: treat as session end
- In `execution_engine/engine.rs` (line 713): add new match arm (will be done in Task 8)

**Step 6: Commit**

```bash
git add core/src/agent_loop/loop_result.rs
git commit -m "agent_loop: add LoopResult::Escalated variant for task routing"
```

---

### Task 6: EscalateTask Built-in Tool

**Files:**
- Create: `core/src/builtin_tools/escalate_task.rs`
- Modify: `core/src/builtin_tools/mod.rs`
- Modify: `core/src/executor/builtin_registry/definitions.rs`

**Step 1: Create escalate_task.rs**

```rust
//! EscalateTask tool — allows LLM to request routing escalation.
//!
//! When the LLM determines the current task is too complex for simple
//! think-act loop, it calls this tool to escalate to DAG/POE/Swarm.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EscalateTaskArgs {
    /// Target execution strategy: "multi_step", "critical", or "collaborative"
    pub target: String,

    /// Why this task should be escalated
    pub reason: String,

    /// Optional: suggested subtask decomposition
    #[serde(default)]
    pub subtasks: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EscalateTaskOutput {
    pub accepted: bool,
    pub message: String,
}

#[derive(Clone)]
pub struct EscalateTaskTool;

#[async_trait]
impl AlephTool for EscalateTaskTool {
    const NAME: &'static str = "escalate_task";
    const DESCRIPTION: &'static str = "Request escalation to a more capable execution strategy. \
        Call this when the current task requires multiple independent steps (multi_step), \
        strict quality verification with success criteria (critical), \
        or collaboration between different expert roles (collaborative). \
        Do NOT call for simple Q&A or single-tool tasks.";

    type Args = EscalateTaskArgs;
    type Output = EscalateTaskOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Validate target
        match args.target.as_str() {
            "multi_step" | "critical" | "collaborative" => {}
            other => {
                return Ok(EscalateTaskOutput {
                    accepted: false,
                    message: format!(
                        "Invalid target '{}'. Use: multi_step, critical, or collaborative.",
                        other
                    ),
                });
            }
        }

        // The actual escalation is handled by Agent Loop when it sees this tool result.
        // This tool just validates and signals intent.
        Ok(EscalateTaskOutput {
            accepted: true,
            message: format!(
                "Escalation to '{}' accepted. Reason: {}",
                args.target, args.reason
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_valid_escalation() {
        let tool = EscalateTaskTool;
        let result = tool
            .call(EscalateTaskArgs {
                target: "multi_step".into(),
                reason: "needs DAG".into(),
                subtasks: vec![],
            })
            .await
            .unwrap();
        assert!(result.accepted);
    }

    #[tokio::test]
    async fn test_invalid_target() {
        let tool = EscalateTaskTool;
        let result = tool
            .call(EscalateTaskArgs {
                target: "invalid".into(),
                reason: "test".into(),
                subtasks: vec![],
            })
            .await
            .unwrap();
        assert!(!result.accepted);
    }
}
```

**Step 2: Register in builtin_tools/mod.rs**

Add module declaration and re-export:
```rust
pub mod escalate_task;
pub use escalate_task::{EscalateTaskTool, EscalateTaskArgs, EscalateTaskOutput};
```

**Step 3: Register in definitions.rs**

In `core/src/executor/builtin_registry/definitions.rs`:

Add to `BUILTIN_TOOL_DEFINITIONS` array:
```rust
BuiltinToolDefinition {
    name: "escalate_task",
    description: "Request escalation to a more capable execution strategy",
    requires_config: false,
},
```

Add match arm in `create_tool_boxed()`:
```rust
"escalate_task" => Some(Box::new(EscalateTaskTool)),
```

**Step 4: Build and test**

Run: `cargo test -p alephcore --lib builtin_tools::escalate_task`
Expected: 2 tests pass

**Step 5: Commit**

```bash
git add core/src/builtin_tools/escalate_task.rs core/src/builtin_tools/mod.rs core/src/executor/builtin_registry/definitions.rs
git commit -m "tools: add escalate_task built-in tool for LLM-driven routing escalation"
```

---

### Task 7: Agent Loop Escalation Guard

**Files:**
- Modify: `core/src/agent_loop/agent_loop.rs`

**Step 1: Add escalation check in the OTAF loop**

This task requires careful reading of the current agent_loop.rs to find the exact insertion point. The escalation check should go AFTER the existing guard check block (around line 471) and BEFORE the compression block.

Add a new field to AgentLoop or accept a TaskRouter reference:

In the AgentLoop struct, add:
```rust
pub task_router: Option<Arc<dyn crate::routing::TaskRouter>>,
```

Add a builder method:
```rust
pub fn with_task_router(mut self, router: Arc<dyn crate::routing::TaskRouter>) -> Self {
    self.task_router = Some(router);
    self
}
```

**Step 2: Add escalation guard logic**

After the guard check block (after `return LoopResult::GuardTriggered(violation);`), add:

```rust
            // ===== Escalation Check =====
            if let Some(ref router) = self.task_router {
                if !state.escalation_checked {
                    let esc_ctx = crate::routing::EscalationContext {
                        step_count: state.step_count,
                        tools_invoked: state.tools_used(),
                        has_failures: state.has_failures(),
                        original_message: state.original_input().to_string(),
                    };
                    if let Some(route) = router.should_escalate(&esc_ctx).await {
                        state.escalation_checked = true;
                        tracing::info!(
                            subsystem = "task_router",
                            event = "escalation_triggered",
                            route = route.label(),
                            steps = state.step_count,
                            "task router triggered dynamic escalation"
                        );
                        let snapshot = crate::routing::EscalationSnapshot {
                            original_message: state.original_input().to_string(),
                            completed_steps: state.step_count,
                            tools_invoked: state.tools_used(),
                            partial_result: state.last_summary().map(|s| s.to_string()),
                        };
                        return LoopResult::Escalated { route, context: snapshot };
                    }
                }
            }
```

**Step 3: Add escalation_checked field to LoopState**

In `core/src/agent_loop/state.rs` (or wherever LoopState is defined), add:
```rust
pub escalation_checked: bool,  // default false
```

Also verify that `tools_used()`, `has_failures()`, `original_input()`, `last_summary()` methods exist on LoopState. If not, add simple accessor methods.

**Step 4: Handle escalate_task tool result**

In the Decision::UseTool handling section of agent_loop.rs, after the tool execution, check if the tool was `escalate_task`:

```rust
// After tool execution result is received
if tool_name == "escalate_task" {
    if let Ok(output) = serde_json::from_value::<EscalateTaskOutput>(result.clone()) {
        if output.accepted {
            let route = match args.get("target").and_then(|v| v.as_str()) {
                Some("multi_step") => TaskRoute::MultiStep { reason: output.message.clone() },
                Some("critical") => TaskRoute::Critical {
                    reason: output.message.clone(),
                    manifest_hints: ManifestHints::default(),
                },
                Some("collaborative") => TaskRoute::Collaborative {
                    reason: output.message.clone(),
                    strategy: CollabStrategy::Parallel,
                },
                _ => TaskRoute::MultiStep { reason: output.message.clone() },
            };
            let snapshot = EscalationSnapshot { /* ... same as above ... */ };
            return LoopResult::Escalated { route, context: snapshot };
        }
    }
}
```

**Step 5: Build and verify**

Run: `cargo check -p alephcore`
Expected: Compiles. Fix any missing method errors by adding accessors to LoopState.

**Step 6: Commit**

```bash
git add core/src/agent_loop/agent_loop.rs core/src/agent_loop/state.rs
git commit -m "agent_loop: add escalation guard and escalate_task tool handling"
```

---

### Task 8: ExecutionEngine Route Dispatch

**Files:**
- Modify: `core/src/gateway/execution_engine/engine.rs`

**Step 1: Add pre-classification before Agent Loop**

In `run_agent_loop()` (around line 414, after history is loaded), add:

```rust
    // ===== Task Routing Pre-Classification =====
    let task_route = if let Some(ref router) = self.task_router {
        let ctx = RouterContext {
            session_history_len: history.len(),
            available_tools: vec![], // populated from tool registry
            user_preferences: None,
        };
        let route = router.classify(&request.input, &ctx).await;
        tracing::info!(
            subsystem = "task_router",
            event = "pre_classified",
            route = route.label(),
            "task pre-classified before execution"
        );
        route
    } else {
        TaskRoute::Simple
    };
```

**Step 2: Add route dispatch**

Replace or wrap the current agent_loop.run() call with route-based dispatch:

```rust
    let result = match task_route {
        TaskRoute::Simple => {
            // Existing path — run Agent Loop directly
            agent_loop.run(run_context, callback.as_ref()).await
        }
        TaskRoute::MultiStep { reason } => {
            // Notify user
            emitter.emit_status("📋 正在规划多步执行计划...").await;
            // TODO: Call Dispatcher DAG (Phase 2 integration)
            // For now, fallback to Agent Loop
            tracing::info!("MultiStep route selected (reason: {}), falling back to Agent Loop", reason);
            agent_loop.run(run_context, callback.as_ref()).await
        }
        TaskRoute::Critical { reason, manifest_hints } => {
            emitter.emit_status("🔍 正在建立质量验证标准...").await;
            // TODO: Call POE Manager + Dispatcher DAG (Phase 2 integration)
            tracing::info!("Critical route selected (reason: {}), falling back to Agent Loop", reason);
            agent_loop.run(run_context, callback.as_ref()).await
        }
        TaskRoute::Collaborative { reason, strategy } => {
            emitter.emit_status("👥 正在组织多角色协作...").await;
            // TODO: Call Swarm Orchestrator (Phase 2 integration)
            tracing::info!("Collaborative route selected (reason: {}, strategy: {:?}), falling back to Agent Loop", reason, strategy);
            agent_loop.run(run_context, callback.as_ref()).await
        }
    };
```

**Step 3: Handle LoopResult::Escalated**

In the LoopResult match block (around line 713), add:

```rust
        LoopResult::Escalated { route, context } => {
            tracing::info!(
                subsystem = "task_router",
                event = "escalated",
                route = route.label(),
                completed_steps = context.completed_steps,
                "Agent Loop escalated to higher execution path"
            );
            // TODO: Re-dispatch via route (Phase 2)
            // For now, return the partial result or a message
            let msg = context.partial_result.unwrap_or_else(|| {
                format!("Task escalated to {} execution ({}步已完成). 完整的路由执行将在后续版本中实现。",
                    route.label(), context.completed_steps)
            });
            Ok(msg)
        }
```

**Step 4: Add task_router field to ExecutionEngine**

```rust
pub struct ExecutionEngine {
    // ... existing fields ...
    task_router: Option<Arc<dyn TaskRouter>>,
}
```

Add builder method:
```rust
pub fn with_task_router(mut self, router: Arc<dyn TaskRouter>) -> Self {
    self.task_router = Some(router);
    self
}
```

**Step 5: Build and verify**

Run: `cargo check -p alephcore`
Expected: Compiles with TODO comments for Phase 2 integration

**Step 6: Commit**

```bash
git add core/src/gateway/execution_engine/engine.rs
git commit -m "execution_engine: add task route dispatch with graceful fallback"
```

---

### Task 9: Configuration Section

**Files:**
- Create: `core/src/config/types/task_routing.rs`
- Modify: `core/src/config/types/mod.rs`
- Modify: `core/src/config/structs.rs`

**Step 1: Create task_routing.rs**

```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use crate::routing::rules::RoutingPatternsConfig;

/// Task routing decision layer configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskRoutingConfig {
    /// Enable the routing decision layer
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Enable LLM fallback when rules don't match
    #[serde(default = "default_true")]
    pub enable_llm_fallback: bool,

    /// Model tier for classification ("fast" = cheapest available)
    #[serde(default = "default_classify_model")]
    pub classify_model: String,

    /// Step count threshold for dynamic escalation
    #[serde(default = "default_escalation_threshold")]
    pub escalation_step_threshold: usize,

    /// Enable dynamic escalation from within Agent Loop
    #[serde(default = "default_true")]
    pub escalation_enabled: bool,

    /// Max parallel agents for Swarm
    #[serde(default = "default_max_parallel")]
    pub max_parallel_agents: usize,

    /// Max rounds for adversarial verification
    #[serde(default = "default_adversarial_rounds")]
    pub adversarial_max_rounds: usize,

    /// Pattern matching rules
    #[serde(default)]
    pub patterns: RoutingPatternsConfig,
}

impl Default for TaskRoutingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            enable_llm_fallback: true,
            classify_model: "fast".into(),
            escalation_step_threshold: 3,
            escalation_enabled: true,
            max_parallel_agents: 4,
            adversarial_max_rounds: 3,
            patterns: RoutingPatternsConfig::default(),
        }
    }
}

fn default_true() -> bool { true }
fn default_classify_model() -> String { "fast".into() }
fn default_escalation_threshold() -> usize { 3 }
fn default_max_parallel() -> usize { 4 }
fn default_adversarial_rounds() -> usize { 3 }
```

**Step 2: Register in config/types/mod.rs**

Add `pub mod task_routing;` and `pub use task_routing::TaskRoutingConfig;`

**Step 3: Add to Config struct in structs.rs**

Add field:
```rust
    #[serde(default)]
    pub task_routing: TaskRoutingConfig,
```

**Step 4: Build and verify**

Run: `cargo check -p alephcore`
Expected: Compiles

**Step 5: Commit**

```bash
git add core/src/config/types/task_routing.rs core/src/config/types/mod.rs core/src/config/structs.rs
git commit -m "config: add [routing] configuration section for task routing"
```

---

### Task 10: Wire Up — Server Startup Integration

**Files:**
- Modify: `core/src/bin/aleph/commands/start/mod.rs` (or wherever server startup assembles components)

**Step 1: Create and inject CompositeRouter at startup**

Find where ExecutionEngine is constructed. Add:

```rust
// Build task router from config
let task_router: Option<Arc<dyn TaskRouter>> = if config.task_routing.enabled {
    let rules = RoutingRules::compile(&config.task_routing.patterns);
    let router = CompositeRouter::new(
        rules,
        config.task_routing.enable_llm_fallback,
        config.task_routing.escalation_step_threshold,
    );
    Some(Arc::new(router))
} else {
    None
};

// Inject into ExecutionEngine
let engine = ExecutionEngine::new(/* existing args */)
    .with_task_router(task_router);

// Inject into AgentLoop (if constructed at startup)
// Or pass through ExecutionEngine to AgentLoop at runtime
```

**Step 2: Build and verify**

Run: `cargo check -p alephcore`
Expected: Compiles

**Step 3: Full test run**

Run: `cargo test -p alephcore --lib`
Expected: All existing tests pass + new routing tests pass

**Step 4: Commit**

```bash
git add core/src/bin/aleph/commands/start/mod.rs
git commit -m "server: wire up TaskRouter at startup from config"
```

---

### Task 11: Integration Smoke Test

**Step 1: Start server with routing enabled**

```bash
RUST_LOG=debug cargo run -p alephcore --bin aleph
```

**Step 2: Send test messages via Telegram and verify routing logs**

1. Simple: "你好" → expect `subsystem="task_router" event="pre_classified" route=simple method=rules`
2. Critical: "分析数据并生成报告" → expect `route=critical method=rules`
3. Ambiguous: "帮我优化代码" → expect `method=llm_fallback` or `method=default`

**Step 3: Verify in logs**

```bash
grep 'subsystem="task_router"' /tmp/aleph-diagnostic.log
```

**Step 4: Commit integration test notes**

```bash
git commit --allow-empty -m "routing: integration smoke test passed"
```

---

## Summary of Changes

| Task | Files | Description |
|------|-------|-------------|
| 1 | `routing/task_router.rs`, `routing/mod.rs` | Core types + TaskRouter trait |
| 2 | `routing/rules.rs` | Rule-based classifier + 6 tests |
| 3 | `routing/llm_classifier.rs` | LLM prompt builder + parser + 6 tests |
| 4 | `routing/composite_router.rs` | CompositeRouter impl + 6 tests |
| 5 | `agent_loop/loop_result.rs` | LoopResult::Escalated variant |
| 6 | `builtin_tools/escalate_task.rs`, registry | EscalateTask tool + 2 tests |
| 7 | `agent_loop/agent_loop.rs`, `state.rs` | Escalation guard + tool handling |
| 8 | `execution_engine/engine.rs` | Route dispatch + graceful fallback |
| 9 | `config/types/task_routing.rs`, `structs.rs` | Configuration section |
| 10 | `bin/aleph/commands/start/mod.rs` | Startup wiring |
| 11 | (smoke test) | Verify routing logs |

## Phase 2 (Future — NOT in this plan)

Tasks 8 uses TODO placeholders for actual Dispatcher/POE/Swarm integration. Phase 2 will:
- Replace `MultiStep` fallback with actual `Dispatcher::plan_and_execute()`
- Replace `Critical` fallback with actual `PoeManager::execute()`
- Replace `Collaborative` fallback with actual `SwarmCoordinator` orchestration
- Wire LLM classify function to real Thinker quick-call
