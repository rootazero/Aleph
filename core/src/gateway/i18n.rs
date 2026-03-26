//! Core i18n — lightweight message catalog for user-facing system messages.
//!
//! Uses the same language codes as the Panel (`en`, `zh-Hans`).
//! Locale is resolved from `GeneralConfig.language` at runtime.

/// Supported locales.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locale {
    En,
    Zh,
}

impl Locale {
    /// Parse a locale string from config. Falls back to `Zh` for any Chinese variant.
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

    // --- execution errors ---
    ErrRateLimit,
    ErrAuth,
    ErrTimeout,
    ErrNetwork,
    ErrServiceUnavailable,
    ErrGeneric { detail: &'a str },

    // --- agent loop exhaustion ---
    ErrLoopExhausted { iterations: usize, tool_calls: usize },

    // --- topic generation prompt (sent to LLM, not shown to user) ---
    TopicGenerationPrompt { conversation: &'a str },
}

/// Translate a message key to the target locale.
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
        // Execution errors
        // ============================================================
        (Msg::ErrRateLimit, Locale::Zh) => {
            "⚠️ AI 服务调用配额已用尽，请稍后再试。".into()
        }
        (Msg::ErrRateLimit, Locale::En) => {
            "⚠️ AI service rate limit reached. Please try again later.".into()
        }

        (Msg::ErrAuth, Locale::Zh) => {
            "⚠️ AI 服务认证失败，请检查 API Key 配置。".into()
        }
        (Msg::ErrAuth, Locale::En) => {
            "⚠️ AI service authentication failed. Please check your API key.".into()
        }

        (Msg::ErrTimeout, Locale::Zh) => {
            "⚠️ AI 服务响应超时，请稍后重试。".into()
        }
        (Msg::ErrTimeout, Locale::En) => {
            "⚠️ AI service timed out. Please try again.".into()
        }

        (Msg::ErrNetwork, Locale::Zh) => {
            "⚠️ 网络连接异常，请检查网络后重试。".into()
        }
        (Msg::ErrNetwork, Locale::En) => {
            "⚠️ Network error. Please check your connection and try again.".into()
        }

        (Msg::ErrServiceUnavailable, Locale::Zh) => {
            "⚠️ AI 服务暂时不可用，请稍后重试。".into()
        }
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
        // Agent loop exhaustion
        // ============================================================
        (Msg::ErrLoopExhausted { iterations, tool_calls }, Locale::Zh) => {
            format!(
                "抱歉，我在处理这个请求时用了太多步骤但没能完成（{iterations} 次迭代，{tool_calls} 次工具调用）。\n\
                 请尝试更直接的指令，比如使用 /video、/image、/audio 等命令直接生成内容。"
            )
        }
        (Msg::ErrLoopExhausted { iterations, tool_calls }, Locale::En) => {
            format!(
                "Sorry, I was unable to complete the task within the allowed limits \
                 ({iterations} iterations, {tool_calls} tool calls).\n\
                 Try using direct commands like /video, /image, or /audio for generation tasks."
            )
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
// Convenience: classify execution errors
// ---------------------------------------------------------------------------

/// Classify a raw execution error string into the appropriate i18n message key,
/// then translate it.
pub fn format_execution_error(error: &str, locale: Locale) -> String {
    let lower = error.to_lowercase();

    let msg = if lower.contains("429")
        || lower.contains("too many requests")
        || lower.contains("rate limit")
        || lower.contains("usage_limit")
    {
        Msg::ErrRateLimit
    } else if lower.contains("401")
        || lower.contains("unauthorized")
        || lower.contains("invalid api key")
    {
        Msg::ErrAuth
    } else if lower.contains("timeout") || lower.contains("timed out") {
        Msg::ErrTimeout
    } else if lower.contains("connection") || lower.contains("network") || lower.contains("dns") {
        Msg::ErrNetwork
    } else if lower.contains("500")
        || lower.contains("internal server error")
        || lower.contains("503")
        || lower.contains("service unavailable")
    {
        Msg::ErrServiceUnavailable
    } else {
        let detail = truncate_error(error, 200);
        return t(Msg::ErrGeneric { detail }, locale);
    };

    t(msg, locale)
}

/// Truncate error message at a char boundary for display.
fn truncate_error(s: &str, max_chars: usize) -> &str {
    if s.chars().count() <= max_chars {
        return s;
    }
    let end = s
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
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
        let msg = t(Msg::NewSessionStarted { topic_suffix: " (测试)" }, Locale::Zh);
        assert!(msg.contains("新对话已开始"));

        let msg = t(Msg::NewSessionStarted { topic_suffix: " (test)" }, Locale::En);
        assert!(msg.contains("New conversation started"));
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
