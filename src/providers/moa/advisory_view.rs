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

/// Head+tail preview with a `[... N chars omitted ...]` marker. UTF-8 safe:
/// slices at char boundaries found via char_indices (no per-char String
/// collection; one full count pass + two partial boundary scans).
pub(crate) fn truncate_tool_result(text: &str, budget: usize) -> String {
    let total = text.chars().count();
    if total <= budget {
        return text.to_string();
    }
    let half = budget / 2;
    // Byte offset AFTER the half-th char (head end boundary).
    let head_end = text.char_indices().nth(half).map_or(text.len(), |(i, _)| i);
    // Byte offset of the (total-half)-th char (tail start boundary).
    let tail_start = text
        .char_indices()
        .nth(total - half)
        .map_or(text.len(), |(i, _)| i);
    let omitted = total - 2 * half;
    format!(
        "{}\n[... {omitted} chars omitted ...]\n{}",
        &text[..head_end],
        &text[tail_start..]
    )
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
            // Advisors can't see pixels, but they must know an image exists —
            // hermes drops multimodal content silently (its #51 gap); the
            // placeholder keeps them from being blindsided by "the screenshot
            // above" (round-2 E4).
            ContentBlock::Image { mime_type, .. } => {
                parts.push(format!("[image: {mime_type}]"));
            }
            // Thinking is the acting model's private reasoning; ToolCall is
            // rendered separately.
            _ => {}
        }
    }
    parts.join("\n")
}

fn render_tool_calls(blocks: &[ContentBlock]) -> Vec<String> {
    let mut lines = Vec::new();
    for block in blocks {
        if let ContentBlock::ToolCall {
            name, arguments, ..
        } = block
        {
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
            } // #[non_exhaustive]: future variants carry no advisory meaning
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
/// std hasher (cache dedup only, not security). Hashes text parts directly
/// (no intermediate join allocation); deliberately ignores cache_control
/// marks so E1's in-place breakpoint marking never perturbs the cache key.
pub(crate) fn view_signature(view: &[UnifiedMessage]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for msg in view {
        let (role, content) = match msg {
            UnifiedMessage::User { content } => ("user", content),
            UnifiedMessage::Assistant { content } => ("assistant", content),
            UnifiedMessage::ToolResult { content, .. } => ("tool", content),
        };
        role.hash(&mut hasher);
        for block in content {
            match block {
                ContentBlock::Text { text, .. } => {
                    if !text.is_empty() {
                        text.hash(&mut hasher);
                    }
                }
                ContentBlock::Json { value } => value.to_string().hash(&mut hasher),
                ContentBlock::Image { mime_type, .. } => {
                    "image".hash(&mut hasher);
                    mime_type.hash(&mut hasher);
                }
                _ => {}
            }
        }
    }
    hasher.finish()
}

/// Mark Anthropic prompt-cache breakpoints on the advisory view: the last
/// Text block of each of the LAST THREE messages gets an ephemeral
/// cache_control (hermes `system_and_3` layout). The view is append-only
/// across iterations, so iteration N+1's prefix replays N's cached segment —
/// without this, per_iteration advisors re-bill the whole prefix every tool
/// step (hermes measured 0/1227 cache reads, 11.5M re-billed tokens).
/// Marking is unconditional: the Anthropic protocol adapter maps the mark to
/// `ephemeral`; every other adapter ignores it (zero per-provider branching).
/// Call AFTER view_signature — the signature deliberately ignores marks.
pub(crate) fn mark_cache_breakpoints(view: &mut [UnifiedMessage]) {
    let len = view.len();
    for msg in view.iter_mut().skip(len.saturating_sub(3)) {
        let content = match msg {
            UnifiedMessage::User { content } | UnifiedMessage::Assistant { content } => content,
            UnifiedMessage::ToolResult { content, .. } => content,
        };
        if let Some(ContentBlock::Text { cache_control, .. }) = content
            .iter_mut()
            .rev()
            .find(|b| matches!(b, ContentBlock::Text { .. }))
        {
            *cache_control = Some(crate::providers::message::CacheControl::Ephemeral { ttl: None });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assistant_with_tool_call() -> UnifiedMessage {
        UnifiedMessage::Assistant {
            content: vec![
                ContentBlock::Text {
                    text: "Let me check.".to_string(),
                    cache_control: None,
                },
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
        assert!(view_texts(&view)[1]
            .1
            .contains("[tool result (error): boom]"));
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
        assert_eq!(
            view_signature(&v1),
            view_signature(&build_advisory_view(&base))
        );
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

    #[test]
    fn signature_ignores_cache_control_marks() {
        let mut a = vec![UnifiedMessage::user("hello")];
        let sig_before = view_signature(&a);
        // Simulate a cache_control mark on the text block (E1 will do this
        // in place) — the signature must not change.
        if let Some(UnifiedMessage::User { content }) = a.last_mut() {
            if let Some(ContentBlock::Text { cache_control, .. }) = content.last_mut() {
                *cache_control =
                    Some(crate::providers::message::CacheControl::Ephemeral { ttl: None });
            }
        }
        assert_eq!(sig_before, view_signature(&a));
    }

    #[test]
    fn cache_breakpoints_mark_last_three_messages() {
        let mut view = vec![
            UnifiedMessage::user("one"),
            UnifiedMessage::assistant("two"),
            UnifiedMessage::user("three"),
            UnifiedMessage::assistant("four"),
            UnifiedMessage::user("five"),
        ];
        mark_cache_breakpoints(&mut view);
        let marked: Vec<bool> =
            view.iter()
                .map(|m| {
                    let content = match m {
                        UnifiedMessage::User { content }
                        | UnifiedMessage::Assistant { content } => content,
                        UnifiedMessage::ToolResult { content, .. } => content,
                    };
                    content.iter().any(|b| {
                        matches!(
                            b,
                            ContentBlock::Text {
                                cache_control: Some(_),
                                ..
                            }
                        )
                    })
                })
                .collect();
        assert_eq!(marked, vec![false, false, true, true, true]);
    }

    #[test]
    fn cache_breakpoints_short_view_marks_all() {
        let mut view = vec![UnifiedMessage::user("only")];
        mark_cache_breakpoints(&mut view);
        let UnifiedMessage::User { content } = &view[0] else {
            panic!()
        };
        assert!(matches!(
            content.last(),
            Some(ContentBlock::Text {
                cache_control: Some(_),
                ..
            })
        ));
    }

    #[test]
    fn image_blocks_render_placeholder_and_json_stringifies() {
        let msgs = vec![UnifiedMessage::User {
            content: vec![
                ContentBlock::Text {
                    text: "look at this".into(),
                    cache_control: None,
                },
                ContentBlock::Image {
                    data: "base64...".into(),
                    mime_type: "image/png".into(),
                },
                ContentBlock::Json {
                    value: json!({"k": 1}),
                },
            ],
        }];
        let view = build_advisory_view(&msgs);
        let texts = view_texts(&view);
        assert!(texts[0].1.contains("look at this"));
        // E4: advisors learn an image exists (hermes drops it silently).
        assert!(texts[0].1.contains("[image: image/png]"));
        assert!(texts[0].1.contains("{\"k\":1}"));
    }

    #[test]
    fn signature_changes_when_image_added() {
        let base = vec![UnifiedMessage::user("go")];
        let with_image = vec![UnifiedMessage::User {
            content: vec![
                ContentBlock::Text {
                    text: "go".into(),
                    cache_control: None,
                },
                ContentBlock::Image {
                    data: "d".into(),
                    mime_type: "image/png".into(),
                },
            ],
        }];
        assert_ne!(
            view_signature(&build_advisory_view(&base)),
            view_signature(&build_advisory_view(&with_image))
        );
    }
}
