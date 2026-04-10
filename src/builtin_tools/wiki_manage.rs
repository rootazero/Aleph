//! wiki_manage — LLM Tool for creating, updating, querying, deleting, and listing wiki pages.

use std::path::PathBuf;

use async_trait::async_trait;
use tracing::{info, warn};

use crate::error::{AlephError, Result};
use crate::gateway::agent_env::AgentEnvFilter;
use crate::memory::context::FactType;
use crate::memory::store::types::SearchFilter;
use crate::memory::store::{MemoryBackend, MemoryStore};
use crate::tools::AlephTool;
use crate::wiki::git::WikiGitManager;
use crate::wiki::index::{WikiIndexEntry, write_index};
use crate::wiki::tools::manage::{
    WikiAction, WikiListEntry, WikiManageArgs, WikiManageResult, build_wiki_fact,
    validate_args_for_create,
};
use crate::wiki::wikilink::parse_frontmatter;
use crate::wiki::{wiki_file_path, wiki_path};

/// Tool for managing wiki knowledge pages.
#[derive(Clone)]
pub struct WikiManageTool {
    data_dir: PathBuf,
    database: MemoryBackend,
    git: WikiGitManager,
}

impl WikiManageTool {
    pub fn new(data_dir: PathBuf, database: MemoryBackend, git: WikiGitManager) -> Self {
        Self {
            data_dir,
            database,
            git,
        }
    }

    /// Default agent ID for wiki operations.
    // TODO: accept agent_id from session context instead of hardcoding
    fn agent_id(&self) -> &str {
        "default"
    }

    /// Find a wiki fact by its VFS path.
    async fn find_wiki_fact_by_path(
        &self,
        path: &str,
    ) -> Result<Option<crate::memory::context::MemoryFact>> {
        let filter = SearchFilter::new()
            .with_fact_type(FactType::Wiki)
            .with_valid_only()
            .with_path_prefix(path);
        let facts = self
            .database
            .text_search(path, &filter, 1)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to search wiki facts: {}", e)))?;

        if let Some(scored) = facts.first() {
            if scored.fact.path == path {
                return Ok(Some(scored.fact.clone()));
            }
        }

        // Fallback: scan all wiki facts for exact path match
        let all_facts = self
            .database
            .get_all_facts(false, None)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to get facts: {}", e)))?;

        Ok(all_facts
            .into_iter()
            .find(|f| f.fact_type == FactType::Wiki && f.path == path))
    }

    /// Regenerate index.md from all wiki facts for an agent.
    async fn regenerate_index(&self, agent_id: &str) -> Result<()> {
        let all_facts = self
            .database
            .get_all_facts(false, None)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to get facts for index: {}", e)))?;

        let entries: Vec<WikiIndexEntry> = all_facts
            .iter()
            .filter(|f| f.fact_type == FactType::Wiki && f.agent == agent_id && f.is_valid)
            .filter_map(|f| {
                // Extract slug from path: aleph://wiki/{agent}/{slug}.md
                let path_str = &f.path;
                let slug = path_str
                    .strip_prefix(&format!("aleph://wiki/{}/", agent_id))
                    .and_then(|s| s.strip_suffix(".md"))?;

                // Try to read frontmatter from the file for title/tags
                let file_path = wiki_file_path(&self.data_dir, agent_id, slug);
                let (title, tags, updated) = if let Ok(content) = std::fs::read_to_string(&file_path)
                {
                    if let Some(fm) = parse_frontmatter(&content) {
                        let title = if fm.title.is_empty() {
                            slug.to_string()
                        } else {
                            fm.title
                        };
                        (title, fm.tags, fm.updated)
                    } else {
                        (slug.to_string(), vec![], String::new())
                    }
                } else {
                    (slug.to_string(), vec![], String::new())
                };

                let updated = if updated.is_empty() {
                    chrono::Local::now().format("%Y-%m-%d").to_string()
                } else {
                    updated
                };

                Some(WikiIndexEntry {
                    slug: slug.to_string(),
                    title,
                    summary: f.content.clone(),
                    tags,
                    updated,
                })
            })
            .collect();

        let agent_dir = self
            .git
            .ensure_agent_dir(agent_id)
            .map_err(AlephError::tool)?;

        write_index(&agent_dir, &entries).map_err(AlephError::tool)?;

        Ok(())
    }

    /// Handle create action.
    async fn handle_create(&self, args: &WikiManageArgs) -> Result<WikiManageResult> {
        let agent_id = self.agent_id();
        let page_slug = args
            .page_slug
            .as_deref()
            .ok_or_else(|| AlephError::tool("page_slug is required for create"))?;
        let title = args
            .title
            .as_deref()
            .ok_or_else(|| AlephError::tool("title is required for create"))?;
        let summary = args
            .summary
            .as_deref()
            .ok_or_else(|| AlephError::tool("summary is required for create"))?;
        let content = args
            .content
            .as_deref()
            .ok_or_else(|| AlephError::tool("content is required for create"))?;

        validate_args_for_create(page_slug, title, summary, content)
            .map_err(AlephError::tool)?;

        // Ensure git repo and agent directory
        self.git.ensure_repo().map_err(AlephError::tool)?;
        let _agent_dir = self
            .git
            .ensure_agent_dir(agent_id)
            .map_err(AlephError::tool)?;

        // Build frontmatter + content
        let now = chrono::Local::now().format("%Y-%m-%d").to_string();
        let full_content = format!(
            "---\ntitle: {}\naliases: []\ntags: []\nsources: []\ncreated: \"{}\"\nupdated: \"{}\"\n---\n\n{}",
            title, now, now, content
        );

        // Atomic create: OpenOptions::create_new ensures the file doesn't already exist,
        // avoiding TOCTOU race between exists() check and write.
        let file_path = wiki_file_path(&self.data_dir, agent_id, page_slug);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&file_path)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    AlephError::tool(format!(
                        "Wiki page '{}' already exists. Use 'update' action instead.",
                        page_slug
                    ))
                } else {
                    AlephError::tool(format!("Failed to create wiki page: {}", e))
                }
            })?;
        std::io::Write::write_all(&mut file, full_content.as_bytes())
            .map_err(|e| AlephError::tool(format!("Failed to write wiki page: {}", e)))?;

        // Create wiki fact in memory store
        let fact = build_wiki_fact(agent_id, page_slug, summary);
        self.database
            .insert_fact(&fact)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to insert wiki fact: {}", e)))?;

        // Regenerate index
        self.regenerate_index(agent_id).await?;

        // Git commit
        if let Err(e) = self.git.commit_changes(agent_id, "create", page_slug) {
            warn!(error = %e, "Git commit failed for wiki create");
        }

        let vfs_path = wiki_path(agent_id, page_slug);
        info!(page = page_slug, path = %vfs_path, "Wiki page created");

        Ok(WikiManageResult {
            success: true,
            message: format!("Created wiki page '{}'", page_slug),
            page_path: Some(vfs_path),
            content: None,
            pages: None,
        })
    }

    /// Handle update action.
    async fn handle_update(&self, args: &WikiManageArgs) -> Result<WikiManageResult> {
        let agent_id = self.agent_id();
        let page_slug = args
            .page_slug
            .as_deref()
            .ok_or_else(|| AlephError::tool("page_slug is required for update"))?;

        let file_path = wiki_file_path(&self.data_dir, agent_id, page_slug);
        if !file_path.exists() {
            return Err(AlephError::tool(format!(
                "Wiki page '{}' does not exist. Use 'create' action first.",
                page_slug
            )));
        }

        // Read existing content
        let existing = std::fs::read_to_string(&file_path)
            .map_err(|e| AlephError::tool(format!("Failed to read wiki page: {}", e)))?;

        // Build updated content
        let new_content = if let Some(content) = &args.content {
            // Replace body, preserving frontmatter if present
            if let Some(_fm) = parse_frontmatter(&existing) {
                // Find end of frontmatter
                let rest = &existing[3..]; // skip leading "---"
                if let Some(end_pos) = rest.find("---") {
                    let frontmatter_section = &existing[..end_pos + 3 + 3]; // "---" + yaml + "---"
                    // Update the "updated" field in frontmatter
                    let now = chrono::Local::now().format("%Y-%m-%d").to_string();
                    let updated_fm = frontmatter_section.replace(
                        &format!("updated: \"{}\"", _fm.updated),
                        &format!("updated: \"{}\"", now),
                    );
                    format!("{}\n\n{}", updated_fm, content)
                } else {
                    content.clone()
                }
            } else {
                content.clone()
            }
        } else {
            // No new content provided, nothing to update in the file
            existing.clone()
        };

        // Write updated file
        std::fs::write(&file_path, &new_content)
            .map_err(|e| AlephError::tool(format!("Failed to write wiki page: {}", e)))?;

        // Update fact summary if provided
        let vfs_path = wiki_path(agent_id, page_slug);
        if let Some(summary) = &args.summary {
            if let Some(fact) = self.find_wiki_fact_by_path(&vfs_path).await? {
                self.database
                    .update_fact_content(&fact.id, summary)
                    .await
                    .map_err(|e| {
                        AlephError::tool(format!("Failed to update wiki fact: {}", e))
                    })?;
            }
        }

        // Regenerate index
        self.regenerate_index(agent_id).await?;

        // Git commit
        if let Err(e) = self.git.commit_changes(agent_id, "update", page_slug) {
            warn!(error = %e, "Git commit failed for wiki update");
        }

        info!(page = page_slug, "Wiki page updated");

        Ok(WikiManageResult {
            success: true,
            message: format!("Updated wiki page '{}'", page_slug),
            page_path: Some(vfs_path),
            content: None,
            pages: None,
        })
    }

    /// Handle query action.
    async fn handle_query(&self, args: &WikiManageArgs) -> Result<WikiManageResult> {
        let agent_id = self.agent_id();
        let query = args
            .query
            .as_deref()
            .ok_or_else(|| AlephError::tool("query is required for query action"))?;

        // Search wiki facts by text
        let filter = SearchFilter::new()
            .with_fact_type(FactType::Wiki)
            .with_valid_only()
            .with_agent_filter(AgentEnvFilter::Single(agent_id.to_string()));

        let results = self
            .database
            .text_search(query, &filter, 10)
            .await
            .map_err(|e| AlephError::tool(format!("Wiki search failed: {}", e)))?;

        if results.is_empty() {
            return Ok(WikiManageResult {
                success: true,
                message: format!("No wiki pages found matching '{}'", query),
                page_path: None,
                content: None,
                pages: Some(vec![]),
            });
        }

        // Read matched pages and build entries
        let mut pages = Vec::new();
        let mut combined_content = String::new();

        for scored in &results {
            let fact = &scored.fact;
            // Extract slug from path
            let slug = fact
                .path
                .strip_prefix(&format!("aleph://wiki/{}/", agent_id))
                .and_then(|s| s.strip_suffix(".md"))
                .unwrap_or("unknown");

            let file_path = wiki_file_path(&self.data_dir, agent_id, slug);
            let file_content = std::fs::read_to_string(&file_path).unwrap_or_default();

            let title = parse_frontmatter(&file_content)
                .map(|fm| fm.title)
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| slug.to_string());

            pages.push(WikiListEntry {
                slug: slug.to_string(),
                title: title.clone(),
                summary: fact.content.clone(),
                path: fact.path.clone(),
            });

            combined_content.push_str(&format!("## {} ({})\n\n{}\n\n---\n\n", title, slug, file_content));
        }

        Ok(WikiManageResult {
            success: true,
            message: format!("Found {} wiki pages matching '{}'", pages.len(), query),
            page_path: None,
            content: Some(combined_content),
            pages: Some(pages),
        })
    }

    /// Handle delete action.
    async fn handle_delete(&self, args: &WikiManageArgs) -> Result<WikiManageResult> {
        let agent_id = self.agent_id();
        let page_slug = args
            .page_slug
            .as_deref()
            .ok_or_else(|| AlephError::tool("page_slug is required for delete"))?;

        let file_path = wiki_file_path(&self.data_dir, agent_id, page_slug);
        if !file_path.exists() {
            return Err(AlephError::tool(format!(
                "Wiki page '{}' does not exist",
                page_slug
            )));
        }

        // Invalidate the fact first (recoverable), then delete the file.
        // An orphan file is harmless; a dangling fact pointing to a missing file is not.
        let vfs_path = wiki_path(agent_id, page_slug);
        if let Some(fact) = self.find_wiki_fact_by_path(&vfs_path).await? {
            self.database
                .invalidate_fact(&fact.id, "Wiki page deleted by user")
                .await
                .map_err(|e| AlephError::tool(format!("Failed to invalidate wiki fact: {}", e)))?;
        }

        // Delete the file
        std::fs::remove_file(&file_path)
            .map_err(|e| AlephError::tool(format!("Failed to delete wiki page: {}", e)))?;

        // Regenerate index
        self.regenerate_index(agent_id).await?;

        // Git commit
        if let Err(e) = self.git.commit_changes(agent_id, "delete", page_slug) {
            warn!(error = %e, "Git commit failed for wiki delete");
        }

        info!(page = page_slug, "Wiki page deleted");

        Ok(WikiManageResult {
            success: true,
            message: format!("Deleted wiki page '{}'", page_slug),
            page_path: None,
            content: None,
            pages: None,
        })
    }

    /// Handle list action.
    async fn handle_list(&self) -> Result<WikiManageResult> {
        let agent_id = self.agent_id();

        // Get all wiki facts for this agent
        let all_facts = self
            .database
            .get_all_facts(false, None)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to list wiki facts: {}", e)))?;

        let pages: Vec<WikiListEntry> = all_facts
            .iter()
            .filter(|f| f.fact_type == FactType::Wiki && f.agent == agent_id && f.is_valid)
            .filter_map(|f| {
                let slug = f
                    .path
                    .strip_prefix(&format!("aleph://wiki/{}/", agent_id))
                    .and_then(|s| s.strip_suffix(".md"))?;

                let file_path = wiki_file_path(&self.data_dir, agent_id, slug);
                let title = std::fs::read_to_string(&file_path)
                    .ok()
                    .and_then(|c| parse_frontmatter(&c))
                    .map(|fm| fm.title)
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| slug.to_string());

                Some(WikiListEntry {
                    slug: slug.to_string(),
                    title,
                    summary: f.content.clone(),
                    path: f.path.clone(),
                })
            })
            .collect();

        // Also read index.md content if it exists
        let agent_dir = self.data_dir.join("wiki").join(agent_id);
        let index_path = agent_dir.join("index.md");
        let index_content = std::fs::read_to_string(&index_path).ok();

        Ok(WikiManageResult {
            success: true,
            message: format!("{} wiki pages", pages.len()),
            page_path: None,
            content: index_content,
            pages: Some(pages),
        })
    }
}

#[async_trait]
impl AlephTool for WikiManageTool {
    const NAME: &'static str = "wiki_manage";
    const DESCRIPTION: &'static str =
        "Manage personal wiki knowledge pages. Create, update, query, delete, or list wiki pages. \
         Pages are Markdown files with YAML frontmatter, tracked by git, and indexed in memory.";

    type Args = WikiManageArgs;
    type Output = WikiManageResult;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "wiki_manage(action='create', page_slug='rust-ownership', title='Rust Ownership', summary='Rust ownership and borrowing rules', content='# Rust Ownership\\n...')".to_string(),
            "wiki_manage(action='update', page_slug='rust-ownership', content='# Updated content...', summary='Updated summary')".to_string(),
            "wiki_manage(action='query', query='ownership borrowing')".to_string(),
            "wiki_manage(action='delete', page_slug='rust-ownership')".to_string(),
            "wiki_manage(action='list')".to_string(),
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
