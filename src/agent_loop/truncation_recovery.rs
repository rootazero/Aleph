//! Truncation recovery state machine.
//!
//! Handles `MaxTokens` stop reason by escalating token limits,
//! generating context-aware continuation prompts, and assembling
//! fragmented outputs with overlap deduplication.

// ── Constants ──────────────────────────────────────────────────────

/// Maximum continuation attempts (after escalation).
const MAX_CONTINUE_ATTEMPTS: u32 = 2;

/// Total attempts before exhaustion (escalation + continuations).
const MAX_TOTAL_ATTEMPTS: u32 = 3;

/// Characters to scan for overlap between fragments.
const OVERLAP_SCAN_CHARS: usize = 300;

/// Characters from the tail of truncated output used to build continuation prompts.
const TAIL_CHARS_FOR_PROMPT: usize = 200;

// ── Types ──────────────────────────────────────────────────────────

/// Recovery phase in the state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryPhase {
    /// No truncation has occurred.
    Idle,
    /// First truncation handled by escalating max_tokens to provider maximum.
    Escalated,
    /// Continuing after escalation; `count` tracks continuation attempts.
    Continuing { count: u32 },
    /// All recovery attempts exhausted.
    Exhausted,
}

/// Action returned by `on_truncation` telling the caller what to do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryAction {
    /// Override max_tokens for the next request, if any.
    pub max_tokens_override: Option<u32>,
    /// Continuation prompt to prepend to the next request.
    pub continuation_prompt: String,
    /// Whether the caller should continue the loop.
    pub should_continue: bool,
}

/// Truncation recovery state machine.
#[derive(Debug)]
pub struct TruncationRecovery {
    phase: RecoveryPhase,
    attempts: u32,
    /// Provider maximum token limit (ceiling for escalation).
    provider_max_tokens: u32,
    /// The original max_tokens before any escalation, restored on reset.
    original_max_tokens: Option<u32>,
    /// Accumulated output fragments for assembly.
    fragments: Vec<String>,
}

impl TruncationRecovery {
    /// Create a new recovery state machine.
    ///
    /// `provider_max_tokens` is the absolute ceiling the provider supports.
    pub fn new(provider_max_tokens: u32) -> Self {
        Self {
            phase: RecoveryPhase::Idle,
            attempts: 0,
            provider_max_tokens,
            original_max_tokens: None,
            fragments: Vec::new(),
        }
    }

    /// Handle a truncation event.
    ///
    /// `current_max_tokens` is the max_tokens used in the request that was truncated.
    /// `truncated_output` is the partial output received.
    ///
    /// Returns a `RecoveryAction` describing what the caller should do.
    pub fn on_truncation(
        &mut self,
        current_max_tokens: Option<u32>,
        truncated_output: &str,
    ) -> RecoveryAction {
        self.attempts += 1;

        // Store the fragment.
        if !truncated_output.is_empty() {
            self.fragments.push(truncated_output.to_owned());
        }

        // Global guard: regardless of which phase we're in, cap total attempts.
        // This interacts with the per-phase MAX_CONTINUE_ATTEMPTS check below —
        // both exist because the entry path into Continuing varies (from Idle or Escalated).
        if self.attempts > MAX_TOTAL_ATTEMPTS {
            self.phase = RecoveryPhase::Exhausted;
            return RecoveryAction {
                max_tokens_override: None,
                continuation_prompt: String::new(),
                should_continue: false,
            };
        }

        let continuation_prompt = build_smart_continuation(truncated_output);

        match self.phase {
            RecoveryPhase::Idle => {
                // First truncation — try to escalate if possible.
                let can_escalate = current_max_tokens
                    .map(|cur| cur < self.provider_max_tokens)
                    .unwrap_or(false);

                if can_escalate {
                    self.original_max_tokens = current_max_tokens;
                    self.phase = RecoveryPhase::Escalated;
                    RecoveryAction {
                        max_tokens_override: Some(self.provider_max_tokens),
                        continuation_prompt,
                        should_continue: true,
                    }
                } else {
                    // Already at max or unknown — go straight to continuing.
                    if self.original_max_tokens.is_none() {
                        self.original_max_tokens = current_max_tokens;
                    }
                    self.phase = RecoveryPhase::Continuing { count: 1 };
                    RecoveryAction {
                        max_tokens_override: None,
                        continuation_prompt,
                        should_continue: true,
                    }
                }
            }
            RecoveryPhase::Escalated => {
                // Escalation wasn't enough — enter continuation mode.
                self.phase = RecoveryPhase::Continuing { count: 1 };
                RecoveryAction {
                    max_tokens_override: None,
                    continuation_prompt,
                    should_continue: true,
                }
            }
            RecoveryPhase::Continuing { count } => {
                let new_count = count + 1;
                if new_count > MAX_CONTINUE_ATTEMPTS {
                    self.phase = RecoveryPhase::Exhausted;
                    RecoveryAction {
                        max_tokens_override: None,
                        continuation_prompt: String::new(),
                        should_continue: false,
                    }
                } else {
                    self.phase = RecoveryPhase::Continuing { count: new_count };
                    RecoveryAction {
                        max_tokens_override: None,
                        continuation_prompt,
                        should_continue: true,
                    }
                }
            }
            RecoveryPhase::Exhausted => {
                // Already exhausted — should not happen, but be safe.
                RecoveryAction {
                    max_tokens_override: None,
                    continuation_prompt: String::new(),
                    should_continue: false,
                }
            }
        }
    }

    /// Assemble all accumulated fragments, deduplicating overlapping regions.
    pub fn assemble_output(&self) -> String {
        deduplicate_and_join(&self.fragments)
    }

    /// Reset to idle state, restoring original max_tokens info.
    /// Returns the original max_tokens value (before escalation) if any.
    pub fn reset(&mut self) -> Option<u32> {
        let original = self.original_max_tokens.take();
        self.phase = RecoveryPhase::Idle;
        self.attempts = 0;
        self.fragments.clear();
        original
    }

    /// Current phase.
    pub fn phase(&self) -> &RecoveryPhase {
        &self.phase
    }

    /// Total truncation attempts so far.
    pub fn attempts(&self) -> u32 {
        self.attempts
    }
}

// ── Continuation prompt builder ────────────────────────────────────

/// Build a context-aware continuation prompt by inspecting the tail of truncated output.
fn build_smart_continuation(truncated_output: &str) -> String {
    let tail = safe_tail(truncated_output, TAIL_CHARS_FOR_PROMPT);

    if has_unclosed_code_fence(tail) {
        return format!(
            "Your previous response was truncated mid-code-block. \
             Continue exactly where you left off. Last output:\n```\n{tail}\n```"
        );
    }

    if has_unbalanced_braces(tail) {
        return format!(
            "Your previous response was truncated inside a JSON/object structure. \
             Continue exactly where you left off. Last output:\n{tail}"
        );
    }

    if has_truncated_list(tail) {
        return format!(
            "Your previous response was truncated inside a list. \
             Continue exactly where you left off. Last output:\n{tail}"
        );
    }

    // General fallback.
    format!(
        "Your previous response was truncated. \
         Continue exactly where you left off. Last output:\n{tail}"
    )
}

/// Extract the last `n` characters from `s`, respecting UTF-8 char boundaries.
fn safe_tail(s: &str, n: usize) -> &str {
    if s.is_empty() {
        return s;
    }
    let char_count = s.chars().count();
    if char_count <= n {
        return s;
    }
    // Skip (char_count - n) characters, then return the rest.
    let skip = char_count - n;
    let byte_offset = s.char_indices().nth(skip).map(|(i, _)| i).unwrap_or(0);
    &s[byte_offset..]
}

/// Detect an unclosed triple-backtick code fence.
fn has_unclosed_code_fence(text: &str) -> bool {
    let count = text.matches("```").count();
    !count.is_multiple_of(2)
}

/// Detect a truncated list by checking for lines near the tail starting with
/// `- `, `* `, or a numbered pattern like `1. `.
fn has_truncated_list(text: &str) -> bool {
    // Check the last few lines for list item patterns.
    text.lines().rev().take(5).any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("- ")
            || trimmed.starts_with("* ")
            || trimmed
                .split_once(". ")
                .map(|(num, _)| !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()))
                .unwrap_or(false)
    })
}

/// Detect unbalanced `{` / `}` braces (heuristic for JSON truncation).
fn has_unbalanced_braces(text: &str) -> bool {
    let opens = text.chars().filter(|&c| c == '{').count();
    let closes = text.chars().filter(|&c| c == '}').count();
    opens > closes
}

// ── Fragment deduplication ─────────────────────────────────────────

/// Join fragments, removing overlapping regions between consecutive pieces.
pub fn deduplicate_and_join(fragments: &[String]) -> String {
    if fragments.is_empty() {
        return String::new();
    }
    let mut result = fragments[0].clone();
    for fragment in &fragments[1..] {
        let overlap_len = find_overlap(&result, fragment, OVERLAP_SCAN_CHARS);
        result.push_str(&fragment[overlap_len..]);
    }
    result
}

/// Find the longest suffix of `left` that is also a prefix of `right`,
/// scanning up to `max_scan` bytes (snapped to char boundaries).
fn find_overlap(left: &str, right: &str, max_scan: usize) -> usize {
    if left.is_empty() || right.is_empty() {
        return 0;
    }

    // Determine the byte range of `left` to scan from the tail.
    let scan_start = if left.len() > max_scan {
        // Snap forward to the next char boundary.
        let candidate = left.len() - max_scan;
        left.ceil_char_boundary(candidate)
    } else {
        0
    };

    let tail = &left[scan_start..];
    let mut best = 0;

    // Try every char boundary in the tail as a potential overlap start.
    for (i, _) in tail.char_indices() {
        let suffix = &tail[i..];
        if right.starts_with(suffix) && suffix.len() > best {
            best = suffix.len();
        }
    }

    best
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_by_default() {
        let recovery = TruncationRecovery::new(16384);
        assert_eq!(*recovery.phase(), RecoveryPhase::Idle);
        assert_eq!(recovery.attempts(), 0);
    }

    #[test]
    fn first_truncation_escalates_when_possible() {
        let mut recovery = TruncationRecovery::new(16384);
        let action = recovery.on_truncation(Some(4096), "partial output");

        assert_eq!(*recovery.phase(), RecoveryPhase::Escalated);
        assert_eq!(action.max_tokens_override, Some(16384));
        assert!(action.should_continue);
        assert_eq!(recovery.attempts(), 1);
    }

    #[test]
    fn first_truncation_continues_when_already_at_max() {
        let mut recovery = TruncationRecovery::new(16384);
        let action = recovery.on_truncation(Some(16384), "partial output");

        assert_eq!(*recovery.phase(), RecoveryPhase::Continuing { count: 1 });
        assert_eq!(action.max_tokens_override, None);
        assert!(action.should_continue);
    }

    #[test]
    fn escalated_then_truncated_enters_continuing() {
        let mut recovery = TruncationRecovery::new(16384);
        // First truncation → Escalated
        recovery.on_truncation(Some(4096), "part 1");
        assert_eq!(*recovery.phase(), RecoveryPhase::Escalated);

        // Second truncation → Continuing(1)
        let action = recovery.on_truncation(Some(16384), "part 2");
        assert_eq!(*recovery.phase(), RecoveryPhase::Continuing { count: 1 });
        assert!(action.should_continue);
        assert_eq!(recovery.attempts(), 2);
    }

    #[test]
    fn exhausts_after_max_attempts() {
        let mut recovery = TruncationRecovery::new(16384);
        // Attempt 1 → Escalated
        recovery.on_truncation(Some(4096), "p1");
        // Attempt 2 → Continuing(1)
        recovery.on_truncation(Some(16384), "p2");
        // Attempt 3 → Continuing(2)
        recovery.on_truncation(Some(16384), "p3");
        // Attempt 4 → exceeds MAX_TOTAL_ATTEMPTS (3) → Exhausted
        let action = recovery.on_truncation(Some(16384), "p4");

        assert_eq!(*recovery.phase(), RecoveryPhase::Exhausted);
        assert!(!action.should_continue);
        assert_eq!(recovery.attempts(), 4);
    }

    #[test]
    fn reset_restores_idle_and_original_max_tokens() {
        let mut recovery = TruncationRecovery::new(16384);
        recovery.on_truncation(Some(4096), "partial");
        assert_eq!(*recovery.phase(), RecoveryPhase::Escalated);

        let original = recovery.reset();
        assert_eq!(original, Some(4096));
        assert_eq!(*recovery.phase(), RecoveryPhase::Idle);
        assert_eq!(recovery.attempts(), 0);
    }

    #[test]
    fn assemble_output_deduplicates_overlapping_fragments() {
        let mut recovery = TruncationRecovery::new(16384);
        // Simulate fragments with overlapping tails/heads.
        recovery
            .fragments
            .push("Hello, world! This is a test".to_owned());
        recovery
            .fragments
            .push("is a test of deduplication.".to_owned());

        let assembled = recovery.assemble_output();
        assert_eq!(assembled, "Hello, world! This is a test of deduplication.");
    }

    #[test]
    fn smart_prompt_detects_code_block() {
        let output = "Here is some code:\n```rust\nfn main() {\n    println!(\"hello\"";
        let prompt = build_smart_continuation(output);
        assert!(
            prompt.contains("truncated mid-code-block"),
            "Expected code block detection, got: {prompt}"
        );
    }

    #[test]
    fn smart_prompt_detects_json() {
        let output = r#"{"name": "test", "nested": {"key": "value""#;
        let prompt = build_smart_continuation(output);
        assert!(
            prompt.contains("JSON/object structure"),
            "Expected JSON detection, got: {prompt}"
        );
    }

    #[test]
    fn smart_prompt_detects_list() {
        let output = "Here are the steps:\n1. First item\n2. Second item\n3. Third ite";
        let prompt = build_smart_continuation(output);
        assert!(
            prompt.contains("truncated inside a list"),
            "Expected list detection, got: {prompt}"
        );

        // Also test dash-style lists.
        let dash_output = "Items:\n- Alpha\n- Beta\n- Gam";
        let dash_prompt = build_smart_continuation(dash_output);
        assert!(
            dash_prompt.contains("truncated inside a list"),
            "Expected list detection for dash list, got: {dash_prompt}"
        );
    }

    #[test]
    fn smart_prompt_general_fallback() {
        let output = "This is just regular text that was cut off mid-sentence";
        let prompt = build_smart_continuation(output);
        assert!(
            prompt.contains("Your previous response was truncated."),
            "Expected general fallback, got: {prompt}"
        );
        assert!(!prompt.contains("code-block"));
        assert!(!prompt.contains("JSON"));
    }

    #[test]
    fn no_max_tokens_provided_skips_escalation() {
        let mut recovery = TruncationRecovery::new(16384);
        let action = recovery.on_truncation(None, "partial");

        assert_eq!(*recovery.phase(), RecoveryPhase::Continuing { count: 1 });
        assert_eq!(action.max_tokens_override, None);
        assert!(action.should_continue);
    }

    // ── Edge case tests ────────────────────────────────────────────

    #[test]
    fn safe_tail_handles_empty_string() {
        assert_eq!(safe_tail("", 100), "");
    }

    #[test]
    fn safe_tail_handles_multibyte_utf8() {
        let s = "你好世界abcd"; // 4 CJK chars + 4 ASCII = 8 chars
        let tail = safe_tail(s, 4);
        assert_eq!(tail, "abcd");
        // Ensure we get last 6 chars: 世界abcd
        let tail6 = safe_tail(s, 6);
        assert_eq!(tail6, "世界abcd");
    }

    #[test]
    fn find_overlap_no_overlap() {
        assert_eq!(find_overlap("abc", "xyz", 300), 0);
    }

    #[test]
    fn find_overlap_full_prefix() {
        // "abc" suffix "c" matches "cde" prefix "c"
        assert_eq!(find_overlap("abc", "cde", 300), 1);
    }

    #[test]
    fn deduplicate_empty_fragments() {
        let result = deduplicate_and_join(&[]);
        assert_eq!(result, "");
    }

    #[test]
    fn deduplicate_single_fragment() {
        let frags = vec!["hello world".to_owned()];
        assert_eq!(deduplicate_and_join(&frags), "hello world");
    }

    #[test]
    fn deduplicate_no_overlap() {
        let frags = vec!["hello ".to_owned(), "world".to_owned()];
        assert_eq!(deduplicate_and_join(&frags), "hello world");
    }
}
