//! `⏺ Read(src/x.rs:1-120)` — the tool row headline.
//!
//! Two decisions live here and nowhere else: what a tool is CALLED on the
//! row (Claude Code vocabulary for Aleph's snake_case names, humanised
//! Title Case for everything else, `Server · Tool` for MCP), and which
//! argument is worth one line (a per-tool rule, then a preferred-key
//! fallback in a fixed order — pi-cc-extensions `names.ts`).

use serde_json::Value;

/// Max columns of the argument text (pi `inputClip` default).
pub const ARGS_CLIP: usize = 100;

/// Aleph tool name → row label. Anything not listed is humanised.
/// The alephcore census (`presentation_census.rs`) calls this table and
/// [`display_name`] directly and asserts: every key here names a registered
/// tool, no key is duplicated, no half of an entry is blank, every registered
/// tool renders a non-blank label, and every content-mutating tool has an
/// explicit entry rather than the fallback.
///
/// Keys verified against the live registry (`src/builtin_tools/`,
/// `src/tools/tool_search.rs`, `src/agents/subagent_tool/mod.rs`) — see
/// task-4-report.md for the full name-check table. Two keys differ from
/// the initial draft: the shell tool registers as `"bash"` (not
/// `"shell_exec"`), and the web search tool registers as `"search"` (not
/// `"web_search"`).
pub const DISPLAY_NAMES: &[(&str, &str)] = &[
    ("file_read", "Read"),
    ("file_edit", "Edit"),
    ("file_write", "Write"),
    ("apply_patch", "Patch"),
    ("file_ops", "Files"),
    ("bash", "Bash"),
    ("grep", "Grep"),
    ("find", "Find"),
    ("web_fetch", "Fetch"),
    ("search", "Search"),
    ("scratchpad", "Plan"),
    ("tool_search", "Tools"),
    ("terminal", "Terminal"),
    ("memory_search", "Memory"),
    ("note_manage", "Note"),
    ("subagent", "Agent"),
    ("ctx_search", "Context"),
];

/// Fallback argument keys, first present wins. Objects/arrays are skipped.
pub const PREFERRED_ARG_KEYS: &[&str] = &[
    "path", "file_path", "command", "query", "question", "pattern", "url", "name", "id",
    "action", "message",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSummary {
    pub display_name: String,
    pub args_text: String,
}

/// `snake_case` / `camelCase` / `kebab-case` → `Title Case`.
#[must_use]
pub fn humanize(name: &str) -> String {
    let mut spaced = String::with_capacity(name.len() + 4);
    let chars: Vec<char> = name.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' || c == '-' {
            spaced.push(' ');
            continue;
        }
        if c.is_ascii_uppercase() && i > 0 && chars[i - 1].is_ascii_alphanumeric() && !chars[i - 1].is_ascii_uppercase() {
            spaced.push(' ');
        }
        spaced.push(c);
    }
    spaced
        .split_whitespace()
        .map(|w| {
            let mut cs = w.chars();
            match cs.next() {
                Some(f) => f.to_uppercase().collect::<String>() + cs.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// MCP tools arrive as `server__tool` (or `mcp__server__tool`); render
/// `Server · Tool`. Returns `None` when the name is not MCP-shaped.
fn mcp_display(name: &str) -> Option<String> {
    let rest = name.strip_prefix("mcp__").unwrap_or(name);
    let (server, tool) = rest.split_once("__")?;
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some(format!("{} · {}", humanize(server), humanize(tool)))
}

#[must_use]
pub fn display_name(tool: &str) -> String {
    if let Some((_, label)) = DISPLAY_NAMES.iter().find(|(n, _)| *n == tool) {
        return (*label).to_string();
    }
    if let Some(mcp) = mcp_display(tool) {
        return mcp;
    }
    humanize(tool)
}

/// First line only, clipped to `max` chars with an ellipsis.
#[must_use]
pub fn clip_one_line(s: &str, max: usize) -> String {
    let first = s.lines().next().unwrap_or("").trim();
    let n = first.chars().count();
    if n <= max {
        return first.to_string();
    }
    let keep = max.saturating_sub(1);
    let mut out: String = first.chars().take(keep).collect();
    out.push('…');
    out
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).filter(|s| !s.trim().is_empty())
}

fn read_range(args: &Value) -> Option<String> {
    let offset = args.get("offset").and_then(Value::as_u64);
    let limit = args.get("limit").and_then(Value::as_u64);
    match (offset, limit) {
        (Some(o), Some(l)) => Some(format!("{}-{}", o.max(1), o.max(1) + l.saturating_sub(1))),
        (Some(o), None) => Some(format!("{}-", o.max(1))),
        (None, Some(l)) => Some(format!("1-{l}")),
        (None, None) => None,
    }
}

fn shorten_path(p: &str) -> String {
    // Keep it as given; surfaces may relativise against cwd. Only collapse
    // a Windows drive-absolute or POSIX home prefix if very long.
    if p.chars().count() <= ARGS_CLIP {
        return p.to_string();
    }
    let tail: String = p.chars().rev().take(ARGS_CLIP - 1).collect::<Vec<_>>().into_iter().rev().collect();
    format!("…{tail}")
}

/// The one-line argument summary for a tool call.
#[must_use]
pub fn summarize(tool: &str, args: &Value) -> CallSummary {
    let display = display_name(tool);
    let text = match tool {
        "file_read" => str_arg(args, "path")
            .or_else(|| str_arg(args, "file_path"))
            .map(|p| match read_range(args) {
                Some(r) => format!("{}:{}", shorten_path(p), r),
                None => shorten_path(p),
            }),
        "file_edit" | "file_write" => str_arg(args, "file_path")
            .or_else(|| str_arg(args, "path"))
            .map(shorten_path),
        "apply_patch" => str_arg(args, "patch").map(|patch| {
            let n = patch.lines().filter(|l| {
                l.starts_with("*** Add File:") || l.starts_with("*** Update File:") || l.starts_with("*** Delete File:")
            }).count();
            format!("{n} file{}", if n == 1 { "" } else { "s" })
        }),
        "grep" | "find" => str_arg(args, "pattern").map(|pat| match str_arg(args, "path") {
            Some(p) => format!("{pat} in {}", shorten_path(p)),
            None => pat.to_string(),
        }),
        "bash" => str_arg(args, "command").map(|c| clip_one_line(c, 80)),
        "subagent" => {
            let who = str_arg(args, "agent").or_else(|| str_arg(args, "name")).unwrap_or("agent");
            let task = str_arg(args, "task").or_else(|| str_arg(args, "prompt")).map(|t| clip_one_line(t, 40));
            Some(match task {
                Some(t) => format!("{who}: {t}"),
                None => who.to_string(),
            })
        }
        _ => PREFERRED_ARG_KEYS.iter().find_map(|k| str_arg(args, k)).map(str::to_string),
    };
    CallSummary {
        display_name: display,
        args_text: clip_one_line(text.as_deref().unwrap_or(""), ARGS_CLIP),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn known_tools_get_claude_code_names() {
        assert_eq!(display_name("file_read"), "Read");
        assert_eq!(display_name("bash"), "Bash");
        assert_eq!(display_name("apply_patch"), "Patch");
    }

    #[test]
    fn unknown_tools_are_humanised_and_mcp_tools_show_server_and_tool() {
        assert_eq!(humanize("get_subagent_result"), "Get Subagent Result");
        assert_eq!(humanize("customTranslate"), "Custom Translate");
        assert_eq!(display_name("mcp__github__search_issues"), "Github · Search Issues");
        assert_eq!(display_name("github__search"), "Github · Search");
        assert_eq!(display_name("weird_tool"), "Weird Tool");
    }

    #[test]
    fn read_shows_path_and_range() {
        let s = summarize("file_read", &json!({"path": "src/a.rs", "offset": 10, "limit": 50}));
        assert_eq!(s.args_text, "src/a.rs:10-59");
        let s = summarize("file_read", &json!({"path": "src/a.rs"}));
        assert_eq!(s.args_text, "src/a.rs");
    }

    #[test]
    fn grep_shows_pattern_in_path_and_bash_shows_the_first_command_line() {
        let s = summarize("grep", &json!({"pattern": "fold|expand", "path": "src"}));
        assert_eq!(s.args_text, "fold|expand in src");
        let s = summarize("bash", &json!({"command": "cargo test -p x\necho done"}));
        assert_eq!(s.args_text, "cargo test -p x");
    }

    #[test]
    fn patch_counts_files_and_subagent_names_agent_and_task() {
        let patch = "*** Begin Patch\n*** Update File: a.rs\n@@\n-x\n+y\n*** Add File: b.rs\n+z\n*** End Patch";
        assert_eq!(summarize("apply_patch", &json!({"patch": patch})).args_text, "2 files");
        let s = summarize("subagent", &json!({"agent": "reviewer", "task": "Review the auth module for injection risks and more"}));
        assert_eq!(s.args_text, "reviewer: Review the auth module for injection ri…");
    }

    #[test]
    fn fallback_walks_preferred_keys_and_skips_objects() {
        let s = summarize("some_tool", &json!({"opts": {"path": "no"}, "url": "https://x.y/z"}));
        assert_eq!(s.args_text, "https://x.y/z");
        let s = summarize("some_tool", &json!({"text": "hi"}));
        assert_eq!(s.args_text, "", "a key outside the preferred list yields no argument");
    }

    #[test]
    fn clip_is_one_line_and_ellipsised() {
        assert_eq!(clip_one_line("a\nb", 10), "a");
        let long = "x".repeat(120);
        let c = clip_one_line(&long, ARGS_CLIP);
        assert_eq!(c.chars().count(), ARGS_CLIP);
        assert!(c.ends_with('…'));
    }
}
