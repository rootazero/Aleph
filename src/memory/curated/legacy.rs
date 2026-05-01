//! Backward-tolerant read for legacy MEMORY.md files (no `§` delimiters).
//!
//! Strategy (per spec D2): if the file has no `§` markers and is non-empty,
//! treat the entire body as a single `legacy` entry. Empty/whitespace-only
//! → zero entries. Spec acceptance: legacy entries are read-only via `add`
//! (rejected when over budget) but `replace` / `remove` may be used to
//! shrink them.

use super::format::ENTRY_DELIMITER;

#[derive(Debug, Clone)]
pub struct ParsedLoad {
    pub entries: Vec<String>,
    pub legacy: bool,
}

pub fn load_body(body: &str) -> ParsedLoad {
    if body.trim().is_empty() {
        return ParsedLoad { entries: Vec::new(), legacy: false };
    }
    if body.contains(ENTRY_DELIMITER) {
        let entries = super::format::parse(body);
        return ParsedLoad { entries, legacy: false };
    }
    // No delimiter → legacy free-form file → single entry.
    ParsedLoad {
        entries: vec![body.trim().to_string()],
        legacy: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_is_not_legacy() {
        let p = load_body("");
        assert!(p.entries.is_empty());
        assert!(!p.legacy);
    }

    #[test]
    fn whitespace_only_is_not_legacy() {
        let p = load_body("\n  \n\t");
        assert!(p.entries.is_empty());
        assert!(!p.legacy);
    }

    #[test]
    fn file_with_delimiter_is_modern() {
        let body = "fact one\n§\nfact two";
        let p = load_body(body);
        assert!(!p.legacy);
        assert_eq!(p.entries.len(), 2);
    }

    #[test]
    fn free_markdown_is_legacy_single_entry() {
        let body = "# MEMORY.md\n## Notes\n- prefer concise replies\n- linux mint host";
        let p = load_body(body);
        assert!(p.legacy);
        assert_eq!(p.entries.len(), 1);
        assert!(p.entries[0].contains("MEMORY.md"));
    }
}
