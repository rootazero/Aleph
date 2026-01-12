//! Title generation for conversation topics.
//!
//! Uses a lightweight AI call to generate concise titles from conversation content.

const TITLE_PROMPT: &str = r#"Based on the following conversation, generate a very short title (maximum 15 Chinese characters or 30 English characters). Return ONLY the title, nothing else.

User: {user_input}
Assistant: {ai_response}

Title:"#;

/// Generate a prompt string for AI-based title generation.
///
/// # Arguments
/// * `user_input` - The user's first message (will be truncated to 200 chars)
/// * `ai_response` - The AI's first response (will be truncated to 200 chars)
///
/// # Returns
/// A prompt string to send to AI for title generation
pub fn build_title_prompt(user_input: &str, ai_response: &str) -> String {
    let truncated_user: String = user_input.chars().take(200).collect();
    let truncated_response: String = ai_response.chars().take(200).collect();

    TITLE_PROMPT
        .replace("{user_input}", &truncated_user)
        .replace("{ai_response}", &truncated_response)
}

/// Generate a default title from user input (fallback when AI call fails).
///
/// # Arguments
/// * `user_input` - The user's first message
///
/// # Returns
/// A truncated version of the user input (max 20 chars) with "..." if truncated
pub fn default_title(user_input: &str) -> String {
    let char_count = user_input.chars().count();
    let truncated: String = user_input.chars().take(20).collect();
    if truncated.chars().count() < char_count {
        format!("{}...", truncated)
    } else {
        truncated
    }
}

/// Clean a generated title by removing quotes and trimming whitespace.
///
/// # Arguments
/// * `title` - The raw title from AI
///
/// # Returns
/// Cleaned title string
pub fn clean_title(title: &str) -> String {
    title
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim()
        .to_string()
}

/// Validate a title and return a default if invalid.
///
/// A title is considered invalid if:
/// - It's empty after cleaning
/// - It's longer than 50 characters
///
/// # Arguments
/// * `title` - The cleaned title
/// * `user_input` - Fallback user input for default title
///
/// # Returns
/// The title if valid, or a default title based on user input
pub fn validate_title(title: &str, user_input: &str) -> String {
    let cleaned = clean_title(title);
    if cleaned.is_empty() || cleaned.chars().count() > 50 {
        default_title(user_input)
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_title_short() {
        let result = default_title("Hello");
        assert_eq!(result, "Hello");
    }

    #[test]
    fn test_default_title_long() {
        let result = default_title("This is a very long message that should be truncated");
        assert_eq!(result, "This is a very long ...");
    }

    #[test]
    fn test_default_title_chinese() {
        let result = default_title("这是一个非常长的中文消息需要被截断处理以便显示");
        // Input has 23 Chinese characters, truncated to 20 + "..."
        assert_eq!(result, "这是一个非常长的中文消息需要被截断处理以...");
    }

    #[test]
    fn test_default_title_exact_length() {
        let result = default_title("Exactly twenty chars");
        assert_eq!(result, "Exactly twenty chars");
    }

    #[test]
    fn test_build_title_prompt() {
        let prompt = build_title_prompt("Help me", "Sure!");
        assert!(prompt.contains("User: Help me"));
        assert!(prompt.contains("Assistant: Sure!"));
        assert!(prompt.contains("Title:"));
    }

    #[test]
    fn test_build_title_prompt_truncates_long_input() {
        let long_input = "a".repeat(300);
        let long_response = "b".repeat(300);
        let prompt = build_title_prompt(&long_input, &long_response);

        // Should contain truncated versions (200 chars each)
        let expected_user = "a".repeat(200);
        let expected_response = "b".repeat(200);
        assert!(prompt.contains(&expected_user));
        assert!(prompt.contains(&expected_response));
        // Should not contain full 300-char strings
        assert!(!prompt.contains(&long_input));
    }

    #[test]
    fn test_clean_title() {
        assert_eq!(clean_title("\"Hello World\""), "Hello World");
        assert_eq!(clean_title("'Hello World'"), "Hello World");
        assert_eq!(clean_title("  Hello World  "), "Hello World");
        assert_eq!(clean_title("\" Hello World \""), "Hello World");
    }

    #[test]
    fn test_validate_title_valid() {
        let result = validate_title("Good Title", "fallback");
        assert_eq!(result, "Good Title");
    }

    #[test]
    fn test_validate_title_empty() {
        let result = validate_title("", "fallback input");
        assert_eq!(result, "fallback input");
    }

    #[test]
    fn test_validate_title_too_long() {
        let long_title = "a".repeat(60);
        let result = validate_title(&long_title, "short fallback");
        assert_eq!(result, "short fallback");
    }

    #[test]
    fn test_validate_title_whitespace_only() {
        let result = validate_title("   ", "fallback");
        assert_eq!(result, "fallback");
    }
}
