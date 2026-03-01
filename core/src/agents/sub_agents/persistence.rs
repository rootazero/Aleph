//! FactsDB persistence for SubAgentRun
//!
//! This module provides helpers for converting SubAgentRun to/from MemoryFact
//! for storage in the FactsDB memory system.

use crate::error::{AlephError, Result};
use crate::memory::context::{
    FactSource, FactSpecificity, FactType, MemoryCategory, MemoryFact, MemoryLayer, TemporalScope,
};

use super::run::SubAgentRun;

/// Helper for converting SubAgentRun to/from MemoryFact
pub struct SubAgentRunFact;

impl SubAgentRunFact {
    /// Convert a SubAgentRun to a MemoryFact for storage
    pub fn from_run(run: &SubAgentRun) -> MemoryFact {
        let content = serde_json::to_string(run).unwrap_or_default();
        let id = format!("subagent:run:{}", run.run_id);

        MemoryFact {
            id,
            content,
            fact_type: FactType::SubagentRun,
            embedding: None,
            source_memory_ids: vec![],
            created_at: run.created_at / 1000, // Convert ms to seconds
            updated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
                .as_secs() as i64,
            confidence: 1.0,
            is_valid: true,
            invalidation_reason: None,
            decay_invalidated_at: None,
            specificity: FactSpecificity::Instance,
            temporal_scope: TemporalScope::Ephemeral,
            similarity_score: None,
            path: String::new(),
            layer: MemoryLayer::L2Detail,
            category: MemoryCategory::Cases,
            fact_source: FactSource::Extracted,
            content_hash: String::new(),
            parent_path: String::new(),
            embedding_model: String::new(),
            namespace: "owner".to_string(),
            workspace: "default".to_string(),
            tier: crate::memory::context::MemoryTier::ShortTerm,
            scope: crate::memory::context::MemoryScope::Global,
            persona_id: None,
            strength: 1.0,
            access_count: 0,
            last_accessed_at: None,
        }
    }

    /// Convert a MemoryFact back to SubAgentRun
    pub fn to_run(fact: &MemoryFact) -> Result<SubAgentRun> {
        serde_json::from_str(&fact.content)
            .map_err(|e| AlephError::config(format!("Failed to deserialize SubAgentRun: {}", e)))
    }

    /// Generate fact ID from run_id
    pub fn fact_id(run_id: &str) -> String {
        format!("subagent:run:{}", run_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::SessionKey;

    #[test]
    fn test_run_to_fact_conversion() {
        let run = SubAgentRun::new(
            SessionKey::main("s1"),
            SessionKey::main("p1"),
            "Test task",
            "explore",
        );
        let fact = SubAgentRunFact::from_run(&run);

        assert!(fact.id.starts_with("subagent:run:"));
        assert_eq!(fact.fact_type, FactType::SubagentRun);
        assert!(fact.content.contains("Test task"));
        assert_eq!(fact.specificity, FactSpecificity::Instance);
        assert_eq!(fact.temporal_scope, TemporalScope::Ephemeral);
    }

    #[test]
    fn test_fact_to_run_conversion() {
        let run = SubAgentRun::new(
            SessionKey::main("s1"),
            SessionKey::main("p1"),
            "Test task",
            "explore",
        );
        let fact = SubAgentRunFact::from_run(&run);
        let restored = SubAgentRunFact::to_run(&fact).unwrap();

        assert_eq!(restored.run_id, run.run_id);
        assert_eq!(restored.task, run.task);
        assert_eq!(restored.status, run.status);
        assert_eq!(restored.agent_type, run.agent_type);
    }

    #[test]
    fn test_fact_id_generation() {
        let id = SubAgentRunFact::fact_id("abc-123");
        assert_eq!(id, "subagent:run:abc-123");
    }
}
