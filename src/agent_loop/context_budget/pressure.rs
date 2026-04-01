//! PressureSensor — token estimation with API usage anchoring.

use super::ContextPressure;
use crate::agent_loop::tool::ToolDefinition;
use crate::providers::message::UnifiedMessage;

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
// PressureSensor
// =============================================================================

/// Stores API-reported usage as an anchor point for subsequent estimation.
struct AnchoredUsage {
    /// Token count reported by the API at anchor time.
    input_tokens: usize,
    /// Number of messages in the conversation at anchor time.
    message_count_at_anchor: usize,
}

/// Measures context window pressure using API-anchored token counts
/// combined with content-aware estimation for new messages.
pub struct PressureSensor {
    anchor: Option<AnchoredUsage>,
    default_ratio: f64,
}

impl PressureSensor {
    /// Create a new sensor with a default characters-per-token ratio.
    pub fn new(default_ratio: f64) -> Self {
        Self {
            anchor: None,
            default_ratio,
        }
    }

    /// Store an API-reported token count as an anchor.
    ///
    /// Future `measure()` calls will use this anchor plus estimated delta
    /// for messages after the anchor point.
    pub fn update_anchor(&mut self, input_tokens: usize, message_count: usize) {
        self.anchor = Some(AnchoredUsage {
            input_tokens,
            message_count_at_anchor: message_count,
        });
    }

    /// Measure context pressure given the current message list, system prompt,
    /// tool definitions, and token budget.
    ///
    /// If an anchor is available, uses `anchor.input_tokens + estimate(delta_messages)`.
    /// Otherwise estimates all messages with content-aware ratio.
    pub fn measure(
        &self,
        messages: &[UnifiedMessage],
        system_prompt: &str,
        tool_defs: &[ToolDefinition],
        token_budget: u64,
    ) -> ContextPressure {
        let overhead = self.estimate_overhead(system_prompt, tool_defs);

        let used_tokens = if let Some(ref anchor) = self.anchor {
            // Use anchor for messages up to anchor point, estimate delta after
            let delta_messages = messages
                .get(anchor.message_count_at_anchor..)
                .unwrap_or(&[]);
            let delta_tokens = self.estimate_all_messages(delta_messages);
            anchor.input_tokens + delta_tokens
        } else {
            // No anchor — estimate everything from scratch including overhead
            let msg_tokens = self.estimate_all_messages(messages);
            overhead + msg_tokens
        };

        let budget = token_budget as usize;
        ContextPressure {
            used_tokens,
            budget_tokens: budget,
            ratio: if budget == 0 {
                1.0
            } else {
                used_tokens as f64 / budget as f64
            },
        }
    }

    /// Estimate tokens for a single string using the default ratio.
    fn estimate_str(&self, s: &str) -> usize {
        let chars = s.chars().count();
        ((chars as f64) / self.default_ratio).ceil() as usize
    }

    /// Estimate overhead tokens for system prompt and tool definitions.
    fn estimate_overhead(&self, system_prompt: &str, tool_defs: &[ToolDefinition]) -> usize {
        let prompt_tokens = self.estimate_str(system_prompt);
        let tool_tokens: usize = tool_defs
            .iter()
            .map(|td| {
                self.estimate_str(&td.name)
                    + self.estimate_str(&td.description)
                    + self.estimate_str(&td.parameters.to_string())
            })
            .sum();
        prompt_tokens + tool_tokens
    }

    /// Estimate total tokens for a slice of messages using content-aware ratio.
    fn estimate_all_messages(&self, messages: &[UnifiedMessage]) -> usize {
        messages
            .iter()
            .map(|m| estimate_tokens_smart(&m.text_content()))
            .sum()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::message::UnifiedMessage;

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

    #[test]
    fn sensor_without_anchor_estimates_from_scratch() {
        let sensor = PressureSensor::new(3.5);
        let msgs = vec![UnifiedMessage::user("Hello world")];
        let pressure = sensor.measure(&msgs, "system prompt", &[], 10_000);
        assert!(pressure.used_tokens > 0);
        assert!(pressure.ratio < 1.0);
    }

    #[test]
    fn sensor_with_anchor_uses_anchor_plus_delta() {
        let mut sensor = PressureSensor::new(3.5);
        sensor.update_anchor(5000, 2);
        let msgs = vec![
            UnifiedMessage::user("old msg 1"),
            UnifiedMessage::assistant("old msg 2"),
            UnifiedMessage::user("new msg after anchor"),
        ];
        let pressure = sensor.measure(&msgs, "system prompt", &[], 10_000);
        assert!(
            pressure.used_tokens > 5000,
            "should include anchor, got {}",
            pressure.used_tokens
        );
        assert!(
            pressure.used_tokens < 6000,
            "delta should be small, got {}",
            pressure.used_tokens
        );
    }

    #[test]
    fn sensor_anchor_update_replaces_previous() {
        let mut sensor = PressureSensor::new(3.5);
        sensor.update_anchor(1000, 1);
        sensor.update_anchor(5000, 3);
        let msgs = vec![
            UnifiedMessage::user("a"),
            UnifiedMessage::assistant("b"),
            UnifiedMessage::user("c"),
            UnifiedMessage::user("new"),
        ];
        let pressure = sensor.measure(&msgs, "", &[], 10_000);
        assert!(pressure.used_tokens > 5000);
    }
}
