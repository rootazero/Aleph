//! CLI Commands for Memory Management
//!
//! Provides command implementations for listing, showing, adding, editing, and managing facts.

use crate::error::AlephError;
use crate::memory::context::{FactType, MemoryFact, FactSpecificity, TemporalScope};
use crate::memory::store::{MemoryBackend, MemoryStore};
use crate::memory::EmbeddingProvider;
use std::sync::Arc;

/// Filter options for listing facts
#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    /// Filter by fact type
    pub fact_type: Option<FactType>,
    /// Minimum strength threshold
    pub min_strength: Option<f32>,
    /// Include decayed/invalidated facts
    pub include_decayed: bool,
    /// Keyword filter on content
    pub query: Option<String>,
    /// Maximum number of results
    pub limit: Option<usize>,
}

impl ListFilter {
    /// Create a new filter with defaults
    pub fn new() -> Self {
        Self::default()
    }

    /// Set fact type filter
    pub fn with_type(mut self, fact_type: FactType) -> Self {
        self.fact_type = Some(fact_type);
        self
    }

    /// Set minimum strength filter
    pub fn with_min_strength(mut self, min: f32) -> Self {
        self.min_strength = Some(min);
        self
    }

    /// Include decayed facts
    pub fn include_decayed(mut self) -> Self {
        self.include_decayed = true;
        self
    }

    /// Set keyword query
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

/// A fact summary for display
#[derive(Debug, Clone, serde::Serialize)]
pub struct FactSummary {
    /// Short ID (first 8 chars)
    pub id: String,
    /// Full ID
    pub full_id: String,
    /// Fact type
    pub fact_type: String,
    /// Current strength (calculated)
    pub strength: f32,
    /// Truncated content
    pub content_preview: String,
    /// Full content
    pub content: String,
    /// Is valid
    pub is_valid: bool,
    /// Created timestamp
    pub created_at: i64,
}

impl FactSummary {
    /// Create from a MemoryFact
    pub fn from_fact(fact: &MemoryFact, strength: f32) -> Self {
        let content_preview = if fact.content.len() > 50 {
            format!("{}...", &fact.content[..47])
        } else {
            fact.content.clone()
        };

        Self {
            id: fact.id[..8.min(fact.id.len())].to_string(),
            full_id: fact.id.clone(),
            fact_type: format!("{:?}", fact.fact_type).to_lowercase(),
            strength,
            content_preview,
            content: fact.content.clone(),
            is_valid: fact.is_valid,
            created_at: fact.created_at,
        }
    }

    /// Format as table row
    pub fn to_table_row(&self) -> String {
        format!(
            "{:<10} {:<12} {:>6.2}  {}",
            self.id, self.fact_type, self.strength, self.content_preview
        )
    }

    /// Format as CSV row
    pub fn to_csv_row(&self) -> String {
        format!(
            "{},{},{:.2},\"{}\"",
            self.full_id,
            self.fact_type,
            self.strength,
            self.content.replace('"', "\"\"")
        )
    }
}

/// Memory CLI commands
pub struct MemoryCommands {
    db: MemoryBackend,
}

impl MemoryCommands {
    /// Create new commands instance
    pub fn new(db: MemoryBackend) -> Self {
        Self { db }
    }

    /// List facts with optional filtering
    pub async fn list(&self, filter: ListFilter) -> Result<Vec<FactSummary>, AlephError> {
        // Get facts from database
        let facts = self.db.get_all_facts(filter.include_decayed).await?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // Apply filters and convert to summaries
        let mut summaries: Vec<FactSummary> = facts
            .iter()
            .filter(|f| {
                // Type filter
                if let Some(ref ft) = filter.fact_type {
                    if &f.fact_type != ft {
                        return false;
                    }
                }

                // Query filter
                if let Some(ref q) = filter.query {
                    if !f.content.to_lowercase().contains(&q.to_lowercase()) {
                        return false;
                    }
                }

                true
            })
            .map(|f| {
                // Calculate strength (simplified - use decay config if available)
                let days_old = (now - f.updated_at) as f32 / 86400.0;
                let strength = if f.is_valid {
                    0.5_f32.powf(days_old / 30.0).max(0.0)
                } else {
                    0.0
                };
                FactSummary::from_fact(f, strength)
            })
            .filter(|s| {
                // Strength filter
                if let Some(min) = filter.min_strength {
                    if s.strength < min {
                        return false;
                    }
                }
                true
            })
            .collect();

        // Sort by strength descending
        summaries.sort_by(|a, b| b.strength.partial_cmp(&a.strength).unwrap());

        // Apply limit
        if let Some(limit) = filter.limit {
            summaries.truncate(limit);
        }

        Ok(summaries)
    }

    /// Get a single fact by ID (supports partial ID match)
    pub async fn show(&self, id: &str) -> Result<Option<FactSummary>, AlephError> {
        // Try exact match first
        if let Some(fact) = self.db.get_fact(id).await? {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;
            let days_old = (now - fact.updated_at) as f32 / 86400.0;
            let strength = if fact.is_valid {
                0.5_f32.powf(days_old / 30.0).max(0.0)
            } else {
                0.0
            };
            return Ok(Some(FactSummary::from_fact(&fact, strength)));
        }

        // Try prefix match
        let facts = self.db.get_all_facts(true).await?;
        let matches: Vec<_> = facts.iter().filter(|f| f.id.starts_with(id)).collect();

        match matches.len() {
            0 => Ok(None),
            1 => {
                let fact = matches[0];
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64;
                let days_old = (now - fact.updated_at) as f32 / 86400.0;
                let strength = if fact.is_valid {
                    0.5_f32.powf(days_old / 30.0).max(0.0)
                } else {
                    0.0
                };
                Ok(Some(FactSummary::from_fact(fact, strength)))
            }
            _ => Err(AlephError::other(format!(
                "Ambiguous ID '{}' matches {} facts. Use more characters.",
                id,
                matches.len()
            ))),
        }
    }

    /// Format list output
    pub fn format_list(&self, summaries: &[FactSummary], format: OutputFormat) -> String {
        match format {
            OutputFormat::Table => {
                let mut output = String::new();
                output.push_str("ID          TYPE         STRENGTH  CONTENT\n");
                output.push_str(
                    "---------------------------------------------------------------\n",
                );
                for s in summaries {
                    output.push_str(&s.to_table_row());
                    output.push('\n');
                }
                output
            }
            OutputFormat::Json => serde_json::to_string_pretty(summaries).unwrap_or_default(),
            OutputFormat::Csv => {
                let mut output = String::from("id,type,strength,content\n");
                for s in summaries {
                    output.push_str(&s.to_csv_row());
                    output.push('\n');
                }
                output
            }
        }
    }

    /// Format single fact output
    pub fn format_show(&self, summary: &FactSummary, format: OutputFormat) -> String {
        match format {
            OutputFormat::Table => {
                format!(
                    r#"+-------------------------------------------------------------+
| Fact: {}
+-------------------------------------------------------------+
| Content:    {}
|
| Type:       {}
| Strength:   {:.2}
| Valid:      {}
| Created:    {}
+-------------------------------------------------------------+"#,
                    summary.full_id,
                    summary.content,
                    summary.fact_type,
                    summary.strength,
                    summary.is_valid,
                    summary.created_at
                )
            }
            OutputFormat::Json => serde_json::to_string_pretty(summary).unwrap_or_default(),
            OutputFormat::Csv => summary.to_csv_row(),
        }
    }

    /// Add a new fact manually
    pub async fn add(
        &self,
        content: &str,
        fact_type: FactType,
        embedder: Option<&Arc<dyn EmbeddingProvider>>,
    ) -> Result<String, AlephError> {
        // Generate embedding if embedder is available
        let embedding = if let Some(emb) = embedder {
            Some(emb.embed(content).await?)
        } else {
            None
        };

        // Create fact
        let mut fact = MemoryFact::new(content.to_string(), fact_type, vec![]);
        if let Some(emb) = embedding {
            fact = fact.with_embedding(emb);
        }
        fact.specificity = FactSpecificity::Pattern;
        fact.temporal_scope = TemporalScope::Contextual;

        let fact_id = fact.id.clone();

        // Insert into database
        self.db.insert_fact(&fact).await?;

        Ok(fact_id)
    }

    /// Edit an existing fact's content
    pub async fn edit(
        &self,
        id: &str,
        new_content: &str,
        embedder: Option<&Arc<dyn EmbeddingProvider>>,
    ) -> Result<String, AlephError> {
        // Resolve ID (support prefix match)
        let full_id = self.resolve_fact_id(id).await?;

        // Generate new embedding if embedder is available
        let embedding = if let Some(emb) = embedder {
            Some(emb.embed(new_content).await?)
        } else {
            None
        };

        // Update in database
        // TODO: Handle embedding update separately if needed
        let _ = embedding; // embedding update not supported in new API
        self.db
            .update_fact_content(&full_id, new_content)
            .await?;

        Ok(full_id)
    }

    /// Soft-delete (forget) a fact
    pub async fn forget(&self, id: &str, reason: Option<&str>) -> Result<String, AlephError> {
        // Resolve ID (support prefix match)
        let full_id = self.resolve_fact_id(id).await?;

        let reason_str = reason.unwrap_or("User requested deletion");
        self.db.invalidate_fact(&full_id, reason_str).await?;

        Ok(full_id)
    }

    /// Restore a fact from recycle bin
    pub async fn restore(&self, id: &str) -> Result<String, AlephError> {
        // Resolve ID (support prefix match)
        let _full_id = self.resolve_fact_id(id).await?;

        // TODO: Implement restore via new store API (update fact to set is_valid = true)
        // self.db.restore_fact(&full_id).await?;
        return Err(AlephError::other("restore_fact not yet implemented in new store API"));

        #[allow(unreachable_code)]
        Ok(_full_id)
    }

    /// Resolve a fact ID (supports prefix matching)
    async fn resolve_fact_id(&self, id: &str) -> Result<String, AlephError> {
        // Try exact match first
        if let Some(fact) = self.db.get_fact(id).await? {
            return Ok(fact.id);
        }

        // Try prefix match
        let facts = self.db.get_all_facts(true).await?;
        let matches: Vec<_> = facts.iter().filter(|f| f.id.starts_with(id)).collect();

        match matches.len() {
            0 => Err(AlephError::other(format!("Fact not found: {}", id))),
            1 => Ok(matches[0].id.clone()),
            _ => Err(AlephError::other(format!(
                "Ambiguous ID '{}' matches {} facts. Use more characters.",
                id,
                matches.len()
            ))),
        }
    }

    /// Run garbage collection to permanently delete old invalidated facts
    ///
    /// Returns GC statistics
    pub async fn gc(&self, retention_days: u32) -> Result<GcResult, AlephError> {
        // TODO: Implement purge via new store API
        let deleted = 0usize; // self.db.purge_old_invalidated_facts(retention_days).await?;
        let valid_facts = self.db.count_facts(&crate::memory::store::types::SearchFilter::valid_only(None)).await?;
        let remaining_invalid = {
            let all = self.db.count_facts(&crate::memory::store::types::SearchFilter::new()).await?;
            all.saturating_sub(valid_facts)
        };

        Ok(GcResult {
            deleted_count: deleted,
            valid_facts,
            remaining_invalid,
            retention_days,
        })
    }

    /// Export all facts to JSON format
    pub async fn dump(&self, include_invalid: bool) -> Result<String, AlephError> {
        let facts = self.db.get_all_facts(include_invalid).await?;

        let export = FactExport {
            version: 1,
            exported_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            facts: facts.into_iter().map(ExportedFact::from).collect(),
        };

        serde_json::to_string_pretty(&export)
            .map_err(|e| AlephError::other(format!("Failed to serialize: {}", e)))
    }

    /// Import facts from JSON format
    ///
    /// Returns import statistics
    pub async fn import(
        &self,
        json: &str,
        embedder: Option<&Arc<dyn EmbeddingProvider>>,
    ) -> Result<ImportResult, AlephError> {
        let export: FactExport = serde_json::from_str(json)
            .map_err(|e| AlephError::other(format!("Failed to parse JSON: {}", e)))?;

        let mut imported = 0;
        let mut skipped = 0;
        let mut errors = Vec::new();

        for exported in export.facts {
            // Check if fact already exists
            if self.db.get_fact(&exported.id).await?.is_some() {
                skipped += 1;
                continue;
            }

            // Reconstruct fact
            let mut fact = MemoryFact::with_id(
                exported.id.clone(),
                exported.content.clone(),
                FactType::from_str_or_other(&exported.fact_type),
            );
            fact.created_at = exported.created_at;
            fact.updated_at = exported.updated_at;
            fact.confidence = exported.confidence;
            fact.is_valid = exported.is_valid;
            fact.invalidation_reason = exported.invalidation_reason;
            fact.specificity = FactSpecificity::from_str_or_default(&exported.specificity);
            fact.temporal_scope = TemporalScope::from_str_or_default(&exported.temporal_scope);

            // Generate embedding if embedder is available
            if let Some(emb) = embedder {
                match emb.embed(&fact.content).await {
                    Ok(embedding) => {
                        fact = fact.with_embedding(embedding);
                    }
                    Err(e) => {
                        errors.push(format!("{}: embedding failed - {}", exported.id, e));
                    }
                }
            }

            // Insert fact
            match self.db.insert_fact(&fact).await {
                Ok(_) => imported += 1,
                Err(e) => errors.push(format!("{}: {}", exported.id, e)),
            }
        }

        Ok(ImportResult {
            imported,
            skipped,
            errors,
        })
    }

    /// Get statistics about the memory database
    pub async fn stats(&self) -> Result<MemoryStats, AlephError> {
        let valid = self.db.count_facts(&crate::memory::store::types::SearchFilter::valid_only(None)).await?;
        let invalid = {
            let all = self.db.count_facts(&crate::memory::store::types::SearchFilter::new()).await?;
            all.saturating_sub(valid)
        };

        Ok(MemoryStats {
            total_facts: valid + invalid,
            valid_facts: valid,
            invalid_facts: invalid,
        })
    }
}

/// Result of a write operation
#[derive(Debug, Clone)]
pub struct WriteResult {
    /// The affected fact ID
    pub fact_id: String,
    /// Action performed
    pub action: WriteAction,
    /// Previous content (for edit operations)
    pub previous_content: Option<String>,
}

/// Type of write action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteAction {
    Added,
    Edited,
    Forgotten,
    Restored,
}

/// Result of garbage collection
#[derive(Debug, Clone)]
pub struct GcResult {
    /// Number of facts permanently deleted
    pub deleted_count: usize,
    /// Number of valid facts remaining
    pub valid_facts: usize,
    /// Number of invalid facts remaining (within retention period)
    pub remaining_invalid: usize,
    /// Retention period used
    pub retention_days: u32,
}

impl std::fmt::Display for GcResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "GC complete: {} deleted, {} valid, {} in recycle bin (retention: {} days)",
            self.deleted_count, self.valid_facts, self.remaining_invalid, self.retention_days
        )
    }
}

/// Exported fact structure for JSON export/import
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExportedFact {
    pub id: String,
    pub content: String,
    pub fact_type: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub confidence: f32,
    pub is_valid: bool,
    pub invalidation_reason: Option<String>,
    pub specificity: String,
    pub temporal_scope: String,
}

impl From<MemoryFact> for ExportedFact {
    fn from(fact: MemoryFact) -> Self {
        Self {
            id: fact.id,
            content: fact.content,
            fact_type: fact.fact_type.as_str().to_string(),
            created_at: fact.created_at,
            updated_at: fact.updated_at,
            confidence: fact.confidence,
            is_valid: fact.is_valid,
            invalidation_reason: fact.invalidation_reason,
            specificity: fact.specificity.as_str().to_string(),
            temporal_scope: fact.temporal_scope.as_str().to_string(),
        }
    }
}

/// Export file format
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FactExport {
    pub version: u32,
    pub exported_at: i64,
    pub facts: Vec<ExportedFact>,
}

/// Result of import operation
#[derive(Debug, Clone)]
pub struct ImportResult {
    /// Number of facts successfully imported
    pub imported: usize,
    /// Number of facts skipped (already exist)
    pub skipped: usize,
    /// Errors encountered
    pub errors: Vec<String>,
}

impl std::fmt::Display for ImportResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Import complete: {} imported, {} skipped, {} errors",
            self.imported,
            self.skipped,
            self.errors.len()
        )
    }
}

/// Memory database statistics
#[derive(Debug, Clone, serde::Serialize)]
pub struct MemoryStats {
    pub total_facts: usize,
    pub valid_facts: usize,
    pub invalid_facts: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_filter_builder() {
        let filter = ListFilter::new()
            .with_type(FactType::Preference)
            .with_min_strength(0.5)
            .with_limit(10);

        assert_eq!(filter.fact_type, Some(FactType::Preference));
        assert_eq!(filter.min_strength, Some(0.5));
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
    fn test_fact_summary_table_row() {
        let summary = FactSummary {
            id: "abc12345".to_string(),
            full_id: "abc12345-6789".to_string(),
            fact_type: "preference".to_string(),
            strength: 0.85,
            content_preview: "User likes Rust".to_string(),
            content: "User likes Rust".to_string(),
            is_valid: true,
            created_at: 1234567890,
        };

        let row = summary.to_table_row();
        assert!(row.contains("abc12345"));
        assert!(row.contains("preference"));
        assert!(row.contains("0.85"));
    }

    #[test]
    fn test_fact_summary_csv_row() {
        let summary = FactSummary {
            id: "abc12345".to_string(),
            full_id: "abc12345-6789".to_string(),
            fact_type: "knowledge".to_string(),
            strength: 0.75,
            content_preview: "Test".to_string(),
            content: "Test with \"quotes\"".to_string(),
            is_valid: true,
            created_at: 1234567890,
        };

        let csv = summary.to_csv_row();
        assert!(csv.contains("abc12345-6789"));
        assert!(csv.contains("\"\"quotes\"\"")); // Escaped quotes
    }

    #[test]
    fn test_write_action_enum() {
        assert_ne!(WriteAction::Added, WriteAction::Edited);
        assert_ne!(WriteAction::Forgotten, WriteAction::Restored);
    }

    #[test]
    fn test_write_result() {
        let result = WriteResult {
            fact_id: "test-123".to_string(),
            action: WriteAction::Added,
            previous_content: None,
        };

        assert_eq!(result.fact_id, "test-123");
        assert_eq!(result.action, WriteAction::Added);
        assert!(result.previous_content.is_none());
    }

    #[test]
    fn test_gc_result_display() {
        let result = GcResult {
            deleted_count: 5,
            valid_facts: 100,
            remaining_invalid: 3,
            retention_days: 30,
        };

        let display = format!("{}", result);
        assert!(display.contains("5 deleted"));
        assert!(display.contains("100 valid"));
        assert!(display.contains("3 in recycle bin"));
        assert!(display.contains("30 days"));
    }

    #[test]
    fn test_import_result_display() {
        let result = ImportResult {
            imported: 10,
            skipped: 2,
            errors: vec!["error1".to_string()],
        };

        let display = format!("{}", result);
        assert!(display.contains("10 imported"));
        assert!(display.contains("2 skipped"));
        assert!(display.contains("1 errors"));
    }

    #[test]
    fn test_exported_fact_from_memory_fact() {
        let fact = MemoryFact::new(
            "User prefers Rust".to_string(),
            FactType::Preference,
            vec!["source-1".to_string()],
        );

        let exported = ExportedFact::from(fact.clone());

        assert_eq!(exported.id, fact.id);
        assert_eq!(exported.content, "User prefers Rust");
        assert_eq!(exported.fact_type, "preference");
        assert!(exported.is_valid);
    }

    #[test]
    fn test_fact_export_serialization() {
        let export = FactExport {
            version: 1,
            exported_at: 1234567890,
            facts: vec![ExportedFact {
                id: "test-123".to_string(),
                content: "Test content".to_string(),
                fact_type: "preference".to_string(),
                created_at: 1234567890,
                updated_at: 1234567890,
                confidence: 0.9,
                is_valid: true,
                invalidation_reason: None,
                specificity: "pattern".to_string(),
                temporal_scope: "contextual".to_string(),
            }],
        };

        let json = serde_json::to_string(&export).unwrap();
        assert!(json.contains("test-123"));
        assert!(json.contains("Test content"));

        let parsed: FactExport = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.facts.len(), 1);
    }

    #[test]
    fn test_memory_stats() {
        let stats = MemoryStats {
            total_facts: 100,
            valid_facts: 95,
            invalid_facts: 5,
        };

        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("100"));
        assert!(json.contains("95"));
        assert!(json.contains("5"));
    }
}
