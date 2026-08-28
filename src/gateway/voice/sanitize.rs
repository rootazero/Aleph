//! TTS text sanitization for outbound voice replies.
//!
//! The `VoiceModeLayer` *asks* the model to avoid markdown, code, and raw URLs,
//! but R7/P7 forbid trusting the model to comply: a stray `**bold**`, a fenced
//! code block, or a bare `https://…` makes the TTS engine read punctuation and
//! URL characters aloud one by one. This pass defensively strips speech-hostile
//! syntax just before synthesis and clamps the result to the provider's hard
//! character ceiling (`OpenAI` TTS rejects > 4096 chars outright), truncating on a
//! sentence/word boundary so a long reply degrades gracefully instead of erroring.
//!
//! Inspired by hermes-agent's pre-TTS markdown stripping, re-expressed without
//! a regex engine — fixed-shape markdown markers are cheap to strip by hand.

/// Hard upper bound on characters sent to a TTS provider. `OpenAI`'s `/audio/speech`
/// rejects inputs above 4096; we leave a small margin and truncate gracefully.
pub const MAX_TTS_CHARS: usize = 4000;

/// Strip a leading/trailing run of a markdown marker from inline spans.
fn strip_inline_markers(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // Emphasis / strong / strikethrough markers — drop entirely.
            '*' | '_' | '~' | '`' => continue,
            // Image prefix `![alt](url)` — drop the `!`, let `[` handle the rest.
            '!' if chars.peek() == Some(&'[') => continue,
            // Markdown link `[label](url)` → keep just the label.
            '[' => {
                let label: String = chars.by_ref().take_while(|&ch| ch != ']').collect();
                // Consume an immediately-following `(...)` target if present.
                if chars.peek() == Some(&'(') {
                    chars.next();
                    for ch in chars.by_ref() {
                        if ch == ')' {
                            break;
                        }
                    }
                }
                out.push_str(&label);
            }
            _ => out.push(c),
        }
    }
    out
}

/// True for a markdown table separator row like `|---|:--:|` or `--- | ---`,
/// which carries no spoken content and would otherwise be voiced as a run of
/// "dash" / "vertical bar".
fn is_table_separator(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && t.contains('-') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

/// Flatten a pipe-delimited table row (`| Name | Age |`) into a spoken comma
/// list ("Name, Age"). Lines without a `|` pass through untouched. The
/// `VoiceModeLayer` asks the model to avoid tables, but per R7/P7 we don't trust
/// compliance — an emitted table must still read cleanly aloud.
fn flatten_table_row(line: &str) -> String {
    if !line.contains('|') {
        return line.to_string();
    }
    line.split('|')
        .map(str::trim)
        .filter(|cell| !cell.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Replace bare `http(s)://…` URLs with a short spoken placeholder so the engine
/// does not spell the address out character by character.
fn replace_urls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for token in text.split_inclusive(char::is_whitespace) {
        let trimmed = token.trim_end();
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            out.push_str("(link)");
            // Preserve the trailing whitespace that `split_inclusive` kept.
            out.push_str(&token[trimmed.len()..]);
        } else {
            out.push_str(token);
        }
    }
    out
}

/// Truncate to at most `MAX_TTS_CHARS`, preferring the last sentence end, then
/// the last word boundary, so a clamped reply still ends cleanly.
fn clamp_length(text: &str) -> String {
    if text.chars().count() <= MAX_TTS_CHARS {
        return text.to_string();
    }
    // Char-safe prefix (P7 UTF-8 safety — no byte slicing).
    let prefix: String = text.chars().take(MAX_TTS_CHARS).collect();
    if let Some(idx) = prefix.rfind(['.', '!', '?', '。', '！', '？']) {
        // `rfind` returns the *start* byte of the matched char; include the
        // whole char so a multibyte terminator (。！？) keeps a valid boundary.
        let end = idx + prefix[idx..].chars().next().map_or(1, char::len_utf8);
        return prefix[..end].trim_end().to_string();
    }
    if let Some(idx) = prefix.rfind(char::is_whitespace) {
        return prefix[..idx].trim_end().to_string();
    }
    prefix
}

/// Strip `<think>…</think>` / `<thinking>…</thinking>` blocks. Reasoning-capable
/// models backing voice replies would otherwise have their chain-of-thought
/// read aloud verbatim. An unclosed opener strips to the end (a truncated
/// thinking block is still not speech).
fn strip_think_blocks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        // Find the earliest opener of either spelling. `<thinking>` is listed
        // first: at the same index it must win over its own prefix `<think>`
        // (min_by_key keeps the first of equal keys).
        let open = ["<thinking>", "<think>"]
            .iter()
            .filter_map(|tag| rest.find(tag).map(|i| (i, *tag)))
            .min_by_key(|(i, _)| *i);
        let Some((start, tag)) = open else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..start]);
        let close = if tag == "<think>" {
            "</think>"
        } else {
            "</thinking>"
        };
        match rest[start..].find(close) {
            Some(rel_end) => rest = &rest[start + rel_end + close.len()..],
            None => return out, // unclosed → drop the tail
        }
    }
}

/// Sanitize reply text for speech synthesis: drop thinking blocks and code
/// fences, strip inline markdown markers, collapse blank runs, soften bare
/// URLs, and clamp length.
///
/// # Empty-output contract
///
/// A reply consisting entirely of stripped content (code fences, thinking
/// blocks, table separators, or emoji-only text with no speakable words)
/// sanitizes to an EMPTY string. Callers MUST check for this before invoking
/// a TTS provider — `voice/outbound.rs` already does
/// (`if spoken.trim().is_empty() { return None }` and the reply falls back
/// to text), and any new call site must do the same: feeding a zero-length
/// string to a provider either errors with a confusing message or silently
/// synthesizes nothing.
#[must_use]
pub fn sanitize_for_tts(text: &str) -> String {
    let text = strip_think_blocks(text);
    let mut lines: Vec<String> = Vec::new();
    let mut in_code_fence = false;
    for raw in text.lines() {
        let trimmed = raw.trim_end();
        if trimmed.trim_start().starts_with("```") {
            in_code_fence = !in_code_fence;
            continue; // drop the fence marker line itself
        }
        if in_code_fence {
            continue; // drop fenced code contents
        }
        // Strip leading markdown structure: heading hashes, blockquote markers,
        // and list bullets — keep the spoken content.
        let mut content = trimmed.trim_start();
        content = content.trim_start_matches('#').trim_start();
        content = content.trim_start_matches('>').trim_start();
        if let Some(rest) = content.strip_prefix("- ") {
            content = rest;
        } else if let Some(rest) = content.strip_prefix("* ") {
            content = rest;
        }
        // Tables read terribly aloud — drop separator rows entirely and flatten
        // pipe-delimited rows into a spoken comma list before inline markers are
        // stripped (so link labels inside cells survive).
        if is_table_separator(content) {
            continue;
        }
        let content = flatten_table_row(content);
        lines.push(strip_inline_markers(&content));
    }

    // Collapse consecutive blank lines into a single paragraph break.
    let mut collapsed: Vec<String> = Vec::with_capacity(lines.len());
    let mut prev_blank = false;
    for line in lines {
        let blank = line.trim().is_empty();
        if blank && prev_blank {
            continue;
        }
        prev_blank = blank;
        collapsed.push(line);
    }

    let joined = collapsed.join("\n");
    let urls_softened = replace_urls(joined.trim());
    clamp_length(&urls_softened)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_inline_markdown() {
        assert_eq!(sanitize_for_tts("**bold** and _italic_"), "bold and italic");
        assert_eq!(
            sanitize_for_tts("use `cargo build` now"),
            "use cargo build now"
        );
    }

    #[test]
    fn drops_code_fences() {
        let input = "Here is code:\n```rust\nfn main() {}\n```\nDone.";
        let out = sanitize_for_tts(input);
        assert!(out.contains("Here is code:"));
        assert!(out.contains("Done."));
        assert!(!out.contains("fn main"));
        assert!(!out.contains("```"));
    }

    #[test]
    fn keeps_link_label_drops_target() {
        assert_eq!(
            sanitize_for_tts("see [the docs](https://x.com/y)"),
            "see the docs"
        );
    }

    #[test]
    fn softens_bare_url() {
        assert_eq!(
            sanitize_for_tts("open https://example.com/path now"),
            "open (link) now"
        );
    }

    #[test]
    fn flattens_table_into_spoken_list() {
        let input = "| Name | Age |\n|------|-----|\n| Alice | 30 |";
        let out = sanitize_for_tts(input);
        // Separator row is gone; content rows read as comma lists.
        assert!(!out.contains('|'));
        assert!(!out.contains("---"));
        assert!(out.contains("Name, Age"));
        assert!(out.contains("Alice, 30"));
    }

    #[test]
    fn table_cell_keeps_link_label() {
        let input = "| [docs](https://x.com) | ok |";
        assert_eq!(sanitize_for_tts(input), "docs, ok");
    }

    #[test]
    fn prose_without_pipe_is_untouched() {
        assert_eq!(
            sanitize_for_tts("just a plain sentence"),
            "just a plain sentence"
        );
    }

    #[test]
    fn strips_heading_and_bullet() {
        assert_eq!(sanitize_for_tts("# Title"), "Title");
        assert_eq!(sanitize_for_tts("- item one"), "item one");
    }

    #[test]
    fn clamps_long_text_on_sentence_boundary() {
        let sentence = "This is a sentence. ";
        let long = sentence.repeat(300); // ~6000 chars
        let out = sanitize_for_tts(&long);
        assert!(out.chars().count() <= MAX_TTS_CHARS);
        assert!(out.ends_with('.'));
    }

    #[test]
    fn short_text_passes_through() {
        assert_eq!(sanitize_for_tts("Hello there"), "Hello there");
    }

    #[test]
    fn think_blocks_are_never_spoken() {
        assert_eq!(
            sanitize_for_tts("<think>internal reasoning</think>好的，已经安排。"),
            "好的，已经安排。"
        );
        assert_eq!(
            sanitize_for_tts("<thinking>step 1… step 2…</thinking>Done."),
            "Done."
        );
        // Unclosed block (truncated stream) drops the tail rather than
        // reading half a chain-of-thought aloud.
        assert_eq!(
            sanitize_for_tts("回答在这。<think>oops truncated"),
            "回答在这。"
        );
    }

    #[test]
    fn utf8_safe_clamp() {
        let long = "中文句子。".repeat(1000); // multibyte, > MAX_TTS_CHARS
        let out = sanitize_for_tts(&long);
        assert!(out.chars().count() <= MAX_TTS_CHARS);
        // Must not panic and must end cleanly.
        assert!(out.ends_with('。'));
    }
}
