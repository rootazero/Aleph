//! @mention extraction for team chat content.
//!
//! Pure parser — takes message content and returns the set of mentioned
//! identifiers. Callers (e.g. `MessageSendTool`) decide how to map mentions
//! onto routing (typically: union with `to` after dropping unknown handles
//! and the sender's own id).
//!
//! ## Grammar
//!
//! - `@<id>` where `<id>` is one or more of `[A-Za-z0-9_-]` — produces the
//!   mention `<id>`.
//! - `@all` and `@everyone` produce the sentinel [`MENTION_ALL`]. Callers
//!   interpret it as a broadcast (fan-out to every team member except the
//!   sender).
//! - The `@` must be preceded by a non-identifier character (or start of
//!   input). This avoids picking up email addresses and substrings like
//!   `you@home`.
//! - A leading backslash escapes the `@` (`\@alice` → no mention).
//! - Inside fenced code blocks (lines starting with ```` ``` ````) and
//!   inline-code spans (`` ` ... ` ``), `@` is treated as literal text.
//!
//! ## Output
//!
//! Mentions are returned in document order with duplicates removed. The
//! `MENTION_ALL` sentinel, if present, always appears at most once and may
//! appear anywhere in the order — callers should branch on its presence
//! rather than on position.

use std::collections::HashSet;

/// Sentinel returned by [`extract_mentions`] when the content addresses the
/// entire team (`@all` or `@everyone`). Callers should treat this as a
/// broadcast hint rather than a literal recipient id.
pub const MENTION_ALL: &str = "*all*";

/// Extract `@mentions` from message content.
///
/// See the module docs for the full grammar. Returns identifiers in document
/// order with duplicates removed.
pub fn extract_mentions(content: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut push = |id: String| {
        if seen.insert(id.clone()) {
            out.push(id);
        }
    };

    // Process line-by-line so we can detect fenced code blocks (```).
    let mut in_fenced = false;
    for line in content.split('\n') {
        let trimmed = line.trim_start();
        // Toggle fence on a line that starts with three or more backticks.
        if trimmed.starts_with("```") {
            in_fenced = !in_fenced;
            continue;
        }
        if in_fenced {
            continue;
        }
        scan_line(line, &mut push);
    }

    out
}

/// Scan a single non-fenced line, accounting for inline code spans and
/// backslash escapes.
fn scan_line(line: &str, push: &mut impl FnMut(String)) {
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut in_inline_code = false;
    // Tracks whether the preceding visible character was an identifier char,
    // so we can require the `@` to be at a word boundary. Start-of-line
    // counts as a boundary.
    let mut prev_is_ident = false;

    while i < bytes.len() {
        let c = bytes[i];
        // Inline-code toggle — backtick flips state, treat content as literal.
        if c == b'`' {
            in_inline_code = !in_inline_code;
            prev_is_ident = false;
            i += 1;
            continue;
        }
        if in_inline_code {
            // Literal text inside a code span. Update boundary tracker just
            // in case the span ends mid-line and an `@` follows immediately.
            prev_is_ident = is_ident_byte(c);
            i += 1;
            continue;
        }
        // Backslash escapes the next character — skip both.
        if c == b'\\' && i + 1 < bytes.len() {
            prev_is_ident = is_ident_byte(bytes[i + 1]);
            i += 2;
            continue;
        }
        if c == b'@' && !prev_is_ident {
            // Read the following identifier.
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && is_ident_byte(bytes[j]) {
                j += 1;
            }
            if j > start {
                let raw = &line[start..j];
                let normalised = match raw {
                    "all" | "everyone" => MENTION_ALL.to_string(),
                    other => other.to_string(),
                };
                push(normalised);
                prev_is_ident = true;
                i = j;
                continue;
            }
            // `@` not followed by an identifier — treat as literal text.
            prev_is_ident = false;
            i += 1;
            continue;
        }
        prev_is_ident = is_ident_byte(c);
        i += 1;
    }
}

#[inline]
fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_mention() {
        assert_eq!(extract_mentions("hi @alice please review"), vec!["alice"]);
    }

    #[test]
    fn multiple_mentions_in_order() {
        let v = extract_mentions("@bob and @carol — see @bob again");
        assert_eq!(v, vec!["bob", "carol"]);
    }

    #[test]
    fn at_all_normalises_to_sentinel() {
        assert_eq!(extract_mentions("@all heads up"), vec![MENTION_ALL]);
        assert_eq!(extract_mentions("hey @everyone"), vec![MENTION_ALL]);
    }

    #[test]
    fn dedups_all_and_everyone_into_one_sentinel() {
        assert_eq!(
            extract_mentions("@all @everyone @all"),
            vec![MENTION_ALL]
        );
    }

    #[test]
    fn ignores_email_address() {
        // The `@` is preceded by an identifier char, so this is not a mention.
        assert!(extract_mentions("contact me at alice@example.com").is_empty());
    }

    #[test]
    fn backslash_escape_prevents_mention() {
        assert!(extract_mentions("literal \\@alice text").is_empty());
    }

    #[test]
    fn ignores_inline_code_span() {
        assert!(extract_mentions("look at `@alice` syntax").is_empty());
    }

    #[test]
    fn ignores_fenced_code_block() {
        let s = "before\n```\n@alice in code\n```\nafter @bob";
        assert_eq!(extract_mentions(s), vec!["bob"]);
    }

    #[test]
    fn mentions_in_inline_code_neighbour_still_picked_up() {
        // Once we leave the inline span, a fresh mention is fine.
        assert_eq!(
            extract_mentions("see `code` then @dave please"),
            vec!["dave"]
        );
    }

    #[test]
    fn supports_underscore_and_hyphen_in_id() {
        assert_eq!(
            extract_mentions("@review-bot @ops_lead"),
            vec!["review-bot", "ops_lead"]
        );
    }

    #[test]
    fn bare_at_is_not_a_mention() {
        assert!(extract_mentions("price is @ $5").is_empty());
    }

    #[test]
    fn at_at_start_of_line_works() {
        assert_eq!(extract_mentions("@alice"), vec!["alice"]);
    }

    #[test]
    fn empty_input() {
        assert!(extract_mentions("").is_empty());
    }
}
