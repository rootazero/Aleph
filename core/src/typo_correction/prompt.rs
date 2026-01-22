//! Prompt constants for typo correction
//!
//! Contains the system prompt designed for identifying and correcting
//! common input method errors in Chinese and English text.

/// System prompt for the typo correction AI
///
/// This prompt is optimized for accuracy while maintaining reasonable latency.
/// Key focus: understanding user intent through context to fix phonetic errors.
pub const SYSTEM_PROMPT: &str = r#"你是中文输入法纠错专家。通过上下文理解用户真实意图，修正输入法错误。

核心任务：识别同音字/近音字错误，还原用户想表达的内容。

常见错误示例：
- "只是的海洋" → "知识的海洋"（只是/知识同音）
- "在见" → "再见"（在/再同音）
- "以经" → "已经"（以/已近音）
- "他地书" → "他的书"（地/的混淆）

规则：
- 结合上下文判断正确词语
- 只改错误，不润色
- 无错误时原样返回
- 直接输出结果，不解释"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_prompt_not_empty() {
        assert!(!SYSTEM_PROMPT.is_empty());
    }

    #[test]
    fn test_system_prompt_contains_key_instructions() {
        assert!(SYSTEM_PROMPT.contains("上下文"));
        assert!(SYSTEM_PROMPT.contains("同音字"));
        assert!(SYSTEM_PROMPT.contains("原样返回"));
    }
}
