//! Whisper hallucination filter for inbound STT.
//!
//! Whisper-family models fabricate plausible-but-absent text when fed silence,
//! background noise, or music — almost always boilerplate harvested from their
//! YouTube training data ("Thanks for watching!", "Please subscribe", etc.) or
//! a single token looped dozens of times. Left unfiltered, these phantom
//! phrases enter the agent's context layer as if the user had spoken them,
//! polluting memory and triggering spurious replies.
//!
//! This guard runs on the raw transcript before it reaches the agent loop. It
//! is deliberately *conservative*: it only nulls a transcript when the **whole**
//! utterance is a known boilerplate phrase or a degenerate repetition loop, so
//! a legitimate "thank you so much for the help" is never eaten.
//!
//! Ported from hermes-agent's `transcription_tools` hallucination filter,
//! re-expressed as an allocation-light Rust pass.

/// Known Whisper boilerplate hallucinations, normalized (lowercase, trailing
/// punctuation stripped). A transcript whose entire normalized body equals one
/// of these is treated as noise and nulled.
const HALLUCINATION_PHRASES: &[&str] = &[
    "thank you",
    "thank you so much",
    "thank you very much",
    "thanks for watching",
    "thanks for watching!",
    "thank you for watching",
    "thank you for watching!",
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
];

/// Minimum repetition count at which an identical short phrase looped back to
/// back is judged a Whisper decode loop rather than genuine speech.
const REPETITION_LOOP_THRESHOLD: usize = 4;

/// Normalize a transcript fragment for phrase comparison: lowercase, trim, and
/// strip surrounding whitespace and trailing sentence punctuation.
fn normalize(text: &str) -> String {
    text.trim()
        .trim_end_matches(['.', '!', '?', ',', '…'])
        .trim()
        .to_lowercase()
}

/// Returns `true` when the whole transcript is a degenerate repetition loop
/// (e.g. "you you you you you") — a Whisper signature on silence/noise.
fn is_repetition_loop(text: &str) -> bool {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.len() < REPETITION_LOOP_THRESHOLD {
        return false;
    }
    // All tokens identical (case-insensitively, punctuation-stripped).
    let first = normalize(tokens[0]);
    if first.is_empty() {
        return false;
    }
    tokens.iter().all(|t| normalize(t) == first)
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
    if is_repetition_loop(text) {
        return String::new();
    }
    text.trim().to_string()
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
    fn preserves_genuine_speech() {
        let s = "Thank you for sending the report, can you summarize section three";
        assert_eq!(filter_transcript(s), s);
    }

    #[test]
    fn preserves_boilerplate_inside_real_sentence() {
        // "thank you" appears but the utterance is not *only* boilerplate.
        let s = "thank you, now please book the meeting";
        assert_eq!(filter_transcript(s), s);
    }

    #[test]
    fn nulls_repetition_loop() {
        assert!(filter_transcript("you you you you you").is_empty());
        assert!(filter_transcript("Yeah. Yeah. Yeah. Yeah.").is_empty());
    }

    #[test]
    fn keeps_short_genuine_phrase() {
        // Below repetition threshold and not a known phrase.
        assert_eq!(filter_transcript("book it now"), "book it now");
    }

    #[test]
    fn empty_in_empty_out() {
        assert!(filter_transcript("").is_empty());
        assert!(filter_transcript("   ").is_empty());
    }
}
