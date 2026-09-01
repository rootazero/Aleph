//! Voice transcript hygiene — Whisper hallucination filtering and utterance
//! assembly.
//!
//! Whisper-family models fabricate plausible-but-absent text when fed silence,
//! background noise, or music — almost always boilerplate harvested from their
//! `YouTube` training data ("Thanks for watching!", "字幕由Amara.org社区提供",
//! etc.) or a short phrase looped back to back. Left unfiltered, these phantom
//! phrases enter the agent's context layer as if the user had spoken them,
//! polluting memory and triggering spurious replies.
//!
//! The pass is deliberately *conservative*: it only nulls a transcript when the
//! **whole** utterance is a known boilerplate phrase or a degenerate repetition
//! loop, so a legitimate "thank you so much for the help" is never eaten. That
//! whole-utterance granularity is why the filter lives in this shared crate:
//! the core applies it on the batch STT path (`transcribe_bytes`), and the
//! Panel applies it at the streaming utterance-lock point — per-segment
//! filtering inside the streaming decoders would eat legitimate short segments
//! of longer speech.
//!
//! Ported from hermes-agent's `transcription_tools` filter; the tail n-gram
//! loop detector follows WhisperLiveKit's `_has_repetition_loop`.

/// Known Whisper boilerplate hallucinations, normalized (lowercase, trailing
/// punctuation stripped — see [`normalize`]). A transcript whose entire
/// normalized body equals one of these is treated as noise and nulled.
///
/// NOTE: entries must be pre-normalized — no trailing `.`/`!`/`。` variants
/// (normalize strips those before the comparison, so such entries would be
/// unreachable dead weight).
const HALLUCINATION_PHRASES: &[&str] = &[
    // English (YouTube-caption heritage)
    "thank you",
    "thank you so much",
    "thank you very much",
    "thanks for watching",
    "thank you for watching",
    "please subscribe",
    "please subscribe to my channel",
    "don't forget to subscribe",
    "like and subscribe",
    "see you next time",
    "see you in the next video",
    "bye",
    "bye bye",
    "you",
    "the end",
    "subtitles by the amara.org community",
    "subtitles by the amara org community",
    "transcription by castingwords",
    "© transcript emily beynon",
    "i'll see you next time",
    "i'll see you in the next video",
    "okay",
    "ok",
    "thanks",
    "music",
    "applause",
    "silence",
    // Chinese (the canonical zh-Whisper silence/noise decodes)
    "字幕由amara.org社区提供",
    "字幕由 amara.org 社区提供",
    "由amara.org社区提供字幕",
    "请不吝点赞 订阅 转发 打赏支持明镜与点点栏目",
    "明镜与点点栏目",
    "谢谢观看",
    "谢谢大家观看",
    "感谢观看",
    "感谢收看",
    "谢谢收看",
    "多谢观看",
    "谢谢大家",
    "请订阅",
    "点赞订阅",
    "订阅频道",
    "字幕制作",
    "中文字幕",
    "未经允许不得转载",
];

/// Minimum repetition count at which an identical single token looped back to
/// back is judged a Whisper decode loop rather than genuine speech.
const TOKEN_LOOP_THRESHOLD: usize = 4;

/// Minimum repetition count for multi-token (n ≥ 2) phrase loops. Lower than
/// the single-token bar because a 2-4 word phrase repeated verbatim three times
/// is already a strong loop signature ("thank you thank you thank you").
const NGRAM_LOOP_THRESHOLD: usize = 3;

/// Largest n-gram (tokens or chars) considered by the loop detectors.
const MAX_LOOP_NGRAM: usize = 4;

/// Minimum character count before the CJK char-level loop detector may fire —
/// keeps short genuine replies ("哈哈哈哈") out of reach.
const CHAR_LOOP_MIN_CHARS: usize = 6;

/// Normalize a transcript fragment for phrase comparison: lowercase, trim, and
/// strip surrounding whitespace and trailing sentence punctuation (ASCII and
/// CJK).
fn normalize(text: &str) -> String {
    text.trim()
        .trim_end_matches([
            '.', '!', '?', ',', '…', '。', '！', '？', '，', '、', '；', ';',
        ])
        .trim()
        .to_lowercase()
}

/// Is `items` nothing but its own `n`-item head, repeated at least `min_reps`
/// times?
///
/// The final repetition may be **cut short**: an endpoint fires mid-phrase, so
/// real Whisper decode loops routinely arrive truncated ("thank you thank you
/// thank you thank"). Requiring an exact multiple of `n` — as this did before —
/// let every one of those through the filter untouched.
fn repeats_head<T: PartialEq>(items: &[T], n: usize, min_reps: usize) -> bool {
    if n == 0 || items.len() < n * min_reps {
        return false;
    }
    let Some(head) = items.get(..n) else {
        return false;
    };
    // `chunks` yields full windows plus a possibly-short tail. A full window must
    // equal the head; the short tail only has to be a prefix of it (which
    // `starts_with` gives for both cases in one test).
    items.chunks(n).all(|c| head.starts_with(c))
}

/// Token-level loop detector: the whole transcript is one n-gram (1..=4 tokens)
/// repeated back to back. Single tokens need [`TOKEN_LOOP_THRESHOLD`] repeats
/// ("you you you you"); longer phrases need [`NGRAM_LOOP_THRESHOLD`]
/// ("thank you thank you thank you").
fn is_token_loop(text: &str) -> bool {
    let tokens: Vec<String> = text.split_whitespace().map(normalize).collect();
    if tokens.iter().any(String::is_empty) {
        return false;
    }
    (1..=MAX_LOOP_NGRAM).any(|n| {
        let min_reps = if n == 1 {
            TOKEN_LOOP_THRESHOLD
        } else {
            NGRAM_LOOP_THRESHOLD
        };
        repeats_head(&tokens, n, min_reps)
    })
}

/// Char-level loop detector for whitespace-free (CJK) transcripts: the whole
/// normalized body is one 1..=4-char group repeated ("谢谢谢谢谢谢",
/// "谢谢观看谢谢观看谢谢观看"). Requires ≥ [`CHAR_LOOP_MIN_CHARS`] chars and
/// ≥ [`NGRAM_LOOP_THRESHOLD`] repeats (single chars ≥ [`TOKEN_LOOP_THRESHOLD`]
/// + the length floor) so short genuine interjections survive.
fn is_char_loop(normalized: &str) -> bool {
    if normalized.contains(char::is_whitespace) {
        return false;
    }
    let chars: Vec<char> = normalized.chars().collect();
    if chars.len() < CHAR_LOOP_MIN_CHARS {
        return false;
    }
    (1..=MAX_LOOP_NGRAM).any(|n| {
        let min_reps = if n == 1 {
            TOKEN_LOOP_THRESHOLD.max(CHAR_LOOP_MIN_CHARS)
        } else {
            NGRAM_LOOP_THRESHOLD
        };
        repeats_head(&chars, n, min_reps)
    })
}

/// Filter a raw Whisper transcript. Returns the transcript unchanged when it
/// looks like genuine speech, or an empty string when the entire utterance is a
/// known hallucination phrase or a repetition loop.
///
/// Conservative by design: a known phrase embedded in a longer sentence is
/// preserved (we only null the transcript when it is *nothing but* boilerplate).
#[must_use]
pub fn filter_transcript(text: &str) -> String {
    let normalized = normalize(text);
    if normalized.is_empty() {
        return String::new();
    }
    if HALLUCINATION_PHRASES.contains(&normalized.as_str()) {
        return String::new();
    }
    if is_token_loop(text) || is_char_loop(&normalized) {
        return String::new();
    }
    text.trim().to_string()
}

/// Should a space be inserted when appending `next` after `prev`?
///
/// True only across an ASCII boundary (Latin word/punctuation → Latin
/// alphanumeric) so English fragments join as words while CJK text is never
/// broken by a stray space. Shared by [`merge_utterance`] and the streaming
/// decoders' committed-text accumulation, so display, wire, and lock-point
/// joins cannot drift.
///
/// Punctuation that lives inside identifiers (`/`, `+`, `=`, `-`, `_`,
/// `\`, `|`, `@`, `#`, `$`, `%`, `&`, `*`, `<`, `>`, `` ` ``, `~`) is
/// treated as NOT breaking a word, so a streaming STT commit landing at
/// `path/` does not turn `path/to` into `path/ to`, and `version=` does
/// not become `version= 1`. Sentence-boundary punctuation (`.`, `,`, `!`,
/// `?`, `;`, `:`) is still treated as breaking — the existing tests
/// assert that behaviour.
#[must_use]
pub fn join_needs_space(prev: &str, next: &str) -> bool {
    let Some(a) = prev.chars().next_back() else {
        return false;
    };
    let Some(b) = next.chars().next() else {
        return false;
    };
    let code_operator = matches!(
        a,
        '/' | '+' | '=' | '-' | '_' | '\\' | '|' | '@' | '#' | '$' | '%' | '&' | '*' | '<' | '>' | '`' | '~'
    );
    (a.is_ascii_alphanumeric() || a.is_ascii_punctuation()) && b.is_ascii_alphanumeric() && !code_operator
}

/// Assemble the final utterance text at the streaming lock point: the locked
/// `committed` text plus the still-floating `interim` hypothesis.
///
/// The interim MUST be included — short utterances ("对", "好的") often live
/// entirely in the floating hypothesis when the endpoint VAD fires, and
/// faster-whisper-backed servers never flush a final for the trailing window
/// after `END_OF_AUDIO`. Space insertion follows [`join_needs_space`].
#[must_use]
pub fn merge_utterance(committed: &str, interim: &str) -> String {
    let c = committed.trim();
    let i = interim.trim();
    if i.is_empty() {
        return c.to_string();
    }
    if c.is_empty() {
        return i.to_string();
    }
    if join_needs_space(c, i) {
        format!("{c} {i}")
    } else {
        format!("{c}{i}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nulls_exact_boilerplate() {
        assert!(filter_transcript("Thank you.").is_empty());
        assert!(filter_transcript("Thanks for watching!").is_empty());
        assert!(filter_transcript("  please subscribe  ").is_empty());
        assert!(filter_transcript("you").is_empty());
    }

    #[test]
    fn nulls_chinese_boilerplate() {
        assert!(filter_transcript("字幕由Amara.org社区提供").is_empty());
        assert!(filter_transcript("谢谢观看！").is_empty());
        assert!(filter_transcript("请订阅。").is_empty());
        assert!(filter_transcript("谢谢大家观看").is_empty());
    }

    #[test]
    fn preserves_genuine_speech() {
        let s = "Thank you for sending the report, can you summarize section three";
        assert_eq!(filter_transcript(s), s);
        let zh = "谢谢，现在帮我安排明天的会议";
        assert_eq!(filter_transcript(zh), zh);
    }

    #[test]
    fn preserves_boilerplate_inside_real_sentence() {
        // "thank you" appears but the utterance is not *only* boilerplate.
        let s = "thank you, now please book the meeting";
        assert_eq!(filter_transcript(s), s);
    }

    #[test]
    fn preserves_short_genuine_chinese() {
        // Everyday spoken turns must never be eaten.
        for s in ["谢谢", "再见", "好的", "对", "帮我看看这个"] {
            assert_eq!(filter_transcript(s), s, "{s} must survive");
        }
    }

    #[test]
    fn nulls_single_token_loop() {
        assert!(filter_transcript("you you you you you").is_empty());
        assert!(filter_transcript("Yeah. Yeah. Yeah. Yeah.").is_empty());
    }

    #[test]
    fn nulls_phrase_loop() {
        // WLK-style tail loop: a 2-gram repeated 3×.
        assert!(filter_transcript("thank you thank you thank you").is_empty());
        assert!(filter_transcript("so much so much so much").is_empty());
    }

    #[test]
    fn nulls_cjk_char_loop() {
        assert!(filter_transcript("谢谢谢谢谢谢").is_empty());
        assert!(filter_transcript("谢谢观看谢谢观看谢谢观看").is_empty());
    }

    #[test]
    fn nulls_loop_cut_short_by_the_endpoint() {
        // The shape the exact-multiple rule missed entirely: the VAD ends the
        // utterance mid-phrase, so the last repetition is truncated.
        assert!(filter_transcript("thank you thank you thank you thank").is_empty());
        assert!(filter_transcript("谢谢观看谢谢观看谢谢观看谢谢").is_empty());
        // …and one below the repetition floor still survives (2 reps + a tail).
        assert_eq!(
            filter_transcript("thank you thank you thank"),
            "thank you thank you thank"
        );
    }

    #[test]
    fn keeps_short_cjk_interjection() {
        // Below the char-loop length floor — genuine laughter/agreement.
        assert_eq!(filter_transcript("哈哈哈哈"), "哈哈哈哈");
        assert_eq!(filter_transcript("好好好"), "好好好");
    }

    #[test]
    fn keeps_short_genuine_phrase() {
        // Below repetition thresholds and not a known phrase.
        assert_eq!(filter_transcript("book it now"), "book it now");
        assert_eq!(filter_transcript("okay let's go"), "okay let's go");
    }

    #[test]
    fn non_repeating_text_is_not_a_loop() {
        let s = "去公园 去超市 去公司 去学校";
        assert_eq!(filter_transcript(s), s);
    }

    #[test]
    fn empty_in_empty_out() {
        assert!(filter_transcript("").is_empty());
        assert!(filter_transcript("   ").is_empty());
    }

    #[test]
    fn merge_includes_floating_interim() {
        assert_eq!(merge_utterance("你好世界", "吗"), "你好世界吗");
        assert_eq!(merge_utterance("", "对"), "对");
        assert_eq!(merge_utterance("帮我查一下天气", ""), "帮我查一下天气");
    }

    #[test]
    fn merge_spaces_ascii_boundary_only() {
        assert_eq!(merge_utterance("hello", "world"), "hello world");
        assert_eq!(merge_utterance("hello,", "world"), "hello, world");
        assert_eq!(merge_utterance("你好", "world"), "你好world");
        assert_eq!(merge_utterance("hello", "你好"), "hello你好");
    }

    #[test]
    fn merge_keeps_code_operators_glued() {
        // Streaming STT commits that land at `/`, `=`, `+`, `-`, `_`, `\`
        // before the next fragment must not insert a space, otherwise paths,
        // version strings, math and similar get mangled into nonsense.
        assert_eq!(merge_utterance("path/", "to"), "path/to");
        assert_eq!(merge_utterance("version=", "1"), "version=1");
        assert_eq!(merge_utterance("a +", "b"), "a +b");
        assert_eq!(merge_utterance("var_", "name"), "var_name");
        assert_eq!(merge_utterance("user@", "host"), "user@host");
        assert_eq!(merge_utterance("a<b", "c"), "a<b c");
    }
}
