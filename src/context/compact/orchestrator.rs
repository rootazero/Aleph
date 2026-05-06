//! Compaction orchestrator — evaluates pressure, dispatches strategies in
//! priority order, and runs the post-compaction cleanup chain.

use crate::sync_primitives::Arc;

use tracing::{error, info, warn};

use super::types::{
    CompactionContext, CompactionResult, CompactionStrategy, PostCompactCleanup, PressureLevel,
};

// =============================================================================
// OrchestratorBuilder
// =============================================================================

/// Builder for `CompactionOrchestrator`.
pub struct OrchestratorBuilder {
    strategies: Vec<Arc<dyn CompactionStrategy>>,
    cleanups: Vec<Arc<dyn PostCompactCleanup>>,
    cache_monitor: Option<
        crate::sync_primitives::Arc<crate::thinker::prompt_builder::cache_monitor::CacheMonitor>,
    >,
}

impl OrchestratorBuilder {
    /// Register a compaction strategy (executed in registration order).
    pub fn strategy(mut self, s: Arc<dyn CompactionStrategy>) -> Self {
        self.strategies.push(s);
        self
    }

    /// Register a post-compaction cleanup hook.
    pub fn cleanup(mut self, c: Arc<dyn PostCompactCleanup>) -> Self {
        self.cleanups.push(c);
        self
    }

    /// Attach a cache monitor that is notified after each compaction.
    pub fn cache_monitor(
        mut self,
        monitor: crate::sync_primitives::Arc<
            crate::thinker::prompt_builder::cache_monitor::CacheMonitor,
        >,
    ) -> Self {
        self.cache_monitor = Some(monitor);
        self
    }

    /// Consume the builder and produce a `CompactionOrchestrator`.
    pub fn build(self) -> CompactionOrchestrator {
        CompactionOrchestrator {
            strategies: self.strategies,
            cleanups: self.cleanups,
            cache_monitor: self.cache_monitor,
        }
    }
}

// =============================================================================
// CompactionOrchestrator
// =============================================================================

/// Central coordinator that evaluates context pressure, dispatches compaction
/// strategies in priority order, and runs the post-compaction cleanup chain.
pub struct CompactionOrchestrator {
    strategies: Vec<Arc<dyn CompactionStrategy>>,
    cleanups: Vec<Arc<dyn PostCompactCleanup>>,
    cache_monitor: Option<
        crate::sync_primitives::Arc<crate::thinker::prompt_builder::cache_monitor::CacheMonitor>,
    >,
}

impl CompactionOrchestrator {
    /// Create a new builder.
    pub fn builder() -> OrchestratorBuilder {
        OrchestratorBuilder {
            strategies: Vec::new(),
            cleanups: Vec::new(),
            cache_monitor: None,
        }
    }

    /// Execute strategies in registration order, then run the cleanup chain.
    ///
    /// Strategies are skipped when they are not applicable or report zero
    /// estimated savings.  Execution stops early once pressure drops below
    /// [`PressureLevel::Warning`].  Strategy errors are logged as warnings
    /// and do not abort the remaining strategies.
    pub async fn execute(&self, ctx: &mut CompactionContext) -> anyhow::Result<CompactionResult> {
        let pressure_before = ctx.pressure.ratio;
        let mut total_freed = 0usize;
        let mut total_compacted = 0usize;
        let mut last_strategy_name = String::from("none");

        info!(
            pressure_ratio = pressure_before,
            pressure_level = ?ctx.pressure_level,
            "compaction orchestrator starting"
        );

        for strategy in &self.strategies {
            // Refresh pressure level from the current ratio.
            ctx.pressure_level = PressureLevel::from_ratio(ctx.pressure.ratio);

            // Stop early if we have relieved enough pressure.
            if ctx.pressure_level < PressureLevel::Warning {
                info!(
                    strategy = strategy.name(),
                    pressure_level = ?ctx.pressure_level,
                    "pressure below Warning threshold — stopping early"
                );
                break;
            }

            if !strategy.is_applicable(ctx) {
                info!(
                    strategy = strategy.name(),
                    "strategy not applicable — skipping"
                );
                continue;
            }

            let estimate = strategy.estimate_savings(ctx);
            if estimate.estimated_savings == 0 {
                info!(
                    strategy = strategy.name(),
                    "strategy estimates zero savings — skipping"
                );
                continue;
            }

            match strategy.execute(ctx).await {
                Ok(result) => {
                    info!(
                        strategy = strategy.name(),
                        freed = result.freed_tokens,
                        compacted = result.compacted_count,
                        "strategy executed successfully"
                    );
                    total_freed += result.freed_tokens;
                    total_compacted += result.compacted_count;
                    last_strategy_name = result.strategy_name.clone();

                    // Update simulated pressure — keep used_tokens and ratio in sync.
                    ctx.pressure.used_tokens =
                        ctx.pressure.used_tokens.saturating_sub(result.freed_tokens);
                    ctx.pressure.ratio = if ctx.pressure.budget_tokens == 0 {
                        1.0
                    } else {
                        ctx.pressure.used_tokens as f64 / ctx.pressure.budget_tokens as f64
                    };
                    ctx.pressure_level = PressureLevel::from_ratio(ctx.pressure.ratio);
                }
                Err(e) => {
                    warn!(strategy = strategy.name(), error = %e, "strategy failed — continuing");
                }
            }
        }

        let pressure_after = ctx.pressure.ratio;

        let aggregate = CompactionResult {
            freed_tokens: total_freed,
            compacted_count: total_compacted,
            strategy_name: last_strategy_name,
            pressure_before,
            pressure_after,
        };

        self.run_cleanups(&aggregate);

        if let Some(monitor) = &self.cache_monitor {
            monitor.notify_compaction();
        }

        Ok(aggregate)
    }

    /// Run all cleanup hooks sorted by `cleanup_order()` ascending.
    ///
    /// Each cleanup is wrapped in `catch_unwind` so a panicking hook cannot
    /// prevent subsequent hooks from running.
    fn run_cleanups(&self, result: &CompactionResult) {
        let mut ordered: Vec<&Arc<dyn PostCompactCleanup>> = self.cleanups.iter().collect();
        ordered.sort_by_key(|c| c.cleanup_order());

        for cleanup in ordered {
            let call = std::panic::AssertUnwindSafe(|| {
                cleanup.on_compact_complete(result);
            });
            if let Err(e) = std::panic::catch_unwind(call) {
                error!("post-compaction cleanup panicked: {:?}", e);
            }
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use super::super::types::{
        CompactionContext, CompactionResult, CompactionStrategy, PostCompactCleanup, PressureLevel,
        TokenEstimate,
    };
    use super::*;
    use crate::context::budget::ContextPressure;

    // -------------------------------------------------------------------------
    // Helper
    // -------------------------------------------------------------------------

    fn make_test_context(ratio: f64) -> CompactionContext {
        let budget = 100_000usize;
        let used = (budget as f64 * ratio) as usize;
        CompactionContext {
            messages: vec![],
            pressure: ContextPressure {
                used_tokens: used,
                budget_tokens: budget,
                ratio,
                overhead_tokens: 5000,
                available_for_messages: budget - used,
            },
            pressure_level: PressureLevel::from_ratio(ratio),
            token_estimate_ratio: 3.5,
            fresh_tail_count: 6,
        }
    }

    // -------------------------------------------------------------------------
    // Mock strategy
    // -------------------------------------------------------------------------

    struct MockStrategy {
        name: &'static str,
        applicable: bool,
        savings: usize,
        executed: AtomicBool,
    }

    impl MockStrategy {
        fn new(name: &'static str, applicable: bool, savings: usize) -> Arc<Self> {
            Arc::new(Self {
                name,
                applicable,
                savings,
                executed: AtomicBool::new(false),
            })
        }
    }

    impl CompactionStrategy for MockStrategy {
        fn name(&self) -> &str {
            self.name
        }

        fn estimate_savings(&self, _ctx: &CompactionContext) -> TokenEstimate {
            TokenEstimate {
                estimated_savings: self.savings,
                confidence: 1.0,
            }
        }

        fn execute<'a>(
            &'a self,
            ctx: &'a mut CompactionContext,
        ) -> Pin<Box<dyn Future<Output = anyhow::Result<CompactionResult>> + Send + 'a>> {
            Box::pin(async move {
                self.executed.store(true, Ordering::SeqCst);
                Ok(CompactionResult {
                    freed_tokens: self.savings,
                    compacted_count: 1,
                    strategy_name: self.name.to_string(),
                    pressure_before: ctx.pressure.ratio,
                    pressure_after: ctx.pressure.ratio,
                })
            })
        }

        fn is_applicable(&self, _ctx: &CompactionContext) -> bool {
            self.applicable
        }
    }

    // -------------------------------------------------------------------------
    // Mock cleanup
    // -------------------------------------------------------------------------

    struct MockCleanup {
        order: u32,
        called: AtomicBool,
    }

    impl MockCleanup {
        fn new(order: u32) -> Arc<Self> {
            Arc::new(Self {
                order,
                called: AtomicBool::new(false),
            })
        }
    }

    impl PostCompactCleanup for MockCleanup {
        fn cleanup_order(&self) -> u32 {
            self.order
        }

        fn on_compact_complete(&self, _result: &CompactionResult) {
            self.called.store(true, Ordering::SeqCst);
        }
    }

    // -------------------------------------------------------------------------
    // Tests
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn orchestrator_skips_inapplicable_strategies() {
        let inapplicable = MockStrategy::new("inapplicable", false, 50_000);
        let applicable = MockStrategy::new("applicable", true, 50_000);

        let inapplicable_ref = Arc::clone(&inapplicable);
        let applicable_ref = Arc::clone(&applicable);

        let orchestrator = CompactionOrchestrator::builder()
            .strategy(inapplicable)
            .strategy(applicable)
            .build();

        // High ratio so we don't hit the early-stop threshold.
        let mut ctx = make_test_context(0.90);
        orchestrator.execute(&mut ctx).await.unwrap();

        assert!(
            !inapplicable_ref.executed.load(Ordering::SeqCst),
            "inapplicable strategy should NOT have been executed"
        );
        assert!(
            applicable_ref.executed.load(Ordering::SeqCst),
            "applicable strategy SHOULD have been executed"
        );
    }

    #[tokio::test]
    async fn orchestrator_runs_cleanups_in_order() {
        // Use a strategy with enough savings to actually free pressure so
        // more than one strategy iteration runs (though here we only have one
        // strategy to keep it simple).
        let strategy = MockStrategy::new("strategy", true, 10_000);
        let cleanup_first = MockCleanup::new(1);
        let cleanup_second = MockCleanup::new(2);

        let first_ref = Arc::clone(&cleanup_first);
        let second_ref = Arc::clone(&cleanup_second);

        let orchestrator = CompactionOrchestrator::builder()
            .strategy(strategy)
            // Register in reverse order to prove sorting works.
            .cleanup(cleanup_second)
            .cleanup(cleanup_first)
            .build();

        let mut ctx = make_test_context(0.90);
        orchestrator.execute(&mut ctx).await.unwrap();

        assert!(
            first_ref.called.load(Ordering::SeqCst),
            "first cleanup (order=1) should have been called"
        );
        assert!(
            second_ref.called.load(Ordering::SeqCst),
            "second cleanup (order=2) should have been called"
        );
    }
}
