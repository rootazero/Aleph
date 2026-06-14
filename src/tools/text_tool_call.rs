//! Plain-text tool-call promotion (openclaw `tool-call-repair` parity).
//!
//! Some models — particularly open-weight families behind OpenAI-compatible
//! proxies (Qwen, Llama-3.1, `DeepSeek`, Mistral, Hermes-format finetunes) — emit
//! a tool call as **plain assistant text** instead of a provider-native
//! function-call block. The harness then sees `tool_calls == []`, treats the
//! turn as a clean completion, and the agent loop stalls: the model "talked
//! about" calling a tool but never actually did.
//!
//! This module recognizes the two dominant text encodings and rewrites them
//! into structured [`PromotedToolCall`]s so the harness can dispatch them like
//! any native call. It is intentionally conservative — promotion is
//! **all-or-nothing** and gated on tool-name resolution against the set of
//! tools actually offered this turn, so it cannot misfire on prose that merely
//! mentions a tool.
//!
//! Supported encodings:
//!
//! 1. **`<tool_call>{json}</tool_call>`** — Qwen / Hermes-format / `DeepSeek` /
//!    Mistral. The JSON body carries `name` plus `arguments` (aliases:
//!    `parameters`, `args`). `arguments` may itself be a JSON-encoded string
//!    (some finetunes double-encode); that is decoded transparently.
//! 2. **`<function=NAME>…<parameter=KEY>VALUE</parameter>…</function>`** —
//!    Llama-3.1 / Qwen XML tool syntax. Each parameter value is coerced to JSON
//!    when it parses as such (numbers, bools, objects, arrays), otherwise kept
//!    as a string.
//!
//! Deferred (documented in the gap analysis, not yet ported from openclaw's
//! `grammar.ts`): the GPT-OSS *Harmony* channel form
//! (`<|channel|>commentary to=NAME …<|message|>{json}<|call|>`) and the legacy
//! bracketed `[tool_name]\n{json}[END_TOOL_REQUEST]` form. They are rarer and
//! carry higher false-positive risk; add them behind the same resolver gate if
//! a real model surfaces them.

use serde_json::{Map, Value};
use std::collections::HashSet;

/// One tool call recovered from assistant text, with its name already resolved
/// to the canonical form offered to the provider this turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotedToolCall {
    pub name: String,
    pub arguments: Value,
}

/// Outcome of a successful promotion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Promotion {
    /// The recovered calls, in source order.
    pub calls: Vec<PromotedToolCall>,
    /// The assistant text with the promoted blocks removed and trimmed. Empty
    /// when the message was nothing but tool-call markup.
    pub residual_text: String,
}

/// A matched block: byte span in the source plus the call it yielded.
struct Match {
    start: usize,
    end: usize,
    call: PromotedToolCall,
}

/// Attempt to promote plain-text tool-call blocks in `text` into structured
/// calls, resolving each emitted name against `allowed_names`.
///
/// Returns `None` (no promotion — leave the text untouched) when no recognized
/// block is present, or when **any** recognized block names a tool that does
/// not resolve to an offered tool. The all-or-nothing rule mirrors openclaw's
/// resolver: a partial promotion would silently drop calls the model intended.
#[must_use]
pub fn promote_plain_text_tool_calls(
    text: &str,
    allowed_names: &HashSet<&str>,
) -> Option<Promotion> {
    if text.is_empty() {
        return None;
    }

    // A model emits one encoding per turn. Try each extractor; the first that
    // produces at least one block wins, so we never interleave formats.
    let matches = extract_tool_call_tags(text).or_else(|| extract_function_xml(text))?;

    // Resolve every name up front. Any miss aborts the whole promotion.
    let mut calls = Vec::with_capacity(matches.len());
    for m in &matches {
        let resolved = resolve_name(&m.call.name, allowed_names)?;
        calls.push(PromotedToolCall {
            name: resolved,
            arguments: m.call.arguments.clone(),
        });
    }

    Some(Promotion {
        calls,
        residual_text: strip_spans(text, &matches),
    })
}

/// Resolve a model-emitted tool name to an offered one. Delegates to the unified
/// [`crate::tools::name_repair`] resolver — the single source of truth shared
/// with the native dispatch path — so the plain-text promotion path gets the
/// same case / separator (`file.read` ↔ `file_read`) / bounded-typo tolerance.
/// Returns the canonical offered name, or `None` when nothing resolves safely
/// (which aborts the whole all-or-nothing promotion).
fn resolve_name(raw: &str, allowed: &HashSet<&str>) -> Option<String> {
    let offered: Vec<&str> = allowed.iter().copied().collect();
    crate::tools::name_repair::repair_tool_name(raw, &offered).map(|r| r.name)
}

/// Rebuild the source text with `matches` spans removed, then trim. `matches`
/// is in ascending, non-overlapping span order (both extractors guarantee it).
fn strip_spans(text: &str, matches: &[Match]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    for m in matches {
        if m.start >= cursor {
            out.push_str(&text[cursor..m.start]);
            cursor = m.end;
        }
    }
    out.push_str(&text[cursor..]);
    out.trim().to_string()
}

// ---------------------------------------------------------------------------
// Encoding 1: <tool_call>{json}</tool_call>
// ---------------------------------------------------------------------------

const TOOL_CALL_OPEN: &str = "<tool_call>";
const TOOL_CALL_CLOSE: &str = "</tool_call>";

/// Extract every `<tool_call>…</tool_call>` block whose body is a JSON object
/// carrying a tool name. Returns `None` when no well-formed block exists.
fn extract_tool_call_tags(text: &str) -> Option<Vec<Match>> {
    let mut matches = Vec::new();
    let mut search = 0usize;
    while let Some(rel_open) = find_ci(text, TOOL_CALL_OPEN, search) {
        let body_start = rel_open + TOOL_CALL_OPEN.len();
        let Some(rel_close) = find_ci(text, TOOL_CALL_CLOSE, body_start) else {
            break;
        };
        let body = &text[body_start..rel_close];
        let end = rel_close + TOOL_CALL_CLOSE.len();
        if let Some(call) = parse_json_tool_call(body) {
            matches.push(Match {
                start: rel_open,
                end,
                call,
            });
        }
        search = end;
    }
    (!matches.is_empty()).then_some(matches)
}

/// Parse a `{ "name": …, "arguments": … }` object (with `parameters`/`args`
/// aliases) into a call. `arguments` may be an object or a JSON-encoded string.
fn parse_json_tool_call(body: &str) -> Option<PromotedToolCall> {
    let value: Value = serde_json::from_str(body.trim()).ok()?;
    let obj = value.as_object()?;
    let name = obj
        .get("name")
        .or_else(|| obj.get("tool"))
        .and_then(Value::as_str)?
        .trim()
        .to_string();
    if name.is_empty() {
        return None;
    }
    let arguments = obj
        .get("arguments")
        .or_else(|| obj.get("parameters"))
        .or_else(|| obj.get("args"))
        .cloned()
        .map_or_else(|| Value::Object(Map::new()), coerce_arguments);
    Some(PromotedToolCall { name, arguments })
}

/// Decode `arguments` when a model double-encodes it as a JSON string; pass
/// objects through unchanged. A bare non-JSON string is kept as-is.
fn coerce_arguments(value: Value) -> Value {
    match value {
        Value::String(s) => serde_json::from_str(&s).unwrap_or(Value::String(s)),
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Encoding 2: <function=NAME><parameter=KEY>VALUE</parameter>…</function>
// ---------------------------------------------------------------------------

const FUNCTION_OPEN: &str = "<function=";
const FUNCTION_CLOSE: &str = "</function>";
const PARAM_OPEN: &str = "<parameter=";
const PARAM_CLOSE: &str = "</parameter>";

/// Extract every `<function=NAME>…</function>` block, reading `<parameter=K>V`
/// children into an arguments object.
fn extract_function_xml(text: &str) -> Option<Vec<Match>> {
    let mut matches = Vec::new();
    let mut search = 0usize;
    while let Some(rel_open) = find_ci(text, FUNCTION_OPEN, search) {
        let name_start = rel_open + FUNCTION_OPEN.len();
        // Name runs until the closing '>' of the opening tag.
        let Some(rel_gt) = text[name_start..].find('>') else {
            break;
        };
        let name = text[name_start..name_start + rel_gt].trim().to_string();
        let inner_start = name_start + rel_gt + 1;
        let Some(rel_close) = find_ci(text, FUNCTION_CLOSE, inner_start) else {
            break;
        };
        let inner = &text[inner_start..rel_close];
        let end = rel_close + FUNCTION_CLOSE.len();
        if !name.is_empty() {
            matches.push(Match {
                start: rel_open,
                end,
                call: PromotedToolCall {
                    name,
                    arguments: parse_xml_parameters(inner),
                },
            });
        }
        search = end;
    }
    (!matches.is_empty()).then_some(matches)
}

/// Read `<parameter=KEY>VALUE</parameter>` children into a JSON object, coercing
/// each VALUE to JSON when it parses (number/bool/object/array), else string.
fn parse_xml_parameters(inner: &str) -> Value {
    let mut map = Map::new();
    let mut search = 0usize;
    while let Some(rel_open) = find_ci(inner, PARAM_OPEN, search) {
        let key_start = rel_open + PARAM_OPEN.len();
        let Some(rel_gt) = inner[key_start..].find('>') else {
            break;
        };
        let key = inner[key_start..key_start + rel_gt].trim().to_string();
        let val_start = key_start + rel_gt + 1;
        let Some(rel_close) = find_ci(inner, PARAM_CLOSE, val_start) else {
            break;
        };
        let raw = inner[val_start..rel_close].trim();
        let value =
            serde_json::from_str::<Value>(raw).unwrap_or_else(|_| Value::String(raw.to_string()));
        if !key.is_empty() {
            map.insert(key, value);
        }
        search = rel_close + PARAM_CLOSE.len();
    }
    Value::Object(map)
}

// ---------------------------------------------------------------------------
// Shared
// ---------------------------------------------------------------------------

/// Case-insensitive ASCII search for `needle` in `haystack` starting at byte
/// offset `from`. Returns the byte offset of the match. Tags emitted by models
/// vary in case (`<Tool_Call>`), so matching must be case-insensitive while
/// still returning byte offsets valid for slicing the original UTF-8 string.
fn find_ci(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    if needle.is_empty() || from > haystack.len() {
        return None;
    }
    let hay = haystack.as_bytes();
    let need = needle.as_bytes();
    if need.len() > hay.len() {
        return None;
    }
    let last = hay.len() - need.len();
    (from..=last).find(|&i| {
        hay[i..i + need.len()]
            .iter()
            .zip(need)
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn allowed<'a>(names: &[&'a str]) -> HashSet<&'a str> {
        names.iter().copied().collect()
    }

    #[test]
    fn tool_call_json_block_promotes() {
        let text = r#"Sure, let me check.
<tool_call>
{"name": "get_weather", "arguments": {"city": "Paris"}}
</tool_call>"#;
        let p = promote_plain_text_tool_calls(text, &allowed(&["get_weather"])).unwrap();
        assert_eq!(p.calls.len(), 1);
        assert_eq!(p.calls[0].name, "get_weather");
        assert_eq!(p.calls[0].arguments, json!({"city": "Paris"}));
        assert_eq!(p.residual_text, "Sure, let me check.");
    }

    #[test]
    fn multiple_tool_call_blocks_promote_in_order() {
        let text = "<tool_call>{\"name\":\"a\",\"arguments\":{}}</tool_call>\
                    <tool_call>{\"name\":\"b\",\"arguments\":{\"x\":1}}</tool_call>";
        let p = promote_plain_text_tool_calls(text, &allowed(&["a", "b"])).unwrap();
        assert_eq!(p.calls.len(), 2);
        assert_eq!(p.calls[0].name, "a");
        assert_eq!(p.calls[1].name, "b");
        assert_eq!(p.calls[1].arguments, json!({"x": 1}));
        assert_eq!(p.residual_text, "");
    }

    #[test]
    fn parameters_alias_and_double_encoded_arguments() {
        // `parameters` alias + arguments double-encoded as a JSON string.
        let text = r#"<tool_call>{"name":"search","parameters":"{\"q\":\"rust\"}"}</tool_call>"#;
        let p = promote_plain_text_tool_calls(text, &allowed(&["search"])).unwrap();
        assert_eq!(p.calls[0].arguments, json!({"q": "rust"}));
    }

    #[test]
    fn function_xml_block_promotes_with_value_coercion() {
        let text = "<function=run_query><parameter=sql>SELECT 1</parameter>\
                    <parameter=limit>10</parameter><parameter=dry>true</parameter></function>";
        let p = promote_plain_text_tool_calls(text, &allowed(&["run_query"])).unwrap();
        assert_eq!(p.calls.len(), 1);
        assert_eq!(p.calls[0].name, "run_query");
        // "SELECT 1" is not valid JSON → string; 10 → number; true → bool.
        assert_eq!(
            p.calls[0].arguments,
            json!({"sql": "SELECT 1", "limit": 10, "dry": true})
        );
    }

    #[test]
    fn dot_underscore_name_resolution() {
        let text = r#"<tool_call>{"name":"file.read","arguments":{"path":"a"}}</tool_call>"#;
        // Offered tool is `file_read`; model emitted `file.read`.
        let p = promote_plain_text_tool_calls(text, &allowed(&["file_read"])).unwrap();
        assert_eq!(p.calls[0].name, "file_read");
    }

    #[test]
    fn case_and_typo_resolve_via_unified_resolver() {
        // The plain-text path now shares the unified `name_repair` resolver, so
        // a case-drifted name (`Get_Weather`) resolves where the old
        // exact+separator-only logic would have aborted the promotion.
        let text = r#"<tool_call>{"name":"Get_Weather","arguments":{"city":"Paris"}}</tool_call>"#;
        let p = promote_plain_text_tool_calls(text, &allowed(&["get_weather"])).unwrap();
        assert_eq!(p.calls[0].name, "get_weather");
    }

    #[test]
    fn unresolved_name_aborts_whole_promotion() {
        // Two blocks, second names a tool we don't offer → no promotion at all.
        let text = "<tool_call>{\"name\":\"a\",\"arguments\":{}}</tool_call>\
                    <tool_call>{\"name\":\"ghost\",\"arguments\":{}}</tool_call>";
        assert!(promote_plain_text_tool_calls(text, &allowed(&["a"])).is_none());
    }

    #[test]
    fn plain_prose_is_not_promoted() {
        let text = "I would call get_weather but I'll just describe it instead.";
        assert!(promote_plain_text_tool_calls(text, &allowed(&["get_weather"])).is_none());
    }

    #[test]
    fn empty_text_returns_none() {
        assert!(promote_plain_text_tool_calls("", &allowed(&["x"])).is_none());
    }

    #[test]
    fn case_insensitive_tags() {
        let text = r#"<Tool_Call>{"name":"x","arguments":{}}</TOOL_CALL>"#;
        let p = promote_plain_text_tool_calls(text, &allowed(&["x"])).unwrap();
        assert_eq!(p.calls[0].name, "x");
    }

    #[test]
    fn missing_arguments_defaults_to_empty_object() {
        let text = r#"<tool_call>{"name":"ping"}</tool_call>"#;
        let p = promote_plain_text_tool_calls(text, &allowed(&["ping"])).unwrap();
        assert_eq!(p.calls[0].arguments, json!({}));
    }

    #[test]
    fn residual_text_preserved_around_block() {
        let text = "before <tool_call>{\"name\":\"x\",\"arguments\":{}}</tool_call> after";
        let p = promote_plain_text_tool_calls(text, &allowed(&["x"])).unwrap();
        assert_eq!(p.residual_text, "before  after");
    }

    #[test]
    fn malformed_json_body_is_ignored() {
        // Unterminated JSON inside the tags → no valid block → None.
        let text = "<tool_call>{not json</tool_call>";
        assert!(promote_plain_text_tool_calls(text, &allowed(&["x"])).is_none());
    }

    #[test]
    fn utf8_text_around_block_slices_safely() {
        let text =
            "天气查询：<tool_call>{\"name\":\"x\",\"arguments\":{\"市\":\"北京\"}}</tool_call>完成";
        let p = promote_plain_text_tool_calls(text, &allowed(&["x"])).unwrap();
        assert_eq!(p.calls[0].arguments, json!({"市": "北京"}));
        assert_eq!(p.residual_text, "天气查询：完成");
    }
}
