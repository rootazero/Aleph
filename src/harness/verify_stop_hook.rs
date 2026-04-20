//! VerifyStopHook — triggers verification before allowing the agent loop to stop.

use super::stop_hooks::{StopHookContext, StopHookHandler, StopHookVerdict};
use tokio_util::sync::CancellationToken;

/// Configuration for the verify stop hook.
pub struct VerifyStopHookConfig {
    pub trigger_for: Vec<String>,
    pub min_iterations: usize,
}

impl Default for VerifyStopHookConfig {
    fn default() -> Self {
        Self {
            trigger_for: vec!["main".into(), "coder".into()],
            min_iterations: 3,
        }
    }
}

pub struct VerifyStopHook {
    config: VerifyStopHookConfig,
    current_agent_id: String,
}

impl VerifyStopHook {
    pub fn new(current_agent_id: impl Into<String>, config: VerifyStopHookConfig) -> Self {
        Self {
            config,
            current_agent_id: current_agent_id.into(),
        }
    }

    /// Check whether verification should block this stop attempt.
    ///
    /// Returns `false` (allow stop) when:
    /// - The current agent is the verify agent itself (prevent recursion)
    /// - The current agent is not in the trigger list
    /// - The task was trivial (below iteration threshold)
    /// - Verification output is already present in `final_text` (VERDICT: marker)
    fn should_verify(&self, ctx: &StopHookContext) -> bool {
        if self.current_agent_id == "verify" {
            return false;
        }
        if !self.config.trigger_for.contains(&self.current_agent_id) {
            return false;
        }
        if ctx.iterations < self.config.min_iterations {
            return false;
        }

        // If the agent's final output already contains a verification verdict,
        // verification has been completed — allow the stop.
        if let Some(ref text) = ctx.final_text {
            if text.contains("VERDICT:") {
                return false;
            }
        }

        true
    }
}

#[async_trait::async_trait]
impl StopHookHandler for VerifyStopHook {
    fn name(&self) -> &str {
        "verify"
    }

    async fn evaluate(
        &self,
        ctx: &StopHookContext,
        _cancel: &CancellationToken,
    ) -> StopHookVerdict {
        if !self.should_verify(ctx) {
            return StopHookVerdict::Allow;
        }
        StopHookVerdict::Block {
            reason: format!(
                "[verify] Verification required for agent '{}' after {} iterations. \
                 Run build checks (cargo check), test suite (cargo test), and lint (cargo clippy). \
                 Report results with VERDICT: PASS/FAIL/PARTIAL.",
                self.current_agent_id, ctx.iterations
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ctx(iterations: usize) -> StopHookContext {
        StopHookContext {
            final_text: Some("done".into()),
            iterations,
            tool_calls_made: iterations * 2,
            stop_reason: "end_turn".into(),
        }
    }

    #[test]
    fn should_verify_true_for_main_with_enough_iterations() {
        let hook = VerifyStopHook::new("main", VerifyStopHookConfig::default());
        assert!(hook.should_verify(&make_ctx(5)));
    }

    #[test]
    fn should_verify_true_for_coder() {
        let hook = VerifyStopHook::new("coder", VerifyStopHookConfig::default());
        assert!(hook.should_verify(&make_ctx(3)));
    }

    #[test]
    fn should_verify_false_for_verify_agent() {
        let hook = VerifyStopHook::new("verify", VerifyStopHookConfig::default());
        assert!(!hook.should_verify(&make_ctx(10)));
    }

    #[test]
    fn should_verify_false_for_explore_agent() {
        let hook = VerifyStopHook::new("explore", VerifyStopHookConfig::default());
        assert!(!hook.should_verify(&make_ctx(10)));
    }

    #[test]
    fn should_verify_false_for_low_iterations() {
        let hook = VerifyStopHook::new("main", VerifyStopHookConfig::default());
        assert!(!hook.should_verify(&make_ctx(1)));
        assert!(!hook.should_verify(&make_ctx(2)));
    }

    #[tokio::test]
    async fn evaluate_blocks_when_verification_needed() {
        let hook = VerifyStopHook::new("main", VerifyStopHookConfig::default());
        let cancel = CancellationToken::new();
        match hook.evaluate(&make_ctx(5), &cancel).await {
            StopHookVerdict::Block { reason } => {
                assert!(reason.contains("[verify]"));
                assert!(reason.contains("cargo check"));
            }
            _ => panic!("Expected Block verdict"),
        }
    }

    #[tokio::test]
    async fn evaluate_allows_when_skipped() {
        let hook = VerifyStopHook::new("explore", VerifyStopHookConfig::default());
        let cancel = CancellationToken::new();
        match hook.evaluate(&make_ctx(5), &cancel).await {
            StopHookVerdict::Allow => {}
            _ => panic!("Expected Allow verdict"),
        }
    }

    #[test]
    fn should_verify_false_when_verdict_present() {
        let hook = VerifyStopHook::new("main", VerifyStopHookConfig::default());
        let ctx = StopHookContext {
            final_text: Some("All checks passed.\nVERDICT: PASS\nREASON: everything green".into()),
            iterations: 10,
            tool_calls_made: 20,
            stop_reason: "end_turn".into(),
        };
        assert!(!hook.should_verify(&ctx));
    }

    #[tokio::test]
    async fn evaluate_allows_after_verdict() {
        let hook = VerifyStopHook::new("main", VerifyStopHookConfig::default());
        let cancel = CancellationToken::new();
        let ctx = StopHookContext {
            final_text: Some("VERDICT: FAIL\nREASON: tests broken".into()),
            iterations: 10,
            tool_calls_made: 20,
            stop_reason: "end_turn".into(),
        };
        match hook.evaluate(&ctx, &cancel).await {
            StopHookVerdict::Allow => {}
            _ => panic!("Expected Allow after VERDICT present"),
        }
    }

    #[test]
    fn custom_config() {
        let config = VerifyStopHookConfig {
            trigger_for: vec!["custom_agent".into()],
            min_iterations: 10,
        };
        let hook = VerifyStopHook::new("custom_agent", config);
        assert!(!hook.should_verify(&make_ctx(5)));
        assert!(hook.should_verify(&make_ctx(10)));
    }
}
