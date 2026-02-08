//! Learning Agent - Agent Loop Integration for Automatic Rule Learning
//!
//! This module integrates the RuleLearner with the Agent Loop, enabling automatic
//! learning from L3 executions. It monitors execution events, extracts patterns,
//! and generates L2 routing rules.
//!
//! # Architecture
//!
//! ```text
//! Agent Loop → L3 Execution → Learning Agent → RuleLearner → ReflexLayer
//!     ↓            ↓              ↓               ↓              ↓
//!  Observe      Execute        Monitor         Learn         Update Rules
//! ```
//!
//! # Learning Flow
//!
//! 1. **Monitor**: Listen to L3 execution events
//! 2. **Extract**: Extract features from user input and action
//! 3. **Learn**: Train the classifier with success/failure feedback
//! 4. **Generate**: Generate L2 rules when confidence threshold is met
//! 5. **Deploy**: Add generated rules to ReflexLayer
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::engine::{LearningAgent, RuleLearner, ReflexLayer};
//!
//! let learner = RuleLearner::new();
//! let reflex_layer = ReflexLayer::new();
//! let mut agent = LearningAgent::new(learner, reflex_layer);
//!
//! // Monitor L3 execution
//! agent.on_l3_success("search for TODO", search_action).await;
//!
//! // Periodically generate and deploy rules
//! agent.generate_and_deploy_rules().await;
//! ```

use super::{AtomicAction, ReflexLayer, RuleLearner, LearnerStats};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Minimum number of observations before generating rules
const MIN_OBSERVATIONS: usize = 100;

/// Rule generation interval (in seconds)
const GENERATION_INTERVAL_SECS: u64 = 300; // 5 minutes

/// Learning event types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LearningEvent {
    /// L3 execution succeeded
    L3Success {
        input: String,
        action: AtomicAction,
        latency: Duration,
    },
    /// L3 execution failed
    L3Failure {
        input: String,
        action: AtomicAction,
        error: String,
    },
    /// Rule generated
    RuleGenerated {
        pattern: String,
        confidence: f64,
    },
    /// Rule deployed to L2
    RuleDeployed {
        pattern: String,
    },
}

/// Learning agent for automatic rule generation
pub struct LearningAgent {
    /// Rule learner
    learner: Arc<RuleLearner>,

    /// Reflex layer (for deploying rules)
    reflex_layer: Arc<RwLock<ReflexLayer>>,

    /// Learning events buffer
    events: DashMap<String, Vec<LearningEvent>>,

    /// Last rule generation time
    last_generation: Arc<RwLock<Instant>>,

    /// Agent statistics
    stats: Arc<RwLock<AgentStats>>,
}

impl LearningAgent {
    /// Create a new learning agent
    pub fn new(learner: Arc<RuleLearner>, reflex_layer: Arc<RwLock<ReflexLayer>>) -> Self {
        Self {
            learner,
            reflex_layer,
            events: DashMap::new(),
            last_generation: Arc::new(RwLock::new(Instant::now())),
            stats: Arc::new(RwLock::new(AgentStats::default())),
        }
    }

    /// Handle L3 execution success
    ///
    /// # Arguments
    ///
    /// * `input` - The user input that triggered the execution
    /// * `action` - The atomic action that was executed
    /// * `latency` - Execution latency
    pub async fn on_l3_success(&self, input: &str, action: AtomicAction, latency: Duration) {
        // Learn from success
        self.learner.learn_success(input, action.clone());

        // Record event
        let event = LearningEvent::L3Success {
            input: input.to_string(),
            action,
            latency,
        };
        self.record_event(input, event).await;

        // Update stats
        self.stats.write().await.l3_successes += 1;

        debug!(
            input = %input,
            latency = ?latency,
            "Learned from L3 success"
        );

        // Check if we should generate rules
        self.maybe_generate_rules().await;
    }

    /// Handle L3 execution failure
    ///
    /// # Arguments
    ///
    /// * `input` - The user input that triggered the execution
    /// * `action` - The atomic action that was attempted
    /// * `error` - Error message
    pub async fn on_l3_failure(&self, input: &str, action: AtomicAction, error: String) {
        // Learn from failure
        self.learner.learn_failure(input, action.clone());

        // Record event
        let event = LearningEvent::L3Failure {
            input: input.to_string(),
            action,
            error: error.clone(),
        };
        self.record_event(input, event).await;

        // Update stats
        self.stats.write().await.l3_failures += 1;

        debug!(
            input = %input,
            error = %error,
            "Learned from L3 failure"
        );
    }

    /// Generate and deploy L2 rules
    ///
    /// This method generates rules from learned patterns and deploys them to the ReflexLayer.
    pub async fn generate_and_deploy_rules(&self) -> usize {
        let rules = self.learner.generate_rules();
        let count = rules.len();

        if count > 0 {
            info!(count = count, "Generated {} new L2 rules", count);

            // Deploy rules to ReflexLayer
            let mut reflex = self.reflex_layer.write().await;
            for rule in rules {
                let pattern = format!("{:?}", rule.pattern);

                // Record event
                let event = LearningEvent::RuleGenerated {
                    pattern: pattern.clone(),
                    confidence: 0.85, // TODO: Get actual confidence from rule
                };
                self.record_event(&pattern, event).await;

                // Deploy rule
                reflex.add_rule(rule);

                let event = LearningEvent::RuleDeployed {
                    pattern: pattern.clone(),
                };
                self.record_event(&pattern, event).await;

                info!(pattern = %pattern, "Deployed rule to L2");
            }

            // Update stats
            self.stats.write().await.rules_deployed += count;
        }

        // Update last generation time
        *self.last_generation.write().await = Instant::now();

        count
    }

    /// Check if we should generate rules and do so if needed
    async fn maybe_generate_rules(&self) {
        let learner_stats = self.learner.stats();
        let last_gen = *self.last_generation.read().await;
        let elapsed = last_gen.elapsed();

        // Generate rules if:
        // 1. We have enough observations (>= MIN_OBSERVATIONS)
        // 2. Enough time has passed since last generation (>= GENERATION_INTERVAL_SECS)
        if learner_stats.total_observations >= MIN_OBSERVATIONS
            && elapsed.as_secs() >= GENERATION_INTERVAL_SECS
        {
            info!(
                observations = learner_stats.total_observations,
                elapsed_secs = elapsed.as_secs(),
                "Triggering automatic rule generation"
            );

            let count = self.generate_and_deploy_rules().await;

            if count > 0 {
                info!(count = count, "Auto-generated and deployed {} rules", count);
            } else {
                warn!("No rules generated despite meeting thresholds");
            }
        }
    }

    /// Record a learning event
    async fn record_event(&self, key: &str, event: LearningEvent) {
        self.events
            .entry(key.to_string())
            .or_default()
            .push(event);
    }

    /// Get learning statistics
    pub async fn stats(&self) -> AgentStats {
        self.stats.read().await.clone()
    }

    /// Get learner statistics
    pub fn learner_stats(&self) -> LearnerStats {
        self.learner.stats()
    }

    /// Clear all learning data
    pub async fn clear(&self) {
        self.learner.clear();
        self.events.clear();
        *self.stats.write().await = AgentStats::default();
        info!("Cleared all learning data");
    }
}

/// Learning agent statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentStats {
    /// Number of L3 successes observed
    pub l3_successes: usize,

    /// Number of L3 failures observed
    pub l3_failures: usize,

    /// Number of rules deployed
    pub rules_deployed: usize,
}

impl AgentStats {
    /// Get total observations
    pub fn total_observations(&self) -> usize {
        self.l3_successes + self.l3_failures
    }

    /// Get success rate
    pub fn success_rate(&self) -> f64 {
        let total = self.total_observations();
        if total == 0 {
            0.0
        } else {
            self.l3_successes as f64 / total as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{SearchPattern, SearchScope};

    #[tokio::test]
    async fn test_learning_agent_basic() {
        let learner = Arc::new(RuleLearner::new());
        let reflex_layer = Arc::new(RwLock::new(ReflexLayer::new()));
        let agent = LearningAgent::new(learner, reflex_layer);

        // Simulate L3 successes
        let action = AtomicAction::Search {
            pattern: SearchPattern::Regex {
                pattern: "TODO".to_string(),
            },
            scope: SearchScope::Workspace,
            filters: vec![],
        };

        for _ in 0..5 {
            agent
                .on_l3_success("search for TODO", action.clone(), Duration::from_millis(100))
                .await;
        }

        // Check stats
        let stats = agent.stats().await;
        assert_eq!(stats.l3_successes, 5);
        assert_eq!(stats.l3_failures, 0);
        assert_eq!(stats.success_rate(), 1.0);
    }

    #[tokio::test]
    async fn test_learning_agent_failure() {
        let learner = Arc::new(RuleLearner::new());
        let reflex_layer = Arc::new(RwLock::new(ReflexLayer::new()));
        let agent = LearningAgent::new(learner, reflex_layer);

        let action = AtomicAction::Bash {
            command: "invalid_command".to_string(),
            cwd: None,
        };

        agent
            .on_l3_failure("run invalid command", action, "Command not found".to_string())
            .await;

        let stats = agent.stats().await;
        assert_eq!(stats.l3_failures, 1);
        assert_eq!(stats.success_rate(), 0.0);
    }

    #[tokio::test]
    async fn test_rule_generation() {
        let learner = Arc::new(RuleLearner::new());
        let reflex_layer = Arc::new(RwLock::new(ReflexLayer::new()));
        let agent = LearningAgent::new(learner, reflex_layer.clone());

        // Train with enough samples
        let action = AtomicAction::Bash {
            command: "git status".to_string(),
            cwd: None,
        };

        for _ in 0..5 {
            agent
                .on_l3_success("git status", action.clone(), Duration::from_millis(50))
                .await;
        }

        // Generate rules
        let count = agent.generate_and_deploy_rules().await;
        assert_eq!(count, 1);

        // Check stats
        let stats = agent.stats().await;
        assert_eq!(stats.rules_deployed, 1);
    }

    #[tokio::test]
    async fn test_clear() {
        let learner = Arc::new(RuleLearner::new());
        let reflex_layer = Arc::new(RwLock::new(ReflexLayer::new()));
        let agent = LearningAgent::new(learner, reflex_layer);

        let action = AtomicAction::Bash {
            command: "test".to_string(),
            cwd: None,
        };

        agent
            .on_l3_success("test", action, Duration::from_millis(10))
            .await;

        agent.clear().await;

        let stats = agent.stats().await;
        assert_eq!(stats.total_observations(), 0);
    }
}
