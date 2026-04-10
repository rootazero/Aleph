//! WikiIndexGenerator — auto-generate index.md from wiki facts.

use std::path::Path;

/// Entry for the wiki index.
#[derive(Debug, Clone)]
pub struct WikiIndexEntry {
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub updated: String,
}

/// Generate the index.md content from a list of wiki index entries.
pub fn generate_index_content(entries: &[WikiIndexEntry]) -> String {
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    let mut lines = Vec::new();

    lines.push("# Wiki Index".to_string());
    lines.push(String::new());
    lines.push("> Auto-generated. Do not edit manually.".to_string());
    lines.push(format!("> Last updated: {}", now));
    lines.push(String::new());
    lines.push(format!("## Pages ({})", entries.len()));
    lines.push(String::new());

    if entries.is_empty() {
        lines.push("_No pages yet._".to_string());
    } else {
        lines.push("| Page | Summary | Tags | Updated |".to_string());
        lines.push("|------|---------|------|---------|".to_string());

        for entry in entries {
            let tags_str = entry.tags.join(", ");
            lines.push(format!(
                "| [{}]({}.md) | {} | {} | {} |",
                entry.title, entry.slug, entry.summary, tags_str, entry.updated
            ));
        }
    }

    lines.push(String::new());
    lines.join("\n")
}

/// Write the index.md file to the agent's wiki directory.
pub fn write_index(agent_dir: &Path, entries: &[WikiIndexEntry]) -> Result<(), String> {
    let content = generate_index_content(entries);
    let index_path = agent_dir.join("index.md");
    std::fs::write(&index_path, content)
        .map_err(|e| format!("Failed to write index.md: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_empty_index() {
        let content = generate_index_content(&[]);
        assert!(content.contains("# Wiki Index"));
        assert!(content.contains("## Pages (0)"));
        assert!(content.contains("_No pages yet._"));
    }

    #[test]
    fn generates_index_with_entries() {
        let entries = vec![
            WikiIndexEntry {
                slug: "rust-ownership".to_string(),
                title: "Rust Ownership".to_string(),
                summary: "Core memory safety".to_string(),
                tags: vec!["rust".to_string(), "memory".to_string()],
                updated: "2026-04-10".to_string(),
            },
            WikiIndexEntry {
                slug: "llm-prompts".to_string(),
                title: "LLM Prompts".to_string(),
                summary: "Prompt engineering best practices".to_string(),
                tags: vec!["llm".to_string()],
                updated: "2026-04-09".to_string(),
            },
        ];
        let content = generate_index_content(&entries);
        assert!(content.contains("## Pages (2)"));
        assert!(content.contains("[Rust Ownership](rust-ownership.md)"));
        assert!(content.contains("rust, memory"));
        assert!(content.contains("[LLM Prompts](llm-prompts.md)"));
    }

    #[test]
    fn writes_index_file() {
        let tmp = tempfile::tempdir().unwrap();
        let entries = vec![WikiIndexEntry {
            slug: "test".to_string(),
            title: "Test Page".to_string(),
            summary: "A test".to_string(),
            tags: vec!["test".to_string()],
            updated: "2026-04-10".to_string(),
        }];
        write_index(tmp.path(), &entries).unwrap();
        let content = std::fs::read_to_string(tmp.path().join("index.md")).unwrap();
        assert!(content.contains("Test Page"));
    }
}
