//! Intent detection for dynamic agent switching.
//!
//! Detects user intent to switch agents via keyword patterns (fast path)
//! or an optional LLM classify function (slow path).

use once_cell::sync::Lazy;
use regex::Regex;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Detected intent from user message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectedIntent {
    /// User wants to switch to a specific agent.
    SwitchAgent {
        /// Agent identifier (may be empty when resolved later by LLM).
        id: String,
        /// Human-readable agent name extracted from the message.
        name: String,
    },
    /// No switching intent detected — treat as normal message.
    Normal,
}

/// Async classify function provided by the LLM layer.
///
/// Given a user message, returns `Some(DetectedIntent)` if the LLM
/// confidently detects switching intent, or `None` to fall through.
pub type IntentClassifyFn =
    Arc<dyn Fn(&str) -> Pin<Box<dyn Future<Output = Option<DetectedIntent>> + Send>> + Send + Sync>;

// ---------------------------------------------------------------------------
// Regex patterns
// ---------------------------------------------------------------------------

/// Chinese switch patterns: "切换到X模式" / "换成X助手" / "切换为X" / "使用X"
static RE_CN_SWITCH: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^(?:切换到|换成|切换为|使用)(.+?)(?:模式|agent)?$").unwrap()
});

/// Chinese "I want to talk with X" patterns: "我想和X聊" / "我想跟X说" / "我想找X咨询"
static RE_CN_WANT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^我想(?:和|跟|找)(.+?)(?:聊|说|谈|咨询)").unwrap()
});

/// English: "switch to X mode" / "change to X agent" / "use X assistant"
static RE_EN_SWITCH: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^(?:switch to|change to|use) (.+?)(?:\s+(?:mode|agent))?$").unwrap()
});

// ---------------------------------------------------------------------------
// IntentDetector
// ---------------------------------------------------------------------------

/// Detects agent-switching intent from user messages.
///
/// Detection pipeline:
/// 1. Keyword regex matching (fast, no I/O)
/// 2. Optional LLM classify function (slow, async)
/// 3. Fall through to `Normal`
pub struct IntentDetector {
    llm_classify_fn: Option<IntentClassifyFn>,
}

impl IntentDetector {
    /// Create a new detector with keyword matching only.
    pub fn new() -> Self {
        Self {
            llm_classify_fn: None,
        }
    }

    /// Attach an LLM classify function for ambiguous messages.
    pub fn with_llm_classify(mut self, f: IntentClassifyFn) -> Self {
        self.llm_classify_fn = Some(f);
        self
    }

    /// Detect intent from a user message.
    ///
    /// Tries keyword matching first, then falls back to the LLM classifier
    /// if one is configured.
    pub async fn detect(&self, text: &str) -> DetectedIntent {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return DetectedIntent::Normal;
        }

        // Fast path: keyword regex
        if let Some(intent) = Self::keyword_match(trimmed) {
            info!(name = %intent_name(&intent), "intent detected via keyword match");
            return intent;
        }

        // Slow path: LLM classify
        if let Some(ref classify) = self.llm_classify_fn {
            debug!("falling back to LLM intent classification");
            if let Some(intent) = classify(trimmed).await {
                info!(name = %intent_name(&intent), "intent detected via LLM classify");
                return intent;
            }
        }

        DetectedIntent::Normal
    }

    /// Try to match the message against keyword regex patterns.
    ///
    /// Returns `Some(DetectedIntent::SwitchAgent)` on match, `None` otherwise.
    pub fn keyword_match(text: &str) -> Option<DetectedIntent> {
        // Chinese patterns — id left empty for later LLM resolution
        if let Some(caps) = RE_CN_SWITCH.captures(text) {
            let name = caps[1].trim().to_string();
            return Some(DetectedIntent::SwitchAgent {
                id: String::new(),
                name,
            });
        }
        if let Some(caps) = RE_CN_WANT.captures(text) {
            let name = caps[1].trim().to_string();
            return Some(DetectedIntent::SwitchAgent {
                id: String::new(),
                name,
            });
        }

        // English pattern — derive id from name
        if let Some(caps) = RE_EN_SWITCH.captures(text) {
            let name = caps[1].trim().to_string();
            let id = name.to_lowercase().replace(' ', "_");
            return Some(DetectedIntent::SwitchAgent { id, name });
        }

        None
    }
}

impl Default for IntentDetector {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// LLM prompt builders & response parser
// ---------------------------------------------------------------------------

/// Build prompt for LLM intent classification (keyword miss fallback)
pub fn build_intent_classify_prompt(message: &str) -> String {
    format!(
        r#"Classify this message. If the user wants to switch to a different AI agent/persona, return JSON:
{{"intent":"switch","id":"english_snake_case","name":"display name"}}
Otherwise return:
{{"intent":"normal"}}

Rules for id: lowercase English, use underscores, short (1-2 words). Examples:
- "交易助手" -> id: "trading"
- "健康顾问" -> id: "health"
- "coding expert" -> id: "coding"

Message: {}"#,
        message
    )
}

/// Build prompt to resolve an English id from a display name
pub fn build_id_resolve_prompt(name: &str) -> String {
    format!(
        r#"Given this AI agent name, return ONLY a short English snake_case id (no quotes, no explanation).
Examples: "交易助手" -> trading, "健康顾问" -> health, "Code Expert" -> coding, "主助手" -> main
Name: {}"#,
        name
    )
}

/// Build prompt to generate SOUL.md content for a new agent
pub fn build_soul_generation_prompt(id: &str, name: &str) -> String {
    format!(
        r#"Generate a concise AI persona description for an agent named "{name}" (id: {id}).
Write 3-5 sentences describing this agent's expertise, communication style, and personality.
Write in the same language as the name. Be specific to the domain.
Output ONLY the persona description, no headers or markdown formatting."#
    )
}

/// Parse LLM response for intent classification
pub fn parse_intent_response(response: &str) -> Option<DetectedIntent> {
    let text = response.trim();
    let start = text.find('{')?;
    let end = text.rfind('}')? + 1;
    let json_str = text.get(start..end)?;

    let value: serde_json::Value = serde_json::from_str(json_str).ok()?;

    match value.get("intent")?.as_str()? {
        "switch" => {
            let id = value.get("id")?.as_str()?.to_string();
            let name = value.get("name")?.as_str()?.to_string();
            if id.is_empty() || name.is_empty() {
                return None;
            }
            Some(DetectedIntent::SwitchAgent { id, name })
        }
        "normal" => Some(DetectedIntent::Normal),
        _ => None,
    }
}

/// Helper to extract the name for logging.
fn intent_name(intent: &DetectedIntent) -> &str {
    match intent {
        DetectedIntent::SwitchAgent { name, .. } => name.as_str(),
        DetectedIntent::Normal => "normal",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Chinese keyword: switch to / change to / use (切换到/换成/切换为/使用) --

    #[test]
    fn cn_switch_to_agent() {
        let result = IntentDetector::keyword_match("切换到编程助手");
        assert_eq!(
            result,
            Some(DetectedIntent::SwitchAgent {
                id: String::new(),
                name: "编程助手".into(),
            })
        );
    }

    #[test]
    fn cn_change_to_mode() {
        let result = IntentDetector::keyword_match("换成翻译模式");
        assert_eq!(
            result,
            Some(DetectedIntent::SwitchAgent {
                id: String::new(),
                name: "翻译".into(),
            })
        );
    }

    #[test]
    fn cn_switch_as() {
        let result = IntentDetector::keyword_match("切换为写作agent");
        assert_eq!(
            result,
            Some(DetectedIntent::SwitchAgent {
                id: String::new(),
                name: "写作".into(),
            })
        );
    }

    #[test]
    fn cn_use_agent() {
        let result = IntentDetector::keyword_match("使用数据分析");
        assert_eq!(
            result,
            Some(DetectedIntent::SwitchAgent {
                id: String::new(),
                name: "数据分析".into(),
            })
        );
    }

    // -- Chinese keyword: I want to chat/talk/consult with X (我想和/跟/找 X 聊/说/谈/咨询) --

    #[test]
    fn cn_want_to_chat() {
        let result = IntentDetector::keyword_match("我想和法律顾问聊");
        assert_eq!(
            result,
            Some(DetectedIntent::SwitchAgent {
                id: String::new(),
                name: "法律顾问".into(),
            })
        );
    }

    #[test]
    fn cn_want_to_consult() {
        let result = IntentDetector::keyword_match("我想找医生咨询");
        assert_eq!(
            result,
            Some(DetectedIntent::SwitchAgent {
                id: String::new(),
                name: "医生".into(),
            })
        );
    }

    #[test]
    fn cn_want_to_talk() {
        let result = IntentDetector::keyword_match("我想跟导师谈");
        assert_eq!(
            result,
            Some(DetectedIntent::SwitchAgent {
                id: String::new(),
                name: "导师".into(),
            })
        );
    }

    // -- English keyword patterns --

    #[test]
    fn en_switch_to() {
        let result = IntentDetector::keyword_match("switch to coding assistant");
        assert_eq!(
            result,
            Some(DetectedIntent::SwitchAgent {
                id: "coding_assistant".into(),
                name: "coding assistant".into(),
            })
        );
    }

    #[test]
    fn en_change_to_mode() {
        let result = IntentDetector::keyword_match("change to writer mode");
        assert_eq!(
            result,
            Some(DetectedIntent::SwitchAgent {
                id: "writer".into(),
                name: "writer".into(),
            })
        );
    }

    #[test]
    fn en_use_agent() {
        let result = IntentDetector::keyword_match("Use Data Analyst agent");
        assert_eq!(
            result,
            Some(DetectedIntent::SwitchAgent {
                id: "data_analyst".into(),
                name: "Data Analyst".into(),
            })
        );
    }

    #[test]
    fn en_case_insensitive() {
        let result = IntentDetector::keyword_match("SWITCH TO helper");
        assert_eq!(
            result,
            Some(DetectedIntent::SwitchAgent {
                id: "helper".into(),
                name: "helper".into(),
            })
        );
    }

    // -- No match --

    #[test]
    fn no_match_normal_text() {
        assert_eq!(IntentDetector::keyword_match("今天天气怎么样"), None);
    }

    #[test]
    fn no_match_english_normal() {
        assert_eq!(
            IntentDetector::keyword_match("What is the weather today?"),
            None
        );
    }

    #[test]
    fn no_match_empty() {
        assert_eq!(IntentDetector::keyword_match(""), None);
    }

    // -- detect() async tests --

    #[tokio::test]
    async fn detect_keyword_hit() {
        let detector = IntentDetector::new();
        let result = detector.detect("switch to coder").await;
        assert_eq!(
            result,
            DetectedIntent::SwitchAgent {
                id: "coder".into(),
                name: "coder".into(),
            }
        );
    }

    #[tokio::test]
    async fn detect_normal_without_llm() {
        let detector = IntentDetector::new();
        let result = detector.detect("hello world").await;
        assert_eq!(result, DetectedIntent::Normal);
    }

    #[tokio::test]
    async fn detect_llm_fallback() {
        let classify: IntentClassifyFn = Arc::new(|_text: &str| {
            Box::pin(async {
                Some(DetectedIntent::SwitchAgent {
                    id: "from_llm".into(),
                    name: "LLM Agent".into(),
                })
            })
        });
        let detector = IntentDetector::new().with_llm_classify(classify);
        // "hello" doesn't match keywords, so LLM classify kicks in
        let result = detector.detect("hello").await;
        assert_eq!(
            result,
            DetectedIntent::SwitchAgent {
                id: "from_llm".into(),
                name: "LLM Agent".into(),
            }
        );
    }

    #[tokio::test]
    async fn detect_keyword_takes_priority_over_llm() {
        let classify: IntentClassifyFn = Arc::new(|_text: &str| {
            Box::pin(async {
                Some(DetectedIntent::SwitchAgent {
                    id: "from_llm".into(),
                    name: "LLM".into(),
                })
            })
        });
        let detector = IntentDetector::new().with_llm_classify(classify);
        // Keyword match should win
        let result = detector.detect("switch to coder").await;
        assert_eq!(
            result,
            DetectedIntent::SwitchAgent {
                id: "coder".into(),
                name: "coder".into(),
            }
        );
    }

    #[tokio::test]
    async fn detect_empty_is_normal() {
        let detector = IntentDetector::new();
        assert_eq!(detector.detect("").await, DetectedIntent::Normal);
        assert_eq!(detector.detect("   ").await, DetectedIntent::Normal);
    }

    // -- LLM prompt builders & parser --

    #[test]
    fn parse_intent_switch() {
        let resp = r#"{"intent":"switch","id":"trading","name":"交易助手"}"#;
        assert_eq!(
            parse_intent_response(resp),
            Some(DetectedIntent::SwitchAgent {
                id: "trading".into(),
                name: "交易助手".into(),
            })
        );
    }

    #[test]
    fn parse_intent_normal() {
        let resp = r#"{"intent":"normal"}"#;
        assert_eq!(parse_intent_response(resp), Some(DetectedIntent::Normal));
    }

    #[test]
    fn parse_intent_with_surrounding_text() {
        let resp = r#"Based on analysis: {"intent":"switch","id":"health","name":"健康顾问"} end."#;
        assert!(matches!(
            parse_intent_response(resp),
            Some(DetectedIntent::SwitchAgent { .. })
        ));
    }

    #[test]
    fn parse_intent_invalid() {
        assert_eq!(parse_intent_response("not json"), None);
        assert_eq!(parse_intent_response(r#"{"intent":"unknown"}"#), None);
    }

    #[test]
    fn build_prompts_not_empty() {
        assert!(!build_intent_classify_prompt("hello").is_empty());
        assert!(!build_id_resolve_prompt("交易助手").is_empty());
        assert!(!build_soul_generation_prompt("trading", "交易助手").is_empty());
    }
}
