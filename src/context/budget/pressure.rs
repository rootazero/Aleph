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

/// Default chars-per-token ratio for English prose. Used as the prose anchor by
/// [`detect_content_ratio`] and as the implicit baseline for [`estimate_tokens_smart`].
pub const DEFAULT_PROSE_RATIO: f64 = 3.5;
/// Chars-per-token ratio for CJK-dominant text (denser per character).
const CJK_RATIO: f64 = 1.5;
/// Chars-per-token ratio for source code (symbol-dense, denser than prose).
const CODE_RATIO: f64 = 2.5;

/// Content-aware chars-per-token ratio, using `prose_ratio` as the baseline for
/// ordinary (non-CJK, non-code) text.
///
/// CJK-dominant and code-like content override the baseline with their denser
/// ratios — these are the two cases a single flat ratio gets catastrophically
/// wrong (a fixed `3.5` under-counts CJK ~2.3× and code ~1.4×). Keeping the
/// caller's `prose_ratio` as the anchor means a configured estimate ratio still
/// governs prose, so wiring this into the budget sensor stays backward-compatible
/// for English conversations while fixing the CJK/code blind spots.
pub fn content_ratio_with_baseline(text: &str, prose_ratio: f64) -> f64 {
    if text.is_empty() {
        return prose_ratio;
    }

    // Check CJK character ratio
    let total_chars = text.chars().count();
    let cjk_count = text.chars().filter(|&c| is_cjk(c)).count();
    let cjk_ratio = cjk_count as f64 / total_chars as f64;

    if cjk_ratio > 0.30 {
        return CJK_RATIO;
    }

    // Check if content looks like code
    if looks_like_code(text) {
        return CODE_RATIO;
    }

    prose_ratio
}

/// Detects the content type and returns an appropriate chars-per-token ratio.
///
/// - Returns `1.5` if more than 30% of characters are CJK (fewer chars per token).
/// - Returns `2.5` if content looks like code.
/// - Returns `3.5` for default English prose.
///
/// This is [`content_ratio_with_baseline`] anchored at [`DEFAULT_PROSE_RATIO`].
pub fn detect_content_ratio(text: &str) -> f64 {
    content_ratio_with_baseline(text, DEFAULT_PROSE_RATIO)
}

/// Estimates token count using content-aware ratio detection.
pub fn estimate_tokens_smart(content: &str) -> usize {
    estimate_tokens_aware(content, DEFAULT_PROSE_RATIO)
}

/// Estimates token count using content-aware ratio detection with an explicit
/// prose baseline.
///
/// Like [`estimate_tokens_smart`] but lets the caller supply the prose
/// chars-per-token anchor (e.g. a configured `token_estimate_ratio`), while CJK
/// and code content still override with their denser ratios. Counts Unicode
/// scalar values, not UTF-8 bytes, so CJK text is not over-counted ~3×.
pub fn estimate_tokens_aware(content: &str, prose_ratio: f64) -> usize {
    let ratio = content_ratio_with_baseline(content, prose_ratio);
    if ratio <= 0.0 {
        return 0;
    }
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

    // --- content_ratio_with_baseline ---

    #[test]
    fn baseline_governs_prose_only() {
        // Prose honors the caller's anchor instead of the hardcoded 3.5.
        assert!(
            (content_ratio_with_baseline("just some plain english prose", 4.0) - 4.0).abs() < 0.01
        );
        // Empty text falls back to the supplied baseline, not 3.5.
        assert!((content_ratio_with_baseline("", 4.0) - 4.0).abs() < 0.01);
    }

    #[test]
    fn baseline_overridden_by_cjk_and_code() {
        // CJK and code override the baseline with their denser ratios regardless
        // of the caller's prose anchor.
        assert!(
            (content_ratio_with_baseline("这是一段中文文本用于测试比率检测", 4.0) - 1.5).abs()
                < 0.01
        );
        let code = "fn main() {\n    let x = vec![1, 2, 3];\n    println!(\"{:?}\", x);\n}";
        assert!((content_ratio_with_baseline(code, 4.0) - 2.5).abs() < 0.01);
    }

    #[test]
    fn detect_content_ratio_is_baseline_at_default() {
        // detect_content_ratio must remain exactly content_ratio_with_baseline @ 3.5.
        for s in [
            "Hello world",
            "这是中文文本测试内容比较多一些字符",
            "fn x() -> i32 { let y = 1; y + 2 }",
            "",
        ] {
            assert_eq!(
                detect_content_ratio(s),
                content_ratio_with_baseline(s, DEFAULT_PROSE_RATIO)
            );
        }
    }

    // --- estimate_tokens_aware ---

    #[test]
    fn estimate_tokens_smart_is_aware_at_default() {
        // estimate_tokens_smart must remain exactly estimate_tokens_aware @ 3.5.
        for s in ["Hello world", "这是中文", "fn x() {}", ""] {
            assert_eq!(
                estimate_tokens_smart(s),
                estimate_tokens_aware(s, DEFAULT_PROSE_RATIO)
            );
        }
    }

    #[test]
    fn aware_estimate_exceeds_flat_for_cjk() {
        // The core fix: a CJK message under a 3.5 prose anchor must estimate far
        // MORE tokens than a flat 3.5 ratio would — otherwise the budget sensor
        // under-counts CJK ~2.3× and overflows before compaction triggers.
        let cjk = "这是一段中文文本用于测试token估算的比率检测逻辑是否正确工作";
        let chars = cjk.chars().count();
        let flat = (chars as f64 / 3.5).ceil() as usize; // what the old sensor did
        let aware = estimate_tokens_aware(cjk, 3.5);
        assert!(
            aware > flat,
            "content-aware ({aware}) must exceed flat-3.5 ({flat}) for CJK"
        );
        // CJK ratio is 1.5, so aware ≈ chars/1.5.
        assert_eq!(aware, (chars as f64 / 1.5).ceil() as usize);
    }

    #[test]
    fn aware_estimate_zero_ratio_guard() {
        assert_eq!(estimate_tokens_aware("hello", 0.0), 0);
    }
}
