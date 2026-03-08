//! Fast-path abort detector using exact matching against multilingual stop words.
//!
//! Checks whether the entire user message (after normalization) is a stop/abort
//! trigger word in any supported language. No substring matching.

use std::collections::HashSet;

/// Detects abort/stop intent via exact match against known trigger words.
pub struct AbortDetector {
    triggers: HashSet<String>,
}

impl AbortDetector {
    /// Create a new `AbortDetector` populated with multilingual stop words.
    pub fn new() -> Self {
        let words = [
            // English
            "stop", "abort", "halt", "cancel", "quit",
            // Chinese
            "停", "停止", "取消", "中止",
            // Japanese (中止 overlaps with Chinese)
            "やめて", "止めて",
            // Korean
            "중지", "멈춰",
            // Russian
            "стоп", "остановись",
            // German
            "stopp", "anhalten",
            // French (with and without accent)
            "arrête", "arrete",
            // Spanish
            "para", "detente",
            // Portuguese
            "pare",
            // Arabic
            "توقف",
            // Hindi
            "रुको",
        ];

        let triggers: HashSet<String> = words.iter().map(|w| w.to_string()).collect();
        Self { triggers }
    }

    /// Check if the input is an abort trigger.
    ///
    /// Returns `true` only if the entire normalized input exactly matches
    /// a known stop word. Substring matches return `false`.
    pub fn is_abort(&self, input: &str) -> bool {
        let normalized = Self::normalize(input);
        if normalized.is_empty() {
            return false;
        }
        self.triggers.contains(&normalized)
    }

    /// Normalize input by trimming whitespace, stripping trailing punctuation,
    /// and lowercasing.
    fn normalize(input: &str) -> String {
        let trimmed = input.trim();
        let stripped = trimmed.trim_end_matches(|c: char| {
            matches!(
                c,
                '。' | '.'
                    | '!'
                    | '！'
                    | '?'
                    | '？'
                    | '…'
                    | '，'
                    | ','
                    | ';'
                    | '；'
                    | ':'
                    | '：'
                    | '\''
                    | '"'
                    | '\u{2018}' // '
                    | '\u{2019}' // '
                    | '\u{201C}' // "
                    | '\u{201D}' // "
                    | '）'
                    | ')'
                    | ']'
                    | '}'
                    | '>'
            )
        });
        stripped.to_lowercase()
    }
}

impl Default for AbortDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abort_english_basic() {
        let d = AbortDetector::new();
        assert!(d.is_abort("stop"));
        assert!(d.is_abort("abort"));
        assert!(d.is_abort("halt"));
        assert!(d.is_abort("cancel"));
        assert!(d.is_abort("quit"));
    }

    #[test]
    fn abort_chinese() {
        let d = AbortDetector::new();
        assert!(d.is_abort("停止"));
        assert!(d.is_abort("取消"));
        assert!(d.is_abort("中止"));
        assert!(d.is_abort("停"));
    }

    #[test]
    fn abort_japanese() {
        let d = AbortDetector::new();
        assert!(d.is_abort("やめて"));
        assert!(d.is_abort("止めて"));
    }

    #[test]
    fn abort_korean() {
        let d = AbortDetector::new();
        assert!(d.is_abort("중지"));
        assert!(d.is_abort("멈춰"));
    }

    #[test]
    fn abort_other_languages() {
        let d = AbortDetector::new();
        // Russian
        assert!(d.is_abort("стоп"));
        // German
        assert!(d.is_abort("stopp"));
        // French
        assert!(d.is_abort("arrête"));
        // Portuguese
        assert!(d.is_abort("pare"));
        // Arabic
        assert!(d.is_abort("توقف"));
        // Hindi
        assert!(d.is_abort("रुको"));
    }

    #[test]
    fn abort_case_insensitive() {
        let d = AbortDetector::new();
        assert!(d.is_abort("STOP"));
        assert!(d.is_abort("Stop"));
        assert!(d.is_abort("ABORT"));
    }

    #[test]
    fn abort_with_trailing_punctuation() {
        let d = AbortDetector::new();
        assert!(d.is_abort("stop!"));
        assert!(d.is_abort("stop!!!"));
        assert!(d.is_abort("stop。"));
        assert!(d.is_abort("停止！"));
        assert!(d.is_abort("cancel..."));
    }

    #[test]
    fn abort_with_whitespace() {
        let d = AbortDetector::new();
        assert!(d.is_abort("  stop  "));
        assert!(d.is_abort("\tstop\n"));
    }

    #[test]
    fn abort_no_substring_match() {
        let d = AbortDetector::new();
        assert!(!d.is_abort("don't stop the music"));
        assert!(!d.is_abort("stop the world I want to get off"));
        assert!(!d.is_abort("bus stop"));
        assert!(!d.is_abort("nonstop flight"));
    }

    #[test]
    fn abort_empty_and_short() {
        let d = AbortDetector::new();
        assert!(!d.is_abort(""));
        assert!(!d.is_abort("   "));
        assert!(!d.is_abort("hi"));
    }

    #[test]
    fn abort_not_normal_text() {
        let d = AbortDetector::new();
        assert!(!d.is_abort("please help me"));
        assert!(!d.is_abort("what is the weather"));
        assert!(!d.is_abort("整理我的文件"));
    }
}
