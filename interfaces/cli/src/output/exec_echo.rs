//! Terminal execution-echo renderer.
//!
//! Turns the rich agent-loop event stream (tool calls, reasoning, scratchpad
//! progress, run summary) into a hierarchical, highly-readable text stream so
//! the user can follow a background run step-by-step instead of staring at a
//! black box.
//!
//! The protocol layer (`aleph_protocol::AgentTraceEvent` + `RunSummary`)
//! already carries every signal we need — tool args, per-call duration,
//! success/error, the scratchpad checklist (in the scratchpad tool's result
//! `content`), and the terminate cause. This module is the *renderable seam*;
//! it adds no new protocol surface (wire-first).
//!
//! Design borrows from two reference agents:
//! - hermes-agent `build_tool_preview`: a per-tool primary-arg preview table
//!   rendered as a single line with an icon + duration.
//! - openclaw activity entries: status badge (✓/✗) + duration + result
//!   preview + secret redaction.
//!
//! All functions are pure (no I/O) so they unit-test without a daemon. Colour
//! and Unicode degrade through the shared [`theme`]/[`icon`] gates, so output
//! stays clean when piped or on ASCII-only locales.

use aleph_protocol::{AgentTraceToolResult, RunSummary};
use serde_json::Value;

use super::icon::use_unicode;
use super::theme::{paint, Style};

/// Max display cells of a tool's primary-arg preview before ellipsis.
const PREVIEW_MAX: usize = 72;
/// Max display cells of a success result preview (verbose mode).
const RESULT_MAX: usize = 100;
/// Max display cells of an error message inlined on the result line.
const ERROR_MAX: usize = 160;
/// Max display cells of a reasoning preview before ellipsis.
const REASONING_MAX: usize = 200;

/// Per-tool glyph. Unicode emoji with a terse ASCII fallback so the column
/// still reads on `LANG=C` / `ALEPH_ASCII=1`.
pub fn tool_icon(tool_name: &str) -> &'static str {
    let uni = use_unicode();
    match tool_name {
        "bash" | "code_exec" | "code_check" | "system" | "automation" => {
            if uni {
                "💻"
            } else {
                "$"
            }
        }
        "search" => {
            if uni {
                "🔍"
            } else {
                "?"
            }
        }
        "web_fetch" => {
            if uni {
                "🌐"
            } else {
                "@"
            }
        }
        "scratchpad" => {
            if uni {
                "📋"
            } else {
                "#"
            }
        }
        "vision" | "media" | "media_send" => {
            if uni {
                "🖼"
            } else {
                "*"
            }
        }
        "remember" | "note_manage" | "vault_store" => {
            if uni {
                "📝"
            } else {
                "+"
            }
        }
        n if n.starts_with("memory_")
            || n.starts_with("recall_")
            || n == "ctx_search"
            || n == "session_search" =>
        {
            if uni {
                "🧠"
            } else {
                "~"
            }
        }
        "ask_user" => {
            if uni {
                "❓"
            } else {
                "?"
            }
        }
        "session_complete" => {
            if uni {
                "🏁"
            } else {
                "="
            }
        }
        n if n.starts_with("skill_") => {
            if uni {
                "🧩"
            } else {
                "&"
            }
        }
        _ => {
            if uni {
                "🔧"
            } else {
                "-"
            }
        }
    }
}

/// Extract a single-line, human-readable preview of a tool call's primary
/// argument — ports hermes-agent's `build_tool_preview` dispatch table to the
/// tool names Aleph actually registers. Falls back to a compact JSON snippet
/// for tools without a bespoke handler.
pub fn tool_preview(tool_name: &str, input: &Value) -> String {
    let s = |key: &str| input.get(key).and_then(Value::as_str).unwrap_or("");
    let raw = match tool_name {
        "bash" | "code_exec" | "code_check" => s("cmd"),
        "search" => s("query"),
        "web_fetch" => s("url"),
        "remember" | "ask_user" => s("text"),
        "vision" => s("prompt"),
        "scratchpad" => return scratchpad_preview(input),
        _ => {
            // Generic: first string-valued field, else compact JSON.
            if let Some(first) = input
                .as_object()
                .and_then(|o| o.values().find_map(Value::as_str).filter(|v| !v.is_empty()))
            {
                first
            } else {
                return truncate(&compact_json(input), PREVIEW_MAX);
            }
        }
    };
    truncate(&redact(raw).replace('\n', " "), PREVIEW_MAX)
}

/// Scratchpad calls preview their `action` (+ index/value) rather than a raw
/// arg, mirroring the action-aware push the channel sink already does.
fn scratchpad_preview(input: &Value) -> String {
    let action = input.get("action").and_then(Value::as_str).unwrap_or("?");
    match action {
        "set_plan" => {
            let n = input
                .get("items")
                .and_then(Value::as_array)
                .map_or(0, std::vec::Vec::len);
            format!("set_plan · {n} step(s)")
        }
        "start_item" | "complete_item" => {
            let idx = input
                .get("item_index")
                .and_then(Value::as_u64)
                .map_or_else(|| "?".into(), |i| i.to_string());
            format!("{action} #{idx}")
        }
        "set_objective" | "initialize" => {
            let v = input.get("value").and_then(Value::as_str).unwrap_or("");
            if v.is_empty() {
                action.to_string()
            } else {
                format!("{action} · {}", truncate(v, 48))
            }
        }
        other => other.to_string(),
    }
}

/// Line shown the instant a tool starts — gives "now running X" feedback.
///
/// `▸ <icon> <name>  <preview>`
pub fn render_tool_start(tool_name: &str, input: &Value) -> String {
    let arrow = if use_unicode() { "▸" } else { ">" };
    let preview = tool_preview(tool_name, input);
    let head = format!(
        "{arrow} {} {}",
        tool_icon(tool_name),
        paint(Style::Bold, tool_name)
    );
    if preview.is_empty() {
        head
    } else {
        format!("{head}  {}", paint(Style::Muted, &preview))
    }
}

/// Line(s) shown when a tool finishes. `verbose` adds a success-output
/// preview; errors are always surfaced. Returns the rendered block, or `None`
/// when there is nothing worth printing (silent success in non-verbose mode).
pub fn render_tool_end(
    tool_name: &str,
    result: &AgentTraceToolResult,
    duration_ms: u64,
    verbose: bool,
) -> Option<String> {
    let dur = paint(Style::Muted, &format_duration(duration_ms));
    match result {
        AgentTraceToolResult::Error { error, .. } => {
            let mark = paint(Style::Error, mark_fail());
            let msg = paint(
                Style::Error,
                &truncate(&error.replace('\n', " "), ERROR_MAX),
            );
            Some(format!("  {mark} {msg}  {dur}"))
        }
        AgentTraceToolResult::Success { output } => {
            // The scratchpad checklist is the headline progress signal —
            // always render it, regardless of verbosity.
            if tool_name == "scratchpad" {
                if let Some(content) = output.get("content").and_then(Value::as_str) {
                    return Some(render_scratchpad_block(content));
                }
            }
            let mark = paint(Style::Success, mark_ok());
            if verbose {
                let preview = success_preview(output);
                if preview.is_empty() {
                    Some(format!("  {mark} {dur}"))
                } else {
                    Some(format!("  {mark} {}  {dur}", paint(Style::Muted, &preview)))
                }
            } else {
                Some(format!("  {mark} {dur}"))
            }
        }
    }
}

/// Render the scratchpad progress / completion checklist as an indented block.
/// The content is the model's own checklist (`render_progress` /
/// `render_completion`), so colouring is purely cosmetic — checked boxes go
/// green, the in-progress marker cyan, the `✅ 目标达成` banner bold-green.
pub fn render_scratchpad_block(content: &str) -> String {
    let mut out = String::new();
    for (i, line) in content.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let trimmed = line.trim_start();
        let styled = if trimmed.starts_with("✅")
            || trimmed.starts_with("- [x]")
            || trimmed.starts_with("- [X]")
        {
            paint(Style::Success, line)
        } else if trimmed.starts_with("- [~]") {
            paint(Style::Info, line)
        } else if trimmed.starts_with("- [ ]") {
            paint(Style::Muted, line)
        } else {
            paint(Style::Header, line)
        };
        out.push_str("  ");
        out.push_str(&styled);
    }
    out
}

/// Dimmed reasoning preview — surfaces the model's internal thinking without
/// drowning the stream. Collapses to the first non-empty lines up to a cap.
pub fn render_reasoning(content: &str) -> Option<String> {
    let txt = content.trim();
    if txt.is_empty() {
        return None;
    }
    let bulb = if use_unicode() { "💭" } else { ":" };
    let preview = truncate(&txt.replace('\n', " "), REASONING_MAX);
    Some(format!("  {bulb} {}", paint(Style::Muted, &preview)))
}

/// Amber status line for a transient provider failure (`stream.run_retrying`).
/// Mirrors the Panel's retry notice so a CLI run shows "retrying" instead of
/// minutes of silence while the provider retry ladder burns attempts.
pub fn render_retry_notice(
    provider: &str,
    attempt: u32,
    max_attempts: u32,
    reason: &str,
) -> String {
    let glyph = if use_unicode() { "⟳" } else { "[~]" };
    let head = paint(
        Style::Warning,
        &format!("{glyph} provider {provider} retrying ({attempt}/{max_attempts})"),
    );
    let detail = truncate(&reason.replace('\n', " "), ERROR_MAX);
    if detail.is_empty() {
        format!("  {head}")
    } else {
        format!("  {head}  {}", paint(Style::Muted, &detail))
    }
}

/// Amber notice when the router fell back to a different model than the one
/// requested (`stream.model_resolved` with `is_fallback`). Mirrors the
/// Panel's fallback chip so terminal users learn which model actually
/// answered.
pub fn render_fallback_notice(model: &str, provider: &str, original: Option<&str>) -> String {
    let glyph = if use_unicode() { "⤷" } else { "[>]" };
    let head = match original {
        Some(orig) if orig != model => format!("{glyph} model fallback: {orig} → {model}"),
        _ => format!("{glyph} model fallback: {model}"),
    };
    format!(
        "  {}  {}",
        paint(Style::Warning, &head),
        paint(Style::Muted, &format!("via {provider}"))
    )
}

/// Prominent block when the agent pauses to ask the user a question
/// (`stream.ask_user`). Previously dropped on the CLI follow path, leaving an
/// interactive run looking hung. Shows the question and any offered options so
/// the user knows an answer is expected.
pub fn render_ask_user(question: &str, options: &[String]) -> String {
    let glyph = if use_unicode() { "❓" } else { "[?]" };
    let head = paint(Style::Header, &format!("{glyph} {question}"));
    if options.is_empty() {
        format!("  {head}")
    } else {
        let opts = options
            .iter()
            .map(|o| format!("    • {}", paint(Style::Info, o)))
            .collect::<Vec<_>>()
            .join("\n");
        format!("  {head}\n{opts}")
    }
}

/// Dimmed line when the model signals it is uncertain about how to proceed
/// (`stream.uncertainty_signal`). Surfaces the doubt so the user can step in
/// rather than the run silently guessing.
pub fn render_uncertainty(uncertainty: &str) -> Option<String> {
    let txt = uncertainty.trim();
    if txt.is_empty() {
        return None;
    }
    let glyph = if use_unicode() { "⚠" } else { "[!]" };
    Some(format!(
        "  {} {}",
        paint(Style::Warning, glyph),
        paint(Style::Muted, &truncate(txt, REASONING_MAX))
    ))
}

/// Status line for a reactive context-compaction rescue (`agent_trace`
/// `reactive_compaction_attempted`): the prompt overflowed the model window,
/// so the harness summarised history and retried. Tells the user *what
/// problem* (context too long), *how it was handled* (compacted), and the
/// *outcome* — instead of an unexplained pause mid-run.
pub fn render_compaction_notice(token_gap: Option<usize>, succeeded: bool) -> String {
    let glyph = if use_unicode() { "🗜" } else { "[~]" };
    let gap = token_gap
        .map(|g| format!(" (over by ~{g}t)"))
        .unwrap_or_default();
    let msg = if succeeded {
        format!("{glyph} context full{gap}: compacted history and retried")
    } else {
        format!("{glyph} context full{gap}: compaction retry failed")
    };
    let style = if succeeded {
        Style::Warning
    } else {
        Style::Error
    };
    format!("  {}", paint(style, &msg))
}

/// Status line for a structural goal-loop watchdog veto (`agent_trace`
/// `verifier_veto`): the model tried to finish but its scratchpad checklist
/// still has unchecked items, so the harness forced another iteration.
/// Surfaces the interception reason so a CLI run shows *why* it kept going
/// instead of looking like a silent extra turn.
pub fn render_veto_notice(reason: &str) -> String {
    let glyph = if use_unicode() { "↻" } else { "[!]" };
    let head = paint(
        Style::Warning,
        &format!("{glyph} goal-loop: checklist incomplete, continuing"),
    );
    let detail = truncate(&reason.replace('\n', " "), ERROR_MAX);
    if detail.is_empty() {
        format!("  {head}")
    } else {
        format!("  {head}  {}", paint(Style::Muted, &detail))
    }
}

/// Optional LLM-authored one-liner summarising a tool call.
pub fn render_tool_summary(summary: &str) -> Option<String> {
    let txt = summary.trim();
    if txt.is_empty() {
        return None;
    }
    let info = paint(Style::Info, super::icon::info());
    Some(format!(
        "  {info} {}",
        paint(Style::Muted, &truncate(txt, RESULT_MAX))
    ))
}

/// Closing footer rendered from the run summary — the "execution receipt".
/// One rule line + a stat line: status, tool/loop counts, tokens, duration,
/// cost. Non-`completed` terminates render a warning with a human label, and
/// any tool errors are listed beneath.
pub fn render_summary_footer(summary: &RunSummary) -> String {
    let mut lines: Vec<String> = Vec::new();
    let rule: String = std::iter::repeat_n(super::box_draw::horizontal(), 40).collect();
    lines.push(paint(Style::Muted, &rule));

    let reason = summary.terminate_reason.as_deref().unwrap_or("completed");
    let (mark, label, style) = terminate_badge(reason);
    let mut stats = vec![paint(style, &format!("{mark} {label}"))];
    if summary.tool_calls > 0 {
        stats.push(format!("{} tools", summary.tool_calls));
    }
    if summary.loops > 0 {
        stats.push(format!("{} loops", summary.loops));
    }
    if summary.total_tokens > 0 {
        stats.push(format!("{} tokens", human_count(summary.total_tokens)));
    }
    if let Some(ms) = summary.duration_ms {
        stats.push(format_duration(ms));
    }
    if let Some(cost) = summary.estimated_cost_usd {
        if cost > 0.0 {
            stats.push(format!("${cost:.4}"));
        }
    }
    let sep = paint(Style::Muted, " · ");
    lines.push(stats.join(&sep));

    for err in &summary.errors {
        let mark = paint(Style::Error, mark_fail());
        let tool = paint(Style::Bold, &err.tool_name);
        let msg = truncate(&err.error.replace('\n', " "), ERROR_MAX);
        lines.push(format!("  {mark} {tool}: {}", paint(Style::Error, &msg)));
    }

    lines.join("\n")
}

/// Reconcile the streamed assistant text with the authoritative
/// `RunSummary.final_response` that `RunComplete` carries.
///
/// The gateway channel emitter already does this — `reply_emitter` falls back
/// to `summary.final_response` when its streamed buffer is empty
/// (`reply_emitter/emitter/streaming.rs`). The CLI's `ask` path streamed
/// `ResponseChunk`s into a local buffer but never consulted the summary
/// fallback, so a run that emitted only reasoning + tool calls (no
/// `ResponseChunk`) printed an empty body and wrote an empty
/// `--output-last-message` file even though the final answer was on the wire.
///
/// Mirrors codex `final_message_from_turn_items` (late final-message
/// extraction) plus its dedup rule: streamed text wins when present (it was
/// already shown inline), and the summary fallback is consulted only when
/// nothing streamed. Pure, zero-alloc, host-testable.
pub fn resolve_final_text<'a>(streamed: &'a str, fallback: Option<&'a str>) -> &'a str {
    if !streamed.trim().is_empty() {
        return streamed;
    }
    match fallback {
        Some(f) if !f.trim().is_empty() => f,
        _ => streamed,
    }
}

/// Map a `TerminateReason` static tag to (glyph, human label, style).
fn terminate_badge(reason: &str) -> (&'static str, &'static str, Style) {
    match reason {
        "completed" => (mark_ok(), "完成", Style::Success),
        "verifier_veto" => (mark_warn(), "目标未达成（已暂停等待指示）", Style::Warning),
        "consecutive_failure_cap" => (mark_warn(), "任务受阻（连续失败熔断）", Style::Warning),
        "hit_max_iterations" => (mark_warn(), "达到最大轮次", Style::Warning),
        "context_budget_exhausted" | "max_output_tokens_exhausted" => {
            (mark_warn(), "上下文预算耗尽", Style::Warning)
        }
        "stall_timeout" | "turn_timeout" => (mark_warn(), "执行超时", Style::Warning),
        "cancelled" => (mark_warn(), "已取消", Style::Warning),
        "budget_exhausted_partial_result" => (mark_warn(), "预算耗尽（部分结果）", Style::Warning),
        _ => (mark_warn(), "已结束", Style::Warning),
    }
}

// ── small helpers ───────────────────────────────────────────────────────

fn mark_ok() -> &'static str {
    if use_unicode() {
        "✓"
    } else {
        "[ok]"
    }
}

fn mark_fail() -> &'static str {
    if use_unicode() {
        "✗"
    } else {
        "[x]"
    }
}

fn mark_warn() -> &'static str {
    if use_unicode() {
        "⚠"
    } else {
        "[!]"
    }
}

/// Human-friendly duration: `340ms`, `1.2s`, `2m03s`.
pub fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        let secs = ms / 1000;
        format!("{}m{:02}s", secs / 60, secs % 60)
    }
}

/// Compact a token count: `820`, `12.3k`, `1.2M`.
pub fn human_count(n: u64) -> String {
    if n < 1000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

/// Display-width-aware truncation with a trailing ellipsis. Never splits a
/// UTF-8 scalar, and budgets by terminal cells (CJK / full-width chars = 2,
/// combining marks = 0) rather than scalar count, so `max` is a true visible
/// width. A `chars().take()` budget double-counts CJK: an "80-char" preview of
/// Chinese text is 160 cells and wraps the one-line activity row. Counting
/// cells keeps previews on one line for Aleph's Chinese-speaking users (the
/// same concern that drives [`super::width`]). Mirrors the width-aware
/// truncation in the Pi / openclaw terminal cores.
fn truncate(s: &str, max: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    if super::width::display_width(s) <= max {
        return s.to_string();
    }
    let ell = if use_unicode() { "…" } else { "..." };
    // Reserve the ellipsis's own cells so the result never exceeds `max`.
    let budget = max.saturating_sub(super::width::display_width(ell));
    let mut width = 0usize;
    let mut head = String::new();
    for c in s.chars() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if width + cw > budget {
            break;
        }
        width += cw;
        head.push(c);
    }
    format!("{head}{ell}")
}

/// Best-effort preview of a successful tool's output Value: prefer a
/// human-ish field, else a compact JSON snippet.
fn success_preview(output: &Value) -> String {
    if let Some(s) = output.as_str() {
        return truncate(&redact(s).replace('\n', " "), RESULT_MAX);
    }
    if let Some(obj) = output.as_object() {
        for key in ["message", "summary", "result", "output", "content"] {
            if let Some(v) = obj.get(key).and_then(Value::as_str) {
                if !v.is_empty() {
                    return truncate(&redact(v).replace('\n', " "), RESULT_MAX);
                }
            }
        }
    }
    truncate(&compact_json(output), RESULT_MAX)
}

fn compact_json(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

/// Light secret redaction for previewed strings — strips obvious key-shaped
/// tokens so a tool arg/result echo never leaks a credential to the terminal.
/// Mirrors the intent of openclaw's redaction pass (kept deliberately small).
fn redact(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for word in s.split_inclusive(char::is_whitespace) {
        let trimmed = word.trim_end();
        if looks_secret(trimmed) {
            let tail = &word[trimmed.len()..];
            out.push_str("«redacted»");
            out.push_str(tail);
        } else {
            out.push_str(word);
        }
    }
    out
}

fn looks_secret(w: &str) -> bool {
    let lower = w.to_ascii_lowercase();
    (lower.starts_with("sk-") && w.len() >= 12)
        || (lower.starts_with("bearer") && w.len() >= 16)
        || (lower.starts_with("ghp_") && w.len() >= 12)
        || (lower.starts_with("xoxb-") && w.len() >= 12)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn duration_scales_units() {
        assert_eq!(format_duration(340), "340ms");
        assert_eq!(format_duration(1200), "1.2s");
        assert_eq!(format_duration(123_000), "2m03s");
    }

    #[test]
    fn count_compacts() {
        assert_eq!(human_count(820), "820");
        assert_eq!(human_count(12_345), "12.3k");
        assert_eq!(human_count(1_200_000), "1.2M");
    }

    #[test]
    fn truncate_is_char_safe() {
        // 5 multibyte scalars; truncating to 3 keeps 2 + ellipsis, no panic.
        let s = "你好世界中";
        let t = truncate(s, 3);
        assert!(t.chars().count() <= 3);
    }

    #[test]
    fn truncate_budgets_by_display_width() {
        use crate::output::width::display_width;
        // Each CJK char is 2 cells: "你好世界中" is 10 cells wide. A scalar-count
        // budget keeps all 5 chars at max=6 (10 cells, overflow); a cell budget
        // must never exceed 6 regardless of the ellipsis glyph (… or ...).
        let s = "你好世界中";
        let t = truncate(s, 6);
        assert!(
            display_width(&t) <= 6,
            "truncated {t:?} is {} cells, over budget 6",
            display_width(&t)
        );
        // It must actually have shrunk (input was 10 cells).
        assert!(display_width(&t) < display_width(s));
        // ASCII within budget is returned untouched.
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn bash_preview_extracts_cmd() {
        let input = json!({"cmd": "git status"});
        assert_eq!(tool_preview("bash", &input), "git status");
    }

    #[test]
    fn search_preview_extracts_query() {
        let input = json!({"query": "rust async"});
        assert_eq!(tool_preview("search", &input), "rust async");
    }

    #[test]
    fn scratchpad_preview_is_action_aware() {
        assert_eq!(
            tool_preview(
                "scratchpad",
                &json!({"action": "set_plan", "items": ["a", "b", "c"]})
            ),
            "set_plan · 3 step(s)"
        );
        assert_eq!(
            tool_preview(
                "scratchpad",
                &json!({"action": "complete_item", "item_index": 2})
            ),
            "complete_item #2"
        );
    }

    #[test]
    fn generic_preview_falls_back_to_first_string() {
        let input = json!({"foo": 1, "label": "hello"});
        assert_eq!(tool_preview("unknown_tool", &input), "hello");
    }

    #[test]
    fn tool_end_error_is_surfaced() {
        let r = AgentTraceToolResult::Error {
            error: "boom".into(),
            retryable: false,
        };
        let out = render_tool_end("bash", &r, 50, false).unwrap();
        assert!(out.contains("boom"));
    }

    #[test]
    fn tool_end_success_silent_unless_verbose() {
        let r = AgentTraceToolResult::Success {
            output: json!({"message": "did a thing"}),
        };
        // non-verbose: no preview text, just mark + duration
        let quiet = render_tool_end("bash", &r, 50, false).unwrap();
        assert!(!quiet.contains("did a thing"));
        // verbose: preview surfaces
        let loud = render_tool_end("bash", &r, 50, true).unwrap();
        assert!(loud.contains("did a thing"));
    }

    #[test]
    fn scratchpad_end_renders_checklist_even_non_verbose() {
        let r = AgentTraceToolResult::Success {
            output: json!({"content": "- [x] step one\n- [ ] step two"}),
        };
        let out = render_tool_end("scratchpad", &r, 10, false).unwrap();
        assert!(out.contains("step one"));
        assert!(out.contains("step two"));
    }

    #[test]
    fn completion_banner_styled_in_block() {
        let block = render_scratchpad_block("✅ 目标达成：x\n- [x] a");
        assert!(block.contains("目标达成"));
    }

    #[test]
    fn redact_masks_obvious_keys() {
        let s = "token sk-abcdefghijklmnop here";
        let r = redact(s);
        assert!(!r.contains("sk-abcdefghijklmnop"));
        assert!(r.contains("«redacted»"));
        assert!(r.contains("here"));
    }

    #[test]
    fn footer_renders_terminate_and_stats() {
        let mut s = RunSummary {
            tool_calls: 4,
            loops: 7,
            total_tokens: 12_345,
            ..Default::default()
        };
        s.duration_ms = Some(8100);
        s.terminate_reason = Some("completed".into());
        let footer = render_summary_footer(&s);
        assert!(footer.contains("4 tools"));
        assert!(footer.contains("7 loops"));
        assert!(footer.contains("12.3k tokens"));
    }

    #[test]
    fn footer_warns_on_verifier_veto() {
        let s = RunSummary {
            terminate_reason: Some("verifier_veto".into()),
            ..Default::default()
        };
        let footer = render_summary_footer(&s);
        assert!(footer.contains("目标未达成"));
    }

    #[test]
    fn retry_notice_carries_provider_and_attempt() {
        let line = render_retry_notice("anthropic", 2, 12, "connect timeout\nafter 10s");
        assert!(line.contains("anthropic"));
        assert!(line.contains("(2/12)"));
        // Newlines in the reason are flattened onto the single status line.
        assert!(!line.contains('\n'));
        assert!(line.contains("connect timeout"));
    }

    #[test]
    fn fallback_notice_shows_requested_and_actual_model() {
        let line = render_fallback_notice("haiku-4-5", "anthropic", Some("fable-5"));
        assert!(line.contains("fable-5"));
        assert!(line.contains("haiku-4-5"));
        assert!(line.contains("anthropic"));
        // When the original is unknown (or identical), only the resolved
        // model is shown — no dangling arrow.
        let bare = render_fallback_notice("haiku-4-5", "anthropic", None);
        assert!(bare.contains("haiku-4-5"));
        assert!(!bare.contains("→"));
    }

    #[test]
    fn reasoning_skips_empty() {
        assert!(render_reasoning("   ").is_none());
        assert!(render_reasoning("thinking...").is_some());
    }

    #[test]
    fn final_text_prefers_streamed_when_present() {
        // Streamed text wins even when a fallback exists (it was shown inline).
        assert_eq!(
            resolve_final_text("streamed answer", Some("summary answer")),
            "streamed answer"
        );
        assert_eq!(
            resolve_final_text("streamed answer", None),
            "streamed answer"
        );
    }

    #[test]
    fn final_text_falls_back_when_stream_empty() {
        // The black-hole case: nothing streamed, but the summary carries the
        // authoritative final answer.
        assert_eq!(
            resolve_final_text("", Some("summary answer")),
            "summary answer"
        );
        assert_eq!(
            resolve_final_text("   \n ", Some("summary answer")),
            "summary answer"
        );
    }

    #[test]
    fn final_text_empty_when_nothing_available() {
        // No stream, no fallback → preserve the (empty) streamed value rather
        // than inventing content.
        assert_eq!(resolve_final_text("", None), "");
        assert_eq!(resolve_final_text("", Some("  ")), "");
    }
}
