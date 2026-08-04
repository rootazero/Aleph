const MAX_MESSAGE_LENGTH: usize = 4000;

#[must_use]
pub fn markdown_to_wechat(markdown: &str) -> String {
    let mut result = markdown.to_string();

    result = convert_bold(&result);
    result = convert_code_blocks(&result);
    result = convert_link(&result);
    result = convert_list(&result);
    result = convert_italic(&result);
    result = convert_inline_code(&result);
    result = truncate(&result);

    result.trim().to_string()
}

fn convert_bold(text: &str) -> String {
    let re = match regex::Regex::new(r"\*\*(.+?)\*\*") {
        Ok(r) => r,
        Err(_) => return text.to_string(),
    };
    re.replace_all(text, "$1").to_string()
}

fn convert_italic(text: &str) -> String {
    let re = match regex::Regex::new(r"\*([^*]+?)\*") {
        Ok(r) => r,
        Err(_) => return text.to_string(),
    };
    let result = re.replace_all(text, "$1");

    let re = match regex::Regex::new(r"_(.+?)_") {
        Ok(r) => r,
        Err(_) => return result.to_string(),
    };
    re.replace_all(&result, "$1").to_string()
}

fn convert_inline_code(text: &str) -> String {
    let re = match regex::Regex::new(r"`(.+?)`") {
        Ok(r) => r,
        Err(_) => return text.to_string(),
    };
    re.replace_all(text, "$1").to_string()
}

fn convert_code_blocks(text: &str) -> String {
    let re = match regex::Regex::new(r"```.*?\n([\s\S]*?)```") {
        Ok(r) => r,
        Err(_) => return text.to_string(),
    };
    re.replace_all(text, "$1").to_string()
}

fn convert_link(text: &str) -> String {
    let re = match regex::Regex::new(r"\[([^\]]+)\]\(([^\)]+)\)") {
        Ok(r) => r,
        Err(_) => return text.to_string(),
    };
    re.replace_all(text, "$1: $2").to_string()
}

fn convert_list(text: &str) -> String {
    let mut result = text.to_string();

    // Only replace at start of line to avoid breaking inline formatting
    // like *italic* where the closing * is followed by a space
    let re = match regex::Regex::new(r"(?m)^- ") {
        Ok(r) => r,
        Err(_) => return result,
    };
    result = re.replace_all(&result, "• ").to_string();

    let re = match regex::Regex::new(r"(?m)^\* ") {
        Ok(r) => r,
        Err(_) => return result,
    };
    result = re.replace_all(&result, "• ").to_string();

    let re = match regex::Regex::new(r"(?m)^\d+\.\s") {
        Ok(r) => r,
        Err(_) => return result,
    };
    re.replace_all(&result, "• ").to_string()
}

/// `MAX_MESSAGE_LENGTH` is WeChat's limit and is counted in BYTES, so this
/// cannot use a char-based cap. The marker is appended past the limit
/// deliberately — the existing behaviour, preserved.
fn truncate(text: &str) -> String {
    if text.len() > MAX_MESSAGE_LENGTH {
        format!(
            "{}...(truncated)",
            crate::utils::text_format::truncate_bytes(text, MAX_MESSAGE_LENGTH)
        )
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
        assert_eq!(convert_bold("**hello** world **bold**"), "hello world bold");
    }

    #[test]
    fn test_convert_italic() {
        assert_eq!(convert_italic("*hello*"), "hello");
        assert_eq!(convert_italic("_hello_"), "hello");
    }

    #[test]
    fn test_convert_italic_does_not_break_lists() {
        let text = "* item 1\n* item 2";
        let after_list = convert_list(text);
        assert_eq!(convert_italic(&after_list), "• item 1\n• item 2");
    }

    #[test]
    fn test_convert_inline_code() {
        assert_eq!(convert_inline_code("`code`"), "code");
    }

    #[test]
    fn test_convert_code_blocks() {
        assert_eq!(convert_code_blocks("```\ncode block\n```"), "code block\n");
    }

    #[test]
    fn test_convert_link() {
        assert_eq!(
            convert_link("[text](https://example.com)"),
            "text: https://example.com"
        );
    }

    #[test]
    fn test_convert_list() {
        assert_eq!(convert_list("- item"), "• item");
        assert_eq!(convert_list("1. item"), "• item");
    }

    #[test]
    fn test_truncate() {
        let long_text = "a".repeat(5000);
        let result = truncate(&long_text);
        assert!(result.contains("truncated"));
        assert!(result.len() <= MAX_MESSAGE_LENGTH + 15);
    }

    #[test]
    fn test_markdown_to_wechat_full_conversion() {
        let md = "**bold** and *italic* and `code` and [link](http://a.com)";
        let result = markdown_to_wechat(md);
        assert_eq!(result, "bold and italic and code and link: http://a.com");
    }
}
