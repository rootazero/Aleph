//! MoA prompt templates + guidance attachment.

use crate::providers::message::{ContentBlock, UnifiedMessage};

/// One advisor's consultation outcome (text or a labelled failure note).
#[derive(Clone, Debug)]
pub(crate) struct AdvisorOutcome {
    pub label: String,
    pub text: String,
}

/// System prompt for every advisor call. Ported from hermes
/// `_REFERENCE_SYSTEM_PROMPT`: without this framing a bare trimmed
/// conversation makes the advisor believe it is the acting agent — it then
/// refuses ("I can't access files") or hallucinates tool calls.
pub(crate) const ADVISOR_SYSTEM_PROMPT: &str =
    "You are an advisor in a Mixture of Agents (MoA) process. You are NOT \
     the acting agent and you do NOT execute anything: you cannot call \
     tools, run commands, browse, or access files, repositories, or URLs, \
     and you should not try to or apologize for being unable to. A separate \
     aggregator model holds those capabilities and will take the actual \
     actions.\n\n\
     The conversation below is the current state of a task handled by that \
     acting agent. Your job is to give your most intelligent analysis of \
     that state: understand the goal, reason about the problem, and advise \
     on what to do next. Surface the best approach, concrete next steps and \
     tool-use strategy, likely pitfalls and risks, and anything the acting \
     agent may have missed or gotten wrong. Assume any referenced files, \
     URLs, or systems exist and reason about them from the context given \
     rather than asking for access.\n\n\
     Respond with your advice directly — no preamble, no disclaimers about \
     tools or access. Your response is private guidance handed to the \
     aggregator, not an answer shown to the user.";

/// Build the guidance block injected at the END of the aggregator's prompt.
pub(crate) fn build_guidance(
    preset: &str,
    aggregator_label: &str,
    outcomes: &[AdvisorOutcome],
) -> String {
    let joined = outcomes
        .iter()
        .enumerate()
        .map(|(idx, o)| format!("Advisor {} — {}:\n{}", idx + 1, o.label, o.text))
        .collect::<Vec<_>>()
        .join("\n\n");
    let labels = outcomes
        .iter()
        .map(|o| o.label.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "[Mixture of Agents advisory context]\n\
         Preset: {preset}\n\
         Aggregator/acting model: {aggregator_label}\n\
         Advisors: {labels}\n\n\
         Use the advisor responses below as private context. You are the \
         aggregator and acting model: answer the user directly or call tools \
         as needed.\n\n\
         {joined}"
    )
}

/// Attach the guidance at the very END of the message list, so the
/// `[system][task][tool-history]` prefix stays byte-stable and KV-cache
/// reusable (hermes lesson: merging into an earlier user turn re-prefills
/// the whole conversation on every tool iteration). Merge into a trailing
/// user turn when present; otherwise append a new user turn.
pub(crate) fn attach_guidance(messages: &mut Vec<UnifiedMessage>, guidance: &str) {
    if let Some(UnifiedMessage::User { content }) = messages.last_mut() {
        if let Some(ContentBlock::Text { text, .. }) = content.last_mut() {
            text.push_str("\n\n");
            text.push_str(guidance);
            return;
        }
        content.push(ContentBlock::Text {
            text: guidance.to_string(),
            cache_control: None,
        });
        return;
    }
    messages.push(UnifiedMessage::user(guidance));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcomes() -> Vec<AdvisorOutcome> {
        vec![
            AdvisorOutcome { label: "openai:gpt-5.5".into(), text: "advice A".into() },
            AdvisorOutcome { label: "deepseek:v4".into(), text: "[failed: timeout]".into() },
        ]
    }

    #[test]
    fn guidance_lists_all_advisors_in_order() {
        let g = build_guidance("default", "anthropic:opus", &outcomes());
        let a = g.find("Advisor 1 — openai:gpt-5.5").unwrap();
        let b = g.find("Advisor 2 — deepseek:v4").unwrap();
        assert!(a < b);
        assert!(g.contains("advice A"));
        assert!(g.contains("[failed: timeout]"));
        assert!(g.contains("Preset: default"));
    }

    #[test]
    fn attach_merges_into_trailing_user_turn() {
        let mut msgs = vec![UnifiedMessage::user("original prompt")];
        attach_guidance(&mut msgs, "GUIDE");
        assert_eq!(msgs.len(), 1);
        let UnifiedMessage::User { content } = &msgs[0] else { panic!() };
        let ContentBlock::Text { text, .. } = &content[0] else { panic!() };
        assert!(text.starts_with("original prompt"));
        assert!(text.ends_with("GUIDE"));
    }

    #[test]
    fn attach_appends_after_trailing_assistant() {
        let mut msgs = vec![
            UnifiedMessage::user("q"),
            UnifiedMessage::assistant("a"),
        ];
        attach_guidance(&mut msgs, "GUIDE");
        assert_eq!(msgs.len(), 3);
        assert!(matches!(msgs.last(), Some(UnifiedMessage::User { .. })));
    }
}
