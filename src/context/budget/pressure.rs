//! Content-aware token-ratio detection for context-pressure estimation.

use crate::providers::message::{ContentBlock, UnifiedMessage};

// =============================================================================
// Content-aware ratio detection
// =============================================================================

/// Returns true if the character is in a CJK Unicode range.
const fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{4E00}'..='\u{9FFF}'   // CJK Unified Ideographs
        | '\u{3400}'..='\u{4DBF}' // CJK Extension A
        | '\u{20000}'..='\u{2A6DF}' // CJK Extension B
        | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
        | '\u{2F800}'..='\u{2FA1F}' // CJK Compatibility Supplement
        | '\u{3000}'..='\u{303F}' // CJK Symbols and Punctuation
        | '\u{FF00}'..='\u{FFEF}' // Halfwidth/Fullwidth Forms
        | '\u{3040}'..='\u{309F}' // Hiragana
        | '\u{30A0}'..='\u{30FF}' // Katakana
        | '\u{AC00}'..='\u{D7AF}' // Hangul Syllables
    )
}

/// Heuristic code detection.
///
/// Samples the first 20 lines, checks for language keywords/operators,
/// and returns true if more than 40% of lines look like code.
fn looks_like_code(text: &str) -> bool {
    let code_indicators = [
        "fn ",
        "let ",
        "mut ",
        "pub ",
        "impl ",
        "struct ",
        "enum ",
        "trait ", // Rust
        "def ",
        "class ",
        "import ",
        "from ",
        "return ",
        "if ",
        "else:",
        "for ", // Python
        "function ",
        "const ",
        "var ",
        "=>",
        "===",
        "!==", // JS/TS
        "int ",
        "void ",
        "return;",
        "#include",
        "using namespace", // C/C++
        "func ",
        "package ",
        "interface ",
        "go ", // Go
        "{",
        "}",
        "//",
        "/*",
        "*/", // Common
        "->",
        "::",
        "&&",
        "||",
        "!=",
        "==",
        "<=",
        ">=", // Operators
    ];

    let mut line_count = 0usize;
    let mut indicator_count = 0usize;
    for line in text.lines().take(20) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        line_count += 1;
        if code_indicators
            .iter()
            .any(|indicator| trimmed.contains(indicator))
        {
            indicator_count += 1;
        }
    }

    if line_count == 0 {
        return false;
    }

    (indicator_count as f64 / line_count as f64) > 0.40
}

/// Default chars-per-token ratio for English prose. Used as the prose anchor by
/// [`detect_content_ratio`] and as the implicit baseline for [`estimate_tokens_smart`].
pub const DEFAULT_PROSE_RATIO: f64 = 3.5;
/// Chars-per-token ratio for CJK-dominant text (denser per character).
const CJK_RATIO: f64 = 1.5;
/// Chars-per-token ratio for source code (symbol-dense, denser than prose).
const CODE_RATIO: f64 = 2.5;

/// Content-aware chars-per-token ratio, using `prose_ratio` as the baseline for
/// ordinary (non-CJK, non-code) text.
///
/// Rather than flipping the *entire* text to one ratio at a 30%-CJK threshold,
/// this charges each character class at its own density and folds the result
/// back into a single effective chars-per-token ratio:
///
/// - CJK characters are charged at [`CJK_RATIO`] (token-dense).
/// - The remaining (Latin/symbol) characters are charged at [`CODE_RATIO`] when
///   the text looks like source, otherwise at the caller's `prose_ratio`.
///
/// The proportional blend matters for *mixed* content — bilingual CN/EN prose,
/// or code with CJK comments — which the old hard threshold mis-sized: a 40%
/// CJK / 60% Latin message was charged entirely at the CJK ratio, over-counting
/// the Latin portion ~2.3× and inflating the budget so compaction / session
/// split fired early and shed context prematurely. Pure-content inputs are
/// unchanged: pure CJK still collapses to [`CJK_RATIO`], pure code to
/// [`CODE_RATIO`], and pure prose to `prose_ratio`, so every caller stays
/// byte-identical at the extremes while the mixed case gets the accurate
/// estimate. The budget's EWMA calibration then refines whatever residual error
/// remains. Keeping the caller's `prose_ratio` as the Latin/prose anchor means a
/// configured estimate ratio still governs ordinary text.
#[must_use]
pub fn content_ratio_with_baseline(text: &str, prose_ratio: f64) -> f64 {
    if text.is_empty() {
        return prose_ratio;
    }

    // Single pass: total chars and the CJK subset (avoids the prior two scans).
    let mut total_chars = 0usize;
    let mut cjk_count = 0usize;
    for c in text.chars() {
        total_chars += 1;
        if is_cjk(c) {
            cjk_count += 1;
        }
    }
    let non_cjk_count = total_chars - cjk_count;

    // Code detection is whole-text (sampled), so the non-CJK class gets one
    // density: the code ratio when the text looks like source, else the prose
    // anchor supplied by the caller.
    let non_cjk_ratio = if looks_like_code(text) {
        CODE_RATIO
    } else {
        prose_ratio
    };

    // Tokens contributed by each class. A non-positive `non_cjk_ratio` (only
    // reachable via an explicit `0.0` anchor) makes the non-CJK class infinite,
    // collapsing the effective ratio to `0.0` so the estimators' `ratio <= 0.0`
    // zero-guard fires — preserving the prior contract.
    let cjk_tokens = cjk_count as f64 / CJK_RATIO;
    let non_cjk_tokens = if non_cjk_count == 0 {
        0.0
    } else if non_cjk_ratio > 0.0 {
        non_cjk_count as f64 / non_cjk_ratio
    } else {
        f64::INFINITY
    };

    let total_tokens = cjk_tokens + non_cjk_tokens;
    if total_tokens <= 0.0 {
        return prose_ratio;
    }
    total_chars as f64 / total_tokens
}

/// Detects the content type and returns an effective chars-per-token ratio.
///
/// Charges CJK characters at `1.5`, source-code characters at `2.5`, and prose
/// at `3.5`, blending proportionally for mixed content (see
/// [`content_ratio_with_baseline`]). Pure CJK collapses to `1.5`, pure code to
/// `2.5`, and pure English prose to `3.5`.
///
/// This is [`content_ratio_with_baseline`] anchored at [`DEFAULT_PROSE_RATIO`].
#[must_use]
pub fn detect_content_ratio(text: &str) -> f64 {
    content_ratio_with_baseline(text, DEFAULT_PROSE_RATIO)
}

/// Estimates token count using content-aware ratio detection.
#[must_use]
pub fn estimate_tokens_smart(content: &str) -> usize {
    estimate_tokens_aware(content, DEFAULT_PROSE_RATIO)
}

/// How many characters of text shaped like `sample` fit in `budget_tokens` —
/// the inverse of [`estimate_tokens_smart`].
///
/// Producers that must *emit* text under a token budget (a windowed file read,
/// a head/tail truncation) need this direction, and doing the arithmetic at the
/// call site is how char and byte units get mixed up. Content-aware by
/// construction: the same budget buys ~2.5× more characters of source than of
/// CJK prose, which is exactly why a fixed "50 KB per read" limit cannot honour
/// a token budget for both.
#[must_use]
pub fn chars_for_token_budget(sample: &str, budget_tokens: usize) -> usize {
    let ratio = detect_content_ratio(sample).max(1.0);
    ((budget_tokens as f64) * ratio) as usize
}

/// Estimates token count using content-aware ratio detection with an explicit
/// prose baseline.
///
/// Like [`estimate_tokens_smart`] but lets the caller supply the prose
/// chars-per-token anchor (e.g. a configured `token_estimate_ratio`), while CJK
/// and code content still override with their denser ratios. Counts Unicode
/// scalar values, not UTF-8 bytes, so CJK text is not over-counted ~3×.
#[must_use]
pub fn estimate_tokens_aware(content: &str, prose_ratio: f64) -> usize {
    let ratio = content_ratio_with_baseline(content, prose_ratio);
    if ratio <= 0.0 {
        return 0;
    }
    // ratio = chars per token, so tokens = chars / ratio
    let chars = content.chars().count();
    ((chars as f64) / ratio).ceil() as usize
}

/// Estimated provider-side token cost of a single inline image block.
///
/// Matches Anthropic's tile-based image pricing and the Hermes constant. This
/// is the single source of truth shared by the budget sensor (which must count
/// images toward pressure) and the historical-image-stripping preflight stage
/// (which frees exactly this many tokens per image it drops) — so the sensor
/// and the stripper agree on what an image costs.
pub const IMAGE_TOKENS_ESTIMATE: usize = 1500;

/// Append `part` to `buf`, space-separating from any prior content. Mirrors
/// `UnifiedMessage::text_content`'s `parts.join(" ")` for non-empty parts, so a
/// single-class message stays byte-identical to the old flattened estimate.
fn push_part(buf: &mut String, part: &str) {
    if !buf.is_empty() {
        buf.push(' ');
    }
    buf.push_str(part);
}

/// Content-aware token estimate for a whole message, charging each block class
/// at its own density.
///
/// Blocks split into three accounting classes instead of being flattened into
/// one prose-anchored string (the old `text_content()` path):
///
/// - **Prose** ([`ContentBlock::Text`] + [`ContentBlock::Thinking`]): estimated
///   via [`estimate_tokens_aware`] at the caller's `prose_ratio`, so CJK/code
///   density still applies inside natural-language blocks.
/// - **Structured** ([`ContentBlock::Json`] tool output + [`ContentBlock::ToolCall`]
///   `name`+`arguments`): estimated at the denser [`CODE_RATIO`] anchor. JSON is
///   symbol-dense (`{`, `"`, `:`, `,`) and tokenizes ~like source, but the old
///   path charged it at the 3.5 prose ratio — `looks_like_code` misses pure JSON
///   (braces only bookend it), so it never tripped the code ratio. Agentic loops
///   are dominated by tool calls and tool results, so this under-count made the
///   sensor systematically *under-report* pressure on exactly the tool-heavy
///   turns, firing compaction late. CJK *inside* structured values still gets
///   CJK density because the code ratio is only the non-CJK anchor.
/// - **Image** ([`ContentBlock::Image`]): a flat [`IMAGE_TOKENS_ESTIMATE`] per
///   block. `text_content()` omits images, so without this a multi-megabyte
///   screenshot counted as **zero tokens** and vision contexts under-reported
///   pressure by ~[`IMAGE_TOKENS_ESTIMATE`] per image.
///
/// A message with no structured or image blocks (ordinary chat: text/thinking
/// only) is byte-identical to the old
/// `estimate_tokens_aware(&msg.text_content(), prose_ratio)` — the empty
/// structured string contributes zero. The EWMA usage calibration then refines
/// whatever residual error remains; this just gives it a more accurate base so
/// it converges from the right side on tool-heavy and vision-heavy contexts.
#[must_use]
pub fn estimate_message_tokens_aware(msg: &UnifiedMessage, prose_ratio: f64) -> usize {
    let mut prose = String::new();
    let mut structured = String::new();
    let mut image_count = 0usize;
    for block in msg.content_blocks() {
        match block {
            ContentBlock::Text { text, .. } => push_part(&mut prose, text),
            ContentBlock::Thinking { thinking, .. } => push_part(&mut prose, thinking),
            ContentBlock::Json { value } => push_part(&mut structured, &value.to_string()),
            ContentBlock::ToolCall {
                name, arguments, ..
            } => push_part(&mut structured, &format!("{name} {arguments}")),
            ContentBlock::Image { .. } => image_count += 1,
        }
    }
    estimate_tokens_aware(&prose, prose_ratio)
        + estimate_tokens_aware(&structured, CODE_RATIO)
        + image_count * IMAGE_TOKENS_ESTIMATE
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_ratio_pure_english() {
        let ratio = detect_content_ratio("Hello world, this is a test message.");
        assert!((ratio - 3.5).abs() < 0.01);
    }

    #[test]
    fn detect_ratio_chinese_text() {
        // CJK-dominant text with a sprinkle of Latin ("token") blends to a ratio
        // near — but, after the proportional blend, no longer pinned exactly at —
        // the pure-CJK 1.5. It stays well below the prose anchor and below 2.0.
        let ratio = detect_content_ratio("这是一段中文文本，用于测试token估算比率。");
        assert!(
            (1.5..2.0).contains(&ratio),
            "CJK-dominant mixed text should blend just above pure-CJK 1.5, got {ratio}"
        );
    }

    #[test]
    fn detect_ratio_pure_cjk_is_exactly_cjk_ratio() {
        // Pure CJK (no Latin) must still collapse to the CJK ratio exactly — the
        // blend is byte-identical at the pure-content extremes.
        let ratio = detect_content_ratio("这是一段没有任何拉丁字符的纯中文文本内容");
        assert!(
            (ratio - 1.5).abs() < 0.01,
            "pure CJK must be 1.5, got {ratio}"
        );
    }

    #[test]
    fn blend_mixed_cjk_english_between_extremes() {
        // A balanced CN/EN mix must land strictly between the CJK ratio (1.5) and
        // the prose anchor (3.5) — the core accuracy fix. The old hard threshold
        // flipped the WHOLE string to 1.5 once CJK passed 30%, over-counting the
        // English half ~2.3×.
        let mixed = "the quick brown fox 这是一段用于测试的中文内容";
        let ratio = detect_content_ratio(mixed);
        assert!(
            ratio > 1.5 && ratio < 3.5,
            "mixed CN/EN must blend between 1.5 and 3.5, got {ratio}"
        );
    }

    #[test]
    fn blend_charges_latin_fewer_tokens_than_hard_flip() {
        // Regression guard for the over-counting bug: a mixed message must
        // estimate FEWER tokens than the old "treat everything as CJK density"
        // behaviour (chars / 1.5), because its Latin half is charged at the
        // sparser prose ratio.
        let mixed = "please review this pull request 请审查这个合并请求并给出反馈意见";
        let blended = estimate_tokens_aware(mixed, DEFAULT_PROSE_RATIO);
        let hard_flip_all_cjk = (mixed.chars().count() as f64 / 1.5).ceil() as usize;
        assert!(
            blended < hard_flip_all_cjk,
            "blended estimate ({blended}) must undercut the all-CJK flip ({hard_flip_all_cjk})"
        );
    }

    #[test]
    fn detect_ratio_code_content() {
        let code = "fn main() {\n    let x = vec![1, 2, 3];\n    println!(\"{:?}\", x);\n}";
        let ratio = detect_content_ratio(code);
        assert!((ratio - 2.5).abs() < 0.01);
    }

    #[test]
    fn detect_ratio_empty_string() {
        let ratio = detect_content_ratio("");
        assert!((ratio - 3.5).abs() < 0.01);
    }

    #[test]
    fn detect_ratio_mixed_content() {
        // ~40% CJK should trigger CJK ratio
        let mixed = "Hello world 这是中文 this is mixed 混合内容测试文本比较多";
        let ratio = detect_content_ratio(mixed);
        assert!(
            ratio < 3.5,
            "mixed with >30% CJK should use lower ratio, got {ratio}"
        );
    }

    // --- content_ratio_with_baseline ---

    #[test]
    fn baseline_governs_prose_only() {
        // Prose honors the caller's anchor instead of the hardcoded 3.5.
        assert!(
            (content_ratio_with_baseline("just some plain english prose", 4.0) - 4.0).abs() < 0.01
        );
        // Empty text falls back to the supplied baseline, not 3.5.
        assert!((content_ratio_with_baseline("", 4.0) - 4.0).abs() < 0.01);
    }

    #[test]
    fn baseline_overridden_by_cjk_and_code() {
        // CJK and code override the baseline with their denser ratios regardless
        // of the caller's prose anchor.
        assert!(
            (content_ratio_with_baseline("这是一段中文文本用于测试比率检测", 4.0) - 1.5).abs()
                < 0.01
        );
        let code = "fn main() {\n    let x = vec![1, 2, 3];\n    println!(\"{:?}\", x);\n}";
        assert!((content_ratio_with_baseline(code, 4.0) - 2.5).abs() < 0.01);
    }

    #[test]
    fn detect_content_ratio_is_baseline_at_default() {
        // detect_content_ratio must remain exactly content_ratio_with_baseline @ 3.5.
        for s in [
            "Hello world",
            "这是中文文本测试内容比较多一些字符",
            "fn x() -> i32 { let y = 1; y + 2 }",
            "",
        ] {
            assert_eq!(
                detect_content_ratio(s),
                content_ratio_with_baseline(s, DEFAULT_PROSE_RATIO)
            );
        }
    }

    // --- estimate_tokens_aware ---

    #[test]
    fn estimate_tokens_smart_is_aware_at_default() {
        // estimate_tokens_smart must remain exactly estimate_tokens_aware @ 3.5.
        for s in ["Hello world", "这是中文", "fn x() {}", ""] {
            assert_eq!(
                estimate_tokens_smart(s),
                estimate_tokens_aware(s, DEFAULT_PROSE_RATIO)
            );
        }
    }

    #[test]
    fn aware_estimate_exceeds_flat_for_cjk() {
        // The core fix: a CJK message under a 3.5 prose anchor must estimate far
        // MORE tokens than a flat 3.5 ratio would — otherwise the budget sensor
        // under-counts CJK ~2.3× and overflows before compaction triggers.
        // Pure CJK (no Latin) so the blend collapses exactly to the CJK ratio.
        let cjk = "这是一段中文文本用于测试估算的比率检测逻辑是否正确工作没有拉丁字符";
        let chars = cjk.chars().count();
        let flat = (chars as f64 / 3.5).ceil() as usize; // what the old sensor did
        let aware = estimate_tokens_aware(cjk, 3.5);
        assert!(
            aware > flat,
            "content-aware ({aware}) must exceed flat-3.5 ({flat}) for CJK"
        );
        // Pure CJK ratio is 1.5, so aware == chars/1.5 exactly.
        assert_eq!(aware, (chars as f64 / 1.5).ceil() as usize);
    }

    #[test]
    fn aware_estimate_zero_ratio_guard() {
        assert_eq!(estimate_tokens_aware("hello", 0.0), 0);
    }

    // --- estimate_message_tokens_aware (image-aware per-message estimate) ---

    fn user_with_image(text: &str) -> UnifiedMessage {
        UnifiedMessage::user_with_content(vec![
            ContentBlock::Image {
                data: "ignored_base64_blob".to_string(),
                mime_type: "image/png".to_string(),
            },
            ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            },
        ])
    }

    #[test]
    fn message_estimate_matches_text_when_no_images() {
        // A text-only message must estimate exactly as the text path — the image
        // term is purely additive, so the common case stays byte-identical.
        let msg = UnifiedMessage::user("just plain english prose here");
        assert_eq!(
            estimate_message_tokens_aware(&msg, DEFAULT_PROSE_RATIO),
            estimate_tokens_aware(&msg.text_content(), DEFAULT_PROSE_RATIO)
        );
    }

    #[test]
    fn message_estimate_charges_for_image() {
        // The core fix: an image-bearing message must cost its text estimate PLUS
        // one image charge — not zero for the image (which is what the sensor saw
        // when it summed estimate_tokens_aware(text_content) and text_content
        // dropped the image block).
        let msg = user_with_image("look at this screenshot");
        let text_only = estimate_tokens_aware(&msg.text_content(), DEFAULT_PROSE_RATIO);
        let with_image = estimate_message_tokens_aware(&msg, DEFAULT_PROSE_RATIO);
        assert_eq!(with_image, text_only + IMAGE_TOKENS_ESTIMATE);
        assert!(
            with_image >= IMAGE_TOKENS_ESTIMATE,
            "image-bearing message must never estimate as ~0 tokens, got {with_image}"
        );
    }

    #[test]
    fn message_estimate_charges_per_image() {
        // Three images in one turn → three image charges.
        let msg = UnifiedMessage::user_with_content(vec![
            ContentBlock::Image {
                data: "a".to_string(),
                mime_type: "image/png".to_string(),
            },
            ContentBlock::Image {
                data: "b".to_string(),
                mime_type: "image/png".to_string(),
            },
            ContentBlock::Image {
                data: "c".to_string(),
                mime_type: "image/png".to_string(),
            },
        ]);
        assert_eq!(
            estimate_message_tokens_aware(&msg, DEFAULT_PROSE_RATIO),
            3 * IMAGE_TOKENS_ESTIMATE
        );
    }

    #[test]
    fn message_estimate_byte_identical_for_multi_block_chat() {
        // Text + Thinking only (no structured / image): must stay exactly the old
        // flattened text_content estimate, so ordinary conversation is untouched.
        let msg = UnifiedMessage::user_with_content(vec![
            ContentBlock::Text {
                text: "first prose block here".to_string(),
                cache_control: None,
            },
            ContentBlock::Thinking {
                thinking: "and a thinking trace block".to_string(),
                signature: None,
            },
        ]);
        assert_eq!(
            estimate_message_tokens_aware(&msg, DEFAULT_PROSE_RATIO),
            estimate_tokens_aware(&msg.text_content(), DEFAULT_PROSE_RATIO),
        );
    }

    #[test]
    fn message_estimate_charges_structured_at_code_density() {
        // The fix: a JSON tool-output block flattened alongside prose used to be
        // folded through the prose anchor (3.5) — `looks_like_code` misses JSON
        // mixed into prose — under-counting the symbol-dense payload and
        // under-reporting pressure on tool-heavy turns. Now the structured block
        // is charged at the denser CODE_RATIO while the prose stays at its anchor.
        let json = serde_json::json!({
            "status": "ok",
            "items": [{"id": 1, "name": "alpha"}, {"id": 2, "name": "beta"}],
            "count": 2
        });
        let text = "Here is the tool result you asked for, summarized in prose.";
        let msg = UnifiedMessage::user_with_content(vec![
            ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            },
            ContentBlock::Json {
                value: json.clone(),
            },
        ]);
        // Exact decomposition: prose at the prose anchor + JSON at code density.
        let expected = estimate_tokens_aware(text, DEFAULT_PROSE_RATIO)
            + estimate_tokens_aware(&json.to_string(), CODE_RATIO);
        assert_eq!(
            estimate_message_tokens_aware(&msg, DEFAULT_PROSE_RATIO),
            expected
        );
        // Never cheaper than charging everything at the looser prose anchor —
        // the systematic direction of the fix (denser ⇒ ≥ tokens).
        let all_prose_anchored = estimate_tokens_aware(text, DEFAULT_PROSE_RATIO)
            + estimate_tokens_aware(&json.to_string(), DEFAULT_PROSE_RATIO);
        assert!(estimate_message_tokens_aware(&msg, DEFAULT_PROSE_RATIO) >= all_prose_anchored);
    }

    #[test]
    fn message_estimate_charges_tool_call_arguments_as_structured() {
        // A tool call's JSON arguments are structured, not prose — charged at the
        // code anchor, matching how the same bytes estimate via the code ratio.
        let arguments = serde_json::json!({"path": "/etc/hosts", "limit": 100});
        let msg = UnifiedMessage::user_with_content(vec![ContentBlock::ToolCall {
            id: "call_1".to_string(),
            name: "read_file".to_string(),
            arguments: arguments.clone(),
            thought_signature: None,
        }]);
        let flattened = format!("read_file {arguments}");
        assert_eq!(
            estimate_message_tokens_aware(&msg, DEFAULT_PROSE_RATIO),
            estimate_tokens_aware(&flattened, CODE_RATIO),
        );
    }
}
