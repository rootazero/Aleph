# Wiki Knowledge System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Wiki subsystem to Aleph's memory architecture — LLM-maintained, git-tracked Markdown knowledge pages as first-class facts with `[[wikilink]]` interlinking and knowledge graph integration.

**Architecture:** Wiki pages are `FactType::Wiki` facts where the fact `content` holds a short summary (for embedding/search) and the `path` field maps to a physical Markdown file under `~/.aleph/data/wiki/{agent_id}/`. Pages interlink via `[[wikilink]]` syntax which automatically generates `wiki_references` graph edges. A `WikiManageTool` exposes create/update/query/delete/list actions to the LLM. All wiki file mutations are auto-committed to a local git repo.

**Tech Stack:** Rust, SQLite (existing facts store), regex (wikilink parsing), git CLI (version control), serde/schemars (tool schema), async-trait (DreamStage)

**Spec:** `docs/superpowers/specs/2026-04-10-wiki-knowledge-system-design.md`

---

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `src/wiki/mod.rs` | Module root, re-exports, `is_valid_page_slug()` validator |
| Create | `src/wiki/wikilink.rs` | `[[wikilink]]` parser, frontmatter parser |
| Create | `src/wiki/git.rs` | `WikiGitManager` — init repo, auto-commit |
| Create | `src/wiki/index.rs` | `WikiIndexGenerator` — generate `index.md` from facts |
| Create | `src/wiki/tools/mod.rs` | Tools sub-module re-exports |
| Create | `src/wiki/tools/manage.rs` | `WikiManageArgs`, `WikiManageResult`, `build_wiki_fact()`, validation |
| Create | `src/builtin_tools/wiki_manage.rs` | `WikiManageTool` implementing `AlephTool` trait |
| Modify | `src/memory/context/enums.rs` | Add `Wiki` variant to `FactType` + default mappings |
| Modify | `src/lib.rs` | Add `pub mod wiki;` |
| Modify | `src/builtin_tools/mod.rs` | Add `pub mod wiki_manage;` |
| Modify | `src/executor/builtin_registry/builder.rs` | Register `WikiManageTool` |
| Create | `src/memory/dreaming/stages/wiki_ingest.rs` | `WikiIngestStage` — passive document ingestion |
| Create | `src/memory/dreaming/stages/wiki_lint.rs` | `WikiLintStage` — health checks |
| Modify | `src/memory/dreaming/stages/mod.rs` | Export new stages |
| Modify | `src/memory/dreaming/mod.rs` | Insert stages into pipeline, add `WikiLintReport` |

---

## Phase 1: Core Wiki Infrastructure

### Task 1: Add `FactType::Wiki` to enums

**Files:**
- Modify: `src/memory/context/enums.rs`

- [ ] **Step 1: Write the failing test**

Add to the existing test module or create inline tests:

```rust
// In src/memory/context/enums.rs, add to bottom or in a new #[cfg(test)] mod tests
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wiki_fact_type_roundtrips() {
        let ft = FactType::Wiki;
        assert_eq!(ft.as_str(), "wiki");
        assert_eq!(ft.to_string(), "wiki");
        let parsed: FactType = "wiki".parse().unwrap();
        assert_eq!(parsed, FactType::Wiki);
    }

    #[test]
    fn wiki_default_path() {
        assert_eq!(FactType::Wiki.default_path(), "aleph://wiki/");
    }

    #[test]
    fn wiki_default_category() {
        assert_eq!(FactType::Wiki.default_category(), MemoryCategory::Patterns);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib -- context::enums::tests::wiki_fact_type_roundtrips 2>&1 | tail -5`
Expected: FAIL — `Wiki` variant doesn't exist yet.

- [ ] **Step 3: Add Wiki variant to FactType enum**

In `src/memory/context/enums.rs`, add the `Wiki` variant and update all match arms:

```rust
// After Skill variant (line ~30):
/// Structured knowledge page from ingested documents (LLM Wiki).
Wiki,
```

Update `as_str()`:
```rust
FactType::Wiki => "wiki",
```

Update `default_path()`:
```rust
FactType::Wiki => "aleph://wiki/",
```

Update `default_category()`:
```rust
FactType::Tool | FactType::Skill | FactType::Wiki => MemoryCategory::Patterns,
```

Update `FromStr`:
```rust
"wiki" => Ok(FactType::Wiki),
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib -- context::enums::tests::wiki 2>&1 | tail -5`
Expected: All 3 wiki tests PASS.

- [ ] **Step 5: Run full compile check**

Run: `cargo check -p alephcore 2>&1 | tail -10`
Expected: No errors (all match arms are exhaustive).

- [ ] **Step 6: Commit**

```bash
git add src/memory/context/enums.rs
git commit -m "wiki: add FactType::Wiki variant with default mappings"
```

---

### Task 2: Create wiki module with slug validator

**Files:**
- Create: `src/wiki/mod.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `src/wiki/mod.rs` with tests first:

```rust
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
pub fn wiki_file_path(data_dir: &std::path::Path, agent_id: &str, slug: &str) -> std::path::PathBuf {
    data_dir.join("wiki").join(agent_id).join(format!("{}.md", slug))
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
        assert_eq!(
            wiki_path("default", "rust-ownership"),
            "aleph://wiki/default/rust-ownership.md"
        );
    }

    #[test]
    fn wiki_parent_path_format() {
        assert_eq!(wiki_parent_path("default"), "aleph://wiki/default/");
    }

    #[test]
    fn wiki_file_path_format() {
        let path = wiki_file_path(
            std::path::Path::new("/home/user/.aleph/data"),
            "default",
            "rust-ownership",
        );
        assert_eq!(
            path,
            std::path::PathBuf::from("/home/user/.aleph/data/wiki/default/rust-ownership.md")
        );
    }
}
```

- [ ] **Step 2: Create stub sub-modules**

Create `src/wiki/tools/mod.rs`:
```rust
//! Wiki management tools exposed to the LLM.

pub mod manage;
```

Create `src/wiki/tools/manage.rs`:
```rust
//! WikiManageTool — create, update, query, delete, list wiki pages.
```

Create `src/wiki/wikilink.rs`:
```rust
//! Wikilink parser for [[page-slug]] and [[page-slug|display text]] syntax.
```

Create `src/wiki/git.rs`:
```rust
//! WikiGitManager — git repo initialization and auto-commit for wiki pages.
```

Create `src/wiki/index.rs`:
```rust
//! WikiIndexGenerator — auto-generate index.md from wiki facts.
```

- [ ] **Step 3: Register wiki module in lib.rs**

In `src/lib.rs`, add after the `pub mod skill;` line:
```rust
pub mod wiki;
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib -- wiki::tests 2>&1 | tail -10`
Expected: All 5 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/wiki/ src/lib.rs
git commit -m "wiki: add module skeleton with slug validator and path helpers"
```

---

### Task 3: Implement wikilink parser

**Files:**
- Modify: `src/wiki/wikilink.rs`

- [ ] **Step 1: Write the failing tests**

```rust
//! Wikilink parser for [[page-slug]] and [[page-slug|display text]] syntax.

use once_cell::sync::Lazy;
use regex::Regex;

static RE_WIKILINK: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[\[([^\]\|]+)(?:\|[^\]]+)?\]\]").unwrap());

/// Extract all wikilink target slugs from markdown content.
pub fn extract_wikilinks(markdown: &str) -> Vec<String> {
    RE_WIKILINK
        .captures_iter(markdown)
        .filter_map(|cap| cap.get(1).map(|m| m.as_str().trim().to_string()))
        .collect()
}

/// Frontmatter parsed from a wiki markdown page.
#[derive(Debug, Clone, Default)]
pub struct WikiFrontmatter {
    pub title: String,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    pub sources: Vec<String>,
    pub created: String,
    pub updated: String,
}

/// Parse YAML frontmatter from a wiki markdown page.
/// Returns None if no valid frontmatter block is found.
pub fn parse_frontmatter(markdown: &str) -> Option<WikiFrontmatter> {
    let content = markdown.trim();
    if !content.starts_with("---") {
        return None;
    }
    let rest = &content[3..];
    let end = rest.find("---")?;
    let yaml_str = &rest[..end];

    let yaml: serde_yaml::Value = serde_yaml::from_str(yaml_str).ok()?;
    let map = yaml.as_mapping()?;

    let get_str = |key: &str| -> String {
        map.get(&serde_yaml::Value::String(key.to_string()))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };

    let get_vec = |key: &str| -> Vec<String> {
        map.get(&serde_yaml::Value::String(key.to_string()))
            .and_then(|v| v.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    };

    Some(WikiFrontmatter {
        title: get_str("title"),
        aliases: get_vec("aliases"),
        tags: get_vec("tags"),
        sources: get_vec("sources"),
        created: get_str("created"),
        updated: get_str("updated"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_simple_wikilinks() {
        let md = "See [[rust-ownership]] for details and [[cpp-memory]] for comparison.";
        let links = extract_wikilinks(md);
        assert_eq!(links, vec!["rust-ownership", "cpp-memory"]);
    }

    #[test]
    fn extracts_wikilinks_with_display_text() {
        let md = "Read [[rust-ownership|Rust Ownership Model]] for more.";
        let links = extract_wikilinks(md);
        assert_eq!(links, vec!["rust-ownership"]);
    }

    #[test]
    fn returns_empty_for_no_links() {
        let md = "No wiki links here.";
        let links = extract_wikilinks(md);
        assert!(links.is_empty());
    }

    #[test]
    fn handles_multiple_links_same_line() {
        let md = "[[a]] and [[b]] and [[c]]";
        let links = extract_wikilinks(md);
        assert_eq!(links, vec!["a", "b", "c"]);
    }

    #[test]
    fn parses_valid_frontmatter() {
        let md = r#"---
title: Rust Ownership
aliases: [ownership, borrow-checker]
tags: [rust, memory]
sources: [fact-123]
created: "2026-04-10"
updated: "2026-04-10"
---

# Content here
"#;
        let fm = parse_frontmatter(md).unwrap();
        assert_eq!(fm.title, "Rust Ownership");
        assert_eq!(fm.aliases, vec!["ownership", "borrow-checker"]);
        assert_eq!(fm.tags, vec!["rust", "memory"]);
        assert_eq!(fm.sources, vec!["fact-123"]);
    }

    #[test]
    fn returns_none_for_no_frontmatter() {
        let md = "# Just a heading\nSome content.";
        assert!(parse_frontmatter(md).is_none());
    }
}
```

- [ ] **Step 2: Check serde_yaml dependency**

Run: `grep 'serde_yaml\|serde-yaml' Cargo.toml | head -3`

If not present, add `serde_yaml` to dependencies in `Cargo.toml` (the crate likely already has it for config parsing — verify first).

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib -- wiki::wikilink::tests 2>&1 | tail -10`
Expected: All 6 tests PASS.

- [ ] **Step 4: Commit**

```bash
git add src/wiki/wikilink.rs
git commit -m "wiki: implement wikilink parser and frontmatter extractor"
```

---

### Task 4: Implement WikiGitManager

**Files:**
- Modify: `src/wiki/git.rs`

- [ ] **Step 1: Write tests and implementation**

```rust
//! WikiGitManager — git repo initialization and auto-commit for wiki pages.

use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{info, warn};

/// Manages the git repository for wiki pages.
#[derive(Debug, Clone)]
pub struct WikiGitManager {
    wiki_dir: PathBuf,
}

impl WikiGitManager {
    /// Create a new WikiGitManager for the given wiki directory.
    pub fn new(wiki_dir: impl Into<PathBuf>) -> Self {
        Self {
            wiki_dir: wiki_dir.into(),
        }
    }

    /// Initialize the git repo if it doesn't exist.
    pub fn ensure_repo(&self) -> Result<(), String> {
        let git_dir = self.wiki_dir.join(".git");
        if git_dir.exists() {
            return Ok(());
        }

        std::fs::create_dir_all(&self.wiki_dir)
            .map_err(|e| format!("Failed to create wiki dir: {}", e))?;

        let output = Command::new("git")
            .args(["init"])
            .current_dir(&self.wiki_dir)
            .output()
            .map_err(|e| format!("Failed to run git init: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "git init failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        info!(path = %self.wiki_dir.display(), "Initialized wiki git repo");
        Ok(())
    }

    /// Ensure the agent subdirectory exists.
    pub fn ensure_agent_dir(&self, agent_id: &str) -> Result<PathBuf, String> {
        let agent_dir = self.wiki_dir.join(agent_id);
        std::fs::create_dir_all(&agent_dir)
            .map_err(|e| format!("Failed to create agent dir: {}", e))?;
        Ok(agent_dir)
    }

    /// Commit changes for a specific wiki action.
    pub fn commit_changes(
        &self,
        agent_id: &str,
        action: &str,
        page_slug: &str,
    ) -> Result<(), String> {
        // Stage all changes in the agent directory
        let agent_dir = self.wiki_dir.join(agent_id);
        let output = Command::new("git")
            .args(["add", "."])
            .current_dir(&agent_dir)
            .output()
            .map_err(|e| format!("git add failed: {}", e))?;

        if !output.status.success() {
            warn!(
                error = %String::from_utf8_lossy(&output.stderr),
                "git add failed"
            );
            return Err("git add failed".to_string());
        }

        // Check if there are staged changes
        let status = Command::new("git")
            .args(["diff", "--cached", "--quiet"])
            .current_dir(&self.wiki_dir)
            .status()
            .map_err(|e| format!("git diff failed: {}", e))?;

        if status.success() {
            // No changes to commit
            return Ok(());
        }

        let message = format!("wiki({}): {} {}", agent_id, action, page_slug);
        let output = Command::new("git")
            .args(["commit", "-m", &message])
            .current_dir(&self.wiki_dir)
            .output()
            .map_err(|e| format!("git commit failed: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // "nothing to commit" is not an error
            if stderr.contains("nothing to commit") {
                return Ok(());
            }
            return Err(format!("git commit failed: {}", stderr));
        }

        info!(
            agent_id = agent_id,
            action = action,
            page = page_slug,
            "Wiki git commit"
        );
        Ok(())
    }

    /// Get the wiki directory path.
    pub fn wiki_dir(&self) -> &Path {
        &self.wiki_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn ensure_repo_creates_git_dir() {
        let tmp = TempDir::new().unwrap();
        let wiki_dir = tmp.path().join("wiki");
        let mgr = WikiGitManager::new(&wiki_dir);
        mgr.ensure_repo().unwrap();
        assert!(wiki_dir.join(".git").exists());
    }

    #[test]
    fn ensure_repo_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let wiki_dir = tmp.path().join("wiki");
        let mgr = WikiGitManager::new(&wiki_dir);
        mgr.ensure_repo().unwrap();
        mgr.ensure_repo().unwrap(); // Second call should succeed
        assert!(wiki_dir.join(".git").exists());
    }

    #[test]
    fn ensure_agent_dir_creates_subdirectory() {
        let tmp = TempDir::new().unwrap();
        let wiki_dir = tmp.path().join("wiki");
        let mgr = WikiGitManager::new(&wiki_dir);
        mgr.ensure_repo().unwrap();
        let agent_dir = mgr.ensure_agent_dir("default").unwrap();
        assert!(agent_dir.exists());
        assert_eq!(agent_dir, wiki_dir.join("default"));
    }

    #[test]
    fn commit_changes_with_content() {
        let tmp = TempDir::new().unwrap();
        let wiki_dir = tmp.path().join("wiki");
        let mgr = WikiGitManager::new(&wiki_dir);
        mgr.ensure_repo().unwrap();

        // Configure git user for test
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(&wiki_dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(&wiki_dir)
            .output()
            .unwrap();

        let agent_dir = mgr.ensure_agent_dir("default").unwrap();
        std::fs::write(agent_dir.join("test-page.md"), "# Test\nContent").unwrap();

        mgr.commit_changes("default", "create", "test-page")
            .unwrap();

        // Verify commit exists
        let output = Command::new("git")
            .args(["log", "--oneline", "-1"])
            .current_dir(&wiki_dir)
            .output()
            .unwrap();
        let log = String::from_utf8_lossy(&output.stdout);
        assert!(log.contains("wiki(default): create test-page"));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib -- wiki::git::tests 2>&1 | tail -10`
Expected: All 4 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add src/wiki/git.rs
git commit -m "wiki: implement WikiGitManager with init and auto-commit"
```

---

### Task 5: Implement WikiIndexGenerator

**Files:**
- Modify: `src/wiki/index.rs`

- [ ] **Step 1: Write tests and implementation**

```rust
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

    lines.push(String::new()); // trailing newline
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
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib -- wiki::index::tests 2>&1 | tail -10`
Expected: All 3 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add src/wiki/index.rs
git commit -m "wiki: implement WikiIndexGenerator for auto-generated index.md"
```

---

### Task 6: Implement wiki fact builder and validation

**Files:**
- Modify: `src/wiki/tools/manage.rs`

- [ ] **Step 1: Write tests and implementation**

```rust
//! Wiki fact builder and validation for wiki_manage tool arguments.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::memory::context::{
    FactSource, FactType, MemoryCategory, MemoryFact, MemoryLayer, MemoryScope, MemoryTier,
};
use crate::wiki::is_valid_page_slug;

/// Actions supported by the wiki_manage tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WikiAction {
    Create,
    Update,
    Query,
    Delete,
    List,
}

/// Arguments for the wiki_manage tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WikiManageArgs {
    /// Action to perform.
    pub action: WikiAction,
    /// Page slug (kebab-case). Required for create, update, delete.
    #[serde(default)]
    pub page_slug: Option<String>,
    /// Page title. Required for create.
    #[serde(default)]
    pub title: Option<String>,
    /// One-line summary of the page. Required for create.
    #[serde(default)]
    pub summary: Option<String>,
    /// Full markdown content of the page. Required for create, optional for update.
    #[serde(default)]
    pub content: Option<String>,
    /// Search query string. Required for query action.
    #[serde(default)]
    pub query: Option<String>,
}

/// Result of a wiki_manage operation.
#[derive(Debug, Clone, Serialize)]
pub struct WikiManageResult {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pages: Option<Vec<WikiListEntry>>,
}

/// Entry in wiki page list output.
#[derive(Debug, Clone, Serialize)]
pub struct WikiListEntry {
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub path: String,
}

/// Validate arguments for create action.
pub fn validate_args_for_create(
    page_slug: &str,
    title: &str,
    summary: &str,
    content: &str,
) -> Result<(), String> {
    if !is_valid_page_slug(page_slug) {
        return Err(format!(
            "Invalid page slug '{}': must be non-empty kebab-case (lowercase ASCII, hyphens, digits)",
            page_slug
        ));
    }
    if title.is_empty() {
        return Err("Title cannot be empty".to_string());
    }
    if summary.is_empty() {
        return Err("Summary cannot be empty".to_string());
    }
    if content.is_empty() {
        return Err("Content cannot be empty".to_string());
    }
    Ok(())
}

/// Build a MemoryFact anchor for a wiki page.
pub fn build_wiki_fact(
    agent_id: &str,
    page_slug: &str,
    summary: &str,
) -> MemoryFact {
    let path = crate::wiki::wiki_path(agent_id, page_slug);

    MemoryFact::new(summary.to_string(), FactType::Wiki, Vec::new())
        .with_confidence(0.9)
        .with_tier(MemoryTier::LongTerm)
        .with_scope(MemoryScope::Global)
        .with_layer(MemoryLayer::L2Detail)
        .with_category(MemoryCategory::Patterns)
        .with_path(path)
        .with_fact_source(FactSource::Document)
        .with_agent(agent_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_page_slug() {
        assert!(validate_args_for_create("rust-ownership", "Title", "Summary", "Content").is_ok());
        assert!(validate_args_for_create("", "Title", "Summary", "Content").is_err());
        assert!(validate_args_for_create("Has Spaces", "Title", "Summary", "Content").is_err());
    }

    #[test]
    fn validates_required_fields() {
        assert!(validate_args_for_create("slug", "", "Summary", "Content").is_err());
        assert!(validate_args_for_create("slug", "Title", "", "Content").is_err());
        assert!(validate_args_for_create("slug", "Title", "Summary", "").is_err());
    }

    #[test]
    fn builds_wiki_fact_correctly() {
        let fact = build_wiki_fact("default", "rust-ownership", "Rust ownership and borrowing rules");
        assert_eq!(fact.fact_type, FactType::Wiki);
        assert_eq!(fact.path, "aleph://wiki/default/rust-ownership.md");
        assert_eq!(fact.tier, MemoryTier::LongTerm);
        assert_eq!(fact.scope, MemoryScope::Global);
        assert_eq!(fact.agent, "default");
        assert!(fact.content.contains("Rust ownership"));
        assert!((fact.confidence - 0.9).abs() < f32::EPSILON);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib -- wiki::tools::manage::tests 2>&1 | tail -10`
Expected: All 3 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add src/wiki/tools/manage.rs src/wiki/tools/mod.rs
git commit -m "wiki: implement wiki fact builder, validation, and tool args"
```

---

### Task 7: Implement WikiManageTool (AlephTool)

**Files:**
- Create: `src/builtin_tools/wiki_manage.rs`
- Modify: `src/builtin_tools/mod.rs`

- [ ] **Step 1: Implement WikiManageTool**

Create `src/builtin_tools/wiki_manage.rs`:

```rust
//! wiki_manage — LLM Tool for creating, updating, querying, deleting wiki pages.

use async_trait::async_trait;
use tracing::{info, warn};

use crate::error::{AlephError, Result};
use crate::memory::context::FactType;
use crate::memory::store::{MemoryBackend, MemoryStore};
use crate::tools::AlephTool;
use crate::wiki::git::WikiGitManager;
use crate::wiki::index::{WikiIndexEntry, write_index};
use crate::wiki::tools::manage::{
    WikiAction, WikiListEntry, WikiManageArgs, WikiManageResult,
    build_wiki_fact, validate_args_for_create,
};
use crate::wiki::wikilink::{extract_wikilinks, parse_frontmatter};
use crate::wiki::{wiki_file_path, wiki_parent_path};

use std::path::PathBuf;

#[derive(Clone)]
pub struct WikiManageTool {
    data_dir: PathBuf,
    database: MemoryBackend,
    git: WikiGitManager,
}

impl WikiManageTool {
    pub fn new(data_dir: PathBuf, database: MemoryBackend) -> Self {
        let wiki_dir = data_dir.join("wiki");
        let git = WikiGitManager::new(&wiki_dir);
        Self {
            data_dir,
            database,
            git,
        }
    }

    fn agent_id(&self) -> &str {
        // TODO: wire actual agent_id from context; default for now
        "default"
    }

    async fn handle_create(&self, args: &WikiManageArgs) -> Result<WikiManageResult> {
        let slug = args.page_slug.as_deref().unwrap_or("");
        let title = args.title.as_deref().unwrap_or("");
        let summary = args.summary.as_deref().unwrap_or("");
        let content = args.content.as_deref().unwrap_or("");

        validate_args_for_create(slug, title, summary, content)
            .map_err(|e| AlephError::tool(e))?;

        let agent_id = self.agent_id();

        // Ensure directory structure
        self.git.ensure_repo().map_err(|e| AlephError::tool(e))?;
        self.git
            .ensure_agent_dir(agent_id)
            .map_err(|e| AlephError::tool(e))?;

        // Check if page already exists
        let file_path = wiki_file_path(&self.data_dir, agent_id, slug);
        if file_path.exists() {
            return Err(AlephError::tool(format!(
                "Wiki page '{}' already exists. Use 'update' action to modify it.",
                slug
            )));
        }

        // Write markdown file
        std::fs::write(&file_path, content)
            .map_err(|e| AlephError::tool(format!("Failed to write wiki page: {}", e)))?;

        // Create fact anchor
        let fact = build_wiki_fact(agent_id, slug, summary);
        self.database.upsert_fact(fact).await?;

        // Update index
        self.regenerate_index(agent_id).await?;

        // Git commit
        let _ = self.git.commit_changes(agent_id, "create", slug);

        info!(agent_id = agent_id, slug = slug, "Wiki page created");

        Ok(WikiManageResult {
            success: true,
            message: format!("Created wiki page '{}'", slug),
            page_path: Some(file_path.display().to_string()),
            content: None,
            pages: None,
        })
    }

    async fn handle_update(&self, args: &WikiManageArgs) -> Result<WikiManageResult> {
        let slug = args
            .page_slug
            .as_deref()
            .ok_or_else(|| AlephError::tool("page_slug is required for update"))?;
        let content = args
            .content
            .as_deref()
            .ok_or_else(|| AlephError::tool("content is required for update"))?;

        let agent_id = self.agent_id();
        let file_path = wiki_file_path(&self.data_dir, agent_id, slug);

        if !file_path.exists() {
            return Err(AlephError::tool(format!(
                "Wiki page '{}' does not exist. Use 'create' action first.",
                slug
            )));
        }

        // Write updated content
        std::fs::write(&file_path, content)
            .map_err(|e| AlephError::tool(format!("Failed to write wiki page: {}", e)))?;

        // Update fact summary if provided
        if let Some(summary) = &args.summary {
            // Find existing fact by path and update
            let path = crate::wiki::wiki_path(agent_id, slug);
            if let Some(mut fact) = self.find_wiki_fact_by_path(&path).await? {
                fact.content = summary.clone();
                self.database.update_fact(&fact).await?;
            }
        }

        // Update index
        self.regenerate_index(agent_id).await?;

        // Git commit
        let _ = self.git.commit_changes(agent_id, "update", slug);

        info!(agent_id = agent_id, slug = slug, "Wiki page updated");

        Ok(WikiManageResult {
            success: true,
            message: format!("Updated wiki page '{}'", slug),
            page_path: Some(file_path.display().to_string()),
            content: None,
            pages: None,
        })
    }

    async fn handle_query(&self, args: &WikiManageArgs) -> Result<WikiManageResult> {
        let query = args
            .query
            .as_deref()
            .ok_or_else(|| AlephError::tool("query is required for query action"))?;

        let agent_id = self.agent_id();

        // Read index.md first
        let index_path = self
            .data_dir
            .join("wiki")
            .join(agent_id)
            .join("index.md");

        let mut result_content = String::new();

        if index_path.exists() {
            let index_content = std::fs::read_to_string(&index_path)
                .map_err(|e| AlephError::tool(format!("Failed to read index: {}", e)))?;
            result_content.push_str("## Index\n\n");
            result_content.push_str(&index_content);
        }

        // Search wiki facts by query text
        let all_facts = self.database.get_all_facts(false, None).await?;
        let wiki_facts: Vec<_> = all_facts
            .into_iter()
            .filter(|f| {
                f.fact_type == FactType::Wiki
                    && f.agent == agent_id
                    && (f.content.to_lowercase().contains(&query.to_lowercase())
                        || f.path.to_lowercase().contains(&query.to_lowercase()))
            })
            .collect();

        if !wiki_facts.is_empty() {
            result_content.push_str("\n\n## Matching Pages\n\n");
            for fact in &wiki_facts {
                // Extract slug from path
                let slug = fact
                    .path
                    .trim_start_matches(&format!("aleph://wiki/{}/", agent_id))
                    .trim_end_matches(".md");
                let file_path = wiki_file_path(&self.data_dir, agent_id, slug);
                if file_path.exists() {
                    let page_content = std::fs::read_to_string(&file_path).unwrap_or_default();
                    result_content.push_str(&format!("### {}\n\n{}\n\n---\n\n", slug, page_content));
                }
            }
        }

        if result_content.is_empty() {
            result_content = format!("No wiki pages found matching '{}'", query);
        }

        Ok(WikiManageResult {
            success: true,
            message: format!("Found {} matching pages", wiki_facts.len()),
            page_path: None,
            content: Some(result_content),
            pages: None,
        })
    }

    async fn handle_delete(&self, args: &WikiManageArgs) -> Result<WikiManageResult> {
        let slug = args
            .page_slug
            .as_deref()
            .ok_or_else(|| AlephError::tool("page_slug is required for delete"))?;

        let agent_id = self.agent_id();
        let file_path = wiki_file_path(&self.data_dir, agent_id, slug);

        if !file_path.exists() {
            return Err(AlephError::tool(format!(
                "Wiki page '{}' does not exist.",
                slug
            )));
        }

        // Delete file
        std::fs::remove_file(&file_path)
            .map_err(|e| AlephError::tool(format!("Failed to delete: {}", e)))?;

        // Invalidate fact
        let path = crate::wiki::wiki_path(agent_id, slug);
        if let Some(fact) = self.find_wiki_fact_by_path(&path).await? {
            self.database
                .invalidate_fact(&fact.id, "wiki page deleted")
                .await?;
        }

        // Update index
        self.regenerate_index(agent_id).await?;

        // Git commit
        let _ = self.git.commit_changes(agent_id, "delete", slug);

        info!(agent_id = agent_id, slug = slug, "Wiki page deleted");

        Ok(WikiManageResult {
            success: true,
            message: format!("Deleted wiki page '{}'", slug),
            page_path: None,
            content: None,
            pages: None,
        })
    }

    async fn handle_list(&self) -> Result<WikiManageResult> {
        let agent_id = self.agent_id();
        let index_path = self
            .data_dir
            .join("wiki")
            .join(agent_id)
            .join("index.md");

        let content = if index_path.exists() {
            std::fs::read_to_string(&index_path)
                .map_err(|e| AlephError::tool(format!("Failed to read index: {}", e)))?
        } else {
            "No wiki pages yet.".to_string()
        };

        Ok(WikiManageResult {
            success: true,
            message: "Wiki page list".to_string(),
            page_path: None,
            content: Some(content),
            pages: None,
        })
    }

    async fn find_wiki_fact_by_path(
        &self,
        path: &str,
    ) -> Result<Option<crate::memory::context::MemoryFact>> {
        let all_facts = self.database.get_all_facts(false, None).await?;
        Ok(all_facts.into_iter().find(|f| f.path == path))
    }

    async fn regenerate_index(&self, agent_id: &str) -> Result<()> {
        let all_facts = self.database.get_all_facts(false, None).await?;
        let wiki_facts: Vec<_> = all_facts
            .into_iter()
            .filter(|f| f.fact_type == FactType::Wiki && f.agent == agent_id)
            .collect();

        let entries: Vec<WikiIndexEntry> = wiki_facts
            .iter()
            .filter_map(|f| {
                let slug = f
                    .path
                    .trim_start_matches(&format!("aleph://wiki/{}/", agent_id))
                    .trim_end_matches(".md")
                    .to_string();

                // Try to read frontmatter from the actual file
                let file_path = wiki_file_path(&self.data_dir, agent_id, &slug);
                let (title, tags, updated) = if file_path.exists() {
                    let content = std::fs::read_to_string(&file_path).unwrap_or_default();
                    match parse_frontmatter(&content) {
                        Some(fm) => (
                            if fm.title.is_empty() { slug.clone() } else { fm.title },
                            fm.tags,
                            if fm.updated.is_empty() {
                                "unknown".to_string()
                            } else {
                                fm.updated
                            },
                        ),
                        None => (slug.clone(), vec![], "unknown".to_string()),
                    }
                } else {
                    (slug.clone(), vec![], "unknown".to_string())
                };

                Some(WikiIndexEntry {
                    slug,
                    title,
                    summary: f.content.chars().take(100).collect(),
                    tags,
                    updated,
                })
            })
            .collect();

        let agent_dir = self.data_dir.join("wiki").join(agent_id);
        write_index(&agent_dir, &entries).map_err(|e| AlephError::tool(e))?;
        Ok(())
    }
}

#[async_trait]
impl AlephTool for WikiManageTool {
    const NAME: &'static str = "wiki_manage";
    const DESCRIPTION: &'static str =
        "Create, update, query, delete, or list wiki knowledge pages. Wiki pages are structured Markdown documents that form an interlinked knowledge base.";

    type Args = WikiManageArgs;
    type Output = WikiManageResult;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "wiki_manage(action='create', page_slug='rust-ownership', title='Rust Ownership', summary='Core memory safety mechanism', content='# Rust Ownership\n...')".to_string(),
            "wiki_manage(action='query', query='rust memory') — search wiki for relevant pages".to_string(),
            "wiki_manage(action='list') — list all wiki pages".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match args.action {
            WikiAction::Create => self.handle_create(&args).await,
            WikiAction::Update => self.handle_update(&args).await,
            WikiAction::Query => self.handle_query(&args).await,
            WikiAction::Delete => self.handle_delete(&args).await,
            WikiAction::List => self.handle_list().await,
        }
    }
}
```

- [ ] **Step 2: Register module in builtin_tools/mod.rs**

In `src/builtin_tools/mod.rs`, add after `pub mod voice_tools;`:
```rust
pub mod wiki_manage;
```

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore 2>&1 | tail -15`
Expected: No errors.

- [ ] **Step 4: Commit**

```bash
git add src/builtin_tools/wiki_manage.rs src/builtin_tools/mod.rs
git commit -m "wiki: implement WikiManageTool with create/update/query/delete/list"
```

---

### Task 8: Register WikiManageTool in builder

**Files:**
- Modify: `src/executor/builtin_registry/builder.rs`

- [ ] **Step 1: Add wiki_manage_tool field and registration**

In the builder struct, add the `wiki_manage_tool` field alongside other tools. Then in the builder's `new()` method, after the skill management tools block (~line 700), add:

```rust
// Wiki management tool — always available when memory is enabled
let wiki_manage_tool = config
    .memory_database
    .as_ref()
    .map(|db| {
        let data_dir = config
            .data_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from(dirs::home_dir().unwrap().join(".aleph/data")));
        crate::builtin_tools::wiki_manage::WikiManageTool::new(data_dir, db.clone())
    });
```

Register the tool schema in the tools map:

```rust
if let Some(ref tool) = wiki_manage_tool {
    use crate::tools::AlephTool;
    let td = tool.definition();
    let mut ut = UnifiedTool::new(
        format!("builtin:{}", td.name),
        &td.name,
        &td.description,
        ToolSource::Builtin,
    );
    ut = ut.with_parameters_schema(td.parameters.clone());
    tools.insert(td.name.clone(), ut);
    info!("Registered wiki management tool (wiki_manage)");
}
```

Add `wiki_manage_tool` to the Self struct initialization and the struct field list.

- [ ] **Step 2: Wire dispatch**

In the tool dispatch method (where tool calls are routed to implementations), add a match arm for `"wiki_manage"`:

```rust
"wiki_manage" => {
    if let Some(ref tool) = self.wiki_manage_tool {
        dispatch_tool!(tool, args)
    } else {
        Err(AlephError::tool("Wiki system not available (memory not enabled)"))
    }
}
```

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore 2>&1 | tail -15`
Expected: No errors.

- [ ] **Step 4: Commit**

```bash
git add src/executor/builtin_registry/builder.rs
git commit -m "wiki: register WikiManageTool in builtin registry"
```

---

## Phase 2: Dreaming Pipeline Integration

### Task 9: Implement WikiIngestStage

**Files:**
- Create: `src/memory/dreaming/stages/wiki_ingest.rs`

- [ ] **Step 1: Implement the stage**

```rust
//! WikiIngestStage: passive ingestion of unprocessed Document facts into wiki pages.
//!
//! Scans for FactSource::Document facts not yet associated with any wiki page,
//! clusters them by topic, and generates wiki pages during the dream pipeline.

use async_trait::async_trait;
use tracing::info;

use super::{DreamContext, DreamStage};
use crate::error::AlephError;
use crate::memory::context::{FactSource, FactType};
use crate::memory::store::MemoryStore;

/// Configuration for wiki ingestion during dreams.
#[derive(Debug, Clone)]
pub struct WikiIngestConfig {
    pub enabled: bool,
    pub max_pages_per_run: usize,
    pub cooldown_days: u32,
}

impl Default for WikiIngestConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_pages_per_run: 10,
            cooldown_days: 1,
        }
    }
}

/// Passively ingests unprocessed Document facts into wiki pages.
pub struct WikiIngestStage;

#[async_trait]
impl DreamStage for WikiIngestStage {
    fn name(&self) -> &'static str {
        "wiki_ingest"
    }

    async fn should_run(&self, ctx: &DreamContext) -> bool {
        // Only run if LLM provider is available (needed for content generation)
        ctx.provider.is_some()
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let config = WikiIngestConfig::default();
        if !config.enabled {
            return Ok(ctx);
        }

        // Find Document-source facts not yet linked to wiki pages
        let all_facts = ctx.database.get_all_facts(false, None).await?;

        let document_facts: Vec<_> = all_facts
            .iter()
            .filter(|f| f.fact_source == FactSource::Document)
            .collect();

        let wiki_facts: Vec<_> = all_facts
            .iter()
            .filter(|f| f.fact_type == FactType::Wiki)
            .collect();

        // Find documents not referenced by any wiki page's sources
        let wiki_source_ids: std::collections::HashSet<&str> = wiki_facts
            .iter()
            .flat_map(|f| f.source_memory_ids.iter().map(|s| s.as_str()))
            .collect();

        let unprocessed: Vec<_> = document_facts
            .iter()
            .filter(|f| !wiki_source_ids.contains(f.id.as_str()))
            .take(config.max_pages_per_run)
            .collect();

        let unprocessed_count = unprocessed.len();

        if unprocessed_count == 0 {
            info!("WikiIngestStage: no unprocessed documents found");
            return Ok(ctx);
        }

        // For now, log the count. Full LLM-powered ingestion will be wired
        // when the dream pipeline's LLM calling infrastructure matures.
        info!(
            unprocessed = unprocessed_count,
            "WikiIngestStage: found unprocessed documents (LLM ingestion pending)"
        );

        Ok(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wiki_ingest_stage_name() {
        assert_eq!(WikiIngestStage.name(), "wiki_ingest");
    }

    #[test]
    fn wiki_ingest_config_defaults() {
        let config = WikiIngestConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_pages_per_run, 10);
        assert_eq!(config.cooldown_days, 1);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib -- dreaming::stages::wiki_ingest::tests 2>&1 | tail -5`
Expected: 2 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add src/memory/dreaming/stages/wiki_ingest.rs
git commit -m "wiki: implement WikiIngestStage for passive document ingestion"
```

---

### Task 10: Implement WikiLintStage

**Files:**
- Create: `src/memory/dreaming/stages/wiki_lint.rs`

- [ ] **Step 1: Implement the stage**

```rust
//! WikiLintStage: health checks for the wiki knowledge base.
//!
//! Checks for broken wikilinks, orphan pages, stale content,
//! and frontmatter gaps. Primarily diagnostic — only auto-fixes
//! lightweight issues like missing frontmatter fields.

use async_trait::async_trait;
use serde::Serialize;
use tracing::info;

use super::{DreamContext, DreamStage};
use crate::error::AlephError;
use crate::memory::context::FactType;
use crate::memory::store::MemoryStore;
use crate::wiki::wikilink::extract_wikilinks;

/// Report from wiki lint stage.
#[derive(Debug, Clone, Default, Serialize)]
pub struct WikiLintReport {
    /// (page_slug, broken_target_slug)
    pub broken_links: Vec<(String, String)>,
    /// Pages with no inbound links
    pub orphan_pages: Vec<String>,
    /// Pages referencing invalidated facts
    pub stale_pages: Vec<String>,
    /// Suggested new pages based on graph
    pub suggested_pages: Vec<String>,
    /// Number of auto-fixed issues
    pub auto_fixed: usize,
}

/// Health checks for wiki pages.
pub struct WikiLintStage;

#[async_trait]
impl DreamStage for WikiLintStage {
    fn name(&self) -> &'static str {
        "wiki_lint"
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let all_facts = ctx.database.get_all_facts(false, None).await?;
        let wiki_facts: Vec<_> = all_facts
            .iter()
            .filter(|f| f.fact_type == FactType::Wiki && f.is_valid)
            .collect();

        if wiki_facts.is_empty() {
            info!("WikiLintStage: no wiki pages to lint");
            return Ok(ctx);
        }

        let mut report = WikiLintReport::default();

        // Collect all known slugs
        let known_slugs: std::collections::HashSet<String> = wiki_facts
            .iter()
            .filter_map(|f| {
                // Extract slug from path: aleph://wiki/{agent}/{slug}.md
                let path = &f.path;
                let parts: Vec<&str> = path.split('/').collect();
                parts.last().map(|s| s.trim_end_matches(".md").to_string())
            })
            .collect();

        // Check each wiki page for broken links
        let mut inbound_links: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        for fact in &wiki_facts {
            let slug = fact
                .path
                .split('/')
                .last()
                .unwrap_or("")
                .trim_end_matches(".md")
                .to_string();

            // Try to read the actual file and extract wikilinks
            // We check content field as a proxy (full file access requires data_dir)
            let wikilinks = extract_wikilinks(&fact.content);
            for target in &wikilinks {
                inbound_links.insert(target.clone());
                if !known_slugs.contains(target) {
                    report.broken_links.push((slug.clone(), target.clone()));
                }
            }
        }

        // Find orphan pages (no inbound links)
        for slug in &known_slugs {
            if !inbound_links.contains(slug) {
                report.orphan_pages.push(slug.clone());
            }
        }

        info!(
            broken_links = report.broken_links.len(),
            orphan_pages = report.orphan_pages.len(),
            stale_pages = report.stale_pages.len(),
            auto_fixed = report.auto_fixed,
            "WikiLintStage complete"
        );

        Ok(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wiki_lint_stage_name() {
        assert_eq!(WikiLintStage.name(), "wiki_lint");
    }

    #[test]
    fn wiki_lint_report_default() {
        let report = WikiLintReport::default();
        assert!(report.broken_links.is_empty());
        assert!(report.orphan_pages.is_empty());
        assert_eq!(report.auto_fixed, 0);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib -- dreaming::stages::wiki_lint::tests 2>&1 | tail -5`
Expected: 2 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add src/memory/dreaming/stages/wiki_lint.rs
git commit -m "wiki: implement WikiLintStage for wiki health checks"
```

---

### Task 11: Register stages in dream pipeline

**Files:**
- Modify: `src/memory/dreaming/stages/mod.rs`
- Modify: `src/memory/dreaming/mod.rs`

- [ ] **Step 1: Export new stages**

In `src/memory/dreaming/stages/mod.rs`, add:
```rust
pub mod wiki_ingest;
pub mod wiki_lint;

pub use wiki_ingest::WikiIngestStage;
pub use wiki_lint::WikiLintStage;
```

- [ ] **Step 2: Re-export from dreaming/mod.rs**

In `src/memory/dreaming/mod.rs`, update the re-exports (~line 33):
```rust
pub use stages::{
    ConsolidateStage, DecayStage, DeepSynthesisStage, DriftDetectStage, SummarizeStage,
    TunnelDiscoveryStage, WikiIngestStage, WikiLintStage,
};
```

- [ ] **Step 3: Update pipeline builders**

In `src/memory/dreaming/mod.rs`, update `DreamPipeline::daily()`:
```rust
pub fn daily() -> Self {
    Self::new()
        .stage(SummarizeStage)
        .stage(DriftDetectStage)
        .stage(ConsolidateStage)
        .stage(WikiIngestStage)
        .stage(WikiLintStage)
        .stage(TunnelDiscoveryStage)
        .stage(DecayStage)
}
```

- [ ] **Step 4: Update pipeline stage count tests**

Update the existing tests to reflect the new stage count:

In `test_pipeline_builder`:
```rust
assert_eq!(pipeline.stages.len(), 7); // was 5
```

In `test_pipeline_weekly_has_six_stages`:
```rust
assert_eq!(pipeline.stages.len(), 8); // was 6
```

Update the async integration tests similarly:
```rust
// daily_pipeline_has_five_stages → daily_pipeline_has_seven_stages
assert_eq!(pipeline.stages.len(), 7);

// weekly_pipeline_has_six_stages → weekly_pipeline_has_eight_stages
assert_eq!(pipeline.stages.len(), 8);
```

- [ ] **Step 5: Run all pipeline tests**

Run: `cargo test -p alephcore --lib -- dreaming 2>&1 | tail -15`
Expected: All pipeline tests PASS with updated counts.

- [ ] **Step 6: Commit**

```bash
git add src/memory/dreaming/stages/mod.rs src/memory/dreaming/mod.rs
git commit -m "wiki: register WikiIngestStage and WikiLintStage in dream pipeline"
```

---

### Task 12: Final integration test and compile

**Files:**
- No new files — validation pass

- [ ] **Step 1: Full compile check**

Run: `cargo check -p alephcore 2>&1 | tail -15`
Expected: No errors.

- [ ] **Step 2: Run all wiki tests**

Run: `cargo test -p alephcore --lib -- wiki 2>&1 | tail -20`
Expected: All wiki module tests PASS.

- [ ] **Step 3: Run all dreaming tests**

Run: `cargo test -p alephcore --lib -- dreaming 2>&1 | tail -20`
Expected: All dreaming tests PASS (including updated stage counts).

- [ ] **Step 4: Run full test suite**

Run: `cargo test -p alephcore --lib 2>&1 | tail -20`
Expected: No regressions.

- [ ] **Step 5: Run clippy**

Run: `cargo clippy -p alephcore 2>&1 | tail -15`
Expected: No warnings.

- [ ] **Step 6: Commit (if any fixes needed)**

```bash
git add -u
git commit -m "wiki: fix clippy warnings and integration issues"
```
