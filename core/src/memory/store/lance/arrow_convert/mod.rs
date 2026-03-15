//! Arrow RecordBatch <-> domain type conversions for LanceDB.
//!
//! Provides serialisation (domain -> Arrow) and deserialisation (Arrow -> domain)
//! for the four LanceDB tables: `facts`, `graph_nodes`, `graph_edges`, and
//! `memories`.

mod helpers;

mod edge;
mod fact;
mod memory;
mod node;

pub use edge::{graph_edges_to_record_batch, record_batch_to_graph_edges};
pub use fact::{facts_to_record_batch, record_batch_to_facts};
pub use memory::{memories_to_record_batch, record_batch_to_memories};
pub use node::{graph_nodes_to_record_batch, record_batch_to_graph_nodes};

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{
        ContextAnchor, FactSource, FactSpecificity, FactType, MemoryCategory, MemoryEntry,
        MemoryFact, MemoryLayer, MemoryScope, MemoryTier, TemporalScope,
    };
    use crate::memory::store::{GraphEdge, GraphNode};
    use helpers::normalize_embedding;

    /// Helper: create a test MemoryFact with an embedding.
    fn make_fact_with_embedding() -> MemoryFact {
        let mut fact = MemoryFact::new(
            "User prefers Rust for systems programming".to_string(),
            FactType::Preference,
            vec!["mem-001".to_string(), "mem-002".to_string()],
        );
        fact.id = "fact-test-001".to_string();
        fact.confidence = 0.95;
        fact.specificity = FactSpecificity::Pattern;
        fact.temporal_scope = TemporalScope::Permanent;
        fact.fact_source = FactSource::Extracted;
        fact.content_hash = "abc123".to_string();
        fact.embedding_model = "BAAI/bge-m3".to_string();
        fact.embedding = Some(vec![0.1_f32; 1024]);
        fact.path = "aleph://user/preferences/coding/".to_string();
        fact.parent_path = "aleph://user/preferences/".to_string();
        fact.created_at = 1700000000;
        fact.updated_at = 1700000100;
        fact
    }

    /// Helper: create a test MemoryFact without embedding.
    fn make_fact_no_embedding() -> MemoryFact {
        let mut fact = MemoryFact::new(
            "User is learning WebAssembly".to_string(),
            FactType::Learning,
            vec!["mem-003".to_string()],
        );
        fact.id = "fact-test-002".to_string();
        fact.confidence = 0.8;
        fact.is_valid = false;
        fact.invalidation_reason = Some("superseded by newer fact".to_string());
        fact.decay_invalidated_at = Some(1700001000);
        fact.content_hash = "def456".to_string();
        fact.embedding_model = "".to_string();
        fact.created_at = 1700000200;
        fact.updated_at = 1700000300;
        fact
    }

    #[test]
    fn test_fact_roundtrip() {
        let original = make_fact_with_embedding();
        let batch = facts_to_record_batch(std::slice::from_ref(&original)).expect("to_batch");
        assert_eq!(batch.num_rows(), 1);

        let recovered = record_batch_to_facts(&batch).expect("from_batch");
        assert_eq!(recovered.len(), 1);

        let f = &recovered[0];
        assert_eq!(f.id, original.id);
        assert_eq!(f.content, original.content);
        assert_eq!(f.fact_type, original.fact_type);
        assert_eq!(f.fact_source, original.fact_source);
        assert_eq!(f.specificity, original.specificity);
        assert_eq!(f.temporal_scope, original.temporal_scope);
        assert_eq!(f.path, original.path);
        assert_eq!(f.parent_path, original.parent_path);
        assert_eq!(f.content_hash, original.content_hash);
        assert_eq!(f.embedding_model, original.embedding_model);
        assert!((f.confidence - original.confidence).abs() < f32::EPSILON);
        assert_eq!(f.is_valid, original.is_valid);
        assert_eq!(f.invalidation_reason, original.invalidation_reason);
        assert_eq!(f.created_at, original.created_at);
        assert_eq!(f.updated_at, original.updated_at);
        assert_eq!(f.decay_invalidated_at, original.decay_invalidated_at);
        assert_eq!(f.source_memory_ids, original.source_memory_ids);

        // Embedding roundtrip
        let emb = f.embedding.as_ref().expect("should have embedding");
        assert_eq!(emb.len(), 1024);
        assert!((emb[0] - 0.1).abs() < f32::EPSILON);
    }

    #[test]
    fn test_fact_roundtrip_no_embedding() {
        let original = make_fact_no_embedding();
        let batch = facts_to_record_batch(std::slice::from_ref(&original)).expect("to_batch");
        assert_eq!(batch.num_rows(), 1);

        let recovered = record_batch_to_facts(&batch).expect("from_batch");
        assert_eq!(recovered.len(), 1);

        let f = &recovered[0];
        assert_eq!(f.id, original.id);
        assert_eq!(f.content, original.content);
        assert_eq!(f.fact_type, original.fact_type);
        assert!(!f.is_valid);
        assert_eq!(
            f.invalidation_reason,
            Some("superseded by newer fact".to_string())
        );
        assert_eq!(f.decay_invalidated_at, Some(1700001000));
        assert!(f.embedding.is_none());
    }

    #[test]
    fn fact_roundtrip_preserves_layer_and_category() {
        let mut original = make_fact_with_embedding();
        original.layer = MemoryLayer::L1Overview;
        original.category = MemoryCategory::Patterns;

        let batch = facts_to_record_batch(std::slice::from_ref(&original)).expect("to_batch");
        let recovered = record_batch_to_facts(&batch).expect("from_batch");

        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].layer, MemoryLayer::L1Overview);
        assert_eq!(recovered[0].category, MemoryCategory::Patterns);
    }

    #[test]
    fn test_fact_batch_multiple() {
        let facts = vec![make_fact_with_embedding(), make_fact_no_embedding()];
        let batch = facts_to_record_batch(&facts).expect("to_batch");
        assert_eq!(batch.num_rows(), 2);

        let recovered = record_batch_to_facts(&batch).expect("from_batch");
        assert_eq!(recovered.len(), 2);
        assert_eq!(recovered[0].id, "fact-test-001");
        assert_eq!(recovered[1].id, "fact-test-002");
        assert!(recovered[0].embedding.is_some());
        assert!(recovered[1].embedding.is_none());
    }

    #[test]
    fn test_fact_empty_batch() {
        let batch = facts_to_record_batch(&[]).expect("empty to_batch");
        assert_eq!(batch.num_rows(), 0);
        let recovered = record_batch_to_facts(&batch).expect("empty from_batch");
        assert!(recovered.is_empty());
    }

    #[test]
    fn test_graph_node_roundtrip() {
        let node = GraphNode {
            id: "gn_test_001".to_string(),
            name: "Rust".to_string(),
            kind: "language".to_string(),
            aliases: vec!["rust-lang".to_string(), "Rust Programming".to_string()],
            metadata_json: r#"{"category":"systems"}"#.to_string(),
            decay_score: 0.95,
            created_at: 1700000000,
            updated_at: 1700000100,
            workspace: "default".to_string(),
        };

        let batch = graph_nodes_to_record_batch(std::slice::from_ref(&node)).expect("to_batch");
        assert_eq!(batch.num_rows(), 1);

        let recovered = record_batch_to_graph_nodes(&batch).expect("from_batch");
        assert_eq!(recovered.len(), 1);

        let n = &recovered[0];
        assert_eq!(n.id, node.id);
        assert_eq!(n.name, node.name);
        assert_eq!(n.kind, node.kind);
        assert_eq!(n.aliases, node.aliases);
        assert_eq!(n.metadata_json, node.metadata_json);
        assert!((n.decay_score - node.decay_score).abs() < f32::EPSILON);
        assert_eq!(n.created_at, node.created_at);
        assert_eq!(n.updated_at, node.updated_at);
    }

    #[test]
    fn test_graph_node_empty_aliases() {
        let node = GraphNode {
            id: "gn_test_002".to_string(),
            name: "WebAssembly".to_string(),
            kind: "technology".to_string(),
            aliases: vec![],
            metadata_json: String::new(),
            decay_score: 1.0,
            created_at: 1700000000,
            updated_at: 1700000000,
            workspace: "default".to_string(),
        };

        let batch = graph_nodes_to_record_batch(std::slice::from_ref(&node)).expect("to_batch");
        let recovered = record_batch_to_graph_nodes(&batch).expect("from_batch");
        assert_eq!(recovered.len(), 1);
        assert!(recovered[0].aliases.is_empty());
        assert!(recovered[0].metadata_json.is_empty());
    }

    #[test]
    fn test_graph_edge_roundtrip() {
        let edge = GraphEdge {
            id: "ge_test_001".to_string(),
            from_id: "gn_001".to_string(),
            to_id: "gn_002".to_string(),
            relation: "uses".to_string(),
            weight: 2.5,
            confidence: 0.9,
            context_key: "app:com.test|window:doc".to_string(),
            decay_score: 0.85,
            created_at: 1700000000,
            updated_at: 1700000100,
            last_seen_at: 1700000200,
            workspace: "default".to_string(),
        };

        let batch = graph_edges_to_record_batch(std::slice::from_ref(&edge)).expect("to_batch");
        assert_eq!(batch.num_rows(), 1);

        let recovered = record_batch_to_graph_edges(&batch).expect("from_batch");
        assert_eq!(recovered.len(), 1);

        let e = &recovered[0];
        assert_eq!(e.id, edge.id);
        assert_eq!(e.from_id, edge.from_id);
        assert_eq!(e.to_id, edge.to_id);
        assert_eq!(e.relation, edge.relation);
        assert!((e.weight - edge.weight).abs() < f32::EPSILON);
        assert!((e.confidence - edge.confidence).abs() < f32::EPSILON);
        assert_eq!(e.context_key, edge.context_key);
        assert!((e.decay_score - edge.decay_score).abs() < f32::EPSILON);
        assert_eq!(e.created_at, edge.created_at);
        assert_eq!(e.updated_at, edge.updated_at);
        assert_eq!(e.last_seen_at, edge.last_seen_at);
    }

    #[test]
    fn test_graph_edge_empty_batch() {
        let batch = graph_edges_to_record_batch(&[]).expect("empty to_batch");
        assert_eq!(batch.num_rows(), 0);
        let recovered = record_batch_to_graph_edges(&batch).expect("empty from_batch");
        assert!(recovered.is_empty());
    }

    #[test]
    fn test_memory_entry_roundtrip() {
        let context = ContextAnchor {
            window_title: "Project Plan".to_string(),
            timestamp: 1700000000,
            session_id: "topic-abc".to_string(),
        };
        let entry = MemoryEntry {
            id: "mem-test-001".to_string(),
            context,
            user_input: "What is Rust?".to_string(),
            ai_output: "Rust is a systems programming language.".to_string(),
            embedding: Some(vec![0.5_f32; 768]),
            namespace: "owner".to_string(),
            workspace: "default".to_string(),
            similarity_score: None,
        };

        let batch = memories_to_record_batch(std::slice::from_ref(&entry)).expect("to_batch");
        assert_eq!(batch.num_rows(), 1);

        let recovered = record_batch_to_memories(&batch).expect("from_batch");
        assert_eq!(recovered.len(), 1);

        let m = &recovered[0];
        assert_eq!(m.id, entry.id);
        assert_eq!(m.context.window_title, entry.context.window_title);
        assert_eq!(m.context.timestamp, entry.context.timestamp);
        assert_eq!(m.context.session_id, "topic-abc");
        assert_eq!(m.user_input, entry.user_input);
        assert_eq!(m.ai_output, entry.ai_output);

        let emb = m.embedding.as_ref().expect("should have embedding");
        assert_eq!(emb.len(), 768);
        assert!((emb[0] - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_memory_entry_no_embedding() {
        let context = ContextAnchor {
            window_title: "Window".to_string(),
            timestamp: 1700000500,
            session_id: crate::memory::context::NO_SESSION.to_string(),
        };
        let entry = MemoryEntry {
            id: "mem-test-002".to_string(),
            context,
            user_input: "Hello".to_string(),
            ai_output: "Hi there!".to_string(),
            embedding: None,
            namespace: "owner".to_string(),
            workspace: "default".to_string(),
            similarity_score: None,
        };

        let batch = memories_to_record_batch(std::slice::from_ref(&entry)).expect("to_batch");
        let recovered = record_batch_to_memories(&batch).expect("from_batch");
        assert_eq!(recovered.len(), 1);
        assert!(recovered[0].embedding.is_none());
        assert_eq!(recovered[0].context.session_id, "none");
    }

    #[test]
    fn test_memory_entry_empty_batch() {
        let batch = memories_to_record_batch(&[]).expect("empty to_batch");
        assert_eq!(batch.num_rows(), 0);
        let recovered = record_batch_to_memories(&batch).expect("empty from_batch");
        assert!(recovered.is_empty());
    }

    #[test]
    fn fact_roundtrip_preserves_acma_fields() {
        let mut fact = MemoryFact::new("test acma".into(), FactType::Preference, vec![]);
        fact.tier = MemoryTier::LongTerm;
        fact.scope = MemoryScope::Persona;
        fact.persona_id = Some("reviewer".to_string());
        fact.strength = 0.75;
        fact.access_count = 5;
        fact.last_accessed_at = Some(1700000000);
        let batch = facts_to_record_batch(std::slice::from_ref(&fact)).unwrap();
        let out = record_batch_to_facts(&batch).unwrap();
        assert_eq!(out[0].tier, MemoryTier::LongTerm);
        assert_eq!(out[0].scope, MemoryScope::Persona);
        assert_eq!(out[0].persona_id, Some("reviewer".to_string()));
        assert!((out[0].strength - 0.75).abs() < 0.001);
        assert_eq!(out[0].access_count, 5);
        assert_eq!(out[0].last_accessed_at, Some(1700000000));
    }

    #[test]
    fn fact_roundtrip_768_dim_embedding() {
        let mut fact = make_fact_no_embedding();
        fact.embedding = Some(vec![0.2_f32; 768]);
        fact.embedding_model = "nomic-embed-text".to_string();

        let batch = facts_to_record_batch(std::slice::from_ref(&fact)).unwrap();
        let out = record_batch_to_facts(&batch).unwrap();

        let emb = out[0].embedding.as_ref().expect("should have 768-dim embedding");
        assert_eq!(emb.len(), 768);
        assert!((emb[0] - 0.2).abs() < f32::EPSILON);
    }

    #[test]
    fn fact_roundtrip_1536_dim_embedding() {
        let mut fact = make_fact_no_embedding();
        fact.embedding = Some(vec![0.3_f32; 1536]);
        fact.embedding_model = "text-embedding-3-small".to_string();

        let batch = facts_to_record_batch(std::slice::from_ref(&fact)).unwrap();
        let out = record_batch_to_facts(&batch).unwrap();

        let emb = out[0].embedding.as_ref().expect("should have 1536-dim embedding");
        assert_eq!(emb.len(), 1536);
        assert!((emb[0] - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn fact_nonstandard_dim_truncation_512_drops() {
        // 512-dim is smaller than the minimum supported (768), should be dropped
        let mut fact = make_fact_no_embedding();
        fact.embedding = Some(vec![0.5_f32; 512]);

        let batch = facts_to_record_batch(std::slice::from_ref(&fact)).unwrap();
        let out = record_batch_to_facts(&batch).unwrap();

        assert!(out[0].embedding.is_none(), "512-dim should be dropped (< 768)");
    }

    #[test]
    fn fact_nonstandard_dim_truncation_900_to_768() {
        // 900-dim should truncate to 768
        let mut fact = make_fact_no_embedding();
        fact.embedding = Some(vec![1.0_f32; 900]);

        let batch = facts_to_record_batch(std::slice::from_ref(&fact)).unwrap();
        let out = record_batch_to_facts(&batch).unwrap();

        let emb = out[0].embedding.as_ref().expect("should have truncated embedding");
        assert_eq!(emb.len(), 768);
    }

    #[test]
    fn fact_nonstandard_dim_truncation_2048_to_1536() {
        // 2048-dim should truncate to 1536
        let mut fact = make_fact_no_embedding();
        fact.embedding = Some(vec![1.0_f32; 2048]);

        let batch = facts_to_record_batch(std::slice::from_ref(&fact)).unwrap();
        let out = record_batch_to_facts(&batch).unwrap();

        let emb = out[0].embedding.as_ref().expect("should have truncated embedding");
        assert_eq!(emb.len(), 1536);
    }

    #[test]
    fn fact_multi_dimension_coexistence() {
        // Two facts with different embedding dimensions should both survive roundtrip
        let mut fact_768 = make_fact_no_embedding();
        fact_768.id = "fact-768".to_string();
        fact_768.embedding = Some(vec![0.1_f32; 768]);

        let mut fact_1024 = make_fact_no_embedding();
        fact_1024.id = "fact-1024".to_string();
        fact_1024.embedding = Some(vec![0.2_f32; 1024]);

        let batch = facts_to_record_batch(&[fact_768, fact_1024]).unwrap();
        let out = record_batch_to_facts(&batch).unwrap();

        assert_eq!(out.len(), 2);
        let emb_768 = out[0].embedding.as_ref().expect("768-dim should survive");
        assert_eq!(emb_768.len(), 768);
        let emb_1024 = out[1].embedding.as_ref().expect("1024-dim should survive");
        assert_eq!(emb_1024.len(), 1024);
    }

    #[test]
    fn normalize_embedding_standard_dims() {
        // Standard dimensions should pass through unchanged
        assert_eq!(normalize_embedding(&vec![1.0; 768]).unwrap().len(), 768);
        assert_eq!(normalize_embedding(&vec![1.0; 1024]).unwrap().len(), 1024);
        assert_eq!(normalize_embedding(&vec![1.0; 1536]).unwrap().len(), 1536);
    }

    #[test]
    fn normalize_embedding_too_small() {
        assert!(normalize_embedding(&vec![1.0; 384]).is_none());
        assert!(normalize_embedding(&vec![1.0; 100]).is_none());
    }

    #[test]
    fn normalize_embedding_nonstandard() {
        // 900 -> 768
        assert_eq!(normalize_embedding(&vec![1.0; 900]).unwrap().len(), 768);
        // 1200 -> 1024
        assert_eq!(normalize_embedding(&vec![1.0; 1200]).unwrap().len(), 1024);
        // 2048 -> 1536
        assert_eq!(normalize_embedding(&vec![1.0; 2048]).unwrap().len(), 1536);
    }
}
