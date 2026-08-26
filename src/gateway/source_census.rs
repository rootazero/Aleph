//! Shared scanners for the gateway's source-level census tests.
//!
//! Several pins in this directory answer a question no compile-anchored test
//! can: what does a producer's *source text* actually publish? Each of them
//! grew its own copy of the same two steps — take the production half of a
//! file, then scrape the topic literals out of it — and the copies had already
//! drifted: `event_visibility`'s form required the quote to sit immediately
//! after the paren, so it scraped 2 of the 4 topics in `server/handler.rs`
//! (rustfmt puts the literal on the next line once the call is long enough).
//! The permissive form kept here is the superset, so a reformatted call site
//! cannot quietly shrink a census.
//!
//! Both helpers implement the repo rules for source scanning: `\r` is stripped
//! before anything is split (a CRLF checkout otherwise matches nothing and the
//! "production half" silently becomes the whole file), the `#[cfg(test)]`
//! separator carries no line anchors, and comment lines are dropped so a doc
//! comment naming a topic can neither satisfy nor break a census.

/// The half of a Rust source file that ships: CRLF-normalized, everything from
/// the first `#[cfg(test)]` onward removed, comment lines dropped.
pub(crate) fn production_prefix(src: &str) -> String {
    crate::utils::source_scan::strip_comment_lines(&crate::utils::source_scan::production_prefix(
        src,
    ))
}

/// Every topic literal in a `TopicEvent::new("…", …)` call in `src`.
///
/// Calls whose first argument is a composed expression (`&topic`,
/// `format!(…)`) have no literal to scrape and are skipped — the callers that
/// need those assert on them directly. Pass [`production_prefix`] output, not
/// the raw file.
pub(crate) fn topic_event_literals(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for seg in src.split("TopicEvent::new(").skip(1) {
        let Some(open) = seg.find('"') else { continue };
        if !seg[..open].chars().all(char::is_whitespace) {
            continue; // composed topic — nothing to scrape
        }
        let rest = &seg[open + 1..];
        let Some(close) = rest.find('"') else {
            continue;
        };
        out.push(rest[..close].to_string());
    }
    out
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
}
