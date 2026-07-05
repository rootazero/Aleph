//! User-facing receipt for a failed run.
//!
//! When a run dies, the execute loop emits a `StreamEvent::RunError` to the
//! originating channel. Previously that event carried `error = e.to_string()`,
//! which flattens the internal error chain (e.g. `"Execution failed: flow:
//! internal dispatch error: harness: llm error: Rate limit error: Anthropic
//! API rate limited (429)..."`) — noise that leaks internals and tells the
//! user nothing actionable.
//!
//! This maps the terminal [`ExecutionError`] into a stable `code` plus a short,
//! non-leaky message so the user learns *whether retrying is worthwhile*
//! (rate-limited / unreachable → yes, soon) without seeing the raw chain. The
//! typed error is still returned to internal callers unchanged; only the
//! user-channel presentation is improved.

use super::ExecutionError;

/// A user-facing receipt for a failed run: a stable machine code plus a short
/// human message suitable for rendering directly in a chat channel.
pub(super) struct FailureReceipt {
    pub code: &'static str,
    pub message: String,
}

impl FailureReceipt {
    pub(super) fn from_error(err: &ExecutionError) -> Self {
        match err {
            ExecutionError::Timeout => Self::new(
                "TIMEOUT",
                "任务超时未完成。请重试，或把任务拆小一些再发给我。",
            ),
            ExecutionError::Cancelled => Self::new("CANCELLED", "任务已取消。"),
            ExecutionError::AgentBusy(_) => {
                Self::new("AGENT_BUSY", "当前 Agent 正忙，请稍后再试。")
            }
            // The only string-carrying variant; classify by signature so a
            // provider rate-limit / network outage reads as transient.
            ExecutionError::Failed(msg) => classify_failed(msg),
            // Routing / lookup errors the user cannot act on — keep generic
            // and never echo the raw internal string.
            ExecutionError::RunNotFound(_)
            | ExecutionError::RunNotActive(_)
            | ExecutionError::Fallthrough { .. }
            | ExecutionError::Orchestrator(_) => Self::new("FAILED", "任务执行失败，请重试。"),
        }
    }

    fn new(code: &'static str, message: &str) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }
}

/// Classify a `Failed(msg)` payload. The message originates from the gateway
/// dispatch loop after failover/retries are exhausted, so a rate-limit or
/// network signature here means *every* provider/model in the chain was tried
/// and the failure is genuinely transient — worth telling the user to retry.
fn classify_failed(msg: &str) -> FailureReceipt {
    if is_rate_limited(msg) {
        FailureReceipt::new(
            "RATE_LIMITED",
            "模型服务商当前限流（请求过于频繁），且备用通道也已用尽。稍等片刻后重试即可。",
        )
    } else if is_unreachable(msg) {
        FailureReceipt::new(
            "PROVIDERS_UNREACHABLE",
            "无法连接到模型 / 搜索服务（网络不可达或服务暂时中断），所有可用通道均已尝试。请检查网络后重试。",
        )
    } else {
        FailureReceipt::new("FAILED", "任务执行失败，请重试。")
    }
}

fn is_rate_limited(msg: &str) -> bool {
    msg.contains("429")
        || msg.contains("rate limit")
        || msg.contains("Rate limit")
        || msg.contains("rate_limit")
        || msg.contains("receiving too many requests")
}

fn is_unreachable(msg: &str) -> bool {
    msg.contains("Network error")
        || msg.contains("error sending request")
        || msg.contains("connection")
        || msg.contains("Connection")
        || msg.contains("dns")
        || msg.contains("timed out")
        || msg.contains("502")
        || msg.contains("503")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_failure_reads_as_retryable() {
        let e = ExecutionError::Failed(
            "flow: internal dispatch error: harness: llm error: Rate limit error: \
             Anthropic API rate limited (429): receiving too many requests"
                .to_string(),
        );
        let r = FailureReceipt::from_error(&e);
        assert_eq!(r.code, "RATE_LIMITED");
        assert!(r.message.contains("限流"));
        // The raw internal chain must not leak to the user.
        assert!(!r.message.contains("dispatch error"));
    }

    #[test]
    fn network_outage_reads_as_unreachable() {
        let e = ExecutionError::Failed(
            "provider kimi-for-coding transient: Network error: error sending request \
             for url (https://api.kimi.com/coding/v1/messages)"
                .to_string(),
        );
        let r = FailureReceipt::from_error(&e);
        assert_eq!(r.code, "PROVIDERS_UNREACHABLE");
        assert!(r.message.contains("网络"));
    }

    #[test]
    fn rate_limit_takes_precedence_over_network() {
        // A 429 that also mentions a url should still classify as rate-limited.
        let e = ExecutionError::Failed(
            "429 rate limit on error sending request for url (x)".to_string(),
        );
        assert_eq!(FailureReceipt::from_error(&e).code, "RATE_LIMITED");
    }

    #[test]
    fn unclassified_failed_stays_generic() {
        let e = ExecutionError::Failed("some opaque internal failure".to_string());
        let r = FailureReceipt::from_error(&e);
        assert_eq!(r.code, "FAILED");
    }

    #[test]
    fn timeout_and_cancel_keep_their_codes() {
        assert_eq!(
            FailureReceipt::from_error(&ExecutionError::Timeout).code,
            "TIMEOUT"
        );
        assert_eq!(
            FailureReceipt::from_error(&ExecutionError::Cancelled).code,
            "CANCELLED"
        );
    }
}
