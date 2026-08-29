//! Fallback registry — declarative table of alternative tools to try
//! when a primary tool fails.
//!
//! This is the concrete "tool ladder": a lookup table the harness consults at
//! error-emission time to surface alternative tools in the structured error
//! hint. The harness DOES NOT auto-invoke alternatives (R7: LLM stays in
//! control of tool selection); it only surfaces the suggestions so the LLM
//! sees them in its next turn.
//!
//! ## Why a registry (not prompt steering)
//!
//! The former `provider_guidance.rs` persistence doctrine told the model up
//! front "if search fails, climb the ladder" — a general manual the model had
//! to *remember* when a failure actually happened. That prose was removed
//! under §1.1 prune-the-prompt (a capable model persists natively); the
//! fallback signal now lives ONLY here, surfaced at the moment of failure with
//! the relevant rung in the `tool_result` error message — "here are 3 concrete
//! next moves" instead of an upfront principle. This is self-correction built
//! into the architecture (the error hint) rather than the prompt. Same idea as
//! claude-code's `is_error: true` flag — the LLM decides, we make the decision
//! well-informed.
//!
//! ## Scope
//!
//! The registry is *small on purpose*. We list well-known builtin
//! tools and the broadest families (network / file / exec / memory).
//! Unknown tools (MCP, extensions) fall through to a family heuristic
//! based on naming convention. The point is to give the LLM SOME
//! suggestion every time — not an exhaustive routing table.

use super::error_kind::ToolErrorKind;
use super::service::ToolError;

/// A single fallback suggestion the LLM can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FallbackSuggestion {
    /// Alternative tool name to try.
    pub tool: &'static str,
    /// One-line rationale appended after the tool name in the hint.
    /// Should fit in <60 chars to keep error blocks compact.
    pub reason: &'static str,
}

impl FallbackSuggestion {
    const fn new(tool: &'static str, reason: &'static str) -> Self {
        Self { tool, reason }
    }
}

/// Coarse tool family used when the exact tool name isn't in the
/// registry — derived from naming conventions (`search*` → Network,
/// `file_*` → File, …). Keeps the suggestions relevant even for
/// MCP / extension / skill tools we don't statically know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolFamily {
    /// Anything that fetches data from the public internet: search,
    /// `web_fetch`, `fetch_url`, `curl_like`, `http_get`, scrape, ...
    Network,
    /// Filesystem operations: `file_read`, `file_write`, `file_ops`, ls, ...
    File,
    /// Shell / code execution: bash, exec, `code_exec`, python, run, ...
    Exec,
    /// Memory subsystem: `memory_search`, recall, note_*, `raw_memory`_*.
    Memory,
    /// Skill / MCP / extension dispatch where no other family fits.
    Other,
}

impl ToolFamily {
    fn from_name(name: &str) -> Self {
        let n = name.to_ascii_lowercase();
        // Prefix-based families are checked FIRST so that
        // `memory_search` lands in `Memory` instead of getting captured
        // by the substring `contains("search")` heuristic below. Same
        // for `file_*` (could otherwise be mis-flagged) and the
        // exec-name prefixes.
        if n.starts_with("memory_") || n.starts_with("recall_") || n.starts_with("note_") {
            return Self::Memory;
        }
        if n.starts_with("file_") || n.starts_with("edit_") {
            return Self::File;
        }
        if n == "bash"
            || n == "shell"
            || n == "exec"
            || n.contains("code_exec")
            || n == "python"
            || n == "run"
        {
            return Self::Exec;
        }
        // Network family — broadest, uses substring matches so it must
        // come AFTER the more specific prefix-based families above.
        if n.contains("search")
            || n.contains("web_fetch")
            || n.contains("fetch_url")
            || n.contains("http")
            || n.contains("scrape")
            || n == "web"
            || n.starts_with("autocli")
            || n.starts_with("playwright")
            || n.starts_with("chrome")
        {
            return Self::Network;
        }
        // Bare File-family names (`ls`, `read`, `write`) — kept at the
        // tail because they are common English words that could
        // collide with other contexts; the structured prefixes above
        // catch the cases that actually matter.
        // `grep` / `find` are exact names, not substrings: the tree-search
        // builtins read the filesystem, so a failure there should suggest
        // the file family, never the web-research ladder `search` heads.
        if n == "ls" || n == "read" || n == "write" || n == "grep" || n == "find" {
            return Self::File;
        }
        Self::Other
    }
}

/// Suggest alternative tools for a `(tool_name, error_kind)` pair.
///
/// The returned list is ordered most-promising-first. Empty when the
/// kind doesn't admit a sensible alternative (e.g. `Duplicate` is a
/// developer error, not a routing signal).
#[must_use]
pub fn suggest_alternatives(tool_name: &str, kind: ToolErrorKind) -> Vec<FallbackSuggestion> {
    // Validation / Permission / ToolNotFound / Duplicate are not
    // routing signals — they need the caller to fix the call shape,
    // not switch tools. Returning empty here keeps the error hint
    // focused on what actually helps.
    if matches!(
        kind,
        ToolErrorKind::Validation
            | ToolErrorKind::Permission
            | ToolErrorKind::ToolNotFound
            | ToolErrorKind::Duplicate
    ) {
        return Vec::new();
    }

    // Exact-name hits go first — they encode known-good ladders.
    if let Some(s) = exact_suggestions(tool_name, kind) {
        return s;
    }

    // Fall through to family-based suggestions.
    family_suggestions(ToolFamily::from_name(tool_name), kind)
}

/// Exact-name registry entries. Returns `None` to defer to the family
/// fallback when the tool name isn't recognised.
fn exact_suggestions(tool_name: &str, kind: ToolErrorKind) -> Option<Vec<FallbackSuggestion>> {
    use FallbackSuggestion as S;
    match tool_name {
        // `search` — the entry-point of the web-research ladder. Rule
        // 13 of guidelines.rs documents the canonical climb.
        "search" => Some(match kind {
            ToolErrorKind::RateLimited | ToolErrorKind::Unauthorized => vec![
                S::new("web_fetch", "go direct to a known canonical URL"),
                S::new("browser_open", "open the gated page in a logged-in browser"),
            ],
            ToolErrorKind::EmptyResult => vec![
                S::new(
                    "search",
                    "retry with broader keywords or a different engine",
                ),
                S::new("web_fetch", "try a canonical URL of the resource directly"),
            ],
            ToolErrorKind::BlockedByPolicy | ToolErrorKind::UpstreamServerError => vec![
                S::new("web_fetch", "different source (BBC/AP/Wikipedia/Reuters)"),
                S::new(
                    "browser_open",
                    "browser-backed fetch via a logged-in session",
                ),
            ],
            _ => vec![
                S::new("web_fetch", "if you have a canonical URL in mind"),
                S::new("search", "retry with refined keywords"),
            ],
        }),

        // `web_fetch` — middle rung. When IT fails, switch source or
        // climb to browser tooling.
        "web_fetch" => Some(match kind {
            ToolErrorKind::Unauthorized
            | ToolErrorKind::BlockedByPolicy
            | ToolErrorKind::UpstreamNotFound => vec![
                S::new("web_fetch", "alternate source (BBC/AP/NYT/Wikipedia)"),
                S::new("search", "find a different canonical URL via search"),
                S::new(
                    "browser_open",
                    "logged-in browser session for gated content",
                ),
            ],
            ToolErrorKind::RateLimited => vec![
                S::new("web_fetch", "different domain to avoid the rate-limit"),
                S::new("search", "use search to surface a fresh source"),
            ],
            ToolErrorKind::EmptyResult => vec![
                S::new("search", "broaden the search or check the URL is current"),
                S::new("web_fetch", "alternate source for the same resource"),
            ],
            _ => vec![
                S::new("search", "find another URL for the same resource"),
                S::new(
                    "browser_open",
                    "browser-backed fetch for sites that gate API",
                ),
            ],
        }),

        // `bash` — exec failures rarely benefit from a different tool.
        // They benefit from the caller examining the error and either
        // adjusting the command or escalating to `code_exec` for
        // longer-running work.
        "bash" => Some(match kind {
            ToolErrorKind::Timeout => vec![S::new(
                "code_exec",
                "longer-running runtime; bash inherits a strict per-call budget",
            )],
            _ => vec![],
        }),

        // `memory_search` — the memory ladder is broader queries +
        // browsing the raw store when the query itself is the problem.
        "memory_search" => Some(match kind {
            ToolErrorKind::EmptyResult => vec![
                S::new("memory_search", "broader query or different scope"),
                S::new(
                    "memory_browse",
                    "walk the raw memory VFS instead of querying it",
                ),
            ],
            _ => vec![],
        }),

        _ => None,
    }
}

/// Family-based fallback when the tool name isn't in the exact registry.
fn family_suggestions(family: ToolFamily, kind: ToolErrorKind) -> Vec<FallbackSuggestion> {
    use FallbackSuggestion as S;
    match (family, kind) {
        (
            ToolFamily::Network,
            ToolErrorKind::Unauthorized
            | ToolErrorKind::RateLimited
            | ToolErrorKind::BlockedByPolicy,
        ) => vec![
            S::new("search", "find a different source via search"),
            S::new("web_fetch", "fetch an alternate canonical URL directly"),
            S::new(
                "browser_open",
                "use a logged-in browser session if available",
            ),
        ],
        (ToolFamily::Network, ToolErrorKind::EmptyResult) => vec![
            S::new("search", "broaden the query or pick a different engine"),
            S::new("web_fetch", "alternate source for the same resource"),
        ],
        (ToolFamily::Network, _) => vec![S::new(
            "search",
            "approach the goal from a different source",
        )],

        (ToolFamily::File, ToolErrorKind::UpstreamNotFound | ToolErrorKind::Execution) => {
            vec![S::new(
                "file_ops",
                "stat / list the directory to confirm the path",
            )]
        }
        (ToolFamily::File, _) => Vec::new(),

        (ToolFamily::Exec, ToolErrorKind::Timeout) => vec![S::new(
            "code_exec",
            "longer-running runtime than bash for slow commands",
        )],
        (ToolFamily::Exec, _) => Vec::new(),

        (ToolFamily::Memory, ToolErrorKind::EmptyResult) => vec![
            S::new("memory_search", "broaden the query or widen the scope"),
            S::new(
                "memory_browse",
                "walk the raw memory VFS instead of querying it",
            ),
        ],
        (ToolFamily::Memory, _) => Vec::new(),

        // `Other` family — keep it generic. The LLM at least gets a
        // nudge to switch instead of repeating the same failed call.
        (ToolFamily::Other, _) => Vec::new(),
    }
}

/// Render the LLM-facing routing hint appended to
/// `SessionEvent::ToolError.error` strings by
/// `harness::agent::act::emit_tool_error`.
///
/// Format (single line, machine-grep-able prefix, human-readable tail):
///
/// ```text
/// [persistence_hint kind=<kind> tool=<tool>
///  switch=<a>: <reason>; <b>: <reason>; ...
///  doctrine=ladder_must_be_climbed_before_fail]
/// ```
///
/// When the registry has no suggestion (e.g. Validation / Permission
/// errors, where switching tools doesn't help), the `switch=` clause
/// is omitted and the hint shrinks to the doctrine reminder. The hint
/// is ALWAYS appended even when no suggestion is available — the
/// `doctrine=ladder_must_be_climbed_before_fail` reminder keeps the
/// LLM aligned with the persistence doctrine in `provider_guidance.rs`.
///
/// The output is intentionally compact (single line, ~200 chars typ.)
/// so it doesn't dominate the `tool_result` body in the next prompt.
#[must_use]
pub fn render_persistence_hint(err: &ToolError, tool_name: &str) -> String {
    let kind = err.kind();
    // A cancelled call is not a rung of any ladder. The user pressed stop; the
    // tool did not fail, was not blocked, and has no alternative worth
    // suggesting. Telling the model to "climb the ladder before failing" here
    // reads as advice about a failure that never happened.
    if kind == ToolErrorKind::Cancelled {
        return String::new();
    }
    let alternatives = suggest_alternatives(tool_name, kind);

    let mut hint = String::with_capacity(160);
    hint.push_str("\n\n[persistence_hint kind=");
    hint.push_str(kind.label());
    hint.push_str(" tool=");
    hint.push_str(tool_name);

    if !alternatives.is_empty() {
        hint.push_str(" switch=");
        for (i, s) in alternatives.iter().enumerate() {
            if i > 0 {
                hint.push_str("; ");
            }
            hint.push_str(s.tool);
            hint.push_str(": ");
            hint.push_str(s.reason);
        }
    }

    hint.push_str(" doctrine=ladder_must_be_climbed_before_fail]");
    hint
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_rate_limit_recommends_web_fetch_first() {
        let s = suggest_alternatives("search", ToolErrorKind::RateLimited);
        assert_eq!(s.first().map(|x| x.tool), Some("web_fetch"));
    }

    #[test]
    fn web_fetch_unauthorized_recommends_alternate_source_first() {
        let s = suggest_alternatives("web_fetch", ToolErrorKind::Unauthorized);
        assert_eq!(s.first().map(|x| x.tool), Some("web_fetch"));
        assert!(s.iter().any(|x| x.reason.contains("alternate source")));
    }

    #[test]
    fn validation_returns_empty_no_routing_signal() {
        // Validation / Permission / ToolNotFound / Duplicate are caller
        // bugs — switching tools doesn't help.
        assert!(suggest_alternatives("search", ToolErrorKind::Validation).is_empty());
        assert!(suggest_alternatives("bash", ToolErrorKind::Permission).is_empty());
        assert!(suggest_alternatives("foo", ToolErrorKind::ToolNotFound).is_empty());
        assert!(suggest_alternatives("foo", ToolErrorKind::Duplicate).is_empty());
    }

    #[test]
    fn family_fallback_kicks_in_for_unknown_tool_names() {
        // `http_get_metrics` isn't in exact registry — Network family
        // should still suggest the ladder.
        let s = suggest_alternatives("http_get_metrics", ToolErrorKind::Unauthorized);
        assert!(!s.is_empty());
        assert!(s.iter().any(|x| x.tool == "search"));
    }

    /// Every rung of the ladder must name a tool the model can actually call.
    ///
    /// This table told the model to `switch=autocli` and
    /// `switch=raw_memory_search` for months; neither has ever been a
    /// registered tool, so following the hint cost a guaranteed
    /// `ToolError::NotFound` turn — and the cross-batch failure memo then
    /// banned the phantom, while the rung it stood for still did not exist.
    /// The harness already learned this once (`act.rs`: "this said 'call
    /// `list_tools`', which production never registers, so the model looped on
    /// NotFound") and `READ_ONLY_TOOLS` learned it again in the 2026-07 ghost
    /// sweep. This is the check that stops the third time.
    #[test]
    fn every_suggested_tool_is_registered() {
        use crate::executor::BUILTIN_TOOL_DEFINITIONS;

        const KINDS: &[ToolErrorKind] = &[
            ToolErrorKind::Unauthorized,
            ToolErrorKind::RateLimited,
            ToolErrorKind::UpstreamNotFound,
            ToolErrorKind::UpstreamServerError,
            ToolErrorKind::BlockedByPolicy,
            ToolErrorKind::EmptyResult,
            ToolErrorKind::Timeout,
            ToolErrorKind::Transport,
            ToolErrorKind::Validation,
            ToolErrorKind::Permission,
            ToolErrorKind::ToolNotFound,
            ToolErrorKind::Duplicate,
            ToolErrorKind::Execution,
        ];
        // Exact-registry names plus one probe per family, so the family
        // fallback arms are covered too.
        const PROBES: &[&str] = &[
            "search",
            "web_fetch",
            "bash",
            "memory_search",
            "some_mcp__http_get",
            "file_read",
            "code_exec",
            "memory_timeline",
            "totally_unknown_tool",
        ];

        for probe in PROBES {
            for kind in KINDS {
                for s in suggest_alternatives(probe, *kind) {
                    assert!(
                        BUILTIN_TOOL_DEFINITIONS.iter().any(|d| d.name == s.tool),
                        "suggest_alternatives({probe}, {kind:?}) offers '{}', which is not \
                         in BUILTIN_TOOL_DEFINITIONS — the model would burn a turn on \
                         ToolError::NotFound following it",
                        s.tool
                    );
                }
            }
        }
    }

    #[test]
    fn bash_timeout_suggests_code_exec() {
        let s = suggest_alternatives("bash", ToolErrorKind::Timeout);
        assert_eq!(s.first().map(|x| x.tool), Some("code_exec"));
    }

    #[test]
    fn memory_empty_suggests_broaden_and_raw() {
        let s = suggest_alternatives("memory_search", ToolErrorKind::EmptyResult);
        assert!(s.iter().any(|x| x.tool == "memory_search"));
        assert!(s.iter().any(|x| x.tool == "memory_browse"));
    }

    #[test]
    fn render_persistence_hint_includes_kind_tool_and_switch_for_known_alts() {
        let err = ToolError::Execution {
            name: "web_fetch".into(),
            cause: "HTTP 401 unauthorized".into(),
        };
        let hint = render_persistence_hint(&err, "web_fetch");
        assert!(hint.contains("kind=unauthorized"));
        assert!(hint.contains("tool=web_fetch"));
        assert!(hint.contains("switch="));
        assert!(hint.contains("alternate source"));
        assert!(hint.contains("doctrine=ladder_must_be_climbed_before_fail"));
    }

    #[test]
    fn render_persistence_hint_omits_switch_clause_for_validation() {
        // Validation errors are caller bugs — no tool switch can fix
        // them. The hint should still carry the doctrine reminder so
        // the LLM knows not to spiral into `fail` for a single
        // missing-arg.
        let err = ToolError::ValidationFailed {
            name: "search".into(),
            cause: "query is required".into(),
        };
        let hint = render_persistence_hint(&err, "search");
        assert!(hint.contains("kind=validation"));
        assert!(!hint.contains("switch="));
        assert!(hint.contains("doctrine=ladder_must_be_climbed_before_fail"));
    }

    #[test]
    fn render_persistence_hint_is_single_line_and_compact() {
        // The hint is appended to existing error messages — it must
        // not contain embedded newlines that would break grep / log
        // parsing, and must stay under a reasonable budget so it
        // doesn't dominate the tool_result body.
        let err = ToolError::Execution {
            name: "search".into(),
            cause: "got 429 too many requests".into(),
        };
        let hint = render_persistence_hint(&err, "search");
        // Leading \n\n separates from the original error; the body
        // itself must not contain further newlines.
        let body = hint.trim_start_matches('\n');
        assert!(!body.contains('\n'));
        assert!(hint.len() < 400);
    }

    #[test]
    fn family_classifier_recognises_naming_conventions() {
        assert_eq!(ToolFamily::from_name("web_fetch"), ToolFamily::Network);
        assert_eq!(ToolFamily::from_name("search_news"), ToolFamily::Network);
        assert_eq!(ToolFamily::from_name("autocli_login"), ToolFamily::Network);
        assert_eq!(ToolFamily::from_name("file_read"), ToolFamily::File);
        assert_eq!(ToolFamily::from_name("bash"), ToolFamily::Exec);
        assert_eq!(ToolFamily::from_name("code_exec"), ToolFamily::Exec);
        assert_eq!(ToolFamily::from_name("memory_search"), ToolFamily::Memory);
        assert_eq!(ToolFamily::from_name("xyzzy"), ToolFamily::Other);
    }
}
