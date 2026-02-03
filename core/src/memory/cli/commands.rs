//! CLI Commands for Memory Management
//!
//! Provides command implementations for listing, showing, and searching facts.

use crate::error::AetherError;
use crate::memory::context::{FactType, MemoryFact};
use crate::memory::database::VectorDatabase;
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
    db: Arc<VectorDatabase>,
}

impl MemoryCommands {
    /// Create new commands instance
    pub fn new(db: Arc<VectorDatabase>) -> Self {
        Self { db }
    }

    /// List facts with optional filtering
    pub async fn list(&self, filter: ListFilter) -> Result<Vec<FactSummary>, AetherError> {
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
    pub async fn show(&self, id: &str) -> Result<Option<FactSummary>, AetherError> {
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
            _ => Err(AetherError::other(format!(
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
}
