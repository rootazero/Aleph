//! § entry serialization for curated memory files.
//!
//! Format: entries separated by `\n§\n` (newline, section sign, newline). Empty
//! file = zero entries. Multiline entries are preserved.

pub const ENTRY_DELIMITER: &str = "\n§\n";

/// Parse a raw file body into entries. Trims surrounding whitespace per entry,
/// drops empty entries.
pub fn parse(body: &str) -> Vec<String> {
    if body.trim().is_empty() {
        return Vec::new();
    }
    body.split(ENTRY_DELIMITER)
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty())
        .collect()
}

/// Serialize entries into a § -separated body. Empty input → empty string.
pub fn serialize(entries: &[String]) -> String {
    entries.join(ENTRY_DELIMITER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_empty_body_as_zero_entries() {
        assert!(parse("").is_empty());
        assert!(parse("\n  \n").is_empty());
    }

    #[test]
    fn parses_single_entry_without_delimiter() {
        let entries = parse("just one fact");
        assert_eq!(entries, vec!["just one fact".to_string()]);
    }

    #[test]
    fn parses_three_entries() {
        let body = "fact one\n§\nfact two\n§\nfact three";
        assert_eq!(
            parse(body),
            vec!["fact one", "fact two", "fact three"]
                .into_iter().map(String::from).collect::<Vec<_>>()
        );
    }

    #[test]
    fn preserves_multiline_entry_content() {
        let body = "line a\nline b\n§\nentry two";
        assert_eq!(parse(body), vec!["line a\nline b", "entry two"]);
    }

    #[test]
    fn entry_containing_lone_section_sign_not_split() {
        // Only "\n§\n" splits. Lone "§" inside content survives.
        let body = "see § symbol used\n§\nnext entry";
        assert_eq!(parse(body), vec!["see § symbol used", "next entry"]);
    }

    #[test]
    fn serialize_round_trips() {
        let entries: Vec<String> = vec!["a".into(), "multiline\nb".into(), "c".into()];
        let body = serialize(&entries);
        assert_eq!(parse(&body), entries);
    }

    #[test]
    fn serialize_empty_returns_empty_string() {
        assert_eq!(serialize(&[]), "");
    }
}
