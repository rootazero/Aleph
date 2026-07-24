//! Render [`FlowOutcome`] to per-channel-friendly strings.
//!
//! Inspired by hermes-agent's `_print_exit_summary` (CLI) plus the
//! `gateway/delivery.py` truncation logic for cross-platform channels.
//! Each [`SummaryStyle`] variant produces a textual representation that
//! suits one delivery surface:
//!
//! | Style       | Audience            | Shape |
//! |-------------|---------------------|-------|
//! | `Plain`     | logs, headless CI   | one-line marker, no formatting |
//! | `Compact`   | CLI footer          | 2–3 lines, light glyphs |
//! | `Markdown`  | `WebChat` / Panel     | sectioned bullet list + tool table |
//! | `Telegram`  | messaging channels  | Markdown shape capped at 4000 chars |
//! | `Json`      | machine consumers   | pretty-printed JSON of the outcome |
//!
//! No channel-specific knowledge lives in this module — the renderer
//! consumes a [`FlowOutcome`] and returns a `String`. Channels decide which
//! style to ask for. This keeps the renderer testable in isolation and the
//! delivery wiring trivially swappable (R3 — core minimalism).

use crate::orchestrator::dispatch::{FlowOutcome, TerminateReason, ToolInvocation};

/// Channel-flavoured renderings of [`FlowOutcome`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SummaryStyle {
    /// Single-line, ASCII-only marker. Suitable for log lines and CI.
    Plain,
    /// 2–3 line CLI footer with light glyphs. Mirrors `_print_exit_summary`.
    Compact,
    /// Sectioned Markdown with per-tool table. Suitable for `WebChat` / Panel.
    Markdown,
    /// Same shape as Markdown but truncated to the cross-platform limit
    /// matched against hermes-agent's `MAX_PLATFORM_OUTPUT = 4000`.
    Telegram,
    /// Pretty-printed JSON of the outcome. Stable wire shape; preserves all
    /// fields including those skipped by other renderers.
    Json,
}

/// Hermes-agent `MAX_PLATFORM_OUTPUT`. Public so test fixtures can pin it.
pub const TELEGRAM_HARD_LIMIT: usize = 4_000;

impl FlowOutcome {
    /// Render this outcome for a specific channel style.
    #[must_use]
    pub fn render(&self, style: SummaryStyle) -> String {
        match style {
            SummaryStyle::Plain => render_plain(self),
            SummaryStyle::Compact => render_compact(self),
            SummaryStyle::Markdown => render_markdown(self),
            SummaryStyle::Telegram => render_telegram(self),
            SummaryStyle::Json => render_json(self),
        }
    }
}

// ─── Style: Plain ────────────────────────────────────────────────────────────

fn render_plain(o: &FlowOutcome) -> String {
    format!(
        "{} ({}ms) — {} iter / {} tools / {} tok",
        o.terminate_reason.as_static_str(),
        o.duration_ms,
        o.iterations,
        o.tool_calls_made,
        o.total_tokens,
    )
}

// ─── Style: Compact (CLI footer) ─────────────────────────────────────────────

fn render_compact(o: &FlowOutcome) -> String {
    let mark = if matches!(o.terminate_reason, TerminateReason::Completed) {
        "\u{2713}" // ✓
    } else if matches!(o.terminate_reason, TerminateReason::Cancelled) {
        "\u{2718}" // ✘
    } else {
        "\u{26a0}\u{fe0f}" // ⚠️
    };
    let mut out = String::new();
    out.push_str(&format!(
        "{mark} {reason} in {dur} | {iters} iter | {tools} tools | {toks} tok",
        reason = humanize_terminate(&o.terminate_reason),
        dur = format_duration_ms(o.duration_ms),
        iters = o.iterations,
        tools = o.tool_calls_made,
        toks = format_token_count(o.total_tokens.into()),
    ));
    if let Some(cost) = o.estimated_cost.as_ref() {
        if cost.status != crate::pricing::CostStatus::Unknown {
            out.push_str(&format!(" | ${:.4}", cost.usd));
        }
    }
    // Per-tool one-liner — only when at least one tool ran. Limited to the
    // first 4 entries so the footer stays scannable.
    if !o.tool_timeline.is_empty() {
        out.push('\n');
        let mut head = o
            .tool_timeline
            .iter()
            .take(4)
            .map(format_tool_glyph)
            .collect::<Vec<_>>()
            .join("  ");
        if o.tool_timeline.len() > 4 {
            head.push_str(&format!("  +{} more", o.tool_timeline.len() - 4));
        }
        out.push_str(&head);
    }
    out
}

// ─── Style: Markdown ─────────────────────────────────────────────────────────

fn render_markdown(o: &FlowOutcome) -> String {
    let mut out = String::new();
    out.push_str("**Summary**\n\n");
    out.push_str(&format!(
        "- **Outcome:** {}\n",
        humanize_terminate(&o.terminate_reason)
    ));
    out.push_str(&format!(
        "- **Duration:** {}\n",
        format_duration_ms(o.duration_ms)
    ));
    out.push_str(&format!("- **Iterations:** {}\n", o.iterations));
    out.push_str(&format!(
        "- **Tokens:** {} ({})\n",
        format_token_count(o.total_tokens.into()),
        format_token_breakdown(o),
    ));
    if let Some(cost) = o.estimated_cost.as_ref() {
        let status = match cost.status {
            crate::pricing::CostStatus::Complete => "complete",
            crate::pricing::CostStatus::PartialMissingPrice => "partial",
            crate::pricing::CostStatus::Unknown => "unknown",
        };
        out.push_str(&format!(
            "- **Estimated cost:** ${:.4} ({})\n",
            cost.usd, status
        ));
    }

    if !o.tool_timeline.is_empty() {
        out.push_str(&format!(
            "\n**Tools** ({} calls)\n\n",
            o.tool_timeline.len()
        ));
        out.push_str("| # | Tool | Duration | Result |\n");
        out.push_str("|---|------|----------|--------|\n");
        for (i, inv) in o.tool_timeline.iter().enumerate() {
            out.push_str(&format!(
                "| {} | {} {} | {} | {} |\n",
                i + 1,
                emoji_for(&inv.name),
                inv.name,
                format_duration_ms(inv.duration_ms),
                if inv.success {
                    "\u{2713}".to_string()
                } else {
                    let err = inv
                        .error
                        .as_deref()
                        .map(|e| truncate_with_ellipsis(e, 60))
                        .unwrap_or_default();
                    format!("\u{2718} {err}")
                },
            ));
        }
    }

    out
}

// ─── Style: Telegram (Markdown + hard cap) ───────────────────────────────────

fn render_telegram(o: &FlowOutcome) -> String {
    let md = render_markdown(o);
    if md.len() <= TELEGRAM_HARD_LIMIT {
        return md;
    }
    // Telegram's limit is in *bytes*, not chars — multi-byte glyphs (emojis,
    // CJK, the ellipsis itself) make char-count budgets overshoot. Walk char
    // boundaries to find the largest prefix that still leaves room for the
    // suffix marker.
    const SUFFIX: &str = "\n\u{2026} (output truncated)";
    let budget = TELEGRAM_HARD_LIMIT.saturating_sub(SUFFIX.len());
    let cutoff = md
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|&i| i <= budget)
        .last()
        .unwrap_or(0);
    let mut out = String::with_capacity(TELEGRAM_HARD_LIMIT);
    out.push_str(&md[..cutoff]);
    out.push_str(SUFFIX);
    out
}

// ─── Style: Json ─────────────────────────────────────────────────────────────

fn render_json(o: &FlowOutcome) -> String {
    // Manual JSON construction so `hit_limit` (a deprecated alias) is omitted
    // and the resulting blob is stable across alephcore versions — channels
    // that machine-parse this output should see the canonical fields only.
    let value = serde_json::json!({
        "final_text": o.final_text,
        "iterations": o.iterations,
        "tool_calls_made": o.tool_calls_made,
        "total_tokens": o.total_tokens,
        "duration_ms": o.duration_ms,
        "terminate_reason": o.terminate_reason,
        "token_breakdown": o.token_breakdown,
        "estimated_cost": o.estimated_cost,
        "tool_timeline": o.tool_timeline,
    });
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string())
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn humanize_terminate(r: &TerminateReason) -> String {
    match r {
        TerminateReason::Completed => "Completed".into(),
        TerminateReason::HitMaxIterations { used } => {
            format!("Max iterations reached ({used})")
        }
        TerminateReason::ContextBudgetExhausted => "Context budget exhausted".into(),
        TerminateReason::StallTimeout { elapsed_ms } => {
            format!("Stalled after {}", format_duration_ms(*elapsed_ms))
        }
        TerminateReason::TurnTimeout { phase, elapsed_ms } => {
            format!(
                "Turn timeout in {phase} ({})",
                format_duration_ms(*elapsed_ms)
            )
        }
        TerminateReason::ConsecutiveFailureCap { consecutive } => {
            format!("Consecutive-failure cap ({consecutive})")
        }
        TerminateReason::VerifierVeto { vetos } => {
            format!("Verifier veto cap ({vetos})")
        }
        TerminateReason::EmptyResponseExhausted => "Empty response from provider".into(),
        TerminateReason::StopHookHalt { reason } => format!("Stop hook halt: {reason}"),
        TerminateReason::MaxOutputTokensExhausted => "Max output tokens exhausted".into(),
        TerminateReason::DiminishingReturns => "Stopped early on diminishing returns".into(),
        TerminateReason::ReactiveCompactExhausted => {
            "Context overflow recovery failed (reactive compaction)".into()
        }
        TerminateReason::Cancelled => "Cancelled".into(),
        TerminateReason::BudgetExhaustedPartialResult { reason, .. } => {
            format!("Budget exhausted ({reason}) — partial result preserved for resume")
        }
    }
}

fn format_duration_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        let mins = ms / 60_000;
        let secs = (ms % 60_000) / 1000;
        format!("{mins}m {secs}s")
    }
}

fn format_token_count(n: u64) -> String {
    if n < 1_000 {
        format!("{n}")
    } else if n < 1_000_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

fn format_token_breakdown(o: &FlowOutcome) -> String {
    let b = &o.token_breakdown;
    let mut parts: Vec<String> = Vec::new();
    if b.input > 0 {
        parts.push(format!("input {}", format_token_count(b.input.into())));
    }
    if b.output > 0 {
        parts.push(format!("output {}", format_token_count(b.output.into())));
    }
    let cache_total = u64::from(b.cache_read) + u64::from(b.cache_creation);
    if cache_total > 0 {
        parts.push(format!("cache {}", format_token_count(cache_total)));
    }
    if b.reasoning > 0 {
        parts.push(format!(
            "reasoning {}",
            format_token_count(b.reasoning.into())
        ));
    }
    if parts.is_empty() {
        "no breakdown".into()
    } else {
        parts.join(" / ")
    }
}

fn format_tool_glyph(inv: &ToolInvocation) -> String {
    let mark = if inv.success { "\u{2713}" } else { "\u{2718}" };
    format!(
        "{emoji} {name} ({dur}) {mark}",
        emoji = emoji_for(&inv.name),
        name = inv.name,
        dur = format_duration_ms(inv.duration_ms),
    )
}

/// Same emoji table the `event_drain` mapper uses. Duplicated rather than
/// shared so the renderer stays free of gateway dependencies — adding a
/// new tool here doesn't touch the gateway layer.
fn emoji_for(name: &str) -> &'static str {
    match name {
        "bash" | "shell" => "\u{26a1}",
        "read" | "read_file" | "fs_read" => "\u{1f4d6}",
        "write" | "write_file" | "fs_write" => "\u{270d}\u{fe0f}",
        "edit" | "patch" => "\u{270f}\u{fe0f}",
        "web_search" | "search" | "google" => "\u{1f50d}",
        "browser" | "web_fetch" | "fetch" => "\u{1f310}",
        "memory_recall" | "memory_store" | "memory" => "\u{1f9e0}",
        "spawn_subagent" | "subagent" => "\u{1f916}",
        _ => "\u{1f527}",
    }
}

/// UTF-8-safe truncation. Hermes-agent does the same to avoid byte-slicing
/// in the middle of a multi-byte character on Telegram payloads.
fn truncate_with_ellipsis(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let cutoff = s
        .char_indices()
        .nth(max_chars.saturating_sub(1))
        .map_or(s.len(), |(i, _)| i);
    format!("{}\u{2026}", &s[..cutoff])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::dispatch::{TerminateReason, TokenBreakdown, ToolInvocation};

    fn outcome_with_tools() -> FlowOutcome {
        FlowOutcome {
            final_text: "All done.".into(),
            iterations: 3,
            tool_calls_made: 2,
            total_tokens: 1500,
            hit_limit: false,
            terminate_reason: TerminateReason::Completed,
            duration_ms: 1234,
            token_breakdown: TokenBreakdown {
                input: 800,
                output: 600,
                cache_read: 50,
                cache_creation: 50,
                reasoning: 0,
            },
            tool_timeline: vec![
                ToolInvocation {
                    id: "t1".into(),
                    name: "bash".into(),
                    duration_ms: 120,
                    success: true,
                    error: None,
                },
                ToolInvocation {
                    id: "t2".into(),
                    name: "web_search".into(),
                    duration_ms: 340,
                    success: false,
                    error: Some("404 not found".into()),
                },
            ],
            estimated_cost: None,
            serving_model: None,
            serving_provider: None,
            context_tokens: 0,
            context_window: 0,
        }
    }

    #[test]
    fn plain_is_single_line_ascii() {
        let s = outcome_with_tools().render(SummaryStyle::Plain);
        assert!(!s.contains('\n'));
        assert!(s.contains("completed"));
        assert!(s.contains("1234ms"));
        assert!(s.contains("3 iter"));
        assert!(s.contains("2 tools"));
    }

    #[test]
    fn compact_includes_tool_glyphs_when_timeline_nonempty() {
        let s = outcome_with_tools().render(SummaryStyle::Compact);
        assert!(s.contains("bash"));
        assert!(s.contains("web_search"));
        assert!(s.lines().count() >= 2);
    }

    #[test]
    fn markdown_uses_section_header_and_table() {
        let s = outcome_with_tools().render(SummaryStyle::Markdown);
        assert!(s.contains("**Summary**"));
        assert!(s.contains("**Tools** (2 calls)"));
        assert!(s.contains("| # | Tool | Duration | Result |"));
        // Failed tool row carries the error message.
        assert!(s.contains("404 not found"));
    }

    #[test]
    fn telegram_caps_output_at_4000_chars() {
        // Construct a synthetic outcome with many tools to overflow the cap.
        let mut outcome = outcome_with_tools();
        let inv = outcome.tool_timeline[0].clone();
        outcome.tool_timeline = std::iter::repeat_n(inv, 400).collect();
        let s = outcome.render(SummaryStyle::Telegram);
        assert!(
            s.len() <= TELEGRAM_HARD_LIMIT,
            "telegram render {len} > {cap}",
            len = s.len(),
            cap = TELEGRAM_HARD_LIMIT,
        );
        assert!(s.contains("output truncated"));
    }

    #[test]
    fn json_includes_terminate_reason_and_breakdown() {
        let s = outcome_with_tools().render(SummaryStyle::Json);
        let parsed: serde_json::Value = serde_json::from_str(&s).expect("valid json");
        assert!(parsed.get("terminate_reason").is_some());
        assert_eq!(parsed["iterations"], 3);
        assert_eq!(parsed["tool_calls_made"], 2);
        // hit_limit is intentionally omitted from the JSON style.
        assert!(parsed.get("hit_limit").is_none());
    }

    #[test]
    fn terminate_humanize_covers_every_variant() {
        // Ensure no panic on any variant; each produces a non-empty string.
        for r in [
            TerminateReason::Completed,
            TerminateReason::HitMaxIterations { used: 200 },
            TerminateReason::ContextBudgetExhausted,
            TerminateReason::StallTimeout { elapsed_ms: 30_000 },
            TerminateReason::TurnTimeout {
                phase: "think".into(),
                elapsed_ms: 60_000,
            },
            TerminateReason::ConsecutiveFailureCap { consecutive: 3 },
            TerminateReason::VerifierVeto { vetos: 10 },
            TerminateReason::Cancelled,
        ] {
            assert!(!humanize_terminate(&r).is_empty(), "{r:?}");
        }
    }

    #[test]
    fn duration_format_thresholds() {
        assert_eq!(format_duration_ms(500), "500ms");
        assert_eq!(format_duration_ms(1500), "1.5s");
        assert_eq!(format_duration_ms(125_000), "2m 5s");
    }

    #[test]
    fn token_format_thresholds() {
        assert_eq!(format_token_count(42), "42");
        assert_eq!(format_token_count(1_500), "1.5k");
        assert_eq!(format_token_count(2_500_000), "2.5M");
    }

    #[test]
    fn truncate_with_ellipsis_is_utf8_safe() {
        // Ensure we don't byte-slice in the middle of a 3-byte CJK char.
        let s = "abc中文def中文";
        let out = truncate_with_ellipsis(s, 5);
        assert!(out.ends_with('\u{2026}'));
        // Should not panic and should be valid UTF-8 (Rust string guarantees).
        let _ = out.chars().count();
    }
}
