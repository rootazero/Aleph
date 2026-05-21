//! Content-aware token-ratio detection for context-pressure estimation.

// =============================================================================
// Content-aware ratio detection
// =============================================================================

/// Returns true if the character is in a CJK Unicode range.
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{4E00}'..='\u{9FFF}'   // CJK Unified Ideographs
        | '\u{3400}'..='\u{4DBF}' // CJK Extension A
        | '\u{20000}'..='\u{2A6DF}' // CJK Extension B
        | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
        | '\u{2F800}'..='\u{2FA1F}' // CJK Compatibility Supplement
        | '\u{3000}'..='\u{303F}' // CJK Symbols and Punctuation
        | '\u{FF00}'..='\u{FFEF}' // Halfwidth/Fullwidth Forms
        | '\u{3040}'..='\u{309F}' // Hiragana
        | '\u{30A0}'..='\u{30FF}' // Katakana
        | '\u{AC00}'..='\u{D7AF}' // Hangul Syllables
    )
}

/// Heuristic code detection.
///
/// Samples the first 20 lines, checks for language keywords/operators,
/// and returns true if more than 40% of lines look like code.
fn looks_like_code(text: &str) -> bool {
    let code_indicators = [
        "fn ",
        "let ",
        "mut ",
        "pub ",
        "impl ",
        "struct ",
        "enum ",
        "trait ", // Rust
        "def ",
        "class ",
        "import ",
        "from ",
        "return ",
        "if ",
        "else:",
        "for ", // Python
        "function ",
        "const ",
        "var ",
        "=>",
        "===",
        "!==", // JS/TS
        "int ",
        "void ",
        "return;",
        "#include",
        "using namespace", // C/C++
        "func ",
        "package ",
        "interface ",
        "go ", // Go
        "{",
        "}",
        "//",
        "/*",
        "*/", // Common
        "->",
        "::",
        "&&",
        "||",
        "!=",
        "==",
        "<=",
        ">=", // Operators
    ];

    let lines: Vec<&str> = text.lines().take(20).collect();
    if lines.is_empty() {
        return false;
    }

    let indicator_count = lines
        .iter()
        .filter(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return false;
            }
            code_indicators
                .iter()
                .any(|indicator| trimmed.contains(indicator))
        })
        .count();

    let ratio = indicator_count as f64 / lines.len() as f64;
    ratio > 0.40
}

/// Detects the content type and returns an appropriate chars-per-token ratio.
///
/// - Returns `1.5` if more than 30% of characters are CJK (fewer chars per token).
/// - Returns `2.5` if content looks like code.
/// - Returns `3.5` for default English prose.
pub fn detect_content_ratio(text: &str) -> f64 {
    if text.is_empty() {
        return 3.5;
    }

    // Check CJK character ratio
    let total_chars = text.chars().count();
    let cjk_count = text.chars().filter(|&c| is_cjk(c)).count();
    let cjk_ratio = cjk_count as f64 / total_chars as f64;

    if cjk_ratio > 0.30 {
        return 1.5;
    }

    // Check if content looks like code
    if looks_like_code(text) {
        return 2.5;
    }

    3.5
}

/// Estimates token count using content-aware ratio detection.
pub fn estimate_tokens_smart(content: &str) -> usize {
    let ratio = detect_content_ratio(content);
    // ratio = chars per token, so tokens = chars / ratio
    let chars = content.chars().count();
    ((chars as f64) / ratio).ceil() as usize
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_ratio_pure_english() {
        let ratio = detect_content_ratio("Hello world, this is a test message.");
        assert!((ratio - 3.5).abs() < 0.01);
    }

    #[test]
    fn detect_ratio_chinese_text() {
        let ratio = detect_content_ratio("这是一段中文文本，用于测试token估算比率。");
        assert!((ratio - 1.5).abs() < 0.01);
    }

    #[test]
    fn detect_ratio_code_content() {
        let code = "fn main() {\n    let x = vec![1, 2, 3];\n    println!(\"{:?}\", x);\n}";
        let ratio = detect_content_ratio(code);
        assert!((ratio - 2.5).abs() < 0.01);
    }

    #[test]
    fn detect_ratio_empty_string() {
        let ratio = detect_content_ratio("");
        assert!((ratio - 3.5).abs() < 0.01);
    }

    #[test]
    fn detect_ratio_mixed_content() {
        // ~40% CJK should trigger CJK ratio
        let mixed = "Hello world 这是中文 this is mixed 混合内容测试文本比较多";
        let ratio = detect_content_ratio(mixed);
        assert!(
            ratio < 3.5,
            "mixed with >30% CJK should use lower ratio, got {ratio}"
        );
    }
}
