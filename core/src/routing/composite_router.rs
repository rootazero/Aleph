//! Composite router combining rule-based and LLM-based classification.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use tracing::info;

use super::rules::RoutingRules;
use super::task_router::{
    CollabStrategy, EscalationContext, ManifestHints, RouterContext, TaskRoute, TaskRouter,
};

/// Type alias for an async LLM classify function.
pub type LlmClassifyFn =
    Arc<dyn Fn(&str) -> Pin<Box<dyn Future<Output = TaskRoute> + Send>> + Send + Sync>;

/// Composite router: tries rules first (zero latency), then optional LLM fallback.
pub struct CompositeRouter {
    rules: RoutingRules,
    llm_fallback_enabled: bool,
    escalation_threshold: usize,
    llm_classify_fn: Option<LlmClassifyFn>,
}

impl CompositeRouter {
    /// Create a new composite router.
    pub fn new(
        rules: RoutingRules,
        llm_fallback_enabled: bool,
        escalation_threshold: usize,
    ) -> Self {
        Self {
            rules,
            llm_fallback_enabled,
            escalation_threshold,
            llm_classify_fn: None,
        }
    }

    /// Set the LLM classify function for fallback classification.
    pub fn with_llm_classify_fn(mut self, f: LlmClassifyFn) -> Self {
        self.llm_classify_fn = Some(f);
        self
    }
}

#[async_trait]
impl TaskRouter for CompositeRouter {
    async fn classify(&self, message: &str, _context: &RouterContext) -> TaskRoute {
        // Try rules first (zero latency)
        if let Some(route) = self.rules.classify(message) {
            info!(
                subsystem = "task_router",
                source = "rules",
                route = route.label(),
                "task classified via rules"
            );
            return route;
        }

        // LLM fallback
        if self.llm_fallback_enabled {
            if let Some(ref classify_fn) = self.llm_classify_fn {
                let route = (classify_fn)(message).await;
                info!(
                    subsystem = "task_router",
                    source = "llm",
                    route = route.label(),
                    "task classified via LLM fallback"
                );
                return route;
            }
        }

        // Default
        info!(
            subsystem = "task_router",
            source = "default",
            route = "simple",
            "no classification matched, defaulting to simple"
        );
        TaskRoute::Simple
    }

    async fn should_escalate(&self, state: &EscalationContext) -> Option<TaskRoute> {
        // Below threshold — no escalation
        if state.step_count < self.escalation_threshold {
            info!(
                subsystem = "task_router",
                step_count = state.step_count,
                threshold = self.escalation_threshold,
                "below escalation threshold"
            );
            return None;
        }

        // Heuristic: multi-domain tools suggest collaborative
        let unique_prefixes: std::collections::HashSet<&str> = state
            .tools_invoked
            .iter()
            .filter_map(|t| t.split('_').next())
            .collect();
        if unique_prefixes.len() >= 3 {
            info!(
                subsystem = "task_router",
                domains = unique_prefixes.len(),
                "escalating to collaborative — multi-domain tools detected"
            );
            return Some(TaskRoute::Collaborative {
                reason: "multi-domain tools detected".into(),
                strategy: CollabStrategy::Parallel,
            });
        }

        // Heuristic: failures suggest critical
        if state.has_failures {
            info!(
                subsystem = "task_router",
                "escalating to critical — failures detected"
            );
            return Some(TaskRoute::Critical {
                reason: "task has failures, needs verification".into(),
                manifest_hints: ManifestHints::default(),
            });
        }

        // Default escalation: multi-step
        info!(
            subsystem = "task_router",
            step_count = state.step_count,
            "escalating to multi-step"
        );
        Some(TaskRoute::MultiStep {
            reason: format!("exceeded {} steps", self.escalation_threshold),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::rules::RoutingPatternsConfig;

    fn make_router(llm_fallback: bool) -> CompositeRouter {
        let rules = RoutingRules::from_config(&RoutingPatternsConfig::default());
        CompositeRouter::new(rules, llm_fallback, 5)
    }

    fn make_context() -> RouterContext {
        RouterContext {
            session_history_len: 0,
            available_tools: vec![],
            user_preferences: None,
        }
    }

    #[tokio::test]
    async fn rules_priority() {
        let router = make_router(false);
        let ctx = make_context();
        let route = router.classify("你好，请帮忙", &ctx).await;
        assert_eq!(route.label(), "simple");
    }

    #[tokio::test]
    async fn llm_fallback() {
        let router = make_router(true).with_llm_classify_fn(Arc::new(|_msg| {
            Box::pin(async {
                TaskRoute::MultiStep {
                    reason: "llm decided".into(),
                }
            })
        }));
        let ctx = make_context();
        // Message that won't match any rule
        let route = router.classify("做一些复杂的事情", &ctx).await;
        assert_eq!(route.label(), "multi_step");
    }

    #[tokio::test]
    async fn default_when_no_fallback() {
        let router = make_router(false);
        let ctx = make_context();
        let route = router.classify("做一些复杂的事情", &ctx).await;
        assert_eq!(route.label(), "simple");
    }

    #[tokio::test]
    async fn escalation_below_threshold() {
        let router = make_router(false);
        let state = EscalationContext {
            step_count: 2,
            tools_invoked: vec![],
            has_failures: false,
            original_message: "test".into(),
        };
        assert!(router.should_escalate(&state).await.is_none());
    }

    #[tokio::test]
    async fn escalation_on_failures() {
        let router = make_router(false);
        let state = EscalationContext {
            step_count: 6,
            tools_invoked: vec!["tool_a".into()],
            has_failures: true,
            original_message: "test".into(),
        };
        let route = router.should_escalate(&state).await;
        assert!(matches!(route, Some(TaskRoute::Critical { .. })));
    }

    #[tokio::test]
    async fn escalation_multi_domain() {
        let router = make_router(false);
        let state = EscalationContext {
            step_count: 6,
            tools_invoked: vec![
                "web_search".into(),
                "code_review".into(),
                "file_read".into(),
            ],
            has_failures: false,
            original_message: "test".into(),
        };
        let route = router.should_escalate(&state).await;
        assert!(matches!(route, Some(TaskRoute::Collaborative { .. })));
    }
}
