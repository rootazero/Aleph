//! Rule-based task classifier with configurable regex patterns.

use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::warn;

use super::task_router::{CollabStrategy, ManifestHints, TaskRoute};

/// Configurable regex patterns for each task category.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RoutingPatternsConfig {
    /// Patterns that indicate a critical task (report generation, analysis, review).
    #[serde(default = "default_critical")]
    pub critical: Vec<String>,
    /// Patterns that indicate a multi-step task.
    #[serde(default = "default_multi_step")]
    pub multi_step: Vec<String>,
    /// Patterns that indicate a simple task (greetings, translations).
    #[serde(default = "default_simple")]
    pub simple: Vec<String>,
    /// Patterns that indicate a collaborative task.
    #[serde(default = "default_collaborative")]
    pub collaborative: Vec<String>,
}

fn default_critical() -> Vec<String> {
    vec![
        r"生成.*报告".into(),
        r"分析.*并.*生成".into(),
        r"审查.*并修复".into(),
    ]
}

fn default_multi_step() -> Vec<String> {
    vec![
        r"先.*然后.*最后".into(),
        r"分步".into(),
        r"依次完成".into(),
    ]
}

fn default_simple() -> Vec<String> {
    vec![
        r"^你好".into(),
        r"^什么是".into(),
        r"^帮我翻译".into(),
    ]
}

fn default_collaborative() -> Vec<String> {
    vec![r"/group".into(), r"@专家".into()]
}

impl Default for RoutingPatternsConfig {
    fn default() -> Self {
        Self {
            critical: default_critical(),
            multi_step: default_multi_step(),
            simple: default_simple(),
            collaborative: default_collaborative(),
        }
    }
}

/// Compiled routing rules for zero-latency classification.
pub struct RoutingRules {
    critical: Vec<Regex>,
    multi_step: Vec<Regex>,
    simple: Vec<Regex>,
    collaborative: Vec<Regex>,
}

impl RoutingRules {
    /// Compile patterns from config, logging and skipping invalid regex.
    pub fn from_config(config: &RoutingPatternsConfig) -> Self {
        Self {
            critical: compile_patterns(&config.critical),
            multi_step: compile_patterns(&config.multi_step),
            simple: compile_patterns(&config.simple),
            collaborative: compile_patterns(&config.collaborative),
        }
    }

    /// Classify a message using rule-based pattern matching.
    ///
    /// Priority order: collaborative > critical > multi_step > simple.
    /// Returns `None` if no pattern matches.
    pub fn classify(&self, message: &str) -> Option<TaskRoute> {
        // Collaborative (highest priority)
        if self.collaborative.iter().any(|r| r.is_match(message)) {
            let strategy = infer_collab_strategy(message);
            return Some(TaskRoute::Collaborative {
                reason: "matched collaborative pattern".into(),
                strategy,
            });
        }

        // Critical
        if self.critical.iter().any(|r| r.is_match(message)) {
            return Some(TaskRoute::Critical {
                reason: "matched critical pattern".into(),
                manifest_hints: ManifestHints::default(),
            });
        }

        // Multi-step
        if self.multi_step.iter().any(|r| r.is_match(message)) {
            return Some(TaskRoute::MultiStep {
                reason: "matched multi-step pattern".into(),
            });
        }

        // Simple (lowest priority)
        if self.simple.iter().any(|r| r.is_match(message)) {
            return Some(TaskRoute::Simple);
        }

        None
    }
}

/// Compile a list of pattern strings, skipping invalid ones.
fn compile_patterns(patterns: &[String]) -> Vec<Regex> {
    patterns
        .iter()
        .filter_map(|p| match Regex::new(p) {
            Ok(r) => Some(r),
            Err(e) => {
                warn!(pattern = %p, error = %e, "skipping invalid routing pattern");
                None
            }
        })
        .collect()
}

/// Infer collaborative strategy from message content.
fn infer_collab_strategy(message: &str) -> CollabStrategy {
    if message.contains("/group") {
        CollabStrategy::GroupChat
    } else {
        CollabStrategy::Parallel
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_rules() -> RoutingRules {
        RoutingRules::from_config(&RoutingPatternsConfig::default())
    }

    #[test]
    fn simple_greeting() {
        let rules = default_rules();
        let route = rules.classify("你好，请问今天天气如何？");
        assert!(matches!(route, Some(TaskRoute::Simple)));
    }

    #[test]
    fn critical_report() {
        let rules = default_rules();
        let route = rules.classify("请生成一份季度报告");
        assert!(matches!(route, Some(TaskRoute::Critical { .. })));
    }

    #[test]
    fn multi_step_task() {
        let rules = default_rules();
        let route = rules.classify("先收集数据，然后分析趋势，最后输出结论");
        assert!(matches!(route, Some(TaskRoute::MultiStep { .. })));
    }

    #[test]
    fn collaborative_group() {
        let rules = default_rules();
        let route = rules.classify("/group 讨论一下架构方案");
        assert!(matches!(route, Some(TaskRoute::Collaborative { .. })));
        if let Some(TaskRoute::Collaborative { strategy, .. }) = route {
            assert!(matches!(strategy, CollabStrategy::GroupChat));
        }
    }

    #[test]
    fn no_match() {
        let rules = default_rules();
        let route = rules.classify("一些随机的文字内容");
        assert!(route.is_none());
    }

    #[test]
    fn invalid_pattern_skipped() {
        let config = RoutingPatternsConfig {
            simple: vec!["[invalid".into(), "^你好".into()],
            ..Default::default()
        };
        let rules = RoutingRules::from_config(&config);
        // Valid pattern still works
        let route = rules.classify("你好世界");
        assert!(matches!(route, Some(TaskRoute::Simple)));
    }
}
