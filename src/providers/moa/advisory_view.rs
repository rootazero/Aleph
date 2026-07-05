//! Advisory-view transform: flatten the acting agent's conversation into
//! plain user/assistant text turns for advisor (reference) models.
//!
//! Faithful port of hermes moa_loop.py `_reference_messages`: advisors see
//! what the agent DID (tool calls) and what came back (truncated tool
//! results) as text — zero tool-role messages, zero tool_calls arrays — so
//! strict providers never 400, and the view always ends on a user turn
//! (Anthropic no-trailing-assistant-prefill rule) without deleting context.

use std::hash::{Hash, Hasher};

use crate::providers::message::{ContentBlock, UnifiedMessage};

/// Per-tool-result character budget for the advisory copy. The acting
/// aggregator always gets the untrimmed transcript; this only shapes the
/// disposable advisory view.
pub(crate) const TOOL_RESULT_BUDGET: usize = 4000;

/// Synthetic trailing user turn when the view would end on an assistant turn.
pub(crate) const ADVISORY_INSTRUCTION: &str =
    "[The conversation above is the current state of the task. Give your \
     most intelligent judgement: what is going on, what should happen next, \
     what risks or mistakes you see, and how the acting agent should \
     proceed.]";

/// Head+tail preview with a `[... N chars omitted ...]` marker. UTF-8 safe.
pub(crate) fn truncate_tool_result(text: &str, budget: usize) -> String {
    let total = text.chars().count();
    if total <= budget {
        return text.to_string();
    }
    let half = budget / 2;
    let head: String = text.chars().take(half).collect();
    let tail: String = {
        let skip = total - half;
        text.chars().skip(skip).collect()
    };
    let omitted = total - 2 * half;
    format!("{head}\n[... {omitted} chars omitted ...]\n{tail}")
}

fn text_of(blocks: &[ContentBlock]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text { text, .. } => {
                if !text.is_empty() {
                    parts.push(text.clone());
                }
            }
            ContentBlock::Json { value } => parts.push(value.to_string()),
            // Thinking is the acting model's private reasoning; ToolCall is
            // rendered separately; images carry no advisory text.
            _ => {}
        }
    }
    parts.join("\n")
}

fn render_tool_calls(blocks: &[ContentBlock]) -> Vec<String> {
    let mut lines = Vec::new();
    for block in blocks {
        if let ContentBlock::ToolCall { name, arguments, .. } = block {
            let args = if arguments.is_null() {
                String::new()
            } else {
                arguments.to_string()
            };
            if args.is_empty() {
                lines.push(format!("[called tool: {name}]"));
            } else {
                lines.push(format!("[called tool: {name}({args})]"));
            }
        }
    }
    lines
}

fn append_to_last_assistant(rendered: &mut Vec<UnifiedMessage>, block: String) {
    if let Some(UnifiedMessage::Assistant { content }) = rendered.last_mut() {
        if let Some(ContentBlock::Text { text, .. }) = content.last_mut() {
            text.push('\n');
            text.push_str(&block);
            return;
        }
    }
    rendered.push(UnifiedMessage::assistant(block));
}

/// Build the flattened advisory view. See module docs for the rules.
pub(crate) fn build_advisory_view(messages: &[UnifiedMessage]) -> Vec<UnifiedMessage> {
    let mut rendered: Vec<UnifiedMessage> = Vec::new();
    let mut last_user_text: Option<String> = None;

    for msg in messages {
        match msg {
            UnifiedMessage::User { content } => {
                let text = text_of(content);
                if !text.trim().is_empty() {
                    last_user_text = Some(text.clone());
                }
                rendered.push(UnifiedMessage::user(text));
            }
            UnifiedMessage::Assistant { content } => {
                let mut parts: Vec<String> = Vec::new();
                let text = text_of(content);
                if !text.trim().is_empty() {
                    parts.push(text.trim().to_string());
                }
                parts.extend(render_tool_calls(content));
                if !parts.is_empty() {
                    rendered.push(UnifiedMessage::assistant(parts.join("\n")));
                }
            }
            UnifiedMessage::ToolResult {
                content, is_error, ..
            } => {
                let result_text = truncate_tool_result(&text_of(content), TOOL_RESULT_BUDGET);
                let tag = if *is_error {
                    "tool result (error)"
                } else {
                    "tool result"
                };
                append_to_last_assistant(&mut rendered, format!("[{tag}: {result_text}]"));
            }
            // #[non_exhaustive]: future variants carry no advisory meaning
            // until explicitly handled.
        }
    }

    match rendered.last() {
        Some(UnifiedMessage::Assistant { .. }) => {
            rendered.push(UnifiedMessage::user(ADVISORY_INSTRUCTION));
        }
        Some(_) => {}
        None => {
            if let Some(text) = last_user_text {
                rendered.push(UnifiedMessage::user(text));
            }
        }
    }
    rendered
}

/// Stable signature of the advisory view — the fan-out cache key. Uses the
/// std hasher (cache dedup only, not security).
pub(crate) fn view_signature(view: &[UnifiedMessage]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for msg in view {
        let (role, content) = match msg {
            UnifiedMessage::User { content } => ("user", content),
            UnifiedMessage::Assistant { content } => ("assistant", content),
            UnifiedMessage::ToolResult { content, .. } => ("tool", content),
        };
        role.hash(&mut hasher);
        text_of(content).hash(&mut hasher);
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assistant_with_tool_call() -> UnifiedMessage {
        UnifiedMessage::Assistant {
            content: vec![
                ContentBlock::Text { text: "Let me check.".to_string(), cache_control: None },
                ContentBlock::ToolCall {
                    id: "c1".to_string(),
                    name: "bash".to_string(),
                    arguments: json!({"cmd": "ls"}),
                    thought_signature: None,
                },
            ],
        }
    }

    fn view_texts(view: &[UnifiedMessage]) -> Vec<(&'static str, String)> {
        view.iter()
            .map(|m| match m {
                UnifiedMessage::User { content } => ("user", super::text_of(content)),
                UnifiedMessage::Assistant { content } => ("assistant", super::text_of(content)),
                _ => panic!("advisory view must contain only user/assistant"),
            })
            .collect()
    }

    #[test]
    fn tool_calls_rendered_as_text_and_results_folded() {
        let msgs = vec![
            UnifiedMessage::user("fix the bug"),
            assistant_with_tool_call(),
            UnifiedMessage::tool_result("c1", "bash", "file1\nfile2", false),
        ];
        let view = build_advisory_view(&msgs);
        let texts = view_texts(&view);
        // user, assistant(text+call+result), synthetic trailing user
        assert_eq!(texts.len(), 3);
        assert_eq!(texts[0], ("user", "fix the bug".to_string()));
        assert!(texts[1].1.contains("[called tool: bash("));
        assert!(texts[1].1.contains("[tool result: file1"));
        assert_eq!(texts[2].1, ADVISORY_INSTRUCTION);
    }

    #[test]
    fn error_results_labelled() {
        let msgs = vec![
            UnifiedMessage::user("go"),
            assistant_with_tool_call(),
            UnifiedMessage::tool_result("c1", "bash", "boom", true),
        ];
        let view = build_advisory_view(&msgs);
        assert!(view_texts(&view)[1].1.contains("[tool result (error): boom]"));
    }

    #[test]
    fn fresh_user_turn_kept_as_terminal() {
        let view = build_advisory_view(&[UnifiedMessage::user("hello")]);
        let texts = view_texts(&view);
        assert_eq!(texts, vec![("user", "hello".to_string())]);
    }

    #[test]
    fn orphan_tool_result_becomes_assistant_line() {
        let msgs = vec![UnifiedMessage::tool_result("c9", "bash", "out", false)];
        let view = build_advisory_view(&msgs);
        let texts = view_texts(&view);
        assert_eq!(texts[0].0, "assistant");
        assert!(texts[0].1.starts_with("[tool result: out]"));
        // ends on the synthetic user turn
        assert_eq!(texts.last().unwrap().1, ADVISORY_INSTRUCTION);
    }

    #[test]
    fn truncation_is_head_tail_and_utf8_safe() {
        let long = "汉".repeat(5000);
        let out = truncate_tool_result(&long, 4000);
        assert!(out.contains("chars omitted"));
        assert!(out.chars().count() < 4100);
        // must not panic on multi-byte boundaries (would have above)
    }

    #[test]
    fn short_results_untouched() {
        assert_eq!(truncate_tool_result("ok", 4000), "ok");
    }

    #[test]
    fn signature_changes_with_new_tool_result_and_is_stable() {
        let base = vec![UnifiedMessage::user("go"), assistant_with_tool_call()];
        let v1 = build_advisory_view(&base);
        let mut grown = base.clone();
        grown.push(UnifiedMessage::tool_result("c1", "bash", "out", false));
        let v2 = build_advisory_view(&grown);
        assert_ne!(view_signature(&v1), view_signature(&v2));
        assert_eq!(view_signature(&v1), view_signature(&build_advisory_view(&base)));
    }

    #[test]
    fn empty_assistant_dropped() {
        let msgs = vec![
            UnifiedMessage::user("go"),
            UnifiedMessage::Assistant { content: vec![] },
        ];
        let view = build_advisory_view(&msgs);
        assert_eq!(view_texts(&view).len(), 1);
    }
}
