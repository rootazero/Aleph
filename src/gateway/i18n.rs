//! Core i18n — lightweight message catalog for user-facing system messages.
//!
//! Uses the same language codes as the Panel (`en`, `zh-Hans`).
//! Locale is resolved from `GeneralConfig.language` at runtime.

use crate::orchestrator::dispatch::TerminateReason;

/// Supported locales.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locale {
    En,
    Zh,
}

impl Locale {
    /// Parse a locale string from config. Falls back to `Zh` for any Chinese variant.
    #[must_use]
    pub fn from_config(s: Option<&str>) -> Self {
        match s {
            Some(s) if s.starts_with("en") => Self::En,
            _ => Self::Zh, // default to Chinese (matches current behavior)
        }
    }
}

// ---------------------------------------------------------------------------
// Message keys
// ---------------------------------------------------------------------------

/// All user-facing system messages that need translation.
pub enum Msg<'a> {
    // --- /new command ---
    NewSessionStarted { topic_suffix: &'a str },

    // --- /stop command ---
    RunStopped,
    NoActiveRun,

    // --- execution errors ---
    ErrRateLimit,
    ErrAuth,
    ErrTimeout,
    ErrNetwork,
    ErrServiceUnavailable,
    ErrGeneric { detail: &'a str },

    // --- topic generation prompt (sent to LLM, not shown to user) ---
    TopicGenerationPrompt { conversation: &'a str },
}

/// Translate a message key to the target locale.
#[must_use]
pub fn t(msg: Msg<'_>, locale: Locale) -> String {
    match (msg, locale) {
        // ============================================================
        // /new command
        // ============================================================
        (Msg::NewSessionStarted { topic_suffix }, Locale::Zh) => {
            format!("新对话已开始{topic_suffix}")
        }
        (Msg::NewSessionStarted { topic_suffix }, Locale::En) => {
            format!("New conversation started{topic_suffix}")
        }

        // ============================================================
        // /stop command
        // ============================================================
        (Msg::RunStopped, Locale::Zh) => "⏹ 已停止当前任务。".into(),
        (Msg::RunStopped, Locale::En) => "⏹ Stopped the current task.".into(),
        (Msg::NoActiveRun, Locale::Zh) => "当前没有正在执行的任务。".into(),
        (Msg::NoActiveRun, Locale::En) => "No task is currently running.".into(),

        // ============================================================
        // Execution errors
        // ============================================================
        (Msg::ErrRateLimit, Locale::Zh) => "⚠️ AI 服务调用配额已用尽，请稍后再试。".into(),
        (Msg::ErrRateLimit, Locale::En) => {
            "⚠️ AI service rate limit reached. Please try again later.".into()
        }

        (Msg::ErrAuth, Locale::Zh) => "⚠️ AI 服务认证失败，请检查 API Key 配置。".into(),
        (Msg::ErrAuth, Locale::En) => {
            "⚠️ AI service authentication failed. Please check your API key.".into()
        }

        (Msg::ErrTimeout, Locale::Zh) => "⚠️ AI 服务响应超时，请稍后重试。".into(),
        (Msg::ErrTimeout, Locale::En) => "⚠️ AI service timed out. Please try again.".into(),

        (Msg::ErrNetwork, Locale::Zh) => "⚠️ 网络连接异常，请检查网络后重试。".into(),
        (Msg::ErrNetwork, Locale::En) => {
            "⚠️ Network error. Please check your connection and try again.".into()
        }

        (Msg::ErrServiceUnavailable, Locale::Zh) => "⚠️ AI 服务暂时不可用，请稍后重试。".into(),
        (Msg::ErrServiceUnavailable, Locale::En) => {
            "⚠️ AI service temporarily unavailable. Please try again later.".into()
        }

        (Msg::ErrGeneric { detail }, Locale::Zh) => {
            format!("⚠️ 处理消息时出错：{detail}")
        }
        (Msg::ErrGeneric { detail }, Locale::En) => {
            format!("⚠️ Error processing message: {detail}")
        }

        // ============================================================
        // Topic generation prompt (sent to LLM, language-adaptive)
        // ============================================================
        (Msg::TopicGenerationPrompt { conversation }, Locale::Zh) => {
            format!(
                "用一句简短的中文概括以下对话的主题（10字以内，不要标点符号）：\n\n{conversation}"
            )
        }
        (Msg::TopicGenerationPrompt { conversation }, Locale::En) => {
            format!(
                "Summarize the topic of the following conversation in a short English phrase \
                 (under 10 words, no punctuation):\n\n{conversation}"
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Agent loop halt — render a cause-specific message per TerminateReason.
//
// Replaces the previous single `ErrLoopExhausted` blob, which always told
// the user to raise `[execution] max_iterations` — wrong advice for 7 of
// the 9 cap-style terminate reasons. Each branch now points at the actual
// root cause (consecutive tool failures, context budget, stall watchdog…)
// so the user sees actionable guidance instead of a misleading generic.
// ---------------------------------------------------------------------------

#[must_use]
pub fn render_loop_halt(
    reason: &TerminateReason,
    iterations: usize,
    tool_calls: usize,
    locale: Locale,
) -> String {
    match (reason, locale) {
        (TerminateReason::HitMaxIterations { used }, Locale::Zh) => format!(
            "抱歉，迭代上限 ({used}) 已用尽（共 {tool_calls} 次工具调用）。\n\
             如确需更多步骤，请调高 `[execution] max_iterations`，\
             或将任务拆分得更小、更具体。"
        ),
        (TerminateReason::HitMaxIterations { used }, Locale::En) => format!(
            "Sorry, hit the iteration cap ({used} iterations, {tool_calls} tool calls).\n\
             If more steps are needed, raise `[execution] max_iterations`, \
             or break the task into smaller, more specific steps."
        ),

        (TerminateReason::ConsecutiveFailureCap { consecutive }, Locale::Zh) => format!(
            "抱歉，连续 {consecutive} 轮工具调用全部失败（共 {iterations} 次迭代，\
             {tool_calls} 次工具调用）。\n\
             请检查工具配置或网络可达性。"
        ),
        (TerminateReason::ConsecutiveFailureCap { consecutive }, Locale::En) => format!(
            "Sorry, {consecutive} consecutive turns failed with tool errors \
             ({iterations} iterations, {tool_calls} tool calls).\n\
             Please check tool config or network reachability."
        ),

        (TerminateReason::ContextBudgetExhausted, Locale::Zh) => format!(
            "抱歉，上下文预算已用尽（{iterations} 次迭代后），无法继续。\n\
             请精简对话或新开会话。"
        ),
        (TerminateReason::ContextBudgetExhausted, Locale::En) => format!(
            "Sorry, the context budget was exhausted after {iterations} iterations.\n\
             Please trim the conversation or start a new session."
        ),

        (TerminateReason::StallTimeout { elapsed_ms }, Locale::Zh) => format!(
            "抱歉，{elapsed_ms} ms 内无任何进展被熔断（已执行 {iterations} 次迭代，\
             {tool_calls} 次工具调用）。\n\
             请检查工具是否阻塞或网络是否中断。"
        ),
        (TerminateReason::StallTimeout { elapsed_ms }, Locale::En) => format!(
            "Sorry, no progress for {elapsed_ms} ms — circuit-broken \
             ({iterations} iterations, {tool_calls} tool calls completed).\n\
             Check whether a tool is blocking or the network is stalled."
        ),

        (TerminateReason::TurnTimeout { phase, elapsed_ms }, Locale::Zh) => format!(
            "抱歉，单轮 `{phase}` 阶段 {elapsed_ms} ms 超时（已执行 {iterations} 次迭代）。"
        ),
        (TerminateReason::TurnTimeout { phase, elapsed_ms }, Locale::En) => format!(
            "Sorry, single-turn `{phase}` phase timed out at {elapsed_ms} ms \
             ({iterations} iterations completed)."
        ),

        (TerminateReason::VerifierVeto { vetos }, Locale::Zh) => {
            format!("抱歉，被验证器连续否决 {vetos} 次后退出（{iterations} 次迭代）。")
        }
        (TerminateReason::VerifierVeto { vetos }, Locale::En) => {
            format!("Sorry, exited after {vetos} verifier vetoes ({iterations} iterations).")
        }

        (TerminateReason::EmptyResponseExhausted, Locale::Zh) => format!(
            "抱歉，模型连续返回空响应（{iterations} 次迭代），无法继续。\n\
             这可能是上游提供商瞬时故障——请稍后重试。"
        ),
        (TerminateReason::EmptyResponseExhausted, Locale::En) => format!(
            "Sorry, the model returned empty responses for {iterations} iterations.\n\
             This may be an upstream provider issue — please try again later."
        ),

        (TerminateReason::StopHookHalt { reason }, Locale::Zh) => {
            format!("抱歉，Stop hook 拦截：{reason}。")
        }
        (TerminateReason::StopHookHalt { reason }, Locale::En) => {
            format!("Sorry, halted by Stop hook: {reason}.")
        }

        (TerminateReason::MaxOutputTokensExhausted, Locale::Zh) => format!(
            "抱歉，输出 token 上限耗尽（{iterations} 次迭代，{tool_calls} 次工具调用）。\n\
             请缩短请求或调高 `max_output_tokens`。"
        ),
        (TerminateReason::MaxOutputTokensExhausted, Locale::En) => format!(
            "Sorry, output token budget exhausted ({iterations} iterations, \
             {tool_calls} tool calls).\n\
             Shorten the request or raise `max_output_tokens`."
        ),

        (TerminateReason::DiminishingReturns, Locale::Zh) => format!(
            "检测到收益递减，已提前收尾（{iterations} 次迭代，{tool_calls} 次工具调用）。\n\
             如需继续，请把任务拆分得更具体后重试。"
        ),
        (TerminateReason::DiminishingReturns, Locale::En) => format!(
            "Stopped early on diminishing returns ({iterations} iterations, \
             {tool_calls} tool calls).\n\
             Break the task into more specific steps and retry if needed."
        ),

        // BudgetExhaustedPartialResult is the partial-result escalation
        // of a budget cap. The user-facing message highlights that the
        // partial work has been preserved for the next run (cron resume).
        // `reason` carries the underlying cap label ("hit_max_iterations",
        // "context_budget_exhausted", "max_output_tokens_exhausted") for
        // diagnostic surfacing.
        (TerminateReason::BudgetExhaustedPartialResult { reason, .. }, Locale::Zh) => format!(
            "抱歉，预算已用尽（{reason}），但已保留部分结果供下次续跑\
             （{iterations} 次迭代，{tool_calls} 次工具调用）。"
        ),
        (TerminateReason::BudgetExhaustedPartialResult { reason, .. }, Locale::En) => format!(
            "Sorry, the budget was exhausted ({reason}); partial result \
             preserved for resume ({iterations} iterations, \
             {tool_calls} tool calls)."
        ),

        (TerminateReason::ReactiveCompactExhausted, Locale::Zh) => format!(
            "抱歉，上下文已超出窗口，自动压缩后仍无法恢复（{iterations} 次迭代）。\n\
             请精简历史或新开会话。"
        ),
        (TerminateReason::ReactiveCompactExhausted, Locale::En) => format!(
            "Sorry, the prompt exceeded the model's context window and reactive \
             compaction did not recover it ({iterations} iterations).\n\
             Trim the conversation history or start a new session."
        ),

        // Completed and Cancelled should never reach here — callers gate on
        // `is_hit_limit()`. Fall back to a generic so we never panic in prod.
        // (`TerminateReason` is `#[non_exhaustive]` for downstream crates but
        // exhaustive within this crate — a new variant added in dispatch.rs
        // will force this match to be updated.)
        (TerminateReason::Completed | TerminateReason::Cancelled, Locale::Zh) => {
            format!("抱歉，会话提前结束（{iterations} 次迭代，{tool_calls} 次工具调用）。")
        }
        (TerminateReason::Completed | TerminateReason::Cancelled, Locale::En) => format!(
            "Sorry, the session ended early ({iterations} iterations, \
             {tool_calls} tool calls)."
        ),
    }
}

// ---------------------------------------------------------------------------
// Convenience: classify execution errors
// ---------------------------------------------------------------------------

/// Classify a raw execution error string into the appropriate i18n message key,
/// then translate it.
///
/// Substring matches use word boundaries (`\b…\b`) or whole-token equality so a
/// stray occurrence inside another word (e.g. an HTTP `429` echoing out of a
/// request id, or "connection" inside "disconnection_policy") does not
/// misclassify the error. This is the kind of regex/keyword classifier R8
/// flags — it is intentionally narrow (HTTP-status + a few provider
/// idioms) and the fallback is `ErrGeneric`, so any unmodelled error
/// degrades to the safe "show the raw message" path instead of a wrong label.
#[must_use]
pub fn format_execution_error(error: &str, locale: Locale) -> String {
    let lower = error.to_lowercase();

    let msg = if contains_phrase(&lower, &["429", "too many requests", "rate limit", "usage_limit"])
    {
        Msg::ErrRateLimit
    } else if contains_phrase(&lower, &["401", "unauthorized", "invalid api key"]) {
        Msg::ErrAuth
    } else if contains_phrase(&lower, &["timeout", "timed out"]) {
        Msg::ErrTimeout
    } else if contains_phrase(&lower, &["connection refused", "network", "dns "]) {
        Msg::ErrNetwork
    } else if contains_phrase(
        &lower,
        &["500", "internal server error", "503", "service unavailable"],
    ) {
        Msg::ErrServiceUnavailable
    } else {
        let detail = truncate_error(error, 200);
        return t(Msg::ErrGeneric { detail }, locale);
    };

    t(msg, locale)
}

/// True if `lower` contains ANY of the supplied phrases as a whole-token
/// substring — i.e. preceded by a non-alphanumeric (or start of string) AND
/// followed by a non-alphanumeric (or end of string). This avoids the
/// `lower.contains("connection")` foot-gun where "disconnection_policy"
/// would match.
fn contains_phrase(lower: &str, phrases: &[&str]) -> bool {
    phrases.iter().any(|p| {
        let needle = *p;
        let bytes = lower.as_bytes();
        let needle_bytes = needle.as_bytes();
        if needle_bytes.is_empty() || needle_bytes.len() > bytes.len() {
            return false;
        }
        let mut start = 0usize;
        while let Some(pos) = lower[start..].find(needle) {
            let abs = start + pos;
            let before_ok = abs == 0 || !bytes[abs - 1].is_ascii_alphanumeric();
            let end = abs + needle_bytes.len();
            let after_ok = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
            if before_ok && after_ok {
                return true;
            }
            start = abs + 1;
        }
        false
    })
}

/// Truncate error message at a char boundary for display.
fn truncate_error(s: &str, max_chars: usize) -> &str {
    if s.chars().count() <= max_chars {
        return s;
    }
    let end = s.char_indices().nth(max_chars).map_or(s.len(), |(i, _)| i);
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_locale_from_config() {
        assert_eq!(Locale::from_config(Some("en")), Locale::En);
        assert_eq!(Locale::from_config(Some("en-US")), Locale::En);
        assert_eq!(Locale::from_config(Some("zh-Hans")), Locale::Zh);
        assert_eq!(Locale::from_config(Some("zh")), Locale::Zh);
        assert_eq!(Locale::from_config(None), Locale::Zh);
    }

    #[test]
    fn test_new_session_message() {
        let msg = t(
            Msg::NewSessionStarted {
                topic_suffix: " (测试)",
            },
            Locale::Zh,
        );
        assert!(msg.contains("新对话已开始"));

        let msg = t(
            Msg::NewSessionStarted {
                topic_suffix: " (test)",
            },
            Locale::En,
        );
        assert!(msg.contains("New conversation started"));
    }

    #[test]
    fn render_loop_halt_hit_max_iterations_names_the_cap() {
        let msg = render_loop_halt(
            &TerminateReason::HitMaxIterations { used: 200 },
            200,
            42,
            Locale::En,
        );
        assert!(msg.contains("iteration cap"), "{msg}");
        assert!(msg.contains("200 iterations"), "{msg}");
        assert!(msg.contains("42 tool calls"), "{msg}");
        assert!(msg.contains("max_iterations"), "{msg}");
    }

    #[test]
    fn render_loop_halt_consecutive_failures_blames_tools_not_iterations() {
        let msg = render_loop_halt(
            &TerminateReason::ConsecutiveFailureCap { consecutive: 6 },
            6,
            6,
            Locale::Zh,
        );
        // Whole point of the fix: surface the real cause (tool failures /
        // network) and never point at the iteration cap, which is irrelevant.
        assert!(msg.contains("连续 6 轮工具调用全部失败"), "{msg}");
        assert!(msg.contains("工具配置或网络可达性"), "{msg}");
        assert!(
            !msg.contains("max_iterations"),
            "must NOT advise raising max_iterations for ConsecutiveFailureCap: {msg}"
        );
    }

    #[test]
    fn render_loop_halt_consecutive_failures_english_blames_tools_not_iterations() {
        let msg = render_loop_halt(
            &TerminateReason::ConsecutiveFailureCap { consecutive: 6 },
            6,
            6,
            Locale::En,
        );
        assert!(msg.contains("6 consecutive turns failed"), "{msg}");
        assert!(msg.contains("tool config or network reachability"), "{msg}");
        assert!(
            !msg.contains("max_iterations"),
            "must NOT advise raising max_iterations for ConsecutiveFailureCap: {msg}"
        );
    }

    #[test]
    fn render_loop_halt_stall_timeout_mentions_elapsed_ms() {
        let msg = render_loop_halt(
            &TerminateReason::StallTimeout { elapsed_ms: 30_000 },
            5,
            3,
            Locale::En,
        );
        assert!(msg.contains("30000 ms"), "{msg}");
        assert!(msg.contains("no progress"), "{msg}");
    }

    #[test]
    fn render_loop_halt_turn_timeout_names_phase() {
        let msg = render_loop_halt(
            &TerminateReason::TurnTimeout {
                phase: "act".into(),
                elapsed_ms: 12_000,
            },
            8,
            4,
            Locale::En,
        );
        assert!(msg.contains("`act`"), "{msg}");
        assert!(msg.contains("12000 ms"), "{msg}");
    }

    #[test]
    fn render_loop_halt_context_budget_advises_trim_session() {
        let msg = render_loop_halt(&TerminateReason::ContextBudgetExhausted, 50, 12, Locale::En);
        assert!(msg.contains("context budget"), "{msg}");
        assert!(msg.contains("trim") || msg.contains("new session"), "{msg}");
    }

    #[test]
    fn render_loop_halt_max_output_tokens_advises_token_knob() {
        let msg = render_loop_halt(&TerminateReason::MaxOutputTokensExhausted, 3, 0, Locale::En);
        assert!(msg.contains("max_output_tokens"), "{msg}");
        assert!(
            !msg.contains("max_iterations"),
            "max_output_tokens variant must not point at max_iterations: {msg}"
        );
    }

    #[test]
    fn render_loop_halt_stop_hook_surfaces_reason() {
        let msg = render_loop_halt(
            &TerminateReason::StopHookHalt {
                reason: "policy violation".into(),
            },
            2,
            0,
            Locale::En,
        );
        assert!(msg.contains("policy violation"), "{msg}");
    }

    #[test]
    fn render_loop_halt_completed_fallback_does_not_panic() {
        // Completed/Cancelled should never reach render_loop_halt (caller
        // gates on is_hit_limit) but if it does we must render a sensible
        // fallback instead of panicking.
        let msg = render_loop_halt(&TerminateReason::Completed, 10, 5, Locale::En);
        assert!(msg.contains("10 iterations"), "{msg}");
        assert!(msg.contains("5 tool calls"), "{msg}");
    }

    #[test]
    fn test_error_classification() {
        let msg = format_execution_error("429 Too Many Requests", Locale::En);
        assert!(msg.contains("rate limit"));

        let msg = format_execution_error("429 Too Many Requests", Locale::Zh);
        assert!(msg.contains("配额"));

        let msg = format_execution_error("connection refused", Locale::En);
        assert!(msg.contains("Network"));
    }
}
