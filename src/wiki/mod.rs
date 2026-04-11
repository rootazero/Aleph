//! Wiki knowledge system — LLM-maintained, git-tracked Markdown knowledge pages.

pub mod tools;
pub mod wikilink;
pub mod git;
pub mod index;

/// Validate a wiki page slug (kebab-case, non-empty, ASCII lowercase + hyphens + digits).
pub fn is_valid_page_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 128
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !slug.starts_with('-')
        && !slug.ends_with('-')
}

/// Build the aleph:// path for a wiki page.
pub fn wiki_path(agent_id: &str, slug: &str) -> String {
    format!("aleph://wiki/{}/{}.md", agent_id, slug)
}

/// Build the parent path for listing all wiki pages of an agent.
pub fn wiki_parent_path(agent_id: &str) -> String {
    format!("aleph://wiki/{}/", agent_id)
}

/// Build the physical file path for a wiki page.
///
/// Layout: `{config_dir}/memory/note/{agent_id}/wiki/{slug}.md`
///
/// Note: `data_dir` is ignored — we derive the path from `get_note_memory_dir()`.
/// The parameter is kept for backward compatibility with existing call sites.
pub fn wiki_file_path(data_dir: &std::path::Path, agent_id: &str, slug: &str) -> std::path::PathBuf {
    crate::utils::paths::get_note_memory_dir()
        .unwrap_or_else(|_| data_dir.join("memory").join("note"))
        .join(agent_id)
        .join("wiki")
        .join(format!("{}.md", slug))
}

/// Build the old (pre-migration) physical directory for wiki pages.
///
/// Old layout: `{data_dir}/wiki/{agent_id}/`
pub fn wiki_old_dir(data_dir: &std::path::Path, agent_id: &str) -> std::path::PathBuf {
    data_dir.join("wiki").join(agent_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_slugs() {
        assert!(is_valid_page_slug("rust-ownership-model"));
        assert!(is_valid_page_slug("llm-prompt-engineering"));
        assert!(is_valid_page_slug("topic123"));
        assert!(is_valid_page_slug("a"));
    }

    #[test]
    fn invalid_slugs() {
        assert!(!is_valid_page_slug(""));
        assert!(!is_valid_page_slug("Has Spaces"));
        assert!(!is_valid_page_slug("UPPERCASE"));
        assert!(!is_valid_page_slug("-leading-hyphen"));
        assert!(!is_valid_page_slug("trailing-hyphen-"));
        assert!(!is_valid_page_slug("special!chars"));
    }

    #[test]
    fn wiki_path_format() {
        assert_eq!(wiki_path("default", "rust-ownership"), "aleph://wiki/default/rust-ownership.md");
    }

    #[test]
    fn wiki_parent_path_format() {
        assert_eq!(wiki_parent_path("default"), "aleph://wiki/default/");
    }

    #[test]
    fn wiki_file_path_format() {
        let path = wiki_file_path(std::path::Path::new("/home/user/.aleph/data"), "default", "rust-ownership");
        assert_eq!(path, std::path::PathBuf::from("/home/user/.aleph/data/memory/default/wiki/rust-ownership.md"));
    }

    #[test]
    fn wiki_old_dir_format() {
        let path = wiki_old_dir(std::path::Path::new("/home/user/.aleph/data"), "default");
        assert_eq!(path, std::path::PathBuf::from("/home/user/.aleph/data/wiki/default"));
    }
}
