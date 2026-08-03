//! MoA prompt templates + guidance attachment.

use std::borrow::Cow;

use crate::providers::message::{ContentBlock, UnifiedMessage};
// The same `ToolDefinition` `RequestPayload::tools` carries — NOT the
// lighter `tools::runtime` struct of the same name.
use crate::tool_metadata::ToolDefinition;

/// Characters of a provider error that reach the AGGREGATOR'S PROMPT.
///
/// `AdvisorResult.error` — the trace/panel channel a human reads — stays
/// full; this budget only bounds the copy that rides into the model's
/// context. A provider that answers with a 2 KB HTML error page from a proxy
/// otherwise injects all 2 KB of it into every prompt for the rest of the run
/// (the breaker retires the slot, but the note it leaves behind is reprinted
/// every iteration).
const ADVISOR_NOTE_BUDGET: usize = 240;

/// One advisor's consultation outcome: either real advice, or the labelled
/// placeholder a failed / timed-out / breaker-skipped / empty slot leaves
/// behind so the aggregator can tell "one advisor configured" from "three
/// configured, two dead" (round-6 G1 副则).
#[derive(Clone, Debug)]
pub(crate) struct AdvisorOutcome {
    pub label: String,
    pub text: String,
    /// `true` only when `text` is advice the aggregator can act on.
    ///
    /// Set at construction from the branch that produced the outcome — never
    /// re-derived by sniffing `text` for a `[failed:` prefix. hermes sniffs
    /// (`_is_failed_reference`), which makes an advisor that legitimately
    /// opens its answer with a bracketed note indistinguishable from a dead
    /// one, and silently mis-classifies the moment a note's wording drifts.
    pub advised: bool,
}

impl AdvisorOutcome {
    /// A slot that answered with usable advice.
    pub(crate) fn advice(label: &str, text: String) -> Self {
        Self {
            label: label.to_string(),
            text,
            advised: true,
        }
    }

    /// A slot that produced no advice. `note` is the bracketed placeholder the
    /// aggregator sees (`[failed: …]`, `[timeout after 120s]`, `[skipped: …]`,
    /// `[empty response]`); it is clamped to [`ADVISOR_NOTE_BUDGET`] because it
    /// may embed raw provider error text.
    pub(crate) fn unavailable(label: &str, note: &str) -> Self {
        Self {
            label: label.to_string(),
            text: crate::utils::text_format::truncate_text(note, ADVISOR_NOTE_BUDGET),
            advised: false,
        }
    }
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

/// Per-tool line budget in the roster (round-6 G2).
const ROSTER_LINE_BUDGET: usize = 100;
/// Whole-roster budget. This text ships to every advisor on every fan-out, so
/// it is deliberately small next to the conversation view itself.
const ROSTER_TOTAL_BUDGET: usize = 1800;

/// First sentence of `description`, clamped to `ROSTER_LINE_BUDGET` chars.
/// UTF-8 safe (char boundaries via `char_indices`), single-line.
fn summarize(description: &str) -> String {
    let first = description
        .split_terminator(". ")
        .next()
        .unwrap_or(description)
        .trim();
    let one_line = first.replace('\n', " ");
    let total = one_line.chars().count();
    if total <= ROSTER_LINE_BUDGET {
        return one_line;
    }
    let end = one_line
        .char_indices()
        .nth(ROSTER_LINE_BUDGET)
        .map_or(one_line.len(), |(i, _)| i);
    format!("{}…", &one_line[..end])
}

/// The advisor system prompt, plus a roster of the tools the ACTING agent
/// holds (round-6 G2).
///
/// [`ADVISOR_SYSTEM_PROMPT`] asks each advisor for "concrete next steps and
/// tool-use strategy", yet the advisory view only reveals a tool once it has
/// already been called (`[called tool: X]`) — tools never yet used are
/// invisible, so advisors invent names or retreat to generic advice. The
/// definitions are already owned by `MoaProvider::process` and were being
/// handed to the aggregator alone; this is pure wiring (hermes references are
/// blind here too, so it is a surpass item, not parity).
///
/// Returns the const verbatim when there are no tools, so the plain-chat path
/// is byte-identical to before. Payload order is preserved (already
/// deterministic), keeping the system prefix stable across iterations so
/// provider prefix caches keep hitting.
pub(crate) fn advisor_system_prompt(tools: Option<&[ToolDefinition]>) -> Cow<'static, str> {
    let tools = match tools {
        Some(t) if !t.is_empty() => t,
        _ => return Cow::Borrowed(ADVISOR_SYSTEM_PROMPT),
    };

    let mut roster = String::new();
    let mut listed = 0usize;
    for tool in tools {
        let line = format!("- {}: {}\n", tool.name, summarize(&tool.description));
        if roster.chars().count() + line.chars().count() > ROSTER_TOTAL_BUDGET {
            break;
        }
        roster.push_str(&line);
        listed += 1;
    }
    if listed == 0 {
        return Cow::Borrowed(ADVISOR_SYSTEM_PROMPT);
    }
    let omitted = tools.len() - listed;
    let tail = if omitted > 0 {
        format!("… (+{omitted} more tools)\n")
    } else {
        String::new()
    };

    Cow::Owned(format!(
        "{ADVISOR_SYSTEM_PROMPT}\n\n\
         The acting agent has the tools below available. You still cannot call \
         any of them — use the list to ground your advice in what is actually \
         possible, and name the specific tools the acting agent should reach \
         for next.\n\n\
         {roster}{tail}"
    ))
}

/// Build the guidance block injected at the END of the aggregator's prompt.
///
/// Two shapes, chosen by whether ANY slot actually advised:
///
/// * **some advice** — the roster names every slot (unavailable ones carry
///   their note inline, so the aggregator can still tell "one advisor
///   configured" from "three configured, two dead"), then the numbered
///   responses list only the slots that advised. Numbering is the SLOT index,
///   not a re-count over the survivors, so an advisor's number never shifts
///   mid-run and lines up with the `i/n` the panel shows.
/// * **no advice at all** — the "use the advisor responses below" framing is
///   dropped entirely. Wrapping a wall of `[failed: …]` notes in an
///   instruction to use them as context is worse than saying nothing: it
///   spends tokens telling the acting model to consult advice that does not
///   exist, on every iteration for the rest of the run (the breaker keeps the
///   slots retired, so the wall is reprinted verbatim each time).
pub(crate) fn build_guidance(
    preset: &str,
    aggregator_label: &str,
    outcomes: &[AdvisorOutcome],
) -> String {
    let roster = outcomes
        .iter()
        .map(|o| {
            if o.advised {
                // rust-doctor-disable-next-line excessive-clone
                o.label.clone()
            } else {
                format!("{} {}", o.label, o.text)
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let header = format!(
        "[Mixture of Agents advisory context]\n\
         Preset: {preset}\n\
         Aggregator/acting model: {aggregator_label}\n\
         Advisors: {roster}\n\n"
    );

    let joined = outcomes
        .iter()
        .enumerate()
        .filter(|(_, o)| o.advised)
        .map(|(idx, o)| format!("Advisor {} — {}:\n{}", idx + 1, o.label, o.text))
        .collect::<Vec<_>>()
        .join("\n\n");

    if joined.is_empty() {
        return format!(
            "{header}\
             No advisor returned usable guidance this turn. You are the acting \
             model: proceed on your own judgment."
        );
    }
    format!(
        "{header}\
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
            AdvisorOutcome::advice("openai:gpt-5.5", "advice A".into()),
            AdvisorOutcome::unavailable("deepseek:v4", "[failed: timeout]"),
        ]
    }

    #[test]
    fn guidance_numbers_advisors_by_slot_and_keeps_dead_slots_in_the_roster() {
        let mut three = outcomes();
        three.push(AdvisorOutcome::advice("xai:grok-5", "advice C".into()));
        let g = build_guidance("default", "anthropic:opus", &three);

        assert!(g.contains("Preset: default"));
        // The roster names every slot, so "3 configured, 2 alive" stays legible.
        assert!(g.contains("Advisors: openai:gpt-5.5, deepseek:v4 [failed: timeout], xai:grok-5"));
        // Only slots that ADVISED get a numbered response block...
        assert!(g.contains("Advisor 1 — openai:gpt-5.5"));
        assert!(g.contains("Advisor 3 — xai:grok-5"));
        // ...and the dead slot is not presented as advice to read.
        assert!(
            !g.contains("Advisor 2 — deepseek:v4"),
            "a failure note must not be numbered as if it were advice:\n{g}"
        );
        // Numbering is the SLOT index — the survivor keeps slot 3, it is not
        // re-counted to 2, so an advisor's number never shifts mid-run.
        assert!(g.contains("advice A") && g.contains("advice C"));
        assert!(g.contains("Use the advisor responses below"));
    }

    #[test]
    fn guidance_drops_the_use_these_framing_when_nothing_advised() {
        let all_dead = vec![
            AdvisorOutcome::unavailable("openai:gpt-5.5", "[failed: 401 unauthorized]"),
            AdvisorOutcome::unavailable("deepseek:v4", "[timeout after 120s]"),
        ];
        let g = build_guidance("default", "anthropic:opus", &all_dead);
        assert!(
            !g.contains("Use the advisor responses below"),
            "must not tell the acting model to use advice that does not exist:\n{g}"
        );
        assert!(g.contains("No advisor returned usable guidance"));
        assert!(g.contains("proceed on your own judgment"));
        // The roster still discloses WHICH slots were down and why.
        assert!(g.contains("openai:gpt-5.5 [failed: 401 unauthorized]"));
        assert!(g.contains("deepseek:v4 [timeout after 120s]"));
        // No numbered response blocks at all.
        assert!(!g.contains("Advisor 1 —"), "{g}");
    }

    #[test]
    fn unavailable_clamps_raw_provider_error_text_bound_for_the_prompt() {
        // A proxy answering with an HTML error page must not ride into the
        // aggregator's context in full, every iteration, for the rest of the run.
        let huge = format!("[failed: {}]", "x".repeat(10_000));
        let o = AdvisorOutcome::unavailable("openai:gpt-5.5", &huge);
        assert!(!o.advised);
        assert!(
            o.text.chars().count() <= ADVISOR_NOTE_BUDGET + 3,
            "note grew to {} chars",
            o.text.chars().count()
        );
        assert!(o.text.starts_with("[failed: xxx"));
    }

    #[test]
    fn unavailable_clamp_is_utf8_safe() {
        let note = format!("[failed: {}]", "顾问挂了".repeat(500));
        // Must not panic on a multi-byte boundary.
        let o = AdvisorOutcome::unavailable("a:b", &note);
        assert!(o.text.chars().count() <= ADVISOR_NOTE_BUDGET + 3);
    }

    #[test]
    fn attach_merges_into_trailing_user_turn() {
        let mut msgs = vec![UnifiedMessage::user("original prompt")];
        attach_guidance(&mut msgs, "GUIDE");
        assert_eq!(msgs.len(), 1);
        let UnifiedMessage::User { content } = &msgs[0] else {
            panic!()
        };
        let ContentBlock::Text { text, .. } = &content[0] else {
            panic!()
        };
        assert!(text.starts_with("original prompt"));
        assert!(text.ends_with("GUIDE"));
    }

    fn tool(name: &str, description: &str) -> ToolDefinition {
        ToolDefinition::new(
            name,
            description,
            serde_json::json!({}),
            crate::tool_metadata::ToolCategory::Builtin,
        )
    }

    #[test]
    fn advisor_prompt_unchanged_without_tools() {
        // The zero-tool path (plain chat) must stay byte-identical to the
        // pre-round-6 prompt.
        assert_eq!(advisor_system_prompt(None), ADVISOR_SYSTEM_PROMPT);
        assert_eq!(advisor_system_prompt(Some(&[])), ADVISOR_SYSTEM_PROMPT);
        assert!(matches!(advisor_system_prompt(None), Cow::Borrowed(_)));
    }

    #[test]
    fn roster_lists_tools_and_keeps_the_no_call_framing() {
        let tools = vec![
            tool("file_ops", "Read, write and search files. Supports globs."),
            tool("web_search", "Search the web for current information."),
        ];
        let prompt = advisor_system_prompt(Some(&tools));
        assert!(prompt.starts_with(ADVISOR_SYSTEM_PROMPT));
        assert!(prompt.contains("- file_ops: Read, write and search files"));
        assert!(prompt.contains("- web_search: Search the web"));
        // Only the FIRST sentence is kept.
        assert!(!prompt.contains("Supports globs"));
        // The "you cannot call tools" framing must survive, or advisors start
        // emitting tool calls the facade has nowhere to put.
        assert!(prompt.contains("You still cannot call"));
    }

    #[test]
    fn roster_respects_the_total_budget_and_says_how_many_it_dropped() {
        let tools: Vec<ToolDefinition> = (0..200)
            .map(|i| tool(&format!("tool_{i}"), "Does a thing for the acting agent."))
            .collect();
        let prompt = advisor_system_prompt(Some(&tools));
        let roster_len = prompt.chars().count() - ADVISOR_SYSTEM_PROMPT.chars().count();
        assert!(
            roster_len < ROSTER_TOTAL_BUDGET + 400,
            "roster grew to {roster_len} chars"
        );
        assert!(prompt.contains("more tools)"), "must admit the truncation");
        assert!(prompt.contains("- tool_0:"));
    }

    #[test]
    fn roster_lines_are_clamped_utf8_safely() {
        let tools = vec![tool("wide", &"汉".repeat(500))];
        let prompt = advisor_system_prompt(Some(&tools));
        // Must not panic on multi-byte boundaries, and must clamp the line.
        assert!(prompt.contains('…'));
        assert!(prompt.chars().count() < ADVISOR_SYSTEM_PROMPT.chars().count() + 400);
    }

    #[test]
    fn attach_appends_after_trailing_assistant() {
        let mut msgs = vec![UnifiedMessage::user("q"), UnifiedMessage::assistant("a")];
        attach_guidance(&mut msgs, "GUIDE");
        assert_eq!(msgs.len(), 3);
        assert!(matches!(msgs.last(), Some(UnifiedMessage::User { .. })));
    }
}
