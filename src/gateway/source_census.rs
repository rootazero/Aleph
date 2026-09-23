//! Shared scanners for the gateway's source-level census tests.
//!
//! Several pins in this directory answer a question no compile-anchored test
//! can: what does a producer's *source text* actually publish? They used to
//! each name the file they read, and a file split is enough to blind that:
//! `event_scope`'s `node.*` pin read `server/handler.rs` alone, `4d1061370`
//! moved both producers into `server/connection/`, and the pin kept compiling
//! against a file that now only re-exports — it scraped nothing and said so
//! only because its non-vacuity assert happened to be first (N7, r11). A third
//! producer (`cluster/enrollment.rs`) was never in its corpus at all.
//! [`all_topic_producers`] walks the whole `src/` tree instead, so a moved
//! producer is still found and a new one is found on the day it lands.
//!
//! The scan follows the repo rules for source scanning: `\r` is stripped,
//! `#[cfg(test)]` items AND whole-file test modules are removed by
//! [`crate::utils::source_scan::production_text`] (path-aware — a `tests.rs`
//! its parent declares under `#[cfg(test)]` contributes nothing), and comments
//! are removed by the lexer, so a doc comment naming a topic can neither
//! satisfy nor break a census. The first argument is read up to its first
//! top-level comma: an earlier scraper required the quote to sit right after
//! the paren and saw 2 of the 4 topics once rustfmt wrapped the call.
//!
//! It sees `TopicEvent::new` producers only: an envelope built by hand as
//! `json!({"topic": …})` is out of its view, stated so no one reads this
//! census as exhaustive.

/// The half of a Rust source file that ships: CRLF-normalized, everything from
/// the first `#[cfg(test)]` onward removed, comment lines dropped.
pub(crate) fn production_prefix(src: &str) -> String {
    crate::utils::source_scan::strip_comment_lines(&crate::utils::source_scan::production_prefix(
        src,
    ))
}

use std::collections::BTreeMap;
use std::path::Path;

use crate::utils::source_scan::{
    code_keeping_literals, production_text, rust_sources_under, strip_visibility,
};

/// Every topic literal in a `TopicEvent::new("…", …)` call in `src`.
///
/// The literal-only view of [`topic_event_first_args`] — one scanner, so the
/// two cannot disagree about where an argument ends. Pass
/// [`production_prefix`] output, not the raw file.
pub(crate) fn topic_event_literals(src: &str) -> Vec<String> {
    topic_event_first_args(src)
        .iter()
        .filter_map(|arg| literal_payload(arg))
        .map(str::to_string)
        .collect()
}

/// One `TopicEvent::new(<topic>, …)` call in the production half of a file
/// under `src/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TopicProducer {
    /// Path relative to the crate root, `/`-separated (`src/gateway/…`).
    pub(crate) file: String,
    /// The first argument's source text, whitespace runs collapsed.
    pub(crate) expr: String,
    /// The topic, when `expr` is a string literal or a `&str` constant this
    /// census can resolve. `None` means "composed — this scan cannot say",
    /// never "no topic".
    pub(crate) topic: Option<String>,
}

/// How many `const A: &str = B;` hops a resolution may follow. Deep enough
/// for every alias chain at HEAD (the longest is two: `TREE_TOPIC` →
/// `aleph_protocol::subagent_tree::TOPIC` → literal), shallow enough that a
/// cycle ends as `None` rather than a hang.
const RESOLVE_HOPS: usize = 4;

/// Every raw `TopicEvent::new` producer in production code under `src/`.
///
/// Constants are resolved against every `const NAME: &str = …;` and every
/// `use <path> as NAME;` in the production half of `src/` and of
/// `shared/protocol/src/` (where `aleph_protocol`'s topic constants live).
pub(crate) fn all_topic_producers() -> Vec<TopicProducer> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = production_sources(&root.join("src"));
    let protocol = production_sources(&root.join("shared/protocol/src"));
    let mut consts = StrConsts::default();
    for (rel, code) in src.iter().chain(protocol.iter()) {
        consts.absorb(rel, code);
    }
    let mut out = Vec::new();
    for (rel, code) in &src {
        for expr in topic_event_first_args(code) {
            let topic = consts.resolve(&expr, rel, RESOLVE_HOPS);
            out.push(TopicProducer {
                file: rel.clone(),
                expr,
                topic,
            });
        }
    }
    out
}

/// `(repo-relative path, production code with comments removed and literal
/// payloads KEPT)` for every `.rs` file under `root`.
fn production_sources(root: &Path) -> Vec<(String, String)> {
    rust_sources_under(root)
        .into_iter()
        .map(|(rel, text)| {
            let code = code_keeping_literals(&production_text(Path::new(&rel), &text));
            (rel, code)
        })
        .collect()
}

/// The first argument of every `TopicEvent::new(` call in `code`, trimmed and
/// with interior whitespace runs collapsed to one space.
///
/// Ends at the first top-level `,` (or the call's closing paren), tracking
/// `()[]{}` depth and string literals, so a composed argument such as
/// `format!("a.{}", b)` comes back whole rather than cut at its inner comma.
pub(crate) fn topic_event_first_args(code: &str) -> Vec<String> {
    let mut out = Vec::new();
    for seg in code.split("TopicEvent::new(").skip(1) {
        let mut depth = 0usize;
        let mut in_str = false;
        let mut escaped = false;
        let mut end = None;
        for (i, c) in seg.char_indices() {
            if in_str {
                match (escaped, c) {
                    (true, _) => escaped = false,
                    (false, '\\') => escaped = true,
                    (false, '"') => in_str = false,
                    _ => {}
                }
                continue;
            }
            match c {
                '"' => in_str = true,
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' if depth == 0 => {
                    end = Some(i);
                    break;
                }
                ')' | ']' | '}' => depth -= 1,
                ',' if depth == 0 => {
                    end = Some(i);
                    break;
                }
                _ => {}
            }
        }
        let Some(end) = end else { continue };
        let arg = seg[..end].split_whitespace().collect::<Vec<_>>().join(" ");
        if !arg.is_empty() {
            out.push(arg);
        }
    }
    out
}

/// The payload of a plain `"…"` literal, or `None` for anything else.
fn literal_payload(expr: &str) -> Option<&str> {
    expr.strip_prefix('"')?
        .strip_suffix('"')
        .filter(|inner| !inner.contains('"'))
}

/// The module name a file defines: its stem, or its directory for `mod.rs`.
fn module_stem(file: &str) -> Option<&str> {
    let path = Path::new(file);
    let stem = path.file_stem()?.to_str()?;
    if stem == "mod" {
        path.parent()?.file_name()?.to_str()
    } else {
        Some(stem)
    }
}

/// `NAME -> [(defining file, right-hand side)]` for `&str` constants and
/// `use … as NAME` aliases.
#[derive(Default)]
struct StrConsts(BTreeMap<String, Vec<(String, String)>>);

impl StrConsts {
    /// Record every one-line `const NAME: &str = RHS;` /
    /// `const NAME: &'static str = RHS;` and `use PATH as NAME;` in `code`.
    /// A declaration split across lines is not recorded: a producer naming it
    /// then resolves to `None`, which T02's census reports — the loud side.
    fn absorb(&mut self, file: &str, code: &str) {
        for line in code.lines() {
            let line = strip_visibility(line.trim());
            let decl = if let Some(rest) = line.strip_prefix("const ") {
                rest.split_once(':').and_then(|(name, rest)| {
                    let (ty, rhs) = rest.split_once('=')?;
                    let ty = ty.trim();
                    if ty != "&str" && ty != "&'static str" {
                        return None;
                    }
                    Some((name.trim(), rhs.trim().strip_suffix(';')?.trim()))
                })
            } else if let Some(rest) = line.strip_prefix("use ") {
                rest.strip_suffix(';')
                    .and_then(|r| r.split_once(" as "))
                    .map(|(path, alias)| (alias.trim(), path.trim()))
            } else {
                None
            };
            if let Some((name, rhs)) = decl {
                self.0
                    .entry(name.to_string())
                    .or_default()
                    .push((file.to_string(), rhs.to_string()));
            }
        }
    }

    /// Resolve `expr` (as written in `file`) to a string, or `None`.
    ///
    /// A qualified path (`aleph_protocol::pty::X`, `crate::gateway::event_bus::X`)
    /// picks the definition whose module stem equals the qualifier; a bare
    /// name prefers a definition in the same file, else the unique one
    /// anywhere. Ambiguity is `None` — never a guess.
    fn resolve(&self, expr: &str, file: &str, hops: usize) -> Option<String> {
        if let Some(lit) = literal_payload(expr) {
            return Some(lit.to_string());
        }
        if hops == 0 {
            return None;
        }
        let segments: Vec<&str> = expr.split("::").map(str::trim).collect();
        let name = *segments.last()?;
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            return None;
        }
        let candidates = self.0.get(name)?;
        let qualifier = segments
            .len()
            .checked_sub(2)
            .map(|i| segments[i])
            .filter(|q| !matches!(*q, "self" | "super" | "crate"));
        let picked: Vec<&(String, String)> = match qualifier {
            Some(q) => candidates
                .iter()
                .filter(|(def, _)| module_stem(def) == Some(q))
                .collect(),
            None => {
                let local: Vec<_> = candidates.iter().filter(|(def, _)| def == file).collect();
                if local.is_empty() {
                    candidates.iter().collect()
                } else {
                    local
                }
            }
        };
        let [only] = picked.as_slice() else {
            return None;
        };
        let (def_file, rhs) = &**only;
        self.resolve(rhs, def_file, hops - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_production_prefix_drops_test_code_and_comments() {
        let src =
            "let a = 1;\r\n// TopicEvent::new(\"commented.out\", x)\r\n#[cfg(test)]\nlet b = 2;";
        let prod = production_prefix(src);
        assert!(prod.contains("let a = 1;"));
        assert!(
            !prod.contains("commented.out"),
            "a commented-out producer must not satisfy a census"
        );
        assert!(
            !prod.contains("let b = 2;"),
            "the CRLF checkout must split on `#[cfg(test)]` like the LF one"
        );
    }

    #[test]
    fn the_scanner_sees_a_literal_wrapped_onto_the_next_line() {
        let src = "publish(&TopicEvent::new(\n    \"node.connected\",\n    data,\n));\n\
                   publish(&TopicEvent::new(\"presence.joined\", data));\n\
                   publish(&TopicEvent::new(&composed, data));";
        assert_eq!(
            topic_event_literals(src),
            vec!["node.connected".to_string(), "presence.joined".to_string()],
            "a wrapped literal counts and a composed topic is skipped"
        );
    }

    #[test]
    fn the_first_argument_scanner_keeps_composed_arguments_whole() {
        let src = "publish(&TopicEvent::new(\n    \"node.connected\",\n    data,\n));\n\
                   publish(&TopicEvent::new(\"presence.joined\", data));\n\
                   publish(&TopicEvent::new(format!(\"a.{}\", b), data));\n\
                   publish(&crate::gateway::event_bus::TopicEvent::new(\n    \
                   aleph_protocol::pty::PTY_EXIT_TOPIC,\n    d,\n));";
        assert_eq!(
            topic_event_first_args(src),
            vec![
                "\"node.connected\"".to_string(),
                "\"presence.joined\"".to_string(),
                "format!(\"a.{}\", b)".to_string(),
                "aleph_protocol::pty::PTY_EXIT_TOPIC".to_string(),
            ],
            "a wrapped literal, an inline literal, a composed argument with an inner \
             comma, and a path-qualified constant — each read up to its top-level comma"
        );
    }

    #[test]
    fn a_constant_resolves_through_its_qualifier_and_through_an_alias() {
        let mut consts = StrConsts::default();
        consts.absorb(
            "shared/protocol/src/pty.rs",
            "pub const PTY_EXIT_TOPIC: &str = \"pty.exit\";",
        );
        consts.absorb(
            "shared/protocol/src/artifact.rs",
            "pub const TOPIC: &str = \"session.artifact\";",
        );
        consts.absorb(
            "shared/protocol/src/canvas.rs",
            "pub const TOPIC: &str = \"canvas.updated\";",
        );
        consts.absorb(
            "src/gateway/x.rs",
            "pub use aleph_protocol::artifact::TOPIC as ARTIFACT_TOPIC;\n\
             const LOCAL: &'static str = aleph_protocol::canvas::TOPIC;",
        );
        let resolve = |expr: &str, file: &str| consts.resolve(expr, file, RESOLVE_HOPS);
        assert_eq!(
            resolve("aleph_protocol::pty::PTY_EXIT_TOPIC", "src/a.rs").as_deref(),
            Some("pty.exit")
        );
        assert_eq!(
            resolve("ARTIFACT_TOPIC", "src/gateway/x.rs").as_deref(),
            Some("session.artifact"),
            "a `pub use … as NAME` alias resolves through its path"
        );
        assert_eq!(
            resolve("LOCAL", "src/gateway/x.rs").as_deref(),
            Some("canvas.updated"),
            "a const whose right-hand side is another const resolves transitively"
        );
        assert_eq!(
            resolve("TOPIC", "src/a.rs"),
            None,
            "two `TOPIC`s, neither local, no qualifier: unknown — never a guess"
        );
        assert_eq!(resolve("self.topic()", "src/a.rs"), None);
    }

    #[test]
    fn the_census_walks_the_whole_tree() {
        let producers = all_topic_producers();
        let has = |file: &str, topic: &str| {
            producers
                .iter()
                .any(|p| p.file == file && p.topic.as_deref() == Some(topic))
        };
        // Anchors for the WALK, not a coverage list: each is a producer the
        // named-file pins could not see, or a constant spelling they skipped.
        for (file, topic) in [
            ("src/gateway/server/connection/mod.rs", "node.connected"),
            ("src/gateway/server/connection/cleanup.rs", "node.disconnected"),
            ("src/cluster/enrollment.rs", "node.disconnected"),
            ("src/gateway/pty/manager.rs", "pty.screen"),
            ("src/gateway/subagent_tree_relay.rs", "run.subagent_tree"),
            ("src/gateway/event_emitter/artifact_ping.rs", "session.artifact"),
            ("src/gateway/handlers/runtimes.rs", "runtimes.install.progress"),
        ] {
            assert!(
                has(file, topic),
                "the census missed `{topic}` in {file} — the walk, the scraper or the \
                 constant resolver stopped matching; saw {producers:#?}"
            );
        }
        assert!(
            !producers
                .iter()
                .any(|p| p.file == "src/gateway/server/handler.rs"),
            "server/handler.rs publishes only from its test modules — a hit here means \
             the production cut regressed"
        );
        assert!(
            !producers
                .iter()
                .any(|p| p.file == "src/gateway/source_census.rs"),
            "this file's own scanner literals are test code and must not count"
        );
    }
}
