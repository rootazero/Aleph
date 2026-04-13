//! NoteSearchResult — search results from notes-based retrieval.
//!
//! Carries full content and metadata, with `to_memory_fact()` bridge to
//! produce `MemoryFact` / `ScoredFact` for downstream compatibility.

use serde::{Deserialize, Serialize};

use crate::memory::context::{MemoryFact, NoteType};
use crate::memory::store::types::ScoredFact;

/// Search result carrying full content from notes-based retrieval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteSearchResult {
    pub path: String,
    pub filename: String,
    pub category: String,
    pub tags: Vec<String>,
    pub content: String,
    pub score: f32,
    pub created_at: i64,
    pub updated_at: i64,
}

impl NoteSearchResult {
    /// Convert to MemoryFact for downstream compatibility.
    ///
    /// Uses `path` as the ID and formats `note://{path}` as the VFS path.
    /// Tags are forwarded as source_memory_ids for traceability.
    /// Defaults: is_valid=true.
    pub fn to_memory_fact(&self, agent_id: &str) -> MemoryFact {
        let note_type = NoteType::from_str_or_other(&self.category);
        let mut fact = MemoryFact::new(self.content.clone(), note_type, self.tags.clone());
        fact.id = self.path.clone();
        fact.path = format!("note://{}", self.path);
        fact.agent = agent_id.to_string();
        fact.created_at = self.created_at;
        fact.updated_at = self.updated_at;
        fact.is_valid = true;
        fact
    }

    pub fn to_scored_fact(&self, agent_id: &str) -> ScoredFact {
        ScoredFact {
            fact: self.to_memory_fact(agent_id),
            score: self.score,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_memory_fact_uses_path_as_id() {
        let result = NoteSearchResult {
            path: "preference/editor".to_string(),
            filename: "editor".to_string(),
            category: "preference".to_string(),
            tags: vec!["editor".to_string()],
            content: "The user prefers Vim".to_string(),
            score: 0.95,
            created_at: 1000,
            updated_at: 2000,
        };

        let fact = result.to_memory_fact("default");
        assert_eq!(fact.id, "preference/editor");
        assert_eq!(fact.path, "note://preference/editor");
        assert_eq!(fact.content, "The user prefers Vim");
        assert_eq!(fact.agent, "default");
        assert!(fact.is_valid);
    }

    #[test]
    fn to_scored_fact_preserves_score() {
        let result = NoteSearchResult {
            path: "wiki/rust".to_string(),
            filename: "rust".to_string(),
            category: "wiki".to_string(),
            tags: vec![],
            content: "Rust content".to_string(),
            score: 0.87,
            created_at: 100,
            updated_at: 200,
        };

        let scored = result.to_scored_fact("default");
        assert!((scored.score - 0.87).abs() < f32::EPSILON);
        assert_eq!(scored.fact.content, "Rust content");
    }

    #[test]
    fn tags_forwarded_as_source_memory_ids() {
        let result = NoteSearchResult {
            path: "a/b".to_string(),
            filename: "b".to_string(),
            category: "a".to_string(),
            tags: vec!["tag1".to_string(), "tag2".to_string()],
            content: "x".to_string(),
            score: 0.5,
            created_at: 0,
            updated_at: 0,
        };

        let fact = result.to_memory_fact("default");
        assert_eq!(
            fact.source_memory_ids,
            vec!["tag1".to_string(), "tag2".to_string()]
        );
    }
}
