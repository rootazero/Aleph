//! Intent detection for dynamic agent switching.
//!
//! Uses LLM semantic understanding to detect agent-switch intent,
//! extract the target agent name, and separate any accompanying task.

use crate::sync_primitives::Arc;
use std::future::Future;
use std::pin::Pin;
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Detected intent from user message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectedIntent {
    /// User wants to switch to a specific agent, optionally with a task.
    SwitchAgent {
        /// Agent identifier (may be empty when resolved later by LLM).
        id: String,
        /// Human-readable agent name extracted from the message.
        name: String,
        /// Task to execute after switching (e.g. "写一份会议纪要").
        /// When present, the router should forward this to the new agent.
        task: Option<String>,
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
// IntentDetector
// ---------------------------------------------------------------------------

/// Detects agent-switching intent from user messages using LLM.
///
/// When an LLM provider is available, all intent detection is done via
/// semantic understanding — no brittle regex patterns.
pub struct IntentDetector {
    /// LLM provider for semantic intent classification
    llm_provider: Option<Arc<dyn crate::providers::AiProvider>>,
    /// Available agent names/ids for context in the LLM prompt
    available_agents: Vec<AgentInfo>,
}

/// Minimal agent info for intent detection context.
#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
}

impl IntentDetector {
    /// Create a new detector (no LLM — all messages treated as Normal).
    pub fn new() -> Self {
        Self {
            llm_provider: None,
            available_agents: Vec::new(),
        }
    }

    /// Set the LLM provider for semantic intent detection.
    pub fn with_llm_provider(mut self, provider: Arc<dyn crate::providers::AiProvider>) -> Self {
        self.llm_provider = Some(provider);
        self
    }

    /// Set the available agents for context.
    pub fn with_available_agents(mut self, agents: Vec<AgentInfo>) -> Self {
        self.available_agents = agents;
        self
    }

    /// Update available agents dynamically.
    pub fn set_available_agents(&mut self, agents: Vec<AgentInfo>) {
        self.available_agents = agents;
    }

    /// Attach an LLM classify function (legacy API, for backward compatibility).
    pub fn with_llm_classify(self, _f: IntentClassifyFn) -> Self {
        // No-op: LLM provider is used directly now
        self
    }

    /// Detect intent from a user message using LLM semantic understanding.
    pub async fn detect(&self, text: &str) -> DetectedIntent {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return DetectedIntent::Normal;
        }

        // Use LLM for semantic understanding
        if let Some(ref provider) = self.llm_provider {
            let prompt = build_intent_classify_prompt(trimmed, &self.available_agents);
            debug!(prompt_len = prompt.len(), "LLM intent classification");

            match provider.process(&prompt, None).await {
                Ok(response) => {
                    if let Some(intent) = parse_intent_response(&response) {
                        info!(name = %intent_name(&intent), "intent detected via LLM");
                        return intent;
                    }
                    debug!("LLM returned unparseable response, treating as Normal");
                }
                Err(e) => {
                    warn!(error = %e, "LLM intent classification failed, treating as Normal");
                }
            }
        }

        DetectedIntent::Normal
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

/// Build prompt for LLM intent classification with available agent context.
pub fn build_intent_classify_prompt(message: &str, agents: &[AgentInfo]) -> String {
    let agent_list = if agents.is_empty() {
        "No specific agents registered.".to_string()
    } else {
        agents
            .iter()
            .map(|a| format!("- id: \"{}\", name: \"{}\"", a.id, a.name))
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        r#"You are an intent classifier. Analyze the user message and determine if they want to switch to a different AI agent/persona.

Available agents:
{agent_list}

Rules:
1. If the user wants to switch agent AND also has a task, extract BOTH
2. The "id" must match one of the available agent ids above, or be a reasonable snake_case id
3. The "task" is the actual work the user wants done (NOT the switch command itself)
4. If there is NO switch intent, return {{"intent":"normal"}}

Return ONLY valid JSON, no other text.

Examples:
- "使用cowork agent写一份报告" → {{"intent":"switch","id":"cowork","name":"cowork","task":"写一份报告"}}
- "切换到main" → {{"intent":"switch","id":"main","name":"main"}}
- "今天天气怎么样" → {{"intent":"normal"}}
- "switch to coding and fix the bug" → {{"intent":"switch","id":"coding","name":"coding","task":"fix the bug"}}
- "帮我翻译这段话" → {{"intent":"normal"}}

Message: {message}"#
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

/// Parse LLM response for intent classification.
///
/// Expects JSON with fields: intent, id, name, and optionally task.
pub fn parse_intent_response(response: &str) -> Option<DetectedIntent> {
    let text = response.trim();
    let start = text.find('{')?;
    let end = text.rfind('}')? + 1;
    let json_str = text.get(start..end)?;

    let value: serde_json::Value = serde_json::from_str(json_str).ok()?;

    match value.get("intent")?.as_str()? {
        "switch" => {
            let id = value.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let name = value.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if name.is_empty() && id.is_empty() {
                return None;
            }
            let task = value
                .get("task")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let display_name = if name.is_empty() { id.clone() } else { name };
            Some(DetectedIntent::SwitchAgent {
                id,
                name: display_name,
                task,
            })
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

    // -- LLM response parser tests --

    #[test]
    fn parse_switch_with_task() {
        let resp = r#"{"intent":"switch","id":"cowork","name":"cowork","task":"写一份会议纪要"}"#;
        assert_eq!(
            parse_intent_response(resp),
            Some(DetectedIntent::SwitchAgent {
                id: "cowork".into(),
                name: "cowork".into(),
                task: Some("写一份会议纪要".into()),
            })
        );
    }

    #[test]
    fn parse_switch_without_task() {
        let resp = r#"{"intent":"switch","id":"main","name":"main"}"#;
        assert_eq!(
            parse_intent_response(resp),
            Some(DetectedIntent::SwitchAgent {
                id: "main".into(),
                name: "main".into(),
                task: None,
            })
        );
    }

    #[test]
    fn parse_normal() {
        let resp = r#"{"intent":"normal"}"#;
        assert_eq!(parse_intent_response(resp), Some(DetectedIntent::Normal));
    }

    #[test]
    fn parse_with_surrounding_text() {
        let resp = r#"Based on analysis: {"intent":"switch","id":"health","name":"健康顾问"} end."#;
        assert!(matches!(
            parse_intent_response(resp),
            Some(DetectedIntent::SwitchAgent { .. })
        ));
    }

    #[test]
    fn parse_invalid() {
        assert_eq!(parse_intent_response("not json"), None);
        assert_eq!(parse_intent_response(r#"{"intent":"unknown"}"#), None);
    }

    #[test]
    fn parse_empty_name_and_id() {
        let resp = r#"{"intent":"switch","id":"","name":""}"#;
        assert_eq!(parse_intent_response(resp), None);
    }

    #[test]
    fn parse_task_with_empty_string() {
        let resp = r#"{"intent":"switch","id":"main","name":"main","task":""}"#;
        assert_eq!(
            parse_intent_response(resp),
            Some(DetectedIntent::SwitchAgent {
                id: "main".into(),
                name: "main".into(),
                task: None,
            })
        );
    }

    // -- detect() async tests --

    #[tokio::test]
    async fn detect_empty_is_normal() {
        let detector = IntentDetector::new();
        assert_eq!(detector.detect("").await, DetectedIntent::Normal);
        assert_eq!(detector.detect("   ").await, DetectedIntent::Normal);
    }

    #[tokio::test]
    async fn detect_without_llm_is_always_normal() {
        let detector = IntentDetector::new();
        assert_eq!(
            detector.detect("切换到编程助手").await,
            DetectedIntent::Normal
        );
    }

    // -- prompt builder tests --

    #[test]
    fn classify_prompt_includes_agents() {
        let agents = vec![
            AgentInfo { id: "main".into(), name: "main".into() },
            AgentInfo { id: "cowork".into(), name: "cowork".into() },
        ];
        let prompt = build_intent_classify_prompt("使用cowork写报告", &agents);
        assert!(prompt.contains("cowork"));
        assert!(prompt.contains("main"));
        assert!(prompt.contains("使用cowork写报告"));
    }

    #[test]
    fn classify_prompt_empty_agents() {
        let prompt = build_intent_classify_prompt("hello", &[]);
        assert!(prompt.contains("No specific agents registered"));
    }

    #[test]
    fn build_prompts_not_empty() {
        assert!(!build_id_resolve_prompt("交易助手").is_empty());
        assert!(!build_soul_generation_prompt("trading", "交易助手").is_empty());
    }
}
