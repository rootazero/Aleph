//! CLI Commands for Memory Management
//!
//! Provides command implementations for listing, showing, adding, and managing
//! knowledge notes.  Migrated from MemoryFact (MemoryStore) to NoteStore CRUD.

use crate::error::AlephError;
use crate::memory::notes::store::NoteIndexEntry;
use crate::memory::notes::store::NoteStore;
use crate::memory::store::MemoryBackend;

/// Filter options for listing notes
#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    /// Filter by category (e.g. "preference", "wiki")
    pub category: Option<String>,
    /// Keyword filter on path / filename
    pub query: Option<String>,
    /// Maximum number of results
    pub limit: Option<usize>,
}

impl ListFilter {
    /// Create a new filter with defaults
    pub fn new() -> Self {
        Self::default()
    }

    /// Set category filter
    pub fn with_category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }

    /// Set keyword query (matches on filename / path)
    pub fn with_query(mut self, query: impl Into<String>) -> Self {
        self.query = Some(query.into());
        self
    }

    /// Set result limit
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
}

/// Display format for CLI output
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    /// ASCII table format
    #[default]
    Table,
    /// JSON format
    Json,
    /// CSV format
    Csv,
}

impl std::str::FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "table" => Ok(OutputFormat::Table),
            "json" => Ok(OutputFormat::Json),
            "csv" => Ok(OutputFormat::Csv),
            _ => Err(format!("Unknown format: {}. Use: table, json, csv", s)),
        }
    }
}

/// A note summary for display
#[derive(Debug, Clone, serde::Serialize)]
pub struct NoteSummary {
    /// Note path (e.g. "preference/editor")
    pub path: String,
    /// Filename / title
    pub filename: String,
    /// Category
    pub category: String,
    /// Tags
    pub tags: Vec<String>,
    /// Number of outgoing wikilinks
    pub link_count: usize,
    /// Last updated timestamp
    pub updated_at: i64,
}

impl NoteSummary {
    /// Create from a NoteIndexEntry
    pub fn from_entry(entry: &NoteIndexEntry) -> Self {
        Self {
            path: entry.path.clone(),
            filename: entry.filename.clone(),
            category: entry.category.clone(),
            tags: entry.tags.clone(),
            link_count: entry.link_count,
            updated_at: entry.updated_at,
        }
    }

    /// Format as table row
    pub fn to_table_row(&self) -> String {
        let tags_preview = if self.tags.is_empty() {
            "-".to_string()
        } else {
            self.tags.join(", ")
        };
        let filename_col = if self.filename.chars().count() > 30 {
            let t: String = self.filename.chars().take(27).collect();
            format!("{}...", t)
        } else {
            self.filename.clone()
        };
        format!(
            "{:<35} {:<12} {:>5}  {}",
            filename_col, self.category, self.link_count, tags_preview
        )
    }

    /// Format as CSV row
    pub fn to_csv_row(&self) -> String {
        format!(
            "{},{},{},\"{}\"",
            self.path,
            self.category,
            self.link_count,
            self.tags.join(";").replace('"', "\"\"")
        )
    }
}

/// Memory CLI commands — operates on NoteStore (notes index).
pub struct MemoryCommands {
    db: MemoryBackend,
    /// Agent scope for all operations
    agent_id: String,
    /// Base memory directory for reading note file content
    memory_dir: std::path::PathBuf,
}

impl MemoryCommands {
    /// Create new commands instance
    pub fn new(db: MemoryBackend, agent_id: impl Into<String>) -> Self {
        let memory_dir = crate::utils::paths::get_note_memory_dir().unwrap_or_else(|_| {
            std::env::temp_dir()
                .join("aleph")
                .join("memory")
                .join("note")
        });
        Self {
            db,
            agent_id: agent_id.into(),
            memory_dir,
        }
    }

    /// Create with an explicit memory directory (useful for testing)
    pub fn with_memory_dir(
        db: MemoryBackend,
        agent_id: impl Into<String>,
        memory_dir: std::path::PathBuf,
    ) -> Self {
        Self {
            db,
            agent_id: agent_id.into(),
            memory_dir,
        }
    }

    /// List notes with optional filtering
    pub async fn list(&self, filter: ListFilter) -> Result<Vec<NoteSummary>, AlephError> {
        let entries = self.db.list_notes(&self.agent_id).await?;

        let mut summaries: Vec<NoteSummary> = entries
            .iter()
            .filter(|e| {
                // Category filter
                if let Some(ref cat) = filter.category {
                    if &e.category != cat {
                        return false;
                    }
                }
                // Query filter (matches on path or filename)
                if let Some(ref q) = filter.query {
                    let q_lower = q.to_lowercase();
                    if !e.path.to_lowercase().contains(&q_lower)
                        && !e.filename.to_lowercase().contains(&q_lower)
                    {
                        return false;
                    }
                }
                true
            })
            .map(NoteSummary::from_entry)
            .collect();

        // Sort by most recently updated first (list_notes already does this,
        // but category filtering may reorder — preserve original ordering)
        if let Some(limit) = filter.limit {
            summaries.truncate(limit);
        }

        Ok(summaries)
    }

    /// Get a single note by path (e.g. "preference/editor") or by filename prefix.
    ///
    /// Returns the index metadata and the markdown content from disk.
    pub async fn show(
        &self,
        path_or_prefix: &str,
    ) -> Result<Option<(NoteSummary, String)>, AlephError> {
        // Try exact path first
        if let Some(entry) = self
            .db
            .get_note_index(path_or_prefix, &self.agent_id)
            .await?
        {
            let content = self.read_note_file(&entry).await.unwrap_or_default();
            return Ok(Some((NoteSummary::from_entry(&entry), content)));
        }

        // Try prefix match on filename
        let all = self.db.list_notes(&self.agent_id).await?;
        let matches: Vec<_> = all
            .iter()
            .filter(|e| {
                e.filename
                    .to_lowercase()
                    .starts_with(&path_or_prefix.to_lowercase())
            })
            .collect();

        match matches.len() {
            0 => Ok(None),
            1 => {
                let entry = matches[0];
                let content = self.read_note_file(entry).await.unwrap_or_default();
                Ok(Some((NoteSummary::from_entry(entry), content)))
            }
            _ => Err(AlephError::other(format!(
                "Ambiguous prefix '{}' matches {} notes. Use more characters.",
                path_or_prefix,
                matches.len()
            ))),
        }
    }

    /// Format list output
    pub fn format_list(&self, summaries: &[NoteSummary], format: OutputFormat) -> String {
        match format {
            OutputFormat::Table => {
                let mut output = String::new();
                output.push_str("FILENAME                            CATEGORY     LINKS  TAGS\n");
                output.push_str(
                    "---------------------------------------------------------------------\n",
                );
                for s in summaries {
                    output.push_str(&s.to_table_row());
                    output.push('\n');
                }
                output
            }
            OutputFormat::Json => serde_json::to_string_pretty(summaries).unwrap_or_default(),
            OutputFormat::Csv => {
                let mut output = String::from("path,category,link_count,tags\n");
                for s in summaries {
                    output.push_str(&s.to_csv_row());
                    output.push('\n');
                }
                output
            }
        }
    }

    /// Format single note output
    pub fn format_show(
        &self,
        summary: &NoteSummary,
        content: &str,
        format: OutputFormat,
    ) -> String {
        match format {
            OutputFormat::Table => {
                format!(
                    r#"+-------------------------------------------------------------+
| Note: {}
+-------------------------------------------------------------+
{}
+-------------------------------------------------------------+"#,
                    summary.path, content
                )
            }
            OutputFormat::Json => {
                #[derive(serde::Serialize)]
                struct NoteDetail<'a> {
                    #[serde(flatten)]
                    summary: &'a NoteSummary,
                    content: &'a str,
                }
                serde_json::to_string_pretty(&NoteDetail { summary, content }).unwrap_or_default()
            }
            OutputFormat::Csv => summary.to_csv_row(),
        }
    }

    /// Soft-delete (forget) a note by removing its index entry and deleting the file.
    ///
    /// `path_or_prefix` is a full path like "preference/editor" or a filename prefix.
    pub async fn forget(&self, path_or_prefix: &str) -> Result<String, AlephError> {
        let full_path = self.resolve_note_path(path_or_prefix).await?;

        // Remove from index (links + FTS included)
        self.db
            .remove_note_index(&full_path, &self.agent_id)
            .await?;

        // Delete the markdown file from disk
        if let Some(entry_) = self.resolve_entry(&full_path).await {
            let file_path = self
                .memory_dir
                .join(&self.agent_id)
                .join(&entry_.category)
                .join(format!("{}.md", entry_.filename));
            let _ = tokio::fs::remove_file(&file_path).await;
        } else {
            // Entry was already removed from index — try to infer the file path
            // from the path components ("category/filename")
            if let Some((category, filename)) = full_path.split_once('/') {
                let file_path = self
                    .memory_dir
                    .join(&self.agent_id)
                    .join(category)
                    .join(format!("{filename}.md"));
                let _ = tokio::fs::remove_file(&file_path).await;
            }
        }

        Ok(full_path)
    }

    /// Count total notes for the agent.
    pub async fn count(&self) -> Result<i64, AlephError> {
        self.db.count_all_notes().await
    }

    /// Get statistics about the memory database
    pub async fn stats(&self) -> Result<MemoryStats, AlephError> {
        let total = self.db.count_all_notes().await? as usize;
        Ok(MemoryStats { total_notes: total })
    }

    // -------------------------------------------------------------------------
    // Helpers
    // -------------------------------------------------------------------------

    /// Read markdown content for a note from disk.
    async fn read_note_file(&self, entry: &NoteIndexEntry) -> Option<String> {
        let file_path = self
            .memory_dir
            .join(&self.agent_id)
            .join(&entry.category)
            .join(format!("{}.md", entry.filename));
        tokio::fs::read_to_string(&file_path).await.ok()
    }

    /// Resolve a path or filename prefix to a full note path.
    async fn resolve_note_path(&self, path_or_prefix: &str) -> Result<String, AlephError> {
        // Try exact match first
        if self
            .db
            .get_note_index(path_or_prefix, &self.agent_id)
            .await?
            .is_some()
        {
            return Ok(path_or_prefix.to_string());
        }

        // Try prefix match on filename
        let all = self.db.list_notes(&self.agent_id).await?;
        let matches: Vec<_> = all
            .iter()
            .filter(|e| {
                e.filename
                    .to_lowercase()
                    .starts_with(&path_or_prefix.to_lowercase())
            })
            .collect();

        match matches.len() {
            0 => Err(AlephError::other(format!(
                "Note not found: {}",
                path_or_prefix
            ))),
            1 => Ok(matches[0].path.clone()),
            _ => Err(AlephError::other(format!(
                "Ambiguous prefix '{}' matches {} notes. Use more characters.",
                path_or_prefix,
                matches.len()
            ))),
        }
    }

    /// Look up an entry after it has already been removed from the index.
    /// Returns `None` if the entry is gone (already deleted).
    async fn resolve_entry(&self, path: &str) -> Option<NoteIndexEntry> {
        self.db
            .get_note_index(path, &self.agent_id)
            .await
            .ok()
            .flatten()
    }
}

/// Result of a delete operation
#[derive(Debug, Clone)]
pub struct DeleteResult {
    /// The deleted note path
    pub note_path: String,
}

/// Result of garbage collection (stub — notes use file deletion via `forget`)
#[derive(Debug, Clone)]
pub struct GcResult {
    /// Number of orphaned index entries cleaned
    pub deleted_count: usize,
    /// Number of valid notes remaining
    pub valid_notes: usize,
    /// Retention period used (days)
    pub retention_days: u32,
}

impl std::fmt::Display for GcResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "GC complete: {} orphan entries removed, {} notes remain (retention: {} days)",
            self.deleted_count, self.valid_notes, self.retention_days
        )
    }
}

/// Memory database statistics
#[derive(Debug, Clone, serde::Serialize)]
pub struct MemoryStats {
    pub total_notes: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_filter_builder() {
        let filter = ListFilter::new().with_category("preference").with_limit(10);

        assert_eq!(filter.category, Some("preference".to_string()));
        assert_eq!(filter.limit, Some(10));
    }

    #[test]
    fn test_output_format_parse() {
        assert_eq!(
            "table".parse::<OutputFormat>().unwrap(),
            OutputFormat::Table
        );
        assert_eq!("JSON".parse::<OutputFormat>().unwrap(), OutputFormat::Json);
        assert_eq!("csv".parse::<OutputFormat>().unwrap(), OutputFormat::Csv);
        assert!("invalid".parse::<OutputFormat>().is_err());
    }

    #[test]
    fn test_note_summary_table_row() {
        let summary = NoteSummary {
            path: "preference/editor".to_string(),
            filename: "editor".to_string(),
            category: "preference".to_string(),
            tags: vec!["vim".to_string()],
            link_count: 3,
            updated_at: 1234567890,
        };

        let row = summary.to_table_row();
        assert!(row.contains("editor"));
        assert!(row.contains("preference"));
        assert!(row.contains("3"));
        assert!(row.contains("vim"));
    }

    #[test]
    fn test_note_summary_csv_row() {
        let summary = NoteSummary {
            path: "wiki/rust".to_string(),
            filename: "rust".to_string(),
            category: "wiki".to_string(),
            tags: vec!["systems".to_string(), "language".to_string()],
            link_count: 5,
            updated_at: 1234567890,
        };

        let csv = summary.to_csv_row();
        assert!(csv.contains("wiki/rust"));
        assert!(csv.contains("wiki"));
        assert!(csv.contains("5"));
    }

    #[test]
    fn test_gc_result_display() {
        let result = GcResult {
            deleted_count: 5,
            valid_notes: 100,
            retention_days: 30,
        };

        let display = format!("{}", result);
        assert!(display.contains("5 orphan"));
        assert!(display.contains("100 notes"));
        assert!(display.contains("30 days"));
    }

    #[test]
    fn test_memory_stats() {
        let stats = MemoryStats { total_notes: 42 };
        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("42"));
    }
}
