//! Coordinator — LLM-based orchestration planner.
//!
//! Pure functions for building prompts and parsing plans.
//! No async, no LLM calls — these are used by the orchestrator to construct
//! the inputs and interpret the outputs of Coordinator and Persona LLM invocations.

use super::protocol::{CoordinatorPlan, GroupChatError, Persona, RespondentPlan};

/// Maximum number of characters from a persona's system_prompt to include
/// in the coordinator prompt (to keep token usage reasonable).
const SYSTEM_PROMPT_TRUNCATE_LEN: usize = 120;

/// Build the prompt for the Coordinator LLM call.
///
/// The coordinator receives a list of available personas (with truncated system
/// prompts), the conversation history so far, and the user's latest message.
/// It must return a JSON plan specifying which personas should respond.
pub fn build_coordinator_prompt(
    personas: &[Persona],
    user_message: &str,
    history: &str,
    topic: &Option<String>,
) -> String {
    let mut prompt = String::new();

    prompt.push_str(
        "You are the Coordinator of a multi-persona group chat. \
         Your job is to decide which personas should respond to the user's message \
         and in what order.\n\n",
    );

    // Topic
    if let Some(t) = topic {
        prompt.push_str(&format!("Discussion topic: {t}\n\n"));
    }

    // Persona list
    prompt.push_str("Available personas:\n");
    for p in personas {
        let truncated = truncate_str(&p.system_prompt, SYSTEM_PROMPT_TRUNCATE_LEN);
        prompt.push_str(&format!("- id=\"{}\" name=\"{}\" prompt=\"{}\"\n", p.id, p.name, truncated));
    }
    prompt.push('\n');

    // History
    if !history.is_empty() {
        prompt.push_str("Conversation history:\n");
        prompt.push_str(history);
        prompt.push('\n');
    }

    // User message
    prompt.push_str(&format!("User message: {user_message}\n\n"));

    // Output instruction
    prompt.push_str(
        "Based on the user's message, select which personas should respond and in what order. \
         Provide guidance for each persona on what to focus on.\n\n\
         Output ONLY valid JSON in this exact format (no extra text):\n\
         {\"respondents\":[{\"persona_id\":\"...\",\"order\":N,\"guidance\":\"...\"}],\"need_summary\":bool}\n\n\
         Rules:\n\
         - Only include personas whose expertise is relevant to the user's message.\n\
         - Order them so that foundational perspectives come first.\n\
         - Set need_summary to true if the topic is complex and benefits from synthesis.\n",
    );

    prompt
}

/// Parse the Coordinator LLM output into a structured plan.
///
/// Handles common LLM quirks like wrapping JSON in markdown code fences.
pub fn parse_coordinator_plan(raw: &str) -> Result<CoordinatorPlan, GroupChatError> {
    let trimmed = strip_markdown_fences(raw.trim());

    serde_json::from_str(trimmed).map_err(|e| {
        GroupChatError::CoordinatorPlanParseError(format!(
            "{e} — raw input: {}",
            truncate_str(raw, 200)
        ))
    })
}

/// Fallback plan when the Coordinator fails — all personas in config order.
///
/// This ensures the conversation continues even if the coordinator LLM call
/// returns unparseable output.
pub fn build_fallback_plan(personas: &[Persona]) -> CoordinatorPlan {
    let respondents = personas
        .iter()
        .enumerate()
        .map(|(i, p)| RespondentPlan {
            persona_id: p.id.clone(),
            order: i as u32,
            guidance: String::new(),
        })
        .collect();

    CoordinatorPlan {
        respondents,
        need_summary: false,
    }
}

/// Build the prompt for a persona's LLM call with cumulative context.
///
/// Each persona receives its own system prompt, the user's original message,
/// any prior discussion from earlier respondents in this round, and the
/// coordinator's guidance for this specific persona.
pub fn build_persona_prompt(
    persona: &Persona,
    user_message: &str,
    prior_discussion: &str,
    guidance: &str,
) -> String {
    let mut prompt = String::new();

    // Identity and system prompt
    prompt.push_str(&format!(
        "You are \"{name}\". {system_prompt}\n\n",
        name = persona.name,
        system_prompt = persona.system_prompt,
    ));

    // Coordinator guidance
    if !guidance.is_empty() {
        prompt.push_str(&format!("Coordinator guidance: {guidance}\n\n"));
    }

    // Prior discussion from this round
    if !prior_discussion.is_empty() {
        prompt.push_str("Prior discussion in this round:\n");
        prompt.push_str(prior_discussion);
        prompt.push('\n');
    }

    // User message
    prompt.push_str(&format!("User message: {user_message}\n\n"));

    // Response instruction
    prompt.push_str(&format!(
        "Please respond from \"{name}\"'s perspective and area of expertise. \
         Be concise and focused. Do not repeat what others have already said.",
        name = persona.name,
    ));

    prompt
}

// =============================================================================
// Helpers
// =============================================================================

/// Truncate a string to at most `max_len` characters, appending "..." if truncated.
///
/// Uses `char_indices` to avoid panicking on multi-byte UTF-8 boundaries.
fn truncate_str(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        return s;
    }
    // Find the byte index of the char boundary at or before max_len chars.
    match s.char_indices().nth(max_len) {
        Some((byte_idx, _)) => &s[..byte_idx],
        None => s, // fewer than max_len chars — return as-is
    }
}

/// Strip markdown code fences (` ```json ... ``` `) from LLM output.
fn strip_markdown_fences(s: &str) -> &str {
    let s = s.trim();

    // Strip leading ```json or ```
    let s = if let Some(rest) = s.strip_prefix("```json") {
        rest.trim_start()
    } else if let Some(rest) = s.strip_prefix("```") {
        rest.trim_start()
    } else {
        return s;
    };

    // Strip trailing ```
    if let Some(body) = s.strip_suffix("```") {
        body.trim()
    } else {
        s.trim()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_personas() -> Vec<Persona> {
        vec![
            Persona {
                id: "arch".to_string(),
                name: "Architect".to_string(),
                system_prompt: "You are a senior software architect with deep expertise in distributed systems and API design.".to_string(),
                provider: Some("anthropic".to_string()),
                model: Some("claude-sonnet-4-20250514".to_string()),
                thinking_level: None,
            },
            Persona {
                id: "pm".to_string(),
                name: "Product Manager".to_string(),
                system_prompt: "You are an experienced product manager focused on user needs and business value.".to_string(),
                provider: None,
                model: None,
                thinking_level: None,
            },
        ]
    }

    #[test]
    fn test_build_coordinator_prompt() {
        let personas = test_personas();
        let prompt = build_coordinator_prompt(
            &personas,
            "How should we design the authentication system?",
            "[Alice]: We discussed OAuth earlier.\n\n",
            &Some("System Architecture Review".to_string()),
        );

        // Contains persona names and IDs
        assert!(prompt.contains("Architect"), "should contain persona name");
        assert!(prompt.contains("arch"), "should contain persona id");
        assert!(prompt.contains("Product Manager"), "should contain second persona name");
        assert!(prompt.contains("pm"), "should contain second persona id");

        // Contains user message
        assert!(prompt.contains("How should we design the authentication system?"));

        // Contains topic
        assert!(prompt.contains("System Architecture Review"));

        // Contains history
        assert!(prompt.contains("Alice"));

        // Contains JSON output instruction
        assert!(prompt.contains("respondents"));
        assert!(prompt.contains("persona_id"));
        assert!(prompt.contains("need_summary"));
    }

    #[test]
    fn test_parse_coordinator_plan_valid() {
        let json = r#"{"respondents":[{"persona_id":"arch","order":1,"guidance":"Focus on tech"}],"need_summary":true}"#;
        let plan = parse_coordinator_plan(json).unwrap();
        assert_eq!(plan.respondents.len(), 1);
        assert_eq!(plan.respondents[0].persona_id, "arch");
        assert_eq!(plan.respondents[0].order, 1);
        assert_eq!(plan.respondents[0].guidance, "Focus on tech");
        assert!(plan.need_summary);
    }

    #[test]
    fn test_parse_coordinator_plan_with_markdown_wrapper() {
        let json = "```json\n{\"respondents\":[{\"persona_id\":\"arch\",\"order\":1,\"guidance\":\"tech\"}],\"need_summary\":false}\n```";
        let plan = parse_coordinator_plan(json).unwrap();
        assert_eq!(plan.respondents.len(), 1);
        assert_eq!(plan.respondents[0].persona_id, "arch");
        assert!(!plan.need_summary);
    }

    #[test]
    fn test_parse_coordinator_plan_invalid() {
        let result = parse_coordinator_plan("not json at all");
        assert!(result.is_err());
        match result.unwrap_err() {
            GroupChatError::CoordinatorPlanParseError(msg) => {
                assert!(msg.contains("not json at all"), "error should contain raw input");
            }
            other => panic!("expected CoordinatorPlanParseError, got: {other:?}"),
        }
    }

    #[test]
    fn test_build_fallback_plan() {
        let personas = test_personas();
        let plan = build_fallback_plan(&personas);

        assert_eq!(plan.respondents.len(), 2);
        assert_eq!(plan.respondents[0].persona_id, "arch");
        assert_eq!(plan.respondents[0].order, 0);
        assert_eq!(plan.respondents[1].persona_id, "pm");
        assert_eq!(plan.respondents[1].order, 1);
        assert!(!plan.need_summary);

        // Guidance should be empty for fallback
        assert!(plan.respondents[0].guidance.is_empty());
        assert!(plan.respondents[1].guidance.is_empty());
    }

    #[test]
    fn test_build_persona_prompt() {
        let personas = test_personas();
        let persona = &personas[0]; // Architect

        let prompt = build_persona_prompt(
            persona,
            "How should we handle rate limiting?",
            "[Product Manager]: We need to consider user tiers.\n\n",
            "Focus on the technical implementation details.",
        );

        // Contains persona identity
        assert!(prompt.contains("Architect"), "should contain persona name");
        assert!(
            prompt.contains("senior software architect"),
            "should contain system prompt content"
        );

        // Contains user message
        assert!(prompt.contains("How should we handle rate limiting?"));

        // Contains guidance
        assert!(prompt.contains("Focus on the technical implementation details."));

        // Contains prior discussion
        assert!(prompt.contains("Product Manager"));
        assert!(prompt.contains("user tiers"));

        // Contains response instruction
        assert!(prompt.contains("perspective and area of expertise"));
    }
}
