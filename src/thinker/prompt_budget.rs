//! Token budget management for system prompt assembly.
//!
//! Prevents system prompt bloat by enforcing character limits and
//! head/tail-truncating the dynamic suffix to fit the model's window.

/// Fraction of the model's usable context window allotted to the assembled
/// system prompt before clamping. `0.10` keeps a 200k-token window at the
/// historical 80k-char ceiling (`200_000 * 0.10 * 3.5 = 70_000`, floor-clamped
/// up to `DEFAULT_PROMPT_CHARS`) while letting a 1M-token window scale up
/// proportionally instead of staying artificially capped. Mirrors the
/// history-side model-awareness in `orchestrator::deps_builder` (feature 2.2)
/// on the prompt side.
pub const PROMPT_WINDOW_FRACTION: f64 = 0.10;

/// Legacy fixed system-prompt character cap. Now doubles as the *floor* for the
/// model-aware budget: scaling the cap to the window only ever widens it for
/// large windows, never tightens it below what the fixed default already
/// allowed (so small/unknown-window models behave exactly as before).
pub const DEFAULT_PROMPT_CHARS: usize = 80_000;

/// Hard ceiling (~137k tokens at the 3.5 prose ratio) for the model-aware
/// system-prompt budget, so a mis-declared or enormous window can never let the
/// prompt grow unbounded.
pub const MAX_PROMPT_CHARS: usize = 480_000;

/// Scale a character budget to a model context window: take `fraction` of the
/// window (in tokens), widen tokens→chars via the crate-wide prose ratio
/// ([`pressure::DEFAULT_PROSE_RATIO`](crate::context::budget::pressure::DEFAULT_PROSE_RATIO)),
/// then clamp into `[floor, ceil]`. Single source of the "size a char cap to
/// the window" math shared by the system-prompt budget
/// ([`TokenBudget::from_context_window`]) and the identity / extra-file caps,
/// so the three stay consistent. `floor` pins each legacy fixed cap as a lower
/// bound and must not exceed `ceil` (callers pass compile-time constants that
/// satisfy this).
#[must_use]
pub fn window_char_budget(window_tokens: u64, fraction: f64, floor: usize, ceil: usize) -> usize {
    const MAX_PRECISE_F64: u64 = 1u64 << 53;
    let capped = window_tokens.min(MAX_PRECISE_F64);
    let scaled_chars =
        capped as f64 * fraction * crate::context::budget::pressure::DEFAULT_PROSE_RATIO;
    (scaled_chars as usize).clamp(floor, ceil)
}

/// Budget configuration for system prompt assembly.
#[derive(Debug, Clone)]
pub struct TokenBudget {
    /// Maximum total characters for assembled system prompt.
    /// Default: [`DEFAULT_PROMPT_CHARS`] (~20K tokens).
    pub max_total_chars: usize,
    /// Warning mode for truncation events.
    pub truncation_warning: TruncationWarning,
}

impl Default for TokenBudget {
    fn default() -> Self {
        Self {
            max_total_chars: DEFAULT_PROMPT_CHARS,
            truncation_warning: TruncationWarning::default(),
        }
    }
}

impl TokenBudget {
    /// Derive a system-prompt character budget from the model's usable context
    /// window (tokens), instead of the fixed [`DEFAULT_PROMPT_CHARS`]. A
    /// 200k-window model lands on the historical 80k-char ceiling; a 1M-window
    /// model gets proportionally more headroom before the dynamic suffix is
    /// trimmed. Clamped to `[DEFAULT_PROMPT_CHARS, MAX_PROMPT_CHARS]`.
    ///
    /// Pairs with `ContextBudgetConfig` (the history-side budget, feature 2.2):
    /// both size off the same chain-minimum window so the prompt and history
    /// views of "how much room is there" agree.
    #[must_use]
    pub fn from_context_window(window_tokens: u64) -> Self {
        Self {
            max_total_chars: window_char_budget(
                window_tokens,
                PROMPT_WINDOW_FRACTION,
                DEFAULT_PROMPT_CHARS,
                MAX_PROMPT_CHARS,
            ),
            ..Self::default()
        }
    }
}

/// Warning mode for truncation events.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TruncationWarning {
    /// Never warn.
    Off,
    /// Warn once per session per unique truncation state.
    #[default]
    Once,
    /// Warn every time.
    Always,
}

/// Byte offset where the `n`-th character begins (i.e. the end of the first
/// `n` characters). Returns the full byte length when `n` is at or past the
/// end. UTF-8 safe by construction — offsets always land on char boundaries.
fn char_byte_offset(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map_or(s.len(), |(i, _)| i)
}

/// First `n` characters of `s` as an owned string (UTF-8 safe).
fn take_chars(s: &str, n: usize) -> String {
    s[..char_byte_offset(s, n)].to_string()
}

/// Truncate content preserving head and tail, UTF-8 safe.
///
/// Keeps `head_ratio` of characters from the start and `tail_ratio` from the
/// end, inserting a truncation marker in between. **All budgeting is in
/// characters, never bytes** — a 3-byte CJK glyph counts as one unit, so
/// multi-byte text is truncated at the correct visual boundary instead of ~3×
/// too aggressively.
#[must_use]
pub fn truncate_with_head_tail(
    content: &str,
    max_chars: usize,
    head_ratio: f64,
    tail_ratio: f64,
) -> String {
    let total_chars = content.chars().count();
    if total_chars <= max_chars {
        return content.to_string();
    }

    let reserved_marker = format!("\n\n[... {total_chars} chars truncated ...]\n\n");

    // Too small for head+tail+marker: just take the head (char-accurate).
    if max_chars <= reserved_marker.len() {
        return take_chars(content, max_chars);
    }

    let usable = max_chars - reserved_marker.len();
    let sum = head_ratio + tail_ratio;
    let head_chars = if sum == 0.0 {
        usable / 2
    } else {
        (usable as f64 * head_ratio / sum) as usize
    };
    let tail_chars = usable.saturating_sub(head_chars);

    let truncated_count = total_chars
        .saturating_sub(head_chars)
        .saturating_sub(tail_chars);
    let marker = format!("\n\n[... {truncated_count} chars truncated ...]\n\n");

    // Char-accurate offsets: the first `head_chars` characters and the last
    // `tail_chars` characters. Since head_chars + tail_chars == usable <
    // total_chars, the head always ends before the tail begins (no overlap).
    let head_end = char_byte_offset(content, head_chars);
    let tail_start = char_byte_offset(content, total_chars.saturating_sub(tail_chars));

    let result = format!(
        "{}{}{}",
        &content[..head_end],
        marker,
        &content[tail_start..]
    );

    // Safety net in *characters* (the marker can nudge the total over budget).
    if result.chars().count() > max_chars {
        return take_chars(&result, max_chars);
    }

    result
}

/// Render a model-visible truncation notice for the assembled system prompt.
///
/// Activates the [`TruncationWarning`] policy (previously declared but never
/// consumed): `Off` stays silent (`None`); `Once` / `Always` emit a
/// `<system-reminder>` block so the model knows its per-request context was
/// trimmed to fit the prompt budget — letting it re-fetch specifics via tools
/// rather than assume it saw the full picture (openclaw near-limit-warning
/// parity).
///
/// Per-session dedup for `Once` would require session state that this pure
/// layer does not own, so `Once` and `Always` both render here; the caller is
/// free to suppress repeats. Returns `None` when nothing was trimmed.
#[must_use]
pub fn render_truncation_notice(mode: TruncationWarning, saved_chars: usize) -> Option<String> {
    if mode == TruncationWarning::Off || saved_chars == 0 {
        return None;
    }
    // Single-source the char→token conversion on the crate-wide prose ratio
    // (`pressure::DEFAULT_PROSE_RATIO`); this notice only has a char count, not
    // the removed text, so content-aware estimation isn't available — but it
    // must not diverge from every other estimate the way the old `/4` did.
    let approx_tokens =
        (saved_chars as f64 / crate::context::budget::pressure::DEFAULT_PROSE_RATIO) as usize;
    Some(format!(
        "\n\n<system-reminder>\n\
         Your per-request context was trimmed by ~{saved_chars} characters (~{approx_tokens} \
         tokens) to fit the system-prompt budget. Some dynamic context (memory, session, \
         runtime hints) may be incomplete — re-fetch specifics with tools rather than assuming \
         you saw everything.\n\
         </system-reminder>"
    ))
}

/// Fit the dynamic system-prompt suffix within the total budget, protecting
/// the stable prefix as a non-negotiable floor.
///
/// The stable prefix (persona / tools / security) is never touched: that keeps
/// the persona+tooling floor intact (hermes parity) and, crucially, preserves
/// Anthropic's prefix cache — the cache breakpoint sits exactly at the
/// stable/dynamic boundary, so trimming only the `cache: false` suffix leaves
/// the cached prefix byte-stable. Only the dynamic suffix (memory, session,
/// runtime hints) is head/tail truncated via [`truncate_with_head_tail`], with
/// a model-visible [`render_truncation_notice`] appended when content is cut.
///
/// Returns `dynamic` unchanged when the assembled prompt is already within
/// budget — the overwhelming common case, so this is a no-op (and byte-stable)
/// for normal-sized prompts.
///
/// `stable_len` is the stable prefix's **character** count (the budget is in
/// characters), so callers pass `stable.chars().count()`. All measurement here
/// is character-based to match [`truncate_with_head_tail`] — a CJK glyph counts
/// once, not thrice.
#[must_use]
pub fn fit_dynamic_suffix(stable_len: usize, dynamic: String, budget: &TokenBudget) -> String {
    if stable_len + dynamic.chars().count() <= budget.max_total_chars {
        return dynamic;
    }
    // Reserve headroom for the notice so the final string stays near budget.
    const NOTICE_RESERVE: usize = 400;
    let avail = budget
        .max_total_chars
        .saturating_sub(stable_len)
        .saturating_sub(NOTICE_RESERVE);
    let before = dynamic.chars().count();
    let trimmed = truncate_with_head_tail(&dynamic, avail, 0.6, 0.3);
    let saved = before.saturating_sub(trimmed.chars().count());
    match render_truncation_notice(budget.truncation_warning, saved) {
        Some(notice) => format!("{trimmed}{notice}"),
        None => trimmed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_budget_values() {
        let b = TokenBudget::default();
        assert_eq!(b.max_total_chars, 80_000);
        assert_eq!(b.truncation_warning, TruncationWarning::Once);
    }

    #[test]
    fn window_budget_matches_legacy_at_200k() {
        // 200k window * 0.10 * 3.5 == 70_000, floor-clamped up to the
        // historical fixed cap (80_000), so the common mid-window model is
        // byte-identical to the old behaviour.
        assert_eq!(
            TokenBudget::from_context_window(200_000).max_total_chars,
            DEFAULT_PROMPT_CHARS
        );
    }

    #[test]
    fn window_budget_widens_for_large_windows() {
        // 1M window scales up proportionally (968k usable would too).
        assert_eq!(
            TokenBudget::from_context_window(1_000_000).max_total_chars,
            350_000 // 100k tokens × 3.5 chars/token (single-source prose ratio)
        );
    }

    #[test]
    fn window_budget_floors_small_windows_at_legacy() {
        // A tiny / mis-declared window never drops below the legacy fixed cap.
        assert_eq!(
            TokenBudget::from_context_window(8_000).max_total_chars,
            DEFAULT_PROMPT_CHARS
        );
        assert_eq!(
            TokenBudget::from_context_window(0).max_total_chars,
            DEFAULT_PROMPT_CHARS
        );
    }

    #[test]
    fn window_budget_ceils_enormous_windows() {
        // Beyond ~1.2M tokens the budget saturates at the hard ceiling.
        assert_eq!(
            TokenBudget::from_context_window(10_000_000).max_total_chars,
            MAX_PROMPT_CHARS
        );
    }

    #[test]
    fn window_char_budget_clamps_both_ends() {
        assert_eq!(window_char_budget(1_000, 0.10, 5_000, 50_000), 5_000);
        assert_eq!(window_char_budget(1_000_000, 0.10, 5_000, 50_000), 50_000);
        // Unclamped middle case: 50_000 * 0.10 * 3.5 (single-source prose ratio).
        assert_eq!(window_char_budget(50_000, 0.10, 5_000, 50_000), 17_500);
    }

    #[test]
    fn window_char_budget_handles_extreme_u64_safely() {
        assert_eq!(
            window_char_budget(u64::MAX, 0.10, 5_000, 50_000),
            50_000,
            "u64::MAX must clamp to ceil without f64 precision artifacts"
        );
        assert_eq!(
            window_char_budget(u64::MAX >> 1, 0.10, 5_000, 50_000),
            50_000,
            "huge values must clamp to ceil"
        );
        assert_eq!(
            window_char_budget(1u64 << 53, 0.10, 5_000, 50_000),
            window_char_budget(1u64 << 52, 0.10, 5_000, 50_000),
            "2^53 and 2^52 must round to the same usize (precision cap kicks in)"
        );
    }

    #[test]
    fn truncate_short_content_unchanged() {
        let content = "Hello, world!";
        let result = truncate_with_head_tail(content, 100, 0.7, 0.2);
        assert_eq!(result, content);
    }

    #[test]
    fn truncate_marker_intact_when_truncated_count_has_many_digits() {
        let content: String = "A".repeat(100_000);
        let result = truncate_with_head_tail(&content, 60, 0.7, 0.2);
        assert!(
            result.contains("truncated ...]\n\n"),
            "marker closing clipped: {result:?}"
        );
        assert!(
            result.chars().count() <= 60,
            "result must respect budget, got {} chars",
            result.chars().count()
        );
    }

    #[test]
    fn truncate_marker_with_six_digit_count_stays_within_budget() {
        let content: String = "B".repeat(1_000_000);
        let result = truncate_with_head_tail(&content, 80, 0.5, 0.5);
        assert!(result.contains("chars truncated ...]"));
        assert!(result.chars().count() <= 80);
    }

    #[test]
    fn truncate_long_content_preserves_head_tail() {
        let content = "A".repeat(1000);
        let result = truncate_with_head_tail(&content, 100, 0.7, 0.2);
        assert!(result.len() < 1000);
        assert!(result.contains("[..."));
        assert!(result.contains("truncated ...]"));
        assert!(result.starts_with("AAAA"));
        assert!(result.ends_with("AAAA"));
    }

    #[test]
    fn truncate_multibyte_utf8_safe() {
        let content = "你好世界".repeat(100);
        let result = truncate_with_head_tail(&content, 50, 0.7, 0.2);
        assert!(result.contains("[..."));
        // Should not panic
    }

    #[test]
    fn truncate_cjk_budgets_in_chars_not_bytes() {
        // Regression for the char/byte unit-confusion bug: head/tail were
        // computed as character counts but applied as byte offsets, so 3-byte
        // CJK glyphs were truncated ~3× too aggressively (and the byte-vs-char
        // final guard usually nuked the tail entirely). Distinct head/tail
        // glyphs let us prove the fix keeps the right *number of characters*.
        let content = format!("{}{}{}", "甲".repeat(20), "乙".repeat(200), "丙".repeat(20));
        let out = truncate_with_head_tail(&content, 60, 0.7, 0.2);

        // Char budget is respected (the old code measured this in bytes).
        assert!(out.chars().count() <= 60, "got {}", out.chars().count());
        // Head keeps ~21 chars (0.7 share of ~27 usable) — all 20 leading 甲
        // survive. The old byte-based math kept only ~7 chars of head.
        let head_jia = out.chars().take_while(|&c| c == '甲').count();
        assert!(
            head_jia >= 10,
            "expected char-based head, kept only {head_jia} 甲 (byte-based bug keeps ~7)"
        );
        // The tail is preserved (byte-vs-char guard no longer discards it).
        assert!(out.ends_with('丙'), "tail glyphs must survive: {out}");
        assert!(out.contains("truncated"));
    }

    #[test]
    fn notice_off_is_silent() {
        assert!(render_truncation_notice(TruncationWarning::Off, 1234).is_none());
    }

    #[test]
    fn notice_zero_saved_is_silent() {
        // Nothing trimmed → no notice even when warnings are enabled.
        assert!(render_truncation_notice(TruncationWarning::Always, 0).is_none());
        assert!(render_truncation_notice(TruncationWarning::Once, 0).is_none());
    }

    #[test]
    fn notice_reports_saved_chars_in_system_reminder() {
        let notice = render_truncation_notice(TruncationWarning::Once, 4096)
            .expect("notice rendered when content trimmed");
        assert!(notice.contains("<system-reminder>"));
        assert!(notice.contains("4096"));
        assert!(notice.contains("trimmed"));
        // Reports the approximate token cost too (4096 / 3.5 ≈ 1170,
        // single-sourced on the crate-wide prose ratio, not the old /4).
        assert!(
            notice.contains("1170"),
            "notice should report ~tokens: {notice}"
        );
    }

    #[test]
    fn fit_dynamic_under_budget_is_byte_identical() {
        // Common path: assembled prompt within budget → suffix untouched.
        let budget = TokenBudget::default();
        let dynamic = "session context".to_string();
        let out = fit_dynamic_suffix(1000, dynamic.clone(), &budget);
        assert_eq!(
            out, dynamic,
            "under-budget suffix must pass through unchanged"
        );
    }

    #[test]
    fn fit_dynamic_over_budget_trims_and_warns() {
        let budget = TokenBudget {
            max_total_chars: 2000,
            ..TokenBudget::default()
        };
        // Stable prefix is a protected floor of 500 chars; dynamic is huge.
        let stable_len = 500;
        let dynamic = "D".repeat(50_000);
        let out = fit_dynamic_suffix(stable_len, dynamic, &budget);
        // Suffix shrank well below its original size...
        assert!(out.len() < 50_000);
        // ...and the model is told it happened.
        assert!(out.contains("<system-reminder>"));
        assert!(out.contains("trimmed"));
        // Total (stable + trimmed suffix) stays in the neighbourhood of budget.
        assert!(stable_len + out.len() <= budget.max_total_chars + 600);
    }

    #[test]
    fn fit_dynamic_over_budget_off_warning_trims_silently() {
        let budget = TokenBudget {
            max_total_chars: 1500,
            truncation_warning: TruncationWarning::Off,
        };
        let out = fit_dynamic_suffix(200, "D".repeat(40_000), &budget);
        assert!(out.len() < 40_000, "still trims to protect the budget");
        assert!(
            !out.contains("<system-reminder>"),
            "Off policy must not emit a model-visible notice",
        );
    }

    #[test]
    fn fit_dynamic_stable_exceeds_budget_drops_suffix() {
        // Pathological: stable floor alone exceeds budget → suffix collapses,
        // stable is still protected (never touched here), model warned.
        let budget = TokenBudget {
            max_total_chars: 100,
            ..TokenBudget::default()
        };
        let out = fit_dynamic_suffix(5000, "D".repeat(2000), &budget);
        assert!(out.contains("<system-reminder>"));
    }
}
