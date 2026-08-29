//! Tool output compression for context-heavy tools.
//!
//! Compresses outputs from Chrome `DevTools` MCP tools before they enter
//! conversation history. Each tool type gets a tailored strategy that
//! preserves actionable information while drastically reducing token count.

use serde_json::Value;
use tracing::debug;

use super::fence::rewrite_interior;
use super::scale_to_budget;
use super::walk::{walk_text_fields, MAX_WALK_DEPTH};

/// Known Chrome `DevTools` Protocol tool names.
const DEVTOOLS_TOOLS: &[&str] = &[
    "take_snapshot",
    "take_screenshot",
    "navigate_page",
    "click",
    "evaluate_script",
    "list_network_requests",
    "list_console_messages",
    "get_network_request",
    "get_console_message",
    "new_page",
    "close_page",
    "select_page",
    "list_pages",
    "hover",
    "fill",
    "fill_form",
    "type_text",
    "press_key",
    "drag",
    "upload_file",
    "handle_dialog",
    "wait_for",
    "emulate",
    "resize_page",
    "performance_start_trace",
    "performance_stop_trace",
    "performance_analyze_insight",
    "take_memory_snapshot",
    "lighthouse_audit",
];

/// Max chars per retained snapshot line — prevents a single long text node
/// from blowing the token budget.
const MAX_SNAPSHOT_LINE_CHARS: usize = 500;

/// Interactive accessibility roles to keep during snapshot compression.
const INTERACTIVE_ROLES: &[&str] = &[
    "link",
    "button",
    "textbox",
    "input",
    "textarea",
    "select",
    "checkbox",
    "radio",
    "combobox",
    "menuitem",
    "tab",
    "switch",
    "searchbox",
    "spinbutton",
    "slider",
];

/// The bare `DevTools` tool name behind `name`, or `None` when this isn't one.
///
/// **MCP tools register server-qualified as `{server}__{tool}`** (see
/// `mcp_adapter`), so matching the bare names in [`DEVTOOLS_TOOLS`] against the
/// registered name never succeeded — the entire compressor was unreachable in
/// production, silently, for every one of these tools. Stripping the prefix is
/// the whole fix.
///
/// It also widens the match from "the Chrome DevTools MCP" to "any MCP server
/// exposing these names", which is correct: all seven are browser-automation
/// verbs, and the compression each one gets (base64 → marker, interactive-node
/// filtering, entry caps) follows from the *shape* of that output, not from which
/// server produced it.
fn devtools_tool_name(name: &str) -> Option<&str> {
    let bare = name.rsplit("__").next().unwrap_or(name);
    DEVTOOLS_TOOLS.contains(&bare).then_some(bare)
}

/// Floors for the budget-scaled caps below. Below these the compression stops
/// being a summary and becomes a rumour: a byte cap under 1 KB cannot hold even
/// the truncation notice plus a usable head, five request lines cannot show the
/// one that failed, and ten console lines lose the burst the model needs to see.
/// The defaults (8/10 KB, 30 requests, 50 lines) are unchanged at the default
/// result budget — [`scale_to_budget`] guarantees it — and a *larger* budget
/// never raises them, because a digest orients, it does not reproduce the
/// output.
const MIN_GENERIC_CAP_BYTES: usize = 1024;
const MIN_NETWORK_ENTRIES: usize = 5;
const MIN_CONSOLE_LINES: usize = 10;

/// Compress a `DevTools` tool result **field by field**, in place. Returns
/// whether any field was actually rewritten — the caller uses that to decide
/// the result is now lossy and the pre-compression original must be persisted
/// (see [`crate::tool_output::ingress`]).
///
/// This is the production entry point; `compress_tool_output` (test-only) is
/// the per-string strategy dispatch kept for the strategy tests. The distinction
/// exists because every strategy here reads either lines
/// ([`compress_snapshot`], [`compress_console_messages`]) or a bare JSON array
/// ([`compress_network_requests`]) — i.e. the payload a server actually sent,
/// not a serialized envelope around it. Handed the latter, `compress_snapshot`
/// sees three lines, matches no interactive role, and falls into its
/// "structural summary" arm, where [`cap_line`] silently amputates the one line
/// that holds the whole snapshot at 500 chars; `compress_network_requests`
/// fails to parse and degrades to a blind head cut. Walking the value and
/// compressing each text field is what feeds the strategies the shape they
/// were written for.
///
/// A field that is a fenced untrusted payload is compressed **inside the
/// fence** (via [`rewrite_interior`]): the boundary markers are structure, not
/// content, and the MCP adapter puts them around exactly the large text blocks
/// this compressor exists for. A strategy returning its input unchanged
/// (passthrough) does not count as a rewrite.
///
/// The walker is shared with hygiene ([`walk_text_fields`]): both stages edit
/// the same value and must see the same field set. `budget_tokens` scales the
/// size knobs down through [`scale_to_budget`]; `None` keeps the shipped
/// defaults byte-for-byte.
pub(crate) fn compress_result_value(
    tool_name: &str,
    value: &mut Value,
    budget_tokens: Option<usize>,
) -> bool {
    let Some(tool_name) = devtools_tool_name(tool_name) else {
        // The overwhelmingly common path: not a DevTools tool, nothing to do.
        return false;
    };
    let mut changed = false;
    walk_text_fields(value, MAX_WALK_DEPTH, &mut |_path, field| {
        let rewritten = rewrite_interior(field, |payload| {
            let compressed = compress_with_budget(tool_name, payload, budget_tokens);
            (compressed != payload).then_some(compressed)
        });
        if let Some(new) = rewritten {
            *field = new;
            changed = true;
        }
    });
    changed
}

/// Per-string strategy dispatch, sized for `budget_tokens`. `None` reproduces
/// the historical fixed caps exactly, so the default call is byte-for-byte what
/// it has always been.
fn compress_with_budget(tool_name: &str, output: &str, budget_tokens: Option<usize>) -> String {
    // Every byte cap below is one knob family: at `Some(budget)` it scales
    // linearly with the budget (never below `MIN_GENERIC_CAP_BYTES`, never
    // above its default), at `None` it is the shipped constant.
    let byte_cap = |default: usize| {
        budget_tokens.map_or(default, |b| {
            scale_to_budget(default, MIN_GENERIC_CAP_BYTES, b)
        })
    };
    match tool_name {
        // `compress_snapshot` deliberately has no output cap: it keeps every
        // interactive node, because a snapshot the model cannot click is worse
        // than a large one. Overshoot is bounded downstream by the result
        // budget's persist path, which is the right layer for it.
        "take_snapshot" => compress_snapshot(output),
        "take_screenshot" => compress_screenshot(output),
        "evaluate_script" => compress_generic(output, byte_cap(8 * 1024)),
        "list_network_requests" => compress_network_requests(output, budget_tokens),
        "list_console_messages" => compress_console_messages(output, budget_tokens),
        "get_network_request" => compress_generic(output, byte_cap(8 * 1024)),
        "get_console_message" => compress_generic(output, byte_cap(8 * 1024)),
        _ => compress_generic(output, byte_cap(10 * 1024)),
    }
}

/// Compress a `DevTools` tool output using a type-specific strategy.
///
/// Non-DevTools tools are returned unchanged. Each `DevTools` tool gets a
/// tailored compression that preserves actionable information while
/// drastically reducing token count.
///
/// Kept as the per-string entry point for the strategy tests; production goes
/// through [`compress_result_value`], which feeds the strategies one text
/// field at a time. See that function for why the envelope is the wrong input.
#[cfg(test)]
pub(crate) fn compress_tool_output(tool_name: &str, output: &str) -> String {
    let Some(tool_name) = devtools_tool_name(tool_name) else {
        return output.to_owned();
    };
    compress_with_budget(tool_name, output, None)
}

/// Replace screenshot output (typically base64) with a short confirmation.
///
/// If the output looks like base64 data, replaces it entirely.
/// Otherwise keeps the first few lines as metadata.
fn compress_screenshot(output: &str) -> String {
    // Pure base64 or data URI — discard entirely.
    // We require at least one base64-specific character (+, /, =) in the
    // prefix, or a trailing '=' padding, to avoid treating long alphanumeric
    // text (e.g. logs, hex dumps) as base64.
    let prefix_is_base64_chars = || {
        output
            .bytes()
            .take(128)
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
    };
    let prefix_has_base64_marker = || {
        output
            .bytes()
            .take(128)
            .any(|b| b == b'+' || b == b'/' || b == b'=')
    };
    let has_base64_padding = || output.ends_with('=');

    // Case-insensitive data-URL detection: RFC 2397 makes mediatype case-
    // insensitive, and a server that emits `DATA:image/png;base64,...` would
    // otherwise fall through to the base64 prefix check and lose the
    // `[Screenshot captured successfully]` placeholder.
    let starts_with_data_image = output
        .get(..10)
        .is_some_and(|p| p.eq_ignore_ascii_case("data:image"));
    if starts_with_data_image
        || (output.len() > 100
            && prefix_is_base64_chars()
            && (prefix_has_base64_marker() || has_base64_padding()))
    {
        return "[Screenshot captured successfully]".to_owned();
    }
    // May contain metadata lines before or instead of base64 — keep first 5 lines
    let lines: Vec<&str> = output.lines().collect();
    let total = lines.len();
    let kept = &lines[..total.min(5)];
    let mut result = kept.join("\n");
    if total > 5 {
        let remaining = total - 5;
        let suffix = if remaining == 1 { "line" } else { "lines" };
        result.push_str(&format!("\n[... {remaining} more {suffix} omitted]"));
    }
    result
}

/// The role token of one snapshot line: leading indentation, list dashes and a
/// node handle removed.
///
/// **Two grammars reach this compressor and only one of them was ever matched.**
/// Playwright-style aria trees put the role first (`- button "Save"`), which is
/// what the original matcher assumed. chrome-devtools-mcp — the server
/// [`DEVTOOLS_TOOLS`] is actually named after — prefixes every node with its
/// handle instead:
///
/// ```text
///   uid=1_4 button "Apply 0"
///   uid=1_5 textbox "filter 0"
/// ```
///
/// So a real `take_snapshot` never produced a single interactive line, every
/// snapshot fell into the structural-summary arm below, and the model was handed
/// 20 of 1103 nodes with none of the controls it needs a snapshot *for*. That
/// went unseen until the ingress pass started feeding this function real lines
/// (see [`crate::tool_output::ingress`]); before that it only ever saw a
/// serialized envelope, where every grammar fails alike.
fn role_token(line: &str) -> &str {
    let trimmed = line.trim_start().trim_start_matches('-').trim_start();
    trimmed
        .strip_prefix("uid=")
        .and_then(|rest| rest.split_once(' '))
        .map_or(trimmed, |(_, after)| after.trim_start())
}

/// Compress accessibility snapshot by keeping only interactive elements.
///
/// Lines are considered interactive if their [`role_token`] starts with a known
/// interactive role name followed by a space or quote character.
fn compress_snapshot(output: &str) -> String {
    // Small snapshots pass through unchanged
    if output.len() < 4 * 1024 {
        return output.to_owned();
    }

    let lines: Vec<&str> = output.lines().collect();
    let total_nodes = lines.len();
    let mut kept: Vec<String> = Vec::new();

    for line in &lines {
        let trimmed = role_token(line);
        let is_interactive = INTERACTIVE_ROLES.iter().any(|role| {
            trimmed.starts_with(role)
                && (trimmed.len() == role.len()
                    || trimmed
                        .as_bytes()
                        .get(role.len())
                        .is_some_and(|&b| b == b' ' || b == b'"'))
        });
        if is_interactive {
            let capped = cap_line(line);
            kept.push(capped);
        }
    }

    if kept.is_empty() {
        // No interactive elements found — keep first 20 lines as structural summary
        // rather than a raw byte truncation so element types are still visible.
        let summary_lines = lines.len().min(20);
        let capped: Vec<String> = lines[..summary_lines].iter().map(|l| cap_line(l)).collect();
        let mut result = capped.join("\n");
        if lines.len() > summary_lines {
            result.push_str(&format!(
                "\n[Snapshot compressed: no interactive elements; kept first {} of {} lines]",
                summary_lines,
                lines.len(),
            ));
        }
        return result;
    }

    let kept_count = kept.len();
    let mut result = kept.join("\n");
    let nodes_word = if total_nodes == 1 { "node" } else { "nodes" };
    result.push_str(&format!(
        "\n[Snapshot compressed: kept {kept_count} interactive elements out of {total_nodes} total {nodes_word}]",
    ));
    result
}

/// Compress network request listing by keeping at most 30 summary lines
/// (scaled down — never up — when the caller has a token budget; see
/// [`compress_with_budget`]).
fn compress_network_requests(output: &str, budget_tokens: Option<usize>) -> String {
    let parsed: Result<Vec<Value>, _> = serde_json::from_str(output);
    let entries = match parsed {
        Ok(v) => v,
        Err(e) => {
            debug!("Failed to parse network requests as JSON, falling back to generic compression: {e}");
            let cap = budget_tokens.map_or(3 * 1024, |b| {
                scale_to_budget(3 * 1024, MIN_GENERIC_CAP_BYTES, b)
            });
            return compress_generic(output, cap);
        }
    };

    let total = entries.len();
    if total == 0 {
        return "[No network requests captured]".to_owned();
    }
    let limit = budget_tokens.map_or(30, |b| scale_to_budget(30, MIN_NETWORK_ENTRIES, b));
    let mut lines: Vec<String> = Vec::with_capacity(limit.min(total));

    for entry in entries.iter().take(limit) {
        let method = entry
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("GET");
        let url = entry
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let status = entry
            .get("status")
            .and_then(|v| {
                v.as_u64()
                    .map(|s| s.to_string())
                    .or_else(|| v.as_str().map(|s| s.to_string()))
            })
            .unwrap_or_else(|| "pending".to_owned());
        lines.push(format!("{method} {url} → {status}"));
    }

    let mut result = lines.join("\n");
    if total > limit {
        result.push_str(&format!(
            "\n[... showing {limit} of {total} total requests]"
        ));
    }
    result
}

/// Compress console messages by keeping only the last 50 lines (scaled down —
/// never up — under a token budget; the tail is kept because a console flood's
/// signal is what the page logged *last*).
fn compress_console_messages(output: &str, budget_tokens: Option<usize>) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let total = lines.len();
    let limit = budget_tokens.map_or(50, |b| scale_to_budget(50, MIN_CONSOLE_LINES, b));

    if total <= limit {
        return output.to_owned();
    }

    let kept = &lines[total - limit..];
    format!(
        "[... showing last {} of {} total console messages]\n{}",
        limit,
        total,
        kept.join("\n"),
    )
}

/// Char-safe cap for a single snapshot line (never slice on a byte boundary).
fn cap_line(s: &str) -> String {
    // Byte-length fast path: each UTF-8 char is at least one byte, so
    // `s.len() <= MAX_SNAPSHOT_LINE_CHARS` implies the char count fits and we
    // can skip the O(n) `chars().count()` walk that this used to do for every
    // snapshot line (the overwhelmingly common short-line case).
    if s.len() <= MAX_SNAPSHOT_LINE_CHARS || s.chars().count() <= MAX_SNAPSHOT_LINE_CHARS {
        return s.to_string();
    }
    let kept: String = s.chars().take(MAX_SNAPSHOT_LINE_CHARS).collect();
    format!("{kept}…")
}

/// Generic truncation: keep at most `max_bytes` with a UTF-8-safe cut.
fn compress_generic(output: &str, max_bytes: usize) -> String {
    if output.len() <= max_bytes {
        return output.to_owned();
    }

    let total = output.len();
    // Walk back to a valid UTF-8 char boundary
    let mut end = max_bytes;
    while end > 0 && !output.is_char_boundary(end) {
        end -= 1;
    }

    let mut result = output[..end].to_owned();
    result.push_str(&format!(
        "\n[... output truncated, showing first {end} bytes of {total} total]",
    ));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- devtools_tool_name ---

    #[test]
    fn test_is_devtools_tool() {
        assert_eq!(devtools_tool_name("take_snapshot"), Some("take_snapshot"));
        assert_eq!(devtools_tool_name("click"), Some("click"));
        assert_eq!(
            devtools_tool_name("lighthouse_audit"),
            Some("lighthouse_audit")
        );
        assert_eq!(devtools_tool_name("bash"), None);
        assert_eq!(devtools_tool_name("web_search"), None);
        assert_eq!(devtools_tool_name(""), None);
    }

    /// The regression that made this whole module unreachable: MCP tools arrive
    /// server-qualified, so the bare-name table never matched a real call.
    #[test]
    fn server_qualified_mcp_names_are_recognized() {
        assert_eq!(
            devtools_tool_name("chrome_devtools__take_snapshot"),
            Some("take_snapshot")
        );
        assert_eq!(
            devtools_tool_name("chrome-devtools__list_console_messages"),
            Some("list_console_messages")
        );
        assert_eq!(devtools_tool_name("some_server__bash"), None);

        // …and the compression actually runs for a qualified name now.
        let base64 = format!("{}+/{}=", "iVBORw0KGgo".repeat(200), "A".repeat(100));
        let compressed = compress_tool_output("chrome_devtools__take_screenshot", &base64);
        assert!(
            compressed.len() < 200,
            "qualified screenshot must be compressed, got {} chars",
            compressed.len()
        );
    }

    // --- passthrough ---

    #[test]
    fn test_passthrough_non_devtools() {
        let output = "some output from bash command";
        assert_eq!(compress_tool_output("bash", output), output);
        assert_eq!(compress_tool_output("web_search", output), output);
    }

    #[test]
    fn test_passthrough_small_devtools_output() {
        let output = "Clicked element at (100, 200)";
        assert_eq!(compress_tool_output("click", output), output,);
    }

    // --- screenshot ---

    #[test]
    fn test_compress_screenshot_replaces_base64_data_uri() {
        let base64_data = format!("data:image/png;base64,{}", "A".repeat(50_000));
        let result = compress_tool_output("take_screenshot", &base64_data);
        assert_eq!(result, "[Screenshot captured successfully]");
    }

    #[test]
    fn test_compress_screenshot_replaces_raw_base64() {
        // Must include at least one base64-specific char (+, /, =) to be recognised
        let raw_b64 = format!("{}=", "A".repeat(50_000));
        let result = compress_tool_output("take_screenshot", &raw_b64);
        assert_eq!(result, "[Screenshot captured successfully]");
    }

    #[test]
    fn test_compress_screenshot_short_text_not_treated_as_base64() {
        // Short ASCII text should NOT be detected as base64
        let output = "OK";
        let result = compress_tool_output("take_screenshot", output);
        assert_eq!(result, "OK");
    }

    #[test]
    fn test_compress_screenshot_long_alphanumeric_not_base64() {
        // Long pure-alphanumeric text (e.g. hex dump, log) should NOT be
        // mistaken for base64 — it lacks +, /, or = and does not end with =.
        let output = "A".repeat(50_000);
        let result = compress_tool_output("take_screenshot", &output);
        // Must NOT be replaced with the base64 placeholder
        assert!(!result.contains("Screenshot captured successfully"));
        // Single-line input without newlines is preserved as-is (1 line <= 5)
        assert_eq!(result, output);
    }

    #[test]
    fn test_compress_screenshot_empty_string() {
        let result = compress_tool_output("take_screenshot", "");
        assert_eq!(result, "");
    }

    #[test]
    fn test_compress_screenshot_keeps_metadata() {
        let output =
            "width: 1920\nheight: 1080\nformat: png\nsome extra info\nmore info\nbase64 data here";
        let result = compress_tool_output("take_screenshot", output);
        assert!(result.contains("width: 1920"));
        assert!(result.contains("format: png"));
        assert!(result.contains("1 more line omitted"));
    }

    // --- snapshot ---

    #[test]
    fn test_compress_snapshot_filters_interactive() {
        // Build input > 4KB with many non-interactive nodes and a few interactive ones
        let mut lines: Vec<String> = Vec::new();
        for i in 0..200 {
            lines.push(format!("  paragraph \"Some text content line {}\"", i));
        }
        // Sprinkle in interactive elements
        lines.push("  button \"Submit\"".to_owned());
        lines.push("  link \"Home page\"".to_owned());
        lines.push("    textbox \"Email\"".to_owned());
        lines.push("  heading \"Title\"".to_owned());
        lines.push("  - checkbox \"Accept terms\"".to_owned());

        let input = lines.join("\n");
        // Ensure it's > 4KB
        assert!(input.len() > 4 * 1024, "Test input must be > 4KB");

        let result = compress_tool_output("take_snapshot", &input);

        // Interactive elements should be kept
        assert!(result.contains("button \"Submit\""));
        assert!(result.contains("link \"Home page\""));
        assert!(result.contains("textbox \"Email\""));
        assert!(result.contains("checkbox \"Accept terms\""));

        // Non-interactive elements should be stripped
        assert!(!result.contains("paragraph"));
        assert!(!result.contains("heading \"Title\""));

        // Summary should be present
        assert!(result.contains("Snapshot compressed: kept 4 interactive elements out of"));
    }

    /// The grammar chrome-devtools-mcp actually emits, transcribed from a live
    /// `take_snapshot` against a real page (2026-08-04 ingress QA). Every node
    /// carries its `uid=` handle before the role; matching only the
    /// Playwright-style role-first form put *every* real snapshot into the
    /// no-interactive-elements arm, where the model got 20 lines out of 1103 and
    /// not one of the controls.
    #[test]
    fn a_uid_prefixed_snapshot_still_finds_its_controls() {
        let mut lines: Vec<String> =
            vec!["uid=1_0 RootWebArea \"Ingress QA fixture\" url=\"http://x/\"".to_owned()];
        for i in 0..120 {
            lines.push(format!(
                "  uid=1_{} link \"Release note {i}\" url=\"http://x/#r{i}\"",
                i * 4 + 1
            ));
            lines.push(format!("  uid=1_{} button \"Apply {i}\"", i * 4 + 2));
            lines.push(format!("  uid=1_{} textbox \"filter {i}\"", i * 4 + 3));
            lines.push(format!(
                "  uid=1_{} StaticText \"Paragraph {i}: prose that is not a control.\"",
                i * 4 + 4
            ));
        }
        let input = lines.join("\n");
        assert!(input.len() > 4 * 1024);

        let result = compress_tool_output("take_snapshot", &input);

        assert!(result.contains("button \"Apply 7\""), "controls kept");
        assert!(result.contains("textbox \"filter 7\""));
        assert!(result.contains("link \"Release note 7\""));
        assert!(!result.contains("StaticText"), "prose dropped");
        assert!(
            result.contains("Snapshot compressed: kept 360 interactive elements out of 481"),
            "got tail: {}",
            &result[result.len().saturating_sub(120)..]
        );
    }

    /// A handle-looking token must not be mistaken for a role by itself: the
    /// role is what follows it, and a non-interactive node stays dropped.
    #[test]
    fn stripping_the_handle_does_not_promote_a_non_control() {
        assert_eq!(role_token("  uid=1_9 StaticText \"x\""), "StaticText \"x\"");
        assert_eq!(role_token("  - button \"Save\""), "button \"Save\"");
        assert_eq!(role_token("  uid=1_9"), "uid=1_9", "no role token to take");
        assert_eq!(role_token("plain"), "plain");
    }

    #[test]
    fn test_compress_snapshot_passthrough_small() {
        let output = "button \"OK\"\nlink \"Home\"";
        assert!(output.len() < 4 * 1024);
        assert_eq!(compress_tool_output("take_snapshot", output), output);
    }

    #[test]
    fn test_compress_snapshot_fallback_no_interactive() {
        // Large snapshot with no interactive elements should keep first 20 lines
        let lines: Vec<String> = (0..200)
            .map(|i| format!("  paragraph \"Content line {}\"", i))
            .collect();
        let input = lines.join("\n");
        assert!(input.len() > 4 * 1024);

        let result = compress_tool_output("take_snapshot", &input);
        assert!(result.contains("paragraph \"Content line 0\""));
        assert!(result.contains("paragraph \"Content line 19\""));
        assert!(!result.contains("paragraph \"Content line 20\""));
        assert!(result.contains("no interactive elements"));
    }

    #[test]
    fn test_compress_snapshot_fallback_grammar_exactly_21_lines() {
        let lines: Vec<String> = (0..21)
            .map(|i| format!("  paragraph \"Line {} {}\"", i, "x".repeat(200)))
            .collect();
        let input = lines.join("\n");
        assert!(input.len() > 4 * 1024);

        let result = compress_tool_output("take_snapshot", &input);
        assert!(result.contains("kept first 20 of 21 lines"));
        assert!(!result.contains("kept first 20 of 21 line\""));
    }

    #[test]
    fn test_compress_snapshot_single_node_summary_grammar() {
        let mut lines: Vec<String> = (0..200)
            .map(|i| format!("  paragraph \"Content line {}\"", i))
            .collect();
        lines.push("  button \"Submit\"".to_owned());
        let input = lines.join("\n");
        assert!(input.len() > 4 * 1024);

        let result = compress_tool_output("take_snapshot", &input);
        assert!(result.contains("kept 1 interactive elements out of 201 total nodes"));
    }

    // --- evaluate_script ---

    #[test]
    fn test_compress_evaluate_script_truncates_large() {
        let output = "x".repeat(20 * 1024);
        let result = compress_tool_output("evaluate_script", &output);
        // Should be truncated to ~8KB + notice
        assert!(result.len() < 9 * 1024);
        assert!(result.contains("output truncated"));
    }

    #[test]
    fn test_compress_evaluate_script_passthrough_small() {
        let output = "{\"value\": 42}";
        assert_eq!(compress_tool_output("evaluate_script", output), output);
    }

    // --- network requests ---

    #[test]
    fn test_compress_network_requests_limits_entries() {
        let entries: Vec<Value> = (0..100)
            .map(|i| {
                serde_json::json!({
                    "method": "GET",
                    "url": format!("https://example.com/api/{}", i),
                    "status": 200
                })
            })
            .collect();
        let input = serde_json::to_string(&entries).unwrap();

        let result = compress_tool_output("list_network_requests", &input);

        assert!(result.contains("showing 30 of 100"));
        assert!(result.contains("https://example.com/api/0"));
        assert!(result.contains("https://example.com/api/29"));
        assert!(!result.contains("https://example.com/api/50"));
    }

    #[test]
    fn test_compress_network_requests_empty_array() {
        let result = compress_tool_output("list_network_requests", "[]");
        assert_eq!(result, "[No network requests captured]");
    }

    #[test]
    fn test_compress_network_requests_string_status() {
        // Some DevTools implementations return status as a string.
        let entries = serde_json::json!([
            {"method": "GET", "url": "https://example.com/", "status": "200"},
            {"method": "POST", "url": "https://example.com/api", "status": "pending"}
        ]);
        let result = compress_tool_output("list_network_requests", &entries.to_string());
        assert!(result.contains("200"));
        assert!(result.contains("pending"));
    }

    // --- console messages ---

    #[test]
    fn test_compress_console_messages_keeps_last() {
        let lines: Vec<String> = (0..200).map(|i| format!("console line {}", i)).collect();
        let input = lines.join("\n");

        let result = compress_tool_output("list_console_messages", &input);

        assert!(result.contains("showing last 50 of 200"));
        // Old lines should be dropped
        assert!(!result.contains("console line 0"));
        assert!(!result.contains("console line 149"));
        // Recent lines should be present
        assert!(result.contains("console line 150"));
        assert!(result.contains("console line 199"));
    }

    // --- generic ---

    #[test]
    fn test_compress_generic_devtools_truncates() {
        let output = "y".repeat(20 * 1024);
        // "click" maps to compress_generic(_, 10*1024) for unknown-ish tools —
        // but click IS in the list and falls through to the _ arm.
        let result = compress_tool_output("click", &output);
        assert!(result.len() < 11 * 1024);
        assert!(result.contains("output truncated"));
    }

    #[test]
    fn test_compress_generic_utf8_boundary() {
        let input: String = (0..11_000)
            .map(|i| if i == 10_999 { "日" } else { "a" })
            .collect();
        assert_eq!(input.len(), 11_002);

        let result = compress_tool_output("click", &input);

        assert!(result.is_char_boundary(result.len()));
    }

    // --- compress_result_value: the field-level entry point ---

    /// The regression this whole pass fixes: the compressor used to be fed the
    /// flattened envelope — one escaped line — so a snapshot fell into the
    /// structural-summary arm and `cap_line` amputated it at 500 chars. Walked
    /// field-wise, the interactive-node strategy sees real lines.
    #[test]
    fn compress_result_value_compresses_the_field_not_the_envelope() {
        let mut lines: Vec<String> = Vec::new();
        for i in 0..200 {
            lines.push(format!("  paragraph \"Some text content line {i}\""));
        }
        lines.push("  button \"Submit late\"".to_owned());
        let snapshot = lines.join("\n");
        assert!(snapshot.len() > 4 * 1024);

        let mut value = serde_json::json!({ "content": [ { "type": "text", "text": snapshot } ] });
        let changed = compress_result_value("chrome_devtools__take_snapshot", &mut value, None);
        assert!(changed, "a 200-line snapshot must compress");

        let text = value["content"][0]["text"]
            .as_str()
            .expect("stays a string");
        assert!(
            text.contains("button \"Submit late\""),
            "the interactive node must survive; got tail: {}",
            &text[text.len().saturating_sub(160)..]
        );
        assert!(
            text.contains("Snapshot compressed: kept 1 interactive"),
            "the interactive-node arm must run, not the summary arm: {text}"
        );
        assert!(
            !text.contains("\"content\""),
            "the old bug compressed the JSON envelope; this must be the snapshot: {text}"
        );
        // The sibling metadata field is not a snapshot; it must pass through.
        assert_eq!(value["content"][0]["type"], "text");
    }

    /// A fenced text field is compressed inside its boundary markers — the
    /// fence is structure, not content.
    #[test]
    fn compress_result_value_preserves_a_fields_fence() {
        use crate::security::content_sanitizer::{wrap_external_content, ContentSource};

        let mut lines: Vec<String> = Vec::new();
        for i in 0..200 {
            lines.push(format!("  paragraph \"filler line {i}\""));
        }
        lines.push("  link \"Home page\"".to_owned());
        let fenced = wrap_external_content(&lines.join("\n"), ContentSource::BrowserContent);

        let mut value = serde_json::json!({ "text": fenced });
        assert!(compress_result_value("take_snapshot", &mut value, None));

        let text = value["text"].as_str().expect("stays a string");
        let split = crate::security::content_sanitizer::split_external_fence(text)
            .expect("the fence must survive compression intact");
        assert!(
            split.interior.contains("link \"Home page\""),
            "the interior is what got compressed: {}",
            split.interior
        );
    }

    #[test]
    fn compress_result_value_reports_no_change_for_passthrough() {
        // Small output: every strategy declines, so the value is untouched and
        // the caller learns nothing was lost (no persist needed).
        let mut value = serde_json::json!({ "text": "Clicked element at (100, 200)" });
        let before = value.clone();
        assert!(!compress_result_value("click", &mut value, None));
        assert_eq!(value, before, "passthrough must be byte-identical");

        // A non-DevTools tool is the zero-cost common path.
        let mut value = serde_json::json!({ "stdout": "x".repeat(64 * 1024) });
        let before = value.clone();
        assert!(!compress_result_value("bash", &mut value, Some(8_000)));
        assert_eq!(value, before);
    }

    /// The size knobs used to be constants, so a tool declaring a small budget
    /// and one declaring the default got byte-identical compression. At
    /// `None` the defaults hold exactly; at a small budget they tighten but
    /// never fall below their floors.
    #[test]
    fn a_small_budget_tightens_the_compression_knobs() {
        // Generic byte cap (evaluate_script's 8 KB).
        let output = "x".repeat(20 * 1024);
        let wide = compress_with_budget("evaluate_script", &output, Some(8_000));
        let tight = compress_with_budget("evaluate_script", &output, Some(400));
        assert!(
            tight.len() < wide.len(),
            "tight {} vs wide {}",
            tight.len(),
            wide.len()
        );
        assert!(
            tight.len() >= MIN_GENERIC_CAP_BYTES,
            "the floor still holds: {}",
            tight.len()
        );
        // …and the default budget reproduces the shipped 8 KB cap.
        let default_budget = compress_tool_output("evaluate_script", &output);
        assert_eq!(wide.len(), default_budget.len());

        // Console lines (50 → fewer, floor 10).
        let lines: Vec<String> = (0..200).map(|i| format!("console line {i}")).collect();
        let output = lines.join("\n");
        let wide = compress_with_budget("list_console_messages", &output, Some(8_000));
        let tight = compress_with_budget("list_console_messages", &output, Some(400));
        assert!(wide.contains("showing last 50 of 200"));
        assert!(!tight.contains("showing last 50 of 200"));
        let kept = tight
            .lines()
            .filter(|l| l.starts_with("console line"))
            .count();
        assert!(kept >= MIN_CONSOLE_LINES, "floor: kept {kept}");
        assert!(kept < 50, "tightened: kept {kept}");

        // Network entries (30 → fewer, floor 5).
        let entries: Vec<Value> = (0..100)
            .map(|i| {
                serde_json::json!({
                    "method": "GET",
                    "url": format!("https://example.com/api/{i}"),
                    "status": 200
                })
            })
            .collect();
        let output = serde_json::to_string(&entries).unwrap();
        let wide = compress_with_budget("list_network_requests", &output, Some(8_000));
        let tight = compress_with_budget("list_network_requests", &output, Some(400));
        assert!(wide.contains("showing 30 of 100"));
        let shown = tight.lines().filter(|l| l.contains("→")).count();
        assert!(shown >= MIN_NETWORK_ENTRIES, "floor: shown {shown}");
        assert!(shown < 30, "tightened: shown {shown}");
    }
}
