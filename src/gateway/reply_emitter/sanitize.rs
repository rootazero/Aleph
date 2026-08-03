use regex::Regex;
use std::borrow::Cow;
use std::sync::LazyLock;

// ── LLM output sanitization ────────────────────────────────────────────────

/// Strip LLM-internal tags that should never reach the user or TTS engine.
///
/// Removes:
/// - `<think|thinking|thought|antthinking>…</…>` — chain-of-thought reasoning
/// - `<completion-check>…</completion-check>` — agent loop completion markers
/// - `<task-complete/>` — agent loop task boundary
/// - Trailing incomplete tags (e.g. `[[`, `<completion-check`)
///
/// **Code-block aware**: tags inside backtick spans or fenced code blocks are
/// preserved, preventing accidental stripping of example/documentation code.
///
/// Returns `Cow::Borrowed` when no tags are found (zero-alloc fast path).
pub(crate) fn sanitize_llm_output(text: &str) -> Cow<'_, str> {
    // Fast path: quick probe for any tag-like pattern before doing real work.
    static QUICK_PROBE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"<(?:think|thinking|thought|antthinking|completion-check|task-complete|memory-context)[\s/>]",
        )
            .unwrap_or_else(|_| unreachable!("quick probe regex is statically valid"))
    });
    static BLANK_LINES_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\n{3,}")
            .unwrap_or_else(|_| unreachable!("blank-lines regex is statically valid"))
    });

    let has_tags = QUICK_PROBE.is_match(text);
    let has_trailing = text.ends_with("[[") || text.ends_with('[');
    // Check for trailing incomplete tag: last '<' has no closing '>'
    let has_incomplete_tag = text
        .rfind('<')
        .is_some_and(|pos| !text[pos..].contains('>'));

    if !has_tags && !has_trailing && !has_incomplete_tag {
        return Cow::Borrowed(text);
    }

    let stripped = strip_tags_code_aware(text);

    // Clean trailing incomplete directives (e.g. "answer text[[" or "<completion-check")
    let cleaned = strip_trailing_incomplete(&stripped);

    let collapsed = BLANK_LINES_RE.replace_all(&cleaned, "\n\n");
    Cow::Owned(collapsed.trim().to_string())
}

/// Tag names that should be stripped (all ASCII, case-insensitive).
const THINKING_TAGS: &[&str] = &["think", "thinking", "thought", "antthinking"];
/// Non-reasoning spans the model may echo that must not reach a user.
///
/// `memory-context` is the fence `memory::assembler::context_block::wrap_memory_context`
/// puts around recalled long-term memory. The live stream already discards it
/// (`streaming_scrubber::DISCARD_TAG_PAIRS`, applied by `MessageAssembler`), but
/// the terminal answer is raw model text — `RunSummary.final_response` is copied
/// verbatim from `content.text` — and this list is the only thing standing
/// between it and every terminal surface. It was missing here, so a model that
/// echoed the fence had the recalled memory scrubbed from the live bubble and
/// then written back over it by `finalize_answer`, and posted to Telegram /
/// Slack / the group transcript / cron results. Exactly the `<think>`
/// resurrection Round-4 closed, one tag later.
///
/// `discard_tag_pairs_are_all_stripped_from_the_terminal_answer` keeps the two
/// lists from drifting apart again.
const OTHER_STRIP_TAGS: &[&str] = &["completion-check", "memory-context"];

/// Strip tags while respecting code block boundaries.
///
/// Operates on `&[u8]` byte slices (all tag names are ASCII) to avoid the
/// `Vec<char>` allocation of the previous implementation. Supports fenced
/// code blocks (```) and multi-backtick inline code spans (`` ` ``, ``` `` ```).
fn strip_tags_code_aware(text: &str) -> String {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut result = String::with_capacity(len);
    let mut i = 0;
    let mut in_fenced_code = false;
    // For inline code: 0 = not in code, N = need N backticks to close
    let mut inline_backtick_count: usize = 0;

    while i < len {
        // Track fenced code blocks (3+ backticks at line start or after whitespace)
        if inline_backtick_count == 0
            && i + 2 < len
            && bytes[i] == b'`'
            && bytes[i + 1] == b'`'
            && bytes[i + 2] == b'`'
        {
            // Count consecutive backticks
            let fence_start = i;
            while i < len && bytes[i] == b'`' {
                i += 1;
            }
            result.push_str(&text[fence_start..i]);
            in_fenced_code = !in_fenced_code;
            continue;
        }

        // Track inline code spans (1 or 2 backticks)
        if !in_fenced_code && bytes[i] == b'`' {
            let bt_start = i;
            let mut bt_count = 0;
            while i < len && bytes[i] == b'`' && bt_count < 3 {
                bt_count += 1;
                i += 1;
            }
            // 3+ backticks handled above; here we have 1-2 backticks
            result.push_str(&text[bt_start..i]);
            if inline_backtick_count == 0 {
                // Opening: need matching count to close
                inline_backtick_count = bt_count;
            } else if bt_count == inline_backtick_count {
                // Closing: matched
                inline_backtick_count = 0;
            }
            // else: different count inside span, just content
            continue;
        }

        // Inside code — pass through unchanged
        if in_fenced_code || inline_backtick_count > 0 {
            // Fast: copy to next backtick or end
            let start = i;
            while i < len && bytes[i] != b'`' {
                i += 1;
            }
            result.push_str(&text[start..i]);
            continue;
        }

        // Outside code — check for tags to strip
        if bytes[i] == b'<' {
            // Try self-closing: <task-complete /> or <task-complete/>
            if let Some(end) = try_match_self_closing_bytes(bytes, i, b"task-complete") {
                i = end;
                continue;
            }

            // Try paired tags
            let mut matched = false;
            for tag in THINKING_TAGS.iter().chain(OTHER_STRIP_TAGS.iter()) {
                if let Some(end) = try_skip_paired_tag_bytes(bytes, i, tag.as_bytes()) {
                    i = end;
                    matched = true;
                    break;
                }
            }
            if matched {
                continue;
            }
        }

        // Copy one UTF-8 character (may be multi-byte)
        let ch_len = utf8_char_len(bytes[i]);
        let end = (i + ch_len).min(len);
        result.push_str(&text[i..end]);
        i = end;
    }

    result
}

/// Try to match `<tag_name>…</tag_name>` at byte position `pos`.
/// Returns the byte position after the closing tag, or None.
fn try_skip_paired_tag_bytes(bytes: &[u8], pos: usize, tag: &[u8]) -> Option<usize> {
    let open_len = 1 + tag.len() + 1; // < + tag + >
    if pos + open_len > bytes.len() || bytes[pos] != b'<' {
        return None;
    }
    // Match opening tag (case-insensitive)
    if !bytes[pos + 1..pos + 1 + tag.len()]
        .iter()
        .zip(tag.iter())
        .all(|(a, b)| a.to_ascii_lowercase() == *b)
    {
        return None;
    }
    if bytes[pos + 1 + tag.len()] != b'>' {
        return None;
    }

    // Find closing </tag>
    let close_tag_len = 2 + tag.len() + 1; // </ + tag + >
    let search_start = pos + open_len;
    for k in search_start..bytes.len().saturating_sub(close_tag_len - 1) {
        if bytes[k] == b'<'
            && bytes[k + 1] == b'/'
            && k + close_tag_len <= bytes.len()
            && bytes[k + 2..k + 2 + tag.len()]
                .iter()
                .zip(tag.iter())
                .all(|(a, b)| a.to_ascii_lowercase() == *b)
            && bytes[k + 2 + tag.len()] == b'>'
        {
            return Some(k + close_tag_len);
        }
    }
    None // No closing tag — don't strip
}

/// Try to match `<tag_name/>` or `<tag_name />` at byte position `pos`.
fn try_match_self_closing_bytes(bytes: &[u8], pos: usize, tag: &[u8]) -> Option<usize> {
    if bytes[pos] != b'<' {
        return None;
    }
    let mut j = pos + 1;
    for &t in tag {
        if j >= bytes.len() || bytes[j].to_ascii_lowercase() != t {
            return None;
        }
        j += 1;
    }
    // Skip optional spaces
    while j < bytes.len() && bytes[j] == b' ' {
        j += 1;
    }
    // Must end with />
    if j + 1 < bytes.len() && bytes[j] == b'/' && bytes[j + 1] == b'>' {
        Some(j + 2)
    } else {
        None
    }
}

/// Length of a UTF-8 character from its leading byte.
const fn utf8_char_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b < 0xE0 {
        2
    } else if b < 0xF0 {
        3
    } else {
        4
    }
}

/// Strip trailing incomplete directives that LLMs sometimes emit at stream end.
fn strip_trailing_incomplete(text: &str) -> String {
    let mut s = text.to_string();

    // Strip trailing "[[" or "[" (incomplete wiki-link / directive)
    while s.ends_with("[[") || s.ends_with('[') {
        s.pop();
    }

    // Strip trailing incomplete opening tag (e.g. "<completion-check" without ">")
    // Only strip if it looks like a tag start (<letter or </), not math like "< 10"
    if let Some(last_lt) = s.rfind('<') {
        let tail = &s[last_lt..];
        if !tail.contains('>') {
            let after_lt = tail.as_bytes().get(1);
            let looks_like_tag = after_lt.is_some_and(|&b| b.is_ascii_alphabetic() || b == b'/');
            if looks_like_tag {
                s.truncate(last_lt);
            }
        }
    }

    s
}

/// Split text into (reasoning, answer) by extracting `<think>…</think>` blocks.
///
/// Code-block aware: tags inside backtick spans or fenced code blocks are
/// preserved, preventing accidental extraction from example code.
pub(crate) fn split_reasoning(text: &str) -> (Option<String>, String) {
    static QUICK_PROBE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)<(?:think|thinking|thought|antthinking)[\s/>]")
            .unwrap_or_else(|_| unreachable!("quick probe regex is statically valid"))
    });

    if !QUICK_PROBE.is_match(text) {
        return (None, text.to_string());
    }

    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut reasoning_parts: Vec<String> = Vec::new();
    let mut answer = String::with_capacity(len);
    let mut i = 0;
    let mut in_fenced_code = false;
    let mut inline_backtick_count: usize = 0;

    while i < len {
        // Track fenced code blocks
        if inline_backtick_count == 0
            && i + 2 < len
            && bytes[i] == b'`'
            && bytes[i + 1] == b'`'
            && bytes[i + 2] == b'`'
        {
            let fence_start = i;
            while i < len && bytes[i] == b'`' {
                i += 1;
            }
            answer.push_str(&text[fence_start..i]);
            in_fenced_code = !in_fenced_code;
            continue;
        }

        // Track inline code spans
        if !in_fenced_code && bytes[i] == b'`' {
            let bt_start = i;
            let mut bt_count = 0;
            while i < len && bytes[i] == b'`' && bt_count < 3 {
                bt_count += 1;
                i += 1;
            }
            answer.push_str(&text[bt_start..i]);
            if inline_backtick_count == 0 {
                inline_backtick_count = bt_count;
            } else if bt_count == inline_backtick_count {
                inline_backtick_count = 0;
            }
            continue;
        }

        // Inside code — pass through to answer
        if in_fenced_code || inline_backtick_count > 0 {
            let start = i;
            while i < len && bytes[i] != b'`' {
                i += 1;
            }
            answer.push_str(&text[start..i]);
            continue;
        }

        // Outside code — check for thinking tags to extract
        if bytes[i] == b'<' {
            let mut matched = false;
            for tag in THINKING_TAGS.iter() {
                if let Some((content, end)) = try_extract_paired_tag_bytes(bytes, i, tag.as_bytes())
                {
                    if !content.is_empty() {
                        reasoning_parts.push(content);
                    }
                    i = end;
                    matched = true;
                    break;
                }
            }
            if matched {
                // Re-run the loop from the new position so an immediately
                // adjacent tag (e.g. `</think><think>`) is also extracted.
                continue;
            }
            // No match — copy '<' and continue normally
            answer.push('<');
            i += 1;
            continue;
        }

        // Copy one UTF-8 character
        let ch_len = utf8_char_len(bytes[i]);
        let end = (i + ch_len).min(len);
        answer.push_str(&text[i..end]);
        i = end;
    }

    let reasoning = if reasoning_parts.is_empty() {
        None
    } else {
        Some(reasoning_parts.join("\n\n"))
    };

    (reasoning, answer)
}

/// Try to extract `<tag_name>…</tag_name>` at byte position `pos`.
/// Returns the extracted content and the byte position after the closing tag.
fn try_extract_paired_tag_bytes(bytes: &[u8], pos: usize, tag: &[u8]) -> Option<(String, usize)> {
    let open_len = 1 + tag.len() + 1; // < + tag + >
    if pos + open_len > bytes.len() || bytes[pos] != b'<' {
        return None;
    }
    // Match opening tag (case-insensitive)
    if !bytes[pos + 1..pos + 1 + tag.len()]
        .iter()
        .zip(tag.iter())
        .all(|(a, b)| a.to_ascii_lowercase() == *b)
    {
        return None;
    }
    if bytes[pos + 1 + tag.len()] != b'>' {
        return None;
    }

    // Find closing </tag>
    let close_tag_len = 2 + tag.len() + 1; // </ + tag + >
    let search_start = pos + open_len;
    for k in search_start..bytes.len().saturating_sub(close_tag_len - 1) {
        if bytes[k] == b'<'
            && bytes[k + 1] == b'/'
            && k + close_tag_len <= bytes.len()
            && bytes[k + 2..k + 2 + tag.len()]
                .iter()
                .zip(tag.iter())
                .all(|(a, b)| a.to_ascii_lowercase() == *b)
            && bytes[k + 2 + tag.len()] == b'>'
        {
            let content = String::from_utf8_lossy(&bytes[search_start..k])
                .trim()
                .to_string();
            return Some((content, k + close_tag_len));
        }
    }
    None
}
