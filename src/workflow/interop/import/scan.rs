//! Walk a comment-blanked `.workflow.js` body in source order and emit the
//! calls it contains as [`ScanEvent`]s.
//!
//! This layer decides WHAT was called, never what it means: no DAG, no phase
//! plan, no manifest. [`super::scan_bare`] owns that. The split matters because
//! the walk is string-aware and the interpretation is not — a prompt reading
//! `"run phase(2) in parallel"` must never register as a marker, and keeping
//! the quote tracking in one place is what makes that true for every construct
//! at once.

use crate::workflow::interop::consts::ConstTable;

use super::lexer::{read_agent_prompt, read_clarify_choices, read_first_string_literal_chars};
use super::opts::{read_agent_opts, AgentOpts};

/// Whether `skeleton` uses `keyword` as a statement keyword — the identifier
/// followed by optional whitespace and `(`.
///
/// Two things a plain `contains` cannot do: accept every spacing JS allows
/// (`for(`, `for (`, `for\t(`), and reject the keyword occurring INSIDE a
/// longer identifier (`forEach(`, `iffy(`, a variable named `switcher`). The
/// leading-boundary check is what makes it safe to match without a space.
pub(super) fn contains_call_like_keyword(skeleton: &str, keyword: &str) -> bool {
    let bytes = skeleton.as_bytes();
    skeleton.match_indices(keyword).any(|(at, _)| {
        let leading_ok = at == 0
            || !bytes
                .get(at - 1)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'$');
        if !leading_ok {
            return false;
        }
        skeleton[at + keyword.len()..].trim_start().starts_with('(')
    })
}

/// One ordered call recovered from a bare `.workflow.js` scan.
pub(super) enum ScanEvent {
    /// A `phase("title")` marker — the title of its string-literal argument.
    Phase(String),
    /// An `agent("prompt", { opts })` call — the prompt plus any recovered opts.
    /// Boxed because `AgentOpts` dwarfs every other variant, and the whole
    /// `Vec<ScanEvent>` would otherwise be padded up to its width.
    Agent(Box<AgentCall>),
    /// A `clarify("question", ["a", "b"])` call (an Aleph extension to the
    /// `.workflow.js` vocabulary) — the question plus any literal choices.
    Clarify(ClarifyCall),
    /// An `agent(…)` / `clarify(…)` call whose first argument is not a static
    /// literal (a bare identifier, `buildPrompt(u)`, a `.map` expression). It is
    /// dynamic and not statically importable; counted for an honest `dropped`
    /// disclosure rather than vanishing silently.
    DynamicPrompt,
    /// The opening of a `parallel([...])` block — the agents up to the matching
    /// [`ParallelEnd`](ScanEvent::ParallelEnd) are siblings (same DAG layer).
    ParallelStart,
    /// The close of the innermost open `parallel([...])` block.
    ParallelEnd,
}

/// A recovered `agent()` call: its prompt and the literal opts that followed.
pub(super) struct AgentCall {
    pub(super) prompt: String,
    pub(super) opts: AgentOpts,
}

/// A recovered `clarify()` call: its question and the literal choice menu
/// (empty for a free-text clarification — the inverse of `render_clarify_call`).
pub(super) struct ClarifyCall {
    pub(super) prompt: String,
    pub(super) choices: Vec<String>,
}

/// Scan `src` for `phase(...)` and `agent(...)` calls in source order, string-
/// aware so a prompt mentioning `phase(`/`agent(` never registers as a call.
///
/// A single forward pass tokenises identifiers and skips string-literal bodies
/// wholesale; only a bare `phase`/`agent`/`parallel` identifier immediately
/// followed by `(` counts, so `subagent(` / `useragent(` and the like never
/// over-match. Calls whose first argument is not a string literal (e.g.
/// `agent(promptVar)`) yield no event. Catches both top-level calls and
/// `() => agent(` inside `parallel([...])`.
///
/// Parenthesis depth is tracked (string contents excluded) so a `parallel(`
/// block's matching `)` is found: the agents between the emitted `ParallelStart`
/// and `ParallelEnd` are siblings of one DAG layer.
pub(super) fn scan_events(src: &str, consts: &ConstTable) -> Vec<ScanEvent> {
    let chars: Vec<char> = src.chars().collect();
    let n = chars.len();
    let mut events = Vec::new();
    let mut i = 0;
    // Depth of `(` nesting in code (not strings). `parallel_watch` records the
    // depth at each open `parallel(` so its close emits `ParallelEnd`.
    let mut paren_depth: i32 = 0;
    let mut parallel_watch: Vec<i32> = Vec::new();
    while i < n {
        let c = chars[i];
        // Skip string-literal bodies so their contents are never tokenised
        // (parens inside a prompt must not perturb the depth count).
        if c == '\'' || c == '"' || c == '`' {
            i += 1;
            while i < n {
                let d = chars[i];
                if d == '\\' {
                    i += 2; // skip the escaped char so \" / \' does not close early
                    continue;
                }
                i += 1;
                if d == c {
                    break;
                }
            }
            continue;
        }
        // Identifier? Consume it, then check for a following `(`.
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < n && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let ident: String = chars[start..i].iter().collect();
            if ident == "phase" || ident == "agent" || ident == "parallel" || ident == "clarify" {
                let mut j = i;
                while j < n && chars[j].is_whitespace() {
                    j += 1;
                }
                if j < n && chars[j] == '(' {
                    // The call's argument list, as a char slice (paren consumed).
                    let after = &chars[j + 1..];
                    match ident.as_str() {
                        // A phase title is always a plain literal.
                        "phase" => {
                            if let Some(lit) = read_first_string_literal_chars(after) {
                                events.push(ScanEvent::Phase(lit));
                            }
                        }
                        // An agent prompt may be a literal or a `[...].join()`
                        // array; the trailing `{ opts }` object is parsed from
                        // where the prompt ends.
                        "agent" => {
                            if let Some((prompt, end)) = read_agent_prompt(after, 0) {
                                let opts = read_agent_opts(after, end, consts);
                                events.push(ScanEvent::Agent(Box::new(AgentCall { prompt, opts })));
                            } else {
                                // A non-literal prompt (`agent(promptVar)`,
                                // `buildPrompt(u)`, a `.map` expression) is
                                // dynamic and not statically importable — record
                                // it so `scan_bare` can report the count instead
                                // of the call vanishing silently (P7).
                                events.push(ScanEvent::DynamicPrompt);
                            }
                        }
                        // A clarify question is a plain/`join`-array literal (the
                        // `read_agent_prompt` shape), optionally followed by a
                        // `["a", "b"]` choices array — the inverse of export's
                        // `render_clarify_call`.
                        "clarify" => {
                            if let Some((prompt, end)) = read_agent_prompt(after, 0) {
                                let choices = read_clarify_choices(after, end);
                                events.push(ScanEvent::Clarify(ClarifyCall { prompt, choices }));
                            } else {
                                events.push(ScanEvent::DynamicPrompt);
                            }
                        }
                        // The block opens at the `(` the main loop is about to
                        // count; watch for the `)` that returns to this depth.
                        "parallel" => {
                            parallel_watch.push(paren_depth);
                            events.push(ScanEvent::ParallelStart);
                        }
                        _ => {}
                    }
                }
            }
            continue;
        }
        // Track code paren depth so `parallel(`'s close can be located. An
        // `agent(...)` / `() =>` call's own parens balance out at a deeper level
        // and never spuriously close the watched block.
        if c == '(' {
            paren_depth += 1;
            i += 1;
            continue;
        }
        if c == ')' {
            paren_depth -= 1;
            if parallel_watch.last() == Some(&paren_depth) {
                parallel_watch.pop();
                events.push(ScanEvent::ParallelEnd);
            }
            i += 1;
            continue;
        }
        i += 1;
    }
    events
}
