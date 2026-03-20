//! Deterministic fallback compaction.
//!
//! When LLM-based summarization is unavailable (e.g., no provider configured
//! or rate-limited), this module applies rule-based truncation to keep the
//! context within budget.

/// Fallback level controlling how aggressively to compress.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FallbackLevel {
    Normal,
    Aggressive,
}

/// Extract the first sentence from text content.
fn first_sentence(text: &str) -> &str {
    for (i, c) in text.char_indices() {
        if (c == '.' || c == '!' || c == '?' || c == '\n') && i > 0 {
            return &text[..=i];
        }
    }
    text
}

/// Deterministic fallback: extract first sentence from each message,
/// concatenate, limit to max_chars.
pub fn deterministic_truncate(messages: &[(String, String)], max_chars: usize) -> String {
    // messages is Vec<(role, content)>
    let mut result = String::new();
    for (role, content) in messages {
        let sentence = first_sentence(content);
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&format!("[{}] {}", role, sentence));
    }
    if result.len() > max_chars {
        // Truncate at char boundary safely
        let boundary = result
            .char_indices()
            .take_while(|(i, _)| *i <= max_chars)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        format!("{}\n[Truncated]", &result[..boundary])
    } else {
        result
    }
}

/// Compute target token count for a summary at a given level.
pub fn target_tokens(input_tokens: usize, level: FallbackLevel) -> usize {
    match level {
        FallbackLevel::Normal => {
            let target = (input_tokens as f64 * 0.35) as usize;
            target.clamp(128, 800)
        }
        FallbackLevel::Aggressive => {
            let target = (input_tokens as f64 * 0.2) as usize;
            target.clamp(64, 400)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_first_sentence() {
        assert_eq!(first_sentence("Hello world. More text."), "Hello world.");
        assert_eq!(first_sentence("No period"), "No period");
        assert_eq!(first_sentence("Line one\nLine two"), "Line one\n");
    }

    #[test]
    fn test_deterministic_truncate_short() {
        let messages = vec![
            ("user".to_string(), "What is X?".to_string()),
            (
                "assistant".to_string(),
                "X is a thing. More details here.".to_string(),
            ),
        ];
        let result = deterministic_truncate(&messages, 512);
        assert!(result.contains("[user] What is X?"));
        assert!(result.contains("[assistant] X is a thing."));
        assert!(!result.contains("[Truncated]"));
    }

    #[test]
    fn test_deterministic_truncate_long() {
        let messages: Vec<(String, String)> = (0..100)
            .map(|i| ("user".to_string(), format!("Message number {} with content.", i)))
            .collect();
        let result = deterministic_truncate(&messages, 512);
        assert!(result.ends_with("[Truncated]"));
    }

    #[test]
    fn test_target_tokens_normal() {
        assert_eq!(target_tokens(1000, FallbackLevel::Normal), 350);
        assert_eq!(target_tokens(100, FallbackLevel::Normal), 128); // min clamp
        assert_eq!(target_tokens(5000, FallbackLevel::Normal), 800); // max clamp
    }

    #[test]
    fn test_target_tokens_aggressive() {
        assert_eq!(target_tokens(1000, FallbackLevel::Aggressive), 200);
        assert_eq!(target_tokens(100, FallbackLevel::Aggressive), 64); // min clamp
        assert_eq!(target_tokens(5000, FallbackLevel::Aggressive), 400); // max clamp
    }
}
