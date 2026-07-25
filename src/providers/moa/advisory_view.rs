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

/// Whole-view character budget (round-6 G3). Per-result truncation alone
/// leaves the view growing without bound across a long agentic run; an
/// advisor on a smaller-context model than the aggregator then 4xx's on every
/// later iteration and MoA silently stops working in exactly the scenario it
/// exists for. This is a guardrail against a hard failure, not a tuning knob —
/// hence a constant, like [`TOOL_RESULT_BUDGET`], rather than config surface.
pub(crate) const ADVISORY_VIEW_BUDGET: usize = 120_000;

/// Per-message allowance once [`ADVISORY_VIEW_BUDGET`] is exhausted. Older
/// messages shrink to a head+tail preview at this size rather than
/// disappearing (see [`apply_view_budget`]).
const OVER_BUDGET_MSG_BUDGET: usize = 400;

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
    let mut args = String::new();
    for block in blocks {
        if let ContentBlock::ToolCall {
            name, arguments, ..
        } = block
        {
            args.clear();
            if !arguments.is_null() {
                args.push_str(&arguments.to_string());
            }
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
    let mut parts: Vec<String> = Vec::new();

    for msg in messages {
        match msg {
            UnifiedMessage::User { content } => {
                let text = text_of(content);
                // Round-6 G4: skip blank user turns, mirroring the assistant
                // arm below. An empty text block survives
                // `anthropic/proto_impl.rs`'s `blocks.is_empty()` fallback
                // (that guard only catches a wholly empty content vec) and
                // reaches the wire as `MessageContent::Text { content: "" }`,
                // which the API rejects with 400 — failing EVERY advisor at
                // once, not just the one turn.
                if text.trim().is_empty() {
                    continue;
                }
                rendered.push(UnifiedMessage::user(text));
            }
            UnifiedMessage::Assistant { content } => {
                parts.clear();
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
        // Ends on an assistant turn: append the synthetic instruction rather
        // than ask a provider to prefill. EMPTY takes the same arm (round-6
        // G4 terminal guarantee) — an advisor call with zero messages is a
        // 4xx on every provider, and the previous `last_user_text` refill was
        // unreachable: the only way to render nothing is to have had no user
        // and no tool-result message at all, in which case that stash is None.
        Some(UnifiedMessage::Assistant { .. }) | None => {
            rendered.push(UnifiedMessage::user(ADVISORY_INSTRUCTION));
        }
        Some(_) => {}
    }
    rendered
}

/// The content blocks of any view message, whatever its role.
fn content_mut(msg: &mut UnifiedMessage) -> &mut Vec<ContentBlock> {
    match msg {
        UnifiedMessage::User { content } | UnifiedMessage::Assistant { content } => content,
        UnifiedMessage::ToolResult { content, .. } => content,
    }
}

/// Total characters of text carried by one view message.
fn text_chars(content: &[ContentBlock]) -> usize {
    content
        .iter()
        .map(|b| match b {
            ContentBlock::Text { text, .. } => text.chars().count(),
            _ => 0,
        })
        .sum()
}

/// Clamp the whole advisory view to `budget` characters by SHRINKING the
/// oldest messages — never dropping them (round-6 G3).
///
/// [`TOOL_RESULT_BUDGET`] bounds one tool result; nothing bounded the view
/// itself, so a long agentic run eventually overflows any advisor whose
/// context window is smaller than the aggregator's — a hard 4xx on every
/// remaining iteration, i.e. MoA dying silently in the long-task scenario it
/// exists for. Shrinking rather than eliding keeps the message count, order
/// and role sequence identical, so the "first message must be `user`" and
/// alternation rules a drop-based scheme would have to re-establish simply
/// cannot break.
///
/// Newest-first: each message keeps its full text while the budget lasts;
/// past that, older messages are re-truncated head+tail through
/// [`truncate_tool_result`] at a shrinking allowance (the marker line each
/// leaves behind is the small, bounded overshoot). The newest message is
/// exempt from the stub allowance — losing the live state is worse than a
/// marginally larger view — and gets the whole budget instead.
///
/// Call BEFORE [`view_signature`], so the cache key describes what is
/// actually sent, and before [`mark_cache_breakpoints`], which assumes the
/// text blocks are final.
pub(crate) fn apply_view_budget(view: &mut [UnifiedMessage], budget: usize) {
    let newest = view.len().saturating_sub(1);
    let mut remaining = budget;
    for (idx, msg) in view.iter_mut().enumerate().rev() {
        let content = content_mut(msg);
        let chars = text_chars(content);
        if chars <= remaining {
            remaining -= chars;
            continue;
        }
        let allowance = if idx == newest {
            budget
        } else {
            OVER_BUDGET_MSG_BUDGET.min(remaining)
        };
        remaining = remaining.saturating_sub(allowance);
        // Reaching here always means `chars > allowance`: for an older message
        // `allowance = min(stub, remaining) <= remaining < chars`, and the
        // newest only lands here when it alone exceeds the whole budget. So
        // the fold below always shortens, and `truncate_tool_result` keeps the
        // result non-empty even at allowance 0 (the omitted-chars marker).
        //
        // `build_advisory_view` renders every message as exactly one text
        // block (images become `[image: …]` TEXT), so folding to one
        // truncated block is shape-preserving — and `text_chars` would
        // under-count anything else.
        debug_assert!(
            content
                .iter()
                .all(|b| matches!(b, ContentBlock::Text { .. })),
            "advisory-view messages must be text-only before budgeting"
        );
        let joined = content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        *content = vec![ContentBlock::Text {
            text: truncate_tool_result(&joined, allowance),
            cache_control: None,
        }];
    }
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
        let content = content_mut(msg);
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
    fn empty_user_turn_is_dropped() {
        // Round-6 G4: a blank user turn must not reach the view. An empty text
        // block clears anthropic's `blocks.is_empty()` fallback (that only
        // catches a wholly empty content vec) and 400s on the wire, failing
        // every advisor at once.
        let msgs = vec![
            UnifiedMessage::user("real question"),
            UnifiedMessage::User { content: vec![] },
            UnifiedMessage::User {
                content: vec![ContentBlock::Text {
                    text: "   ".into(),
                    cache_control: None,
                }],
            },
        ];
        let view = build_advisory_view(&msgs);
        assert_eq!(
            view_texts(&view),
            vec![("user", "real question".to_string())]
        );
    }

    #[test]
    fn all_empty_input_yields_instruction_turn() {
        // Terminal guarantee: the view is never empty and never carries an
        // empty text block — zero-message advisor calls 4xx on every provider.
        for msgs in [
            vec![],
            vec![UnifiedMessage::User { content: vec![] }],
            vec![UnifiedMessage::Assistant { content: vec![] }],
        ] {
            let view = build_advisory_view(&msgs);
            assert_eq!(
                view_texts(&view),
                vec![("user", ADVISORY_INSTRUCTION.to_string())]
            );
        }
    }

    /// `turns` alternating user/assistant messages of `chars` characters
    /// each. Use an ODD count so the transcript ends on a user turn and
    /// `build_advisory_view` appends no synthetic instruction — keeping the
    /// "newest message" of the budget tests a real, large one.
    fn long_conversation(turns: usize, chars: usize) -> Vec<UnifiedMessage> {
        (0..turns)
            .map(|i| {
                let text = format!("{i}").repeat(chars);
                if i % 2 == 0 {
                    UnifiedMessage::user(text)
                } else {
                    UnifiedMessage::assistant(text)
                }
            })
            .collect()
    }

    #[test]
    fn view_budget_is_noop_under_budget() {
        let mut view = build_advisory_view(&long_conversation(6, 10));
        let before = view.clone();
        apply_view_budget(&mut view, ADVISORY_VIEW_BUDGET);
        assert_eq!(view_texts(&view), view_texts(&before));
    }

    #[test]
    fn view_budget_shrinks_oldest_and_preserves_roles() {
        // Round-6 G3: shrink, never drop — message count, order and role
        // sequence must survive so the "first message must be user" and
        // alternation rules cannot break.
        let mut view = build_advisory_view(&long_conversation(9, 5000));
        let before = view_texts(&view);
        apply_view_budget(&mut view, 12_000);
        let after = view_texts(&view);

        assert_eq!(before.len(), after.len(), "no message may be dropped");
        for (b, a) in before.iter().zip(after.iter()) {
            assert_eq!(b.0, a.0, "roles must be preserved");
            assert!(!a.1.is_empty(), "no message may shrink to an empty block");
        }
        // Newest keeps its full text; the oldest is elided.
        assert_eq!(before.last().unwrap().1, after.last().unwrap().1);
        assert!(after[0].1.contains("chars omitted"), "{}", after[0].1);
        // Bounded: budget plus the per-message elision markers left behind.
        let total: usize = after.iter().map(|(_, t)| t.chars().count()).sum();
        assert!(total < 12_000 + 100 * after.len(), "total was {total}");
    }

    #[test]
    fn view_budget_keeps_newest_when_it_alone_busts_the_budget() {
        // The live state is the one thing worth keeping: a single oversized
        // newest message is clamped to the whole budget, not to the stub
        // allowance the older messages get.
        let mut view = build_advisory_view(&long_conversation(5, 5000));
        apply_view_budget(&mut view, 1_000);
        let after = view_texts(&view);
        assert!(
            after.last().unwrap().1.chars().count() > 900,
            "newest kept {} chars",
            after.last().unwrap().1.chars().count()
        );
        assert!(after[0].1.chars().count() < 500);
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
