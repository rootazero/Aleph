//! Parse/serialize the note frontmatter `keywords:` field.
//!
//! Format mirrors the existing `tags:` line: `keywords: [a, b, c]`. Values are
//! lowercase kebab/plain tokens; commas separate, surrounding whitespace and
//! brackets are stripped. Empty list serializes as `keywords: []`.

/// Extract the keyword list from a frontmatter string. Returns empty when the
/// `keywords:` line is absent or empty.
pub fn parse_keywords(frontmatter: &str) -> Vec<String> {
    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("keywords:") {
            let inner = rest.trim().trim_start_matches('[').trim_end_matches(']');
            return inner
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
        }
    }
    Vec::new()
}

/// Render a `keywords: [a, b]` frontmatter line (no trailing newline).
pub fn serialize_keywords(keywords: &[String]) -> String {
    format!("keywords: [{}]", keywords.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_keywords_line() {
        let fm =
            "category: entity\nkeywords: [us-iran-conflict, monitoring, ceasefire]\ntags: []\n";
        assert_eq!(
            parse_keywords(fm),
            vec!["us-iran-conflict", "monitoring", "ceasefire"]
        );
    }

    #[test]
    fn missing_keywords_is_empty() {
        assert!(parse_keywords("category: entity\ntags: []\n").is_empty());
    }

    #[test]
    fn serialize_round_trips() {
        let kw = vec!["a".to_string(), "b-c".to_string()];
        let line = serialize_keywords(&kw);
        assert_eq!(line, "keywords: [a, b-c]");
        assert_eq!(parse_keywords(&format!("{line}\n")), kw);
    }

    #[test]
    fn serialize_empty_is_empty_brackets() {
        assert_eq!(serialize_keywords(&[]), "keywords: []");
    }
}
