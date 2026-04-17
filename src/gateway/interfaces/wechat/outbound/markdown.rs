//! Markdown to WeChat Format Conversion
//!
//! Converts Markdown text to WeChat-compatible format.

const MAX_MESSAGE_LENGTH: usize = 2000;

/// Convert markdown to WeChat format.
pub fn markdown_to_wechat(markdown: &str) -> String {
    let mut result = markdown.to_string();

    result = convert_bold(&result);
    result = convert_italic(&result);
    result = convert_code(&result);
    result = convert_link(&result);
    result = convert_list(&result);
    result = truncate(&result);

    result.trim().to_string()
}

fn convert_bold(text: &str) -> String {
    let mut result = text.to_string();
    result = result.replace("**", "");
    result
}

fn convert_italic(text: &str) -> String {
    let mut result = text.to_string();
    result = result.replace("*", "");
    result = result.replace("_", "");
    result
}

fn convert_code(text: &str) -> String {
    let mut result = text.to_string();
    result = result.replace("```", "");
    result = result.replace("`", "");
    result
}

fn convert_link(text: &str) -> String {
    let re = match regex::Regex::new(r"\[([^\]]+)\]\(([^\)]+)\)") {
        Ok(r) => r,
        Err(_) => return text.to_string(),
    };
    let result = re.replace_all(text, "$1: $2");
    result.to_string()
}

fn convert_list(text: &str) -> String {
    let mut result = text.to_string();
    result = result.replace("- ", "• ");
    result = result.replace("* ", "• ");
    let re = match regex::Regex::new(r"^\d+\.\s") {
        Ok(r) => r,
        Err(_) => return result,
    };
    result = re.replace_all(&result, "• ").to_string();
    result
}

fn truncate(text: &str) -> String {
    if text.len() > MAX_MESSAGE_LENGTH {
        format!("{}...(truncated)", &text[..MAX_MESSAGE_LENGTH])
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_bold() {
        assert_eq!(convert_bold("**hello**"), "hello");
    }

    #[test]
    fn test_convert_italic() {
        assert_eq!(convert_italic("*hello*"), "hello");
        assert_eq!(convert_italic("_hello_"), "hello");
    }

    #[test]
    fn test_convert_code() {
        assert_eq!(convert_code("`code`"), "code");
        assert_eq!(convert_code("```code```"), "code");
    }

    #[test]
    fn test_truncate() {
        let long_text = "a".repeat(3000);
        let result = truncate(&long_text);
        assert!(result.contains("truncated"));
        assert!(result.len() <= MAX_MESSAGE_LENGTH + 15);
    }

    #[test]
    fn test_markdown_to_wechat() {
        let md = "**bold** and *italic* and `code`";
        let result = markdown_to_wechat(md);
        assert!(!result.contains("**"));
        assert!(!result.contains("*"));
    }
}
